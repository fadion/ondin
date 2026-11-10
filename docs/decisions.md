# Ondin — key decisions

The decisions a developer has to know, or would otherwise argue again. Each is stated as it
stands now. The `D<n>` labels are the numbers these had in the old deviation ledger, so
the `§15 D<n>` citations in the code still lead somewhere. The full ledger is frozen at
`docs/archive/ledger.md` — grep it for one number; don't read it. No new D-numbers are
issued: a new decision goes here as a dated line without a number, or into the design file
it belongs to.

Decided non-goals are in `docs/non-goals.md`.

## Model

- **D21** The model type is `Artboard`; every user-facing word is "frame". Frames nest.
- **D12** Polygon, Star, Spiral and Arc are their own `NodeKind`s, not `Path`s, so their sides, points, turns and sweep stay editable.
- **D400** A frame's fill is its fill list, the same as every painted layer; there is no separate background field.
- **D28** A page's picked ground is document state, saved with the file. An untouched page has none and is drawn on the viewer's default: the app's theme canvas, `DEFAULT_CANVAS_BACKGROUND` headless.
- **A page is a parentless root listed in a page table**, not a node under one root: the structural operations, the loader's tree rules and the clipboard's rules hold unchanged (Delete, Reparent and Reorder refuse a page as a node), and `Document::new` stays a one-page document. Name and modes are the root's; ground and draft flag are the table's.
- **D119** A path's corner radius lives in the model, not in the geometry.
- **D114** A pen anchor carries two independent handles, not one mirrored vector.
- **A mirrored path stores which axes, not where they are**: each runs through the centre of the path's control box, so it cannot drift from the points, and a flag is kept only while the path is symmetric about it, so a writer that knows nothing of mirrors can drop an axis but never leave a false one.
- **D68** A rectangle's stroke can sit on one side; a side is an open path. **D69** Dash mode is classified from the stored pattern, never stored beside it.
- **D144** Frames stroke over their contents and outside their own clip. Type can be stroked, which is how outlined type is expressed.
- **D282** A mask is a layer that paints nothing and clips the run of siblings above it, by its outline, its alpha or its luminance (`MaskMode`). A mask is never hit, and what it masks is hit only inside it.
- **D333** Layer effects are an ordered stack (drop shadow, inner shadow, layer blur, background blur, colour filters) on any node but the root, listed in model order. `Resolved::ink_bounds` is the drawn box and `world_bounds` the editing box. A layer and each shadow carry a blend mode; noise is a non-goal.
- **D340** The effect stack is reordered by dragging its rows, as fills and strokes are, though order changes pixels only between `Filters` and `LayerBlur` and between two shadows on one side.
- **D176** Image fills use `peniko::Brush<ImageBrush<ImageRef>>` with our reference in place of pixels; there is no brush enum of our own. **D178** peniko has no transparent extend, so a fitted picture needs a clip as well as a brush transform.
- **D412** A gradient carries its own affine on the brush. An elliptical radial is a circular gradient under a squash. **D767** Gradient opacity is a field separate from the stops.
- **A gradient follows its layer's box.** Its points stay stored in the layer's own px, and a box changing size is composed into `GradientBrush::transform` as scale(new/old), exact for every kind and with no save-format change: on write when a resize changes the stored size, at draw time when layout or a bound size draws another box (`ondin_core::paint_box`). The box it is stored against reads a bound axis at its variable's default-mode number, never the placeholder 0. Without it, free ends on a resized shape end in a flat band of the last stop.
- **D301** Image data is `Arc<[u8]>`, so commits and undo never deep-copy pictures. **D600** Exactly PNG, JPEG, WebP and GIF are accepted, enforced by a magic-byte predicate (`images::signature_is_accepted`), not by Cargo features.
- **D358** There is one unit, px. Density is the export scale.
- **D868** Specified and used values are two numbers. Used boxes are derived in `Resolved` and never saved.
- **An inserted icon or photo carries its credit on its layer** (`Node::credit`): written once by the insert, never edited or drawn, kept by every copy and by an ungroup, dropped when a photo's picture is replaced. Exports write it once per file (SVG metadata, a comment atop code export's page), not per element.

## Operations and the commit

