# Font picker

The Type panel's family picker (`fonts/preview.rs`, `panels/typography.rs`): how each row previews its
own family, how the list stays fast, and what applying a family clears. Part of §9.4 (v1 tools).

## 9.4 The font picker

### Rows are set in the family they name

- Typefaces are chosen by eye, so each row draws its name in that family. Drawing a row asks for the
  family's regular upright — the same file applying it needs — so previewing is prefetching: by the
  click the bytes are registered.
- **The name is rasterized through `vello_cpu`, not egui's fonts.** `text::family_preview` shapes the
  name through the document's shaping path and `render::rasterize_glyph_run` draws it into one small
  cached texture per family. The row therefore shows what the canvas will draw.
- Strips are white coverage **tinted at draw time**, so one image serves normal, hovered and selected
  and follows the theme.

### The preview declines rather than falling back

`family_preview` returns `None` (the row then draws the name in the UI font) when:

1. the run did not come from that family's own blob (parley would substitute Inter — a row reading
   "Bungee Shade" set in Inter lies about the typeface);
2. any glyph is `.notdef` (catches symbol fonts, whose Latin name would be empty boxes);
3. the strip is too tall (`preview::strip_px`).

Strip size bounds differ per axis on purpose:

| Axis | Bound | Over the bound |
| --- | --- | --- |
| Width | `MAX_STRIP_PT` | **clipped** — the useful part of a name is its start |
| Height | `MAX_STRIP_H_PT` (6 × `PREVIEW_PT`) | **refused** — half a line is not a shorter name |

The height bound exists because a font may declare a tiny `unitsPerEm` against maximal
`ascender`/`descender` (line-box ratio up to ~2,048), which would overflow `rasterize_glyph_run`'s
`u16` and allocate enormous strips on the UI thread. `strip_px` is a free function so a test can drive
it without such a font.

### Loading dot

A pulsing dot at the row's right edge, drawn on `FontService::is_loading`, says the face is still
coming (otherwise a failed download and a pending one look the same).

- Its room (`LOADING_DOT_SLOT`) is reserved on **every** row, lit or not, so names never reflow when a
  face lands.
- A raised-cosine pulse on a wall clock (every dot starts at the trough), never fully dark (no strobe).
- `paint_loading_dot` requests its own repaint — eframe here is reactive.

### Virtualized list

- System fonts plus the Google catalog is ~1,900 families; one widget per family cost about a second
  per open. The list uses `ScrollArea::show_rows`, which needs a fixed row height — hence the constant
  `FAMILY_ROW_H` and hand-drawn rows rather than `selectable_label`.
- The filter's match list is memoized against the needle **and** the family list's revision, since
  the list grows under an open picker as the catalog and the system scan land. Each arrival wakes the
  UI thread (§5.4a), so rows in flight when a scroll stops still get their face.

### A popup you click into

The combo uses `CloseOnClickOutside` (egui's default closes on any click, which made the search field
dismiss the list) and closes itself explicitly when a family row is picked. The same arrangement is
used by `variants::swap_picker` (a combo with a search field) and two `Popup`s in
`panels/variants.rs` (a variant value chip's menu holding its rename field, and the Properties card's
`+` menu). Every other combo keeps the default.

### Applying a family

- **A family change clears the layer's variable-axis settings and OpenType features**: both are named
  by tag, and a tag means something different (or nothing) in the next typeface.
- **A re-pick of the family already shown is not a change** and clears nothing.
  `family_row` reports `clicked()` for any row (`selected` only paints the ground), so the decision is
  the call site's, spelled once in `family_actually_changed`.
- **A re-pick over a *mixed* selection is an edit**: the row shows one member's family (the *Mixed*
  convention), so picking it makes the selection agree and the clearing is right for every layer it
  changes.

### Spacing units (Type panel)

`%` is letter spacing's default unit (tracking must scale with size so a type token can carry it);
px is paragraph spacing's (it belongs to the layout's spacing scale and should not move when type is
tuned).

## Traps

- `Context::add_font` looks like the obvious route for previews and is not: it drops every cached
  galley and re-uploads egui's whole atlas, and keeps a second copy of each font beside parley's.
- Reserving the loading dot's slot only while loading makes each row twitch once when its face lands.
- `family_row`'s `clicked()` fires on the already-selected row; gating on it alone wipes axes and
  features on a no-op re-pick and leaves an undo step.
