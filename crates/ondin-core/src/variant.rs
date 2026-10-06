//! Variants and component properties (§5.3d build step 7, §15 D982).
//!
//! **A variant is a main.** A *component set* is a frame carrying
//! [`crate::Node::set`] — its variant properties, each a name and an ordered list
//! of values — and every main directly inside it is a variant, carrying
//! [`crate::Node::variant`]: one value per property, in the set's order. Nothing
//! else about a variant is new: an instance of one is an instance of a main,
//! linked, propagated and reset exactly as §5.3d says, which is the whole reason
//! the set is a frame around mains rather than a main of its own.
//!
//! **A variant's name is derived** — its values joined by `", "` — and kept so by
//! [`settle`], the commit-time pass, so a rename of a value renames every variant
//! holding it, and through the compare rule every instance that still carries the
//! variant's name (an instance's name is copied verbatim — §5.3d's rule under §15
//! D979 (e), held by the maintainer over the mockup's set-name label, §15 D982).
//!
//! **A component property is a view over fields** (the session's): a boolean is
//! the visibility of the layers it is bound to, a text property their content.
//! Nothing stores a property's value on an instance — the value *is* the bound
//! field of the instance's counterpart, an overridden property is a bound field
//! that differs from its source, and resetting one is resetting that field. So
//! a variant switch carries properties with every other override, and undo, the
//! preview and propagation need nothing new. The properties live on the
//! **owner**: the set for a variant, the main itself otherwise.

use crate::document::Document;
use crate::id::{IdSource, NodeId};
use crate::item::{ItemId, Keyed};
use crate::node::{Node, NodeKind};
use crate::op::{Operation, Transaction};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

/// A component set's variant properties: each a name and the ordered values a
/// variant may take for it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantSet {
    pub props: Vec<VariantProp>,
}

/// One variant property — `Size`, with `Small` and `Large`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantProp {
    pub name: String,
    pub values: Vec<String>,
}

/// What a component property drives on the layers bound to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PropKind {
    /// Their visibility.
    Boolean,
    /// Their text content (text layers only).
    Text,
    /// The main they show — an **instance swap** (§15 D983), bound to nested
    /// instances inside the main; on an instance, the copies of those swap.
    Swap,
    /// Not a value but a **showing** (§15 D988, `design/Variants.dc.html` 4K–4S):
    /// the nested instances inside the main whose own properties an instance's
    /// card shows under a sub-heading of their own, without selecting them. At
    /// most one per owner, bound to every shown slot in every variant, and
    /// **nameless** — the card lists it as *Shown from nested*, never by a name,
    /// so the naming rules pass it by.
    Nested,
}

/// A component property: a name, a kind, and the layers inside the owner it is
/// bound to. Its default is not stored — it is the main's own value of the bound
/// field, which is what an instance compares against anyway.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Property {
    pub name: String,
    pub kind: PropKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty", with = "wire_ids")]
    pub bound: Vec<NodeId>,
    /// A swap property's **filter** (§15 D983 (5)): the picker offers the mains
    /// whose name starts with it, compared without case, every main when it is
    /// empty. It scopes what is offered, never what is valid — a swap made before
    /// the filter changed stands. Empty, and unwritten, on the other kinds.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub filter: String,
}

/// Node ids in a saved property, written as wire strings like every other id in
/// the format.
mod wire_ids {
    use crate::id::NodeId;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(ids: &[NodeId], s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(ids.iter().map(|id| id.to_wire()))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<NodeId>, D::Error> {
        let raw: Vec<String> = Vec::deserialize(d)?;
        raw.iter()
            .map(|s| {
                NodeId::from_wire(s)
                    .ok_or_else(|| serde::de::Error::custom(format!("bad node id {s:?}")))
            })
            .collect()
    }
}

/// The name a variant with `values` carries: its values, joined.
pub fn derived_name(values: &[String]) -> String {
    values.join(", ")
}

/// Whether `id` is a component set.
pub fn is_set(doc: &Document, id: NodeId) -> bool {
    doc.get(id).is_some_and(|n| n.set.is_some())
}

/// The set `id` is a variant of: its parent, when `id` is a main and the parent a
/// set.
pub fn set_of(doc: &Document, id: NodeId) -> Option<NodeId> {
    let n = doc.get(id)?;
    let p = doc.get(n.parent?)?;
    (n.component && p.set.is_some()).then_some(p.id)
}

/// The variants of `set` — its main children, in z-order.
pub fn variants(doc: &Document, set: NodeId) -> Vec<NodeId> {
    doc.get(set)
        .map(|s| {
            s.children
                .iter()
                .copied()
                .filter(|c| doc.get(*c).is_some_and(|c| c.component))
                .collect()
        })
        .unwrap_or_default()
}

/// The variant of `set` whose values are `values`, the first in z-order when
/// several clash.
pub fn find_variant(doc: &Document, set: NodeId, values: &[String]) -> Option<NodeId> {
    variants(doc, set)
        .into_iter()
        .find(|v| doc.get(*v).is_some_and(|v| v.variant == values))
}

/// The variants of `set` that share their values with another — the clash the
/// tab and the card warn about. Empty for a set with none.
pub fn clashes(doc: &Document, set: NodeId) -> Vec<NodeId> {
    let vs = variants(doc, set);
    let mut seen: FxHashMap<&[String], usize> = FxHashMap::default();
    for v in &vs {
        *seen
            .entry(&doc.get(*v).expect("a variant").variant)
            .or_default() += 1;
    }
    vs.iter()
        .copied()
        .filter(|v| seen[doc.get(*v).expect("a variant").variant.as_slice()] > 1)
        .collect()
}

/// Where a component's properties live: the set, for a variant; the main itself
/// otherwise. `None` for a node that is neither a main nor a set.
pub fn owner_of_main(doc: &Document, main: NodeId) -> Option<NodeId> {
    let n = doc.get(main)?;
    if n.set.is_some() {
        return Some(main);
    }
    if !n.component {
        return None;
    }
    Some(set_of(doc, main).unwrap_or(main))
}

/// The owner whose properties a layer **inside** a main may be bound to — the
/// nearest main strictly above it, or that main's set. `None` outside every main
/// and for a main's own root.
pub fn owner_above(doc: &Document, id: NodeId) -> Option<NodeId> {
    let mut at = doc.get(id)?.parent;
    while let Some(n) = at.and_then(|a| doc.get(a)) {
        if n.component {
            return owner_of_main(doc, n.id);
        }
        at = n.parent;
    }
    None
}

// ── The rules (`component::check` calls this) ─────────────────────────────────

/// Which variant or property rule a document broke. Carried inside
/// `component::LinkRule::Variant`, so `OpError::BadLink` and the loader say it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VariantRule {
    /// `set` on a node that is not a frame, or on a main, a linked node, or one
    /// inside a main, an instance or another set.
    SetKind,
    /// A set's property names, or one property's values, are empty or repeat; or a
    /// property has no values.
    SetNames,
    /// A main inside a set whose values do not fit it, or `variant` on a node
    /// that is not a main inside a set.
    Values,
    /// Properties on a node that is neither a main nor a set, or on a variant
    /// (whose properties are its set's).
    PropertyOwner,
    /// A property's name is empty or repeats among the owner's properties, or
    /// names one of a set's variant properties.
    PropertyName,
    /// A bound layer is missing, is not strictly inside a main of the owner, is
    /// not text for a text property, or is bound twice for the same field.
    Binding,
}

impl std::fmt::Display for VariantRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::SetKind => "is a component set but not a frame on its own",
            Self::SetNames => "has a variant property or value that is empty or repeats",
            Self::Values => "has variant values that do not fit its set",
            Self::PropertyOwner => "has component properties but is not a main or a set",
            Self::PropertyName => "has a component property whose name is empty or repeats",
            Self::Binding => "binds a component property to a layer it cannot drive",
        })
    }
}

/// Check the variant and property rules over `nodes`.
///
/// Free when nothing is a set, a variant or a property's owner — one scan.
pub(crate) fn check(nodes: &FxHashMap<NodeId, Node>) -> Result<(), (NodeId, VariantRule)> {
    if !nodes
        .values()
        .any(|n| n.set.is_some() || !n.variant.is_empty() || !n.props.is_empty())
    {
        return Ok(());
    }
    let parent = |id: NodeId| nodes.get(&id).and_then(|n| n.parent);
    let ancestors = |id: NodeId| std::iter::successors(parent(id), move |p| parent(*p));
    for n in nodes.values() {
        if let Some(set) = &n.set {
            let nested = ancestors(n.id).any(|a| {
                nodes
                    .get(&a)
                    .is_some_and(|a| a.component || a.link.is_some() || a.set.is_some())
            });
            if !matches!(n.kind, NodeKind::Artboard { .. })
                || n.component
                || n.link.is_some()
                || nested
            {
                return Err((n.id, VariantRule::SetKind));
            }
            if !names_ok(set.props.iter().map(|p| p.name.as_str()))
                || set
                    .props
                    .iter()
                    .any(|p| p.values.is_empty() || !names_ok(p.values.iter().map(String::as_str)))
            {
                return Err((n.id, VariantRule::SetNames));
            }
        }
        let in_set = n
            .parent
            .and_then(|p| nodes.get(&p))
            .and_then(|p| p.set.as_ref());
        match (n.component, in_set) {
            (true, Some(set)) => {
                let fits = n.variant.len() == set.props.len()
                    && n.variant
                        .iter()
                        .zip(&set.props)
                        .all(|(v, p)| p.values.contains(v));
                if !fits {
                    return Err((n.id, VariantRule::Values));
                }
            }
            _ if !n.variant.is_empty() => return Err((n.id, VariantRule::Values)),
            _ => {}
        }
        if !n.props.is_empty() {
            let owner = n.set.is_some() || (n.component && in_set.is_none());
            if !owner {
                return Err((n.id, VariantRule::PropertyOwner));
            }
            let reserved: Vec<&str> = n
                .set
                .iter()
                .flat_map(|s| s.props.iter().map(|p| p.name.as_str()))
                .collect();
            // The showing is nameless and one (`PropKind::Nested`).
            let named = || n.props.iter().filter(|p| p.kind != PropKind::Nested);
            if !names_ok(named().map(|p| p.name.as_str()))
                || named().any(|p| reserved.contains(&p.name.as_str()))
                || n.props
                    .iter()
                    .filter(|p| p.kind == PropKind::Nested)
                    .count()
                    > 1
            {
                return Err((n.id, VariantRule::PropertyName));
            }
            let mut fields: FxHashSet<(NodeId, PropKind)> = FxHashSet::default();
            for p in n.props.iter() {
                for b in &p.bound {
                    let ok = nodes.get(b).is_some_and(|bn| {
                        binding_fits(nodes, n.id, bn) && kind_fits(nodes, p.kind, bn)
                    });
                    if !ok || !fields.insert((*b, p.kind)) {
                        return Err((n.id, VariantRule::Binding));
                    }
                }
            }
        }
    }
    Ok(())
}

