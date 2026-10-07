//! Resetting an instance to its main — §5.3d's reset family (build step 5, §15
//! D979 (a), D981).
//!
//! Nothing records which values a copy overrides (§15 D979 (a)), so the three
//! questions a reset asks are all answered by **comparing a copy with its
//! source**, the node it links to one level up:
//! - **a field** is overridden where the copy's value differs from its source's,
//!   and resetting it writes the source's value back ([`overrides`]);
//! - **a removed child** is a child of a source with no counterpart anywhere in the
//!   copy's instance, and restoring it inserts a fresh linked copy at its anchor —
//!   the propagation pass's placement rule ([`restore_children`]);
//! - **the order** is off where the copy's linked children are not in their
//!   sources' order, and resetting it puts them back in the slots they occupy
//!   ([`reset_order`]).
//!
//! **No reset deletes a local layer** (§15 D981 (1)): a layer of the instance's
//! own keeps its place. A list reset is the source's list, and the copy's own
//! items go with it (§15 D994, `[X4-L2-01]`).
//!
//! **Reading a node as operations.** [`state_ops`] writes every field the
//! propagation pass carries as the operation that would set it, so a copy and its
//! source compare op by op and the reset is the source's op aimed at the copy —
//! the same currency `crate::propagate` deals in, which is what keeps the two from
//! disagreeing about what a field is. Locks, the component flag and the link are
//! not fields (`propagate`'s `Mode::Skip`), and an instance root's placement is its
//! own (`propagate::is_placement`): neither is ever an override.
//!
//! **Every reset is an edit like any other**: it goes through the session's
//! commit, so a reset of a nested copy inside a main reaches that main's instances
//! by the ordinary propagation.

use crate::document::Document;
use crate::id::{IdSource, NodeId};
use crate::item::{ItemId, Keyed};
use crate::node::{Node, NodeKind, Paint};
use crate::op::{GeometryPatch, Operation, Transaction};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;
use serde_json::Value;

/// A node's whole state as the operations that would write it: one per field the
/// propagation pass carries, the kind's fields included, and nothing for locks, the
/// component flag or the link.
///
/// **Destructured whole on purpose**: a field added to `Node` stops the build here,
/// and its author says whether a copy can override it.
pub fn state_ops(node: &Node) -> Vec<Operation> {
    let Node {
        id,
        parent: _,
        children: _,
        kind,
        transform,
        name,
        visible,
        locked: _,
        proportions_locked: _,
        opacity,
        clip,
        mask,
        mask_mode,
        fill_rule,
        paint: Paint { fills, strokes },
        effects,
        pivot,
        exports,
        grids,
        insets,
        display,
        item,
        component: _,
        link: _,
        // A swap is an override (§15 D983), but not a field one: resetting it
        // rewrites the copy's children, which `overrides` writes as a `SetSwap`
        // and `crate::swap::settle` expands at the commit.
        swap: _,
        // A main's or a set's own, read by an instance through its link (§15 D982).
        set: _,
        variant: _,
        props: _,
    } = node;
    let id = *id;
    use Operation as O;
    let mut ops = vec![
        O::SetTransform {
            id,
            transform: *transform,
        },
        O::SetName {
            id,
            name: name.clone(),
        },
        O::SetVisible {
            id,
            visible: *visible,
        },
        O::SetOpacity {
            id,
            opacity: *opacity,
        },
        O::SetClip { id, clip: *clip },
        O::SetMask { id, mask: *mask },
        O::SetMaskMode {
            id,
            mode: *mask_mode,
        },
        O::SetFillRule {
            id,
            rule: *fill_rule,
        },
        O::SetFills {
            id,
            fills: fills.clone(),
        },
        O::SetStrokes {
            id,
            strokes: strokes.clone(),
        },
        O::SetEffects {
            id,
            effects: effects.clone(),
        },
        O::SetPivot { id, pivot: *pivot },
        O::SetExports {
            id,
            exports: exports.clone(),
        },
        O::SetLayoutGrids {
            id,
            grids: grids.clone(),
        },
        O::SetInsets {
            id,
            insets: *insets,
        },
        O::SetDisplay {
            id,
            display: display.clone(),
        },
        O::SetLayoutItem { id, item: *item },
    ];
    ops.extend(kind_ops(id, kind));
    ops
}

