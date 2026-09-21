//! Walking the base folder and turning it into a list of documents.
//!
//! **The filesystem is the truth and this is derived from it.** Nothing about a
//! document lives only in a scan: the name and the project come out of the
//! file's own metadata block, the edit time and the size come off the directory
//! entry, and everything here can be thrown away and rebuilt. That is what makes
//! the base folder something a user can point a sync client at, edit in
//! Explorer, or restore from a backup, without the app needing to be told.
//!
//! **Two levels deep and no further.** Documents sit at the root of the library
//! or inside a project's folder, which is one level down (`super::project`), so
//! that is the whole of the tree this walks. A deeper limit would be a promise
//! about arbitrary nesting the rest of the library does not keep — a document
//! three levels down could not be *put* there by anything in the app — and an
//! unbounded walk over a folder someone has pointed at their entire home
//! directory is a hang rather than a feature.
//!
//! ⚠️ **Dot-directories are skipped**, which is not tidiness: `.versions` holds
//! a copy of every document under its own id, so a walk that descended into it
//! would list every version of everything as a separate file; `.trash` holds
//! documents whose whole state is "deleted"; and `.recovery` holds a crash
//! snapshot of whatever was open, which is not a document at all until somebody
//! says it is (`super::recovery`). All three are `.ondin` files that are not
//! library entries, and this is the only thing that distinguishes them.

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
/// library — a version history that does not follow the file it belongs to is
/// one nobody will rely on. Dot-prefixed so the walk below skips them and so a
/// user opening the folder sees their documents rather than the machinery.
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
    /// **The filesystem's, not the document's**, which is the whole reason there
    /// is no `modified` field in `DocumentMeta`: a stamp written into the file
    /// on every save would make every save rewrite a line and end byte-identical
    /// re-saves for the format. The directory entry already knows, for free, and
    /// travels through every sync client there is.
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
    /// ⚠️ **The fallback is a guess and reads like one.** `my-cool-design`
    /// becomes "My cool design", which is right often enough to be worth doing
    /// and wrong whenever the original had punctuation, an acronym or a capital
    /// in the middle. It only ever applies to a document that predates the
    /// library or was written by something else; anything this app has filed
    /// carries the real name, and filing one is what replaces the guess.
    pub fn display_name(&self) -> String {
        display_name(&self.meta, &self.stem)
    }
}

/// What to call a document: the typed name, or the stem turned back into words.
///
/// ⚠️ **One function, because two callers must not disagree.** The dashboard
/// names a document from an [`Entry`] and the editor's top bar names the *open*
/// one from its session — and a document showing as "Landing v4" in the library
/// and `landing-v4` in the title bar is the drift this exists to prevent. It is
/// also the bug the library introduced and nearly shipped: `document_title` read
/// the file stem, which used to be whatever the user typed into a save dialog
/// and is now a slug.
pub fn display_name(meta: &DocumentMeta, stem: &str) -> String {
    if let Some(name) = meta.name.as_deref().filter(|n| !n.trim().is_empty()) {
        return name.to_string();
    }
    de_slug(stem)
}

/// Turn a filename stem back into something readable: dashes to spaces, first
/// letter up.
///
/// Deliberately minimal — it does not title-case every word, because "Landing
/// V4" is worse than "Landing v4" and there is no way to tell which one the
/// stem came from.
///
/// Public as [`name_from_stem`] for the importer, which has to *write* a name
/// for a document that arrives without one — the same guess the list makes, and
/// it has to be the same one or an imported document would be renamed by being
/// imported.
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
/// ⚠️ **The whole reason this is worth having is that a conflict copy is a copy
/// of the *file*** — so it carries the original's metadata block, and therefore
/// the original's typed name, its project and its id. Two cards, one name, one
/// project dot, one cover, and the only thing that differs is a filename the
/// dashboard never shows. Marking one of the pair is not decoration; it is the
/// only signal there is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// The name says so in words no [`super::naming::slug`] can emit — a space, a
    /// parenthesis, or a timestamp. Nothing this app writes can be mistaken for
    /// one, so it stands on its own even when the document it copied is gone.
    Stated,
    /// The name is only *shaped* like a second copy — `landing v4 (1)`,
    /// `landing v4 - copy`. A person makes that shape too, so it counts only when
    /// the document it claims to be a copy of is sitting beside it.
    Suspected,
}

