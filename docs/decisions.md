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
- **D12** Polygon and Star are their own `NodeKind`s, not `Path`s.
- **D400** A frame's fill is its fill list, the same as every painted layer; there is no separate background field.
- **D28** The canvas ground is document state, saved with the file.
- **D119** A path's corner radius lives in the model, not in the geometry.
- **D114** A pen anchor carries two independent handles, not one mirrored vector.
- **D68** A rectangle's stroke can sit on one side; a side is an open path. **D69** Dash mode is classified from the stored pattern, never stored beside it.
- **D144** Frames stroke over their contents and outside their own clip. Type can be stroked, which is how outlined type is expressed.
- **D282** A mask is a layer that paints nothing and clips the run of siblings above it. Alpha masking only. A mask is never hit, and what it masks is hit only inside it.
- **D333** Layer effects are an ordered stack (drop shadow, inner shadow, layer blur, colour filters) on any node but the root, listed in model order. `Resolved::ink_bounds` is the drawn box and `world_bounds` the editing box. Background blur and blend modes are planned; noise is a non-goal.
- **D340** The effect stack is reordered by dragging its rows, as fills and strokes are, though order changes pixels only between `Filters` and `LayerBlur` and between two shadows on one side.
- **D176** Image fills use `peniko::Brush<ImageBrush<ImageRef>>` with our reference in place of pixels; there is no brush enum of our own. **D178** peniko has no transparent extend, so a fitted picture needs a clip as well as a brush transform.
- **D412** A gradient carries its own affine on the brush. An elliptical radial is a circular gradient under a squash. **D767** Gradient opacity is a field separate from the stops.
- **D301** Image data is `Arc<[u8]>`, so commits and undo never deep-copy pictures. **D600** Exactly PNG, JPEG, WebP and GIF are accepted, enforced by a magic-byte predicate (`images::signature_is_accepted`), not by Cargo features.
- **D358** There is one unit, px. Density is the export scale.
- **D868** Specified and used values are two numbers. Used boxes are derived in `Resolved` and never saved.

## Operations and the commit

- **D421** Finiteness is enforced at `Document::apply` (`OpError::NonFinite`); the loader does not check. Derived bounds are guarded too.
- **D428** A transaction that changes nothing is not an undo step: `Operation::changes_nothing` is exhaustive, with no wildcard arm. It cannot see real ops that net to zero, so planners guard those themselves.
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
- **D982** A set is a frame around mains; a variant is a main inside it with a derived name (`variant::settle` runs before `propagate`). A component property is a view over fields, never a stored value. A variant switch happens in place, matching by name path.
- **D983** Instance swap is a `swap` field beside `link`, rewritten in place by `swap::settle`, only to a main of the same kind.
- **D1010/D1012** A kit is a document marked as one. Its mains are vendored read-only into each consumer's vault (a second parentless root) with provenance, so a consumer works with the kit absent. Changes arrive only when the kit publishes and the consumer accepts.
- **D1016** A kit update rewrites a vendored main in place as one transaction. *Make local* forks the variables and styles the main reads.
- **D1037** A variable is typed (colour, number, string, boolean) with one value per mode, and can alias another. Modes are set per frame and inherited. A style is one slot's values, applied whole and never stacked; any edit detaches it.
- **D1039** Bindings are sidecars beside the literal, not a `Literal | Bound` enum. A write over a bound field detaches it at the commit; deleting a used variable detaches its users to the resolved value.
- **D1062** A styled layer holds a linked copy written by `style::settle`. The slots are fills, strokes, effects, text and layout.
- **D1064** A kit carries the variables and styles its mains read into the consumer's own tables, with an origin and a per-kit mode mapping. One planner, `tokens::plan`, serves insert, update and carrying paste.
- **A mode renamed or duplicated keeps the kit columns it reads**: the browser's doors write a mapping entry where the new name would match another kit mode (`variable::rename_mode_ops`, `copy_mode_ops`). A kit's variable may alias any kit's, never the document's own.
- **Bound texts are re-measured rather than traced**: every bound text's layout memo is dropped on each `Resolved::update`, so a commit re-laying a chain re-measures every bound text in it (21 ms at 200 bound labels).
- **A colour cell's preview patches every bound node each frame**, not only the variable's readers (3.7 ms a frame at 200 content-bound texts).
- **A text style made over a live selection takes the layer's character defaults**, not the selection's shown attributes (`style::value_drawn`).

## Text and fonts

