//! **Push to main** (§5.3d): an instance's differences written into the main it is
//! compared with, so every instance still following the main takes them. Reset's
//! opposite, one link up: a copy pushes into its [`reset::source_of`], which is
//! the node *Reset* compares it against, and nothing past the main that holds that
//! node (its **home**). The commit's own passes carry the push on: an instance
//! still equal to the main's old value takes the new one, one with its own keeps
//! it.
//!
//! Five families, each its own menu row: property values, fields, local layers,
//! order and removals. *Push all* is the first four in one transaction; a removal
//! deletes a layer from the main and is only ever pushed on its own.

use crate::component;
use crate::document::Document;
use crate::id::{IdSource, NodeId};
use crate::item::ItemId;
use crate::op::{Operation, Transaction};
use crate::reset::{self, source_of};
use crate::variant::PropKind;
use rustc_hash::{FxHashMap, FxHashSet};

/// Which push — the card's rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// *Push all to main*: properties, fields, local layers and order.
    All,
    /// *Push properties to main*: the boolean, text and swap properties' fields.
    Properties,
    /// *Push fields to main*: every other overridden field.
    Fields,
    /// *Push local layers to main*.
    Layers,
    /// *Push order to main*.
    Order,
    /// *Push removals to main*: the main's layers this instance removed, deleted.
    Removals,
}

/// Why an instance cannot push at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    /// Its main lives in a kit's vault, which only the kit changes.
    Kit,
    /// Its main is locked.
    Locked,
}

/// Why one local layer cannot be pushed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerBlocked {
    /// A local group wrapping layers linked to the main: pushing it would move
    /// the main's own children.
    WrapsLinked,
    /// It holds an instance of the main (or of one that reaches it): a component
    /// cannot contain itself.
    HoldsMain,
}

/// What each push would write for one instance — the rows' counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pushable {
    /// Property fields (a bound visibility, text or swap) that differ.
    pub properties: usize,
    /// Field units ([`reset::Override::units`]) that differ, properties apart.
    pub fields: usize,
    /// Outermost local layers that can be pushed.
    pub layers: usize,
    /// Outermost local layers [`layer_blocked`] refuses.
    pub layers_blocked: usize,
    /// Parents whose linked children are out of the main's order.
    pub order: usize,
    /// The main's layers this instance removed.
    pub removals: usize,
}

impl Pushable {
    /// Whether *Push all* would write anything.
    pub fn any(&self) -> bool {
        self.properties + self.fields + self.layers + self.order > 0
    }
}

/// The main a push from the instance at `root` writes into: the main holding the
/// node `root` is compared with. `None` outside an instance.
pub fn home(doc: &Document, root: NodeId) -> Option<NodeId> {
    main_holding(doc, source_of(doc, root))
}

/// The main holding a swapped `root`'s slot — its link, which its swap and
/// slot fields are compared with and pushed into — `None` for a root that is
/// not swapped.
fn slot_home(doc: &Document, root: NodeId) -> Option<NodeId> {
    doc.get(root)?.swap?;
    main_holding(doc, reset::slot_source_of(doc, root))
}

/// The component at or above `at`.
fn main_holding(doc: &Document, mut at: Option<NodeId>) -> Option<NodeId> {
    while let Some(n) = at.and_then(|a| doc.get(a)) {
        if n.component {
            return Some(n.id);
        }
        at = n.parent;
    }
    None
}

/// Why nothing can be pushed into `main`.
fn main_blocked(doc: &Document, main: NodeId) -> Option<Blocked> {
    if doc.in_vault(main) {
        Some(Blocked::Kit)
    } else if crate::query::is_effectively_locked(doc, main) {
        Some(Blocked::Locked)
    } else {
        None
    }
}

