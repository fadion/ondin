//! Crash recovery: a snapshot of the open document, taken beside the library
//! rather than into it (§15 D377).
//!
//! **What this is for is the gap autosave cannot close.** Autosave writes the
//! *document*, on an interval the user sets and can set to zero, and it
//! deliberately refuses to file a document that has never been saved
//! (`OndinApp::autosave_tick`) — creating a file on a timer for a canvas someone
//! was doodling on puts something in their library they did not ask for. Both
//! rules are right, and together they leave a crash costing anything from a few
//! seconds to everything.
//!
//! **A snapshot answers that without breaking either rule**, because it is not a
//! document: it goes into `.recovery/`, which [`super::scan`] skips like every
//! other dot-directory, so nothing it writes can appear in the library, be
//! opened by accident, or be counted by a heading. That is what lets it run on a
//! short interval the user cannot turn off, and what lets it cover the unfiled
//! document autosave will not touch.
//!
//! **The write goes through [`crate::atomic`]**, because a file whose entire
//! purpose is surviving a crash cannot be written in a way a crash can corrupt:
//! a half-written snapshot is worse than none, being a recovery offer that fails
//! when accepted, at the one moment the user is already unhappy.
//!
//! ⚠️ **This module argued that alone for about an hour, and it was the wrong
//! shape of an argument that applied to everything.** The paragraph here used to
//! say "…where the document's own save is a plain `fs::write`", and note that
//! `store::write_document` had the same exposure and was deliberately not
//! changed. It was raised as an open question rather than left silently
//! inconsistent, the maintainer said *make every save atomic*, and now the
//! document, `projects.json`, the preferences and the per-machine index all go
//! through the same function (§15 D378). What is left of the original point is
//! only that this file got there first.
//!
//! **Nothing here is a fact about a document**, which is why the module's
//! standing question — *does it travel?* — has an unusual answer. A snapshot is
//! in-flight work belonging to one machine and one run. It lives in the base
//! folder anyway, beside `.versions` and `.trash`, so that the whole library is
//! one directory to back up, move or point a sync client at; and
//! [`super::relocate`] carries it along for the same reason.

use super::ids;
use super::scan::{self, DOCUMENT_EXT, Entry};
use ondin_core::Document;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Where snapshots live, relative to the root.
///
/// Dot-prefixed for [`super::scan`]'s reason and one more: a user who opens the
/// base folder should see their documents, not the machinery that protects them.
pub const RECOVERY_DIR: &str = ".recovery";

/// How often a snapshot is taken, at most, while the document is dirty.
///
/// **A constant rather than a preference, and not `prefs.autosave_secs`.**
/// Autosave's interval is the user's to choose because autosave writes *their*
/// file; this writes a scratch file they will never see, so the only thing a
/// setting could buy is the chance to turn off the thing that saves them. Ten
/// seconds is the worst case this feature promises, and it is decoupled from
/// autosave on purpose: `autosave_secs = 0` means **off** (§15 D364), and off is
/// exactly when a crash costs the most.
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
    /// library. `None` is a document that was never filed — which is a real
    /// case, not a broken one.
    pub target: Option<PathBuf>,
    /// When the snapshot was written, in seconds since the epoch — `None` where
    /// the file's stamp could not be read (§15 D716).
    ///
    /// ⚠️ **`None` is offered, not dropped**, so the recovery card has to say
    /// something for it. It says nothing: the sentence loses its *"from N hours
    /// ago"* clause rather than inventing a time, because the two available
    /// fudges are both lies the user would act on — `0` reads as *"56 years
    /// ago"* and `now` reads as *"just now"* about a file that may be a week old.
    pub written: Option<u64>,
}

