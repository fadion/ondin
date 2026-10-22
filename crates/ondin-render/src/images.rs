//! The decode cache: encoded bytes in, `peniko::ImageData` out, once per image.
//!
//! It lives here because the canvas and `ondin-export` both need decoded pixels
//! and core does no decoding (§3). It stores the shared `peniko::ImageData`; each
//! backend converts at the boundary (`cpu.rs` memoizes the CPU's per render). The
//! budget bounds bytes, not a count.
//!
//! It also holds pixels *derived* from a decode, since an [`ImageAdjust`] is a
//! per-pixel function: keyed by `(id, adjustment)`, under its own budget, filled
//! where *read* because only the scene walk knows the fills. Hence the lock, and
//! [`ImageStore::pixels`], not [`ImageStore::get`], is what a backend paints with.
//!
//! It does not bound a document: eviction drops only images the document no
//! longer carries, or a referenced one would be decoded and evicted every frame.
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
/// It bounds *stale* decodes (see the module docs), a ceiling on waste rather than
/// on the working set.
pub const DECODED_BYTES: usize = 256 * 1024 * 1024;

/// The largest edge either backend will accept.
///
/// A guard against a panic: `vello_cpu` builds its `Pixmap` from `u16` dimensions
/// and asserts on this bound, so it is refused at the decode.
pub const MAX_EDGE: u32 = u16::MAX as u32;

/// vello's image atlas at its largest, in pixels a side.
///
/// Upstream's private `MAX_ATLAS_SIZE` in `vello_encoding-0.9.0`, so a vello
/// upgrade can silently invalidate it (as [`crate::cpu::MAX_RASTER_SIDE`]). A
/// picture that does not fit is silently not drawn, so the canvas must arrange in
/// advance not to lose one (§15 D745).
pub const ATLAS_EDGE: u32 = 8192;

/// The most a picture is shrunk before the canvas gives up on fitting it.
///
/// Past it [`atlas_downscale`] stops looking and the drop happens after all.
pub const MAX_ATLAS_DOWNSCALE: u32 = 64;

/// Whether every one of `sizes` fits an atlas `side` pixels square.
///
/// A *shelf* packing, where vello uses a guillotine allocator. Shelf packing is
/// strictly worse, so "the shelf fits" implies "the guillotine fits". Tallest
/// first, or a short picture could open a shelf a tall one has to leave.
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
/// Searches upward from 1, so a page that fits is untouched. One factor for all,
/// since per-picture factors disagree about sharpness and depend on placement
/// order. Integer, because [`reduced_by`] box-averages whole blocks.
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
/// No other cap is in decoded RGBA bytes of a live image: `image`'s `max_alloc`
/// bounds the decoder's own buffer (`into_rgba8` quadruples grayscale, and its
/// allocation failure aborts), [`MAX_EDGE`] counts edge pixels, and
/// [`DECODED_BYTES`] weighs only stale entries. It cannot go below a 50 MP camera
/// file (240 MB of RGBA), so it bounds the unbounded tail, not an 8000×8000.
pub const MAX_DECODED_BYTES: u64 = DECODED_BYTES as u64;

/// Why an image could not be decoded, for the caller that wants to say so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The bytes are not a readable image of the format the entry claims.
    Corrupt,
    /// Wider or taller than [`MAX_EDGE`], which the CPU backend cannot hold.
    TooLarge { width: u32, height: u32 },
    /// Small enough on both edges and still too much RGBA — see
    /// [`MAX_DECODED_BYTES`]. Separate from [`Self::TooLarge`] so a caller can say
    /// which: that is a bound the renderer asserts on, this is memory.
    TooMuchPixelData { width: u32, height: u32, bytes: u64 },
}

/// Adjusted pixels held before the least recently asked-for are dropped.
///
/// A separate ceiling from [`DECODED_BYTES`]: every one of these is recomputable
/// from a decode, so this is a working-set bound and eviction costs a recompute.
pub const ADJUSTED_BYTES: usize = 256 * 1024 * 1024;

/// The shortest a scrub preview's long edge is allowed to get — a **floor**, not
/// a ceiling (see [`reduced`]).
///
/// The reduction is by a whole factor, so a preview's long edge lands between this
/// and twice it, and a picture under twice this is not reduced at all.
pub const PREVIEW_EDGE: u32 = 1024;

