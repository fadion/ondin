---
name: release
description: Ondin's push-and-release flow — verify the local bar, push main, bump the workspace version, tag, build the binary, create the GitHub release and write its notes. Use whenever the user asks to cut, tag, ship, or publish a release, or says anything like "push and bump to 0.2.0", "bump to v1.2.0 and push", "release 0.1", "tag a new version", or "ship this". Also use it when they ask only for part of the flow (just the bump, just the release notes) so the surrounding steps and their gates aren't skipped by accident.
---

# Cutting an Ondin release

The flow is **verify → push → bump+tag → build → publish**.

🚨 **There is no CI, so the local bar is the only gate there is.** Schemaic's
version of this skill waits on a GitHub Actions run between the push and the tag;
Ondin has no `.github/` at all. Nothing downstream will catch what Phase 0 misses,
which makes Phase 0 the whole safety of this flow rather than a convenience —
**never trim it to save time.** If the user is in a hurry, tell them what is
running rather than tagging blind.

*(When CI does arrive, the gate goes between Phase 1 and Phase 2 — push, watch
the run, tag only on green — and the phases below are numbered to leave room for
it.)*

Report the version and what is about to happen before Phase 2: the tag push is
the first irreversible step.

## Phase 0 — the ground

```bash
git status --short
```

The tree must be clean. Uncommitted work means the user has something in flight:
stop and ask rather than sweeping it into the release.

Then the bar. These are the project's own gates from CLAUDE.md — check that list
rather than this one if they disagree, and carry the correction back here.
`rust-verify` runs most of them and returns a distilled report.

```bash
cargo fmt --all --check
```

```bash
cargo clippy --workspace --all-targets
```

```bash
for p in ondin-core ondin-render ondin-export ondin-mcp ondin-app; do cargo clippy -p $p --all-targets; done
```

```bash
cargo test --workspace
```

```bash
cargo doc --workspace --no-deps --document-private-items
```

```bash
cargo check --release -p ondin-app
```

```bash
cargo test --workspace --release
```

```bash
cargo deny check licenses
```

Why each of the surprising ones is there, so none of them gets dropped as
duplication:

- **The per-package clippy loop** is not the workspace run repeated. The two
  spellings compile different code — feature unification changes a dependency's
  layout — so a warning that only one of them prints is a warning nobody sees
  (CLAUDE.md, measured 2026-09-01 and explained 2026-09-08).
- **`--document-private-items`** is what makes the doc gate read anything: most
  items here are private, and without the flag rustdoc checks none of their
  links. A stale doc link is the cheapest drift to introduce and the most
  expensive to trust.
- **`cargo check --release`** is the only gate not compiled with
  `debug_assertions` (§15 D161).
- **`cargo test --workspace --release`** is the only thing that *runs* anything
  under the release cfg. `check --release` is scoped `-p ondin-app` and compiles
  no other crate's test targets. The class it catches is a test whose *meaning*
  changes with the profile — egui defaults `warn_on_id_clash` to
  `cfg!(debug_assertions)`, which made one test red with nobody looking (§15
  D597). Cold this is ~96 s and 2.4 GB of a second profile's artifacts; for a
  release that is cheap.
- **`cargo deny check licenses`** is what keeps the MIT claim true. A release is
  the moment a `cargo update` since the last one can have pulled a copyleft crate
  into a binary you are about to hand people. `THIRD-PARTY-NOTICES.md` is the
  companion: if the tree gained a **bundled asset**, the notices need an entry and
  `cargo-deny` cannot tell you that — it only sees crates.
  ⚠️ **Check `licenses` specifically, not a bare `cargo deny check`.** The full
  command also runs `advisories`, which is **deliberately red** on two quick-xml
  entries that reach no Windows build and cannot be updated from here —
  `deny.toml` carries the analysis. Read that block before reacting to the colour,
  and treat any *new* advisory as a conversation with the user rather than
  something this skill decides on its own.

