# Ondin — the scripting VM

> **Status: design, not built.** Nothing here exists in code. This document is the working design
> for the language behind Command Mode (`architecture.md` §13), kept separate from `architecture.md`
> so it can churn without disturbing a document that describes shipped code. When a decision here
> hardens and something is built on it, its record moves to `architecture.md` — this file is not a
> second §15.
>
> §13 stands. It already decides one-shot evaluation, world-space property projection,
> selection-relative addressing and explicit modality, and this document does not reopen any of
> them. What it decides is the layer §13 explicitly deferred: *"expression grammar details,
> multi-select semantics in commands, macros/scripting"*.

---

## 1. The thesis

Command Mode should not be a list of hand-written commands with a bespoke parser behind each. It
should be a real, sandboxed language, exposed two ways: as single lines in the command bar, and as
whole scripts. One vocabulary serves three consumers — the operator typing at speed, the user
automating a repetitive build, and an agent writing edits over MCP.

The argument for a language rather than a command list is that the third consumer changes the
economics. An agent constructing a 6x4 card grid in JSON emits twenty-four objects; in a language it
emits a loop. Arithmetic, iteration and relative positioning are in-band instead of being unrolled
into a data structure that the model has to keep consistent by hand.

The argument that this is cheap is the codebase itself:

- **Invariant 2** already forces every mutation through `Document::apply(Transaction)`. There is no
  second path to bind, and no way for a script to reach around the one that exists.
- **`ondin-core/src/build.rs`** is already the intent layer — 43 public functions, most of them pure
  `(&Document, &Resolved, ...) -> Transaction`. `align`, `distribute`, `z_order`, `group`, `boolean`,
  `flatten`, `recolor`, `set_opacity_all`. That is the verb list, already written, already headless,
  already shared with MCP by construction.
- **`NodeId` is stable and never reused** (`id.rs`), with a textual wire form `"<actor-hex>:<seq>"`.
  Script handles therefore cannot suffer the invalidation problem that plagues plugin APIs whose ids
  are indices or generations. A stale handle names a node that no longer exists, which is a clean
  error rather than a silent aliasing bug.
- **Most builders already take `&[NodeId]`**, and `PaintShown::Mixed` / `shared_corner_radius` /
  `inspector::shared_opacity_shown` already define what a property *read* means across several nodes.
  Multi-select semantics are not something this design has to invent; they are something it has to
  expose. ⚠️ **This named `build::shared_opacity` until §15 D607 deleted it** — it had zero callers
  anywhere in the tree, tests included, so the answer this file was pointing at was one nothing asked
  for. The live one is the panel's, and the agreement tolerance is `build::OPACITY_AGREEMENT`.

So the VM is a front end. The expensive part of "anything the UI can do" is not the language — see §9.

## 2. Decision: Luau

**Luau, via `mlua`'s `luau` feature. Not Lua 5.4, not Rhai.**

The deciding argument is not the sandbox. It is `+=`.