fn names_ok<'a>(names: impl Iterator<Item = &'a str>) -> bool {
    let mut seen: FxHashSet<&str> = FxHashSet::default();
    for n in names {
        if n.trim().is_empty() || !seen.insert(n) {
            return false;
        }
    }
    true
}

fn is_text(n: &Node) -> bool {
    matches!(n.kind, NodeKind::Text { .. })
}

/// Whether a property of `kind` can drive `bound`: a text property only a text
/// layer; a swap, and a showing, only a **nested instance** inside the main — a
/// layer linked straight to a main, whose copies in an instance are what swap
/// (§15 D983) and what an instance's card shows the properties of (§15 D988).
fn kind_fits(nodes: &FxHashMap<NodeId, Node>, kind: PropKind, bound: &Node) -> bool {
    match kind {
        PropKind::Boolean => true,
        PropKind::Text => is_text(bound),
        PropKind::Swap | PropKind::Nested => bound
            .link
            .and_then(|l| nodes.get(&l))
            .is_some_and(|l| l.component),
    }
}

/// Whether `bound` may carry a property of `owner`: strictly inside the owner's
/// main — for a set, strictly inside one of its variants, never a variant's root.
fn binding_fits(nodes: &FxHashMap<NodeId, Node>, owner: NodeId, bound: &Node) -> bool {
    let mut at = bound.parent;
    while let Some(p) = at.and_then(|a| nodes.get(&a)) {
        if p.component {
            return p.id == owner
                || p.parent
                    .is_some_and(|pp| pp == owner && nodes[&owner].set.is_some());
        }
        at = p.parent;
    }
    false
}

// ── The commit-time pass ─────────────────────────────────────────────────────

/// What a transaction owes the variant and property rules, so that no door —
/// a drag into a set, a delete, a duplicate, an undo-free paste — has to know
/// about them. `component::settle_links`' shape: the transaction is applied to a
/// scratch copy and the tree it leaves is asked:
///
/// - **every variant fits its set**: a main that arrived in a set keeps the values
///   it brought where they still fit, takes the property's first value where one
///   does not, and takes the **first free combination** when it brought none; a
///   node that left its set, or stopped being a main, drops its values;
/// - **every variant carries its derived name** ([`derived_name`]) — written here,
///   before the propagation pass, so instances still holding the old name follow;
/// - **properties sit on an owner**: a variant's move to its set, merged by name
///   and kind; a node that is no longer a main or a set loses them;
/// - **every binding still fits**: a bound layer deleted, moved out of the
///   owner's mains, or bound twice for one field is unbound.
///
/// Empty for a document with no set, variant or property and a transaction that
/// makes none.
pub fn settle(doc: &Document, tx: &Transaction) -> Vec<Operation> {
    let makes = tx.0.iter().any(|op| {
        matches!(
            op,
            Operation::SetVariantSet { .. }
                | Operation::SetVariant { .. }
                | Operation::SetProperties { .. }
        )
    });
    // **Only an edit that can break a rule pays for the scratch copy** — the
    // clone is this pass's whole cost and the commit runs it on every edit: the
    // tree changing shape, a main made or unmade, or a variant renamed by hand.
    let can_break = makes
        || tx.0.iter().any(|op| match op {
            Operation::CreateNode { .. }
            | Operation::DeleteNode { .. }
            | Operation::InsertSubtree { .. }
            | Operation::Reparent { .. }
            | Operation::SetComponent { .. } => true,
            Operation::SetName { id, .. } => doc.get(*id).is_some_and(|n| !n.variant.is_empty()),
            _ => false,
        });
    let relevant = |n: &Node| n.set.is_some() || !n.variant.is_empty() || !n.props.is_empty();
    if !can_break || (!makes && !doc.node_map().values().any(relevant)) {
        return Vec::new();
    }
    let mut scratch = doc.clone();
    if scratch.apply_unchecked(tx).is_err() {
        return Vec::new(); // `apply` will refuse it with the reason
    }
    let mut ops = Vec::new();
    let mut ids: Vec<NodeId> = scratch.node_map().keys().copied().collect();
    ids.sort();

    // Values: off a node that is not a variant; fitted onto one that is.
    let mut props_moved: FxHashMap<NodeId, Vec<Keyed<Property>>> = FxHashMap::default();
    for &id in &ids {
        let n = scratch.get(id).expect("listed");
        let set = n
            .parent
            .and_then(|p| scratch.get(p))
            .filter(|p| n.component && p.set.is_some());
        match set {
            None if !n.variant.is_empty() => ops.push(Operation::SetVariant {
                id,
                values: Vec::new(),
            }),
            None => {}
            Some(_) if !n.props.is_empty() => {
                props_moved.insert(id, n.props.clone());
            }
            Some(_) => {}
        }
        if !n.props.is_empty() && !n.component && n.set.is_none() {
            ops.push(Operation::SetProperties {
                id,
                props: Vec::new(),
            });
        }
    }
    let mut sets: Vec<NodeId> = ids
        .iter()
        .copied()
        .filter(|id| scratch.get(*id).is_some_and(|n| n.set.is_some()))
        .collect();
    sets.sort();
    for &s in &sets {
        let set = scratch.get(s).and_then(|n| n.set.clone()).expect("a set");
        let vs = variants(&scratch, s);
        let fits = |v: &[String]| {
            v.len() == set.props.len()
                && v.iter().zip(&set.props).all(|(x, p)| p.values.contains(x))
        };
        let mut taken: FxHashSet<Vec<String>> = vs
            .iter()
            .filter_map(|v| scratch.get(*v).map(|n| n.variant.clone()))
            .filter(|v| fits(v))
            .collect();
        for &v in &vs {
            let cur = scratch.get(v).expect("a variant").variant.clone();
            let next = if fits(&cur) {
                cur.clone()
            } else if cur.len() == set.props.len() {
                cur.iter()
                    .zip(&set.props)
                    .map(|(x, p)| {
                        if p.values.contains(x) {
                            x.clone()
                        } else {
                            p.values[0].clone()
                        }
                    })
                    .collect()
            } else {
                let free = first_free(&set, &taken);
                taken.insert(free.clone());
                free
            };
            if next != cur {
                ops.push(Operation::SetVariant {
                    id: v,
                    values: next.clone(),
                });
            }
            let name = derived_name(&next);
            if scratch.get(v).is_some_and(|n| n.name != name) {
                ops.push(Operation::SetName { id: v, name });
            }
        }
        // A variant's properties are its set's: merged by name and kind.
        let mut props = scratch.get(s).expect("a set").props.clone();
        let mut grew = false;
        for v in vs {
            let Some(moved) = props_moved.remove(&v) else {
                continue;
            };
            ops.push(Operation::SetProperties {
                id: v,
                props: Vec::new(),
            });
            for p in moved {
                match props
                    .iter_mut()
                    .find(|q| q.kind == p.kind && (q.name == p.name || p.kind == PropKind::Nested))
                {
                    Some(q) => q.bound.extend(p.bound.iter().copied()),
                    None => props.push(p),
                }
                grew = true;
            }
        }
        if grew {
            ops.push(Operation::SetProperties { id: s, props });
        }
    }
    // Apply what is decided so far, and prune the bindings on the result.
    if scratch.apply_unchecked(&Transaction(ops.clone())).is_err() {
        return ops;
    }
    let nodes = scratch.node_map();
    for &id in &ids {
        let Some(n) = nodes.get(&id) else { continue };
        if n.props.is_empty() {
            continue;
        }
        let reserved: Vec<String> = n
            .set
            .iter()
            .flat_map(|s| s.props.iter().map(|p| p.name.clone()))
            .collect();
        let mut fields: FxHashSet<(NodeId, PropKind)> = FxHashSet::default();
        let mut names: FxHashSet<String> = FxHashSet::default();
        let mut next: Vec<Keyed<Property>> = Vec::new();
        for p in n.props.iter() {
            let mut p = p.clone();
            let kind = p.kind;
            p.bound.retain(|b| {
                nodes
                    .get(b)
                    .is_some_and(|bn| binding_fits(nodes, id, bn) && kind_fits(nodes, kind, bn))
                    && fields.insert((*b, kind))
            });
            // The showing is nameless and merges by kind alone: one per owner.
            if kind == PropKind::Nested {
                match next.iter_mut().find(|q| q.kind == PropKind::Nested) {
                    Some(q) => q.bound.extend(p.bound.iter().copied()),
                    None => {
                        p.name = String::new();
                        next.push(p);
                    }
                }
                continue;
            }
            // A merge can bring a name in twice, or onto a variant property's.
            let mut name = p.name.clone();
            let mut k = 2;
            while name.trim().is_empty() || reserved.contains(&name) || names.contains(&name) {
                name = format!("{} {k}", p.name.trim());
                k += 1;
            }
            names.insert(name.clone());
            p.name = name;
            next.push(p);
        }
        // A showing with nothing left to show goes, as `set_shown` takes it away
        // with its last slot — or a main with no other property would keep a
        // disabled *Reset properties* for it (§15 D988).
        next.retain(|q| !(q.kind == PropKind::Nested && q.bound.is_empty()));
        if next != n.props {
            ops.push(Operation::SetProperties { id, props: next });
        }
    }
    ops
}

