# Pen and node tools

Drawing paths with the pen and the pencil, mirrored or not, resuming them, editing their points with
the node tool (`A`), joining two path layers, and per-anchor corner radii. Part of §9.4.

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
- **`Ctrl` held moves points instead of placing them** (`canvas::pen_holds_points`). With a run in
  flight a press takes one of its anchors or drawn handles within `PEN_PICK_PX`, handles first
  (`pen_run_grab`), as `Drag::PenPoint`: an anchor lands where a pen press would snap and carries its
  handles, a handle pulls as the node tool's does, mirrored unless `Alt` (`pen_point_drag`). While
  held every anchor shows its handles, and the pending segment, its readout and the close and join
  rings go; the cursor is the node tool's arrow. A press on nothing does nothing, and the drag runs
  to its release whatever `Ctrl` does meanwhile; nothing is committed, as with the rest of the run.
  Inside a path edit with the pen bias armed and no run in flight, `Ctrl` is the node tool's point
  editing on the edited path instead (`node_input`), with the pen's verbs and `Ctrl`'s own node
  meanings (pull handles out, bend a segment) off while it is held.
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

- **The trail.** Press and drag leave a trail of samples (one per screen pt of movement, read from
  every pointer move the OS reported in a frame, so the frame rate does not thin it), drawn as a
  1 pt line in the selection blue, not as the final stroke, so nothing looks fitted before it is. A
  stroke in flight keeps the pencil's input while Space is held, so its release still commits it.
- **The fit.** On release `build::freehand_outline` fits the trail as *Simplify* does (`fit::fit_open`
  or `fit_closed`, then `straightened`), within 1.5 screen pt, so zoomed in you draw finer. Its corner
  angle is `build::FREEHAND_CORNER_DEG` (45°), wider than `SIMPLIFY_CORNER_DEG`: at the pencil's
  coarser reach a hand's jitter on a small loop turns by more than 15°, and 15° left a hand-drawn
  circle in far more segments. A drag that never gets 3 screen pt from its press is a click and draws
  a dot: a run a hundredth of the stroke's width long from the press, which the round caps draw round
  at any zoom. On the end of a
  path it would carry on, a click draws nothing.
- **What lands.** One stroke, no fill, round caps and joins, in the frame under the press
  (`draw_target`). Colour and width are the last pencil path's as it stands now
  (`OndinApp::pencil_last`), so a width changed on it in the Stroke card carries to the next stroke;
  before any, the pen's (`PEN_STROKE_COLOR`, `PEN_STROKE_WIDTH`). `tools::create_paths` takes the whole
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

## Mirror while drawing

The pen and the pencil draw the other half of a new path as they go, reflected across a vertical
axis, a horizontal one or both (`crate::mirror`).

- **State.** `EditorSession::mirror` (`mirror::Mirror { axes, at }`), per document, in memory only.
  `at` is where the axes cross once one has been dragged. `None` is the default: the middle of the
  frame the run lands in (`draw_target` under the run's first point; under the pointer while nothing
  is drawn), on open canvas the middle of the canvas past the rulers (`view_middle`).
  `OndinApp::mirror_axes` resolves it. A new run on open canvas fixes `at` there as it starts
  (`pin_mirror_axes`), so a pan or zoom mid-run does not move the axes under anchors already placed;
  *Reset axis* lets them follow the view again. Leaving the pen and the pencil for another tool
  puts `at` back to `None`, so a placed axis does not carry into the next
  spell of drawing; the eyedropper's round trip keeps it, and the axes choice itself stays.
- **The joining rule** (`mirror::mirrored`). An end of the run within `ON_AXIS_PT` (8 screen pt) of
  an axis is pulled onto it, and the halves join there:

  | Ends on the axis | What lands |
  | --- | --- |
  | both | one closed symmetric path: the run, then its image backwards, the shared anchors once |
  | one | one open path, joined there |
  | neither | two subpaths in the one layer |

  An already-closed run gets its image as a second subpath. At a join the anchor keeps the drawn
  handle on one side and takes its mirror on the other, so the path is as smooth across the axis as
  it was drawn. *Both axes* applies the vertical rule, then the horizontal one over what that made,
  so a quarter drawn from axis to axis closes into a whole.
