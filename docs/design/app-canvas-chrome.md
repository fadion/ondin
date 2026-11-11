# Canvas chrome, the top bar's menus, the command palette, batch rename, clean up, fill with content, the assets browser, find and replace, colour review, the cheatsheet, focus, private and present mode

Overlay chrome drawn during gestures (escaping clip, create box, size pill, Alt-drag ghost), the
pixel grid, the minimap, the contrast check mode, the vision simulation, the top bar's View/Snap/Tools menus and Settings button,
the command palette, the batch rename box, the clean up box, the fill with content box, the assets browser and credits, the find and replace box, the colour review card, the shortcut cheatsheet, focus mode, the peek, private mode and
present mode.
Part of §9.4.

## 9.4 Gesture chrome

### A layer leaving a frame draws outside its clip (`RenderOverrides::escape_clip`)

- A child dragged past its artboard's edge draws in full rather than clipped; the Alt-drag copy too
  (`escape_clip_ghosts`).
- Set **exactly when the release would reparent**, through the drop's own `move_destination` /
  `frame_covering`: still clipped means the drop stays in the frame.
- **Z-order is untouched**: the parent's clip layer is closed and reopened around the child. The walk
  tracks whether the layer is open; consequence — a semi-transparent frame composites in two pieces
  while a child leaves.
- Driven from the **gesture**, never derived from "has a transform override" (a resize has one and
  must stay clipped).

### The create box and size pill

- A create drag draws a `color::SELECT` 1.5px outline over the renderer's ghost of the shape (not for
  the Line tool). Both ends come from `canvas::create_box` — the transaction's own function — so Shift,
  Alt and the pixel grid are honoured.
- The size pill hangs under that box exactly as under a selection: one painter
  (`canvas::paint_size_pill`), one rounding (`canvas::size_label`), the box from `create_outline_box`
  (Line answers `None` and gets neither). The pill clears `HANDLE_PICK_PX`.

### The Alt-drag copy

While Alt is held during a move the original stays and the *copy* travels, previewed as a
`render::GhostNode` subtree (children and clip included). Selection outline, handles and size badge
are offset onto it by `canvas::alt_clone_offset`. The `SELECT_DIM` outline on the committed box now
marks the original that stays. Toggles live with the key.

## The pixel grid (`grid.rs`)

- A one-world-unit lattice drawn **over artwork and under every other overlay**. Overlay stack bottom
  up: grid, frame name tags, gesture chrome, ruler bars with guides.
- Appears at `grid::VISIBLE_AT_ZOOM` = 8.0 (800% on the readout; `camera.zoom` is device px per world
  unit). That also bounds the line count by construction.
- **Adaptive ink** (`prefs.canvas.grid_adaptive`, on by default). Each grid pixel is the canvas
  pixel under it moved 10 in CIE L* (`grid_ink::STEP`) with its hue kept: above L* 45
  (`grid_ink::DARKEN_ABOVE`) it darkens, by scaling its linear channels by `Y(L*−10)/Y`; at or
  below it lightens, mixing toward white in linear light by `(Y(L*+10) − Y)/(1 − Y)`. A fixed ink
  vanishes over a mid-tone fill; this one reads over every fill. `ondin_render::grid_ink`
  holds the rule (`adapt_rgba8`) and its compute pass, which `CanvasRenderer::shade_grid` runs
  every frame the grid shows, after the vision simulation (the grid is chrome over what is
  seen), into a third texture, `gridded`; `target` and `seen` are never written, so pans, patches
  and the eyedropper read true pixels. The egui overlays paint over that texture, so the grid
  still sits under every other overlay.
- **Fixed ink** (*Adaptive* off): one-device-pixel egui lines in `prefs.canvas.grid_ink`,
  straight alpha, default `CanvasPrefs::GRID_INK` (mid-grey at alpha 60). Where the pass cannot
  run, adaptive falls back to these lines: headless (no GPU) and under pixel preview, whose
  texture is one texel a unit.
