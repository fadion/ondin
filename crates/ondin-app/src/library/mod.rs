//! The library: one folder that holds every document, and the rules for putting
//! things into it.
//!
//! **The premise is that the user is never asked where a file goes.** A new
//! document is created from the dashboard, saved and autosaved into the base
//! folder, and found again by name — there is no save dialog, no open dialog and
//! no per-document path to remember. What that buys, and the reason the folder
//! is configurable rather than fixed, is sync: point the base folder at a Drive,
//! Dropbox or OneDrive directory and every document, its history and its
//! grouping travel, with nothing in this app that knows a cloud exists.
//!
//! ⚠️ **That is also the constraint everything here is written against.** A fact
//! the user would expect to survive moving to a second machine may not live in a
//! cache directory, because a cache directory does not sync — so this module's
//! standing question about any new piece of state is *does it travel*, and the
//! answer decides where it is written before it decides anything else.
//!
//! Naming lives in [`naming`]: what a typed name becomes on disk, and why only
//! one of those two functions may name a file.

pub mod cache;
pub mod clock;
pub mod cover;
pub mod ids;
pub mod naming;
pub mod project;
pub mod recovery;
pub mod relocate;
pub mod scan;
pub mod state;
pub mod store;
pub mod writer;

use std::path::{Path, PathBuf};

/// The base folder's default, relative to the user's home: `~/.ondin`.
const DEFAULT_DIR_NAME: &str = ".ondin";

/// Where the library lives — the configured folder, or `~/.ondin`.
///
/// **Resolved per platform rather than hardcoded**, which on Windows is
/// `C:\Users\<name>\.ondin` and not a path with a `~` in it. The dotted name is
/// kept on every platform anyway: the folder is meant to be *findable* — a user
/// pointing their sync client at it has to be able to see it — but it is also
/// not somewhere to browse, since the filenames in it are slugs rather than the
/// names the user typed.
///
/// `override_dir` is the configured value straight from preferences, so an empty
/// setting and an unset one are the same thing here rather than at the call
/// site.
///
/// Returns `None` only when the home directory cannot be resolved *and* nothing
/// is configured — a headless or misconfigured environment where there is no
/// right answer to invent. Every caller has to say what it does about that, and
/// silently writing into the working directory is not it.
pub fn root(override_dir: Option<&Path>) -> Option<PathBuf> {
    match override_dir {
        Some(p) if !p.as_os_str().is_empty() => Some(p.to_path_buf()),
        _ => dirs::home_dir().map(|h| h.join(DEFAULT_DIR_NAME)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_configured_folder_wins_and_an_empty_one_does_not() {
        let configured = Path::new("D:/Dropbox/Ondin");
        assert_eq!(
            root(Some(configured)),
            Some(PathBuf::from("D:/Dropbox/Ondin"))
        );
        // ⚠️ An empty string is what a cleared text field hands over, and it must
        // mean "unset" rather than "the current directory". Flip-check: dropping
        // the `is_empty` guard returns `Some("")` here and fails, while the
        // default case below stays green — the two arms are independent.
        assert_eq!(root(Some(Path::new(""))), root(None));
    }

    #[test]
    fn the_default_sits_under_the_home_directory() {
        // Skipped rather than asserted-around where there is no home: the point
        // of the test is the shape of the path, and inventing one would test the
        // fallback this function deliberately does not have.
        let Some(home) = dirs::home_dir() else { return };
        assert_eq!(root(None), Some(home.join(".ondin")));
    }
}
