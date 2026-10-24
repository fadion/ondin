//! Present mode: the document's top-level frames shown one at a time, fitted to
//! a fullscreen window, with nothing of the editor around them and nothing to
//! edit with. Its own branch of `OndinApp::ui` draws and reads the keyboard, so no
//! overlay or command needs to know it exists. Focus mode, which hides the panels
//! while editing goes on, is a different thing.

use crate::view::Camera;
use ondin_core::NodeId;
use ondin_core::kurbo::{Point, Rect, Vec2};

/// A presentation in progress.
#[derive(Clone, Debug)]
pub(crate) struct Presenting {
    /// The frames shown, in reading order ([`reading_order`]).
    pub frames: Vec<NodeId>,
    /// Which of them is on screen.
    pub at: usize,
    /// Whether the window was fullscreen before, so leaving puts it back.
    pub was_fullscreen: bool,
    /// When the frame last changed, in egui's clock — what times the counter's fade.
    pub stepped_at: f64,
    /// The view zoomed in on the frame (`Ctrl`+wheel), or `None` while it is
    /// fitted whole. Kept across steps: frames presented together are usually
    /// one size.
    pub view: Option<Camera>,
    /// Set by a step that moves: the next frame drawn opens at the new frame's
    /// top, centred across, at the zoom `view` carries.
    pub to_top: bool,
}

impl Presenting {
    /// Move by `by` frames, stopping at either end. Whether it moved.
    pub fn step(&mut self, by: isize, now: f64) -> bool {
        let last = self.frames.len().saturating_sub(1) as isize;
        let next = (self.at as isize + by).clamp(0, last) as usize;
        self.go(next, now)
    }

    /// Go to the first (`false`) or last (`true`) frame.
    pub fn jump(&mut self, to_end: bool, now: f64) {
        let to = if to_end {
            self.frames.len().saturating_sub(1)
        } else {
            0
        };
        self.go(to, now);
    }

    fn go(&mut self, to: usize, now: f64) -> bool {
        let moved = to != self.at;
        self.to_top |= moved;
        self.at = to;
        self.stepped_at = now;
        moved
    }
}

/// Frames as a page is read: rows from top to bottom, each left to right. A frame
/// joins the row above while its top is above that row's first frame's middle,
/// so a row of frames a few units out of line stays one row.
pub(crate) fn reading_order(mut frames: Vec<(NodeId, Rect)>) -> Vec<NodeId> {
    frames.sort_by(|a, b| a.1.y0.total_cmp(&b.1.y0).then(a.1.x0.total_cmp(&b.1.x0)));
    let mut rows: Vec<Vec<(NodeId, Rect)>> = Vec::new();
    for f in frames {
        match rows.last_mut() {
            Some(row) if f.1.y0 < row[0].1.center().y => row.push(f),
            _ => rows.push(vec![f]),
        }
    }
    rows.into_iter()
        .flat_map(|mut row| {
            row.sort_by(|a, b| a.1.x0.total_cmp(&b.1.x0));
            row.into_iter().map(|(id, _)| id)
        })
        .collect()
}

/// The zoom that fits `frame` whole into a canvas of `px` device pixels — no
/// margin: what is outside it is covered by the matte. Within the editor's zoom
/// range, which the renderer's budgets are measured against.
pub(crate) fn fit_zoom(frame: Rect, px: (u32, u32)) -> f64 {
    let (w, h) = (frame.width().max(1.0), frame.height().max(1.0));
    (f64::from(px.0) / w)
        .min(f64::from(px.1) / h)
        .clamp(crate::view::MIN_ZOOM, crate::view::MAX_ZOOM)
}

/// Hold a zoomed view on `frame`: zoomed out to the fit or past it is the fit
/// (`None`); otherwise, on each axis, the frame stays centred while it is
/// narrower than the screen, and its edges stay outside the screen's once it is
/// wider — so a tall frame zooms about its middle and the wheel scrolls it.
pub(crate) fn hold_on_frame(view: Camera, frame: Rect, px: (u32, u32)) -> Option<Camera> {
    if view.zoom <= fit_zoom(frame, px) {
        return None;
    }
    let axis = |at: f64, lo: f64, hi: f64, screen: u32| {
        let half = f64::from(screen) / view.zoom / 2.0;
        if hi - lo <= 2.0 * half {
            (lo + hi) / 2.0
        } else {
            at.clamp(lo + half, hi - half)
        }
    };
    Some(Camera {
        center: Point::new(
            axis(view.center.x, frame.x0, frame.x1, px.0),
            axis(view.center.y, frame.y0, frame.y1, px.1),
        ),
        zoom: view.zoom,
    })
}

