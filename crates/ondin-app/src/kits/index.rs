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
//! does (`OndinApp::toggle_kit_picker`), and the update watch does when a kit the
//! document uses changed on disk (`OndinApp::watch_kits`, §15 D1020); nothing
//! else invalidates it.

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
    /// The last publish's note, when it had one — the update notice shows it (5A).
    pub note: Option<String>,
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
        let (Some(id), Some(_)) = (e.meta.id.as_deref(), e.meta.kit.as_ref()) else {
            continue;
        };
        if Some(id) == own || by_id.contains_key(id) {
            continue;
        }
        if let Some(winner) = kit_file(lib, id) {
            by_id.insert(id, winner);
        }
    }
    // **Sharing is asked of the file that won, not before** (§15 D1024, the
    // kits QA review's second pass): filtered first, a kit that stopped sharing
    // left its conflict copy — still marked sharing — standing in for it, and
    // consumers went on seeing the kit, usually at a stale publish.
    let mut out: Vec<KitEntry> = by_id
        .into_iter()
        .filter(|(_, e)| e.meta.kit.as_ref().is_some_and(|k| k.is_shared()))
        .map(|(id, e)| KitEntry {
            id: id.to_string(),
            name: e.display_name(),
            path: e.path.clone(),
            modified: e.modified,
            version: e.meta.kit.as_ref().map_or(0, |k| k.version),
            note: e.meta.kit.as_ref().and_then(|k| k.note.clone()),
        })
        .collect();
    out.sort_by_key(|k| k.name.to_lowercase());
    out
}

/// **The file that is the kit `id`** among those carrying a kit mark with it —
/// [`kits_in`]'s rule, and the dashboard's, asked in one place: the one that is
/// not a conflict copy (§15 D1011 (10)); if every claimant is one, the first by
/// path, so the answer does not hang on the scan's order.
///
/// 🚨 **The dashboard once asked its own question** (the v0.5.0 release review's
/// `[X3.1-L1-03]`): it ruled out every conflict copy, so a kit whose original
/// was trashed or renamed away stood in the picker, from its copy, and was
/// neither listed under *Kits* nor offered *Stop sharing* on its card.
pub(crate) fn kit_file<'a>(lib: &'a Library, id: &str) -> Option<&'a Entry> {
    let claimants = lib
        .entries
        .iter()
        .filter(|e| e.meta.id.as_deref() == Some(id) && e.meta.kit.is_some());
    let mut first: Option<&Entry> = None;
    for e in claimants {
        if lib.conflict(e).is_none() {
            return Some(e);
        }
        if first.is_none_or(|f| e.path < f.path) {
            first = Some(e);
        }
    }
    first
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
pub(crate) struct Pixels {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl Pixels {
    /// As an egui texture named `name`.
    pub(crate) fn texture(&self, ctx: &egui::Context, name: String) -> egui::TextureHandle {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [self.width as usize, self.height as usize],
            &self.rgba,
        );
        ctx.load_texture(name, image, egui::TextureOptions::LINEAR)
    }
}

struct Shelf {
    modified: u64,
    /// The version the file said it last published **when it was read** — not
    /// the scan's, which may be older (§15 D1023).
    version: Option<u32>,
    load: Load,
    /// Whether the tile pictures are still being drawn — after the published
    /// state has landed (`[X3.1-L4-01]`).
    drawing: bool,
    /// The rendered pictures not yet uploaded: each is taken out as its
    /// texture is made (`[X3.1-L4-02]`).
    pixels: HashMap<NodeId, Pixels>,
    tiles: HashMap<NodeId, egui::TextureHandle>,
}

struct Job {
    id: String,
    path: PathBuf,
    modified: u64,
}

/// What the worker sends: a kit's published state as soon as it is read, then
/// its tile pictures once drawn.
///
/// 🚨 **Two messages, so nothing waits on the drawing** (the v0.5.0 release
/// review's `[X3.1-L4-01]`): one message carrying both kept the update pill's
/// *Accept all* disabled under *"Loading…"* for every second a picture-heavy
/// kit took to render — tiles the pill never shows.
enum Done {
    State {
        id: String,
        modified: u64,
        version: Option<u32>,
        load: Load,
        /// Whether a [`Done::Tiles`] follows.
        drawing: bool,
    },
    Tiles {
        id: String,
        modified: u64,
        pixels: HashMap<NodeId, Pixels>,
    },
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
                version: None,
                load: Load::Pending,
                drawing: false,
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
        if self.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn land(&mut self, d: Done) {
        // A result for a file that has changed again since it was asked for is
        // dropped; the newer ask is on its way.
        match d {
            Done::State {
                id,
                modified,
                version,
                load,
                drawing,
            } => {
                if let Some(s) = self.shelves.get_mut(&id)
                    && s.modified == modified
                {
                    s.load = load;
                    s.version = version;
                    s.drawing = drawing;
                    s.pixels.clear();
                    s.tiles.clear();
                }
            }
            Done::Tiles {
                id,
                modified,
                pixels,
            } => {
                if let Some(s) = self.shelves.get_mut(&id)
                    && s.modified == modified
                {
                    s.drawing = false;
                    s.pixels = pixels;
                    s.tiles.clear();
                }
            }
        }
    }

