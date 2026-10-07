//! Keeping instances in step with their mains (§5.3d build step 3, §15 D978–D980).
//!
//! An instance is a linked copy, so a main's edit reaches its copies as more
//! operations in the same transaction — written here, at the commit, by
//! [`propagate`], which `EditorSession::commit_inner` runs after its other
//! passes; the live preview runs it too, over the gesture's transaction as that
//! door rewrites it (build step 6, `EditorSession::preview_follows`), and commits
//! nothing. Nothing is recorded about which values a copy overrides
//! (§15 D979 (a)): **a copy follows wherever it still holds the main's old value,
//! and keeps its own value everywhere else.** That one comparison is the whole
//! of the override model.
//!
//! **Reading a value without a reader per field.** Every node-property operation's
//! inverse carries the old value of exactly the field it writes, so a copy's
//! current value is read by applying the main's operation, retargeted to the copy,
//! to a scratch document and keeping the inverse (`Document::peek`). Three ways of
//! comparing, chosen per operation in [`mode`]:
//! - **whole value** — a copy follows when its old value equals the main's;
//! - **field by field** for a struct payload (a text style, a layout item, the
//!   insets): each field follows on its own, so an instance that changed only its
//!   font size still follows the main's new family;
//! - **item by item** for the five keyed lists (§15 D980): items are matched by
//!   id, each item's fields follow on their own, and additions, removals and a
//!   reorder follow by the structural rule one level down.
//!
//! **Not here:** structure (a main gaining or losing a child — step 4), and
//! anything about whether the values the commit's arithmetic writes compare
//! exactly — the build's main open risk (§5.3d), to be measured against this pass.

use crate::document::Document;
use crate::id::NodeId;
use crate::item::{ItemId, Keyed};
use crate::node::NodeKind;
use crate::op::{Operation, Transaction};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// How an operation's value is compared between a main and a copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Never propagated: editing state (locks), the component links themselves,
    /// and everything that is not a node's property.
    Skip,
    Whole,
    Fields,
    Items,
}

/// Wildcard-free on purpose: a new operation stops the build here, and its author
/// says how a copy follows it.
fn mode(op: &Operation) -> Mode {
    match op {
        Operation::SetTransform { .. }
        | Operation::SetGeometry { .. }
        | Operation::SetText { .. }
        | Operation::SetTextSpans { .. }
        | Operation::SetParagraphSpans { .. }
        | Operation::SetName { .. }
        | Operation::SetVisible { .. }
        | Operation::SetOpacity { .. }
        | Operation::SetPivot { .. }
        | Operation::SetClip { .. }
        | Operation::SetMask { .. }
        | Operation::SetMaskMode { .. }
        | Operation::SetFillRule { .. } => Mode::Whole,
        Operation::SetTextStyle { .. }
        | Operation::SetParagraphStyle { .. }
        | Operation::SetBlockStyle { .. }
        | Operation::SetInsets { .. }
        | Operation::SetDisplay { .. }
        | Operation::SetLayoutItem { .. } => Mode::Fields,
        Operation::SetFills { .. }
        | Operation::SetStrokes { .. }
        | Operation::SetEffects { .. }
        | Operation::SetExports { .. }
        | Operation::SetLayoutGrids { .. } => Mode::Items,
        // A lock is how one person is editing, not what the layer is.
        Operation::SetLocked { .. }
        | Operation::SetProportionsLocked { .. }
        | Operation::SetComponent { .. }
        | Operation::SetLink { .. }
        // A copy of a swapped node follows what the swap rewrote, one link up,
        // never the swap itself (§15 D983).
        | Operation::SetSwap { .. }
        // A set, a variant's values and a component's properties belong to the
        // main or set they are on; an instance reads them through its link.
        | Operation::SetVariantSet { .. }
        | Operation::SetVariant { .. }
        | Operation::SetProperties { .. }
        | Operation::CreateNode { .. }
        | Operation::DeleteNode { .. }
        | Operation::InsertSubtree { .. }
        | Operation::Reparent { .. }
        | Operation::Reorder { .. }
        | Operation::SetCanvasBackground { .. }
        | Operation::AddGuide { .. }
        | Operation::RemoveGuide { .. }
        | Operation::SetGuidePosition { .. }
        | Operation::SetGuideColor { .. }
        | Operation::SetGuideScope { .. }
        | Operation::AddImage { .. }
        | Operation::RemoveImage { .. } => Mode::Skip,
    }
}

/// Fields that say where an **instance root** sits and whether it shows — its own
/// placement, never its main's: the transform (§5.3d), its insets and item
/// properties inside its parent, and its visibility (a main hidden on a
/// components page must not hide every instance of it).
pub(crate) fn is_placement(op: &Operation) -> bool {
    matches!(
        op,
        Operation::SetTransform { .. }
            | Operation::SetInsets { .. }
            | Operation::SetLayoutItem { .. }
            | Operation::SetVisible { .. }
    )
}

/// Whether the operation also writes text spans, which index into the content and
/// so may only follow onto a copy holding the same content.
fn writes_spans(op: &Operation) -> bool {
    matches!(
        op,
        Operation::SetTextSpans { .. }
            | Operation::SetParagraphSpans { .. }
            | Operation::SetTextStyle { spans: Some(_), .. }
            | Operation::SetParagraphStyle { spans: Some(_), .. }
    )
}

/// Whether `tx` writes to any node something is copied from — whether
/// [`propagate`] can owe it anything. A scan of the links and nothing more, so a
/// caller can skip the work around the pass (the live preview's door rewrites)
/// for the nearly every edit that touches no main.
pub fn touches_copied(doc: &Document, tx: &Transaction) -> bool {
    let written: FxHashSet<NodeId> = tx.0.iter().filter_map(Operation::overwrites).collect();
    !written.is_empty()
        && doc.node_map().values().any(|n| {
            n.link.is_some_and(|l| written.contains(&l))
                || n.swap.is_some_and(|s| written.contains(&s))
        })
}

/// What of its source's edits a copy takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Takes {
    /// Everything — but an instance root's placement, which is its own.
    All,
    /// Only the slot's fields: a swapped copy from its link (§15 D983).
    Slot,
    /// All but the slot's fields: a swapped copy from its swap.
    Content,
}

/// Every source → the copies that follow it, and what of it each takes. A copy
/// follows its link; a **swapped** copy follows its link for the slot's fields
/// and its swap for the rest (§15 D983, `swap::is_slot_field`). Each list sorted.
fn followers(doc: &Document) -> FxHashMap<NodeId, Vec<(NodeId, Takes)>> {
    let mut out: FxHashMap<NodeId, Vec<(NodeId, Takes)>> = FxHashMap::default();
    for n in doc.node_map().values() {
        match (n.link, n.swap) {
            (Some(l), Some(s)) => {
                out.entry(l).or_default().push((n.id, Takes::Slot));
                out.entry(s).or_default().push((n.id, Takes::Content));
            }
            (Some(l), None) => out.entry(l).or_default().push((n.id, Takes::All)),
            _ => {}
        }
    }
    for list in out.values_mut() {
        list.sort_by_key(|(id, _)| *id);
    }
    out
}