/// Everything the app needs to hold between frames.
///
/// **One struct rather than four fields on `OndinApp`**, because they are only
/// ever correct together: the key names the file the revision was captured into,
/// and clearing one without the other is how a snapshot outlives the session
/// that owns it.
#[derive(Default)]
pub struct RecoveryState {
    /// The key this session's snapshot is filed under, once one has been taken.
    pub key: Option<String>,
    /// The `EditorSession::revision` the last snapshot captured.
    ///
    /// ⚠️ **This is what stops a dirty, idle session rewriting the same bytes
    /// every ten seconds** — which is invisible locally and is a file change
    /// every sync client in the world will pick up, the same trap
    /// `autosave_tick`'s `is_dirty` gate exists for one level up. `is_dirty`
    /// alone is not enough here: it stays true from the first edit until the
    /// next save, so a user who edits once and then thinks for five minutes
    /// would produce thirty identical writes.
    pub at: Option<u64>,
    /// When the last snapshot was taken. `None` is "never", which means the next
    /// tick writes.
    pub last: Option<Instant>,
    /// Snapshots from a previous run, newest last so the modal can `pop`.
    pub pending: Vec<Pending>,
    /// Whether the user has already been told that snapshots cannot be written.
    ///
    /// ⚠️ **A latch, because the alternative is a status line every ten
    /// seconds.** The failure is worth saying *once*: crash recovery that
    /// silently does not work is the kind of thing a person only discovers at
    /// the worst possible moment.
    pub warned: bool,
}

/// Write `doc` as the snapshot for `key`.
///
/// Through [`crate::atomic::write`], which also makes `.recovery/` if it is not
/// there — see the module note.
///
/// ⚠️ **The key is checked before it is joined**, and the reason is that this is
/// the writer with the worst reach. `OndinApp::recovery_tick` takes the key from
/// the open document's own `meta.id`, so a `.ondin` whose block reads
/// `"id": "../my-cool-design"` made this build `root/.recovery/../…` and
/// `atomic::write` rename attacker-chosen bytes over the user's real document —
/// every ten seconds, silently, and atomically enough that there is nothing torn
/// to notice. On Windows an absolute id discards the base outright.
/// [`ids::is_minted`] says why this guard is here as well as at the loader.
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
    // The same guard as [`write`], for the same reason one level down: a key
    // that escapes `.recovery/` would make *Discard* delete a file the user did
    // not point at.
    //
    // ⚠️ **It cannot strand a snapshot**, which is the objection to check before
    // believing this is free. The only keys ever passed here come from
    // [`RecoveryState::key`] — written by [`write`], which refuses anything else
    // — or from [`Pending::key`], which is a `file_stem` of a file *inside*
    // `.recovery/` and therefore already a single component. A file this build
    // cannot delete is one no build could have written.
    if !ids::is_minted(key) {
        return;
    }
    let path = root
        .join(RECOVERY_DIR)
        .join(format!("{key}.{DOCUMENT_EXT}"));
    let _ = std::fs::remove_file(&path);
    // ⚠️ **And the temp files, which no other caller of `atomic::write` bothers
    // with.** Elsewhere a `.writing` left by a dead process is swept by a later
    // save into that directory; here that sweep is age-gated by an hour (§15
    // D548), and *Discard*'s whole point is that this key is never written again.
    // A snapshot the user threw away must not leave a readable copy of it in the
    // folder for an hour, or at all. `forget` clears every temp for the path
    // unconditionally, which is exactly this caller's meaning.
    crate::atomic::forget(&path);
}

