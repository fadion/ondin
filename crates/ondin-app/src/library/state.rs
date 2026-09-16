//! The library as the app holds it for a frame: the root, the projects, the
//! entries, and the sort the dashboard is showing them in.
//!
//! **Rebuilt from disk rather than maintained.** Every write in
//! [`super::store`] changes files, and this is re-scanned afterwards rather than
//! patched to match — which is what makes the filesystem the truth in practice
//! and not only in the module docs. A patched list and a scanned one disagree the
//! first time something changes the folder that is not this app, and the base
//! folder is explicitly meant to be a synced directory where that happens all the
//! time.
//!
//! ⚠️ **So a scan is a real cost and must not be per-frame.** [`Library::refresh`]
//! reads every document's prefix; sixty documents is sixty file opens. The
//! dashboard calls it when it is entered, after a write, and on nothing else.

use super::cache::LocalIndex;
use super::project::{Project, Projects, ProjectsRead};
use super::scan::{self, Entry};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// How the dashboard orders what it shows.
///
/// The four the design's sort control cycles. Stored in preferences by
/// [`Sort::id`] rather than by index, so inserting one later cannot silently
/// change what an existing install opens on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Edited,
    Name,
    Created,
    Size,
}

impl Sort {
    /// The order the control cycles through, which is the design's.
    pub const ALL: [Sort; 4] = [Sort::Edited, Sort::Name, Sort::Created, Sort::Size];

    /// The stable key written to preferences.
    pub fn id(self) -> &'static str {
        match self {
            Sort::Edited => "edited",
            Sort::Name => "name",
            Sort::Created => "created",
            Sort::Size => "size",
        }
    }

    /// What the control shows.
    pub fn label(self) -> &'static str {
        match self {
            Sort::Edited => "Last edited",
            Sort::Name => "Name",
            Sort::Created => "Date created",
            Sort::Size => "Size",
        }
    }

    /// Read a preference back, degrading to the default rather than failing.
    pub fn from_id(id: &str) -> Sort {
        Sort::ALL
            .into_iter()
            .find(|s| s.id() == id)
            .unwrap_or_default()
    }

    /// The next in the cycle — the sort control is one button, not a menu.
    pub fn next(self) -> Sort {
        let i = Sort::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Sort::ALL[(i + 1) % Sort::ALL.len()]
    }
}