/// The conflict marker in `stem`, and the stem it claims to be a copy of.
///
/// **Every pattern here is one `slug` cannot produce**, which is what makes the
/// false-positive rate against this app's *own* files exactly zero rather than
/// merely low: `slug` emits lowercase alphanumerics and `-`, and collisions take
/// `-1`, `-2` (`super::naming`). So a space, a `(`, or a run of eight digits is
/// proof the name came from somewhere else. That is the load-bearing fact, and
/// it is why `document-2.ondin` — which this app writes all the time — is not on
/// the list, and why the roadmap's own illustration `document-conflict-2` is
/// [`Conflict::Suspected`] at best: "Document conflict 2" is a name a person can
/// type, and it slugs to exactly that.
///
/// ⚠️ **OneDrive's shape is deliberately absent.** It appends the machine name —
/// `landing-v4-DESKTOP-8H2K1.ondin` — which is a hyphen and a word, i.e. exactly
/// what a typed name slugs to. There is no pattern to match that does not also
/// match "Landing v4 desktop 8h2k1", so this reports nothing rather than
/// guessing, and the gap is recorded here instead of in a heuristic that fires on
/// somebody's real document.
pub fn conflict_marker(stem: &str) -> Option<(Conflict, String)> {
    let lower = stem.to_lowercase();
    // --- stated: a phrase with a space or a parenthesis in it ---------------
    // ⚠️ **Two spellings and not one**, because Dropbox puts a *name* in front of
    // the phrase — `landing-v4 (Jon's conflicted copy 2026-01-02)` — so a search
    // for `(conflicted` misses it entirely and a search for ` conflicted copy`
    // misses Nextcloud's `(conflicted copy …)`, which has no space before it.
    //
    // ⚠️ **And two rather than three.** A bare `(conflict` was here to catch a
    // parenthetical that does not spell out "copy", and it is exactly the kind of
    // arm this whole function is written against: no client is known to produce
    // it, while `Notes (conflict resolution).ondin` — a file somebody named by
    // hand, which is the only way that name can exist, since `slug` emits no
    // parentheses — matches it and gets told it is a sync artefact. A pattern
    // that costs a real document a false warning has to be paid for by a client
    // that actually writes it.
    for marker in [" conflicted copy", "(conflicted copy"] {
        if let Some(at) = lower.find(marker) {
            return Some((Conflict::Stated, base_before(stem, at)));
        }
    }
    // Syncthing: `<base>.sync-conflict-20260102-120000-ABCDEFG`. The eight digits
    // are what make this stated rather than suspected — `sync-conflict-test` is a
    // name somebody can type, `sync-conflict-20260102` is not.
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
/// ⚠️ **A marker inside a parenthetical takes its parenthesis with it.** Dropbox
/// writes `landing-v4 (Jon's conflicted copy 2026-01-02)`, where the phrase
/// begins after `(Jon's` — cutting at the phrase alone would leave `(Jon's`
/// glued to the base and no lookup would ever find the original. So the cut goes
/// back to the last ` (` when there is one.
///
/// It can over-trim a base that legitimately ends in a parenthetical
/// (`my doc (v2) (conflicted copy 1)` gives `my doc`), which costs nothing today:
/// the base is only *looked up* for [`Conflict::Suspected`], and a stated
/// conflict is stated whatever it was a copy of.
fn base_before(stem: &str, at: usize) -> String {
    let head = &stem[..at];
    let cut = head.rfind(" (").unwrap_or(head.len());
    head[..cut]
        .trim_end_matches([' ', '-', '_', '.'])
        .to_string()
}

/// Read a document's metadata without reading the document.
///
/// Opens the file, takes [`PREFIX_BYTES`], and asks
/// [`probe::meta_in_prefix`]. Falls back to reading the whole file **only** when
/// the prefix was inconclusive, which for a file this app wrote never happens —
/// see the module docs on cloud placeholders for why that distinction is the
/// point of the function rather than an optimisation.
///
/// Returns `None` when the file cannot be read at all.
pub fn read_meta(path: &Path) -> Option<(DocumentMeta, bool)> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut prefix = vec![0u8; PREFIX_BYTES];
    let mut filled = 0;
    // A single `read` may return fewer bytes than asked for without being at the
    // end — on a hydrating cloud file, routinely. Loop until it says zero.
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
            // The block is there but did not fit, or the file is not ours.
            // Reading the rest is the honest answer and the rare one.
            let bytes = std::fs::read(path).ok()?;
            match ondin_core::io::load(&bytes) {
                Ok(doc) => Some((doc.meta().clone(), false)),
                // A `.ondin` this build cannot parse still belongs in the list —
                // hiding it would make a file the user can see in Explorer
                // invisible in the app that owns the folder.
                Err(_) => Some((DocumentMeta::default(), true)),
            }
        }
    }
}

