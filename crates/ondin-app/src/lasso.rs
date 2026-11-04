//! The lasso: the marquee drawn as a loop instead of a box (`Q`, `ViewSwitch::Lasso`).
//!
//! It is a shape for the marquee, not a tool: it starts wherever a marquee would,
//! takes the same candidates and modifiers, and selects as it goes. A layer is
//! taken when its shape touches the loop's area (`ondin_core::touches_loop`), not
//! its box; in the node tool an anchor inside the loop is (`ondin_core::inside_loop`).

use crate::app::OndinApp;
use crate::preview::Drag;
use crate::theme::color;
use eframe::egui;
use ondin_core::kurbo::Point;

/// The least the pointer moves, in screen pt, before the loop takes another point.
const STEP_PT: f64 = 2.0;

impl OndinApp {
    /// Take the pointer at `world` into a lasso marquee's loop; nothing for a box.
    pub(crate) fn extend_lasso(&mut self, world: Point) {
        let ppw = self.points_per_world();
        if let Drag::Marquee {
            lasso: Some(trail), ..
        } = &mut self.drag
            && trail
                .last()
                .is_none_or(|p| (world - *p).hypot() * ppw >= STEP_PT)
        {
            trail.push(world);
        }
    }

    /// The loop so far: a thin selection-blue line over a faint fill, closed by a
    /// dashed straight run back to its start, which letting go closes.
    pub(crate) fn draw_lasso(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        trail: &[Point],
        end: Point,
    ) {
        let mut pts: Vec<egui::Pos2> = trail
            .iter()
            .map(|p| self.to_screen(*p, rect, ppp))
            .collect();
        pts.push(self.to_screen(end, rect, ppp));
        painter.add(egui::Shape::mesh(fill_spans(&pts, color::MARQUEE_FILL)));
        let stroke = egui::Stroke::new(1.0, color::SELECT);
        painter.add(egui::Shape::line(pts.clone(), stroke));
        if let (Some(first), Some(last)) = (pts.first(), pts.last()) {
            painter.extend(egui::Shape::dashed_line(&[*last, *first], stroke, 3.0, 3.0));
        }
    }
}

/// The loop `pts`' area, read non-zero, as one mesh of point-tall spans. egui fills
/// only a convex path correctly, and a lasso is rarely convex and may cross itself.
fn fill_spans(pts: &[egui::Pos2], fill: egui::Color32) -> egui::Mesh {
    let mut mesh = egui::Mesh::default();
    if pts.len() < 3 {
        return mesh;
    }
    let (top, bottom) = pts
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(t, b), p| {
            (t.min(p.y), b.max(p.y))
        });
    let n = pts.len();
    let mut crossings: Vec<(f32, i32)> = Vec::new();
    let mut y = top.floor();
    while y < bottom {
        // Read at the row's middle; the half-open test counts a vertex on it once.
        let mid = y + 0.5;
        crossings.clear();
        for i in 0..n {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            let dir = match (a.y <= mid, b.y <= mid) {
                (true, false) => 1,
                (false, true) => -1,
                _ => continue,
            };
            let t = (mid - a.y) / (b.y - a.y);
            crossings.push((a.x + t * (b.x - a.x), dir));
        }
        crossings.sort_by(|p, q| p.0.total_cmp(&q.0));
        let mut w = 0;
        for pair in crossings.windows(2) {
            w += pair[0].1;
            if w != 0 {
                let span = egui::Rect::from_min_max(
                    egui::pos2(pair[0].0, y),
                    egui::pos2(pair[1].0, y + 1.0),
                );
                mesh.add_colored_rect(span, fill);
            }
        }
        y += 1.0;
    }
    mesh
}
