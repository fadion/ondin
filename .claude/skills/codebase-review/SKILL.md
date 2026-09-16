---
name: codebase-review
description: Set up and start a multi-level review of the Ondin codebase — check the gates, cut the code into review slices, run a mechanical fact sheet, then the repo-wide sweeps. Use when the user asks for a full/deep/multi-level codebase review, a review "by module" or "in different lenses" (bugs, quality, performance, architecture, security), an audit of the whole repo, or says "start the review" / "set up a codebase review". Produces review/plan.md + review/findings.md; individual passes after setup are run by the codebase-review-pass skill. Do NOT use for reviewing a pull request or the working diff — that's /code-review.
---

# Start a codebase review

A whole-codebase review that stays useful is **slice × lens**, run as a sequence of small passes
with every finding written to one ledger. This skill does the setup and the
repo-wide part; `codebase-review-pass` runs everything after that, dispatching each pass to its own
subagent and stopping only at the attended checkpoints.

`reference.md` (next to this file) is the method spec — lenses, per-pass protocol, the verification
gate, the ledger format and its integrity rules, the orchestration contract. **Read it before
Phase 2.** Both skills follow it, so it's the one place the method lives.

**This skill is Ondin-specific.** The parts below that name cargo commands, the invariants in
`docs/architecture.md` §4, this repo's traps and its scope decisions are the highest-yield content
in it — a generic reviewer misses all of it. `reference.md` is the portable half and transfers
between projects unchanged.

## Artifacts

Everything goes in `review/`, which is gitignored:

| File | Role |
| --- | --- |
| `review/plan.md` | The work order for *this round* — the slice table with measured LOC, attended/delegated flags, the triage schedule. Generated in Phase 1. |
| `review/facts.md` | Pass 0's mechanical fact sheet. |
| `review/findings.md` | The ledger: pass log (the cursor), findings, rejected candidates. |
| `review/index.md` | One line per finding (`ID \| severity \| file:line \| claim`) + the rejected one-liners. What each subagent reads instead of the ledger. |
| `review/.snapshots/` | Per-pass ledger snapshots (reference §5). Kept, not rotated. |

What lives *here* rather than in `plan.md` is everything durable across rounds: the preconditions,
the A1 evidence set, the reviewer notes, the scope decisions. `plan.md` holds only what is measured
once, at the round's opening SHA, and gets revised mid-round.

If `review/plan.md` already exists, the review is set up — **don't regenerate it**. Hand straight
over to `codebase-review-pass`.

## Phase 0 — the ground

**This review deliberately does not freeze the tree.** The upstream method freezes at a tag, and
that is right for a repo whose history is kept. Here a review-round tag would pin `main` for weeks
to buy an anchor the round does not need. **The maintainer's decision, 2026-09-03.** Development,
and the review's own fixes, continue on `main` throughout.

🚨 **The original argument for that decision has expired, and the decision has not.** It read
*"here git is a safety net rather than a record — `.git` is to be deleted once the feature list
lands and the repo restarted, probably at a `0.1.0` tag — so a review-round tag would anchor
findings to something that will not exist by the time anyone re-reads them"*. The reset happened on
2026-09-16 and the repository that replaced it is published at `https://github.com/fadion/ondin`
with its history kept, so a tag would now survive perfectly well. What survives of the reasoning is
the second half — the cost of pinning `main` — and that is enough on its own for a round that runs
for weeks. ⚠️ **A range review is the opposite case and freezes deliberately**; see
`../release-review/SKILL.md` Phase 0.

Three consequences, and each has to be handled or the review quietly degrades:

1. **`file:line` rots, so a line number is not the anchor.** Every finding records the enclosing
   item as well — `crates/ondin-app/src/canvas.rs:5528 · fn snapped_point`. A symbol survives an
   insertion above it; a line number does not, and in a repo whose history is going away the symbol
   is the more durable half anyway. A finding that cannot name an enclosing item names the nearest
   `///` heading or a quoted line of code instead.
2. **A later pass reads newer code than an earlier one did.** So the pass log records the SHA each
   pass ran at, rather than one SHA at the top. That number is what triage needs: a finding whose
   file has changed since its pass is re-checked before it is ranked
   (`git log --oneline <pass-sha>..HEAD -- <file>`), because it may already be fixed.
