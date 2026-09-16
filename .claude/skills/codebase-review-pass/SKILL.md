---
name: codebase-review-pass
description: Run the in-progress codebase review forward (work order in review/plan.md, ledger in review/findings.md) — dispatches each pass to a subagent and keeps going until an attended checkpoint. Use whenever the user says "next", "next pass", "continue", "keep going", or "run the review" while a review is underway, and to run the final triage once the last slice pass is done. If review/plan.md doesn't exist yet the review hasn't been set up — use the codebase-review skill instead. Not for pull-request or working-diff review.
---

# Drive the review forward

The user should have to say **"next" a handful of times per review, not seventy**. Which pass comes
next, and how to run it, are both on disk. **Never ask the user which pass comes next — read it.**

Each pass runs in its own **subagent**. This session is the orchestrator: it reads the plan row,
dispatches, audits the result, closes the row, and moves on — it does not read findings blocks in
full, which is what lets it drive a whole segment without drowning. Method and rationale:
`reference.md` §7.

## State

| File | Role |
| --- | --- |
| `review/plan.md` | The work order: slices, lens assignment, attended/delegated flags, triage schedule. |
| `review/findings.md` | The ledger. Its **pass log** is the cursor, and its `SHA` column says which commit each pass read — the tree is deliberately **not** frozen here. |
| `review/index.md` | One line per finding + the rejected one-liners. This session owns it. |
| `review/facts.md` | Pass 0's fact sheet — cite it, don't re-derive it. |
| `review/.snapshots/` | Per-pass ledger snapshots. Kept. |
| `.claude/skills/codebase-review/SKILL.md` | This project's preconditions, the A1 evidence set, the reviewer notes, scope. |
| `.claude/skills/codebase-review/reference.md` | The method: lenses (§1), the gate (§3), the per-pass protocol (§4), the ledger + its integrity rules (§5), triage (§6), orchestration (§7). |

If `review/plan.md` is missing, the review isn't set up — say so and point at the `codebase-review`
skill. If it exists but `review/findings.md` doesn't, the ledger was never created: create it
(opening SHA + the not-frozen note + an empty pass log with its `SHA` column, per reference §5) and
start at Pass 0. Don't
assume the census ran — with no ledger there's no record either way, and re-running Pass 0 is cheap
next to a review built on a census nobody has.

## The loop

Reference §7's loop, run until a stop condition fires. Per iteration:

1. **Record the ground, don't assert it** — `git rev-parse --short HEAD` into this pass's row. The
   tree is deliberately unfrozen (`codebase-review/SKILL.md` Phase 0), so a moved `main` is normal
   and is **not** a stop condition. If it moved since the previous pass, run
   `git diff --name-only <prev>..HEAD -- crates/` and note in the row which slices' files changed;
   any finding already raised against one of them gets re-checked at the next triage, not now.
2. **Audit the previous row** (§7), then the §5 integrity checks against its snapshot. Truncated,
   unverified, or short of what it reported ⇒ **re-run that pass instead of moving on**, and say
   why. **Cap re-runs at two**, recorded in the row (`attempt 2`) — twice means the slice, the
   split point or the lens is wrong, so stop and say which you think it is.
3. **Pick the next pass** — the first in the plan's sequence whose row isn't `done`. Attended, or a
   triage checkpoint (reference §6) ⇒ stop and hand back. Every slice pass done ⇒ what's next is
   the **final triage**, also attended.
4. **Mark it `in progress`** with its SHA, then snapshot to
   `review/.snapshots/findings-<pass-id>.md`, and **capture `git status --short`** — you need the
   before-picture for step 6. A pass that dies mid-way re-runs rather than being skipped, and the
   row is what decides that.
5. **Dispatch the subagent** (template below) and wait.
6. **Guards on return** (§7): `git status --short` must match what step 4 captured. Not "empty" —
   the user may have work in progress on an unfrozen tree. `review/` is gitignored, so any **new**
   entry is the subagent's: revert that entry, record `attempt 2`, re-run. Then the §5 ledger checks
   against the snapshot.
7. **Close the row** — findings by severity and what the pass did *not* cover, from its report —
   append them to `review/index.md` one line each, and loop.

## Stop conditions

Stop the loop, report, and hand back on any of:

- the next pass is **attended** or is a **triage checkpoint**;
- the subagent reports a **Critical** — surface it in your report immediately and say it warrants
  fixing now, on `main` (there is no frozen tag here). An orchestrator that keeps going has
  re-buried the thing escalation exists for;
- a pass **degraded twice**;
- a guard failed in a way you can't resolve.

**A moved `HEAD` is not a stop condition** — it is recorded in the row (step 1). The upstream method
stops there because it freezes; this repo does not.

## Reporting

