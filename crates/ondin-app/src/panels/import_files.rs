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
use crate::panels::dashboard::Nav;
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
/// Where a [`ui::tick_box`]'s label starts from its left edge: its 14pt box and 7pt
/// gap, so a sentence under the label lines up with it.
const TICK_TEXT_X: f32 = 14.0 + 7.0;
/// The tallest the page list grows before it scrolls.
const PAGES_MAX_H: f32 = 150.0;
/// The tallest the report's rows grow before they scroll, so a big document's
/// summary keeps the card short.
const REPORT_MAX_H: f32 = 240.0;
/// Between two groups of options.
const GROUP_GAP: f32 = 14.0;

/// What a chosen file is, by its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Ondin,
    /// Sketch's own, which Lunacy saves too, read into a new document; the
    /// file itself is never written to.
    Sketch,
    /// Penpot 2's export, read into a new document; the file itself is never
    /// written to.
    Penpot,
    /// A Figma file read through Figma's API from a pasted link, never a file
    /// on disk.
    Figma,
    /// Figma's own `.fig` binary, which Ondin does not read: the field sends the
    /// user to the file's link instead.
    Fig,
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
            "fig" => Self::Fig,
            "pdf" | "ai" | "svg" | "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif"
            | "tiff" | "avif" => Self::Placed,
            _ => Self::Other,
        }
    }

    /// Whether the card imports this kind of file.
    pub(crate) fn imports(self) -> bool {
        matches!(self, Self::Ondin | Self::Sketch | Self::Penpot)
    }

    /// Whether this is a design tool's document, which a canvas refuses.
    pub(crate) fn is_document(self) -> bool {
        self.imports() || self == Self::Fig
    }

    /// What the message line calls a file of this kind.
    fn noun(self) -> &'static str {
        match self {
            Self::Ondin => "Ondin document",
            Self::Sketch => "Sketch document",
            Self::Penpot => "Penpot file",
            Self::Figma | Self::Fig => "Figma file",
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
            Self::Figma => Words {
                frames: ("frame", "frames"),
                components: "Components and variants",
                styles: "Styles",
                styles_off: "Off: each use keeps the values it shows.",
                variables: "Variables",
                variables_off: "Figma's API gives them to Enterprise plans only; the values they show come in.",
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
            Self::Ondin => ("ONDIN".into(), color::accent_400()),
            Self::Sketch => ("SKETCH".into(), color::by_theme(SKETCH, SKETCH_LIGHT)),
            Self::Penpot => ("PENPOT".into(), color::by_theme(PENPOT, PENPOT_LIGHT)),
            Self::Figma => ("LINK".into(), color::by_theme(FIGMA, FIGMA_LIGHT)),
            Self::Fig => ("FIG".into(), color::by_theme(FIGMA, FIGMA_LIGHT)),
            Self::Placed | Self::Other => (
                path.extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_ascii_uppercase)
                    .unwrap_or_else(|| "FILE".into()),
                theme::text::dim(),
            ),
        }
    }
}

/// Figma's badge colour, on a link and on a `.fig`; each badge ink has a deeper
/// twin that holds its contrast on the light theme's grounds.
const FIGMA: egui::Color32 = egui::Color32::from_rgb(0xc9, 0xa6, 0xe6);
const FIGMA_LIGHT: egui::Color32 = egui::Color32::from_rgb(0x78, 0x46, 0xaa);
const SKETCH: egui::Color32 = egui::Color32::from_rgb(0xe6, 0xb8, 0x66);
const SKETCH_LIGHT: egui::Color32 = egui::Color32::from_rgb(0x96, 0x64, 0x0a);
const PENPOT: egui::Color32 = egui::Color32::from_rgb(0x8f, 0xd1, 0xa8);
const PENPOT_LIGHT: egui::Color32 = egui::Color32::from_rgb(0x1e, 0x7d, 0x4b);

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
    /// A row per family that is not installed, by family, kept apart so the
    /// copy reports only the families its trimmed text still uses.
    missing_fonts: Vec<(String, ReportRow)>,
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
    /// A `.fig`: the notice that sends the user to its link.
    Fig(PathBuf),
    /// A pasted Figma link, waiting for *Fetch*.
    Link {
        url: String,
        key: String,
    },
    /// A Figma link being fetched and read.
    Fetching {
        url: String,
        key: String,
        job: Job<Result<Source, String>>,
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
    /// The Figma token, held for this card only unless `remember` is ticked.
    pub(crate) token: String,
    pub(crate) show_token: bool,
    /// Keep the token in the system keychain once a fetch succeeds with it.
    pub(crate) remember: bool,
    /// The keychain's remembered token, being read when the card opened.
    token_job: Option<Job<Option<String>>>,
    /// Whether the keychain holds the token, so a paste fetches at once and
    /// unticking *Remember* forgets it then and there.
    remembered: bool,
    /// The last keychain write asked for, storing or forgetting, on a worker
    /// since the store can block; a failure lands in `fetch_error`. Each write
    /// waits for the one before it, so a quick tick and untick end as asked.
    keychain: Option<Job<Result<(), String>>>,
    /// What the card asked of the keychain, in order, for the tests to read: a
    /// test build's keychain keeps nothing.
    #[cfg(test)]
    asked: Vec<KeychainAsk>,
    /// The writes in the order their workers made them, for the tests.
    #[cfg(test)]
    written: std::sync::Arc<std::sync::Mutex<Vec<KeychainAsk>>>,
    /// A cancelled copy's worker is still running: *Import* waits for it, since
    /// it may yet write under the name this copy would take.
    pub(crate) settling: bool,
    /// Why the last fetch failed, under the token row.
    pub(crate) fetch_error: Option<String>,
}

/// What the card asks of the keychain.
#[derive(Clone, Debug, PartialEq, Eq)]
enum KeychainAsk {
    Store,
    Forget,
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
            token: String::new(),
            show_token: false,
            remember: false,
            token_job: None,
            remembered: false,
            keychain: None,
            #[cfg(test)]
            asked: Vec::new(),
            #[cfg(test)]
            written: Default::default(),
            settling: false,
            fetch_error: None,
        }
    }

    /// Store the card's token in the keychain, or forget the one kept there, on
    /// a worker after the write before it, and note which is kept.
    fn ask_keychain(&mut self, ctx: &egui::Context, ask: KeychainAsk) {
        #[cfg(test)]
        self.asked.push(ask.clone());
        self.remembered = ask == KeychainAsk::Store;
        let token = self.token.trim().to_owned();
        let before = self.keychain.take();
        #[cfg(test)]
        let written = self.written.clone();
        self.keychain = Some(Job::spawn("figma-keychain", ctx, move || {
            // Each `security`/`secret-tool` run is a process of its own, so two
            // writes left to race could land in either order.
            let earlier = before.map_or(Ok(()), |job| {
                job.wait()
                    .unwrap_or_else(|| Err("The keychain stopped before it answered".to_owned()))
            });
            // A test build's store is slow, as a real one can be, so a write
            // after it that did not wait would land first.
            #[cfg(test)]
            {
                if ask == KeychainAsk::Store {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                written.lock().unwrap().push(ask.clone());
            }
            let now = match ask {
                KeychainAsk::Store => crate::keychain::store(&token),
                KeychainAsk::Forget => {
                    crate::keychain::forget();
                    Ok(())
                }
            };
            earlier.and(now)
        }));
    }

    /// *Remember the token*, ticked or unticked: kept at once once a fetch has
    /// worked with it, forgotten at once if it was kept, else decided when the
    /// next fetch works.
    fn toggle_remember(&mut self, ctx: &egui::Context) {
        self.remember = !self.remember;
        let fetched =
            matches!(&self.field, Field::One { source, .. } if source.kind == Kind::Figma);
        if !self.remember && self.remembered {
            self.ask_keychain(ctx, KeychainAsk::Forget);
        } else if self.remember && fetched && !self.remembered {
            self.ask_keychain(ctx, KeychainAsk::Store);
        }
    }

    /// Whether the field holds a Figma link, fetched or not, and so the token
    /// row shows.
    fn has_link(&self) -> bool {
        match &self.field {
            Field::Link { .. } | Field::Fetching { .. } => true,
            Field::One { source, .. } => source.kind == Kind::Figma,
            _ => false,
        }
    }

    /// Whether *Import* would do something.
    pub(crate) fn ready(&self) -> bool {
        if self.settling {
            return false;
        }
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
    if card.settling && matches!(card.field, Field::One { .. } | Field::Many(_)) {
        return "Finishing the cancelled import…".into();
    }
    match &card.field {
        // The field already asks for one.
        Field::Empty => String::new(),
        Field::Reading { .. } => "Reading the file…".into(),
        Field::Refused { .. } | Field::Fig(_) => "Not a file Ondin imports".into(),
        Field::Link { .. } => "Fetch the file to choose what comes over".into(),
        Field::Fetching { .. } => "Fetching from Figma…".into(),
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
        missing_fonts: Vec::new(),
        doc: Arc::new(doc),
    })
}