Then the two gated suites the workspace run does not reach. Run them and record
the result — **a carried-forward "unverifiable on this machine" is not evidence**;
this machine has an RTX 4070 Ti and the GPU suite runs:

```bash
cargo test -p ondin-render --release -- --ignored
```

```bash
cargo test -p ondin-app fonts -- --ignored
```

Every `#[ignore]` in this tree is legitimately gated (GPU, network, one long
boolean instrument), so "no `#[ignore]`" is *not* the precondition. "Every
`#[ignore]` has a reason string, and the gated suites were run once" is.

**An unformatted tree is a fix, not a stop**: run `cargo fmt --all`, commit it as
its own `style:` or `chore:` commit, carry on. A broken doc link is the same
shape — fix, commit as `docs:`, carry on. Failing clippy or tests is a stop.

Then read the current version so the requested one can be sanity-checked:

```bash
grep -n '^version' Cargo.toml
```

The new version must be greater than the current one, and the tag must not
already exist (`git tag --list vX.Y.Z`). If either is off, stop and ask.

⚠️ **The workspace is still at `0.0.0`.** The first tag is therefore a decision
rather than an increment — it sets the scheme every later release reads as
precedent — so say what you think it should be and let the user pick it. The
editor being incomplete argues for `0.1.0` rather than `1.0.0`.

Finally, check whether the range being shipped has been reviewed. If
`review/release-<last-tag>/findings.md` doesn't exist — or exists but its header
records a head SHA older than the current one — mention it once and offer the
`release-review` skill, which reviews `<last-tag>..HEAD` autonomously and writes
its findings there. **It's an offer, not a gate.** Don't run it uninvited, and
never run it after the tag — its whole value is arriving before one.

## Phase 1 — push the work

```bash
git push origin main
```

With no CI there is nothing to wait for and nothing to watch. The push either
succeeded or it didn't; if it was rejected, report why and stop rather than
forcing anything.

## Phase 2 — bump and tag

Edit **one** place: `[workspace.package].version` in the root `Cargo.toml`. Every
crate inherits it, so a per-crate `version = ` is a mistake — if you find one,
say so rather than editing it.

Bumping the version changes `Cargo.lock` too (the workspace crates are listed
there), so run a build before staging or the lock is stale in the commit:

```bash
cargo check --workspace
```

```bash
git add Cargo.toml Cargo.lock
```

Commit with exactly this subject, since the release history reads as a series of
them:

```bash
git commit -m "chore: release vX.Y.Z"
```

Then tag and push both. The tag must match the `Cargo.toml` version exactly, or
the built binary reports a version nobody can find:

```bash
git tag vX.Y.Z
```

```bash
git push origin main
```

```bash
git push origin vX.Y.Z
```

## Phase 3 — build the artifact

No workflow builds this; the tag triggers nothing. Build it here, from the
tagged tree:

```bash
cargo build --release -p ondin-app
```

That produces `target/release/ondin.exe`. ⚠️ **Stop any running `ondin.exe`
first** — on Windows the linker cannot overwrite a running binary, and the error
it gives reads like a build failure.

**Windows is the only target that ships.** The tree carries
`#[cfg(not(windows))]` twins for four decision functions and a `winresource`
build script that shells out to `rc.exe`, and none of the non-Windows arms is
compiled by any gate on this machine — so a Linux or macOS binary would be
untested in the most literal sense. Say that in the release notes rather than
leaving a reader to discover it.

Sanity-check what you are about to publish before publishing it:

```bash
./target/release/ondin.exe export <a .ondin document> --svg -o out.svg
```

A headless export exercises the whole document → markup path without opening a
window, and a binary that cannot do that is not one to ship. ⚠️ **The repository
holds no sample document** — the one that used to sit at the root was a
migration fixture with a misleading name and was deleted on 2026-09-16 — so save
one from the app, or build one in a throwaway test, and keep it out of the tree.
**Never launch the GUI to check a release** (CLAUDE.md): it grabs focus and the
mouse. If the release needs an eye on it, ask the user to run it and say what to
look for.

