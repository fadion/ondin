# Pen and node tools

Drawing paths with the pen and the pencil, resuming them, editing their points with the node tool
(`A`), joining two path layers, and per-anchor corner radii. Part of §9.4.

## 9.4 The pen

- Press plants an anchor; dragging before release pulls mirrored handles. **`Alt` while dragging breaks
  the pair** (incoming handle stays), read live every frame.
- `preview::PenAnchor { at, out, back }`; a corner when both handles are zero. A segment is a line
  when the two handles it uses are zero (`PenAnchor::straight_to`), else a cubic — clicked corners
  stay a polyline in the file.
- The rubber band is drawn from `tools::pen_outline` (the commit's function), in the commit's ink
  (`PEN_STROKE_COLOR`/`PEN_STROKE_WIDTH`, black, 2 units). Anchors and handles are chrome in the
  selection colour.
- **A readout beside the pointer** quotes a segment as *angle · length* (`canvas::segment_readout`;
  the angle as a Line's Rotation field shows it, world start to end): the pending segment to where a
  press would land, or, while a handle is dragged out, the segment that placed that anchor
  (`canvas::pen_segment`). It is the gradient handles' label (`spacing::paint_label`).
- **Backspace removes the last anchor mid-draw** (`canvas::pen_backspace`), cancelling the path when
  the first goes; intercepted in `app::dispatch` as `Action::Delete if self.pen.is_some()` (keyboard
  actions dispatch before `pen_input`). Undo cannot help — nothing is committed yet.
- **Closing**: pressing the first anchor within `PEN_PICK_PX` (7 screen pt — the one radius for pen
  anchors and handles). Inside that radius the pending segment snaps onto the first anchor and it gets
  a ring (`tools::pen_close_snap`, used by both `canvas::pen_press` and the overlay). The closing press
  swallows its own release (`gesture_cancelled`).

### Round-tripping and resuming

- `tools::pen_anchors`: `BezPath` → `Vec<PenSubpath>` (anchors + closed flag). Quads elevated to
  cubics, several subpaths kept, the duplicate point a `ClosePath` leaves folded onto the first anchor.
  `tools::pen_path` writes back, `map_subpaths` transforms one, `rewrite_path` commits one
  `SetGeometry`/`GeometryPatch::Path` **without touching the node's transform**.
- The round trip can **add** a segment: `pen_outline` draws the closing segment and emits `ClosePath`,
  so `M0,0 L80,0 L80,60 Z` comes back with an explicit `LineTo(0,0)`. `rewrite_path`'s no-op guard
  therefore compares **canonical forms** (both sides through `pen_anchors` → `pen_path`/`pen_radii`,
  whose trailing-zero trim makes `[0,0,0]` equal `[]`). What is stored is not changed.
- Pressing an open endpoint with the pen resumes it (`canvas::pen_resume`, `pen_resume_target`),
  `PEN_PICK_PX` radius, ringed on hover. Resuming at a subpath's start reverses a working copy and
  `finish_pen` turns it back, so the start is extended by prepending; a resume that changed nothing is
  skipped.

## 9.4 The pencil (`Shift+P`)

`Tool::Pencil` (`crate::pencil`) draws a path freehand. It makes geometry, not ink: what lands is an
ordinary stroked `Path` the node tool edits like a pen path.

- **The trail.** Press and drag leave a trail of samples (one per screen pt of movement), drawn as a
  1 pt line in the selection blue, not as the final stroke, so nothing looks fitted before it is.
- **The fit.** On release `build::freehand_outline` fits the trail as *Simplify* does (`fit::fit_open`
  or `fit_closed`, then `straightened`), within 1.5 screen pt, so zoomed in you draw finer. Its corner
  angle is `build::FREEHAND_CORNER_DEG` (45°), wider than `SIMPLIFY_CORNER_DEG`: at the pencil's
  coarser reach a hand's jitter on a small loop turns by more than 15°, and 15° left a hand-drawn
  circle in far more segments. A drag that never gets 3 screen pt from its press is a click and draws
  a dot: a run one screen pt long from the press, which the round caps draw round. On the end of a
  path it would carry on, a click draws nothing.
- **What lands.** One stroke, no fill, round caps and joins, in the frame under the press
  (`draw_target`). Colour and width are the last pencil path's as it stands now
  (`OndinApp::pencil_last`), so a width changed on it in the Stroke card carries to the next stroke;
  before any, the pen's (`PEN_STROKE_COLOR`, `PEN_STROKE_WIDTH`). `tools::create_path` takes the whole
  `Stroke`. The new path is selected; one stroke is one undo step, *Draw path*.
- **The tool stays on** after a stroke, unlike the pen: freehand comes in runs. Arming it keeps the
  selection, since a stroke can carry on the selected path.
- **Closing.** Letting go within 8 screen pt of the start, having been more than 16 pt away, closes
  the path, fitted as a loop with no seam; the start wears the pen's close ring while in range. Still
  no fill.
- **Shift mid-stroke** runs the trail straight from where Shift went down, in 45° steps
  (`axial_corner`); releasing Shift carries on freehand. Both ends of the straight run are kept as
  corner anchors through the fit.
- **Carrying a path on.** A stroke started on an end anchor (within `PEN_PICK_PX`) of the single
  selected, unlocked open path extends that path's subpath instead of making a new one, reversed back
  when grabbed at the start, as pen resume does; the end is ringed on hover. Ending within 8 pt of
  that subpath's other end closes it.
- **Ways out.** Escape or a right-click mid-drag drops the stroke and records nothing
  (`cancel_gesture`); Escape with nothing in flight falls through the ladder to Select. A tool
  switch, an undo or redo that moves the document (`document_rewound`) and a page switch drop a
  stroke in flight; a document switch also forgets `pencil_last`.

## The node tool (`A`)

- A tool, not a mode. Subject: the layer selection when it is a single `Path`. Ways in: `A` with one
  selected, a click with the tool armed, a double-click, `Enter`, or the context menu's *Edit points*.
  It has no button on the tool rail: like image editing it is entered on a subject, and the path bar
  is its chrome.
- Anchors draw as markers (square = corner, round = smooth); the selection box is hidden while they
  are up (`canvas::chrome_hidden`, also covering a pen gesture).
- Click selects an anchor, `Shift` adds, a drag from empty canvas marquees points, live, as a box or
  a lasso (`Shift` toggles, `Alt` removes; `app-selection.md`, *The marquee and the lasso*), dragging moves all
  selected anchors. Handles drag from their dots, or are pulled out of an anchor with `Ctrl`+drag;
  `Alt` breaks the pair. Dragging a handle back onto its anchor within `PEN_PICK_PX` retracts it.
  `Delete` removes points and heals. `Enter` steps in and out; `Escape` unwinds a rung. Every grab goes
  through `canvas::node_grab`, which offers only drawn handles.
- Point editing also covers adding a point on a segment, bending a segment directly, keyboard anchor
  navigation and numeric handle fields.
- **An undo or redo that moves the document clears the point selection and any live pen run**
  (`OndinApp::document_rewound`, only when `EditorSession::undo` reports a change). `PointSet` holds
  `(subpath, anchor)` indices that stay in range but stop naming the clicked points; `PenEdit::subpaths`
  is a snapshot that `finish_pen` writes back whole.
- **The pen from inside the edit**: `P` arms `app::pen_bias` without switching tool. `canvas::pen_verb`
  picks by target — an open end of the edited path continues it, its ink inserts an anchor, bare canvas
  begins a **new subpath in the same node**; handles, middle anchors and the point box keep their
  meanings; another layer switches subject. Confined to the edited node (`canvas::pen_scope`). Escape
  disarms the bias one rung above the points.
- `Shift+D` (`canvas::reverse_subpaths`) reverses the runs the selected points are on, or every run;
  also works on a selected path outside point editing (non-zero fill makes winding matter).
- **Join paths** (layer menu, `canvas::join_layers`; subject `join_layers_subject`) joins two path
  layers: exactly two `NodeKind::Path` selected, both unlocked, each with an open subpath of two or
  more anchors (a line-tool `Line` is not offered). The key layer, else the first selected, is kept
  with its paint and name; the other's subpaths move into it in world space, re-expressed in the kept
  layer's space by `tools::rewrite_path`; the nearest pair of open ends between the two (world
  distance) splice through `tools::join_ends`, the point-level *Join*'s splice; the other layer is
  deleted (with `component::relink_for_delete`). One undo step.
- **A locked path is not a subject**: `canvas::edited_path` refuses it (the tool still arms, finds
  nothing); `enter_action`'s `Path` arm refuses with a sentence; `reverse_subpaths`' fallback arm asks
  `query::is_effectively_locked` itself.
- Inspector (`inspector_points`): **Edit Path** identity card with every action dead; Transform = X/Y
  of the selected points; Appearance = opacity; Fill and Stroke still the whole path's. Align buttons
  act on points (two to align, three to distribute); spacing row via `tools::space_points` (two needed,
  first point held). Arrows nudge points.

## The path bar

While a path's points are edited (`canvas::edited_path`), a short rail floats beside the tool rail,
centred on its Pen slot (`OndinApp::pen_button`, whichever of Pen and Pencil it shows) and free to overlap the ruler (`crate::path_bar`). It holds the tool's verbs:
select points (`A`) and the pen bias (`P`); *Make corner*, *Make smooth* and *Join*, dimmed until the
point selection allows them; *Reverse* (`Shift+D`); *Simplify*; *Offset*; and *Done* (`Enter`). It shows
nothing the canvas does not already offer except *Simplify* and *Offset*.

- **Simplify** (`build::simplify_path`, `simplify_outline`) flattens the drawn outline and refits it
  with `fit`'s cubics, within `SIMPLIFY_ACCURACY` (0.5 px in the layer's units), as *Offset path*'s
  refit does. Turns sharper than `SIMPLIFY_CORNER_DEG` (15°), read over a few accuracies rather than
  between neighbouring points, stay corners, so a drawn polyline loses its jitter and a real corner
  survives; a run its chord already passes within the accuracy of becomes a line. kurbo's simplifier,
  which an earlier version used, wants a smooth source: on a drawn polyline it looped and strayed tens
  of units. A refit with no fewer segments commits nothing. `ondin-core`'s `tests/simplify_suite.rs`
  holds it to that over drawn polylines and the catalogue's curves.
