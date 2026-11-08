# Tools and canvas interaction

The v1 tool set, the tool rail and how canvas gestures behave: the wheel, the canvas cursors, cancelling, context menus and their
chrome, selection chrome, multi-selection transforms, the Scale tool, skew, snapping during
resize, drop outlines, the transform origin (pivot), *Transform again*, *Repeat around origin* and
*Warp*.

## 9.4 v1 tools

- **Select** (click, shift-click, marquee as a box or a lasso, select-all) and **Hand** (pan).
- **Move and rotate** (`SetTransform`): Shift locks a move to one axis and a rotation to 15° steps.
- **Resize** by corners or sides (`SetGeometry`, §5.6): Shift locks the ratio, or releases it on a proportion-locked layer; Alt scales about the box centre. Group resize is recursive; a multi-selection gets one box for resize, rotate and skew.
- **Skew**: `Ctrl`-drag a side; Shift holds 15° steps. **Transform origin**: a draggable pivot marker with two numeric fields. **Scale** (`K`): the same gestures, but stroke widths, radii and type scale too.
- **Shapes**: frame, rect, ellipse, polygon, star, line, spiral, arc. **Text**: edit on the canvas with a real caret.
- **Pen** (`Path`), straight and curved. Corner↔smooth conversion over a point selection goes through `edit::set_smoothness`; a smooth pair is exactly mirrored, because that is what `is_smooth` means.
- **Pencil** (`Shift+P`, `Path`): a freehand trail fitted to curves on release (`app-pen-and-nodes.md`).
- **Spiral** (`Shift+L`) and **Arc** (no key) draw a `Spiral` and an `Arc` node (`core-model.md`) with the line's stroke and no fill. A spiral is dragged from its centre: the drag's length is the outer radius and its angle is where the spiral ends, and it starts at 3.5 turns. `edit::create_spiral` gives the node the true spiral's box at that radius, so the stretch that fills the box is uniform, and turns the node so the end lands on the drag. An arc is dragged from end to end, bulging to the left of the drag, and starts at 90°; `edit::create_arc` places it as `create_line` places a line, the sweep negated under a mirroring parent so the bulge stays where it was drawn. `↑`/`↓` step the turns by halves (½ to 20) or walk the arc's signed ladder (`edit::arc_sweep`): 15° steps up to 345° either way, and below 15° to 5°, then across to −5° and on down the other side, so one drag reaches both bulges. The pill beside the pointer gives the value (`tools::drawn_count_label`: `3.5 turns`, `90°`, `-5°`), placed by `spacing::paint_label` as every pointer readout is. Shift steps the direction by 45° as Line does; Alt mirrors an arc's chord through the press, and a spiral ignores it. An arc is its own kind rather than an ellipse with its sweep set, because an ellipse's stroke follows the wedge's radii too.
- **Slice** (`Shift+F`, `Slice`): a box dragged out over the page to export the region under it (below).
- **Shape builder** (`Shift+M`): drag across the faces overlapping shapes cut each other into, open paths and lines cutting them further, to merge them, Alt-drag to remove them (`app-booleans.md`).
- **Knife** (no key; `C` is held for comments): drag a line through closed shapes to cut each into the pieces it parts (`app-booleans.md`, *The knife*).
- **Group/ungroup** and **frame selection** (`build::frame`, a new frame sized to the selection's union; a second builder beside `group`, not a flag on it).
- **Align** to each other, to the container (one layer selected), or to a **key layer** (`Alt`+`Shift`+click). **Distribute** (equal gaps, 3+ layers) and a **named gap** (2+ layers, packed outwards from the key). **Tidy up** (2+ layers, `Alt+Shift+T`) lays the selection out as the grid it is roughly in (`build::tidy_moves`):
  - **Rows and columns** are clusters of box centres. A box joins a band while its centre is within half the band's largest box, or half its own when it is larger, of the band's mean centre.
  - **Cells** are kept, holes included, when no cell holds two boxes and some row spans every column. Otherwise each row is refilled from the left, which is what happens to bricks.
  - **Sizes**: a column is as wide as its widest box and a row as tall as its tallest, and each box is centred in its cell.
  - **Gaps** are the median of the current gaps between neighbouring bands, never negative; an axis with no gaps takes the other's. They are measured band to band rather than box to box, so a box inset in its cell does not read as a wider gap next time — except across refilled rows, whose columns are not bands the user made: there the gap across is read box to box within each row.
  - **Stability**: the union's top-left holds still, and tidying twice moves nothing. `tidy_suite.rs` checks these properties over hand cases and seeded random layouts.
  - Once the gaps are even, the selection shows **spacing handles** on the canvas: drag a gap band to set every gap of its axis, or a layer's centre dot to reorder it (`layout.md`, *Spacing handles on a loose selection*).
- **Flip** H/V across the selection's centreline, or about a single layer's pivot where one is placed; **quarter turns** either way. **Z-order** one step or all the way (`Ctrl`+`[`/`]`).
- **Transform again** (the last move, rotation, scale or skew once more) and **Repeat around origin…** (copies turned about the pivot), below.
- **Warp…**: an envelope round the selection, dragged by its corners or bent by an *Arc*, *Flag* or *Bulge* preset, baked into plain paths (below).
- **Snapping**: alignment, equal-gap (gaps quoted on the canvas), baselines, the pixel grid. **Guides and view**: rulers and guides (`Shift`+`R`), View and Snap menus, focus mode.
- **Eyedropper** (`I`, or a picker's pick button): picks a colour off any pixel in the window, chrome included, onto the selection or into one picker slot (below).
- **Paint editing**: stacked fills and strokes (§6.3), solid or linear, radial or angular gradient, in a detached picker with the gradient's handles on the canvas; stroke options, per-corner radius and corner smoothing, polygon sides, star points and ratio, a polygon's or star's radius, an ellipse's start, sweep and ratio, a spiral's turns, an arc's sweep, opacity, artboard background. Bulk editing covers a multi-selection's transform, opacity, radius, fill and stroke list, Group Colors.
- **Clipboard** (cut/copy/paste, duplicate via `InsertSubtree`; Alt-drag drags out a copy), the **layers panel** (drag reorder/reparent of the whole selection, rename, `Shift`/`Ctrl` selection, visibility, lock, filter), nudge and the `Ctrl`+arrow size step, pan/zoom, and save/open with unsaved-change protection.

### The tool rail (`rail.rs`)

`OndinApp::tool_rail` is a 46pt left panel: the Select slot, Hand, Scale · the Frame slot, four
shapes and the Line slot · the Pen slot, Text, Place image · the Builder slot and the eyedropper,
which act on what is drawn and are used less; at its foot the styles-and-variables
and insert-picker buttons. The node tool and image editing have no slot: each is entered on a
subject. The four filled shape tools keep a slot each while the rail has room for them.

- **Grouped slots.** Five slots stand for several tools each (`rail::Group::members`): Select is
  Pointer and Lasso, Frame is Frame and Slice, Line is Line, Spiral and Arc, Pen is Pen and
  Pencil, Builder is Shape builder and Knife. A member that cannot be armed now (the Shape
  builder with nothing to build, `rail_usable`) draws dimmed, in the slot and the row, and a
  click on it says why instead of arming; the slot still opens its row. A slot shows the armed tool when it is a member, else the member last picked
  (`rail_shown`), and carries a small `text::dim()` triangle in its bottom-right corner
  (`paint_more_mark`).
- **Memory.** The Select slot's memory is the lasso switch itself (`app-selection.md`); Pointer
  and Lasso arm Select with the switch off and on. The others remember through `Rail::slice`,
  `Rail::line`, `Rail::pencil` and `Rail::knife`, set in `choose_tool` (`remember_rail_pick`)
  however the tool was armed — slot, letter or palette — and kept in `prefs.view` (`rail_slice`,
  `rail_line`, `rail_pencil`, `rail_knife`). The pen bias inside a node edit shows Pen.
- **Gestures** (`rail_group`, `rail_flyout`). A click arms what the slot shows. A press held
  `HOLD_S` (0.45 s) or a right-click opens the members as a row of icon buttons beside the slot
  (`ui::flyout_place`, a `ui::menu_frame` card, area `"rail-flyout"`), the shown member lit, each
  with its name and key as a tooltip. A click in the row picks. At the end of the hold that opened
  it, letting go over a member picks it (Photoshop's press-drag-release), and while the press is
  dragged the member under the pointer is lit instead; letting go on the slot leaves the row open
  and picks nothing. Escape, or a press anywhere but the row and its slot, closes it. The slot
  shows no tooltip while pressed or while its row is open.
- **The hold is 0.45 s**, the long-press convention (Photoshop, macOS): a longer wait reads as
  broken, and the right-click and press-drag-release are the fast ways in.

### The wheel

`canvas::wheel_input` runs while the canvas is hovered (or the pointer is over the origin fields or
the image-edit *Done* pill, whose `Area`s it takes the wheel back from). It reads raw `MouseWheel`
events, not egui's smoothed deltas (§15 D26).

| Input | Effect |
| --- | --- |
| scroll | pan vertically |
| `Shift`+scroll | pan horizontally |
| `Ctrl`/`Cmd`+scroll, `Alt`+scroll | zoom at the cursor (`Alt` is Adobe's spelling) |

Settings' *Zoom with the scroll wheel* (`CanvasPrefs::wheel_zooms`, off by default) swaps the plain
and `Ctrl`/`Alt` meanings for a mouse wheel only (`canvas::wheel_travel`): a notch zooms at the
cursor and `Ctrl`/`Alt`+notch pans vertically; `Shift`+notch still pans sideways, and a touchpad's
scroll and pinch keep the table's meanings. Only the canvas passes the preference; the minimap and
present mode do not.

- **A mouse is told by its delta, not its unit** (`canvas::is_mouse_wheel`): a page delta, or a line
  delta of whole lines. On Windows winit reports every wheel message as lines, a precision touchpad's
  scroll and pinch included, but a touchpad's come in fractions of a line and a notch is one.
  Points are never a mouse. Limit: a free-spinning wheel's fractional lines read as a touchpad and
  pan. `the_wheel_zoom_preference_swaps_a_mouse_wheel_only` pins it.

### Canvas cursors (`cursor.rs`)

Over the canvas the app draws every cursor itself, the arrow included, so each says what a press
there would do. Panels, menus and window edges keep the system cursor. A caller answers a
`cursor::Pointer`, `(CursorIcon, Option<Bitmap>)`: the bitmap is handed to egui as an OS cursor
image, and the `CursorIcon` set beside it is the stock fallback when the bitmap cannot be drawn.

- **A base and at most one badge.** `cursor::Bitmap` names the base (`Arrow`, `Node`, `Pen`,
  `Pencil`, turned `Resize`/`Skew`/`Rotate`, `Crosshair`, `FromCentre`, `Loaded`, `IBeam`,
  `TextOnPath`, `Eyedropper`, `Guide`/`CopyGuide`/`Gap` by axis, `Discard`, `Pan`, `Panning`);
  `cursor::Badge` the badge. One badge means one thing on every base. `Duplicate` is a second
  arrow of the same size 2 pt right and 3 pt up, behind the first; the pointer aims from the
  front arrow's tip. The node arrow is the
  select arrow with its inks swapped, so the two tools read apart.
- **Drawn, not taken from the icon font.** Each drawing is SVG path data on a box 32 units square
  (`Glyph::parts`), flattened once and rasterized by supersampling: a rim pass in the set's ink
  (the shape widened by a 1.5 pt mitred outline, mitre limit 8, square caps on strokes), then the
  core in its fill. Layers paint in order, each with its own soft shadow (blur 0.7 pt, drop
  0.6 pt, 32%) drawn into the bitmap, so the duplicate's front arrow's rim and shadow fall on the
  copy. A drawing aimed from its
  middle is nudged onto the texel grid so its one-unit lines stay crisp; one aimed from a tip has
  the tip on a texel corner.
- **A unit is a point, or 0.75 pt for the pointing drawings** (the arrows, pen, pencil and
  dropper, `POINTING_UNIT_PT`): at 0.75 the arrow, outline included, is the OS's own size
  (Windows' is 12 × 19 texels at 100%), and the resize arrows at a point per unit already match
  the OS's. The rim and shadow are in points, so they keep their weight at either unit.
- **Two ink sets**, matching each OS's own pointer: the light set (white core, dark rim) on
  Windows, the dark set on macOS and Linux. The inks are `#121316` and white, the cursor's own and
  the same under either theme, so not theme colours.
- **Size**: 32 units × the unit's points × pointer scale × texels per point. The pointer scale is the OS pointer size
  over its default: `CursorBaseSize ÷ 32` from `HKCU\Control Panel\Cursors` on Windows,
  `XCURSOR_SIZE ÷ 24` on Linux, 1 on macOS, which scales app cursors itself. It is re-read every
  2 s, so a settings change shows without a restart. macOS gets one texel per point
  (`texels_per_point`): winit sizes the cursor's `NSImage` in points equal to its pixel count, so
  a 2× bitmap would show at twice the size; on a Retina display the OS upscales a 1× bitmap.
- **Turned cursors** face the handle's screen direction in 15° steps (`cursor::Turn`). Resize
  and skew keep twelve steps, since a double-headed drawing turned 180° is the same drawing.

| Where | Cursor |
| --- | --- |
| Select at rest | arrow; `Shift` → Plus; `Alt` over a layer → Duplicate, over empty canvas → Minus; `Ctrl`/`Cmd` over a layer → Deep; over a locked selected layer → No |
| Scale tool at rest | arrow with Scale, replaced by a modifier's badge |
| Shape builder | arrow with Plus; `Alt` → Minus |
| Knife | the crosshair, as the pencil's |
| A move | the rest arrow; Duplicate once the Alt copy exists |
| A marquee | the mode fixed at its press: Toggle → Plus, Subtract → Minus |
| Pivot, and its drag | arrow with Pivot |
| Line ends, rail handle, shape dots, gradient handles, and their drags | arrow with Dot |
| A loose run's centre dot, and a reorder | arrow with Swap |
| A loose run's gap band | `Gap` along its axis |
| Box, point-box, crop and spacing handles | `Resize` at the handle's angle |
| `Ctrl` on a side | `Skew` along the side |
| Rotate band (box and point box), and the turn | `Rotate`, facing out from the corner; while turning, from the pivot towards the pointer |
| Rulers, guide lines, guide drags | `Guide`; `Alt` while dragging an existing guide → `CopyGuide`; back over its ruler → `Discard` |
| Create tools | `Crosshair`; `Alt` → `FromCentre` (the Text tool keeps the plain crosshair) |
| Image tool, loaded | `Loaded`, the crosshair with the count left |
| Pen | Close on the first anchor, Join on another open end, Continue on an open end with no path in flight, Convert while `Alt` pulls the handle |
| Node tool | node arrow; Anchor, Segment or Dot over those; `Ctrl` on an anchor or `Alt` on a handle → Convert; `Ctrl` on a segment → Bend; `Shift` on an anchor, a segment or empty canvas → Plus (a handle ignores it); `Alt` over empty canvas → Minus. Mid-drag the drag's own: Anchor, Segment (`Ctrl` → Bend) or Dot (`Alt` → Convert), as those keys still act |
| Node tool, pen armed | pen with Plus (insert) on a segment, Continue, Star (new subpath) |
| Pencil over the selected open path's end | pencil with Continue |
| Text tool over text, and in a text edit | `IBeam`; over an outline → `TextOnPath` |
| Eyedropper armed | the loupe (`CursorIcon::None`); `Eyedropper` off the canvas until the frozen frame lands |
| Space, Hand tool, inside a crop | `Pan`; while dragging, or the middle button → `Panning` |
| A kit tile dragged over the canvas | arrow with Plus |

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

- **One registry, seven targets.** `menu::Item` is one enum of verbs; `Item::spec()` gives each its label, glyph, accelerator and `Group`. `menu::build` sorts requested rows into the canonical group order, so a menu function chooses *which* rows appear, never which group they go in. Emitted order within a group is kept: a `Group` buys a separator, not a position. `build` returns `Vec<Vec<Row>>`, so an empty group takes its separator with it.
- **Promotion.** `Row::promote()` is the one exception: a verb a kind lifts into its head is moved, not duplicated (*Ungroup* on a group, *Flatten* on a boolean).
- **Submenus.** A row with children (`Row::sub: Vec<Vec<Row>>`) is a submenu, its groups written by the menu function rather than bucketed. Its parent `Item` runs nothing (`perform_menu_item` no-ops `CopyPasteAs`, `BooleanMenu`, `AlignMenu`, `Arrange`, `MoveToPage`); `context_menu_ui` draws the one open submenu beside its row (`context-menus.md` §8).
- **Rows map to actions.** Most rows hand to an `input::Action`, so a row and its chord cannot mean different things. Rows with no chord (*Paste here*, *Add point here*, key-layer and base-operand designations, guide verbs, or anything `shortcuts.md` does not bind) call their canvas verb directly.
- **Exhaustive matches.** `Item::spec`'s match and `menu::ViewState::get` are exhaustive. A row's chord is its action's row in `input::BINDINGS`, spelled from there; `Row::checked` takes an `Option` so a caller with no answer can say so.
- `every_view_switch_is_on_exactly_one_menu` checks membership, not cardinality, across the hand-written `VIEW_CANVAS`, `VIEW_MODES`, `VIEW_PANELS` and `SNAP_MENU`; `Lasso` must be on none. The canvas menu's view rows derive from `VIEW_CANVAS` and `VIEW_MODES` minus `context-menus.md` §6.1's excludes.

**A region opens only the menu it claimed** (R2). Four responses read `secondary_clicked()`: the
canvas, a layers row, the layers panel background and a Pages row, which opens the page menu
(`Target::Page`; `picker::stop_bar` keeps its own right-click for removing a gradient stop). The
canvas resolves five of the seven targets, in order:

1. a live text session;
2. a point selection;
3. a guide (`guide_at` answers `None` while guides are locked, so the canvas menu carries *Lock guides* itself);
4. `pick_at_pointer` → `pick_for_click`, the query a left-click runs, including `Ctrl` deep-select and a page frame's or main's name tag;
5. the empty canvas.

Between the second and the third, the edited gradient's handles keep the button and open no menu
(`gradient_secondary`, the stop bar's twin): a right-click on a stop's dot removes it, and one on any
handle or dot is spent either way.

A hit inside the selection leaves it whole (right-click one of five layers and still reach
*Union*); a hit outside replaces it. Empty canvas does not deselect, because *Paste here* is why
that menu exists.

**Opening the next floating thing closes the last** (R4). `OndinApp::context_menu` is the one slot;
opening it clears `open_menu`, `picker`, `stroke_menu`, `type_menu`, `effect_menu`, `export_menu` and
`export_row_open`. The image-editing card is left out: it is a mode, not a popover. Three popover
lists exist:

| List | Members |
| --- | --- |
| `a_popover_is_open` (the lane test) | the inspector's popovers (`inspector_popover_open`), the Repeat and Blend cards, the image card |
| R4's clear | the inspector's popovers, plus the dropdown slot and the picker |
| `a_popover_owns_escape` | the inspector's popovers and the contrast and simulation bars; not the image card, because `Escape` is how image editing is left, nor the Repeat and Blend cards, which are rungs of the ladder |

- `ContextMenu::just_opened` makes the opening frame skip the dismissal test, or the opening release would dismiss it.
- A click that dismisses a menu is spent on the dismissal: the canvas skips its mode input and a layers row its click. The library screen's ⋮ popup and project-filter dropdown follow the rule (§9.5).

**Escape and menus** (R3): an open menu is the first thing Escape unwinds, and the press is consumed.
`OndinApp::frame_body` handles the key and returns without calling `escape`;
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

- **One row type.** `ui::MenuRow` serves top-bar dropdowns and context menus (accelerator column, checked state, destructive tint, dimmed-but-hoverable state); `menu_item` is the same row with the options off. Both allocate `ui::MENU_ROW_H` (24). Dimmed rows sense `hover()`, not `click()`, so the tooltip saying why can show.
- **Separators.** `ui::menu_sep`: a nominal 1pt `text_a(20)` rule with `SEP_AIR` (3) either side, `ui::MENU_SEP_H` (7); the rule is `round(ppp).max(1.0)` device pixels for crispness.
- **Placement.** `ui::menu_place` flips, rounds, then clamps (last, because rounding a clamped value can push it out by a fraction of a point) against `content_rect()`. Height is computed from the row and separator constants, not measured, because placement precedes the `Area`. `ui::dropdown_place` (a list anchored on a field) puts the left edge on the field's (right if that overflows), above the field when there is no room below, then `menu_place` clamps.
- **Border and width.** A computed height includes `ui::MENU_BORDER` on every side (a `Frame` stroke grows its outer rect). `ui::menu_inner_w(card, pad)` subtracts padding and border before `set_width`: context menu 210, top-bar menus 150 and 160, `app::POPOVER_W` 272.
- **Red.** `theme::color::danger()` (`#e0736b` dark, `#c83c34` light) appears only on *Delete*, distinct from amber `warn()`. The save dot uses `color::unsaved()`, the same value under its own name.

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
- **Hover** outlines exactly what a click would select, `Ctrl` and `Ctrl`+`Alt` included; a frame or a group is traced by its box (the selection box's, hidden children included), since tracing every leaf it holds made a busy ring of a large group. The ring is 3px, against the selection box's 1.2px.
- `canvas::stroke_outlines` takes a `view` (`canvas::outline_view`, the viewport inflated by the ring width) and rejects subtrees missing it, copying `scene.rs`'s `bounds_cover_subtree`: a group or clipping frame can be rejected whole, a non-clipping frame cannot. Visibility is read through `display_node`, so a layers-panel eye preview counts.
- **Inspector edits hide the chrome** (box, handles, badge, pivot, hover outline, key-layer trace). `canvas::chrome_hidden` holds the list: `app::ChromeHold` is one term, beside `editing_in_place` (pen, path editing, image editing, gradient handles, an open warp envelope). A scrub's 1.5s count starts when the button comes up and each keystroke restarts it; the chrome returns on the timeout, on pointer movement over the canvas, or at once on `cancel_gesture`. An edit arms the hide if its transaction `changes_ink` (§5.7): a 90° turn does; the proportion lock and opening the per-corner dialog do not. Box trim moves the box without moving ink, so it commits through `commit_chrome_edit` and keeps its chrome. `app::edit_note` also requires that the control did something.
- Every inspector control commits through `commit_edit` or `edit_valve` (§9.3). `only_the_layers_panel_commits_without_the_committer` requires `self.session.commit` only in `panels/layers.rs`; the identity row's *Group*/*Ungroup* call the keyboard verbs declared in `app.rs`.

**A lone `Line` gets two end handles instead of a box.** `canvas::lone_line` is `Some` only when the
selection is exactly one `Line`; `canvas::line_ends` places the ends.

- Dragging an end (`Drag::LineEnd { id, which }`, `edit::move_line_end`) moves that point. Shift constrains to eighths of a turn about the other end (`axial_corner`); snapping applies to the unshifted case, on both axes. The two ends behave asymmetrically, because of the model (§15 D99).
- `canvas::selection_handles` returns `None` on `lone_line` (not on `line_ends`, so an unresolvable transform cannot fall through to the box), suppressing corners, side bands, skew, the rotate ring and their cursors. `draw_size_badge` returns early.
- The panel shows X/Y as the start point, rotation as the start→end angle, and `L` (length) across the full row where W/H would be. An `Arc` shows the same fields, `L` being its chord, but keeps the selection box: `lone_line` answers only for a `Line`. Dragged
  by a handle of that box, an arc alone always scales evenly (`edit::resize_geometry`), as if Shift
  were held: a circle cannot stretch, and the box holds its bulge, so only an even scale keeps the
  held edge still and brings the dragged one to the pointer. Inside a group or a multi-selection
  an uneven resize scales its two ends and keeps its sweep.

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
- **Resize** runs on the oriented box. `canvas::selection_resize_box` is the committed-tree twin of `selection_frame_box`; `edit::resize_selection` takes `edit::SelectionBox { frame, extent }` and works in frame space (`frame · S · frame⁻¹ · world`); `canvas::resize_axes` maps the handle through the same frame. `Handles` stores no extent; everything recomputes it from the committed tree, because scaling against a previewed box compounds every frame. `snapped_resize` excludes the whole selection from the snap targets.
- **Typed doors** (`OndinApp::size_step`, `inspector::scale_tx`, the multi W/H fields) keep the upright union, which the panel reports.
- **Target.** `preview::Target { Layer(NodeId), Selection }` travels on `Drag::Resize`, `Drag::Rotate`, `Drag::Skew` and `Handles.target`; `resize_tx`, `rotate_tx`, `skew_tx`, `keep_ratio`, `snapped_resize` and `resize_axes` dispatch on it, so a drag cannot change target midway.
- **Style.** 1px `SELECT_DIM`, against each member's 1.2px `SELECT` outline; `COMPONENT_DIM` box and `COMPONENT` handles when every member is a main or instance (§5.3d).
- **Rotation** turns the arrangement about the union centre (`edit::rotate_selection`, `rotate_node` per member). §15 D50 covers the pivot, the proportion lock over a set, and where `build::outermost` applies.

**Resize mechanism.** A transform never carries scale (§5.6), so `edit::resize_selection` works like
`resize_group` with world space for the shared parent: each root is carried through the scale
(`scaled_about`), projected into its parent, then split by `split_geometry_scale` into the transform
it keeps and the scale its geometry takes. Shared pieces: `resize_factors` (factors and anchor,
including the `symmetric` and `keep_ratio` rules) and `scale_geometry` (a leaf's size, a line's end,
a path's points, or recursion into a container); `resize_group`, `scale_subtree` and
`resize_geometry` (§5.6) are built from these. An off-axis root under a non-uniform drag gets a skew
so it lands exactly. Options travel as `edit::Resize { keep_ratio, symmetric, scaling }`. A line's
or path's gradients ride the same scale in `scale_geometry` (`carry_gradients`); box-sized kinds are
carried at the commit door (§6.2).

**Overflow guards** refuse per node: the member stays put and the rest move.

- `scaled_about` returns `Option<Affine>`; a product that overflows to `inf` is skipped.
- `split_geometry_scale` returns `Option<(Affine, f64, f64)>`, refusing a finite basis that cannot be written as `orientation × scale`, such as a rank-1 matrix (`build::Basis::exact`, §5.7a). All six call sites refuse per node: a missing edit is recoverable, a wrong one is not.

### Gradient handles (`gradient.rs`)

**The open picker is the mode**; there is no gradient tool. `OndinApp::edited_gradient` answers while
a selecting tool is up, the picker shows a gradient face (not *Variables*) for a `Fill(i)` or
`Stroke(i)`, and that layer is the whole selection. Then `editing_in_place` is true, so the selection
chrome hides, and the box handles, a lone line's ends and the rail handle do not arm
(`selection_handles`, `line_ends`, `grab_rail_handle` answer `None`).

| Kind | Handles | Guide |
| --- | --- | --- |
| Linear | the two ends | the line |
| Radial | centre; first axis (sets the radius and turns the ellipse; Shift keeps it round); second axis (sets only the other radius) | the first axis, and the outline traced |
| Angular | centre; start, on a ring of `RING_PX` (50) screen px | the ring |

- **Stops between the ends are dots on the guide**, the stop bar's stops (`Picker::stop` is shared).
  A click on a handle or dot selects its stop; a click on the bare guide adds a sampled stop there and
  selects it (`gradient_click`, ahead of the set's `+` and the pick). A right-click on a dot removes
  it while more than two remain (`gradient_secondary`, after the point-edit claimant in
  `canvas_context_menu`).
- **The press** is the rung after the pivot in `begin_select_drag` (`begin_gradient_drag`), and
  `chrome_claims` asks `gradient_claims`. `Drag::Gradient { id, slot, grab, from }` re-derives from
  the brush at the press each frame, previews through `preview_slot` and commits once through
  `write_slot`: one undo step.
- **Snapping**: ends and centres snap to the box's nine points within `SNAP_PX` (6 screen px). Shift
  drops the snap and instead holds a linear end to eighths of a turn about the other end, and a sweep's
  start to 45°.
- Edits go through the picker's slot verbs on the brush as drawn (§9.3), so a gradient layout stretches
  is edited on the box it is drawn on. The geometry is `paint::linear_ends`/`with_linear_ends`,
  `radial_axes`/`with_radial_axes`, `sweep_geom`/`with_sweep_geom`/`sweep_dir`, `guide_point` and
  `offset_at`, all in the shape's space.
- A readout pill (the spacing handles' `paint_label`) shows angle and length, the radii (and turn), or
  the start angle. Cursor: the arrow with a Dot badge over a handle or dot and through its drag, the
  plain arrow over the bare guide, where the ghost dot is the affordance. A locked layer draws the handles and arms none
  of them.

### Shape handles (`shape_handles.rs`)

Under Select, one selected rect or ellipse that is unlocked and shown draws dots while the pointer is
over it or a dot is dragged. A frame has none: its radius is rarely set, and its Appearance fields
are enough. They hide while its shorter side is under `MIN_SIDE_PX` (48
screen pt), during any other drag, while the chrome is hidden and while a gradient is edited. Dots are
worked in the layer's local space, so a rotated layer's dots turn with it. The press takes them after the
pivot, the gradient handles and a set's `+`, and ahead of the box handles they may sit on
(`begin_shape_drag`; `chrome_claims` asks `shape_handle_at`). `Drag::ShapeHandle` previews each frame
and commits once; the value shows beside the pointer.

| Kind | Dots | Drag | Undo step |
| --- | --- | --- | --- |
| Rect | one inside each corner, inset by its radius along both edges, or `ZERO_INSET_PX` (12 screen pt) at radius 0 to clear the resize handle | all four corners to one radius, whole points, 0 to half the shorter side ("Radius 32"); Alt sets that corner alone and opens the per-corner disclosure ("Top left 48") | *Corner radius* |
| Ellipse | start and end on the outline; ratio on the inner edge at mid-sweep (the centre while there is no hole) | end sets Sweep; start moves Start with the end held; ratio sets Ratio in whole percent. Shift snaps an angle to 15°; Sweep stops at 0 and 360 rather than wrapping | *Start*, *Sweep*, *Ratio* |

- Smoothing does not move a corner dot. A drag unbinds what it writes (all four corners, or with Alt the one) by a `SetCornerVars` in its own transaction, so the commit's binding keeper leaves it alone.
- A whole ellipse's start and end dots coincide; the end is listed first, so the press drags Sweep.
- Polygons and stars have no dots.

### The Scale tool

`Tool::Scale` (`K`, glyph `resize`, rail slot three beside Select and Hand) is Select with a
different verb on the handles.

- **Shared gates.** Click, marquee, move, rotate ring and pivot behave as under Select. `Tool::selects()` is the shared gate (click, drag start, hover outline, pivot marker, `rulers.rs` guide lines); the cursor shares one match arm.
- Only `Tool::scaling()` differs; `canvas::resize_opts` passes it into `edit::Resize`.
- **Proportional by default.** `canvas::keep_ratio` treats the tool as locked, so Shift releases it. Released, scalar properties take the geometric mean of the two factors.
- **Cursor.** At rest the arrow wears the Scale badge (`canvas::rest_cursor`), the tool's only warning that the gestures differ; a modifier's badge replaces it, and over a handle the resize arrows win.
- **Numeric door.** The Transform card's Scale % row appears only while the tool is held (`inspector.md`).

**Adding a `Tool` variant is found by the compiler; adding a `NodeKind` is not.** Every predicate
over `Tool` is an exhaustive `match` (the cursor, `create_tx`), a `matches!` allow-list
(`drags_a_box`, `selects`), or a wildcard with the right default (`scaling` → `Scaling::Geometry`).
`NodeKind`'s `matches!` lists answer a missing kind with a confident "no" (§15 D87–D88).

**Arming a create tool clears the selection** (`OndinApp::choose_tool`). The set is `drags_a_box`:
Frame, Slice, Rect, Ellipse, Polygon, Star, Line, Spiral, Arc, Text, Image. Left out: Select, Scale and Shape builder (operate on the
selection), Hand (a pan keeps it), Node and ImageEdit (entered *from* a selected layer), Pen (places
anchors, never drags a box), Pencil (a stroke can carry on the selected path), Eyedropper (picks onto the selection). `place_image_action` clears it too.

**A tool switch abandons the drag in flight.** `choose_tool` clears `self.drag` and `self.alt_clone`
and calls `clear_gesture_preview`. It does not call `clear_preview` (which would discard a live text
session's content) nor `cancel_gesture` (which needs an `&egui::Context` and covers states a tool
switch cannot be in). Unlike `cancel_gesture`'s `InFlight::any`, its three fields are a hand-kept list.

### The Slice tool (`slice.rs`)

`Tool::Slice` (`Shift+F`, glyph `knife`, the Frame slot's second member, crosshair cursor) cuts slices
(§5.3), the boxes that export a region of their page (§7).

- **A drag cuts one** through the ordinary create gesture (`app-create-gestures.md`): one *Add slice*
  step, landing in the frame under the press. `edit::create_slice` rounds both corners to whole world
  units whether or not the pixel grid is on, since a raster cut on half a pixel blurs its border, and
  gives the slice one export spec, PNG at 1×, so *Export all* takes it at once.
- **The tool stays on** after a cut, the new slice selected, so slices are cut in runs. A click
  without a drag makes nothing (`click_makes` answers `None`). A drag begun on the selection's
  handles resizes it, as with Select, rather than cutting another.
- **Picking.** With the tool on, a click inside a slice picks it (`OndinApp::slice_at`: the topmost
  shown, unlocked slice whose box holds the point); elsewhere it does nothing. With any other tool a
  click inside passes through to what lies under it, and the slice is picked by its edge band
  (`PICK_SLOP_PX`, §5.10) or its layers row; that band drags it. Once selected, a press anywhere inside
  it moves it (`OndinApp::selected_slice_at`, ahead of the art under it in `begin_select_drag`) and its
  handles resize it. The marquee and the other bulk doors skip slices (`app-selection.md`).
- **Chrome** (`OndinApp::draw_slices`, after the hover outline). A slice draws nothing in a render,
  so the canvas draws a dashed 1 pt outline (4 on, 3 off) round every shown slice on the page, at its
  live size (the display node, so a resize or a W/H scrub shows as it happens): in `color::SELECT`
  when it is hovered (Select's hover target, or with the Slice tool the slice under the pointer) or
  selected, at 45% otherwise, so what is already cut shows.
  *Shown* here and in `slice_at` is `Document::shown_through`: a slice in a hidden group is hidden.
- **Slice selection** (the layer menu's Structure group, and the palette) cuts a slice fitted to the
  selection instead of dragging one (`OndinApp::slice_selection` → `build::slice_selection`): the
  selection's world bounds rounded out to whole units, just above the topmost selected layer in their
  nearest common container that is neither a boolean (it would be an operand) nor turned or skewed
  (its box would be the box of a turned box), a lone layer's name, one PNG 1× spec, selected afterwards. It dims, and
  does nothing, on a selection of slices only.

### The eyedropper (`eyedropper.rs`)

**It samples what is seen**, anywhere in the window, from two sources:

- **Over the canvas** (where egui sees the pointer): the rendered canvas texture's pixel under the
  hotspot, composited over the canvas ground and opaque, so a translucent layer gives the blended
  colour. egui paints the overlays (selection, handles, guides, rulers, pixel grid) over that
  texture, so they are never sampled. Read live, so a pan or zoom made while armed is followed.
  Pixel preview yields while a dropper is armed, so the texture is at device pixels.
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

- **A pick snaps to a document colour.** The sampled pixel becomes the nearest opaque colour the
  document already uses within Colour review's *Tight* limit (OKLab Δ ≤ 1, whatever the card is set
  to; `OndinApp::dropper_snap`, over `color_review::census` of every page, taken once per
  revision): antialiasing and blends leave pixels a hair off the colour meant. A colour bound to a
  variable wins a tie, and on the rail door the pick then **binds** the top paint
  (`build::bind_top_paint_all`, keeping its `%`) where the variable's scope takes the field and it
  draws the snapped colour in every target's modes, else copies it (the census values a variable
  at its default mode); a picker's slot takes the colour. `Ctrl` held
  takes the raw pixel (`PendingPick::raw`); `Alt` and `Shift` keep their meanings, so `Ctrl` was free.
- **One click, one transaction, one undo step.** A click never changes the selection.
- **No canvas preview.** Hovering writes nothing and installs no `RenderOverrides`: a preview would change the pixels the next sample reads. From a picker, the hovered colour (canvas sample or frozen pixel) shows in its plane, hue and value row (`Picker::preview`): a dim *was XXXXXX* beside a hex, or the channel fields tinted accent with the old colour as a chip in the format chooser. That preview is not in the frozen frame: picking over the picker reads it as captured.
- **The readback** (`CanvasRenderer::request_sample`): after the render, the `LOUPE_SIDE`² (11×11) device-pixel region round the hotspot is copied out of the canvas texture into a mapped buffer, without waiting, and collected a frame later. A `SampleKey` (hotspot, session revision, camera centre and zoom, canvas pixel size, `pick`) that matches the last sample is not read again, which keeps a still pointer from repainting forever. Headless, `cpu_region` draws the same region with `vello_cpu` and it lands at once; the tests use that arm.
- **A pick always reads afresh.** Uncommitted changes (typing in a text session, a preview, a decode landing, the previewed ground) move pixels without moving the revision, so a key can match pixels that have changed. Hover keys carry `pick` 0; each click mints its own number (`OndinApp::dropper_picks`, in `dropper_input`), so its key matches no earlier sample and the canvas is read again. A stale loupe is only a look; a stale pick is a wrong write. The accepted cost: under a still pointer the hover loupe can stay stale until the pointer moves. Pinned by `a_pick_reads_afresh_when_pixels_moved_under_a_still_pointer`.
- **Pick timing.** A click records a `PendingPick` whose `at` is a `PickAt`: `Canvas(key)` on the canvas, `Window(pos)` elsewhere. `OndinApp::dropper_frame` (run in `draw_canvas` after the render) lands a canvas pick when a sample with that key arrives, a window pick once the frozen frame is there. A canvas pick deferred behind an in-flight hover readback is copied off a later frame's render, so a pick whose camera or canvas size no longer matches is re-aimed at the pixel now showing the world point it was clicked on, or dropped when that point has left the canvas (`OndinApp::reaim_pick`).
- **The frozen frame** (`Frozen`: `None` → `Asked(pass, screen)` → `Ready(image, screen)`). Armed and `None`, `dropper_frame` sends one `ViewportCommand::Screenshot` tagged `FrozenShot(pass)`, and that frame draws no loupe, because eframe captures the frame the command is sent in. `gate_dropper_input` takes the `Event::Screenshot` only when its tag matches the pending ask, so a capture from an earlier arming is ignored. While `Asked`, `dropper_frame` keeps asking for frames for 30 passes, not forever: a surface format egui cannot capture (only Rgba8Unorm and Bgra8Unorm are) never answers. It is asked again after a pick that leaves the dropper armed (`land_pick` resets it, since the panels now show the write), and when the window's size in points or its pixels-per-point differ from the frozen frame's. Disarming drops it. `window_region` reads the `LOUPE_SIDE`² device-pixel region round `floor(p * ppp)`.
- **Input.** In `normal_mode_input` the dropper takes every press an armed pan does not (Space, Hand and middle-drag still pan), ahead of guides, handles and tools: they are chrome its sample never sees. It also routes in `TextInsert`, so a picker opened over a text session can pick. While a dropper is armed the canvas does not answer as Select (`OndinApp::selecting`): a picker's pick button leaves the tool on Select, and without this the hover outline, spacing bands and guide grabs kept answering.
- **The rest of the window is inert while armed** (`OndinApp::gate_dropper_input`, from `raw_input_hook`), which is what keeps the frozen frame true. egui gets the pointer only over the canvas; the picking picker too is pickable, not clickable. Elsewhere egui is told the pointer is gone and gets no press or wheel, so no panel hovers, takes a click or scrolls; `dropper_pointer` keeps the position for the loupe. A press there is the dropper's (`OndinApp::dropper_press`) and its release acts: primary records a `PickAt::Window` pick at the press position (`Alt` stroke, `Shift` stay), secondary disarms. A press egui already has finishes untouched, so a pan dragged off the canvas still ends; disarming hands the pointer back where it is. The canvas is told apart from the panels by `canvas_rect`, since all of them share egui's background layer; any area above that layer is chrome.
- **Ways out.** `Escape` is one rung (`disarm_dropper`, `shortcuts.md` §11): it disarms a picker's button and leaves the picker open, or hands the rail tool back. A right-click anywhere disarms and spends the click (above). A second `I` is `choose_tool`'s toggle arm. Leaving the editor for the library disarms (`go_to_dashboard`), since the gate runs on every screen and the library has neither a canvas nor the escape ladder. The picker's own pick button is not one: a picking picker takes no click.
- **The loupe is the cursor** everywhere (`CursorIcon::None`): it draws the region at 8pt a pixel, centred on the hotspot, the centre pixel ringed, with a chip below (above near the window's bottom) showing what a click would apply: the hex, or after a snap the document colour with the magnet glyph, named by its variable when bound; with `Ctrl` held, the raw pixel's hex. Its edge is two rims, dark outside light, under a soft shadow, so one of the pair contrasts with any ground. Off the canvas, until the frozen frame lands, the eyedropper cursor (`cursor::Bitmap::Eyedropper`, aimed from its tip like the pen's) stands in, with no chip.
- **On the canvas the picker's preview is the latest sample**, not the one for the pixel under the pointer: each pixel of movement starts a readback that lands a frame later, and waiting for the matching one put the slot's own colour back for that frame.

### Skew

`Ctrl`-dragging a side skews: the dragged side follows the pointer along its own length, the
opposite side stays put (`canvas::skew_tx` → `edit::skew_to_handle` / `edit::skew_selection`).

- **Sides only.** Corners keep resize, with the rotate ring beyond.
- **Modifier.** `Handles::armed` takes a `skewing` flag and re-reads the side bands, so one statement of handle priority (D14) decides cursor, gesture and hover outline. Ctrl is read at the press.
- **Measured from the press.** `Drag::Skew` carries the press point, so the grabbed point follows the pointer and a second skew starts from zero.
- **No pointer snapping** (the pointer stands for an angle); Shift snaps to 15°. **Clamped** to ±89° (`edit::MAX_SKEW`); a wild drag pins at the limit.
- **Single node, shape or group:** the lean is in the node's own frame on its local box, no recursion. **Multi-selection:** a world-axis lean about the shared upright union; the selection frame is drawn from the same shear (`edit::side_shear`).
- **Cursor** points along the side (`Handles::screen_tangent`, the outward normal turned a quarter turn in the box's frame). §15 D52.

### Resize snapping and Alt

- **Snapped axes.** A resize snaps only the axes its handle travels along. `canvas::resize_axes` maps the handle's box-local outward direction through the box's world transform: a corner claims both axes, an upright side one, a rotated side both. The point box uses the same function.
- **Alt** keeps the box centre fixed: `edit::resized_box` and `resize_group` take `symmetric` next to `keep_ratio`; Shift and Alt compose. The centre is a fixed pivot, not the node's transform origin: a fixed pivot and a handle tracking the pointer cannot both be honoured (§15 D60), so `resized_box` and `resize_factors` take no pivot.
- **Alt+Shift.** Under `symmetric` the origin is centred on both axes, including the one the handle does not own (matters when `keep_ratio` drives the second axis, as with a side handle). Both copies of the rule, `resized_box`'s `edge` and `resize_factors`' `pin`, answer the centre before the anchor.
- **Past the anchor** the shape flips: both copies keep the sign of the scale factor.

### Drop outlines during a move

- **Frame drop.** `canvas::draw_frame_drop_outline` boxes any frame (never the root) that would adopt the selection on release: 3px `color::SELECT`, above the hover outline, below the selection chrome. The decision is `OndinApp::move_destination` (split out of `move_tx`), asked by outline and commit with the same snapped delta; for an in-flow item it reads the pointer from `Drag::Move`'s anchor, so `finish_drag` restores the drag around the transaction it builds.
- **Flex landing.** A flex item's move outlines its landing slot, dashed 1.5px `color::SELECT` (`canvas::draw_flex_landing`), from `RenderOverrides::landings` (§6.2) filtered to the item `flex_reorder_of` names. None for an item leaving its frame or a multi-selection. A grid item's move outlines the target cell and draws its container's track lines from `build::drop_grid`. When the reorder moves the container itself, the landing is composed through the container's previewed world transform.
- **Memoised frames.** The artboard list the drop rule uses is memoised (`OndinApp::with_frames` → `FrameIndex`), keyed on `EditorSession::revision`, which is drawn from one counter every session shares (`session::next_revision`, so a key never names another tab's document) and bumped by `commit_inner`, `undo`, `redo` and `adopt_document` and nothing else: nothing may add, remove or move an artboard without one of those. A gesture in flight does not bump it, because the drop rule is defined against committed bounds. The same walk lists the page's tagged layers and slices, and `OndinApp::instances_of` fills every main's instances (`component::instances_by_main`) on the first ask at a revision, so nothing the canvas or the component cards ask every frame walks the document.

### The transform origin (pivot)

**The toggle.** `OndinApp::show_pivot` is app-wide and sticky (someone rigging pivots will place
several): a `ui::HeadAction` toggle on the Transform header, dimmed with a tooltip for a
multi-selection. Escape turns it off, after `cancel_gesture` and before the `entered_group` unwind,
without undoing placed pivots.

**`canvas::draw_pivot` states:**

| State | When | Glyph |
| --- | --- | --- |
| Editable | toggle on, one layer selected | `icon::CROSSHAIR` (`CROSSHAIR_SIMPLE` with a filled centre) |
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
field (composes about it); quarter turns; *Transform again*; *Repeat around origin* over one layer;
`build::flip`, only when a pivot has been placed (D55). Canvas skew and Alt-resize deliberately do
not, each having its own fixed point (D55, D60).

### Transform again (`again.rs`)

A palette verb (`Item::TransformAgain`, group Transform, `icon::TRANSFORM_AGAIN`, no chord:
`shortcuts.md`, *Deliberately unbound*). It applies the last move, rotation, scale or skew once more
to every outermost selected layer, each about its own pivot (`pivot_world`); a move is the same world
delta. Each application is one undo step, *Transform again*; the status says *Rotated 15° again*
(*Moved*, *Scaled*, *Skewed*).

- **Learnt at the commit, not at the doors.** `Session::commit_inner` starts an `again::Watch` when
  the step's name is a transform's (`Move`, `Nudge`, `Rotate`, `Rotate 90° …`, `Scale`, `Skew`):
  the first subject's world transform, and for a scale its size along its own axes (`again::own_size`:
  the local box times its transform's scale), before the commit, compared after. The layers panel's drop is named *Reorder* or *Move layer*,
  not *Move*, so the shift a layout gives a dropped layer teaches nothing. So the canvas drags, the arrow keys, the Transform card's fields and quarter turns, the
  Scale row and the Scale tool's handle drags (named *Scale*, not *Resize*) teach it without each
  door saying so. A step named otherwise teaches nothing: `Resize` (a handle drag under any other
  tool, W/H, `Ctrl`+arrows) and *Transform again* itself, so repeating leaves the step what it was.
- **What is kept** is `Session::last_transform`, per document, in memory, across selection changes:
  a move's world delta; a rotation's angle; a scale as the ratio of the layer's own size, only when
  both its sides grew alike, at any turn (a stretch has no one factor and keeps the old step); a skew as the world-space
  lean, named by how far the layer's own lean moved. A commit that changed nothing measurable keeps
  the old step.
- **Applied** through `edit::rotate_node` (rotation), `edit::transform_node` (the skew's lean about
  the pivot) and `edit::resize_selection` with `Scaling::Photographic` about the pivot (strokes,
  radii and type scale too).
- **The palette row names the step**: *Transform again: Rotate 15°* (*Move 24, 0*, *Scale 120%*,
  *Skew 10°*). With nothing learnt it is dim with *Move, rotate or scale something first*; with
  nothing selected, *Select a layer first*.

### Repeat around origin (`repeat.rs`, `ondin_core::repeat`)

Copies of the selection turned step by step about one point, committed grouped with the originals.

- **The ring** (`repeat::Ring { count, step, turn }`). The count includes the original (12 is a clock
  face), from 2 to `repeat::MAX_COUNT`. `Ring::full` divides a full turn by the count. With *turn*
  off the copies keep their orientation and travel round: the selection box's centre is what the
  rotation carries.
- **The point.** One layer turns about its pivot (`pivot_world`). Several layers have no shared
  origin, so they turn as one arrangement about their box's centre (`repeat::centre`).
- **The result** (`Place::Group`, through `build::group`, so the layers must share a parent): a group
  named *‹top layer› × ‹count›*, the originals first in the layers list and the copies below them in
  order round the ring. Copies are captured subtrees settled with `component::settle_copy`, so a
  main's copies land as its instances. The group is selected; the status says *Made 11 copies around
  the origin*. One undo step, *Repeat*.
- **Refusals** (`repeat_refusal`): nothing selected (*Select a layer first*), *This layer is
  locked*, *A group containing this layer is locked*, *Repeat takes layers that share one parent*,
  and *Repeat can't work inside an auto layout* (`repeat::in_layout`, which core's `repeat` and
  `linked` refuse too): in a flex or grid parent the ring's group, or the preview's copies, would
  be laid out as items, moving the original and its siblings, as the shape builder and the knife
  refuse such a layer. The same holds through plain groups or booleans between the layer and such
  a parent (`container::lays_out_above`): the ring would grow the group the layout places. The card closes when the selection comes to be refused.
- **Doors.** The palette (`Item::RepeatAround`, group Transform, `icon::REPEAT_AROUND`, unbound) and
  the Transform card's fifth button (`inspector.md`), On while the card is up. A layer menu carries
  no Transform rows, so the card is the door.

**The card** is an inspector-lane popover: an `Order::Foreground` `Area` from
`OndinApp::transform_popover_area`, placed by `popover_anchor` level with the top of the inspector's
Transform card (`OndinApp::transform_card`, recorded by `panel_full` each frame; the inspector
column's top when no Transform card is drawn), on `ui::menu_frame(ui::POPOVER_PAD)` at `POPOVER_W`,
rows at `ui::POPOVER_SECTION_GAP`. It is no modal and has no click-away: the canvas stays live,
because the pivot decides the ring and must stay draggable. It holds the lane alone: opening it
closes the inspector's popovers (`close_inspector_popovers`) and the Blend shapes card
(`app-booleans.md`), and an inspector popover opened after it closes it (`inspector_popover_open`).
Find and Colour review are left open. If the pivot handle is down and one layer is selected, opening
raises it (`show_pivot`); closing or committing puts it back down only if the card raised it
(`RepeatCard::raised_pivot`).

- **Rows.** Head `ui::popover_head` *Repeat around origin*; *Full turn · Fixed angle*; *Count*
  (`icon::COPY_SIMPLE`) beside *Step* (`icon::ROTATION`), the step disabled showing 360/count under
  *Full turn* and editable under *Fixed angle*, which starts from the full turn's step; the switch
  *Turn the copies with the step*; the switch *Link the copies* with a caption under it at
  `ui::SECTION_LABEL_GAP`, always two rows tall and cut with an ellipsis past them, so the card keeps
  its height as the switch and the selection change the words; foot `settings::footer_row`, *Cancel* then primary *Repeat*. Its settings
  outlive a close, so it reopens on the ring last made.
- **Keys.** `Enter` repeats (`dispatch` hands `Action::Enter` to the card while it is open); `Escape`
  cancels, the rung after the Credits card. The card closes when another document comes to the front.
- **A right-click cancels a scrub** of *Count* or *Step* wherever the pointer is: `InFlight::repeat_scrub`
  holds both as they were, armed while a field is dragged, put back by `cancel_gesture` and cleared at
  the frame's end once no button is down, as `settings_scrub` is.
- **The preview** is `Place::Beside`: the copies in each original's own parent at 45% of its opacity,
  since a preview must not reparent what is on the canvas. It is minted from a copy of the id
  counter, so it spends no ids; rebuilt when the revision, selection, pivot or ring changes; and put
  back after another gesture's preview cleared it. A canvas drag owns the preview while it lasts.
- **Linked** makes the original a component first — in place for a frame or group, else wrapped in a
  group named after the layer and made the main (not *Create component*'s frame: the wrapper only
  carries the link, and a frame would add a clip and a fill), so the ring group reads *‹layer› ×
  ‹count›* — then repeats that main, so the copies are its instances (`repeat::linked`, the
  `repeat` verb's `linked`). Both halves are built on a scratch copy of the document and commit as
  one *Repeat* step; the status says *Made Tick a
  component and 11 linked copies around the origin*. Off is the default, and the choice is kept per
  document in memory (`Session::repeat_linked`). Where it cannot apply the switch is disabled, its
  reason both tooltip and caption: several layers, off, *Linked copies repeat one layer at a time*;
  a main or an instance root, on, *Copies of a component are linked already*; otherwise off, with
  *Create component*'s reason (`repeat::why_not_linked` decides which).

### Warp (`warp.rs`, `ondin_core::warp`)

An envelope round the selection's world box, bent on the canvas and baked into plain paths. Nothing
stays live. The box (`warp::bounds`) is the union of the outlines that are warped, not of the
layers' bounds: a boolean's box is its result's, smaller than the operands the warp carries, and a
point outside the box would be extrapolated past a perspective map's horizon or round an arc's
centre.

- **Envelopes** (`warp::Envelope`). *Free distort* (`Free([Point; 4])`) is a perspective map of the
  box onto four dragged corners, so straight lines stay straight. The presets (`Preset { preset,
  bend, vertical }`, `bend` in `-1..=1`, along the width or, when `vertical`, the height):

  | Preset | Map |
  | --- | --- |
  | Arc | the box bent round a circle; the centreline keeps its length and its ends stay level, the top of the bend rises. It folds once the circle's radius falls to half the box's height across the bend, so `max_bend` is min(1, 2w/(πh)) |
  | Flag | one wave along the box, amplitude bend·h/2, up at the first quarter and down at the third |
  | Bulge | y·(1 + bend·(1 − 4u²)) about the centre; a negative bend pinches |

- **Refit.** Each outline is carried through the map in world space and refitted with cubics whose
  handles are the original handles carried by the map's derivative, halved until the fit is within
  `warp::TOLERANCE` (0.05 world units) of the true image, or of the coordinates' own rounding
  where the box lies so far out that it is coarser (16 ε times its distance from the origin), so a
  far-off selection never halves every curve to the depth cap. Under *Free distort* a line stays a
  line.
- **Subjects** (`warp::subjects`) open groups and booleans down to their leaves; a slice draws
  nothing and is left as it is. Refusals
  (`Refusal::says`), which both the dimmed row and a refused commit give: *Select layers to warp*
  (nothing selected, or nothing with an outline), *Pictures can't be warped; trace it first*,
  *Frames can't be warped; warp the layers inside*, *A layer to warp is locked* (anywhere under the
  selection, since a locked child would be rewritten too), *Warp can't work inside a component or an
  instance*.
- **The commit** (`warp::warp`): a path keeps its node and takes the warped outline (`SetGeometry`,
  radii baked, mirror cleared); any other leaf, shapes and text (outlined) alike, is replaced through
  `build::replace_with_path` (`app-booleans.md`), keeping its look, name and place. A replaced
  selected layer is replaced in the selection by its path. One undo step, *Warp* (`icon::WARP`); the
  status says *Warped*. An envelope left square commits nothing.
- **The preview** (`warp::preview`) cannot be the commit, since a render preview refuses
  `DeleteNode`: a path takes its warped outline, and every other shown leaf is hidden
  (`SetVisible false`) under a stand-in path directly above it, wearing its look
  (`build::carried_look`) and, for a mask, its mask flag and mode (`build::carried_mask`); a hidden
  leaf gets none. Above, not beneath: a hidden mask ends the run under it, so a stand-in mask below
  it would mask nothing. The render preview takes a stand-in's mask flag and mode onto its ghost,
  which masks the run above it as the node will. Stand-ins are minted from a copy of the id counter, so the preview spends
  no ids. It is rebuilt when the envelope changes and put back when another preview cleared it.

**The session** (`WarpSession` on `OndinApp::warp`).

- **Doors.** The layer menu's Structure row *Warp…* (`Item::Warp`, glyph `WARP`, `context-menus.md`
  §4) and the palette (keywords *distort*, *envelope*, *arc*,
  *flag*, *bulge*, *bend*, *perspective*). No chord. Opening arms Select if a non-selecting tool was
  armed, and closes the inspector's popovers and the Repeat and Blend cards.
- **The canvas.** The session owns every press an armed pan does not (`normal_mode_input`), so the
  selection cannot change under it; `editing_in_place` hides the selection chrome. `canvas::draw_warp`
  draws the outline in `SELECT`; mesh lines at thirds in `SELECT_DIM` while a handle is held or the
  pointer is inside; under *Free distort* the four corners, under a preset one handle at the warped
  top of the bend (the left edge when vertical, the flag's first crest). Every handle is a round dot
  in the shape dots' look (`shape_handles::DOT_R`), and the one in the hand inverts as a dragged
  shape dot does; the envelope is pulled by points, not by the selection's resize squares. While
  the bend handle is dragged a *Bend 40%* pill sits beside the pointer (`spacing::paint_label`).
- **Cursor** (`canvas_cursor`). While the envelope is open the canvas shows the arrow, and over a
  handle or through its drag `handle_cursor`'s dot arrow, never the selection box's resize or rotate
  cursors: the session swallows every press but its handles', so those would promise a drag.
- **Dragging.** A corner lands on the pixel grid when it is on; Shift keeps it on the line of its
  nearer edge instead. A corner that would make the quad non-convex stops, saying *That corner would
  fold the shape*. The bend handle and the card's field stop at `max_bend`, saying *That much bend
  folds the shape*. Picking *Arc* over text says *Put the text on a path to keep it editable*. A
  right-click mid-drag puts the corner or bend back (`cancel_warp_grab`, from `cancel_gesture`), and
  a right-click mid-scrub of the card's *Bend* restores it (`InFlight::warp_scrub`).
- **Status.** On opening *Warping: drag a corner, or pick a shape on the bar; Enter to finish,
  Escape to cancel*; on picking a preset *Warping: drag the handle to bend it; Enter to finish,
  Escape to cancel*.
- **The warp bar** takes the path bar's place (`path_bar::warp_bar`, `app-pen-and-nodes.md`): *Free
  distort*, *Arc*, *Flag* and *Bulge* as one choice (`WARP_FREE`, `WARP_ARC`, `WARP_FLAG`,
  `WARP_BULGE`); a rule; *Reset the envelope* (`ARROW_COUNTER_CLOCKWISE`, dimmed *The envelope is
  already square*); a rule; *Done* (`Enter`). A preset hangs a card beside the bar as *Offset* does
  (`warp_card`): one label, *Bend*, as the Offset card has one (the lit bar button names the
  preset), over a value field in percent (−100 to 100) wearing the preset's glyph, and *Horizontal ·
  Vertical* as `ui::segmented`. The presets share one bend, held to each
  one's range as they are switched.
- **Ways out.** `Enter` commits (`Action::Enter`). `Escape` cancels, its own rung after the Blend
  card's. Choosing any tool cancels (`choose_tool`). A selection change or a document edit under it
  closes it (`tend_warp`, every frame). Its standing preview is excused in `cancel_gesture`, as the
  Repeat and Blend cards' are, so a right-click still opens a menu.

### `Ctrl`+arrow size step

`Ctrl`+arrows step the selection's box by the nudge step (`input::NudgeStep`): 1, or 10 with
`Shift`. `→` wider, `←` narrower, `↓` taller, `↑` shorter (`shortcuts.md` §11).

- `OndinApp::size_step` uses `edit::resize_box_to` for one layer (grows along its own axes) and `edit::resize_selection` over the upright world union for several.
- The top-left corner is held, so the step does not `reveal_selection`. One arrow table serves nudge and resize; `normal_mode` picks the verb from the modifier.
- 1pt floor per axis. With every member proportion-locked, both axes move and the arrow names the driver (`edit::paired_size`). Guides are inert.
- Bursts coalesce under the undo verb `resize-layers`, separate from nudges.

## Traps

- **A step's name is what teaches *Transform again*.** `again::kind_of` matches the undo-step names the doors pass to `name_step`; renaming one (`Move`, `Rotate`, `Scale`, …) silently stops that door teaching it. `transform_steps_are_recognised_by_name` pins the matcher, not the doors' names.
- **Hidden is not disarmed.** `editing_in_place` hides the selection chrome but arms nothing off; `selection_handles`, `line_ends`, `grab_rail_handle` and `on_pivot` each refuse under `edited_gradient`, or a hidden handle takes the press from the gradient's (the pivot sits on a new radial's centre). `the_box_handles_step_aside` and `a_hidden_pivot_does_not_take_a_centre_s_press` pin the box handles and the pivot.
- **The hold's release is not a pick.** egui reports the release that ends a hold as the slot's click; `rail_group`'s `ends_hold` guard keeps it from arming the shown member and closing the row. The row finds the member under a held drag from its own geometry, since the press belongs to the slot and no button in the row reads as hovered. `a_held_press_opens_the_row_and_a_release_over_a_member_picks_it` pins the guard, and its release over the lasso exercises the geometry.
- **A warp's preview is not its commit.** Previewing `warp::warp`'s transaction would show nothing, since a render preview with a `DeleteNode` is dropped whole; `warp::preview` hides each replaced leaf under a stand-in instead. `a_warp_bakes_a_group_into_paths` pins that the preview deletes nothing.
- **Same press, two consumers.** The cancel and the context menu share one press: `gesture_cancelled` must be read at the top of `canvas_context_menu`, before any branch selects.
- **Dropdown keyboard focus.** A dropdown opened by clicking its head leaves `focused() == None`, so `egui_wants_keyboard_input` does not cover it.
- **Popover flags outlive popovers.** Gate on `popover_heard`, not on the flags, or Escape is swallowed behind a popover nobody is drawing.
- **Three popover lists.** They differ by the image card and by the Repeat and Blend cards. A shared list would have to carry those exceptions.
- **Inert `TopMenu` row.** `TopMenu` needs its own Escape arm; without one, Escape closes the menu *and* drops the tool.
- **Two copies of one resize rule.** `resized_box` and `resize_factors` have diverged twice. Change both, and test both: the box can be identical while the path differs.
- **Unguarded `draw_drop_indicator`.** It does not check `shown_visible`, so a picture inside a hidden group is still ringed during a file drag.
- **Accelerators by hand for the chords read in code.** A table chord's label is spelled from its `input::BINDINGS` row, but the clipboard chords, `Ctrl+Alt+X`, `F2`, `Del`/`Backspace`, centre-align, the text session's Alt+arrows and the host-dependent chords are spelled by hand in `Item::spec`/`Item::alt_accel`; only the cheatsheet's chord test holds those to `input::resolve`.
- **Clear without `clear_preview`.** `choose_tool` must call `clear_gesture_preview`, not `clear_preview`, or a live text session's content is lost.
- **Predicate negatives.** `drags_a_box` decides whether arming a tool deletes the selection. When a predicate gains a second consumer, re-check its negative members.
- **Middle-button pan over the origin fields.** It is still swallowed by their `Area` (unverified).
- **Menu placement order.** `menu_place` must clamp after rounding; reversing the order cuts off a menu's last column.
- **Cursor bitmaps are cached and their angles quantized on purpose.** egui dedupes cursor uploads by `Arc` pointer identity, so a buffer drawn afresh each frame, or a raw angle in place of `cursor::Turn`, uploads a new cursor every frame of a drag. The cache is keyed by cursor and cleared when the scale changes (`the_bitmaps_are_reused_until_the_scale_changes`).
- **macOS cursors are one texel per point.** Raising `texels_per_point` to the display's pixels per point on macOS doubles the cursor on Retina; `the_bitmap_is_sized_for_the_display` pins it, on a macOS build only.
- **The eyedropper's repaint guard is unpinned.** `request_sample`'s GPU arm skipping a key that matches the last sample is all that stops a still, armed pointer repainting every frame; headless the CPU arm answers `false` whatever the key, so `the_hover_sample_is_keyed_to_the_pointer` does not catch its removal.
- **The frozen frame's three guards.** The capture frame draws no loupe (`the_captured_frame_has_no_loupe`), a capture is taken only on its own `FrozenShot` tag (`off_the_canvas_a_pick_reads_the_frozen_window`), and `dropper_frame` stops asking for frames 30 passes after an unanswered ask, or a surface egui cannot capture repaints forever (unpinned).
