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
//! - **In memory**, as egui textures, with a per-pass time budget so opening the
//!   dashboard on a large library does not stall the frame that first shows it.
//!   `thumbs::ImageThumbs` and `fonts::FontPreviews` are the same shape and §15
//!   D160 is where those numbers were argued.
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
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The longest edge of a cover, in pixels.
///
/// **Sized for the card it fills at 2× display scaling** — the grid's cards are
/// around 170pt wide and 116pt tall — rather than for the document. A cover is a
/// thumbnail; asking the renderer for more is paying for pixels the card cannot
/// show, on every document in the library.
const COVER_MAX_PX: f64 = 400.0;

/// How long one pass may spend rendering covers.
///
/// **Larger than `ImageThumbs`' 2 ms**, and the difference is the point: that
/// one decodes an image that is already in memory, while this one runs the whole
/// document → pixels path. Two milliseconds would render roughly nothing and the
/// grid would fill in over several seconds of scrolling. Eight is still under a
/// 120 Hz frame, and the progress guarantee below means a document too slow for
/// any budget still arrives.
const FRAME_BUDGET: Duration = Duration::from_millis(8);

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

/// Where a cover lives on disk.
fn cache_path(key: &str) -> Option<PathBuf> {
    Some(
        dirs::cache_dir()?
            .join("ondin")
            .join("covers")
            .join(format!("{key}.png")),
    )
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
pub struct Covers {
    covers: HashMap<String, Cover>,
    /// The pass [`Self::spent`] was accumulated in, so the budget resets once per
    /// pass. egui's own counter rather than a flag someone has to clear.
    pass: u64,
    spent: Duration,
    /// [`FRAME_BUDGET`], as a field so a test can take it away.
    budget: Duration,
}

impl Default for Covers {
    fn default() -> Self {
        Self {
            covers: HashMap::new(),
            pass: 0,
            spent: Duration::ZERO,
            budget: FRAME_BUDGET,
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
        let pass = ctx.cumulative_pass_nr();
        if pass != self.pass {
            self.pass = pass;
            self.spent = Duration::ZERO;
        }
        let key = key(entry)?;
        // ⚠️ **The lookup is split from the insert rather than using an entry
        // API**, because the miss path renders — and holding a mutable borrow of
        // the map across a hundred milliseconds of rasterizing is how a borrow
        // checker fight turns into a clone of the whole cache.
        if self.covers.contains_key(&key) {
            return match self.covers.get(&key) {
                Some(Cover::Ready(t)) => Some(t),
                _ => None,
            };
        }

        // **One render a pass whatever the budget says**, which is the progress
        // guarantee: a document big enough to spend the whole budget on its own
        // would otherwise never be drawn at all.
        if !self.spent.is_zero() && self.spent >= self.budget {
            ctx.request_repaint();
            return None;
        }

        let started = Instant::now();
        let cover = match render(entry, &key) {
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
        // Timed whether or not it worked: a document that fails after 80 ms of
        // parsing has spent the pass's budget exactly as one that succeeded.
        self.spent += started.elapsed();
        ctx.request_repaint();
        self.covers.insert(key.clone(), cover);
        match self.covers.get(&key) {
            Some(Cover::Ready(t)) => Some(t),
            _ => None,
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
    /// lazy and per-pass budgeted, so a document whose cover has not been
    /// attempted yet answers `false` — which is why the dashboard asks
    /// `entry.unread || this`, and why the mark can arrive a pass late on a large
    /// library. `get` requests a repaint, so late arrives.
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
    /// re-renders all of them at one document per pass. The claim was true of
    /// *this function* and false of the program, which is the worst shape a doc
    /// comment has: correct about its own three lines and wrong about what
    /// happens.
    ///
    /// ⚠️ **The cost is bounded and the sentence was not.** Covers are
    /// deliberately disposable (§15 D366), the render budget is 8 ms a pass with
    /// a one-render progress guarantee, so the damage is a slowly filling grid
    /// rather than a wrong picture — which is why this is a corrected sentence
    /// and not a rewritten cache.
    ///
    /// **Two ways to make the old claim true, neither taken here.** Namespace
    /// the directory by a hash of the library root, which would also stop two
    /// libraries holding the same synced document from sharing an entry; or
    /// sweep on an *age* bound rather than on membership, so a cover no current
    /// document claims survives a switch and a genuinely dead one still goes.
    /// Both change eviction semantics and the first changes the cache layout, so
    /// they are decisions rather than repairs.
    pub fn clear(&mut self) {
        self.covers.clear();
    }

    /// Delete cached covers on disk that no live document claims.
    ///
    /// ⚠️ **Sweeps by key, which is id *and* mtime, so this is also what removes
    /// the superseded covers of documents that are still here.** Without it the
    /// folder grows by one PNG per save, forever — the failure mode a cache keyed
    /// on a changing value always has, and the one nothing on screen would ever
    /// show.
    pub fn sweep(live: &[Entry]) {
        let Some(dir) = cache_path("x").and_then(|p| p.parent().map(PathBuf::from)) else {
            return;
        };
        let keep: std::collections::HashSet<String> = live.iter().filter_map(key).collect();
        let Ok(entries) = std::fs::read_dir(&dir) else {
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
fn render(entry: &Entry, key: &str) -> Result<Rgba, NoCover> {
    let path = cache_path(key);
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

/// ⚠️ **These tests write real covers into the user's cache directory**, because
/// [`cache_path`] resolves `dirs::cache_dir()` and there is no seam to point it
/// somewhere else. It is harmless and self-cleaning — the ids are random and
/// belong to no library, so the next [`Covers::sweep`] the app runs deletes them
/// — but it is worth knowing before adding a test that writes a hundred.
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
        let entry = doc_with_a_rect(&root, "Landing v4");
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

        let entry = scan::scan(&root)
            .into_iter()
            .next()
            .expect("a document this build cannot load is still listed");
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

    /// ⚠️ **One render a pass whatever the budget says, and no more.**
    ///
    /// The budget is taken away rather than waited out, so this is about the
    /// *rule* rather than about how fast this machine rasterizes: with no budget
    /// at all the first document still gets its cover — the progress guarantee,
    /// without which a library of documents each slower than the budget would
    /// draw no covers ever — and the second is deferred to the next pass.
    ///
    /// Flip-check, run: removing the `!self.spent.is_zero()` guard makes the
    /// first call return `None` too and fails on the first assertion. Removing
    /// the budget check entirely fails the second. The two arms are independent
    /// and the test pins both.
    #[test]
    fn one_cover_a_pass_arrives_however_spent_the_budget_is() {
        let root = temp("budget");
        let first = doc_with_a_rect(&root, "Landing v4");
        let second = doc_with_a_rect(&root, "Pricing table");
        let ctx = egui::Context::default();
        let mut covers = Covers {
            budget: Duration::ZERO,
            ..Default::default()
        };
        // A pass has to be in flight for `load_texture` and the pass counter to
        // mean anything.
        let _ = ctx.run_ui(Default::default(), |_| {});

        assert!(
            covers.get(&ctx, &first).is_some(),
            "the first render of a pass must happen whatever the budget says"
        );
        assert!(
            covers.get(&ctx, &second).is_none(),
            "the second must not, the budget being spent"
        );
        // And the next pass starts the budget again.
        let _ = ctx.run_ui(Default::default(), |_| {});
        assert!(
            covers.get(&ctx, &second).is_some(),
            "the budget is per pass, not per session"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A cover that could not be made is remembered as a failure rather than
    /// retried, or an empty document would re-enter the render path on every
    /// pass for the life of the session — spending the budget that the documents
    /// which *can* be drawn need.
    #[test]
    fn a_document_with_no_cover_is_not_retried_every_pass() {
        let root = temp("failed");
        let mut doc = Document::new(IdSource::new(1).mint());
        store::file_document(&root, None, "Untitled", &mut doc).unwrap();
        let entry = scan::scan(&root).into_iter().next().unwrap();
        let ctx = egui::Context::default();
        let mut covers = Covers::default();
        let _ = ctx.run_ui(Default::default(), |_| {});

        assert!(covers.get(&ctx, &entry).is_none());
        // The tell is the budget: a second call that re-rendered would spend
        // more of it, and one that read the remembered failure spends none.
        let spent_after_first = covers.spent;
        assert!(covers.get(&ctx, &entry).is_none());
        assert_eq!(
            covers.spent, spent_after_first,
            "a remembered failure must cost nothing to look up"
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
}
