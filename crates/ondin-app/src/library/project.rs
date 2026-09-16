//! Projects: named, coloured containers a document can point at, and the one
//! extra file the base folder carries (§9.5, §15 D363).
//!
//! **A project is a set of files that name it, not a directory that holds
//! them.** The id lives in each document's own metadata
//! (`ondin_core::meta::DocumentMeta::project`), so moving a `.ondin` between
//! folders does not move it between projects, a file dragged out of the library
//! still remembers what it belonged to, and a project can exist with no files
//! and no folder at all. A matching folder on disk is offered when a project is
//! created and is the user's choice; when there is one, renaming the project
//! renames it too, and that is the *only* thing the folder is for.
//!
//! ⚠️ **Which means the folder can disagree with the metadata, and the metadata
//! wins.** A file sitting in `kestrel/` whose block says `project: atlas` is in
//! Atlas — every reader here asks the block. The alternative, deriving the
//! project from the path, sounds tidier right up to the first file the user
//! moves in Explorer, at which point the app would silently reassign it.
//!
//! **`projects.json` sits in the base folder rather than in the cache**, for the
//! reason everything else in this module is shaped the way it is: the base
//! folder is meant to be syncable, and a library that arrives on a second
//! machine with its names and colours missing is not the same library.
//!
//! **Archiving is a flag on the project and nothing else** ([`Project::archived`]).
//! It moves no file, touches no document's metadata and does not remove the
//! project — an archived project still owns its files, still counts them, and
//! still has a page of its own. What it loses is every *list* the app offers a
//! project in ([`Projects::active`]); it is reachable from the sidebar's
//! *Archived projects* group and from search. That is why the flag lives here
//! rather than in the local cache: hiding a project is a decision about the
//! library, and a library that arrives on the second machine with its archived
//! projects back in the sidebar has not travelled.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file, at the root of the library.
pub const PROJECTS_FILE: &str = "projects.json";

/// The nine swatches the New project dialog offers, as written to disk.
///
/// Hex strings rather than a palette index, so a hand-edited file can name a
/// colour this build has never heard of and the next one still reads it. The
/// first is the accent, which is what a project created without touching the
/// swatches gets.
pub const PROJECT_COLORS: [&str; 9] = [
    "#6d8cd9", "#6dc0d9", "#7fbf9a", "#c9c06d", "#d99a6d", "#d97f7f", "#d97fb4", "#b98cd9",
    "#8f8f96",
];

/// One project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// Opaque and permanent (`super::ids::mint`). **Never the name slug**: the
    /// id is written into every member document, so deriving it from the name
    /// would orphan all of them on the first rename.
    pub id: String,
    /// What the user typed.
    pub name: String,
    /// One of [`PROJECT_COLORS`], as `#rrggbb`.
    pub color: String,
    /// The folder in the base directory this project's files are kept in, if the
    /// user asked for one.
    ///
    /// A stem, not a path: it is always directly under the base folder, so a
    /// configured base folder that moves takes its projects with it. `None` is
    /// an ordinary state, not a missing value — a project with no folder keeps
    /// its files at the root beside everything else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// When the project was made, seconds since the Unix epoch.
    pub created: u64,
    /// Whether the project is archived — out of every list, but otherwise
    /// untouched (§15 D379). See this module's own notes for what that does and
    /// does not mean.
    ///
    /// **Written only when it is true**, so a library that has never archived
    /// anything produces exactly the file it produced before this field existed
    /// — and an older build reading a newer file ignores a key it does not know,
    /// which is the additive shape [`Projects::version`] exists to avoid having
    /// to bump.
    #[serde(default, skip_serializing_if = "is_false")]
    pub archived: bool,
}

/// `skip_serializing_if` for a plain `bool`, spelled out rather than reached for
/// as `std::ops::Not::not`: serde hands it a `&bool`, and a reader should not
/// have to work out which `Not` impl that resolves to.
fn is_false(b: &bool) -> bool {
    !*b
}