/// The kind's fields as operations — the geometry patches, and a text's content,
/// styles and rail. Wildcard-free, like [`state_ops`].
fn kind_ops(id: NodeId, kind: &NodeKind) -> Vec<Operation> {
    use GeometryPatch as G;
    let geo = |geometry: G| Operation::SetGeometry { id, geometry };
    match kind {
        NodeKind::Root | NodeKind::Group => Vec::new(),
        NodeKind::Artboard { size } | NodeKind::Ellipse { size } => vec![geo(G::Size(*size))],
        NodeKind::Rect { size, corner_radii } => {
            vec![geo(G::Size(*size)), geo(G::CornerRadii(*corner_radii))]
        }
        NodeKind::Polygon { size, sides } => vec![geo(G::Size(*size)), geo(G::Sides(*sides))],
        NodeKind::Star {
            size,
            points,
            inner_ratio,
        } => vec![
            geo(G::Size(*size)),
            geo(G::Sides(*points)),
            geo(G::InnerRatio(*inner_ratio)),
        ],
        NodeKind::Line { end } => vec![geo(G::LineEnd(*end))],
        NodeKind::Path { path, corner_radii } => vec![geo(G::Path {
            path: path.clone(),
            corner_radii: corner_radii.clone(),
        })],
        NodeKind::Boolean { op } => vec![geo(G::BoolOp(*op))],
        NodeKind::Text {
            content,
            style,
            spans,
            para_spans,
            paragraph,
            block,
            sizing,
            on_path,
            on_path_flip,
            on_path_offset,
        } => vec![
            // Content and its spans are one unit (§5.3d: *"Do not split it"*).
            Operation::SetText {
                id,
                content: content.clone(),
                spans: spans.clone(),
                para_spans: para_spans.clone(),
            },
            Operation::SetTextStyle {
                id,
                style: (**style).clone(),
                spans: None,
            },
            Operation::SetParagraphStyle {
                id,
                paragraph: paragraph.clone(),
                spans: None,
            },
            Operation::SetBlockStyle { id, block: *block },
            geo(G::TextSizing(*sizing)),
            geo(G::TextPath(on_path.as_deref().cloned())),
            geo(G::TextPathFlip(*on_path_flip)),
            geo(G::TextPathOffset(*on_path_offset)),
        ],
    }
}

/// One overridden field of a copy: the operation that resets it, and how many
/// **units** of it differ — the count a card header and *Reset fields N* show.
///
/// A plain value is one unit. A struct payload (a text style, insets, a layout
/// item) counts each field that differs, at any depth, so a typography card with
/// four overrides says four. A keyed list counts each of its source's items the
/// copy changed or removed, one more where it reordered them, and each item of
/// the copy's own as one too, since a list reset removes it (§15 D994).
#[derive(Clone, Debug, PartialEq)]
pub struct Override {
    pub reset: Operation,
    pub units: usize,
}

/// The node `id` is compared with: its link, when it sits in an instance — its
/// **swap** when it is swapped (§15 D983), for its children and every field but
/// the slot's, which [`overrides`] compares with the link. `None` for anything
/// unlinked — a main, an ordinary layer, a local addition.
pub fn source_of(doc: &Document, id: NodeId) -> Option<NodeId> {
    crate::swap::content_source(doc, id).filter(|l| doc.get(*l).is_some())
}

/// The node `id`'s **slot fields** — placement, size, visibility
/// ([`crate::swap::is_slot_field`]) — are compared with: [`source_of`], except
/// for a swapped copy, whose slot keeps them from its **link** (§15 D983 (3)) —
/// the half [`overrides`] already splits. A placement reset reads its insets and
/// a Transform mark its X, Y, W and H from here; read from [`source_of`] they
/// named the swap target's page position and size, and a reset moved the slot
/// out of its button or unpinned it (`[X9.1-L1-01]`).
pub fn slot_source_of(doc: &Document, id: NodeId) -> Option<NodeId> {
    let n = doc.get(id)?;
    match n.swap {
        Some(_) => n.link.filter(|l| doc.get(*l).is_some()),
        None => source_of(doc, id),
    }
}

/// Whether `id`'s **placement is its own** — an instance linked straight to a
/// main, whose transform, insets, layout item and visibility never compare with
/// the main's (§5.3d). A nested copy inside an outer main is not: its place is the
/// outer main's to move.
pub fn placement_is_own(doc: &Document, id: NodeId) -> bool {
    crate::propagate::linked_to_main(doc, id)
}

/// Every field in which the copy `id` differs from its source, each with the
/// operation that resets it. Empty for an unlinked node.
///
/// A copy whose kind is not its source's — a variant no edit can cross — compares
/// only the fields every node has.
///
/// **A swapped copy** (§15 D983) compares its slot's fields — placement, size,
/// visibility — with its link and the rest with its swap, and the **swap itself
/// is an override** (the maintainer's ruling (2)): one unit, reset by clearing it,
/// which the commit expands back into the slot's own contents (`swap::settle`).
pub fn overrides(doc: &Document, id: NodeId) -> Vec<Override> {
    let (Some(copy), Some(src)) = (doc.get(id), source_of(doc, id).and_then(|s| doc.get(s))) else {
        return Vec::new();
    };
    let own_place = placement_is_own(doc, id);
    let slot = copy.swap.and(copy.link).and_then(|l| doc.get(l));
    let same_kind = std::mem::discriminant(&copy.kind) == std::mem::discriminant(&src.kind);
    let mine: FxHashMap<_, Operation> = state_ops(copy)
        .into_iter()
        .filter_map(|op| Some((field_key(&op)?, op)))
        .collect();
    let mut out = Vec::new();
    if copy.swap.is_some() {
        out.push(Override {
            reset: Operation::SetSwap { id, swap: None },
            units: 1,
        });
    }
    // The slot's fields from the slot, everything else from the swap.
    let theirs_all: Vec<Operation> = match slot {
        Some(slot) => state_ops(slot)
            .into_iter()
            .filter(crate::swap::is_slot_field)
            .chain(
                state_ops(src)
                    .into_iter()
                    .filter(|op| !crate::swap::is_slot_field(op)),
            )
            .collect(),
        None => state_ops(src),
    };
    for theirs in theirs_all {
        if own_place && crate::propagate::is_placement(&theirs) {
            continue;
        }
        if !same_kind && is_kind_op(&theirs) {
            continue;
        }
        let Some(cur) = field_key(&theirs).and_then(|k| mine.get(&k)) else {
            continue;
        };
        let Some(theirs) = theirs.retargeted(id) else {
            continue;
        };
        if let Some(o) = compare(&theirs, cur) {
            out.push(o);
        }
    }
    out
}

