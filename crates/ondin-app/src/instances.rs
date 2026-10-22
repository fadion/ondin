//! Which other Ondin processes are running on this machine (§15 D970).
//!
//! Asked before an update is applied: Velopack's updater ends every process running
//! from the install folder on Windows and swaps the app under them elsewhere, which
//! would kill a second window's unsaved work or an `ondin export` a script awaits. So
//! the restart is refused while [`others`] counts anything.
//!
//! Each GUI or headless run holds an exclusive lock on a file of its own in
//! `machine_dir::root()/instances` ([`register`]); the OS drops the lock however the
//! process ends, so a file whose lock can be taken is a leftover and is swept. One
//! path on every platform, instead of enumerating processes per platform.
//!
//! Every registering build counts, including `cargo run`: erring that way costs a
//! developer a click, the other way costs somebody's work.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The folder under `machine_dir::root()`.
const DIR: &str = "instances";

/// This process's own registration, held until the process ends.
///
/// A `static` because eframe can leave the process without unwinding `main`; the
/// file is never removed by this process, and the next [`register`] or [`others`]
/// sweeps it.
static OWN: OnceLock<Registration> = OnceLock::new();

/// One process's lock file, and the handle that holds the lock.
#[derive(Debug)]
struct Registration {
    /// Never read: holding it open is what holds the lock.
    _file: File,
    path: PathBuf,
}

/// Register this process, once. Failure is silent: the process still runs, uncounted.
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

/// The machine's folder, or `None` under `cfg(test)`: a count sweeps, so a test
/// must not reach the real folder (§15 D845). Tests call the `_in` functions.
fn dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(crate::machine_dir::root()?.join(DIR))
}

/// [`register`] in `dir`, as process `pid`.
///
/// Locked under a temporary name, then renamed into place, so a counter never sees
/// a `.lock` file before its lock is held and sweeps it as a leftover. The lock
/// belongs to the open file, so it survives the rename.
fn register_in(dir: &Path, pid: u32) -> Option<Registration> {
    std::fs::create_dir_all(dir).ok()?;
    // Sweep leftovers from ended processes.
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
/// A file that cannot be opened or tested counts as running: "could not tell" is
/// not "no".
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
    /// rather than counted. A lock conflicts between two handles in one process
    /// (`flock` per open file description, `LockFileEx` per handle), so one test
    /// stands in for two processes.
    ///
    /// flip: `running += 1` in the `Ok(())` arm of `others_in` → fails at "a leftover
    /// is not running"; skipping no file as `own` → fails at "one's own registration
    /// is not another".
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

        // The second process ends: the file stays behind, as after a crash.
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
