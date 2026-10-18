# IO: save format, schema versions, library block

The native `.ondin` file format (JSON, versioned DTO), its integrity checks, the migration ladder, and the library block that gives a document its identity.

## 5.11 IO

### Format and the two readers

- Native format is JSON, currently **v8** (`io::CURRENT_SCHEMA_VERSION`). `schema.rs` defines a versioned DTO decoupled from the in-memory types.
- `io::clip` writes a copied subtree for the system clipboard through the same `NodeDto` and `ImageDto` a file uses (`from_node`/`into_node`, `from_entry`/`into_entry` are lifted out of `DocumentDto` for this), so every normalization applies to a pasted layer too. Refuse a second projection beside `NodeDto`: a field added to one and not the other compiles, saves and loads.
- The clipboard deliberately does **not** share the migration ladder. `io::clip::read` compares `schema_version` for equality and refuses anything else: a file is an archive and must open in a build written years later; a clipboard is a handoff between two processes running now. Do not route `clip` through `migrate.rs`.
- `io::clip::read` carries envelope checks of its own (§15 D833–D836, D852):
  - The fence is a marker, not a landmark; the payload is the final line, so `write`'s single-line `serde_json::to_string` is load-bearing.
  - `parse` refuses: an empty subtree; other than exactly one root; a root that is not the first entry; a duplicate image id; an image entry *linked* to a file (linked images are a v1 non-goal, and a link would put an attacker-chosen URL into every SVG export as an `href`).
  - It drops an image entry no node keys into, and a non-finite *Paste here* box.
  - It does not validate the tree; that is `op_insert_subtree`'s job (§5.7).
- The envelope also carries `kits` (since v6): the vault mains a copied kit instance reaches, each a subtree with its origins, written by `write_carrying`, skipped when empty, held to the same envelope rules. Since v7 it carries `vars` (`variable::Carried`): the copied layers' variables by id, name and kind, the modes their frames set, and a detached twin of each layer that reads one, taken against the source. `variable::land` rebinds a paste from it, or falls back to the twin. Absent when the copy reads no variable.
- The library block is the first key after `schema_version` (§5.11a).

### save and load

- `save` emits nodes **sorted by NodeId** with stable field order, so re-saves are byte-identical (invariant 9). Guides are sorted by id too (the in-memory list is unordered, §5.5).
- `load` reads `schema_version`, runs the `migrate.rs` chain, builds the `Document`, then verifies tree integrity.
- The version is compared in the header's own denomination (`u64`) and never narrowed: casting to `u32` first let 2³³ truncate to 0 and open. `IoError::UnsupportedVersion` carries a `u64` so a reintroduced cast does not compile.

### Integrity verification

The in-memory walks (`subtree_ids`, `paint_path`, `resolve`) carry no visited set because `apply` cannot produce a cycle. The loader is where that guarantee is established, and it checks all of:

- root exists, is `Root`, has no parent;
- every non-root parent resolves; every listed child points back; no node is listed as a child twice;
- every node is reachable from the root, or from the vault (the one other parentless node a file may hold, §5.3e). `kit::check` holds the vault's shape and origins at the end, beside `component::check`;
- no node deeper than `io::MAX_TREE_DEPTH` (256);
- every parent/child pair obeys the same kind rules operations do;
- no frame under a boolean or a mask (§5.3, §15 D876), checked as a flag per stack entry on the reachability walk;
- no item id twice in one of a node's item lists (`Node::check_item_ids`). A repeated property id in `props` is **re-keyed, never refused** (`item::rekey_repeats`, lowest positional id the list lacks, deterministic) because an old development build wrote such files;
- variables and styles: `variable::check` and `style::check` run after the tree checks; a dangling binding is refused (it has no value to draw); a repeated style id is refused.

Reachability and the duplicate check are what make cycles and orphan components impossible; without them a hand-edited file could hang the app.

**Guides** are verified without a tree: malformed id, duplicate id and non-finite position are refused. A guide far off the artwork is legal. Ids come from the same reservation as nodes (§5.2). An `owner` that is dangling or not a frame is refused rather than promoted to global, since a scoped guide's position is in its owner's space. This also keeps drawing and snapping free of a fallback `check_guide_owner` will not let `apply` produce.

**There is deliberately no finiteness check in the loader**: `serde_json` writes non-finite `f64` as `null` and refuses `null`/overflowing literals at `from_value`, above the integrity walk. Protection is at the operation (`OpError::NonFinite`, invariant 8), which covers every number the model carries including paint.

The save format and the MCP snapshot (§7 snapshot) are distinct schemas; never conflate them.

