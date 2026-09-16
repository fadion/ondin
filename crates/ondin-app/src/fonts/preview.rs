//! Picker rows drawn in the family they name (§9.2).
//!
//! **The list is the sample sheet.** Choosing a typeface is done by eye, so a
//! column of names all set in the UI font is a list you have to click through to
//! read; a column of names each set in its own face is the thing designers
//! actually pick from, and it is the one part of Figma's font picker that is not
//! about speed.
//!
//! ## Why not egui's own fonts
//!
//! The obvious route is `Context::add_font` per family. It is a trap: `add_font`
//! sets `fonts = None`, so egui rebuilds **every** font, drops every cached
//! galley and re-uploads the whole atlas. Per row that is a stutter per row; even
//! batched it is a dropped frame each time a download lands, and it keeps a
//! second copy of every font's bytes beside parley's.
//!
//! So a row's name is rasterized through the same CPU backend that drives PNG
//! export, and cached as one small texture per family. Nothing touches egui's
//! atlas, the cost is paid once per family, and — the part that matters — the
//! preview goes through the *document's* shaping path, so what the row shows is
//! what the canvas will draw.
//!
//! The strips are **white coverage, tinted at draw time**: one cached image
//! serves the normal, hovered, selected and disabled colours, and follows the
//! theme instead of being baked against it.

use eframe::egui;
use ondin_core::kurbo::Affine;
use ondin_core::text;

/// Type size for a row's name, in points. One size for every family, as a
/// sample sheet has: the row is for comparing typefaces, and a per-family size
/// would be comparing our arithmetic instead.
const PREVIEW_PT: f32 = 15.0;

/// Widest strip rasterized, in points. Long family names are clipped by the row
/// rather than growing an unbounded texture; the useful part of a name is its
/// beginning.
const MAX_STRIP_PT: f32 = 260.0;

/// Tallest strip rasterized, in points — **a refusal, not a clip** (§15 D586,
/// `[S21-L1-05]`).
///
/// A strip is one line, so clipping the height the way [`MAX_STRIP_PT`] clips the
/// width would draw a sliced name rather than a shorter one. A family whose line
/// box exceeds this is declined and the row falls back to the UI font, which
/// `paint` already does for three other reasons.
///
/// **Six times [`PREVIEW_PT`], which is five times the widest real ratio.** The
/// hazard is a font declaring a small `unitsPerEm` against maximal
/// `ascender`/`descender`: the spec allows `unitsPerEm` down to 16 and both
/// metrics are `i16`, so a line-box ratio of ~2,048 is representable and 15 pt ×
/// 2,048 × ppp 2 is about 61,000 px. Inter measures 18.15 pt at 15 pt — a ratio
/// of 1.21 — so no real face comes near this, and the bound is set by what a
/// *row* can sensibly show rather than by what would overflow.
///
/// ⚠️ **The `u16` overflow is refused at the other end too** —
/// `ondin_render::rasterize_glyph_run` now answers `None` rather than truncating —
/// and that is deliberately not the only guard. This one keeps a merely *large*
/// strip from being a 128 MB allocation on the UI thread for one picker row,
/// which is the half that stays a hazard well below the wrap.
const MAX_STRIP_H_PT: f32 = PREVIEW_PT * 6.0;

/// How much of one frame may go into rasterizing strips.
///
/// **The bytes are off the UI thread and the raster is not**, and it cannot be:
/// [`text::family_preview`] goes through core's thread-local shaping engine, so a
/// worker would build its own font collection instead of sharing the registered
/// one. What is bounded instead is how much of a frame the misses may take — a
/// flung scrollbar brings a screenful of new families into view at once, and
/// rasterizing all of them in the frame that first shows them is the stall.
///
/// **A time budget rather than a count of strips, because the cost per strip
/// spans 40× between builds.** Measured, for a warm family: 0.022 ms in release
/// and 0.80 ms in debug at 100% scale, 0.031 against 1.25 at 200%. A count tuned
/// to the debug cost would leave the release build doing a fortieth of the work it
/// could afford in the same time; one tuned to release would stall the debug build
/// the app is actually run from. At 2 ms this is about two strips a debug frame
/// and a whole screenful of them in release, which is the behaviour wanted at both
/// ends. *The measurement is a floor, not the real cost*: it re-rasterized one
/// family, so parley's family lookup was warm, where a scroll hits families it has
/// never resolved.
///
/// A deferred row draws its name in the UI font for that frame — the fallback
/// `paint` already had for a family whose bytes have not arrived — so the visible
/// cost of the bound is a name or two appearing in the wrong face for a frame.
const FRAME_BUDGET: std::time::Duration = std::time::Duration::from_millis(2);