/// Every source → the copies whose **children** are copies of its children: a
/// copy of its link's, a swapped copy of its swap's (§15 D983). Each list sorted.
fn structural_copies(doc: &Document) -> FxHashMap<NodeId, Vec<NodeId>> {
    let mut out: FxHashMap<NodeId, Vec<NodeId>> = FxHashMap::default();
    for n in doc.node_map().values() {
        if let Some(src) = n.swap.or(n.link) {
            out.entry(src).or_default().push(n.id);
        }
    }
    for list in out.values_mut() {
        list.sort();
    }
    out
}

/// The operations `tx`'s edits to mains owe their instances: for every copy, at
/// every depth, the main's change written onto whatever the copy still shares with
/// it. Empty in a document with no instances.
///
/// - **The user's own edits win**: a copy the transaction itself edits in the same
///   way is not overwritten, and the propagation does not pass through it.
/// - An edit reaches a copy's copies in turn (a nested instance inside an outer
///   main carries it on to the outer main's instances), each compared with its own
///   old value.
/// - Where one transaction edits the same field of a node twice, the last edit is
///   the one carried, compared against the value before the transaction.
/// - **A copy the transaction deletes, or whose link it rewrites, takes nothing**
///   ([`leaves_its_source`]): the copy map is read from the document before the
///   edit, and the structural pass ahead of this one deletes or cuts the
///   counterparts of a layer dragged out of its main. Following onto those wrote
///   a `SetTransform` on a deleted node, and the whole drag was refused at the
///   commit; or, for a counterpart kept because the instance had changed it, moved
///   the instance's own layer to the main's new place, outside its instance
///   (`[X3-L1-01]`).
pub fn propagate(doc: &Document, tx: &Transaction) -> Vec<Operation> {
    let copies = followers(doc);
    if copies.is_empty() {
        return Vec::new();
    }
    let gone = leaves_its_source(doc, tx);
    let user: FxHashSet<_> = tx.0.iter().filter_map(Operation::shape_key).collect();
    // The last edit of each (node, field), in the order those last edits came.
    let mut last: Vec<(usize, &Operation)> = Vec::new();
    let mut seen: FxHashMap<_, usize> = FxHashMap::default();
    for (i, op) in tx.0.iter().enumerate() {
        if mode(op) == Mode::Skip {
            continue;
        }
        let (Some(key), Some(id)) = (op.shape_key(), op.overwrites()) else {
            continue;
        };
        if !copies.contains_key(&id) {
            continue;
        }
        seen.insert(key, i);
        last.push((i, op));
    }
    last.retain(|(i, op)| op.shape_key().and_then(|k| seen.get(&k)) == Some(i));
    if last.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    for (_, op) in last {
        let Some(n) = op.overwrites() else { continue };
        let Some(old) = doc.peek(op) else {
            continue;
        };
        let mut queue = vec![(n, op.clone(), old)];
        while let Some((src, new, old)) = queue.pop() {
            for &(m, takes) in copies.get(&src).into_iter().flatten() {
                if gone.contains(&m) {
                    continue;
                }
                let Some(probe) = new.retargeted(m) else {
                    continue;
                };
                if probe.shape_key().is_some_and(|k| user.contains(&k)) {
                    continue;
                }
                let skip = match takes {
                    Takes::All => is_placement(op) && linked_to_main(doc, m),
                    Takes::Slot => !crate::swap::is_slot_field(op),
                    Takes::Content => crate::swap::is_slot_field(op),
                };
                if skip {
                    continue;
                }
                if writes_spans(op) && !same_content(doc, src, m) {
                    continue;
                }
                let Some(cur) = doc.peek(&probe) else {
                    continue; // a copy of another kind cannot take this edit
                };
                if let Some(follow) = follow(&new, &old, &cur, m) {
                    out.push(follow.clone());
                    queue.push((m, follow, cur));
                }
            }
        }
    }
    out
}

/// Everything the components owe `tx`, appended to it in the order the passes
/// must run: what `EditorSession::commit_inner` runs once its doors' rewrites
/// (`build::keep_insets`, `build::keep_flex_sizes`) are done.
///
/// **One function so the order is one fact.** The core tests that pin it —
/// *"settle after propagate leaves the instance named Small"* — ran it through
/// a hand-written copy of this list, and reversing two passes in the app's own
/// copy passed every test in the workspace (`[X6.2-L6-05]`, CLAUDE.md's gate
/// hole 16: two lists a person keeps agree whatever either says). The tests call
/// this now, so a flip here fails there.
pub fn owed(doc: &Document, tx: &mut Transaction, ids: &mut crate::id::IdSource) {
    // A swap's rewrite (§15 D983): a nested copy swapped to another main, or
    // back, has its children and fields rewritten in place here — the one place
    // they are — so the picker, a reset and a nested variant switch need write
    // only the `SetSwap`. First, so what it brings in and takes away reaches the
    // copy's own copies through the structural pass below.
    let swaps = crate::swap::settle(doc, tx, ids);
    tx.0.extend(swaps);
    // A main's children gained, lost, moved or reordered reach its instances
    // (§5.3d build step 4) — before the settling below, which tidies any link
    // these leave behind.
    let structure = propagate_structure(doc, tx, ids);
    tx.0.extend(structure);
    // And the links a structural edit owes (§5.3d): a link into a deleted node
    // climbs past it, a linked layer moved out of its instance becomes its own,
    // and a swap that no longer stands is settled (§15 D983) — every door that
    // moves, deletes or regroups comes through here, so none of them has to know
    // `component::check` exists.
    let cuts = crate::component::settle_links(doc, tx);
    tx.0.extend(cuts);
    // And what the variant rules owe (§15 D982): a main moved into a set takes
    // values, one moved out drops them, every variant is renamed from its values
    // — before the propagation below, so an instance still carrying a variant's
    // old name follows it — and a binding that no longer fits goes.
    let settled = crate::variant::settle(doc, tx);
    tx.0.extend(settled);
    // And last, what the edit owes the instances (§5.3d build step 3): a main's
    // change written onto every copy that still holds the main's old value —
    // after the passes above, so what they append to a main propagates too.
    let follows = propagate(doc, tx);
    tx.0.extend(follows);
}