/// The most another tool's zip may inflate to, all its entries together; its
/// pictures are most of that.
const ZIP_LIMIT: u64 = 2 << 30;

/// The most one JSON entry may inflate to. Parsed into `serde_json::Value`,
/// JSON takes up to some twenty times its bytes (a two-byte `0,` is a 32-byte
/// value), so this, not [`ZIP_LIMIT`], bounds what a parse can ask for: about
/// 3 GiB at most. A real `document.json` or page is tens of megabytes.
const JSON_ENTRY_LIMIT: u64 = 128 << 20;

/// The most all of a zip's JSON entries may inflate to together: a reader keeps
/// every page it parsed until it builds, so sixteen entries each within
/// [`JSON_ENTRY_LIMIT`] would otherwise ask for tens of gigabytes.
const JSON_LIMIT: u64 = 256 << 20;

/// Another tool's zip, for `ondin_core::foreign::Archive`, and how much of
/// [`ZIP_LIMIT`] its entries have used.
struct Zip<'a> {
    zip: zip::ZipArchive<std::io::Cursor<&'a [u8]>>,
    read: u64,
    /// The most one `.json` entry may inflate to, [`JSON_ENTRY_LIMIT`].
    json_limit: u64,
    /// What is left of [`JSON_LIMIT`] for the `.json` entries still to read.
    json_left: u64,
}

impl ondin_core::foreign::Archive for Zip<'_> {
    fn names(&mut self) -> Vec<String> {
        self.zip.file_names().map(str::to_owned).collect()
    }

    /// `None` for an entry that would pass the limit: whole or not at all, since a
    /// cut entry would read as a broken one.
    fn entry(&mut self, name: &str) -> Option<Vec<u8>> {
        let entry = self.zip.by_name(name).ok()?;
        let mut room = ZIP_LIMIT.saturating_sub(self.read);
        let json = name.ends_with(".json");
        if json {
            room = room.min(self.json_limit).min(self.json_left);
        }
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
        if json {
            self.json_left -= out.len() as u64;
        }
        Some(out)
    }
}

/// Pictures inside another tool's file, or a script's `add_image`, hashed and sized
/// as a placed one is.
pub(crate) struct Pictures;

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

    fn draw(
        &mut self,
        width: u32,
        height: u32,
        art: &[(ondin_core::kurbo::BezPath, ondin_core::peniko::Color)],
    ) -> Option<(ondin_core::ImageId, ondin_core::ImageEntry)> {
        let png = crate::canvas::art_png(width, height, art)?;
        self.place(png, ondin_core::ImageFormat::Png)
    }
}

/// Another tool's file read into a document, with what did not map, the font
/// families its text asks for, and the name the file gives itself.
pub(crate) type Read = (
    ondin_core::Document,
    Vec<(Part, ReportRow)>,
    Vec<(String, usize)>,
    Option<String>,
);

/// Penpot's older binary export inflated, within [`ZIP_LIMIT`], and unpacked
/// into the entries its zip export would hold.
fn penpot_one(bytes: &[u8]) -> Result<ondin_core::penpot_v1::Entries, String> {
    use ondin_core::penpot_v1;
    if !penpot_v1::is_binfile(bytes) {
        return Err(ondin_core::penpot_in::PenpotError::OldFormat.to_string());
    }
    let unreadable = |e: &dyn std::fmt::Display| format!("Could not be read: {e}");
    let stream = ruzstd::decoding::StreamingDecoder::new(&bytes[penpot_v1::HEADER..])
        .map_err(|e| unreadable(&e))?;
    let mut body = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(stream, ZIP_LIMIT + 1), &mut body)
        .map_err(|e| unreadable(&e))?;
    if body.len() as u64 > ZIP_LIMIT {
        return Err(unreadable(&"it unpacks to more than 2 GiB"));
    }
    penpot_v1::unpack(&body).map_err(|e| unreadable(&e))
}

/// Another tool's file read in a child process (`crate::reader`), so a file that
/// crashes its reader costs the import and not the app.
fn read_foreign(kind: Kind, bytes: &[u8]) -> Result<Read, String> {
    let request = match kind {
        Kind::Penpot => crate::reader::Request::Penpot(bytes.to_vec()),
        _ => crate::reader::Request::Sketch(bytes.to_vec()),
    };
    crate::reader::foreign(request).map_err(crate::reader::Failure::message)
}

/// [`read_foreign`]'s work, in the reading child.
pub(crate) fn read_foreign_here(kind: Kind, bytes: &[u8]) -> Result<Read, String> {
    let mut archive: Box<dyn ondin_core::foreign::Archive> =
        if kind == Kind::Penpot && ondin_core::penpot_in::is_old_format(bytes) {
            Box::new(penpot_one(bytes)?)
        } else {
            let zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
                .map_err(|_| format!("Not a {}: it does not open as one", kind.noun()))?;
            Box::new(Zip {
                zip,
                read: 0,
                json_limit: JSON_ENTRY_LIMIT,
                json_left: JSON_LIMIT,
            })
        };
    // A fresh stream, as a new document's: nothing else holds ids under it.
    let actor = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64 | 1);
    let mut ids = ondin_core::IdSource::new(actor);
    let unreadable = |e: &dyn std::fmt::Display| format!("Could not be read: {e}");
    let (doc, report, families, name) = match kind {
        Kind::Penpot => {
            let r = ondin_core::penpot_in::read(&mut *archive, &mut ids, &mut Pictures)
                .map_err(|e| unreadable(&e))?;
            (r.doc, r.report, r.families, Some(r.name))
        }
        _ => {
            let r = ondin_core::sketch_in::read(&mut *archive, &mut ids, &mut Pictures)
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

/// A read that landed becomes the field's one document, its options set from it
/// and the families it asks for that are not installed added to its report — on
/// the frame, since the font list is the frame's.
fn adopt(
    fonts: &crate::fonts::FontService,
    card: &mut ImportCard,
    path: PathBuf,
    mut source: Source,
) {
    for (family, runs) in std::mem::take(&mut source.families) {
        if fonts.family_status(&family) == crate::fonts::FamilyStatus::Missing {
            let drawn_in = fonts
                .stand_in_for(&family)
                .unwrap_or_else(|| "Inter".to_string());
            source.missing_fonts.push((
                family.clone(),
                ReportRow {
                    what: family,
                    count: ui::count(runs, "run"),
                    how: format!("Not installed: drawn in {drawn_in} until it is"),
                },
            ));
        }
    }
    let o = &mut card.options;
    o.name.clone_from(&source.name);
    o.pages = vec![true; source.census.pages.len()];
    card.field = Field::One {
        path,
        source: Box::new(source),
    };
}

/// Where Figma's REST API lives.
const FIGMA_API: &str = "https://api.figma.com/v1";
/// The most a fetched file and its pictures may weigh together.
const FIGMA_LIMIT: u64 = 2 << 30;

/// A fetched Figma file and its pictures, for `ondin_core::figma_in`.
struct Fetched(std::collections::BTreeMap<String, Vec<u8>>);

impl ondin_core::foreign::Archive for Fetched {
    fn entry(&mut self, name: &str) -> Option<Vec<u8>> {
        self.0.get(name).cloned()
    }
}

/// Fetch the file at `key` with `token` and read it, on the worker: the file
/// with its geometry, then the address of each picture it fills with, then the
/// pictures. The token goes to Figma and nowhere else.
fn fetch_figma(key: &str, token: &str) -> Result<Source, String> {
    use std::time::Duration;
    // No test reaches Figma: the read is tested through `read_figma`.
    if cfg!(test) {
        return Err("A test build does not reach Figma".into());
    }
    let agent: ureq::Agent = ureq::config::Config::builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(120)))
        .timeout_global(Some(Duration::from_secs(600)))
        // A status is read here, since a 429's `Retry-After` says how long to wait.
        .http_status_as_error(false)
        .build()
        .new_agent();
    let mut used = 0u64;
    let mut get = |url: &str, with_token: bool| -> Result<Vec<u8>, String> {
        let mut req = agent.get(url);
        if with_token {
            req = req.header("X-Figma-Token", token);
        }
        let mut resp = req.call().map_err(fetch_failed)?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let wait = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse().ok());
            return Err(status_failed(status, wait));
        }
        let bytes = resp
            .body_mut()
            .with_config()
            .limit(FIGMA_LIMIT.saturating_sub(used))
            .read_to_vec()
            .map_err(|e| format!("Figma's answer did not arrive whole: {e}"))?;
        used += bytes.len() as u64;
        Ok(bytes)
    };
    let mut files = std::collections::BTreeMap::new();
    files.insert(
        "file.json".to_owned(),
        get(&format!("{FIGMA_API}/files/{key}?geometry=paths"), true)?,
    );
    // Without the addresses the file still comes in, its pictures reported missing.
    let addresses: serde_json::Value = get(&format!("{FIGMA_API}/files/{key}/images"), true)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    if let Some(images) = addresses
        .get("meta")
        .and_then(|m| m.get("images"))
        .and_then(|i| i.as_object())
    {
        for (reference, url) in images {
            // Pictures are served from storage that wants no token, and one that
            // does not arrive is reported by the reader, not fatal here.
            if let Some(url) = url.as_str()
                && let Ok(bytes) = get(url, false)
            {
                files.insert(format!("images/{reference}"), bytes);
            }
        }
    }
    read_figma(files, used)
}

