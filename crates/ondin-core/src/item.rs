//! Identity for the items in a layer's lists (§15 D980).
//!
//! Five lists on a node are edited as items — `Paint::fills`, `Paint::strokes`,
//! `Node::effects`, `Node::exports` and `Node::grids` — and each item carries an
//! [`ItemId`] beside it in a [`Keyed`] wrapper. The id is what lets an instance's
//! fill follow its main component's fill **item by item** once an instance can add,
//! delete and reorder items of its own (§5.3d): position cannot say which item is
//! which after any of those.
//!
//! 🚨 **The id lives beside the item, never inside it.** `Fill`, `Stroke`, `Effect`,
//! `ExportSpec` and `LayoutGrid` all derive `PartialEq`, and every *same look*
//! comparison in the workspace — the inspector's *Mixed*, `changes_nothing`,
//! `overwrites` and run merging — compares them. An id field inside the item would
//! make two identical shadows unequal, silently. With the wrapper, `a.value == b.value`
//! asks *same look* and `a == b` asks *same item, same look*; the five whole-list
//! *Mixed* readings compare through [`same_values`]. Do not "simplify" the wrapper
//! into a field.
//!
//! ⚠️ **There is deliberately no `From<Vec<T>>`.** The tempting edit shape — clone the
//! values out, change one, collect them back in — would re-key every item by
//! position and quietly cut each one from its counterpart in the main. Every site
//! that builds a keyed list says how: it keeps the ids it read ([`Keyed::map`]), it
//! mints a fresh one for an item it adds ([`IdSource::mint_item`]), or it builds a
//! list for a node that has never had one ([`keyed_by_position`]).

use crate::id::{IdSource, NodeId};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The identity of one item in one of a node's five item lists.
///
/// A newtype over [`NodeId`], as `GuideId` is, minted from the same session
/// stream by the caller (invariant 3 — `apply` never allocates). **Unique within
/// its list, not across nodes**: duplicating a node copies its item ids verbatim,
/// and that is the match — a copy's fill is its source's fill exactly when their
/// ids agree. Minted from the global stream rather than a per-list counter so an
/// item added locally to an instance and one added later to its main can never
/// collide.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub NodeId);

impl ItemId {
    /// The id `migrate_4_to_5` and [`keyed_by_position`] give the item at `index`:
    /// actor 0, seq = index. Deterministic, so a migrated document saves to the
    /// same bytes twice (invariant 9), and unique within the list it numbers.
    ///
    /// ⚠️ **Actor 0 is improbable for a live session, not impossible** —
    /// `session::random_actor` is an unguarded hash. What keeps a minted id off a
    /// positional one is [`crate::reserve_existing_ids`] sweeping item ids too, so a
    /// session whose actor happens to be 0 starts its seqs past every one in use.
    pub fn positional(index: usize) -> ItemId {
        ItemId(NodeId {
            actor: 0,
            seq: index as u64,
        })
    }
}

impl std::fmt::Debug for ItemId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

/// Written as the node id's wire form, `"<actor-hex>:<seq>"`, so a saved item id
/// reads like every other id in the file.
impl Serialize for ItemId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_wire())
    }
}

impl<'de> Deserialize<'de> for ItemId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        NodeId::from_wire(&s)
            .map(ItemId)
            .ok_or_else(|| serde::de::Error::custom(format!("bad item id {s:?}")))
    }
}

impl IdSource {
    /// A fresh [`ItemId`] for an item being **added** to an existing list.
    pub fn mint_item(&mut self) -> ItemId {
        ItemId(self.mint())
    }
}

/// One item of a keyed list: its identity and its value.
///
/// Derefs to the value, so reading a field (`fill.brush`, `effect.visible`) is
/// unchanged; building one is not, which is the point — see the module doc.
/// Saved flattened: `{"id":"0:0","brush":…,"visible":true}`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyed<T> {
    pub id: ItemId,
    #[serde(flatten)]
    pub value: T,
}

impl<T> Keyed<T> {
    pub fn new(id: ItemId, value: T) -> Self {
        Self { id, value }
    }

    /// The same item with a new value — the shape of every in-place edit.
    pub fn map(&self, f: impl FnOnce(&T) -> T) -> Self {
        Self {
            id: self.id,
            value: f(&self.value),
        }
    }
}

impl<T> std::ops::Deref for Keyed<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T> std::ops::DerefMut for Keyed<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

/// A list for a node that has never had one, keyed by position
/// ([`ItemId::positional`]).
///
/// **Only for a list built whole**: a new node, an import, a migration, a test.
/// It is safe there because the ids are unique within the list and any item added
/// later is minted. It is **not** safe for an item appended to an existing list —
/// an instance that deleted its main's second fill and then added one positionally
/// would take the deleted fill's id and be matched to it.
pub fn keyed_by_position<T>(items: impl IntoIterator<Item = T>) -> Vec<Keyed<T>> {
    items
        .into_iter()
        .enumerate()
        .map(|(i, value)| Keyed::new(ItemId::positional(i), value))
        .collect()
}

/// Whether two keyed lists **look** the same: equal values in the same order,
/// ids ignored. The comparison every *Mixed* reading means.
pub fn same_values<T: PartialEq>(a: &[Keyed<T>], b: &[Keyed<T>]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.value == y.value)
}

/// The values of a keyed list, cloned out. For readers that only ever look.
pub fn values<T: Clone>(items: &[Keyed<T>]) -> Vec<T> {
    items.iter().map(|k| k.value.clone()).collect()
}