/// The long edge a picture is reduced to **once**, before any thumbnail is cut
/// out of it.
///
/// [`reduced`] reads every source pixel, so doing it per thumbnail would run a
/// full box filter each frame of a scrub (a new [`ThumbKey`] per frame). 128
/// leaves headroom over a row's twenty points at 300% scaling and centre-cropped
/// 16:9. At 64 KB a picture it rides on the decode cache's eviction.
pub const THUMB_BASE: u32 = 128;

/// How many consecutive frames an adjustment has to stand still before its
/// full-resolution buffer is built.
///
/// Two, not one: the same adjustment asked twice running is what a released
/// slider looks like; one frame would spend a full pass on every scrubbed value.
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
/// [`ondin_core::ImageRef::framing`] maps *source-pixel space* onto the frame, so a
/// reduced buffer needs `to_source` composed under it; the identity for every
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
    /// Kept so a promotion can redo the work at full resolution without
    /// rebuilding an [`ImageAdjust`] out of the key's bit patterns.
    pipeline: AdjustPipeline,
    to_source: Affine,
    /// Whether `data` is the picture at its own size rather than a preview.
    full: bool,
    bytes: usize,
    /// The clock of the most recent frame that asked for this, and of the frame
    /// that asked before it (see [`SETTLED_FRAMES`]).
    ///
    /// `None` rather than zero for "no frame before": a sentinel 0 collides with
    /// clock 0.
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
/// Bitwise comparison is safe because the neutral value never reaches this:
/// [`ImageAdjust::pipeline`] answers `None` for it, resolving `-0.0` vs `0.0`.
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
/// Every field is one the drawing changes with. The framing *mode* is absent
/// because a thumbnail has no box but its own square. Opaque, so [`Self::of`] owns
/// normalizing `-0.0`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ThumbKey {
    id: ImageId,
    adjust: AdjustKey,
    /// `ImageOrient` is not `Hash`; these are the two fields read, through
    /// `quarters()` so a hand-written `4` keys as the `0` it draws as.
    orient: (u8, bool),
    crop: [u64; 4],
    /// Device pixels, not points, so display scale changes the key and the
    /// caller's stale sweep collects the old size.
    edge: u32,
}

