# Canvas chrome, the top bar's menus and present mode

Overlay chrome drawn during gestures (escaping clip, create box, size pill, Alt-drag ghost), the
pixel grid, the top bar's View/Snap menus and Settings button, and present mode. Part of §9.4.

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
- Ink is neutral mid-grey at low alpha (works over white frames and dark grounds alike).
- Not drawn under the ruler bars.
- Every line lands via `rulers::snap_across_axis`, the guides' own snap.
- Showing the grid and snapping to it are separate switches.

## The top bar's right cluster

Between undo/redo and the zoom readout: **View** (eye + caret) and **Snap** (magnet + caret).

- Each head (zoom readout's too) is `ui::menu_head`: `HOVER` ground on its 26pt rect, radius 5, held
  while its own menu is open.
- Rows are `ui::menu_check`, `ui::MENU_ITEM_H` (24) tall, in `ui::menu_frame`; width 160pt or the
  widest row (`ui::menu_check_width`, 8pt inset both sides). The 13pt accent tick keeps its column when
  off; "off" also dims the label.
- **View**: *Show rulers*, *Show guides*, *Lock guides*, *Show grid*, *Show layout grid*, *Show layers*,
  *Show toolbar*, *Show status messages*, *Present mode*.
- **Snap**: *Snap to shapes*, *Snap to guides*, *Snap to grid*, *Snap to baselines* (last: no
  accelerator, not Illustrator's).
- Both render rows from `input::ViewSwitch`, the enum chords name, so label, order and state come from
  one place. `ViewSwitch::accel` is `None` for *Snap to baselines*, *Show layout grid*, *Show status
  messages*.
- All are **view state**, not persisted. All default on except *Lock guides* and present mode.

### Settings

- Rightmost: Phosphor `sliders`, 28pt, ten points clear of the document's controls. The update chip
  sits between it and that gap. On Windows/Linux the window caption buttons are to its right.
- Opens `settings.rs`'s modal: nudge step, *Show tree guides* and *Collapse groups on open* (under a
  **Layers** eyebrow), the web-font switch and cache row. `Ctrl+,` opens it (and only opens:
  `input::resolve` is not called while a modal is up). Staged; writes on *Save changes*.
- Not a `TopMenu` value. Panel-owned preferences stay on their panel (export habits in the export
  popover, written on the spot).
- There is no *Share* button (local file editor; nothing to share).

### One open menu

`app::TopMenu` holds one value — `None | View | Snap | Zoom | Document | KitUpdates` — so menus are
mutually exclusive by construction. `OndinApp::dropdown` spells the closing half once (click outside,
Escape). `OndinApp::context_menu` is the same one-slot shape; opening a context menu clears this field
and the floating popovers.

## Present mode

- Hides exactly: top bar, layers pane, tool rail, inspector, picker, ruler bars. Everything edited
  *with* stays — guides, layout grids, selection box, handles, size badge, hover outline, frame tags —
  and the canvas works normally. The **floating status bar stays** (it carries *"Present mode — Escape
  to leave"*).
- On Windows/Linux the top bar is the title bar, so caption buttons go too; `chrome::present_strip`
  shows them in a 32pt strip when the pointer reaches the window's top edge with no button held, keeps
  it while hovered or a button is down. Approaching from the canvas below shows nothing.
- It **overrides** the switches it reads (rulers, layers, toolbar) without changing them, so leaving
  restores the user's workspace. The canvas menu opens in present mode with *Show rulers* dimmed
  (*"Present mode hides the rulers"*); the other View rows stay live.
- Escape is handled **first** in `OndinApp::escape`. A context menu open in present mode takes the
  first Escape (R3 returns before `escape`), the second leaves the mode. The popover arm asks for a
  popover heard last frame (present mode draws none). Kit dialogs are drawn outside the `!present`
  guard so they get the first Escape.
- **Every flag `modal_is_up` reads must be a card drawn whenever it is set**, or an undrawn card holds
  every key.

## Traps

- Folding `present` into some overlay's predicate: present mode does not hide overlays; read §15 D750
  first.
- A modal flag set behind a `!present` draw guard swallows all keys in present mode.
- Deriving the escape clip from overrides makes resized children pop out of their frame.
