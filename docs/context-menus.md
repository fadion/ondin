# Ondin — right-click context menus

**Status: specified and built, 2026-08-18.** The registry, the four rules of §0, the chrome of §8
and every row §9.1 and §9.2 could deliver are in `crates/ondin-app/src/menu.rs`; the rest of §9 is
still §9.3 and §9.4 and is listed there. **What is built is what this file says**, with four
recorded exceptions — §9.6.

A peer of `shortcuts.md`, and written to the same rules. Not in `design/` on purpose — that folder
is a regenerated design export and a hand-written spec would be lost with the next regeneration.
**The export now draws one of these menus** (`design/Editor.dc.html`, the canvas's), which changes
nothing about that division: the export is the picture, this file is the spec. §8 reads the picture
off it and says exactly how far its authority runs — style, and not content.

**Where the items come from.** Same provenance rule as the keymap: take Figma and Sketch where the
two families disagree about a concept both have, take Adobe's where Figma has no answer at all,
and record anything invented as invented. Where Affinity is cited it is because it is the only one
of the five with an answer (*Convert to Curves* on a text node, *Edit Image*).

**Every row here is scored for what it actually costs** — *wiring* (the op or builder exists and
one call site is missing), *cheap* (a small new function), *feature* (real work), *blocked* (waits
on something else). §9 is the ledger. The scoring was read off the code rather than guessed, and
two of the readings turned up live gaps that have nothing to do with menus (§9.5).

---

## 0. The collision that has to be decided first

**Right-click is currently spent entirely on cancelling the gesture in flight** — a layer being
dragged, a handle being resized, a shape being drawn, a value being scrubbed
(`<OndinApp as eframe::App>::ui`, §15 D31, D168, D698). It fires on `i.pointer.button_pressed(Secondary)`, before the UI runs, and it is a
strict no-op when nothing is in flight — which is what leaves `picker::stop_bar`'s
right-click-to-remove-a-gradient-stop alone.

Four rules resolve it. They are the whole of the interaction contract; everything after this
section is content.

**R1 — The press cancels; the *release* opens the menu; and a press that cancelled spends the
click.** `cancel_gesture` sets a flag, and the flag is cleared by the release that follows it. So
one right-click either cancels or opens a menu, never both.

The cancel has to stay on the press: §15 D168 is the record of what happens when it does not — a
right-click whose press and release fell in the same frame found that egui had already forgotten
the drag, and the reset "almost never worked". The menu has to be on the release for a different
reason, and it is the one that decides this: **a menu drawn on the press has the release land
inside itself**, on whatever row is under the pointer, and picks it. That is why Windows opens menu
*bars* on press and context menus on release. `roadmap.md` proposed "menu on press when nothing is in
flight" — this is the same decision with the flag added and the two halves of the click split.

The menu opens at the **press** position, not the release position, so a click that jitters a few
pixels does not shift the menu out from under the pointer.

**R2 — A widget that already reads the secondary button keeps it, and a menu is claimed per
region.** There is no global "any right-click opens something". **Three** regions claim a menu — the
canvas, a layers row and the layers panel's background — each through its own
`Response::secondary_clicked()`, which egui delivers on release. Everything else keeps the button it
has: the gradient stop bar removes a stop with it and no menu appears over it, and the **ruler** is now
one of those (§6.6, struck 2026-08-26 when units were decided against).

**R3 — An open menu is Escape's new first rung, it consumes the press, and it holds the
keyboard.** `OndinApp::escape` unwinds session → gesture in flight → pen bias → point selection →
layer selection → Select tool (§15 D125); a menu goes **above** all of it. ~~One thing stays above
the menu, and it is the arm that is already first: `self.present`~~ — 🚨 **nothing does, and present
mode was the thing that looked like it did** (§15 D756). A menu opens in present mode now, present
mode hiding the app's chrome while the canvas goes on working (§15 D750) and a context menu being the
canvas answering a click, so the two **can** contend and **R3 wins**: the first Escape closes the
menu and the second leaves the mode. It falls out of this rule's own arm being a `return` — `ui`
answers the key off `context_menu.is_some()` and never calls `escape`, so `self.present` being that
ladder's first rung is not reached. It is the order to want: had present won, one press would have
restored the whole chrome *underneath a menu that stayed open*. ⚠️ **§6.1 keeping *Present mode* out
of the canvas menu is a separate rule and it stands** — that is about the **row**, which is a
chrome-wide switch with the View menu one click away, and it was never a reason to refuse the whole
**menu**. One sentence used to carry both.

**The press that closes a menu closes only the menu.** Escape is a ladder and pays out one rung per
press; the rung a menu adds is not a free one. A version that closed the menu and then fell through
would deselect, or leave the group, or drop back to Select, in the same keystroke that dismissed a
menu the user opened by accident — and the state it ate is the state the menu was aimed at. One
press, one rung; the second press starts the ladder the menu was covering.

⚠️ **That is the rule for every floating thing, not for this menu, and the top-bar dropdowns were not
keeping it** (§15 D527). R4 already names them — one slot across the menus, the picker and the
inspector popovers — but R3's keyboard clause was only ever justified in the context menu's own three
regions, so the principle covered `TopMenu` while no verdict did. One `Escape` over an open Zoom, View
or Snap menu closed the menu **and** dropped the tool to Select. The fix is the mirror of this arm, a
match on `Action::Escape if open_menu != TopMenu::None` in `dispatch`; the three modals reach the same
answer by a third route (they are gated out of `input::resolve` altogether, §15 D464). ⚠️ **Write the
rule down where it is general**: three sites arrived at it by three separate fixes, and the second and
third were only found by somebody reading this paragraph and asking who else it was about.

⚠️ **And R3 was broken by a route the guard at the top of the frame cannot reach** (§15 D533). While
the effects popover survived R4's clear, one `Escape` over an open context menu closed **both** — the
inspector runs before the menu and `effect_menu_popup` reads the same unconsumed key — so the press
that closed the menu also took the popover, which is this rule failing without any second *menu* being
involved. It is closed by R4 being obeyed rather than by anything R3 does: with the popover cleared on
the open, the state cannot arise. **A rule about one press is only as good as the set of things that
can be on screen at once.**

**And the menu holds the keyboard while it is up.** `input::resolve` must not run while one is
open, for the same reason it already checks `egui_wants_keyboard_input`: with a menu open over a
layer, `Delete` would delete the layer while its own menu was offering the row.

**R4 — There is one open menu, ever, and the right-click that opens the next one is what closes
the last.** Not a dismissal rule that happens to fire first: the app holds **one** menu slot, so
the second right-click overwrites it and the first menu is gone by construction. That is the shape
`TopMenu` already has, and the reason `toggle_menu`'s comment can promise "there is no path where
two dropdowns overlap" without a single line of closing code. The context menu joins that slot
rather than sitting beside it.

**One slot across every floating thing, not just across the menus.** Opening a context menu closes
the top bar's dropdowns, the colour picker and the **five** inspector popovers — typography, stroke,
effects, and the Export card's row settings and its own menu; opening any of those closes the
context menu. The design export already says this in code — `openCanvasMenu`
clears `menu`, `picker`, `strokePop`, `typePop`, `framePop` and `adjPop` in the same call that
positions the menu — and it is the only sane rule, since every one of them draws over the canvas the
menu was aimed at.

⚠️ **This said *"the four inspector popovers"* and *"all six"* until 2026-09-08, and both numbers
were the design export's rather than the app's** (§15 D533). The four are `openCanvasMenu`'s four
popovers, written down when they were all there were; the app has grown the effects popover and the
Export card's two since, and neither half of the sentence followed. The code was short in the same
way and in two places — `menu::open_context_menu` cleared four of these seven, and
`app::picker_lane_right` knew four of the **six** popovers in the popover lane — so **a set nobody
owns gets miscounted by everyone who counts it**, and the count here is the one a reader checks the
code against. The lane's six are these five plus the image-editing card, which is deliberately **not**
in this clear: R4 closes floating things somebody *opened*, and that card is what a mode looks like.

**The trap, and it is a bug this app has already paid for once.** `dropdown`'s click-away test is
`i.pointer.any_click() && !menu.contains_pointer() && !head.contains_pointer()`, and both
exemptions miss here: `any_click()` counts the secondary button, and a context menu **has no
head** — it is opened by a click on the canvas, which is precisely "outside the menu". So the very
release that opens the menu reads as the click that dismisses it, and the menu shows for one frame
and vanishes. That is §15 D82 exactly, and `panels::dismissed_by_click`'s doc comment is the record
of what it cost: "the popup showed for exactly one frame and vanished. That was the reported bug."

The answer is not a sixth exemption term in `dismissed_by_click`. **The release that opens a menu
is spent opening it**, so the dismissal test does not run on the frame that wrote the slot — the
mirror of R1's rule that a press which cancelled spends the click. Between them the two halves of a
right-click are always accounted for: the press either cancels or does nothing, the release either
opens or dismisses, and neither ever does two things.

**A right-click inside an open menu does nothing at all.** It does not cancel, does not close, does
not reopen the menu three pixels over, and does not activate the row under it — rows take the
primary button. It is the one right-click in the app that is a deliberate no-op, and it is what all
five reference apps do.

**A left-click outside closes the menu and is spent doing so** — it does not also select what it
landed on. The menu was aimed at something; a click to get rid of it that also retargets the
selection turns dismissal into an edit. Listed again at the end as a position rather than a fact,
because it is one line to reverse.

⚠️ **This clause has three enforcement sites and had two until 2026-09-08** (§15 D558). The canvas
skips its mode input and a layers row skips its click while a menu is open; the **library screen**
obeyed it at neither of its two floating menus, so a click aimed at getting rid of the ⋮ popup opened
the document it landed on — and there the cost is a whole navigation rather than a changed selection.
It is the trap two paragraphs up arriving from the other direction: those two menus are hand-rolled
`egui::Area`s whose dismissals read `any_click()` and do not **consume** it, so the rule is not one a
`dropdown` term can carry — it has to be honoured by whatever the click would otherwise have done
(`dashboard::pick_or_open`). §9.6 item 16 has the rest.

---

## 1. One registry, two doors

`roadmap.md`'s *Command palette* section already decided the shape of this, and it is the reason the
menus are cheap now and would have been expensive built any other way: a registry entry carries a
display name, an **availability predicate**, **current state** for the toggles, and the thing it
dispatches — and then "context menus become *which subset of the registry applies to this
target*".

So: **no second list of strings.** The palette, the shortcut cheatsheet and these menus are three
renderings of one registry. A row's label, chord, glyph, predicate and verb live in one place, and
the accelerator a menu row prints is the same string the cheatsheet prints. Note the boundary the
same section draws and this file inherits: the registry is *app* actions and may **call** into
`build.rs`, but it does not absorb it — the document vocabulary stays where MCP and Command Mode
can share it.

**The layers panel's menu and the canvas's menu are one function of one target, not two menus.**
Identical rows, identical order, identical wording — muscle memory is the entire point of a
context menu, and a *Flatten* that is fourth in one door and seventh in the other is worse than no
menu at all. What differs between the two doors is only this:

| | Canvas | Layers panel |
|---|---|---|
| Target | what a left-click there would select (§2) | the row under the pointer |
| Reaches a **locked** or hidden layer | no — `hit_test` skips both | yes, and it is the only door that does |
| *Paste* means | paste **here**, at the pointer | paste as a sibling above this row |
| *Rename* | yes, and it opens the panel's field (§4) | yes |

---

## 2. Target resolution

**C1 — On canvas the menu's target is whatever a left-click would have selected.** Same query,
same pixel: `pick_for_click` → `pick_at_pointer` → group policy and `entered_group`, with `Ctrl` deep
selecting because it is the same query and not a second rule. This is D116's pairing applied to a
third claimant, and it is what stops a menu acting on something other than what it appears to be
aimed at. ⚠️ **That chain read `pick_leaf` here and in the code until 2026-09-07, and "same query"
was the thing that was not true** — see C5 and item 12 of §9.6. `pick_at_pointer` is the one function
both doors now ask (§15 D469).

**C2 — A hit inside the selection leaves the selection whole; a hit outside it replaces it.**
Figma, Illustrator and Affinity agree. The first half is the one that matters: without it,
right-clicking one of five selected layers to reach *Union* would silently reduce the selection to
one and produce nothing. `Shift` and `Alt` are ignored on a right-click; `Ctrl` is not, because
C1 makes it the same query.

**C3 — Empty canvas opens the canvas menu (§6.1) and does *not* clear the selection.** A left-click
there deselects; a right-click must not, because the canvas menu's reason to exist is *Paste here*
and a menu is not an edit. The consequence is deliberate and worth stating outright: **the menu is
decided by what is under the pointer, never by what is selected**, so the canvas menu carries no
selection rows even while something is selected.

**C4 — A locked layer has no canvas menu at all.** `hit_test` filters out anything hidden or
locked, directly or through an ancestor (`query.rs`), so the pointer cannot reach one. This is not
a defect to route around — it is the reason the panel's menu must carry the complete set,
including the *Unlock* that is the only way back (§5.9). **The "or through an ancestor" half was
untested until 2026-08-23** and is now pinned by
`resolve.rs::a_locked_group_locks_its_contents_for_the_query_and_for_a_click` — the old test locked the
node it then failed to click, so breaking the ancestor walk left the whole of `ondin-core` green, which
is how four refusals in the app came to read a node's own flag (§15 D321).

**C5 — A frame is right-clickable exactly where it is left-clickable.** An occupied frame passes
presses through so a marquee can be dragged across its contents (`is_occupied_frame`), so its menu
is reachable on its **name tag** (`frame_label_at`), on its edge, or anywhere on it while it is
empty. ~~Nothing new is needed for this; it falls out of C1.~~ ⚠️ **It did not fall out of C1, and
that was true from the day the menus were built until 2026-09-07** (§15 D469): `canvas_context_menu`
asked `pick_leaf` alone where the select-click arm asked the tag first, so a right-click on the tag
opened the *empty-canvas* menu at the pixel a left-click selects the frame — and since the edge and
the interior resolve through `pick_leaf` too, an occupied frame had **no canvas menu at all**. The
two doors call one function now, `canvas::pick_at_pointer`, which is C1 made literal rather than
assumed. **The *"or on its edge"* half is still answered by only one door**: `begin_select_drag` adds
`selected_frame_at` and neither of the other two has it — open work in `roadmap.md`, and the reason
this rule is worth checking against the code rather than read as settled.

**C6 — In the panel, a right-press first commits an open rename.** Already true: "a press anywhere
else ends the rename" (`layers::layer_rename_field`). The press commits, the release opens the
menu, and no special case is written.

**C7 — Guides out-rank layers, and only while they are unlocked.** The pointer inside a guide's
grab band opens the guide menu (§6.3). With *Lock guides* on, a guide is not a target at all and
the click falls through to the layer or canvas menu — which is why the canvas menu carries *Lock
guides* itself, or the setting could not be reversed from where it was set.

**C8 — Two modes claim their own menus while they are running**: a text session (§6.4) and the
node tool over the path it is editing (§6.5). Same principle as C1 — the menu belongs to the
subject the pointer is actually on.

---

## 3. The shape of a menu

**One canonical order, and every menu is a filtered projection of it**, so a row is always in the
same place relative to the rows that survive beside it. Nine groups, separated by hairlines:

| # | Group | Contents |
|---|---|---|
| 1 | **Head** | the kind's own verbs — 0 to 4 rows, §5 |
| 2 | Clipboard | Cut · Copy · Paste here · Duplicate · Delete |
| 3 | Properties | Copy properties · Paste properties |
| 4 | Structure | Group · Frame selection · Ungroup · the four booleans · Flatten · Outline shape / Convert to path |
| 5 | Order | Bring to front · Bring forward · Send backward · Send to back |
| 6 | Transform | Flip horizontal · Flip vertical · Reset origin |
| 7 | State | Hide/Show · Lock/Unlock · Rename |
| 8 | Navigate | Zoom to selection |
| 9 | Export | Copy as SVG · Copy as PNG |

⚠️ **The head read "0 to 5 rows" until 2026-08-23**, the 5 being §5.7's picture head; it is two rows
now and the longest head is text's four (§15 D305). This is the *per-kind* maximum and always was —
§5.7 composes with 5.4–5.6, so a text layer with a photograph in it has both heads and always ran
past the number in this cell.

**The head is at the top, and that is the one ordering choice worth arguing.** The head is where
the "one level in" verbs live — *Edit text*, *Edit points*, *Enter group* — and
today **every one of them is reachable only by double-click**. §15 D118's own rule is that a mode
entered only by double-click is the failure to avoid; a context menu is the cheapest possible
repair, and putting those rows first is what makes it one. It is also what the user right-clicked
*this particular thing* for.

**Image editing had no row here for two days, and the decision was re-asked and reversed**
(§15 D268). *Edit crop…* stood in this list until the crop tool was replaced, and the mode that took
its place shipped with a double-click and `Enter` as its only doors — which left it the one "one
level in" verb with no menu repair, the very repair this section argues the head exists to provide.
That was answered when the mode was designed rather than overlooked, and living with it settled it
the other way: ***Edit image*** heads §5.7 since 2026-08-22, and the reasoning that made it worth
re-asking is the reason it is now first in that head.

**It is *Edit image*, not *Edit image…*.** The ellipsis was inherited from *Edit crop…*, which
earned it back when it opened something; this row steps into a mode exactly as *Edit text* and *Edit
points* do, and neither of those carries one — an ellipsis here would promise a dialog that does not
exist. The old estimate of "one registry entry and one arm" was wrong as well: it is **five sites**
(the `Item` variant, its row in the `cfg(test)` `Item::ALL` table, the `spec()` arm, the row push and
the dispatch arm), which `roadmap.md` had already corrected on 2026-08-21, and six counting the
`Item::EditPoints` doc comment that asserted the absence was deliberate.

**Presence and availability are two questions and get two different answers:**

- **Kind decides presence.** *Ungroup* is not on a rectangle's menu; *Original size* is not on a text
  node with no picture in it. A row that could never apply to this kind is absent. (That second
  example read *Reset crop* until 2026-08-23, when that row left the menu for image editing's
  Settings tab — §5.7, §15 D305.)
- **State decides enabled.** A row that belongs to the kind but cannot run now is **dimmed and
  says why** — *Paste properties* with nothing copied, every editing row on a locked layer.
  `inspector::menu_action` is the pattern and its trap is recorded there: a widget inside a
  `disable()`d `Ui` reports no hover and `Ui::scope`'s own response is never hovered, so a dimmed
  row needs an explicit `ui.interact` claimant of its own or the sentence never appears in the one
  state where it carries information.
- **One exception: a row whose only purpose is to undo a non-default state is omitted when that
  state is default** — *Reset origin* here, and *Reset crop* and *Reset adjustments* on the other
  side of the same rule. Image editing's card dims those instead, and correctly: it is a fixed card
  whose rows are a standing list of what a picture can do. A menu is rebuilt per open and already
  type-filtered, so absence there is not information anyone is missing, while a row that is dim on
  every open is indistinguishable from one that is broken. **The contrast is now one-sided for the
  crop**: *Reset crop* was a menu row obeying the omission rule until 2026-08-23 and is the card's
  alone since (§5.7, §15 D305), so the rule keeps its example and loses its live demonstration.

**A row a kind promotes into its head is *moved*, not duplicated.** *Ungroup* on a group and
*Flatten* on a boolean belong to Structure and appear in the head instead, where the eye goes on
that kind — and then they are **gone from Structure**, because the same verb twice in one menu is
the first thing that makes a menu look generated rather than designed.

**Multi-selection follows the bulk semantics core already has.** A **head** row appears only when
*every* member qualifies — one text node among four rectangles does not put *Edit text* on the
menu. A **tail** row appears when *any* member qualifies and acts on those that do, which is what
`build::paint_targets`, `set_opacity_all` and `edit_fills_all` already mean by a selection.

**Length, counted rather than hoped for.** The tail is 19 rows before Structure adds anything
(Clipboard 5 · Properties 2 · Order 4 · Transform 2 · State 3 · Navigate 1 · Export 2). So a **v1**
menu — only the rows §9.1 and §9.2 can actually deliver — runs **23 rows for a primitive to 29 for a
boolean**, whose four operation rows and two promoted verbs are most of the difference; and **the two
totals have converged** — "with everything left in §9.3–§9.4 built as well" was 22 to 28 and is now the
same pair, neither section having an unbuilt layer row left in it.
**Two rows have been added since, and the first is the one this paragraph said could not exist**:
*Use as mask* took both ends up by one on 2026-08-21 and the Structure group from 6 to 7 (§15 D286), reversing the
§9.3 non-row it was counted as. So "nothing outstanding can lengthen these menus" was true of the open
ledger and never a ceiling — a *decision* can lengthen them, and this one did, the honest way: a verb
listed as a deliberate non-row became a real one because the model grew what it was waiting for. The
tail above is untouched by that one, Structure not being part of it.
**The second is *Copy as PNG*, and it is the row that moved the tail** — 19 to 20, Export 2 to 3, on
2026-08-22 (§15 D259's other half), taking both ends up by one again. It is a third kind of move, after
"the code arriving at §3's table" and "a decision reversing a non-row": §3's table had **never** listed
a PNG row and now does, because the code went first and this file followed it. §5.5 carries why that
was the right direction here and why it is not a §9.6 divergence.
**And a fourth kind of move, hours later the same day: a row *leaving*.** *Export as…* was removed
(§15 D264), taking Export back to **2**, the tail back to **19** and both ends back to **23 and 29**.
**That the two moves cancel is a coincidence and not a correction**, and it is worth saying outright:
a reader comparing today's numbers against 2026-08-21's finds every one of them unchanged and the
Export group entirely rewritten underneath. What moved is the *labels*, which is why the primitive
test spells them out rather than only counting them — either move alone would have failed it, and
together they do not touch the count at all.
**Both totals are the menu as actually
built, and both ends are pinned by name** —
`a_primitives_menu_is_twenty_three_rows_in_canonical_group_order` and
`a_booleans_menu_is_the_long_end_of_the_range`. The long end is the one nothing was counting, and it
is the end the ceiling below rests on, so a range with only the short end measured was half a claim.
They read 16 and 24 until 2026-08-20 and were wrong twice over: this paragraph counted the Properties
pair in its tail of 17 and in neither total, and *Outline shape* (§15 D230) and *Frame selection*
(§15 D249) had since landed out of §9.3–§9.4 and are in every one of these menus. §15 D257 is the
pair that closed the first half; §15 D259's *Copy as SVG* is what took both ends up by one, filling
the Export group in the code for the first time, and §15 D264's *Export as…* took them up by one again
the same day and has since taken that one back down.

**Text is 27 rows, and the range still holds** — head 4 · Clipboard 5 · Properties 2 · Structure 4 ·
Order 4 · Transform 2 · State 3 · Navigate 1 · Export 2, counted once with a throwaway probe on
2026-08-20 when §5.6's last two rows landed (§15 D260, D261) and **not pinned by a test name**, unlike
the two ends. ⚠️ It was 26 then, and the one extra row is *Use as mask*, **read rather than
re-measured**: it is pushed for any selection with no frame in it and not inside a boolean, which a
text layer satisfies. *Copy as PNG* and *Export as…* both moved it on 2026-08-22 and in opposite
directions, being §4's invariant tail, which every layer menu gets whole — so this number cancelled
exactly as the two ends did. Re-probe it if it is ever load-bearing: three row moves have now been
applied to this number by reading rather than by measuring, where the two ends have a test each. It is stated because the sentence above reads as though the primitive and the boolean
bracket a spread of *kinds*, and they no longer do in the way they did: text used to be the obviously
short kind and now sits two rows off the long end. The endpoints do not move — the three sizing rows
are a head and *Convert to path* takes *Outline shape*'s slot — so nothing here is a new ceiling.

That is Figma's order of magnitude (its shape menu is around 22 items) and it is also the ceiling: a
context menu that needs a scrollbar has stopped being one. **If it reads long on the machine, the
two groups to give up first are Export and Navigate** — both are second doors to things reachable
elsewhere, where Properties and the head are not. Say that now, so the trim is not taken out of the
rows that justify the menu. ⚠️ **That is less true of Export than it was.** It said so when the group
held *Export as…*, whose subject the inspector's Export panel also reaches; what is left is the two
clipboard rows, and nothing else in the app puts artwork on the clipboard at all. Navigate is still a
second door. If this trim is ever taken, take it knowing the Export group is now a first door and not
a spare one.

---

## 4. The invariant tail — on every layer target, canvas and panel alike

Groups 2–9 of §3. Chords are `shortcuts.md`'s; a row with no chord has none there either.

### Clipboard

**Every *Paste* row bar the text session's now asks one question — "would a paste do anything" — and
it is not "is the in-app clipboard full"** (§15 D218). **Three** terms, in the order both paste paths
try them (§15 D224): a **picture** on the OS clipboard; the app's own payload *while it is
still what the OS clipboard describes*, so a stale one is not offered; **or** text from any
application, which the row turns into a text layer. Before 2026-08-18 it was the middle term without
the staleness check and without the third at all, so *Paste* went dim on the commonest paste there
is — a sentence copied in a browser; the picture was added on 2026-08-19, when the row turned out to
be dim over a screenshot as well as unable to place one.

**The picture term is a snapshot and costs a bitmap conversion to take**, which is why it is asked
once per open and answered with a `bool` rather than by keeping the image: `arboard` has no
format query, so `get_image` is the only way to ask and it decodes. See D224 — including why the ask
is nonetheless cheap on the opens where there is no picture, and what to re-check if right-click ever
starts to feel slow.

| Row | Chord | Cost | |
|---|---|---|---|
| Cut | `Ctrl+X` | wiring | ✅ `Action::Cut` |
| Copy | `Ctrl+C` | wiring | ✅ |
| **Paste here** | `Ctrl+V` | cheap | ✅ canvas: at the pointer — Figma's, and **the only door onto it**: §7's `Ctrl+V` row wanting the same was decided against 2026-08-22 (§15 D300), so the chord's fixed offset and this row's aim are two behaviours on purpose rather than one owed to the other. Panel: *Paste*, and the "as a sibling above the clicked row" placement is still §9.6.3. All three payloads reach it as of 2026-08-19 (§15 D224); **a picture is aimed like a *drop*** — a selected shape under the press, or a new layer at it. **It did not aim after a *cut*** until 2026-08-20 — the box was read back off the deleted source layers, so the row silently became *Paste in place* and stayed there on every repeat; it is recorded at copy time now (§15 D251) |
| Duplicate | `Ctrl+D` | wiring | ✅ |
| Delete | `Del` | wiring | ✅ |

*Paste to replace* (Figma) is deliberately not here — see §7.

### Properties

| Row | Chord | Cost | |
|---|---|---|---|
| Copy properties | `Ctrl+Alt+C` | cheap | ✅ Figma. Built 2026-08-20 (§15 D257) — fills, strokes and opacity off the **key layer**, or a lone selected one. **Live on a locked layer**: reading one is not editing it |
| Paste properties | `Ctrl+Alt+V` | cheap | ✅ dimmed, saying so, when no properties have been copied — and dimmed by the *lock*, whose sentence wins when both apply |

These are the pair worth having in a menu even more than on a key: nobody discovers
`Ctrl+Alt+V` from a keymap.

**What the payload holds is the decision, and what it leaves out is most of it** (§15 D257). Position,
size, rotation, the name, the pivot and the lock are out — they say *which layer this is* rather than
how it is drawn — and geometry is out for a second reason, a corner radius living in `NodeKind` and
meaning nothing carried from a rect to a star. The scope is not a new rule either: the paste is
`set_fills_all`, `set_strokes_all` and `set_opacity_all` composed, so it reaches through a group and
stops at a frame for paint, and touches the outermost members only for opacity, exactly as every other
bulk edit in the app does. *Copy properties* refuses a **group**, which is why it has two dim
sentences rather than one; both rows are otherwise present on every kind, a group being a legitimate
*destination*.

### Structure

Every row here is kind-gated, and the gates are the code's, not this file's.

| Row | Chord | Cost | |
|---|---|---|---|
| Group selection | `Ctrl+G` | wiring | ✅ `build::group`. Present on one layer too, as Figma's is |
| **Frame selection** | — | feature | ✅ **built 2026-08-20** (§15 D249). The scoring was right that it is new work — `build::frame` is a second builder, not a flag on `group` — and the reason is the frame having a *box*: it is created at the union's corner, so every member is moved back by exactly that corner where a group touches nothing. **Present where *Group selection* is absent**, on a selection of frames, since frames nest. Dim inside a `Group` (§5.3 lets an `Artboard` hang off the root or another `Artboard` and nowhere else), and **no accelerator** — `shortcuts.md` binds none and a chord invented at a row is one that file does not know about |
| Ungroup | `Ctrl+Shift+G` | wiring | ✅ — and **only on a Group or a Boolean**: `build::ungroup`'s kind gate is exactly those two, so the row is absent on a frame, which is honest rather than a bug to hide |
| **Use as mask** | `Ctrl+Alt+M` | feature | ✅ **built 2026-08-21** (§15 D286), reversing §7's non-row. Directly after the group verbs and before the booleans, because with the group-wrapping arm it *is* one of the container-making verbs — after *Ungroup* where that row is present, after *Frame selection* on a shape, where it is not. **Checkable like *Clip content*, not like the booleans**: a toggle whose off-state is worth showing, ticked off `LayerState::masked`, which is true when the layer **is** a mask *or* holds one — the second half being the state the verb itself leaves behind. **Absent** on a selection containing a frame (a frame can neither be a mask nor be grouped — *Group selection*'s pair of reasons) and inside a boolean, where operands are combined rather than drawn; dim on a lock. `build::mask` is the verb |
| **Even-odd fill ✓** | feature | ✅ **built 2026-08-31** (§15 D239), and **a row this file never specified** — it arrived with the fill rule itself. Directly after *Use as mask* and before the booleans, which is where the emitted order puts it. **Checkable, like *Use as mask* and unlike the four booleans**: two rules, and the off-state is worth showing, where four alternatives need four labels. **On a lone `Path` only** — the one authored kind whose shape can differ between the rules, a rect, an ellipse, a star and a polygon being single non-crossing outlines where the control would visibly do nothing; and one layer at a time, because the tick reports *this* shape's rule and a mixed selection has no answer to show. **Absent on a boolean, deliberately**: an `Exclude` is even-odd because that is what a symmetric difference is, and `Node::fill_rule` derives it from the operation, so a row appearing to toggle it would be offering to make the shape wrong. Dim on a lock |
| Union / Subtract / Intersect / Exclude | `Ctrl+Alt+U`/`S`/`I`/`X` | wiring | ✅ `build::boolean`, two or more members. `Ctrl+Alt+X` **was** a live bug — it cut the selection instead of excluding it, and this row was the one path to Exclude the bug did not touch — **fixed 2026-08-18** by reading the chord off the key *release* (`shortcuts.md` §L2, §15 D204). The Alt guard that fix installed is a *branch* since 2026-08-20 (§15 D257) and `Ctrl+Alt+X` is still dropped there, because the release arm owns it |
| Flatten | `Ctrl+E` | wiring | ✅ Figma. On a boolean, or on two or more of anything |
| **Outline shape** | — | ~~*undecided*~~ | ✅ **decided and built 2026-08-19** (§15 D230). It is a **second verb**, `build::outline`, not a widened `flatten` — which still refuses a lone non-boolean, deliberately, so the two rows are never offered together. On one shape only; on a `Path` it bakes the per-anchor radii and is **dim when there are none to bake**, which is §3's rule on the one kind where presence and availability differ. Absent on a `Group` (its outline is the union of its contents, which is what *Flatten* gives a set), a frame, a boolean and text |
| **Convert to path** | — | feature | ✅ **built 2026-08-20** (§15 D260). §5.6's row, filed **here** rather than in the text head because §3 groups by what a verb does and this replaces the layer. `build::outline_text`, a **third** verb: text's outline is a *shaped* thing, so it is the one of the three that needs `Resolved`. On one text layer only, and it takes *Outline shape*'s slot — the two are mutually exclusive by kind, so no menu holds both. **Dim when the layout has no contours** (empty content, or nothing but spaces), which `build::can_outline_text` answers by asking `text::outline` rather than by counting glyphs |
| **Flip to other side ✓** | feature | ✅ **built 2026-09-01** (§15 D406), and **a row this file never specified** — type on a path did not exist when it was written. Runs the type the other way along its rail, and so along the other side of it. **Checkable rather than two rows**, because it is one state with two values and the tick says which; a pair of rows would have to name the two sides, and *which* side is "the other" depends on which way the user happened to draw the curve, so neither name would be true of every rail. On a single layer that is already on a rail (`Context::on_a_rail`), so it is never dim except on a lock. Filed **here** rather than in §5.6's head for *Convert to path*'s reason: §3 groups by what a verb does |
| **Detach from path** | feature | ✅ **built 2026-09-01** (§15 D405), same origin as the row above and directly below it — *Flip* is what you reach for while the type is on the curve and this is what you reach for when you have finished with it. Takes the text layer off its rail, leaving the rail as a path layer. ⚠️ **There is no *Text on path* row to pair with, and its absence is the decision** (§15 D409): setting type on a curve is the Text tool's own gesture — hover an edge, click, type — so a menu row asking for a text layer and a shape to be selected *together first* was a second, worse way in. This is the way back out, and it has no gesture of its own, which is what keeps it a row |

### Order

| Row | Chord | Cost | |
|---|---|---|---|
| Bring to front | `Ctrl+Shift+]` | wiring | ✅ `build::z_order` |
| Bring forward | `Ctrl+]` | wiring | ✅ |
| Send backward | `Ctrl+[` | wiring | ✅ |
| Send to back | `Ctrl+Shift+[` | wiring | ✅ |

**Never dimmed for being already frontmost.** `z_order` returns an empty transaction and the app's
existing idiom for that is an info line, the way `distribute` says "Already evenly spaced". Four
predicates run on every menu open to grey four rows is a worse trade than one sentence after a
click that did nothing.

### Transform

| Row | Chord | Cost | |
|---|---|---|---|
| Flip horizontal | `Shift+H` | wiring | ✅ Figma, `build::flip` |
| Flip vertical | `Shift+V` | wiring | ✅ |
| Reset origin | — | cheap | `SetPivot { pivot: None }`. **Omitted unless the pivot has been moved** (§3's exception) — and it is the only way back to "not moved", which is a state the model can say and no field currently un-says |

*Rotate 90°* is not here: nothing in `build.rs` turns a selection by a quarter, and `flip` is the
model to copy when someone wants it. Left out rather than half-specified.

### State

| Row | Chord | Cost | |
|---|---|---|---|
| Hide / Show | `Ctrl+Shift+H` | wiring | Figma **and** Sketch. The chord is built (`OndinApp::toggle_hidden`), so this row is a dispatch. Label follows the target exactly as this row always said — a mixed selection reads *Hide* and hides all of it, which is what the chord does |
| Lock / Unlock | `Ctrl+Shift+L` | wiring | Figma **and** Sketch. Chord built; mixed reads *Lock* and locks all of it, for the row above's reason. On the panel's menu this is the row C4 exists for |
| Rename | `Ctrl+R`, `F2` | wiring | Figma. Opens the panel's inline field (`layers::layer_rename_field`, built). ⚠️ **The hidden-panel case was decided the other way**: this said dim it with a reason, and the built chord **shows the panel**. A chord has no row to dim, so it had no third option — and once the chord opens the panel, a menu row that dimmed instead would be the odd one out. Two further limits the chord imposes and this row should match: single-selection only (with several picked there is no one row to lay the field over), and refused outright in present mode, which draws no panel for the field to appear in |

*Keep proportions* is not here — the inspector's chain link owns it and it is a modifier on how an
edit behaves rather than a verb.

### Navigate

| Row | Chord | Cost | |
|---|---|---|---|
| Zoom to selection | `Shift+2`, `Ctrl+2` | wiring | Figma and Sketch both. ✅ — `Action::ZoomSelection` exists as of 2026-08-18 (`decisions.md` §15 D209), so this row is now a dispatch rather than a feature |

*Reveal in layers panel* was the second row here and is not built — see §7.

### Export

| Row | Chord | Cost | |
|---|---|---|---|
| ~~Export selection…~~ | ~~`Ctrl+Shift+E`~~ | **withdrawn** | Figma's. **Built 2026-08-20 as *Export as…* and removed 2026-08-22** (§15 D264, both halves of it). Kept visible rather than deleted, because this table specified the row from the start and the useful record is that it was built and then *withdrawn* — not that it was never built. It asked where, what format and at what size on every single use and remembered none of it; the inspector's Export panel (§15 D274) answers all three from the layer and carries a button that runs the export, so this had become the slower of two doors onto the same file. **Removed as a whole verb, not as a row**: the variant, `Item::ALL`, the `spec()` arm, the push, the dispatch, `Action::ExportSelection`, `OndinApp::export_selection` and `export_subtrees`, and the two free functions the compiler then found dead (`wants_raster`, `written_name`) — because a chord with no menu row is the reachability failure §5 argues against, in different clothes. ⚠️ **`Ctrl+Shift+E` is unbound but not free**: plain `Ctrl+E` is *Flatten*, so its `!m.shift` guard is now the only thing between a hand still reaching for this chord and a dissolved selection, and it reads like a redundant guard on a chord that no longer exists. `a_stray_shift_on_ctrl_e_does_nothing_rather_than_flattening` is the pin. What does **not** go with it is `scene::build_of`, which this row paid for and which is now the *only* spelling of the scene walk — `scene::build` is one call of it with the root on its own, so the canvas, *Copy as PNG*, the Export panel and the CLI all run through it |
| Copy as SVG | — | feature | ✅ **built 2026-08-20** (§15 D259). Figma's *Copy as ▸ SVG*. The scoring was right: the row is one line and the *writer* was the work — `export::svg` took `artboard: Option<NodeId>` and not a selection, so `svg_of(doc, res, &[NodeId])` is new in `ondin-export`. Copies the **committed** document and `Resolved`, the pair `ondin export` renders from, so a gesture mid-drag cannot leak an uncommitted position into another application; `build::in_document_order`, which is `outermost` for *Copy*'s reason **plus the ordering the writer requires and this row shipped without** — a selection is in *pick* order, so shift-clicking front-to-back copied the artwork restacked until 2026-08-20 (§15 D267). **Writes no `clipboard_stamp`**, which is what correctly makes `owns_the_clipboard` answer *no* afterwards, so `Ctrl+V` stops offering layers the user has since replaced. ~~**No *Copy as PNG* beside it** — §9.3.~~ **There is one, since 2026-08-22** — the row below. The scoping this line rested on was mechanical and the mechanism arrived for another row |
| Copy as PNG | — | ~~scoped out~~ | ✅ **built 2026-08-22** — §15 D259's other half, and no new number, because this is that entry's own scoped-out half arriving rather than a fresh decision. Figma's *Copy as ▸ PNG*, and Figma offers **exactly one of them, with no 1×/2×/3×** — tested on the machine that day rather than assumed, which is why there is no submenu here and why the next reader should not re-propose one. A multiplier belongs to the inspector's Export panel, where a *repeatable* export is what keeps settings (§15 D274); this row is `RasterOpts::default()` entire — scale 1.0, no background, no trim, no pad. **`raster_of`, not `png_of`**: `arboard::ImageData` takes raw straight-alpha RGBA and `png_of` returns an *encoded file*, which the clipboard would then read as pixels — so the wrong version pastes as noise rather than failing, and it **compiles**, the size coming honestly from `extent`. That is why the choice is lifted into `OndinApp::png_for_the_clipboard`, an associated function taking no `self`: `copy_as_png` itself ends in the OS clipboard and a test of *it* would write to the user's. **`arboard` directly, not egui** — `ctx.copy_text` is a *text* clipboard, which is why the row above can use it and this one cannot; the same direct dependency `paste_image` already opens to read a picture *in* (§15 D183), used in the opposite direction. **Writes no `clipboard_stamp`** either, and here that is the load-bearing part: of every row that writes to that clipboard this is the one whose content cannot be turned back into layers at all, so `owns_the_clipboard` must go on answering *no* and `Ctrl+V` must stop offering layers the user has since replaced. `build::in_document_order` for the row above's two reasons (§15 D267). Never dim, for *Export original…*'s reason — a locked layer keeps both, neither of them changing the layer |

Both are last so the menu reads the same with them absent — which neither now is. The group met §3's
table exactly on 2026-08-20, went a row past it on 2026-08-22 with the table amended after the fact
rather than before it, and came back to two the same day when the file row was withdrawn. **It is a
clipboard group now and no longer a mixed one** — nothing in it writes a file — which is why "never
dim" is true of the whole group rather than of two rows out of three: the menu is open on a layer, so
there is always something to write, and neither row changes the layer.

---

## 5. The heads, per kind

The rows above group 2. Everything in §4 follows, filtered.

### 5.1 Frame (`NodeKind::Artboard`)

| Row | Cost | |
|---|---|---|
| Clip content ✓ | wiring | `SetClip`, a checkable row. The frame's signature property and the only kind where `Node::clip` means anything (`NodeKind::clips_children`) |
| ~~Background…~~ | **decided non-goal** | **Not built, and struck on 2026-09-19 rather than deferred (§15 D800).** It was specified to open the picker on `Artboard::background` — second door to the inspector's field, deliberately, a frame's background being the thing people right-click a frame for — and there was never an `Item` for it: `head_rows`' `Kind::Frame` arm pushes *Clip content* alone. The field it was a door to **no longer exists**: §15 D400 made a frame's fill its **fill list** and deleted `Operation::SetArtboardBackground` and `PaintSlot::Background` with it, and D397 renamed the inspector card to *Fill*. **The ruling takes D400's own reason**: the inspector's Fill row does this job for a frame exactly as it does for anything else, so a frame-only second door is the special case D400 deliberately removed from every fill verb in the workspace. 🚨 **A row would need no new operation** — it could open the picker through the ordinary paint path — **which is what makes it tempting and is also why it is redundant**; the same argument §6.1 already makes against *Canvas background…*. ⚠️ **Until the ruling, no §15 entry recorded this row being dropped, deferred or made a non-goal** — the op went and the menu was never mentioned — so it was neither landed nor struck nor blocked, a state this ledger had no bucket for; it was question 2 of the codebase review's final triage. **A row this file specified cannot leave without a §15 entry**, and that is the whole of why one was written for a row nobody built. |
| ~~Export frame as SVG… / PNG…~~ | **subsumed** | **Not built, and a decided non-goal rather than a deferral (§15 D264).** It was retired on 2026-08-20 by *Export as…* — this line's own observation, `export::svg(doc, res, Some(artboard))` "is exactly this", being also exactly `svg_of(doc, res, &[artboard])` — and §3's Export group sits on **every** layer's menu, so with a frame right-clicked the frame *is* the selection. ⚠️ **The row that subsumed it was itself removed on 2026-08-22, and this one does not come back with it.** The subsumer is now the inspector's **Export panel** (§15 D274), which is what made the general row redundant in the first place and does so a fortiori here: if a one-off file export is not worth a row on *every* layer's menu, it is not worth a narrower one on a frame's. Re-opening this would be re-opening the verb that was just withdrawn, one kind at a time. What genuinely changed is the *reach*, and it changed for every kind rather than for frames — the panel wants a spec on the layer where the menu row wanted none, so a one-off is a click or two further away than it was; that cost belongs to D264's removal and is recorded there, not here. The one thing this row could have meant that neither door does — *export the frame this layer is in*, from a **child's** menu — is still not what it says, and is still a different row |

*Ungroup* is absent, per §4: `build::ungroup` takes a Group or a Boolean and nothing else. Note
also that a frame cannot be a member of *Group selection* (`build::group` refuses it), so on a
frame the Structure group is often empty and collapses to nothing — which is what the projection
rule is for.

### 5.2 Group

| Row | Cost | |
|---|---|---|
| Enter group | wiring | ✅ `entered_group` + one step of `group_chain`, which is exactly what `double_click_pick` does. Illustrator's *Isolate Selected Group*, and the discoverable half of a gesture that had no other announcement — **`Enter` is now the third door and shares this row's body** (`OndinApp::enter_container`, §15 D228), which is what stops a row and a chord drifting apart |
| Ungroup | wiring | promoted out of Structure, not repeated there (§3) |

### 5.3 Boolean

| Row | Cost | |
|---|---|---|
| Enter boolean | wiring | the operands stay selectable and editable — non-destructive, like Figma's — and this says so |
| Union ✓ / Subtract ✓ / Intersect ✓ / Exclude ✓ | wiring | four **checkable** rows over `build::set_boolean_op`, the same four the inspector's dropdown offers, with `layers::op_glyph`'s glyphs. `menu_check` already draws exactly this row |
| Make this the base operand | cheap | only when the pointer is on a child. The base is *bottom of the list* and only `Subtract` reads it (§15 D113); `designate_key` already turns this into a `Reorder` to index 0. Nothing on screen currently announces `Alt+Shift`+click |
| Flatten | wiring | promoted out of Structure (§3) — on this kind it is *the* verb, and it turns the boolean into the `Path` its outline already is |
| Ungroup | wiring | promoted likewise; releases the operands. Illustrator calls it *Release Compound Shape*, and the row keeps our word and our chord |

### 5.4 Path

| Row | Cost | |
|---|---|---|
| Edit points | wiring | `choose_tool(Tool::Node)` — and the info line the double-click already prints, which is the closest thing the app has to teaching this. **It dims on a locked layer, and since 2026-08-23 the mode refuses as well** (§15 D321): the row dimmed from the day it was built while `Enter` armed the tool anyway, which is C4's rule broken in the direction §15 D261 calls the failure. The guard is in `enter_action`'s `Path` arm rather than in a funnel, because nothing in `tools/` reads the lock — a locked path's anchors are draggable the moment the tool is armed. ⚠️ This arm's own three lines are still shut only by the dim — but the *mode* refuses underneath them, `canvas::edited_path` returning no subject for a locked path, so `A` and the rail's node button arm onto nothing too and no door reaches a locked path's points |
| Outline shape | see §4 | ✅ and a path is the kind that made the question stale in the first place — anchor editing is what made the trade worth taking. Here it **bakes the per-anchor radii into the geometry** and is dim when there are none, since `round_corners` would return the path untouched (§15 D230). *Flatten* is **not** the same row and is not offered on one shape |

**A third row reaches only this kind and is specified in §4's Structure table**: *Even-odd fill* (§15
D239), offered on a lone `Path` and nowhere else, because a path is the one authored kind whose shape
can differ between the two rules. It is not in this head — §3 groups by what a verb does, and changing
which parts of a shape are inside is structural — which is why it is described beside the booleans it
sits among rather than here.

### 5.5 Primitive shapes (Rect · Ellipse · Polygon · Star · Line)

**No head at all.** These are the baseline menu — groups 2–9 and nothing else, 23 rows in v1 and the
shortest menu in the app. Corner radius, side count and star ratio are numeric fields and belong to
the inspector; a menu is the wrong instrument for a number.

The one candidate was *Outline shape* (§4) — where a rect stops having W/H fields and
starts having anchors — and it **landed 2026-08-19** (§15 D230), in Structure rather than in a head of
its own, which is where §3 puts it. **That is the row that took this menu from 16 to 17**, and
*Frame selection* took it to **18** on 2026-08-20 (§15 D249) — not a head candidate at all, just
Structure's third row. Later the same day the **Properties** pair took it to **20** (§15 D257), which
is the first of these three that §3 had counted all along, *Copy as SVG* took it to **21** (§15 D259),
and *Export as…* to **22** (§15 D264) — so the last three moves were the code arriving at §3's table
rather than drifting from it, and with the Export group complete it had arrived exactly. **23 is *Use
as mask*** (§15 D286), the first row to move this number since, and it moves it by a *decision* rather
than an implementation: §3 had counted the row as a deliberate non-row, and the model grew what that
refusal was waiting for.

**24 was *Copy as PNG*** (2026-08-22, §15 D259's other half), and it was a **third** kind of move,
worth separating from both of the others: §3's table had never listed a PNG row at all — §9.3's
scoring line said so in those words — so the code went one row *beyond* the Export group as specified,
where every previous move brought it *up to* the specification or reversed a stated refusal. §3's
table has been amended to match rather than the code trimmed to it, because the row belongs where it
landed: it is a layer verb in the group §3 built for layer verbs, and the only reason it was not
listed is that the renderer could not do it when the list was written. That is also why it is **not**
a §9.6 divergence — §9.6 records where the code and this file disagree, and after the amendment they
do not.

**And back to 23 hours later**, when *Export as…* was removed (§15 D264) — a **fourth** kind of move,
a row leaving, and the first time this number has gone *down*. **The two moves cancel and that is a
coincidence**, worth naming because 23 is what this section said the day before as well: the menu is
the same length and its ninth group has been completely rewritten, from one file row and two
clipboard rows to two clipboard rows and nothing else.
`a_primitives_menu_is_twenty_three_rows_in_canonical_group_order` is the count, and it is the
**labels** the test spells out that tell the two days apart — the number alone cannot, which is the
whole argument for asserting a sequence rather than a length.

### 5.6 Text

| Row | Cost | |
|---|---|---|
| Edit text | wiring | ✅ `begin_edit_text` — a double-click, this row, and (since 2026-08-19) `Enter`. **The panel door has no point to place a caret from and no longer invents one**: it passed `Point::ZERO`, and both it and `Enter` now select the whole string instead (§15 D228). **A lock refuses in `begin_edit_text` since 2026-08-23, a locked group's included** (§15 D321) — the row had dimmed all along while `Enter` opened a session on the same layer and let the words be retyped, so the refusal is in the one function both doors call and the row dims *because* the mode refuses |
| Auto width ✓ / Auto height ✓ / Fixed size ✓ | wiring | ✅ **built 2026-08-20** (§15 D261). Three checkable rows over `TextSizing`, `Item::TextSizing(u8)` carrying the panel's cell index. Duplicates the inspector's control on purpose — *Auto width* is the single most reached-for text command in Figma and right-click is where a Figma hand reaches for it — so the panel's body was lifted into `OndinApp::set_text_sizing` and both doors are the one verb (§8). **Only over a single text layer**: the tick is a fact about one node, and `LayerState::text_sizing: Option<u8>` is what makes the state with no answer unspellable. The labels are this menu's own, not `TextSizing::LABELS`' abbreviations |
| Convert to path | feature | ✅ **built 2026-08-20** (§15 D260) — Affinity's *Convert Text to Curves*, and the only app of the five with a clean answer. **The scoring's premise had expired**: `text::outline` has pulled glyph contours out of `skrifa` since outside-aligned type strokes did (§15 D145), so the row was wiring plus one refusal. `build::outline_text` is a **third** verb beside `flatten` and `outline` — text's outline is a *shaped* thing, so it is the one of the three that needs `Resolved` — and it files under **Structure** rather than in this head, §3 grouping by what a verb does. Still a one-way door, which is what the wording and the status line are for |

**This section now has no unbuilt row in it** — the second of the file to say so, after §6.5 (§15 D255,
D256). Four of these rows are the *head*, in the order above; *Convert to path* is the fifth row this
table names and it is not in the head at all, having gone to Structure. **Two further rows reach a
text layer and are specified in §4's Structure table with it**: *Flip to other side* and *Detach from
path* (§15 D405, D406, D409), which the code files under Structure for *Convert to path*'s reason and
which appear only on a layer already on a rail. They arrived on 2026-09-01, after this section was
written, and `Item::DetachTextPath` and `Item::FlipTextPath` cite **§5.6** in their doc comments — so
this is where a reader is sent and §4 is where they are described. A text menu is therefore **27
rows** — head 4 · Clipboard 5 · Properties 2 · Structure 4 · Order 4 · Transform 2 · State 3 · Navigate
1 · Export 2 — which sits inside §3's range without moving either end of it, and a **railed** text
layer's is **29**, Structure being 6 there. It read 26 with Structure
3 until 2026-08-22, and the term that moved is one this section does not own: *Use as mask* in
Structure (§15 D286). **Export moved twice that day and came back to where it started** — *Copy as
PNG* added (§15 D259's other half) and *Export as…* removed (§15 D264), both in §4's tail, which every
layer menu gets whole. §3's own copy of this count carries the ⚠️ about it being read rather than
re-probed, and that warning is worth more now than it was: this number has survived three row moves
without anyone measuring it.

### 5.7 Any layer with a picture in it

**Not a kind.** An image is a fill, not a node kind (§5.5a), so this head is gated on
`tools::croppable_fill` — the same predicate image editing and `double_click_pick` use, so the row
cannot offer what the mode would refuse. It composes with 5.4–5.6: a text node or a path with a
photograph in it gets **both** heads, in `double_click_pick`'s own order — text, then the picture,
then the points — so the menu and the double-click cannot disagree about what "one level in" means
on the same layer.

**Two rows, and the head verb is first.** *Edit crop…* was the first of them and went with the crop
tool (§15 D268); *Edit image* took its place on 2026-08-22, under the mode's own name and without the
ellipsis — see §5's note on the head. It read **five rows** until 2026-08-23, when the maintainer
split the picture verbs between this menu and image editing's Settings tab by asking of each which
door a hand actually reaches for (§15 D305): *Reset crop* and *Replace…* keep the tab alone, *Export
original…* was removed from both, and *Original size* keeps this door alone. The three that left are
struck below rather than deleted, as *Export as…* is in §4 — this section specified them, and the
useful record is that they were built and then withdrawn, two of them **to another surface** and one
**altogether**, not that they never existed.

**That opening paragraph is now doing more work than it was written to do.** "Gated on
`croppable_fill` … so the row cannot offer what the mode would refuse" was a claim about *Reset crop*
and its neighbours when it was written. With *Edit image* here it is a claim about the mode's own
door: `LayerState::has_picture` **is** `tools::croppable_fill(node.paint()).is_some()`, the same
function `Enter` and the double-click ask, so the row's presence and the mode's willingness cannot
drift apart — not "the same reading written twice" but the same call.

| Row | Cost | |
|---|---|---|
| Edit image | wiring | ✅ **built 2026-08-22** (§15 D268). The head verb: `OndinApp::begin_image_edit` on the one layer under the pointer — a lock refusal since 2026-08-23, then `set_one`, `choose_tool(Tool::ImageEdit)`, the status line. This cell said the arm was "copied rather than shared, and if that string ever earns a constant all three doors want it at once"; **it earned one on 2026-08-23**, when a fourth door landed on the inspector's fill row, and the three lines are now that one function (§15 D305). **Pushed first**, ahead of *Original size*: §5 argues the head exists so a mode entered only by double-click has a menu repair, and a repair below the other rows is not one. The cost was five sites rather than the "one registry entry and one arm" §5 used to estimate; the *doors* it dispatches into already existed. `edit_image_heads_the_picture_rows_and_appears_only_with_a_picture` pins the order and the gate — against **one** other row now rather than four, and still worth having, because "first" is the whole of what makes it a repair. **It dims on a locked layer since 2026-08-23** (§15 D320), which reverses what this cell and §15 D268 both argued: a crop *is* a resize — `crop_resize_tx` goes through `tools::resize_layer` — so it writes what *Original size* writes and the lock reaches both. ⚠️ The row dims **because** the mode refuses, never instead of it: `begin_image_edit` returns early on a locked node, so `Enter` gives the same answer, and dimming the row while the mode still allowed it would be §15 D261's failure even though the menu would look identical |
| Original size | wiring | resizes the layer to the picture's own pixels — the *turned* size for a turned picture. One layer at a time. **The whole of this head besides the verb since 2026-08-23**, and the only one of the five that kept the menu door rather than the tab (§15 D305): it is what a hand reaches for *after* playing with a picture and wanting the layer back at its true size, which is wanted without being in the mode, and the card required entering one to offer it. It **dims on a locked layer** because it writes the node's geometry — and that dimming is now the only answer, the card's copy having committed the write on a locked layer with no gate at all (§15 D261, D305). It was alone in this head in dimming until 2026-08-23, when *Edit image* joined it: a crop is a geometry write too (§15 D320), which is the argument that settled the row above and is why the pair now agrees. `a_lock_dims_both_of_the_picture_rows` — renamed from `a_lock_dims_original_size_and_leaves_edit_image_alone`, and its guard is `app::image_edit_lock_tests::a_locked_picture_refuses_every_door`, without which it asserts only a tidy-up |
| ~~Reset crop~~ | ~~wiring~~ | **withdrawn 2026-08-23** (§15 D305). `whole_crop()`, omitted when nothing is cropped (§3's exception). It survives in image editing's Settings tab, which is the door it was always better suited to: worth having, not worth a row in a menu that opens on every layer |
| ~~Replace…~~ | ~~wiring~~ | **withdrawn 2026-08-23** (§15 D305). The file dialog; §15 D179's repair for the missing-picture placeholder, and the row a broken image most needs — which is why it stays in the Settings tab rather than going altogether, beside *Replace and reset*, which this table never listed |
| ~~Export original…~~ | ~~cheap~~ | **built 2026-08-19 (§15 D225) and removed 2026-08-23 from both of its doors** (§15 D305) — this one and the inspector's Asset section, together, so there is no surviving spelling of the verb. It wrote `ImageEntry`'s stored bytes back out through `ImageFormat::extension`: the encoded original, byte-identical to the file that went in, which is the payoff of a format that keeps originals rather than decoded buffers. A real capability and one nobody reaches for — "export this picture" means the layer at a size and a format, which is what the inspector's Export panel is (§15 D274). What the row is worth remembering for is two rules it carried. **It was the row a locked layer kept live for a reason about locking itself** — a lock protects a layer from being *changed*, and reading its bytes back out changes nothing — where *Edit image*, the other live row, was live only because the mode's other doors did not check the lock either — a match with a gap rather than agreement, and it stopped being live on 2026-08-23 when the gap was closed (§15 D320). So the distinction is still the sharper of the two and now has **no** surviving instance: this row is the only one that was ever live for a reason about locking itself, which is why it is worth reading here rather than deriving again. And it **dimmed only for a *linked* picture, whose bytes the document does not hold — which this file said and the code did not do until 2026-08-20** (§15 D265), the row having carried no gate at all and refused *after* the click where the inspector refused before it. The predicate that closed that, `tools::original_refusal`, went with the rows and left `ImageEntry::is_linked` back at zero callers; its three-state reasoning is preserved as comments where the function was (§15 D280, D305). ⚠️ `export_original_dims_for_a_linked_picture_and_says_something_else_for_a_missing_one` **no longer exists** — this cell named it until 2026-08-23 |

The four fit modes (*Fill · Fit · Crop · Tile*) are **not** here — image editing's card owns the
strip, they are a four-way exclusive choice rather than a verb, and the card shows the tile scale
and orientation beside them, which a menu row cannot. Since §15 D268 they are also only meaningful
*inside* that mode: Crop is what the handles do, and the other three are the override that undoes
it, which is not a statement a menu on an unopened picture could make.

### 5.8 Multi-selection

**No head** — there is no single kind — unless every member is the same kind, in which case that
kind's head applies whole (§3). Otherwise the menu is groups 2–9, and Structure is where it earns
its keep: *Group*, *Frame selection*, the four booleans, *Flatten*.

One extra row, in the head slot:

| Row | Cost | |
|---|---|---|
| Make this the key layer | cheap | `designate_key` on the member under the pointer — the layer align aligns *to*, the base operand of a boolean (§15 D113), and, since 2026-08-21, **which member of the selection becomes the mask** (§15 D286). Three consumers now, all reading the one designation. `Alt+Shift`+click is the only way *in* today — but the result **is** announced, which this row claimed otherwise until 2026-08-22 (§15 D287): a key wears `canvas::draw_key_outline`'s 3px `color::KEY` trace of its own outline (§15 D113), and a `Subtract`'s base operand additionally wears the layers panel's stack badge (§15 D244). What is missing is a second door, not a way to see the result |

Align and distribute are deliberately absent — §7.

### 5.9 Locked and hidden layers (panel only, per C4)

Not a kind either, but the state that decides most of a menu. **The whole menu is present and
every editing row is dimmed** while *Unlock*, *Show*, *Rename*, *Copy* and *Zoom to selection* stay
live. A hidden-but-unlocked layer is fully editable and gets an ordinary menu whose *Hide* row reads
*Show*.

**The state is wider than this layer's own flag, and there are two sentences rather than one** (§15
D321). A locked *group* locks its contents, so the rows dim for a layer **inside** one too —
`LayerState::locked_within()`, which every `dim_if` reads. The sentence is *"This layer is locked"* when
the layer's own flag is set and *"A group containing this layer is locked"* when it is not: a layer
inside a locked group is not itself locked, so the first sentence would point the hand at a toggle that
is already off, and the reason a refusal gives has to name the thing that would have to change. For the
same reason *Lock* / *Unlock* is the one row still reading the **own** flag (`LayerState::locked`) —
offering *Unlock* on a layer whose flag is already clear would write nothing and read as a broken
control. Pinned by `menu::tests::a_layer_inside_a_locked_group_dims_with_the_groups_reason`.

**That sentence was an overstatement until 2026-08-23 and is not one now.** §5.7's head carried the
two exceptions: *Export original…*, live because reading a layer's bytes is not editing it, which was
withdrawn from both its doors (§15 D305); and *Edit image*, live only because no door onto the mode
read the lock, which now dims **because the mode itself refuses** (§15 D320). Neither survives, so
the rule above holds of the rows it was written about.

This is the menu worth building first, because it is the one that fixes something currently
unreachable rather than merely faster.

---

## 6. Targets that are not layers

### 6.1 Empty canvas

Figma's page menu, trimmed to what this app has.

| Row | Chord | Cost | |
|---|---|---|---|
| Paste here | `Ctrl+V` | cheap | at the pointer — the reason C3 does not clear the selection |
| Paste in place | `Ctrl+Shift+V` | cheap | ✅ **built 2026-08-20** (§15 D248), and the scoring was right: the placement is `PASTE_OFFSET` replaced by zero, everything else having been built for *Paste here*. **Canvas only, as specced** — a row aimed at a layer would be a third answer to "where", and this one's whole content is refusing to be aimed |
| Select all | `Ctrl+A` | wiring | ✅ |
| Zoom to fit | `Ctrl+1` | wiring | ✅ — and it now means **everything**, unconditionally. It used to fall back to the selection whenever there was one; D209 split that off onto the row above, so these two are genuinely different menu items rather than one written twice |
| Export all | `Ctrl+Alt+E` | — | ✅ **built 2026-08-21** (§15 D274), and **not in the spec** — added because the app grew a document-wide verb it did not have when this table was written. Re-runs every layer's saved export settings into the folder the last export went to. The page is the only target it belongs to: every other menu is aimed at something, and this one is aimed at the *file*. Dims when no layer in the document has export settings, which is §3's rule and here also the only place the feature announces itself to someone who has not opened the panel. It sits in `Group::Export` and therefore lands above the View rows |
| Show rulers ✓ | `Shift+R` | wiring | the five View rows a canvas right-click plausibly wants, in the View menu's own order. ⚠️ **Dimmed in present mode and alone among the five** (§15 D758): `rulers_on()` is `show_rulers && !present`, so ticking it there moves the tick and leaves the bars where they are. The other four are live in present mode by §15 D750, so this is one row whose effect present mode swallows rather than "present mode dims the View rows" |
| Show guides ✓ | `Ctrl+;` | wiring | |
| Lock guides ✓ | `Ctrl+Alt+;` | wiring | the row C7 requires — with guides locked this is the only way back |
| Show grid ✓ | `Ctrl+'` | wiring | |
| Show layout grid ✓ | — | wiring | ✅ **added 2026-09-15** (§15 D757), a session after the switch itself (§15 D755). *Show grid* was in both menus and this one in the View menu alone; the maintainer closed it with *"keep it consistent"*. **Unbound on purpose** (§15 D355's reasoning), so the chord column is empty rather than pending. It wears `icon::COLUMNS`, the glyph the inspector's Layout grid card already draws for `GridAxis::Columns`, so the row and the panel it is about are one picture |

*Show layers*, *Show toolbar* and *Present mode* stay out: they are chrome-wide switches, the View
menu is one click away, and *Present mode* hides the very chrome the canvas menu sits over.

⚠️ *A ⚠️ stood here for one day, naming a fifth View row the app had and this table did not* (§15
D755). It is the last row above now (§15 D757) — the maintainer's answer was *"keep it consistent"* —
and the day it took is the argument for recording an asymmetry rather than leaving it: a question
written into a spec table is one somebody can settle in a sentence, where the same asymmetry left in
`menu::view_rows` is one nobody meets until they open two menus in a row. 🚨 **Nothing in the suite
pinned this table's row set before D757**, so D755 could add a switch to the View menu and leave this
one short with every gate green; the test derives the set from `ViewSwitch::VIEW_MENU` less the three
excluded below, which is why a sixth View row cannot land in one menu only.

*Canvas background…* is a candidate rather than a row. The inspector already shows it when nothing
is selected (`SetCanvasBackground`), so this would be a second door to a control that has one — no
harm, no need, one look on the machine.

### 6.2 Layers panel background

| Row | Chord | Cost | |
|---|---|---|---|
| Paste | `Ctrl+V` | cheap | at the root, below everything |
| Select all | `Ctrl+A` | wiring | ✅ |

Nothing else. *Collapse all* / *Expand all* is already a button in the panel's header, and a
context menu that repeats a control six pixels away is noise.

### 6.3 A guide

| Row | Chord | Cost | |
|---|---|---|---|
| Delete guide | `Del` | wiring | ✅ |
| Clear all guides | — | cheap | a loop of `RemoveGuide`; `build::guides_of` already collects the ones a subtree owns |
| Lock guides ✓ | `Ctrl+Alt+;` | wiring | ✅ Illustrator |
| Show guides ✓ | `Ctrl+;` | wiring | ✅ |

The guide's colour and position stay in the inspector, which shows both for a selected guide and
has the picker beside them.

### 6.4 Inside a text session

| Row | Chord | Cost | |
|---|---|---|---|
| Cut | `Ctrl+X` | ~~feature~~ | ✅ **built 2026-08-18** — `TextEdit::cut` and an arm in the session's own loop (§15 D217) |
| Copy | `Ctrl+C` | ~~feature~~ | ✅ same; both chords did nothing at all until then, which is what made this a bug rather than a gap |
| Paste | `Ctrl+V` | wiring | ✅ the session handles `Event::Paste` — but the **row** was reading the layer clipboard and pasting layers, see §9.6.6 |
| Select all | `Ctrl+A` | ~~cheap~~ | ✅ pure wiring, and this row's cost was **mis-scored**: `TextEdit::select_all` already existed and `Ctrl+A` was already calling it |

Four rows and no more. Type is what the Typography panel is for, and a session already has the
panel open beside it. **All four are built as of 2026-08-18** — the whole of this target, and the
two live gaps §9.7 opened are closed with it.

**Its *Paste* is the one in the whole app that does not make a layer.** Every other *Paste* row ends
in either a captured subtree or — since §15 D218 — a text layer; this one inserts at the caret. That
is why it asks `Context::system_text` where the others ask `can_paste`.

**All four act on the *editor* and share their `Item` with the layer verbs of the same name**, with
`perform_menu_item` routing by target. That is what keeps one label, one glyph and one accelerator
per verb: had the session's *Copy* been given an item of its own it could have drifted to a
different name from the chord that also performs it. It is also the rule `Item::Delete` already
rested on — what a verb acts on is the target's question, not the row's.

Note that these rows do **not** go through the clipboard-event resolution `shortcuts.md` §0
describes — that trap is about keys egui_winit intercepts before the app sees them, and a menu row
is app code calling the editor directly. It is the one place in the app where cut and copy are
reachable without touching `Event::Cut`.

### 6.5 A point selection (node tool)

The subject is the anchors, the segment, or the subpath under the pointer — `selected_anchors`
already answers "a selected segment means its two anchors" for every other reader.

| Row | Chord | Cost | |
|---|---|---|---|
| Add point here | — | wiring | ✅ **built 2026-08-20** (§15 D255), live only when the right-click landed on the ink. `pen_verb`'s *insert an anchor* at the stored open position — the menu knows where it was opened, which is the one thing the keyboard cannot express. The scoring was right; what the build settled is that the gate and the verb ask **one** query (`canvas::segment_at`, wrapping `node_grab`), so a right-click on an existing anchor refuses rather than stacking a point on it |
| Delete points | `Del` | wiring | ✅ |
| Delete segment | `Del` | wiring | ✅ — two verbs on one key today (§15 D120), and two rows here, which is where the distinction can finally be seen |
| Make corner / Make smooth | — | cheap | ✅ **built 2026-08-20** (§15 D250), as **two** rows — unlike *Delete points* above, both can be live at once, since a point selection may hold a corner and a smooth point together. The scoring was half right: the builder is small, but `PenAnchor::smooth(at, out)` needs an `out` and a corner has none, so the tangent is derived from the chord `next − prev` and **both handles get one length** — an unequal pair is not `is_smooth()` here, so it would come apart on the next drag |
| Reverse subpath | `Shift+D` | wiring | ✅ §15 D125. **Named *Reverse subpath direction* until 2026-08-20**, when it was found running *under* its own accelerator — the label and the accelerator are painted at fixed anchors, so a long label overlaps rather than clipping (§15 D262) |
| Join | — | cheap | ✅ **built 2026-08-20** (§15 D256). Joining existed only as a **pen gesture** — draw *into* an open endpoint — which cannot be aimed at two ends already where they belong, so this row did need an entry point of its own: `tools::joinable_ends` and `join_ends`, over the point selection rather than the pointer, sharing `join_runs` with the gesture. Two ends of one run close it, ends of two runs splice into the **lower** subpath index, and coincident ends are **not merged** |

### 6.6 A ruler — ~~reserved~~ struck

This reserved the ruler as a target on the grounds that **right-click on a ruler is where both Adobe
apps put the unit menu**, and that the rows would be the units `roadmap.md` had not decided on. Units
were decided on 2026-08-26 and there is exactly one: px, for ever (§15 D358). A menu whose only row is
the unit you already have is worse than no menu, so **the reservation is struck rather than filled** and
the ruler claims no region at all.

What the reservation was really protecting still stands and is why this paragraph is kept rather than
deleted: **nobody should wire the canvas menu to the bar.** The ruler is not the canvas, and a canvas
menu appearing over it is the kind of thing that is hard to take back once hands learn it. If a ruler
ever earns a menu it will be for guides — *Lock guides*, *Clear guides* — which is a different list
with a different reason, and it is not reserved here.

---

## 7. Deliberately not in these menus

Recorded so they are not re-argued, the way `shortcuts.md`'s *Deliberately unbound* is.

- **Align and distribute.** None of the five apps put them in a context menu — they are panels
  everywhere — and the reason is visible as soon as it is written out: six aligns and two
  distributes is eight rows for a control the inspector already draws as one legible cluster, with
  `Alt`+letter on every one of them (`shortcuts.md` §5). If a submenu is ever built, this is the
  first thing to reconsider, and only then.
- **Reveal in layers panel.** Specified here, built, used, and **removed the same day** — which is
  why it is recorded rather than quietly deleted. The row is redundant in almost every open:
  selecting on canvas already scrolls the tree to the row and opens its ancestors
  (`sync_tree_to_selection`, which runs every frame the panel draws), so by the time the menu is up
  the panel is *already* showing what the row promises to reveal. It earns its place only when the
  panel is shut — and a menu row that is worth having in one configuration out of two, for a panel
  a chord toggles, is not worth the line it costs in a menu whose ceiling is a scrollbar (§3). If
  it comes back it should come back as "show the layers panel", which is a different row with a
  different name.
- **Select all with same…** — `shortcuts.md`'s reasoning stands: no convention in any of the five
  apps, and it belongs in the palette, where the registry can enumerate *which* "same".
- ~~**Mask.**~~ **A row since 2026-08-21 — this entry is reversed** (§15 D286). It sat here first
  because nothing in the model expressed a mask, and then, for a day, because the verb is a *toggle*
  and a toggle looked like a poor fit for a row. The second reason was wrong on this file's own terms:
  *Clip content* is a toggle and has had a **checkable** row all along, so the shape was already
  spelled here. *Use as mask* is that row — checked while the layer is a mask or holds one, absent
  where the verb has no answer. Struck rather than deleted, because the lesson is that a non-row
  argued from a *mechanism* expires when the mechanism does.
- **Blend modes, effects, components, auto layout.** Deferred in §1. A menu row is not the place a
  deferred feature first appears.
- **Paste to replace** (Figma). Attractive, and it needs a rule for what "replace" means when the
  clipboard and the target hold different numbers of layers. A candidate, not a v1 row.
- **Collapse all / Expand all**, **Keep proportions**, **the four image fit modes**, **guide
  colour**, **corner radius and side counts** — each already has a control that is better than a
  menu row, and each is named above where it belongs.

---

## 8. The chrome this needs

### The design export's menu, and how far it is normative

`design/Editor.dc.html` now draws one of these — a canvas right-click menu (`[data-canvas-menu]`),
placed at the pointer inside the canvas rect. **It is normative for style and for nothing else.**
Its eight rows are a sample rather than the content: §§4–6 are the content, and the export's own
order disagrees with §3 in two places, putting Order before Structure and *Delete* last in a group
of its own. Where the two disagree about *what a row is or where it sits*, this file wins; where
they disagree about *what a row looks like*, the export wins.

⚠️ **One recorded deviation from that last clause, and it is about a glyph** (§15 D761). The export
gives *Bring to front* `ph-stack` and *Send to back* `ph-stack-simple`; the app draws
`arrow-line-up` / `arrow-up` / `arrow-down` / `arrow-line-down` across the four Order rows. The
reason it is taken rather than deferred to is that **the export's own answer does not remove the
duplication it was chosen over, it moves it**: `ph-stack-simple` is *Flatten* in the same export, and
`restack_rows` emits the Order four unconditionally while *Flatten* is pushed on any 2+ selection, so
the two co-occur. Of the three candidate schemes the arrow family is the only one with no picture
used twice. §4's *Order* table lists no icons and is untouched.

**Most of it is already built.** Measured against the export, `ui::menu_frame` is exactly this card
and not approximately it:

| | Export | App |
|---|---|---|
| Fill | `text 16%` over `#000` | `color::CARD` — the 16% tier |
| Hairline | `0 0 0 1px` at `text 6%` | `Stroke::new(1.0, text_a(15))` — 15/255 = 5.9% |
| Shadow | `0 10px 24px rgba(0,0,0,.32)` | offset `[0,10]`, blur `24`, `from_black_alpha(82)` — 82/255 = .32 |
| Corner / padding | `5px` / `5px` | `CornerRadius::same(5)` / `MENU_PAD` = 5.0 |
| Row gap | `1px` | `item_spacing.y = 1.0` |

and `ui::menu_item` is exactly this row: an 11.5pt label, a 4pt hover ground at
`text_a(20)` against the export's `text 8%` (20/255 = 7.8%), and a resting glyph at
`theme::text::DIM` against the export's `text 45%` — `DIM` *is* `text_a(115)`, 45%. The design and
the code were built off the same numbers and they have not drifted, which is why the list below is
short.

**Two numbers are deliberate departures, and both were taken on the same day.** The **row height**:
the export says 26 and the app allocates `ui::MENU_ITEM_H`, which is **24 since 2026-08-20**, after a
report that the menus had grown too tall (§15 D262). Two points off every row of every menu, so the
top bar's View, Snap and zoom menus moved with these — the constant used to be written out three times
and is now one. And the **separator's air**: the export's `margin: 4px 0` is **3 since 2026-08-20**
(§15 D263), asked for once the row change had been seen, so a separator costs 7 rather than 9. The
rule itself is still the export's 1pt — the hairline is the thing being seen and the air is only what
it costs. Between them a primitive's menu went 646 → 592. **And the label and the
accelerator are painted at fixed anchors with no layout between them**, so a label too long for its
row does not clip or wrap, it overlaps the accelerator, silently and only on the row that is too long.
That is why the registry is tested for it (`no_menu_label_runs_into_its_accelerator`) rather than
guarded at the draw.

**What the export adds** — the same items this section already called missing, now with values
instead of intentions:

- The **accelerator column** is `10.5pt` at `text 38%`, which is `theme::text::FAINT` exactly. This
  file first guessed `DIM`; the export says one tier further down, and the constant for it exists.
  (True of every *live* row. A **disabled** row's three inks moved down together on 2026-08-19 — 79 /
  65 / 49 for glyph, label and accelerator — so that its glyph could clear the "switched off" greys
  without collapsing into its own label; §15 D235.)
- The **separator** is 1pt at `text 8%` — `text_a(20)`, the hover ground's value, **not** the
  `text_a(15)` this file first wrote — with 4pt of air either side (`margin: 4px 0`). The app draws
  **3** of air, per the paragraph above; the rule is the export's.
- A **destructive row**: *Delete* paints its glyph, label and accelerator `#e0736b` (the
  accelerator at 55% of it) and keeps the ordinary neutral hover ground. That colour appears
  **exactly once in the entire export**, on this row, and the app has no red at all — `theme::color`
  has `WARN` (`#e0a04a`) and nothing else warm. So the menu introduces a palette entry, and it
  belongs in `theme::color` beside `WARN` rather than at the call site.
- **210pt wide**, against the top bar's dropdowns at 160. The accelerator column is what buys the
  extra 50, and a row like *Make this the base operand* needs it whether or not it carries a chord.
  The app's 210 is the width the card **paints**, border and all, since 2026-08-23; it was the
  **content** width and the card painted **212**, the frame's border sitting outside it (§9.6.7,
  §15 D307).

Note what the export does **not** answer, because it is a static mock: it has no Escape handling, no
second menu to be replaced by, and no edge flipping — its menu is a plain `left`/`top` at the
pointer. Those are §0's R3 and R4 and the bullet on edge flipping below, and none of them can be
read off a picture.

### What the app already has

The app has three quarters of the menu already: `ui::menu_frame` (the floating ground),
`ui::menu_item` (glyph + label + hover ground), `ui::menu_check` (a checkable row that keeps its
tick column), `inspector::menu_action` (dim + say why), and `OndinApp::dropdown`, which owns **the
two ways every menu closes** — a click outside and Escape. Reuse that last one rather than writing
a fourth copy of the click-away rule; `panels::dismissed_by_click` is the record of what happens
when the same six terms get hand-rolled per popover. Reuse it **with R4's two amendments**, which
are what a headless menu needs and a headed one does not: the frame that opened the menu is exempt
(there is no head to exempt in its place), and a secondary click landing outside opens the next
menu into the same slot rather than merely closing this one.

What is missing:

- **An accelerator column on `menu_item`.** Right-aligned, `10.5pt` at `theme::text::FAINT` — the
  export's value, read above — from the registry. A context menu is where people learn shortcuts,
  and a menu without them is a menu that has to be opened forever. It is also what takes the card
  from the dropdowns' 160pt to the export's 210.
- **A destructive row.** `#e0736b` on the glyph, the label and the accelerator, the last at 55%,
  over the ordinary hover ground. New to the app's palette, so it wants a name in `theme::color`
  beside `WARN` rather than a literal at the one call site that uses it today.
- **A separator.** Nothing in the app draws one; the dropdowns set `item_spacing.y = 1.0` and rely
  on rows. Nine groups need a hairline — 1pt of `theme::color::text_a(20)` with 4pt of air either
  side, which is the export's `text 8%` and `margin: 4px 0`, and **not** the `menu_frame` stroke
  value this file first assumed. Spacing alone reads as one long list.
- **No submenus in v1.** Nothing in the app has one, and hover intent, click-through and keyboard
  traversal are all real work. The two candidates are answered another way: the boolean four are
  four rows, because they are the headline structural verb and cost four rows; align is not in the
  menu at all (§7). *Copy as ▸* is the first thing that will genuinely want one.
- **Edge flipping.** `egui::Area::fixed_pos` does not flip: a menu opened near the bottom or right
  of the viewport must open up and to the left, and one taller than the viewport must clamp. This
  is a numbers question and therefore a headless probe question — see §10.
- ✅ **Keyboard navigation. *Built 2026-08-19*** (§15 D223). `↑`/`↓` move the highlight, `Enter`
  activates, `Escape` closes (R3), `Home`/`End` jump. No type-ahead in v1. This is the one keyboard
  claimant that must **not** go through `input::resolve` — R3 turns that off while the menu is open —
  and holding the keyboard turned out to have a second half this file did not name: the five keys are
  *consumed* in R3's own gate, at the top of the frame, so no widget drawn later can read an `↑` the
  menu is using. Three rules the paragraph above does not state came out of building it, and D223 is
  where they live: the highlight is a **verb rather than an index**, only rows a press could **run**
  are reachable, and the keyboard and the pointer share **one** highlight channel.
- **Glyphs.** Every `menu_item` row carries one, so the registry needs about fifteen new Phosphor
  picks. Reuse what exists — `layers::op_glyph` for the booleans, `layers::node_icon` for the
  kinds, the tool rail's for the pen — and name them in the registry rather than at the call
  site, so the same verb cannot be two pictures in two menus.

---

## 9. Ledger

### 9.1 Wiring only — the op or builder exists and a call site is missing

Cut · Copy · Duplicate · Delete · Group · Ungroup (Group and Boolean only) · the four booleans ·
Flatten · the four z-moves · Flip horizontal/vertical · Hide/Show (`SetVisible`) · Lock/Unlock
(`SetLocked`) · Clip content (`SetClip`) · Rename (the panel's field) · the boolean op switch
(`set_boolean_op`) · Edit text · Edit points · Enter group · Edit image · ~~Reset crop~~ · Original
size · ~~Replace…~~ · ~~a frame's *Background…*~~ — **the one row in this bucket that never landed,
and struck on 2026-09-19 as a decided non-goal** (§5.1, §15 D800; the op it was scored as wiring for,
`SetArtboardBackground`, had itself been deleted by §15 D400, so the bucket was wrong about it as
well as unspent) · the base-operand and key-layer designations
(`designate_key`) · Delete guide · Lock/Show guides · Show rulers/grid · Select all · Zoom to fit ·
Add point here · Delete points · Delete segment · Reverse subpath · the three text sizing
modes (the Type panel's own segmented control).

**The three sizing rows were added to this list on 2026-08-20, having never been in it** — and they
were not in §9.5's *not landed* line either, so an unbuilt row sat in no bucket at all and could not be
counted from either end of this ledger. §5.6 had scored them *wiring* and that was right, with one
qualification the build added: the wiring was a **lift**, the body having lived inside the panel's
segmented control until `OndinApp::set_text_sizing` was split out of it so that both doors are the one
verb (§15 D261).

***Edit image* was added to this list on 2026-08-22, on the day it was built** — it had never been in
the ledger at all, because §5 had scored it a decided *non-goal* rather than an unbuilt row, and a
non-goal has no bucket here. That is the second late arrival after the sizing rows above, and the
two are different failures: those sat unscored while genuinely unbuilt, this one was scored and the
score was *decided against*. *Wiring* is the right bucket in retrospect: `choose_tool` and
the mode were built, and what was missing was a call site — five sites of one, which is what
`roadmap.md` had corrected the "one registry entry and one arm" estimate to the day before.

***Reset crop* and *Replace…* are struck above because they left the menu on 2026-08-23, not because
they were unbuilt** (§5.7, §15 D305). Both are wiring that shipped and then moved: the verbs are
image editing's Settings tab alone now, and a row this file scored and specified is worth keeping
visible as a withdrawal rather than deleting — the same treatment §4 gives *Export as…*. Struck
in place, so the bucket still shows what the scoring got right.

***Add point here* was the last of this list as it then stood to land, on 2026-08-20 (§15 D255), and the
scoring held**:
`insert_point` was already reached by the pen bias and by a double-click, so what was missing was the
call site — plus one four-line query, `canvas::segment_at`, which the gate and the verb share so that
the row is dim exactly when taking it would insert nothing.

### 9.2 New but cheap

Paste here (paste-at-pointer — and this row is now the *only* door onto it: `shortcuts.md` §7
carried a `Ctrl+V` row wanting the same thing and it was decided against 2026-08-22, §15 D300,
which is what makes this one load-bearing rather than a convenience) · ~~Paste in place~~ — **built
2026-08-20 (§15 D248), and the scoring was right twice over**: the row is four lines and the
placement is one constant swapped for zero. What it also settled is that `shortcuts.md` §7's
"deferred as a group" covered two rows it did not bind — this one is `Shift`, read inside the
existing Alt guard, and never needed the branch the two property rows do · ~~Copy properties
· Paste properties~~ — **built 2026-08-20 (§15 D257)**, and *cheap* was right about the rows and wrong
about where the work was: the payload is three fields and the paste is three existing bulk verbs
composed, and what it cost is deciding what the payload **excludes** and reading `Ctrl`+`Alt`+`V` off
the key *release* rather than off `Event::Paste`, which no reading of the row would have produced ·
Zoom to selection (`Action::ZoomSelection`, §3 owes it) · Reset origin
(`SetPivot(None)`) · Clear all guides · ~~Export original…~~ — **built 2026-08-19 (§15 D225), and the
scoring was right: `ImageFormat::extension` had no caller until the inspector's Asset section got one,
and this row was then four lines. Removed again 2026-08-23 from both doors (§15 D305), which put that
function straight back to zero callers — `dead_code` never fires on a `pub` item in a library, so the
scoring premise and the rot it named are the same fact seen twice** · ~~Select all (text session)~~ — **mis-scored: it was
§9.1 wiring all along, `TextEdit::select_all` existed** · ~~Make corner / Make
smooth~~ — **built 2026-08-20 (§15 D250)**, and *cheap* was half right: the builder is small and the
**handle it has to invent** is a decision, which running it settled rather than reading it ·
~~Join as a command~~ — **built 2026-08-20 (§15 D256)**, and *cheap* was right about the size and wrong
about where the work was: the two builders are under forty lines, and what they cost is three **refusals**,
of which "the two ends of a two-anchor run cannot be closed" is one no reading of the row would have
produced.

### 9.3 Features

~~Frame selection~~ — **built 2026-08-20 (§15 D249)**, and it belonged in *Features* rather than in
*New but cheap*: the row is four lines and the *builder* is a hundred, because a frame has a box and a
group does not · ~~Convert text to path~~ — **built 2026-08-20 (§15 D260)**, and *Features* was wrong
for a reason worth keeping: it scored a **dependency** ("glyph outlines have to come out of `skrifa`
and through `geometry`"), and by the time anyone came to build it they already had —
`ondin_core::text::outline` has existed since outside-aligned type strokes did (§15 D145), so the row
was wiring plus one refusal. A scoring that names a missing part goes stale the moment something else
needs the same part, and **nothing recompiles when it does**; the entries left in this section and the
next are both scored that way · ~~Copy as SVG~~ — **built 2026-08-20 (§15 D259)**, and
*Features* was right about where the work was: the row is a registry entry and one arm, and the
*writer* is new — `svg_of` takes a **set** of origins where `svg` took one optional artboard, with a
viewBox over the union of their world bounds. ~~***Copy as PNG* is scoped out, not deferred**~~ —
**built 2026-08-22 (§15 D259's other half)**, and the two halves of that scoping expired one after the
other. The **mechanical** half read that the raster path is viewport-scoped — `png(doc, res,
&Viewport)`, a `view` rect and a pixel size, with no way to say "only these layers" — and that building
it meant render overrides hiding everything else or a second entry point. It was the second entry
point, and *Export as…* built it on 2026-08-20: `scene::build_of` walks a set of origins and `png_of`
frames them (§15 D264), so a PNG of the selection paints neither its neighbours nor the ground. What
was left was the scoping **proper**, which was never mechanical — §3's Export group listed two rows and
a PNG was not one of them — and it came down to one open question: whether the clipboard wants a
*picture* of the artwork beside the editable copy at all. **It was asked and answered *yes*, and §3's
table was amended to three rows** — two again since the file row went the same day (§15 D264), which
does not touch this answer. So this entry closes the way the two above it do and by a third road: not
a dependency that rotted and not a model that grew, but a **question** that was put and decided, once
the renderer had stopped answering it for us. The row is the one line this entry last called it; the
work is in what it does *not* call (`png_of`) and what it does not write (`clipboard_stamp`) ·
~~Cut and Copy inside a text session~~
— **built 2026-08-18**, two functions in core and two arms in the session's loop, which is less than
this scoring feared (§15 D217) · ~~Mask (nothing in the model)~~ — **the feature landed 2026-08-21
(§15 D282) and the row with it the same day** (§15 D286): *Use as mask*, checkable, in Structure after
the group verbs. The scoring named a **model** dependency, which is the one shape of entry in this
ledger that expires without anything recompiling — and this is the second time that has happened here,
after *Convert to path*'s `skrifa`.

### 9.4 Blocked, and on what

~~*Export selection…* and *Export frame as…* — on there being an export UI (`Ctrl+Shift+E` is unbuilt;
`ondin export` is a CLI).~~ **Both closed 2026-08-20 and neither by the door this named** (§15 D264):
the blocker was a *dependency* premise and it had rotted — `svg_of` and five `rfd` save dialogs were
already in the tree, and what was actually missing was `scene::build_of`, a raster walk that can name
layers. The first was **built**, as *Export as…* and on this very chord; the second was **subsumed** by
it, §5.1's row being the same call over the same subject. ⚠️ **Two days later the first was removed and
the chord unbound** (§15 D264): the inspector's Export panel answers where, what format and at what
size from the layer, so the menu's one-off had become the slower of two doors. Neither row comes back —
§5.1 explains why the subsumption survives its subsumer — but this entry now records a row that was
unblocked, built and then withdrawn, which is a shape no other entry in this ledger has. Struck rather
than deleted because the *shape* is the lesson: this section's entries are premises about the tree, and
nothing recompiles when one stops being true. ~~The ruler's unit menu — on units.~~ **Closed 2026-08-26,
and closed by the decision going the other way** (§15 D358): units were decided against, so the rows
this was waiting for will never exist and §6.6's reservation is struck. That makes **two** of this
section's entries closed by something other than the thing they named, and this one is the sharper
lesson — a blocked entry can be unblocked by its dependency *arriving* or by its premise being
*refused*, and only the first leaves anything to build. **§9.4 is now empty.** ~~*Outline shape* — on the decision
`roadmap.md` is holding, not on any code.~~ **Unblocked 2026-08-19: the decision was taken — allow it
— and the row is built** (§15 D230). It was the only entry in this section blocked on a *decision*
rather than on code, which is why it is the only one that could close without anything else landing
first.

### 9.5 What landed, and what did not

**Landed**: all of §9.1 **except a frame's *Background…*, which is struck rather than outstanding**
(§15 D800), and from §9.2 *Paste here*
(paste-at-pointer), *Zoom to selection*,
*Reset origin*, *Clear all guides* and *Select all* inside a text session.
⚠️ **This said "all of §9.1" until 2026-09-07 and the row it counted was never built** — and the
operation it was scored as wiring for had been deleted (§15 D400), so the bucket was wrong about
the row twice over. It is still named here rather than quietly dropped from the sentence, now that
it is a decided non-goal, because a bucket that reports itself empty is what stopped anyone
checking. All four rules of §0. The
chrome of
§8, all of it: the accelerator column, the separator, the destructive red, edge
flipping and clamping. All six targets of §2 and §6 except the ruler — and the ruler is no longer an
exception waiting to be closed but a target that will never exist, §6.6's reservation having been
struck when units were decided against (§15 D358). §6.1–§6.5 are the whole list.

**Landed 2026-08-18, after this ledger was first written**: *Cut* and *Copy* inside a text session —
scored as features in §9.3 and cheaper than that (§15 D217) — and with them the whole of §6.4,
including the repair of its *Paste*, which had been reading the layer clipboard (§9.6.6). §9.7's two
live gaps close here: the first is fixed, and the second — reserved in §6.6 — closed on 2026-08-26 by
being struck rather than built (§15 D358).

**Landed 2026-08-19**: **keyboard navigation** (§8's `↑`/`↓`/`Enter`/`Home`/`End`), which was the one
piece of §8 scoped out rather than deferred by a dependency, and therefore the last of that section.
What it added beyond the four keys is in §15 D223 and summarised on §8's own bullet.

**Landed 2026-08-19**, as well as the keyboard: ***Export original…*** (§15 D225), which arrived from
the other direction — the inspector's Asset section built the action and this row was then a registry
entry and one arm, which is exactly what §9.2 scored. **It is gone again since 2026-08-23** (§15
D305), from this door and the inspector's together, so it is the second row in this ledger to be
built and then withdrawn after *Export as…* — and the two went for one reason, that the Export panel
answers "export this" better than a one-off row does. And ***Outline shape*** (§15 D230), the one row
in this whole ledger that was blocked on a **decision** rather than on code: it was taken (allow it),
and the row is `build::outline` — a second verb, so *Flatten* still refuses a lone non-boolean and the
two are never offered together.

And ***Paste in place*** (§15 D248), which §6.1 placed directly under *Paste here* and which is now
there — the pair being the two answers to "where", one taken from the pointer and one refusing to be
taken from anywhere. And ***Frame selection*** (§15 D249), in §4's slot directly after *Group
selection*, which is the first row from §9.3's *Features* list to land.

And ***Make corner* / *Make smooth*** (§15 D250), §6.5's pair, as two rows rather than one that
renames itself — the distinction from *Delete points* being that both of these can be live at once.

And, later the same day, ***Add point here*** (§15 D255) and ***Join*** (§15 D256), which finish §6.5
— **the first section of this file with no unbuilt row in it**. They landed together and are opposite
in shape: the first is the last of §9.1's wiring, a verb that existed reached through a query the gate
and the verb share, and the second is a *second door* onto a behaviour that had only ever had a
gesture. What the pair settled beyond the rows: within a group the **emitted** order decides, so a
row's group buys a hairline and not a position (D255); and a join **merges nothing**, leaving
coincident ends as two anchors with a zero-length segment between them, exactly as the pen does
(D256).

And, last of all on that day, ***Copy properties*** and ***Paste properties*** (§15 D257) — **which
empties §9.2**, so *New but cheap* has no unbuilt row left in it either. They also discharge
`shortcuts.md` §7's "deferred as a group" outright: D248 took two of the four rows out from behind
that blocker by showing it never bound them, and these two are the ones it did bind. What the pair
settled beyond the rows: the payload is fills, strokes and opacity and nothing else, geometry being
excluded twice over; *Copy properties* refuses a **group** rather than lifting an empty appearance,
which would arrive at the paste as an instruction to clear; and `Ctrl`+`Alt`+`V` has to be read from
the key **release**, because `Event::Paste` exists only when the OS clipboard holds text and a
property paste has nothing to do with it.

And, later still on that day, ***Copy as SVG*** (§15 D259) — **the first row §3's Export group has
ever had in the code**, and — for a few hours — the only way artwork leaves this app without the CLI,
which is why it was worth having before the file rows. **It also unblocked them**: `svg_of` is what
turned "waits on an export UI" from true into false, and *Export as…* landed the same day on top of it
(§15 D264). What it settled beyond the row: the writer
takes a **set** of origins (`svg_of`), and the caller owes it in document order because SVG has no
z-index; there is deliberately **no `clipboard_stamp`**, which is what correctly makes
`owns_the_clipboard` answer *no* afterwards; and with SVG text on the clipboard `Ctrl+V` pasted it
back as a **text layer** (§15 D218, D221), which was what the existing rules already said and was the
one place SVG *import*, whenever it came, would meet them. ✅ **It came on 2026-08-31** (§15 D394):
markup on the clipboard is read as layers now, between the app's own payload and that text-layer
fallback, so this row's round trip closes for everything but `<text>`. *The prediction was right about
where — the change was to the paste path and not to this row, which is untouched.* Building it also found §15 D258 — a scoped
export applying the origin's own transform twice, green for as long as it had been wrong because the
only caller of that path was a test asserting a `<rect>` was *present*.

And, on that same day, ***Convert to path*** (§15 D260) and the **three sizing rows** (§15 D261) —
**which empties §5.6**, the second section of this file with no unbuilt row in it after §6.5. They are
opposite in shape and each taught this ledger something about itself. The first was scored a *feature*
on a **dependency**, "glyph outlines have to come out of `skrifa`", which had since arrived by another
road: `text::outline` has existed since outside-aligned type strokes did (§15 D145), so what was left
was `build::outline_text` — a **third** verb beside `flatten` and `outline`, and the one of the three
that needs `Resolved`, text's outline being a *shaped* thing — plus the refusal that stops a text node
with no contours offering a conversion which fails after the click. The second was scored *wiring* and
was wiring, but it **was not in §9.1's list**, nor in the line below, so it sat in no bucket at all;
§9.1 names the three rows now. What the pair settled beyond the rows: the convertibility gate is a
**snapshot** taken at open, because asking the question *builds the outlines* — about a microsecond a
glyph — and a live read would pay that every frame the menu is up; and the sizing verb is a lifted
method, `OndinApp::set_text_sizing`, so the row and the panel's segment cannot come to mean different
things.

**Not landed, and each is named where it belongs rather than half-built**: what was left of §9.3 and
§9.4 was ***Mask*, and only that** — and **that closed on 2026-08-21, as a row** (§15 D286). *Use as
mask* is in Structure after the group verbs and before the booleans, checkable off `LayerState::masked`
and absent where the verb has no answer. That empties §9.3; §9.4's last entry was the ruler's unit
menu, and **that emptied too on 2026-08-26** — not by being unblocked but by units being decided
against, so there is nothing left in either section (§15 D358). Both *Export* rows closed on 2026-08-20
and neither closed the way
this line expected: *Export selection…* was **built** (§15 D264) once its "waits on an export UI"
turned out to be a rotted dependency, and *Export frame as…* was **subsumed** by it rather than built,
§5.1's row being the same call over the same subject. ⚠️ **The built one was then removed on
2026-08-22** (§15 D264 again), the inspector's Export panel having made a menu row that asks three
questions and remembers none of them the slower of two doors; the subsumed one stays subsumed, by the
panel now rather than by its sibling. Neither is an open item — a withdrawn verb is a decision, not a
gap. ***Copy as PNG* was not on that list** either —
§9.3 scoped it out on the grounds that the raster path is viewport-scoped — and that ground went with
D264, `scene::build_of` being a raster walk that names layers. What was left of the scoping was one
question, whether the clipboard wants a picture of the artwork beside the editable copy at all; **it
was put and answered *yes*, and the row landed 2026-08-22** (§15 D259's other half). §3's Export group
went to three rows, and this file's table was amended to the code rather than the other way round,
which is not the direction this file usually moves in; §5.5 says why it was right here. **It is two
rows again**, the file row having gone the same day — so the group is now the clipboard pair, and the
one row of the three that ever wrote a file is the one that is no longer there. What the row settled
beyond itself: the
clipboard wants **raw RGBA**, so `raster_of` and not `png_of`, a mistake that compiles and pastes as
noise; and the decision is lifted into `OndinApp::png_for_the_clipboard` so it can be tested at all,
`copy_as_png` ending in the OS clipboard.

### 9.6 Where the code differs from this file, and why

Sixteen — twelve until 2026-09-08, when items 13 to 16 joined as four more bugs, every one found the
way item 12 was and every one closed the same day (§15 D529, D530, D533, D558). **Seven are
deliberate**, eight are bugs this
file's rows exposed in code older than they are, and the last was a live divergence that closed on
2026-08-23. ⚠️ **This paragraph read *Fourteen* while item 15 was already below it**, added the same
day without moving the count with it — which is, for the third time, the exact failure the two
paragraphs below say a list stating its own length exists to catch. The number did not catch it;
writing item 16 did. The third became one of the bugs on
2026-08-19, when building it found the door being inferred rather than read (§15 D226), and is struck
rather than deleted for what its *wording* got wrong. (It read *Eight* and *four* until 2026-08-22,
when item 9 — added on
2026-08-21 without moving the count with it, which is the failure a list that states its own length is
for — took it to *Nine* and *five*; the old item 8 then went with *Export as…* the same day and put it
back. **Two numbers that return to where they were are not two numbers that never moved**: one item
left this list and another joined it, and the eight below are not the eight of two days ago.)

⚠️ **And it read *Eight* and *four* again until 2026-09-07, having missed three rows.** Items 9 and 10
below are *Even-odd fill*, *Flip to other side* and *Detach from path* — all three shipped, all three
with §15 entries, none of them in this file's tables or in this ledger, and the oldest of them a week
old when it was found. Item 11 is the row the second pair was specified against, deleted the day after
it shipped. That is the failure
the paragraph above says a list stating its own length exists to catch, and it did not catch it: the
count was written once and then read as a heading rather than as an assertion, which is what a number
does when nothing recounts it. **It was found by walking `Item::ALL` against §§4–6 one entry at a time**,
and that comparison, not this number, is the check — the two are not the same length and never will
be, so a total on either side cannot stand in for it.
Item 8's own closing line said it: *a row in the code that is not in the table is exactly the drift §9
exists to catch, and finding it in the code first is how the table stops being the record.* It has now
happened four times.

1. **§6.5's two Delete rows are one row that renames itself.** *Delete points* and *Delete segment*
   cannot both be offered: `PointSet` holds anchors **or** segments and never both, so one of the
   pair would be dim on every open — which §3 itself calls indistinguishable from broken. The row
   takes whichever name the selection has armed, which shows the distinction where §6.5 wanted it
   shown and cannot offer the unreachable half.
2. **A checkable row has no tick column; its own glyph goes accent.** §5.3 asked for `menu_check`'s
   tick *and* `op_glyph`'s mark, and 210pt does not hold both columns. So a checked row paints its
   glyph in `ACCENT` — where `menu_check` puts its tick — and keeps the second signal too, the label
   full strength on and muted off. Two channels for one bit, exactly as the View menu has.
3. ~~**The panel's *Paste* is not yet "as a sibling above this row"** (§1's table).~~ **Closed
   2026-08-19 (§15 D226).** It pasted by the ordinary rule — back where it came from, just above the
   original — and the reason was not a missing refinement but a **field being read for the door**:
   `perform_menu_item` matched `Item::PasteHere` on `Option<Point>`, and a panel row has no world point
   by construction, so it fell into the chord's arm and could never have done anything else. It matches
   on `Target`'s `Door` now. Kept in this list because "live and correct, only the placement is
   missing" is how it read from the outside, and it was neither.
4. **The *Rename* row opened the field without putting the caret in it**, and so did `Ctrl+R` and
   `F2` — `rename_selection` set `renaming_layer` and asked for no focus, so the panel looked ready
   to type into and the first keystroke went to the canvas. A pre-existing bug in the chord that
   this file's row made visible, because a menu row is the more discoverable of the two doors.
   Fixed at the moment the rename opens, which is where `layers.rs`'s double-click already asks and
   where its comment records why asking from inside the field instead is a trap.
5. **§3's nine groups are ten in the code, and the tenth is not one of §3's.** The five View switches
   §6.1 and §6.3 carry between them — four until *Show layout grid* landed on 2026-09-15, §15 D757 —
   are about the *page* where §3 is a *layer*'s menu, so they have a
   `Group::View` of their own, last, which is where §6.1 already put them. That is now the **only**
   difference between the two lists. It was **eight** in the code until 2026-08-20, `Group::Properties`
   and `Group::Export` existing in §3 and nowhere else; both filled that day, Properties with §15
   D257's pair and Export with *Copy as SVG* (§15 D259). ~~*Export* is nine-tenths empty even so — its
   other row is §9.4, blocked on there being an export UI.~~ **Both of its rows were built that same
   day** (§15 D264), so this list's last incomplete group became complete; it holds a different pair
   now — *Copy as SVG* and *Copy as PNG*, the file row withdrawn on 2026-08-22 — and is still full. The
   argument the struck
   sentence carried still stands on its own and is worth keeping for the next empty group: an empty one
   costs a variant and collapses by itself, so it is never worth dropping to save a line.
6. **§6.4's *Paste* was dimmed by the wrong clipboard and pasted the wrong thing** — a bug in the
   row as first written, not a deliberate difference. It read `Context::clipboard_full`, the in-app
   clipboard of captured *subtrees*, and dispatched `Item::PasteHere`, whose canvas arm is
   `paste_at(world)`. So text copied in a browser offered a dead *Paste*, and a layer copied first
   offered a live one that pasted **layers onto the canvas** from inside a text session. Both halves
   are now `ContextMenu::system_text` — the OS clipboard's text, read once at open. **It is the one
   piece of a menu that is a snapshot rather than rebuilt per frame**, because reading it means
   opening an OS clipboard handle and doing that sixty times a second fights other applications for
   the lock; a clipboard that changes while a context menu is open is not a case worth serving live.
7. ~~**The card paints 212 wide where §8 says 210, and that one is still open.**~~ **Closed on
   2026-08-23: the card paints 210** (§15 D307). `MENU_W` was used as the *content* width
   (`ui.set_width(MENU_W - MENU_PAD * 2.0)`) with `menu_frame`'s border outside it, a `Frame`'s stroke
   growing its outer rect. The **placement** had been fixed on 2026-08-20 — `menu_place` handed
   `MENU_W + MENU_BORDER * 2.0`, `menu::height` adding the border too, which is §15 D263's bug — and
   the drawn width left alone, because narrowing every menu by two points was a visual change nobody
   had asked for. The arithmetic this item sketched is what was done:
   `set_width(MENU_W - MENU_PAD * 2.0 - MENU_BORDER * 2.0)`, now spelled once as
   `ui::menu_inner_w(MENU_W, MENU_PAD)`, with `no_menu_label_runs_into_its_accelerator`'s `inner`
   moved along with it. **The call was made at every call site in the app rather than at this one**,
   because the divergence was never context-menu-specific: the top bar's dropdowns painted 152 and
   162 against 150 and 160, and every `app::POPOVER_W` card painted 274 against 272 — eight call sites
   in all. Measured over all 36
   accelerator-carrying rows *before* the change, since a 2pt narrowing is exactly the size of thing
   that flips an assertion nobody re-runs: the tightest row, *Paste properties*, went from 19.69pt of
   clearance to **17.69**, against a bound of 4. `menu_place` is handed a plain `MENU_W` now — the
   placer and the drawing are one number instead of two kept two apart.
8. **The canvas menu carries a row this spec never listed: *Export all*** (§6.1, §15 D274). Added
   2026-08-21 because the app grew a document-wide verb after this file was written — one that is
   aimed at the file rather than at anything under the pointer, which is the first of its kind here
   and is why §6.1 rather than §3 is its home. It wears `icon::EXPORT`, which was justified here as
   the same glyph as *Export as…* — deliberate rather than §8's "same verb, two pictures" failure,
   because it is the *same* verb at a different subject and the two can never share a menu, that row
   being on a layer and this one on the page. ⚠️ **That partner went on 2026-08-22** (§15 D264), and
   the argument went with it: the glyph's remaining sharer was *Export original…*, a **different**
   verb — it copied a stored file out where this one renders. **That row went too, on 2026-08-23**
   (§5.7, §15 D305), so `icon::EXPORT` now has exactly one row in the registry and there is no
   collision left to defend. What is worth carrying forward is that the collision was never decided,
   only twice made moot: the day a second export-flavoured verb wants this glyph, "they cannot share
   a menu" is the argument that has to be made again rather than inherited. ⚠️ The two code comments
   in `menu.rs` that still name *Export original…* as the sharer are stale.
   Listed here because a row in the code that is not in the table
   is exactly the drift §9 exists to catch, and finding it in the code first is how the table stops
   being the record.
9. **The Structure group carries a row this spec never listed: *Even-odd fill*** (§4, §5.4, §15 D239).
   Added 2026-08-31 with the fill rule itself, which is the same shape as item 8 — the app grew a verb
   after this file was written — but a *layer* verb rather than a page one, so §4 is its home and not
   §6.1. It is checkable and on a lone `Path` only, and the two halves of that gating are the parts
   worth having in a table: a boolean is **absent** rather than dim, because `Node::fill_rule` derives
   an `Exclude`'s rule from the operation and a row offering to toggle it would be offering to make the
   shape wrong; and a *multi*-selection is absent because the tick reports one shape's rule and a mixed
   selection has none to show. Recorded a week late, in the sweep that also found items 10 and 11.
10. **And two more the spec never listed: *Flip to other side* and *Detach from path*** (§4, §5.6,
    §15 D405, D406, D409). Added 2026-09-01 with type on a path, a feature that did not exist when this
    file was written, so this is the honest version of item 8's case rather than a miss in the scoring.
    Both appear only on a single layer already on a rail, which is a state with nothing to explain, so
    neither is ever dim but for a lock.
11. ⚠️ **The row that shipped beside those two was deleted the next day, and its absence is a decision
    rather than a gap** (§15 D409). *Text on path* — taken on a text layer and a shape selected
    together — was built on 2026-09-01 and removed on 2026-09-02: setting type on a curve is the Text tool's own gesture now,
    so the row was a second and worse way in. `Item::TextOnPath`, `build::text_on_path` and
    `Context::on_path_pair` are all gone. It is filed here rather than in §9.4 because nothing was ever
    blocked: this is a row this file would have specified, decided against **after** it shipped, and
    the pair above are what survives it — the way back out of a state whose way in is a gesture.
12. ⚠️ **C5 did not fall out of C1, and had not since the menus were built** (§15 D469, `[S12.1-L2-01]`).
    `canvas_context_menu` asked `pick_leaf` alone where the select-click arm asked the frame's name tag
    first, so a right-click on the tag opened `Target::Canvas` — the *empty-canvas* menu — at the very
    pixel a left-click selects the frame; and because an occupied frame's edge and interior go through
    `pick_leaf` too, such a frame had **no canvas menu at all**, the panel's row being the only route.
    Fixed 2026-09-07 by writing the chain once, `canvas::pick_at_pointer`, asked by both doors. It is
    listed here because C5's own closing sentence — *"nothing new is needed for this; it falls out of
    C1"* — is what stopped anyone checking, which is the same failure as item 8's in the other
    direction: **a rule this file says is free is a rule nothing tests.** What is *not* closed is C5's
    *"or on its edge"* half, which `begin_select_drag` answers with a third fallback neither of the
    other doors carries; that is in `roadmap.md` rather than here.
13. ⚠️ **§5.9's list of five rows that stay live over a lock was four in the code, and *Rename* was the
    missing one** (§15 D529, `[S15.2-L1-01]`). `layer_menu` dimmed it, so the row was the only one of
    that verb's three doors that refused — `Ctrl+R`, `F2` and the panel's double-click all renamed a
    locked layer without complaint, which is §15 D261's failure in the direction D320 fixed the other
    way round for *Edit image*. A name is not the layer's artwork, which is the argument this file
    already makes for *Copy* two groups up, so the dim went rather than the chords growing a refusal.
    ⚠️ **The test that quotes this section quoted four of its five rows**, which is why nothing caught
    it: *Rename* and *Show* were live-by-spec and asserted by nothing, and one of the two was wrong.
    **A test that quotes a sentence of this file should quote the whole sentence** — a shortened list
    reads as a filter somebody chose, and the code fills the gap.
14. ⚠️ **§5.6's *"only over a single text layer"* was not enforced, and neither was the code comment
    saying it** (§15 D530, `[S15.2-L1-02]`). `head_rows` pushed the three sizing rows on
    `all_are(Kind::Text)` alone, where every neighbouring push in that function is inside an `if one`,
    so two text layers in different states offered all three rows, ticked the *hit* layer's state as
    though it were the selection's, and wrote only that layer. The `Option` this table names as what
    makes the unanswerable state unspellable could not do it: `layer_menu_state` reads the field off
    the hit node alone, so it is never `None` while the hit is text. Recorded here because the rule was
    written in three places — this table, §15 D261 and a comment three lines above the push — and
    enforced in none, which is the same shape as item 12's *"it falls out of C1"*.
15. ⚠️ **R4's set was four in the code, six in the design export and seven in the app — and §0 said
    four** (§15 D533, `[S15.2-L1-03]` with `[S16.4-L1-05]`). `open_context_menu` cleared `open_menu`,
    `picker`, `stroke_menu` and `type_menu`, so a right-click left the effects popover and the Export
    panel's own menu standing over the canvas the menu was aimed at — the state R4 exists to make
    unreachable — and neither dismisses itself, both reading `PointerButton::Primary` only.
    ⚠️ **The second consequence is R3's**: one `Escape` then closed the menu *and* the effects popover,
    which reads the key itself. §0's *"the four inspector popovers"* was the design export's count of
    `strokePop`, `typePop`, `framePop` and `adjPop`, carried across and never revised as the app grew
    three more; it is corrected above and **said to have been wrong rather than quietly made right**,
    because half of this defect is that the spec and the code were short in the same way and neither
    could catch the other. Listed here because the same hand-written set exists a second time in
    `app::picker_lane_right`, which knew four of the **six** popovers in the popover lane: the two are
    deliberately kept apart — they differ by the image-editing card — and
    `opening_a_context_menu_clears_every_floating_thing` asserts the difference so nobody merges them.
16. ⚠️ **§0's left-click clause was obeyed on the canvas and in the layers panel and by neither of the
    library screen's two menus** (§15 D558, `[S20.2-L1-02]`). The ⋮ popup and the *All projects ⌄*
    dropdown are `egui::Area`s covering only their own rects, and each dismissal is the same six
    hand-rolled lines — `any_click()` plus two `contains` tests — with **no consume**, so the click
    reached the card underneath and `dashboard::pick_or_open` turned it into `Act::Open`: getting rid
    of the ⋮ on one document **opened another one**, the library gone and the editor up on a file the
    user only clicked to dismiss a popup. Listed here because this file is where the rule lives and
    because the shape is §0's own warning about hand-rolled copies — the fix could not be a term in
    `dropdown`, a dismissal being unable to consume a click the card has already turned into an act,
    so it is the *act* that is gated, off a flag latched before anything is drawn. ⚠️ **Two more
    doors on that screen went unspent for a day** — a sidebar nav row and the dashed *New file* card,
    the second of which *creates a document* — and both read the flag since 2026-09-08, which makes
    this rule five call sites on one screen rather than one. 🚨 **Two doors are still unspent after
    that pass**: the sidebar's accent *New file* button, which is the create-a-document case again,
    and *Recent*'s project cards. Seven doors, five gated — the moral is that this rule is a property
    of every **act** on a screen and not of the functions a finding happens to name. ⚠️ **The keyboard half is a different question and only half
    open**: the ⋮ popup blocks the library's keymap and answers `Escape` (§15 D374), while the
    *filter dropdown* reads no key at all and is in none of the guards, so the arrows and `Escape`
    still reach the grid behind it. That one is `roadmap.md`'s open *Escape* question rather than
    anything R4 decides.

**And one item left this list rather than being struck**, which is worth a line because nothing else
here has. The **old** item 8 — the row above took its number — recorded that §3's *Export selection…*
was *Export as…* in the code, the label having
been decided by `no_menu_label_runs_into_its_accelerator` — the specified wording cleared a wide
`Ctrl+Shift+E` by 2.1pt where that test demands 4. With the row gone there is no divergence to record;
§4's own entry keeps the history, and the measurement survives where it is still load-bearing, in
§15 D264 and in item 7's warning that narrowing the card takes 2pt *off* the label column.

### 9.7 Two live gaps this spec surfaced — one fixed, one still reserved

1. ✅ **`Ctrl+C` in a text session did nothing at all. *Fixed 2026-08-18* (§15 D217).**
   `input::resolve` returns an empty `Vec` in `Mode::TextInsert` by design, and the session's own
   event loop handled `Event::Paste` and had no `Copy` or `Cut` arm (`canvas.rs`) — so text could be
   pasted *into* Ondin and never copied *out* of it, for as long as in-canvas editing had existed.
   Fixed on its own account, as this entry said it should be: `TextEdit::selected_text` and
   `TextEdit::cut` in core, two arms in the session's loop, and the trio gated on Alt being up there
   too, which is `shortcuts.md` §L2's rule reaching a second loop.
2. ✅ ~~**The ruler has no right-click home yet** and both Adobe apps put units there. Reserved in
   §6.6 rather than left to be discovered by whoever wires the canvas menu first.~~ **Closed
   2026-08-26** (§15 D358) — and closed the way a reservation should be able to close: units were
   decided against, so the menu has no rows and §6.6 is struck. What the reservation was protecting
   survives it, and is the half worth keeping: **the canvas menu must not be wired to the bar.** A
   ruler is not the canvas.

---

## 10. Tests worth writing

**Where they ended up.** The five that are about *placement, projection and chrome* are written and
each was checked against the plausible wrong version — see below for what two of them caught; the
keyboard bullet at the end arrived with the navigation and carries four more. The
four that drive `RawInput` through the whole app — R1's spent click, R4's replacement, R3's single
rung and "no document action fires while a menu is open" — **were written on 2026-09-19** (§15
D795), in `app::context_menu_rule_tests`, and this paragraph said *"not written — writable now and
unwritten"* until they were. They read as impossible while `OndinApp::new` was the only constructor,
since it takes an `eframe::CreationContext` with a live wgpu render state; `OndinApp::headless`
exists as of 2026-08-22 (§15 D303, D318), which turned them from a limit into a queue — and the
queue then sat for four weeks. 🚨 **They are the first test anywhere in the workspace to drive a
synthetic secondary button through the app**: every earlier context-menu test called
`open_context_menu` by hand or built a `menu::Context` directly, so `canvas_context_menu`'s door had
no test through it at all, which is how thirty-odd green tests in `menu.rs` left all four rules
open. Each bullet's own flip was run and is recorded on the test it belongs to. ⚠️ **One clause of
the queue survives**: the Escape bullet asks for `entered_group` and the tool as well as the
selection, and what was written asserts the menu and the selection — so that half is still verified
by reading and by hand.

The rule this project applies to a green test is *what would also pass this* — so each of these
names the plausible wrong implementation it is aimed at.

- **A right-click that cancels a gesture opens no menu, and the next one does.** Drive press ·
  move · press · release through `RawInput`. Flip against a menu that opens on the **press**: the
  release then lands inside the menu and activates the row under the pointer, which is R1's whole
  argument and is invisible in the source. ⚠️ **Written** as
  `a_right_click_opens_its_menu_on_the_release_and_not_on_the_press` (§15 D795), and that flip is
  red on the **press** assertion while the release assertion stays green under it — a test asking
  only whether a menu is up at the end passes against the bug.
- **Right-clicking a member of a multi-selection leaves the selection whole; right-clicking a
  non-member replaces it.** Flip against "always select the hit" — with five layers selected, the
  wrong version leaves one and *Union* produces nothing.
- **A locked row's menu offers Unlock and dims the rest, and each dimmed row is hoverable.**
  `inspector::menu_action`'s measurement already caught the `ui.scope` spelling being silently
  dead; this is the same assertion at a second call site, and it must assert on `hovered()`
  rather than on the tooltip's ink — a tooltip's galley never reaches `run_ui`'s `.shapes`.
- **The menu stays inside the viewport** opened at all four corners, at `pixels_per_point` 1.0
  **and** 1.5. Both halves matter: the flip, and the rounding — a fix that rounds back to the
  original value at 150% is a no-op on the user's actual machine, which has happened here before.
  ⚠️ **Written, and the flip caught it being vacuous**: "is it on screen" passes against a
  placement with *no flip in it at all*, because the clamp shoves the card left until it fits. What
  the flip actually claims is that the menu is hinged on the pointer, so the assertion had to become
  *the card lies wholly on one side of the press on each axis*. `ctx.screen_rect()` is also gone —
  egui 0.35 calls it `content_rect()`, which is the safe area rather than the whole viewport, and is
  the better answer anyway: a row under a notch is as hidden as a row off the bottom.
- **A second right-click moves the menu rather than adding one.** Open over a layer, right-click
  somewhere else, assert exactly one menu `Area` exists and that it sits at the *second* press's
  position. Flip against close-on-click-away and open-on-secondary-click written as two independent
  rules in that order: the close then fires on the release that opened the new one and **no** menu
  is left at all — §15 D82's one-frame bug wearing a different hat. ⚠️ **Written** as
  `a_second_right_click_replaces_the_open_menu_rather_than_closing_it` (§15 D795), and the predicted
  shape is what the flip produced: **`None`**, by the refusal leaving the first menu in the slot
  with `just_opened` false, so the same click is then read as a click-away.
- **The opening release does not dismiss the menu it opened.** One right-click, then five more
  frames with the pointer still, and the menu is there on every one of them. Flip by letting
  `dropdown`'s click-away test run on the opening frame: it passes at frame 0 and fails at frame 1,
  which is precisely what "showed for exactly one frame and vanished" looks like from a probe, and
  the reason to pump frames rather than assert once.
- **Escape closes the menu and nothing else.** Open one with a layer selected inside an entered
  group and the node tool active, press Escape *once*, and assert the menu is gone while the
  selection, `entered_group` and the tool are all untouched. Flip against a menu that closes without
  consuming the rung: the same press also leaves the group, and the ladder eats two states per
  keystroke for the rest of the session. ⚠️ **Written in part** as
  `escape_over_a_menu_closes_the_menu_and_keeps_the_selection` (§15 D795): a **selection**
  underneath rather than an entered group and the node tool, so the menu and the selection are
  asserted and `entered_group` and the tool are not. The selection is the sharper of the three
  anyway — it is cleared on the *same* rung, where present mode and an entered group are further
  down the ladder — but this bullet is not closed.
- **No document action fires while a menu is open.** Open one over a layer, press `Delete`, assert
  the document is byte-identical. Flip: without R3's gate the layer is gone and the menu is left
  pointing at nothing.
- **Every registry entry has a predicate, a glyph and a group.** One table-driven test over the
  registry, which is what stops the twelfth row being added without one. ⚠️ **Half of it is not
  enough**: `Item::ALL` is written by hand, so a new variant is invisible to a walk over the list.
  What makes the list answerable for the enum is the other half — build *every* menu the app can
  open, over every kind, and demand each row found is in `ALL`.
- **A text session's *Paste* reads the system clipboard and not the layer one.** Build §6.4's menu
  with layers on the in-app clipboard and no text on the system one — the state the row's first
  predicate got exactly backwards — and assert *Paste* is dim, then turn the three session states on
  and assert all four rows live. ⚠️ **Written**, and the fixture has to be asserted first here more
  than anywhere: over the one-row menu this replaced, three of its four availability assertions would
  have passed by finding nothing at all. Flip-checked by restoring `!cx.clipboard_full`.
- **Assert the fixture before asserting the projection.** `assert_eq!(rows.len(), N)` first, then
  which rows and in what order — a projection test over an empty menu passes for the wrong reason,
  and §3's whole claim is about *order*. ⚠️ **And assert the *sequence*, not that it is sorted.**
  `build` buckets rows by their registry group, so "the groups come out in order" became true by
  construction and a test of it would pass against every possible assignment of rows *to* groups.
  What can still be wrong is the assignment, so the assertion is the primitive menu's 23 labels
  spelled out — which is what would have caught the View switches sitting in `Group::State` and
  pushing *Zoom to fit* below them. (A small history worth keeping, because it is the drift rule
  working: this file said 17, was corrected to **16** on 2026-08-19 against the code, went back to
  **17** the same day when *Outline shape* landed and made the file right again — §15 D230 — and to
  **18** on 2026-08-20 with *Frame selection*, §15 D249. It then sat at 18 here while the code went
  to 20, 21, 22, 23, 24 and back to 23, which is six rows of drift in a bullet whose whole subject is
  drift. The number was never the thing to trust;
  `a_primitives_menu_is_twenty_three_rows_in_canonical_group_order` is, and it is renamed with the
  count precisely so a stale one cannot sit in a green suite — as this sentence's did, the test having
  been renamed every time and this copy of it not.)
- **The keyboard walk visits only the rows a press could run, and `↑` from nothing is the last one**
  (§8, §15 D223). Four tests, and none of them needs an app: `navigate` takes a built menu and
  returns the next verb, so the whole of the walk is testable over the one fixture in the file that
  is *mostly dimmed* — a locked layer's menu, where seven of twenty-three rows are live (read off the
  push conditions, not measured; the test asserts only that the fixture has some of each, which is
  all the filter needs and is why this number could go stale from three without anything failing).
  ⚠️ **Flip against
  the walk with no `.filter(enabled)` in it**, which is the obvious spelling: the first `↓` lands on
  *Cut*, dimmed because the layer is locked, and `Enter` there does nothing a keyboard user can
  diagnose. And flip `↑`-from-nothing against `unwrap_or(0)`, which is the arm every other assertion
  still passes with. The chrome half is its own test and its own flip: with the pointer on row 0 and
  the highlight walked to row 2, the naturally-written `highlight == Some(true) || hovered()` paints
  **two** grounds, and a menu with two highlights cannot say what `Enter` will do.

---

## Undecided, and worth one look on the machine

Six questions this file took a position on that are cheap to reverse, listed so the position is
visible as a position:

1. **The head at the top** (§3). The alternative is Figma's shape — universal rows first, type
   verbs last — which is more predictable and less useful.
2. **`Reset …` rows omitted rather than dimmed** (§3), which is the one place this file
   deliberately differs from image editing's card, where they are dimmed.
3. **The three text sizing rows** (§5.6, §15 D261 — built 2026-08-20, not yet seen on screen; this
   entry said *four* until they existed to count). They duplicate an inspector control; the argument
   for them is that *Auto width* is the most-reached-for text command in Figma.
4. **Whether the canvas menu carries *Canvas background…*** (§6.1), which the inspector already
   shows when nothing is selected.
5. **A left-click that dismisses a menu is spent doing so** (R4) and does not select what it landed
   on. The alternative — dismiss *and* select — is one line, and reads as faster right up to the
   first time it retargets the selection the menu was about to act on.
6. ***Join* merges nothing** (§6.5, §15 D256). Two ends that happen to sit on the same point become
   two anchors with a zero-length segment between them, exactly as the pen's own join leaves them.
   The alternative wants a tolerance and a rule about whose handles survive; consistency with the
   gesture was judged worth more than either. Listed here because it is the kind of thing that gets
   re-argued the first time someone notices the duplicate anchor.
