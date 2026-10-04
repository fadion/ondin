//! Where this machine's own files live: the library index, the cover cache and
//! the font cache (§15 D969).
//!
//! `dirs::cache_dir()/ondin` on Linux and macOS, and **`OndinData` beside it on
//! Windows**, because there `dirs::cache_dir()` is `%LocalAppData%` and
//! `%LocalAppData%\Ondin` is **the Velopack installer's root** — the pack id
//! (`release.yml`'s `--packId Ondin`, permanent per §15 D956) joined onto the
//! same folder, and NTFS compares names without regard to case. Setup over a
//! root that is not empty renames it aside and deletes it once the install
//! succeeds, a repair does the same, and uninstall empties it. The covers and
//! the fonts would come back; `library.json` holds every star on the machine,
//! which nothing else records (`library::cache`'s module doc), and it went with
//! them behind a dialog that asked about an *application*.
//!
//! ⚠️ **Another name under the root would not do**: the installer owns
//! `%LocalAppData%\Ondin` whole, so the first path component has to differ. And
//! the pack id cannot move instead — it is the identity every installed copy
//! updates by.
//!
//! The old Windows spelling is still read once, by [`adopt_legacy`], so an index
//! written before this moved is carried across rather than abandoned.

use std::path::{Path, PathBuf};

/// The folder's name on Linux and macOS, where nothing else owns it.
const DIR_NAME: &str = "ondin";

/// The folder's name on Windows: anything but the installer's pack id,
/// `Ondin`, in any case — which the tests below hold it to, reading the pack
/// id out of the Release workflow.
const WINDOWS_DIR_NAME: &str = "OndinData";

/// This machine's data folder, or `None` where the platform has no cache folder.
pub fn root() -> Option<PathBuf> {
    Some(root_in(&dirs::cache_dir()?, cfg!(windows)))
}

/// [`root`], falling back to the temporary folder — the font cache's answer to
/// a machine with no cache folder, which it had before this module existed.
pub fn root_or_temp() -> PathBuf {
    root_in(
        &dirs::cache_dir().unwrap_or_else(std::env::temp_dir),
        cfg!(windows),
    )
}

/// Where the data folder sat before §15 D969, if that is somewhere else.
///
/// `None` off Windows, where the folder never moved.
pub fn legacy_root() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    Some(dirs::cache_dir()?.join(DIR_NAME))
}

/// The folder inside `cache`, decided by the platform rather than read from it,
/// so a test can ask about Windows from any host.
fn root_in(cache: &Path, windows: bool) -> PathBuf {
    cache.join(if windows { WINDOWS_DIR_NAME } else { DIR_NAME })
}

/// What [`adopt_legacy`] found.
#[derive(Debug, PartialEq, Eq)]
pub enum Adopted {
    /// Nothing to do: the new file exists, or the old one does not.
    Nothing,
    /// The old file's bytes now sit at the new path.
    Copied,
    /// An old file is there and could not be read or copied.
    ///
    /// ⚠️ **The caller must not write the new file this session.** An empty
    /// index written at the new path would be read from then on, and the old
    /// one — the stars — would never be looked at again. It is
    /// `LocalIndex::unreadable`'s latch, reached from one step earlier.
    Blocked,
}