- **D421** Finiteness is enforced at `Document::apply` (`OpError::NonFinite`); the loader does not check. Derived bounds are guarded too.
- **A node's stored geometry, transform, pivot and layout lengths are clamped to one limit, `build::GEOMETRY_LIMIT`, and its stroke lengths to `node::MAX_STROKE_WIDTH`, at the operations and the loader, never refused**, so a drag past it commits at it rather than failing mid-gesture; the app's fields share the numbers (`ui::FIELD_LIMIT`, `ui::MAX_STROKE_WIDTH`). The limit bounds each number, not their products: a layer whose extent overflows or passes `scene::MAX_DEVICE_EXTENT` on the device is skipped by the scene walk instead.
- **D428** A transaction that changes nothing is not an undo step: `Operation::changes_nothing` is exhaustive, with no wildcard arm. It cannot see real ops that net to zero, so planners guard those themselves.
- **An undo step carries the name its door gave at the commit** (the command's menu name, or the MCP tool's), never one derived from its operations. The open session's history is uncapped.
- **D416** `io::MAX_TREE_DEPTH` (256) bounds the tree at load and at the op layer, because a stack overflow kills unsaved work.
- **D491** A transaction that orphans a guide is refused. **D492** Unusable export specs are refused at the op and dropped on load.
- **D439** A new container (group, frame, boolean, flatten, mask) takes the topmost member's z-slot, through one `container_slot`.
- **D832** The insert door checks inserted subtrees for reachability and depth.
- **D55** The pivot is baked by the gesture that reads it, never composed into a layer's world transform.
- **D5** The inspector shows X/Y and rotation in world space, projected back through `build::place_at_world`. W/H are geometry edits, not scale.
- **D321** A locked group locks everything inside it, through one query, `query::is_effectively_locked`. A lock guards canvas accidents, not deliberate panel edits. **D746** Bulk selection never reaches a locked layer (`may_select_in_bulk`).
- **A selection holding a locked layer is not dragged on the canvas**: no handle arms, no move starts, no spacing band is offered. The handles stay drawn and the size badge's padlock says why; the keyboard nudge does not ask.
- **D113** A multi-selection can have a key layer (Alt+Shift+click). With none, align uses the union and boolean uses its own base operand.

## Layout

- **D867** Container layout is CSS flexbox and grid, computed by taffy over Ondin's own nodes with no mirror tree. Where CSS and design-tool habit disagree, CSS wins. It is a fixed algorithm re-run from specified values, not a constraint engine.
- **D873** Used values are quantized to 1/64 px (`container::quantize`).
- **D869** A group with `display` has a box; without it, it is a plain group. **D870** A frame may sit in any group, but never under a boolean or a mask.
- **D871** Constraints are `position: absolute` with insets. **D874** Tools edit from drawn geometry, and `build::keep_insets` turns the result into insets at the commit door.
- **D872** Every leaf is measured; shapes are replaced elements and take their stored geometry as intrinsic size.
- **D875** In flex, auto-width text never wraps, auto-height text wraps at the given width. Resizing an item writes grow/shrink 0 (`build::keep_flex_sizes`). Hidden layers, masks and pinned children leave the flow.
- **D877** Dragging an in-flow item reorders it. Several items of one container drag as a block. **D894** Pinning never moves an item. **D896** An in-flow item turns about its box centre.
- **D909** `Start`/`End` are `flex-start`/`flex-end`.
- **D913/D914** Grid shares one item record with flex, tracks use CSS's grammar, and ops refuse what CSS refuses. A drag writes explicit lines.
- **D929** Any door that takes a layer out of a layout writes it where it is drawn first.
- **D385** Layout grids and frame presets are chrome: stored on frames, drawn only as canvas overlay, never exported.
- **D1078** Spacing handles draw a container's gaps and padding as canvas bands, writing one `SetDisplay` through the commit door.

## Components, kits, styles and variables

