# Snapping

What a gesture snaps to (shapes, ruler guides, layout grids, text baselines, equal gaps, the pixel grid), the
precedence between them, and how moves, resizes, guide drags and creation each use them (`snap.rs`).
Part of §9.4.

## 9.4 Snapping

### Sources arrive as one value

`snap::Snapping { targets, guides, baselines, pixel, zoom }`, built only in `OndinApp::snapping` and
taken whole by every snap function.

- A switched-off source arrives **empty**, not flagged — an empty list cannot win. `pixel` is the one
  flag (the lattice is everywhere). `zoom` is not `Default`-able (zero zoom = infinite tolerance).
- **The Snap menu is the only way to switch snapping off**; no modifier does. `Snapping::none` is
  `#[cfg(test)]`.

| Source | Built by | Notes |
| --- | --- | --- |
| Shape targets | `snap::targets` | edges and centres of nodes in view; excludes the **gesture's own subtree** (§15 D43); capped by `SNAP_TARGET_LIMIT` |
| Ruler guides | `OndinApp::guide_lines` → `snap::GuideLines { x, y }` | via `shown_guide` (a dragged guide snaps where it looks); frame-scoped guides projected only if the frame is `rulers::upright` (angle and skew ≈ 0, mirrors allowed; a quarter turn is not upright). No culling. |
| Layout grids | `OndinApp::layout_grid_lines`, appended to the guide lines | under *Snap to guides*, and only while *Show layout grid* is on (`show_layout_grids`): both edges of every visible track of a shown frame, clipped to the frame, x for columns and y for rows (swapped under a quarter turn), in world space; they match as ruler guides do (tie wins, no mark). A frame that is moving or inside what moves is skipped (its grid travels with it), and so is one turned off the axes; a mirrored frame snaps. No culling. |
| Baselines | `canvas::baseline_lines` → `Vec<f64>` world ys | from cached `TextLayout::baselines` of visible text in view, excluding the gesture's whole subtree (`snap::is_inside_any`); rotated/skewed text contributes nothing, a half-turn is admitted (off-diagonal test); own switch, on by default, unbound |
| Pixel grid | `snap::pixel_delta` | fallback only |

`rulers::upright` also decides the dotted guide extension and whether `rulers::measure_guide` can
quote a guide, so snap, extend and measure cannot drift apart.

### Precedence

**node / guide / baseline match > pixel grid > nothing**, with an equal-gap match competing on
distance at the top rung and losing ties to alignments.

- A ruler guide (or layout-grid edge, which arrives in the same list) wins a tie with a node and with
  a baseline (it was placed on purpose; also prevents flicker). A node edge beats a baseline too.
- Pixel rounding is consulted only for an axis nothing else claimed — an alignment at 100.5 stays at
  100.5.
- Private `Matched { Node(Rect), RulerGuide, Baseline(f64), Spacing(..) }` decides what is drawn,
  never where the box lands. A guide match draws no snap line (the guide is already a line); a
  baseline match draws a mark spanning the moving box.

### What a snap draws

`snap::Marks { guides, gaps }`: alignment lines, and gaps as `measure::Measure`s drawn through
`canvas::draw_measure` (the Alt-hover measure's function). `From<Vec<Guide>>` for the non-move snaps.
The overlay builds `Marks` in every arm of its match on `Drag`. Lines are recomputed in `draw_canvas`
for Move and Resize, never stashed on `Drag`.

### Move (`snap_rect`)

- **Equal gaps** (`snap::spacing_candidates`), two per axis: centre the box between its two
  neighbours, or match the rhythm past the end of a run. Only boxes that **share a band** with the
  moving one; nothing against a neighbour it overlaps on that axis; nothing under `snap::MIN_GAP`
  (0.5). Move only. Off when *Snap to shapes* is off.
- **Shift locks the axis** (`snap::move_axes`, `snap::constrain`): the dominant axis re-read from the
  live delta every frame, tie to x. Constrained *inside* the snap: `snap_rect` takes the `(bool, bool)`
  pair and gates node, guide and pixel candidates per axis, so a locked axis gets zero adjustment and
  no line, and is not offered the grid either.
- Pixel rounding rounds the box's **leading edge** (the X field's number); width untouched.

### Resize (`snap_point`)

- Snaps the **pointer**, not the box: `resize_to_handle` puts the handle at the pointer, so the
  anchored side never moves. Same tolerance and line rules as a move. Shift means `keep_ratio` only.
- `OndinApp::resize_axes`: which world axes the handle travels, from `Handle::outward` mapped through
  the node's world transform (threshold 0.3, so a 45° edge claims both). A multi-selection uses
  `canvas::selection_frame`, the transform its handles are drawn with.
- No spacing snap on a resize (it would have to move the anchored edge).
- `snap_point` also serves line endpoints, pen anchors and point/segment drags, all under *Snap to
  grid*; it rounds the dragged edge only.

### Dragged guide (`snap_guide`)

Catches node edges and centres on the guide's axis — workflow: block out with rectangles, drop guides
on their edges, delete the rectangles. Not other guides. No target exclusions. Off at the press, on
from the first move. Shift does nothing. Rounds to the pixel grid when nothing catches.

### Creation

Creation takes the pixel grid and **not** alignment snapping (the drag is aiming at a size). Both ends
round through `canvas::pixel_point`. Shift does not suppress the grid. Constraints are in
`app-create-gestures.md`.

## Traps

- Excluding only the moved node (not its subtree) from baseline or shape sources makes a dragged group
  snap to its own stationary contents.
- Zeroing the snap result after the fact on a locked axis instead of constraining inside: the layer
  slides off its line and draws a guide for a discarded adjustment.
- The switch tests drive moves only; the baseline switch is asserted on the overlay half only.
