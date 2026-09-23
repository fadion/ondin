//! The decode cache: encoded bytes in, `peniko::ImageData` out, once per image.
//!
//! **Why it lives in `ondin-render` rather than in the app.** Two crates need
//! decoded pixels — the app's canvas and `ondin-export`'s PNG — and both already
//! depend on this one. Core cannot hold it: core does no decoding, has no image
//! dependency, and is the thing that stays headless (§3).
//!
//! **What it stores is the shared form, not a backend's.** All three brushes in
//! play are the same `peniko::Brush` over different image slots — the model's
//! carries an [`ImageRef`], vello's carries `peniko::ImageData`, and vello_cpu's
//! carries a `vello_cpu::ImageSource` — so the cache holds the middle one and
//! each backend converts its own slot at the boundary. That conversion is free
//! for the GPU (an `Arc` clone of the blob) and is not for the CPU, which
//! premultiplies into a `Pixmap`; `cpu.rs` memoizes it per render for that
//! reason.
//!
//! **The budget bounds bytes, not a count** — the font preview cache's early
//! `PREVIEW_CAP` is the cautionary tale the image plan named (§5.5a, §15 D160),
//! and it is the right one: a count of images bounds nothing when one of them is
//! 12 MP.
//!
//! **It holds a second kind of thing now: pixels *derived* from a decode.** An
//! image fill carries an [`ImageAdjust`], which is a per-pixel function rather
//! than a brush transform, so unlike the framing it cannot be answered by drawing
//! the same buffer differently — it needs its own. Those are keyed by `(id,
//! adjustment)`, live under their own budget, and are filled where they are
//! *read* rather than where the table is prepared: an adjustment belongs to a
//! fill, and only the scene walk knows the fills — including the previewed one a
//! walk of the document would never see. Hence the lock, and hence
//! [`ImageStore::pixels`] rather than [`ImageStore::get`] being what a backend
//! paints with.
//!
//! **What it does not do is bound a document.** Eviction only ever drops an image
//! the document no longer carries — one left behind by an undo or a purge — so a
//! file whose own images exceed the budget keeps all of them decoded. That is
//! deliberate: the alternative is evicting something a fill still refers to, and
//! then either the picture disappears mid-frame or the next frame decodes it
//! again and evicts it again. That thrash is exactly what `PREVIEW_FLOOR` exists
//! to prevent in the font cache (§15 D160), and here the honest floor is "every
//! image the document is holding". A hard bound on a huge document needs
//! decode-on-demand with a resident set, which is a different design and not one
//! this feature needs yet.
//!
//! [`ImageRef`]: ondin_core::ImageRef
//! [`ImageAdjust`]: ondin_core::ImageAdjust

use ondin_core::{
    AdjustPipeline, Adjustment, Document, ImageAdjust, ImageFormat, ImageId, ImageRef,
    ImageSource as CoreSource, kurbo::Affine,
};
use peniko::{Blob, ImageAlphaType, ImageData, ImageFormat as PenikoFormat};
use rustc_hash::FxHashMap;
use std::sync::{Arc, Mutex};

/// Decoded bytes held before anything is dropped, when the document stops
/// referring to them.
///
/// 256 MB is roughly twenty 12-megapixel photographs at four bytes a pixel. The
/// number bounds *stale* decodes — see the module docs — so it is a ceiling on
/// waste rather than on the working set, and it is generous on purpose: the cost
/// of it being too small is re-decoding an image the user just undid the deletion
/// of.
pub const DECODED_BYTES: usize = 256 * 1024 * 1024;

/// The largest edge either backend will accept.
///
/// **A guard against a panic, not a policy.** `vello_cpu` builds its `Pixmap`
/// from `u16` dimensions and `ImageSource::from_peniko_image_data` asserts on
/// exactly this bound, so a 70000-pixel-wide image would take the renderer down
/// rather than draw badly. Refused at the decode instead, where it becomes a
/// picture that does not appear rather than a crash.
pub const MAX_EDGE: u32 = u16::MAX as u32;

/// vello's image atlas at its largest, in pixels a side.
///
/// **Upstream's number, pinned to a version.** `vello_encoding-0.9.0`'s
/// `image_cache.rs` has `const MAX_ATLAS_SIZE: i32 = 8192;`, with
/// `DEFAULT_ATLAS_SIZE` 1024 and `bump_size` doubling until it reaches this. It is
/// **not public**, so this cannot be derived and a vello upgrade can silently
/// invalidate it — the same standing hazard [`crate::cpu::MAX_RASTER_SIDE`]
/// carries, and what stands in for a gate is the same thing: a test that renders
/// at the boundary.
///
/// ⚠️ **What happens when a picture does not fit is nothing at all.**
/// `resolve.rs:505 · resolve_pending_images` says so in a comment — *"If the atlas
/// is already maximum size, there's nothing we can do. Set the xy field to None so
/// this image isn't rendered and then carry on"* — not an error, not a `Result`,
/// not a log. So the canvas cannot be told it lost a picture; it has to arrange in
/// advance not to (§15 D745, `[S9.2-L1-01]`).
pub const ATLAS_EDGE: u32 = 8192;

/// The most a picture is shrunk before the canvas gives up on fitting it.
///
/// Reached only by a page carrying more photographs than an atlas can hold at any
/// resolution; past it [`atlas_downscale`] stops looking and the drop it was
/// avoiding happens after all. At 64 a 4500-pixel photograph is 71 across, so a
/// page that still does not fit is one with thousands of pictures on it.
pub const MAX_ATLAS_DOWNSCALE: u32 = 64;

/// Whether every one of `sizes` fits an atlas `side` pixels square.
///
/// 🚨 **A *shelf* packing, where vello uses a guillotine allocator, and the
/// direction of that difference is the whole reason this is allowed to exist.**
/// Shelf packing is strictly the worse of the two — it wastes the tail of every
/// row — so **"the shelf fits" implies "the guillotine fits"**, which is the
/// one-way guarantee this needs. The converse does not hold, so this refuses some
/// sets vello would have taken, and the cost of that is a photograph shrunk when
/// it need not have been. The alternative is depending on `guillotiere` directly
/// and hoping our copy matches vello's, which trades a conservative answer for a
/// version-skew bug that draws nothing.
///
/// Tallest first, which is what makes a shelf packing worth simulating at all: in
/// encounter order a short picture can open a shelf that a tall one then has to
/// leave.
///
/// ⚠️ **Its cases are the finding's own measurements** and it reproduces every
/// one of them — two 4500 × 3000 photographs fit and three do not, an 8192-wide
/// picture fits and 8193 does not. That is what says the model is the right model
/// rather than a plausible one.
fn fits_by_shelf(sizes: &[(u32, u32)], side: u32) -> bool {
    let mut v: Vec<(u32, u32)> = sizes.to_vec();
    v.sort_by_key(|s| std::cmp::Reverse(s.1));
    let (mut x, mut y, mut shelf) = (0u32, 0u32, 0u32);
    for (w, h) in v {
        if w > side || h > side {
            return false;
        }
        if x + w > side {
            y += shelf;
            x = 0;
            shelf = 0;
        }
        if y + h > side {
            return false;
        }
        x += w;
        shelf = shelf.max(h);
    }
    true
}

/// The one integer factor every picture is shrunk by before the GPU sees it, so
/// that the page's pictures fit the atlas.
///
/// **Full resolution until it stops fitting, then shrink** — the maintainer's
/// decision, and the reason this searches upward from 1 rather than computing a
/// factor from the total area. A page with two photographs on it is handed both at
/// their own size, exactly as before; the third is what changes anything.
///
/// **One factor for all of them rather than one each.** A per-picture factor
/// packs better and makes the page's pictures disagree with each other about
/// sharpness, which reads as one photograph being wrong rather than as a budget;
/// and it makes the answer depend on the order they were placed in. One number is
/// also one cache key.
///
/// **Integer, because [`reduced_by`] is a box average over whole blocks** — and
/// because a factor that moved continuously would rebuild every picture on the
/// page each time one was added.
pub fn atlas_downscale(sizes: &[(u32, u32)], side: u32) -> u32 {
    (1..=MAX_ATLAS_DOWNSCALE)
        .find(|f| {
            let scaled: Vec<(u32, u32)> = sizes
                .iter()
                .map(|(w, h)| (w.div_ceil(*f).max(1), h.div_ceil(*f).max(1)))
                .collect();
            fits_by_shelf(&scaled, side)
        })
        .unwrap_or(MAX_ATLAS_DOWNSCALE)
}

/// The most RGBA a single picture may expand to (§15 D449).
///
/// **Three caps were in play here and not one of them was denominated in decoded
/// RGBA bytes of a live image**, which is the whole finding. `image`'s
/// `Limits::default().max_alloc` is 512 MiB and bounds *the decoder's own buffer
/// in the decoder's own format* — grayscale is one byte a pixel, so it permits
/// 536 M pixels, and `into_rgba8` turns that into **2 GiB** with nothing left to
/// consult. [`MAX_EDGE`] is denominated in **edge pixels**, so `60000×200` sails
/// through it and holds 48 MB. [`DECODED_BYTES`] is in the right unit and bounds
/// only **stale** entries, so a picture the document still refers to is never
/// weighed against it at all.
///
/// Measured, release, uniform grayscale PNG through `ImageStore::insert`: a
/// **76 KB** file at 8000×8000 held **256 MB** after 433 ms — a **3,356×**
/// amplification — and `20000×20000` would reach 1.6 GB. `into_rgba8` allocates
/// through `Vec`, so failing it is `handle_alloc_error`, an **abort** that no `?`
/// or `.ok()?` on the path can see.
///
/// ⚠️ **Set equal to [`DECODED_BYTES`], and the reasoning is a trade rather than
/// a derivation.** A picture that alone exceeds what the whole store budgets for
/// held decodes is one the store should refuse; but the number cannot go much
/// lower, because a **50 MP camera file is legitimate** — a Sony A7R V is
/// 9504×6336, which is 240 MB of RGBA, and refusing it would be a worse bug than
/// the one this fixes.
///
/// ⚠️ **So this does not stop the freeze, and saying so is the point.** The
/// pathological 8000×8000 and a real 64 MP photograph are *the same size*; no cap
/// that accepts the second can refuse the first. What this closes is the
/// unbounded tail — the 1.6 GB and 2 GB cases, and the abort. The 433 ms
/// synchronous decode inside the dashboard's egui pass
/// (`library::cover::rasterize` → `export::png::png` → `prepare`) is a
/// **threading** problem and is recorded as one; it is not fixed here.
pub const MAX_DECODED_BYTES: u64 = DECODED_BYTES as u64;

/// Why an image could not be decoded, for the caller that wants to say so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The bytes are not a readable image of the format the entry claims.
    Corrupt,
    /// Wider or taller than [`MAX_EDGE`], which the CPU backend cannot hold.
    TooLarge { width: u32, height: u32 },
    /// Small enough on both edges and still too much RGBA — see
    /// [`MAX_DECODED_BYTES`]. Separate from [`Self::TooLarge`] because it is a
    /// different question with a different answer: that one is about a bound the
    /// renderer would assert on, this one about how much memory the picture
    /// becomes, and a caller explaining itself to a user wants to say which.
    TooMuchPixelData { width: u32, height: u32, bytes: u64 },
}

/// Adjusted pixels held before the least recently asked-for are dropped.
///
/// A separate ceiling from [`DECODED_BYTES`] because it bounds a different kind
/// of thing: a decode is the only copy of a picture the document carries, where
/// every one of these is **recomputable from one**. So this one really is a
/// working-set bound rather than a waste bound, and evicting under it costs a
/// recomputation rather than a re-decode.
pub const ADJUSTED_BYTES: usize = 256 * 1024 * 1024;

/// The shortest a scrub preview's long edge is allowed to get — a **floor**, not
/// a ceiling (see [`reduced`], where getting that backwards was a reported bug).
///
/// The reduction is by a whole factor, so a preview's long edge always lands
/// between this and twice it, and **a picture under twice this is not reduced at
/// all**. With the pass measured at roughly 5 ms per megapixel (`AdjustLut`,
/// 2026-08-15) that puts the numbers here:
///
/// - up to 2047 across — every photograph anybody places in a layout — no
///   preview exists, so there is nothing to see change, and the full pass is
///   2–13 ms;
/// - a 10-megapixel photograph reduces by 3, to about 1334 across: 6 ms a frame
///   instead of 51.
///
/// **1024 is chosen from the sharpness end, not the speed end**, because the
/// speed end stopped being the binding constraint once the pass was compiled to
/// tables. It is more than the canvas shows of a picture in an
/// inspector-sized window at 100%, so what softness remains is confined to
/// zoomed-in work on genuinely large photographs.
pub const PREVIEW_EDGE: u32 = 1024;

/// The long edge a picture is reduced to **once**, before any thumbnail is cut
/// out of it.
///
/// **A thumbnail is not a small preview, and the difference is which cost it
/// pays.** [`PREVIEW_EDGE`] answers "how much resolution may a moving slider
/// throw away". This answers "how big must the source be so that cutting a
/// twenty-pixel square out of it is free" — because [`reduced`] is
/// O(width × height) whatever its target, reading every source pixel to produce
/// one output block. Done per thumbnail that would put a twelve-megapixel box
/// filter in the layers panel on every frame of a scrub, since a scrub walks a
/// new [`ThumbKey`] each frame. Done once per [`ImageId`] and shared by every
/// thumbnail of it, the per-frame work is a 128×128 read instead.
///
/// **128 rather than something nearer the twenty points a row draws**, because
/// two multipliers sit between the two numbers. The key carries the *device*
/// edge, so a 20-point thumbnail at 300% display scaling asks for 60 pixels; and
/// centre-cropping keeps only the picture's short edge, which for 16:9 is 72 of
/// these 128 at the worst factor [`reduced`] will choose. The headroom is what
/// keeps a thumbnail sharp at a scale nobody has measured on.
///
/// 64 KB a picture, which is why it rides on the decode cache's eviction rather
/// than carrying a budget of its own: a ceiling over entries this size would be
/// machinery bounding nothing.
pub const THUMB_BASE: u32 = 128;

/// How many consecutive frames an adjustment has to stand still before its
/// full-resolution buffer is built.
///
/// **Two, and it has to be two rather than one.** Promoting what was asked for on
/// the previous frame alone would, during a scrub, spend a full-resolution pass
/// on *every* value the slider passed through — one frame late, and every one of
/// them thrown away. Asking for the same adjustment twice running is what a
/// released slider looks like and what a moving one never does.
const SETTLED_FRAMES: u64 = 2;

struct Entry {
    data: ImageData,
    /// Decoded size in bytes — what the budget counts.
    bytes: usize,
    /// `prepare`'s clock when this was last seen in the document.
    touched: u64,
}

/// What an image reference resolves to: pixels, and where they sit relative to
/// the picture's own grid.
///
/// **The second field is what makes a reduced preview safe.**
/// [`ondin_core::ImageRef::framing`] maps *source-pixel space* onto the frame, so
/// handing a backend a buffer of different dimensions under that transform would
/// draw the picture at the wrong size — a bug that appears only while a slider is
/// moving, which is the worst possible place for it. Composing this under the
/// framing is the whole correction, and it is the identity for every
/// full-resolution answer.
#[derive(Clone)]
pub struct Pixels {
    pub data: ImageData,
    /// This buffer's pixel grid → the source's.
    pub to_source: Affine,
}

