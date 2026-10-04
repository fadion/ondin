//! The rules a document's components and instances must satisfy (§5.3d, §15 D978).
//!
//! An instance is a **linked copy**: its nodes are ordinary stored nodes, each
//! carrying [`crate::Node::link`] — the node it was copied from, one level up. A
//! main component is a frame or group with [`crate::Node::component`] set. That
//! link is the first reference from one node to another — a guide's owner, the
//! only earlier cross-reference, runs from a guide to a node — and §15 D405
//! declined one for exactly the semantics it owes. It is held to
//! account the way the owner is (§15 D491): [`check`] runs after the last op of
//! every transaction in `Document::apply`, and on load.
//!
//! **What this module does not do** is keep instances in step with their mains —
//! that is the commit-time pass (§5.3d, build step 3). Under compare-values
//! (§15 D979 (a)) a value an instance holds is never *invalid*, only an override,
//! so nothing here compares values: the rules are all about structure and links.

use crate::document::Document;
use crate::id::NodeId;
use crate::node::{Node, NodeKind};
use crate::op::{Operation, Transaction};
use rustc_hash::{FxHashMap, FxHashSet};

/// Which rule a document broke. Carried by `OpError::BadLink` and turned into the
/// loader's message, so both doors say the same thing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkRule {
    /// A node's link names a node that is not in the document, or itself.
    Dangling,
    /// `component` on a kind that is not a frame or a group.
    ComponentKind,
    /// A node both is a main and is linked.
    ComponentLinked,
    /// A main component sits inside a main component or an instance.
    NestedMain,
    /// A linked node that is not an instance root has no instance around it, or
    /// its source is not inside the source of the instance it belongs to.
    Membership,
    /// Two nodes of one instance are linked to the same source.
    SharedSource,
    /// A link chain loops without reaching a main component.
    LinkCycle,
    /// A main component contains, at some depth, an instance of itself.
    ComponentCycle,
}

impl std::fmt::Display for LinkRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Dangling => "links to a node that is not in the document",
            Self::ComponentKind => "is a main component but not a frame or a group",
            Self::ComponentLinked => "is a main component and is linked",
            Self::NestedMain => "is a main component inside a main component or an instance",
            Self::Membership => "is linked to a node outside the instance it belongs to",
            Self::SharedSource => "shares its source with another node of the same instance",
            Self::LinkCycle => "is on a chain of links that never reaches a main component",
            Self::ComponentCycle => "is a main component that contains an instance of itself",
        })
    }
}

