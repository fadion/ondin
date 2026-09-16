//! Every write the library makes: filing a new document, renaming one, moving
//! it between projects, deleting it, and pinning a version (§9.5, §15 D364).
//!
//! **One rule holds all of these together — the file's metadata block is the
//! record, and the path is a consequence of it.** A rename writes a new name
//! into the document and *then* renames the file to match; a move writes a new
//! project id and then moves the file into that project's folder. Doing it the
//! other way round — deriving the name or the project from where the file sits —
//! is what makes a library break the first time somebody drags something in
//! Explorer.
//!
//! ⚠️ **So every operation here has two halves, and the second one may fail on
//! its own.** A rename that writes the document and then cannot rename the file
//! leaves a document called "My cool design" in `old-name.ondin`. That is the
//! failure this module is *designed* to land on, because the dashboard reads the
//! block: the entry shows the right name, the stem is merely stale, and the next
//! rename fixes it. The opposite order fails into a file whose name and contents
//! disagree with nothing to say which is right.
//!
//! **Nothing here is undoable.** These are library actions, not document edits —
//! see `ondin_core::meta` for why they are outside `apply` — so *Delete* moves
//! to [`super::scan::TRASH_DIR`] rather than removing anything, and that is the
//! only "undo" the library has.

use super::project::Project;
use super::scan::{DOCUMENT_EXT, Entry, TRASH_DIR, VERSIONS_DIR};
use super::{clock, ids, naming};
use ondin_core::Document;
use ondin_core::meta::DocumentMeta;
use std::io;
use std::path::{Path, PathBuf};

/// The path a document with this stem would take in `dir`.
fn doc_path(dir: &Path, stem: &str) -> PathBuf {
    dir.join(format!("{stem}.{DOCUMENT_EXT}"))
}

/// Pick a stem free in `dir`, given the typed name.
///
/// `keep` is the one existing file that does not count as a collision — the
/// document being renamed. Without it, renaming "Landing v4" to "Landing v4"
/// (or to anything that slugs the same, like "landing V4") would walk to
/// `landing-v4-1` because the file being renamed is sitting there.
fn free_stem(dir: &Path, name: &str, keep: Option<&Path>) -> String {
    naming::unique_stem(name, &|stem| {
        let candidate = doc_path(dir, stem);
        if Some(candidate.as_path()) == keep {
            return false;
        }
        candidate.exists()
    })
}

/// Write a document into the library for the first time: mint its identity,
/// stamp it, name it, and save it.
///
/// Used by *New file* and by *Import* alike — an imported `.ondin` is a document
/// that has not been through the library, which is exactly the state a new one
/// is in. **Only the fields that are missing are filled**, so importing a
/// document that already carries a name and a creation date keeps both; what it
/// always gets is a project (the one it was imported into) and, if it has none,
/// an id.
///
/// Returns the path written.
pub fn file_document(
    root: &Path,
    project: Option<&Project>,
    name: &str,
    doc: &mut Document,
) -> io::Result<PathBuf> {
    let dir = project
        .map(|p| p.dir(root))
        .unwrap_or_else(|| root.to_path_buf());
    std::fs::create_dir_all(&dir)?;

    let mut meta = doc.meta().clone();
    if meta.id.is_none() {
        meta.id = Some(ids::mint());
    }
    if meta.created.is_none() {
        meta.created = Some(clock::now());
    }
    meta.name = Some(name.to_string());
    meta.project = project.map(|p| p.id.clone());
    doc.set_meta(meta);

    let path = doc_path(&dir, &free_stem(&dir, name, None));
    write_document(&path, doc)?;
    Ok(path)
}

/// Save a document over its existing path.
///
/// Separate from [`file_document`] because it is the one write that must **not**
/// touch the metadata: an autosave firing every thirty seconds is not the moment
/// to re-decide a document's identity, and a save that could rename the file
/// under the editor would be a bug nobody could reproduce.
///
/// ⚠️ **Atomic** (`crate::atomic`, §15 D378), and this is the headline case: it
/// was a plain `fs::write` until 2026-08-28, which truncates the user's document
/// before writing a byte — so the window in which a crash left them with neither
/// version was the whole of every save, autosave included.
pub fn write_document(path: &Path, doc: &Document) -> io::Result<()> {
    let bytes = ondin_core::io::save(doc).map_err(io::Error::other)?;
    crate::atomic::write(path, &bytes)
}

