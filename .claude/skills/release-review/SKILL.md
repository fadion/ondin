---
name: release-review
description: Review everything that changed since the last release, autonomously, before tagging — takes the commit range (default last tag..HEAD, or one you name), cuts it into slices, and runs the whole multi-pass subagent review end to end without checkpoints. Use when the user asks to review the changes since the last release/tag, review what's about to ship, "review before I release", "review v0.1.0..HEAD", or asks for a pre-release audit. Produces a triaged ledger under review/ and stops there — it fixes nothing and releases nothing. For the periodic whole-codebase review use codebase-review; for the working diff use /code-review.
---

# Review a release range

Same method as the periodic codebase review, aimed at one commit range and run **unattended**: one
invocation goes from the range to a triaged ledger without asking the user anything. The user says
"review before the release" once and reads the report.

Two things it does not do, both deliberate: it **edits no source file** — a separate writer session
picks up the ledger, builds a fix plan and implements it — and it **does not release**. The
`release` skill owns the tag.

`../codebase-review/reference.md` is the method spec: lenses (§1), the verification gate (§3), the
per-pass protocol (§4), the ledger format and its integrity rules (§5), triage buckets (§6), the
orchestration contract (§7). **Read it before Phase 1.** It transfers unchanged — everything below
is only where a range review differs from a whole-tree one.

`../codebase-review/SKILL.md`'s **"Project-specific notes for reviewers"** section transfers
unchanged too: the deliberate-conservatism traps listed there are the most common source of wrong
findings in this codebase, and every subagent brief carries them. Don't duplicate that section
here — point at it, so there's one copy to keep true.

## Artifacts

One directory per run, under the already-gitignored `review/`:

| File | Role |
| --- | --- |
| `review/release-<base-tag>/plan.md` | The work order: the range, its diffstat, the slice table, the pass sequence. Generated in Phase 1, never signed off. |
| `review/release-<base-tag>/facts.md` | Pass 0's mechanical fact sheet, scoped to the range. |
| `review/release-<base-tag>/findings.md` | The ledger: header, pass log, findings, `## Rejected`. Final triage prepends `## Triage`. |
| `review/release-<base-tag>/index.md` | One line per finding — `ID \| severity \| origin \| file:line · item \| claim` — plus the rejected one-liners. What each subagent reads instead of the ledger. |
| `review/release-<base-tag>/.snapshots/` | Per-pass ledger snapshots (reference §5). Kept. |

`<base-tag>` is the range's base — `review/release-v0.1.0/` for `v0.1.0..HEAD`. If that directory
already exists from a run at a **different** head SHA, rename it to
`review/release-<base-tag>-<old-head-sha>/` and start clean; same head SHA means resume (below).

**The base need not be a tag.** A mid-cycle run over `<first-sha>^..HEAD` is a normal use — the base
is simply a SHA and the directory is named after it. Such a run reviews a feature, not a release: it
says nothing about whether the tag should go out, and the pre-release run over the full
`<last-tag>..HEAD` still happens and still reads every slice. Being reviewed mid-cycle is never a
reason to narrow that one.

The periodic review's `review/findings.md` is a different, longer-lived ledger — 23k lines, round 1
complete, and **`.gitignore`d, so it has no `git checkout` behind it**. Never write to it, and never
let a subagent near it with a script. Do **read** its `## Rejected` section and its `## Triage — T8`
buckets: re-raising something a previous round already settled wastes the writer session's time.

Pass IDs are `P0`, `R1`–`R3`, `X1`…`Xn` (sub-passes `X3.1`), `T` for triage. ⚠️ **`A*` and `S*` are
both taken** by the periodic review's Tier A sweeps and slice passes, which is why the slice passes
here are `X` — a `[S3-L1-02]` from this run would be indistinguishable from one of round 1's, and
the two ledgers are read together.

### One field the periodic ledger doesn't have

Every finding block carries **`Origin: introduced | pre-existing`**, on the line above `Status`.
Introduced means the range created it or moved it somewhere it now misbehaves; pre-existing means
the pass met it while reading around the change. `git log -S` or `git blame` on the site settles
it, and a finding that genuinely can't be attributed says `pre-existing` — the conservative answer,
since it's the one that doesn't hold a tag hostage on a guess.