## Phase 4 — publish and write the notes

```bash
gh release create vX.Y.Z target/release/ondin.exe --notes-file <path>
```

Write the body to a file in the scratchpad, not in the repo — multi-line markdown
through `--notes` gets mangled by PowerShell quoting, and a file sidesteps it
entirely. Show the user the drafted notes before publishing if the release is
substantial; it is the one part of this flow that is a judgement call rather than
a procedure.

Draft the notes from the commits in the range:

```bash
git log --oneline vPREV..vX.Y.Z
```

Group by **theme, not one bullet per commit** — several commits usually make one
story worth telling, and the reader cares about what changed for them, not about
your commit boundaries. Read the actual diffs for anything you can't summarize
honestly from the subject line.

Format:

- **No title heading.** GitHub already shows the tag as the title. Open with one
  sentence: `Ondin vX.Y.Z — **theme one**, **theme two**, and a third thing.`
- Then `## ` sections in this order, each **optional if empty**: **Highlights**,
  **Improvements**, **Performance**, **Fixes**.
- Bullets lead with a bold phrase: `* **Feature name.** What changed.`

**No "Under the hood" section.** The audience is someone deciding whether to
download a design tool, and internal refactors, module reorganizations and test
counts tell them nothing about whether the app got better. If a piece of internal
work genuinely changed the experience, say it as the experience — "boolean
operations no longer take the editor down with them" rather than "added a
`catch_unwind` guard" — and it belongs in Improvements or Fixes. If it can't be
stated that way, it doesn't go in the notes at all.

**Keep it short — this is the hard part.** The easy failure is writing the commit
message again. A changelog is *scanned*, by someone deciding whether to update or
hunting the thing that broke. The reasoning, the edge cases and the rejected
alternatives already live in the commit and in §15; repeating them here buries
the release under its own footnotes.

- **Highlights: one sentence, two at most.** Three or four bullets — if
  everything is a highlight, nothing is. Bold the sub-features inline rather than
  spending a sentence on each.
- **Improvements and Fixes: one short sentence each, often just a clause.** A fix
  is *what was broken*, not the diagnosis that found it.
- Where a sentence is spare, spend it on the problem that existed before rather
  than on how the fix works. That's the one piece of "why" worth the space.

⚠️ **Nothing has been released yet, so there is no prior tag to calibrate
against.** Once one exists, read it before drafting the next
(`gh release view vPREV --json body --jq .body`) and match its scope. **The first
release sets that precedent** — write it as tightly as the rules above describe,
because every later one will be written to look like it.

For the *first* release specifically: `git log --oneline` covers the whole
history, which is a fresh repository rather than the project's real past. Don't
summarize commits — describe **what the app does**, in the same sections, and
say plainly what is not finished. §15 is the record of how it got there and the
notes are not a second copy of it.

## Command notes (Windows / PowerShell)

Two things mangle commands on this machine, both worth knowing before spending a
round trip debugging them:

- **`gh --jq` expressions containing `->` or `\(…)` get eaten** — PowerShell reads
  `>` as a redirect. Keep `--jq` to plain field access (`'.[0].databaseId'`) and
  do anything structured by piping the JSON through `ConvertFrom-Json`.
- **Multi-line text can't go through `-m`.** A `>=` inside a commit message is
  read as a redirect and the message arrives split into pathspecs. Write the text
  to a file and use `git commit -F` / `gh release create --notes-file`.

## Finishing

Report: the version, which gates ran and what they said, the release URL, and
what was attached to it. If anything was skipped or went sideways, say so plainly
— a release that half-happened is worth flagging loudly, and with no CI behind
this flow there is no second opinion to correct it.
