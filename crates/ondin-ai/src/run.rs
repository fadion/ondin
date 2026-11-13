//! The run folder each session works in, and the sweep that removes those of
//! sessions whose owner is gone.

use crate::process::Owner;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How old a folder whose owner's start time could not be read must be before
/// a sweep removes it. Only reached where the platform reports no start times.
const UNOWNED_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// A fresh, exclusive, owner-only folder under `run_root`, its name carrying
/// this process's pid and start time so a later sweep can tell whether its
/// owner is gone. Never a folder that already exists: that one could be
/// somebody else's, or a link into theirs.
pub(crate) fn make(run_root: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(run_root).map_err(|e| e.to_string())?;
    let owner = Owner::me();
    let mut last = String::new();
    for _ in 0..8 {
        let path = run_root.join(folder_name(owner, &random_tag()));
        match create_exclusive_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

/// `s<pid>-<start>-<tag>`; a start of 0 means it could not be read.
fn folder_name(owner: Option<Owner>, tag: &str) -> String {
    let (pid, started) = owner.map_or((std::process::id(), 0), |o| (o.pid, o.started));
    format!("s{pid}-{started}-{tag}")
}

/// The owner a folder name records, and whether it is one of ours at all.
fn owner_in(name: &str) -> Option<(u32, u64)> {
    let mut it = name.strip_prefix('s')?.splitn(3, '-');
    let pid = it.next()?.parse().ok()?;
    let started = it.next()?.parse().ok()?;
    let tag = it.next()?;
    (!tag.is_empty() && tag.bytes().all(|b| b.is_ascii_hexdigit())).then_some((pid, started))
}

/// `mkdir`, not `mkdir -p`, owner-only where the platform has modes. Windows
/// folders inherit the profile's ACL, which other accounts are not in.
fn create_exclusive_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir(path)
    }
}

/// A random hex tag: an OS-seeded hasher over a counter and the clock.
pub(crate) fn random_tag() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(SEQ.fetch_add(1, Ordering::Relaxed));
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    format!("{:016x}", h.finish())
}

/// May a folder with this recorded owner be removed? A live owner's never; a
/// folder whose owner's start time was unknown only once it is old.
fn may_sweep(owner: (u32, u64), age: Duration) -> bool {
    match owner {
        (_, 0) => age > UNOWNED_AFTER,
        (pid, started) => !Owner { pid, started }.is_live(),
    }
}

/// Remove what sessions whose owner is gone left behind under `run_root`:
/// their run folders, what Cursor recorded about their workspaces in the
/// person's own Cursor folder, and Antigravity's registration and rules. A
/// live owner's are kept. Starts `agy` only on a dead owner's claim.
pub fn sweep(run_root: &Path) {
    let now = std::time::SystemTime::now();
    let mut dead = Vec::new();
    if let Ok(entries) = std::fs::read_dir(run_root) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(owner) = owner_in(&name) else {
                continue;
            };
            let age = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .unwrap_or_default();
            if e.path().is_dir() && may_sweep(owner, age) {
                dead.push(e.path());
            }
        }
    }
    if !dead.is_empty() {
        let workspaces: Vec<PathBuf> = dead
            .iter()
            .map(|d| crate::cursor::workspace_in(d))
            .collect();
        crate::cursor::forget(&workspaces);
        for d in dead {
            let _ = std::fs::remove_dir_all(d);
        }
    }
    crate::antigravity::sweep(run_root);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder name records its owner, and a name that is not ours records none.
    #[test]
    fn a_folder_name_round_trips_its_owner() {
        let o = Owner {
            pid: 42,
            started: 7_000,
        };
        let name = folder_name(Some(o), "00ff");
        assert_eq!(name, "s42-7000-00ff");
        assert_eq!(owner_in(&name), Some((42, 7_000)));
        for theirs in [
            "antigravity-owner",
            "s42-7000-",
            "s42-x-00ff",
            "notes",
            "s-1-2-ab",
        ] {
            assert_eq!(owner_in(theirs), None, "{theirs}");
        }
    }

    /// The sweep removes a dead owner's folder and keeps a live one's, this
    /// process's included; a folder of unknown owner waits a day.
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    #[test]
    fn the_sweep_keeps_live_owners_and_removes_dead_ones() {
        let root = std::env::temp_dir().join(format!("ondin-ai-sweep-{}", random_tag()));
        let live = make(&root).expect("a run folder");
        std::fs::write(live.join("f"), "x").unwrap();
        let me = Owner::me().unwrap();
        let dead = root.join(folder_name(
            Some(Owner {
                started: me.started.wrapping_add(1),
                ..me
            }),
            "abcd",
        ));
        std::fs::create_dir(&dead).unwrap();
        std::fs::write(dead.join("f"), "x").unwrap();
        let unknown = root.join(format!("s{}-0-beef", me.pid));
        std::fs::create_dir(&unknown).unwrap();
        sweep(&root);
        assert!(live.is_dir(), "a live owner's folder is kept");
        assert!(!dead.exists(), "a dead owner's folder is removed");
        assert!(unknown.is_dir(), "an unknown owner's new folder waits");
        assert!(may_sweep(
            (me.pid, 0),
            UNOWNED_AFTER + Duration::from_secs(1)
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Two run folders are never the same folder.
    #[test]
    fn run_folders_are_exclusive() {
        let root = std::env::temp_dir().join(format!("ondin-ai-run-{}", random_tag()));
        let a = make(&root).unwrap();
        let b = make(&root).unwrap();
        assert_ne!(a, b);
        assert!(
            create_exclusive_dir(&a).is_err(),
            "an existing folder is refused"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