It exists because this review answers a question the periodic one doesn't: **should this tag go
out?** A Critical the range introduced is a regression and blocks it; the same severity in code
that shipped two releases ago is a bug worth fixing on its own schedule, and conflating the two
either holds a good release for old news or ships a regression under cover of a long list.
Reference §6's buckets sort by *how to fix safely*, which is a different axis, so both are recorded
and triage reports them crossed. This review is *designed* to surface pre-existing bugs, by reading
whole functions and tracing into unchanged callers; they are welcome, they just aren't blockers.

## Phase 0 — the range and the ground

The range is the skill's argument, or defaults to the last tag:

```bash
git describe --tags --abbrev=0
```

Accept a tag or a SHA as the **base**. The head is always `HEAD` — every command below anchors to
it and Phase 4 stops if it moves, so a `base..head` naming some other head would review one thing
and check another. If the user gives one, say the head is ignored and review to `HEAD`, or have
them check that commit out first.

⚠️ **If there is no tag yet, there is no release range**, and this skill is the wrong instrument:
a base of the repository's first commit is the whole codebase, which is `codebase-review`'s job and
is already done for round 1. Say so and stop, or take an explicit base SHA from the user.

🚨 **This review freezes its range, and the periodic review deliberately does not freeze the tree.**
`codebase-review`'s Phase 0 argues at length for reviewing a moving `main`, on the ground that git
here was a safety net rather than a record. **That ground is gone** — the repository was reset and
republished on 2026-09-16 and history is kept now — and in any case it never applied to a range
review, whose whole subject is a fixed set of commits. Findings still record the enclosing item
beside the line number (`crates/ondin-app/src/canvas.rs:5528 · fn snapped_point`), because a symbol
survives an insertion above it and a line number does not.

Measure the range, and put the numbers in the plan rather than estimating from them:

```bash
git diff --stat <base>..HEAD | tail -1
```

```bash
git log --oneline <base>..HEAD
```

```bash
git diff --numstat <base>..HEAD
```

The ground has to be checkable, in this order. **This is the `release` skill's Phase 0 bar — keep
the two in step, and with CLAUDE.md's gate list, rather than with memory.** With no CI in this
project, a local bar narrower than the release's own passes a tree the release will reject:

1. `git status --short` empty. Reviewing a dirty tree reviews something that isn't shipping.
2. `cargo fmt --all --check` → exit 0.
3. `cargo clippy --workspace --all-targets` → clean, **then per package**:
   `for p in ondin-core ondin-render ondin-export ondin-mcp ondin-app; do cargo clippy -p $p --all-targets; done`.
   The two spellings compile different code, so a warning only one prints is a warning nobody sees.
4. `cargo test --workspace` → green.
5. `cargo doc --workspace --no-deps --document-private-items` → exit 0. **The flag is not optional**:
   without it rustdoc checks no link on any private item, which here is most of them.
6. `cargo check --release -p ondin-app` → the one gate not compiled with `debug_assertions`.
7. `cargo test --workspace --release` → the only thing that *runs* under the release cfg (§15 D597).
8. The two gated suites, run once and **recorded**:
   `cargo test -p ondin-render --release -- --ignored` (GPU — this machine has one, so a
   carried-forward "unverifiable here" is a claim to check rather than accept) and
   `cargo test -p ondin-app fonts -- --ignored` (network). If either was not run, the ledger says
   that path is **unverified** rather than quietly proceeding.
9. Record `git rev-parse HEAD` and the base SHA at the top of the ledger.

**A failure here is one of the run's few hard stops.** Report it and stop — don't fix it, and don't
review past it. A red tree isn't shippable, so a review of it answers a question nobody asked.

Two environment facts that have already cost a pass: on Windows, stop a running `ondin.exe` before
building; and **never launch the GUI** (CLAUDE.md) — it grabs focus and the mouse. A pass with a
question about chrome answers it with a throwaway headless `#[test]` (`OndinApp::headless(&ctx)`,
§15 D303) or with `ondin export`, and deletes the probe before returning.

## Phase 1 — cut the diff into slices

Generate `plan.md` and **proceed** — there is no sign-off. What replaces it is that the slicing is
mechanically checkable, and the plan states the check:

- **Slice by theme, not by file or by commit.** The commit log is the best available grouping
  signal — a range typically reads as a few clusters (model → resolve → render → inspector for one
  feature). One cluster is one slice, and it deliberately spans crates, because in this codebase the
  bugs that matter live in the **core→render→app seam** and at the export boundary.