/// The nodes `tx` takes away from the source they follow: everything under a
/// `DeleteNode`, at any depth, and every node a `SetLink` names — cut, climbed
/// past a deleted node, or relinked, its old source is not the one it follows
/// once the transaction lands. [`propagate`] writes nothing onto these.
fn leaves_its_source(doc: &Document, tx: &Transaction) -> FxHashSet<NodeId> {
    let deleted: Vec<NodeId> =
        tx.0.iter()
            .filter_map(|op| match op {
                Operation::DeleteNode { id } => Some(*id),
                _ => None,
            })
            .collect();
    let mut gone: FxHashSet<NodeId> = crate::build::subtree_nodes(doc, &deleted)
        .into_iter()
        .collect();
    gone.extend(tx.0.iter().filter_map(|op| match op {
        Operation::SetLink { id, .. } => Some(*id),
        _ => None,
    }));
    gone
}

/// The operations a transaction's **structural** edits to mains owe their
/// instances (§5.3d build step 4, §15 D979 (b)), worked out from the tree the
/// transaction leaves (applied to a scratch copy):
///
/// - **A child a main gained** is copied into every copy of its parent, at every
///   depth, each copied node linked to the one it was copied from — after the
///   counterpart of its preceding sibling, else before the counterpart of its next
///   one, else topmost. A copy that has lost the parent's counterpart gets nothing.
/// - **A child a main lost** takes each counterpart with it **only if the
///   counterpart and everything under it still equal their sources** — the same
///   fields, the same children in the same order, no layer of the instance's own
///   among them. A changed counterpart stays, as the instance's own layer
///   (`component::settle_links` cuts its link). Recursively: a deleted counterpart
///   is itself a lost child for its own copies.
/// - **A reorder** follows onto a copy whose linked children are still in the
///   main's old order, a layer of the copy's own keeping its place after the
///   sibling it followed.
/// - **A move within a main** follows onto a copy still under the counterpart of
///   the old parent — to the new parent's counterpart, **including the copy this
///   pass just made of a parent the same edit created**, so *Group selection*
///   inside a main moves each instance's own layers into a copy of the new group
///   rather than re-copying them. Where an instance has no counterpart of the new
///   parent, the move is a removal there (delete if untouched, else cut). A layer
///   moved into a main **from outside it** is a gain, copied whole.
/// - **An ungroup or *Release*** lifts an instance's counterparts out of its copy
///   of the group only where that copy is untouched, the children aside; a copy
///   the instance changed stays as a local wrapper with its children inside it,
///   still linked (§15 D1003 (2)).
///
/// A copy the transaction itself deletes is owed nothing by any arm.
///
/// The ops go out as inserts, then moves, then deletes, then reorders, and the
/// delete decisions are made after the moves: *Ungroup* lifts the children out of
/// a group before the group goes, and its copy is untouched once they have left.
///
/// Mints ids for what it copies, so it runs where an `IdSource` is: before
/// `settle_links`, which then settles any link these leave behind.
pub fn propagate_structure(
    doc: &Document,
    tx: &Transaction,
    ids: &mut crate::id::IdSource,
) -> Vec<Operation> {
    let before = doc.node_map();
    // A swapped copy's children are its swap's, never its slot's (§15 D983).
    let copies = structural_copies(doc);
    if copies.is_empty() {
        return Vec::new();
    }
    let structural = tx.0.iter().any(|op| {
        matches!(
            op,
            Operation::CreateNode { .. }
                | Operation::DeleteNode { .. }
                | Operation::InsertSubtree { .. }
                | Operation::Reparent { .. }
                | Operation::Reorder { .. }
        )
    });
    if !structural {
        return Vec::new();
    }
    let mut after = doc.clone();
    if after.apply_unchecked(tx).is_err() {
        return Vec::new();
    }
    let after_nodes = after.node_map();
    let mut out = Vec::new();

    // Parents whose children changed, among nodes that have copies.
    let mut parents: Vec<NodeId> = copies
        .keys()
        .copied()
        .filter(|p| {
            let b = before.get(p).map(|n| &n.children);
            let a = after_nodes.get(p).map(|n| &n.children);
            // A parent the edit deleted counts: its children are lost from it, and
            // the ones lifted out first (an ungroup) are moves.
            b.is_some() && b != a
        })
        .collect();
    parents.sort();

    // Lost children: delete the untouched counterparts, recursively.
    //
    // **A deleted parent loses its children only if it was itself lost from a
    // parent that stays** (§15 D984) — an ungrouped group, a deleted child of a
    // main, at any depth below one. A deleted *main* is lost from nothing: its
    // instances detach and keep their look (§15 D979 (c)), so its children are
    // not losses for them, and reading them so deleted every untouched layer of
    // every instance of a main the user deleted.
    let lost_from = |p: NodeId| -> Vec<NodeId> {
        let a: FxHashSet<NodeId> = after_nodes
            .get(&p)
            .map(|n| n.children.iter().copied().collect())
            .unwrap_or_default();
        before[&p]
            .children
            .iter()
            .copied()
            .filter(|c| !a.contains(c))
            .collect()
    };
    let mut lost: Vec<NodeId> = Vec::new();
    let mut counted: FxHashSet<NodeId> = FxHashSet::default();
    let mut pending: Vec<NodeId> = parents.clone();
    loop {
        let (now, later): (Vec<NodeId>, Vec<NodeId>) = pending
            .into_iter()
            .partition(|p| after_nodes.contains_key(p) || lost.contains(p));
        if now.is_empty() {
            break;
        }
        for p in now {
            if counted.insert(p) {
                lost.extend(lost_from(p));
            }
        }
        pending = later;
    }
    // A child moved to another parent inside the same main is a **move**, not a
    // loss — judged on the tree the transaction leaves, so a parent made by the same
    // edit counts: *Group selection* inside a main moves each instance's own
    // counterpart into a copy of the new group rather than re-copying it.
    let moved_within: FxHashMap<NodeId, NodeId> = lost
        .iter()
        .filter_map(|c| Some((*c, after_nodes.get(c)?.parent?)))
        .filter(|(c, np)| {
            main_of_node(doc, *c).is_some() && main_of_node(doc, *c) == main_of_node(&after, *np)
        })
        .collect();

    // **The ops go out as inserts, then moves, then deletes, then reorders.** A move
    // can target a copy the inserts make (the new group's), and a delete must come
    // after the moves out of what it deletes — *Ungroup* lifts the children before
    // the group goes, and deleting an untouched group copy first took the children's
    // counterparts with it (the first build: `NoSuchNode`).
    let mut deletes: Vec<NodeId> = Vec::new();
    let mut deleted: FxHashSet<NodeId> = FxHashSet::default();

    // Gained children: copy them in, recursively. Each parent's children are
    // simulated as the inserts land, so a second child gained in the same edit, and a
    // copy one level further down, are placed against what is really there. (The
    // deletes come after, so they are still in the lists here.)
    let mut sim: FxHashMap<NodeId, Vec<NodeId>> = FxHashMap::default();
    let mut new_links: FxHashMap<NodeId, NodeId> = FxHashMap::default();
    // (instance root, a new node of the main) → its copy in that instance, so a move
    // into a parent this edit made finds the parent's copy.
    let mut made: FxHashMap<(NodeId, NodeId), NodeId> = FxHashMap::default();
    let children_of = |sim: &FxHashMap<NodeId, Vec<NodeId>>, p: NodeId| -> Vec<NodeId> {
        sim.get(&p).cloned().unwrap_or_else(|| {
            after_nodes
                .get(&p)
                .map(|n| n.children.clone())
                .or_else(|| before.get(&p).map(|n| n.children.clone()))
                .unwrap_or_default()
        })
    };
    for p in &parents {
        let Some(after_p) = after_nodes.get(p) else {
            continue; // deleted: it gained nothing
        };
        let b: FxHashSet<NodeId> = before[p].children.iter().copied().collect();
        let main = main_of_node(&after, *p);
        let gained: Vec<NodeId> = after_p
            .children
            .iter()
            .copied()
            .filter(|c| !b.contains(c) && !moved_within.contains_key(c))
            .collect();
        for x in gained {
            let Some(template) = after.capture_subtree(x) else {
                continue;
            };
            // Only what is **new to this main**: a layer that already sat in it and
            // was moved (a group made around existing layers) is a move, handled
            // below, and its counterpart moves rather than being copied again. A
            // layer that came in from outside the main — dragged in from the canvas
            // or from another main — is copied whole, as a `CreateNode` is (§5.3d
            // build step 4). The first form pruned every node that existed before
            // the edit, and a layer dragged into a main never reached its instances
            // (`[X2-L1-01]`).
            let template = only_new(template, |id| {
                before.contains_key(&id) && main_of_node(doc, id) == main
            });
            if template.is_empty() {
                continue;
            }
            // (source parent, the subtree as it stands at that level)
            let mut level = vec![(*p, template)];
            while let Some((src_parent, nodes)) = level.pop() {
                for &pc in copies.get(&src_parent).into_iter().flatten() {
                    // A copy the edit itself deleted is owed nothing (`[X2-L1-02]`).
                    if deleted.contains(&pc) || !after_nodes.contains_key(&pc) {
                        continue;
                    }
                    let Some((mut copy, _)) = crate::document::remap_subtree(&nodes, ids) else {
                        continue;
                    };
                    for (c, t) in copy.iter_mut().zip(&nodes) {
                        c.make_copy_of(t.id);
                    }
                    let siblings = children_of(&sim, src_parent);
                    let kids = children_of(&sim, pc);
                    let link_of = |k: NodeId| {
                        new_links
                            .get(&k)
                            .copied()
                            .or_else(|| before.get(&k).and_then(|n| n.link))
                    };
                    let index = anchor(&siblings, nodes[0].id, &kids, |s| {
                        kids.iter().copied().find(|k| link_of(*k) == Some(s))
                    });
                    let new_root = copy[0].id;
                    new_links.insert(new_root, nodes[0].id);
                    if let Some(root) = crate::component::instance_root(doc, pc) {
                        for (c, t) in copy.iter().zip(&nodes) {
                            made.insert((root, t.id), c.id);
                        }
                    }
                    let mut placed = kids.clone();
                    placed.insert(index.min(placed.len()), new_root);
                    sim.insert(pc, placed);
                    out.push(Operation::InsertSubtree {
                        nodes: copy.clone(),
                        parent: pc,
                        index,
                    });
                    // The copy's own copies take it in turn, one level down.
                    level.push((pc, copy));
                }
            }
        }
    }

    // Moves within a main, carried down the copy chain: a counterpart still under
    // its source's old parent's counterpart moves to the new parent's counterpart —
    // a node that existed, or the copy the inserts above just made. Where an
    // instance has no counterpart of the new parent, the move is a removal there:
    // the counterpart goes if untouched and is cut loose otherwise (§5.3d).
    let target_of = |root: NodeId, src: NodeId| {
        made.get(&(root, src))
            .copied()
            .or_else(|| counterpart(doc, root, src))
    };
    let mut cut: Vec<NodeId> = Vec::new();
    let mut moved_out: FxHashSet<NodeId> = FxHashSet::default();
    let mut moves: Vec<(NodeId, NodeId, NodeId)> = moved_within
        .iter()
        .map(|(x, np)| {
            (
                *x,
                before[x].parent.expect("a lost child had a parent"),
                *np,
            )
        })
        .collect();
    moves.sort();
    // An ungroup (or a boolean's *Release*) deletes the parent it lifts out of. A
    // copy of that parent the instance changed keeps its children: what they look
    // like depends on it — a moved or faded group — and lifting them left an empty
    // unlinked shell holding the change while they drew as the main did
    // (`[R3-L5-01]`). §15 D1003 (2): the changed copy stays as a local wrapper,
    // its children inside it still linked; an untouched copy ungroups as the main
    // did. Judged ignoring the children being lifted, whose own changes travel
    // with them.
    let wrapper = |gc: NodeId, g: NodeId| -> bool {
        if after_nodes.contains_key(&g) {
            return false; // not an ungroup: the old parent stays
        }
        let lifting: FxHashSet<NodeId> = before[&gc]
            .children
            .iter()
            .copied()
            .filter(|k| {
                before
                    .get(k)
                    .and_then(|n| n.link)
                    .is_some_and(|s| moved_within.contains_key(&s))
            })
            .collect();
        !untouched(doc, gc, g, &lifting)
    };
    while let Some((x, old_parent, new_parent)) = moves.pop() {
        for &c in copies.get(&x).into_iter().flatten() {
            // A copy the edit itself deleted is owed nothing (`[X2-L1-02]`).
            if !after_nodes.contains_key(&c) {
                continue;
            }
            let Some(root) = crate::component::instance_root(doc, c) else {
                continue;
            };
            let Some(from) = before[&c].parent else {
                continue;
            };
            if Some(from) != counterpart(doc, root, old_parent) {
                continue; // the instance moved it itself
            }
            if wrapper(from, old_parent) {
                // Where it is: pin what the edit wrote on `x` at the copy's own
                // values, so the field pass carries none of it — an ungroup bakes
                // the group's transform into each child, which is the main's group
                // and not this copy's.
                out.extend(
                    tx.0.iter()
                        .filter(|op| op.overwrites() == Some(x))
                        .filter_map(|op| doc.peek(&op.retargeted(c)?)),
                );
                continue;
            }
            let Some(target) = target_of(root, new_parent) else {
                if untouched(doc, c, x, &moved_out) {
                    if deleted.insert(c) {
                        deletes.push(c);
                    }
                } else {
                    cut.push(c);
                }
                continue;
            };
            let siblings = children_of(&sim, new_parent);
            let mut kids = children_of(&sim, target);
            kids.retain(|k| *k != c);
            let link_of = |k: NodeId| {
                new_links
                    .get(&k)
                    .copied()
                    .or_else(|| before.get(&k).and_then(|n| n.link))
            };
            let index = anchor(&siblings, x, &kids, |s| {
                kids.iter().copied().find(|k| link_of(*k) == Some(s))
            });
            let mut placed = kids.clone();
            placed.insert(index.min(placed.len()), c);
            sim.insert(target, placed);
            if let Some(from) = before[&c].parent {
                let mut left = children_of(&sim, from);
                left.retain(|k| *k != c);
                sim.insert(from, left);
            }
            out.push(Operation::Reparent {
                id: c,
                new_parent: target,
                index,
            });
            moved_out.insert(c);
            moves.push((
                c,
                before[&c].parent.expect("a moved node had a parent"),
                target,
            ));
        }
    }

    // Lost children: an untouched counterpart goes with its source, recursively —
    // judged **after** the moves, ignoring what they carry out: an ungrouped
    // group's copy is untouched once its children have left it, whatever the
    // instance did to those children.
    let mut queue: Vec<NodeId> = lost
        .iter()
        .copied()
        .filter(|c| !moved_within.contains_key(c))
        .collect();
    while let Some(gone) = queue.pop() {
        for &c in copies.get(&gone).into_iter().flatten() {
            // A counterpart the edit itself deleted — selected beside its source,
            // or inside an instance deleted with it — is not deleted twice: that
            // refused the whole transaction, `NoSuchNode` (`[X2-L1-02]`).
            if deleted.contains(&c)
                || !after_nodes.contains_key(&c)
                || !untouched(doc, c, gone, &moved_out)
            {
                continue;
            }
            deleted.insert(c);
            deletes.push(c);
            queue.push(c);
        }
    }
    out.extend(deletes.iter().map(|id| Operation::DeleteNode { id: *id }));
    out.extend(
        cut.into_iter()
            .map(|id| Operation::SetLink { id, link: None }),
    );

    // Reorders: a parent whose children are the same set in a new order — carried
    // down the copy chain as the other three arms are, a copy this reorders being
    // a parent reordered in turn for its own copies. Single-level until the
    // release review: a nested copy in an outer main's instance kept the old
    // order, and from then on refused every later reorder as one the instance
    // had made itself (`[X3-L2-02]`).
    let mut reorders: Vec<(NodeId, Vec<NodeId>, Vec<NodeId>)> = parents
        .iter()
        .filter_map(|p| {
            let a = &after_nodes.get(p)?.children;
            Some((*p, before[p].children.clone(), a.clone()))
        })
        .collect();
    while let Some((p, b, a)) = reorders.pop() {
        let bs: FxHashSet<_> = b.iter().collect();
        if b.len() != a.len() || !a.iter().all(|c| bs.contains(c)) {
            continue;
        }
        for &pc in copies.get(&p).into_iter().flatten() {
            if !after_nodes.contains_key(&pc) {
                continue;
            }
            let kids = &before[&pc].children;
            let linked: Vec<NodeId> = kids
                .iter()
                .filter_map(|k| before.get(k).and_then(|n| n.link))
                .filter(|s| bs.contains(s))
                .collect();
            let old: Vec<NodeId> = b.iter().copied().filter(|s| linked.contains(s)).collect();
            if linked != old {
                continue; // the instance reordered them itself
            }
            let new: Vec<NodeId> = a.iter().copied().filter(|s| linked.contains(s)).collect();
            // The copy's linked children take the main's new order in the slots
            // they occupy; its own layers stay where they are.
            let mut target = kids.clone();
            let slots: Vec<usize> = (0..kids.len())
                .filter(|&i| {
                    before
                        .get(&kids[i])
                        .and_then(|n| n.link)
                        .is_some_and(|s| bs.contains(&s))
                })
                .collect();
            for (slot, src) in slots.into_iter().zip(&new) {
                if let Some(k) = kids
                    .iter()
                    .find(|k| before.get(k).and_then(|n| n.link) == Some(*src))
                {
                    target[slot] = *k;
                }
            }
            if target == *kids {
                continue;
            }
            for (i, id) in target.iter().enumerate() {
                out.push(Operation::Reorder { id: *id, index: i });
            }
            reorders.push((pc, kids.clone(), target));
        }
    }
    out
}

