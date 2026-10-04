//! The rules a document's components and instances must satisfy (§5.3d, §15 D978).
//!
//! An instance is a **linked copy**: its nodes are ordinary stored nodes, each
//! carrying [`crate::Node::link`] — the node it was copied from, one level up. A
//! main component is a frame or group with [`crate::Node::component`] set. That
//! link is the first node-to-node reference in the model besides a guide's owner
//! (§15 D405 declined one for exactly the semantics it owes), and it is held to
//! account the way the owner is (§15 D491): [`check`] runs after the last op of
//! every transaction in `Document::apply`, and on load.
//!
//! **What this module does not do** is keep instances in step with their mains —
//! that is the commit-time pass (§5.3d, build step 3). Under compare-values
//! (§15 D979 (a)) a value an instance holds is never *invalid*, only an override,
//! so nothing here compares values: the rules are all about structure and links.

use crate::id::NodeId;
use crate::node::{Node, NodeKind};
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
/// - every other linked node belongs to the nearest instance root above it, and its
///   source lies strictly inside that root's source; within one instance, no two
///   such nodes share a source;
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