/// What a request that got no answer says: Figma not being reached.
fn fetch_failed(e: ureq::Error) -> String {
    match e {
        ureq::Error::StatusCode(n) => status_failed(n, None),
        e => format!("Figma could not be reached: {e}"),
    }
}

/// What Figma's answer `status` says, by what it means; a 429 with how long its
/// `Retry-After` (`wait`, in seconds) asks, since a plan's file-read limit can
/// hold for days, and any other status as Figma's error.
fn status_failed(status: u16, wait: Option<u64>) -> String {
    match status {
        401 => "Figma did not accept the token: check it was copied whole".to_owned(),
        403 => "Figma refused the token: it needs read access to file content".to_owned(),
        404 => "Figma has no file at that link that this token can open".to_owned(),
        429 => {
            let wait = match wait {
                None | Some(0..=90) => "a minute".to_owned(),
                Some(s @ 91..=5400) => format!("{} minutes", s.div_ceil(60)),
                Some(s @ 5401..=259_200) => format!("{} hours", s.div_ceil(3600)),
                Some(s) => format!("{} days", s.div_ceil(86_400)),
            };
            format!(
                "Figma's limit on file reads for this account was reached: try Fetch again in {wait}"
            )
        }
        n => format!("Figma answered with an error ({n}): try again later, or a smaller file"),
    }
}

/// A fetched file, `bytes` long with its pictures, read into a document.
fn read_figma(
    files: std::collections::BTreeMap<String, Vec<u8>>,
    bytes: u64,
) -> Result<Source, String> {
    let (doc, report, families, name) =
        crate::reader::foreign(crate::reader::Request::Figma(files))
            .map_err(crate::reader::Failure::message)?;
    let census = ondin_core::take::census(&doc);
    Ok(Source {
        name: name.unwrap_or_else(|| "Figma file".to_owned()),
        census,
        kind: Kind::Figma,
        bytes,
        report,
        families,
        missing_fonts: Vec::new(),
        doc: Arc::new(doc),
    })
}