/// `template` (a captured subtree, root first) without the nodes `existed` admits
/// and anything under them — what an edit **made**, as opposed to what it moved in.
/// Children lists are pruned to match, so `remap_subtree` still accepts the result.
pub(crate) fn only_new(
    template: Vec<crate::node::Node>,
    existed: impl Fn(NodeId) -> bool,
) -> Vec<crate::node::Node> {
    let mut dropped: FxHashSet<NodeId> = FxHashSet::default();
    let mut kept: Vec<crate::node::Node> = Vec::new();
    for n in template {
        let parent_dropped = n.parent.is_some_and(|p| dropped.contains(&p));
        if existed(n.id) || parent_dropped {
            dropped.insert(n.id);
        } else {
            kept.push(n);
        }
    }
    for n in &mut kept {
        n.children.retain(|c| !dropped.contains(c));
    }
    kept
}

/// The main a node sits in (the nearest main at or above it), if any.
fn main_of_node(doc: &Document, id: NodeId) -> Option<NodeId> {
    let mut at = doc.get(id);
    while let Some(n) = at {
        if n.component {
            return Some(n.id);
        }
        at = n.parent.and_then(|p| doc.get(p));
    }
    None
}

/// The node of the instance rooted at `root` that is linked to `src` — `root`
/// itself when `src` is its source, a swapped root's being its swap (§15 D983).
pub(crate) fn counterpart(doc: &Document, root: NodeId, src: NodeId) -> Option<NodeId> {
    let r = doc.get(root)?;
    if r.swap.or(r.link) == Some(src) {
        return Some(root);
    }
    crate::build::subtree_nodes(doc, &[root])
        .into_iter()
        .find(|id| doc.get(*id).and_then(|n| n.link) == Some(src))
}

