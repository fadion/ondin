# Compare with a version

The open document set against one pinned version (`.versions/…`, `library.md`), or two pinned
versions against each other: a read-only mode on the canvas marking every layer that differs, a
bar over the canvas and a changes card in the inspector's place. Part of §9.4. The engine is `ondin_core::diff`, the one `ondin diff` prints,
matched by id, so a renamed or moved layer is one change.

## Compare with a version (`compare.rs`)

- **State.** `EditorSession::compare: Option<Box<Compare>>`, on the document's session, so it
  travels with its tab; `adopt_document` (open, restore) drops it. `Compare::pick` is the version,
  or the older of two; `Compare::newer`, when two are compared, is the newer, which stands for Now
  everywhere the open document would (`Compare::now_doc`): the diff, the boxes, the names, Now's
  drawing. `OndinApp::then_canvas` and `now_canvas` are the versions' canvases, each made on first
  use and dropped on the first frame with no compare.
- **Doors.** Each Version history row has one compare button, *Compare with now*
  (`versions::NOW_GLYPH`, the git-diff glyph), before the pencil and shown with the row's
  other buttons (`library.md`): it closes the card and opens the compare. An opened row's *Show
  on canvas* (the git-diff glyph) opens the version against the one before it, as two versions —
  the pair its summary line describes; Now's opens the newest version against now. With
  Shift held it is the second of two (`OndinApp::compare_pick`): after a Shift+click on another
  row, or with another row's compare with now open, it compares the older of the two (the lower
  row) with the newer; else it picks its row (`OndinApp::versions_pick`, tinted, its button lit,
  the status line asking for a second) and the card stays, a second Shift+click on the same row
  dropping the pick and closing the card dropping it too. The rows being compared are tinted when
  the card is opened again. *Compare with version…*
  (`Action::CompareVersion`, `menu::Item::CompareVersion`, a palette command only, since the
  Document menu's *Version history…* opens the same card; the
  palette row disabled outside the library with the version card's sentence) opens the card with
  the compare buttons lit (`OndinApp::versions_compare`). A document unchanged since the version
  opens no compare and says *Nothing has changed since this version* (two the same: *These
  versions are the same*). Opening leaves a compare already open, closes the find box, Colour
  review and a tokens review, ends a contrast check and a vision simulation, clears any
  gesture preview, and takes the selection aside; leaving gives it back when the compare ends on
  the page it opened on.
- **Read-only, in three layers**: the session refuses behind the doors the app closes itself.
  - `EditorSession::commit_inner` refuses any non-empty transaction with
    `CommitError::ReadOnly`, so `commit` answers `false` and no door reports a write that never
    happened; `undo`, `redo` and `jump_to` (a History row) refuse, each saying `compare::READ_ONLY`
    on the status line. What is written outside `apply` is refused at its doors
    (`OndinApp::refused_by_compare`): for the tokens link the review does not open, and *Unlink*
    and Apply say `READ_ONLY`; *Make a kit*, *Stop sharing as kit* and the publish dialog say it
    too, and the publish itself refuses.
  - `OndinApp::dispatch` runs only what `compare::admits` (save, open, new, close and tab keys,
    settings, exports, cheatsheet, *Show in library*, version history, compare, zoom, page steps,
    private mode, Escape) and says `READ_ONLY` for the rest — silently for `Enter`, which the bar
    reads. The palette and the other view switches are refused with the top bar's View, Snap and
    Tools menus, which are disabled, as are undo and redo: each reaches modes and overlays the compare
    does not draw, and a diff viewer is not worth guarding every one of them for.
  - The canvas runs `compare_input` instead of the mode input and the context menu: any drag pans,
    Swipe's handle drags it, a click on a mark makes it current. The tool rail is hidden, the
    changes card replaces the inspector, and `draw_canvas` draws `paint_compare` (the version,
    then the frame labels over it, then the marks) and none of the editing chrome.
- **Rows** (`compare::rows`), from `diff::diff(then, now)`, grouped by page in the diff's order
  (removed pages last). A page's own change, or an added or removed page, is a row with no mark;
  a row's page is the page holding it in whichever document has it (the version, for a removed
  layer), `None` only in the kit vault.
  **The topmost listed layer is marked**: a listed layer under one that already has a row folds
  into it. Under a changed row it is kept in the row's `folded`, with its own changes, and counted
  in the row's `layers`. An added or removed row folds nothing: its `inside` counts the layers that
  came or went with it, which the diff does not list, and a layer listed under it was there before
  (moved into a new frame, say), so it keeps a row of its own. Each row keeps its world box in both documents
  (`Resolved::world_bounds`); *moved or resized* is both boxes present and differing by
  `diff::EPSILON`. Variables, modes, styles, guides and publish state go under *Not on the canvas*,
  their status read from which side has a value.
- **The bar** (`compare_bar`) sits where the contrast checker's and the vision bar's do, on their
  frame: centred on the canvas, 12pt below its top (and below the rulers when they show). It holds
  the git-diff glyph, *Comparing ‹name› · ‹when› with now* (an unnamed version: its time; two
  versions: *with ‹name› · ‹when›*), a `ui::segmented` *Then · Now · Swipe*, the *Overlay*
  toggle (a 24pt field button with the bounding-box glyph, as tall as the segmented track;
  `Compare::overlay`, lit while the marks show), then the contrast checker's steps: *n changes*,
  *i / n* once one is current (in a cell as wide as *n / n*), the previous and next carets, and an
  × that leaves.