/// Every document in the library, unordered.
///
/// Errors are dropped rather than reported: a folder that cannot be listed, a
/// file that vanished between the walk and the stat, a permission failure on one
/// subdirectory. **The dashboard's contract is that it shows what it can find**,
/// and a library that refuses to open because one entry misbehaved is worse than
/// one that is missing a row.
pub fn scan(root: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    collect(root, true, &mut out, &mut Vec::new());
    out
}

/// The `.ondin` files in the library whose **filename is not valid Unicode**, so
/// [`scan`] cannot name them and never made them entries (§15 D809).
///
/// 🚨 **Reproduced, which is what this function is for.** It was recorded as a
/// hazard nobody had established — *"it is not established that a filename
/// Windows accepts can fail `to_str`"* — and it is: a lone UTF-16 surrogate is a
/// legal code unit and not a scalar value, `std::fs::write` on a path built from
/// `OsString::from_wide(&[0xD800, …])` **succeeds** on NTFS, and the entry
/// `read_dir` hands back answers `None` to `file_name().to_str()`. Measured on
/// this machine over three shapes — a lone high surrogate, a lone low one and a
/// reversed pair — with an ASCII name and a well-formed astral pair as controls.
/// Nothing in this app can *create* one; a sync client or another tool can.
///
/// ⚠️ **What it exists for is the migration and not the list.** The document
/// staying out of the library is a limitation — there is no honest label to draw
/// for a name that is not text — and it being *silently left behind* by *Change
/// base folder* is a loss: the file ends up alone in a folder the app no longer
/// reads, which is the shape §15 D431 fixed for a `.trash` collision and did not
/// fix here. `super::relocate` carries these across by their `OsStr` name.
///
/// **The same walk as [`scan`], by construction rather than by agreement** — one
/// [`collect`], two outputs — so the two cannot come to disagree about the
/// two-level rule or about which dot-directories are skipped.
pub fn unnameable(root: &Path) -> Vec<PathBuf> {
    let mut skipped = Vec::new();
    collect(root, true, &mut Vec::new(), &mut skipped);
    skipped
}

/// A filename the operating system accepts and `OsStr::to_str` refuses
/// (§15 D809), with `.ondin` on the end.
///
/// **Beside `unnameable` rather than inside a test module**, because two
/// modules' tests need it — this one's and `super::relocate`'s, which asserts the
/// other half of D809 — and a platform fact spelled twice is a platform fact that
/// can come to disagree with itself.
///
/// ⚠️ **The two arms are two different facts, not one fact spelled twice.** On
/// Windows a name is UTF-16 and the gap is an **unpaired surrogate** — a legal
/// code unit that is not a scalar value; on Unix it is bytes and the gap is any
/// byte that is not valid UTF-8. Neither construction means anything on the other
/// platform.
///
/// ⚠️ **`#[cfg(unix)]` is compiled by nothing on this machine** — `CLAUDE.md`
/// lists four production functions in the same position and no gate for them — so
/// the arm below is written to be read rather than trusted, and the Windows one is
/// what the measurement behind D809 was taken on.
///
/// Plain backticks throughout: this item is `#[cfg(test)]`, so `cargo doc` never
/// builds it and an intra-doc link here would resolve against nothing and be
/// checked by no gate (§15 D319).
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
/// **The one error [`scan`] drops that the app cannot afford to drop.** Every
/// other failure above costs one row; this one costs the whole library and comes
/// back as an empty list — indistinguishable, to everything downstream, from a
/// library that is genuinely empty. An unplugged drive, a sync folder that has
/// not mounted yet, a base folder renamed in Explorer: all three read as "you
/// have no files", and one of the things the app then does about that is
/// [`super::cache::LocalIndex::retain_known`], which sweeps the stars of every
/// document it cannot see.
///
/// ⚠️ **False is not by itself a problem**: a fresh install has no `~/.ondin`
/// either, because the folder is created by the first write rather than at
/// launch. What separates the two is whether this machine has ever recorded
/// anything in this library — see [`super::state::Library::root_unavailable`].
///
/// A directory that exists but cannot be *read* (permissions) answers false too,
/// which is the honest answer: the library is not visible from here.
pub fn readable(root: &Path) -> bool {
    std::fs::read_dir(root).is_ok()
}

