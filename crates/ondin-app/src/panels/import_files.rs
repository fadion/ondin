//! The library's *Import files* card: one field for a file, the options its type
//! offers once one is chosen, then an overview of what came in.
//!
//! The card is the one door for documents. *Import files* opens it empty, and a
//! drop on the library window opens it filled; a drop on the open card refills
//! the field. Reading a file for its options and filing the copy both run on a
//! worker, so a large document never stalls a frame, and the copy can be
//! cancelled while it is being made.

use crate::app::OndinApp;
use crate::job::Job;
use crate::library::{project, store};
use crate::settings;
use crate::theme::{self, color, icon};
use crate::ui::{self, FieldButton};
use eframe::egui;
use ondin_core::foreign::Part;
use ondin_core::take::{Census, Keep};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The card's width, border and padding included: room for a page name and its
/// count on one line.
const CARD_W: f32 = 480.0;
/// The file field and the buttons beside it.
const FIELD_H: f32 = 32.0;
/// A tick box's side.
const TICK: f32 = 14.0;
/// The tallest the page list grows before it scrolls.
const PAGES_MAX_H: f32 = 150.0;
/// Between two groups of options.
const GROUP_GAP: f32 = 14.0;

/// What a chosen file is, by its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Ondin,
    /// Sketch's own, which Lunacy saves too: read, never written back.
    Sketch,
    /// Penpot 2's export: read, never written back.
    Penpot,
    /// A PDF, an SVG or a picture: placed on a canvas, never imported.
    Placed,
    /// Anything else.
    Other,
}

impl Kind {
    pub(crate) fn of(path: &Path) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match ext.as_str() {
            crate::library::scan::DOCUMENT_EXT => Self::Ondin,
            "sketch" => Self::Sketch,
            "penpot" => Self::Penpot,
            "pdf" | "svg" | "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff"
            | "avif" => Self::Placed,
            _ => Self::Other,
        }
    }

    /// Whether the card imports this kind.
    pub(crate) fn imports(self) -> bool {
        matches!(self, Self::Ondin | Self::Sketch | Self::Penpot)
    }

    /// What the message line calls a file of this kind.
    fn noun(self) -> &'static str {
        match self {
            Self::Ondin => "Ondin document",
            Self::Sketch => "Sketch document",
            Self::Penpot => "Penpot file",
            Self::Placed | Self::Other => "File",
        }
    }

    /// The options' and the overview's words, in the tool's own terms.
    fn words(self) -> Words {
        match self {
            Self::Sketch => Words {
                frames: ("artboard", "artboards"),
                components: "Symbols as components",
                styles: "Shared styles",
                styles_off: "Off: each use keeps the values it shows.",
                variables: "Colour variables",
                variables_off: "Off: each use keeps its colour as a raw value.",
            },
            // Penpot's own names, with what each becomes.
            Self::Penpot => Words {
                frames: ("board", "boards"),
                components: "Components",
                styles: "Typographies",
                styles_off: "As text styles. Off: each text keeps its own type.",
                variables: "Colours",
                variables_off: "As colour variables. Off: each use keeps its colour.",
            },
            _ => Words {
                frames: ("frame", "frames"),
                components: "Components",
                styles: "Styles",
                styles_off: "Off: each use keeps the values it shows.",
                variables: "Variables",
                variables_off: "Off: bound values become raw values in the current mode.",
            },
        }
    }

    /// The badge at the head of the filled field.
    fn badge(self, path: &Path) -> (String, egui::Color32) {
        match self {
            Self::Ondin => ("ONDIN".into(), egui::Color32::from_rgb(0x8b, 0xa5, 0xe0)),
            Self::Sketch => ("SKETCH".into(), egui::Color32::from_rgb(0xe6, 0xb8, 0x66)),
            Self::Penpot => ("PENPOT".into(), egui::Color32::from_rgb(0x8f, 0xd1, 0xa8)),
            Self::Placed | Self::Other => (
                path.extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_ascii_uppercase)
                    .unwrap_or_else(|| "FILE".into()),
                theme::text::DIM,
            ),
        }
    }
}

/// How a kind of file names what it holds.
#[derive(Clone, Copy)]
struct Words {
    frames: (&'static str, &'static str),
    components: &'static str,
    styles: &'static str,
    styles_off: &'static str,
    variables: &'static str,
    variables_off: &'static str,
}

/// A document read for its options: the document itself, kept for the copy, and
/// what it holds.
pub(crate) struct Source {
    doc: Arc<ondin_core::Document>,
    census: Census,
    kind: Kind,
    /// The name it will be filed under unless the field is edited.
    name: String,
    /// The file's size on disk, for the message line.
    bytes: u64,
    /// What another tool's file held that did not come in as it was, each row
    /// with the part it is about, so a part left out takes its rows along.
    report: Vec<(Part, ReportRow)>,
    /// The font families its text asks for, with how many runs ask, for the
    /// landing to say which are not installed.
    families: Vec<(String, usize)>,
}

/// What the field holds.
pub(crate) enum Field {
    Empty,
    /// Being read for its options.
    Reading {
        path: PathBuf,
        job: Job<Result<Source, String>>,
    },
    /// One document that read.
    One {
        path: PathBuf,
        source: Box<Source>,
    },
    /// Several Ondin documents, dropped or picked together.
    Many(Vec<PathBuf>),
    /// A file the card will not take, and why.
    Refused {
        path: PathBuf,
        why: String,
    },
}

/// The choices under the field. Per page, `pages` lines up with the source's
/// census.
pub(crate) struct Options {
    pub(crate) name: String,
    /// The project to file into, by id; `None` is no project.
    pub(crate) project: Option<String>,
    pub(crate) pages: Vec<bool>,
    pub(crate) components: bool,
    pub(crate) styles: bool,
    pub(crate) variables: bool,
    pub(crate) leave_pictures: bool,
}

/// Where the card is: the form, the copy being made, or the overview.
pub(crate) enum Step {
    Form,
    Loading {
        job: Job<Landed>,
        cancel: Arc<AtomicBool>,
        what: String,
    },
    Done(Overview),
}

/// The *Import files* card's whole state, while it is open.
pub(crate) struct ImportCard {
    pub(crate) field: Field,
    /// Typed or pasted into the empty field, until it names a file.
    pub(crate) typed: String,
    pub(crate) options: Options,
    pub(crate) step: Step,
    /// Said under the field about the drop or pick that filled it — the files a
    /// drop of several left out.
    pub(crate) note: Option<String>,
}

/// What a worker filing copies hands back.
pub(crate) struct Landed {
    /// Every file written, in order.
    written: Vec<PathBuf>,
    /// The copies' contents, summed.
    counts: Counts,
    /// What did not come in, a row each.
    report: Vec<ReportRow>,
}

/// What the overview counts.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) pages: usize,
    pub(crate) frames: usize,
    pub(crate) components: usize,
    pub(crate) styles: usize,
    pub(crate) variables: usize,
    pub(crate) pictures: usize,
}

impl Counts {
    fn of(c: &Census) -> Self {
        Self {
            pages: c.pages.len(),
            frames: c.frames,
            components: c.components,
            styles: c.styles,
            variables: c.variables,
            pictures: c.pictures,
        }
    }

    fn add(&mut self, o: Self) {
        self.pages += o.pages;
        self.frames += o.frames;
        self.components += o.components;
        self.styles += o.styles;
        self.variables += o.variables;
        self.pictures += o.pictures;
    }

    /// Each count with its noun, frames called what `frames` calls them.
    fn cells(
        self,
        frames: (&'static str, &'static str),
    ) -> [(usize, &'static str, &'static str); 6] {
        [
            (self.pages, "page", "pages"),
            (self.frames, frames.0, frames.1),
            (self.components, "component", "components"),
            (self.styles, "style", "styles"),
            (self.variables, "variable", "variables"),
            (self.pictures, "picture", "pictures"),
        ]
    }
}

/// One line of the overview's report: a thing that did not come in as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReportRow {
    pub(crate) what: String,
    pub(crate) count: String,
    pub(crate) how: String,
}

/// The card's second step: what came in.
pub(crate) struct Overview {
    pub(crate) title: String,
    pub(crate) sub: String,
    pub(crate) counts: Counts,
    /// What the file's tool calls a frame.
    frames: (&'static str, &'static str),
    pub(crate) report: Vec<ReportRow>,
    /// How many documents were written.
    pub(crate) written: usize,
    /// The document *Open document* opens; `None` after several.
    pub(crate) open: Option<PathBuf>,
}

impl Overview {
    /// The report as plain text, for *Copy report*.
    fn text(&self) -> String {
        let mut out = format!("{}\n{}\n\n", self.title, self.sub);
        for (n, one, many) in self.counts.cells(self.frames) {
            out.push_str(&format!("{n} {}\n", if n == 1 { one } else { many }));
        }
        if self.report.is_empty() {
            out.push_str("\nNothing to report\n");
        } else {
            out.push('\n');
            for r in &self.report {
                out.push_str(&format!("{} · {} · {}\n", r.what, r.count, r.how));
            }
        }
        out
    }
}