/// Every snapshot worth offering, oldest first — so a caller answering them one
/// at a time by `pop` sees the newest first.
///
/// `entries` is the library, used to find the document a key belongs to.
///
/// ⚠️ **A snapshot the document already contains is dropped rather than offered**,
/// which is what keeps a quit quiet, and it takes **two** tests because neither
/// is sufficient alone:
///
/// - the document is **strictly newer** — it has everything the snapshot has and
///   whatever came after, so there is nothing to recover;
/// - or the two are **byte-identical**, which is the same conclusion reached
///   without trusting a clock.
///
/// ⚠️ **The byte test is not belt-and-braces; it is the one that covers a real
/// hole.** `Entry::modified` is whole seconds, so a save and a snapshot inside
/// the same second compare equal — and "save, edit, crash" *is* a thing that
/// happens inside one second. A rule that dropped every tie would silently throw
/// that work away, and a rule that offered every tie would prompt after an
/// ordinary "save, then quit". Comparing the bytes decides it on what the files
/// actually say instead of on which side of a tie to guess.
///
/// The read costs two files, and only for a snapshot whose document is not
/// already older — which after a crash is the uncommon case, and there is at
/// most a handful of snapshots ever.
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
            // 🚨 **Neither sentinel may decide to delete** (§15 D716,
            // `[S1.2-L1-06]`). Both of these read the conservative answer the
            // unsafe way round, and what is on the other side of the branch is
            // the permanent removal of the only copy of the user's unsaved work.
            //
            // **The clock refuses rather than guesses.** `modified_secs` used to
            // end `.unwrap_or(0)`, so an unreadable or pre-epoch stamp became
            // "older than everything" and `entry.modified > written` deleted it
            // on the spot. `None` now means *the clock has nothing to say*, so it
            // settles nothing in either direction and the bytes decide.
            //
            // **And the bytes have to be read to count as equal.**
            // `read(a).ok() == read(b).ok()` made two `None`s compare *equal*, so
            // one lock catching both files in a single AV or sync sweep read as
            // byte-identical and deleted the snapshot. The comment here called
            // that *"the right answer by luck and by intent: neither file can be
            // read, so there is nothing to offer either way"* — which holds only
            // if deleting is not part of the answer, and it is. Five lines down,
            // `read_meta` answering `None` for the identical reason `continue`s
            // and deletes **nothing**: the two branches sat eight lines apart
            // disagreeing about what an unreadable file means.
            //
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
        // The snapshot is a whole document, so it names itself — and it names
        // itself the same way the library does, through the one function that
        // turns a metadata block and a stem into a name.
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
/// **Minted rather than derived from anything.** An unfiled document has no id,
/// no name and no path; the only thing that distinguishes it is the session it
/// is open in, and this is that session's name for it.
pub fn mint_key() -> String {
    ids::mint()
}

