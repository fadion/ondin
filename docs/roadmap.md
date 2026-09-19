# Ondin — Roadmap

> **What is left to build, what is deliberately not being built, and what is parked.** This was
> `todo.md` until 2026-08-19; the file of that name is now the maintainer's private scratchpad and is
> not tracked. Everything of record moved here.
>
> **Completed work is not logged here** — it lives in `architecture.md` and in `decisions.md` (§15).
> A finished item's record moves there and is struck from this file; it does not live in both. **A
> decided non-goal is never an open item** either: §0 below is what stops those being re-argued.
>
> Refs are `file` + function name, never line numbers — files move. **Visual and panel specifics for
> the design blocks below live in `design/` and hand over from there.** This file keeps work items,
> ideas, and the technical points a mock cannot carry.

## How this is organised

Sections are prefixed with when they are wanted, not with what they touch:

| Prefix | Meaning |
| --- | --- |
| **Now** | Open work on features that have already shipped — defects, gaps and polish on live code. |
| **Next** | Designed, decided, unbuilt. Each carries the decisions already taken, so the build starts from them. |
| **Later** | Post-v1, or parked with a stated reason. Non-blocking by construction. |

Two sections carry no prefix: *Principle* is a standing rule rather than work, and *Decided
non-goals* is the list below.

### Everything open, 2026-08-31

**A table of contents, not a copy** — each row says which section holds the item and nothing else, so
it cannot drift into a second, worse statement of the work. ⚠️ It *can* still outlive its subject,
which is this file's own §9.5 lesson: **check the section before believing the row.**

