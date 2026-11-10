# Ondin — Roadmap

Open work only. **Now** is the work that lands before Command Mode and MCP: charts, icon packs
and code export. The **Later** sections wait on that work or on a stated trigger; **Considerations** are ideas not yet
decided. Decided non-goals are in `non-goals.md`; built work is described in `architecture.md`.
Refs are file + function name, never line numbers. Read the code before picking an item up — an
item's premise can go stale while its ask still reads true.

## Now · Charts

Designed in `design/charts.md`. Built over several sessions, one phase at a time.

### Work plan

Phases in landing order; each is a session or less and ends green and committed. A session
takes the first phase listed, and deletes it here when it lands. A phase left part-done carries
a *Landed:* line saying what is in and what is next, so the next session resumes there.

- **The Chart card, what is left** — *Landed:* the card's sections, presets, previews, series and
  node swatches, flatten. Next: the *Outline* preset (stroked, unfilled marks: an option the
  generator needs first); *From variables* and bindable swatches, where the variable flows learn
  the description's colours (`variable::reads`, `remove_ops`' detach, `detached`, a kit's
  rebind) and `Chart::validate` stops refusing a bound one; a series' dash and point shape on its
  row; a node's total on its row; the text family; number format prefix, suffix, decimals and
  percent; an axis range; a multi-selection of charts of one kind.
- **The data window** — the `TABLE` glyph, the grid, its keyboard, range selection, TSV copy and
  paste, the header and row menus, the undo steps; opening on a new chart, and a double-click on
  a chart opening it at the clicked mark's cell.
- **Import and the linked file** — CSV, TSV and JSON readers, the import box, the drop doors, the
  link with the tokens link's path rules, watching, the pill and *Update*, relink and unlink.
- **Elsewhere** — the context menu, the palette rows, `control_audit` for the card and the
  window.

## Now · Icon packs

A kit of icon components that the assets browser's Icons tab shows beside the remote sets. It is
a kit, not a second publishing path: *Make this a kit*, publishing, the vault and the update pill
stay as `kits.md` has them. Not designed yet; the pieces:

- **The flag** — a kit carries an explicit "show in the Icons tab" mark, never inferred from its
  content. Settings ▸ Assets lists flagged library kits beside `sets::SETS`, off by default like
  them. The kit stays in the insert picker too.
- **Browsing** — the Icons tab's grid, search, set dropdown and size field take the flagged kits'
  mains as icons. A set shows once, by its default variant.
- **Inserting** — a click or drag goes through `kit::insert`, placing an instance sized to the
  size field, one undo step. Resizing follows the author's constraints; nothing is assumed.
- **To decide: recolouring.** A remote icon is one group with its `currentColor` written black;
  an instance has no icon colour, only per-layer fill overrides or a bound variable. Leading
  option: an icon kit binds its ink to one colour variable that the instance overrides.
- **To decide: variants.** The tab has one variant dropdown and a set can have several
  properties (weight × filled). Either the kit names the property the dropdown drives, or
  icon kits allow one property.
- **Strokes at size** — an outline icon set to scale thickens its strokes as it grows, one set
  fixed does not. That is the author's constraints working, and `kits.md` should say so.

## Now · Code export (HTML/CSS, React, Vue)

Designed in §15 D1098. The three emitters and `ondin export --html|--react|--vue` are built
(`export.md`, *Code export*).

- **The fidelity tests on Linux and macOS** — `code_fidelity.rs` fails on CI without a browser and
  has not run on CI's Linux and macOS legs; unverified until it does. It draws the HTML output
  and the component walk (`html_of_components`), not React's or Vue's own syntax: no bundler,
  TypeScript or Vue compiler runs in the tests. A CI leg parsing the emitted `.tsx` with `tsc`
  and the `.vue` files with `@vue/compiler-sfc` would catch output that does not compile.

## Later