- **A file belongs to as many slices as it has themes.** This follows from slicing by theme and is
  the rule a file-per-slice instinct breaks: `canvas.rs` is 21k lines and `inspector.rs` 24k, and
  each collects every feature in the range. Assigning such a file to one slice hides the other
  themes' code from the only reviewer who would have recognised it. So the unit is **(file, theme)**,
  and a shared file's plan row names the commits or the line ranges that put it in each slice — use
  `git log --oneline <base>..HEAD -- <path>` to see which themes touched it.
- **Size a pass at ~2.5k changed lines**, `ceil(slice_lines / 2.5k)` passes each, numbered `X3.1`,
  `X3.2`, with **the split point named in the plan**. Changed lines means added + deleted, from
  `--numstat`; for a shared file, count only the hunks that slice owns. Note that this splits a
  slice by *size* and the rule above splits a file by *theme* — a big shared file usually needs both.
- **Order by blast radius.** The document write path, migration, crash recovery, the library store
  and export go first, ahead of larger but inert slices. State the rationale and check the order
  against it.
- **Assign 2–4 lenses per slice**, not all of them to all of them.
- **Coverage is verified mechanically before the plan is written**: every path in
  `git diff --name-only <base>..HEAD` appears in **at least one** slice, and the plan says so with
  the file count. A file in no slice is the failure this catches, and prose like "the effects work"
  is not coverage. A file in several slices is expected, not a defect — but each appearance states
  which hunks it brings, so "covered" never means "somebody looked at the file".
- **…and the file check alone is not enough — check the *hunks*.** For every file in more than one
  slice, the hunks its rows claim must together cover the file's whole diff. A file-level check goes
  green while a remainder nobody owns goes unread: each row names the theme it owns and **nothing
  owns what is left**. Mechanically: `git diff --numstat <base>..HEAD -- <path>` per shared file,
  against the sum its rows claim; a shortfall means a slice needs widening or a row needs adding.
  Say the result in the plan — a file-count line alone is the failure mode this replaces.
- **State the invocation count** (passes + P0 + sweeps + triage) at the top.
- Flag any slice needing the **GPU** (`--ignored` suites) or the network. It still delegates; its
  report must name what it exercised, or say the check was unverified.

## Phase 2 — Pass 0, the mechanical fact sheet

Create `findings.md` **first** (header + pass log with P0 `in progress`) and an empty `index.md`,
then write `facts.md`. Everything here is scoped to the range — the whole-repo census is the
periodic review's job, and repeating it buries the twenty facts that are about this release.

| Row | Kind | Feeds |
| --- | --- | --- |
| `cargo clippy --workspace --all-targets -- -W clippy::pedantic -W clippy::nursery`, filtered to changed files | census | L1/L3 |
| `unwrap()` / `expect()` / `panic!` / raw `[i]` **added** by the range, outside tests | census | L1 panic surface |
| `#[allow(...)]` added | census — each is a silenced signal | L3 |
| `TODO` / `FIXME` / `HACK` added | census | L3 / triage |
| New `src/*.rs` modules, and whether each is named in `docs/architecture.md` | census | L8 |
| D-numbers newly cited from `crates/`, set-differenced against the base, and **which of them resolve to a §15 entry** | census | L8 — a cited number with no entry is invisible to every gate here |
| Test-count delta per changed file (`#[test]` added vs. changed lines) | census | L6 |
| New intra-doc `[` links added inside `#[cfg(test)]` prose or under `crates/*/tests/` | census — **the correct population is zero** (§15 D319, D622) | L8 |
| New `#[ignore]` without a reason string | census | L6 |
| New `///` runs over 52 lines, and whether each describes the item beneath it | **candidate list** — judgment deferred | R2 (doc-comment theft) |
| New `NodeKind` / `Operation` variants, and every predicate that should have gained an arm | **candidate list** — counts and sites only | R1 |
| New `&mut self` decision methods on `OndinApp` | **candidate list** | R1 — no test can observe their call sites (§15 D269) |

Deletions are facts too: a **removed** test, a removed guard, a removed `#[cfg(test)]` module
belongs in the census beside the additions. Note explicitly whatever failed to run — a silently
dropped census reads later as a clean result.

## Phase 3 — the range sweeps

Three, in order. Each is one delegated pass with a ledger row.

