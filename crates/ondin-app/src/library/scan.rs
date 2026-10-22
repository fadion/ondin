//! Walking the base folder and turning it into a list of documents.
//!
//! **The filesystem is the truth and a scan is derived from it.** The name and
//! project come from the file's own metadata block, the edit time and size from
//! the directory entry; everything here can be thrown away and rebuilt, so the
//! base folder can be synced, edited in Explorer or restored from a backup.
//!
//! **Two levels deep and no further.** Documents sit at the root or in a
//! project's folder (`super::project`); nothing in the app can put one deeper,
//! and an unbounded walk over a folder pointed at a home directory is a hang.
//!
//! ⚠️ **Dot-directories are skipped**: `.versions`, `.trash` and `.recovery`
//! (`super::recovery`) all hold `.ondin` files that are not library entries, and
//! the leading dot is what distinguishes them.

use super::project::PROJECTS_FILE;
use ondin_core::io::probe::{self, MetaProbe, PREFIX_BYTES};
use ondin_core::meta::DocumentMeta;
use std::io::Read as _;
use std::path::{Path, PathBuf};

/// The extension a library document carries.
pub const DOCUMENT_EXT: &str = "ondin";

/// Where version history and deleted documents live, relative to the root.
///
/// Inside the base folder rather than the cache, so both travel with a synced
/// library. Dot-prefixed so the walk below skips them.
pub const VERSIONS_DIR: &str = ".versions";
/// See [`VERSIONS_DIR`].
pub const TRASH_DIR: &str = ".trash";

/// One document, as the dashboard needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Absolute path to the `.ondin` file.
    pub path: PathBuf,
    /// The filename without its extension — the slug (`super::naming`).
    pub stem: String,
    /// The document's own metadata block, empty for a file that has never been
    /// through the library.
    pub meta: DocumentMeta,
    /// Last modification time, seconds since the Unix epoch.
    ///
    /// **The filesystem's, not the document's**: a stamp written into the file
    /// would end byte-identical re-saves, which is why `DocumentMeta` has no
    /// `modified` field.
    pub modified: u64,
    /// Size in bytes, for the list view's *Size* column.
    pub size: u64,
    /// Whether the metadata had to be guessed because the file could not be
    /// probed — see [`Entry::display_name`].
    pub unread: bool,
}

impl Entry {
    /// What to show in the dashboard: the typed name, or the stem turned back
    /// into words.
    ///
    /// The fallback is a guess: `my-cool-design` becomes "My cool design", wrong
    /// when the original had punctuation or an acronym. It applies only to a
    /// document that predates the library; filing one replaces the guess.
    pub fn display_name(&self) -> String {
        display_name(&self.meta, &self.stem)
    }
}

/// What to call a document: the typed name, or the stem turned back into words.
///
/// One function, because the dashboard (from an [`Entry`]) and the editor's top
/// bar (from the open session) must not disagree about a document's name.
pub fn display_name(meta: &DocumentMeta, stem: &str) -> String {
    if let Some(name) = meta.name.as_deref().filter(|n| !n.trim().is_empty()) {
        return name.to_string();
    }
    de_slug(stem)
}

/// Turn a filename stem back into something readable: dashes to spaces, first
/// letter up.
///
/// Deliberately minimal: no title-casing, since the stem cannot say which
/// capitals the name had.
///
/// Public as [`name_from_stem`] for the importer, which must write the same
/// guess the list shows or an import would rename the document.
pub fn name_from_stem(stem: &str) -> String {
    de_slug(stem)
}

fn de_slug(stem: &str) -> String {
    let spaced = stem.replace('-', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => spaced,
    }
}

/// How sure a filename is that a sync client, rather than a person, put it there.
///
/// A conflict copy is a copy of the *file*, so it carries the original's name,
/// project and id; the marker is the only signal that tells the two cards apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// The name says so in words no [`super::naming::slug`] can emit — a space, a
    /// parenthesis, or a timestamp — so it stands even when the original is gone.
    Stated,
    /// The name is only *shaped* like a second copy — `landing v4 (1)`,
    /// `landing v4 - copy`. A person makes that shape too, so it counts only when
    /// the document it claims to copy is sitting beside it.
    Suspected,
}