/// Why the instance at `root` cannot push, or `None` when it can: a swapped
/// root can while either its home or its slot's main takes a push.
pub fn blocked(doc: &Document, root: NodeId) -> Option<Blocked> {
    let main = home(doc, root)?;
    let why = main_blocked(doc, main)?;
    match slot_home(doc, root) {
        Some(slot) if main_blocked(doc, slot).is_none() => None,
        _ => Some(why),
    }
}

/// The nodes of `root`'s home main, and of its slot's main for a swapped root,
/// each that takes a push — where every push target must lie.
fn inside(doc: &Document, root: NodeId) -> FxHashSet<NodeId> {
    let mains: Vec<NodeId> = [home(doc, root), slot_home(doc, root)]
        .into_iter()
        .flatten()
        .filter(|m| main_blocked(doc, *m).is_none())
        .collect();
    crate::build::subtree_nodes(doc, &mains)
        .into_iter()
        .collect()
}

/// The mains the push `pushed` from the instance at `root` writes into: its home,
/// then its slot's main for a swapped root, each only when an operation lands in
/// it. A swap pushed from a swapped nested copy lands in the slot's main alone,
/// not in the main the copy shows.
pub fn landed(doc: &Document, root: NodeId, pushed: &[Operation]) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = Vec::new();
    for main in [home(doc, root), slot_home(doc, root)]
        .into_iter()
        .flatten()
    {
        let within: FxHashSet<NodeId> = crate::build::subtree_nodes(doc, &[main])
            .into_iter()
            .collect();
        if !out.contains(&main) && pushed.iter().any(|op| lands_in(op, &within)) {
            out.push(main);
        }
    }
    out
}

/// Whether `op` writes into one of the nodes `within`: a layer inserted under
/// one, one deleted, or a field overwritten on one.
fn lands_in(op: &Operation, within: &FxHashSet<NodeId>) -> bool {
    match op {
        Operation::InsertSubtree { parent, .. } => within.contains(parent),
        Operation::DeleteNode { id } => within.contains(id),
        _ => op.overwrites().is_some_and(|i| within.contains(&i)),
    }
}

/// The node a field of copy `c` is pushed into: the slot (its link) for a swapped
/// copy's slot fields, else [`source_of`] — the split [`reset::overrides`] makes.
fn target_of(doc: &Document, c: NodeId, op: &Operation) -> Option<NodeId> {
    let n = doc.get(c)?;
    match n.swap.is_some() && crate::swap::is_slot_field(op) {
        true => n.link,
        false => source_of(doc, c),
    }
}

/// Whether `op` (an override of `c`) is one a component property drives.
fn is_property(op: &Operation, c: NodeId, bound: &FxHashSet<(NodeId, PropKind)>) -> bool {
    match op {
        Operation::SetVisible { .. } | Operation::SetVisibleVar { .. } => {
            bound.contains(&(c, PropKind::Boolean))
        }
        Operation::SetText { .. } | Operation::SetTextVars { .. } => {
            bound.contains(&(c, PropKind::Text))
        }
        Operation::SetSwap { .. } => bound.contains(&(c, PropKind::Swap)),
        _ => false,
    }
}

/// The field `reset` writes, pushed: the target's own value with **the parts
/// `reset` would change on the copy** taken from the copy — so a mark's reset
/// of one axis, written as the whole size with the other axis the copy's own,
/// pushes that axis alone. Aimed at the push target; `None` where there is no
/// target inside `home`.
fn field_push(
    doc: &Document,
    c: NodeId,
    reset: &Operation,
    home: &FxHashSet<NodeId>,
) -> Option<Operation> {
    let key = reset::field_key(reset)?;
    let target = target_of(doc, c, reset).filter(|t| home.contains(t))?;
    let of = |n: NodeId| {
        reset::state_ops(doc.get(n)?)
            .into_iter()
            .find(|o| reset::field_key(o) == Some(key))
    };
    let (mine, theirs) = (of(c)?, of(target)?);
    let (Ok(r), Ok(m), Ok(t)) = (
        serde_json::to_value(reset.retargeted(c)?),
        serde_json::to_value(&mine),
        serde_json::to_value(theirs.retargeted(c)?),
    ) else {
        return mine.retargeted(target);
    };
    match serde_json::from_value::<Operation>(changed_from(&r, &m, t)) {
        Ok(op) => op.retargeted(target),
        Err(_) => mine.retargeted(target),
    }
}