/// Whether `s` is a folder name a project may carry — exactly one ordinary path
/// component, and therefore always directly under the base folder (§15 D420).
///
/// ⚠️ **[`Project::folder`]'s doc has always said *"a stem, not a path"* and
/// nothing enforced it, in a field read straight back out of a JSON file in a
/// directory the design invites the user to share.** With `"folder": "../.."`,
/// [`Project::dir`] returns a path outside the library, so every *New file* and
/// *Import* into that project is written somewhere nothing will ever scan again,
/// and *Move to project* `rename`s the existing document out of the library
/// permanently. [`rename_folder`] is worse than a write: it `rename`s
/// `root.join(old)` with `old` taken straight off the file, so
/// `"../../../Users/<you>/Documents"` moves that directory into the library
/// under a slug. `unique_stem` was applied to the destination; the **source** is
/// what came from disk.
///
/// **And the delivery route needs no local attacker.** `relocate::merge_projects`
/// adopts the *source* library's project records wholesale when the user changes
/// the base folder, so pointing *Change base folder* at a shared or received
/// directory is enough.
///
/// Checked with `Path::components` rather than by looking for `/` and `\`,
/// because that answers the Windows cases a substring search misses: a drive
/// prefix (`C:\Users\Public`), a UNC root, and `.`/`..` in any position.
pub fn is_folder_stem(s: &str) -> bool {
    let mut parts = Path::new(s).components();
    matches!(parts.next(), Some(std::path::Component::Normal(_))) && parts.next().is_none()
}

impl Project {
    /// The absolute directory this project's files live in, given the library
    /// root — the project's own folder, or the root itself.
    pub fn dir(&self, root: &Path) -> PathBuf {
        match &self.folder {
            Some(f) => root.join(f),
            None => root.to_path_buf(),
        }
    }

    /// Drop anything a *file* is not allowed to assert.
    ///
    /// Today that is [`Self::folder`] alone. Dropping it rather than refusing the
    /// project is the same trade `ondin_core::DocumentMeta::sanitized` makes for
    /// a document's id: `None` is an ordinary state — "this project keeps its
    /// files at the root beside everything else" — so the project still appears,
    /// its documents are still found by their `meta.project`, and nothing is
    /// hidden from the user. Refusing to load `projects.json` because one record
    /// is wrong would empty the sidebar.
    fn sanitized(mut self) -> Self {
        if self.folder.as_deref().is_some_and(|f| !is_folder_stem(f)) {
            self.folder = None;
        }
        self
    }
}

/// Rename a project's folder to match a new project name, returning the stem it
/// should carry afterwards.
///
/// This is the module doc's *"renaming the project renames it too"*, which was a
/// promise about a feature that did not exist until *Edit project* did. `Ok(None)`
/// for a project with no folder, which is an ordinary answer rather than a
/// failure — a project without one keeps its files at the root and there is
/// nothing on disk to follow the name.
///
/// ⚠️ **The new stem can collide, and it is resolved the way a document's is** —
/// through [`super::naming::unique_stem`], with the project's *own* folder
/// excluded from the collision test. Without that exclusion, renaming "Kestrel" to
/// "Kestrel " (or to anything that slugs the same) would walk to `kestrel-1`
/// because the directory being renamed is sitting there — the same correction
/// `store::free_stem` makes for a document.
///
/// ⚠️ **A folder the user has since deleted is adopted, not recreated.** The stem
/// is where the project's files are *filed*, so recording the new one is the
/// useful answer; the directory appears again the next time something is written
/// into it, since [`crate::atomic::write`] creates the parent on the way past.
/// Failing here instead would make a project whose folder had gone permanently
/// unrenameable.
pub fn rename_folder(
    root: &Path,
    project: &Project,
    new_name: &str,
) -> std::io::Result<Option<String>> {
    // ⚠️ **`is_folder_stem` on the *source*, which is the half `unique_stem`
    // never covered.** The destination has always been slugged; `old` comes
    // straight off `projects.json`, and this line is an `fs::rename` of whatever
    // it names. `Projects::read` already drops such a value, so this is the belt
    // to that buckle — and it reads as "the project has no folder", which is the
    // answer this function already gives for one.
    let Some(old) = project.folder.as_deref().filter(|f| is_folder_stem(f)) else {
        return Ok(None);
    };
    let stem = super::naming::unique_stem(new_name, &|s| s != old && root.join(s).exists());
    if stem != old && root.join(old).exists() {
        std::fs::rename(root.join(old), root.join(&stem))?;
    }
    Ok(Some(stem))
}