/// The documents in one directory, without descending.
///
/// The trash's listing (`super::store::trashed`) — a flat folder of `.ondin`
/// files that are deliberately *not* library entries, which is why [`scan`]
/// cannot be pointed at it: [`scan`] skips dot-directories, and the trash is
/// one.
pub fn scan_dir(dir: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    collect(dir, false, &mut out, &mut Vec::new());
    out
}

/// One directory level. `descend` is false for the project folders, which is how
/// the two-level limit is spelled.
///
/// `unnameable` collects the documents this walk had to pass over because their
/// filenames are not valid Unicode — see [`unnameable`], which is the only caller
/// that reads it. A second output rather than a second walk: the two questions
/// are answered about the same directories, and nothing else here could make them
/// stay that way.
fn collect(dir: &Path, descend: bool, out: &mut Vec<Entry>, unnameable: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            // **Recorded, not merely skipped** (§15 D809). The extension is
            // compared as an `OsStr`, since the whole case here is a name that
            // has no `&str` to compare — and the file type is asked because a
            // *directory* called `<not unicode>.ondin` is not a document, however
            // unlikely. Everything else about such an entry is unknowable from
            // here: it is not read, so it is not known to be a document, only to
            // be shaped like one.
            if path.extension() == Some(std::ffi::OsStr::new(DOCUMENT_EXT))
                && entry.file_type().map(|t| t.is_file()).unwrap_or(false)
            {
                unnameable.push(path);
            }
            continue;
        };
        // The one rule that keeps `.versions` and `.trash` out. See module docs.
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

    /// Write a real document, so every assertion below is against bytes the app
    /// actually produces rather than against a fixture that agrees with the test.
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

    /// ⚠️ The rule that stops version history and the trash from being listed as
    /// documents. Both hold real `.ondin` files, so nothing but the leading dot
    /// tells them apart.
    ///
    /// ⚠️ **Flip-check, run — and the predicted number was wrong, which is the
    /// finding.** Removing the `starts_with('.')` guard was supposed to surface
    /// all three files; it surfaces **two**, because `.versions/<id>/<stamp>` is
    /// three levels down and the depth limit above already refuses it. So the
    /// dot guard is load-bearing for `.trash` alone, and version history is kept
    /// out of the library by a rule that has nothing to do with the dot and was
    /// written for another reason entirely.
    ///
    /// That is worth knowing before anyone "simplifies" either one: flattening
    /// `.versions` to `<root>/.versions/<id>-<stamp>.ondin` would move it to two
    /// levels and put it back under the dot guard, and lifting the depth limit
    /// would do the same in reverse. Neither is wrong; they just cannot both be
    /// changed while assuming the other still covers this.
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

    /// Three levels down is not reachable by anything in the app, so it is not
    /// listed — the assertion that pins the two-level limit rather than leaving
    /// it as a comment.
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

    /// A pre-library document shows a de-slugged filename, and the file beside it
    /// that *has* been filed shows its typed name — including the capitals and
    /// the space no filename could have carried.
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
        // And the guess is marked as coming from the stem, not from the file.
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
    /// Hiding it would make a file the user can see in Explorer invisible in the
    /// app that owns the folder — and a sync conflict is exactly how a truncated
    /// one appears.
    ///
    /// ⚠️ **Two fixtures, because one of them never reached the state this test is
    /// named for** (§15 D485, `[S1.1-L1-04]`). The truncated object is the shape
    /// the doc above describes and it lands on `MetaProbe::Inconclusive`, which is
    /// the only arm of `read_meta` that reads the file and can report it unread. A
    /// **renamed PNG** — a `.ondin` this build equally cannot parse — answered
    /// `Absent` instead, which `read_meta` maps to a healthy pre-library document,
    /// so it was listed as fine. The test could not see that: it asserted the one
    /// corrupt shape whose first byte happens to be `{`.
    ///
    /// So the two fixtures are not two examples of one case. They are the two sides
    /// of the byte that used to decide it.
    #[test]
    fn a_corrupt_document_is_listed_rather_than_hidden() {
        let root = temp_root("corrupt");
        std::fs::write(
            root.join("half-synced.ondin"),
            b"{ \"schema_version\": 3, \"nod",
        )
        .unwrap();
        // Not an object at all, which is the half that used to read as healthy.
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
        // ⚠️ **By stem, not by index.** With one fixture `found[0]` was safe; with
        // two it depends on `read_dir` order, and `scan`'s own doc says *"every
        // document in the library, **unordered**"*. It passes on NTFS and would be
        // a flake on a filesystem that answers the other way — a positional
        // dependency introduced by widening the fixture, which is the ordinary way
        // a test acquires one.
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

    /// Every conflict shape this knows, and — the half that matters — every name
    /// this app writes itself.
    ///
    /// ⚠️ **The second block is the test.** A conflict marker that also fires on
    /// `landing-v4-2.ondin` would put a warning on a document the user made with
    /// *Duplicate* thirty seconds earlier, which is worse than not marking
    /// anything: it teaches that the mark means nothing. So the negatives are
    /// generated the same way the app generates them — through
    /// `super::naming::slug` — rather than typed out as strings that agree with
    /// the heuristic.
    ///
    /// **Flip-checked against the plausible wrong version**: a bare
    /// `contains("conflict")` arm and a bare `contains('(')` arm — which is what
    /// "surface the roadmap's two examples" looks like written directly.
    ///
    /// ⚠️ **The predicted failure site was wrong, and where it actually failed is
    /// the more useful fact.** The prediction was `conflict-resolution-flow`, in
    /// the second block, where a real document gets marked. It fails four
    /// assertions earlier, on `landing-v4.sync-conflict-notes` — a *positive*
    /// case, asserting that Syncthing's marker without a date is nothing — which
    /// says the loose arm is caught by the tier boundary before it is ever caught
    /// by a false positive. So the assertion doing the most work here is the one
    /// that looks like a footnote on the client list, not the block of real names
    /// underneath it. Both stay: the second block is what stops the *next* loose
    /// arm, which may not touch the word "conflict" at all.
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

        // ⚠️ **Hand-made filenames, which cannot have gone through `slug`** — a
        // `.ondin` named in Explorer or arriving from somebody else is the one
        // class of file with a parenthesis in it that is not a conflict, so it is
        // the only thing the `slug` argument below does *not* protect. A bare
        // `(conflict` arm was in the code until `arch-scribe` read the brief
        // against it and asked what wrote that shape; nothing does, and this is
        // what it cost.
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

        // ⚠️ Nothing this app can write may match. These are not typed out — they
        // are put through the same `slug` the library names files with.
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
            // And its collision suffixes, which are the other half of what the
            // app puts on disk (`naming::unique_stem`).
            for n in 1..=3 {
                let with = format!("{stem}-{n}");
                assert_eq!(conflict_marker(&with), None, "{with:?}");
            }
        }
    }

    /// A `.ondin` whose filename is not valid Unicode is **not an entry** and
    /// **is** reported by `unnameable` (§15 D809).
    ///
    /// 🚨 **The first half is the pre-existing behaviour and the second is the
    /// whole fix.** `roadmap.md` carried this as *"never reproduced"* — it was not
    /// established that a filename Windows accepts can fail `to_str` — and it can:
    /// `std::fs::write` on a path holding a lone UTF-16 surrogate succeeds on
    /// NTFS, and the entry `read_dir` gives back answers `None`. The document
    /// staying out of the list is a limitation with no better answer; being
    /// *silently* left behind by `super::relocate` was the loss, and this is what
    /// `relocate` asks so that it is not.
    ///
    /// ⚠️ **Three controls, because a walk that reported everything would pass
    /// the claim above.** An ordinary document is an entry and is not reported; a
    /// non-Unicode name that is **not** a `.ondin` is neither; and the second
    /// level is walked, since `unnameable` and `scan` share one `collect` and the
    /// two-level rule has to reach both.
    ///
    /// **Two flips, both run, and the first landed somewhere other than
    /// predicted.** Dropping the `path.extension() == …` term from `collect`, so
    /// every unnameable entry is reported, was predicted to fail at a
    /// *not-a-document* assertion — there is no such assertion, and the `.txt`
    /// control is only visible as a **third entry inside the `reported == want`
    /// equality**, which is where it actually fails. ⚠️ *A control asserted as a
    /// member of a set bites at the set, and the message that names the set is the
    /// one the next reader has to understand.* Dropping the whole
    /// `unnameable.push` fails at that same assertion with an **empty** left side,
    /// which is the honest tell for the two being different faults.
    #[test]
    fn a_document_with_a_non_unicode_filename_is_not_listed_and_is_reported() {
        let root = temp_root("unnameable");
        write_doc(&root.join("landing-v4.ondin"), named("Landing v4", None));
        let project = root.join("kestrel");
        std::fs::create_dir_all(&project).unwrap();

        let odd = root.join(unnameable_doc_name());
        let deep = project.join(unnameable_doc_name());
        // Written as bytes rather than through `write_doc`: what it holds is
        // irrelevant, and nothing here ever reads it — the whole point is that the
        // name alone decides.
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