- **Command Mode** — `architecture.md` §13, `vm.md`. Its verbs are the registry's (`ondin_core::verbs`, built). Scripts
  stored inert, never run by the document (§15 D1100). Open in `vm.md` §13: the editor's UI, the
  parameter header, `mlua` exposing Luau's analysis, files outside the document (parked), first-run
  disclosure, schema place, kit scripts. `/ai` is built after the assistant.
- **The assistant** — `ai.md`. Waits on `ondin-mcp` serving its Phase 1 reads. Then: finding and
  probing each harness, the sealed spawn per harness and its stream parser, `propose_edit` and its
  card, the chat window, attaching layers from the canvas and the layers tree, Settings'
  *Assistant* section, the agent log. Open in `ai.md` §9.
- **MCP and headless (M6)** — `architecture.md` §8, §10; `ondin-mcp` is a stub. Its Phase 2 tools
  are the registry's verbs (`ondin_core::verbs`), held to them by a test; each tool's schema shows
  `verbs::example` for its model parameters. Also owed:
  the commit pass sequence out of `EditorSession::commit_inner`; kits and library code out of the app
  for Phase 4 (§15 D1096). The snapshot owes: per-run colour, list markers and level (§15 D169, D172),
  12 of 13 paragraph fields and `BlockStyle`, the missing-font warning, fill item ids (§15 D980), and
  specified beside used values (§15 D868). The page verbs are built; `export_svg` and `export_png`
  still take no page.
- **The *Measured empty* caption for every width from content.** The Variables card's warning
  under a querying frame (`panels::variables`, beside `widths_fold`) checks only the frame's own
  *Fit content*. A frame sized by an auto grid track, or a flex item that does not grow, is also
  measured empty under containment and gets no caption. Ask layout whether the width comes from
  content, not each setting in turn.

## Later · Platform and dependencies

- **egui 0.36** is blocked by vello requiring wgpu 29 (check — last asked 2026-09-03). Re-derive
  `ui::menu_rows`' button-style arithmetic when it moves.

## Considerations

Ideas worth weighing, not yet decided or scheduled.

- **A control channel into the running app.** Loopback only: read UI state, drive tools with
  synthetic pointer and key events, and screenshot the window. Agents never launch the GUI because
  the window takes the user's mouse and keyboard; a window that accepts input only through the
  channel (hidden or offscreen) would let an agent check chrome against the real app instead of
  throwaway probe tests. It could grow from the local socket the window already listens on for
  open requests (`one_window`).
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
- UI toolkit revisit (gpui / Xilem), `architecture.md` §9.1.
- MCP transport hardening (auth, TCP/remote).
- `vello_hybrid` as a third backend if low-end GPUs demand it.
- **PNG goldens** — wait on an aarch64/NEON measurement (§15 D304); per-`vello_cpu` too (D414).
  `num_threads` is still a default.
- **Layers panel `MIN_W = 210`** rests on one reading, not two (§15 D444): re-tighten or amend its prose.
- **Chrome hairline base** in `rulers::snap_across_axis` (guides) is unruled (§15 D479). The pixel
  grid no longer calls it: it places lines from its lattice, which
  `the_lattice_picks_the_pixels_the_snapped_lines_did` holds to the pixels `snap_across_axis` picks,
  so ruling the base means moving the lattice with it.
- **The Type panel's four `%` caps** are 200, inherited, not chosen (§15 D861).
- **Dashboard virtualisation** — deferred (§15 D862); not a one-line `show_rows` (rename field, ⋮ menu,
  `follow_selection` depend on drawn cards).
- **Components: five unruled questions** — a group instance's canvas label; a main-scoped guide not
  copied; a linked node moved into a nested instance losing its link; instance roots exempt from
  membership (§15 D978); a moved-out layer previewed on copies (§15 D979 (xiv)).
- **`Ctrl+Alt` is `AltGr`** on many European layouts (`shortcuts.md` §12, §15 D987).
- **One font-availability question for panel and snapshot** — only worth it if the snapshot ever
  reads a live `FontService`.
