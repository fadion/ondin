//! Layer and paint-row thumbnails: decoded pixels in, an egui texture out.
//!
//! **The seam this file is, and why it is here rather than on the canvas.**
//! `ondin_render::ImageStore` holds `peniko::ImageData` and the panels draw
//! through `egui::TextureHandle`, so something has to own the upload between
//! them and decide how long it lives. The store cannot: it is in a crate that
//! does not know egui exists, and `ondin export --png` holds one. And
//! `CanvasRenderer` should not, because [`crate::canvas::CanvasRenderer::images`] is
//! deliberately a `&self` handle — a mutable texture cache behind it would put
//! every panel on `images_mut`, which exists for one caller for a stated reason.
//! So the pixels are the store's and the texture is the chrome's, and this is
//! the chrome's half.
//!
//! It is `fonts::FontPreviews` again, in nearly every respect — a map keyed by
//! what the drawing depends on, a per-pass time budget so a scroll that brings a
//! screenful of new pictures into view does not rasterize all of them in the
//! frame that first shows them, and a progress guarantee of one upload a pass
//! however spent the budget is. §15 D160 is where those numbers were argued.
//!
//! **One respect where it is not, and it is the one that matters.** A font
//! preview is keyed by family, and a document has a bounded number of families,
//! so `FontPreviews` never evicts anything. A thumbnail is keyed by seven
//! scrubbable floats and a crop rectangle, so a single drag of the exposure
//! slider walks a few hundred distinct keys — one texture each, uploaded and
//! then never asked for again. So this sweeps: anything no recent pass wanted
//! goes. Without that the layers panel is a slow leak that only shows up in a
//! session long enough to have adjusted a few pictures.

use eframe::egui;
use ondin_core::ImageRef;
use ondin_render::{ImageStore, ThumbKey};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How much of one pass may go into building thumbnails.
///
/// The same 2 ms the font previews take, for the same reason and against a
/// similar cost. **The first ask for a picture is the expensive one**: it cuts
/// the reduced base, which reads every pixel of the decode whatever size is
/// wanted out of it, so a 12-megapixel photograph is tens of milliseconds and a
/// panel scrolled onto ten fresh ones would spend all of it in a single frame.
/// Every ask after that is a few hundred reads out of a 128×128 buffer.
///
/// A deferred row draws what it drew before there was a thumbnail — the layer
/// glyph, or the chip's placeholder grey — so the visible cost of the bound is a
/// row or two arriving a frame late.
const FRAME_BUDGET: Duration = Duration::from_millis(2);

/// How many passes a thumbnail survives without being asked for.
///
/// Three rather than one, because a pass is not a frame: egui runs a second pass
/// over the same frame whenever a layout needs settling, and a widget that
/// painted in the first and not the second must not lose its texture for it.
/// Above that the number does nothing useful — a scrub abandons a key
/// permanently the instant the value moves, and the point is that it goes.
const KEEP_PASSES: u64 = 3;

/// The whole of a texture, for [`egui::Painter::image`].
pub const FULL_UV: egui::Rect = egui::Rect {
    min: egui::Pos2 { x: 0.0, y: 0.0 },
    max: egui::Pos2 { x: 1.0, y: 1.0 },
};

struct Thumb {
    texture: egui::TextureHandle,
    /// The pass that last wanted this, which [`ImageThumbs::texture`] sweeps by.
    asked: u64,
}

/// Uploaded thumbnails, keyed by everything their drawing depends on.
pub struct ImageThumbs {
    thumbs: HashMap<ThumbKey, Thumb>,
    /// The pass [`Self::spent`] was accumulated in, so the budget resets once per
    /// pass and the sweep runs once per pass. egui's own counter rather than a
    /// flag someone has to remember to clear, for `FontPreviews`' reason: there
    /// is one entry point and it cannot know it is the first caller of a pass
    /// except by asking.
    pass: u64,
    spent: Duration,
    /// [`FRAME_BUDGET`], as a field so a test can take it away.
    budget: Duration,
}

impl Default for ImageThumbs {
    fn default() -> Self {
        Self {
            thumbs: HashMap::new(),
            pass: 0,
            spent: Duration::ZERO,
            budget: FRAME_BUDGET,
        }
    }
}