/// The first combination of `set`'s values, in reading order, that `taken` does
/// not hold — or the first combination of all when every one is taken.
pub fn first_free(set: &VariantSet, taken: &FxHashSet<Vec<String>>) -> Vec<String> {
    let mut at = vec![0usize; set.props.len()];
    let first: Vec<String> = set.props.iter().map(|p| p.values[0].clone()).collect();
    loop {
        let combo: Vec<String> = at
            .iter()
            .zip(&set.props)
            .map(|(i, p)| p.values[*i].clone())
            .collect();
        if !taken.contains(&combo) {
            return combo;
        }
        // Odometer, the last property turning fastest.
        let mut k = set.props.len();
        loop {
            if k == 0 {
                return first;
            }
            k -= 1;
            at[k] += 1;
            if at[k] < set.props[k].values.len() {
                break;
            }
            at[k] = 0;
        }
    }
}

// ── Making and editing a set ─────────────────────────────────────────────────

/// The space a variant is placed below the one it was copied from.
pub const VARIANT_GAP: f64 = 24.0;

/// *Combine as variants*: wrap `mains` in a new frame and make it a set whose one
/// property, `Property 1`, takes each main's name as a value (§15 D982). Mains
/// whose names repeat share a value and so clash, which the set then shows.
/// Answers the transaction and the set's id; refused unless every member is a
/// main not already in a set, and all share a parent (`build::frame`'s rule).
pub fn combine(
    doc: &Document,
    res: &crate::resolve::Resolved,
    ids: &mut IdSource,
    mains: &[NodeId],
) -> Result<(Transaction, NodeId), crate::op::OpError> {
    use crate::op::OpError;
    if mains.is_empty()
        || mains
            .iter()
            .any(|m| !doc.get(*m).is_some_and(|n| n.component) || set_of(doc, *m).is_some())
    {
        return Err(OpError::WrongKindForOp);
    }
    // In z-order, so the values read in the order the layers stack.
    let parent = doc.get(mains[0]).and_then(|n| n.parent);
    let mut ordered: Vec<NodeId> = parent
        .and_then(|p| doc.get(p))
        .map(|p| {
            p.children
                .iter()
                .copied()
                .filter(|c| mains.contains(c))
                .collect()
        })
        .unwrap_or_default();
    if ordered.len() != mains.len() {
        ordered = mains.to_vec();
    }
    let (mut tx, set) = crate::build::frame(doc, res, ids, &ordered)?;
    let names: Vec<String> = ordered
        .iter()
        .map(|m| doc.get(*m).map(|n| n.name.clone()).unwrap_or_default())
        .map(|n| {
            if n.trim().is_empty() {
                "Variant".to_string()
            } else {
                n
            }
        })
        .collect();
    let mut values: Vec<String> = Vec::new();
    for n in &names {
        if !values.contains(n) {
            values.push(n.clone());
        }
    }
    tx.0.push(Operation::SetVariantSet {
        id: set,
        set: Some(VariantSet {
            props: vec![VariantProp {
                name: "Property 1".into(),
                values,
            }],
        }),
    });
    for (m, n) in ordered.iter().zip(&names) {
        tx.0.push(Operation::SetVariant {
            id: *m,
            values: vec![n.clone()],
        });
    }
    tx.0.push(Operation::SetName {
        id: set,
        name: set_name(&names),
    });
    Ok((tx, set))
}

/// A new set's name: the part its variants' names share before a separator, or
/// the first name when they share none.
fn set_name(names: &[String]) -> String {
    let first = names.first().cloned().unwrap_or_default();
    let mut common = first.clone();
    for n in &names[1..] {
        let len = common
            .chars()
            .zip(n.chars())
            .take_while(|(a, b)| a == b)
            .count();
        common = common.chars().take(len).collect();
    }
    let trimmed = common
        .trim_end_matches(|c: char| c == '/' || c == ',' || c == '-' || c.is_whitespace())
        .to_string();
    if names.len() > 1 && !trimmed.is_empty() && trimmed.len() < first.len() {
        trimmed
    } else {
        first
    }
}

/// *Add variant*: a copy of `from` (or the set's last variant) as a new main in
/// the set, below its source, taking the **first free combination**, with every
/// set property bound in the source bound in the copy too (§15 D982). The set
/// grows to hold it. Answers the transaction and the new variant.
pub fn add_variant(
    doc: &Document,
    res: &crate::resolve::Resolved,
    ids: &mut IdSource,
    set: NodeId,
    from: Option<NodeId>,
) -> Option<(Transaction, NodeId)> {
    let s = doc.get(set)?;
    let vs = s.set.clone()?;
    let source = from.or_else(|| variants(doc, set).last().copied())?;
    let template = doc.capture_subtree(source)?;
    let index = s.children.iter().position(|c| *c == source)? + 1;
    let height = crate::query::local_box(doc, res, source)?.height();
    let (mut tx, made) = crate::build::insert_subtrees_as(
        doc,
        ids,
        &[crate::build::Placement {
            nodes: template.clone(),
            parent: set,
            index: Some(index),
        }],
        kurbo::Vec2::new(0.0, height + VARIANT_GAP),
        crate::component::MainCopy::NewMain,
    );
    let new = *made.first()?;
    let copy: Vec<NodeId> = tx
        .0
        .iter()
        .find_map(|op| match op {
            Operation::InsertSubtree { nodes, .. } => Some(nodes.iter().map(|n| n.id).collect()),
            _ => None,
        })
        .unwrap_or_default();
    let map: FxHashMap<NodeId, NodeId> = template
        .iter()
        .map(|n| n.id)
        .zip(copy.iter().copied())
        .collect();
    let taken: FxHashSet<Vec<String>> = variants(doc, set)
        .iter()
        .filter_map(|v| doc.get(*v).map(|n| n.variant.clone()))
        .collect();
    let values = first_free(&vs, &taken);
    tx.0.push(Operation::SetVariant {
        id: new,
        values: values.clone(),
    });
    tx.0.push(Operation::SetName {
        id: new,
        name: derived_name(&values),
    });
    if !s.props.is_empty() {
        let props: Vec<Keyed<Property>> = s
            .props
            .iter()
            .map(|p| {
                p.map(|p| {
                    let mut p = p.clone();
                    let more: Vec<NodeId> =
                        p.bound.iter().filter_map(|b| map.get(b).copied()).collect();
                    p.bound.extend(more);
                    p
                })
            })
            .collect();
        tx.0.push(Operation::SetProperties { id: set, props });
    }
    // Grow the set to hold the copy, where it is a frame with a size of its own.
    if let NodeKind::Artboard { size } = &s.kind {
        let src = doc.get(source)?;
        let local = crate::query::local_box(doc, res, source)?;
        let placed = crate::geometry::transform_rect(src.transform, local);
        let bottom = placed.y1 + height + VARIANT_GAP;
        let right = placed.x1;
        let want = kurbo::Size::new(size.width.max(right), size.height.max(bottom + VARIANT_GAP));
        if want != *size {
            tx.0.push(Operation::SetGeometry {
                id: set,
                geometry: crate::op::GeometryPatch::Size(want),
            });
        }
    }
    Some((tx, new))
}

/// A variant's values set to `values` — the variant card's dropdowns. A taken
/// combination is not refused: the clash shows instead (the design's rule).
pub fn set_values(doc: &Document, variant: NodeId, values: Vec<String>) -> Option<Transaction> {
    let set = doc.get(set_of(doc, variant)?)?.set.clone()?;
    let fits = values.len() == set.props.len()
        && values
            .iter()
            .zip(&set.props)
            .all(|(v, p)| p.values.contains(v));
    fits.then(|| {
        Transaction(vec![Operation::SetVariant {
            id: variant,
            values,
        }])
    })
}

/// The ops that write `next` as `set`'s properties and each variant's values as
/// `remap` turns them — the one shape every edit of a set's properties takes.
fn edit_set(
    doc: &Document,
    set: NodeId,
    next: VariantSet,
    remap: impl Fn(&[String]) -> Vec<String>,
) -> Transaction {
    let mut ops = vec![Operation::SetVariantSet {
        id: set,
        set: Some(next),
    }];
    for v in variants(doc, set) {
        let cur = &doc.get(v).expect("a variant").variant;
        let values = remap(cur);
        if values != *cur {
            ops.push(Operation::SetVariant { id: v, values });
        }
    }
    Transaction(ops)
}

