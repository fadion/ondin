# Ondin — keyboard shortcuts

The whole keymap: ✅ bound, ➕ agreed and unbuilt, ⛔ deliberately unbound. The code wins:
`crates/ondin-app/src/input.rs` (`resolve`, `normal_mode`, `text_insert_mode`,
`ViewSwitch::accel`) for the editor, `panels/dashboard.rs` (`dashboard_keys`, `search_field`) for
the library.

**Unbuilt:** nothing. The cheatsheet (`Ctrl+Shift+/`, `app-canvas-chrome.md`)
renders the editor's bindings from the command registry, plus hand-kept Mouse and Library
sections. Its tests check every registry chord it shows against `input::resolve`, and every
action `input::resolve` binds against the sheet; the hand-kept sections and this file are not
checked. Not kept on the design canvas: its artboards are
mockups, not specs.

**Sources.** Figma and Sketch agree with each other far more than with Adobe, so take their
binding where they disagree with Adobe about a concept both have, and Adobe's where Figma has no
answer (grid, guides, snapping — §6). Three collisions are unresolvable (bottom).

---

## 0. How egui delivers a key is not how you spell it

`egui_winit` builds a key from `logical_key.or(physical_key)` — **the logical key wins**. A
shifted punctuation key arrives as the `egui::Key` variant for the character it produces, not as
the unshifted key plus Shift. `egui::Key` has variants for `!` `?` `:` `|` `{` `}` and none for
`@` `)` `"` `<` `>`:

| Chord | Character | Arrives as | Action |
|---|---|---|---|
| `Shift+1` | `!` | `Key::Exclamationmark` | accept both — ✅ (§3) |
| `Ctrl+Shift+;` | `:` | `Key::Colon` | accept both — ✅ (§6) |
| `Ctrl+Shift+\` | `\|` | `Key::Pipe` | accept both — ✅ (§6) |
| `Ctrl+Shift+/` | `?` | `Key::Questionmark` | accept both — ✅ (§8) |
| `Ctrl+Shift+[` `]` | `{` `}` | `Key::Open/CloseCurlyBracket` | ✅ |
| `Ctrl+Shift+=` | `+` | `Key::Plus` | ✅ the *primary* zoom-in spelling (§3) |
| `Shift+2`, `Shift+0` | `@`, `)` | no variant → physical `Num2`/`Num0` | nothing (§3) |
| `Ctrl+Shift+'` | `"` | no variant → physical `Quote` | nothing |
| `Ctrl+Shift+,` `.` | `<` `>` | no variant → physical `Comma`/`Period` | nothing (§10) |

**A binding naming only the unshifted key in the left column reads perfectly and fires never.**
Model: accept the shifted variant unconditionally (it cannot be produced without Shift) and read
the plain spelling against the modifier. Accepting both is right on every layout (a UK `Shift+2`
is `"`, also variantless).

### The clipboard trio is intercepted upstream and ignores Alt and Shift

`is_cut_command` / `is_copy_command` / `is_paste_command` test `command && Key::X|C|V` and the
caller returns, so those keys never arrive as presses while Ctrl is down. **Every
clipboard-adjacent binding resolves from `Event::Cut`/`Copy`/`Paste` plus that frame's
`i.modifiers`**: `Ctrl+Shift+V`, `Ctrl+Alt+C`, and the paste-properties and Exclude chords below.

**The key coming back up survives**: the guards sit inside `if pressed`, so the release arrives
as an ordinary `Key { pressed: false }`. That is how Exclude (`Ctrl+Alt+X`), paste-properties
(`Ctrl+Alt+V`), an image paste (§15 D183) and text centre-align (`Ctrl+Shift+C`, §10) are read.

⚠️ **A binding that reads both spellings fires twice, a frame apart** (§15 D219). `Ctrl+V` over
text emits the event on the press frame and the release later; a same-frame guard cannot see
that. `Paste` and `PasteRelease` are two `Action`s, and the release acts only when
`system_clipboard_text()` is `None` — the complement of the condition that makes the event
exist. A flag is not a substitute: the press is swallowed, so `key_down` is false on the frame
the event arrives.

---

## Live bugs this spec found — both fixed

The two worked examples of §0 (`decisions.md` §15 D204, D205).

**L1 — `Shift+1` (zoom to fit) never fired.** The key arrives as `Exclamationmark`; egui's
`key_pressed` never consults `physical_key`. `normal_mode` accepts `Exclamationmark` and `Num1`.
`shift_1_zooms_to_fit_and_arrives_as_an_exclamation_mark` pins our half; the backend→egui half is
unverifiable headlessly. `Ctrl+1` was never broken, so the dead line read as one binding written
twice.

**L2 — `Ctrl+Alt+X` cut instead of excluding.** `is_cut_command` fires on it, so `Event::Cut`
became `Action::Cut` with no modifier check. Fixed twice: the clipboard event arms test Alt (§7),
and `Boolean(Exclude)` resolves from `key_released(Key::X)`. `Ctrl+Alt+U/S/I` were unaffected.
`save_and_cut_are_not_booleans` and `the_four_boolean_chords_resolve` missed it by pushing a
synthetic `Event::Key`; they drive the real signal now, and `ctrl_alt_x_excludes_and_does_not_cut`
pins the pair. **A test of a Ctrl+letter chord where the letter is C, X or V must drive the event
or the release, not the press.**

---

## 1. Tools

Single key, `plain && !shift`, Normal mode only.

