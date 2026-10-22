//! Picker rows drawn in the family they name (§9.2).
//!
//! Not `Context::add_font` per family: it rebuilds every egui font, drops every cached
//! galley and re-uploads the atlas. A row's name is instead rasterized through the CPU
//! backend and the document's shaping path, and cached as one texture per family.
//! The strips are white coverage, tinted at draw time, so one image serves every state.

use eframe::egui;
use ondin_core::kurbo::Affine;
use ondin_core::text;

/// Type size for a row's name, in points; one size for every family so rows compare faces.
const PREVIEW_PT: f32 = 15.0;

/// Widest strip rasterized, in points; longer names are clipped, since a name's
/// beginning is the useful part.
const MAX_STRIP_PT: f32 = 260.0;

/// Tallest strip rasterized, in points — a refusal, not a clip (§15 D586).
///
/// A font declaring a small `unitsPerEm` against maximal ascender/descender gets a
/// huge line box; a sliced name is wrong, so the row falls back to the UI font. This
/// also keeps a merely large strip from a huge allocation on the UI thread; the
/// renderer refuses the `u16` overflow separately.
const MAX_STRIP_H_PT: f32 = PREVIEW_PT * 6.0;

/// How much of one frame may go into rasterizing strips.
///
/// The raster cannot leave the UI thread ([`text::family_preview`] uses core's
/// thread-local shaping engine), so the misses are bounded instead. A time budget
/// rather than a count because the per-strip cost differs about 40× between debug
/// and release. A deferred row draws in the UI font for a frame.
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
    /// `None` records a family that cannot be previewed yet (bytes not arrived) or
    /// ever (a symbol font); it stops every frame re-shaping the name, and is
    /// dropped whenever fonts are registered.
    strips: std::collections::HashMap<String, Option<Strip>>,
    /// The display scale the strips were rasterized at.
    ppp: f32,
    /// The pass [`Self::spent`] was accumulated in, so the budget resets once per frame.
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
    /// in the family itself. Returns `false` when there is no preview to draw or
    /// none yet, and the caller falls back to the UI font (one treatment for both).
    pub fn paint(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        family: &str,
        tint: egui::Color32,
    ) -> bool {
        let ppp = ui.ctx().pixels_per_point();
        if self.ppp != ppp {
            // Every strip was rasterized for the old scale.
            self.strips.clear();
            self.ppp = ppp;
        }
        let pass = ui.ctx().cumulative_pass_nr();
        if self.pass != pass {
            self.pass = pass;
            self.spent = std::time::Duration::ZERO;
        }
        if !self.strips.contains_key(family) {
            // One strip a frame whatever the budget says: the progress guarantee on a
            // machine slow enough to spend the whole budget on one strip.
            if !self.spent.is_zero() && self.spent >= self.budget {
                // The app repaints reactively; without this deferred rows wait for input.
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
        // One baseline down the whole list, not each line box centred: leading differs
        // per family and centring would make the column wobble. The offset stands in
        // for half a cap height.
        let baseline_y = (rect.center().y + PREVIEW_PT * 0.35).round();
        let at = egui::Rect::from_min_size(
            egui::pos2(rect.left(), baseline_y - strip.baseline),
            strip.size,
        );
        // Clipped to the row so a long name stops at its edge, not under the scrollbar.
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

    /// Forget the families that could not be previewed; new font data may fix them.
    pub fn retry_misses(&mut self) {
        self.strips.retain(|_, strip| strip.is_some());
    }

    /// Drop a family's strip once its faces are un-registered.
    pub fn forget(&mut self, family: &str) {
        self.strips.remove(family);
    }
}

/// The pixel size of a strip measuring `width` × `height` points at `ppp`, or
/// `None` when it must not be rasterized at all.
///
/// A free function so a test can drive the bound without building an sfnt. A width
/// over [`MAX_STRIP_PT`] is clipped, a height over [`MAX_STRIP_H_PT`] is refused.
/// A pixel of margin each side keeps script faces' overhang from clipping.
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
    // Scale first, then shift one device pixel (kurbo composes right to left).
    let transform = Affine::translate((1.0, 1.0)) * Affine::scale(f64::from(ppp));
    // `None` when the backend cannot hold that buffer; `ColorImage` asserts on a mismatch.
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

    /// A miss past the frame's budget is deferred (no cache entry), not recorded as
    /// an un-previewable `None`. Both rows paint in one `run_ui` because the budget
    /// is per pass; the budget is zeroed rather than waited out to stay timing-free.
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

        // The progress guarantee: the first strip draws even with no budget.
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

        // A new pass resets the budget; this family really cannot be previewed.
        let _ = ctx.run_ui(Default::default(), |ui| {
            previews.paint(ui, ui.max_rect(), unknown, egui::Color32::WHITE);
        });
        assert!(
            matches!(previews.strips.get(unknown), Some(None)),
            "a family with no bytes is a recorded miss: {:?}",
            previews.strips.keys().collect::<Vec<_>>()
        );
    }

    /// An extreme line box is declined where a long name is only clipped. The input
    /// is arithmetic, not a font: no face with such metrics is in the tree.
    ///
    /// flip: drop the `height > MAX_STRIP_H_PT` term in `strip_px` → fails at "an
    /// extreme line box must be declined".
    #[test]
    fn an_extreme_line_box_is_declined_and_a_long_name_is_only_clipped() {
        // Inter measures 18.15 pt at 15 pt, nowhere near the bound.
        let inter = strip_px(120.0, 18.15, 2.0).expect("an ordinary family previews");
        assert_eq!(inter, (242, 39));

        // 2048 is the line-box ratio the format allows.
        assert_eq!(
            strip_px(120.0, PREVIEW_PT * 2048.0, 2.0),
            None,
            "an extreme line box must be declined"
        );

        // A name three times the cap is clipped, not refused.
        let (w, _) = strip_px(MAX_STRIP_PT * 3.0, 18.15, 1.0).expect("a long name still previews");
        assert_eq!(
            w,
            MAX_STRIP_PT as u32 + 2,
            "a long name is clipped, not refused"
        );

        // The lower end.
        assert_eq!(strip_px(0.0, 18.0, 1.0), None);
        assert_eq!(strip_px(120.0, 0.0, 1.0), None);
    }
}
