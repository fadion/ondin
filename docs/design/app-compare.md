# Compare with a version

The open document set against one pinned version (`.versions/…`, `library.md`): a read-only mode
on the canvas marking every layer that differs, a bar over the canvas and a changes card in the
inspector's place. Part of §9.4. The engine is `ondin_core::diff`, the one `ondin diff` prints,
matched by id, so a renamed or moved layer is one change.

## Compare with a version (`compare.rs`)

- **State.** `EditorSession::compare: Option<Box<Compare>>`, on the document's session, so it
  travels with its tab; `adopt_document` (open, restore) drops it. `OndinApp::then_canvas` is the
  version's canvas, made on first use and dropped on the first frame with no compare.
- **Doors.** Each Version history row has a compare button (the git-diff glyph,
  `theme::icon::GIT_DIFF`, before the pencil): it closes the card and opens the compare. The row
  being compared is tinted when the card is opened again. *Compare with version…*
  (`Action::CompareVersion`, `menu::Item::CompareVersion`, the Document menu and the palette; the
  palette row disabled outside the library with the version card's sentence) opens the card with
  the compare buttons lit (`OndinApp::versions_compare`). A document unchanged since the version
  opens no compare and says *Nothing has changed since this version*. Opening closes the find box,
  ends a contrast check and a colour-vision simulation, clears any gesture preview, and takes the
  selection aside; leaving gives it back when the compare ends on the page it opened on.
- **Read-only, in three layers**: the session refuses behind the doors the app closes itself.
  - `EditorSession::commit_inner` refuses any non-empty transaction (returns `Ok` without applying)
    and `undo`/`redo` refuse, each saying `compare::READ_ONLY` on the status line.
  - `OndinApp::dispatch` runs only what `compare::admits` (save, open, new, close and tab keys,
    settings, exports, palette, cheatsheet, *Show in library*, version history, compare, zoom, page
    steps, view toggles, Escape) and says `READ_ONLY` for the rest — silently for `Enter`, which the
    bar reads.
  - The canvas runs `compare_input` instead of the mode input and the context menu: any drag pans,
    Swipe's handle drags it, a click on a mark makes it current. The tool rail is hidden, the
    changes card replaces the inspector, and `draw_canvas` draws `paint_compare` (the version,
    then the frame labels over it, then the marks) and none of the editing chrome.
- **Rows** (`compare::rows`), from `diff::diff(then, now)`, grouped by page in the diff's order
  (removed pages last). A page's own change, or an added or removed page, is a row with no mark.
  **The topmost listed layer is marked**: a listed layer under one that already has a row folds
  into it. Under a changed row it is kept in the row's `folded`, with its own changes, and counted
  in the row's `layers`; under an added or removed row nothing is added, since its `inside` count
  already says. Each row keeps its world box in both documents
  (`Resolved::world_bounds`); *moved or resized* is both boxes present and differing by
  `diff::EPSILON`. Variables, modes, styles, guides and publish state go under *Not on the canvas*,
  their status read from which side has a value.
- **The bar** (`compare_bar`), centred over the canvas's top (14pt down, below the rulers when
  they show; 38 tall, card fill, radius 8, the find box's shadow): the git-diff glyph, *Comparing
  ‹name› · ‹when› with now* (an unnamed version: its time), the segments *Then · Now · Swipe*,
  previous and next arrows round *i of n* (*n changes* before one is current), and *Done*.
- **Keys.** `Enter` / `Shift+Enter` step, `\` with no modifiers toggles Then and Now, both read by
  the bar while no modal, palette or text field has the keyboard. `Escape` leaves: a rung of
  `OndinApp::escape` just below the find box.
- **Stepping** (`step_compare`) goes round the ends. `show_change` switches page when it must;
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

  The current mark is ringed white at 2pt and tinted, and drawn last. A click picks the smallest
  mark under the pointer. While comparing, each Pages row carries an ochre badge counting its
  changed layers.
- **Then and Swipe** draw the version through a second `CanvasRenderer` (`CanvasRenderer::another`:
  the same device, its own texture and decode cache, so switching sides redraws neither) with the
  session's camera and page. The version never changes, so its stamps are one fresh
  `session::next_revision` for good. A page the version lacks draws the theme's canvas ground.
  Swipe clips the version left of a draggable handle (a 2pt accent line, a 24×36 accent grip with
  a left-right arrow), with *THEN* and *NOW* pills in the bottom corners.
- **The card** (`compare_card`): *CHANGES* and *n layers · n pages · n more*; per page a heading
  (file glyph, name, row count, dimmed when the page is not in the open document); each row a
  16pt status tag, the dim trail then the name (*· n inside* for an added or removed layer with
  contents), up to four value lines *what before → after* (before struck), then *and n more
  changes*; a folded layer's changes are among them as *Child · what* (*Child added* / *removed*).
  A row with folded layers has a caret at its right end (`Compare::expanded`): expanded, the row
  shows all its own values, then each folded layer apart — a 12pt status tag and its name, and all
  its values indented 17pt under it. A click on the caret expands or folds; a click elsewhere on a
  row makes it current and steps to it.
- **Private mode** masks the names in the card and in the marks' tags through `private::layer` and
  `private::page` and the bar's (*a version*), drops the trail, and replaces the value lines with
  *n changes*.

## Traps

- The session's refusal in `commit_inner`, `undo` and `redo` looks redundant beside `admits`, but
  panels and canvas paths commit without going through `dispatch`.
  `a_compare_refuses_every_edit_and_escape_leaves_it` pins the commit refusal.
- `compare_bar` runs after the canvas in `OndinApp::ui`, and the reveal with it: before it, a page's
  first-visit framing lands after the reveal and the change is out of view.
  `stepping_switches_page_and_reveals_the_change` pins the reveal.
