# Rulers and guides

The two ruler bars, the guides pulled out of them (document state), how guides are picked, moved,
scoped, copied, drawn and inspected, and the canvas's press-priority ladder. Snapping to guides is in
`app-snapping.md`. Part of §9.4.

## 9.4 Rulers

- Two 20pt bars, view state (`OndinApp::show_rulers`, on by default, `Shift+R`, *View ▸ Show
  rulers*). Every reader asks `OndinApp::rulers_on()` (`show_rulers` alone; focus mode keeps the
  bars), so hit test and paint agree.
- **Selection band**: a tinted `ACCENT_900` band spans the selection's world extent on each bar's
  axis, its two end coordinates in `ACCENT_200`, read through the preview layer. A tick number under a
  band number is dropped (`EXTENT_LABEL_CLEAR`). Band numbers use the tick fallback — plain, then
  thousands form (`1.1k`), then nothing. Clamped to the bar; gone once none of it is on screen.
  `extent_band` is a free function.
- **A single selected frame re-origins the rulers** to its own top-left (lattice anchored there, so
  the band reads `0` to `W`). Only a lone frame — not a shape inside one, since the inspector reads
  X/Y in world space. The origin is not draggable.
- **Pointer tracking**: a full-thickness mark on both bars at the hover position; suppressed only
  during a guide drag (whose chip already shows the number).

## Guides

- Saved with the document (§5.5); **drawn as egui chrome, not scene content** — one hairline at any
  zoom, above clipping frames, invisible to `ondin export`.
- Not to be confused with `snap.rs::Guide`, a transient one-frame line explaining a snap.

### Switches

| Switch | Chord | Predicate |
| --- | --- | --- |
| Show guides | `Ctrl+;` | `guides_on()` = `show_guides` (focus mode does **not** hide guides) |
| Lock guides | `Ctrl+Alt+;` | `guides_pickable()` = `guides_on() && !lock_guides` |

- `guide_lines()` is empty when guides are off (no snapping to invisible lines).
- **Lock is global**, not per-guide (a per-guide lock would hide its own unlock control). It is one
  early return in `guide_at`, which hover, cursor, press, drag and discard all use. The measure overlay
  uses `guide_under` (no lock check): what may be measured is what may be snapped to.
- Arrow-key nudge (`rulers::nudge_guides`) and `Delete` (`inspector::remove_selected_guides`) reach
  guides through the selection, so each asks `guides_on()` itself; Delete says so in the status line.
  Hiding guides does not clear the selection (`measurable_guide` relies on that).
- New guides can still be pulled out while locked and land locked, but **the release must not select
  one**; `set_guides_locked` drops held guides via `Selection::clear_guides` (guide half only).
- Pulling a guide while guides are hidden turns *Show guides* on, **on the press**.

### Creating, moving, deleting (`finish_guide_drag`)

- Drag off a ruler creates; drag a guide moves it; drop back on **its own** ruler deletes; a new one
  released there is never created. A press that never moved (position **and** scope) commits nothing;
  `commit_inner` also drops a no-op `SetGuideScope`.
- **Scope comes from the release point**: inside a frame scopes the guide to the innermost frame
  (`rulers::frame_at_world`, via the frame's inverse world transform, so rotated frames count by their
  true rectangle); outside is global. The commit is one `SetGuideScope`.
- A frame-scoped guide's position field and ruler chip show the **offset from its frame** — the one
  exception to §9.3's world-space rule.
- **Latched at the press** (`GuideDrag::rulers_shown`): whether the bars exist. `update_guide_drag`
  spells both discard terms against that latch and the free `ruler_at`, so toggling rulers mid-drag
  cannot turn a cancel into a create.
- Shift+click adds to the guide selection (no move). `Ctrl+A` always selects layers.
- Arrows nudge each selected guide on its own axis only (inert on the other); one transaction per
  press, and a held burst coalesces through `commit_run` under the verb `nudge-guides`.
- Grab band `GUIDE_PICK_PT` = 6pt (= `canvas::EDGE_PICK_PX`). Drawing and picking use one resolved
  `GuideLine`; distance is to the **segment**, so a scoped guide is grabbable only where drawn.

### Clipboard, duplicate, Alt-copy

- `OndinApp::guide_clipboard: Option<Vec<Guide>>` beside the layer `clipboard`, mutually exclusive by
  hand (every copy clears the other). `property_clipboard` is a third, independent slot.