fn fresh(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Rename `set`'s property `prop`. `None` for an empty name or one taken.
pub fn rename_property(
    doc: &Document,
    set: NodeId,
    prop: usize,
    name: &str,
) -> Option<Transaction> {
    let s = doc.get(set)?;
    let mut next = s.set.clone()?;
    let name = fresh(name)?;
    let clash = next
        .props
        .iter()
        .enumerate()
        .any(|(i, p)| i != prop && p.name == name)
        || s.props.iter().any(|p| p.name == name);
    if clash {
        return None;
    }
    next.props.get_mut(prop)?.name = name;
    Some(edit_set(doc, set, next, |v| v.to_vec()))
}

/// Rename value `value` of property `prop`, in the set and in every variant
/// holding it — each renamed by [`settle`] in the same commit.
pub fn rename_value(
    doc: &Document,
    set: NodeId,
    prop: usize,
    value: usize,
    name: &str,
) -> Option<Transaction> {
    let mut next = doc.get(set)?.set.clone()?;
    let name = fresh(name)?;
    let p = next.props.get_mut(prop)?;
    let old = p.values.get(value)?.clone();
    if p.values
        .iter()
        .enumerate()
        .any(|(i, v)| i != value && *v == name)
    {
        return None;
    }
    p.values[value] = name.clone();
    Some(edit_set(doc, set, next, |v| {
        let mut v = v.to_vec();
        if v.get(prop) == Some(&old) {
            v[prop] = name.clone();
        }
        v
    }))
}

/// Add a property named `name` whose one value, `value`, every variant takes.
pub fn add_property(doc: &Document, set: NodeId, name: &str, value: &str) -> Option<Transaction> {
    let s = doc.get(set)?;
    let mut next = s.set.clone()?;
    let (name, value) = (fresh(name)?, fresh(value)?);
    if next.props.iter().any(|p| p.name == name) || s.props.iter().any(|p| p.name == name) {
        return None;
    }
    next.props.push(VariantProp {
        name,
        values: vec![value.clone()],
    });
    Some(edit_set(doc, set, next, |v| {
        let mut v = v.to_vec();
        v.push(value.clone());
        v
    }))
}

/// Add a value to property `prop`, last.
pub fn add_value(doc: &Document, set: NodeId, prop: usize, value: &str) -> Option<Transaction> {
    let mut next = doc.get(set)?.set.clone()?;
    let value = fresh(value)?;
    let p = next.props.get_mut(prop)?;
    if p.values.contains(&value) {
        return None;
    }
    p.values.push(value);
    Some(edit_set(doc, set, next, |v| v.to_vec()))
}

/// A name for a new value of `prop` no value already has: `Value 2`, `Value 3`…
pub fn next_value_name(set: &VariantSet, prop: usize) -> String {
    let taken = |n: &str| {
        set.props
            .get(prop)
            .is_some_and(|p| p.values.iter().any(|v| v == n))
    };
    (2..)
        .map(|k| format!("Value {k}"))
        .find(|n| !taken(n))
        .expect("an unbounded range")
}

/// A name for a new property no property already has: `Property 2`…
pub fn next_property_name(set: &VariantSet) -> String {
    (1..)
        .map(|k| format!("Property {k}"))
        .find(|n| !set.props.iter().any(|p| p.name == *n))
        .expect("an unbounded range")
}

/// Remove property `prop` from the set and from every variant. Refused for the
/// last one: a set with no properties has no way to tell its variants apart.
pub fn delete_property(doc: &Document, set: NodeId, prop: usize) -> Option<Transaction> {
    let mut next = doc.get(set)?.set.clone()?;
    if next.props.len() < 2 || prop >= next.props.len() {
        return None;
    }
    next.props.remove(prop);
    Some(edit_set(doc, set, next, |v| {
        let mut v = v.to_vec();
        if prop < v.len() {
            v.remove(prop);
        }
        v
    }))
}

/// Move value `from` of property `prop` to position `to`.
pub fn move_value(
    doc: &Document,
    set: NodeId,
    prop: usize,
    from: usize,
    to: usize,
) -> Option<Transaction> {
    let mut next = doc.get(set)?.set.clone()?;
    let p = next.props.get_mut(prop)?;
    if from >= p.values.len() || to >= p.values.len() || from == to {
        return None;
    }
    let v = p.values.remove(from);
    p.values.insert(to, v);
    Some(edit_set(doc, set, next, |v| v.to_vec()))
}

/// The variants that hold value `value` of property `prop` — what deleting that
/// value deletes with it.
pub fn using_value(doc: &Document, set: NodeId, prop: usize, value: usize) -> Vec<NodeId> {
    let Some(name) = doc
        .get(set)
        .and_then(|s| s.set.as_ref())
        .and_then(|s| s.props.get(prop))
        .and_then(|p| p.values.get(value))
    else {
        return Vec::new();
    };
    variants(doc, set)
        .into_iter()
        .filter(|v| doc.get(*v).and_then(|n| n.variant.get(prop)) == Some(name))
        .collect()
}

/// Delete value `value` of property `prop`, and **the variants holding it with
/// it** — their instances detaching, as any main's deletion detaches them (§15
/// D979 (c), held over the mockup's relink by the maintainer, §15 D982).
/// Refused for a property's last value.
pub fn delete_value(doc: &Document, set: NodeId, prop: usize, value: usize) -> Option<Transaction> {
    let mut next = doc.get(set)?.set.clone()?;
    let p = next.props.get_mut(prop)?;
    if p.values.len() < 2 || value >= p.values.len() {
        return None;
    }
    p.values.remove(value);
    let gone = using_value(doc, set, prop, value);
    let mut ops = crate::component::relink_for_delete(doc, &gone);
    ops.extend(gone.iter().map(|id| Operation::DeleteNode { id: *id }));
    ops.push(Operation::SetVariantSet {
        id: set,
        set: Some(next),
    });
    Some(Transaction(ops))
}

// ── Switching an instance to another variant ──────────────────────────────────

/// The variant of the same set an instance of `main` would switch to when
/// property `prop` takes `value` — `None` when the set has no such combination
/// (the dropdown greys it, with the reason).
pub fn switch_target(doc: &Document, main: NodeId, prop: usize, value: &str) -> Option<NodeId> {
    let set = set_of(doc, main)?;
    let mut values = doc.get(main)?.variant.clone();
    *values.get_mut(prop)? = value.to_string();
    find_variant(doc, set, &values)
}

/// Whether the instance rooted at `root` can switch variants here: an instance
/// linked **straight to** a main — placed by itself, or nested inside a main —
/// which switches by relinking; or a **nested copy inside an outer instance**,
/// which switches by swapping (§15 D983 (6): the link model had no place for "the
/// counterpart of this node, but an instance of that main" until the `swap`
/// field beside `link`, of which this switch is the case restricted to one set).
pub fn can_switch(doc: &Document, root: NodeId) -> bool {
    crate::propagate::linked_to_main(doc, root) || crate::swap::can_swap(doc, root)
}

/// What [`rewrite`] does with the root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rewrite {
    /// The variant switch of a root linked straight to a main: the root is
    /// relinked to the new main, its placement kept.
    Relink,
    /// A swap (§15 D983): the root keeps its link — its slot — and everything the
    /// slot gives it, placement, size and visibility (`swap::is_slot_field`).
    Swap,
}

/// **Switch** the instance rooted at `root` to the main `to` (§15 D982) — the
/// design's variant switch, made **in place** so every id, the selection and a
/// nested copy's own copies survive it:
///
/// - layers are matched by **name path** below the two mains, the k-th sibling of
///   a name matching the k-th, and only within one kind;
/// - a matched counterpart is relinked, and each of its fields takes the new
///   main's value **where it still held the old main's** — the propagation pass's
///   own rule (`propagate::follow`), so every override carries and nothing else
///   does; a list item matches by id, else by position;
/// - a counterpart with no match goes if it still equals its source, else stays as
///   the instance's own layer (§15 D979 (b)'s rule); a layer of the new main with
///   no match is copied in at its anchor under its parent's counterpart, and not
///   at all where the instance removed that parent; one whose match the instance
///   had removed stays removed;
/// - the order follows where the instance still had the old main's.
///
/// The root's placement is its own, as always. `None` unless [`can_switch`].
///
/// **A nested copy inside an outer instance switches by swapping** (§15 D983
/// (6)): the transaction is its `SetSwap` alone, and the commit makes the same
/// rewrite this function makes for a root linked straight to a main
/// (`swap::settle`) — so the two differ in what keeps the slot, never in how the
/// layers are matched.
pub fn switch(doc: &Document, root: NodeId, to: NodeId, ids: &mut IdSource) -> Option<Transaction> {
    if !can_switch(doc, root) {
        return None;
    }
    if !crate::propagate::linked_to_main(doc, root) {
        return crate::swap::swap(doc, root, to);
    }
    let from = doc.get(root)?.link?;
    if from == to || !doc.get(to)?.component {
        return None;
    }
    rewrite(doc, root, from, to, ids, Rewrite::Relink).map(Transaction)
}