    /// Whether a published state or its pictures are still on their way.
    fn busy(&self) -> bool {
        self.shelves
            .values()
            .any(|s| s.drawing || matches!(s.load, Load::Pending))
    }

    /// Drop every kit `listed` does not name — a kit deleted, moved out of
    /// the library or no longer shared, whose pictures were otherwise kept for
    /// the window's life (the v0.5.0 release review's `[X3.1-L4-02]`) — and
    /// forget every read that failed, so the next ask reads the file again: a
    /// sync client or an antivirus holding the file once (§15 D716) left the
    /// kit *Unreadable* until its file changed (`[X3.1-L1-01]`). The picker
    /// calls it as it opens.
    pub(crate) fn refresh(&mut self, listed: &[KitEntry]) {
        self.shelves
            .retain(|id, s| !matches!(s.load, Load::Failed) && listed.iter().any(|k| &k.id == id));
    }

    /// `kit`'s published state **as its file now stands** — `None` when the shelf
    /// read an older version of the file, or has not read it.
    ///
    /// ⚠️ **What an accept must ask, not [`Self::load`]**: a copy read before the
    /// kit published again would be written into the document under the new
    /// version, and the update it was meant to bring would never arrive
    /// (`arch-scribe`'s reading of D1016's first cut).
    ///
    /// ⚠️ **And `None` when the file read had published past the scan's
    /// version** (§15 D1023, the kits QA review's finding): a kit that published
    /// between the scan and the read gave the newer content under the older
    /// number, written into the document as that version — a mislabel the next
    /// pill re-offered. The library rescans and the shelf is asked again.
    pub(crate) fn current(&self, kit: &KitEntry) -> Option<Arc<Published>> {
        let s = self.shelves.get(&kit.id)?;
        match &s.load {
            Load::Ready(Some(p))
                if s.modified == kit.modified && s.version == Some(kit.version) =>
            {
                Some(p.clone())
            }
            _ => None,
        }
    }

    /// The version `id`'s file said it published when the shelf read it — what
    /// a vendoring from [`Self::load`]'s state records (§15 D1023).
    pub(crate) fn read_version(&self, id: &str) -> Option<u32> {
        self.shelves.get(id)?.version
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
        // Taken out as it is uploaded: the texture is the picture from here,
        // and a re-read brings both again (`[X3.1-L4-02]`).
        let t = s
            .pixels
            .remove(&top)?
            .texture(ctx, format!("kit-tile-{id}-{}", top.to_wire()));
        s.tiles.insert(top, t.clone());
        Some(t)
    }

    /// Wait for every outstanding ask — the tests', `Covers::settle`'s shape (§15
    /// D820): a test that pumped "the next frame" would race the worker.
    #[cfg(test)]
    pub(crate) fn settle(&mut self) {
        while self.busy() {
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
                let (load, version) = read(&job.path);
                let published = match &load {
                    Load::Ready(Some(p)) => Some(p.clone()),
                    _ => None,
                };
                let sent = done_tx.send(Done::State {
                    id: job.id.clone(),
                    modified: job.modified,
                    version,
                    load,
                    drawing: published.is_some(),
                });
                if sent.is_err() {
                    return;
                }
                if let Some(p) = published {
                    let pixels = tiles_of(p.document(), p.tops());
                    let sent = done_tx.send(Done::Tiles {
                        id: job.id,
                        modified: job.modified,
                        pixels,
                    });
                    if sent.is_err() {
                        return;
                    }
                }
            }
        })
        .expect("spawn the kit loader");
    (job_tx, done_rx)
}