/// `theirs` with each part where `reset` and `mine` differ taken from `mine`:
/// objects part by part, a run of numbers (a transform's coefficients) number by
/// number, anything else — a keyed list, a string — whole.
fn changed_from(
    reset: &serde_json::Value,
    mine: &serde_json::Value,
    theirs: serde_json::Value,
) -> serde_json::Value {
    use serde_json::Value;
    if reset == mine {
        return theirs;
    }
    match (reset, mine, theirs) {
        (Value::Object(r), Value::Object(m), Value::Object(mut t))
            if r.keys().eq(m.keys()) && m.keys().all(|k| t.contains_key(k)) =>
        {
            for (k, mv) in m {
                let tv = t.remove(k).unwrap_or(Value::Null);
                t.insert(k.clone(), changed_from(&r[k], mv, tv));
            }
            Value::Object(t)
        }
        (Value::Array(r), Value::Array(m), Value::Array(t))
            if r.len() == m.len()
                && m.len() == t.len()
                && m.iter().chain(r).chain(&t).all(Value::is_number) =>
        {
            Value::Array(
                r.iter()
                    .zip(m)
                    .zip(t)
                    .map(|((r, m), t)| if r == m { t } else { m.clone() })
                    .collect(),
            )
        }
        _ => mine.clone(),
    }
}

/// The swap of the swapped copy `c` pushed into its slot: the slot relinked (a
/// root linked straight to a main) or swapped (a nested copy) to show what `c`
/// shows, through the variant switch's rewrite.
fn swap_push(
    doc: &Document,
    c: NodeId,
    home: &FxHashSet<NodeId>,
    ids: &mut IdSource,
) -> Option<Vec<Operation>> {
    let n = doc.get(c)?;
    let (slot, to) = (n.link.filter(|l| home.contains(l))?, n.swap?);
    crate::variant::switch(doc, slot, to, ids).map(|tx| tx.0)
}

/// One field override of copy `c` as its push: a swap through [`swap_push`],
/// anything else through [`field_push`].
fn push_override(
    doc: &Document,
    c: NodeId,
    reset: &Operation,
    home: &FxHashSet<NodeId>,
    ids: &mut IdSource,
) -> Vec<Operation> {
    match reset {
        Operation::SetSwap { .. } => swap_push(doc, c, home, ids).unwrap_or_default(),
        _ => field_push(doc, c, reset, home).into_iter().collect(),
    }
}

/// Each overridden field under `root`, with whether a property drives it.
fn overridden(doc: &Document, root: NodeId) -> Vec<(NodeId, reset::Override, bool)> {
    let bound = crate::variant::property_fields(doc, root);
    reset::scope_nodes(doc, root)
        .into_iter()
        .flat_map(|c| {
            let bound = &bound;
            reset::overrides(doc, c).into_iter().map(move |o| {
                let prop = is_property(&o.reset, c, bound);
                (c, o, prop)
            })
        })
        .collect()
}

/// Whether the override can be pushed at all: its target lies in `home`.
fn reaches(doc: &Document, c: NodeId, reset: &Operation, home: &FxHashSet<NodeId>) -> bool {
    match reset {
        Operation::SetSwap { .. } => doc
            .get(c)
            .and_then(|n| n.link)
            .is_some_and(|l| home.contains(&l)),
        _ => target_of(doc, c, reset).is_some_and(|t| home.contains(&t)),
    }
}