/// The field an operation writes, without its subject: the variant, and the patch
/// variant for a geometry edit — `Operation::shape_key` less the node.
pub(crate) fn field_key(
    op: &Operation,
) -> Option<(
    std::mem::Discriminant<Operation>,
    Option<std::mem::Discriminant<GeometryPatch>>,
)> {
    op.shape_key().map(|(op, patch, _)| (op, patch))
}

pub(crate) fn is_kind_op(op: &Operation) -> bool {
    matches!(
        op,
        Operation::SetGeometry { .. }
            | Operation::SetText { .. }
            | Operation::SetTextStyle { .. }
            | Operation::SetParagraphStyle { .. }
            | Operation::SetBlockStyle { .. }
    )
}

/// `theirs` (the source's value, aimed at the copy) against `cur` (the copy's),
/// as an [`Override`] when they differ in anything a reset would change.
fn compare(theirs: &Operation, cur: &Operation) -> Option<Override> {
    use Operation as O;
    let units = match (theirs, cur) {
        (O::SetFills { fills: s, id }, O::SetFills { fills: c, .. }) => {
            return list(s, c, |fills| O::SetFills { id: *id, fills });
        }
        (O::SetStrokes { strokes: s, id }, O::SetStrokes { strokes: c, .. }) => {
            return list(s, c, |strokes| O::SetStrokes { id: *id, strokes });
        }
        (O::SetEffects { effects: s, id }, O::SetEffects { effects: c, .. }) => {
            return list(s, c, |effects| O::SetEffects { id: *id, effects });
        }
        (O::SetExports { exports: s, id }, O::SetExports { exports: c, .. }) => {
            return list(s, c, |exports| O::SetExports { id: *id, exports });
        }
        (O::SetLayoutGrids { grids: s, id }, O::SetLayoutGrids { grids: c, .. }) => {
            return list(s, c, |grids| O::SetLayoutGrids { id: *id, grids });
        }
        (O::SetTextStyle { style: s, .. }, O::SetTextStyle { style: c, .. }) => leaves(s, c),
        (O::SetParagraphStyle { paragraph: s, .. }, O::SetParagraphStyle { paragraph: c, .. }) => {
            leaves(s, c)
        }
        (O::SetBlockStyle { block: s, .. }, O::SetBlockStyle { block: c, .. }) => leaves(s, c),
        (O::SetInsets { insets: s, .. }, O::SetInsets { insets: c, .. }) => leaves(s, c),
        (O::SetDisplay { display: s, .. }, O::SetDisplay { display: c, .. }) => leaves(s, c),
        (O::SetLayoutItem { item: s, .. }, O::SetLayoutItem { item: c, .. }) => leaves(s, c),
        _ => usize::from(theirs != cur),
    };
    (units > 0).then(|| Override {
        reset: theirs.clone(),
        units,
    })
}

/// How many fields of two struct payloads differ, at any depth — read through
/// JSON the way `propagate`'s merge reads them, a field's absence a value of its
/// own and a changed enum variant one difference.
fn leaves<T: Serialize>(a: &T, b: &T) -> usize {
    match (serde_json::to_value(a), serde_json::to_value(b)) {
        (Ok(a), Ok(b)) => leaf_diffs(Some(&a), Some(&b)),
        _ => 1,
    }
}

fn leaf_diffs(a: Option<&Value>, b: Option<&Value>) -> usize {
    if a == b {
        return 0;
    }
    match (a, b) {
        (Some(Value::Object(x)), Some(Value::Object(y)))
            if !crate::propagate::variant_changed(x, y) =>
        {
            let keys: std::collections::BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            keys.into_iter()
                .map(|k| leaf_diffs(x.get(k), y.get(k)))
                .sum()
        }
        _ => 1,
    }
}

/// A keyed list against its source's: the [`Override`] resetting it, if any.
fn list<T: Clone + PartialEq>(
    src: &[Keyed<T>],
    cur: &[Keyed<T>],
    op: impl FnOnce(Vec<Keyed<T>>) -> Operation,
) -> Option<Override> {
    let units = item_units(src, cur);
    (units > 0).then(|| Override {
        reset: op(reset_items(src)),
        units,
    })
}

/// What a list item is to its instance, for the inspector's trailing slot and
/// ghost rows (§15 D981).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemState {
    /// Equal to its source's item.
    Follows,
    /// Its source has it, with another value.
    Overridden,
    /// The instance's own — its source has no item with this id.
    Local,
}