/// [`read_figma`]'s work, in the reading child.
pub(crate) fn read_figma_here(
    files: std::collections::BTreeMap<String, Vec<u8>>,
) -> Result<Read, String> {
    let actor = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64 | 1);
    let mut ids = ondin_core::IdSource::new(actor);
    let read = ondin_core::figma_in::read(&mut Fetched(files), &mut ids, &mut Pictures)
        .map_err(|e| format!("Could not be read: {e}"))?;
    let report = read
        .report
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
    let name = Some(read.name).filter(|n| !n.trim().is_empty());
    Ok((read.doc, report, read.families.into_iter().collect(), name))
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
/// left out, and each of `fonts` (a missing family's row, by family) whose family
/// the trimmed text still uses. Stops before writing once `cancel` is set.
#[allow(clippy::too_many_arguments)]
fn import_one(
    root: &Path,
    project: Option<&project::Project>,
    name: &str,
    doc: &ondin_core::Document,
    keep: &Keep,
    report: Vec<ReportRow>,
    fonts: Vec<(String, ReportRow)>,
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
    let used: std::collections::HashSet<String> = ondin_core::families::families_in_use(&doc)
        .into_iter()
        .map(|(family, _)| family.to_lowercase())
        .collect();
    landed.report.extend(
        fonts
            .into_iter()
            .filter(|(family, _)| used.contains(&family.to_lowercase()))
            .map(|(_, row)| row),
    );
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
        // The card names what it reads and asks what to call it.
        if crate::private::on() {
            self.session.info("Leave private mode to import");
            return;
        }
        let mut card = ImportCard::new(self.nav_project().map(|p| p.id));
        // The keychain can block (a locked macOS keychain asks), so off the frame.
        card.token_job = Some(Job::spawn(
            "figma-token",
            &self.egui_ctx,
            crate::keychain::read,
        ));
        self.dash.import = Some(card);
    }

    /// Put a pasted Figma link in the field, fetching at once when the token was
    /// remembered.
    fn fill_link(&mut self, ctx: &egui::Context, url: String, key: String) {
        let Some(card) = self.dash.import.as_mut() else {
            return;
        };
        card.typed.clear();
        card.note = None;
        card.fetch_error = None;
        card.field = Field::Link { url, key };
        if card.remembered && !card.token.trim().is_empty() {
            self.start_fetch(ctx);
        }
    }

    /// *Fetch*: read the linked file through Figma's API, on a worker.
    fn start_fetch(&mut self, ctx: &egui::Context) {
        let Some(card) = self.dash.import.as_mut() else {
            return;
        };
        let token = card.token.trim().to_owned();
        let Field::Link { url, key } = &card.field else {
            return;
        };
        if token.is_empty() {
            card.fetch_error = Some("Paste a personal access token first".into());
            return;
        }
        let (url, key) = (url.clone(), key.clone());
        let from = key.clone();
        card.fetch_error = None;
        card.field = Field::Fetching {
            job: Job::spawn("figma-fetch", ctx, move || fetch_figma(&from, &token)),
            url,
            key,
        };
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
            (0, None, Some(path), None) if Kind::of(path) == Kind::Fig => Field::Fig(path.clone()),
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
        if matches!(card.field, Field::Refused { .. }) {
            return;
        }
        // Each reason its own: a document already in the library is left out
        // for being there, not for being one of several files.
        let left = others.len() + tools.len();
        let mut why = Vec::new();
        if left > 0 {
            why.push(format!(
                "{left} other file{} left out: Ondin documents come in together, other files one at a time",
                if left == 1 { " was" } else { "s were" }
            ));
        }
        if !already.is_empty() {
            let n = already.len();
            why.push(format!(
                "{} already in the library {} left out",
                ui::count(n, "document"),
                if n == 1 { "was" } else { "were" }
            ));
        }
        if !why.is_empty() {
            card.note = Some(why.join(". "));
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
    pub(crate) fn poll_import(&mut self, ctx: &egui::Context) {
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
        let settling = !self.dash.import_abandoned.is_empty();
        let fonts = &self.fonts;
        let Some(card) = self.dash.import.as_mut() else {
            return;
        };
        card.settling = settling;
        let mut failed = None;
        let landed = card.keychain.as_ref().and_then(|job| match job.poll() {
            Ok(answer) => answer,
            Err(()) => Some(Err("The keychain stopped before it answered".to_owned())),
        });
        if let Some(answer) = landed {
            card.keychain = None;
            failed = answer.err();
        }
        if failed.is_some() {
            card.fetch_error = failed;
        }
        let mut fetch_now = false;
        if let Some(job) = &card.token_job {
            // A worker that died answered no token.
            let landed = match job.poll() {
                Ok(answer) => answer,
                Err(()) => Some(None),
            };
            if let Some(remembered) = landed {
                card.token_job = None;
                if let Some(token) = remembered.filter(|_| card.token.is_empty()) {
                    card.token = token;
                    card.remember = true;
                    card.remembered = true;
                    fetch_now = matches!(card.field, Field::Link { .. });
                }
            }
        }
        if let Field::Reading { path, job } = &card.field {
            match job.poll() {
                Ok(None) => {}
                Ok(Some(Ok(source))) => {
                    let path = path.clone();
                    adopt(fonts, card, path, source);
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
        if let Field::Fetching { url, key, job } = &card.field {
            let landed = match job.poll() {
                Ok(None) => None,
                Ok(Some(result)) => Some(result),
                Err(()) => Some(Err("The fetch stopped before it finished".to_owned())),
            };
            match landed {
                None => {}
                Some(Ok(source)) => {
                    card.fetch_error = None;
                    // Kept once it has worked, by the tick as it is now.
                    let path = PathBuf::from(url.clone());
                    adopt(fonts, card, path, source);
                    if card.remember {
                        card.ask_keychain(ctx, KeychainAsk::Store);
                    } else if card.remembered {
                        card.ask_keychain(ctx, KeychainAsk::Forget);
                    }
                }
                Some(Err(why)) => {
                    card.fetch_error = Some(why);
                    card.field = Field::Link {
                        url: url.clone(),
                        key: key.clone(),
                    };
                }
            }
        }
        if fetch_now {
            self.start_fetch(ctx);
        }
        let Some(card) = self.dash.import.as_mut() else {
            return;
        };
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
        let written = landed.written.clone();
        card.step = Step::Done(overview(&card.field, &card.options, into, landed));
        self.library.refresh();
        for path in &written {
            if let Some(id) = self.library.entry_at(path).and_then(|e| e.meta.id.clone()) {
                self.library.local.mark_imported(&id);
            }
        }
        self.library.local.save();
    }

    /// *Done* on the overview: the library shows where the copies went, the
    /// project they were filed into or *All files*, with the one copy selected.
    fn done_import(&mut self) {
        let Some(card) = self.dash.import.take() else {
            return;
        };
        let Step::Done(o) = &card.step else {
            return;
        };
        if o.written == 0 {
            return;
        }
        self.dash.nav = match card
            .options
            .project
            .as_deref()
            .and_then(|id| self.library.projects.destination(id))
        {
            Some(p) => Nav::Project(p.id.clone()),
            None => Nav::All,
        };
        self.dash.selected = o.open.clone();
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
                        Part::Styles => o.styles,
                        Part::Variables => o.variables,
                        Part::Layers => true,
                    })
                    .map(|(_, row)| row.clone())
                    .collect();
                let fonts = source.missing_fonts.clone();
                let name = o.name.trim().to_owned();
                let what = format!("Importing {name}…");
                let job = Job::spawn("import-file", ctx, move || {
                    import_one(
                        &root,
                        project.as_ref(),
                        &name,
                        &doc,
                        &keep,
                        report,
                        fonts,
                        &flag,
                    )
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
            card.settling = true;
        }
    }

    /// Close the card, giving up a copy in progress.
    fn close_import(&mut self) {
        self.cancel_import();
        self.dash.import = None;
    }

    /// Draw the card, if it is open.
    pub(crate) fn import_modal(&mut self, ctx: &egui::Context) {
        self.poll_import(ctx);
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
        if act.is_none() && ui::card_enter(ctx) {
            act = match self.dash.import.as_ref().map(|c| (&c.step, &c.field)) {
                Some((Step::Form, Field::Link { .. })) => Some(CardAct::Fetch),
                Some((Step::Form, _)) => Some(CardAct::Import),
                Some((Step::Done(o), _)) if o.open.is_some() => Some(CardAct::Open),
                _ => None,
            };
        }
        match act {
            None => {}
            Some(CardAct::Close) => self.close_import(),
            Some(CardAct::Done) => self.done_import(),
            Some(CardAct::Cancel) => self.cancel_import(),
            Some(CardAct::Browse) => self.browse_for_import(ctx),
            Some(CardAct::Fill(path)) => self.fill_import(ctx, vec![path]),
            Some(CardAct::Link(url, key)) => self.fill_link(ctx, url, key),
            Some(CardAct::Fetch) => self.start_fetch(ctx),
            Some(CardAct::Remember) => {
                if let Some(card) = self.dash.import.as_mut() {
                    card.toggle_remember(ctx);
                }
            }
            Some(CardAct::PasteLink) => {
                let pasted = crate::app::system_clipboard_text().unwrap_or_default();
                match ondin_core::figma_in::file_key(&pasted) {
                    Some(key) => self.fill_link(ctx, pasted.trim().to_owned(), key),
                    None => {
                        if let Some(card) = self.dash.import.as_mut() {
                            card.note =
                                Some("The clipboard holds no Figma link. In Figma, use Share › Copy link.".into());
                        }
                    }
                }
            }
            Some(CardAct::Clear) => {
                if let Some(card) = self.dash.import.as_mut() {
                    card.field = Field::Empty;
                    card.note = None;
                    card.fetch_error = None;
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
    /// A Figma link and its file key, typed or pasted.
    Link(String, String),
    Fetch,
    /// *Remember the token*, ticked or unticked.
    Remember,
    /// *Paste link* on a `.fig`'s notice: the clipboard's link into the field.
    PasteLink,
    Clear,
    Import,
    Copy,
    Open,
    /// *Done* on the overview.
    Done,
}

/// What the field says about a file it will not take.
fn refusal(kind: Kind) -> &'static str {
    match kind {
        Kind::Placed => {
            "Ondin places a PDF, an SVG or a picture onto a canvas rather than importing it. Open a document and use Place image."
        }
        Kind::Ondin | Kind::Sketch | Kind::Penpot | Kind::Figma | Kind::Fig | Kind::Other => {
            "Ondin imports Ondin, Sketch and Penpot files and Figma links, and cannot read this kind of file."
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
        (Field::One { source, .. }, _) if source.kind == Kind::Figma => (
            format!("{} is in {place}", o.name.trim()),
            "Read from Figma, which is unchanged".to_owned(),
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
    super::dashboard::field_label(ui, "Ondin, Sketch, Penpot file, or Figma link");
    file_field(ui, card, act);
    if let Some(note) = &card.note {
        message(ui, icon::INFO, note, theme::text::dim());
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
                true,
            ) {
                card.options.leave_pictures = !card.options.leave_pictures;
            }
        }
        _ => {}
    }
    ui.add_space(settings::FOOTER_GAP - 6.0);
    let busy = matches!(card.field, Field::Reading { .. } | Field::Fetching { .. });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if busy {
            ui.add(
                egui::Spinner::new()
                    .size(ui::SMALL_PT)
                    .color(theme::text::faint()),
            );
        }
        ui.label(
            egui::RichText::new(foot)
                .size(settings::CAPTION_PT)
                .color(theme::text::faint()),
        );
        let state = if ready {
            FieldButton::Primary
        } else {
            FieldButton::Disabled
        };
        match settings::footer_row(ui, &[("Cancel", FieldButton::Off), ("Import", state)]) {
            Some(0) => *act = Some(CardAct::Close),
            Some(_) => *act = Some(CardAct::Import),
            None => {}
        }
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
            Field::One { source, path } if source.kind == Kind::Figma => {
                Some((Kind::Figma.badge(path), path.to_string_lossy().into_owned()))
            }
            Field::Link { url, .. } | Field::Fetching { url, .. } => {
                Some((Kind::Figma.badge(Path::new(url)), url.clone()))
            }
            Field::Reading { path, .. }
            | Field::One { path, .. }
            | Field::Refused { path, .. }
            | Field::Fig(path) => Some((
                Kind::of(path).badge(path),
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            )),
            Field::Many(paths) => Some((
                Kind::Ondin.badge(&paths[0]),
                format!("{} documents", paths.len()),
            )),
        };
        match shown {
            None => {
                let before = card.typed.chars().count();
                let resp = ui::text_field(
                    ui,
                    egui::vec2(w, ui::CONTROL_H),
                    &mut card.typed,
                    "Choose a file, or paste a Figma link",
                );
                let entered = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let grew = card.typed.chars().count().saturating_sub(before);
                if resp.changed() || entered {
                    if let Some(key) = typed_link(&card.typed, grew, entered) {
                        *act = Some(CardAct::Link(card.typed.trim().to_owned(), key));
                    } else if let Some(path) = typed_path(&card.typed) {
                        *act = Some(CardAct::Fill(path));
                    }
                }
            }
            Some(((badge, tint), name)) => {
                ui::field_row(ui, egui::vec2(w, ui::CONTROL_H), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    badge_chip(ui, &badge, tint);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(name)
                                .size(ui::CONTROL_PT)
                                .color(color::text()),
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
            egui::vec2(browse_w, ui::CONTROL_H),
            FieldButton::Off,
        )
        .clicked()
        {
            *act = Some(CardAct::Browse);
        }
    });
    match &card.field {
        Field::Empty => {}
        Field::Reading { .. } => message(ui, icon::DOCUMENT, "Reading…", theme::text::faint()),
        Field::One { source, .. } if source.kind == Kind::Figma => message(
            ui,
            icon::CHECK_CIRCLE,
            &format!(
                "{} · {} · fetched just now",
                source.name,
                ui::count(source.census.pages.len(), "page")
            ),
            color::saved(),
        ),
        Field::Link { .. } | Field::Fetching { .. } => message(
            ui,
            icon::GLOBE,
            "A Figma link, read through Figma's API with your token.",
            theme::text::faint(),
        ),
        Field::Fig(_) => fig_notice(ui, act),
        Field::One { path, source } => {
            let folder = path
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            message(
                ui,
                icon::DOCUMENT,
                &format!(
                    "{} · {folder} · {}",
                    source.kind.noun(),
                    settings::human_bytes(source.bytes)
                ),
                theme::text::faint(),
            );
        }
        Field::Many(paths) => {
            let names: Vec<String> = paths
                .iter()
                .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                .collect();
            message(ui, icon::FILES, &names.join(", "), theme::text::faint());
        }
        Field::Refused { why, .. } => message(ui, icon::INFO, why, color::warn()),
    }
    if card.has_link() {
        token_row(ui, card, act);
    }
}

/// The notice a `.fig` gets: Ondin reads Figma by link, and *Paste link* puts the
/// clipboard's link in the field.
fn fig_notice(ui: &mut egui::Ui, act: &mut Option<CardAct>) {
    ui.add_space(10.0);
    egui::Frame::NONE
        .fill(color::warn().gamma_multiply(0.11))
        .stroke(egui::Stroke::new(1.0, color::warn().gamma_multiply(0.3)))
        .corner_radius(8)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 9.0;
                ui.label(theme::icon_text(icon::INFO, 15.0, color::warn()));
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.label(
                        egui::RichText::new("Figma files come in by link")
                            .size(ui::CONTROL_PT)
                            .color(theme::text::strong()),
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                "Ondin doesn't read .fig files. It reads the file through Figma's \
                                 API instead. In Figma, use Share › Copy link and paste the link here.",
                            )
                            .size(ui::SMALL_PT)
                            .color(theme::text::muted()),
                        )
                        .wrap(),
                    );
                    ui.add_space(6.0);
                    if settings::text_button(ui, "Paste link", FieldButton::Off).clicked() {
                        *act = Some(CardAct::PasteLink);
                    }
                });
            });
        });
}

