# Ondin — right-click context menus

The menus as built, with the conditions that shape them. A peer of `shortcuts.md`. The code is
`crates/ondin-app/src/menu.rs` (registry, builders), `canvas::canvas_context_menu` and
`panels/layers.rs` (the doors), and `panels/binding.rs` (the inspector field menu, §6.7).

**One registry, two doors.** Every row is an `Item` whose label, glyph, accelerator and group live
once in `Item::spec`; a menu only chooses *which* rows, and `menu::build` buckets them into the
canonical groups (§3). The canvas and the layers panel open the **same** menu for a layer — same
rows, order and wording. The **target** is what is under the pointer, never what is selected (§2).
A layer menu is the kind's **head** rows, then the **invariant tail** (§4). Groups are separated by
a hairline and an empty group collapses with its separator. **Kind decides presence** (a row that
can never apply is absent); **state decides enabled** (a row that cannot run now is dimmed and its
tooltip says why). One exception: a row whose only job is to undo a non-default state is
**omitted** while that state is default (*Reset origin*, every component *Reset*/*Restore*, *Detach
variable*).

---

## 0. The interaction rules

- **R1** — The right-button press cancels a gesture in flight; the **release** opens the menu. A
  press that cancelled spends the click, so no menu opens. An armed eyedropper counts as a gesture:
  the click disarms it and opens no menu, on the canvas or off it. The menu opens at the press position.
- **R2** — A menu is claimed per region: the canvas, a layers row, the layers panel's background,
  and an inspector field (§6.7). Widgets that already use the secondary button keep it (gradient
  stop bar, ruler).
- **R3** — An open menu is `Escape`'s first rung, above present mode: one press closes only the
  menu. While open the menu holds the keyboard (`input::resolve` does not run). The same
  one-press-one-rung rule holds for the top-bar dropdowns and the inspector popovers.
- **R4** — One floating slot: opening a context menu closes the top-bar dropdowns, the colour
  picker and the five inspector popovers (type, stroke, effects, Export row settings, Export
  menu); the image-editing card is a mode and is not closed. A second right-click replaces the
  menu. The release that opens a menu is not also read as a click-away.
- A right-click inside an open menu does nothing. A left-click outside closes it and is spent —
  it does not also select (canvas, layers panel and library screen all honour this).

---

## 1. One registry, two doors

| | Canvas | Layers panel |
|---|---|---|
| Target | what a left-click there would select (§2) | the row under the pointer |
| Reaches a locked or hidden layer | no — hit testing skips both; a locked frame's name tag is the exception | yes |
| *Paste* row | **Paste here**, at the pointer | **Paste**, as a sibling above the row |
| *Rename* | opens the panel's field | yes |

---

## 2. Target resolution

- **C1** — On canvas the target is what a left-click would select: `canvas::pick_at_pointer`
  (name tag, leaf, frame edge) then `pick_for_click`'s group policy; `Ctrl` deep-selects.
- **C2** — A hit inside the selection keeps the selection whole; a hit outside replaces it with
  the hit. Same in the panel. `Shift`/`Alt` are ignored.
- **C3** — Empty canvas opens the canvas menu (§6.1) and does not clear the selection.
- **C4** — A locked layer has no canvas menu, except a locked frame through its name tag
  (`app-selection.md`, *Locked layers*). The panel menu and the tag's carry the full set, *Unlock*
  included (§5.9).
- **C5** — A frame is right-clickable where it is left-clickable: its name tag, its edge
  (`FRAME_EDGE_PICK_PX`, world AABB), or anywhere while empty. An occupied frame's interior passes
  through to its contents.
- **C6** — In the panel, the right-press commits an open rename.
- **C7** — An unlocked guide under the pointer out-ranks layers (and becomes the selection). With
  *Lock guides* on, guides are not targets.
- **C8** — A live text session opens §6.4; the node tool with a path under edit opens §6.5
  (anywhere on the canvas, checked before guides and layers).

Precedence on canvas: text session → point edit → guide → layer → empty canvas.

---

## 3. The shape of a menu

| # | Group | Contents |
|---|---|---|
| 1 | Head | the kind's own verbs (§5) |
| 2 | Clipboard | Cut · Copy · Paste here · Paste to replace · Duplicate · Delete |
| 3 | Properties | Copy properties · Paste properties |
| 4 | Structure | Group · Frame · component rows · Ungroup · mask · even-odd · booleans · Flatten · conversions · Join paths |
| 5 | Order | four restack rows |
| 6 | Transform | Flip H · Flip V · Reset origin |
| 7 | State | Hide/Show · Lock/Unlock · Rename |
| 8 | Navigate | Zoom to selection |
| 9 | Export | Copy as SVG · Copy as PNG |
| 10 | View | the page's view switches (§6.1, §6.3 only) |

- **Head at the top**: it holds the "one level in" verbs that are otherwise double-click only.
- **Promoted, not duplicated**: *Ungroup* on a group and *Flatten*/*Ungroup* on a boolean move into
  the head and leave Structure.
- **Multi-selection**: a tail row appears when any member qualifies and acts on those that do.
  Head rows: see §5.8.
- **Lock**: editing rows dim with *"This layer is locked"*, or *"A group containing this layer is
  locked"* when only an ancestor is (§5.9).
- **Length**: a primitive is 26 rows, text 29, a boolean 32, railed text 31; component roles reach
  34, a linked boolean in an instance (`the_menus_the_two_ends_do_not_count`). The ceiling is "needs no scrollbar"; if trimmed,
  Navigate goes first, then Export.

---

## 4. The invariant tail — on every layer target, canvas and panel alike

### Clipboard

| Row | Chord | Shown / enabled |
|---|---|---|
| Cut | `Ctrl+X` | dims on lock |
| Copy | `Ctrl+C` | always live |
| Paste here (canvas) / Paste (panel) | `Ctrl+V` | dims *"Nothing has been copied yet"* unless the OS clipboard has a picture, the app's own payload is still current, or the OS clipboard has text. Canvas: at the pointer; a picture is aimed like a drop |
| Paste to replace | — | dims *"No layers have been copied"* unless the app's own layers are on the clipboard and current (`Context::can_replace`); dims on lock. Each selected layer gives way to a copy of the whole clipboard, centred on it, in its slot (`app-frames-and-copies.md`). `Ctrl+Shift+R` is not spent on it: `Ctrl+R` is rename |
| Duplicate | `Ctrl+D` | dims on lock |
| Delete | `Del` | dims on lock; destructive styling |

### Properties

| Row | Chord | Shown / enabled |
|---|---|---|
| Copy properties | `Ctrl+Alt+C` | fills, strokes, effects and opacity off the key layer (or the lone layer). Dims when the source has no paint of its own (three sentences: lone layer, no key layer, key layer without paint). Live on a lock |
| Paste properties | `Ctrl+Alt+V` | dims when nothing copied; dims on lock (lock sentence wins) |

### Structure

In emitted order. Kind-gated; the component rows are gated by role (`menu::Role`).

| Row | Chord | Shown / enabled |
|---|---|---|
| Group selection | `Ctrl+G` | always, one layer included; dims on lock |
| Frame selection | `Ctrl+Alt+G` | always; dims *"A frame cannot go inside a boolean or a mask"* where `build::can_frame` refuses; dims on lock |
| Combine as variants | — | two or more mains outside any set under one parent; dims over mains of two kinds; dims on lock |
| *component rows* | | by role — table below |
| Ungroup | `Ctrl+Shift+G` | any member is a Group or Boolean, unless promoted to the head; dims on lock |
| Use as mask ✓ | `Ctrl+Alt+M` | absent on a frames-only selection and on a boolean's operand. Ticked when the layer is a mask or holds one. Dims with the inspector's refusal (`mask_refusal`), and on lock |
| Even-odd fill ✓ | — | a lone `Path` only; dims on lock |
| Union / Subtract / Intersect / Exclude | `Ctrl+Alt+U` / `S` / `I` / `X` | two or more layers, or a lone boolean (then checkable, the current op ticked); dims on lock |
| Flatten | `Ctrl+E` | two or more layers (on a lone boolean it is in the head); dims on lock |
| Convert to path | — | one text layer; dims *"This text has no glyphs to convert"*; dims on lock |
| Flip to other side ✓ | — | one text layer on a rail; ticked when flipped; dims on lock |
| Detach from path | — | one text layer on a rail; dims on lock |
| Outline shape | — | one primitive or `Path`; on a path dims *"This path is already an outline"* when it has no radii to bake; dims on lock |
| Outline stroke | — | one primitive, `Path` or boolean; dims *"This layer has no stroke"* without a visible stroke (`build::can_outline_stroke`); dims on lock. Each visible stroke becomes a filled path (`app-booleans.md`) |
| Join paths | — | exactly two `Path` layers (not line-tool lines); dims *"Both paths need an open end"* unless each has an open subpath of two or more anchors, and *"One of these paths is locked"* when either is (`join_layers_subject`); the target's own lock dims with the lock sentence. The nearest pair of open ends splices into one layer (`app-pen-and-nodes.md`) |

*Outline shape* and *Convert to path* never share a menu; neither appears beside *Flatten*.

**Component rows**, directly after *Frame selection* (`Combine as variants` leads them):

| Role | Rows, in order | Shown / enabled |
|---|---|---|
| Plain / Local | Create component (`Ctrl+Alt+K`) | withheld when the selection holds a main or a set; dims with the commit's own refusal (`create_refusal`: inside a component or instance, holds a component, wrong kind for the set, …) and on lock |
| Set | Add variant · Select all instances | *Add variant* dims on lock |
| Main | Add variant (in a set only) · Duplicate as component · Select all instances | first two dim on lock |
| Instance | Go to main component · Reset ‹property› · Reset all · Restore children · Detach instance (`Ctrl+Alt+B`) | *Reset ‹property›* only for a nested copy bound to a property that differs; *Reset all* only when drifted; *Restore children* only when its own children (collapsed row: its subtree's structure) differ; all but *Go to main* dim on lock |
| Kit instance | Go to main in ‹kit› · Accept kit updates · *resets as above* · Detach instance · Make local… | *Accept kit updates* only when a newer publish waits (omitted, not dimmed); it and *Make local…* dim on lock |
| Member (linked child) | Go to main component · Reset ‹property› · Reset ‹layer name› · Restore children | each reset only when it has something to reset; resets dim on lock |

*Go to main* and *Select all instances* change only the selection and never dim. *Create component*
and *Duplicate as component* draw the filled hexagon.

### Order

| Row | Chord | Shown / enabled |
|---|---|---|
| Bring to front | `Ctrl+Shift+]` | dims on lock; never dimmed for being already frontmost — a no-op says so on the status line |
| Bring forward | `Ctrl+]` | as above |
| Send backward | `Ctrl+[` | as above |
| Send to back | `Ctrl+Shift+[` | as above |

### Transform

| Row | Chord | Shown / enabled |
|---|---|---|
| Flip horizontal | `Shift+H` | dims on lock |
| Flip vertical | `Shift+V` | dims on lock |
| Reset origin | — | one layer whose pivot has been moved, otherwise omitted; dims on lock |

### State

| Row | Chord | Shown / enabled |
|---|---|---|
| Hide / Show | `Ctrl+Shift+H` | *Show* only when every selected layer is hidden, as the action shows only then (a mixed selection hides); never dims |
| Lock / Unlock | `Ctrl+Shift+L` | *Unlock* only when every selected layer's **own** flag is set, as the action unlocks only then; never dims |
| Rename | `Ctrl+R` (`F2`) | dims *"Pick one layer to rename it"* unless one layer, and *"Leave present mode to rename"* in present mode, where the panel that holds the field is hidden; live on a lock |

### Navigate

| Row | Chord | Shown / enabled |
|---|---|---|
| Zoom to selection | `Shift+2` | always live |

### Export

| Row | Chord | Shown / enabled |
|---|---|---|
| Copy as SVG | — | always live; the committed document, in document order |
| Copy as PNG | — | always live; `RasterOpts::default()` (1×, no background), no scale submenu |

---

## 5. The heads, per kind

Rows above the tail, in this order when several apply: the kind's rows (5.1–5.3), *Edit text* and
sizing (5.6), the picture rows (5.7), *Edit points* (5.4), then the designation row (5.3/5.8) —
the same order as `double_click_pick`.

### 5.1 Frame

| Row | Shown / enabled |
|---|---|
| Clip content ✓ | one frame; ticked from `Node::clip`; dims on lock |

### 5.2 Group

| Row | Shown / enabled |
|---|---|
| Enter group | one group; same body as `Enter` and the double-click |
| Ungroup | promoted from Structure; dims on lock |

### 5.3 Boolean

| Row | Shown / enabled |
|---|---|
| Enter boolean | one boolean |
| Flatten | promoted; dims on lock |
| Ungroup | promoted (releases the operands); dims on lock |
| Make this the base operand | on a layer inside a boolean; dims on lock |

The four operations stay in Structure as checkable rows (§4).

### 5.4 Path

| Row | Shown / enabled |
|---|---|
| Edit points | one `Path`; dims on lock (the mode refuses a locked path too) |

*Outline shape*, *Outline stroke*, *Even-odd fill* and *Join paths* reach a path from Structure (§4).

### 5.5 Primitive shapes (Rect · Ellipse · Polygon · Star · Line)

No head. The baseline menu, 26 rows.

### 5.6 Text

| Row | Shown / enabled |
|---|---|
| Edit text | every member is text; dims on lock (the session refuses too). The panel door selects the whole string |
| Auto width ✓ / Auto height ✓ / Fixed size ✓ | one text layer only; the current mode ticked; dims on lock |

*Convert to path*, *Flip to other side* and *Detach from path* are in Structure (§4).

### 5.7 Any layer with a picture in it

Gated on `tools::croppable_fill`, the predicate image editing uses; composes with 5.4–5.6.

| Row | Shown / enabled |
|---|---|
| Edit image | one layer; dims on lock (the mode refuses too) |
| Original size | one layer; resizes to the picture's own (turned) pixels; dims on lock |

### 5.8 Multi-selection

| Row | Shown / enabled |
|---|---|
| Make this the key layer | two or more selected and the right-clicked layer is not inside a boolean; designates it (align target, base operand, mask) |

Of the kind heads, only *Edit text* applies to several layers (all text); every other head row
needs a single layer.

### 5.9 Locked and hidden layers (panel only, per C4)

The whole menu is present and every editing row dims. Live over a lock: *Copy*, *Paste*, *Copy
properties*, *Unlock*, *Show*/*Hide*, *Rename*, *Zoom to selection*, *Copy as SVG/PNG*, *Enter
group/boolean*, *Make this the key layer*, *Go to main*, *Select all instances*. A layer inside a
locked group is treated as locked, with the group's sentence; *Lock/Unlock* reads the layer's own
flag. A hidden, unlocked layer gets an ordinary menu reading *Show*.

---

## 6. Targets that are not layers

### 6.1 Empty canvas

| Row | Chord | Shown / enabled |
|---|---|---|
| Paste here | `Ctrl+V` | at the pointer; dims when nothing to paste |
| Paste in place | `Ctrl+Shift+V` | dims when nothing to paste |
| Select all | `Ctrl+A` | |
| *—* | | |
| Zoom to fit | `Ctrl+1` | everything, not the selection |
| *—* | | |
| Export all | `Ctrl+Alt+E` | re-runs every saved export spec; dims *"No layer in this document has export settings"* |
| *—* | | |
| Show rulers ✓ | `Shift+R` | dims *"Present mode hides the rulers"* in present mode |
| Show guides ✓ | `Ctrl+;` | |
| Lock guides ✓ | `Ctrl+Alt+;` | the way back once guides are locked (C7) |
| Show grid ✓ | `Ctrl+'` | |
| Show layout grid ✓ | — | |

No selection rows, even while something is selected (C3). The View rows are the View menu's
switches less *Show layers*, *Show toolbar*, *Show status messages* and *Present mode*.

### 6.2 Layers panel background

| Row | Chord | Shown / enabled |
|---|---|---|
| Paste | `Ctrl+V` | at the root; dims when nothing to paste |
| Select all | `Ctrl+A` | |

### 6.3 A guide

| Row | Chord | Shown / enabled |
|---|---|---|
| Delete guide | `Del` | destructive styling |
| Clear all guides | — | destructive styling; dims when there are no guides |
| *—* | | |
| Lock guides ✓ | `Ctrl+Alt+;` | |
| Show guides ✓ | `Ctrl+;` | |

### 6.4 Inside a text session

| Row | Chord | Shown / enabled |
|---|---|---|
| Cut | `Ctrl+X` | dims *"Nothing is selected"* without a text selection |
| Copy | `Ctrl+C` | as above |
| Paste | `Ctrl+V` | inserts at the caret; dims *"The clipboard holds no text"* (OS clipboard text, read once at open) |
| Select all | `Ctrl+A` | dims *"There is no text yet"* when empty |

The rows share their `Item` with the layer verbs and act on the editor.

### 6.5 A point selection (node tool)

| Row | Chord | Shown / enabled |
|---|---|---|
| Add point here | — | inserts an anchor at the menu's open position; dims unless that position is on the path's outline (an existing anchor refuses) |
| *—* | | |
| Delete points / Delete segment | `Del` | one row, named for what the selection holds |
| *—* | | |
| Make corner | — | always offered |
| Make smooth | — | always offered |
| Reverse subpath | `Shift+D` | |
| Join | — | dims *"Select two open ends of this path to join"*; coincident ends are not merged |

### 6.6 A ruler

No menu. The ruler claims no right-click region, and the canvas menu must not be wired to it.

### 6.7 An inspector field

A separate `Area` (`OndinApp::field_menu_ui`), closed by its act, a press outside or `Escape`.
Offered on a **bindable** field: a solid paint row's hex field, a bound field's chip (not a *Mixed*
capsule), a single layer's corner radius and per-corner fields, the Container card's gap and
padding fields, the identity card's eye, W and H, opacity, a stroke's width, an effect's numbers,
and the Type card's content and family. X, Y and rotation are not bindable.

| Row | Accelerator | Shown / enabled |
|---|---|---|
| Bind to variable… | `--` (typed fields only) | opens the variable list filtered to the field's type, the current one highlighted |
| Extract into variable… | — | files a variable from the value and binds it in one step, then opens the browser on its name; disabled while bound |
| Detach variable | `Backspace` (typed fields only) | the field keeps its resolved value; omitted when not bound |

No variable is offered on a field's label badge (it scrubs).

---

## 7. Deliberately not in these menus

- **Align and distribute** — the inspector's cluster and `Alt`+letter own them; reconsider only
  if submenus exist.
- **Reveal in layers panel** — built and removed: selecting already scrolls the tree to the row.
- **Select same… and Select inverse** — palette-only verbs (`app-selection.md`).
- **Rotate 90°** — the inspector's quarter-turn buttons (`inspector::rotate_selection_quarter`); the
  palette entry is in `roadmap.md`.
- **Keep proportions** — the inspector's chain link; a modifier, not a verb.
- **Corner radius, side count, star ratio** — numbers belong to the inspector.
- **Blend modes, effects, container layout** — authored in their inspector cards.
- **Frame *Background…*** — the inspector's Fill row does it for every layer.
- **Export frame as…, Export selection… / Export as…** — the inspector's Export panel is the file
  door; *Ctrl+Shift+E* is unbound.
- **Reset crop, Replace…** — image editing's Settings tab only. **Export original…** — removed.
- **The four image fit modes** — image editing's card owns them.
- **Text on path** — the Text tool's gesture; the menu has only the way out (*Detach from path*).
- **Type controls in a text session** — the Type card is beside it.
- **Show layers, Show toolbar, Present mode, Show status messages** on the canvas menu —
  chrome-wide switches, one click away in the View menu.
- **Collapse all / Expand all** — the layers panel header has the button.
- **Guide colour and position** — the inspector, for a selected guide.
- **Submenus**, including *Copy as ▸* and a PNG scale choice; **type-ahead** in menus.
- **A ruler menu** (units are px only).

---

## 8. Chrome

The design canvas's Editor artboard's canvas menu is normative for style only; this file decides content and
order.

| | Value |
|---|---|
| Card | `ui::menu_frame`: `color::CARD`, 1pt `text_a(15)` hairline, shadow `[0,10]` blur 24 α 82, corner 5, padding 5 |
| Width | 210pt painted, border included (`MENU_W`) |
| Row | `ui::MENU_ITEM_H` 24pt, 1pt gap; 11.5pt label; hover ground `text_a(20)`; glyph `text::DIM` |
| Accelerator | right-aligned, 10.5pt, `text::FAINT`; label cut with `…` at `MENU_ACCEL_GAP` (2pt) from it, the full label in the tooltip |
| Separator | 1pt `text_a(20)`, 3pt air either side |
| Checkable row | no tick column; the glyph goes `ACCENT` when on, the label full strength on and muted off |
| Disabled row | glyph / label / accelerator inks 79 / 65 / 49; the reason in the row's tooltip |
| Destructive row | `color::DANGER` `#e0736b` on glyph and label, accelerator at 55%; neutral hover |
| Order glyphs | arrow-line-up / arrow-up / arrow-down / arrow-line-down (a deliberate departure from the export's stack glyphs) |
| Placement | `ui::menu_place`: flips up/left near an edge, clamps last; height computed, not measured |
| Keyboard | `↑`/`↓` move, `Home`/`End` jump, `Enter` runs, `Escape` closes; only enabled rows are reachable; keyboard and pointer share one highlight |

The inspector field menu (§6.7) is drawn with the same row and frame functions at 220pt wide and
26pt rows.

---

## Open

- Whether a 34-row component menu is past the no-scrollbar ceiling, and if so what is trimmed.
- Whether the canvas menu should carry *Canvas background…* (a second door to the inspector's
  no-selection field).
