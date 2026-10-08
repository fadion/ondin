//! The library's kits, as the insert picker reads them (§5.3e, §15 D1014).
//!
//! **Two halves with two costs.** Which documents are kits, and which version each
//! last published, is in the library scan already — `meta.kit`, read from each
//! file's first 4 KB (§15 D1012) — so listing them costs nothing ([`kits_in`]).
//! What a kit published is not: the frozen copy sits at the end of the file, after
//! everything else, and reading it means loading the whole document. So the
//! published states are loaded **on a thread of their own**, with each published
//! main's tile picture rendered there too ([`KitShelf`]) — the cover worker's
//! shape (`library::cover`, §15 D820) and its reason: a document load and a
//! render are hundreds of milliseconds at worst, and the picker opens on a click.
//!
//! The shelf is keyed by kit id and the file's modification time **as the
//! library scan last read it**, so a kit that publishes while this document is
//! open is read again once the library has rescanned — which opening the picker
//! does (`OndinApp::toggle_kit_picker`); nothing else invalidates it.

use crate::library::scan::Entry;
use crate::library::state::Library;
use eframe::egui;
use ondin_core::NodeId;
use ondin_core::kit::Published;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};

/// The largest a tile picture is rendered, in pixels — twice the mockup's tile
/// (about 100×52), so it stays sharp at 200% display scaling.
const TILE_W: f64 = 200.0;
const TILE_H: f64 = 104.0;

/// One kit in the library: a document whose mark is present and sharing.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct KitEntry {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub modified: u64,
    /// The version it last published; 0 when it never has (3H).
    pub version: u32,
}

/// Every kit in `lib` but `own` — the document being edited, whose mains are its
/// own rather than a kit's — by name.
///
/// **A conflict copy loses** (§15 D1011 (10)): a sync client's copy carries the
/// kit's `meta.id` too, so two files claim one kit; the one that is not a conflict
/// copy by its filename (`Library::conflict`, §15 D375) is the kit, and the rest
/// are ignored. If every claimant is one, the first stands rather than the kit
/// vanishing.
pub(crate) fn kits_in(lib: &Library, own: Option<&str>) -> Vec<KitEntry> {
    let mut by_id: HashMap<&str, &Entry> = HashMap::new();
    for e in &lib.entries {
        let (Some(id), Some(kit)) = (e.meta.id.as_deref(), e.meta.kit.as_ref()) else {
            continue;
        };
        if !kit.is_shared() || Some(id) == own {
            continue;
        }
        match by_id.get(id) {
            Some(held) if lib.conflict(held).is_none() => {}
            Some(_) if lib.conflict(e).is_some() => {}
            _ => {
                by_id.insert(id, e);
            }
        }
    }
    let mut out: Vec<KitEntry> = by_id
        .into_iter()
        .map(|(id, e)| KitEntry {
            id: id.to_string(),
            name: e.display_name(),
            path: e.path.clone(),
            modified: e.modified,
            version: e.meta.kit.as_ref().map_or(0, |k| k.version),
        })
        .collect();
    out.sort_by_key(|k| k.name.to_lowercase());
    out
}

/// A kit as a list heading names it (4F, 3I).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KitHeading {
    /// The kit's id — what a list groups by, since two kits can share a name
    /// and two lost ones both read *Kit* (`arch-scribe`'s reading).
    pub kit: String,
    /// What the heading reads.
    pub name: String,
    /// On the right: the copy's version, or *unavailable*.
    pub status: String,
}

/// Which kit a vendored main came from, as a list heading says it: the kit's name
/// and, on the right, its version — or *unavailable* when the library cannot find
/// it (4F, 3I). `None` for a main of the document's own.
pub(crate) fn kit_heading(
    doc: &ondin_core::Document,
    library: &[KitEntry],
    main: NodeId,
) -> Option<KitHeading> {
    let vault = doc.vault()?;
    let mut top = main;
    for _ in 0..=doc.len() {
        let parent = doc.get(top)?.parent()?;
        if parent == vault {
            break;
        }
        top = parent;
    }
    let origin = doc.get(top)?.origin()?;
    let found = library.iter().find(|k| k.id == origin.kit);
    let name = found
        .map(|k| k.name.clone())
        .or_else(|| origin.name.clone())
        .unwrap_or_else(|| "Kit".into());
    let status = match found {
        Some(_) => format!("v{}", origin.version),
        None => "unavailable".into(),
    };
    Some(KitHeading {
        kit: origin.kit.clone(),
        name,
        status,
    })
}

/// Where a kit's published state is.
#[derive(Clone)]
pub(crate) enum Load {
    /// Asked for; the worker has it.
    Pending,
    /// Loaded. `None` when the kit has published nothing (3H).
    Ready(Option<Arc<Published>>),
    /// The file could not be read or loaded.
    Failed,
}