| Where | Open |
| --- | --- |
| *Now · Images* | *Embed*/*Link* and *Relink…*, plus *Embed all* — all three blocked on nothing being able to author a linked source. |
| *Now · SVG import* | **Built the day it was asked for**, all of it (§15 D394). What is left: ~~a `<filter>` that is more than one `<feGaussianBlur>`,~~ `<foreignObject>`, CSS combinators, the rejoined **paragraph**, ~~and an **elliptical radial gradient**~~ — each skipped or approximated *and reported*, so **nothing here is lost silently**. **Filters left this row on 2026-09-03 by being read whole** (§15 D411): the shadow chain, `<feDropShadow>` and the writer's identity `<feOffset/>`, which is what closes *Copy as SVG* for a shadow — until then every effect this app **exported** came back as nothing, reported and lost. ⚠️ **The same audit found the gradient's reason wrong** — not "the model holds circles" but a missing gradient transform, both backends already having the mechanism — **and it left this row the next day by being built** (§15 D412), the field landing on the brush rather than on `Fill` so a gradient-stroked shape gets it too. **`<textPath>` left this row on 2026-09-01 by being read** (§15 D406), hours after the writer started emitting it. ⚠️ **This row was stale in two directions when D405 read it**: it still listed per-run **font** properties as the one silent loss, which §15 D398 built hours after it was written, and it named the elliptical gradient, which appears nowhere in the section below — the file's own §9.5 lesson, twice in one row. |
| *Now · Canvas* | `Event::Zoom` (a macOS portability item, untestable here) and touchscreen pinch (needs a touchscreen) — **nothing else** *of the gestures*, as of 2026-09-01: the extreme-zoom crash closed with §15 D402 and the last unbounded term with **D403**, hours apart. ⚠️ **Two context-menu items were open under this section as of 2026-09-07** and the *"nothing else"* predates both — a frame's *Background…*, and C5's edge clause (§15 D469), which are decisions rather than work. **The first was ruled on 2026-09-19 and is a decided non-goal** (§15 D800), so C5's edge clause is what is left of the pair. 🚨 **Four context-menu tests plus a ruler-origin test were added here on 2026-09-09** (`[A7-L8-06]`) as the only **work** in this row, and both are **closed 2026-09-19** — §15 D795 and D797 — leaving the one decision above. They had been queued in `context-menus.md` §10, §15 **D214** (this row said D226, which is the panel's *Paste* slot) and §15 D36, and in none of them was this table or the section bullet. *The row said "nothing else of the gestures" and was scoped so narrowly that it stayed technically true through three additions; a qualifier that survives everything is not doing the job.* |
| *Now · Text* | Justify-all, tab stops, columns, widow/orphan — **four** now, and **not one of them is short of a dependency**; hyphenation was the last to hold that claim, lost it on 2026-09-01 and left this row the same day for §0 as a decided non-goal (§15 D399), and **text-on-path left it hours later by being built** (§15 D405). Plus the paragraph scope's absence from the MCP snapshot, parked with MCP — to which D405 adds `on_path` itself. |
| *Now · Inspector* | Three items: the hex field commits through no shared valve, which is the routing half §15 D517 left standing when it closed both *measured* halves of `[S14.4-L1-04]` on 2026-09-07 — a **shape** argument, which is why it is one line and not a fix — `char_valve`'s third arm, **which as of 2026-09-19 is a maintainer question and no longer a test** — §15 D802 wrote the test D523 asked for and found the picker's raw sensed regions do not reach the arm at all, and §15 D803 the same day found the one thing that *does* reach it: an egui clamp rewrite `ui::value_field_f64` already opts out of, which makes deleting the arm a second defence against `[S6.2-L1-01]` rather than tidying, so what is open is that ruling — and the menu-row border, which is a layout item rather than a valve one. 🚨 **This row was stale in *two* directions on 2026-09-09 and was corrected by `arch-scribe` reading it against the section.** It named the multi Transform card's **angle** fields, which §15 **D628** closed and struck from the section that same day, and it did not name the menu-row border, which had been the section's third bullet all along — so *"every one is a remainder of a landed fix"* and *"all three are valve remainders"* were both false. **A row that summarises a section is a second copy of it**, and this is §9.5's lesson at the table rather than in a bullet. |
| *Now · Path editing* | One decision: whole-path geometry patches, revisited with MCP. ⚠️ **The `retain_valid`/`subpath_lengths` pair left this row on 2026-09-19**, ruled to the narrow `#[allow(dead_code)]` with the no-effect call removed (§15 D637, *Resolved*) — it had been here since 2026-09-09. |
| *Now · Files, library and storage* | Three, all from the codebase review or from closing one of its findings: a dashboard cover rendered synchronously on the UI thread, a `.ondin` whose filename is not valid Unicode being invisible to the whole library, and the per-machine index having no injection point — so the suite writes to the developer's own cache (§15 D619). ⚠️ **This row did not exist until 2026-09-09** and the section had held open work since 2026-09-06; the *"nothing open"* line below was the half that got corrected first, and a missing row is the same failure with nothing to contradict. |
| *Now · Text alignment* | Side bearings / optical margin alignment, marked *Later*. |
| *Later* | The command palette and cheatsheet; the parked decisions; post-v1 (MCP, Command Mode, multiplayer). |

**Sections with nothing open**: *Doc drift*.
What those sections hold is standing rules and the record of how each item closed; read them for the
traps. ⚠️ **`Keyboard` came off this list on 2026-09-07 too**, and it had been wrong for the length
of one session: §15 D466 left an open question there — whether `Undo`, `Redo`, `Save` and `Open`
should resolve in `Mode::TextInsert` at all — and the entry was written into *Now · Keyboard* while
this line went on saying the section was clear. Which is the shape to watch: **this sentence is a
claim about four other sections and is maintained nowhere but here.**
⚠️ **Booleans was on this list from 2026-08-31, came off it on 2026-09-07 and has been clear again
since 2026-09-19** (§15 D794), when a defect nobody was looking for fell out of a *control* written
for §15 D457 and was then attributed and fixed — which is worth one clause here rather than only in
the section: *nothing open* is a statement about what has been looked at, and the thing that
re-opened it was found by writing the case that was supposed to stay green. 🚨 **It is not back on
the list above, because it was never a section**: the item lived as a bullet under *Now · Canvas and
interaction* while the table carried a row headed *Now · Booleans*, so the pointer named a heading
this file does not have and *"check the section before believing the row"* would have sent a reader
nowhere. That row is struck with the item; this clause is where Booleans' state is kept.
🚨 **`Files, library and storage` came off it on 2026-09-09, and it is the worst of the four,
because the contradiction was *inside the section it named*.** That section's own opening sentence
reads **"Three things are open here"** — it has said some such number since 2026-09-06 — while this
line went on listing it as clear. So the two halves of one file disagreed in plain prose, three
sessions running, and neither is hidden: a reader who followed the pointer would have hit it
immediately. **What found it was `arch-scribe` striking one of those three** (§15 D613's
`Entry::unread` bullet) and reporting that the line above was already wrong before the strike — *"not
mine to fix on this change"*, which is right, and is why it is recorded here as its own repair rather
than folded into that entry. ⚠️ **A count and a list are two claims about one fact, and only the
count is anywhere near the thing it counts.** The cheapest check is the one that caught it: read this
sentence against each named section's own first paragraph, which is one grep and four lines.
⚠️ **`Inspector` came off it the same day**, on the sixth session's last entry, and for a third
distinct reason: §15 D517 closed both *measured* halves of `[S14.4-L1-04]` and declined the third
thing the fix sketch asked for, because the measurements said it would buy nothing. **A declined half
of a landed fix is still open work**, so it is a row above and a bullet in the section — and unlike
Keyboard's and Booleans', it went into all three places in the same edit as the entry that created it.

### An entry's *premise* rots faster than its ask, and nothing recompiles when it does

**Read the code before picking an item up, not after.** Two entries were taken off this file on
2026-08-19 not because they were done but because they had stopped being true, and both had been sitting
here reading as live work:

- ***Scale tool — still no numeric path*** rested on "`size0` is `None` for Group, Line and Path". By the
  time anyone re-read it, `Path` and `Boolean` had W/H from their derived box (D98), a `Line` had an `L`
  (D229) and a multi-selection had had W/H over its union all along. One kind was actually missing, and
  the fix was one `match` arm (§15 D242).
- ***`Node::from_parts` is thirteen positional arguments*** was accurate about the count and wrong about
  the hazard: it scored the transposition risk as latent by counting two of the four call sites, and the
  two it had not counted were passing `true, false, false, 1.0, false` positionally (§15 D240).

**The shape both share is a premise that is a list** — of kinds, of call sites — and a list in prose is
one the compiler never checks. Suspect any entry here whose argument names one, and re-derive it from
`crates/` before estimating the work. There is no reason to think those were the last two; a pass that
re-reads this file *against the code* rather than picking items off it is real work and has never been
done.

**That pass was run on 2026-08-21**, over every open entry in the file, checking each one's *premise*
against `crates/` rather than its ask. **Five had rotted, and every one of them rotted in one of the
two shapes named above.** Three were a list — the *Units* consumers (five, then six, then nine paths,
and ten readouts over eight by the time §15 D358 closed the section without needing any of them),
the image-editing menu row (scored at two sites, actually five), the 212-vs-210 card (scored at two,
actually four, one of them a headroom assertion that could genuinely flip). Two were a dependency that
had arrived while nobody was looking — **masks**, whose whole scene-and-export mechanism already existed
for other reasons and which were **built the same day** once the user asked (§15 D282–D285), and
**decoration skip-ink**, filed under "deferred outright" twenty lines from the call
that supplies the glyph outlines it says it needs — and **built on 2026-08-25** (§15 D356), where the
mechanism the amended entry proposed turned out to be the wrong one. The pass also turned up **three defects no document
carried**: cross-document image paste silently losing the picture, an exported SVG skipping ink where
our own canvas does not, and every committed edit deep-copying every embedded photograph. The first two
were fixed the same day and their records are **§15 D280** and **§15 D281**; the third is an entry
below. Everything not amended in this pass was read on the same date and left standing
deliberately — the flo_curves guard, the winit pinch analysis, the `Ctrl+B`/`Ctrl+I` note and the
format's six git properties among them.

**The pass was run again on 2026-08-31**, over every open entry, and the ten days since had moved
**eight**. It is worth knowing what the second run changed about the *shape* of this warning, because
only half of what it found is the shape the paragraph above describes.

- **Four had rotted outright, and three of the four are lists after all.** *Text-on-path* is the
  skip-ink case exactly repeated — deferred as "needs a dependency", with every dependency in the
  tree, one of them (`kurbo`'s `inv_arclen`) in a crate core has depended on since scaffold time. The
  sentence carrying it, ***"all six dependencies are still genuinely absent"***, was a list of six
  covering **one** situation it described. The **context-menu** bullet points at a §9.5 that no longer
  has an unbuilt row in it. And the **command palette**'s roll of `Action`s without an `Item` named
  `SaveAs`, which does not exist and has not for some time.
- **Four more held and said something false about how they held**, which is the half the paragraph
  above has no name for. An entry can be right about the work and wrong about the reason, the scope or
  the count — *Menu rows*' warning applies to one of two families of row, the linked-image "exactly
  one place" is about the constructor and not the ways a link can reach you, the PNG goldens' "pinned"
  is one field and two defaults, and the parked **multi-selection origin** describes a question the
  code has since answered four times in two directions on purpose. ⚠️ **None of those four would be
  caught by re-scoring the ask**; they are only visible by reading the *sentence* against the code, and
  three of the four were introduced by work that had no reason to touch this file.
- **One item existed in neither direction: SVG import**, which two documents wrote about as scheduled
  and which was in no section here — not open work, not a §0 non-goal. Filed as an open item rather
  than a non-goal, because nobody had taken the decision. ⚠️ **The maintainer took it the same day and
  it was built the same day** (§15 D394), which is the useful ending: *the thing nothing scheduled was
  the thing being missed while using the app*. An item in neither direction is not a small oversight —
  it is a feature nobody could ask for through this file.

**A list is still the tell, and it is no longer the only one.** The three additions to the palette's
count — `NewDocument`, `CloseDocument`, `OpenSettings` — arrived with the library, which is a feature
answering a *Later* section's premise while nobody was looking at either. **Suspect any entry that
names a number, a count, a variant or a call site, not only one that names a kind**; and re-derive it,
because six of these eight sentences were written by someone who had read the code correctly on the
day.

Left standing after this pass and read on the same date: the winit pinch analysis (re-verified end to
end — one consumer, one producer, iOS and macOS only, `Touch` present on Windows), the flo_curves
guard, the snapshot's twelve-of-thirteen count, the font-warning split, `menu::Context`'s 28, the
format's git properties, and the autosave debounce — to which the crash snapshot now adds a second
interval-gated writer rather than a per-op one.

⚠️ **Two things this pass could not check offline, and one of them was checked the same day and moved
a verdict.** flo_curves **had** released — 0.8.1, fixing the D239 comparator outright — so the boolean
entry's "nothing short of a fork can fix it here" was false and had been for as long as nobody looked.
The other, the aarch64 measurement the PNG goldens wait on, needs hardware and still stands. **The
lesson is a ninth premise shape and it is not in the list above**: every entry here is re-derived
against `crates/`, and *not one step of that reads a dependency's version*. An upstream release is the
one way an item can close with **no change to this repository at all** — nothing to notice, nothing to
grep, and no gate that goes red. `cargo search` on every pinned crate an open entry names is thirty
seconds and belongs in the next run of this pass. ⚠️ **It bit on the first run**: *hyphenation*'s
"there is none in the lock" was true of the lock and false of crates.io — an entry that measured the
lock file and read as if it had measured the world. The price that re-derivation put on it was taken
to the maintainer the same day and declined, so it is a §0 non-goal now and **§15 D399** carries the
price rather than the *Text* section below. ⚠️ **The step needs a second half for a crate in an
*interop set*, which the first run learned the hard way**: a newer version is not a bump until its
peers can move with it, so having found one, read what *those* require. `Cargo.toml` names all three
sets, though not in one place: its `INTEROP PINS` header carries `wgpu` (shared by vello and
egui-wgpu) and `kurbo`/`peniko` (shared by ondin-core and both renderers), and the `ondin-app` block
carries the egui trio plus `eframe`, "pinned exactly, moves together". The egui bullet below
is what the pass produces without that half: a version that is **available and not reachable**,
written down as if it were work.

**The `cargo search` step was run for the first time on 2026-09-03**, over every pinned crate an open
entry here names. It moved one verdict — `vello_cpu`'s bump was taken the same day (§15 D414) — and
left the rest, which is worth writing down so the next run compares rather than re-derives:

- **parley 0.11.1 is out and the justify-all wall is unmoved**, checked in the 0.11.1 *source* rather
  than inferred from the version number: `align` and `LayoutData` are still `pub(crate)`,
  `ClusterData::advance` is still `pub(crate)`, and `align_impl` still hard-codes
  `matches!(line.break_reason, BreakReason::None | BreakReason::Explicit)`. ⚠️ **A version bump is not
  the measurement** — only the source is, since a release can move a number without touching the item.
- **peniko** unchanged at 0.6.1. **roxmltree** 0.20.0 → 0.21.1. **egui** `=0.35.0` → 0.36.1 — and this
  bullet said the bump "re-opens *Menu rows*", which read as available work and is wrong. **The bump is
  blocked upstream** (attempted and abandoned 2026-09-03, no code changed): `egui-wgpu` **0.36.0 and
  0.36.1 both require `wgpu = "30.0"`**, read in both published manifests rather than inferred from
  one, while **vello requires wgpu 29** — not only the pinned 0.9.0 but **0.10.0, the latest release,
  which still declares `wgpu = "29.0.3"`**, with nothing published after it and no wgpu-30 release
  anywhere in its version list. No combination of released `vello` and released `egui` 0.36 shares a
  wgpu, so the egui trio plus `eframe` is held at 0.35 **by vello's requirement, not by any choice made
  here**.
  - ⚠️ **The interop pin was verified against the code rather than trusted from `Cargo.toml`'s own
    header.** It is real: `canvas.rs`'s `ensure_target` creates the canvas `wgpu::Texture` that vello
    renders into, takes a `wgpu::TextureView` of it, and hands that view plus `render_state.device` to
    **`egui-wgpu`'s `Renderer::register_native_texture`**. The `TextureView` *is* the handoff, so two
    wgpu majors are two incompatible `TextureView` types and the thing does not compile — a hard
    constraint rather than a cautious one.
  - **What unblocks it is a vello release against wgpu 30, and nothing in this repository.** That is
    the ninth premise shape above, and it is the flo_curves case with the sign reversed: there the
    release had already happened and nobody had looked, so an entry sat wrongly closed for twelve days;
    here it has not happened, so the item is genuinely waiting and the only useful thing to record is
    the date it was last asked — **2026-09-03**. Re-ask by checking vello's newest release for a
    wgpu-30 requirement; until one exists there is nothing to attempt.
  - *Menu rows* below is therefore **not** re-opened. It is written against egui 0.35's
    `Style::button_style`, so it is the first thing to re-derive when vello moves and the trio goes to
    0.36 — **unmeasured**: nobody here has read 0.36's `button_style`, and this claims nothing about
    what changed in it. The duplicate `vello_cpu`/`vello_common` in the tree resolves at the same
    moment for the same upstream reason, `epaint` 0.35 being what pulls the second copy (§15 D414), so
    that duplicate compilation is a cost currently being paid for this pin.
- **`vello_cpu` `=0.0.9` → `=0.2.0`, taken the same day** and the baseline for the next run of this
  pass. The warning it raised is measured rather than carried now: the bump moved one stroke and
  nothing else, and `RenderMode` did not change its default but changed *struct* (**§15 D414**).
- **flo_curves** read 0.8.1 in the lock on **2026-09-03** while `crates/ondin-core/Cargo.toml` read
  `"0.8.0"` — a caret requirement, so harmless as far as the D239 fix arriving goes, and named here
  so nobody re-reads the manifest and concludes that fix is absent. ⚠️ **"Harmless" was true of the
  fix arriving and said nothing about the review gap** (§15 **D703**, 2026-09-09): a caret means the
  crate behind this project's most carefully argued safety guard bumps patch-to-patch with no
  manifest change and nothing to look at, while `vello`, `vello_cpu`, `wgpu` and the four egui crates
  are all `=`-pinned. 🚨 **Ruled on and pinned**: the manifest reads `flo_curves = "=0.8.1"` since
  `17f4eb9` (§15 **D782**, which also records D703 having gone on saying *"deliberately not pinned"*
  for a day after it landed). **This row stays as the 2026-09-03 baseline for the next run of the
  pass to compare against**, and is a dated measurement rather than open work.

---

## 0. Decided non-goals for v1

**Do not re-report these known gaps:** auto layout, components, styles/variables, MCP, export,
blend modes, ~~effects (shadow/blur)~~ **background blur, noise and reordering an effect stack**,
rich text runs, constraints,
prototyping, plugins,
multiplayer, Command Mode, eyedropper/comment tools, modifier badges on the cursor, **vertical
writing mode**, **a unit system**, **hyphenation**, **a multiple selection in the library**.

The last of those was decided 2026-08-28 (§15 D383) — *"we don't have multi-selection and I don't
think we will"* — and it is here rather than in `shortcuts.md`'s gap list because that is the file's
own rule: a decided non-goal is never an open item. What it settles is `Ctrl+A` on the dashboard and
every verb behind it; the four the library has (`Enter`, `Delete`, `F2`, `Ctrl+D`) each take one
document by design, and `DashboardState::selected` stays one path.

**Hyphenation** was decided 2026-09-01 (§15 D399) — *"agreed, file it and don't build it"* — and it is
on this list **on price, not on capability**: parley already breaks at `U+00AD` and the glyph is
inkless, so what was declined is the offset map §15 D80 specifies, a second layout pass and a
dictionary crate, not any wall. ⚠️ **Nothing here is blocked**, and the *Text* section's struck bullet
below keeps the pointer so the "blocked upstream" reading — which was wrong and was corrected the same
day — cannot come back.

MCP and Command Mode are non-goals for v1 only and have designs of their own — `architecture.md` §8
and §13, and `vm.md` for the language behind the second. The rest are unstarted.

### How this list got shorter

*Path editing* closed 2026-08-03 — its rules, decisions and traps all had records in §15 or in the
code, so the section is now one deferred decision. *Guides and rulers* closed the same day and left
nothing behind, its design now in `architecture.md` §5.5–§5.7, §5.11, §9.2–§9.4 and §15 D136–D142,
with *Units* the piece already split out of it — and that piece is now a non-goal in its own right
(§15 D358), so the split outlived what it was split from by three weeks. *Improve typography feature
tags* closed on the
fourth, its whole record being §15 D157, and the one bullet it left under *Text* — the n-way `cvXX`
control — closed 2026-08-16 into that same entry.

*Modifier badges on the cursor* — Figma's small glyph at the bottom-left of the arrow — became a
non-goal on 2026-08-19 and left *Canvas and interaction* on the same day, its whole record being **§15
D233**. It had been carried as cheap work because `cursor::compose` takes an arbitrary coverage
closure, and the compositing genuinely is one closure; what the entry missed is that all seven of the
modifiers it listed sit on a *stock* cursor, so a badge has no base to composite onto and shipping one
means drawing our own arrow first. There is no reason to override the system's cursor for that. The
Image tool's remaining-count badge was never covered by this — it is the one badge with a drawn base
under it — and it landed the same day (§15 D236).

*Nudging the transform origin from the keyboard* joined the list on 2026-08-23, out of *Transform origin
(pivot)* — a section retired whole the same day, everything left in it being a restatement of §15 D55,
D60, D240, D247 and D252–D254. Its numeric-pair bullet had carried this as the half still open. There
is indeed no chord that nudges an origin and `shortcuts.md` reserves nothing for one — and that is not
a gap, because the on-canvas pills commit **typed** values as well as scrubs, and the pair is a
percentage of the box, so `0` / `50` / `100` type an edge or the centre with no arithmetic. **§15 D308**
is the record.

*Aligning a multi-selection to its container* joined the list the same day, out of *Align and
distribute* — retired whole with it, the rest of that section being D113 and past-tense D243/D245 —
which had carried this as the third of three candidate targets waiting on a control `design/` does not
draw. The control was never the blocker: `align_target`'s `1 =>` arm already aligns one layer to its
parent, so the feature is *select them one at a time*, and for a set whose members
have different parents "the container" names either the root or *n* different boxes. **§15 D309** is
the record, and it corrects the reason both that bullet and D113 gave for the delay.

*Now · Scale tool* was retired whole the same day, the third section to go on 2026-08-23 — and unlike
the other two it added nothing to the list above, because it held no open work at all: all three of its
paragraphs were past-tense records of **§15 D242, D297 and D311**, each kept only because it had
corrected the one before it. Nothing of record went with it. The shape it taught — *an entry whose
premise is a list of kinds goes stale silently* — is the rule at the top of this file and sits in D242's
own body, and the observation that the list had rotted again one kind over opens D297. The row's own
review on the machine followed it out the same day: **§15 D312** has the typing bug, the multiples
beside the field and the tool gate, and **D313** the pair those multiples became.

*Vertical writing mode* joined the list on 2026-08-21, and joined it as a **correction**: it had been a
decision in force since the OpenType work and lived nowhere but a code comment, which is the state this
list exists to prevent. `panels/typography.rs` withholds ten tags on the grounds that "this app has no
vertical writing mode", so the decision was already deciding things — and a future editor completing
that registry would have unwound it without ever meeting an argument. **§15 D279** is the record.

*Effects* left the list on 2026-08-24 and **two narrower items took their place**. The maintainer
reopened them — *"Something we marked for after v1, but the target has changed"* — with `design/`
regenerated to draw an Effects panel of six types, and **four were built**: drop shadow, inner shadow,
layer blur and colour filters, the four that can be made to agree across the canvas, the raster
export and the SVG writer (§15 D333, `architecture.md` §5.3a and §6.4). **Background blur** and
**noise** are the two that cannot, and they are non-goals with reasons rather than work not yet done —
background blur means nothing unless the export contains the backdrop and SVG cannot express it at
all; noise would need one generator written three times and agreeing to the pixel, and Figma has
already moved it into the *fill* list, where our `Brush` machinery would make it exact for free.
**Per-effect blend modes** are not a separate entry: they ride the blend-mode non-goal already on
the list above.

**Nothing of the feature is open any more.** The panel, its multi-selection form and the GPU canvas's
passes all landed on 2026-08-24/25 (§15 D337–D342), and the last item — **reordering the stack** —
joined the non-goal list above on the maintainer's call: *"strike the reorder entirely. If lack of
reorder bites later, we may revisit. I doubt."* It is a non-goal with a **measurement** behind it
rather than a deferral, which is what makes it safe to strike: §15 D340 rendered every pair of effects
both ways round and counted the channels that differ, and the order changes a pixel in exactly two
places — `Filters` against `LayerBlur`, and two shadows on the same side. A blur and a shadow cannot
reorder at all, because §6.4 builds the appearance in one stage and casts every shadow from it in the
next.

*The inspector editing a locked layer* joined the list on 2026-08-23, out of *Now · Inspector*, where it
stood as "a locked layer can still be moved and resized by typing, and nobody has decided whether it
should" — the Transform card's X / Y / W / H, rotation and skew all commit straight through. What was
decided is a rule rather than a ruling on those six fields: **a lock guards against accidents on the
canvas, not against deliberate edits through a panel you had to select the layer to reach.** The panel is
only on screen because someone picked that layer, which no stray drag does, so the paint rows being live
(D71) was this rule all along rather than an exception to it. **§15 D323** is the record, and it names
D321's marquee fix as the thing that makes the premise true — before it a rubber band could sweep up a
layer inside a locked group with nobody clicking it.

*A unit system* joined the list on 2026-08-26 and took the whole of *Next · Units* — eighty lines,
whose consumer count had rotted three times over (five, six, nine, then ten readouts over eight paths)
— out with it, on the question *"does Ondin need units? This is a UI app, so what would units really
solve here?"* Nothing it has a use for: physical units need a resolution nothing downstream
reads, the platform units a UI tool would want (iOS pt, Android dp, CSS px) are **the same number at
1x**, and the one conversion a screen tool does need is @2x, which belongs to the export. **§15 D358**
is the record. This is a decided answer and not a deferral, which is what makes it safe to strike a
section that had been re-scored three times. Two things came out of it and did *not* close: the
`expr::eval` bug it had described in passing — `12px` typed into a field showing `%` read as twelve
percent — which is fixed as **§15 D359** — and `ondin export`'s missing `--scale`, **§15 D360**, built
the same day and finished by **D361** (`--width`, `--height`, and `--scale-override` for `--all`). The
second was mis-stated when it was raised, and the correction is the part worth keeping: the density
multiplier was already built (`ExportScale`, Figma's ladder, the Export panel's per-row combo,
`Close@2x.png`) and `--all` had always honoured it. Only the ad-hoc `--png` path was pinned, at a
`viewport_over` call. **A gap named from outside a subsystem gets scored as a missing feature when it
is a missing flag.**

*Hyphenation* joined the list on 2026-09-01, out of *Now · Text*, and it joined it **hours after being
re-derived out of the opposite state**: for as long as it had been on this file it read as blocked —
*"still wants a dictionary crate, and there is none in the lock"* — and the lock was never the world.
The re-derivation priced it instead: parley breaks at `U+00AD` unasked, the glyph is inkless, and the
overflow that leaves is fixable with parley's *public* incremental API, three of whose four calls
`text::break_lines` already makes for other reasons. What is actually expensive is the offset map §15
D80 declined for the case transform, plus a second layout pass and the dictionary. The maintainer's
call on that price was *"agreed, file it and don't build it"*, and **§15 D399** is the record, carrying
every measurement. ⚠️ **This is the one entry on this list whose danger is being read as a blocker
rather than being re-argued**: it is a non-goal on *cost*, and the reason to say so twice is that the
sentence it replaces said the opposite and no re-derivation against `crates/` would ever have
contradicted it.

---

## Now · Images

**Closed — the whole v1 image build list.** The model, the decode cache, placing, framing,
image editing (`Tool::ImageEdit`), drag-and-drop and paste, the seven adjustments, export in all
three writers, thumbnails, the loaded cursor's count badge and the Windows drop position. **`images.md`
was the scope document and is gone**, so **§5.5a and §15 D176–D191 are the whole record**, plus D225,
D236, D241, D268, D280 and D305 for what came after it. D191 is the out-of-v1 list nothing else
carried. **Linked images are the one thing that does not work**, which is the whole of what is open:

- **The Asset section is two entries short: *Embed*/*Link* and *Relink…***, and both wait on the same
  thing — a **linked** source, which nothing in the app can author and nothing can therefore create
  work for. One dependency holding both, rather than a cheap-versus-expensive split. A third entry,
  **Embed all** for a document that arrives with dead links, is document-wide rather than per-fill, so
  it wants a home outside that row — and it is the one no §15 entry names. (The other two of the five
  are done: *Replace…* in §15 D179, *Export original…* withdrawn as a decided non-goal in D305.
  Framing keeps the room D190 held in reserve.)
  - **The dependency holds, re-checked 2026-08-31** — `ImageSource::Linked` is constructed in exactly one
    place, `io/schema.rs`'s deserializer, and every app path onto a picture funnels through
    `app::load_image_bytes`, which builds `ImageSource::Embedded` unconditionally. What that sharpens
    is *which* half is missing: a hand-written or foreign `.ondin` **can** carry a link and the app
    will open it, so the subject exists and only the *authoring* does not. The two rows are blocked on
    a gesture, not on the model.
    - **"Exactly one place" is about the *constructor*, and there is a second path a link travels
      by** — noticed 2026-08-31 and not a change of verdict. `build::missing_image_ops` copies an
      entry wholesale out of the document a paste came from, so a `Linked` entry can enter a document
      the app never deserialized. Still nothing *authors* one, which is the blocker; but a reader
      taking "one place" as "one way a link can be in front of you" would be wrong, and the two rows
      would have to cope with a link that arrived this way as much as with one that was opened.
  - ⚠️ **The one piece of linking that was handled has been un-handled**, and the next reader should
    know where the spec went. `ImageEntry::bytes` returning `None` had a consumer: *Export original…*
    dimmed for a linked picture and said why, in three states rather than two — fine, linked, or an
    id the table does not hold — through `tools::original_refusal` (**§15 D280**, which is also where
    the cross-document paste defect found on 2026-08-21 was closed). That row went from both doors on
    2026-08-23 and took the predicate with it, which put `ImageEntry::is_linked` back to **zero
    callers** for the second time (§15 D305). Nothing regressed, because nothing can author a link
    yet; what is lost is the worked answer, and it is preserved as comments where the function stood
    and on `is_linked` itself. **Rebuild from those rather than re-deriving**, and do not infer
    "linked" from absent bytes — that inference is the bug D280 is about.