/// The personal access token, *Fetch*, and whether to keep the token.
fn token_row(ui: &mut egui::Ui, card: &mut ImportCard, act: &mut Option<CardAct>) {
    ui.add_space(GROUP_GAP);
    super::dashboard::field_label(ui, "Personal access token");
    // Right to left, so *Fetch* is sized to its word and the field takes the rest.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Lit only while there is a link to fetch, not once its file is in.
            let linked = matches!(card.field, Field::Link { .. });
            let state = FieldButton::enabled_if(linked && !card.token.trim().is_empty());
            if settings::text_button(ui, "Fetch", state).clicked() && state.is_enabled() {
                *act = Some(CardAct::Fetch);
            }
            let w = ui.available_width();
            ui::field_row(ui, egui::vec2(w, ui::CONTROL_H), |ui| {
                let eye_w = 26.0;
                let font = egui::FontId::proportional(ui::CONTROL_PT);
                let pad = ui::edit_frame(ui, &font, ui::CONTROL_H - ui::FIELD_BORDER_H, 0.0);
                #[expect(
                    clippy::disallowed_methods,
                    reason = "the entry inside a composed field row, which paints the ground and ui::typing_edge itself"
                )]
                let _ = pad.add(
                    ui,
                    ui.available_width() - eye_w,
                    egui::TextEdit::singleline(&mut card.token)
                        .password(!card.show_token)
                        .font(font)
                        .hint_text(egui::RichText::new("figd_…").size(ui::CONTROL_PT)),
                );
                let glyph = if card.show_token {
                    icon::EYE_SLASH
                } else {
                    icon::EYE
                };
                if ui::icon_button(ui, glyph, 22.0, 14.0, false, true)
                    .on_hover_text(if card.show_token {
                        "Hide token"
                    } else {
                        "Show token"
                    })
                    .clicked()
                {
                    card.show_token = !card.show_token;
                }
            });
        });
    });
    settings::caption(
        ui,
        "Made in Figma's account settings, with read access to file content. Sent to Figma and nowhere else.",
    );
    ui.add_space(8.0);
    if check_row(
        ui,
        "Remember the token on this computer",
        "",
        "In the system keychain, never in a document",
        card.remember,
        true,
    ) {
        *act = Some(CardAct::Remember);
    }
    if let Some(why) = &card.fetch_error {
        message(ui, icon::INFO, why, color::warn());
    }
}

/// The file key of a Figma link in the field, taken when it arrived by paste
/// (`grew` by more than a character at once) or with Enter: a link typed by hand
/// names a key at its first character, and taking it there would cut it short.
fn typed_link(typed: &str, grew: usize, entered: bool) -> Option<String> {
    (grew > 1 || entered)
        .then(|| ondin_core::figma_in::file_key(typed))
        .flatten()
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
        egui::FontId::proportional(ui::CAPTION_PT),
        tint,
    );
}

/// The line under the field: a glyph and a sentence, wrapped to the card.
fn message(ui: &mut egui::Ui, glyph: &str, text: &str, col: egui::Color32) {
    // The sentence rides 1 px above the glyph, which centres its ink on it.
    ui.add_space(5.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.vertical(|ui| {
            ui.add_space(1.0);
            ui.label(theme::icon_text(glyph, 12.0, col));
        });
        ui.add(egui::Label::new(egui::RichText::new(text).size(ui::SMALL_PT).color(col)).wrap());
    });
}

/// The hairline between the field and the options.
fn divider(ui: &mut egui::Ui) {
    ui.add_space(GROUP_GAP + 2.0);
    ui::card_rule(ui);
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
        None => ("○", theme::text::faint()),
    };
    job.append(
        glyph,
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(ui::CAPTION_PT),
            color: col,
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    job.append(
        name,
        7.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(ui::CONTROL_PT),
            color: color::text(),
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
                let meta = ui::count_as(page.frames, words.frames.0, words.frames.1);
                if check_row(ui, &page.name, &meta, "", *on, true) {
                    *on = !*on;
                }
            }
        });
}

/// What to bring over besides the layers, each saying what leaving it does. A
/// part the file has none of offers no choice.
fn bring_over(ui: &mut egui::Ui, census: &Census, words: Words, o: &mut Options) {
    ui.add_space(GROUP_GAP);
    group_label(ui, "Bring over");
    if check_row(
        ui,
        words.components,
        &census.components.to_string(),
        "Off: instances become plain layers.",
        o.components,
        census.components > 0,
    ) {
        o.components = !o.components;
    }
    if check_row(
        ui,
        words.styles,
        &census.styles.to_string(),
        words.styles_off,
        o.styles,
        census.styles > 0,
    ) {
        o.styles = !o.styles;
    }
    let vars = if census.modes > 1 {
        format!("{} · {} modes", census.variables, census.modes)
    } else {
        census.variables.to_string()
    };
    if check_row(
        ui,
        words.variables,
        &vars,
        words.variables_off,
        o.variables,
        census.variables > 0,
    ) {
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
        count > 0,
    ) {
        o.leave_pictures = !o.leave_pictures;
    }
}

/// A [`ui::tick_box`], a count against the right margin and, when given, a
/// sentence under the label. Answers whether it was clicked; a row that is not
/// `enabled` is dimmed and never is.
fn check_row(
    ui: &mut egui::Ui,
    label: &str,
    meta: &str,
    sub: &str,
    on: bool,
    enabled: bool,
) -> bool {
    let clicked = ui::disable_unless(ui, enabled, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut ticked = on;
        let w = ui.available_width();
        let row = ui.allocate_ui_with_layout(
            egui::vec2(w, ui::MENU_ROW_H),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| ui::tick_box(ui, label, "", &mut ticked).clicked(),
        );
        let clicked = row.inner;
        ui.painter().text(
            egui::pos2(row.response.rect.left() + w, row.response.rect.center().y),
            egui::Align2::RIGHT_CENTER,
            meta,
            egui::FontId::proportional(ui::SMALL_PT),
            theme::text::faint(),
        );
        if !sub.is_empty() {
            ui.horizontal(|ui| {
                // Under the label: the tick box's side and its gap.
                ui.add_space(TICK_TEXT_X);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(sub)
                            .size(settings::CAPTION_PT)
                            .color(theme::text::faint()),
                    )
                    .wrap(),
                );
            });
        }
        clicked
    })
    .inner;
    enabled && clicked
}

/// The copy in progress, in place of the options.
fn loading(ui: &mut egui::Ui, what: &str) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.add(
            egui::Spinner::new()
                .size(ui::TITLE_PT)
                .color(theme::text::dim()),
        );
        ui.label(
            egui::RichText::new(what)
                .size(ui::CONTROL_PT)
                .color(theme::text::strong()),
        );
    });
}

/// The overview: what came in, counted, what did not, and the way into it.
fn done(ui: &mut egui::Ui, o: &Overview, act: &mut Option<CardAct>) {
    let ok = o.written > 0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::hover());
        let tint = if ok { color::saved() } else { color::warn() };
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
                    .size(ui::TITLE_PT)
                    .color(theme::text::strong()),
            );
            ui.label(
                egui::RichText::new(&o.sub)
                    .size(ui::SMALL_PT)
                    .color(theme::text::dim()),
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
                    egui::FontId::proportional(ui::HEADING_PT),
                    theme::text::strong(),
                );
                p.text(
                    rect.left_bottom() + egui::vec2(11.0, -9.0),
                    egui::Align2::LEFT_BOTTOM,
                    if n == 1 { one } else { many },
                    egui::FontId::proportional(ui::SMALL_PT),
                    theme::text::dim(),
                );
            }
        });
        ui.add_space(gap);
    }
    ui.add_space(8.0);
    report(ui, &o.report);
    ui.add_space(settings::FOOTER_GAP - 6.0);
    ui.horizontal(|ui| {
        if ui::icon_button(ui, icon::COPY, 26.0, 13.0, false, true)
            .on_hover_text("Copy report")
            .clicked()
        {
            *act = Some(CardAct::Copy);
        }
        let mut buttons = vec![("Done", FieldButton::Off)];
        if o.open.is_some() {
            buttons.push(("Open document", FieldButton::Primary));
        }
        match settings::footer_row(ui, &buttons) {
            Some(0) => *act = Some(CardAct::Done),
            Some(_) => *act = Some(CardAct::Open),
            None => {}
        }
    });
}

