# Image fills and image editing

The inspector's image fill row, the image-editing card (Settings and Adjustments tabs), image editing
mode (`Tool::ImageEdit`, the crop by handles and pan), and pictures arriving by drop or paste.
Part of §9.4.

## 9.4 The image fill row

- `panels::paint::PaintKind::Image` exists but is no picker kind (neither *Solid* nor in
  `PaintKind::GRADIENTS`; an image comes from a file dialog, not a colour wheel). `picker_body` returns
  early for an image.
- One line. The chip draws the thumbnail (`ui::Swatch::Picture`) over the checkerboard **at the brush's
  `ImageSampler::alpha`** — carried on the variant, a uniform white×alpha tint, because `thumbs` shares
  one texture per picture across layers. Grey ramp while a thumbnail is missing or deferred. The hex
  field is a plain "Image" label.
- **Clicking the chip or the label enters image editing** (tooltip *Edit this picture — crop it, move
  it, scale it*), through `OndinApp::begin_image_edit`. The door is
  `inspector::row_edits_the_mode_s_picture`, which compares the row's slot with
  `tools::croppable_fill`'s answer, so a second image fill's row is inert. A locked layer (or locked
  group, via `query::is_effectively_locked`) closes the door, with a third tooltip state naming the
  lock. The picker still opens from a locked layer's colour rows.
- The Fill/Fit/Crop/Tile strip and popover buttons live in the image-editing card, not the row. Mode
  writes go through `write_slot` and `paint::with_fit` everywhere, keeping the crop across a switch.

## The image-editing card (`inspector::image_edit_card`)

- A `POPOVER_W` `Foreground` `Area` in `menu_frame`, at `popover_anchor` taken from the **paint row's**
  rect, in the shared lane (`picker_lane_right`, which asks `edited_image`).
- **No button and no close**: it appears with the mode and goes when `Enter`/`Escape` leaves it. There
  is no click-away rule. `OndinApp` keeps only `image_tab: ImageCard` (the last tab, kept across
  entries); which layer and paint is `canvas::edited_image`'s answer.
- **The tab strip is the header**: a full-width two-cell `ui::segmented` with `ImageCard::title`
  labels. Controls are `CELL` = 26; both tracks (this and Settings' mode strip) ask for `TRACK_CELL`,
  four less, since `ui::segmented` pads two each side.

### Settings tab

- **Framing modes** Fill / Fit / Crop / Tile — an override: Crop is what handles and pan do; the others
  re-frame inside the box the crop left and hide the ghost. The crop rectangle survives.
- **Asset** eyebrow over *Replace…* (pick one file, embed it, **keep the framing** — the repair for a
  missing picture) and *Replace and reset* (framing fields to defaults, adjustments kept). Both decline
  for a selection slot and run after the `Area` closure (the dialog is modal); the card stays behind
  the dialog. Linked-source entries (*Embed/Link*, *Relink…*) are not built — no linking is authored.
- **Mirror** eyebrow over two `ui::label_button`s, *Horizontal* and *Vertical* — actions, never lit;
  words, since on a symmetric shape the two flips look the same. No quarter turn here.
- **Scale** (`icon::RESIZE`) and **Tile** (`icon::GRID_FOUR`) half-width fields, percent of source
  pixels (`1.0` = one unit per source pixel); each read by one mode and **dimmed, not hidden**,
  otherwise.
  - Scale is the crop zoom (no gesture can scale the picture). Floor `ImageRef::cover_scale`; ceiling
    `floor.max(10_000.0)` — the one inspector bound from the document. Writes via `image::crop_scaled`
    (uniform about the window centre); readout seeded from `cover_crop` outside Crop. Scrubs in
    hundredths (multiplicative); Tile in whole units.
- *Reset crop* restores `whole_crop()`'s sentinel.
- Actions go through `inspector::menu_action`, which shadows a dimmed row with an explicit
  `Sense::empty()` claimant so its tooltip still shows.
- Not here: *Original size* (resizes the node; it is `context-menus.md` §5.7's row), *Export
  original…* (removed; exporting is the Export panel), *Edit crop…*.

### Adjustments tab

- Seven rows from `Adjustment::ALL` at `ui::CARD_ROW_GAP`. Each is a `ui::value_field_metered` with the
  **word** as prefix, scrubbing percent over the file's `-1..=1`, written through `valve_slot` (one
  undo step per scrub).
- The meter is an accent wash across the field's ink **growing from the middle** (negative left,
  positive right, ±1 reaching an edge). A 1pt centre tick (`ui::METER_TICK_W`, from `ui::meter_wash()`)
  is always drawn. The number is right-aligned, signed via `inspector::signed_number`.
- A per-row reset is **dimmed, not absent**, at zero; *Reset adjustments* is a `menu_action`.

## Image editing mode (`Tool::ImageEdit`)

**Outside the mode, resizing a picture resizes it; inside it, the handles crop.** The layer's box is a
window onto its picture. There is no crop tool, rail button or `C` key.

- A `Tool` variant because routing, cursor, `chrome_hidden`, the Escape ladder and `Enter`'s toggle are
  keyed off `self.tool`.
- **Subject is derived**: `canvas::edited_image` — the tool plus a single selection whose first visible
  image fill (`tools::croppable_fill` → `Paint::image_fill`). Not stored.