- `app::pasted_guide(from, id, owner, step)` is the one placement rule for paste and duplicate: fresh
  id, axis and colour carried, position stepped by `GUIDE_PASTE_OFFSET` (= `PASTE_OFFSET.x`), or no
  step for paste-in-place. Scope carries; a guide whose frame is gone falls back to the canvas with its
  number unchanged, and the message says how many. Both counts come from the ops built, not the
  selection.
- Copy also writes axis and number to the **system** clipboard — `Event::Paste` only fires when the OS
  clipboard holds something. `Ctrl+X` works on guides.
- `Ctrl+D` uses neither clipboard nor `CloneChain`; the copy becomes the selection, so repeats step
  evenly.
- **Alt+drag copies a guide** (`GuideDrag::copying`, live every frame). The draw loop draws the
  stationary original and `shown_guide` does not apply the drag override to it. A copy released on the
  ruler is discarded.
- Every route leaves the new guide unselected while guides are locked (`select_pasted_guides`).

### Drawing

| State | Ink |
| --- | --- |
| Resting | 1px, guide's own colour |
| Hover | same 1px + position chip + dotted extension |
| Selected | 2px `color::SELECT` (overrides a recoloured guide) + chip |

- A frame-scoped guide shows a dotted continuation beyond its frame on hover/selection, same width and
  colour; none for global guides or guides in rotated frames (`rulers::upright`). The solid segment is
  clipped by the frame, the dotted one by the viewport.
- **Discard cursor**: dragging a guide back onto its ruler shows `Bitmap::Discard` (Phosphor
  `trash-simple`; fallback `CursorIcon::NoDrop`), from `GuideDrag::announces_discard()` =
  `discarding && left_ruler`, shared with the fade. A guide that has not yet left the bars never
  announces discard. `ruler_cursor` returns `(CursorIcon, Option<Bitmap>)`.

### Inspector cards for guides

- **Guide** identity card: ruler icon, fixed name (*Guide* / *N Guides*), no eye, no lock.
- **Transform**: one full-width position field per axis present; each reads *Mixed* or a shared value.
- **Appearance**: colour plus a reset (dead at default) writing `color: None` so the guide follows the
  default again. Shared implementation with the canvas ground: `inspector::colour_panel` over a
  `ColourCard`, row `single_paint_row`; each caller builds its own reset transaction.
- *Remove* and *Remove all Guides* are 28×28 icon buttons in the card body, each one transaction.

## Press priority on the canvas

An armed pan, then **any selection chrome** (`OndinApp::chrome_claims`), then the ruler bar (claims a
press from every tool), then guide lines (Select tool only). A gesture already in flight keeps the
pointer.

- `chrome_claims(point)` walks the same rungs as `canvas::begin_select_drag`'s press ladder: pivot
  marker, line endpoints, rail handle, box handles, then a container's spacing band (`spacing_at`).
  It takes the point as a parameter: hover position here, `press_origin` there.
- A new rung in `begin_select_drag` owes `chrome_claims` a term; tests sweep strips of points to bind
  the two.
- Everything is decided on the press, from `press_origin`, not at drag-start.

### Spacing band drag (§5.3c draws the bands)

`Drag::Spacing { id, band, anchor, base }` — band grabbed, press in the container's local space, the
value the band **drew** there. The band may belong to an unselected container under the pointer.

- Its press sits ahead of the artwork's, so a band's `Alt+Shift` drag (all four sides) duplicates
  nothing. Modifiers choose sides and step, read every frame.
- Each frame previews one `SetDisplay`, led by a `SetSpacingVars` cutting touched bound slots whose
  value leaves their variable's number.
- Cursor: resize arrows along the band's axis, turned by the container's world transform.
- Right button or `Escape` cancels via `cancel_gesture`.
- Release commits via `session.commit`, skipped where every written slot already **draws** the value.
  A release that cuts a binding says *Detached ‹name›* (`OndinApp::band_drag_cut`, merged via
  `said_detached`).

## Traps

- Asking `rulers_on()` live during a guide drag: toggling rulers with the button down turns the cancel
  into a create. Read the press-time latch.
- Clearing the whole selection when locking guides deselects the user's layers; clear the guide half.
- `spacing_at` answers `None` while any drag is up; a test sweeping the ladder must reset `drag` first.
- The band release's no-op skip and the commit door's no-op test are two defences; flipping one alone
  stays green.