/// The conflict marker in `stem`, and the stem it claims to be a copy of.
///
/// **Every pattern here is one `slug` cannot produce**, so the false-positive rate
/// against this app's own files is zero: `slug` emits lowercase alphanumerics and
/// `-`, and collisions take `-1`, `-2` (`super::naming`). A space, a `(` or eight
/// digits proves the name came from elsewhere; `document-2` is therefore not on
/// the list, and `document-conflict-2` is at best [`Conflict::Suspected`].
///
/// ⚠️ **OneDrive's shape is deliberately absent.** It appends the machine name
/// (`landing-v4-DESKTOP-8H2K1`), which is exactly what a typed name slugs to; no
/// pattern matches it without also matching a real document.
pub fn conflict_marker(stem: &str) -> Option<(Conflict, String)> {
    let lower = stem.to_lowercase();
    // --- stated: a phrase with a space or a parenthesis in it ---------------
    // Two spellings: Dropbox puts a name before the phrase
    // (`landing-v4 (Jon's conflicted copy …)`), Nextcloud has no space before the
    // parenthesis. No bare `(conflict` arm: no client writes it, and a
    // hand-named `Notes (conflict resolution)` would get a false warning.
    for marker in [" conflicted copy", "(conflicted copy"] {
        if let Some(at) = lower.find(marker) {
            return Some((Conflict::Stated, base_before(stem, at)));
        }
    }
    // Syncthing: `<base>.sync-conflict-20260102-120000-ABCDEFG`. The eight digits
    // make it stated; `sync-conflict-test` is a name somebody can type.
    if let Some(at) = lower.find(".sync-conflict-") {
        let rest = &lower[at + ".sync-conflict-".len()..];
        if rest.chars().take(8).filter(|c| c.is_ascii_digit()).count() == 8 {
            return Some((Conflict::Stated, base_before(stem, at)));
        }
    }
    // --- suspected: the shapes a person also makes --------------------------
    // `<base> (1)` — Google Drive, and Explorer's paste-into-the-same-folder.
    if let Some(open) = lower.rfind(" (")
        && lower.ends_with(')')
        && lower[open + 2..lower.len() - 1]
            .chars()
            .all(|c| c.is_ascii_digit())
        && lower.len() - open > 3
    {
        return Some((Conflict::Suspected, base_before(stem, open)));
    }
    // `<base> - copy` and `<base> - copy (2)` — Explorer's other one. The app's
    // own *Duplicate* writes "{name} copy", which slugs to `<base>-copy` with no
    // spaces, so this cannot match it.
    if let Some(at) = lower.rfind(" - copy") {
        let tail = &lower[at + " - copy".len()..];
        if tail.is_empty()
            || (tail.starts_with(" (")
                && tail.ends_with(')')
                && tail[2..tail.len() - 1].chars().all(|c| c.is_ascii_digit()))
        {
            return Some((Conflict::Suspected, base_before(stem, at)));
        }
    }
    None
}

/// The stem left when the marker starting at `at` is cut away.
///
/// A marker inside a parenthetical takes its parenthesis with it: cutting
/// Dropbox's `landing-v4 (Jon's conflicted copy …)` at the phrase would leave
/// `(Jon's` glued to the base, so the cut goes back to the last ` (`.
///
/// It can over-trim a base ending in a parenthetical (`my doc (v2) (conflicted
/// copy 1)` gives `my doc`), harmless because only [`Conflict::Suspected`] looks
/// the base up.
fn base_before(stem: &str, at: usize) -> String {
    let head = &stem[..at];
    let cut = head.rfind(" (").unwrap_or(head.len());
    head[..cut]
        .trim_end_matches([' ', '-', '_', '.'])
        .to_string()
}