impl ThumbKey {
    /// The key `r` draws under at `edge` device pixels.
    pub fn of(r: &ImageRef, edge: u32) -> Self {
        Self {
            id: r.id.clone(),
            // The neutral adjustment collapses to one key, not to its own bits:
            // `-0.0` and `0.0` differ in bits, so keying on them would give one
            // drawing two textures that never hit.
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
    /// One reduced copy per picture, shared by every adjustment of it.
    small: FxHashMap<ImageId, (ImageData, Affine)>,
    /// One picture reduced to [`THUMB_BASE`], shared by every thumbnail of it.
    /// Keyed by picture alone, so a scrub's new [`ThumbKey`] each frame never
    /// recomputes it.
    thumb_base: FxHashMap<ImageId, ImageData>,
    /// One picture reduced to the atlas budget's factor (§15 D745).
    ///
    /// Keyed by the *blob's* id, not the picture's: the buffer
    /// [`ImageStore::pixels`] returns may be the decode, a preview or an adjusted
    /// copy (vello keys its atlas the same way). [`ImageStore::prepare`] drops
    /// entries at any factor but the current one.
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
    /// Behind a lock because it is filled where it is read, by a scene walk that
    /// holds the store immutably.
    derived: Mutex<Adjusted>,
    budget: usize,
    derived_budget: usize,
    /// Whether a miss may be answered with a reduced picture while the value is
    /// still moving.
    ///
    /// Off by default, so an export never silently writes a downsampled
    /// photograph; only [`Self::interactive`] opts in.
    previews: bool,
    clock: u64,
    /// The integer factor every picture is shrunk by before the GPU sees it, so
    /// the page's pictures fit vello's atlas (§15 D745).
    ///
    /// Recomputed by [`Self::prepare`] from the document's table, not from what a
    /// frame draws, or a photograph would resharpen as its neighbour scrolled off.
    /// `0` from `Default`, read as `1` ([`Self::atlas_factor`]).
    atlas_factor: u32,
}

impl ImageStore {
    /// A store that always answers at full resolution, as `ondin export --png`
    /// needs.
    pub fn new() -> Self {
        Self::with_budget(DECODED_BYTES)
    }

    /// [`Self::new`], plus permission to answer a *fresh* adjustment with a
    /// reduced picture until it has stood still for [`SETTLED_FRAMES`] frames.
    ///
    /// For the canvas alone. The caller owes a repaint while [`Self::wants_frame`]
    /// is true, or the last frame of a scrub stays soft.
    pub fn interactive() -> Self {
        Self {
            previews: true,
            ..Self::new()
        }
    }

    /// Whether a derived buffer for `r` is held, without building one as
    /// `Self::pixels` would.
    #[cfg(test)]
    fn holds_derived(&self, r: &ImageRef) -> bool {
        let key = (r.id.clone(), AdjustKey::of(&r.adjust));
        let cache = self.derived.lock().unwrap_or_else(|e| e.into_inner());
        cache.entries.contains_key(&key)
    }

    /// Shrink the derived-buffer budget, for a test that would otherwise have to
    /// build 256 MB of pixels to reach it.
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
    /// Cheap enough to call every frame, as the canvas does; a change signal would
    /// eventually miss one. Linked images are skipped: whoever has the bytes calls
    /// [`Self::insert`], and until then it is a missing image (§15 D179).
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
        // After the eviction, so a photograph the document dropped does not go on
        // shrinking the ones it still carries.
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

    /// The factor [`Self::atlas_pixels`] shrinks by — `1` when everything fits.
    ///
    /// `max(1)` because `Default` leaves the field at zero and an unprepared store
    /// must still answer "no reduction".
    pub fn atlas_factor(&self) -> u32 {
        self.atlas_factor.max(1)
    }

    /// [`Self::pixels`], shrunk to the page's share of vello's image atlas.
    ///
    /// The GPU backend's accessor, where the backends are meant to disagree (§15
    /// D745): vello silently draws nothing when a picture will not fit its atlas,
    /// while the CPU backend and `--svg` keep calling [`Self::pixels`].
    ///
    /// Applied to whatever buffer comes back, not the decode: reducing a preview
    /// again is one step softer until the slider stops. Returns the unreduced
    /// answer when [`reduced_by`] declines: better a picture that may not fit than
    /// none.
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
                // Composed, not replaced: a preview under an adjustment carries both.
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
    /// Runs off the two previous frames' asks (see [`SETTLED_FRAMES`]). The
    /// pipeline comes from the entry, since the previewed value may be an override
    /// the document never saw. One per frame however many are due, as a full pass
    /// over a large photograph is a visible fraction of a second; the pick is
    /// sorted so the order is not a hash order.
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
                    // Asked on the previous frame and the one before it.
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
    /// A **preview** is one position of a slider nobody returns to, so it goes as
    /// soon as it is stale. A **full** buffer cost real work, so it goes only under
    /// the budget (§15 D160). The budget is enforced by [`trim_derived`], which
    /// [`Self::pixels`] also calls: paths that `prepare` once never reach this.
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
        // The thumbnail base goes with the picture, not the last adjustment of it:
        // `thumbnail` makes no derived entry, so the `small` rule would throw the
        // base away every frame.
        cache.thumb_base.retain(|id, _| sources.contains_key(id));
    }

    /// Whether a repaint is owed: some adjustment is still showing a preview and
    /// will be promoted once it has stood still.
    ///
    /// The caller owes this a `request_repaint`: promotion happens in a later
    /// frame's `prepare`, and the releasing frame is often egui's last.
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
    /// This, not [`Self::get`], is what a backend paints with; `get` answers the
    /// decode and is still the question for "can this be drawn at all"
    /// (`ScenePainter::has_image`).
    ///
    /// A neutral adjustment resolves to the decode itself: an `Arc` clone of the
    /// blob, no cache entry.
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
        // picture.
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
            // The geometric test, not `!previews`: a picture smaller than
            // `PREVIEW_EDGE` is its own preview, so there is nothing to promote.
            full: to_source == Affine::IDENTITY,
            data,
            pipeline,
            to_source,
            asked: self.clock,
            asked_before: None,
        };
        let out = derived.pixels();
        cache.entries.insert(key.clone(), derived);
        // Enforced on insertion, not only from `prepare`: export callers `prepare`
        // once before the first read, so a cap checked only there never fires.
        trim_derived(&mut cache, self.derived_budget, self.clock, Some(&key));
        Some(out)
    }

    /// A square thumbnail of what `r` shows, `edge` device pixels on a side, in
    /// the same straight-alpha RGBA8 [`Self::get`] answers in.
    ///
    /// Deliberately not [`Self::pixels`]: that notes an ask, and repeating it
    /// builds the full-resolution buffer ([`SETTLED_FRAMES`]) and holds
    /// [`Self::wants_frame`] true, wasted on a twenty-pixel chip. This touches
    /// neither clock.
    ///
    /// Reduce, then adjust, the opposite order to the canvas; the nonlinear tone
    /// curve makes them differ, invisibly at this size, and the pass runs over
    /// hundreds of pixels instead of millions. Orientation and crop apply; the
    /// framing mode does not, giving a **centre-crop** (see [`square_of`]).
    ///
    /// `None` for a picture the store cannot draw, as [`Self::get`] (§15 D179).
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
        // The lock is released before the sampling, the one part that is not a
        // hash lookup.
        let square = square_of(&small, r, edge)?;
        Some(match r.adjust.pipeline() {
            Some(p) => adjusted_data(&square, &p),
            None => square,
        })
    }

    /// Decode `bytes` under `id` if it is not already held.
    ///
    /// The entry point for bytes that did not come out of the table, such as a
    /// linked image or one being placed. Returns the intrinsic size, so a caller
    /// need not decode twice.
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
    /// Oldest-first rather than largest-first: the biggest stale image is as
    /// likely as any to be the one an undo brings back.
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
    /// `None` is not an error at this seam: both backends answer
    /// `ScenePainter::has_image` with this, so it makes the fill come out as the
    /// grey cross (§5.5a, §15 D179).
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
    /// The number a scrub must not run up: it is the observable difference between
    /// the two-frame rule and a one-frame one, which nothing on screen shows.
    pub fn derived_full_len(&self) -> usize {
        let cache = self.derived.lock().unwrap_or_else(|e| e.into_inner());
        cache.entries.values().filter(|d| d.full).count()
    }
}

