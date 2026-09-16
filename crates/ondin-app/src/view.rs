//! Viewport camera: pure screen<->world math for pan/zoom (§9.2).
//!
//! The camera is expressed in DEVICE PIXELS (matching the Vello render target),
//! so the app multiplies egui point coordinates by `pixels_per_point` before
//! calling in. All functions here are pure and unit-tested; the egui glue in
//! `app.rs` only feeds them events.

use ondin_core::kurbo::{Point, Rect, Vec2};
use ondin_render::Viewport;

/// Camera over the infinite canvas: what world point sits at the canvas center,
/// and how many device pixels one world unit occupies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub center: Point,
    pub zoom: f64,
}

impl Default for Camera {
    fn default() -> Self {
        // Centered over a default 800x600 artboard placed at the origin.
        Self {
            center: Point::new(400.0, 300.0),
            zoom: 1.0,
        }
    }
}

/// The closest and furthest the camera may sit, and the only statement of it
/// **inside this crate** (§15 D689).
///
/// ⚠️ **`pub(crate)` because they were private and the range was therefore
/// spelled twice.** `Camera::zoom` is a `pub` field, so [`Camera::zoom_by`] was
/// never the only writer: `OndinApp::zoom_to` — *Fit to page* and *Fit
/// selection* — wrote `zoom.clamp(0.02, 256.0)` with the same two numbers as
/// bare literals, because there was nothing to name. Changing a bound here moved
/// every wheel, chord and menu zoom and left the fit paths on the old one, with
/// every gate green. Assert against these rather than against `256.0`, or a test
/// pins the literal instead of the rule.
///
/// ⚠️ **There is a third reader of the number and `pub(crate)` cannot reach
/// it.** §15 D402 and D403 cost GPU work against *"the app's own `MAX_ZOOM` is
/// 256"*, and the test that exercises that lives in `ondin-render`; `ondin-app`
/// declares only `[[bin]] name = "ondin"` and has **no lib target**, so no other
/// crate can name this constant at any visibility. D689 closed the two
/// statements inside this crate and deliberately does not claim to have closed
/// that one — moving the range into `ondin-core` is what would, and that is a
/// larger question than the duplication was.
pub(crate) const MIN_ZOOM: f64 = 0.02;
pub(crate) const MAX_ZOOM: f64 = 256.0;

impl Camera {
    /// The world-space rectangle visible in a canvas of `pixel_size` device px.
    pub fn viewport(&self, pixel_size: (u32, u32)) -> Viewport {
        let w = pixel_size.0 as f64 / self.zoom;
        let h = pixel_size.1 as f64 / self.zoom;
        Viewport {
            view: Rect::from_center_size(self.center, (w, h)),
            pixel_size,
        }
    }

    /// Map a canvas-local pixel offset (from the canvas top-left) to world space.
    pub fn screen_to_world(&self, screen_px: Vec2, pixel_size: (u32, u32)) -> Point {
        let view = self.viewport(pixel_size).view;
        Point::new(
            view.min_x() + screen_px.x / self.zoom,
            view.min_y() + screen_px.y / self.zoom,
        )
    }

    /// Map a world point to a canvas-local pixel offset.
    pub fn world_to_screen(&self, world: Point, pixel_size: (u32, u32)) -> Vec2 {
        let view = self.viewport(pixel_size).view;
        Vec2::new(
            (world.x - view.min_x()) * self.zoom,
            (world.y - view.min_y()) * self.zoom,
        )
    }

    /// Pan by a device-pixel delta (content follows the cursor).
    pub fn pan_pixels(&mut self, delta_px: Vec2) {
        self.center -= delta_px / self.zoom;
    }

    /// Put the camera at `zoom`, clamped into [`MIN_ZOOM`]`..=`[`MAX_ZOOM`].
    ///
    /// **The one place the range is enforced** (§15 D689). Every writer that is
    /// not setting a known-good constant goes through here, so the bounds have a
    /// single implementation rather than one per caller. It does not close the
    /// `pub` field — a caller can still assign `zoom` directly, and
    /// `Action::ZoomReset` deliberately does, writing `1.0` — but it means a
    /// caller with a *computed* zoom has somewhere to put it that is named after
    /// the rule.
    pub fn set_zoom(&mut self, zoom: f64) {
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    }

    /// Zoom by `factor`, keeping the world point under `anchor_px` fixed on screen.
    pub fn zoom_by(&mut self, factor: f64, anchor_px: Vec2, pixel_size: (u32, u32)) {
        let before = self.screen_to_world(anchor_px, pixel_size);
        self.set_zoom(self.zoom * factor);
        let after = self.screen_to_world(anchor_px, pixel_size);
        self.center += before - after;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PX: (u32, u32) = (800, 600);

    fn approx(a: Point, b: Point) {
        assert!(
            (a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6,
            "{a:?} vs {b:?}"
        );
    }

    #[test]
    fn screen_world_round_trips() {
        let cam = Camera::default();
        let p = Vec2::new(123.0, 456.0);
        let world = cam.screen_to_world(p, PX);
        let back = cam.world_to_screen(world, PX);
        assert!((back.x - p.x).abs() < 1e-6 && (back.y - p.y).abs() < 1e-6);
    }

    #[test]
    fn center_maps_to_canvas_middle() {
        let cam = Camera::default();
        let mid = cam.world_to_screen(cam.center, PX);
        assert!((mid.x - 400.0).abs() < 1e-6 && (mid.y - 300.0).abs() < 1e-6);
    }

    #[test]
    fn zoom_keeps_anchor_world_point_fixed() {
        let mut cam = Camera::default();
        let anchor = Vec2::new(200.0, 150.0);
        let world_before = cam.screen_to_world(anchor, PX);
        cam.zoom_by(2.5, anchor, PX);
        let world_after = cam.screen_to_world(anchor, PX);
        approx(world_before, world_after);
        assert!((cam.zoom - 2.5).abs() < 1e-9);
    }

    #[test]
    fn zoom_is_clamped() {
        let mut cam = Camera::default();
        for _ in 0..100 {
            cam.zoom_by(0.5, Vec2::new(0.0, 0.0), PX);
        }
        assert!(cam.zoom >= MIN_ZOOM);
        for _ in 0..100 {
            cam.zoom_by(2.0, Vec2::new(0.0, 0.0), PX);
        }
        assert!(cam.zoom <= MAX_ZOOM);
    }

    #[test]
    fn pan_shifts_world_under_a_fixed_screen_point() {
        let mut cam = Camera::default();
        let screen = Vec2::new(400.0, 300.0);
        let before = cam.screen_to_world(screen, PX);
        cam.pan_pixels(Vec2::new(100.0, 0.0)); // drag content right by 100px
        let after = cam.screen_to_world(screen, PX);
        // The world point under that screen position moved left by 100px/zoom.
        assert!((after.x - (before.x - 100.0)).abs() < 1e-6);
    }
}
