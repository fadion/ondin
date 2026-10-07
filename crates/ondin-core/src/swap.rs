//! Instance swap — a nested copy showing another main's contents in its slot
//! (§15 D983).
//!
//! **The link keeps the slot, the swap brings the contents.** A copy inside an
//! outer instance is linked to its counterpart in the outer main (§5.3d), and
//! that link is what places it, orders it among its siblings and scopes its
//! resets. A swap is a second reference beside it, [`crate::Node::swap`], naming
//! the main whose contents the copy shows instead: its children are copies of
//! that main's, linked to that main's nodes, and its own fields follow that
//! main's root — **all but the slot's**: where it sits (transform, insets, layout
//! item), its size and whether it shows ([`is_slot_field`]), which keep following
//! the link (the maintainer's rulings (3), §15 D983).
//!
//! **A swap is one field, and the rewrite is the commit's.** `SetSwap` writes
//! the reference alone; [`settle`] — a commit-time pass, `variant::settle`'s
//! shape — sees it and rewrites the copy's children **in place** from the
//! contents it showed to the ones it shows now, matched by name path with every
//! override carried (`crate::variant::rewrite`, the variant switch's own
//! machinery). So the picker, a reset (whose `SetSwap` back to `None` is an
//! override's reset like any other) and a nested copy's variant switch all get
//! the same rewrite, and nothing that writes a swap has to know how. Undo needs
//! nothing: it replays the committed transaction, rewrite included.
//!
//! **A copy of a swapped copy follows it, one link up**, and carries no swap of
//! its own unless it overrides one: the rewrite reaches it as ordinary field and
//! structural propagation, ids kept.

use crate::document::Document;
use crate::id::{IdSource, NodeId};
use crate::node::Node;
use crate::op::{GeometryPatch, Operation, Transaction};
use rustc_hash::{FxHashMap, FxHashSet};

/// The node a copy takes its **contents** from — its children's sources and
/// every field but the slot's: its swap, else its link. `None` for a node that is
/// neither.
pub fn content_source(doc: &Document, id: NodeId) -> Option<NodeId> {
    let n = doc.get(id)?;
    n.swap.or(n.link)
}

/// Whether `op` writes a field a swapped copy keeps from its **slot** — its
/// placement and visibility (`propagate::is_placement`, the four an instance root
/// keeps as its own) and its size (§15 D983 (3)).
pub fn is_slot_field(op: &Operation) -> bool {
    crate::propagate::is_placement(op)
        || matches!(
            op,
            Operation::SetGeometry {
                geometry: GeometryPatch::Size(_),
                ..
            }
        )
}

/// The main an instance root **shows** — the end of its chain of links, a swap
/// taking the chain's place wherever one sits on it. `None` for a node that is
/// not part of an instance.
pub fn shown_main(nodes: &FxHashMap<NodeId, Node>, id: NodeId) -> Option<NodeId> {
    let mut at = nodes.get(&id)?;
    for _ in 0..=nodes.len() {
        if at.component {
            return Some(at.id);
        }
        if let Some(s) = at.swap {
            return nodes.get(&s).filter(|s| s.component).map(|s| s.id);
        }
        at = nodes.get(&at.link?)?;
    }
    None
}

/// Whether `root` can take a swap: a **nested copy inside an outer instance** —
/// an instance root by its chain of links, linked to a node that is not itself a
/// main. A root linked straight to a main changes main by relinking (the variant
/// switch's door, §15 D983's second round), never by a swap.
pub fn can_swap(doc: &Document, root: NodeId) -> bool {
    let nodes = doc.node_map();
    let Some(n) = nodes.get(&root) else {
        return false;
    };
    let Some(src) = n.link.and_then(|l| nodes.get(&l)) else {
        return false;
    };
    !n.component && !src.component && crate::component::is_instance_root(nodes, n)
}