/// Rename a document: the typed name inside the file, then the file itself.
///
/// Returns the new path, which is the old one when the slug did not change —
/// "Landing v4" to "Landing V4" is a real rename that moves no file.
pub fn rename(entry: &Entry, new_name: &str) -> io::Result<PathBuf> {
    let mut doc = load(&entry.path)?;
    let mut meta = doc.meta().clone();
    meta.name = Some(new_name.to_string());
    doc.set_meta(meta);
    // Written to the *old* path first. See the module's note on which half is
    // allowed to fail.
    write_document(&entry.path, &doc)?;

    let dir = entry.path.parent().unwrap_or(Path::new("."));
    let stem = free_stem(dir, new_name, Some(&entry.path));
    let target = doc_path(dir, &stem);
    if target != entry.path {
        std::fs::rename(&entry.path, &target)?;
    }
    Ok(target)
}

/// Move a document to another project — or out of every project, with `None`.
///
/// The block is written first and the file follows it into the project's folder,
/// if that project has one. A project with no folder leaves the file exactly
/// where it is, which is the case that makes "may or may not create a folder"
/// invisible here.
pub fn move_to_project(
    root: &Path,
    entry: &Entry,
    project: Option<&Project>,
) -> io::Result<PathBuf> {
    let mut doc = load(&entry.path)?;
    let mut meta = doc.meta().clone();
    meta.project = project.map(|p| p.id.clone());
    doc.set_meta(meta);
    write_document(&entry.path, &doc)?;

    let dir = project
        .map(|p| p.dir(root))
        .unwrap_or_else(|| root.to_path_buf());
    if dir == entry.path.parent().unwrap_or(Path::new(".")) {
        return Ok(entry.path.clone());
    }
    std::fs::create_dir_all(&dir)?;
    let stem = free_stem(&dir, &entry.display_name(), None);
    let target = doc_path(&dir, &stem);
    std::fs::rename(&entry.path, &target)?;
    Ok(target)
}

/// Copy a document into a new one, with a fresh identity.
///
/// ⚠️ **A new id, deliberately.** A copy that kept the original's id would share
/// its version history and its thumbnail — so pinning a version of the copy
/// would appear in the original's history, and the two would fight over one
/// cache entry. It is also the honest answer to what a duplicate *is*: a new
/// document that happens to start out identical.
///
/// The creation stamp is new for the same reason.
///
/// **Not `ondin_core::copy_name`**, which answers a different question: that one
/// numbers a name *within a set of siblings* and returns `None` for a name a
/// person typed, because renaming somebody's "Hero content" to "Hero content 2"
/// would be hostile. Every document name is one a person typed, so it would
/// return `None` for all of them — and documents have no siblings to be unique
/// among, since two of them may share a name outright. Appending " copy" is the
/// whole rule here, and duplicating twice gives two documents that read alike
/// and differ only in their file.
pub fn duplicate(root: &Path, entry: &Entry, project: Option<&Project>) -> io::Result<PathBuf> {
    let mut doc = load(&entry.path)?;
    doc.set_meta(DocumentMeta::default());
    let name = format!("{} copy", entry.display_name());
    file_document(root, project, &name, &mut doc)
}

