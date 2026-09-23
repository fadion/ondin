//! Moving a library from one base folder to another.
//!
//! **What "change the base folder" means without this.** Nothing is lost — the
//! documents stay exactly where they were and the app simply looks somewhere
//! else — but the user sees an empty library and has to move the files by hand,
//! which for a library with version history and a trash means knowing about two
//! dot-directories they were never meant to think about. This is the button that
//! does it for them.
//!
//! ⚠️ **Nothing here overwrites anything.** A destination that already holds a
//! `landing-v4.ondin` gets `landing-v4-1.ondin`; a `projects.json` at both ends
//! is *merged* by project id rather than replaced; a version file that exists at
//! both ends is skipped, because the same document id and the same timestamp is
//! the same version. The cost is that a migration run twice leaves duplicates
//! rather than being idempotent — which is the right way round, since the other
//! failure silently eats work.
//!
//! 🚨 **And there is no rollback, by decision** (§15 D814). *Undo this migration*
//! was the third of the three asks §15 D620 left with the maintainer — the other
//! two, the full list of what was stranded and a *Try again*, landed at §15 D810 —
//! and it is ruled a **non-goal**. The reason is the paragraph above: nothing here
//! overwrites anything, so an "undo" is not a reverse of a known operation but a
//! decision about everything that *did* arrive, including a `.trash` file renamed
//! on the way in by `Collision::Rename` and now indistinguishable from a document
//! that was always there. A reverse migration can also partly fail, which leaves a
//! second and worse partial state. **The retry is the recovery**, and it is
//! reachable now, which is what the ask was really about.
//!
//! **Move, with copy-and-delete as the fallback.** `fs::rename` is instant and
//! atomic within a volume and simply fails across one, which is the common case
//! here: the whole reason to change the base folder is usually to put it on a
//! synced drive that is not the system disk.

use super::naming;
use super::project::{PROJECTS_FILE, Projects, ProjectsRead};
use super::recovery::RECOVERY_DIR;
use super::scan::{self, DOCUMENT_EXT, TRASH_DIR, VERSIONS_DIR};
use std::path::{Path, PathBuf};

/// What a migration did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Moved {
    /// Every document that moved, as `(where it was, where it is)` (§15 D430).
    ///
    /// **The pairs rather than a count, because the caller needs the
    /// destination.** The app may have one of these files *open*, and its
    /// `session.path` is absolute: left alone, the next save recreates the
    /// document in the folder the library has just abandoned, under an id the new
    /// library also holds — two files, one `meta.id`, and every edit after the
    /// move going to the one nothing lists.
    ///
    /// ⚠️ **A path map and not a prefix swap**, which is the reason the caller
    /// cannot compute this for itself: the stem may change on the way across.
    /// `unique_stem` renames into a collision, so `old/kestrel/notes.ondin` can
    /// arrive as `new/kestrel/notes-1.ondin`, and a caller re-joining the
    /// relative path under the new root would point the session at a file that
    /// belongs to a *different* document.
    ///
    /// Sidecars are not in here — nothing outside this module holds a path to a
    /// version file or a snapshot; both are found by id.
    pub paths: Vec<(PathBuf, PathBuf)>,
    /// Version files and trashed documents moved.
    pub sidecars: usize,
    /// Projects added to the destination's `projects.json`.
    pub projects: usize,
    /// Everything that could not be moved, **named** — the source path in each
    /// case, because that is the one the user can still go and look at.
    ///
    /// 🚨 **This was a `usize`, and a count is not a report** (§15 D620,
    /// `[S1.3-L6-06]`). It is incremented from twelve sites — a document, a
    /// version file, a trashed document, a crash snapshot, a directory that could
    /// not be created, a `projects.json` that would not parse — and the status
    /// line said *"Moved 37 file(s); 23 could not be moved"*: no names, no paths,
    /// and no way to tell a document from a sidecar. A migration is not
    /// re-runnable in place either (`apply_library_settings` re-points the app at
    /// the new root whether it succeeded, partly succeeded or did nothing), so the
    /// only recovery is to go and find the files by hand — which is impossible
    /// without knowing which they are.
    ///
    /// **Source paths and not destinations**, deliberately: a failure means the
    /// file is still where it was, and `move_file` is copy-then-delete with the
    /// delete last, so the source is the thing that exists.
    pub failed: Vec<PathBuf>,
}

