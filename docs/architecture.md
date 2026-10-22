# Ondin — Architecture

Ondin is a native 2D design tool: a Rust workspace of `ondin-core` (the headless document
model), `ondin-render` (vello on GPU and CPU), `ondin-export` (SVG, PNG, HTML/CSS, JSON), `ondin-mcp`
(the agent adapter) and `ondin-app` (egui + wgpu). This file is the index. The design is in
`docs/design/`, one file per subsystem. **Open only the file you need.** Headings keep the
old section numbers, so a `§5.3d` cited from code is found by grepping `docs/design/` for
`5.3d`.

Key decisions are in `decisions.md`, non-goals in `non-goals.md`, open work in
`roadmap.md`.

## Invariants (`design/invariants.md`)

1. Core is headless. 2. Single mutation path: `Document::apply(Transaction)`.
3. Identity is stable, never reused, and caller-minted. 4. The document is truth; everything
else is derived. 5. Uncommitted state never enters the document. 6. Local transforms only.
7. Colour space discipline. 8. Operations are total and validated. 9. Serialization is
versioned and deterministic. 10. MCP is a thin adapter. 11. Renderers sit behind one
shared walk. 12. The distribution identity never changes once shipped.

Read the file for the exact wording before relying on one.

## Design files

| File | § | Covers |
| --- | --- | --- |
| `foundations.md` | 1–3, 10, 12–14 | Scope, stack, workspace and dependency rules, milestones, multiplayer readiness, Command Mode seams, deferred decisions |
| `invariants.md` | 4 | The twelve invariants |
| `testing.md` | 11 | Testing strategy, goldens |
| **Core** | | |
| `core-model.md` | 5–5.3, 5.5 | Module layout, identity, the node model, the document and guides |
| `core-ops.md` | 5.6–5.10 | Transform and resize semantics, operations, builders (`build.rs`), layer names, history, `Resolved`, queries |
| `effects.md` | 5.3a, 6.4 | Layer effects: model and render passes |
| `layout.md` | 5.3b, 5.3c | Layout grids; container layout (flexbox, grid, insets) |
| `components.md` | 5.3d | Components, instances, overrides, variants, swap, reset |
| `kits.md` | 5.3e | Kits: shared component libraries |
| `styles-variables.md` | 5.3f | Variables, modes, bindings, styles |
| `text.md` | 5.4, 5.4a | The text node and fonts |
| `images.md` | 5.5a | Images |
| `io.md` | 5.11, 5.11a | Save format, schema migration, clipboard envelope, the library block |
| **Render, export, MCP** | | |
| `render.md` | 6.1–6.3 | The render boundary, `RenderOverrides`, backends |
| `export.md` | 7 | SVG, PNG, code export (HTML/CSS), JSON snapshot, the CLI, export doors |
| `mcp.md` | 8 | MCP topology, headless mode, concurrency, the tool list (a test parses §8.4) |
| **App** | | |
| `app-chrome.md` | 9.1, 9.2 | UI chrome and panel layout |
| `app-dataflow.md` | 9.3 | The one-way valve: gestures, previews, the commit door |
| `tools.md` | 9.4 | The v1 tools and canvas interaction |
| `app-selection.md` | 9.4 | Selection on the canvas and in the layers panel |
| `app-create-gestures.md` | 9.4 | Creation gestures |
| `app-pen-and-nodes.md` | 9.4 | Pen and node tools |
| `app-booleans.md` | 9.4 | Booleans, flatten, outline, masks |
| `app-frames-and-copies.md` | 9.4 | Frames on the canvas, reparenting, copies |
| `app-snapping.md` | 9.4 | Snapping |
| `app-rulers-guides.md` | 9.4 | Rulers and guides |
| `app-autopan.md` | 9.4 | Auto-pan |
| `app-canvas-chrome.md` | 9.4 | Canvas chrome, top-bar menus, present mode |
| `app-image-editing.md` | 9.4 | Image fills and image editing |
| `inspector.md` | 9.4 | The inspector cards |
| `type-panel.md` | 9.4 | The Type panel and typography popup |
| `font-picker.md` | 9.4 | The font picker |
| `library.md` | 9.5 | The library: dashboard, projects, versions, trash |

Also in `docs/`: `shortcuts.md` (the keymap), `context-menus.md` (the menus), `vm.md` (the
Command Mode language, unbuilt).

## Keeping this true

Each file states the design as it is now. When a change alters a rule, rewrite the passage
that states it, in place — no amendment notes, no history. When a file outgrows ~40 KB,
split it and add the row here.
