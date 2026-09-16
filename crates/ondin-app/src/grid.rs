//! The pixel grid — a one-unit lattice, drawn once the zoom makes it worth
//! seeing.
//!
//! **One world unit is one pixel.** That has always been true of the model
//! (`Viewport` derives zoom from the ratio of a world rect to a pixel size, and
//! the design's 51.94% shows 100 units every 51.94px), but nothing depended on
//! it until this grid and `snap::to_pixel` did. They are two views of the same
//! lattice — what the app *shows* and what it *honours* — so if one ever picks a
//! different pitch the other is lying.
//!
//! The grid and snapping to it are separately switchable (*View ▸ Show grid* and
//! *Snap ▸ Snap to grid*), and deliberately so: they answer different questions.
//! Showing it asks "where are the pixel boundaries", which only has a useful
//! answer when they are far enough apart to see; snapping to it asks "should my
//! geometry be whole", which is true at every zoom or none.

use crate::app::OndinApp;
use eframe::egui;
use ondin_core::kurbo::Point;

/// The zoom at which the grid appears: 800%, as the zoom readout spells it.
///
/// `camera.zoom` is device pixels per world unit, and the readout is that same
/// number as a percentage, so this threshold is the one the user sees. It is
/// also the physical answer: below about this, one-pixel cells are closer
/// together than the hairlines drawn between them and the grid reads as a grey
/// wash rather than as a lattice.
pub const VISIBLE_AT_ZOOM: f64 = 8.0;

/// The grid's ink.
///
/// A neutral mid-grey at low alpha rather than a tint of the chrome's near-white
/// text. The grid is the one piece of chrome drawn over *artwork of any
/// lightness* — the canvas ground is document state and may be white, and a
/// frame's own background usually is — so a light hairline would vanish exactly
/// where a pixel grid is most wanted, inside a white frame. Mid-grey survives
/// both directions; a ground-derived ink would only ever solve one of them.
const INK: egui::Color32 = egui::Color32::from_rgba_premultiplied(30, 30, 30, 60);

impl OndinApp {
    /// Whether the grid should be on screen: switched on, and magnified enough
    /// to be legible.
    pub(crate) fn grid_visible(&self) -> bool {
        self.show_grid && self.session.camera.zoom >= VISIBLE_AT_ZOOM
    }

