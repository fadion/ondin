//! Writing a file so that a reader never sees half of one (§15 D378).
//!
//! Every record this app keeps goes through [`fn@write`]: a document, each pinned
//! version, the project list, the preferences, the per-machine index, a crash
//! snapshot. A bare `fs::write` (or `fs::copy`) opens, truncates and streams, so a
//! process dying in the middle leaves a file that is neither the old contents nor
//! the new, and the next reader cannot tell. The exposure is the whole of every
//! save, autosave included.
//!
//! After [`fn@write`] returns, `path` holds either the complete new contents or
//! the complete old ones, whatever happens next: `rename` is atomic on POSIX and
//! on Windows (`MoveFileEx` with `REPLACE_EXISTING`). The `sync_all` before the
//! rename extends that from process death to power loss; without it the rename can
//! land while the data is still in the page cache.
//!
//! The directory entry itself is not synced (POSIX asks for it, Rust does not
//! portably expose it). A power cut just after the rename can leave the old name
//! pointing at the old data: the residual risk is losing the new version, never
//! tearing a file.
//!
//! Two Ondin instances can write one path: there is no single-instance check, and
//! `instances`' lock files only count running copies for the updater. [`temp_for`]
//! therefore carries the process id and a per-process counter, so a second writer
//! is a lost update rather than a torn file (a shared temp name let one writer's
//! handle follow the file through the other's rename and extend it).
//!
//! Three kinds of file deliberately do not use this. The derived caches (cover
//! PNGs, font data) are regenerated or re-fetched when they fail to parse, and are
//! written in bulk. The exports (`main.rs`, `panels::export`) are output the user
//! re-runs. `ondin.log` (`logging`) is an append-only stream whose worst crash is
//! a torn last line. A file worth an fsync is one nothing else can reconstruct; a
//! new `fs::write` has to argue its way onto this list.

// Link as `[`fn@write`]`, not `[`write`]`: a bare link is ambiguous with the
// prelude's `std::write!`, and the doc gate fails the build on it.
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Write `bytes` to `path`, atomically.
///
/// Creates the parent directory if it is missing.
pub fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // Write through a symlink: a rename would replace the link, leaving a plain
    // file beside a target that never sees the save again.
    let resolved = through_link(path);
    let path = resolved.as_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // [`temp_for`]'s unique names are never overwritten, so stale debris is swept.
    remove_if_stale(&legacy_temp_for(path));
    sweep_once(path);
    let temp = temp_for(path);
    // Every failure removes the temp, not only the rename's: disk-full surfaces at
    // `write_all`, and `sweep_once` runs once per directory.
    //
    // A block, so the handle is closed before the rename: Windows will not rename
    // from a file still open in this process.
    let result = (|| -> std::io::Result<()> {
        {
            let mut file = std::fs::File::create(&temp).map_err(|e| at(e, "create", &temp))?;
            file.write_all(bytes).map_err(|e| at(e, "write", &temp))?;
            file.sync_all().map_err(|e| at(e, "flush", &temp))?;
            // Keep the replaced file's permissions (a `0600` document must not
            // become `0644`). Ignored on failure.
            if let Ok(meta) = std::fs::metadata(path) {
                let _ = file.set_permissions(meta.permissions());
            }
        }
        // The destination, not the temp, is named: it is the path the user knows.
        std::fs::rename(&temp, path).map_err(|e| at(e, "replace", path))
    })();
    if result.is_err() {
        // Ignored: the caller gets the write's error, not a tidy-up failure.
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// The file `path` names: its target if it is a symbolic link that resolves,
/// and `path` itself otherwise.
fn through_link(path: &Path) -> PathBuf {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
        }
        _ => path.to_path_buf(),
    }
}

/// Remove every temp file that belongs to `path`, whichever writer left it.
///
/// For callers that delete a record. Unconditional, where [`fn@write`]'s sweep is
/// age-gated: the record is gone, so a temp for it is debris, and a second
/// instance mid-write fails its rename and reports it.
pub fn forget(path: &Path) {
    for temp in leftovers(path) {
        let _ = std::fs::remove_file(temp);
    }
}

/// Say which step failed and on which file, keeping the kind.
///
/// An `io::Error` carries no path. `Error::new` keeps `kind()` and drops
/// `raw_os_error()`, which is already in the OS's message text. The verbs are in a
/// user's words ("could not replace"); `flush` is `sync_all`.
fn at(e: std::io::Error, step: &str, on: &Path) -> std::io::Error {
    std::io::Error::new(e.kind(), format!("could not {step} {}: {e}", on.display()))
}

/// The suffix every temp file this module writes ends with.
const TEMP_EXT: &str = ".writing";

