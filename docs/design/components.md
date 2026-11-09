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
  - **whole** — transform, geometry, text with spans, the span ops, name, visibility, opacity, blend mode, pivot,
    clip, mask, mask mode, fill rule, a frame's mode set (`SetModes`, one field for every collection), its width queries (`SetQueries`, likewise; `reset::state_ops` writes them back), the variable bindings (`SetCornerVars`,
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
- No arm acts on a copy the edit itself deleted. No gained child is copied into a copy holding a
  child whose link to it the edit wrote (`supplied`: made by the edit or relinked by it) — a kit
  update writes a nested main's layer and the nested copy's layer in one transaction, and a push
  links the instance's own layer to the one it gives the main. No arm, nor the field pass, writes a copy in the kit vault
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
  the layout item of each layer it resizes (`reset::items_stated`), or `keep_flex_sizes` reads it as
  the hand's resize. `reset::stated` is a reset with both (`reset::placement_stated` the insets), the
  context menu's door and `reset_overrides`'.

### Push to main — `push.rs`

Reset's opposite: an instance's differences written into its main. The commit's own passes carry them
on — an instance still equal to the main's old value takes the new one, one with its own keeps it.

- **One link up.** A copy pushes into `reset::source_of`, the node Reset compares it with, and never
  past the main holding that node (`push::home`). A swapped copy's slot fields go to its link and the
  rest to its swap, the split `reset::overrides` makes; a target outside `home`'s subtree is skipped,
  so a swapped copy's content overrides inside an instance are not pushed (its swap is).
- **Five families** (`push::Kind`), counted by `push::pushable`: properties (a field a boolean, text
  or swap property drives, `variant::property_fields`), fields (every other override), local layers,
  order, removals. *Push all* is the first four in one transaction: the layers first, the order read
  on the document they leave so a pushed layer is ordered with its siblings, then the fields. A
  removal deletes the main's layer and is only ever pushed on its own.
- **A field** is the target's own `reset::state_ops` value with the parts the reset would change on
  the copy taken from the copy (`push::changed_from`: objects part by part, a run of numbers number
  by number, a list or string whole), retargeted at the target — so a mark's reset of one axis,
  written as the whole size, pushes that axis alone (`push::fields` turns a reset's ops into their
  pushes). **A swap** is `variant::switch` on the slot — a relink for a
  root linked straight to a main, a swap for a nested copy; the copy's swap then equals what its slot
  shows and `swap::settle` clears it. **A list item the copy removed** is deleted from the target's
  list (`push::item_removal`).
- **A local layer** (outermost only, `push::is_local`; `push::push_layer`) is copied under its
  parent's source at `propagate::anchor`'s place, and in the same transaction each of its nodes is
  `SetLink`ed to its copy, so the commit gives it to every other instance and not to this one again
  (`supplied`, above). **Order**: the main parent's children take the instance's order in the slots
  its linked children hold. **A removal** is the main layer's `DeleteNode`; *Main loses a child*
  decides each other instance's counterpart.
- **Placement is stated** (`push::stated`): every pushed `SetTransform` or resizing `SetGeometry`
  with no `SetInsets` beside it carries the target's insets, and every resize the target's layout
  item, so `keep_insets` and `keep_flex_sizes` do not read the push as a hand's move or resize.
- **Refused**: `push::blocked` — the main in the vault (`Kit`) or effectively locked (`Locked`).
  `push::layer_blocked` — `WrapsLinked`, a local group holding linked copies, which would carry the
  main's own children; `HoldsMain`, where `component::can_hold` refuses the layer under its parent's
  source (an instance of the main, or of one reaching it).
- **Counted**: `push::outcome(before, after, root, ops)` counts the main's other instances that took
  the push and those that kept their own (a removal: lost it, or kept it unlinked);
  `push::predicted` runs `propagate::owed` on a scratch document, for a row's tooltip.

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