- Not drawn under the ruler bars.
- Both inks place a line by one rule, `grid_ink::on_line` over `OndinApp::grid_placed`'s
  `Lattice` (world 0's texel modulo the pitch, the zoom as pitch, the artwork area as the region):
  a line takes the device pixel holding its position, a half rounding up, so switching *Adaptive*
  never moves a line. It picks the pixels `rulers::snap_across_axis` gave a one-device-pixel line
  (`the_lattice_picks_the_pixels_the_snapped_lines_did`).
- Showing the grid and snapping to it are separate switches.

## The minimap (`minimap.rs`)

*View ▸ Panels ▸ Minimap* (`ViewSwitch::Minimap`, *Show minimap* in the palette, `Ctrl+Shift+M`,
`prefs.view.minimap`, off by default). Not on the canvas menu: it is chrome-wide, like the layers pane.

- **The card.** An `egui::Area` at `Order::Middle`, pivoted at the canvas's bottom left, 12pt in
  (the status bar's margin): a `ui::menu_frame` 220pt wide with 6pt padding. The map's height
  follows the page content's aspect, the card clamped to 90–160pt; an empty page keeps the last
  height. The floating status bar moves right by the card's width + 8pt
  (`OndinApp::minimap_clearance`), so the two share the bottom line.
- **What it draws**, the current page flat. The walk goes down through frames and groups while a
  container's shorter side is at least 6pt on the map (at most 8 levels), so detail follows what
  the map can show rather than a fixed depth — a page built of unfilled section groups shows their
  contents, not blank boxes. Anything under 1pt on its longer side is left out, and a clipping frame
  cuts its children to its box. Each layer is a box of its preview world bounds (a live drag shows),
  except a frame, which is always its own rectangle: a layer being dragged out is still its child
  until the drop, and must not widen it. The colour is the topmost visible fill, else the topmost
  visible stroke: a bound colour resolved, a gradient as its first stop, an image as grey. An
  opened group with no paint draws nothing itself; an unfilled frame, or a container too small to
  open, is a faint neutral box. A text layer is a rounded bar through the middle 60% of its box.
  A mask draws nothing, as on the canvas, where it is not artwork.
  The selection's bounds are outlined in `color::SELECT`; the viewport is an accent box.
- **The fit** is the page content ∪ the viewport, padded 4% of the larger side, so the viewport box
  never leaves the map; an empty page fits the viewport at 2.4× its size.
- **Input.** A press outside the box centres the camera there and carries on as a drag; a press
  inside picks the box up. The canvas follows the drag frame by frame; the fit is frozen at the
  press, so the map holds still under the pointer, and a dashed ghost marks where the box started.
  A right-click during the drag puts the camera back. The wheel, any axis or modifier, and a pinch zoom
  around the world point under the pointer (`Camera::zoom_by` anchored at that point's canvas position,
  even off-screen). Double-click zooms to fit the page. Hovering a top-level frame outside the box
  shows its name as a tooltip, the only text the map shows. Cursor: grab over the box, grabbing
  while dragging.
- **None of it is an edit**: camera only, no history step, nothing in the file. egui gives a press
  on the card to the map's widget, so it never reaches the canvas and never deselects.
- **Placement in the frame.** `minimap_ui` runs before the canvas's `CentralPanel`, placed from last
  frame's `canvas_rect`, so a pan it makes reaches this frame's render. It is outside the `!focus`
  guard, so focus mode keeps it; present mode's early return hides it.

## The contrast check mode (`contrast.rs`)

*Tools ▸ Check contrast* (`Action::CheckContrast`, `menu::Item::CheckContrast`, `Group::View`, no
chord): the Tools menu's first row, and a palette row reading *Stop checking contrast* while on.
It measures the shown text layers, and with *Non-text* small shapes, against what is drawn behind
them with `ondin_export::contrast::check_with`, the measurement `ondin check contrast` runs
(`export.md`). `OndinApp::contrast` is view state: `reset_transient_state` turns it off whenever a
document opens, it is never saved or exported, and present mode's own branch never draws it.
Starting it ends the vision simulation, and starting that ends it, so the two bars never
stack.

- **Scope: one frame at most** (`contrast_target`), since measuring renders every layer's box
  three or four times and a page of several frames can take minutes. The selection when the mode starts, cut to
  the top-level frame of its first layer; with nothing selected, the top-level frame filling most
  of the view, else the page's first frame in reading order; a page with no frame is checked
  whole. The scope is fixed for the mode's life: selecting other layers afterwards does not move
  it. The bar names it in a pill; a click on the pill re-targets to the selection or the frame in
  view. Switching pages re-targets on the new page. Subjects are in reading order:
  `present::reading_order` over the page's top-level layers, then within each.
- **Measuring** runs on a worker (`job::Job`) over a clone of the document, one job at a time.
  `contrast_tick`, before the canvas, compares the document with a snapshot whenever
  `session.revision()` moves. A layer has changed when its stored node differs or its world box
  moved; a subject is queued, and drawn stale, when it changed or its box overlaps the old or new
  ink (`Resolved::ink_bounds`, a shadow included) of a changed or removed layer. A change to
  variables, styles or the page ground, or of the level or *Non-text*, queues everything. The
  queue goes once the document has been still for `SETTLE` (0.15 s), so a run of nudges measures
  once, `CHUNK` (6) layers a job in reading order, so badges arrive top down as they are measured.
  The first job at a revision prepares the page (`contrast::Prepared`: each mode's document,
  layout and decoded pictures) and hands it back; later jobs at that revision measure on it.
  A layer queued again while its job is in flight stays stale when that answer lands. A layer the
  answer leaves out (nothing of it is drawn) is marked so, with no badge, and waits for an edit
  that could change that rather than being queued on every edit.
- **Badges** (`contrast_badges`) keep one screen size, above the layer's top left; a badge that
  overlaps earlier ones moves down below the lowest it touches, and one pushed past the canvas's
  bottom is not drawn. They are painted in the canvas panel's own layer
  after `canvas_ui`, so every floating card covers them.

  | Mark | Badge |
  | --- | --- |
  | Fail | red ground, warning glyph, `2.3 : 1 · needs 4.5` (`Dark · ` before *needs* when the worst mode is named), and a red outline 3pt outside the layer: red never stands alone |
  | Pass | dark chip, tick, the bare score (`12.7`, `Lc 78`), ` · large` when only the text's size makes it pass; hidden with *Show passing* off |
  | Skipped | dashed chip, `Not measured · gradient fill` or `picture fill` |

  A stale badge dims to 45% with a spinning circle-notch in place of its glyph. During a drag (a
  gesture preview that is not the fix card's own) the badges of the selected layers and their
  descendants hide.
- **The bar** (`contrast_bar`), an `Area` at the canvas's top centre, 12pt below the top ruler (below the canvas's top edge with rulers hidden): circle-half glyph,
  the scope pill (*Contrast* on a page with no frame), the level head (`ui::menu_head`, as the top
  bar's dropdowns), then *Measuring…*, *All N pass* or *N failing* with `k / N` once stepped and
  prev/next arrows, then × to end the mode; groups part at hairlines. *Measuring…* pulses
  (`ui::pulse_alpha`) through a whole run — the first look or a level change — and the count
  shows only once it ends; an edit's remeasure keeps the count, dimmed. The step position's cell
  is as wide as `N / N`, so stepping never moves the arrows under the pointer. The arrows step
  through failures in reading order, selecting each and
  moving the camera only when the layer is not wholly in view: centred if it fits, else `zoom_to`.
- **The level menu**, from the head: AA (4.5 / 3), AAA (7 / 4.5), APCA (Lc), then *Show passing*
  and *Non-text (3 : 1)*, kept in `prefs.contrast` (`ContrastPrefs`; AA, passing shown, text only).
- **The fix card** (`contrast_fix_card`, a 270pt `Area` beside the badge) opens on a click on a
  failing badge, which also selects the layer; the badge is a widget registered after the canvas's,
  so it wins the pointer, and lights (a lighter ground and a ring) under it, since the cursor stays
  the arrow. Header (layer name; *body text*, *large text* or *non-text*; the level),
  the worst sample's ink and backdrop as a split swatch with both hexes and the score in red, a row
  per mode when there are several, then *Nearest* (`contrast::nearest`; *No colour at this hue
  passes*), then *Variables* (`contrast::passing_variables`, at most six; *No colour variable
  passes*). Each section notes what its edit takes besides the colour (`side_effects`):
  *Unbinds ‹var›* for a fill or stroke it rewrites that was bound, *Detaches ‹style›* for a
  styled slot it writes. Hovering a row previews it (`session.set_preview`); a click, or Enter
  (the hovered row, else the first) while neither the palette, a modal nor a text field has the
  keyboard, applies it as one undo step, *Fix contrast*. Escape, a click away, an undo or a redo
  closes it. A remeasure redoes the rows, and closes the card when the layer now passes or is
  gone. The card and the level menu own Escape through `a_popover_owns_escape` and
  `popover_heard`.
- **Every fix passes in every mode.** A raw colour is the same in all modes, so a fix for one mode
  that breaks another is no fix: *Nearest* must pass every mode's worst backdrop, and *Variables*
  lists only variables that pass in every mode, nearest the ink first. When no lightness at the
  hue passes everywhere the card offers only variables, the case modes exist for.
- **What a fix edits** (`ondin_core::contrast::fix_transaction`, the `fix_contrast` verb's too): the
  runs and paints whose colour fails the finding,
  never the backdrop. A bound fill or stroke is judged by its variable's value in each mode against
  that mode's sample, so a variable that adapts to every mode is left bound. A run's own colour is
  replaced in the spans; runs drawing in the fill change the top visible fill. A run's colour
  cannot be bound, so a variable goes through the fill: a failing run's own colour is cleared onto
  the fill and the fill bound, and passing runs that drew in the fill are given its old colour
  (black when there was no visible fill) so they keep their look. Text with no visible fill gets
  a new one on top, its hidden fills kept. On a shape only the failing visible solid fills and
  strokes the measured ink came from are replaced or bound: a white box with a failing border is
  fixed on its border.

## The vision simulation (`vision.rs`)

*Tools ▸ Simulate vision* (`Action::SimulateVision`, `menu::Item::SimulateVision`, `Group::View`,
no chord): the Tools menu's row after *Check contrast*, and a palette row reading
*Stop simulating vision* while on. Its search words keep the old name's (*colour blind*, *color
blind*, *colour blindness*) so muscle memory still finds it. One frame of the canvas is shown as
someone sees it with a condition of colour vision or of sight, at a severity
(`ondin_render::vision`, `render.md`). `OndinApp::vision` is view state, as the contrast check is:
off when a document opens (`reset_transient_state`), never saved or exported, never drawn in
present mode. It is not a switch inside the contrast check: WCAG contrast is luminance, which the
colour deficiencies barely move.

- **Scope: one frame** (`vision_target`): the top-level frame of the first selected layer, else
  `OndinApp::frame_in_view`, the frame-in-view rule `contrast_target` uses; a page with no frame
  is simulated whole. Fixed for the mode's life, like the check's: a click on the bar's scope chip
  re-targets, and switching pages, or the frame going away (deleted or undone), re-targets to the
  frame in view.
- **The bar**, in the contrast bar's style and place (`contrast::{bar_frame, separator,
  scope_chip, cell, BAR_H}`): eye glyph, the frame's scope chip (named through `private::layer`) or
  *Whole canvas*, a hairline, the condition head (`ui::menu_head`, hugging its label), the severity
  slider (`ui::slider`, `SLIDER_W` 88pt, *How strong the simulation is*), the percentage in a cell
  as wide as *100%* (`contrast::cell`), a hairline, then × (*Stop simulating vision*). The bar
  widens and narrows with the condition's name, so a short one leaves no gap; the cell keeps the ×
  still as the percentage changes.
- **The slider** writes the shown condition's severity live and saves it when the press ends. A
  right-click while it is held puts the severity back and the rest of the press does nothing
  (`InFlight::vision_scrub`, restored by `OndinApp::cancel_gesture`), as every drag in the app
  cancels (`the_slider_sets_the_shown_condition_s_severity`).
- **The condition menu**, from the head, 240pt (`MENU_W`): Protanopia, Deuteranopia, Tritanopia,
  Achromatopsia, a hairline, then Blurred vision, Contrast loss, Ghosting, Yellowing, each a
  ticked row *Label · gloss* (*Blurred vision · Out of focus*). A pick switches at once and closes
  the menu. Escape or a click away closes it, Escape before it reaches the canvas
  (`vision_owns_escape`).
- **Preferences** (`prefs.vision`, `VisionPrefs`): the condition last chosen (`Condition::key`;
  deuteranopia by default) and each condition's own severity, so the slider shows the picked
  condition's. A condition never set opens at `Condition::default_severity`: 100% for a
  deficiency (the dichromacy), 50% for a loss of sight. The slider writes the shown condition's
  severity live and saves when a drag ends or on a click or key, not on every drag frame. A file
  holding a lone deficiency key reads as that deficiency at 100% (`VisionStored`, untagged;
  `a_stored_deficiency_key_still_reads`).
- **Drawing** (`CanvasRenderer::seen_through`, given `vision::Seen`). While the mode is on,
  the pass writes the canvas texture (`target`) into a second texture, `seen`, through the
  condition's effect inside the frame's box (`vision_region`: its world bounds mapped to texels
  through the camera and size that drew `target`, rounded out and clamped to the texture) and
  copied outside it, and egui is handed `seen`. The scale is `camera.zoom`, device px to the
  design px, so a blur or ghost reads the same at any zoom; a blur or ghost reads neighbours only
  inside the region, so nothing outside bleeds in. Under pixel preview that is the 1x texture,
  simulated before it is enlarged. `target` is never written, so pans
  and patches keep working from true pixels and the eyedropper reads true colours. The edit
  chrome is painted after the canvas texture and is never simulated. The region is the frame's
  axis-aligned box, so a rounded or turned frame's corners take in a little of what lies outside
  it. The pass runs only when `target` was drawn into or the `Seen` changed since its last run,
  so a repaint that changed neither (a caret blink, a hover) shows `seen` as it stands: a blur is
  tens of times a tint's cost. With the mode off, `seen` and the blur's scratch texture are let
  go, and the scratch is also let go while a condition other than a blur is shown.

## The top bar's right cluster

Between undo/redo and the zoom readout: **View** (eye + caret), **Snap** (magnet + caret) and
**Tools** (toolbox + caret), so the bar reads View · Snap · Tools · zoom · Settings.

**The resource meter** (`meter.rs`), left of undo behind a rule, while Settings' *Show resource meter*
is on (off by default): CPU (`icon::CPU`) and memory (`MEMORY`), each
a 15pt glyph in `text::dim()` and an 11.5pt value in `text::muted()`, in a slot as wide as its longest
figure so the bar never shifts, 14pt between readouts; a tooltip gives the exact figure.

- CPU is the process's share of every core over the last second, from its CPU time
  (`GetProcessTimes`, `getrusage`); memory is its working set on Windows
  (`K32GetProcessMemoryInfo`) and its resident size elsewhere (`/proc/self/statm`, `proc_pidinfo`),
  in MB under a gigabyte and GB with one decimal from one. Both are read once a second while the meter
  shows, and that second is a `request_repaint_after`, the app being reactive. Switching the meter off
  forgets the readings, and a meter that missed an egui pass (the library, present mode) starts its
  CPU share afresh rather than averaging the time away. Told by the pass number, not the clock: a
  frame that takes seconds is drawn in the next pass, and its share is the one worth showing.
- CPU above 80% turns its readout `color::warn()`; memory has no threshold.
- There is no frame-rate readout: the meter is for the user, and a canvas frame count served only the
  performance work.

- Each head (zoom readout's too) is `ui::menu_head`: `hover()` ground on its 26pt rect, radius 5, held
  while its own menu is open.
- `OndinApp::view_snap_tools_menus` draws the three heads, each tooltipped with its name and
  disabled while comparing (*‹Name› — not while comparing*); comparing also closes whichever is open.
- View and Snap rows are `ui::menu_check`, a checkable `ui::menu_row` with no glyph, `ui::MENU_ROW_H`
  (24) tall, in `ui::menu_frame`. The accent tick sits in the glyph column and keeps it when off;
  "off" also dims the label. View and Snap are 160pt or their widest row; Tools is its widest row
  (`ui::menu_check_accel_width`: the label column `ui::MENU_LABEL_X`, or a bare row's 8pt inset
  for Tools, the label and any chord, then 8pt).
- **View** reads in three groups between separators, so each kind of row is found in one place:
  - what the canvas draws, `ViewSwitch::VIEW_CANVAS`: *Show rulers*, *Show guides*, *Lock guides*,
    *Show grid*, *Show layout grid*;
  - the modes, `ViewSwitch::VIEW_MODES`: *Outline mode*, *Pixel preview*, *Private mode*, *Focus mode*, then *Present
    mode* — a verb, not a switch: a check row that never ticks, which closes the menu and dispatches
    `Action::Present`;
  - *Panels ▸* (`PANELS_ROW`, a `ui::submenu_check_row`), whose flyout
    (`OndinApp::panels_flyout`, a `ui::flyout`) holds `ViewSwitch::VIEW_PANELS` labelled short: *Layers*, *History*,
    *Minimap*, *Toolbar*, *Messages*. *Minimap* follows *History* so the two about the layers pane
    stay together.

  Hovering or clicking *Panels* opens the flyout; hovering any other row closes it. A click in the
  flyout toggles its pane and leaves both open: `OndinApp::dropdown`'s click-outside close skips
  the area `OndinApp::flyout_id` names. The marquee's *Lasso select* is on no menu: `Q`, the palette,
  the cheatsheet and the rail's Select slot carry it (`app-selection.md`).
- **Snap**: *Snap to shapes*, *Snap to guides* (ruler guides, and layout grids while shown), *Snap to
  grid*, *Snap to baselines* (last: no accelerator, not Illustrator's).
- **Tools** (`OndinApp::tools_menu`, rows from `ToolsRow::ALL`) holds what works across the
  document. Verbs on the selection stay in the context menu, document-level rows in the Document
  menu, view modes in View, inserting in the insert picker.
  - the checks, which change nothing: *Check contrast* and *Simulate vision*, each toggling its
    mode (never remembered) and named *Stop checking contrast* / *Stop simulating vision* while on,
    the palette's and the bar ✕'s words (`contrast::STOP_CONTRAST`, `vision::STOP_VISION`), then
    *Colour review…*, opening its card;
  - a hairline, then the bulk edits, each opening its box: *Find and replace…* (`Ctrl+F`,
    `Action::FindText { replace: false }`), *Batch rename…* (`Ctrl+Shift+R`; dimmed with
    `private::DECLINE_RENAME` in private mode), *Clean up…* (no chord) and *Fill with content…* (no
    chord; never dimmed: with nothing to fill the box opens and says what to select).

  Every row is a bare `ui::menu_row` (an empty glyph, never checked), its label at the row's own
  inset: nothing in the menu ticks, and a check that is on says so in its own bar over the canvas.
  The bulk edits show their chords from the keymap (`Item::spec().accel`).
  *Find and replace…* is this menu's own label (`ToolsRow::label`): the palette names the box's two
  doors, *Find in text…* and *Replace text…*. A pick closes the menu, then acts. The palette keeps
  every row.
- **Zoom** (`zoom_control`): a text field on top, unfocused at open, holding the live zoom as its
  value; Enter applies `OndinApp::apply_typed_zoom` (`150`, `150%`, `37.5 %`; about the canvas
  centre, clamped like every zoom; anything else says *"Type a zoom percentage, like 150"*) and
  closes the menu — an untouched value applies nothing, since it reads the rounded zoom. Below it
  *Zoom in*, *Zoom out*, *Actual size*, *Zoom to fit*, *Fit selection*. `OndinApp::zoom_entry`
  holds the typed text while the field has focus, `None` otherwise.
- View and Snap render rows from `input::ViewSwitch`, the enum chords name, so label, order and state come from
  one place; the *Panels ▸* flyout alone drops the "Show" its labels need elsewhere. `input::BINDINGS` has no chord for *Snap to baselines*, *Show layout grid*, *Outline mode*,
  *Show history*, *Show messages*.
- **Outline mode** draws the artwork as one-device-pixel hairlines of every path the walk would fill
  or stroke, with type in the same ink, in near-black or near-white by the page ground's luminance.
  It pushes no layer, so nothing is clipped, masked, faded or given an effect: the view exists to
  show what the artwork covers. It is `scene::OutlinePainter` wrapping the backend's painter, so the
  walk is the same but for its cull: a clipping frame, a group and the page are not culled by
  their ink, which stops at a clipping frame's box, so a frame's overflow shows with the frame
  out of view. A hairline is stroked on the path mapped to world space, so it is one device pixel
  on every axis of a skewed or snapped layer. `RenderOverrides::set_outline` carries the ink, installed every frame
  by `draw_canvas` (present mode clears it). Hit testing and selection are unchanged. The eyedropper
  samples the canvas as drawn, so in outline view it picks up the ink.
- **Pixel preview** (`OndinApp::pixel_preview`, `Ctrl+Alt+Y`, glyph `PIXEL_PREVIEW`) shows the anti-aliasing a viewer
  at 1x gets. The pixel grid (`grid.rs`) shows where pixels fall over artwork drawn at device
  resolution, where a half-pixel edge looks sharp; the preview shows it blur. `canvas::pixel_preview`
  takes whole world units from the unit at the view's corner, one more than the view spans so the
  size holds whatever the corner, and returns a zoom-1 camera over exactly those units (one texel a
  unit), the texture size, and where that texture lies on the canvas in device pixels. `draw_canvas`
  renders through the same `CanvasRenderer::render` with that camera and size, so the drawn-key
  cache, pans and patches apply unchanged, then paints the texture enlarged over that rect, clipped
  to the canvas. The canvas texture
  is registered with nearest filtering (`Gpu::texture`), which gives the hard edges; there is no
  pass of its own. Overlays stay egui at screen resolution, and present mode draws as ever.
  - Off at 100% or below (`camera.zoom` is device pixels per unit): the canvas already draws a unit
    to a pixel or less, and the preview would only throw detail away.
  - It yields while a dropper is armed (`dropper_armed`), since the eyedropper reads the canvas
    texture at device pixels.
  - Per session, as outline mode is: a launch into blurred artwork would look like a broken
    renderer.
- The switches are **view state**, not document state. All but focus mode, outline view and pixel preview are kept in `prefs.view`
  (`prefs::ViewPrefs`), written once a frame when one has changed
  (`OndinApp::remember_view_switches`, which also catches side doors such as a guide pulled from a
  ruler turning guides on), and a launch opens on them. All default on except *Lock guides*,
  *Show history*, *Show minimap*, *Lasso select*, outline view, pixel preview and focus mode; a launch never opens in focus mode,
  outline view or pixel preview. `prefs.view` also keeps what the rail's Frame and Pen slots last showed
  (`rail_slice`, `rail_pencil`; `tools.md`, *The tool rail*). Private mode is a preference of its own (`Prefs::private`), below.

### Settings

- Rightmost: `icon::SLIDERS`, 28pt, ten points clear of the document's controls. The update chip
  sits between it and that gap. The window's caption buttons are in the tab strip above.
- Opens `settings.rs`'s modal: the theme and *Show resource meter* (under **Appearance**; the
  library's Settings shares the theme row only), nudge step, the **Canvas** rows (*Zoom with the
  scroll wheel* with its caption, `tools.md` *The wheel*; *Ruler guides colour* and *Ruler guides thickness*, *Adaptive pixel grid* with its caption, and *Pixel grid
  colour*, dimmed with *Turn off Adaptive pixel grid to pick a colour* while adaptive is on;
  `app-rulers-guides.md`, `prefs::CanvasPrefs`), *Show tree guides* and *Collapse groups on open* (under a
  **Layers** eyebrow), *Default font* (under **Text**: the typography panel's family dropdown,
  its glyph `warn()` with a tooltip when the family is `Missing`, §5.4a, and *Check spelling*), the web-font switch and
  cache row, then **Assets** (*Assets browser*, below). `Ctrl+,` opens it (and only opens:
  `input::resolve` is not called while a modal is up). Staged; writes on *Save changes*.
- The sections scroll between the title and the footer once the card would be taller than the
  window, with no bar. The scroll area takes the sections' height as last measured, capped by the
  window, as `min_scrolled_height`: an `Area` hands a scroll area last frame's size, so a
  `max_height` alone shrinks and never grows back
  (`settings::the_sections_scroll_and_the_footer_stays_on_screen`).
- Not a `TopMenu` value. Panel-owned preferences stay on their panel (export habits in the export
  popover, written on the spot).
- There is no *Share* button (local file editor; nothing to share).

### One open menu

`app::TopMenu` holds one value — `None | View | Snap | Tools | Zoom | Document | KitUpdates` — so menus are
mutually exclusive by construction. `OndinApp::dropdown` spells the closing half once (click outside,
Escape). `OndinApp::context_menu` is the same one-slot shape; opening a context menu clears this field
and the floating popovers.

## The command palette (`palette.rs`)

`Ctrl+K`, alias `Ctrl+/` (`shortcuts.md` §8), in the editor only: in the library `Ctrl+K` stays the
file search, and the library screen clears an open palette. Both chords are `input::BINDINGS` rows
read in both modes. With a modal up the chord does nothing, and a
modal that opens closes the palette. App actions only; typed syntax belongs to Command Mode
(`vm.md`).

- **One registry.** A row is a `menu::Item`, with the context menu's label, glyph, chord, tick and
  dim reason. The app verbs no context menu shows are `Item`s too (undo and redo, save, new file,
  close tab, next and previous tab, reopen closed tab (`Action::Tab`, which the keymap never
  yields), the library, settings, *Export as …*, *Present*, *Check contrast*, *Simulate vision*, *Colour review…*, *Clean up…*, tools, place image, zoom, align,
  distribute, insert, the assets browser, styles and variables, publish kit, the text chords, *Show ‹card›*, the palette
  and the cheatsheet), each in a `Group` whose `Group::label` is the row's side note.
  `Item::action` maps each to its chord's `Action`, which `perform_menu_item` dispatches, so row and
  chord are one verb; `Item::aliases` holds the extra words. `palette_commands` chooses which verbs
  are offered; their words live in the registry. The Document menu's rows (*Credits…*, *Make this a kit*,
  *Kits used…*, *Stop sharing as kit…*, *Show in library*) are registry items the menu draws its
  labels from, offered always and dimmed with a reason where the menu would leave them out (their
  `Action`s re-ask the menu's guards); the Transform card's quarter turns are
  `Item::RotateQuarter`, dimmed without a selection. *Transform again* and *Repeat around origin…*
  (`Item::TransformAgain`, `Item::RepeatAround`) are rows of their own in the Transform group
  (`tools.md`). *Select same …* (`Item::SelectSame`) dims with
  `select_same_source`'s reason, *Select matching layers* with `select_matching_sources`'s, and
  *Select inverse* without a selection (`app-selection.md`).
- **The target** (`palette_target`, re-read every frame): a text session → `Target::TextSession`; a
  point or segment selection on the edited path → `Target::Points`; selected layers →
  `Target::Layer { id, door: Panel }`, `id` the key layer, else the topmost selected in
  layers-panel order; only a guide selected → `Target::Guide`; else `Target::Canvas`. The rows are
  that target's menu built with no pointer (`menu_snapshot`), then in a text session the text
  chords, then always the canvas menu's rows. *Add point here* and *Make this the key
  layer* are never listed: the keyboard supplies no point and no pointer names "this". The footer
  names the target (*Card and 2 more*, *Editing text*, *Nothing selected*).
- **Paste.** `PasteHere` is labelled *Paste* and runs `Action::Paste` outside a text session
  (`run_palette_item`).
- **Present, dim, absent.** A row the menu would dim is dim with the same reason and keeps its rank,
  so the list does not reshuffle when the selection changes. The verbs a target's menu omits come
  from `menu::KIND_VERBS`, a hand list giving each its *Needs …* sentence (*Needs text on a path*);
  with nothing selected every one reads `menu::NEEDS_SELECTION`, *Select a layer first*. They show
  only when the query matches, last, under *Not available here*. App verbs dim on their own
  predicate (*Nothing to undo*, *Needs three or more layers*); *Publish kit…* takes the document
  menu's, read at open (*Make this document a kit first*, *No changes*). Enter on a dim row runs
  nothing; the footer shows the reason.
- **Matching** (`rank`). Every query word matches the start of a label word, in any order,
  case-folded, each taking a distinct word (*al c* → *Align horizontal centres*; *g g* does not
  find *Group selection*). Matched letters take the accent. An alias counts only when the label
  alone cannot answer some query word, and lights nothing (*svg* → *Copy as SVG*). Rank 0 when the
  query is a prefix of the label's words, 1 for word starts, 2 through an alias; ties go by recency,
  then registry order. *Not available here* sorts by rank, then registry order. A query with no
  words (`🔥`, `#`) matches no command, so only the layers section answers it and Enter never runs
  an arbitrary verb.
- **No syntax.** No `>` or `@` prefixes and no typed arguments: a verb with an argument is one row
  per value (*Align left*, *Export as React…*).
- **Running.** Enter or a click closes the palette first, then `run_palette_item` records the row
  in *Recent* and dispatches it as its menu row would, so a dialog never opens under it.
- **Empty query.** *Recent* (the last five rows run, keyed by `verb_key`, the `Item`'s `Debug`
  form, so *Hide* and *Show* are one; `prefs.palette_recent`), then the target's own menu rows
  (*For the selection*, *For the page*, *For the text*, …). The first enabled row is highlighted,
  and again whenever the query or scope changes.
- **Layers.** Matching layers follow the commands: `layers::name_hits`, the layers panel's own
  filter rule (`matching_rows` keeps these and their ancestors), a case-folded substring in panel
  order, the root dropped. The first five show, then *N more layers*, which Tab or Enter opens. Each
  row has its kind glyph, ancestor path (outermost first, cut from the left a parent at a time) and
  hidden or locked mark. Tab toggles a layers-only scope (a chip in the field, the query kept,
  Backspace on an empty query removes the chip), capped at 200 rows. Enter selects the layer as
  clicking its panel row does; the panel opens its parents and `reveal_selection` brings it into
  view without changing zoom. Shift+Enter or Shift-click adds it to the selection.
- **Verbs with no other door.** *Export as HTML… / React… / Vue…*, one row per `code::Target`, dim
  with nothing selected, aliases *code*, *html*, *react*, *vue*, *zip*. *Export as Penpot file…*
  and *Export as Sketch file…*, one row per `EditableFormat`, never dim since each writes the
  whole document, aliases *penpot* or *sketch* and *lunacy*, and *editable*, *save as*. *Show ‹card›*: the cards the
  inspector drew last frame (`cards_shown`) are live; cards drawn earlier this session but not now
  (`cards_seen`) are dim, *Not shown for this selection*; in focus mode all are dim. Running one
  sets `reveal_card`, and `panel_full` opens that card, scrolls it to the top and focuses its first
  field: the first widget drawn in it that senses click, drag and focus (`first_field`), which a
  value or text field does and a button or a reorder grip does not. Tool rows (*Tool:
  Pen*, `P`). The pages, the one way to another page with the layers pane hidden: *New page*,
  *Next page* and *Previous page* (dim at either end), and per page *Go to page: ‹name›* (dim on the
  current one) and, for every other page, *Move to page: ‹name›* (dim with nothing selected). The View and Snap switches show their tick, which gives the unbound ones (*Snap to
  baselines*, *Show messages*, *Show layout grid*, *Outline mode*) a keyboard route.
- **In a text session** the chord opens the palette too, with the text session's menu rows and
  styling chords first. The session stays open under it; the palette's field holds focus, so the
  canvas reads no keys, and Escape closes only the palette.
- **Placement and keys.** An `egui::Modal` on the settings card's ground (`settings::card_frame`)
  with a transparent backdrop, 560 wide, at
  `ui::canvas_popup_area`: centre-top over the canvas column, its top `ui::CANVAS_POPUP_TOP` (56pt)
  below the canvas's top, its list scrolling past `ui::CANVAS_POPUP_LIST_H`. Nothing dims, so selection
  outlines stay visible; the panels are unchanged. A backdrop click closes it and is swallowed. While
  open it owns the keyboard: `palette::read_keys` takes its keys at the top of the frame, before any
  panel draws, and `input::resolve` does not run. ↑↓ wrap, Page Up/Down move eight rows, Escape
  closes whatever is typed, the chord again closes, and the cheatsheet's chord (read first, since
  the palette's `/` would take it too) swaps the palette for the sheet. It works in focus mode.
- **Not listed:** nudge, size step, point stepping, opacity digits, Escape and Enter, text chords
  outside a text session, `PasteRelease`.

## Batch rename (`batch_rename.rs`)

A box that renames many layers at once: the selection, the page or every page. The text rule is
`ondin_core::rename`'s; `batch_rename::plan_into` picks, orders and numbers the layers.

What the plan reads of the document — the layers of every
page it can reach, in tree order, with name, bounds, glyph, a text layer's content (its words
joined by single spaces, so line breaks and whitespace runs become one space), and whether each
has a default name, is a variant, inside an instance, selected or under the selection — is a copy
(`Snapshot`), taken at open and again whenever the document's revision, the page or the selection
moves (`refresh_batch_rename`), as Find searches again: the canvas stays live under the box. A
worker thread plans from the copy each request carries, whenever a setting or the copy changes, taking only the newest request and abandoning a plan once a newer one is
asked for, and sends the rows back in parts of 500 layers, so typing never waits on a large
document. The preview keeps the previous plan until the new one's first part lands, its count
worded for the scope that plan was made for (`Plan::scope`).

- **Two verbs.** *Rename* (`Ctrl+R`, `F2`) is the layers tree's one-layer field (`app-selection.md`).
  *Batch rename…* (`Ctrl+Shift+R`, `Action::BatchRename`, `Item::BatchRename`, State group,
  `CURSOR_TEXT`) opens the box and is always live. With two or more layers selected, `Ctrl+R` and `F2`
  open the box rather than refusing.
- **Doors.** The chord; *Tools ▸ Batch rename…*; the palette, offered with any target (aliases *find*, *replace*, *number*,
  *bulk*); the layer menu's one rename row, which reads *Batch rename…* with several layers selected
  and *Rename* with one, as `Ctrl+R` does; a `CURSOR_TEXT` button in the
  Layers header between collapse-all and search (*Batch rename (Ctrl+Shift+R)*); the cheatsheet's
  Layer section.
- **Placement and keys.** The palette's place, 520 wide, centred over the canvas column, its top
  56pt below the canvas's top: an open card (`CardHold::Open`), not a modal, so the canvas keeps
  its input and the layers can be selected, edited and panned under it, as with Find and Colour
  review. Opening it closes the palette, the context menu, the top-bar menu, the colour picker and
  the other cards over the canvas's top (Find, Colour review, Clean up, Fill with content); opening any of those
  closes it. Choosing it again while it is up keeps what was typed and takes only a search the
  Layers header hands it. Enter renames from Find or *Rename to* (with the order list open it does nothing);
  anywhere else it is the canvas's. The *Rename* button renames; *Cancel* or ✕ closes it, and
  Escape closes it through `OndinApp::escape`'s rung below Find's (the order list, open, takes the
  press first). It is drawn outside the `!focus` guard, so it works in focus mode.
- **Scope**, the *In* row: *Selection · This page · All pages*, a `ui::segmented` of equal cells
  (`scope_segments`, which Find shares) with the selection's count after *Selection*. The box's
  fields are `ui::CONTROL_H` tall, as every field is. It opens on *Selection* when
  something is selected, else on *This page*. *Selection* is the live selection, and is disabled
  with nothing selected; a selection emptied under the box moves it to *This page*. One selected layer with children opens with *Include layers inside*
  ticked. The Layers header button, with text in the tree's search, opens on *This page* with Find
  holding that text. *All pages* leaves draft pages out, as everything gathered from every page
  does (`core-model.md` §5.5), but keeps the current page when it is a draft, since the box was
  opened from it. *Keep case* is on at page and document scope and off at *Selection*; switching
  scope resets it.
- **Which layers.** *Selection*: the selected layers, and every layer under them with *Include
  layers inside*. *This page* and *All pages*: every layer except those inside an instance (an
  instance root is included); *Include layers inside instances* takes them in, and while it is off
  *· N skipped* counts the matches it leaves out. Locked and hidden layers are renamed. A variant in
  a set is never renamed, since its name comes from its property values; it is listed with *Named by
  its properties* in amber. *Only default names* (`Settings::default_names`), a tick box above the
  scope's own, leaves out every layer whose name the app did not write (a default name is one
  `naming::generated_stem` reads, as *Rectangle 3*, *Group*, *Text 2*, or one the app writes outside
  that table, *Image 2*, *Blend*, *Trace*, *Component*, *Mask*, *Clip*: `batch_rename::app_named`):
  such a layer is not listed, counted or numbered. It is off at open and not remembered.