/// Where a node placed at `x` among `main_siblings` lands among `copy_children`:
/// after the counterpart of its nearest preceding sibling, else before the
/// counterpart of its nearest following one, else at the end (topmost).
pub(crate) fn anchor(
    main_siblings: &[NodeId],
    x: NodeId,
    copy_children: &[NodeId],
    counterpart_of: impl Fn(NodeId) -> Option<NodeId>,
) -> usize {
    let at = main_siblings.iter().position(|c| *c == x).unwrap_or(0);
    let find =
        |s: NodeId| counterpart_of(s).and_then(|c| copy_children.iter().position(|k| *k == c));
    if let Some(i) = main_siblings[..at].iter().rev().find_map(|s| find(*s)) {
        return i + 1;
    }
    if let Some(i) = main_siblings[at + 1..].iter().find_map(|s| find(*s)) {
        return i;
    }
    copy_children.len()
}

/// Whether the counterpart `copy` and everything under it still equal `src` and
/// its subtree — the test for deleting it with its source (§15 D979 (b)).
///
/// Children in `moved_out` — counterparts this same pass is carrying elsewhere —
/// are left out of the comparison, with their sources: what an ungroup lifts out
/// of a group no longer decides whether the group's copy is untouched.
pub(crate) fn untouched(
    doc: &Document,
    copy: NodeId,
    src: NodeId,
    moved_out: &FxHashSet<NodeId>,
) -> bool {
    let (Some(c), Some(s)) = (doc.get(copy), doc.get(src)) else {
        return false;
    };
    let link = |id: NodeId| doc.get(id).and_then(|n| n.link);
    let c_kids: Vec<NodeId> = c
        .children
        .iter()
        .copied()
        .filter(|k| !moved_out.contains(k))
        .collect();
    let s_kids: Vec<NodeId> = s
        .children
        .iter()
        .copied()
        .filter(|sk| {
            !c.children
                .iter()
                .any(|ck| moved_out.contains(ck) && link(*ck) == Some(*sk))
        })
        .collect();
    let same = c.kind == s.kind
        && c.transform == s.transform
        && c.name == s.name
        && c.visible == s.visible
        && c.opacity == s.opacity
        && c.clip == s.clip
        && c.mask == s.mask
        && c.mask_mode == s.mask_mode
        && c.fill_rule == s.fill_rule
        && c.paint == s.paint
        && c.effects == s.effects
        && c.pivot == s.pivot
        && c.exports == s.exports
        && c.grids == s.grids
        && c.insets == s.insets
        && c.display == s.display
        && c.item == s.item
        // A swap is an override (§15 D983): a swapped copy is never untouched.
        && c.swap.is_none()
        && c_kids.len() == s_kids.len();
    same && c_kids
        .iter()
        .zip(&s_kids)
        .all(|(cc, sc)| link(*cc) == Some(*sc) && untouched(doc, *cc, *sc, moved_out))
}

