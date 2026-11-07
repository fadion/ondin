//! The pixel grid — a one-unit lattice, drawn once the zoom makes it worth seeing.
//!
//! One world unit is one pixel; the grid and `snap::to_pixel` are two views of that
//! lattice and must share a pitch. They are switched separately (*View ▸ Show grid*,
//! *Snap ▸ Snap to grid*): showing is useful only when cells are far enough apart to
//! see, snapping to whole geometry is wanted at every zoom or none.
//!
//! With *Adaptive* on (`CanvasPrefs::grid_adaptive`) the canvas shades the lines
//! into its texture (`ondin_render::grid_ink`); otherwise, and wherever that pass
//! cannot run, [`OndinApp::draw_pixel_grid`] draws them as egui lines. Both place
//! a line by [`ondin_render::grid_ink::on_line`] over [`OndinApp::grid_placed`].

use crate::app::OndinApp;
use eframe::egui;
use ondin_core::kurbo::Point;
use ondin_render::grid_ink::Lattice;

/// The zoom at which the grid appears: 800%, as the zoom readout spells it.
///
/// `camera.zoom` is the readout as a fraction, so this is the threshold the user
/// sees; below it the cells crowd into a grey wash.
pub const VISIBLE_AT_ZOOM: f64 = 8.0;

/// The lattice laid on the canvas texture, and where that texture lands on screen.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Placed {
    pub lattice: Lattice,
    /// Per axis, the device pixel texel 0 shows on: whole, since the texture is
    /// drawn with nearest sampling.
    pub shift: [f64; 2],
    /// The artwork area in points, which an egui line spans.
    pub area: egui::Rect,
}

/// One axis of [`Placed`]: the line origin, the region's ends and the shift, all
/// in texels, for a canvas edge at `edge` points, an artwork span `lo..hi` points
/// and world 0 at `zero` texels.
fn place_axis(
    edge: f32,
    [lo, hi]: [f32; 2],
    zero: f64,
    pitch: f64,
    ppp: f32,
    texels: u32,
) -> (f32, [u32; 2], f64) {
    let ppp = f64::from(ppp);
    // Nearest sampling shows texel x on device pixel x + round(edge × ppp).
    let shift = (f64::from(edge) * ppp).round();
    let origin = (zero + f64::from(edge) * ppp - shift).rem_euclid(pitch);
    // A line is kept when its pixel's centre is inside the area.
    let end = f64::from(texels);
    let from = (f64::from(lo) * ppp - shift - 0.5).ceil().clamp(0.0, end);
    let to = ((f64::from(hi) * ppp - shift - 0.5).floor() + 1.0).clamp(0.0, end);
    (origin as f32, [from as u32, to as u32], shift)
}

impl OndinApp {
    /// Whether the grid should be on screen: switched on, and magnified enough
    /// to be legible.
    pub(crate) fn grid_visible(&self) -> bool {
        self.show_grid && self.session.camera.zoom >= VISIBLE_AT_ZOOM
    }

    /// Where the grid falls on the canvas texture drawn across `rect`, or `None`
    /// while it is hidden or the artwork area is empty.
    pub(crate) fn grid_placed(&self, rect: egui::Rect, ppp: f32) -> Option<Placed> {
        if !self.grid_visible() {
            return None;
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
            return None;
        }
        let zero = self
            .session
            .camera
            .world_to_screen(Point::ZERO, self.canvas_px);
        let pitch = self.session.camera.zoom;
        // The texture's size, as `canvas_ui` sizes it from `rect`.
        let (w, h) = (
            (rect.width() * ppp).round() as u32,
            (rect.height() * ppp).round() as u32,
        );
        let (ox, [x0, x1], sx) = place_axis(
            rect.left(),
            [area.left(), area.right()],
            zero.x,
            pitch,
            ppp,
            w,
        );
        let (oy, [y0, y1], sy) = place_axis(
            rect.top(),
            [area.top(), area.bottom()],
            zero.y,
            pitch,
            ppp,
            h,
        );
        Some(Placed {
            lattice: Lattice {
                origin: [ox, oy],
                pitch: [pitch as f32; 2],
                region: [x0, y0, x1, y1],
            },
            shift: [sx, sy],
            area,
        })
    }