---

## Now · Canvas and interaction

- ~~**A frame can still exceed the GPU's watchdog at extreme zoom, and it is no longer the blur.**~~
  **Fixed 2026-09-01 — §15 D402**, and what it was is not what this entry said. The premise came
  straight from D395's closing note — *"the cost of rasterizing 417 paths at sixteen times
  magnification, in the main scene and in the effect batch"* — and a sweep with the effects stripped
  renders that drawing at **2.0 ms** at 4K and 32×, and **20,000** paths in 30 ms. The main scene was
  never in it. Both hangs were the effect pass asking vello for a scene bigger than the page: a level
  of siblings packed side by side, and a shadow's buffer grown by an escape measured in *device*
  pixels. **`pack` and `effect::buffer_box_within` are both bounded by the page's own pixel count
  now.** ⚠️ **The 2,118 ms in this entry was the watchdog rather than the work** — a killed frame
  reports ≈2,1xx ms however long it would have taken, so the only honest reading at that end is
  whether the device is alive afterwards.
- ~~**A drop shadow's kernel is still linear in the zoom, and that is what is left of this.**~~
  **Built the same day — §15 D403**, hours after being written down. The silhouette a shadow is
  blurred from is not the artwork D395 was protecting, so it can be resampled: `ceil(σ/24)`-pixel
  blocks, blurred at σ/k, read back bilinearly. **1,557 ms → 20.5 ms at 256×**, the curve flat rather
  than doubling per zoom step, and a whole frame byte-identical to the unresampled one. ⚠️ **The CPU
  backend was the worse half and nobody would have noticed** — 19.6 s → 0.1 s on the same frame, an
  `ondin export --scale` hazard with no watchdog to make it fail loudly. *This entry existed for
  about four hours, which is the useful ending: it was written as "a feature rather than a bound" and
  the feature was five hundred lines including its tests.*
- **Context menus: one row and one clause open, both decisions rather than work — plus one clause of
  one test** — the four tests were written on 2026-09-19 (§15 D795) and are struck below, all but
  the Escape bullet's wider fixture. ⚠️ *This headline read "one row and one clause open, and both
  are decisions rather than work" until 2026-09-09, when it was corrected to "one row, one clause and
  four tests open": it had been true of everything the bullet knew about and false of the section,
  the four tests having been queued in `context-menus.md` §10 and **§15 D214** the whole time — this
  sentence and the struck sub-bullet below both said **D226**, which is the panel's *Paste* slot and
  carries no test queue at all. It is back
  near the shorter form because the tests landed, not because the reading changed.* Chrome,
  keyboard navigation
  and every other row are built, and
  `context-menus.md` plus §15 D214–D218 and D223–D226 are the whole record. **That file is still the
  one to read before adding a row.** ⚠️ *This entry pointed at its §9.5 ledger as a queue until
  2026-08-31 and the ledger had been empty for days* — two of its sections emptied without a row being
  built at all (units decided against, §15 D358; the two *Export* rows built and then **withdrawn**,
  D264 and D305). **A pointer does not go stale the way a copy does — it goes stale by outliving its
  subject.** ⚠️ *And it said **nothing open** until 2026-09-07, which was the same failure once more:
  the row below had been unbuilt since the spec was written and the ledger that would have said so
  claimed it had landed.*
  - ~~**A frame's *Background…*** (`context-menus.md` §5.1). Specified, never built, and the
    operation it was scored as wiring for — `SetArtboardBackground` — was deleted by §15 D400 when a
    frame's fill became its fill list.~~ **Closed 2026-09-19 — §15 D800**: a decided non-goal, on
    D400's own reason, the inspector's Fill row already doing this job for a frame exactly as for
    anything else. `context-menus.md` §5.1's row is struck in place with the argument, and §9.1 and
    §9.5 follow it.
  - **C5's *"or on its edge"* half is answered by one door out of three** (`context-menus.md` §2, §15
    D469). The select-click arm and the context-menu door now share one chain, `canvas::pick_at_pointer`
    — the frame's name tag, then `pick_leaf` — and `begin_select_drag` carries a further fallback,
    `selected_frame_at`, which is the edge clause and which neither of the other two has. Widening the
    shared chain to match wants a decision rather than arriving as a side effect of sharing a helper:
    it would make an already-selected frame's *interior* a right-click target, which is the half of C5
    that has never been true. **The decision, not the work** — the code is one `.or_else` either way.
  - ~~🚨 **Four of the rules have no test, and this bullet is where that belongs**
    (`context-menus.md` §10, §15 D226, `[A7-L8-06]`).~~ **Closed 2026-09-19 — §15 D795**:
    `app::context_menu_rule_tests` is the four, every flip run, and they are the first test anywhere
    in the workspace to drive a synthetic **secondary** button through the app —
    `canvas_context_menu`'s door had never had one, every earlier context-menu test calling
    `open_context_menu` by hand or building a `menu::Context` directly. ⚠️ **One clause is still
    open and is kept in `context-menus.md` §10 rather than restated here**: that file's Escape
    bullet asks for `entered_group` and the tool as well as the selection, and the test written
    asserts the menu and the selection.
    🚨 **The lesson stays because it is about this file rather than about the work**: these four were
    queued in two other documents and in neither of the places a reader looks for open work, which is
    the corollary of *the roadmap holds open work only* that nobody had written down — **it must hold
    all of it**. Open work recorded nowhere a reader looks is the same defect as a closed item left
    here, and it is harder to see, because no document is individually wrong. **This bullet's own
    history is the proof**: the 2026-08-31 pass read the §9.5 *row* ledger, found it empty, and
    rewrote the bullet to *"nothing open"* — while §10 of that same file, five hundred lines further
    down, held these four. **The correction moved it from one wrong state to another.**
- ~~🚨 **A ruler's origin on a rotated frame — writable now and unwritten** (§15 D36,
  `[A7-L8-06]`).~~ **Closed 2026-09-19 — §15 D797**: `rulers::origin_tests` pins the upright origin
  and the rotated rough edge. 🚨 **The rough edge is worse than D36's illustration** — that entry
  said the band *"can read −50 to 480"*, and a 480×320 frame at 30° measures **−160 to 415.7**, a
  band 575.7 wide for a frame of 480, the far end short of the width rather than past it. The edge
  itself is D36's decision and stays; what was open was the test, and the figures are a measurement
  now. ⚠️ **The only record of it was one clause inside a live *Keep* entry**, where nothing looking
  for open work would ever meet it — the same shape as the four above and a different subject, which
  is what says it was a habit rather than an accident.
- **Vector paths are editable and the brief is closed** — pen, node tool, corner radius, segments,
  snapping, joining, the pen reachable from inside an edit, reverse-subpath (§15 D114, D116–D125).
  **Read D114 and D116–D125 before touching any of it**; between them they carry every rule and
  decision this file used to restate. D116 belongs in the read list and said D117 until 2026-08-03 —
  it is the press-swallow trap, which the *Path editing* section names as one of the four that recur.
  One deferred decision is left, under that section.
