# Invariants

The non-negotiable rules of the Ondin architecture (original §4, numbering preserved). If a requested change would violate one, stop and surface the conflict instead of working around it.

## 4. INVARIANTS

1. **Core is headless.** `ondin-core` has no GPU/windowing/UI dependency and is fully testable without a device (§3).

2. **Single mutation path.** The only way to change a `Document` is `Document::apply(Transaction)`. Node fields are private; there is no public `&mut` access. UI, MCP, importers and paste all go through operations.
   - Carve-out: the `meta` block. `Document::set_meta` replaces it whole, outside `apply` and outside undo (six production call sites). A rename is a library action taken from the dashboard, often while the document is not open; routing it through `apply` would put an entry in the wrong document's history and make `Ctrl+Z` in the editor undo what the file browser did. It reports no `DirtySet`. No node, edge or geometry is reachable this way (§5.11a, §15 D735).

3. **Identity is stable, never reused, and caller-minted.** `NodeId = (actor, seq)`: `actor` is a random per-session 64-bit id, `seq` a session-local monotonic counter. Ids are minted by the caller (`IdSource`) and carried inside operations; `apply` never allocates ids. Deleting a node never invalidates other ids; undo of a delete restores identical ids. This makes replay deterministic (§12).
   - List-item ids (fills, strokes, effects and other item lists) are minted the same way (`IdSource::mint_item`) but are unique only within one list and are copied verbatim when a node is duplicated, so a copy's fill matches its source's. Pre-v5 items get ids from the v4→v5 migration (§5.3d, D980).

4. **Document = truth; everything else derived.** `Document` is pure and serializable. `Resolved` (world transforms, bounds, R-tree, text layouts) and renderer scenes are derived: never serialized, always reconstructible, and no render representation feeds back into the model.
   - "Reconstructible" means from the document plus the registered fonts. Text is shaped by the thread-local parley engine in `core::text`, so `Resolved::rebuild(&doc)` on a fresh thread shapes against only the two bundled Inter faces and silently gives different bounds. All rebuilds today are on the UI thread; `library/writer.rs` hands its worker an owned `Document`, never a `Resolved`. (D735)
   - Stored derived values, deliberately:
     - `ImageEntry::width`/`height` come from the decoder and are serialized. Entries are content-hash-keyed with immutable bytes, and a linked image's bytes are not in the document, so the stored size is the only record. Do not "repair" this by re-decoding on load. (D788)
     - Components (§5.3d, D978): an instance's copies are stored nodes whose following fields the commit writes from their sources.
     - Kits (§5.3e, D1010, D1012): a vendored kit main is a stored copy of another document's main at a published version, rewritten only by the commit that accepts an update. A kit's own published state is not another member: it is the tree as it stood at publish, with nothing left to derive it from.
     - Kit-carried variables and styles (§5.3f, D1064): copies of the kit's at a published version, each with its origin, rewritten only by the accepting commit.
     - Styles (§5.3f, D1062; type via D1067, container layout via D1075): a styled layer's fills, strokes, effects, type or layout are a copy of its style's list, written by the commit (`style::settle`).
     - Variables add no member: a bound field stores its binding and resolves at read.

5. **Uncommitted state never enters the Document.** Interactive gestures (drag, resize, pen preview, text-in-progress) live in app-side preview state and reach the renderer as `RenderOverrides` (§6.2): per-node patches projected from the very transaction the gesture will commit. Exactly one `Transaction` is committed per completed gesture; history contains only committed transactions.

6. **Local transforms only.** Nodes store transforms local to their parent; world transforms live only in `Resolved`.

7. **Colour space discipline.** The document stores colours as sRGB with straight (non-premultiplied) alpha. Whatever representation a backend needs, the conversion lives only in `ondin-render` (`color.rs`), and a pin-time test asserts a known sRGB fill produces the expected pixel through each backend. Verify against the pinned Vello version rather than assuming.