impl Moved {
    /// How many documents moved.
    ///
    /// **Derived rather than counted alongside**, so the number in the status
    /// line and the map the caller re-points the session through cannot disagree
    /// about which files went.
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
    /// One spelling, so no site can record a failure without naming one — which
    /// is what a bare `failed += 1` allowed at all twelve of them.
    fn fail(&mut self, what: &Path) {
        self.failed.push(what.to_path_buf());
    }

    /// A one-line report for the status bar.
    ///
    /// ⚠️ **It names the first one.** A status line cannot hold twenty-three
    /// paths and the *Library settings* modal is where a full list belongs —
    /// §15 D620 left that to the maintainer and **§15 D810 built it**, so the
    /// modal now draws the names and offers the run again. One name here is
    /// still the difference between a number the user can act on and one they
    /// cannot: it says which folder to go and look in, and whether what was left
    /// behind is a document or a sidecar, without opening Settings at all.
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
        // **The name the file already had, wherever it is free** (§15 D509,
        // `[S1.3-L1-04]`). This re-derived a stem from `entry.display_name()`,
        // which is a *rename* of every document in the library, done silently —
        // including files the user named by hand.
        //
        // ⚠️ **What it destroyed is the sync-conflict mark, and that mark exists
        // only in the filename.** `scan::conflict_marker`'s own doc says every
        // pattern it matches is one `slug` cannot produce, which is what makes
        // its false-positive rate against this app's own files zero — so
        // re-slugging is the one operation that can delete it. A conflict copy
        // carries the *original's* metadata block, so `landing-v4.ondin` and
        // Dropbox's `landing-v4 (1).ondin` both slug to `landing-v4` and the
        // second took `-1`. Measured: both marked `conflict=None` afterwards,
        // sharing one `meta.id`.
        //
        // ⚠️ **`-1` is `naming::unique_stem`'s collision suffix, i.e. what two
        // documents somebody named the same thing get** — *not* a *Duplicate*'s,
        // which is `format!("{} copy", …)` and slugs to `…-copy`. The pair is
        // therefore indistinguishable from two hand-named documents, and the
        // thing that separates it from any legitimate pair is that a legitimate
        // one has **two ids** and this has one.
        //
        // Two entries under one id then collide on everything keyed by it:
        // `.versions/<id>/`, the star and the *Recent* row, and `cover::key`
        // (`{id}-{modified}`, and a rename preserves the mtime, so the second
        // card can draw the first's picture). `recovery::pending` resolves a
        // snapshot with a `find` over that id and would offer to overwrite
        // whichever of the pair it met first.
        //
        // The fallback is unchanged and still needed: a stem can be taken at the
        // destination, which is what merging two libraries means.
        let taken = |s: &str| dir.join(format!("{s}.{DOCUMENT_EXT}")).exists();
        let stem = match taken(&entry.stem) {
            false => entry.stem.clone(),
            true => naming::unique_stem(&entry.display_name(), &taken),
        };
        let target = dir.join(format!("{stem}.{DOCUMENT_EXT}"));
        match move_file(&entry.path, &target) {
            // **Recorded here, where both halves are in hand.** The destination
            // is not derivable from the source outside this loop — `unique_stem`
            // may have renamed it — so a caller that has one of these files open
            // has no way to find it again unless the pair is kept.
            true => moved.paths.push((entry.path.clone(), target)),
            false => moved.fail(&entry.path),
        }
    }

    // **Documents `scan` could not name, carried across verbatim** (§15 D809).
    // A `.ondin` whose filename is not valid Unicode is not an `Entry` and never
    // was, so the loop above cannot see it — and until this ran, *Change base
    // folder* left it alone in a folder the app no longer reads, **uncounted**,
    // under a status line that said the migration had succeeded. That is exactly
    // what `Collision::Stranded`'s note calls the bad outcome and what §15 D431
    // fixed for a `.trash` collision and not for this.
    //
    // ⚠️ **By its own `OsStr` name, with no `unique_stem` and no re-derivation.**
    // The loop above may rename into a collision because it has a stem to work
    // with; here there is no `&str` at all, so the only honest target is the name
    // the file already has. A destination already holding that name is therefore a
    // failure rather than a rename — one of these cannot be told from another by
    // looking, which is `Collision::Stranded`'s own argument.
    //
    // ⚠️ **Moving it does not make it visible**, and this deliberately does not
    // try: the library still cannot list a name that is not text (see
    // `scan::unnameable`). What the move buys is that the file is where the user
    // pointed their library, and that `Moved` says it went.
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

    // Version history, the trash and the crash snapshots: whole trees, merged
    // rather than replaced.
    //
    // ⚠️ **`.recovery` travels for the same reason the other two do, and it is
    // the one whose absence would be silent.** What would be stranded is a
    // snapshot the user has *not yet answered* — one from a crash, sitting in
    // the old folder, which nothing would ever ask about again.
    //
    // ⚠️ **This used to open by saying that a library moved mid-session "leaves
    // the app writing into the new root on its next tick either way", and that
    // was false of the document** (§15 D430). It is true of the *snapshot*:
    // `drop_recovery` re-reads `library.root` and the recovery key is the
    // document id, so both survive the move. `session.path` is **absolute**, and
    // left alone it went on naming the abandoned folder — the next save
    // recreating the document there, beside the copy that had just been carried
    // across under the same `meta.id`. The caller re-points it now, through
    // [`Moved::paths`]; the sentence is struck rather than deleted because it is
    // the reason nobody looked.
    //
    // ⚠️ **One policy each, and they are not the same policy** — see
    // [`Collision`], which is where the argument for each of the three is.
    for (dir, collision) in [
        (VERSIONS_DIR, Collision::Identical),
        (TRASH_DIR, Collision::Rename),
        (RECOVERY_DIR, Collision::Stranded),
    ] {
        merge_tree(&from.join(dir), &to.join(dir), collision, &mut moved);
    }

    merge_projects(from, to, &mut moved);
    moved
}