/// One family name, rasterized in its own face.
struct Strip {
    texture: egui::TextureHandle,
    /// Size in points, which is what the row lays out in.
    size: egui::Vec2,
    /// Distance from the top of the strip to the baseline, in points.
    baseline: f32,
}

/// Cached preview strips, one per family.
pub struct FontPreviews {
    /// `None` records a family whose name cannot be previewed *yet* (bytes have
    /// not arrived) or *ever* (a symbol font, whose Latin name is all `.notdef`).
    /// Keeping the negative answer is what stops every frame re-shaping the name
    /// of every visible row; it is dropped again whenever fonts are registered.
    strips: std::collections::HashMap<String, Option<Strip>>,
    /// The display scale the strips were rasterized at.
    ppp: f32,
    /// The pass [`Self::spent`] was accumulated in, so the budget resets once per
    /// frame. egui's own counter rather than a flag someone has to remember to
    /// clear: `paint` is the only entry point, and it cannot know it is the first
    /// row of a frame except by asking.
    pass: u64,
    /// Time this pass has put into [`rasterize`].
    spent: std::time::Duration,
    /// [`FRAME_BUDGET`], as a field so a test can take it away.
    budget: std::time::Duration,
}

impl Default for FontPreviews {
    fn default() -> Self {
        FontPreviews {
            strips: std::collections::HashMap::new(),
            ppp: 0.0,
            pass: 0,
            spent: std::time::Duration::ZERO,
            budget: FRAME_BUDGET,
        }
    }
}

impl FontPreviews {
    /// Draw `family`'s name inside `rect`, left-aligned and vertically centred,
    /// in the family itself. Returns `false` when there is no truthful preview to
    /// draw **or none to draw yet**, and the caller should fall back to the UI
    /// font.
    ///
    /// The two reasons for `false` are deliberately not distinguished: the caller
    /// does the same thing either way, and a row that says "coming" in one case and
    /// draws the name in the other would be two treatments of one appearance.
    pub fn paint(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        family: &str,
        tint: egui::Color32,
    ) -> bool {
        let ppp = ui.ctx().pixels_per_point();
        if self.ppp != ppp {
            // Every strip was rasterized for the old scale, so all of them are
            // now the wrong size. Cheaper to redo than to draw blurred.
            self.strips.clear();
            self.ppp = ppp;
        }
        let pass = ui.ctx().cumulative_pass_nr();
        if self.pass != pass {
            self.pass = pass;
            self.spent = std::time::Duration::ZERO;
        }
        if !self.strips.contains_key(family) {
            // **One strip a frame whatever the budget says**, which is the
            // progress guarantee: a scale or a machine slow enough to spend the
            // whole budget on a single strip would otherwise leave every row in the
            // fallback font for ever, and the list would never become the sample
            // sheet it exists to be.
            if !self.spent.is_zero() && self.spent >= self.budget {
                // Without this the deferred rows wait for the next input event,
                // since the app repaints reactively — a fling would settle with
                // half its names in the UI font until the pointer moved again.
                ui.ctx().request_repaint();
                return false;
            }
            let started = std::time::Instant::now();
            let strip = rasterize(ui.ctx(), family, ppp);
            self.spent += started.elapsed();
            self.strips.insert(family.to_string(), strip);
        }
        let Some(Some(strip)) = self.strips.get(family) else {
            return false;
        };
        // **One baseline down the whole list**, rather than each name's own line
        // box centred in the row. Auto leading is a different height in every
        // family, so centring the boxes would sit each name a pixel or two off
        // its neighbours — in a column of forty rows that reads as the list
        // wobbling. The offset stands in for half a cap height, which is where a
        // baseline lands when a line of type is optically centred.
        let baseline_y = (rect.center().y + PREVIEW_PT * 0.35).round();
        let at = egui::Rect::from_min_size(
            egui::pos2(rect.left(), baseline_y - strip.baseline),
            strip.size,
        );
        // Clipped to the space the row gave it: a long family name is wider than
        // the panel, and without this it would run under the scrollbar rather
        // than stopping at the row's edge.
        ui.painter()
            .with_clip_rect(rect.intersect(ui.clip_rect()))
            .image(
                strip.texture.id(),
                at,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                tint,
            );
        true
    }

