//! The pixel grid — a one-unit lattice, drawn once the zoom makes it worth seeing.
//!
//! One world unit is one pixel; the grid and `snap::to_pixel` are two views of that
//! lattice and must share a pitch. They are switched separately (*View ▸ Show grid*,
//! *Snap ▸ Snap to grid*): showing is useful only when cells are far enough apart to
//! see, snapping to whole geometry is wanted at every zoom or none.

use crate::app::OndinApp;
use eframe::egui;
use ondin_core::kurbo::Point;

/// The zoom at which the grid appears: 800%, as the zoom readout spells it.
///
/// `camera.zoom` is the readout as a fraction, so this is the threshold the user
/// sees; below it the cells crowd into a grey wash.
pub const VISIBLE_AT_ZOOM: f64 = 8.0;

/// The grid's ink: a neutral mid-grey at low alpha, because the grid is drawn over
/// artwork of any lightness (a white frame is where it is most wanted) and a light
/// or ground-derived ink would vanish in one direction.
const INK: egui::Color32 = egui::Color32::from_rgba_premultiplied(30, 30, 30, 60);

impl OndinApp {
    /// Whether the grid should be on screen: switched on, and magnified enough
    /// to be legible.
    pub(crate) fn grid_visible(&self) -> bool {
        self.show_grid && self.session.camera.zoom >= VISIBLE_AT_ZOOM
    }

    /// Draw the lattice across `rect`. Bounded by the zoom threshold: cells are at
    /// least [`VISIBLE_AT_ZOOM`] device pixels apart, so the line count stays small.
    pub(crate) fn draw_pixel_grid(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        if !self.grid_visible() {
            return;
        }
        // Only the artwork area; a lattice under the ruler ticks would hide both.
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
        // A one-device-pixel line must land on a pixel, not straddle two; the guides'
        // `snap_across_axis` stays right if this width ever changes.
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

    /// Every hairline this module drew, as endpoint pairs, filtered by `INK`.
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

    /// Both sides of the threshold are asserted, with `show_grid = false` at each: the
    /// switch and the threshold are separate reasons to be off.
    ///
    /// flip: `>=` → `>` in `grid_visible` → fails at exactly 8.0; `&&` → `||` →
    /// fails at 7.99 with the switch on.
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

    /// The lattice stops at the ruler bars. The rulers-off control matters because
    /// the line count barely changes; the leftmost vertical's position does. The
    /// fixture sets `present` too, since `rulers_on()` is `show_rulers && !present`.
    ///
    /// flip: delete the `rulers_on()` branch in `draw_pixel_grid` → fails at "the
    /// first vertical clears the ruler bar".
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

    /// The threshold is what the zoom readout shows: 800, not 8 or 80000.
    #[test]
    fn the_grid_threshold_is_the_zoom_readout_at_800_percent() {
        // The readout is `(camera.zoom * 100).round()`, as `zoom_control` builds it.
        let readout = |zoom: f64| (zoom * 100.0).round() as i32;
        assert_eq!(readout(VISIBLE_AT_ZOOM), 800);
    }

    /// A cell is at least 8 device pixels across once the grid appears.
    #[test]
    fn a_cell_is_wide_enough_to_read_at_the_threshold() {
        let device_px_per_cell = VISIBLE_AT_ZOOM;
        assert!(device_px_per_cell >= 8.0);
        // At 150% display scaling it is still over five points.
        assert!(device_px_per_cell / 1.5 >= 5.0);
    }

    /// `rulers::snap_across_axis` covers whole pixels at one device pixel (wants a
    /// pixel centre) and at two (a pixel edge), so the grid may call it at either width.
    #[test]
    fn a_pixel_grid_line_covers_whole_pixels_at_either_width() {
        for ppp in [1.0f32, 1.25, 1.5, 1.75, 2.0, 3.0] {
            for device_px in [1.0f32, 2.0] {
                let width = device_px / ppp;
                // Fractional on purpose: an integral coordinate is right for any arithmetic.
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
