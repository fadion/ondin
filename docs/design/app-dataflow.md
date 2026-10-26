# App data flow: the one-way valve (§9.3)

How an interaction becomes a document edit: gestures preview, one commit makes one undo step, and the panels read the in-progress value back through the preview. The rules below are the contract.

## 9.3 Data flow

```
input event (winit/egui)
  -> tool updates PREVIEW state (no Document change)          # during gesture
  -> renderer draws Document + RenderOverrides(preview)
  -> on gesture end: tool builds ONE Transaction (ids minted via session IdSource)
  -> op queue -> Document::apply -> ApplyOutcome{inverse, dirty}
  -> History.push(inverse); Resolved.update(dirty)
  -> next frame renders committed state, preview cleared
```

### One gesture, one undo step

- One gesture = one transaction = one undo step (invariant 5). A drag never floods history.
- **Commit loops are not allowed.** A command that touches several things builds one transaction against a document advanced as each part is built (`app::ungroup_selection`, `app::drop_images_at`), because both read structure back from the tree. `commit_run` cannot fix this: a run merges only if every operation `overwrites()`, which is `None` for `Reparent`, `DeleteNode`, `CreateNode` (§5.8). Folding belongs in the builder.
- **An interaction that changed nothing is no undo step.** `commit_inner` drops an empty transaction and a no-op one (`Transaction::changes_nothing`, §5.7), asked once here rather than at the many doors. A no-op commit still clears the gesture preview (it ends a gesture).
- **Three things a per-operation guard cannot see**, each guarded where it happens:
  - *A press and release with no travel* (`canvas::gesture_travelled`, measured in world space from a stored `grab_world`; gates the commit, never the preview). Stored by `Drag::PointBox`'s `grab` and `CropPan`/`CropResize`'s `grab_world`. The crop drag genuinely sets `fit = Crop`, so `changes_nothing` is right to pass it.
  - *A transaction never built*: a zero nudge on the multi Transform card would cost a whole `tools::resize_selection` recursion; `build::local_for_world` may not return bit-identical for a zero move.
  - *A composition that returns to the start*: `layers::plan_layer_move` compares simulated child lists after the loop and clears the ops only when nothing was reparented.
- **Zero-travel releases are answered by four routes:** `Move` has its own epsilon, `Skew` is caught by emptiness (`tools::side_shear` returns `None` for zero), `Rotate` at its producer (`tools::rotate_node`), and `Resize`, `LineEnd`, `RailOffset` by `changes_nothing`. `canvas::zero_travel_release_tests::no_committing_drag_arm_spends_an_undo_step_on_a_zero_travel_release` asserts the outcome over a hand-written table (seven rows including `Drag::Spacing`); a new committing arm must add a row, and nothing forces it.

### Preview