/// One picture with one set of adjustments applied to it.
struct Derived {
    data: ImageData,
    /// Kept so a promotion can redo the work at full resolution without having
    /// to rebuild an [`ImageAdjust`] out of the key's bit patterns.
    pipeline: AdjustPipeline,
    to_source: Affine,
    /// Whether `data` is the picture at its own size rather than a preview.
    full: bool,
    bytes: usize,
    /// The clock of the most recent frame that asked for this, and of the frame
    /// that asked before that one. Two, because "has this stood still?" is a
    /// question about two frames (see [`SETTLED_FRAMES`]).
    ///
    /// **`None` rather than zero for "there was no frame before".** A sentinel of
    /// 0 is indistinguishable from clock 0, which is one arithmetic step away from
    /// the promotion test on the *second* frame of a session — where it promotes
    /// every value a drag passes through, one per frame, which is the whole defect
    /// the two-frame rule exists to avoid.
    asked: u64,
    asked_before: Option<u64>,
}

impl Derived {
    fn note_ask(&mut self, clock: u64) {
        if self.asked != clock {
            self.asked_before = Some(self.asked);
            self.asked = clock;
        }
    }

    fn pixels(&self) -> Pixels {
        Pixels {
            data: self.data.clone(),
            to_source: self.to_source,
        }
    }
}

/// The seven adjustment values as bits — an [`ImageAdjust`] is `f32`s and so is
/// neither `Eq` nor `Hash`, and a cache key has to be both.
///
/// Safe to compare bitwise here because the *neutral* value never reaches this:
/// [`ImageAdjust::pipeline`] answers `None` for it, so `-0.0` and `0.0` — the one
/// pair that is equal with different bits — are both resolved before a key is
/// built.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
struct AdjustKey([u32; 7]);

impl AdjustKey {
    fn of(a: &ImageAdjust) -> Self {
        Self(Adjustment::ALL.map(|k| k.of(a).to_bits()))
    }
}

/// What a thumbnail's pixels depend on, so the chrome can cache a texture
/// without knowing how any of it is spelled.
///
/// **Every field is one the drawing changes with**, which is why the [`ImageId`]
/// alone will not do: two layers can share one picture and show different halves
/// of it, and a panel that cached by id would draw one of them on both rows. The
/// framing *mode* is deliberately absent — Fill, Fit, Crop and Tile are
/// statements about the shape's box, and a thumbnail has no box but its own
/// square.
///
/// Opaque on purpose. The adjustment is seven `f32`s and the crop four `f64`s,
/// and both carry the one pair of bit patterns that is equal-but-different
/// (`-0.0`); normalizing that is [`Self::of`]'s business rather than a caller's.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ThumbKey {
    id: ImageId,
    adjust: AdjustKey,
    /// `ImageOrient` is not `Hash`, and these are the two fields of it that are
    /// read — through `quarters()`, so a hand-written `4` keys as the `0` it
    /// draws as.
    orient: (u8, bool),
    crop: [u64; 4],
    /// **Device** pixels, not points. Display scale therefore changes the key
    /// rather than needing a field of its own to invalidate against, and the
    /// stale sweep the caller already owes collects the old size.
    edge: u32,
}

impl ThumbKey {
    /// The key `r` draws under at `edge` device pixels.
    pub fn of(r: &ImageRef, edge: u32) -> Self {
        Self {
            id: r.id.clone(),
            // **The neutral adjustment collapses to one key rather than to its
            // own bits.** [`ImageAdjust::pipeline`] answers `None` for it, so
            // nothing downstream reads the seven values at all — but `-0.0` and
            // `0.0` have different bits, so keying on them would give one
            // drawing two textures and neither would ever be a hit. This is the
            // same corner [`AdjustKey`] is documented as being safe from
            // *because* the neutral value never reaches it; here it does.
            adjust: match r.adjust.pipeline() {
                Some(_) => AdjustKey::of(&r.adjust),
                None => AdjustKey([0; 7]),
            },
            orient: (r.orient.quarters(), r.orient.mirrored),
            crop: [r.crop.x0, r.crop.y0, r.crop.x1, r.crop.y1].map(f64::to_bits),
            edge,
        }
    }
}

#[derive(Default)]
struct Adjusted {
    entries: FxHashMap<(ImageId, AdjustKey), Derived>,
    /// One reduced copy per picture, shared by every adjustment of it — seven
    /// sliders on one photograph downsample it once.
    small: FxHashMap<ImageId, (ImageData, Affine)>,
    /// One picture reduced to [`THUMB_BASE`], shared by every thumbnail of it.
    ///
    /// Keyed by picture alone, where everything else in here is keyed by picture
    /// *and* value — which is the whole point of it. A scrub walks a new
    /// [`ThumbKey`] every frame and none of those frames recomputes this.
    thumb_base: FxHashMap<ImageId, ImageData>,
    /// One picture reduced to the atlas budget's factor (§15 D745).
    ///
    /// 🚨 **Keyed by the *blob's* id and not the picture's**, which is what makes
    /// it correct for an adjusted picture as well as a bare one: the buffer
    /// [`ImageStore::pixels`] hands back may be the decode, a preview of it, or a
    /// derived copy carrying somebody's exposure slider, and those are three
    /// different sets of pixels under one [`ImageId`]. It is also the key **vello
    /// itself** uses for atlas residency (`image_cache.rs`'s
    /// `self.map.entry(image.data.id())`), so a hit here and a hit there mean the
    /// same thing.
    ///
    /// The factor is in the key too, and [`ImageStore::prepare`] drops every entry
    /// that is not at the current one — so the map holds one generation, and a
    /// page whose photograph count keeps changing does not accumulate.
    atlas: FxHashMap<(u64, u32), (ImageData, Affine)>,
}

/// Decoded images, keyed by the id a fill refers to.
#[derive(Default)]
pub struct ImageStore {
    entries: FxHashMap<ImageId, Entry>,
    /// Ids that failed to decode, so a corrupt image is not re-decoded every
    /// prepare. Cleared with the entry it belongs to.
    failed: FxHashMap<ImageId, DecodeError>,
    /// Pixels derived from those by an [`ImageAdjust`].
    ///
    /// **Behind a lock because it is filled where it is read, not where it is
    /// prepared.** An adjustment lives on a *fill*, and a fill is only known to
    /// the scene walk — which holds the store immutably, and which sees a
    /// previewed adjustment a walk of the document's table never would. The
    /// alternative was a second walk of every node's paints, run before the real
    /// one, kept in step with `RenderOverrides` by hand; this is the same work
    /// done once, by the walk that already has the brush in its hand.
    derived: Mutex<Adjusted>,
    budget: usize,
    derived_budget: usize,
    /// Whether a miss may be answered with a reduced picture while the value is
    /// still moving.
    ///
    /// **Off by default, which is the safe direction.** An export that quietly
    /// wrote a downsampled photograph would be a defect nothing on screen could
    /// show; a canvas that is briefly soft under a moving slider is the intended
    /// behaviour. So the interactive caller opts in ([`Self::interactive`]) and
    /// every other caller — `ondin export --png` included — gets the picture.
    previews: bool,
    clock: u64,
    /// The integer factor every picture is shrunk by before the GPU sees it, so
    /// the page's pictures fit vello's atlas (§15 D745, `[S9.2-L1-01]`).
    ///
    /// Recomputed by [`Self::prepare`] from the document's own table, **not from
    /// what a frame happens to draw**, and that is deliberate: a factor derived
    /// from visibility would change as the canvas is panned, so a photograph would
    /// visibly resharpen when its neighbour scrolled off. The table is what the
    /// store already decodes and holds, so it is also free to read.
    ///
    /// `0` from `Default`, which every reader treats as `1` — see
    /// [`Self::atlas_factor`]. Only the GPU backend consults it; the CPU one has
    /// no atlas and `ondin export --png` must write the picture.
    atlas_factor: u32,
}

impl ImageStore {
    /// A store that always answers at full resolution. What every non-interactive
    /// caller wants, and what `ondin export --png` must have.
    pub fn new() -> Self {
        Self::with_budget(DECODED_BYTES)
    }

    /// [`Self::new`], plus permission to answer a *fresh* adjustment with a
    /// reduced picture until it has stood still for [`SETTLED_FRAMES`] frames.
    ///
    /// For the canvas alone. See [`Self::wants_frame`], which the caller owes a
    /// repaint for — without it the last frame of a scrub is the one left on
    /// screen, and it is the soft one.
    pub fn interactive() -> Self {
        Self {
            previews: true,
            ..Self::new()
        }
    }

    /// Whether a derived buffer for `r` is held **without building one** (§15
    /// D596).
    ///
    /// 🚨 **The one observation the store did not offer, and its absence made a
    /// test vacuous.** `derived_len` is a count, and [`trim_derived`] runs on
    /// every insertion — so after an eviction the count is back at the budget
    /// whichever buffer went, and asking for the survivor through
    /// [`Self::pixels`] rebuilds it and evicts another, returning the same
    /// number. `a_stale_derived_buffer_goes_before_a_live_one` was written that
    /// way and passed under a *reversed* preference; `arch-scribe` caught it
    /// reading the test against what it can see. **A test about *which* entry
    /// survived needs an observation that does not disturb the thing it is
    /// observing.**
    #[cfg(test)]
    fn holds_derived(&self, r: &ImageRef) -> bool {
        let key = (r.id.clone(), AdjustKey::of(&r.adjust));
        let cache = self.derived.lock().unwrap_or_else(|e| e.into_inner());
        cache.entries.contains_key(&key)
    }

    /// Shrink the derived-buffer budget, for a test that would otherwise have to
    /// build 256 MB of pixels to reach it (§15 D596).
    ///
    /// [`Self::with_budget`]'s twin, and test-only for the reason that one is
    /// not: nothing in the app has any business choosing this number —
    /// [`ADJUSTED_BYTES`] is the one answer and its doc is where the argument for
    /// it lives.
    #[cfg(test)]
    fn set_derived_budget(&mut self, bytes: usize) {
        self.derived_budget = bytes;
    }

    pub fn with_budget(budget: usize) -> Self {
        Self {
            entries: FxHashMap::default(),
            failed: FxHashMap::default(),
            derived: Mutex::default(),
            budget,
            derived_budget: ADJUSTED_BYTES,
            previews: false,
            clock: 0,
            // "No reduction" until a `prepare` has seen what the document holds.
            atlas_factor: 1,
        }
    }

    /// Decode everything `doc` carries that is not decoded yet, and drop what it
    /// no longer carries once that exceeds the budget.
    ///
    /// **Cheap enough to call every frame, and the canvas does.** Once the
    /// decoding is done this is one hash lookup per table entry, against a scene
    /// walk that touches every visible node — so driving it off a change signal
    /// would save nothing measurable and would eventually miss one. Undo is the
    /// case that would find that.
    ///
    /// **Linked images are skipped**, because reading a file is not this crate's
    /// job: whoever has the bytes calls [`Self::insert`] with them. A linked entry
    /// therefore behaves exactly as a missing one until it is loaded, which is the
    /// same state as a link whose file has moved — one code path, and one drawing:
    /// all three states get the placeholder `ondin_core::missing_placeholder`
    /// describes (§15 D179).
    pub fn prepare(&mut self, doc: &Document) {
        self.clock += 1;
        for (id, entry) in doc.images() {
            let CoreSource::Embedded(bytes) = &entry.source else {
                continue;
            };
            self.ensure(id, bytes, entry.format);
        }
        // Anything the document has stopped carrying: `touched` is the clock of
        // the last prepare that saw it, so it is stale exactly when it is behind.
        let now = self.clock;
        self.evict_stale(now);
        // **After the eviction, so the population is the live one** (§15 D745).
        // A photograph the document has stopped carrying must not go on shrinking
        // the ones it still does.
        let sizes: Vec<(u32, u32)> = self
            .entries
            .values()
            .map(|e| (e.data.width, e.data.height))
            .collect();
        let was = self.atlas_factor;
        self.atlas_factor = atlas_downscale(&sizes, ATLAS_EDGE);
        if self.atlas_factor != was {
            let f = self.atlas_factor;
            let mut cache = self.derived.lock().unwrap_or_else(|e| e.into_inner());
            cache.atlas.retain(|(_, at), _| *at == f);
        }
        self.promote_settled();
        self.evict_derived();
    }

    /// The factor [`Self::atlas_pixels`] shrinks by — `1` when everything fits,
    /// which is every page this changes nothing for.
    ///
    /// `max(1)` because `Default` leaves the field at zero and a store that has
    /// never been prepared must still answer "no reduction" rather than divide by
    /// nothing.
    pub fn atlas_factor(&self) -> u32 {
        self.atlas_factor.max(1)
    }

    /// [`Self::pixels`], shrunk to the page's share of vello's image atlas.
    ///
    /// 🚨 **The GPU backend's accessor, and the one place the two backends are
    /// *meant* to disagree** (§15 D745, `[S9.2-L1-01]`). vello keeps every picture
    /// a scene draws in one 8192-pixel sheet and, when the next one will not fit,
    /// **draws nothing and says nothing** — measured: three ordinary 4500 × 3000
    /// photographs on one page drew 1 250 opaque pixels of 1 875, with the layers
    /// row and the Fill swatch still showing all three, because those read the
    /// *store* and only the canvas reads the atlas. `ondin export --png` and
    /// `--svg` both wrote the picture correctly the whole time, and must go on
    /// doing so — so the CPU backend keeps calling [`Self::pixels`] and this is
    /// the canvas's own answer.
    ///
    /// **Applied to whatever buffer comes back, not to the decode.** A picture
    /// under a moving slider is already a preview, and reducing a preview again is
    /// one step softer than it needs to be — for the duration of a scrub, on a
    /// page that is over the atlas budget anyway. The alternative is a second
    /// factor threaded through the derived cache to say how much of the budget
    /// each buffer has already spent, which is a great deal of machinery for a
    /// case that resolves itself the moment the slider stops.
    ///
    /// Returns the unreduced answer when [`reduced_by`] declines — a non-RGBA8
    /// format, or a buffer already one pixel across. **Better a picture that may
    /// not fit than no picture**, which is the same direction
    /// `push_effect_layer`'s fallback chooses.
    pub fn atlas_pixels(&self, r: &ondin_core::ImageRef) -> Option<Pixels> {
        let px = self.pixels(r)?;
        let f = self.atlas_factor();
        if f < 2 {
            return Some(px);
        }
        let key = (px.data.data.id(), f);
        let mut cache = self.derived.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((data, to_source)) = cache.atlas.get(&key) {
            return Some(Pixels {
                data: data.clone(),
                // **Composed, not replaced.** `px.to_source` maps the buffer we
                // were handed onto the source grid and this one maps the reduced
                // buffer onto that, so a preview under an adjustment carries both.
                to_source: px.to_source * *to_source,
            });
        }
        let Some((data, to_source)) = reduced_by(&px.data, f) else {
            return Some(px);
        };
        cache.atlas.insert(key, (data.clone(), to_source));
        Some(Pixels {
            data,
            to_source: px.to_source * to_source,
        })
    }

    /// Build the full-resolution answer for an adjustment that has stood still,
    /// and drop the preview it replaces.
    ///
    /// **Runs before the frame that will draw it, off the two previous frames'
    /// asks** — see [`SETTLED_FRAMES`] for why one frame is not enough. The
    /// pipeline is taken from the entry rather than from the document, because
    /// the value being previewed may be an override the document has never seen.
    ///
    /// **One per frame, however many are due.** A full-resolution pass over a
    /// 12-megapixel photograph is a visible fraction of a second, and this runs at
    /// the top of a frame — so a document where five pictures settle together
    /// would spend that five times over in one frame and drop it. Spread out they
    /// are five ordinary frames, and [`Self::wants_frame`] keeps asking until the
    /// last one is done. **Sorted**, so which one goes first is the same every run
    /// rather than a hash order.
    fn promote_settled(&mut self) {
        let clock = self.clock;
        if clock < SETTLED_FRAMES {
            return;
        }
        let due: Option<((ImageId, AdjustKey), AdjustPipeline)> = {
            let cache = self.derived.get_mut().unwrap_or_else(|e| e.into_inner());
            cache
                .entries
                .iter()
                .filter(|(_, d)| {
                    // Asked on the previous frame, and on the one before that:
                    // two consecutive frames wanting the same numbers is what a
                    // released slider looks like and what a moving one never does.
                    !d.full
                        && d.asked + 1 == clock
                        && d.asked_before == Some(clock - SETTLED_FRAMES)
                })
                .min_by(|a, b| a.0.cmp(b.0))
                .map(|(k, d)| (k.clone(), d.pipeline))
        };
        if let Some((key, pipeline)) = due
            && let Some(base) = self.entries.get(&key.0)
        {
            let data = adjusted_data(&base.data, &pipeline);
            let cache = self.derived.get_mut().unwrap_or_else(|e| e.into_inner());
            if let Some(d) = cache.entries.get_mut(&key) {
                d.bytes = data.data.data().len();
                d.data = data;
                d.to_source = Affine::IDENTITY;
                d.full = true;
            }
        }
    }

