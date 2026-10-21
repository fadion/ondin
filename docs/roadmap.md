# Ondin — Roadmap

Open work only. **Now** is defects and gaps on shipped features, **Next** is decided and unbuilt,
**Later** is deferred with a stated reason or trigger. Decided non-goals are in `non-goals.md`; built
work is described in `architecture.md`. Refs are file + function name, never line numbers. Read the
code before picking an item up — an item's premise can go stale while its ask still reads true.

## Next · Trim the code comments (the maintainer's call, 2026-10-20: the whole tree)

**What was measured (2026-10-20):** comments are **8.4 MB of the 17 MB of Rust (49%)**:
`ondin-app` 5.3 of 10.5 MB, `ondin-core` 2.3 of 4.8 MB, `ondin-render` 0.5 of 1.0 MB,
`ondin-export` 0.3 of 0.7 MB. About 4.7 MB of that sits in test code and 3.7 MB in production.
The worst files: `panels/inspector.rs`, `canvas.rs` and `app.rs` each carry 650–710 KB of
comments, about half of each file. Next come `dashboard.rs`, `typography.rs`, `core/text.rs`,
`ui.rs`, `tools/mod.rs` and `svg_in.rs`, at 240–320 KB each. 1,175 doc runs are 20 lines or
longer. D-number citations appear 8,700 times.

**Why:** an agent reading code pays for every comment line, so half of every code read is prose.
Much of that prose is the same history the docs had: amendment trails, "found in session N",
flip-check write-ups, §15 citation chains and counts that have gone stale. The docs went
state-only on 2026-10-18 (CLAUDE.md 175 → 11 KB, architecture 1.5 MB → a 4 KB index and
per-subsystem files). The comments are the last place that history piles up, and they are read
far more often than the docs.

**The rule to apply** is `AGENTS.md`'s *Code comments*, in force for new comments since
2026-10-20. For the trim, one addition: an existing `§15 D<n>` citation may stay as a bare
pointer when the archived reasoning matters; otherwise drop it.

**How:**
- One agent per file, or per line range for the files over ~500 KB. Disjoint files, so several
  can run at once.
- **`Edit` only.** No script rewrites, per CLAUDE.md. `doc-run-guard` flags any doc comment an
  edit carries off its item.
- Comment-only changes. Each batch runs `cargo fmt`, `cargo check --all-targets`, the doc gate
  (`--document-private-items`) and `cargo test`, then commits per crate or per large file.
- After each batch, `git diff` should show no change outside `//` lines (`git diff -U0 | grep '^[+-]' | grep -v '^[+-][[:space:]]*//'` returns only the file headers).
- Order: the three largest app files, then the rest of `ondin-app`, then `ondin-core`,
  `ondin-render` and `ondin-export`.

**Cost:** roughly 5–6M subagent tokens; reading the tree alone is about 4.5M. Target: comments
down to about 15–20% of the source, roughly 2.5–3.5 MB in place of 8.4 MB.

## Later · Components and layout

- **Pushing an instance's changes to its main** (`architecture.md` §5.3d).
- **A verb that un-makes a component** — owes a detach of its instances, as a delete does (§15 D979).
- **CSS layout properties §5.3c defers** — non-`auto` margin, `order`, named grid areas,
  `auto-fill`/`auto-fit`, `calc`, `aspect-ratio`, block/inline/float.
- **A per-component review table for kit updates** — deferred, not declined (§15 D1010 (3), D1011 5D).
- **Kit verbs in the command palette** (§15 D1011 (5)) — waits on the palette.
- **Team publishing of kits, and servers** — undesigned (§15 D1010).
- **Binding the other users of an extracted value** after *Extract into variable…* (§15 D1037, D1038).
- **Whether a preset follows a paste's rule** across documents (D1037 premise (iv)) — trigger: a preset
  with a bindable field. The only one, `prefs::ExportPreset`, has none.

## Later · Rendering and performance

- **GPU device lost while zooming a component-heavy file, cause unmeasured** (§15 D992). The fx
  textures are pooled since (D1003 (7)); whether that cured it is unconfirmed. Trigger: the
  `GPU device lost (<reason>)` line in `ondin.log`.
- **Loading a long chain of mains is super-linear** (§15 D416): 20,000 mains ≈200 s in debug vs ≈2 s
  for 2,000, measured through a fixture that also builds and saves. Not in `component::reaches_itself`
  or `io::schema`'s integrity pass; `variant::check`, `kit::check`, `variable::check` and
  `style::check` are unread.

## Later · Code export (HTML/CSS, React, Vue)

Designed in §15 D1098; nothing built.