/// How long the frame counter stays after a step, and how long it then takes to
/// fade, in seconds.
pub(crate) const COUNTER_HOLD: f64 = 1.5;
pub(crate) const COUNTER_FADE: f64 = 0.4;

/// The counter's opacity `since` seconds after a step: whole while held, then
/// fading to nothing.
pub(crate) fn counter_alpha(since: f64) -> f32 {
    if since <= COUNTER_HOLD {
        1.0
    } else {
        (1.0 - (since - COUNTER_HOLD) / COUNTER_FADE).clamp(0.0, 1.0) as f32
    }
}

impl crate::app::OndinApp {
    /// The visible top-level frames with their world boxes, in no order.
    pub(crate) fn presentable(&self) -> Vec<(NodeId, Rect)> {
        let doc = &self.session.doc;
        self.root_artboards()
            .into_iter()
            .filter(|id| doc.get(*id).is_some_and(|n| n.visible()))
            .filter_map(|id| Some((id, self.session.resolved.world_bounds(id)?)))
            .collect()
    }

    /// Start presenting (`Ctrl+Alt+Enter`, the View menu, the palette): from the
    /// frame holding the selection, else the one most in view, else the first.
    pub(crate) fn start_presenting(&mut self, ctx: &egui::Context) {
        let found = self.presentable();
        if found.is_empty() {
            self.session.info("No frames to present");
            return;
        }
        self.finish_text_first();
        let doc = &self.session.doc;
        let frames = reading_order(found.clone());
        let holding = self.session.selection.ids().iter().find_map(|&id| {
            let mut at = id;
            loop {
                if frames.contains(&at) {
                    return Some(at);
                }
                at = doc.get(at)?.parent()?;
            }
        });
        let view = self.session.camera.viewport(self.canvas_px).view;
        let most_in_view = found
            .iter()
            .map(|(id, b)| (*id, b.intersect(view).area()))
            .filter(|(_, a)| *a > 0.0)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id);
        let start = holding.or(most_in_view).unwrap_or(frames[0]);
        let at = frames.iter().position(|f| *f == start).unwrap_or(0);
        self.context_menu = None;
        self.palette = None;
        let was_fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        self.presenting = Some(Presenting {
            frames,
            at,
            was_fullscreen,
            stepped_at: ctx.input(|i| i.time),
            view: None,
            to_top: false,
        });
    }

    /// Leave present mode, the window as it was found.
    pub(crate) fn stop_presenting(&mut self, ctx: &egui::Context) {
        if let Some(p) = self.presenting.take()
            && !p.was_fullscreen
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
    }

    /// Present mode's whole frame: its keys, the frame on screen fitted to the
    /// window over a matte of the canvas colour, and a counter after each step.
    ///
    /// → ↓ Space Page Down Enter and a click step on; ← ↑ Page Up Backspace and a
    /// right-click step back; Home and End jump; Escape leaves. `Ctrl`+wheel
    /// zooms in on the frame and the wheel then scrolls it ([`hold_on_frame`]).
    /// Nothing else is read, so no command can edit what nobody can see being
    /// edited.
    pub(crate) fn present_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click());
        let (next, back, home, end, leave) = ctx.input(|i| {
            let any = |keys: &[egui::Key]| keys.iter().any(|k| i.key_pressed(*k));
            use egui::Key::*;
            (
                any(&[ArrowRight, ArrowDown, Space, PageDown, Enter]),
                any(&[ArrowLeft, ArrowUp, PageUp, Backspace]),
                i.key_pressed(Home),
                i.key_pressed(End),
                i.key_pressed(Escape),
            )
        });
        if leave {
            self.stop_presenting(&ctx);
            return;
        }
        let Some(p) = self.presenting.as_mut() else {
            return;
        };
        if next || resp.clicked() {
            p.step(1, now);
        }
        if back || resp.secondary_clicked() {
            p.step(-1, now);
        }
        if home || end {
            p.jump(end, now);
        }
        let (frame, at, total, since) = (p.frames[p.at], p.at, p.frames.len(), now - p.stepped_at);

        let ppp = ctx.pixels_per_point();
        let px = (
            ((rect.width() * ppp).round() as u32).max(1),
            ((rect.height() * ppp).round() as u32).max(1),
        );
        let ground = self.session.preview_canvas_background();
        let matte = egui::Color32::from_rgba_unmultiplied(
            (ground.components[0] * 255.0) as u8,
            (ground.components[1] * 255.0) as u8,
            (ground.components[2] * 255.0) as u8,
            255,
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, matte);
        // A frame deleted under us (an autosave cannot, but a recovery could) ends it.
        let Some(bounds) = self.session.resolved.world_bounds(frame) else {
            self.stop_presenting(&ctx);
            return;
        };
        let fitted = Camera {
            center: bounds.center(),
            zoom: fit_zoom(bounds, px),
        };
        let (pan, zoom) = crate::canvas::wheel_travel(&ctx, rect.height());
        let Some(p) = self.presenting.as_mut() else {
            return;
        };
        let mut view = p.view.unwrap_or(fitted);
        if std::mem::take(&mut p.to_top) {
            let top = Point::new(bounds.center().x, f64::NEG_INFINITY);
            view = hold_on_frame(
                Camera {
                    center: top,
                    ..view
                },
                bounds,
                px,
            )
            .unwrap_or(fitted);
        }
        view.pan_pixels(Vec2::new(f64::from(pan.x * ppp), f64::from(pan.y * ppp)));
        if zoom != 0.0 {
            let at = ctx.pointer_hover_pos().unwrap_or(rect.center()) - rect.min;
            let at = Vec2::new(f64::from(at.x * ppp), f64::from(at.y * ppp));
            view.zoom_by(crate::canvas::wheel_zoom_factor(zoom), at, px);
        }
        // Every frame, not only on a wheel: a window resize changes the fit.
        p.view = hold_on_frame(view, bounds, px);
        let camera = p.view.unwrap_or(fitted);
        self.session.set_chrome_ghost(None);
        let (doc, res, overrides) = self.session.render_inputs();
        if let Some(tex) = self.canvas.render(doc, res, overrides, &camera, px, ground) {
            painter.image(
                tex,
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        if self.canvas.images().wants_frame() {
            ctx.request_repaint();
        }
        // The matte over whatever lies outside the frame: its neighbours, and its
        // own children overflowing an unclipped edge.
        let to_screen = |x: f64, y: f64| {
            let v = camera.world_to_screen(ondin_core::kurbo::Point::new(x, y), px);
            rect.min + egui::vec2(v.x as f32 / ppp, v.y as f32 / ppp)
        };
        // Within the screen: zoomed in, the frame runs past it.
        let shown = egui::Rect::from_min_max(
            to_screen(bounds.x0, bounds.y0),
            to_screen(bounds.x1, bounds.y1),
        )
        .intersect(rect);
        for r in [
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, shown.min.y)),
            egui::Rect::from_min_max(egui::pos2(rect.min.x, shown.max.y), rect.max),
            egui::Rect::from_min_max(
                egui::pos2(rect.min.x, shown.min.y),
                egui::pos2(shown.min.x, shown.max.y),
            ),
            egui::Rect::from_min_max(
                egui::pos2(shown.max.x, shown.min.y),
                egui::pos2(rect.max.x, shown.max.y),
            ),
        ] {
            painter.rect_filled(r, 0.0, matte);
        }

        let alpha = counter_alpha(since);
        if alpha > 0.0 {
            let name = self
                .session
                .doc
                .get(frame)
                .map(|n| n.name().to_string())
                .unwrap_or_default();
            counter(
                &painter,
                rect,
                &format!("{name}   {} / {total}", at + 1),
                alpha,
            );
            ctx.request_repaint();
        }
    }
}