/// Read a document's metadata without reading the document.
///
/// Takes [`PREFIX_BYTES`] and asks [`probe::meta_in_prefix`]; reads the whole
/// file **only** when the prefix is inconclusive, which never happens for a file
/// this app wrote (so a cloud placeholder is not hydrated by a scan).
///
/// Returns `None` when the file cannot be read at all.
pub fn read_meta(path: &Path) -> Option<(DocumentMeta, bool)> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut prefix = vec![0u8; PREFIX_BYTES];
    let mut filled = 0;
    // A single `read` may return short without being at the end (routine on a
    // hydrating cloud file); loop until it says zero.
    while filled < prefix.len() {
        match file.read(&mut prefix[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        }
    }
    prefix.truncate(filled);
    match probe::meta_in_prefix(&prefix) {
        MetaProbe::Found(m) => Some((m, false)),
        MetaProbe::Absent => Some((DocumentMeta::default(), false)),
        MetaProbe::Inconclusive => {
            // The block did not fit, or the file is not ours: read the rest.
            let bytes = std::fs::read(path).ok()?;
            match ondin_core::io::load(&bytes) {
                Ok(doc) => Some((doc.meta().clone(), false)),
                // An unparseable `.ondin` stays in the list, marked unread.
                Err(_) => Some((DocumentMeta::default(), true)),
            }
        }
    }
}

/// Every document in the library, unordered.
///
/// Errors (an unlistable folder, a file that vanished mid-walk, a permission
/// failure) are dropped: the dashboard shows what it can find, and a missing row
/// beats a library that refuses to open.
pub fn scan(root: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    collect(root, true, &mut out, &mut Vec::new());
    out
}

/// The `.ondin` files in the library whose **filename is not valid Unicode**, so
/// [`scan`] cannot name them and never made them entries (§15 D809).
///
/// A lone UTF-16 surrogate is a legal NTFS filename that `to_str` refuses; a sync
/// client can create one, the app cannot. The document staying out of the list is
/// a limitation, but *Change base folder* must not silently leave it behind, so
/// `super::relocate` carries these across by their `OsStr` name.
///
/// The same walk as [`scan`] — one [`collect`], two outputs — so the two cannot
/// disagree about the two-level rule or the skipped dot-directories.
pub fn unnameable(root: &Path) -> Vec<PathBuf> {
    let mut skipped = Vec::new();
    collect(root, true, &mut Vec::new(), &mut skipped);
    skipped
}

/// A filename the operating system accepts and `OsStr::to_str` refuses
/// (§15 D809), with `.ondin` on the end.
///
/// Shared by this module's tests and `super::relocate`'s, so the platform fact is
/// spelled once.
///
/// The two arms are different facts: on Windows the gap is an unpaired UTF-16
/// surrogate; on Unix it is any byte that is not valid UTF-8. The unix arm is
/// compiled only by CI's Linux and macOS legs; macOS (APFS) refuses such a name,
/// so tests built on it are `ignore`d there.
#[cfg(test)]
pub(crate) fn unnameable_doc_name() -> std::ffi::OsString {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        // A lone high surrogate, then `.ondin`.
        let mut units = vec![0xD800u16];
        units.extend(".ondin".encode_utf16());
        std::ffi::OsString::from_wide(&units)
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let mut bytes = vec![0xFFu8];
        bytes.extend_from_slice(b".ondin");
        std::ffi::OsString::from_vec(bytes)
    }
}

/// Whether the base folder can be listed at all.
///
/// **The one error [`scan`] drops that the app cannot afford to drop.** An
/// unplugged drive, an unmounted sync folder or a renamed base folder comes back
/// as an empty list, indistinguishable from an empty library, and the app then
/// sweeps stars via [`super::cache::LocalIndex::retain_known`].
///
/// False is not by itself a problem: a fresh install has no `~/.ondin` until the
/// first write. What separates the two is whether this machine has recorded
/// anything here — see [`super::state::Library::root_unavailable`]. A directory
/// that exists but cannot be read answers false too.
pub fn readable(root: &Path) -> bool {
    std::fs::read_dir(root).is_ok()
}

/// The documents in one directory, without descending.
///
/// The trash's listing (`super::store::trashed`); [`scan`] cannot be pointed at
/// it because it skips dot-directories.
pub fn scan_dir(dir: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    collect(dir, false, &mut out, &mut Vec::new());
    out
}

