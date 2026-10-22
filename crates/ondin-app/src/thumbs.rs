//! Layer and paint-row thumbnails: decoded pixels in, an egui texture out.
//!
//! The pixels are `ondin_render::ImageStore`'s (a crate that knows no egui) and the
//! texture is the chrome's; `CanvasRenderer::images` is a `&self` handle, so the
//! texture cache lives here instead.
//!
//! Like `fonts::FontPreviews` it is keyed by what the drawing depends on, with a
//! per-pass time budget and one upload a pass whatever the budget (§15 D160). Unlike
//! it, it sweeps: a thumbnail is keyed by scrubbable floats, so one slider drag walks
//! hundreds of keys that are never asked for again.

use eframe::egui;
use ondin_core::ImageRef;
use ondin_render::{ImageStore, ThumbKey};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How much of one pass may go into building thumbnails.
///
/// The first ask for a picture is the expensive one: it cuts the reduced base,
/// reading every pixel of the decode, while later asks read a small buffer. A
/// deferred row draws its layer glyph or placeholder grey for a pass.
const FRAME_BUDGET: Duration = Duration::from_millis(2);

/// How many passes a thumbnail survives without being asked for. Three rather than
/// one because egui runs a second pass over a frame when a layout needs settling,
/// and a widget painted only in the first must keep its texture.
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
    /// The pass [`Self::spent`] was accumulated in; the budget and the sweep reset once per pass.
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
    /// The texture for what `r` shows, in a square `edge` points on a side, or `None`
    /// when there is nothing to draw or none yet (the caller draws its placeholder
    /// either way).
    ///
    /// The store is asked in device pixels, so display scale lands in the key: a
    /// scale change misses and the sweep collects the old size.
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
            self.thumbs.retain(|_, t| t.asked + KEEP_PASSES >= pass);
        }
        let px = (edge * ctx.pixels_per_point()).ceil().max(1.0) as u32;
        let key = ThumbKey::of(r, px);
        if let Some(thumb) = self.thumbs.get_mut(&key) {
            thumb.asked = pass;
            return Some(thumb.texture.id());
        }
        // One upload a pass whatever the budget says: the progress guarantee for a
        // picture big enough to spend the whole budget.
        if !self.spent.is_zero() && self.spent >= self.budget {
            // The app repaints reactively; without this a deferred row waits for input.
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
        // Counted whether or not anything was built, so the budget measures the frame.
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

    /// A 4×4 PNG of one flat colour.
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

    /// A second ask in the same pass is a hit, and a moved slider is not (a cache
    /// keyed by the picture alone would stop following the adjustment).
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

    /// What a scrub leaves behind is swept: ten slider values, then the tenth kept
    /// being drawn. The settled value surviving separates a sweep from a clear.
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
        // `KEEP_PASSES + 1` exactly: the sweep runs at the top of a pass, which then
        // adds its own. A bound would also hold with no sweep at all.
        assert_eq!(
            thumbs.thumbs.len(),
            KEEP_PASSES as usize + 1,
            "a ten-value scrub left {} textures held",
            thumbs.thumbs.len()
        );

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

    /// A thumbnail past the pass's budget is deferred, not recorded as missing. The
    /// budget is zeroed rather than waited out, and both asks share one `run_ui`
    /// because the budget is per pass.
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

        // A new pass resets it, and the deferred one arrives.
        let _ = ctx.run_ui(Default::default(), |_| {
            two = thumbs.texture(&ctx, &store, &second, 20.0);
        });
        assert!(two.is_some(), "a deferred thumbnail must be tried again");
    }

    /// A picture the store cannot draw asks again next pass, unlike a font preview:
    /// the lookup is cheap and the answer changes when a linked file loads or an
    /// undo brings the bytes back.
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

        // When the bytes arrive it draws.
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
