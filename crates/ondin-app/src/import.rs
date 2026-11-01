//! Files imported by a drop, *Place image* or *Replace image*, and a pasted picture or
//! SVG markup, read off the frame. A worker reads the bytes, decodes pictures and
//! builds SVG and PDF layers, minting ids from an actor of its own; the frame loop
//! lands the result in the tab it was given to, once that tab is in front and no
//! drag or pen run is live. A tab closed first takes its imports with it, unsaid: a
//! tab behind is never written to while it waits.

use crate::app::{ImageRefusal, LoadedImage, OndinApp};
use crate::canvas::{Decoded, PdfRead, SvgImport};
use crate::job::Job;
use crate::panels::PaintSlot;
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
#[derive(Clone)]
pub(crate) enum Door {
    /// Dropped at `at` (world), `alone` when the drop was one file, which a picture
    /// needs to replace the fill under it.
    Drop { at: Option<Point>, alone: bool },
    /// Picked in *Place image*: pictures arm the Image tool, vector art lands where
    /// a paste would.
    Place,
    /// Pasted, at *Paste here*'s point or, with `None`, aimed by the selection.
    Paste { at: Option<Point> },
    /// Picked in *Replace image*, for the picture in `slot` of `id`, which held
    /// `was` when asked: a slot changed meanwhile is not written to.
    Replace {
        id: NodeId,
        slot: PaintSlot,
        keep_framing: bool,
        was: Option<Box<ondin_core::Brush>>,
    },
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
    /// Pasted markup that would not read as SVG, pasted as text instead.
    Text(String),
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
fn read_all(files: Vec<Incoming>, ids: &mut IdSource, parent: NodeId) -> Vec<Read> {
    files
        .into_iter()
        .map(|file| read_one(file, ids, parent))
        .collect()
}

/// One file: a PDF or SVG by its bytes becomes layers, anything else a picture.
fn read_one(file: Incoming, ids: &mut IdSource, parent: NodeId) -> Read {
    let name = file.name;
    // Another tool's document is a new library document, never layers in this one.
    if crate::panels::import_files::Kind::of(std::path::Path::new(&name)).is_document() {
        return Read::Refused(format!(
            "{name} is a document: bring it in with Import files in the library"
        ));
    }
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
    } else if is_illustrator(&name) {
        // An `.ai` saved with PDF compatibility took the arm above; the rest are
        // Illustrator's private format, which only a re-save can open.
        return Read::Refused(format!(
            "{name} was saved without PDF compatibility: re-save it from Illustrator with Create PDF Compatible File"
        ));
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

/// Whether a file is named as Illustrator's, `.ai`.
fn is_illustrator(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("ai"))
}

impl OndinApp {
    /// Read `files` on a worker for the tab in front, saying so meanwhile;
    /// [`Self::poll_imports`] lands them.
    pub(crate) fn start_import(&mut self, ctx: &egui::Context, files: Vec<Incoming>, door: Door) {
        if files.is_empty() {
            return;
        }
        let said = importing(&files);
        self.start_reading(ctx, said, door, move |ids, parent| {
            read_all(files, ids, parent)
        });
    }

    /// Read a picture picked in *Replace image* on a worker, for the picture in
    /// `slot` of `id`; anything but a picture is refused, as the dialog's filter asks.
    pub(crate) fn start_replace_image(
        &mut self,
        ctx: &egui::Context,
        path: PathBuf,
        id: NodeId,
        slot: PaintSlot,
        keep_framing: bool,
    ) {
        let name = crate::app::image_name(&path);
        let door = Door::Replace {
            id,
            slot,
            keep_framing,
            was: self.slot_brush(id, slot).map(Box::new),
        };
        self.start_reading(ctx, format!("Importing {name}…"), door, move |_, _| {
            // An unreadable file and a non-picture share one message.
            let read = std::fs::read(&path)
                .map_err(|_| ImageRefusal::NotAnImage)
                .and_then(|bytes| {
                    let ext = crate::app::file_extension(&path);
                    crate::app::read_picture(bytes, ext, Some(name.clone()))
                });
            vec![match read {
                Ok((image, pixels)) => Read::Picture(image, pixels),
                Err(why) => Read::Refused(why.message(&name)),
            }]
        });
    }