/// Whether `id` is linked **straight to a main** — an instance placed by itself,
/// whose placement is its own. Not every instance root: a nested copy inside an
/// outer main is an instance root by its chain, and its place inside that main is
/// the main's to move, so its outer instances follow it (`check`'s and
/// `settle_links`' predicate — the first build asked `instance_root` here, and a
/// nested instance moved inside its outer main stayed put in every outer instance).
pub(crate) fn linked_to_main(doc: &Document, id: NodeId) -> bool {
    doc.get(id)
        .and_then(|n| n.link)
        .and_then(|s| doc.get(s))
        .is_some_and(|s| s.component)
}

fn same_content(doc: &Document, a: NodeId, b: NodeId) -> bool {
    let content = |id: NodeId| match doc.get(id).map(|n| &n.kind) {
        Some(NodeKind::Text { content, .. }) => Some(content.clone()),
        _ => None,
    };
    content(a).is_some() && content(a) == content(b)
}

/// The edit a copy `m` takes, given the main's `new` edit, the main's `old` value
/// and the copy's `cur` value (both as inverse operations), or `None` when the copy
/// keeps everything it has.
pub(crate) fn follow(
    new: &Operation,
    old: &Operation,
    cur: &Operation,
    m: NodeId,
) -> Option<Operation> {
    use Operation as O;
    match (new, old, cur) {
        (
            O::SetFills { fills: n, .. },
            O::SetFills { fills: o, .. },
            O::SetFills { fills: c, .. },
        ) => changed(c, items(o, n, c)).map(|fills| O::SetFills { id: m, fills }),
        (
            O::SetStrokes { strokes: n, .. },
            O::SetStrokes { strokes: o, .. },
            O::SetStrokes { strokes: c, .. },
        ) => changed(c, items(o, n, c)).map(|strokes| O::SetStrokes { id: m, strokes }),
        (
            O::SetEffects { effects: n, .. },
            O::SetEffects { effects: o, .. },
            O::SetEffects { effects: c, .. },
        ) => changed(c, items(o, n, c)).map(|effects| O::SetEffects { id: m, effects }),
        (
            O::SetExports { exports: n, .. },
            O::SetExports { exports: o, .. },
            O::SetExports { exports: c, .. },
        ) => changed(c, items(o, n, c)).map(|exports| O::SetExports { id: m, exports }),
        (
            O::SetLayoutGrids { grids: n, .. },
            O::SetLayoutGrids { grids: o, .. },
            O::SetLayoutGrids { grids: c, .. },
        ) => changed(c, items(o, n, c)).map(|grids| O::SetLayoutGrids { id: m, grids }),
        (
            O::SetTextStyle {
                style: n,
                spans: ns,
                ..
            },
            O::SetTextStyle {
                style: o,
                spans: os,
                ..
            },
            O::SetTextStyle {
                style: c,
                spans: cs,
                ..
            },
        ) => {
            let style = fields(o, n, c);
            let spans = ns.clone().filter(|_| os == cs);
            (style != *c || spans.as_ref().is_some_and(|s| Some(s) != cs.as_ref())).then(|| {
                O::SetTextStyle {
                    id: m,
                    style,
                    spans,
                }
            })
        }
        (
            O::SetParagraphStyle {
                paragraph: n,
                spans: ns,
                ..
            },
            O::SetParagraphStyle {
                paragraph: o,
                spans: os,
                ..
            },
            O::SetParagraphStyle {
                paragraph: c,
                spans: cs,
                ..
            },
        ) => {
            let paragraph = fields(o, n, c);
            let spans = ns.clone().filter(|_| os == cs);
            (paragraph != *c || spans.as_ref().is_some_and(|s| Some(s) != cs.as_ref())).then(|| {
                O::SetParagraphStyle {
                    id: m,
                    paragraph,
                    spans,
                }
            })
        }
        (
            O::SetBlockStyle { block: n, .. },
            O::SetBlockStyle { block: o, .. },
            O::SetBlockStyle { block: c, .. },
        ) => changed(c, fields(o, n, c)).map(|block| O::SetBlockStyle { id: m, block }),
        (
            O::SetInsets { insets: n, .. },
            O::SetInsets { insets: o, .. },
            O::SetInsets { insets: c, .. },
        ) => changed(c, fields(o, n, c)).map(|insets| O::SetInsets { id: m, insets }),
        (
            O::SetDisplay { display: n, .. },
            O::SetDisplay { display: o, .. },
            O::SetDisplay { display: c, .. },
        ) => changed(c, fields(o, n, c)).map(|display| O::SetDisplay { id: m, display }),
        (
            O::SetLayoutItem { item: n, .. },
            O::SetLayoutItem { item: o, .. },
            O::SetLayoutItem { item: c, .. },
        ) => changed(c, fields(o, n, c)).map(|item| O::SetLayoutItem { id: m, item }),
        // Whole value: the copy follows when its old value is the main's old value,
        // and only if the edit changed anything.
        _ => {
            let same_old = cur.retargeted(NodeId { actor: 0, seq: 0 })
                == old.retargeted(NodeId { actor: 0, seq: 0 });
            let same_new = new.retargeted(NodeId { actor: 0, seq: 0 })
                == old.retargeted(NodeId { actor: 0, seq: 0 });
            (same_old && !same_new).then(|| new.retargeted(m)).flatten()
        }
    }
}

