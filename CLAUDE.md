# Ondin — working notes for Claude

**The project's documents live in `docs/`.** Only this file is markdown at the root;
`design/` keeps the mockups and their handoff notes, which are assets rather than
documents, and is untracked.

| File | Holds |
| --- | --- |
| `docs/architecture.md` | The design and the invariants. **Source of truth.** ~11,800 lines. |
| `docs/decisions.md` | **§15** — every deviation from that design, **D1–D803**, each with a verdict. ~45,700 lines. |
| `docs/roadmap.md` | Open work, decided non-goals, parked decisions, post-v1. |
| `docs/shortcuts.md` | The whole keymap — bound, unbound and agreed. |
| `docs/context-menus.md` | The context-menu spec and its own deviation ledger. |
| `docs/vm.md` | The language behind Command Mode. Nothing here is built. |
| `todo.md` | **The maintainer's private scratchpad. Untracked.** Not a project record — never write a decision, a gap or a trap there and leave it there. |

**Don't page these in by hand — ask `design-oracle`.** `architecture.md` and
`decisions.md` are ~57,000 lines together, and reading them is what runs a session out
of room. Page a section in only when you need to *edit* it.

## The record

🚨 **Source of truth is not the same as true.** When the code and `architecture.md`
disagree, the question is which one is wrong — and it has been the document. One fix-phase
session corrected three passages asserting things the code had never done, one of them
(§9.4's copy of `ui::menu_place`'s order) in the direction where a reader repairing the
*code* to match the *design* would have introduced the bug.

🚨 **A stated rule can be the thing a repair has to break.** §7 said an effect's filter
region is written *"from `Resolved::ink_bounds`"*, and §5.9 carried a copy. That was the
**reason** an inner shadow exported as nothing: `escape` answers where an effect's ink
*lands*, and a filter region needs where its primitives are *evaluated* — for an inner
shadow those differ. Both passages were true of the code and wrong about the world.
**When a fix requires contradicting `architecture.md`, the §15 entry says which rule
changed. Never quietly leave the sentence standing.**

⚠️ **A passage can be made false by a fix that is then reverted, inside one day.** Session
18 amended §9.5 and D377 on the strength of a one-line repair, reverted the repair four
hours later, and both passages then said things untrue at `HEAD`. **The record is written
from a brief, and a brief is a claim about a commit that may not be the last one.**

**Keep it honest as you work, not afterwards.** Silent drift from the code is the most
damaging kind of bug here — several modules once carried comments asserting features that
were never built, and seven passages plus two code comments asserted byte-compared golden
files that did not exist until they were finally built (§15 D304), at which point every
one of those passages had to be rewritten a *second* time, in the opposite direction.
**Drift costs twice: once when it is written and once when the thing it lied about comes
true.** When an implementation ends up differing from the design, either fix the code or
record it in `docs/decisions.md` with a reason and a keep/fix verdict.

**`docs/roadmap.md` holds open work only.** When something lands, its record moves to §15
— it does not live in both — and **a decided non-goal is never an open item**, because §15
is the thing that stops it being re-argued. Without that rule the file drifts into a
second, worse copy of §15. `arch-scribe` strikes the entry a finished change closes, so
routing the write through it is the cheapest way to keep this true.

### Picking a D-number

**§15 keeps its number even though it moved**, so a comment citing "§15 D123" still
resolves — in `docs/decisions.md`.

**Take the next free number from §15.0's *last index line*, plus one.** Not the entry
count (two numbers, **D476 and D477**, are cited from `crates/` with no entry and have
been for twenty-plus sessions). Not the file's own header prose, which has gone stale
three times and twice *self-contradictory* — one version read *"D689 and D690 are reserved
and unspent, so the next free number is D689"*, both halves in one sentence, while nothing
anywhere cited either. **The live figures: 801 index rows, 801 body headings, next free
D804** — but trust the procedure over any number written down here, including that one.
⚠️ **`decisions.md`'s own header carried that same sentence and it was deleted on 2026-09-19
rather than corrected**, because a file that names its own next free number is a second copy
of §15.0's last index line and it is the copy that rots. This table is the third copy; it
rots too, which is what the sentence above is for.

🚨 **Reserve the *block*, before writing a single citation.** Take the range, run a
negative grep over it from the repository root, write the numbers down in a scratchpad
ledger, *then* spend them. "Pick the next one when you need it" is what produced session
17's collision: D647–D662 went through four batches with a reservation each and none
collided, then D663 was picked ad hoc **between** batches and used for two different
findings an hour apart. **Nothing in this project could have caught it** — both numbers
resolved to entries that did not exist yet, so the doc gate, clippy and the census were
all blind. Only `grep -rn 'D663' crates/` says so.

⚠️ **A number is owed from the moment it is typed into a comment**, and a session's
documentation batch closing does not end its need for numbers — three entries in a row
were spent *after* a closing census, one of them by a session that existed only to repair
one sentence. **There is no such thing as a session too small to need a reservation.**

⚠️ **A record-only entry needs its citation planted by hand.** An entry that changes no
production line is invisible to the census, which is how one spent an hour resolving to
nothing. Put the number on the doc of whatever a reader meets the question at.

### The citation census

```bash
grep -rhoE 'D[0-9]{1,3}\b' crates/ --include=*.rs | sort -u
```

698 distinct numbers today, every one resolving except D476 and D477. Three separate
checks, and each catches something the others cannot:

1. **Set-difference against the previous run.** Never compare totals — a count that moved
   by one is equally consistent with one gained, and with two gained and one silently
   dropped. Name the commit you are differencing against, and difference *the same
   command* on both sides: widening the sieve on one side only once made a manifest-only
   number look like an arrival.
2. **The unresolved half** — `comm -23` of cited-from-code against §15.0's index. This is
   the only check that finds a number **cited with no entry**, which is how D727 and D728
   lived through a session with every gate green after the batch that would have written
   them died to a rate limit.
3. **A per-number `grep -rl`** over the numbers you just spent. The only check that
   catches a number issued twice, and the only one that catches an entry written for code
   that never cited it.

⚠️ **Run it from the repository root over `.rs`, `.toml` and `.wgsl`.** Numbers have been
spent in a member manifest, in the *workspace root* manifest, and in a shader — each
invisible to a `crates/ --include=*.rs` census. 🚨 **And one was spent in a commit
*message*, cited from no line of code at all. A commit message is not a citation.**