- The preview costs O(nodes the gesture touches): `set_preview` projects the pending transaction into `RenderOverrides` rather than cloning and re-resolving. Exceptions: while a main component's layer is edited, propagation (§5.3d) runs and clones one node per read (`Document::peek`); it also runs `keep_insets` and `keep_flex_sizes` first, and `keep_insets` clones the document where the edit places a pinned layer. An edit to no main pays one scan of links (`propagate::touches_copied`).
- The preview composes over a live text session's transaction (session ops first, gesture's after; `RenderOverrides::absorb` is last-one-wins per field).
- **A text session's preview is its own transaction** (`EditorSession::session_preview`, set by `set_session_preview`) and outlives other gestures' previews. Both places that drop previews (a commit, a right-click cancel) go through `EditorSession::clear_gesture_preview`, the single spelling of that exemption. The gesture's half is `preview_holds_gesture`; `has_gesture_preview` reads that. All four combinations are reachable. Code that wants to ignore a preview reads past it (`committed_node`).
- The session preview is built like a gesture's, plus the already-shaped layout: `RenderOverrides::from_transaction_shaped` takes an optional `(NodeId, TextLayout)` (`from_transaction` passes `None`), because `set_kind`'s "re-shape only on change" guard cannot fire for a live session (it compares against the last commit). The layout is keyed to a `NodeId`, derived from the same `parts()` as the transaction.
- **Preview-agrees-with-commit covers the render layer only.** `RenderOverrides::from_transaction` makes a rendered preview structurally match its transaction (differential test, §6.2). Egui-painted overlays that claim to show where an edit lands must derive geometry from the same function the transaction does; nothing checks this.
- Panels read through `EditorSession::display_node`, not the `Document`, so accumulating controls show the in-progress value. Its `kind()` is override, then used, then document; every `preview_*` helper falls back to used geometry the same way. So a control computing an edit from it computes from where layout put the node: `build::keep_insets` turns the placement written for a pinned layer into insets; `build::keep_flex_sizes` turns a resize of an in-flow flex item into `flex-grow`/`flex-shrink` 0 and `flex-basis` `auto` (or `align-self: start`/`end` for one drawn stretched) and keeps the item's stored translation under a `SetTransform` written in-flow; any resize of a `%` or `fit-content` dimension becomes px on the changed axis. A text's sizing mode is read from the node at drawn extents (`tools::gesture_text_sizing`). The preview carries the same holds (`build::flex_holds`). `committed_node` has no used kind (it reports what a commit overwrites). `DisplayNode::display` and `item` have overrides behind them.
- **A field reading through the preview cannot tell "edited" from "moved this frame".** On the release frame the field shows its own preview's value, so a field that builds its edit only when the number changed that frame emits an empty transaction. Layout-card number fields use `layout::edited`, which latches "moved since engaged" through the engagement's last frame; the Transform card's W/H do too (every sizing mode). Do not compare the number to the one drawn: a clamped read-back (a label's widest word) would empty a typed edit.
- **Multi-selection Transform fields carry their own drag total:** `OndinApp::multi_transform_drag: Option<(MultiField, f64)>`; every frame (release included) builds from the same committed base. The single-layer path reads back through the render override. Rotation uses a gesture latch (`inspector::AngleScrub`) because a mirrored layer relabels the reading by 180° mid-drag. `multi_valve` differs from `edit_valve` only in clearing that total.
- **`OndinApp::shown_guide` is the same valve for guides** (a guide has no scene fragment for `RenderOverrides`). It reads the committed guide with the canvas drag (`Drag::Guide`) or inspector scrub (`OndinApp::guide_previews`, a `Vec`, one entry per guide named) applied. All drawing and hit-testing of guides goes through it. An Alt-copy drag does not apply its override (the original stays). Every previewing path owes a fork into `preview_guide` beside `set_preview` (`edit_valve`, `preview_slot`); a `SetGuideColor` handed to `RenderOverrides` alone is a no-op.
- Inspector X/Y, rotation and skew are **world-space** for every node, via `build::place_at_world`. X/Y are the box's corner, not the transform translation (a `Boolean`, pen `Path` or `Group` has a box its origin does not describe); the edit is `translate(typed − shown) · world`.
- **A frame-scoped guide's position field shows its offset from the frame**, the one exception (a guide in a rotated frame has no world coordinate: `rulers::guide_world_coord` is `None`; the ruler chip quotes the same number). `inspector::guide_position_fields` has the argument. That field carries its own absolute drag total (`app::InFlight::guide_position_drag: Option<(GuideAxis, f64)>`, keyed by axis): a drag sets every guide on that axis to the number shown, ending any disagreement (unlike `multi_number`, which nudges a layer's X relatively and keeps saying `Mixed`).

### Text editing

- Placing a text node commits `CreateNode` immediately as its own undo step; each editing session (click in, type, Escape/click out) is preview state driven by `core::text::TextEdit` and commits exactly one `SetText` transaction. Undo restores pre-session content (a fresh empty node is removed instead).
- "Changed nothing" is measured against the document as it last stood: `TextSession::original` is re-based onto the **document** (not the editor, which is a commit ahead) by `restyle_session` (via `canvas::text_session_restyled`, `…_after`, `restyle_session_with`) after the Type panel commits attributes mid-session.
- **Every exit from the editor commits the session first** (a live editor holds content nothing else does; `finish_text_edit` is the sole committer). `go_to_dashboard`, `choose_tool` and the top bar's Undo/Redo/Save call `OndinApp::finish_text_first`. Doors that open or replace a document call `may_replace_current_document` (which finishes the session): `open_path`, `begin_open`, `recover`, and the head of each kit verb that commits or saves (`accept_kit_updates`, `make_local_now`, `make_kit`, `stop_sharing_kit`, `open_publish`, `publish_now`). Seam is `dispatch`, not the keymap: finishing first is a fact about the action.

### Selection