3. **"Dirty tree" can no longer mean "the subagent edited something."** The guard becomes a
   *comparison*: capture `git status --short` before dispatching a pass and again on return; any
   new entry is the subagent's, and a probe it forgot to delete is the usual cause.

Before Pass 0, verify in order (these are the project's own gates, in CLAUDE.md's corrected form):

1. `git status --short` empty before Pass 0 — a pass that starts on top of unsaved work cannot tell
   the user's edits from a subagent's, which is what the per-pass status comparison in point 3
   above exists to detect. ⚠️ **The way to get there is no longer "commit it"**: CLAUDE.md's
   *"commit freely and without asking"* was reversed on 2026-09-16 and a checkpoint commit is now
   permanent published history. Ask the user to commit outstanding work through the `commit` skill,
   or wait. **The review's own artifacts are the exception and count as setup**: these skills and
   the `/review/` line in `.gitignore` go in before Pass 0.
2. `cargo fmt --all --check` → exit 0.
3. `cargo clippy --workspace --all-targets` → clean, **then re-run it per package**:
   `for p in ondin-core ondin-render ondin-export ondin-mcp ondin-app; do cargo clippy -p $p --all-targets; done`.
   The workspace form silently drops a member crate's warnings (CLAUDE.md, measured 2026-09-01);
   a warning only one spelling of the command prints is a warning nobody sees.
4. `cargo test --workspace` → green.
5. `cargo doc --workspace --no-deps --document-private-items` → exit 0. **The flag is not
   optional**: without it rustdoc checks no link on any private item, which here is most of them.
6. `cargo check --release -p ondin-app` → the one gate not compiled with `debug_assertions`.
7. **The two suites the workspace run does not reach**, on this machine, once, and recorded:
   - `cargo test -p ondin-render --release -- --ignored` — ~21 tests behind
     `#[ignore = "requires a GPU adapter"]` plus the zoom sweep. Slices S10/S11 lean on these; if
     they are not run, say in the ledger that the on-device path is **unverified** rather than
     quietly proceeding.
   - `cargo test -p ondin-app fonts -- --ignored` — three network tests (Fontsource/jsDelivr).
     Slice S21 leans on them.

   All 33 `#[ignore]`s in the tree are legitimately gated (GPU, network, one 23-second boolean
   instrument). "No `#[ignore]`" is therefore **not** a precondition here; "every `#[ignore]` has a
   reason string and the gated suites were run once" is.
8. **The two record-integrity baselines** from CLAUDE.md, run and recorded in `facts.md`:
   - `grep -rhoE 'D[0-9]{1,3}\b' crates/ --include=*.rs | sort -u | wc -l` → expected **298**. Not
     the number alone — **set-difference it** against the previous list, because one gained and one
     silently dropped is indistinguishable from unchanged.
   - The `///`-run length ranking (see A6). Baseline, module docs excluded: `text::skip_ink` 82,
     `snapshot::SNAPSHOT_VERSION` 68, `boolean::evaluate` 58, the `Exclude` coverage test 55,
     `build::mask_target` 54. A new name in that head is a finding.
9. Record `git rev-parse HEAD` at the top of `review/findings.md` as the **starting** SHA — the
   commit the round opened at, not a frozen one — and say there that the tree is deliberately not
   frozen, so that a reader a month later does not mistake a drifted line ref for a wrong finding.

**If any of these fails, stop and report.** Don't fix them uninvited and don't review anyway — the
user decides.

Two environment facts that will otherwise cost a pass:

- **Windows:** stop a running `ondin.exe` before building, or the linker can't overwrite it.
- **Never launch the GUI** (CLAUDE.md). It grabs focus and the mouse and hijacks whatever the user
  is doing. A pass with a question about chrome answers it with a **throwaway headless `#[test]`**
  (`OndinApp::headless(&ctx)`, §15 D303) or with `ondin export`, and deletes the probe. A pass that
  cannot answer its question either way says so in "Not covered" rather than guessing — and a
  reviewer probe is still an edit to the tree, so it must be removed before the pass returns and
  `git status --short` must come back empty.