⚠️ **Expect the total to disagree with "the entries this session wrote", in both
directions.** Old numbers arrive by at least six doors — citing the entry you are
*extending*, the entry you are *amending*, the entry whose invariant you have just proved
you broke, the entry describing the thing you *kept* (through a deletion), the entry that
*measured* something, and the entry that establishes how long a defect gets to live. And
the total runs *low* when an entry is prose-only with nothing to cite it. No total can be
reasoned forward; only the set-difference says which number arrived and why.

🚨 **A citation that resolves is not a citation that agrees, and nothing here can see the
difference.** Seven comments once cited D475 for an argument that is D425's — D475's own
body says so. A `§` reference is worse than a D-number, because naming the right
neighbourhood reads as having read it: one comment called the SVG filter's effect order
*"§5.3a's question and not this file's"* when §6.4 **decides** it in as many words. **Ask
what the cited entry says, not whether the number exists.**

### Checking §15's integrity

- **§15.0 is in numeric order. The body is not, has never been, and must not be checked
  that way.** Entries are grouped by **subject** — the body opens D110, D111, D112, D113,
  **D309** — and roughly two in three sit out of numeric order, as they did on the file's
  first day. **A numeric-order test pointed at the body is a false-finding generator that
  reports five hundred defects in a file with none**, and the next reader to "repair" it
  would renumber a clustering nobody has seen a reason to abandon. Where a new entry goes
  is the subject's business, not the number's.
- **Order-check §15.0 only**: `grep -oE '^- \*\*D[0-9]+\*\*'` piped through `sort -n` and
  diffed against itself. That found an inversion once, and in fixing it a **missing
  newline** that had joined two index rows into one physical line — invisible to every
  count in this file.
- **Compare the index and the body as *sets*, with `comm`, both ways.** Never as counts.
- **The body-heading regex is `^\*\*D[0-9]+ —`** — with the em dash. A looser
  `^\*\*D[0-9]+ ` also matches prose lines opening `**D344 amended**`, and this file twice
  carried a wrong *count* of those strays inside the very sentence warning against counts.

### `review/` — the codebase review's ledger

Round 1 is complete: 426 findings, 415 fixed across twenty-five sessions. `review/` is
gitignored and **is** the record. 🚨 **`review/findings.md` is 23,761 lines with no
`git checkout` behind it** — the splicing ban below matters more there than anywhere else
in the tree.

Its census has traps, all found the hard way:

- **`export LC_ALL=C` before every `sort` and `comm`.** A UTF-8 collation orders
  `S8.2-L1-01` differently from the byte order `comm` assumes, and silently reports closed
  findings as open. A whole reading was lost to it. Check that `comm -13` comes back empty
  — that is the cheap tell that both sides are ordered alike.
- Round 1's fix log ranks its rows, so the finding id is in the **third** column there and
  the second everywhere else.
- `index.md` has **428 rows for 426 findings** — `[A4-MAP]` and `[S2.1-MATRIX]` are
  reference rows — and the severity column is not spelled uniformly.
- 🚨 **A finding id mentioned in the *body* of a fix-log row is extracted as closed.** The
  only trap here that makes the census read too **high**. Extract `^\| \`\[` as a second
  reading and compare.
- 🚨 **And the mirror: naming a finding in a fix-log table is what closes it, whatever the
  prose beside it says.** A half-finished finding has to be kept out of every `^|` row —
  including out of another row's body.

⚠️ **A finding can be closed by *another* finding's fix, or by a §15 entry written before
it was ranked, and neither leaves a row anywhere.** Eight instances across four sessions.
**Grep §15 for the *mechanism* a finding names, not only for its id, before measuring it**
— and read the site anyway, because a superseded finding is still a pointer at a place
worth reading.

## Editing files

**Edit files with the `Edit` tool. Never splice source programmatically.** No `sed -i`, no
`perl -pi`, no `awk` rewriting a file in place, no heredoc or Python script regenerating
one that already exists — not for a rename, not for "just one line", not because a shell
loop looks faster. Use `Read`/`Edit`/`Write`, which match exact text, fail loudly instead
of half-applying, and cannot truncate a file they were only meant to touch.

🚨 **This rule is paid for.** A `perl -0pi -e` that both looped and slurped its own input
deadlocked, and killing it left a 3,900-line source file at **zero bytes**, unrecoverable.
It was not the first time on this machine.
A stream editor writes the whole file every time it runs, so every mistake it makes is a
whole-file mistake — and the files here are the worst size for that:

```bash
find crates -path '*/src/*' -name '*.rs' | xargs wc -l | awk '$1>1000 && $2!="total"' | wc -l
```

**40** modules over a thousand lines, `inspector.rs` at 24,801 and `canvas.rs` at 21,673,
against `decisions.md`'s 45,676 and `architecture.md`'s 11,881.

⚠️ **On Windows a scripted rewrite also re-decides the line endings.** A three-line Python
`read()`/`replace()`/`write()` used for a *flip-check* — the most tempting case, because
the edit is reverted thirty seconds later — was correct in its replacement and still made
git see all 11,665 lines of `app.rs` as changed, because `open(p, 'w')` translates `\n` to
`os.linesep`, the committed blob is LF, and `core.autocrlf` here is **false**. The real
61-line diff was invisible inside it. **A flip-check is an edit, and the rule covers it.** The cheap tell is
`git diff --stat`: a one-line change that reports thousands is a line-ending rewrite.

🚨 **The tell that you are about to break this rule**: the edit is *mechanical* (a number,
a rename, a counter), the file is *huge*, and `Edit` feels like overkill for something a
regex does in one line. Every recorded instance had all three. `Edit` is not overkill; it
is the only form of the operation that fails loudly.

Reading with `cat`, `sed -n`, `grep` and `find` is fine; it is *writing* that is banned.
The rule binds the subagents too — `arch-scribe`, `rust-mechanic` and `ui-prober` all
carry `Edit` for exactly this reason, and `ui-prober`'s contract that the tree is left
exactly as it found it only holds if the probe comes back out the way it went in.

### Anchor an insertion above, never on the item below it

`Edit` cannot truncate a file, but it will happily put a new item between an existing one
and its doc comment — and the result compiles, tests, lints, formats and passes
`cargo doc`, while paragraphs of reasoning now describe the wrong thing.

**There are twenty-two recorded instances.** Twelve were committed by a session that spent
the day fixing this exact class, and one was committed by the session that ran *two*
whole-tree sweeps for it, between them. **Writing the trap down does not prevent the
trap** — only the two habits do:

1. **Anchor on the line above**, and know that "the line above the `fn`" is not the same
   instruction: `#[derive]`, `#[allow]`, `#[test]` and `#[cfg]` all sit between an item
   and its doc, and every one of them is an anchor somebody will reach for.