/// Whether these bytes carry the signature of one of the four formats v1 accepts.
///
/// Bytes `image` cannot place at all are accepted and left to the hinted decoder.
/// Split out of [`fn@decode`] so it can be asserted directly: the end-to-end BMP
/// test only discriminates under `cargo test --workspace` (§15 D600).
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

/// Encoded bytes to straight-alpha RGBA8.
///
/// The claimed format is a hint, not the authority: `image` sniffs the content.
/// The entry's format still matters for what an SVG data URI and *Export original*
/// write.
///
/// The four accepted formats are decided in code, not by a feature list (§15
/// D600): cargo unifies features, so dependencies such as `arboard` add a decoder
/// per platform and a mislabelled file would draw on one and not another; see
/// [`signature_is_accepted`].
fn decode(bytes: &[u8], format: ImageFormat) -> Result<ImageData, DecodeError> {
    let hinted = match format {
        ImageFormat::Png => image::ImageFormat::Png,
        ImageFormat::Jpeg => image::ImageFormat::Jpeg,
        ImageFormat::Webp => image::ImageFormat::WebP,
        ImageFormat::Gif => image::ImageFormat::Gif,
    };
    // Only a recognised format is refused; unknown signatures go to the hinted
    // decoder.
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
    // Before the expansion: `into_rgba8` is where the memory is spent and where
    // an allocation failure would abort the process (§15 D449).
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
        // Straight alpha (invariant 7); the CPU backend premultiplies on its own.
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    })
}

/// One over an 8-bit channel's full scale, in the fixed point [`AdjustLut`]
/// sums in. 16 bits of fraction keeps every term of the sum inside an `i32`.
const FIXED_ONE: i32 = 1 << 16;

/// What one input channel at one level contributes to each of the three output
/// channels, in [`FIXED_ONE`]ths.
///
/// Indexed by the input channel first: a pixel is then three loads of twelve
/// adjacent bytes instead of nine scattered ones.
type Contributions = Box<[[[i32; 3]; 256]; 3]>;