/// One directory level. `descend` is false for the project folders, which is how
/// the two-level limit is spelled.
///
/// `unnameable` collects the documents passed over because their filenames are
/// not valid Unicode — see [`unnameable`]. A second output rather than a second
/// walk, so both answer about the same directories.
fn collect(dir: &Path, descend: bool, out: &mut Vec<Entry>, unnameable: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            // A *folder* with such a name is walked, not dropped (§15 D848), or
            // a migration would leave its documents behind uncounted. Descending
            // needs no `&str`; the dot rule is asked of the lossy form, where a
            // leading `.` survives.
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                let hidden = path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'));
                if descend && !hidden {
                    collect(&path, false, out, unnameable);
                }
                continue;
            }
            // Recorded, not merely skipped (§15 D809). The extension is compared
            // as an `OsStr`; the file type is asked so a directory named
            // `<not unicode>.ondin` is not reported. The file is never read, so
            // it is only known to be shaped like a document.
            if path.extension() == Some(std::ffi::OsStr::new(DOCUMENT_EXT))
                && entry.file_type().map(|t| t.is_file()).unwrap_or(false)
            {
                unnameable.push(path);
            }
            continue;
        };
        // Keeps `.versions`, `.trash` and `.recovery` out. See module docs.
        if name.starts_with('.') {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if descend {
                collect(&path, false, out, unnameable);
            }
            continue;
        }
        if name == PROJECTS_FILE {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some(DOCUMENT_EXT) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some((meta, unread)) = read_meta(&path) else {
            continue;
        };
        let (modified, size) = match entry.metadata() {
            Ok(m) => (
                m.modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                m.len(),
            ),
            Err(_) => (0, 0),
        };
        out.push(Entry {
            stem: stem.to_string(),
            path,
            meta,
            modified,
            size,
            unread,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::{Document, IdSource};

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ondin-scan-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Write a real document, so assertions run against bytes the app produces.
    fn write_doc(path: &Path, meta: DocumentMeta) {
        let mut ids = IdSource::new(0xD0C);
        let mut doc = Document::new(ids.mint());
        doc.set_meta(meta);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, ondin_core::io::save(&doc).unwrap()).unwrap();
    }

    fn named(name: &str, project: Option<&str>) -> DocumentMeta {
        DocumentMeta {
            id: Some(super::super::ids::mint()),
            name: Some(name.into()),
            project: project.map(str::to_string),
            created: Some(1_774_483_200),
            kit: None,
        }
    }

    #[test]
    fn a_scan_finds_documents_at_the_root_and_one_level_down() {
        let root = temp_root("levels");
        write_doc(&root.join("landing-v4.ondin"), named("Landing v4", None));
        write_doc(
            &root.join("kestrel").join("pricing-table.ondin"),
            named("Pricing table", Some("proj-1")),
        );
        let mut names: Vec<String> = scan(&root).iter().map(|e| e.display_name()).collect();
        names.sort();
        assert_eq!(names, vec!["Landing v4", "Pricing table"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Version history and the trash are not listed as documents.
    ///
    /// flip: removing the `starts_with('.')` guard in `collect` surfaces two files,
    /// not three — `.versions/<id>/<stamp>` is three levels down and the depth
    /// limit already refuses it, so the dot guard is load-bearing for `.trash`
    /// alone. Flattening `.versions` or lifting the depth limit changes which
    /// rule covers it; do not change both assuming the other still does.
    #[test]
    fn the_dot_directories_are_not_part_of_the_library() {
        let root = temp_root("dots");
        write_doc(&root.join("landing-v4.ondin"), named("Landing v4", None));
        write_doc(
            &root.join(VERSIONS_DIR).join("abc").join("1774483200.ondin"),
            named("Landing v4", None),
        );
        write_doc(
            &root.join(TRASH_DIR).join("deleted-thing.ondin"),
            named("Deleted thing", None),
        );
        let found = scan(&root);
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].display_name(), "Landing v4");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Pins the two-level limit: a document three levels down is not listed.
    #[test]
    fn a_document_three_levels_down_is_not_found() {
        let root = temp_root("depth");
        write_doc(
            &root.join("kestrel").join("deep").join("buried.ondin"),
            named("Buried", None),
        );
        assert!(scan(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An unfiled document shows a de-slugged filename; a filed one its typed name.
    #[test]
    fn an_unfiled_document_falls_back_to_its_filename() {
        let root = temp_root("fallback");
        write_doc(&root.join("old-sketch.ondin"), DocumentMeta::default());
        write_doc(
            &root.join("my-cool-design.ondin"),
            named("My cool design", None),
        );
        let mut found = scan(&root);
        found.sort_by_key(|e| e.stem.clone());
        assert_eq!(found[0].display_name(), "My cool design");
        assert_eq!(found[1].display_name(), "Old sketch");
        assert!(found[1].meta.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `projects.json` and anything that is not a `.ondin` stay out of the list.
    #[test]
    fn only_documents_are_listed() {
        let root = temp_root("filter");
        write_doc(&root.join("real.ondin"), named("Real", None));
        std::fs::write(root.join(PROJECTS_FILE), b"{}").unwrap();
        std::fs::write(root.join("notes.txt"), b"hello").unwrap();
        std::fs::write(root.join("photo.png"), b"\x89PNG").unwrap();
        let found = scan(&root);
        assert_eq!(found.len(), 1, "{found:#?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A `.ondin` this build cannot parse is still listed, marked `unread`.
    ///
    /// Two fixtures, not two examples of one case: the truncated object lands on
    /// `MetaProbe::Inconclusive`, the only `read_meta` arm that can report unread,
    /// while a renamed PNG answers `Absent` and must still be marked unread.
    #[test]
    fn a_corrupt_document_is_listed_rather_than_hidden() {
        let root = temp_root("corrupt");
        std::fs::write(
            root.join("half-synced.ondin"),
            b"{ \"schema_version\": 3, \"nod",
        )
        .unwrap();
        // Not an object at all.
        std::fs::write(root.join("not-ours.ondin"), b"\x89PNG\r\n\x1a\n\x00\x00").unwrap();
        let found = scan(&root);
        assert_eq!(found.len(), 2, "{found:#?}");
        for entry in &found {
            assert!(
                entry.unread,
                "a .ondin this build cannot parse is unread however it is malformed: \
                 {entry:#?}"
            );
        }
        // By stem, not by index: `scan` is unordered.
        let half = found
            .iter()
            .find(|e| e.path.file_stem().is_some_and(|s| s == "half-synced"))
            .expect("the truncated fixture is listed");
        assert_eq!(half.display_name(), "Half synced");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_root_scans_to_nothing_rather_than_failing() {
        let root = std::env::temp_dir().join("ondin-scan-does-not-exist-xyz");
        let _ = std::fs::remove_dir_all(&root);
        assert!(scan(&root).is_empty());
    }

    /// Every conflict shape is caught, and no name this app writes is.
    ///
    /// The slugged-name block is the test: a marker firing on `landing-v4-2` would
    /// warn on a fresh *Duplicate*. Its negatives go through `super::naming::slug`
    /// rather than being typed to agree with the heuristic.
    ///
    /// flip: a bare `contains("conflict")` or `contains('(')` arm fails first at
    /// the `landing-v4.sync-conflict-notes` assertion, not in the slugged block.
    #[test]
    fn conflict_markers_catch_the_clients_and_never_this_app() {
        use super::super::naming::slug;
        let kind = |s: &str| conflict_marker(s).map(|(k, _)| k);
        let base = |s: &str| conflict_marker(s).map(|(_, b)| b);

        // Dropbox puts a person's name inside the parenthetical, so the phrase is
        // not at the bracket — and the base has to lose the bracket anyway.
        assert_eq!(
            kind("landing-v4 (Jon's conflicted copy 2026-01-02)"),
            Some(Conflict::Stated)
        );
        assert_eq!(
            base("landing-v4 (Jon's conflicted copy 2026-01-02)").as_deref(),
            Some("landing-v4")
        );
        // Nextcloud, with no name in front of it.
        assert_eq!(
            kind("landing-v4 (conflicted copy 2026-01-02 120000)"),
            Some(Conflict::Stated)
        );
        // Syncthing — stated only because of the eight digits.
        assert_eq!(
            kind("landing-v4.sync-conflict-20260102-120000-K7QW3A"),
            Some(Conflict::Stated)
        );
        assert_eq!(
            kind("landing-v4.sync-conflict-notes"),
            None,
            "a date is what makes this a client's and not a person's"
        );
        // Google Drive and Explorer: shapes a person also makes, so the list has
        // to be consulted before either is called a conflict.
        assert_eq!(kind("landing-v4 (1)"), Some(Conflict::Suspected));
        assert_eq!(base("landing-v4 (1)").as_deref(), Some("landing-v4"));
        assert_eq!(kind("landing-v4 - Copy"), Some(Conflict::Suspected));
        assert_eq!(kind("landing-v4 - Copy (2)"), Some(Conflict::Suspected));

        // Hand-made filenames cannot have gone through `slug`, so the `slug`
        // block below does not protect them.
        for stem in [
            "Notes (conflict resolution)",
            "Palette (draft)",
            "Landing v4 (final)",
        ] {
            assert_eq!(
                conflict_marker(stem),
                None,
                "{stem:?} is somebody's filename, not a client's"
            );
        }

        // Nothing this app can write may match; names go through the library's `slug`.
        for name in [
            "Landing v4",
            "Landing v4 copy", // the app's own *Duplicate*
            "Conflict",        // and the words themselves
            "Conflicted copy", // even this one, once it has been slugged
            "Conflict resolution flow",
            "Poster (2)",
            "Sync conflict notes",
            "My doc - Copy",
        ] {
            let stem = slug(name);
            assert_eq!(
                conflict_marker(&stem),
                None,
                "{name:?} slugs to {stem:?}, which this app writes"
            );
            // And its collision suffixes (`naming::unique_stem`).
            for n in 1..=3 {
                let with = format!("{stem}-{n}");
                assert_eq!(conflict_marker(&with), None, "{with:?}");
            }
        }
    }

    /// A `.ondin` whose filename is not valid Unicode is **not an entry** and
    /// **is** reported by `unnameable` (§15 D809).
    ///
    /// Three controls keep a walk that reported everything from passing: an
    /// ordinary document is an entry and unreported; a non-Unicode name that is
    /// not a `.ondin` is neither; the second level is walked.
    ///
    /// flip: dropping the `path.extension() == …` term in `collect` fails at the
    /// `reported == want` assertion (the `.txt` control is a third entry there);
    /// dropping `unnameable.push` fails at the same assertion with an empty left.
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "APFS refuses a name that is not UTF-8, so the case cannot arise (§15 D967)"
    )]
    fn a_document_with_a_non_unicode_filename_is_not_listed_and_is_reported() {
        let root = temp_root("unnameable");
        write_doc(&root.join("landing-v4.ondin"), named("Landing v4", None));
        let project = root.join("kestrel");
        std::fs::create_dir_all(&project).unwrap();

        let odd = root.join(unnameable_doc_name());
        let deep = project.join(unnameable_doc_name());
        // Raw bytes: the name alone decides, the content is never read.
        std::fs::write(&odd, b"{}").expect("the OS accepts this name");
        std::fs::write(&deep, b"{}").unwrap();
        // The not-a-document control, beside it under the same broken name.
        let mut other = unnameable_doc_name();
        other.push(".txt");
        std::fs::write(root.join(&other), b"x").unwrap();

        let stems: Vec<String> = scan(&root).into_iter().map(|e| e.stem).collect();
        assert_eq!(
            stems,
            vec!["landing-v4".to_string()],
            "the ordinary document is the only entry"
        );

        let mut reported = unnameable(&root);
        reported.sort();
        let mut want = vec![odd, deep];
        want.sort();
        assert_eq!(
            reported, want,
            "both are reported, including the one a level down"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
