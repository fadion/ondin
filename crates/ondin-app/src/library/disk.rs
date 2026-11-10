//! Whether an open document's file changed under it: written by another program, a
//! sync client, another machine, `git`, or `ondin merge -o` / `ondin run -o`.
//!
//! A session keeps a [`Stamp`] of the bytes it last read from or wrote to its file,
//! and every save of the document asks [`Stamp::check`] first. A save that would
//! write over someone else's change is held, and the user is asked
//! (`OndinApp::disk_clash_card`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

/// The library's own rewrites of a closed or parked document's file (a rename, a
/// move, a kit mark: `store::rewrite`), as the digest read and the digest written.
/// A stamp follows them, so a tab holding the file does not take the library's
/// write for someone else's. Keyed by digest, not path, since a rename then moves
/// the file.
static REWRITES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// Record that this process rewrote the bytes digested `from` into `to`.
pub fn rewrote(from: &str, to: &str) {
    let mut map = REWRITES.lock().unwrap_or_else(|e| e.into_inner());
    map.get_or_insert_with(HashMap::new)
        .insert(from.to_owned(), to.to_owned());
}

/// Whether this process's own rewrites lead from `from` to `to`.
fn rewritten(from: &str, to: &str) -> bool {
    let map = REWRITES.lock().unwrap_or_else(|e| e.into_inner());
    let Some(map) = map.as_ref() else {
        return false;
    };
    let mut at = from;
    // Bounded by the map: a rewrite and its undo make a cycle.
    for _ in 0..map.len() {
        match map.get(at) {
            Some(next) if next == to => return true,
            Some(next) => at = next,
            None => return false,
        }
    }
    false
}

/// The file as this session last read or wrote it: its size and modification time,
/// which are cheap to compare, and the digest of its bytes, which decides when
/// those two moved but the contents did not (a sync client rewriting the same
/// bytes, a `touch`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    digest: String,
}

impl Stamp {
    /// The stamp of `path` just written with bytes whose digest is `digest`
    /// (`history::digest`). `None` when the file cannot be looked at, which leaves
    /// the session's saves unchecked.
    pub fn of(path: &Path, digest: String) -> Option<Self> {
        Some(Self::from_meta(&std::fs::metadata(path).ok()?, digest))
    }

    /// The stamp of a file whose metadata was `meta` **before** its bytes, with
    /// digest `digest`, were read: a write between the two then leaves a stamp
    /// whose time no longer matches, which [`Self::check`] reads again, rather than
    /// one that matches the new file with the old digest.
    pub fn from_meta(meta: &std::fs::Metadata, digest: String) -> Self {
        Self {
            len: meta.len(),
            modified: meta.modified().ok(),
            digest,
        }
    }

    /// Whether `path` now holds something other than what this stamp recorded.
    ///
    /// A file that is gone, or cannot be looked at, has not changed in this sense:
    /// the save recreates it or reports its own failure. Size and time moved with
    /// the same bytes, or bytes this process rewrote them into ([`rewrote`]),
    /// refresh the stamp rather than ask.
    pub fn check(&mut self, path: &Path) -> bool {
        let Ok(meta) = std::fs::metadata(path) else {
            return false;
        };
        let modified = meta.modified().ok();
        if meta.len() == self.len && modified == self.modified {
            return false;
        }
        let Ok(bytes) = std::fs::read(path) else {
            return false;
        };
        let now = super::history::digest(&bytes);
        if now != self.digest {
            if !rewritten(&self.digest, &now) {
                return true;
            }
            self.digest = now;
        }
        self.len = meta.len();
        self.modified = modified;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ondin-disk-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Other bytes are a change; the same bytes written again are not, and refresh
    /// the stamp; a file gone is not.
    ///
    /// flip: `check` answering on size and time alone → fails at "the same bytes
    /// again".
    #[test]
    fn only_other_bytes_count_as_a_change() {
        let dir = dir("check");
        let path = dir.join("a.ondin");
        std::fs::write(&path, b"mine").unwrap();
        let mut stamp = Stamp::of(&path, super::super::history::digest(b"mine")).unwrap();
        assert!(!stamp.check(&path), "untouched");

        // Another size, so the cheap comparison cannot pass whatever the clock.
        std::fs::write(&path, b"mine, again").unwrap();
        std::fs::write(&path, b"mine").unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1))
            .unwrap();
        drop(file);
        assert!(!stamp.check(&path), "the same bytes again");
        assert_eq!(
            stamp.modified,
            std::fs::metadata(&path).unwrap().modified().ok(),
            "and the stamp takes up the new time"
        );

        std::fs::write(&path, b"theirs").unwrap();
        assert!(stamp.check(&path), "other bytes");

        std::fs::remove_file(&path).unwrap();
        assert!(!stamp.check(&path), "gone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The library's own rewrites, two in a row, are followed rather than asked
    /// about; a rewrite of someone else's bytes is not.
    ///
    /// flip: `check` without its `rewritten` arm → fails at "two rewrites
    /// followed".
    #[test]
    fn the_librarys_own_rewrites_are_followed() {
        use super::super::history::digest;
        let dir = dir("rewrites");
        let path = dir.join("a.ondin");
        let (a, b, c) = (
            &b"disk-a"[..],
            &b"disk-b, renamed"[..],
            &b"disk-c, marked"[..],
        );
        std::fs::write(&path, a).unwrap();
        let mut stamp = Stamp::of(&path, digest(a)).unwrap();
        rewrote(&digest(a), &digest(b));
        rewrote(&digest(b), &digest(c));
        std::fs::write(&path, c).unwrap();
        assert!(!stamp.check(&path), "two rewrites followed");
        assert_eq!(stamp.digest, digest(c), "and the stamp is the file now");

        let theirs = &b"disk-d, someone else's"[..];
        std::fs::write(&path, theirs).unwrap();
        rewrote(&digest(theirs), &digest(b"disk-e"));
        std::fs::write(&path, b"disk-e").unwrap();
        assert!(stamp.check(&path), "a rewrite of their change still asks");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