2. **Read the result of the `Edit`**, immediately, with the item's own keyword:
   `grep -n -B3 "^\(pub \)\?\(fn\|struct\|enum\|const\|type\) <neighbour>"`.

⚠️ **The unique anchor and the correct anchor are different lines**, which is why this
keeps failing: a test's `fn` name is the unique string, its `#[test]` and its doc are not.

Three shapes worth expecting, because the obvious detectors miss all of them:

- **A swap, not a theft.** Anchoring a whole documented `#[test]` on the next test's `fn`
  line exchanges the two doc comments. *Nothing lost a doc*, so "did the item below lose
  its comment" answers no.
- **A duplication.** A flip reverted by *re-adding* the block beside the copy that
  survived leaves a 21-line comment twice, byte-identical. Valid Rust describing the right
  item, so no gate objects. **Revert with the inverse of the edit, and `git diff` the file
  against `HEAD` before committing** — that is the only thing on this list that catches it.
- **A stolen doc is invisible to the person changing the thing it describes.**
  `resize_tx`'s four-case paragraph was sitting on the item below it, and ended by
  asserting behaviour §15 D478 had just made false. The last place in the tree still
  asserting the old rule was a comment *on the wrong function*, and every sweep that would
  have caught it looks at the function the comment describes.

⚠️ **`#[test]` is a coincidence, not a safety net.** One instance was caught only because
the displaced `#[test]` landed on the new doc and became a `duplicate_macro_attributes`
warning. One line lower and every gate would have been green.

🚨 **In *prose* the same rule has the opposite sign.** A doc comment **precedes** its item,
so an insertion below one steals it. An antecedent **precedes** its pronoun, so an
insertion placed *above* — the direction the rule prescribes — orphans a reference below
it. `architecture.md` §5.6 read *"… hands text to `tools::scaled_text_sizing` … **That
function** scales each axis exactly when the user authors it"*; a ⚠️ inserted between the
two ended its last clause on `tools::railed_resize`, and the sentence has credited that
function with a rule it has no line of ever since — **without one character of it being
edited** (§15 D790). **A pronoun is a citation with no name in it**: `cargo doc` resolves
links, the census resolves `D<n>`, and *nothing* resolves "That function". **Name the
subject in any sentence a later insertion could come between.**

### The whole-tree sweep: a `///`-run length ranking

The neighbour grep only helps where you know you inserted; the damage is silent everywhere
else and it *piles up* — `crop_exit_pill` once carried **three** functions' doc comments in
one 79-line block, both victims with none, and nothing anywhere disagreed. What made it
findable is that a merged block is an **outlier in length**.

Measure every contiguous run of `///` lines over **`crates/`** — not `crates/*/src/`, which
misses the integration tests where some of the longest prose lives — sort descending, print
the item each run precedes, and read the first twenty. **A run whose first line does not
describe the item under it is the bug.**

⚠️ **Ignore every run starting at line 1**: those are module docs and precede nothing.
`boolean.rs` tops the raw ranking at 116 lines and is correct.

Four rules this heuristic earned the hard way:

- **An entry joins the excused list only after its run has been read line by line against
  the item beneath it.** `build::mask_target` sat on the list for eight clean re-runs as a
  54-line entry; 27 of those lines were `pub fn mask`'s. **Being on the list is what hid
  it**, and comparing cannot catch that.
- **Print the head and read it against the whole list, entry by entry.** The baseline has
  been found short **eight times**, and three of those were sessions that had *grown the
  run themselves* and then reported "nothing joined". A sweep that greps for the names it
  remembers will confirm a baseline it has just invalidated.
- **A ranking that gains an entry has not necessarily lost one.** The head has grown from
  five entries to twenty-three while the floor sat at 52 — so the floor is not a threshold.
  The *tell* is the first line; length is only how such a run comes to be looked at.
- **A clean reading says nothing about the edit you just made.** One sweep came back
  perfectly clean over a session that had committed a theft the whole time — the merged run
  was 30 lines against a 52-line floor. This finds *accumulated* damage. For your own edit,
  the neighbour grep is the only thing that works.

The generalised form of that grep needs no list of insertion sites: one `awk` for *an item
declaration whose preceding line is a `///` whose own preceding line is neither a doc line,
an attribute, a blank, nor a brace*. Its only known false positives are one-line `const`
docs sitting under the previous `const`.

⚠️ **A second detector was built for the direction the ranking cannot see and is too
imprecise to keep** — "an undocumented item below a documented one" yields ~40 candidates
in `crates/` and the ordinary case is all of them. Recorded so it is not re-invented. **The
enum-variant version does work**: check each variant's run against the variant beneath it,
since a variant's doc almost always opens by naming its verb. Expect about a dozen reads
and no defects — every current hit is a doc legitimately citing a sibling for comparison.

### After deleting or renaming anything

**Grep the old name.** `cargo doc` will not tell you: most of this project's prose is
inside `#[cfg(test)]` modules, which that gate cannot see at all (§15 D319). Deleting one
function left three `[`link`]`s pointing at nothing with every gate green.

## Git

**Git is history now, and it is published.** The repository lives at
`https://github.com/fadion/ondin`, it is the record, and it is read by people who were not
in the session that wrote it. Commits are grouped on purpose and messages are written to be
read later.

🚨 **This reverses the rule that stood until 2026-09-16**, which read *"Git is here as a
safety net, not as history … commit freely and without asking … how commits are grouped and
what the messages say does not matter"*. That was correct for a repository due to be
deleted and started again. It was — `.git` was reset on 2026-09-16 — and what replaced it
is tracked properly. **If you find a passage anywhere in this project that licenses a
checkpoint commit, it is stale, and it is worth correcting rather than working around.**

- **Commit when the user asks.** Invoking the `commit` skill is the ask; finishing a task
  is not. Leave the work in the tree and say it is ready.
- **One logical change per commit**, Conventional Commits: `type(scope): subject`. The
  `commit` skill carries the format and the pre-commit bar and is the authority.
- **No attribution trailer.** No `Co-Authored-By`, no "Generated with Claude Code".
- **Version bumps are explicit-only** and belong to the `release` skill.

**The safety net is now the working tree plus the edit tools, not the commit.** The `Edit`
tool is what stops a write from destroying a file, and a session's uncommitted work is
meant to *stay* uncommitted until it is one coherent change.

🚨 **`git checkout <file>` is that net pointed the wrong way, and a session lost work to
it.** A stray block had been appended to a file that also held 200 lines of good
**uncommitted** work; `git checkout` threw away both, because it does not know which half
you meant. **Undo an edit with an edit.** Keep `git checkout` for a file you are willing to
lose *whole*, and **copy a file to the scratchpad before an experiment you expect to
revert** — one `cp`, and it leaves no commit behind.