/// Move a document to the trash.
///
/// **Nothing is deleted, and nothing records where it came from.** Restoring
/// files it back into its project's folder, which the block already names — so
/// the trash needs no index of its own and cannot get out of step with one. The
/// only thing lost is a document that was sitting somewhere its project did not
/// account for, which comes back to its project's folder instead.
pub fn trash(root: &Path, entry: &Entry) -> io::Result<PathBuf> {
    let dir = root.join(TRASH_DIR);
    std::fs::create_dir_all(&dir)?;
    let target = doc_path(&dir, &free_stem(&dir, &entry.display_name(), None));
    std::fs::rename(&entry.path, &target)?;
    // Best-effort: a stamp that could refuse to be written must not turn a
    // delete into a failure. ⚠️ **But "errs towards purging it sooner" was the
    // wrong direction and the reason this comment used to end there**: the
    // architecture calls the trash the library's *only* undo, so a document
    // that is silently permanently deleted on the next sweep is the one outcome
    // this feature exists to prevent. The measured way for the stamp to fail — a
    // read-only document — is now handled inside [`trash_stamp`]; what is left
    // discarded here is the unmeasured remainder, and it is discarded because
    // the delete itself has already succeeded and reporting it as a failure
    // would be a lie about a file that has moved.
    let _ = trash_stamp(&target);
    Ok(target)
}

/// Everything in the trash, oldest deletion first.
///
/// Deletion time is the file's modification time, which `fs::rename` does *not*
/// update — so this is really "when the document was last edited", and a
/// document deleted today after a year untouched is a year old to
/// [`purge_trash`]. ⚠️ **That is wrong and it is load-bearing**, so the trash's
/// retention is applied from a stamp written on the way in rather than from
/// this: see [`trash_stamp`].
pub fn trashed(root: &Path) -> Vec<Entry> {
    let mut out = super::scan::scan_dir(&root.join(TRASH_DIR));
    out.sort_by_key(|e| e.modified);
    out
}

/// The moment a document entered the trash.
///
/// **Written into the file's modification time on the way in**, because there is
/// nowhere else for it to go: the metadata block may not carry a clock (§5.11a),
/// a sidecar index is the thing [`trash`] exists without, and `fs::rename`
/// preserves the original mtime. So `trash` stamps the moved file and this reads
/// it back — the one place in the library where an mtime is written rather than
/// merely read. §15 D432 is why a stamp that is refused is retried rather than
/// shrugged off.
fn trash_stamp(path: &Path) -> io::Result<()> {
    // `File::set_times` needs the file open for writing; the times themselves
    // are the current instant, which is what an explicit touch means.
    let touch = |p: &Path| -> io::Result<()> {
        let f = std::fs::OpenOptions::new().write(true).open(p)?;
        f.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now()))
    };
    let Err(refused) = touch(path) else {
        return Ok(());
    };
    // ⚠️ **A read-only document, and the attribute is cleared for the length of
    // the touch and put straight back.** Measured: `fs::rename` into `.trash` is
    // *not* refused by the Windows read-only attribute and `fs::remove_file` is
    // not refused by it either — Rust's implementation clears it and retries —
    // so this open was the one step of the three that failed, and its result was
    // the one being discarded. The stamp is the retention clock, so the file
    // kept the mtime of its last edit and `purge_trash` deleted it permanently
    // at the next sweep while the Trash screen said it was kept for 30 days.
    //
    // **Restored rather than left clear**, which is the difference from what
    // `remove_file` does and the reason this is not two lines: a document is
    // marked read-only to protect it, and the trash is the library's only undo —
    // so restoring one must give back the file the user had, protection
    // included. Setting permissions does not touch the mtime, so the stamp
    // survives the second call.
    let original = std::fs::metadata(path)?.permissions();
    if !original.readonly() {
        return Err(refused);
    }
    let mut writable = original.clone();
    owner_may_write(&mut writable);
    std::fs::set_permissions(path, writable)?;
    let stamped = touch(path);
    // The *original* object, not `set_readonly(true)`: on Unix that would mean
    // 0o444 and drop write permission from a group that had it.
    let _ = std::fs::set_permissions(path, original);
    stamped
}

/// Let the file's owner write it, changing nothing else about who may.
///
/// ⚠️ **Not `Permissions::set_readonly(false)`**, which is the obvious spelling
/// and is wrong off Windows: on Unix it means mode `0o666` whatever the file's
/// mode was, so a briefly-writable file would also be briefly **world**-writable.
/// The one bit [`trash_stamp`] needs is the owner's, and it needs it for the
/// length of one `set_times`.
#[cfg(unix)]
fn owner_may_write(perms: &mut std::fs::Permissions) {
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(perms.mode() | 0o200);
}