impl ImageThumbs {
    /// The texture for what `r` shows, in a square `edge` **points** on a side,
    /// or `None` when there is nothing to draw — or nothing to draw *yet*.
    ///
    /// The two are deliberately not distinguished, exactly as in
    /// `FontPreviews::paint`: the caller does the same thing either way, which is
    /// to draw what it drew before thumbnails existed.
    ///
    /// Points in, device pixels to the store, so a thumbnail is cut at the size
    /// it will be drawn at. Display scale therefore lands in the key and needs no
    /// field of its own to invalidate against — a change of scale simply misses,
    /// and the sweep collects the old size.
    pub fn texture(
        &mut self,
        ctx: &egui::Context,
        store: &ImageStore,
        r: &ImageRef,
        edge: f32,
    ) -> Option<egui::TextureId> {
        let pass = ctx.cumulative_pass_nr();
        if self.pass != pass {
            self.pass = pass;
            self.spent = Duration::ZERO;
            // Anything no recent pass wanted: a scrubbed-past adjustment, a row
            // scrolled out of the list, a picture drawn at the old display scale.
            self.thumbs.retain(|_, t| t.asked + KEEP_PASSES >= pass);
        }
        let px = (edge * ctx.pixels_per_point()).ceil().max(1.0) as u32;
        let key = ThumbKey::of(r, px);
        if let Some(thumb) = self.thumbs.get_mut(&key) {
            thumb.asked = pass;
            return Some(thumb.texture.id());
        }
        // **One upload a pass whatever the budget says**, which is the progress
        // guarantee: a picture big enough to spend the whole budget cutting its
        // base would otherwise leave its row showing a glyph for ever.
        if !self.spent.is_zero() && self.spent >= self.budget {
            // Without this a deferred row waits for the next input event, since
            // the app repaints reactively — a flung layers panel would settle
            // with half its pictures still drawn as squares.
            ctx.request_repaint();
            return None;
        }
        let started = Instant::now();
        let built = store.thumbnail(r, px).map(|data| {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [data.width as usize, data.height as usize],
                data.data.data(),
            );
            ctx.load_texture(
                format!("image-thumb:{}", r.id),
                image,
                egui::TextureOptions::LINEAR,
            )
        });
        // Counted whether or not there was anything to build. A miss is a hash
        // lookup and costs nothing, but timing only the successes is how a budget
        // ends up measuring something other than the frame.
        self.spent += started.elapsed();
        let texture = built?;
        let id = texture.id();
        self.thumbs.insert(
            key,
            Thumb {
                texture,
                asked: pass,
            },
        );
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::{ImageFormat, ImageId};

    /// A 4×4 PNG of one flat colour. Encoded through `image`, which this crate
    /// already depends on for placement, rather than declaring the `png` crate
    /// for four pixels of fixture.
    fn flat_png(v: u8) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([v, v, v, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png)
            .expect("the fixture encodes");
        out.into_inner()
    }

    fn store_with_one() -> ImageStore {
        let mut store = ImageStore::new();
        store
            .insert(&ImageId("a".into()), &flat_png(200), ImageFormat::Png)
            .expect("the fixture decodes");
        store
    }

    /// **A second ask in the same pass is a hit, and a moved slider is not.**
    ///
    /// Both halves matter and each fails on its own. Hit too rarely and every
    /// frame of a scrub uploads a texture for both call sites; hit too often —
    /// keyed by the picture alone, which is the obvious thing to write — and the
    /// thumbnail stops following the adjustment it is supposed to be showing,
    /// which looks like the cache working.
    #[test]
    fn one_texture_serves_a_repeated_ask_and_a_changed_value_gets_its_own() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let store = store_with_one();
        let mut thumbs = ImageThumbs::default();
        let mut r = ImageRef::new(ImageId("a".into()));

        let (mut first, mut again, mut moved) = (None, None, None);
        let _ = ctx.run_ui(Default::default(), |_| {
            first = thumbs.texture(&ctx, &store, &r, 20.0);
            again = thumbs.texture(&ctx, &store, &r, 20.0);
            r.adjust.exposure = 0.5;
            moved = thumbs.texture(&ctx, &store, &r, 20.0);
        });

