---
name: commit
description: Ondin's commit flow — group the working tree into logical changes, run the pre-commit bar, then write the Conventional Commits message. Use whenever the user asks to commit ("commit this", "commit the changes", "make a commit", "commit what we just did"), including when they ask only for the message. Also carries the single-pass prompt for reviewing a range of commits, for when the user asks for one. Not for pushing, tagging or releasing — that's the `release` skill — and not for the full pre-release audit, which is `release-review`.
---

# Committing in Ondin

The order is **group → verify → commit**.

**Never commit unless the user asked.** Invoking this skill is the ask. Making
edits earlier in the session is not, and neither is finishing a task — leave the
work in the tree and say it's ready.

🚨 **This is a reversal, and a session that has read older prose may believe the
opposite.** Until 2026-09-16 CLAUDE.md said *"commit freely and without asking …
how commits are grouped and what the messages say does not matter"*, because the
repository was scaffolding due to be deleted. It was deleted, and the one that
replaced it is published at `https://github.com/fadion/ondin` and kept. **If you
find a passage anywhere in this project that licenses a checkpoint commit, it is
stale — and it is worth correcting rather than working around.**

**Reviews are the user's call, and this skill never makes it.** Don't weigh
whether a change has earned one, don't offer, don't flag a surface as
review-worthy, and don't note a deferral to raise later. When they do ask, the
two shapes are in *Reviewing on request* below.

This skill does not push. Unpushed commits batch for the next tag, and `release`
owns everything from the push onward.

## Phase 0 — see what is actually changing

```bash
git status --short
```

```bash
git diff --stat
```

Then read the diff itself — not the stat. Both judgements left in this skill —
how the tree splits into logical changes, and what the message says the change
does — need to know what the code actually does, and neither can be made from
filenames.

**If something is already staged, that is the user's grouping.** Respect it:
review and commit only what's staged, and read it with `git diff --staged` — a
bare `git diff` shows the *unstaged* remainder, which is the opposite set. Mixing
the two up produces a message describing code that isn't in the commit.

**One logical change per commit.** If the tree holds two unrelated ones, say so
and propose the split rather than sweeping them into a message vague enough to
cover both. The user may well want them together — that's their call, but make it
a call rather than an accident.

### 🚨 Staging, and the subagent race

**Never `git add -A` while a subagent is running.** `arch-scribe` writes `docs/`
*and* corrects `crates/` comments when the brief invites it to — which is the
single most valuable thing it does — so a sweeping stage picks up its
half-finished work and commits it under an unrelated message. This has happened
four times across sessions 14 and 19 (CLAUDE.md, *Git*). It used to cost nothing
because the history was disposable. It now produces a permanent, published commit
that misdescribes itself.

**Stage the specific paths this change touched**, or wait for the agent to
return. `git add crates/` is *not* the mitigation — that was the fix that failed.

The sin is sweeping, not the flag: when Phase 0 established that the whole tree
is the one change and nothing else is running, `git add -A` is the honest way to
say so.

## Phase 1 — the bar

`rust-verify` runs these and returns a distilled report instead of pages of cargo
output — prefer it to running them inline, and read what it says about each gate
rather than just its verdict.

```bash
cargo fmt --all --check
```

```bash
cargo test --workspace
```

```bash
cargo clippy --workspace --all-targets
```

```bash
cargo doc --workspace --no-deps --document-private-items
```

Four notes, each of which has cost this project a green run that was not green:

- **`--all-targets` is not decoration.** Without it clippy never lints a single
  `#[cfg(test)]` module, which is where most of this project's assertions live
  (§15 D302).
- **…and it is not sufficient either. Re-run clippy on the crate you edited**:
  `cargo clippy -p ondin-core --all-targets`. The workspace spelling and the `-p`
  spelling compile *different code* — workspace feature unification changes a
  dependency's layout, so a lint whose threshold the type straddles fires under
  one and not the other (CLAUDE.md, measured: `peniko::Gradient` is 168 bytes
  under `-p` and 160 under `--workspace`).
- **Run the per-package clippy *after* the test build, not beside it.** The first
  run in a batch is the one that compiles the test target, so lints on a test
  written minutes earlier land on the *second* invocation.
- **`--document-private-items` is not optional.** Without it rustdoc checks no
  link on any private item, which here is most of them — and the doc gate is the
  one gate that checks the record against the code.

`cargo check --release -p ondin-app` for anything more than a few lines: it is
the only gate not compiled with `debug_assertions`, and a line behind egui's own
`#[cfg(debug_assertions)]` can break the release build with everything else
green (§15 D161).

**A red bar is a stop.** Report what failed and leave the tree alone. Don't commit
broken work "so it isn't lost" — the working tree is already that.

A `docs/`-only or `CLAUDE.md`-only change may skip the bar; say that you skipped
it and why, rather than silently reporting a commit as verified. **A change that
touches only doc *comments* in `crates/` may not** — that is precisely what
`cargo doc` gates.

### Two checks no gate performs

Both are about damage that compiles, tests, lints, formats and passes `cargo
doc`, and that is now permanent once pushed.

- **If this change inserted items into a module, check the neighbours kept their
  doc comments.** Anchoring an `Edit` on the item below steals its `///` block;
  twenty-two instances are on CLAUDE.md's list, twelve of them committed by a
  session that spent the day fixing that exact class. Mechanically, over the
  items you inserted:
  `grep -n -B3 "^\(pub \)\?\(fn\|struct\|enum\|const\|type\) <neighbour>"`.