/// The main `root` shows **with no swap of its own** — what its slot gives it,
/// one link up. Choosing it in the picker clears the swap rather than storing a
/// swap that names what the link already brings.
pub fn slot_main(doc: &Document, root: NodeId) -> Option<NodeId> {
    let link = doc.get(root)?.link?;
    shown_main(doc.node_map(), link)
}

/// Whether swapping `root` to `main` is allowed: `root` [`can_swap`], `main` is a
/// main of the **same kind** as `root` — a frame for a frame, a group for a group
/// (the session's, §15 D983's amendment: no operation turns a layer into another
/// kind, and the copy's id has to survive the swap for its own copies to follow
/// it) — and the swap would not put a main inside itself.
pub fn can_swap_to(doc: &Document, root: NodeId, main: NodeId) -> bool {
    let (Some(r), Some(m)) = (doc.get(root), doc.get(main)) else {
        return false;
    };
    m.component
        && can_swap(doc, root)
        && std::mem::discriminant(&r.kind) == std::mem::discriminant(&m.kind)
        && !would_cycle(doc, root, main)
}

/// Whether showing `main` at `root` would put a main inside itself: `root` sits
/// in a main that `main` already reaches, through the instances inside it.
fn would_cycle(doc: &Document, root: NodeId, main: NodeId) -> bool {
    let nodes = doc.node_map();
    let parent = |id: NodeId| nodes.get(&id).and_then(|n| n.parent);
    let Some(outer) =
        std::iter::successors(Some(root), |a| parent(*a)).find(|a| nodes[a].component)
    else {
        return false;
    };
    let uses = crate::component::uses(nodes);
    let mut stack = vec![main];
    let mut seen: FxHashSet<NodeId> = FxHashSet::default();
    while let Some(m) = stack.pop() {
        if m == outer {
            return true;
        }
        if seen.insert(m) {
            stack.extend(uses.get(&m).into_iter().flatten().copied());
        }
    }
    false
}

/// **Swap** the nested copy `root` to show `main` (§15 D983) — or back to its
/// slot's own, when `main` is what the slot gives it. Just the field: the commit
/// rewrites the children ([`settle`]). `None` unless [`can_swap_to`], or when it
/// already shows `main`.
pub fn swap(doc: &Document, root: NodeId, main: NodeId) -> Option<Transaction> {
    if !can_swap_to(doc, root, main) {
        return None;
    }
    let next = (slot_main(doc, root) != Some(main)).then_some(main);
    (doc.get(root)?.swap != next).then(|| {
        Transaction(vec![Operation::SetSwap {
            id: root,
            swap: next,
        }])
    })
}

/// The mains a swap at `root` offers, filtered by a property's `filter` — a
/// **prefix of the name**, compared without case and with no separator chosen
/// (§15 D983 (5)) — and by the picker's `search`, anywhere in the name, also
/// without case. Every main when both are empty. Only mains [`can_swap_to`]
/// accepts, the one `root` shows now included; sorted by name, then id.
pub fn options(doc: &Document, root: NodeId, filter: &str, search: &str) -> Vec<NodeId> {
    let filter = filter.trim().to_lowercase();
    let search = search.trim().to_lowercase();
    let mut out: Vec<&Node> = doc
        .node_map()
        .values()
        .filter(|n| n.component)
        .filter(|n| {
            let name = n.name.to_lowercase();
            name.starts_with(&filter) && name.contains(&search)
        })
        .filter(|n| can_swap_to(doc, root, n.id))
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    out.into_iter().map(|n| n.id).collect()
}