/// Each of `cur`'s items read against `src`, in `cur`'s order.
pub fn item_states<T: PartialEq>(src: &[Keyed<T>], cur: &[Keyed<T>]) -> Vec<ItemState> {
    cur.iter()
        .map(|c| match src.iter().find(|s| s.id == c.id) {
            None => ItemState::Local,
            Some(s) if s.value == c.value => ItemState::Follows,
            Some(_) => ItemState::Overridden,
        })
        .collect()
}

/// The source's items the instance removed — its ghost rows (§15 D981), in the
/// source's order.
pub fn removed_items<T: Clone>(src: &[Keyed<T>], cur: &[Keyed<T>]) -> Vec<Keyed<T>> {
    src.iter()
        .filter(|s| cur.iter().all(|c| c.id != s.id))
        .cloned()
        .collect()
}

/// `cur` with the source's item `id` put back — its value the source's, placed
/// after the counterpart of the item before it in the source, else before the
/// counterpart of the one after, else first: the children's anchor rule one level
/// down (§15 D980), except that a list's last resort is **first**, as
/// `propagate::items` places an item with nothing to follow — where a child's is
/// topmost. One overridden item's reset (its row's ↺). `cur` unchanged when the
/// source has no such item.
///
/// ⚠️ **Not a ghost row's *Restore* any more** (§15 D994): that resets the whole
/// list to the source's, the copy's own items included, so this function's
/// putting-back branch — the item absent from `cur` — has no caller in the app.
/// It stays because it is right and tested, and is the shape a per-item restore
/// would want if one is asked for again.
///
/// ⚠️ It called `propagate::anchor` until `arch-scribe` read the fallback: that
/// one ends at the end of the list, and this doc already said first.
pub fn reset_item<T: Clone>(src: &[Keyed<T>], cur: &[Keyed<T>], id: ItemId) -> Vec<Keyed<T>> {
    let Some(at_src) = src.iter().position(|s| s.id == id) else {
        return cur.to_vec();
    };
    let mut out = cur.to_vec();
    if let Some(at) = out.iter().position(|c| c.id == id) {
        out[at] = src[at_src].clone();
        return out;
    }
    let find = |sid: ItemId| out.iter().position(|c| c.id == sid);
    let index = src[..at_src]
        .iter()
        .rev()
        .find_map(|p| find(p.id))
        .map(|i| i + 1)
        .or_else(|| src[at_src + 1..].iter().find_map(|n| find(n.id)))
        .unwrap_or(0);
    out.insert(index, src[at_src].clone());
    out
}

/// How many of `src`'s items `cur` changed or removed, plus one for each item of
/// its own, plus one if it reordered the ones both still hold.
///
/// **An item of the copy's own counts** (§15 D994): a list's reset removes it
/// ([`reset_items`]), so a list that differs from its source only by an added
/// fill is a list with something to reset. It counted nothing while every reset
/// kept it (D981's clarification (1), overturned for lists).
fn item_units<T: PartialEq>(src: &[Keyed<T>], cur: &[Keyed<T>]) -> usize {
    let mut n = src
        .iter()
        .filter(|s| {
            cur.iter()
                .find(|c| c.id == s.id)
                .is_none_or(|c| c.value != s.value)
        })
        .count();
    let in_src: FxHashSet<ItemId> = src.iter().map(|s| s.id).collect();
    let in_cur: FxHashSet<ItemId> = cur.iter().map(|c| c.id).collect();
    n += cur.iter().filter(|c| !in_src.contains(&c.id)).count();
    let a: Vec<ItemId> = src
        .iter()
        .map(|s| s.id)
        .filter(|i| in_cur.contains(i))
        .collect();
    let b: Vec<ItemId> = cur
        .iter()
        .map(|c| c.id)
        .filter(|i| in_src.contains(i))
        .collect();
    if a != b {
        n += 1;
    }
    n
}

/// A list reset: **the source's list, exactly** — its items, its order, its values,
/// and none of the copy's own (§15 D994, the maintainer's ruling: *"reset the whole
/// fill card to the original's … removing anything the user added"*).
///
/// ⚠️ **This overturns D981's clarification (1) for lists**, which kept a copy's
/// own items through every reset, reinserted after the item they followed. That
/// left the recoloured case broken in the way that matters: an instance whose fill
/// was replaced holds its own item where the main's was, and a reset that put the
/// main's back *after* it left the instance's colour on top and nothing on screen
/// changed. Local **layers** still survive every reset; this is about lists.
fn reset_items<T: Clone>(src: &[Keyed<T>]) -> Vec<Keyed<T>> {
    src.to_vec()
}

// ── Scope ──────────────────────────────────────────────────────────────────────

