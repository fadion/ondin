# Ondin — Roadmap

Open work only. **Now** is defects and gaps on shipped features, **Next** is decided and unbuilt,
**Later** is deferred with a stated reason or trigger. Decided non-goals are in `non-goals.md`; built
work is described in `architecture.md`. Refs are file + function name, never line numbers. Read the
code before picking an item up — an item's premise can go stale while its ask still reads true.

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

Designed in §15 D1098. The three emitters and `ondin export --html|--react|--vue` are built
(`export.md`, *Code export*).

- **Merging a set's variant trees** that match by name path into one tree with conditional
  classes (D1098 (9)). Each variant is its own tree behind a condition now, which is exact but
  long.
- **The fidelity tests on Linux and macOS** — `code_fidelity.rs` fails on CI without a browser and
  has not run on CI's Linux and macOS legs; unverified until it does. It covers only the HTML
  output; React and Vue have no browser test (no bundler in the test run).
- **Text styles as classes** — revisit once real exports exist (D1098 (7)).
- **A per-layer HTML tag field** — parked, new model (D1098 (3)).

## Later · Command palette and cheatsheet

`Ctrl+K`, `Ctrl+/` alias (`shortcuts.md` §8). For app actions, distinct from Command Mode.

- **Build on `menu.rs`'s registry extending `Action`**, not a list of strings: name, availability
  (`Row::dim_if`, dim with a reason), toggle state, dispatched `Action`.
- **A palette has no `Target`**; `menu::Context` is keyed on a mandatory one. Something must answer it.
- **About 16 `Action`s have no `Item`** (`Undo`, `Save`, `NewDocument`, `OpenSettings`, zoom, …) and
  verbs with no context ("Open Typography panel") need a home.
- **`Action::ExportCode` has no other door**: the palette is how code export reaches the GUI. Drop
  its `expect(dead_code)` when the palette constructs it.
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
