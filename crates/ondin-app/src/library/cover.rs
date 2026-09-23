//! A document's cover: the picture the dashboard's grid draws on a card (§15
//! D366).
//!
//! **Rendered by the same headless path `ondin export --png` uses**, so a cover
//! is the document rather than an impression of it — `Resolved::rebuild` and
//! `ondin_export::png`, with no GPU and no window. That matters more than it
//! sounds: a hand-drawn approximation would be one more thing to keep in step
//! with the renderer, and would be wrong about exactly the documents whose
//! appearance is interesting.
//!
//! ⚠️ **Rendering a document is expensive and there can be sixty of them.** So
//! there are two caches with different jobs and different lifetimes:
//!
//! - **On disk**, under `dirs::cache_dir()/ondin/covers`, keyed by document id
//!   *and* modification time. This is what survives a restart, and the mtime in
//!   the key is what makes it correct rather than merely fast: a document edited
//!   on another machine and synced in has a new mtime, so it gets a new key and
//!   the stale picture is never shown.
//! - **In memory**, as egui textures.
//!
//! 🚨 **And the rendering happens on a thread of its own** (§15 D820). This said
//! *"with a per-pass time budget so opening the dashboard on a large library does
//! not stall the frame that first shows it"*, and the budget did not do that: the
//! *progress guarantee* under it rendered one document per pass whatever the
//! budget said — without which a library of documents each slower than 8 ms would
//! draw no covers ever — so the cost was **one stall per pass** until the covers
//! were built rather than one long freeze, which is the worse-feeling of the two.
//! A single picture's decode was measured at 433 ms (§15 D449). `thumbs::ImageThumbs`
//! and `fonts::FontPreviews` keep their budgets and are *not* the same shape any
//! more: both do work that really is in the frame, and §15 D160 is where those
//! numbers were argued.
//!
//! **The disk cache is genuinely disposable.** Delete it and the covers redraw;
//! nothing about a document lives here. That is why it is in the cache directory
//! and not in the base folder, and it is the one piece of the library that is
//! deliberately *not* made to sync — a picture that can be rebuilt in a second
//! is not worth uploading.

use super::ids;
use super::scan::Entry;
use eframe::egui;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The longest edge of a cover, in pixels.
///
/// **Sized for the card it fills at 2× display scaling** — the grid's cards are
/// around 170pt wide and 116pt tall — rather than for the document. A cover is a
/// thumbnail; asking the renderer for more is paying for pixels the card cannot
/// show, on every document in the library.
const COVER_MAX_PX: f64 = 400.0;

// 🚨 **There was a `FRAME_BUDGET` here and it is gone with what it bounded**
// (§15 D820). Eight milliseconds — larger than `ImageThumbs`' two, on the
// argument that this path runs the whole document→pixels walk rather than
// decoding an image already in memory — and it never bounded the thing anyone
// felt: the *progress guarantee* rendered one document per pass whatever the
// budget said, so a library of slow documents cost one stall per pass rather
// than none. Both were consequences of rendering **inside** the pass. The render
// is a thread now, so the pass spends nothing and there is nothing left to
// ration; `ImageThumbs`' own 2 ms budget is untouched and still bounds a decode
// that really does happen in the frame.

/// What a cover key is: the document's identity plus the state of its file.
///
/// ⚠️ **Both halves, and the mtime is the one that is easy to leave out.** Keyed
/// on the id alone, a cover would be drawn once and then be wrong for the rest of
/// the install — including the case this library is built for, where the edit
/// happened on another machine and arrived through a sync client that never told
/// the app anything.
///
/// ⚠️ **The id is checked, and this is the site that needs it least and matters
/// most.** The key becomes a *filename* under the cover cache, so an id holding
/// `../` writes a PNG outside it — and this function runs for **every card the
/// dashboard draws**, so it is the one door in the class that opens without the
/// user clicking anything. A document with a malformed id simply gets no cached
/// cover, which is the same state a document with no id already has.
fn key(entry: &Entry) -> Option<String> {
    let id = entry.meta.id.as_deref().filter(|id| ids::is_minted(id))?;
    Some(format!("{id}-{}", entry.modified))
}

/// The cover cache's directory, as a [`Covers`] made by `Default` is given it.
///
/// ⚠️ **`None` under `cfg(test)`, and that is a seam rather than a skipped
/// path** (§15 D845, `[X1.1-L6-02]`). The suite used to write real covers into
/// the developer's cache directory, and — the half that mattered — it could
/// not test [`Covers::sweep`] at all, since the only directory `sweep` knew was
/// that one and a test of a function that deletes files would have deleted the
/// developer's. §15 D807 built the same seam for the per-machine index, the file
/// that is *written*; this is the file that is *deleted*. A test that wants the
/// disk names its own directory through `Covers::with_dir`, and runs the same
/// code the app does against it.
fn covers_dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(dirs::cache_dir()?.join("ondin").join("covers"))
}

/// Where a cover lives on disk.
fn cache_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.png"))
}