/// The outermost local layers under `root`: unlinked (or linked elsewhere)
/// layers whose parent is one of the instance's copies.
fn local_layers(doc: &Document, root: NodeId) -> (Vec<NodeId>, FxHashSet<NodeId>) {
    let scope: FxHashSet<NodeId> = reset::scope_nodes(doc, root).into_iter().collect();
    let locals = crate::build::subtree_nodes(doc, &[root])
        .into_iter()
        .filter(|id| {
            !scope.contains(id)
                && doc
                    .get(*id)
                    .and_then(|n| n.parent)
                    .is_some_and(|p| scope.contains(&p))
        })
        .collect();
    (locals, scope)
}

/// Why the local layer `layer` cannot be pushed into its parent's source, or
/// `None` when it can. Also `None` for a layer that is not local.
pub fn layer_blocked(doc: &Document, layer: NodeId) -> Option<LayerBlocked> {
    let root = component::instance_root(doc, doc.get(layer)?.parent?)?;
    let (_, scope) = local_layers(doc, root);
    layer_blocked_in(doc, layer, &scope)
}

fn layer_blocked_in(
    doc: &Document,
    layer: NodeId,
    scope: &FxHashSet<NodeId>,
) -> Option<LayerBlocked> {
    if crate::build::subtree_nodes(doc, &[layer])
        .iter()
        .any(|n| scope.contains(n))
    {
        return Some(LayerBlocked::WrapsLinked);
    }
    let src = source_of(doc, doc.get(layer)?.parent?)?;
    (!component::can_hold(doc, src, layer)).then_some(LayerBlocked::HoldsMain)
}

/// Whether `layer` is a local layer of an instance that [`push_layer`] can push.
pub fn is_local(doc: &Document, layer: NodeId) -> bool {
    let Some(root) = doc
        .get(layer)
        .and_then(|n| n.parent)
        .and_then(|p| component::instance_root(doc, p))
    else {
        return false;
    };
    local_layers(doc, root).0.contains(&layer)
}

/// What each push would write for the instance at `root`.
pub fn pushable(doc: &Document, root: NodeId) -> Pushable {
    let home = inside(doc, root);
    if home.is_empty() {
        return Pushable::default();
    }
    let mut out = Pushable::default();
    let mut props: FxHashSet<(NodeId, std::mem::Discriminant<Operation>)> = FxHashSet::default();
    for (c, o, prop) in overridden(doc, root) {
        if !reaches(doc, c, &o.reset, &home) {
            continue;
        }
        match prop {
            true => {
                props.insert((c, std::mem::discriminant(&o.reset)));
            }
            false => out.fields += o.units,
        }
    }
    out.properties = props.len();
    let (locals, scope) = local_layers(doc, root);
    for l in locals {
        let parent_in = doc
            .get(l)
            .and_then(|n| n.parent)
            .and_then(|p| source_of(doc, p))
            .is_some_and(|s| home.contains(&s));
        if !parent_in {
            continue;
        }
        match layer_blocked_in(doc, l, &scope) {
            None => out.layers += 1,
            Some(_) => out.layers_blocked += 1,
        }
    }
    out.order = order_ops(doc, root, &home).len();
    out.removals = removal_ops(doc, root, &home).len();
    out
}