- **Booleans: nothing open as of 2026-09-19**, and the sub-bullets below are the record of how each
  item closed rather than a queue — §15 D87–D91, D230, D239 (six amendments), D244, D282–D286,
  D298, D299, D453, D457, D794. **Three flo_curves traps are recorded in `boolean.rs`'s module docs
  and in D91 and D794 — read them before touching that file**: it works even-odd where we fill
  non-zero, `Exclude` cannot be composed as `sub(add, intersect)`, and it compiles in **absolute**
  tolerances no argument reaches, which is why every coordinate is scaled by `FLO_SCALE` through `c`
  and `k`. ⚠️ **One consequence to know rather than to fix**: since
  `Exclude` became a concatenation it never enters flo_curves, so D298's abandoned-boolean machinery —
  `failures`, the amber layers row, the placeholder — is unreachable for the one operation that most
  needed it. Every sub-bullet below is how something closed.
  - ~~**`Intersect` loses half a thin result's area, and it is a shape with no concavity and no
    hole.**~~ **Closed 2026-09-19 (§15 D794)**: flo_curves compiles in absolute tolerances no argument
    reaches — `CLOSE_DISTANCE = 0.01` merges two points into one — so the result's two short ends,
    each exactly its thickness, were erased and the rectangle came back a bow-tie. Geometry is scaled
    by `FLO_SCALE` on the way in and out. ⚠️ **This entry's own guess was half right in the way that
    stalls a reader**: `ACCURACY` is load-bearing and only in combination, and the constant doing the
    erasing is not one this crate can pass. `a_thin_intersect_keeps_its_area` asserts it now.
  - ~~**Booleans are untested at the pixel level.**~~ **Closed 2026-08-19**: two tests in
    `ondin-export/tests/boolean.rs` render a hole and assert it is `alpha == 0`, which is the cheap
    version this entry asked for. Worth knowing what flipping them found — **they are not one test
    twice**. A subtracted hole is a *reversed nested subpath*, so it fails when the winding correction
    goes (§15 D91's second half); an excluded overlap is an *absence of any subpath*, so it survives
    that and fails when the arm is replaced by `path_add` (D91's first half). Two faults under one
    report, and each needed its own assertion.
  - ~~**Performance is unmeasured.**~~ **Closed 2026-08-19 (§15 D239)**: measured, and Union, Subtract
    and Intersect are a non-problem — under 4 ms for one whole `evaluate` at sixty-four overlapping
    operands, far past what anyone selects. **The caching this entry named as "the first lever" — per
    operand rather than per container — is a decided non-goal now, not a deferred optimisation**: there
    is nothing measurable to win, and the entry's own instruction was to measure first. A boolean of
    booleans costs nothing extra either. `Exclude` is the outlier, and what is left of it is below.
  - ~~**On some operand sets a boolean draws nothing, and `Exclude` is much the likeliest door.**~~
    **Closed 2026-08-31 — fixed upstream, by flo_curves 0.8.1** (§15 D239, amended). flo_curves 0.8.0
    panicked out of `GraphPath::exterior_paths` on a comparator that is not transitive; since
    2026-08-19 `evaluate` caught the unwind and answered `None`, so what was left was a wrong result
    rather than a crash. 0.8.1 replaces that comparator with a `total_cmp` sort plus a pass that
    re-sorts close-x runs by y — the epsilon segments the array instead of living inside the
    comparison — and every one of the eight counts D239 measured now returns a real path. The bump was
    `cargo update -p flo_curves`: the requirement was already caret, so only the lock had pinned it.
    ⚠️ **This entry said "nothing short of a fork can fix it here", and the answer was neither a fork
    nor us** — the upstream simply released, and the sentence had been carried for twelve days by a
    file that never re-checks a dependency's version. *This is what the pass above could not check
    offline, and it is the one of the two unanswerable questions that turned out to change a verdict.*
    **The guard stays** and its meaning changes with it: a caught unwind is no longer the known defect
    but an unknown one, because D239's own finding — the variable is the geometry, not the count —
    means "nothing panics" was never establishable by sweep. **"Silently" was struck earlier
    (2026-08-22, §15 D298)**: `boolean::failures()` separates an abandoned
    boolean from a correctly empty one, the layers row wears §5.5a's warning colour for it, and the
    commit that trips it says so — and since §15 D299 the canvas draws §5.5a's placeholder over the
    operands' box, which also gave the node the world bounds it needed to be clickable at all. All of
    that is kept.
    - **What it cost is the reproduction, and one thing is genuinely open.** Six tests reached the
      abandoned state through a forty-circle ring and none of them can now, so the panic is injected
      (`boolean::poison_next`) and they make a **weaker claim than they used to** — that the guard and
      its consequences work, not that flo_curves can still produce a panic. The alternative, hunting a
      new panicking set, was declined as proving a negative after NaN, infinite and 1e300 operands
      found nothing.
    - **The cost table was re-measured on 0.8.1 the same day and the upgrade costs nothing** — every
      0.8.0 cell reproduces within noise. `Exclude` at 40, the cell that used to panic, is **197.58 ms**
      and sits exactly on its neighbours' curve, so that row held no surprise. ⚠️ **What the
      measurement did find is one row lower**: `Exclude` left a 60 Hz frame at about **nineteen
      operands** (18 → 13.34 ms, 20 → 18.81), and the preview path re-evaluates a boolean on every
      frame of a drag — so a twenty-circle `Exclude` dropped frames while an operand was being
      moved. *(The spatial cull two bullets down moved that threshold to about twenty-nine; these are
      the numbers as found, kept because the entries below are written against them.)*
    - ~~**That is the case a per-operand cache would be for.**~~ **Built 2026-08-31 (§15 D239, amended
      twice), and the finding is worth more than the cache.** The *obvious* per-operand cache — fold
      the static operands once and combine the moving one last — is **unsound**: `Exclude`'s
      associativity is a property of the mathematics and not of this implementation, and reordering a
      twenty-circle fold moves the result by **3% of its area**. What shipped is the
      order-preserving version, which reuses the fold's leading steps and is bit-identical. ⚠️ **It
      fixes one case and not the problem**: dragging the *last* operand of twenty goes 19.49 ms →
      3.54, under the frame budget; dragging the middle saves 12% and dragging the first saves
      nothing. **No sound cache fixes the rest**, because the reordering that would is the unsafe
      thing. So the frame-budget entry above stays open in every case but one — and it is now open
      with a reason rather than a hope.
      - ~~The frame-budget entry.~~ **Closed the same day, and not by anything above it.** The fill
        rule deleted the fold, so `Exclude` is a concatenation: **0.00 ms at every count** to
        sixty-four, where it had been 18.81 at twenty and 1087 at sixty-four. *The entry was open
        "with a reason" for a few hours and the reason was answered by a change that was not about
        cost at all* — which is this file's own warning about premises, arriving from the pleasant
        direction. **The cache and the cull were then removed** (§15 D239's sixth amendment); both
        existed only for the fold that no longer runs.
    - **It also exposed a live defect**, since fixed: `RenderOverrides::operand_children` appended
      ghost operands on the strength of that same false commuting claim, so an Alt-drag into a large
      `Exclude` previewed a shape 3% off the commit. The test its own doc said did not exist now does.
  - ~~⚠️ **`Exclude` returns a wrong shape for some operand geometries, silently.**~~ **Closed
    2026-08-31 by the fill rule** (§15 D239, amended a fourth time): `Exclude` does no arithmetic at
    all now — its outline is its operands concatenated and the node reads even-odd — and it agrees
    with the odd-coverage referee at every sampled point at every count. The section below is kept as
    the record of the defect and of how it was found.
  - ⚠️ **`Exclude` returned a wrong shape for some operand geometries, silently — up to 35% wrong.**
    Found 2026-08-31 while going after its cost (§15 D239, amended a third time). **Nothing had ever
    checked `Exclude` against anything but itself**; the symmetric difference is exactly odd coverage,
    so a grid sampler referees it with no path arithmetic, and it finds the arithmetic accurate at 10,
    11 and 15–18 operands and wrong at 12–14, 19 and 20 — worst at fourteen, 94,564 against a true
    69,810. **The pattern is D239's own signature**: not monotonic, fine either side of a bad run, so
    the variable is the geometry rather than the count — presumably the same fragility in
    `path_full_intersect` that used to panic, except that **this failure is silent**. Nothing D298
    built to report an abandoned boolean can see it, because nothing is abandoned. It was pinned as a
    characterisation, and **the characterisation test got to fail**: it is
    `boolean::tests::exclude_agrees_with_odd_coverage_at_every_sample_point` now, demanding the
    opposite of what it was written to hold.
    - ~~**Open, and the likely fix is a feature rather than a repair.**~~ **Right, and built the same
      day.** XOR *is* even-odd fill — the operands concatenated and read even-odd is exact and costs
      nothing — and what prevented it was that the model had no per-node fill rule. It has one.

### ~~The fill rule, costed and then built, 2026-08-31~~

**Built, and then the machinery it retired was removed** — §15 D239's fourth, fifth and sixth
amendments carry all of it: `Node::fill_rule` derived for an `Exclude` and stored for anything else,
`Operation::SetFillRule`, an additive `NodeDto` field, the rule through both backends, the hit test
and the SVG writer, an *Even-odd fill* row on the layer menu; then the spatial cull and the prefix
cache deleted, since the fold they made cheaper no longer runs. `Exclude` is exact and free.

**Three things about the costing are worth carrying, and only the third is a warning.** The
propagation landed near `mask_mode`'s scale as predicted and the hit test was the sharp site as
predicted. The stroke leak it priced in **did not arise** — *a worry priced into a design can turn
out to be a property of the design it was priced against.* And it claimed B "fixes `flatten`",
which was true as a capability and implemented as nothing: flattening an `Exclude` drew its union
for a day, with all six gates green. ⚠️ ***A costing's estimates are unfalsifiable until the work is
done; its predictions are checkable the moment it is*** — and nobody re-read that one against the
function it named.

  **Flatten's refusal of a lone non-boolean is now a decision rather than a gap, and it stands**
  (§15 D230, decided 2026-08-19). The conversion is real and is **`build::outline`** — a second verb,
  reached from *Outline shape* — so `flatten` keeps meaning "one boolean, or two or more of anything".
  A `Path` gets the row and bakes its per-anchor radii; a `Group` does not, its outline being the union
  of its contents, which is what `flatten` already gives a set. Gone from this list rather than struck.
  - ~~**Mask is drawn but does nothing.**~~ **Closed 2026-08-21 (§15 D282–D285)**: the user asked for
    masks, so D191's out-of-v1 verdict is reversed and the feature is built — `Node::mask`, the run
    rule, the scene walk's own layer, the SVG `<clipPath>`, the save flag, and the identity row's
    action as a **toggle**. The re-derivation this entry carried was right that the parts had arrived
    for other reasons, and its closing sentence named the work exactly: the
    **two decisions nobody had taken** are taken — bounds in D283 (the node keeps its own box, its
    container gets only what the mask lets through, and a marquee can still find clipped-away ink), and
    hit-testing in D284 (a mask is never hit; what it masks is hit only inside it). The user's four
    review asks — `Ctrl+Alt+M`, a context-menu row, wrapping two or more layers in a group, and
    `Alt+Shift` to designate which layer is the mask — landed the same day (§15 D286). Nothing is left.
  - ~~**Nothing in the panel names the base operand.**~~ **Closed 2026-08-19 (§15 D244)**: the bottom
    row of a `Subtract` wears a faint `stack-simple` glyph in the right-hand slot the size badge uses,
    derived from the tree every frame rather than stored — so dragging the operand up one row stops it
    being the base, which is the property D113 refused a stored flag to keep. This entry's own scoping
    was right on both counts and is worth noting as such: nothing is stored, and only `Subtract` is
    marked.
- ~~No frame size presets, no layout grids (columns/rows overlay).~~ **Both built 2026-08-29**
  (§15 D385, §5.3b): `Node::grids` carries a frame's columns and rows as **chrome** — drawn over the
  frame by the canvas, absent from the scene, every export writer and the snapshot — with a *Layout
  grid* card to author them and a *Frame templates* panel of pictures, which the presets became the
  same day after starting as a dropdown under W and H (§15 D387). ⚠️ **This was the file's only
  one-line entry, and the line was the whole specification**; what it did not say, and what the work
  turned out to be about, is that a grid is measured against an **authored** size, which is what
  confines it to a frame while the field itself sits on the node beside `exports`. *An entry short
  enough to read as small can still be the largest thing in its section.*
- **The *layer* clipboard is app-internal; text and images are not.** Copy puts layer *names* on the
  system clipboard — and those names are now also the **receipt** that says whether the in-app payload
  is still what the OS clipboard describes, without which a paste of text from another application
  could never be reached (§15 D218). A layer goes **out** as SVG since 2026-08-20 — *Copy as SVG* on
  every layer menu (§15 D259) — and **comes back in as layers**, `Ctrl+V` over markup, since 2026-08-31
  (§15 D394; the paragraph at the end of this entry has the closure). Markup that looks like SVG and
  will not parse still falls through to a **text layer**, deliberately. ⚠️ *This opened with "can't
  paste SVG in … that markup comes back as a text layer" until 2026-09-07 — for a week after its own
  closing paragraph said the opposite. **An entry can contradict itself end to end and read fine from
  either end**, which is what a correction appended rather than applied costs.*
  An image pastes as a layer or into the selected shape's fill (§15 D183); **text crosses both ways inside a live text session**
  (§15 D217) **and pastes as a new text layer on the canvas** — the centre of the frame the selection
  is in, else the middle of the view, and **no frame is required**: a paste onto empty canvas lands on
  the canvas, where it used to refuse (§15 D218, D221). `Ctrl+V` targets the original parent at a fixed 20×20
  offset, and the context menu's *Paste here* is the pointer-aimed one (`paste_at`, §15 D215).
  **What is left to build, and what it would cost.** Copy captures subtrees into `OndinApp::clipboard`
  and puts only a text stand-in of their names on the OS clipboard (§15 D17), so **no layer crosses
  between two `ondin` windows** or into another tool as *layers* — the case the SVG round trip above
  does not cover, since what comes back is an import of markup and not the subtrees that left,
  identity and all. Serializing a
  `Vec<Node>` through the existing schema is most of the work — `InsertSubtree` already carries
  exactly that payload, so the operation the paste would emit needs nothing new; what is missing is a
  serialization of it that is not `NodeDto`, which is `pub(crate)` to `ondin-core`, and the id minting
  on the way back in. *Moved here on 2026-09-07 from an unheaded, unnumbered bullet list
  at the end of `docs/decisions.md`, where it had been sitting with an imperative "Build it." on it —
  open work in the one file that holds none.*
  Paste-in-place (§15 D248), the pasted layer's *slot*, and the decision that `Ctrl+V` keeps its
  fixed offset while paste-at-pointer stays a non-goal (§15 D300) are all closed. **And "can't paste
  SVG in" is closed too, as of 2026-08-31** (§15 D394): markup on the clipboard becomes layers, so
  the *Copy as SVG* round trip closes for everything but text. See the section below for what is left
  of it.
- **Trackpad pinch arrives as Ctrl+wheel (`wheel_input`), and on Windows that is the only way it can
  arrive.** This read as one gap and is two things, checked against the vendored sources on 2026-08-19
  and neither of them built. **`Event::Zoom` would be dead code here**: egui only ever *consumes* that
  event (`input_state/mod.rs` is its one reader), its sole producer is `egui-winit`'s
  `WindowEvent::PinchGesture` arm, and winit 0.30.13 raises `PinchGesture` from its **iOS and macOS**
  backends alone — `platform_impl/windows` has no path to it at all. So it is a macOS portability item
  rather than a defect, and it cannot be tested on the machine it would be written on. **Multi-touch is
  the half that is reachable**: winit does deliver `WindowEvent::Touch` on Windows, so egui's
  `multi_touch()` would see a touchscreen pinch — which needs a touchscreen to develop against. Worth
  knowing before anyone reads this line as cheap work.
- **egui's own keyboard zoom is off** (`theme::install`, `zoom_with_keyboard`). `Ctrl`+`+`/`−` was
  scaling `pixels_per_point` *and* the canvas at once. Anything else the app wants to bind that egui
  also reads needs the same treatment — check `Options` before adding a chord.
- ~~**The click-to-select *policy* is still untestable, and that is the open half of `[A5-L6-06]`.**~~
  **Done 2026-09-19 exactly as this bullet prescribed it** (§15 **D805**): `pick_from_chain(ctrl, alt,
  chain, leaf)` is a free function, `pick_preview` is the input read plus a call, and all three
  mutations the review measured as green are red. `[A5-L6-06]` is closed whole, D503 having closed the
  `group_chain` half. ⚠️ **The lift was not free** — the chain is now built on the `Ctrl`-only path
  too, where the early return used to skip it — and ⚠️ **both flips' predicted sites were wrong**,
  which is what the entry is worth reading for.
- ~~**A rounded `Path`'s outline is rebuilt on every frame, and that is the open half of
  `[S4.2-L4-04]`** — the fix being a new map on `Resolved` holding the rounded outline, filled beside
  `boolean` so the walk is handed it.~~ **Closed 2026-09-15 by the maintainer's ruling** (§15
  **D778**): the map is **declined**, together with D741's local-box map, and the per-frame rebuild
  is an accepted cost rather than queued work. A seventh derived map is one `update` must keep equal
  to `rebuild` against a differential that compares four of the six there are, and D778 carries the
  trigger that would re-open the question — a real ~10k-node selection, or a way to cache through the
  preview door. `[S4.2-L4-05]`'s perimeter cache closes with it, its own sketch having read *"cache
  the perimeter beside the outline **if** `[S4.2-L4-04]`'s map is built; otherwise leave it"*.

---

## Now · SVG import

**Built 2026-08-31, the day it was asked for — §15 D394.** `Ctrl+V`, *Paste in place* and *Paste
here* all read markup on the clipboard as layers, between the app's own payload and the text-layer
fallback. `ondin_core::svg_in` is the reader; `ondin-export/tests/svg_roundtrip.rs` is what binds it
to the writer, which is the whole risk of the two living in different crates. ⚠️ **A suite only binds
what it covers**: it carried no effect coverage at all until 2026-09-03, and that is precisely where
the two had drifted (§15 D411).

⚠️ **The premise this section carried — "a parser plus a mapping onto our node kinds" — had never
been read**, and two of the three grammars were already in core's own crates: `BezPath::from_svg` for
the `d` grammar, `peniko::color::parse_color` for the colour grammar. One dependency, `roxmltree`.
*The third entry in this file to be priced as a feature and turn out to be wiring*, after *Convert to
path* and text-on-path.

~~**Open, in the order worth doing**: `<text>`, `<image>`, `<use>`, `<clipPath>` onto `Node::mask`,
and CSS `<style>` blocks.~~ **All five built the same day** (§15 D394, amended), after the paste was
tried on a real Lucide icon and worked. What is left is a list of things nobody is missing:

- ~~**Filters.** No answer in the model at all, and the only one of the original list that is a
  *feature* rather than a mapping.~~ **Read whole on 2026-09-03 — §15 D411**, and ⚠️ **the sentence
  above was false in both halves and was covering a defect.** `EffectKind::DropShadow`,
  `InnerShadow` and `LayerBlur` are exact counterparts, so this was a mapping all along; meanwhile
  the writer emits a multi-primitive `<filter>` for **every** stack while the reader took a lone
  `<feGaussianBlur>`, so **every effect this app exported was lost on re-import** — reported, so not
  silent, but *Copy as SVG* did not close for any shadow, and `svg_roundtrip.rs` had no effect
  coverage to say so. The list tell again: "no answer in the model at all" is a claim about the set
  of `EffectKind`s.
- **`<foreignObject>`**, which is HTML in an SVG and is not a drawing. ⚠️ **Read as an open item and
  is really a decided non-goal nobody has moved** (audited 2026-09-03, §15 D411): nothing in this
  model draws HTML, so there is no version of this to build. It sits here rather than in §0 only
  because no one has said the words.
- **A CSS selector needing a combinator**, which is a cascade engine rather than a lookup. Type,
  class and id are read. Re-read 2026-09-03 and correct as written.
- ~~**Per-run *font* properties inside one `<text>`, and they are the one thing here lost
  silently.**~~ **Built 2026-09-01, hours after it was written down — §15 D398.** Size, weight,
  italic and family are character spans now, the family through the same picker the node's own
  default uses. ⚠️ **The module's "nothing is lost silently" contract holds again**, which is the
  claim this entry existed to withdraw: what a `<tspan>` still loses is a gradient fill and a
  one-sided stroke, and it says so about both.
- **The paragraph, which is a structural loss rather than a missing feature.** A two-line text layer
  of ours comes back as two layers in the right places, because nothing in the markup says they were
  one node. Rejoining them means inventing the line height that decides where every line after the
  first sits. ⚠️ **Better covered than this reads** (2026-09-03): a positioned `<tspan x y>` already
  starts a new layer and a bare one joins the text, pinned by
  `a_positioned_tspan_starts_a_new_layer_and_a_bare_one_does_not`, so what is open is the *rejoin*
  alone and nothing else about the element.
- ~~**`<textPath>`**, which the *writer* started emitting on 2026-09-01 (§15 D405) and which this
  reader skips like any other unread element.~~ **Read the same day — §15 D406**, and the estimate
  held: the `href` lookup plus `BezPath::from_svg`, with `Gradients::ids` already holding every
  element that carries an id. ⚠️ **What the entry did not predict is the part that took the work** —
  the runs live inside the `<textPath>` child rather than under the `<text>`, and `text-anchor` stops
  folding into the placement and becomes `align`, because railed text has no `x` to shift from.
- ~~**An elliptical radial gradient**, which `peniko` cannot hold — the model's are circles.
  Approximated and reported (`Import::approximated`, *"elliptical radial gradient (drawn round)"*).~~
  **Built 2026-09-03 — §15 D412**, the day after the re-scope (§15 D411) that was the whole of the
  work: the reason was true of `RadialGradientPosition` and irrelevant, because an ellipse *is* a
  circle under a squash and **both backends already had the brush transform**. What was missing was a
  place in the model to put the affine, and `GradientBrush` is it — on the **brush** rather than on
  `Fill`, which the re-scope got wrong in the harmless direction, so a gradient-stroked shape gets it
  too. Wire-compatible by `#[serde(flatten)]`; `svg_in` bakes every mapping it can express exactly and
  carries only the squash, so nothing on that path is approximated any more; the writer emits
  `gradientTransform`, which it had never had. ⚠️ **Listed in the table of contents above and nowhere
  here until 2026-09-01**, which is that table's own warning happening: a row can outlive, or in this
  case outrun, its section.

⚠️ **A file dialog is not on this list and is not a decided non-goal either.** The ask was paste,
paste is what was built, and nobody has said whether *File → Import…* should exist. It is a sentence
to write when someone wants it, not a gap.

## Now · Inspector

- **The hex field commits through no shared valve, and that is the open half of `[S14.4-L1-04]`.**
  §15 D517 stopped `inspector::paint_hex_field` writing on a bare click: it compares the typed bytes
  against `hex_of` of the colour already there, which closed both measured symptoms — the 8-bit
  quantisation of anything the picker's HSV plane produced, and the undo step a click that typed
  nothing was spending. What it did not do is the finding's other half, routing the write through
  `app::edit_valve` so the field's commit *timing* is decided at the seam §9.3 exists to decide it at.
  **A shape argument rather than a defect**: there is nothing measured left to fix, and a text field
  has no drag to preview, so the valve would be belt-and-braces. It is here because the next person to
  change how this field commits should know that D316's engagement latch has never reached it and the
  comparison is what stands in for one. ⚠️ **It is not the only control off the seam** — JPEG quality
  holds its own in-flight value in `export_quality_scrub` (§9.4, §15 D274) — **but that one carries a
  stated reason and this one does not**, which is the whole of what is open here: either route it, or
  write the sentence that says why it is not routed.

- **Delete `char_valve`'s third arm, whose only known user is a rewrite the app already opts out
  of?** A maintainer question, narrowed twice on 2026-09-19 and no longer about reachability. §15
  D523 gave the Type panel's valve D316's engagement latch and kept a third arm nothing else in the
  app has — a `changed()` frame on a control that was **never engaged** — for the picker's **raw
  sensed regions**, on the reasoning that without it a click on the hue strip would commit nothing,
  ever. **The test was written and the reasoning was false** (§15 D802): `picker::pointer_slot`
  answers a click through `write_slot` before it would reach `valve_slot`, so the hue slider and the
  alpha strip never enter the valve on a click; a drag enters it as the engaged arm and its release
  as the falling edge, both arms the other two valves have; and on those frames `changed()` is never
  true. **And the arm does have a user** (§15 D803): removing `ui::value_field_f64`'s
  `clamp_existing_to_range(false)` and instrumenting the arms again, egui's own per-frame clamp of a
  stored value outside a *ranged* field marks a control holding no focus as changed, and **the third
  arm fires** — committing it and spending an undo step. That is `[S6.2-L1-01]`, and D425's one line
  is the whole of what keeps it from happening. **So the ruling is between two defences against one
  bug and a route with no wanted user at all**: deleting the arm is a second guard on D425's line
  rather than tidying, and keeping it keeps a path whose only known user is a hazard. Removing the
  arm still breaks nothing in the suite. ⚠️ **Seven call sites, two measured** — *only known* is the
  honest phrase, not *only*. ⚠️ **And the route that *is* taken now has a test** —
  `picker::text_colour_route_tests` — so nothing here is about coverage any more.

- **Menu rows carry no border, and that is what stops them moving** (`ui::menu_rows`). egui 0.35 puts a
  `Button`'s `bg_stroke` in the *layout* for a hovered or selected row and leaves it out for a resting
  one, so the two states want different sizes on **both** axes — at the panels' zero padding, 21 against
  23 tall and 2pt of width. The `MENU_ROW_H` floor fixes the height only (and only because it clears
  both); zeroing the stroke makes the states geometrically equal, which is the guarantee that holds
  whatever the natural size turns out to be. **And a floor only ever *raises***: it is 22 since
  2026-08-20 (§15 D262), which is below the 23 a row would want at the theme's own padding, so it
  decides the height only because every dropdown's scope brings that padding to 0 or 2 first. Any new
  dropdown owes `ui::menu_rows` *and* that padding, and restoring a border to a menu row puts the shift
  back.
  - ⚠️ **"Any new dropdown" is too wide, and the debt is only owed by one of the two families of row
    in the tree** (read 2026-08-31). All of the above is about rows built from an egui widget, where
    `button_padding` and `interact_size.y` decide the natural height. `ui::menu_row` and
    `ui::menu_item` are not those: they take an explicit height and `allocate_exact_size` it, so
    `button_padding` cannot reach them and the floor is not what holds them still. The Export panel's
    preset menu is the new caller this warning was waiting for, and **it is correct for a reason the
    warning does not give** — it calls `ui::menu_rows` and never touches the padding, which the
    sentence above would score as the failure case. *The tell for which family a row is in is whether
    its height is an argument*, and a warning that cannot be applied without reading the callee is
    half a warning.

## Now · Keyboard

**The whole keymap — bound, unbound and agreed — is now `shortcuts.md`.** It was specced and
signed off 2026-08-03 against Figma, Sketch, Illustrator and Photoshop, and it carries the
three unresolvable convention collisions so they are not re-argued. `input.rs` is still the
implementation and the only place *keymap* keys are read — a live text session reads its own,
including the `Tab` that nests a list, which is why that one is built without being an `Action`
(§15 D173). What stays here is the work that is *not* keymap work:

- **`Ctrl+B` is a font-selection feature, not a toggle; `Ctrl+I` is closer to one than this used to
  say.** Weight is an axis or a named instance, so there is no flag for Bold to flip — it has to pick a
  cut, which is what `type_weight_toggles` approximates with 400/700. Italic *does* have a stored flag
  (`CharAttr::Italic`, which that same pair already writes); what is derived is which variant the list
  highlights, and it now reads a face's own style as well as `ital` and `slnt`, so the old note here
  that "a family expressing italic through `slnt` reports none at all" was wrong before the change and
  is doubly wrong after it (§15 D149).
**`Enter`'s missing arms closed 2026-08-19 (§15 D228)** and are gone rather than struck. It was scored
as "an arm in `enter_action` rather than a new key" and that was right about the cost; what the scoring
did not name is the part worth keeping, which is that the arms are **`double_click_pick`'s, in
`double_click_pick`'s order** — two gestures now mean "one level in" and they must not disagree about
which meaning applies to the same layer. Two lists in one order is a duplication: if a third gesture
ever wants it, the order should become a function rather than a third copy.
- **No shortcut cheatsheet and no quick-actions palette (`Ctrl+K` / `Ctrl+/`)** — see *Command
  palette* below. The cheatsheet **is** `shortcuts.md` rendered from the registry, which is the
  argument for extending `Action` rather than keeping a second list of strings; when it lands,
  that file becomes the registry's test rather than its source.
- **Should `Undo`, `Redo`, `Save` and `Open` resolve in `Mode::TextInsert` at all?** They are in
  `normal_mode` and not in `text_insert_mode`, so the four chords are dead during a text session — by
  omission rather than by a decision, `TextInsert`'s rule being about *bare* keys (§9.3). The top bar's
  buttons for three of them were live the whole time and now finish the session before acting
  (§15 D466), so **the damage is closed and the button and its chord still disagree**. What this needs
  is the decision rather than the work: either `text_insert_mode` grows the four chords, finishing the
  session the way the buttons do, or the disagreement is recorded as deliberate and the buttons are the
  only door.

## Now · Text

**Four things are open here**: the six deferred features, the paragraph scope's absence from the MCP
snapshot, one line to suspect if the canvas ever looks a frame stale during a text session,
and whether a chord stops where its field stops. **Everything above them is closed** — the attribute
model, the three-tab popup, paragraph layout through lists and nesting, and six review passes — and is
kept as the record of how, in §15 D77–D82, D103–D109, D145, D148–D174 and `architecture.md`
§5.4/§5.11/§9.2. *Read it for the traps, not for a queue.*

The attribute model and the three-tab popup are in — design and reasoning in `architecture.md`
§5.4 / §5.11 / §9.2 and §15 D77–D82, D103–D109. The popup was **redesigned against
`design/Editor.dc.html` and `design/handoff-type-popups.md`** (2026-07-31): three tabs instead of four
(the Font tab folded into Character), one exclusive decoration instead of two independent ones, the
wrap controls moved to the Box tab as segmented tracks, the picker on a decoration's colour, axis rows
with a numeric field and a reset, `ui::switch_row` for the OpenType list, and a clickable unit chip
inside every `Length` field. Two further passes the same day closed the reported gaps: the unit chip's
strip is allocated rather than painted over (D105 — three symptoms, one bug) and lifted a point off the
field's centre to sit on its ink; the popup's height no longer ratchets between tabs (D106); the
alignment row sits on the field columns above it; the popup's field gaps match the card's and its
sections are 15 apart; right-click no longer dismisses either popover (it is spent on gesture-cancel);
the decoration swatch opens the picker, and its reset asks `ragged` rather than byte 0; and **both
popovers moved to the left of the inspector** so they stop blending into the cards (D107). A third pass
found the one that mattered: **a valved edit was comparing itself against its own preview, so no drag
ever committed** — the value lived in the override until something cleared it, which is what made the
whole popup look dead after the picker was used (D109). Also: `ui::slider` is now hand-painted to the
design, the unit's hover is a colour rather than a ground, the paragraph fields valve (D108), a
drag-ending click no longer dismisses either popover, and "Clip content" is a switch.

A fourth pass (2026-08-03) took this section down to the deferrals. In order: the sizing, font-metadata,
gradient-readout, sign-hint and by-eye-icon entries (**D148–D152**); **D81**, one `<text>` per line at its
shaped baseline with one `<tspan>` per run, `text-anchor` gone because alignment is now in the line's own
`x`, at the cost of a `text::export_lines` seam in core; and **per-run colour end to end** (**D154**),
whose slot-verb hoist closed **D129**'s press-preview gap as the side effect it was predicted to be. The
plan document it was built from is deleted; D154 is the whole record.

A fifth pass (2026-08-04) closed the *Improve typography feature tags* section and **five of the deferrals
here** with it, in this order: the OpenType list became three tiers of authority — the font's own
`ssXX`/`cvXX` strings, a 124-tag table of the registry, then the raw tag for a private one — with the
registry's own UI-suggestion text deciding what is offered at all (**D157**); an auto-sized label's left
edge holds its right one (**D158**); a run with an underline and no strikethrough exports
`text-underline-offset`, negated (**D159**); a trimmed auto label's handles and its gesture measure from
one datum, which also corrected D158's own note — the mismatch was never a `Fixed` box's (**D162**); and
line height's Auto state shows what it resolves to and seeds from that rather than from a round 120%
(**D162** as well). The popup's four first-draft numbers were reviewed on screen the same day and
accepted, so they are a settled decision in §15 D152 rather than an open question here.

A sixth pass (2026-08-05) closed **the three defects that were left here**, each of which named the
decision it wanted rather than a patch, and each of which turned out to want a little more than it said.
The span re-statement's undo now works because both style ops carry `spans: Option<…>` and their inverses
always fill it in, which also hoisted the re-statement into `Spans::restated` and found the **render
preview** was not doing it at all — a divergence no picture shows, because what the re-statement drops is
by definition an echo, so the test for it had to assert on the override's *kind* (§15 D163). The whole-node character write a live editor put back is
answered by `text_session_restyled_after(&tx)` — the transaction is the witness, so the rule cannot be
got wrong at a call site and it answers per *scope* — plus a canvas refresh the original report did not
contain (§15 D164). And the click that landed a line low was **not 2.2pt but 40**: a probe over the whole
band showed an inverse-by-subtraction has no answer anywhere inside a paragraph gap, because the gap does
not exist in layout y, so `YMap` is one direction now and `Shaped::layout_y` resolves the line from the
box tiling first (§15 D166). Two of the three are pinned by tests confirmed to fail without the fix; the
third is app wiring, so its *decision* is pinned as a free function and the wiring is still read rather
than reproduced.

**Then the same pass built paragraph layout's stage 3 — lists — and the plan's route turned out to be
unusable** (§15 D169). The marker was to be a parley out-of-flow inline box; the todo said to check that
first, and checking it is what settled the design. parley honours `InlineBoxKind` when it *draws* and
ignores it in three x-readers, so a 30px out-of-flow box at byte 0 leaves the glyph at x 0 and puts the
caret at x 30 — D165's shape mirrored. So a marker is **ink without bytes**: not in `content`, not in
parley, right-aligned on the `indent_start` gutter D163 had already built, which is the hanging measure
the plan was after. The render path needed nothing at all, `TextLayout::runs` being per-run already; what
needed care was the *export*, a second path that reads `export_lines` and not `runs`. `trim_insets` was
expected to want work here and does not, for a reason that only holds because the marker left parley.
Nesting followed as stage 4 (§15 D172): a `level` field of its own rather than a reading of the
indent, one gutter per level out of one function both the lines and the marker call, and a stack of
counters so a sublist restarts while its parent counts through. What is left: two
clusters deferred on purpose, two items parked with MCP. (The reading that was to be checked on the
machine has been — it was right about the bug and wrong about what fixing it would cost; §15 D174.)

**Paragraph layout has been lived with, and the six things that came back are closed** (§15 D165–D168).
Two of them were not about paragraphs at all and are the pair worth remembering: `Mode::TextInsert` routes
to a **second input path**, so it had no middle-click pan and no keyboard-focus guard — a session swallowed
the digits typed into the Type panel and put them in the document. *A second input path owes every guard
the first one has, and loses each of them silently* (D167).

Every fix was confirmed by putting the bug back, and three things reading alone did not settle: a NULL
`name` id is `0`, which is the **copyright notice**, so an unguarded read puts the foundry's copyright on
the hover of every `cvXX` row; an existing test was *encoding* D158's bug, expecting a left-edge drag and
a right-edge drag to produce the same width; and a stale *comment* was the tell for the line-height one,
having described the readout the code did not implement.

**Four things the fourth pass found that reading alone would not have.** Two were live bugs older than the
work — **D153**, where a span on the *first* character leaked its baseline shift and decoration onto every
run that merely inherited (the brush an inheriting run carries is the defaults', which was also style run
0's index), and **D156**, the picker's handle jumping up to 36pt sideways near black because it re-derives
its position from an 8-bit colour. Two were mine: D81's stated mechanism was wrong three times over, and
**D155** collects the four things that only showed up on a real run — plus the three failed attempts at
the last of them, which is the part worth reading.

**"Nothing in this section was found by a test" was true until 2026-08-04 and is worth striking rather
than deleting**, because what changed is *which* tests pay off here. Four found real bugs in one pass, and
none of them was an assertion about the feature being built: a tripwire between two mechanisms caught
`\r\n` being two hard breaks in parley and not one, on its first run; a test written only to de-risk a line
lookup caught the `YMap` handing ink coordinates to a table keyed on line boxes (D166); an *existing* test
caught a containment search returning the line above; and a `RawInput` probe caught egui forgetting a drag
on a same-frame right-click (D168) after reading had failed three times. The pattern: the tests that earn
their keep here are the ones aimed at a **seam between two mechanisms**, not at the mechanism. What is
left:

- **Deferred outright**: ~~hyphenation~~ (**a decided non-goal since 2026-09-01** — §0 above, §15
  D399), justify-all, tab stops, columns, widow/orphan control, ~~text-on-path~~ (**built
  2026-09-01** — §15 D405).
  ⚠️ **"All six dependencies are still genuinely absent" was one sentence covering six
  different situations, and re-deriving them found *none* of the six it describes.** The 2026-08-31
  pass left hyphenation standing as the one item the sentence was true of; re-deriving that one on
  2026-09-01 took the count to zero, because the crate it named exists and the thing actually in the
  way is not a dependency at all. The generalisation is the bug: five items were swept in behind one
  named dependency without ever being priced, and the named one had not been priced either.
  - ~~**Hyphenation — the dictionary crate exists, parley already breaks at the soft hyphen, and what
    is left is a hyphen's width the line breaker is not charged for.**~~ **Decided against 2026-09-01
    — a non-goal for v1, in §0 above, and §15 D399 is the record.** *"agreed, file it and don't build
    it"*, taken on the price the re-derivation had just put on it: the offset map §15 D80 specifies, a
    second layout pass, and a dictionary crate (`hyphenation 0.8.4`, which the old *"there is none in
    the lock"* had measured the lock file for and not the world).

    ⚠️ **Read D399 rather than reconstructing this from the strike-through, because the danger here is
    a wrong summary and not a re-argument.** Every measurement that priced it is in that entry —
    parley breaking `"hy\u{ad}phen\u{ad}ation"` into three lines at a 60pt measure where the plain
    word overflows to ~112pt, the soft hyphen's ink box being *identical* to the plain one
    (`x0: 1.4375, x1: 112.119140625`, thirteen glyphs against eleven), the 8.99-wide hyphen that takes
    a 46.34 line to 55.33 in a 50pt box, and the four `pub` parley calls that make charging it a retry
    in `text::break_lines` rather than justify-all's `pub(crate)` wall. **Nothing about it was
    blocked**, upstream or here; it was priced and declined, and `text::break_lines`' own doc comment
    carries the same warning on the loop the retry would have gone in.
  - ~~**Text-on-path — every dependency has arrived, and this is the *skip-ink* shape again.**~~
    **Built 2026-09-01 — §15 D405.** The rail is a field on the text node (a container and a
    cross-node reference were both offered and both declined), the layout is bent onto it as a pass
    over the finished flat one, and the writer emits `<textPath>`. ⚠️ **The dependency half of this
    entry was right and "a design, not a feature" was wrong in one clause that decided the size of
    the work**: `text::Glyph` was `{id, x, y}` with **no rotation**, and both backends draw a run
    under a single `Affine`, so the render boundary had to gain a concept before any of the
    arithmetic below could be used. Left as history because the *shape* of the misprice is the
    fourth instance of this file's own warning, and the first where the premise was under- rather
    than over-stated.
    ⚠️ **And the underpricing carried on past the build, which is the part worth keeping.** Five
    more entries followed within two days, none of them foreseen here: the flip and the
    `<textPath>` reader (§15 D406), then a **caret drawn in the space the ink had left** plus a
    decoration a whole baseline off its rail (§15 D407), then the affordance that makes the feature
    reachable at all — hover a path edge with the Text tool (§15 D408) — then that affordance
    making the *menu row* redundant, plus a start offset, because the type began wherever the shape
    happened to have been drawn from (§15 D409), and finally a handle to slide it along the rail
    (§15 D410), which closed it. *The feature that priced as
    a design and cost a render-boundary change went on costing after it was called done*, and every
    one of the five was reported or asked for rather than found by a gate. **Three of the five are
    the same shape**: a feature that works is not a feature anyone can *reach*, and neither the
    affordance, nor the start point, nor the way to move it was in anybody's estimate.
    **Accepted and closed 2026-09-02** — the whole of D405–D410, with the declined alternatives
    listed at the foot of D405 so none of them is re-argued from first principles.
    The original reasoning, all of it still true: glyph
    outlines came with outside-aligned type strokes (`text::outline`, §15 D145); the path model is
    `BezPath`; and the piece nobody looked for is in kurbo, which core has depended on all along —
    `ParamCurveArclen` carries **`inv_arclen`**, the solve-for-the-parameter-at-a-given-arc-length
    that placing glyphs along a curve is entirely about, and it is implemented **for `PathSeg`**,
    the segment type a `BezPath` already yields. Nothing is behind a feature flag. What is left is
    a *design* — where the path lives relative to the text node, and what the SVG writer emits —
    which is a different and much better problem than a missing crate.
  - **Justify-all is not a dependency question at all.** parley is present and in use: `TextAlign::Justify`
    is wired, and `justify_last` already ships three readings of the last line that are ours — the line
    is *translated* rather than re-spaced, which is what made those three affordable and is stated on
    `JustifyLast` itself rather than in §15, no entry naming it. What blocks justify-*all* is upstream and
    internal: `align_impl` hard-codes skipping `BreakReason::None | Explicit`, and `align`,
    `LayoutData` and `ClusterData::advance` are every one of them `pub(crate)`. **That files it beside
    the flo_curves comparator rather than beside hyphenation** — an upstream gap, and one small enough
    upstream to be worth a patch rather than a fork.
  - **Tab stops** — parley classifies `Whitespace::Tab` and does no tab-stop layout, so this is a
    feature to build on top of it, not an absent dependency.
  - **Columns and widow/orphan control** never had a dependency. They are unbuilt features of our own
    paragraph layout, and pricing them means reading `text.rs`, not the lock file.
- **A run colour and a list marker are both invisible to the MCP snapshot**, which emits node-level paint
  and the raw `content` — and a marker is deliberately not in `content` (§15 D169), so an agent reading a
  list sees unmarked paragraphs. The **nesting level** goes with the marker (§15 D172): same emitter, same
  trip, and nothing of the paragraph scope reaches the snapshot but `align`. **That last clause was an
  understatement** (measured 2026-08-21): `align` is indeed the only one of `ParagraphStyle`'s
  **thirteen** fields that arrives, so twelve do not — `spacing`, `indent`, `hanging`, `indent_start`,
  `indent_end`, `justify_last`, `wrap`, `word_break`, `overflow_wrap`, `direction`, `marker`, `level` —
  and `snapshot.rs`'s destructure of `NodeKind::Text` closes with `..`, which also drops `para_spans` and
  the whole of `BlockStyle` with it. So the gap is twelve of thirteen paragraph fields *plus* the block
  scope, not run colour and markers. Worth stating as a count rather than as examples, because a `..` is
  the one thing that grows silently as the model does.
  Parked with MCP (§8) rather than open, and now the second *and third*
  things waiting on it — the missing-font warning under *Doc drift* is the other, so whoever builds the
  span emitter should do all three. The marker is the one that needs a decision rather than a field: a
  snapshot either reports `marker` as an attribute, or renders the label into the text it emits, and only
  the first keeps the byte offsets an agent might write back with honest.
- **The *wiring* on the character- and paragraph-attribute path has no automated test above core**, which
  is what this bullet's own last sentence always said and what its opening sentence overstated until
  2026-08-21. Two corrections, both in the direction of there being more cover than claimed. The Text
  tool's press/release **routing** (§15 D170) is *not* untested: `input.rs`'s `mod text_insert_tests`
  drives `input::actions(Mode::TextInsert, …)` directly and pins the chord routing, including the seam
  itself — `the_alignment_chords_resolve_and_centre_comes_off_the_release` drives both the press and the
  release and demands that only the release resolve. And the list of pinned free functions was short by
  at least four: `typography::char_attrs_tx`, `TypeSubject::ragged`, `decoration_of` and
  `TypeSubject::text_ramp` all have assertions of their own. What genuinely rests on reading is the list
  the sentence should have been: **the three slot verbs, the picker's frame-by-frame call pattern,
  `cancel_gesture`'s gate, and every cursor decision** — which is exactly where two passes have now
  gone wrong.
  **The reason that list was unreachable is gone as of 2026-08-22 (§15 D303)**: `OndinApp::headless`
  exists, because the wgpu dependency turned out to be one field touched by two functions rather than a
  property of the app. So none of the above is blocked any more — it is simply unwritten, which is a
  different and much better problem. ~~D35's standing instruction~~ **done the same day**
  (`rulers::guide_grab_tests` — the guide grab and the teleport-on-click fix it asked for), and
  ~~the 34-comment prose sweep~~ **done with it**, and ~~the four still verified by reading~~ — the
  line chrome, `pen_verb` routing, autopan and the cursor ladder — **written and flip-checked the same
  day**. Writing them found a doc comment on `Drag::autopans` arguing both ways about guides, which is
  the return the exercise was for. ~~The five places whose comments said "writable now and
  unwritten"~~ **answered the same day** — three written (`cancel_gesture`'s gate, the autopan restore,
  the arrow router) and two declined with the reason recorded where the comment was: the zero guide
  step is gated behind a read of the real **system** clipboard, and `paint_unit`'s callers and the
  Rotation field's wiring wanted a `DragValue`'s frame-by-frame interaction state reproduced. Those two
  got one assertion that covers every field in the panel instead — *drawing the inspector commits
  nothing and previews nothing*. ~~The three slot verbs and the picker's frame-by-frame call pattern~~
  **written the same day** — the preview/commit agreement, the character-scoped diversion (whose first
  draft was vacuous and was caught by its own flip), and `pointer_slot`'s press-previews and
  cancelled-release arms driven through real frames — and ~~two named remainders~~ **`valve_slot`'s
  share of the diversion and `paint_unit`'s `last` flag with them**. **So this bullet's list is
  empty**, and the whole of what it recorded as resting on reading now rests on tests. *Two of those
  tests were vacuous in their first draft and both were caught by their own flip*, which is the note
  worth carrying forward from the exercise rather than the count.
  The free functions and the plain data types *are* pinned (`hsv_for`, `char_color_attr`,
  `PaintSlot::char_scoped`, `para_attrs_tx`, `char_attrs_tx`, `decoration_of`,
  `TypeSubject::para_ragged`/`shown_paragraph`/`ragged`/`text_ramp`,
  `canvas::stated_spans`, `tools::create_text`, `preview::CaretBlink`, core's
  `reshape_keeping_selection` and `text::paragraph_bounds`), and so are two pieces of egui's own behaviour
  that a decision depends on (`cancel_gate`, and `canvas::text_gesture` for click-or-drag and the drag
  threshold); the **wiring** between them is not. *The pattern that keeps working: lift the decision into
  a free function or a plain type, pin that, and say plainly which wiring is still only read.*
  Two things settled a bug faster than reading did
  and are worth reaching for early rather than late: a temporary `eprintln!` of a call *pattern*
  (`pointer_slot` + `write_char_slot`), and a `RawInput`-driven probe when the question is *when in the
  frame* rather than what (§15 D168, D170) — the second has now paid off four times and is the first
  thing to reach for whenever a claim is about egui rather than about us. The fourth was the valve
  question this section used to carry (§15 D174), and it is the one to remember: the *user* reported that
  field working, because a preview and a commit look identical from the chair.
- **One thing to watch on the next text edit, not a task.** D171 replaced `draw_canvas`'s unconditional
  `request_repaint()` for a live session with a `request_repaint_after` scheduled to the caret's next
  blink. The call it replaced carried no comment saying what it was for, and `draw_canvas` is not
  headlessly testable — so if anything on the canvas ever looks a frame stale *while a text session is
  open*, that is the line to suspect. Everything else that could have wanted it has a repaint of its own
  (the chrome-hold timer, the font service, `resp.dragged()`, and input itself).
- **Should a chord stop where its field stops?** `text_chord`'s `TextChord::Tracking` arm steps
  `0.01em` per `Alt`+`→` through `apply_char_attrs` with **no range check at all**, and `TextStyle::set`
  canonicalizes `letter_spacing` without bounding it — so a held key walks tracking past
  `MAX_TRACKING_PCT` and out of what the field can be scrubbed to. The `Size` arm in the same `match`
  is the other answer, and says so in a comment: `CharAttr::Size` clamps itself to
  `MIN_FONT_SIZE..=MAX_FONT_SIZE`, so held keys stop at the ends without a second bound at the chord.
  **This is a question rather than a defect** — the caps are on the controls and not on the model, a
  file may legally hold any finite value, and since §15 D475 the field *shows* an out-of-range value
  instead of rewriting it — so nothing is lost today. What wants deciding is whether the two chords
  should agree, and if they should, whether the bound belongs at `TextStyle::set` (where `Size`'s is)
  or at the arm. **The decision, not the work**: it is one `.clamp` either way. Found while writing
  D475's test; `[S6.2-L1-01]`'s other half is closed, `[S6.3-L1-03]` having gone with D425.

## Now · Doc drift

- **The two font warnings are still two questions, and only one of them is now well asked.** The Type
  panel's landed 2026-08-19 with the third state it needed (§15 D227, §5.4a): `FamilyStatus` is
  `Ready`/`Pending`/`Missing`, and the family row wears `WARN` on its glyph for the last of those. What
  did *not* close is the other half of the old item — "the snapshot's warning and the panel's can be
  the same question asked twice". They cannot yet: `snapshot.rs` is in `ondin-export`, which has no
  `FontService` and asks `text::is_family_available` directly. **That is honest rather than broken** for
  a headless run, which never fetches anything and so has no "not here yet" state to confuse — so this
  is a *unification* worth doing only if the snapshot ever reads a live service, and is not a defect
  in either warning today. Recorded so nobody re-derives the three-state design for a consumer that
  does not need it.
**Nothing else is open here.** The golden-files claim (corrected at seven sites, built for SVG and
JSON in §15 D304, the PNG third parked below), the `D103` numbering collision and the missing-font
warning's UI half all closed 2026-08-19 to 2026-08-22 and are gone rather than struck. **The bullet
above is a note against re-deriving, not work.**

---

## Principle — when a mode earns its keep

**Reconciled, and not where this said.** The trigger — "once something is built on it" — has fired
twice: §9.4 states the principle for the node tool in the four terms below, and §15 D125 leans on the
same four to say why a *pen bias* inside an edit is not the mode this argues against. The destination
turned out to be **§9.4**, beside the tool, rather than §9.1. What is left here is the principle itself,
kept because §9.4 states it about one tool and this states it generally.

- A mode earns its keep only when it resolves an ambiguity the pointer target and the active tool
  cannot resolve alone. `Mode::TextInsert` does; Command Mode (§13) will. An origin-set mode and
  Figma's vector focus mode don't — the thing under the cursor already says what a drag means.
- **If it is a mode, make it a tool.** The tool rail is already modal and nobody minds, because a
  tool is on the rail, named, has a letter, and **puts its own chrome on the canvas**. Every mode
  failure below is one missing at least one of those four.
  - That fourth term read "announces itself through the cursor" until 2026-08-03, and the case the
    principle was written for falsifies it: §15 D118 records the node tool's cursor going back to the
    plain arrow *as a correction*, because a path already draws a marker on every anchor and a second
    announcement under the pointer is a hand flickering across a dozen marks. Canvas chrome is the
    term that survived contact; a cursor is one way to supply it and not the test.
- Focus comes from scoping what is *displayed*, not from restricting input.

---

## Now · Path editing

**Closed 2026-08-03.** The whole feature is built and its record is §15 D114 and D117–D125. What
used to be here — six inherited rules, three standing decisions, four recurring traps — was checked
line by line against the document and the code before being removed, and every item had a home:
`PEN_PICK_PX`, `gesture_travelled`, the scrub-versus-drag delta direction, `rewrite_path`'s no-op
comparison, the signed handle drag (D121) and line-or-cubic-per-segment (`straight_to`) are all in
`architecture.md`; the four traps are D116, D122 and D123; `SNAP_RING_PX` is not in the document at
all but `canvas.rs`'s doc comment on the constant states the "still not `PEN_PICK_PX`" rule better
than this file did. The fillet-versus-arc construction and the inferred point type were each
recorded **twice** — in D119/D114 and again here — which is the duplication this file's own rule
exists to stop.

One item is left, and it is a decision rather than a gap. (It was two until 2026-09-19, when the
`retain_valid` / `subpath_lengths` pair was ruled on — the `allow`, and the no-effect call removed;
§15 D637, now *Resolved*.)

- **Every point edit is a whole-path `GeometryPatch::Path`, deliberately.** Right for undo
  granularity — one gesture, one step — and coarse only for **MCP**, where "move anchor 3" would
  rather be an op than a path replacement. MCP is parked until the editor is finished (§8), so this
  is revisited *with* that work and not before: an op-level patch designed now would be designed
  against a consumer nobody has written. **D123's closing note explicitly asks for this to stay
  here** — "it stays in the todo as a decision rather than a gap" — which is why it survived the prune.
  (It read D125 until 2026-08-03; D125's closing note is about something else, namely `pen_verb`'s
  routing, which was the last untested decision in the path-editing work and is now pinned — §15 D318.)

## Now · Files, library and storage

**Four things are open here, and all four came out of the codebase review or out of closing one of
its findings** — the last three on 2026-09-09, when §15 D613 struck one item while §15 D619 and §15
D620 each added one, so the count moved by one in a session that changed three of its members.
(This paragraph said *"one thing … 2026-09-06"* and was a count behind before the 2026-09-07 item was
added; ⚠️ **a count that happens to stay right through a strike and an addition is not evidence it is
maintained** — read the bullets.) Both of the section's original open questions
closed 2026-08-28 (§15 D384) and are struck below with what each got wrong; what is left under
*Technical, and still binding* is a set of **standing rules**, not work. The rest of this section is
the record.

**Built 2026-08-26 — §15 D362–D366 and §5.11a, §9.5.** The base folder, the naming, projects,
autosave, versions, the trash, the dashboard, search, import, relocation and covers all landed
together. Three of this section's answers came out **differently** from what was recorded here, and
those reversals are in §15 rather than struck silently: metadata travels *in the file* rather than in a
cache sidecar (D362), a project is an id rather than a directory (D363), and *Per-file Set location…*
is gone with the dialogs it needed (D364). Two of the four **Open** questions were answered — version
history lives in the base folder under `.versions`, and a duplicate gets a **new** id — and what
remains is below.

**`design/Dashboard.dc.html` was walked end to end against the code on 2026-08-28**, so what is left on
that screen is a read rather than a survey to re-run. **All four of the walk's findings were built the
same day** and live in §15 rather than here: a single click picks a document out (D372), a `.ondin`
dropped on the library is filed in it (D373), the library answers its own keys (D374) and a project's
grid ends in a *New file in {project}* card (D376). Three things the walk settled and nobody should
re-check: every literal label in the design is accounted for in the
code; the app is **ahead** of the design in three places — *Rename* and *Star* in the ⋮ menu, and per-nav
empty states, none of which the design has; and the design's *Import files* modal with its *Target
project* picker is answered differently on purpose, the target being the project the user is looking at
(§15 D365). The departures the walk found and the maintainer declined — the *New project* wording and the
switch's side, the 16pt circle swatches, the outline star, no resting star on an unstarred card, and
`cursor:pointer` — are §15 D367, D369 and D371, and are **not** open work.

⚠️ **The middle one of those three "settled" clauses moved the same day, in two ways, which is why
this paragraph is dated rather than standing.** *Per-nav empty states* lost a case: a project with no
files now shows the
dashed *New file* card instead of an empty state, because the empty state's sentence and the
base-folder path under it are a **library** question (§15 D376); the empty state is still what a
dangling project id gets, and still what the other four navs get. And the app is ahead of the design
in a **fourth** place, which the walk could not have found because the design has no vocabulary for
it: the amber `CONFLICT` mark on a sync client's copy (§15 D375). Neither is a walk finding — they
are what the walk's findings turned into — but a reader taking "three places" as a current inventory
would be wrong, and that is the drift this file exists to not have.

**Open**

- **A dashboard cover is rendered synchronously inside the egui pass, and one document can cost
  hundreds of milliseconds.** `Covers::get` → `cover::rasterize` → `io::load` → `png::png` →
  `ImageStore::prepare` runs the whole document→pixels path on the UI thread, and a picture's decode
  alone was **measured at 433 ms** for an 8000² image (§15 D449). The 8 ms `FRAME_BUDGET` does not
  bound it: the progress guarantee renders one document per pass *whatever the budget says*, without
  which a library of slow documents would draw no covers ever — so the cost is one stall per pass
  until the covers are built, rather than one long freeze, which is the worse-feeling of the two.
  **D449 caps how large a picture may become and deliberately does not fix this**: a 50 MP camera file
  is 240 MB of RGBA and legitimate, so no bound that admits it can refuse the case that stalls. Moving
  the render off the UI thread is what would make both the budget and the progress guarantee
  unnecessary; which thread it goes on is undecided. Same class as the `Document::apply` cost
  parked under *Per-edit cost*, and a different remedy: that one needs a decision, this one needs a
  thread.
- **A `.ondin` whose filename is not valid Unicode is invisible to the whole library, migration
  included.** `scan::collect` skips any entry whose `file_name().to_str()` is `None`, so such a file is
  never an entry, never scanned, and — the half that made it a finding — is silently left behind by
  *Change base folder*, uncounted, exactly as a `.trash` collision used to be (§15 D431 fixed that
  half and not this one). **Never reproduced**, which is why it is here rather than in §15 with a
  verdict: it is not established that a filename Windows accepts can fail `to_str`, and the fix
  differs by whether the answer is "skip but count it" or "carry it by `OsStr`". Reproduce first.
- ~~**`scan::Entry::unread` has no production consumer — give it one or delete it.** `scan::collect`
  writes it, `library/state.rs` hardcodes it `false`, and the only thing that reads it is
  `a_corrupt_document_is_listed_rather_than_hidden`'s assertion; `Entry` derives `PartialEq`, which is
  what keeps `dead_code` quiet about a field nothing consumes. Goes with the library-store pass; the
  two answers are a mark on the card (the `CONFLICT` mark's shape, §15 D375) or removing the
  field.~~ **Closed 2026-09-09 — §15 D613, and it took the first answer.** The field got a reader:
  a red `UNREADABLE` chip, the same chip the conflict copy wears, at both call sites. ⚠️ **The flag
  was not enough on its own and the entry could not have known why.** It is set only through
  `MetaProbe::Inconclusive`, so the case that actually reaches a synced library — a document written
  by a **newer build**, well-formed JSON that `io::load` refuses — was reported healthy; the mark
  reads `entry.unread || covers.unreadable(entry)`, the second being `io::load`'s whole-file answer
  that `cover::rasterize` was already computing for the thumbnail and discarding. **What is still a
  decision rather than work** is whether *Open* should be disabled or warning-styled on such a card;
  the mark and the status line already say it, and refusing to open a file the user may want to
  inspect is not obviously right.
- **The per-machine index has no injection point, so running the test suite edits the developer's own
  *Recent searches*.** `LocalIndex::save` writes `dirs::cache_dir()/ondin/library.json` and
  `Library::open` loads from it; the `app()` fixture redirects the **base folder**, which is a
  different knob, so anything driving `OndinApp` reaches the real file. Found 2026-09-09 while closing
  `[S20.1-L6-05]` (§15 D619) — **the first run of that test found its fixture already populated from a
  previous run**, and the arrows test has been writing there for longer. ⚠️ **The `load_from`/`save_to`
  pair §15 D370 built is the right answer and nothing routes the app through it**, which is the shape
  worth noticing: a mitigation can exist, be documented as the rule (§9.5), and be reached by no
  production path. What is undecided is *where* the path comes from — a field on `Library`, an
  argument threaded from `OndinApp`, or a process-wide override for tests only — so this is work
  rather than a ruling. Until it lands, a test that touches this path asserts persistence through
  `save_to`/`load_from` on a temp path and says why.
  ⚠️ **The model already exists one module away, so the *where* above has a fourth and cheapest
  answer** (found 2026-09-09 writing §15 D626): `prefs::Prefs::ephemeral` is a `#[serde(skip)]` flag
  `OndinApp::headless` sets and `Prefs::save` reads on its **first line**, before anything touches the
  filesystem, while `save_to` stays available for a temp path — so a headless app's writes never leave
  the process and no test has to remember to redirect anything. D626's export test drives
  `write_export`, which calls `prefs.save()`, and touches no `prefs.json` at all, which is the negative
  this bullet wants. `save`'s own comment gives the reason in the words this entry needed: *"the damage
  is silent and permanent — the file it writes outlives the test by however long it takes somebody to
  notice."* **One module has the shape and one does not.**
  ⚠️ **A second resource was found with no injection point on 2026-09-19 and had one by the end of
  the day** — the OS clipboard (§15 D796; **the ruling is D798**): `OndinApp::headless` now
  sets a process-wide `CLIPBOARD_OFF` flag that the two readers and both writers check, so this
  bullet is about the per-machine index alone. It is kept as the shape, because the clipboard took
  `Prefs::ephemeral`'s answer — a flag set by `headless` and read before the resource is touched —
  and that is still the cheapest of the four above.
- **A partly-failed library migration has no way back, and the full list of what it lost is nowhere on
  screen.** `Moved::failed` names every file that stayed behind and `summary()` puts the first of them
  in the status line (§15 D620), which is enough to act on and is not enough to work from: the
  *Library settings* modal shows none of it, and `apply_library_settings` re-points the app at the new
  root whether the migration succeeded, partly succeeded or did nothing. ⚠️ **The module's own defence
  is an argument for a retry the UI does not offer** — *"a migration run twice leaves duplicates rather
  than being idempotent, which is the right way round"* is true and assumes somebody can run it twice,
  where today that means a three-step dance back through Settings whose middle step looks like the
  operation that lost the files. Three separable asks: the full list in the modal, a *re-run the
  migration* button, and rollback. The first two are work; the third is a **decision**, because
  `relocate` deliberately overwrites nothing, so "undo" means deciding what to do with everything that
  did arrive.

**Both of this section's *original* open items were closed on 2026-08-28 (§15 D384) and are struck
rather than deleted, because each was wrong in a way worth keeping.**

- ~~**A missing file is not marked missing**, because the library is scanned rather than indexed: a
  document on an unplugged drive simply is not in the list, with nothing to say it existed. Whether
  that is a defect depends on whether documents can ever live outside the base folder, which today
  they cannot.~~ ⚠️ **The premise was right and the conclusion was the wrong size.** Documents cannot
  live outside the base folder — so the unit that goes missing is never *a file*, it is the **whole
  library**, and the app could not tell an unplugged drive from an empty one. Worse than cosmetic:
  `refresh` swept the per-machine index against the scan, so one launch with the library offline
  dropped every star on the machine. The sweep is now gated on the folder having actually been read,
  the empty state says the folder is unavailable, and the three doors that create a file refuse
  rather than resurrecting the path. Per-*file* marking stays impossible by design, which is the half
  this entry had right.
- ~~**The `Created` column is UTC**, so it can name the wrong day for a few hours around midnight.
  Correct local time needs the zone's *history* — a July stamp has a different offset from a January
  one — so it is a dependency or a platform call. Raised and parked with the maintainer.~~ ⚠️ **"A
  dependency or a platform call" was true and the second half was already paid for.** Windows keeps
  the zone's history and answers for a given instant, through `windows-sys` — which this app has
  linked since D241 for `GetCursorPos`. One function, one added feature flag, no new crate. *An entry
  that names two options and prices only one is how a cheap change stays parked.*

**Technical, and still binding**

- **Filesystem is truth.** Every action writes to disk and re-scans; nothing about a document lives
  only in the app's memory, and no fact may live only in the cache.
- **Don't break the format's git properties:** `to_vec_pretty` (line-level diffs), the id-sorted
  node list, stable string ids, byte-stable output (invariant 9), the camera living in
  `EditorSession` rather than the DTO, and `skip_serializing_if`. **A modified-at stamp or view state
  in `DocumentDto` kills all of it** — which is exactly the rule §5.11a's block is written under and
  the reason an edit time is read off the directory entry instead. Re-checked 2026-08-26: all six hold
  with the block in place. The old imprecision stands corrected — *"a structural edit touches one
  entry"* is really **two**, since `NodeDto.children` is a `Vec<String>` and a create or a reparent
  rewrites the parent's array as well as the child. The property worth defending is that there is **no
  cascade across unrelated nodes**.
- **Debounce the autosave write** — writing on every op into a synced folder thrashes the client. The
  interval does this today; nothing writes per-op.

## Now · Text alignment and measurement

Box trim is in (§15 D78) and is now **on for new text**, with the line box it gave up drawn dashed
beside it (§15 D199), and baselines are snap targets with a selected node's own drawn solid beside
that (§15 D355). What stays here is the part that is not a text attribute:

- **Don't use ink bounds for alignment.** They depend on the string, so two labels aligned by ink
  jump apart the moment the text is edited. Font-metric-derived trim is stable — which is why trim
  is the mechanism and ink bounds are not.
- Later: side bearings / optical margin alignment (a left-aligned label still reads as indented,
  because "H" has almost no left side bearing). "Figure out distances" is closed — the Alt-hover
  measure and then equal-gap snapping with its labels (§15 D198, D200).

## Later · Command palette (`Ctrl+K`, with `Ctrl+/` as an alias — `shortcuts.md` §8)

**Distinct from Command Mode (§13), deliberately.** The palette is for *app* actions — toggle
things, open panels ("Open Typography panel"), search layers. Command Mode is a command prompt for
*manipulating the document*, with its own vocabulary of nouns and selectors. Keeping them separate
is what stops the palette becoming a half-CLI that never becomes either thing.

- **Build it on a command registry that extends `Action` — not on its own list of strings.**
  `Action` + `dispatch` is already the nearest thing to a registry: a resolved-intent type consumed
  in exactly one place. A parallel list drifts from the keymap inside one release.
- A registry entry wants: display name, an **availability predicate** (Ungroup with nothing selected
  should not offer itself), **current state** for the toggles (`Show rulers ✓`), and the `Action` it
  dispatches.
- **`menu.rs` is that registry, one door early, and it should be read before a second one is
  written.** Context menus landed first (§15 D214–D215) and needed exactly the four fields above:
  `Item::spec()` gives display name, glyph, accelerator and group, `Row::checked` is the toggle state,
  and most items dispatch straight to an `Action`. It knows nothing about being drawn as a menu,
  deliberately, so the palette and the **shortcut cheatsheet** — the same registry rendered as a list —
  extend it rather than start again. **Two corrections to how it is described here** (2026-08-21).
  Availability does not live in `menu::build`: that function only dispatches on `cx.target` to one of
  six per-target builders and then sorts the rows into `Group::ORDER`. The predicates live in the six
  builders, as `Row::dim_if(cond, why)` — **dimming with a reason rather than filtering**, which is
  §3's own rule and is also the better shape for a palette, since a hidden row teaches nothing.
  And the gap is bigger than "a search key". `menu::Context` is **one question per field** —
  `Context`'s own, with `LayerState`'s flattened into the one they hang off — keyed on a **mandatory**
  `Target`, and a palette has no target at all; something has to answer that. **There is deliberately
  no number here any more.** It is `(fields of menu::Context) − 1 + (fields of menu::LayerState)`,
  read off the two `struct` declarations in `crates/ondin-app/src/menu.rs`, and it has been in the
  low thirties since 2026-09-07.
  - ⚠️ **This said 28 and vouched for it** — *"re-derived on 2026-08-31 and is exact"* — and rotted
    three days later, on `LayerState::even_odd` (§15 D400) and `Context::on_a_rail` /
    `Context::rail_flipped` (§15 D405, D409). The paragraph hedged the counts either side of it and
    certified the one that moved, which is this file's own warning above landing on the single number
    exempted from it. **A count that carries its own certificate of accuracy is the most trusted kind
    and the most expensive to be wrong**, because the certificate is what stops the next reader
    checking. Counted from the two `struct` declarations, not from this line.
  - 🚨 **Then it said 31, and that rotted too — which is why there is no number here now.**
    `[A7-L8-08]` was closed in fix-phase session 3 by writing **31** where **28** had stood, and
    `Context::present` then arrived with §15 D755's *Show layout grid* work (`5011659`, which added
    `ViewState::layout_grid` in the same breath) without the count moving with it. Twenty-one fields
    became twenty-two; the number beside them did not. **That is the third rot by one mechanism, so
    correcting the number is not the repair** — a figure standing where a derivation belongs is the
    defect, and only the formula survives the next field. ⚠️ **`ViewState` is a third nested struct
    and the formula above does not flatten it**, deliberately: neither the 28 nor the 31 did either,
    so *View*'s five switches have always counted as the one question they hang off. Flattening it
    would be a different number and a defensible one; what is not defensible is two readers deriving
    it two ways from the same sentence.
  Beyond it, **22** `Action`s have no `Item` anywhere in the registry — `Undo`, `Redo`, `Save`, `Open`,
  `NewDocument`, `CloseDocument`, `OpenSettings`, `ZoomIn`, `ZoomOut`, `ZoomReset`, `Nudge`, `Align`,
  `Distribute`, `PlaceImage`, `ChooseTool`, `Escape`, `Enter`, and the pure keyboard-mechanism variants
  beside them (`PasteRelease`, `StepPoint`, `SizeStep`, `OpacityDigit`, `TextStyle`) — which is
  roughly sixteen a palette would actually offer. Plus a home for the verbs no context is a door onto
  at all ("Open Typography panel").
  - ⚠️ **That list said `SaveAs` until 2026-08-31, and there is no such variant** — `input.rs` says so
    in as many words, and says the chord it had is free. **A list of things that are missing is the one
    kind of list nothing can check**: a name that resolves to nothing looks exactly like a name that
    resolves to an `Action` with no `Item`, and no gate reads either. The count moved 20 → 22 in the
    same re-derivation, and **the three additions are the ones a palette most wants** — `NewDocument`,
    `CloseDocument` and `OpenSettings` all arrived with the library (§15 D362–D366, D383) and are app
    verbs with no context menu to reach them from, which is this section's whole premise arriving in
    code while the section slept.
- **A third was listed here until 2026-08-04 and was wrong**: "MCP and Command Mode name the same
  commands instead of maintaining a second vocabulary". Those two should share a vocabulary with each
  other — but not with *this* registry, and not by extending `Action`. `Action` (`input.rs`) is a
  keyboard-intent enum — `Nudge`, `ChooseTool`, `ToggleView`, `Escape` — resolved from a keymap and
  consumed by `dispatch`; growing it to reach the document is the wrong axis. The document vocabulary
  already exists on the other one: `build.rs`'s `align`, `distribute`, `z_order`, `group`, `boolean`,
  `recolor`, `set_opacity_all`, mostly pure `-> Transaction`, headless, and shared with MCP by
  construction because both go through `Document::apply`. The palette may well *call* into that
  registry; it must not absorb it, which is the same separation the paragraph above already argues
  for. Command Mode's side of it is designed in `vm.md`.
- Layer search already exists in the layers panel (`matching_rows`, which includes the ancestors
  needed to locate a hit). ~~The palette should call it, not re-implement matching.~~ **It cannot call
  it, and that is a ~40-line extraction rather than a call** (read 2026-08-21). `matching_rows` is a
  private `fn` on `OndinApp` reading `self.session.doc`; it returns a `HashSet<NodeId>` that
  deliberately **conflates hits with the ancestors shown to locate them**, which is right for a tree
  and useless for a ranked list; it has no ranking at all; and it requires the caller to have
  lowercased the needle already — `layers.rs` does that at its own call site, so a mixed-case needle
  handed straight in silently matches nothing. What the palette should reuse is the *rule* (match on
  name, keep the path to the hit), lifted into a free function over `&Document` that returns hits and
  ancestors separately. Not re-implement matching; not call this either.

---

## Later · Parked decisions (non-blocking)

**This was `architecture.md` §14** and moved here on 2026-08-19; that section is now a pointer. Each
of these is a decision nobody has had to make yet, recorded so that the absence is deliberate rather
than an oversight. None blocks v1.

- Binary save format (IO API stays stable; the versioned JSON format ships).
- Vello scene fragment caching granularity (seam in `scene.rs`; tune under load).
- Rich text model (`runs: Vec<(Range, TextStyle)>` migration path reserved — `architecture.md` §5.4).
- History cap / snapshot compaction for huge delete inverses.
- Font embedding in `.ondin` files for non-Google local fonts (document portability) —
  `architecture.md` §5.4a.
- **A transform origin for a multi-selection: one body, or *n* layers each with their own.** Parked
  2026-08-23, and parked because the question has two legitimate answers rather than because nobody has
  got to it — *"both are useful in different scenarios, so picking one is just assuming"*. Read as a
  **group**, a set gets one placeable origin on the union box and turns about it as a rigid body; read
  as **n layers**, the crosshair edits each member's own pivot and the set spins in place. Note what is
  and is not missing: a multi-selection already rotates about a shared point — `Handles.pivot` holds the
  union box's centre for exactly that case and `rotate_selection` turns the set about it — so what is
  absent is a **stored, user-placed** origin for a set, not rotation about one. Meanwhile the toggle is
  shown and dimmed on both Transform panels with the tooltip carrying the reason (§15 D134), and the
  workaround is better than a substitute: **group, set the pivot, rotate, ungroup**, where the group is
  the thing that owns the origin — which is precisely what the group reading would otherwise assert
  silently. *Declined on the way here*: shipping that reading alone and making a placeable origin for a
  set a non-goal, on the grounds that other tools rotate a set as a rigid body. Declined because it is
  still a choice made without evidence. **What unparks it is a real case only one of the two readings
  serves.**
  - ⚠️ **The app has since answered it four times, in both directions, on purpose — and this entry
    still reads as though the question is untouched** (re-derived 2026-08-31). The split is per
    *verb*, and every one of the four says so in its own doc comment: `tools::rotate_selection` and
    `build::flip` take the **rigid-body** reading — each member turns or mirrors about the union, so
    the arrangement sweeps round as one thing, "the only reading that makes sense for a box drawn
    over all of it". `OndinApp::rotate_selection_quarter` and the multi-selection **rotation field**
    take the ***n*-layers** reading — each about its own origin, so the layers spin in place and the
    arrangement is untouched, "a selection has no shared transform origin to turn about, and the one
    the union box would offer is a point off to the side that nobody asked for". So the same two
    layers turn one way from the canvas ring and the other way from the panel, deliberately.
  - **The parked verdict survives that and its reason changes.** What is still absent is exactly what
    this entry says — a *stored, user-placed* origin for a set — and the four verbs need no such
    thing, each deriving its point per gesture. But *"both are useful in different scenarios, so
    picking one is just assuming"* has stopped being a guess: **the code is the evidence, and it went
    both ways.** Which also settles the unparking condition against itself — a real case only one
    reading serves is not what will arrive, because four cases have arrived and they split two-two.
    **What would unpark it now is a user asking for the *stored* origin**, not a case discriminating
    the readings.
- UI toolkit revisit (gpui / Xilem) post-v1 — `architecture.md` §9.1.
- MCP transport hardening (auth, TCP/remote) — local socket + proxy only for now.
- `vello_hybrid` as a third backend if low-end GPU performance demands it.
- **Per-edit cost.** `Document::apply` clones the whole document to stage a transaction, and
  `Resolved::update` bulk-reloads the R-tree. Both are O(nodes) per edit with small constants.
  ~~**The clone half has rotted into O(nodes + total embedded image bytes)**, and images are what did
  it.~~ **Measured and fixed 2026-08-22 (§15 D301)** — and the measurement is worth keeping, because
  this entry guessed the order of magnitude and was right: in release, one nudge cost **3.5 ms at 20 MB
  of images and 10.2 ms at 60 MB**, against 0.001 ms after, on every commit and every undo and redo.
  `ImageSource::Embedded` holds an `Arc<[u8]>`, so the clone is a refcount bump; it was the surgical
  fix rather than the staged-diff rewrite it is filed beside, exactly as this entry predicted. *The
  premise it gave was not quite the right one*: the table is not append-only — `RemoveImage` exists so
  a purge can be undone — and what makes the sharing safe is that the bytes are **immutable**, which is
  a weaker and more durable claim. The R-tree half is unchanged and still becomes worth fixing at the
  same time the preview path stops cloning (D1).
  - ⚠️ **A third term, and it is the binding one for a bulk create: naming is Θ(n²) per parent**
    (measured 2026-09-07, §15 D446). `op_create` resolves an unnamed node's name through
    `Document::child_names(parent, &[])`, which walks every sibling into a fresh set once per child,
    so applying *n* children under one parent costs *n²*. On a 32,000-shape SVG paste that is **62.6 s
    of `Document::apply` against 38 ms of import**, on the egui UI thread — 1,600×, and the codebase
    review had attributed the whole cost to the importer's own quadratic index, which was real,
    fixed, and not the binding term. **What makes this a decision rather than a bug** is that the
    chokepoint *is* §5.7b's per-parent numbering (§15 D127): the name has to be resolved against the
    siblings, so the fix is either an incremental name index carried through the transaction or an
    admission that a bulk create numbers differently from a click. Nobody has picked. It bites on
    paste and import only — every interactive route creates one node at a time.
- **PNG goldens.** §11's SVG and JSON goldens were built on 2026-08-22 and the SIMD level is pinned
  (§15 D304); the raster third is the remainder, and it is **blocked on nothing but a decision about
  architectures**. `cpu.rs`'s `export_settings` pins `Level::baseline()`, which is target-derived —
  `Fallback` on default x86_64, `Neon` on aarch64 — so a checked-in PNG is a golden for the
  architecture that generated it. **aarch64/NEON is untested**: the measurement behind D304 is x86
  only, so whether `Neon` agrees with `Fallback` is unknown, and it is the question that decides
  whether one file can serve both machines or whether `tests/goldens/` grows a per-target
  subdirectory. That is a measurement on a real Apple-silicon machine, not a line of code. Everything
  else the raster half needed is already true: the level is pinned export-only (`rasterize_glyph_run`
  still detects, for the font picker's preview strips), and the harness — fixture,
  `ONDIN_UPDATE_GOLDENS`, `.gitattributes -text` — is written and in use by the other two formats.
  Decided against and not to be re-argued: adding
  `f32_pipeline` for `OptimizeQuality`, and keeping strokes out of the fixture to sidestep the one
  level-variant primitive.
  ⚠️ **There is a second axis, measured 2026-09-03: a raster golden is per-architecture *and*
  per-`vello_cpu`** (§15 D414). The `=0.0.9` → `=0.2.0` bump moved 162 bytes over 78 pixels of the
  fixture, all of them on one mitered outside stroke — the same primitive D304 found moving across
  SIMD levels. It does not block anything, a pinned dependency being a pin where a detected CPU is
  not, and `goldens.rs`'s regeneration note already carries it — **regenerate, then check the residual
  is confined to strokes at a worst delta of 1, and treat a residual anywhere else as the finding**.
  So the raster half inherits an acceptance criterion for the next bump, and what it still waits on is
  the aarch64 measurement above and nothing else.
  - ⚠️ **`num_threads` is a *default*, not a pin, and it is the last one left.** `cpu::export_settings`
    names one field — `level: Level::baseline()` — and takes `..Default::default()` for the rest,
    which at `=0.2.0` is that one field; `cpu::raster_settings` spells out all four of the other
    struct's. **This warning named `RenderMode` too until 2026-09-03, when the bump it was about was
    taken, and it was right and wrong in an instructive way** (§15 D414): the difference did show up
    at the version bump, but not as a changed default — the field moved to another struct, so
    `..Default::default()` went on compiling and went on meaning something else, and three knobs
    nobody had named arrived beside it. Five of the six settings are pinned now. Still not worth
    fixing `num_threads` before the goldens exist — it is 0 without the `multithreading` feature —
    but the third format should be written against the settings that are actually pinned.
- **The layers panel's `MIN_W` now rests on one reading where its own prose claims two.** Opened
  2026-09-07 by §15 D444, which corrected both width constants: they subtracted the *header*'s
  `2 * PAD_X` where a row is narrowed by the `ScrollArea`'s own `TREE_PAD_X`, half of it. `MIN_W = 210`
  was argued as *"where two independent readings of it meet"* — the room a **hovered top-level** row's
  name needs against real layer names, and the width at which a **four-deep** row's hovered name room
  stops being negative, below which a nested name is drawn underneath the eye. Corrected, that second
  crossing is **194.5** and not the 206.5 the sentence rested on, so it would have allowed 195 and the
  two readings no longer meet. **Nothing on screen is wrong today**: the error was wholly in the safe
  direction, 210 is more generous than it was argued to be, and D444 deliberately left it alone on the
  grounds that a number which looks right on screen is not a derivation's to overrule. What is open is
  a **decision, not a defect** — re-tighten toward the corrected crossing, or rule that 210 stands on
  the first reading alone and amend the constant's prose so it stops claiming a second. Either answer
  is one constant and one paragraph: `panel_width_tests`' band test is already anchored on the measured
  194.5 rather than on the `MIN_W − 10` stand-in it used to carry. **Unparked by the maintainer's eye
  on the panel at its minimum**, not by any further measurement — the arithmetic is settled.
- **A chrome hairline whose *base* is a decision nobody has made.** Opened 2026-09-07 by §15 D479,
  which unified the three chrome rules' device-grid snap and deliberately left two neighbours alone.
  `rulers::snap_across_axis` rounds for **both** parities, so a guide's odd case is a nearest pixel
  *edge* with half a device pixel added — up to a whole device pixel from where the user put the guide.
  Nothing has ever made that visible, a guide having no column to leave, and moving guide ink is a
  visible change to the one piece of this arithmetic that *is* pinned by tests
  (`rulers::guide_geometry_tests`). The answer is one line and one paragraph; what it should not get is
  a silent "unification" behind a merge, D479 having measured that the two bases differ by up to a
  whole device pixel. ⚠️ **The blast radius doubled on 2026-09-07**: §15 D484 replaced
  `grid::draw_pixel_grid`'s hand-inlined copy with a call, so `snap_across_axis` now places the pixel
  grid as well as the guides and whichever answer this gets moves both. That is an argument for deciding
  it rather than against — the copy was right only by accident of the grid's one-device-pixel width —
  but the paragraph that lands it owes the grid a sentence too. ⚠️ **The *width* half of this entry is
  closed**: `ui::menu_sep` painted one device pixel where its own doc and §9.4 both said 1pt, and it is
  a nominal point snapped to whole device pixels since 2026-09-10 (§15 D721), which is a **different
  question** from this one — that was a width disagreeing with its own record, this is a base nobody
  has ruled on.

---

## Later · Post-v1

Designed but deliberately outside v1. Each has a home already; this section exists so the shape of
what comes after is visible from one place, not so the design gets restated.

- **MCP and headless (`M6`)** — `architecture.md` §8 and §10. Deferred until the editor is
  feature-complete, on the argument that an MCP surface over a half-built editor encodes the gaps
  into a tool schema that then has to be versioned. `ondin-mcp` is a stub: message shapes and the
  tool list, no handlers, no socket. **Three things are queued behind it** and whoever builds the
  span emitter should do all three: per-run colour, the list marker and its nesting level (§15 D169,
  D172), and the missing-font warning under *Now · Doc drift*.
  ⚠️ **One thing has to happen *before* the first tool handler rather than behind it** (§15 **D786**,
  `[A2-L7-01]`): the headless half of `tools/mod.rs` and `preview.rs` moves to `ondin-core`.
  `ondin-app` has no lib target, so nothing outside the binary can link `resize_box_to`,
  `rotate_node`, `pen_anchors` or the point verbs, and §3's graph puts `ondin-mcp` below it — so a
  handler for `set_geometry` either moves that arithmetic or writes a second copy of it. **The
  trigger is the handler, not M6**, and the reason for the order is that before it the move is a
  compiler-enumerated sweep and after it two implementations have to be reconciled. D786 has the
  boundary (everything taking only `Document`/`Resolved`/kurbo types moves; `Drag`, `PenState`,
  `TextSession` and `CaretBlink` stay).
- **Command Mode** — `architecture.md` §13 for the vision and the four settled decisions, `vm.md`
  for the language. The seams v1 must reserve are all cheap and all listed in §13; none is
  outstanding.
- **Multiplayer** — `architecture.md` §12. Nothing is being built; four properties of the model keep
  it possible, and they are already true.
- **The command palette** — the section above. Post-v1 in ambition, but `menu.rs` is already the
  registry it wants, which is why it is worth reading before a second one is written.
