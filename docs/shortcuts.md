# Ondin — keyboard shortcuts

**Status: agreed 2026-08-03; §1–§6, §9 and §10's cheap rows built 2026-08-18, §7 and §8's export
rows 2026-08-20, §8a — the library's own keys — 2026-08-28, and `Ctrl+N`/`Ctrl+W`/`F2`/`Ctrl+D` the
same day.** **Three** rows in §8 and two in §10 are
what is left; the count is kept here rather than only in the ledger because this is the line a
reader checks first. ⚠️ **It was five until 2026-08-28**, when `Ctrl+N` and `Ctrl+W` — the two that
had been waiting on a dashboard that arrived two days earlier — were bound (§15 D383); the three
left all wait on the command registry, which is one blocker rather than three. ⚠️ **§8a did not
move that count then and does not now.** It is *new* rows, all ✅ on the day they were written, and
its own gaps are named at the foot of it rather than added to §8's count — `Ctrl+N` appears in both
sections and is counted once, in §8. This is the
spec `input.rs` was built out to, and it stays the keymap's inventory rather than becoming a
changelog — a row marked ➕ when it was written is marked ✅ now, and the *Implementation
ledger* at the bottom is where what is left lives. It supersedes the five-bullet *Keyboard*
section `roadmap.md` used to carry; that section is now a pointer here.

**§7 is closed** — the clipboard group was "unbuilt in its entirety" when this line was
written, and it is now built in its entirety bar one row that is a **decided non-goal**:
paste-at-pointer, declined 2026-08-22 (§15 D248, D257, D300). What is still unbuilt is three rows
in §8 — all three waiting on the command registry — plus `Ctrl+B`/`Ctrl+I` in §10. ⚠️ **`Ctrl+N` and
`Ctrl+W` were the other two and are bound as of 2026-08-28** (§15 D383): they waited on the dashboard,
the dashboard landed on 2026-08-26, and nothing between the two dates said so. **Export is no longer one of them, and the correction this
sentence carried was itself half wrong within the day**: it listed export as a third blocker until
2026-08-22, two days after `Ctrl+Alt+E` and `Ctrl+Shift+E` were both bound (§15 D264, D274) — and
`Ctrl+Shift+E` was **unbound again** hours later, *Export as…* having been removed when the Export
panel made it the slower of two doors onto one file (§15 D264). So the honest reading is that
`Ctrl+Alt+E` is ✅ and `Ctrl+Shift+E` is a **decided non-goal** of §8's own, not a row waiting on
anything. ⚠️ It is unbound and **not free** — see its row in §8. Nothing else in this
file is a proposal any more.

Not in `design/` on purpose — that folder is a regenerated design export and a
hand-written spec would be lost with the next regeneration.

**§1–§10 are the sections that were reviewed and agreed, and keep their numbers from that
review. §11 completes the inventory** — the history, structure and navigation keys that were
already built and never in question. The file is the whole keymap, not the new part of it.

**Where the bindings come from.** Figma and Sketch agree with each other far more often
than either agrees with Adobe, so the rule applied throughout was: take the Figma/Sketch
binding where the two families disagree about a *concept both have*, and take Adobe's
where Figma has no answer at all (grid, guides, snapping — §6). Three collisions could
not be resolved in anyone's favour and are recorded as such at the bottom.

---

## 0. Read this first: how egui delivers a key is not how you spell it

Three separate bugs in this file's history are the same bug, and it is the only thing here
that cannot be found by reading the keymap.

`egui_winit` builds a key from `logical_key.or(physical_key)` — **the logical key wins**.
So a shifted punctuation key arrives as whatever `egui::Key` variant matches the character
it produces, *not* as the unshifted key plus `Modifiers::shift`. `egui::Key` has variants
for `!` `?` `:` `|` `{` `}` and does **not** have them for `@` `)` `"` `<` `>`. That
asymmetry decides, per binding, whether the obvious spelling works:

| Chord | Character | Arrives as | Action needed |
|---|---|---|---|
| `Shift+1` | `!` | `Key::Exclamationmark` | **accept both** — ✅ done, see §L1 |
| `Ctrl+Shift+;` | `:` | `Key::Colon` | **accept both** — ✅ done (§6) |
| `Ctrl+Shift+\` | `\|` | `Key::Pipe` | **accept both** — ✅ done (§6) |
| `Ctrl+Shift+/` | `?` | `Key::Questionmark` | accept both — binding unbuilt (§8) |
| `Ctrl+Shift+[` `]` | `{` `}` | `Key::Open/CloseCurlyBracket` | ✅ already handled correctly |
| `Shift+2`, `Shift+0` | `@`, `)` | no variant → physical `Num2`/`Num0` | nothing — ✅ (§3) |
| `Ctrl+Shift+'` | `"` | no variant → physical `Quote` | nothing |
| `Ctrl+Shift+,` `.` | `<` `>` | no variant → physical `Comma`/`Period` | nothing — ✅ (§10) |

**A binding in the left column that names only the unshifted key reads perfectly and fires
never.** The existing bracket-restack code is the model to copy: accept the curly spelling
unconditionally (it cannot be produced without Shift) and read the plain spelling against
the modifier.

Layout-dependence is real but does not change the rule — on a UK layout `Shift+2` is `"`,
which also has no variant and also falls back to the physical digit. Accepting both
spellings is correct on every layout; accepting one is correct on some.

### The clipboard trio is intercepted upstream, and it ignores Alt *and Shift*

`egui_winit`'s `is_cut_command` / `is_copy_command` / `is_paste_command` test
`modifiers.command && Key::X|C|V` — **Alt and Shift are not examined** — and the caller
`return`s, so `Key::X`, `Key::C` and `Key::V` never reach egui at all while Ctrl is down.
On Windows `command == ctrl`.

Consequence for this spec: **every clipboard-adjacent binding must be resolved from
`Event::Cut`/`Copy`/`Paste` plus that frame's `i.modifiers`**, never from a key press. That
covers `Ctrl+Shift+V`, `Ctrl+Alt+C`, `Ctrl+Alt+V` — and the Exclude boolean, which is
where it was a live bug until 2026-08-18 (§L2).

**The one signal that does survive is the key coming back *up*.** All three guards sit inside
`if pressed`, so a release arrives as an ordinary `Key { pressed: false }` however the press was
swallowed. That is how `Ctrl+Alt+X` reaches Exclude and how `Ctrl+V` reaches an image paste
(`decisions.md` §15 D183, D204) — and it is the second spelling available to any binding in the
paragraph above.

⚠️ **And a binding that reads *both* spellings fires twice, one frame apart.** `Ctrl+V` over text emits
the event on the press frame and the release later, and a same-frame guard cannot see it — one text
paste ran twice for as long as the release has been read, invisibly, because the second copy lands on
the first (§15 D219). The signals are two `Action`s now and the release only acts when the clipboard
holds no text, which is the exact complement of the condition that decides whether the event exists.
**Any binding tempted to accept both spellings owes this discrimination**, and a same-frame flag is not
it: `egui_winit` swallows the press, so `key_down` is false on the frame the event arrives and "while
the key is held" is not a question that can be asked.

---

## Live bugs this spec found — both fixed 2026-08-18

Kept rather than deleted, because §0 is the reason both existed and these are the two
worked examples of it. `decisions.md` §15 **D204** and **D205** carry the full record.

**L1 — `Shift+1` (zoom to fit) had never once fired. *Fixed.*** `plain && m.shift &&
key_pressed(Key::Num1)` at `input::normal_mode`, the zoom block. The key arrives as
`Exclamationmark`, and `InputState::num_presses` never consults `physical_key`. Both
spellings are accepted now, `Num1` kept for a layout that passes the unshifted digit
through. `shift_1_zooms_to_fit_and_arrives_as_an_exclamation_mark` pins our half; the
backend→egui half — that the key really arrives as `!` — is still unverifiable headlessly,
because `ctx.run_ui` only shows egui→backend. **`Ctrl+1` was never broken, which is why the
dead line survived review**: it carries no Shift, so the pair read as one binding written
twice.

**L2 — `Ctrl+Alt+X` cut the selection instead of excluding it. *Fixed.*** `is_cut_command` fires
on `Ctrl+Alt+X`, so `Event::Cut` was pushed, `Key::X` was dropped, and `input::normal_mode`, the `Event::Cut` arm, turned
that into `Action::Cut` with no modifier check. Exclude was unreachable from the keyboard
and the destructive action happened silently. `Ctrl+Alt+U/S/I` were unaffected — `X` is the
only intercepted letter among the four.

