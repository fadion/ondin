//! Moving a library from one base folder to another.
//!
//! **Nothing here overwrites anything.** A taken `landing-v4.ondin` makes the
//! arrival `landing-v4-1.ondin`; `projects.json` is merged by project id; a version
//! file present at both ends is skipped, being the same document id and timestamp.
//! A migration run twice leaves duplicates rather than being idempotent, which is
//! the right way round: the other failure silently eats work.
//!
//! There is no rollback, by decision (§15 D814): an "undo" would be a decision
//! about everything that arrived, including renamed `.trash` files that now look
//! like they were always there, and it can itself fail partway. The retry is the
//! recovery.
//!
//! `fs::rename` is atomic within a volume and fails across one, the common case
//! when the new base folder is on a synced drive, so `move_file` falls back to
//! copy-and-delete.

use super::history::HISTORY_DIR;
use super::naming;
use super::project::{PROJECTS_FILE, Projects, ProjectsRead};
use super::recovery::RECOVERY_DIR;
use super::scan::{self, DOCUMENT_EXT, TRASH_DIR, VERSIONS_DIR};
use std::path::{Path, PathBuf};

/// What a migration did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Moved {
    /// Every document that moved, as `(where it was, where it is)`.
    ///
    /// The caller re-points an open session through this map: `session.path` is
    /// absolute, so otherwise the next save recreates the document in the
    /// abandoned folder under an id the new library also holds. It is a map and
    /// not a prefix swap because `unique_stem` may rename into a collision
    /// (`notes.ondin` arriving as `notes-1.ondin`). Sidecars are not here; they
    /// are found by id.
    pub paths: Vec<(PathBuf, PathBuf)>,
    /// Version files and trashed documents moved.
    pub sidecars: usize,
    /// Projects added to the destination's `projects.json`.
    pub projects: usize,
    /// Everything that could not be moved, named by source path: a failure means
    /// the file is still where it was (`move_file` deletes last), so the source is
    /// the one the user can go and look at. A bare count could not say which files
    /// or whether they were documents or sidecars.
    pub failed: Vec<PathBuf>,
}

impl Moved {
    /// How many documents moved.
    ///
    /// Derived from `paths`, so the status-line number and the re-point map cannot
    /// disagree.
    pub fn documents(&self) -> usize {
        self.paths.len()
    }

    /// Where `was` ended up, if this migration moved it.
    pub fn destination_of(&self, was: &Path) -> Option<&Path> {
        self.paths
            .iter()
            .find(|(from, _)| from == was)
            .map(|(_, to)| to.as_path())
    }

    /// Note that `what` did not move.
    ///
    /// One spelling, so no site can record a failure without naming it.
    fn fail(&mut self, what: &Path) {
        self.failed.push(what.to_path_buf());
    }

    /// A one-line report for the status bar.
    ///
    /// Names only the first failure; the *Library settings* modal draws the full
    /// list. One name still says which folder to look in and whether a document or
    /// a sidecar was left behind.
    pub fn summary(&self) -> String {
        if let Some(first) = self.failed.first() {
            let name = first
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| first.display().to_string());
            return match self.failed.len() {
                1 => format!(
                    "Moved {} file(s); “{name}” could not be moved",
                    self.documents()
                ),
                n => format!(
                    "Moved {} file(s); “{name}” and {} other(s) could not be moved",
                    self.documents(),
                    n - 1
                ),
            };
        }
        match (self.documents(), self.projects) {
            (0, 0) => "Nothing to move".into(),
            (d, 0) => format!("Moved {d} file(s)"),
            (d, p) => format!("Moved {d} file(s) and {p} project(s)"),
        }
    }
}