/// One cover, as the map holds it.
///
/// 🚨 **The two failures used to be one variant, and telling them apart is what
/// gives the dashboard something true to draw** (§15 D613, `[S1.3-L3-05]`). A
/// document that will not load and a document with nothing in it both produce no
/// cover and both want the card's plain plate — but only one of them is a
/// **broken file**, and `Entry::unread`, the flag that was supposed to say so, can
/// only answer for a file whose first 4 KB fail to parse. This is where the real
/// answer already is: `io::load` runs here, over the whole file, once per document
/// per session, and the result was being thrown away.
enum Cover {
    /// Rendered and uploaded.
    Ready(egui::TextureHandle),
    /// The file could not be read or would not load. **Remembered**, so a broken
    /// document is not re-parsed on every pass for the life of the session.
    Unreadable,
    /// Loaded perfectly and has no extent — an empty document, which is an
    /// ordinary state and not a mark to put on a card.
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
/// 🚨 **The render used to happen inside the egui pass** (§15 D820). `get` →
/// [`render`] → `io::load` → `png::png` → `ImageStore::prepare` is the whole
/// document→pixels path, and a picture's decode alone was measured at **433 ms**
/// for an 8000² image (§15 D449). The 8 ms `FRAME_BUDGET` did not bound it: the
/// progress guarantee rendered one document per pass *whatever the budget said*,
/// without which a library of slow documents would draw no covers ever — so the
/// cost was one stall per pass until the covers were built, which is the
/// worse-feeling of the two. D449 caps how large a *picture* may become and
/// deliberately does not fix this, a 50 MP camera file being 240 MB of RGBA and
/// legitimate.
///
/// **It is a thread now, and that is what makes both the budget and the progress
/// guarantee unnecessary.** Nothing in the pass renders; `get` enqueues and
/// [`Self::drain`] uploads what came back.
///
/// **`Default` is hand-written again, for a different reason** (§15 D845). It
/// was hand-written to seed a budget from the 8 ms `FRAME_BUDGET`, derived once
/// §15 D820 deleted that, and is written out now to put [`covers_dir`] in
/// `dir` — the one field whose default is not its type's.
pub struct Covers {
    covers: HashMap<String, Cover>,
    /// Keys handed to the renderer and not yet answered, so a card drawn every
    /// frame enqueues its document once rather than sixty times a second.
    ///
    /// **Separate from `covers` rather than a `Cover::Pending` variant**, because
    /// every reader of that map asks a question about a *finished* answer —
    /// [`Self::unreadable`] most of all — and a fourth variant would put "not yet"
    /// into three `match`es that have no use for it.
    queued: std::collections::HashSet<String>,
    /// The renderer, spawned on the first miss.
    ///
    /// Lazily, for `library::writer::Writer::spawn`'s reason: an app that never
    /// opens the dashboard — or a headless probe that never draws a card — pays
    /// for no thread.
    ///
    /// 🚨 **`None` again whenever the worker is known to be gone**, and that is
    /// what makes a death recoverable rather than permanent (§15 D845,
    /// `[X1.1-L1-01]`). The next miss spawns a fresh one.
    render: Option<Renderer>,
    /// The disk cache's directory; `None` is no disk cache, which is what
    /// [`covers_dir`] answers when there is no cache directory at all and what
    /// every test gets unless it names one.
    dir: Option<PathBuf>,
}

impl Default for Covers {
    fn default() -> Self {
        Self {
            covers: HashMap::new(),
            queued: std::collections::HashSet::new(),
            render: None,
            dir: covers_dir(),
        }
    }
}

/// The cover renderer's thread, and the answers coming back.
///
/// **A second worker rather than a job on `library::writer::Writer`'s queue**
/// (§15 D820), and the reason is that module's own contract: it is *"one
/// background writer of anything, in **FIFO** order"*, and the FIFO is
/// load-bearing twice — a `Forget` must not be overtaken by the `Snapshot` it
/// cancels, and a save must not be overtaken by anything. A cover render is
/// hundreds of milliseconds of work that nothing is waiting on; putting one in
/// front of an autosave would make the feature that protects work wait behind the
/// feature that decorates a card.
///
/// ⚠️ **Nothing here writes a record.** `render` may write a PNG into the cover
/// cache, which `crate::atomic` deliberately **exempts** as derived data — a torn
/// one is regenerated from the document it came from — so the second writer this
/// adds is not a second writer of anything that module protects.
struct Renderer {
    /// `None` only while [`Drop`] is taking it; dropping the sender ends the
    /// worker's loop.
    tx: Option<std::sync::mpsc::Sender<(Entry, String)>>,
    done: std::sync::mpsc::Receiver<(String, Result<Rgba, NoCover>)>,
    /// Jobs sent and not yet answered — `Writer::in_flight`'s field, for
    /// `Covers::settle`'s sake and for the same reason: a `Receiver` cannot say
    /// how many sends are still in the worker's hands. (Plain backticks: that
    /// method is `cfg(test)`, so a production doc may not link it — §15 D319, and
    /// the doc gate exits 101 on one that tries, which is how this was caught.)
    in_flight: usize,
    handle: Option<std::thread::JoinHandle<()>>,
    /// Set by [`Drop`], and read by the worker before each job.
    ///
    /// 🚨 **Without it the join waited on the whole backlog, not the current
    /// item** (§15 D845, `[X1.1-L1-03]`). Dropping the sender does not stop the
    /// worker's `for` loop until every job *already buffered* has been taken, so
    /// a library of sixty uncached documents made the join sixty renders long —
    /// harmless only because nothing ever dropped a `Renderer`, which was the
    /// other half of the same finding. Now that [`Covers::clear`] does, the
    /// backlog is abandoned and the join is one render at most.
    cancel: Arc<AtomicBool>,
}

/// The one cover key the worker will panic on instead of rendering.
///
/// ⚠️ **The fault has to be injected, for `library::writer`'s `POISONED`
/// reason and in its shape**: no panic in `io::load` → `Resolved::rebuild` →
/// `png::png` is known, and the whole of `[X1.1-L1-01]` is what the renderer
/// does once there is one. Keyed on a cover key rather than a bare one-shot, so
/// a fault armed by one test cannot fire in another that happened to render
/// next — cover keys carry a freshly minted document id.
///
/// Plain backticks: this item is `cfg(test)`, and a link on it resolves against
/// nothing under `cargo doc` (§15 D319).
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
                // 🚨 **A panic costs one cover and not the worker** (§15 D845,
                // `[X1.1-L1-01]`). Uncaught, it ended this thread, and the pass
                // read the hung-up channel as an idle one: that document's key
                // sat in `queued` forever, every *other* document's send then
                // failed against the dropped receiver with its key left in
                // `queued` too, and no cover appeared again all session, with no
                // word said. `boolean::guarded` is the precedent for catching
                // one here, and `Cargo.toml`'s `panic = "abort"` note is why
                // that is sound in this workspace.
                //
                // ⚠️ **`Blank`, not `Unreadable`**, for the reason `render`
                // gives a PNG it cannot decode: a panic is a fault in this build
                // and says nothing about the file, so it is not a red mark to
                // put on the card. The document may open perfectly.
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
                // **`request_repaint`, for `crate::fonts::FontService::new`'s
                // reason**: eframe here is reactive, so without it a cover
                // finished while the pointer is still would sit in the channel
                // until something else woke the app up.
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
    /// **Joined rather than detached**, so a renderer that goes away — the base
    /// folder changed — does not leave a thread rendering covers for a library
    /// nobody is looking at.
    ///
    /// ⚠️ **This said *"a `Covers` that goes away"*, and no `Covers` ever does**
    /// (§15 D845, `[X1.1-L1-03]`): `OndinApp::covers` is built once and never
    /// reassigned, and eframe drops nothing at exit. What goes away is the
    /// `Renderer`, taken out by [`Covers::clear`] — and the join waits on one
    /// render rather than the backlog because `cancel` is set first.
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
    /// The two are deliberately not distinguished, exactly as in
    /// `ImageThumbs::get`: the caller does the same thing either way, which is to
    /// draw the plain plate it drew before covers existed. A `None` that means
    /// "later" also asks for a repaint, so later arrives.
    pub fn get(&mut self, ctx: &egui::Context, entry: &Entry) -> Option<&egui::TextureHandle> {
        self.drain(ctx);
        let key = key(entry)?;
        if self.covers.contains_key(&key) {
            return match self.covers.get(&key) {
                Some(Cover::Ready(t)) => Some(t),
                _ => None,
            };
        }
        // **Enqueued once.** A card is drawn on every frame it is on screen, and
        // without `queued` a slow document would be handed to the renderer sixty
        // times a second — a queue that grows faster than it drains and a thread
        // that never reaches the second document.
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
                // 🚨 **A send that fails means the worker has gone, and the key
                // must not stay behind** (§15 D845, `[X1.1-L1-01]`). This arm
                // did nothing: the key sat in `queued` with no answer owed, so
                // the `insert` above answered `false` on every later pass and
                // the document was never asked for again. `drain` catches a
                // death on the *receiving* side; this is the same death met a
                // moment later on the sending side, and it gets the same
                // repair — forget the worker, so the next pass spawns one.
                self.queued.remove(&key);
                self.render = None;
            }
        }
        None
    }

    /// Take what the renderer has finished and turn it into textures.
    ///
    /// **Called from [`Self::get`] rather than once per frame by the dashboard**,
    /// which is the cheaper of the two and the one that cannot be forgotten: the
    /// only thing that ever wants a cover is a card asking for one, and a pass
    /// that draws no cards has nothing to upload.
    ///
    /// ⚠️ **The texture upload stays on this thread and has to.** `load_texture`
    /// needs the `Context`, which is not `Send` to a worker in any useful sense —
    /// and it is the cheap half: what cost 433 ms was the decode, which is now the
    /// worker's, and what is left is an upload of a picture bounded by
    /// [`COVER_MAX_PX`].
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
                // 🚨 **A hung-up channel is not an idle one** (§15 D845,
                // `[X1.1-L1-01]`). This was `while let Ok(..)`, which leaves on
                // both — the defect §15 D549 fixed in `library::writer::Writer`'s
                // own `drain` a fortnight before this worker was modelled on it.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break true,
            }
        };
        for (key, answer) in arrived {
            self.arrive(ctx, key, answer);
        }
        if gone {
            // **Every key still queued was owed by the dead worker**, so none of
            // them will ever be answered; forgetting them and the worker is what
            // lets the next pass ask again, of a fresh one. The job that killed
            // it — if it was a job — is asked again too, and meets the
            // `catch_unwind` this time, so a respawn cannot loop.
            self.queued.clear();
            self.render = None;
        }
    }

    /// One answer from the renderer, into the map — **the only place an answer
    /// becomes a [`Cover`]**.
    ///
    /// 🚨 **There were two** (§15 D845, `[R2-L6-02]`): `drain` and the test-only
    /// `settle`, each with its own copy of this `match`. Every cover test reads
    /// through `settle`, so `drain`'s copy — the one the app runs — was executed
    /// by no test at all: turning its `Unreadable` arm into `Blank` stayed green
    /// and would have quietly taken the dashboard's red mark away. **One
    /// conversion makes a test through `settle` a test of the app's**, which is
    /// the whole of `settle`'s own argument for existing.
    fn arrive(&mut self, ctx: &egui::Context, key: String, answer: Result<Rgba, NoCover>) {
        self.queued.remove(&key);
        let cover = match answer {
            Ok(rgba) => {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [rgba.width as usize, rgba.height as usize],
                    &rgba.pixels,
                );
                Cover::Ready(ctx.load_texture(
                    format!("cover-{key}"),
                    image,
                    egui::TextureOptions::LINEAR,
                ))
            }
            Err(NoCover::Unreadable) => Cover::Unreadable,
            Err(NoCover::Blank) => Cover::Blank,
        };
        self.covers.insert(key, cover);
    }

    /// Wait for every queued cover and take the answers.
    ///
    /// ⚠️ **For tests, and it is the reason there is only one production path.**
    /// `library::writer::Writer::settle` is the precedent and the argument is the
    /// same: an asynchronous feature whose test drove a *synchronous* variant
    /// would be testing code the app does not run. So the app is always
    /// asynchronous and a probe that wants an answer asks for one here.
    ///
    /// ⚠️ **`#[cfg(test)]` rather than `pub`**, which is D672's answer and not
    /// D699's: nothing in production waits for a cover, so a `pub` one is an item
    /// `dead_code` **does** analyse — `ondin-app` declares only a `[[bin]]` and
    /// has no lib target — and it warned. No production doc links it, so the
    /// narrow `#[allow]` D699 is about would be the wrong tool here.
    ///
    /// ⚠️ **"Nothing in production waits for a cover" is still true, and the
    /// migration that looked like it falsified it cancels instead** (§15 D845,
    /// `[X1.1-L1-02]`): the answers in flight at a base-folder change are about
    /// paths that are about to stop existing, so `clear` discards them rather
    /// than waiting for them.
    ///
    /// Plain backticks throughout: this item is `cfg(test)`, so `cargo doc`
    /// cannot see it and a link on it resolves against nothing (§15 D319).
    #[cfg(test)]
    pub(crate) fn settle(&mut self, ctx: &egui::Context) {
        while self.render.as_ref().is_some_and(|r| r.in_flight > 0) {
            let answer = self.render.as_ref().and_then(|r| r.done.recv().ok());
            let Some((key, answer)) = answer else {
                // The worker is gone; nothing else will arrive, and counting on
                // is a hang rather than a wait. `drain` is what forgets it, so
                // the state a test sees after this is the one the next pass
                // would.
                self.drain(ctx);
                break;
            };
            if let Some(r) = self.render.as_mut() {
                r.in_flight = r.in_flight.saturating_sub(1);
            }
            self.arrive(ctx, key, answer);
        }
    }

    /// A `Covers` whose disk cache is `dir` — the seam `covers_dir`'s doc
    /// describes, for the tests that need the disk half.
    #[cfg(test)]
    pub(crate) fn with_dir(dir: PathBuf) -> Self {
        Self {
            dir: Some(dir),
            ..Self::default()
        }
    }

    /// Whether this document has been **found unloadable** — `io::load` was run
    /// over the whole file to draw its cover and refused it.
    ///
    /// 🚨 **This is the honest half of `[S1.3-L3-05]` and `Entry::unread` is the
    /// cheap half** (§15 D613). That flag is set only through
    /// `MetaProbe::Inconclusive`, so a file whose first `PREFIX_BYTES` = 4096
    /// parse cleanly answers `Found` or `Absent` and is reported healthy **without
    /// the rest of it being looked at at all**. The whole-file answer already
    /// existed one call away.
    ///
    /// ⚠️ **`false` is "not known to be broken", not "fine".** The cover cache is
    /// lazy and rendered on a worker, so a document whose cover has not been
    /// attempted yet answers `false` — which is why the dashboard asks
    /// `entry.unread || this`, and why the mark can arrive a pass late on a large
    /// library. `get` requests a repaint, so late arrives.
    ///
    /// ⚠️ **This said *"per-pass budgeted"* until §15 D844.** That was the
    /// mechanism before §15 D820 deleted `FRAME_BUDGET` and moved the render off
    /// the UI thread; the *conclusion* — that `false` is not a health claim — is
    /// unchanged, which is exactly why the stale premise was easy to leave
    /// standing under a sentence that still reads correctly.
    ///
    /// **Cheap on purpose**: a map lookup on the key `get` already built, and no
    /// parsing of its own. Everything that reads this is drawing.
    pub fn unreadable(&self, entry: &Entry) -> bool {
        key(entry).is_some_and(|k| matches!(self.covers.get(&k), Some(Cover::Unreadable)))
    }

    /// Forget every texture — after the base folder changes, when the documents
    /// on screen are a different set entirely.
    ///
    /// The *disk* cache is not cleared **by this function**, and it is keyed by
    /// document id, so an entry stays correct across a move.
    ///
    /// 🚨 **It used to say that made the new folder's covers "appear immediately
    /// if they have been seen before", and that is false** (§15 D708,
    /// `[S1.3-L8-07]`). [`Self::sweep`] runs on the same transition — the next
    /// `go_to_dashboard` — and its keep set is built from the **current**
    /// library alone, over one shared directory that nothing in the path or the
    /// key namespaces per library. So switching from library A to B and entering
    /// the dashboard deletes every one of A's cached covers, and switching back
    /// re-renders every one of them on the worker. (That read *"at one document
    /// per pass"* until §15 D845 — a fourth copy of the rationing §15 D820
    /// deleted, in the paragraph §15 D844 amended for the third.) The claim was true of
    /// *this function* and false of the program, which is the worst shape a doc
    /// comment has: correct about its own three lines and wrong about what
    /// happens.
    ///
    /// ⚠️ **The cost is bounded and the sentence was not.** Covers are
    /// deliberately disposable (§15 D366) and the render costs the **pass**
    /// nothing at all, being a worker thread's work since §15 D820 — so the
    /// damage is a slowly filling grid rather than a wrong picture, which is
    /// why this is a corrected sentence and not a rewritten cache.
    ///
    /// 🚨 **That clause read *"the render budget is 8 ms a pass with a
    /// one-render progress guarantee"* until §15 D844, and it is the
    /// load-bearing half of this paragraph** — it is the entire cost argument
    /// for why §15 D708 was answered by correcting a sentence rather than by
    /// namespacing the cache. D820 deleted `FRAME_BUDGET`, `Covers::spent` and
    /// `Covers::budget` **in the same range that amended `architecture.md` to
    /// say so**, and left this copy standing. So the verdict survives and its
    /// stated evidence had expired: the next person weighing D708's two named
    /// repairs below would have read a cost model measured against rationing
    /// that no longer exists, found the damage still bounded, and left the cache
    /// alone on the strength of it. **The conclusion is *more* true now — a pass
    /// that spends nothing is a better bound than a pass that spends eight
    /// milliseconds** — which is exactly what makes a stale premise under a
    /// surviving verdict so easy to walk past.
    ///
    /// **Two ways to make the old claim true, neither taken here.** Namespace
    /// the directory by a hash of the library root, which would also stop two
    /// libraries holding the same synced document from sharing an entry; or
    /// sweep on an *age* bound rather than on membership, so a cover no current
    /// document claims survives a switch and a genuinely dead one still goes.
    /// Both change eviction semantics and the first changes the cache layout, so
    /// they are decisions rather than repairs.
    ///
    /// 🚨 **And it is the renderer's teardown, which it did not used to be**
    /// (§15 D845, `[X1.1-L1-02]`, `[X1.1-L1-03]`). This cleared the map and
    /// nothing else, so after a base-folder change the worker went on rendering
    /// the *old* library's backlog — refilling the map this had just emptied, and
    /// putting the new library's covers behind all of it. Worse, with *Move my
    /// existing files there* on, a job queued before the move read its document
    /// at the pre-move path, failed, and cached `Unreadable` under a key that is
    /// path-independent by design (§15 D509) — so the moved, healthy document
    /// wore the red *"will not open"* mark for the session. **Dropping the
    /// renderer discards every answer still owed**, and the caller does this
    /// *before* `relocate` so no job is reading a file while it moves.
    pub fn clear(&mut self) {
        self.covers.clear();
        self.queued.clear();
        self.render = None;
    }

    /// Delete cached covers on disk that no live document claims.
    ///
    /// ⚠️ **Sweeps by key, which is id *and* mtime, so this is also what removes
    /// the superseded covers of documents that are still here.** Without it the
    /// folder grows by one PNG per save, forever — the failure mode a cache keyed
    /// on a changing value always has, and the one nothing on screen would ever
    /// show.
    ///
    /// 🚨 **An empty list sweeps nothing** (§15 D845, `[X1.1-L1-04]`). A root
    /// that could not be listed — a network share that is down, a USB drive that
    /// is out — scans as no documents at all, and this deleted every PNG in the
    /// directory: which is one directory for **every** library on the machine
    /// (§15 D708), so another window's healthy library lost its covers to this
    /// one's unplugged drive. §15 D384 is the rule, and `Library::refresh` already
    /// applied it to the other membership sweep in the same function: *an empty
    /// list is not evidence that the documents are gone*. **Here rather than at
    /// the call site** so no future caller can forget it, and the only cost is an
    /// honestly empty library keeping covers nothing will draw until its first
    /// document arrives.
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
    // Falling through covers two cases and treats them alike: no cached file,
    // and a cached file that will not decode — a half-written PNG from a run
    // that was killed. Both mean "render it again", and re-rendering writes over
    // the bad one.
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

