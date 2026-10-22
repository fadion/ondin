//! The rules a document's components and instances must satisfy (§5.3d).
//!
//! An instance is a linked copy: its nodes are ordinary stored nodes, each
//! carrying [`crate::Node::link`], the node it was copied from one level up. A
//! main component is a frame or group with [`crate::Node::component`] set.
//! [`check`] runs after the last op of every transaction in `Document::apply`,
//! and on load.
//!
//! Keeping instances in step with their mains is the commit-time pass, not this
//! module. A value an instance holds is never invalid, only an override, so the
//! rules here are all about structure and links.

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
    /// A swap on a node that is not a nested copy inside an outer instance, or
    /// naming something that is not a main of the node's own kind.
    Swap,
    /// A variant or component-property rule.
    Variant(crate::variant::VariantRule),
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
            Self::Swap => "is swapped to something it cannot show",
            Self::Variant(rule) => return rule.fmt(f),
        })
    }
}

/// Check every component rule over `nodes`, answering the first node that breaks
/// one and the rule it broke.
///
/// Free when the document has no components: one scan to find that no node is a
/// main or linked.
///
/// The rules, in the terms of §5.3d:
/// - every link resolves, and not to the node itself;
/// - `component` only on an `Artboard` or a `Group`, never with a link, and never
///   beneath a main or a linked node;
/// - an instance root is a linked node whose source is a main, or whose source
///   is itself an instance root (a nested instance copied with its outer main); a
///   link chain must end at a main;
/// - every linked node except one linked straight to a main belongs to the
///   nearest instance root above it, and its source lies strictly inside that
///   root's source; within one instance, no two such nodes share a source. A
///   nested copy's root is held to this like a member; a swapped root's source,
///   for its members, is its swap;
/// - a swap sits only on a nested copy inside an outer instance (`swap::can_swap`)
///   and names a main of the copy's own kind;
/// - no main contains, at any depth, an instance of a main it shows — through
///   its chain, or through a swap on it.
pub fn check(nodes: &FxHashMap<NodeId, Node>) -> Result<(), (NodeId, LinkRule)> {
    crate::variant::check(nodes).map_err(|(id, rule)| (id, LinkRule::Variant(rule)))?;
    if !nodes.values().any(|n| n.component || n.link.is_some()) {
        if let Some(n) = nodes.values().find(|n| n.swap.is_some()) {
            return Err((n.id, LinkRule::Swap));
        }
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
        if let Some(s) = n.swap {
            let Some(target) = nodes.get(&s) else {
                return Err((n.id, LinkRule::Dangling));
            };
            let nested_copy = !n.component
                && n.link
                    .and_then(|l| nodes.get(&l))
                    .is_some_and(|l| !l.component)
                && is_instance_root(nodes, n);
            let same_kind = std::mem::discriminant(&n.kind) == std::mem::discriminant(&target.kind);
            if !target.component || !nested_copy || !same_kind {
                return Err((n.id, LinkRule::Swap));
            }
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
        // Only a link straight to a main is placed freely. A nested copy's root
        // (linked to an instance root inside an outer main) belongs to the
        // instance around it like any member; otherwise a layer elsewhere linked
        // to a nested instance would pass anywhere, and two could share it.
        if nodes[&src].component {
            continue;
        }
        let root = ancestors(n.id)
            .filter_map(|a| nodes.get(&a))
            .find(|a| is_root(a))
            .ok_or((n.id, LinkRule::Membership))?;
        // A swapped root's members are copies of its swap's nodes, and of what its
        // slot adds of its own, nested instances included ([`slot_adds`]).
        let root_src = root.swap.or(root.link).expect("an instance root is linked");
        let slot_own = root.swap.is_some() && root.link.is_some_and(|l| slot_adds(nodes, l, src));
        if !inside(src, root_src) && !slot_own {
            return Err((n.id, LinkRule::Membership));
        }
        if !claimed.insert((root.id, src)) {
            return Err((n.id, LinkRule::SharedSource));
        }
    }

    // Component → the mains of the instances inside it; a cycle there is an
    // instance of a main inside itself, at some depth, which could never be
    // expanded.
    for n in nodes.values().filter(|n| is_root(n)) {
        main_of(n.id)?;
    }
    let uses = uses(nodes);
    let mut done: FxHashSet<NodeId> = FxHashSet::default();
    for &start in uses.keys() {
        if reaches_itself(start, &uses, &mut done) {
            return Err((start, LinkRule::ComponentCycle));
        }
    }
    Ok(())
}

/// Each main → the mains shown by the instances inside it, at any depth below it
/// and itself excluded (`swap::shown_main`: a swap counts, the chain it replaces
/// does not) — the graph a component cycle is a cycle in.
pub(crate) fn uses(nodes: &FxHashMap<NodeId, Node>) -> FxHashMap<NodeId, Vec<NodeId>> {
    let parent = |id: NodeId| nodes.get(&id).and_then(|n| n.parent);
    let mut uses: FxHashMap<NodeId, Vec<NodeId>> = FxHashMap::default();
    for n in nodes.values().filter(|n| is_instance_root(nodes, n)) {
        let Some(main) = crate::swap::shown_main(nodes, n.id) else {
            continue;
        };
        if let Some(outer) = std::iter::successors(Some(n.id), |a| parent(*a))
            .find(|a| nodes.get(a).is_some_and(|a| a.component))
        {
            uses.entry(outer).or_default().push(main);
        }
    }
    uses
}

/// Whether `n` is an instance root: linked to a main, or to an instance root —
/// equivalently, its chain of links ends at a main. A member's chain ends at an
/// unlinked node inside a main instead, which is how the two are told apart.
/// A swap does not change the answer: the link still names the slot.
pub(crate) fn is_instance_root(nodes: &FxHashMap<NodeId, Node>, n: &Node) -> bool {
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

/// Whether `src` — the source of a layer inside a swapped copy whose slot is
/// `slot` — is a layer the slot adds of its own: inside the slot, and copying
/// nothing of the main the slot shows, its chain of content sources
/// (`swap::content_source`) never landing there. That is `variant::rewrite`'s own
/// test for what a swap keeps (`Base::of_node`), so [`check`] and [`settle_links`]
/// admit exactly the layers the rewrite left.
///
/// A slot-added layer may itself be linked (a nested instance, or a group holding
/// one), so "linked to nothing" is the wrong test.
pub(crate) fn slot_adds(nodes: &FxHashMap<NodeId, Node>, slot: NodeId, src: NodeId) -> bool {
    let parent = |id: NodeId| nodes.get(&id).and_then(|n| n.parent);
    let inside = |id: NodeId, outer: NodeId| {
        std::iter::successors(parent(id), |a| parent(*a)).any(|a| a == outer)
    };
    if !inside(src, slot) {
        return false;
    }
    let Some(shown) = crate::swap::shown_main(nodes, slot) else {
        return true;
    };
    let mut at = src;
    for _ in 0..=nodes.len() {
        if at == shown || inside(at, shown) {
            return false;
        }
        match nodes.get(&at).and_then(|n| n.swap.or(n.link)) {
            Some(next) => at = next,
            None => return true,
        }
    }
    false // a loop, which is `LinkRule::LinkCycle`'s to report
}

/// Whether a depth-first walk of `uses` from `start` comes back to a main still
/// on its path — a component cycle. `done` is shared across starts: a main
/// already walked from an earlier start, and not on this path, has nothing new
/// below it.
///
/// An explicit stack, not recursion: this runs on every load and apply, and a
/// crafted file can make the graph as long as it has mains while its tree stays
/// under `io::MAX_TREE_DEPTH`. A stack overflow aborts, and nothing catches it.
/// The path is a set so the search is not quadratic in the chain's length.
fn reaches_itself(
    start: NodeId,
    uses: &FxHashMap<NodeId, Vec<NodeId>>,
    done: &mut FxHashSet<NodeId>,
) -> bool {
    if !done.insert(start) {
        return false;
    }
    let mut on_path: FxHashSet<NodeId> = FxHashSet::default();
    on_path.insert(start);
    // Each frame is a main and the index of the next of its uses to walk.
    let mut stack: Vec<(NodeId, usize)> = vec![(start, 0)];
    while let Some(&(at, next)) = stack.last() {
        let Some(&m) = uses.get(&at).and_then(|ms| ms.get(next)) else {
            on_path.remove(&at);
            stack.pop();
            continue;
        };
        if let Some(top) = stack.last_mut() {
            top.1 += 1;
        }
        if on_path.contains(&m) {
            return true;
        }
        if done.insert(m) {
            on_path.insert(m);
            stack.push((m, 0));
        }
    }
    false
}

// ── The verbs (§5.3d, build step 2) ────────────────────────────────────────────

/// Whether `id` could be made a main component as it stands — [`why_not_main`]
/// with no answer.
pub fn can_be_main(doc: &Document, id: NodeId) -> bool {
    why_not_main(doc, id).is_none()
}

/// Why a layer cannot be made a main component — in place ([`why_not_main`]) or
/// wrapped in a new frame first ([`why_not_wrap`]) — so a menu row and the chord
/// can say which rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotMain {
    /// The layer is missing, or is not a frame or a group (in place only — the
    /// verb wraps anything else).
    Kind,
    /// It is a main already.
    Already,
    /// It sits inside a main or an instance — or, for a wrap, the parent the new
    /// frame would be made in is one or sits in one: `LinkRule::NestedMain`.
    Inside,
    /// It is an instance root (detach it first).
    Instance,
    /// It is a set, or holds a main or a set: making it a main nests that main
    /// (`LinkRule::NestedMain`) or that set (`VariantRule::SetKind`).
    Holds,
    /// It would be a variant of a set whose variants are another kind (one kind
    /// per set).
    SetKind,
}

/// Why `id` could not be made a main component in place, or `None` when it can —
/// [`check`]'s own rules, and `variant::check`'s, asked ahead so a menu row can
/// say so rather than the commit refusing. The subtree is asked too, as the
/// commit asks the whole tree.
pub fn why_not_main(doc: &Document, id: NodeId) -> Option<NotMain> {
    let nodes = doc.node_map();
    let Some(n) = nodes.get(&id) else {
        return Some(NotMain::Kind);
    };
    if !matches!(n.kind, NodeKind::Artboard { .. } | NodeKind::Group) {
        return Some(NotMain::Kind);
    }
    if n.component {
        return Some(NotMain::Already);
    }
    if n.parent.is_some_and(|p| main_or_instance_around(nodes, p)) {
        return Some(NotMain::Inside);
    }
    if n.link.is_some() {
        return Some(NotMain::Instance);
    }
    if holds_main_or_set(doc, id) {
        return Some(NotMain::Holds);
    }
    if let Some(p) = n.parent
        && crate::variant::is_set(doc, p)
        && !crate::variant::fits_set(doc, p, &n.kind, Some(id))
    {
        return Some(NotMain::SetKind);
    }
    None
}

/// [`why_not_main`] for *Create component*'s wrap: `members` framed where they
/// stand (`build::frame`) and the frame made the main. `None` when the commit
/// will take it.
///
/// The new frame is made in the members' parent, so that parent — not a member —
/// must not be or sit in a main or an instance, and no member may hold a main or
/// a set. Members with no one parent answer `None` and are `build::frame`'s to
/// refuse.
pub fn why_not_wrap(doc: &Document, members: &[NodeId]) -> Option<NotMain> {
    let nodes = doc.node_map();
    let parent = members
        .first()
        .and_then(|m| nodes.get(m))
        .and_then(|n| n.parent)?;
    if members
        .iter()
        .any(|m| nodes.get(m).and_then(|n| n.parent) != Some(parent))
    {
        return None;
    }
    if main_or_instance_around(nodes, parent) {
        return Some(NotMain::Inside);
    }
    if members.iter().any(|m| holds_main_or_set(doc, *m)) {
        return Some(NotMain::Holds);
    }
    let frame = NodeKind::Artboard {
        size: kurbo::Size::ZERO,
    };
    if crate::variant::is_set(doc, parent) && !crate::variant::fits_set(doc, parent, &frame, None) {
        return Some(NotMain::SetKind);
    }
    None
}

/// Whether `id` or any node above it is a main or linked.
fn main_or_instance_around(nodes: &FxHashMap<NodeId, Node>, id: NodeId) -> bool {
    let mut at = Some(id);
    while let Some(n) = at.and_then(|a| nodes.get(&a)) {
        if n.component || n.link.is_some() {
            return true;
        }
        at = n.parent;
    }
    false
}

/// Whether `id`'s subtree, `id` included, holds a main or a set.
fn holds_main_or_set(doc: &Document, id: NodeId) -> bool {
    crate::build::subtree_nodes(doc, &[id])
        .iter()
        .filter_map(|s| doc.get(*s))
        .any(|s| s.component || s.set.is_some())
}

/// Whether `parent` can take `moved` as a child under the component and variant
/// rules — the half of "can this land here" that `build::can_parent`, a question
/// about two kinds, cannot see. False when:
///
/// - `moved` is or holds a main or a set and `parent` is or sits in a main or an
///   instance — `LinkRule::NestedMain`, `VariantRule::SetKind`;
/// - `moved` is or holds a set and `parent` is or sits in a set — `SetKind`;
/// - `moved` is a main, `parent` a set, and the set's variants are another kind
///   (one kind per set — refused at this door, never at the loader);
/// - `moved` holds an instance whose shown main is the main `parent` sits in, or
///   one that reaches it through instances — `LinkRule::ComponentCycle`.
///
/// Always true for `moved`'s own parent. Membership is not asked: a linked layer
/// moved out of its instance is cut loose by [`settle_links`], which is the
/// commit's answer rather than a refusal.
pub fn can_hold(doc: &Document, parent: NodeId, moved: NodeId) -> bool {
    let nodes = doc.node_map();
    let Some(m) = nodes.get(&moved) else {
        return false;
    };
    // Staying is never refused: a variant of a mixed set the loader admitted
    // would otherwise leave its set on a nudge.
    if m.parent == Some(parent) {
        return true;
    }
    let sub = crate::build::subtree_nodes(doc, &[moved]);
    let sub_nodes = || sub.iter().filter_map(|s| nodes.get(s));
    let mut main_around = None;
    let (mut in_instance, mut in_set) = (false, false);
    let mut at = Some(parent);
    while let Some(a) = at.and_then(|a| nodes.get(&a)) {
        if a.component && main_around.is_none() {
            main_around = Some(a.id);
        }
        in_instance |= a.link.is_some();
        in_set |= a.set.is_some();
        at = a.parent;
    }
    if (main_around.is_some() || in_instance) && sub_nodes().any(|s| s.component || s.set.is_some())
    {
        return false;
    }
    if in_set && sub_nodes().any(|s| s.set.is_some()) {
        return false;
    }
    if m.component
        && crate::variant::is_set(doc, parent)
        && !crate::variant::fits_set(doc, parent, &m.kind, Some(moved))
    {
        return false;
    }
    let Some(host) = main_around else {
        return true;
    };
    let shown: Vec<NodeId> = sub_nodes()
        .filter(|s| is_instance_root(nodes, s))
        .filter_map(|s| crate::swap::shown_main(nodes, s.id))
        .collect();
    if shown.is_empty() {
        return true;
    }
    // `host` would contain an instance of each `shown` main: a cycle exactly when
    // one of them is `host` or uses it, at some depth.
    let uses = uses(nodes);
    let mut seen: FxHashSet<NodeId> = FxHashSet::default();
    let mut stack = shown;
    while let Some(x) = stack.pop() {
        if x == host {
            return false;
        }
        if seen.insert(x) {
            stack.extend(uses.get(&x).into_iter().flatten().copied());
        }
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
///
/// Never one inside the vault: a kit main nesting `main` holds a copy there that
/// no one can see, pick or edit, and it must not be selected or counted.
pub fn instances_of(doc: &Document, main: NodeId) -> Vec<NodeId> {
    let nodes = doc.node_map();
    let mut out: Vec<NodeId> = nodes
        .values()
        .filter(|n| n.link.is_some() && is_instance_root(nodes, n))
        .filter(|n| !doc.in_vault(n.id))
        .filter(|n| main_of(doc, n.id) == Some(main))
        .map(|n| n.id)
        .collect();
    out.sort();
    out
}

/// The main an instance root shows: its chain of links' end, or the swap that
/// takes the chain's place where one sits on it (`swap::shown_main`) — what
/// *Instance of*, *Select all instances*, *Go to main* and the properties an
/// instance offers all mean by its main.
pub fn main_of(doc: &Document, root: NodeId) -> Option<NodeId> {
    crate::swap::shown_main(doc.node_map(), root)
}

/// Links into `gone` climb past it: every node `among` admits whose link names a
/// node in `gone` is relinked to the first source above that is not in `gone`, or
/// cut when there is none.
///
/// The one rule behind every way a link's target disappears (§5.3d):
/// - deleting a main: its instances' own links climb to nothing, so they detach
///   and keep their look, while a nested instance copied with them climbs to its
///   own main and stays an instance;
/// - deleting a node inside a main: each counterpart is cut loose and kept as the
///   instance's own layer (or climbs, when the deleted child was itself a nested
///   copy), so instance-side work is never destroyed by an edit to the main;
/// - detaching ([`detach`]) is the same climb, past the instance's own main.
///
/// A swapped copy lands by `swap::landed`: one whose climb ends at a main, or at
/// nothing, becomes an instance of the main it shows. A swap naming a main in
/// `gone` is cleared.
pub fn relink_past(
    doc: &Document,
    gone: &FxHashSet<NodeId>,
    among: impl Fn(NodeId) -> bool,
) -> Vec<Operation> {
    let nodes = doc.node_map();
    let mut by_node: Vec<(NodeId, Vec<Operation>)> = Vec::new();
    for n in nodes.values() {
        if !among(n.id) {
            continue;
        }
        let Some(link) = n.link.filter(|l| gone.contains(l)) else {
            if n.swap.is_some_and(|s| gone.contains(&s)) {
                by_node.push((
                    n.id,
                    vec![Operation::SetSwap {
                        id: n.id,
                        swap: None,
                    }],
                ));
            }
            continue;
        };
        let (to, met) = crate::swap::climb(doc, link, |s| gone.contains(&s));
        let mut ops = if n.swap.is_some_and(|s| gone.contains(&s)) {
            vec![
                Operation::SetLink { id: n.id, link: to },
                Operation::SetSwap {
                    id: n.id,
                    swap: None,
                },
            ]
        } else {
            // A swap made on what it climbs past is its to carry, unless that
            // main goes too.
            crate::swap::land_ops_through(doc, n, to, met.filter(|s| !gone.contains(s)))
        };
        ops.dedup();
        by_node.push((n.id, ops));
    }
    // Deterministic order, so the same edit produces the same transaction.
    by_node.sort_by_key(|(id, _)| *id);
    by_node.into_iter().flat_map(|(_, ops)| ops).collect()
}

/// The ops that keep every link valid when the subtrees under `deleted` go — the
/// [`relink_past`] the Delete verb owes. Empty in a document with no instances.
pub fn relink_for_delete(doc: &Document, deleted: &[NodeId]) -> Vec<Operation> {
    let gone: FxHashSet<NodeId> = crate::build::subtree_nodes(doc, deleted)
        .into_iter()
        .collect();
    relink_past(doc, &gone, |id| !gone.contains(&id))
}

/// Detach the instance rooted at `root`: it becomes ordinary layers that keep
/// their current look (§5.3d).
///
/// The root and every node that belongs to it are cut loose. A nested instance
/// inside it stays an instance of its own main: its links climb past this
/// instance's main to the nested main's nodes ([`relink_past`]). Links to
/// anything else are left alone. `None` when `root` is not an instance root.
///
/// A swapped root is detached from what it shows: its members are copies of its
/// swap's nodes, so those are the links cut, and its own link and swap both go. A
/// swapped copy nested inside it lands by `swap::landed`.
pub fn detach(doc: &Document, root: NodeId) -> Option<Transaction> {
    let nodes = doc.node_map();
    let r = nodes.get(&root)?;
    if !is_instance_root(nodes, r) {
        return None;
    }
    let src = r.swap.or(r.link)?;
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
    let mut ops = vec![Operation::SetLink {
        id: root,
        link: None,
    }];
    if r.swap.is_some() {
        ops.push(Operation::SetSwap {
            id: root,
            swap: None,
        });
    }
    for id in crate::build::subtree_nodes(doc, &[root]) {
        if id == root {
            continue;
        }
        let Some(n) = nodes.get(&id) else { continue };
        let Some(link) = n.link else {
            continue;
        };
        if !past.contains(&link) {
            continue; // a local instance of some other main: not this instance's link
        }
        let (to, met) = if owner(id) == Some(root) {
            // The root and its own members: cut. Climbing would hand a member a
            // link with no instance root above it once the root is cut.
            (None, None)
        } else {
            // A nested instance copied inside this one, and its members: climb
            // past this instance's main to the nested main's own nodes — taking
            // a swap made on what it climbs past (`swap::land_ops_through`).
            crate::swap::climb(doc, link, |s| past.contains(&s))
        };
        ops.extend(crate::swap::land_ops_through(doc, n, to, met));
    }
    Some(Transaction(ops))
}

/// The links a transaction's structural edits owe (§5.3d), so that no door that
/// moves, deletes or regroups layers has to know about components.
/// `EditorSession::commit_inner` runs it over every transaction.
///
/// The transaction is applied to a scratch copy (`Document::apply_unchecked`) and
/// the tree it leaves is asked two things:
/// - links into deleted nodes climb past them ([`relink_past`]'s rule) — the
///   Delete verb does this itself, but `build::ungroup`, *Flatten*, *Outline* and
///   text-on-a-new-path delete nodes too, and an ungrouped main is a deleted main;
/// - every linked node still belongs to its instance: an instance root above it
///   whose source contains its source, and no other node of that instance on the
///   same source. What fails is cut, except a nested copy whose outer instance
///   lost its link in this edit, which climbs past its source as [`detach`] does,
///   carrying a swap on a node it passes (`swap::land_ops_through`). Asked
///   top-down, so a cut nested root cuts its members; where two nodes of one
///   instance share a source, the one the transaction did not touch keeps it.
///
/// A link straight to a main is placed anywhere, as [`check`] allows. Empty for a
/// document with no links or a transaction with no structural op.
///
/// It also settles the swaps it leaves (`swap::tidy`): once the links are settled
/// a swap that no longer stands — its main gone, its node no longer linked, or
/// linked straight to a main — is cleared or turned into the link it stands for.
/// That runs before the membership pass, which reads a swapped root's members
/// against its swap, and also for a transaction that only writes links or swaps
/// where the document holds a swap.
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
    let relinks = tx.0.iter().any(|op| {
        matches!(
            op,
            Operation::SetLink { .. } | Operation::SetSwap { .. } | Operation::SetComponent { .. }
        )
    }) && before.values().any(|n| n.swap.is_some());
    if !(structural || relinks) || !before.values().any(|n| n.link.is_some()) {
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
        let mut climbs: Vec<(NodeId, Vec<Operation>)> = scratch
            .node_map()
            .values()
            .filter_map(|n| {
                let link = n.link.filter(|l| gone.contains(l))?;
                // Walked on the document before the edit, where the gone nodes
                // still are; a swap one of them carried is the climber's.
                let (to, met) = crate::swap::climb(doc, link, |s| gone.contains(&s));
                let met = met.filter(|s| !gone.contains(s));
                Some((n.id, crate::swap::land_ops_through(&scratch, n, to, met)))
            })
            .collect();
        climbs.sort_by_key(|(id, _)| *id);
        let climbs: Vec<Operation> = climbs.into_iter().flat_map(|(_, o)| o).collect();
        if scratch
            .apply_unchecked(&Transaction(climbs.clone()))
            .is_err()
        {
            return Vec::new();
        }
        ops.extend(climbs);
    }
    // The swaps that no longer stand, settled before membership reads them.
    let tidied = crate::swap::tidy(&scratch);
    if !tidied.is_empty() {
        if scratch
            .apply_unchecked(&Transaction(tidied.clone()))
            .is_err()
        {
            return ops;
        }
        ops.extend(tidied);
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
            // A swapped root's members are copies of its swap's nodes or of a
            // layer its slot adds, as in `check`.
            let root_src = |r: NodeId| nodes[&r].swap.or(nodes[&r].link).expect("a root is linked");
            let slot_own = |r: NodeId| {
                nodes[&r].swap.is_some() && nodes[&r].link.is_some_and(|l| slot_adds(nodes, l, src))
            };
            match root {
                Some(r) if inside(src, root_src(r)) || slot_own(r) => {
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
    // A nested copy whose outer instance this edit detached climbs, as `detach`
    // climbs it: on an ungroup the nested source survives (lifted out), so
    // nothing climbed and the membership cut it to plain layers. Only where the
    // copy's outer root is still around it and lost its link here; a nested copy
    // the user moved out of its instance is still cut. The walk is `detach`'s,
    // swap and all (`swap::climb`, `land_ops_through`): a copy following a swap
    // made one link up must take that swap, or it shows the wrong main while its
    // layers link elsewhere and `check` refuses the ungroup as `Membership`.
    let detached: Vec<NodeId> = {
        let mut v: Vec<NodeId> = cut.iter().copied().collect();
        v.sort();
        v
    };
    let mut climbed: Vec<(NodeId, Vec<Operation>)> = Vec::new();
    for id in detached {
        let Some(b) = before.get(&id) else { continue };
        let Some(src) = b
            .link
            .filter(|s| before.get(s).is_some_and(|s| !s.component))
        else {
            continue;
        };
        if !is_instance_root(before, b) || !cut.contains(&id) {
            continue;
        }
        let was_parent = |a: NodeId| before.get(&a).and_then(|n| n.parent);
        let owner = std::iter::successors(was_parent(id), |a| was_parent(*a))
            .find(|a| before.get(a).is_some_and(|n| is_instance_root(before, n)));
        let Some(owner) = owner else { continue };
        let detached_here = before[&owner].link.is_some()
            && nodes.get(&owner).is_some_and(|o| o.link.is_none())
            && inside(id, owner);
        if !detached_here {
            continue;
        }
        let past: FxHashSet<NodeId> = crate::build::subtree_nodes(doc, &[src])
            .into_iter()
            .collect();
        for k in crate::build::subtree_nodes(&scratch, &[id]) {
            let Some(kn) = nodes.get(&k) else { continue };
            if !cut.contains(&k) {
                continue;
            }
            let Some(link) = kn.link else { continue };
            let (to, met) = crate::swap::climb(doc, link, |s| past.contains(&s));
            if to.is_some() && to != kn.link {
                cut.remove(&k);
                climbed.push((k, crate::swap::land_ops_through(&scratch, kn, to, met)));
            }
        }
    }
    climbed.sort_by_key(|(id, _)| *id);
    ops.extend(climbed.into_iter().flat_map(|(_, o)| o));
    let mut cut: Vec<NodeId> = cut.into_iter().collect();
    cut.sort();
    for id in cut {
        ops.push(Operation::SetLink { id, link: None });
        // A swapped copy cut loose is plain layers, as an unswapped one is: its
        // members are cut with it.
        if nodes.get(&id).is_some_and(|n| n.swap.is_some()) {
            ops.push(Operation::SetSwap { id, swap: None });
        }
    }
    ops
}

/// How a copy of a main lands: as an instance of it, which is what copy and
/// paste, duplicate and Alt-drag do, or as a new main — *Duplicate as component*.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MainCopy {
    Instance,
    NewMain,
}

/// Settle the links of `copy` — `template` run through `remap_subtree`, index for
/// index — for where it is about to land in `doc`. Answers whether the copy became
/// an instance of the main it was copied from.
///
/// - A copy of a main in this document becomes an instance of it under
///   [`MainCopy::Instance`]: every copied node links to the node it was copied
///   from, and the copy is not itself a main. Its names stay the main's, which is
///   why the caller skips the copy-numbering then.
/// - Otherwise links are settled one at a time. A link `remap_subtree` pointed
///   inside the copy stays (a main copied with its instance). A link to a node not
///   in this document is dropped (a paste from another document). An instance
///   root's link stays: its copy is a new instance of the same source. A member's
///   link stays only if its instance root came along; a member copied on its own
///   becomes the copy's own layer, which keeps a duplicate inside its own
///   instance from claiming its original's source (`LinkRule::SharedSource`).
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
            c.make_copy_of(t.id);
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
            // A dropped link takes the swap with it: a swapped copy whose slot is
            // not here lands as an instance of what it shows when that main is
            // (`swap::landed`), else as plain layers. `settle_links`' tidy is not
            // reached for a document holding no link before the paste.
            (c.link, c.swap) = crate::swap::landed(doc, c, None);
            if c.link.is_none() {
                c.swap = None;
            }
        }
    }
    // A nested instance copied on its own — its root linked to the nested copy
    // inside some outer main, with no outer instance coming along — belongs to no
    // instance where it lands, so it climbs one level up, `detach`'s rule: its
    // root and members relink past that nested copy to the nested main's own
    // nodes. This covers every such nested instance in the copy, not only one at
    // its root, or a member group holding one lands half-linked.
    let climbers: Vec<&Node> = template
        .iter()
        .filter(|t| {
            !root_above(t)
                && is_instance_root(nodes, t)
                && t.link
                    .and_then(|s| nodes.get(&s))
                    .is_some_and(|s| !s.component)
        })
        .collect();
    for t in climbers {
        let src = t.link.expect("filtered to linked");
        let past: FxHashSet<NodeId> = crate::build::subtree_nodes(doc, &[src])
            .into_iter()
            .collect();
        // The template nodes at or under `t`.
        let mut under: FxHashSet<NodeId> = FxHashSet::default();
        under.insert(t.id);
        for n in template {
            let mut at = n.parent;
            while let Some(p) = at {
                if p == t.id {
                    under.insert(n.id);
                    break;
                }
                at = in_template.get(&p).and_then(|a| a.parent);
            }
        }
        for (c, n) in copy.iter_mut().zip(template) {
            if !under.contains(&n.id) {
                continue;
            }
            let mut to = c.link;
            for _ in 0..=nodes.len() {
                match to {
                    Some(s) if past.contains(&s) => to = nodes.get(&s).and_then(|s| s.link),
                    _ => break,
                }
            }
            // A swapped copy landing alone is an instance of what it shows
            // (`swap::landed`); its members already link there.
            (c.link, c.swap) = crate::swap::landed(doc, c, to);
        }
    }
    false
}
