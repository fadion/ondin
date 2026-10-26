# Ondin — Roadmap

Open work only. **Later · First** is the editor's remaining gaps, which land before MCP and Command
Mode; **Later · Second** is what Ondin can offer that other design tools do not, and follows it. The
other **Later** sections are deferred with a stated reason or trigger. Decided non-goals are in
`non-goals.md`; built work is described in `architecture.md`. Refs are file + function name, never line
numbers. Read the code before picking an item up — an item's premise can go stale while its ask still
reads true.

## Later · First

- **Blend modes**, per layer and per effect. `shortcuts.md` §2 reserves `Shift+3`…`Shift+9`.
- **Background blur.** Canvas and raster draw the backdrop; SVG cannot express it and the code export
  has `backdrop-filter`, so the SVG writer owes an answer (a reported approximation, at least).
- **More than one open document** — `OndinApp` holds one `session`.
- **A light theme and a UI scale** — `theme.rs` is dark only; there is no scale setting.
- **Tools**: pencil (freehand), slices.
- **Shapes**: ellipse arcs and donuts; corner smoothing on frames, polygons, stars and paths (a rect
  has it). A polygon's and star's corner radius takes no variable yet, and the multi-selection radius
  field skips them.
- **Paths**: vector networks (`core-model.md`: every reader takes a `BezPath`).
- **Canvas**: lasso, spacing handles on a loose multi-selection (`spacing.rs` covers
  containers).
- **PDF import.**
- **Importing other tools' files**, in this order: **Sketch** (a published JSON spec; also covers
  Lunacy, which saves `.sketch`), **Figma** (`.fig` is reverse-engineered binary that breaks when
  Figma changes it; the REST API is documented but needs a network and the user's token), then
  **Penpot** (open source, so its format can be read from its code). Pixso works with `.fig`;
  Affinity and Illustrator arrive through SVG and PDF.
- **Variable collections and scoping** — one mode list per document today (`styles-variables.md`).

## Later · Second

- **Colour review.** A card (Document menu, palette) over the selection or the document that groups
  near-identical raw colours — fills, strokes, gradient stops, effect and text-run colours — by OKLab
  distance, alpha compared apart. Each group grows around its most-used colour so A≈B≈C cannot chain,
  and shows its members as one split patch with hex, use count and distance; clicking a swatch selects
  its users. *Merge* into a chosen member, or *Merge into a new variable…*, is one transaction through
  recolour-by-colour. A bound colour is never merged away and is the preferred target; vendored kit
  colours are targets only. A *Tight / Close / Loose* threshold reflows the groups. With it, the
  eyedropper snaps to a document colour a hair from the sampled pixel (a modifier takes the raw pixel).
- **Contrast check mode** (design canvas, *Contrast Check*). *View › Check contrast* or the palette, no
  chord; view state, off when a document opens. A bar at the canvas's top centre holds the level menu,
  the failure count and arrows stepping through failures in reading order (selecting each, zooming
  only when it is off screen); closing it ends the mode. It checks the page's shown text layers, or
  those inside the selection the mode started with: that scope is fixed, named in the bar, and
  cleared only from the bar.
  - **Measuring** is `contrast::check`'s (`export.md`), off the main thread. An edit remeasures the
    edited layers and the text they overlap; a badge waiting on that dims with a spinner, and a
    dragged layer's badge hides until release. Every mode is checked and the badge names the worst.
  - **Badges** keep one size at every zoom. A failure gets a red outline and a warning badge, `2.3 : 1
    · needs 4.5`; a pass a quiet chip (`3.8 · large` when only size makes it pass), hidden by *Show
    passing*; an unmeasurable layer a dashed chip saying why.
  - **The fix card** opens from a failing badge: the text and worst backdrop colours, the ratio, one
    row per mode, the nearest passing lightness at the same hue and chroma, then the passing colour
    variables. Hover previews, applying is one undo step on the failing runs. When no colour at that
    hue passes, it offers only variables. It never offers to change the backdrop.
  - **Options:** AA (default), AAA, APCA (Lc values), a pref; *Non-text (3 : 1)* measures strokes and
    shapes under 24 pt across (icons, borders, outlines) and skips larger blocks. APCA and non-text
    are not built in `contrast::check` either. Never saved, never exported, hidden in present mode.
- **Batch rename** of the selected layers: find and replace, and numbering patterns. Only
  `Item::Rename`, one layer, exists.
- **Design tokens import.** The export is built (`dtcg::tokens`, `ondin tokens export`; `export.md`).
  Import of the same file matches by the id in `$extensions`, then by path, so a rename made in code
  is not a delete plus a create. It shows what is added, changed, removed and renamed, and applies it
  as one transaction. A `tokens.json` can also be linked like a kit, with `watch_kits`' pill offering
  its changes. Scoping, in *First*, would make a number's `dimension` or `number` explicit rather than
  read from its bindings.
- **Document diff in the app.** `ondin diff` reports changes by id (`ondin_core::diff`); what is
  open is a visual diff marking the changed regions over the canvas, and the same compare in the
  version history card (`versions.rs`). The git merge driver is built (`ondin merge`).
- **A fidelity report for code export.** Render the HTML output in a browser, as `code_fidelity.rs`
  does, and report to the user what did not translate (background blur in SVG, a property with no
  CSS) instead of drifting silently.
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
- **Command Mode's navigation first** — leap labels and structural `hjkl` navigation (`foundations.md`
  §13) need no Luau, and could ship before the command line and scripting.
- **Private mode**, for sharing a screen without exposing names (a client's or a company's name is
  often the first segment of a component's). A view flag saved as a preference, so a restart mid-call
  does not come back revealing; a shortcut, a palette command, and an accent *Private* pill in the top
  bar that turns it off. It hides names, not content: layer, frame, component, page, document and
  project names are masked by kind and position ("Frame", "Page 2"); text, images, colours, and style
  and variable names stay. Masked, not removed, so a frame label still selects its frame. Pages and
  Layers fold as an overlay (`prefs.pages_folded || private`), leaving the fold preferences as they
  were. Every drawn name goes through one function, toasts included; today views read `.name()`
  directly in some twenty files. Doors that need the real name (rename, layer search) decline with
  *Leave private mode to rename*. The library masks document and project names and keeps thumbnails.
  The guard is a leak test: a document whose every name carries a sentinel, whole frames with each
  surface open (inspector, palette, menus, popovers, library), failing on any drawn text holding it.
  First the chrome, canvas labels, inspector, toasts and palette with that test; then the popovers,
  kits and library.

## Later · Components and layout

- **Pushing an instance's changes to its main** (`architecture.md` §5.3d).
- **A verb that un-makes a component** — owes a detach of its instances, as a delete does (§15 D979).
- **CSS layout properties §5.3c defers** — non-`auto` margin, `order`, named grid areas,
  `auto-fill`/`auto-fit`, `calc`, `aspect-ratio`, block/inline/float.
- **A per-component review table for kit updates** — deferred, not declined (§15 D1010 (3), D1011 5D).
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
  specified beside used values (§15 D868). The tools owe pages: the snapshot lists them, but no tool
  takes a page (`export_svg`/`export_png`, `create_node`'s parent, page add/rename/move).
- **Command Mode** — `architecture.md` §13, `vm.md`. Scripts stored inert, never run by the document
  (§15 D1100). Open in `vm.md` §13: the editor's UI, the parameter header, `mlua` exposing Luau's
  analysis, files outside the document (parked), first-run disclosure, schema place, kit scripts.
- **Multiplayer** — `architecture.md` §12; components bend the fourth property (§15 D978).