§13's syntax is `.x += 15`. Lua 5.4 has no compound assignment operators at all, so choosing Lua 5.4
means either abandoning the single most valuable terse form in a command bar, or building a rewriter
that makes the bar's language differ from the script language in a load-bearing way. Luau has
[`+=`, `-=`, `*=`, `/=`, `//=`, `%=`, `^=` and `..=`](https://rfcs.luau.org/syntax-compound-assignment.html).

The rest is corroboration rather than reason:

- `Lua::sandbox` in `mlua` is Luau-only. It makes the global table read-only — no assignment, no
  `rawset`, no `setmetatable` — so globals cannot be monkey-patched in place.
- Luau's [interrupt handler](https://luau.org/sandbox/) is guaranteed to be called at function calls
  and loop iterations. That is the kill switch for `while true do end`, which is the failure mode that
  actually matters when the script runs on the UI thread.
- Luau's standard library already omits `io`, `package` and `require`, and reduces `os` and `debug` to
  a few pure functions, so the removal list is short rather than a security review.
- Optional type annotations mean the exposed API can be *typed*, which gives an agent a checkable
  contract instead of runtime discovery.

Rhai is rejected despite being pure Rust with no C toolchain: the whole LLM-ergonomics argument rests
on the model having seen millions of lines of the language, and it has not seen Rhai. Choosing a
language an agent writes badly discards the reason for doing this.

**Costs to accept.** Luau is C++, so the workspace gains a C++ toolchain requirement (MSVC on
Windows is fine). **This is the one thing here that will need a change to `architecture.md` the day
`mlua` is added rather than the day it is designed**: §2's stack table lists the dependency set and
its constraints, and a C++ build requirement belongs there beside the interop pins. It is deliberately
not there yet, because there is no dependency yet. And Luau is a dialect: an agent that emits Lua 5.4
mostly works, but `goto` is absent and a few library corners differ. Verify the exact standard-library
surface against the pinned `mlua` version rather than trusting this paragraph.

**Known trap to verify before building.** `Lua::sandbox` makes globals read-only, and the resolution
metatable in §4 needs to intercept *unknown global reads*. Those two may conflict — the documented
escape is that globals "can only be substituted through `setfenv`", which suggests the resolver
belongs on a per-evaluation environment rather than on `_G`. Confirm this with a throwaway test
before designing around either shape.

## 3. Two tiers, one runtime

**Tier 1** is the terse command grammar. **Tier 2** is Luau. Tier 1 is not a separate language: it is
a small set of textual rewrites that produce Luau, and the bar displays the Luau it produced. Typing
terse commands therefore teaches the scripting language rather than competing with it — the same
trick as Blender's Info log, which shows the Python for whatever you just clicked.

**The sugar belongs to the command bar, not to the language.** A `.luau` script file is Luau with no
rewriting. Rule 2 below would turn every bare word in a script body into a call, and a leading `.`
inside a multi-line function is baffling rather than terse. This is a deliberate asymmetry, and it is
why the bar shows its desugaring.

### The rewrite rules

Applied in order, to one line, in the command bar only.

**Rule 1 — verb call.** If the first token is a bare identifier that names a registry verb, and the
next character is whitespace or end-of-line (not `.`, `(`, `[`, `=`, or an operator), the line is a
call. Remaining whitespace-separated tokens become arguments: a numeric literal stays a number,
`true`/`false`/`nil` stay themselves, a quoted string stays a string, **and every other token becomes
a string literal**.

```
group                ->  group()
align left           ->  align("left")
dup 3                ->  dup(3)
rename "My Frame"    ->  rename("My Frame")
```

An argument that needs to be an expression uses the call form, which is already Luau and passes
through untouched: `align("left", n)`. This is shell argument semantics, and its one sharp edge —
`rename my frame` becomes two arguments — is the edge every shell has.

**Rule 2 — subject elision.** A `.` in *prefix position* (line start, or immediately following `=`,
an operator, `(`, `,`, or whitespace after one of those) expands to `sel`.

```
.width = parent.width - 20    ->  sel.width = parent.width - 20
sk.x = .x + 5                 ->  sk.x = sel.x + 5
```

The `.` in `sk.x` is not in prefix position and is left alone. This is what makes rule 2 a rewrite
rather than a parser.

**Rule 3 — compound assignment is per-node relative.** `<expr>.<prop> <op>= <rhs>` rewrites to a
per-node adjust rather than a read-modify-write:

```
.x += 15    ->  __adjust(sel, "x", function(v) return v + (15) end)
```

**This is the one place tier 1 is not a pure rewrite, and it is deliberate.** Luau's `+=` is syntax,
not a metamethod, so `sel.x += 15` would compile to a read of `sel.x` followed by a write — and
reading a property across a mixed selection raises (§5). A designer nudging three layers by 15 expects
all three to move by 15, not an error. Rule 3 buys that, at the cost of `.x += 15` in the bar meaning
something a script's `sel.x += 15` does not. See the open question in §13.

**Rule 4 — everything else is Luau, verbatim.** There is no third grammar.

## 4. Name resolution

Lua's `__index` on the evaluation environment *is* §13 decision 3. An unknown identifier resolves in
this order:

1. **`sel`** — the selection, and the target of rule 2's elision. `selection` is an alias.
2. **Relative references** — `parent`, `prev`, `next`, `children`.
3. **Leap-label handles** — the hint labels §13 specifies, bound to `NodeId`s.
4. **Registry verbs** — the bound subset of `build.rs` (§6).
5. **The permitted Luau standard library** — `math`, `string`, `table`, `ipairs`, `pairs`, `select`,
   `tonumber`, `tostring`, `type`, `assert`, `error`, `print` (routed to the bar, not stdout).
6. **Layer name, disambiguated.** Last, and it *raises on ambiguity* rather than picking one.
   `naming.rs` deliberately provides only courtesy numbering and states that uniqueness is never
   load-bearing, so this must never be the primary mechanism — exactly as §13 decision 3 says. No
   name-to-`NodeId` resolver exists today; it is new work this design implies.
7. **Otherwise, raise** — naming the identifier.

Step 7 is not a detail. Lua's default is that an unknown global reads as `nil`, which turns a typo
into a silent no-op three lines later. For an agent writing scripts unattended, a raised error naming
the identifier is the difference between a fixable failure and a wrong document. **Strict globals are
mandatory.**

**Leap labels bind for the session, not for the frame.** A label minted when the overlay was last
shown stays bound afterwards. This is safe precisely because `NodeId`s are never reused: a label
pointing at a deleted node raises "no such node", and can never silently point at a different one.

## 5. The subject, and mixed values

**`sel` is always a collection**, even when one node is selected. Making it a collection unconditionally
avoids a language where every line behaves differently at a selection size of two, and it matches the
API underneath: most of `build.rs` already takes `&[NodeId]`.

- `sel.x = 10` **broadcasts** — every selected node gets 10.
- `sel.x` **reads the shared value, and raises if the nodes disagree.** This is
  `shared_opacity_shown`'s `Option<f32>` and `PaintShown::Mixed` promoted into the language. Core
  already decides what "the selection's fill" means when the selection disagrees, and that decision
  has been argued once already, in the inspector; the VM adopts it rather than inventing a second
  answer. ⚠️ **"Disagree" is a *tolerance*, not `==`** (§15 D607): `build::OPACITY_AGREEMENT` and
  `build::RADIUS_AGREEMENT`. A VM that raised on a part in a billion would be raising on arithmetic
  the panel it is adopting calls one value.
- `sel:shared("x")` returns the value or `nil` instead of raising, for scripts that want to branch.
- `#sel`, `sel[1]`, and iteration work as expected.

Properties are **world space** per §13 decision 2 — `x`, `y`, `width`, `height`, `rotation`, `opacity`,
`fill`, `stroke` — projected onto local transforms by the facade in core. `place_at_world` and
`move_by_world` in `build.rs` are the existing half of that facade.

## 6. What is bound, and what is not

**Bound:** the `build.rs` builders, the read helpers (`shared_*`, `colors_in`, `paint_targets`,
`outermost`, `subtree_nodes`), `query.rs` (`hit_test`, `bounds`, `local_box`, `group_chain`,
`is_within`), node property get/set through the world-space facade, selection get/set, and creation
(which needs the host's `IdSource`, since `apply` never allocates ids).

**Not bound, by design:** gestures, tools-as-interaction, previews, and egui chrome. A drag with a
live preview has no scriptable form, because the only thing a script can express is the *committed
outcome*. `edit_valve` (`app.rs:3087`) exists precisely to turn a drag into one commit; a script starts
where the valve ends. Anything that touches `RenderOverrides` stays out of the language.

This boundary needs stating in the doc because it is the first thing someone will try to cross.

## 7. Transactions, undo and failure

§13 already settles *one command = one transaction = one undo step*. It says nothing about a script
that performs two hundred mutations, and a naive implementation gives two hundred undo entries.

**A script is one working copy, one commit, one undo step.**

`Document::apply` is already atomic by clone-then-swap (`document.rs:199` builds a working copy). The
VM widens that existing mechanism from one transaction to one script:

1. Clone the document once, at script start.
2. Apply each operation into the working copy as the script runs, accumulating inverses.
3. On success, swap the working copy in and push **one composed inverse** as a single history entry.
4. On error, interrupt, or budget exhaustion, drop the working copy. The real document was never
   touched.

This gets three properties at once that are usually traded against each other:

- **Reads see writes.** A script that positions each child relative to the previous one works, because
  step 2 mutates the copy the script is reading. A buffered "collect all the ops, apply at the end"
  design would silently make `n.x = n.x + 10` twice mean `+10`, which is a footgun that would be hit
  in the first hour.
- **One undo step**, without touching `History`'s run-merging. `commit_into_run`'s `Shape` check
  deliberately refuses to fold anything that is not a plain overwrite, so a script that creates a node
  could never have folded; this path bypasses the question entirely.
- **A timeout cannot leave a half-built document**, which is what makes the step budget in §8 safe to
  set aggressively.

One clone per script, not per statement. `History` needs a new entry point for this; that is the only
change to core's mutation machinery the design requires.

A script should call `end_edit_run` before committing, so it never merges into a preceding
600ms nudge run (`session.rs`, `RUN_WINDOW`).

## 8. Sandbox and limits

**State the threat model honestly, because it changes what is worth building.** A script the user
typed or keeps in their library is their own, and the realistic failure there is a runaway loop
freezing the app, not exfiltration. **Scripts travel since §15 D1100** — in a document, in a kit, as a
shared `.luau` file — so the adversarial model is real, and §10 answers it with two things together:
nothing runs a script but the person, by hand, and every run is inside this sandbox. What is left is a
sandbox escape through a bug in Luau or `mlua` — real, and small, Luau's sandbox being built to run code
its host does not trust.

- `Lua::sandbox` on, globals read-only.
- Removed if present: `io`, `package`, `require`, `dofile`, `loadstring`/`load`, and `debug` beyond
  `debug.traceback`. Luau omits most of these already.
- **Step budget** via `set_interrupt`, plus a wall-clock budget. Exceeding either drops the working
  copy (§7) and reports.
- **Memory limit** via `set_memory_limit`.
- **No filesystem, no network, no clock, no randomness by default.** Determinism is not a nicety here:
  `apply` is replay-deterministic because ids are minted and carried in operations, and that property
  is what future multiplayer needs. A script reaching for `os.time()` breaks it. If randomness is
  wanted later it must be a seeded generator the host supplies.
- **No coroutine yielding into the UI.** A script runs to completion at one defined point in the frame
  — after input resolution, before layout and paint — holding the session. It cannot call egui, cannot
  re-enter the command bar, and cannot observe a partially rendered frame.

## 9. The expensive part is coverage, and the language does not help

"Anything the UI can do" is the ambitious claim, and it is unverifiable by reading. `canvas.rs` is
7,725 lines and `panels/inspector.rs` is 7,395; nobody is going to establish by inspection that every
committable outcome has a scriptable form.

**The cheap proof, available before any VM exists:** every commit already funnels through
`EditorSession::commit` / `commit_run`. Render each committed `Transaction` back into its scriptable
form and log it. Anything the UI can commit that the log cannot express is a visible hole, and the
list of holes is the actual work item. This is testable headlessly, needs no Luau, and is worth doing
first because it sizes the job.

It also produces the Blender Info-log affordance for free: the user clicks something, and the bar
shows the command that would have done it.

## 10. Where scripts live

**A script may be stored in the document; it is never run by the document** (§15 D1100, the
maintainer's ruling of 2026-10-17). This section read *"Never in the `.ondin` file"* until then. The
rule that replaced it is about running, not storage: **no script runs on open, none re-runs on any
change, ever** — the maintainer: *"one would never want to autorun a script."* The two reasons the old
rule gave are both still answered, the first whole:

1. **A script the document runs is a constraint engine wearing a disguise.** §13 decision 1 forbids
   expression syntax growing into "an accidental constraint engine", and the way that happens is not
   through syntax — it is through a script that the document re-runs. Absence enforced this once; the
   no-automatic-run rule enforces it now, and it is the load-bearing half of D1100. ⚠️ **A "re-run when
   the inputs change" convenience is this reason being broken**, however small it looks.
2. Opening a shared document would execute someone else's code. It cannot: nothing runs on open, and a
   run the person starts is inside §8's sandbox — no filesystem, network, clock or randomness, and
   step, wall-clock and memory budgets.

**No warning on open.** The alternative first floated was to warn when a document carrying scripts
opens, offer its source as text, then run. Warnings train people to click *Allow* — Office macros'
*Enable content*, until Microsoft blocked macros in downloaded files by default; Blender ships *Auto
Run Python Scripts* off — and reading source protects few people. With nothing running on open there
is nothing to warn about.

**A document never needed a script to be complete.** A script is a tool whose run commits native
layers (§7), and the document records the result, so an `.ondin` file never needed a reference to an
external script — it was always complete. A stored script is a tool shipped beside the work, not part
of it.

**Three places a script can be:**

- **Unsaved** — typed into the command bar or the editor, run, discarded: tier 1 and tier 2 as §3 has
  them.
- **Library** — saved outside any document, as plain `.luau` files in a folder beside preferences
  (`dirs` is already a dependency), so they can be git-tracked and shared as files.
- **Document** — saved in the `.ondin` file, inert. Listed in the script list marked as coming from this
  document, and run only by hand; **the first time one is run, Ondin shows that it came with the
  document, and its source.** A kit can carry document scripts under the same rule, listed as coming
  from the kit.

**Copies, never links.** Promoting a document script to the library, or putting a library script into
a document, copies it. Components, styles and kits are already three linked-copy systems, and scripts
are not a fourth. A document script and a library script with the same name are both listed, each
with its source; neither shadows the other.

**The editor is in the app** (the maintainer: sending people to Zed or VS Code *"would be weird"*).
egui ships no code editor, so this is built. Its v1: monospace text, line numbers, syntax highlighting,
undo, find, an error placed on its line after a run, and an output area for `print`. **Completion
covers Ondin's API only** — `sel`, node fields, the verbs — not Lua itself, and is generated from the
same definitions as the bindings, so the two cannot drift. Type-aware completion is not promised:
Luau's type checker is a separate C++ analysis library that `mlua` is believed not to expose —
**unverified**. A `.d.luau` definitions file generated from the same source gives an outside editor
completion through luau-lsp at nearly no cost — an extra, not a replacement. Agents are expected to
write most scripts (§11), so the editor's first job is reading, tweaking and running: highlighting and
errors come before completion. Its UI waits on a Claude Design pass.

**Parameters are designed for, not in the first build.** A script may declare its inputs in a header,
and Ondin draws them as a small form when it is run — Blender's operator properties. It is still a run
the person starts, not reactivity. The header's shape is to be settled up front, before the first
build (§13).

Plugins stay out (§12): a script mutates the document, and a plugin story is a different feature with
a different security design.

## 11. Relationship to MCP and `/ai`

`ondin-mcp` has no handler today, and `tools.rs` holds §8.4's planned tools as the constants
`PHASE_1` to `PHASE_4`, checked against it (§15 D726, D1095, D1096) — Phase 1 read and export,
Phase 2 write mapped 1:1 onto `Operation`, Phase 3 components, variables and styles through the
verbs, Phase 4 kits and the library through the app's own actions. Nothing is built. **This is the
reason to decide the VM's role now even though the build stays parked:** committing to Luau `eval`
as the bulk-write surface costs nothing today and costs a deprecation once the write tools of
Phases 2 and 3 ship.

The split that survives contact with an agent:

- **Reads stay typed tools.** The snapshot (`ondin-export/src/snapshot.rs`, `SNAPSHOT_VERSION = 3`) is
  how an agent sees the document, and a language does nothing for that. Keep `get_document`,
  `get_node`, `get_selection`, `hit_test`, `get_bounds`.
- **Single edits stay typed tools**, because a typed call gives validated arguments and per-call error
  attribution. A forty-line script that fails on line 31 leaves an agent diagnosing state instead of
  reading an error.
- **Bulk and procedural work goes through one `eval` tool.** §13 decision 2 already predicted this:
  the world-space facade "also enables a future MCP `eval` tool for free".

`/ai` gains a property worth naming: the agent emits tier-2 Luau, the operator can *read it before it
runs*, and it lands as one undoable step. Natural language in, reviewable code out, ordinary undo.

## 12. Non-goals

- **Constraints or reactivity.** §13 decision 1, reinforced by §10 above.
- **Scripts run by the document** — on open, on a change, or on any trigger but the person's own.
  §10. *Storing* one in the document is allowed since §15 D1100, which took this line's old form,
  *"Scripts stored in the document"*, out.
- **Links between a document's scripts and the library's.** Copies only (§10, §15 D1100).
- **Plugins and UI extension.** A script mutates the document; it does not add panels, tools or
  chrome. Different feature, different security design.
- **An embedded model.** §13 already says Ondin embeds no model; `/ai` targets a configured provider.
- **Async, coroutines, or anything that yields across frames.** §8.
- **Binding the interaction layer.** §6.

## 13. Open questions

These are the parts worth arguing about before anything is built.

1. **Should rule 3's divergence exist?** `.x += 15` in the bar meaning a per-node adjust, while a
   script's `sel.x += 15` raises on a mixed selection, is the one place the two tiers disagree. The
   alternative is that the bar raises too, and per-node relative edits need `sel:adjust("x", 15)`
   explicitly — consistent, and worse to type. This is a taste call and it is yours.
2. **The verb list and its names.** `build.rs` names are internal (`set_opacity_all`,
   `set_corner_radius_all`). The registry needs a public naming pass, and once an agent has seen those
   names they are expensive to change.
3. **Percentages in expressions.** `.width = 50%` needs a referent — of the parent? of the
   current value? — and until it has one the field parser treats a `%` as the noise a percentage
   field's own formatter writes. **Units are no longer part of this question**: they were decided on
   2026-08-26 and there is one, px (§15 D358), which is what closed `roadmap.md`'s *Units* section
   this line used to point at. The referent is the whole of what is left, it is shared with
   `expr::eval`, and it should be answered once for both.
4. ~~**How full scripts are edited.**~~ **Answered 2026-10-17 (§15 D1100): in the app** — an editor
   built in egui, its v1 scope in §10, its UI waiting on a design pass. Library scripts being plain
   `.luau` files, an outside editor still works, with completion from the generated `.d.luau`.
5. **Whether the API is typed.** Luau type annotations would let an agent's script be checked before
   it runs. This is a meaningful win for unattended agents and meaningful work to maintain. ⚠️ The
   checker is Luau's separate analysis library, which `mlua` is believed not to expose (unverified,
   §10) — so in-app checking is not free; a generated `.d.luau` gives luau-lsp the types either way.
6. **Completion and history in the bar.** Not designed. Interacts with the registry from §9.
7. **`/ai` invocation mechanics** remain deferred exactly as §13 leaves them — provider vs. connected
   client, keys, streaming into the bar.
8. **The parameter header's shape** (§10). Designed for and not in the first build, but to be settled
   up front.
9. **Reaching outside the document — parked** (§15 D1100 (7)). The maintainer's case: one base design
   in twenty colour-and-logo variations, each its own file. That needs writing files and reading
   images, which §8 forbids. The recorded answer today is one document: the script generates the
   frames, or a variable mode per brand with each frame exported. If files are wanted, two shapes are
   on the table and neither is ruled: (a) **host-mediated** — the script returns new documents and
   Ondin saves them into a folder the person picks after the run, and images arrive as data from a
   folder the person picks, never as paths; (b) **a CLI batch run** — `ondin run script.luau --in
   base.ondin --out dir/`, the shell owning the files. It is its own decision because it is the first
   time a script reaches past the document.
10. **The build order.** The session's recommendation, not ruled: §9's cheap proof first — log each
    committed transaction in its scriptable form, sizing the coverage before any Luau exists — then
    the VM with unsaved scripts, then library scripts, then document scripts, the editor growing
    alongside.
11. **Details of document scripts the ruling leaves open** (this file's reading, not ruled): whether
    the first-run disclosure comes back when a document script's text has changed since it was last
    shown; where scripts sit in the `.ondin` schema; and whether a kit's scripts follow the kit's
    updates, as the rest of a kit's content does (§15 D1016), or are copied once like everything else in
    §10.
