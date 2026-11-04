# Ondin — Roadmap

Open work only. **Now** is the editor's remaining work — its gaps, what other design tools have
and Ondin lacks, and what Ondin can offer that they do not — roughly smallest first; it lands before
MCP and Command Mode. **Illustrations** is vector editing for icons, logos and simple
illustrations. The **Later** sections are deferred with a stated reason or trigger;
**Considerations** are ideas not yet decided. Decided non-goals are in
`non-goals.md`; built work is described in `architecture.md`. Refs are file + function name, never line
numbers. Read the code before picking an item up — an item's premise can go stale while its ask still
reads true.

## Now

- **Tab reaches every control.** The inspector, the popovers and the panels are a mix today: some
  controls are tab stops and some are painted rects that sense the pointer only. Every control and
  field joins the Tab order in reading order, with `ui::focus_ring` showing where focus is, and
  Return/Space acting on a focused button as a click does (`app-chrome.md`, *Two kinds of focus*).
- **Breakpoints through modes.** A mode carries an optional `min-width`; `tokens.css` writes its block
  as `@media (min-width: …)` instead of `[data-mode]`, and a frame set to *Mode: by width* picks its
  mode from its own width, so resizing it previews the breakpoints. Covers what binds (gaps, padding,
  sizes, radius, visibility, type numbers); a direction change needs an enum binding for flex
  direction. Per-node overrides keyed by breakpoint are not the plan: every reader would take them.
- **Interaction states in code export.** A variant property marked as an interaction state, its values
  tied to `:hover`, `:active`, `:focus-visible` and `:disabled`; the exporter diffs each state's
  variant against the default and writes only what changed. That needs a key pairing each state's
  layers with the default's; layer names are fragile, so settle one before building. A state whose
  layers do not pair falls back to the prop and is reported
  approximated. The prop stays either way, for forcing a state. Live hover in present mode is
  prototyping, a non-goal.

## Illustrations

Vector editing enough for icons, logos and simple illustrations without leaving Ondin.

### Highest value

- **A shape builder.** Drag across overlapping shapes to merge the regions crossed; `Alt`-drag removes
  them. With it, **Divide**, which cuts overlaps into separate paths. The faces come from the planar
  arrangement of the selection's outlines (`i_overlay` is already a dependency) and land as plain
  paths. With the next two items, it covers for a designer most of what a vector network would
  offer (`non-goals.md`).
- **Snapping for points.** Pen anchors and dragged points snap to the pixel grid only
  (`app-snapping.md`, `snap_point`). Add anchors and segments of other paths, intersections, and,
  for node-tool handles, `Shift` angle steps (`canvas::handle_tx` reads `Alt` only; the pen already
  steps by 45°).
- **Cutting paths.** Split a path at an anchor into two subpaths, delete a segment to open a closed
  path, and a knife that cuts along a drawn line. *Delete* on points always heals.
- **Repeat with a transform.** *Repeat around origin…*: N copies turned about the transform origin
  (rays, teeth, ticks); and *Transform again*, the last move, rotation or scale applied once more.
- **Icon-ready SVG export.** Options for `currentColor`, a viewBox from the frame, number precision,
  and outlined versus stroked output; and one action that writes every main of a component set as a
  folder of SVGs.
- **Pixel-fit.** A command moving anchors and stroke edges onto whole or half pixels, so 16–24px
  icons render crisp.

### Medium

- **Warp and distort.** Free distort by four corners, and arc, flag and bulge presets, baked into
  a plain path on commit.
- **Image trace.** A raster sketch or logo to paths (a tracer such as `vtracer`, then `fit` and
  *Simplify*).
- **Mirror symmetry while drawing** about an axis. A flipped linked copy already works as a
  workaround.
- **Blend between two shapes** in N steps.
- **Path cleanup.** Remove redundant points, add points evenly, and smooth only the selected anchors
  (*Simplify* works on the whole path).

### Small

- **Spiral and arc tools.**

## Later · Components and layout

- **Pushing an instance's changes to its main** (`architecture.md` §5.3d).
- **A verb that un-makes a component** — owes a detach of its instances, as a delete does (§15 D979).
- **CSS layout properties §5.3c defers** — non-`auto` margin, `order`, named grid areas,
  `auto-fill`/`auto-fit`, `calc`, `aspect-ratio`, block/inline/float.
- **A per-component review table for kit updates** — deferred, not declined (§15 D1010 (3), D1011 5D).
- **Binding the other users of an extracted value** after *Extract into variable…* (§15 D1037, D1038).
- **Collection follow-ups** (`styles-variables.md`, *Collections and modes*): a kit's collection is not
  in the browser's collection picker (its rows list under the document collection it maps from); and
  a DTCG alias whose scope
  disagrees with its target's is not refused at the scope (the export types each from its own scope).