/// Render the document to a PNG, framed on everything it contains.
/// ⚠️ **The two `None`s were the same `None` and they are not the same fact**
/// (§15 D613, `[S1.3-L3-05]`): a file that will not read or will not load is a
/// broken document the dashboard has to mark, and a document with no extent is an
/// ordinary empty one.
fn rasterize(entry: &Entry) -> Result<Vec<u8>, NoCover> {
    let bytes = std::fs::read(&entry.path).map_err(|_| NoCover::Unreadable)?;
    let doc = ondin_core::io::load(&bytes).map_err(|_| NoCover::Unreadable)?;
    let res = ondin_core::Resolved::rebuild(&doc);
    // An empty document has no extent and gets no cover — the card's plain plate
    // is the honest picture of one, and inventing a 512-square of canvas ground
    // would draw sixty identical rectangles on a library of fresh files.
    let view = ondin_export::extent(&res, &[doc.root()]).ok_or(NoCover::Blank)?;
    let (w, h) = (view.width().max(1.0), view.height().max(1.0));
    // Fit the longest edge, never magnify: a 40×40 icon document should be drawn
    // at 40 pixels and centred by the card, not blown up to 400 and blurred.
    let scale = (COVER_MAX_PX / w.max(h)).min(1.0);
    let vp = ondin_export::png::viewport_at(view, scale);
    Ok(ondin_export::png::png(&doc, &res, &vp))
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

/// ⚠️ **These tests used to write real covers into the user's cache directory**,
/// because the cache path resolved `dirs::cache_dir()` with no seam to point it
/// anywhere else — which also meant `Covers::sweep`, the one function here that
/// deletes files, could not be tested at all. `covers_dir` is that seam
/// (§15 D845): a `Covers::default()` has no disk cache under test, and a test
/// that wants one names a directory of its own.
///
/// Plain backticks in this module's prose, including this doc: the module is
/// `cfg(test)`, so `cargo doc` cannot see it (§15 D319). This doc carried two
/// links until §15 D845 — `cache_path` and `Covers::sweep` — which is the
/// population CLAUDE.md records as zero, and the item-versus-module shape
/// §15 D827's *Fix* names: the *doc on* a `cfg(test)` module is as invisible as
/// the prose inside it.
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

    /// A document `io::load` refuses and the metadata probe calls healthy — one
    /// number changed, structurally perfect: a file written by a **newer build**.
    /// `a_document_the_loader_refuses_is_unreadable_where_an_empty_one_is_merely_blank`
    /// says why this and not a truncation.
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

    /// ⚠️ **An empty document gets no cover, and that is a decision.** Returning
    /// a blank raster would draw an identical grey rectangle on every card of a
    /// fresh library — worse than the plate, because it looks like a preview.
    #[test]
    fn an_empty_document_has_no_cover() {
        let root = temp("empty");
        let mut doc = Document::new(IdSource::new(1).mint());
        store::file_document(&root, None, "Untitled", &mut doc).unwrap();
        let entry = scan::scan(&root).into_iter().next().unwrap();
        // ⚠️ **`Blank` and not merely "no cover"** (§15 D613, `[S1.3-L3-05]`).
        // This asserted `is_none()` and could not have told the two failures
        // apart — which is why an empty document used to get the same answer as a
        // corrupt one, and why the dashboard had nothing true to draw a mark from.
        assert_eq!(rasterize(&entry).err(), Some(NoCover::Blank));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A document that will not load says so, and an empty one does not**
    /// (§15 D613, `[S1.3-L3-05]`).
    ///
    /// 🚨 **The dashboard had nothing true to read.** `Entry::unread` is the flag
    /// that was supposed to mark a broken file, and it is written only on
    /// `MetaProbe::Inconclusive`; the `Found` and `Absent` arms report a document
    /// healthy **without the rest of it being looked at at all**.
    ///
    /// ⚠️ **The finding's own account of the gap does not survive reading
    /// `read_meta`, and the real one is worse.** It names a document *truncated*
    /// past its `meta` block — but a truncation makes `scan_value` fail on the
    /// `root` key, which is `Inconclusive`, and that arm **reads the whole file
    /// and runs `io::load`**. Truncation was already caught. What is not caught is
    /// a document whose JSON is **well-formed** and which `io::load` nonetheless
    /// refuses: the probe walks to `root`, answers `Found`, and nothing ever
    /// parses it. A file written by a *newer build* is exactly that — one number
    /// changed, structurally perfect, and `migrate`'s `version > CURRENT` guard
    /// refuses it — and a synced library is precisely where one appears.
    ///
    /// **So the fixture is a version bump and not a cut**, and the `unread`
    /// assertion is what says the probe is still answering *healthy*: without it
    /// this test would pass against a build where the flag already covered the
    /// case, and prove nothing about why the cover's answer is needed.
    ///
    /// ⚠️ **Flip-check, run**: mapping `io::load`'s error to `NoCover::Blank` —
    /// which is what the single `Failed` variant amounted to — fails the first
    /// assertion with `left: Some(Blank)  right: Some(Unreadable)`. And returning
    /// `Unreadable` for a missing extent instead fails
    /// `an_empty_document_has_no_cover` above, which is the control this one needs
    /// and is why the two sit together.
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

    /// ⚠️ **The key carries the modification time**, so an edit invalidates the
    /// cover rather than being papered over by it — including an edit that
    /// arrived through a sync client, which is the case the id alone gets wrong.
    ///
    /// Flip-check, run: dropping `entry.modified` from `key` makes the two keys
    /// equal and fails here. Nothing else in this module notices, which is the
    /// shape that says this is the only thing guarding staleness.
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
        // And both still name the same document, which is what makes the sweep
        // able to tell a superseded cover from a foreign one.
        let id = entry.meta.id.clone().unwrap();
        assert!(before.starts_with(&id) && after.starts_with(&id));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **Nothing renders in the pass, and every queued cover arrives** (§15
    /// D820).
    ///
    /// 🚨 **This test replaces `one_cover_a_pass_arrives_however_spent_the_budget_is`,
    /// and the rule it pinned is gone rather than changed.** That one asserted
    /// the progress guarantee — *one render a pass whatever the budget says* —
    /// which existed because the render happened **in** the pass and a library of
    /// slow documents would otherwise draw no covers ever. With the render on a
    /// thread there is nothing in the frame to budget: both documents are queued
    /// on the first pass and both come back.
    ///
    /// ⚠️ **The first assertion is the one with teeth and it is a negative.**
    /// `get` returning `None` for a document whose file is sitting right there is
    /// the whole change; a version that rendered inline would answer `Some`
    /// immediately and fail here.
    ///
    /// ⚠️ **`settle` rather than a sleep or a frame count.** A timing-based wait
    /// would be a flake on a loaded machine and, worse, a *green* one on a fast
    /// machine that had reintroduced the stall — see `CLAUDE.md` on the recovery
    /// test whose red run was correct behaviour for a state the clock had picked.
    ///
    /// **Flip-check, run** by making `get` render inline again (`render(entry,
    /// &key)` in place of the enqueue): fails at *"nothing renders in the pass"*,
    /// the predicted site.
    #[test]
    fn covers_are_rendered_off_the_pass_and_all_of_them_arrive() {
        let root = temp("async");
        let first = doc_with_a_rect(&root, "Landing v4");
        let second = doc_with_a_rect(&root, "Pricing table");
        let ctx = egui::Context::default();
        // **With a disk cache**, so this test also runs the write half of
        // `render` — which every test here did, into the user's own cache
        // directory, until §15 D845 gave them one of their own.
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
    /// retried, or an empty document would re-enter the render path on every
    /// pass for the life of the session — queueing work the documents which
    /// *can* be drawn are waiting behind.
    ///
    /// ⚠️ **The tell used to be the budget and is now the queue** (§15 D820).
    /// With the render inline, a second `get` that re-rendered spent more of
    /// `Covers::spent`; there is no such field any more, and the question is
    /// instead whether the key goes back on the renderer's queue.
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

    /// A worker that has already died, built by hand: both channels hung up, one
    /// job counted and never to be answered, and its key still in `queued` —
    /// **exactly the residue `[X1.1-L1-01]` describes**, reached without a
    /// thread having to die in the test.
    ///
    /// `report` is returned rather than dropped when `answers_hang_up` is false,
    /// so the *receiving* side can stay connected while the *sending* side has
    /// gone — the two halves of a death are met on different lines of `get`.
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

    /// **A worker that has died is forgotten, and the next pass spawns another**
    /// (§15 D845, `[X1.1-L1-01]`).
    ///
    /// 🚨 **Before this no cover appeared again for the rest of the session.**
    /// `drain` read the hung-up channel as an idle one, `render` stayed `Some`, and
    /// every later `get` put its key in `queued`, failed its send and left the key
    /// there — so the document was never asked for again either.
    ///
    /// ⚠️ **Flip-checks, run, against the two plausible wrong versions**, since
    /// deleting the repair outright proves little:
    /// - `drain`'s `Disconnected` arm answering `false` — the old `while let` —
    ///   fails at *"the dead worker's owed answer is forgotten with it"*: `get`'s
    ///   send-failure arm still forgets the worker, but it removes only the key
    ///   it was sending, so the dead job's key stays in `queued` for good.
    /// - The arm clearing `queued` and **not** `render` — the finding's own named
    ///   wrong fix — passes that and fails at *"a fresh worker draws it"*: the
    ///   send meets the dead receiver, and the pass that notices comes back
    ///   empty-handed.
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

    /// **A send that fails leaves no key behind** (§15 D845, `[X1.1-L1-01]`) —
    /// the death met on the sending side, where the answers channel has not hung
    /// up yet, so `drain` sees `Empty` and cannot help.
    ///
    /// ⚠️ **Flip-check, run**: deleting `get`'s `else` arm fails at
    /// *"the key is not left in the queue"*. The consequence that matters is the
    /// last assertion — a key stuck in `queued` makes `insert` answer `false` on
    /// every later pass — and it is reasoned from that, not observed, since the
    /// first failure stops the test.
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

    /// **A panic costs one cover and not the worker** (§15 D845, `[X1.1-L1-01]`).
    ///
    /// The injected fault is `POISONED`, `library::writer`'s shape. The poisoned
    /// document answers `Blank` — no picture and **no red mark**, since a panic is
    /// this build's fault and says nothing about the file — and the healthy one
    /// beside it still arrives.
    ///
    /// ⚠️ **Flip-check, run**: calling `render` without the `catch_unwind` fails
    /// at *"the healthy one still arrives"* — the worker dies on the first job,
    /// `settle` meets the hung-up channel, and the second is owed by nobody.
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

    /// **A card drawn every frame sends its document once** (§15 D845,
    /// `[X1.1-L6-04]`) — `queued`'s whole stated purpose, which nothing asserted.
    ///
    /// `queued.len()` cannot witness it: it is a set, so a version that inserted
    /// the key *and still sent on every call* gave the same length. `in_flight`
    /// counts sends. **Flip-check, run**: moving the send out of the
    /// `if self.queued.insert(..)` block reads `3`.
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
        // **Sends still owed plus answers already in**, so the sum does not
        // depend on the clock: a render quick enough to finish between two calls
        // moves one from the first term to the second and leaves the total alone.
        let sends = covers.render.as_ref().map_or(0, |r| r.in_flight);
        let arrived = covers.covers.len();
        assert_eq!(sends + arrived, 1, "one send, however many frames ask");
        covers.settle(&ctx);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The dashboard's red mark comes through the path the app runs**
    /// (§15 D845, `[R2-L6-02]`).
    ///
    /// Every test above that reads a `Cover` reads it through `settle`, which had
    /// its own copy of the answer→`Cover` conversion; `drain`'s — the app's — was
    /// run by no test, and `Covers::unreadable` had none at all. There is one
    /// conversion now, so this is a test of both.
    ///
    /// ⚠️ **Flip-check, run**: `arrive`'s `Unreadable` arm answering
    /// `Cover::Blank` fails at *"the refused document is marked"*. The blank
    /// document is the control — `Unreadable` for *everything* fails there.
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

    /// **`clear` abandons the backlog, and nothing it abandoned arrives later**
    /// (§15 D845, `[X1.1-L1-02]`, `[X1.1-L1-03]`).
    ///
    /// This is the mechanism under the migration: every answer owed at a
    /// base-folder change is about a path that is about to stop existing, and an
    /// answer that arrived afterwards was cached under a path-independent key.
    ///
    /// ⚠️ **The assertion is on the map after a `settle`, not on the set after
    /// the `clear`**, because the plausible wrong version — clearing `queued` and
    /// leaving `render` — empties the set perfectly and then lets the old worker's
    /// answers in. **Flip-check, run** against exactly that: fails at *"only the
    /// new library's cover arrives"* with `3` against `1`.
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

    /// **The sweep keeps what the list claims, deletes what it does not, and
    /// deletes nothing on an empty list** (§15 D845, `[X1.1-L1-04]`,
    /// `[X1.1-L6-02]`) — the first test `sweep` has had, since until the seam it
    /// could only have run against the developer's own cache.
    ///
    /// The empty half is the unlistable root: an unplugged drive scans as no
    /// documents, and this used to delete every cover on the machine, other
    /// libraries' included.
    ///
    /// ⚠️ **Flip-check, run**: removing the `live.is_empty()` guard fails at
    /// *"an empty list is not evidence"*, the second half, and nowhere else — the
    /// first half is the control that the guard is not simply "sweep nothing".
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
}