/// The nodes a reset at `scope` touches: every node at or under it linked into its
/// instance's source — the instance's own copies, a nested instance's included,
/// and not a local instance of some other main placed inside it, which is a local
/// addition. Empty outside every instance. Preorder.
///
/// **A swapped copy in scope brings its own members** (§15 D983): they link into
/// its swap's main, not into the instance's source, and resetting the instance
/// resets them too — the swap with them.
///
/// ⚠️ **Only the members *under* it** (`[X4-L1-02]`). This grew one set with the
/// whole of every swap it met, keyed on link targets, so a local instance of the
/// main a slot is swapped to — placed anywhere after that slot — passed as a copy
/// (its link, the main, was in the swap's subtree; its members' links too): the
/// card read *0 local layers* and *Reset all* wrote its fields back to the main's,
/// dropped its own list items (D994) and restored its removed children. So each
/// node is read against the swapped copies **above it**, a layer linked to
/// something out of scope takes its subtree out with it (a local instance's
/// members are its own), and a link to a main admits only the scope's own root —
/// an instance placed inside its instance is never one of its copies.
pub fn scope_nodes(doc: &Document, scope: NodeId) -> Vec<NodeId> {
    let Some(root) = crate::component::instance_root(doc, scope) else {
        return Vec::new();
    };
    // The scope's own source, and the slot's for a swapped scope, so the root
    // itself — linked to its slot — is in it.
    let mut sources: Vec<NodeId> = source_of(doc, root).into_iter().collect();
    sources.extend(
        doc.get(root)
            .filter(|r| r.swap.is_some())
            .and_then(|r| r.link),
    );
    let within: FxHashSet<NodeId> = crate::build::subtree_nodes(doc, &sources)
        .into_iter()
        .collect();
    // Each swapped copy in scope → its swap's nodes, the main itself excluded.
    let mut swaps: FxHashMap<NodeId, FxHashSet<NodeId>> = FxHashMap::default();
    // Each node visited → the swapped copies in scope above its children, or
    // `None` under a layer that is not one of the instance's copies but is linked
    // (a local instance, or a member of one): nothing below it is in scope.
    let mut reach: FxHashMap<NodeId, Option<Vec<NodeId>>> = FxHashMap::default();
    let mut out = Vec::new();
    // Preorder, so a node's parent — and a swapped copy, before its members — is
    // visited first.
    for id in crate::build::subtree_nodes(doc, &[scope]) {
        let Some(n) = doc.get(id) else { continue };
        let above = if id == scope {
            Some(Vec::new())
        } else {
            n.parent.and_then(|p| reach.get(&p).cloned().flatten())
        };
        let Some(mut above) = above else {
            reach.insert(id, None);
            continue;
        };
        let copy = n.link.is_some_and(|l| {
            let to_main = doc.get(l).is_some_and(|s| s.component);
            (!to_main || id == root)
                && (within.contains(&l) || above.iter().any(|s| swaps[s].contains(&l)))
        });
        if !copy {
            // A local layer: an unlinked one may still hold copies (members
            // regrouped inside their instance); a linked one is someone else's.
            reach.insert(id, n.link.is_none().then_some(above));
            continue;
        }
        out.push(id);
        if let Some(s) = n.swap {
            let mut nodes: FxHashSet<NodeId> =
                crate::build::subtree_nodes(doc, &[s]).into_iter().collect();
            nodes.remove(&s);
            swaps.insert(id, nodes);
            above.push(id);
        }
        reach.insert(id, Some(above));
    }
    out
}

/// How far the instance at `scope` has drifted from its main — the counts §15
/// D981's card and menu rows show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Drift {
    /// Overridden field units ([`Override::units`]) — *Reset fields N*, and the
    /// card's *N overrides*.
    pub fields: usize,
    /// The source's children missing from the instance — *Restore removed
    /// children N*. A removed subtree counts once.
    pub removed: usize,
    /// Parents whose linked children are out of their sources' order — *Reset
    /// order*.
    pub order: usize,
    /// The instance's own layers, outermost only — the card's *N local layers*.
    pub local: usize,
}

impl Drift {
    /// Whether any reset would do anything.
    pub fn any(&self) -> bool {
        self.fields + self.removed + self.order > 0
    }
}

/// [`Drift`] at `scope`.
pub fn drift(doc: &Document, scope: NodeId) -> Drift {
    let nodes = scope_nodes(doc, scope);
    let set: FxHashSet<NodeId> = nodes.iter().copied().collect();
    let local = crate::build::subtree_nodes(doc, &[scope])
        .into_iter()
        .filter(|id| {
            !set.contains(id)
                && doc
                    .get(*id)
                    .and_then(|n| n.parent)
                    .is_some_and(|p| set.contains(&p))
        })
        .count();
    Drift {
        fields: nodes
            .iter()
            .flat_map(|n| overrides(doc, *n))
            .map(|o| o.units)
            .sum(),
        removed: missing(doc, &nodes).len(),
        order: nodes
            .iter()
            .filter(|n| order_target(doc, **n).is_some())
            .count(),
        local,
    }
}

/// What differs among **one parent's own children** — the part of [`Drift`] a
/// row in the layers panel owns (§15 D1003 (9)): the source's children with no
/// counterpart anywhere in the instance, and whether the linked children are out
/// of the source's order. A field override is the child's own row's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChildDrift {
    /// The source's children of `p` missing from the instance, as [`Drift::removed`]
    /// counts them.
    pub removed: usize,
    /// Whether `p`'s linked children are out of its source's order.
    pub order: bool,
}

impl ChildDrift {
    /// Whether anything about the children differs.
    pub fn any(&self) -> bool {
        self.removed > 0 || self.order
    }
}

