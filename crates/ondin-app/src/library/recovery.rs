//! Crash recovery: a snapshot of the open document, taken beside the library
//! rather than into it (§15 D377).
//!
//! **This closes the gap autosave cannot.** Autosave runs on an interval the user
//! can set to zero and refuses to file a never-saved document
//! (`OndinApp::autosave_tick`); together a crash can cost everything.
//!
//! **A snapshot is not a document**: it goes into `.recovery/`, which
//! [`super::scan`] skips, so it never appears in the library. That is what lets
//! it run on a short interval the user cannot turn off and cover the unfiled
//! document.
//!
//! **The write goes through [`crate::atomic`]**: a half-written snapshot is a
//! recovery offer that fails when accepted.
//!
//! **A snapshot is in-flight work for one machine and one run**, yet it lives in
//! the base folder beside `.versions` and `.trash` so the library stays one
//! directory to back up or sync; [`super::relocate`] carries it along.

use super::ids;
use super::scan::{self, DOCUMENT_EXT, Entry};
use ondin_core::Document;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Where snapshots live, relative to the root.
///
/// Dot-prefixed so [`super::scan`] skips it.
pub const RECOVERY_DIR: &str = ".recovery";

/// How often a snapshot is taken, at most, while the document is dirty.
///
/// A constant, not `prefs.autosave_secs`: this writes a scratch file the user
/// never sees, and `autosave_secs = 0` means off (§15 D364), which is exactly
/// when a crash costs the most. Ten seconds is the worst case promised.
pub const SNAPSHOT_SECS: u64 = 10;

/// One snapshot from a previous run, waiting to be answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    /// The filename stem — a document id, or a minted key for one never filed.
    pub key: String,
    /// The snapshot itself.
    pub path: PathBuf,
    /// What to call it, from the snapshot's own metadata block.
    pub name: String,
    /// The document this belongs to, when the key still names one in the
    /// library. `None` is a document that was never filed.
    pub target: Option<PathBuf>,
    /// When the snapshot was written, in seconds since the epoch — `None` where
    /// the file's stamp could not be read (§15 D716).
    ///
    /// `None` is offered, not dropped: the recovery card loses its "from N hours
    /// ago" clause rather than inventing a time (`0` or `now` would mislead).
    pub written: Option<u64>,
}

/// Everything the app needs to hold between frames.
///
/// One struct because the fields are only correct together: the key names the
/// file the revision was captured into.
#[derive(Default)]
pub struct RecoveryState {
    /// The key this session's snapshot is filed under, once one has been taken.
    pub key: Option<String>,
    /// The `EditorSession::revision` the last snapshot captured.
    ///
    /// Stops a dirty, idle session rewriting the same bytes every ten seconds (a
    /// file change every sync client picks up). `is_dirty` alone is not enough: it
    /// stays true from the first edit until the next save.
    pub at: Option<u64>,
    /// When the last snapshot was taken. `None` is "never", which means the next
    /// tick writes.
    pub last: Option<Instant>,
    /// Snapshots from a previous run, newest last so the modal can `pop`.
    pub pending: Vec<Pending>,
    /// Whether the user has already been told that snapshots cannot be written.
    ///
    /// A latch: the failure is said once, not every ten seconds.
    pub warned: bool,
}

/// Write `doc` as the snapshot for `key`.
///
/// Through [`crate::atomic::write`], which also makes `.recovery/`.
///
/// ⚠️ **The key is checked before it is joined.** `OndinApp::recovery_tick` takes
/// it from the open document's own `meta.id`, so an id like `../my-cool-design`
/// would rename hostile bytes over the user's real document every ten seconds;
/// on Windows an absolute id discards the base outright. [`ids::is_minted`] says
/// why the guard is here as well as at the loader.
pub fn write(root: &Path, key: &str, doc: &Document) -> io::Result<PathBuf> {
    if !ids::is_minted(key) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a snapshot key must be a minted id",
        ));
    }
    let bytes = ondin_core::io::save(doc).map_err(io::Error::other)?;
    let target = root
        .join(RECOVERY_DIR)
        .join(format!("{key}.{DOCUMENT_EXT}"));
    crate::atomic::write(&target, &bytes)?;
    Ok(target)
}

