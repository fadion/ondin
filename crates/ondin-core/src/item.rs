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
/// `p`; an item the anchor never had (a row the user added, which its caller
/// minted) **keeps its own id on every target**, so one edit adding a row to
/// several layers adds one item to all of them.
///
/// **One shared id, not one per target** (§15 D1003 (6), `[X1-L1-01]`). This
/// minted per target until the ruling, and over a main and its instance selected
/// together that cut the instance's new row from the main's at birth: a local
/// addition with its own id, a ghost row for the main's, and every later edit of
/// the main's row skipping the copy (`propagate` stands down on a field the
/// user's own transaction wrote). An id shared across nodes is how D980 spells a
/// match, and between unlinked layers it costs nothing, since an id is unique
/// within one list only. The one place an un-anchored id is **not** kept is a
/// target already holding it, where it is minted after all — keeping it there
/// would repeat an id in that list, which `apply` refuses.
///
/// Callers pass lists that agree in value position by position; where they do not
/// (a shorter target), an anchored item with no counterpart at its position keeps
/// its id by the same rule as an added one.
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
/// anything past `own`'s end the id `fresh` holds for `p` — minted the first time
/// any target of the write reaches that position, and the same for every target
/// after it (§15 D1003 (6), `[X1-L1-01]`: a paste onto a main and its instance
/// together gives each pasted item past the end one id on both, so the copy
/// follows the main's later edit of it). Pass one [`PastTheEnd`] per write, never
/// one per target.
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
/// target's end, **one per position for the whole write**, shared by every target
/// that reaches it (§15 D1003 (6)).
///
/// Unique within each list all the same: a target keeps its own ids below its
/// end and takes these only at and past it, and each of these was minted fresh,
/// so it is in no list yet.
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
/// property list a build before `[R1-L2-02]`'s fix saved with one id twice
/// (*Combine as variants* made them). Refusing such a file would lose the
/// document over a defect the app itself wrote.
///
/// **A function of the list alone**, never a minted id: the loader must give the
/// same document for the same bytes (invariant 9; `io::schema`'s note on the
/// document id it does not mint). A positional id is actor 0's, which
/// [`crate::reserve_existing_ids`] sweeps past for any session that is.
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

    /// The multi-selection write keeps each target's ids: an edited value lands on
    /// the target's own item, a deleted one drops it, and an added one **keeps the
    /// id its caller minted**, on this target as on every other (§15 D1003 (6),
    /// `[X1-L1-01]` — it was minted fresh per target until the ruling, and this
    /// test asserted that). A target that already holds the added id mints instead,
    /// or its list would repeat one.
    ///
    /// Flips, run: writing `edited` verbatim (the anchor's ids) fails the first
    /// assertion; minting for every un-anchored item (the rule before) fails *"an
    /// added item keeps its id"*; keeping the id even where the target holds it
    /// fails *"minted where the target holds it"*.
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

    /// A wholesale write gives the positions past each target's end **one id per
    /// position across the write** (§15 D1003 (6)): a one-item and a two-item
    /// target written three values share the id at position 2, and the one-item
    /// target's position 1 is fresh, not the other's own item. Flip, run:
    /// `PastTheEnd::at` minting on every call fails *"one id at position 2"*.
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

    /// The loader's repair (`[R1-L2-02]`): each repeat after the first takes the
    /// lowest positional id the list does not already hold — **anywhere** in it,
    /// so `0:0`, carried by an item *after* the repeats, is skipped and kept by
    /// its own item. The first of the repeats keeps its id.
    ///
    /// Flip, run: the `held` test dropped hands the first repeat `0:0` and
    /// re-keys the later item that owned it, *its* id changing for no reason —
    /// the equality fails with `[a, 0:0, 0:1, 0:2]`. (A first version of this
    /// test put the positional item *before* the repeats, where `seen` already
    /// covers it, and the same flip stayed green.)
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
