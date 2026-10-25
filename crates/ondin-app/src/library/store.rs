//! Every write the library makes: filing a new document, renaming one, moving
//! it between projects, deleting it, and pinning a version (§9.5).
//!
//! The file's metadata block is the record and the path is a consequence of it. A
//! rename writes the new name into the document and then renames the file; a move
//! writes the new project id and then moves the file. Deriving the name or
//! project from where the file sits breaks the first time somebody drags
//! something in Explorer.
//!
//! So every operation has two halves, and the second may fail on its own: a
//! rename that cannot rename the file leaves "My cool design" in `old-name.ondin`.
//! That is the failure to land on, because the dashboard reads the block and the
//! next rename fixes the stem; the opposite order leaves a name and contents that
//! disagree with nothing to say which is right.
//!
//! Nothing here is undoable (see `ondin_core::meta`): *Delete* moves to
//! [`super::scan::TRASH_DIR`], the library's only "undo".

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
/// document being renamed — so a rename to the same slug does not walk to `-1`.
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
/// Used by *New file* and *Import* alike. Only missing fields are filled, so an
/// imported document keeps its name and creation date; it always gets the project
/// it was imported into and, if it has none, an id.
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
/// Separate from [`file_document`] because it must not touch the metadata: an
/// autosave is not the moment to re-decide identity or rename the file under the
/// editor.
///
/// Atomic (`crate::atomic`): a plain write truncates the document before writing
/// a byte, leaving a crash with neither version.
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

/// Make a closed document a kit, or stop it sharing — the card menu's *Make a
/// kit* / *Stop sharing as kit* (§5.3e, 1B, 1D). The editor's verbs
/// (`OndinApp::make_kit`, `OndinApp::stop_sharing_kit`) do the same to the
/// open document.
///
/// Turning a mark on keeps its version, so a kit made again continues from its
/// last publish; turning it off keeps the mark and its published state, and
/// only stops the sharing. An unfiled document's identity is minted here as
/// filing mints one, since a kit is found by its id.
pub fn set_kit(entry: &Entry, sharing: bool) -> io::Result<()> {
    let mut doc = load(&entry.path)?;
    let mut meta = doc.meta().clone();
    if sharing {
        if meta.id.is_none() {
            meta.id = Some(ids::mint());
        }
        meta.kit
            .get_or_insert_with(ondin_core::meta::KitMark::default)
            .sharing = true;
    } else {
        match meta.kit.as_mut() {
            Some(kit) => kit.sharing = false,
            None => return Ok(()),
        }
    }
    doc.set_meta(meta);
    write_document(&entry.path, &doc)
}

/// Copy a document into a new one, with a fresh identity.
///
/// A new id and creation stamp, deliberately: a copy that kept the original's id
/// would share its version history and thumbnail cache entry.
///
/// A kit's published state is left behind: it is the kit's last publish, the copy
/// is no kit until made one, and a kept copy would make its first publish compare
/// against a dead set of mains.
///
/// Not `ondin_core::copy_name`, which numbers a name within a set of siblings and
/// returns `None` for a name a person typed; every document name is one, and
/// documents may share a name. Appending " copy" is the whole rule.
pub fn duplicate(root: &Path, entry: &Entry, project: Option<&Project>) -> io::Result<PathBuf> {
    let mut doc = load(&entry.path)?;
    doc.set_meta(DocumentMeta::default());
    doc.set_published(None);
    let name = format!("{} copy", entry.display_name());
    file_document(root, project, &name, &mut doc)
}

/// Move a document to the trash.
///
/// Nothing is deleted, and nothing records where it came from: restoring files it
/// into its project's folder, which the block already names, so the trash needs
/// no index. A document that sat outside its project's folder comes back inside it.
pub fn trash(root: &Path, entry: &Entry) -> io::Result<PathBuf> {
    let dir = root.join(TRASH_DIR);
    std::fs::create_dir_all(&dir)?;
    let target = doc_path(&dir, &free_stem(&dir, &entry.display_name(), None));
    std::fs::rename(&entry.path, &target)?;
    // Best-effort: the delete has already succeeded, so a stamp failure must not
    // be reported as one. The known failure, a read-only document, is handled
    // inside [`trash_stamp`].
    let _ = trash_stamp(&target);
    Ok(target)
}