    /// Draw the lattice across `rect` as one-device-pixel egui lines in the
    /// Settings ink: the grid with *Adaptive* off, and its fallback where the
    /// canvas cannot shade it (no GPU, pixel preview). Cells are at least
    /// [`VISIBLE_AT_ZOOM`] device pixels apart, so the line count stays small.
    pub(crate) fn draw_pixel_grid(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        let Some(Placed {
            lattice,
            shift,
            area,
        }) = self.grid_placed(rect, ppp)
        else {
            return;
        };
        // On the pixel the shader shades: the centre of the texel's device pixel.
        let at =
            |texel: u32, shift: f64| ((f64::from(texel) + shift + 0.5) / f64::from(ppp)) as f32;
        let stroke = egui::Stroke::new(
            1.0 / ppp,
            crate::ui::color_to_egui(self.prefs.canvas.grid_ink),
        );
        for x in lattice.columns().map(|t| at(t, shift[0])) {
            painter.line_segment(
                [egui::pos2(x, area.top()), egui::pos2(x, area.bottom())],
                stroke,
            );
        }
        for y in lattice.rows().map(|t| at(t, shift[1])) {
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

    /// Every hairline this module drew, as endpoint pairs, filtered by the
    /// Settings ink.
    fn grid_lines(
        app: &OndinApp,
        ctx: &egui::Context,
        rect: egui::Rect,
        ppp: f32,
    ) -> Vec<[egui::Pos2; 2]> {
        let out = ctx.run_ui(Default::default(), |ui| {
            app.draw_pixel_grid(ui.painter(), rect, ppp);
        });
        let ink = crate::ui::color_to_egui(app.prefs.canvas.grid_ink);
        out.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::LineSegment { points, stroke } if stroke.color == ink => Some(*points),
                _ => None,
            })
            .collect()
    }

    /// The device columns the egui grid drew before it went through `on_line`:
    /// each world unit's screen position put through `rulers::snap_across_axis`
    /// at one device pixel, kept while inside the area. Per axis, the pixel under
    /// each line, the line's exact device position, and whether it was kept.
    fn snapped_columns(
        app: &OndinApp,
        rect: egui::Rect,
        area: egui::Rect,
        ppp: f32,
    ) -> [Vec<(i64, f64, bool)>; 2] {
        let width = 1.0 / ppp;
        let world_min = app.to_world(area.min, rect, ppp);
        let world_max = app.to_world(area.max, rect, ppp);
        let exact = |i: f64, axis: usize| {
            let v = app.session.camera.world_to_screen(
                Point::new(i * (1 - axis) as f64, i * axis as f64),
                app.canvas_px,
            );
            [v.x, v.y][axis] + f64::from([rect.left(), rect.top()][axis]) * f64::from(ppp)
        };
        let mut out = [Vec::new(), Vec::new()];
        for (axis, (lo, hi), (min, max)) in [
            (0, (world_min.x, world_max.x), (area.left(), area.right())),
            (1, (world_min.y, world_max.y), (area.top(), area.bottom())),
        ] {
            for i in (lo.floor() as i64)..=(hi.ceil() as i64) {
                let p = app.to_screen(Point::new(i as f64, i as f64), rect, ppp);
                let v = crate::rulers::snap_across_axis([p.x, p.y][axis], ppp, width);
                let kept = v >= min && v <= max;
                out[axis].push(((v * ppp - 0.5).round() as i64, exact(i as f64, axis), kept));
            }
        }
        out
    }

