# 8. ondin-mcp

The MCP endpoint: the running Ondin process hosts it, agents connect through a stdio proxy configured once, and every write goes through the same op queue and commit door as the app's own edits.

## 8. ondin-mcp

### 8.1 Topology: one configured server, any document

Documents live in a running Ondin process, the window or a headless `serve`, so that process hosts the MCP endpoint and agents connect to it. A person configures one server in their harness, `ondin mcp-proxy` with no arguments, and it reaches any document.

- **Hosts.** The window (one per machine, holding its documents as tabs, `one_window.rs`) and `ondin serve` each listen on a local endpoint and serve any number of documents, addressed by absolute path. Each writes a discovery file naming its endpoint, `mcp/<pid>.json` in the window lock's folder (`one_window::window_dir`: the machine folder on Windows, the user's runtime folder elsewhere), and removes it on exit; the proxy removes one whose endpoint does not answer.
- **The proxy** (`ondin mcp-proxy`) bridges an agent that spawns its server over stdio. It connects to the window's endpoint when a window runs, else to a running `serve`, else starts `ondin serve` and connects to that. It forwards lines and remembers one thing: the connection's current document, so that when its host goes away or a window starts it reconnects to the window and restores it.
- **The current document** is per connection. `use_document(document)` sets it and every other tool acts on it until the next `use_document`; a tool called before any fails naming `use_document`. The document is an absolute path, or a name or relative path matched against the host's open documents, then the library's by file name or document name; the proxy first turns a relative path that exists from the harness's working folder, with or without `.ondin`, into an absolute one. Several matches fail with the candidates, for the agent to ask. The tool's description tells the agent to switch only when the person names another file.
- **Opening.** In the window, a document not yet open opens as a tab in the background: the window is not raised and the tab in front does not change, and the agent's edits show when the person looks at it. An agent's edit to a tab behind is written to its file at once, since a tab behind is kept clean (`tabs.rs`); one to the tab in front waits for the save or the autosave, as the person's own edits do. In `serve` it loads into memory.
- **Security.** Same user, same machine, no token: a same-user process can already read and write the files the endpoint guards. On Windows the pipe rejects remote clients, its default DACL lets only the current user write to it, and the host drops a client whose process runs as anyone else (`one_window::same_user`). Elsewhere the socket sits in a 0700 folder, is 0600, and the host checks the peer's uid. TCP or remote transport is parked.

### 8.1a Wire

JSON-RPC 2.0, one message per line, on std threads with `serde_json`; no SDK and no async runtime. The methods are `initialize` (one pinned protocol version, negotiated), `ping`, `tools/list`, `tools/call` and the `initialized` and `cancelled` notifications. Each tool's input schema is generated from its verb's `Param`s, with `verbs::example` showing a model parameter's shape. No resources, prompts or server-sent notifications: see *Revisions* below.

### 8.2 Headless mode

`ondin serve` hosts documents with no window: core, history, CPU renderer and the MCP endpoint. Same op queue, same tools, so an agent can build, edit and export on a machine with no display, or edit a file nobody has open. This follows from the headless-core invariant and is a first-class v1 deliverable.

- `serve` writes each commit to its file: there is no person to save later, and a window with the same file open as a clean tab follows it (`library.md`, *the file is watched*).
- `serve` exits when a window takes the window lock, and after its last connection closes; the proxy moves its connection to the window.

### 8.3 Concurrency model

