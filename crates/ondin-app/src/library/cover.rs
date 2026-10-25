//! A document's cover: the picture the dashboard's grid draws on a card (§15
//! D366).
//!
//! **Rendered by the same headless path `ondin export --png` uses** —
//! `Resolved::rebuild` and `ondin_export::png`, no GPU, no window — so a cover is
//! the document rather than a hand-drawn approximation to keep in step with the
//! renderer.
//!
//! Rendering is expensive and there can be sixty documents, so there are two
//! caches:
//!
//! - **On disk**, under [`crate::machine_dir::root`]`/covers`, keyed by document id
//!   *and* modification time, so a document edited elsewhere and synced in gets a
//!   new key and the stale picture is never shown.
//! - **In memory**, as egui textures.
//!
//! **Rendering happens on a thread of its own**: a per-pass time budget could not
//! bound it. **The disk cache is disposable** — delete it and the covers redraw —
//! so it lives in the cache directory, not the base folder, and is not synced.

use super::ids;
use super::scan::Entry;
use eframe::egui;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The longest edge of a cover, in pixels.
///
/// **Sized for the card it fills at 2× display scaling** (about 170×116 pt), not
/// for the document: more pixels are ones the card cannot show.
const COVER_MAX_PX: f64 = 400.0;

/// How many cover textures [`Covers`] keeps resident before [`Covers::pass`]
/// starts letting go of the least recently asked-for.
///
/// A cover is up to ~640 KB, so 128 is ~80 MB at the worst case and more than one
/// screen of file cards on most windows. A screen of project cards can pass it,
/// which is why eviction never takes a cover asked for on the current or previous
/// pass: the cap is exceeded rather than churned.
const MAX_RESIDENT_COVERS: usize = 128;

/// What a cover key is: the document's identity plus the state of its file.
///
/// **Both halves, and the mtime is the easy one to leave out**: keyed on the id
/// alone, a cover edited on another machine and synced in would stay wrong.
///
/// **The id is checked**: the key becomes a filename under the cover cache, so an
/// id holding `../` would write a PNG outside it, and this runs for every card
/// drawn. A malformed id gets no cached cover, as a document with no id does.
fn key(entry: &Entry) -> Option<String> {
    let id = entry.meta.id.as_deref().filter(|id| ids::is_minted(id))?;
    Some(format!("{id}-{}", entry.modified))
}

/// The cover cache's directory, as a [`Covers`] made by `Default` is given it.
///
/// **`None` under `cfg(test)`, a seam rather than a skipped path**: otherwise the
/// suite would write into, and [`Covers::sweep`] delete from, the developer's real
/// cache. A test that wants the disk names its own directory through
/// `Covers::with_dir`.
fn covers_dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(crate::machine_dir::root()?.join("covers"))
}

/// Where a cover lives on disk.
fn cache_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.png"))
}

/// One cover, as the map holds it.
///
/// A document that will not load and one with nothing in it both produce no cover
/// and want the card's plain plate, but only the first is a broken file.
/// `Entry::unread` answers only for a file whose first 4 KB fail to parse;
/// `io::load` runs here over the whole file, so the real answer is kept.
enum Cover {
    /// Rendered and uploaded, with the dashboard pass it was last asked for or
    /// arrived on, which is what [`Covers::pass`] evicts by.
    Ready(egui::TextureHandle, u64),
    /// The file could not be read or would not load. **Remembered**, so it is not
    /// re-parsed on every pass.
    Unreadable,
    /// Loaded and has no extent — an empty document, not a mark for a card.
    Blank,
}

/// Why [`rasterize`] produced nothing, which is the distinction [`Cover`] keeps.
#[derive(Debug, PartialEq, Eq)]
enum NoCover {
    Unreadable,
    Blank,
}

/// The dashboard's cover cache.
///
/// Rendering happens on a worker thread: `get` enqueues and [`Self::drain`]
/// uploads what came back, so nothing in the egui pass renders. (The whole
/// document→pixels path is expensive; a large picture's decode alone takes
/// hundreds of milliseconds.)
///
/// `Default` is hand-written to put [`covers_dir`] in `dir`, the one field whose
/// default is not its type's.
pub struct Covers {
    covers: HashMap<String, Cover>,
    /// Keys handed to the renderer and not yet answered, so a card drawn every
    /// frame enqueues its document once rather than sixty times a second.
    ///
    /// Separate from `covers` rather than a `Cover::Pending` variant: every
    /// reader of that map asks about a *finished* answer.
    queued: std::collections::HashSet<String>,
    /// The renderer, spawned on the first miss.
    ///
    /// Lazily, so an app that never draws a card pays for no thread.
    ///
    /// `None` again whenever the worker is known to be gone, so a death is
    /// recoverable: the next miss spawns a fresh one.
    render: Option<Renderer>,
    /// The disk cache's directory; `None` is no disk cache.
    dir: Option<PathBuf>,
    /// How many dashboard passes [`Self::pass`] has seen — the clock a `Ready`
    /// cover's stamp is read against; egui's pass number would also count the
    /// editor's.
    pass: u64,
    /// Every `pass` and `get` call, in order, for the dashboard's test that the
    /// pass runs once a frame and before any card asks — a question about where
    /// `dashboard_ui` makes the call, which no map or stamp can answer.
    ///
    /// Plain backticks: this field is `cfg(test)`.
    #[cfg(test)]
    calls: Vec<&'static str>,
}