- **New runs mirror; extending an end does not.** The pen's and the pencil's new-path finishes go
  through `OndinApp::mirror_run` and `tools::create_paths` (several subpaths in one `Path` node), one
  undo step under its usual name, *Add path* or *Draw path*, and mark the path with the axes they
  mirrored across (`mirror::mark_mirrored`, `path_mirror_of`), so it is saved mirrored (*Mirrored
  paths*, below). The model keeps only the axes that hold, so a path drawn in a turned frame lands
  plain. A new run inside the edited path (the node tool's pen on bare canvas, `PenEdit::minted`)
  mirrors too: across that path's own axes when it is mirrored, otherwise across the edit's own
  switch when it is on, which leaves a plain path plain. The edit's switch (`OndinApp::edit_mirror`,
  read and written through `OndinApp::mirror` / `mirror_mut`, as the drawing switch is outside an
  edit) starts off in every edit, whatever the drawing switch says, and ends with the edit. `OndinApp::mirror_frame` makes that choice for the commit,
  the snapping and the chrome. Extending an open end (pen resume, a pencil stroke carried on) does
  not mirror.
- **Pen snapping.** A new run's anchor within `ON_AXIS_PT` of an axis lands on it, over the other
  snaps (`snap_to_axis`, in `canvas::pen_point`), so a run can start and end exactly on it. It is
  measured in the space the axes are straight in: a mirrored path's own, the world for the switch.
- **On the canvas** (`mirror_chrome`). While the pen or the pencil is armed and mirroring, or the
  node tool's pen is, and the run in flight is not extending a path, each axis
  is a dashed `accent_400` line through the frame the run began in or the pointer is over, when the
  axes cross inside it; otherwise down the whole canvas past the rulers (`axis_frame`). A 16×10
  accent pill at its top end (left end for the horizontal axis) is its grab handle: a drag moves
  that axis, a right-click mid-drag puts it back where the drag began (`Grips::from`, in
  `cancel_gesture`), a double-click puts the axes back to the default. The pill claims its pointer
  (`mirror::Grips`, `OndinApp::mirror_grip`): a press or drag on it places no point, shows no
  *On axis* mark and wears the resize cursor, under the pen, the pencil and the node tool. While the pen
  draws, the mirrored run shows at 45% of the pen stroke; while the pencil draws, its trail's image
  shows as it goes, the trail's own line at 45%, joined where the commit will join it. Where the next point would land on an
  axis (`lands_on_axis`: the pen's next anchor as its press would snap it, or the pencil's pointer,
  whose end the commit pulls on) a snap ring and an *On axis* `ui::canvas_pill` show, both in
  `color::MEASURE`, the snaps' red. The pen's opening status reads
  *Mirroring across the vertical axis · start and end on the axis to close the shape* (the
  horizontal axis likewise; under both axes, *draw from one axis to the other to close the shape*)
  instead of its usual hint.
- **The switch** is the path bar's Mirror button (*The path bar*, below).

### Mirrored paths

A path drawn mirrored is saved mirrored (`NodeKind::Path::mirror`, `core-model.md`): its axes run
through the centre of its control box and are kept only while the path is symmetric about them.
Point editing keeps it symmetric. It wears the mirror switch's `flip-horizontal` glyph twice: at the
head of its size badge while it is the lone selection (`canvas::size_badge_marks`), and as its layer
row's resting badge (`BADGE_MIRROR`, tooltip *Mirrored path*), which the eye and lock cover on hover.