/// Move everything the library owns from `from` to `to`.
///
/// A no-op when the two are the same path or `from` does not exist. The caller
/// re-opens the library afterwards; nothing here touches app state.
pub fn relocate(from: &Path, to: &Path) -> Moved {
    let mut moved = Moved::default();
    if from == to || !from.exists() {
        return moved;
    }
    if std::fs::create_dir_all(to).is_err() {
        // The destination root itself. Nothing moved and nothing was going to,
        // so naming the folder is the whole report.
        moved.fail(to);
        return moved;
    }

    // Documents, keeping each one's project folder. `scan` already skips the
    // dot-directories, which are moved separately below and by different rules.
    for entry in scan::scan(from) {
        let Ok(relative) = entry.path.strip_prefix(from) else {
            moved.fail(&entry.path);
            continue;
        };
        let Some(dir) = relative.parent().map(|d| to.join(d)) else {
            moved.fail(&entry.path);
            continue;
        };
        if std::fs::create_dir_all(&dir).is_err() {
            // The *document*, not the folder: what the user asked to move is the
            // file, and the folder that would have held it is an implementation
            // detail of the move.
            moved.fail(&entry.path);
            continue;
        }
        // Keep the name the file already has; re-slugging would erase a
        // sync-conflict mark, which exists only in the filename (a conflict copy
        // carries the original's `meta.id`, so `landing-v4 (1).ondin` would become
        // `landing-v4-1.ondin`, indistinguishable from a hand-named pair, and two
        // entries would share one id across `.versions`, the star, `cover::key`
        // and `recovery::pending`). The fallback covers a stem taken at the
        // destination, as when merging two libraries.
        let taken = |s: &str| dir.join(format!("{s}.{DOCUMENT_EXT}")).exists();
        let stem = match taken(&entry.stem) {
            false => entry.stem.clone(),
            true => naming::unique_stem(&entry.display_name(), &taken),
        };
        let target = dir.join(format!("{stem}.{DOCUMENT_EXT}"));
        match move_file(&entry.path, &target) {
            // Recorded here, where both halves are in hand: `unique_stem` may have
            // renamed the target.
            true => moved.paths.push((entry.path.clone(), target)),
            false => moved.fail(&entry.path),
        }
    }

    // Documents whose filename is not valid Unicode are not `Entry`s, so the loop
    // above cannot see them. They move under their own `OsStr` name, with no
    // `unique_stem` (there is no `&str` to derive one from), so a taken name is a
    // failure, as in `Collision::Stranded`. Moving does not make them listable
    // (see `scan::unnameable`); it puts them where the library now points.
    for path in scan::unnameable(from) {
        let Ok(relative) = path.strip_prefix(from) else {
            moved.fail(&path);
            continue;
        };
        let target = to.join(relative);
        let made = target
            .parent()
            .map(|d| std::fs::create_dir_all(d).is_ok())
            .unwrap_or(false);
        if !made || target.exists() || !move_file(&path, &target) {
            moved.fail(&path);
            continue;
        }
        moved.paths.push((path, target));
    }

    // Version history, undo history, the trash and the crash snapshots: whole trees, merged
    // rather than replaced. `.recovery` must travel too, or an unanswered crash
    // snapshot is stranded silently. The policy per tree is argued at
    // [`Collision`].
    for (dir, collision) in [
        (VERSIONS_DIR, Collision::Identical),
        (HISTORY_DIR, Collision::Identical),
        (TRASH_DIR, Collision::Rename),
        (RECOVERY_DIR, Collision::Stranded),
    ] {
        merge_tree(&from.join(dir), &to.join(dir), collision, &mut moved);
    }

    merge_projects(from, to, &mut moved);
    moved
}

/// What a file already at the destination means, which differs per tree, so
/// `merge_tree` cannot have one answer: skipping a `.trash` collision would
/// strand a document, since two libraries can hold different `notes.ondin`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Collision {
    /// The same file arriving twice, so skipping loses nothing. True of
    /// `.versions`, where a path is `<document id>/<unix seconds>.ondin`, and near
    /// enough of `.history`, where either copy is read only if its digest matches
    /// the document and losing one costs only undo steps.
    Identical,
    /// Different files sharing a name, so the arrival takes a suffix through
    /// [`naming::unique_stem`], as `store::trash` does within one folder.
    Rename,
    /// Cannot be told apart and must not be renamed, so the file stays and the
    /// migration reports it. A `.recovery` stem is the document id, which
    /// `recovery::pending` matches and `recovery::remove` insists on, so a
    /// renamed `<id>-1.ondin` would be offered and then be undiscardable.
    Stranded,
}

