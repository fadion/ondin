//! Where this machine's own files live: the library index, the cover cache and
//! the font cache (§15 D969).
//!
//! `dirs::cache_dir()/ondin` on Linux and macOS, and `OndinData` beside it on
//! Windows, because there `%LocalAppData%\Ondin` is the Velopack installer's root
//! (the permanent pack id, §15 D956; NTFS ignores case). Setup, repair and uninstall
//! delete a root they own, and `library.json` holds the only record of the stars.
//! The installer owns that folder whole, so the first path component must differ.
//!
//! The old Windows spelling is read once by [`adopt_legacy`].

use std::path::{Path, PathBuf};

/// The folder's name on Linux and macOS, where nothing else owns it.
const DIR_NAME: &str = "ondin";

/// The folder's name on Windows: anything but the installer's pack id, `Ondin`, in
/// any case; the tests hold it to that.
const WINDOWS_DIR_NAME: &str = "OndinData";

/// This machine's data folder, or `None` where the platform has no cache folder.
pub fn root() -> Option<PathBuf> {
    Some(root_in(&dirs::cache_dir()?, cfg!(windows)))
}

/// [`root`], falling back to the temporary folder (the font cache's answer to a
/// machine with no cache folder).
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
    /// The caller must not write the new file this session: an empty index there
    /// would be read from then on and the old one (the stars) never looked at again.
    Blocked,
}

/// Copy `old` to `new` if `new` does not exist yet and `old` does.
///
/// A copy of the bytes, not a parse, so a damaged file is latched as unreadable at
/// the new path just as at the old. A copy, not a move: the old folder belongs to
/// the installer, and an older build run meanwhile still finds its file.
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

    /// The installer's pack id, `--packId` in `.github/workflows/release.yml`.
    const INSTALL_ID: &str = "Ondin";

    /// The Windows folder is not the installer's (compared as NTFS does), and the
    /// workflow still packs with `INSTALL_ID`, which keeps the first check meaningful.
    ///
    /// flip: `WINDOWS_DIR_NAME = "ondin"` or `"ONDIN"` → fails at the first assertion.
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
    /// flip: drop the `new.exists()` early return in `adopt_legacy` → third case
    /// gets `Copied` for `Nothing`; answer `Nothing` on a non-`NotFound` read error
    /// → the fourth fails.
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

        // An old path that exists but cannot be read (a folder) blocks.
        std::fs::remove_file(&new).unwrap();
        std::fs::remove_file(&old).unwrap();
        std::fs::create_dir_all(&old).unwrap();
        assert_eq!(adopt_legacy(&old, &new), Adopted::Blocked);
        assert!(!new.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