- **D978** Instances are linked copies written at the commit, not subtrees derived in `Resolved`.
- **D979** An override is any value that differs from its source; nothing records it. Instance structure is free, deleting a main detaches its instances, components are document-local, and copying a main makes an instance.
- **D980** The five item lists (fills, strokes, effects, exports, layout grids) carry ids in a `Keyed<T>` wrapper, so an instance follows its main item by item. A list reset restores the main's list exactly.
- **D982** A set is a frame around mains; a variant is a main inside it with a derived name (`variant::settle` runs before `propagate`). A component property is a view over fields, never a stored value. A variant switch happens in place, matching layers by lineage key, else by name path.
- **Variant layers pair by a shared lineage key, name path the fallback.** Copying a variant into its set gives each copied layer its source's key (`Node::lineage`), so the switch, the code export's merge and its states pair layers across a rename. A shared key, not a pointer to the source: a pointer breaks on a chain of copies and when the source is deleted.
- **D983** Instance swap is a `swap` field beside `link`, rewritten in place by `swap::settle`, only to a main of the same kind.
- **Push to main goes one link up**: an instance writes its differences into the node Reset compares it with, never past the main holding that node, and the commit's passes carry them on to the other instances. A removal deletes the main's layer and is only pushed on its own, never in *Push all*.
- **D1010/D1012** A kit is a document marked as one. Its mains are vendored read-only into each consumer's vault (a parentless root beside the pages, never one of them) with provenance, so a consumer works with the kit absent. Changes arrive only when the kit publishes and the consumer accepts.
- **D1016** A kit update rewrites a vendored main in place as one transaction. *Make local* forks the variables and styles the main reads.
- **A kit update is taken per component, its variables and styles whole**: an unticked main stays at its version, so its kit stays offered, and a variable or style the publish removes stays while a held main reads it. Taking a main without one its new version reaches is prevented by the review's ticks (`Update::needs`).
- **D1037** A variable is typed (colour, number, string, boolean) with one value per mode of its collection, and can alias another. Modes are set per frame, one per collection, and inherited. A style is one slot's values, applied whole and never stacked; any edit detaches it.
- **D1039** Bindings are sidecars beside the literal, not a `Literal | Bound` enum. A write over a bound field detaches it at the commit; deleting a used variable detaches its users to the resolved value.
- **A bound paint keeps its own `%`, which multiplies the colour variable's alpha**, as a paint's opacity composites over its colour in Figma and in CSS: a 50% fill bound to a 50% black draws 25%. Binding is choosing the variable's value, so `--` and the menus keep the `%` as it was (`build::bind_brush_at_all`). Two doors keep what is drawn instead: *Extract into variable…* files the colour opaque, and *Bind other uses* sets each paint's `%` so the colour on screen does not change.
- **D1062** A styled layer holds a linked copy written by `style::settle`. The slots are fills, strokes, effects, text and layout.
- **D1064** A kit carries the variables and styles its mains read into the consumer's own tables, with an origin and a per-kit mode mapping. One planner, `tokens::plan`, serves insert, update and carrying paste.
- **A mode renamed or duplicated keeps the kit columns it reads**: the browser's doors write a mapping entry where the new name would match another kit mode (`variable::rename_mode_ops`, `copy_mode_ops`). A kit's variable may alias any kit's, never the document's own.
- **A variable's collection is the one owning its cells' modes** (`Variables::collection_of`); it records no collection field, so moving one is re-keying its cells under the same id, and bindings and aliases hold.
- **A frame sets a mode set, at most one per collection** (`Node::modes`, `SetModes` written whole); each collection inherits on its own, and an instance's set is one override field.
- **Breakpoints are a collection picked by width, not per-node overrides**: at most one collection carries a minimum width per mode, and a top-level frame (no main, set or instance) is the viewport, picking the mode its width reaches; a pin on the frame, then on its page, beats the width, and nested frames inherit it. Code export writes those modes as `@media (min-width)` blocks.
- **A component picks by its own width through a width query, CSS's container query**: a frame's query answers for its contents, never for the frame itself, so the width it picks by never depends on what it picks. The width is the content box, as CSS measures a container; code export writes it as `@container`, and the laid-out widths settle after every resolve until no pick moves (`Resolved::settle`).
- **A kit collection maps from the document collection of its name, else the first** (`Variables::kit_onto`); within it a kit mode is read by hand-set entry, then by name, then its first.
- **Bound texts are re-measured rather than traced**: every bound text's layout memo is dropped on each `Resolved::update`, so a commit re-laying a chain re-measures every bound text in it (21 ms at 200 bound labels).
- **A colour cell's preview patches every bound node each frame**, not only the variable's readers (3.7 ms a frame at 200 content-bound texts).
- **A text style made over a live selection takes the layer's character defaults**, not the selection's shown attributes (`style::value_drawn`).
- **A colour merge into a variable binds what can bind and writes the value into the rest.** Only fills and strokes bind to a colour variable, so Colour review's merge gives gradient stops, shadow colours and text colours the variable's default-mode value as a literal and says how many in the status line, rather than skipping them (`color_review::Merged::unbound`).
- **A design-tokens import matches a token by the id the export wrote, then by path**, so a rename made in code keeps its bindings. It is reviewed row by row and applied as one transaction; a linked file offers only what it changed since it was last applied (`dtcg_in::review`, `Review::since`).

