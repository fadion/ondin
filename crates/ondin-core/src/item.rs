//! Identity for the items in a layer's lists (§15 D980).
//!
//! Five lists on a node (`Paint::fills`, `Paint::strokes`, `Node::effects`,
//! `Node::exports`, `Node::grids`) are edited as items, each carrying an [`ItemId`]
//! in a [`Keyed`] wrapper, so an instance's fill follows its main's item by item
//! once it can add, delete and reorder its own (§5.3d).
//!
//! The id lives beside the item, never inside it: the item types derive `PartialEq`
//! and every "same look" comparison uses it, which an id field would silently break.
//! `a.value == b.value` asks same look, `a == b` same item and look; whole-list
//! comparisons go through [`same_values`]. Do not "simplify" the wrapper into a field.
//!
//! There is deliberately no `From<Vec<T>>`: clone-edit-collect would re-key every
//! item by position and cut it from its counterpart in the main. A site that builds
//! a keyed list keeps the ids it read ([`Keyed::map`]), mints one for an added item
//! ([`IdSource::mint_item`]), or numbers a list that never had ids
//! ([`keyed_by_position`]).

use crate::id::{IdSource, NodeId};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The identity of one item in one of a node's five item lists.
///
/// A newtype over [`NodeId`], minted by the caller from the session stream
/// (invariant 3 — `apply` never allocates). Unique within its list, not across
/// nodes: duplicating a node copies its item ids, and equal ids are the match. The
/// global stream, not a per-list counter, keeps an instance's local item from
/// colliding with one later added to its main.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub NodeId);

impl ItemId {
    /// The id `migrate_4_to_5` and [`keyed_by_position`] give the item at `index`:
    /// actor 0, seq = index. Deterministic, so a migrated document saves to the
    /// same bytes twice (invariant 9), and unique within the list it numbers.
    ///
    /// A live session can in principle draw actor 0 (`session::random_actor` is an
    /// unguarded hash); [`crate::reserve_existing_ids`] sweeps item ids too, so
    /// such a session starts its seqs past every one in use.
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
/// Only for a list built whole (a new node, import, migration, test). Not for an
/// item appended to an existing list: an instance that deleted its main's second
/// fill and added one positionally would take the deleted fill's id.
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
/// lists agree in value, and each layer receives the edit. Writing `edited` verbatim
/// would give every layer the anchor's ids and cut an instance's items from its
/// main's. So an item whose id is the anchor's item at position `p` takes the
/// target's id at `p`; an item the anchor never had (an added row, minted by the
/// caller) keeps its own id on every target.
///
/// One shared id, not one per target (§15 D1003 (6)): minting per target cut an
/// instance's new row from its main's when both were selected. An id shared across
/// unlinked nodes costs nothing, since ids are unique within one list only. A
/// target already holding the id mints instead, or `apply` would refuse the repeat.
///
/// Callers pass lists that agree in value position by position; an anchored item
/// with no counterpart (a shorter target) keeps its id by the same rule as an
/// added one.
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
                .unwrap_or_else(|| match target.iter().any(|t| t.id == item.id) {
                    true => ids.mint_item(),
                    false => item.id,
                });
            Keyed::new(id, item.value.clone())
        })
        .collect()
}

/// `values` written over `own` **wholesale**, item `p` taking `own`'s id at `p` and
/// anything past `own`'s end the id `fresh` holds for `p`, minted the first time any
/// target reaches that position and shared by every later target (§15 D1003 (6)).
/// Pass one [`PastTheEnd`] per write, never one per target.
///
/// For a replacement that is not an edit (*Paste properties*, clearing every fill),
/// where no anchor says which item became which; keeping ids by position lets an
/// instance read the paste as overrides of its main's items. Not for an edit:
/// deleting the first of two items would hand the second the deleted item's id.
/// An edit goes through [`retarget`].
pub fn rekey_by_position<T>(
    own: &[Keyed<T>],
    values: impl IntoIterator<Item = T>,
    fresh: &mut PastTheEnd,
    ids: &mut IdSource,
) -> Vec<Keyed<T>> {
    values
        .into_iter()
        .enumerate()
        .map(|(p, v)| {
            let id = own.get(p).map(|k| k.id).unwrap_or_else(|| fresh.at(p, ids));
            Keyed::new(id, v)
        })
        .collect()
}

/// The ids one wholesale write ([`rekey_by_position`]) gives the positions past a
/// target's end, one per position for the whole write, shared by every target that
/// reaches it. Still unique within each list: each is minted fresh and taken only
/// at and past a target's end.
#[derive(Debug, Default)]
pub struct PastTheEnd(Vec<Option<ItemId>>);