/// The counter pill at the bottom of the screen.
fn counter(painter: &egui::Painter, screen: egui::Rect, text: &str, alpha: f32) {
    let galley = painter.layout_no_wrap(
        text.to_string(),
        egui::FontId::proportional(12.0),
        crate::theme::text::STRONG.gamma_multiply(alpha),
    );
    let size = galley.size() + egui::vec2(COUNTER_PAD_X * 2.0, COUNTER_PAD_Y * 2.0);
    let pill = egui::Rect::from_center_size(
        egui::pos2(
            screen.center().x,
            screen.max.y - COUNTER_BOTTOM - size.y / 2.0,
        ),
        size,
    );
    painter.rect_filled(
        pill,
        egui::CornerRadius::same((size.y / 2.0) as u8),
        egui::Color32::from_black_alpha((180.0 * alpha) as u8),
    );
    painter.galley(
        pill.min + egui::vec2(COUNTER_PAD_X, COUNTER_PAD_Y),
        galley,
        egui::Color32::WHITE,
    );
}

/// The counter pill's padding and its distance from the screen's bottom edge.
const COUNTER_PAD_X: f32 = 12.0;
const COUNTER_PAD_Y: f32 = 6.0;
const COUNTER_BOTTOM: f32 = 24.0;

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::IdSource;

    /// Rows read top to bottom and each left to right; a frame a little lower
    /// than its neighbour stays in its row.
    ///
    /// flip: `reading_order` sorting by top alone → fails at the first row
    /// (`b` sits 10 lower than `a` but left of it).
    #[test]
    fn frames_are_read_in_rows() {
        let mut ids = IdSource::new(0x9A);
        let [a, b, c, d] = [(); 4].map(|_| ids.mint());
        let r = |x: f64, y: f64| Rect::new(x, y, x + 100.0, y + 80.0);
        let order = reading_order(vec![
            (a, r(200.0, 0.0)),
            (b, r(0.0, 10.0)),
            (c, r(100.0, 200.0)),
            (d, r(0.0, 200.0)),
        ]);
        assert_eq!(order, vec![b, a, d, c]);
    }

    /// Stepping stops at both ends and says whether it moved; a step that moves
    /// sends the view to the new frame's top, one that stops at an end leaves
    /// it where it is.
    #[test]
    fn stepping_stops_at_the_ends() {
        let mut ids = IdSource::new(0x9B);
        let mut p = Presenting {
            frames: vec![ids.mint(), ids.mint()],
            at: 0,
            was_fullscreen: false,
            stepped_at: 0.0,
            view: None,
            to_top: false,
        };
        assert!(!p.step(-1, 1.0), "nothing before the first");
        assert!(!p.to_top, "and the view stays where it was scrolled");
        assert!(p.step(1, 2.0));
        assert!(p.to_top, "a new frame opens at its top");
        p.to_top = false;
        assert!(!p.step(1, 3.0), "nothing after the last");
        assert_eq!(p.at, 1);
        p.jump(false, 4.0);
        assert_eq!(p.at, 0);
        assert!(p.to_top, "a jump too");
    }

    /// A zoomed view of a tall frame: centred across while it is narrower than
    /// the screen, its edges kept off-screen along it, and at or past the fit
    /// back to the fit.
    ///
    /// flip: `hold_on_frame` keeping the centre where the frame fits, rather
    /// than centring → fails at "centred across".
    #[test]
    fn a_zoomed_view_holds_on_the_frame() {
        let tall = Rect::new(0.0, 0.0, 400.0, 2000.0);
        let px = (1600, 1000);
        assert!(
            (fit_zoom(tall, px) - 0.5).abs() < 1e-9,
            "fixture: fitted by height"
        );
        let at = |x: f64, y: f64, zoom: f64| {
            hold_on_frame(
                Camera {
                    center: Point::new(x, y),
                    zoom,
                },
                tall,
                px,
            )
        };

        let held = at(-300.0, -50.0, 2.0).expect("zoomed in");
        assert_eq!(held.center.x, 200.0, "centred across: 800 px wide on 1600");
        assert_eq!(held.center.y, 250.0, "the top edge held at the screen's");
        let held = at(0.0, 1990.0, 2.0).expect("zoomed in");
        assert_eq!(held.center.y, 1750.0, "the bottom edge too");
        let held = at(-300.0, 1000.0, 5.0).expect("zoomed in");
        assert_eq!(
            held.center.x, 160.0,
            "wider than the screen: edges held across"
        );

        assert!(at(200.0, 1000.0, 0.5).is_none(), "the fit is the fit");
        assert!(at(200.0, 1000.0, 0.2).is_none(), "and past it");
    }

    /// A frame twice as wide as tall fits a square canvas by its width.
    #[test]
    fn the_fit_is_the_tighter_axis() {
        let z = fit_zoom(Rect::new(0.0, 0.0, 400.0, 200.0), (800, 800));
        assert!((z - 2.0).abs() < 1e-9, "{z}");
    }

    #[test]
    fn the_counter_holds_then_fades() {
        assert_eq!(counter_alpha(0.0), 1.0);
        assert_eq!(counter_alpha(COUNTER_HOLD), 1.0);
        assert!(counter_alpha(COUNTER_HOLD + COUNTER_FADE / 2.0) < 1.0);
        assert!(counter_alpha(COUNTER_HOLD + COUNTER_FADE) < 1e-6);
        assert_eq!(counter_alpha(COUNTER_HOLD + COUNTER_FADE + 1.0), 0.0);
    }
}