### Bounds on hostile input

Each bound guards a different quantity; none implies another.

| Bound | Guards | Notes |
| --- | --- | --- |
| `io::MAX_TREE_DEPTH` 256 | Stack depth of recursive tree walks (an overflow aborts, taking unsaved work) | Rides the reachability walk. `op_insert_subtree` enforces it too, measured against the *destination parent's* depth (the bound is absolute; a subtree-relative check passes chains that compose past it). `CreateNode` in a loop is unbounded with no measured route. |
| `svg_in` depth 64 | Import recursion | Tighter than the loader, so an import never builds a document the loader refuses. |
| `svg_in::MAX_XML_DEPTH` 96 | The XML parser's own recursion (ran before 64 was asked) | Read off the text before parsing; past it `import` fails with `SvgError::TooDeep`. 65–96 still imports, reporting what lies past 64. The debug profile set the number. |
| `component::check` | Graph of mains | Cycle search uses an explicit stack, not recursion. |
| `MAX_SVG_NODES` 50,000 | `<use>` expansion (843 bytes could expand to 16,384 shapes) | Asked at `node_ignoring`, the choke point every emitted node passes; past it the import stops and reports. |
| `MAX_GRADIENT_STOPS` 256 | Payload per node (each shape naming a gradient copies the stop list) | Stops are **resampled**, not truncated or refused; reported once as `approximated`. |
| `MAX_CSS_RULES` 2,048 | `svg_in::Css::declaration` is linear in rules x elements x properties | Chosen with headroom over real exports, not measured against a corpus. Overflow is reported through the existing `"style (complex selector)"` line; rules read before the cap still apply. Re-ordering the cascade's tests was slower. |
| `svg_in::match_chain` memo | Selector backtracking (a 534-byte file did not finish in 120 s) | A table over `(chain index, ancestor index)`; ancestors are a path, so it is subsequence matching in O(chain x depth). A cap on selector compounds was rejected (`C(64,8)` is not a bound). |

