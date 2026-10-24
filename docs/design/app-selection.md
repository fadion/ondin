# Selection on the canvas and in the layers panel

Group-first picking, entering groups, the lock rule for every selecting door, double-click, key-layer
designation, the Alt measure overlay, and the layers panel's range selection and rename. Part of §9.4.

## 9.4 Selection

### Group-first (Figma semantics)

- A click selects the outermost `Group` above the hit. A frame between groups is one of the levels (a
  click on text in a card in a row selects the row; double-click steps to the card, then the text). An
  instance is a level whatever its kind (§5.3d).
- `Ctrl` reaches the leaf; `Ctrl+Alt` the immediate child of the outermost group. Over a selection
  handle band `Ctrl` means skew instead (bands are dispatched before deep-select).
- Both modifiers also **enter the group**, like a double-click. While `OndinApp::entered_group` is set,
  `group_chain` stops there. A click outside or on empty canvas clears it;
  `OndinApp::leave_a_scope_the_selection_left` (top of every editor frame, and in `escape`) clears it
  once something is selected and none of it is inside, or the group is gone. An empty selection keeps
  it. No on-screen affordance.
- `pick_for_click` opens the scope **after** asking `pick_preview` what to select.
- The policy is `canvas::pick_from_chain(ctrl, alt, chain, leaf)`, a free function shared by hover and
  click. Side effects (clearing the scope before the chain is built; opening it on a `Ctrl` reach) stay
  on the method.

### Locked layers

The canvas never selects a locked layer by a gesture that describes a **set**:

- Click: `query::hit_test` refuses it.
- Marquee: `canvas::apply_marquee` picks frame contents plus loose layers, from
  `OndinApp::marquee_candidates`, filtered by `OndinApp::may_select_in_bulk`
  (= `query::is_effectively_locked`). The visibility filter is `apply_marquee`'s own and uses the
  node's own flag: a layer inside a hidden group *can* be marqueed. The modifier held at the press
  decides what the band does on release (`preview::MarqueeMode`): none replaces the selection,
  `Shift` toggles each layer it covers, `Alt` removes them. The node tool's point marquee follows
  the same rule.
- `Ctrl+A` (`OndinApp::select_all`): same candidates (locked excluded, hidden included). With the
  candidate range empty it selects the frames themselves (`root_artboards`), filtered too. Emptiness is
  asked of the range **before** the lock filter (`marquee_range`).
- `inspector::select_by_color` (Group Colors reticle, §9.2) excludes locked layers and reports
  *"Selected 2 of 3 layers with this colour — 1 locked"*.
- The frame edge (`canvas::frame_edge_at`) checks both lock and visibility.
- **Exception**: a layers-panel row selects a locked layer (`layers::select_from_row` reads no lock);
  once selected, inspector rules apply (a lock stops dragging, not recolouring).
- **Exception**: a frame's name tag selects a locked frame (`canvas::frame_label_at` reads no lock).
  A name is not artwork, which is the panel row's argument too, and the tag's layer menu is the
  canvas's only route to *Unlock*, the only one at all in present mode.
- **A selection holding an effectively locked layer is not dragged** (`canvas::selection_locked`): it
  arms no resize, skew or rotate handle (`Handles::armed` answers `locked`, so the cursor agrees),
  no line endpoint, rail handle or pivot marker (`grab_line_end`, `grab_rail_handle`, `on_pivot`
  answer `None`), `begin_select_drag` starts no move (the press still selects), and it offers no
  spacing band (`spacing_subject` drops a locked selected container as `spacing_hover` drops a
  hovered one). The handles are still drawn; the size badge's padlock labels the refusal.
- Otherwise the lock is enforced at **selection time only** — `delete_selection`, the keyboard
  nudge, restacks and aligns do not read it — so every selecting door must carry the filter
  (`Selection::set_one` filters nothing).

### Double-click (`canvas::double_click_pick`)

- Goes one level in, **group step first**; only with no group left does it open the layer's contents:
  text → caret, picture → image editing, path → points. So the third double-click on a picture nested
  one group deep crops it.
- What counts as a double-click is `ui::double_clicked`: a primary click whose previous click was
  within egui's `max_double_click_delay` and `max_click_dist` and was not itself the second of a
  double. All six readers use it (select arm, pen finish, node tool's added point, text word select,
  layers rename, title strip).

### `Alt+Shift+click` designates the key layer

The key is what align aligns to, a boolean's base operand, and which layer becomes the mask. An
unselected layer is added first; inside an existing boolean it is a reorder to index 0. The chord
starts **no drag on the artwork** (Alt alone would duplicate on any travelled release); on the box's
own chrome it drags normally — the refusal sits inside `begin_select_drag` below the pivot, line-end
and handle arms (so `Alt+Shift` on a corner is a proportional, centred resize).

### `Alt` on a resting pointer measures (`measure.rs`, `canvas::draw_measures`)

- One thin red line per distance, ticked at the ends, number on a pill. Per axis: apart → gap between
  facing edges; overlapping → near-to-near and far-to-far. Diagonal measures meet in an L. Flush counts
  as apart; a `0` measure is dropped. Quotes **world bounding boxes**.
- Target is `canvas::hover_target`; gated on `canvas::hover_chrome_live`. A ruler guide under the
  pointer wins over the layer below it and is measured on one axis.
- Four pairings: selected layer → hovered layer / hovered guide; selected guide → hovered layer /
  hovered guide. `measure::to_guide` answers both mixed pairings. The branch is an `else` — `Selection`
  holds layers *or* guides (§9.3). Every selected guide measures. `rulers::measurable_guide` re-asks
  `guides_on()`; hidden guides are not measured to, locked ones are (`guide_under`).
- Guide to guide: `measure::between_guides` takes an `at`; `draw_measures` passes the **middle of the
  canvas** (`canvas_inner(rect).center()`), not the pointer. Perpendicular or coincident guides give
  nothing. `Axis::across()` owns the axis flip.

### Layers panel

- `Shift` takes a range along rows **as displayed** (collapsed and filtered rows excluded); anchor is
  the last non-Shift click and Shift does not move it. `Ctrl`/`Cmd` toggles a row. (Canvas Shift still
  adds/removes one layer.)
- A drag carries the whole selection (`build::outermost`) and follows the indent (move left at the
  bottom of a group to leave it, §9.4); hovering a collapsed group for a second opens it.
- The tree un-collapses and scrolls to a canvas selection; one header button collapses or expands all.
- **Rename**: double-click, `Ctrl+R`, `F2` and the context menu's *Rename* all set `renaming_layer`;
  the field requests focus itself on the first frame it draws, detected with
  `read_response(id).is_none()`. One undo step; `Escape` discards. The chord shows the layers panel if
  hidden and refuses a multi-selection.

## Traps

- Requesting focus for a widget the frame does not draw panics `accesskit_consumer`; re-requesting
  whenever unfocused fights `Enter`/`Escape`.
- A double needs both clicks delivered to a widget that asks: a press with no pointer move before it
  is nobody's click — tests must send the move.
- Asking emptiness after the lock filter makes `Ctrl+A` select frames whose locked children `Delete`
  then destroys.
- Measuring guide-to-guide at the pointer makes the label slide while the number stays still.