- **D77** Five paragraph attributes are spanned (`ParaSpans`); the rest are node-level.
- **D598** `CharAttrKind`'s declaration order is the sort key of every saved span list.
- **D405** Type on a path is a rail on the text node: shaped flat, then bent by a warp pass.
- **Alignment never uses ink bounds**: they depend on the string, so labels aligned by ink jump apart when edited. Font-metric box trim is the datum. Optical margins move the ink to meet the datum; they don't move the datum.
- **D356** Underlines skip ink per run (`Decoration::skip_ink`, default on).
- **D83** The Google Fonts catalog is fetched online and disk-cached, not shipped with the build.
- **D330/D720** *Load web fonts* is one switch, and off means no request leaves the machine.

## Rendering and export

- **D774** eframe owns the wgpu device; `ondin-render` borrows it. Only `ondin-render` depends on vello.
- **D339** Each effect layer is recorded into a sub-scene, rasterized, filtered by `fx.wgsl` compute passes and handed back as an image. Sibling layers are packed one pass per renderer.
- **D395** Effect cost is bounded independent of zoom: heavy blurs run at reduced resolution and the batch is limited to a pixel budget. **D744** Effect buffers are capped at 8192 px. **D745** The canvas packs every picture into one 8192 px atlas; exports use source resolution.
- **D742** An inner shadow complements the tint, not the silhouette.
- **D414** `vello_cpu` is pinned exactly; raster goldens are per architecture and version. **D434** Raster output is capped at 65408 px per side.
- **D702** SVG export uses `ondin_render::effects` for effect arithmetic, so there is one implementation of it.
- **D780** SVG export reports its losses (`svg_of_reported`).
- **D81** SVG text is one `<text>` per line and one `<tspan>` per run. **D611** A layer's name is written as `data-name`.
- **D394** SVG import (`svg_in`, roxmltree) is a paste path into one `Transaction`. What it cannot hold is reported, never lost silently. **D806** It has a small CSS cascade.
- **D274** Export settings live on the layer and are saved with the document.
- **D1098** Code export is one DOM core writing HTML/CSS, React and Vue, with every downgrade reported. All three are built. In React and Vue a set is one component choosing its variant's tree by union props — one tree, each element choosing its class and a layer some variants lack shown by a condition, when the trees match by name path — and an instance is a use of a component only while it differs from the main in nothing but its boolean, text and swap properties, those of the nested instances its main shows (forwarded as the outer component's own props), and its root's size, visibility and mode (a use inside an element carrying `data-mode`, unless the main draws a mode's values where `var()` cannot carry them); otherwise it is written out as its layers and reported. A swap property is a prop taking a component, not an element or slot content: the slot's placement and size are its main's to write, and a passed element would arrive without them. In the app it is a one-shot of the selection, an `Action` with no chord or menu row, reached through the command palette.
- Code export's CSS tier holds only what headless Chrome draws as the canvas does: an all-sides stroke as an offset `outline` (on a frame's last child where it must paint over the children), per-side `border`s only inside, on square corners and a childless layer, linear, concentric radial and similarity sweep gradients, embedded, unadjusted, fully opaque image fills as `background` layers, one zero-spread drop shadow, and an automatic line height as the canvas's px. The rest goes to inline SVG.
- Code export's number variables are unitless in `tokens.css`, the unit added at the use site (`calc(var(--x) * 1px)`, `calc(var(--x) / 100)` for opacity), because one number can bind both a length and an opacity percentage.

## Files, library and storage

- **D362** Documents live in one base folder (default `~/.ondin`). Identity is a block at the top of the `.ondin` file, holding only user-changed values.
- **D363** A project is an id in each file's metadata; metadata wins over folder location.
- **D364** `Ctrl+S` pins a version under `.versions/`. Autosave is silent and only when dirty. There is no Save As.
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

## Workspace and build

- **D427/D775** Core and MCP crates stay free of UI and render dependencies, enforced by `deps_forbidden` walking `cargo tree --workspace` against a rules table.
- **D703** Shared dependency versions are pinned in the workspace `Cargo.toml`.
- **D445** `panic = "unwind"` is enforced by a `compile_error!`, since `catch_unwind` guards boolean evaluation and font decoding.
- **D303** `OndinApp::headless(&ctx)` builds the app without GPU, font threads or prefs, so app decisions are testable.
- **D304** SVG and JSON goldens are byte-compared; PNG goldens are not.
- **D1095** The MCP tool list covers the model as it is: reads, writes, components, variables, styles, kits and the library. **D1097** Invariant 10 covers document-level actions on §8.3's queue.

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
- **D345** Dropping in the layers panel chooses the parent level by horizontal travel.
- **D750** Present mode hides chrome but keeps guides and layout grids.
- **D749** The dashboard's default page is set only in Settings.
- **D768** Open and Recover ask or save before replacing the open document.
- **The eyedropper picks what is seen**, anywhere in the window: on the canvas the rendered pixel, composited over the ground, with egui's overlays never in it; elsewhere a frame of the window frozen while armed, chrome included. Hovering previews nothing on the canvas, since the next sample would read the preview.