/// A swap property's suggested filter (§15 D983 (5)): the name of the main the
/// slot shows, less its last segment — `Button - Icons - Star` gives
/// `Button - Icons`. The separator is guessed here and only here: the last
/// character that is neither alphanumeric nor a space, cut with any more of that
/// same character and the spaces around it — so `Button -- Star` gives `Button`
/// and `Icons (v2) / Star` keeps its `)`, while a mixed run keeps all but its
/// last kind (`Icons -/ Star` gives `Icons -`). Empty when the name has none.
pub fn suggested_filter(name: &str) -> String {
    let trimmed = name.trim_end();
    let Some(cut) = trimmed
        .char_indices()
        .rev()
        .find(|(_, c)| !c.is_alphanumeric() && !c.is_whitespace())
        .map(|(i, _)| i)
    else {
        return String::new();
    };
    // The whole run, not the one character found: `Button -- Star` is
    // `Button`, never `Button -` (`arch-scribe` read the first build's cut). Of
    // that separator and spaces only, so `Icons (v2) / Star` keeps its `)`.
    let sep = trimmed[cut..].chars().next().expect("found at `cut`");
    trimmed[..cut]
        .trim_end_matches(|c: char| c.is_whitespace() || c == sep)
        .to_string()
}

/// The rewrites a transaction's swaps owe (§15 D983): for every node whose
/// contents source changes — a swap set, changed or cleared — its children and
/// fields rewritten **in place** from what it showed to what it shows, by
/// `crate::variant::rewrite`. Read on the tree the transaction leaves, outermost
/// first, each rewrite applied before the next is read. Mints ids for the layers
/// a swap brings in, so it runs where an `IdSource` is — before
/// `propagate_structure`, which then carries the rewrite to the copy's own copies.
///
/// **Nothing else writes these rewrites**, so this is the only place one is made
/// and a transaction holding a swap is expanded exactly once. Empty when the
/// transaction holds no `SetSwap` and no `SetLink`.
///
/// **A swap left naming what its slot shows is cleared** (§15 D1003 (4)): an
/// override is a difference (D979), so a copy whose slot came to show its swap's
/// main — the slot swapped, or relinked by a variant switch in the outer main —
/// follows its slot again, rather than counting an override that changes nothing
/// and staying put through the slot's later swaps (`[X5-L2-01]`). Only
/// [`swap()`] kept one from being stored.
///
/// **A copy is rewritten after the swaps of everything it copies** — its
/// ancestors and its link chain's — not merely outermost first: swapping a
/// nested copy and a copy of it in one transaction (*Select all instances*, then
/// the swap property) rewrote the copy-of-copy against its slot's new contents
/// while still measuring it from the old, and left it a cut layer, a "removed"
/// child and a redundant swap (`[X5-L1-02]`). With the slot first, the copy's
/// swap is found redundant and it follows the slot through the structural and
/// field passes.
///
/// A layer the outer main added to the slot carries across a swap, at its place
/// among its siblings (§15 D1003 (3), `variant::rewrite`).
pub fn settle(doc: &Document, tx: &Transaction, ids: &mut IdSource) -> Vec<Operation> {
    let mut swapped: Vec<NodeId> =
        tx.0.iter()
            .filter_map(|op| match op {
                Operation::SetSwap { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
    let relinks =
        tx.0.iter()
            .any(|op| matches!(op, Operation::SetLink { .. }));
    if swapped.is_empty() && !relinks {
        return Vec::new();
    }
    // Any swap may have become redundant when a slot changes, so every swapped
    // node is a candidate — none in a document without swaps.
    if !swapped.is_empty() || doc.node_map().values().any(|n| n.swap.is_some()) {
        swapped.extend(
            doc.node_map()
                .values()
                .filter(|n| n.swap.is_some())
                .map(|n| n.id),
        );
    }
    if swapped.is_empty() {
        return Vec::new();
    }
    let mut scratch = doc.clone();
    if scratch.apply_unchecked(tx).is_err() {
        return Vec::new(); // `apply` will refuse it with the reason
    }
    swapped.retain(|id| scratch.get(*id).is_some());
    swapped.sort();
    swapped.dedup();
    let mut out = Vec::new();
    for r in after_what_they_copy(&scratch, swapped) {
        if let Some(s) = scratch.get(r).and_then(|n| n.swap)
            && slot_main(&scratch, r) == Some(s)
        {
            let clear = Transaction(vec![Operation::SetSwap { id: r, swap: None }]);
            if scratch.apply_unchecked(&clear).is_err() {
                return out;
            }
            out.extend(clear.0);
        }
        let from = content_source(doc, r);
        let to = content_source(&scratch, r);
        let (Some(from), Some(to)) = (from, to) else {
            continue;
        };
        if from == to || scratch.get(from).is_none() || scratch.get(to).is_none() {
            continue;
        }
        let ops =
            crate::variant::rewrite(&scratch, r, from, to, ids, crate::variant::Rewrite::Swap)
                .unwrap_or_default();
        if ops.is_empty() {
            continue;
        }
        if scratch.apply_unchecked(&Transaction(ops.clone())).is_err() {
            return out;
        }
        out.extend(ops);
    }
    out
}

/// `nodes` ordered so each comes after every other one it copies from — an
/// ancestor of it, or of any node on its link chain — ties by depth, then id.
/// [`settle`]'s order; a cycle (which `component::check` refuses) falls back to
/// the remaining order.
fn after_what_they_copy(doc: &Document, nodes: Vec<NodeId>) -> Vec<NodeId> {
    let parent = |id: NodeId| doc.get(id).and_then(|n| n.parent);
    let depth = |id: NodeId| std::iter::successors(Some(id), |a| parent(*a)).count();
    let set: FxHashSet<NodeId> = nodes.iter().copied().collect();
    let deps = |r: NodeId| -> FxHashSet<NodeId> {
        let mut out = FxHashSet::default();
        let mut chain = Some(r);
        for _ in 0..=doc.node_map().len() {
            let Some(at) = chain else { break };
            for a in std::iter::successors(Some(at), |a| parent(*a)) {
                if a != r && set.contains(&a) {
                    out.insert(a);
                }
            }
            chain = doc.get(at).and_then(|n| n.link);
        }
        out
    };
    let mut pending: Vec<(NodeId, FxHashSet<NodeId>)> =
        nodes.iter().map(|r| (*r, deps(*r))).collect();
    pending.sort_by_key(|(id, _)| (depth(*id), *id));
    let mut done: FxHashSet<NodeId> = FxHashSet::default();
    let mut out = Vec::with_capacity(pending.len());
    while !pending.is_empty() {
        let at = pending
            .iter()
            .position(|(_, d)| d.iter().all(|x| done.contains(x)))
            .unwrap_or(0);
        let (r, _) = pending.remove(at);
        done.insert(r);
        out.push(r);
    }
    out
}

/// The link and swap a copy lands with when its link has just been moved to
/// `to` — by a climb past deleted nodes, a detach of its outer instance, a copy
/// landing alone. A swapped copy whose link would end at a main, or at nothing,
/// **becomes an instance of what it shows**: linked to its swap, the swap
/// cleared — its children already link there. Anything else takes `to`.
pub(crate) fn landed(
    doc: &Document,
    n: &Node,
    to: Option<NodeId>,
) -> (Option<NodeId>, Option<NodeId>) {
    landed_with(doc, n.swap, to)
}

/// [`landed`] for a node carrying `swap`.
fn landed_with(
    doc: &Document,
    swap: Option<NodeId>,
    to: Option<NodeId>,
) -> (Option<NodeId>, Option<NodeId>) {
    match swap {
        Some(s)
            if doc.get(s).is_some_and(|s| s.component)
                && to.is_none_or(|t| doc.get(t).is_some_and(|t| t.component)) =>
        {
            (Some(s), None)
        }
        swap => (to, swap),
    }
}

/// [`landed`] as operations on `n`: its `SetLink`, and a `SetSwap` when that
/// changes too.
pub(crate) fn land_ops(doc: &Document, n: &Node, to: Option<NodeId>) -> Vec<Operation> {
    land_ops_through(doc, n, to, None)
}

/// [`land_ops`] for a climb that passed through a node carrying the swap `met`
/// — a nested copy that **followed** a swap made one link up (`r3` copying
/// `r2`, swapped to Heart inside the Card main). Its link climbs past that node,
/// so the swap is the climber's to carry, or it lands showing the slot's main
/// with the swap's layers cut loose (`[X5-L1-01]`): a node with no swap of its
/// own takes `met`, unless where it lands already shows that main.
pub(crate) fn land_ops_through(
    doc: &Document,
    n: &Node,
    to: Option<NodeId>,
    met: Option<NodeId>,
) -> Vec<Operation> {
    let inherited = met.filter(|s| {
        n.swap.is_none()
            && to.and_then(|t| shown_main(doc.node_map(), t)) != Some(*s)
            && doc.get(*s).is_some_and(|m| m.component)
    });
    let (link, swap) = landed_with(doc, n.swap.or(inherited), to);
    let mut ops = vec![Operation::SetLink { id: n.id, link }];
    if swap != n.swap {
        ops.push(Operation::SetSwap { id: n.id, swap });
    }
    ops
}

/// A link climbed past every node `past` admits, up its chain: where it lands,
/// and the first swap carried by a node it passed ([`land_ops_through`]).
pub(crate) fn climb(
    doc: &Document,
    link: NodeId,
    past: impl Fn(NodeId) -> bool,
) -> (Option<NodeId>, Option<NodeId>) {
    let mut to = Some(link);
    let mut met = None;
    for _ in 0..=doc.node_map().len() {
        match to {
            Some(s) if past(s) => {
                let sn = doc.get(s);
                met = met.or(sn.and_then(|n| n.swap));
                to = sn.and_then(|n| n.link);
            }
            _ => break,
        }
    }
    (to, met)
}

/// The swaps the tree `doc` holds that no longer stand, and the ops that settle
/// them — `component::settle_links`' last word on swaps, after the links have
/// climbed and been cut:
/// - a swap whose main is gone, or is no longer a main, is cleared;
/// - a swapped node no longer linked is cleared — it is no instance any more;
/// - a swapped node now linked **straight to a main** becomes an instance of what
///   it shows ([`landed`]).
///
/// A swap left equal to what its slot now shows is [`settle`]'s, which can
/// rewrite the copy to follow its slot (§15 D1003 (4)); this one has no
/// `IdSource`.
pub(crate) fn tidy(doc: &Document) -> Vec<Operation> {
    let nodes = doc.node_map();
    let mut out: Vec<(NodeId, Vec<Operation>)> = Vec::new();
    for n in nodes.values() {
        let Some(s) = n.swap else { continue };
        let target_ok = nodes.get(&s).is_some_and(|s| s.component);
        let link = n.link.and_then(|l| nodes.get(&l));
        let ops = match (target_ok, link) {
            (false, _) | (true, None) => vec![Operation::SetSwap {
                id: n.id,
                swap: None,
            }],
            (true, Some(l)) if l.component => land_ops(doc, n, Some(l.id)),
            _ => continue,
        };
        out.push((n.id, ops));
    }
    out.sort_by_key(|(id, _)| *id);
    out.into_iter().flat_map(|(_, ops)| ops).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The suggestion drops the last segment whatever the separator, and only
    /// the separator run and the spaces around it.
    #[test]
    fn a_suggested_filter_drops_the_last_segment() {
        assert_eq!(suggested_filter("Button - Icons - Star"), "Button - Icons");
        assert_eq!(suggested_filter("Icons/Star"), "Icons");
        assert_eq!(suggested_filter("icon.star"), "icon");
        assert_eq!(suggested_filter("Star"), "");
        assert_eq!(suggested_filter("Star "), "");
        // A run of separators goes whole.
        assert_eq!(suggested_filter("Button -- Star"), "Button");
        assert_eq!(suggested_filter("Icons // Star"), "Icons");
        // And only the separator: other punctuation in the kept part stays.
        assert_eq!(suggested_filter("Icons (v2) / Star"), "Icons (v2)");
    }
}