    /// Over a sweep of zooms, positions, canvas offsets and display scales, the
    /// lattice puts every line on the device pixel `snap_across_axis` put it on,
    /// so the shader and the egui lines draw the grid the egui path always drew.
    /// A line within a thousandth of a pixel of a pixel edge is a tie each path
    /// settles by float rounding, and is skipped; so is a pixel a fractional
    /// canvas edge leaves outside the texture.
    ///
    /// flip: `on_line` rounding with `floor(origin + k × pitch)` → fails at "the
    /// same pixels".
    #[test]
    fn the_lattice_picks_the_pixels_the_snapped_lines_did() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.show_grid = true;
        let (mut compared, mut ties) = (0usize, 0usize);
        for zoom in [8.0, 8.5, 9.3, 12.0, 16.0, 23.7, 32.0, 47.25, 64.0] {
            for centre in [
                (0.0, 0.0),
                (100.37, -40.81),
                (-7.5, 3.125),
                (1234.56, 789.01),
            ] {
                for ppp in [1.0f32, 1.25, 1.5, 1.75, 2.0] {
                    for (left, top) in [(0.0f32, 0.0f32), (37.0, 52.0), (213.4, 61.8)] {
                        for rulers in [false, true] {
                            app.session.camera.zoom = zoom;
                            app.session.camera.center = Point::new(centre.0, centre.1);
                            app.show_rulers = rulers;
                            let rect = egui::Rect::from_min_size(
                                egui::pos2(left, top),
                                egui::vec2(400.0, 300.0),
                            );
                            app.canvas_px = (
                                (rect.width() * ppp).round() as u32,
                                (rect.height() * ppp).round() as u32,
                            );
                            let placed = app.grid_placed(rect, ppp).expect("fixture: visible");
                            let picked = [
                                placed
                                    .lattice
                                    .columns()
                                    .map(|t| i64::from(t) + placed.shift[0] as i64)
                                    .collect::<Vec<_>>(),
                                placed
                                    .lattice
                                    .rows()
                                    .map(|t| i64::from(t) + placed.shift[1] as i64)
                                    .collect::<Vec<_>>(),
                            ];
                            let snapped = snapped_columns(&app, rect, placed.area, ppp);
                            // A canvas edge between device pixels leaves the pixel
                            // it halves to the egui lines alone.
                            let texels = [app.canvas_px.0, app.canvas_px.1];
                            let in_texture = |axis: usize, s: i64| {
                                let t = s - placed.shift[axis] as i64;
                                t >= 0 && t < i64::from(texels[axis])
                            };
                            for axis in 0..2 {
                                // A tie may also fall either side of the area's edge.
                                let tie = |(_, at, _): &(i64, f64, bool)| {
                                    ((at + 0.5) - (at + 0.5).round()).abs() < 1e-3
                                };
                                ties += snapped[axis].iter().filter(|c| tie(c)).count();
                                let want: Vec<i64> = snapped[axis]
                                    .iter()
                                    .filter(|c| c.2 && !tie(c) && in_texture(axis, c.0))
                                    .map(|c| c.0)
                                    .collect();
                                let got: Vec<i64> = picked[axis]
                                    .iter()
                                    .copied()
                                    .filter(|p| {
                                        !snapped[axis]
                                            .iter()
                                            .any(|c| tie(c) && (c.0 - p).abs() <= 1)
                                    })
                                    .collect();
                                compared += want.len();
                                assert_eq!(
                                    got, want,
                                    "the same pixels: axis {axis} at {zoom}×, centre \
                                     {centre:?}, ppp {ppp}, canvas at ({left}, {top}), \
                                     rulers {rulers}"
                                );
                            }
                        }
                    }
                }
            }
        }
        assert!(compared > 10_000, "fixture: a sweep, got {compared} lines");
        assert!(
            ties < compared / 4,
            "most lines are no tie: {ties} of {compared}"
        );
    }

    /// Headless, with *Adaptive* on, the canvas falls back to the egui lines: the
    /// shading pass needs a GPU.
    ///
    /// flip: `draw_canvas` skipping `draw_pixel_grid` whenever `grid_adaptive` is
    /// on → fails at "falls back".
    #[test]
    fn headless_the_adaptive_grid_falls_back_to_egui_lines() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        app.show_grid = true;
        app.show_rulers = false;
        app.session.camera.zoom = 16.0;
        assert!(app.prefs.canvas.grid_adaptive, "fixture: on by default");
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
        let ink = crate::ui::color_to_egui(app.prefs.canvas.grid_ink);
        let mut lines = 0;
        // The first frame teaches the canvas its size.
        for _ in 0..2 {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ui| app.canvas_ui(ui),
            );
            lines = out
                .shapes
                .iter()
                .filter(|c| {
                    matches!(&c.shape, egui::Shape::LineSegment { stroke, .. } if stroke.color == ink)
                })
                .count();
        }
        assert!(lines > 8, "falls back: {lines} lines");
    }

    /// With *Adaptive* off the lines draw in the Settings ink, whatever it is.
    ///
    /// flip: `draw_pixel_grid` stroking in `CanvasPrefs::GRID_INK` → fails at
    /// "in the Settings ink".
    #[test]
    fn the_egui_lines_draw_in_the_settings_ink() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.show_grid = true;
        app.show_rulers = false;
        app.session.camera.zoom = 16.0;
        app.prefs.canvas.grid_adaptive = false;
        app.prefs.canvas.grid_ink = ondin_core::peniko::Color::from_rgba8(217, 70, 143, 128);
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
        assert!(
            grid_lines(&app, &ctx, rect, 1.0).len() > 8,
            "in the Settings ink"
        );
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
    /// the line count barely changes; the leftmost vertical's position does.
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

    /// At the threshold the drawn lattice steps one world unit, which the camera
    /// makes `VISIBLE_AT_ZOOM` device pixels at every display scale.
    ///
    /// flip: stepping either loop by two units in `draw_pixel_grid` → fails at
    /// "a cell is one world unit" on that axis.
    #[test]
    fn a_cell_is_one_world_unit_and_wide_enough_to_read_at_the_threshold() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.show_grid = true;
        app.show_rulers = false;
        app.session.camera.zoom = VISIBLE_AT_ZOOM;
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
        for ppp in [1.0f32, 1.25, 1.5, 2.0] {
            let lines = grid_lines(&app, &ctx, rect, ppp);
            // A vertical line keeps its x; a horizontal one its y.
            for (axis, across) in [("vertical", 0), ("horizontal", 1)] {
                let mut at: Vec<f32> = lines
                    .iter()
                    .filter(|p| (p[0][across] - p[1][across]).abs() < 0.01)
                    .map(|p| p[0][across])
                    .collect();
                at.sort_by(f32::total_cmp);
                assert!(
                    at.len() > 8,
                    "fixture: a {axis} lattice at {ppp}, got {}",
                    at.len()
                );
                for gap in at.windows(2).map(|w| (w[1] - w[0]) * ppp) {
                    assert!(
                        (gap - VISIBLE_AT_ZOOM as f32).abs() < 1e-3,
                        "a cell is one world unit: {axis} {gap} device px at {ppp}"
                    );
                }
            }
        }
    }

    /// Every egui grid line is one device pixel wide and centred on a device
    /// pixel, so it covers that pixel whole, at every display scale.
    ///
    /// flip: `draw_pixel_grid` placing a line at `texel + shift` without the half
    /// → fails at "centred on a device pixel".
    #[test]
    fn an_egui_grid_line_covers_one_whole_device_pixel() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.show_grid = true;
        app.show_rulers = false;
        app.session.camera.zoom = 9.3;
        app.session.camera.center = Point::new(100.37, -40.81);
        for ppp in [1.0f32, 1.25, 1.5, 1.75, 2.0, 3.0] {
            let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
            let lines = grid_lines(&app, &ctx, rect, ppp);
            assert!(lines.len() > 8, "fixture: a lattice at {ppp}");
            for p in lines {
                let across = if (p[0].x - p[1].x).abs() < 0.01 {
                    p[0].x
                } else {
                    p[0].y
                };
                let device = across * ppp - 0.5;
                assert!(
                    (device - device.round()).abs() < 1e-3,
                    "centred on a device pixel: {across} at ppp {ppp}"
                );
            }
        }
    }
}