/// [`ChildDrift`] of the copy `p`. Empty for anything unlinked — a main, an
/// ordinary layer, a local layer of an instance.
///
/// 🚨 **The case no row marked** (`[X11.1-L2-03]`): an **expanded** instance with
/// a removed or reordered child showed no dot anywhere. Each row asked
/// [`overrides`], fields only; a removed child has no row, and reordered children
/// each have no field override, so the mark went nowhere — collapsing the
/// instance showed it, the bubbled [`drift`] counting both. The maintainer's ruling
/// (§15 D1003 (9)) puts the dot on the parent whose children differ, expanded or
/// collapsed.
pub fn child_drift(doc: &Document, p: NodeId) -> ChildDrift {
    if source_of(doc, p).is_none() {
        return ChildDrift::default();
    }
    ChildDrift {
        removed: missing(doc, &[p]).len(),
        order: order_target(doc, p).is_some(),
    }
}

/// Each (copy, its source's child) where the child has no counterpart anywhere in
/// the copy's instance — moved elsewhere in the instance is not missing. A child
/// under a missing one is not asked about: restoring the outer one brings it.
fn missing(doc: &Document, nodes: &[NodeId]) -> Vec<(NodeId, NodeId)> {
    let mut linked_in: FxHashMap<NodeId, FxHashSet<NodeId>> = FxHashMap::default();
    let mut out = Vec::new();
    for &p in nodes {
        let (Some(src), Some(owner)) = (
            source_of(doc, p).and_then(|s| doc.get(s)),
            outermost_root(doc, p),
        ) else {
            continue;
        };
        let links = linked_in
            .entry(owner)
            .or_insert_with(|| links_under(doc, owner));
        out.extend(
            src.children
                .iter()
                .filter(|c| !links.contains(c))
                .map(|c| (p, *c)),
        );
    }
    out
}

/// The **outermost** instance root at or above `id` — where a restore looks for
/// counterparts the instance still holds.
///
/// ⚠️ **Not the nearest** (`component::instance_root`), which is what this read
/// first: a nested copy is an instance root itself, and a member dragged out of it
/// into the outer instance keeps its link — `settle_links` claims it for the outer
/// root, whose source contains its source — so a search under the nested copy
/// alone found it missing and restored a second node on the same source, under a
/// different root, where `LinkRule::SharedSource` cannot see it. Found by
/// `arch-scribe`; `a_member_dragged_out_of_a_nested_copy_is_not_restored_twice`.
///
/// ⚠️ **And it climbs only out of nested copies**, stopping at a root linked
/// straight to a main — an instance, or a local instance placed inside one. The
/// first fix climbed through every root, so a local instance's deleted child was
/// masked by the outer instance's own counterpart of the same source and never
/// restored (`a_local_instances_removed_child_is_not_masked_by_its_host`, also
/// `arch-scribe`'s). A member dragged out of a local instance is cut by
/// `settle_links` anyway, so nothing past that root can hold its links.
///
/// ⚠️ **Nor out of a swapped copy** (`[X4-L1-03]`). Its members link into its
/// swap (§15 D983), which the outer instance's source does not hold, so
/// `settle_links` cuts one dragged out of it as it cuts a local instance's —
/// and climbing past it let a **second** slot swapped to the same main mask the
/// first's deleted child: `b1`'s `r2` holds a `Shine` linked to the same Heart
/// node `r`'s was, and *Restore removed children* read 0
/// (`two_slots_swapped_to_one_main_do_not_mask_a_removed_child`).
fn outermost_root(doc: &Document, id: NodeId) -> Option<NodeId> {
    let mut root = crate::component::instance_root(doc, id)?;
    while !crate::propagate::linked_to_main(doc, root)
        && doc.get(root).is_some_and(|n| n.swap.is_none())
        && let Some(outer) = doc
            .get(root)
            .and_then(|n| n.parent)
            .and_then(|p| crate::component::instance_root(doc, p))
    {
        root = outer;
    }
    Some(root)
}

/// Every link held at or under `root`.
fn links_under(doc: &Document, root: NodeId) -> FxHashSet<NodeId> {
    crate::build::subtree_nodes(doc, &[root])
        .into_iter()
        .filter_map(|id| doc.get(id).and_then(|n| n.link))
        .collect()
}

/// `p`'s children in its source's order, where they are not — the linked
/// children taking the source's order in the slots they occupy, the instance's own
/// layers staying where they are (`propagate_structure`'s reorder rule).
fn order_target(doc: &Document, p: NodeId) -> Option<Vec<NodeId>> {
    let node = doc.get(p)?;
    let src = doc.get(source_of(doc, p)?)?;
    let theirs: FxHashSet<NodeId> = src.children.iter().copied().collect();
    let link = |k: &NodeId| doc.get(*k).and_then(|n| n.link);
    let slots: Vec<usize> = (0..node.children.len())
        .filter(|&i| link(&node.children[i]).is_some_and(|s| theirs.contains(&s)))
        .collect();
    let by_source: FxHashMap<NodeId, NodeId> = slots
        .iter()
        .map(|&i| (link(&node.children[i]).expect("filtered"), node.children[i]))
        .collect();
    let wanted: Vec<NodeId> = src
        .children
        .iter()
        .filter_map(|s| by_source.get(s).copied())
        .collect();
    let mut target = node.children.clone();
    for (slot, id) in slots.into_iter().zip(wanted) {
        target[slot] = id;
    }
    (target != node.children).then_some(target)
}