/// Everything in the trash, oldest deletion first.
///
/// Sorted by modification time, which `fs::rename` does not update, so this is
/// "when last edited" and retention is applied from the stamp written on the way
/// in: see [`trash_stamp`].
pub fn trashed(root: &Path) -> Vec<Entry> {
    let mut out = super::scan::scan_dir(&root.join(TRASH_DIR));
    out.sort_by_key(|e| e.modified);
    out
}

/// The moment a document entered the trash.
///
/// Written into the file's modification time on the way in, because the metadata
/// block may not carry a clock (§5.11a), the trash keeps no index, and
/// `fs::rename` preserves the original mtime. A stamp that is refused is retried
/// rather than shrugged off (§15 D432).
fn trash_stamp(path: &Path) -> io::Result<()> {
    // `File::set_times` needs the file open for writing.
    let touch = |p: &Path| -> io::Result<()> {
        let f = std::fs::OpenOptions::new().write(true).open(p)?;
        f.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now()))
    };
    let Err(refused) = touch(path) else {
        return Ok(());
    };
    // A read-only document: the attribute is cleared for the length of the touch
    // and put straight back. The Windows read-only attribute refuses this open
    // but not the rename into `.trash`; an unstamped file keeps its last-edit
    // mtime and `purge_trash` would delete it at the next sweep. Restored rather
    // than left clear, because restoring must give back the protected file the
    // user had. Setting permissions does not touch the mtime.
    let original = std::fs::metadata(path)?.permissions();
    if !original.readonly() {
        return Err(refused);
    }
    let mut writable = original.clone();
    owner_may_write(&mut writable);
    std::fs::set_permissions(path, writable)?;
    let stamped = touch(path);
    // The original object, not `set_readonly(true)`: on Unix that means 0o444.
    let _ = std::fs::set_permissions(path, original);
    stamped
}

/// Let the file's owner write it, changing nothing else about who may.
///
/// Not `Permissions::set_readonly(false)`: on Unix that means mode `0o666`, so a
/// briefly-writable file would also be briefly world-writable.
#[cfg(unix)]
fn owner_may_write(perms: &mut std::fs::Permissions) {
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(perms.mode() | 0o200);
}

/// Windows has one bit and this is it; see the Unix twin.
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
/// `.versions/<document id>/<unix seconds>.ondin` — keyed on the id rather than
/// the stem, so history survives a rename.
///
/// A document with no id yet has no history to pin to; that is not an error, and
/// the caller files it first.
///
/// Read-then-[`crate::atomic::write`], not `fs::copy`: [`versions`] lists the
/// directory by extension, so a half-copied pin would be listed as a version and
/// fail when restored.
pub fn pin_version(root: &Path, entry: &Entry) -> io::Result<Option<PathBuf>> {
    // `is_minted`, not just `is_some`: the id is joined as a directory name, so
    // a document asserting `"id": "../.."` would control where the copy lands. A
    // malformed id reads as "not filed".
    let Some(id) = entry.meta.id.as_deref().filter(|id| ids::is_minted(id)) else {
        return Ok(None);
    };
    let dir = root.join(VERSIONS_DIR).join(id);
    let stamp = clock::now();
    let mut target = dir.join(format!("{stamp}.{DOCUMENT_EXT}"));
    // Two pins inside the same second would otherwise overwrite the first.
    let mut n = 1;
    while target.exists() {
        target = dir.join(format!("{stamp}-{n}.{DOCUMENT_EXT}"));
        n += 1;
    }
    // `atomic::write` takes a slice and makes `dir` on the way; `exists()` on a
    // missing directory is `false`, the right answer for the loop above.
    let bytes = std::fs::read(&entry.path)?;
    crate::atomic::write(&target, &bytes)?;
    Ok(Some(target))
}

/// A document's pinned versions, newest first — *Version history…*'s list
/// (`versions.rs`).
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
    // By the stamp in the name, numerically — time without a stat per file. A byte
    // sort would put a same-second `S-1` before the `S` it followed. Newest first.
    out.sort_by_key(|p| std::cmp::Reverse(version_stamp(p)));
    out
}