        assert!(first.is_some(), "the fixture is drawable");
        assert_eq!(first, again, "the same reference twice is one texture");
        assert_ne!(
            first, moved,
            "a moved adjustment must not draw the picture it had before"
        );
        assert_eq!(thumbs.thumbs.len(), 2);
    }

    /// **What a scrub leaves behind is swept**, which is the one thing this cache
    /// does that `FontPreviews` does not have to.
    ///
    /// Ten values of a slider, then the panel goes on drawing the tenth. Without
    /// the sweep all ten textures are still held, and the leak is invisible until
    /// a session has adjusted enough pictures to matter.
    ///
    /// The last value is asked for *throughout*, not merely at the end: that is
    /// what separates "stale entries go" from "the map is cleared", and the
    /// second would pass a test that only counted what was left.
    #[test]
    fn a_scrub_leaves_one_texture_behind_rather_than_one_per_value() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let store = store_with_one();
        let mut thumbs = ImageThumbs::default();
        let mut r = ImageRef::new(ImageId("a".into()));

        for step in 0..10i16 {
            r.adjust.exposure = 0.05 * f32::from(step);
            let _ = ctx.run_ui(Default::default(), |_| {
                thumbs.texture(&ctx, &store, &r, 20.0);
            });
        }
        // **`KEEP_PASSES + 1`, exactly**, and the `+ 1` is the sweep running at
        // the *top* of a pass: three survivors are carried in and the pass then
        // adds its own. An exact count rather than a bound because the steady
        // state is what a leak would show up against — "ten values left at most
        // ten textures" is true of the version with no sweep at all.
        assert_eq!(
            thumbs.thumbs.len(),
            KEEP_PASSES as usize + 1,
            "a ten-value scrub left {} textures held",
            thumbs.thumbs.len()
        );

        // The settled value survives being the only one asked for, which is what
        // makes the count above a sweep rather than a clear.
        let settled = r.clone();
        for _ in 0..KEEP_PASSES + 2 {
            let _ = ctx.run_ui(Default::default(), |_| {
                thumbs.texture(&ctx, &store, &settled, 20.0);
            });
        }
        assert_eq!(
            thumbs.thumbs.len(),
            1,
            "the value still on screen must not be swept with the rest"
        );
    }

    /// **A thumbnail past the pass's budget is deferred, not recorded as
    /// missing** — and the caller cannot tell, which is why the state has to be.
    ///
    /// The budget is taken away rather than waited out: spending 2 ms of real
    /// reduction to reach it would make the test depend on the machine it runs
    /// on, and on the `opt-level = 3` that only `ondin-render` carries.
    ///
    /// **Both asks are inside one `run_ui`**, which is the arrangement being
    /// tested rather than a detail — the budget is per pass, so a version that
    /// called `run_ui` twice would reset it between them and prove nothing.
    #[test]
    fn a_thumbnail_past_the_passes_budget_is_deferred_and_the_budget_resets() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let store = store_with_one();
        let mut thumbs = ImageThumbs {
            budget: Duration::ZERO,
            ..Default::default()
        };
        let first = ImageRef::new(ImageId("a".into()));
        let mut second = ImageRef::new(ImageId("a".into()));
        second.adjust.exposure = 0.5;

        let (mut one, mut two) = (None, None);
        let _ = ctx.run_ui(Default::default(), |_| {
            one = thumbs.texture(&ctx, &store, &first, 20.0);
            two = thumbs.texture(&ctx, &store, &second, 20.0);
        });
        assert!(
            one.is_some(),
            "the first upload of a pass must happen whatever the budget says"
        );
        assert!(!thumbs.spent.is_zero(), "and it must count against it");
        assert!(two.is_none(), "the second must not, the budget being spent");

        // A new pass resets it, and the deferred one arrives — a deferral must
        // leave no entry behind saying the picture cannot be drawn.
        let _ = ctx.run_ui(Default::default(), |_| {
            two = thumbs.texture(&ctx, &store, &second, 20.0);
        });
        assert!(two.is_some(), "a deferred thumbnail must be tried again");
    }

    /// **A picture the store cannot draw asks again next pass**, where a font
    /// preview remembers its misses.
    ///
    /// The difference is real rather than an inconsistency: a family that cannot
    /// be previewed has to be re-shaped to find that out, so `FontPreviews`
    /// caches the negative. Asking the store for a picture it does not hold is a
    /// hash lookup, and the answer changes the moment a linked file is loaded or
    /// an undo brings the bytes back — so caching it would be paying to be wrong.
    #[test]
    fn a_picture_the_store_does_not_hold_is_not_remembered_as_undrawable() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut thumbs = ImageThumbs::default();
        let r = ImageRef::new(ImageId("missing".into()));

        let mut got = Some(egui::TextureId::default());
        let empty = ImageStore::new();
        let _ = ctx.run_ui(Default::default(), |_| {
            got = thumbs.texture(&ctx, &empty, &r, 20.0);
        });
        assert!(got.is_none(), "there is nothing to draw");
        assert!(
            thumbs.thumbs.is_empty(),
            "a missing picture must not occupy the cache"
        );

        // And when the bytes arrive it draws, with nothing needing to be told.
        let store = {
            let mut s = store_with_one();
            s.insert(&ImageId("missing".into()), &flat_png(10), ImageFormat::Png)
                .expect("the fixture decodes");
            s
        };
        let _ = ctx.run_ui(Default::default(), |_| {
            got = thumbs.texture(&ctx, &store, &r, 20.0);
        });
        assert!(got.is_some(), "the picture arrived and was not asked for");
    }
}