/// `edited` — a list that started as `anchor` and was changed — written onto
/// `target`, a list that **looks** like `anchor`, keeping `target`'s own ids.
///
/// The multi-selection write: the inspector shows one list for several layers whose
/// lists agree in value, the user edits it, and each layer receives the edit. Writing
/// `edited` verbatim would give every layer the anchor's ids, and on an instance's
/// child that cuts each item from its counterpart in the main. So an item of
/// `edited` whose id is the anchor's item at position `p` takes the target's id at
/// `p`; an item the anchor never had (a row the user added) is minted fresh **per
/// target**, so two layers never share a newly added id.
///
/// Callers pass lists that agree in value position by position; where they do not
/// (a longer target), positions past the anchor's end are simply not reachable.
///
/// ⚠️ **Ruled otherwise for a main and its instance, not built** (§15 D1003 (6)): a
/// row one edit adds to both takes one shared id, so the instance follows its main's
/// new item. [`rekey_by_position`]'s past-the-end ids are the same case.
pub fn retarget<T: Clone>(
    anchor: &[Keyed<T>],
    target: &[Keyed<T>],
    edited: &[Keyed<T>],
    ids: &mut IdSource,
) -> Vec<Keyed<T>> {
    edited
        .iter()
        .map(|item| {
            let id = anchor
                .iter()
                .position(|a| a.id == item.id)
                .and_then(|p| target.get(p))
                .map(|t| t.id)
                .unwrap_or_else(|| ids.mint_item());
            Keyed::new(id, item.value.clone())
        })
        .collect()
}

/// `values` written over `own` **wholesale**, item `p` taking `own`'s id at `p` and
/// anything past `own`'s end minted fresh.
///
/// For a replacement that is not an edit of the list it lands on — *Paste
/// properties*, a panel clearing every fill — where there is no anchor to say which
/// item became which, and keeping the ids by position is what lets an instance's
/// child read the paste as overrides of its main's items rather than as every item
/// deleted and new local ones added. ⚠️ **Not for an edit**: deleting the first of
/// two items shifts the second into position 0, and this would hand it the deleted
/// item's id. An edit goes through [`retarget`].
pub fn rekey_by_position<T>(
    own: &[Keyed<T>],
    values: impl IntoIterator<Item = T>,
    ids: &mut IdSource,
) -> Vec<Keyed<T>> {
    values
        .into_iter()
        .enumerate()
        .map(|(p, v)| {
            let id = own.get(p).map(|k| k.id).unwrap_or_else(|| ids.mint_item());
            Keyed::new(id, v)
        })
        .collect()
}

/// The first id that appears twice in `items`, if any — `apply`'s post-condition
/// and the loader's check that an item id is unique within its list.
pub fn first_duplicate<T>(items: &[Keyed<T>]) -> Option<ItemId> {
    let mut seen = rustc_hash::FxHashSet::default();
    items.iter().map(|k| k.id).find(|id| !seen.insert(*id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_item_id_saves_as_a_wire_string_and_reads_back() {
        let id = ItemId(NodeId {
            actor: 0x1a2b,
            seq: 7,
        });
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"1a2b:7\"");
        assert_eq!(serde_json::from_str::<ItemId>(&json).unwrap(), id);
    }

    #[test]
    fn a_keyed_item_saves_flattened_beside_its_id() {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        struct V {
            a: u32,
        }
        let k = Keyed::new(ItemId::positional(2), V { a: 5 });
        let json = serde_json::to_string(&k).unwrap();
        assert_eq!(json, r#"{"id":"0:2","a":5}"#);
        assert_eq!(serde_json::from_str::<Keyed<V>>(&json).unwrap(), k);
    }

    #[test]
    fn same_values_ignores_ids_and_equality_does_not() {
        let a = keyed_by_position([1, 2]);
        let mut ids = IdSource::new(0xAB);
        let b: Vec<_> = [1, 2].map(|v| Keyed::new(ids.mint_item(), v)).into();
        assert!(same_values(&a, &b));
        assert_ne!(a, b, "the ids differ, so the items are not the same items");
        assert!(!same_values(&a, &keyed_by_position([2, 1])));
        assert!(!same_values(&a, &keyed_by_position([1])));
    }

    /// The multi-selection write keeps each target's ids: an edited value lands on
    /// the target's own item, a deleted one drops it, an added one is minted fresh.
    /// Flip: writing `edited` verbatim (the anchor's ids) fails the first assertion.
    #[test]
    fn retarget_keeps_the_targets_ids_and_mints_for_an_added_item() {
        let anchor = keyed_by_position(['a', 'b', 'c']);
        let mut ids = IdSource::new(0xAB);
        let target: Vec<_> = ['a', 'b', 'c']
            .map(|v| Keyed::new(ids.mint_item(), v))
            .into();
        // Edit `a`, delete `b`, keep `c`, add `d`.
        let added = Keyed::new(ids.mint_item(), 'd');
        let edited = vec![anchor[0].map(|_| 'A'), anchor[2], added];
        let out = retarget(&anchor, &target, &edited, &mut ids);
        assert_eq!(out[0].id, target[0].id);
        assert_eq!(out[0].value, 'A');
        assert_eq!(out[1].id, target[2].id);
        assert_eq!(out.len(), 3);
        assert!(
            target.iter().chain(&anchor).all(|t| t.id != out[2].id) && out[2].id != added.id,
            "an added item is minted fresh for this target"
        );
    }

    #[test]
    fn first_duplicate_finds_a_repeated_id() {
        let mut l = keyed_by_position([1, 2, 3]);
        assert_eq!(first_duplicate(&l), None);
        l[2].id = l[0].id;
        assert_eq!(first_duplicate(&l), Some(l[0].id));
    }
}