- **Four doors**, all calling `OndinApp::begin_image_edit(id)` (selects, takes the tool, prints the
  status line): double-click on a picture (after the group step), `Enter` on a selected one, *Edit
  image* at the head of `context-menus.md` §5.7, and a click on a fill row's chip or label.
- **Lock**: `begin_image_edit` refuses a locked node (*"This layer is locked — unlock it to edit the
  picture"*), and `edited_image` carries the same guard (the panel can lock the row or move the
  selection without re-entering the tool). The menu row dims because the mode refuses. Text
  (`canvas::begin_edit_text`) and points refuse a locked layer the same way.

### The crop gestures

- A **handle drag** moves one edge with the picture nailed down in world space; a **drag on the
  picture** slides it under a fixed box. Neither scales the photograph. Side handles are live.
- `ImageRef::picture_box`: the whole photograph's rectangle from the transform the backends paint with.
  `tools::crop_resized`: moves the grabbed edge, bounded by that rectangle, and **holds the handle on its
  own side** (no mirroring). `image::crop_reframed`: re-reads the old mapping at the new box's corners
  so the picture stays still. Crop and frame stay in proportion — no stretch.
- `crop_resize_tx` aims `tools::resize_layer` at `crop_resized`'s corner and emits **one transaction**
  with the geometry and the `SetFills` together. The mode is forced to `Crop`. A crop is a resize, so
  the lock reaches it.
- Both drags capture start values at the press and re-derive each frame (`session.set_preview`). The
  **commit is gated on travel** (`grab_world`, `gesture_travelled`, §9.3); an untravelled release
  clears the preview (`clear_gesture_preview`). The preview is ungated.

### Chrome

- **The offcut is real ink**: `canvas::image_ghost` builds a `RenderOverrides` **chrome ghost** — a
  `Rect` at `picture_box` filled with the uncropped picture (orientation and adjustments included) at
  `GHOST_ALPHA` × layer opacity, at the layer's index so it draws under it. Only in `Crop`.
- The chrome ghost lives in its own `RenderOverrides` field so `is_empty` (`has_gesture_preview`) stays
  false while the mode is merely open. `ghosts_of` chains it. Re-installed every frame at the top of
  `draw_canvas` (survives commits; `set_preview` replaces overrides wholesale).
- `chrome_hidden` removes the selection box; `canvas::draw_crop_box` (called from `draw_canvas`, shapes
  from the free `canvas::crop_chrome`) draws the window **dashed** — 3pt at 6/6 (`CROP_DASH_PX`,
  `CROP_GAP_PX`, `CROP_STROKE_PX`) — with hollow corner handles, plus the picture's solid outline when
  the ghost is live, both via `shown_crop`. Cursor: a hand for the pan, the stock resize cursor on
  handles.
- **Done pill** (`canvas::crop_exit_pill`) in the size badge's slot, same ground, ink, radius, gap and
  `badge_placement`. A `Foreground` `Area`; `wheel_input`'s exemption names its layer.

## Pictures by drop and paste

- `take_dropped_images` runs every frame from `update`, whatever the tool, **unless
  `OndinApp::modal_is_up`**, reading `raw.dropped_files` (browser drags as bytes, file-manager drops
  as paths, both to the file dialog's loader).
- Lands at the pointer, cascaded 24 units per file; at the viewport centre with no pointer over the
  canvas. **One transaction per drop** — placements built via `canvas::image_tx` against a document
  advanced per picture (index, free name, embedded-blob check); every picture made is selected.
- Windows: no drag event carries a position, so the pointer is the desktop cursor via `GetCursorPos`
  mapped against `ViewportInfo::inner_rect` (`canvas::os_cursor_desktop_px`), read before the event
  stream.
- A **single** picture dropped onto the **selected** shape replaces its image fill, keeping crop, mode,
  tile scale and orientation. `Ctrl+V` addresses the selection with no point; the context menu's
  *Paste here* is aimed like the drop. Otherwise a new layer, fitted like a click-placed one.
- `canvas::draw_drop_indicator` while files hover: outline round the target frame (`target_artboard`,
  or an inset canvas border), a pill naming the outcome, and the hover outline on a shape to be
  replaced.
- SVG is never rasterized: a dropped or placed `.svg` (sniffed by its bytes, `app::looks_like_svg`)
  is read by `svg_in::import` as a paste is (`canvas::import_svg`, shared with `paste_svg`): one
  group named for the file's stem, centred on the drop point (cascaded `DROP_CASCADE` per file) or
  where a paste would land for the *Place image* dialog, which takes `.svg` and imports at once
  rather than loading the Image tool. Its report (*Imported logo.svg — 5 layers*) follows the
  pictures' in one status, and the drop selects both.
- `Ctrl+V` over a picture-only clipboard produces no egui event, so `input::normal_mode` also reads the
  key **release**; `Action::Paste` tries the picture before the in-app layer paste.

## Traps

- Forcing the mode to `Crop` is a real write: an untravelled press-release would flip a `Fill` picture
  to `Crop` and leave an invisible undo step.
- Calling `draw_crop_box` from inside `draw_points` means it never runs (that only runs for the node
  tool); where a `&mut self` method is called from is not testable.
- Counting the chrome ghost as a preview makes every right-click "cancel" a gesture nobody started.
- Refusing only at the four doors misses the routes into the mode that bypass `begin_image_edit`.