/// Copy `old` to `new` if `new` does not exist yet and `old` does.
///
/// **A copy of the bytes, not a parse**: a damaged old file arrives damaged, so
/// the three-way read at the new path sees it and latches exactly as it would
/// have at the old one. **And a copy, not a move**: the old folder belongs to
/// the installer, which deletes it on its own schedule, and an older build run
/// in the meantime still finds its file.
pub fn adopt_legacy(old: &Path, new: &Path) -> Adopted {
    if new.exists() {
        return Adopted::Nothing;
    }
    match std::fs::read(old) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Adopted::Nothing,
        Err(_) => Adopted::Blocked,
        Ok(bytes) => match crate::atomic::write(new, &bytes) {
            Ok(()) => Adopted::Copied,
            Err(_) => Adopted::Blocked,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The installer's pack id: `--packId` in `.github/workflows/release.yml`,
    /// which the first test reads to keep the two in step.
    const INSTALL_ID: &str = "Ondin";

    /// The Windows folder is not the installer's, compared the way NTFS
    /// compares, and the installer's id is the one the Release workflow packs
    /// with (§15 D969, `[R3-L5-01]`).
    ///
    /// **Flip-check, run**: `WINDOWS_DIR_NAME = "ondin"` — the spelling every
    /// build before D969 used — fails the first assertion; so does `"ONDIN"`.
    /// The second assertion is what keeps the first meaningful: a pack id
    /// renamed in the workflow without this constant would leave the first
    /// comparing against a name nothing installs to.
    #[test]
    fn the_windows_folder_is_not_the_installers_root() {
        let cache = Path::new(r"C:\Users\someone\AppData\Local");
        let root = root_in(cache, true);
        let first = root
            .strip_prefix(cache)
            .expect("the folder is inside the cache folder")
            .components()
            .next()
            .expect("and is not the cache folder itself")
            .as_os_str()
            .to_string_lossy()
            .into_owned();
        assert!(
            !first.eq_ignore_ascii_case(INSTALL_ID),
            "`{first}` is the Windows installer's root, which Setup and uninstall delete"
        );

        let workflow = include_str!("../../../.github/workflows/release.yml");
        let packs = workflow.matches("--packId ").count();
        assert!(packs > 0, "the Release workflow names no pack id");
        assert_eq!(
            workflow.matches(&format!("--packId {INSTALL_ID} ")).count()
                + workflow
                    .matches(&format!("--packId {INSTALL_ID}\n"))
                    .count()
                + workflow
                    .matches(&format!("--packId {INSTALL_ID}\r\n"))
                    .count(),
            packs,
            "every `--packId` in release.yml must be `{INSTALL_ID}`"
        );
    }

    /// Off Windows the folder is where it always was, so nothing moves there.
    #[test]
    fn the_folder_off_windows_did_not_move() {
        let cache = Path::new("/home/someone/.cache");
        assert_eq!(root_in(cache, false), cache.join("ondin"));
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ondin-machine-dir-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The four cases of the one-time copy.
    ///
    /// **Flip-check, run**: dropping the `new.exists()` early return fails the
    /// third case with `Copied` for `Nothing` — the old file's bytes would
    /// overwrite the index this machine has written since — and answering
    /// `Nothing` on a read error that is not `NotFound` fails the fourth.
    #[test]
    fn the_old_index_is_copied_once_and_never_over_a_new_one() {
        let dir = scratch("adopt");
        let old = dir.join("old").join("library.json");
        let new = dir.join("new").join("library.json");

        // Neither exists: nothing, and nothing created.
        assert_eq!(adopt_legacy(&old, &new), Adopted::Nothing);
        assert!(!new.exists());

        // Only the old one: copied byte for byte, the old one left in place.
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, b"{\"starred\":[\"a\"]} not json").unwrap();
        assert_eq!(adopt_legacy(&old, &new), Adopted::Copied);
        assert_eq!(
            std::fs::read(&new).unwrap(),
            b"{\"starred\":[\"a\"]} not json"
        );
        assert!(old.exists(), "a copy, not a move");

        // Both: the new one wins and is not touched.
        std::fs::write(&new, b"{}").unwrap();
        assert_eq!(adopt_legacy(&old, &new), Adopted::Nothing);
        assert_eq!(std::fs::read(&new).unwrap(), b"{}");

        // An old path that exists and cannot be read — a folder in its place —
        // blocks, so the caller does not write an empty index over the move.
        std::fs::remove_file(&new).unwrap();
        std::fs::remove_file(&old).unwrap();
        std::fs::create_dir_all(&old).unwrap();
        assert_eq!(adopt_legacy(&old, &new), Adopted::Blocked);
        assert!(!new.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