/// Windows has one bit and this is it. See the Unix twin for why they are two
/// functions.
#[cfg(not(unix))]
fn owner_may_write(perms: &mut std::fs::Permissions) {
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
}

/// Delete trashed documents older than `keep_days`, permanently.
///
/// Returns how many were removed. `0` for `keep_days` empties the trash.
pub fn purge_trash(root: &Path, keep_days: u64) -> usize {
    let cutoff = keep_days.saturating_mul(86_400);
    let mut removed = 0;
    for entry in trashed(root) {
        if clock::since(entry.modified) >= cutoff && std::fs::remove_file(&entry.path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Copy a document's current bytes into its version history.
///
/// `.versions/<document id>/<unix seconds>.ondin` — **keyed on the id rather
/// than the stem**, so history survives a rename, which is the whole reason
/// `DocumentMeta::id` exists.
///
/// A document with no id yet has no history to pin to; that is not an error, it
/// is a document that has never been filed, and the caller files it first.
///
/// ⚠️ **Read-then-[`crate::atomic::write`], not `fs::copy`** (§15 D378). This one
/// nearly escaped the sweep on the grounds that it cannot destroy anything: the
/// target is a fresh name that never collides, so a torn copy is a bad *new* file
/// rather than a lost old one. That reasoning is right and the conclusion was
/// wrong — [`versions`] lists this directory by the `ondin` extension, so a
/// half-copied pin is **listed as a version**, offered for restore, and fails
/// when taken. A version history with an entry that cannot be opened is worse
/// than one with a gap, because only the second is honest about itself.
pub fn pin_version(root: &Path, entry: &Entry) -> io::Result<Option<PathBuf>> {
    // ⚠️ **`is_minted`, not just `is_some`.** The id is joined as a directory
    // name here, so a document asserting `"id": "../.."` in its own file would
    // give *Pin version* full control of where the copy lands. See
    // `library::ids::is_minted`; a malformed id reads as "not filed", which is
    // the state this function already declines.
    let Some(id) = entry.meta.id.as_deref().filter(|id| ids::is_minted(id)) else {
        return Ok(None);
    };
    let dir = root.join(VERSIONS_DIR).join(id);
    let stamp = clock::now();
    let mut target = dir.join(format!("{stamp}.{DOCUMENT_EXT}"));
    // Two pins inside the same second — Ctrl+S twice — would otherwise silently
    // overwrite the first. A suffix costs a filename and keeps both.
    let mut n = 1;
    while target.exists() {
        target = dir.join(format!("{stamp}-{n}.{DOCUMENT_EXT}"));
        n += 1;
    }
    // The bytes rather than the file, because `atomic::write` takes a slice —
    // and it makes `dir` on the way, which is why the `create_dir_all` that used
    // to stand above the collision loop is gone. `exists()` on a directory that
    // is not there yet is `false`, which is the right answer for the loop.
    let bytes = std::fs::read(&entry.path)?;
    crate::atomic::write(&target, &bytes)?;
    Ok(Some(target))
}

/// A document's pinned versions, newest first.
///
/// 🚨 **Allow-listed *here*, because the app pins versions and has no path that
/// ever lists them** (§15 D699, `[S16.5-L3-04]`). [`pin_version`] is reached
/// from `save_file`, so the `.versions/` tree really is being written; this is
/// the reader half, and its six call sites are every one of them inside a
/// `#[cfg(test)]` module. It was invisible because `main.rs` carried a blanket
/// `#[allow(dead_code)]` over the whole `library` module — a receipt for a
/// condition that ended when the dashboard landed — and removing that surfaced
/// this and nothing else, under both the plain build and `--all-targets`.
///
/// ⚠️ **On the item rather than on the module, and that is the whole point.** A
/// module-level allow shields everything under it for ever; this one shields
/// one function, so the plain build is still the gate that catches the *next*
/// dead thing in `library` — verified with a throwaway `fn`, which it reported.
///
/// 🚨 **`#[cfg(test)]` was tried first and the doc gate refused it**, which is
/// worth knowing because §15 D672's sibling lesson pushes the other way: an
/// allow-listed item is still a live *root*, so an allow can shield a whole
/// graph hanging off it. That trap needs the item to **name** other dead
/// things — it was found on a `pub const ALL` listing 143 constants — and this
/// function names only `VERSIONS_DIR` and `DOCUMENT_EXT`, both of which
/// `pin_version` uses too, so it shields nothing but itself. What `cfg(test)`
/// *did* do was delete the item from the documented crate and break
/// `pin_version`'s doc link to it — and that link is load-bearing: it is the
/// argument for D378's read-then-atomic-write, namely that a half-copied pin is
/// **listed as a version** by this function. **The narrower attribute is the one
/// that keeps the reasoning readable.**
///
/// ⚠️ **This is not a ruling on whether the function should exist.** Keeping it
/// as dead code or building the version-history screen it is the back half of
/// are both open, and marking it decides neither — it makes the choice visible
/// instead of shielded. Delete the attribute the moment a real caller lands,
/// which is the instruction the blanket receipt gave and that went unfollowed
/// for a fortnight.
#[allow(dead_code)]
pub fn versions(root: &Path, id: &str) -> Vec<PathBuf> {
    let dir = root.join(VERSIONS_DIR).join(id);
    let Ok(read) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(DOCUMENT_EXT))
        .collect();
    // By filename, which is the stamp — and therefore by time, without a stat
    // per file. Reversed for newest first.
    out.sort();
    out.reverse();
    out
}

/// Read a document off disk.
fn load(path: &Path) -> io::Result<Document> {
    let bytes = std::fs::read(path)?;
    ondin_core::io::load(&bytes).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::project::PROJECT_COLORS;
    use crate::library::scan::scan;
    use ondin_core::IdSource;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ondin-store-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn blank() -> Document {
        let mut ids = IdSource::new(0xD0C);
        Document::new(ids.mint())
    }

    fn project(id: &str, folder: Option<&str>) -> Project {
        Project {
            id: id.into(),
            name: "Kestrel".into(),
            color: PROJECT_COLORS[0].into(),
            folder: folder.map(str::to_string),
            created: 1_774_483_200,
            archived: false,
        }
    }

    fn entry_named(root: &Path, name: &str) -> Entry {
        scan(root)
            .into_iter()
            .find(|e| e.display_name() == name)
            .unwrap_or_else(|| panic!("no entry called {name} in {root:?}"))
    }

    #[test]
    fn filing_a_document_names_it_stamps_it_and_gives_it_an_id() {
        let root = temp_root("file");
        let mut doc = blank();
        let path = file_document(&root, None, "My cool design", &mut doc).unwrap();
        assert_eq!(path, root.join("my-cool-design.ondin"));
        assert_eq!(doc.meta().name.as_deref(), Some("My cool design"));
        assert!(doc.meta().id.is_some());
        assert!(doc.meta().created.is_some());
        assert_eq!(doc.meta().project, None);
        // And it is on disk with the block in it, readable by the scan.
        assert_eq!(entry_named(&root, "My cool design").stem, "my-cool-design");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Two documents may share a typed name; the filesystem disambiguates and
    /// the names stay identical.
    #[test]
    fn a_second_document_of_the_same_name_gets_a_suffixed_file() {
        let root = temp_root("collide");
        let mut a = blank();
        let mut b = blank();
        let first = file_document(&root, None, "My cool design", &mut a).unwrap();
        let second = file_document(&root, None, "My cool design", &mut b).unwrap();
        assert_eq!(first, root.join("my-cool-design.ondin"));
        assert_eq!(second, root.join("my-cool-design-1.ondin"));
        assert_eq!(a.meta().name, b.meta().name);
        assert_ne!(a.meta().id, b.meta().id);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_project_with_a_folder_takes_its_files_and_one_without_does_not() {
        let root = temp_root("folders");
        let with = project("p-with", Some("kestrel"));
        let without = project("p-without", None);
        let mut a = blank();
        let mut b = blank();
        let in_folder = file_document(&root, Some(&with), "Landing v4", &mut a).unwrap();
        let at_root = file_document(&root, Some(&without), "Pricing table", &mut b).unwrap();
        assert_eq!(in_folder, root.join("kestrel").join("landing-v4.ondin"));
        assert_eq!(at_root, root.join("pricing-table.ondin"));
        // Both know their project regardless of where the file sits.
        assert_eq!(a.meta().project.as_deref(), Some("p-with"));
        assert_eq!(b.meta().project.as_deref(), Some("p-without"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn renaming_writes_the_name_and_moves_the_file() {
        let root = temp_root("rename");
        let mut doc = blank();
        file_document(&root, None, "Landing v4", &mut doc).unwrap();
        let entry = entry_named(&root, "Landing v4");
        let moved = rename(&entry, "Homepage hero").unwrap();
        assert_eq!(moved, root.join("homepage-hero.ondin"));
        assert!(!root.join("landing-v4.ondin").exists());
        assert_eq!(entry_named(&root, "Homepage hero").stem, "homepage-hero");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ⚠️ A rename that changes only the casing must not walk to `-1`.
    ///
    /// Flip-check, run: dropping the `keep` argument from `free_stem` produces
    /// `landing-v4-1.ondin` and fails on the path assertion — the *name*
    /// assertion stays green, because the block was written correctly either
    /// way. So a test asserting only the visible name would have passed a
    /// library that grew a suffix every time somebody fixed a capital letter.
    #[test]
    fn renaming_to_the_same_slug_keeps_the_file_where_it_is() {
        let root = temp_root("recase");
        let mut doc = blank();
        file_document(&root, None, "Landing v4", &mut doc).unwrap();
        let entry = entry_named(&root, "Landing v4");
        let moved = rename(&entry, "Landing V4").unwrap();
        assert_eq!(moved, root.join("landing-v4.ondin"));
        assert_eq!(entry_named(&root, "Landing V4").stem, "landing-v4");
        assert_eq!(scan(&root).len(), 1, "no second file may appear");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn moving_to_a_project_writes_the_id_and_follows_its_folder() {
        let root = temp_root("move");
        let mut doc = blank();
        file_document(&root, None, "Landing v4", &mut doc).unwrap();
        let target = project("p-1", Some("kestrel"));
        let moved =
            move_to_project(&root, &entry_named(&root, "Landing v4"), Some(&target)).unwrap();
        assert_eq!(moved, root.join("kestrel").join("landing-v4.ondin"));
        assert_eq!(
            entry_named(&root, "Landing v4").meta.project.as_deref(),
            Some("p-1")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Moving into a folderless project rewrites the block and leaves the file
    /// alone — the assertion that pins "grouping is not the folder".
    #[test]
    fn moving_to_a_folderless_project_moves_no_file() {
        let root = temp_root("move-flat");
        let mut doc = blank();
        let original = file_document(&root, None, "Landing v4", &mut doc).unwrap();
        let target = project("p-flat", None);
        let moved =
            move_to_project(&root, &entry_named(&root, "Landing v4"), Some(&target)).unwrap();
        assert_eq!(moved, original);
        assert_eq!(
            entry_named(&root, "Landing v4").meta.project.as_deref(),
            Some("p-flat")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_duplicate_is_a_new_document_with_the_same_drawing() {
        let root = temp_root("dup");
        let mut doc = blank();
        file_document(&root, None, "Landing v4", &mut doc).unwrap();
        let entry = entry_named(&root, "Landing v4");
        let copy = duplicate(&root, &entry, None).unwrap();
        let copied = scan(&root)
            .into_iter()
            .find(|e| e.path == copy)
            .expect("the copy must be in the library");
        assert_ne!(copied.meta.id, entry.meta.id, "a copy is a new document");
        assert_eq!(copied.display_name(), "Landing v4 copy");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Trashing takes the document out of the library without deleting it, and
    /// restoring is just filing it again — which is why the trash keeps no index.
    #[test]
    fn trashing_removes_it_from_the_library_but_not_from_disk() {
        let root = temp_root("trash");
        let mut doc = blank();
        file_document(&root, None, "Landing v4", &mut doc).unwrap();
        let gone = trash(&root, &entry_named(&root, "Landing v4")).unwrap();
        assert!(scan(&root).is_empty(), "the library must no longer list it");
        assert!(gone.exists(), "but the file is still there");
        assert_eq!(trashed(&root).len(), 1);
        assert_eq!(trashed(&root)[0].display_name(), "Landing v4");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ⚠️ **Retention counts from the deletion, not from the last edit** — and
    /// nothing else in this module proves it, because `fs::rename` preserves the
    /// mtime and a freshly written fixture is young either way.
    ///
    /// The fixture is aged to a year old on purpose: without the stamp, a
    /// 30-day retention purges it the instant it is deleted, so a document
    /// somebody had not touched since last year would vanish out of the trash
    /// before they could look for it. Flip-check, run: dropping the
    /// `trash_stamp` call fails both assertions here and leaves every other test
    /// in the module green.
    #[test]
    fn deleting_an_old_document_starts_its_retention_now() {
        let root = temp_root("trash-stamp");
        let mut doc = blank();
        let path = file_document(&root, None, "Ancient", &mut doc).unwrap();
        let a_year_ago =
            std::time::SystemTime::now() - std::time::Duration::from_secs(365 * 86_400);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(a_year_ago))
            .unwrap();
        assert!(
            clock::since(entry_named(&root, "Ancient").modified) > 300 * 86_400,
            "the fixture must actually be old, or this test is about nothing"
        );

        trash(&root, &entry_named(&root, "Ancient")).unwrap();
        assert!(
            clock::since(trashed(&root)[0].modified) < 60,
            "the trashed copy must be stamped with the deletion"
        );
        assert_eq!(purge_trash(&root, 30), 0, "and survive a 30-day retention");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **`[S1.2-L1-01]`'s loss: the same document, marked read-only.**
    ///
    /// The attribute refuses only the *middle* one of the three steps a delete
    /// takes — `rename` into `.trash` is allowed and `remove_file` clears the
    /// attribute for itself — and the middle step is the stamp, whose result was
    /// thrown away. So a read-only document deleted after a year untouched was
    /// permanently gone at the next sweep, under a Trash screen saying it was
    /// kept for thirty days.
    ///
    /// ⚠️ **The test beside this one passes either way and always did**, which is
    /// why this is a second fixture rather than a stronger assertion in that one:
    /// its file is writable, so the flip-check its own doc records proves the
    /// `trash_stamp` call is *reached* and nothing there proves it succeeded.
    ///
    /// Flip-check, run: taking the read-only retry back out of `trash_stamp`
    /// fails at *"the trashed copy must be stamped with the deletion"* — the
    /// clock, which is the loss; the purge assertion after it is the consequence
    /// and the attribute assertion last is the promise the fix must not break.
    #[test]
    fn deleting_a_read_only_document_still_starts_its_retention() {
        let root = temp_root("trash-stamp-ro");
        let mut doc = blank();
        let path = file_document(&root, None, "Ancient", &mut doc).unwrap();
        let a_year_ago =
            std::time::SystemTime::now() - std::time::Duration::from_secs(365 * 86_400);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(a_year_ago))
            .unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&path, perms).unwrap();
        assert!(
            std::fs::metadata(&path).unwrap().permissions().readonly(),
            "the fixture must actually be read-only, or this test is about nothing"
        );

        trash(&root, &entry_named(&root, "Ancient")).unwrap();
        let gone = trashed(&root);
        assert_eq!(gone.len(), 1, "the delete itself still works");
        assert!(
            clock::since(gone[0].modified) < 60,
            "the trashed copy must be stamped with the deletion"
        );
        assert_eq!(purge_trash(&root, 30), 0, "and survive a 30-day retention");
        assert!(
            std::fs::metadata(&gone[0].path)
                .unwrap()
                .permissions()
                .readonly(),
            "and it is still the protected file the user had"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn purging_with_no_retention_empties_the_trash() {
        let root = temp_root("purge");
        let mut doc = blank();
        file_document(&root, None, "Landing v4", &mut doc).unwrap();
        trash(&root, &entry_named(&root, "Landing v4")).unwrap();
        assert_eq!(purge_trash(&root, 0), 1);
        assert!(trashed(&root).is_empty());
        // And a retention window keeps a fresh deletion.
        let mut other = blank();
        file_document(&root, None, "Pricing table", &mut other).unwrap();
        trash(&root, &entry_named(&root, "Pricing table")).unwrap();
        assert_eq!(purge_trash(&root, 30), 0);
        assert_eq!(trashed(&root).len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Version history is keyed on the document's id, so a rename does not
    /// orphan it — the assertion that makes `DocumentMeta::id` worth having.
    #[test]
    fn pinned_versions_survive_a_rename() {
        let root = temp_root("versions");
        let mut doc = blank();
        file_document(&root, None, "Landing v4", &mut doc).unwrap();
        let id = doc.meta().id.clone().unwrap();
        let entry = entry_named(&root, "Landing v4");
        assert!(pin_version(&root, &entry).unwrap().is_some());
        assert_eq!(versions(&root, &id).len(), 1);

        rename(&entry, "Homepage hero").unwrap();
        assert_eq!(
            versions(&root, &id).len(),
            1,
            "history is keyed on the id, not the filename"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A pin holds the document's bytes.
    ///
    /// ⚠️ **Nothing asserted this until D378 changed how a pin is written** — the
    /// two tests around it count files in `.versions` and never open one. A pin
    /// that wrote the wrong document, or an empty one, would have passed both, and
    /// the failure only shows up when somebody restores a version months later.
    /// Counting is what a test writes when the interesting part felt like
    /// `fs::copy`'s problem.
    ///
    /// ⚠️ **And it does *not* assert atomicity, which cannot be asserted here.**
    /// Flipping `pin_version` back to `fs::copy` leaves this green: both produce
    /// the right file, and the difference is only visible to a process that dies
    /// between the two halves. What covers the mechanism is `atomic`'s own tests;
    /// what this covers is that a pin goes through it with the right bytes.
    #[test]
    fn a_pin_holds_the_documents_bytes() {
        let root = temp_root("pin-bytes");
        let mut doc = blank();
        let path = file_document(&root, None, "Landing v4", &mut doc).unwrap();
        let id = doc.meta().id.clone().unwrap();
        let entry = entry_named(&root, "Landing v4");
        let pinned = pin_version(&root, &entry).unwrap().unwrap();
        assert_eq!(
            std::fs::read(&pinned).unwrap(),
            std::fs::read(&path).unwrap(),
            "a version is the document as it stood, byte for byte"
        );
        assert_eq!(versions(&root, &id), vec![pinned]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ⚠️ Two pins in the same second must both survive. The stamp has
    /// one-second resolution and Ctrl+S twice is a thing people do.
    #[test]
    fn two_pins_in_one_second_do_not_overwrite_each_other() {
        let root = temp_root("pin-twice");
        let mut doc = blank();
        file_document(&root, None, "Landing v4", &mut doc).unwrap();
        let id = doc.meta().id.clone().unwrap();
        let entry = entry_named(&root, "Landing v4");
        pin_version(&root, &entry).unwrap();
        pin_version(&root, &entry).unwrap();
        assert_eq!(versions(&root, &id).len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A document that has never been filed has no history to pin to, which is
    /// an answer rather than an error.
    #[test]
    fn pinning_an_unfiled_document_is_not_an_error() {
        let root = temp_root("pin-unfiled");
        let mut doc = blank();
        let path = root.join("loose.ondin");
        write_document(&path, &doc).unwrap();
        doc.set_meta(DocumentMeta::default());
        let entry = entry_named(&root, "Loose");
        assert!(pin_version(&root, &entry).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