impl PastTheEnd {
    /// The id for position `p`, minted the first time any target asks for it.
    fn at(&mut self, p: usize, ids: &mut IdSource) -> ItemId {
        if self.0.len() <= p {
            self.0.resize(p + 1, None);
        }
        *self.0[p].get_or_insert_with(|| ids.mint_item())
    }
}

/// `items` with every repeat of an id after its first given the lowest positional
/// id ([`ItemId::positional`]) the list does not hold — the loader's repair for a
/// property list saved with one id twice by an older build. Refusing the file would
/// lose a document over a defect the app wrote.
///
/// A function of the list alone, never a minted id: the same bytes must load to the
/// same document (invariant 9).
pub(crate) fn rekey_repeats<T>(mut items: Vec<Keyed<T>>) -> Vec<Keyed<T>> {
    let held: rustc_hash::FxHashSet<ItemId> = items.iter().map(|k| k.id).collect();
    let mut seen = rustc_hash::FxHashSet::default();
    let mut next = 0;
    for k in &mut items {
        if seen.insert(k.id) {
            continue;
        }
        while held.contains(&ItemId::positional(next)) || seen.contains(&ItemId::positional(next)) {
            next += 1;
        }
        k.id = ItemId::positional(next);
        seen.insert(k.id);
    }
    items
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

    /// The multi-selection write keeps each target's ids, and an added item keeps
    /// the id its caller minted unless the target already holds it.
    ///
    /// flip: writing `edited` verbatim → fails at the first id assertion; minting
    /// for every un-anchored item → fails at "an added item keeps its id"; keeping
    /// the id where the target holds it → fails at "minted where the target holds it".
    #[test]
    fn retarget_keeps_the_targets_ids_and_an_added_items_own() {
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
        assert_eq!(out[2].id, added.id, "an added item keeps its id");
        // A target already holding that id — the added row given one of its own.
        let holder = vec![target[0], target[1], Keyed::new(added.id, 'z')];
        let out = retarget(&anchor, &holder, &edited, &mut ids);
        assert!(
            holder.iter().all(|t| t.id != out[2].id),
            "minted where the target holds it"
        );
        assert_eq!(first_duplicate(&out), None);
    }

    /// A wholesale write gives positions past each target's end one id per position
    /// across the write. flip: `PastTheEnd::at` minting on every call → fails at
    /// "one id at position 2".
    #[test]
    fn a_wholesale_write_shares_the_ids_past_the_end() {
        let mut ids = IdSource::new(0xAB);
        let short: Vec<_> = ['a'].map(|v| Keyed::new(ids.mint_item(), v)).into();
        let long: Vec<_> = ['a', 'b'].map(|v| Keyed::new(ids.mint_item(), v)).into();
        let mut fresh = PastTheEnd::default();
        let s = rekey_by_position(&short, ['x', 'y', 'z'], &mut fresh, &mut ids);
        let l = rekey_by_position(&long, ['x', 'y', 'z'], &mut fresh, &mut ids);
        assert_eq!(
            (s[0].id, l[0].id, l[1].id),
            (short[0].id, long[0].id, long[1].id)
        );
        assert_eq!(s[2].id, l[2].id, "one id at position 2");
        assert_ne!(
            s[1].id, long[1].id,
            "the other target's own item is not shared"
        );
        assert_eq!((first_duplicate(&s), first_duplicate(&l)), (None, None));
    }

    /// Each repeat takes the lowest positional id the list does not hold anywhere,
    /// so `0:0`, carried by an item after the repeats, stays with its owner.
    ///
    /// flip: dropping the `held` test in `rekey_repeats` → fails at the equality
    /// (`[a, 0:0, 0:1, 0:2]`). The positional item must follow the repeats, or
    /// `seen` covers it and the flip stays green.
    #[test]
    fn rekey_repeats_takes_the_lowest_free_positional_id() {
        let mut ids = IdSource::new(0xAB);
        let a = ids.mint_item();
        let list = vec![
            Keyed::new(a, 1),
            Keyed::new(a, 2),
            Keyed::new(a, 3),
            Keyed::new(ItemId::positional(0), 4),
        ];
        let got: Vec<ItemId> = rekey_repeats(list).iter().map(|k| k.id).collect();
        assert_eq!(
            got,
            [
                a,
                ItemId::positional(1),
                ItemId::positional(2),
                ItemId::positional(0)
            ]
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
