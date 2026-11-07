# Ondin — Roadmap

Open work only. **Now** is the work that lands before Command Mode and MCP: the editor's remaining
gaps, components and layout, code export, and the command registry both later tracks call. The
**Later** sections wait on that work or on a stated trigger; **Considerations** are ideas not yet
decided. Decided non-goals are in `non-goals.md`; built work is described in `architecture.md`.
Refs are file + function name, never line numbers. Read the code before picking an item up — an
item's premise can go stale while its ask still reads true.

## Now

- **Guide and pixel-grid settings.** A *Canvas* section in Settings (`settings.rs`), per-user prefs:
  - *Ruler guides colour*: the default for every guide whose own colour is unset, starting at
    `DEFAULT_GUIDE_COLOR`, with a picker and a reset. A guide recoloured in the inspector keeps its
    colour, and a selected guide stays the selection blue (`rulers::guide_ink`).
  - *Ruler guides thickness*: 1–5 points, default 1. A selected guide draws one point thicker, as
    the 1 and 2 of `draw_guide_inner` do today.
  - *Pixel grid colour*: an *Adaptive* switch, on by default. Adaptive ink moves each grid pixel's
    colour by 10 in CIE L*, darker above L* 45 and lighter below, keeping its hue (darken by scaling
    in linear light, lighten by mixing toward white). Every fill then gets the contrast today's ink
    gives on white, where today's 50% grey at 24% (`grid::INK`) gives none on mid-grey and almost
    none on mid-tone colours. It reads the drawn pixels, so it is a GPU pass over the rendered
    canvas like `vision::VisionPass`, run only while the grid shows. Off, a picker with alpha
    starts at `grid::INK`, and reset returns to it.