// ── The resets ─────────────────────────────────────────────────────────────────

/// *Reset fields*: every overridden field at or under each scope, written back to
/// its source's value. Local layers are untouched; a list becomes its source's
/// list, the copy's own items going with the rest (§15 D994).
pub fn reset_fields(doc: &Document, scopes: &[NodeId]) -> Vec<Operation> {
    scopes
        .iter()
        .flat_map(|s| scope_nodes(doc, *s))
        .flat_map(|n| overrides(doc, n))
        .map(|o| o.reset)
        .collect()
}

/// *Restore removed children*: each missing counterpart under each scope copied
/// back in from its source — fresh ids, each copied node linked to the one it was
/// copied from — at its anchor: after the counterpart of its preceding sibling,
/// else before the next one's, else topmost (§5.3d). A node of the removed
/// subtree whose counterpart the instance moved elsewhere is not copied again.
pub fn restore_children(doc: &Document, scopes: &[NodeId], ids: &mut IdSource) -> Vec<Operation> {
    let mut out = Vec::new();
    // Each parent's children as the inserts land, so two restored under one parent
    // are placed against each other.
    let mut sim: FxHashMap<NodeId, Vec<NodeId>> = FxHashMap::default();
    let mut new_links: FxHashMap<NodeId, NodeId> = FxHashMap::default();
    for scope in scopes {
        let nodes = scope_nodes(doc, *scope);
        for (p, c) in missing(doc, &nodes) {
            // `missing`'s owner, for its reason (`outermost_root`).
            let Some(owner) = outermost_root(doc, p) else {
                continue;
            };
            let Some(template) = doc.capture_subtree(c) else {
                continue;
            };
            let held = links_under(doc, owner);
            let template = crate::propagate::only_new(template, |t| held.contains(&t));
            if template.is_empty() {
                continue;
            }
            let Some((mut copy, _)) = crate::document::remap_subtree(&template, ids) else {
                continue;
            };
            for (k, t) in copy.iter_mut().zip(&template) {
                k.make_copy_of(t.id);
            }
            let Some(src_parent) = source_of(doc, p).and_then(|s| doc.get(s)) else {
                continue;
            };
            let kids = sim
                .get(&p)
                .cloned()
                .or_else(|| doc.get(p).map(|n| n.children.clone()))
                .unwrap_or_default();
            let link_of = |k: NodeId| {
                new_links
                    .get(&k)
                    .copied()
                    .or_else(|| doc.get(k).and_then(|n| n.link))
            };
            let index = crate::propagate::anchor(&src_parent.children, c, &kids, |s| {
                kids.iter().copied().find(|k| link_of(*k) == Some(s))
            });
            let index = index.min(kids.len());
            new_links.insert(copy[0].id, c);
            let mut placed = kids;
            placed.insert(index, copy[0].id);
            sim.insert(p, placed);
            out.push(Operation::InsertSubtree {
                nodes: copy,
                parent: p,
                index,
            });
        }
    }
    out
}

/// *Reset order*: each parent under each scope whose linked children are out of
/// their sources' order, put back in it, the instance's own layers keeping their
/// slots.
pub fn reset_order(doc: &Document, scopes: &[NodeId]) -> Vec<Operation> {
    let mut out = Vec::new();
    for scope in scopes {
        for p in scope_nodes(doc, *scope) {
            if let Some(target) = order_target(doc, p) {
                out.extend(
                    target
                        .into_iter()
                        .enumerate()
                        .map(|(index, id)| Operation::Reorder { id, index }),
                );
            }
        }
    }
    out
}

/// Which reset — the four §15 D981 names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// *Reset all*, or *Reset Label* on a child.
    All,
    /// *Reset fields N*.
    Fields,
    /// *Restore removed children N*.
    Children,
    /// *Reset order*.
    Order,
}

/// The operations the reset `kind` at each of `scopes` owes.
///
/// **The outermost of the scopes only**: a scope inside another is already
/// covered by it, and resetting both restored a missing child twice — the second
/// copy cut loose by `settle_links` and landing as an unlinked duplicate.
/// `overlapping_scopes_restore_a_child_once`; found by `arch-scribe`. The four
/// functions below take their scopes as given and are this one's parts.
pub fn reset(doc: &Document, kind: Kind, scopes: &[NodeId], ids: &mut IdSource) -> Vec<Operation> {
    let scopes = &crate::build::outermost(doc, scopes);
    match kind {
        Kind::All => reset_all(doc, scopes, ids),
        Kind::Fields => reset_fields(doc, scopes),
        Kind::Children => restore_children(doc, scopes, ids),
        Kind::Order => reset_order(doc, scopes),
    }
}