/// Forget the snapshot for `key`. Missing is success — the caller is saying the
/// work is safe, not that a file exists.
pub fn remove(root: &Path, key: &str) {
    // The same guard as [`write`]: an escaping key would make *Discard* delete a
    // file the user did not point at. It cannot strand a snapshot, since keys come
    // from [`write`] or from a `file_stem` inside `.recovery/`.
    if !ids::is_minted(key) {
        return;
    }
    let path = root
        .join(RECOVERY_DIR)
        .join(format!("{key}.{DOCUMENT_EXT}"));
    let _ = std::fs::remove_file(&path);
    // Temp files too: elsewhere a dead process's `.writing` is swept by a later
    // save, age-gated by an hour (§15 D548), but a discarded key is never written
    // again. `forget` clears every temp for the path unconditionally.
    crate::atomic::forget(&path);
}

/// Every snapshot worth offering, oldest first — so a caller answering them one
/// at a time by `pop` sees the newest first.
///
/// `entries` is the library, used to find the document a key belongs to.
///
/// A snapshot the document already contains is dropped, which keeps a quit
/// quiet. Two tests, neither sufficient alone: the document is **strictly newer**,
/// or the two are **byte-identical**.
///
/// ⚠️ The byte test covers a real hole: `Entry::modified` is whole seconds, so
/// "save, edit, crash" inside one second ties. Dropping every tie would discard
/// that work; offering every tie would prompt after an ordinary save-then-quit.
pub fn pending(root: &Path, entries: &[Entry]) -> Vec<Pending> {
    let dir = root.join(RECOVERY_DIR);
    let Ok(read) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for file in read.flatten() {
        let path = file.path();
        if path.extension().and_then(|e| e.to_str()) != Some(DOCUMENT_EXT) {
            continue;
        }
        let Some(key) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else {
            continue;
        };
        let Ok(meta) = file.metadata() else { continue };
        let written = modified_secs(&meta);
        let target = entries
            .iter()
            .find(|e| e.meta.id.as_deref() == Some(key.as_str()));
        if let Some(entry) = target {
            // Neither sentinel may decide to delete (§15 D716): this branch removes
            // the only copy of unsaved work. An unreadable clock (`None`) settles
            // nothing and the bytes decide; the bytes count as equal only when
            // both reads succeed, since two failed reads must not compare equal
            // (`read_meta` below deletes nothing for an unreadable file either).
            // Read only when the clock has not already settled it.
            let document_is_newer = written.is_some_and(|w| entry.modified > w);
            let not_older = written.is_none_or(|w| entry.modified >= w);
            let identical = not_older
                && matches!(
                    (std::fs::read(&entry.path), std::fs::read(&path)),
                    (Ok(a), Ok(b)) if a == b
                );
            if document_is_newer || identical {
                let _ = std::fs::remove_file(&path);
                continue;
            }
        }
        // The snapshot names itself the way the library does.
        let name = match scan::read_meta(&path) {
            Some((meta, _)) => scan::display_name(&meta, &key),
            // Unreadable: the crash landed mid-write and the rename did not
            // happen, or the file is not ours. Nothing to offer.
            None => continue,
        };
        out.push(Pending {
            key,
            path,
            name,
            target: target.map(|e| e.path.clone()),
            written,
        });
    }
    out.sort_by_key(|p| p.written);
    out
}

/// A key for a document that has none — one that has never been filed.
///
/// Minted, not derived: an unfiled document has no id, name or path, so this is
/// its session's name for it.
pub fn mint_key() -> String {
    ids::mint()
}