    /// Encode and decode a clipboard picture (`rgba`, `size` pixels) on a worker,
    /// pasted at `at` (`OndinApp::paste_image`).
    pub(crate) fn start_paste_image(
        &mut self,
        ctx: &egui::Context,
        rgba: Vec<u8>,
        size: (u32, u32),
        at: Option<Point>,
    ) {
        let said = "Pasting image…".to_string();
        self.start_reading(ctx, said, Door::Paste { at }, move |_, _| {
            let read = crate::app::rgba_to_png(&rgba, size)
                .and_then(|png| crate::app::read_picture(png, None, None).ok());
            vec![match read {
                Some((image, pixels)) => Read::Picture(image, pixels),
                // Our own fresh PNG refused is a bug, said plainly rather than in detail.
                None => Read::Refused("The pasted picture could not be read".to_string()),
            }]
        });
    }

    /// Read pasted SVG markup on a worker, pasted at `at` (`OndinApp::paste_svg`):
    /// markup that will not read lands as text, as the paste's last arm would have.
    pub(crate) fn start_paste_svg(&mut self, ctx: &egui::Context, text: String, at: Option<Point>) {
        let said = "Pasting SVG…".to_string();
        self.start_reading(ctx, said, Door::Paste { at }, move |ids, parent| {
            let mut images = Decoded::default();
            vec![
                match crate::canvas::read_svg(&text, ids, parent, None, &mut images) {
                    Ok(out) => Read::Svg {
                        name: String::new(),
                        out,
                        images,
                    },
                    Err(SvgImport::TooDeep) => Read::Refused(
                        "That SVG is nested too deeply to read — nothing was pasted.".to_string(),
                    ),
                    Err(_) => Read::Text(text),
                },
            ]
        });
    }

    /// Whether a drag, a pen run or a pencil stroke is live: each writes back a
    /// document from before a landing when it ends, or loses its stroke to the tool
    /// change a landing makes, so work that lands waits for it.
    pub(crate) fn gesture_live(&self) -> bool {
        !self.drag.is_none() || self.pen.is_some() || self.pencil.is_some()
    }

    /// Run `work` on a worker for the tab in front, the status saying `said`
    /// meanwhile. Layers it builds hang under the stand-in parent it is given, minted
    /// from its own `IdSource`; the landing replaces the parent.
    fn start_reading(
        &mut self,
        ctx: &egui::Context,
        said: String,
        door: Door,
        work: impl FnOnce(&mut IdSource, NodeId) -> Vec<Read> + Send + 'static,
    ) {
        self.session.info(said);
        let mut ids = IdSource::new(crate::session::random_actor());
        let parent = ids.mint();
        // A worker's engine holds only Inter: an SVG naming the document's
        // families reads them as available only once they are registered there.
        let fonts = self.session.resolved.text_fonts();
        let job = Job::spawn("ondin-import", ctx, move || {
            ondin_core::text::register_font_data(&fonts);
            work(&mut ids, parent)
        });
        self.imports.push(Import {
            tab: self.tabs.active,
            door,
            job: Some(job),
            read: None,
        });
    }