fn changed<T: PartialEq>(cur: &T, next: T) -> Option<T> {
    (next != *cur).then_some(next)
}

/// `cur` with every field `old` → `new` changed where `cur` still held `old`'s.
///
/// Through JSON, so one function serves every struct payload: two objects of the
/// **same shape** (the same keys — the same enum variant, the same struct) are
/// merged key by key, recursively; anything else is one value, followed whole.
/// That is what keeps a merge from mixing two variants: a brush that went from
/// solid to linear is replaced whole or not at all. Exact float comparison is
/// sound here because the values compared were written by the same code from the
/// same source; `serde_json` round-trips an `f64` exactly.
fn fields<T: Serialize + DeserializeOwned + Clone + PartialEq>(old: &T, new: &T, cur: &T) -> T {
    let (Ok(o), Ok(n), Ok(c)) = (
        serde_json::to_value(old),
        serde_json::to_value(new),
        serde_json::to_value(cur),
    ) else {
        return cur.clone();
    };
    serde_json::from_value(merge(&o, &n, &c)).unwrap_or_else(|_| cur.clone())
}

fn merge(old: &Value, new: &Value, cur: &Value) -> Value {
    merge_present(Some(old), Some(new), Some(cur)).unwrap_or(Value::Null)
}

/// [`merge`] with a field's **absence** as a value of its own. Most of these
/// structs skip a field at its default when serialized (`skip_serializing_if`), so
/// the same struct writes different keys depending on what is at its default — a
/// side pinned or not, an item given growth or not. Merging over the **union** of
/// keys, with "absent" compared like any value and written back as absence, needs
/// no knowledge of what any default is: the copy's missing key equals the main's
/// old missing key, so it takes the main's new one, and a key the main dropped is
/// dropped from a copy that still held the main's old value.
///
/// ⚠️ **The first build recursed only when all three had the same keys**, so any
/// field at or moving to its default made the struct compare whole and a copy with
/// one override stopped following every other field — the common case, and
/// `arch-scribe` found it by reading the merge against D979 (a). A test struct that
/// skips nothing could not see it; `fields_follow_with_skipped_defaults` uses one
/// that does.
///
/// **An enum is never mixed**: an object keyed by a variant name (serde's external
/// tagging, an upper-case key) whose variant changed is one value, followed whole,
/// as is one whose internal `"type"` tag changed. Anything this still mixes into an
/// invalid shape fails to deserialize, and [`fields`] then keeps the copy's value —
/// the safe direction.
fn merge_present(old: Option<&Value>, new: Option<&Value>, cur: Option<&Value>) -> Option<Value> {
    if old == new {
        return cur.cloned();
    }
    if let (Some(Value::Object(o)), Some(Value::Object(n)), Some(Value::Object(c))) =
        (old, new, cur)
        && !variant_changed(o, n)
        && !variant_changed(o, c)
    {
        let mut out = serde_json::Map::new();
        let keys: std::collections::BTreeSet<&String> =
            o.keys().chain(n.keys()).chain(c.keys()).collect();
        for k in keys {
            if let Some(v) = merge_present(o.get(k), n.get(k), c.get(k)) {
                out.insert(k.clone(), v);
            }
        }
        return Some(Value::Object(out));
    }
    if old == cur {
        new.cloned()
    } else {
        cur.cloned()
    }
}

/// Whether two objects are different variants of one enum — different single
/// upper-case keys, or different internal `"type"` tags.
pub(crate) fn variant_changed(
    a: &serde_json::Map<String, Value>,
    b: &serde_json::Map<String, Value>,
) -> bool {
    let tagged = |m: &serde_json::Map<String, Value>| {
        (m.len() == 1)
            .then(|| {
                m.keys()
                    .next()
                    .filter(|k| k.starts_with(|c: char| c.is_ascii_uppercase()))
            })
            .flatten()
            .cloned()
    };
    let external = match (tagged(a), tagged(b)) {
        (Some(x), Some(y)) => x != y,
        (Some(_), None) | (None, Some(_)) => true,
        (None, None) => false,
    };
    external || a.get("type").is_some_and(|t| b.get("type") != Some(t))
}

