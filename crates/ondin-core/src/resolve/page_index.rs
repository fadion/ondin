//! One page's spatial index: a bulk-loaded tree that is never inserted into, and
//! a tree for what was added since.

use super::BoundsEntry;
use rstar::{AABB, RTree};

/// The R-tree entries of one page.
///
/// **`rstar` cannot insert into a bulk-loaded tree.** `RTree::bulk_load` puts
/// leaves at uneven depths, and `insert` descends as if they were even: it
/// panics ("This is a bug in rstar") on the first insert whose path reaches a
/// leaf early. So the bulk-loaded tree only answers queries and gives up entries,
/// which `remove` does on any shape, and every insert goes into `added`, a tree
/// built by inserts alone. Once `added` outgrows [`Self::rebuild_at`], the two are
/// bulk-loaded into one again.
#[derive(Clone, Default)]
pub(super) struct PageIndex {
    bulk: RTree<BoundsEntry>,
    added: RTree<BoundsEntry>,
}

impl PageIndex {
    pub(super) fn bulk_load(entries: Vec<BoundsEntry>) -> Self {
        Self {
            bulk: RTree::bulk_load(entries),
            added: RTree::new(),
        }
    }

    pub(super) fn insert(&mut self, e: BoundsEntry) {
        self.added.insert(e);
        if self.added.size() > Self::rebuild_at(self.bulk.size()) {
            let all: Vec<BoundsEntry> = self.iter().cloned().collect();
            *self = Self::bulk_load(all);
        }
    }

    /// Takes out the entry equal to `e`, wherever it is.
    pub(super) fn remove(&mut self, e: &BoundsEntry) -> Option<BoundsEntry> {
        self.bulk.remove(e).or_else(|| self.added.remove(e))
    }

    pub(super) fn locate_in_envelope_intersecting(
        &self,
        env: AABB<[f64; 2]>,
    ) -> impl Iterator<Item = &BoundsEntry> {
        self.bulk
            .locate_in_envelope_intersecting(env)
            .chain(self.added.locate_in_envelope_intersecting(env))
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &BoundsEntry> {
        self.bulk.iter().chain(self.added.iter())
    }

    /// How many inserts `added` takes before the page is bulk-loaded again: a
    /// share of the bulk tree, so a rebuild's cost is spread over that many
    /// inserts, and never so few that a small page rebuilds on every commit.
    fn rebuild_at(bulk: usize) -> usize {
        (bulk / 4).max(256)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::NodeId;
    use rustc_hash::FxHashSet;

    fn e(seq: u64, x: f64, y: f64) -> BoundsEntry {
        BoundsEntry {
            id: NodeId { actor: 1, seq },
            min: [x, y],
            max: [x + 5.0, y + 5.0],
        }
    }

    /// Entries scattered so the bulk loader's slabs come out uneven.
    fn scattered(n: u64, from: u64) -> Vec<BoundsEntry> {
        (from..from + n)
            .map(|i| e(i, (i * 7919 % 10_007) as f64, (i * 104_729 % 10_009) as f64))
            .collect()
    }

    fn ids(index: &PageIndex) -> FxHashSet<u64> {
        index.iter().map(|e| e.id.seq).collect()
    }

    /// The premise: `rstar` bulk-loads trees whose leaves sit at different
    /// depths, which is what makes inserting into one unsafe. If an `rstar`
    /// upgrade stops doing this, the split in `PageIndex` is no longer needed.
    #[test]
    fn a_bulk_loaded_tree_can_have_leaves_at_uneven_depths() {
        fn depths(n: &rstar::ParentNode<BoundsEntry>, d: usize, out: &mut FxHashSet<usize>) {
            for c in n.children() {
                match c {
                    rstar::RTreeNode::Leaf(_) => {
                        out.insert(d);
                    }
                    rstar::RTreeNode::Parent(p) => depths(p, d + 1, out),
                }
            }
        }
        let tree = RTree::bulk_load(scattered(100, 0));
        let mut d = FxHashSet::default();
        depths(tree.root(), 0, &mut d);
        assert!(d.len() > 1, "leaf depths {d:?}");
    }

    /// Inserting into bulk-loaded pages of many sizes, with removals between,
    /// never panics and keeps exactly the entries put in; queries see both trees.
    ///
    /// flip: `PageIndex::insert` inserting into `bulk` → panics inside `rstar`
    /// ("This is a bug in rstar").
    #[test]
    fn inserts_after_a_bulk_load_hold_every_entry() {
        for n in [1, 17, 100, 1_000, 5_000] {
            let mut index = PageIndex::bulk_load(scattered(n, 0));
            let mut want: FxHashSet<u64> = (0..n).collect();
            for (k, new) in scattered(n.min(600), 1_000_000).into_iter().enumerate() {
                if k % 3 == 0 {
                    let old = e(k as u64, 0.0, 0.0);
                    let old = index.iter().find(|x| x.id == old.id).cloned();
                    if let Some(old) = old {
                        assert!(index.remove(&old).is_some());
                        want.remove(&old.id.seq);
                    }
                }
                want.insert(new.id.seq);
                index.insert(new);
            }
            assert_eq!(ids(&index), want, "{n}: the entries put in");
            let everywhere = AABB::from_corners([-1e9, -1e9], [1e9, 1e9]);
            let found: FxHashSet<u64> = index
                .locate_in_envelope_intersecting(everywhere)
                .map(|e| e.id.seq)
                .collect();
            assert_eq!(found, want, "{n}: a query sees both trees");
        }
    }

    /// An entry inserted since the bulk load can be removed again.
    #[test]
    fn an_added_entry_can_be_removed() {
        let mut index = PageIndex::bulk_load(scattered(50, 0));
        let new = e(999, 3.0, 3.0);
        index.insert(new.clone());
        assert_eq!(index.remove(&new), Some(new));
        assert!(!ids(&index).contains(&999));
    }
}
