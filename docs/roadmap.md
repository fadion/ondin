# Ondin — Roadmap

Open work only. **Now · Immediately** is defects and dead ends on shipped features, **Now · Soon** is
what a designer reaches for and does not find, **Later · Before MCP** is larger work that lands before
MCP and Command Mode, and the other **Later** sections are deferred with a stated reason or trigger.
Decided non-goals are in `non-goals.md`; built work is described in `architecture.md`. Refs are file +
function name, never line numbers. Read the code before picking an item up — an item's premise can go
stale while its ask still reads true.

## Now · Immediately

- **Drag-reorder of the effects card's rows**, as the fill and stroke rows have (`paint::PaintDrag`,
  `inspector.md` *Live reorder*). The effects stack is ordered and `SetEffects` writes it whole, so
  only the card's drag is missing.
- **Version history has no door.** `Ctrl+S` pins a copy under `.versions/`, but `library::store::versions`
  has no production caller, so pins are never offered back. Browse and restore.
- **A top-level instance cannot change main.** `swap::can_swap` admits only a nested copy inside an
  outer instance; `components.md` says a root "changes main by relinking", but nothing writes
  `SetLink` except cut-move (`app.rs` `paste_cut_move`).
- **SVG export drops text properties without reporting them**: `svg::run_attrs` writes no
  `font-variation-settings` beyond weight, no `font-feature-settings`, no baseline shift and no
  `xml:lang`, and `svg::report_node` names none of them. The code export writes the first two
  (`code::walk`).
- **Command palette coverage.** The Document menu's *Make this a kit*, *Kits used…*, *Stop sharing as
  kit* and *Show in library* are not registry `Item`s, and quarter turns
  (`inspector::rotate_selection_quarter`) are not in `palette::palette_commands`.

## Now · Soon

- **Outline stroke** — a verb turning a stroke into a filled path. `scene::lay_stroke` already expands
  outlines with `kurbo::stroke`.
- **Joining the ends of two path layers.** Join works only between open ends of one path
  (`tools::joinable_ends`, `canvas::join_points`).
- **Gradient editing.** On-canvas handles; a linear gradient's two ends (only an angle today, spanning
  the box: `paint::with_linear_angle`); a radial's radius and ellipse (`paint::with_radial_centre`,
  `GradientBrush::transform`); conic in the picker (`paint::PaintKind::Sweep` has no tab and no
  geometry editor, though the model and both renderers draw it; a fifth tab does not fit the strip
  beside *Variables* at 10pt).
- **Colour entry beyond hex** — RGB/HSL/HSB fields and CSS `rgb()`/`hsl()` input — and **recent
  colours** beside the document swatches (`picker::document_swatches`).
- **Select same** (fill, stroke, font, effect) and **select inverse**, in the palette
  (`shortcuts.md` *Deliberately unbound*).
- **Paste into the selected frame** (`app.rs` `paste_clipboard_at` returns a paste to its original
  parent) and **paste to replace** (`context-menus.md` §7: owes a rule for mismatched layer counts).
- **Paste properties carrying effects** — `build::Properties` takes fills, strokes and opacity only.
- **Adding layout to a loose selection** should infer direction, gap and padding from where the
  children sit and order them visually (`panels/layout.rs` `set_display` writes `Flex::default()`).
- **The insert picker drops at the top level only** (`kits/picker.rs`); a drop over a frame or flex
  container should land inside it.
- **Snapping to layout grids** — `snap.rs` never reads `Node::grids`.
- **Combine as variants** should parse `Size=L, State=Hover` names into properties
  (`variant::combine` makes one *Property 1*).
- **Groups in the styles browser** for slash paths (`variables.rs` `styles_tab`), as variables have
  (`Menu::Group`).
- **Descriptions** on components, styles and variables — no field on `Style`, `Variable` or a main.
  An agent over MCP reads them too.