Run after the last op of `apply`, before the commit (a transaction may pass through a breaking
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

Item ids and the component fields arrived with schema v5 (the current version is in `io.md`).
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
- No verb un-makes a component (`non-goals.md`); one would owe its instances a detach, as a delete
  does.
- Unruled, parked in `roadmap.md`: a main-scoped guide not copied into instances; whether a group
  *instance* gets a canvas label.
- Real-time collaboration — non-goal for now (it is where linked copies cost).

### Chrome

**Component-ness is shape, never the selection accent**: a filled hexagon for a main, an outline
hexagon for an instance. The filled hexagon is its own glyph, `icon::HEXAGON_FILL`, which
`ui::paint_hexagon_filled` sets centred where `HEXAGON` would sit; used on canvas, layers, the
Component card's faces and the *Create component* / *Duplicate as component* menu rows.

Canvas:

- A main's tag is a chip, `color::COMPONENT` fill (no edge), filled hexagon and name in
  `COMPONENT_INK`, *N instances* in `COMPONENT_INK_DIM` while selected. Shown selected or not, where it is worn. A group that is a
  main gets a canvas label as a page frame does (`OndinApp::tagged`, walked by `canvas::draw_frame_labels`).
- A frame instance on its page wears a tag: outline hexagon and name in `COMPONENT`, its main's name trailing in
  `COMPONENT_DIM` when they differ — drawn **only while the instance is selected or entered**. The
  entered instance (`canvas::entered_instance`) is tagged wherever it sits, nested included
  (`canvas::canvas_tags`), and reads *Editing Name* with *· Esc to exit* in `COMPONENT_DIM`: the
  top-left tag is the one place no other chrome, the size badge included, competes for.
- Neither label ever takes `color::SELECT`; nor does a set's tab. A main's or instance root's
  selection/hover outlines are `COMPONENT` (`canvas::chrome_hue`), and so are handles and the size
  badge when every selected layer is one (`selection_hue`); a set and layers inside a component keep
  `SELECT`. Nothing off the canvas takes the hue.
- A main wears its chip and a set its tab only on its page, as a frame its tag, and a set's variants
  keep theirs (`app::wears_tag`); nothing inside a frame, a main or an instance wears a tag but the
  entered instance, since a frame inside another frame or a group picks as a group does
  (`app-frames-and-copies.md` §9.4). A selected main or instance without a tag carries its hexagon
  in the size badge instead.
- **An instance picks like a group**, whatever its kind: `query::group_chain` keeps an instance root
  as a rung even when it is a page frame, and `canvas::pick_leaf` does not skip an occupied page frame
  that is an instance — for a frame instance an exception to D22. `Enter` enters it ahead of its text/picture/path arms
  (`enter_action`), and so does a double-click on its own background (`double_click_pick`).
- `canvas::draw_component_chrome`: a `COMPONENT` hairline round each instance of a selected main; round
  the entered instance (`canvas::entered_instance`, nearest instance root at/above `entered_group`) a
  dashed `COMPONENT` boundary, cleared once the selection leaves by any door
  (`OndinApp::leave_a_scope_the_selection_left`). This chrome lies over the artwork, not the canvas
  ground, so it takes the violet: a neutral grey vanished on a light frame.

Layers panel: neutral icons wearing the hexagon. Trailing mark, hidden while the row is hovered (its
slot stays, so the eye and lock hold still): **+** for a local layer (unlinked inside an instance, or a
local instance); a **dot** for an override on the row; on a collapsed row
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
- **Link**: under the description on the same three cards, `component::link_field` — the host while
  unfocused, the whole address while focused, committed through `web_link` on `Enter` or defocus as
  one undo step (*Link*). The field and the step wear `icon::GLOBE`, as does the host in the insert
  picker's hover card. A refused address stays in the field, ringed in the error colour with the
  reason under it, and is not stored; `Escape` drops it. A variant without its own link hints its
  set's host. An open button (`icon::ARROW_SQUARE_OUT`) opens the link shown, the main's own else its
  set's (`component::doc_link_of`). A single instance's card puts the same button (tooltip *Open
  documentation: ‹host›*) at the end of the name line when its main or set has a link, and none
  otherwise. The host shown (`ondin_core::link_host`) ends at a `\` as well as at `/`, `?` and `#`,
  as a browser reads the address, so it is the host the link opens. **`component::open_doc_link` is the only door to the browser** (egui's `OpenUrl`)
  and re-checks `web_link`, so only an `http`/`https` address leaves the app; nothing is fetched.
- A face's instance count is on its name's line and is the *Select all*
  (`component::name_line(ui, glyph, name, instances)`; at zero plain `text::dim()` and dead).
- Resets: *Reset all*, and a ⋯ overflow with *Reset fields N*, *Restore removed children N*, *Reset
  order*, *Reset properties* — **disabled, not hidden** at zero; counts shown only above zero. No reset
  deletes a local *layer*; list resets do remove the instance's own items.
- The drift summary counts fields, removed children and order, and properties apart (`PropDrift`);
  several instances read *K with changes*. Counts come from `reset::Drift` cached on
  `EditorSession::revision` (`OndinApp::drift_cache`, also read by the menu; property readings in
  `OndinApp::prop_cache`). Resets commit through `commit_edit` (`OndinApp::reset_tx`).
- **Pushes** (`panels/push.rs`): a single instance's ⋯ adds, under a rule after the resets, *Push all
  to main*, *Push properties to main N* (where the component defines properties), *Push fields to main
  N*, *Push local layers to main N*, *Push order to main*, and under a second rule *Push removals to
  main N*, the one push that deletes. Text rows, disabled at zero (*Nothing here differs from the main
  component*; order *The order matches the main*) or with the kit or lock reason. A hovered row
  predicts its effect: *Write this instance's fields into Card — 9 other instances follow, 2 keep
  their own*; *Delete Footer from Card — 9 instances lose it, 2 with edits keep it, unlinked*.
- A local layer, and an instance dropped into another instance (which shows the *Local to this
  instance* line too), gets a full-width `ui::action_button` *Push to ‹main›* under that line,
  disabled with its reason. Pushing a nested instance is how it joins the main; nothing in the push
  shows its properties, which stays the main author's call (in a set, `owed_slots` still binds it
  where another variant shows a slot at its name path).
- **Every reset of one thing offers its push beside it** on a right-click (`ui::reset_push_menu`): a
  card header with overrides (*Reset fill* / *Push fill to main*), a field's mark (*Reset to main* /
  *Push to main*), a marked label (`component::label_mark`: *Push ‹label› to main*, on a property
  row, in the Type popup and on the grid template and item placement rows), an overridden dropdown's list (*Push
  to main* under its reset row), a ghost row (*Restore* / *Push removal to main*). The header's hover
  chip stays the reset alone: a push reaches every instance, so it is never one stray click away. A
  menu's push reaches the reset door as that reset with `ui::request_push` set; `commit_reset` and the
  property rows' commit loop take it (`ui::take_push`) and commit `push::fields` instead, refused
  with *Select one instance to push* when the reset spans several instances. Each frame
  clears a request no door took before any widget runs, so it cannot turn a later reset into a push.