🚨 **Never `git add -A` while a subagent is running.** `arch-scribe` writes `docs/` *and*
corrects `crates/` comments when the brief invites it to — the single most valuable thing
it does — so a sweeping stage picks up its half-finished work under an unrelated message.
This happened four times across two sessions. It used to cost nothing because the history
was disposable; it now produces a permanent, published commit that misdescribes itself.
⚠️ **`git add crates/` is not the mitigation** — that is the fix that failed, three times
in one day. **Stage the specific files you changed**, or wait for the agent.

## Delegate the reading (`.claude/agents/`)

The files here are large enough that paging them into the main context is what runs a
session out of room. Five subagents exist to do that reading in their own windows:

- **`design-oracle`** — what does the design say about X? Read-only, answers with `§`
  citations and verbatim quotes for rules. **Use this instead of reading
  `docs/architecture.md` or `docs/decisions.md` yourself**; page in a section by hand only
  when you need to edit it or the oracle's citation is not enough.
- **`arch-scribe`** — makes the `architecture.md` and `decisions.md` edits a finished
  change requires, in the document's own voice, **and strikes the `roadmap.md` entry that
  change closes**. See below; it is not a formality.
- **`rust-verify`** — runs check/test/clippy/fmt and returns a distilled report instead of
  pages of cargo output.
- **`rust-mechanic`** — propagates a *decided* change across the workspace (a new field
  through every construction site, a signature through every caller). Hands back anything
  needing judgement rather than guessing.
- **`ui-prober`** — answers a chrome question that is really about numbers, by the
  throwaway-`#[test]` technique below. Writes the probe, runs it, reports the measurements,
  removes it. ⚠️ Expensive (60–100k tokens); for a single number, write the test inline.

`Explore` (built in) is the right agent for "where is X?" sweeps across many files.
Editing a module you are actively designing in still belongs in the main loop: the comment
standard here is too specific to delegate.

### Briefing `arch-scribe`

**Eleven sessions running, it has returned corrections from every batch.** Budget for
acting on them. What it catches that nothing else can:

- **Factual errors in comments the session shipped an hour earlier**, usually *a number
  copied out of a finding*. `app::InFlight`'s doc claimed a *"sixteenth field"* over a
  struct with **seven** — sixteen was a finding's count over a *different* population. 🚨
  **A finding's numbers are a measurement of the day it was written**, and a review that
  ran for weeks has numbers a year apart in it. Re-measure, or say whose they are.
- **A brief citing the wrong §15 entry for a rule** — D114 for a truncation rule that is
  D118's; D269 for a reachability argument that is D303's. Both numbers resolve, so no gate
  could ever see it.
- **A live bug**, by reading a builder against the placement rule the brief described
  (`insert_subtrees` advancing a per-parent count where the indices no longer ascended);
  and `effect::escaped`'s squared ellipse mapping, and the same fault in `stack_escape`
  running the *opposite* direction — which fails in the worse way, reporting a blur's
  extent as a bare rectangle so the walk culls artwork that is on screen.
- **A fix quietly widening a departure from a live §15 entry.** A brief called adding a
  dash to one more field a *consistency* repair — true of the panel, false of the record,
  since D130 is *Resolved* and names *"exactly two"* dashes in the whole app. **Nothing in
  this project can see a conflict between a change and a decision.**
- **A premise that turns out to be a *fixed* finding.** Told a route was reachable
  "because the recovery card does not own the keyboard", it read the cited finding and
  found D464 had fixed it — and on the route that is actually live, the proposed one-line
  repair deletes the user's work. **A citation to a fixed finding is more dangerous than
  one that dangles, because it still resolves.**
- **A comment right in its verdict and wrong in its *argument*.** Nothing can gate that:
  the code compiles, the tests pass, the links resolve, and the next reader reorders
  something on the strength of a sentence that was never true.

⚠️ **So brief it on the *premises* and the *citations*, not only the change** — most of
those came back because it went and read the entry. Give it the previous batch's own output
to check, too: it caught itself amending §9.5 for a fix that was later reverted.

⚠️ **It will decline to write a number rather than write a wrong one**, and it refuses
ordinals ("the eighth doc-comment theft") on the ground that a position in a tally is a
count in prose. ⚠️ **It has no shell in some configurations**, so any *count* it reports is
carried forward rather than measured. ⚠️ **And it will not edit `CLAUDE.md`**, on the
ground that an agent's message is not authorization to change the project's instructions —
which is correct, and makes this file a job the delegating session owes at the close of its
own.

⚠️ **Batch the work rather than saving it up.** Each block is reserved against a
`decisions.md` that already holds the previous one, the census closes at every boundary,
and corrections arrive early enough to fix the *code* rather than only the record.

🚨 **And it can die mid-job, leaving the record claiming work it did not do.** A rate limit
stopped one run after five entries and four index rows asserting `architecture.md`
amendments it never reached. Nothing catches that — the entries read as prose and the
census counts them. **After a subagent stops early, diff what it claimed against what it
wrote**; `git diff --stat docs/` is the whole check.

## Running the app for testing

**Do NOT launch the GUI (`cargo run -p ondin-app` / `target/debug/ondin.exe`) as a routine
check.** Launching opens a native window that grabs focus and the mouse/keyboard, which
hijacks whatever the user is doing at the time.

- Prefer `cargo check`/`cargo build`/`cargo test` and code reasoning to verify work.
- Only launch when genuinely necessary — a complex UI feature or multi-step interactive
  debugging where a screenshot is the only way to confirm. Say so and why first.
- Otherwise, **ask the user to run and verify**, and describe what to look for.

### Checking rendering without a window

`ondin export` runs the whole document → pixels/markup path headlessly (no GPU, no
window), so rendering questions usually do not need the GUI at all:

```bash
cargo run -p ondin-app -- export path/to/doc.ondin --svg -o out.svg
```

`--svg` (default) is the most readable — it shows structure, transforms, clipping and
gradients as text. `--png` rasterizes through the CPU backend that shares the exact scene
walk the GPU canvas uses, and `--json` emits the MCP snapshot. Canvas *interaction*
(gestures, overlays, panels) still needs a human.

#### Checking an *import* against a real browser

`svg_in::import` → apply to a `Document` → `ondin_export::svg::svg_of` gives markup you can
put beside the source file and compare, by eye and by pixel. This is how §15 D394's later
amendments found nine defects that every green test and the round-trip suite agreed were
not there: **both sides of a hand-written fixture agree about what SVG looks like**, so
only a real file disagrees with you.