- The owning process holds each document as an `EditorSession` on its main thread.
- Every tool call funnels through one single-writer **queue** (`ondin_mcp::queue`): a connection's thread sends the call's name and arguments with a reply channel and waits. The call travels rather than a built transaction because a verb reads the document to answer its edit, and the document is the owning thread's.
- The owning thread runs the handler (`ondin-app`'s `mcp::Engine`) against the document's `EditorSession`: a read projects it, a write runs the registry verb (`verbs::call`) and commits through `EditorSession::try_commit`. `serve` drains the queue in its loop; the window drains it every frame. One source of truth and one history; an agent's edits appear live on the canvas through the same path as the person's. An MCP edit's undo step is named by the tool it called and remembers the client (`EditorSession::name_mcp_step`, `Origin::Mcp`), so the History section can mark it.
- **Document-level actions** (invariant 10, §15 D1097): `make_kit`, `publish_kit(note)`, `stop_sharing_kit`, `create_document(name, project?)` ride the **same queue**, so they are ordered against surrounding edits (a publish sent after a batch of writes publishes those writes). The owning thread runs the app's own code: `make_kit`, `publish_now`, `stop_sharing_kit` (the verbs the document menu's rows end in, past their dialogs) and `library::store::file_document`, and replies with the result. None is a `Transaction`, none enters history, `undo` reverts none.
- No connection thread ever touches a document (invariant 10).

### 8.4 Tools

Phase 1 — read/export (ship first):
`use_document(document)`, `get_document() -> Snapshot`, `get_node(id)`, `list_children(id)`, `find_nodes(query)`, `hit_test(x,y)`,
`get_bounds(id)`, `get_selection() -> Vec<NodeId>`, `export_svg(artboard?)`, `export_png(viewport)`,
`run_exports(ids?) -> Vec<File>`, `export_code(target, ids?) -> CodeExport`, `list_components()`, `list_variables() -> VariableTable`,
`list_styles() -> StyleTable`.

Phase 2 — write (1:1 with operations/composites):
`create_node`, `create_text(parent, text, x?, y?, width?)`, `insert_subtree`, `delete_node`,
`reparent`, `reorder`, `group(ids)`, `ungroup(id)`, `duplicate(id)`, `join_paths(id, other)`,
`repeat(ids, count, angle?, turn?)`, `set_transform`, `set_pivot`, `set_geometry`,
`set_size(id, width, height)`, `pixel_fit(ids)`, `set_name`, `set_visible`,
`set_locked`, `set_opacity`, `set_blend(ids, mode)`, `set_clip`, `set_mask(id, mode?)`, `set_text`, `set_text_style`,
`set_text_spans`, `set_paragraph_style`, `set_block_style`, `style_text(id, attrs, start?, end?)`,
`style_paragraphs(id, attrs, start?, end?)`, `set_fills`, `set_strokes`,
`set_effects`, `add_image(bytes) -> ImageId`, `replace_image(id, image, index?)`, `fix_contrast(ids, level?, colour?, variable?)`, `clean_up(ids?, hidden_paint?, covered_fills?, hidden_layers?, empty_layers?, one_layer_groups?, guides?, skip_locked?)`, `fill_content(ids, locale?, kind?, records?, avatars?, seed?, example_emails?)`, `set_display`, `set_layout_item`, `set_insets`,
`insert_chart(parent, kind, table?, mapping?, x?, y?, width?, height?)`, `set_chart(id, kind?, mapping?, options?)`, `set_chart_data(id, table)`, `flatten_chart(ids)`,
`set_exports`, `add_page(name?, index?)`, `duplicate_page(page)`, `delete_page(page)`,
`move_page(page, index)`, `set_page_draft(page, draft)`, `undo()`, `redo()`, `set_selection(ids)`.