    /// Forget the families that could not be previewed, because new font data
    /// has arrived and some of them can be now.
    pub fn retry_misses(&mut self) {
        self.strips.retain(|_, strip| strip.is_some());
    }

    /// Drop a family's strip — its faces have been un-registered, so the strip
    /// cannot be redrawn and is holding a texture for nothing.
    pub fn forget(&mut self, family: &str) {
        self.strips.remove(family);
    }
}

/// The pixel size of a strip measuring `width` × `height` points at `ppp`, or
/// `None` when it must not be rasterized at all.
///
/// **A free function so the bound can be driven** (§15 D586, D269's shape): the
/// input that reaches the refusal is a font declaring a small `unitsPerEm`
/// against maximal `ascender`/`descender`, and no such face is in the tree — so
/// with this inline in [`rasterize`] the only test that could reach it would have
/// to build an sfnt by hand.
///
/// **The two axes are guarded in different ways, deliberately.** A width over
/// [`MAX_STRIP_PT`] is *clipped*, because the useful part of a family name is its
/// beginning; a height over [`MAX_STRIP_H_PT`] is **refused**, because a strip is
/// one line and half of one is not a shorter name.
///
/// A pixel of margin each side: glyphs of a script face lean out past the advance
/// width they were measured by, and a clipped serif reads as a rendering bug.
fn strip_px(width: f32, height: f32, ppp: f32) -> Option<(u32, u32)> {
    let width = width.min(MAX_STRIP_PT);
    if width <= 0.0 || height <= 0.0 || height > MAX_STRIP_H_PT {
        return None;
    }
    Some((
        (width * ppp).ceil() as u32 + 2,
        (height * ppp).ceil() as u32 + 2,
    ))
}