/// When `meta`'s file was written, or `None` where the clock has nothing to say.
///
/// ⚠️ **`None` rather than `0`** (§15 D716). This ended `.unwrap_or(0)`, and
/// zero is not a neutral value here — it is *the beginning of time*, so an
/// unreadable stamp compared as older than every document and
/// [`pending`]'s branch deleted the snapshot. **A sentinel that has to be
/// compared is a sentinel that will be compared wrongly**; the type is what
/// stops it, because `Option` cannot be fed to `>` by accident.
///
/// Both doors are real. `modified()` is `Err` on a platform or filesystem that
/// does not keep the stamp, and `duration_since(UNIX_EPOCH)` is `Err` for any
/// file dated before 1970 — which a backup or sync client restoring a file with
/// a bogus stamp produces, and which is the case
/// `a_snapshot_with_an_unreadable_stamp_is_offered_rather_than_deleted` stages,
/// being the one of the two that can be arranged portably.
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

    /// A snapshot key with the shape `ids::mint` produces.
    ///
    /// ⚠️ **These read `"doc-1"` and `"orphan-key"` until `write` started
    /// checking**, and the readability was worth something — but a key that
    /// could never occur is a fixture testing a path the app cannot take. They
    /// are now what a real key looks like, which is also the only shape the
    /// writers accept, and the difference is why `a_key_that_is_a_path_is_refused`
    /// exists beside them.
    const DOC_KEY: &str = "5d0c1a7b93e24f6081c5aa3fe7b20d14";
    /// A second one, for a snapshot no document in the library claims.
    const ORPHAN_KEY: &str = "0ba9f11ce0ba9f11ce0ba9f11ce0ba9f";

    /// A snapshot round-trips, replaces itself in place, and leaves no partial
    /// file behind.
    ///
    /// ⚠️ **The third assertion moved when the write became shared, and the move
    /// is the point.** It named `<key>.writing`, which was this module's own temp
    /// name; `crate::atomic` derives its temp from the whole path, so the file
    /// became `<key>.ondin.writing` — and the assertion as written would have gone
    /// on passing while checking for a file that could no longer exist under any
    /// circumstances. It asks `crate::atomic` now, which is the only version of it
    /// that cannot rot. The mechanism it is about is unchanged: a temp left behind
    /// after a *successful* write means the rename did not happen and the snapshot
    /// on disk is whatever was there before.
    ///
    /// ⚠️ **And the scheme changed a second time — §15 D548 made the name unique
    /// per write — which is the warning being paid off rather than a new one.**
    /// The question is now *"is there any temp for this path"* rather than *"does
    /// this one name exist"*, so it asks `atomic::leftover_temps`. Had the
    /// assertion still spelled a name out it would have been vacuous for the
    /// second time in one file.
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
    /// ⚠️ **This is what keeps a clean quit quiet**, and it is a property of the
    /// *stamps* rather than of the contents: nothing here compares bytes, because
    /// the snapshot and the document are only ever byte-equal by coincidence — a
    /// save writes the document and the snapshot was taken up to ten seconds
    /// earlier.
    ///
    /// ⚠️ **The rule this asserts was changed by a test one level up, which is
    /// the useful part of its history.** It was `entry.modified >= written` and
    /// nothing else — drop every tie — and this test passed. What failed was
    /// `app::library_wiring_tests::work_lost_to_a_crash_comes_back_…`, whose
    /// fixture saves and then snapshots inside the same second and so had its
    /// recovery silently thrown away. **That is not a test artefact**: "save,
    /// edit, crash" happens inside one second in real use too. The tie is now
    /// decided by comparing the bytes, and this test's second half is the same
    /// second with *identical* content, which is the other side of it.
    ///
    /// **Flip-checked** by deleting the byte comparison and keeping `>=`: the
    /// second assertion still passes — the tie is dropped either way — and it is
    /// the app-level test that fails. Which says this file's coverage of the rule
    /// is only half of it, and where the other half lives.
    #[test]
    fn a_snapshot_older_than_its_document_is_dropped_rather_than_offered() {
        let root = temp_root("stale");
        let mut doc = blank();
        let path = store::file_document(&root, None, "Landing v4", &mut doc).unwrap();
        let id = doc.meta().id.clone().unwrap();
        let entries = scan::scan(&root);
        assert_eq!(entries.len(), 1, "the fixture is one filed document");

        // ⚠️ **Every stamp in this test is *set*, and it was intermittently red
        // until they were — reproduced deterministically before it was fixed, by
        // stamping the second snapshot one second on: it fails exactly as the wild
        // run did, at the tie assertion, with the document's own key alongside the
        // orphan's.** `entries` holds the document's mtime in whole seconds
        // and [`pending`] compares each snapshot's against it, so a second boundary
        // crossing between two writes — a coin toss on a cold `cargo test`, and how
        // this surfaced — makes a snapshot strictly *newer* than the document,
        // which is a state `pending` is right to offer. The tie is the case this
        // test is about, so it is arranged rather than raced for. `File::set_times`
        // needs the handle open for writing, which is the only reason for the
        // `options()` dance.
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

        // ⚠️ **And the same snapshot two seconds *newer* is offered**, which is what
        // makes the stamping above load-bearing rather than tidiness: without it
        // this test passed or failed on which side of a second boundary two writes
        // landed, and *both* outcomes are correct behaviour for the state the clock
        // happened to produce — so the red run was never evidence of a bug, and the
        // green one was never evidence of the rule.
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

    /// **A snapshot whose stamp cannot be read is offered, not deleted**
    /// (§15 D716, `[S1.2-L1-06]`).
    ///
    /// `modified_secs` used to end `.unwrap_or(0)`, so a file whose `modified()`
    /// is unreadable — or whose stamp predates the epoch, which a backup or sync
    /// client restoring a file can produce — came back as `written = 0`. Then
    /// `entry.modified > written` is trivially true and the branch
    /// **`remove_file`s it**. *A sentinel meaning "unknown" was being read as
    /// "older than everything"*, and the answer to "older than everything" here
    /// is to delete the user's unsaved work, permanently, at the one moment it
    /// matters: `open_at_launch` runs this over the whole of `.recovery/`
    /// immediately after a crash.
    ///
    /// ⚠️ **The bytes must differ, or this test is about nothing.** With the
    /// stamp refusing to decide, the byte comparison is what settles it — that is
    /// the fix, not a hole in it — so the fixture edits the document after
    /// filing it and asserts the two files differ before asking `pending`
    /// anything. Written the obvious way, with the snapshot a copy of what was
    /// filed, it is deleted for the *right* reason and stays green through the
    /// bug.
    ///
    /// **Its sibling half is `identical`**, which was
    /// `read(a).ok() == read(b).ok()` — two `None`s compare equal, so a lock
    /// catching both files in one sweep read as *byte-identical* and deleted the
    /// snapshot. Its comment called that *"the right answer by luck and by
    /// intent"*, which is true only if deleting is not part of the answer; five
    /// lines down, `read_meta` returning `None` for the very same reason
    /// `continue`s and deletes nothing. That half needs two simultaneous read
    /// failures and is not staged here — it is closed by construction, the
    /// comparison now requiring `(Ok, Ok)`.
    #[test]
    fn a_snapshot_with_an_unreadable_stamp_is_offered_rather_than_deleted() {
        let root = temp_root("no-stamp");
        let mut doc = blank();
        let path = store::file_document(&root, None, "Landing v4", &mut doc).unwrap();
        let id = doc.meta().id.clone().unwrap();
        let entries = scan::scan(&root);
        assert_eq!(entries.len(), 1, "the fixture is one filed document");

        // The unsaved work: the snapshot has to say something the document does
        // not, or the byte test is entitled to drop it.
        let mut meta = doc.meta().clone();
        meta.name = Some("Landing v4 — with the bit that was never saved".into());
        doc.set_meta(meta);
        let snapshot = write(&root, &id, &doc).unwrap();
        assert_ne!(
            std::fs::read(&snapshot).unwrap(),
            std::fs::read(&path).unwrap(),
            "the fixture holds work the document has not got"
        );

        // A stamp before the epoch: `duration_since(UNIX_EPOCH)` is `Err`, which
        // is the same door an unreadable `modified()` comes through.
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
    /// ⚠️ **The reachable version of this is not exotic and needs no click.**
    /// `OndinApp::recovery_tick` takes the key from the open document's own
    /// `meta.id`, verbatim, so a `.ondin` whose block reads
    /// `"id": "../my-cool-design"` made `write` build
    /// `root/.recovery/../my-cool-design.ondin` and `crate::atomic::write`
    /// rename the hostile document's bytes over the user's real one — every ten
    /// seconds for as long as the file was open and dirty, with no dialog, and
    /// atomically enough that there was never a torn file to notice. The base
    /// folder is documented as somewhere to point a sync client, so a file
    /// arriving there is the designed case.
    ///
    /// **Both halves of `Path::join`'s behaviour are asserted**, because they
    /// fail differently: `..` escapes the directory, and on Windows an
    /// *absolute* component discards the base entirely, which is the one a
    /// reader is least likely to predict.
    ///
    /// The real fix is one level up — `ondin_core::DocumentMeta::sanitized`
    /// drops a malformed id at the loader *and* at `io::probe`, so the value
    /// never reaches here. This is the belt to that pair of braces, and the
    /// reason it is worth having is that the load path is not the only reader:
    /// `library::scan::read_meta` answers from the prefix probe and feeds
    /// `cover::key` for every card the dashboard draws.
    ///
    /// Flip-check, run: with the `is_minted` guard removed, the first assertion
    /// fails with `victim.ondin` holding the snapshot's bytes rather than
    /// `ORIGINAL`.
    #[test]
    fn a_key_that_is_a_path_is_refused() {
        let root = temp_root("escape");
        let doc = blank();

        // A file next to the library that the key would reach over.
        //
        // 🚨 **Process-unique, and it was not.** `temp_root` salts the *root* by pid,
        // but `root.parent()` is `temp_dir()` itself — so the victim sat at a fixed
        // shared path while every other fixture in this file was careful. Two
        // `cargo test` runs on one machine then share it, and the `remove_file` at
        // the end of this test deletes the *other* run's file between its `write`
        // and its `read`, failing an assertion about a guard that is working
        // perfectly. Seen on 2026-09-10. **A pid on the directory is not a pid on
        // the fixture when the fixture is deliberately outside the directory.**
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

        // ⚠️ On Windows an absolute component makes `Path::join` discard the
        // base, so this one does not even have to climb.
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