    /// Draw the lattice across `rect`.
    ///
    /// Bounded by construction rather than by a cap: the zoom threshold puts the
    /// cells at least [`VISIBLE_AT_ZOOM`] device pixels apart, so the line count
    /// can never exceed the canvas's pixel width over eight — a few hundred
    /// hairlines at any window size, which is nothing to `epaint`.
    pub(crate) fn draw_pixel_grid(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        if !self.grid_visible() {
            return;
        }
        // Only the artwork area: the ruler bars are chrome of their own and a
        // lattice running under their ticks would make both unreadable.
        let area = if self.rulers_on() {
            egui::Rect::from_min_max(
                egui::pos2(
                    rect.left() + crate::rulers::THICKNESS,
                    rect.top() + crate::rulers::THICKNESS,
                ),
                rect.max,
            )
        } else {
            rect
        };
        if area.width() <= 0.0 || area.height() <= 0.0 {
            return;
        }

        let world_min = self.to_world(area.min, rect, ppp);
        let world_max = self.to_world(area.max, rect, ppp);
        // A one-device-pixel line has to land *on* a pixel rather than straddling
        // two and coming out as a two-pixel smear, and it matters more here than
        // anywhere else in the app: every line is the thinnest thing the screen can
        // draw.
        //
        // ⚠️ **This used to hand-inline the guides' arithmetic and is the call now**
        // (§15 D484, `[S16.5-L3-07]`). The copy was `round() / ppp + half` with the
        // half *unconditional*, and it agreed with `snap_across_axis` at every
        // scale measured — but only because `width` is `1.0 / ppp`, so
        // `floor(width × ppp)` is 1 at every scale and the odd branch is the only
        // one it ever reaches. **Nothing here said that**, and sixteen lines of
        // measured reasoning that have already been rewritten twice were sitting in
        // two places. Changing this width to 2pt would have silently made the copy
        // wrong; through the call it stays right.
        let width = 1.0 / ppp;
        let stroke = egui::Stroke::new(width, INK);

        for i in (world_min.x.floor() as i64)..=(world_max.x.ceil() as i64) {
            let x = crate::rulers::snap_across_axis(
                self.to_screen(Point::new(i as f64, 0.0), rect, ppp).x,
                ppp,
                width,
            );
            if x < area.left() || x > area.right() {
                continue;
            }
            painter.line_segment(
                [egui::pos2(x, area.top()), egui::pos2(x, area.bottom())],
                stroke,
            );
        }
        for i in (world_min.y.floor() as i64)..=(world_max.y.ceil() as i64) {
            let y = crate::rulers::snap_across_axis(
                self.to_screen(Point::new(0.0, i as f64), rect, ppp).y,
                ppp,
                width,
            );
            if y < area.top() || y > area.bottom() {
                continue;
            }
            painter.line_segment(
                [egui::pos2(area.left(), y), egui::pos2(area.right(), y)],
                stroke,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every hairline this module drew, as endpoint pairs.
    ///
    /// Filtered by the grid's own `INK` rather than by shape alone, so nothing
    /// egui happens to emit into the same frame is counted as a grid line.
    fn grid_lines(
        app: &OndinApp,
        ctx: &egui::Context,
        rect: egui::Rect,
        ppp: f32,
    ) -> Vec<[egui::Pos2; 2]> {
        let out = ctx.run_ui(Default::default(), |ui| {
            app.draw_pixel_grid(ui.painter(), rect, ppp);
        });
        out.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::LineSegment { points, stroke } if stroke.color == INK => Some(*points),
                _ => None,
            })
            .collect()
    }

    /// **The threshold is a boundary and both sides of it are asserted**
    /// (`[A5-L6-02]`, §15 D504).
    ///
    /// ⚠️ **Neither decision function in this module had a test caller.** The two
    /// tests below `the_grid_threshold_is_the_zoom_readout_at_800_percent` are
    /// closed over the constant alone — one re-derives `(8.0 × 100).round() ==
    /// 800`, the other is literally `assert!(8.0 >= 8.0)` — so `>=` → `>` and
    /// `&&` → `||` both stayed green, and the grid failing to appear at exactly
    /// 800% is the one number the module's whole doc is about.
    ///
    /// `7.99` and `8.01` bracket it, `8.0` is the case in the name, and
    /// `show_grid = false` is asserted at all three: the switch and the threshold
    /// are separate reasons to be off, and an `||` makes *View ▸ Show grid* stop
    /// switching the grid off above 8×.
    ///
    /// **Two flips run.** `>=` → `>` fails at exactly `8.0`, the predicted site.
    /// `&&` → `||` fails one assertion **earlier** than predicted — at `7.99`
    /// with the switch *on*, not on the switched-off case — because `true || …`
    /// is already wrong there. Both assertions catch it; the earlier one gets
    /// there first, and only running it says which.
    #[test]
    fn the_grid_is_on_at_the_threshold_and_off_below_it_and_off_when_switched_off() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        for (zoom, want) in [(7.99, false), (VISIBLE_AT_ZOOM, true), (8.01, true)] {
            app.session.camera.zoom = zoom;
            app.show_grid = true;
            assert_eq!(
                app.grid_visible(),
                want,
                "at {zoom}× the grid should be {}",
                if want { "on" } else { "off" }
            );
            app.show_grid = false;
            assert!(
                !app.grid_visible(),
                "at {zoom}×, switched off is off — the threshold is not the only reason"
            );
        }
    }

    /// **The lattice stops at the ruler bars** (`[A5-L6-02]`, §15 D504) — the
    /// outcome the code's own comment names, *"a lattice running under their ticks
    /// would make both unreadable"*, and which nothing asserted.
    ///
    /// The rulers-off case is the control and it is the one that gives the
    /// assertion its teeth: deleting the `if self.rulers_on()` branch leaves the
    /// **count** roughly unchanged, so a test that only counted lines would pass.
    /// What separates the two is where the leftmost vertical sits.
    ///
    /// ⚠️ **`present` mode turns the rulers off**, so `rulers_on()` is
    /// `show_rulers && !present` and the fixture sets both — a test that set only
    /// `show_rulers` would be asserting the inset on an app that might not have
    /// one.
    ///
    /// **Flip run**, the `rulers_on()` branch deleted and `rect` used
    /// unconditionally: fails on *"the first vertical clears the ruler bar"* — the
    /// predicted site — at **1.5 against 20**, the leftmost line back at the
    /// canvas edge and well inside the bar.
    #[test]
    fn the_lattice_does_not_run_under_the_ruler_bars() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.show_grid = true;
        app.session.camera.zoom = 16.0;
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
        let ppp = 1.0;

        app.show_rulers = false;
        app.present = false;
        let without = grid_lines(&app, &ctx, rect, ppp);
        assert!(
            without.len() > 8,
            "fixture: at 16× a 400pt canvas is drawn as a lattice, got {}",
            without.len()
        );

