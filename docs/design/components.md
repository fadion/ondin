# Components and overrides

Main components, instances as linked copies, overrides found by comparison, structural and field
propagation, resets, variants and component properties, instance swap, nested properties, and the
chrome that shows them. Code: `ondin-core/src/{component,propagate,reset,variant,swap,item}.rs`,
`ondin-app`'s `panels/{component,variants}.rs`, `canvas.rs`, `layers`.

## 5.3d Components and overrides

### Linked copies, not derived ones

An instance is a **linked copy**: its subtree is stored as ordinary `Node`s in `Document.nodes`, each
with its own minted id and a link to the node it was copied from. A commit-time pass writes a main's
changes into its copies as operations in the same transaction (Penpot's model; Figma and Sketch
derive instance contents from main + override record).

Why: every reader is keyed by a stored `NodeId` — `Resolved`'s maps, `container::lay_out`, selection,
hit-testing, `scene::build`, the SVG writer, the snapshot, the layers panel, `RenderOverrides`. A
derived child has no stored id; a linked copy is an ordinary node, so every tool, the inspector,
export and layout work on it unchanged.

This reinterprets §15 D868: what instances share with layout is the **commit-time write-back** (the
shape of `build::keep_insets` / `keep_flex_sizes` in `EditorSession::commit_inner`), not `Resolved`'s
derivation. Layout stays derived.

Costs accepted: a copy's following fields are derived data stored in the document, bending §12's "no
derived state in the document" (single-editor and op replay converge; concurrent collaboration would
need a resync on merge); file size grows with instances; one main edit fans out into ops on every
copy. Revisit if real-time collaboration is built.

### The model

- A main component is a **flag on an `Artboard` or a `Group`** — `Node::component`, read by
  `component()` — not a new `NodeKind` (a new kind would fall through the `matches!` predicates and
  every layout/paint applicability check; "paint and clip stay frame-only" keeps holding).
- Every node has `Node::link: Option<NodeId>` (`link()`), the node it was copied from. `NodeDto`
  saves `component` skipped when false, `link` as a wire string skipped when none.
- Ops `SetComponent` and `SetLink`, inline in `apply_one`, each inverting to the old value;
  `changes_ink == false`, no-ops in `RenderOverrides::absorb`.
- An **instance root** is a linked node whose chain of links reaches a main (`component::is_instance_root`);
  a member's nearest instance root is the one whose source contains the member's source.
- `remap_subtree` copies `component` verbatim and remaps a link pointing inside the template, keeping
  one pointing outside it: a pasted main+instance links the pasted instance to the pasted main, and a
  duplicated whole instance stays an instance of its main.
- A link is a cross-node reference (D405 declined one for text-on-path). The precedent is a guide's
  `owner`: a post-condition after the last op of `apply`, the deleting verb carrying the cascade, and
  a loader that refuses a dangling reference rather than repairing it.