    /// Drop derived buffers nothing is looking at.
    ///
    /// Two rules, because there are two kinds of waste here. A **preview** whose
    /// value has been scrolled past is worthless the moment it stops being asked
    /// for — it is one position of a slider nobody will return to — so it goes as
    /// soon as it is stale. A **full** buffer is a real answer that cost real
    /// work, so it goes only under the budget, least recently asked first, and
    /// stale before live: evicting one the walk is about to ask for again is the
    /// thrash `PREVIEW_FLOOR` exists to prevent in the font cache (§15 D160).
    ///
    /// ⚠️ **This is not where the budget is enforced any more** (§15 D596,
    /// `[S9.2-L4-03]`) — [`trim_derived`] is, and [`Self::pixels`] calls it too.
    /// Being *only* here meant the cap did nothing at all on every path that
    /// calls `prepare` once and then reads; see `pixels`.
    fn evict_derived(&mut self) {
        let clock = self.clock;
        let cache = self.derived.get_mut().unwrap_or_else(|e| e.into_inner());
        let live = |asked: u64| asked + SETTLED_FRAMES >= clock;
        let sources = &self.entries;
        cache
            .entries
            .retain(|(id, _), d| sources.contains_key(id) && (d.full || live(d.asked)));
        trim_derived(cache, self.derived_budget, clock, None);
        // A reduced copy exists only to feed previews of one picture; when none
        // of them is left it is a megabyte nobody can reach.
        cache
            .small
            .retain(|id, _| cache.entries.keys().any(|(k, _)| k == id));
        // **The thumbnail base goes with the picture, not with the last
        // adjustment of it**, which is one step further out than the rule above
        // and follows from its being keyed by id alone. A layer whose thumbnail
        // is on screen has no derived entry at all — `thumbnail` deliberately
        // makes none — so the `small` rule applied here would throw the base
        // away every frame and rebuild it the next.
        cache.thumb_base.retain(|id, _| sources.contains_key(id));
    }

    /// Whether a repaint is owed: some adjustment is still showing a preview and
    /// will be promoted once it has stood still.
    ///
    /// **The caller owes this a `request_repaint`**, and the reason is exact: the
    /// promotion happens in a *later* frame's `prepare`, and the frame that
    /// releases a slider is often the last one egui draws. Without it the picture
    /// is left soft until the user happens to move the pointer again — which
    /// reads as "the adjustment blurred my photograph" rather than as a preview.
    pub fn wants_frame(&self) -> bool {
        let cache = self.derived.lock().unwrap_or_else(|e| e.into_inner());
        let clock = self.clock;
        cache
            .entries
            .values()
            .any(|d| !d.full && d.asked + 1 >= clock)
    }

    /// The pixels `r` draws from, adjustments and all.
    ///
    /// **This, not [`Self::get`], is what a backend paints with.** `get` answers
    /// the decode — the picture as the file has it — and is still the right
    /// question for "can this be drawn at all" (`ScenePainter::has_image`), since
    /// an adjustment can fail at nothing. What a *fill* shows is this.
    ///
    /// A neutral adjustment resolves to the decode itself, at no cost and with no
    /// cache entry: an `Arc` clone of the blob, exactly as before adjustments
    /// existed.
    pub fn pixels(&self, r: &ImageRef) -> Option<Pixels> {
        let base = self.entries.get(&r.id)?;
        let Some(pipeline) = r.adjust.pipeline() else {
            return Some(Pixels {
                data: base.data.clone(),
                to_source: Affine::IDENTITY,
            });
        };
        let key = (r.id.clone(), AdjustKey::of(&r.adjust));
        let mut cache = self.derived.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(d) = cache.entries.get_mut(&key) {
            d.note_ask(self.clock);
            return Some(d.pixels());
        }
        // A miss. The reduced copy is shared between every adjustment of this
        // picture, so seven sliders on one photograph downsample it once.
        let (source, to_source) = match self.previews {
            true => match cache.small.get(&r.id) {
                Some(small) => small.clone(),
                None => {
                    let small = reduced(&base.data, PREVIEW_EDGE)
                        .unwrap_or_else(|| (base.data.clone(), Affine::IDENTITY));
                    cache.small.insert(r.id.clone(), small.clone());
                    small
                }
            },
            false => (base.data.clone(), Affine::IDENTITY),
        };
        let data = adjusted_data(&source, &pipeline);
        let derived = Derived {
            bytes: data.data.data().len(),
            // The geometric test, not `!previews`: a picture already smaller than
            // `PREVIEW_EDGE` is its own preview, so an interactive store's answer
            // for it is the full one and there is nothing to promote.
            full: to_source == Affine::IDENTITY,
            data,
            pipeline,
            to_source,
            asked: self.clock,
            asked_before: None,
        };
        let out = derived.pixels();
        cache.entries.insert(key.clone(), derived);
        // 🚨 **The budget is enforced on insertion, not only from `prepare`**
        // (§15 D596, `[S9.2-L4-03]`). `evict_derived` used to be the only place
        // it was checked and `prepare` is its only caller — and every
        // non-interactive caller runs `prepare` **once, before the first read**:
        // `png::png` and `png::raster_of` both open with
        // `let mut images = ImageStore::new(); images.prepare(doc);` and then hand
        // the store to the renderer, which asks for this per distinct fill for the
        // whole render. So on the export and library-cover paths the cap was never
        // consulted at all. Measured in release on one 4000×3000 picture, driven
        // in the export's own order: four distinct adjustments held about 192 MB,
        // **eight held about 384 MB against a 256 MB budget**, and the number is
        // unbounded above — one per distinct `(ImageId, AdjustKey)` the walk
        // visits, each read exactly once and never again. The canvas shape, where
        // `prepare` keeps running, held at 240 MB on the same fixture.
        trim_derived(&mut cache, self.derived_budget, self.clock, Some(&key));
        Some(out)
    }

    /// A square thumbnail of what `r` shows, `edge` device pixels on a side, in
    /// the same straight-alpha RGBA8 [`Self::get`] answers in.
    ///
    /// **Deliberately not [`Self::pixels`], and for a cost rather than a
    /// convenience.** `pixels` is the input to the promotion machinery: it notes
    /// an ask, and the same ask on two consecutive frames builds the
    /// *full-resolution* adjusted buffer ([`SETTLED_FRAMES`]). A twenty-pixel
    /// chip asking that question would spend a twelve-megapixel pass on a
    /// picture nothing is drawing at size — on a hidden layer, or one scrolled
    /// off the canvas — and would hold [`Self::wants_frame`] true on its own
    /// behalf, so the app would keep repainting for a row. This touches neither
    /// clock, so a panel may ask as often as it likes and change nothing.
    ///
    /// **Reduce, then adjust — the opposite order to the canvas.** The two do
    /// not commute: the tone curve is nonlinear, so averaging before it and
    /// averaging after it are different pictures. They differ where one output
    /// block spans high contrast, bounded by the curve's slope across that
    /// block; at twenty pixels every block spans high contrast and no difference
    /// between them is visible. What it buys is the adjustment pass running over
    /// four hundred pixels instead of over twelve million.
    ///
    /// Orientation and crop **are** applied, because two layers sharing a
    /// picture and showing different halves of it are two different thumbnails.
    /// The framing mode is not: a chip has no box for Fill and Fit to argue
    /// about. What happens instead is a **centre-crop** — see [`square_of`].
    ///
    /// `None` for a picture the store cannot draw, which is the same answer
    /// [`Self::get`] gives and reaches the same place: the row keeps the glyph
    /// D179 marks in amber, so a missing picture and a thumbnail are mutually
    /// exclusive by construction rather than by a rule someone has to hold.
    pub fn thumbnail(&self, r: &ImageRef, edge: u32) -> Option<ImageData> {
        if edge == 0 {
            return None;
        }
        let base = self.entries.get(&r.id)?;
        let small = {
            let mut cache = self.derived.lock().unwrap_or_else(|e| e.into_inner());
            match cache.thumb_base.get(&r.id) {
                Some(d) => d.clone(),
                None => {
                    // `reduced` declines a picture under twice `THUMB_BASE`,
                    // which is already small enough to cut from directly.
                    let d = reduced(&base.data, THUMB_BASE)
                        .map_or_else(|| base.data.clone(), |(d, _)| d);
                    cache.thumb_base.insert(r.id.clone(), d.clone());
                    d
                }
            }
        };
        // The lock is gone before the sampling, which is the one part of this
        // that is not a hash lookup — and the scene walk is holding the other
        // end of it every frame.
        let square = square_of(&small, r, edge)?;
        Some(match r.adjust.pipeline() {
            Some(p) => adjusted_data(&square, &p),
            None => square,
        })
    }

    /// Decode `bytes` under `id` if it is not already held.
    ///
    /// The entry point for bytes that did not come out of the table — a linked
    /// image read off disk, an image being placed before it is committed.
    /// Returns what is held, so a caller placing an image can read the intrinsic
    /// size back without decoding it twice.
    pub fn insert(
        &mut self,
        id: &ImageId,
        bytes: &[u8],
        format: ImageFormat,
    ) -> Result<(u32, u32), DecodeError> {
        self.ensure(id, bytes, format);
        match self.entries.get(id) {
            Some(e) => Ok((e.data.width, e.data.height)),
            None => Err(self.failed.get(id).cloned().unwrap_or(DecodeError::Corrupt)),
        }
    }

    fn ensure(&mut self, id: &ImageId, bytes: &[u8], format: ImageFormat) {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.touched = self.clock;
            return;
        }
        if self.failed.contains_key(id) {
            return;
        }
        match decode(bytes, format) {
            Ok(data) => {
                let bytes = data.data.data().len();
                self.entries.insert(
                    id.clone(),
                    Entry {
                        data,
                        bytes,
                        touched: self.clock,
                    },
                );
            }
            Err(e) => {
                self.failed.insert(id.clone(), e);
            }
        }
    }

    /// Drop entries nothing saw this pass, oldest first, until the held bytes are
    /// back under budget.
    ///
    /// Oldest-*first* rather than largest-first: the biggest stale image is as
    /// likely as any to be the one an undo brings back, and recency is the only
    /// signal here that means anything.
    fn evict_stale(&mut self, now: u64) {
        if self.bytes_held() <= self.budget {
            return;
        }
        let mut stale: Vec<(u64, ImageId)> = self
            .entries
            .iter()
            .filter(|(_, e)| e.touched < now)
            .map(|(id, e)| (e.touched, id.clone()))
            .collect();
        stale.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let mut held = self.bytes_held();
        for (_, id) in stale {
            if held <= self.budget {
                break;
            }
            if let Some(e) = self.entries.remove(&id) {
                held -= e.bytes;
            }
            self.failed.remove(&id);
        }
    }

    /// The decoded pixels for `id`, or `None` for one that is missing, still a
    /// link, or refused.
    ///
    /// `None` is not an error at this seam: a fill whose image cannot be resolved
    /// reaches the missing-image path, which is a document that needs relinking
    /// rather than one that is broken (§5.5a).
    ///
    /// **This is what the placeholder is decided by, one step back**: the walk
    /// asks `ScenePainter::has_image`, both backends answer it with this, and a
    /// `None` here is what makes a fill come out as the grey cross rather than as
    /// the picture (§15 D179). The shape stays selectable either way — hit
    /// testing is geometric and knows nothing about paint.
    pub fn get(&self, id: &ImageId) -> Option<&ImageData> {
        self.entries.get(id).map(|e| &e.data)
    }

    /// Why `id` has no pixels, when the answer is that decoding refused them.
    pub fn failure(&self, id: &ImageId) -> Option<&DecodeError> {
        self.failed.get(id)
    }

    pub fn bytes_held(&self) -> usize {
        self.entries.values().map(|e| e.bytes).sum()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many adjusted buffers are held — decoded pictures do not count.
    pub fn derived_len(&self) -> usize {
        let cache = self.derived.lock().unwrap_or_else(|e| e.into_inner());
        cache.entries.len()
    }

    /// How many of those are the picture at its own size rather than a preview.
    ///
    /// **The number a scrub must not run up.** It is the observable difference
    /// between the two-frame rule and a one-frame one: both hand a moving slider a
    /// preview, and only the eager one *also* spends a full-resolution pass on
    /// every value it passed through. Nothing on screen distinguishes them, which
    /// is why this is asked rather than inferred.
    pub fn derived_full_len(&self) -> usize {
        let cache = self.derived.lock().unwrap_or_else(|e| e.into_inner());
        cache.entries.values().filter(|d| d.full).count()
    }
}

/// Encoded bytes to straight-alpha RGBA8.
///
/// Whether these bytes carry the signature of one of the four formats v1 accepts.
///
/// **Bytes `image` cannot place at all are accepted here**, and left to the hinted
/// decoder exactly as before: this answers "a format we can name and do not want",
/// not "a format we can decode". Narrowing it further would refuse payloads that
/// decode today.
///
/// 🚨 **Split out of [`fn@decode`] so that it can be asserted directly, and that is
/// not tidiness** (§15 D600). The end-to-end test — BMP bytes in, `Corrupt` out —
/// **cannot discriminate under `cargo test -p ondin-render`**, because that
/// spelling does not compile the `bmp` decoder and the bytes would be refused with
/// or without this guard. Only `cargo test --workspace` compiles the fifth decoder,
/// via `arboard`, and only there does removing this guard turn that test red. This
/// predicate has no such dependency: it reads magic bytes, so it says the same
/// thing under either spelling and a test on it is worth the same under both.
/// ⚠️ **That is `[A8-L3-02]`'s hole 4 deciding whether a *test* can fail**, where
/// every other instance of it has been about what a lint reports.
fn signature_is_accepted(bytes: &[u8]) -> bool {
    match image::guess_format(bytes) {
        Ok(f) => matches!(
            f,
            image::ImageFormat::Png
                | image::ImageFormat::Jpeg
                | image::ImageFormat::WebP
                | image::ImageFormat::Gif
        ),
        Err(_) => true,
    }
}

