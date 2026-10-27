//! The library as the app holds it for a frame: the root, the projects, the
//! entries, and the sort the dashboard is showing them in.
//!
//! **Rebuilt from disk rather than maintained.** Every write in
//! [`super::store`] is followed by a re-scan, not a patch: a patched list drifts
//! the first time something other than this app changes the synced base folder.
//!
//! ⚠️ **A scan is a real cost and must not be per-frame.** [`Library::refresh`]
//! opens every document's prefix. The dashboard calls it when entered and after a
//! write, and on nothing else.

use super::cache::LocalIndex;
use super::project::{Project, Projects, ProjectsRead};
use super::scan::{self, Entry};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// How the dashboard orders what it shows.
///
/// The four the dashboard's sort control cycles. Stored in preferences by
/// [`Sort::id`] rather than by index, so inserting one cannot change what an
/// existing install opens on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Edited,
    Name,
    Created,
    Size,
}

impl Sort {
    /// The order the control cycles through.
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
    /// The resolved base folder, resolved once so a setting edited mid-session
    /// cannot make two halves of a frame disagree.
    pub root: PathBuf,
    pub projects: Projects,
    /// Every document found by the last [`Self::refresh`].
    pub entries: Vec<Entry>,
    /// The trash's listing as of the last [`Self::refresh_trash`] (§15 D728).
    ///
    /// Held because reading it is a scan and the sidebar asks every frame:
    /// `store::trashed` is `scan::scan_dir`, which opens every file in `.trash`,
    /// and `dashboard_sidebar` paints the *Trash* count on every nav.
    ///
    /// ⚠️ Private, unlike [`Self::entries`]: [`Self::trashed`] is the read, and the
    /// only way to move this is a refresh, which keeps the invalidation
    /// enumerable.
    trashed: Vec<Entry>,
    /// Per-machine facts — the *last opened* times behind the Recent list.
    pub local: LocalIndex,
    /// Set when `projects.json` exists but could not be read.
    ///
    /// ⚠️ A latch on writing: the in-memory list is empty but the file is not, so
    /// creating a project would overwrite the real ones. Every writer checks it;
    /// see [`Self::may_write_projects`].
    pub projects_unreadable: bool,
    /// Set when the base folder could not be listed **and** this machine has
    /// recorded documents in it — an unplugged drive, a sync folder that has not
    /// mounted, a library renamed in Explorer (§15 D384).
    ///
    /// ⚠️ The second half makes this a fact: a fresh install has no base folder
    /// until the first write, so an unreadable root alone is ordinary. A non-empty
    /// [`LocalIndex`] says this machine has seen documents here, which makes an
    /// empty scan a disagreement.
    ///
    /// A latch on writing as well as a message: creating a document while the
    /// real library is offline makes a second library at the same path.
    pub root_unavailable: bool,
}

impl Library {
    /// Open the library at `root` and scan it.
    pub fn open(root: PathBuf) -> Self {
        Self::open_with_index(root, LocalIndex::load())
    }

    /// [`Self::open`], with the per-machine index handed in rather than read.
    ///
    /// For a change of base folder, which already holds the index (§15 D848). The
    /// index is per machine and keyed by document id, so the in-memory copy is
    /// what `load` would read; the `unreadable` latch stays set until relaunch,
    /// the conservative direction. It also lets a test reach
    /// [`Self::root_unavailable`] through the app's constructor, since `load`
    /// answers an empty index under `cfg!(test)` (§15 D807).
    ///
    /// A parameter, not the constructor-set flag `cache::index_is_reachable`
    /// rejects: a value passed in cannot be silently discarded by a later `open`.
    pub fn open_with_index(root: PathBuf, local: LocalIndex) -> Self {
        let read = Projects::read(&root);
        let projects_unreadable = matches!(read, ProjectsRead::Unreadable);
        let mut lib = Self {
            root,
            projects: read.or_empty(),
            entries: Vec::new(),
            // Both filled by the `refresh` below.
            trashed: Vec::new(),
            local,
            projects_unreadable,
            root_unavailable: false,
        };
        lib.refresh();
        lib
    }

    /// Re-read the folder. See the module note on why this is not per-frame.
    ///
    /// ⚠️ **The sweep is gated on the folder having been readable.**
    /// [`LocalIndex::retain_known`] drops every id the scan did not see, which is
    /// right after a delete and catastrophic after an unplugged drive: every star
    /// on the machine would be lost once the index is next saved. An empty list is
    /// not evidence the documents are gone; only a folder that was listed is.
    ///
    /// The read is a second syscall rather than part of [`scan::scan`]'s result,
    /// since every other error that walk meets is right to drop.
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
    /// Cheap, and the only way to ask: `store::trashed` is a scan; see the field.
    pub fn trashed(&self) -> &[Entry] {
        &self.trashed
    }