⚠️ **Serve it over HTTP, do not open the file.** The Browser pane renders a `file://` page
as a static snapshot: its own `<script>` never runs, relative `<img src>` does not resolve,
and there is a size cap somewhere under 650 KB. `.claude/launch.json` carries a
`probe-static` entry (`python -m http.server` over `target/probe`) for exactly this. The
pane also caps open tabs at nine, and the symptom is `navigate` refusing a path that
exists; `tabs_context` then `tabs_close` clears it.

Two things make the comparison *say* something rather than just look like something:

- **A coarse mean-error grid over the frame names the regions that differ** — 20×12 cells
  of mean absolute channel error, printed as ASCII. That turns "the gradients look off"
  into a list of places, and every one of them turned out to have a distinct cause.
- **Attribute the residual, do not assume it.** Repaint the shapes you *believe* are
  responsible in some impossible colour, render that file too, and overlay its coverage on
  the error grid. It has confirmed an attribution twice and would catch a wrong one.

Mind the export's own `viewBox` offset when placing the two on one canvas, or you diff a
shift.

### Checking the *chrome* without a window

`export` covers the document, not the egui UI. For chrome — a widget's geometry, a cursor
bitmap, a shape's tessellation — the trick is a **throwaway `#[test]` that prints the
pixels or vertices as text**, run with `-- --nocapture`. egui is usable headlessly:
`egui::Context::default()`, `theme::install(&ctx)`, then one
`ctx.run_ui(Default::default(), |_| {})` to make fonts available.

**And the app itself is buildable — `OndinApp::headless(&ctx)`** (§15 D303), which swaps
three things for inert ones: the wgpu device, `FontService`'s five background threads (one
of them a *network* fetch) and the preferences file. Anything the app *decides* — tool
routing, the cursor rules, autopan, `resize_tx`, the line chrome — is reachable from a test.

Two things this does *not* change. **No test can say where a `&mut self` method is called
from** — a headless app makes the method callable, not the call site observable, which is
the whole story of §15 D269 and why lifting a decision into a free function is still the
better shape. And there are still no pixels.

It settles the questions that are actually about numbers, and has caught things review
missed:

- Feeding a shape to `epaint::Tessellator` and measuring the output vertices proved the
  colour-swatch corner mask was throwing spikes 6px outside a 14px chip — a bug that reads
  as "some weird lines" and is invisible in code.
- Printing a cursor bitmap as ASCII art confirmed the hand and pen glyphs were the right
  icons, the right way up, with the hotspot on the nib.
- Looping the same dump over `pixels_per_point` caught a "fix" that rounded back to the
  original value at 150% display scaling — a no-op on the user's actual machine.
- Driving real pointer and key events through `RawInput` — click, click, Home, Shift+End —
  made a text selection and proved a focused numeric field was painting its selected digits
  *transparent*. Asserting on the visuals the fix changes would have proved nothing; the
  events were the point.
- Reading the **font atlas** identified a Phosphor codepoint by its picture: laying a glyph
  out puts it in the atlas, and `uv_rect` says where, so its coverage can be dumped as ASCII
  art. That confirmed `arrows-in-line-vertical` was the arrows-pointing-*in* glyph rather
  than trusting a table.
- Counting decimals in `.shapes` caught a field rendering `411.672736201535258` for one
  frame. A one-frame artefact is worth a test: the user *does* catch them, by stepping a
  video capture.

**What `ctx.run_ui` will and will not tell you.** It returns `FullOutput`, and:

- `.shapes` is **already layer-flattened** — a `Vec<ClippedShape>` with no layer on it — so
  a probe cannot check *which* layer something was drawn into. Test the visible outcome
  instead (does the ink move?), which is the better assertion anyway.
- A glyph's *shape* is not in the galley. Its mesh is atlas-UV quads, so the vertices give
  you the box and nothing about the drawing. But the drawing **is** in the font atlas: take
  `uv_rect` off the galley's glyph and read the coverage out of `ctx.fonts(|f| f.image())`.
- `.viewport_output[…].commands` is what the frame asked the **backend** to do —
  `CursorPosition`, `CursorGrab`, the title. Behaviour that only exists once winit acts on
  it is still testable to that seam: `ui.rs`'s cursor-wrap tests assert the command, its
  position, and that no second one is issued while one is in flight.
- egui's own debug overlays land in `.shapes` like anything else, so "did egui complain
  this frame" is assertable — a red stroked `RectShape` is `warn_if_rect_changes_id` or
  `warn_on_id_clash` firing (§15 D65).
- **A widget's interaction state is last frame's**, read back through
  `Context::read_response`, so a probe that pumps synthetic pointer events to make something
  hover is measuring a value still converging. Two probes of the same dropdown disagreed for
  exactly this reason (§15 D96). Pump several frames *and* corroborate — what settled it was
  reading egui's own formula (`Style::button_style` computes the inner margin as
  `button_padding − bg_stroke.width`, keeping the stroke for a hovered or selected row and
  dropping it for a resting one) and then asserting on the **states**: a `selected` row
  needs no synthetic hover at all. **Prefer the state you can ask for directly over the
  state you have to drive the pointer into.**
- **A tooltip's ink never arrives.** `on_hover_text` defers an `Area`, and its galley is not
  in `.shapes` however many frames you pump — checked with time past the 0.5s
  `tooltip_delay`, pointer still, the row reporting `hovered()` throughout. So "does this
  control explain itself" has to be asserted one step back, on the `Response` the tooltip
  hangs from, and the test should say that is what it is doing.
- **`Ui::scope`'s own response is never hovered** — it is a placeholder over the rect, not a
  registered widget — so a tooltip hung on it is dead in *both* states. That is the obvious
  spelling for "explain why this row is dimmed", and it silently shows nothing, because a
  widget inside a `disable()`d `Ui` reports no hover either. What works is an explicit
  `ui.interact(rect, id, Sense::hover())` beside the disabled scope
  (`inspector::menu_action`).
- It is `ctx.style_of(egui::Theme::Dark)`, not `ctx.style()`, and `ctx.fonts_mut(|f| …)` for
  layout.
- 🚨 **Never nest two `Context` accessors.**
  `ctx.data_mut(|d| d.insert_temp(id, ctx.cumulative_pass_nr()))` deadlocks — both take the
  context's own lock. In debug, epaint panics with "Failed to acquire RwLock read after
  10s"; in release it simply hangs. Read into a local first, then write.