**A link points one level up** — to the node copied from, never the chain's far end. Propagation is
transitive, in topological order (inner main → nested copy in outer main → outer main's instances).

**One rule for every way a link is cut** (detach, a main's deletion, content kept when the main loses
a child): a node whose source is itself linked relinks to its source's source; only a node whose
source is unlinked loses its link. This keeps a nested instance an instance of its own main
(`component::relink_past(doc, gone, among)`).

### Overrides are found by comparing values

Nothing records which properties a copy overrides. When a main node changes a field from `old` to
`new`, each node linked to it takes `new` **iff its current value equals `old`**; otherwise the
difference *is* the override.

- Granularity is the field. Struct payloads of independently editable fields (`TextStyle`,
  `ParagraphStyle`, `BlockStyle`, `LayoutItem`, `Display`, `Insets`, `GeometryPatch`'s parts) compare
  per field. Keyed lists compare item by item, by id. Coupled payloads compare as one unit: a text's
  `content` with its `CharSpans` and `ParaSpans`.
- Consequences: drift cannot exist; *reset* copies the source's value back; the inspector marks a
  field overridden by comparing it with its source. Accepted catch: setting a copy's value equal to
  the main's makes it follow again.
- A bound field compares its **binding**, not its resolved value (§5.3f). A card's style likewise: a
  copy on the main's style is no override; another style or detached is, and the reset writes the
  main's binding beside its list.
- **An instance root's transform is never linked** — it is where the instance sits. Precisely: the
  transform of a node whose source is a component root, so a nested copy inside an outer main still
  passes its place within that main to the outer instances. Every other root field, size included,
  follows by the compare rule.
- **User operations win over propagation in the same transaction**: a counterpart the transaction
  itself targets for a field is not overwritten by that field's propagation.
- **Undo and redo never re-propagate**: history applies through `Document::apply_restoring`, and the
  step already holds the propagated ops or their inverses.

### Field propagation — `propagate::propagate`

Runs last of the component passes in `commit_inner`. For each of the transaction's edits to a node
that has copies — the last edit per `shape_key`, against the value before the transaction — it walks
copies at every depth through the reverse link map, each level compared against its own old value.

- A copy the transaction deletes (under a `DeleteNode`) or names in a `SetLink` takes nothing
  (`propagate::leaves_its_source`).
- No reader per field: an op's inverse carries the old value, so `Document::peek` applies an op to a
  scratch document holding **only the node it names** (plus its subtree for `SetMask`, which refuses a
  mask over a group holding a frame, `holds_a_frame`) and keeps the inverse. This stands on every such
  op writing its node alone.
- `propagate::mode`, wildcard-free, picks the comparison:
  - **whole** — transform, geometry, text with spans, the span ops, name, visibility, opacity, pivot,
    clip, mask, mask mode, fill rule, display mode (`SetMode`), the variable bindings (`SetCornerVars`,
    `SetSpacingVars`, `SetVisibleVar`, `SetSizeVars`, `SetOpacityVar`) and the card
    styles (`SetFillStyle`, `SetStrokeStyle`, `SetEffectsStyle`, `SetTypeStyle`, `SetLayoutStyle`);
  - **fields** — text/paragraph/block style, insets, display, layout item: a JSON merge
    (`merge_present`) over the **union** of keys, absence a value of its own, a changed enum variant
    followed whole so two variants never mix. `SetTextVars` and an item's `limit_vars` follow **slot
    by slot**, as the literals beside them follow field by field: compared whole, a copy binding
    another slot kept none of the main's new bindings and drew their placeholder literal 0;
  - **items** — the keyed lists;
  - **skip** — locks, links, swaps, variant/property ops, everything structural.
- **An instance root's placement is its own**: `SetTransform`, `SetInsets`, `SetLayoutItem`,
  `SetVisible`, `SetVisibleVar` (`propagate::is_placement`) do not follow onto a root linked straight
  to a main (`linked_to_main`). A main hidden on a components page hides no instance. A nested
  instance moved inside its outer main does move in the outer instances.
- Spans follow only onto a copy with the same content, and no content write (`SetText`, span writes)
  follows onto a copy whose content binding differs from its source's.
- **A copy's own binding holds its literal**: a size or corner follow keeps the copy's literal on
  each axis or corner it binds otherwise than its source (`propagate::keep_own_bound_geometry`), and
  is dropped when nothing else is left to follow.

### Item ids — `item.rs`

Five lists have ids: `Paint::fills`, `Paint::strokes`, `Node::effects`, `Node::exports`,
`Node::grids` (plus component properties `Node::props`, below). Compared whole, not by item: a grid
template's track list, a gradient's stops, a `Path`'s points, `corner_radii`, text spans.

- Shape: `Keyed<T> { id: ItemId, #[serde(flatten)] value: T }`, derefs to `T`, saves flattened
  (`{"id":"0:0","brush":…}`).
- **Not an `id` inside `Fill`/`Stroke`/`Effect`**: they derive `PartialEq`, and an id inside would make
  identical-looking fills unequal, silently changing *Mixed*, `Transaction::changes_nothing`,
  `Operation::overwrites`, run merging and export/import dedupe. The wrapper separates *same item*
  (`id`) from *same look* (`value`).
- A whole-list `==` still compares ids, so every *Mixed* reading and the export plan's dedupe compare
  through `item::same_values` (`build::paint_shown`, `shared_effects`, `shared_grids_keyed`,
  `shared_exports_keyed`, `ondin-export/src/plan.rs`).
- `ItemId` is a newtype over `NodeId`, minted by `IdSource::mint_item` from the global stream — an
  item added locally to an instance and one added later to its main must never collide.
- Unique **within its list** only: `Node::check_item_ids` (all six lists), run by `Document::apply`
  over dirty nodes (`OpError::DuplicateItemId`) and by the loader's `schema::verify_integrity`.
  Duplicating a node copies item ids verbatim — the match between a copy's item and its source's.
- No `From<Vec<T>>`, deliberately. A keyed list is built by keeping the ids it read (`Keyed::map`),
  minting for an added item, or `keyed_by_position` for a list built whole (new node, import,
  migration, test) — never for an item appended to an existing list.

**Multi-selection writes keep every layer's own ids**, in three shapes:

1. An edit of a shared list is retargeted through the anchor (`item::retarget`;
   `build::retarget_fills_all`/`retarget_strokes_all`; inspector `write_fill_list`, `write_strokes`,
   `write_effects`, `retarget_grids`; export `retarget_exports`): an edited item takes the target's id
   at the anchor's position; a user-added row **keeps its minted id on every target**, re-minted only
   where the target already holds that id.
2. A wholesale replacement (*Paste properties*, mixed-row clear, export preset/paste, mixed grid
   override) keeps each target's ids **by position** (`item::rekey_by_position`;
   `set_fills_all`/`set_strokes_all`); positions past a target's end take one id per write, shared by
   every target (`item::PastTheEnd`). Not for an edit — deleting the first of two would hand the second
   the deleted id.
3. One row changed in place (`slot_transaction`'s Selection arm, `build::edit_fill_at_all` /
   `edit_stroke_at_all`) moves no id.

The appending `+` of grids and exports (`inspector::append_grid`, `export::append_exports`) mints its
row once for every subject, so a row added to a main and its instance together stays linked.

**The per-item rule is the structural rule one level down**: items match by id; within a matched item
fields compare as above; an item the main gains is inserted after the counterpart of its preceding
item; an order change follows only where the shared items are still in the main's old order; an item
the main loses goes from a copy only if it still equals its source.

### Instance structure is free

An instance may add local (unlinked) children, delete linked ones, reorder, and reparent within
itself. Whatever still matches the main follows it; whatever differs belongs to the instance. A
*counterpart* is the node in an instance linked to a given main node.

- **Main gains a child** (`CreateNode`, `InsertSubtree`, or `Reparent` into the main): in each instance
  still holding the parent's counterpart, a copy (fresh ids, each linked to its source) is inserted
  after the counterpart of its preceding main sibling; failing that, before the next one's; failing
  that, topmost. The anchor is read among the children of the parent's counterpart wherever it now
  sits; a moved or deleted sibling counterpart is no anchor. No parent counterpart, no insert. A layer
  dragged in from outside the main is a gain, copied whole.
- **Main reorders**: an instance follows only if its linked siblings present in both are still in the
  main's old order; local additions keep their anchor.
- **Main reparents within itself**: a copy follows only if still under the old parent's counterpart; a
  missing new-parent counterpart makes it a removal there.
- **Main loses a child** (`DeleteNode`, or reparent out): the counterpart is deleted **only if it and
  its whole subtree still equal their sources and it holds no local additions**; otherwise it is kept
  and unlinked (by the one cut rule). **A main edit never destroys instance-side work.**
- **Instance-side edits**: a linked node reparented out of its instance loses its link (membership is
  judged at the nearest instance root, so moving into a nested instance also cuts it); a linked node
  duplicated or pasted inside its own instance becomes local — at most one node per instance links to
  a given source.

**`propagate::propagate_structure`** implements it from the transaction's final tree against a reverse
link map:

- A gained child is copied in at its anchor, down the copy chain, each node linked one level up,
  against a per-parent child list simulated as inserts land.
- A lost child takes a counterpart only where `untouched` — every field but locks equal, same linked
  children in the same order recursively, no local additions; a deleted counterpart is a lost child
  for its own copies.
- Reorders go down the copy chain; the pass lays every linked child of each copy in the main's new
  order in the linked slots.
- No arm acts on a copy the edit itself deleted. No gained child is copied into a copy the edit already
  gave a node linked to it (`supplied`) — a kit update writes a nested main's layer and the nested
  copy's layer in one transaction. No arm, nor the field pass, writes a copy in the kit vault
  (`propagate::follows_nothing`, §5.3e).
- Ops go out **inserts → moves → deletes → reorders**, delete decisions after the moves, `untouched`
  ignoring children the pass carries out.
- A parent the edit deleted counts as changed, so children lifted out of it are moves (this makes
  *Ungroup* inside a main work). But its children are lost only where it was itself lost **from a
  parent that stays** — a fixed point over changed parents. A deleted **main** is lost from nothing, so
  its instances keep their layers.
- **A move into a parent the same edit made** (*Group selection*) is judged on the final tree: the new
  parent's copy is pruned of every node already in the same main (`only_new`) and each existing
  counterpart **moves** into it, so an instance's changes survive grouping and ungrouping. The
  instance holding a copy is asked of the copy's **parent** (a nested copy is an instance root itself).
- **Ungroup (or a boolean's *Release*) over a copy the instance changed** keeps that copy as a local
  wrapper with its children still linked. A kept child has every field the edit wrote on its source
  pinned at its own value, so the main's baked transform does not follow. `settle_links` cuts the
  wrapper's link; `component::check` admits a local group holding linked members. At every depth: each
  move carries `goes` (inherited by moves below) and `origin` (the main layer whose written fields a
  kept child pins).

### Reset — `reset.rs`

Reset a field; reset structure (re-insert missing counterparts at their anchors, local layers left
alone); reset all (fields, missing counterparts, the main's order).

- A copy is compared with its source **one link up** (`reset::source_of`).
- `reset::state_ops` writes a node's whole state as the ops that would set it, destructuring `Node`
  with no wildcard — a new field stops the build until its author decides whether a copy can override
  it. An override is a field whose op differs between copy and source; its reset is the source's op
  aimed at the copy (`Operation::retargeted`) — the same currency as `propagate`.
- Locks, `component`, `link` are not fields; a root linked straight to a main never overrides its
  placement; a copy of a different kind variant compares only common fields.
- Counts (`Override::units`): a plain value is one; a struct payload counts each differing leaf (via
  JSON); a keyed list counts each source item changed or removed, one for an order difference, and one
  per item of the copy's own.
- **A list's reset is the source's list exactly** (`reset_items`): items, order, values, the copy's own
  items removed — via a card header's chip, *Reset fields*, *Reset all* and a ghost row's restore.
  `reset::reset_item` resets one overridden item in place (a row's ↺). `item_states`/`removed_items`
  feed the trailing slot and ghost rows.
- **Scope** (`scope_nodes`): nodes at or under the scope whose link lies inside the source subtree of
  its instance root — a nested copy's members included, a local instance of another main excluded. A
  swapped copy's members are in scope only under that copy; a linked layer out of scope takes its
  subtree with it; a local instance of the very main a slot is swapped to stays local.
- **`reset::Drift`** counts: field units; **removed** children (a source child with no counterpart
  anywhere in the copy's **outermost** instance — `reset::outermost_root` — so a moved one is not
  missing, and a missing subtree counts once); parents whose linked children are out of source order;
  outermost **local** layers (no reset touches them). `outermost_root` climbs only out of nested
  copies: it stops at a root linked straight to a main and at a swapped root.
- *Restore removed children* (`restore_children`) copies the missing child in with fresh ids, linked
  one level up, at `propagate::anchor`, pruned by `propagate::only_new` of nodes whose counterpart the
  instance holds elsewhere. *Reset order* re-slots linked children into source order, local layers
  staying in their slots. *Reset all* = restore, order, fields, each computed on the document the
  previous leaves, one transaction (its first two steps are `reset::restore_structure`).
- *Restore children* (row-scoped) follows the row's fold: expanded, `reset::restore_own_children` —
  one parent's own removed children (`restore_missing`) then its linked children's order, exactly what
  `child_drift` counts; collapsed, `restore_structure` over the subtree, what `subtree_child_drift`
  counts. Neither touches a field.
- `reset::reset` takes the outermost of its scopes (`build::outermost`), so overlapping scopes restore
  a child once. Every reset is an ordinary commit, so resetting a nested copy inside a main reaches
  that main's instances.
- Not done: moving back a counterpart the instance reparented elsewhere inside itself. A layer once a
  counterpart and since cut is local with no record of its past, so a restore puts a fresh copy beside
  it.
- A reset states placement fully: every placing write carries the axis's insets (the node's own where
  its placement is its own, else its **slot source**'s, `reset::slot_source_of`), or `keep_insets`
  re-pins it against the copy's frame; an instance root writes its own insets back. Every reset states
  the layout item of each layer it resizes (`OndinApp::items_stated`), or `keep_flex_sizes` reads it as
  the hand's resize.

### The verbs

- **Delete** (`relink_for_delete`, from `OndinApp::delete_selection`): deleting a main **detaches its
  instances** in the same transaction — each becomes a plain copy keeping its look; undo restores main
  and links; nested chains relink one level up. Toast *Deleted "Button" — 4 instances detached ·
  Ctrl+Z to undo*.
- **Cut is a move**: *Cut* then *Paste* of a main in the same document restores it under its own ids
  and relinks the instances the cut detached. The cut detaches as delete does and keeps the links it
  took (`OndinApp::cut_move`, a `CutMove`); the first paste of that clipboard into that document, none
  of its ids present, inserts the subtrees under their original ids and gives back each link whose
  node still holds exactly what the delete left (`paste_cut_move`, `TakenLink`). A second paste is an
  instance; a refused move falls back to an ordinary paste; an undo past the cut **parks** the move
  (`document_rewound`) — a redo back to the cut with no commit between unparks it, a commit while
  parked ends it.
- **Detach** (`component::detach`) cuts the root and its own members, letting a nested instance inside
  climb past this main to its own.
- **Create component** `Ctrl+Alt+K`, in place or wrapping the selection in a frame (toast only when
  wrapping). On an instance root: *"An instance can't become a component — detach it first
  (Ctrl+Alt+B)."* **Detach instance** `Ctrl+Alt+B`; also *Duplicate as component*, *Select all
  instances*, *Go to main component*, *Reset all*, *Reset <name>* on a linked child.

**A copy of a main is an instance**: copy/paste, duplicate and Alt-drag of a main make an instance;
*Duplicate as component* makes a new main. A copy of a whole instance is another instance of the same
main. **Names are copied verbatim** (§5.7b's duplicate numbering does not apply, or every renumbered
name would start as an override). `component::settle_copy`, run by `build::insert_subtrees_as` on each
`remap_subtree` copy (`MainCopy::Instance` / `MainCopy::NewMain`):

- a copy of a main in this document becomes an instance, each node linked to its original;
- a link pointing inside the copy stays; a link to a node outside the document is dropped; an instance
  root's link stays; a member's link stays only if its instance root came along, otherwise it becomes
  local;
- every nested instance in the copy with no instance root above it climbs one level up (relinks past
  the nested copy to the nested main) and lands as a plain instance of that main;
- a swapped copy whose link is dropped lands at once by `swap::landed`.

**`component::settle_links`** — so no door that moves, deletes or regroups layers has to know about
components. It applies the transaction to a scratch copy through `Document::apply_unchecked` and
settles the resulting tree:

- links into deleted nodes climb past them (an ungrouped main is a deleted main), including nested
  copies in its instances; a cut nested copy whose outer instance lost its link climbs as `detach`
  does, carrying a swap it passes (`swap::climb`, `land_ops_through`);
- top-down, a linked node not linked straight to a main, with no instance root above it whose source
  contains its own, is **cut** (a layer dragged out of its instance; members of a dissolved group
  instance);
- where two nodes of one instance share a source, the one the transaction did not touch keeps it;
- iterates to a fixed point; free without a structural op or without a link.

`build::ungroup`, *Flatten*, *Outline* and text-on-a-new-path are settled by this pass;
`delete_selection` still relinks for itself because its toast counts the detaches.

**Components are document-local** for their own mains; pasting into another document drops every link
whose source is not there. The exception is kit instances (§5.3e): a copy of a kit instance carries
the vendored mains it reaches and the paste vendors them.

### Post-conditions — `component::check`

Run after the last op of `apply`, before `*self = working` (a transaction may pass through a breaking
state). Refused as `OpError::BadLink(NodeId, LinkRule)`:

| Rule | Refuses |
| --- | --- |
| `Variant(VariantRule)` | the six variant/property rules, run first by `variant::check` |
| `Dangling` | a link to a missing node or to itself (also a dangling swap) |
| `ComponentKind` | `component` on anything but a frame or group |
| `ComponentLinked` | a node both main and linked |
| `NestedMain` | a main beneath a main or a linked node |
| `Membership` | a linked node (except one linked straight to a main) with no instance root above it, or with its source not strictly inside its nearest instance root's source (a swapped root's source being its swap; a layer the slot adds admitted via `component::slot_adds`) |
| `SharedSource` | two nodes of one instance root linked to one source (a node linked straight to a main exempt; a nested copy's root is held to it) |
| `LinkCycle` | a chain that never reaches a main |
| `ComponentCycle` | a main containing an instance whose chain ends at itself — DFS over main → mains-shown-by-instances-inside (`component::uses`), with an explicit stack and on-path set (`component::reaches_itself`), never recursion |
| `Swap` | a swap on anything but a nested copy inside an outer instance, or naming anything but a main of the copy's own kind |

- Runs over the **whole document**, not `dirty`: a link breaks when its target goes, and a delete
  dirties the deleted node, not the instance pointing at it. Free (one scan) when nothing is a main or
  linked. The item-id check runs at the same place over `dirty`.
- The loader runs `component::check` at the end of `schema::verify_integrity` and refuses a failing
  file. It never compares values — there is nothing to drift.

**The doors ask ahead, so nothing offers what the commit refuses**:

- `component::why_not_main` → `NotMain::{Kind, Already, Inside, Instance, Holds, SetKind}`;
  `can_be_main` is it with no reason. `why_not_wrap` answers for *Create component*'s wrap, where the
  members' parent must not be or sit in a main or instance.
- `component::can_hold(doc, parent, moved)` — the half `build::can_parent` cannot see: false for a main
  or set into/under a main or instance, a set under a set, a main into a set of another kind
  (`variant::fits_set`), an instance whose shown main is or reaches the destination's main. Always true
  for `moved`'s own parent. Membership is not asked — `settle_links` cuts.

### Save format

Item ids and the component fields arrived with schema v5 (`CURRENT_SCHEMA_VERSION` is now 8).
`migrate_4_to_5` gives each existing item `"id": "0:<index>"` (`ItemId::positional`), keeping an id an
item already has — deterministic, so re-saves are byte-identical. `component`/`link` are additive but
bumped anyway: the loader has no `deny_unknown_fields`, so an older build would silently drop every
link and save instances back as detached copies, whereas an unknown version is refused
(`IoError::UnsupportedVersion`). The clipboard needs nothing: `io::clip::read` refuses every version
but its own.

### Finding counterparts, cost

The source → copies map is a scan of the document per commit and per preview frame. A maintained index
is a later question for a measurement (as a `Document` field it owes D301's clone question; as a
`Resolved` map it is a decision). `propagate::propagate` returns early when no op touches a node
anything is copied from.

### Preview

`EditorSession::preview_follows` runs `propagate::propagate` over the transaction `set_preview`
composes (live text session's ops, the gesture's, then `build::flex_holds`) **as the commit door
rewrites it** — `keep_flex_sizes(keep_insets(tx))` — so a pinned layer's move reaches copies as insets
and an in-flow item's move as nothing. `set_session_preview` runs it over a text session's own
transaction. Gated on `propagate::touches_copied`.

- **The follows travel beside the gesture**: `RenderOverrides::from_transaction_following` absorbs
  them after the gesture's ops but hands only the gesture's to `flex_relayout` and `relayout`, so each
  copy is laid out by its own container.
- **Only the field pass**: structure and `settle_links` remove/move nodes, which overrides cannot
  express; a main's gained or lost child reaches its instances on release.
- The pivot preview reads the gesture's ops, never the follows.
- A pinned copy given a transform and no insets has its own insets restated by `preview_follows`
  (`relayout` re-places only a layer with an insets or kind override).

### Snapshot

No `snapshot_version` bump: a copy reads as an ordinary node. The snapshot (`ondin-export`) exposes
neither `component` nor `link` yet; both are decided for MCP (§8.4), and whether a snapshot fill
carries its item id is open (unverified). A change of the snapshot's shape owes its own bump.

### The commit door and pass order

The passes live in `ondin-core`; `EditorSession`'s commit is the only door that writes through them
(MCP's write tools and Command Mode commit through it too, invariant 10). **A door that edits a
document holding components and bypasses it is a defect**: a field edit to a main reaching
`Document::apply` without the pass leaves every copy at the old value, which the compare rule then
reads, permanently, as an override. Nothing gates this.

Order is one function, `propagate::owed`, called by `commit_inner` and the core test fixtures:

1. `style::settle` (a style edit written onto its users; a styled slot the edit wrote detaches), then
   `build::keep_size_bindings` again if it wrote a `SetDisplay`
2. `swap::settle`
3. `propagate_structure` (mints ids)
4. `settle_links`
5. `variant::settle`
6. `propagate`
7. `style::settle` once more (a follow or copy that reached a styled slot is put back to its style)
8. `keep_size_bindings` again, always: a copy holding its own size binding that a follow makes
   computed (growth, a stretch) detaches as the main's did
9. `inert_spacing`: a copy holding its own gap/padding binding whose layout a follow takes away has
   the binding cleared

All after `keep_insets` and `keep_flex_sizes` (incl. `kept_flow_translations`), so what those append
to a main propagates and a follow carries the door's value.

### Exact equality — open in general

The comparison is exact, so any pass that *recomputes* a copy's stored value could turn a following
field into an override by an ulp. Measured and holding: a pinned child after the instance is resized
(`keep_insets` never rewrites a resized frame's children); a held flex resize in the preview; tool
writes on a main's in-flow flex item through the commit (holds because `propagate` runs last); *Reset
all* of a pinned child (`keep_insets` leaves a layer whose insets the transaction sets). Unmeasured:
any other pass that recomputes a copy's value.

### Not built / open

- Relinking a top-level instance to a kit main not yet vendored (the *Main* row offers the
  document's mains, vendored kit mains among them).
- Pushing an instance's changes to its main, and a verb that un-makes a component (it would owe a
  detach) — `roadmap.md` *Later*.
- Unruled, parked in `roadmap.md`: a main-scoped guide not copied into instances; whether a group
  *instance* gets a canvas label.
- Real-time collaboration — non-goal for now (it is where linked copies cost).

### Chrome

**Component-ness is shape, never the selection accent**: a filled hexagon for a main, an outline
hexagon for an instance. The filled hexagon is drawn (`ui::paint_hexagon_filled`) because the bundled
font is Phosphor Regular only; used on canvas, layers, the Component card's faces and the *Create
component* / *Duplicate as component* menu rows.

Canvas:

- A main's tag is a chip, `color::COMPONENT` fill (no edge), filled hexagon and name in
  `COMPONENT_INK`, *N instances* in `COMPONENT_INK_DIM` while selected. Always shown. A group that is a
  main gets a canvas label as a frame does (`OndinApp::tagged`, walked by `canvas::draw_frame_labels`).
- An instance's tag: outline hexagon and name in `COMPONENT`, its main's name trailing in
  `COMPONENT_DIM` when they differ, *Name › Child* while entered with a layer inside selected — drawn
  **only while the instance is selected or entered**.
- Neither label ever takes `color::SELECT`; nor does a set's tab. A main's or instance root's
  selection/hover outlines are `COMPONENT` (`canvas::chrome_hue`), and so are handles and the size
  badge when every selected layer is one (`selection_hue`); a set and layers inside a component keep
  `SELECT`. Nothing off the canvas takes the hue.
- No tag inside an instance nor on an instance inside a main (`canvas::tag_withheld`), a main excepted.
  A plain frame inside a main keeps its tag (its one handle besides its edge, D22).
- **An instance picks like a group**, whatever its kind: `query::group_chain` stops at an instance
  root, `canvas::pick_leaf` does not skip an occupied frame that is or sits in an instance — for a
  frame instance an exception to D22. `Enter` enters it ahead of its text/picture/path arms
  (`enter_action`), and so does a double-click on its own background (`double_click_pick`).
- `canvas::draw_component_chrome`: a `FAINT` hairline round each instance of a selected main; round the
  entered instance (`canvas::entered_instance`, nearest instance root at/above `entered_group`) a
  dashed `DIM` boundary and *Editing inside instance · Esc to exit*, cleared once the selection leaves
  by any door (`OndinApp::leave_a_scope_the_selection_left`).

Layers panel: neutral icons wearing the hexagon. Trailing mark: **+** for a local layer (unlinked
inside an instance, or a local instance); a **dot** for an override on the row; on a collapsed row
inside an instance, or a collapsed instance root, a dot for any drift (`reset::Drift::any` or a local
layer, from `drift_cache`), bubbling no further than the root. A removed or reordered child's dot goes
on **its parent's row**, with a tooltip (`reset::child_drift`); the context menu then offers
***Restore children***, scoped by the row's fold (`OndinApp::row_folded`: no search open; the canvas
door reads the same fold). A collapsed row lit by fields alone offers *Reset all* only. A kept child
shows as local (nothing records provenance). A deleted child layer gets **no** ghost row.

Inspector:

- **Component card**, directly under the identity card (`panels/component.rs`). Faces: a main
  (button *Duplicate*), an instance (the main's name as the link), several instances (*N instances
  of*), instances among other layers, a variant (*Variant in*). A child's line (*In Card instance* /
  *Local to this instance*), the mixed selection's line and the binding line are rows of the identity
  card (`inspector_component` takes a `ComponentPart`: `Identity` / `Cards`).
- **Description**: a main's and a variant's Component card and a set's *Variants* card show a
  multiline field under the name (hint *Add a description*; buffered in `OndinApp::desc_edit`,
  committed trimmed on defocus). A single instance's card shows its main's description dim under the
  main's name — the variant's own, else its set's (`component::description_note`); it is never an
  override. A row of the swap picker hovers its main's description (`SwapTarget::description`).
- A face's instance count is on its name's line and is the *Select all*
  (`component::name_line(ui, glyph, name, instances)`; at zero plain `text::DIM` and dead).
- Resets: *Reset all*, and a ⋯ overflow with *Reset fields N*, *Restore removed children N*, *Reset
  order*, *Reset properties* — **disabled, not hidden** at zero; counts shown only above zero. No reset
  deletes a local *layer*; list resets do remove the instance's own items.
- The drift summary counts fields, removed children and order, and properties apart (`PropDrift`);
  several instances read *K with changes*. Counts come from `reset::Drift` cached on
  `EditorSession::revision` (`OndinApp::drift_cache`, also read by the menu; property readings in
  `OndinApp::prop_cache`). Resets commit through `commit_edit` (`OndinApp::reset_tx`).
- **Override look**: a 4px neutral dot and full-brightness label, swapped for ↺ on hover with the main's
  value in a tooltip (*Reset to main*). Card headers carry the dot and a unit count, and on an open
  card a hover chip (*↺ Reset fill*), from `reset::overrides` grouped by `component::card_of`.
  Sub-fields (an inset, a gap, a grow, a text size) are marked by `OndinApp::sub_mark` reading the
  source value from `OndinApp::field_overrides`, so a field `overrides` skips is never marked.
- Marked cards: Transform (compared on **stored local** values and the axis's insets, never the world
  readings; laid-out W/H via `layout::size_field`), Appearance, Mask, Position (centre buttons
  compared on the auto margins), Container (`display` cells by mode, `ByMode`), Item, Type (on the
  node's own style, never a run's; the Type popup one mark per section via `section_marked`,
  *Optical size*/*Axes*/face as disjoint parts of `variations` reset in the main's order by
  `axes_reset`; *Sizing* on mode alone), the multi-selection Appearance card (opacity and radius,
  each joining its binding, `multi_appearance_marks`). A dropdown's ↺ is the first row of its
  open list; a segmented control's mark sits on its lit cell; a switch row's dot has no ↺.
- Not marked, by decision: the multi-selection Transform card, each grid track's field, the identity
  card.
- Lists (Fill, Stroke, Effects, Export, Layout grid): a trailing slot (empty, dot, +); an item the
  instance deleted is a dashed **ghost row** laid out as the live row with an icon-only ↺, restoring
  the main's whole list. Item resets are written verbatim, never through a retargeting writer.
- *Mixed* keeps D130's drawing (`ui::segment_mixed`); Mixed is about the value, an override about the
  label.

Menus: *Create component*, *Detach instance*, *Duplicate as component*, *Select all instances*, *Go to
main component* in Structure after *Frame selection* (`context-menus.md` §4); *Reset all* on an
instance and *Reset <name>* on a linked child, **omitted** when `reset::Drift::any` is false
(`context-menus.md` §3's exception). Toasts are text-only `EditorSession::info` lines naming `Ctrl+Z`.

### Variants and component properties — `variant.rs`

Rulings: deleting a variant detaches its instances (deleting a value deletes the variants holding it,
so theirs detach too); `Ctrl+D` on a variant makes an **instance** (a plain layer in the set) — *Add
variant* is the verb that takes the next free combination; an instance's name is copied verbatim.

**A component set is a frame around mains, never a main; a variant is a main whose parent is a set.**

- `Node::set: Option<VariantSet>` — the set's variant properties, each `VariantProp { name, values }`.
- `Node::variant: Vec<String>` — a variant's values, one per property in the set's order.
- `Node::props: Vec<Keyed<Property>>` — component properties of a main or set:
  `Property { name, kind: PropKind::{Boolean, Text, Swap, Nested}, bound: Vec<NodeId>, filter: String, preferred: Vec<NodeId> }`.
- `Node::description: String` (`description()`) — what a main or set is for, in the user's words; empty
  is none. Written by `SetDescription { of: Described::Node(id), text }`, the one op a style's and a
  variable's description share (§5.3f). An instance reads its main's through the link, so a copy never
  holds one; a duplicated main keeps its own (`remap_subtree` copies it).
- Ops `SetVariantSet`, `SetVariant`, `SetProperties`, `SetDescription`, each inverting to the old value;
  no ink, no-ops in `absorb`, skipped by `propagate::mode`, never written by `reset::state_ops` —
  **never followed or overridden**. `description` and `Property::preferred` are new in v9.
- `Node::make_copy_of` is the one place a node becomes a linked copy (linked; swap, flag, values,
  properties and description cleared); every door that makes an instance node goes through it.

**`variant::check`** (as `LinkRule::Variant`):

| Rule | Holds |
| --- | --- |
| `SetKind` | a set only on an `Artboard`, never a main or linked, never inside a main, instance or set |
| `SetNames` | set property names/values well formed |
| `Values` | a main in a set fits it; values on anything else refused |
| `PropertyOwner` | properties only on a main not in a set, or a set |
| `PropertyName` | unique, non-empty, not a variant property's name (the nameless showing exempt; a second showing refused) |
| `Binding` | a bound layer exists strictly inside the owner's main (for a set, inside a variant, never a variant's root); text for a text property; a nested instance for a swap or showing; bound once per field |

**`variant::settle`** — after `settle_links` and **before `propagate`** (load-bearing: a renamed value
renames its variants here, and `propagate` carries the new name onto every instance still holding the
old; swapped, instances keep the old name). It fits each variant's values to its set (kept where they
fit, a property's first value where not, first free combination when the count is wrong), drops values
off non-variants, **derives each variant's name from its values joined by `", "`**, moves a variant's
properties to its set (merged by name and kind; an id clash re-keyed from the commit's `IdSource`),
and prunes stale bindings. Gated so an edit that cannot break a variant rule pays no scratch clone
(the gate includes reparents, written links or swaps, and pasted subtrees carrying a set, values or
properties). Over a property with no values it leaves the refusal to `apply` (`SetNames`).

**Verbs**: `combine` (*Combine as variants* — mains framed with `SET_PAD` (10) of air on every side,
nothing moving on the page, properties read from the names as below); `add_variant` (duplicates a
variant as a main `VARIANT_GAP` (24) below, grows the set to `SET_PAD` past it, first free
combination, set bindings extended); the set's property/value renames, adds, moves, deletes
(`delete_value` deletes the variants holding it through `relink_for_delete`).

**`combine` reads properties from the mains' names** (`named_values`). When every name, after its last
`/`, is comma-separated `Key=Value` pairs (keys non-empty and unique within the name, values non-empty,
no second `=`), the set takes those keys as properties, in the order the names first mention them, each
value list in first-mention order; a main whose name leaves a key out takes `Default`
(`variant::MISSING_VALUE`), appended to that property's values. The set is named the path before the
last `/` when every main shares one, else `Component`. If any name is not all pairs, the set gets one
property, *Property 1*, each whole name a value, and is named by the names' common prefix
(`set_name`). Either way `settle` then names each variant by its values.

**A set holds one kind**: `variant::switch` refuses a main of another kind (`variant::same_kind`); the
doors refuse first with a message — *Combine as variants* over two kinds (`variant::one_kind`, row
dimmed with `menu::COMBINE_MIXED`, `combine` refusing `WrongKindForOp`), a canvas drag into a set
(`component::can_hold` → `variant::fits_set`, `canvas::SET_OF_ANOTHER_KIND`), the layers panel drop
(`drop_is_legal`; `OndinApp::set_refusal` gives `SET_OF_ANOTHER_KIND` or `SET_OF_MIXED_KINDS`).
`variant::takes_together` (newcomer mains all one kind, fitting the set) is asked by both doors, since
an empty set fits either kind. The loader never refuses a mixed set.

**The variant switch is made in place** (`variant::switch`), so matched ids, the selection and a nested
copy's own copies survive:

- layers below the two mains match by name path — k-th same-named sibling to k-th, within one kind,
  only under a matched parent;
- a matched counterpart is relinked and each field goes through `propagate::follow` as if the old main
  had become the new one — **overrides carry, nothing else does**; keyed list items match by id, else
  by position;
- an unmatched counterpart goes if `propagate::untouched`, else stays as the instance's own;
- a new layer is copied in at its anchor; order follows where the instance still had the old main's;
  the root's placement is its own.

**A variant choice is never an override** for a root linked straight to a main — it is the root's
`link`, which no reset compares. A nested copy's choice is a swap, and so is an override.
`variant::can_switch`: an instance switches if linked straight to a main (relinks) or is a nested copy
inside an outer instance (swaps via `swap::swap`); both rewrite through `variant::rewrite`.

**A component property is a view over fields**: a boolean is its bound layers' visibility, a text
property their content. Nothing stores its value on an instance; the default is the main's bound field;
an overridden property is a bound field differing from its source; resetting it resets that field. So
a switch carries properties with other overrides, and undo, preview and propagation need nothing new.

### Instance swap — `swap.rs`

**The link keeps the slot, the swap brings the contents.** `Node::swap` names the main a nested copy
inside an outer instance shows instead of its link's; `link` keeps the slot's identity (what places,
orders and scopes its resets). Only a nested copy can swap (`swap::can_swap`); a root linked straight
to a main changes main by relinking (`swap::can_relink`, `can_relink_to`: a main of its kind that
would not put a main inside itself), through `variant::switch`'s rewrite, so overrides carry by name
path. The Component card gives such a root a **Main** row in the Swap row's place: the same picker
over `swap::options` (which lists relink targets too), no override mark, since a relink is not an
override. Kit mains not yet vendored are offered to a swap only (`kit::swap_offers` requires
`can_swap`).

- The slot keeps the root's placement (transform, insets, layout item), size and visibility
  (`swap::is_slot_field`), following its link; **everything else comes from the swap** (fill, stroke,
  effects, corner radius, clip, `display`, and children copied from the swap's, linked to them).
- So `propagate`'s `followers` give a swapped copy `Takes::Slot` from its link and `Takes::Content`
  from its swap; `structural_copies` key its children by the swap; `reset::source_of` is
  `swap::content_source` (swap, else link); `reset::overrides` compares slot fields with the link and
  the rest with the swap; `linked_to_main` still reads `link`.
- **Same kind only**: no op turns a layer into another kind, and the copy's id must survive for its own
  copies to follow it.
- A swap is an override, one unit, reset by `SetSwap { None }`. Overrides carry across it, measured
  against the counterpart in the main the slot shows (`variant::rewrite`'s `Base::of_node`), so an
  override on the slot inside the outer main carries too. Layers match by that main's names
  (`paths_by` over each side's base).
- **`SetSwap` is all any door writes; `swap::settle` makes the rewrite**, in place through
  `variant::rewrite`'s swap mode — so the picker, a reset and a nested variant switch share one rewrite
  and undo replays it whole. Whenever a transaction writes a swap or link it reads every swapped node,
  clears a swap equal to what its slot shows, and rewrites each node after everything it copies
  (ancestors and link chain).
- A copy of a swapped copy follows it one link up with no swap of its own.
- A layer (or nested instance, or group holding one) the outer main added to the slot carries across a
  swap at its place (`component::slot_adds`: inside the slot, its chain of content sources never
  landing in the shown main). It never takes part in name matching — skipped from both pools with its
  subtree — so a same-named target layer is copied in beside it. A new-side layer some instance node
  already copies is not copied again.
- **Landing** (`swap::landed`): a swapped copy whose link would end at a main or nothing (outer
  instance detached, copied alone) becomes an instance of what it shows; one whose link climbs past a
  swapped node takes that swap unless it already shows it (`swap::land_ops_through`); deleting the
  swapped-to main clears the swap and leaves its layers the copy's own; detaching or dragging out
  leaves plain layers; `swap::tidy`, inside `settle_links`, clears a swap that no longer stands.
- `component::main_of` answers the main a root shows, through a swap — for the card's link, *Select
  all instances*, *Go to main*, properties.
- Offered: a swap property (`PropKind::Swap`), bound to a nested instance inside the main, made from
  that layer's binding line (*Instance*) or the Properties card's `+` menu (`new_swap_property`). Its
  **filter** is a case-insensitive prefix of a main's name (empty = all), prefilled by
  `swap::suggested_filter` (shown main's name less its last segment), edited in its own row; the
  picker searches within it. Every nested copy also gets a *Swap* row offering every main of its kind,
  unfiltered. The filter scopes what is offered, never what is valid.
- A swap property's **preferred mains** (`Property::preferred`, ids of mains, saved as wire ids) lead
  its picker under a *Preferred* heading with a star, in their own order and past the filter
  (`swap::preferred`); the rest of the list leaves them out. One that is no longer a main the slot
  can show is passed over, not removed. Edited on the main under the filter row
  (`variants::preferred_rows`): a row per main with a ×, then *Add a preferred main…* over
  `swap::options` at the bound slot. Copies remap them by the swap's rule (`remap_subtree`), and
  vendoring by `kit::relink`. The nested copy's own *Swap* row and the *Main* row offer none.

### Variants chrome — `panels/variants.rs`, `panels/component.rs`

- **A set** gets a *Variants* card in the Component card's place (`OndinApp::set_body`): glyph and name
  via `component::name_line` with its instances' count as the select link; variant count in the title
  (`OndinApp::panel_head`'s `size`); the clash note with a warning glyph; each variant property as a
  name field with a × (disabled for the last) and its values as chips, a hairline
  (`variants::card_rule`) between properties. A chip's popup renames, moves and deletes its value;
  deleting a used value confirms in a modal (`OndinApp::value_delete_confirmation`: *N variants use it
  and are deleted with it. M instances will detach.*; a value changed meanwhile deletes nothing).
- **A variant** gets the Component card with *Variant in* and the set as a link, its derived read-only
  name, a dropdown per property — a taken combination noted as *Variant clash* with *Select it* — and
  *Duplicate*.
- **An instance's card** gains a dropdown per variant property (no dot for a root linked to a main; a
  dot on a nested copy, whose choice is a swap), switching via `variant::switch` — *Mixed* across a
  disagreeing selection, a missing combination greyed, disabled where any root fails `can_switch` —
  then each component property (boolean as switch row, text as field) with its mark. Several
  instances get the rows when they share one owner. Label cut with `…` and a tooltip only when cut
  (`component::fitted_label` asks `Galley::elided`; `component::label_mark_in`, `MARK_ROOM`); a boolean
  row is `variants::toggle_row`. Bound to boolean variables the switch sits in a variable's capsule; a
  text field over string-variable-bound contents is the chip (§5.3f). *Reset fields* leaves
  property-driven fields alone.
- **A lone main or a set** gets a *Properties* card under the Component card: each property's kind
  glyph, name, ×, default and binding. A `+` in the header (`ui::HeadClicks::action_rect`,
  `OndinApp::panel_head`) opens a menu hung `ui::MENU_GAP` under the `+`'s painted box: *Boolean ·
  visibility*, *Text · text content*, an *Instance swap* row per unbound nested instance (one dimmed
  row saying why if none), and on a lone main *Variant · makes a set*. The card rests collapsed while
  empty (`sync_paint_collapse`). Three groups split by hairlines: boolean and text; swaps (each with
  its filter row); *Shown from nested* only when something is shown.
- **A layer inside a main** gets *Bind to a component property* rows in the identity card: a
  *Visibility* dropdown and, for text, *Content* — *None*, each property of that kind, *New
  property…*. A nested instance in a main gets the line under its instance card.
- **Menu**: `menu::Role::Set`; no *Create component* on a set or any selection holding a main or set
  (`menu::Context::holds_main`), dimmed with the commit's reason wherever it would refuse
  (`menu::create_refusal`); *Combine as variants* on two or more mains outside any set under one
  parent; *Add variant* on a set or variant; a set's *Select all instances*; *Reset <property>* on a
  linked layer whose bound field differs. No new chords.
- **Canvas**: a set's tag is a **tab** on the frame's top edge (`canvas::frame_label`, `SET_TAB_PAD`),
  card colour and border, text never the selection blue; *N variants* while selected, or on a clash
  the warning glyph and `panels::clash_text`'s sentence; a clashing variant's tag leads with the
  warning glyph. A `+` disc at the bottom edge's middle while the set alone is selected, with its own
  hit route (`canvas::set_plus_at` asked before any pick, calling `OndinApp::add_variant`;
  `chrome_claims` keeps guides off it; `begin_select_drag` starts nothing on it), drawn after
  `draw_key_outline` (`draw_set_plus`).
- **Layers**: the badge slot shows *N variants* on a collapsed set, `{}` on a layer bound to a property
  of the owner above; priority mask → set count → `{}` → frame size / subtract base. A variant row is
  named by its values and wears the filled hexagon.
- Departures from the mockup kept: values edited from a chip's popup rather than in place and dragged;
  binding from one line rather than `{}` buttons on layers/Appearance/Type; defining through the header
  `+` menu, with no *Default* row; *Preferred mains* as rows under the filter, not a list of their own.

### Nested component properties

A main's author can show a nested instance's own properties (variant dropdowns, booleans, text, swaps —
never raw fields) on every instance of the main.

- Opt-in once per nested instance: *Show properties on instances* on its card inside the main
  (`show_switch`); the main's Properties card lists them under *Shown from nested*, − to stop
  (`nested_slots`).
- Model: `PropKind::Nested`, a **showing**, not a value — at most one per owner, nameless, bound to the
  shown slots, driving no field, so binding checks, delete pruning and set scope are the property
  machinery's. `set_shown` binds the slot at its name path in every variant of a set, and
  `variant::settle` keeps that true at every commit: a main joining the set by any door, or a nested
  instance added or renamed onto a shown path, is bound (`owed_slots`). It binds by count, per layer
  name: a variant showing fewer slots of a name than another is owed the slots at the other's shown
  paths, the first variant's first. So a reorder of same-named slots in one variant keeps the showing
  on the layers picked and binds nothing in the others.
- An instance's card draws, after its own rows, one group per shown instance (`shown_nested`, layers
  order, recursing so two levels flatten under a › path), headed by the layer and the main it shows now
  (a group follows an outer swap); rows are `instance_rows` = `own_rows` + `shown_groups`, but **never
  a Swap row**. Several instances match groups by layer name path; *Different mains* where the slot
  disagrees; a group an outer boolean hides dims read-only (`hidden_by`).
- A shown row counts as a property, never an override (`property_fields`, `shown_rows_overridden`);
  *Reset properties* writes `reset_all_properties`.
- A nested copy's variant dropdown draws the dot: the choice is a swap, compared with the slot's own
  main's value of the same-named property and reset **within the set the copy shows now**
  (`nested_variant_state`, `nested_variant_reset`).
- Across a swap, a shown copy's variant values carry **by property name** (`carried_target`); booleans
  and text keep the rewrite's name-path carry.
- Only a main's author shows nested properties. A nested instance added inside an instance, not in
  its main, has no slot and shows nothing on the outer card; it is edited by selecting it. Promoting
  it waits on pushing an instance's changes to its main (`roadmap.md`, *Later*).

### Traps

- **Bypassing the commit door corrupts silently**: a main field edit applied without `propagate`
  becomes a permanent override on every copy. Nothing gates it.
- **Pass order is load-bearing**: `propagate` last of the component passes (a follow must carry the
  door's rewritten value); `variant::settle` before `propagate` (renames reach instances). Keep the
  order in `propagate::owed` only — never copy the list.
- **Two defences, swap**: `swap::settle` rewrites, and `component::check`'s `Membership` refuses an
  unexpanded swap — with `settle` gone, swap tests fail at the commit, not at their own assertions.
- **Two defences, preview**: a main's in-flow move keeps its copies in their slots because the door
  rewrite drops the move's follow *and* the layout passes take no follow for a drag; each alone
  suffices for that case, so either looks removable — but a pinned copy in a wider instance needs both
  and a pinned instance root needs the split.
- **`supplied` is a second defence** behind `follows_nothing` for the vault; keep it as the general
  rule for an edit that supplies a counterpart itself.
- **`only_new` in `restore_children` is load-bearing**: without it the commit is not refused —
  `settle_links` cuts the duplicate and an unlinked copy lands silently.
- **Fixtures the size of their main hide layout defects**: test instances wider than their main (inset
  re-pinning shows only there).
- **Keyed lists**: never rebuild a list by cloning values and collecting (re-keys by position, cutting
  every item from its counterpart); never put an id inside a `PartialEq` value type; whole-list `==`
  compares ids — use `item::same_values` for "same look".
- **A migrated item id doesn't collide because of `reserve_existing_ids`**, not actor 0 (actor is a
  hash; 0 is improbable, not excluded).
- **`ComponentCycle` must stay iterative**: the main graph is not bounded by `io::MAX_TREE_DEPTH`.
- **`component::check` must run over the whole document**, not `dirty`.
- **`can_hold` must stay true for a node's own parent**, or a variant of a loader-admitted mixed set
  leaves its set on a nudge.
- **A reset must state insets and layout items** for every placing/resizing write, or `keep_insets`
  re-pins and `keep_flex_sizes` freezes growth.
- **`reset::outermost_root` must stop** at a root linked straight to a main and at a swapped root, or a
  host or sibling slot masks removed children.
- **A deleted main is lost from nothing**: don't count its children as lost in `propagate_structure`,
  or every instance is emptied.