    /// Re-read the trash alone (§15 D728).
    ///
    /// The narrow half of [`Self::refresh`], for callers that moved a file into or
    /// out of `.trash` and changed nothing else (`purge_trash` and the permanent
    /// delete). Trashing, `delete_project`'s member sweep and `Act::Restore` move
    /// files between the library and `.trash`, so they need a full `refresh`.
    ///
    /// 🚨 **This is the invalidation point, and the whole risk of caching the
    /// trash.** `store::trash`, `store::purge_trash`, `relocate` and `trash_or_purge`'s
    /// `Nav::Trash` arm (a bare `fs::remove_file` in a panel) all write `.trash`;
    /// the last is invisible to a grep for `store::`. Enumerate the writers
    /// rather than trusting a list, from the repository root:
    ///
    /// ```text
    /// grep -rn 'store::trash(\|store::purge_trash(' crates/ondin-app/src --include=*.rs
    /// grep -rn 'remove_file\|fs::rename' crates/ondin-app/src --include=*.rs
    /// ```
    ///
    /// The second matters: a writer that reaches `.trash` without naming `store`
    /// is the one this cache cannot survive. (§15 D728)
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
    /// False only when [`Self::root_unavailable`]. Every write is a
    /// `create_dir_all` away from resurrecting the path, so filing into an offline
    /// library would succeed into a brand-new empty folder and leave the sync
    /// client two libraries to reconcile.
    ///
    /// Not the same question as [`Self::may_write_projects`] (one unparseable
    /// file); either can be true without the other.
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
    /// `None` covers both "no project" and an id nothing here knows (a document
    /// synced before its `projects.json`). Treating it as unfiled loses nothing:
    /// it joins its project once the list syncs.
    pub fn project_of(&self, entry: &Entry) -> Option<&Project> {
        self.projects.get(entry.meta.project.as_deref()?)
    }

    /// Whether this document is a sync client's conflict copy, and the name of
    /// the document it is a copy of.
    ///
    /// [`scan::conflict_marker`] reads one filename, and a
    /// [`scan::Conflict::Suspected`] answer holds only if the original sits beside
    /// it — a question about the list. `landing-v4 (1)` next to `landing-v4` is a
    /// sync artefact; alone it is somebody's file.
    ///
    /// ⚠️ **Same directory, not merely the same stem**: a project is a folder
    /// (`super::project`), so a root document and a same-named copy inside
    /// *Kestrel* are unrelated. Compared case-insensitively, like the filesystem.
    ///
    /// Linear over `entries`, affordable because only a filename that already
    /// carries a suspected marker reaches the scan.
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
    /// Every sort is broken by name and then by stem, so the order is total; ties
    /// (a bulk import shares a created second) would otherwise follow directory
    /// order and shuffle between visits.
    pub fn sort_entries(&self, entries: &mut [Entry], sort: Sort) {
        entries.sort_by(|a, b| {
            let primary = match sort {
                // Descending: largest or most recent first.
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
    /// Only documents actually opened here: a library synced onto a new machine
    /// has a full *All files* and an empty *Recent*.
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
        // An unknown key degrades rather than failing.
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
    /// Not written to disk: a sort test over a temp folder is partly answered by
    /// the filesystem (see `a_tied_sort_falls_back_to_the_name`).
    fn entry(name: &str, stem: &str, modified: u64, created: u64, size: u64) -> Entry {
        Entry {
            path: PathBuf::from(format!("/lib/{stem}.ondin")),
            stem: stem.into(),
            meta: ondin_core::meta::DocumentMeta {
                id: Some(stem.into()),
                name: Some(name.into()),
                project: None,
                created: Some(created),
                kit: None,
            },
            modified,
            size,
            unread: false,
        }
    }

    /// A tied sort is still a total order: documents created in the same second
    /// are ordered by name.
    ///
    /// Entries are constructed, not scanned (`read_dir` on NTFS is already
    /// filename-sorted and would satisfy the assertion), and the stems run
    /// opposite to the names so a fall-through to the stem also fails.
    ///
    /// flip: dropping the `then_with` chain, or only the display-name link, fails
    /// at the names assertion.
    #[test]
    fn a_tied_sort_falls_back_to_the_name() {
        let lib = Library::open(temp_root("ties"));
        // Stems ascend a, b, c; names descend Zebra, Mango, Apple; input is a third order.
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
        // Byte order would give "Alpha", "Gamma", "beta"; the stems run reverse so
        // the fall-through cannot produce this by accident.
        assert_eq!(names, vec!["Alpha", "beta", "Gamma"]);
        let _ = std::fs::remove_dir_all(&lib.root);
    }

    /// The descending sorts, which the tie-break tests (tied on the primary key)
    /// do not exercise.
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

    /// A `projects.json` that cannot be parsed is not overwritten by the empty
    /// list that failure produced.
    ///
    /// flip: making `save_projects` write unconditionally leaves the first two
    /// assertions green and fails the last.
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
    /// The fixture is copied by hand, not filed through `store`, since nothing in
    /// the app can produce these filenames; a copy carries the original's
    /// metadata, so only the stem differs.
    ///
    /// flip: making `Conflict::Suspected` return `Some` without consulting
    /// `entries` fails at the second assertion (the lone `orphan (1)` file marked
    /// as a copy); `a_conflict_is_matched_inside_one_folder_only` fails on the
    /// same flip.
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

        // What the mark is for: without it these are the same card.
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

    /// A library whose folder cannot be read keeps this machine's index, and says
    /// so. The star assertion is the one with teeth: an ungated sweep would drop
    /// every star at a launch with the drive unplugged.
    ///
    /// Built by hand, not through `Library::open`, which calls `LocalIndex::load`
    /// and would read the developer's real index.
    ///
    /// flip: `if false` in place of `if !readable` in `refresh` fails at the
    /// star assertion, not the flag. See the sibling test for the other half.
    #[test]
    fn a_library_that_cannot_be_read_keeps_its_stars_and_reports_itself() {
        let root = temp_root("offline");
        // The unplugged drive: a path that resolves to nothing.
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
    /// Stops the feature crying wolf: `~/.ondin` does not exist until the first
    /// document is filed, so every first launch would otherwise warn.
    ///
    /// flip: dropping `&& !self.local.is_empty()` from `refresh` leaves the test
    /// above green (it is about the sweep) and fails here, at `root_unavailable`.
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