When you stop, ~8 lines: which passes ran, findings by severity across them, the single most
important one, what's next, and anything needing the user's judgment. Between passes, stay quiet —
a line per pass at most.

## The subagent brief

Use a `general-purpose` subagent (it needs Read/Grep/Glob plus Bash for probe harnesses and cargo).
It inherits nothing from this session, so the prompt carries everything:

```
You are running pass <ID> of an in-progress codebase review of Ondin, a Rust 2D design editor
(egui/wgpu/vello). Read these first:

- .claude/skills/codebase-review/reference.md — §1 lenses, §3 the verification gate,
  §4 the per-pass protocol, §5 the ledger format and its integrity rules. Follow them exactly.
- .claude/skills/codebase-review/SKILL.md — "Project-specific notes for reviewers". The
  deliberate-conservatism traps listed there are the most common source of wrong findings, and
  the note about §15 verdicts is the second.
- docs/architecture.md §3 (layering) and §4 (the 11 invariants). L2 is the highest-yield lens
  here. Use the `design-oracle` subagent to ask what the design says rather than paging
  architecture.md or decisions.md into your own context — together they are 31k lines.
- CLAUDE.md — the editing rules, the gates and their known holes, and "Checking the chrome
  without a window" if your pass needs a measurement.
- review/index.md — every finding raised so far, one line each, plus the rejected candidates.
  Check it before writing anything: if your candidate is already there, it's a duplicate (say so
  in your report instead of re-raising it) or already rejected (don't re-raise it at all).
- review/facts.md — Pass 0's census. Cite it; never restate one of its rows as a finding.

Your pass:
  Slice:  <slice name>
  Files:  <files + the split point (line ranges) if this is a sub-pass>
  Lenses: <only these — do not carry others>
  <"This pass needs a GPU adapter / the network. State in your report which suite you ran." —
   only if the plan row flags it>

Protocol: read the whole slice before writing anything; trace 2-3 flows end to end (the real bugs
in this codebase live in the model->resolve->scene->render seam and the gesture->preview->
transaction seam, not inside one function); draft candidates with their lens's required evidence;
verify each against §3 before it enters the ledger; write survivors ranked, capped at 12.

Before claiming a gap, search for the guard. Before claiming a deviation, check §15 — a *Keep*,
*Intentional* or *Deviation* entry is the design accepting the current code, not an open defect,
and *Resolved* is history. Ask design-oracle if you are unsure.

Anchor every location on its enclosing item, not on a line number alone — the tree is NOT frozen
for this review, so line numbers drift and symbols do not. Write
`crates/ondin-app/src/canvas.rs:5528 · fn snapped_point`, and do the same for every line ref
inside Evidence. Where there is no enclosing item, quote the line of code.

Writing to review/findings.md: append your findings block at the "**Status:** open" / "---" /
"## Rejected" boundary, using Edit with that anchor. Prefer Edit over a heredoc — heredocs here
have failed on long bodies; if you use one, Write the block to the scratchpad first and
`cat scratch >> review/findings.md`. NEVER mutate that file with a script that rebuilds it
(reference §5). Do not touch the pass log or review/index.md; the orchestrator owns those.
Rejected candidates go in the ## Rejected section, one line each on why.

Hard rules: edit no source file — findings only, no fixes, no scripted or bulk edits anywhere.
Never launch the GUI (CLAUDE.md forbids it; it hijacks the user's screen). A throwaway headless
`#[test]` is the allowed way to measure chrome, and you must delete it before you return —
`git status --short` must come back empty. Don't restate a Pass-0 item (review/facts.md) as a
finding. If you find a Critical (document loss or corruption, a lost export, a credential or
path-traversal issue), write it to the ledger and say so prominently in your report — it gets
escalated immediately, not at triage.

Return a compact report, NOT the findings themselves:
  - pass ID and counts by severity (C/H/M/L/D)
  - each finding: ID, severity, file:line, one-line claim
  - "Not covered": files skimmed, flows not traced. Never blank — a blank is a claim of full
    coverage.
  - an explicit statement that every finding cleared §3, and how many candidates you rejected.
```

## Hard rules

- **Edit no source file** — this session or any subagent. Findings only; a probe is removed before
  the pass returns.
- **Escalate a Critical in the pass that finds it**, and break the loop.
- **Never mutate the ledger from a shell script** (reference §5), and snapshot before every pass.
- **The gate is not optional.** This codebase is full of deliberate conservatism that makes
  plausible-sounding findings wrong — and it writes the reason down, so the guard is findable.
- **Cap at 12 findings per pass.** More means the pass isn't ranking.
- **Append to the ledger, never rewrite it.**
- **Read `docs/architecture.md` §4 (via design-oracle) before any pass carrying L2** — that lens is
  usually the highest-yield one and it's entirely project-specific.