impl Default for Covers {
    fn default() -> Self {
        Self {
            covers: HashMap::new(),
            queued: std::collections::HashSet::new(),
            render: None,
            dir: covers_dir(),
            pass: 0,
            #[cfg(test)]
            calls: Vec::new(),
        }
    }
}

/// The cover renderer's thread, and the answers coming back.
///
/// **A second worker rather than a job on `library::writer::Writer`'s queue**:
/// that queue is FIFO and load-bearing for saves, and a render of hundreds of
/// milliseconds must not put an autosave behind a card's decoration.
///
/// Nothing here writes a record: the PNG `render` may write is derived data that
/// `crate::atomic` exempts.
struct Renderer {
    /// `None` only while [`Drop`] is taking it; dropping the sender ends the
    /// worker's loop.
    tx: Option<std::sync::mpsc::Sender<(Entry, String)>>,
    done: std::sync::mpsc::Receiver<(String, Result<Rgba, NoCover>)>,
    /// Jobs sent and not yet answered, for `Covers::settle` (plain backticks: that
    /// method is `cfg(test)`).
    in_flight: usize,
    handle: Option<std::thread::JoinHandle<()>>,
    /// Set by [`Drop`], and read by the worker before each job. Dropping the
    /// sender alone does not stop the `for` loop until every buffered job has been
    /// taken, so without it the join would wait on the whole backlog.
    cancel: Arc<AtomicBool>,
}

/// The one cover key the worker will panic on instead of rendering.
///
/// The fault is injected, as in `library::writer`'s `POISONED`: no real panic in
/// the render path is known. Keyed on a cover key rather than a one-shot, so a
/// fault armed by one test cannot fire in another.
///
/// Plain backticks: this item is `cfg(test)`.
#[cfg(test)]
static POISONED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

impl Renderer {
    fn spawn(ctx: &egui::Context, dir: Option<PathBuf>) -> Self {
        let (tx, jobs) = std::sync::mpsc::channel::<(Entry, String)>();
        let (report, done) = std::sync::mpsc::channel::<(String, Result<Rgba, NoCover>)>();
        let ctx = ctx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::clone(&cancel);
        let handle = std::thread::spawn(move || {
            for (entry, key) in jobs {
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                // A panic costs one cover and not the worker: uncaught, it ends
                // the thread and no cover appears again all session.
                // `Cargo.toml`'s `panic = "abort"` note is why catching is sound.
                //
                // `Blank`, not `Unreadable`: a panic is a fault in this build and
                // says nothing about the file.
                let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    #[cfg(test)]
                    if POISONED.lock().is_ok_and(|p| p.as_deref() == Some(&key)) {
                        panic!("synthetic cover panic — cover::POISONED({key})");
                    }
                    render(dir.as_deref(), &entry, &key)
                }))
                .unwrap_or(Err(NoCover::Blank));
                if report.send((key, answer)).is_err() {
                    return;
                }
                // eframe is reactive: without this a finished cover would sit in
                // the channel until something else woke the app.
                ctx.request_repaint();
            }
        });
        Self {
            tx: Some(tx),
            done,
            in_flight: 0,
            handle: Some(handle),
            cancel,
        }
    }
}

impl Drop for Renderer {
    /// **Joined rather than detached**, so a renderer dropped by [`Covers::clear`]
    /// (the base folder changed) does not leave a thread rendering covers for a
    /// library nobody is looking at. `cancel` is set first, so the join waits on
    /// one render rather than the backlog.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.tx = None;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Covers {
    /// The texture for a document's cover, or `None` when there is not one —
    /// **or not one yet**.
    ///
    /// The two are deliberately not distinguished: the caller draws the plain
    /// plate either way. The worker requests the repaint when it sends, so this
    /// function requests none.
    pub fn get(&mut self, ctx: &egui::Context, entry: &Entry) -> Option<&egui::TextureHandle> {
        #[cfg(test)]
        self.calls.push("get");
        self.drain(ctx);
        let key = key(entry)?;
        let now = self.pass;
        if let Some(cover) = self.covers.get_mut(&key) {
            return match cover {
                // Stamped, so a cover on screen is never the one evicted.
                Cover::Ready(t, seen) => {
                    *seen = now;
                    Some(t)
                }
                _ => None,
            };
        }
        // Enqueued once: a card is drawn every frame, and without `queued` a slow
        // document would be handed to the renderer sixty times a second.
        if self.queued.insert(key.clone()) {
            let dir = &self.dir;
            let render = self
                .render
                .get_or_insert_with(|| Renderer::spawn(ctx, dir.clone()));
            let sent = render
                .tx
                .as_ref()
                .is_some_and(|tx| tx.send((entry.clone(), key.clone())).is_ok());
            if sent {
                render.in_flight += 1;
            } else {
                // A failed send means the worker has gone, and the key must not
                // stay in `queued` (the `insert` above would answer `false`
                // forever). `drain` meets the same death on the receiving side;
                // forget the worker so the next pass spawns one.
                self.queued.remove(&key);
                self.render = None;
            }
        }
        None
    }