/// The list of what did not come in as it was, under its count, on the count
/// cards' ground.
fn report(ui: &mut egui::Ui, rows: &[ReportRow]) {
    let head = if rows.is_empty() {
        "Nothing to report".to_owned()
    } else {
        format!("Not brought over as it was · {}", rows.len())
    };
    egui::Frame::NONE
        .fill(color::text_a(15))
        .corner_radius(8)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        egui::RichText::new(head)
                            .size(ui::SMALL_PT)
                            .color(theme::text::muted()),
                    );
                });
            if rows.is_empty() {
                return;
            }
            egui::ScrollArea::vertical()
                .id_salt("import-report")
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .max_height(REPORT_MAX_H)
                .auto_shrink([false, true])
                .show(ui, |ui| report_rows(ui, rows));
        });
}

/// The report's rows, each under a hairline.
fn report_rows(ui: &mut egui::Ui, rows: &[ReportRow]) {
    ui.spacing_mut().item_spacing.y = 0.0;
    for r in rows {
        let (line, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::empty());
        ui.painter().hline(
            line.x_range(),
            line.center().y,
            egui::Stroke::new(1.0, color::text_a(13)),
        );
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(&r.what)
                            .size(ui::CONTROL_PT)
                            .color(theme::text::strong()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(&r.count)
                                .size(ui::SMALL_PT)
                                .color(theme::text::faint()),
                        );
                    });
                });
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(&r.how)
                        .size(ui::SMALL_PT)
                        .color(theme::text::dim()),
                );
            });
    }
}

#[cfg(test)]
impl OndinApp {
    /// Wait for every worker the card has running, then land their answers as a
    /// frame would.
    pub(crate) fn settle_import(&mut self) {
        self.wait_import();
        self.poll_import(&egui::Context::default());
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
                Field::Fetching { url, key, job } => Field::Fetching {
                    job: Job::done(job.wait().unwrap_or_else(|| Err("no worker".into()))),
                    url,
                    key,
                },
                other => other,
            };
            if let Some(job) = card.token_job.take() {
                card.token_job = Some(Job::done(job.wait().flatten()));
            }
            card.keychain = card
                .keychain
                .take()
                .map(|job| Job::done(job.wait().unwrap_or(Ok(()))));
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
            ground: Some(ondin_core::peniko::Color::WHITE),
            draft: false,
            modes: Vec::new(),
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