## Phase 1 — generate the plan

Inventory first: crates, **production** line counts per file (measured with inline `#[cfg(test)]`
modules excluded — they are 36% of this tree and would triple the pass count), test counts, and
`docs/architecture.md`'s §3 layering and §4 invariants. Then cut slices.

**Slicing heuristics** — the part that makes or breaks the review:

- **Cut along flows, not directories.** A slice should be one capability end to end. In this
  codebase the bugs that matter cluster in the **model→resolve→scene→render seam** and in the
  **gesture→preview→transaction seam**, so a slice that is "one module" misses exactly those.
- **A slice is a capability; a pass is ~2.5k production lines.** Different units. Aim for **~20
  slices**, then give each `ceil(prod LOC / 2.5k)` passes, numbered `S12.1`, `S12.2`.
- **Measure the lines, don't estimate, and do the arithmetic in the plan.** State the total
  invocation count (passes + Pass 0 + sweeps + triage checkpoints) up front — that number is what
  the user budgets against, and it's the single easiest thing for a plan to get wrong.
- **Name each sub-pass's split point in the plan**, not mid-pass. Deferring it pushes the decision
  to the moment of maximum incentive to skim, and a pass that skims is worse than one that doesn't
  run, because it looks covered. `inspector.rs` has **nine** test modules interleaved with source,
  so split points there are line ranges, not "the first half".
- **Order by blast radius, not size.** Slices where a bug destroys the user's document or their
  exported file go first: save/load/recovery, then export, then the model and the edit builders.
  State the rationale, then check the order against it.
- **Assign lenses per slice, not all lenses to every slice.** Typically 2–4. Most cells in a full
  slice × lens matrix are empty and filling them produces noise.
- **Flag each pass attended or delegated** (reference §7). Default is delegated. Attended: the plan
  sign-off, A1, and every triage checkpoint.
- **Verify coverage mechanically before showing the plan.** List every `.rs` file under `crates/`,
  check each appears in exactly one slice, and state in the plan that it does. Prose like "the
  panels" reads as coverage and isn't — that's how files go missing.

Also define in the plan: the **triage schedule** (reference §6 — after Tier A, then every 6–8 slice
passes; budget them against the *final* slice count) and any **scope decisions** for this round.

**Show the plan and get agreement before Phase 2.** The slice map determines everything downstream.

## Phase 2 — Pass 0, the mechanical fact sheet

Create `review/findings.md` **first** — the round's opening SHA and the not-frozen note at the top,
with a `SHA` column in the pass log, then the pass
log with Pass 0's row marked `in progress`. The ledger has to exist before the first pass, or Pass 0
has nowhere to record itself and a resumed session can't tell it ran. Create `review/index.md`
empty at the same time.

Machine-findable issues must never consume a model pass:

```bash
cargo clippy --workspace --all-targets -- -W clippy::pedantic -W clippy::nursery
```

**plus the same sweep per package** — the workspace form drops a member crate's output. Then the
census below, written to `review/facts.md`.

| Row | Kind | Feeds |
| --- | --- | --- |
| `unwrap()` / `expect()` / `panic!` / `unreachable!` / raw `[i]` **outside test modules** | census (**64** production sites, measured 2026-09-03 — ~1,330 more are inside `#[cfg(test)]`) — split by crate and by whether the input is user-controllable | L1 panic surface; **invariant 8** ("operations never panic on bad input") |
| `unsafe` blocks | census (2 today — enumerate both) | L5 |
| `#[allow(...)]` in source | census (**38** attributes) — each is a silenced signal | L3 |
| `TODO` / `FIXME` / `XXX` / `HACK` | census (2) | L3 / triage |
| `cargo deny check` (installed), `cargo tree --duplicates` | census | L5 / L3 |
| `cargo llvm-cov --workspace --summary-only` per file (installed) | census | L6 / A5 baseline |
| `#[ignore]` sites + their reason strings | census (**25** attributes) | L6 — a gate nothing runs |
| Every `D<n>` cited from `crates/` (298) → resolves to a §15 heading or index line | census + **set-difference vs. the previous list** | A7 |
| Contiguous `///` run lengths, module docs (`//!`, runs starting at line 1) excluded, sorted desc with the item each precedes | census | A6 doc-comment theft |
| `pub` items in `ondin-core` with zero callers in the workspace | **candidate list** — counts per module only; `dead_code` never fires on a `pub` lib item | L3 |
| Structs deriving `PartialEq` (any crate) with fields written but never read | **candidate list** — a derived `PartialEq` silences `dead_code` on an unread field, measured | L3 |
| `.clone()` inside per-frame closures / the scene walk / per-keystroke paths | **candidate list** — counts per file only | L4 |
| `get`/`set_preview` call sites vs. `preview_guide` call sites | **candidate list** — the second preview path every `set_preview` owes beside it | L1 |