Two headless probes have paid for themselves by *failing*: one caught `UiBuilder::id_salt`
not actually stabilising a row's widget ids (a salted child still folds the parent's
positional counter in — `UiBuilder::id` is the one that works), and one caught a `ComboBox`
painting 26px where the field beside it painted 28.

### Flip-checking

**Check the probe fails without the fix.** Cheap — flip the constant or the condition, run
the one test, put it back — and it has caught tests that passed for the wrong reason. It
also gets the *reported symptom* into the failure message, which is worth more than any
comment.

**Flip against the *plausible* wrong version, not against nothing.** Deleting the feature
always fails and proves little; what the flip is for is the implementation somebody would
actually have written. Three tests in one session passed until this was done properly, each
vacuous in a different way:

- **It asserted one of two boundaries.** A test covering a list interrupted by a *non*-item
  passed against numbering that never restarted on a **changed** marker — the case it tested
  was answered by code the case it missed was not.
- **It asserted a consequence the wrong version also produces.** A caret-blink test checked
  that adding the deadline flips the phase; a deadline of a whole half-cycle passes that from
  anywhere. Sampling a hair **either side** pins it to the *next* flip rather than to *a* flip.
- **Its fixture never reached the state.** Assert the fixture is in the state you think —
  `assert_eq!(lines, 3)` before the interesting assertion — or the test is about nothing.

*The question to ask of a green test is not "does this pass" but "what would also pass this".*

**A flip that does *not* bite is a finding, not a failed experiment**, and usually a finding
about which paths the tests take. Flipping `Handle::BottomRight` to `TopLeft` left every
`size_step` test green, because that constant is only on the several-selected arm and every
test used a single selection — a whole arm was uncovered while the suite looked complete.
**Both halves belong in the test's doc**: the flip that fails proves the assertion has teeth,
the flip that passes says where the teeth are not.

**And a flip has a predicted failure *site*, not only a predicted colour.** Naming which
assertion will catch it, then running it, has been wrong three times in one session — once a
missing floor left the *width* assertion green and failed on `min_x`, the width having come
back to 1 from the other side, so a test asserting only the obvious number would have been
green for a shape that had turned inside out and moved. **Write the predicted site down, run
it, and correct the prose to what actually happened.**

⚠️ **Name the *mutation* precisely enough to be re-run.** A comment once claimed "moving the
collect below the `reopen_last` block empties the queue". The measurement was real; the
attribution was not — what had been run was the collect moved *inside* the `!reopen_last`
guard, so it never ran at all. The flip that was *described* leaves every assertion green,
making the ordering it was cited as proving **not observable at all**. Write the mutation as
a diff you could re-apply ("*inside* the guard" and "*after* the block" are opposite edits
and both read as "moved the collect"), and **when a flip's result confirms a claim you
already believed, re-read the edit before believing it.**

🚨 **A mutation named by the text it removes does not name a line, and the slip runs in the
dangerous direction** (§15 D803). `ui.rs` calls `clamp_existing_to_range(false)` at **four**
production sites and two are byte-identical — `badge_field`'s and `value_field_f64`'s, both
twenty-space chained calls. D475's note said *"removing `.clamp_existing_to_range(false)`"*,
which is precise about the text and identifies nothing. A re-run spent it on `badge_field`'s,
which is **documented as inert** — both its callers pass an unranged `Scrub`, so there is
nothing to clamp against — came back green, and came within one commit of being recorded as
*a flip with no teeth*. On `value_field_f64`'s line it fails exactly as written.
**A no-op is indistinguishable from a flip that does not bite**, so this manufactures a false
negative, which reads as "the test has no teeth" and invites deleting the line it protects.
The opposite slip announces itself. **Name the mutation by its *function*, not by its text** —
and an inert copy of a decision is a decoy for anyone mutating the live one, which is a cost
worth pricing before making the same opt-out in two places.

⚠️ **An intermittently *red* test may be no evidence of a bug at all.** A recovery test
failed twice, always on a cold run: a file's mtime is whole seconds, and the test *hoped*
two writes landed in the same one rather than arranging it. The red run and the green run
were both correct behaviour for a state the clock had picked. **Set the stamps**
(`File::set_modified`) and the case the test names becomes the case it runs — and reproduce
the flake first, or you are guessing at which of several plausible causes it was.

Delete the probe once it has answered the question; keep whatever it proved as a real
assertion (see `ui.rs` and `cursor.rs` for both halves of that).

## Build / test

```bash
cargo build -p ondin-app      # build the app binary (target/debug/ondin.exe)
cargo test                    # workspace tests
cargo clippy --workspace --all-targets   # --all-targets or the tests are unlinted
cargo fmt --all
cargo check --release -p ondin-app   # the one gate that is not the debug cfg
cargo doc --workspace --no-deps --document-private-items   # the gate that reads doc comments
```

**At the close of a session, once — not in the edit loop** (§15 D771, D597):

```bash
cargo test --workspace --release
```

**It is the only thing that *runs* anything under the release cfg**, and `check --release`
is scoped `-p ondin-app`, so it does not compile the other four crates' test targets at
all. The class it catches is a test whose **meaning** changes with the profile — egui
defaults `warn_on_id_clash` to `cfg!(debug_assertions)`, which made one test red with
nobody looking and another green while asserting nothing. ⚠️ **Ask what the libraries do
with the cfg, not only what we do**: every instance that has bitten was a dependency
reading it on our behalf. Warm: ~26 s of fingerprinting plus a 6 s run. Cold: ~96 s, and
**2.4 GB** of a second profile's artifacts.

🚨 **The cost figure for this was false in three places for weeks** — *"it roughly doubles
the test time"*. Measured: the release *run* is 6 s against debug's 20 s, three times
**faster**, partly because `[profile.dev.package.ondin-render] opt-level = 3` already
optimises the hot crate in debug. The phrase landed near the right *total* for entirely the
wrong reason, and the reason is what the decision turns on.

The app is a `bin` crate (`ondin`) — run its unit tests with `cargo test -p ondin-app`
(not `--lib`).

### Clippy

**`--all-targets` is not decoration: without it the lint never sees a single test.** A bare
`cargo clippy --workspace` builds lib and bin targets only, so every `#[cfg(test)]` module
in the workspace — where most of the assertions in this project live — went unlinted from
the day the gate was written until §15 D302. Adding the flag surfaced five warnings that
had been passing, one of them **a test written the day before that compared a dangling
pointer**. `rust-verify` had carried `--all-targets` all along and this file had not, which
is the whole lesson — **a gate written down in two places is two gates.**

