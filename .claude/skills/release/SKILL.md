---
name: release
description: Ondin's push-and-release flow — verify the local bar, push main and wait for CI, bump the workspace version, tag, wait for the Release workflow to build and publish every platform, check the assets, and write the notes. Use whenever the user asks to cut, tag, ship, or publish a release, or says anything like "push and bump to 0.2.0", "bump to v1.2.0 and push", "release 0.1", "tag a new version", or "ship this". Also use it when they ask only for part of the flow (just the bump, just the release notes) so the surrounding steps and their gates aren't skipped by accident.
---

# Cutting an Ondin release

The flow is **verify → push → CI → bump+tag → Release workflow → check → notes**.

**CI builds and publishes; nothing is built here for a release** (§15 D955,
D956). The tag fires `.github/workflows/release.yml`, which builds Windows, Linux
and macOS, packs the Velopack installers and feeds, the portable archives, the
`.deb` and `.rpm`, attaches them all to the GitHub Release, and then rebuilds the
apt/dnf repositories on GitHub Pages (`pages.yml`). Until 2026-10-03 this skill
built `ondin.exe` locally and uploaded it; v0.3.0 was the last release cut that
way.

Report the version and what is about to happen before Phase 2: the tag push is
the first irreversible step.

## Phase 0 — the ground

```bash
git status --short
```

The tree must be clean. Uncommitted work means the user has something in flight:
stop and ask rather than sweeping it into the release.

Then the local bar. CI runs most of it again, but CI is minutes away and a red
run after the push costs a round trip; and two of these (the gated suites) CI
cannot run at all. `rust-verify` runs most of them and returns a distilled report.

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

Why the surprising ones are there: the per-package clippy loop compiles different
code from the workspace run (feature unification); `--document-private-items` is
what makes the doc gate read most of this tree; the release test is the only thing
that runs under the release cfg (§15 D597). ⚠️ **`licenses`, not a bare `cargo
deny check`** — `advisories` is deliberately red on two quick-xml entries;
`deny.toml` has the analysis, and a *new* advisory is a conversation with the user.

Then the two gated suites, **which no CI runner can run** — a GPU and a network
font fetch. This machine has an RTX 4070 Ti; a carried-forward "unverifiable" is
not evidence:

```bash
cargo test -p ondin-render --release -- --ignored
```

```bash
cargo test -p ondin-app fonts -- --ignored
```

**An unformatted tree is a fix, not a stop**: `cargo fmt --all`, commit as its own
`style:` commit, carry on. A broken doc link likewise (`docs:`). Failing clippy or
tests is a stop.

Read the current version:

```bash
grep -n '^version' Cargo.toml
```

The new version must be greater, and the tag must not exist
(`git tag --list vX.Y.Z`). If either is off, stop and ask.

Finally, check whether the range has been reviewed: if
`review/release-<last-tag>/findings.md` is missing, or its header's head SHA is
older than the current one, mention it once and offer the `release-review` skill.
**An offer, not a gate**, and never after the tag.

## Phase 1 — push, and wait for CI

```bash
git push origin main
```

Then find the CI run for that push and wait for it. The run takes a moment to
register, so retry an empty answer a few times:

```bash
gh run list --branch main --workflow CI --limit 1 --json databaseId,headSha,status
```

Check `headSha` is the commit just pushed, then:

```bash
gh run watch <id> --exit-status
```

**Red CI is a stop.** Show the failure (`gh run view <id> --log-failed`) and fix
it before going on. The macOS and Linux legs are the only compilers those
platforms have — nobody here has a Mac — so a failure there is real even when
everything local is green.

## Phase 2 — bump and tag

Edit **one** place: `[workspace.package].version` in the root `Cargo.toml`. Every
crate inherits it. The Release workflow **refuses a tag that does not match it**,
so this is not optional.

```bash
cargo check --workspace
```

```bash
git add Cargo.toml Cargo.lock
```

```bash
git commit -m "chore: release vX.Y.Z"
```

```bash
git tag vX.Y.Z
```

```bash
git push origin main
```

```bash
git push origin vX.Y.Z
```

## Phase 3 — wait for the Release workflow

It runs longer than `gh run watch`'s patience on this machine, so poll it:

```bash
gh run list --workflow Release --limit 1 --json databaseId,headBranch,status,conclusion
```

`headBranch` is the tag. In PowerShell, poll until it completes:

```powershell
do { Start-Sleep 30; $r = gh run view <id> --json status,conclusion | ConvertFrom-Json } while ($r.status -ne "completed"); $r.conclusion
```

**Anything but `success` is a stop.** The tag is already pushed: report what
failed (`gh run view <id> --log-failed`) and ask. **Never delete or re-push a tag
unprompted** — an installed copy may already have seen the feed.