/// **The claimed format is a hint, not the authority.** `image` sniffs the
/// content, and the entry's `format` is what the *document* was told when the
/// bytes arrived — so a file renamed from `.png` to `.jpg` decodes as what it
/// actually is instead of failing. The format on the entry still matters: it is
/// what an SVG data URI and *Export original* write.
///
/// 🚨 **The four v1 accepts are decided here, in code, because a feature list
/// cannot decide them** (§15 D600). The workspace manifest asks `image` for
/// exactly `png,jpeg,webp,gif` with `default-features = false` and says so in a
/// comment — *"named here so adding a fifth is a decision"* — and **cargo unifies
/// features across the graph, so what is compiled is not what was asked for.**
/// Measured at `a01840d`: `cargo tree --workspace` reports `bmp,gif,jpeg,png,webp`
/// on this machine, the `bmp` arriving from `arboard`'s own
/// `[target."cfg(windows)".dependencies.image]`. The same manifest asks for
/// **`tiff`** under `cfg(target_os = "macos")` — the one format
/// `ondin_core::ImageFormat`'s doc names as excluded on purpose — and for nothing
/// extra on Linux. So the shipped decoder set was `{png,jpeg,webp,gif}` **plus a
/// different fifth per platform**.
///
/// What that bought, before this guard: `load_from_memory` sniffs with whatever
/// is compiled, so BMP bytes labelled `image/png` **drew the picture on Windows
/// and the missing-image placeholder on Linux** — one document, two platforms, no
/// report on either. Both doors are real. `app::image_format_of` falls through to
/// the file extension for anything it does not recognise, and `svg_in::data_uri`
/// maps the declared MIME straight through with no sniff at all, which is
/// untrusted input. `[S22-L2-02]`.
///
/// `image::guess_format` reads magic bytes and is **not** gated on the compiled
/// decoders, which is what makes the guard say the same thing everywhere — see
/// [`signature_is_accepted`].
fn decode(bytes: &[u8], format: ImageFormat) -> Result<ImageData, DecodeError> {
    let hinted = match format {
        ImageFormat::Png => image::ImageFormat::Png,
        ImageFormat::Jpeg => image::ImageFormat::Jpeg,
        ImageFormat::Webp => image::ImageFormat::WebP,
        ImageFormat::Gif => image::ImageFormat::Gif,
    };
    // **Only a *recognised* format is refused.** Bytes whose signature `image`
    // does not know are left to the hinted decoder exactly as before — the guard
    // is about a format we can name and do not accept, not about tightening what
    // reaches the decoder.
    if !signature_is_accepted(bytes) {
        return Err(DecodeError::Corrupt);
    }
    let decoded = image::load_from_memory(bytes)
        .or_else(|_| image::load_from_memory_with_format(bytes, hinted))
        .map_err(|_| DecodeError::Corrupt)?;
    let (width, height) = (decoded.width(), decoded.height());
    if width > MAX_EDGE || height > MAX_EDGE {
        return Err(DecodeError::TooLarge { width, height });
    }
    if width == 0 || height == 0 {
        return Err(DecodeError::Corrupt);
    }
    // **Before the expansion, not after** — `into_rgba8` is where the memory is
    // spent and where an allocation failure would abort the process, so a check
    // afterwards is a check that never runs. The dimensions are already known
    // here; that is the whole reason this can be one line (§15 D449).
    let bytes = u64::from(width) * u64::from(height) * 4;
    if bytes > MAX_DECODED_BYTES {
        return Err(DecodeError::TooMuchPixelData {
            width,
            height,
            bytes,
        });
    }
    let rgba = decoded.into_rgba8().into_raw();
    Ok(ImageData {
        data: Blob::new(Arc::new(rgba)),
        format: PenikoFormat::Rgba8,
        // `into_rgba8` gives straight alpha, which is what the document's colours
        // are and what `color.rs` says both backends take (invariant 7). The CPU
        // backend premultiplies on its own conversion; saying so here would make
        // it do it twice and wash every transparent edge out.
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    })
}

/// One over an 8-bit channel's full scale, in the fixed point [`AdjustLut`]
/// sums in. 16 bits of fraction leaves every term of the sum inside an `i32`
/// with room to spare, and rounds to a quarter-thousandth of a level.
const FIXED_ONE: i32 = 1 << 16;

/// What one input channel at one level contributes to each of the three output
/// channels, in [`FIXED_ONE`]ths.
///
/// **Indexed by the *input* channel first, and that is a measurement rather than
/// a preference.** Written the other way round — output first, the shape the
/// matrix itself has — one pixel is nine scattered loads; this way it is three
/// loads of twelve adjacent bytes, and the pass is a third faster for it.
type Contributions = Box<[[[i32; 3]; 256]; 3]>;

/// An [`AdjustPipeline`] compiled to lookup tables.
///
/// **This is a memoization of core's function, not a second copy of it**, and the
/// distinction is the whole reason it is safe. Every number in here is *read out
/// of* the pipeline or produced by calling [`ondin_core::transfer`]; the only
/// arithmetic this file adds is the summation and the 8-bit rounding, which is
/// what [`AdjustPipeline::apply`] does too.
/// `the_table_agrees_with_the_function_it_compiles` sweeps the two against each
/// other, and an `ondin export --png` of an adjusted document is byte-identical
/// to the one the arithmetic produced.
///
/// It exists because the straightforward spelling — one `apply` per pixel — costs
/// **11.5 ms** on an 800×500 photograph in a release build, and a scrub rebuilds
/// the whole buffer *every frame*. Compiled, that is **1.9 ms**. Reported as
/// adjustment sliders being laggy, with the values jumping.
///
/// **The tables and the dev profile's `opt-level` are separate fixes and the
/// numbers do not multiply**, which is worth stating because they look
/// interchangeable. Measured on the same photograph, both stages:
///
/// | | debug | release |
/// |---|---|---|
/// | one `apply` per pixel | 101.5 ms | 11.5 ms |
/// | these tables | 51.7 ms | 1.9 ms |
/// | tables + `opt-level = 3` | 2.1 ms | — |
///
/// So the tables are worth 6× where the optimiser is already running and only 2×
/// where it is not, and the profile is worth 25× on top of them. Neither alone
/// gets a debug build under a frame.
struct AdjustLut {
    /// The matrix as [`Contributions`], with its fifth column beside as the
    /// per-output-channel offset.
    matrix: Option<(Contributions, [i32; 3])>,
    /// The tone curve at each of the 256 levels a byte has.
    curve: Option<[u8; 256]>,
}

impl AdjustLut {
    fn build(p: &AdjustPipeline) -> Self {
        let matrix = p.matrix.map(|m| {
            let mut contrib = Box::new([[[0i32; 3]; 256]; 3]);
            for (inp, table) in contrib.iter_mut().enumerate() {
                for (v, cell) in table.iter_mut().enumerate() {
                    for (out, term) in cell.iter_mut().enumerate() {
                        let k = m[out * 5 + inp];
                        *term = (k * v as f32 / 255.0 * FIXED_ONE as f32).round() as i32;
                    }
                }
            }
            let offset = [0, 1, 2].map(|out| (m[out * 5 + 4] * FIXED_ONE as f32).round() as i32);
            (contrib, offset)
        });
        let curve = p.curve.map(|c| {
            let mut lut = [0u8; 256];
            for (v, cell) in lut.iter_mut().enumerate() {
                *cell = (ondin_core::transfer(&c, v as f32 / 255.0) * 255.0 + 0.5) as u8;
            }
            lut
        });
        Self { matrix, curve }
    }

    /// One pixel's colour channels, as bytes.
    ///
    /// **The tone curve reads a *quantized* matrix result**, where
    /// [`AdjustPipeline::apply`] passes it a float. That is the one place the two
    /// differ, it is bounded by the curve's slope at half a level — well under one
    /// level of output — and it is what an 8-bit filter implementation does
    /// anyway, since the answer is about to become a byte.
    #[inline]
    fn apply(&self, rgb: [u8; 3]) -> [u8; 3] {
        let mut v = rgb;
        if let Some((c, off)) = &self.matrix {
            let (cr, cg, cb) = (
                &c[0][rgb[0] as usize],
                &c[1][rgb[1] as usize],
                &c[2][rgb[2] as usize],
            );
            v = [0, 1, 2].map(|out| {
                let sum = cr[out] + cg[out] + cb[out] + off[out];
                // Clamped to `0..=1` before rounding, exactly as `apply` clamps
                // each filter primitive's result (SVG does the same).
                ((sum.clamp(0, FIXED_ONE) * 255 + FIXED_ONE / 2) >> 16) as u8
            });
        }
        if let Some(lut) = &self.curve {
            v = v.map(|x| lut[x as usize]);
        }
        v
    }
}

/// Bring the derived cache under `budget`, dropping least-useful first and never
/// `keep` (§15 D596, `[S9.2-L4-03]`).
///
/// **The order is stale before live, and within each, least recently asked
/// first.** Stale first is [`ImageStore::evict_derived`]'s own rule and the
/// reason is its: a buffer the walk is about to ask for again costs a
/// recomputation to evict, which is the thrash `PREVIEW_FLOOR` exists to prevent
/// in the font cache (§15 D160).
///
/// 🚨 **But a live buffer *is* evictable here, which is the change.** The old
/// budget loop filtered its candidates to `!live(d.asked)` and could therefore
/// free nothing at all when everything was live — which is exactly the state a
/// one-shot render produces: a single `prepare` sets `clock` to 1, every entry
/// the walk creates is asked at 1, and `live(1)` is true for all of them for the
/// whole render. **A cap that cannot evict anything is not a cap**, and
/// [`ADJUSTED_BYTES`]' own doc claims to be *"a working-set bound rather than a
/// waste bound"*. Preferring stale keeps the canvas's behaviour wherever there is
/// a choice; where there is not, something has to go and it is the oldest.
///
/// `keep` is the entry the caller has just built and is about to return — never a
/// candidate, or [`ImageStore::pixels`] could hand back a buffer it had already
/// dropped.
fn trim_derived(
    cache: &mut Adjusted,
    budget: usize,
    clock: u64,
    keep: Option<&(ImageId, AdjustKey)>,
) {
    let mut held: usize = cache.entries.values().map(|d| d.bytes).sum();
    if held <= budget {
        return;
    }
    let mut order: Vec<((ImageId, AdjustKey), (bool, u64))> = cache
        .entries
        .iter()
        .filter(|(k, _)| keep != Some(*k))
        .map(|(k, d)| (k.clone(), (d.asked + SETTLED_FRAMES >= clock, d.asked)))
        .collect();
    // `false < true`, so the stale ones sort first; then oldest ask; then the key
    // itself, so the order is total and two runs over one fixture agree.
    order.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    for (key, _) in order {
        if held <= budget {
            break;
        }
        if let Some(d) = cache.entries.remove(&key) {
            held -= d.bytes;
        }
    }
}

/// `src` with `pipeline` applied to every pixel.
///
/// **The arithmetic is core's, not this function's** —
/// [`AdjustPipeline::apply`] is the one description of what a slider does, and it
/// is the same one the SVG writer emits as a filter. What happens here is the
/// 8-bit boundary, and it goes through [`AdjustLut`], which is that same function
/// with its 256 possible inputs per channel worked out in advance.
///
/// **Alpha is copied through untouched, and it is straight rather than
/// premultiplied** — which is what `decode` produces and what both backends take
/// (invariant 7). Adjusting a premultiplied buffer would tint every soft edge,
/// and the bug would show only on pictures with transparency.
fn adjusted_data(src: &ImageData, pipeline: &AdjustPipeline) -> ImageData {
    let bytes = src.data.data();
    if src.format != PenikoFormat::Rgba8 || bytes.len() < (src.width * src.height * 4) as usize {
        return src.clone();
    }
    let lut = AdjustLut::build(pipeline);
    let mut out = vec![0u8; bytes.len()];
    for (dst, px) in out
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(bytes.as_chunks::<4>().0)
    {
        dst[..3].copy_from_slice(&lut.apply([px[0], px[1], px[2]]));
        dst[3] = px[3];
    }
    ImageData {
        data: Blob::new(Arc::new(out)),
        format: src.format,
        alpha_type: src.alpha_type,
        width: src.width,
        height: src.height,
    }
}

/// `src` reduced by the largest whole factor that still leaves its long edge at
/// or above `edge`, with the map from the reduced grid back onto the source's —
/// or [`None`] when no such factor exists, which is every picture under twice
/// `edge`.
///
/// **`edge` is a floor, not a ceiling, and that distinction was a reported bug.**
/// The factor is `long_edge / edge` rounded **down**, so the result is always
/// between `edge` and `2·edge`. Rounded *up* — which is the obvious spelling for
/// "no edge exceeds `edge`" — a 1200-pixel photograph reduces by 2, to 600: half
/// its resolution thrown away to get 17% under a threshold, and reported as a
/// "noticeable jump in quality" while scrubbing, which is exactly what it is. The
/// same spelling also has no way to decline, so *every* picture over the
/// threshold paid at least half.
///
/// The consequence worth knowing: a picture under `2·edge` is not reduced at all
/// and its scrub pays the full pass. That is the trade this is deliberately
/// making — see [`PREVIEW_EDGE`], where the numbers are.
///
/// **A box average in *premultiplied* space.** The buffer is straight alpha, so
/// averaging the colour channels directly would let a fully transparent pixel's
/// colour — which is arbitrary, and is often black — vote in the average, and
/// every soft edge in the preview would darken. Weighting by alpha and dividing
/// back out is what the premultiplied form is for; it is done here rather than
/// stored, because straight alpha is what the backends take.
///
/// **One integer factor for both axes**, so the reduction cannot change the
/// picture's proportions. The map back is nonetheless computed from the *actual*
/// dimensions rather than from the factor: the last row and column of blocks are
/// short whenever the factor does not divide the size, and a map built from the
/// factor would place the picture a fraction of a pixel large.
fn reduced(src: &ImageData, edge: u32) -> Option<(ImageData, Affine)> {
    let (w, h) = (src.width, src.height);
    if edge == 0 {
        return None;
    }
    // ⚠️ **Integer division, so [`PREVIEW_EDGE`] is a *floor* and not a cap** — a
    // picture merely over it is left alone, and two tests pin that. The atlas
    // budget wants the other reading, which is why it computes its own factor and
    // calls [`reduced_by`] rather than passing an edge here (§15 D745).
    let f = w.max(h) / edge;
    if f < 2 {
        return None;
    }
    reduced_by(src, f)
}