| Key | Tool | |
|---|---|---|
| `V` | Select | ✅ Figma, Sketch, Illustrator |
| `H` | Hand | ✅ Figma |
| `K` | Scale | ✅ Figma |
| `F` | Frame | ✅ Figma (also `A`; taken here) |
| `R` | Rect | ✅ Figma, Sketch |
| `O` | Ellipse | ✅ **primary** — Figma, Sketch |
| `E` | Ellipse | ✅ alias; the rail tooltip shows `O` only |
| `G` | Polygon | ✅ no convention |
| `S` | Star | ✅ |
| `L` | Line | ✅ Figma, Sketch |
| `P` | Pen — **or arms the pen bias** inside an active point edit | ✅ (§15 D125) |
| `A` | Node | ✅ Illustrator/Sketch direct-selection letter (Figma's `A` is Frame). **No lock check in the key's arm, and none wanted**: a locked layer can join the selection from the layers panel, so the refusal is in `canvas::edited_path` — the tool arms and finds no subject, as `A` over a rectangle does (§15 D321) |
| `T` | Text — **click** plants an auto-width node, **drag** draws a fixed box | ✅ (§15 D170) |
| `I` | Eyedropper — one shot; a second `I` hands back to the tool it was armed from (`choose_tool`) | ✅ Figma |

**`C` is unbound** (§15 D268): Crop's letter until image editing replaced the crop tool, which has
neither rail slot nor letter. It is the letter a comment tool would want.

## 2. Opacity

| Key | Action |
|---|---|
| `1`–`9` | ✅ opacity 10%–90% |
| `0` | ✅ 100% |
| two digits within **600 ms** | ✅ that exact percentage (`4`,`5` → 45%) |
| `0`,`0` within 600 ms | ✅ 0% |

Plain only; numpad digits work (`Key::Num0`–`Num9`). Applies via `build::set_opacity_all`; no-op on
an empty selection.

- **The window needs a repaint to close**: the pending first digit schedules
  `request_repaint_after(remaining)`, folded into `<OndinApp as eframe::App>::ui` through
  `fold_opacity_entry` (§15 D698).
- **The first digit is a preview, not a commit**, so `4`,`5` leaves one history step reading 45%.
  The window shutting commits — by a second digit or the deadline (`OPACITY_DIGIT_WINDOW`).
  `opacity_window_open` is shared by `opacity_step` (pairing) and the deadline so they cannot
  drift and spend one keystroke twice.
- ⚠️ **A third way the window shuts commits nothing** (§15 D535): `Escape` or a right-click, via
  `OndinApp::cancel_gesture`, which drops `opacity_entry` along with the preview. A cancelled
  gesture owes zero transactions (§9.3).
- ⚠️ **The pending digit carries the layers it was typed against** (§15 D470): `4` on A, click B,
  and the deadline used to apply 40% to B. `4` on A then `5` on B are two values.
- ⚠️ **Reserve `Shift+3`…`Shift+9`** for blend modes (Adobe); `Shift+0/1/2` are zoom (§3).

## 3. Zoom

| Key | Action | |
|---|---|---|
| `Ctrl` `+` / `=` | zoom in | ✅ both spellings |
| `Ctrl` `−` | zoom out | ✅ |
| `Ctrl+0` | 100% | ✅ Sketch |
| `Ctrl+1` / `Shift+1` | fit the current page | ✅ Sketch / Figma (§L1) |
| `Shift+0` | 100% | ✅ Figma |
| `Ctrl+2` / `Shift+2` | **zoom to selection** | ✅ Sketch / Figma |

`ZoomSelection` exists because `ZoomFit` used to fall back to the selection, making "fit
everything" unreachable while anything was selected. `document_bounds` and `selection_bounds` are
separate; the zoom dropdown has a *Fit selection* row; `reveal_selection` reads
`selection_bounds`. `ZoomIn` is the one arm that must not exclude Shift (`Plus` cannot be made
without it); the `!m.shift` sits on `Equals` only.

⚠️ Adobe's `Ctrl+0`/`Ctrl+1` are the reverse (see *Unresolvable*).

## 4. Layer state

| Key | Action | |
|---|---|---|
| `Ctrl+Shift+H` | hide / show | ✅ Figma, Sketch |
| `Ctrl+Shift+L` | lock / unlock | ✅ Figma, Sketch |
| `Ctrl+R` · `F2` | **rename** | ✅ Figma · Windows alias |
| `Shift+H` / `Shift+V` | flip horizontal / vertical | ✅ Figma |

**Hide and lock are one switch for the whole selection, and a mixed selection hides / locks**:
*Hide* is a verb, not a toggle. The rejected argument, which returns: a hidden or locked layer is
unpickable (`query.rs` hit test skips `!visible() || locked()`), so hiding a mixed selection puts
layers out of the canvas's reach, one stray click from needing the layers panel. Rejected anyway:
normalising away from the verb is the stranger surprise, and `Ctrl+Z` answers a mis-click.
Both commit through `session.commit`, not `commit_edit`, whose one extra (arming the inspector's
selection-chrome hide) a visibility switch has nothing to look at.

Rename opens the same `renaming_layer` field as a layers-panel double-click, showing the panel if
hidden and unfolding its Layers section. It **refuses a multi-selection** and **refuses in focus mode** (no panels are drawn, so
it would arm against a field that never renders).

**`Ctrl+R` is rename, not rulers** (Illustrator/Photoshop). Rulers keep `Shift+R` and `Ctrl+R` is
not aliased to them. It means the same on the library screen (§8a): a chord spent on rename
everywhere beats one spent in the editor only. `Shift+H`/`V` are safe because the tool block is
gated on `!m.shift` (`shift_r_toggles_rulers_without_stealing_the_rect_tool` is the pattern).

## 5. Align and distribute

| Key | Action | |
|---|---|---|
| `Alt+A` / `Alt+D` | align left / right | ✅ Figma |
| `Alt+W` / `Alt+S` | align top / bottom | ✅ Figma |
| `Alt+H` / `Alt+V` | align centres horizontally / vertically | ✅ Figma |
| `Alt+Shift+H` / `V` | distribute horizontally / vertically | ✅ **invented** |
| `Alt+Shift+T` | tidy up | ✅ **invented** — Figma's `Ctrl+Alt+T` is the boolean namespace here |

Figma's distributes (`Ctrl+Alt+H`/`V`) collide with paste-properties (§7), and `Ctrl+Alt` is this
keymap's **boolean namespace**. Illustrator has no align shortcuts. `Alt+S` does not collide with
`Ctrl+Alt+S` (subtract): the block is `!cmd`. `build::align`/`distribute` honour the key layer when
one is designated (§15 D113).

## 6. View, grid, guides and snapping — Adobe's set

| Key | Action | |
|---|---|---|
| `Shift+R` | rulers | ✅ Figma |
| `Ctrl+'` | show grid | ✅ Illustrator, Photoshop |
| — | show layout grid | **unbound** (§15 D755) |
| — | outline mode | **unbound**: Figma's and Illustrator's `Ctrl+Y` is redo, and `Ctrl+Shift+Y` stays unspent beside it |
| `Ctrl+Shift+'` | snap to grid | ✅ Illustrator's `Ctrl+"` |
| `Ctrl+U` | snap to shapes | ✅ Illustrator's Smart Guides |
| `Ctrl+;` | show / hide guides | ✅ Illustrator |
| `Ctrl+Shift+;` | snap to guides | ✅ both spellings |
| — | snap to baselines | **unbound** (§15 D355) |
| `Ctrl+Alt+;` | lock guides | ✅ Illustrator |
| `Ctrl+\` | focus mode (hide the panels) | ✅ Figma's show/hide-UI |
| `Ctrl+Alt+\` | layers panel | ✅ |
| — | show history | **unbound**: a list few people keep open |
| `Ctrl+Shift+M` | minimap | ✅ `cmd_only && shift`; `Ctrl+Alt+M` stays the mask chord. `ctrl_shift_m_toggles_the_minimap` |
| `Ctrl+Shift+\` | toolbar | ✅ both spellings |
| — | show messages | **unbound** (§15 D1025) |

`ViewSwitch::accel` answers `None` for exactly the unbound rows (`LayoutGrid`, `Outline`, `History`,
`StatusMessages`, `SnapBaselines`): no free convention to match, a menu one click away. They are listed
so nobody calls a free chord parity. `Ctrl+\` maps to focus mode, which already is "hide the
panels, restore on leaving"; Escape still leaves it. `Ctrl+U` is underline in TextInsert (§10).
Guide lock is a workspace toggle on `OndinApp`, not a per-guide property (`architecture.md` §9.4).
`theme::install` switches `zoom_with_keyboard` off, the only egui chord that collided; check it
before adding another.

## 7. Clipboard and properties

| Key | Action | |
|---|---|---|
| `Ctrl+C` / `X` / `V` | copy / cut / paste | ✅ |
| `Ctrl+V` | text on the clipboard becomes a text layer | ✅ at the centre of the selection's frame, else the view (§15 D218, D221) |
| ~~`Ctrl+V`~~ | ~~paste at the pointer~~ | **decided non-goal** (§15 D300): the chord is position-free by choice (`paste_at` could take a point); *Paste here* is the positional door |
| `Ctrl+Shift+V` | paste in place | ✅ Illustrator (§15 D248) |
| `Ctrl+V` · `Ctrl+Shift+V` | with one unlocked frame selected, paste **inside** it | ✅ `paste_into_selected_frame`; rules in `design/app-frames-and-copies.md` |
| `Ctrl+Alt+C` | copy properties | ✅ Figma (§15 D257): fills, strokes, effects, opacity off the key layer or a lone selection |
| `Ctrl+Alt+V` | paste properties | ✅ Figma; read from the **release** |
| `Ctrl+D` | duplicate | ✅ Figma |

"Properties" are appearance only; position, size, rotation, name, pivot, lock say *which layer*,
and a corner radius lives in `NodeKind` and means nothing across kinds.

The clipboard event arms in `normal_mode` **branch on Alt** rather than refuse: Copy+Alt →
`CopyProperties`; Cut+Alt and Paste+Alt are silenced; Paste+Shift → `PasteInPlace`. Alt is tested
before Shift so a hand still on Shift cannot turn a property paste into a paste in place. Copy
and cut have **no Shift reading**: `Ctrl+Shift+C` is the text session's centre-align.
`Ctrl+Alt+X` emits nothing in the event loop; it belongs to Exclude's release arm, and emitting
there would restore L2's cut.

**`Ctrl+Alt+V` is read from the release** (§15 D257): `Event::Paste` exists only when the OS
clipboard holds text, and a property paste has nothing to do with it, so the chord would work only
when a sentence happened to be copied elsewhere. The event arm only silences the chord.
`cmd_only` excludes Alt, so the release arm cannot double `PasteRelease`.

## 8. File and discovery

| Key | Action | |
|---|---|---|
| `Ctrl+S` | **save and pin a version** | ✅ writes *and* copies into `.versions/<document id>/<unix seconds>.ondin`, keyed on the id so a rename cannot orphan history (§15 D364). Autosave runs every thirty seconds, so bare save meant nothing. The top bar's disk button is the same action |
| `Ctrl+Shift+S` | ~~save as~~ | ⛔ every document lives in the base folder (§9.5); *Duplicate* in the dashboard mints a new id. ⚠️ The `!m.shift` on `Save` is all that keeps it unbound; `save_takes_the_bare_chord_and_the_shifted_one_is_free` asserts the **empty** result |
| `Ctrl+O` | **the library** | ✅ `go_to_dashboard`, as the brand mark does. No file dialog exists |
| `Ctrl+K` / `Ctrl+P` | **search files and projects** | ✅ **dashboard only**, read in `search_field` because `input::resolve` does not run while the library is up. `Enter` takes the first document match, never a project: most recently edited, ties by name (§15 D967). **`Ctrl` without Alt and Shift** (§15 D712) |
| `Ctrl+N` | **new document** | ✅ the dashboard's *New file*, from both screens. **The project is computed at the door**: the library reads the nav being viewed, the editor the open document's project — *the project in front of you*. Unfiled at the root from *Recent*, *All files*, *Starred*, the trash, or a session not in the library (§15 D383) |
| `Ctrl+W` | **close the document** | ✅ Figma's meaning, not the window's: lands where `Ctrl+O` does (`go_to_dashboard` autosaves, drops the crash snapshot, re-scans). Another door onto one screen, differing in intent ("I am done" vs "my files"). The window-close reading (reusing `handle_close_request`'s modal) was put to the maintainer and not taken |
| `Ctrl+Shift+E` | ~~export~~ | ⛔ *Export as…* was removed when the inspector's Export panel (§15 D274) answered where, format and size from the layer. ⚠️ **Unbound but not free**: plain `Ctrl+E` is *Flatten*, so `!m.shift` on that arm is all that stands between a stray hand and a dissolved selection. `a_stray_shift_on_ctrl_e_does_nothing_rather_than_flattening` pins it |
| `Ctrl+Alt+E` | export all | ✅ not Figma's. Re-runs every layer's saved export settings into the last export folder, asking only the first time (`architecture.md` §7). Alt **and** not-Shift: `Ctrl+Alt+Shift+E` is neither this nor `Ctrl+Shift+E` |
| `Ctrl+Alt+Enter` | **present** | ✅ Figma's. `Action::Present`, normal mode only, `cmd && alt && !shift`; plain `Ctrl+Enter` stays unbound. Present mode reads only its own keys (§8b) |
| `Ctrl+,` | **settings** | ✅ VS Code's; `,` was unspent. ⚠️ **Opens, cannot close**: `resolve` is not called while a modal is up (`Action::OpenSettings` is named for that). Escape, ✕, *Cancel*, backdrop close it. `cmd_only && !m.shift` |
| `Ctrl+Shift+K` | place image | ✅ Figma's (`architecture.md` §5.5a) |
| `Ctrl+K` | **command palette** | ✅ editor only, this file's choice; the library keeps `Ctrl+K` for its file search. Opens and closes (`Action::Palette`, read by `input::discovery_chord` in both modes, so in a text session too; while open, `palette::read_keys` reads it). `Ctrl` without Alt and Shift |
| `Ctrl+/` | **command palette** | ✅ Figma's quick actions, alias; Shift up, so it never reads as `Ctrl+Shift+/` |
| `Ctrl+Shift+/` | **shortcut cheatsheet** | ✅ `Key::Questionmark` too (§0). Editor (both modes) and library (`dashboard_keys`); the chord again closes it, read in `cheatsheet_ui` because `resolve` does not run under a modal |

⚠️ `Ctrl+K` (palette) and `Ctrl+Shift+K` (place image) are adjacent and unrelated; a stray Shift
opens a file dialog, visible and undone by `Escape`. If it grates, give up `Ctrl+K`: `Ctrl+/` has
the convention behind it.

**Place image is not a tool letter.** Every §1 letter arms a persistent mode; this opens a file
dialog and *loads the cursor* with the images, one per click, returning to Select when they run
out. The rail's image button runs the same function. A click places at the fitted size; a drag
draws the box.

### 8a. The library's own keys — the dashboard only

| Key | Action | |
|---|---|---|
| `←` `→` `↑` `↓` | **move the selection** | ✅ (§15 D374). The grid steps in reading order, so `→` off a row's end is the next row's start; `↑`/`↓` move a whole row of `grid_cols` cards (§15 D964). In **list** view verticals are single rows and horizontals do nothing. Off either end **clamps**. With nothing selected, `↓`/`→` enter at the top and `↑`/`←` at the bottom (`arrow_target`) |
| `Enter` | **open the selection** | ✅ same predicate as a double click (`can_open`), ⚠️ **false in the trash**: opening there puts the editor on a file the purge deletes |
| `Delete` / `Backspace` | **delete the selection** | ✅ through the confirmation (a key can be the tail of a chord meant elsewhere). In the trash it says *Delete permanently* |
| `Escape` | **clear the selection** | ✅ the keyboard's click on the empty body. The search overlay, a file's **⋮ menu** and the **All projects ⌄** dropdown take it first (`library_menu_open`): the last two are bare `Area`s that took only an outside click, unlike `card_modal`s (§15 D578) |
| `F2` · `Ctrl+R` | **rename the selection** | ✅ (§15 D383). The ⋮'s *Rename*, pre-filled from `Entry::display_name` (stem minus a sync conflict suffix). Refused in the trash. The two keys sit on opposite sides of the `modifiers.is_none()` gate, so the verb is one function, `start_rename` |
| `Ctrl+D` | **duplicate the selection** | ✅ (§15 D383), the ⋮'s *Duplicate*: one chord, one verb, two nouns (§4). Refused in the trash |

`Ctrl+N` works here too and is §8's row, not a second one.

⚠️ **None of these goes through `input::resolve`, and none can**: `OndinApp::ui` returns before it
while the library is up, so `dashboard_keys` reads the context. Its guard (`library_keys_are_free`)
lists what owns the keyboard: the two confirmations, the ⋮ menu, the *All projects ⌄* dropdown, the
*Move to project* sheet, the *New project* and *Settings* cards, the rename field, the search
overlay, and `egui_wants_keyboard_input`. Plus `modifiers.is_none()`, which keeps `Ctrl+K` from
also stepping the selection; the three chords are read above that gate, so everything unnamed is
still refused by one line.

Unbound: `Home`/`End`, and `Ctrl+A` with anything needing a **multiple** selection — a **decided
non-goal**; `DashboardState::selected` is one path and every verb takes one document.

### 8b. Present mode's own keys

| Key | Action |
|---|---|
| `→` `↓` `Space` `Page Down` `Enter` · click | next frame |
| `←` `↑` `Page Up` `Backspace` · right-click | previous frame |
| `Home` / `End` | first / last frame |
| `Ctrl+PageDown` / `Ctrl+PageUp` | first frame of the next / previous page presented |
| `Ctrl`/`Alt`+wheel | zoom in on the frame at the cursor; back to the fit at or below it |
| wheel · `Shift`+wheel | scroll the zoomed frame vertically / horizontally |
| `Escape` | leave |

Stepping stops at either end. A step to another frame keeps the zoom and opens the frame at its
top. `present_ui` reads these and nothing else: `OndinApp::ui` returns before `input::resolve` while presenting, so no editor chord runs (`design/app-canvas-chrome.md`).

## 9. Gesture modifiers

Built: space+drag pan · `Shift` constrain / keep ratio · `Alt` duplicate-drag · `Alt` resize about
the centre · `Alt` **draw** about the press point · `Ctrl`+side-drag skew · `Shift` 15° rotate and
skew snap · `Ctrl`+click deep select · `Alt`+`Shift`+click key layer · `Ctrl`/`Alt`+wheel zoom ·
`Shift`+wheel horizontal scroll · `Shift` pen constrain · `Ctrl` pen/node curvature · `Alt` hover
measure (`measure.rs`; the overlay is in `canvas.rs`) · middle-mouse pan in **both** modes (Space
is not the alias in a text session, where it is a character) · `Alt`+click eyedropper onto the
top stroke · `Shift`+click eyedropper stays armed · `Shift`+marquee toggles what it covers ·
`Alt`+marquee removes it (layers and path points alike, read at the press).

- **`Space` held mid-draw repositions the uncommitted shape** (Figma, Adobe): `Drag::Create`
  carries the previous pointer, so the shape moves by the delta while Space was held.
- **`↑` / `↓` while dragging a polygon or star adds / removes sides** (Illustrator). The count
  lives on `Drag::Create`, starts at `POLYGON_DEFAULT_SIDES` / `STAR_DEFAULT_POINTS`, and
  `tools::step_sides` clamps it to `MIN_SIDES..=MAX_SIDES`, the inspector field's range.
- **`Alt` while drawing grows from the press point** (§15 D289); `canvas::create_box` constrains
  first and mirrors second, so `Alt`+`Shift` draws a circle around the click.
- **A plain click with the Image tool places the picture at its own size** (§15 D295): aspect kept,
  shrunk only to fit the frame.
- **A plain click with Rect / Ellipse / Polygon / Star makes one at 100 × 100**
  (`CLICK_CREATE_SIZE`, §15 D291); `Alt` centres it rather than doubling. All creation clicks go
  through the exhaustive `Tool::click_makes`.
- **A click while Space is held does nothing, for every tool** (§15 D293): Space+click is a pan
  that has not travelled; it is one binding, not a guard per arm.
- **`Alt`+`Shift` is a click on one gesture and a drag on another.** A click designates the key
  layer; a drag on the artwork is a duplicate locked to one axis, and on a handle a proportional,
  centred resize.

**The arrows had a second claimant.** `input::resolve` is pure and knows nothing of a gesture in
flight, so `dispatch` drops `Nudge` (and `SizeStep`, §15 D346) while a create drag is up; otherwise
drawing a heptagon walks the previously selected layer. The step is a parameter
(`input::NudgeStep`, §15 D330), which keeps `resolve` pure.

⚠️ `Alt`+arrows nudge in Normal mode (the arm is gated on `!cmd`, not `!alt`), so they are occupied
there; in TextInsert they are letter spacing. `Ctrl`+arrows resize (§11), from the **same table**:
the box is held by its top-left, so the arrow names the moving edge for both verbs. The resize arm
is `cmd_only`; `Ctrl`+`Alt`+arrow stays unclaimed.

## 10. Text insert mode

In `Mode::TextInsert` the canvas editor owns every bare key so the buffer cannot drift from the
document; **modified chords pass through**. `text_insert_mode` binds only modified chords
(`no_bare_key_is_a_command_in_a_text_session`).

**The clipboard trio and `Ctrl+A` are not `Action`s and never can be**: they address the
**buffer**, so they live in the editor's loop beside `Home`/`End`, gated on `!alt && !shift`. `AltGr`
is `Ctrl+Alt` on Windows and `is_cut_command` ignores Alt, so an `AltGr`+`C` spelling a character
would arrive as `Event::Copy` with the character gone. The same functions ignore Shift, so
`Ctrl+Shift+C` (centre-align) was swallowed and re-emitted as a copy that overwrote the clipboard
and toasted *"Copied text"* (§15 D217, D545). **`Ctrl+Shift+C` is read from the key release**, §0
in a place §0 did not predict; the C/X/V test rule now covers a non-clipboard binding.

| Key | Action | |
|---|---|---|
| `Tab` / `Shift+Tab` | nest / un-nest the list items touched | ✅ editor loop, not an `Action` (§15 D173) |
| `Ctrl+C` / `X` / `V` / `A` | copy / cut / paste / select all, **the buffer's** | ✅ (§15 D217) |
| `Ctrl+U` | underline on / off | ✅ |
| `Ctrl+Shift+,` `.` | font size ∓ 2 | ✅ |
| `Alt+←` `→` | letter spacing ∓ | ✅ |
| `Alt+↑` `↓` | line height ∓ | ✅ |
| `Ctrl+Shift+L` `C` `R` `J` | align left / centre / right / justify | ✅ `C` from the **release** |
| `Ctrl+Z`, `Ctrl+Shift+Z`/`Ctrl+Y`, `Ctrl+S`, `Ctrl+O` | undo, redo, save, open | ✅ since §15 D815; guards match `normal_mode` term for term; `dispatch` calls `finish_text_first` |
| `Ctrl+B` | bold on / off | ✅ `TextChord::Bold`, the family's own face (`text::bold_step`) |
| `Ctrl+I` | italic on / off | ✅ `TextChord::Italic`, the family's own face (`text::italic_step`) |

**`Ctrl+B`/`Ctrl+I` pick one of the family's own faces; they never flip a bare flag on a family that
cannot draw it.** Both go through `OndinApp::toggle_face`, as the Type panel's Bold/Italic toggles
do, and apply the result as picking a variant does (weight, italic and axis coordinates merged).
The rule is core's (`text.md` §5.4, *Bold and italic*):

- **Bold** reads as on at `text::BOLD_FROM` (600) and up, unless the selection's weights are mixed,
  which is made bold. Off steps toward 400 and on toward 700, to the nearest variant of the same
  slope in that direction, else along `wght`.
- **Italic** takes the other slope's variant nearest in weight, else the bare flag when an `ital` or
  `slnt` axis can carry it. A mixed selection is made italic; upright is always reachable.
- Each layer steps from its own weight and slope. Over a range of several weights Italic writes the
  slope alone, and over a range of several slopes Bold writes the weight alone, so neither flattens
  the other.
- **Nothing that way** refuses with a status line: *X has no bold*, *X has nothing lighter*, *X has
  no italic*. No faux bold or oblique is drawn, so a family with no slope has no italic.
- A family core has not been handed gets the bare weight or flag.

**Underline is exclusive with strikethrough**, so `Ctrl+U` on struck-through text *replaces* the
decoration — the one row whose semantics differ from every app it is borrowed from.

**`Tab` is not a keymap binding**, which keeps it clear of `Tab` stepping the point selection
(§11, §15 D123). `text_insert_mode` resolves **no bare key**; that rule, not an empty `Vec`, is
the separation. It is claimed **whether or not it moves anything**: a `Tab` reaching egui moves the
focus ring and swallows the next keystroke. `took_tab` in `<OndinApp as eframe::App>::ui` hands focus
back (§15 D698).

**Modality carries load here.** `Ctrl+U` (snap to shapes), `Ctrl+Shift+L` (lock), `Alt`+arrows
(nudge) and `Tab` mean something else in Normal mode; only `Mode` separates them
(`the_colliding_chords_mean_one_thing_per_mode`). **The arrows are harder**: the editor's loop and
the keymap read the same keys with no mutual check, so *both fire* and *neither fires* are
reachable by editing one side. `Ctrl+Alt+←` was dead that way. The editor's `arrow_moves_caret`
(`!(alt && !command)`) yields exactly what the keymap takes, and a test sweeping all eight modifier
combinations asserts the two **partition** the space.

## 11. Structure, history and navigation

| Key | Action | |
|---|---|---|
| `Ctrl+Z` | undo | ✅ a step with nothing on the current page switches to the page its layers are on and selects them (`design/app-chrome.md`, *Pages*) |
| `Ctrl+Shift+Z`, `Ctrl+Y` | redo | ✅ as undo |
| `Ctrl+A` | select all | ✅ the current page's layers |
| `Ctrl+PageDown` / `Ctrl+PageUp` | next / previous page | ✅ as a browser steps tabs; stops at either end (`Action::StepPage`). Plain `Page Down`/`Page Up` stay present mode's |
| `Ctrl+G` / `Ctrl+Shift+G` | group / ungroup | ✅ |
| `Ctrl+Alt+G` | frame selection | ✅ Figma (§15 D987); free because the group chords are `cmd_only`. `ctrl_alt_g_frames_and_leaves_the_group_chords_alone` |
| `Shift+A` | add flex layout, read from where the children sit; a loose selection is framed first | ✅ Figma's chord. Plain `A` is the Node tool, read with no Shift. `shift_a_adds_a_layout_and_leaves_a_alone` |
| `Ctrl+E` | flatten | ✅ Figma |
| `Ctrl+Alt+M` | use as mask / release | ✅ Figma (§15 D286). `M` is otherwise spent only by `Ctrl+Shift+M` (minimap, §6); `masking_is_a_chord_and_the_bare_letter_is_still_unspent` keeps plain `M` and `Ctrl+M` free |
| `Ctrl+Alt+U` `S` `I` `X` | union, subtract, intersect, exclude | ✅ Figma; `X` from the **release** (§L2) |
| `Ctrl+]` / `[` | bring forward / send backward | ✅ |
| `Ctrl+Shift+]` / `[` | bring to front / send to back | ✅ curly spellings handled |
| `Shift+D` | reverse subpath direction | ✅ (§15 D125); no app binds this |
| `Delete`, `Backspace` | delete | ✅ pen point, path points, then layers |
| arrows | nudge (1) | ✅ *Settings › Nudge › Step*, `input::NudgeStep::small`, in `prefs.json` (§15 D330); a struct because the pair crosses three seams and a swap compiles |
| `Shift`+arrows | nudge (10) | ✅ `NudgeStep::large`; need not exceed the small step |
| `Ctrl`+arrows | resize: `→` wider, `←` narrower, `↓` taller, `↑` shorter | ✅ (§15 D346); no convention behind it. Distance is the nudge step, held by the top-left corner |
| `Tab` / `Shift+Tab` | step the point selection | ✅ node tool only (§10: nests in TextInsert) |
| `Enter` | step **into** the selection, or back out | ✅ `OndinApp::enter_action`: the node tool and image editing toggle out; with one layer selected an instance with children first (§12), then text, picture, path, group/boolean, in `double_click_pick`'s order so key and double-click cannot disagree. On text with no pointer it **selects the whole string**; a group is a *depth*, so `Escape` is the way out |
| `Escape` | unwind one rung | ✅ |

`OndinApp::escape` is an `else if` chain, so the order is the whole content; re-derive it from the
code rather than editing this sentence:

focus mode → a bindable field's menu / `--` list / mode menu → a style card's menu, picker, name
or edit (`escape_styles`) → an armed eyedropper (`disarm_dropper`: a picker's pick button, else the
rail tool, which hands back to the tool it was armed from) → the detached colour picker → the
insert picker (`kit_picker`) → the
variables browser (inner rungs first) → a text session → the pen → **a gesture in flight**
(`cancel_gesture`) → **a chrome field that just lost focus** (`chrome_focus`) → the path bar's
Offset card (`offset_card`) → pen bias armed over an edit → the point selection → the pivot handle → the entered group (selects it) → a tool that is
not Select (to Select) → **the layer selection**, the fall-through `else`.

🚨 A prose ordering is the least checkable thing this project writes down: an earlier list omitted
rungs and put the layer selection above the tool, and `context-menus.md` §10 had focus mode and
the entered group on the wrong side.

⚠️ **The chrome-field rung sits below the gesture** (§15 D821): a valved numeric field's cancel must
keep reaching `cancel_gesture`, which stops the release committing (§15 D317); the rung is taken
only when nothing was in flight — a plain `TextEdit`, where renaming a layer and pressing `Escape`
used to clear the selection too. `Enter` is the ladder's counterpart, not its alias, and
**asymmetric** (§15 D228): it toggles the two tools it can enter and only goes **deeper** into a
group; coming back out is the ladder's job.

**Anything floating takes the press before the ladder and takes only one thing** (§15 D527, D801):
a context menu, a top-bar dropdown (`Action::Escape if open_menu != None`), a modal, the detached
colour picker and the five inspector/Export popovers close on `Escape` and pay out no rung —
dismissing something opened by accident must not deselect or drop the tool. 🚨 **Only a popover on
screen takes the press** (§15 D847): a flag outlives its popover when the panel drawing it is
skipped (non-text selection, focus mode), and `Escape` was swallowed for the session; such a flag
is cleared by the press, which then pays out its rung. ⚠️ **Image editing's card is deliberately
not in that set**: it is a tool, and leaving it is the ladder's last rung.

**Image editing's only keyboard door is `Enter`** (§15 D268); `Escape` or a second `Enter` leaves,
as does the **Done** pill (`choose_tool`, §15 D270). **All three `Enter` targets refuse on a locked
layer** (§15 D320, D321) through `query::is_effectively_locked`, so a locked *group* shuts the door
on its contents: `OndinApp::begin_image_edit`, `canvas::begin_edit_text` and the `Path` arm of
`enter_action` each refuse, so chord and menu row agree. `A` arms with no check and is shut in
`canvas::edited_path`.

⚠️ `Delete`/`Backspace` have **no modifier gate** (`normal_mode`, the selection block), so
`Ctrl+Backspace` and `Alt+Delete` also delete; know this before binding a chord ending in either.
On Windows `Shift+Delete` arrives as `Event::Cut`, never as a key.

## 12. Components

| Key | Action | |
|---|---|---|
| `Ctrl+Alt+K` | create component | ✅ Figma's (`Action::CreateComponent`). A test asserts `Ctrl+Shift+K` still places an image; the dashboard's is inert; plain `K` is Scale |
| `Ctrl+Alt+B` | detach instance | ✅ Figma's (`Action::DetachInstance`) |
| `Enter` | step **into** an instance | ✅ §11's `Enter` with instances as a target (§15 D981, D985): an instance root with children is entered **first**, ahead of its text, picture and path arms, selecting its topmost layer. A double-click on an instance's background enters it too |

⚠️ **`Ctrl+Alt` is `AltGr` on many European layouts**, which §10 meets from the other side. A
`Ctrl+Shift` fallback does not work (`Ctrl+Shift+K` is place image), and `Ctrl+Alt` already carries
mask, booleans, properties clipboard, frame selection, export all and `Ctrl+Alt+P`. This is an
**open question about the whole keymap**; the file has no section for open questions, so it sits
here.

## 13. Kits

| Key | Action | |
|---|---|---|
| `Shift+I` | open the insert picker | ✅ `Action::InsertPicker` (§15 D1014): the floating picker at the foot of the vertical toolbar, toggled with the rail's hexagon. Plain `I` is the eyedropper (§3); `Ctrl+Alt+I` is Intersect. ⚠️ **It closes the picker only while the search field does not hold the keyboard**: the field takes it once on opening and a focused widget silences the keymap, so `Shift+I` right after opening types a capital *I*. `Escape` and the button close it either way (`toggle_kit_picker`) |
| `Ctrl+Alt+P` | publish kit… | ✅ `Action::PublishKit` (`cmd && alt && !shift && P`, §15 D1015). In a kit only; otherwise the status line says why. `the_kit_chords_land_and_leave_their_neighbours_alone`. ⚠️ The dashboard's `Ctrl+Alt+P` is deliberately inert (§15 D712) |

The command palette also lists both (*Insert*, *Publish kit…*). `Ctrl+Alt+P` is one more chord for
§12's `AltGr` question.

## 14. Styles and variables

| Key | Action | |
|---|---|---|
| `Shift+S` | open the styles and variables browser | ✅ `Action::VariablesBrowser` (`plain && m.shift && S`, §15 D1040), beside `Shift+I`: both panels swap in place at the foot of the vertical toolbar, one at a time; also toggled by the rail's swatches button. Free: §1's `S` is read with no Shift; `Alt+S` and `Ctrl+Alt+S` need Alt; `Ctrl+Shift+S` is inert. `shift_s_opens_the_variables_browser_and_leaves_s_alone`. ⚠️ `Shift+I`'s caveat holds: right after opening, the chord types a capital *S* in the search |

---

## Deliberately unbound

- **Select same… and Select inverse** — no convention; built as palette verbs (*Select same fill /
  stroke / font / effects*), since the palette can enumerate *which* "same".
- **Paste to replace** — a layer-menu row (and so a palette row), no chord. `Ctrl+Shift+R` is not spent on it: `Ctrl+R` is
  rename.
- **Outline shape** — built, in the context menu (§15 D230), no chord: a one-way structural
  conversion no reference app binds. `Ctrl+E` is *Flatten*, a different verb, and must not cover
  this one by having the only key. The third reason an `Item` is not an `Action`: "no key was spent".
- **Outline stroke** and **Join paths** — layer-menu rows (and so palette rows), no chord, as
  *Outline shape*.
- **Numeric distribute spacing** — a panel affordance.
- **`Tab` to hide panels** (Adobe) — `Tab` steps the point selection and `Ctrl+\` covers the intent.
  `Tab` is egui's focus key and costs a frame to claim (§15 D123): the last key worth a second
  meaning. The TextInsert nesting meaning (§10) is affordable because the two live in different
  `Mode`s; a global toggle would have to out-rank both.
- **`Shift`+digits 3–9** — held for blend modes (§2).
- **`Ctrl+Shift`+ the key of a bound `Ctrl` chord** — never a second spelling, guarded since
  §15 D710. **A chord that does nothing is the honest state for one whose action was removed or
  never bound**; it leaves the combination available. Eight `normal_mode` arms answered the shifted
  spelling silently (`Ctrl+Shift`+ `O`, `A`, `D`, `Y`, `−`, `0`, `1`, `2`); `Ctrl+Shift+D` was a
  document edit one modifier from `Shift+D`.
  `ctrl_shift_is_not_a_second_spelling_of_ctrl` asserts both directions for each.
  - ⚠️ **Four Shift readings are load-bearing; do not sweep them up.** `Ctrl+Shift+=` is the primary
    `Ctrl` `+` (§3). `Ctrl+Shift+C`/`X` must still copy and cut: the centre-align they would
    collide with is the text session's. `Ctrl+Alt+Shift+V` must stay a property paste (Alt beats
    Shift, §7). `Ctrl+Shift+Z` is Redo, so only the `Ctrl+Y` half took the guard.
  - 🚨 **The rule is kept twice: the dashboard is a second keymap** (§15 D712). `input::resolve`
    does not run there, so `search_field` once answered `Ctrl+Shift+K`, `Ctrl+Alt+K`,
    `Ctrl+Shift+P` and `Ctrl+Alt+P` with *search*, while `dashboard_keys` beside it spelled the rule
    correctly. Those four are deliberately not listed as rows. **A keymap rule stated for one
    screen holds nothing on the other** — §8a's keys are the other reader.

## Unresolvable collisions, recorded so they are not re-argued

1. **`Ctrl+0` / `Ctrl+1`.** Sketch and Figma: 100% / fit; Adobe: fit / 100%. We follow Sketch.
2. **`Ctrl+R`.** Figma: rename; Illustrator and Photoshop: rulers. We say rename; rulers have
   `Shift+R` (§4).
3. **`Shift`+digits.** Figma: zoom; Adobe: blend modes. We follow Figma for 0/1/2 and reserve 3–9
   (§2).

## Implementation notes

- **View toggles are one action**, `Action::ToggleView(ViewSwitch)`: rulers, guides, guide lock,
  grid, layout grid, layers panel, history, toolbar, messages, outline mode, focus, and the snap
  switches.
  `ViewSwitch` also drives both dropdowns' rows through one `label()`, so a row and the state it
  drives stay in step while the chord is a second caller that never sees the row.
- **Not `Action`s**: the clipboard trio and `Ctrl+A` in a text session, and `Tab` nesting a list,
  because they address the buffer. A chord that reaches the *document* is the one that wants an
  `Action`: all five text-styling chords are `Action::TextStyle`. The arrows are the seam.
- A "waiting on X" row goes stale silently, since nothing recompiles when X arrives: `Ctrl+N` and
  `Ctrl+W` sat "waiting on the dashboard" after it existed. Re-read the premise before trusting one.