- **The rule** (`rename::Find`, `Template`, `Numbering`). An empty Find replaces the whole name and
  `[Name]` is that name; a filled one replaces every match and `[Name]` is the matched text. Matching
  ignores case unless *Aa*; *.\** reads Find as a regex (a literal Find is an escaped one). A pattern
  that does not parse shows its one-line reason under Find (*Not a pattern: …*) and renames nothing.
  The template's tokens are `[Name]`, `[Number]`, `[Text]` and `[Group k]`; `[n]` and `[k]` are
  short forms (`[Text]` has none), and every token reads in any case. `[Text]` (`Piece::Content`)
  is a text layer's content and nothing for any other layer; `Rule::apply(name, number, content)`
  fills it. *Keep case* (*Ab*, with Find filled) writes a replacement's new
  text in its match's case — `BUTTON` upper, `button` lower, `Button` a capital then lower; a mixed
  case leaves it as typed.
- **Rename to** draws each token as a pill: the canonical `[Word]` text with its brackets hidden, so
  the caret walks real characters, at the text's own size so pill and words share a line. The
  edit is `put` into the field's whole height, as Find is, which centres its text. Two pills side by
  side keep 2pt of air painted between them; egui joins neighbouring glyphs of one background into
  one rectangle, so every other pill's ground is a unit of alpha lighter. A caret the arrow keys move into a pill leaves it on the side it was heading
  for, so Left and Right step pill by pill; one a click puts inside goes after it. A typed short form is rewritten to the canonical token;
  Backspace and Delete take a pill out whole; *+ Name*, *+ Number*, *+ Text* and one *+ Group k* per
  pattern group insert at the caret. The field opens holding the last template (one *Name* pill the first
  time), all selected, so typing replaces it. An empty template with Find filled shows *Empty:
  removes the match*.