/// A keyed list's edit `old` → `new`, carried onto `cur` item by item (§15 D980):
/// - an item in both has its fields followed one by one ([`fields`]);
/// - an item the main removed goes from `cur` only if `cur`'s still equals the
///   main's old one — instance-side work is never destroyed;
/// - an item the main added is inserted after the counterpart of the item before
///   it in the main's list, or first when there is none;
/// - a reorder of the items both lists still share follows only if `cur` still has
///   them in the main's old order.
fn items<T: Serialize + DeserializeOwned + Clone + PartialEq>(
    old: &[Keyed<T>],
    new: &[Keyed<T>],
    cur: &[Keyed<T>],
) -> Vec<Keyed<T>> {
    let find = |list: &[Keyed<T>], id: ItemId| list.iter().position(|k| k.id == id);
    let mut out: Vec<Keyed<T>> = cur.to_vec();
    // Fields of the items both keep.
    for n in new {
        if let (Some(o), Some(at)) = (find(old, n.id), find(&out, n.id)) {
            out[at].value = fields(&old[o].value, &n.value, &out[at].value);
        }
    }
    // Removed in the main: gone from the copy where it was still the main's.
    out.retain(|c| {
        find(new, c.id).is_some() || find(old, c.id).is_none_or(|o| old[o].value != c.value)
    });
    // Added in the main: after its predecessor's counterpart.
    for (i, n) in new.iter().enumerate() {
        if find(old, n.id).is_some() || find(&out, n.id).is_some() {
            continue;
        }
        let at = new[..i]
            .iter()
            .rev()
            .find_map(|p| find(&out, p.id))
            .map_or(0, |p| p + 1);
        out.insert(at, n.clone());
    }
    // Reorder of the shared items, if the copy still has the main's old order —
    // read over the **survivors**, the items in both of the main's lists that the
    // copy still has, which is the children's rule (§5.3d): a copy that deleted one
    // of them still follows a reorder of the rest.
    let shared: Vec<ItemId> = old
        .iter()
        .map(|k| k.id)
        .filter(|id| find(new, *id).is_some() && find(&out, *id).is_some())
        .collect();
    let new_order: Vec<ItemId> = new
        .iter()
        .map(|k| k.id)
        .filter(|id| shared.contains(id))
        .collect();
    let slots: Vec<usize> = out
        .iter()
        .enumerate()
        .filter(|(_, k)| shared.contains(&k.id))
        .map(|(i, _)| i)
        .collect();
    let in_cur: Vec<ItemId> = slots.iter().map(|&i| out[i].id).collect();
    if shared != new_order && in_cur == shared {
        let moved: Vec<Keyed<T>> = new_order
            .iter()
            .map(|id| out[find(&out, *id).expect("shared and present")].clone())
            .collect();
        for (slot, item) in slots.into_iter().zip(moved) {
            out[slot] = item;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::keyed_by_position;

    #[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
    struct S {
        a: u32,
        b: u32,
    }

    /// Field by field: the copy changed `b`, the main changes `a` — the copy takes
    /// `a` and keeps its `b`. Flip: comparing the struct whole keeps all of `cur`.
    #[test]
    fn a_struct_follows_field_by_field() {
        let (old, new, cur) = (S { a: 1, b: 1 }, S { a: 2, b: 1 }, S { a: 1, b: 9 });
        assert_eq!(fields(&old, &new, &cur), S { a: 2, b: 9 });
        // And a field the copy overrode is kept when the main changes it too.
        assert_eq!(fields(&old, &S { a: 1, b: 2 }, &cur), cur);
    }

    /// Two variants are never mixed: a value whose shape changed follows whole or
    /// not at all.
    #[test]
    fn an_enum_that_changed_variant_follows_whole() {
        #[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
        enum E {
            P { x: u32 },
            Q { y: u32 },
        }
        let (old, new) = (E::P { x: 1 }, E::Q { y: 5 });
        assert_eq!(fields(&old, &new, &E::P { x: 1 }), new);
        assert_eq!(fields(&old, &new, &E::P { x: 3 }), E::P { x: 3 });
    }

    /// Item by item: a field edit lands on the matching item, an addition after its
    /// predecessor's counterpart, a removal only where the copy still had the
    /// main's item, and a local item the copy added stays.
    #[test]
    fn a_list_follows_item_by_item() {
        let old = keyed_by_position([S { a: 1, b: 0 }, S { a: 2, b: 0 }]);
        let local = Keyed::new(ItemId::positional(9), S { a: 7, b: 7 });
        let mut cur = old.clone();
        cur[1].value.b = 5; // the copy's own change to item 1
        cur.push(local.clone());
        // Main: edit item 0's `a`, remove item 1, add a new item after item 0.
        let added = Keyed::new(ItemId::positional(3), S { a: 4, b: 0 });
        let new = vec![old[0].map(|v| S { a: 10, ..v.clone() }), added.clone()];
        let out = items(&old, &new, &cur);
        assert_eq!(out[0].value, S { a: 10, b: 0 }, "the field follows");
        assert_eq!(out[1], added, "added after its predecessor's counterpart");
        assert!(
            out.iter().any(|k| k.id == old[1].id),
            "item 1 stays: the copy had changed it"
        );
        assert!(out.contains(&local), "the copy's own item stays");
    }

    /// A reorder follows only onto a copy that still has the old order.
    #[test]
    fn a_reorder_follows_only_the_old_order() {
        let old = keyed_by_position([1u32, 2, 3]);
        let new = vec![old[2], old[0], old[1]];
        assert_eq!(items(&old, &new, &old), new);
        let mine = vec![old[1], old[0], old[2]];
        assert_eq!(items(&old, &new, &mine), mine, "the copy's own order stays");
    }

    /// A struct that skips its defaults, as `Insets`, `LayoutItem` and the text
    /// styles do.
    #[derive(Clone, Debug, Default, PartialEq, Serialize, serde::Deserialize)]
    struct Skips {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        a: Option<u32>,
        #[serde(default, skip_serializing_if = "is_zero")]
        b: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        c: Option<u32>,
    }
    fn is_zero(v: &u32) -> bool {
        *v == 0
    }

    /// The case the first build got wrong: the copy overrode `c`, the main pins `a`
    /// (from its default) and drops `b` (to its default) — the copy takes both and
    /// keeps its `c`. Flip: recursing only on identical key sets keeps all of `cur`.
    #[test]
    fn fields_follow_with_skipped_defaults() {
        let old = Skips {
            b: 3,
            ..Default::default()
        };
        let new = Skips {
            a: Some(7),
            ..Default::default()
        };
        let cur = Skips {
            b: 3,
            c: Some(9),
            ..Default::default()
        };
        assert_eq!(
            fields(&old, &new, &cur),
            Skips {
                a: Some(7),
                b: 0,
                c: Some(9)
            }
        );
    }

    /// **The rule as a property** — the build order's test for step 3: after a main
    /// edit, each field of a copy equals the main's new value **if and only if** it
    /// equalled the main's old value (or already equalled the new one), and is
    /// otherwise unchanged. Over every combination of three values per field, at
    /// and away from the skipped defaults.
    #[test]
    fn every_field_follows_iff_it_held_the_old_value() {
        let vals = [None, Some(1u32), Some(2)];
        let bs = [0u32, 1, 2];
        let mk = |a: usize, b: usize, c: usize| Skips {
            a: vals[a],
            b: bs[b],
            c: vals[c],
        };
        let mut cases = 0;
        for o in 0..27 {
            for n in 0..27 {
                for c in 0..27 {
                    let split = |i: usize| (i / 9, (i / 3) % 3, i % 3);
                    let ((oa, ob, oc), (na, nb, nc), (ca, cb, cc)) = (split(o), split(n), split(c));
                    let (old, new, cur) = (mk(oa, ob, oc), mk(na, nb, nc), mk(ca, cb, cc));
                    let got = fields(&old, &new, &cur);
                    fn expect<V: PartialEq>(o: V, n: V, c: V) -> V {
                        if c == o { n } else { c }
                    }
                    assert_eq!(
                        got,
                        Skips {
                            a: expect(old.a, new.a, cur.a),
                            b: expect(old.b, new.b, cur.b),
                            c: expect(old.c, new.c, cur.c),
                        },
                        "old {old:?} new {new:?} cur {cur:?}"
                    );
                    cases += 1;
                }
            }
        }
        assert_eq!(cases, 27 * 27 * 27);
    }

    /// A reorder is read over the survivors: a copy that deleted one shared item
    /// still follows the reorder of the rest.
    #[test]
    fn a_reorder_follows_over_the_survivors() {
        let old = keyed_by_position([1u32, 2, 3]);
        let new = vec![old[2], old[1], old[0]];
        let cur = vec![old[0], old[2]]; // the copy deleted item 2
        assert_eq!(items(&old, &new, &cur), vec![old[2], old[0]]);
    }
}