/// A pinned copy's `(unix seconds, same-second counter)`, from the name
/// [`pin_version`] gives it; `None` for any other name.
pub fn version_stamp(path: &Path) -> Option<(u64, u64)> {
    let stem = path.file_stem()?.to_str()?;
    let (secs, n) = stem.split_once('-').unwrap_or((stem, "0"));
    Some((secs.parse().ok()?, n.parse().ok()?))
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

    /// A rename that changes only the casing must not walk to `-1`.
    ///
    /// flip: dropping `keep` from `free_stem` → fails at the path assertion (the
    /// name assertion stays green).
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

    /// A duplicated kit keeps the drawing, mains and all, but neither the mark nor
    /// the published copy.
    ///
    /// flip: `duplicate` without `set_published(None)` → fails at "no publish".
    #[test]
    fn a_duplicated_kit_leaves_its_publish_behind() {
        use ondin_core::{NodeKind, Operation, Transaction};
        let root = temp_root("dup-kit");
        let mut doc = blank();
        let main = ondin_core::IdSource::new(0xC1).mint();
        let parent = doc.first_page();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: main,
                parent,
                index: 0,
                kind: NodeKind::Artboard {
                    size: ondin_core::kurbo::Size::new(24.0, 24.0),
                },
                transform: None,
                name: Some("Badge".into()),
            },
            Operation::SetComponent {
                id: main,
                component: true,
            },
        ]))
        .unwrap();
        let path = file_document(&root, None, "Atlas", &mut doc).unwrap();
        let mut meta = doc.meta().clone();
        meta.kit = Some(ondin_core::meta::KitMark::default());
        doc.set_meta(meta);
        ondin_core::kit::publish(&mut doc, None, 1).unwrap();
        write_document(&path, &doc).unwrap();

        let copy = duplicate(&root, &entry_named(&root, "Atlas"), None).unwrap();
        let copied = load(&copy).unwrap();
        assert!(copied.contains(main), "the drawing, mains and all");
        assert!(copied.meta().kit.is_none(), "no mark");
        assert!(copied.published().is_none(), "no publish");
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

    /// Retention counts from the deletion, not from the last edit: the fixture is
    /// aged a year, because `fs::rename` preserves the mtime and a fresh fixture
    /// is young either way.
    ///
    /// flip: dropping the `trash_stamp` call → fails both assertions here only.
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

    /// A read-only document still gets its deletion stamp, stays protected, and
    /// survives retention. The writable fixture of the test above only proves the
    /// `trash_stamp` call is reached, not that it succeeded.
    ///
    /// flip: taking the read-only retry out of `trash_stamp` → fails at "the
    /// trashed copy must be stamped with the deletion".
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

    /// Versions list newest first by their stamps, a same-second pin after the one it
    /// followed; a byte sort of the names would put `S-1` before `S`.
    /// flip: `versions` sorting by path → fails at the same-second pair.
    #[test]
    fn versions_list_newest_first_including_within_one_second() {
        let root = temp_root("version-order");
        let dir = root
            .join(VERSIONS_DIR)
            .join("0ba9f11ce0ba9f11ce0ba9f11ce0ba9f");
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["100", "200", "200-1", "200-2", "99"] {
            std::fs::write(dir.join(format!("{name}.{DOCUMENT_EXT}")), b"{}").unwrap();
        }
        let names: Vec<String> = versions(&root, "0ba9f11ce0ba9f11ce0ba9f11ce0ba9f")
            .iter()
            .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["200-2", "200-1", "200", "100", "99"]);
        assert_eq!(version_stamp(Path::new("200-2.ondin")), Some((200, 2)));
        assert_eq!(version_stamp(Path::new("notes.ondin")), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A pin holds the document's bytes.
    ///
    /// The other pin tests only count files; this opens one. It does not assert
    /// atomicity (`pin_version` as `fs::copy` stays green); `atomic`'s own tests
    /// cover the mechanism.
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

    /// Two pins in the same second both survive (the stamp has one-second
    /// resolution).
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