- A push is one undo step, *Push to main* (`icon::PUSH_TO_MAIN`), with no confirmation; its toast says
  what followed: *Pushed 3 fields into Card · 9 instances follow · 2 keep their own · Ctrl+Z to undo*,
  *Pushed “Badge” into Card · 11 other instances now have it*, *Deleted “Footer” from Card · 9
  instances lost it · 2 with edits keep it, unlinked*.
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
(`context-menus.md` §3's exception); *Push all to main* after *Reset all*, omitted when nothing would
be pushed and dimmed with `menu::PUSH_KIT` or `PUSH_LOCKED` — the finer pushes live on the card. Toasts are text-only `EditorSession::info` lines naming `Ctrl+Z`.
A selection never spans pages, so *Select all instances* selects those on the current page and the
status line counts the ones on other pages; *Go to main component* switches to the main's page
(`OndinApp::select_main`).

### Variants and component properties — `variant.rs`

Rulings: deleting a variant detaches its instances (deleting a value deletes the variants holding it,
so theirs detach too); `Ctrl+D` on a variant makes an **instance** (a plain layer in the set) — *Add
variant* is the verb that takes the next free combination; an instance's name is copied verbatim.

**A component set is a frame around mains, never a main; a variant is a main whose parent is a set.**

- `Node::set: Option<VariantSet>` — the set's variant properties, each `VariantProp { name, values,
  states }`, and `element: RootElement` (*Interaction states*, below).
- `Node::variant: Vec<String>` — a variant's values, one per property in the set's order.
- `Node::lineage: Option<NodeId>` — the **lineage key** a variant's layer shares with its counterparts
  in the other variants; `Node::lineage()` is the recorded key, else the layer's own id. Only
  `build::insert_subtrees_as` writes one, when a variant is copied into its own set as a new main
  (*Add variant*, *Duplicate as component* on a variant): each copied layer takes its source's key, so
  a chain of copies shares one. `remap_subtree` remaps a key pointing inside the template (a whole set
  copied or vendored keeps its variants' keys consistent), gives a key pointing outside that several
  copied layers share one fresh key (its layer was deleted, and the copies still pair), and drops one
  pointing outside that only one holds (a copy within one variant gets a fresh key), so a key is unique
  within a variant. A shared key rather than
  a pointer to the source, because a pointer breaks on a chain of copies and when the source is
  deleted. Never shown, never an override (`reset::state_ops` ignores it).
- `Node::props: Vec<Keyed<Property>>` — component properties of a main or set:
  `Property { name, kind: PropKind::{Boolean, Text, Swap, Nested}, bound: Vec<NodeId>, filter: String, preferred: Vec<NodeId> }`.
- `Node::description: String` (`description()`) — what a main or set is for, in the user's words; empty
  is none. Written by `SetDescription { of: Described::Node(id), text }`, the one op a style's and a
  variable's description share (§5.3f). An instance reads its main's through the link, so a copy never
  holds one; a duplicated main keeps its own (`remap_subtree` copies it).
- `Node::doc_link: String` (`doc_link()`) — where a main's or set's documentation lives; empty is
  none. Held to `ondin_core::web_link`'s form, `http`/`https` only: `SetDocLink { id, url }` refuses
  any other url (`OpError::BadDocLink`), and the loader drops one rather than refusing the file.
  Copied and cleared as `description` is. `web_link` trims, puts `https://` before a bare host,
  lower-cases the scheme and refuses another scheme (`LinkRefusal::NotWeb`) or a missing host or a
  space or control character (`Malformed`); a colon followed by a digit is a port, so `localhost:8080`
  is a bare host. `link_host` is the host a card shows.