/// The operations push `kind` from the instance at `root` writes, with every
/// placing write's insets and every resize's item stated beside it. Empty when
/// it is [`blocked`] or there is nothing to push.
pub fn push(doc: &Document, kind: Kind, root: NodeId, ids: &mut IdSource) -> Vec<Operation> {
    if blocked(doc, root).is_some() {
        return Vec::new();
    }
    let home = inside(doc, root);
    if home.is_empty() {
        return Vec::new();
    }
    let fields = |doc: &Document, ids: &mut IdSource, props: bool| -> Vec<Operation> {
        let mut out = Vec::new();
        for (c, o, prop) in overridden(doc, root) {
            if prop == props {
                out.extend(push_override(doc, c, &o.reset, &home, ids));
            }
        }
        out
    };
    let ops = match kind {
        Kind::Properties => fields(doc, ids, true),
        Kind::Fields => fields(doc, ids, false),
        Kind::Layers => layer_ops(doc, root, &home, ids),
        Kind::Order => order_ops(doc, root, &home).into_iter().flatten().collect(),
        Kind::Removals => removal_ops(doc, root, &home),
        Kind::All => {
            // The layers first, then the order read on the document they leave,
            // so a pushed layer is ordered with its siblings; the fields from the
            // document as it is, which the layers do not change.
            let mut out = layer_ops(doc, root, &home, ids);
            let mut scratch = doc.clone();
            if scratch.apply_unchecked(&Transaction(out.clone())).is_ok() {
                let home_after = inside(&scratch, root);
                out.extend(order_ops(&scratch, root, &home_after).into_iter().flatten());
            }
            out.extend(fields(doc, ids, true));
            out.extend(fields(doc, ids, false));
            out
        }
    };
    stated(doc, ops)
}

/// The push of one field override each, as a card header's menu or a field's
/// mark offers it beside its reset: `resets` are reset ops aimed at copies
/// (`reset::Override::reset`), and each comes back as the copy's own value aimed
/// at its push target. Stated as [`push`] is.
pub fn fields(doc: &Document, resets: &[Operation], ids: &mut IdSource) -> Vec<Operation> {
    let mut out = Vec::new();
    for reset in resets {
        let Some(c) = reset.overwrites() else {
            continue;
        };
        let Some(root) = component::instance_root(doc, c) else {
            continue;
        };
        if blocked(doc, root).is_some() {
            continue;
        }
        let home = inside(doc, root);
        out.extend(push_override(doc, c, reset, &home, ids));
    }
    stated(doc, out)
}

/// Whether a push writes one field of one main node twice: two copies pushing
/// into the same node, where the last would win silently. A door that pushes
/// several resets at once refuses such a push.
pub fn writes_twice(ops: &[Operation]) -> bool {
    let mut seen = FxHashSet::default();
    ops.iter()
        .filter_map(Operation::shape_key)
        .any(|key| !seen.insert(key))
}

/// The list item `item` the copy `c` removed, deleted from its source's list —
/// a ghost row's *Push removal to main*. `list` is the list's reset op (any
/// value; only its kind is read). `None` where the source has no such item or
/// the instance cannot push.
pub fn item_removal(
    doc: &Document,
    c: NodeId,
    list: &Operation,
    item: ItemId,
) -> Option<Transaction> {
    let root = component::instance_root(doc, c)?;
    if blocked(doc, root).is_some() {
        return None;
    }
    let target = target_of(doc, c, list).filter(|t| inside(doc, root).contains(t))?;
    let key = reset::field_key(list)?;
    let theirs = reset::state_ops(doc.get(target)?)
        .into_iter()
        .find(|o| reset::field_key(o) == Some(key))?;
    let before = theirs.clone();
    let mut op = theirs;
    match &mut op {
        Operation::SetFills { fills, .. } => fills.retain(|f| f.id != item),
        Operation::SetStrokes { strokes, .. } => strokes.retain(|f| f.id != item),
        Operation::SetEffects { effects, .. } => effects.retain(|f| f.id != item),
        Operation::SetExports { exports, .. } => exports.retain(|f| f.id != item),
        Operation::SetLayoutGrids { grids, .. } => grids.retain(|f| f.id != item),
        _ => return None,
    }
    (op != before).then(|| Transaction(vec![op]))
}