- **Numbering.** The Number and Order rows show once the template has a Number token. Number holds
  *From*, *Step* and *Pad* as the inspector's scrubbable value fields (`ui::value_field`, the word
  inside the field), sharing the row, and scrubbed slowly for control: 20pt of travel a unit for
  From and Step, 25pt for Pad (the usual is 4). A negative step counts down; Pad, 0 to 5 and 0 by default,
  zero-pads to at least that many digits (`Numbering::pad`; 3 writes `001`). Order, under
  it, is a dropdown as the Settings modal's are. All are 28 tall, the Settings modal's field height.
  A right-click mid-scrub puts the numbering back as the scrub found it (`InFlight::rename_scrub`,
  wound back in `cancel_gesture`).
  The orders: *Layers order*
  (tree top to bottom, the default), *Layers order, reversed*, *Left to right* (rows: a layer joins
  the first band its centre falls in, bands taken top to bottom, each read left to right by left
  edge) and *Top to bottom* (columns, the same way). A layer whose box holds another run layer's —
  a frame round its cards, a background under a grid — forms no band with them, since its span
  would swallow every row it covers: such layers come first, read among themselves the same way,
  and the rest are banded without them. So, next, does a layer that holds none but whose span holds
  the centres of two layers that do not overlap — a sidebar beside the rows (`batch_rename::spanning`).
  Each page's run is ordered and the pages follow
  the Pages list; the number runs on across pages. Only a layer Find matches takes a number, so a
  skipped one leaves no gap; a matched layer whose name already is its result keeps that number, so
  no two layers are given the same one.
- **Preview**, in run order: glyph, old name with its matches underlined, arrow, new name with its
  new runs in `accent_300()`; it scrolls past eight rows with no bar shown, and only the rows in view are laid out
  (`ScrollArea::show_rows`), page headings at a row's height. At *Selection* every layer is listed and the
  unchanged ones dim; elsewhere only matches, and *All pages* heads each page's rows with its name
  and count. The count reads *N of M change* at *Selection*, *N of M match* on a page, plus *· K
  pages* for all pages. A name that would come out empty keeps its own (*Would be empty · kept*,
  amber) and takes no number. An empty preview reads *Matching…* only until the first plan lands,
  and *No layers match* after it, also while a later keystroke's plan is in flight. Nothing on the
  canvas changes until Rename.
- **Renaming.** One transaction of `SetName`, one undo step named *Rename* with `CURSOR_TEXT`; the
  status line says *Renamed N layers · Ctrl+Z to undo*. New names are cleaned as any typed name
  (`naming::usable`). Renaming a main reaches its copies by propagation; renaming inside an instance
  is an override, as in the tree. It does not switch page. *Cancel* and *Rename N layers* are the
  Settings modal's footer buttons (`settings::text_button`). Rename dims, its tooltip saying why,
  while the plan is still matching, when the pattern does not parse (its reason), when nothing
  matches Find, when every match would come out empty, when every match is a variant, or when
  nothing would change. Find with its *Aa* and *.\** (a pattern means nothing read as literal
  text), template, from, step, pad and order are kept in `prefs.batch_rename` when a rename runs;
  the Layers header's search opens Find as plain text with both switches off.

## Clean up (`clean_up_box.rs`, `ondin_core::clean_up`)

A box that takes away what a design no longer shows, in the selection, the page or every page, as
one undo step. *Clean up* is also the path bar's card (`cleanup.rs`, `app-pen-and-nodes.md`): the
same act at the scale of one path's points, so the two share the name and `MAGIC_WAND`. Opening
either closes the other, so the two never show together.

### The rule (`clean_up::plan`)

