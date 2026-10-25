# Frames on the canvas, moving between them, and copies

How an occupied frame is picked (name tag and edge), how a moved layer changes frame or reorders in a
layout, where duplicates, pastes and Alt-drag copies land, what the clipboard carries (including
between windows), and the hand-kept lists of transient state. Part of §9.4.

## 9.4 Frame name tags and picking

- Every frame draws its name above its top-left corner, outside the box: 5px in from the left edge,
  5px between text bottom and the frame's top edge, 11pt, `text::MUTED`, or `color::SELECT` when
  selected. Drawn under every overlay but the pixel grid.
- Component frames: a main's tag is a violet chip (same selected or not); an instance's a violet label
  with an outline hexagon (`color::COMPONENT`); neither ever takes `color::SELECT`. Group mains are
  tagged too (§5.3d).
- No tag on a frame inside an instance, nor on an instance inside a main. A main keeps its chip
  anywhere; a plain frame inside a main keeps its tag. **An instance's tag shows only while it is
  selected or entered**; a selected main does not bring its instances' tags.
- **Why the tag exists**: an occupied frame's background passes the press through to the marquee
  (`canvas::pick_leaf` skips it, §5.10). Exceptions: a frame with a group above it picks with its
  group chain (a card in a row selects the row); an instance, or a frame inside one, picks like a
  group, background included. An **empty** frame is clickable anywhere; a frame that is already the
  selection stays draggable from anywhere inside it (`selected_frame_at`).
- **One pick chain**: `canvas::pick_at_pointer` = tag, then `pick_leaf`, then the **edge**
  (`FRAME_EDGE_PICK_PX`, 8pt either side of the border). Used by the select click, the context menu,
  and `canvas::hover_target` (via `pick_at_pointer` and `pick_preview`). `begin_select_drag` adds
  `selected_frame_at` on top. A press-drag in the edge strip moves the frame.
- **Coincident tags**: `OndinApp::artboards` is depth-first paint order; tag readers walk
  `OndinApp::tagged` and take the **last** match — the one painted on top and readable.

## Frame membership follows the artwork

- Dragging a layer more than half out of its frame moves it to the canvas; more than half into one
  attaches it (area test, §15 D20). Commits as one transaction with the move.
- Candidates for the destination frame exclude, **before** the topmost is chosen:
  - the moved frame and every frame inside it (`OndinApp::frame_covering_except`);
  - any layer moving with it (`OndinApp::moving_with`: the selection's outermost roots, or the moved
    copy alone on an Alt-drag);
  - frames the component rules refuse (`component::can_hold`, skipped via
    `canvas::covering_that_holds`, asked only where `FrameIndex::components` says the document has
    components). A main passed over by a set of another kind is told why
    (`canvas::SET_OF_ANOTHER_KIND`); mains of two kinds into an empty set are refused together
    (`variant::takes_together`, `canvas::SET_OF_MIXED_KINDS`).
- **A layer with a group above it hops only between frames inside that group** (`canvas::group_fence`).
  A loose layer may still drop into a card in a group.
- **An in-flow layout item leaves only once the pointer is out of its container**
  (`canvas::move_destination`), area deciding past that. Before it: a frame inside the item's own
  container covering most of the moved box takes it. After it: an item pressed on its overflowing part
  stays while its moved box still meets the container's box (`OndinApp::box_meets`).
- Creation: `canvas::draw_target` gives a new layer (or frame) the topmost frame containing the press,
  else the root.

## Reorder inside a layout

- One selected in-flow layer staying in its parent: `canvas::flex_reorder_of` answers the `Reorder`
  `build::flex_reorder` asks for, through `build::layout_drop` (a grid answers new lines instead,
  §5.3c). Preview = translation plus reorder; release commits **the `Reorder` alone**. A drop back in
  place is an empty transaction.
- `canvas::stays_in_flow` is the one predicate: `move_tx`'s multi-selection arm stores no
  `SetTransform` for such an item, and `canvas::move_preview_tx` drops it, so the item keeps its slot
  during the drag.
- Several items of one container, none leaving: `canvas::flex_block_reorder_of` →
  `build::flex_reorder_many`, committed alone (one undo step); grids through `build::layout_drop_many`.
- Alt-drag unchanged.

## Where copies land

