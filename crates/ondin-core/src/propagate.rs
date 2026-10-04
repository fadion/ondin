//! Keeping instances in step with their mains (§5.3d build step 3, §15 D978–D980).
//!
//! An instance is a linked copy, so a main's edit reaches its copies as more
//! operations in the same transaction — written here, at the commit, by
//! [`propagate`], which `EditorSession::commit_inner` runs after its other
//! passes. Nothing is recorded about which values a copy overrides
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
fn is_placement(op: &Operation) -> bool {
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
pub fn propagate(doc: &Document, tx: &Transaction) -> Vec<Operation> {
    let nodes = doc.node_map();
    let mut copies: FxHashMap<NodeId, Vec<NodeId>> = FxHashMap::default();
    for n in nodes.values() {
        if let Some(src) = n.link {
            copies.entry(src).or_default().push(n.id);
        }
    }
    if copies.is_empty() {
        return Vec::new();
    }
    for list in copies.values_mut() {
        list.sort();
    }
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

    let mut scratch = doc.clone();
    let mut out = Vec::new();
    for (_, op) in last {
        let Some(n) = op.overwrites() else { continue };
        let Some(old) = scratch.peek(op) else {
            continue;
        };
        let mut queue = vec![(n, op.clone(), old)];
        while let Some((src, new, old)) = queue.pop() {
            for &m in copies.get(&src).into_iter().flatten() {
                let Some(probe) = new.retargeted(m) else {
                    continue;
                };
                if probe.shape_key().is_some_and(|k| user.contains(&k)) {
                    continue;
                }
                if is_placement(op) && is_instance_root(doc, m) {
                    continue;
                }
                if writes_spans(op) && !same_content(doc, src, m) {
                    continue;
                }
                let Some(cur) = scratch.peek(&probe) else {
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

fn is_instance_root(doc: &Document, id: NodeId) -> bool {
    crate::component::instance_root(doc, id) == Some(id)
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
fn follow(new: &Operation, old: &Operation, cur: &Operation, m: NodeId) -> Option<Operation> {
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
    if old == new {
        return cur.clone();
    }
    match (old, new, cur) {
        (Value::Object(o), Value::Object(n), Value::Object(c))
            if o.keys().eq(n.keys()) && o.keys().eq(c.keys()) =>
        {
            let mut out = c.clone();
            for (k, ov) in o {
                out.insert(k.clone(), merge(ov, &n[k], &c[k]));
            }
            Value::Object(out)
        }
        _ if old == cur => new.clone(),
        _ => cur.clone(),
    }
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
    // Reorder of the shared items, if the copy still has the main's old order.
    let shared: Vec<ItemId> = old
        .iter()
        .map(|k| k.id)
        .filter(|id| find(new, *id).is_some())
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
}