impl ImportCard {
    /// An empty card filing into `project`.
    pub(crate) fn new(project: Option<String>) -> Self {
        Self {
            field: Field::Empty,
            typed: String::new(),
            options: Options {
                name: String::new(),
                project,
                pages: Vec::new(),
                components: true,
                styles: true,
                variables: true,
                leave_pictures: false,
            },
            step: Step::Form,
            note: None,
        }
    }

    /// Whether *Import* would do something.
    pub(crate) fn ready(&self) -> bool {
        match &self.field {
            Field::One { .. } => {
                !self.options.name.trim().is_empty() && self.options.pages.iter().any(|&p| p)
            }
            Field::Many(_) => true,
            _ => false,
        }
    }
}

/// What the footer says beside its buttons.
fn foot_text(card: &ImportCard, projects: &project::Projects) -> String {
    let into = card
        .options
        .project
        .as_deref()
        .and_then(|id| projects.get(id))
        .map(|p| p.name.clone());
    match &card.field {
        Field::Empty => "Choose a file".into(),
        Field::Reading { .. } => "Reading the file…".into(),
        Field::Refused { .. } => "Not a file Ondin imports".into(),
        Field::One { .. } => match into {
            Some(p) => format!("Into {p} as a new document"),
            None => "Into the library as a new document".into(),
        },
        Field::Many(paths) => match into {
            Some(p) => format!("Into {p} as {} new documents", paths.len()),
            None => format!("Into the library as {} new documents", paths.len()),
        },
    }
}

/// Read one file for its options, on the worker.
fn read_source(path: &Path) -> Result<Source, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Could not be read: {e}"))?;
    let kind = Kind::of(path);
    let (doc, report, families, own_name) = match kind {
        Kind::Sketch | Kind::Penpot => read_foreign(kind, &bytes)?,
        _ => (
            ondin_core::io::load(&bytes).map_err(|e| format!("Not a readable document: {e}"))?,
            Vec::new(),
            Vec::new(),
            None,
        ),
    };
    let census = ondin_core::take::census(&doc);
    Ok(Source {
        name: own_name.unwrap_or_else(|| document_name(&doc, path)),
        census,
        kind,
        bytes: bytes.len() as u64,
        report,
        families,
        doc: Arc::new(doc),
    })
}

/// The most another tool's zip may inflate to, all its entries together, so a
/// crafted file cannot exhaust memory.
const ZIP_LIMIT: u64 = 2 << 30;

/// Another tool's zip, for `ondin_core::foreign::Archive`, and how much of
/// [`ZIP_LIMIT`] its entries have used.
struct Zip<'a> {
    zip: zip::ZipArchive<std::io::Cursor<&'a [u8]>>,
    read: u64,
}

impl ondin_core::foreign::Archive for Zip<'_> {
    fn names(&mut self) -> Vec<String> {
        self.zip.file_names().map(str::to_owned).collect()
    }

    /// `None` for an entry that would pass the limit: whole or not at all, since a
    /// cut entry would read as a broken one.
    fn entry(&mut self, name: &str) -> Option<Vec<u8>> {
        let entry = self.zip.by_name(name).ok()?;
        let room = ZIP_LIMIT.saturating_sub(self.read);
        if entry.size() > room {
            return None;
        }
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut std::io::Read::take(entry, room + 1), &mut out).ok()?;
        // The declared size can lie; the bytes read cannot.
        if out.len() as u64 > room {
            return None;
        }
        self.read += out.len() as u64;
        Some(out)
    }
}

/// Pictures inside another tool's file, hashed and sized as a placed one is.
struct Pictures;

impl ondin_core::svg_in::ImageSink for Pictures {
    fn place(
        &mut self,
        bytes: Vec<u8>,
        format: ondin_core::ImageFormat,
    ) -> Option<(ondin_core::ImageId, ondin_core::ImageEntry)> {
        let (picture, _) =
            crate::app::read_picture(bytes, Some(format.extension().to_owned()), None).ok()?;
        Some((picture.id, picture.entry))
    }
}

/// Another tool's file read into a document, with what did not map, the font
/// families its text asks for, and the name the file gives itself.
type Read = (
    ondin_core::Document,
    Vec<(Part, ReportRow)>,
    Vec<(String, usize)>,
    Option<String>,
);

fn read_foreign(kind: Kind, bytes: &[u8]) -> Result<Read, String> {
    if kind == Kind::Penpot && ondin_core::penpot_in::is_old_format(bytes) {
        return Err(ondin_core::penpot_in::PenpotError::OldFormat.to_string());
    }
    let zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|_| format!("Not a {}: it does not open as one", kind.noun()))?;
    // A fresh stream, as a new document's: nothing else holds ids under it.
    let actor = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64 | 1);
    let mut ids = ondin_core::IdSource::new(actor);
    let mut archive = Zip { zip, read: 0 };
    let unreadable = |e: &dyn std::fmt::Display| format!("Could not be read: {e}");
    let (doc, report, families, name) = match kind {
        Kind::Penpot => {
            let r = ondin_core::penpot_in::read(&mut archive, &mut ids, &mut Pictures)
                .map_err(|e| unreadable(&e))?;
            (r.doc, r.report, r.families, Some(r.name))
        }
        _ => {
            let r = ondin_core::sketch_in::read(&mut archive, &mut ids, &mut Pictures)
                .map_err(|e| unreadable(&e))?;
            (r.doc, r.report, r.families, None)
        }
    };
    let report = report
        .into_iter()
        .map(|u| {
            let row = ReportRow {
                what: u.what.to_owned(),
                count: u.count,
                how: u.how.to_owned(),
            };
            (u.part, row)
        })
        .collect();
    let name = name.filter(|n| !n.trim().is_empty());
    Ok((doc, report, families.into_iter().collect(), name))
}

/// The name a document files under: its own, else its filename de-slugged as the
/// list does.
fn document_name(doc: &ondin_core::Document, path: &Path) -> String {
    doc.meta()
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| {
            crate::library::scan::name_from_stem(
                path.file_stem().and_then(|s| s.to_str()).unwrap_or(""),
            )
        })
}

/// File `doc` as a new library document: a copy with an identity of its own.
fn file_copy(
    root: &Path,
    project: Option<&project::Project>,
    name: &str,
    mut doc: ondin_core::Document,
) -> std::io::Result<PathBuf> {
    let mut meta = doc.meta().clone();
    meta.id = None;
    doc.set_meta(meta);
    store::file_document(root, project, name, &mut doc)
}

/// One document, trimmed to `keep` and filed, `report` saying what its reading
/// left out. Stops before writing once `cancel` is set.
fn import_one(
    root: &Path,
    project: Option<&project::Project>,
    name: &str,
    doc: &ondin_core::Document,
    keep: &Keep,
    report: Vec<ReportRow>,
    cancel: &AtomicBool,
) -> Landed {
    let mut doc = doc.clone();
    let trimmed = ondin_core::take::take(&mut doc, keep);
    let counts = Counts::of(&ondin_core::take::census(&doc));
    let mut landed = Landed {
        written: Vec::new(),
        counts,
        report,
    };
    if let Err(e) = trimmed {
        landed.counts = Counts::default();
        landed.report.push(ReportRow {
            what: name.to_owned(),
            count: String::new(),
            how: format!("Could not be trimmed as chosen: {e}"),
        });
        return landed;
    }
    if cancel.load(Ordering::Relaxed) {
        return landed;
    }
    match file_copy(root, project, name, doc) {
        Ok(path) => landed.written.push(path),
        Err(e) => landed.report.push(ReportRow {
            what: name.to_owned(),
            count: String::new(),
            how: format!("Could not be written: {e}"),
        }),
    }
    landed
}

/// Several documents, each filed whole (less its pictures, when asked). A file
/// that does not read is reported and the rest go on.
fn import_many(
    root: &Path,
    project: Option<&project::Project>,
    paths: &[PathBuf],
    leave_pictures: bool,
    cancel: &AtomicBool,
) -> Landed {
    let mut landed = Landed {
        written: Vec::new(),
        counts: Counts::default(),
        report: Vec::new(),
    };
    for path in paths {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut doc = match std::fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|b| ondin_core::io::load(&b).map_err(|e| e.to_string()))
        {
            Ok(doc) => doc,
            Err(_) => {
                landed.report.push(ReportRow {
                    what: file,
                    count: String::new(),
                    how: "Could not be read, so it was left out".into(),
                });
                continue;
            }
        };
        if leave_pictures {
            let keep = Keep::all_but_pictures(&doc);
            if let Err(e) = ondin_core::take::take(&mut doc, &keep) {
                landed.report.push(ReportRow {
                    what: file,
                    count: String::new(),
                    how: format!("Could not be trimmed, so it was left out: {e}"),
                });
                continue;
            }
        }
        let name = document_name(&doc, path);
        let counts = Counts::of(&ondin_core::take::census(&doc));
        match file_copy(root, project, &name, doc) {
            Ok(written) => {
                landed.written.push(written);
                landed.counts.add(counts);
            }
            Err(e) => landed.report.push(ReportRow {
                what: file,
                count: String::new(),
                how: format!("Could not be written: {e}"),
            }),
        }
    }
    landed
}