- Selection, hover and marquee are app state derived via queries, never document state. `Selection` carries `ids: Vec<NodeId>` and `guides: Vec<GuideId>` (mutually exclusive: node mutators clear guides, `set_guide`/`toggle_guide` clear nodes), plus `key: Option<NodeId>` (a designation within the layer half). Reads: `guides()`, `has_guide(id)`, `toggle_guide(id)`, `clear_guides()` (leaves layers and key), `set_guide`.
- **Two emptiness questions:** `is_empty`/`len`/`single` answer about layers (a guide selection reads empty; use for node work); `Selection::nothing()` means no layers and no guides (use when asking if the inspector has a subject).

### The edit valve

- Inspector edits preview while a control is engaged and commit when it stops. Keyboard nudges are instant single-op transactions.
- **"Ends" is a transition.** The valve asks whether the control is *engaged* (`dragged() || has_focus()`), previews while it is, and commits on the frame that stops being true. Both halves matter (a bare `Sense::click_and_drag` region never reports focus; a field being typed in is never dragged). Never commit on `drag_stopped() || lost_focus() || changed()`: a `DragValue` reports `changed()` per keystroke and `lost_focus()` on two consecutive frames.
- **`Escape` never commits** (egui hands a typed number back rather than reverting). Escape in a focused field also reaches `OndinApp::escape` and `cancel_gesture` on the same frame.
- **A cancelled frame spends the transition:** read and write the latch before the cancel bail, or the next frame commits what the cancel threw away. Both the picker's `pointer_slot` and the valve bow out after a right-click cancel (§9.4).
- **Three valves, one rule:** `edit_valve`, `inspector::multi_valve`, `typography::char_valve` all carry the engagement latch, the `Escape` refusal and the cancel-bail ordering. The "changed on a never-engaged control" third arm is deleted (egui's per-frame clamp of an out-of-range stored value triggered it; both `DragValue` helpers opt out of that clamp).
- **The falling edge arrives one frame before the number does:** a valve committing from the falling edge must be handed the typed value by the field, not read it from the response.
- **Picker pointer targets** (`picker::pointer_slot`): a click on the saturation plane sets neither `dragged` nor `changed`, so it commits directly; a press previews through `preview_slot` before it travels. The press arm claims the frame only if `preview_slot` returned a non-empty transaction.
- **`edit_valve` reports whether it committed; no caller predicts it.** `inspector::valve_slot` retargeting a Group Colors row must use the return. The `bool` is `EditorSession::commit`'s "the commit took", true even if `changes_nothing` dropped the transaction.
- **Text fields:** a `TextEdit` ends on focus loss but commits only if the key was not `Escape`. `ui::defocus_commits(resp)` (`lost_focus() && !key_pressed(Escape)`) is the one spelling, used by the colour text fields (`colour_text_field`, `paint_hex_field`, `char_hex`), the layer-name field, stroke-dash list, Export prefix/suffix; the clear of the buffer must run on every way out. Grid-card CSS fields (`layout::grid::css_field`) compare text with the seeded text, not what is shown (a `Mixed` field shows nothing).
- **A field not drawn on the frame it loses focus commits nothing**; the inspector hold cures this for every field. **A card keeping one write per frame drops the earlier row's write** when a click on another control in the same card shares the frame: the Component card commits each row's write in turn (`own_rows`; a text row's reset wins over its typed field); the Variants and Properties cards defer writes and build them once the card is drawn (`OndinApp::run_edits`), naming a property by item id (a set's property/value, which has none, by name followed through the frame's renames).
- **Type panel axes:** the variable-axis field is valved like `axis_slider`; both read `axis_coords_after` (including `clear_at_default`).
- **The `lost_focus` gate:** `app.rs · valve_condition_gate::nothing_commits_on_the_expression_d316_removed` reads every `.rs` under `crates/`, blanks `//` comments, and fails on any `lost_focus()` with `changed()` within 160 characters. Exempt with a `valve-gate-not-a-commit` marker in a comment beside the line (not a path list), placed last in its comment block. It does not cover a bare `lost_focus()` commit (e.g. the picker's `colour_text_field`); an `Escape` gate would need ~8 markers across 6 files and is not built.

### Colour picker

It keeps no colour of its own: it reads its target brush through `display_node` each frame and writes each edit to the slot. A layer's gradient is edited as drawn: `OndinApp::slot_brush` maps a Fill or Stroke brush onto the box layout gives the layer (`paint_box_map`, §6.2) and `slot_transaction` maps it back (`stored_on_box`, which keeps the held transform exactly when the edit left it alone, so a colour edit leaves no rounding in it). It keeps only intent the document cannot hold: selected gradient stop, the hue behind a grey, a half-typed colour. Its window opens every time beside the inspector, `app::POPOVER_GAP` from the cards (or from an open popover, `picker_lane_right`) as a popover is, level with the field it edits (`Picker::spawn`; `placed` forces `current_pos` on the first frame; `default_size` height zero so the first-frame estimate is not clamped). A picker requested by a header `+` has no row yet: parked in `OndinApp::pending_picker` and claimed by the created row next frame. Retargeting keeps its position.

**The faces.** The tab strip is *Solid* · *Gradient* · *Variables* (the last only where the slot binds;
*Gradient* disabled where it cannot hold one). *Gradient* from a solid or a binding makes a linear.
Each gradient face opens with its geometry row, whose first control is the 34px preview chip
(`picker::kind_chip`, a caret badge on its corner): a click opens a menu of `PaintKind::GRADIENTS`,
and picking one converts the paint, stops kept and geometry reframed on the box. The rest of the row:

| Kind | Row after the chip |
| --- | --- |
| Linear | angle field, *Flip 180°*, *Reverse the stops* |
| Radial | CX and CY fields |
| Angular (`PaintKind::Sweep`) | start-angle field, *Reverse the stops* |

Ends, centres and radii are dragged on the canvas, not in the picker (`tools.md`, gradient handles).
The stop bar follows the row; the selected stop (`Picker::stop`) is shared with the canvas's dots.

**The value row** (`hex_row`): the pick button, the format chooser, the colour in that format, the
opacity.

- **Formats** (`picker::Format`): Hex, RGB, HSL, HSB, one choice for the whole app
  (`Prefs::colour_format`, a string key; an unknown key reads as Hex). The chooser
  (`format_chooser`) is a 44pt button over an `egui::Popup::menu` whose rows show the current colour
  in each format. The inspector's paint rows stay hex.
- **Hex is a text field** (`colour_text_field`), buffered and committed on defocus. **RGB, HSL and
  HSB are three scrub fields** (`channel_fields`: `DragValue`s through `ui::bare_drag_value_parsed`,
  `update_while_editing(false)`), previewing through `valve_slot` like the panel's numbers, one undo
  step per edit. HSB is the plane's own HSV and HSL is computed from it, both through the picker's
  `intent` (`channel_brush`), so B to 0 and back keeps the hue. A whole colour typed or pasted into
  one channel field sets the whole colour; a hash-less hex of six or eight digits is a colour
  before it is a number, one of three or four only past the channel's max.
- **Over a *Mixed* row** (`PaintSlot::SelectionAll`) a field format falls back to the text field,
  reading *Mixed*: layers that disagree have no one channel to show.

The pick button (`Picker::picking`) is the eyedropper's door into this one slot (§9.4). Only the
Solid and gradient faces draw the row, so `picker_body` disarms the button whenever the Variables
face or an Image brush is showing: a face with no button cannot stay armed. While it is armed the
picker takes no click, only picks (§9.4), and the hovered colour stands in for the edited stop's in
the plane, hue and value row (`Picker::preview`), for the eye only: nothing previews on the canvas,
because the next sample would read it. Hex shows a dim *was XXXXXX* beside the field; a channel
format tints its fields accent and shows the old colour as a chip in the chooser. A pick writes
through `OndinApp::write_slot_rgb`, the typed colour's door: the RGB is set and that paint's or
stop's alpha kept (a colour typed with an alpha sends it, and every paint in scope takes it); over a
*Mixed* row each paint otherwise keeps its own opacity (§15 D540). One transaction.

**Colour text** (`colour_text`: `parse`, `matches`, `refusal`, `Typed`) is the one parser behind
every colour field — the picker's text and channel fields, the inspector's paint rows
(`inspector::paint_hex_field`), the Type panel's colour field and variable cells
(`variables::parse_colour`) — so typing or pasting a colour from CSS or another tool needs no
conversion to hex first.

- It reads hex (`RGB`, `RGBA`, `RRGGBB`, `RRGGBBAA`, `#` optional), `rgb()`/`rgba()` and
  `hsl()`/`hsla()` in comma or space syntax with `/ alpha`, percentages, hue in deg/rad/grad/turn,
  and the CSS named colours plus `transparent`, in any case. Channels out of range clamp, as CSS does.
- Bytes go through `Color::from_rgba8`'s arithmetic, so a hex stores the float it always did.
- `Typed::alpha` is `Some` only when the text spelled an alpha. A typed alpha sets the alpha; without
  one a paint keeps its own and a variable cell is opaque.
- `matches` compares as bytes, the alpha only when typed: committing an untouched field over an
  off-lattice colour writes nothing.
- Committed text that does not parse writes nothing and puts *Not a colour: <text>* in the floating
  status (`refusal`, quiet for empty text, a `--name` and *Mixed*). A variable cell refuses with its
  own sentence instead.
- `ui::COLOUR_CHAR_LIMIT` (64) is room for any pasted CSS colour: a field too short for the paste
  refuses it whole.

**Recent colours** (`Prefs::recent_colours`, `OndinApp::remember_colour`): the last six distinct
colours, RGB only, compared as bytes, most recent first. They are the person's, not the document's,
app-wide and kept across launches, so the colour just used is one click away in the next file.

- Recorded when a colour is committed, never during a drag (each call rewrites the preferences
  file), and only what changed it: a click or a drag's release on the plane or a strip
  (`write_stop_colour`, not a gradient handle's drag), a field edit, a swatch click, `write_slot_rgb` (a typed colour, a picker's eyedropper), the eyedropper's selection
  pick, the inspector's paint rows and the Type panel's colour field.
- The Solid face's swatches are two rows of six (`document_swatches`): the recent colours (clock
  glyph) above the document's own (`icon::FILE`, `document_palette`, most-used first). An empty place
  draws a faint outline, so the face does not grow the first time a colour is committed. A swatch
  sets the colour and the paint keeps its alpha.
- **Every face is one height**, so nothing jumps under the pointer: the gradient faces' plane is 96pt
  against the Solid face's 104, because their geometry row and stop bar take the room of the alpha
  slider and swatches; the Variables face takes the Solid face's measured height (a 226pt list
  until measured). `HEIGHT_HINT` (320), roughly the tallest face, places the window clear of the
  screen edge. Changing one face's rows means re-balancing the others.

### Input routing (`input.rs`)

- Every keystroke resolves against an explicit `Mode` (normal / text-insert; command, §13, is a third arm) and against whether a chrome widget owns the keyboard. No key handling happens elsewhere, so the keymap is one function.
- **A modal owns the keyboard and the pointer.** While Settings, *Unsaved changes* or crash recovery is up, `input::resolve` is not called (a card of buttons focuses nothing). Dropped files are filled before widgets run, so `take_dropped_images` is gated too. The shared condition is `OndinApp::modal_is_up`.
- Outside the keymap: `canvas::text_mode_input` reads raw events for a session's editing keys (chars, Escape, Enter, Backspace, arrows, `Tab`, the clipboard trio gated on Alt and Shift up).
- `TextInsert` is near-exclusive: no bare key resolves to an `Action` there, but modified chords do: `Action::TextStyle`, and `Undo`, `Redo`, `Save`, `Open`. `Tab` also resolves to `Action::StepPoint`; the modes keep them apart. Focus half of the guard still applies.
- The top bar does not consult `Mode`; its Undo/Redo/Save call `finish_text_first`.
- **Two actions from one paste:** `Event::Paste` gives `Action::Paste`; the `Ctrl`-without-Alt `V` release gives `Action::PasteRelease` (acts only when the clipboard holds no text). `Ctrl+Alt+X` boolean and `Ctrl+Shift+C` (centre-align) are read from the release; the stolen press still arrives as `Event::Copy`/cut and the loop must decline it.
- **egui's keyboard zoom is off** (`theme::install` sets `zoom_with_keyboard = false`): the app owns `Action::ZoomIn/Out`. Check `egui::Options` before binding any chord egui also reads.

## Traps

- Two defences recur: preview guards at both the commit door and the producer. Removing one often leaves tests green; flip them together.
- A guard on `lost_focus()` alone reads `Escape` as confirm.
- A predicate that predicts what another function will do is a copy of it; ask the function.
- Per-operation no-op guards cannot see gesture-level no-ops (see above); each is guarded where it arises.
- `changes_nothing` returning false for a transaction whose only effect is a mode change (crop click) is correct; travel decides.