/// An [`AdjustPipeline`] compiled to lookup tables.
///
/// A memoization of core's function, not a second copy: every number is read out
/// of the pipeline or produced by [`ondin_core::transfer`]. It exists because one
/// `apply` per pixel is too slow for a scrub, which rebuilds the buffer every
/// frame; the tables and the dev profile's `opt-level` are separate fixes.
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
    /// The tone curve reads a *quantized* matrix result, where
    /// [`AdjustPipeline::apply`] passes a float: the one difference, under one level.
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
                // Clamped to `0..=1` before rounding, as `apply` does.
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
/// `keep`.
///
/// Stale before live, then least recently asked first (§15 D160). A live buffer
/// is evictable as a last resort: in a one-shot render every entry is live, and a
/// loop that spared them would free nothing. `keep` is the entry just built and
/// about to be returned, never a candidate.
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
/// The arithmetic is core's ([`AdjustPipeline::apply`], also what the SVG writer
/// emits as a filter), through [`AdjustLut`]. Alpha is copied through and is
/// straight (invariant 7); adjusting premultiplied would tint soft edges.
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
/// `edge` is a floor, not a ceiling: the factor is `long_edge / edge` rounded
/// down, so the result lies between `edge` and `2·edge`. Rounding up would halve a
/// 1200-pixel photograph.
///
/// A box average in premultiplied space, converted back: straight-alpha averaging
/// would let transparent pixels' arbitrary colour darken soft edges. The map back
/// is computed from the actual dimensions, not the factor, since the last row and
/// column of blocks are short when the factor does not divide the size.
fn reduced(src: &ImageData, edge: u32) -> Option<(ImageData, Affine)> {
    let (w, h) = (src.width, src.height);
    if edge == 0 {
        return None;
    }
    // Integer division, so `edge` is a floor: a picture merely over it is left
    // alone. The atlas budget needs a cap, so it calls `reduced_by` directly.
    let f = w.max(h) / edge;
    if f < 2 {
        return None;
    }
    reduced_by(src, f)
}