/// The in-place rewrite of the instance rooted at `root` from showing `from` to
/// showing `to` — [`switch`]'s whole body, and `swap::settle`'s. `from` and `to`
/// are the sources the root's contents were and will be copies of: a variant and
/// its sibling for a switch; for a swap, any two of the slot's counterpart and
/// the mains it is swapped between. What happens to the root itself is `mode`'s.
pub(crate) fn rewrite(
    doc: &Document,
    root: NodeId,
    from: NodeId,
    to: NodeId,
    ids: &mut IdSource,
    mode: Rewrite,
) -> Option<Vec<Operation>> {
    let paths_old = paths(doc, from);
    let paths_new = paths(doc, to);
    let by_path_new: FxHashMap<&Vec<(String, usize)>, NodeId> =
        paths_new.iter().map(|(id, p)| (p, *id)).collect();
    let same_kind = |a: NodeId, b: NodeId| {
        let (Some(a), Some(b)) = (doc.get(a), doc.get(b)) else {
            return false;
        };
        std::mem::discriminant(&a.kind) == std::mem::discriminant(&b.kind)
    };
    // Old source → new source.
    let mut matched: FxHashMap<NodeId, NodeId> = FxHashMap::default();
    matched.insert(from, to);
    for (id, p) in &paths_old {
        if let Some(n) = by_path_new.get(p)
            && same_kind(*id, *n)
        {
            matched.insert(*id, *n);
        }
    }
    let matched_new: FxHashSet<NodeId> = matched.values().copied().collect();
    // Old source → its counterpart in this instance.
    let old_sources: FxHashSet<NodeId> = crate::build::subtree_nodes(doc, &[from])
        .into_iter()
        .collect();
    // The root is `from`'s counterpart by definition: a swapped root's link names
    // its slot, which is not `from` when it is swapped away from another main.
    let mut cp: FxHashMap<NodeId, NodeId> = FxHashMap::default();
    cp.insert(from, root);
    for id in crate::build::subtree_nodes(doc, &[root]) {
        if id == root {
            continue;
        }
        if let Some(l) = doc.get(id).and_then(|n| n.link)
            && old_sources.contains(&l)
        {
            cp.insert(l, id);
        }
    }

    // **What an override is measured against: the counterpart in the main `from`
    // shows, not `from`'s own node** (§15 D991). For a variant
    // switch the two are the same — `from` is a main. For a swap they are not:
    // `from` is the slot, a copy inside the outer main, and an override made *on
    // the slot* — a Button's label set to "Save" inside a Toolbar main — is a
    // value the copy shares with the slot, so measured against the slot it read
    // as no override at all and the new main's "Button" replaced it. Walked down
    // the content sources only as far as that main and no further: a variant's
    // own overrides on a nested instance inside it are the variant's design, not
    // the instance's, and must still give way to the target's.
    let shown = crate::swap::shown_main(doc.node_map(), from).unwrap_or(from);
    let in_shown: FxHashSet<NodeId> = crate::build::subtree_nodes(doc, &[shown])
        .into_iter()
        .collect();
    let base_of = |s: NodeId| {
        let mut at = s;
        // Bounded by the document, as `shown_main` is: the walk is as long as
        // the slot's nesting, which the shown main's own size says nothing about.
        for _ in 0..=doc.node_map().len() {
            if in_shown.contains(&at) {
                return at;
            }
            match crate::swap::content_source(doc, at) {
                Some(next) => at = next,
                None => break,
            }
        }
        s
    };

    let mut deletes: Vec<NodeId> = Vec::new();
    let mut cuts: Vec<NodeId> = Vec::new();
    let mut fields: Vec<Operation> = Vec::new();
    let none = FxHashSet::default();
    let mut sorted: Vec<(NodeId, NodeId)> = cp.iter().map(|(s, c)| (*s, *c)).collect();
    sorted.sort();
    for (s_old, c) in &sorted {
        match matched.get(s_old) {
            Some(s_new) => {
                if *c != root || mode == Rewrite::Relink {
                    fields.push(Operation::SetLink {
                        id: *c,
                        link: Some(*s_new),
                    });
                }
                let keep = match (*c == root, mode) {
                    (false, _) => Keep::Nothing,
                    (true, Rewrite::Relink) => Keep::Placement,
                    (true, Rewrite::Swap) => Keep::Slot,
                };
                fields.extend(carry(doc, *c, base_of(*s_old), *s_new, keep));
            }
            None => {
                // Only the topmost unmatched counterpart decides; below it, the
                // whole subtree goes with it or is kept with it.
                let parent_unmatched = doc
                    .get(*s_old)
                    .and_then(|n| n.parent)
                    .is_some_and(|p| p != from && !matched.contains_key(&p));
                if parent_unmatched {
                    continue;
                }
                if crate::propagate::untouched(doc, *c, *s_old, &none) {
                    deletes.push(*c);
                } else {
                    for k in crate::build::subtree_nodes(doc, &[*c]) {
                        if doc
                            .get(k)
                            .and_then(|n| n.link)
                            .is_some_and(|l| old_sources.contains(&l))
                        {
                            cuts.push(k);
                        }
                    }
                }
            }
        }
    }
    let deleted: FxHashSet<NodeId> = crate::build::subtree_nodes(doc, &deletes)
        .into_iter()
        .collect();

    // The new main's layers with no match, copied in under their parent's
    // counterpart — the topmost of each unmatched run, whole.
    let new_to_c: FxHashMap<NodeId, NodeId> = matched
        .iter()
        .filter_map(|(o, n)| cp.get(o).map(|c| (*n, *c)))
        .collect();
    let mut inserts: Vec<(NodeId, NodeId, Vec<Node>)> = Vec::new(); // (parent, source, nodes)
    for (s_new, _) in &paths_new {
        if matched_new.contains(s_new) {
            continue;
        }
        let Some(parent) = doc.get(*s_new).and_then(|n| n.parent) else {
            continue;
        };
        if parent != to && !matched_new.contains(&parent) {
            continue; // under an unmatched parent: copied with it
        }
        let Some(&host) = new_to_c.get(&parent) else {
            continue; // the instance removed the parent's counterpart
        };
        let template = doc.capture_subtree(*s_new)?;
        let (mut copy, _) = crate::document::remap_subtree(&template, ids)?;
        for (k, t) in copy.iter_mut().zip(&template) {
            k.make_copy_of(t.id);
        }
        inserts.push((host, *s_new, copy));
    }

    // Each parent's final order: the survivors, the new main's order where the
    // instance still had the old main's, and the inserts at their anchors.
    let mut ops: Vec<Operation> = Vec::new();
    ops.extend(deletes.iter().map(|id| Operation::DeleteNode { id: *id }));
    ops.extend(cuts.iter().map(|id| Operation::SetLink {
        id: *id,
        link: None,
    }));
    ops.extend(fields);
    let mut hosts: Vec<(NodeId, NodeId)> = matched
        .iter()
        .filter_map(|(o, n)| cp.get(o).map(|c| (*c, *n)))
        .filter(|(c, _)| !deleted.contains(c))
        .collect();
    hosts.sort();
    for (c, s_new) in hosts {
        let Some(cn) = doc.get(c) else { continue };
        let mut kids: Vec<NodeId> = cn
            .children
            .iter()
            .copied()
            .filter(|k| !deleted.contains(k))
            .collect();
        // The root's old source is `from`, whatever its link says (a swap).
        let s_old = if c == root {
            from
        } else {
            doc.get(c).and_then(|n| n.link).unwrap_or(from)
        };
        let s_old_kids = doc
            .get(s_old)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        let s_new_kids = doc
            .get(s_new)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        let src_of = |k: NodeId| doc.get(k).and_then(|n| n.link);
        // Reorder the matched ones into the new main's order, in their slots, if
        // the instance still had the old main's.
        let linked: Vec<NodeId> = kids
            .iter()
            .filter_map(|k| src_of(*k))
            .filter(|s| s_old_kids.contains(s) && matched.contains_key(s))
            .collect();
        let old_order: Vec<NodeId> = s_old_kids
            .iter()
            .copied()
            .filter(|s| linked.contains(s))
            .collect();
        if linked == old_order {
            let new_order: Vec<NodeId> = s_new_kids
                .iter()
                .filter_map(|n| linked.iter().find(|o| matched.get(o) == Some(n)).copied())
                .collect();
            let slots: Vec<usize> = (0..kids.len())
                .filter(|&i| src_of(kids[i]).is_some_and(|s| linked.contains(&s)))
                .collect();
            let cur = kids.clone();
            for (slot, s) in slots.into_iter().zip(&new_order) {
                if let Some(k) = cur.iter().find(|k| src_of(**k) == Some(*s)) {
                    kids[slot] = *k;
                }
            }
        }
        // The inserts under this host, at their anchors among the new main's
        // siblings, through each kid's new source.
        let new_src = |k: NodeId| src_of(k).and_then(|s| matched.get(&s).copied());
        let mut placed: Vec<(NodeId, NodeId)> = kids
            .iter()
            .map(|k| (*k, new_src(*k).unwrap_or(*k)))
            .collect();
        // Appended, then put in place by the reorders below: the deletes have
        // already shortened the list, so its length is the only safe index.
        for (host, s_new_child, copy) in inserts.iter().filter(|(h, _, _)| *h == c) {
            let ids_now: Vec<NodeId> = placed.iter().map(|(k, _)| *k).collect();
            let at = crate::propagate::anchor(&s_new_kids, *s_new_child, &ids_now, |s| {
                placed.iter().find(|(_, src)| *src == s).map(|(k, _)| *k)
            });
            ops.push(Operation::InsertSubtree {
                nodes: copy.clone(),
                parent: *host,
                index: placed.len(),
            });
            placed.insert(at.min(placed.len()), (copy[0].id, *s_new_child));
        }
        let target: Vec<NodeId> = placed.iter().map(|(k, _)| *k).collect();
        let before: Vec<NodeId> = cn
            .children
            .iter()
            .copied()
            .filter(|k| !deleted.contains(k))
            .collect();
        if target != before {
            ops.extend(
                target
                    .iter()
                    .enumerate()
                    .map(|(i, id)| Operation::Reorder { id: *id, index: i }),
            );
        }
    }
    Some(ops)
}

/// Each layer below `main` with its **name path** — the names from just below
/// `main` down to it, each with its index among same-named siblings.
fn paths(doc: &Document, main: NodeId) -> Vec<(NodeId, Vec<(String, usize)>)> {
    let mut out = Vec::new();
    let mut stack: Vec<(NodeId, Vec<(String, usize)>)> = vec![(main, Vec::new())];
    while let Some((id, path)) = stack.pop() {
        let Some(n) = doc.get(id) else { continue };
        let mut seen: FxHashMap<&str, usize> = FxHashMap::default();
        for c in &n.children {
            let Some(cn) = doc.get(*c) else { continue };
            let k = seen.entry(cn.name.as_str()).or_default();
            let mut p = path.clone();
            p.push((cn.name.clone(), *k));
            *k += 1;
            out.push((*c, p.clone()));
            stack.push((*c, p));
        }
    }
    out
}

