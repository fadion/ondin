# 8. ondin-mcp

The MCP endpoint: the running Ondin process hosts it, agents connect over a local socket or a stdio proxy, and every write goes through the same op queue and commit door as the app's own edits.

## 8. ondin-mcp

### 8.1 Topology: the app is the server

The document lives in a running Ondin process (GUI or headless), so that process hosts the MCP endpoint and agents connect to it.

- It listens on a local socket: Unix domain socket (Linux) or named pipe (Windows), at a well-known per-user path, with a discovery file recording endpoint and open document path: `$XDG_RUNTIME_DIR/ondin/<pid>.json` on Linux, `machine_dir::root()/run/<pid>.json` on Windows. It is not `%LOCALAPPDATA%\ondin\`, which is the Velopack installer's own folder under another case (Setup deletes it, uninstall empties it, and a non-empty one makes Setup ask about an existing installation).
- A thin **stdio proxy** (`ondin mcp-proxy`) bridges agents that spawn an MCP server over stdio: it connects to the running instance's socket and forwards. With no instance running it exits with an error naming `ondin serve`.
- With several documents or instances, the proxy picks by document path argument, else the single running instance, else errors listing candidates.

### 8.2 Headless mode

`ondin serve <file.ondin>` opens a document with no window: core, history, CPU renderer and the MCP endpoint. Same op queue, same tools, so an agent can build, edit and export on a machine with no display. This follows from the headless-core invariant and is a first-class v1 deliverable.

### 8.3 Concurrency model

- The owning process holds `Document`, `History` and `Resolved` on its main thread.
- All mutations funnel through a single-writer **op queue**: `mpsc<OpRequest { tx: Transaction, reply: oneshot<Result<ApplyOutcome, OpError>> }>`.
- **Document-level actions** (invariant 10, §15 D1097): `make_kit`, `publish_kit(note)`, `stop_sharing_kit`, `create_document(name, project?)` ride the **same queue** as a second request kind, `ActionRequest { action, reply }`, so they are ordered against surrounding edits (a publish sent after a batch of writes publishes those writes). The owning thread runs the app's own code: `make_kit`, `publish_now`, `stop_sharing_kit` (the verbs the document menu's rows end in, past their dialogs) and `library::store::file_document`, and replies with the result. None is a `Transaction`, none enters history, `undo` reverts none.
- MCP handlers (async, own thread/runtime) build transactions with their session `IdSource`, submit to the queue and await the reply. The GUI pumps the queue every frame; `serve` pumps it in its main loop. One source of truth and one history; AI edits appear live on the canvas through the same path as UI edits. An MCP edit's undo step is named by the tool it called and remembers the client (`EditorSession::name_mcp_step`, `Origin::Mcp`), so the History section can mark it; the transport does not call it yet.
- No MCP handler ever touches `Document` directly (invariant 10).

### 8.4 Tools

Phase 1 — read/export (ship first):
`get_document() -> Snapshot`, `get_node(id)`, `list_children(id)`, `hit_test(x,y)`,
`get_bounds(id)`, `get_selection() -> Vec<NodeId>`, `export_svg(artboard?)`, `export_png(viewport)`,
`run_exports(ids?) -> Vec<File>`, `export_code(target, ids?) -> CodeExport`, `list_components()`, `list_variables() -> VariableTable`,
`list_styles() -> StyleTable`.

Phase 2 — write (1:1 with operations/composites):
`create_node`, `insert_subtree`, `delete_node`, `reparent`, `reorder`, `group(ids)`, `ungroup(id)`,
`duplicate(id)`, `set_transform`, `set_pivot`, `set_geometry`, `set_name`, `set_visible`,
`set_locked`, `set_opacity`, `set_clip`, `set_mask(id, mode?)`, `set_text`, `set_text_style`,
`set_text_spans`, `set_paragraph_style`, `set_block_style`, `set_fills`, `set_strokes`,
`set_effects`, `add_image(bytes) -> ImageId`, `set_display`, `set_layout_item`, `set_insets`,
`set_exports`, `undo()`, `redo()`, `set_selection(ids)`.

Phase 3 — components, variables and styles (through the verbs, never the raw ops):
`make_component(id)`, `create_instance(main, parent, at)`, `detach_instance(id)`,
`swap_instance(id, main)`, `combine_as_variants(ids)`, `switch_variant(id, property, value)`,
`define_property(main, field)`, `set_instance_property(id, property, value)`,
`reset_overrides(id, scope)`, `add_variable(kind, name, value)`, `remove_variable(id)`,
`rename_variable(id, name)`, `set_variable_value(id, mode, value | alias)`, `add_mode(name)`,
`remove_mode(id)`, `rename_mode(id, name)`, `set_frame_mode(id, mode?)`,
`bind_variable(id, field, var?)`, `add_style(slot, name, value)`, `remove_style(id)`,
`rename_style(id, name)`, `set_style_value(id, value)`, `apply_style(id, slot, style?)`.

Phase 4 — kits and the library (the app's own actions, some outside undo):
`list_documents()`, `create_document(name, project?) -> Path`, `list_kits()`, `get_kit(kit)`,
`insert_from_kit(kit, main, parent, at)`, `list_kit_updates()`, `accept_kit_updates(kit?)`,
`make_local(ids)`, `map_kit_modes(kit, mapping)`, `make_kit()`, `publish_kit(note)`,
`stop_sharing_kit()`.

Selection is app state, not document state (§9.3), but deliberately part of the MCP surface, because "the selected layer" is how humans phrase most AI requests. The MCP layer reads and writes it in the owning process's session state, never the `Document`.

Which of this is built: the `ondin-mcp` crate holds the tool list (`PHASE_1` to `PHASE_4` in `tools.rs`); the headless commit path and the Phase 4 app-side half are not reachable from `serve` yet (see "Not reachable headless" below). The editor itself is built; M6 (the MCP milestone) is parked until the editor is complete.

#### Conventions

- `create_node`, `insert_subtree` and `duplicate` return the new ids (the handler minted them); so do `create_instance`, `insert_from_kit`, `add_image`, `add_variable`, `add_mode` and `add_style`.
- The snapshot exposes world transforms so agents can compute local placements; a `place_at_world(id, world_transform)` adapter is allowed in core if agents struggle.
- `insert_subtree` is the tool a whole layout goes through: a page of a few hundred layers is a handful of calls, one tree per section, where `create_node` is a round trip per layer.
- `export_png` is the agent's eyes: placement errors no read tool reports (a card under the band below it, a label over a button) show only in the render. A client that can see images is expected to export, look and correct.

#### The commit door

Every write goes through the commit door the app's edits use (`EditorSession`), so the same passes run for an agent's transaction: `keep_insets` and `keep_flex_sizes`, the eight `keep_*_bindings` passes, then `propagate::owed`, which runs `style::settle`, `swap::settle`, `propagate_structure`, `component::settle_links`, `variant::settle`, `propagate`, `style::settle` once more, `keep_size_bindings` again and the inert-spacing clear, in that order (pinned by core tests). An agent never has to know a copy exists to keep it in step with its main.

**Not reachable headless**: that sequence lives in `ondin-app` (`EditorSession::commit_inner`). The component and style half is one core function (`propagate::owed`) but the passes before it (the last `paint_box::keep_gradients`) are called in that order nowhere else, and `ondin-app` has no lib target (§15 D786's wall). The sequence must become reachable without a window before `serve` can honour the paragraph above. The door also reads an edit of a styled layer as an edit of the style while the style browser's *Edit* is open (`style::edit_through`, first in `commit_inner`, §15 D1063), so an agent's write landing then would change the style and all its users; the GUI half of M6 must answer that.

#### Phase 3: verbs, not ops

Phase 3 names verbs because the raw ops are refused or wrong alone:

- Binding a field writes a canonical literal beside the binding (`build::bind_*_ops`: 0 for a number, empty for a string, `true` for a visibility), so a main and a copy bound to one variable compare equal whatever either held. A colour binding replaces the paint's brush outright, keeping its alpha percentage (`BrushSource::Bound`, `build::bind_brush_at_all`).
- Removing a variable or style detaches its users to what they drew in the same transaction (`variable::remove_ops`, `style::remove_ops`); removing a mode returns its frames to *Inherit* (`variable::remove_mode_ops`). Each is refused at the commit otherwise (§15 D1038). A bare `RemoveStyle` or a bare `SetCornerVars` over a non-canonical literal would be refused or wrong.
- Renaming a mode keeps every kit column it reads (`variable::rename_mode_ops`); a bare `SetModeName` re-maps a kit by the new name.
- Making an instance writes a linked copy (§15 D978).
- `bind_variable`'s `field` names any bindable field: corner radii, gaps and padding, width and height, a layout item's min/max width and height, visibility, opacity, a text's content, family, size, weight, line height, letter spacing and paragraph spacing, a fill's or stroke's colour, a stroke's width, and a shadow's X, Y, blur and spread or a layer blur's radius. An effect's colour and a filter's numbers bind nothing. It is one tool over the six `Set*Vars` ops plus the bindings a fill, stroke, effect and layout item carry in their own items (`BrushSource`, `LayoutItem::limit_vars`); `var: None` detaches to the value drawn. An item's binding names its item: the core verbs take a stroke's or effect's `ItemId`, and a paint's colour its index in the list.

#### Reads must grow with Phase 3

A bound field's stored literal is a placeholder, so the snapshot reports the **drawn** value of every bound field. It does that today for a paint's colour, a stroke's width, an effect's numbers, size and corners (through `Resolved`'s used kind), visibility, opacity, and a text's content and family. Still open: it names no binding, and does not report each node's style per slot, its component link (main, instance, variant, overrides) or a frame's mode. Phase 2's layout tools also write what it does not read: a container's display, gaps and padding and an item's insets and layout-item settings are in no field of `NodeSnapshot`, and its fill, stroke and effect lists are visible entries only, with no item id (which `bind_variable` on an item needs). `list_variables` returns every mode's cells (aliases unresolved and resolved); `list_styles` returns each style's slot, name, value and users' count. `list_components`, `list_variables` and `list_styles` also carry each main's or set's, variable's and style's description (`Node::description`, `Variable::description`, `Style::description`), read from the model; the snapshot carries none. That is a `SNAPSHOT_VERSION` bump when it lands.

#### Phase 4: building and using a design system

An agent builds a design system by filling a document through Phases 2 and 3 with mains, variables and styles, then `make_kit` and `publish_kit`; it uses one by listing the library's kits, reading a kit's published mains, variables and styles with `get_kit`, placing a main with `insert_from_kit`, and taking later publishes with `accept_kit_updates`. The tools are the app's verbs:

| Tool | Backing |
| --- | --- |
| `insert_from_kit` | `kit::insert`: vendors the main and every nested main it reaches into the vault; variables and styles carried by `tokens::plan` into the document's own tables |
| `accept_kit_updates` | The app's accept, per kit or all kits, never per main; transaction built by `kit::update_all` |
| `make_local` | `kit::make_local`, forking the kit variables and styles the main reads |
| `map_kit_modes` | The variables browser's mapping row (`panels/variables.rs`): a `SetKitModes` carrying the kit's held record with only its hand-set `map` changed |
| `swap_instance` | Takes a kit main too, vendoring it first (`kit::swap_offers`, `kit::vendored`) |

The raw kit ops (`OpenVault`, `CloseVault`, `SetOrigin`, a `SetKitModes` rewriting the kit's name or modes, `SetStyleOrigin`, whole-variable `SetVariable`) stay behind those verbs, where the planner keeps them consistent.

**Outside undo.** The kit mark is document metadata, not an operation (§5.11a), so `make_kit`, `publish_kit` and `stop_sharing_kit` save the file and `undo()` reverts none (`make_kit` mints the document id first when it has none). `publish_kit` is the only outward write: it changes what every consuming document is offered (an open one's update pill within the watch's 45 seconds or on regaining focus; a closed one's when next opened).

**No dialog (§15 D1097).** The agent publishes, stops sharing and files documents on request: the request is the person's ask, the publish note is the tool's argument, and a dialog would hang the agent on a window it cannot see. The person instead gets the status line saying the agent did it. The three kit actions already write a status line through `EditorSession::info` (for example *Published Atlas v4 · 3 components*), but none says who acted, and filing a document writes none (the dashboard's *New file* opens the document instead). Wording "by the agent" on the three, and a line for filing, is still to write (M6).

**One document per endpoint (§8.1).** `create_document` files a new document in the library (`library::store::file_document`) and returns its path. The agent reaches it through the proxy's pick-by-path once the GUI has opened it or `serve` hosts it; the endpoint never switches documents under the person using it. Renaming, moving, duplicating and trashing documents stay with the person on the dashboard.

**Half of Phase 4 is `ondin-app` code.** Not a linking wall (`serve` is the same `ondin` binary) but the windowed app holds it: kit actions are `OndinApp` methods, which windowless `serve` does not construct. The verbs are core (`kit::insert`, `update_all`, `make_local`, `publish`, `swap_offers`, `vendored`, `tokens::plan`); everything that finds and reads a kit is the app's: which documents are kits and their published states (the library scan's `meta.kit` and `KitShelf`, `kits/index.rs`), which updates wait (`kits_waiting` and the watch), the saves around marking and publishing, and filing a document (`library/`). Phase 4 is GUI-only until those move.

#### Exports

`set_exports` writes a layer's export list (the *Export* card's rows: format, scale, prefix and suffix, background, quality, trim, square padding; `ExportSpec`, see export.md §7). `run_exports` renders the listed layers' files through `ondin_export::plan` and `plan::render` (the path `ondin export --all` takes) and **returns bytes with their names** rather than writing them; where files land is the client's and the person's call. The panel's *Export all* uses the same two functions; the panel and CLI each keep a copy of the walk that collects layers carrying settings, held to one answer by `build::all_in_document_order`.

`export_code` writes layers as HTML/CSS, React or Vue through `code::Target::export` (export.md, *Code export*), the CLI's `--html`, `--react` and `--vue` path, and returns the files with the three report lists (`approximated`, `positioned`, `inlined`) beside them, never writing to disk. `ids` defaults to the selection, as *Export as HTML / React / Vue…* takes it. It is the door the deterministic translation exists for: the snapshot carries used values, so CSS an agent writes from it is absolute positions, where `export_code` keeps the specified layout, the variables and the components for the agent to build on.

#### set_pivot

`set_pivot` is for the person who comes after the agent. An agent can already rotate about any point with `set_transform` and an affine it computes, since the pivot is baked by the gesture that reads it and never composed into the world transform (§15 D55). The stored pivot decides where a person's later turn goes (canvas rotation, the inspector's rotation and skew fields, a flip; resize and skew drag ignore it, §15 D60): a needle about its hub, a door about its hinge.

#### Not exposed, on purpose

Guides, layout grids, a page's ground, the fill rule and the proportions lock (visual only, irrelevant to an agent; §15 D1095, D1096). Paragraph spans travel inside `set_paragraph_style`.

## Traps

- **The four phase paragraphs above are parsed by a test.** `ondin-mcp/src/tools.rs` holds the same names as `PHASE_1` to `PHASE_4`, and a `#[cfg(test)]` module `include_str!`s the architecture document and diffs every phase in order. The parser anchors on each phase's opening sentence, takes the paragraph up to the first blank line, and reads **every backticked token in it as a tool name**, dropping any signature at the `(`. Do not reword a phase's opening sentence, do not merge a phase paragraph into the next, and do not put a backticked aside inside one; put asides after the last phase. A rename fails at the comparison, a moved heading fails loudly at the parse. **When the architecture file is split, `tools.rs` must point its `include_str!` at this file (`docs/design/mcp.md`) or it will read the old path.**
- Phase 3 tools wrap verbs; writing the raw op (`RemoveStyle`, `SetCornerVars`) hands the commit a transaction it refuses or a non-canonical literal.
- Kit actions and document creation are not `Transaction`s: they bypass history, so `undo()` is not a safety net for them.
- The commit-door pass order (`commit_inner`) is not reachable from `serve`; a headless write would skip the `keep_*` passes.
