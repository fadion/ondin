# Ondin — working notes for Claude

Ondin is a native 2D design tool in Rust (egui + wgpu), published at
`https://github.com/fadion/ondin`. This file holds working rules only. The design lives in
`docs/`, open work in `docs/roadmap.md`, and history in git.

## Documents

| File | Holds |
| --- | --- |
| `docs/architecture.md` | The design and its invariants. Large: ask `design-oracle`, do not page it in. |
| `docs/decisions.md` | §15, the old deviation ledger (D1–D1100). **Frozen** — see below. |
| `docs/roadmap.md` | Open work, decided non-goals, parked decisions. |
| `docs/shortcuts.md` | The keymap. |
| `docs/context-menus.md` | The context-menu spec. |
| `docs/vm.md` | The Command Mode language. Nothing of it is built. |
| `todo.md` | The maintainer's private scratchpad, untracked. Never record anything there. |

`design/` holds mockups. It is untracked and regenerated between sessions, so re-read it
rather than trusting an earlier look, and never keep a spec only there.

### Keeping the record

- **Docs state what is true now.** Write the current rule and the reason for it. No session
  narratives, no "found in session N", no amendment trails, no counts that go stale. Git is
  the history.
- **When code and docs disagree, find out which is wrong** — it has often been the doc. If a
  fix changes a rule, change the sentence that states it; never leave it standing.
- **Edit the passage in place.** A change to the design rewrites the section it touches.
- **D-numbers are frozen.** Code comments cite `§15 D<n>` thousands of times and those still
  resolve in `docs/decisions.md`, but **no new D-number is minted** and no new ledger entry
  is written. A new comment gives its reason in the comment.
- **The roadmap holds open work only.** When something lands, delete its entry. A decided
  non-goal stays as one line so it is not re-argued.
- Name the subject in a sentence rather than "that function" — an insertion above it
  orphans the pronoun.

## Editing files

- **Use `Edit`/`Write`. Never rewrite a source file with a script** — no `sed -i`,
  `perl -pi`, in-place `awk`, heredoc or Python regenerating an existing file, including a
  throwaway flip-check. A stream editor once left a 3,900-line file at zero bytes. Reading
  with `cat`, `grep`, `sed -n` is fine.
- On Windows a script rewrite also flips LF to CRLF (`core.autocrlf` is false here). A
  one-line change that `git diff --stat` reports as thousands is that.
- **Anchor an insertion on the line above, never on the item below.** Anchoring on a `fn`,
  `#[test]` or `#[derive]` line puts the new item between that item and its doc comment, and
  every gate stays green. After inserting, read the result: does the item below still have
  its doc, and does the run above your new item describe *it*?
- To revert a flip, use the inverse edit, then `git diff` the file against `HEAD`.
- Copy a file to the scratchpad before an experiment you expect to revert. `git checkout
  <file>` throws away uncommitted work along with the mistake.
- After deleting or renaming anything, grep the old name: `cargo doc` cannot see
  `#[cfg(test)]` modules, where much of the prose lives.

## Git

- **Commit only when asked.** Invoking the `commit` skill is the ask; finishing a task is
  not.
- One logical change per commit, Conventional Commits (`type(scope): subject`). The `commit`
  skill is the authority. No attribution trailer.
- **Stage specific files. Never `git add -A` while a subagent is running** — it picks up the
  agent's half-finished work.
- Several agents in one tree on disjoint files is fine. When two touched one file, stage
  exact content (`git apply --cached --recount` of a hunk-filtered patch, written as bytes),
  then build and test the staged tree from `git checkout-index -a --prefix=…` with its own
  `CARGO_TARGET_DIR`. Check free disk first: such a build is ~11 GB, and `cargo check
  --all-targets` (~1 GB) is usually enough.
- Version bumps and tags belong to the `release` skill. **A tag is a release**: CI builds it
  and every installed copy offers to update to it. A dry run is the Release workflow's
  `workflow_dispatch`.
- Identity values are permanent: Velopack `--packId Ondin`, the channels
  `win-x64`/`linux-x64`/`osx-arm64`, and `chrome::APP_ID` (`io.github.fadion.Ondin`). Add a
  channel; never rename one.

## Agents (`.claude/agents/`)

- **`design-oracle`** — what does the design say about X? Use it instead of reading
  `docs/architecture.md` or `docs/decisions.md`.
- **`arch-scribe`** — updates the docs after a finished change and deletes the roadmap entry
  it closes. Brief it on the premises as well as the change; it often finds real defects.
  It will not edit `CLAUDE.md`.
- **`rust-verify`** — runs the gates and returns a short report.
- **`rust-mechanic`** — propagates a decided change across the workspace.
- **`ui-prober`** — measures egui chrome with a throwaway test. Expensive; for one number,
  write the test inline.
- `Explore` for "where is X?" sweeps.

After a subagent stops early, check what it claimed against `git diff --stat`.

## Running the app

