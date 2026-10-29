//! Files imported by a drop or the *Place image* dialog, read off the frame. A
//! worker reads the bytes, decodes pictures and builds SVG and PDF layers, minting
//! ids from an actor of its own; the frame loop lands the result in the tab the
//! files were given to, once that tab is in front and no drag or pen run is live.
//! A tab closed first takes its imports with it, unsaid: a tab behind is never
//! written to while it waits.

use crate::app::{ImageRefusal, LoadedImage, OndinApp};
use crate::canvas::{Decoded, PdfRead, SvgImport};
use crate::job::Job;
use crate::session::StatusKind;
use crate::tabs::TabId;
use ondin_core::kurbo::{Point, Vec2};
use ondin_core::{IdSource, NodeId};
use std::path::PathBuf;
use std::sync::Arc;

/// A file given to an import: its name, and its bytes or where to read them.
pub(crate) struct Incoming {
    pub(crate) name: String,
    pub(crate) bytes: Option<Arc<[u8]>>,
    pub(crate) path: Option<PathBuf>,
}

/// Which door the files came through, which decides how they land.
#[derive(Clone, Copy)]
pub(crate) enum Door {
    /// Dropped at `at` (world), `alone` when the drop was one file, which a picture
    /// needs to replace the fill under it.
    Drop { at: Option<Point>, alone: bool },
    /// Picked in *Place image*: pictures arm the Image tool, vector art lands where
    /// a paste would.
    Place,
}

/// One import: the worker reading it, then what it read, waiting for its tab.
pub(crate) struct Import {
    tab: Option<TabId>,
    door: Door,
    job: Option<Job<Vec<Read>>>,
    read: Option<Vec<Read>>,
}

/// One file as the worker left it.
enum Read {
    Picture(LoadedImage, ondin_core::peniko::ImageData),
    Svg {
        name: String,
        out: ondin_core::svg_in::Import,
        images: Decoded,
    },
    Pdf {
        name: String,
        read: PdfRead,
        images: Decoded,
    },
    /// What to tell the user about a file that did not read.
    Refused(String),
}

/// What the status line says while files are read.
fn importing(files: &[Incoming]) -> String {
    match files {
        [one] => format!("Importing {}…", one.name),
        many => format!("Importing {} files…", many.len()),
    }
}

/// Read every file, on the worker. Layers hang under `parent`, a stand-in id the
/// landing replaces with the parent it picks.
fn read_all(files: Vec<Incoming>, mut ids: IdSource, parent: NodeId) -> Vec<Read> {
    files
        .into_iter()
        .map(|file| read_one(file, &mut ids, parent))
        .collect()
}

/// One file: a PDF or SVG by its bytes becomes layers, anything else a picture.
fn read_one(file: Incoming, ids: &mut IdSource, parent: NodeId) -> Read {
    let name = file.name;
    let bytes = match (file.bytes, &file.path) {
        (Some(b), _) => Some(b.to_vec()),
        (None, Some(p)) => std::fs::read(p).ok(),
        (None, None) => None,
    };
    let Some(bytes) = bytes else {
        return Read::Refused(ImageRefusal::NotAnImage.message(&name));
    };
    let stem = std::path::Path::new(&name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.clone());
    let mut images = Decoded::default();
    // Vector art becomes layers, never a rasterized picture.
    let (kind, outcome) = if ondin_export::pdf_in::looks_like_pdf(&bytes) {
        let read = crate::canvas::read_pdf(bytes, ids, parent, &stem, &mut images);
        (
            "PDF",
            read.map(|read| Read::Pdf {
                name: name.clone(),
                read,
                images,
            }),
        )
    } else if crate::app::looks_like_svg(&bytes) {
        let read = String::from_utf8(bytes)
            .map_err(|_| SvgImport::Unreadable)
            .and_then(|text| crate::canvas::read_svg(&text, ids, parent, Some(&stem), &mut images));
        (
            "SVG",
            read.map(|out| Read::Svg {
                name: name.clone(),
                out,
                images,
            }),
        )
    } else {
        let ext = file.path.as_deref().and_then(crate::app::file_extension);
        return match crate::app::read_picture(bytes, ext, Some(name.clone())) {
            Ok((image, pixels)) => Read::Picture(image, pixels),
            Err(why) => Read::Refused(why.message(&name)),
        };
    };
    outcome.unwrap_or_else(|why| {
        Read::Refused(match why {
            SvgImport::TooDeep => format!("{name} is nested too deeply to read"),
            SvgImport::Locked => format!("{name} is password-protected"),
            SvgImport::Placed(_) | SvgImport::Unreadable => {
                format!("{name} could not be read as {kind}")
            }
        })
    })
}

impl OndinApp {
    /// Read `files` on a worker for the tab in front, saying so meanwhile;
    /// [`Self::poll_imports`] lands them.
    pub(crate) fn start_import(&mut self, ctx: &egui::Context, files: Vec<Incoming>, door: Door) {
        if files.is_empty() {
            return;
        }
        self.session.info(importing(&files));
        let mut ids = IdSource::new(crate::session::random_actor());
        let parent = ids.mint();
        let job = Job::spawn("ondin-import", ctx, move || read_all(files, ids, parent));
        self.imports.push(Import {
            tab: self.tabs.active,
            door,
            job: Some(job),
            read: None,
        });
    }