⚠️ **The `pages` job is part of that run**, so a repository publish failure makes
the run red even though the Release itself is complete. Read the job before
guessing. **A job that failed in about two seconds with no steps** was refused by
the `github-pages` environment's deployment rule — the annotation reads *"Tag … is
not allowed to deploy to github-pages due to environment protection rules"*.
v0.4.0's first publish hit exactly this; the environment now carries a `v*` tag
rule beside `main`, and `packaging/repo/README.md` §3 has the command if it is
ever lost. `gh run view --log-failed` prints *"log not found"* for such a job, so
read the annotations instead:

```bash
gh api repos/fadion/ondin/check-runs/<job-id>/annotations --jq ".[].message"
```

A job that ran and failed in its steps is usually the signing key
(`GPG_PRIVATE_KEY`, `packaging/repo/README.md`). Either way it is repaired by
running the **Pages** workflow again from the Actions tab, on `main` — not by
re-tagging.

Then **check the assets**, which the workflow's conclusion does not prove — one of
its three publish steps, *Publish the Velopack artifacts*, tolerates missing files
by design, since every leg names every platform's files. The other two say
`fail_on_unmatched_files: true`; until §15 D974 the portable-archive step only
*claimed* to be strict, and the action's default made all three tolerant:

```bash
gh release view vX.Y.Z --json assets --jq ".assets[].name"
```

Expect, for version `X.Y.Z`:

- `Ondin-win-x64-Setup.exe`, `Ondin-linux-x64.AppImage`, `Ondin-osx-arm64-Setup.pkg`, `Ondin-osx-arm64.dmg`
- `Ondin-X.Y.Z-win-x64-full.nupkg`, `Ondin-X.Y.Z-linux-x64-full.nupkg`, `Ondin-X.Y.Z-osx-arm64-full.nupkg`
- `releases.win-x64.json`, `releases.linux-x64.json`, `releases.osx-arm64.json`
- `ondin-vX.Y.Z-windows-x86_64.zip`, `ondin-vX.Y.Z-linux-x86_64.tar.gz`
- `ondin_X.Y.Z_amd64.deb`, `ondin-X.Y.Z-1.x86_64.rpm`

A missing feed (`releases.<channel>.json`) means installed copies on that platform
will never see this release — say so loudly.

⚠️ **A leg that has already published its Velopack files cannot simply be
re-run** (§15 D974, `[X3-L1-04]`). Its *Fetch the previous release* step then
downloads the release it just published, and `vpk pack` refuses a version equal
to the channel's latest. The `.deb`/`.rpm` are *built* before that upload, so a
Linux leg that dies after it has died in *Publish the distribution packages*:
the packages and Pages are what is missing. Attach the packages by hand with
`gh release upload` (the leg's build log names them; rebuild from the tag on a
Linux host if they are gone),
and run the **Pages** workflow on `main`. Re-running the whole leg is the wrong
repair, and so is re-tagging.

**Never launch the GUI to check a release** (CLAUDE.md). If it needs an eye,
ask the user to install it and say what to look for.

## Phase 4 — the notes

The workflow creates the Release with no body. Draft the notes from:

```bash
git log --oneline vPREV..vX.Y.Z
```

and calibrate against the last one:

```bash
gh release view vPREV --json body --jq .body
```

Write them to a file in the scratchpad (PowerShell mangles multi-line `--notes`)
and apply:

```bash
gh release edit vX.Y.Z --notes-file <path>
```

Show the user the draft first if the release is substantial — the one judgement
call in this flow.

Format:

- **No title heading.** Open with one sentence: `Ondin vX.Y.Z: **theme one**, **theme two**, and a third thing.`
- Then `## ` sections, each optional: **Highlights**, **Improvements**, **Performance**, **Fixes**.
- Bullets lead with a bold phrase: `* **Feature name.** What changed.`

**No "Under the hood" section.** The reader is deciding whether to update a
design tool; refactors and test counts tell them nothing. Internal work that
changed the experience is said as the experience.

**Keep it short — this is the hard part.** Highlights one or two sentences, three
or four bullets; Improvements and Fixes one short sentence each. A fix is *what was
broken*, not the diagnosis. The reasoning lives in the commits and §15.

## Command notes (Windows / PowerShell)

- **`gh --jq` expressions containing `->` or `\(…)` get eaten** — PowerShell reads
  `>` as a redirect. Keep `--jq` to plain field access and pipe anything
  structured through `ConvertFrom-Json`.
- **Multi-line text can't go through `-m`.** Use `git commit -F` and
  `gh release edit --notes-file`.

## Finishing

Report: the version, the local bar, the CI run, the Release run, the release URL,
and the asset check. If anything was skipped or went sideways, say so plainly.
