//! Writing a file so that a reader never sees half of one (§15 D378).
//!
//! **Every record this app keeps goes through [`fn@write`].** A document, each
//! pinned version of it, the project list, the preferences, the per-machine
//! index, a crash snapshot — all of them are files that are read back and
//! *believed*, and all of them were a bare `fs::write` until 2026-08-28, or an
//! `fs::copy` in the pin's case, which is the same three steps with the bytes
//! coming off a disk rather than out of memory: open, truncate, stream. A process
//! that dies anywhere in the middle of that leaves a file that is neither the old
//! contents nor the new ones, and the thing that reads it next cannot tell.
//!
//! ⚠️ **The window is not small and it is not rare.** `write_document` truncates
//! the user's actual document before it writes a byte, so the exposure is the
//! whole of every save — the autosave that fires every thirty seconds included.
//! `library::recovery` was built with the temp-and-rename shape from the start,
//! on the grounds that a file whose entire purpose is surviving a crash cannot be
//! written in a way a crash can corrupt; this module is that argument applied to
//! the files the snapshot exists to protect.
//!
//! **What this gives and what it does not.** After [`fn@write`] returns, `path`
//! holds either the complete new contents or the complete old ones, whatever
//! happens next — that is `rename` being atomic, on POSIX and on Windows
//! (`MoveFileEx` with `REPLACE_EXISTING`) alike. The `sync_all` before the rename
//! extends that from *process death* to *power loss*: without it the rename can
//! land while the data behind it is still in the page cache, which is a
//! zero-length or garbage file after a hard power cut.
//!
//! ⚠️ **The directory entry itself is not synced**, which POSIX asks for and Rust
//! does not portably expose. So a power cut in the microseconds after the rename
//! can still leave the *old* name pointing at the old data — which is the
//! pre-write state, and therefore the safe end of the failure. Nothing here can
//! produce a torn file; the residual risk is losing the new version, not
//! corrupting the old one.
//!
//! ⚠️ **That claim was false for a second writer of the same path until §15 D548,
//! and the premise it rested on was about the *process* rather than the machine.**
//! The temp name used to be derived from the target with no unique part, on the
//! stated grounds that this app never has two writers for one path — true of one
//! Ondin, and nothing prevents two. Nothing does: there is no lock file, no named
//! mutex and no single-instance check, and the base folder is documented as
//! syncable. Two instances with the same document open both autosave every thirty
//! seconds and snapshot every ten, through here, onto one temp name; the second
//! `File::create` truncates the first's temp, and the first's handle then follows
//! the file *through* the rename and extends it. Staged deterministically that
//! ends with the destination holding `BBBB` + twelve NULs + `AAAA` **after both
//! saves reported success** — a torn file produced by the mechanism that exists to
//! make torn files impossible. [`temp_for`] now carries the process id and a
//! per-process counter, which turns that into a lost update: the safe direction,
//! and the only one this module can reach without an instance lock.
//!
//! **Two kinds of file deliberately do not use this.** The *derived caches* —
//! the cover PNGs and the font data — are regenerated from a document or
//! re-fetched when they fail to parse, so a torn one costs nothing and is
//! self-healing, and they are written in bulk where a `sync_all` each would be
//! felt. The *exports*, CLI and panel alike (`main.rs`, `panels::export`), are an
//! output the user re-runs rather than a record the app keeps. **The rule is that
//! a file worth an fsync is one nothing else can reconstruct**, and those are the
//! whole of the un-converted set — which is worth stating as an inventory,
//! because the next `fs::write` somebody adds should have to argue its way onto
//! this list rather than simply not be noticed.

