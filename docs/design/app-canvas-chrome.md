# Canvas chrome, the top bar's menus, the command palette, the cheatsheet and present mode

Overlay chrome drawn during gestures (escaping clip, create box, size pill, Alt-drag ghost), the
pixel grid, the top bar's View/Snap menus and Settings button, the command palette, the shortcut
cheatsheet, and present mode. Part of §9.4.

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
- **Snap**: *Snap to shapes*, *Snap to guides* (ruler guides, and layout grids while shown), *Snap to
  grid*, *Snap to baselines* (last: no accelerator, not Illustrator's).
- **Zoom** (`zoom_control`): a text field on top, unfocused at open, holding the live zoom as its
  value; Enter applies `OndinApp::apply_typed_zoom` (`150`, `150%`, `37.5 %`; about the canvas
  centre, clamped like every zoom; anything else says *"Type a zoom percentage, like 150"*) and
  closes the menu — an untouched value applies nothing, since it reads the rounded zoom. Below it
  *Zoom in*, *Zoom out*, *Actual size*, *Fit to page*, *Fit selection*. `OndinApp::zoom_entry`
  holds the typed text while the field has focus, `None` otherwise.
- View and Snap render rows from `input::ViewSwitch`, the enum chords name, so label, order and state come from
  one place. `ViewSwitch::accel` is `None` for *Snap to baselines*, *Show layout grid*, *Show status
  messages*.
- All are **view state**, not document state. All but present mode are kept in `prefs.view`
  (`prefs::ViewPrefs`), written once a frame when one has changed
  (`OndinApp::remember_view_switches`, which also catches side doors such as a guide pulled from a
  ruler turning guides on), and a launch opens on them. All default on except *Lock guides* and
  present mode; a launch never opens in present mode.

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

## The command palette (`palette.rs`)

`Ctrl+K`, alias `Ctrl+/` (`shortcuts.md` §8), in the editor only: in the library `Ctrl+K` stays the
file search, and the library screen clears an open palette. `input::discovery_chord` reads the
chord in both `normal_mode` and `text_insert_mode`. With a modal up the chord does nothing, and a
modal that opens closes the palette. App actions only; typed syntax belongs to Command Mode
(`vm.md`). The design canvas's *Command Palette* artboard is the mockup; these rules are the spec.

- **One registry.** A row is a `menu::Item`, with the context menu's label, glyph, chord, tick and
  dim reason. The app verbs no context menu shows are `Item`s too (undo and redo, save, new and
  close document, the library, settings, *Export as …*, tools, place image, zoom, align,
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
  chords, then always the page's (canvas menu's) rows. *Add point here* and *Make this the key
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
  (`cards_seen`) are dim, *Not shown for this selection*; in present mode all are dim. Running one
  sets `reveal_card`, and `panel_full` opens that card, scrolls it to the top and focuses its first
  field: the first widget drawn in it that senses click, drag and focus (`first_field`), which a
  value or text field does and a button or a reorder grip does not. Tool rows (*Tool:
  Pen*, `P`). The View and Snap switches show their tick, which gives the unbound ones (*Snap to
  baselines*, *Show status messages*, *Show layout grid*) a keyboard route.
- **In a text session** the chord opens the palette too, with the text session's menu rows and
  styling chords first. The session stays open under it; the palette's field holds focus, so the
  canvas reads no keys, and Escape closes only the palette.
- **Placement and keys.** An `egui::Modal` with a transparent backdrop, 560 wide, anchored
  centre-top over the canvas column, its top 56pt below the canvas's top. Nothing dims, so selection
  outlines stay visible; the panels are unchanged. A backdrop click closes it and is swallowed. While
  open it owns the keyboard: `palette::read_keys` takes its keys at the top of the frame, before any
  panel draws, and `input::resolve` does not run. ↑↓ wrap, Page Up/Down move eight rows, Escape
  closes whatever is typed, the chord again closes, and the cheatsheet's chord (read first, since
  the palette's `/` would take it too) swaps the palette for the sheet. It works in present mode.
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
- **Sections** (`sections`): Tools, Zoom, Edit, Structure, Arrange, Layer, View, Components and
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
- The cheatsheet's chord both opens it (at its door) and closes it (in `cheatsheet_ui`), in one
  frame's input; `just_opened` is all that stops the opening press closing it.
  `the_chord_opens_the_sheet_and_closes_it` pins it.