- Ops `SetVariantSet`, `SetVariant`, `SetProperties`, `SetDescription`, `SetDocLink`, each inverting to
  the old value; no ink, no-ops in `absorb`, `overwrites` none, skipped by `propagate::mode`, never
  written by `reset::state_ops` — **never followed or overridden**. `description` and
  `Property::preferred` are new in v9; `doc_link`, `VariantProp::states`, `VariantSet::element` and
  `lineage` in v13.
- `Node::make_copy_of` is the one place a node becomes a linked copy (linked; swap, flag, values,
  properties, description, link and lineage key cleared); every door that makes an instance node goes
  through it.

**`variant::check`** (as `LinkRule::Variant`):

| Rule | Holds |
| --- | --- |
| `SetKind` | a set only on an `Artboard`, never a main or linked, never inside a main, instance or set |
| `SetNames` | set property names/values well formed |
| `Values` | a main in a set fits it; values on anything else refused |
| `PropertyOwner` | properties only on a main not in a set, or a set |
| `PropertyName` | unique, non-empty, not a variant property's name (the nameless showing exempt; a second showing refused) |
| `Binding` | a bound layer exists strictly inside the owner's main (for a set, inside a variant, never a variant's root); text for a text property; a nested instance for a swap or showing; bound once per field |
| `States` | at most one property of a set holds ties; it ties only its own values, each state once |

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