/// What [`carry`] leaves alone on the node it rewrites.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Keep {
    Nothing,
    /// A switched root's placement, its own as an instance root's always is.
    Placement,
    /// A swapped root's slot fields (`swap::is_slot_field`, §15 D983 (3)).
    Slot,
}

/// The field edits a switch makes to the counterpart `c`: every field of `s_new`
/// where `c` still held `s_old`'s value — `propagate::follow`, the propagation
/// pass's rule, applied as though `s_old` had become `s_new` — but what `keep`
/// keeps. `s_old` is the node an override is measured against, which for a swap
/// is the counterpart in the main the slot shows rather than the slot's own
/// (`rewrite`'s `base_of`, §15 D991).
fn carry(doc: &Document, c: NodeId, s_old: NodeId, s_new: NodeId, keep: Keep) -> Vec<Operation> {
    use crate::reset::{field_key, state_ops};
    let (Some(cn), Some(on), Some(nn)) = (doc.get(c), doc.get(s_old), doc.get(s_new)) else {
        return Vec::new();
    };
    let olds: FxHashMap<_, Operation> = state_ops(on)
        .into_iter()
        .filter_map(|op| Some((field_key(&op)?, op)))
        .collect();
    let curs: FxHashMap<_, Operation> = state_ops(cn)
        .into_iter()
        .filter_map(|op| Some((field_key(&op)?, op)))
        .collect();
    let mut out = Vec::new();
    for new in state_ops(nn) {
        let kept = match keep {
            Keep::Nothing => false,
            Keep::Placement => crate::propagate::is_placement(&new),
            Keep::Slot => crate::swap::is_slot_field(&new),
        };
        if kept {
            continue;
        }
        let Some(key) = field_key(&new) else { continue };
        let (Some(old), Some(cur)) = (olds.get(&key), curs.get(&key)) else {
            continue;
        };
        let (old, cur) = rekey_lists(&new, old, cur);
        let Some(new) = new.retargeted(c) else {
            continue;
        };
        if let Some(op) = crate::propagate::follow(&new, &old, &cur, c) {
            out.push(op);
        }
    }
    out
}

/// For a keyed list, `old`'s and `cur`'s items renamed to `new`'s ids **by
/// position** where an old id has no match in `new` — two variants made apart
/// share no item ids, and matching their fills by position is what a switch
/// means. Anything else passes through.
fn rekey_lists(new: &Operation, old: &Operation, cur: &Operation) -> (Operation, Operation) {
    use Operation as O;
    fn map_ids<T: Clone>(new: &[Keyed<T>], old: &[Keyed<T>]) -> FxHashMap<ItemId, ItemId> {
        let has = |list: &[Keyed<T>], id: ItemId| list.iter().any(|k| k.id == id);
        old.iter()
            .enumerate()
            .filter(|(i, o)| !has(new, o.id) && new.get(*i).is_some_and(|n| !has(old, n.id)))
            .map(|(i, o)| (o.id, new[i].id))
            .collect()
    }
    fn apply<T: Clone>(list: &[Keyed<T>], map: &FxHashMap<ItemId, ItemId>) -> Vec<Keyed<T>> {
        list.iter()
            .map(|k| Keyed::new(map.get(&k.id).copied().unwrap_or(k.id), k.value.clone()))
            .collect()
    }
    macro_rules! lists {
        ($($v:ident { $f:ident }),*) => {
            match (new, old, cur) {
                $((O::$v { $f: n, .. }, O::$v { id: oi, $f: o }, O::$v { id: ci, $f: c }) => {
                    let m = map_ids(n, o);
                    (O::$v { id: *oi, $f: apply(o, &m) }, O::$v { id: *ci, $f: apply(c, &m) })
                })*
                _ => (old.clone(), cur.clone()),
            }
        };
    }
    lists!(
        SetFills { fills },
        SetStrokes { strokes },
        SetEffects { effects },
        SetExports { exports },
        SetLayoutGrids { grids }
    )
}

// ── Component properties on an instance ──────────────────────────────────────

/// A component property's value on one instance.
#[derive(Clone, Debug, PartialEq)]
pub enum PropValue {
    Boolean(bool),
    Text(String),
    /// The main a swap property's copy shows (§15 D983).
    Swap(NodeId),
}

/// The properties an instance rooted at `root` offers — its main's owner's (the
/// set's, for a variant) — with their item ids.
pub fn instance_properties(doc: &Document, root: NodeId) -> Vec<Keyed<Property>> {
    let Some(main) = crate::component::main_of(doc, root) else {
        return Vec::new();
    };
    owner_of_main(doc, main)
        .and_then(|o| doc.get(o))
        .map(|o| o.props.clone())
        .unwrap_or_default()
}

/// The layers of the instance rooted at `root` that property `p` drives: each
/// node below `root` whose chain of links reaches a layer `p` is bound to, the
/// bound layers outside this instance's own variant contributing nothing.
pub fn counterparts(doc: &Document, root: NodeId, p: &Property) -> Vec<NodeId> {
    let bound: FxHashSet<NodeId> = p.bound.iter().copied().collect();
    let mut out = Vec::new();
    for id in crate::build::subtree_nodes(doc, &[root]) {
        if id == root {
            continue;
        }
        let mut at = doc.get(id).and_then(|n| n.link);
        for _ in 0..64 {
            match at {
                Some(l) if bound.contains(&l) => {
                    out.push(id);
                    break;
                }
                Some(l) => at = doc.get(l).and_then(|n| n.link),
                None => break,
            }
        }
    }
    out
}

/// A layer's value of the field a property of `kind` drives.
pub fn field_value(doc: &Document, id: NodeId, kind: PropKind) -> Option<PropValue> {
    let n = doc.get(id)?;
    match kind {
        PropKind::Boolean => Some(PropValue::Boolean(n.visible)),
        PropKind::Text => match &n.kind {
            NodeKind::Text { content, .. } => Some(PropValue::Text(content.clone())),
            _ => None,
        },
        PropKind::Swap => crate::component::main_of(doc, id).map(PropValue::Swap),
        // A showing drives no field.
        PropKind::Nested => None,
    }
}

/// Property `p`'s value on the instance rooted at `root` — its first
/// counterpart's — and whether it is **overridden**: any counterpart's field
/// differing from its source's, one link up, which is what a reset would undo.
pub fn property_state(doc: &Document, root: NodeId, p: &Property) -> Option<(PropValue, bool)> {
    let cps = counterparts(doc, root, p);
    let value = field_value(doc, *cps.first()?, p.kind)?;
    let overridden = cps
        .iter()
        .any(|c| !property_reset_ops(doc, *c, p.kind).is_empty());
    Some((value, overridden))
}

/// The ops that set property `p` to `value` on each instance in `roots`. A text
/// set to its source's own content takes the source's spans too, so it reads as
/// no override; any other text drops its spans, which index into what it replaced.
pub fn set_property(
    doc: &Document,
    roots: &[NodeId],
    p: &Property,
    value: &PropValue,
) -> Vec<Operation> {
    let mut ops = Vec::new();
    for r in roots {
        for c in counterparts(doc, *r, p) {
            match value {
                // The swap verb's own rule: the slot's main clears the swap. A
                // shown copy carries its variant values by name (4Q).
                PropValue::Swap(main) => {
                    if let Some(tx) = swap_carrying(doc, c, *main) {
                        ops.extend(tx.0);
                    }
                }
                PropValue::Boolean(v) => {
                    if doc.get(c).is_some_and(|n| n.visible != *v) {
                        ops.push(Operation::SetVisible { id: c, visible: *v });
                    }
                }
                PropValue::Text(t) => {
                    let Some(NodeKind::Text { content, .. }) = doc.get(c).map(|n| &n.kind) else {
                        continue;
                    };
                    if content == t {
                        continue;
                    }
                    let src = crate::reset::source_of(doc, c).and_then(|s| doc.get(s));
                    let op = match src.map(|s| &s.kind) {
                        Some(NodeKind::Text {
                            content: sc,
                            spans,
                            para_spans,
                            ..
                        }) if sc == t => Operation::SetText {
                            id: c,
                            content: t.clone(),
                            spans: spans.clone(),
                            para_spans: para_spans.clone(),
                        },
                        _ => Operation::SetText {
                            id: c,
                            content: t.clone(),
                            spans: Default::default(),
                            para_spans: Default::default(),
                        },
                    };
                    ops.push(op);
                }
            }
        }
    }
    ops
}

/// The ops that put a counterpart's property-driven field back to its source's.
fn property_reset_ops(doc: &Document, c: NodeId, kind: PropKind) -> Vec<Operation> {
    crate::reset::overrides(doc, c)
        .into_iter()
        .map(|o| o.reset)
        .filter(|op| match kind {
            PropKind::Boolean => matches!(op, Operation::SetVisible { .. }),
            PropKind::Text => matches!(op, Operation::SetText { .. }),
            PropKind::Swap => matches!(op, Operation::SetSwap { .. }),
            PropKind::Nested => false,
        })
        .collect()
}

/// The ops that reset property `p` on each instance in `roots`.
pub fn reset_property(doc: &Document, roots: &[NodeId], p: &Property) -> Vec<Operation> {
    roots
        .iter()
        .flat_map(|r| counterparts(doc, *r, p))
        .flat_map(|c| property_reset_ops(doc, c, p.kind))
        .collect()
}

/// Every (layer, field) a property drives in the instance rooted at `root` — the
/// fields the card counts as **properties** rather than overrides — and, for
/// each nested instance the card shows ([`shown_nested`]), its swap and every
/// field its own properties drive: *"a shown row counts as a property, never an
/// override"* (§15 D988).
pub fn property_fields(doc: &Document, root: NodeId) -> FxHashSet<(NodeId, PropKind)> {
    let mut out = own_property_fields(doc, root);
    for s in shown_nested(doc, root) {
        out.insert((s.copy, PropKind::Swap));
        out.extend(own_property_fields(doc, s.copy));
    }
    out
}