**Separate a census from a candidate list.** A census is mechanical and its output is a fact. A
grep whose hits still need per-site judgment is a *candidate list*, and it belongs in the fact
sheet as counts and file distribution only, feeding a later pass. Putting one in as a census either
buries the real facts under hundreds of undifferentiated hits or quietly turns Pass 0 into a model
pass.

⚠️ **Count attributes, not mentions — in this repo the difference is large.** Round 1's Pass 0
found three of the numbers above inflated by doc comments *discussing* the attribute being counted:
`#[allow]` grepped 39 against 38 real attributes, and `#[ignore]` grepped **33 against 25**, an
eight-mention gap that is the origin of the "33" this table carried until 2026-09-03. A codebase
whose comments are 43% of its production lines and which writes about its own gates at length will
do this to any bare grep. **And a naive `#[cfg(test)]` split is worse than an inflated count**: the
first such module in `inspector.rs` starts at line 832 of 17,887, so "count everything above the
first `#[cfg(test)]`" discards 95% of the file. Round 1 used a brace-matched scanner and validated
it against raw grep; anything less is not reproducible.

**Note explicitly whatever failed to run.** A silently dropped census reads later as a clean result.

Then stop and summarize.

## Phase 3 — Tier A sweeps

Standard shapes in reference §2. A1–A5 are the standard set; **A6–A8 are specific to this repo** and
are where its distinctive risk lives — the record, and the gates that don't cover what they look
like they cover.