/// Check every component rule over `nodes`, answering the first node that breaks
/// one and the rule it broke.
///
/// **Free when the document has no components**: one pass to find that no node is
/// a main or linked, and nothing else — so the post-condition costs an ordinary
/// document one scan beside the clone `apply` already makes.
///
/// The rules, in the terms of §5.3d:
/// - every link resolves, and not to the node itself;
/// - `component` only on an `Artboard` or a `Group`, never with a link, and never
///   beneath a main or a linked node;
/// - an **instance root** is a linked node whose source is a main, or whose source
///   is itself an instance root (a nested instance copied with its outer main); a
///   link chain must end at a main;
/// - every linked node **except one linked straight to a main** belongs to the
///   nearest instance root above it, and its source lies strictly inside that
///   root's source; within one instance, no two such nodes share a source. A
///   nested copy's root is held to this like a member — only a link to a main
///   itself (an instance, or a local instance inside another) is placed freely;
/// - no main contains, at any depth, an instance whose chain ends at itself.
pub fn check(nodes: &FxHashMap<NodeId, Node>) -> Result<(), (NodeId, LinkRule)> {
    if !nodes.values().any(|n| n.component || n.link.is_some()) {
        return Ok(());
    }
    let parent = |id: NodeId| nodes.get(&id).and_then(|n| n.parent);
    let ancestors = |id: NodeId| std::iter::successors(parent(id), move |p| parent(*p));
    let inside = |id: NodeId, outer: NodeId| ancestors(id).any(|a| a == outer);

    for n in nodes.values() {
        if n.component {
            if !matches!(n.kind, NodeKind::Artboard { .. } | NodeKind::Group) {
                return Err((n.id, LinkRule::ComponentKind));
            }
            if n.link.is_some() {
                return Err((n.id, LinkRule::ComponentLinked));
            }
            let nested = ancestors(n.id).any(|a| {
                nodes
                    .get(&a)
                    .is_some_and(|a| a.component || a.link.is_some())
            });
            if nested {
                return Err((n.id, LinkRule::NestedMain));
            }
        }
        if let Some(src) = n.link
            && (src == n.id || !nodes.contains_key(&src))
        {
            return Err((n.id, LinkRule::Dangling));
        }
    }

    // The main a node's chain ends at; `Err` for a chain that loops.
    let main_of = |id: NodeId| -> Result<NodeId, (NodeId, LinkRule)> {
        let mut at = id;
        for _ in 0..=nodes.len() {
            let n = &nodes[&at];
            if n.component {
                return Ok(at);
            }
            match n.link {
                Some(src) => at = src,
                None => return Err((id, LinkRule::Membership)),
            }
        }
        Err((id, LinkRule::LinkCycle))
    };
    let is_root = |n: &Node| is_instance_root(nodes, n);

    // Which instance root owns each member, and the sources already claimed in it.
    let mut claimed: FxHashSet<(NodeId, NodeId)> = FxHashSet::default();
    for n in nodes.values() {
        let Some(src) = n.link else { continue };
        if is_root(n) {
            main_of(n.id)?;
        }
        // **Only a link straight to a main is placed freely** — an instance, or a
        // local instance inside some other instance. A nested copy's root (linked
        // to an instance root inside an outer main) belongs to the instance around
        // it like any member: it was copied with that main, and a layer elsewhere
        // linked to the nested instance inside main B would otherwise pass
        // anywhere, and two of them could share it.
        if nodes[&src].component {
            continue;
        }
        let root = ancestors(n.id)
            .filter_map(|a| nodes.get(&a))
            .find(|a| is_root(a))
            .ok_or((n.id, LinkRule::Membership))?;
        let root_src = root.link.expect("an instance root is linked");
        if !inside(src, root_src) {
            return Err((n.id, LinkRule::Membership));
        }
        if !claimed.insert((root.id, src)) {
            return Err((n.id, LinkRule::SharedSource));
        }
    }

    // Component → the mains of the instances inside it; a cycle there is an
    // instance of a main inside itself, at some depth, which could never be
    // expanded.
    let mut uses: FxHashMap<NodeId, Vec<NodeId>> = FxHashMap::default();
    for n in nodes.values().filter(|n| is_root(n)) {
        let main = main_of(n.id)?;
        if let Some(outer) = std::iter::once(n.id)
            .chain(ancestors(n.id))
            .find(|a| nodes.get(a).is_some_and(|a| a.component))
        {
            uses.entry(outer).or_default().push(main);
        }
    }
    let mut done: FxHashSet<NodeId> = FxHashSet::default();
    for &start in uses.keys() {
        let mut path: Vec<NodeId> = Vec::new();
        if reaches_itself(start, &uses, &mut path, &mut done) {
            return Err((start, LinkRule::ComponentCycle));
        }
    }
    Ok(())
}

/// Whether `n` is an instance root: linked to a main, or to an instance root —
/// equivalently, its chain of links ends at a main. A member's chain ends at an
/// unlinked node inside a main instead, which is how the two are told apart.
fn is_instance_root(nodes: &FxHashMap<NodeId, Node>, n: &Node) -> bool {
    let mut at = n;
    for _ in 0..=nodes.len() {
        let Some(src) = at.link.and_then(|s| nodes.get(&s)) else {
            return false;
        };
        if src.component {
            return true;
        }
        at = src;
    }
    false
}

fn reaches_itself(
    at: NodeId,
    uses: &FxHashMap<NodeId, Vec<NodeId>>,
    path: &mut Vec<NodeId>,
    done: &mut FxHashSet<NodeId>,
) -> bool {
    if path.contains(&at) {
        return true;
    }
    if !done.insert(at) {
        return false;
    }
    path.push(at);
    let looped = uses
        .get(&at)
        .is_some_and(|next| next.iter().any(|m| reaches_itself(*m, uses, path, done)));
    path.pop();
    looped
}

