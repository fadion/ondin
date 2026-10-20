# Ondin — working notes for coding agents

Ondin is a native 2D design tool in Rust (egui + wgpu), published at
`https://github.com/fadion/ondin`. This file holds the working rules every agent in this
repository follows. The design lives in `docs/`, open work in `docs/roadmap.md`, and history in
git.

## Documents

| File | Holds |
| --- | --- |
| `docs/architecture.md` | The index: the invariants in brief and one row per design file. Read it first. |
| `docs/design/*.md` | The design, one file per subsystem; headings keep the old § numbers. Open only the one you need. |
| `docs/decisions.md` | The key decisions, stated as they stand, under their old D-numbers. |
| `docs/archive/ledger.md` | The old §15 ledger (D1–D1100), **frozen**. Never read it whole; grep it for one number. |
| `docs/roadmap.md` | Open work and parked decisions, nothing else. |
| `docs/non-goals.md` | What is deliberately not built, one line each. |
| `docs/shortcuts.md` | The keymap. |
| `docs/context-menus.md` | The context-menu spec. |
| `docs/vm.md` | The Command Mode language. Nothing of it is built. |
| `todo.md` | The maintainer's private scratchpad, untracked. Never record anything there. |

The mockups live on a claude.ai Design canvas,
`https://claude.ai/artifact/5crMbaHmNpm78vSLrhDf3D`, one artboard per feature. Others may edit
it, so read an artboard with the Artifact tool rather than trusting an earlier look, and never
keep a spec only there. The untracked `design/` folder is a stale export; don't read it.

### Keeping the record

- **Docs state what is true now.** Write the current rule and the reason for it. No session
  narratives, no "found in session N", no amendment trails, no counts that go stale. Git is
  the history.
- **When code and docs disagree, find out which is wrong** — it has often been the doc. If a
  fix changes a rule, change the sentence that states it; never leave it standing.
- **Edit the passage in place.** A change to the design rewrites the section it touches. A
  pixel nudge or a fix that restores documented behaviour needs no doc edit.
- **D-numbers are frozen.** Code comments cite `§15 D<n>` thousands of times; those resolve
  in `docs/decisions.md` for the key ones and in `docs/archive/ledger.md` for the rest
  (`grep -n '^\*\*D123 —' docs/archive/ledger.md`). **No new D-number is minted** and no
  ledger entry is written. A new decision that matters goes into `docs/decisions.md` as a
  line.
- **The roadmap holds open work only.** When something lands, delete its entry. A decided
  non-goal goes to `docs/non-goals.md` as one line so it is not re-argued.
- Name the subject in a sentence rather than "that function" — an insertion above it
  orphans the pronoun.

## Code comments

Comments are read by every agent that reads the code, so they cost on every read. The same
rule as the docs: state what is true now.

- A doc comment says what the item does and, when it is not obvious, why — **one to five
  lines**.
- A non-obvious constraint keeps one sentence: two defences that each look removable, an
  ordering that matters, a trap the next editor would "simplify" away.
- A test's doc says in one line what it pins. Add `flip: <mutation> → fails at <assertion>`
  only when the flip is what proves the test.
- **No history**: no session stories, no "found by", no ledger trails, no measurements kept
  only as history, no counts. Don't cite a D-number in a new comment; give the reason.
- When you edit an item whose comment breaks this rule, trim it in the same change.

## Editing files

- **Edit with an exact-match editing tool. Never rewrite a source file with a script** — no
  `sed -i`, `perl -pi`, in-place `awk`, heredoc or Python regenerating an existing file,
  including a throwaway flip-check. A stream editor once left a 3,900-line file at zero
  bytes. Reading with `cat`, `grep`, `sed -n` is fine.
- On Windows a script rewrite also flips LF to CRLF (`core.autocrlf` is false here). A
  one-line change that `git diff --stat` reports as thousands is that.
- **Anchor an insertion on the line above, never on the item below.** Anchoring on a `fn`,
  `#[test]` or `#[derive]` line puts the new item between that item and its doc comment, and
  every gate stays green. After inserting, read the result: does the item below still have
  its doc, and does the run above your new item describe *it*?
- To revert a flip, use the inverse edit, then `git diff` the file against `HEAD`.
- Copy a file to a scratch location before an experiment you expect to revert. `git checkout
  <file>` throws away uncommitted work along with the mistake.
- After deleting or renaming anything, grep the old name: `cargo doc` cannot see
  `#[cfg(test)]` modules, where much of the prose lives.

## Git

- **Commit only when asked.** Finishing a task is not the ask; leave the work in the tree and
  say it is ready.
- One logical change per commit, Conventional Commits (`type(scope): subject`). No
  attribution trailer.
- **Stage specific files. Never `git add -A` while another agent is working in the tree** —
  it picks up that agent's half-finished work.
- Several agents in one tree on disjoint files is fine. When two touched one file, stage
  exact content (`git apply --cached --recount` of a hunk-filtered patch, written as bytes),
  then build and test the staged tree from `git checkout-index -a --prefix=…` with its own
  `CARGO_TARGET_DIR`. Check free disk first: such a build is ~11 GB, and `cargo check
  --all-targets` (~1 GB) is usually enough.
- Version bumps and tags are explicit-only. **A tag is a release**: CI builds it and every
  installed copy offers to update to it. A dry run is the Release workflow's
  `workflow_dispatch`.
- Identity values are permanent: Velopack `--packId Ondin`, the channels
  `win-x64`/`linux-x64`/`osx-arm64`, and `chrome::APP_ID` (`io.github.fadion.Ondin`). Add a
  channel; never rename one.

## Running the app

**Never launch the GUI as a check** — the window grabs the user's mouse and keyboard. Use
tests and reasoning, or ask the user to look and say what to look for.

- **Rendering:** `cargo run -p ondin-app -- export doc.ondin --svg -o out.svg` (`--png`
  through the CPU backend, `--json` for the MCP snapshot). To compare an SVG import against a
  browser, serve it over HTTP, not `file://`.
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