| # | Sweep | Lenses | This codebase |
| --- | --- | --- | --- |
| **A1** | Invariant conformance | L2 | The 11 numbered invariants in `docs/architecture.md` §4, one at a time, enumerating **every** site each governs. Evidence set below. **Attended.** |
| **A2** | Architecture & boundaries | L7, L3 | The §3 dependency graph as actually built (`deps_forbidden.rs` covers core only — nothing enforces "only `ondin-app` may depend on egui/winit"). The god modules: `inspector.rs` 11.1k production lines, `canvas.rs` 11.2k, `app.rs` 7.3k, `tools/mod.rs` 3.6k — still cohesive, or landfills? Are `ScenePainter`, `RenderOverrides`, `Operation`/`Transaction`, `Resolved` real seams or decoration? What does `ondin-app` reach into that it should be asking core for? |
| **A3** | Security & data safety | L5 | Untrusted input end to end: **SVG import** (`svg_in.rs`, 3.0k lines parsing arbitrary files), **image decode** (four decoders, arbitrary bytes, `image` crate), **font download** (`ureq` → Fontsource/jsDelivr → `woff2` decode → cache dir write), **clipboard paste**, **file drag-drop**. Then the write paths: `atomic::write` and every caller, the `.recovery/` snapshot, the library store, zip export, and every place a user-supplied name becomes a path component. Panic messages carrying document data. |
| **A4** | Performance map | L4 | Not a code read: a *map*. Per-frame (the scene walk, `nodes_in_view`, the canvas overlays, the rulers), per-keystroke (text shaping/`TextLayout`, the typography panel), per-gesture (hit test, snap, `RenderOverrides` rebuild), per-image (the adjust pass and its preview reduction, the decode cache, thumbnails), per-export. Output: ranked ≤8 paths worth deep L4 attention. Note that `ondin-render` is `opt-level = 3` even in debug, so a debug-build measurement of anything else is not comparable. |
| **A5** | Test map | L6 | `llvm-cov` per file against decision functions; logic sitting in `&mut self` app methods where no test can reach it (§15 D269 — the fix is lifting it into a free function, not a harness); modules at zero; and whether the invariant-enforcing tests still enforce — `deps_forbidden.rs`, the SVG/JSON goldens, the round-trip suite, the boolean unwind guard. |
| **A6** | Doc-comment ownership | L3, L8 | The `///`-run ranking from Pass 0. **Five instances of a doc comment silently reassigned to the wrong item are on record**, all with every gate green; one block had swallowed three. Investigate every run over 54 lines that is not one of the five excused baseline entries, and check the head of the ranking entry by entry rather than against a remembered top. The ranking is blind to short thefts and to deletions — so also spot-check `grep -n -B2 "fn <name>"` around items added in the last ~50 commits. |
| **A7** | Record integrity | L8 | The thing CLAUDE.md is written about. Every `D<n>` cited from `crates/` resolves (298). Every §15 *Keep* / *Intentional* / *Deviation* entry still describes the code as it stands — those are the live ones; *Resolved* is history. `docs/roadmap.md` holds open work **only**, and nothing in it is already recorded in §15. Code comments asserting behaviour that does not exist — that has happened here at scale twice (several modules; seven passages about golden files). Doc comments whose prose contradicts the field or function below them. |
| **A8** | Gate holes | L6, L3 | **Five gates that look like they cover the code and do not are on record**, by five unrelated mechanisms, and CLAUDE.md predicts a sixth. This sweep goes looking on purpose: take each gate, break something it claims to cover, and check it goes red. Known five, to re-confirm rather than rediscover: clippy without `--all-targets` lints no test; `cargo doc` cannot see a `#[cfg(test)]` module; `cargo doc` without `--document-private-items` checks no private item; `cargo clippy --workspace` drops a member crate's warnings; a `pub` item in core is never `dead_code`, and a derived `PartialEq` hides an unread field anywhere. Then: what about the files under `crates/*/tests/`, which are separate crate roots and carry **no `deny`** — their intra-doc links are checked by nothing, and neither is `build.rs`, which is a third target kind (§15 D622, `[A8-L6-01]`). *No numbers here on purpose: this row said "21 files … 11 links" while the tree held 23 and 19, and it was one of seven stale copies of a figure nothing recomputes. Count it during the sweep.* And `.claude/agents/rust-verify.md`'s gate list is itself stale (no `--document-private-items`, no per-package clippy), which is a gate hole one level up. |

A1 is **attended** — a wrong call there is read as settled by every slice pass after it. A2–A8
delegate. Run them one at a time, appending to the ledger, and pause after A1.

### A1's evidence set — the 11 invariants and what each governs

`docs/architecture.md` §4 heads them "non-negotiable". Enumerate every site, don't sample.