/// *Reset all*: the missing counterparts restored, then the order, then the
/// fields — each read on the document the one before leaves, so a restored child
/// is ordered with its siblings. One transaction's worth of operations.
pub fn reset_all(doc: &Document, scopes: &[NodeId], ids: &mut IdSource) -> Vec<Operation> {
    let mut scratch = doc.clone();
    let mut out = restore_children(doc, scopes, ids);
    if scratch.apply_unchecked(&Transaction(out.clone())).is_err() {
        return out; // the commit will refuse it, with the reason
    }
    let order = reset_order(&scratch, scopes);
    if scratch
        .apply_unchecked(&Transaction(order.clone()))
        .is_err()
    {
        out.extend(order);
        return out;
    }
    out.extend(order);
    out.extend(reset_fields(&scratch, scopes));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{Fill, Pivot, Stroke, TextSizing, TextStyle};
    use kurbo::{Affine, Size, Vec2};

    /// **`state_ops` writes a node whole**: two artboards and two texts are made
    /// to differ in every field cheap to set, and one node's state ops aimed at
    /// the other make them equal in everything but identity, place in the tree,
    /// locks and links. The destructure in `state_ops` is what forces a *new*
    /// field to be considered; this is what checks the existing ones are mapped to
    /// the operation that writes them. ⚠️ It does not set effects, exports, grids,
    /// insets, the layout item or the paragraph and block styles — each is one
    /// whole-value op `state_ops` copies straight out of its field, and
    /// `tests/reset.rs` reaches fills, names and text styles through `overrides`.
    /// Flip: `state_ops` writing an opacity of 1 fails the artboard pair.
    #[test]
    fn state_ops_write_a_node_whole() {
        let mut ids = IdSource::new(0xAD);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [a, b, ta, tb] = [(); 4].map(|_| ids.mint());
        let board = NodeKind::Artboard {
            size: Size::new(10.0, 10.0),
        };
        let text = |content: &str| NodeKind::Text {
            content: content.into(),
            style: Box::new(TextStyle {
                font_family: "Inter".into(),
                font_size: 12.0,
                ..Default::default()
            }),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: Default::default(),
            sizing: TextSizing::Auto,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        };
        let create = |id, parent, kind| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind,
            transform: None,
            name: None,
        };
        doc.apply(&Transaction(vec![
            create(a, root, board.clone()),
            create(b, root, board),
            create(ta, a, text("one")),
            create(tb, b, text("two")),
        ]))
        .expect("the fixture");
        let fill = |r| {
            crate::item::keyed_by_position([Fill {
                brush: crate::Brush::Solid(peniko::Color::from_rgba8(r, 0, 0, 255)),
                visible: true,
            }])
        };
        let clip = doc.get(a).unwrap().clip;
        doc.apply(&Transaction(vec![
            Operation::SetTransform {
                id: a,
                transform: Affine::translate((3.0, 4.0)),
            },
            Operation::SetName {
                id: a,
                name: "A".into(),
            },
            Operation::SetVisible {
                id: a,
                visible: false,
            },
            Operation::SetOpacity {
                id: a,
                opacity: 0.5,
            },
            Operation::SetClip { id: a, clip: !clip },
            Operation::SetFills {
                id: a,
                fills: fill(9),
            },
            Operation::SetStrokes {
                id: a,
                strokes: crate::item::keyed_by_position([Stroke::default()]),
            },
            Operation::SetPivot {
                id: a,
                pivot: Some(Pivot::Normalized(Vec2::new(0.2, 0.3))),
            },
            Operation::SetDisplay {
                id: a,
                display: Some(crate::container::Display::Flex(crate::container::Flex {
                    row_gap: 4.0,
                    ..Default::default()
                })),
            },
            Operation::SetGeometry {
                id: a,
                geometry: GeometryPatch::Size(Size::new(40.0, 30.0)),
            },
            Operation::SetTextStyle {
                id: ta,
                style: TextStyle {
                    font_family: "Mono".into(),
                    font_size: 20.0,
                    weight: 700,
                    italic: true,
                    ..Default::default()
                },
                spans: None,
            },
            Operation::SetGeometry {
                id: ta,
                geometry: GeometryPatch::TextSizing(TextSizing::AutoHeight(80.0)),
            },
            Operation::SetGeometry {
                id: ta,
                geometry: GeometryPatch::TextPathOffset(5.0),
            },
            Operation::SetGeometry {
                id: ta,
                geometry: GeometryPatch::TextPathFlip(true),
            },
        ]))
        .expect("a differs from b in everything set here");
        let ops: Vec<Operation> = state_ops(doc.get(a).unwrap())
            .into_iter()
            .filter_map(|op| op.retargeted(b))
            .chain(
                state_ops(doc.get(ta).unwrap())
                    .into_iter()
                    .filter_map(|op| op.retargeted(tb)),
            )
            .collect();
        doc.apply(&Transaction(ops)).expect("a state applies");
        for (from, to) in [(a, b), (ta, tb)] {
            let want = doc.get(from).unwrap();
            let mut got = doc.get(to).unwrap().clone();
            got.id = want.id;
            got.parent = want.parent;
            got.children = want.children.clone();
            assert_eq!(&got, want, "{to:?} written whole from {from:?}");
        }
    }
}