8. **Operations are total and validated.** Every operation validates inputs and returns `Result<_, OpError>`; never panic on bad input (unknown id, cycle-creating reparent, duplicate id in `CreateNode`, a non-finite number in a geometry, transform, pivot, effect, guide position or paint, etc.).
   - **Finiteness is the operation's job, not the loader's.** `serde_json` cannot carry a non-finite `f64`, so the operation is the last place to refuse one while the user still has their work; past it the document saves and never reopens. (D421)
   - **Refusal is per transaction.** `apply` is atomic, so one node whose arithmetic overflows costs the whole gesture. Callers must refuse per node first; `tools::scaled_about` does this for resize (§9.4). (D481)
   - **Predicates, by family**, each asked at every door that writes the value (`SetX` and `op_insert_subtree`'s field loop, the paste door):
     - Coordinates, transforms, guides: `OpError::NonFinite` (the message says "number", not "coordinate").
     - Paint: `build::brush_is_finite`, asked by `op_set_fills` and `op_set_strokes`. It weighs a solid's channels, a gradient's geometry, every stop's offset and colour, the gradient transform, an image brush's sampler alpha, a stroke's `width` and every `dashes` entry. `build::image_ref_is_finite` weighs the image ref's crop rect, tile scale and seven adjustment sliders. `GradientBrush::opacity` must also pass `valid_opacity` (the same predicate a node's `opacity` uses), since a finite out-of-range value survives a round trip. (D451, D452, D767)
     - Pivot: `Pivot::is_finite`, on the enum because `Normalized` holds a `Vec2` and `Local` a `Point`. Asked by `SetPivot` and the insert-subtree loop. (D639)
     - Effects: `build::effect_is_finite` weighs a shadow's offset, blur, spread and colour, a layer blur's radius, and the four filter factors. Asked by `SetEffects` and the insert-subtree loop. This is the only family whose bad value does damage before save: an infinite blur radius makes `inf − inf = NaN` in `effect::escaped`, `f64::min` swallows it, and the cull/export plan then believe the blur reaches nowhere. Do not "fix" this by answering an infinite box there. (D641, D495)
     - Exports: `document::export_spec_is_usable` rejects unusable scales (`Width(0)`, `Times(-2.0)`). The loader's mirror is a drop, not a refusal (§5.11), because the two doors answer different questions. (D492)
   - **Finite is not usable.** `ImageAdjust::pipeline` additionally requires `-1.0..=1.0` (the range the panel clamps to), refused at resolution because an out-of-range finite value turned the colour matrix to NaN and painted black. A finiteness guard answers what `serde_json` will write, not what downstream arithmetic survives; ask the second question where the value is used. (D538)
   - **Out-of-spec file values are clamped at the doors, not refused**: a gradient stop alpha is clamped in `svg_in::stop_of` and `panels::paint::alpha_of`, since the model's choice is to read a foreign file rather than refuse it. `GradientBrush::opacity`, which the app itself writes, is refused at the operation. Which remedy applies depends on whether the value arrives from a gesture or from a document. (D692, D767)
   - **Validated operands do not make a usable derived value.** `build::affine_is_finite` accepts `Affine::new([1e308, 0, 0, 1e308, 0, 0])`, and `geometry::transform_rect` then overflows to infinite bounds. The answer is a second check where the value is derived: `geometry::measurable` (§5.9). A property test for this invariant must assert on derived bounds, not only accepted operands. (D495)
   - **An inverse is the same trap.** `affine_is_finite` accepts a singular matrix; `Affine::inverse` then writes NaN. Readers that invert must ask `build::affine_is_invertible` (`affine_is_finite(t) && affine_is_finite(t.inverse())`), threshold-free; it is the only spelling of that question in the workspace (`f64::EPSILON` is relative and wrong for a determinant in squared world units). The loader still accepts a singular transform and carries it as written (normalising to identity would make an invisible degenerate paint visible); geometry with no defensible rewrite is carried and guarded at the reader. (D713, D762)
   - **Post-condition checks**, run in `apply` after the last op and before the working copy is committed, because a transaction may pass through an invalid state:
     - Every guide's owner exists (`check_guide_owner`, also asked by `AddGuide` and `SetGuideScope`). Frame-then-guides ends valid, so a per-op check would wrongly refuse it. (D491)
     - No item id repeats within one of a node's five item lists (`OpError::DuplicateItemId`, over the nodes the transaction dirtied). (D980)
     - The component rules, `component::check`, refused as `OpError::BadLink`. This scans the whole document, not the dirty set, because a link breaks when its target goes and a delete dirties only the deleted node. (D978)

9. **Serialization is versioned and deterministic.** Every saved document carries `schema_version`; the loader migrates older versions to current. Saving the same document twice gives identical bytes (nodes in sorted-id order, stable field order). The SVG and JSON goldens rest on this (`testing.md`); the PNG golden is not built.

10. **MCP is a thin adapter.** Tool handlers only translate to and from `Operation`s, queries, and the owning process's document-level actions: marking a document a kit, publishing it, stopping its sharing, and filing a new document in the library (§8.3, D1097). Any logic they need lives in `ondin-core`. Renaming, moving, duplicating and deleting a document stay the person's, on the dashboard (§8.4). The four actions are never the handler's own: each is a request to the owning process, which runs the app's own verbs (the ones the document menu's rows and the dashboard's *New file* end in).
    - Pending move: `tools/mod.rs` and `preview.rs` hold thousands of lines of headless geometry (`resize_box_to`, `rotate_node`, `pen_outline`, `pen_anchors`, `insert_point`, `align_points`, `distribute_points`, `space_points`) in a crate with only a `[[bin]]` target, so nothing outside that binary can link them, and `ondin-mcp` sits below `ondin-app`. The first handler that needs one must move the pure half rather than duplicate it; doing it before is a compiler-enumerated sweep, after is reconciling two implementations. (D786)

11. **Renderers behind one shared walk.** Both backends implement `trait ScenePainter`, and the document→scene walk is written exactly once, in `scene.rs`. Adding or swapping a backend means writing a `ScenePainter` and must not touch core, export or app logic. (The originally designed `trait Renderer`/`RenderTarget` are gone; `ScenePainter` is the boundary, D2.)
    - The rule can be broken from below: a backend that substitutes a value the walk left open is deciding, and the walk never sees it. The GPU painter once answered `clip: None` with a ±1e7 rectangle in the node's local space, so an ancestor `scale(0.001)` turned an unclipped layer into a clipped one. Where a backend must supply a value, lift the choice into a free function and test it there. (D612)
    - The SVG writer is not a scene walk. `svg::emit_node` re-decides eleven things `scene::build` also decides: visibility, opacity, clip, mask and mask mode, effects, fill rule, paint order, the stroke-alignment clip, the missing-image placeholder and image framing. Agreement is held by convention plus `ondin-export/tests/two_walks.rs`, a differential over the ink set read node by node through per-node colours. The two legitimately differ on culling, geometry spelling and element ordering. (D781)

12. **The app's distribution identity never changes once shipped.** Installed copies find updates by these values; renaming one orphans every install with no route back to those users. (D956, D957)
    - Velopack `--packId Ondin` and channels `win-x64`, `linux-x64`, `osx-arm64`.
    - Reverse-DNS id `io.github.fadion.Ondin` (`chrome::APP_ID`: Wayland app id, X11 class, desktop entry, AppStream metadata, icons, macOS `--bundleId`).
    - Release feed `https://github.com/fadion/ondin` and opt-out `ONDIN_NO_UPDATE_CHECK` (`update.rs`).
    - Package repositories' base URL `https://fadion.github.io/ondin`, keyring path, `Origin: Ondin`, `Suite: stable`, `main`/`amd64`.
    - A new platform adds a channel; it never renames one.
    - Guards: `release.yml` refuses an unknown channel; a test pins the feed and opt-out; a `pages.yml` step compares repository values across their copies; `machine_dir::tests` reads every `--packId` from `release.yml` and demands `Ondin`. The app id has no check at all.
    - The pack id is also a folder: Velopack installs to `%LocalAppData%\<packId>`, which Setup renames aside and deletes and uninstall empties, so nothing the app keeps may live there. NTFS is case-insensitive, so the app's own data lives in `OndinData`. (D969)

## Traps

- Invariant 8 spreads over many doors: a new op or field that writes a number must ask the matching predicate at both its own op and `op_insert_subtree`'s field loop, or paste/import become the hole.
- Invariant 8's operand guards do not cover derived values (bounds, inverses); test derived output.
- Invariant 4's list of stored derived values grows with each linked-copy feature; every new member must be rewritten only by the commit pass that owns it.
- Invariant 11: the SVG writer re-decides eleven things; a change to any of them in `scene.rs` needs the same change in `svg::emit_node`, and `two_walks.rs` is what catches divergence.
- Invariant 12: check the app id by hand; no test compares it.