/// [`property_fields`] for `root`'s own properties alone.
fn own_property_fields(doc: &Document, root: NodeId) -> FxHashSet<(NodeId, PropKind)> {
    instance_properties(doc, root)
        .iter()
        .filter(|p| p.kind != PropKind::Nested)
        .flat_map(|p| {
            counterparts(doc, root, p)
                .into_iter()
                .map(move |c| (c, p.kind))
        })
        .collect()
}

// ── Nested properties (§15 D988, `design/Variants.dc.html` 4K–4S) ────────────

/// A nested instance whose own properties an instance's card shows, under a
/// sub-heading of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShownNested {
    /// The copy inside the instance.
    pub copy: NodeId,
    /// The shown copies from the outermost down to `copy`: one where the
    /// instance's own main shows the slot, more where a shown copy's main shows
    /// one in turn — 4P's *"opt-in carries through each level"*, flattened.
    pub path: Vec<NodeId>,
    /// What matches this group across several instances of one owner (4S): the
    /// layers from below the instance root down to `copy`, each as its name and
    /// which sibling of that name it is. A name path, because the slots of a
    /// set's variants are different nodes — and the k-th sibling of a name, the
    /// variant switch's own matching, because an instance takes its main's name
    /// verbatim (§15 D982) and two instances of one main in a main share one
    /// until renamed.
    pub key: Vec<(String, usize)>,
}

/// The nested instances whose properties the card of the instance rooted at
/// `root` shows (§15 D988): the copies of every slot its main's owner shows, in
/// layers order, each followed by the ones its own main shows, and so on down.
/// Empty for a node that is not an instance or shows nothing.
pub fn shown_nested(doc: &Document, root: NodeId) -> Vec<ShownNested> {
    let mut out = Vec::new();
    shown_below(doc, root, root, &[], &mut out);
    out
}

fn shown_below(
    doc: &Document,
    top: NodeId,
    root: NodeId,
    path: &[NodeId],
    out: &mut Vec<ShownNested>,
) {
    // A showing that reached itself would recurse for ever; the depth of the
    // tree bounds an honest one.
    if path.len() > 32 {
        return;
    }
    let Some(showing) = instance_properties(doc, root)
        .into_iter()
        .find(|p| p.kind == PropKind::Nested)
    else {
        return;
    };
    for copy in counterparts(doc, root, &showing) {
        let mut at = path.to_vec();
        at.push(copy);
        out.push(ShownNested {
            copy,
            path: at.clone(),
            key: name_path(doc, top, copy),
        });
        shown_below(doc, top, copy, &at, out);
    }
}

/// The layers from below `top` down to `id`, `id`'s included, each as its name
/// and which sibling of that name it is. Empty where `id` is not below `top`.
fn name_path(doc: &Document, top: NodeId, id: NodeId) -> Vec<(String, usize)> {
    let mut steps = Vec::new();
    let mut at = Some(id);
    while let Some(n) = at.and_then(|a| doc.get(a)) {
        if n.id == top {
            steps.reverse();
            return steps;
        }
        let k = n
            .parent
            .and_then(|p| doc.get(p))
            .map(|p| {
                p.children
                    .iter()
                    .take_while(|c| **c != n.id)
                    .filter(|c| doc.get(**c).is_some_and(|c| c.name == n.name))
                    .count()
            })
            .unwrap_or(0);
        steps.push((n.name.clone(), k));
        at = n.parent;
    }
    Vec::new()
}

/// The layer below `top` at `path` — [`name_path`]'s inverse, the k-th sibling
/// of each name.
fn at_name_path(doc: &Document, top: NodeId, path: &[(String, usize)]) -> Option<NodeId> {
    let mut at = top;
    for (name, k) in path {
        at = doc
            .get(at)?
            .children
            .iter()
            .copied()
            .filter(|c| doc.get(*c).is_some_and(|c| c.name == *name))
            .nth(*k)?;
    }
    Some(at)
}

/// Whether `copy` is a **shown** nested copy: some node on its chain of links is
/// a slot its owner shows. What gates the carry by property name (4Q's *"limited
/// to shown rows"*).
pub fn is_shown(doc: &Document, copy: NodeId) -> bool {
    let mut at = doc.get(copy).and_then(|n| n.link);
    for _ in 0..64 {
        let Some(s) = at else { return false };
        if slot_is_shown(doc, s) {
            return true;
        }
        at = doc.get(s).and_then(|n| n.link);
    }
    false
}

/// Whether `slot` — a nested instance inside a main — is one its owner shows.
pub fn slot_is_shown(doc: &Document, slot: NodeId) -> bool {
    owner_above(doc, slot)
        .and_then(|o| doc.get(o))
        .is_some_and(|o| {
            o.props
                .iter()
                .any(|p| p.kind == PropKind::Nested && p.bound.contains(&slot))
        })
}