/// What a file already at the destination means, which differs per tree — and
/// the reason `merge_tree` cannot have one answer (§15 D431).
///
/// ⚠️ **It had one — skip — and it was only ever argued for `.versions`.** The
/// same line applied to `.trash` strands a document: `store::trash` uniquifies
/// *within one folder*, so two libraries can each hold a `notes.ondin` that are
/// different documents, and the source's was left in a folder the app no longer
/// reads, uncounted, under a status line that said *"Moved 2 file(s)"*. It could
/// never be purged either, since `purge_trash` only reads the current root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Collision {
    /// **The same file arriving twice**, so skipping loses nothing. True of
    /// `.versions` and only of `.versions`: a path there is
    /// `<document id>/<unix seconds>.ondin`, so a collision is the same document
    /// at the same instant.
    Identical,
    /// **Different files that happen to share a name**, so the arrival takes a
    /// suffix — the same answer `store::trash` and `store::file_document`
    /// already give within one folder, spelled through the same
    /// [`naming::unique_stem`].
    Rename,
    /// **Cannot be told apart and must not be renamed**, so the file stays where
    /// it is and the migration says so.
    ///
    /// `.recovery` is this one, and the *renaming* is what makes it so: a
    /// snapshot's stem is the document id it belongs to, `recovery::pending`
    /// matches on it, and `recovery::remove` refuses a stem that is not a minted
    /// id — so a `<id>-1.ondin` would be offered to the user and then be
    /// **undiscardable**, which is worse than being left behind and told about.
    /// A collision needs two libraries holding one document id, which is a
    /// hand-copied file rather than anything the app does.
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
                        // Counted as a failure rather than passed over, because
                        // that is what it is from the user's side: a file they
                        // asked to move that is still in the old folder. The
                        // summary already has the sentence for it.
                        Collision::Stranded => {
                            moved.fail(&path);
                            continue;
                        }
                        Collision::Rename => {
                            // ⚠️ **An extension that is not text is stranded
                            // rather than dropped** (§15 D848, `[X1.1-L1-05]`).
                            // It read `.and_then(to_str)`, so `notes.<not text>`
                            // arrived as a bare `notes` — no `.ondin`, so nothing
                            // the trash listing would ever show, restore or
                            // purge. There is no extension to put back, which is
                            // §15 D809's argument for the unnameable loop above,
                            // so the honest outcome is `Stranded`'s.
                            //
                            // 🚨 **The finding's headline case is not a defect.**
                            // It said a stem that is not text became `.ondin`, a
                            // hidden name; it becomes `slug("")`'s fallback, and
                            // the file arrives as a listable `untitled.ondin`.
                            // Measured by the flip that was meant to prove the
                            // opposite, which left *"nothing arrives under a name
                            // the library cannot list"* green.
                            //
                            // `None` is no extension; `Some(None)` is one that is
                            // not text.
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
                            // The extension is put back afterwards, so the
                            // suffix lands on the stem and `notes.ondin` becomes
                            // `notes-1.ondin` rather than `notes.ondin-1`.
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
/// ⚠️ **By id, adding only what is missing.** Two libraries can legitimately
/// hold a project called "Kestrel" that are not the same project, and they can
/// hold the same project under two names if one machine renamed it — so the id
/// is the only thing that can decide, and the destination's copy wins on a
/// conflict because that is the library the user is moving *into*.
///
/// A destination whose file cannot be read is left alone entirely, for
/// `Library::save_projects`'s reason: writing it would replace something that is
/// merely unreadable with something that is definitely wrong.
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
/// Returns whether the file is now at `to` and gone from `from`. ⚠️ **A copy
/// that succeeds and a delete that fails still counts as moved**, because the
/// data is safely at the destination and the leftover is a stale duplicate in a
/// folder the app no longer reads — the opposite order of failure is the one
/// that loses work.
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

    /// **A document that could not be moved is named, left where it was, and
    /// reported** (§15 D620, `[S1.3-L6-06]`).
    ///
    /// 🚨 **No test exercised a failed file move at all.** All seven tests in
    /// this module were happy paths; `moved.failed` was asserted non-zero in
    /// exactly one, and that path never calls `move_file`. So `Moved::summary`'s
    /// failure arm had no test caller either — **deleting it and returning
    /// `"Moved {d} file(s)"` unconditionally left the module green**, i.e. the app
    /// could report a partial migration as a complete one and nothing noticed.
    ///
    /// **The failure is arranged rather than mocked**: a *file* is put where the
    /// project's *folder* has to go, so `create_dir_all` fails for exactly one
    /// document and the rest of the migration runs normally. That is a real
    /// arm of `relocate` — the one at the top of the document loop — and it is
    /// deterministic on every platform, which a permission-denied directory or a
    /// held-open handle is not.
    ///
    /// ⚠️ **The second document is the control and it is not decoration.** A
    /// version that gave up on the whole migration at the first failure would
    /// satisfy every assertion about the failed one; what says the migration
    /// carried on is that the loose document arrived.
    ///
    /// **Flip-check, run, both bite at their predicted sites.**
    /// `Moved::summary`'s failure arm deleted — the review's own flip, which left
    /// this module green — fails at *"the status line says which"* with
    /// **`"Moved 1 file(s) and 1 project(s)"`**, a cheerful line for a migration
    /// that stranded a document. ⚠️ The prediction said `"Moved 1 file(s)"` and
    /// forgot the project also moves in this fixture, which is a reminder that the
    /// arm being flipped is not the only one that composes the sentence. It also
    /// bites `a_snapshot_that_would_collide_is_left_behind_and_reported`, which is
    /// the *other* test that reads the summary.
    ///
    /// And replacing `moved.fail(&entry.path)` at that site with a fail on `dir`
    /// keeps the count at one and fails the **name**, `…\to\kestrel` against
    /// `…\from\kestrel\landing-v4.ondin` — which is precisely the difference
    /// between this test and the `usize` it replaced.
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

    /// ⚠️ **Nothing is overwritten.** A document whose slug is already taken at
    /// the destination arrives beside it, not on top of it.
    ///
    /// Flip-check, run: replacing `unique_stem` with a bare `slug` leaves the
    /// count assertion green — the migration still reports one document moved —
    /// and fails on the *second* one, because the destination ends up with a
    /// single file holding the source's contents. A test asserting only the
    /// return value would have passed a migration that silently ate a document.
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

    /// **A sync-conflict copy keeps its mark across the move** (`[S1.3-L1-04]`,
    /// §15 D509).
    ///
    /// `relocate` re-derived every stem from `entry.display_name()`, which is a
    /// silent rename of every document in the library. ⚠️ **The conflict mark
    /// exists only in the filename** — `scan::conflict_marker`'s own doc says
    /// every pattern it matches is one `slug` cannot produce, which is what makes
    /// its false-positive rate against this app's own files zero — so re-slugging
    /// is the one operation that can delete it.
    ///
    /// A conflict copy carries the **original's** metadata block, so
    /// `landing-v4.ondin` and Dropbox's `landing-v4 (1).ondin` both slugged to
    /// `landing-v4` and the second took `-1`. Measured before the fix: both
    /// afterwards reported `conflict=None`, sharing one `meta.id` —
    /// `.versions/<id>/`, the star, the *Recent* row, `cover::key` and
    /// `recovery::pending`'s `find` all keyed on a value two entries share.
    ///
    /// ⚠️ **`-1` is `naming::unique_stem`'s collision suffix — what two documents
    /// somebody named alike get — and not a *Duplicate*'s**, which is
    /// `format!("{} copy", …)` and slugs to `…-copy`. So the pair reads as two
    /// hand-named documents, and the only thing separating it from a legitimate
    /// pair is that a legitimate one has two ids. *(This sentence said "the suffix
    /// a Duplicate takes" until `arch-scribe` read `store::duplicate` against
    /// it — the argument survived, the example did not.)*
    ///
    /// **Three assertions and each is a different failure.** The conflict copy is
    /// still named what it was named; it is still the one the dashboard *marks*,
    /// which is the thing the user sees; and the original kept its own name too,
    /// which is what says the fallback did not simply stop renaming everything.
    ///
    /// ⚠️ **The mark is checked before the move as well.** A fixture whose copy
    /// was never marked would pass the "still marked" assertion by never having
    /// been unmarked, which is `CLAUDE.md`'s third vacuity shape.
    ///
    /// **Flip run**, the stem re-derived unconditionally as before: fails on
    /// *"the conflict copy keeps its filename"* — the predicted site — with the
    /// file arriving as `landing-v4-1.ondin`.
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

    /// An unanswered crash snapshot travels with the library (§15 D377).
    ///
    /// ⚠️ **This is the one of the three trees whose absence would be silent.**
    /// A stranded version or a stranded trashed file is visible — the history is
    /// short, the trash is empty, and the user can go and look in the old folder.
    /// A stranded snapshot is a question that is simply never asked again: the
    /// launch that would have offered the work reads the *new* root, finds
    /// nothing, and says nothing. The work is still on disk and nothing will ever
    /// mention it.
    ///
    /// **Flip-checked** by dropping `RECOVERY_DIR` from `relocate`'s tree loop —
    /// which is the state this shipped in until the module was written — and the
    /// second assertion fails with the snapshot still sitting in the old folder.
    /// Predicted that site and it was that site; the `documents` count in the
    /// first assertion is untouched either way, because a snapshot is not a
    /// document and `merge_tree` does not count into that field.
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

    /// **`[S1.3-L5-03]`'s loss.** Two libraries can each hold a trashed
    /// `notes.ondin` — `store::trash` uniquifies within one folder and knows
    /// nothing about any other — and they are *different documents*. Skipping
    /// the arrival left one of them in a folder the app no longer reads, with
    /// no counter, under a status line that said the move had succeeded, and
    /// `purge_trash` could never reach it either.
    ///
    /// ⚠️ **The assertion order puts the file first and the count second.** A
    /// run that reported the strand honestly and still lost the file would be a
    /// better bug and the same loss; the count is what makes it *findable*, not
    /// what makes it wrong.
    ///
    /// Flip-check, run: setting `.trash`'s policy back to `Collision::Identical`
    /// fails at *"both trashed documents are in the library that was moved
    /// into"* with `left: ["destination"] right: ["destination", "source"]` — the
    /// source document simply absent. ⚠️ **And `moved.failed` stays 0 under that
    /// flip**, which is the silence the finding was actually about: the run that
    /// loses the file and the run that does not are indistinguishable from the
    /// status line, so a test asserting only the counters would have been green.
    /// The bodies are read as text for exactly that reason — a failure here has
    /// to *name* what went missing.
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

    /// **A snapshot that cannot move says so**, where a trashed document renames
    /// itself out of the way.
    ///
    /// A `.recovery` stem is the document id it belongs to, and both
    /// `recovery::pending` and `recovery::remove` read it as one — so renaming
    /// would produce a snapshot the user is offered and cannot discard. It stays
    /// where it is, and the migration reports a failure rather than success.
    ///
    /// ⚠️ **This is not the same shape as the version skip beside it**: a
    /// colliding version is genuinely the same file, and a colliding snapshot is
    /// two different unsaved states of one document. Hence one silent skip and
    /// one counted one.
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
        // ⚠️ **The path and not the count** (§15 D620, `[S1.3-L6-06]`). This
        // asserted `moved.failed == 1`, which is satisfied by a migration that
        // strands the wrong file — and the whole complaint about the old `usize`
        // is that the user could not tell which.
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

    /// ⚠️ An unreadable destination `projects.json` stops the merge rather than
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

    /// **A document whose filename is not valid Unicode travels with the rest of
    /// the library, and it is counted** (§15 D809).
    ///
    /// 🚨 **It used to be left behind in silence.** `scan::collect` skips an entry
    /// whose `file_name().to_str()` is `None`, so such a file is never an `Entry`
    /// and the document loop cannot see it — it stayed in a folder the app no
    /// longer reads, absent from `Moved::failed`, under a status line that said
    /// the migration had succeeded. That is `Collision::Stranded`'s bad outcome
    /// arriving through a different door from §15 D431's.
    ///
    /// ⚠️ **The name is built per platform and means two different things** — see
    /// `scan::unnameable_doc_name`. What is asserted here is the same on
    /// both.
    ///
    /// ⚠️ **The ordinary document is the control**, and it is what says the new
    /// loop did not simply move everything twice or stop the old one: a version
    /// that moved the odd file *instead* of running the scan's loop would satisfy
    /// every assertion below about the odd one.
    ///
    /// **Flip-check, run** — the whole `for path in scan::unnameable(from)` loop
    /// removed: fails at *"and it arrived"*, the predicted site. ⚠️ The assertion
    /// **above** it, on `documents()`, was predicted to be the one that bites and
    /// is not — it reads 1 against 2 under the flip and would have caught it, but
    /// the arrival is asserted first and the count is the weaker claim of the two.
    /// Ordering the loss before the tally is what puts the right sentence in the
    /// failure message.
    #[test]
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

    /// **A document in a project folder whose name is not text is moved and
    /// counted, and a hidden one is still hidden** (§15 D848, `[R1-L2-04]`).
    ///
    /// 🚨 **It was in neither half of the scan.** `scan::collect`'s non-Unicode
    /// arm recorded documents and `continue`d past everything else, so such a
    /// *folder* was never descended into, and a migration — which iterates exactly
    /// `scan` and `unnameable` — left every document in it behind, absent from
    /// `Moved::failed` and so from the *Stranded* card.
    ///
    /// ⚠️ **The hidden folder is the control for the dot rule**, which the walk
    /// now asks of the lossy name: a leading `.` survives the loss, and a folder
    /// that starts with one is not a project whatever follows it.
    ///
    /// **Flip-checks, run**: the directory arm's descent disabled fails at *"the
    /// walk reaches the folder whose name is not text"* — the listing, before the
    /// migration is ever asked; its `!hidden` term removed fails at *"the hidden
    /// one is not a project"*, with `stray` in the listing.
    #[test]
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

    /// **A destination already holding the unnameable file's name is a failure,
    /// and the file there is untouched** (§15 D848, `[X1.1-L6-03]`).
    ///
    /// `target.exists()` is §15 D809's whole collision policy for this loop and
    /// the only thing between it and `move_file`, which overwrites on both of its
    /// arms. The sibling test's destination never existed, so deleting the guard
    /// passed all five of its assertions.
    ///
    /// **The loss is asserted first**, the ordering the sibling test learned by
    /// predicting the wrong site. **Flip-check, run**: the `target.exists()` term
    /// removed fails at *"the file already there is untouched"*.
    #[test]
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

    /// **A `.trash` collision on a name that is not text: a bad *stem* is renamed
    /// to something listable, a bad *extension* is stranded** (§15 D848,
    /// `[X1.1-L1-05]`).
    ///
    /// 🚨 **The finding's headline was wrong and this test is what said so.** It
    /// claimed `Collision::Rename`'s `.unwrap_or("")` named the arrival `.ondin`,
    /// a leading-dot name `scan` skips; the first version of this test asserted
    /// that nothing arrived under that name, and the flip meant to prove the
    /// guard left it green. `slug("")` answers its fallback, so the arrival is
    /// `untitled.ondin` — ordinary, listable, restorable — and the first half
    /// below pins that as the behaviour rather than a defect. What *was* real is
    /// the finding's aside: an extension that is not text was dropped, and a bare
    /// `notes` in the trash is a file nothing will ever list.
    ///
    /// **Flip-check, run**: the `Some(None)` arm restored to dropping the
    /// extension fails at *"a bad extension is not dropped"*.
    #[test]
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
