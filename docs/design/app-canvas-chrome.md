# Canvas chrome, the top bar's menus, the command palette, the cheatsheet, focus and present mode

Overlay chrome drawn during gestures (escaping clip, create box, size pill, Alt-drag ghost), the
pixel grid, the minimap, the contrast check mode, the top bar's View/Snap menus and Settings button,
the command palette, the shortcut cheatsheet, focus mode and present mode. Part of §9.4.

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

## The minimap (`minimap.rs`)

*View ▸ Show minimap* (`ViewSwitch::Minimap`, `Ctrl+Shift+M`, `prefs.view.minimap`, off by default).
In the View menu and the palette, not the canvas menu: it is chrome-wide, like *Show layers*. The
design canvas's *Minimap* artboard is the mockup; these rules are the spec.

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
  The selection's bounds are outlined in `color::SELECT`; the viewport is an accent box.
- **The fit** is the page content ∪ the viewport, padded 4% of the larger side, so the viewport box
  never leaves the map; an empty page fits the viewport at 2.4× its size.
- **Input.** A press outside the box centres the camera there and carries on as a drag; a press
  inside picks the box up. The canvas follows the drag frame by frame; the fit is frozen at the
  press, so the map holds still under the pointer, and a dashed ghost marks where the box started.
  A right-click during the drag puts the camera back. The wheel, any axis or modifier, zooms around
  the world point under the pointer (`Camera::zoom_by` anchored at that point's canvas position,
  even off-screen). Double-click zooms to fit the page. Hovering a top-level frame outside the box
  shows its name as a tooltip, the only text the map shows. Cursor: grab over the box, grabbing
  while dragging.
- **None of it is an edit**: camera only, no history step, nothing in the file. egui gives a press
  on the card to the map's widget, so it never reaches the canvas and never deselects.
- **Placement in the frame.** `minimap_ui` runs before the canvas's `CentralPanel`, placed from last
  frame's `canvas_rect`, so a pan it makes reaches this frame's render. It is outside the `!focus`
  guard, so focus mode keeps it; present mode's early return hides it.

## The contrast check mode (`contrast.rs`)

*View ▸ Check contrast* (`Action::CheckContrast`, `menu::Item::CheckContrast`, `Group::View`, no
chord): a ticked row in the View menu, and a palette row reading *Stop checking contrast* while on.
It measures the shown text layers, and with *Non-text* small shapes, against what is drawn behind
them with `ondin_export::contrast::check_with`, the measurement `ondin check contrast` runs
(`export.md`). `OndinApp::contrast` is view state: `reset_transient_state` turns it off whenever a
document opens, it is never saved or exported, and present mode's own branch never draws it.

- **Scope.** The selection when the mode starts, the page when that is empty, fixed for the mode's
  life: selecting other layers afterwards does not move it. The bar names it in a chip whose ×
  resets it to the whole page. Switching pages starts over on the new page, page-wide. Subjects
  are in reading order: `present::reading_order` over the page's top-level layers, then within each.
- **Measuring** runs on a worker (`job::Job`) over a clone of the document, one job at a time.
  `contrast_tick`, before the canvas, compares the document with a snapshot whenever
  `session.revision()` moves. A layer has changed when its stored node differs or its world box
  moved; a subject is queued, and drawn stale, when it changed or its box overlaps the old or new
  box of a changed or removed layer. A change to variables, styles or the page ground, or of the
  level or *Non-text*, queues everything. The queue goes once the document has been still for
  `SETTLE` (0.15 s), so a run of nudges measures once. A layer queued again while its job is in
  flight stays stale when that answer lands.