| # | Sweep | Lenses | Shape |
| --- | --- | --- | --- |
| **R1** | **Invariant conformance on what changed** | L2 | Walk `docs/architecture.md`'s eleven architecture invariants (`../codebase-review/SKILL.md` has the evidence set for each) and, for each, enumerate the sites **the range added or moved** — not the whole population, which is the periodic review's A1. New `NodeKind`/`Operation` variants against every predicate that switches on them; new geometry against the pivot and escape rules; new paint against the resolve boundary; new writes against the atomic temp-then-rename rule; new document mutations against undo. Ask `design-oracle` for a rule rather than paging `architecture.md` in. This is the highest-yield sweep on a feature range and it is cheap, because the candidate sites are already in `facts.md`. |
| **R2** | **Doc drift & test map** | L8, L6 | `docs/architecture.md` is this project's specification and a large range is where it goes stale: every claim the range touched, checked against the code that now exists; every §15 entry the range contradicts — **including entries it silently widened a departure from**, which no gate can see; the doc-comment placement candidates from `facts.md`, each read against the item beneath it; new decision functions against the tests that should exist; and whether the invariant-enforcing tests still enforce over the new surface (the export goldens, `deps_forbidden`, the `panic = "abort"` `compile_error!`). |
| **R3** | **Document safety across the range** | L5 | Run it **only if** the range touches the save path, schema migration, crash recovery, the library store, autosave, image decode, SVG import, or file/path handling — the plan decides from `--name-only` and says which. Trace end to end rather than per slice: what a new write can destroy, what a malformed import can do, whether a migration can lose a field, whether recovery can offer a snapshot it should have dropped, what a new error message leaks about a path. |

R1's findings are **candidates, not settled facts**, and the slice-pass briefs say so — in the
periodic review that sweep is attended precisely because a wrong call there gets read as settled by
everything after it. Unattended, the mitigation is that each later pass applies §3 independently
rather than inheriting R1's conclusions.

## Phase 4 — the slice passes

The loop from reference §7, with the checkpoints removed. Per iteration:

1. `git rev-parse HEAD` still matches the ledger's head SHA. If it moved, **stop** — every
   `file:line` is anchored to it, and a range review can't re-anchor itself while it runs.
2. **Audit the previous row**, mechanically, without reading the findings: the expected number of
   blocks with that pass's ID prefix landed, each carries Failure / Evidence / Confidence / Origin /
   Status, "Not covered" is non-empty, the row isn't still `in progress`, and the §5 integrity
   checks pass. This audit is the whole of the quality control an unattended chain has — a degraded
   pass propagating unnoticed is the failure mode of the shape.
3. Pick the first row that isn't `done`. Mark it `in progress`; snapshot the ledger to
   `.snapshots/findings-<pass-id>.md`.
4. Dispatch the subagent (brief below). Wait.
5. **Guards on return:** `git status --short` must be empty — the run directory is gitignored, so
   anything there means the subagent edited source or left a probe behind: revert it with an `Edit`,
   record `attempt 2`, re-run the pass. Then the §5 ledger checks against the snapshot.
6. Close the row (counts by severity + what it didn't cover), append its findings to `index.md`,
   loop.

**Cap re-runs at two.** A pass that degrades twice is degrading structurally — slice too big, wrong
split point, wrong lens — so stop and say which you think it is. That is a hard stop.

Passes stay **sequential**, for reference §7's reason: the risk ordering is load-bearing and
cross-slice duplicates are common.

## Phase 5 — final triage

Delegated like everything else — at this scale (typically 5–9 passes, capped at 12 findings each)
there is no volume problem, and no interim checkpoint unless the ledger passes ~60 open findings, in
which case insert one triage pass midway.

Triage reads what's in the ledger whole, dedupes across slices (one root cause commonly surfaces in
three), re-ranks globally, and prepends a `## Triage` section using reference §6's buckets. It
changes no code and no source file. It states its expected line delta before editing, since it is
the one pass allowed to *move* entries rather than only append.

`## Triage` opens with the **release verdict**, before the buckets: every `Origin: introduced`
finding at Critical or High, listed, with the sentence that the tag should wait for them — or an
explicit "nothing introduced by this range blocks the tag" when there are none. That line is what
the user came for, and it must be readable without scrolling into the buckets. Everything else,
pre-existing included, is then bucketed as normal; a pre-existing Critical is called out as urgent
but *not* as a blocker, and triage says which it is rather than leaving the reader to infer it.

The buckets are what the writer session reads first, so the triage output must be actionable
without the reviewer present: each entry keeps its ID, severity, `file:line · item`, origin, the
concrete failure, and the evidence — never a bare claim.

