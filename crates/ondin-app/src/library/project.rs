//! Projects: named, coloured containers a document can point at, and the one
//! extra file the base folder carries (§9.5).
//!
//! A project is a set of files that name it, not a directory that holds them. The
//! id lives in each document's own metadata
//! (`ondin_core::meta::DocumentMeta::project`), so moving a `.ondin` between
//! folders does not move it between projects, and a project can exist with no
//! files and no folder. A matching folder is the user's choice at creation; when
//! there is one, renaming the project renames it, and that is all it is for.
//!
//! The folder can disagree with the metadata, and the metadata wins: every reader
//! asks the block, since deriving the project from the path would silently
//! reassign any file the user moves in Explorer.
//!
//! `projects.json` sits in the base folder rather than the cache because the base
//! folder is meant to be syncable. The same holds for archiving: it is a flag on
//! the project ([`Project::archived`]) and nothing else, moving no file and
//! touching no document. An archived project still owns and counts its files and
//! keeps its page; it leaves every list ([`Projects::active`]) and is reachable
//! from the sidebar's *Archived projects* group and from search.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file, at the root of the library.
pub const PROJECTS_FILE: &str = "projects.json";

/// The nine swatches the New project dialog offers, as written to disk.
///
/// Hex strings rather than a palette index, so a hand-edited file can name a
/// colour this build has never heard of. The first is the accent, the default.
pub const PROJECT_COLORS: [&str; 9] = [
    "#6d8cd9", "#6dc0d9", "#7fbf9a", "#c9c06d", "#d99a6d", "#d97f7f", "#d97fb4", "#b98cd9",
    "#8f8f96",
];

/// One project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// Opaque and permanent (`super::ids::mint`), never the name slug: the id is
    /// written into every member document, so a rename would orphan them.
    pub id: String,
    /// What the user typed.
    pub name: String,
    /// One of [`PROJECT_COLORS`], as `#rrggbb`.
    pub color: String,
    /// The folder in the base directory this project's files are kept in, if the
    /// user asked for one.
    ///
    /// A stem, not a path ([`is_folder_stem`]): always directly under the base
    /// folder, so a moved base folder takes its projects with it. `None` is an
    /// ordinary state: the files stay at the root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// When the project was made, seconds since the Unix epoch.
    pub created: u64,
    /// Whether the project is archived — out of every list, but otherwise
    /// untouched (see the module doc).
    ///
    /// Written only when true, so the field is additive and [`Projects::version`]
    /// need not bump.
    #[serde(default, skip_serializing_if = "is_false")]
    pub archived: bool,
}

/// `skip_serializing_if` for a plain `bool`; serde hands it a `&bool`.
fn is_false(b: &bool) -> bool {
    !*b
}