/// How old a temp file has to be before [`fn@write`] will sweep it.
///
/// An hour, generous on purpose: sweeping another live writer's temp fails that
/// rename, and the base folder is syncable, so an mtime can come off another
/// machine's clock.
const STALE_TEMP: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// One counter for the whole process, so two threads racing for one path cannot
/// pick the same temp; the pid alone covers only two instances.
static WRITE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The directories [`sweep_once`] has already been through.
///
/// A `Vec` rather than a `HashSet`: there are only a few, and `Vec::new` is `const`.
static SWEPT: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// The temp file used while writing `path`: the target's own name, this process's
/// id, a counter, and `.writing`.
///
/// Unique per call, which makes two writers of one path safe. Appended to the
/// target's name, so the temp is always in the destination's directory.
///
/// The extension stays `writing`, which keeps these out of the library:
/// `library::scan` matches an extension of exactly `ondin`, as does
/// `library::recovery::pending`.
///
/// The `<pid>-<counter>` segment is load-bearing beyond uniqueness: it lets
/// [`sweep_once`] tell this module's debris from a user's own `notes.writing`.
fn temp_for(path: &Path) -> PathBuf {
    let n = WRITE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{}-{n}{TEMP_EXT}", std::process::id()));
    PathBuf::from(name)
}

