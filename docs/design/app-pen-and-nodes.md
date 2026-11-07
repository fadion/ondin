# Pen and node tools

Drawing paths with the pen and the pencil, mirrored or not, resuming them, editing their points with
the node tool (`A`), joining two path layers, per-anchor corner radii, and *Pixel-fit*. Part of §9.4.

## 9.4 The pen

- Press plants an anchor; dragging before release pulls mirrored handles. **`Alt` while dragging breaks
  the pair** (incoming handle stays), read live every frame.
- `preview::PenAnchor { at, out, back }`; a corner when both handles are zero. A segment is a line
  when the two handles it uses are zero (`PenAnchor::straight_to`), else a cubic — clicked corners
  stay a polyline in the file.
- The rubber band is drawn from `edit::pen_outline` (the commit's function), in the commit's ink
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
  a ring (`edit::pen_close_snap`, used by both `canvas::pen_press` and the overlay). The closing press
  swallows its own release (`gesture_cancelled`).

### Round-tripping and resuming

- `edit::pen_anchors`: `BezPath` → `Vec<PenSubpath>` (anchors + closed flag). Quads elevated to
  cubics, several subpaths kept, the duplicate point a `ClosePath` leaves folded onto the first anchor.
  `edit::pen_path` writes back, `map_subpaths` transforms one, `rewrite_path` commits one
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
  before any, the pen's (`PEN_STROKE_COLOR`, `PEN_STROKE_WIDTH`). `edit::create_paths` takes the whole
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
  through `OndinApp::mirror_run` and `edit::create_paths` (several subpaths in one `Path` node), one
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
  (`edit::rewrite_points`, `symmetry::symmetric_edit`, in core so a verb writes paths the same way;
  reached through `canvas::rewrite_edit` and
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
  straight through `edit::rewrite_path`, and the model drops an axis they break.
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
  `Alt` breaks the pair. `Shift` steps the handle to an eighth of a turn measured on the canvas
  (`canvas::handle_step`, the pen's `axial_corner`), its length free, and draws a dashed line
  through the anchor along it with the angle on a pill (`draw_handle_step`); the soft pull onto the
  axes and the partner handle is off meanwhile, and both keys can be held. Dragging a handle back
  onto its anchor within `PEN_PICK_PX` retracts it.
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
  layer's space by `edit::rewrite_path`; the nearest pair of open ends between the two (world
  distance) splice through `edit::join_ends`, the point-level *Join*'s splice; the other layer is
  deleted (with `component::relink_for_delete`). One undo step.
- **A locked path is not a subject**: `canvas::edited_path` refuses it (the tool still arms, finds
  nothing); `enter_action`'s `Path` arm refuses with a sentence; `reverse_subpaths`' fallback arm asks
  `query::is_effectively_locked` itself.
- Inspector (`inspector_points`): **Edit Path** identity card with every action dead; Transform = X/Y
  of the selected points; Appearance = opacity; Fill and Stroke still the whole path's. Align buttons
  act on points (two to align, three to distribute); spacing row via `edit::space_points` (two needed,
  first point held). Arrows nudge points.

## The path bar

A short rail floats beside the tool rail, centred on its Pen slot (`OndinApp::pen_button`, whichever
of Pen and Pencil it shows) and free to overlap the ruler (`crate::path_bar`; every bar is drawn by
`show_bar`). It shows nothing the canvas does not already offer except *Clean up*, *Offset*,
*Pixel-fit* on selected points and the mirror.

- **While a path's points are edited** (`canvas::edited_path`) it holds the node tool's verbs: select
  points (`A`), the pen bias (`P`) and Mirror, the edit's own switch with the drawing bar's flyout,
  dimmed until the pen bias is picked (*Pick the pen to mirror what it draws*) and on a mirrored
  path, whose runs follow its own axes; *Make corner*, *Make smooth* and *Join*, dimmed until the
  point selection allows them; *Reverse* (`Shift+D`); *Split at point*; *Clean up* (the wand);
  *Offset*; *Pixel-fit* (`pixel_fit_points`: the selected points, or the whole path with none
  selected; *Pixel-fit*, below); and *Done* (`Enter`).
- **Split at point** (`canvas::split_points`, `edit::split_at_points`) cuts the edited path at its
  selected anchors: a closed run opens there and an open run becomes two subpaths of the same layer.
  The anchor stays on both new ends, which coincide until one is dragged away; the finishing end
  keeps the arriving handle and the starting end the leaving one, so the outline draws as before. Of
  the two ends, the one whose run leaves toward the pointer stays selected (the menu's open position;
  the bar has none and keeps the starting end), so the next drag pulls them apart. A mirrored path is
  split at each point's image too. It dims with *Select a point to split at* and, when every
  selected anchor is an open end, *An open end has nothing to split* (`canvas::split_subject`). Its
  glyph is `icon::SPLIT_POINT`; `SCISSORS` means *Cut*. One undo step. *Delete segment* already opens
  a path at a segment, and *Delete points* heals.
- **While the pen or the pencil is armed** (`path_bar::drawing_bar`) it holds Mirror, a rule, and
  *Done*, which finishes a pen run and drops to Select. Mirror
  (`icon::FLIP_HORIZONTAL`, lit while mirroring or while its flyout is open) opens a `ui::flyout` in
  the `Foreground` order (`OndinApp::mirror_open`): *Off*, *Vertical axis*, *Horizontal axis* and
  *Both axes* as `ui::menu_check` rows, a rule, and *Reset axis*, dimmed until an axis has been
  dragged (*The axis is where it starts*). A pick or a press elsewhere closes it, and so does
  `Escape`, a rung of the ladder; leaving the pen and the pencil drops it.
- **While a warp envelope is open** the warp bar stands in its place, ahead of both, and the mirror
  flyout and the two cards close (`path_bar::warp_bar`, `tools.md` *Warp*).

- **The Clean up card** (`crate::cleanup`) hangs beside the bar, its top on the wand's, as the
  Offset card does, and the two replace each other. It is a popover (`ui::menu_frame`,
  `popover_head` *Clean up* with its ×) of four `ui::labelled` rows, each a field and a word button
  of one width: *Simplify* (accuracy in px, from `SIMPLIFY_MIN_ACCURACY` to 20, *Apply*, with
  *64 points → 9* under it, which keeps the last count while a refit is in flight and says
  *Fitting…* only after `FIT_PATIENCE`), *Remove redundant points* (*9 points can go* or *Nothing to
  remove*, *Remove*, with a standing caption naming the three kinds it takes and that the shape stays
  exactly as it is), *Add points* (a bare count, 2–16, its tooltip saying it is the pieces each segment is
  cut into, *Add*) and *Smooth selected* (an amount in percent,
  *Smooth*, with *Acts on the 4 selected points* under it). A row's result shows on the canvas
  through the gesture preview while its button is hovered or focused or its field scrubbed or
  typed in, never as the pointer merely crosses the card, and the points it would take away are
  dotted in the snaps' red. Its button commits it as one step: *Simplify*, *Remove points*,
  *Add points* or *Smooth*. *Remove* dims when every point shapes the path, and *Smooth* and its
  amount field with *Select points to smooth them*. It opens with *Apply* focused, so the wand and then `Enter`
  simplifies in one go. It stays open while points are picked for *Smooth*; the wand, its ×,
  `Escape` (a rung, under the Offset card's) and the end of the edit close it. A right-click mid-scrub
  puts the number back (`InFlight::cleanup_scrub`), as every card's does; the Offset card's
  distance too (`offset_scrub`). Its numbers are kept while it is closed (`cleanup_numbers`).
  - *Remove redundant points* (`edit::redundant_points`) takes out an anchor twinned on the one
    before it, one in the middle of a straight run, and one splitting what is a single cubic in
    two, its handles in line and the curves either side that cubic's halves to within 0.001 units;
    never a rounded corner or an open end. *Add points* (`edit::split_segments`) cuts each segment
    into pieces of equal length, exactly, so the outline draws as before; the two undo each other.
    *Smooth selected* (`edit::smooth_points`) moves each selected anchor's handles that far toward
    fresh smooth ones, along the line between its neighbours, a third of the shorter leg long.
- **Simplify** (`build::simplify_outline_within`) flattens the drawn outline and refits it
  with `fit`'s cubics, within the card's accuracy (`SIMPLIFY_ACCURACY`, 0.5 px in the layer's
  units, at first), as *Offset path*'s refit does. Turns sharper than `SIMPLIFY_CORNER_DEG` (15°), read over a few accuracies rather than
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
  (`OffsetCard::mirror`), so going back to zero restores an axis a lopsided offset dropped. A primary press outside the card, `Escape`, or the edit ending closes it;
  a right-click does not, since mid-scrub it is the scrub's cancel.
- Both write a new path with no corner radii, the radii baked into the curves, and clear the point
  selection, whose anchors are gone.
- Both run on a worker thread (`crate::job::Job`), so the frame loop never waits on the arithmetic.
  Each card keeps one in flight; the preview shows the newest that has landed, and a scrub that
  moved on asks again when it does. Only the commit waits, for the number the field shows. The
  Clean up card reads its path again whenever the stored path changes (`CleanupCard::basis`), its
  own commits included, dropping the refits of the old one; leaving the document closes it.
- The Offset card writes only over the path it opened on or last committed (`OffsetCard::owns`):
  an undo, a redo, a clean-up or any other edit of the path closes it, committing nothing.
  Closed by a press elsewhere while its field still holds a typed distance, it commits that
  distance, as any field does on losing focus; Escape closes it committing nothing.

## Path corner radii

`NodeKind::Path` carries a radius per anchor, resolved by `geometry::local_path` like a `Rect`'s four —
never baked, except by *Simplify* and *Offset*, which replace the anchors. The Appearance field writes the selected corner points, or every corner when the path is
selected without the node tool. Every corner rounds; a radius on a smooth anchor draws nothing.

## Pixel-fit

A command, not a mode (`crate::pixel_fit`): it moves a layer's edges onto the pixel grid once, so a
16–24px icon renders crisp at 1×. The grid is world units. Its doors are the layer menu's Structure
row (`OndinApp::pixel_fit`, over the selection; `context-menus.md` §4), the palette, and the path
bar's button (`pixel_fit_points`, above); no chord. Its glyph is `icon::PIXEL_FIT`, an arrow
pushing a square's corner, not `GRID_FOUR`, which is the View menu's grid.

- **Anchors** (`fit_anchors`): a corner moves to the nearest grid line on both axes; a smooth anchor
  moves only across a level or upright tangent, and one with a diagonal tangent stays. Handles ride
  along, so curves keep their shape.
- **Box shapes** (rectangle, ellipse, polygon, star, spiral) keep their kind: their world box is
  rounded (`fit_rect`), a tie growing outward, and a side at least one unit long stays so. A line or
  an arc snaps both ends.
- **The grid** is whole units for every layer, so a box's X, Y, W and H all land on integers. A stroke
  one unit wide and centred then straddles two pixel rows; aligned inside or outside it is crisp.
  No half-unit grid centres such a stroke on a pixel, since that would leave X and Y on halves. An
  anchor's tie goes down, so a whole shape moves as one and keeps its size.
- **What it walks**: groups, frames and booleans are walked into (a frame's own box is not moved);
  hidden layers are passed over. Text, and any layer not square to the page (turned other than by a
  quarter turn, or sheared, `square_to_the_page`), is skipped and counted, since no move of its points
  lines it up with the pixels. A slice, which draws nothing, is left alone uncounted. An auto-layout child
  gets no special case.
- One undo step, *Pixel-fit*. The status line (`Fitted::message`) says what moved and what was
  skipped: *Moved 7 points onto the pixel grid; skipped 1 text layer and 1 rotated layer*, *Already on
  the pixel grid*, *Pixel-fit skipped 2 text layers*.

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