`plan(doc, res, scope, rules) -> Plan` is read-only. The rules run in `Rule::ALL`'s order on a clone
of the document, each rule's transaction applied to the clone before the next looks, and their
operations are concatenated into `Plan::tx`, valid against the original as one transaction (the
simulate-don't-concatenate rule, `core-ops.md` §5.7a). So a shape that loses its last hidden fill
is then empty, and a group that loses an empty sibling then holds one layer. A transaction the
clone refuses is dropped with its rows, so the plan never carries one the document would refuse
(a rule's whole transaction; one pass of empty layers; one group's ungroup). Every rule takes only what draws
nothing or what draws exactly what replaces it; what it finds but would change a pixel by taking is
a `Finding` with `kept: Some(Kept)` and its reason.

| Rule | Takes | Lists as kept |
| --- | --- | --- |
| Hidden paint | hidden fills, strokes, effects and layout grids | a list a style supplies (*From a style*) |
| Covered fills | fills under the topmost visible literal solid with alpha 1; a bound brush never covers | a style's list; a layer of a main whose copy in an instance draws another list (*An instance shows it*) |
| Hidden layers | a literal eye off with no visibility variable, outermost only, with what they hold | a subtree holding a main (*Main · N instances would detach*), an export *Export all* writes, or (with *Skip locked*) a locked layer; state-paired (*Paired with a state*) |
| Empty layers | a group or boolean with no children; a frame with no children and no visible fill, stroke or effect; a text with empty content and no content binding (a text with content draws, in black when unpainted); any other shape with no visible fill, stroke or effect, except a boolean's operand, whose outline is the boolean's drawing; never a slice | a mask, an export, a main, a spacer (its parent lays out, or groups between it and such a parent take their box from it), state-paired |
| One-layer groups | a `Group` with one child, literally visible, not a main or variant, ungrouped through `build::ungroup` | opacity ≠ 1 or bound, a blend other than the kind's default, any effects, a mask or a lone child that is one (ungrouped, it would clip the layers above), an export, its own layout, a spacer (its own parent lays out; under a plain group it goes, since its box is its layer's and that group's box stays), state-paired |
| Guides | *Selection*: guides owned by layers in scope; a page: guides owned by the page or a frame on it | all, as *Locked*, while the view's guides are locked and *Skip locked* is on |

- **Skip locked** lists an effectively locked layer under whichever rule finds it, as *Locked ·
  skipped*.
- **Empty layers repeat** until nothing new empties; a layer inside a deleted empty one goes with it
  and has no row of its own.
- **One-layer groups rebuild `Resolved` from the clone** once an earlier rule took anything:
  `build::ungroup` places through `Resolved`, and the caller's describes the original.
- **State-paired** means in a variant of a set with interaction states, where a state may show,
  hide or restructure the layer.
- **Instances.** Nothing inside an instance is touched. An instance root's own paint is not edited,
  and it is neither ungrouped nor deleted as empty; a hidden one goes whole.
- **Defaults** (`Rules::default`): *Hidden layers* and *Guides* off, since people hide a layer or
  place a guide on purpose; the rest on; *Skip locked* on.
- **Items**: one per fill, stroke, effect or grid, one per layer, group or guide; a kept row counts
  zero. `Plan::layers` counts the distinct layers that lose something; guides are not layers.
- **The verb** `clean_up(nodes?, hidden_paint?, covered_fills?, hidden_layers?, empty_layers?,
  one_layer_groups?, guides?, skip_locked?)` runs the same plan, over every non-draft page when no
  nodes are given, each switch at its default unless given. It has no view, so it never keeps a
  guide as locked. It is an MCP Phase 2 tool (`mcp.md` §8.4).

### The box

- **Built as batch rename's**: `settings::canvas_card`, 520 wide, `CardHold::Open`, so the canvas
  stays live under it as under Find. Opening it closes the palette, the context menu, the top-bar
  menu, the colour picker and the other cards over the canvas's top; choosing it again while it is
  up leaves its scope and switches as set. The commit button cleans up;
  Enter stays the canvas's (the box has no field to type in). *Cancel* or ✕ closes it, and Escape
  through `OndinApp::escape`'s rung below batch rename's. The foot's note reads *Esc closes*. It is
  drawn outside the `!focus` guard.
- **Head and scope.** `card_head` *Clean up*. The *In* row is batch rename's `scope_segments`,
  opening on *Selection* when something is selected, else on *This page*. *Selection* is the live
  selection (`OndinApp::selected_layers`), except that at *Selection* a row's click, which selects
  that row's layer, pins the roots it looked under until the selection changes again, as Find's
  double-click does; a selection emptied under the box moves it to *This page*. *All pages* leaves drafts out
  but keeps the current page.
- **Remove**: the six rules as `ui::switch_row_noted`, two to a line (left *Hidden paint*, *Covered
  fills*, *One-layer groups*; right *Hidden layers*, *Empty layers*, *Guides*), each noted with its
  count from a second plan with every switch on, so an off switch still says what it would take;
  `Rule::hint` is its tooltip. Then *Skip locked layers* on a fourth line at the same pitch. The
  *Remove* label sits level with the first line (`batch_rename::row` puts its label on a tall row's
  first control-height line).
- **Planning** is synchronous, and only when the scope, a switch, the page, the document's revision
  or the selection *Selection* reads changes (`CleanUpBox::replan`), as Find searches again; the
  view's guide lock is read into the switches every frame, so locking guides replans too. A drag
  previews without committing, so a gesture replans once, at its end.
- **Preview**: the eyebrow *Preview* and *N items on M layers*, with *· K kept* when amber rows
  show. Rows are grouped by rule in `Rule::ALL` order, each under a heading row with the rule's
  item count, every row listed: the list scrolls, so nothing folds. Rows are `ui::MENU_ROW_H` and
  the list scrolls past twelve with no bar shown, as batch rename's preview does. A row shows the layer's glyph, *Parent / Name*
  (masked in private mode) and what it loses, or the kept reason in amber; a guide row reads
  *Horizontal guide · 120*. Hovering a row rings its layer on the canvas, a heading its group's at
  a hairline (`canvas::draw_panel_hover`); a row on another page rings nothing and its tooltip
  names the page (*On ‹page›*). **A click shows the row's subject** (`show_finding`), as Find
  shows a match: its page made current, the layer selected, and on the next frame, once that page
  is drawn, brought into view by the least pan with the zoom kept. A guide's row shows the frame
  that owns it and selects nothing. The box stays open.
- **Empty**: *Nothing to clean up in the selection / on this page / on any page*, then, when a
  switched-off rule would find something, *Turned off: Hidden layers 3 · Guides 2*.
- **Foot**: `card_foot` with *Esc closes*, *Cancel* and *Clean up N items*; with
  nothing to take the button reads *Clean up*, dimmed, its tooltip *Nothing to clean up*.
- **Commit**: one undo step *Clean up* with `MAGIC_WAND`; the status line says *Cleaned up N items ·
  Ctrl+Z to undo*; the selection keeps only what still exists. The switches are saved to
  `prefs.clean_up` (`CleanUpPrefs { on: rule keys, skip_locked }`) when it runs.
- **Doors.** *Tools ▸ Clean up…* (after *Batch rename…*) and the palette (`Item::CleanUp`, aliases
  *tidy*, *unused*, *hidden*, *empty*); `Action::CleanUp` has no chord.

## Fill with content (`fill_box.rs`, `ondin_core::content_fill`)

A box that fills the selection's text layers with realistic names, addresses, companies, text,
numbers and dates, and its avatar shapes with a drawn picture, a different value per layer, as one
undo step. The tables ship inside core, so it works offline.

### The rule (`content_fill::plan`)

`plan(doc, roots, &Options) -> Plan` is read-only; `Plan::transaction(doc, Option<&mut dyn
ImageSink>)` is the edit.

- **Targets** (`gather`), in pre-order under the roots, children first to last: every text layer
  whose content is not bound to a variable, and every rect, ellipse, polygon, star or childless
  frame named as an avatar or showing an image fill. A name makes an avatar (`is_avatar_name`, whole
  words) with *avatar*, *headshot*, *pfp* or *portrait* anywhere in it, or with *photo*, *picture*,
  *pic*, *profile*, *foto* or *bild* when every word is one of those or a filler (*user*, *author*,
  *member*, *person*, *account*, *image*, *img*): *Profile picture* is an avatar, *Profile header bg*
  and *Product photo* are not. A hidden layer
  is passed over with all it holds; an effectively locked target is counted in `Plan::locked` and
  skipped. With avatars off no shape is listed.
- **Kinds**, in the groups *Person*, *Place*, *Company*, *Text*, *Numbers and dates* and *Image*
  (`Kind::ALL`, the lists' order; `KIND_KEYS`, the verb's and prefs' spelling). *Text* holds
  *Bio*, a third-person bio using the person's first name, and *Quote*, a first-person
  testimonial. A row's kind is the one chosen for it (`Options::kinds`, `None` meaning *Leave as
  is*); else, for text, the box's *Content* (`Options::content`); else what it reads as. A name is
  read first (`kind_from_name`: `NAME_RULES`, phrases in every shipped language, most specific
  first: *Job title* is a job before *title* a headline, *Street address* a street before
  *address* a full one, *Product price* a price before *product*, *Team member* a full name before
  *team* a department, a time before a date, the generic *name* last; *cell* alone is not a phone),
  then the text (`kind_from_text`: an email, price, percent, time, date, a phone number of seven
  digits or more, a number; text opening *Lorem ipsum* is a paragraph). A time has hours and
  minutes a clock shows and `.` between them only before an am/pm, so `12.50` is a number; a count
  grouped in thousands with a one- or two-digit lead (`1.250.000`) is a number, not a phone. *Heading* reads as
  nothing, so a list's heading is left. A shape is *Avatar* only by its name. What reads as
  nothing is left as is.
- **Labels**: a text whose name only repeats its text, as importers name them (`named_like_text`),
  may be a label. Of three words or fewer it is never read as a paragraph, sentence, bio or quote
  (`kind_of_text_layer`: a nav's *About*, a button's *Copy link*). When its name gives a kind its
  text does not, it reads as nothing where another layer of that kind sits beside it (same parent:
  a field's label beside its value) or in a record nested inside its own (a table's header over
  its rows). Otherwise it fills: *Name* in a card is a placeholder.
- **Records** (`Options::records`): each selected frame or group is a record, and so is each child
  of a container under the selection whose container children repeat: two or more sharing a main,
  or a name stem (*Card 1*, *Card 2*) and kind (`record_roots`). A layer's record is its nearest
  record ancestor. Loose layers under one parent are one record unless two of them want the same
  kind, when each is its own. Off, every layer is its own. `Plan::filled_records` is the record
  frames with a changed row; `Plan::record_noun` their shared name stem (*row*, *card*), else
  *record*. The box says *record* while private mode masks names, since the stem is one.
- **Agreement**: a record draws one person (the names, email, username, avatar), one place
  (street, city, postcode, full address, a phone number's area code) and one company. A job, bio or
  quote takes the person's gendered form. An email is at `example.com` while
  `Options::example_emails` is on, else at the record's company's domain when the record has a
  *Company* row, else at one domain the whole fill shares. A redraw (`Options::redraws`) of a row
  drawn from one of the three redraws all of it in that record, so the email follows the name; any
  other kind redraws only its row.
- **Determinism**: every value comes from `Options::seed` through splitmix64. A record's person,
  place and company are keyed by the record's own seed, a hash of what it is (its frame, its loose
  layers' parent, or its lone layer) rather than its place in the fill; a row's other draws by its
  layer; both by redraw count. So the same seed and options fill the same values, two cards filled
  in separate calls with one seed are two people, and changing one row's kind or the avatars switch
  reshuffles no other record.
- **No repeats**: records draw in order, each passing over the given and family names, cities,
  jobs, headlines, sentences, bios and quotes earlier records took while the list has others
  (`Rng::pick_fresh`: one draw, then the next free entry, so the stream stays in step); a company
  or product name, a combination of two lists, is drawn again up to eight times. A city or company
  counts as taken only where a row shows it. Departments repeat, as a real list's do. `plan` draws
  in two passes: every record as if nothing were redrawn, then each record with a redrawn row
  again, passing over every other record's values, so a redraw changes only its own record.
- **Variety**: a username takes one of five forms (initial and surname, given name and surname,
  *given.surname*, *given_initial*, given name and two digits); a number has 1 to 4 digits, each
  length as likely; a date is in 1965–2004 when its layer's name says birthday, born or dob (or
  another language's word), in 2022–2025 when it says created, updated, joined, since, published,
  posted or modified, else in 2025–2027 (`date_years`).
- **Fitting** (`room`), measured on the kind the text is drawn at (`Resolved::used_kind`: grown or
  stretched by a layout, its size bound to a variable), which `plan` and `Plan::transaction` take
  a `Resolved` for; an avatar is drawn at its drawn box too. Auto width takes the value's natural
  length. A one-line box (fixed under
  about 1.8 lines, or auto-height not yet wrapping) takes a value whose max-content width fits:
  up to `ATTEMPTS` draws (an entity's redrawn whole for its rows), else the first draw kept with
  `Row::fits` false; a full address there has its lines joined with *, * (`for_room`). A fixed box at least about 1.8 lines tall is an area, where a paragraph adds
  sentences while they fit; an auto-height box that wraps takes a paragraph grown to about its
  present length.
- **Text keeps its style**: one `SetText` per layer, its content replaced whole through
  `Spans::replaced` for both the character and the paragraph spans, so the new text takes the
  first character's attributes and the first paragraph's: a heading over a bulleted line stays a
  heading. Inside an instance it is an override like any edit.
- **Avatars** (`Avatar { name, initials, style }`): *Initials*, white Inter 600 at 0.4 of the
  shorter side, or *Pattern*, three seeded shapes, on one of the eight `GROUNDS` (each at least
  4.5:1 against white), picked by a hash of the name so the same person gets the same avatar. A
  shape's style is the one picked for it (`Options::styles`), else `Options::avatars`'.
  `Avatar::art(w, h)` answers filled paths, ground first. The transaction draws it at twice the
  shape's box (clamped to 8–2048 px) through `ImageSink::draw`, adds each image once and replaces the
  shape's fills with one image fill, dropping a photo's credit as a replace does
  (`credit::Replaced::Fills`). `draw` is a default method answering `None`; the app's
  `canvas::Decoded` and `import_files::Pictures` draw with `ondin_render::rasterize_paths` and a PNG
  encode (`canvas::art_png`). Without a sink that draws, avatars are left out.
- **Locales**, one file each in `ondin-core/assets/content/` (en-US, en-GB, de, fr, es, it, pt-BR,
  nl), `include_str!`'d uncompressed into `SOURCES`, whose order the locale list shows; each is
  named in English (*German*, *English (UK)*). A file holds `#` comments, `@key value` formats and
  `[section]` lists, with `\uXXXX` escapes; in a pattern `#` is a digit, `&` a digit 1–9 (a UK
  postcode district never reads 0), `@` a capital and a whole `a..b` a number in that range, and a
  phone format may list alternatives split by `|`. Given names are `[given-f]` and `[given-m]`
  (`[given]` still reads, ungendered); a `[jobs]`, `[bios]` or `[quotes]` entry may be
  `masculine|feminine`. Every shipped locale is gendered. `@inherit` takes what a locale leaves out
  from another, one level (en-GB from en-US). A locale is added as a file and its line in
  `SOURCES`. Phone numbers come from blocks reserved for fiction where the country has them:

  | Locale | Phone numbers |
  | --- | --- |
  | en-US | 555-0100 to 555-0199 |
  | en-GB | Ofcom's drama ranges per city, else 01632 960xxx; mobile 07700 900xxx |
  | fr | ARCEP's 01 99 00, 02 61 91, 03 53 01, 04 65 71, 05 36 49; mobile 06 39 98 |
  | de | the Bundesnetzagentur's drama numbers for Berlin, Frankfurt, Hamburg, Köln and München, else a subscriber number starting with 0, never assigned; mobile 0171 39200xx or 0176 040690xx |
  | nl, pt-BR | a landline's subscriber number starts with 0, never assigned; mobiles are only well-formed |
  | es, it | only well-formed: no regulator reserves numbers |

  `locale_for(tag)` is the exact tag, else the first of its language (*en* → en-US, *pt* → pt-BR).
  `agreed_locale` is the locale the target texts that set a Language (`TextStyle::locale`) agree
  on; a text without one says nothing, and `None` when they differ, one names no shipped locale, or
  none sets one.
- **The verb** `fill_content(nodes, locale?, kind?, records?, avatars?, seed?, example_emails?)`
  runs the same plan: `kind` a `KIND_KEYS` word, `avatars` *initials* (the default), *pattern* or
  *none*, records on, seed 0 and `example_emails` off unless given; the locale is the agreed one, else en-US, and a tag not shipped is refused.
  It draws avatars only when the caller lends a sink that draws, and fills the texts either way. It
  is an MCP Phase 2 tool (`mcp.md` §8.4).

### The box

- **Built as Clean up's**: `settings::canvas_card` *Fill with content*, 520 wide, `CardHold::Open`,
  replanned when an option, the page, the revision or the selection changes (`FillBox::replan`),
  and a row's click pins the roots it looked under as Clean up's does. Opening it closes the
  palette, the context menu, the top-bar menu, the colour picker, the path bar's Clean up card and
  the other cards over the canvas's top; opening any of those closes it. Enter stays the canvas's.
  *Cancel* or ✕ closes it, and Escape through `OndinApp::escape`'s rung below Clean up's (an open
  kind list takes the press first). It is drawn outside the `!focus` guard.
- **Opening**: the box opens on any selection, none included, and follows the selection made
  after; while nothing is fillable the preview says *Select text layers, or frames that hold them*.
  The locale is `agreed_locale`, else the last used, read again each time the selection changes
  until a locale is picked in the row, which holds it (`FillBox::locale_picked`); records, example
  emails, avatars and the default avatar style come from `prefs.fill_content`; the seed is the
  clock's, so each opening draws anew.
- **Fill** ends a live text session first (`OndinApp::finish_text_first`) and plans again on what
  that leaves: the session's buffer predates the fill, and its commit when the session ended would
  write over the value filled.
- **Rows** (batch rename's `row`): *Locale*, a combo of the locales, the current one lit with a
  check. *Content*, a combo of *By layer name* and each text kind under its group's eyebrow, its
  sample in the locale as a faint tail (`MenuRow::note`, `content_fill::sample`); picking a kind
  there drops the text rows' own choices and keeps the shapes'. *Records*, a `switch_row_noted`
  *Keep each {noun}'s values together* noted *3 rows*. *Emails*, a switch *Use example.com
  addresses*. *Avatars*, a `switch_row_noted` labelled with the shape count; each shape's style is
  picked on its preview row. Locale and Content stand 10pt apart; the three switch rows 2pt, Clean
  up's `SWITCH_GAP`. While records or avatars is
  off, both counts come from `content_fill::outline` with both on (the rows and records, no value
  drawn), so an off switch still says what it would act on without a second full plan.
- **Preview**: the eyebrow *Preview*, *13 layers in 3 rows · 1 left as is* and a `DICE` button,
  *Shuffle* (*Shuffle · Draw new values for every layer*), which takes a new seed and clears the
  redraws. Rows sit under a dim head naming their record's frame, scrolling past twelve with no
  bar, as Clean up's. A row is 32pt tall, the kind combo's `ui::TOOLBAR_H` and 8pt, so stacked
  combos do not read as one column of fields. A row: the layer's glyph, *Parent / Name* (masked in
  private mode), a kind combo (a text row: *Leave as is* and every text kind; a shape row: *Leave
  as is*, *Avatar · Initials* and *Avatar · Pattern*, the face naming the pick the same way; a
  shape with no pick of its own shows the box's style), then the value: a kept row's
  current text, faint; an overflow in warn ink with *Longer than its box: no draw fitted*; an avatar
  as its ground swatch with the initials, then the name. A hovered row with a kind shows a `DICE`
  *Draw another*, but for a *Country* row: the locale has one country. Hovering a row rings its layer, a head its frame at a hairline
  (`canvas::draw_panel_hover`); a click selects the layer and brings it into view.
- **Foot**: `card_foot` with *Esc closes*, *Cancel* and *Fill N layers*; dimmed as *Fill* with
  *Every layer is left as is*, or *Select text layers, or frames that hold them* when nothing is
  listed.
- **Commit**: one undo step *Fill with content* with `FILL_CONTENT`; the status line says *Filled N
  layers · Ctrl+Z to undo*, with *· skipped K locked* before the hint when the plan skipped any.
  Locale, records, example emails and avatars are saved to `prefs.fill_content`
  (`FillContentPrefs`); the styles picked on rows become the next box's default style only when
  they all agree.
- **Doors.** *Tools ▸ Fill with content…* (after *Clean up…*); the palette (`Item::FillContent` in
  the Text group, aliases *lorem*, *placeholder*, *fake*, *sample*, *dummy*, *names*, *data*); the
  text layers' context menu, after the sizing rows. None is dimmed for an empty or unfillable
  selection: the box opens and says what to select. `Action::FillContent` has no
  chord. `DICE` means drawing new values (*Shuffle*, *Draw another*); `FILL_CONTENT`, lines of text
  with a small die, is the act and its undo step.

## Assets browser (`assets/`, `panels/credit.rs`)

Open-source icons and stock photos, inserted with their credit (`Node::credit`, §5.3).

- **Providers.** Icon sets (`sets::SETS`): Phosphor, Lucide, Tabler, Material Symbols, Heroicons
  and Font Awesome Free, each an Iconify JSON package pinned to one version, fetched whole from
  jsDelivr the first time it is needed and kept under `machine_dir::root_or_temp()/assets`, so it
  works offline after (written beside and renamed; a package that will not read is deleted and
  fetched again). A set's variants are name suffixes inside a package, the longest that fits
  winning, or a second package (Font Awesome's Solid and Regular). Iconify has no tags, so search
  ranks the name, then aliases. Photo services (`photos::SERVICES`): Unsplash and Pexels, each with
  the user's own key, and Openverse with none, searched only for licences allowing commercial use
  and modification. Every request runs on the module's workers, with the font service's deadlines.
  A thumbnail is decoded through the insert's bounded decoder and fitted into 320 × 960 (an empty
  one is refused: a zero-sized texture is fatal to wgpu); a first page landing releases the
  thumbnails it does not show and forgets a failed one, which is then asked for again. Nothing is
  on by default, and nothing reaches the network until a set or service is switched on.
- **Keys** live in the system keychain (`keychain::read_account`, `store_account`,
  `forget_account`; account `<service>-key`, the Figma token's `figma-token` beside them), read and
  written on workers, and sent only in their own service's `Authorization` header. They are read
  once a session, and only once a keyed service is on (a Linux store may ask to unlock); searches
  and reports take the session's keys, reading the keychain only while they are unread. A write
  is read back, so the panel and the form believe what the store kept, and a refused write is said
  in the status line. Unsplash's links carry its referral `utm_` parameters, and each Unsplash
  photo is reported to its `download_location` once its insert is committed, never for a preview
  or a fetch that was replaced — with the key only when that address is Unsplash's API, over
  https. A fetch replaced by another click or a document switch stops before its next download.
- **The panel** (`panel.rs`) is a `ui::rail_popup` 340 wide, bottom-aligned with its rail button
  (`icon::ASSETS`, the top of the rail's foot: *Assets*, *Styles and variables*, *Insert
  components*). It is one of three panels in that place, with the insert picker and the styles
  browser, each beside its own button; opening one closes the others. Doors: the rail button and *Assets…* in the
  palette; no chord. `Escape` closes it, on the insert picker's rung. *Icons* · *Photos* on
  `ui::segmented`, then `ui::search_box`, which takes the keyboard on opening.
  - *Icons*: a set dropdown (*All sets* or one) with the set's variant dropdown (a long label cut,
    so the panel keeps its width) and a size field defaulting to the variant's own grid when its
    icons share one (`IconSet::grid_of`: Heroicons' Mini at 20, Micro at 16), else the set's (a grid
    over 64 units, Phosphor's 256, draws at 24); a grid six
    tiles across, 44 tall, 120 shown and more as the end scrolls into view; a set still downloading
    shows its progress row.
  - *Photos*: service and orientation dropdowns, then two columns at each photo's aspect, the next
    page asked for when the end scrolls into view, one at a time. A later page joins the shown one
    less any photo already shown; one that fails or comes back empty ends the paging and leaves
    the grid. A search goes out once typing rests 0.4 s, or at once on `Enter`.
  - The foot caption names the set or service and its licence, and the count. A tile resting 0.5 s
    shows a hover card beside the panel on the insert picker's ground (`ui::hover_card_frame`):
    an icon's name, set, variant, size,
    *Also:* its aliases and licence; a photo's title, credit sentence and the size and licence it
    comes in at; then what a click and a drag do.
  - Empty states (`ui::empty_state`) say what to do: no set or service on, a keyed service without
    its key, a key refused, the hourly limit, an unreachable set or service (`icon::WIFI_SLASH`,
    *Try again*); the first three offer *Open settings*. They sit 40pt under the tabs, the styles
    and variables browser's air.
- **Inserting.** A click places at the view's centre and selects it; a drag drops at the pointer.
  The parent is the frame under that point (`draw_target`, through `place_svg` for an icon and
  `image_tx` for a photo); a flex frame's gap is not chosen. An icon is the SVG importer's group
  named after the icon, `size` points across, its `currentColor` written black, the ink of a new
  text layer. A photo is fetched at Settings' *Photo size* (Openverse serves only the original) and
  embedded. With unlocked shapes or frames selected (not text, a line, a spiral or an arc, nor a
  layer inside an inserted icon, whose one credit is the icon's), a photo's
  click fills them instead (the picture fill or top fill replaced, else a new fill), each a
  different photo in grid order from the one clicked. A locked layer in the selection is left out
  of the fill rather than making the click a placement, and the strip counts it (*Click fills 3 of
  4 layers*). At landing a target that is no longer
  fillable (locked meanwhile) is skipped, and a photo that failed to download leaves its target
  as it was while the rest land (*Filled 2 layers · 1 photo could not be downloaded*); a strip says
  *Click fills Hero image*, and `Alt`-click places. Every insert is one undo step carrying its
  credit: *Insert icon*, *Insert photo*, *Fill with photo*. The status says *Placed Arrow right ·
  Ctrl+Z to undo*, or *Filled Hero image · Photo by … · Ctrl+Z to undo* (names through private
  mode). A fetched photo lands when no gesture is live and no modal is up, even with the panel
  closed or in focus mode (`assets_pass` runs outside the panels' guard); one asked for in a
  document left behind is dropped. The panel's choices are written to the preferences once no
  button is down, not on each frame of a scrubbed size.
- **Settings ▸ Assets**, after the web fonts: a caption, then *Icon sets* and *Photos*, a
  `ui::switch_row_noted` per set or service noted with its licence or what it needs; under a keyed
  service that is on, its key in `ui::secret_field` and a caption saying where a key is made and
  that it goes to that service only; *Photo size* (*Small · 400 px*, *Regular · 1080 px*, *Full
  size*); and the asset cache row with *Clear*, the web-font row's twin, which empties at once as
  that one does. The rest is staged until *Save changes*; a key field left empty forgets its key.
- **Credits…** (`icon::QUOTES`) is offered while the card would list something
  (`OndinApp::has_credits`: a layer carries a credit whose licence asks to be shown). The Document
  menu hides it otherwise, since most documents never carry a credit; the palette lists it disabled
  with the card's own words, *Nothing here asks to be credited* (`credit::NOTHING_OWED`). It opens a `settings::canvas_card` listing every page's
  credits whose licence asks for them (`Credit::required`), photos first: a photo a line, an icon
  set's one sentence a line for all its layers, each naming its layer (masked in private mode) or
  counting them as a `MenuRow::note`. A row's click selects its layers, going to their page when
  none is on this one; the foot's *Click a row to select its layers* shows only while rows are
  listed. *Copy all* puts one plain line per credit on the clipboard. When any credit is left out,
  a note says MIT, ISC and Apache icons and public-domain photos ask for no credit and are not
  listed; with none listed the card reads *Nothing here asks to be credited*, at the eyebrows' 6pt
  inset, level with the head. The
  inspector's *Credit* card is in `inspector.md`.

## Find and replace (`find.rs`)

Find and replace in what text layers say; layer names are batch rename's. The rule is
`ondin_core::find`, which shares batch rename's matcher (`rename::Rule`).

- **Core.** `Query::new(find, replace, match_case, whole_words, pattern, keep_case)`: the
  replacement is literal text unless `pattern` is set, when it is a rename `Template`, so `[1]`
  inserts group 1 and `[Text]` the whole of the text searched (`Rule::replacements`). Whole words wrap the pattern in `\b{start-half}(?:…)\b{end-half}`
  (`Find::with_words`): no word character just outside either end, so *C++* and *#tag* are found as
  whole words, which `\b` would refuse. `Scope` is
  `Under(roots)` (those layers and all under them), `Page(p)`, or `Pages { here }`, every page but
  the drafts while keeping `here`, as batch rename's gather does. `find` orders the `Hit`s by page in
  document order, then by layer in the layers panel's order (top of the stack first), then by place
  in the content. A `Hit` carries its node, page, byte range, `locked` and `hidden` (self or an
  ancestor), `instance` (the instance root it is a copy inside) and `main` (the main it is part of,
  with its copy count). `Hit::writable(inside_instances)` is never true when locked, and inside an
  instance only when asked. `replace` builds one `SetText` per layer, applied back to front, and
  leaves alone a hit whose range no longer matches; the replacement takes the first replaced byte's
  character attributes (`Spans::replaced`, `text.md`).
- **Stored text only.** Matching reads `content`: a case transform is drawn, not stored, and list
  markers are not text. Text whose content is bound to a variable stores a placeholder and is skipped.
- **Doors.** `Ctrl+F` (`Action::FindText { replace: false }`) opens on Find; `Ctrl+H` (⌥⌘F on macOS, where
  ⌘H hides the app) opens with the Replace row showing, unshifted only, since `Ctrl+Shift+H` is
  hide/show. Both are normal-mode
  chords. The palette rows *Find in text…* and *Replace text…* (`Item::FindText`, `ReplaceText`,
  State group) are also in the cheatsheet's Layer section; *Tools ▸ Find and replace…* opens on
  Find, as `Ctrl+F` does. Opening again while open only takes the
  keyboard back.
- **Placement.** Not a modal, so the canvas stays live and the matches are found again as the text
  changes. An `Area` at the canvas's top right, 420 wide, 16 below the top (below the rulers when
  they show) and 16 left of `inspector_column`, or of the canvas's right edge in focus mode, where it
  is drawn too. The list shows ten rows at most, fewer when the canvas is short, and scrolls without
  a bar. With Find empty there is no list and no rule under the fields. `OndinApp::find`
  (`FindBox`) is kept while closed, so the box reopens as it was left.
- **Rows.** A caret folds the Replace row and the options under it; the fold is remembered. Find
  shows *N of M*, a warm *0* (a word took the field's room), or *Not a pattern* with the reason under it, then ↑ ↓ ✕.
  The Find field holds the switches *Aa*, *ab* (whole words) and *.\** inside it, and Replace's
  field holds *Keep case* (*Ab*, on the first time the box opens), all `ui::field_toggle`s as in
  Batch rename's fields; then *Replace* and a primary *All*. Then the scope *Selection · This page ·
  All pages*: it opens on *Selection* when something is selected, else *This page*, and *Selection*
  is disabled with nothing selected. Last, *Include text inside instances*. The fields, the scope
  (`batch_rename::scope_segments`) and the tick box (`ui::tick_box`) are Batch rename's.
- **The list**, grouped under page headings with their counts. A row is a list row
  (`ui::MENU_ROW_H`, `ui::list_row_ground`, lit when current). It has the kind glyph (text, an
  outlined hexagon inside an instance, a filled accent hexagon in a main), the text around the match
  with the match lit, and the layer name, followed for a main with copies by `· N`, its copy count. Its tail is a lock when
  locked, a slashed eye when hidden, else `TEXT_AA` for a case-transformed run. A row a replace would
  skip is drawn at 45%. The footer reads *N matches in M layers · K inside an instance skipped · L
  locked*. An empty list reads *No layers in ‹the selection / this page / any page› match* (`ui::empty_note`) with a
  button widening the scope (*Search this page*, *Search all pages*).
- **Keys.** `Enter` in Find steps to the next match and `Shift+Enter` to the previous, round the
  ends; `Enter` in Replace replaces. The fields keep the keyboard (`return_key(None)`). `Escape` is a
  rung of `OndinApp::escape` just below a text edit, so a text edit opened from the box finishes
  first; the box reads no `Escape` of its own.
- **Showing a match never selects it.** Stepping switches page (`go_to_page`) if it must, then pans
  the match itself into view by the least amount with the zoom kept (`OndinApp::reveal`, which
  `reveal_selection` also calls; the layer's bounds when the match cannot be laid out), so a long
  text's later lines come into view. A click on a row makes it current; a double-click edits that text
  with the match selected (`begin_edit_text`, then `TextEdit::select_range`).
- **Replacing.** *Replace* writes the current match if it is writable, else steps over it, and then
  makes current the first match after what it wrote, so replacing *team* with *teams* moves on. *All*
  writes every writable match as one undo step named *Replace text*; its *Replaced N in M layers ·
  Ctrl+Z to undo* goes to the footer and the status line. A replace in a main reaches its copies
  through the commit's propagation.
- **On the canvas** `paint_find_marks`, drawn under the edited text's own highlight, tints this
  page's matches accent (grey when not writable) and rings the current one. The quads come from
  `TextEdit::range_quads`, cached per node per revision.
- **Private mode** masks the layer and page names in the list. The text is the work, so it is
  searched and shown.

## Colour review (`colour_review.rs`)

Groups the near-identical colours of the selection or the document and merges a group into one
colour, one undo step each. The rule is `ondin_core::color_review`.

- **Census.** `census(doc, roots)` counts every raw colour under `roots` — solid fills and strokes,
  gradient stops, drop and inner shadow colours, a text layer's `TextStyle::color` and its
  `CharAttr::Color` runs — and each colour variable a fill or stroke is bound to, keyed by variable
  and binding alpha and valued at the default mode. Hidden paints count: a merge that skipped them
  would bring the old colour back when one is switched on. Decoration, layout-grid and guide colours
  do not count. `roots` is the selection or every page's root, never the vault.
- **Groups.** `groups(census, Threshold)` takes anchors most-used first, and every member sits within
  `Threshold::limit()` of its anchor itself (`delta`, OKLab distance ×100: *Tight* 1, *Close* 2,
  *Loose* 4), so A≈B≈C cannot chain; a colour taken is no anchor. Members share their alpha byte. A
  group shows only with two members or more, one of them raw.
- **Merging.** `Group::default_target` is the most-used bound member, else the anchor; `merged` is
  the raw members but the target, so a bound member is never merged away, and a kit's variable may
  be the target. `merge` is one transaction: into a raw target the merged places take its colour;
  into a bound one fills and strokes bind to its variable (the binding alpha keeping the drawn alpha)
  and stops, shadow and text colours take its default-mode value (`Merged::unbound`). So does a fill
  or stroke the variable's scope refuses, and every place on a layer where the variable draws
  another colour than its default mode's, which is the value the census matched: a bind there would
  change the colour by more than the threshold. Effectively locked layers are never written
  (`Merged::locked`). `merge_into_new` adds *Mode 1* to a document with no modes, a variable valued
  the target's colour in every mode, and binds every raw member's users, the target's included;
  with every use on a locked layer it writes nothing, the variable included.
- **Doors and place.** *Tools ▸ Colour review…*, after *Simulate vision* and never ticked,
  and the palette (`Item::ColourReview`, unbound). It hangs at the palette's place
  (`ui::canvas_popup_area`), 420 wide, on the settings card's ground (`settings::card_frame`), but is
  no modal: nothing blocks the canvas and a click there leaves it open. It is at most
  `ui::canvas_popup_room` tall, its list scrolling past `ui::CANVAS_POPUP_LIST_H`; title, controls
  and foot stay put. One card over the canvas's top shows at a time: opening it, Find, batch
  rename, Clean up, Fill with content or the chart data window closes the others, since centred one would run under Find's corner. The census is taken again when the revision, the scope or the threshold changes.
- **Head.** Batch rename's title row (*Colour review*, ✕). Scope *Selection · Document*, opening on *Selection* when something is selected;
  *Selection* is disabled with *Nothing is selected* when nothing is, and a card left on it says
  *Nothing selected · reviewing the document*. Threshold *Tight · Close · Loose*, *Close* by default,
  kept in `prefs.colour_review` across documents. Caption: *N groups · M colours could be N · Δ ≤ x*.
- **Groups**, most-used first, the first unfolded (again after a merge). A folded row: caret, a split
  patch with a slice per member, the target's name (hex, or the variable's capsule, a kit's after
  the package glyph) and *N colours · M uses*. A member row: the merge glyph (`icon::ARROWS_MERGE`, accent) on the
  target, in the tick boxes' column, then swatch, hex or
  capsule (a kit's reads *Kit · name*, the kit masked in private mode as on every kit surface),
  *kept* on a bound member that is not the target, uses, *anchor* or *Δ x.x*, and a reticle
  selecting that colour's users on this page, else on the first one's page, through
  `select_by_color`; on *Selection* the card keeps reviewing what was selected before
  (`ColourReview::reticle`) until the selection changes some other way. Rows wear `ui::list_row_ground`, hovered under the pointer and lit on the
  target; a click anywhere on a member row but its box and reticle makes it the target. The row
  says so through the foot rather than a tooltip: while the pointer is on another member's row
  (not on its box or reticle), the foot's left reads *Click to merge into X instead*.
- **Skips.** Every raw member but the target carries a tick box (`ui::paint_tick_box`), ticked
  while it merges; unticked, the colour is skipped: its words fade by `ui::UNTICKED_FADE` (its
  swatch keeps its true colour) and *skipped* replaces *anchor* or *Δ x.x*. A merge is handed the
  group without its skipped members, so they keep their colour and the button and the foot count
  only the rest. Skips are kept by colour (`ColourReview::skipped`) across merges and thresholds,
  so a colour skipped beside the one it was near is not offered again once the merge lands; picking
  a skipped colour as the target ticks it, and another document coming to the front forgets them
  all (`reset_transient_state`). A group the skips left nothing to merge reads *N colours · all
  skipped*, its merge disabled with *Every other colour is skipped*, and goes below the rest when
  the census is taken again (not as the box is unticked, so it does not jump away); the caption
  counts only the rest and adds *· K skipped*.
- **Foot.** Batch rename's: below a divider, what the merge does on the left (*Rewrites U uses*,
  *Binds U uses*, or *Unfold a group to merge it* beside a disabled *Merge*; cut short with a `…`
  before the buttons) and a `ui::split_button`
  *Merge N into X* on the right, merging the unfolded group; its caret holds *Merge into X* and
  *Merge into a new variable…*, the second disabled with *The group has a variable* when the target
  is bound. The button keeps the last choice. Naming a new variable turns the foot into *New colour variable ·
  value HEX in every mode*, a field seeded `color/` with the caret at its end, *Cancel* and *Create
  and merge*; a name is refused as the variables browser refuses one
  (`variable::name_refusal`).
- **Status.** Undo steps *Merge colours* and *Merge into a new variable*. *Merged N colours into X · U
  uses on L layers*, then *· K took its value instead of binding* and *· K on locked layers left*
  when they apply. The first names no reason, for it has several: stops, shadows and text colours
  never bind, a field the variable's scope refuses does not, and nor does a layer in a mode where
  the variable draws another colour. With no group: *No near-identical colours at
  ‹threshold›* over how many colours the scope holds.
- **Keys.** `Escape` is a rung of `OndinApp::escape` below Find's: the caret menu, then the naming,
  then the card.

## The shortcut cheatsheet (`cheatsheet.rs`)

- **Doors.** `Ctrl+Shift+/`, ⌃⇧/ on macOS (and `Key::Questionmark`), in the editor (`cheatsheet_pressed`, both modes)
  and in the library (`dashboard_keys`, through `input::cheatsheet_chord`), and the palette's
  *Keyboard shortcuts* row. Opening it closes the palette and any context menu.
- **The card.** A `settings::card_modal`, 960 wide and at most 640 tall, each shrunk to the window
  less 24 a side: title, a filter field and ✕; the body scrolls. It counts in `modal_is_up` and
  `library_keys_are_free`, and while it is up a text session under it reads no keys, even once a
  click on the card has taken the filter's focus (`text_mode_input`). It
  closes on Escape, ✕, the backdrop, or the chord again — read in `cheatsheet_ui` itself, since
  `resolve` does not run under a modal, and not on the frame that opened it (`just_opened`).
- **Rows** are the registry's items with an accel: label and spelling from `Item::spec` (*Hide /
  Show*, *Lock / Unlock* and *Paste* name the verb rather than one state), alternatives from
  `Item::alt_accel` joined by "or". A row is never dimmed for availability and never runs. Mouse,
  Tabs and Library are hand-written rows: `tabs::read_keys` reads the tab chords before the keymap,
  so the chord test, which drives the keymap, cannot cover them. Layer's *Opacity 10–100%* and
  *Blend mode* (`Shift+3–9`) rows are hand-written too: a digit is a value, not a registry verb.
- **Sections** (`sections`): Tools, Zoom, Pages, Edit, Structure, Arrange, Layer, View, Components and
  kits, File, Tabs, Text editing, Mouse, Library, dealt in order into three columns, each filled to about
  its share; a section is never split. *Lasso select* sits in Tools, beside the tools whose marquee
  it shapes. A narrow window deals into two or one, so no column is under
  220 wide (`column_count`). Opened from the library, its section leads, followed by an *In a
  file* divider; opened in a text session, the text keys lead.
- **Filter and hold.** Every filter word must start a label word or a key name. Holding modifiers
  with an empty filter dims every row whose chord does not use exactly those modifiers — only once
  every modifier has been let go since the sheet opened, so the opening chord's own keys do not.
- **Hand-kept, a known duplication.** The Mouse section, the opacity-digit row and the Library
  section have no registry rows (gestures and opacity digits are not verbs, and the library's keys
  are read by `dashboard_keys`, not `input::resolve`), so a change to one of those keys is made here
  too. Which items each section lists is also chosen by hand in `sections`.
- **What its test covers.** `every_sheet_chord_resolves_to_its_rows_action` walks `sections`, so
  a new row is covered, and drives every registry chord and its alternative through
  `input::resolve` as `egui_winit` delivers it: a command chord on C, X or V arrives as a
  clipboard event, and the key's release follows on the next frame. Each must resolve to its
  row's `Item::action` and nothing else; the one exemption is `PasteRelease` beside a paste, the
  image-clipboard witness `dispatch` stands down. A sheet row with no `Item::action` fails it, and
  an item `sections` names that has lost its chord trips `items_section`'s assertion.
  `every_bound_action_is_on_the_sheet` runs the other way: every key under every modifier set, in
  both modes, must resolve to an action on the sheet or to one kept off it (nudge, size and point
  steps, Escape, Enter, `PasteRelease`, opacity digits). It checks actions, not spellings. A
  table chord's spelling cannot drift from its binding, since both are one `input::BINDINGS` row;
  a second key read in code (`F2`, `Backspace`) is `Item::alt_accel`'s to carry by hand.

## Focus mode

Focus mode (`OndinApp::focus`, `ViewSwitch::Focus`, `Ctrl+\`, the palette's *focus* or *zen*) is
a normal editing mode with the panels out of the way. Present mode is a separate, view-only mode and
none of the rules below are its.

- Hides exactly: tab strip, top bar, layers pane, tool rail, inspector, and the inspector's popups (picker,
  kit picker, var browser, field menu, var autocomplete, mode menu, style popups). Everything edited
  *with* stays — ruler bars, guides, layout grids, selection box, handles, size badge, hover outline,
  frame tags — and the canvas works normally, so a guide can be pulled out of a ruler. The rulers
  are an editing aid like guides and layout grids, not app chrome. The **floating status bar stays**
  (it carries *"Focus mode — Escape to leave"*), and so does the minimap.
- It hides the tab strip too, which is the title bar on Windows/Linux, so caption buttons go; `chrome::focus_strip`
  shows them in a 32pt strip when the pointer reaches the window's top edge with no button held, keeps
  it while hovered or a button is down. Approaching from the canvas below shows nothing.
- It **overrides** the switches it reads (layers, toolbar) without changing them, so leaving
  restores the user's workspace. Every View row stays live in the canvas menu and the palette; the
  layer menu's *Rename* dims (*"Leave focus mode to rename"*), since its field is in the hidden panel.
- Escape is handled **first** in `OndinApp::escape`. A context menu open in focus mode takes the
  first Escape (R3 returns before `escape`), the second leaves the mode. The popover arm asks for a
  popover heard last frame (focus mode draws none). Kit dialogs are drawn outside the `!focus`
  guard so they get the first Escape.
- **Every flag `modal_is_up` reads must be a card drawn whenever it is set**, or an undrawn card holds
  every key.

## The peek

Holding bare `\` shows the artwork with the canvas chrome hidden; letting go brings it back. Held,
not toggled: a live selection that cannot be seen would take a nudge.

- **The key** (`canvas::peek_key_held`, `peek_held`): held from the key's own press to its own
  release, kept in `OndinApp::peek_key`, so auto-repeat changes nothing. The press counts without
  `Ctrl`/`Cmd` (those spell the panel toggles' `\`); `Alt` is allowed, which some Mac layouts type
  `\` with. The press must type `\` (or `|` on the `\` key), so an ISO layout's `#` on that key is
  no peek. `peek_key` keeps the press's physical key, and the release counts on it however the
  modifiers left held spell it (`|` under `Shift`, `/` once `Option` is up), so a peek never sticks
  on. Not while a control holds the keyboard; a window that
  loses focus ends it, whose release would never arrive. Where `\` needs `AltGr` (`Ctrl+Alt` on
  Windows) the chord is *Show/hide layers* instead, and there is no peek: §12's `AltGr` question.
- **A press made during the peek** sits out to its release (`gesture_cancelled`), so it does not
  come back as a click once the peek ends.
- **Only from rest.** `OndinApp::canvas_ui` sets `OndinApp::peeking` each frame, and only in
  `Mode::Normal` with no drag, no compare (which spends bare `\` on its own toggle) and no armed
  eyedropper: a gesture never loses its release to a peek, and in a text session `\` types.
- **What shows.** `draw_canvas` draws the artwork texture (vision simulation, outline mode and pixel
  preview still apply: they are view modes, not chrome) without the adaptive grid's shading, then
  only the ruler bars, which frame it, and returns before every other overlay.
  `draw_rulers_and_guides` skips the guides while peeking and the bars drop the selection's band,
  so no trace of the selection shows; `contrast_badges` is skipped too.
- **Presses do nothing**: the mode input and `canvas_context_menu` are skipped and the cursor is the
  plain arrow. The wheel and a pinch still pan and zoom.
- The cheatsheet lists it as a hand-kept row in its Mouse section, since no binding renders it.
- `holding_backslash_hides_the_chrome_until_released` pins the hiding and the end on lost focus.

## Private mode (`private.rs`)

For sharing a screen without showing what things are called: a client's or a company's name is often
the first segment of a component's. It hides names, not content.

- **Doors.** `ViewSwitch::Private` (*Private mode*, `Ctrl+Shift+N`): the View menu's modes, between
  *Outline mode* and *Focus mode*, the palette and the cheatsheet; not the canvas menu. The library answers the chord too
  (`dashboard_keys`). An accent *Private mode* pill (`private::pill`, the incognito glyph, hat and
  glasses) sits on both the editor's and the library's top bar while it is on; clicking it turns it
  off. It names the mode rather than reading "Private": the app is personal and offline-first, and a
  bare "Private" read as a document not shared with a team.
- **A preference, not a workspace switch** (`Prefs::private`), so a restart in the middle of a shared
  screen comes back masked. `OndinApp::set_private` writes and
  saves it, sets the switch at once and, turning on, drops whatever shows a real name for editing:
  the inline layer rename, the inspector's
  name buffer, batch rename, a page rename, layer search, the library's document rename and project
  edit.
- **The switch is a thread-local** (`private::on`/`set`), copied from the preference once a frame at
  the top of `OndinApp::ui`, so any view can mask without a handle on the app.
- **Masked as kind and place.** `private::layer` gives "‹Kind› ‹n›", n the layer's place among its
  siblings of that word ("Component" for a main, "Instance" for a linked copy, else
  `NodeKind::default_name`); `page` gives "Page ‹n›" by page order; `document` and `project` give
  "Document ‹n›" and "Project ‹n›" by place in the list drawn beside them (the tab strip, the
  library's entries or project list); `version` gives "Version ‹n›" by place in the version history.
  So one document can carry different numbers on different
  surfaces. `OndinApp::shown_page_name`, `shown_title`, `shown_project_name` and
  `Library::shown_project`/`shown_entry` are the drawn names; `page_name` stays the real one for
  comparing, renaming and file names. Kit names are document names (`kits::index::kits_in`,
  `binding::kit_label`, `kit_heading`).
- **What is masked:** layer, frame, component, page, document, project and version names, everywhere one is
  drawn — canvas tags, inspector headers, panels, palette, menus, toasts, the contrast check, the
  vision simulation's bar, the minimap, find and replace's list, a compare's bar, card and mark tags, the version card's summaries and opened rows (trails dropped, and what belongs to no page by kind alone, so variable and style names hide there too), present mode, exports' toasts (a single export's file name becomes a count), tabs, crumb,
  kit dialogs and the whole library screen. A main's or set's description and documentation link
  are masked with the names: the description field is disabled and empty (*Description hidden in
  private mode*), the link field reads *Link hidden in private mode*, an instance shows no description
  note and its book's tooltip drops the host, the swap picker hovers no description, and the insert
  picker's hover card shows only the masked name and the variant count. **What stays:** text, images,
  colours, style, variable and variant property names. Masked, not removed: a frame label still selects its frame, rows still
  navigate.
- **Pages and Layers fold as an overlay** (`pages_folded || private`, `layers_folded || private`),
  the fold preferences left as they were; a click on a folded head says *Leave private mode to see the
  names*. The History section is hidden, its steps carrying layer names.
- **Doors that need the real name decline** with a toast: rename (`rename_selection`, and the
  inspector's layer and page name header, drawn masked), batch rename, page rename, the library's
  document rename and *Edit project* (*Leave private mode to rename*), *Import files* (*Leave
  private mode to import*) and *Combine as variants*, whose values would be the mains' names
  (`private::DECLINE_COMBINE`). A version's pencil is disabled with the rename words as its tooltip,
  and the version card drops an open name field, so a name is never typed blind. Searches over names find nothing and say *Leave private mode to search
  names*: the palette's layer search, the kit picker's, the library's, whose remembered searches
  are hidden too, and the swap picker's, which lists every main. A property made from a layer
  (`bind_line`'s *New property…*, a new swap property) is named for the layer as drawn. A swap
  property's filter, a prefix of a main's name, is drawn as *Filtered by name* and not edited.
- **Failed writes name no file**: `atomic::write` keeps the path apart from its message, and the
  status lines of a failed save, version, duplicate, move, restore, rename or new file go through
  `private::io_error`, which leaves the path out. The library settings' list of files a migration
  left behind masks their names.
- **Known limits.** The OS save dialog an export opens prefills a file name made from the layer's or
  document's name; the export folder path in a toast is shown as is; a toast written before private
  mode was turned on keeps its text until it fades. No test reaches a tooltip: its ink never enters
  `FullOutput::shapes`, so the masked tooltips (the tab's cut name, the crumb's, the minimap's frame
  names, a kit card's back link) are held by review alone.

## Present mode (`present.rs`)

The document's top-level frames shown one at a time, page by page, fitted to a fullscreen window,
with nothing of the editor around them and nothing to edit with. `OndinApp::presenting` holds a
`Presenting { frames, pages, entered_at, at, was_fullscreen, stepped_at, view, to_top }` while it
runs (`pages` names each frame's page, index for index).

- **Doors.** `Action::Present`: `Ctrl+Alt+Enter` in normal mode, `menu::Item::Present` (*Present mode*,
  `PLAY`, `Group::View`) in the palette and the cheatsheet's File section, and the last row of the View
  menu's modes. The palette row dims *"No frames to present"* on the same test `start_presenting` makes
  (`presentable`: the current page's visible top-level frames with bounds).
- **Starting** (`start_presenting`). The frames are every page's that is not a draft, in page
  order — and the current page's even when it is a draft, since presenting from it asked for it.
  Each page gives its visible top-level frames with world bounds (`presentable_on`) in
  `reading_order`: rows top to bottom, each left to right, a frame joining the row above while
  its top is above that row's first frame's middle. It opens on the frame holding the selection,
  else the one with most area in the current view, else the current page's first. It finishes a live text session,
  closes the context menu and palette, and sends `ViewportCommand::Fullscreen(true)`, remembering
  whether the window already was. With nothing to show, the status line says *"No frames to
  present"* and nothing changes.
- **Its own branch of `OndinApp::ui`**, after the font poll and before keyboard routing: it runs
  `disk_results`, `autosave_tick` and `recovery_tick`, draws `present_ui` in a frameless
  `CentralPanel` and returns. No other key, panel, overlay or command runs, so nothing else needs to
  know the mode exists. A modal up ends it first, since a modal holds every key (closing the window
  asks one, which is how a close gets out of fullscreen); so does the dashboard's branch, which
  comes earlier.
- **Keys** (`present_ui`). → ↓ Space Page Down Enter and a click step on; ← ↑ Page Up Backspace
  and a right-click step back, stopping at either end; Home and End jump; `Ctrl+PageDown` and
  `Ctrl+PageUp` jump to the first frame of the next or previous page (`Presenting::jump_page`);
  Escape leaves.
  `stop_presenting` leaves fullscreen unless the window was fullscreen before. A shown frame that no
  longer resolves ends it.
- **Zoom**, because a tall frame fitted to the screen's height is too small to read. `Ctrl`+wheel
  (or `Alt`+wheel) or a trackpad or touchscreen pinch zooms in at the cursor (a touch pinch at its
  fingers' centre, and its first finger's release is no tap); the plain wheel and `Shift`+wheel
  then scroll.
  The wheel and the pinch are read as the canvas reads them (`canvas::wheel_travel`,
  `canvas::wheel_zoom_factor`, `canvas::pinch_zoom`).
  `hold_on_frame` holds the view on the frame, every frame so a resize refits: on each axis the
  frame stays centred while it is narrower than the screen, and once wider its edges stay outside
  the screen's. Zooming out to the fit or past it is the fit (`view` back to `None`). The zoom is
  kept across steps, since frames presented together are usually one size; a step to another frame
  (keys, click, Home/End) sets `to_top`, and the next frame drawn opens at the new frame's top,
  centred across. A frame the kept zoom no longer magnifies opens fitted, and the zoom is dropped.
- **Drawing.** The scene goes through `canvas.render` with present mode's own camera: `view`, else
  centred on the frame at `fit_zoom` (the tighter axis, no margin, clamped to `MIN_ZOOM..MAX_ZOOM`).
  The editing camera and selection are untouched, so leaving restores the view. A matte in the
  ground of the frame's page then covers everything outside the frame: its neighbours, and a child
  overflowing an unclipped frame. No selection, guides, grids, frame tags, rulers or status bar.
- **The counter**, a pill at the bottom with the frame's name and *n / total*, shows for
  `COUNTER_HOLD` (1.5 s) after a step and fades over `COUNTER_FADE` (0.4 s).
- **The page chip** (`page_chip`), top left, reads *‹name› · page n of m*, counting the pages
  presented. It shows on entering a page (from `entered_at`) and fades as the counter does; with
  one page presented there is none.

## Traps

- Folding `focus` into some overlay's predicate, or back into `rulers_on()`: focus mode hides
  panels, not what the canvas is edited with; read §15 D750 first.
  `focus_mode_leaves_the_guides_and_rulers_on_screen_and_live` pins the rulers and guides.
- A modal flag set behind a `!focus` draw guard swallows all keys in focus mode.
- `minimap_ui` sits before the canvas and outside the `!focus` guard. Moving it beside
  `status_bar` lags every pan it makes by a frame; moving it into the guard drops it from focus
  mode (`focus_mode_keeps_the_map`).
- `contrast_claim_keys` takes the fix card's Enter at the top of the frame, before `input::resolve`,
  which would otherwise open the selected text layer for editing
  (`enter_applies_the_card_s_first_fix`), and before the palette's keys, so it stands aside while
  the palette, a modal or a text field has the keyboard
  (`the_palette_keeps_its_enter_over_the_fix_card`); opening the palette closes the card, as it
  closes every other floating thing. `contrast_badges` runs after `canvas_ui` in the same
  panel; called before it, the canvas takes the badge's press
  (`a_click_on_a_failing_badge_opens_the_card`).
- `seen_through` writes the simulation into its own `seen` texture, never into `target`. Simulating
  in place would save a texture and corrupt the next pan or patch, which copy kept pixels out of
  `target`, and the eyedropper, which reads it. No test pins it: headless, `seen_through` hands
  back the drawn texture. `shade_grid` writes `gridded` for the same reason.
- The *Panels ▸* flyout is its own `egui::Area`, so the dropdown's `contains_pointer` is false over
  it; only `dropdown`'s `on_flyout` test keeps a click in it from closing the View menu.
  `the_view_menu_s_panels_row_opens_the_panes_beside_it` pins it.
- Deriving the escape clip from overrides makes resized children pop out of their frame.
- The cheatsheet's chord both opens it (at its door) and closes it (in `cheatsheet_ui`), in one
  frame's input; `just_opened` is all that stops the opening press closing it.
  `the_chord_opens_the_sheet_and_closes_it` pins it.
- Find and replace's `Escape` rung sits below `self.text` in `OndinApp::escape`: above it, the press
  that should finish an edit opened from a row closes the box instead.
  `editing_a_match_selects_it_and_escape_closes_one_thing` pins it.
- The `return` after `present_ui` is what makes present mode view-only: without it the editor runs
  below and `Delete` deletes the selection nobody can see. `present_mode_steps_frames_and_edits_nothing`
  pins it.
- A new place that draws a name must take it from `crate::private` (or a `shown_*` helper), never
  `.name()` or `page_name`, which stay the real name for comparing, renaming and files. The guard is
  `private::leak`: a document whose every layer, page, component and file name carries a sentinel,
  whole frames drawn with each surface open, failing on any drawn string or viewport command holding
  it. Each surface carries its own control (`surface`): with private mode off, opening it must draw a
  name the app did not draw without it, so a surface that is not drawn at all (culled below the
  fold, a card folded shut) fails rather than passing unread. `no_surface_draws_a_name_in_private_mode`
  opens the editor with a layer, an instance, a main with a swap property and a variant set
  selected, the swap property's picker open, the palette empty and typed, find and replace over
  every page, a layer's
  menu with its *Move to page* list open, the variables browser, the Export card, the contrast check,
  the vision simulation, version history, the recovery and close cards and present mode;
  `no_kit_surface_draws_a_name_in_private_mode` an instance's kit card, the insert picker, *Kits
  used*, *Make local*, the kit update popover (its one kit's table open, components named as layers)
  and *Publish*; `the_library_draws_no_name_in_private_mode` the library with its delete, move and
  missing-file cards; and `contrast::tests::the_fix_card_masks_its_layer_in_private_mode` the
  contrast fix card, which needs that module's measured fixture. A surface they do not open is
  unguarded.
- `set_private` must not write `layers_folded` or `pages_folded`: the fold is an overlay so leaving
  restores the panes as they were. `the_chord_turns_it_on_and_the_pill_says_so` pins it.
- Fill with content's `plan` draws twice on purpose. Folding the redraws into the first pass looks
  simpler, but a redrawn record then takes different list entries and every later record, passing
  over them, shifts too. `a_draw_repeats_and_a_redraw_moves_only_its_record` pins it.