- **Badges** (`contrast_badges`) keep one screen size, above the layer's top left; a badge that
  overlaps an earlier one moves down below it. They are painted in the canvas panel's own layer
  after `canvas_ui`, so every floating card covers them.

  | Mark | Badge |
  | --- | --- |
  | Fail | red ground, warning glyph, `2.3 : 1 · needs 4.5` (`Dark · ` before *needs* when the worst mode is named), and a red outline 3pt outside the layer: red never stands alone |
  | Pass | dark chip, tick, the bare score (`12.7`, `Lc 78`), ` · large` when only the text's size makes it pass; hidden with *Show passing* off |
  | Skipped | dashed chip, `Not measured · gradient fill` or `picture fill` |

  A stale badge dims to 45% with a spinning circle-notch in place of its glyph. During a drag (a
  gesture preview that is not the fix card's own) the badges of the selected layers and their
  descendants hide.
- **The bar** (`contrast_bar`), an `Area` at the canvas's top centre, 12pt down: circle-half glyph,
  *Contrast* or the scope chip, the level head (`AA ▾`), then *Measuring…*, *All N pass* or
  *N failing* with `k / N` once stepped and prev/next arrows, then × to end the mode. The count dims
  while anything is stale. The arrows step through failures in reading order, selecting each and
  moving the camera only when the layer is not wholly in view: centred if it fits, else `zoom_to`.
- **The level menu**, from the head: AA (4.5 / 3), AAA (7 / 4.5), APCA (Lc), then *Show passing*
  and *Non-text (3 : 1)*, kept in `prefs.contrast` (`ContrastPrefs`; AA, passing shown, text only).
- **The fix card** (`contrast_fix_card`, a 270pt `Area` beside the badge) opens on a click on a
  failing badge, which also selects the layer; the badge is a widget registered after the canvas's,
  so it wins the pointer. Header (layer name; *body text*, *large text* or *non-text*; the level),
  the worst sample's ink and backdrop as a split swatch with both hexes and the score in red, a row
  per mode when there are several, then *Nearest* (`contrast::nearest`; *No colour at this hue
  passes*), noting *Unbinds ‹var›* when the text draws in a bound fill that fails, then *Variables*
  (`contrast::passing_variables`, at most six; *No colour variable passes*). Hovering a row
  previews it (`session.set_preview`); a click, or Enter (the hovered row, else the first),
  applies it as one undo step, *Fix contrast*. Escape or a click away closes it. A remeasure redoes
  the rows, and closes the card when the layer now passes or is gone. The card and the level menu
  own Escape through `a_popover_owns_escape` and `popover_heard`.
- **Every fix passes in every mode.** A raw colour is the same in all modes, so a fix for one mode
  that breaks another is no fix: *Nearest* must pass every mode's worst backdrop, and *Variables*
  lists only variables that pass in every mode, nearest the ink first. When no lightness at the
  hue passes everywhere the card offers only variables, the case modes exist for.
- **What a fix edits** (`fix_transaction`): the runs and paints whose colour fails the finding,
  never the backdrop. A run's own colour is replaced in the spans; runs drawing in the fill change
  the top visible fill. A run's colour cannot be bound, so a variable goes through the fill: a
  failing run's own colour is cleared onto the fill and the fill bound, and passing runs that drew
  in the fill are given its old colour so they keep their look. On a shape the failing visible
  solid fills and strokes are replaced or bound.

## The top bar's right cluster

Between undo/redo and the zoom readout: **View** (eye + caret) and **Snap** (magnet + caret).

- Each head (zoom readout's too) is `ui::menu_head`: `HOVER` ground on its 26pt rect, radius 5, held
  while its own menu is open.
- Rows are `ui::menu_check`, `ui::MENU_ITEM_H` (24) tall, in `ui::menu_frame`; width 160pt or the
  widest row (`ui::menu_check_width`, 8pt inset both sides). The 13pt accent tick keeps its column when
  off; "off" also dims the label.
- **View**: *Show rulers*, *Show guides*, *Lock guides*, *Show grid*, *Show layout grid*, *Show
  layers*, *Show history*, *Show minimap*, *Show toolbar*, *Show messages*, *Outline mode*, *Focus mode*,
  *Check contrast* (a mode, ticked while on and never remembered), then *Present mode* — a verb,
  not a switch: a check row that never ticks, which closes the menu and dispatches `Action::Present`.
  *Show minimap* follows *Show history* so the two rows about the layers pane stay together.
- **Snap**: *Snap to shapes*, *Snap to guides* (ruler guides, and layout grids while shown), *Snap to
  grid*, *Snap to baselines* (last: no accelerator, not Illustrator's).
- **Zoom** (`zoom_control`): a text field on top, unfocused at open, holding the live zoom as its
  value; Enter applies `OndinApp::apply_typed_zoom` (`150`, `150%`, `37.5 %`; about the canvas
  centre, clamped like every zoom; anything else says *"Type a zoom percentage, like 150"*) and
  closes the menu — an untouched value applies nothing, since it reads the rounded zoom. Below it
  *Zoom in*, *Zoom out*, *Actual size*, *Zoom to fit*, *Fit selection*. `OndinApp::zoom_entry`
  holds the typed text while the field has focus, `None` otherwise.
- View and Snap render rows from `input::ViewSwitch`, the enum chords name, so label, order and state come from
  one place. `ViewSwitch::accel` is `None` for *Snap to baselines*, *Show layout grid*, *Outline mode*,
  *Show messages*.
- **Outline mode** draws the artwork as one-device-pixel hairlines of every path the walk would fill
  or stroke, with type in the same ink, in near-black or near-white by the page ground's luminance.
  It pushes no layer, so nothing is clipped, masked, faded or given an effect: the view exists to
  show what the artwork covers. It is `scene::OutlinePainter` wrapping the backend's painter, so the
  walk itself is unchanged, and `RenderOverrides::set_outline` carries the ink, installed every frame
  by `draw_canvas` (present mode clears it). Hit testing and selection are unchanged. The eyedropper
  samples the canvas as drawn, so in outline view it picks up the ink.
- The switches are **view state**, not document state. All but focus mode and outline view are kept in `prefs.view`
  (`prefs::ViewPrefs`), written once a frame when one has changed
  (`OndinApp::remember_view_switches`, which also catches side doors such as a guide pulled from a
  ruler turning guides on), and a launch opens on them. All default on except *Lock guides*,
  *Show history*, *Show minimap*, outline view and focus mode; a launch never opens in focus mode
  or outline view.

### Settings

- Rightmost: Phosphor `sliders`, 28pt, ten points clear of the document's controls. The update chip
  sits between it and that gap. On Windows/Linux the window caption buttons are to its right.
- Opens `settings.rs`'s modal: nudge step, *Show tree guides* and *Collapse groups on open* (under a
  **Layers** eyebrow), *Default font* (under **Text**: the typography panel's family dropdown,
  its glyph `WARN` with a tooltip when the family is `Missing`, §5.4a), the web-font switch and
  cache row. `Ctrl+,` opens it (and only opens:
  `input::resolve` is not called while a modal is up). Staged; writes on *Save changes*.
- Not a `TopMenu` value. Panel-owned preferences stay on their panel (export habits in the export
  popover, written on the spot).
- There is no *Share* button (local file editor; nothing to share).

### One open menu

`app::TopMenu` holds one value — `None | View | Snap | Zoom | Document | KitUpdates` — so menus are
mutually exclusive by construction. `OndinApp::dropdown` spells the closing half once (click outside,
Escape). `OndinApp::context_menu` is the same one-slot shape; opening a context menu clears this field
and the floating popovers.

## The command palette (`palette.rs`)

`Ctrl+K`, alias `Ctrl+/` (`shortcuts.md` §8), in the editor only: in the library `Ctrl+K` stays the
file search, and the library screen clears an open palette. `input::discovery_chord` reads the
chord in both `normal_mode` and `text_insert_mode`. With a modal up the chord does nothing, and a
modal that opens closes the palette. App actions only; typed syntax belongs to Command Mode
(`vm.md`). The design canvas's *Command Palette* artboard is the mockup; these rules are the spec.

- **One registry.** A row is a `menu::Item`, with the context menu's label, glyph, chord, tick and
  dim reason. The app verbs no context menu shows are `Item`s too (undo and redo, save, new and
  close document, the library, settings, *Export as …*, *Present*, *Check contrast*, tools, place image, zoom, align,
  distribute, insert, styles and variables, publish kit, the text chords, *Show ‹card›*, the palette
  and the cheatsheet), each in a `Group` whose `Group::label` is the row's side note.
  `Item::action` maps each to its chord's `Action`, which `perform_menu_item` dispatches, so row and
  chord are one verb; `Item::aliases` holds the extra words. `palette_commands` chooses which verbs
  are offered; their words live in the registry. The Document menu's rows (*Make this a kit*,
  *Kits used…*, *Stop sharing as kit…*, *Show in library*) are registry items the menu draws its
  labels from, offered always and dimmed with a reason where the menu would leave them out (their
  `Action`s re-ask the menu's guards); the Transform card's quarter turns are
  `Item::RotateQuarter`, dimmed without a selection. *Select same …* (`Item::SelectSame`) dims with
  `select_same_source`'s reason and *Select inverse* without a selection (`app-selection.md`).
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
  with nothing selected, aliases *code*, *html*, *react*, *vue*, *zip*. *Show ‹card›*: the cards the
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
- **Placement and keys.** An `egui::Modal` with a transparent backdrop, 560 wide, anchored
  centre-top over the canvas column, its top 56pt below the canvas's top. Nothing dims, so selection
  outlines stay visible; the panels are unchanged. A backdrop click closes it and is swallowed. While
  open it owns the keyboard: `palette::read_keys` takes its keys at the top of the frame, before any
  panel draws, and `input::resolve` does not run. ↑↓ wrap, Page Up/Down move eight rows, Escape
  closes whatever is typed, the chord again closes, and the cheatsheet's chord (read first, since
  the palette's `/` would take it too) swaps the palette for the sheet. It works in focus mode.
- **Not listed:** nudge, size step, point stepping, opacity digits, Escape and Enter, text chords
  outside a text session, `PasteRelease`.

## The shortcut cheatsheet (`cheatsheet.rs`)

- **Doors.** `Ctrl+Shift+/` (and `Key::Questionmark`) in the editor (`discovery_chord`, both modes)
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
  `Item::alt_accel` joined by "or". A row is never dimmed for availability and never runs.
- **Sections** (`sections`): Tools, Zoom, Pages, Edit, Structure, Arrange, Layer, View, Components and
  kits, File, Text editing, Mouse, Library, dealt in order into three columns, each filled to about
  its share; a section is never split. A narrow window deals into two or one, so no column is under
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
  steps, Escape, Enter, `PasteRelease`, opacity digits). It checks actions, not spellings, so a
  second key for an action already shown is the hand list's to carry (`Item::alt_accel`).

## Focus mode

Focus mode (`OndinApp::focus`, `ViewSwitch::Focus`, `Ctrl+\`, the palette's *focus* or *zen*) is
a normal editing mode with the panels out of the way. Present mode is a separate, view-only mode and
none of the rules below are its.

- Hides exactly: top bar, layers pane, tool rail, inspector, and the inspector's popups (picker,
  kit picker, var browser, field menu, var autocomplete, mode menu, style popups). Everything edited
  *with* stays — ruler bars, guides, layout grids, selection box, handles, size badge, hover outline,
  frame tags — and the canvas works normally, so a guide can be pulled out of a ruler. The rulers
  are an editing aid like guides and layout grids, not app chrome. The **floating status bar stays**
  (it carries *"Focus mode — Escape to leave"*), and so does the minimap.
- On Windows/Linux the top bar is the title bar, so caption buttons go too; `chrome::focus_strip`
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

## Present mode (`present.rs`)

The document's top-level frames shown one at a time, page by page, fitted to a fullscreen window,
with nothing of the editor around them and nothing to edit with. `OndinApp::presenting` holds a
`Presenting { frames, pages, entered_at, at, was_fullscreen, stepped_at, view, to_top }` while it
runs (`pages` names each frame's page, index for index).

- **Doors.** `Action::Present`: `Ctrl+Alt+Enter` in normal mode, `menu::Item::Present` (*Present mode*,
  `PLAY`, `Group::View`) in the palette and the cheatsheet's File section, and the View menu's last
  row. The palette row dims *"No frames to present"* on the same test `start_presenting` makes
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
  (or `Alt`+wheel) zooms in at the cursor; the plain wheel and `Shift`+wheel then scroll. The wheel
  is parsed as the canvas parses it (`canvas::wheel_travel`, `canvas::wheel_zoom_factor`).
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
  (`enter_applies_the_card_s_first_fix`). `contrast_badges` runs after `canvas_ui` in the same
  panel; called before it, the canvas takes the badge's press
  (`a_click_on_a_failing_badge_opens_the_card`).
- Deriving the escape clip from overrides makes resized children pop out of their frame.
- The cheatsheet's chord both opens it (at its door) and closes it (in `cheatsheet_ui`), in one
  frame's input; `just_opened` is all that stops the opening press closing it.
  `the_chord_opens_the_sheet_and_closes_it` pins it.
- The `return` after `present_ui` is what makes present mode view-only: without it the editor runs
  below and `Delete` deletes the selection nobody can see. `present_mode_steps_frames_and_edits_nothing`
  pins it.