/// The temp file the earlier scheme used for `path` — the target's name with
/// `.writing` and nothing between.
///
/// [`fn@write`] clears it by name from the exact target it is writing, the only
/// context in which a bare `.writing` is unambiguously ours.
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
/// Either `<target>.writing` (the earlier scheme) or
/// `<target>.<digits>-<digits>.writing`.
///
/// The digits are checked rather than the shape assumed, so `doc.ondin` does not
/// claim `doc.ondin.bak.4-0.writing`, which [`forget`] would then delete.
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
/// Once per directory: a `read_dir` plus a `stat` per entry is too much per write
/// in a base folder of thousands of documents, and debris from a dead process
/// predates this one. It only removes names carrying [`temp_for`]'s
/// `<pid>-<counter>` stamp, never a user's own `draft.writing`.
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
        // `is_temp_of` with the target taken from the candidate itself.
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
/// Exposed so no test spells a name out: a test asserting `foo.writing` is absent
/// passes forever once the scheme changes.
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

    /// Make `write(path, …)` fail: a read-only destination on Windows, a read-only
    /// directory on Linux and macOS (a rename is governed by the directory, so the
    /// temp's creation is refused a step earlier). Root ignores both.
    fn lock(path: &Path) {
        #[cfg(windows)]
        {
            let mut perms = std::fs::metadata(path).unwrap().permissions();
            perms.set_readonly(true);
            std::fs::set_permissions(path, perms).unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = path.parent().unwrap();
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        }
    }

    /// Undo `lock`, so the temp root can be removed.
    fn unlock(path: &Path) {
        #[cfg(windows)]
        // Clippy warns that `set_readonly(false)` grants everyone write on Unix;
        // this arm is Windows', where it is one flag on a file this test made.
        #[allow(clippy::permissions_set_readonly_false)]
        {
            let mut perms = std::fs::metadata(path).unwrap().permissions();
            perms.set_readonly(false);
            std::fs::set_permissions(path, perms).unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = path.parent().unwrap();
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// A write replaces the file, leaves no temp behind, and makes the parent
    /// directory it needs.
    ///
    /// flip: `rename` replaced by `fs::copy` → fails at the leftovers assertion
    /// with the temp still present, content assertions green. The same flip fails
    /// `a_temp_file_left_by_a_dead_process_is_not_a_document` at its leftovers assertion.
    ///
    /// `sync_all` has no flip: a test can stage process death but not power loss.
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

    /// A crashed save's temp stays out of the library (asserted through
    /// `library::scan`), and the next save sweeps it once it is old enough, dated
    /// with `File::set_modified`.
    #[test]
    fn a_temp_file_left_by_a_dead_process_is_not_a_document() {
        let root = temp_root("debris");
        let path = root.join("landing-v4.ondin");
        // What a process killed between `create` and `rename` leaves, in both schemes.
        let old = legacy_temp_for(&path);
        let mine = temp_for(&path);
        std::fs::write(&old, b"half a document").unwrap();
        std::fs::write(&mine, b"half a document").unwrap();
        assert!(
            crate::library::scan::scan(&root).is_empty(),
            "a half-written save must not appear in the library, in either scheme"
        );

        // Dated back past the sweep's gate, so they are debris and not a live writer's.
        for temp in [&old, &mine] {
            let stale = std::time::SystemTime::now() - STALE_TEMP - ONE_MINUTE;
            std::fs::OpenOptions::new()
                .write(true)
                .open(temp)
                .unwrap()
                .set_modified(stale)
                .unwrap();
        }

        // The next successful save of the same file clears both.
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
    const ONE_MINUTE: std::time::Duration = std::time::Duration::from_secs(60);

    /// Two writers of one path get different temp files, beside the target and
    /// recognised as its debris.
    ///
    /// flip: `temp_for` reverted to `name.push(TEMP_EXT)` → fails at the first
    /// assertion, and at `a_second_writer_cannot_tear_the_destination`.
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

    /// A second writer of the same path cannot tear the destination, staged
    /// deterministically. The other writer uses `legacy_temp_for` on purpose: the
    /// name a reverted `temp_for` would collide with.
    ///
    /// flip: `temp_for` reverted to `name.push(TEMP_EXT)` → fails at the content
    /// assertion, 20 bytes against 4 (`BBBB` + twelve NULs + `AAAA`).
    #[test]
    fn a_second_writer_cannot_tear_the_destination() {
        let root = temp_root("two-writers");
        let path = root.join("doc.ondin");
        write(&path, b"OLD").unwrap();

        // The other instance, mid-write: its temp open, 16 bytes in, not yet synced.
        let mut other = std::fs::File::create(legacy_temp_for(&path)).unwrap();
        other.write_all(&[b'A'; 16]).unwrap();

        // This instance's whole write, start to finish, in that window.
        write(&path, b"BBBB").unwrap();

        // The other instance finishing: its rename fails either way, and it then
        // writes through the handle it still holds.
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

    /// A live writer's temp is not swept; one older than `STALE_TEMP` is.
    ///
    /// flip: `STALE_TEMP` set to zero → fails at the `in_flight` assertion, the
    /// sweep eating the other writer's temp.
    ///
    /// Dropping the `<pid>-<counter>` check from `sweep_once` leaves this green
    /// (the fixture carries a stamp); `a_users_own_writing_file_is_not_debris`
    /// catches it.
    #[test]
    fn a_live_writers_temp_is_left_alone() {
        let root = temp_root("live-writer");
        let theirs = root.join("theirs.ondin");
        let mine = root.join("mine.ondin");

        // Their write, in flight.
        let in_flight = temp_for(&theirs);
        std::fs::write(&in_flight, b"in flight").unwrap();

        // And one from a session that died an hour ago.
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

    /// The sweep does not delete a file the user named `something.writing`: the
    /// `<pid>-<counter>` stamp is what distinguishes them.
    ///
    /// flip: `sweep_once` filtering on `TEMP_EXT` alone → both assertions fail; the
    /// second is why `write` clears the legacy name from its own target instead of
    /// from the directory.
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

    /// A write that fails leaves the old file whole, with no temp behind.
    ///
    /// The regression to guard is "remove the destination first" (`fs::rename`
    /// already replaces it). On Windows `remove_file` clears the read-only
    /// attribute, so it defeats the lock and silently overwrites the document.
    /// On Linux and macOS this only shows that a refused `create` leaves the old
    /// file whole; the rename is pinned on Windows alone.
    ///
    /// flip (Windows only): `let _ = std::fs::remove_file(path);` above the
    /// `rename` → fails at the content assertion, `NEW` against `OLD`, with the
    /// success-path tests green. The content assertion goes first so the loss is
    /// reported before the announcement.
    #[test]
    fn a_failed_write_leaves_the_old_file_intact() {
        let root = temp_root("failed");
        let path = root.join("doc.ondin");
        write(&path, b"OLD").unwrap();

        lock(&path);

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
        // The cleanup is one `if result.is_err()`, so this covers all four failure
        // points but exercises one: on Windows the `rename`, on Linux and macOS the
        // `File::create`. The `write_all` and `sync_all` arms want a full disk.
        //
        // flip: disabling the cleanup → fails the leftovers assertion above and
        // only that one, on Windows; it cannot fail on Linux or macOS, where no
        // temp is ever created.

        unlock(&path);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A refused write names the file and keeps the error kind (substring match,
    /// since the OS text differs by platform and locale).
    ///
    /// flip: the `rename`'s `map_err` removed → fails at the file-name assertion.
    /// On Linux and macOS it cannot bite: the refused `create` names a temp that
    /// carries the destination's name.
    #[test]
    fn a_refused_write_names_the_file_it_could_not_write() {
        let root = temp_root("named-error");
        let path = root.join("doc.ondin");
        write(&path, b"OLD").unwrap();

        lock(&path);

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

        unlock(&path);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A save keeps the document's mode and its symlink: a `0600` file stays
    /// `0600`, and a document reached through a link is written at the link's
    /// target, the link left a link.
    ///
    /// `cfg(unix)`: both are Linux and macOS semantics.
    ///
    /// flip: the permissions copy disabled → fails at "the mode is kept", `420`
    /// (`0o644`, a `022` umask) against `384`; `write` skipping `through_link` →
    /// fails at "the link is still a link".
    #[cfg(unix)]
    #[test]
    fn a_save_keeps_the_documents_mode_and_its_link() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_root("mode-link");
        let doc = root.join("doc.ondin");
        std::fs::write(&doc, b"old").unwrap();
        std::fs::set_permissions(&doc, std::fs::Permissions::from_mode(0o600)).unwrap();
        write(&doc, b"new").unwrap();
        let mode = std::fs::metadata(&doc).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the mode is kept");

        let real = root.join("real").join("doc.ondin");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        std::fs::write(&real, b"old").unwrap();
        let link = root.join("link.ondin");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write(&link, b"new").unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link is still a link"
        );
        assert_eq!(
            std::fs::read(&real).unwrap(),
            b"new",
            "and its target was saved"
        );
        assert!(
            leftovers(&real).is_empty(),
            "the temp was beside the target"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