/// Every project in the library, in the order they were created.
///
/// **Insertion order rather than sorted**, because this is the order the
/// sidebar shows and the user put them in it. Nothing here needs the byte
/// stability the document format does: `projects.json` is a handful of lines
/// that changes only when a project does, so there is no diff to protect.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Projects {
    /// Bumped only if the shape changes incompatibly; a new optional field is
    /// additive here exactly as it is in the document format.
    #[serde(default = "one")]
    pub version: u32,
    #[serde(default)]
    pub projects: Vec<Project>,
}

fn one() -> u32 {
    1
}

impl Projects {
    /// Look one up by id.
    pub fn get(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    /// Every project that is not archived, in the same order.
    ///
    /// ⚠️ **This is what a *list of projects* means everywhere in the app, and
    /// [`Projects::get`] is deliberately not filtered.** The two answer different
    /// questions: a list is a set of things to choose between, and a lookup is
    /// "what is this id" — a document filed in an archived project must still
    /// resolve to its name and its colour, or archiving would silently unfile
    /// every file in it. So the sidebar, the header's filter, *Move to project*
    /// and the *Recent* page's cards read this, and everything that starts from
    /// an id reads `get`.
    pub fn active(&self) -> impl Iterator<Item = &Project> {
        self.projects.iter().filter(|p| !p.archived)
    }

    /// The project `id` names, unless it is archived — the lookup for *"may a new
    /// document be filed here"* rather than for *"what is this id"*.
    ///
    /// ⚠️ **A third question, and it needed its own name because [`Self::get`]
    /// was answering it twice** (§15 D557, `[S20.3-L1-03]`). The paragraph above
    /// draws the line between a *list* and a *lookup* and is right about both; a
    /// **destination** is neither. `dashboard::new_file_into` and
    /// `OndinApp::nav_project` both resolved the nav's id through `get`, so
    /// standing on a project's grid and archiving it left the dashed *New file in
    /// {project}* card on screen and filed a brand-new document straight into the
    /// archived project — the one thing `move_modal`'s own ⚠️ says cannot be done:
    /// *"a document cannot be filed into an archived project without unarchiving
    /// it first."* The ⋮ then offered **nothing** under *Move to project*, because
    /// the project it was in is archived.
    ///
    /// **`move_modal`'s exception does not apply here and that is the difference
    /// between the two.** That list keeps the archived project a document is
    /// *already* in, so the sheet can show where it is; there is no "already" for
    /// a document that does not exist yet.
    pub fn destination(&self, id: &str) -> Option<&Project> {
        self.get(id).filter(|p| !p.archived)
    }

    /// The archived ones, in the same order — the sidebar's own group.
    pub fn archived(&self) -> impl Iterator<Item = &Project> {
        self.projects.iter().filter(|p| p.archived)
    }

    /// Read `projects.json` from the library root.
    ///
    /// **A missing file is an empty library, not an error** — that is what a
    /// base folder the user has just pointed at looks like, and the first
    /// project created writes the file.
    ///
    /// ⚠️ **A file that exists but does not parse is also empty, and that is the
    /// uncomfortable one.** It is the right behaviour for the app — refusing to
    /// open the dashboard because one small file is malformed would be worse —
    /// but it means a sync conflict that leaves half a file behind reads as "you
    /// have no projects", and saving over it would then finish the job. So this
    /// reports which case it was, and the caller must not write over an
    /// [`ProjectsRead::Unreadable`] without saying so.
    pub fn read(root: &Path) -> ProjectsRead {
        let path = root.join(PROJECTS_FILE);
        match std::fs::read(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => ProjectsRead::Missing,
            Err(_) => ProjectsRead::Unreadable,
            Ok(bytes) => match serde_json::from_slice::<Self>(&bytes) {
                // ⚠️ **Sanitized here, at the one reader, rather than at each
                // join.** There are five joins — `Project::dir` (which
                // `store::file_document` and `store::move_to_project` both go
                // through), `rename_folder`'s source *and* destination — and a
                // check at each is four chances to miss the fifth. This function
                // is also the door `relocate::merge_projects` adopts another
                // library's records through, which is the route that needs no
                // local attacker. See [`is_folder_stem`].
                Ok(p) => ProjectsRead::Loaded(Self {
                    projects: p.projects.into_iter().map(Project::sanitized).collect(),
                    ..p
                }),
                Err(_) => ProjectsRead::Unreadable,
            },
        }
    }

    /// Write `projects.json` into the library root, creating the folder if it is
    /// not there yet.
    ///
    /// Pretty-printed for the same reason the document format is: this is a file
    /// in a folder the user was invited to point a sync client at, so it should
    /// be something they can open and read.
    /// ⚠️ **Atomic** (`crate::atomic`, §15 D378), and this is the file with the
    /// most to lose from not being: `Library::may_write_projects` exists because
    /// an *unreadable* `projects.json` must never be overwritten by the empty
    /// list that failure produced — and a save killed halfway is precisely how a
    /// readable one becomes unreadable. The latch guarded the second write and
    /// nothing guarded the first.
    pub fn write(&self, root: &Path) -> std::io::Result<()> {
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        crate::atomic::write(&root.join(PROJECTS_FILE), &bytes)
    }
}

/// What [`Projects::read`] found — three outcomes, because "no file" and
/// "unreadable file" must not lead to the same write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectsRead {
    Loaded(Projects),
    /// No file yet: a fresh or newly pointed-at base folder.
    Missing,
    /// A file that could not be read or parsed. See [`Projects::read`].
    Unreadable,
}