// ── The verbs (§5.3d, build step 2) ────────────────────────────────────────────

/// Whether `id` could be made a main component as it stands: a frame or a group,
/// not linked, and with no main or linked node above it — [`check`]'s own rules,
/// asked ahead so a menu row can say so rather than the commit refusing.
pub fn can_be_main(doc: &Document, id: NodeId) -> bool {
    let nodes = doc.node_map();
    let Some(n) = nodes.get(&id) else {
        return false;
    };
    if n.component
        || n.link.is_some()
        || !matches!(n.kind, NodeKind::Artboard { .. } | NodeKind::Group)
    {
        return false;
    }
    let mut at = n.parent;
    while let Some(p) = at.and_then(|p| nodes.get(&p)) {
        if p.component || p.link.is_some() {
            return false;
        }
        at = p.parent;
    }
    true
}

/// The instance root `id` belongs to — itself, if it is one, else the nearest one
/// above it — or `None` outside every instance. A local addition inside an
/// instance belongs to it too: this is about where a node sits, not whether it is
/// linked.
pub fn instance_root(doc: &Document, id: NodeId) -> Option<NodeId> {
    let nodes = doc.node_map();
    let mut at = Some(id);
    while let Some(n) = at.and_then(|a| nodes.get(&a)) {
        if is_instance_root(nodes, n) {
            return Some(n.id);
        }
        at = n.parent;
    }
    None
}

/// Every instance of `main` in the document — each instance root whose chain of
/// links ends at it, nested copies included — in id order.
pub fn instances_of(doc: &Document, main: NodeId) -> Vec<NodeId> {
    let nodes = doc.node_map();
    let mut out: Vec<NodeId> = nodes
        .values()
        .filter(|n| n.link.is_some() && is_instance_root(nodes, n))
        .filter(|n| main_of(doc, n.id) == Some(main))
        .map(|n| n.id)
        .collect();
    out.sort();
    out
}

/// The main an instance root's chain of links ends at.
pub fn main_of(doc: &Document, root: NodeId) -> Option<NodeId> {
    let nodes = doc.node_map();
    let mut at = nodes.get(&root)?;
    for _ in 0..=nodes.len() {
        if at.component {
            return Some(at.id);
        }
        at = nodes.get(&at.link?)?;
    }
    None
}

/// Links into `gone` **climb past it**: every node `among` admits whose link
/// names a node in `gone` is relinked to the first source above that is not in
/// `gone`, or cut when there is none.
///
/// The one rule behind every way a link's target disappears (§5.3d's *"one rule
/// for every way a link is cut"*):
/// - **deleting a main** (§15 D979 (c)): its instances' own links climb to
///   nothing, so they detach and keep their look, while a nested instance copied
///   with them climbs to *its* main and stays an instance;
/// - **deleting a node inside a main**: each counterpart is cut loose and kept as
///   the instance's own layer (or climbs, when the deleted child was itself a
///   nested copy, to the nested main's node — the first bullet's case), so instance-side work is never destroyed by an
///   edit to the main — until build step 4 can tell an untouched counterpart from
///   a changed one, which is when the untouched ones start being deleted with it;
/// - **detaching** ([`detach`]) is the same climb, past the instance's own main.
pub fn relink_past(
    doc: &Document,
    gone: &FxHashSet<NodeId>,
    among: impl Fn(NodeId) -> bool,
) -> Vec<Operation> {
    let nodes = doc.node_map();
    let mut ops = Vec::new();
    for n in nodes.values() {
        let Some(link) = n.link else { continue };
        if !gone.contains(&link) || !among(n.id) {
            continue;
        }
        let mut to = Some(link);
        for _ in 0..=nodes.len() {
            match to {
                Some(s) if gone.contains(&s) => to = nodes.get(&s).and_then(|s| s.link),
                _ => break,
            }
        }
        ops.push(Operation::SetLink { id: n.id, link: to });
    }
    // Deterministic order, so the same edit produces the same transaction.
    ops.sort_by_key(|op| match op {
        Operation::SetLink { id, .. } => *id,
        _ => unreachable!(),
    });
    ops
}