| # | Invariant (§4) | Enumerate |
| --- | --- | --- |
| 1 | Core is headless — no GPU/windowing/UI dependency, fully testable without a device | `deps_forbidden.rs` covers the *transitive* tree, so this one is machine-enforced. Check instead that the test still runs and still asserts the full forbidden list, and that nothing in core needs a device at test time. |
| 2 | **Single mutation path** — "The only way to change a `Document` is `Document::apply(Transaction)`. Node fields are private; no public `&mut` access." | Every `pub fn` on `Node`/`Document` returning `&mut`; every mutation of node state reachable from outside `apply`. |
| 3 | Identity stable, never reused, **caller-minted** — "`apply` never allocates ids" | Every `IdSource` use; any id minting inside `apply` or an op handler; `remap_subtree` / `reserve_existing_ids` on the load and paste paths. |
| 4 | Document = truth, everything else derived — "No render representation ever feeds back into the model" | Any write into `Document`/`Node` originating in `ondin-render`, `Resolved`, or scene code. |
| 5 | **Uncommitted state never enters the Document** — gestures live in app-side preview state and reach the renderer as `RenderOverrides`; "exactly one `Transaction` is committed per completed gesture" | Every gesture in `tools/`, `canvas.rs`, `rulers.rs`, `panels/`: does it mutate the document mid-gesture, and does it commit exactly one transaction? Its corollary "one gesture = one transaction = one undo step" (§9.3) is the assertable half. |
| 6 | Local transforms only — world transforms live only in `Resolved` | Any node field or serialized value holding a world transform. |
| 7 | Colour-space discipline — sRGB, straight alpha in the document; "the conversion lives **only** in `ondin-render` (`color.rs`)" | Every premultiply/linearize/gamma site outside `render/color.rs`. |
| 8 | Operations total & validated — "returns `Result<_, OpError>`; never panic on bad input" | Cross this with Pass 0's panic census: every `unwrap`/`expect`/`panic!`/index in an op handler or on a path reachable from a loaded file. `apply` is atomic — "on any error mutate nothing" — so also: does any handler mutate before the last validation? |
| 9 | Serialization versioned & deterministic — "saving the same document twice produces identical bytes" | Every `HashMap`/`HashSet` (incl. `rustc-hash`) whose iteration order reaches `save`; every `#[serde(default)]` field added without a version bump where the default changes an old document's meaning; `migrate.rs`'s coverage of every version. |
| 10 | MCP is a thin adapter | `crates/ondin-mcp/` is 50 lines today; check it holds no logic and stays that way. |
| 11 | Renderers behind one shared walk — "the document→scene walk is written exactly once, in `scene.rs`" | Any walk logic duplicated in `gpu.rs`/`cpu.rs`/`fx_gpu.rs`/`export::svg`. Note §15 D2: the original `trait Renderer`/`RenderTarget` design is dead and the doc says so inline — check the prose around it hasn't re-drifted. |

Three more rules recur across many sites and belong in the census even though they sit outside §4:

- **§6.2 — `RenderOverrides` refuses what it cannot represent.** Delete/reparent/reorder/`SetClip`/
  `SetMask`/`SetMaskMode` return `None` rather than a lying preview: "a missing preview is
  recoverable; a lying one is not." Enumerate every `set_preview` site — **and its
  `preview_guide` counterpart**, which is the second preview path chrome needs and which
  `RenderOverrides` cannot carry.
- **§5.9 — a masked layer keeps its own world bounds**, "conservative because a clip only removes
  ink". Governs every bounds/snap/selection-box consumer.
- **§5.11 — integrity on load is total, not a spot check**: root validity, parent/child
  back-references, no double-listed child, full reachability, kind-consistency.

## Project-specific notes for reviewers

These go into every subagent's brief. They are the difference between a review of this codebase and
a generic one.

- **The invariants are the point.** `docs/architecture.md` §4 heads them "non-negotiable", and each
  encodes a decision already paid for. **L2 outranks everything else here.**
- **§15 has no "open defect" verdict.** Its four verdicts are *Resolved* (history — the code no
  longer deviates), *Keep*, *Intentional*, *Deviation*. The last three describe the code **as it
  stands and as the design accepts it**. So a §15 entry is never a bug to re-raise: if a finding
  contradicts a live entry, the finding is probably wrong, and if it is right the entry is the
  drift. Open work lives in `docs/roadmap.md` only, never duplicated into §15. §15.0 is the
  numeric index.
- **This codebase is deliberately conservative in specific places**, and each is a trap for a
  plausible-but-wrong finding. **Check the guard before claiming the gap** (reference §3.2):
  - `RenderOverrides` returns `None` rather than a wrong preview (§6.2).
  - A missing image draws a placeholder **for a fill only** — a stroke or text run painted with a
    missing picture deliberately draws nothing, "the placeholder is a statement about an area and a
    stroke has none" (§15 D179).
  - `crop_reframed` is deliberately **not** clamped where `crop_scaled` is (§5.5a).
  - A masked node's world bounds are never shrunk by the mask, and `ink_bounds` deliberately
    over-covers: "too small is the one error these bounds may not make" (§5.9).
  - Migration **clamps** a stale text-span range rather than rejecting the file (§5.11); a
    dangling guide `owner`, by contrast, is **rejected** rather than silently promoted to global.
  - Case transform is length-preserving only — `ß`/`İ` are left uncased on purpose, rather than
    risk a byte-offset bug in the caret/span system (§15 D80).
  - `build::flatten_union` drops the mask, lock and proportion-lock on purpose.
  - `boolean::evaluate` wraps `flo_curves` in `catch_unwind` because its comparator is not
    transitive (§15 D239) — and `panic = "abort"` would make that guard inert, which is why no
    profile sets it.
  - There is **no PNG golden** and that is a decision, not a gap: per-architecture and
    per-`vello_cpu`-version SIMD residual on stroked paths (§15 D304, D414).