- **Keys.** `Enter` / `Shift+Enter` step, `\` with no modifiers toggles Then and Now, both read by
  the bar while no modal, palette or text field has the keyboard. `O` toggles the overlay under
  the same conditions, taken before the keymap and consumed (`compare_overlay_key`), since it is
  the Ellipse tool's letter and would otherwise reach `dispatch` and its read-only sentence.
  `Escape` leaves: a rung of `OndinApp::escape` just below colour review.
- **The overlay** is on whenever a compare opens. Off, no mark, tag or ghost is drawn and a click
  on the canvas picks none; the Swipe handle, its pills, the frame labels and the card stay. A row
  picked in the card turns it back on (`show_card_row`) and steps to it; the bar's steps and
  `Enter` leave it as it is, so Then and Now can be flicked between bare.
- **Stepping** (`step_compare`) goes round the ends. `show_change` switches page when it must, or
  shows a page the open document lacks;
  `reveal_change` then pans the change, and a moved layer's old box with it, into view with the
  zoom kept (`OndinApp::reveal`). It runs from the bar, after the canvas, so a first visit's framing
  does not override it.
- **Marks**, outset 3pt from the layer's box and told apart by more than hue:

  | Status | Mark |
  | --- | --- |
  | Added | solid blue (`compare::ADDED`) outline, a *+ name* tag |
  | Removed | dashed orange (`REMOVED`) outline where it was, a *− name* tag |
  | Changed | solid ochre (`CHANGED`) outline, *~ name*, *· n* when it stands for several layers |
  | Moved or resized | the changed mark, plus a dashed orange ghost at the old box and a line between |

  A tag is no wider than its mark (48pt at least), a longer name cut with an ellipsis, so it never
  runs over the frames beside it. The current mark is ringed white at 2pt and tinted, and drawn
  last. A click picks the smallest
  mark under the pointer. While comparing, each Pages row carries an ochre badge counting its
  changed layers.
- **A compared page the open document lacks** (`Compare::elsewhere`) is listed in the Pages
  section after the document's pages: its name faint (from the newer version when it has the page,
  else the version: `Compare::page_doc`), a 16pt status tag at the row's end — removed for one
  only Then has; with two versions, added for one only the newer has and changed for one both have
  — and a tip saying which (`Compare::elsewhere_tip`). A click on it, or stepping to a row on it,
  shows it (`OndinApp::go_away`): `Compare::away` holds the page and parks the camera of the
  current page, which `EditorSession::page` stays all the while, so nothing outside the compare
  ever sees a page the document lacks. The page is fitted (`document_bounds` answers for it), the
  side switches when the one showing lacks the page (Now to Then, Then to Now), and a side without
  it draws the theme's canvas ground; no frame labels are drawn. `go_to_page` comes back first, the parked camera with it, and so does leaving; the
  library's last view records the parked camera, never the away one. `OndinApp::compare_page` is
  the page the canvas shows, for the marks, the reveal and the Pages check.
- **Then and Swipe** draw the version through a second `CanvasRenderer` (`CanvasRenderer::another`:
  the same device, its own texture and decode cache, so switching sides redraws neither) with the
  session's camera and page. The version never changes, so its stamps are one fresh
  `session::next_revision` for good. A page the version lacks draws the theme's canvas ground.
  With two versions Now draws the newer through a third (`now_canvas`) over the open document's
  drawing, and the frame labels, which are the open document's, are not drawn.
  Swipe clips the version left of a draggable handle (`ui::split_handle`: a 2pt accent line, a
  14×36 accent grip with the drag grip's six dots), with *THEN* and *NOW* pills in the bottom corners.
  A press within `ui::SPLIT_REACH` of the line takes it; anywhere else pans. Over the line and
  through its drag the cursor is the drawn horizontal resize cursor (`canvas::resize_cursor`), not a
  stock icon, which would sit under the drawn arrow `canvas_cursor` has already set. A right-click during
  the handle's drag puts it back where the drag began (`Compare::split_from`, through
  `OndinApp::cancel_gesture`), and the held press moves nothing until it is let go.
- **The card** (`compare_card`): *CHANGES* and *n layers · n pages · n more*; per page a heading
  (file glyph, name, row count, dimmed for a removed page); each row a
  16pt status tag, the dim trail then the name (*· n inside* for an added or removed layer with
  contents), up to four value lines *what before → after* (before struck), then *and n more
  changes*; a folded layer's changes are among them as *Child · what* (*Child added* / *removed*).
  A row with folded layers has a caret at its right end (`Compare::expanded`): expanded, the row
  shows all its own values, then each folded layer apart — a 12pt status tag and its name, and all
  its values indented 17pt under it. A click on the caret expands or folds; a click elsewhere on a
  row makes it current and steps to it. The list measures every entry each frame by arithmetic
  (`card_items`, `row_line_count`) and draws only those in view and one past each end, so `Tab`
  finds the next row; each entry's id is its own, not the layout's next, and the list ends with
  `ui::keep_focus_in_view`.
- **Private mode** masks the names in the card and in the marks' tags through `private::layer` and
  `private::page` and the bar's (*a version*, *with another*), drops the trail, and replaces the value lines with
  *n changes*.

## Traps

- The session's refusal in `commit_inner`, `undo`, `redo` and `jump_to` looks redundant beside `admits`, but
  panels and canvas paths commit without going through `dispatch`.
  `a_compare_refuses_every_edit_and_escape_leaves_it` pins the commit refusal.
- `compare_bar` runs after the canvas in `OndinApp::ui`, and the reveal with it: before it, a page's
  first-visit framing lands after the reveal and the change is out of view.
  `stepping_switches_page_and_reveals_the_change` pins the reveal.