/// [`reduced`] at a factor the caller has already chosen.
///
/// **Split out for the atlas budget** (§15 D745, `[S9.2-L1-01]`), which needs a
/// *cap* where `reduced`'s `edge` is a floor: a 9504-pixel photograph has to come
/// back under 8192, and `reduced(src, 8192)` answers `None` because `9504 / 8192`
/// is 1. The pixel arithmetic is unchanged and lives here; `reduced` is now the
/// one-line floor rule on top of it.
fn reduced_by(src: &ImageData, f: u32) -> Option<(ImageData, Affine)> {
    let (w, h) = (src.width, src.height);
    let bytes = src.data.data();
    if f < 2 || src.format != PenikoFormat::Rgba8 || bytes.len() < (w * h * 4) as usize {
        return None;
    }
    let (pw, ph) = (w.div_ceil(f).max(1), h.div_ceil(f).max(1));
    let mut out = Vec::with_capacity((pw * ph * 4) as usize);
    for by in 0..ph {
        for bx in 0..pw {
            let (mut sr, mut sg, mut sb, mut sa, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for y in by * f..((by + 1) * f).min(h) {
                for x in bx * f..((bx + 1) * f).min(w) {
                    let i = ((y * w + x) * 4) as usize;
                    let a = bytes[i + 3] as u64;
                    sr += bytes[i] as u64 * a;
                    sg += bytes[i + 1] as u64 * a;
                    sb += bytes[i + 2] as u64 * a;
                    sa += a;
                    n += 1;
                }
            }
            let n = n.max(1);
            let (r, g, b) = match sa {
                0 => (0, 0, 0),
                _ => (
                    ((sr + sa / 2) / sa) as u8,
                    ((sg + sa / 2) / sa) as u8,
                    ((sb + sa / 2) / sa) as u8,
                ),
            };
            out.extend([r, g, b, ((sa + n / 2) / n) as u8]);
        }
    }
    Some((
        ImageData {
            data: Blob::new(Arc::new(out)),
            format: src.format,
            alpha_type: src.alpha_type,
            width: pw,
            height: ph,
        },
        Affine::scale_non_uniform(w as f64 / pw as f64, h as f64 / ph as f64),
    ))
}

/// The centre square of what `r` frames, cut out of `src` at `edge` pixels a
/// side.
///
/// `src` is the picture already reduced to [`THUMB_BASE`], so the box average
/// below runs over a few source pixels per output and the whole function is a
/// few hundred reads. The averaging is alpha-weighted for [`reduced`]'s reason:
/// a straight-alpha buffer's transparent pixels carry an arbitrary colour,
/// usually black, and letting it vote darkens every soft edge.
///
/// **Centre-crop, not fit.** Fitting a 16:9 photograph into the square would
/// draw it twenty wide and eleven tall with the ground showing above and below,
/// and a column of those reads as a list of slivers rather than as pictures.
/// Filling the square costs the picture's ends, which at this size is not
/// information anyone was reading.
fn square_of(src: &ImageData, r: &ImageRef, edge: u32) -> Option<ImageData> {
    let (bw, bh) = (src.width, src.height);
    let bytes = src.data.data();
    if src.format != PenikoFormat::Rgba8 || bytes.len() < (bw * bh * 4) as usize {
        return None;
    }
    let (q, mirrored) = (r.orient.quarters(), r.orient.mirrored);
    // The picture as the orientation leaves it: an odd number of quarter turns
    // trades the axes, and `crop` is normalized to *this* rather than to the
    // source (§5.5a), which is what lets the rectangle be read without
    // un-orienting it first.
    let (ow, oh) = match q % 2 {
        1 => (bh, bw),
        _ => (bw, bh),
    };
    let (fow, foh) = (f64::from(ow), f64::from(oh));
    // Clamped rather than refused. Core stores the rectangle it is given and
    // documents that coordinates outside `0..1` sample the edge pixel, since
    // `Extend::Pad` is the only clamp peniko has; a clamp here is that same
    // answer, arrived at one step earlier.
    let x0 = r.crop.x0.min(r.crop.x1).clamp(0.0, 1.0) * fow;
    let x1 = r.crop.x0.max(r.crop.x1).clamp(0.0, 1.0) * fow;
    let y0 = r.crop.y0.min(r.crop.y1).clamp(0.0, 1.0) * foh;
    let y1 = r.crop.y0.max(r.crop.y1).clamp(0.0, 1.0) * foh;
    // A rectangle with no pixels in it shows the whole picture rather than
    // nothing: an empty thumbnail reads as a broken image, which is a different
    // statement and one D179's amber glyph already owns.
    let (x0, x1, y0, y1) = match (x1 - x0 >= 1.0, y1 - y0 >= 1.0) {
        (true, true) => (x0, x1, y0, y1),
        _ => (0.0, fow, 0.0, foh),
    };
    let side = (x1 - x0).min(y1 - y0);
    let (sx, sy) = (x0 + (x1 - x0 - side) / 2.0, y0 + (y1 - y0 - side) / 2.0);
    let step = side / f64::from(edge);
    let mut out = Vec::with_capacity((edge * edge * 4) as usize);
    for py in 0..edge {
        for px in 0..edge {
            let (fx, fy) = (sx + f64::from(px) * step, sy + f64::from(py) * step);
            // **At least one whole source pixel per output block, however small
            // the step.** A crop tighter than the thumbnail is a zoom, and there
            // the block is a fraction of a pixel wide — `ceil` alone would then
            // agree with `floor` and the loop below would average nothing.
            let ix0 = (fx.floor().max(0.0) as u32).min(ow - 1);
            let iy0 = (fy.floor().max(0.0) as u32).min(oh - 1);
            let ix1 = ((fx + step).ceil().max(0.0) as u32).clamp(ix0 + 1, ow);
            let iy1 = ((fy + step).ceil().max(0.0) as u32).clamp(iy0 + 1, oh);
            let (mut sr, mut sg, mut sb, mut sa, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for oy in iy0..iy1 {
                for ox in ix0..ix1 {
                    let (bx, by) = un_orient(ox, oy, bw, bh, q, mirrored);
                    let i = ((by * bw + bx) * 4) as usize;
                    let a = u64::from(bytes[i + 3]);
                    sr += u64::from(bytes[i]) * a;
                    sg += u64::from(bytes[i + 1]) * a;
                    sb += u64::from(bytes[i + 2]) * a;
                    sa += a;
                    n += 1;
                }
            }
            let n = n.max(1);
            let (red, green, blue) = match sa {
                0 => (0, 0, 0),
                _ => (
                    ((sr + sa / 2) / sa) as u8,
                    ((sg + sa / 2) / sa) as u8,
                    ((sb + sa / 2) / sa) as u8,
                ),
            };
            out.extend([red, green, blue, ((sa + n / 2) / n) as u8]);
        }
    }
    Some(ImageData {
        data: Blob::new(Arc::new(out)),
        format: src.format,
        alpha_type: src.alpha_type,
        width: edge,
        height: edge,
    })
}

/// Which source pixel the *oriented* picture's `(ox, oy)` comes from.
///
/// `(bw, bh)` are the **source's** dimensions; the caller holds the oriented
/// ones, which trade places for an odd `q`.
///
/// This is the inverse of `ImageOrient`'s composition — mirror first, then `q`
/// quarter turns clockwise. Written out per case rather than as a matrix because
/// it is read once per sample and there are only four of them. The forward maps,
/// for anyone checking the algebra: a mirrored picture's `(mx, my)` goes to
/// `(bh-1-my, mx)` under one quarter turn, `(bw-1-mx, bh-1-my)` under two, and
/// `(my, bw-1-mx)` under three. What follows is those three solved for
/// `(mx, my)` — and solving them is the point, because the forward map for one
/// turn is the inverse map for *three*, so writing the obvious thing here turns
/// every picture the wrong way and only an asymmetric fixture can tell.
fn un_orient(ox: u32, oy: u32, bw: u32, bh: u32, q: u8, mirrored: bool) -> (u32, u32) {
    let (mx, my) = match q % 4 {
        1 => (oy, bh - 1 - ox),
        2 => (bw - 1 - ox, bh - 1 - oy),
        3 => (bw - 1 - oy, ox),
        _ => (ox, oy),
    };
    (if mirrored { bw - 1 - mx } else { mx }, my)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The atlas budget reproduces the finding's own measurements, row for
    /// row** — `[S9.2-L1-01]`, §15 D745.
    ///
    /// `atlas_downscale` is a model of somebody else's allocator, so the question
    /// that matters is not whether it is conservative but whether it agrees with
    /// what was *measured on the device*. The finding's table is the fixture:
    ///
    /// | document | GPU opaque px | CPU opaque px |
    /// | --- | ---: | ---: |
    /// | one 8192×64 picture | 3600 | 3600 |
    /// | one **8193**×64 picture | **0** | 3600 |
    /// | 2 × 4500×3000 photographs | 1250 | 1250 |
    /// | **3** × 4500×3000 photographs | **1250** | 1875 |
    ///
    /// Every row where the GPU column matches the CPU one must come back at
    /// factor 1, and every row where it does not must come back above it. A model
    /// that shrank the two-photograph page would be *safe* and would break the
    /// maintainer's decision, which is full resolution until it stops fitting.
    ///
    /// ⚠️ **The 8192/8193 pair is the boundary and is why the test is written with
    /// both.** `DEFAULT_ATLAS_SIZE` is 1024 and `bump_size` doubles, so the atlas
    /// arrives at exactly 8192 and one pixel more is the whole difference. A test
    /// with only the 8193 row would pass against a model that refused everything.
    ///
    /// ⚠️ Flipped by sorting `fits_by_shelf` ascending instead of tallest-first:
    /// the three-photograph row still fails to fit, so **that flip does not bite
    /// here** — the ordering matters for mixed heights, which
    /// `a_short_picture_does_not_open_a_shelf_a_tall_one_has_to_leave` covers.
    #[test]
    fn the_atlas_budget_agrees_with_what_the_device_measured() {
        let photo = (4500, 3000);
        assert_eq!(
            atlas_downscale(&[(8192, 64)], ATLAS_EDGE),
            1,
            "a picture at exactly the atlas edge fits"
        );
        assert!(
            atlas_downscale(&[(8193, 64)], ATLAS_EDGE) > 1,
            "and one pixel more does not"
        );
        assert_eq!(
            atlas_downscale(&[photo, photo], ATLAS_EDGE),
            1,
            "two photographs fit at full resolution and must not be touched"
        );
        assert!(
            atlas_downscale(&[photo, photo, photo], ATLAS_EDGE) > 1,
            "the third is what the user loses today, so it is what must shrink"
        );
        assert_eq!(
            atlas_downscale(&[], ATLAS_EDGE),
            1,
            "a page with no pictures on it"
        );
    }

    /// **The reduced set actually fits**, which is the property `atlas_downscale`
    /// exists for and is not the same claim as "the factor is bigger than one".
    ///
    /// A factor that came back 2 while the page still overflowed would pass the
    /// test above and lose the photograph anyway — the failure this whole entry is
    /// about, one step further along.
    #[test]
    fn whatever_factor_comes_back_makes_the_page_fit() {
        for page in [
            vec![(4500u32, 3000u32); 3],
            vec![(4500, 3000); 9],
            vec![(9504, 6336)],
            vec![(8193, 8193)],
            vec![(1200, 800); 40],
        ] {
            let f = atlas_downscale(&page, ATLAS_EDGE);
            let scaled: Vec<(u32, u32)> = page
                .iter()
                .map(|(w, h)| (w.div_ceil(f).max(1), h.div_ceil(f).max(1)))
                .collect();
            assert!(
                fits_by_shelf(&scaled, ATLAS_EDGE),
                "{page:?} at factor {f} still does not fit"
            );
        }
    }

    /// Tallest-first is what makes the shelf simulation worth running, and it
    /// takes three pictures to show it.
    ///
    /// ⚠️ **The obvious two-picture fixture proves nothing and was written first.**
    /// A shelf's height is the tallest thing on it, so with two pictures the order
    /// cannot matter: either they share one shelf and its height is the max
    /// whichever went down first, or they are on two shelves and the total is the
    /// sum. `[(4096, 100), (4096, 8000)]` fits with the sort and **fits without
    /// it**, so the flip that was supposed to bite came back green — which is the
    /// finding, not a failed experiment.
    ///
    /// Three 4096-wide pictures, two 5000 tall and one 800. In the order given the
    /// short one shares the first shelf with a tall one, so that shelf is 5000
    /// high, and the second tall one wraps to y = 5000 and needs 10 000. Sorted
    /// tallest-first the two tall ones share the first shelf and the short one
    /// wraps to 5800. The atlas is 8192, so the order decides it.
    ///
    /// ⚠️ Flipped by removing the `sort_by`: this fails. **Both fixtures are kept**
    /// — the two-picture one as the control that says the assertion is about the
    /// ordering and not about shelf packing in general.
    #[test]
    fn a_short_picture_does_not_open_a_shelf_a_tall_one_has_to_leave() {
        assert!(
            fits_by_shelf(&[(4096, 5000), (4096, 800), (4096, 5000)], ATLAS_EDGE),
            "sorted tallest-first the two tall ones share a shelf"
        );
        assert!(
            fits_by_shelf(&[(4096, 100), (4096, 8000)], ATLAS_EDGE),
            "the control: with two pictures the order cannot matter"
        );
    }

    /// A 2×2 PNG: red, green / blue, transparent. Written by hand rather than
    /// fixtured, so the test has no file to lose — and the pixels are known, which
    /// is what lets the decode be checked rather than merely not-crashing.
    fn png_2x2() -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, 2, 2);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[
                255, 0, 0, 255, // red
                0, 255, 0, 255, // green
                0, 0, 255, 255, // blue
                0, 0, 0, 0, // transparent
            ])
            .unwrap();
        }
        out
    }

    fn id(s: &str) -> ImageId {
        ImageId(s.into())
    }

    /// A uniform **grayscale** PNG, `w` × `h`.
    ///
    /// Grayscale on purpose: one byte a pixel, so it compresses to almost nothing
    /// and `into_rgba8` multiplies it by **four**. That ratio is the whole subject
    /// of `a_pictures_rgba_expansion_is_bounded_in_rgba_bytes`.
    fn gray_png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::Eight);
            let mut wr = enc.write_header().unwrap();
            wr.write_image_data(&vec![128u8; (w as usize) * (h as usize)])
                .unwrap();
        }
        out
    }

    /// A 1×1 red BMP — 24-bit, `BITMAPINFOHEADER`, no compression — written by
    /// hand for the same reason `png_2x2` is: a format the workspace does not
    /// accept has no encoder here to make one with.
    ///
    /// It is a *valid* BMP on purpose. A malformed one would be refused by every
    /// build and would prove nothing about the guard.
    fn bmp_1x1() -> Vec<u8> {
        let mut out = Vec::with_capacity(58);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&58u32.to_le_bytes()); // file size
        out.extend_from_slice(&0u32.to_le_bytes()); // reserved
        out.extend_from_slice(&54u32.to_le_bytes()); // offset to pixels
        out.extend_from_slice(&40u32.to_le_bytes()); // DIB header size
        out.extend_from_slice(&1i32.to_le_bytes()); // width
        out.extend_from_slice(&1i32.to_le_bytes()); // height
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&24u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        out.extend_from_slice(&4u32.to_le_bytes()); // image size
        out.extend_from_slice(&2835i32.to_le_bytes()); // x pixels per metre
        out.extend_from_slice(&2835i32.to_le_bytes()); // y pixels per metre
        out.extend_from_slice(&0u32.to_le_bytes()); // palette entries used
        out.extend_from_slice(&0u32.to_le_bytes()); // palette entries required
        out.extend_from_slice(&[0, 0, 255, 0]); // one BGR pixel, then the row pad
        out
    }

    /// 🚨 **A format the workspace does not accept is refused on every platform,
    /// and until §15 D600 it was decoded on this one.**
    ///
    /// `[S22-L2-02]`: the manifest's *"four formats, and a fifth is a decision"* is
    /// defeated by cargo's feature unification. `arboard` asks the same `image`
    /// build for `bmp` on Windows and `tiff` on macOS, and `load_from_memory`
    /// sniffs with whatever ends up compiled — so these bytes, labelled
    /// `ImageFormat::Png` by a file extension or by an SVG data URI's MIME type,
    /// **drew a red pixel here and the missing-image placeholder on Linux.** Same
    /// document, two platforms, no report on either.
    ///
    /// 🚨 **The end-to-end half of this test can only fail under one of the two
    /// spellings of `cargo test`, and that is the finding's own mechanism deciding
    /// whether its test has teeth.**
    ///
    /// Flip-check, run, and it took two runs to read correctly. Widening the guard
    /// to admit `Bmp` and running `cargo test -p ondin-render` left this **green** —
    /// which looks like a vacuous test and is not: `-p ondin-render` resolves
    /// `image` to `gif,jpeg,png,webp` and `--workspace` resolves it to
    /// `bmp,gif,jpeg,png,webp`, so under `-p` there is no BMP decoder for the guard
    /// to be protecting anything from. The same flip under
    /// **`cargo test --workspace`** fails at the `Err(Corrupt)` assertion with
    /// `left: Ok((1, 1))` — the red pixel, decoded. ⚠️ **So a flip-check of this
    /// guard is only meaningful workspace-wide**, and `[A8-L3-02]`'s hole 4 has
    /// reached a third kind of victim: it has changed what a lint reports, what a
    /// `size_of` measures, and now **what a test can observe.**
    ///
    /// The predicate assertion below is the half with no such dependency —
    /// `signature_is_accepted` reads magic bytes and answers the same under either
    /// spelling — which is why the guard was split out of `decode` rather than
    /// written inline. (Plain backticks, §15 D319: `cargo doc` cannot see a
    /// `cfg(test)` module, so a link here is checked by nothing.)
    ///
    /// The control is `a_png_is_still_a_png` beside it: `guess_format` is consulted
    /// on every decode now, so an accepted format has to come through it unchanged.
    #[test]
    fn a_format_outside_the_four_is_refused_whatever_the_platform_compiled() {
        // The fixture has to actually be a BMP, or the refusal below is about
        // nothing — only that some bytes failed to decode.
        assert_eq!(
            image::guess_format(&bmp_1x1()).ok(),
            Some(image::ImageFormat::Bmp),
            "the fixture is not a BMP"
        );
        // The guard itself, which says the same thing under `-p` and `--workspace`.
        assert!(
            !signature_is_accepted(&bmp_1x1()),
            "a fifth format is refused by the code, whatever cargo compiled"
        );
        assert!(
            signature_is_accepted(&png_2x2()),
            "and the four are not refused"
        );

        let mut store = ImageStore::new();
        assert_eq!(
            store.insert(&id("mislabelled"), &bmp_1x1(), ImageFormat::Png),
            Err(DecodeError::Corrupt),
            "BMP bytes under a PNG label are not one of the four v1 accepts"
        );
        assert!(store.is_empty(), "and nothing is held");
    }

    /// The control for the guard above: an accepted format still decodes, pixels
    /// and all. `guess_format` now runs on every call, so this is what says it does
    /// not refuse the four it is there to admit.
    #[test]
    fn a_png_is_still_a_png() {
        let mut store = ImageStore::new();
        assert_eq!(
            store.insert(&id("ok"), &png_2x2(), ImageFormat::Png),
            Ok((2, 2))
        );
        let data = store.get(&id("ok")).expect("decoded");
        assert_eq!(data.data.as_ref()[..4], [255, 0, 0, 255], "the red pixel");
    }

    /// **A picture's RGBA expansion is bounded in RGBA bytes**, which is the one
    /// denomination none of the three existing caps was in.
    ///
    /// `image`'s `max_alloc` bounds the *decoder's* buffer in the decoder's own
    /// format; `MAX_EDGE` bounds **edges**; `DECODED_BYTES` is in the right
    /// unit and weighs only **stale** entries, so a picture the document still
    /// refers to is never measured against it. Grayscale is one byte a pixel and
    /// `into_rgba8` multiplies by four, so the gap between the caps and the
    /// allocation is 4× on top of whatever the decoder already let through.
    ///
    /// Measured, release, through `insert`: a **76 KB** 8000×8000 grayscale PNG
    /// held **256 MB** after 433 ms, a 3,356× amplification, and `20000×20000`
    /// reaches 1.6 GB — where a `Vec` allocation failure is `handle_alloc_error`,
    /// an **abort** nothing on the path can catch.
    ///
    /// ⚠️ **The control is doing the real work here.** The cap sits at 256 MiB
    /// because a **50 MP camera file is legitimate** — a Sony A7R V is 9504×6336,
    /// 240 MB of RGBA — so the assertion that it is *accepted* is what stops a
    /// later tightening quietly making the app refuse ordinary photographs. The
    /// pathological case and the legitimate case are the same order of magnitude,
    /// which is why this cannot be set tight and why the 433 ms freeze is a
    /// **threading** problem rather than a bound one.
    ///
    /// ⚠️ **Flipped against the plausible wrong version, not against nothing.** A
    /// cap applied to the *file* bytes — the obvious reading of "refuse huge
    /// images" — leaves this green at 105 KB, which is what says the denomination
    /// is the finding. Flipped by deleting the guard: fails on the `Err`
    /// assertion, the predicted site, and the store then holds 324 MB.
    #[test]
    fn a_pictures_rgba_expansion_is_bounded_in_rgba_bytes() {
        // 9000² is 324 MB of RGBA — over the cap — from a file of ~100 KB.
        let bytes = gray_png(9000, 9000);
        let mut store = ImageStore::new();
        let got = store.insert(&id("huge"), &bytes, ImageFormat::Png);

        assert_eq!(
            got,
            Err(DecodeError::TooMuchPixelData {
                width: 9000,
                height: 9000,
                bytes: 9000 * 9000 * 4,
            }),
            "a {} byte file expanding to 324 MB is refused",
            bytes.len()
        );
        assert!(
            store.is_empty(),
            "and nothing is held: {} bytes",
            store.bytes_held()
        );

        // The control, and the reason the cap is not tighter: a 50 MP camera file
        // is an ordinary thing to place. 9504×6336 is 240 MB and must come in.
        let mut store = ImageStore::new();
        let camera = gray_png(9504, 6336);
        assert_eq!(
            store.insert(&id("a7rv"), &camera, ImageFormat::Png),
            Ok((9504, 6336)),
            "a 50 MP photograph is not what this cap is for"
        );
    }

    #[test]
    fn a_png_decodes_to_straight_alpha_rgba() {
        let mut store = ImageStore::new();
        let size = store
            .insert(&id("a"), &png_2x2(), ImageFormat::Png)
            .expect("a 2x2 PNG decodes");
        assert_eq!(size, (2, 2));

        let data = store.get(&id("a")).unwrap();
        assert_eq!(data.format, PenikoFormat::Rgba8);
        assert_eq!(
            data.alpha_type,
            ImageAlphaType::Alpha,
            "the backends premultiply; claiming premultiplied here doubles it"
        );
        assert_eq!(data.data.data().len(), 2 * 2 * 4);
        assert_eq!(
            &data.data.data()[0..4],
            &[255, 0, 0, 255],
            "top-left is red"
        );
        assert_eq!(
            &data.data.data()[12..16],
            &[0, 0, 0, 0],
            "bottom-right is transparent"
        );
    }

    /// **Decoding happens once**, which is the whole point of the cache: the same
    /// id asked for twice must not decode twice, and two fills sharing an image
    /// share one buffer.
    #[test]
    fn a_second_ask_for_the_same_id_reuses_the_decoded_buffer() {
        let mut store = ImageStore::new();
        store
            .insert(&id("a"), &png_2x2(), ImageFormat::Png)
            .unwrap();
        let first = store.get(&id("a")).unwrap().data.data().as_ptr();

        // Different bytes under the same id: an id is a content hash, so this
        // cannot happen honestly — and asserting the *pointer* is unchanged is
        // what proves the second call decoded nothing rather than decoding to an
        // equal buffer.
        store
            .insert(&id("a"), &[0xFF; 16], ImageFormat::Png)
            .unwrap();
        assert_eq!(store.len(), 1);
        assert_eq!(
            store.get(&id("a")).unwrap().data.data().as_ptr(),
            first,
            "the second insert re-decoded instead of hitting the cache"
        );
    }

    #[test]
    fn corrupt_bytes_are_refused_once_and_remembered() {
        let mut store = ImageStore::new();
        let err = store.insert(&id("bad"), b"not an image", ImageFormat::Png);
        assert_eq!(err, Err(DecodeError::Corrupt));
        assert_eq!(store.get(&id("bad")), None);
        assert_eq!(store.failure(&id("bad")), Some(&DecodeError::Corrupt));
        assert!(store.is_empty(), "a refusal must not occupy the budget");
    }

    /// **A mislabelled image decodes as what it is.** The format on the entry is
    /// what the document was told; the bytes are the authority.
    #[test]
    fn the_claimed_format_does_not_have_to_be_right() {
        let mut store = ImageStore::new();
        let size = store.insert(&id("a"), &png_2x2(), ImageFormat::Jpeg);
        assert_eq!(size, Ok((2, 2)), "a PNG labelled JPEG still decodes");
    }

    /// A square PNG `n`×`n`, so a test can ask for a known number of decoded
    /// bytes: `n * n * 4`.
    fn png_square(n: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, n, n);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&vec![128u8; (n * n * 4) as usize])
                .unwrap();
        }
        out
    }

    /// A document carrying `ids`, each an `n`×`n` embedded PNG.
    fn doc_with(ids: &[&str], n: u32) -> ondin_core::Document {
        let mut source = ondin_core::IdSource::new(3);
        let mut doc = ondin_core::Document::new(source.mint());
        let ops = ids
            .iter()
            .map(|s| ondin_core::Operation::AddImage {
                id: ImageId((*s).into()),
                entry: ondin_core::ImageEntry {
                    source: CoreSource::Embedded(png_square(n).into()),
                    format: ImageFormat::Png,
                    width: n,
                    height: n,
                },
            })
            .collect();
        doc.apply(&ondin_core::Transaction(ops)).unwrap();
        doc
    }

    /// **`prepare` decodes what the document carries and counts the bytes.**
    ///
    /// The fixture is asserted before the budget is: a test about eviction is
    /// about nothing if the images never landed in the first place.
    #[test]
    fn prepare_decodes_the_documents_images_and_counts_their_decoded_size() {
        let doc = doc_with(&["a", "b"], 8);
        let mut store = ImageStore::new();
        store.prepare(&doc);
        assert_eq!(store.len(), 2);
        assert_eq!(
            store.bytes_held(),
            2 * 8 * 8 * 4,
            "the budget counts decoded RGBA, not the encoded PNG"
        );
    }

    /// **An image the document no longer carries is dropped once that puts the
    /// store over budget** — and not before, because an undo may be about to ask
    /// for it again.
    #[test]
    fn an_image_the_document_dropped_is_evicted_oldest_first_when_over_budget() {
        let one = 8 * 8 * 4;
        // Room for exactly three.
        let mut store = ImageStore::with_budget(one * 3);

        // **Two entries have to go stale at *different* times and both survive to
        // the eviction**, or this is not a test of recency at all: staleness alone
        // would leave them on one clock and the id tie-break would decide the
        // outcome without `touched` ever being read. That means staying under the
        // budget while they accumulate, and then crossing it with a *new* image —
        // which is exactly how it happens in use, when an undo brings a layer back
        // and a fresh one is placed on top.
        store.prepare(&doc_with(&["a", "b", "c"], 8));
        store.prepare(&doc_with(&["b", "c"], 8)); // "a" goes stale at clock 2
        store.prepare(&doc_with(&["c"], 8)); // "b" goes stale at clock 3
        assert_eq!(store.len(), 3, "nothing is dropped while under budget");

        store.prepare(&doc_with(&["c", "d"], 8)); // "d" pushes it over

        assert_eq!(store.bytes_held(), one * 3, "back to the budget, not below");
        assert!(
            store.get(&ImageId("c".into())).is_some(),
            "still referenced"
        );
        assert!(store.get(&ImageId("d".into())).is_some(), "just decoded");
        assert!(
            store.get(&ImageId("a".into())).is_none(),
            "the *oldest* stale entry is the one that goes"
        );
        assert!(
            store.get(&ImageId("b".into())).is_some(),
            "eviction stops at the budget rather than clearing everything stale"
        );
    }

    /// **Nothing the document still carries is ever evicted**, however far over
    /// budget that leaves the store.
    ///
    /// This is the floor, and it is the whole reason the budget is described as
    /// bounding *waste* rather than the working set. Without it a document with
    /// more images than the budget would decode one, evict it to make room for the
    /// next, and do it again on the following frame — the thrash `PREVIEW_FLOOR`
    /// exists to prevent in the font cache (§15 D160).
    #[test]
    fn an_image_the_document_still_carries_is_never_evicted() {
        let one = 8 * 8 * 4;
        // Room for less than one of them.
        let mut store = ImageStore::with_budget(one / 2);
        let doc = doc_with(&["a", "b", "c"], 8);

        for _ in 0..3 {
            store.prepare(&doc);
            assert_eq!(
                store.len(),
                3,
                "a referenced image was evicted, so the next frame re-decodes it"
            );
        }
        assert_eq!(store.bytes_held(), one * 3, "over budget, and correctly so");
    }

    /// **A linked image is not decoded by `prepare`** — its bytes are on disk, and
    /// reading files is not this crate's job. Until something calls `insert` with
    /// them it behaves exactly as a missing image, which is the same state as a
    /// link whose file has moved.
    #[test]
    fn a_linked_image_is_left_for_its_caller_to_supply() {
        let mut source = ondin_core::IdSource::new(5);
        let mut doc = ondin_core::Document::new(source.mint());
        let id = ImageId("linked".into());
        doc.apply(&ondin_core::Transaction(vec![
            ondin_core::Operation::AddImage {
                id: id.clone(),
                entry: ondin_core::ImageEntry {
                    source: CoreSource::Linked("/photos/hero.png".into()),
                    format: ImageFormat::Png,
                    width: 8,
                    height: 8,
                },
            },
        ]))
        .unwrap();

        let mut store = ImageStore::new();
        store.prepare(&doc);
        assert!(store.is_empty(), "prepare must not read the filesystem");
        assert_eq!(store.get(&id), None);

        // And once its caller supplies the bytes, it is an image like any other.
        store.insert(&id, &png_square(8), ImageFormat::Png).unwrap();
        assert!(store.get(&id).is_some());
    }

    mod adjust_tests {
        use super::*;

        fn reference(name: &str, set: impl FnOnce(&mut ImageAdjust)) -> ImageRef {
            let mut r = ImageRef::new(id(name));
            set(&mut r.adjust);
            r
        }

        /// A store already holding one `n`×`n` picture under `"a"`.
        fn store_with(n: u32, previews: bool) -> ImageStore {
            let mut store = if previews {
                ImageStore::interactive()
            } else {
                ImageStore::new()
            };
            store
                .insert(&id("a"), &png_square(n), ImageFormat::Png)
                .expect("the fixture decodes");
            store
        }

        /// **An unadjusted picture costs nothing at all** — not a second buffer, not
        /// a cache entry, not a copy of the pixels.
        ///
        /// The pointer is the assertion. Every image in every document that has never
        /// been adjusted takes this path, so a version that built a derived buffer for
        /// the neutral value would double the memory of every document and look
        /// identical on screen.
        #[test]
        fn an_unadjusted_picture_resolves_to_the_decode_itself() {
            let store = store_with(8, true);
            let plain = ImageRef::new(id("a"));
            let px = store.pixels(&plain).expect("the store holds it");
            assert_eq!(px.to_source, Affine::IDENTITY);
            assert_eq!(
                px.data.data.data().as_ptr(),
                store.get(&id("a")).unwrap().data.data().as_ptr(),
                "a neutral adjustment must hand back the decode, not a copy of it"
            );
            assert!(
                !store.wants_frame(),
                "and it must not leave a promotion owed"
            );
        }

        /// **The key is the picture *and* what was done to it.**
        ///
        /// Two halves, and each fails on its own: keyed too loosely, two fills at
        /// two exposures paint from one buffer and the second looks like the first;
        /// keyed too tightly — by anything that changes frame to frame — the work is
        /// redone every frame and the scrub is the thing that suffers.
        #[test]
        fn adjusted_pixels_are_keyed_by_the_picture_and_the_adjustment() {
            let store = store_with(8, false);
            let warm = store
                .pixels(&reference("a", |a| a.temperature = 0.5))
                .unwrap();
            let cool = store
                .pixels(&reference("a", |a| a.temperature = -0.5))
                .unwrap();
            assert_ne!(
                warm.data.data.data(),
                cool.data.data.data(),
                "two adjustments of one picture are two pictures"
            );

            let again = store
                .pixels(&reference("a", |a| a.temperature = 0.5))
                .unwrap();
            assert_eq!(
                again.data.data.data().as_ptr(),
                warm.data.data.data().as_ptr(),
                "the same adjustment asked for twice re-did the work"
            );
        }

        /// **An export store never answers with a preview**, however large the
        /// picture.
        ///
        /// This is the trap the whole preview mechanism creates, and it is a silent
        /// one: `ondin export --png` would write a soft photograph and nothing on
        /// screen would say so. The assertion is on the *dimensions* rather than on a
        /// flag, because dimensions are what would actually be wrong.
        #[test]
        fn only_an_interactive_store_ever_reduces_a_picture() {
            let big = PREVIEW_EDGE * 2;
            let r = reference("a", |a| a.exposure = 0.5);

            let exact = store_with(big, false);
            let px = exact.pixels(&r).expect("held");
            assert_eq!(
                (px.data.width, px.data.height),
                (big, big),
                "an export store must adjust the picture, not a reduction of it"
            );
            assert_eq!(px.to_source, Affine::IDENTITY);

            let live = store_with(big, true);
            let px = live.pixels(&r).expect("held");
            assert!(
                px.data.width >= PREVIEW_EDGE && px.data.width < big,
                "the canvas answers a fresh adjustment with a reduction that stops \
                 at the floor, got {}",
                px.data.width
            );
            assert_ne!(px.to_source, Affine::IDENTITY);
        }

        /// **`PREVIEW_EDGE` is a floor, so a picture that is merely *over* it is
        /// not reduced at all** — and one that is reduced never drops below it.
        ///
        /// This is a reported bug, and the reported symptom is the assertion: the
        /// rule was "reduce until no edge exceeds the threshold", spelled with a
        /// round-*up* factor, which halved a 1200-pixel photograph to 600 to get it
        /// 17% under a line — half the resolution thrown away for nothing, seen as
        /// a "noticeable jump in quality" the moment a slider was touched. Both
        /// arms matter and the wrong version fails the first: rounding up, *every*
        /// size here reduces.
        #[test]
        fn a_picture_just_over_the_floor_keeps_all_of_itself() {
            let probe = |n: u32| {
                let mut store = ImageStore::interactive();
                store
                    .insert(&id("a"), &png_square(n), ImageFormat::Png)
                    .expect("the fixture decodes");
                store
                    .pixels(&reference("a", |a| a.exposure = 0.5))
                    .expect("held")
                    .data
                    .width
            };
            for n in [PREVIEW_EDGE + 1, PREVIEW_EDGE * 2 - 1] {
                assert_eq!(
                    probe(n),
                    n,
                    "a {n}-pixel picture is under twice the floor and must be left alone"
                );
            }
            for n in [PREVIEW_EDGE * 2, PREVIEW_EDGE * 3 + 7] {
                let got = probe(n);
                assert!(
                    (PREVIEW_EDGE..n).contains(&got),
                    "a {n}-pixel picture reduced to {got}, outside [{PREVIEW_EDGE}, {n})"
                );
            }
        }

        /// **The tables are a memoization of core's function, not a second copy of
        /// it**, and this is what keeps that true.
        ///
        /// `AdjustLut` exists because one `AdjustPipeline::apply` per pixel costs
        /// about 11 ms on an 800×500 photograph and a scrub rebuilds the buffer
        /// every frame. The danger in any such table is that it quietly becomes a
        /// *different* function — and the result would still look like a
        /// photograph, so nothing else here would catch it. The whole point of the
        /// pipeline's shape is that the canvas, the CPU export and the SVG filter
        /// say the same thing (§15 D186); a table that drifted would break that on
        /// two of the three and leave the markup right.
        ///
        /// **One level of tolerance, and the reason is stated rather than tuned
        /// to.** The tables round twice where `apply` rounds once — the matrix
        /// result is quantized to a byte before the tone curve reads it, which is
        /// what an 8-bit filter implementation does anyway — and the curve's slope
        /// is under 2, so half a level in cannot be more than one level out.
        #[test]
        fn the_table_agrees_with_the_function_it_compiles() {
            let sliders = [
                ImageAdjust {
                    exposure: 0.8,
                    ..ImageAdjust::NEUTRAL
                },
                ImageAdjust {
                    contrast: -0.6,
                    ..ImageAdjust::NEUTRAL
                },
                ImageAdjust {
                    saturation: 1.0,
                    ..ImageAdjust::NEUTRAL
                },
                ImageAdjust {
                    temperature: 0.9,
                    tint: -0.7,
                    ..ImageAdjust::NEUTRAL
                },
                ImageAdjust {
                    highlights: -1.0,
                    ..ImageAdjust::NEUTRAL
                },
                ImageAdjust {
                    shadows: 1.0,
                    ..ImageAdjust::NEUTRAL
                },
                // Everything at once, and hard enough that the matrix overshoots
                // white — the case where the two spellings' clamps have to agree.
                ImageAdjust {
                    exposure: 1.0,
                    contrast: 0.7,
                    saturation: -0.5,
                    temperature: -0.8,
                    tint: 0.6,
                    highlights: -0.9,
                    shadows: 0.4,
                },
            ];
            let mut worst = 0i32;
            for adj in sliders {
                let p = adj.pipeline().expect("a moved slider");
                let lut = AdjustLut::build(&p);
                // Coarse enough to run fast and fine enough to hit both ends,
                // every corner of the colour cube, and the middle.
                for r in (0..=255u32).step_by(17) {
                    for g in (0..=255u32).step_by(17) {
                        for b in (0..=255u32).step_by(17) {
                            let rgb = [r as u8, g as u8, b as u8];
                            let want = p
                                .apply(rgb.map(|c| c as f32 / 255.0))
                                .map(|c| (c * 255.0 + 0.5) as u8);
                            let got = lut.apply(rgb);
                            for c in 0..3 {
                                let d = (want[c] as i32 - got[c] as i32).abs();
                                worst = worst.max(d);
                                assert!(
                                    d <= 1,
                                    "{adj:?} on {rgb:?}: the function says {want:?}, \
                                     the table says {got:?}"
                                );
                            }
                        }
                    }
                }
            }
            assert!(worst <= 1, "worst channel difference was {worst}");
        }

        /// **A reduction maps back onto the source's box exactly**, which is the one
        /// thing that keeps a preview from being visibly the wrong size.
        ///
        /// `ImageRef::framing` maps *source*-pixel space onto the frame, so the
        /// correction has to carry the reduced grid all the way onto the source's box
        /// — corner to corner. Asserted at sizes the factor does **not** divide
        /// (2081 over a factor of 2, 3079 over 3, both leaving a short last row and
        /// column), because a correction built from the factor rather than from the
        /// actual dimensions is right for every round number and wrong for those.
        #[test]
        fn a_reduced_picture_maps_onto_the_source_box_corner_to_corner() {
            for n in [PREVIEW_EDGE * 2 + 33, PREVIEW_EDGE * 3 + 7] {
                let store = store_with(n, true);
                let px = store
                    .pixels(&reference("a", |a| a.exposure = 0.5))
                    .expect("held");
                // The fixture has to be in the state this is about: a size that is
                // *not* reduced makes every assertion below trivially true.
                assert!(px.data.width < n, "{n} was not reduced at all");
                let far = px.to_source
                    * ondin_core::kurbo::Point::new(px.data.width as f64, px.data.height as f64);
                assert!(
                    (far.x - n as f64).abs() < 1e-9 && (far.y - n as f64).abs() < 1e-9,
                    "a {n}-wide picture reduced to {} maps its far corner to {far:?}",
                    px.data.width
                );
                assert_eq!(
                    px.to_source * ondin_core::kurbo::Point::ZERO,
                    ondin_core::kurbo::Point::ZERO,
                    "and its near corner to the origin"
                );
            }
        }

        /// **A picture already smaller than the preview edge is its own preview**, so
        /// there is nothing to promote and no repaint to ask for.
        #[test]
        fn a_small_picture_is_answered_at_full_resolution_even_interactively() {
            let store = store_with(8, true);
            let px = store
                .pixels(&reference("a", |a| a.exposure = 0.5))
                .expect("held");
            assert_eq!((px.data.width, px.data.height), (8, 8));
            assert!(
                !store.wants_frame(),
                "nothing was reduced, so no frame is owed"
            );
        }

        /// **A settled adjustment is promoted and a moving one is not** — the whole
        /// of what makes a scrub cheap.
        ///
        /// The two halves are one test on purpose, because the failure modes are
        /// mirror images and a version that gets one right gets the other wrong.
        /// Promote too eagerly (on one frame of stillness) and every value a slider
        /// passes through costs a full-resolution pass, one frame late and thrown
        /// away — the exact defect the preview exists to prevent, reintroduced.
        /// Promote never, and the picture the user is left looking at is the soft one.
        ///
        /// Driven the way the canvas drives it: `prepare` at the top of a frame, one
        /// `pixels` during the walk.
        ///
        /// **The drag half is asserted on `derived_full_len`, not on the width it
        /// hands back** — which is the whole difficulty, and the version of this
        /// test that checked the width passed against a one-frame rule. A fresh
        /// key is always answered with a preview whatever the rule is; what the
        /// eager rule does wrong is build a full-resolution buffer for the
        /// *previous* frame's value, which nothing on screen ever shows and only
        /// the count can see.
        #[test]
        fn a_settled_adjustment_reaches_full_resolution_and_a_moving_one_does_not() {
            let n = PREVIEW_EDGE * 2;
            let mut store = store_with(n, true);
            let doc = doc_with(&[], 8);

            // A slider being dragged: a different value every frame.
            for i in 0..8 {
                store.prepare(&doc);
                let px = store
                    .pixels(&reference("a", |a| a.exposure = 0.1 * (i + 1) as f32))
                    .expect("held");
                assert!(
                    px.data.width < n,
                    "frame {i} of a drag was answered at full resolution"
                );
                assert_eq!(
                    store.derived_full_len(),
                    0,
                    "frame {i} of a drag built a full-resolution buffer nothing will draw"
                );
                assert!(
                    store.wants_frame(),
                    "a preview is showing, so a frame is owed"
                );
            }

            // Released: the same value every frame from here on.
            let settled = reference("a", |a| a.exposure = 0.95);
            let mut widths = Vec::new();
            for _ in 0..4 {
                store.prepare(&doc);
                widths.push(store.pixels(&settled).expect("held").data.width);
            }
            assert!(
                widths.last() == Some(&n),
                "a slider that stopped never reached full resolution: {widths:?}"
            );
            assert!(
                widths.iter().filter(|w| **w == n).count() <= 3,
                "it was promoted before it had stood still: {widths:?}"
            );
            assert!(
                !store.wants_frame(),
                "and once it is the picture, no more frames are owed"
            );
        }

        /// **A scrolled-past preview is dropped and a real answer is kept.**
        ///
        /// The two kinds of derived buffer are worth exactly different amounts: a
        /// preview is one position of a slider nobody will return to, where a
        /// full-resolution buffer is the picture on screen. Sweeping both on
        /// staleness would recompute the visible one every time it left the viewport;
        /// sweeping neither would leave every value of every drag in memory.
        #[test]
        fn a_stale_preview_is_dropped_where_a_stale_full_answer_is_not() {
            let n = PREVIEW_EDGE * 2;
            let mut store = store_with(n, true);
            let doc = doc_with(&[], 8);

            // One adjustment held still long enough to be promoted...
            let kept = reference("a", |a| a.exposure = 0.5);
            for _ in 0..4 {
                store.prepare(&doc);
                store.pixels(&kept);
            }
            assert_eq!(store.derived_len(), 1);

            // ...then a run of values nobody comes back to.
            for i in 0..5 {
                store.prepare(&doc);
                store.pixels(&reference("a", |a| a.contrast = 0.1 * (i + 1) as f32));
            }
            // Several frames with nothing asking, so every preview has gone stale.
            for _ in 0..4 {
                store.prepare(&doc);
            }
            assert_eq!(
                store.derived_len(),
                1,
                "the previews outlived the drag that made them"
            );
            assert_eq!(
                store.pixels(&kept).expect("held").data.width,
                n,
                "and the settled answer survived rather than being swept with them"
            );
        }

        /// **Derived pixels go when their picture does.**
        ///
        /// A buffer keyed by an id the store can no longer decode is unreachable —
        /// `pixels` returns `None` before it ever looks at the derived map — so
        /// keeping it is pure waste, and the waste is a whole photograph.
        #[test]
        fn dropping_a_picture_drops_what_was_derived_from_it() {
            let one = 8 * 8 * 4;
            let mut store = ImageStore::with_budget(one);
            store.prepare(&doc_with(&["a"], 8));
            store
                .pixels(&reference("a", |adj| adj.exposure = 0.5))
                .expect("held");
            assert_eq!(store.derived_len(), 1);

            // "a" leaves the document and is pushed out by two that do not fit
            // beside it.
            store.prepare(&doc_with(&["b"], 8));
            store.prepare(&doc_with(&["b"], 8));
            assert!(store.get(&id("a")).is_none(), "the fixture evicted it");
            assert_eq!(
                store.derived_len(),
                0,
                "its adjusted buffer is unreachable and must go with it"
            );
        }

        /// 🚨 **An export-shaped run stays under the derived budget** (§15 D596,
        /// `[S9.2-L4-03]`).
        ///
        /// `evict_derived` was the only place the budget was checked and `prepare`
        /// is its only caller — and every non-interactive caller runs `prepare`
        /// at most **once, before the first read**. `png::png` and `png::raster_of` both
        /// open with `let mut images = ImageStore::new(); images.prepare(doc);` and
        /// then hand the store to the renderer, which asks for `pixels` per
        /// distinct fill for the whole render. So the cap was never consulted on
        /// the export or library-cover paths, and the store held one full-size
        /// buffer per distinct `(ImageId, AdjustKey)` the walk visited. Measured in
        /// release on a 4000×3000 picture: 192 MB at four adjustments, **384 MB at
        /// eight, against a 256 MB budget**, and unbounded above.
        ///
        /// ⚠️ **Two defects, and either alone leaves this red.** Moving the check
        /// into `pixels` is not enough while the candidate list filters to
        /// `!live(d.asked)`: one `prepare` sets `clock` to 1, every entry a
        /// one-shot walk creates is asked at 1, and `live(1)` is true for all of
        /// them — so the loop has nothing it is willing to evict. **A cap that
        /// cannot evict anything is not a cap.**
        ///
        /// ⚠️ **The fixture assertion is inside the loop**, and it is not
        /// decoration: if a single buffer already exceeded the budget,
        /// `derived_len` would sit at 1 at the end for reasons that have nothing
        /// to do with eviction. It measures a buffer the store really built —
        /// the first draft compared the budget against itself, which is
        /// arithmetic and cannot fail.
        ///
        /// ⚠️ **No `prepare` here, deliberately**: `store_with` inserts, so the
        /// clock stands at 0 and `live(0)` is true of everything, which is the
        /// export's own condition and the one the predicate half is about.
        ///
        /// **Both flips run, separately, and both fail here at 6 against 3.**
        /// (1) `trim_derived`'s call removed from `pixels`; (2) the call kept but
        /// the candidate list filtered back to `!live(d.asked)`. ⚠️ **Flip (2)
        /// leaves `a_stale_derived_buffer_goes_before_a_live_one` green**, which
        /// is the pair that says the *time* of the check and the *predicate* are
        /// two independent defects rather than one described twice — and flip (1)
        /// takes that control down with it at 4 against 3, because without the
        /// insertion-time call nothing bounds anything.
        ///
        /// (Plain backticks, §15 D319 — `cargo doc` builds without the `test` cfg.)
        #[test]
        fn an_export_shaped_run_does_not_exceed_the_derived_budget() {
            let mut store = store_with(16, false);
            store.set_derived_budget(16 * 16 * 4 * 3);

            // The export's own shape: the clock stands still and six distinct
            // adjustments are read once each.
            for i in 0..6 {
                let r = reference("a", |a| a.exposure = 0.1 * (i as f32 + 1.0));
                assert!(store.pixels(&r).is_some(), "each read must answer");
                if i == 0 {
                    // ⚠️ **The fixture, measured rather than asserted over two
                    // constants.** The first draft was `assert!(one <= one * 3)`,
                    // which is arithmetic on the budget and cannot fail; what has
                    // to be true is that a buffer this store actually builds fits
                    // in it, or `derived_len` would sit at 1 below for reasons
                    // that have nothing to do with eviction.
                    assert_eq!(
                        store.derived_len(),
                        1,
                        "the fixture: one buffer must fit the budget"
                    );
                }
            }

            assert_eq!(
                store.derived_len(),
                3,
                "the export path held every buffer it built"
            );
        }

        /// **The control: a *stale* buffer goes before a live one.**
        ///
        /// `trim_derived` may evict a live entry, and that is the change — but only
        /// as a last resort. Given a choice it takes the stale one, which is
        /// `evict_derived`'s own rule and the reason §15 D160 gives: a buffer the
        /// walk is about to ask for again costs a recomputation to drop.
        ///
        /// The clock is advanced with `prepare` so the first two entries go stale
        /// while a third stays live; a fourth insertion then pushes the store past
        /// a budget of three, and which one survives says which rule won.
        ///
        /// 🚨 **The survivors are read with `holds_derived`, and the first draft
        /// of this test could not fail.** It asked `pixels` and then the count —
        /// but `trim_derived` runs on every insertion, so a *rebuilt* buffer puts
        /// the count straight back to three and a reversed preference was green.
        /// `arch-scribe` caught it reading the test against what it can observe.
        /// **A test about which entry survived needs an observation that does not
        /// disturb what it observes.** Flip run with the sort key inverted
        /// (`>=` to `<`, i.e. live evicted first): fails on *"the live buffer was
        /// evicted while a stale one was available"*, which the count-based
        /// version passed.
        #[test]
        fn a_stale_derived_buffer_goes_before_a_live_one() {
            let doc = doc_with(&["a"], 16);
            let mut store = ImageStore::new();
            store.prepare(&doc);
            let one = 16 * 16 * 4;
            store.set_derived_budget(one * 3);

            let at = |e: f32| reference("a", move |a| a.exposure = e);
            store.pixels(&at(0.1));
            store.pixels(&at(0.2));
            // Three `prepare`s put the clock past `SETTLED_FRAMES`, so the two
            // above are stale by the time anything else is asked for.
            for _ in 0..3 {
                store.prepare(&doc);
            }
            store.pixels(&at(0.3));
            assert_eq!(
                store.derived_len(),
                3,
                "the fixture: three buffers, two of them stale, and under budget"
            );

            store.pixels(&at(0.4));
            assert_eq!(store.derived_len(), 3, "the budget still holds");

            // 🚨 **Asked of `holds_derived`, not of `pixels`.** The first version
            // of this test read the survivor back through `pixels` and asserted
            // the count — which cannot fail: `trim_derived` runs on every
            // insertion, so a *rebuilt* buffer restores the same three and a
            // reversed preference is green. `arch-scribe` caught it; the
            // observation that does not disturb what it observes is the fix.
            assert!(
                store.holds_derived(&at(0.3)),
                "the live buffer was evicted while a stale one was available"
            );
            assert!(
                store.holds_derived(&at(0.4)),
                "and the one just built must never be a candidate"
            );
            assert!(
                !store.holds_derived(&at(0.1)),
                "the oldest stale buffer is the one that should have gone"
            );
        }
    }

    mod thumb_tests {
        use super::*;

        /// A `w`×`h` PNG with the pixels given, RGBA and row-major.
        fn png_rgba(w: u32, h: u32, px: &[u8]) -> Vec<u8> {
            assert_eq!(
                px.len(),
                (w * h * 4) as usize,
                "the fixture is the wrong size"
            );
            let mut out = Vec::new();
            {
                let mut enc = png::Encoder::new(&mut out, w, h);
                enc.set_color(png::ColorType::Rgba);
                enc.set_depth(png::BitDepth::Eight);
                let mut writer = enc.write_header().unwrap();
                writer.write_image_data(px).unwrap();
            }
            out
        }

        /// The four opaque pixels of a thumbnail, as `(r, g, b)` triples in
        /// reading order.
        fn quads(data: &ImageData) -> Vec<(u8, u8, u8)> {
            data.data
                .data()
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| (p[0], p[1], p[2]))
                .collect()
        }

        const RED: [u8; 4] = [255, 0, 0, 255];
        const GREEN: [u8; 4] = [0, 255, 0, 255];
        const BLUE: [u8; 4] = [0, 0, 255, 255];
        const WHITE: [u8; 4] = [255, 255, 255, 255];

        /// A store holding a 2×2 under `"a"`: red green over blue white. Four
        /// distinguishable pixels, which is what an orientation can be read off
        /// — a symmetric fixture would pass under every one of the eight.
        fn store_2x2() -> ImageStore {
            let mut px = Vec::new();
            for c in [RED, GREEN, BLUE, WHITE] {
                px.extend(c);
            }
            let mut store = ImageStore::interactive();
            store
                .insert(&id("a"), &png_rgba(2, 2, &px), ImageFormat::Png)
                .expect("the fixture decodes");
            store
        }

        /// **A quarter turn puts the picture's top-left at the thumbnail's
        /// top-right**, and the fixture is asymmetric so that it can say so.
        ///
        /// This is the test `un_orient`'s doc comment is written for. The
        /// forward map for one quarter turn *is* the inverse map for three, so
        /// the plausible wrong version — writing the composition down rather
        /// than solving it — turns every picture the wrong way and produces a
        /// thumbnail that is equally plausible on screen. Flipped against that
        /// version this fails with `green` where `blue` is expected, which is
        /// exactly the two the wrong turn swaps.
        #[test]
        fn a_quarter_turn_turns_the_thumbnail_the_way_the_canvas_turns_the_picture() {
            let store = store_2x2();
            let mut r = ImageRef::new(id("a"));
            // The fixture is 2×2 and `THUMB_BASE` is 128, so `reduced` declines
            // it and the thumbnail is cut from the decode itself — one source
            // pixel per output pixel, and the assertion is about the map alone.
            assert_eq!(
                quads(&store.thumbnail(&r, 2).expect("held")),
                vec![(255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 255)],
                "unturned, a thumbnail is the picture"
            );

            r.orient.quarters = 1;
            assert_eq!(
                quads(&store.thumbnail(&r, 2).expect("held")),
                vec![(0, 0, 255), (255, 0, 0), (255, 255, 255), (0, 255, 0)],
                "one quarter clockwise: the left column becomes the top row"
            );

            r.orient.quarters = 3;
            assert_eq!(
                quads(&store.thumbnail(&r, 2).expect("held")),
                vec![(0, 255, 0), (255, 255, 255), (255, 0, 0), (0, 0, 255)],
                "three quarters is the other way, and must not equal one"
            );

            r.orient = Default::default();
            r.orient.mirrored = true;
            assert_eq!(
                quads(&store.thumbnail(&r, 2).expect("held")),
                vec![(0, 255, 0), (255, 0, 0), (255, 255, 255), (0, 0, 255)],
                "mirrored swaps the columns and leaves the rows alone"
            );
        }

        /// **A wide picture is centre-cropped, not fitted**, which is the whole
        /// of what the square is for.
        ///
        /// Three pixels across and one down, so the centre square is one pixel
        /// and it is the middle one. The version that would also be written —
        /// fit the picture into the square and let the ground show — averages
        /// all three and answers a muddy grey, and the version that crops from
        /// the *origin* rather than the centre answers red. Only the right one
        /// answers green, and each wrong answer names which mistake it was.
        #[test]
        fn a_wide_picture_is_cropped_to_its_middle_rather_than_squeezed_into_the_square() {
            let mut px = Vec::new();
            for c in [RED, GREEN, BLUE] {
                px.extend(c);
            }
            let mut store = ImageStore::new();
            store
                .insert(&id("wide"), &png_rgba(3, 1, &px), ImageFormat::Png)
                .expect("the fixture decodes");

            let thumb = store
                .thumbnail(&ImageRef::new(id("wide")), 1)
                .expect("held");
            assert_eq!(thumb.width, 1, "a thumbnail is square whatever it holds");
            assert_eq!(thumb.height, 1);
            assert_eq!(
                quads(&thumb),
                vec![(0, 255, 0)],
                "the centre pixel: red is a crop from the origin, and a muddy \
                 grey is a fit"
            );
        }

        /// **The crop is read, and it is read in the *oriented* picture's
        /// space** — the invariant §5.5a states and the crop gesture depends on.
        ///
        /// The fixture is asked twice with one rectangle and two orientations,
        /// and the two answers must differ: a version that un-oriented the
        /// rectangle first, or that ignored the orientation while cropping,
        /// gives the same pixel both times.
        #[test]
        fn the_crop_rectangle_is_normalized_to_the_oriented_picture() {
            let store = store_2x2();
            // The left half of whatever is on screen.
            let left = ondin_core::kurbo::Rect::new(0.0, 0.0, 0.5, 1.0);

            let mut upright = ImageRef::new(id("a"));
            upright.crop = left;
            assert_eq!(
                quads(&store.thumbnail(&upright, 1).expect("held")),
                // Half a pixel wide, so the block rounds out to the whole left
                // column and averages red over blue.
                vec![(128, 0, 128)],
                "upright, the left half is the red/blue column"
            );

            let mut turned = ImageRef::new(id("a"));
            turned.crop = left;
            turned.orient.quarters = 1;
            assert_eq!(
                quads(&store.thumbnail(&turned, 1).expect("held")),
                // Turned, the left column of what is shown is blue over white.
                vec![(128, 128, 255)],
                "the same rectangle over a turned picture must select different \
                 pixels, or it is being read in the source's space"
            );
        }

        /// **A thumbnail must not enter the promotion machinery**, which is the
        /// reason `ImageStore::thumbnail` exists at all rather than the panel
        /// calling `ImageStore::pixels` with a small number.
        ///
        /// `pixels` notes an ask, and an ask repeated on two consecutive frames
        /// builds the *full-resolution* adjusted buffer. A twenty-pixel chip
        /// driving that would spend a full pass on a picture nothing is drawing
        /// at size, and would hold `wants_frame` true so the app kept
        /// repainting for a row.
        ///
        /// **The `pixels` call at the end is not a coda, it is what makes the
        /// assertions mean anything.** Without it every one of them would pass
        /// against a store that cached nothing at all, or against a
        /// `derived_len` that always answered zero.
        ///
        /// **And the fixture has to be over `2 · PREVIEW_EDGE`**, which is the
        /// only reason it is 2100 pixels of test data. Under that, `reduced`
        /// declines, every answer is already full resolution, and `wants_frame`
        /// is false whatever either function does — so the interesting half of
        /// this test passed against a small picture by meaning nothing.
        #[test]
        fn asking_for_a_thumbnail_leaves_the_adjustment_cache_alone() {
            let mut store = ImageStore::interactive();
            store
                .insert(&id("a"), &png_square(2100), ImageFormat::Png)
                .expect("the fixture decodes");
            let mut r = ImageRef::new(id("a"));
            r.adjust.exposure = 0.5;

            // Twice, because "twice" is precisely what promotion is triggered
            // by — one ask could never promote whatever the implementation did.
            for _ in 0..2 {
                store.thumbnail(&r, 8).expect("held");
            }
            assert_eq!(
                store.derived_len(),
                0,
                "a thumbnail must leave no adjusted buffer behind"
            );
            assert!(
                !store.wants_frame(),
                "and must not make the app owe itself a repaint"
            );

            // The contrast: the same reference through the canvas's entry point
            // does all of that, so the assertions above are about `thumbnail`
            // and not about a store that never caches.
            store.pixels(&r).expect("held");
            assert_eq!(store.derived_len(), 1);
            assert!(store.wants_frame());
        }

        /// **The neutral adjustment is one key, not two.**
        ///
        /// `-0.0` and `0.0` are equal numbers with different bits, and the
        /// adjustment is neutral under both — so keying on the raw bits gives
        /// one drawing two textures, and the second never hits. The corner
        /// `AdjustKey` is documented as being safe from *because the neutral
        /// value never reaches it*; here it does.
        #[test]
        fn a_neutral_adjustment_keys_the_same_however_its_zeroes_are_signed() {
            let plain = ImageRef::new(id("a"));
            let mut negative = ImageRef::new(id("a"));
            negative.adjust.exposure = -0.0;
            assert_ne!(
                negative.adjust.exposure.to_bits(),
                plain.adjust.exposure.to_bits(),
                "the fixture is in the state this test is about"
            );
            assert_eq!(ThumbKey::of(&plain, 16), ThumbKey::of(&negative, 16));

            // And the things that *do* change the drawing change the key.
            let mut adjusted = ImageRef::new(id("a"));
            adjusted.adjust.exposure = 0.5;
            assert_ne!(ThumbKey::of(&plain, 16), ThumbKey::of(&adjusted, 16));
            let mut cropped = ImageRef::new(id("a"));
            cropped.crop = ondin_core::kurbo::Rect::new(0.0, 0.0, 0.5, 1.0);
            assert_ne!(ThumbKey::of(&plain, 16), ThumbKey::of(&cropped, 16));
            let mut turned = ImageRef::new(id("a"));
            turned.orient.quarters = 1;
            assert_ne!(ThumbKey::of(&plain, 16), ThumbKey::of(&turned, 16));
            assert_ne!(ThumbKey::of(&plain, 16), ThumbKey::of(&plain, 32));

            // The framing mode is deliberately *not* in it: a thumbnail has no
            // box for Fill and Fit to disagree about.
            let mut fitted = ImageRef::new(id("a"));
            fitted.fit = ondin_core::ImageFit::Fit;
            assert_eq!(ThumbKey::of(&plain, 16), ThumbKey::of(&fitted, 16));
        }

        /// **The reduced base is cut once per picture and shared**, which is
        /// what keeps a scrub from putting a full-size box filter in the layers
        /// panel every frame.
        #[test]
        fn every_thumbnail_of_one_picture_shares_one_reduced_base() {
            let mut store = ImageStore::interactive();
            store
                .insert(&id("a"), &png_square(512), ImageFormat::Png)
                .expect("the fixture decodes");
            let mut r = ImageRef::new(id("a"));
            for step in 0..5i16 {
                r.adjust.exposure = 0.1 * f32::from(step);
                store.thumbnail(&r, 16).expect("held");
            }
            let cache = store.derived.lock().unwrap();
            assert_eq!(cache.thumb_base.len(), 1, "one base for five thumbnails");
            let base = &cache.thumb_base[&id("a")];
            assert!(
                base.width <= 2 * THUMB_BASE && base.width >= THUMB_BASE,
                "reduced by a whole factor towards the floor, not past it: {}",
                base.width
            );
        }

        /// **The reduced base goes when its picture does**, rather than when the
        /// last adjustment of it does.
        ///
        /// That distinction is the whole of the retain line it pins, and getting
        /// it wrong is invisible in the other direction: a thumbnail leaves *no*
        /// derived entry behind by design, so the rule the `small` buffer uses —
        /// keep it while some adjustment of this picture is cached — would throw
        /// this away on the very next `prepare` and cut it again the frame after,
        /// for ever, which is a full-size box filter every frame and looks
        /// exactly like working code.
        #[test]
        fn a_thumbnail_base_goes_when_its_picture_does_and_not_before() {
            let one = 8 * 8 * 4;
            let mut store = ImageStore::with_budget(one);
            store.prepare(&doc_with(&["a"], 8));
            store.thumbnail(&ImageRef::new(id("a")), 4).expect("held");
            assert_eq!(store.derived.lock().unwrap().thumb_base.len(), 1);

            // Surviving a `prepare` is the half that fails against the `small`
            // rule, and the picture is still in the document here.
            store.prepare(&doc_with(&["a"], 8));
            assert_eq!(
                store.derived.lock().unwrap().thumb_base.len(),
                1,
                "the base was thrown away while its picture was still on screen"
            );

            // "a" leaves the document and is pushed out by one that does not fit
            // beside it.
            store.prepare(&doc_with(&["b"], 8));
            store.prepare(&doc_with(&["b"], 8));
            assert!(store.get(&id("a")).is_none(), "the fixture evicted it");
            assert!(
                store.derived.lock().unwrap().thumb_base.is_empty(),
                "the base outlived the picture it was cut from"
            );
        }
    }
}