/// Everything the dashboard reads, plus where it came from.
pub struct Library {
    /// The resolved base folder. **Resolved once and kept**, so that a setting
    /// edited mid-session cannot make two halves of one frame disagree about
    /// where the library is.
    pub root: PathBuf,
    pub projects: Projects,
    /// Every document found by the last [`Self::refresh`].
    pub entries: Vec<Entry>,
    /// The trash's listing as of the last [`Self::refresh_trash`] (§15 D728).
    ///
    /// 🚨 **Held because reading it is a scan, and the sidebar asks every frame.**
    /// `store::trashed` is `scan::scan_dir`, which opens and JSON-prefix-parses
    /// **every file in `.trash`** — and `dashboard_sidebar` paints the *Trash*
    /// count on every nav, whether or not the trash is being looked at, with
    /// `visible_entries` scanning a second time for the same frame when it is.
    /// With 200 documents in the trash that was 200 file opens a frame, 400 on
    /// *Trash*, for a number that changes only when the user acts
    /// (`[S20.1-L3-07]`).
    ///
    /// ⚠️ **This module's own head already forbade it** — *"a refresh reads every
    /// document's prefix… **Nothing in this file may call it from inside a layout
    /// loop**"* — and named `Library::refresh` as the thing not to call. This was
    /// the same hazard by a route that sentence does not enumerate, which is
    /// exactly why it read as guarded and was not. **A rule that names one road
    /// is not a rule about the destination.**
    ///
    /// ⚠️ **Private, unlike [`Self::entries`].** A `pub` field is a second way to
    /// get the answer and the wrong one to reach for; [`Self::trashed`] is the
    /// read, and the only way to move this is a refresh — which is what keeps the
    /// invalidation enumerable.
    trashed: Vec<Entry>,
    /// Per-machine facts — the *last opened* times behind the Recent list.
    pub local: LocalIndex,
    /// Set when `projects.json` exists but could not be read.
    ///
    /// ⚠️ **A latch on writing, not a message.** With this set, the app has an
    /// empty project list and a file on disk that is not empty — so creating a
    /// project would write one project over however many were really there.
    /// Every writer checks it; see [`Self::may_write_projects`].
    pub projects_unreadable: bool,
    /// Set when the base folder could not be listed **and** this machine has
    /// recorded documents in it — an unplugged drive, a sync folder that has not
    /// mounted, a library renamed in Explorer (§15 D384).
    ///
    /// ⚠️ **The second half is what makes this a fact rather than a guess.** A
    /// fresh install has no base folder either — it is created by the first
    /// write, not at launch — so an unreadable root on its own is the ordinary
    /// state of an app nobody has saved anything with yet. What tells the two
    /// apart is [`LocalIndex::is_empty`]: a per-machine index with something in
    /// it is this computer saying it has seen documents in this library, and an
    /// empty scan is then a *disagreement* rather than an answer.
    ///
    /// A latch on writing as well as a message, for `projects_unreadable`'s
    /// reason one level up: creating a document while the real library is offline
    /// makes a **second** library at the same path, which is the shape of loss a
    /// sync client then resolves by picking one.
    pub root_unavailable: bool,
}

impl Library {
    /// Open the library at `root` and scan it.
    pub fn open(root: PathBuf) -> Self {
        let read = Projects::read(&root);
        let projects_unreadable = matches!(read, ProjectsRead::Unreadable);
        let mut lib = Self {
            root,
            projects: read.or_empty(),
            entries: Vec::new(),
            // Both filled by the `refresh` below, which is what `open` is for.
            trashed: Vec::new(),
            local: LocalIndex::load(),
            projects_unreadable,
            root_unavailable: false,
        };
        lib.refresh();
        lib
    }

    /// Re-read the folder. See the module note on why this is not per-frame.
    ///
    /// ⚠️ **The sweep is gated on the folder having been readable, and that is
    /// the destructive half of this function.** [`LocalIndex::retain_known`]
    /// drops every id the scan did not see — which is right after a delete and
    /// catastrophic after an unplugged drive, where the scan sees *nothing*: one
    /// launch with the library offline and every star on the machine is gone, for
    /// good, the moment anything saves the index afterwards. The list being empty
    /// is not evidence that the documents are; only a folder that was actually
    /// listed is.
    ///
    /// The read is a second syscall rather than something [`scan::scan`] returns,
    /// because *every other* error that walk meets is one it is right to drop
    /// (see its own note) and threading a result type through it would be a worse
    /// trade than one `read_dir` per refresh — which is already sixty file opens
    /// for sixty documents.
    pub fn refresh(&mut self) {
        let readable = scan::readable(&self.root);
        self.entries = scan::scan(&self.root);
        self.refresh_trash();
        self.root_unavailable = !readable && !self.local.is_empty();
        if !readable {
            return;
        }
        let live: HashSet<String> = self
            .entries
            .iter()
            .filter_map(|e| e.meta.id.clone())
            .collect();
        self.local.retain_known(&live);
    }

    /// The trash's listing, as of the last refresh (§15 D728).
    ///
    /// **Cheap, and it is the only way to ask.** `store::trashed` is a scan; see
    /// the field.
    pub fn trashed(&self) -> &[Entry] {
        &self.trashed
    }