/// When `meta`'s file was written, or `None` where the clock has nothing to say.
///
/// ⚠️ **`None` rather than `0`** (§15 D716): zero would compare older than every
/// document and [`pending`] would delete the snapshot; `Option` cannot be fed to
/// `>` by accident.
///
/// Both failures are real: `modified()` is `Err` where the stamp is not kept, and
/// `duration_since(UNIX_EPOCH)` is `Err` for a pre-1970 stamp, the one
/// `a_snapshot_with_an_unreadable_stamp_is_offered_rather_than_deleted` stages.
fn modified_secs(meta: &std::fs::Metadata) -> Option<u64> {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::store;
    use ondin_core::IdSource;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ondin-recovery-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn blank() -> Document {
        let mut ids = IdSource::new(0xD0C);
        Document::new(ids.mint())
    }

    /// A snapshot key with the shape `ids::mint` produces, the only shape the
    /// writers accept.
    const DOC_KEY: &str = "5d0c1a7b93e24f6081c5aa3fe7b20d14";
    /// A second one, for a snapshot no document in the library claims.
    const ORPHAN_KEY: &str = "0ba9f11ce0ba9f11ce0ba9f11ce0ba9f";

    /// A snapshot round-trips, replaces itself in place, and leaves no partial
    /// file behind.
    ///
    /// The temp check asks `atomic::leftover_temps` rather than spelling a temp
    /// name out, so it cannot go vacuous when the temp naming scheme changes.
    #[test]
    fn a_snapshot_replaces_itself_and_leaves_no_temp_file() {
        let root = temp_root("write");
        let mut doc = blank();
        let mut meta = doc.meta().clone();
        meta.name = Some("Landing v4".into());
        doc.set_meta(meta);

        let at = write(&root, DOC_KEY, &doc).unwrap();
        assert!(at.exists());
        let first = std::fs::read(&at).unwrap();

        let mut second_doc = blank();
        let mut meta = second_doc.meta().clone();
        meta.name = Some("Landing v5".into());
        second_doc.set_meta(meta);
        write(&root, DOC_KEY, &second_doc).unwrap();
        assert_ne!(
            std::fs::read(&at).unwrap(),
            first,
            "a second snapshot replaces the first rather than sitting beside it"
        );
        assert!(
            crate::atomic::leftover_temps(&at).is_empty(),
            "the temp file is renamed, not copied — a leftover means the file on \
             disk is not the one just written"
        );

        remove(&root, DOC_KEY);
        assert!(!at.exists());
        // Missing is success: the caller is saying the work is safe.
        remove(&root, DOC_KEY);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A snapshot for a document the library already holds at or past that
    /// moment is not offered — and is cleaned up rather than left to be asked
    /// about again.
    ///
    /// Keeps a clean quit quiet. The second half is the same-second tie with
    /// identical content.
    ///
    /// Coverage finding: deleting the byte comparison and keeping `>=` leaves this
    /// test green (the tie is dropped either way); the other half of the rule is
    /// pinned by `app::library_wiring_tests::work_lost_to_a_crash_comes_back_…`.
    #[test]
    fn a_snapshot_older_than_its_document_is_dropped_rather_than_offered() {
        let root = temp_root("stale");
        let mut doc = blank();
        let path = store::file_document(&root, None, "Landing v4", &mut doc).unwrap();
        let id = doc.meta().id.clone().unwrap();
        let entries = scan::scan(&root);
        assert_eq!(entries.len(), 1, "the fixture is one filed document");

        // Every stamp is set: a second boundary crossing between two writes makes
        // a snapshot strictly newer than the document, which `pending` rightly
        // offers, so the tie is arranged rather than raced for. Setting the
        // time needs the handle open for writing.
        let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();
        let restamp = |at: &Path, to: std::time::SystemTime| {
            std::fs::File::options()
                .write(true)
                .open(at)
                .unwrap()
                .set_modified(to)
                .unwrap();
        };

        // A snapshot for a document nobody has, which is the unfiled case.
        let orphan = write(&root, ORPHAN_KEY, &doc).unwrap();
        restamp(&orphan, stamp);
        let waiting = pending(&root, &entries);
        assert_eq!(
            waiting.iter().map(|p| p.key.as_str()).collect::<Vec<_>>(),
            vec![ORPHAN_KEY],
            "a snapshot with no document behind it is exactly what recovery is for"
        );
        assert_eq!(
            waiting[0].name, "Landing v4",
            "it names itself from its own \
             metadata block, not from its key"
        );
        assert_eq!(waiting[0].target, None);

        // And one for the document that is already on disk, written in the same
        // second the file was — the tie the byte comparison exists for.
        let snapshot = write(&root, &id, &doc).unwrap();
        restamp(&snapshot, stamp);
        let waiting = pending(&root, &entries);
        assert_eq!(
            waiting.iter().map(|p| p.key.as_str()).collect::<Vec<_>>(),
            vec![ORPHAN_KEY],
            "the document already holds this, so there is nothing to recover"
        );
        assert!(
            !snapshot.exists(),
            "and it is cleaned up rather than left to be asked about every launch"
        );

        // The same snapshot two seconds newer is offered.
        let snapshot = write(&root, &id, &doc).unwrap();
        restamp(&snapshot, stamp + std::time::Duration::from_secs(2));
        let waiting = pending(&root, &entries);
        assert_eq!(
            waiting.iter().map(|p| p.key.as_str()).collect::<Vec<_>>(),
            vec![ORPHAN_KEY, id.as_str()],
            "a snapshot written after the document is work the document has not got"
        );
        assert_eq!(
            waiting[1].target.as_deref(),
            Some(path.as_path()),
            "and it knows which document it would be recovered over"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A snapshot whose stamp cannot be read is offered, not deleted (§15 D716).
    ///
    /// The bytes must differ, or the test is about nothing: with the stamp
    /// refusing to decide, the byte comparison settles it, so the fixture edits
    /// the document after filing it and asserts the files differ first. The
    /// sibling case (two failed reads comparing equal) is closed by `(Ok, Ok)` in
    /// `pending` and not staged here.
    #[test]
    fn a_snapshot_with_an_unreadable_stamp_is_offered_rather_than_deleted() {
        let root = temp_root("no-stamp");
        let mut doc = blank();
        let path = store::file_document(&root, None, "Landing v4", &mut doc).unwrap();
        let id = doc.meta().id.clone().unwrap();
        let entries = scan::scan(&root);
        assert_eq!(entries.len(), 1, "the fixture is one filed document");

        // The unsaved work: the snapshot must differ from the document.
        let mut meta = doc.meta().clone();
        meta.name = Some("Landing v4 — with the bit that was never saved".into());
        doc.set_meta(meta);
        let snapshot = write(&root, &id, &doc).unwrap();
        assert_ne!(
            std::fs::read(&snapshot).unwrap(),
            std::fs::read(&path).unwrap(),
            "the fixture holds work the document has not got"
        );

        // A pre-epoch stamp: `duration_since(UNIX_EPOCH)` is `Err`.
        std::fs::File::options()
            .write(true)
            .open(&snapshot)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH - std::time::Duration::from_secs(1))
            .unwrap();

        let waiting = pending(&root, &entries);
        assert!(
            snapshot.exists(),
            "an undatable snapshot must not be deleted — this is the user's only \
             copy of work that never reached the file"
        );
        assert_eq!(
            waiting.iter().map(|p| p.key.as_str()).collect::<Vec<_>>(),
            vec![id.as_str()],
            "and it has to actually be offered, not merely left on disk"
        );
        assert_eq!(
            waiting[0].written, None,
            "with the stamp reported as unknown rather than invented"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A snapshot key that is a **path** writes nothing, anywhere.
    ///
    /// A hostile `meta.id` such as `../my-cool-design` must not let `write`
    /// rename bytes over a real document. Both `Path::join` behaviours are
    /// asserted since they fail differently: `..` escapes, and on Windows an
    /// absolute component discards the base. This is the belt to
    /// `DocumentMeta::sanitized` at the loader and `io::probe`, since
    /// `scan::read_meta` is another reader.
    ///
    /// flip: removing the `is_minted` guard fails the first assertion, with
    /// `victim.ondin` holding the snapshot's bytes rather than `ORIGINAL`.
    #[test]
    fn a_key_that_is_a_path_is_refused() {
        let root = temp_root("escape");
        let doc = blank();

        // A file next to the library that the key would reach over. Pid-salted
        // itself: it lives in `temp_dir()`, outside the salted root, and
        // concurrent runs would otherwise delete each other's file.
        let victim = format!("ondin-recovery-victim-{}", std::process::id());
        let outside = root.parent().unwrap().join(format!("{victim}.ondin"));
        let key = format!("../{victim}");
        std::fs::write(&outside, b"ORIGINAL").unwrap();

        let err = write(&root, &key, &doc).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(
            std::fs::read(&outside).unwrap(),
            b"ORIGINAL",
            "a key that climbs out of .recovery/ writes nothing"
        );

        // An absolute component makes `Path::join` discard the base (Windows).
        let absolute =
            std::env::temp_dir().join(format!("ondin-recovery-absolute-{}", std::process::id()));
        assert!(write(&root, absolute.to_str().unwrap(), &doc).is_err());
        assert!(
            !absolute.with_extension("ondin").exists(),
            "an absolute key discards the library root and must be refused too"
        );

        // And `remove`, which would otherwise delete rather than overwrite.
        std::fs::write(&outside, b"ORIGINAL").unwrap();
        remove(&root, &key);
        assert!(
            outside.exists(),
            "and Discard cannot delete out of the folder"
        );

        // The control: a well-formed key still writes, or the guard is a lock.
        assert!(write(&root, DOC_KEY, &doc).is_ok());

        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&root);
    }
}