🚨 **And `--all-targets` is not enough either: `--workspace` and `-p` compile *different
code*.** Workspace **feature unification** changes a dependency's memory layout, so the two
spellings are two compilations of `ondin-core`, not two views of one. Measured with
`const _: [(); std::mem::size_of::<peniko::Gradient>()] = [];` and rustc's own `E0308`:

```
cargo check -p ondin-core --all-targets  → "expected an array with a size of 168"
cargo check --workspace   --all-targets  → "expected an array with a size of 160"
```

Eight bytes. That is why a `large_enum_variant` on `svg_in.rs`'s `enum Paint` printed under
`-p` and not under `--workspace`: threshold 200, and `224 − 16 = 208 > 200` against
`216 − 16 = 200`, **not** `> 200`. **The lint was behaving perfectly and was being handed a
different type.** It also explains why a synthetic reconstruction failed to reproduce it —
only an enum whose size depends on a third-party type can differ.

Three consequences:

1. **Run clippy per package when it matters**, or at minimum re-run `-p` on the crate you
   edited. A warning only one spelling prints is a warning nobody sees.
2. **Anything pinned by size or layout is pinned under whichever spelling was typed.**
3. 🚨 **It decides whether a *test* can fail.** §15 D600's guard refuses an image format
   outside the four v1 accepts, and its test feeds it BMP — but `bmp` is compiled **only
   under `--workspace`**, arriving through `arboard`'s Windows dependency. The flip that
   widens the guard is **green under `cargo test -p ondin-render` and red under
   `--workspace`**. Under `-p` there is no decoder for the guard to protect anything from.
   **"I flip-checked it" is an incomplete claim here — ask which spelling.** The repair is
   general: split the decision into a predicate that does not depend on the resolved graph
   (`images::signature_is_accepted` reads magic bytes) and assert *that*.

⚠️ **Run the per-package clippy *after* the test build, not beside it.** The first run in a
batch is the one that *compiles* the test target, so lints on a test written in between land
on the second invocation. It reads exactly like the hole above and is a different thing.

### The doc gate

**This is the gate that checks the record against the code.** A doc comment naming an item
that has been renamed or deleted compiles, tests, lints and formats perfectly — the
*cheapest* kind of drift to introduce and the most expensive to trust. Each of the **five
lib and bin crate roots** denies `rustdoc::broken_intra_doc_links` and
`rustdoc::invalid_html_tags`, so it exits 101 rather than printing a warning nobody reads.
`rustdoc::private_intra_doc_links` is deliberately **allowed**: it fires on links that are
correct and merely unrenderable, and it cannot hide staleness.

It is paid for: added against **52 warnings**, twenty of which were names resolving to
nothing, and among them a paragraph asserting a contrast between two functions — one of
which did not exist, while the other had become the thing the paragraph said it was the
opposite of (§15 D296).

⚠️ **`--document-private-items` is not optional.** Without it rustdoc checks no link on any
private item, which here is most of them — `Builder`, every `OndinApp` method, every free
helper. Measured: a deliberately broken link on a private method survives a full
`cargo doc --workspace --no-deps` at exit **0**. Adding the flag immediately surfaced three
broken links plus eight naming a struct with no `impl` block at all.

**Three target kinds it cannot see, by three different mechanisms:**

1. **`#[cfg(test)]` modules.** `cargo doc` builds without the `test` cfg, so a module gated
   on it is simply absent. Forcing the cfg does not help — rustdoc excludes
   `#[test]`-attributed items under any flags.
2. **`crates/*/tests/`.** Separate test *targets*; the flag that would ask for them does not
   exist (`--tests` errors out). Each file is **its own crate root** and not one carries a
   `deny`, so a doc step for the tests would not be enough on its own.
3. **`build.rs`.** `crates/ondin-app/build.rs` carries 22 lines of `//!` prose; a broken
   link there passes at exit 0 while `fmt --check` and clippy both catch the same probe
   (§15 D622).

🚨 **The convention for all three is plain backticks and a sentence saying why** (§15 D319).
**Both populations are ZERO** as of 2026-09-16 — 277 links across 53 files were resolved by
hand and converted, and none dangled. **This is an invariant, not a figure**: the only
correct value is 0, so it cannot rot the way every other count in this file can, and a
non-zero reading is unambiguously a regression.

**The re-check is your own diff**, not a tree-wide scan:

```bash
git diff <base>..HEAD -- crates/ | grep -E '^\+ *(///|//!).*\[\`'
```

Then ask of each hit whether its item, or the module around it, is `cfg(test)`. Writing
`[`foo`]` is the reflex; this grep is the correction, and it caught one per session for
six sessions running. ⚠️ **One such link was on a *production* item and named a test** —
`ui::SLIDER_H`'s doc — and the doc gate itself exited 101 on it, because a production doc
**cannot name a test-only item**. That is the same mechanism from a third direction.

⚠️ **If you ever do rebuild a tree-wide scanner, reset state per file.** `awk` keeps
variables across the files `xargs` hands it, so one file ending while the scan still
believes it is inside a test module makes the *next* file count **whole-file** — production
links and all. That is how a pass reported 362 links over 48 files against a true 259 over
44. ⚠️ **And `\b` in gawk means backspace, not a word boundary**, so `/\bmod\b/` silently
matches nothing; use `(^|[^A-Za-z_])mod[[:space:]]` and open with
`FNR==1 { intest=0; depth=0 }`.

### The release check, and the cfg census

**Every gate above `check --release` that compiles anything compiles it with
`debug_assertions` on** — build, test and clippy; `fmt` compiles nothing. So a line behind
egui's own `#[cfg(debug_assertions)]` can break the release build with all four green,
which is what happened to `theme::install` (§15 D161). `check --release` rather than
`build --release`: same cfg without the codegen wait.

🚨 **This paragraph used to end with a parenthesis that was incapable of disagreeing with
it** (§15 D732): *"`debug_assertions` is still the only cfg the app reads
(`grep 'cfg(debug'` → `theme.rs` alone)"*. **That grep matches only cfgs whose name starts
with `debug`**, so by construction it cannot report any other cfg. It is the purest instance
of this file's recurring subject — a check that looks like it covers something and cannot
falsify it. **When a claim comes with a command, read the command against the claim, not
just its output.**

The real census cannot be scoped to its own answer:

```bash
grep -rhoE 'cfg\(([a-z_]+)' crates/ --include=*.rs | sort | uniq -c
```

Last run: **327 `test`, 6 `windows`, 3 `panic`, 3 `not`, 3 `debug_assertions`, 2
`target_os`, 2 `all`, 1 `unix`.** ⚠️ Only `test` has moved across five sessions — **the
interesting half of this census is the tail, not the total.**