- **Reordering modes**, so the default (the first) can change. `op.rs` has add, remove and rename only.
- **More number bindings**: font size, line height, letter spacing, weight, paragraph spacing,
  min/max W/H (`binding::FieldKey`).
- **Preferred mains for swap** (`components.md`).
- **Missing fonts, document-wide**: a list of the families the document lacks and a replace-all.
  Today a per-layer `WARN` glyph (`fonts::FamilyStatus::Missing`).
- **Fonts installed mid-session** — `fonts/mod.rs` loads system fonts once, at startup.
- **`Ctrl+B` / `Ctrl+I`** (`shortcuts.md` §10). Decide what bold and italic mean for a family with
  named faces, a `wght` axis or a `slnt` axis (`typography::bold_toggle` is a 400↔700 switch), then
  bind them (`input::TextChord`).
- **WebP export** (`export::ExportFormat`); WebP already imports.
- **Copy as PNG at a scale** (1× only, `context-menus.md`) and **copy as CSS** of a layer.
- **Export all frames** without first giving each an export row (`Ctrl+Alt+E` runs layers that carry
  specs).
- **Present mode**: step between frames, fit to frame, fullscreen, and no selection chrome. It only
  hides the panels today.
- **A typed zoom percentage** in `app.rs` `zoom_control`.

## Later · Before MCP

- **Pages.** `Document` has one root (and the kit vault). Settle it before MCP fixes the snapshot and
  tool surface around a single page.
- **Blend modes**, per layer and per effect. `shortcuts.md` §2 reserves `Shift+3`…`Shift+9`.
- **Background blur.** Canvas and raster draw the backdrop; SVG cannot express it and the code export
  has `backdrop-filter`, so the SVG writer owes an answer (a reported approximation, at least).
- **More than one open document** — `OndinApp` holds one `session`.
- **A light theme and a UI scale** — `theme.rs` is dark only; there is no scale setting.
- **Tools**: pencil (freehand), slices.
- **Shapes**: ellipse arcs and donuts, corner smoothing, a polygon's and star's corner radius.
- **Paths**: simplify, offset, and vector networks (`core-model.md`: every reader takes a `BezPath`).
- **Canvas**: lasso, tidy up, spacing handles on a loose multi-selection (`spacing.rs` covers
  containers).
- **Views and panels**: outline (wireframe) view, minimap, an undo history panel.
- **PDF** export (through the SVG and a converter, with a text-as-outlines decision;
  `export::ExportFormat`) and import.
- **Importing other tools' files**, in this order: **Sketch** (a published JSON spec; also covers
  Lunacy, which saves `.sketch`), **Figma** (`.fig` is reverse-engineered binary that breaks when
  Figma changes it; the REST API is documented but needs a network and the user's token), then
  **Penpot** (open source, so its format can be read from its code). Pixso works with `.fig`;
  Affinity and Illustrator arrive through SVG and PDF.
- **A default font preference** — `FontService::default_text_parts` is Inter.
- **Variable collections and scoping** — one mode list per document today (`styles-variables.md`).

## Later · Components and layout

- **Pushing an instance's changes to its main** (`architecture.md` §5.3d).
- **A verb that un-makes a component** — owes a detach of its instances, as a delete does (§15 D979).
- **CSS layout properties §5.3c defers** — non-`auto` margin, `order`, named grid areas,
  `auto-fill`/`auto-fit`, `calc`, `aspect-ratio`, block/inline/float.
- **A per-component review table for kit updates** — deferred, not declined (§15 D1010 (3), D1011 5D).
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
  specified beside used values (§15 D868).
- **Command Mode** — `architecture.md` §13, `vm.md`. Scripts stored inert, never run by the document
  (§15 D1100). Open in `vm.md` §13: the editor's UI, the parameter header, `mlua` exposing Luau's
  analysis, files outside the document (parked), first-run disclosure, schema place, kit scripts.
- **Multiplayer** — `architecture.md` §12; components bend the fourth property (§15 D978).
