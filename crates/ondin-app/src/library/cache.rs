//! The half of the library that does not travel: what this machine knows about
//! documents it has opened (§15 D366).
//!
//! The dividing line is "would you expect this on your other laptop". The name,
//! project and creation date would, so they live in the document
//! (`ondin_core::meta`); *last opened* would not, since "recent" means recent here.
//!
//! It lives beside the font cache in [`crate::machine_dir::root`]
//! (`dirs::cache_dir()/ondin`; `OndinData` on Windows, §15 D969).
//!
//! Local is not disposable: deleting it loses the Recent order, which refills,
//! and every star on the machine, which nothing else records. So the read reports
//! an unreadable file ([`LocalIndex::unreadable`]) instead of treating it as
//! "no file".
//!
//! Keyed on the document id, not the path, which goes stale on rename.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Per-machine facts about the library, keyed by document id.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalIndex {
    /// When each document was last opened here, seconds since the Unix epoch.
    pub opened: HashMap<String, u64>,
    /// The documents starred here, by id.
    ///
    /// Per-machine by choice: a star reads as a fact about the document, but the
    /// alternative is a field in `DocumentMeta`, so starring would rewrite the
    /// whole `.ondin` and re-upload it through a sync client.
    pub starred: std::collections::HashSet<String>,
    /// The last few search queries typed here, newest first. Never swept: a
    /// stale query is a string that refers to nothing.
    pub recent_searches: Vec<String>,
    /// For each document, the kits it inserted from here, most recent first —
    /// the insert picker's last-use order (§5.3e, 3E; §15 D1014). Local for
    /// `opened`'s reason.
    pub kit_use: HashMap<String, Vec<String>>,
    /// For each document, the kit versions its update notice was told *Later*
    /// for; a newer publish brings the notice back (§5.3e, 5C; §15 D1016).
    pub kit_later: HashMap<String, HashMap<String, u32>>,
    /// Set when `library.json` exists but could not be read (§15 D418).
    ///
    /// A latch on writing, like [`super::state::Library::projects_unreadable`]:
    /// the app runs on an empty index while the file on disk is not empty, so the
    /// next `save` would finish what the corruption started. See
    /// [`Self::save_to`].
    ///
    /// `#[serde(skip)]`: a fact about this read, never stored.
    #[serde(skip)]
    pub unreadable: bool,
}

impl LocalIndex {
    /// Read it, or an empty one — recording which of the two it was.
    ///
    /// An unreadable file sets [`Self::unreadable`] rather than collapsing into
    /// "no file": stars are user intent, not derived data. Reading it as empty is
    /// right, since the app must run; writing that emptiness back is what the
    /// latch declines.
    ///
    /// Answers an empty index without looking when [`index_is_reachable`] is
    /// false, as under test (§15 D807).
    pub fn load() -> Self {
        if !index_is_reachable() {
            return Self::default();
        }
        let Some(p) = path() else {
            return Self::default();
        };
        // An index at the old Windows root (§15 D969) is carried across once,
        // before the first read; one that cannot be carried latches.
        if let Some(old) = crate::machine_dir::legacy_root().map(|r| r.join(FILE_NAME))
            && crate::machine_dir::adopt_legacy(&old, &p) == crate::machine_dir::Adopted::Blocked
        {
            return Self {
                unreadable: true,
                ..Self::default()
            };
        }
        Self::load_from(&p)
    }

    /// [`Self::load`] against an explicit path, so a test can drive its failures.
    ///
    /// `path()` has no injection point, so a test driving `load` or `save` would
    /// read or overwrite the developer's own index (§15 D370, D807); every
    /// decision either makes lives in this pair instead.
    pub fn load_from(path: &std::path::Path) -> Self {
        match std::fs::read(path) {
            // No file is a fresh install, where writing an empty index is right.
            // Matched on the error kind, not a follow-up `exists()` stat.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(_) => Self {
                unreadable: true,
                ..Self::default()
            },
            Ok(bytes) => match serde_json::from_slice::<Self>(&bytes) {
                Ok(index) => index,
                Err(_) => Self {
                    unreadable: true,
                    ..Self::default()
                },
            },
        }
    }

    /// Write it, ignoring failure.
    ///
    /// The reachability check stays the first line, before any path resolves, as
    /// in `Prefs::save`: the damage is silent and permanent (§15 D807).
    pub fn save(&self) {
        if !index_is_reachable() {
            return;
        }
        let Some(path) = path() else { return };
        self.save_to(&path);
    }

    /// [`Self::save`] against an explicit path.
    ///
    /// Atomic (`crate::atomic`, §15 D378): unlike covers and font data it is not
    /// derived, so a torn index would lose the Recent list and every star. It
    /// also refuses to write over a file it could not read; every open, star,
    /// duplicate and search saves, so without the latch one half-synced file
    /// costs every star at the first click.
    pub fn save_to(&self, path: &std::path::Path) {
        if self.unreadable {
            return;
        }
        if let Ok(bytes) = serde_json::to_vec(self) {
            let _ = crate::atomic::write(path, &bytes);
        }
    }

    /// Record that a document was opened now.
    pub fn mark_opened(&mut self, id: &str) {
        self.opened.insert(id.to_string(), super::clock::now());
    }

    /// When a document was last opened here, if ever.
    pub fn opened_at(&self, id: Option<&str>) -> Option<u64> {
        self.opened.get(id?).copied()
    }

    /// Whether this machine has recorded anything about this library.
    ///
    /// The witness that separates a fresh install from a library that has gone
    /// missing ([`super::state::Library::root_unavailable`]): both show an
    /// unreadable base folder and no documents, but only the latter has history
    /// here. Both sets are checked, as either can be empty alone.
    pub fn is_empty(&self) -> bool {
        self.opened.is_empty() && self.starred.is_empty()
    }

    /// Whether a document is starred here.
    pub fn is_starred(&self, id: &str) -> bool {
        self.starred.contains(id)
    }

    /// Star or unstar.
    pub fn toggle_star(&mut self, id: &str) {
        if !self.starred.remove(id) {
            self.starred.insert(id.to_string());
        }
    }

    /// Drop entries for documents that no longer exist.
    ///
    /// Called after a scan, with the ids that survived, so the maps do not grow
    /// for the life of the install.
    pub fn retain_known(&mut self, live: &std::collections::HashSet<String>) {
        self.opened.retain(|id, _| live.contains(id));
        // Starred is swept too: a stale id would silently re-star a document
        // restored from the trash.
        self.starred.retain(|id| live.contains(id));
        // A deleted kit in a live document's order is harmless (the picker
        // ignores it), so only the documents are swept.
        self.kit_use.retain(|id, _| live.contains(id));
        self.kit_later.retain(|id, _| live.contains(id));
    }
}