/// The local layer `layer` pushed into the main: copied in under its parent's
/// source at the same place among its siblings, and linked to that copy in the
/// same transaction, so the commit gives it to every other instance and not to
/// this one a second time. Empty when it [`layer_blocked`] or the instance is
/// [`blocked`].
pub fn push_layer(doc: &Document, layer: NodeId, ids: &mut IdSource) -> Vec<Operation> {
    let Some(root) = doc
        .get(layer)
        .and_then(|n| n.parent)
        .and_then(|p| component::instance_root(doc, p))
    else {
        return Vec::new();
    };
    if blocked(doc, root).is_some() || !is_local(doc, layer) {
        return Vec::new();
    }
    let home = inside(doc, root);
    let (_, scope) = local_layers(doc, root);
    if layer_blocked_in(doc, layer, &scope).is_some() {
        return Vec::new();
    }
    let mut sim = FxHashMap::default();
    let mut links = FxHashMap::default();
    let ops = insert_layer(doc, layer, &home, ids, &mut sim, &mut links);
    stated(doc, ops)
}

/// Every pushable outermost local layer under `root`, as [`push_layer`] pushes
/// one, placed against each other as they land.
fn layer_ops(
    doc: &Document,
    root: NodeId,
    home: &FxHashSet<NodeId>,
    ids: &mut IdSource,
) -> Vec<Operation> {
    let (locals, scope) = local_layers(doc, root);
    let mut sim = FxHashMap::default();
    let mut links = FxHashMap::default();
    let mut out = Vec::new();
    for l in locals {
        if layer_blocked_in(doc, l, &scope).is_none() {
            out.extend(insert_layer(doc, l, home, ids, &mut sim, &mut links));
        }
    }
    out
}

/// [`push_layer`]'s body. `sim` holds each main parent's children as inserts
/// land, `links` each pushed layer's new main node, so two layers pushed under
/// one parent place against each other.
fn insert_layer(
    doc: &Document,
    layer: NodeId,
    home: &FxHashSet<NodeId>,
    ids: &mut IdSource,
    sim: &mut FxHashMap<NodeId, Vec<NodeId>>,
    links: &mut FxHashMap<NodeId, NodeId>,
) -> Vec<Operation> {
    let Some(parent) = doc.get(layer).and_then(|n| n.parent) else {
        return Vec::new();
    };
    let Some(src) = source_of(doc, parent).filter(|s| home.contains(s)) else {
        return Vec::new();
    };
    let (Some(template), Some(p)) = (doc.capture_subtree(layer), doc.get(parent)) else {
        return Vec::new();
    };
    let Some((copy, _)) = crate::document::remap_subtree(&template, ids) else {
        return Vec::new();
    };
    let kids = sim
        .get(&src)
        .cloned()
        .or_else(|| doc.get(src).map(|n| n.children.clone()))
        .unwrap_or_default();
    // The instance's siblings each name their main counterpart by their link, or
    // by the node a push in this same transaction gave them.
    let index = crate::propagate::anchor(&p.children, layer, &kids, |k| {
        links
            .get(&k)
            .copied()
            .or_else(|| doc.get(k).and_then(|n| n.link))
            .filter(|l| kids.contains(l))
    })
    .min(kids.len());
    links.insert(layer, copy[0].id);
    let mut placed = kids;
    placed.insert(index, copy[0].id);
    sim.insert(src, placed);
    let relinks: Vec<Operation> = template
        .iter()
        .zip(&copy)
        .map(|(t, k)| Operation::SetLink {
            id: t.id,
            link: Some(k.id),
        })
        .collect();
    let mut out = vec![Operation::InsertSubtree {
        nodes: copy,
        parent: src,
        index,
    }];
    out.extend(relinks);
    out
}