impl ProjectsRead {
    /// The projects, treating both failure cases as empty — for the readers that
    /// only want to draw a list.
    ///
    /// Callers that are about to **write** must match on the variant instead.
    pub fn or_empty(self) -> Projects {
        match self {
            ProjectsRead::Loaded(p) => p,
            _ => Projects::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ondin-projects-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn sample() -> Projects {
        Projects {
            version: 1,
            projects: vec![
                Project {
                    id: "0f9c2b7a4e".into(),
                    name: "Kestrel".into(),
                    color: PROJECT_COLORS[0].into(),
                    folder: Some("kestrel".into()),
                    created: 1_774_483_200,
                    archived: false,
                },
                Project {
                    id: "1a2b3c4d5e".into(),
                    name: "Field Notes".into(),
                    color: PROJECT_COLORS[7].into(),
                    folder: None,
                    created: 1_774_483_300,
                    archived: false,
                },
            ],
        }
    }

    #[test]
    fn projects_round_trip_through_the_file() {
        let root = temp_root("roundtrip");
        let p = sample();
        p.write(&root).unwrap();
        assert_eq!(Projects::read(&root), ProjectsRead::Loaded(p));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A project with no folder writes no key, so the common case reads as
    /// plainly as it behaves.
    #[test]
    fn a_project_without_a_folder_writes_no_folder_key() {
        let only_rootless = Projects {
            version: 1,
            projects: vec![sample().projects.remove(1)],
        };
        let text = serde_json::to_string(&only_rootless).unwrap();
        assert!(!text.contains("folder"), "{text}");
    }

    /// ⚠️ The distinction the three-variant return exists for. A folder with no
    /// file is `Missing` and safe to write into; a folder with a broken one is
    /// `Unreadable`, and a caller that treated the two alike would overwrite a
    /// half-synced file with an empty list.
    ///
    /// Flip-check, run: collapsing `Unreadable` into `Missing` fails only the
    /// second assertion here — the first stays green, which is exactly why the
    /// two cases need separate names rather than one "is it empty".
    #[test]
    fn a_missing_file_and_a_broken_one_are_different_answers() {
        let root = temp_root("broken");
        assert_eq!(Projects::read(&root), ProjectsRead::Missing);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(PROJECTS_FILE), b"{ \"projects\": [ ").unwrap();
        assert_eq!(Projects::read(&root), ProjectsRead::Unreadable);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A project's directory is the root when it has no folder — the case that
    /// makes "may or may not create a folder" invisible to every writer.
    #[test]
    fn a_projects_directory_is_its_folder_or_the_root() {
        let root = Path::new("D:/lib");
        let p = sample();
        assert_eq!(p.projects[0].dir(root), PathBuf::from("D:/lib/kestrel"));
        assert_eq!(p.projects[1].dir(root), PathBuf::from("D:/lib"));
    }

    /// An older file with no `version` key reads as version 1 rather than 0,
    /// which is the difference between "written before the key existed" and
    /// "written by something that meant zero".
    #[test]
    fn a_file_without_a_version_key_reads_as_one() {
        let p: Projects = serde_json::from_str(r#"{"projects":[]}"#).unwrap();
        assert_eq!(p.version, 1);
    }

    /// The archived flag survives the file, and a library that has never archived
    /// anything writes exactly the bytes it wrote before the field existed — which
    /// is what makes this additive rather than a `version` bump.
    #[test]
    fn archiving_round_trips_and_writes_no_key_until_it_is_true() {
        let mut p = sample();
        let text = serde_json::to_string(&p).unwrap();
        assert!(!text.contains("archived"), "{text}");
        p.projects[1].archived = true;
        let text = serde_json::to_string(&p).unwrap();
        assert!(text.contains(r#""archived":true"#), "{text}");
        assert_eq!(serde_json::from_str::<Projects>(&text).unwrap(), p);
    }

    /// A file written before the field existed reads as a library with nothing
    /// archived, rather than failing to parse — and `Projects::read` turns a parse
    /// failure into an *empty library*, so getting this wrong would not error, it
    /// would silently lose every project.
    #[test]
    fn a_file_without_an_archived_key_reads_as_active() {
        let p: Projects = serde_json::from_str(
            r##"{"projects":[{"id":"a","name":"Kestrel","color":"#6d8cd9","created":1}]}"##,
        )
        .unwrap();
        assert!(!p.projects[0].archived);
        assert_eq!(p.active().count(), 1);
    }

    /// ⚠️ The split `Projects::active`'s doc is about: archiving takes a project
    /// out of every *list* and leaves the *lookup* alone. A document filed in an
    /// archived project must still resolve to its name and colour, or archiving
    /// would quietly unfile everything in it.
    #[test]
    fn archiving_takes_a_project_out_of_the_list_and_leaves_the_lookup_alone() {
        let mut p = sample();
        p.projects[0].archived = true;
        assert_eq!(
            p.active().map(|x| x.name.as_str()).collect::<Vec<_>>(),
            ["Field Notes"]
        );
        assert_eq!(
            p.archived().map(|x| x.name.as_str()).collect::<Vec<_>>(),
            ["Kestrel"]
        );
        assert_eq!(
            p.get("0f9c2b7a4e").map(|x| x.name.as_str()),
            Some("Kestrel")
        );
    }

    /// Renaming a project takes its folder — and its files inside it — with it,
    /// and a stem already in use is stepped around exactly as a document's is.
    #[test]
    fn renaming_a_project_moves_its_folder_and_steps_around_a_collision() {
        let root = temp_root("rename-folder");
        std::fs::create_dir_all(root.join("kestrel")).unwrap();
        std::fs::write(root.join("kestrel").join("a.ondin"), b"x").unwrap();
        std::fs::create_dir_all(root.join("atlas")).unwrap();
        let mut p = sample().projects.remove(0);

        assert_eq!(
            rename_folder(&root, &p, "Harbor App").unwrap().as_deref(),
            Some("harbor-app")
        );
        assert!(root.join("harbor-app").join("a.ondin").is_file());
        assert!(!root.join("kestrel").exists());

        p.folder = Some("harbor-app".into());
        assert_eq!(
            rename_folder(&root, &p, "Atlas").unwrap().as_deref(),
            Some("atlas-1"),
            "an occupied stem steps aside rather than merging two projects' files"
        );
        assert!(root.join("atlas-1").join("a.ondin").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ⚠️ The project's *own* folder does not count as a collision. Without that
    /// exclusion a rename that does not change the slug — "Kestrel" to "kestrel ",
    /// or to itself — would walk to `kestrel-1` because the directory being renamed
    /// is sitting there.
    ///
    /// Flip-check, run: dropping `s != old` from the predicate fails here at
    /// `Some("kestrel-1")` against `Some("kestrel")`, and dropping the
    /// `root.join(old).exists()` guard beside it panics
    /// `a_missing_folder_is_adopted_and_no_folder_is_not_an_error` on the `unwrap`.
    /// ⚠️ **The collision test above stays green under both** — its slug always
    /// changes and its folder always exists, so it never reaches either arm. The
    /// self-exclusion is covered by this test alone.
    #[test]
    fn a_rename_that_does_not_change_the_slug_moves_nothing() {
        let root = temp_root("rename-same");
        std::fs::create_dir_all(root.join("kestrel")).unwrap();
        let p = sample().projects.remove(0);
        assert_eq!(
            rename_folder(&root, &p, "  Kestrel  ").unwrap().as_deref(),
            Some("kestrel")
        );
        assert!(root.join("kestrel").is_dir());
        assert!(!root.join("kestrel-1").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The two cases where there is nothing on disk to move: a project that never
    /// had a folder answers `None`, and one whose folder the user has since deleted
    /// adopts the new stem rather than failing. Neither creates a directory — the
    /// next write into the project does that.
    #[test]
    fn a_missing_folder_is_adopted_and_no_folder_is_not_an_error() {
        let root = temp_root("rename-missing");
        std::fs::create_dir_all(&root).unwrap();
        let unfoldered = sample().projects.remove(1);
        assert_eq!(rename_folder(&root, &unfoldered, "Anything").unwrap(), None);

        let mut gone = sample().projects.remove(0);
        gone.folder = Some("deleted-in-explorer".into());
        assert_eq!(
            rename_folder(&root, &gone, "Field Notes")
                .unwrap()
                .as_deref(),
            Some("field-notes")
        );
        assert!(!root.join("field-notes").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A `folder` that is not a single path component is dropped on the way in,
    /// so nothing downstream can join it.
    ///
    /// ⚠️ **The field's doc has said *"a stem, not a path"* since it was
    /// written, and until this test nothing said so to the file.** `projects.json`
    /// sits in a folder the design invites the user to point a sync client at, and
    /// `relocate::merge_projects` adopts another library's records wholesale when
    /// the base folder changes — so the delivery route is *changing your base
    /// folder to a shared directory*, not an attacker with local write access.
    ///
    /// **`dir` is the assertion rather than the field**, because the field is the
    /// mechanism and the escape is the loss: `store::file_document` does
    /// `create_dir_all(&project.dir(root))` and writes there, and
    /// `store::move_to_project` `rename`s an existing document into it.
    ///
    /// ⚠️ **The Windows cases are the ones a substring search for `/` and `\`
    /// misses**, which is why `is_folder_stem` asks `Path::components`: a drive
    /// prefix discards the base entirely under `join`, exactly as an absolute
    /// `meta.id` does.
    ///
    /// Flip-check, run: removing the `.map(Project::sanitized)` from
    /// `Projects::read` fails on the first `dir` assertion, printing a path with
    /// `..` still in it.
    #[test]
    fn a_project_folder_that_escapes_the_root_is_refused() {
        let root = temp_root("escape");
        std::fs::create_dir_all(&root).unwrap();

        for bad in [
            "../..",
            "..",
            ".",
            "sub/dir",
            r"sub\dir",
            r"C:\Users\Public",
            r"\\server\share",
            "/etc",
            "",
        ] {
            let mut projects = sample();
            projects.projects[0].folder = Some(bad.into());
            projects.write(&root).unwrap();

            let ProjectsRead::Loaded(read) = Projects::read(&root) else {
                panic!("the file still parses — a bad record must not empty the sidebar");
            };
            assert_eq!(read.projects.len(), 2, "and the project is still listed");
            assert_eq!(
                read.projects[0].dir(&root),
                root,
                "folder {bad:?} must not move the project's files anywhere"
            );
            // And the rename door, whose *source* never went through a slug.
            assert_eq!(
                rename_folder(&root, &read.projects[0], "Kestrel").unwrap(),
                None
            );
        }

        // The control: an ordinary stem still works, or the check is a feature
        // removal rather than a guard.
        let projects = sample();
        projects.write(&root).unwrap();
        let ProjectsRead::Loaded(read) = Projects::read(&root) else {
            panic!("a well-formed file loads")
        };
        assert_eq!(read.projects[0].dir(&root), root.join("kestrel"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