        app.show_rulers = true;
        let with = grid_lines(&app, &ctx, rect, ppp);
        let vertical_left = |v: &[[egui::Pos2; 2]]| {
            v.iter()
                .filter(|p| (p[0].x - p[1].x).abs() < 0.01)
                .map(|p| p[0].x)
                .fold(f32::MAX, f32::min)
        };
        assert!(
            vertical_left(&with) >= rect.left() + crate::rulers::THICKNESS,
            "the first vertical clears the ruler bar: {} against {}",
            vertical_left(&with),
            rect.left() + crate::rulers::THICKNESS
        );
        assert!(
            vertical_left(&without) < rect.left() + crate::rulers::THICKNESS,
            "control: with the bars off it does not, or the assertion above is \
             about nothing"
        );
    }

    /// The threshold is the number the zoom readout shows, so "shows at 800%"
    /// means the readout saying 800 — not 8, and not 80000.
    #[test]
    fn the_grid_threshold_is_the_zoom_readout_at_800_percent() {
        // The readout is `(camera.zoom * 100).round()`, as `zoom_control` builds
        // it. Pinned here because the two have to agree for the menu row and the
        // percentage in the bar to tell the same story.
        let readout = |zoom: f64| (zoom * 100.0).round() as i32;
        assert_eq!(readout(VISIBLE_AT_ZOOM), 800);
    }

    /// One pixel cell is at least 8 device pixels across once the grid appears,
    /// which is what makes a hairline lattice legible rather than a grey wash.
    #[test]
    fn a_cell_is_wide_enough_to_read_at_the_threshold() {
        let device_px_per_cell = VISIBLE_AT_ZOOM;
        assert!(device_px_per_cell >= 8.0);
        // And at 150% display scaling it is still over five points, so the lines
        // do not merge on a scaled display either.
        assert!(device_px_per_cell / 1.5 >= 5.0);
    }

    /// **The snap adapts to the width it is given, which is why calling it is safe
    /// where the hand-inlined copy was only accidentally right** (§15 D484,
    /// `[S16.5-L3-07]`).
    ///
    /// `draw_pixel_grid`'s line is `1.0 / ppp` **points** wide — one device pixel —
    /// so `floor(width × ppp)` is 1 at every scale and `rulers::snap_across_axis`
    /// only ever reaches its odd branch. That is why the old
    /// `round() / ppp + 0.5 / ppp`, with the half **unconditional**, agreed with it
    /// everywhere. It is a fact about the *width*, and nothing in this file said
    /// so: changing the grid to a 2pt line would have made the copy silently wrong.
    ///
    /// **So the assertion is over two widths, not one.** One device pixel wants a
    /// pixel centre and two want a pixel edge, and the property that has to hold
    /// for both is that the ink covers whole pixels.
    ///
    /// ⚠️ **This test was written over one width first and a flip proved it
    /// vacuous.** With only `1.0 / ppp` in the loop, changing the width to
    /// `2.0 / ppp` left it **green** — because `snap_across_axis` handles the even
    /// case correctly, so the *right* implementation passes at any width. The test
    /// was pinning the callee's contract, which `rulers::guide_geometry_tests`
    /// already pins, and said nothing about the merge. The two widths are what make
    /// the difference visible.
    ///
    /// ⚠️ **Flip-check, run: the unconditional `+ 0.5 / ppp` this file used to
    /// carry, in place of the call.** Passes at one device pixel — the branch it
    /// was accidentally right on — and fails at two with a half-pixel overhang at
    /// every scale. That is exactly the latent hazard the finding names.
    #[test]
    fn a_pixel_grid_line_covers_whole_pixels_at_either_width() {
        for ppp in [1.0f32, 1.25, 1.5, 1.75, 2.0, 3.0] {
            for device_px in [1.0f32, 2.0] {
                let width = device_px / ppp;
                // A deliberately fractional screen coordinate: an integral one is
                // the case any arithmetic gets right.
                let at = crate::rulers::snap_across_axis(100.37, ppp, width);
                let centre = at * ppp;
                let (lo, hi) = (centre - width * ppp / 2.0, centre + width * ppp / 2.0);
                let whole = (lo - lo.round()).abs() < 1e-4 && (hi - hi.round()).abs() < 1e-4;
                assert!(
                    whole,
                    "at ppp {ppp}, a {device_px}-device-pixel line spans {lo}..{hi} \
                     in device space, which is not a whole number of pixels"
                );
            }
        }
    }
}
