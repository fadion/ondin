# Ondin — Claude Code notes

@AGENTS.md

The rules imported above are `AGENTS.md`, which every coding agent in this repository follows
and which stays the authority — a source comment citing "CLAUDE.md" means a rule that now lives
there. What follows is Claude Code's own.

## Subagents (`.claude/agents/`)

- **`design-oracle`** — what does the design say about X? Use it instead of reading
  `docs/architecture.md`, `docs/design/` or `docs/decisions.md` yourself.
- **`arch-scribe`** — updates the docs after a finished change and deletes the roadmap entry
  it closes. Brief it on the premises as well as the change; it often finds real defects.
  It will not edit `CLAUDE.md` or `AGENTS.md`.
- **`rust-verify`** — runs the gates and returns a short report.
- **`rust-mechanic`** — propagates a decided change across the workspace.
- **`ui-prober`** — measures egui chrome with a throwaway test. Expensive; for one number,
  write the test inline.
- `Explore` for "where is X?" sweeps.

After a subagent stops early, check what it claimed against `git diff --stat`. Editing a
module you are actively designing in belongs in the main loop.

## Skills and hooks (`.claude/skills/`)

- **`commit`** and **`release`** carry the commit and release flows; invoking `commit` is the
  ask to commit.
- **`edit-guard`** refuses script rewrites, directory-wide `git add` and `git checkout`-style
  undo; **`doc-run-guard`** flags an edit that carried a doc comment off its item. Both
  enforce `AGENTS.md`'s editing rules.
- `.claude/settings.json` (local, gitignored) denies `Read` on `docs/archive/**`; grep it.
- `.claude/launch.json`'s `probe-static` serves `target/probe` for browser comparisons.
- Use the scratchpad for files you expect to revert.