- **Every point edit that keeps the anchor count is made symmetric before it is written**
  (`OndinApp::rewrite_points`, `mirror::symmetric_edit`, reached through `canvas::rewrite_edit` and
  `rewrite_scrub`): drag, nudge, handle drag and fields, corner radius, align, distribute and
  spacing, point-box scale and rotate, segment move and bend. In the path's own space, where an
  anchor changed its image takes its reflection; where both changed, the first in path order wins.
  An anchor on an axis is held to it and slides along it: where the run crosses the axis its two
  handles are each other's image, where the run lies along the axis both handles stay on it.
- **The base is what the edit started from**: the committed path for a drag
  (`edited_subpaths`), last frame's preview for a scrub's increment (`preview_subpaths`).
- **Images pair by position** (`mirror::partners`), and which way round the run passes an image is
  read from the anchors' neighbours, not their handles: a corner has none to tell by.
- **Delete** takes each point's images too (`with_mirrored_points`). **Add point** inserts the
  images too (`mirrored_inserts`); on a segment that is its own image, one crossing an axis, a point
  within 0.02 of the middle goes in once, at the middle. The point asked for is selected.
- **Other path writes are not adjusted**: cutting a segment, *Join*, *Reverse* and the rest go
  straight through `tools::rewrite_path`, and the model drops an axis they break.
- **The axes on the canvas** (`own_axes_chrome`). While the node tool edits a mirrored path, its
  own axes are dashed `accent_400` lines through its control box, `OWN_AXIS_OVERHANG` (16 screen pt)
  past each end, turned with the layer, with no grab pill: the points put them there. A new run
  inside it shows its mirrored half and the *On axis* mark as a new path's does.
- **Flatten** makes a lone mirrored path plain: the same outline, its axes cleared, one step
  (`app-booleans.md`).

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

A short rail floats beside the tool rail, centred on its Pen slot (`OndinApp::pen_button`, whichever
of Pen and Pencil it shows) and free to overlap the ruler (`crate::path_bar`; both bars are drawn by
`show_bar`). It shows nothing the canvas does not already offer except *Simplify*, *Offset* and the
mirror.

- **While a path's points are edited** (`canvas::edited_path`) it holds the node tool's verbs: select
  points (`A`), the pen bias (`P`) and Mirror, the edit's own switch with the drawing bar's flyout,
  dimmed until the pen bias is picked (*Pick the pen to mirror what it draws*) and on a mirrored
  path, whose runs follow its own axes; *Make corner*, *Make smooth* and *Join*, dimmed until the
  point selection allows them; *Reverse* (`Shift+D`); *Simplify*; *Offset*; and *Done* (`Enter`).
- **While the pen or the pencil is armed** (`path_bar::drawing_bar`) it holds Mirror, a rule, and
  *Done*, which finishes a pen run and drops to Select. Mirror
  (`icon::FLIP_HORIZONTAL`, lit while mirroring or while its flyout is open) opens a `ui::flyout` in
  the `Foreground` order (`OndinApp::mirror_open`): *Off*, *Vertical axis*, *Horizontal axis* and
  *Both axes* as `ui::menu_check` rows, a rule, and *Reset axis*, dimmed until an axis has been
  dragged (*The axis is where it starts*). A pick or a press elsewhere closes it, and so does
  `Escape`, a rung of the ladder; leaving the pen and the pencil drops it.

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
  stored path back, radii included. Every distance asks for the mirror the card opened with
  (`OffsetCard::mirror`), so going back to zero restores an axis a lopsided offset dropped. A press outside the card, `Escape`, or the edit ending closes it.
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
- A mirrored path's image is told which way round by its neighbours, never its handles: a handle
  pulled out of a corner would land on the wrong side
  (`a_handle_pulled_from_a_corner_mirrors_onto_the_right_side`).
- `GeometryPatch::Path`'s inverse must carry `Some(old)` for the mirror, not `None`: `None` would
  keep only what holds after the edit, and undo would leave a dropped axis dropped
  (`a_lopsided_edit_drops_the_axis_and_undo_restores_it`).