    /// Land every import read for the tab in front, oldest first; drop those whose
    /// tab has closed. Held while a drag or a pen run is live, which would write
    /// back a document from before the import when it ends, and while a modal is
    /// up, which may be asking about the document as it stands.
    pub(crate) fn poll_imports(&mut self) {
        if self.imports.is_empty()
            || !self.drag.is_none()
            || self.pen.is_some()
            || self.modal_is_up()
        {
            return;
        }
        for import in &mut self.imports {
            if let Some(job) = &import.job {
                match job.poll() {
                    Ok(None) => {}
                    Ok(Some(read)) => {
                        import.read = Some(read);
                        import.job = None;
                    }
                    // Said where it was asked, as a read that refused.
                    Err(()) => {
                        import.read = Some(vec![Read::Refused(
                            "Could not import: the reader stopped".to_string(),
                        )]);
                        import.job = None;
                    }
                }
            }
        }
        self.land_ready();
    }

    /// Land the imports that have been read and whose tab is in front. One asked of
    /// the scratch session no tab shows is dropped once a tab is active, as that
    /// session is gone.
    fn land_ready(&mut self) {
        let tabs = &self.tabs;
        self.imports.retain(|i| match i.tab {
            Some(t) => tabs.get(t).is_some(),
            None => tabs.active.is_none(),
        });
        let active = self.tabs.active;
        let (ready, waiting) = std::mem::take(&mut self.imports)
            .into_iter()
            .partition(|i| i.read.is_some() && i.tab == active);
        self.imports = waiting;
        for import in ready {
            if let Some(read) = import.read {
                self.land_import(import.door, read);
            }
        }
    }

    /// Wait for every import's worker, then poll as a frame would (tests only).
    #[cfg(test)]
    pub(crate) fn settle_imports(&mut self) {
        for import in &mut self.imports {
            if let Some(job) = import.job.take() {
                import.read = Some(job.wait().unwrap_or_default());
            }
        }
        self.poll_imports();
    }

    /// Place what one import read, as its door places it, and report it in one
    /// status: the pictures first, then a line per vector file; refusals on their
    /// own.
    fn land_import(&mut self, door: Door, reads: Vec<Read>) {
        let (mut pictures, mut vector, mut refused) = (Vec::new(), Vec::new(), Vec::new());
        for read in reads {
            match read {
                Read::Picture(image, pixels) => {
                    self.canvas.images_mut().adopt(&image.id, pixels);
                    pictures.push(image);
                }
                Read::Svg { name, out, images } => {
                    self.adopt_decoded(images);
                    vector.push((name, Vector::Svg(out)));
                }
                Read::Pdf { name, read, images } => {
                    self.adopt_decoded(images);
                    vector.push((name, Vector::Pdf(read)));
                }
                Read::Refused(why) => refused.push(why),
            }
        }
        if !refused.is_empty() {
            self.session.fail(refused.join("; "));
        }
        let reported = !pictures.is_empty();
        let mut selected = Vec::new();
        let at = match door {
            Door::Drop { at, alone } => {
                if reported {
                    self.drop_images_at(pictures, at, alone);
                    selected.extend_from_slice(self.session.selection.ids());
                }
                at
            }
            Door::Place => {
                let n = pictures.len();
                if n > 0 {
                    self.pending_images = pictures;
                    self.choose_tool(crate::tools::Tool::Image);
                    self.session.info(match n {
                        1 => "Click to place the image".to_string(),
                        n => format!("Click to place {n} images"),
                    });
                }
                None
            }
        };
        self.place_vectors(vector, at, selected, reported);
    }

    /// Place read SVG and PDF files as layers, cascaded from `at` as dropped
    /// pictures are, and select them beside `selected` (what the same import placed
    /// already). Each file says what it made, after the pictures' report when
    /// `reported` says one was written.
    fn place_vectors(
        &mut self,
        files: Vec<(String, Vector)>,
        at: Option<Point>,
        mut selected: Vec<NodeId>,
        reported: bool,
    ) {
        if files.is_empty() {
            return;
        }
        let (mut said, mut failed) = (Vec::new(), Vec::new());
        for (i, (name, file)) in files.into_iter().enumerate() {
            let step = crate::app::DROP_CASCADE * i as f64;
            let at = at.map(|p| p + Vec2::new(step, step));
            let (kind, outcome) = match file {
                Vector::Svg(out) => ("SVG", self.place_svg(out, at)),
                Vector::Pdf(read) => ("PDF", self.place_pdf(read, at)),
            };
            match outcome {
                SvgImport::Placed(made) => {
                    said.push(format!("Imported {name} — {made}"));
                    selected.extend_from_slice(self.session.selection.ids());
                }
                _ => failed.push(format!("{name} could not be read as {kind}")),
            }
        }
        if !failed.is_empty() {
            self.session.fail(failed.join("; "));
        }
        if said.is_empty() {
            return;
        }
        // The pictures' report goes first; an error joins on its own.
        let shown = self.session.status();
        let placed = (reported && shown.kind == StatusKind::Info)
            .then(|| shown.text.clone())
            .filter(|t| !t.is_empty());
        said.splice(0..0, placed);
        self.session.selection.set(selected);
        self.session.info(said.join(crate::session::STATUS_JOIN));
    }
}

/// A vector file read and waiting to be placed.
enum Vector {
    Svg(ondin_core::svg_in::Import),
    Pdf(PdfRead),
}
