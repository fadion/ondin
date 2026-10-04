//! Which other Ondin processes are running on this machine (§15 D970).
//!
//! **Asked before an update is applied, and only then.** Velopack's updater,
//! once launched, does not stop at the window that launched it: on Windows it
//! ends **every** process running from the install folder
//! (`force_stop_package`, a `TerminateProcess`), and on Linux and macOS it
//! swaps the app under them and starts the new version beside them. A second
//! window with unsaved work, or an `ondin export` a script is waiting on, would
//! be killed or orphaned with no question asked. So the restart is refused
//! while [`others`] counts anything, and asked again at the moment of exit.
//!
//! **A lock file per process, not a list of processes.** Each GUI or headless
//! run takes an exclusive lock on a file of its own in
//! `machine_dir::root()/instances` and holds it for its lifetime
//! ([`register`]). The operating system drops the lock however the process
//! ends — a crash and a kill included — so a file whose lock *can* be taken is
//! a leftover, and is swept. One path on every platform, all of it compiled and
//! tested here, where enumerating processes would have been three, two of them
//! compiled only by CI.
//!
//! ⚠️ **It counts every build that registers, not only the installed one.** A
//! `cargo run` window is outside the install folder and would survive the
//! updater, and it still holds an update back. Erring that way costs a
//! developer a click; the other way costs somebody's work.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The folder under `machine_dir::root()`.
const DIR: &str = "instances";

/// This process's own registration, held until the process ends.
///
/// A `static` because the lock must outlive everything, and because eframe can
/// leave the process without unwinding `main`. Its file is therefore never
/// removed by this process; the next [`register`] or [`others`] sweeps it.
static OWN: OnceLock<Registration> = OnceLock::new();

/// One process's lock file, and the handle that holds the lock.
#[derive(Debug)]
struct Registration {
    /// Never read: holding it open is what holds the lock.
    _file: File,
    path: PathBuf,
}

/// Register this process, once. Failure is silent: an unregistered process
/// can still run, and is simply not counted — the updater then behaves as it
/// did before this module.
pub fn register() {
    let Some(dir) = dir() else {
        return;
    };
    if let Some(reg) = register_in(&dir, std::process::id()) {
        let _ = OWN.set(reg);
    }
}

/// How many other registered processes are running now.
pub fn others() -> usize {
    let Some(dir) = dir() else {
        return 0;
    };
    others_in(&dir, OWN.get().map(|r| r.path.as_path()))
}

/// The machine's folder, or `None` under `cfg(test)` — a count *sweeps*, so a
/// test reaching the real folder would delete files in it. `library::cover`'s
/// `covers_dir` seam, for the same reason (§15 D845). Tests name their own
/// folder and call the `_in` functions.
fn dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(crate::machine_dir::root()?.join(DIR))
}

/// [`register`] in `dir`, as process `pid`.
///
/// **Locked under a temporary name and then renamed into place**, so a counter
/// never sees a `.lock` file before its lock is held — the instant between
/// `create` and `try_lock` would otherwise read as a leftover, and be swept from
/// under the process that was making it. A lock belongs to the open file, not
/// to its name, so it survives the rename.
fn register_in(dir: &Path, pid: u32) -> Option<Registration> {
    std::fs::create_dir_all(dir).ok()?;
    // Leftovers from processes that have ended, so the folder holds what is
    // running rather than every launch there has ever been.
    let _ = others_in(dir, None);
    let tmp = dir.join(format!("{pid}.tmp"));
    let file = File::create(&tmp).ok()?;
    file.try_lock().ok()?;
    let path = dir.join(format!("{pid}.lock"));
    std::fs::rename(&tmp, &path).ok()?;
    Some(Registration { _file: file, path })
}

/// Count the `.lock` files in `dir` whose lock is held, other than `own`,
/// removing those whose lock is not.
///
/// ⚠️ **A file that cannot be opened or tested counts as running.** The
/// question is whether applying an update could kill somebody's work, and
/// "could not tell" is not "no".
fn others_in(dir: &Path, own: Option<&Path>) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut running = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "lock") || Some(path.as_path()) == own {
            continue;
        }
        let file = match File::open(&path) {
            Ok(file) => file,
            // Its process removed it between the listing and the open.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                running += 1;
                continue;
            }
        };
        match file.try_lock() {
            Ok(()) => {
                drop(file);
                let _ = std::fs::remove_file(&path);
            }
            Err(_) => running += 1,
        }
    }
    running
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ondin-instances-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A live registration counts, one's own does not, and a leftover is swept
    /// rather than counted (§15 D970).
    ///
    /// A lock conflicts between two handles inside one process — `flock` per
    /// open file description on Unix, `LockFileEx` per handle on Windows — which
    /// is what lets one test stand in for two processes.
    ///
    /// **Flip-check, run**: counting a file whose lock *was* taken (`running +=
    /// 1` in the `Ok(())` arm) fails on *"a leftover is not running"* with 1;
    /// skipping no file as `own` fails on *"one's own registration is not
    /// another"* with 1.
    #[test]
    fn a_held_lock_counts_and_a_leftover_is_swept() {
        let dir = scratch("count");
        let a = register_in(&dir, 1).expect("registers");
        assert_eq!(
            others_in(&dir, Some(&a.path)),
            0,
            "one's own registration is not another"
        );
        let b = register_in(&dir, 2).expect("registers");
        assert_eq!(others_in(&dir, Some(&a.path)), 1, "a second process counts");
        assert_eq!(others_in(&dir, None), 2);

        // The second process ends: its handle closes, the OS drops the lock and
        // the file stays behind, as it does after a crash.
        let leftover = b.path.clone();
        drop(b);
        assert!(leftover.exists());
        assert_eq!(
            others_in(&dir, Some(&a.path)),
            0,
            "a leftover is not running"
        );
        assert!(!leftover.exists(), "and is swept");

        drop(a);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Only `.lock` files are counted: a registration still under its temporary
    /// name is not yet one, and nothing else in the folder is.
    #[test]
    fn only_lock_files_are_registrations() {
        let dir = scratch("names");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = File::create(dir.join("7.tmp")).unwrap();
        tmp.try_lock().unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        assert_eq!(others_in(&dir, None), 0);
        drop(tmp);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
