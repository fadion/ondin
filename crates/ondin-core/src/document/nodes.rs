//! The node table, which journals what a transaction overwrites so that a refused
//! one is put back exactly (`Document::apply`).

use crate::id::NodeId;
use crate::node::Node;
use rustc_hash::FxHashMap;

/// A document's nodes by id.
///
/// Reads go through `Deref`; every write goes through [`Self::get_mut`],
/// [`Self::insert`] or [`Self::remove`], which record a node's value before its
/// first write while a journal is open. **There is no `DerefMut`**: a write that
/// bypassed the journal would survive a refused transaction's rollback.
#[derive(Default)]
pub(super) struct NodeTable {
    map: FxHashMap<NodeId, Node>,
    /// Each node written since [`Self::open_journal`], with its value before the
    /// first write (`None`: it did not exist). `Some` only inside an apply.
    journal: Option<FxHashMap<NodeId, Option<Node>>>,
}

impl NodeTable {
    pub(super) fn new(map: FxHashMap<NodeId, Node>) -> Self {
        Self { map, journal: None }
    }

    pub(super) fn get_mut(&mut self, id: &NodeId) -> Option<&mut Node> {
        self.note(*id);
        self.map.get_mut(id)
    }

    pub(super) fn insert(&mut self, id: NodeId, node: Node) -> Option<Node> {
        self.note(id);
        self.map.insert(id, node)
    }

    pub(super) fn remove(&mut self, id: &NodeId) -> Option<Node> {
        self.note(*id);
        self.map.remove(id)
    }

    /// Record `id`'s current value, unless this journal already holds an older one.
    fn note(&mut self, id: NodeId) {
        let Self { map, journal } = self;
        if let Some(journal) = journal {
            journal.entry(id).or_insert_with(|| map.get(&id).cloned());
        }
    }

    /// Start recording. A journal already open means an apply inside an apply,
    /// whose rollback would put back the outer one's writes as well.
    pub(super) fn open_journal(&mut self) {
        assert!(self.journal.is_none(), "a node journal is already open");
        self.journal = Some(FxHashMap::default());
    }

    /// Keep every write since [`Self::open_journal`].
    pub(super) fn commit(&mut self) {
        self.journal = None;
    }

    /// Put every node written since [`Self::open_journal`] back as it was:
    /// re-inserted, restored, or removed if it did not exist.
    pub(super) fn roll_back(&mut self) {
        for (id, old) in self.journal.take().into_iter().flatten() {
            match old {
                Some(node) => {
                    self.map.insert(id, node);
                }
                None => {
                    self.map.remove(&id);
                }
            }
        }
    }
}

impl std::ops::Deref for NodeTable {
    type Target = FxHashMap<NodeId, Node>;

    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

/// The nodes alone: a copy starts with no journal open.
impl Clone for NodeTable {
    fn clone(&self) -> Self {
        Self::new(self.map.clone())
    }
}

/// The nodes alone; the journal is not part of the document.
impl PartialEq for NodeTable {
    fn eq(&self, other: &Self) -> bool {
        self.map == other.map
    }
}

impl std::fmt::Debug for NodeTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.map.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::NodeKind;

    fn nid(seq: u64) -> NodeId {
        NodeId { actor: 1, seq }
    }

    fn node(id: u64, name: &str) -> Node {
        let mut n = crate::document::root_node(nid(id), name.to_owned());
        n.kind = NodeKind::Group;
        n
    }

    /// A rollback restores a written node, re-inserts a removed one and removes an
    /// inserted one, each to its value before its first write.
    #[test]
    fn roll_back_restores_each_node_to_its_first_recorded_value() {
        let mut table = NodeTable::new(
            [(nid(1), node(1, "a")), (nid(2), node(2, "b"))]
                .into_iter()
                .collect(),
        );
        let before = table.clone();
        table.open_journal();
        table.get_mut(&nid(1)).unwrap().name = "a1".into();
        table.get_mut(&nid(1)).unwrap().name = "a2".into();
        table.remove(&nid(2));
        table.insert(nid(3), node(3, "c"));
        table.insert(nid(2), node(2, "b2"));
        assert_ne!(table, before, "the fixture wrote nothing");
        table.roll_back();
        assert_eq!(table, before);
        assert!(table.journal.is_none());
    }

    /// Writes made with no journal open, and after a commit, are kept.
    #[test]
    fn commit_and_unjournaled_writes_are_kept() {
        let mut table = NodeTable::new([(nid(1), node(1, "a"))].into_iter().collect());
        table.get_mut(&nid(1)).unwrap().name = "b".into();
        table.open_journal();
        table.insert(nid(2), node(2, "c"));
        table.commit();
        table.roll_back();
        assert_eq!(table[&nid(1)].name, "b");
        assert!(table.contains_key(&nid(2)));
    }

    /// A second journal over an open one is refused rather than merged.
    #[test]
    #[should_panic(expected = "already open")]
    fn a_nested_journal_panics() {
        let mut table = NodeTable::default();
        table.open_journal();
        table.open_journal();
    }
}