- layers below the two mains match only under a matched parent and within one kind: between two
  variants of one set, first by lineage key (the one child of the matched parent holding it), else by
  name path — k-th same-named sibling to k-th. The key keeps overrides across a rename on either side,
  which a name path loses silently (*Icon copy* after a paste);
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

**Interaction states.** A variant property's values may be tied to browser states, for code export
(`export.md`, *Code export*): `VariantProp::states` ties values by name to
`Interaction::{Hover, FocusVisible, Active, Disabled}` (*Hover*, *Focus*, *Pressed*, *Disabled* in the
app). **A property with any tie is its set's interaction state** (`VariantProp::is_interaction`,
`VariantSet::interaction`); there is no separate mark, so a set is never interactive with nothing tied.
**Rest is not stored**: it is the default variant's value (`variant::rest_value`, through
`kit::default_variant`), so it follows the default; a value tied to none stays a plain prop value.
`VariantSet::element` (`RootElement::{Button, Link, Plain}`, Button by default) says what the root is
written as, read only while a property holds ties.

- `tie_state` ties a value or unties it. **The first tie on a property also ties its other values as
  their names suggest** (`Interaction::guess`: *hover*; *pressed*/*active*; *focus*; *disabled*), in
  the same step, never rest's value and never a state already tied; untying the last makes the property
  plain again. Guessing waits for that deliberate first tie, so a *Tab* set's *Active* value is never
  written as `:active` unasked. `why_not_tie` refuses a tie while another property holds the states,
  or on rest's value; `tie_state` also refuses a state another value holds. `set_element` sets the
  element. `rename_value` keeps a tie; `delete_value` drops it.
- `unpaired_states(doc, set) -> Vec<Unpaired { variant, why, layer }>` lists each state variant whose
  layers do not line up with its rest variant (the one with the same values of every other property):
  no rest variant, other text, or another order (*“Label” moves before “Bg”*, naming the layer moved
  ahead). Text is other where its bound variable, its cased content or its runs styled apart from the
  layer differ, since the code export writes each such run as an element of its own. Layers pair by lineage key, whichever side recorded it (rest may be the copy), else name
  path. A layer only one side draws is a show or hide, and one of another kind is a swap (rest's
  hidden, the state's shown), as the code export writes them; neither is a failure.
- A set saved before lineage keys is not stamped when tied: its layers pair by name path until a
  variant is copied again.

### Instance swap — `swap.rs`

**The link keeps the slot, the swap brings the contents.** `Node::swap` names the main a nested copy
inside an outer instance shows instead of its link's; `link` keeps the slot's identity (what places,
orders and scopes its resets). Only a nested copy can swap (`swap::can_swap`); a root linked straight
to a main changes main by relinking (`swap::can_relink`, `can_relink_to`: a main of its kind that
would not put a main inside itself), through `variant::switch`'s rewrite, so overrides carry as a
switch's do. The Component card gives such a root a **Main** row in the Swap row's place: the same picker
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
  unfiltered. The filter scopes what is offered, never what is valid. Neither offers a main that
  reaches, through its instances, the main the copy sits in or a main any instance around it shows,
  on a page or not (`swap::can_swap_to`): there the copy would hold its own slot, which the rewrite
  never copies into it.
- A swap property's **preferred mains** (`Property::preferred`, ids of mains, saved as wire ids) lead
  its picker under a *Preferred* heading with a star, in their own order and past the filter
  (`swap::preferred`); the rest of the list leaves them out. One that is no longer a main the slot
  can show is passed over, not removed. Edited on the main under the filter row
  (`variants::preferred_rows`): a row per main with a ×, then *Add a preferred main…* over
  `swap::options` at the bound slot. Copies remap them by the swap's rule (`remap_subtree`), a
  paste or *Make local* by `kit::relink`, and `kit::vendor` and a kit update point each at the
  document's copy of that main where it holds one. A preferred main never forces vendoring: one the
  document does not hold keeps the kit's id and is passed over. The nested copy's own *Swap* row
  and the *Main* row offer none.

### Variants chrome — `panels/variants.rs`, `panels/component.rs`

- **A set** gets a *Variants* card in the Component card's place (`OndinApp::set_body`): glyph and name
  via `component::name_line` with its instances' count as the select link; variant count in the title
  (`OndinApp::panel_head`'s `size`); the clash note with a warning glyph; each variant property as a
  name field, a menu button and a × (disabled for the last), and its values as chips, a hairline
  (`variants::card_rule`) between properties. A chip's popup renames, moves and deletes its value;
  deleting a used value confirms in a modal (`OndinApp::value_delete_confirmation`: *N variants use it
  and are deleted with it. M instances will detach.*; a value changed meanwhile deletes nothing).
- **Interaction state on the Variants card.** Every value chip's popup has a *State ▸* row
  (`icon::HAND_POINTING`, `ui::submenu_row`), greyed with `why_not_tie`'s reason on rest's value and
  on a second property's. It opens a flyout (`ui::flyout`) of checkable rows *None*, then *Hover*,
  *Pressed*, *Focus*, *Disabled*, the selector in the accel column; a state another value holds shows
  that value's name there and is greyed. Undo step *Interaction state*. Once the property holds a tie,
  each value chip is `ui::label_button_noted`, its note `rest` or the tied selector, and an *Element*
  dropdown (Button, Link, Plain; undo step *Element*) shows under its chips. Each `unpaired_states` entry is a warning note
  (*Large, Focus does not pair*, the sentence in its tooltip) with *Select it* (the layer, else the
  variant), as the clash note is.
- **A variant** gets the Component card with *Variant in* and the set as a link, its derived read-only
  name, a dropdown per property — a taken combination noted as *Variant clash* with *Select it* — and
  *Duplicate*.
- **An instance's card** gains a dropdown per variant property (no dot for a root linked to a main; a
  dot on a nested copy, whose choice is a swap), switching via `variant::switch` — *Mixed* across a
  disagreeing selection, a missing combination greyed, disabled where any root fails `can_switch` —
  then each component property (boolean as switch row, text as field) with its mark. The interaction
  state's label hovers *In code the browser sets <property>. Pick one here to draw it, and to force it
  in the export; <rest> leaves it to the browser.* Several
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
- **Layers**: the badge slot shows *N variants* on a collapsed set, `icon::PROPERTY_BOUND` on a layer bound
  to a property of the owner above; priority mask → set count → bound → frame size / subtract base. A variant row is
  named by its values and wears the filled hexagon.
- Values are renamed, moved and deleted from a chip's popup, never edited in place or dragged;
  binding is from one line, not `{}` buttons on layers/Appearance/Type; defining is through the
  header `+` menu, with no *Default* row; *Preferred mains* are rows under the filter, not a list of
  their own.

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
  its main, has no slot and shows nothing on the outer card; it is edited by selecting it, or pushed
  into the main (*Push to ‹main›*), which shows nothing until the main's author opts in.

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
- **`supplied` is a second defence** behind `follows_nothing` for the vault, and the only one for a
  pushed local layer: it must test a link the edit **wrote**, not a node it made, or the pushing
  instance gets its layer twice (`a_pushed_local_layer_lands_in_the_main_once_per_instance` asserts
  the one copy).
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
- **A documentation link is checked three times**: `SetDocLink` refuses, the loader drops, and
  `open_doc_link` re-checks before the browser. Each looks redundant; `InsertSubtree` does not look at
  `doc_link`, so the last check is what keeps a non-web address that arrives another way from being
  opened. `a_doc_link_is_a_web_address_kept_on_its_main` pins the first two; nothing pins the third.
  A credit's addresses (§5.3) take the same three: `SetCredit` refuses, the loader empties
  (`Credit::normalized`), and the Credit card opens them through `open_doc_link`.