- **If a flip-check was reverted by hand, `git diff` the file against `HEAD`
  before staging.** Two committed defects came from re-adding a block *beside*
  the copy that survived — a duplicated comment no detector in this project can
  see.

### If the change earned a §15 entry

A decision entry and its citation belong with the change, not after it. Before
writing a line of it: take the D-number block from `docs/decisions.md` §15.0's
**last index line**, run a negative grep over the range to confirm it is free,
and write the numbers down. Then route the prose through `arch-scribe`, which
also strikes the `docs/roadmap.md` row the change closes — and **read its closing
notes**, because it checks the brief against the code and has caught a wrong
mechanism, a wrong citation and a live bug that way.

Entry and code may be one commit or two adjacent ones. What they must not be is
one pushed and one forgotten: a D-number cited from code with no entry behind it
is invisible to every gate here (CLAUDE.md records that hole twice).

## Reviewing on request

**Not a phase.** Nothing here runs unless the user asks. Two shapes, chosen by
*what* is being reviewed.

### The working tree, before it lands

The usual case, and the reason to ask before committing rather than after —
nothing is committed yet, so findings can change the code instead of following it.

```bash
/code-review high (working tree only — git diff HEAD, not the unpushed range)
```

**Name the scope, not just the level.** Left to itself the review takes
`git diff @{upstream}...HEAD` and folds the working tree in on top. Commits batch
on `main` here for the next tag, so `@{upstream}` is however many commits back
the last push was, and a pre-commit review would read all of them alongside the
two files in question. None of `/code-review`'s documented targets spells
"working tree", so the scope goes in the argument as plain text.

Always name a level: a bare `/code-review` silently reuses the last one typed.
**`ultra` you cannot launch** — it is user-triggered and billed, so print the
command and say it has to come from them.

`--fix` writes to the working tree, so **re-run Phase 1** afterwards and re-read
the diff: the message you were about to write may no longer describe the change.

### A range of commits that already landed

A multi-commit feature the user wants looked at now that it is whole.
`/code-review` cannot do it — it reads the *working diff*, and those commits
aren't in it. Find the range:

```bash
git log --oneline origin/main..HEAD
```

Take the first commit of the feature and review `<first-sha>^..HEAD` in **one
pass** — in this context, or a single subagent if the range would crowd it — with
this brief:

> Review the commits in `<range>` as one pass — no slicing, no ledger. Read
> `git diff <range>` in full, plus enough surrounding code to judge it. Focus on
> three things, in order: **correctness bugs** (wrong conditions, missing guards,
> broken callers, the `&mut self` call sites no test can observe), **the
> invariants in `docs/architecture.md`** for the surfaces the range touched —
> ask `design-oracle` rather than paging the file in — and **document safety**
> (the save path, migration, recovery, export) where it touches them. Report
> findings inline, most severe first, each with the concrete scenario in which
> the code misbehaves. Don't fix anything.

**One pass, deliberately.** `release-review` is the wrong instrument here: its
cost is dominated by fixed overhead — a fact sheet, three range sweeps, a triage
— which runs whether the range is two commits or thirty. A single reader finds
less than a fan-out; that is the trade, made knowingly, and `release-review`
reads the same commits again before anything ships.

**Nothing is recorded** — no file, no ledger. Report the findings in the
conversation and let the fixes land as ordinary commits.

## Phase 2 — the commit

Format, per CLAUDE.md, which stays the authority:

- `type(scope): subject` — imperative, no trailing period, lower-case after the colon
- types: `feat` `fix` `refactor` `perf` `docs` `test` `chore` `build`
- scope = the crate or module the change centers on. `crate/module` where it
  helps — `core/text`, `app/canvas`, `render/effect`, `export/svg` — or the bare
  crate (`core`, `render`, `export`, `app`, `mcp`) when it is wider than one
  module. Omit only when genuinely cross-cutting.
- optional body after a blank line, explaining the **why** — the diff already
  says what. Cite the §15 entry or the finding id where one exists.
- **no trailer.** No `Co-Authored-By`, no "Generated with Claude Code" — turned
  off globally on 2026-09-16.

```bash
git log --oneline -20
```

Read that before writing the subject. Subjects here say what the change does
*for the codebase*, often with the consequence attached, rather than narrating
the edit. A new subject should read like its neighbours.

Multi-line messages never go through `-m`: PowerShell reads a `>=` inside the
text as a redirect and the message arrives split into pathspecs. Use `-F`, and
prefer stdin over a temp file nobody remembers to delete — the Bash tool is a
POSIX shell, so a heredoc works:

```bash
git commit -F - <<'EOF'
type(scope): subject

Body.
EOF
```

Three things this phase never does: stage work that isn't part of this change;
touch `[workspace.package].version` (bumps are explicit-only and belong to
`release`); or run `git tag` / `git push`.

## Finishing

Report, in a few lines: the subject and short SHA, what the bar ran and what it
said, and — only if a review actually ran because the user asked for one — that
it ran and what came of it. If any part of the bar was skipped, name it and why.
If the tree still holds unrelated changes you deliberately left out, say what's
still uncommitted.

Nothing here reports on a review that *didn't* run. There is no offer to decline
and no deferral to track, so "no review was warranted" is not a line this skill
has to write — it is a judgement it doesn't make.