// ⚠️ **`[`fn@write`]` throughout, not `[`write`]`.** A bare link to this
// module's `write` is ambiguous with `std::write!`, which is in the prelude — and
// `cargo doc` is a hard gate here, so it fails the build rather than warning.
// Worth knowing before naming any other item after a prelude macro.
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Write `bytes` to `path`, atomically.
///
/// Creates the parent directory if it is missing, which every caller wanted
/// anyway and two of them used to do by hand.
pub fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Both halves of what [`temp_for`]'s uniqueness costs: the debris no longer
    // self-clears, so it is swept instead. Cheap — one `stat` per write, and one
    // `read_dir` per directory for the whole life of the process.
    remove_if_stale(&legacy_temp_for(path));
    sweep_once(path);
    let temp = temp_for(path);
    // 🚨 **Every failure removes the temp, not only the rename's** (§15 D706,
    // `[S1.2-L2-07]`). The cleanup used to sit in the `rename` error arm alone,
    // so a failure at `File::create`, `write_all` or `sync_all` returned through
    // `?` and left the temp behind — and **disk-full, which is the reason this
    // comment gives for cleaning up at all, surfaces at `write_all`**, the one
    // of the four with no cleanup.
    //
    // ⚠️ **`sweep_once` above does not cover it, and the reason is worth
    // knowing.** D548 bounds that sweep to once per directory per process on the
    // argument that *"debris left by a dead process was there before this one
    // started"* — which is true of debris this process inherits and false of
    // debris it creates *after* its own sweep has run. That is bounded rather
    // than permanent, since a later launch sweeps it once `STALE_TEMP` has
    // passed, but the honest place to clean up a write's own failure is the
    // write.
    //
    // A block, so the handle is closed before the rename. Windows will not
    // rename over — or from — a file that is still open in this process.
    let result = (|| -> std::io::Result<()> {
        {
            let mut file = std::fs::File::create(&temp).map_err(|e| at(e, "create", &temp))?;
            file.write_all(bytes).map_err(|e| at(e, "write", &temp))?;
            file.sync_all().map_err(|e| at(e, "flush", &temp))?;
        }
        // **The destination, not the temp.** This is the one step whose path the
        // user has heard of, and it is the step reachable today — see
        // `a_refused_write_names_the_file_it_could_not_write`.
        std::fs::rename(&temp, path).map_err(|e| at(e, "replace", path))
    })();
    if result.is_err() {
        // The temp file is the failure's only trace, and leaving it would mean a
        // folder that slowly fills with the debris of a disk that is full —
        // which is the most likely reason to be here. Ignored, because the
        // caller is being handed the *write's* error and a failure to tidy up
        // after it is not the more interesting one.
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// Remove every temp file that belongs to `path`, whichever writer left it.
///
/// For callers that delete a record and want its debris to go too; the ordinary
/// path needs nothing, because [`fn@write`] sweeps.
///
/// ⚠️ **Unconditional, where [`fn@write`]'s sweep is age-gated**, and the
/// difference is deliberate: this is a caller saying *this record is gone*, so a
/// temp for it is debris by definition. If a second instance is mid-write on the
/// same path its rename then fails and it reports the failure — a lost update,
/// announced, which is the right end of a race the user started by deleting the
/// thing being written.
pub fn forget(path: &Path) {
    for temp in leftovers(path) {
        let _ = std::fs::remove_file(temp);
    }
}

/// Say which step failed and on which file, keeping the kind (§15 D715).
///
/// **An `io::Error` carries a kind and a sentence and no path**, so every refusal
/// out of [`fn@write`] reached the status line as `Save failed: Access is denied.
/// (os error 5)` — naming neither the file nor the step, on a message whose whole
/// job is to be acted on. `[S1.2-L1-08]` measured the Windows sharing-violation
/// spelling of the same thing.
///
/// ⚠️ **`Error::new` keeps `kind()` and drops `raw_os_error()`.** Nothing in this
/// workspace reads either off a save today; the kind is preserved because it is
/// the half a caller would plausibly want to classify on, and the raw code is
/// already in the message text the OS wrote.
///
/// The verbs are the four steps rather than the four function names — a user
/// reads *"could not replace"*, not *"rename failed"* — and `flush` is
/// `sync_all`, which is the durability barrier rather than a buffer flush, but is
/// the word that means anything outside this module.
fn at(e: std::io::Error, step: &str, on: &Path) -> std::io::Error {
    std::io::Error::new(e.kind(), format!("could not {step} {}: {e}", on.display()))
}

/// The suffix every temp file this module writes ends with.
const TEMP_EXT: &str = ".writing";

/// How old a temp file has to be before [`fn@write`] will sweep it.
///
/// ⚠️ **An hour, which is absurdly generous on purpose.** The thing this bound
/// protects is another *live* writer's temp: sweep one of those and its rename
/// fails. The slowest write this module has measured is a 60 MB document, ~1.2 s
/// to serialise before a byte reaches here, so any sane figure is orders out — and
/// the reason it is an hour rather than a minute is that the base folder is
/// documented as syncable, so an mtime here can have come off another machine's
/// clock. An hour is longer than the skew anyone tolerates on a synced folder and
/// still short enough that a crash's debris is gone by the next session.
const STALE_TEMP: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// One counter for the whole process, so two threads racing for one path cannot
/// pick the same temp either.
///
/// The process id alone would fix the two-*instance* case and leave the
/// two-*thread* case, which `library::writer`'s module doc has been holding shut
/// by hand — *"one worker for both, and that is not tidiness"* — since D392. It
/// still should; this makes the write layer stop depending on it.
static WRITE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The directories [`sweep_once`] has already been through.
///
/// A `Vec` rather than a `HashSet` because there are four of them for the life of
/// the process — the base folder, `.recovery/`, the versions folder and the config
/// dir — and `Vec::new` is `const` where `HashSet::new` is not.
static SWEPT: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// The temp file used while writing `path`: the target's own name, this process's
/// id, a counter, and `.writing`.
///
/// ⚠️ **Unique per call, which is what makes two writers of one path safe** — see
/// the module doc, and §15 D548 for what it replaced and what that cost. It is the
/// *target's* name with a suffix appended rather than a name of its own, so the
/// temp is always in the destination's directory and a cross-volume rename is
/// unreachable.
///
/// ⚠️ **The extension is still `writing`, which is what keeps these out of the
/// library.** `library::scan` matches on an extension of exactly `ondin`, so
/// `landing-v4.ondin.8412-3.writing` is not a document, is not counted by a
/// heading and cannot be opened by accident — and `library::recovery::pending`
/// filters the same way.
///
/// ⚠️ **The `<pid>-<counter>` segment is load-bearing beyond uniqueness**: it is
/// what lets [`sweep_once`] tell this module's debris from a file the user happens
/// to have named `notes.writing`. A bare `.writing` suffix cannot be swept safely
/// for exactly that reason, and [`legacy_temp_for`] is how the old scheme's litter
/// is cleared instead.
fn temp_for(path: &Path) -> PathBuf {
    let n = WRITE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{}-{n}{TEMP_EXT}", std::process::id()));
    PathBuf::from(name)
}

/// The temp file the pre-D548 scheme used for `path` — the target's name with
/// `.writing` and nothing between.
///
/// Kept because an installed build wrote them, and a user upgrading has one per
/// file it ever crashed a save of. [`fn@write`] clears this one by *name*, from the
/// exact target it is writing, which is the only context in which a bare
/// `.writing` is unambiguously ours.
fn legacy_temp_for(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(TEMP_EXT);
    PathBuf::from(name)
}

/// Every temp file in `path`'s directory that belongs to `path`, of either scheme.
fn leftovers(path: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let target = name.to_string_lossy().into_owned();
    entries
        .flatten()
        .filter(|entry| is_temp_of(&target, &entry.file_name().to_string_lossy()))
        .map(|entry| entry.path())
        .collect()
}

/// Whether `candidate` is a temp file for the target file named `target`.
///
/// Either `<target>.writing` (the pre-D548 scheme) or
/// `<target>.<digits>-<digits>.writing`.
///
/// ⚠️ **The digits are checked rather than the shape assumed**, so that
/// `doc.ondin` does not claim `doc.ondin.bak.4-0.writing` — which it would under
/// the obvious "starts with the name, ends with the suffix" predicate, and which
/// [`forget`] would then delete.
fn is_temp_of(target: &str, candidate: &str) -> bool {
    let Some(rest) = candidate.strip_prefix(target) else {
        return false;
    };
    let Some(middle) = rest.strip_suffix(TEMP_EXT) else {
        return false;
    };
    if middle.is_empty() {
        return true;
    }
    let Some(stamp) = middle.strip_prefix('.') else {
        return false;
    };
    let Some((pid, counter)) = stamp.split_once('-') else {
        return false;
    };
    !pid.is_empty()
        && !counter.is_empty()
        && pid.bytes().all(|b| b.is_ascii_digit())
        && counter.bytes().all(|b| b.is_ascii_digit())
}

/// Remove `temp` if it is older than [`STALE_TEMP`].
fn remove_if_stale(temp: &Path) {
    let Ok(modified) = std::fs::metadata(temp).and_then(|m| m.modified()) else {
        return;
    };
    if std::time::SystemTime::now()
        .duration_since(modified)
        .is_ok_and(|age| age >= STALE_TEMP)
    {
        let _ = std::fs::remove_file(temp);
    }
}

/// Clear stale temp files out of `path`'s directory, the first time this process
/// writes into it.
///
/// ⚠️ **Once per directory, not once per write.** The sweep is a `read_dir` plus a
/// `stat` per entry, and the base folder can hold thousands of documents — which
/// would be tens of milliseconds inside the frame that saves the preferences. The
/// bound costs nothing: debris left by a *dead* process was there before this one
/// started, so the first write into a directory is the only moment that can find
/// any that matters.
///
/// ⚠️ **It only removes names carrying [`temp_for`]'s `<pid>-<counter>` stamp**, and
/// that is the whole reason the stamp is in the name. A directory sweep does not
/// know which files are records this app writes, so a bare `*.writing` filter would
/// delete a user's own `draft.writing` out of their base folder — data loss, from
/// the module whose subject is not losing data.
fn sweep_once(path: &Path) {
    let Some(dir) = path.parent() else {
        return;
    };
    {
        let Ok(mut swept) = SWEPT.lock() else {
            return;
        };
        if swept.iter().any(|seen| seen == dir) {
            return;
        }
        swept.push(dir.to_path_buf());
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // `is_temp_of` with the target taken from the candidate itself: whatever
        // precedes a well-formed stamp is a target name by construction.
        let Some(stamp) = name.strip_suffix(TEMP_EXT) else {
            continue;
        };
        let Some((target, _)) = stamp.rsplit_once('.') else {
            continue;
        };
        if is_temp_of(target, &name) {
            remove_if_stale(&entry.path());
        }
    }
}

/// Every temp file belonging to `path`, for tests that assert none was left
/// behind.
///
/// ⚠️ **Exposed so no test spells a name out**, which is not fastidiousness: a
/// test asserting `foo.writing` does not exist goes on passing forever once the
/// scheme changes, because it is then checking for a file that cannot exist under
/// any circumstances. `library::recovery`'s did exactly that for the length of one
/// edit — and D548 changed the scheme, which is the second time that warning has
/// been the difference between a test and a decoration.
#[cfg(test)]
pub fn leftover_temps(path: &Path) -> Vec<PathBuf> {
    leftovers(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ondin-atomic-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A write replaces the file, leaves no temp behind, and makes the parent
    /// directory it needs.
    ///
    /// ⚠️ **The temp-file assertion is the one about the mechanism.** A leftover
    /// `.writing` after a *successful* write means the rename did not happen and
    /// the bytes at `path` are whatever was there before — a save that reports
    /// success and did nothing. Flip-checked by replacing the `rename` with a
    /// `fs::copy`: that assertion fails with the temp still present, and the
    /// content assertions stay green, which is exactly the shape of the bug. The
    /// same flip also fails `a_temp_file_left_by_a_dead_process_is_not_a_document`
    /// at *its* self-clears assertion, which is the same fact from the other side.
    ///
    /// ⚠️ **`sync_all` has no flip**, and nothing here can have one: it is the
    /// difference between surviving a process death and surviving a power cut,
    /// and a test can stage the first but not the second. That is stated rather
    /// than papered over — the module doc says exactly what the call buys, and it
    /// is the one line in this file no assertion is watching.
    #[test]
    fn a_write_replaces_the_file_and_leaves_no_temp_behind() {
        let root = temp_root("write");
        let path = root.join("nested").join("doc.ondin");
        write(&path, b"first").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        write(&path, b"second").unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"second",
            "a second write replaces the first"
        );
        assert!(
            leftovers(&path).is_empty(),
            "the temp file is renamed away, not copied — a leftover means the \
             file on disk is not the one just written"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The temp file is not a document, which is what keeps a crashed save out of
    /// the library rather than in it under a name nobody chose.
    ///
    /// ⚠️ **Asserted through `library::scan` rather than by inspecting the
    /// name.** The property that matters is not "the extension is `writing`", it
    /// is "the scan does not list it" — and those are the same fact only for as
    /// long as `scan` matches on the extension it matches on today.
    ///
    /// ⚠️ **The self-clearing half changed with §15 D548 and is now the sweep's,
    /// not `File::create`'s.** It used to hold because the temp name was the
    /// target's, so the next save truncated the same file and renamed it away. A
    /// unique name per write ends that, so the debris is swept instead — which is
    /// **age-gated**, and the case this test names therefore has to be *arranged*
    /// rather than hoped for. The stamp is set with `File::set_modified`, which is
    /// `recovery`'s lesson: a test that hopes two writes land in the same whole
    /// second is a test of the clock.
    #[test]
    fn a_temp_file_left_by_a_dead_process_is_not_a_document() {
        let root = temp_root("debris");
        let path = root.join("landing-v4.ondin");
        // The state a process killed between `create` and `rename` leaves, in
        // both schemes: one of the old build's, one of this one's.
        let old = legacy_temp_for(&path);
        let mine = temp_for(&path);
        std::fs::write(&old, b"half a document").unwrap();
        std::fs::write(&mine, b"half a document").unwrap();
        assert!(
            crate::library::scan::scan(&root).is_empty(),
            "a half-written save must not appear in the library, in either scheme"
        );

        // Dated back past the sweep's gate, which is what makes them debris
        // rather than another live writer's work in progress.
        for temp in [&old, &mine] {
            let stale = std::time::SystemTime::now() - STALE_TEMP - ONE_MINUTE;
            std::fs::OpenOptions::new()
                .write(true)
                .open(temp)
                .unwrap()
                .set_modified(stale)
                .unwrap();
        }

        // And the next successful save of the same file clears both.
        write(&path, b"whole").unwrap();
        assert!(
            leftovers(&path).is_empty(),
            "the debris is swept by the next save — {:?} left",
            leftovers(&path)
        );
        assert_eq!(crate::library::scan::scan(&root).len(), 1);

        forget(&path);
        // Missing is fine: `forget` is a caller saying "and its debris too".
        forget(&path);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A minute, for dating a fixture's temp file either side of `STALE_TEMP`.
    ///
    /// Plain backticks, not a `[link]` — §15 D319's convention: this item is
    /// `#[cfg(test)]`, so `cargo doc` cannot see it and no gate would validate one.
    const ONE_MINUTE: std::time::Duration = std::time::Duration::from_secs(60);

    /// **Two writers of one path get different temp files**, which is the whole of
    /// §15 D548's fix (`[A3-L5-04]`).
    ///
    /// ⚠️ **Flip-check, run: `temp_for` reverted to `name.push(TEMP_EXT)`.** Fails
    /// here on the first assertion, and — the one that matters — on
    /// `a_second_writer_cannot_tear_the_destination` below, which is where the
    /// consequence lives. This test is the property; that one is the damage.
    ///
    /// The counter and not just the pid: a process id makes two *instances* safe
    /// and leaves two *threads*, and `library::writer`'s single worker is the only
    /// thing that has been holding that shut.
    #[test]
    fn two_writes_of_one_path_get_different_temp_files() {
        let path = Path::new("C:").join("lib").join("doc.ondin");
        let (first, second) = (temp_for(&path), temp_for(&path));
        assert_ne!(
            first, second,
            "a temp name shared between two writers is the tear in \
             a_second_writer_cannot_tear_the_destination"
        );
        for temp in [&first, &second] {
            assert_eq!(temp.parent(), path.parent(), "and stays beside its target");
            assert!(
                is_temp_of("doc.ondin", &temp.file_name().unwrap().to_string_lossy()),
                "and is recognised as debris for that target: {temp:?}"
            );
        }
    }

    /// **A second writer of the same path cannot tear the destination** — the
    /// failure `[A3-L5-04]` names, staged deterministically rather than raced.
    ///
    /// The interleaving is the one from the finding, with the ordering that makes
    /// it *worse* than the finding claimed: the loser's handle follows the file
    /// through the rename and extends it, so the destination ends up holding both
    /// payloads with a hole between them — **after both saves reported success**.
    ///
    /// ⚠️ **The other writer is spelled `legacy_temp_for`, which looks like the
    /// thing `leftover_temps`' doc forbids and is the opposite of it.** That
    /// warning is about asserting a *name is absent*, which goes vacuously green
    /// when the scheme changes. Here the name is an **adversary's**, and it has to
    /// be the pre-D548 name specifically, because that is the name the flipped
    /// version of `temp_for` would collide with. Written any other way the flip
    /// below does not bite.
    ///
    /// ⚠️ **Flip-check, run: `temp_for` reverted to `name.push(TEMP_EXT)`.** Fails
    /// on the content assertion with 20 bytes against 4 —
    /// `BBBB` + twelve NULs + `AAAA` — which is the reported symptom in the
    /// failure message rather than in a comment.
    #[test]
    fn a_second_writer_cannot_tear_the_destination() {
        let root = temp_root("two-writers");
        let path = root.join("doc.ondin");
        write(&path, b"OLD").unwrap();

        // The other instance, mid-write: its temp open, 16 bytes in, not yet
        // synced. Under the old scheme this is the very file `write` is about to
        // create over.
        let mut other = std::fs::File::create(legacy_temp_for(&path)).unwrap();
        other.write_all(&[b'A'; 16]).unwrap();

        // This instance's whole write, start to finish, in that window.
        write(&path, b"BBBB").unwrap();

        // And the other instance finishing: its rename fails either way, because
        // its temp has been renamed away (old scheme) or is simply a different
        // file (new). What it does next is write through the handle it still
        // holds.
        other.write_all(&[b'A'; 4]).unwrap();
        other.sync_all().unwrap();
        drop(other);

        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"BBBB",
            "the destination holds one complete payload or the other, never a \
             graft of both — and both writers reported success"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A live writer's temp is not swept**, which is the bound on the sweep that
    /// replaced §15 D548's self-clearing.
    ///
    /// ⚠️ **Flip-check, run: `STALE_TEMP` set to zero.** Fails on the
    /// `still_there` assertion — the sweep eats the other writer's in-flight temp,
    /// whose rename then fails and whose save is lost. So the age gate is what
    /// keeps a *concurrent* save a lost update rather than an error, and this is
    /// the assertion watching it.
    ///
    /// ⚠️ **And a second flip, which did *not* bite where it was predicted to:**
    /// dropping the `<pid>-<counter>` check from `sweep_once` and filtering on
    /// `.writing` alone leaves this test green, because the fixture's own name
    /// carries a stamp. It fails on `a_users_own_writing_file_is_not_debris`
    /// instead — the one that stages the data loss the stamp exists to prevent.
    #[test]
    fn a_live_writers_temp_is_left_alone() {
        let root = temp_root("live-writer");
        let theirs = root.join("theirs.ondin");
        let mine = root.join("mine.ondin");

        // Their write, in flight: a well-formed temp of this very moment.
        let in_flight = temp_for(&theirs);
        std::fs::write(&in_flight, b"in flight").unwrap();

        // And one of theirs from a session that died an hour ago.
        let dead = temp_for(&theirs);
        std::fs::write(&dead, b"a dead session").unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&dead)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - STALE_TEMP - ONE_MINUTE)
            .unwrap();

        write(&mine, b"mine").unwrap();

        assert!(
            in_flight.exists(),
            "a temp written seconds ago belongs to a writer that is still running"
        );
        assert!(
            !dead.exists(),
            "and one older than STALE_TEMP does not — that is the debris the \
             unique name would otherwise accumulate"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The sweep does not delete a file the user named `something.writing`.**
    ///
    /// The base folder is the user's own directory, so a directory-wide sweep on
    /// the extension alone is data loss from the module whose subject is not
    /// losing data. The `<pid>-<counter>` stamp is what distinguishes them, and
    /// this is the assertion that says so.
    ///
    /// ⚠️ **Flip-check, run: `sweep_once` filtering on `TEMP_EXT` alone.** Both
    /// assertions fail — the hand-named file and the pre-D548 debris go together,
    /// and the second is why `write` clears the legacy name from the *target* it is
    /// writing instead of from the directory.
    #[test]
    fn a_users_own_writing_file_is_not_debris() {
        let root = temp_root("user-file");
        let theirs = root.join("chapter-three.writing");
        let old_scheme = root.join("someone-elses.ondin.writing");
        for file in [&theirs, &old_scheme] {
            std::fs::write(file, b"the user's own work").unwrap();
            std::fs::OpenOptions::new()
                .write(true)
                .open(file)
                .unwrap()
                .set_modified(std::time::SystemTime::now() - STALE_TEMP - ONE_MINUTE)
                .unwrap();
        }

        write(&root.join("mine.ondin"), b"mine").unwrap();

        assert!(
            theirs.exists(),
            "a `.writing` file with no stamp was not written by this module"
        );
        assert!(
            old_scheme.exists(),
            "and neither is a bare `<name>.ondin.writing` swept by the directory \
             pass — it is cleared by `write`, from the target it is writing, which \
             is the only place a bare suffix is unambiguously ours"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A write that fails leaves the old file whole** — this module's headline
    /// promise, and until §15 D480 it was asserted by nothing (`[S1.2-L6-05]`).
    ///
    /// Both tests above exercise the success path, and the flip recorded in the
    /// first one picks `rename` → `fs::copy`, which fails on the *leftover temp*.
    /// So the pair looks like it covers the mechanism and covers the temp file
    /// rather than the destination. **The regression that admits is the one every
    /// reviewer of Windows code reaches for**: *"`fs::rename` fails if the
    /// destination exists, so remove it first."* Inserting
    /// `let _ = std::fs::remove_file(path);` above the rename leaves both of them
    /// green, and every save then has a window in which the user's document does
    /// not exist at all.
    ///
    /// **Read-only is the lock, because it is portable and it is one line.** The
    /// destination is made read-only, so the rename onto it is refused; the
    /// alternative measured for `[S1.2-L6-05]` — holding the file open with
    /// `share_mode(FILE_SHARE_READ)`, no `DELETE` — produces the same
    /// `PermissionDenied` and needs `std::os::windows`.
    ///
    /// ⚠️ **Flip-check, run: `let _ = std::fs::remove_file(path);` inserted above
    /// the `rename`.** Fails on the content assertion with `NEW` against `OLD`,
    /// and **both tests above stay green** — which is the measurement that says
    /// this test is not a restatement of them.
    ///
    /// ⚠️ **The predicted failure was wrong twice over and the corrections are
    /// worth more than the prediction.** It was expected to fail *because the file
    /// was gone*; it fails because the file was **replaced**. On Windows
    /// `std::fs::remove_file` clears the read-only attribute and retries the
    /// delete, so the flipped version defeats the very lock that stages this test:
    /// the rename then succeeds, `write` returns `Ok`, and the protected document
    /// is silently overwritten. That is a *worse* outcome than the one predicted
    /// and this test is what shows it — the "remove it first" instinct does not
    /// merely open a window, it walks through a refusal.
    ///
    /// And the assertions had to be reordered to say so. With `result.is_err()`
    /// first the flip failed on *"the fixture has to reach a failing rename"* — a
    /// fixture guard, reporting that the setup was wrong when the setup was fine
    /// and the code had destroyed the file. **The loss goes before the report**:
    /// what the user loses is the old contents, and whether the failure was
    /// announced is the second question.
    ///
    /// ⚠️ **`fs::rename` really does replace an existing destination on Windows**
    /// (`MOVEFILE_REPLACE_EXISTING`), so the "remove it first" instinct is wrong on
    /// its own terms as well as dangerous. Measured for `[S1.2-L6-05]` and recorded
    /// here so the next reader does not re-derive it from documentation — and
    /// `temp_for` appends to the full destination path, so the temp is always in
    /// the destination's own directory and a cross-volume rename is unreachable.
    #[test]
    fn a_failed_write_leaves_the_old_file_intact() {
        let root = temp_root("failed");
        let path = root.join("doc.ondin");
        write(&path, b"OLD").unwrap();

        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&path, perms).unwrap();

        let result = write(&path, b"NEW");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"OLD",
            "the destination must still hold the complete old contents — either \
             the rename was refused and nothing moved, or something unlinked the \
             very file the read-only flag was protecting"
        );
        assert!(
            result.is_err(),
            "and the refusal has to be reported rather than swallowed"
        );
        assert!(
            leftovers(&path).is_empty(),
            "and it must not leave its temp file behind"
        );
        // 🚨 **This assertion now covers all four failure points and exercises
        // one** (§15 D706). The cleanup used to live in the `rename` error arm,
        // which is the arm this fixture stages; it is a common `if
        // result.is_err()` now, so `File::create`, `write_all` and `sync_all`
        // are covered by construction. **They are not covered by a test, and
        // saying so is the point** — a read-only *destination* is the only
        // failure this module can stage portably, and the three earlier arms
        // want a full disk or a revoked ACL. Writable-with-difficulty rather
        // than written, which is a queue rather than a fact.
        //
        // ⚠️ **Flip, run:** disabling the cleanup fails this line, and this line
        // alone — the content and `is_err` assertions above stay green, which is
        // what says the temp-file question is asked here and nowhere else.

        // Clear the flag before the cleanup. `set_readonly(false)` is what clippy
        // warns about — it grants *everyone* write on Unix — and it is right about
        // production code and wrong about a temp file this test made two lines ago
        // on a platform where the flag is one bit.
        #[allow(clippy::permissions_set_readonly_false)]
        {
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_readonly(false);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A refused write says which file it could not write** (§15 D715,
    /// `[S1.2-L1-08]`).
    ///
    /// The status line is `format!("Save failed: {e}")` (`app.rs`), and every
    /// error out of `write` used to be a bare `std::io::Error` — which
    /// carries a *kind* and a sentence and **no path**. So a locked or read-only
    /// document produced *"Save failed: The process cannot access the file
    /// because it is being used by another process. (os error 32)"*: it names
    /// neither the file nor which of the four steps was refused, and the user
    /// cannot act on it.
    ///
    /// 🚨 **This is the half of `[S1.2-L1-08]` that survives, and the finding's
    /// headline does not.** That finding is about a stale `.writing` another
    /// process holds open making `File::create` fail forever — which **§15 D548
    /// closed** by putting the process id and a per-call counter in the temp
    /// name, so the name a write is about to create has never existed before.
    /// `two_writes_of_one_path_get_different_temp_files` above is what pins that.
    /// The finding was taken at a SHA before D548 and its measurement was correct
    /// then.
    ///
    /// ⚠️ **And the residual is wider than the finding's own account of it**,
    /// which asked for the *temp* path in the message. The step reachable today is
    /// the **rename**, refused by a destination something else holds open — a sync
    /// client or an indexer on the user's own `.ondin`, not on a leftover — and
    /// the path worth printing there is the destination, which is the file the
    /// user has actually heard of.
    ///
    /// **Asserted on the substring rather than the whole sentence**, because the
    /// OS text differs by platform and locale and pinning it would make this a
    /// test of Windows' phrasing.
    ///
    /// ⚠️ **Flip-check, run: the `rename`'s `map_err` removed, leaving the three
    /// temp-file steps wrapped.** Fails here with *"Access is denied. (os error
    /// 5)"* — the bare message again — and the seven tests above stay green.
    /// 🚨 **That flip is the finding's own fix sketch**, which asked for the temp
    /// path and no more; it would have left the one step a user actually reaches
    /// as unattributable as it was. *A sketch written against the mechanism that
    /// was reachable when it was written is not a sketch of the fix once another
    /// entry has closed that mechanism.*
    #[test]
    fn a_refused_write_names_the_file_it_could_not_write() {
        let root = temp_root("named-error");
        let path = root.join("doc.ondin");
        write(&path, b"OLD").unwrap();

        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&path, perms).unwrap();

        let err = write(&path, b"NEW").expect_err("the fixture has to refuse the write");
        let text = err.to_string();
        assert!(
            text.contains("doc.ondin"),
            "a save the user has to act on must name the file: {text:?}"
        );
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::PermissionDenied,
            "and the kind has to survive the wrapping, or a later caller that \
             wants to classify this cannot"
        );

        #[allow(clippy::permissions_set_readonly_false)]
        {
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_readonly(false);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