/// [`reduced`] at a factor the caller has already chosen.
///
/// Split out for the atlas budget (§15 D745), which needs a cap where `reduced`'s
/// `edge` is a floor: `reduced(src, 8192)` answers `None` for 9504 pixels.
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
/// `src` is already reduced to [`THUMB_BASE`], so the box average runs over a few
/// source pixels per output, alpha-weighted as in [`reduced`]. Centre-crop, not
/// fit: a fitted 16:9 photograph is a sliver.
fn square_of(src: &ImageData, r: &ImageRef, edge: u32) -> Option<ImageData> {
    let (bw, bh) = (src.width, src.height);
    let bytes = src.data.data();
    if src.format != PenikoFormat::Rgba8 || bytes.len() < (bw * bh * 4) as usize {
        return None;
    }
    let (q, mirrored) = (r.orient.quarters(), r.orient.mirrored);
    // The picture as the orientation leaves it: an odd number of quarter turns
    // trades the axes, and `crop` is normalized to this, not the source (§5.5a).
    let (ow, oh) = match q % 2 {
        1 => (bh, bw),
        _ => (bw, bh),
    };
    let (fow, foh) = (f64::from(ow), f64::from(oh));
    // Clamped rather than refused: coordinates outside `0..1` sample the edge
    // pixel in core too (`Extend::Pad`).
    let x0 = r.crop.x0.min(r.crop.x1).clamp(0.0, 1.0) * fow;
    let x1 = r.crop.x0.max(r.crop.x1).clamp(0.0, 1.0) * fow;
    let y0 = r.crop.y0.min(r.crop.y1).clamp(0.0, 1.0) * foh;
    let y1 = r.crop.y0.max(r.crop.y1).clamp(0.0, 1.0) * foh;
    // A rectangle with no pixels shows the whole picture: an empty thumbnail would
    // read as a broken image.
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
            // At least one whole source pixel per block: a crop tighter than the
            // thumbnail is a zoom, where `ceil` alone would equal `floor` and the
            // loop below would average nothing.
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
/// The inverse of `ImageOrient`'s composition — mirror first, then `q` quarter
/// turns clockwise. The forward maps are `(mx, my)` to `(bh-1-my, mx)`,
/// `(bw-1-mx, bh-1-my)` and `(my, bw-1-mx)` for one to three turns; the arms are
/// those solved for `(mx, my)`. The forward map for one turn is the inverse for
/// three, so writing it here turns every picture the wrong way.
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

    /// Pins `atlas_downscale` to the device measurements: 8192×64 fits and 8193×64
    /// does not, two 4500×3000 photographs fit at factor 1 and three do not. Both
    /// of the pair are needed, or a model that refused everything passes.
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

    /// The reduced set actually fits, not merely "the factor is above one".
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

    /// Tallest-first matters only with three pictures (4096 wide: two 5000 tall,
    /// one 800), where the order decides whether the second tall one wraps past
    /// 8192. With two the order cannot matter, so that fixture is the control.
    /// flip: remove the `sort_by` in `fits_by_shelf` → fails at the first assertion.
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

    /// A 2×2 PNG: red, green / blue, transparent, with known pixels.
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

    /// A uniform grayscale PNG, `w` × `h`: it compresses to almost nothing and
    /// `into_rgba8` quadruples it.
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

    /// A 1×1 red BMP, written by hand since no encoder is available. It is valid
    /// on purpose: a malformed one would be refused by every build.
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

    /// BMP bytes labelled PNG are refused on every platform (§15 D600). The
    /// `Corrupt` assertion bites only under `cargo test --workspace`, which
    /// compiles the `bmp` decoder; the `signature_is_accepted` ones bite under
    /// either. Control: `a_png_is_still_a_png`.
    #[test]
    fn a_format_outside_the_four_is_refused_whatever_the_platform_compiled() {
        // The fixture has to actually be a BMP.
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
    /// and all.
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

    /// A picture's RGBA expansion is bounded in RGBA bytes. The control matters as
    /// much: a 50 MP camera file (240 MB) must be accepted. A cap on the encoded
    /// file bytes stays green here. flip: delete the guard in `decode` → fails at
    /// the `Err` assertion.
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

        // The control: 9504×6336 is 240 MB and must come in.
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

    /// The same id asked for twice does not decode twice.
    #[test]
    fn a_second_ask_for_the_same_id_reuses_the_decoded_buffer() {
        let mut store = ImageStore::new();
        store
            .insert(&id("a"), &png_2x2(), ImageFormat::Png)
            .unwrap();
        let first = store.get(&id("a")).unwrap().data.data().as_ptr();

        // Different bytes under the same id cannot happen honestly (an id is a
        // content hash); the unchanged *pointer* proves nothing was decoded.
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

    /// A mislabelled image decodes as what it is; the bytes are the authority.
    #[test]
    fn the_claimed_format_does_not_have_to_be_right() {
        let mut store = ImageStore::new();
        let size = store.insert(&id("a"), &png_2x2(), ImageFormat::Jpeg);
        assert_eq!(size, Ok((2, 2)), "a PNG labelled JPEG still decodes");
    }

    /// A square PNG `n`×`n`, decoding to `n * n * 4` bytes.
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

    /// `prepare` decodes what the document carries and counts decoded bytes.
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

    /// An image the document no longer carries is dropped, oldest first, once that
    /// puts the store over budget, and not before.
    #[test]
    fn an_image_the_document_dropped_is_evicted_oldest_first_when_over_budget() {
        let one = 8 * 8 * 4;
        // Room for exactly three.
        let mut store = ImageStore::with_budget(one * 3);

        // Two entries must go stale at different times and both survive to the
        // eviction, or the id tie-break decides without `touched` being read.
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

    /// Nothing the document still carries is ever evicted, however far over budget
    /// that leaves the store; otherwise it would decode and evict every frame
    /// (§15 D160).
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

    /// A linked image is not decoded by `prepare`; until something calls `insert`
    /// with its bytes it behaves as a missing image.
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

        /// An unadjusted picture costs no second buffer or cache entry; the
        /// pointer is the assertion, as a copy would look identical on screen.
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

        /// The key is the picture and what was done to it.
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

        /// An export store never answers with a preview, however large the
        /// picture. Asserted on the dimensions, not a flag.
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

        /// `PREVIEW_EDGE` is a floor: a picture merely over it is not reduced, and
        /// one that is reduced never drops below it. flip: round the factor up in
        /// `reduced` → the first loop fails, as every size reduces.
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

        /// The tables agree with `AdjustPipeline::apply` (§15 D186); a drifted table
        /// would still look like a photograph. One level of tolerance: the tables
        /// round twice and the curve's slope is under 2.
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

        /// A reduction maps back onto the source's box corner to corner, at sizes
        /// the factor does not divide, where a map built from the factor is wrong.
        #[test]
        fn a_reduced_picture_maps_onto_the_source_box_corner_to_corner() {
            for n in [PREVIEW_EDGE * 2 + 33, PREVIEW_EDGE * 3 + 7] {
                let store = store_with(n, true);
                let px = store
                    .pixels(&reference("a", |a| a.exposure = 0.5))
                    .expect("held");
                // A size that is not reduced would make the assertions below trivial.
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

        /// A picture already smaller than the preview edge is its own preview, so
        /// nothing is promoted and no repaint is owed.
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

        /// A settled adjustment is promoted and a moving one is not; the failure
        /// modes mirror each other, so it is one test. The drag half asserts
        /// `derived_full_len`, not the width: a fresh key always gets a preview, and
        /// the eager rule's fault is a full buffer only the count can see.
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

        /// A scrolled-past preview is dropped and a full-resolution answer is kept.
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

        /// Derived pixels go when their picture does; they would be unreachable.
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

        /// An export-shaped run stays under the derived budget. No `prepare`, so
        /// the clock stands at 0 and every entry is live, as in an export. flip:
        /// remove the `trim_derived` call from `pixels`, or filter its candidates
        /// to `!live(d.asked)` → each fails at the final count (the second leaves
        /// `a_stale_derived_buffer_goes_before_a_live_one` green).
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
                    // The fixture: a built buffer must fit the budget.
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

        /// The control: a stale buffer goes before a live one (§15 D160). Survivors
        /// are read with `holds_derived`, since `pixels` rebuilds. flip: invert the
        /// sort key in `trim_derived` → fails at "the live buffer was evicted while
        /// a stale one was available".
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

            // Asked of `holds_derived`, not `pixels`, which would rebuild.
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

        /// A store holding a 2×2 under `"a"`: red green over blue white, asymmetric
        /// so an orientation can be read off.
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

        /// A quarter turn puts the picture's top-left at the thumbnail's top-right.
        /// flip: use the forward map in `un_orient` instead of its inverse → fails
        /// at the one-quarter assertion with `green` where `blue` is expected.
        #[test]
        fn a_quarter_turn_turns_the_thumbnail_the_way_the_canvas_turns_the_picture() {
            let store = store_2x2();
            let mut r = ImageRef::new(id("a"));
            // `reduced` declines a 2×2, so the thumbnail is cut from the decode:
            // one source pixel per output pixel, and the assertion is the map alone.
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

        /// A wide picture is centre-cropped, not fitted: a fit averages to grey and
        /// a crop from the origin answers red.
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

        /// The crop is read in the *oriented* picture's space (§5.5a): one
        /// rectangle under two orientations selects different pixels.
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

        /// A thumbnail must not enter the promotion machinery. The closing `pixels`
        /// call is the contrast against a store that cached nothing, and the
        /// fixture must exceed `2 · PREVIEW_EDGE`, or `wants_frame` is false
        /// whatever either function does.
        #[test]
        fn asking_for_a_thumbnail_leaves_the_adjustment_cache_alone() {
            let mut store = ImageStore::interactive();
            store
                .insert(&id("a"), &png_square(2100), ImageFormat::Png)
                .expect("the fixture decodes");
            let mut r = ImageRef::new(id("a"));
            r.adjust.exposure = 0.5;

            // Twice, since one ask could never promote whatever the implementation.
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

            // The contrast: the canvas's entry point does all of that.
            store.pixels(&r).expect("held");
            assert_eq!(store.derived_len(), 1);
            assert!(store.wants_frame());
        }

        /// The neutral adjustment is one key, not two: `-0.0` and `0.0` differ in
        /// bits, so raw-bit keying would give one drawing two textures.
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

        /// The reduced base is cut once per picture and shared by every thumbnail.
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

        /// The reduced base goes when its picture does, not when the last
        /// adjustment does: a thumbnail leaves no derived entry, so the `small`
        /// rule would recut the base every `prepare`.
        #[test]
        fn a_thumbnail_base_goes_when_its_picture_does_and_not_before() {
            let one = 8 * 8 * 4;
            let mut store = ImageStore::with_budget(one);
            store.prepare(&doc_with(&["a"], 8));
            store.thumbnail(&ImageRef::new(id("a")), 4).expect("held");
            assert_eq!(store.derived.lock().unwrap().thumb_base.len(), 1);

            // Surviving a `prepare` is the half that fails against the `small` rule.
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