`op_insert_subtree` validates reachability by a real walk (the same algorithm as the loader's check), not a node-count comparison: a two-node cycle balances `claimed.len() + 1 == nodes.len()` while being unreachable.

### Additive fields need no version bump

A purely additive field does not bump the version: `schema.rs` reuses the model's serde representation, so a new field is on disk at once. With `#[serde(default)]` whose default is what old files already meant, every existing file loads correctly and a migration step would be a no-op. Preferred form is `skip_serializing_if`, so an unused field never reaches the file and no existing document's bytes change (invariant 9).

Fields in that form, and the particular rule each adds:

| Field | Rule |
| --- | --- |
| `Stroke::align` | Missing means `Center`, how v1 files were drawn. |
| `Node::clip` | Named default says "yes", so it is re-checked against the kind on load (`dto.clip && dto.kind.clips_children()`). It is also normalized on the way **out** (`DocumentDto::from_document`), because `op_set_clip` accepts an inert flag on purpose. The loader's copy stays to guard hand-edited files. |
| `DocumentDto::canvas_background` | First top-level additive field; absence reproduces the default ground. |
| `DocumentDto::guides` | Absent means none. |
| `Node::proportions_locked` | Bare default; no normalization (inert on every kind). |
| `Node::pivot` | `skip_serializing_if = "Option::is_none"`; the loader must not re-decide which `Pivot` variant a kind gets (§5.3). |
| `Stroke::sides`, `Stroke::dash_fit` | Skipped when whole-outline / unfitted. |
| `Node::mask` | Skipped when false; normalized on load (`dto.mask && dto.kind.can_mask()`) to guard hand-edited files. |
| `Node::mask_mode` | `skip_serializing_if = "is_shape_mask"`; loaded **without** gating on `mask`, since the mode is remembered while the mask is off. |
| `GuideDto::owner` | Missing means whole-canvas. |
| `ImageRef::fit`, `crop`, `tile_scale`, `orient`, `adjust` | Three predicates for framing (parameters live beside the mode, so a cropped image set back to Fill still writes its rectangle); `adjust` has one predicate plus one per slider, so a neutral adjustment is no bytes. The chosen value going *in* must be tested as well as the default staying out. |
| `NodeDto::exports` | Skipped when empty, and every `ExportSpec` field but `format` skipped at its default (a plain 1x PNG writes `{"format":"Png"}`). A spec whose scale or quality cannot be valid is **dropped** on load (the operation refuses it, §5.7), since refusing the document over an export preset would lose the artwork to save the recipe. |
| `NodeDto::effects` | Skipped when empty. Its test fixture is two same-kind entries on an artboard, to catch dedupe-by-kind. |
| `Stroke::miter_limit` | Weaker kind: `#[serde(default = "default_miter_limit")]` with no skip, so every stroke writes it. Old documents re-save with a new key; invariant 9 (re-save of what was loaded) still holds. |
| Text: `spans`, `para_spans`, `paragraph`, `block`, `ParagraphStyle::marker`/`level`/`optical_margins` | Absent on uniform text. |

A bump **is** required when a field's meaning changes or its default would silently alter old documents. A bump is also taken for an *older build's* sake: a reader that does not know a version refuses it, where an older one would silently drop the new fields (v5, v6, v7, v8 below). Fields added while a version was unreleased joined it without a new bump.

**Text spans on load**: both span lists are clamped to the content (a stale range is a hand-edit or an old writer's rounding), and onto its **character boundaries**: `Spans::clamped` takes the content, not its length, and snaps outward. `op_set_text` and `op_set_text_style` use the same function. The pass also canonicalizes each span's value so a hand-edited `Em(0.020000000000000004)` does not fragment the stored list against a typed `Em(0.02)`.

### Migration ladder

| Step | Change | Notes |
| --- | --- | --- |
| v1 to v2 | Rect scalar `corner_radius` becomes per-corner `corner_radii` | The one number is spread over all four corners. A rename is not "additive": serde would drop the old key and square off every rounded rectangle. |
| v2 to v3 | Text attribute model (§5.4) | Mostly additive. Two shape changes: `line_height` bare multiplier becomes `Option<Length>` (a v2 number reads as `Em(m)`); `align` moves from the character defaults into the paragraph scope, **preserving the physical value** rather than mapping to `Start`/`End` (which would flip alignment for the first RTL document). |
| v3 to v4 | A frame's ground leaves `NodeKind::Artboard` for `Node::paint` | `{"Artboard": {"size", "background"}}` becomes `{"Artboard": {"size"}}` with the brush moved into `fills` as `{"brush", "visible": true}`; a `null` background contributes no fill. The brush is **prepended** (the list is z-order, bottom first; today the list is always empty, so prepending and appending are equal, but the strict rule protects future inputs). |
| v4 to v5 | Every item of the five item lists (`paint.fills`, `paint.strokes`, `effects`, `exports`, `grids`) gets an id `"0:<index>"` (`ItemId::positional`), keeping an `id` it already has | Deterministic (byte-identical re-save) and unique within its list, the only uniqueness an item id owes. A list that is not an array, or an item not an object, is left for the loader to refuse. `reserve_existing_ids` sweeps item ids. Component fields (§5.3d) and variant fields (`set`, `variant`, `props`, `swap`, a property's `filter`, `PropKind::Nested`) share v5 unbumped. |
| v5 to v6 | The vault (§5.3e) | `migrate_5_to_6` only changes the number. Keys are additive: `vault` after `root`, `NodeDto::origin`, `meta.kit`, and `published` last after `images`. A v5 reader would drop `vault` and refuse the file over the vault's parentless node. `published.document` is a whole document of the same version, and **the ladder does not descend into it**: a later migration that reshapes a document owes the nested one too. The loader refuses a published state carrying one of its own, so nesting is one level. |
| v6 to v7 | Variables and modes (§5.3f) | Number only, moving `published.document`'s with it. Keys: `DocumentDto::variables` (modes in order, variables by id, above `images`), `NodeDto::mode`, `NodeDto::corner_vars`, each skipped when unset. A literal brush saves as before; a bound one as `{"var": <id>, "alpha": a}` (`alpha` omitted at 1). A v6 reader would drop `variables` and then refuse a bound fill as a malformed brush. |
| v7 to v8 | Bindings, styles and kit tokens | Number only, published document moving with it. A v7 reader would silently lay bound gaps and padding out at 0. Keys, all skipped when unset or empty: `NodeDto::spacing_vars` (gap and padding); `visible_var` (a boolean cell saves as `{"boolean": true}`); `size_vars` (W/H); `text_vars` (a string cell saves as `{"string": "..."}`); `opacity_var`; `Stroke::width_var` and `Effect::vars`, saved **in the item** (the stroke or effect serializes as itself); `DocumentDto::styles` (above `images`, sorted by id) and `NodeDto::styles`, the loader refusing a repeated style id; a `"layout"` style value and `StyleRefs::layout`; `VariablesDto::kits` (each kit's modes and mapping by kit id); a variable's and a style's `origin` and `forked`. A layer whose list differs from its style's loads as written; the next commit running `style::settle` puts it back. |

## 5.11a The library block

- `core::meta::DocumentMeta` (`id`, `name`, `project`, `created`, and since v6 `kit`) is the only document state not about drawing. It is a top-level key written second, after `schema_version`.
- **`kit`** is the kit mark: `Option<KitMark { version, date, note, sharing }>`, present when the document is a kit. It lives in the block so "is this a kit" and "which version did it last publish" are answered from the scan's 4 KB prefix without loading the file. The frozen published state is `Document::published`, at the end of the file. It is written only by *Make this a kit*, *Stop sharing* and *Publish*, never by a save (the publish date is stamped by the user's publish). A note long enough to overflow the prefix makes the probe `Inconclusive` and the scan loads that file whole (slow, not wrong).
- **Why it is in the file**: the library's base folder can be a Drive/Dropbox/OneDrive directory (§9.6), and a fact stored anywhere else does not reach the user's second machine. The cache directory holds only derived data.
- A copy of the file is a copy of the identity: a sync client's conflict copy or an Explorer paste yields a second document with the same name, project and id, drawn as two identical cards. A conflict copy is therefore recognized from its *filename* (§9.5). *Duplicate* inside the app mints a new id.
- **Nothing in it may be written by a clock**, or byte-identical re-saves (invariant 9) end for every document. Fields change only when the user renames, moves between projects, or creates the document. Edit time is read off the directory entry.
- It is **not an `Operation`**, so outside undo: a rename is a dashboard action, often on a closed document, and routing it through `apply` would put an entry in the wrong history. `Document::set_meta` writes it directly and reports no `DirtySet`.
- Every field is `Option` + `skip_serializing_if`, and the whole block is skipped when empty, so a never-filed document is written as before.
- **`io::load` never mints a missing `id`** (loading must be deterministic). A pre-library file loads unfiled; the app gives it an identity the first time it files it.
- **An `id` is 32 lowercase hex characters, otherwise dropped.** `DocumentMeta::is_wellformed_id` is the shape `library::ids::mint` produces; `DocumentMeta::sanitized` drops anything else rather than refusing the document. This is a security check: the id is joined as a path component by the crash snapshot, version pin and cover cache, so `"../my-cool-design"` would rename attacker bytes over a real document, and an absolute id on Windows makes `Path::join` discard the base. Applied at both boundaries: `io::schema::into_document` (load) and `io::probe::meta_in_prefix` (the scan's reader, which feeds `library::cover::key` for every dashboard card without loading). The three app writers also ask `library::ids::is_minted` before joining.
- **`io::probe::meta_in_prefix`** reads the block from the first 4 KB and answers `Found`, `Absent` or `Inconclusive`. Hand-rolled, because serde needs a complete value and a prefix is incomplete. A zero-byte read from an unhydrated cloud placeholder must never be cached as "no name".
  - `library::scan::read_meta` maps `Found` to the block; `Absent` to a healthy pre-library document with default metadata and `Entry::unread = false` *without opening the file*; `Inconclusive` to a full read + `io::load`, the one path that can report a file unreadable.
  - `Absent` is a positive claim ("one of ours, no block"). Anything the prefix cannot definitely account for (not an object, a stray byte where a key belongs, a duplicate `meta`, a block that will not deserialize, a prefix that ran out) is `Inconclusive`. A wrong `Inconclusive` costs a slow scan; a wrong `Absent` asserts a renamed PNG is a healthy document.
  - A duplicate `meta` key is refused, not resolved: serde keeps the last, the probe would return on the first, and the probe's value feeds `Entry::meta`/`cover::key` while recovery and version history use the loaded document's. The walk defers its return to `root` to see a second key, the writer putting `root` immediately after `meta`.
  - `Absent` is the one place key order is trusted (`schema.rs` emits `meta` before `root`). A hand-reordered file costs the dashboard a filename instead of a typed name and heals on the next save; that is the only wrong answer allowed.

## Traps

- Routing `io::clip` through the migration ladder, or adding a second DTO projection beside `NodeDto`.
- Narrowing the header version before comparing it.
- A normalization applied on load but not on save (or vice versa) makes bytes move the first time a file is opened and saved.
- A depth bound is not a size bound is not a payload bound is not a CSS-work bound; extending the family does not cover the next quantity.
- `published.document` is nested and the migration ladder does not descend into it; each bump must move its number by hand.
- The migration for a changed field must write the new field (a migration that removes the key but never writes the replacement passes a kind assertion and fails only on the value; there is no `deny_unknown_fields`).
- `Absent` from the prefix probe trusts key order; `Inconclusive` is the safe default.
- Never write a clock value into the library block.