/// Move every file under `from` into the same relative place under `to`,
/// resolving a file already at the destination by `collision`.
fn merge_tree(from: &Path, to: &Path, collision: Collision, moved: &mut Moved) {
    let Ok(entries) = std::fs::read_dir(from) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name() else {
            continue;
        };
        let mut target = to.join(name);
        match entry.file_type() {
            Ok(t) if t.is_dir() => merge_tree(&path, &target, collision, moved),
            Ok(_) => {
                if target.exists() {
                    match collision {
                        Collision::Identical => continue,
                        // A failure from the user's side: a file still in the
                        // old folder.
                        Collision::Stranded => {
                            moved.fail(&path);
                            continue;
                        }
                        Collision::Rename => {
                            // An extension that is not text is stranded rather
                            // than dropped: a bare `notes` is never listed,
                            // restored or purged. `None` is no extension;
                            // `Some(None)` is one that is not text.
                            let ext = match path.extension().map(|e| e.to_str()) {
                                None => None,
                                Some(Some(e)) => Some(e),
                                Some(None) => {
                                    moved.fail(&path);
                                    continue;
                                }
                            };
                            // A stem that is not text becomes `""`, which `slug`
                            // turns into its fallback — so the arrival is an
                            // ordinary, listable `untitled.ondin` in the trash.
                            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                            // The extension goes back after the suffix:
                            // `notes-1.ondin`, not `notes.ondin-1`.
                            let named = |s: &str| match ext {
                                Some(e) => to.join(format!("{s}.{e}")),
                                None => to.join(s),
                            };
                            target = named(&naming::unique_stem(stem, &|s| named(s).exists()));
                        }
                    }
                }
                if std::fs::create_dir_all(to).is_err() {
                    moved.fail(&path);
                    continue;
                }
                match move_file(&path, &target) {
                    true => moved.sidecars += 1,
                    false => moved.fail(&path),
                }
            }
            // The file type could not be read, so it is named and skipped rather
            // than guessed at.
            Err(_) => moved.fail(&path),
        }
    }
    // Best-effort tidy: only succeeds if everything was moved out.
    let _ = std::fs::remove_dir(from);
}

/// Merge the source's projects into the destination's `projects.json`.
///
/// Merges by id, adding only what is missing: names can match across different
/// projects or differ for one, so the id decides, and the destination's copy wins.
/// An unreadable destination file is left alone, for `Library::save_projects`'s
/// reason.
fn merge_projects(from: &Path, to: &Path, moved: &mut Moved) {
    let source = match Projects::read(from) {
        ProjectsRead::Loaded(p) => p,
        _ => return,
    };
    let mut dest = match Projects::read(to) {
        ProjectsRead::Loaded(p) => p,
        ProjectsRead::Missing => Projects::default(),
        ProjectsRead::Unreadable => {
            moved.fail(&to.join(PROJECTS_FILE));
            return;
        }
    };
    for p in source.projects {
        if dest.projects.iter().any(|d| d.id == p.id) {
            continue;
        }
        dest.projects.push(p);
        moved.projects += 1;
    }
    if dest.write(to).is_err() {
        moved.fail(&to.join(PROJECTS_FILE));
        return;
    }
    let _ = std::fs::remove_file(from.join(PROJECTS_FILE));
}