    /// Re-read the trash alone (§15 D728).
    ///
    /// **The narrow half of [`Self::refresh`], for callers that moved a file into
    /// or out of `.trash` and changed nothing else** — `purge_trash` at launch and
    /// from the Settings card, and the **permanent delete** below. A full `refresh`
    /// re-scans the whole library to learn a number that came from one folder.
    ///
    /// ⚠️ **The other delete paths are not callers of this and should not be**:
    /// trashing a document, `delete_project`'s member sweep and `Act::Restore` each
    /// move a file *between* the library and `.trash`, so both listings changed and
    /// a full `refresh` is the right reach.
    ///
    /// 🚨 **This is the invalidation point, and it is the whole risk of caching
    /// the trash.** `entries` has one producer (`scan::scan`) and one consumer
    /// contract; the trash is written by `store::trash`, `store::purge_trash`,
    /// `relocate` **and one door that goes through none of them**, and *before this
    /// cache existed not one of them had to tell anybody*. `refresh` calls this, so
    /// anything that already refreshed is covered without being changed.
    ///
    /// 🚨 **The door outside `store` is the one that was missed, and the enumeration
    /// is why.** `trash_or_purge`'s `Nav::Trash` arm deletes for good with a bare
    /// `fs::remove_file` in a panel — so a grep for `store::trash` / `store::purge_trash`
    /// cannot see it, and the permanently-deleted document stayed in the Trash list
    /// and in the sidebar's count until something unrelated refreshed. Fixed at that
    /// site the day after this cache landed; §15 D728 carries the episode.
    ///
    /// ⚠️ **A count is deliberately not written here.** This doc said *"two of the
    /// five call sites … a sixth writer added later has to know"*, and the sixth
    /// writer was **already in the tree** — a number in prose is a gate nobody built,
    /// and this one read as a survey of a closed set. Enumerate instead, from the
    /// repository root, and read both:
    ///
    /// ```text
    /// grep -rn 'store::trash(\|store::purge_trash(' crates/ondin-app/src --include=*.rs
    /// grep -rn 'remove_file\|fs::rename' crates/ondin-app/src --include=*.rs
    /// ```
    ///
    /// The first finds the doors that announce themselves; **the second is the one
    /// that matters**, because a writer that reaches `.trash` without naming `store`
    /// is exactly the writer this cache cannot survive.
    pub fn refresh_trash(&mut self) {
        self.trashed = super::store::trashed(&self.root);
    }

    /// Re-read `projects.json` as well — after a write, or after the base folder
    /// changed.
    pub fn reload_projects(&mut self) {
        let read = Projects::read(&self.root);
        self.projects_unreadable = matches!(read, ProjectsRead::Unreadable);
        self.projects = read.or_empty();
    }

    /// Whether it is safe to write `projects.json`.
    ///
    /// False when the file on disk could not be parsed: the in-memory list is
    /// empty because of that failure rather than because there are no projects,
    /// and writing it back would turn a recoverable sync conflict into a real
    /// loss.
    pub fn may_write_projects(&self) -> bool {
        !self.projects_unreadable
    }

    /// Whether it is safe to create anything in the base folder.
    ///
    /// False only when [`Self::root_unavailable`] — i.e. the folder could not be
    /// listed and this machine says there should be documents in it. **Every
    /// write here is a `create_dir_all` away from resurrecting the path**, so
    /// filing a new document into an offline library does not fail: it succeeds,
    /// into a brand-new empty folder wearing the missing one's name, and the sync
    /// client that brings the real one back then has two libraries to reconcile.
    ///
    /// Not the same question as [`Self::may_write_projects`], which is about one
    /// file being unparseable rather than the folder being gone, and both are
    /// asked because either can be true without the other.
    pub fn may_write(&self) -> bool {
        !self.root_unavailable
    }

    /// Save `projects.json`, refusing when [`Self::may_write_projects`] says so.
    ///
    /// Returns whether it wrote.
    pub fn save_projects(&mut self) -> bool {
        if !self.may_write_projects() {
            return false;
        }
        self.projects.write(&self.root).is_ok()
    }