    /// Once per dashboard pass, before any card asks: take what the renderer has
    /// finished, and let go of covers nothing has asked for in a while.
    ///
    /// **The drain is here as well as in [`Self::get`]** because an answer carries
    /// a fact (`Cover::Unreadable`, the red mark) as well as a picture: the list
    /// view and off-screen cards ask for no covers, so without it an answer
    /// arriving after the grid was left would sit in the channel.
    ///
    /// Eviction is described at [`MAX_RESIDENT_COVERS`]; its recency guard holds
    /// only if this runs once a frame, before any card asks.
    pub fn pass(&mut self, ctx: &egui::Context) {
        #[cfg(test)]
        self.calls.push("pass");
        self.pass += 1;
        self.drain(ctx);
        self.evict(MAX_RESIDENT_COVERS);
    }

    /// Drop the least recently asked-for textures until at most `cap` remain —
    /// **never one asked for on this pass or the one before**, so a card on screen
    /// cannot lose its picture to make room, and a screen holding more than `cap`
    /// covers exceeds the cap rather than churning.
    ///
    /// Only `Ready` goes: `Unreadable` holds no texture and is the fact `mark_of`
    /// reads. An evicted cover is asked for afresh by the next `get`, which reads
    /// the disk cache when there is one.
    fn evict(&mut self, cap: usize) {
        let resident = self
            .covers
            .values()
            .filter(|c| matches!(c, Cover::Ready(..)))
            .count();
        if resident <= cap {
            return;
        }
        let mut ready: Vec<(u64, String)> = self
            .covers
            .iter()
            .filter_map(|(k, c)| match c {
                Cover::Ready(_, seen) => Some((*seen, k.clone())),
                _ => None,
            })
            .collect();
        // Oldest first, and the key breaks a tie so the order is not the map's.
        ready.sort_unstable();
        for (seen, key) in ready.into_iter().take(resident - cap) {
            if seen + 1 >= self.pass {
                break;
            }
            self.covers.remove(&key);
        }
    }

    /// Take what the renderer has finished and turn it into textures.
    ///
    /// Called from [`Self::pass`] once a dashboard pass and from [`Self::get`] per
    /// card. It is a `try_recv` loop, so a second call in one pass costs nothing.
    ///
    /// The texture upload stays on this thread: `load_texture` needs the
    /// `Context`, and the upload of a picture bounded by [`COVER_MAX_PX`] is the
    /// cheap half.
    fn drain(&mut self, ctx: &egui::Context) {
        let Some(render) = self.render.as_mut() else {
            return;
        };
        let mut arrived = Vec::new();
        let gone = loop {
            match render.done.try_recv() {
                Ok(answer) => {
                    render.in_flight = render.in_flight.saturating_sub(1);
                    arrived.push(answer);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break false,
                // A hung-up channel is not an idle one; `while let Ok(..)` would
                // leave on both.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break true,
            }
        };
        for (key, answer) in arrived {
            self.arrive(ctx, key, answer);
        }
        if gone {
            // Every key still queued was owed by the dead worker; forgetting them
            // and the worker lets the next pass ask a fresh one. The job that
            // killed it meets the `catch_unwind` this time, so a respawn cannot loop.
            self.queued.clear();
            self.render = None;
        }
    }

    /// One answer from the renderer, into the map — **the only place an answer
    /// becomes a [`Cover`]**.
    ///
    /// `drain` and the test-only `settle` share it, so a test through `settle`
    /// exercises the conversion the app runs.
    fn arrive(&mut self, ctx: &egui::Context, key: String, answer: Result<Rgba, NoCover>) {
        self.queued.remove(&key);
        let cover = match answer {
            Ok(rgba) => {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [rgba.width as usize, rgba.height as usize],
                    &rgba.pixels,
                );
                Cover::Ready(
                    ctx.load_texture(format!("cover-{key}"), image, egui::TextureOptions::LINEAR),
                    self.pass,
                )
            }
            Err(NoCover::Unreadable) => Cover::Unreadable,
            Err(NoCover::Blank) => Cover::Blank,
        };
        self.covers.insert(key, cover);
    }

    /// Wait for every queued cover and take the answers. For tests, so the app has
    /// one asynchronous path; `#[cfg(test)]` rather than `pub` because `dead_code`
    /// analyses a `pub` item in a bin-only crate. Plain backticks: `cfg(test)`.
    #[cfg(test)]
    pub(crate) fn settle(&mut self, ctx: &egui::Context) {
        while self.render.as_ref().is_some_and(|r| r.in_flight > 0) {
            let answer = self.render.as_ref().and_then(|r| r.done.recv().ok());
            let Some((key, answer)) = answer else {
                // The worker is gone and counting on would hang; `drain` forgets
                // it, as the next pass would.
                self.drain(ctx);
                break;
            };
            if let Some(r) = self.render.as_mut() {
                r.in_flight = r.in_flight.saturating_sub(1);
            }
            self.arrive(ctx, key, answer);
        }
    }

    /// A `Covers` whose disk cache is `dir` — the `covers_dir` seam.
    #[cfg(test)]
    pub(crate) fn with_dir(dir: PathBuf) -> Self {
        Self {
            dir: Some(dir),
            ..Self::default()
        }
    }