**Never launch the GUI as a check** — the window grabs the user's mouse and keyboard. Use
tests and reasoning, or ask the user to look and say what to look for.

- **Rendering:** `cargo run -p ondin-app -- export doc.ondin --svg -o out.svg` (`--png`
  through the CPU backend, `--json` for the MCP snapshot). To compare an SVG import against a
  browser, serve it over HTTP (`.claude/launch.json`'s `probe-static`), not `file://`.
- **Chrome:** a throwaway `#[test]` printing pixels or vertices, run with `--nocapture`.
  `egui::Context::default()`, `theme::install(&ctx)`, one `ctx.run_ui(…)` for fonts.
  `OndinApp::headless(&ctx)` builds the whole app without GPU, font threads or prefs.
- egui probe traps:
  - Interaction state is last frame's — pump several frames, and prefer a state you can set
    (`selected`) over one you drive the pointer into.
  - A tooltip's ink never reaches `.shapes`; assert on the `Response` it hangs from.
    `Ui::scope`'s response is never hovered — use `ui.interact(rect, id, Sense::hover())`.
  - The app counts its own double-clicks (`ui::double_clicked`). Move the pointer onto the
    widget before the first press. Plain egui widgets need ~40 idle frames between clicks.
  - **Never nest two `Context` accessors** (`ctx.data_mut(|d| … ctx.cumulative_pass_nr())`)
    — it deadlocks. Read into a local first.
  - `UiBuilder::id` stabilises ids; `id_salt` does not.
- **Flip-check every new test**: break the fix the plausible way, predict which assertion
  fails, run it, put it back. A flip that does not bite is a coverage finding — say so in
  the test's doc. Name the mutation by function, not by the text removed (identical lines
  exist). Assert the fixture reached the state you think it did.
- A test whose assertion depends on work finishing must wait (`settle`), not sleep. Tests
  touching the OS clipboard must not run concurrently. Run a filtered set ~20 times before
  calling it stable.

## Build / test

```bash
cargo build -p ondin-app                  # target/debug/ondin.exe
cargo test                                # workspace; app tests: cargo test -p ondin-app
cargo clippy --workspace --all-targets    # --all-targets or the tests go unlinted
cargo fmt --all
cargo check --release -p ondin-app        # the only gate without debug_assertions
cargo doc --workspace --no-deps --document-private-items
```

Once at the close of a session, not in the edit loop:

```bash
cargo test --workspace --release 2>&1 | tee /tmp/release-run.log
```

Keep the whole log; a red run here cannot be asked for again.

- **`--workspace` and `-p` compile different code** (feature unification changes dependency
  layouts and enabled decoders). Re-run clippy and the relevant test with `-p` on the crate
  you edited, after the test build.
- **The doc gate** denies broken intra-doc links. It needs `--document-private-items` and
  cannot see `#[cfg(test)]` items, `crates/*/tests/` or `build.rs` — there, write plain
  backticks, never `` [`link`] ``. Check your own diff:
  `git diff <base> -- crates/ | grep -E '^\+ *(///|//!).*\[`'`, then ask whether each hit's
  item is `cfg(test)`.
- **Dead code blind spots:** `dead_code` never fires on a `pub` item in a lib crate, and a
  derived `PartialEq` hides an unread field. After removing a caller or reader, grep the name
  and read the struct.
- **Platform arms:** `cfg(unix)`/`target_os` code compiles only on CI here. WSL Ubuntu has
  Rust and runs the Unix arms locally (pipe output through `tr -d '\0'`):
  `wsl.exe -d Ubuntu -- bash -lc 'cd /mnt/c/Users/jonid/Projects/ondin && CARGO_TARGET_DIR=$HOME/ondin-wsl-target cargo test -p ondin-app --bin ondin -- <filter>'`.
  WSL also runs Docker, for throwaway Fedora containers. From Git Bash, put commands in a
  script and run it with `MSYS_NO_PATHCONV=1`.
- **Windows CI checks out with `core.autocrlf=true`** and this machine does not, so an
  `include_str!` test matching `\n` can pass here and fail there.
- `cargo test` stops at the first failing target; run `cargo test --workspace --release`
  before pushing a CI fix.
- On-device GPU tests: `cargo test -p ondin-render --release -- --ignored`, with nothing else
  running (timing tests fail under load).
- Python here is a Store stub; never pipe into `python -`.

### Gates that look like they cover something and do not

Before trusting a gate on a defect it was quiet about, read its predicate, and break the
thing it claims to cover to see it go red. Known holes:

- `clippy::duplicated_attributes` is silent inside `impl` blocks.
- No gate catches an unused dependency.
- `valve_condition_gate` reads a 160-character window and excuses a line by a marker within
  15 lines back / 4 forward. Put the marker last in its comment block.
- A test comparing two hand-kept lists checks that they agree, not that either is complete.
- A command in a doc is a claim: when one comes with a figure, run it, with a control.
