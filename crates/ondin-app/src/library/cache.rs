//! The half of the library that does **not** travel: what this machine knows
//! about documents it has opened (§15 D366).
//!
//! **The dividing line is "would you expect this on your other laptop".** The
//! name, the project and the creation date would, so they live in the document
//! (`ondin_core::meta`) inside a base folder the user can point at a sync
//! client. *Last opened* would not — "recent" means recent *here*, and a file
//! somebody else touched on another machine appearing at the top of this one's
//! Recent list would be a bug rather than a feature.
//!
//! So this lives beside the font cache under `dirs::cache_dir()/ondin`.
//!
//! ⚠️ **It is *local*, and that is not the same as disposable** — this paragraph
//! said it was, and [`LocalIndex::load`] used to be written on that argument.
//! Deleting it loses the ordering of the Recent list, which refills as documents
//! are opened, **and every star on the machine**, which does not: a star is
//! something a person did, nothing else records it, and no amount of using the
//! app brings one back. That is why the read reports an unreadable file
//! ([`LocalIndex::unreadable`]) instead of collapsing it into "no file".
//!
//! ⚠️ **Keyed on the document id, not the path.** A path key goes stale the first
//! time a document is renamed — which the dashboard makes a one-click action —
//! and the entry it leaves behind would keep a document in Recent under a name
//! that no longer exists.

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
    /// ⚠️ **Per-machine, which is a choice and may be the wrong one.** A star
    /// reads as a fact about the *document*, so a user with a synced library
    /// will reasonably expect it on their other laptop. It is here anyway
    /// because the alternative is a field in `DocumentMeta`, which makes
    /// starring rewrite the whole `.ondin` — a 12 MB write and a sync upload for
    /// one click on a star. Worth revisiting if the two ever stop being the only
    /// options.
    pub starred: std::collections::HashSet<String>,
    /// The last few search queries typed here, newest first.
    ///
    /// Local without an argument: a search is something a person did at a
    /// keyboard, not a property of the library. Nothing sweeps it — unlike
    /// [`Self::opened`] and [`Self::starred`], a stale query is a *string* and
    /// refers to nothing that could have been deleted.
    pub recent_searches: Vec<String>,
    /// Set when `library.json` exists but could not be read (§15 D418).
    ///
    /// ⚠️ **A latch on writing, the same one
    /// [`super::state::Library::projects_unreadable`] is** — and it is here
    /// because the argument the module head made for *not* having one was wrong
    /// about which fields this file holds. With this set, the app has an empty
    /// index and a file on disk that is not empty, so the next `save` would
    /// finish what the corruption started. Every write checks it; see
    /// [`Self::save_to`].
    ///
    /// `#[serde(skip)]`: it is a fact about *this* read, never a stored one.
    #[serde(skip)]
    pub unreadable: bool,
}

impl LocalIndex {
    /// Read it, or an empty one — recording which of the two it was.
    ///
    /// ⚠️ **This file used to argue it needed no three-way read and the
    /// argument was wrong**, in a way worth keeping because both halves of it
    /// are still written in this module. `load`'s reasoning was *"this file is
    /// derived and local; the worst an overwrite costs is a Recent list that has
    /// forgotten"*; fourteen lines below it [`Self::save_to`] says *"nothing can
    /// reconstruct which documents this machine has opened, so a torn index
    /// silently loses the Recent list **and every star**"*. The second is right,
    /// and the stars are exactly what the first omits — they are user intent,
    /// not derived data, and nothing anywhere else records them.
    ///
    /// So an unreadable file now sets [`Self::unreadable`] rather than being
    /// collapsed into "no file", and the *write* is what declines. Reading it as
    /// empty is still correct: the app has to run, and an empty Recent list is a
    /// survivable frame. What was not survivable was writing that emptiness back.
    ///
    /// Answers an empty index without looking when [`index_is_reachable`] says
    /// the machine's file is off limits, which is what a test gets (§15 D807).
    pub fn load() -> Self {
        if !index_is_reachable() {
            return Self::default();
        }
        match path() {
            Some(p) => Self::load_from(&p),
            None => Self::default(),
        }
    }