- **Offset** (`build::offset_outline`) is the shape grown by the distance, or shrunk for a negative
  one: everything within the distance of the outline, with corners mitred and the mitre clipped
  square at `OFFSET_MITER_LIMIT` (4) times the distance from the corner (SVG 2's `miter-clip`), so a
  sharp tip keeps more of itself than a bevel would leave. It is polygon arithmetic, not curve
  booleans: the outline is flattened (`polyline`, chords within `OFFSET_FLATTEN` or an eighth of the
  distance if less, no vertex turning more than `OFFSET_MAX_TURN_DEG` except on a curve tighter than
  the distance, whose inner side the offset swallows), read with the node's fill rule, offset by
  i_overlay with round joins, which are true to the curves however coarsely they are cut, and then
  each real corner's clipped mitre is added as a wedge outside the shape, or cut from inside it on a
  shrink. The offset runs in steps (`offset_stages`: a first of `OFFSET_STAGE` or an eighth of the
  distance, each after it as long as all before): growing by two distances is growing by one and then
  the other, and the short first step swallows the dense detail that, offset in one long step, has
  every vertex's join overlap thousands of others. The result is
  refitted with curves (`fit::fit_closed`, Schneider's algorithm: corners where the polygon turns by
  more than `OFFSET_REFIT_CORNER_DEG`, every point within `OFFSET_REFIT_ACCURACY`, or a quarter of
  the distance or a two-hundredth of the contour if less, a fitted cubic that lies along its chord
  written as a line). A grow keeps every contour, its smallest dot included, and drops only a hole it
  nearly closed; a shrink drops the crumbs it erodes to, under a hundredth of the shape's size
  squared. flo_curves, which an earlier version used, drops or inverts operands that share an edge,
  and an offset built from band pieces is all shared edges. An open subpath grows into a
  round-capped stroke around it, a dot into a disc, and has no inside to shrink.
  `ondin-core`'s `tests/offset_suite.rs` checks the shapes of `tests/common` at sixteen distances,
  and even-odd and crossing shapes, tiny shapes and distances, open paths and dots, against an
  independent point-by-point oracle (the shape and its mitred stroke) whose blind band narrows with
  the distance, and for specks, stray edges and runaway segment counts; the areas of a square and a
  circle at every distance hold the distance and the mitres exactly. A shape that misbehaves
  belongs in that catalogue. The bar's *Offset* opens a card with one scrubbable distance field through the
  edit valve; every distance is measured from the outline the card opened on, and zero writes the
  stored path back, radii included. A press outside the card, `Escape`, or the edit ending closes it.
- Both write a new path with no corner radii, the radii baked into the curves, and clear the point
  selection, whose anchors are gone.
- Both run on a worker thread (`crate::job::Job`), so the frame loop never waits on the arithmetic.
  The Offset card keeps one offset in flight; the preview shows the newest that has landed, and a
  scrub that moved on asks again when it does. Only the release waits, for the distance it ends on,
  so the commit is what the field says. A *Simplify* that lands after its path changed is dropped,
  and says so; one landing during a drag or a pen run waits for it to end; a second ask while one
  runs says *Still simplifying*; leaving the document drops it.
- The Offset card writes only over the path it opened on or last committed (`OffsetCard::owns`):
  an undo, a redo, a landed *Simplify* or any other edit of the path closes it, committing nothing.
  Closed by a press elsewhere while its field still holds a typed distance, it commits that
  distance, as any field does on losing focus; Escape closes it committing nothing.

## Path corner radii

`NodeKind::Path` carries a radius per anchor, resolved by `geometry::local_path` like a `Rect`'s four —
never baked, except by *Simplify* and *Offset*, which replace the anchors. The Appearance field writes the selected corner points, or every corner when the path is
selected without the node tool. Every corner rounds; a radius on a smooth anchor draws nothing.

## Traps

- Comparing raw paths in `rewrite_path` makes a bare click into the radius field commit and grow the
  outline by a zero-length segment.
- `PointSet` index truncation is no guard against an undo: the stale indices stay valid.
- A verb with a second door (`reverse_subpaths`' fallback) owes the lock check on that door too.