- **A green test may be vacuous, and this project has caught its own three times in one session.**
  For L6, the question is not "does this pass" but "what would also pass this". The three shapes on
  record: it asserted one of two boundaries; it asserted a consequence the wrong implementation also
  produces; its fixture never reached the state it names. An L6 finding names the test that should
  exist, or the assertion that has no teeth — and says which.
- **Most of this project's prose lives where nothing checks it.** `cargo doc` cannot see a
  `#[cfg(test)]` module, the files under `crates/*/tests/` are separate crate roots carrying
  no `deny` at all, and `build.rs` is a third target the gate never reads (§15 D622). The doc comments on tests are where the reasoning, the flip-checks and the ⚠️
  traps live — the sentences most worth trusting and the only ones nothing verifies. Treat a claim
  in a test's doc comment as **unverified** until read against the code.
- **Never bulk-rewrite anything with a script.** CLAUDE.md says why; a `perl -0pi` left a 3,900-line
  source file at zero bytes. The ledger falls under the same rule and is worse off, since it isn't
  in git (reference §5).
- **Never launch the GUI.** Chrome questions are answered with a throwaway headless `#[test]`
  (`OndinApp::headless(&ctx)`) or with `ondin export --svg`, and the probe is deleted before the
  pass returns. CLAUDE.md's "Checking the *chrome* without a window" section lists what `ctx.run_ui`
  will and will not tell you — read it before writing a probe, or the probe will measure the wrong
  thing (a tooltip's ink never arrives; a widget's interaction state is last frame's; `.shapes` is
  already layer-flattened).

## Scope decisions

Carried across rounds unless the user changes them:

- **In scope:** everything under `crates/` — source, inline test modules, and the 21 integration
  test files. Test code is reviewed under L6 and L3; its *prose* is A6/A7's problem.
- **Out of scope:** `docs/` as documents (A7 reviews them only where they contradict the code),
  `design/` (regenerated mockups, not source), `todo.md` (untracked scratchpad), `icons/`,
  `stress.ondin`, `Cargo.lock`, `target/`, and `.claude/agents/*` except where A8 finds a gate list
  there that is wrong.
- **`Cargo.toml`'s dependency notes are in scope** (S22) — they carry the interop pins and two
  measured profile decisions, and a stale one is exactly the drift this review is for.

Round-specific additions go in `plan.md`.

## Then hand over

After A1, tell the user setup is done and that `codebase-review-pass` now runs the remaining passes
— delegating each to a subagent and stopping only at attended passes, triage checkpoints, an
escalated Critical, or a pass that degraded twice. Don't keep going yourself unless asked.

## Hard rules (both skills, every pass)

- **Edit no source file.** This phase produces findings; fixing is a separate phase with its own
  risk buckets. A session that starts fixing loses the ledger and the ranking. A headless probe is
  the one exception and it must be removed before the pass returns.
- **A Critical is escalated, not queued** (reference §3) — written up, surfaced in that pass's
  report immediately, and fixed on `main` (there is no frozen tag to branch from, and CLAUDE.md
  wants small commits rather than a branch). The fix is a separate act from the review: the pass
  that found it still returns findings only, and the ledger entry records the SHA the fix landed at.
- **No scripted or bulk edits** — of source or of the ledger (reference §5).
- **The verification gate is not optional** (reference §3). This codebase is full of deliberate
  conservatism that makes plausible-sounding findings wrong, and it writes the reason down.
- **Cap findings per pass.** More than ~12 means you aren't ranking.
- **Every pass states what it did not cover.** A blank there is a claim of full coverage.