/// Move one file, falling back to copy-and-delete across volumes.
///
/// Returns whether the file is now at `to`. A copy that succeeds and a delete that
/// fails still counts as moved: the data is safe, and the leftover is a stale
/// duplicate.
fn move_file(from: &Path, to: &PathBuf) -> bool {
    if std::fs::rename(from, to).is_ok() {
        return true;
    }
    if std::fs::copy(from, to).is_err() {
        return false;
    }
    let _ = std::fs::remove_file(from);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::project::{PROJECT_COLORS, Project};
    use crate::library::store;
    use ondin_core::{Document, IdSource};

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ondin-reloc-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn blank() -> Document {
        let mut ids = IdSource::new(0xD0C);
        Document::new(ids.mint())
    }

    fn project(id: &str, name: &str) -> Project {
        Project {
            id: id.into(),
            name: name.into(),
            color: PROJECT_COLORS[0].into(),
            folder: Some(naming::slug(name)),
            created: 0,
            archived: false,
        }
    }

    /// A document that cannot be moved is named, left where it was, and reported,
    /// while the rest of the migration carries on.
    ///
    /// A *file* put where the project's folder must go makes `create_dir_all` fail
    /// for exactly one document, deterministically on every platform; the loose
    /// document is the control that the migration continued.
    ///
    /// flip: `Moved::summary`'s failure arm deleted → fails at "the status line
    /// says which"; a fail on `dir` instead of `entry.path` → fails the named path.
    #[test]
    fn a_document_that_cannot_be_moved_is_named_and_left_where_it_was() {
        let from = temp("failed-move-from");
        let to = temp("failed-move-to");
        let kestrel = project("p-1", "Kestrel");
        Projects {
            version: 1,
            projects: vec![kestrel.clone()],
        }
        .write(&from)
        .unwrap();

        let mut doc = blank();
        let stuck = store::file_document(&from, Some(&kestrel), "Landing v4", &mut doc).unwrap();
        let mut loose = blank();
        store::file_document(&from, None, "Loose sketch", &mut loose).unwrap();

        // A *file* where the project's folder has to be, so `create_dir_all`
        // refuses for this one document and for nothing else.
        std::fs::write(to.join("kestrel"), b"not a directory").unwrap();

        let moved = relocate(&from, &to);

        assert_eq!(
            moved.failed,
            vec![stuck.clone()],
            "the one that could not move is named, and only it: {moved:?}"
        );
        assert!(
            stuck.exists(),
            "and it is still where it was — a failed move must not lose the file"
        );
        assert_eq!(
            moved.documents(),
            1,
            "the control: the rest of the migration ran"
        );
        assert!(to.join("loose-sketch.ondin").exists());
        let line = moved.summary();
        assert!(
            line.contains("landing-v4.ondin"),
            "the status line says which: {line:?}"
        );

        let _ = std::fs::remove_dir_all(&from);
        let _ = std::fs::remove_dir_all(&to);
    }

    #[test]
    fn a_whole_library_moves_with_its_projects_history_and_trash() {
        let from = temp("from");
        let to = temp("to");
        let kestrel = project("p-1", "Kestrel");
        Projects {
            version: 1,
            projects: vec![kestrel.clone()],
        }
        .write(&from)
        .unwrap();

        let mut doc = blank();
        store::file_document(&from, Some(&kestrel), "Landing v4", &mut doc).unwrap();
        let mut loose = blank();
        store::file_document(&from, None, "Loose sketch", &mut loose).unwrap();
        let entry = scan::scan(&from)
            .into_iter()
            .find(|e| e.display_name() == "Landing v4")
            .unwrap();
        store::pin_version(&from, &entry).unwrap();
        let trashme = scan::scan(&from)
            .into_iter()
            .find(|e| e.display_name() == "Loose sketch")
            .unwrap();
        store::trash(&from, &trashme).unwrap();

        let moved = relocate(&from, &to);
        assert_eq!(moved.documents(), 1, "{moved:?}");
        assert_eq!(moved.projects, 1, "{moved:?}");
        assert!(moved.failed.is_empty(), "{moved:?}");

        // The document arrived, inside its project's folder.
        assert!(to.join("kestrel").join("landing-v4.ondin").exists());
        // Its history came with it, under the same id.
        let id = doc.meta().id.clone().unwrap();
        assert_eq!(store::versions(&to, &id).len(), 1);
        // And so did the trash.
        assert_eq!(store::trashed(&to).len(), 1);
        // The project list merged into the destination.
        assert!(Projects::read(&to).or_empty().get("p-1").is_some());
        // The source is empty of documents.
        assert!(scan::scan(&from).is_empty());

        let _ = std::fs::remove_dir_all(&from);
        let _ = std::fs::remove_dir_all(&to);
    }

    /// A document whose slug is taken at the destination arrives beside it, not
    /// on top of it.
    ///
    /// flip: `unique_stem` replaced by a bare `slug` → the count stays green and
    /// the file-count assertion fails.
    #[test]
    fn a_name_that_is_taken_at_the_destination_does_not_overwrite_it() {
        let from = temp("clash-from");
        let to = temp("clash-to");
        let mut incoming = blank();
        store::file_document(&from, None, "Landing v4", &mut incoming).unwrap();
        let mut existing = blank();
        store::file_document(&to, None, "Landing v4", &mut existing).unwrap();

        let moved = relocate(&from, &to);
        assert_eq!(moved.documents(), 1);
        assert_eq!(
            scan::scan(&to).len(),
            2,
            "both documents must survive the move"
        );
        let ids: Vec<Option<String>> = scan::scan(&to).iter().map(|e| e.meta.id.clone()).collect();
        assert!(ids.contains(&incoming.meta().id));
        assert!(ids.contains(&existing.meta().id));

        let _ = std::fs::remove_dir_all(&from);
        let _ = std::fs::remove_dir_all(&to);
    }

    /// A sync-conflict copy keeps its filename, and so its mark, across the move.
    ///
    /// The mark exists only in the filename and the copy shares the original's
    /// `meta.id`, so re-slugging would turn it into `landing-v4-1.ondin`. The mark
    /// is asserted before the move too, so the fixture cannot pass by never having
    /// been marked.
    ///
    /// flip: the stem re-derived unconditionally → fails at "the conflict copy
    /// keeps its filename".
    #[test]
    fn a_conflict_copy_keeps_its_mark_across_a_move() {
        let from = temp("mark-from");
        let to = temp("mark-to");
        let mut original = blank();
        store::file_document(&from, None, "Landing v4", &mut original).unwrap();
        // The copy a sync client leaves: the same bytes, hence the same `meta.id`
        // and the same display name, under a filename only the client writes.
        let copy = from.join(format!("landing-v4 (1).{DOCUMENT_EXT}"));
        std::fs::copy(from.join(format!("landing-v4.{DOCUMENT_EXT}")), &copy).unwrap();

        let marked_before = scan::scan(&from)
            .iter()
            .filter(|e| scan::conflict_marker(&e.stem).is_some())
            .count();
        assert_eq!(
            marked_before, 1,
            "the fixture must actually build a marked conflict copy"
        );

        let moved = relocate(&from, &to);
        assert_eq!(moved.documents(), 2);

        let after = scan::scan(&to);
        assert!(
            after.iter().any(|e| e.stem == "landing-v4 (1)"),
            "the conflict copy keeps its filename: {:?}",
            after.iter().map(|e| e.stem.clone()).collect::<Vec<_>>()
        );
        assert_eq!(
            after
                .iter()
                .filter(|e| scan::conflict_marker(&e.stem).is_some())
                .count(),
            1,
            "and is still the one the dashboard marks"
        );
        assert!(
            after.iter().any(|e| e.stem == "landing-v4"),
            "the original keeps its own name too"
        );

        let _ = std::fs::remove_dir_all(&from);
        let _ = std::fs::remove_dir_all(&to);
    }

    /// An unanswered crash snapshot travels with the library (§15 D377). Its
    /// absence would be silent: the next launch reads the new root and never asks.
    ///
    /// flip: `RECOVERY_DIR` dropped from `relocate`'s tree loop → fails at the
    /// second assertion.
    #[test]
    fn an_unanswered_crash_snapshot_moves_with_the_library() {
        use super::super::recovery::{RECOVERY_DIR, pending};
        let from = temp("rec-from");
        let to = temp("rec-to");
        let mut doc = blank();
        store::file_document(&from, None, "Landing v4", &mut doc).unwrap();
        // Written by hand rather than through `recovery::write`, so the fixture
        // does not depend on the thing being moved: this is about `relocate`.
        std::fs::create_dir_all(from.join(RECOVERY_DIR)).unwrap();
        let bytes = ondin_core::io::save(&doc).unwrap();
        std::fs::write(from.join(RECOVERY_DIR).join("orphan-key.ondin"), &bytes).unwrap();

        let moved = relocate(&from, &to);
        assert_eq!(
            moved.documents(),
            1,
            "one document, and the snapshot is not one"
        );
        assert!(
            to.join(RECOVERY_DIR).join("orphan-key.ondin").exists(),
            "a snapshot left in the old folder is a question nobody ever asks again"
        );
        assert_eq!(
            pending(&to, &scan::scan(&to))
                .iter()
                .map(|p| p.key.clone())
                .collect::<Vec<_>>(),
            vec!["orphan-key".to_string()],
            "and the next launch at the new root can still find it"
        );

        let _ = std::fs::remove_dir_all(&from);
        let _ = std::fs::remove_dir_all(&to);
    }

    /// Two libraries each holding a different trashed `notes.ondin` keep both
    /// after the move. The file is asserted before the count, since the count only
    /// makes a loss findable.
    ///
    /// flip: `.trash`'s policy set to `Collision::Identical` → fails at "both
    /// trashed documents are in the library that was moved into", while
    /// `moved.failed` stays empty.
    #[test]
    fn two_libraries_trashing_the_same_name_keep_both_documents() {
        let from = temp("trash-from");
        let to = temp("trash-to");
        for (root, body) in [(&from, "source"), (&to, "destination")] {
            let dir = root.join(TRASH_DIR);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("notes.ondin"), body).unwrap();
        }

        let moved = relocate(&from, &to);
        // Read as text so a failure names the document that went missing rather
        // than printing two arrays of bytes.
        let mut bodies: Vec<String> = std::fs::read_dir(to.join(TRASH_DIR))
            .unwrap()
            .flatten()
            .map(|e| std::fs::read_to_string(e.path()).unwrap())
            .collect();
        bodies.sort();
        assert_eq!(
            bodies,
            vec!["destination".to_string(), "source".to_string()],
            "both trashed documents are in the library that was moved into"
        );
        assert_eq!(moved.sidecars, 1, "and the arrival is counted as moved");
        assert!(moved.failed.is_empty(), "not as a failure — it did move");
        assert!(
            !from.join(TRASH_DIR).join("notes.ondin").exists(),
            "nothing is left behind in the folder the library left"
        );

        let _ = std::fs::remove_dir_all(&from);
        let _ = std::fs::remove_dir_all(&to);
    }

    /// A colliding snapshot stays where it is and is reported, where a trashed
    /// document would rename: its stem is its document id, so renaming would
    /// leave a snapshot the user is offered and cannot discard. Unlike a colliding
    /// version it is not the same file, hence counted rather than skipped.
    #[test]
    fn a_snapshot_that_would_collide_is_left_behind_and_reported() {
        let from = temp("recov-from");
        let to = temp("recov-to");
        for (root, body) in [(&from, b"source"), (&to, b"destin")] {
            let dir = root.join(RECOVERY_DIR);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("doc-1.ondin"), body).unwrap();
        }

        let moved = relocate(&from, &to);
        assert_eq!(
            std::fs::read(from.join(RECOVERY_DIR).join("doc-1.ondin")).unwrap(),
            b"source",
            "the snapshot is still where it was"
        );
        // The path and not a count, which a migration stranding the wrong file
        // would satisfy.
        assert_eq!(
            moved.failed,
            vec![from.join(RECOVERY_DIR).join("doc-1.ondin")],
            "and the migration names the snapshot it left behind"
        );
        assert!(
            moved.summary().contains("could not be moved"),
            "in the line the user reads: {:?}",
            moved.summary()
        );

        let _ = std::fs::remove_dir_all(&from);
        let _ = std::fs::remove_dir_all(&to);
    }

    /// The same version at both ends is the same version, so it is skipped
    /// rather than duplicated under a new name.
    #[test]
    fn an_identical_version_file_is_not_moved_twice() {
        let from = temp("ver-from");
        let to = temp("ver-to");
        let dir_from = from.join(VERSIONS_DIR).join("doc-1");
        let dir_to = to.join(VERSIONS_DIR).join("doc-1");
        std::fs::create_dir_all(&dir_from).unwrap();
        std::fs::create_dir_all(&dir_to).unwrap();
        std::fs::write(dir_from.join("100.ondin"), b"a").unwrap();
        std::fs::write(dir_to.join("100.ondin"), b"b").unwrap();
        std::fs::write(dir_from.join("200.ondin"), b"c").unwrap();

        let moved = relocate(&from, &to);
        assert_eq!(moved.sidecars, 1, "only the one the destination lacked");
        assert_eq!(store::versions(&to, "doc-1").len(), 2);
        assert_eq!(
            std::fs::read(dir_to.join("100.ondin")).unwrap(),
            b"b",
            "the destination's copy is the one that survives"
        );

        let _ = std::fs::remove_dir_all(&from);
        let _ = std::fs::remove_dir_all(&to);
    }

    /// Two projects with the same id are one project; the destination's name for
    /// it wins.
    #[test]
    fn projects_merge_by_id_and_the_destination_wins() {
        let from = temp("proj-from");
        let to = temp("proj-to");
        Projects {
            version: 1,
            projects: vec![project("p-1", "Kestrel"), project("p-2", "Atlas")],
        }
        .write(&from)
        .unwrap();
        Projects {
            version: 1,
            projects: vec![project("p-1", "Kestrel renamed here")],
        }
        .write(&to)
        .unwrap();

        let moved = relocate(&from, &to);
        assert_eq!(moved.projects, 1, "only Atlas was new");
        let merged = Projects::read(&to).or_empty();
        assert_eq!(merged.projects.len(), 2);
        assert_eq!(merged.get("p-1").unwrap().name, "Kestrel renamed here");
        assert_eq!(merged.get("p-2").unwrap().name, "Atlas");

        let _ = std::fs::remove_dir_all(&from);
        let _ = std::fs::remove_dir_all(&to);
    }

    /// An unreadable destination `projects.json` stops the merge rather than
    /// replacing it — `Library::save_projects`'s rule, reached from here.
    #[test]
    fn an_unreadable_destination_project_list_is_left_alone() {
        let from = temp("proj-broken-from");
        let to = temp("proj-broken-to");
        Projects {
            version: 1,
            projects: vec![project("p-1", "Kestrel")],
        }
        .write(&from)
        .unwrap();
        let broken = b"{ \"projects\": [ ".as_slice();
        std::fs::write(to.join(PROJECTS_FILE), broken).unwrap();

        let moved = relocate(&from, &to);
        assert_eq!(moved.projects, 0);
        assert_eq!(
            moved.failed,
            vec![to.join(PROJECTS_FILE)],
            "and the unreadable file is named rather than merely counted"
        );
        assert_eq!(std::fs::read(to.join(PROJECTS_FILE)).unwrap(), broken);

        let _ = std::fs::remove_dir_all(&from);
        let _ = std::fs::remove_dir_all(&to);
    }

    /// A document whose filename is not valid Unicode travels with the library
    /// and is counted (§15 D809). `scan::unnameable_doc_name` builds the name per
    /// platform; the ordinary document is the control that the scan loop still ran.
    ///
    /// flip: the `for path in scan::unnameable(from)` loop removed → fails at
    /// "and it arrived", asserted before the count so the message names the loss.
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "APFS refuses a name that is not UTF-8, so the case cannot arise (§15 D967)"
    )]
    fn a_document_with_a_non_unicode_filename_is_carried_across_and_counted() {
        let from = temp("odd-name-from");
        let to = temp("odd-name-to");
        store::file_document(&from, None, "Landing v4", &mut blank()).unwrap();
        let odd = from.join(scan::unnameable_doc_name());
        std::fs::write(&odd, b"{}").expect("the OS accepts this name");

        let moved = relocate(&from, &to);

        let arrived = to.join(scan::unnameable_doc_name());
        assert!(
            arrived.is_file(),
            "and it arrived — under the name it already had, since there is no \
             `&str` here to derive one from"
        );
        assert!(!odd.exists(), "and it is not in two places");
        assert_eq!(
            moved.documents(),
            2,
            "both documents are counted: {:?}",
            moved.paths
        );
        assert!(
            moved.failed.is_empty(),
            "nothing was stranded: {:?}",
            moved.failed
        );
        assert!(
            to.join("landing-v4.ondin").is_file(),
            "control: the ordinary document went too"
        );

        let _ = std::fs::remove_dir_all(&from);
        let _ = std::fs::remove_dir_all(&to);
    }

    /// A folder name the OS accepts and `OsStr::to_str` refuses — the stem of
    /// `scan::unnameable_doc_name`, so the one platform fact stays spelled once.
    fn unnameable_folder_name() -> std::ffi::OsString {
        Path::new(&scan::unnameable_doc_name())
            .file_stem()
            .expect("the doc name has a stem")
            .to_os_string()
    }

    /// A document in a project folder whose name is not text is moved and counted,
    /// and a hidden such folder is still hidden (§15 D848). The hidden folder
    /// controls the dot rule, which applies to the lossy name.
    ///
    /// flip: the directory arm's descent disabled → fails at "the walk reaches the
    /// folder whose name is not text"; its `!hidden` term removed → fails at "the
    /// hidden one is not a project".
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "APFS refuses a name that is not UTF-8, so the case cannot arise (§15 D967)"
    )]
    fn a_document_in_a_non_unicode_project_folder_is_carried_across_and_counted() {
        let from = temp("odd-folder-from");
        let to = temp("odd-folder-to");
        store::file_document(&from, None, "Landing v4", &mut blank()).unwrap();
        let odd = from.join(unnameable_folder_name());
        std::fs::create_dir_all(&odd).expect("the OS accepts this name");
        let mut doc = ondin_core::Document::new(IdSource::new(0xDB).mint());
        let filed = store::file_document(&from, None, "Pricing table", &mut doc).unwrap();
        std::fs::rename(&filed, odd.join("pricing-table.ondin")).unwrap();
        let mut hidden_name = std::ffi::OsString::from(".");
        hidden_name.push(unnameable_folder_name());
        let hidden = from.join(hidden_name);
        std::fs::create_dir_all(&hidden).unwrap();
        // A real document, or the scan would skip it for not being one and the
        // control would be about nothing.
        let mut stray = ondin_core::Document::new(IdSource::new(0xDC).mint());
        let filed = store::file_document(&from, None, "Stray", &mut stray).unwrap();
        std::fs::rename(&filed, hidden.join("stray.ondin")).unwrap();

        let listed: Vec<String> = scan::scan(&from).into_iter().map(|e| e.stem).collect();
        assert!(
            listed.iter().any(|s| s == "pricing-table"),
            "the walk reaches the folder whose name is not text: {listed:?}"
        );
        assert!(
            !listed.iter().any(|s| s == "stray"),
            "the hidden one is not a project: {listed:?}"
        );

        let moved = relocate(&from, &to);
        let arrived = to
            .join(unnameable_folder_name())
            .join("pricing-table.ondin");
        assert!(
            arrived.is_file(),
            "and it arrived, in a folder of the same name"
        );
        assert!(
            !odd.join("pricing-table.ondin").exists(),
            "and it is not in two places"
        );
        assert_eq!(moved.documents(), 2, "both counted: {:?}", moved.paths);
        assert!(
            to.join("landing-v4.ondin").is_file(),
            "control: the ordinary document went too"
        );

        let _ = std::fs::remove_dir_all(&from);
        let _ = std::fs::remove_dir_all(&to);
    }

    /// A destination already holding the unnameable file's name is a failure, and
    /// the file there is untouched. `target.exists()` is the only guard before
    /// `move_file`, which overwrites on both arms.
    ///
    /// flip: the `target.exists()` term removed → fails at "the file already there
    /// is untouched".
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "APFS refuses a name that is not UTF-8, so the case cannot arise (§15 D967)"
    )]
    fn an_unnameable_file_does_not_overwrite_one_already_at_the_destination() {
        let from = temp("odd-collide-from");
        let to = temp("odd-collide-to");
        let odd = from.join(scan::unnameable_doc_name());
        std::fs::write(&odd, b"the one moving").unwrap();
        let there = to.join(scan::unnameable_doc_name());
        std::fs::write(&there, b"the one already there").unwrap();

        let moved = relocate(&from, &to);
        assert_eq!(
            std::fs::read(&there).unwrap(),
            b"the one already there",
            "the file already there is untouched"
        );
        assert!(
            odd.exists(),
            "and the one that could not move is still at home"
        );
        assert_eq!(
            moved.failed,
            vec![odd.clone()],
            "and it is named as stranded"
        );

        let _ = std::fs::remove_dir_all(&from);
        let _ = std::fs::remove_dir_all(&to);
    }

    /// A `.trash` collision on a name that is not text: a bad stem is renamed to
    /// the listable fallback `untitled.ondin` (`slug("")`), a bad extension is
    /// stranded (§15 D848).
    ///
    /// flip: the `Some(None)` arm restored to dropping the extension → fails at "a
    /// bad extension is not dropped".
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "APFS refuses a name that is not UTF-8, so the case cannot arise (§15 D967)"
    )]
    fn a_trash_collision_on_a_name_that_is_not_text_keeps_the_file_findable() {
        let from = temp("odd-trash-from");
        let to = temp("odd-trash-to");
        let (from_trash, to_trash) = (from.join(TRASH_DIR), to.join(TRASH_DIR));
        std::fs::create_dir_all(&from_trash).unwrap();
        std::fs::create_dir_all(&to_trash).unwrap();
        // A stem that is not text.
        let odd = from_trash.join(scan::unnameable_doc_name());
        std::fs::write(&odd, b"odd stem").unwrap();
        std::fs::write(to_trash.join(scan::unnameable_doc_name()), b"already there").unwrap();
        // An extension that is not text.
        let mut bad_ext = std::ffi::OsString::from("notes.");
        bad_ext.push(unnameable_folder_name());
        let odd_ext = from_trash.join(&bad_ext);
        std::fs::write(&odd_ext, b"odd extension").unwrap();
        std::fs::write(to_trash.join(&bad_ext), b"already there").unwrap();

        let moved = relocate(&from, &to);
        assert_eq!(
            std::fs::read(to_trash.join("untitled.ondin"))
                .ok()
                .as_deref(),
            Some(&b"odd stem"[..]),
            "a bad stem arrives under the fallback name, which the trash lists"
        );
        assert!(
            !to_trash.join("notes").exists(),
            "a bad extension is not dropped"
        );
        assert!(odd_ext.exists(), "that file stays where it was");
        assert_eq!(
            moved.failed,
            vec![odd_ext.clone()],
            "and is named as stranded"
        );

        let _ = std::fs::remove_dir_all(&from);
        let _ = std::fs::remove_dir_all(&to);
    }

    #[test]
    fn moving_a_library_onto_itself_or_from_nowhere_does_nothing() {
        let dir = temp("noop");
        assert_eq!(relocate(&dir, &dir), Moved::default());
        let missing = std::env::temp_dir().join("ondin-reloc-does-not-exist");
        let _ = std::fs::remove_dir_all(&missing);
        assert_eq!(relocate(&missing, &dir), Moved::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