impl OndinApp {
    /// *Import files*: the card, empty, filing into the project being viewed.
    pub(crate) fn open_import_card(&mut self) {
        if !self.library_writable() {
            return;
        }
        self.dash.import = Some(ImportCard::new(self.nav_project().map(|p| p.id)));
    }

    /// Fill the card's field with `paths` — a pick, a drop or a pasted path —
    /// opening the card first if it is not up. Several files are taken as several
    /// documents when they are documents; anything else comes in one at a time.
    pub(crate) fn fill_import(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if self.dash.import.is_none() {
            self.open_import_card();
        }
        let root = self.library.root.clone();
        let Some(card) = self.dash.import.as_mut() else {
            return;
        };
        // Not mid-copy: the field is not on screen to take it.
        if !matches!(card.step, Step::Form) {
            return;
        }
        card.typed.clear();
        card.note = None;
        let (docs, rest): (Vec<PathBuf>, Vec<PathBuf>) =
            paths.into_iter().partition(|p| Kind::of(p) == Kind::Ondin);
        let (docs, already): (Vec<PathBuf>, Vec<PathBuf>) = docs
            .into_iter()
            .partition(|p| in_library(&root, p).is_none());
        let (mut tools, others): (Vec<PathBuf>, Vec<PathBuf>) =
            rest.into_iter().partition(|p| Kind::of(p).imports());
        // Documents come in together; another tool's file comes alone, the first.
        let read = |path: &PathBuf| {
            let from = path.clone();
            Field::Reading {
                job: Job::spawn("import-read", ctx, move || read_source(&from)),
                path: path.clone(),
            }
        };
        let taken_tool = docs.is_empty() && !tools.is_empty();
        card.field = match (docs.len(), tools.first(), others.first(), already.first()) {
            (1, ..) => read(&docs[0]),
            (0, Some(path), ..) => read(path),
            (0, None, _, Some(path)) => Field::Refused {
                path: path.clone(),
                why: in_library(&root, path).unwrap_or_default().into(),
            },
            (0, None, Some(path), None) => Field::Refused {
                path: path.clone(),
                why: refusal(Kind::of(path)).into(),
            },
            (0, None, None, None) => Field::Empty,
            _ => Field::Many(docs.clone()),
        };
        if taken_tool {
            tools.remove(0);
        }
        let left = others.len() + already.len() + tools.len();
        if left > 0 && !matches!(card.field, Field::Refused { .. }) {
            card.note = Some(format!(
                "{left} other file{} left out: Ondin documents come in together, other files one at a time",
                if left == 1 { " was" } else { "s were" }
            ));
        }
    }

    /// *Browse…*: the system dialog, filtered to what the card imports.
    fn browse_for_import(&mut self, ctx: &egui::Context) {
        let Some(files) = rfd::FileDialog::new()
            .add_filter(
                "Ondin, Sketch and Penpot files",
                &[crate::library::scan::DOCUMENT_EXT, "sketch", "penpot"],
            )
            .add_filter("All files", &["*"])
            .set_title("Import into the library")
            .pick_files()
        else {
            return;
        };
        self.fill_import(ctx, files);
    }

    /// Land the workers' answers: a file read for its options, a copy finished,
    /// and any copy whose card was cancelled or closed, which is taken back out.
    pub(crate) fn poll_import(&mut self) {
        self.dash.import_abandoned.retain(|job| match job.poll() {
            Ok(None) => true,
            Ok(Some(landed)) => {
                for path in &landed.written {
                    let _ = std::fs::remove_file(path);
                }
                false
            }
            Err(()) => false,
        });
        let fonts = &self.fonts;
        let Some(card) = self.dash.import.as_mut() else {
            return;
        };
        if let Field::Reading { path, job } = &card.field {
            match job.poll() {
                Ok(None) => {}
                Ok(Some(Ok(mut source))) => {
                    // Here, not on the worker: the font list is the frame's.
                    for (family, runs) in std::mem::take(&mut source.families) {
                        if fonts.family_status(&family) == crate::fonts::FamilyStatus::Missing {
                            source.report.push((
                                Part::Layers,
                                ReportRow {
                                    what: family,
                                    count: counted(runs, "run", "runs"),
                                    how: "Not installed: drawn in Inter until it is".into(),
                                },
                            ));
                        }
                    }
                    let o = &mut card.options;
                    o.name.clone_from(&source.name);
                    o.pages = vec![true; source.census.pages.len()];
                    card.field = Field::One {
                        path: path.clone(),
                        source: Box::new(source),
                    };
                }
                Ok(Some(Err(why))) => {
                    card.field = Field::Refused {
                        path: path.clone(),
                        why,
                    }
                }
                Err(()) => {
                    card.field = Field::Refused {
                        path: path.clone(),
                        why: "Could not be read".into(),
                    }
                }
            }
        }
        let Step::Loading { job, .. } = &card.step else {
            return;
        };
        let landed = match job.poll() {
            Ok(None) => return,
            Ok(Some(landed)) => landed,
            Err(()) => Landed {
                written: Vec::new(),
                counts: Counts::default(),
                report: vec![ReportRow {
                    what: "The import".into(),
                    count: String::new(),
                    how: "Stopped before it finished".into(),
                }],
            },
        };
        let into = card
            .options
            .project
            .as_deref()
            .and_then(|id| self.library.projects.get(id))
            .map(|p| p.name.clone());
        card.step = Step::Done(overview(&card.field, &card.options, into, landed));
        self.library.refresh();
    }

    /// *Import*: start the copy on a worker.
    fn start_copy(&mut self, ctx: &egui::Context) {
        let root = self.library.root.clone();
        let Some(card) = self.dash.import.as_mut() else {
            return;
        };
        if !card.ready() {
            return;
        }
        // `destination`, so an archived project files as unfiled, as *New file* does.
        let project = card
            .options
            .project
            .as_deref()
            .and_then(|id| self.library.projects.destination(id))
            .cloned();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let (job, what) = match &card.field {
            Field::One { source, .. } => {
                let o = &card.options;
                let keep = Keep {
                    pages: source
                        .census
                        .pages
                        .iter()
                        .zip(&o.pages)
                        .filter(|(_, on)| **on)
                        .map(|(p, _)| p.id)
                        .collect(),
                    components: o.components,
                    styles: o.styles,
                    variables: o.variables,
                    pictures: !o.leave_pictures,
                };
                let doc = source.doc.clone();
                // A row about a part left out says nothing about the copy.
                let report = source
                    .report
                    .iter()
                    .filter(|(part, _)| match part {
                        Part::Components => o.components,
                        Part::Pictures => !o.leave_pictures,
                        Part::Layers => true,
                    })
                    .map(|(_, row)| row.clone())
                    .collect();
                let name = o.name.trim().to_owned();
                let what = format!("Importing {name}…");
                let job = Job::spawn("import-file", ctx, move || {
                    import_one(&root, project.as_ref(), &name, &doc, &keep, report, &flag)
                });
                (job, what)
            }
            Field::Many(paths) => {
                let paths = paths.clone();
                let leave = card.options.leave_pictures;
                let what = format!("Importing {} documents…", paths.len());
                let job = Job::spawn("import-files", ctx, move || {
                    import_many(&root, project.as_ref(), &paths, leave, &flag)
                });
                (job, what)
            }
            _ => return,
        };
        card.step = Step::Loading { job, cancel, what };
    }

    /// Stop a copy in progress and go back to the form. What the worker has
    /// already written is taken back out when it answers.
    fn cancel_import(&mut self) {
        let Some(card) = self.dash.import.as_mut() else {
            return;
        };
        if let Step::Loading { .. } = card.step {
            let Step::Loading { job, cancel, .. } = std::mem::replace(&mut card.step, Step::Form)
            else {
                unreachable!()
            };
            cancel.store(true, Ordering::Relaxed);
            self.dash.import_abandoned.push(job);
        }
    }

    /// Close the card, giving up a copy in progress.
    fn close_import(&mut self) {
        self.cancel_import();
        self.dash.import = None;
    }