    /// The project an entry belongs to, by the id in its own metadata.
    ///
    /// `None` covers both "belongs to no project" and "belongs to a project id
    /// nothing on this machine knows about" — the second being what a document
    /// synced from a machine whose `projects.json` has not arrived yet looks
    /// like. Treating it as unfiled is right: the document shows up in *All
    /// files*, nothing is lost, and it joins its project the moment the list
    /// syncs.
    pub fn project_of(&self, entry: &Entry) -> Option<&Project> {
        self.projects.get(entry.meta.project.as_deref()?)
    }

    /// Whether this document is a sync client's conflict copy, and the name of
    /// the document it is a copy of.
    ///
    /// **The scan cannot answer this and this is why it is here**:
    /// [`scan::conflict_marker`] reads one filename, and half its answers
    /// ([`scan::Conflict::Suspected`]) are only conflicts if the document they
    /// claim to copy is sitting beside them — which is a question about the
    /// *list*. `landing-v4 (1).ondin` next to `landing-v4.ondin` is a sync
    /// artefact; on its own it is somebody's file, and marking it would be the
    /// app telling a user their document is broken because of a bracket.
    ///
    /// ⚠️ **Same directory, not merely the same stem.** A project is a folder
    /// (`super::project`), so `landing-v4.ondin` at the root and
    /// `landing-v4 (1).ondin` inside *Kestrel* are two unrelated documents that a
    /// stem comparison alone would pair. Compared case-insensitively because the
    /// filesystem this most often runs on is.
    ///
    /// **Linear, and that is affordable because the marker is checked first.**
    /// The scan of `entries` only happens for a filename that already carries a
    /// suspected marker, which is nearly none of them — a per-frame `O(n²)` over
    /// every card would not be (see this module's note on cost).
    pub fn conflict(&self, entry: &Entry) -> Option<String> {
        let (kind, base) = scan::conflict_marker(&entry.stem)?;
        match kind {
            scan::Conflict::Stated => Some(scan::name_from_stem(&base)),
            scan::Conflict::Suspected => self
                .entries
                .iter()
                .find(|e| {
                    e.stem.eq_ignore_ascii_case(&base) && e.path.parent() == entry.path.parent()
                })
                .map(|e| e.display_name()),
        }
    }

    /// How many documents point at a project.
    pub fn file_count(&self, project_id: &str) -> usize {
        self.entries
            .iter()
            .filter(|e| e.meta.project.as_deref() == Some(project_id))
            .count()
    }

    /// The most recent edit among a project's documents, for its subtitle.
    pub fn project_touched(&self, project_id: &str) -> Option<u64> {
        self.entries
            .iter()
            .filter(|e| e.meta.project.as_deref() == Some(project_id))
            .map(|e| e.modified)
            .max()
    }

    /// Order a set of entries in place.
    ///
    /// **Every sort is broken by name and then by stem**, so the order is total:
    /// two documents of the same size, or two created in the same second — which
    /// is what a bulk import produces — would otherwise land in whatever order
    /// the directory read happened to give, and shuffle between visits for no
    /// reason the user could see.
    pub fn sort_entries(&self, entries: &mut [Entry], sort: Sort) {
        entries.sort_by(|a, b| {
            let primary = match sort {
                // Descending: the most recent first, which is what every one of
                // these questions is actually asking.
                Sort::Edited => b.modified.cmp(&a.modified),
                Sort::Created => b.meta.created.cmp(&a.meta.created),
                Sort::Size => b.size.cmp(&a.size),
                Sort::Name => std::cmp::Ordering::Equal,
            };
            primary
                .then_with(|| {
                    a.display_name()
                        .to_lowercase()
                        .cmp(&b.display_name().to_lowercase())
                })
                .then_with(|| a.stem.cmp(&b.stem))
        });
    }