/// The ops that keep every link valid when the subtrees under `deleted` go — the
/// [`relink_past`] the Delete verb owes (§15 D979 (c)). Empty in a document with
/// no instances.
pub fn relink_for_delete(doc: &Document, deleted: &[NodeId]) -> Vec<Operation> {
    let gone: FxHashSet<NodeId> = crate::build::subtree_nodes(doc, deleted)
        .into_iter()
        .collect();
    relink_past(doc, &gone, |id| !gone.contains(&id))
}

/// **Detach** the instance rooted at `root`: it becomes ordinary layers that keep
/// their current look (§5.3d).
///
/// The root and every node that belongs to it are cut loose. A nested instance
/// inside it is not detached with it: its links climb past this instance's main
/// to the nested main's own nodes ([`relink_past`]), so it stays an instance of
/// that main. Links to anything else — a local instance of some other main —
/// are left alone. `None` when `root` is not an instance root.
pub fn detach(doc: &Document, root: NodeId) -> Option<Transaction> {
    let nodes = doc.node_map();
    let r = nodes.get(&root)?;
    if !is_instance_root(nodes, r) {
        return None;
    }
    let src = r.link?;
    let past: FxHashSet<NodeId> = crate::build::subtree_nodes(doc, &[src])
        .into_iter()
        .collect();
    // Which instance root each node of this one belongs to: the nearest instance
    // root at or above it, a nested root belonging to itself.
    let owner = |id: NodeId| {
        let mut at = nodes.get(&id);
        while let Some(n) = at {
            if is_instance_root(nodes, n) {
                return Some(n.id);
            }
            at = n.parent.and_then(|p| nodes.get(&p));
        }
        None
    };
    let mut ops = Vec::new();
    for id in crate::build::subtree_nodes(doc, &[root]) {
        let Some(link) = nodes.get(&id).and_then(|n| n.link) else {
            continue;
        };
        if !past.contains(&link) {
            continue; // a local instance of some other main: not this instance's link
        }
        let to = if owner(id) == Some(root) {
            // The root and its own members: cut. Climbing would hand a member a
            // link with no instance root above it once the root is cut.
            None
        } else {
            // A nested instance copied inside this one, and its members: climb
            // past this instance's main to the nested main's own nodes.
            let mut to = Some(link);
            for _ in 0..=nodes.len() {
                match to {
                    Some(s) if past.contains(&s) => to = nodes.get(&s).and_then(|s| s.link),
                    _ => break,
                }
            }
            to
        };
        ops.push(Operation::SetLink { id, link: to });
    }
    Some(Transaction(ops))
}