/// One `Reorder` run per main parent whose children the instance orders
/// otherwise: the linked children's sources take the instance's order in the
/// slots they hold, the main's children the instance removed keeping theirs.
fn order_ops(doc: &Document, root: NodeId, home: &FxHashSet<NodeId>) -> Vec<Vec<Operation>> {
    let mut out = Vec::new();
    for p in reset::scope_nodes(doc, root) {
        if reset::order_target(doc, p).is_none() {
            continue;
        }
        let (Some(s), Some(pn)) = (source_of(doc, p).filter(|s| home.contains(s)), doc.get(p))
        else {
            continue;
        };
        let Some(theirs) = doc.get(s).map(|n| n.children.clone()) else {
            continue;
        };
        let ours: Vec<NodeId> = pn
            .children
            .iter()
            .filter_map(|k| doc.get(*k).and_then(|n| n.link))
            .filter(|l| theirs.contains(l))
            .collect();
        let slots: Vec<usize> = (0..theirs.len())
            .filter(|&i| ours.contains(&theirs[i]))
            .collect();
        let mut target = theirs.clone();
        for (slot, id) in slots.into_iter().zip(ours) {
            target[slot] = id;
        }
        if target != theirs {
            out.push(
                target
                    .into_iter()
                    .enumerate()
                    .map(|(index, id)| Operation::Reorder { id, index })
                    .collect(),
            );
        }
    }
    out
}

/// A `DeleteNode` for each of the main's layers the instance at `root` removed.
fn removal_ops(doc: &Document, root: NodeId, home: &FxHashSet<NodeId>) -> Vec<Operation> {
    let mut seen = FxHashSet::default();
    reset::missing(doc, &reset::scope_nodes(doc, root))
        .into_iter()
        .map(|(_, c)| c)
        .filter(|c| home.contains(c) && seen.insert(*c))
        .map(|id| Operation::DeleteNode { id })
        .collect()
}

/// `ops` with the placement stated: every `SetTransform`, or `SetGeometry` that
/// resizes, on a main node with no `SetInsets` beside it takes that node's own
/// insets, and every resize its own item, so the commit door does not read the
/// push as a hand's move (`keep_insets`) or resize (`flex_holds`). A copy whose
/// insets or item differ pushes them as fields of their own.
fn stated(doc: &Document, mut ops: Vec<Operation>) -> Vec<Operation> {
    let named = |ops: &[Operation], id: NodeId, insets: bool| {
        ops.iter().any(|o| match o {
            Operation::SetInsets { id: i, .. } => insets && *i == id,
            Operation::SetLayoutItem { id: i, .. } => !insets && *i == id,
            _ => false,
        })
    };
    let mut extra: Vec<Operation> = Vec::new();
    for op in &ops {
        let (id, resizes) = match op {
            Operation::SetTransform { id, .. } => (*id, false),
            Operation::SetGeometry { id, geometry } if geometry.resizes() => (*id, true),
            _ => continue,
        };
        let Some(n) = doc.get(id) else { continue };
        if !named(&ops, id, true) && !named(&extra, id, true) {
            extra.push(Operation::SetInsets {
                id,
                insets: *n.insets(),
            });
        }
        if resizes && !named(&ops, id, false) && !named(&extra, id, false) {
            extra.push(Operation::SetLayoutItem {
                id,
                item: *n.item(),
            });
        }
    }
    ops.extend(extra);
    ops
}

/// What a commit of a push did to the main's other instances, for its toast and
/// a row's tooltip: how many took it, and how many kept a value of their own
/// (or, for a removal, kept the layer, unlinked).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub follow: usize,
    pub kept: usize,
}