    /// Draw the card, if it is open.
    pub(crate) fn import_modal(&mut self, ctx: &egui::Context) {
        self.poll_import();
        if self.dash.import.is_none() {
            return;
        }
        let mut act = None;
        let projects: Vec<project::Project> = self.library.projects.active().cloned().collect();
        let foot = self
            .dash
            .import
            .as_ref()
            .map(|c| foot_text(c, &self.library.projects))
            .unwrap_or_default();
        let modal = settings::card("import-files", ctx, |ui| {
            ui.set_width(ui::menu_inner_w(CARD_W, settings::PAD));
            ui.spacing_mut().item_spacing.y = 0.0;
            let Some(card) = self.dash.import.as_mut() else {
                return;
            };
            let title = if matches!(card.step, Step::Done(_)) {
                "Imported"
            } else {
                "Import a file"
            };
            if settings::modal_title(ui, title) {
                act = Some(CardAct::Close);
            }
            ui.add_space(settings::TITLE_GAP - 8.0);
            match &mut card.step {
                Step::Form => form(ui, card, &projects, &foot, &mut act),
                Step::Loading { what, .. } => {
                    loading(ui, what);
                    if settings::cancel_footer(ui) {
                        act = Some(CardAct::Cancel);
                    }
                }
                Step::Done(o) => done(ui, o, &mut act),
            }
        });
        if modal.should_close() && act.is_none() {
            act = Some(CardAct::Close);
        }
        if act.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            act = match self.dash.import.as_ref().map(|c| &c.step) {
                Some(Step::Form) => Some(CardAct::Import),
                Some(Step::Done(o)) if o.open.is_some() => Some(CardAct::Open),
                _ => None,
            };
        }
        match act {
            None => {}
            Some(CardAct::Close) => self.close_import(),
            Some(CardAct::Cancel) => self.cancel_import(),
            Some(CardAct::Browse) => self.browse_for_import(ctx),
            Some(CardAct::Fill(path)) => self.fill_import(ctx, vec![path]),
            Some(CardAct::Clear) => {
                if let Some(card) = self.dash.import.as_mut() {
                    card.field = Field::Empty;
                    card.note = None;
                }
            }
            Some(CardAct::Import) => self.start_copy(ctx),
            Some(CardAct::Copy) => {
                if let Some(Step::Done(o)) = self.dash.import.as_ref().map(|c| &c.step) {
                    ctx.copy_text(o.text());
                }
            }
            Some(CardAct::Open) => {
                let open = match self.dash.import.as_ref().map(|c| &c.step) {
                    Some(Step::Done(o)) => o.open.clone(),
                    _ => None,
                };
                self.dash.import = None;
                if let Some(path) = open {
                    self.begin_open(&path);
                }
            }
        }
    }
}

/// What a press in the card asks for, acted on once the card is drawn.
enum CardAct {
    Close,
    Cancel,
    Browse,
    Fill(PathBuf),
    Clear,
    Import,
    Copy,
    Open,
}

/// What the field says about a file it will not take.
fn refusal(kind: Kind) -> &'static str {
    match kind {
        Kind::Placed => {
            "Ondin places a PDF, an SVG or a picture onto a canvas rather than importing it. Open a document and use Place image."
        }
        Kind::Ondin | Kind::Sketch | Kind::Penpot | Kind::Other => {
            "Ondin imports Ondin, Sketch and Penpot files, and cannot read this kind of file."
        }
    }
}

/// The last part of `path`, as the user named the file.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The overview of a finished copy.
fn overview(field: &Field, o: &Options, into: Option<String>, landed: Landed) -> Overview {
    let place = into.unwrap_or_else(|| "the library".into());
    let (title, sub) = match (field, landed.written.len()) {
        (_, 0) => (
            "Nothing was imported".to_owned(),
            "The library is as it was".to_owned(),
        ),
        (Field::One { source, path }, _) if source.kind != Kind::Ondin => (
            format!("{} is in {place}", o.name.trim()),
            format!("From {}, which is unchanged", file_name(path)),
        ),
        (Field::One { .. }, _) => (
            format!("{} is in {place}", o.name.trim()),
            "A copy, with its own identity".to_owned(),
        ),
        (_, n) => (
            format!(
                "{n} document{} in {place}",
                if n == 1 { " is" } else { "s are" }
            ),
            "Copies, each with its own identity".to_owned(),
        ),
    };
    Overview {
        title,
        sub,
        counts: landed.counts,
        frames: match field {
            Field::One { source, .. } => source.kind.words().frames,
            _ => Kind::Ondin.words().frames,
        },
        report: landed.report,
        written: landed.written.len(),
        open: match field {
            Field::One { .. } => landed.written.first().cloned(),
            _ => None,
        },
    }
}

/// The first step: the field, then the options its file offers, then the footer.
fn form(
    ui: &mut egui::Ui,
    card: &mut ImportCard,
    projects: &[project::Project],
    foot: &str,
    act: &mut Option<CardAct>,
) {
    super::dashboard::field_label(ui, "File");
    file_field(ui, card, act);
    if let Some(note) = &card.note {
        message(ui, icon::INFO, note, theme::text::DIM);
    }
    let ready = card.ready();
    match &card.field {
        Field::One { source, .. } => {
            divider(ui);
            name_and_project(ui, &mut card.options, projects, true);
            let words = source.kind.words();
            pages(ui, &source.census, words, &mut card.options.pages);
            bring_over(ui, &source.census, words, &mut card.options);
            pictures(
                ui,
                source.census.pictures,
                source.census.picture_bytes,
                &mut card.options,
            );
        }
        Field::Many(_) => {
            divider(ui);
            name_and_project(ui, &mut card.options, projects, false);
            ui.add_space(GROUP_GAP);
            group_label(ui, "Pictures");
            if check_row(
                ui,
                "Leave out their pictures",
                "",
                "Image fills stay, drawn as missing until a picture is placed in them.",
                card.options.leave_pictures,
            ) {
                card.options.leave_pictures = !card.options.leave_pictures;
            }
        }
        _ => {}
    }
    ui.add_space(settings::FOOTER_GAP - 6.0);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(foot)
                .size(settings::CAPTION_PT)
                .color(theme::text::FAINT),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let state = if ready {
                FieldButton::On
            } else {
                FieldButton::Disabled
            };
            if settings::text_button(ui, "Import", state).clicked() && ready {
                *act = Some(CardAct::Import);
            }
            if settings::text_button(ui, "Cancel", FieldButton::Off).clicked() {
                *act = Some(CardAct::Close);
            }
        });
    });
}

/// The file field and *Browse…*: a badge and the file's name once one is chosen,
/// else a text field a path can be typed or pasted into. The line under it says
/// what the file is, or why it is not taken.
fn file_field(ui: &mut egui::Ui, card: &mut ImportCard, act: &mut Option<CardAct>) {
    let browse_w = 96.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let w = ui.available_width() - browse_w - 8.0;
        let shown = match &card.field {
            Field::Empty => None,
            Field::Reading { path, .. } | Field::One { path, .. } | Field::Refused { path, .. } => {
                Some((
                    Kind::of(path).badge(path),
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                ))
            }
            Field::Many(paths) => Some((
                Kind::Ondin.badge(&paths[0]),
                format!("{} documents", paths.len()),
            )),
        };
        match shown {
            None => {
                let resp = ui::text_field(
                    ui,
                    egui::vec2(w, FIELD_H),
                    &mut card.typed,
                    "Choose a file, or paste its path",
                    12.0,
                );
                if resp.changed()
                    && let Some(path) = typed_path(&card.typed)
                {
                    *act = Some(CardAct::Fill(path));
                }
            }
            Some(((badge, tint), name)) => {
                ui::field_row_inset(ui, egui::vec2(w, FIELD_H), 6.0, 4.0, |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    badge_chip(ui, &badge, tint);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(name)
                                .size(12.0)
                                .color(theme::text::STRONG),
                        )
                        .truncate(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui::icon_button(ui, icon::X_CIRCLE, 22.0, 14.0, false, true)
                            .on_hover_text("Clear")
                            .clicked()
                        {
                            *act = Some(CardAct::Clear);
                        }
                    });
                });
            }
        }
        if ui::label_button(
            ui,
            "Browse…",
            egui::vec2(browse_w, FIELD_H),
            FieldButton::Off,
        )
        .clicked()
        {
            *act = Some(CardAct::Browse);
        }
    });
    match &card.field {
        Field::Empty => {}
        Field::Reading { .. } => message(ui, icon::FILE, "Reading…", theme::text::FAINT),
        Field::One { path, source } => {
            let folder = path
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            message(
                ui,
                icon::FILE,
                &format!(
                    "{} · {folder} · {}",
                    source.kind.noun(),
                    settings::human_bytes(source.bytes)
                ),
                theme::text::FAINT,
            );
        }
        Field::Many(paths) => {
            let names: Vec<String> = paths
                .iter()
                .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                .collect();
            message(ui, icon::FILES, &names.join(", "), theme::text::FAINT);
        }
        Field::Refused { why, .. } => message(ui, icon::INFO, why, color::WARN),
    }
}

/// Why `path` is refused as already inside the library at `root`, or `None` when
/// it is outside. Compared canonically, so a case difference, a `..` or a
/// junction neither slips a library file past the refusal nor refuses one outside
/// (`root/../elsewhere` starts with `root` component by component).
fn in_library(root: &Path, path: &Path) -> Option<&'static str> {
    // Both or neither: a canonical root (`\\?\C:\…` on Windows) never prefixes a raw path.
    let (root, path) = match (std::fs::canonicalize(root), std::fs::canonicalize(path)) {
        (Ok(r), Ok(p)) => (r, p),
        _ => (root.to_path_buf(), path.to_path_buf()),
    };
    let rest = path.strip_prefix(&root).ok()?;
    let first = rest.components().next()?.as_os_str();
    Some(if first == crate::library::scan::TRASH_DIR {
        "In the library's trash. Restore it there to have it back."
    } else if first == crate::library::scan::VERSIONS_DIR {
        "A saved version inside the library. Restore it from the document's history."
    } else {
        "Already in the library. Duplicate it there to have a copy."
    })
}