    /// [`Self::load`] against an explicit path, so the failure it is built
    /// around can be driven by a test.
    ///
    /// ⚠️ **`load` itself is untestable and that is the point of this pair.**
    /// `path()` reads `dirs::cache_dir()` with no injection point, so a probe
    /// driving `load` would read — and a probe driving `save` would *overwrite* —
    /// the developer's own library index. §15 D370 exists because a test once
    /// did exactly that to `prefs.json`, and §15 D807 because the suite was
    /// doing it here: [`index_is_reachable`] is why a test now gets nothing out
    /// of `load` and cannot write through `save` at all, and this pair is still
    /// where every decision either of them makes is asserted.
    pub fn load_from(path: &std::path::Path) -> Self {
        match std::fs::read(path) {
            // No file yet is the ordinary state of a fresh install, and it is
            // the one case where writing an empty index is right. Matched on the
            // error *kind* rather than on a follow-up `exists()`, which is
            // `Projects::read`'s idiom and avoids a second stat that can
            // disagree with the first.
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
    /// ⚠️ **First line, before anything resolves a path**, which is
    /// `Prefs::save`'s own shape and is load-bearing for the same reason: the
    /// damage is silent and permanent. See [`index_is_reachable`] (§15 D807).
    pub fn save(&self) {
        if !index_is_reachable() {
            return;
        }
        let Some(path) = path() else { return };
        self.save_to(&path);
    }

    /// [`Self::save`] against an explicit path.
    ///
    /// Atomic (`crate::atomic`, §15 D378). ⚠️ **A cache, and still not one of the
    /// files that module exempts**: covers and font data are exempt because they
    /// are *derived* — a torn one is regenerated from the document it came from —
    /// and this is not. Nothing can reconstruct which documents this machine has
    /// opened, so a torn index silently loses the *Recent* list and every star.
    ///
    /// Which is why it also refuses to write over a file it could not read: the
    /// doors are ordinary — every `open_path`, every star, every duplicate, every
    /// search — so without the latch a single half-synced file costs every star
    /// on the machine at the first click after launch.
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
    /// **The witness that separates a fresh install from a library that has gone
    /// missing** ([`super::state::Library::root_unavailable`]). Both show an
    /// unreadable base folder and no documents; only one of them has a file here
    /// saying that this computer has opened or starred something in it.
    ///
    /// Both sets, because either one alone is evidence: a user who has starred a
    /// document and never re-opened it has an empty `opened` map, and one whose
    /// stars were all on documents they have since deleted has an empty
    /// `starred` set.
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
    /// Called after a scan, with the ids that survived. Without it the map grows
    /// for the life of the install — every document ever deleted keeps its row —
    /// and since the key is an id there is nothing about a stale row to notice.
    pub fn retain_known(&mut self, live: &std::collections::HashSet<String>) {
        self.opened.retain(|id, _| live.contains(id));
        // ⚠️ **The starred set is swept too, and it is the one that would rot
        // silently.** An `opened` entry for a deleted document is invisible —
        // nothing draws it. A `starred` one is invisible *and* comes back:
        // restore the document from the trash and it is mysteriously starred
        // again, months later, because nothing ever cleared the id.
        self.starred.retain(|id| live.contains(id));
    }
}

/// Whether this process may resolve [`path`] at all — `false` under
/// `cfg(test)`, which is the injection point this module said it did not have
/// (§15 D807).
///
/// 🚨 **Running the suite edited the developer's own *Recent searches*.**
/// [`LocalIndex::save`] writes `dirs::cache_dir()/ondin/library.json` and
/// [`LocalIndex::load`] reads it, and the app fixture redirects the **base
/// folder**, which is a different knob — so anything driving the app reached the
/// real file. The first run of `a_remembered_search_is_deduped_promoted_capped_and_shown`
/// found its fixture already populated from a previous run, and the arrows test
/// had been writing there for longer. §11's *"a test may not write outside the
/// repository"* held here by test discipline rather than by construction.
///
/// **A `cfg!` rather than a flag set by a constructor, and the reason is
/// [`super::state::Library::open`].** `Prefs::ephemeral` is a field because
/// `Prefs` is built once, by the one function that knows a test's app is
/// different; this index is built by `Library::open`, which **tests call
/// directly after building the app** — `app.library = Library::open(root)` at
/// two sites — so a field the constructor set would be silently discarded by the
/// very fixture that needs it, and the second-cheapest answer (a process-wide
/// flag stored by that constructor, `CLIPBOARD_OFF`'s spelling for §15 D798)
/// would leave every `Library::open` in `library::state`'s own tests reading the
/// machine's file until some *other* test in the binary happened to build an
/// app first. A `cfg!` has no door to forget.
///
/// ⚠️ **What this does not cost is coverage, and that is the whole of why it is
/// allowed to differ by profile.** [`LocalIndex::load`] and [`LocalIndex::save`]
/// hold no behaviour beyond resolving [`path`]: every decision either one makes
/// — the three-way read, the [`LocalIndex::unreadable`] write latch, the
/// round trip — is in [`LocalIndex::load_from`] and [`LocalIndex::save_to`],
/// which are `pub`, take a path and are asserted below. That is not true of the
/// other two resources in this class, which is why neither took this answer:
/// `prefs::Prefs`' latch test drives `save_to`, and `clipboard_gate_tests` has
/// to open a *real* handle to prove the lock holds under eight threads.
fn index_is_reachable() -> bool {
    !cfg!(test)
}

/// Beside the font cache, which is the closest precedent for the path.
fn path() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("ondin").join("library.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The machine's own `library.json` is out of reach from a test, and the two
    /// functions that could resolve it both ask first (§15 D807).
    ///
    /// ⚠️ **It asserts the guard and does not perform the side effect**, which is
    /// `an_ephemeral_prefs_never_reaches_the_disk`'s rule and for the same
    /// reason: a probe that called `save()` to prove the negative *is* the bug.
    /// So the first assertion is where the decision is, and the second is the
    /// only consequence that can be observed without writing.
    ///
    /// ⚠️ **The second assertion has teeth exactly where the defect was and is
    /// vacuous on a clean box**, which is worth stating rather than hiding: on a
    /// machine that has used the app — or has run this suite before the fix —
    /// `load()` returns somebody's real Recent list, and here it must not. On a
    /// fresh CI runner there is no file and an unguarded `load` answers `Default`
    /// too. It can therefore false-*pass* and never false-fail, and the first
    /// assertion is what holds when it does.
    ///
    /// **Flip-check, run** by deleting the `if !index_is_reachable()` from
    /// `LocalIndex::load`: fails on the second assertion with
    /// `["alp", "d", "alpha", "gamma", "beta"]` — this suite's own fixture
    /// strings, read back out of the developer's cache directory, which is the
    /// reported symptom itself rather than a stand-in for it. Deleting the guard
    /// in `LocalIndex::save` instead leaves this green, by construction: the
    /// only thing that would catch it is a write nobody may perform, so that
    /// half rests on the first assertion and on the `cfg!`.
    ///
    /// (Plain backticks above, not `[links]` — §15 D319's convention inside a
    /// `cfg(test)` module, which `cargo doc` cannot see at all.)
    #[test]
    fn the_machines_own_index_is_unreachable_from_a_test() {
        assert!(
            !index_is_reachable(),
            "a test may not read or write `dirs::cache_dir()/ondin/library.json`"
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
        // ⚠️ A document with no id at all — a pre-library file — must not panic
        // or be confused with one that has never been opened under some empty
        // key. `opened_at` takes an `Option` for exactly this.
        assert_eq!(index.opened_at(None), None);
        index.mark_opened("abc");
        assert!(index.opened_at(Some("abc")).is_some());
        assert_eq!(index.opened_at(None), None);
    }

    /// An index that could not be read is **never written back over**.
    ///
    /// ⚠️ **The bytes are the assertion, not the flag.** What this is about is
    /// that a half-synced or hand-broken `library.json` costs the user every
    /// star on the machine at the first ordinary click after launch — every
    /// `open_path`, star, duplicate and search calls `save` — so the test does
    /// the whole round trip and then reads the file back, rather than asserting
    /// that a boolean is set.
    ///
    /// Flip-check, run: removing the `if self.unreadable { return; }` from
    /// `save_to` fails on the bytes assertion — the predicted site — with
    /// `{"opened":{"doc-1":1788708912},"starred":["doc-1"],"recent_searches":[]}`
    /// where `{not json` was. The `mark_opened`/`toggle_star` in between are what
    /// make the flip bite *legibly*: without them the write would still happen
    /// and the failure would read as an empty index rather than as a star that
    /// replaced somebody's whole file.
    ///
    /// The control below is the other half: a **missing** file is a fresh
    /// install, and writing an empty index over nothing is exactly right.
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
        // ⚠️ Compared as text, not as `Vec<u8>`. Under the flip a byte-slice
        // comparison prints two rows of decimal numbers; this prints
        // `{"opened":{"doc-1":…},"starred":["doc-1"],…}` against `{not json`,
        // which is the loss itself.
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