Phase 3 — components, variables and styles (through the verbs, never the raw ops):
`make_component(id)`, `create_instance(main, parent, at)`, `detach_instance(id)`,
`swap_instance(id, main)`, `combine_as_variants(ids)`, `switch_variant(id, property, value)`,
`define_property(main, field)`, `set_instance_property(id, property, value)`,
`reset_overrides(id, scope)`, `add_variable(kind, name, value)`, `remove_variable(id)`,
`rename_variable(id, name)`, `set_variable_value(id, mode, value | alias)`, `add_mode(name, collection?)`,
`remove_mode(id)`, `rename_mode(id, name)`, `set_frame_mode(id, mode?, collection?)` (one pick, the frame's
other collections' modes kept),
`bind_variable(id, field, variable?)`, `add_style(slot, name, value)`, `remove_style(id)`,
`rename_style(id, name)`, `set_style_value(id, value)`, `apply_style(id, slot, style?)`.

Phase 4 — kits and the library (the app's own actions, some outside undo):
`list_documents()`, `create_document(name, project?) -> Path`, `list_kits()`, `get_kit(kit)`,
`insert_from_kit(kit, main, parent, at)`, `list_kit_updates()`, `accept_kit_updates(kit?)`,
`make_local(ids)`, `map_kit_modes(kit, mapping)`, `make_kit()`, `publish_kit(note)`,
`stop_sharing_kit()`.

Selection is app state, not document state (§9.3), but deliberately part of the MCP surface, because "the selected layer" is how humans phrase most AI requests. The MCP layer reads and writes it in the owning process's session state, never the `Document`.

Which of this is built: the wire, the proxy, `serve`, the window as a host (`mcp::window`), and every Phase 1 to 3 tool; `tools/list` offers those (`ondin_mcp::schema`). Phase 4 is not built.

#### Conventions

- `style_text` and `style_paragraphs` are how an agent styles part of a text: a range counted in
  characters becomes runs over it, as the text editor writes a selection; the whole text becomes the
  layer's defaults with that attribute's runs dropped, as the type panel writes it.
- A page is named by its root node's id: `create_node`'s and `insert_subtree`'s parent may be one,
  and `set_name` renames one.
- `create_node`, `insert_subtree`, `duplicate`, `add_page` and `duplicate_page` return the new ids (the handler minted them); so do `create_instance`, `insert_from_kit`, `add_image`, `add_variable`, `add_mode` and `add_style`.
- The snapshot exposes world transforms so agents can compute local placements; a `place_at_world(id, world_transform)` adapter is allowed in core if agents struggle.
- `set_blend(ids, mode)` takes the mode as the snapshot names it (`"Multiply"`, `"Pass through"`) and writes through `build::set_blend_all`, as the inspector and `Shift`+digits do: the outermost members only, and Pass through landing as Normal on a layer that is not a frame or group.
- A chart is edited through its four verbs, which write `SetChart`; its parts regenerate after the commit (`chart::settle`), and any other tool writing a part is refused by `chart::guard` (*Part of a chart — edit the chart, or flatten it first*). `insert_chart` makes a 480 × 280 frame on the kind's sample table, or on the table given with its mapping read off the column types (`chart::Mapping::guess`) unless one is given; `set_chart`'s `mapping` and `options` are JSON merge patches over the chart's own; `set_chart_data` keeps the mapping and series colours of each column whose name survives (`Chart::with_table`) and drops the linked file; `flatten_chart` is `chart::flatten_ops` for each chart (*Flatten*, `charts.md`). The snapshot's `chart` names the column ids they take.
- `insert_subtree` is the tool a whole layout goes through: a page of a few hundred layers is a handful of calls, one tree per section, where `create_node` is a round trip per layer.
- `export_png` is the agent's eyes: placement errors no read tool reports (a card under the band below it, a label over a button) show only in the render. A client that can see images is expected to export, look and correct. Its longest side is capped by default, and an argument raises the cap.
- `find_nodes(query)` is how an agent reaches "the home header" without reading the whole document: layers whose name or text contains the query, each with its id, name, kind and the names of its ancestors. A request about one part of a page is a find and a subtree read, not a snapshot.

#### Revisions

The person and the agent edit one live document, so an agent's earlier read goes stale without its knowing. Each document has a revision that every commit advances, the agent's or the person's, undo and redo included.

- Every tool result carries `rev`. The host remembers, per connection, the revision it last showed; when the document moved since, the result also carries `changed`: the layers added, changed and removed since then, by id and name, read from the history's commits.
- `get_document`, `get_node` and `list_children` take `since: rev` and answer `unchanged`, or only the layers changed and removed since. Nothing is withheld unless the agent passes `since`: a harness may have summarised its earlier read away.
- A write takes `base_rev`. If a layer it writes changed after that revision the write is refused and the refusal lists them; without `base_rev` it applies.

Server-sent notifications would be the protocol's own channel for this, but harnesses do not put them in front of the model mid-turn; a tool result always reaches it.

#### The commit door

Every write goes through the commit door the app's edits use (`EditorSession`), so the same passes run for an agent's transaction: `keep_insets` and `keep_flex_sizes`, the eight `keep_*_bindings` passes, then `propagate::owed`, which runs `style::settle`, `swap::settle`, `propagate_structure`, `component::settle_links`, `variant::settle`, `propagate`, `style::settle` once more, `keep_size_bindings` again and the inert-spacing clear, in that order (pinned by core tests). An agent never has to know a copy exists to keep it in step with its main.

That sequence lives in `EditorSession::commit_inner`, in `ondin-app`, which has no lib target (§15 D786's wall), so `ondin-mcp` cannot call it. It never needs to: the handlers hand transactions to the owning thread, and `serve` is the same `ondin` binary as the window, so both own an `EditorSession` per document and commit through it. The door also reads an edit of a styled layer as an edit of the style while the style browser's *Edit* is open (`style::edit_through`, first in `commit_inner`, §15 D1063). An agent's transaction skips that step: the same call does the same thing whatever is open on screen, and an agent changes a style with `set_style_value`.

#### Phase 3: verbs, not ops

Phase 3 names verbs because the raw ops are refused or wrong alone:

- Binding a field writes a canonical literal beside the binding (`build::bind_*_ops`: 0 for a number, empty for a string, `true` for a visibility), so a main and a copy bound to one variable compare equal whatever either held. A colour binding replaces the paint's brush outright, keeping its alpha percentage (`BrushSource::Bound`, `build::bind_brush_at_all`).
- Removing a variable or style detaches its users to what they drew in the same transaction (`variable::remove_ops`, `style::remove_ops`); removing a mode returns its frames to *Inherit* for its collection (`variable::remove_mode_ops`). Each is refused at the commit otherwise (§15 D1038). A bare `RemoveStyle` or a bare `SetCornerVars` over a non-canonical literal would be refused or wrong.
- Renaming a mode keeps every kit column it reads (`variable::rename_mode_ops`); a bare `SetModeName` re-maps a kit by the new name.
- Making an instance writes a linked copy (§15 D978).
- `bind_variable`'s `field` names any bindable field: corner radii, gaps and padding, width and height, a layout item's min/max width and height, visibility, opacity, a text's content, family, size, weight, line height, letter spacing and paragraph spacing, a fill's or stroke's colour, a stroke's width, and a shadow's X, Y, blur and spread or a layer or background blur's radius. An effect's colour and a filter's numbers bind nothing. It is one tool over the six `Set*Vars` ops plus the bindings a fill, stroke, effect and layout item carry in their own items (`BrushSource`, `LayoutItem::limit_vars`); an absent `variable` detaches to the value drawn. An item's binding names its item: the core verbs take a stroke's or effect's `ItemId`, and a paint's colour its index in the list.

#### Reads must grow with Phase 3

A bound field's stored literal is a placeholder, so the snapshot reports the **drawn** value of every bound field. It does that for a paint's colour, a stroke's width, an effect's numbers, size and corners (through `Resolved`'s used kind), visibility, opacity, a text's content and family, and a layout's gaps, padding, flow and limits. Beside the drawn values each node names what it reads, in the terms the Phase 3 tools take: `bindings` lists each bound field as `bind_variable`'s `field` with its variable, `styles` the style per slot by `apply_style`'s slot names, `modes` the modes a frame sets, and `component` its part (main, set, instance with the main it shows, or copy with the layer it is linked to) and a variant's values. The fill, stroke and effect lists stay visible entries only, so each stroke and effect carries its item id and a stroke its index in the whole list, and `fill_indices` maps fills past a hidden one. Still open: an instance's overrides and the property values it is set to. Layout reads back as written: `display`, `item` and `insets` carry the shapes `set_display`, `set_layout_item` and `set_insets` take, so an agent edits one by changing the field it read; the item is the authored one, not the width a frame with width queries is laid at. `list_variables` returns every mode's cells (aliases unresolved and resolved); `list_styles` returns each style's slot, name, value and users' count. `list_components`, `list_variables` and `list_styles` also carry each main's or set's, variable's and style's description (`Node::description`, `Variable::description`, `Style::description`), read from the model, and `list_components` each main's or set's documentation link (`Node::doc_link`); the snapshot carries neither.

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

**Documents by path (§8.1).** `create_document` files a new document in the library (`library::store::file_document`) and returns its path, which the agent passes to `use_document`; opening it never changes the tab in front of the person. Renaming, moving, duplicating and trashing documents stay with the person on the dashboard.

**Half of Phase 4 is `ondin-app` code.** Not a linking wall (`serve` is the same `ondin` binary) but the windowed app holds it: kit actions are `OndinApp` methods, which windowless `serve` does not construct. The verbs are core (`kit::insert`, `update_all`, `make_local`, `publish`, `swap_offers`, `vendored`, `tokens::plan`); everything that finds and reads a kit is the app's: which documents are kits and their published states (the library scan's `meta.kit` and `KitShelf`, `kits/index.rs`), which updates wait (`kits_waiting` and the watch), the saves around marking and publishing, and filing a document (`library/`). Phase 4 is GUI-only until those move.

#### Exports

`set_exports` writes a layer's export list (the *Export* card's rows: format, scale, prefix and suffix, background, quality, trim, square padding; `ExportSpec`, see export.md §7). `run_exports` renders the listed layers' files through `ondin_export::plan` and `plan::render` (the path `ondin export --all` takes) and **returns bytes with their names** rather than writing them, each file an embedded resource (text for SVG, base64 otherwise); where files land is the client's and the person's call. With no `ids` it takes the layers *Export all* takes (`panels::exportable_layers_of`); the CLI keeps its own copy of that walk, held to the same answer by `build::all_in_document_order`.

`export_code` writes layers as HTML/CSS, React or Vue through `code::Target::export` (export.md, *Code export*), the CLI's `--html`, `--react` and `--vue` path, and returns the files with the three report lists (`approximated`, `positioned`, `inlined`) beside them, never writing to disk. `ids` defaults to the selection, as *Export as HTML / React / Vue…* takes it, and with nothing selected (always, in `serve`) to the first page's layers, as the CLI takes them. It is the door the deterministic translation exists for: the snapshot carries used values, so CSS an agent writes from it is absolute positions, where `export_code` keeps the specified layout, the variables and the components for the agent to build on.

#### set_pivot

`set_pivot` is for the person who comes after the agent. An agent can already rotate about any point with `set_transform` and an affine it computes, since the pivot is baked by the gesture that reads it and never composed into the world transform (§15 D55). The stored pivot decides where a person's later turn goes (canvas rotation, the inspector's rotation and skew fields, a flip; resize and skew drag ignore it, §15 D60): a needle about its hub, a door about its hinge.

#### Not exposed, on purpose

Guides, layout grids, a page's ground, the fill rule and the proportions lock (visual only, irrelevant to an agent; §15 D1095, D1096). Paragraph spans travel inside `style_paragraphs`.

## Traps

- **The four phase paragraphs above are parsed by a test.** `ondin-mcp/src/tools.rs` holds the same names as `PHASE_1` to `PHASE_4`, and a `#[cfg(test)]` module `include_str!`s the architecture document and diffs every phase in order. The parser anchors on each phase's opening sentence, takes the paragraph up to the first blank line, and reads **every backticked token in it as a tool name**, dropping any signature at the `(`. Do not reword a phase's opening sentence, do not merge a phase paragraph into the next, and do not put a backticked aside inside one; put asides after the last phase. A rename fails at the comparison, a moved heading fails loudly at the parse. **When the architecture file is split, `tools.rs` must point its `include_str!` at this file (`docs/design/mcp.md`) or it will read the old path.**
- Phase 3 tools wrap verbs; writing the raw op (`RemoveStyle`, `SetCornerVars`) hands the commit a transaction it refuses or a non-canonical literal.
- Kit actions and document creation are not `Transaction`s: they bypass history, so `undo()` is not a safety net for them.
- A write applied to the `Document` anywhere but `EditorSession::commit` skips the `keep_*` passes and the propagation, and every gate stays green.