/// The nested instances inside `owner`'s main — its first variant's, for a set —
/// that a showing could take (4L): every layer linked straight to a main and not
/// itself inside such a layer, in layers order, each with whether it is shown.
pub fn nested_slots(doc: &Document, owner: NodeId) -> Vec<(NodeId, bool)> {
    let main = match doc.get(owner) {
        Some(o) if o.set.is_some() => match variants(doc, owner).first() {
            Some(v) => *v,
            None => return Vec::new(),
        },
        Some(_) => owner,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = doc
        .get(main)
        .map(|m| m.children.iter().rev().copied().collect())
        .unwrap_or_default();
    while let Some(id) = stack.pop() {
        let Some(n) = doc.get(id) else { continue };
        if n.link.and_then(|l| doc.get(l)).is_some_and(|l| l.component) {
            out.push((id, slot_is_shown(doc, id)));
            continue;
        }
        stack.extend(n.children.iter().rev().copied());
    }
    out
}

/// Show or stop showing the slot `slot` on its owner's instances (4K) — and, in
/// a set, the slot at the same name path in **every** variant, so the switch
/// means what 4K's caption says, *"appears on every Button"* (the session's: a
/// showing per variant would make a set's instances disagree about a slot they
/// all hold). The owner's one `PropKind::Nested` is made on the first showing
/// and goes with the last. `None` for a slot outside a main or a change that
/// changes nothing.
pub fn set_shown(
    doc: &Document,
    ids: &mut IdSource,
    slot: NodeId,
    on: bool,
) -> Option<Transaction> {
    let owner = owner_above(doc, slot)?;
    let o = doc.get(owner)?;
    let slots: Vec<NodeId> = match o.set {
        Some(_) => {
            let variant_of = |id: NodeId| {
                let mut at = Some(id);
                while let Some(n) = at.and_then(|a| doc.get(a)) {
                    if n.component {
                        return Some(n.id);
                    }
                    at = n.parent;
                }
                None
            };
            let path = name_path(doc, variant_of(slot)?, slot);
            variants(doc, owner)
                .into_iter()
                .filter_map(|v| at_name_path(doc, v, &path))
                .filter(|s| {
                    doc.get(*s)
                        .and_then(|n| n.link)
                        .and_then(|l| doc.get(l))
                        .is_some_and(|l| l.component)
                })
                .collect()
        }
        None => vec![slot],
    };
    let mut props = o.props.clone();
    let at = props.iter().position(|p| p.kind == PropKind::Nested);
    match (at, on) {
        (Some(i), true) => {
            let bound = &mut props[i].value.bound;
            let before = bound.len();
            for s in slots {
                if !bound.contains(&s) {
                    bound.push(s);
                }
            }
            if bound.len() == before {
                return None;
            }
        }
        (Some(i), false) => {
            let bound = &mut props[i].value.bound;
            let before = bound.len();
            bound.retain(|b| !slots.contains(b));
            if bound.len() == before {
                return None;
            }
            if bound.is_empty() {
                props.remove(i);
            }
        }
        (None, true) => props.push(Keyed::new(
            ids.mint_item(),
            Property {
                name: String::new(),
                kind: PropKind::Nested,
                bound: slots,
                filter: String::new(),
            },
        )),
        (None, false) => return None,
    }
    Some(Transaction(vec![Operation::SetProperties {
        id: owner,
        props,
    }]))
}

/// The main a **shown** copy should swap to when `to` is picked for it (4Q): the
/// variant of `to`'s set holding, for each variant property it shares **by
/// name** with the set the copy shows now, the value the copy shows now — so
/// *Icon → Check* keeps *Weight: Bold* — with `to`'s own value wherever a name
/// is not shared, or the value is not one `to`'s set offers. `to` itself for a
/// copy that is not shown, or a main in no set, or a combination the set lacks.
pub fn carried_target(doc: &Document, copy: NodeId, to: NodeId) -> NodeId {
    if !is_shown(doc, copy) {
        return to;
    }
    let (Some(from), Some(to_set)) = (crate::component::main_of(doc, copy), set_of(doc, to)) else {
        return to;
    };
    let Some(from_set) = set_of(doc, from) else {
        return to;
    };
    if from_set == to_set {
        return to;
    }
    let (Some(fs), Some(ts)) = (
        doc.get(from_set).and_then(|s| s.set.clone()),
        doc.get(to_set).and_then(|s| s.set.clone()),
    ) else {
        return to;
    };
    let from_values = doc.get(from).map(|n| n.variant.clone()).unwrap_or_default();
    let Some(mut want) = doc.get(to).map(|n| n.variant.clone()) else {
        return to;
    };
    for (ti, tp) in ts.props.iter().enumerate() {
        let carried = fs
            .props
            .iter()
            .position(|fp| fp.name == tp.name)
            .and_then(|fi| from_values.get(fi))
            .filter(|v| tp.values.contains(v));
        if let (Some(v), Some(slot)) = (carried, want.get_mut(ti)) {
            *slot = v.clone();
        }
    }
    find_variant(doc, to_set, &want).unwrap_or(to)
}

/// [`crate::swap::swap`] with the carry by property name a shown copy takes
/// ([`carried_target`]).
pub fn swap_carrying(doc: &Document, copy: NodeId, to: NodeId) -> Option<Transaction> {
    crate::swap::swap(doc, copy, carried_target(doc, copy, to))
}

/// A nested copy's variant property `prop` against its **slot's**: the value the
/// slot's own main holds for the property of that name, and whether the copy
/// shows another (§15 D988's departure, 4N: a nested copy's variant switch is a
/// swap, so an override, and its dropdown draws the dot). `None` for a root that
/// cannot swap — a root linked straight to a main switches by relinking, and
/// *"a variant choice is never an override"* there (§15 D982) — or a slot whose
/// main has no property of that name.
pub fn nested_variant_state(doc: &Document, copy: NodeId, prop: usize) -> Option<(String, bool)> {
    if !crate::swap::can_swap(doc, copy) {
        return None;
    }
    let shown = crate::component::main_of(doc, copy)?;
    let name = doc
        .get(set_of(doc, shown)?)?
        .set
        .as_ref()?
        .props
        .get(prop)?
        .name
        .clone();
    let current = doc.get(shown)?.variant.get(prop)?.clone();
    let slot = crate::swap::slot_main(doc, copy)?;
    let slot_set = doc.get(set_of(doc, slot)?)?.set.clone()?;
    let at = slot_set.props.iter().position(|p| p.name == name)?;
    let source = doc.get(slot)?.variant.get(at)?.clone();
    let differs = current != source;
    Some((source, differs))
}

/// The reset of [`nested_variant_state`]'s mark: the copy switched, **within the
/// set it shows now**, to the slot's value for that property — the other values,
/// and an outer swap to another set, kept. A swap back to the slot's own main
/// clears the swap (`swap::swap`'s rule). `None` where there is nothing to reset
/// or no such variant.
pub fn nested_variant_reset(doc: &Document, copy: NodeId, prop: usize) -> Option<Transaction> {
    let (source, differs) = nested_variant_state(doc, copy, prop)?;
    if !differs {
        return None;
    }
    let shown = crate::component::main_of(doc, copy)?;
    let to = switch_target(doc, shown, prop, &source)?;
    crate::swap::swap(doc, copy, to)
}

/// Whether the shown copy `s` is hidden in the instance rooted at `root`, and by
/// what (4R): `None` while it and every layer above it, up to `root`, are
/// visible; else the name of the first **Boolean** property — `root`'s, or a
/// shown copy's on the way down — that drives the visibility of the layer that
/// hides it, or an empty name where none does.
pub fn hidden_by(doc: &Document, root: NodeId, s: &ShownNested) -> Option<String> {
    let mut hider = None;
    let mut at = Some(s.copy);
    while let Some(n) = at.and_then(|a| doc.get(a)) {
        if n.id == root {
            break;
        }
        if !n.visible {
            hider = Some(n.id);
        }
        at = n.parent;
    }
    let hider = hider?;
    let roots = std::iter::once(root).chain(s.path.iter().copied());
    for r in roots {
        for p in instance_properties(doc, r) {
            if p.kind == PropKind::Boolean && counterparts(doc, r, &p).contains(&hider) {
                return Some(p.name.clone());
            }
        }
    }
    Some(String::new())
}

/// Every property reset of the instance rooted at `root` — its own properties'
/// and each shown copy's, with each shown copy's swap (which carries its variant
/// rows) — what *Reset properties* writes (4N: *"Reset properties includes
/// it"*). Each op once.
pub fn reset_all_properties(doc: &Document, root: NodeId) -> Vec<Operation> {
    let mut ops: Vec<Operation> = Vec::new();
    let push = |op: Operation, ops: &mut Vec<Operation>| {
        if !ops.contains(&op) {
            ops.push(op);
        }
    };
    for p in instance_properties(doc, root) {
        for op in reset_property(doc, &[root], &p) {
            push(op, &mut ops);
        }
    }
    for s in shown_nested(doc, root) {
        if doc.get(s.copy).is_some_and(|n| n.swap.is_some()) {
            push(
                Operation::SetSwap {
                    id: s.copy,
                    swap: None,
                },
                &mut ops,
            );
        }
        for p in instance_properties(doc, s.copy) {
            for op in reset_property(doc, &[s.copy], &p) {
                push(op, &mut ops);
            }
        }
    }
    ops
}

/// How many of the instance rooted at `root`'s **shown rows** differ from the
/// main (4N's count): each shown copy's swap once — its swap row and its
/// variant rows are one field — unless an outer swap property already counts
/// it, and each of its own properties that differs.
pub fn shown_rows_overridden(doc: &Document, root: NodeId) -> usize {
    let shown = shown_nested(doc, root);
    let mut bound_swaps: FxHashSet<NodeId> = FxHashSet::default();
    for r in std::iter::once(root).chain(shown.iter().map(|s| s.copy)) {
        for p in instance_properties(doc, r) {
            if p.kind == PropKind::Swap {
                bound_swaps.extend(counterparts(doc, r, &p));
            }
        }
    }
    let mut n = 0;
    for s in &shown {
        if !bound_swaps.contains(&s.copy) && doc.get(s.copy).is_some_and(|c| c.swap.is_some()) {
            n += 1;
        }
        n += instance_properties(doc, s.copy)
            .iter()
            .filter(|p| property_state(doc, s.copy, p).is_some_and(|(_, o)| o))
            .count();
    }
    n
}

// ── Defining properties on a main ────────────────────────────────────────────

/// Add a property to `owner` — a main or a set — bound to `bound`. `None` for a
/// name that is empty or taken.
///
/// A **swap** property's filter starts as the suggestion §15 D983 (5) makes —
/// the name of the main its first bound layer shows, less its last segment
/// (`swap::suggested_filter`) — for the user to edit.
pub fn define(
    doc: &Document,
    ids: &mut IdSource,
    owner: NodeId,
    name: &str,
    kind: PropKind,
    bound: Vec<NodeId>,
) -> Option<(Transaction, ItemId)> {
    let o = doc.get(owner)?;
    let name = fresh(name)?;
    let reserved = o.set.iter().flat_map(|s| s.props.iter().map(|p| &p.name));
    if o.props.iter().any(|p| p.name == name) || reserved.into_iter().any(|r| *r == name) {
        return None;
    }
    let filter = match kind {
        PropKind::Swap => bound
            .first()
            .and_then(|b| crate::component::main_of(doc, *b))
            .and_then(|m| doc.get(m))
            .map(|m| crate::swap::suggested_filter(&m.name))
            .unwrap_or_default(),
        PropKind::Boolean | PropKind::Text | PropKind::Nested => String::new(),
    };
    let item = ids.mint_item();
    let mut props = o.props.clone();
    props.push(Keyed::new(
        item,
        Property {
            name,
            kind,
            bound,
            filter,
        },
    ));
    Some((
        Transaction(vec![Operation::SetProperties { id: owner, props }]),
        item,
    ))
}

/// `owner`'s properties with the one item `item` rewritten by `f`, or removed
/// when `f` answers `None`.
pub fn edit_property(
    doc: &Document,
    owner: NodeId,
    item: ItemId,
    f: impl FnOnce(&Property) -> Option<Property>,
) -> Option<Transaction> {
    let o = doc.get(owner)?;
    let at = o.props.iter().position(|p| p.id == item)?;
    let mut props = o.props.clone();
    match f(&props[at]) {
        Some(p) => props[at] = Keyed::new(item, p),
        None => {
            props.remove(at);
        }
    }
    Some(Transaction(vec![Operation::SetProperties {
        id: owner,
        props,
    }]))
}

/// The property of `owner` that drives `node`'s field of `kind`, if one does.
pub fn bound_to(
    doc: &Document,
    owner: NodeId,
    node: NodeId,
    kind: PropKind,
) -> Option<Keyed<Property>> {
    doc.get(owner)?
        .props
        .iter()
        .find(|p| p.kind == kind && p.bound.contains(&node))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(props: &[(&str, &[&str])]) -> VariantSet {
        VariantSet {
            props: props
                .iter()
                .map(|(n, vs)| VariantProp {
                    name: n.to_string(),
                    values: vs.iter().map(|v| v.to_string()).collect(),
                })
                .collect(),
        }
    }

    fn combo(vs: &[&str]) -> Vec<String> {
        vs.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn the_first_free_combination_turns_the_last_property_fastest() {
        let s = set(&[("Size", &["S", "L"]), ("State", &["Default", "Hover"])]);
        let mut taken = FxHashSet::default();
        assert_eq!(first_free(&s, &taken), combo(&["S", "Default"]));
        taken.insert(combo(&["S", "Default"]));
        assert_eq!(first_free(&s, &taken), combo(&["S", "Hover"]));
        taken.insert(combo(&["S", "Hover"]));
        assert_eq!(first_free(&s, &taken), combo(&["L", "Default"]));
        taken.insert(combo(&["L", "Default"]));
        taken.insert(combo(&["L", "Hover"]));
        // Every one taken: the first, which is a clash the card then shows.
        assert_eq!(first_free(&s, &taken), combo(&["S", "Default"]));
    }

    #[test]
    fn a_property_round_trips_its_bound_ids_as_wire_strings() {
        let p = Property {
            name: "Show icon".into(),
            kind: PropKind::Boolean,
            bound: vec![NodeId {
                actor: 0xab,
                seq: 7,
            }],
            filter: String::new(),
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"ab:7\""), "{json}");
        assert!(
            !json.contains("filter"),
            "an empty filter is not written: {json}"
        );
        assert_eq!(serde_json::from_str::<Property>(&json).unwrap(), p);
    }
}