/// Whether this process may resolve [`path`] at all: `false` under `cfg(test)`,
/// so a test never reads or writes the developer's own `library.json` (§15 D807).
///
/// A `cfg!` rather than a flag set by a constructor, because tests call
/// [`super::state::Library::open`] directly after building the app and would
/// discard a flag the constructor set; a `cfg!` has no door to forget. It costs no
/// coverage: [`LocalIndex::load`] and [`LocalIndex::save`] only resolve
/// [`path`], and their decisions live in [`LocalIndex::load_from`] and
/// [`LocalIndex::save_to`], which tests drive.
fn index_is_reachable() -> bool {
    !cfg!(test)
}

/// The index's file name, in [`crate::machine_dir::root`] and in its legacy root
/// before §15 D969.
const FILE_NAME: &str = "library.json";

/// Beside the font cache.
fn path() -> Option<PathBuf> {
    Some(crate::machine_dir::root()?.join(FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The machine's own `library.json` is out of reach from a test (§15 D807).
    /// It asserts the guard rather than performing the side effect; the second
    /// assertion passes vacuously on a machine with no index, so the first holds.
    ///
    /// flip: delete the `if !index_is_reachable()` in `LocalIndex::load` → fails
    /// at the second assertion on a machine with a real index. The same deletion
    /// in `LocalIndex::save` leaves it green.
    #[test]
    fn the_machines_own_index_is_unreachable_from_a_test() {
        assert!(
            !index_is_reachable(),
            "a test may not read or write the machine's own `library.json`"
        );
        assert_eq!(
            LocalIndex::load().recent_searches,
            Vec::<String>::new(),
            "so `load` answers an empty index without looking"
        );
    }

    #[test]
    fn an_unopened_document_has_no_time_and_an_unfiled_one_asks_nothing() {
        let mut index = LocalIndex::default();
        assert_eq!(index.opened_at(Some("abc")), None);
        // A pre-library document has no id; it must not panic or match an empty key.
        assert_eq!(index.opened_at(None), None);
        index.mark_opened("abc");
        assert!(index.opened_at(Some("abc")).is_some());
        assert_eq!(index.opened_at(None), None);
    }

    /// An index that could not be read is never written back over. The bytes read
    /// back are the assertion, not the flag; the control is a missing file, a
    /// fresh install that must still write.
    ///
    /// flip: remove the `if self.unreadable { return; }` in `save_to` → fails at
    /// the bytes assertion.
    #[test]
    fn an_unreadable_index_is_not_overwritten_by_the_empty_one_it_produced() {
        let dir = std::env::temp_dir().join(format!("ondin-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("library.json");
        let corrupt = b"{not json";
        std::fs::write(&path, corrupt).unwrap();

        let mut index = LocalIndex::load_from(&path);
        assert!(index.unreadable, "the read failed and said so");
        assert!(index.starred.is_empty(), "and the app runs on an empty one");

        index.mark_opened("doc-1");
        index.toggle_star("doc-1");
        index.save_to(&path);
        // Compared as text so a failure prints the loss, not rows of decimals.
        assert_eq!(
            String::from_utf8_lossy(&std::fs::read(&path).unwrap()),
            String::from_utf8_lossy(corrupt),
            "the file a user could still repair is untouched"
        );

        // Control: no file at all is a fresh install and must still write.
        let fresh = dir.join("fresh.json");
        let mut index = LocalIndex::load_from(&fresh);
        assert!(!index.unreadable);
        index.toggle_star("doc-1");
        index.save_to(&fresh);
        assert!(
            LocalIndex::load_from(&fresh).is_starred("doc-1"),
            "a first run still records what it did"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sweeping_drops_documents_that_are_gone() {
        let mut index = LocalIndex::default();
        index.mark_opened("still-here");
        index.mark_opened("deleted");
        index.retain_known(&HashSet::from(["still-here".to_string()]));
        assert!(index.opened_at(Some("still-here")).is_some());
        assert_eq!(index.opened_at(Some("deleted")), None);
    }
}