⚠️ **So it is the first of a class, and the rest of the class has no gate at all.**
`canvas::os_cursor_desktop_px`, `library::clock::local_offset` and `library::store`'s
permissions arm each have a `#[cfg(not(windows))]` twin, and `panels::show_in_file_browser`
has a `#[cfg(target_os = "macos")]` arm — **four decision functions this machine never
compiles**, in either profile, under any gate. A rename, a signature change or a
`rust-mechanic` sweep passes straight over them. Not fixable by adding a gate here; it wants
a cross-compile or a CI runner.

### Dead code has two blind spots

**A clean clippy run does not mean there is no dead code.**

1. **`ondin-core` is a library, so a `pub` item is part of its API** and `dead_code` never
   fires on one however many callers it has lost. Removing a builder's last caller leaves
   the builder — and its now-false doc — with every gate green. **After deleting or
   superseding anything `pub` in core, grep the workspace for the name and check the count
   is what you expect.**
2. **A derived `PartialEq` hides an unread field anywhere in the workspace.** Measured
   rather than reasoned: bare, `dead_code` fires; with `#[derive(Debug)]` it still fires
   (rustc deliberately does not count that as a read); with `Clone, Copy` it still fires;
   add `PartialEq, Eq` and it goes **silent**, `--test` or not. So any struct here deriving
   `PartialEq` can carry a field nothing reads with all six gates green — which is most of
   the small records in `ondin-app`.

⚠️ **Outside core, blaming the `pub` is the wrong reflex**: `ondin-app` declares only
`[[bin]] name = "ondin"` and has **no lib target**, so a `pub` item inside one of its
modules is not externally reachable and the lint *does* analyse it. That is what
`layers::DropTarget::into` did — written on every frame of a drag, read by nothing in
production, caught by `arch-scribe` reading the struct rather than by any gate (§15 D345).
**After removing a *reader*, read the struct; grepping the name answers the other half.**

⚠️ **And `#[allow(dead_code)]` on a module is not the tool.** Moving it onto a
`pub const ALL` that names all 143 icon constants **restores the blanket shield**, because
an allow-listed item is still a live *root*. `#[cfg(test)]` is what works — and it makes the
constant's own doc true rather than aspirational. ⚠️ **But not always**: where the item is
named by a **production** doc link, `#[cfg(test)]` makes the doc gate exit 101, and the
narrow `#[allow]` is right precisely because that item names nothing else dead (§15 D672 and
D699 bracket the same attribute from opposite sides).

### Gates that look like they cover the code and do not

**Thirteen, by thirteen unrelated mechanisms.** The list matters less than the standing advice
under it:

1. Clippy without `--all-targets` never lints a test (D302).
2. `cargo doc` cannot see a `#[cfg(test)]` module (D319).
3. `ondin-core` being a lib means `dead_code` never fires on a `pub` item.
4. `--workspace` and `-p` compile different code (feature unification).
5. `cargo doc` without `--document-private-items` checks no private item's links.
6. An enforcement test whose predicate is narrower than the rule in its own name —
   `deps_forbidden`'s `starts_with` over eight literal prefixes, where
   `"eframe".starts_with("egui")` is false, and so are `emath`, `ecolor`, `epaint`.
7. **No gate detects an unused dependency** — `unused_crate_dependencies` is absent from all
   five crate roots, which made a documented §3 rule false for as long as `ondin-mcp`
   declared an `ondin-export` no line of it used.
8. 🚨 **A gate can name the right rule, use a wide-enough predicate, and be pointed at the
   wrong artefact.** `deps_forbidden` ran `cargo tree -p ondin-core` while every gate that
   *compiles* core resolves features workspace-wide — reading a tree nothing builds (D427).
9. `png::MAX_RASTER_SIDE` is derived from reading `vello_cpu`'s source and **neither
   constant is public**, so an upgrade can silently invalidate it. What stands in for a gate
   is a test that *renders at the cap on both axes* (D434).
10. 🚨 **A gate nobody wrote because the documentation said not to bother.** The
    `panic = "abort"` note closed by telling every reader the absence was undefendable —
    *"nothing in the code can detect the setting"* and *"there is no test that can fail"*.
    **Both false.** `#[cfg(panic = "abort")]` is a stable rustc cfg, and `boolean::guarded`'s
    synthetic panic aborts its own test binary under the setting, which cargo reports as a
    hard failure of the whole target. A `compile_error!` now sits beside the guard (D445).
11. `check --release` compiles and does not *run*, so a test whose meaning changes with the
    profile is invisible to all six (D597).
12. 🚨 **A working gate written *out* of the record.** D319, a live *Keep* entry, decided
    `--document-private-items` off the gate command on the claim that *"a broken link is
    reported whatever the item's visibility"*. False, and never measured — reasoned from
    what a gate is *for* (D622).

13. 🚨 **A green `cargo test` does not mean the tests are independent, and nothing here can
    say which ones are not.** Any test touching a process-wide resource can be wrong only at
    certain interleavings, so the harness's own parallelism decides whether it is seen.
    Found the hard way (D796): four new tests that each opened a context menu aborted the
    binary with `STATUS_HEAP_CORRUPTION` four runs in five, because a context menu snapshots
    the **OS clipboard** and two `arboard` handles at once corrupt the heap. **The whole
    1,290-odd-test suite was green throughout and stayed green** — the tests that could
    collide already existed and were simply spread thin enough to rarely meet. ⚠️ **The tell
    is a filtered run, not a full one**: `cargo test <filter>` on a handful of related tests
    packs them onto every core at once, and that is what made it reproducible. **When a
    module's tests share anything the OS owns, run that module's filter on its own a few
    times.** ⚠️ And a fix for this class must not be checked at the lowest seam — the test
    that proves the lock works has to keep reaching the real resource, which is why D798's
    refusal sits in the four callers and not inside `with_clipboard`.

**Three questions to ask of a gate**: does it check the rule, with a predicate wide enough,
**against the thing that actually ships?** And two of an *absent* one: **when the record says
a check is impossible, check** (D445), and **when the record says a check is unnecessary,
check** (D622).

⚠️ **Suspect a fourteenth.** None of the thirteen was found by looking for it — two came from a
subagent's aside, one from reading a doc link against the type it named, and one from writing
an unrelated test. The cheap general move that found several: **take a gate, break something
it claims to cover on purpose, and check it goes red.** It costs a minute.

⚠️ **The test-shaped version of the same failure is a different list**: an assertion that
passes because a flip aborts on an earlier case, and an assertion that fails against its own
fixture rather than against the change. **A gate hole is found by breaking the gate; a
vacuous assertion is found by disabling the one in front of it.**