The fix took both halves: the **whole** clipboard event loop is gated on `!m.alt` — Cut,
Copy and Paste alike, which is the `cmd_only` rule every other binding in that function
already stated for itself — and `Action::Boolean(Exclude)` is resolved from
`key_released(Key::X)` rather than from `Event::Cut && m.alt`, which is what this entry
originally prescribed. Either would have worked; the release keeps the four boolean chords
one loop over four keys.
**Consequence, deliberate at the time: `Ctrl+Alt+C` and `Ctrl+Alt+V` were inert rather than
clipboard operations.** The guard became a *branch* on 2026-08-20 and both are bound now (§7,
`decisions.md` §15 D257); `Ctrl+Alt+X` is still dropped there, because the release arm owns it.

`save_and_cut_are_not_booleans` missed L2 because it pushes a synthetic `Event::Key`, and so
did `the_four_boolean_chords_resolve`, which was green for the whole time Exclude was
unreachable. Both now drive the real signal, and `ctrl_alt_x_excludes_and_does_not_cut` pins
the pair. **Any test for a Ctrl+letter chord where the letter is C, X or V must drive the
event or the release, not the key press.**

---

## 1. Tools

All single-key, `plain && !shift` (no Ctrl, no Alt), Normal mode only.

| Key | Tool | |
|---|---|---|
| `V` | Select | ✅ Figma, Sketch, Illustrator |
| `H` | Hand | ✅ Figma |
| `K` | Scale | ✅ Figma |
| `F` | Frame | ✅ Figma (which also binds `A`; taken here) |
| `R` | Rect | ✅ Figma, Sketch |
| `O` | Ellipse | ✅ **primary** — Figma and Sketch |
| `E` | Ellipse | ✅ keep as alias. Two letters for one tool costs nothing and `Ctrl+E` (flatten) is unaffected |
| `G` | Polygon | ✅ keep. No app has a convention; nothing collides |
| `S` | Star | ✅ keep |
| `L` | Line | ✅ Figma, Sketch |
| `P` | Pen — **or arms the pen bias**, inside an active point edit | ✅ Figma, Sketch. Context-sensitive since §15 D125 |
| `A` | Node | ✅ keep — Illustrator's and Sketch's direct-selection letter. **No lock check in the key's arm, and none wanted**: the tool's subject is the layer selection, which a locked layer can join from the layers panel, so the refusal is in `canvas::edited_path` — the key arms the tool and it finds no subject, exactly as `A` over a rectangle already does (§15 D321) |
| `T` | Text — **click** plants an auto-width node, **drag** draws a fixed box | ✅ Figma, Sketch. The two gestures since §15 D170 |
**`C` was Crop's and is unbound again (§15 D268).** The row above it used to read
*Crop — moves and scales an image inside its frame*, taking the comment letter on
the argument that a built tool outranks an inert rail button, and defending the
rail slot on D118's rule that a mode entered only by double-click is the failure
to avoid. Both halves went with the tool: **image editing** replaced it, is
entered by double-clicking a picture or pressing `Enter` on one, and has neither
a rail slot nor a letter. What answers D118 there is that it cannot be entered
without a picture to enter and that entering it puts a card on screen — where the
old crop tool could be armed from the rail with nothing to crop, which is half of
why *Crop* and the bounding box ended up meaning different things. So `C` is
free, and it is the letter a comment tool would want.

**The comment at `input::normal_mode`'s tool block was corrected when this landed.** It
claimed `A` was "Figma's letter for the node/vector-edit tool"; Figma binds `A` to the
**Frame** tool alongside `F` and has no letter for vector editing at all. The binding was
right for Illustrator/Sketch users and the justification was wrong — which is the shape of
thing that gets a good binding "corrected" later.

The rail's tooltip shows `O` for the ellipse. `E` is undocumented in the UI — an alias for
hands that already know it, not a second published binding.

## 2. Opacity

| Key | Action |
|---|---|
| `1`–`9` | ✅ set selection opacity to 10%–90% |
| `0` | ✅ 100% |
| two digits within **600 ms** | ✅ that exact percentage — `4`,`5` → 45% |
| `0`,`0` within 600 ms | ✅ 0% |

Plain only (no Ctrl, no Alt, no Shift). Universal across Figma, Photoshop and Illustrator.
Nothing plain-digit is bound today, so this is free. Applies to the whole selection via
`build::set_opacity_all`, which already exists; no-op with an empty selection.

`Key::Num0`–`Num9` cover the numpad as well as the main row, per their own doc comments —
so this works on both without a second spelling.

**The 600 ms window needs a repaint to close**, and it got one. In a reactive app nothing
wakes the frame loop after the last keypress, so the pending first digit schedules
`request_repaint_after(remaining)` — same class as the inspector's 1500 ms chrome timer and
the stranded font-face repaint (§15 D110), and folded beside it in
`<OndinApp as eframe::App>::ui` (§15 D698).

**The first digit is a *preview*, not a commit**, which is the part the plan above did not
say and undo demands: `4`,`5` has to leave one history step reading 45%, not a 40% the user
never asked for with a 45% on top of it. So the window shutting is what commits — by a second
digit, or by the deadline — and `opacity_window_open` is deliberately shared by the pairing
rule and the deadline so the two cannot drift apart and spend one keystroke twice.

⚠️ **There is a third way the window shuts, and it commits nothing** (§15 D535): `Escape`, or a
right-click, both of which reach `OndinApp::cancel_gesture`. The two-item list above is what this
file and D210 both said, and while it was the whole list `cancel_gesture` cleared the preview and
left the pending digit standing — so the cancelled 40% arrived 600 ms later anyway, with an undo
step and an autosave behind it. A cancelled gesture owes zero transactions (§9.3), so the cancel
now drops the entry as well as the preview.

⚠️ **The pending digit carries the layers it was typed against, not just the clock** (§15 D470).
600 ms is long enough for the selection to move, and the deadline used to re-read whatever was
selected *then*: type `4` against A, click B, and A went back to full while **B** took 40%. The
subject is part of the entry now, which also makes `4` on A then `5` on B two values rather than
45% on B. This paragraph enumerated the empty-selection case above and not this one.