## Text and fonts

- **D77** Five paragraph attributes are spanned (`ParaSpans`); the rest are node-level.
- **D598** `CharAttrKind`'s declaration order is the sort key of every saved span list.
- **D405** Type on a path is a rail on the text node: shaped flat, then bent by a warp pass.
- **Alignment never uses ink bounds**: they depend on the string, so labels aligned by ink jump apart when edited. Font-metric box trim is the datum. Optical margins move the ink to meet the datum; they don't move the datum.
- **D356** Underlines skip ink per run (`Decoration::skip_ink`, default on).
- **Bold and italic are the family's own faces or axes; nothing is synthesized.** No faux bold or oblique is drawn, so `Ctrl+B`/`Ctrl+I` refuse a family with nothing that way rather than write a flag that would draw unchanged (`text::bold_step`, `italic_step`).
- **D83** The Google Fonts catalog is fetched online and disk-cached, not shipped with the build.
- **D330/D720** *Load web fonts* is one switch, and off means no request leaves the machine.
- **The assets browser's icon sets are pinned Iconify packages, fetched whole once and cached per machine**, so they work offline; photos are searched online with the user's own keys, kept in the system keychain and sent only to their own service. Nothing reaches the network until a set or service is switched on in Settings.

## Rendering and export

- **D774** eframe owns the wgpu device; `ondin-render` borrows it. Only `ondin-render` depends on vello.
- **D339** Each effect layer is recorded into a sub-scene, rasterized, filtered by `fx.wgsl` compute passes and handed back as an image. Sibling layers are packed one pass per renderer.
- **D395** Effect cost is bounded independent of zoom: heavy blurs run at reduced resolution and the batch is limited to a pixel budget. **D744** Effect buffers are capped at 8192 px. **D745** The canvas packs every picture into one 8192 px atlas; exports use source resolution.
- **D742** An inner shadow complements the tint, not the silhouette.
- **A screen draws at a level of detail; an export never does.** The canvas and the presentation view skip ink under half a device pixel, each layer judged alone (so a dense pattern of sub-pixel layers vanishes together), draw a layer plain when its shadows and blurs reach under one, and greek type under four device pixels (`RenderOverrides::set_level_of_detail`); every export and thumbnail draws the document exactly.
- **`Resolved` caches every shape's outline** (`shape_path`), kept equal to `rebuild` like its other maps: rebuilding them was ~8 ms of a 31k-layer page's scene walk on every draw, and during a gesture only the reshaped layers fall back to building their own.
- **The canvas draws only when what it draws from changes** (`CanvasRenderer::render`'s `DrawnKey`); egui's own repaints keep the texture. A preview or re-shaped text moves `EditorSession::drawn`, never `revision`, which other caches read as "the document changed"; re-shaped text also moves `EditorSession::shaped`, so the canvas never takes it for a preview change. A preview change redraws only the page's top-level items it touches, measured under both previews (`scene::damage`); a commit, undo or redo redraws only the top-level layers it dirtied, their old ink recorded before `Resolved` updates (`InkLog`) and their new ink measured.
- **The camera puts the world origin on a whole device pixel** (`Camera::viewport` rounds the view's corner; the centre stays exact). A pan then moves the whole drawing by whole pixels, so the canvas moves its last frame and draws only what came into view; the price is a zoom anchor that holds to within a device pixel rather than exactly. A frame whose pixels depended on where the canvas sat (`VelloGpuRenderer::pinned`) is drawn whole instead.
- **A blend mode is the outermost layer of a node**, outside the effect layer, so a layer's shadow blends with it; the SVG writer's blend wrapper sits outside its filter wrapper for the same reason. Only the sixteen modes CSS, SVG and PDF share are offered, so every export agrees with the canvas.
- **A page and a top-level frame isolate when they hold a blended layer** (a mode or a visible shadow's mode), so what is inside blends with the page's or the frame's content and never the canvas ground: the canvas looks the same as every export, which lays its ground after drawing.
- **A shadow with a blend mode is drawn in its own pass**, since it blends with what lies under the layer and the effect buffer does not hold that; a stack without one keeps the single effect layer. An SVG filter cannot see beneath its element, so the SVG writer draws the shadow Normal and reports it.
- **D414** `vello_cpu` is pinned exactly; raster goldens are per architecture and version. **D434** Raster output is capped at 65408 px per side.
- **D702** SVG export uses `ondin_render::effects` for effect arithmetic, so there is one implementation of it.
- **D780** SVG export reports its losses (`svg_of_reported`).
- **D81** SVG text is one `<text>` per line and one `<tspan>` per run. **D611** A layer's name is written as `data-name`.
- **PDF is our SVG converted** (`usvg` and `krilla`), not a third writer, and its text is either embedded fonts or our own glyph outlines, a per-spec choice SVG shares.
- **D394** SVG import (`svg_in`, roxmltree) is a paste path into one `Transaction`. What it cannot hold is reported, never lost silently. **D806** It has a small CSS cascade.
- **D274** Export settings live on the layer and are saved with the document.
- **D1098** Code export is one DOM core writing HTML/CSS, React and Vue, with every downgrade reported. All three are built. In React and Vue a set is one component choosing its variant's tree by union props — one tree, each element choosing its class and a layer some variants lack shown by a condition, for every variant whose tree pairs with the default's — and an instance is a use of a component only while it differs from the main in nothing but its boolean, text and swap properties, those of the nested instances its main shows (forwarded as the outer component's own props), and its root's size, visibility and mode (a use inside an element carrying `data-mode`, unless the main draws a mode's values where `var()` cannot carry them); otherwise it is written out as its layers and reported. A swap property is a prop taking a component, not an element or slot content: the slot's placement and size are its main's to write, and a passed element would arrive without them. In the app it is a one-shot of the selection, an `Action` with no chord or menu row, reached through the command palette.
- **Interaction states are a variant property whose values are tied to browser states, written as CSS diffs against rest.** Its values tie to `:hover`, `:focus-visible`, `:active` and `:disabled`, and a tie is what makes the property the interaction state: no separate mark, so a set is never interactive with nothing tied, and name-based guessing waits for the first deliberate tie. Rest is the default variant's value. Each state writes only what differs from its rest variant; a state whose layers do not pair is written behind the prop alone and reported. The prop stays, for forcing a state, and the forced `data-state` is the value's exact name, since two values may differ only by case.
- **A layer's HTML element is a field the designer sets, never guessed from names**, offered by kind and refused by the menu where the HTML would be invalid; the export writes one that has since become invalid as Automatic and reports it. It is the main's on every copy, never an override, and a component set has one element on all its roots, which is what its interaction states' root is written as.
- Code export's CSS tier holds only what headless Chrome draws as the canvas does: an all-sides stroke as an offset `outline` (on a frame's last child where it must paint over the children), per-side `border`s only inside, on square corners and a childless layer, linear, concentric radial and similarity sweep gradients, embedded, unadjusted, fully opaque image fills as `background` layers, one zero-spread drop shadow, and an automatic line height as the canvas's px. The rest goes to inline SVG.
- Code export's number variables are unitless in `tokens.css`, the unit added at the use site (`calc(var(--x) * 1px)`, `calc(var(--x) / 100)` for opacity), because one number can bind both a length and an opacity percentage.
- **`tokens.json` writes each collection as a top-level group** once a document has two, its modes and `valueMode` in the group's extension; a one-collection document writes as before, so its files do not change.
- **A number's DTCG `$type` is its scope's when it has one** (lengths only `dimension`, weight only `fontWeight`, else `number`); an unscoped number is typed by what it binds, so existing exports do not change.

## Files, library and storage

- **D362** Documents live in one base folder (default `~/.ondin`). Identity is a block at the top of the `.ondin` file, holding only user-changed values.
- **D363** A project is an id in each file's metadata; metadata wins over folder location.
- **D364** `Ctrl+S` pins a version under `.versions/`. Autosave is silent and only when dirty. There is no Save As.
- Version history says under each version what it changed against the one pinned before it, counted as the compare counts (topmost changed layers). The summary is worked out off the UI thread and cached for the app's life by the pair's two paths, since a pinned version never changes.
- **D366** Recent files, stars and covers are local to the machine.
- **D377** Crash recovery is a whole-document snapshot every 10 s, not a journal. **D393** One writer thread serialises autosave, snapshots and forgets; `Ctrl+S` stays synchronous.
- **D378** Every record is written atomically (`atomic::write`: temp, fsync, rename). **D975** It keeps the file's mode and writes through a symlink.
- **D418** A record that failed to read is never overwritten with defaults.
- **D419** Anything that becomes a filename is validated where it is parsed.
- **D638** The schema-version guard is `u64`; a too-new file is refused.
- **D663** The writer normalizes inert state, so `save(load(save(d))) == save(d)`.
- **D823** Cross-window copy is the document schema as JSON on the OS clipboard (`io::clip`). **D796** Clipboard handles are serialized process-wide.
- **D1100** Scripts may be stored in the file as inert text; the document never runs them.
- **Filesystem is truth.** Every action writes to disk and re-scans. Nothing about a document lives only in the app's memory, and no fact lives only in a cache.
- **The format stays git-friendly**: pretty JSON (line-level diffs), an id-sorted node list, stable string ids, byte-stable output (invariant 9), the camera in `EditorSession` rather than the DTO, and `skip_serializing_if`. A modified-at stamp or view state in `DocumentDto` would break all of it, so an edit time is read off the directory entry. An edit touches only its node and its parent, with no cascade across unrelated nodes.
- **Autosave is debounced**: writing on every op into a synced folder thrashes the sync client.
- **Leaving a tab saves it**, as leaving for the library does, instead of an autosave tick running for tabs behind: a tab behind is clean unless it cannot be saved, and nothing is written behind the user's back while it waits.
- **Undo history is kept per document in the library** (`.history/<document id>.json`), never beside the file, so it does not travel with a copy. It is read back only for byte-identical file contents written by the same version (`CARGO_PKG_VERSION`): a history replayed onto any other document would corrupt it. Two development builds of one version share the check; released versions never do.
- **Another tool's file (Sketch, Penpot, a fetched Figma file) and a placed PDF are parsed in a child process** (`ondin reader`), the result crossing back as save-format bytes the app loads and so re-validates: a reader's stack overflow or failed allocation aborts its process, which no `catch_unwind` sees. Each reader keeps its own bounds; the child is the guard for what they miss. `.ondin` loads and SVG import stay in the app, bounded by their own depth limits. The child only parses (the app fetches, and keeps the Figma token) and has no memory cap.

## Workspace and build

- **D427/D775** Core and MCP crates stay free of UI and render dependencies, enforced by `deps_forbidden` walking `cargo tree --workspace` against a rules table.
- **D703** Shared dependency versions are pinned in the workspace `Cargo.toml`.
- **D445** `panic = "unwind"` is enforced by a `compile_error!`, since `catch_unwind` guards boolean evaluation and font decoding.
- **D303** `OndinApp::headless(&ctx)` builds the app without GPU, font threads or prefs, so app decisions are testable.
- **D304** SVG and JSON goldens are byte-compared; PNG goldens are not.
- **D1095** The MCP tool list covers the model as it is: reads, writes, components, variables, styles, kits and the library. **D1097** Invariant 10 covers document-level actions on §8.3's queue.

## Agents and Command Mode

- **AI comes through a configured agent harness, never an embedded model or a provider key of Ondin's own** (2026-11-10). The person picks a headless coding agent they already use and are signed in to; Ondin starts it, points it at its own MCP endpoint and shows the turn in a chat window, with layers attached from the canvas (`ai.md`). The harness holds the model, the account and the billing. Command Mode's `/ai` is a door into the same assistant and is built after it.
- **Leap labels outline what they label** (2026-11-10): dashed for a group or frame with a dark label, solid for any other layer with the yellow label, and the first typed letter keeps outlines only on what it still matches (`foundations.md` §13). In a tight spot the question is *the group, or the layer inside it?*, and line style and label lightness answer it without hue.

## Distribution

- **D952** The window draws its own title bar on Windows and Linux and keeps the system's on macOS.
- **D953** The release `ondin.exe` is a GUI-subsystem binary; `ondin.com` is its console twin.
- **D954** Velopack auto-update from GitHub Releases applies only at the exit the user asked for, never while another Ondin runs. `ONDIN_NO_UPDATE_CHECK` opts out.
- **D955** CI builds and tests on Linux, Windows and macOS.
- **D956/D976** A `v*` tag builds and publishes every platform. Build steps hold a read-only token; one `publish` job writes. Every action in a job that holds a write token or the package signing key (`release.yml`'s `publish`, `pages.yml`'s steps after the key import) is pinned by commit, since a tag can be moved.
- **D957** Signed apt and dnf repositories are rebuilt on GitHub Pages. `install.sh` pins the key fingerprint.
- **D969** The Windows data folder is `OndinData`, because `%LocalAppData%\Ondin` belongs to the installer.

## Interface

- **D134** A control that does not apply is shown disabled, never removed.
- **Every form control follows one standard, held by the shared primitives in `ui.rs`** (`app-chrome.md`, *Shared rules*): one height (28, with one named toolbar scale of 24), one ground, one hover that never lights the hairline, one row height for every list, sizes the primitive picks rather than the caller. A primary action is a state of its own, separate from a latched On.
- **D747** The panels are keyboard-traversable: Tab is meant to reach every control and field, not only a dialog's.
- **The chrome has a dark and a light palette; the canvas marks are theme-independent.** Chrome colours are read through functions of the current theme; selection, key, component, measure and point marks stay constants, since they sit on artwork.
- **The app ships its own icon set** (`OndinIcons.ttf`, built from `assets/icons/glyphs.json`) rather than a third-party one: each icon is drawn for the size it is shown at, so no picture is a compromise forced by what another set happens to contain.
- **Over the canvas the app draws every cursor, the arrow included**: a base drawing with at most one badge saying what a press there would do, one badge meaning one thing on every base, in the OS's own ink set and pointer size. Panels, menus and window edges keep the system cursor.
- **D345** Dropping in the layers panel chooses the parent level by horizontal travel.
- **Only a frame on its page is a page container**, as in Figma: it wears a name tag and is picked by it or its edge. A frame inside another frame or a group wears no tag and picks as a group does, so a design of nested frames is not a wall of names.
- **D750** Focus mode hides the panels but keeps the rulers, guides and layout grids: they are editing aids, not app chrome.
- **Focus mode and present mode are separate modes**: focus mode is for editing with the chrome hidden; present mode is view-only.
- **Present mode is its own branch of `OndinApp::ui`**, which reads only its own keys and returns, not a flag the overlays and commands check.
- **Private mode hides names, not content, and is a saved preference**: every drawn layer, page, document and project name is masked as its kind and place while the work itself stays on screen, and a restart in the middle of a shared screen comes back masked.
- **A contrast fix passes in every mode or is not offered.** A raw colour is the same in all modes, so the nearest colour must pass every mode's worst backdrop and only variables passing in every mode are listed; when no lightness at the hue passes everywhere, only variables are offered.
- **Renaming is two verbs**: *Rename* (`Ctrl+R`, `F2`) is the layers tree's field for one layer, *Batch rename…* (`Ctrl+Shift+R`) a box for many. With two or more layers selected `Ctrl+R` opens the box rather than refusing, the verb that fits them.
- **A layer's context menu fits without a large monitor, using submenus**: the other-format copies and the two alternate clipboards are one *Copy/Paste as ▸*, the four boolean operations one *Boolean ▸*, the six aligns one *Align ▸* (distribute stays off), the chord-driven restacks sit behind *Arrange ▸*, Flip H/V (one click in the inspector) and *Zoom to selection* are off it, and *Rename*/*Batch rename…* share the one row `Ctrl+R` stands for.
- **D749** The dashboard's default page is set only in Settings.
- **Which page is current is view state, never saved in the file**: switching pages is no edit and no undo step, each page's camera and selection are kept for the sitting only, and the library remembers per machine the page a document was left on.
- **D768** Open and Recover never replace an open document: each opens in a tab of its own, or brings forward the tab already holding that file. A document is never open twice in one window, by any spelling of its path, since two sessions saving one file would overwrite each other. Two Ondin processes can still each open it.
- **The colour picker's tabs are Solid · Gradient · Variables**; the gradient kind (Linear, Radial, Angular) is a menu on the geometry row's preview chip. A tab per kind does not fit beside *Variables* at 10pt, and a menu adds no row, so every face keeps one height and another kind costs one menu row.
- **The open picker is the gradient editing mode**: no tool of its own. While it shows a gradient fill or stroke of the one selected layer, the canvas draws that gradient's handles in place of the selection box.
- **The eyedropper picks what is seen**, anywhere in the window: on the canvas the rendered pixel, composited over the ground, with egui's overlays never in it; elsewhere a frame of the window frozen while armed, chrome included. Hovering previews nothing on the canvas, since the next sample would read the preview.