/// A published main's tile picture, as the worker rendered it.
struct Pixels {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

struct Shelf {
    modified: u64,
    load: Load,
    pixels: HashMap<NodeId, Pixels>,
    tiles: HashMap<NodeId, egui::TextureHandle>,
}

struct Job {
    id: String,
    path: PathBuf,
    modified: u64,
}

struct Done {
    id: String,
    modified: u64,
    load: Load,
    pixels: HashMap<NodeId, Pixels>,
}

/// The published states and tile pictures of the kits the picker has asked for,
/// loaded off the frame (module doc).
#[derive(Default)]
pub(crate) struct KitShelf {
    shelves: HashMap<String, Shelf>,
    worker: Option<(mpsc::Sender<Job>, mpsc::Receiver<Done>)>,
}

impl KitShelf {
    /// Ask for `kit`'s published state — read again when its file has changed
    /// since it was last read. Free when it is held and current.
    pub(crate) fn ask(&mut self, kit: &KitEntry) {
        if self
            .shelves
            .get(&kit.id)
            .is_some_and(|s| s.modified == kit.modified)
        {
            return;
        }
        self.shelves.insert(
            kit.id.clone(),
            Shelf {
                modified: kit.modified,
                load: Load::Pending,
                pixels: HashMap::new(),
                tiles: HashMap::new(),
            },
        );
        let (tx, _) = self.worker.get_or_insert_with(spawn);
        let _ = tx.send(Job {
            id: kit.id.clone(),
            path: kit.path.clone(),
            modified: kit.modified,
        });
    }

    /// Take what the worker finished, and keep frames coming while it works.
    pub(crate) fn pass(&mut self, ctx: &egui::Context) {
        let Some((_, rx)) = &self.worker else {
            return;
        };
        let done: Vec<Done> = rx.try_iter().collect();
        for d in done {
            self.land(d);
        }
        if self.pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn land(&mut self, d: Done) {
        // A result for a file that has changed again since it was asked for is
        // dropped; the newer ask is on its way.
        if let Some(s) = self.shelves.get_mut(&d.id)
            && s.modified == d.modified
        {
            s.load = d.load;
            s.pixels = d.pixels;
            s.tiles.clear();
        }
    }

    fn pending(&self) -> bool {
        self.shelves
            .values()
            .any(|s| matches!(s.load, Load::Pending))
    }

    /// Where `id`'s published state is, if it was asked for.
    pub(crate) fn load(&self, id: &str) -> Option<&Load> {
        self.shelves.get(id).map(|s| &s.load)
    }

    /// `top`'s tile texture in kit `id`, uploaded the first time it is asked for.
    pub(crate) fn tile(
        &mut self,
        ctx: &egui::Context,
        id: &str,
        top: NodeId,
    ) -> Option<egui::TextureHandle> {
        let s = self.shelves.get_mut(id)?;
        if let Some(t) = s.tiles.get(&top) {
            return Some(t.clone());
        }
        let p = s.pixels.get(&top)?;
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [p.width as usize, p.height as usize],
            &p.rgba,
        );
        let t = ctx.load_texture(
            format!("kit-tile-{id}-{}", top.to_wire()),
            image,
            egui::TextureOptions::LINEAR,
        );
        s.tiles.insert(top, t.clone());
        Some(t)
    }

    /// Wait for every outstanding ask — the tests', `Covers::settle`'s shape (§15
    /// D820): a test that pumped "the next frame" would race the worker.
    #[cfg(test)]
    pub(crate) fn settle(&mut self) {
        while self.pending() {
            let Some((_, rx)) = &self.worker else {
                return;
            };
            match rx.recv_timeout(std::time::Duration::from_secs(20)) {
                Ok(d) => self.land(d),
                Err(_) => return,
            }
        }
    }
}

fn spawn() -> (mpsc::Sender<Job>, mpsc::Receiver<Done>) {
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let (done_tx, done_rx) = mpsc::channel::<Done>();
    std::thread::Builder::new()
        .name("ondin-kits".into())
        .spawn(move || {
            for job in job_rx {
                let (load, pixels) = read(&job.path);
                let sent = done_tx.send(Done {
                    id: job.id,
                    modified: job.modified,
                    load,
                    pixels,
                });
                if sent.is_err() {
                    return;
                }
            }
        })
        .expect("spawn the kit loader");
    (job_tx, done_rx)
}

/// Load a kit's file and render a tile for every published main and set.
fn read(path: &std::path::Path) -> (Load, HashMap<NodeId, Pixels>) {
    let Some(doc) = std::fs::read(path)
        .ok()
        .and_then(|b| ondin_core::io::load(&b).ok())
    else {
        return (Load::Failed, HashMap::new());
    };
    let Some(published) = doc.published() else {
        return (Load::Ready(None), HashMap::new());
    };
    let frozen = published.document();
    let res = ondin_core::Resolved::rebuild(frozen);
    let mut pixels = HashMap::new();
    for top in published.tops() {
        let Some(view) = ondin_export::extent(&res, &[*top]) else {
            continue;
        };
        let (w, h) = (view.width().max(1.0), view.height().max(1.0));
        let scale = (TILE_W / w).min(TILE_H / h).min(4.0);
        let opts = ondin_export::png::RasterOpts {
            scale,
            ..Default::default()
        };
        if let Some(r) = ondin_export::png::raster_of(frozen, &res, &[*top], &opts) {
            pixels.insert(
                *top,
                Pixels {
                    rgba: r.rgba,
                    width: r.width,
                    height: r.height,
                },
            );
        }
    }
    (Load::Ready(Some(Arc::new(published.clone()))), pixels)
}