- **The trigger** — Exports panel if per-framework options fit, else a menu; no new context-menu rows.
- **The build** — DOM core and report, three emitters, `tokens.css` and the zip, three doors (GUI,
  `ondin export`, an MCP tool). The MCP tool lands with `architecture.md` §8.4 and `tools.rs` in one
  commit (D726's gate). The first build commit owes D1098 its citation.
- **Fidelity goldens** (D1098 (11)) — headless browser vs CPU PNG; which browser and how CI runs it.
- **Owed before tier 1**: which stroke alignment a CSS `border` writes exactly; the gradient tier; the
  unit for a bound opacity (D1072).
- **The alias trap** (D1098 (6)) — check aliases re-declared per mode block in a real browser.
- **Text styles as classes** — revisit once real exports exist (D1098 (7)).
- **A per-layer HTML tag field** — parked, new model (D1098 (3)).

## Later · Command palette and cheatsheet

`Ctrl+K`, `Ctrl+/` alias (`shortcuts.md` §8). For app actions, distinct from Command Mode.

- **Build on `menu.rs`'s registry extending `Action`**, not a list of strings: name, availability
  (`Row::dim_if`, dim with a reason), toggle state, dispatched `Action`.
- **A palette has no `Target`**; `menu::Context` is keyed on a mandatory one. Something must answer it.
- **About 16 `Action`s have no `Item`** (`Undo`, `Save`, `NewDocument`, `OpenSettings`, zoom, …) and
  verbs with no context ("Open Typography panel") need a home.
- **Layer search**: lift `matching_rows`' rule into a free function over `&Document` returning hits
  and ancestors separately, with its own case folding.
- **Do not absorb `build.rs`'s document vocabulary** — that is shared by MCP and Command Mode.
- **The cheatsheet** is `shortcuts.md` rendered from the registry; it then becomes the registry's test.

## Later · Platform and dependencies

- **`Event::Zoom` and touchscreen pinch** (§15 D822) — trigger: a Mac or a touch device.
- **Eyedropper: sampling outside the window** with the same loupe (design canvas, *Eyedropper*).
  Needs a capture path per OS (Windows has no system picker), and an off-window sample needs
  converting from the display's space to sRGB. Inside the window is built (`tools.md`, *The
  eyedropper*).
- **egui 0.36** is blocked by vello requiring wgpu 29 (check — last asked 2026-09-03). Re-derive
  `ui::menu_rows`' button-style arithmetic when it moves.

## Later · Tooling

- **The review skills' census rows are never controlled** (§15 D827): the rows in `codebase-review`'s
  and `release-review`'s fact sheet are descriptions a pass turns into commands. Trigger: the next
  review's Pass 0 — write each command down and run it against a fixture that must match and one
  that must not before trusting its count.

## Parked decisions

- Binary save format (the versioned JSON format ships).
- Vello scene fragment caching granularity (`scene.rs` seam).
- Rich text model migration (`runs: Vec<(Range, TextStyle)>`, `architecture.md` §5.4).
- History cap / snapshot compaction for huge delete inverses.
- Font embedding for non-Google local fonts (`architecture.md` §5.4a).
- **Point edits as whole-path `GeometryPatch::Path`** (§15 D123) — revisit with MCP.
- **Justify-all** — blocked upstream in parley (`align_impl`, `pub(crate)` internals); trigger is a
  parley patch (§15 D825).
- **A stored, user-placed transform origin for a multi-selection** — unparks when a user asks for it.
- UI toolkit revisit (gpui / Xilem), `architecture.md` §9.1.
- MCP transport hardening (auth, TCP/remote).
- `vello_hybrid` as a third backend if low-end GPUs demand it.
- **Per-edit cost**: `Resolved::update` reloads the R-tree per edit; and bulk-create naming is Θ(n²)
  per parent (§15 D446, 62 s on a 32k-shape paste) — incremental name index, or number bulk creates
  differently.
- **PNG goldens** — wait on an aarch64/NEON measurement (§15 D304); per-`vello_cpu` too (D414).
  `num_threads` is still a default.
- **Layers panel `MIN_W = 210`** rests on one reading, not two (§15 D444): re-tighten or amend its prose.
- **Chrome hairline base** in `rulers::snap_across_axis` (guides and pixel grid) is unruled (§15 D479).
- **The Type panel's four `%` caps** are 200, inherited, not chosen (§15 D861).
- **Dashboard virtualisation** — deferred (§15 D862); not a one-line `show_rows` (rename field, ⋮ menu,
  `follow_selection` depend on drawn cards).
- **Components: five unruled questions** — a group instance's canvas label; a main-scoped guide not
  copied; a linked node moved into a nested instance losing its link; instance roots exempt from
  membership (§15 D978); a moved-out layer previewed on copies (§15 D979 (xiv)).
- **Is a 32/33-row context menu past the ceiling?** (`context-menus.md` *Undecided* 7, §15 D981, D1003 (9)).
- **`Ctrl+Alt` is `AltGr`** on many European layouts (`shortcuts.md` §12, §15 D987).
- **Is the chrome meant to be keyboard-traversable?** (§15 D747) — deferred by the maintainer.
- **One font-availability question for panel and snapshot** — only worth it if the snapshot ever
  reads a live `FontService`.

## Post-v1

- **MCP and headless (M6)** — `architecture.md` §8, §10; `ondin-mcp` is a stub. Before the first tool
  handler, move the headless half of `tools/mod.rs` and `preview.rs` into core (§15 D786). Also owed:
  the commit pass sequence out of `EditorSession::commit_inner`; kits and library code out of the app
  for Phase 4 (§15 D1096). The snapshot owes: per-run colour, list markers and level (§15 D169, D172),
  12 of 13 paragraph fields and `BlockStyle`, the missing-font warning, fill item ids (§15 D980), and
  specified beside used values (§15 D868).
- **Command Mode** — `architecture.md` §13, `vm.md`. Scripts stored inert, never run by the document
  (§15 D1100). Open in `vm.md` §13: the editor's UI, the parameter header, `mlua` exposing Luau's
  analysis, files outside the document (parked), first-run disclosure, schema place, kit scripts.
- **Multiplayer** — `architecture.md` §12; components bend the fourth property (§15 D978).