/// Load a kit's file: its published state, with the version the file says it
/// published, as read. Its tiles are drawn after, by [`tiles_of`], in a
/// message of their own (`[X3.1-L4-01]`).
fn read(path: &std::path::Path) -> (Load, Option<u32>) {
    let Some(doc) = std::fs::read(path)
        .ok()
        .and_then(|b| ondin_core::io::load(&b).ok())
    else {
        return (Load::Failed, None);
    };
    let version = doc.meta().kit.as_ref().map(|k| k.version);
    let Some(published) = doc.published() else {
        return (Load::Ready(None), version);
    };
    (Load::Ready(Some(Arc::new(published.clone()))), version)
}

/// What a tile for `top` shows: a set's default variant, anything else itself.
///
/// **A set's tile is its default variant**, the one a click places (§15
/// D1028): the whole set drew as a sheet of variants too small to tell apart,
/// and the tile's glyph already says it is a set.
pub(crate) fn tile_subject(doc: &ondin_core::Document, top: NodeId) -> NodeId {
    match doc.get(top) {
        Some(n) if n.set().is_some() => ondin_core::kit::default_variant(doc, top).unwrap_or(top),
        _ => top,
    }
}

/// A tile picture for each of `tops` in `doc`, keyed by the top — the kits'
/// published states here, and the document's own mains in the picker's *This
/// document* (`super::picker`'s `LocalTiles`).
///
/// 🚨 **Each drawn as a placement will have it** (`ondin_core::kit::as_placed`):
/// moved to the root with its own transform, as `kit::insert` and the picker
/// place it. Drawn where it sat (the v0.5.0 release review's `[X4.1-L1-02]`,
/// with `[X4.1-L1-04]`), a main in a frame rotated 90° and scaled 2× dragged
/// as 40 × 160 and turned and landed 80 × 20 and level; since the ancestor was
/// no input to `LocalTiles`' comparison, rotating it left the tile as it was;
/// a variant drew at the size its set's layout gave it, not the one it lands
/// at; and a vault main, never resolved where it sits, dragged as an empty box.
pub(crate) fn tiles_of(doc: &ondin_core::Document, tops: &[NodeId]) -> HashMap<NodeId, Pixels> {
    // Each subject chosen before the move: out of its set, a default variant
    // is no longer the set's, and asking afterwards drew the next variant.
    let shown: Vec<NodeId> = tops.iter().map(|t| tile_subject(doc, *t)).collect();
    let doc = &ondin_core::kit::as_placed(doc, &shown);
    let res = ondin_core::Resolved::rebuild(doc);
    // The pictures decoded once for every tile (`[X3.1-L4-01]`).
    let images = prepared(doc);
    let mut pixels = HashMap::new();
    for (top, shown) in tops.iter().zip(shown) {
        if let Some(p) = raster_fit(doc, &res, shown, TILE_W, TILE_H, 4.0, &images) {
            pixels.insert(*top, p);
        }
    }
    pixels
}

/// `doc`'s pictures, decoded — what [`raster_fit`] draws from, made once per
/// document however many tiles it draws.
pub(crate) fn prepared(doc: &ondin_core::Document) -> ondin_render::ImageStore {
    let mut images = ondin_render::ImageStore::new();
    images.prepare(doc);
    images
}