    /// Land every import read for the tab in front, oldest first; drop those whose
    /// tab has closed. Held while a gesture is live ([`Self::gesture_live`]) and
    /// while a modal is up, which may be asking about the document as it stands.
    pub(crate) fn poll_imports(&mut self) {
        if self.imports.is_empty() || self.gesture_live() || self.modal_is_up() {
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
        // In the order asked: one still reading holds those asked after it.
        let mut held = false;
        let (ready, waiting) = std::mem::take(&mut self.imports)
            .into_iter()
            .partition(|i| {
                if i.tab != active {
                    return false;
                }
                held |= i.read.is_none();
                !held
            });
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
        let mut text = None;
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
                Read::Text(t) => text = Some(t),
            }
        }
        if !refused.is_empty() {
            self.session.fail(refused.join("; "));
        }
        let reported = !pictures.is_empty();
        match door {
            Door::Drop { at, alone } => {
                let mut selected = Vec::new();
                if reported {
                    self.drop_images_at(pictures, at, alone);
                    selected.extend_from_slice(self.session.selection.ids());
                }
                self.place_vectors(vector, at, selected, reported);
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
                self.place_vectors(vector, None, Vec::new(), reported);
            }
            Door::Paste { at } => {
                for image in &pictures {
                    self.land_pasted_image(image, at);
                }
                for (_, file) in vector {
                    if let Vector::Svg(out) = file
                        && let SvgImport::Placed(said) = self.place_svg(out, at)
                    {
                        self.session.info(format!("Pasted SVG — {said}"));
                    }
                }
                if let Some(text) = text {
                    self.paste_text(&text, at);
                }
            }
            Door::Replace {
                id,
                slot,
                keep_framing,
                was,
            } => {
                if let Some(image) = pictures.first() {
                    if was.is_some() && self.slot_brush(id, slot).map(Box::new) == was {
                        self.replace_image_with(id, slot, keep_framing, image);
                    } else {
                        self.session.fail(format!(
                            "{} was not placed: the fill it was replacing changed while it read",
                            image.label()
                        ));
                    }
                }
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::{Brush, NodeKind};

    /// A document dropped on a canvas is refused with a pointer to the library,
    /// whoever's it is, rather than read as a picture that fails.
    #[test]
    fn a_document_dropped_on_a_canvas_points_to_the_library() {
        let mut ids = IdSource::new(1);
        let parent = ids.mint();
        for name in [
            "Marketing.sketch",
            "Onboarding.ondin",
            "Kit.penpot",
            "Checkout.fig",
        ] {
            let file = Incoming {
                name: name.into(),
                bytes: Some(Arc::from(&b"PK\x03\x04"[..])),
                path: None,
            };
            let Read::Refused(why) = read_one(file, &mut ids, parent) else {
                panic!("{name} is refused");
            };
            assert!(why.contains("Import files"), "{why}");
        }
    }

    /// An `.ai` is read as the PDF its bytes are; one without a PDF inside is
    /// refused with the re-save that fixes it, not as a picture that failed.
    ///
    /// flip: `read_one` without its `is_illustrator` arm → fails at "re-save".
    #[test]
    fn an_illustrator_file_reads_as_its_pdf_or_asks_for_a_re_save() {
        let mut ids = IdSource::new(1);
        let parent = ids.mint();
        let read = |bytes: &'static [u8], ids: &mut IdSource| {
            let file = Incoming {
                name: "Logo.AI".into(),
                bytes: Some(Arc::from(bytes)),
                path: None,
            };
            match read_one(file, ids, parent) {
                Read::Refused(why) => why,
                _ => panic!("neither fixture is a PDF that parses"),
            }
        };
        let pdf = read(b"%PDF-1.4 not really", &mut ids);
        assert_eq!(pdf, "Logo.AI could not be read as PDF", "took the PDF arm");
        let ps = read(b"%!PS-Adobe-3.0\n%%Creator: Adobe Illustrator", &mut ids);
        assert!(ps.contains("re-save"), "{ps}");
    }

    /// The nodes on the document's first page.
    fn page_nodes(app: &OndinApp) -> Vec<&ondin_core::Node> {
        let doc = &app.session.doc;
        ondin_core::subtree_nodes(doc, &[doc.first_page()])
            .into_iter()
            .filter_map(|id| doc.get(id))
            .collect()
    }

    /// An SVG pasted with a turned frame selected lands inside it, centred on the
    /// frame's centre: its box measured alone stands in the frame's own space.
    /// flip: `place_svg` mapping the scratch bounds' centre through the parent's
    /// inverse as well → fails at the centre assertion.
    #[test]
    fn a_pasted_svg_centres_in_a_turned_frame() {
        use ondin_core::kurbo::{Affine, Size};
        use ondin_core::{Operation, Transaction};
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let page = app.session.doc.first_page();
        let frame = app.session.ids.mint();
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id: frame,
            parent: page,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 200.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((300.0, 100.0)) * Affine::rotate(0.5)),
            name: None,
        }])));
        app.session.selection.set_one(frame);
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 20"><rect x="5" y="5" width="40" height="20" fill="red"/></svg>"#;
        app.start_paste_svg(&ctx, svg.to_string(), None);
        app.settle_imports();
        let root = app.session.selection.single().expect("the import selected");
        assert_eq!(
            app.session.doc.get(root).and_then(|n| n.parent()),
            Some(frame)
        );
        let res = &app.session.resolved;
        let (got, want) = (
            res.world_bounds(root).unwrap().center(),
            res.world_bounds(frame).unwrap().center(),
        );
        assert!((got - want).hypot() < 1e-6, "{got:?} vs {want:?}");
    }

    /// Imports land in the order they were asked, whichever finished reading first:
    /// a quick second one waits for a slow first.
    /// flip: `land_ready` partitioning on `read.is_some()` alone → fails at "the
    /// second waits".
    #[test]
    fn imports_land_in_the_order_asked() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let slow = Job::spawn("test-import", &ctx, move || {
            let _ = wait.recv();
            vec![Read::Text("first".into())]
        });
        let tab = app.tabs.active;
        app.imports.push(Import {
            tab,
            door: Door::Paste { at: None },
            job: Some(slow),
            read: None,
        });
        app.imports.push(Import {
            tab,
            door: Door::Paste { at: None },
            job: Some(Job::done(vec![Read::Text("second".into())])),
            read: None,
        });
        let landed = |app: &OndinApp, s: &str| {
            page_nodes(app)
                .iter()
                .any(|n| matches!(n.kind(), NodeKind::Text { content, .. } if content == s))
        };
        app.poll_imports();
        assert!(!landed(&app, "second"), "the second waits");
        go.send(()).unwrap();
        app.settle_imports();
        assert!(landed(&app, "first") && landed(&app, "second"));
        // Document order, bottom first.
        let order: Vec<String> = page_nodes(&app)
            .iter()
            .filter_map(|n| match n.kind() {
                NodeKind::Text { content, .. } if content != "Ondin" => Some(content.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(order, ["first", "second"], "the first below");
    }

    /// A pasted SVG's text in a family the document uses lands in that family, read
    /// on a worker whose engine was given the document's faces.
    /// flip: `start_reading` without `register_font_data` → fails at the family.
    #[test]
    fn a_pasted_svgs_text_keeps_the_documents_family() {
        use ondin_core::{Operation, Transaction};
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        assert!(ondin_core::text::register_fonts_as(
            ondin_core::text::bundled_inter()[0].to_vec(),
            "Probe Sans"
        ));
        let page = app.session.doc.first_page();
        let id = app.session.ids.mint();
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id,
            parent: page,
            index: 0,
            kind: NodeKind::Text {
                content: "Probe".into(),
                style: Box::new(ondin_core::TextStyle {
                    font_family: "Probe Sans".into(),
                    font_size: 20.0,
                    ..Default::default()
                }),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing: ondin_core::TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        }])));
        assert!(
            !app.session.resolved.text_fonts().is_empty(),
            "fixture: a face drew"
        );

        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 40"><text x="0" y="30" font-family="Probe Sans" font-size="20">Pasted</text></svg>"#;
        app.start_paste_svg(&ctx, svg.to_string(), None);
        app.settle_imports();
        let pasted = page_nodes(&app)
            .into_iter()
            .find_map(|n| match n.kind() {
                NodeKind::Text { content, style, .. } if content == "Pasted" => {
                    Some(style.font_family.clone())
                }
                _ => None,
            })
            .expect("the text came in");
        assert_eq!(pasted, "Probe Sans");
        let status = &app.session.status().text;
        assert!(!status.contains("substituted"), "{status}");
    }

    /// Pasted SVG markup is read off the frame and lands as layers; markup that
    /// sniffs as SVG but will not read lands as the text layer the paste's last arm
    /// would have made.
    ///
    /// flip: `start_paste_svg` refusing unreadable markup instead of `Read::Text` →
    /// fails at "lands as text".
    #[test]
    fn pasted_svg_lands_as_layers_or_else_as_text() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="red"/></svg>"#;
        let before = app.session.revision();
        app.start_paste_svg(&ctx, svg.to_string(), None);
        assert_eq!(
            (app.session.revision(), app.session.status().text.as_str()),
            (before, "Pasting SVG…"),
            "read off the frame"
        );
        app.settle_imports();
        assert!(
            app.session.status().text.starts_with("Pasted SVG — "),
            "{}",
            app.session.status().text
        );
        assert!(
            page_nodes(&app)
                .iter()
                .any(|n| matches!(n.kind(), NodeKind::Rect { .. })),
            "the rect came in"
        );

        let empty = r#"<svg xmlns="http://www.w3.org/2000/svg"></svg>"#;
        app.start_paste_svg(&ctx, empty.to_string(), None);
        app.settle_imports();
        assert!(
            page_nodes(&app)
                .iter()
                .any(|n| matches!(n.kind(), NodeKind::Text { content, .. } if content == empty)),
            "lands as text"
        );
        assert!(app.imports.is_empty());
    }

    /// A clipboard picture is encoded and decoded off the frame, then lands as an
    /// image layer whose pixels the decode cache holds.
    ///
    /// flip: `land_import` skipping `adopt` for a picture → fails at "decoded".
    #[test]
    fn a_pasted_picture_lands_as_a_layer_with_its_pixels() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let before = app.session.revision();
        app.start_paste_image(&ctx, vec![255; 2 * 2 * 4], (2, 2), None);
        assert_eq!(
            (app.session.revision(), app.session.status().text.as_str()),
            (before, "Pasting image…"),
            "read off the frame"
        );
        app.settle_imports();
        assert_eq!(app.session.status().text, "Pasted image");
        let id = page_nodes(&app)
            .iter()
            .find_map(|n| match n.paint().fills.first()?.brush.literal() {
                Some(Brush::Image(img)) => Some(img.image.id.clone()),
                _ => None,
            })
            .expect("an image layer");
        assert_eq!(
            app.canvas.images().get(&id).map(|d| (d.width, d.height)),
            Some((2, 2)),
            "decoded"
        );
    }

    /// *Replace image* reads its file off the frame and gives it to the slot; a file
    /// that is not a picture is refused by name and leaves the fill alone, and so
    /// does a picture whose fill changed while it read.
    ///
    /// flip: `land_import`'s `Door::Replace` arm doing nothing → fails at "replaced";
    /// the arm without its `was` check → fails at "the changed fill stands".
    #[test]
    fn replace_image_reads_its_file_off_the_frame() {
        use ondin_core::kurbo::{Affine, Size};
        use ondin_core::peniko::Color;
        use ondin_core::{Fill, Operation, Transaction, keyed_by_position};
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let shape = app.session.ids.mint();
        let page = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: shape,
                parent: page,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 100.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
            Operation::SetFills {
                id: shape,
                fills: keyed_by_position([Fill {
                    brush: Brush::Solid(Color::from_rgba8(0, 0, 255, 255)).into(),
                    visible: true,
                }]),
            },
        ])));
        let dir = std::env::temp_dir().join(format!("ondin-replace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (png, notes) = (dir.join("photo.png"), dir.join("notes.txt"));
        std::fs::write(
            &png,
            crate::app::rgba_to_png(&[255, 0, 0, 255], (1, 1)).unwrap(),
        )
        .unwrap();
        std::fs::write(&notes, "not a picture").unwrap();
        let is_image = |app: &OndinApp| {
            matches!(
                app.session.doc.get(shape).unwrap().paint().fills[0]
                    .brush
                    .literal(),
                Some(Brush::Image(_))
            )
        };
        let slot = PaintSlot::Fill(0);

        app.start_replace_image(&ctx, notes, shape, slot, true);
        app.settle_imports();
        assert_eq!(app.session.status().text, "Could not read: notes.txt");
        assert!(!is_image(&app), "the fill stands");

        // The fill edited while the file reads: the picture goes nowhere, and says so.
        app.start_replace_image(&ctx, png.clone(), shape, slot, true);
        let green = Fill {
            brush: Brush::Solid(Color::from_rgba8(0, 255, 0, 255)).into(),
            visible: true,
        };
        assert!(app.session.commit(Transaction(vec![Operation::SetFills {
            id: shape,
            fills: keyed_by_position([green]),
        }])));
        app.settle_imports();
        assert!(!is_image(&app), "the changed fill stands");
        assert!(
            app.session.status().text.contains("was not placed"),
            "{}",
            app.session.status().text
        );

        let before = app.session.revision();
        app.start_replace_image(&ctx, png, shape, slot, true);
        assert_eq!(app.session.revision(), before, "read off the frame");
        app.settle_imports();
        assert!(is_image(&app), "replaced");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