## Reviewing a diff, not a tree

The rules that don't come up in a whole-tree review, and that a diff reviewer gets wrong by default:

- **The hunk is not the slice.** A reviewer who reads only changed lines misses every bug that is
  visible only in the surrounding code. Read each changed function whole, and trace it into the
  layer on either side. This is the single highest-value rule here.
- **Deletions are findings too.** A removed guard, a removed test, a dropped `?`, a narrowed match
  arm — `git diff` shows them and a reader scanning for new code skips them.
- **The commit message is a claim to check, not context to accept.** A commit whose message and diff
  disagree is a finding.
- **Churn is a signal.** A file touched by several commits in the range was hard to get right; read
  it more carefully than its final diff suggests. `git log --oneline <base>..HEAD -- <path>` gives
  the count.
- **Interaction with unchanged code is in scope.** New code that is correct in isolation and wrong
  against an existing caller is the characteristic bug of a feature range, and it is invisible from
  the diff alone. Here that most often means a new `NodeKind` or `Operation` variant and the
  predicate that quietly kept its old arm.
- **A narrowed predicate is a finding even when the narrowing is right.** §15 D591 narrowed font
  invalidation correctly and turned an omission the coarse version had covered by accident into a
  live bug. Every omission in a narrowed predicate is invisible while the predicate is coarse.
- **Golden-backed gates need their goldens extended, not worked around.** New export fidelity with
  no new fixture is an L6 finding.

## Autonomy

The periodic review's attended set is replaced, not dropped:

| Was attended | Replaced by |
| --- | --- |
| Plan sign-off | Mechanical coverage check, stated in `plan.md` with the file count and the per-file hunk sums |
| A1 invariant census | R1 delegated, its findings marked as candidates that later passes re-verify under §3 |
| Triage checkpoints | One delegated final triage; an interim one only past ~60 open findings |
| Critical escalation breaks the loop | Recorded, marked in the ledger, and **led with in the final report** — the run is one session, so stopping buys nothing that reporting doesn't. An introduced Critical additionally opens the triage verdict |

**The only hard stops:** a Phase 0 ground failure; HEAD moving mid-run; a pass degrading twice; a
ledger integrity failure that a snapshot restore doesn't fix. Everything else is recorded and the
run continues.

## Resuming

Re-invoking the skill with the run directory present and the head SHA unchanged resumes from the
pass log — that is the cursor, and it's on disk precisely so a lost session costs one pass. Don't
regenerate `plan.md`, and don't re-run `P0` if its row says `done`. A head SHA that has changed is a
different review: archive and start clean (see Artifacts).

⚠️ **A subagent can die mid-job and leave the record claiming work it did not do** — a rate limit
stopped one `arch-scribe` run after it had written five entries and four claims about passages it
never reached. After any pass that stops early, diff what it claimed against what it wrote;
`git status --short` plus the snapshot comparison is the whole check.

## The subagent brief

A `general-purpose` subagent (Read/Grep/Glob plus Bash for `git` and probe harnesses). It inherits
nothing:

```
You are running pass <ID> of an automated pre-release review of Ondin, covering the commit
range <base>..<head>. Read these first:

- .claude/skills/codebase-review/reference.md — §1 lenses, §3 the verification gate, §4 the
  per-pass protocol, §5 the ledger format and its integrity rules. Follow them exactly, with one
  substitution: wherever it names `review/findings.md` or `review/index.md`, the file is
  <run-dir>/findings.md or <run-dir>/index.md. The paths it hardcodes belong to a different,
  longer-lived ledger that this run must not touch.
- .claude/skills/codebase-review/SKILL.md — "Project-specific notes for reviewers". The
  deliberate-conservatism traps listed there are the most common source of wrong findings, and
  §15's four verdicts mean a live entry is never a bug to re-raise.
- .claude/skills/release-review/SKILL.md — "Reviewing a diff, not a tree". Those rules are what
  this pass is for.
- <run-dir>/index.md — every finding raised so far, one line each, plus the rejected ones. Check
  it before writing: a candidate already there is a duplicate (say so in your report instead of
  re-raising) or already rejected (don't re-raise it at all).
- <run-dir>/facts.md — Pass 0's census. Cite it; never restate one of its linter items as a
  finding.

Ask the `design-oracle` subagent what the design says rather than reading docs/architecture.md or
docs/decisions.md yourself — together they are ~56,000 lines and paging them costs the pass.

Your pass:
  Slice:  <slice name>
  Range:  <base>..<head>
  Files:  <files + the split point if this is a sub-pass. A file marked "shared" is in another
           slice too, for its other theme — review only the hunks listed here and leave the rest;
           another pass owns them.>
  Lenses: <only these — do not carry others>
  <"Needs the GPU suite / the network suite. State in your report what you exercised; if it was
   not runnable, say the check is unverified rather than omitting it." — if flagged>

Protocol: start with `git log --oneline <base>..HEAD -- <files>` and `git diff <base>..HEAD --
<files>` to see what changed and what each commit claimed; then read each changed function WHOLE
in the current tree, not just its hunks, and trace 2-3 flows end to end across the core->render->app
seam. Deletions and interactions with unchanged callers are in scope. Draft candidates with their
lens's required evidence; verify each against §3 before it enters the ledger; write survivors
ranked, capped at 12.

Anchor every finding on the enclosing item as well as the line —
`crates/ondin-app/src/canvas.rs:5528 · fn snapped_point`. A symbol survives an insertion above it.

Every finding block carries one extra line above **Status**:

  **Origin:** introduced | pre-existing — <how you decided>

"introduced" means this range created the fault or moved it somewhere it now misbehaves;
"pre-existing" means you met it while reading around the change. Settle it with `git log -S` or
`git blame` on the site, not by whether the line appears in the diff — a caller the range didn't
touch can be the one that breaks. If you genuinely can't attribute it, say pre-existing: that is
the answer that doesn't hold a release on a guess. Pre-existing findings are wanted, not noise —
this pass is meant to read past the hunks — so record them at their real severity and let triage
decide what blocks the tag.

<Only for slice passes:> R1's invariant findings are candidates, not settled facts — re-verify
independently rather than inheriting them.

Answering a question about chrome: NEVER launch the GUI. Write a throwaway headless #[test]
(`OndinApp::headless(&ctx)`, §15 D303) or use `ondin export`, and DELETE the probe before you
return — `git status --short` must come back empty.

Writing to <run-dir>/findings.md: append your block at the "**Status:** open" / "---" /
"## Rejected" boundary, using Edit with that anchor. NEVER mutate that file with a shell script,
sed, perl or a Python rewrite — CLAUDE.md bans splicing outright and a PowerShell rewrite destroyed
a 3,900-line file on this machine. Do not touch the pass log or index.md; the orchestrator owns
those. Rejected candidates go in ## Rejected, one line each on why.

Hard rules: edit no source file — findings only, no fixes, no scripted or bulk edits anywhere.
A Critical (document loss, corruption, silent data loss on export or migration) goes in the ledger
AND prominently in your report.

Return a compact report, NOT the findings themselves:
  - pass ID and counts by severity (C/H/M/L/D), and how many of those are introduced
  - each finding: ID, severity, origin, file:line · item, one-line claim
  - "Not covered": files skimmed, flows not traced. Never blank — a blank is a claim of full
    coverage.
  - an explicit statement that every finding cleared §3, and how many candidates you rejected.
```

## The report

One report, at the end. ~12 lines, and it **opens with the release verdict** — what the range
introduced at Critical or High, or that it introduced nothing that blocks the tag. Then: the range
and its size, the passes that ran, findings by severity split introduced vs. pre-existing, the
triage buckets' counts, where the ledger is, and anything the run could not verify. Between passes
stay quiet — a line per pass at most.

Close by naming the two follow-ups the review deliberately doesn't do: a writer session picks up
`<run-dir>/findings.md` for the fix plan, and the `release` skill cuts the tag once the fixes land.

## Hard rules

- **Edit no source file** — this session or any subagent. Findings only. Fixing is a separate
  session's job and mixing them loses the ledger and the ranking.
- **Never mutate any ledger from a shell script** (reference §5, and CLAUDE.md's splicing ban);
  snapshot before every pass; append, never rewrite.
- **The verification gate is not optional** (reference §3). This codebase is deliberately
  conservative in specific places and each one is a trap for a plausible-but-wrong finding.
- **Cap 12 findings per pass.** More means the pass isn't ranking.
- **Every pass states what it did not cover.** A blank there is a claim of full coverage.
- **Every finding states its `Origin`.** Without it the ledger can't answer whether the tag should
  go out, which is the only question this review exists for.
- **Don't touch `review/findings.md`** — that's the periodic review's ledger, it is gitignored, and
  it has no `git checkout` behind it. Read-only.