⚠️ **Reserve `Shift+3`…`Shift+9`.** Adobe puts blend modes on `Shift`+digit. Blend modes are
on the deferred list, and `Shift+0/1/2` are already spent on zoom (§3, Figma's choice) — so
do not hand the rest out.

## 3. Zoom

| Key | Action | |
|---|---|---|
| `Ctrl` `+` / `=` | zoom in | ✅ both spellings already accepted |
| `Ctrl` `−` | zoom out | ✅ |
| `Ctrl+0` | 100% | ✅ Sketch |
| `Ctrl+1` | fit | ✅ Sketch |
| `Shift+1` | fit | ✅ both spellings accepted — was bound and dead, §L1 |
| `Shift+0` | ✅ 100% | Figma |
| `Shift+2` | ✅ **zoom to selection** | Figma |
| `Ctrl+2` | ✅ zoom to selection | Sketch |

`Action::ZoomSelection` exists, and the split it required was the real work: `ZoomFit` used
to fall back to the selection whenever there was one, so **"fit everything" was unreachable
while anything was selected**, with no key to get it back. `content_bounds` became
`document_bounds` and `selection_bounds`; the zoom dropdown gained a *Fit selection* row,
because "Fit to page" was quietly doing that job and splitting them would otherwise have
taken it away from everyone who reached it by menu. `reveal_selection` shared that helper and
so fell through to the whole document on an empty selection — it reads `selection_bounds`
now, which is what its name always claimed.

⚠️ Adobe's `Ctrl+0`/`Ctrl+1` are the reverse of this (fit / 100%). Sketch and Figma agree
with what is here. Recorded under *Unresolvable* below.

## 4. Layer state

| Key | Action | |
|---|---|---|
| `Ctrl+Shift+H` | ✅ hide / show | Figma **and** Sketch |
| `Ctrl+Shift+L` | ✅ lock / unlock | Figma **and** Sketch |
| `Ctrl+R` | ✅ **rename** | Figma. Decided against the Adobe reading — see below |
| `F2` | ✅ rename | Windows convention, alias |
| `Shift+H` | ✅ flip horizontal | Figma |
| `Shift+V` | ✅ flip vertical | Figma |

`Operation::SetVisible` / `SetLocked` and `build::flip` all existed, so these were wiring —
with one decision that was not. **Hide and lock are one switch for the whole selection**, and a
*mixed* selection **hides / locks**: a per-layer flip would make the chord's effect depend on a
count the user cannot see, and *Hide* is a verb rather than a toggle — someone pressing it on
five layers of which two are already hidden is tidying up, not asking for the two back.

**It shipped the other way for a day, and the argument that lost is worth keeping**, because it
is the one that comes back. A hidden or locked layer is unpickable (`query.rs`'s hit test skips
`!visible() || locked()`), so hiding a mixed selection puts layers where the canvas cannot reach
them. The chord still undoes itself — neither operation drops the selection — but that recovery
is **one stray click wide**: click elsewhere and the only way back is the layers panel, which
may be shut. Releasing on mixed avoids that and costs a press. Rejected anyway: normalising
*away* from what the verb says is the stranger surprise, and `Ctrl+Z` is a better answer to a
mis-click than a chord that second-guesses its own verb every time.

Both go through `session.commit` rather than `commit_edit`. Not because `commit_edit` gates
anything — it gates nothing, being `note_edit` plus the same commit — but because its one
addition is arming the *inspector's* selection-chrome hide, and a visibility switch has nothing
to look at. Its own doc already names the layers panel as the exception, and that panel's eye
and lock buttons commit these very operations the same way.

Rename opens the **same** `renaming_layer` field a double-click in the layers panel opens — so
it shows that panel if it was hidden, and refuses a multi-selection, there being no one row to
lay the field over. **It also refuses in present mode**, which draws no panels at all: showing
the panel there shows nothing, and the rename would arm against a field that never renders —
no caret, no way to cancel, and a rename box springing open the moment present mode ended.

**`Ctrl+R` is rename, not rulers.** Illustrator and Photoshop both toggle rulers on
`Ctrl+R`, and Ondin does not: rulers keep `Shift+R` (Figma's, already built) and `Ctrl+R`
is not aliased to them. An Adobe hand pressing `Ctrl+R` therefore opens a rename field
rather than toggling nothing. Decided deliberately, because the alternative spends a free
chord on a capability that already has a key. ⚠️ **And it means the same thing on the library
screen as of 2026-08-28** (§8a, §15 D383), which is the second half of that argument arriving
two months later: a chord this file spent on *rename* everywhere is worth more than one spent
on rename in the editor and on nothing in the dashboard.

`Shift+H` and `Shift+V` are safe because the tool block is gated on `!m.shift` — the
existing `shift_r_toggles_rulers_without_stealing_the_rect_tool` test is the pattern, and
flip owes two more like it.

## 5. Align and distribute

| Key | Action | |
|---|---|---|
| `Alt+A` | ✅ align left | Figma |
| `Alt+D` | ✅ align right | Figma |
| `Alt+W` | ✅ align top | Figma |
| `Alt+S` | ✅ align bottom | Figma |
| `Alt+H` | ✅ align centres horizontally | Figma |
| `Alt+V` | ✅ align centres vertically | Figma |
| `Alt+Shift+H` | ✅ distribute horizontally | **invented** |
| `Alt+Shift+V` | ✅ distribute vertically | **invented** |

The six aligns are Figma exact. The two distributes are ours on purpose: Figma's sit on
`Ctrl+Alt+H`/`V`, which collide head-on with paste-properties (§7), and `Ctrl+Alt` is
already this keymap's **boolean namespace** — muddying it is worse than picking a fresh
chord. `Alt+Shift+H` reads as the bigger version of centring on that axis. Illustrator
ships no align shortcuts at all, so no convention is being contradicted.

`Alt+S` does not collide with `Ctrl+Alt+S` (subtract): the align block is Alt-without-Ctrl.
A bare `Alt` is not a problem here — there is no system menu bar to poke.

`build::align` and `build::distribute` exist and both understand a selection. The key layer
(§15 D113) is already the align target when one is designated, so these bindings inherit it
for free.

## 6. View, grid, guides and snapping — Adobe's set

Figma has almost nothing here, so this whole group is Illustrator's, which is the oldest
and deepest convention of the five apps.

| Key | Action | |
|---|---|---|
| `Shift+R` | show / hide rulers | ✅ Figma |
| `Ctrl+'` | ✅ show grid | Illustrator, Photoshop |
| — | show layout grid | **deliberately unbound** (§15 D755) |
| `Ctrl+Shift+'` | ✅ snap to grid | Illustrator's `Ctrl+"` is literally this chord |
| `Ctrl+U` | ✅ snap to shapes | Illustrator's Smart Guides — semantically the same switch |
| `Ctrl+;` | show / hide guides | ✅ Illustrator |
| `Ctrl+Shift+;` | ✅ snap to guides | both spellings accepted |
| — | snap to baselines | **deliberately unbound** (§15 D355) |
| `Ctrl+Alt+;` | lock guides | ✅ Illustrator |
| `Ctrl+\` | ✅ present mode (hide all chrome) | Figma's show/hide-UI chord |
| `Ctrl+Alt+\` | ✅ show / hide layers panel | |
| `Ctrl+Shift+\` | ✅ show / hide toolbar | both spellings accepted |

**Snap to baselines has no chord and that is the decision, not an omission** (§15 D355). The
other three snap switches earned theirs by being Illustrator's; no application has baseline
snapping at all, so there is no convention to match and no chord worth spending on a fourth
switch in a menu that is one click away. It is listed here because this file holds the keymap
*bound, unbound and agreed* — a row saying "none, on purpose" is what stops the next reader
reaching for a free chord and calling it parity.

**Show layout grid is the second of those, on the same reasoning** (§15 D755). The frame's own
column and row bands got a workspace switch on 2026-09-15 — *View ▸ Show layout grid*, the row
after *Show grid* — and no chord, there being no convention to match: ⚠️ **this file was grepped
before the switch was built and had never reserved or declined one for it**, which is what the row
above records so nobody has to grep again. `ViewSwitch::accel` answers `None` for exactly these two.

`Ctrl+\` maps onto present mode rather than a new toggle: present mode already *is* "hide
every piece of chrome at once and restore what the user had on leaving", which is what
Figma's chord does. Escape still leaves it — that stays the one row reachable another way,
for the reason `OndinApp::escape` gives.

`Ctrl+U` does not collide with `Ctrl+Alt+U` (union). It **does** mean underline in
TextInsert — see §10.

The two guide rows were marked "state does not exist" and "not in the model" and are now both
bound (`Action::ToggleGuides`, `Action::ToggleGuideLock`). The second marker was resolved by
**deciding it needed no model change at all**: lock is a workspace toggle on `OndinApp`, not a
per-guide property, which is what makes one chord a legitimate way in and out — see
`architecture.md` §9.4.

⚠️ Checked before these landed: `theme::install` already switches `zoom_with_keyboard` off,
which is the only `Options` chord that collided, and egui claims none of the rest. Still the
first thing to check before adding another, for the reason that one existed.

## 7. Clipboard and properties

**Deferred as a group, 2026-08-18 — and the group was wrong, then discharged.** It was the
one section of §1–§10 left standing after that pass. Not because the rows were hard:
paste-in-place and paste-at-pointer were both cheap, and copy/paste *properties* was scored as
a subsystem that did not exist and needed designing (which properties? fills, strokes,
opacity, radius, type?). What made it a group was the guard below: the `!m.alt` gate had to be
**widened into a branch**, and doing that for two rows while the other two stayed inert would
leave the event loop half-converted with nothing to show for it.

That argument was sound and it bound **two** rows, not four. *Paste in place* is `Shift`, not
`Alt`: it is read *inside* the existing guard, as one arm on `Event::Paste`, and it wanted the
Alt branch neither widened nor touched. *Paste at the pointer* is the same shape. So the two
paste rows were only ever standing behind a blocker that was never theirs, which is what
building the first of them showed (2026-08-20, §15 D248). **A shared blocker is a claim about
each member, and this one was only ever true of two.**

**And those two landed later the same day** (§15 D257), so nothing here is deferred any
longer — and since 2026-08-22 nothing here is *open*: the last ➕ row, paste-at-pointer, is a
decided non-goal (§15 D300) rather than a want nobody had got to. The guard **is** a branch,
and the answer to "which properties?" is fills, strokes and
opacity — the appearance — with position, size, rotation, name, pivot and lock excluded as
being about *which layer this is*, and geometry excluded a second time over because a corner
radius lives in `NodeKind` and means nothing carried from a rect to a star.

| Key | Action | |
|---|---|---|
| `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | copy / cut / paste | ✅ |
| `Ctrl+V` | ✅ **text on the clipboard becomes a text layer** | Figma. Built 2026-08-18 — the centre of the frame the selection is in, else the middle of the view; no frame needed (§15 D218, D221) |
| ~~`Ctrl+V`~~ | ~~paste **at the pointer** when the canvas has it~~ | **Decided against 2026-08-22 (§15 D300).** The chord is position-free on purpose; *Paste here* is the positional door |
| `Ctrl+Shift+V` | ✅ **paste in place** | Illustrator. Built 2026-08-20 (§15 D248) — `Event::Paste` plus the frame's `shift`, which needs none of the Alt work below |
| `Ctrl+Alt+C` | ✅ copy properties | Figma. Built 2026-08-20 (§15 D257) — fills, strokes and opacity off the **key layer**, or a lone selected one |
| `Ctrl+Alt+V` | ✅ paste properties | Figma. Read from the key **release**, not from `Event::Paste` — see below |
| `Ctrl+D` | duplicate | ✅ Figma |

**Three of these four rows are resolved from the clipboard *events* plus the frame's
`i.modifiers`** — see §0. `Event::Copy` and `Event::Paste` carry no modifiers of their own,
so the discrimination is `i.modifiers.alt` / `.shift` read in the same frame the event
arrived in. Fragile-looking and correct; the alternative does not exist, because the key is
gone before the app runs.

**`Ctrl+Alt+V` is the exception and is read from the key *release*** (§15 D257). `egui_winit`
pushes `Event::Paste` **only** when the clipboard yields non-empty text (§15 D183), and a
property paste has nothing to do with the OS clipboard at all — so read from the event, the
chord would work whenever a sentence happened to be copied in another application and do
nothing the rest of the time. The release falls past the swallow, which is the same seam
Exclude is read at. The event arm still exists and *silences* the chord, so it cannot fall
through to an ordinary paste.

⚠️ **The event loop *dropped* every Alt-modified clipboard event until 2026-08-20**, which was
§L2's fix: a `Ctrl+Alt` chord belongs to the booleans. It is a **branch** now (§15 D257) —
`Ctrl+Alt+C` selects the copy-properties row, `Ctrl+Alt+V` is silenced, and `Ctrl+Alt+X` is
still nothing at all, because it belongs to Exclude's release arm and emitting anything for it
would put §L2's destructive cut straight back. **The release arm needed no discrimination**,
where this paragraph predicted it would ("or `Ctrl+Alt+V` pastes a picture as an ordinary
paste"): `cmd_only` is `command && !alt`, so `Ctrl+Alt+V` never reached `Action::PasteRelease`
and the guard the prediction feared had already been written with the image paste
(`decisions.md` §15 D183, D204). Corrected rather than deleted — a fear that turns out to be
already answered is worth the sentence.

**Paste-at-pointer is decided against, and §7 is now closed** (2026-08-22, §15 D300). It was
never blocked — it is one dispatch arm, `paste_at` already takes a world point, and the
off-canvas case has had its answer in `Item::PasteHere`'s `None` arm the whole time. It is
declined on its merits: `Ctrl+V` landing somewhere that depends on where the mouse happens to
be resting is *less* predictable than a fixed step, and the positional paste already exists one
right-click away. Reviewed in use and kept as it is.

⚠️ **The reason the code gave for the fixed offset was wrong even though the offset is right.**
`paste_at` and the menu both said "a key has no position to paste at"; a key press arrives on a
frame like any other event and `canvas::drop_point` reads a world point off a bare `&Context`.
Both comments now say the true thing — the chord is position-free *by choice* — which is what
keeps this decision from being re-opened by the next person who notices the mechanism exists.

## 8. File and discovery

| Key | Action | |
|---|---|---|
| `Ctrl+S` | **save and pin a version** | ✅ **Its job changed 2026-08-26** (`decisions.md` §15 D364). With autosave writing the file every thirty seconds, "save" on its own stopped meaning anything a person can feel — so this writes *and* copies the document into `.versions/<document id>/<unix seconds>.ondin`. Keyed on the document id, not the filename, so a rename cannot orphan the history |
| `Ctrl+Shift+S` | ~~save as~~ | ⛔ **unbound 2026-08-26**, and deliberately. *Save As* asked where to put a file and nothing asks that any more — every document lives in the base folder (§9.5). What it was used for is *Duplicate* in the dashboard's file menu, which mints a **new** document id rather than leaving two files claiming one history. ⚠️ **The `!m.shift` on the `Ctrl+S` arm is the only thing keeping this unbound**: without it the chord would quietly become a second *Save*, invisibly, and only for the user whose fingers still reach for it. `save_takes_the_bare_chord_and_the_shifted_one_is_free` asserts the **empty** result |
| `Ctrl+O` | **the library** | ✅ No longer a file dialog — there are none. Goes to the dashboard, as the brand mark and the folder button do; three doors onto one screen, because that is where a person looks for "home" *and* for "open" |
| `Ctrl+K` / `Ctrl+P` | **search files and projects** | ✅ bound 2026-08-26, **on the dashboard only**. ⚠️ Read directly from the context rather than through `input::resolve`, which is not running at all while the library is up (`OndinApp::ui` returns before it) — the keymap in this file is the *editor's*, and the library answers its own keys. Enter takes the first document match, never a project. ⚠️ **It was "its own two keys" until 2026-08-28**, when §8a below made it six — and while the overlay is up those six are the overlay's, which is a guard rather than a coincidence. ⚠️ **`Ctrl` without Alt and without Shift, and it was bare `Ctrl` until 2026-09-09** (`decisions.md` §15 D712): `Ctrl+Shift+K`, `Ctrl+Alt+K`, `Ctrl+Shift+P` and `Ctrl+Alt+P` all opened the search, four spellings this row does not list — the *Deliberately unbound* rule, on the one screen the editor's guards cannot reach |
| `Ctrl+N` | **new document** | ✅ bound 2026-08-28 (`decisions.md` §15 D383). The dashboard's *New file* verb, reachable from both screens — and **the project it lands in is computed at the door**, because the two doors know different things: the library reads the nav being viewed, the editor reads the open document's own project. Same rule, *the project in front of you*; letting the editor read the nav would file into whichever nav the dashboard was left on, which nothing on the editor's screen names. Unfiled at the root from *Recent*, *All files*, *Starred*, the trash, and from a session whose document is not in the library |
| `Ctrl+W` | **close the document** | ✅ bound 2026-08-28. Figma's meaning rather than the window's: this app has one window and the library is what is behind a document, so it lands where the brand mark, the folder button and `Ctrl+O` land — `go_to_dashboard`, which autosaves, drops the crash snapshot and re-scans. ⚠️ **It is therefore a fourth door onto one screen, and that is the stated cost**: the row above and this one differ in what the user is thinking — "take me to my files" against "I am done with this" — not in where they arrive. The alternative read of *close* was the window, reusing `handle_close_request`'s unsaved-work modal, and it was put to the maintainer rather than assumed |
| `Ctrl+Shift+E` | ~~export~~ | ⛔ **unbound 2026-08-22**, and deliberately. It was *Export as…* from 2026-08-20 (`decisions.md` §15 D264) — the selection to an SVG or PNG file, the save dialog picking the format — and went with its menu row when the inspector's Export panel (§15 D274) turned out to answer where, what format and at what size from the layer, with a button that runs the export. Two doors onto one file, and this was the slower. ⚠️ **The chord is unbound but not free**: plain `Ctrl+E` is *Flatten*, so the `!m.shift` on that arm is now the only thing between a hand still reaching for this and a dissolved selection. `a_stray_shift_on_ctrl_e_does_nothing_rather_than_flattening` is the pin, and it flip-fails with `[Flatten]` |
| `Ctrl+Alt+E` | export all | ✅ Not Figma's — Figma has no repeat. Re-runs every layer's saved export settings into the folder the last export went to, asking only the first time (`architecture.md` §7, `decisions.md` §15 D274). **Alt *and* not-Shift**, both stated: this keymap tells `Ctrl` chords from `Ctrl+Alt` ones by Alt alone, and `Ctrl+Alt+Shift+E` should be neither of these rather than both — the same guard `Ctrl+Alt+S` once failed, firing *Save* on every boolean subtract. The row above used to state `!alt` as the other half of this pair and no longer states anything, being unbound |
| `Ctrl+,` | **settings** | ✅ bound 2026-08-24 (§15 D330). VS Code's, and every editor that copied it; Figma has no keyboard route to its preferences at all. `,` was **completely unspent** in every combination, so this chord cost nothing and had no collision to argue about. ⚠️ **It opens and cannot close**: `resolve` is not called at all while the modal is up, because a modal owns the keyboard — so `Action::OpenSettings` is named for what it does rather than for a toggle it cannot perform. Escape, the ✕, *Cancel* and the backdrop are the four ways out. `cmd_only && !m.shift`, like `Save`, so `Ctrl+Shift+,` and `Ctrl+Alt+,` stay unbound rather than becoming second doors |
| `Ctrl+Shift+K` | **place image** | ✅ Figma's. See `architecture.md` §5.5a |
| `Ctrl+K` | ➕ command palette | the modern convention |
| `Ctrl+/` | ➕ command palette | Figma's quick actions, alias |
| `Ctrl+Shift+/` | ➕ shortcut cheatsheet | accept `Key::Questionmark` too |

⚠️ **`Ctrl+K` and `Ctrl+Shift+K` are adjacent and unrelated** — palette and place-image. Accepted
rather than overlooked: `Ctrl+Shift+K` is Figma's and worth matching, while `Ctrl+K` for the palette
is *this file's own* choice (VS Code, Slack, Linear), not Figma's. A stray Shift opens a file dialog
instead of a palette, which is instantly visible and `Escape` undoes it. If that ever grates, the
one to give up is `Ctrl+K` — `Ctrl+/` is the binding with a convention behind it.

**Place image is not a tool letter.** Every letter in §1 arms a persistent mode; this opens a file
dialog and then *loads the cursor* with the chosen images, placing one per click and returning to
Select when they run out (`architecture.md` §5.5a, §9.4). The rail's image button is the same action, not a
tool switch — arming `Tool::Image` *is* picking files, so the button and the chord run one function.
A click places at the fitted size; a drag draws the box, as every other box tool does.

### 8a. The library's own keys — the dashboard only

| Key | Action | |
|---|---|---|
| `←` `→` `↑` `↓` | **move the selection** | ✅ bound 2026-08-28 (`decisions.md` §15 D374). The grid steps in reading order, so `→` off the end of a row is the start of the next and the fourth card is not a wall; `↑`/`↓` are a whole row. In the **list** view the verticals are single rows and the horizontals do nothing, because a full-width row has nothing beside it. Off either end **clamps** rather than refusing — `↓` from a column the short last row does not reach lands on the last card, which is Explorer's answer and the one with no dead press in it. With nothing selected, `↓`/`→` enter at the top and `↑`/`←` at the bottom |
| `Enter` | **open the selection** | ✅ the same verb as a double click, and it reads the same predicate — ⚠️ **which is false in the trash**. Opening a document inside `.trash` puts the editor on a file the library does not list and the purge will delete out from under it; the ⋮ menu had said so since the trash was built, and the double click had been ignoring it |
| `Delete` / `Backspace` | **delete the selection** | ✅ Through the confirmation, not straight to the act — a menu row was aimed at, where a key can be the tail of a chord meant for something else. In the trash the confirmation says *Delete permanently*, which is a card that had never been on screen: the ⋮'s own *Delete permanently* skips the modal |
| `Escape` | **clear the selection** | ✅ the keyboard's version of a click on the empty body, which was the only other way to put one back. With the search overlay up it belongs to the overlay, which closes instead — and with a file's **⋮ menu** or the **All projects ⌄** dropdown open it closes that. ⚠️ That last one is not a nicety: every card on this screen is a `settings::card_modal` and gets Escape and the backdrop free through `should_close`, while the two menus are bare `Area`s that took only an outside click. The hole was invisible while nothing here read the keyboard — and the dropdown kept it until 2026-09-08, being the one of the two nobody had named (§15 D578) |
| `F2` · `Ctrl+R` | **rename the selection** | ✅ bound 2026-08-28 (§15 D383). The ⋮'s *Rename*, on **both** the keys §4 already binds to renaming a layer — the library matching the editor being worth more than either key being unique. Pre-filled from `Entry::display_name` — the stem with a sync client's conflict suffix taken off — because the field the ⋮ opens is filled that way and handing the raw stem back would let the user save the suffix. Refused in the trash. ⚠️ **The two are read on opposite sides of the `modifiers.is_none()` gate** — `F2` bare, `Ctrl+R` as a chord — which is why the verb is a function (`start_rename`) rather than two arms: a rule added to either site would otherwise be a rule only one of the keys obeyed |
| `Ctrl+D` | **duplicate the selection** | ✅ bound 2026-08-28 (§15 D383). The ⋮'s *Duplicate*, which is `Ctrl+D` in the editor for the selected layer (§4) — one chord, one verb, two nouns. Refused in the trash. ⚠️ **The refusal is not what makes it silent there**, and the flip proved it: `Act::Duplicate` looks its subject up in `library.entries`, which does not hold the trash, so a trashed duplicate was already inert by way of another module's scan. `F2` has no such accident — it reads the `entries` the keymap is *handed*, which in the trash is the trashed list |

⚠️ **None of these goes through `input::resolve`, and none of them can.** That function is the
*editor's* keymap and is not called at all while the library is up — `OndinApp::ui` returns before it
— so `panels::dashboard` reads the context directly, exactly as `Ctrl+K` above already did. The
guard in its place is a list of the things that own the keyboard while they are up: the two
confirmations, the ⋮ menu, **the *All projects ⌄* dropdown**, the *Move to project* sheet, the *New
project* and *Settings* cards, the rename field, the search overlay, and
`egui_wants_keyboard_input` for anything holding a caret. ⚠️ **The dropdown was missing from this
list until 2026-09-08 and from the code with it** (§15 D578): it is the ⋮ menu's own shape and owned
no keyboard at all, so `Escape` left it open and cleared the card cursor behind it, `Delete` opened
the confirmation underneath it and an arrow stepped the cursor behind it. It shares the ⋮'s early
return through `library_menu_open()` rather than being a term of its own, which is the predicate the
pointer side already asks. Plus
`modifiers.is_none()`, which is what keeps `Ctrl+K` from also stepping the selection. ⚠️ **That last
one was written to leave every chord here free to be bound later, and three of them now are** —
`Ctrl+N`, `Ctrl+D` and `Ctrl+R`, read *above* that gate rather than exempted inside it, so everything
not named is still refused by one line. The third of them was added a day after the first two and cost
exactly the one arm this predicted, which is the claim that had not been tested when it was written.

`Ctrl+N` is bound here too, and is **not** a row of its own in the table above: it is §8's row, which
means the same thing on both screens (only the destination is read differently), and a chord listed
twice is two chords by the next reading.

Still unbound here, and listed so the next pass starts from the gap rather than from the survey:
`Home`/`End`, and `Ctrl+A` with everything else that would need a **multiple** selection —
`DashboardState::selected` is one path, and every verb on this screen takes one document. ⚠️ **The
multiple selection is a decided non-goal as of 2026-08-28**, not a gap: *"we don't have
multi-selection and I don't think we will"*. So `Ctrl+A` is off this list in the same breath it was
last on it, and the four verbs are each one document by design rather than by omission.

Palette and cheatsheet both fall out of the command registry described in `roadmap.md`'s
*Command palette* section — the cheatsheet **is** this file rendered from the registry,
which is the argument for extending `Action` rather than keeping a second list of strings.
When that lands, this document becomes the registry's test rather than its source.

## 9. Gesture modifiers

Already built: space+drag pan · `Shift` constrain / keep ratio · `Alt` duplicate-drag ·
`Alt` resize about the box centre · `Alt` **draw** about the press point ·
`Ctrl`+side-drag skew · `Shift` 15° rotate and skew
snap · `Ctrl`+click deep select · `Alt`+`Shift`+click key layer · `Ctrl`+wheel zoom ·
`Shift`+wheel horizontal scroll · `Shift` pen constrain · `Ctrl` pen/node curvature ·
`Alt` hover measure (decided, unbuilt) · middle-mouse drag pans, in **both** modes
(`normal_mode_input` had it all along; a text session got it with §15 D167, where Space is
deliberately *not* the alias because there it is a character).

✅ Three added 2026-08-18, each a convention in both families:

- **`Space` held mid-draw repositions the uncommitted shape.** Figma and Adobe both. The
  single most missed gesture when absent. `Drag::Create` carries the previous frame's pointer
  so the shape moves by the *delta while the key was held* — an absolute reading would snap it
  to wherever the cursor was when Space went down. The cursor stops promising a pan while one
  of these is in flight, which it was doing on the strength of Space alone.
- **`↑` / `↓` while dragging a polygon or star adds / removes sides.** Illustrator exact.
  `POLYGON_DEFAULT_SIDES` and `STAR_DEFAULT_POINTS` are now what the gesture *starts* at, with
  the live count on `Drag::Create` and `tools::step_sides` clamping it to the model's own
  `MIN_SIDES..=MAX_SIDES` — the same pair the inspector's field scrubs between, so a shape
  cannot be drawn with a count the field would refuse.
- **`Alt`+wheel zooms.** Adobe's; a free alias beside the existing `Ctrl`+wheel.

✅ Two more 2026-08-22, both reported as the same kind of gap — a modifier that works on one gesture
and not on its twin (§15 D289):

- **`Alt` held while *drawing* grows the shape from the press point**, as it already did while
  resizing. Figma, Illustrator and Photoshop all read it this way, and it is what one actually wants
  for an ellipse: click where the centre goes, `Alt`+`Shift`, drag out a circle around it.
  `canvas::create_box` constrains first and mirrors second, which is what makes the chord centre the
  square on the press rather than near it.
- **A plain click with the Image tool places the picture at its own size** (§15 D295) — intrinsic,
  aspect kept, shrunk only if it exceeds the frame. The gesture was written and unreachable: the tool
  had no click arm at all, so a loaded cursor could only be discharged by dragging a box. Every
  creation tool's click now goes through one exhaustive table, `Tool::click_makes`.
- **A plain click with Rect / Ellipse / Polygon / Star creates one at 100 × 100** (§15 D291), which
  is not a modifier but is the third reading these tools give a press and belongs beside the other
  two. `Alt` centres that box on the click rather than doubling it — the one place Alt's two
  gestures differ, because a click authors no extent to double.
- **A click while Space is held does nothing, for every tool** (§15 D293). Space+drag pans, so a
  Space+click is that gesture having not travelled. It used to fall through: a caret with the Text
  tool, a changed or cleared selection with Select. The guard was written once per arm and carried by
  one of three; it is one binding now.
- **`Alt`+`Shift` reaches the resize handles.** Either modifier alone already worked; both together
  did nothing, because the `Alt`+`Shift`+click key-layer chord refused to start *any* select drag and
  the handles were behind that refusal. The refusal is right on the artwork — a slipped designation
  would otherwise leave an Alt-drag duplicate — and means nothing on chrome the selection already
  owns, so it moved below the handle arms. It read as dead rather than as refused: the cursor was
  promising a resize the whole time, since `select_cursor` never saw the chord.

**The arrows had a second claimant and it had to be given up.** They nudge the selection in
Normal mode, and `input::resolve` is deliberately a pure function of one frame's input that
knows nothing about a gesture in flight — so `dispatch` drops a `Nudge`, and a `SizeStep` beside it
(§15 D346), for as long as a create drag is up. **The step is now a parameter of that function and not a constant in it**
(`input::NudgeStep`, §15 D330), which keeps the purity the sentence above rests on: a preference
the keymap *read* would be state, where one it is *handed* is still one frame in, one list out. Without that, drawing a heptagon walks the previously selected layer four
steps across the artboard, which is the quiet half of the collision.

⚠️ `Alt`+arrows already nudge in Normal mode — the nudge block is gated on `!cmd` but not
on `!alt` (`input::normal_mode`, the nudge block). That is fine (it is the same nudge), but it means Alt+arrows are
**occupied** in Normal mode and cannot later be given a different meaning there. In
TextInsert they are letter spacing, which is a different mode and therefore not a collision.

**That `!cmd` gate is what left `Ctrl`+arrow free, and all four are now spent** (§11, §15 D346):
`Ctrl`+arrows resize the selection. It is the **same table** as the nudge above rather than a second one
— the box is held by its top-left, so the arrow names the edge that moves for both verbs, and
`normal_mode` picks between them on the modifier. The resize arm is gated on `cmd_only` — `Ctrl`
*without* `Alt` — because of the sentence above: `Alt`+arrow is already the nudge, so `Ctrl`+`Alt`+arrow
is a chord one of whose halves is spoken for, and it stays unclaimed rather than picking up a second
meaning nobody chose.

## 10. Text insert mode

`Mode::TextInsert` used to return an empty `Vec` for everything, deliberately — the canvas
editor owns every key so the buffer cannot drift from the document. **Decided, and built
2026-08-18: let *modified* chords through and keep every bare key exclusive.** The
exclusivity argument is about the buffer, and `Ctrl+B` does not touch it. `input::resolve`'s
TextInsert arm is now `text_insert_mode`, and every binding in it carries a modifier —
`no_bare_key_is_a_command_in_a_text_session` is that invariant rather than a spot check.

**The clipboard trio is the one part of this section that is *not* an `Action` and never can be.**
`Ctrl+C`, `Ctrl+X`, `Ctrl+V` and `Ctrl+A` in a session address the **buffer**, not the document, so
they belong beside `Home` and `End` in the editor's own loop by the same test the `Tab` row below
passes — and `input::resolve` resolves nothing clipboard-shaped in `Mode::TextInsert` anyway. Copy and
cut did nothing at all until 2026-08-18, which made this a live bug rather than an unbuilt row: text
could be pasted *into* Ondin and never copied *out* of it (§15 D217, `context-menus.md` §9.7). The
three events are gated on `!alt && !shift` in that loop exactly as §L2 gates them in
`input::normal_mode` — `AltGr` **is** `Ctrl+Alt` on Windows and `is_cut_command` does not examine
Alt, so an `AltGr`+`C` meant to spell a character arrives as `Event::Copy` with the character
already gone. A silent cut is not the better of the two outcomes.

⚠️ **`!shift` since §15 D545, and it closes the other half of the paragraph below.** The same
functions do not examine Shift either, so `Ctrl+Shift+C` — the centre-align binding — was swallowed
and re-emitted as `Event::Copy` too. The paragraph below records that the *press* is stolen; what it
did not say is what the thief then does with it. It **copied the selection to the system clipboard
and toasted *"Copied text"***, so one keystroke centred the paragraph and overwrote the clipboard,
and the toast was the only thing on screen naming what the key had done. Both halves were
individually correct — D212 is why the release is read, D217 is why the loop reads `Event::Copy` —
and neither knew about the other.

**`Ctrl+Shift+C` had to be read from the key *release*, and this is §0 in a place §0 did not
predict.** `is_copy_command` tests `command && Key::C` without examining Shift, so the press
is swallowed and re-emitted as `Event::Copy`: a centre-align binding written as
`key_pressed(C)` reads perfectly and fires never. §L2's `Ctrl+Alt+X`, one letter over, and the
same seam fixes it. **The rule §L2 states — any test for a Ctrl+letter chord where the letter
is C, X or V must drive the event or the release — now has a second instance, and it is not a
clipboard binding at all.**

| Key | Action | Cost |
|---|---|---|
| `Tab` / `Shift+Tab` | nest / un-nest the list items the selection touches | ✅ **built** — §15 D173 |
| `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | copy / cut / paste **the buffer's** selection | ✅ **built 2026-08-18** — §15 D217 |
| `Ctrl+A` | select the whole buffer | ✅ built |
| `Ctrl+U` | underline on / off | ✅ built |
| `Ctrl+Shift+,` `.` | font size ∓ 2 | ✅ built |
| `Alt+←` `→` | letter spacing ∓ | ✅ built |
| `Alt+↑` `↓` | line height ∓ | ✅ built |
| `Ctrl+Shift+L` `C` `R` `J` | align left / centre / right / justify | ✅ built — and `C` is read from the **release** |
| `Ctrl+B` | ➕ bold | ⚠️ **not a toggle here** |
| `Ctrl+I` | ➕ italic | ⚠️ **not a toggle here** |

**These two carry both markers, and they say different things** — ➕ that the row is unbuilt,
⚠️ that its semantics are not the ones the chord implies elsewhere. They carried only the ⚠️
until 2026-08-22, which made them the two unbuilt rows in the file that a search for ➕ does not
find; the header's count is now the same seven either way.

**`Ctrl+B` and `Ctrl+I` are real work, not wiring.** Weight lives on the `wght` axis or in a
named instance, and italic is *derived* — `text.rs` reads it from a named instance's `ital`
coordinate or parley's `FontStyle`, deliberately, so there is no flag to flip. Both chords
have to mean "select the family's bold/italic face, or move the axis", with a defined answer
for a family that has neither. Note also that a family expressing italic through `slnt`
never reports italic at all, and `slnt` is counter-clockwise-positive — so a naive `Ctrl+I`
implementation can lean the wrong way. Ship the five cheap rows first.

**Underline is exclusive with strikethrough** (the 2026-07-31 redesign: one exclusive
decoration, not two independent ones), so `Ctrl+U` on struck-through text *replaces* the
decoration rather than adding to it. That is the right behaviour and worth a test, because
it is the one row here whose semantics differ from every app it is borrowed from.

**`Tab` is the one row here that is built, and it is *not* a keymap binding** — which is
what keeps it clear of `Tab` stepping the point selection (§11, §15 D123). That one is an
`Action`, and `text_insert_mode` resolves **no bare key**, `Tab` among them, so the two
never see the same key; this is an arm of the canvas editor's own loop, beside `Home`
and `End`. ⚠️ **This said `input::resolve` *"returns nothing at all in TextInsert"* and that
was never quite true** — the styling chords have resolved there since §15 D211 — **and since
§15 D815 four document chords do too** (`Undo`, `Redo`, `Save`, `Open`). The separation
`Tab` relies on is the *bare key* rule, not an empty `Vec`. Only paragraphs with a marker move, each by one and clamped separately, so a
parent and its sublist keep their shape instead of levelling (§15 D173). It is claimed
**whether or not it moves anything**, because a `Tab` that reached egui would move the focus
ring and the guard on this loop is `egui_wants_keyboard_input` — the next keystroke of the
session would be swallowed by whatever button took focus.
`<OndinApp as eframe::App>::ui`'s `took_tab` (§15 D698)
hands the focus back, the same workaround D123 needed and now scoped to both claimants.

**This section is where modality starts carrying load.** Three of these chords mean
something else in Normal mode — `Ctrl+U` is snap-to-shapes (§6), `Ctrl+Shift+L` is lock
(§4), `Alt`+arrows are the nudge (§9) — and `Tab` above is a fourth. Nothing disambiguates
them but `Mode`, which is
exactly what `input.rs`'s routing was built for and the first time the keymap has depended
on it for more than swallowing keys. Each of the three owed a paired test — the chord in
Normal mode, and the same chord in TextInsert — and
`the_colliding_chords_mean_one_thing_per_mode` is all three at once.

**The arrows are the harder case, because `Mode` is not what separates them there.** Inside a
session both the editor's own key loop and the keymap read the same four keys from the same
frame, and nothing makes the two consult each other — so *both fire* and *neither fires* are
each reachable by editing one side alone. Both are visible: both walks the caret while the
tracking widens, neither leaves the arrow key dead. `Ctrl+Alt+←` was in fact dead — the editor
yielded on `alt` while the keymap took only `!command && alt` — and it was found by the test
that sweeps all eight modifier combinations asserting the two conditions **partition** the
space, rather than by the test that checks the case somebody thought of. The editor now yields
exactly what the keymap takes.

## 11. Structure, history and navigation — all already bound

Nothing proposed here; this is the rest of the keymap, so the file is a complete inventory
and the cheatsheet can be rendered from one source.

| Key | Action | |
|---|---|---|
| `Ctrl+Z` | undo | ✅ |
| `Ctrl+Shift+Z`, `Ctrl+Y` | redo | ✅ both spellings |
| `Ctrl+A` | select all | ✅ |
| `Ctrl+G` / `Ctrl+Shift+G` | group / ungroup | ✅ |
| `Ctrl+E` | flatten | ✅ Figma |
| `Ctrl+Alt+M` | use as mask / release it | ✅ Figma — bound 2026-08-21 (§15 D286). `M` was **completely unspent** in any combination, so this chord cost nothing and had no collision to argue about; `masking_is_a_chord_and_the_bare_letter_is_still_unspent` asserts the chord *and* that plain `M` and `Ctrl+M` stay free, which is the half that matters — the risk is someone later giving `M` a tool |
| `Ctrl+Alt+U` `S` `I` `X` | union, subtract, intersect, exclude | ✅ Figma — `X` is read from the **release**, §L2 |
| `Ctrl+]` / `Ctrl+[` | bring forward / send backward | ✅ Illustrator, Figma |
| `Ctrl+Shift+]` / `Ctrl+Shift+[` | bring to front / send to back | ✅ curly spellings handled |
| `Shift+D` | reverse subpath direction | ✅ new with §15 D125 — no app has a binding for this, so it follows the app's own plain-plus-Shift pattern |
| `Delete`, `Backspace` | delete | ✅ context-sensitive: pen point, path points, then layers |
| `←` `→` `↑` `↓` | nudge (1 by default) | ✅ **the one distance in this table the user can change** — *Settings › Nudge › Step*, `input::NudgeStep::small`, persisted in `prefs.json` (§15 D330). A struct rather than two `f64` parameters because the pair crosses three seams and the swap compiles |
| `Shift`+arrows | nudge (10 by default) | ✅ *Settings › Nudge › Shift*, `input::NudgeStep::large`. Not constrained to be larger than the plain step: someone who wants the modifier to mean *finer* is not making a mistake |
| `Ctrl`+arrows | resize the selection — `→` wider, `←` narrower, `↓` taller, `↑` shorter | ✅ new 2026-08-25 with §15 D346, and **undesigned territory** — this file reserved no `Ctrl`/`Cmd`+arrow row at all, and the only convention behind it is the request's own *"Figma does something similar"*. The distance is the **nudge** step, the same `NudgeStep` the bare arrows read, so `Shift` gives 10 and a retuned nudge retunes this too. Held by the **top-left corner**, which is what makes `↓` taller and `↑` shorter, and what lets the two verbs share **one** table of signed pairs in `normal_mode`. `Ctrl+Alt`+arrow is deliberately left unclaimed, the binding being gated on `cmd_only` |
| `Tab` / `Shift+Tab` | step the point selection along the path | ✅ node tool only — and see §10, where the same key nests a list in TextInsert |
| `Enter` | step **into** the selection, or back out | ✅ all five kinds, 2026-08-19 (§15 D228) — text, picture, path, group, boolean, in `double_click_pick`'s order so the key and the double-click cannot disagree. On text with no pointer to place a caret from it **selects the whole string**; a group is a *depth* rather than a mode, so `Escape` is the way back out of that one |
| `Escape` | unwind one rung | ✅ |

**The Escape ladder is part of the keymap and belongs in the cheatsheet.** Read off
`OndinApp::escape` on 2026-09-22, it is **twelve** rungs (§15 D125, D821, D828):

present mode → the detached colour picker → a text session → the pen → **a gesture in flight**
(`cancel_gesture`) → **a chrome field that just lost focus** → pen bias armed over an edit →
the point selection → the pivot handle → the entered group → a tool that is not Select →
**the layer selection**, which is the fall-through `else` under everything.

🚨 **This list read "session → gesture in flight → a chrome field that just lost focus → pen
bias armed over an edit → point selection → layer selection → Select tool" until 2026-09-22,
and it was wrong in both ways a list can be.** It omitted five rungs — present mode, which is
the **first** arm, the picker, the pen, the pivot handle and the entered group — and it put
**the layer selection above the tool when the code puts it below**. The order is the whole
content of a ladder, so an inverted pair is not a smaller error than a missing rung: a reader
pricing "what does one more press cost here" got the opposite answer. ⚠️ **The same inversion
was live in `context-menus.md` §10 at the same time**, where it read that the selection is
cleared on the same rung as the menu *"where present mode and an entered group are further
down the ladder"* — both of those are further **up**. Two files, one wrong belief, and
`cargo doc` cannot see either: **a prose ordering is the least checkable thing this project
writes down.** Re-derive it from the `else if` chain rather than editing the sentence.

⚠️ **The chrome-field rung sits *below* the gesture and not above it** (§15 D821): a valved
numeric field's cancel has to go on reaching `cancel_gesture`, which is what stops the release
committing after all (§15 D317), so it is only ever taken when nothing was in flight — the
plain `TextEdit` case, where renaming a layer and pressing `Escape` used to clear the
selection too. It is the one key whose meaning is a sequence rather than an action,
and `Enter` is deliberately its counterpart rather than its alias — a toggle in and out
against a ladder that unwinds one press at a time. **The counterpart is not symmetric, and that is
deliberate** (§15 D228): `Enter` toggles the two *tools* it can enter — the node tool and image
editing, which are modes you are either in or out of — and only ever goes **deeper** into a group,
because a group is a depth and `Enter` on a group inside a group has an obvious meaning. Coming back
out of those is the ladder's job, and always was.

**Anything floating takes the press before the ladder sees it, and takes only one thing with it**
(§15 D527, D801). A context menu, a top-bar dropdown, a modal, the detached colour picker and the
five inspector and Export popovers each answer `Escape` by closing, and none of them also pays out a
rung — dismissing something opened by accident must not deselect, leave the group or drop the tool in
the same keystroke. ⚠️ **The five popovers were the half nobody had measured**: until 2026-09-19 the
typography, stroke and effects popovers closed *and* dropped a rung, and the Export card's two did
not read the key at all, so `Escape` dropped a rung and left the popover standing. ⚠️ **Image
editing's card is deliberately not in that set** — it is a tool, so `Escape` leaving it *is* the
ladder's own last rung, and gating the key on it would swallow the way out of the mode.

**For image editing `Enter` is now the *only* keyboard door**, since §15 D268 took the letter and the
rail slot away. That raises what was a convenience to load-bearing: a picture selected with the mouse
and `Enter`ed is the whole keyboard path in, and `Escape` or a second `Enter` is the path out.

**It refuses on a locked layer since 2026-08-23** (§15 D320), and refuses in
`OndinApp::begin_image_edit` rather than in the key's arm, so the chord and the *Edit image* menu row
give one answer — a menu that refused where the chord allowed would be §15 D261's failure. **All three
`Enter` targets are like this now** (§15 D321), the day after: `canvas::begin_edit_text` and the `Path`
arm each refuse too, and all three ask `query::is_effectively_locked`, so a locked *group* shuts the
door on what is inside it. Text and points were the asymmetry the wrong way round — both menu rows dim
and had done all along, while the key opened the mode — which is why closing them mattered more than the
picture. Points has two doors that are not `Enter` — `A` and the rail's node button, which arm the tool
with no lock check — and they are shut one level down, in `canvas::edited_path`: a locked path is not a
subject, so the tool arms and finds nothing, the way pressing `A` over a rectangle already does.

**And for a while those two keys were the only way out at all** — the rail button D268 removed had
been an exit as much as an entrance, which is a thing worth noticing about any mode whose entry is
being redesigned. The **Done** pill under the crop window is the pointer's exit (§15 D270); it runs
the same `choose_tool` these keys do, so the three are one verb rather than three.

⚠️ `Delete` and `Backspace` are read with **no modifier gate at all** (`input::normal_mode`, the selection block), so
`Ctrl+Backspace` and `Alt+Delete` also delete. Harmless today and worth knowing before a
chord that ends in either key is added. Note also that on Windows `Shift+Delete` is
intercepted as `Event::Cut` by `egui_winit` and never arrives as a key — cut rather than
delete, which is arguably correct and is certainly not what the binding says.

---

## Deliberately unbound

- **Select-all-with-same** — no convention in any of the five apps. It belongs in the
  palette, where the registry can enumerate *which* "same".
- **Outline shape** — built and reachable from the context menu (§15 D230), and deliberately given no
  chord: it is a **one-way** structural conversion, and none of the five reference apps binds one
  either. `Ctrl+E` is *Flatten*, which is a different verb on a different subject and must not come to
  cover this one by having the only key. This is the third reason an `Item` in the registry is not an
  `Action` — not "a keyboard cannot express it", just "no key was spent".
- **Numeric distribute spacing** — a panel affordance, not a key.
- **`Tab` to hide panels** (Adobe's) — `Tab` steps the point selection, and `Ctrl+\` covers
  the intent. `Tab` is also egui's focus key and costs a frame to claim (§15 D123); it is
  the last key in this app worth spending on a second meaning.
  - **A second meaning was spent, on nesting a list in TextInsert** (§10, §15 D173), and
    this bullet is the argument for why that one is affordable and hiding panels is not:
    the two live in different `Mode`s and neither is reachable from the other, where a
    global panel toggle would have to out-rank both. It cost exactly what this bullet
    predicted — one line in `took_tab` — because D123 had already paid for the machinery.
- **`Shift`+digits 3–9** — held for blend modes (§2).
- **`Ctrl+Shift`+ the key of a bound `Ctrl` chord** — never a second spelling of that chord, and
  guarded now rather than merely unwritten (`decisions.md` §15 D710). This file had been stating the
  rule one row at a time — `Ctrl+Shift+S` under §8's *save as*, `Ctrl+Shift+E` under *export*,
  `Ctrl+Shift+,` under *settings* — and it is general: **a chord that does nothing is the honest
  state for one whose action was removed or was never bound**, and it leaves the combination
  available. Eight arms of `input::normal_mode` were answering the shifted spelling silently until
  2026-09-09 — `Ctrl+Shift`+ `O`, `A`, `D`, `Y`, `−`, `0`, `1` and `2` — of which `Ctrl+Shift+D` was
  a **document edit** and an undo step, one modifier from §11's `Shift+D`.
  `ctrl_shift_is_not_a_second_spelling_of_ctrl` asserts both directions for each of the eight.
  - ⚠️ **Four Shift readings are load-bearing and must not be swept up with them**, all four tried
    and all four failing a test that already existed. `Ctrl+Shift+=` is the **primary** spelling of
    `Ctrl` `+` (§3, and §0's L1 for the mechanism) — `Key::Plus` is unreachable without Shift, so a
    blanket guard removes zoom in. `Ctrl+Shift+C` and `Ctrl+Shift+X` must still copy and cut, because
    the centre-align they would collide with is the **text session's** (§10) and does not exist in
    this keymap. `Ctrl+Alt+Shift+V` must stay a property paste: Alt beats Shift on a paste, and a
    hand still resting on Shift is the case that rule was written for (§7). And `Ctrl+Shift+Z` is
    Redo (§11), so only the `Ctrl+Y` half of that arm took the guard.
  - 🚨 **The rule has to be kept twice, because the dashboard is a second keymap** (§15 D712).
    `input::resolve` is not called at all while the library is up, so none of the guards above
    reaches that screen: `panels::dashboard::search_field` was answering `Ctrl+Shift+K`,
    `Ctrl+Alt+K`, `Ctrl+Shift+P` and `Ctrl+Alt+P` with §8's *search files and projects* until
    2026-09-09, while `dashboard_keys` beside it had spelled the rule correctly — and with Alt in it,
    which §8's `Ctrl+Alt+E` row states as the other half (*"this keymap tells `Ctrl` chords from
    `Ctrl+Alt` ones by Alt alone"*). **Those four spellings are deliberately not listed as rows**:
    the rule covers them, and a list of combinations is a count nothing checks. **A keymap rule
    stated for one screen holds nothing on the other** — §8a's keys are the other reader.

## Unresolvable collisions, recorded so they are not re-argued

1. **`Ctrl+0` / `Ctrl+1`.** Sketch and Figma say 100% / fit; Adobe says fit / 100%. We
   follow Sketch. No binding serves both and there is no third key that means either.
2. **`Ctrl+R`.** Figma says rename; Illustrator and Photoshop say rulers. We say rename,
   because rulers already have `Shift+R` (§4).
3. **`Shift`+digits.** Figma says zoom; Adobe says blend modes. We follow Figma for 0/1/2
   and reserve 3–9 so the Adobe reading stays possible for the rest (§2).

## Implementation ledger

**Built 2026-08-18**: `ZoomSelection`, `OpacityDigit(u8)` (the *digit*, not a percentage —
the two-digit window needs state the keymap deliberately does not have), `ToggleHidden`,
`ToggleLocked`, `Rename`, `Flip(Axis)`, `Align(Axis, Edge)`, `Distribute(Axis)`, and
`TextStyle(TextChord)` for the TextInsert set.

**The nine separate view toggles this list asked for became one.** `ToggleGrid`,
`ToggleSnap(kind)`, `TogglePresent`, `ToggleLayersPanel` and `ToggleToolbar` are
`ToggleView(ViewSwitch)`, and `ToggleRulers` / `ToggleGuides` / `ToggleGuideLock` were folded
into it — that is the consolidation this pass was actually for. `ViewSwitch` is also what both
dropdowns render their rows *from*: they were keyed by the row's own string
(`set_view_toggle("Show grid", on)`), which held the row and the state it drives in step only
for as long as nobody typed a different string, and the chords are a second caller that never
sees the row at all. One enum, two callers, one `label()`.

**Still to build**: `New`, `Close`, `Export`, `Palette`, `Cheatsheet`. ~~`PasteInPlace`,
`CopyProperties`, `PasteProperties` (§7, deferred whole)~~ — **all three built 2026-08-20**
(§15 D248, D257); this line outlived them by two days, and §7's own heading said so while this
one did not.

Not keymap work, and should not be filed as such:

- **the clipboard trio and `Ctrl+A` inside a text session** (§10) — built 2026-08-18 and
  deliberately not `Action`s, for the same reason `Tab` is not one: they address the buffer. Copy and
  cut were the two of the four that did nothing at all before then (§15 D217).
- **`Tab` / `Shift+Tab` nesting a list** (§10) — built, and the reason it is listed here is
  that it never became an `Action`. **The prediction beside it held**: "a chord that reaches
  the *document* rather than the buffer is the one that wants an `Action`" is exactly the line
  the rest of the TextInsert set fell on. All five of them restyle the document through the
  session, so all five are `Action::TextStyle`; `Tab` changes which paragraph the caret is in,
  and stayed in the editor's loop. The arrows are the seam, being claimed by one of each.
- `Action::ZoomSelection`'s behaviour — splitting fit-everything from fit-selection (§3).
  **Done**, and it was the larger half of that row: `content_bounds` became two functions and
  `reveal_selection` turned out to have been reading the wrong one all along.
- ~~**the whole of §7** — paste-at-pointer, paste-in-place and the properties clipboard.
  Deferred as a group 2026-08-18.~~ **The group is gone**: paste-in-place and the properties
  clipboard were built 2026-08-20 (§15 D248, D257) — the shared `!m.alt` blocker turned out to
  be true of only two of the four rows — and paste-at-pointer is a decided non-goal as of
  2026-08-22 (§15 D300). *A deferral kept after its members land is how a closed section reads
  as open work.*
- ~~`Ctrl+N`, `Ctrl+W` — wait on the dashboard (§8)~~ **both bound 2026-08-28** (§15 D383), two days
  after the dashboard they were waiting on. Neither needed a mechanism it did not already have:
  `Ctrl+N` is *New file* with the destination computed at the door, and `Ctrl+W` is the walk back that
  three other doors already take. ⚠️ **The two rows had sat here since before the library existed**,
  which is the shape of entry this list is worst at: nothing recompiles when the thing an item waits
  on arrives, so a chord can stay "waiting" for as long as nobody re-reads the premise.
  ~~`Ctrl+Shift+E`~~ **bound 2026-08-20** to
  *Export as…* (`decisions.md` §15 D264); it never waited on "export" as a subsystem, only on a raster
  walk that could name layers — **and unbound again 2026-08-22**, the verb removed when the inspector's
  Export panel made a menu row that asks three questions and remembers none of them the slower of two
  doors onto one file. It is off this list in both directions now: not waiting on anything, and not
  bound to anything. ⚠️ **Not free either** — §8's row says why
- `Ctrl+B` / `Ctrl+I` — a font-selection feature, not a toggle (§10)
- the two live bugs, which were fixes to what was already there — **done 2026-08-18**
  (§L1, §L2; `decisions.md` §15 D204, D205)