- **One command registry.** Every editing verb as a named command with typed parameters, in one
  list that the app, a headless CLI, MCP and Command Mode all call, so "an agent can do whatever a
  click can" holds by construction. Today there are two levels, joined by hand:
  - *UI commands* act on the selection, a menu's target and the pointer. `menu::Item` is already
    their registry for the menus, the palette and the cheatsheet (label, glyph, chord spelling,
    aliases), and `input::Action` is what keys produce. `Item::action`, `Item::step_name` and
    `perform_menu_item`'s forwards to `dispatch` bridge the two.
  - *Document verbs* take explicit parameters. MCP's list (`ondin-mcp`, `tools.rs`) is names with no
    parameters and no link to either app list, the CLI (`main.rs`, `parse`) is its own parser, and
    `vm.md` assumes a verb list built from `build.rs` that still needs its public naming pass.

  The registry belongs at the second level, in core, in this order:
  1. Move the headless half of `tools/mod.rs` and `preview.rs` into core (§15 D786). Neither uses
     egui, so the move is a sweep the compiler enumerates; after the first MCP handler it would be
     two implementations to reconcile.
  2. A verb table in `ondin-core`: name, typed parameters, a one-line doc, and a function from the
     document and its parameters to a `Transaction`. MCP's Phase 2 tools, Command Mode and a
     headless `ondin run script.json --out card.ondin` bind it, and a test checks `tools.rs`'
     names against it.
  3. The app's editing `Item`s call the same verbs with the selection filled in. What needs the
     menu's target or the pointer stays an `Item` of the app's own.

  Apart from those, keys resolve to `Item`s through one binding table, so a chord's binding and
  its menu spelling are one value; today they agree only because the cheatsheet's tests check them
  (`ViewSwitch::accel`'s doc says so).

## Now · Components and layout

- **Pushing an instance's changes to its main** (`architecture.md` §5.3d). A nested instance added
  inside an instance rather than its main shows no properties on the outer card until it is
  promoted, which waits on this (`components.md`).
- **Container queries for instances** — an instance picking its mode by its own width, as a
  top-level frame picks by the viewport's. Needs a design first: instances are kept out of
  `Document::picks_by_width` on purpose, and a collection's breakpoints are keyed to the viewport.
- **A per-component review table for kit updates** — the next step after the *accept all* notice
  (§15 D1010 (3), D1011 5D): each kit row's disclosure, whose room the design reserves, opens onto
  the components that changed.
- **Binding the other users of an extracted value** after *Extract into variable…*, which binds only
  the field it was run from (§15 D1037, D1038): offer the other fields drawing the same value.
- **Three CSS layout properties §5.3c defers** — `aspect-ratio`, named grid areas and
  `auto-fill`/`auto-fit` (`layout.md`). The engine supports each.

## Now · Code export (HTML/CSS, React, Vue)

Designed in §15 D1098. The three emitters and `ondin export --html|--react|--vue` are built
(`export.md`, *Code export*).

- **The fidelity tests on Linux and macOS** — `code_fidelity.rs` fails on CI without a browser and
  has not run on CI's Linux and macOS legs; unverified until it does. It draws the HTML output
  and the component walk (`html_of_components`), not React's or Vue's own syntax: no bundler,
  TypeScript or Vue compiler runs in the tests. A CI leg parsing the emitted `.tsx` with `tsc`
  and the `.vue` files with `@vue/compiler-sfc` would catch output that does not compile.
- **Text styles as classes.** Today each text node carries its style's resolved declarations, its
  variables kept as `var()`. Emit one `text-styles.css` with a class per text style the export
  uses, and give each node its style's class plus only the declarations it overrides. React
  imports the file once at its root and Vue holds it in an unscoped `<style>`, which answers
  D1098 (7)'s worry that global classes fight CSS modules. Colour is not part of a text style
  (`styles-variables.md`), so it stays on the node, as spans do.
- **A per-layer HTML tag field.** The emitters write four elements (`div`, `span`/`p`, `img`,
  `svg`) and guess nothing from layer names (D1098 (3)). A tag field lets a layer say it is a
  `button`, `a`, `h1`–`h6`, `nav`, `header`, `footer`, `section` or `ul`/`li`. It is new model: a
  node field, a save-format version, an inspector row, the emitters and the MCP snapshot. Each tag
  needs a rule for which kinds may take it; a picture cannot be a list.

## Later

- **Command Mode** — `architecture.md` §13, `vm.md`. Its verbs are the registry's (*Now*). Scripts
  stored inert, never run by the document (§15 D1100). Open in `vm.md` §13: the editor's UI, the
  parameter header, `mlua` exposing Luau's analysis, files outside the document (parked), first-run
  disclosure, schema place, kit scripts.
- **MCP and headless (M6)** — `architecture.md` §8, §10; `ondin-mcp` is a stub. Its Phase 2 tools
  bind the registry (*Now*), whose first step moves the headless editing code into core. Also owed:
  the commit pass sequence out of `EditorSession::commit_inner`; kits and library code out of the app
  for Phase 4 (§15 D1096). The snapshot owes: per-run colour, list markers and level (§15 D169, D172),
  12 of 13 paragraph fields and `BlockStyle`, the missing-font warning, fill item ids (§15 D980), and
  specified beside used values (§15 D868). The tools owe pages: the snapshot lists them, but no tool
  takes a page (`export_svg`/`export_png`, `create_node`'s parent, page add/rename/move).

## Later · Platform and dependencies

- **Touchscreen pinch** (§15 D822): egui's `multi_touch()` on a Windows touchscreen; a trackpad
  pinch is built (`canvas::pinch_zoom`). Trigger: a touch device.
- **One file in two Ondin processes.** A second launch (`ondin <file>`, or a double-click on Windows
  and Linux) opens a file another process already has open, and the two save over each other; one
  window guards only its own tabs (`tab_of_file`). Needs a lock beside the file, or
  single-instance forwarding of the open to the running window.
- **egui 0.36** is blocked by vello requiring wgpu 29 (check — last asked 2026-09-03). Re-derive
  `ui::menu_rows`' button-style arithmetic when it moves.

## Later · Tooling

- **The review skills' census rows are never controlled** (§15 D827): the rows in `codebase-review`'s
  and `release-review`'s fact sheet are descriptions a pass turns into commands. Trigger: the next
  review's Pass 0 — write each command down and run it against a fixture that must match and one
  that must not before trusting its count.

## Considerations

Ideas worth weighing, not yet decided or scheduled.

- **A control channel into the running app.** Loopback only: read UI state, drive tools with
  synthetic pointer and key events, and screenshot the window. Agents never launch the GUI because
  the window takes the user's mouse and keyboard; a window that accepts input only through the
  channel (hidden or offscreen) would let an agent check chrome against the real app instead of
  throwaway probe tests.
- **Multiplayer without a server.** The host's app holds the session and peers join by link, peer
  to peer over QUIC with NAT traversal and an end-to-end encrypted relay when no direct path exists.
  Edits merge per property, a delete beats an edit, and each person's undo takes back only their
  own edits; optional password, approval of each joiner, view-only access. §12 locks in the ids and
  transactions such a layer needs (components bend its fourth property, §15 D978) and leaves the
  protocol and conflict rules undesigned; this shape needs no hosting and no accounts. The cost:
  the session ends when the host closes the file.
- **A tolerant pixel diff.** A compare with anti-aliasing detection and SSIM that writes an HTML
  report. The PNG golden is absent because `vello_cpu`'s bytes vary with SIMD level and version
  (*Parked decisions*); a compare that forgives edge pixels and still catches a moved shape is what
  would unblock it. The same harness could hold approved screenshots of the chrome as regression
  baselines.

## Parked decisions

- Binary save format (the versioned JSON format ships).
- Rich text model migration (`runs: Vec<(Range, TextStyle)>`, `architecture.md` §5.4).
- History cap / snapshot compaction for huge delete inverses.
- Font embedding for non-Google local fonts (`architecture.md` §5.4a).
- **Point edits as whole-path `GeometryPatch::Path`** (§15 D123) — revisit with MCP.
- **Justify-all** — blocked upstream in parley (`align_impl`, `pub(crate)` internals); trigger is a
  parley patch (§15 D825).
- **A stored, user-placed transform origin for a multi-selection** — unparks when a user asks for it.
- **A Tools menu** — *Check contrast* moves there from the View menu once more tools arrive.
- UI toolkit revisit (gpui / Xilem), `architecture.md` §9.1.
- MCP transport hardening (auth, TCP/remote).
- `vello_hybrid` as a third backend if low-end GPUs demand it.
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
- **`Ctrl+Alt` is `AltGr`** on many European layouts (`shortcuts.md` §12, §15 D987).
- **One font-availability question for panel and snapshot** — only worth it if the snapshot ever
  reads a live `FontService`.