/// Shape `family`'s name in `family` and rasterize it, or `None` if that would
/// not be a truthful preview (see [`text::family_preview`]).
fn rasterize(ctx: &egui::Context, family: &str, ppp: f32) -> Option<Strip> {
    let preview = text::family_preview(family, family, f64::from(PREVIEW_PT))?;
    let (w_px, h_px) = strip_px(preview.width as f32, preview.height as f32, ppp)?;
    // Scale first, then shift by one *device* pixel — kurbo composes right to
    // left, so the translate written on the left is applied last.
    let transform = Affine::translate((1.0, 1.0)) * Affine::scale(f64::from(ppp));
    // `None` when the backend cannot hold a buffer that size — the row falls back
    // to the UI font rather than handing `ColorImage` a buffer that does not match
    // the dimensions beside it, which is what asserted inside epaint (§15 D586).
    let rgba = ondin_render::rasterize_glyph_run(&preview.run, transform, w_px, h_px)?;
    let image = egui::ColorImage::from_rgba_unmultiplied([w_px as usize, h_px as usize], &rgba);
    Some(Strip {
        texture: ctx.load_texture(
            format!("font-preview:{family}"),
            image,
            egui::TextureOptions::LINEAR,
        ),
        size: egui::vec2(w_px as f32 / ppp, h_px as f32 / ppp),
        baseline: preview.baseline as f32 + 1.0 / ppp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A miss past the frame's budget is deferred, not recorded as a miss** —
    /// and the difference is the whole of the mechanism, because the two look
    /// identical from the row: `paint` returns `false` either way and the name
    /// draws in the UI font.
    ///
    /// What separates them is the cache. A family that *cannot* be previewed is
    /// remembered as `None`, which is what stops every frame re-shaping it; a
    /// family that has merely run out of budget must leave no entry at all, or the
    /// deferral would be permanent until something called
    /// `FontPreviews::retry_misses`.
    ///
    /// The budget is taken away rather than waited out, because a test that spends
    /// 2 ms of real rasterizing to get there would be timing-dependent on exactly
    /// the 40× build difference `FRAME_BUDGET` is written about.
    ///
    /// **Both rows are painted inside one `run_ui`, and that is the point rather
    /// than a detail.** The budget is per *pass*, so a version of this that called
    /// `run_ui` once per row reset it between them and proved nothing — the second
    /// row rasterized as the first of its own frame. A list paints all its rows in
    /// one pass, which is the arrangement being tested.
    #[test]
    fn a_strip_past_the_frames_budget_is_deferred_and_the_budget_resets_next_frame() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut previews = FontPreviews {
            budget: std::time::Duration::ZERO,
            ..Default::default()
        };
        let unknown = "No Such Font 12345";
        let (mut first, mut second) = (false, false);
        let _ = ctx.run_ui(Default::default(), |ui| {
            let row = ui.max_rect();
            first = previews.paint(ui, row, "Inter", egui::Color32::WHITE);
            second = previews.paint(ui, row, unknown, egui::Color32::WHITE);
        });

        // The progress guarantee: the first strip of a frame draws even with no
        // budget at all, or a slow machine would never show a preview.
        assert!(
            first,
            "the first strip of a frame must draw whatever the budget says"
        );
        assert!(!previews.spent.is_zero(), "and it must count against it");
        assert!(!second, "the second must not, the budget being spent");
        assert!(
            !previews.strips.contains_key(unknown),
            "a deferred family must not be cached as un-previewable"
        );

        // A new pass resets the budget, so the deferred row is tried again — and
        // this one really cannot be previewed, so now it is remembered.
        let _ = ctx.run_ui(Default::default(), |ui| {
            previews.paint(ui, ui.max_rect(), unknown, egui::Color32::WHITE);
        });
        assert!(
            matches!(previews.strips.get(unknown), Some(None)),
            "a family with no bytes is a recorded miss: {:?}",
            previews.strips.keys().collect::<Vec<_>>()
        );
    }

    /// **A family with an extreme line box is declined, where a long name is
    /// merely clipped** (§15 D586, `[S21-L1-05]`).
    ///
    /// The width was capped and the height was not, and `rasterize_glyph_run`
    /// truncated to `u16` — so `h_px = 70000` handed `ColorImage` 142,848 bytes
    /// where it wanted 2,240,000 and epaint asserted, on the UI thread, for one
    /// picker row. Below the wrap the same arithmetic is a 128 MB allocation, and
    /// `paint`'s progress guarantee runs the first strip of a frame *before*
    /// checking the budget, so `FRAME_BUDGET` does not bound it either.
    ///
    /// ⚠️ **The input is arithmetic and not a font**, and that is the honest
    /// limit of this test. The face that produces such a height is one declaring
    /// `unitsPerEm = 16` against maximal `i16` metrics; no such file is in the
    /// tree and building one is a bigger fixture than the guard. What is driven
    /// here is the decision, which is why it was lifted out of `rasterize`.
    ///
    /// **Flip run**, the `height > MAX_STRIP_H_PT` term removed: fails on *"an
    /// extreme line box must be declined"*, having answered `Some((242, 61442))`
    /// — the 61,442 being the number that reaches the backend, which is past both
    /// `MAX_RASTER_SIDE` and the `u16` this used to be truncated by.
    #[test]
    fn an_extreme_line_box_is_declined_and_a_long_name_is_only_clipped() {
        // The fixture, in real numbers: Inter measures 18.15 pt at 15 pt, a ratio
        // of 1.21, so an ordinary family is nowhere near the bound.
        let inter = strip_px(120.0, 18.15, 2.0).expect("an ordinary family previews");
        assert_eq!(inter, (242, 39));

        // 15 pt × 2048 is the ratio the format allows, and it is what the bound is
        // for.
        assert_eq!(
            strip_px(120.0, PREVIEW_PT * 2048.0, 2.0),
            None,
            "an extreme line box must be declined"
        );

        // The other axis, and the other *kind* of guard: a name three times the
        // cap is clipped to the cap rather than refused, because the useful part
        // of a name is its beginning.
        let (w, _) = strip_px(MAX_STRIP_PT * 3.0, 18.15, 1.0).expect("a long name still previews");
        assert_eq!(
            w,
            MAX_STRIP_PT as u32 + 2,
            "a long name is clipped, not refused"
        );

        // And the lower end, which was the only one guarded before this.
        assert_eq!(strip_px(0.0, 18.0, 1.0), None);
        assert_eq!(strip_px(120.0, 0.0, 1.0), None);
    }
}