/// [`Outcome`] of the push `pushed` from the instance at `root`, read from the
/// document before the commit and the one after it, over the instances of the
/// mains it [`landed`] in.
pub fn outcome(before: &Document, after: &Document, root: NodeId, pushed: &[Operation]) -> Outcome {
    let mains = landed(before, root, pushed);
    if mains.is_empty() {
        return Outcome::default();
    }
    let within: FxHashSet<NodeId> = crate::build::subtree_nodes(before, &mains)
        .into_iter()
        .collect();
    // The whole instance the push came from, not only `root`: pushed from a
    // nested copy, the instance holding it now matches the main, and is no
    // other instance following.
    let outer = component::outermost_instance_root(after, root).unwrap_or(root);
    let ours: FxHashSet<NodeId> = crate::build::subtree_nodes(after, &[outer])
        .into_iter()
        .collect();
    // The main nodes written, each with the fields written on it; the layers
    // added; the layers deleted.
    let mut written: FxHashMap<NodeId, Vec<Operation>> = FxHashMap::default();
    let mut added: Vec<NodeId> = Vec::new();
    let mut deleted: Vec<NodeId> = Vec::new();
    for op in pushed {
        match op {
            Operation::InsertSubtree { nodes, parent, .. } if within.contains(parent) => {
                added.extend(nodes.first().map(|n| n.id));
            }
            Operation::DeleteNode { id } if within.contains(id) => deleted.push(*id),
            _ => {
                if let Some(id) = op.overwrites().filter(|i| within.contains(i))
                    && reset::field_key(op).is_some()
                {
                    written.entry(id).or_default().push(op.clone());
                }
            }
        }
    }
    let others: Vec<NodeId> = component::instances_by_main(after)
        .into_values()
        .flatten()
        .filter(|r| !ours.contains(r) && mains.iter().any(|m| reaches_main(after, *r, *m)))
        .collect();
    let mut out = Outcome::default();
    for r in others {
        let nodes = crate::build::subtree_nodes(after, &[r]);
        let mut follows = false;
        let mut keeps = false;
        for n in &nodes {
            if added.iter().any(|a| links_to(after, *n, *a)) {
                follows = true;
            }
            for (t, ops) in &written {
                if !links_to(after, *n, *t) {
                    continue;
                }
                let (Some(mine), Some(main_node)) = (after.get(*n), after.get(*t)) else {
                    continue;
                };
                let mine = reset::state_ops(mine);
                let theirs = reset::state_ops(main_node);
                for op in ops {
                    let key = reset::field_key(op);
                    let a = mine.iter().find(|o| reset::field_key(o) == key);
                    let b = theirs
                        .iter()
                        .find(|o| reset::field_key(o) == key)
                        .and_then(|o| o.retargeted(*n));
                    match a == b.as_ref() {
                        true => follows = true,
                        false => keeps = true,
                    }
                }
            }
        }
        if !deleted.is_empty() {
            let before_nodes = crate::build::subtree_nodes(before, &[r]);
            for n in before_nodes {
                if deleted.iter().any(|d| links_to(before, n, *d)) {
                    match after.get(n).is_some() {
                        true => keeps = true,
                        false => follows = true,
                    }
                }
            }
        }
        if keeps {
            out.kept += 1;
        } else if follows {
            out.follow += 1;
        }
    }
    out
}

/// [`outcome`] ahead of the commit: `ops` committed on a scratch document through
/// the passes a commit runs (`propagate::owed`). For a row's tooltip. The ids
/// the passes mint are a throwaway actor's, `actor` turned so they cannot meet
/// the session's.
pub fn predicted(doc: &Document, root: NodeId, ops: &[Operation], actor: u64) -> Outcome {
    let mut ids = IdSource::new(actor ^ 0x9E37_79B9_7F4A_7C15);
    let mut tx = Transaction(ops.to_vec());
    crate::propagate::owed(doc, &mut tx, &mut ids);
    let mut after = doc.clone();
    if after.apply_unchecked(&tx).is_err() {
        return Outcome::default();
    }
    outcome(doc, &after, root, ops)
}

/// Whether `id`'s chain of links passes through `target`.
fn links_to(doc: &Document, id: NodeId, target: NodeId) -> bool {
    let mut at = doc.get(id).and_then(|n| n.link);
    for _ in 0..64 {
        match at {
            Some(a) if a == target => return true,
            Some(a) => at = doc.get(a).and_then(|n| n.link),
            None => return false,
        }
    }
    false
}

/// Whether the instance root `r`'s chain of links reaches `main`.
fn reaches_main(doc: &Document, r: NodeId, main: NodeId) -> bool {
    links_to(doc, r, main)
}