/// A path typed or pasted into the empty field, once it names a file: quotes
/// trimmed, as Explorer's *Copy as path* adds them.
fn typed_path(typed: &str) -> Option<PathBuf> {
    let t = typed.trim().trim_matches('"').trim();
    if t.is_empty() {
        return None;
    }
    let path = PathBuf::from(t);
    path.is_file().then_some(path)
}

/// The type badge at the head of a filled field.
fn badge_chip(ui: &mut egui::Ui, text: &str, tint: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(48.0, 18.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 4.0, tint.gamma_multiply(0.16));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(9.0),
        tint,
    );
}

/// The line under the field: a glyph and a sentence, wrapped to the card.
fn message(ui: &mut egui::Ui, glyph: &str, text: &str, col: egui::Color32) {
    ui.add_space(6.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(theme::icon_text(glyph, 12.0, col));
        ui.add(
            egui::Label::new(
                egui::RichText::new(text)
                    .size(settings::CAPTION_PT + 0.5)
                    .color(col),
            )
            .wrap(),
        );
    });
}

/// The hairline between the field and the options.
fn divider(ui: &mut egui::Ui) {
    ui.add_space(GROUP_GAP + 2.0);
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::empty());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        egui::Stroke::new(1.0, theme::color::DIVIDER),
    );
    ui.add_space(GROUP_GAP + 2.0);
}

/// The small grey word over a group of options.
fn group_label(ui: &mut egui::Ui, text: &str) {
    super::dashboard::field_label(ui, text);
    ui.add_space(2.0);
}

/// *Name* and *Project* side by side; several documents keep their own names, so
/// they get the project alone.
fn name_and_project(
    ui: &mut egui::Ui,
    o: &mut Options,
    projects: &[project::Project],
    with_name: bool,
) {
    let gap = 12.0;
    let half = (ui.available_width() - gap) / 2.0;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        if with_name {
            ui.vertical(|ui| {
                ui.set_width(half);
                super::dashboard::field_label(ui, "Name");
                ui::text_field(
                    ui,
                    egui::vec2(half, settings::FIELD_H),
                    &mut o.name,
                    "Document name",
                    12.0,
                );
            });
        }
        ui.vertical(|ui| {
            ui.set_width(half);
            super::dashboard::field_label(ui, "Project");
            project_picker(ui, o, projects, half);
        });
    });
}

/// The *Project* dropdown: *No project*, then every project a document may be
/// filed into, each with its colour.
fn project_picker(ui: &mut egui::Ui, o: &mut Options, projects: &[project::Project], w: f32) {
    let shown = o
        .project
        .as_deref()
        .and_then(|id| projects.iter().find(|p| p.id == id));
    let selected = match shown {
        Some(p) => dotted(&p.name, Some(&p.color)),
        None => dotted("No project", None),
    };
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = settings::FIELD_H;
        ui.spacing_mut().button_padding.y = 0.0;
        egui::ComboBox::from_id_salt("import-project")
            .icon(ui::combo_chevron)
            .width(w)
            .selected_text(selected)
            .show_ui(ui, |ui| {
                ui::menu_rows(ui);
                ui.spacing_mut().button_padding.y = 2.0;
                ui.selectable_value(&mut o.project, None, dotted("No project", None));
                for p in projects {
                    ui.selectable_value(
                        &mut o.project,
                        Some(p.id.clone()),
                        dotted(&p.name, Some(&p.color)),
                    );
                }
            });
    });
}

/// A project's name behind its colour dot, or behind a hollow ring for none.
fn dotted(name: &str, hex: Option<&str>) -> egui::WidgetText {
    let mut job = egui::text::LayoutJob::default();
    let (glyph, col) = match hex {
        Some(hex) => ("●", super::dashboard::swatch(hex)),
        None => ("○", theme::text::FAINT),
    };
    job.append(
        glyph,
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(9.0),
            color: col,
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    job.append(
        name,
        7.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(12.0),
            color: color::TEXT,
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    job.into()
}

/// One tick per page, with how many frames it holds.
fn pages(ui: &mut egui::Ui, census: &Census, words: Words, keep: &mut [bool]) {
    ui.add_space(GROUP_GAP);
    group_label(ui, "Pages");
    egui::ScrollArea::vertical()
        .id_salt("import-pages")
        .max_height(PAGES_MAX_H)
        .show(ui, |ui| {
            for (page, on) in census.pages.iter().zip(keep.iter_mut()) {
                let meta = counted(page.frames, words.frames.0, words.frames.1);
                if check_row(ui, &page.name, &meta, "", *on) {
                    *on = !*on;
                }
            }
        });
}

/// What to bring over besides the layers, each saying what leaving it does.
fn bring_over(ui: &mut egui::Ui, census: &Census, words: Words, o: &mut Options) {
    ui.add_space(GROUP_GAP);
    group_label(ui, "Bring over");
    if check_row(
        ui,
        words.components,
        &census.components.to_string(),
        "Off: instances become plain layers.",
        o.components,
    ) {
        o.components = !o.components;
    }
    if check_row(
        ui,
        words.styles,
        &census.styles.to_string(),
        words.styles_off,
        o.styles,
    ) {
        o.styles = !o.styles;
    }
    let vars = if census.modes > 1 {
        format!("{} · {} modes", census.variables, census.modes)
    } else {
        census.variables.to_string()
    };
    if check_row(ui, words.variables, &vars, words.variables_off, o.variables) {
        o.variables = !o.variables;
    }
}

/// Whether to leave the pictures behind.
fn pictures(ui: &mut egui::Ui, count: usize, bytes: u64, o: &mut Options) {
    ui.add_space(GROUP_GAP);
    group_label(ui, "Pictures");
    let meta = if count == 0 {
        "0".to_owned()
    } else {
        format!("{count} · {}", settings::human_bytes(bytes))
    };
    if check_row(
        ui,
        "Leave out its pictures",
        &meta,
        "Image fills stay, drawn as missing until a picture is placed in them.",
        o.leave_pictures,
    ) {
        o.leave_pictures = !o.leave_pictures;
    }
}

/// `n` and its noun, singular at one.
fn counted(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A tick box, its label, a count against the right margin and, when given, a
/// sentence under the label. Answers whether it was clicked.
fn check_row(ui: &mut egui::Ui, label: &str, meta: &str, sub: &str, on: bool) -> bool {
    let w = ui.available_width();
    let text_x = TICK + 9.0;
    let sub_g = (!sub.is_empty()).then(|| {
        ui.fonts_mut(|f| {
            f.layout(
                sub.to_owned(),
                egui::FontId::proportional(settings::CAPTION_PT),
                theme::text::FAINT,
                w - text_x,
            )
        })
    });
    let h = 22.0 + sub_g.as_ref().map_or(0.0, |g| g.size().y + 1.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::click());
    let p = ui.painter();
    let line = rect.top() + 11.0;
    let tick = egui::Rect::from_min_size(
        egui::pos2(rect.left(), line - TICK / 2.0),
        egui::vec2(TICK, TICK),
    );
    if on {
        p.rect_filled(tick, 3.0, color::ACCENT);
        p.text(
            tick.center(),
            egui::Align2::CENTER_CENTER,
            icon::CHECK,
            theme::icon_font(10.0),
            egui::Color32::WHITE,
        );
    } else {
        let ring = if resp.hovered() {
            color::text_a(140)
        } else {
            color::text_a(90)
        };
        p.rect_stroke(
            tick,
            3.0,
            egui::Stroke::new(1.0, ring),
            egui::StrokeKind::Inside,
        );
    }
    p.text(
        egui::pos2(rect.left() + text_x, line),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(12.0),
        theme::text::STRONG,
    );
    p.text(
        egui::pos2(rect.right(), line),
        egui::Align2::RIGHT_CENTER,
        meta,
        egui::FontId::proportional(11.0),
        theme::text::FAINT,
    );
    if let Some(g) = sub_g {
        p.galley(
            egui::pos2(rect.left() + text_x, line + 8.0),
            g,
            theme::text::FAINT,
        );
    }
    resp.clicked()
}

/// The copy in progress, in place of the options.
fn loading(ui: &mut egui::Ui, what: &str) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.add(egui::Spinner::new().size(14.0).color(theme::text::DIM));
        ui.label(
            egui::RichText::new(what)
                .size(12.0)
                .color(theme::text::STRONG),
        );
    });
}