- **`Ctrl+D` remembers** (`CloneChain`: the last clones, the first one's placement, the step): move
  the duplicates and duplicate again, and the step repeats, for one layer or several (measured on
  the first clone, since a selection moves as one). Duplicating any other set restarts at zero.
- `Ctrl+V` offsets by 20 on each axis (`PASTE_OFFSET`); `Ctrl+Shift+V` by nothing.
- **Every copy lands one step above its original** (duplicate, paste, Alt-drag), under whatever was
  above it. Paste can name the slot because a captured subtree keeps the original's ids until
  `insert_subtrees` remaps them; if the original is gone or the parent differs, append.
- **With one unlocked frame selected, `Ctrl+V` and `Ctrl+Shift+V` paste inside it**, on top
  (`OndinApp::paste_into_selected_frame`, tried first by `paste` and `paste_in_place`); a layer
  copied from that frame still lands one step above its original. Each root is re-based through
  world space (`template_world`, `rebased_placements`): where the pasted box, offset applied,
  overlaps the frame it keeps its place; otherwise `Ctrl+V` centres it on the frame. The ordinary
  paste runs instead when the clipboard holds no layers, holds that frame or an ancestor of it (a
  duplicate, not a nesting), holds only layers copied from inside it, or `paste_is_held` refuses.
  *Paste here* and the panel's *Paste* do not take this path.
- **Paste to replace** (`OndinApp::paste_to_replace`, the layer menu's row, no chord): each selected outermost
  unlocked layer is replaced by a copy of the **whole** clipboard, its union box centred on the
  target's box, in the target's slot of its parent. The union box is `clip_box`: the originals'
  boxes now while every one is still in the document, else the copy-time `clipboard_from`, so an
  original moved since the copy does not throw the aim off. Targets go as
  `delete_layers` deletes them (a frame's guides first, `relink_for_delete`); one undo step, and the
  copies end selected. The counts of copied layers and targets never need to match.
- Several copies from one parent: `insert_subtrees` advances each index only past earlier inserts at or
  below it. Alt-drag builds its own `InsertSubtree` but uses the same accumulator.
- Names: typed names kept, numbered names increment per destination parent (`build::CopyNames`, §5.7b).

## What the clipboard carries

- `OndinApp::clipboard: Option<Clip>` holds subtrees **and** the image-table entries their fills use,
  collected by `build::image_ids_in` in `copy_selection` at copy time. On paste `insert_all` prepends
  `build::missing_image_ops` (only what the target lacks), **before** the layers using them.
- The clipboard survives opening another document (pasting a picture across documents is supported).
  Duplicate carries no entries.
- **Across `ondin` windows**: `copy_selection` writes the system clipboard as text — layer names, then
  `io::clip::FENCE`, then the JSON payload. Every paste door asks `OndinApp::take_clipboard`:
  `Layers` (ours, or an adopted foreign payload), `Foreign`, or `Unreadable` (an Ondin copy this build
  cannot read — never pasted as a text layer). A foreign parent id fails `Document::contains`, so the
  paste lands at the root.
- **Kit instances carry their main** (§5.3e): `kit::carried` vault mains go in `Clip::kits`;
  `insert_all` and `paste_is_held` build through `kit::insert_carrying`. Refusals (`NeedsUpdate`,
  `Circular`) are said by `insert_all` via `OndinApp::carry_refused`. Instances landing cut loose are
  counted on the status line (*"· N instance(s) detached — its main is not in this document"*).

## Hand-kept lists of transient state

Three functions clear editor state by hand-written lists, and nothing checks them:

- `reset_transient_state` (on open; clears `alt_clone`, leaves `clipboard`).
- `go_to_dashboard` (clears `picker`, `context_menu`, `open_menu`, `rename_entry`, `confirming_close`).
- `cancel_gesture`: a gate and a clear. The gate is `app::InFlight::any` (`*self != Self::default()`,
  exhaustive by construction) over `session_scrub`, `settings_scrub`, `paint_drag`,
  `export_quality_scrub`, `grid_scrub`, `grid_paint`, `guide_position_drag`, plus `drag` and
  `layer_drag` named beside it. The clear is still a list (incl. `guide_previews`, `alt_clone`,
  `autopan_total`, `chrome_hold`, `opacity_entry`); `InFlight` fields other than the grid pair are read
  back by their owners on `gesture_cancelled`, so a field that must **restore** something (e.g.
  `session_scrub`, the only record of the spans) gets to.
- `has_gesture_preview` is two fields because one `bool` cannot describe "a session" × "a gesture".

## Traps

- Filtering the moved frame out of the *answer* after choosing the topmost leaves no answer.
- Any new door resolving a pointer to a layer owes the tag and the edge; a second copy of the chain
  drifts.
- A new piece of transient state must be added to the clear lists by hand; read the clear against the
  gate as well as against the struct.
- `*self = InFlight::default()` looks like the obvious cleanup and would remove the restore path.
- Leaving the system clipboard untouched on copy means `Ctrl+V` never arrives.