/// `id` rendered to fit `w × h` pixels, never scaled past `max_scale`, from
/// `images` — [`prepared`] for `doc` once by the caller: a store made here,
/// per call, decoded every picture in the document for every tile (the v0.5.0
/// release review's `[X3.1-L4-01]`, 9.3 s for 200 mains over ten photos).
pub(crate) fn raster_fit(
    doc: &ondin_core::Document,
    res: &ondin_core::Resolved,
    id: NodeId,
    w: f64,
    h: f64,
    max_scale: f64,
    images: &ondin_render::ImageStore,
) -> Option<Pixels> {
    let view = ondin_export::extent(res, &[id])?;
    let (vw, vh) = (view.width().max(1.0), view.height().max(1.0));
    let scale = (w / vw).min(h / vh).min(max_scale);
    let opts = ondin_export::png::RasterOpts {
        scale,
        ..Default::default()
    };
    let r = ondin_export::png::raster_of_with(doc, res, &[id], &opts, images)?;
    Some(Pixels {
        rgba: r.rgba,
        width: r.width,
        height: r.height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::Size;
    use ondin_core::meta::{DocumentMeta, KitMark};
    use ondin_core::{Document, IdSource, NodeKind, Operation, Transaction};

    /// A kit file with one published main, *Badge*, at `path`; its id and main.
    fn kit_file(path: &std::path::Path) -> (String, NodeId) {
        let mut ids = IdSource::new(0xC1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let badge = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: badge,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(40.0, 20.0),
                },
                transform: None,
                name: Some("Badge".into()),
            },
            Operation::SetComponent {
                id: badge,
                component: true,
            },
        ]))
        .unwrap();
        let id = "5eed00000000000000000000000000c1".to_string();
        doc.set_meta(DocumentMeta {
            id: Some(id.clone()),
            kit: Some(KitMark::default()),
            ..Default::default()
        });
        ondin_core::kit::publish(&mut doc, None, 1).unwrap();
        crate::library::store::write_document(path, &doc).unwrap();
        (id, badge)
    }

    fn entry(id: &str, path: &std::path::Path) -> KitEntry {
        KitEntry {
            id: id.into(),
            name: "Atlas".into(),
            path: path.to_path_buf(),
            modified: 7,
            version: 1,
            note: None,
        }
    }

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ondin-shelf-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// **The published state is sent ahead of the pictures** (the v0.5.0
    /// release review's `[X3.1-L4-01]`): an accept waits on the state, never on
    /// the drawing.
    ///
    /// Flip, run: one message carrying both fails *"the state first"*.
    #[test]
    fn the_worker_sends_the_state_before_the_pictures() {
        let dir = temp("order");
        let path = dir.join("atlas.ondin");
        let (id, badge) = kit_file(&path);
        let (tx, rx) = spawn();
        tx.send(Job {
            id: id.clone(),
            path,
            modified: 7,
        })
        .unwrap();
        let wait = std::time::Duration::from_secs(20);
        match rx.recv_timeout(wait).unwrap() {
            Done::State { load, drawing, .. } => {
                assert!(matches!(load, Load::Ready(Some(_))));
                assert!(drawing, "and the pictures follow");
            }
            Done::Tiles { .. } => panic!("the state first"),
        }
        match rx.recv_timeout(wait).unwrap() {
            Done::Tiles { pixels, .. } => assert!(pixels.contains_key(&badge)),
            Done::State { .. } => panic!("then the pictures"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A tile's pixels go once its texture is made** (the v0.5.0 release
    /// review's `[X3.1-L4-02]`): each was held twice for the window's life.
    ///
    /// Flip, run: `tile` reading the pixels without taking them fails
    /// *"uploaded, let go"*.
    #[test]
    fn a_tiles_pixels_go_once_uploaded() {
        let ctx = egui::Context::default();
        let dir = temp("pixels");
        let path = dir.join("atlas.ondin");
        let (id, badge) = kit_file(&path);
        let mut shelf = KitShelf::default();
        shelf.ask(&entry(&id, &path));
        shelf.settle();
        assert!(
            shelf.shelves[&id].pixels.contains_key(&badge),
            "fixture: drawn"
        );
        let first = shelf.tile(&ctx, &id, badge).expect("a tile");
        assert!(
            !shelf.shelves[&id].pixels.contains_key(&badge),
            "uploaded, let go"
        );
        assert_eq!(
            shelf.tile(&ctx, &id, badge).map(|t| t.id()),
            Some(first.id())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A read that failed is tried again when the picker opens, and a kit the
    /// library no longer lists is let go** (the v0.5.0 release review's
    /// `[X3.1-L1-01]`, `[X3.1-L4-02]`). The file is missing at the first read,
    /// then appears with the modification time the scan had — which kept the
    /// kit *Unreadable* until its file changed again.
    ///
    /// Flip, run: `refresh` keeping a failed shelf fails *"read again"*.
    #[test]
    fn a_failed_read_is_tried_again_and_an_unlisted_kit_let_go() {
        let dir = temp("retry");
        let path = dir.join("atlas.ondin");
        let kit = entry("5eed00000000000000000000000000c1", &path);
        let mut shelf = KitShelf::default();
        shelf.ask(&kit);
        shelf.settle();
        assert!(matches!(shelf.load(&kit.id), Some(Load::Failed)), "fixture");
        kit_file(&path);
        shelf.ask(&kit);
        assert!(
            matches!(shelf.load(&kit.id), Some(Load::Failed)),
            "fixture: the same time is not asked again"
        );
        shelf.refresh(std::slice::from_ref(&kit));
        shelf.ask(&kit);
        shelf.settle();
        assert!(
            matches!(shelf.load(&kit.id), Some(Load::Ready(Some(_)))),
            "read again"
        );

        shelf.refresh(&[]);
        assert!(shelf.load(&kit.id).is_none(), "an unlisted kit let go");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