/// The overview: what came in, counted, what did not, and the way into it.
fn done(ui: &mut egui::Ui, o: &Overview, act: &mut Option<CardAct>) {
    let ok = o.written > 0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::hover());
        let tint = if ok { color::SAVED } else { color::WARN };
        ui.painter().rect(
            rect,
            9.0,
            tint.gamma_multiply(0.16),
            egui::Stroke::new(1.0, tint.gamma_multiply(0.4)),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            if ok { icon::CHECK } else { icon::INFO },
            theme::icon_font(17.0),
            tint,
        );
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(&o.title)
                    .size(14.0)
                    .color(theme::text::STRONG),
            );
            ui.label(
                egui::RichText::new(&o.sub)
                    .size(settings::CAPTION_PT + 0.5)
                    .color(theme::text::DIM),
            );
        });
    });
    ui.add_space(16.0);
    let gap = 8.0;
    let cell_w = (ui.available_width() - gap * 2.0) / 3.0;
    for row in o.counts.cells(o.frames).chunks(3) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for &(n, one, many) in row {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(cell_w, 50.0), egui::Sense::hover());
                let p = ui.painter();
                p.rect_filled(rect, 8.0, color::text_a(15));
                p.text(
                    rect.left_top() + egui::vec2(11.0, 9.0),
                    egui::Align2::LEFT_TOP,
                    n.to_string(),
                    egui::FontId::proportional(16.0),
                    theme::text::STRONG,
                );
                p.text(
                    rect.left_bottom() + egui::vec2(11.0, -9.0),
                    egui::Align2::LEFT_BOTTOM,
                    if n == 1 { one } else { many },
                    egui::FontId::proportional(11.0),
                    theme::text::DIM,
                );
            }
        });
        ui.add_space(gap);
    }
    ui.add_space(8.0);
    report(ui, &o.report);
    ui.add_space(settings::FOOTER_GAP - 6.0);
    ui.horizontal(|ui| {
        if ui
            .add(
                egui::Button::new(theme::icon_text(icon::COPY, 13.0, theme::text::MUTED))
                    .frame(false),
            )
            .on_hover_text("Copy report")
            .clicked()
        {
            *act = Some(CardAct::Copy);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if o.open.is_some()
                && settings::text_button(ui, "Open document", FieldButton::On).clicked()
            {
                *act = Some(CardAct::Open);
            }
            if settings::text_button(ui, "Done", FieldButton::Off).clicked() {
                *act = Some(CardAct::Close);
            }
        });
    });
}

/// The boxed list of what did not come in as it was, under its count.
fn report(ui: &mut egui::Ui, rows: &[ReportRow]) {
    let head = if rows.is_empty() {
        "Nothing to report".to_owned()
    } else {
        format!("Not brought over as it was · {}", rows.len())
    };
    egui::Frame::NONE
        .stroke(egui::Stroke::new(1.0, color::text_a(23)))
        .corner_radius(8)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            egui::Frame::NONE
                .fill(color::text_a(13))
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        egui::RichText::new(head)
                            .size(11.0)
                            .color(theme::text::MUTED),
                    );
                });
            for r in rows {
                egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(&r.what)
                                    .size(12.0)
                                    .color(theme::text::STRONG),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(&r.count)
                                            .size(11.0)
                                            .color(theme::text::FAINT),
                                    );
                                },
                            );
                        });
                        ui.add_space(2.0);
                        ui.label(
                            egui::RichText::new(&r.how)
                                .size(11.0)
                                .color(theme::text::DIM),
                        );
                    });
            }
        });
}

#[cfg(test)]
impl OndinApp {
    /// Wait for every worker the card has running, then land their answers as a
    /// frame would.
    pub(crate) fn settle_import(&mut self) {
        self.wait_import();
        self.poll_import();
    }

    /// Wait for every worker the card has running, landing nothing.
    fn wait_import(&mut self) {
        let abandoned = std::mem::take(&mut self.dash.import_abandoned);
        self.dash.import_abandoned = abandoned
            .into_iter()
            .filter_map(|job| job.wait().map(Job::done))
            .collect();
        if let Some(card) = self.dash.import.as_mut() {
            card.field = match std::mem::replace(&mut card.field, Field::Empty) {
                Field::Reading { path, job } => Field::Reading {
                    job: Job::done(job.wait().unwrap_or_else(|| Err("no worker".into()))),
                    path,
                },
                other => other,
            };
            card.step = match std::mem::replace(&mut card.step, Step::Form) {
                Step::Loading { job, cancel, what } => match job.wait() {
                    Some(landed) => Step::Loading {
                        job: Job::done(landed),
                        cancel,
                        what,
                    },
                    None => Step::Form,
                },
                other => other,
            };
        }
    }