    /// Whether this document's cover has ever been asked for — queued, or answered
    /// either way. A question about the request, so it cannot be read off the
    /// textures: an answer not yet arrived leaves none.
    ///
    /// Plain backticks: this item is `cfg(test)`.
    #[cfg(test)]
    pub(crate) fn asked_for(&self, entry: &Entry) -> bool {
        key(entry).is_some_and(|k| self.queued.contains(&k) || self.covers.contains_key(&k))
    }

    /// The `pass` and `get` calls since the last time this was asked, in order,
    /// and a fresh log — so a test reads one frame alone.
    ///
    /// Plain backticks: this item is `cfg(test)`.
    #[cfg(test)]
    pub(crate) fn take_calls(&mut self) -> Vec<&'static str> {
        std::mem::take(&mut self.calls)
    }

    /// Whether this document has been **found unloadable**: `io::load` ran over the
    /// whole file to draw its cover and refused it. `Entry::unread` is the cheap
    /// answer, set only when the first 4 KB fail to parse.
    ///
    /// **`false` is "not known to be broken", not "fine"**: a document whose cover
    /// has not been attempted (never on screen, or the list view) answers `false`,
    /// which is why the dashboard asks `entry.unread || this`. A map lookup only.
    pub fn unreadable(&self, entry: &Entry) -> bool {
        key(entry).is_some_and(|k| matches!(self.covers.get(&k), Some(Cover::Unreadable)))
    }

    /// Forget every texture — after the base folder changes, when the documents
    /// on screen are a different set entirely.
    ///
    /// The disk cache is not cleared here, but [`Self::sweep`] runs on the next
    /// `go_to_dashboard` with a keep set from the current library alone, so
    /// switching libraries deletes the other's cached covers and they re-render.
    /// Namespacing the directory per library, or sweeping by age, would avoid it.
    ///
    /// **It is also the renderer's teardown**: dropping it discards every answer
    /// still owed, so a job queued before a move cannot cache `Unreadable` under
    /// the path-independent key. The caller does this *before* `relocate`.
    pub fn clear(&mut self) {
        self.covers.clear();
        self.queued.clear();
        self.render = None;
    }

    /// Delete cached covers on disk that no live document claims.
    ///
    /// Sweeps by key (id and mtime), so it also removes the superseded covers of
    /// documents that are still here; otherwise the folder grows by one PNG per
    /// save.
    ///
    /// **An empty list sweeps nothing**: an unlistable root scans as no documents,
    /// and the directory is shared by every library on the machine. The guard is
    /// here rather than at the call site so no caller can forget it.
    pub fn sweep(&self, live: &[Entry]) {
        let Some(dir) = self.dir.as_deref() else {
            return;
        };
        if live.is_empty() {
            return;
        }
        let keep: std::collections::HashSet<String> = live.iter().filter_map(key).collect();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("png") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !keep.contains(stem) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

/// Decoded pixels, ready for egui.
struct Rgba {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
}

/// Produce a cover: from the disk cache if it is there, by rendering if not.
fn render(dir: Option<&Path>, entry: &Entry, key: &str) -> Result<Rgba, NoCover> {
    let path = dir.map(|d| cache_path(d, key));
    if let Some(rgba) = path
        .as_ref()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|cached| decode(&cached))
    {
        return Ok(rgba);
    }
    // No cached file and a cached file that will not decode (a half-written PNG)
    // both mean "render it again"; re-rendering writes over the bad one.
    let png = rasterize(entry)?;
    if let Some(p) = &path {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Best-effort: a cover that cannot be cached is still a cover.
        let _ = std::fs::write(p, &png);
    }
    // A PNG this build has just produced and cannot decode says nothing about the
    // document, so it is not a mark to put on the card.
    decode(&png).ok_or(NoCover::Blank)
}

/// Render the document's first page that is not a draft to a PNG, framed on
/// everything on it.
///
/// A file that will not read or load is `Unreadable` (a broken document the
/// dashboard marks); a page with no extent is `Blank` (an ordinary empty one).
fn rasterize(entry: &Entry) -> Result<Vec<u8>, NoCover> {
    let bytes = std::fs::read(&entry.path).map_err(|_| NoCover::Unreadable)?;
    let doc = ondin_core::io::load(&bytes).map_err(|_| NoCover::Unreadable)?;
    let res = ondin_core::Resolved::rebuild(&doc);
    // The first page that is not a draft, or the first page when every one is.
    let page = doc.gathered_pages().next().unwrap_or(doc.first_page());
    // An empty page gets no cover: the card's plain plate is its honest
    // picture, and an invented square would repeat on every fresh file.
    let view = ondin_export::extent(&res, &[page]).ok_or(NoCover::Blank)?;
    let (w, h) = (view.width().max(1.0), view.height().max(1.0));
    // Fit the longest edge, never magnify: a 40×40 icon document should be drawn
    // at 40 pixels and centred by the card, not blown up to 400 and blurred.
    let scale = (COVER_MAX_PX / w.max(h)).min(1.0);
    let vp = ondin_export::png::viewport_at(view, scale);
    Ok(ondin_export::png::png(&doc, &res, &vp, page))
}