- **Whether a preset follows a paste's rule** across documents (D1037 premise (iv)) — trigger: a preset
  with a bindable field. The only one, `prefs::ExportPreset`, has none.

## Later · Code export (HTML/CSS, React, Vue)

Designed in §15 D1098. The three emitters and `ondin export --html|--react|--vue` are built
(`export.md`, *Code export*).

- **The fidelity tests on Linux and macOS** — `code_fidelity.rs` fails on CI without a browser and
  has not run on CI's Linux and macOS legs; unverified until it does. It draws the HTML output
  and the component walk (`html_of_components`), not React's or Vue's own syntax: no bundler,
  TypeScript or Vue compiler runs in the tests. A CI leg parsing the emitted `.tsx` with `tsc`
  and the `.vue` files with `@vue/compiler-sfc` would catch output that does not compile.
- **Text styles as classes** — revisit once real exports exist (D1098 (7)).
- **A per-layer HTML tag field** — parked, new model (D1098 (3)).

## Later · Platform and dependencies

- **`Event::Zoom` and touchscreen pinch** (§15 D822) — trigger: a Mac or a touch device.
- **Double-click to open on macOS.** Finder sends an `application:openURLs:` event, which winit 0.30
  does not expose, and the `.app`'s `Info.plist` (written by `vpk`) declares no
  `CFBundleDocumentTypes`. Windows and Linux are built (`app-chrome.md`, *Opening a document from
  the OS*); `ondin <file>` works everywhere. Trigger: a Mac to test on, or a winit that delivers it.
- **One file in two Ondin processes.** A second launch (`ondin <file>`, a double-click) opens a
  file another process already has open, and the two save over each other; one window guards
  only its own tabs (`tab_of_file`). Needs a lock beside the file, or single-instance forwarding
  of the open to the running window.
- **Eyedropper: sampling outside the window** with the same loupe.
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

## Considerations

Ideas worth weighing, not yet decided or scheduled.

- **One command registry.** Every menu item, tool, palette entry and gesture as a named command
  with typed parameters, in one list that the UI, a CLI, MCP and Command Mode all call. Today the
  MCP tools (`ondin-mcp`, `tools.rs`) are a list kept apart from the app's commands, and Command
  Mode would be a third. One registry makes "an agent can do whatever a click can" true by
  construction, and gives the CLI a way to run a script of commands against a file with no window
  (`ondin run build.json --out card.ondin`).
- **A control channel into the running app.** Loopback only: read UI state, drive tools with
  synthetic pointer and key events, and screenshot the window. Agents never launch the GUI because
  the window takes the user's mouse and keyboard; a window that accepts input only through the
  channel (hidden or offscreen) would let an agent check chrome against the real app instead of
  throwaway probe tests.
- **Multiplayer without a server.** The host's app holds the session and peers join by link, peer
  to peer over QUIC with NAT traversal and an end-to-end encrypted relay when no direct path exists.
  Edits merge per property, a delete beats an edit, and each person's undo takes back only their
  own edits; optional password, approval of each joiner, view-only access. §12 locks in the ids and
  transactions such a layer needs and leaves the protocol and conflict rules undesigned; this shape
  needs no hosting and no accounts. The cost: the session ends when the host closes the file.
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

## Post-v1

- **MCP and headless (M6)** — `architecture.md` §8, §10; `ondin-mcp` is a stub. Before the first tool
  handler, move the headless half of `tools/mod.rs` and `preview.rs` into core (§15 D786). Also owed:
  the commit pass sequence out of `EditorSession::commit_inner`; kits and library code out of the app
  for Phase 4 (§15 D1096). The snapshot owes: per-run colour, list markers and level (§15 D169, D172),
  12 of 13 paragraph fields and `BlockStyle`, the missing-font warning, fill item ids (§15 D980), and
  specified beside used values (§15 D868). The tools owe pages: the snapshot lists them, but no tool
  takes a page (`export_svg`/`export_png`, `create_node`'s parent, page add/rename/move).
- **Command Mode** — `architecture.md` §13, `vm.md`. Scripts stored inert, never run by the document
  (§15 D1100). Open in `vm.md` §13: the editor's UI, the parameter header, `mlua` exposing Luau's
  analysis, files outside the document (parked), first-run disclosure, schema place, kit scripts.
- **Multiplayer** — `architecture.md` §12; components bend the fourth property (§15 D978).