/// Whether `s` is a folder name a project may carry — exactly one ordinary path
/// component, and therefore always directly under the base folder.
///
/// [`Project::folder`] is read straight from a JSON file in a directory the user
/// may share, and `relocate::merge_projects` adopts another library's records
/// wholesale. With `"folder": "../.."`, [`Project::dir`] would leave the library
/// and [`rename_folder`] would `rename` an arbitrary directory into it.
///
/// Checked with `Path::components`, which answers the Windows cases a substring
/// search misses: a drive prefix, a UNC root, and `.`/`..` in any position. A
/// backslash is refused outright on every platform, because elsewhere `components`
/// treats `\` as an ordinary character, and the same record read on Windows would
/// be a nested path or a drive.
pub fn is_folder_stem(s: &str) -> bool {
    let mut parts = Path::new(s).components();
    !s.contains('\\')
        && matches!(parts.next(), Some(std::path::Component::Normal(_)))
        && parts.next().is_none()
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

    /// Drop anything a file is not allowed to assert — today [`Self::folder`]
    /// alone. As with `ondin_core::DocumentMeta::sanitized`, dropping beats
    /// refusing: `None` is an ordinary state, the project still appears and its
    /// documents are still found by `meta.project`, whereas refusing to load
    /// `projects.json` over one bad record would empty the sidebar.
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
/// `Ok(None)` for a project with no folder, which is an ordinary answer: its
/// files are at the root and nothing on disk follows the name.
///
/// The new stem is resolved as a document's is, through
/// [`super::naming::unique_stem`], with the project's own folder excluded from the
/// collision test (as `store::free_stem` does), so a rename that keeps the slug
/// does not walk to `kestrel-1`.
///
/// A folder the user has since deleted is adopted, not recreated: the directory
/// reappears when [`crate::atomic::write`] next creates the parent, and failing
/// would leave the project permanently unrenameable.
pub fn rename_folder(
    root: &Path,
    project: &Project,
    new_name: &str,
) -> std::io::Result<Option<String>> {
    // `is_folder_stem` on the source, the half `unique_stem` never covered: `old`
    // comes straight off `projects.json` and feeds an `fs::rename`. It is the
    // belt to `Projects::read`'s buckle, and reads as "the project has no folder".
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
/// Insertion order rather than sorted: it is the order the sidebar shows and the
/// user put them in.
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
    /// What a list of projects means everywhere in the app; [`Projects::get`] is
    /// deliberately not filtered. A list is a set to choose between, a lookup is
    /// "what is this id": a document filed in an archived project must still
    /// resolve to its name and colour. The sidebar, the header's filter, *Move to
    /// project* and the *Recent* cards read this; anything starting from an id
    /// reads `get`.
    pub fn active(&self) -> impl Iterator<Item = &Project> {
        self.projects.iter().filter(|p| !p.archived)
    }

    /// The project `id` names, unless it is archived — the lookup for "may a new
    /// document be filed here" rather than "what is this id". It is neither a list
    /// nor a plain lookup: resolving a destination through `get` would file a new
    /// document into an archived project. Unlike the move sheet, which keeps the
    /// project a document is already in, a new document has no "already".
    pub fn destination(&self, id: &str) -> Option<&Project> {
        self.get(id).filter(|p| !p.archived)
    }

    /// The archived ones, in the same order — the sidebar's own group.
    pub fn archived(&self) -> impl Iterator<Item = &Project> {
        self.projects.iter().filter(|p| p.archived)
    }

    /// Read `projects.json` from the library root.
    ///
    /// A missing file is an empty library, not an error: it is what a freshly
    /// pointed-at base folder looks like.
    ///
    /// A file that exists but does not parse is also empty (refusing to open the
    /// dashboard would be worse), but a sync conflict leaving half a file then
    /// reads as "no projects" and saving would finish the job. So this reports
    /// which case it was, and the caller must not write over a
    /// [`ProjectsRead::Unreadable`] without saying so.
    pub fn read(root: &Path) -> ProjectsRead {
        let path = root.join(PROJECTS_FILE);
        match std::fs::read(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => ProjectsRead::Missing,
            Err(_) => ProjectsRead::Unreadable,
            Ok(bytes) => match serde_json::from_slice::<Self>(&bytes) {
                // Sanitized at the one reader rather than at each join
                // (`Project::dir`, `rename_folder`'s source and destination), and
                // this is the door `relocate::merge_projects` adopts another
                // library's records through. See [`is_folder_stem`].
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
    /// Pretty-printed, as the document format is: the file sits in a folder the
    /// user may sync, so it should be readable.
    ///
    /// Atomic (`crate::atomic`): a save killed halfway is how a readable
    /// `projects.json` becomes unreadable, and `Library::may_write_projects`
    /// guards only the write after that.
    pub fn write(&self, root: &Path) -> std::io::Result<()> {
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        crate::atomic::write(&root.join(PROJECTS_FILE), &bytes)
    }
}

/// What [`Projects::read`] found: "no file" and "unreadable file" must not lead
/// to the same write.
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
    /// only want to draw a list. Callers about to write must match on the variant.
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

    /// A project with no folder writes no key.
    #[test]
    fn a_project_without_a_folder_writes_no_folder_key() {
        let only_rootless = Projects {
            version: 1,
            projects: vec![sample().projects.remove(1)],
        };
        let text = serde_json::to_string(&only_rootless).unwrap();
        assert!(!text.contains("folder"), "{text}");
    }

    /// A folder with no file is `Missing` and safe to write into; a broken file is
    /// `Unreadable`, and treating the two alike would overwrite a half-synced file
    /// with an empty list.
    ///
    /// flip: collapsing `Unreadable` into `Missing` → fails only the second
    /// assertion.
    #[test]
    fn a_missing_file_and_a_broken_one_are_different_answers() {
        let root = temp_root("broken");
        assert_eq!(Projects::read(&root), ProjectsRead::Missing);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(PROJECTS_FILE), b"{ \"projects\": [ ").unwrap();
        assert_eq!(Projects::read(&root), ProjectsRead::Unreadable);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A project's directory is the root when it has no folder.
    #[test]
    fn a_projects_directory_is_its_folder_or_the_root() {
        let root = Path::new("D:/lib");
        let p = sample();
        assert_eq!(p.projects[0].dir(root), PathBuf::from("D:/lib/kestrel"));
        assert_eq!(p.projects[1].dir(root), PathBuf::from("D:/lib"));
    }

    /// An older file with no `version` key reads as version 1, not 0.
    #[test]
    fn a_file_without_a_version_key_reads_as_one() {
        let p: Projects = serde_json::from_str(r#"{"projects":[]}"#).unwrap();
        assert_eq!(p.version, 1);
    }

    /// The archived flag survives the file, and a library that has never archived
    /// anything writes no key, which keeps the field additive.
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

    /// A file written before the field existed reads as nothing archived; a parse
    /// failure would silently empty the library.
    #[test]
    fn a_file_without_an_archived_key_reads_as_active() {
        let p: Projects = serde_json::from_str(
            r##"{"projects":[{"id":"a","name":"Kestrel","color":"#6d8cd9","created":1}]}"##,
        )
        .unwrap();
        assert!(!p.projects[0].archived);
        assert_eq!(p.active().count(), 1);
    }

    /// Archiving takes a project out of every list and leaves the lookup alone, so
    /// a document filed in it still resolves to its name and colour.
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

    /// The project's own folder does not count as a collision, so a rename that
    /// keeps the slug does not walk to `kestrel-1`. The collision test above
    /// reaches neither arm, so this test alone covers the self-exclusion.
    ///
    /// flip: dropping `s != old` from the predicate → fails here at
    /// `Some("kestrel-1")` against `Some("kestrel")`. Dropping the
    /// `root.join(old).exists()` guard → panics
    /// `a_missing_folder_is_adopted_and_no_folder_is_not_an_error` on the `unwrap`.
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

    /// With nothing on disk to move, a project without a folder answers `None` and
    /// one whose folder was deleted adopts the new stem; neither creates a directory.
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
    /// `dir` is the assertion rather than the field, because the escape is the
    /// loss: `store::file_document` writes into `project.dir(root)` and
    /// `store::move_to_project` renames a document into it. The Windows cases (a
    /// drive prefix discards the base under `join`) are why `is_folder_stem` asks
    /// `Path::components`.
    ///
    /// flip: removing `.map(Project::sanitized)` from `Projects::read` → fails at
    /// the first `dir` assertion.
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
            // And the rename door, whose source never went through a slug.
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