/// PNG bytes to RGBA8.
fn decode(png: &[u8]) -> Option<Rgba> {
    let image = image::load_from_memory(png).ok()?.to_rgba8();
    let (width, height) = image.dimensions();
    Some(Rgba {
        pixels: image.into_raw(),
        width,
        height,
    })
}

/// A `Covers::default()` has no disk cache under test (`covers_dir`); a test that
/// wants one names its own directory.
///
/// Plain backticks in this module's prose, including this doc: the module is
/// `cfg(test)`, so `cargo doc` cannot see it.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{scan, store};
    use ondin_core::kurbo::Size;
    use ondin_core::{Document, IdSource, NodeKind, Operation, Transaction};
    use std::path::Path;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ondin-cover-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A document with one 80×60 rectangle in it, so it has an extent to frame.
    fn doc_with_a_rect(root_dir: &Path, name: &str) -> Entry {
        let mut ids = IdSource::new(0xC0FFEE);
        let root = ids.mint();
        let mut doc = Document::new(root);
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: ids.mint(),
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(80.0, 60.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        }]))
        .expect("build");
        store::file_document(root_dir, None, name, &mut doc).unwrap();
        scan::scan(root_dir)
            .into_iter()
            .find(|e| e.display_name() == name)
            .expect("the fixture must be in the library")
    }

    /// A document `io::load` refuses and the metadata probe calls healthy: one
    /// number changed, structurally perfect — a file written by a newer build.
    fn written_by_a_newer_build(root_dir: &Path) -> Entry {
        let entry = doc_with_a_rect(root_dir, "Landing v4");
        let whole = std::fs::read_to_string(&entry.path).expect("the fixture is on disk");
        let version = whole
            .find("\"schema_version\":")
            .expect("every filed document carries one");
        let end = whole[version..]
            .find(',')
            .map(|i| version + i)
            .expect("and something after it");
        let newer = format!(
            "{}\"schema_version\":9999{}",
            &whole[..version],
            &whole[end..]
        );
        std::fs::write(&entry.path, &newer).expect("rewrite");
        scan::scan(root_dir)
            .into_iter()
            .next()
            .expect("a document this build cannot load is still listed")
    }

    /// The whole render path, headless — the assertion that a cover is the
    /// document rather than a placeholder.
    #[test]
    fn a_document_with_something_in_it_rasterizes_to_its_own_shape() {
        let root = temp("raster");
        let entry = doc_with_a_rect(&root, "Landing v4");
        let png = rasterize(&entry).expect("a document with a rect must render");
        let rgba = decode(&png).expect("and the PNG must decode");
        // 80×60 is under the cap, so it is drawn 1:1 rather than magnified.
        assert_eq!((rgba.width, rgba.height), (80, 60));
        assert_eq!(rgba.pixels.len(), 80 * 60 * 4);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An empty document gets no cover: a blank raster would look like a preview
    /// on every card of a fresh library.
    #[test]
    fn an_empty_document_has_no_cover() {
        let root = temp("empty");
        let mut doc = Document::new(IdSource::new(1).mint());
        store::file_document(&root, None, "Untitled", &mut doc).unwrap();
        let entry = scan::scan(&root).into_iter().next().unwrap();
        // `Blank`, not merely "no cover": the two failures must be told apart.
        assert_eq!(rasterize(&entry).err(), Some(NoCover::Blank));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A document `io::load` refuses is `Unreadable`, not `Blank`. The fixture is a
    /// version bump, not a truncation (which the metadata probe already catches);
    /// the `unread` assertion shows the probe still calls it healthy.
    ///
    /// flip: mapping `io::load`'s error to `NoCover::Blank` in `rasterize` → fails
    /// at the first assertion.
    #[test]
    fn a_document_the_loader_refuses_is_unreadable_where_an_empty_one_is_merely_blank() {
        let root = temp("unreadable");
        let entry = written_by_a_newer_build(&root);
        assert_eq!(rasterize(&entry).err(), Some(NoCover::Unreadable));
        assert!(
            !entry.unread,
            "the probe still calls it healthy, which is what makes the cover's \
             answer the one worth drawing"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The key carries the modification time, so an edit that arrived through a
    /// sync client invalidates the cover.
    ///
    /// flip: dropping `entry.modified` from `key` → fails at `assert_ne!`.
    #[test]
    fn editing_a_document_changes_its_cover_key() {
        let root = temp("key");
        let entry = doc_with_a_rect(&root, "Landing v4");
        let before = key(&entry).unwrap();

        // What a sync client does: the same file, a newer stamp.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&entry.path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(later))
            .unwrap();
        let after = key(&scan::scan(&root).into_iter().next().unwrap()).unwrap();

        assert_ne!(before, after);
        // Both still name the same document, so the sweep can tell a superseded
        // cover from a foreign one.
        let id = entry.meta.id.clone().unwrap();
        assert!(before.starts_with(&id) && after.starts_with(&id));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Nothing renders in the pass, and every queued cover arrives. Uses `settle`,
    /// not a sleep, which could stay green against a reintroduced stall.
    ///
    /// flip: `get` rendering inline instead of enqueueing → fails at "nothing
    /// renders in the pass".
    #[test]
    fn covers_are_rendered_off_the_pass_and_all_of_them_arrive() {
        let root = temp("async");
        let first = doc_with_a_rect(&root, "Landing v4");
        let second = doc_with_a_rect(&root, "Pricing table");
        let ctx = egui::Context::default();
        // With a disk cache, so this also runs the write half of `render`.
        let mut covers = Covers::with_dir(root.join("covers"));
        // A pass has to be in flight for `load_texture` to mean anything.
        let _ = ctx.run_ui(Default::default(), |_| {});

        assert!(
            covers.get(&ctx, &first).is_none() && covers.get(&ctx, &second).is_none(),
            "nothing renders in the pass: both are queued and neither is ready"
        );
        assert_eq!(
            covers.queued.len(),
            2,
            "and both were queued, not just the one the old budget allowed"
        );

        covers.settle(&ctx);
        assert!(
            covers.get(&ctx, &first).is_some(),
            "the first arrives once the worker has it"
        );
        assert!(
            covers.get(&ctx, &second).is_some(),
            "and so does the second, in the same pass — which is what the \
             per-pass progress guarantee existed to ration and no longer has to"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A cover that could not be made is remembered as a failure rather than
    /// retried: the key does not go back on the renderer's queue.
    #[test]
    fn a_document_with_no_cover_is_not_retried_every_pass() {
        let root = temp("failed");
        let mut doc = Document::new(IdSource::new(1).mint());
        store::file_document(&root, None, "Untitled", &mut doc).unwrap();
        let entry = scan::scan(&root).into_iter().next().unwrap();
        let ctx = egui::Context::default();
        let mut covers = Covers::default();
        let _ = ctx.run_ui(Default::default(), |_| {});

        assert!(covers.get(&ctx, &entry).is_none(), "queued, not ready");
        covers.settle(&ctx);
        assert!(
            covers.queued.is_empty(),
            "the fixture must reach the state: the answer came back"
        );
        assert!(
            covers.get(&ctx, &entry).is_none(),
            "an empty document still has no cover"
        );
        assert!(
            covers.queued.is_empty(),
            "and the remembered failure is not queued again — which is what \
             stops one blank document occupying the renderer for the session"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A document that has never been filed has no id and therefore no cover
    /// key — it must not fall back to something derived from the path, which
    /// would go stale on the first rename.
    #[test]
    fn an_unfiled_document_has_no_cover_key() {
        let root = temp("unfiled");
        let doc = Document::new(IdSource::new(1).mint());
        std::fs::write(
            root.join("loose.ondin"),
            ondin_core::io::save(&doc).unwrap(),
        )
        .unwrap();
        let entry = scan::scan(&root).into_iter().next().unwrap();
        assert!(entry.meta.id.is_none());
        assert!(key(&entry).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A worker that has already died, built by hand: one job counted and never
    /// answered, its key still in `queued`. `report` is returned when
    /// `answers_hang_up` is false, keeping the answers side connected.
    fn a_dead_worker(
        covers: &mut Covers,
        answers_hang_up: bool,
    ) -> Option<std::sync::mpsc::Sender<(String, Result<Rgba, NoCover>)>> {
        let (tx, jobs) = std::sync::mpsc::channel();
        drop(jobs);
        let (report, done) = std::sync::mpsc::channel();
        covers.render = Some(Renderer {
            tx: Some(tx),
            done,
            in_flight: 1,
            handle: None,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        covers.queued.insert("the-job-that-killed-it".into());
        (!answers_hang_up).then_some(report)
    }

    /// A worker that has died is forgotten, and the next pass spawns another.
    ///
    /// flip: `drain`'s `Disconnected` arm breaking with `false` → fails at "the
    /// dead worker's owed answer is forgotten with it"; that arm clearing `queued`
    /// but not `render` → fails at "a fresh worker draws it".
    #[test]
    fn a_dead_worker_is_forgotten_and_the_next_pass_spawns_another() {
        let root = temp("dead");
        let entry = doc_with_a_rect(&root, "Landing v4");
        let ctx = egui::Context::default();
        let mut covers = Covers::default();
        let _ = ctx.run_ui(Default::default(), |_| {});
        let _ = a_dead_worker(&mut covers, true);

        assert!(covers.get(&ctx, &entry).is_none(), "asked for, not ready");
        assert!(
            !covers.queued.contains("the-job-that-killed-it"),
            "the dead worker's owed answer is forgotten with it"
        );
        covers.settle(&ctx);
        assert!(
            covers.get(&ctx, &entry).is_some(),
            "a fresh worker draws it — the whole of the finding"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A send that fails leaves no key behind: the death met on the sending side,
    /// where the answers channel has not hung up and `drain` sees `Empty`.
    ///
    /// flip: deleting `get`'s `else` arm → fails at "the key is not left in the
    /// queue".
    #[test]
    fn a_send_that_fails_leaves_no_key_behind() {
        let root = temp("sendfail");
        let entry = doc_with_a_rect(&root, "Landing v4");
        let ctx = egui::Context::default();
        let mut covers = Covers::default();
        let _ = ctx.run_ui(Default::default(), |_| {});
        let report = a_dead_worker(&mut covers, false);
        assert!(report.is_some(), "the fixture keeps the answers side open");

        assert!(covers.get(&ctx, &entry).is_none());
        let owed = key(&entry).unwrap();
        assert!(
            !covers.queued.contains(&owed),
            "the key is not left in the queue"
        );
        assert!(covers.render.is_none(), "and the worker is forgotten");
        assert!(covers.get(&ctx, &entry).is_none(), "asked for again");
        covers.settle(&ctx);
        assert!(
            covers.get(&ctx, &entry).is_some(),
            "and the document is drawn"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A panic costs one cover and not the worker: the `POISONED` document
    /// answers `Blank` (no red mark) and the healthy one beside it still arrives.
    ///
    /// flip: calling `render` without the `catch_unwind` → fails at "the healthy
    /// one still arrives".
    #[test]
    fn a_panic_in_one_render_costs_that_cover_and_not_the_worker() {
        let root = temp("panic");
        let poisoned = doc_with_a_rect(&root, "Landing v4");
        let healthy = doc_with_a_rect(&root, "Pricing table");
        let ctx = egui::Context::default();
        let mut covers = Covers::default();
        let _ = ctx.run_ui(Default::default(), |_| {});
        *POISONED.lock().unwrap() = key(&poisoned);

        assert!(covers.get(&ctx, &poisoned).is_none());
        assert!(covers.get(&ctx, &healthy).is_none());
        covers.settle(&ctx);
        *POISONED.lock().unwrap() = None;

        assert!(
            covers.get(&ctx, &healthy).is_some(),
            "the healthy one still arrives"
        );
        assert!(
            covers.get(&ctx, &poisoned).is_none() && !covers.unreadable(&poisoned),
            "the poisoned one has no cover and no mark"
        );
        assert!(
            covers.queued.is_empty(),
            "and it is answered rather than owed, so it is not asked again"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A card drawn every frame sends its document once. `queued.len()` cannot
    /// witness it (a set), so `in_flight`, which counts sends, does.
    ///
    /// flip: moving the send out of `get`'s `if self.queued.insert(..)` block →
    /// fails at "one send, however many frames ask".
    #[test]
    fn a_document_asked_for_every_frame_is_sent_once() {
        let root = temp("dedupe");
        let entry = doc_with_a_rect(&root, "Landing v4");
        let ctx = egui::Context::default();
        let mut covers = Covers::default();
        let _ = ctx.run_ui(Default::default(), |_| {});

        for _ in 0..3 {
            let _ = covers.get(&ctx, &entry);
        }
        // Sends owed plus answers in, so a render finishing between two calls
        // leaves the total alone.
        let sends = covers.render.as_ref().map_or(0, |r| r.in_flight);
        let arrived = covers.covers.len();
        assert_eq!(sends + arrived, 1, "one send, however many frames ask");
        covers.settle(&ctx);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The dashboard's red mark comes through `arrive`, the conversion the app
    /// runs: a refused document is marked and an empty one is not.
    ///
    /// flip: `arrive`'s `Unreadable` arm answering `Cover::Blank` → fails at "the
    /// refused document is marked".
    #[test]
    fn a_document_the_loader_refuses_is_marked_and_an_empty_one_is_not() {
        let root = temp("mark");
        let refused = written_by_a_newer_build(&root);
        let empty_root = temp("mark-empty");
        let mut doc = Document::new(IdSource::new(1).mint());
        store::file_document(&empty_root, None, "Untitled", &mut doc).unwrap();
        let empty = scan::scan(&empty_root).into_iter().next().unwrap();
        let ctx = egui::Context::default();
        let mut covers = Covers::default();
        let _ = ctx.run_ui(Default::default(), |_| {});

        assert!(
            !covers.unreadable(&refused),
            "not known to be broken before it is tried — `false` is not `fine`"
        );
        let _ = covers.get(&ctx, &refused);
        let _ = covers.get(&ctx, &empty);
        covers.settle(&ctx);
        assert!(
            covers.unreadable(&refused),
            "the refused document is marked"
        );
        assert!(!covers.unreadable(&empty), "and the empty one is not");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&empty_root);
    }

    /// `clear` abandons the backlog, and nothing it abandoned arrives later. The
    /// assertion is on the map after a `settle`, since clearing only `queued` also
    /// empties the set.
    ///
    /// flip: `clear` leaving `render` in place → fails at "only the new library's
    /// cover arrives".
    #[test]
    fn clearing_abandons_the_backlog_and_nothing_it_abandoned_arrives() {
        let root = temp("clear");
        let old_a = doc_with_a_rect(&root, "Landing v4");
        let old_b = doc_with_a_rect(&root, "Pricing table");
        let new = doc_with_a_rect(&root, "Onboarding");
        let ctx = egui::Context::default();
        let mut covers = Covers::default();
        let _ = ctx.run_ui(Default::default(), |_| {});

        let _ = covers.get(&ctx, &old_a);
        let _ = covers.get(&ctx, &old_b);
        covers.clear();
        assert!(
            covers.queued.is_empty(),
            "the set empties — which the wrong version does too"
        );

        let _ = covers.get(&ctx, &new);
        covers.settle(&ctx);
        assert_eq!(
            covers.covers.len(),
            1,
            "only the new library's cover arrives"
        );
        assert!(covers.get(&ctx, &new).is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The sweep keeps what the list claims, deletes what it does not, and deletes
    /// nothing on an empty list (an unplugged drive scans as no documents).
    ///
    /// flip: removing the `live.is_empty()` guard in `sweep` → fails at "an empty
    /// list is not evidence".
    #[test]
    fn the_sweep_keeps_what_the_list_claims_and_an_empty_list_claims_nothing() {
        let root = temp("sweep");
        let kept = doc_with_a_rect(&root, "Landing v4");
        let dir = root.join("covers");
        std::fs::create_dir_all(&dir).unwrap();
        let covers = Covers::with_dir(dir.clone());
        let plant = |name: &str| std::fs::write(dir.join(name), b"x").unwrap();
        let kept_png = format!("{}.png", key(&kept).unwrap());

        plant(&kept_png);
        plant("superseded.png");
        plant("notes.txt");
        covers.sweep(std::slice::from_ref(&kept));
        assert!(dir.join(&kept_png).exists(), "a claimed cover stays");
        assert!(
            !dir.join("superseded.png").exists(),
            "an unclaimed one goes"
        );
        assert!(
            dir.join("notes.txt").exists(),
            "and nothing but PNGs is touched"
        );

        plant("superseded.png");
        covers.sweep(&[]);
        assert!(
            dir.join(&kept_png).exists() && dir.join("superseded.png").exists(),
            "an empty list is not evidence that the documents are gone"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Past the cap the least recently asked-for cover goes, and one asked for on
    /// this pass or the one before never does; asking refreshes the stamp. Answers
    /// go in through `arrive` as 1×1 pictures, since the bookkeeping is under test.
    ///
    /// flip: dropping `get`'s `*seen = now` → fails at "a cover a card asked for
    /// this pass outlives every older one".
    #[test]
    fn past_the_cap_the_least_recently_asked_cover_goes_and_a_recent_one_never_does() {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut covers = Covers::default();
        let pixel = || {
            Ok(Rgba {
                pixels: vec![0; 4],
                width: 1,
                height: 1,
            })
        };
        let ready = |c: &Covers| {
            c.covers
                .values()
                .filter(|c| matches!(c, Cover::Ready(..)))
                .count()
        };
        let resident = |c: &Covers, k: &str| matches!(c.covers.get(k), Some(Cover::Ready(..)));

        // Oldest first.
        covers.pass(&ctx);
        for i in 0..MAX_RESIDENT_COVERS {
            covers.arrive(&ctx, format!("old-{i:03}"), pixel());
        }
        covers.pass(&ctx);
        covers.pass(&ctx);
        for i in 0..10 {
            covers.arrive(&ctx, format!("new-{i}"), pixel());
        }
        assert_eq!(
            ready(&covers),
            MAX_RESIDENT_COVERS + 10,
            "the fixture: ten over"
        );
        covers.pass(&ctx);
        assert_eq!(ready(&covers), MAX_RESIDENT_COVERS, "back to the cap");
        assert!(
            (0..10).all(|i| !resident(&covers, &format!("old-{i:03}"))),
            "the ten that go are the ten asked for longest ago"
        );
        assert!(
            resident(&covers, "old-010") && (0..10).all(|i| resident(&covers, &format!("new-{i}"))),
            "and nothing newer goes with them"
        );

        // Never a recent one: a burst bigger than the cap, all on one pass.
        let burst = MAX_RESIDENT_COVERS + 72;
        for i in 0..burst {
            covers.arrive(&ctx, format!("burst-{i:03}"), pixel());
        }
        covers.pass(&ctx);
        assert!(
            (0..burst).all(|i| resident(&covers, &format!("burst-{i:03}"))),
            "a cover from the previous pass is never evicted, even past the cap"
        );
        assert_eq!(
            ready(&covers),
            burst,
            "so a screen of more than the cap exceeds it rather than churning"
        );

        // Asking refreshes: a real document, asked for through `get`.
        let root = temp("evict");
        let entry = doc_with_a_rect(&root, "Landing v4");
        let _ = covers.get(&ctx, &entry);
        covers.settle(&ctx);
        let k = key(&entry).expect("a filed document has a key");
        assert!(resident(&covers, &k), "the fixture: its cover is in");
        for _ in 0..5 {
            covers.pass(&ctx);
        }
        for i in 0..MAX_RESIDENT_COVERS {
            covers.arrive(&ctx, format!("late-{i:03}"), pixel());
        }
        let _ = covers.get(&ctx, &entry);
        covers.pass(&ctx);
        assert!(
            resident(&covers, &k),
            "a cover a card asked for this pass outlives every older one"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `pass` takes the worker's answers, and the mark they carry, whether or not
    /// a card asks again (the list view asks for no covers). Waits on a deadline,
    /// not a sleep.
    #[test]
    fn a_pass_takes_the_answers_a_card_never_came_back_for() {
        let root = temp("pass-drain");
        let refused = written_by_a_newer_build(&root);
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut covers = Covers::default();

        let _ = covers.get(&ctx, &refused);
        assert!(!covers.unreadable(&refused), "the fixture: not known yet");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !covers.unreadable(&refused) && std::time::Instant::now() < deadline {
            covers.pass(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            covers.unreadable(&refused),
            "a pass with no card asking still takes the answer and its mark"
        );
        covers.settle(&ctx);
        let _ = std::fs::remove_dir_all(&root);
    }
}