    /// *Done* shows where an unfiled copy went, *All files*, with the copy
    /// selected and marked new until it is opened.
    /// flip: `done_import` leaving `nav` alone → fails at "All files".
    #[test]
    fn done_shows_the_copy_marked_new_until_opened() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "done");
        let source = two_pages(&mut f, "Fresh");
        f.app.dash.nav = Nav::Recent;
        assert_eq!(f.app.import_through_card(&ctx, vec![source]), 1);
        f.app.done_import();
        assert!(f.app.dash.import.is_none(), "the card closes");
        assert_eq!(f.app.dash.nav, Nav::All, "All files");
        let copy = f.app.dash.selected.clone().expect("the copy is selected");
        let id = f
            .app
            .library
            .entry_at(&copy)
            .and_then(|e| e.meta.id.clone())
            .expect("the copy is listed");
        assert!(f.app.library.local.is_new(&id), "new");
        f.app.begin_open(&copy);
        f.app.settle_open();
        assert!(!f.app.library.local.is_new(&id), "opened, no longer new");
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

    /// Enter on a focused tick box is that box's click, not *Import*: tabbed onto
    /// a page's tick, Enter unticks it and files nothing.
    /// flip: `import_modal` reading a raw `key_pressed(Enter)` → fails at "the card
    /// stays on its form".
    #[test]
    fn enter_on_a_focused_option_does_not_import() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut f = fixture(&ctx, "enter");
        let source = two_pages(&mut f, "Doc");
        f.app.fill_import(&ctx, vec![source]);
        f.app.settle_import();
        let (runs, _) = drawn(&mut f, &ctx);
        let archive = runs
            .iter()
            .find(|(_, t)| t == "Archive")
            .expect("the second page's row is drawn")
            .0
            .center()
            .y;
        let frame = |f: &mut Fixture, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 1000.0),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| f.app.import_modal(ui.ctx()));
        };
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let on_tick = |ctx: &egui::Context| {
            let id = ctx.memory(|m| m.focused());
            id.and_then(|id| ctx.read_response(id))
                .is_some_and(|r| r.rect.y_range().contains(archive))
        };
        for _ in 0..30 {
            if on_tick(&ctx) {
                break;
            }
            frame(&mut f, vec![key(egui::Key::Tab)]);
            frame(&mut f, Vec::new());
        }
        assert!(on_tick(&ctx), "the fixture: Tab reached the Archive tick");
        frame(&mut f, vec![key(egui::Key::Enter)]);
        frame(&mut f, Vec::new());
        f.app.settle_import();
        assert!(
            matches!(
                f.app.dash.import.as_ref().map(|c| &c.step),
                Some(Step::Form)
            ),
            "the card stays on its form"
        );
        assert_eq!(
            f.app.dash.import.as_ref().map(|c| c.options.pages.clone()),
            Some(vec![true, false]),
            "the tick took the click"
        );
        assert!(listed(&f).is_empty(), "files nothing");
    }

    /// `Tab` reaches the form's footer the way it reads: *Cancel*, then *Import*.
    /// flip: `form` laying its buttons in a `right_to_left` row → fails at "left to right".
    #[test]
    fn tab_walks_the_import_footer_left_to_right() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut f = fixture(&ctx, "tab");
        let source = two_pages(&mut f, "Doc");
        f.app.fill_import(&ctx, vec![source]);
        f.app.settle_import();
        let frame = |f: &mut Fixture, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 1000.0),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| f.app.import_modal(ui.ctx()));
            ctx.memory(|m| m.focused())
                .and_then(|id| ctx.read_response(id))
                .map(|r| r.rect)
        };
        let tab = || {
            vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]
        };
        frame(&mut f, Vec::new());
        let mut stops: Vec<egui::Rect> = Vec::new();
        for _ in 0..40 {
            frame(&mut f, tab());
            if let Some(at) = frame(&mut f, Vec::new())
                && !stops.contains(&at)
            {
                stops.push(at);
            }
        }
        let foot = stops.iter().map(|r| r.center().y).fold(f32::MIN, f32::max);
        let footer: Vec<_> = stops
            .iter()
            .filter(|r| (r.center().y - foot).abs() < 1.0)
            .collect();
        assert_eq!(
            footer.len(),
            2,
            "the fixture: both buttons are tab stops: {stops:?}"
        );
        assert!(
            footer[0].center().x < footer[1].center().x,
            "left to right: {footer:?}"
        );
    }

    /// A file already in the library is refused in the field rather than copied
    /// again. flip: drop the `in_library` partition in `fill_import` → fails at
    /// the refusal.
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
        f.app.fill_import(&ctx, vec![two.clone(), two.clone()]);
        check(&mut f, &["2 documents", "Leave out their pictures"]);
        let fig = f.outside.join("Checkout.fig");
        std::fs::write(&fig, b"fig").unwrap();
        f.app.fill_import(&ctx, vec![fig]);
        check(&mut f, &["Figma files come in by link", "Paste link"]);
        f.app.fill_link(
            &ctx,
            "https://www.figma.com/design/AbC123/A-rather-long-file-name-for-a-link".into(),
            "AbC123".into(),
        );
        check(
            &mut f,
            &[
                "Ondin, Sketch, Penpot file, or Figma link",
                "Personal access token",
                "Fetch",
                "Remember the token on this computer",
            ],
        );
        f.app.dash.import.as_mut().unwrap().field = Field::Empty;
        f.app.fill_import(&ctx, vec![two.clone(), two.clone()]);
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

    /// Rows about the styles and about the fonts follow what the copy keeps: a
    /// repeated style name goes with the styles, and a missing family only a
    /// page left out uses goes with that page. flip: keep `Part::Styles` rows
    /// whatever the choice in `start_copy` → fails at the trimmed report.
    #[test]
    fn style_and_font_rows_follow_what_the_copy_keeps() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "sketch-styles");
        // The font sources landed, so a family nothing lists is missing.
        f.app.fonts.settle_for_test();
        let frame = serde_json::json!({ "x": 0, "y": 0, "width": 10, "height": 10 });
        let text = serde_json::json!({ "_class": "text", "do_objectID": "T", "name": "Label",
            "frame": frame, "attributedString": { "string": "Hi", "attributes": [{
                "location": 0, "length": 2, "attributes": {
                    "MSAttributedStringFontAttribute": { "attributes": {
                        "name": "NoSuchFamilyAnywhere-Regular", "size": 12 } } } }] } });
        let rect = serde_json::json!({ "_class": "rectangle", "do_objectID": "R", "name": "Box",
            "frame": frame });
        let style = serde_json::json!({ "fills": [{ "_class": "fill", "isEnabled": true,
            "fillType": 0, "color": { "red": 1, "green": 0, "blue": 0, "alpha": 1 } }] });
        let document = serde_json::json!({ "_class": "document",
            "pages": [{ "_ref": "pages/A" }, { "_ref": "pages/B" }],
            "layerStyles": { "objects": [
                { "do_objectID": "S1", "name": "Red", "value": style },
                { "do_objectID": "S2", "name": "Red", "value": style },
            ] } });
        let page = |id: &str, layer: &serde_json::Value| serde_json::json!({ "_class": "page", "do_objectID": id, "name": id, "layers": [layer] });
        let zip = ondin_export::plan::zip(&[
            ("document.json".into(), document.to_string().into_bytes()),
            (
                "pages/A.json".into(),
                page("A", &text).to_string().into_bytes(),
            ),
            (
                "pages/B.json".into(),
                page("B", &rect).to_string().into_bytes(),
            ),
        ])
        .unwrap();
        let path = f.outside.join("Styles.sketch");
        std::fs::write(&path, zip).unwrap();
        let report = |f: &mut Fixture, styles: bool, pages: [bool; 2]| {
            f.app.dash.import = None;
            f.app.fill_import(&ctx, vec![path.clone()]);
            f.app.settle_import();
            let card = f.app.dash.import.as_mut().unwrap();
            card.options.styles = styles;
            card.options.pages = pages.to_vec();
            f.app.start_copy(&ctx);
            f.app.settle_import();
            match f.app.dash.import.as_ref().map(|c| &c.step) {
                Some(Step::Done(o)) => o.report.iter().map(|r| r.what.clone()).collect::<Vec<_>>(),
                _ => panic!("the card shows the overview"),
            }
        };
        assert_eq!(
            report(&mut f, true, [true, true]),
            ["Style names used twice", "No Such Family Anywhere"],
            "control: both rows are there"
        );
        assert!(report(&mut f, false, [false, true]).is_empty());
    }

    /// A forget asked right after a store is written after it, so a quick tick
    /// and untick leave nothing kept. flip: drop the wait on the write before
    /// in `ask_keychain` → fails at the order.
    #[test]
    fn keychain_writes_land_in_the_order_asked() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "figma-keychain-order");
        f.app.open_import_card();
        f.app.settle_import();
        let card = f.app.dash.import.as_mut().unwrap();
        card.token = "figd_x".into();
        card.ask_keychain(&ctx, KeychainAsk::Store);
        card.ask_keychain(&ctx, KeychainAsk::Forget);
        assert_eq!(card.keychain.take().and_then(Job::wait), Some(Ok(())));
        assert_eq!(
            *card.written.lock().unwrap(),
            [KeychainAsk::Store, KeychainAsk::Forget]
        );
    }

    /// *Remember* acts when it changes: a fetch that worked stores the token
    /// while it is ticked, unticking forgets it at once, and unticking during a
    /// fetch stores nothing when it lands. flip: skip the forget in
    /// `toggle_remember` → fails at the untick.
    #[test]
    fn remember_acts_on_the_tick_as_it_is() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "figma-remember");
        f.app.open_import_card();
        f.app.settle_import();
        let fetched = |f: &mut Fixture| {
            let card = f.app.dash.import.as_mut().unwrap();
            card.token = "figd_x".into();
            card.field = Field::Fetching {
                url: "https://www.figma.com/design/K1/F".into(),
                key: "K1".into(),
                job: Job::done(read_figma(a_fetched_figma_file(), 10)),
            };
        };
        let asked = |f: &Fixture| f.app.dash.import.as_ref().unwrap().asked.clone();
        f.app.dash.import.as_mut().unwrap().remember = true;
        fetched(&mut f);
        f.app.settle_import();
        assert_eq!(asked(&f), [KeychainAsk::Store], "kept once it worked");
        f.app.dash.import.as_mut().unwrap().toggle_remember(&ctx);
        assert_eq!(asked(&f), [KeychainAsk::Store, KeychainAsk::Forget]);
        f.app.dash.import.as_mut().unwrap().toggle_remember(&ctx);
        assert_eq!(
            asked(&f),
            [KeychainAsk::Store, KeychainAsk::Forget, KeychainAsk::Store],
            "ticked again after the fetch, kept again"
        );
        f.app.settle_import();
        // Unticked while a fetch runs, with nothing kept: the landing keeps nothing.
        f.app.dash.import = None;
        f.app.open_import_card();
        f.app.settle_import();
        f.app.dash.import.as_mut().unwrap().remember = true;
        fetched(&mut f);
        f.app.dash.import.as_mut().unwrap().toggle_remember(&ctx);
        f.app.settle_import();
        assert!(asked(&f).is_empty(), "{:?}", asked(&f));
    }

    /// *Import* waits while a cancelled copy's worker may still write, so a new
    /// copy cannot take the name the old one is writing under; once it answers,
    /// the copy goes ahead. flip: drop the `settling` test in `ready` → fails at
    /// the step, the copy started at once.
    #[test]
    fn a_new_copy_waits_for_a_cancelled_one() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "settling");
        let source = two_pages(&mut f, "Twice");
        f.app.fill_import(&ctx, vec![source]);
        f.app.settle_import();
        f.app.start_copy(&ctx);
        f.app.cancel_import();
        f.app.start_copy(&ctx);
        let card = f.app.dash.import.as_ref().unwrap();
        assert!(matches!(card.step, Step::Form), "the copy waits");
        assert_eq!(
            foot_text(card, &f.app.library.projects),
            "Finishing the cancelled import…"
        );
        f.app.settle_import();
        assert!(listed(&f).is_empty(), "the cancelled copy is gone");
        f.app.start_copy(&ctx);
        f.app.settle_import();
        assert!(matches!(
            f.app.dash.import.as_ref().map(|c| &c.step),
            Some(Step::Done(_))
        ));
        assert_eq!(listed(&f).len(), 1);
    }

    /// A document already in the library, dropped with another, is said to be
    /// left out for being there. flip: count it among the other files in
    /// `fill_import` → fails at the note.
    #[test]
    fn a_library_document_in_a_drop_is_named_as_already_there() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "already-mixed");
        let outside = two_pages(&mut f, "Outside");
        let mut doc = Document::new(f.app.session.ids.mint());
        let inside = store::file_document(&f.root, None, "Inside", &mut doc).unwrap();
        f.app.fill_import(&ctx, vec![outside, inside]);
        let note = f.app.dash.import.as_ref().unwrap().note.clone();
        assert_eq!(
            note.as_deref(),
            Some("1 document already in the library was left out")
        );
        f.app.settle_import();
    }

    /// A link typed by hand is not taken at its key's first character, only on
    /// Enter; a paste is taken at once. flip: take the key on every change in
    /// `typed_link` → fails at the first prefix.
    #[test]
    fn a_typed_link_waits_for_enter() {
        let link = "https://www.figma.com/design/AbC123xyz/Checkout";
        for end in 1..link.len() {
            assert_eq!(typed_link(&link[..end], 1, false), None, "{}", &link[..end]);
        }
        assert_eq!(typed_link(link, 1, true).as_deref(), Some("AbC123xyz"));
        assert_eq!(
            typed_link(link, link.len(), false).as_deref(),
            Some("AbC123xyz")
        );
    }

    /// Figma's answers are worded by what they mean: any status is Figma's own
    /// error, not Figma being out of reach.
    #[test]
    fn a_status_from_figma_is_not_called_unreachable() {
        let said = |n| fetch_failed(ureq::Error::StatusCode(n));
        assert!(said(403).contains("read access"));
        assert!(said(401).contains("did not accept"));
        assert_eq!(
            said(500),
            "Figma answered with an error (500): try again later, or a smaller file"
        );
        assert!(fetch_failed(ureq::Error::ConnectionFailed).contains("could not be reached"));
    }

    /// A 429 says how long Figma asks to wait, read from `Retry-After`, since a
    /// plan's limit can hold for days. flip: `status_failed` ignoring `wait` →
    /// fails at the hours.
    #[test]
    fn a_rate_limit_says_how_long_to_wait() {
        let waits = |s| status_failed(429, s);
        assert!(waits(None).ends_with("in a minute"));
        assert!(waits(Some(600)).ends_with("in 10 minutes"));
        assert!(waits(Some(181_924)).ends_with("in 51 hours"));
        assert!(waits(Some(400_000)).ends_with("in 5 days"));
    }

    /// A JSON entry past its own limit is not read, whatever room the archive
    /// has left, since a parse costs many times its bytes; a picture is.
    /// flip: drop the `.json` limit in `Zip::entry` → fails at the JSON entry.
    #[test]
    fn a_json_entry_past_its_limit_is_not_read() {
        let bytes = ondin_export::plan::zip(&[
            ("pages/P.json".into(), vec![b' '; 100]),
            ("images/p.png".into(), vec![0; 100]),
        ])
        .unwrap();
        let mut zip = Zip {
            zip: zip::ZipArchive::new(std::io::Cursor::new(&bytes[..])).unwrap(),
            read: 0,
            json_limit: 64,
            json_left: JSON_LIMIT,
        };
        use ondin_core::foreign::Archive as _;
        assert_eq!(zip.entry("pages/P.json"), None);
        assert_eq!(zip.entry("images/p.png").map(|b| b.len()), Some(100));
    }

    /// JSON entries each within their own limit are not read once together
    /// they pass the file's. flip: leave `json_left` unspent in `Zip::entry` →
    /// fails at the third page.
    #[test]
    fn json_entries_share_the_files_limit() {
        let bytes = ondin_export::plan::zip(&[
            ("pages/A.json".into(), vec![b' '; 100]),
            ("pages/B.json".into(), vec![b' '; 100]),
            ("pages/C.json".into(), vec![b' '; 100]),
        ])
        .unwrap();
        let mut zip = Zip {
            zip: zip::ZipArchive::new(std::io::Cursor::new(&bytes[..])).unwrap(),
            read: 0,
            json_limit: 100,
            json_left: 250,
        };
        use ondin_core::foreign::Archive as _;
        assert!(zip.entry("pages/A.json").is_some());
        assert!(zip.entry("pages/B.json").is_some());
        assert_eq!(zip.entry("pages/C.json"), None);
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

    /// Penpot's binary export is inflated and unpacked before Penpot's reader
    /// sees it: one whose stream holds no file reaches the reader and is
    /// refused for having no pages, not for its format.
    /// flip: inflate from the start of the file in `penpot_one` → fails at the
    /// message.
    #[test]
    fn a_binary_penpot_export_is_inflated_for_the_reader() {
        let label = [
            &[0xef, 0xe3, 11][..],
            b"clj/keyword",
            &[2, 0xf7, 0xe3, 11],
            b"v1/metadata",
        ]
        .concat();
        let empty_map = [&[0xef, 0xe3, 7][..], b"clj/map", &[1, 0xe4]].concat();
        let mut body = vec![4, 5];
        body.extend((label.len() as u64).to_be_bytes());
        body.extend(&label);
        body.push(5);
        body.extend((empty_map.len() as u64).to_be_bytes());
        body.extend(&empty_map);
        let mut file = vec![1];
        file.extend(0x0B1A_8650_63A1_5FC5u64.to_be_bytes());
        file.extend(1u64.to_be_bytes());
        file.extend(ruzstd::encoding::compress_to_vec(
            &body[..],
            ruzstd::encoding::CompressionLevel::Fastest,
        ));
        let Err(why) = read_foreign(Kind::Penpot, &file) else {
            panic!("a file of no pages reads");
        };
        assert_eq!(why, "Could not be read: the file has no pages");
    }

    /// A non-zip `.penpot` that is not the binary export Ondin reads either is
    /// refused with what to do about it.
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
            matches!(&card.field, Field::Refused { why, .. } if why.contains("export it again")),
            "{:?}",
            matches!(card.field, Field::Refused { .. })
        );
    }

    /// A Figma file as Figma's API sends it: one page holding one frame.
    fn a_fetched_figma_file() -> std::collections::BTreeMap<String, Vec<u8>> {
        let file = serde_json::json!({
            "name": "Checkout flow",
            "document": { "id": "0:0", "type": "DOCUMENT", "children": [{
                "id": "0:1", "name": "Flows", "type": "CANVAS", "children": [{
                    "id": "1:2", "name": "Cart", "type": "FRAME",
                    "relativeTransform": [[1, 0, 0], [0, 1, 0]], "size": { "x": 375, "y": 812 },
                    "transitionNodeID": "1:9",
                }],
            }]},
        });
        [("file.json".to_owned(), file.to_string().into_bytes())].into()
    }

    /// A pasted Figma link waits for *Fetch*, with the token row under it and
    /// *Import* off.
    #[test]
    fn a_pasted_figma_link_waits_for_fetch() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "figma-link");
        f.app.open_import_card();
        let link = "https://www.figma.com/design/AbC123/Checkout-flow?node-id=0-1".to_owned();
        f.app.fill_link(&ctx, link, "AbC123".into());
        let card = f.app.dash.import.as_ref().unwrap();
        assert!(matches!(&card.field, Field::Link { key, .. } if key == "AbC123"));
        assert!(card.has_link(), "the token row shows");
        assert!(!card.ready());
        assert_eq!(
            foot_text(card, &f.app.library.projects),
            "Fetch the file to choose what comes over"
        );
        f.app.start_fetch(&ctx);
        let card = f.app.dash.import.as_ref().unwrap();
        assert!(
            matches!(card.field, Field::Link { .. }),
            "no token, so no fetch"
        );
        assert!(
            card.fetch_error
                .as_deref()
                .is_some_and(|e| e.contains("token"))
        );
    }

    /// A fetched file lands as one document in Figma's words, named after the
    /// file, and comes in through the card. flip: land a fetch through
    /// `Field::Refused` in `poll_import` → fails at the kind.
    #[test]
    fn a_fetched_figma_file_comes_in_through_the_card() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "figma-fetched");
        f.app.open_import_card();
        let source = read_figma(a_fetched_figma_file(), 100).expect("the file reads");
        let url = "https://www.figma.com/design/AbC123/Checkout-flow".to_owned();
        f.app.dash.import.as_mut().unwrap().field = Field::Fetching {
            url,
            key: "AbC123".into(),
            job: Job::done(Ok(source)),
        };
        f.app.settle_import();
        let card = f.app.dash.import.as_ref().unwrap();
        let Field::One { source, .. } = &card.field else {
            panic!("the fetch landed");
        };
        assert_eq!(source.kind, Kind::Figma);
        assert_eq!(card.options.name, "Checkout flow");
        f.app.start_copy(&ctx);
        f.app.settle_import();
        let Some(Step::Done(o)) = f.app.dash.import.as_ref().map(|c| &c.step) else {
            panic!("the card shows the overview");
        };
        assert_eq!(o.title, "Checkout flow is in the library");
        assert_eq!(o.sub, "Read from Figma, which is unchanged");
        assert!(o.report.iter().any(|r| r.what == "Prototype interactions"));
        assert_eq!(listed(&f).len(), 1);
    }

    /// A `.fig` gets the notice that sends the user to its link, and *Import*
    /// stays off.
    #[test]
    fn a_fig_file_is_sent_to_its_link() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "fig");
        let fig = f.outside.join("Checkout flow.fig");
        std::fs::write(&fig, b"fig-kiwi").unwrap();
        f.app.fill_import(&ctx, vec![fig]);
        let card = f.app.dash.import.as_ref().unwrap();
        assert!(matches!(card.field, Field::Fig(_)));
        assert!(!card.ready());
    }

    /// A remembered token fetches as soon as a link is pasted.
    #[test]
    fn a_remembered_token_fetches_on_paste() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "figma-remembered");
        f.app.open_import_card();
        f.app.settle_import();
        let card = f.app.dash.import.as_mut().unwrap();
        assert!(card.token.is_empty(), "a test build remembers no token");
        card.token = "figd_x".into();
        card.remembered = true;
        f.app.fill_link(
            &ctx,
            "https://www.figma.com/design/K1/F".into(),
            "K1".into(),
        );
        assert!(matches!(
            f.app.dash.import.as_ref().unwrap().field,
            Field::Fetching { .. }
        ));
        // A test build's fetch fails at once, which puts the link back.
        f.app.settle_import();
        let card = f.app.dash.import.as_ref().unwrap();
        assert!(matches!(card.field, Field::Link { .. }));
        assert_eq!(
            card.fetch_error.as_deref(),
            Some("A test build does not reach Figma")
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
        assert_eq!(Kind::of(Path::new("logo.ai")), Kind::Placed);
        assert_eq!(Kind::of(Path::new("logo.svg")), Kind::Placed);
        assert_eq!(Kind::of(Path::new("notes.txt")), Kind::Other);
    }
}