    /// The documents this machine has opened, most recent first.
    ///
    /// **Only documents that have actually been opened here**, which is what
    /// makes this different from sorting everything by edit time: a library
    /// synced onto a new machine has a full *All files* and an empty *Recent*,
    /// and that is the honest answer.
    pub fn recent(&self) -> Vec<Entry> {
        let mut out: Vec<(u64, Entry)> = self
            .entries
            .iter()
            .filter_map(|e| {
                let at = self.local.opened_at(e.meta.id.as_deref())?;
                Some((at, e.clone()))
            })
            .collect();
        out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.stem.cmp(&b.1.stem)));
        out.into_iter().map(|(_, e)| e).collect()
    }

    /// Find the entry for an open document's path.
    pub fn entry_at(&self, path: &Path) -> Option<&Entry> {
        self.entries.iter().find(|e| e.path == path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::project::PROJECT_COLORS;
    use crate::library::store;
    use ondin_core::{Document, IdSource};

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ondin-libstate-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn blank() -> Document {
        let mut ids = IdSource::new(0xD0C);
        Document::new(ids.mint())
    }

    fn file(root: &Path, name: &str, project: Option<&Project>) -> String {
        let mut doc = blank();
        store::file_document(root, project, name, &mut doc).unwrap();
        doc.meta().id.clone().unwrap()
    }

    #[test]
    fn the_sort_cycle_round_trips_through_its_preference_key() {
        for s in Sort::ALL {
            assert_eq!(Sort::from_id(s.id()), s);
        }
        // ⚠️ An unknown key degrades rather than failing — the whole reason the
        // preference stores an id instead of an index.
        assert_eq!(Sort::from_id("by-vibes"), Sort::Edited);
        assert_eq!(Sort::from_id(""), Sort::Edited);
        // And the cycle closes.
        let mut s = Sort::Edited;
        for _ in 0..Sort::ALL.len() {
            s = s.next();
        }
        assert_eq!(s, Sort::Edited);
    }

    /// Build an entry directly, so the display name and the stem can be set
    /// independently.
    ///
    /// ⚠️ **Not written to disk, and that is the point — see
    /// `a_tied_sort_falls_back_to_the_name`.** A sort test built from a temp
    /// folder is partly answered by the filesystem rather than by the code under
    /// test.
    fn entry(name: &str, stem: &str, modified: u64, created: u64, size: u64) -> Entry {
        Entry {
            path: PathBuf::from(format!("/lib/{stem}.ondin")),
            stem: stem.into(),
            meta: ondin_core::meta::DocumentMeta {
                id: Some(stem.into()),
                name: Some(name.into()),
                project: None,
                created: Some(created),
            },
            modified,
            size,
            unread: false,
        }
    }

    /// ⚠️ Sorting by a field that ties must still be a total order. Documents
    /// created in the same second — which is what an import produces — are
    /// ordered by name.
    ///
    /// ⚠️ **This test was vacuous in its first form and the flip is what found
    /// it.** It built three documents in a temp folder and asserted they came
    /// back alphabetically; dropping the whole `then_with` chain left it green,
    /// because **`read_dir` on NTFS returns entries already sorted by
    /// filename** — so the assertion was being satisfied by the operating system
    /// and the tie-break could have been deleted without a single test noticing.
    /// Any sort test whose fixture is a real directory has that problem.
    ///
    /// Two things fix it and both are needed: the entries are constructed rather
    /// than scanned, so the input order is chosen; and the stems are in the
    /// *opposite* order to the display names, so a tie-break that fell through to
    /// the stem — the second link in the chain — would also fail. Flipped again
    /// afterwards: dropping the chain now fails here, and dropping only the
    /// display-name link fails here too.
    #[test]
    fn a_tied_sort_falls_back_to_the_name() {
        let lib = Library::open(temp_root("ties"));
        // Stems ascend a, b, c while names descend Zebra, Mango, Apple, and the
        // input order is a third order again.
        let mut entries = vec![
            entry("Mango", "b", 100, 100, 10),
            entry("Zebra", "a", 100, 100, 10),
            entry("Apple", "c", 100, 100, 10),
        ];
        lib.sort_entries(&mut entries, Sort::Created);
        let names: Vec<String> = entries.iter().map(|e| e.display_name()).collect();
        assert_eq!(names, vec!["Apple", "Mango", "Zebra"]);
        let _ = std::fs::remove_dir_all(&lib.root);
    }

    #[test]
    fn name_sort_is_case_insensitive() {
        let lib = Library::open(temp_root("case"));
        let mut entries = vec![
            entry("beta", "x", 0, 0, 0),
            entry("Gamma", "y", 0, 0, 0),
            entry("Alpha", "z", 0, 0, 0),
        ];
        lib.sort_entries(&mut entries, Sort::Name);
        let names: Vec<String> = entries.iter().map(|e| e.display_name()).collect();
        // ⚠️ Byte order would put every capital before every lowercase, so this
        // reads "Alpha", "Gamma", "beta" without the fold. The stems are in
        // reverse order so the fall-through cannot produce this by accident.
        assert_eq!(names, vec!["Alpha", "beta", "Gamma"]);
        let _ = std::fs::remove_dir_all(&lib.root);
    }

    /// The descending sorts, which the tie-break tests above deliberately do not
    /// exercise — every fixture there is tied on its primary key.
    #[test]
    fn edited_created_and_size_all_read_largest_first() {
        let lib = Library::open(temp_root("desc"));
        let build = || {
            vec![
                entry("Small old", "a", 100, 100, 10),
                entry("Big new", "b", 300, 300, 30),
                entry("Middle", "c", 200, 200, 20),
            ]
        };
        for sort in [Sort::Edited, Sort::Created, Sort::Size] {
            let mut entries = build();
            lib.sort_entries(&mut entries, sort);
            let names: Vec<String> = entries.iter().map(|e| e.display_name()).collect();
            assert_eq!(names, vec!["Big new", "Middle", "Small old"], "{sort:?}");
        }
        let _ = std::fs::remove_dir_all(&lib.root);
    }

    /// Recent is what *this machine* has opened, so a freshly synced library has
    /// a full *All files* and an empty *Recent*.
    #[test]
    fn recent_is_empty_until_something_is_opened_here() {
        let root = temp_root("recent");
        let a = file(&root, "Landing v4", None);
        file(&root, "Pricing table", None);
        let mut lib = Library::open(root.clone());
        assert_eq!(lib.entries.len(), 2);
        assert!(lib.recent().is_empty(), "nothing has been opened here yet");

        lib.local.mark_opened(&a);
        let recent = lib.recent();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].display_name(), "Landing v4");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A document pointing at a project nothing knows about is unfiled rather
    /// than hidden — the state a half-synced library is in.
    #[test]
    fn a_document_naming_an_unknown_project_is_treated_as_unfiled() {
        let root = temp_root("orphan");
        let ghost = Project {
            id: "not-in-the-list".into(),
            name: "Ghost".into(),
            color: PROJECT_COLORS[0].into(),
            folder: None,
            created: 0,
            archived: false,
        };
        file(&root, "Landing v4", Some(&ghost));
        let lib = Library::open(root.clone());
        assert_eq!(lib.entries.len(), 1, "it must still be listed");
        assert!(lib.project_of(&lib.entries[0]).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ⚠️ The latch. A `projects.json` that cannot be parsed must not be
    /// overwritten by the empty list that failure produced.
    ///
    /// Flip-check, run: making `save_projects` write unconditionally leaves the
    /// first two assertions green and fails the last — and the damage it models
    /// is real, since the bytes it would replace are somebody's whole project
    /// list mid-sync.
    #[test]
    fn a_broken_projects_file_is_never_written_over() {
        let root = temp_root("latch");
        let broken = b"{ \"projects\": [ ".as_slice();
        std::fs::write(root.join(super::super::project::PROJECTS_FILE), broken).unwrap();
        let mut lib = Library::open(root.clone());
        assert!(lib.projects.projects.is_empty());
        assert!(!lib.may_write_projects());
        assert!(!lib.save_projects(), "the write must be refused");
        assert_eq!(
            std::fs::read(root.join(super::super::project::PROJECTS_FILE)).unwrap(),
            broken,
            "and the bytes on disk must be untouched"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A stated conflict stands alone; a suspected one needs the document it
    /// claims to copy, in the same folder (§15 D375).
    ///
    /// ⚠️ **The fixture is written by hand rather than through `store`,** because
    /// nothing in this app can produce these filenames — that is the property
    /// `conflict_marker` leans on. `std::fs::copy` of a real document is also what
    /// makes the point the mark exists for: the copy carries the original's
    /// metadata, so both entries report the same `Entry::display_name`, the same
    /// project and the same id, and only the stem differs.
    ///
    /// **Flip-checked** by making `Conflict::Suspected` return `Some` without
    /// consulting `entries`: the second assertion fails with the lone `(1)` file
    /// marked as a copy of "Orphan", which is somebody's document being told it is
    /// broken. Predicted that site and it was that site;
    /// `a_conflict_is_matched_inside_one_folder_only` fails on the same flip, at
    /// its only assertion.
    #[test]
    fn a_suspected_conflict_needs_its_original_and_a_stated_one_does_not() {
        let root = temp_root("conflict");
        let mut doc = blank();
        let original = store::file_document(&root, None, "Landing v4", &mut doc).unwrap();
        for stem in [
            "landing-v4 (1)",
            "landing-v4 (Jon's conflicted copy 2026-01-02)",
            "orphan (1)",
        ] {
            std::fs::copy(&original, root.join(format!("{stem}.ondin"))).unwrap();
        }
        let lib = Library::open(root.clone());
        let of = |stem: &str| {
            lib.entries
                .iter()
                .find(|e| e.stem == stem)
                .map(|e| lib.conflict(e))
                .unwrap_or_else(|| panic!("no entry {stem:?}"))
        };
        assert_eq!(
            lib.entries.len(),
            4,
            "the fixture is one document and three files beside it"
        );
        assert_eq!(
            of("landing-v4 (1)").as_deref(),
            Some("Landing v4"),
            "a bracketed copy sitting next to its original is a conflict"
        );
        assert_eq!(
            of("orphan (1)"),
            None,
            "and one with no original is somebody's file"
        );
        assert!(
            of("landing-v4 (Jon's conflicted copy 2026-01-02)").is_some(),
            "a client that said so needs no corroboration"
        );
        assert_eq!(of("landing-v4"), None, "the original is not the conflict");

        // ⚠️ The thing the mark is for: without it these two are the same card.
        let names: Vec<String> = lib
            .entries
            .iter()
            .filter(|e| e.stem.starts_with("landing-v4"))
            .map(|e| e.display_name())
            .collect();
        assert_eq!(
            names,
            vec!["Landing v4".to_string(); 3],
            "a conflict copy carries the original's typed name, so the list shows \
             three identical documents and the filename is the only difference"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A conflict in a project folder is not paired with a same-stemmed document
    /// at the root.
    #[test]
    fn a_conflict_is_matched_inside_one_folder_only() {
        let root = temp_root("conflictdir");
        let project = Project {
            id: "p1".into(),
            name: "Kestrel".into(),
            color: PROJECT_COLORS[0].into(),
            folder: Some("kestrel".into()),
            created: 0,
            archived: false,
        };
        let mut doc = blank();
        let at_root = store::file_document(&root, None, "Landing v4", &mut doc).unwrap();
        std::fs::create_dir_all(root.join("kestrel")).unwrap();
        std::fs::copy(&at_root, root.join("kestrel").join("landing-v4 (1).ondin")).unwrap();
        let mut lib = Library::open(root.clone());
        lib.projects.projects.push(project);
        let inside = lib
            .entries
            .iter()
            .find(|e| e.stem == "landing-v4 (1)")
            .expect("the copy inside the project");
        assert_eq!(
            lib.conflict(inside),
            None,
            "the document it would pair with is a folder away, which makes them two \
             unrelated documents rather than a conflict"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A library whose folder cannot be read **keeps this machine's index**, and
    /// says so — the case a scan cannot tell from an empty library on its own.
    ///
    /// ⚠️ **The first assertion is the one with teeth, and it is about data
    /// rather than about a message.** `retain_known` drops every id the scan did
    /// not see; with the folder gone the scan sees none, so an ungated sweep
    /// takes every star on the machine at the first launch with the drive
    /// unplugged — permanently, as soon as anything saves the index afterwards.
    /// The label on the empty state is the cheap half of this feature.
    ///
    /// ⚠️ **Built by hand rather than through `Library::open`**, which calls
    /// `LocalIndex::load` and would read the *developer's real* library index off
    /// this machine — making the fixture whatever happens to be in the cache
    /// directory, and this assertion about nothing.
    ///
    /// Flip-check, run: ungating the sweep (`if false` in place of `if
    /// !readable`, which is the edit that leaves the rest of `refresh` intact)
    /// fails here at the **star** — line one of the assertions, not the flag —
    /// which is the honest site, since the message is a consequence and the star
    /// is the loss. See the sibling test below for the other half of the pair.
    #[test]
    fn a_library_that_cannot_be_read_keeps_its_stars_and_reports_itself() {
        let root = temp_root("offline");
        // Gone after the fixture, which is the unplugged drive: a path that
        // resolves to nothing rather than to an empty folder.
        std::fs::remove_dir_all(&root).unwrap();
        let mut local = LocalIndex::default();
        local.mark_opened("doc-1");
        local.toggle_star("doc-1");
        let mut lib = Library {
            root: root.clone(),
            projects: Default::default(),
            entries: Vec::new(),
            trashed: Vec::new(),
            local,
            projects_unreadable: false,
            root_unavailable: false,
        };

        lib.refresh();
        assert!(
            lib.local.is_starred("doc-1"),
            "a scan that could not look is not evidence that the document is gone"
        );
        assert!(lib.local.opened_at(Some("doc-1")).is_some());
        assert!(
            lib.root_unavailable,
            "and this machine says there should be something here"
        );
        assert!(!lib.may_write(), "so nothing may be created into it");
    }

    /// The same unreadable folder on a machine that has **never** seen this
    /// library is an ordinary fresh install, not a fault.
    ///
    /// ⚠️ **This is the assertion that stops the feature crying wolf**, and it is
    /// why the flag is not simply "the folder could not be read": `~/.ondin` does
    /// not exist until the first document is filed, so every first launch would
    /// otherwise open on a warning about a library the user has not made yet.
    ///
    /// Flip-check, run: dropping `&& !self.local.is_empty()` from `refresh`
    /// leaves the test above **green** — it is about the sweep, and the sweep is
    /// gated on readability alone — and fails here, at `root_unavailable`. Which
    /// is the pair worth having: one test for the data, one for the claim, and
    /// neither covers the other.
    #[test]
    fn an_unreadable_folder_on_a_machine_that_knows_nothing_is_a_fresh_install() {
        let root = temp_root("fresh");
        std::fs::remove_dir_all(&root).unwrap();
        let mut lib = Library {
            root,
            projects: Default::default(),
            entries: Vec::new(),
            trashed: Vec::new(),
            local: LocalIndex::default(),
            projects_unreadable: false,
            root_unavailable: false,
        };
        lib.refresh();
        assert!(!lib.root_unavailable);
        assert!(lib.may_write(), "the first save is what creates the folder");
    }
}