    /// Drop `paths` on the card, press *Import* and wait: how many documents were
    /// written.
    pub(crate) fn import_through_card(
        &mut self,
        ctx: &egui::Context,
        paths: Vec<PathBuf>,
    ) -> usize {
        self.fill_import(ctx, paths);
        self.settle_import();
        self.start_copy(ctx);
        self.settle_import();
        match self.dash.import.as_ref().map(|c| &c.step) {
            Some(Step::Done(o)) => o.written,
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::{Document, Operation, Transaction};

    struct Fixture {
        app: OndinApp,
        root: PathBuf,
        outside: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
            let _ = std::fs::remove_dir_all(&self.outside);
        }
    }

    fn fixture(ctx: &egui::Context, tag: &str) -> Fixture {
        let dir = |side: &str| {
            let d = std::env::temp_dir()
                .join(format!("ondin-import-{tag}-{side}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            d
        };
        let (root, outside) = (dir("lib"), dir("src"));
        let mut app = OndinApp::headless(ctx);
        app.prefs.base_folder = Some(root.clone());
        app.library = crate::library::state::Library::open(root.clone());
        Fixture { app, root, outside }
    }

    /// A two-page document named `name`, filed outside the library.
    fn two_pages(f: &mut Fixture, name: &str) -> PathBuf {
        let first = f.app.session.ids.mint();
        let second = f.app.session.ids.mint();
        let mut doc = Document::new(first);
        doc.apply(&Transaction(vec![Operation::AddPage {
            id: second,
            index: 1,
            name: "Archive".into(),
            ground: ondin_core::peniko::Color::WHITE,
            draft: false,
            mode: None,
        }]))
        .unwrap();
        store::file_document(&f.outside, None, name, &mut doc).unwrap()
    }

    fn listed(f: &Fixture) -> Vec<crate::library::scan::Entry> {
        crate::library::scan::scan(&f.root)
    }

    /// Importing copies, keeps the name that arrived, and mints a fresh id each
    /// time (two documents claiming one identity would share history and cover).
    #[test]
    fn an_import_is_a_copy_with_an_identity_of_its_own() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "copy");
        let source = two_pages(&mut f, "My cool design");
        let original = crate::library::scan::read_meta(&source)
            .and_then(|(m, _)| m.id)
            .unwrap();

        assert_eq!(f.app.import_through_card(&ctx, vec![source.clone()]), 1);
        f.app.dash.import = None;
        assert_eq!(f.app.import_through_card(&ctx, vec![source.clone()]), 1);

        assert!(source.exists(), "importing copies rather than moves");
        let got = listed(&f);
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|e| e.display_name() == "My cool design"));
        assert!(
            got.iter()
                .all(|e| e.meta.id.as_deref() != Some(original.as_str()))
        );
        assert_ne!(got[0].meta.id, got[1].meta.id);
    }

    /// Nothing is written before *Import*, and the overview names where the copy
    /// went and offers it.
    #[test]
    fn nothing_is_filed_until_import_and_the_overview_offers_the_copy() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "overview");
        let source = two_pages(&mut f, "Onboarding");
        f.app.fill_import(&ctx, vec![source]);
        f.app.settle_import();
        assert!(f.app.dash.import.as_ref().is_some_and(ImportCard::ready));
        assert!(listed(&f).is_empty(), "the field alone files nothing");

        f.app.start_copy(&ctx);
        f.app.settle_import();
        let Some(Step::Done(o)) = f.app.dash.import.as_ref().map(|c| &c.step) else {
            panic!("the card shows the overview");
        };
        assert_eq!(o.title, "Onboarding is in the library");
        assert_eq!(o.counts.pages, 2);
        assert!(o.open.as_ref().is_some_and(|p| p.starts_with(&f.root)));
        assert!(o.text().contains("Nothing to report"));
    }

    /// A page left unticked is not in the copy. flip: build `Keep::pages` from
    /// every page in `start_copy` → fails at the page count.
    #[test]
    fn an_unticked_page_is_left_out_of_the_copy() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "pages");
        let source = two_pages(&mut f, "Big one");
        f.app.fill_import(&ctx, vec![source]);
        f.app.settle_import();
        let card = f.app.dash.import.as_mut().unwrap();
        assert_eq!(
            card.options.pages,
            [true, true],
            "both pages offered, ticked"
        );
        card.options.pages[1] = false;
        f.app.start_copy(&ctx);
        f.app.settle_import();
        let got = listed(&f);
        assert_eq!(got.len(), 1);
        let doc = ondin_core::io::load(&std::fs::read(&got[0].path).unwrap()).unwrap();
        assert_eq!(doc.pages().len(), 1);
    }

    /// No page ticked turns *Import* off.
    #[test]
    fn import_needs_a_page_and_a_name() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "ready");
        let source = two_pages(&mut f, "Doc");
        f.app.fill_import(&ctx, vec![source]);
        f.app.settle_import();
        let card = f.app.dash.import.as_mut().unwrap();
        card.options.pages = vec![false, false];
        assert!(!card.ready());
        card.options.pages[0] = true;
        card.options.name = "  ".into();
        assert!(!card.ready());
    }

    /// A file already in the library is refused in the field rather than copied
    /// again. flip: remove the `starts_with(&root)` partition in `fill_import` →
    /// fails at the refusal.
    #[test]
    fn a_file_already_in_the_library_is_refused() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "already");
        let mut doc = Document::new(f.app.session.ids.mint());
        let inside = store::file_document(&f.root, None, "Landing v4", &mut doc).unwrap();
        f.app.fill_import(&ctx, vec![inside]);
        f.app.settle_import();
        let card = f.app.dash.import.as_ref().unwrap();
        assert!(
            matches!(&card.field, Field::Refused { why, .. } if why.contains("Already in the library"))
        );
        assert!(!card.ready());
    }

    /// A document with no name of its own takes its filename's, written down once.
    #[test]
    fn a_document_without_a_name_is_named_from_its_file() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "bare");
        let doc = Document::new(f.app.session.ids.mint());
        let source = f.outside.join("old-hero-sketch.ondin");
        std::fs::write(&source, ondin_core::io::save(&doc).unwrap()).unwrap();
        assert_eq!(f.app.import_through_card(&ctx, vec![source]), 1);
        let got = listed(&f);
        assert_eq!(got[0].meta.name.as_deref(), Some("Old hero sketch"));
    }

    /// Several documents come in together; one that does not read is reported
    /// and the rest land.
    #[test]
    fn several_documents_land_together_and_a_bad_one_is_reported() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "many");
        let a = two_pages(&mut f, "First");
        let b = two_pages(&mut f, "Second");
        let bad = f.outside.join("broken.ondin");
        std::fs::write(&bad, b"not a document").unwrap();
        f.app.fill_import(&ctx, vec![a, b, bad]);
        assert!(matches!(
            f.app.dash.import.as_ref().map(|c| &c.field),
            Some(Field::Many(p)) if p.len() == 3
        ));
        f.app.fill_import(&ctx, vec![]);
        assert!(
            matches!(
                f.app.dash.import.as_ref().map(|c| &c.field),
                Some(Field::Many(_))
            ),
            "an empty refill leaves the field alone"
        );
        f.app.start_copy(&ctx);
        f.app.settle_import();
        let Some(Step::Done(o)) = f.app.dash.import.as_ref().map(|c| &c.step) else {
            panic!("the card shows the overview");
        };
        assert_eq!(o.title, "2 documents are in the library");
        assert_eq!(o.report.len(), 1);
        assert_eq!(o.report[0].what, "broken.ondin");
        assert_eq!(o.open, None, "several documents have no one to open");
        assert_eq!(listed(&f).len(), 2);
    }

    /// A copy cancelled after its worker has written leaves nothing behind: the
    /// cancel arrives too late to stop the write, so the landing takes it back
    /// out. flip: skip the removal in `poll_import` → fails at the listing.
    #[test]
    fn a_copy_cancelled_after_it_was_written_leaves_nothing_behind() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "cancel");
        let source = two_pages(&mut f, "Abandoned");
        f.app.fill_import(&ctx, vec![source]);
        f.app.settle_import();
        f.app.start_copy(&ctx);
        f.app.wait_import();
        assert_eq!(listed(&f).len(), 1, "the worker has written the copy");
        f.app.cancel_import();
        assert!(matches!(
            f.app.dash.import.as_ref().map(|c| &c.step),
            Some(Step::Form)
        ));
        f.app.settle_import();
        assert!(f.app.dash.import_abandoned.is_empty());
        assert!(listed(&f).is_empty(), "{:?}", listed(&f));
    }

    /// A typed or pasted path fills the field once it names a file; Explorer's
    /// quotes are trimmed.
    #[test]
    fn a_pasted_path_is_taken_once_it_names_a_file() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "typed");
        let source = two_pages(&mut f, "Typed");
        assert_eq!(
            typed_path(&format!("\"{}\"", source.display())),
            Some(source.clone())
        );
        assert_eq!(
            typed_path(&f.outside.display().to_string()),
            None,
            "a folder"
        );
        assert_eq!(typed_path("C:\\no\\such\\file.ondin"), None);
        assert_eq!(typed_path("  "), None);
    }

    /// Draw the card for a few frames: every text run, and the card's rect.
    fn drawn(f: &mut Fixture, ctx: &egui::Context) -> (Vec<(egui::Rect, String)>, egui::Rect) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 1000.0),
            )),
            ..Default::default()
        };
        let mut runs = Vec::new();
        for _ in 0..4 {
            let full = ctx.run_ui(input.clone(), |ui| f.app.import_modal(ui.ctx()));
            runs = full
                .shapes
                .iter()
                .filter_map(|cs| match &cs.shape {
                    egui::Shape::Text(t) => Some((
                        egui::Rect::from_min_size(t.pos, t.galley.size()),
                        t.galley.text().to_owned(),
                    )),
                    _ => None,
                })
                .collect();
        }
        let card = ctx
            .memory(|m| m.area_rect(egui::Id::new("import-files")))
            .expect("the card was drawn");
        (runs, card)
    }

    /// In every state — empty, refused, one document, several, copying, done —
    /// the card draws its words and none runs past its edge.
    #[test]
    fn every_state_of_the_card_stays_inside_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut f = fixture(&ctx, "drawn");
        let one = two_pages(&mut f, "A document with quite a long name indeed");
        let two = two_pages(&mut f, "Second");
        let pdf = f.outside.join("Poster A2.pdf");
        std::fs::write(&pdf, b"%PDF").unwrap();
        f.app.open_import_card();
        let check = |f: &mut Fixture, want: &[&str]| {
            let (runs, card) = drawn(f, &ctx);
            for (rect, text) in &runs {
                assert!(
                    card.expand(0.5).contains_rect(*rect),
                    "{text:?} at {rect:?} runs out of the card {card:?}"
                );
            }
            for w in want {
                assert!(
                    runs.iter().any(|(_, t)| t.contains(w)),
                    "{w:?} is not drawn: {:?}",
                    runs.iter().map(|(_, t)| t).collect::<Vec<_>>()
                );
            }
        };
        check(&mut f, &["Import a file", "Browse…", "Choose a file"]);
        f.app.fill_import(&ctx, vec![pdf]);
        check(&mut f, &["Place image", "Not a file Ondin imports"]);
        f.app.fill_import(&ctx, vec![one]);
        f.app.settle_import();
        check(
            &mut f,
            &[
                "Pages",
                "Archive",
                "Bring over",
                "Variables",
                "Leave out its pictures",
            ],
        );
        f.app.fill_import(&ctx, vec![two.clone(), two]);
        check(&mut f, &["2 documents", "Leave out their pictures"]);
        // A copy that answers only when the test lets it, so the frames see it running.
        let (go, wait) = std::sync::mpsc::channel::<()>();
        f.app.dash.import.as_mut().unwrap().step = Step::Loading {
            job: Job::spawn("held", &ctx, move || {
                let _ = wait.recv();
                Landed {
                    written: Vec::new(),
                    counts: Counts::default(),
                    report: Vec::new(),
                }
            }),
            cancel: Arc::new(AtomicBool::new(false)),
            what: "Importing 2 documents…".into(),
        };
        check(&mut f, &["Importing 2 documents…", "Cancel"]);
        drop(go);
        f.app.settle_import();
        f.app.dash.import.as_mut().unwrap().step = Step::Form;
        f.app.start_copy(&ctx);
        f.app.settle_import();
        check(
            &mut f,
            &["Imported", "2 documents are in the library", "pages"],
        );
    }

    /// A library file is recognised however its path is spelled, and a trashed one
    /// is told to be restored rather than duplicated. flip: compare the raw paths
    /// in `in_library` → fails at the `..` out of the library.
    #[test]
    fn a_library_file_is_known_by_any_spelling_and_the_trash_by_name() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "spelling");
        let mut doc = Document::new(f.app.session.ids.mint());
        let inside = store::file_document(&f.root, None, "Inside", &mut doc).unwrap();
        let round = f
            .root
            .join("..")
            .join(f.root.file_name().unwrap())
            .join(inside.file_name().unwrap());
        assert!(round.is_file(), "control: the spelling names the same file");
        assert_eq!(in_library(&f.root, &f.outside), None);
        let around = f.root.join("..").join(f.outside.file_name().unwrap());
        assert_eq!(
            in_library(&f.root, &around),
            None,
            "a `..` out of the library"
        );
        assert!(in_library(&f.root, &round).is_some_and(|w| w.starts_with("Already")));
        let trashed = f
            .root
            .join(crate::library::scan::TRASH_DIR)
            .join("old.ondin");
        std::fs::create_dir_all(trashed.parent().unwrap()).unwrap();
        std::fs::write(&trashed, b"{}").unwrap();
        assert!(in_library(&f.root, &trashed).is_some_and(|w| w.contains("trash")));
    }

    /// A `.sketch` zip holding one page with an artboard, a rectangle and a picture.
    fn a_sketch_file(f: &Fixture, name: &str) -> PathBuf {
        let frame =
            |x: f64, w: f64| serde_json::json!({ "x": x, "y": 0, "width": w, "height": 50 });
        let page = serde_json::json!({
            "_class": "page", "do_objectID": "P", "name": "Web",
            "layers": [{
                "_class": "artboard", "do_objectID": "A", "name": "Home", "frame": frame(0.0, 200.0),
                "layers": [
                    { "_class": "rectangle", "do_objectID": "R", "name": "Card", "frame": frame(10.0, 40.0),
                      "flow": { "destinationArtboardID": "A" } },
                    { "_class": "bitmap", "do_objectID": "B", "name": "Photo", "frame": frame(60.0, 40.0),
                      "image": { "_ref": "images/photo.png" } },
                ],
            }],
        });
        let document =
            serde_json::json!({ "_class": "document", "pages": [{ "_ref": "pages/P" }] });
        let mut png = Vec::new();
        image::write_buffer_with_format(
            &mut std::io::Cursor::new(&mut png),
            &[255u8; 16],
            2,
            2,
            image::ExtendedColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .unwrap();
        let zip = ondin_export::plan::zip(&[
            ("document.json".into(), document.to_string().into_bytes()),
            ("pages/P.json".into(), page.to_string().into_bytes()),
            ("images/photo.png".into(), png),
        ])
        .unwrap();
        let path = f.outside.join(name);
        std::fs::write(&path, zip).unwrap();
        path
    }

    /// A Sketch file is read into the card in Sketch's words, and comes in as a
    /// new document whose overview says the file is unchanged and what did not
    /// map.
    #[test]
    fn a_sketch_file_comes_in_through_the_card() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "sketch");
        let source = a_sketch_file(&f, "Marketing site.sketch");
        f.app.fill_import(&ctx, vec![source.clone()]);
        f.app.settle_import();
        let card = f.app.dash.import.as_ref().unwrap();
        let Field::One { source: read, .. } = &card.field else {
            panic!(
                "the file reads: {:?}",
                matches!(card.field, Field::Refused { .. })
            );
        };
        assert_eq!(read.kind, Kind::Sketch);
        assert_eq!(card.options.name, "Marketing site");
        assert_eq!(read.census.frames, 1);
        assert_eq!(read.census.pictures, 1, "the picture was read and sized");
        f.app.start_copy(&ctx);
        f.app.settle_import();
        let Some(Step::Done(o)) = f.app.dash.import.as_ref().map(|c| &c.step) else {
            panic!("the card shows the overview");
        };
        assert_eq!(o.sub, "From Marketing site.sketch, which is unchanged");
        assert!(o.text().contains("1 artboard"), "{}", o.text());
        assert!(
            o.report.iter().any(|r| r.what == "Prototype links"),
            "{:?}",
            o.report
        );
        assert!(source.exists());
        assert_eq!(listed(&f).len(), 1);
    }

    /// A report row about a part the import leaves out is dropped with it: an
    /// unreadable picture is not reported once the pictures stay behind. flip:
    /// keep every row in `start_copy` → fails at the second report.
    #[test]
    fn a_row_about_a_part_left_out_is_not_reported() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "sketch-rows");
        let page = serde_json::json!({ "_class": "page", "do_objectID": "P", "name": "Web",
            "layers": [{ "_class": "bitmap", "do_objectID": "B", "name": "Photo",
                         "frame": { "x": 0, "y": 0, "width": 10, "height": 10 },
                         "image": { "_ref": "images/gone.png" } }] });
        let document = serde_json::json!({ "pages": [{ "_ref": "pages/P" }] });
        let zip = ondin_export::plan::zip(&[
            ("document.json".into(), document.to_string().into_bytes()),
            ("pages/P.json".into(), page.to_string().into_bytes()),
        ])
        .unwrap();
        let path = f.outside.join("Gone.sketch");
        std::fs::write(&path, zip).unwrap();
        let report = |f: &mut Fixture, leave: bool| {
            f.app.dash.import = None;
            f.app.fill_import(&ctx, vec![path.clone()]);
            f.app.settle_import();
            f.app.dash.import.as_mut().unwrap().options.leave_pictures = leave;
            f.app.start_copy(&ctx);
            f.app.settle_import();
            match f.app.dash.import.as_ref().map(|c| &c.step) {
                Some(Step::Done(o)) => o.report.iter().map(|r| r.what.clone()).collect::<Vec<_>>(),
                _ => panic!("the card shows the overview"),
            }
        };
        assert_eq!(
            report(&mut f, false),
            ["Pictures"],
            "control: the row is there"
        );
        assert!(report(&mut f, true).is_empty());
    }

    /// A Penpot export is read in Penpot's words and filed under the name the
    /// file gives itself.
    #[test]
    fn a_penpot_export_comes_in_through_the_card() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "penpot");
        let root = "00000000-0000-0000-0000-000000000000";
        let shape = |id: &str, kind: &str, x: f64, kids: &[&str]| {
            serde_json::json!({ "id": id, "name": id, "type": kind, "x": x, "y": 0,
                "width": 100, "height": 50, "shapes": kids,
                "selrect": { "x": x, "y": 0, "width": 100, "height": 50 } })
        };
        let file = |v: serde_json::Value| v.to_string().into_bytes();
        let zip = ondin_export::plan::zip(&[
            (
                "manifest.json".into(),
                file(serde_json::json!({ "files": [{ "id": "F", "name": "Brand kit" }] })),
            ),
            (
                "files/F.json".into(),
                file(serde_json::json!({ "id": "F" })),
            ),
            (
                "files/F/pages/P.json".into(),
                file(serde_json::json!({ "id": "P", "name": "Main", "index": 0 })),
            ),
            (
                format!("files/F/pages/P/{root}.json"),
                file(shape(root, "frame", 0.0, &["B"])),
            ),
            (
                "files/F/pages/P/B.json".into(),
                file(shape("B", "frame", 10.0, &[])),
            ),
        ])
        .unwrap();
        let path = f.outside.join("export.penpot");
        std::fs::write(&path, zip).unwrap();
        f.app.fill_import(&ctx, vec![path]);
        f.app.settle_import();
        let card = f.app.dash.import.as_ref().unwrap();
        let Field::One { source, .. } = &card.field else {
            panic!("the export reads");
        };
        assert_eq!(source.kind, Kind::Penpot);
        assert_eq!(card.options.name, "Brand kit", "the file's own name");
        f.app.start_copy(&ctx);
        f.app.settle_import();
        let Some(Step::Done(o)) = f.app.dash.import.as_ref().map(|c| &c.step) else {
            panic!("the card shows the overview");
        };
        assert_eq!(o.sub, "From export.penpot, which is unchanged");
        assert!(o.text().contains("1 board"), "{}", o.text());
        assert_eq!(listed(&f).len(), 1);
    }

    /// Penpot 1's binary export is refused with what to do about it.
    #[test]
    fn an_old_penpot_export_is_refused_with_a_way_forward() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "penpot-old");
        let old = f.outside.join("Old.penpot");
        std::fs::write(&old, [0x01, 0x0b, 0x32, 0x06, 0, 0]).unwrap();
        f.app.fill_import(&ctx, vec![old]);
        f.app.settle_import();
        let card = f.app.dash.import.as_ref().unwrap();
        assert!(
            matches!(&card.field, Field::Refused { why, .. } if why.contains("Penpot 2")),
            "{:?}",
            matches!(card.field, Field::Refused { .. })
        );
    }

    /// A drop of several Sketch files takes the first, alone, and says so.
    #[test]
    fn another_tools_files_come_in_one_at_a_time() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "sketch-many");
        let a = a_sketch_file(&f, "A.sketch");
        let b = a_sketch_file(&f, "B.sketch");
        f.app.fill_import(&ctx, vec![a.clone(), b]);
        let card = f.app.dash.import.as_ref().unwrap();
        assert!(matches!(&card.field, Field::Reading { path, .. } if *path == a));
        assert!(
            card.note
                .as_deref()
                .is_some_and(|n| n.contains("1 other file"))
        );
        f.app.settle_import();
    }

    /// A zip that is not a Sketch file is refused in the field.
    #[test]
    fn a_broken_sketch_file_is_refused() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "sketch-bad");
        let bad = f.outside.join("Broken.sketch");
        std::fs::write(&bad, b"not a zip").unwrap();
        f.app.fill_import(&ctx, vec![bad]);
        f.app.settle_import();
        let card = f.app.dash.import.as_ref().unwrap();
        assert!(matches!(&card.field, Field::Refused { why, .. } if why.contains("Sketch")));
    }

    /// Files by extension: documents, files placed rather than imported, and the
    /// rest.
    #[test]
    fn a_file_is_known_by_its_extension() {
        assert_eq!(Kind::of(Path::new("a/B.ONDIN")), Kind::Ondin);
        assert_eq!(Kind::of(Path::new("Site.sketch")), Kind::Sketch);
        assert_eq!(Kind::of(Path::new("Kit.penpot")), Kind::Penpot);
        assert_eq!(Kind::of(Path::new("poster.pdf")), Kind::Placed);
        assert_eq!(Kind::of(Path::new("logo.svg")), Kind::Placed);
        assert_eq!(Kind::of(Path::new("notes.txt")), Kind::Other);
    }
}