/// The links a transaction's **structural** edits owe (§5.3d), so that no door
/// that moves, deletes or regroups layers has to know about components. A
/// commit-time pass, `build::keep_insets`' shape: `EditorSession::commit_inner`
/// runs it over every transaction.
///
/// The transaction is applied to a scratch copy (`Document::apply_unchecked`) and
/// the tree it leaves is asked two things:
/// - **links into deleted nodes climb past them** ([`relink_past`]'s rule) — the
///   Delete verb does this itself, but `build::ungroup`, *Flatten*, *Outline* and
///   text-on-a-new-path delete nodes too, and an ungrouped main is a deleted main;
/// - **every linked node still belongs to its instance**: an instance root above
///   it whose source contains its source, and no other node of that instance on
///   the same source. What fails is cut — a layer dragged out of its instance, the
///   members of an instance whose root an ungroup dissolved. Asked top-down, so a
///   cut nested root cuts its members; and where two nodes of one instance share a
///   source, **the one the transaction did not touch keeps it**.
///
/// A link straight to a main is placed anywhere, as [`check`] allows. Empty for
/// a document with no links or a transaction with no structural op.
pub fn settle_links(doc: &Document, tx: &Transaction) -> Vec<Operation> {
    let before = doc.node_map();
    let structural = tx.0.iter().any(|op| {
        matches!(
            op,
            Operation::Reparent { .. }
                | Operation::DeleteNode { .. }
                | Operation::InsertSubtree { .. }
                | Operation::CreateNode { .. }
        )
    });
    if !structural || !before.values().any(|n| n.link.is_some()) {
        return Vec::new();
    }
    let mut scratch = doc.clone();
    if scratch.apply_unchecked(tx).is_err() {
        return Vec::new(); // `apply` will refuse it with the reason
    }
    let mut ops = Vec::new();

    // Links into deleted nodes climb past them.
    let gone: FxHashSet<NodeId> = before
        .keys()
        .filter(|id| scratch.get(**id).is_none())
        .copied()
        .collect();
    if !gone.is_empty() {
        let mut climbs: Vec<Operation> = scratch
            .node_map()
            .values()
            .filter_map(|n| {
                let link = n.link.filter(|l| gone.contains(l))?;
                let mut to = Some(link);
                for _ in 0..=before.len() {
                    match to {
                        Some(s) if gone.contains(&s) => to = before.get(&s).and_then(|s| s.link),
                        _ => break,
                    }
                }
                Some(Operation::SetLink { id: n.id, link: to })
            })
            .collect();
        climbs.sort_by_key(set_link_id);
        if scratch
            .apply_unchecked(&Transaction(climbs.clone()))
            .is_err()
        {
            return Vec::new();
        }
        ops.extend(climbs);
    }

    // Membership on the tree the transaction leaves.
    let nodes = scratch.node_map();
    let parent_of = |id: NodeId| nodes.get(&id).and_then(|n| n.parent);
    let inside = |id: NodeId, outer: NodeId| {
        std::iter::successors(parent_of(id), |a| parent_of(*a)).any(|a| a == outer)
    };
    let depth = |id: NodeId| std::iter::successors(Some(id), |a| parent_of(*a)).count();
    let mut touched: FxHashSet<NodeId> = FxHashSet::default();
    for op in &tx.0 {
        match op {
            Operation::Reparent { id, .. } => {
                touched.extend(crate::build::subtree_nodes(&scratch, &[*id]));
            }
            Operation::CreateNode { id, .. } => {
                touched.insert(*id);
            }
            Operation::InsertSubtree {
                nodes: inserted, ..
            } => {
                touched.extend(inserted.iter().map(|n| n.id));
            }
            _ => {}
        }
    }
    // Whether `id`'s chain ends at a main, given the links cut so far.
    let is_root = |id: NodeId, cut: &FxHashSet<NodeId>| {
        let mut at = id;
        for _ in 0..=nodes.len() {
            if cut.contains(&at) {
                return false;
            }
            let Some(src) = nodes.get(&at).and_then(|n| n.link) else {
                return false;
            };
            if nodes.get(&src).is_some_and(|s| s.component) {
                return true;
            }
            at = src;
        }
        false
    };
    let mut members: Vec<NodeId> = nodes
        .values()
        .filter(|n| {
            n.link
                .is_some_and(|s| nodes.get(&s).is_some_and(|s| !s.component))
        })
        .map(|n| n.id)
        .collect();
    members.sort_by_key(|id| (depth(*id), *id));
    let mut cut: FxHashSet<NodeId> = FxHashSet::default();
    for _ in 0..=members.len() {
        let mut changed = false;
        let mut claims: FxHashMap<(NodeId, NodeId), Vec<NodeId>> = FxHashMap::default();
        for &id in &members {
            if cut.contains(&id) {
                continue;
            }
            let src = nodes[&id].link.expect("filtered to linked nodes");
            let root =
                std::iter::successors(parent_of(id), |a| parent_of(*a)).find(|a| is_root(*a, &cut));
            match root {
                Some(r) if inside(src, nodes[&r].link.expect("a root is linked")) => {
                    claims.entry((r, src)).or_default().push(id);
                }
                _ => {
                    cut.insert(id);
                    changed = true;
                }
            }
        }
        for mut sharing in claims.into_values().filter(|v| v.len() > 1) {
            sharing.sort_by_key(|id| (touched.contains(id), *id));
            for id in &sharing[1..] {
                cut.insert(*id);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut cuts: Vec<Operation> = cut
        .into_iter()
        .map(|id| Operation::SetLink { id, link: None })
        .collect();
    cuts.sort_by_key(set_link_id);
    ops.extend(cuts);
    ops
}

fn set_link_id(op: &Operation) -> NodeId {
    match op {
        Operation::SetLink { id, .. } => *id,
        _ => unreachable!("only SetLinks are sorted here"),
    }
}

/// How a copy of a **main** lands (§15 D979 (e)): as an instance of it, which is
/// what copy and paste, duplicate and Alt-drag do, or as a new main — *Duplicate
/// as component*.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MainCopy {
    Instance,
    NewMain,
}

/// Settle the links of `copy` — `template` run through `remap_subtree`, index for
/// index — for where it is about to land in `doc`. Answers whether the copy became
/// an **instance** of the main it was copied from.
///
/// - A copy of a main in this document becomes an instance of it under
///   [`MainCopy::Instance`]: every copied node links to the node it was copied
///   from, and the copy is not itself a main. Its names stay the main's (§15
///   D981), which is why the caller skips the copy-numbering then.
/// - Otherwise links are settled one at a time. A link `remap_subtree` pointed
///   inside the copy stays (a main copied with its instance). A link to a node not
///   in this document is dropped — a paste from another document (§15 D979 (d)).
///   An instance root's link stays: its copy is a new instance of the same source.
///   **A member's link stays only if its instance root came along**; a member
///   copied on its own becomes the copy's own layer, which is what keeps a
///   duplicate inside its own instance from claiming its original's source
///   (`LinkRule::SharedSource`).
pub(crate) fn settle_copy(
    doc: &Document,
    template: &[Node],
    copy: &mut [Node],
    mains: MainCopy,
) -> bool {
    let nodes = doc.node_map();
    let in_template: FxHashMap<NodeId, &Node> = template.iter().map(|n| (n.id, n)).collect();
    let root = template
        .iter()
        .find(|n| n.parent.is_none_or(|p| !in_template.contains_key(&p)));
    if mains == MainCopy::Instance
        && let Some(root) = root
        && root.component
        && nodes.get(&root.id).is_some_and(|n| n.component)
    {
        for (c, t) in copy.iter_mut().zip(template) {
            c.link = Some(t.id);
            c.component = false;
        }
        return true;
    }
    let copied: FxHashSet<NodeId> = copy.iter().map(|n| n.id).collect();
    let root_above = |t: &Node| {
        let mut at = t.parent.and_then(|p| in_template.get(&p).copied());
        while let Some(a) = at {
            if is_instance_root(nodes, a) {
                return true;
            }
            at = a.parent.and_then(|p| in_template.get(&p).copied());
        }
        false
    };
    for (c, t) in copy.iter_mut().zip(template) {
        let Some(link) = c.link else { continue };
        if copied.contains(&link) {
            continue;
        }
        let keep = nodes.contains_key(&link) && (is_instance_root(nodes, t) || root_above(t));
        if !keep {
            c.link = None;
        }
    }
    // A **nested instance copied on its own** — its root linked to the nested copy
    // inside some outer main, with no outer instance coming along — belongs to no
    // instance where it lands, so it climbs one level up, `detach`'s rule for a
    // nested instance: its root and its members relink past that nested copy to
    // the nested main's own nodes, and it lands as a plain instance of that main.
    if let Some(t) = root
        && !root_above(t)
        && is_instance_root(nodes, t)
        && let Some(src) = t.link
        && nodes.get(&src).is_some_and(|s| !s.component)
    {
        let past: FxHashSet<NodeId> = crate::build::subtree_nodes(doc, &[src])
            .into_iter()
            .collect();
        for c in copy.iter_mut() {
            let mut to = c.link;
            for _ in 0..=nodes.len() {
                match to {
                    Some(s) if past.contains(&s) => to = nodes.get(&s).and_then(|s| s.link),
                    _ => break,
                }
            }
            c.link = to;
        }
    }
    false
}
