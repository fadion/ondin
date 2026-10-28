//! The minimap: a card at the canvas's bottom left that draws the current page
//! flat, with the canvas's viewport as an accent box. A click centres the canvas
//! there, dragging the box pans it, the wheel zooms around the point under the
//! pointer and a double-click fits the page. None of it is an edit: the camera
//! is view state, so no history step and nothing in the file.
//!
//! *View ▸ Show minimap*, `Ctrl+Shift+M`; kept in `prefs.view`, off by default.

use ondin_core::kurbo::{Point, Rect, Vec2};
use ondin_core::{NodeId, NodeKind};

use crate::app::OndinApp;
use crate::theme::color;

/// The card's width; its height follows the page's shape between [`MIN_H`] and
/// [`MAX_H`].
pub(crate) const CARD_W: f32 = 220.0;
const MIN_H: f32 = 90.0;
const MAX_H: f32 = 160.0;
/// The card's padding around the map.
const PAD: f32 = 6.0;
/// From the canvas's left and bottom edges — the status bar's margin, so the two
/// sit on one line.
const MARGIN: f32 = 12.0;
/// Between the card and the status bar beside it.
const GAP: f32 = 8.0;
/// The share of the fitted extent left clear around it, so a box at the edge of
/// the work is not cut by the map's edge.
const FIT_PAD: f64 = 0.04;
/// The map's height when nothing has given it a shape yet: 132pt at a 208pt map.
const FIRST_MAP_H: f32 = 132.0;
/// An image fill reads as a grey box, not as its picture.
const IMAGE_GREY: egui::Color32 = egui::Color32::from_rgb(0x8d, 0x8f, 0x96);
/// A layer that paints nothing of its own and is drawn anyway: an unfilled frame, a
/// container too small to open, or a gradient with no stops.
const UNFILLED: egui::Color32 = crate::theme::color::text_a(28);
/// The smallest a container's shorter side may be on the map, in points, for the
/// walk to open it and draw its contents instead.
const DESCEND: f64 = 6.0;
/// Below this, on its longer side in points, a layer is not drawn at all.
const SMALLEST: f64 = 1.0;
/// How deep the walk goes at most, so a deeply nested file costs a bounded pass.
const MAX_DEPTH: usize = 8;

/// The minimap's state between frames.
#[derive(Default)]
pub(crate) struct Minimap {
    /// The drag in progress, if the primary button went down on the map.
    drag: Option<Drag>,
    /// The map's height last frame, kept so an empty page does not reshape the card.
    map_h: Option<f32>,
    /// The map's rect and placement as last drawn, for the tests to aim at.
    #[cfg(test)]
    seen: Option<(egui::Rect, Fit)>,
}

/// A drag on the map. The map does not refit under the pointer while it runs.
#[derive(Clone, Copy, Debug)]
struct Drag {
    fit: Fit,
    /// The pointer's world point less the camera's centre at the press, so the box
    /// keeps its place under the pointer.
    grab: Vec2,
    /// The camera's centre at the press, where a right-click puts it back.
    from: Point,
    /// The viewport at the press, drawn dashed while the box travels.
    ghost: Rect,
}

/// World ↔ map placement: `scale` map points per world unit, `origin` the map
/// point where world `(0, 0)` lands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Fit {
    scale: f64,
    origin: egui::Pos2,
}

impl Fit {
    /// Fit `world`, padded by [`FIT_PAD`], into `map`, centred on both axes.
    pub(crate) fn new(world: Rect, map: egui::Rect) -> Self {
        let pad = world.width().max(world.height()) * FIT_PAD;
        let world = world.inflate(pad, pad);
        let (w, h) = (f64::from(map.width()), f64::from(map.height()));
        let scale = (w / world.width().max(1e-9)).min(h / world.height().max(1e-9));
        let origin = egui::pos2(
            (f64::from(map.min.x) + (w - world.width() * scale) / 2.0 - world.min_x() * scale)
                as f32,
            (f64::from(map.min.y) + (h - world.height() * scale) / 2.0 - world.min_y() * scale)
                as f32,
        );
        Self { scale, origin }
    }

    pub(crate) fn to_map(self, p: Point) -> egui::Pos2 {
        egui::pos2(
            self.origin.x + (p.x * self.scale) as f32,
            self.origin.y + (p.y * self.scale) as f32,
        )
    }

    pub(crate) fn to_world(self, p: egui::Pos2) -> Point {
        Point::new(
            f64::from(p.x - self.origin.x) / self.scale,
            f64::from(p.y - self.origin.y) / self.scale,
        )
    }

    /// `r` on the map, at least a point each way so a hairline layer still shows.
    fn rect(self, r: Rect) -> egui::Rect {
        let min = self.to_map(r.origin());
        let size = egui::vec2(
            ((r.width() * self.scale) as f32).max(1.0),
            ((r.height() * self.scale) as f32).max(1.0),
        );
        egui::Rect::from_min_size(min, size)
    }
}

/// The map's height for content of `shape`, at the card's width.
fn map_height(shape: Rect) -> f32 {
    let w = CARD_W - 2.0 * PAD;
    let h = (f64::from(w) * shape.height() / shape.width().max(1e-9)) as f32;
    h.clamp(MIN_H - 2.0 * PAD, MAX_H - 2.0 * PAD)
}

/// One box on the map.
struct MapBox {
    bounds: Rect,
    fill: egui::Color32,
    /// A text layer, drawn as a rounded bar through the middle of its bounds
    /// ([`text_bar`]) rather than a block.
    text: bool,
}

/// The bar a text layer draws on the map: the middle [`TEXT_BAR`] of its box's
/// height, at least a point, fully rounded.
fn text_bar(r: egui::Rect) -> (egui::Rect, f32) {
    let h = (r.height() * TEXT_BAR).max(1.0);
    let bar = egui::Rect::from_center_size(r.center(), egui::vec2(r.width(), h));
    (bar, h / 2.0)
}

/// The share of a text layer's height its bar covers.
const TEXT_BAR: f32 = 0.6;

impl OndinApp {
    /// How far right the status bar moves to clear the minimap card: nothing while
    /// it is off.
    pub(crate) fn minimap_clearance(&self) -> f32 {
        if self.show_minimap && self.canvas_rect.is_positive() {
            CARD_W + GAP
        } else {
            0.0
        }
    }

    /// Draw the minimap and act on its input. Placed from `canvas_rect`, which is
    /// last frame's when this runs before the canvas.
    pub(crate) fn minimap_ui(&mut self, ctx: &egui::Context) {
        let canvas = self.canvas_rect;
        if !self.show_minimap || !canvas.is_positive() {
            self.minimap.drag = None;
            return;
        }
        let content = self.document_bounds().filter(|r| r.area() > 0.0);
        let map_h = match content {
            Some(c) => map_height(c),
            None => self.minimap.map_h.unwrap_or(FIRST_MAP_H),
        };
        self.minimap.map_h = Some(map_h);
        let map_size = egui::vec2(CARD_W - 2.0 * PAD, map_h);

        egui::Area::new(egui::Id::new("minimap"))
            .order(egui::Order::Middle)
            .fade_in(false)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(canvas.left_bottom() + egui::vec2(MARGIN, -MARGIN))
            .show(ctx, |ui| {
                crate::ui::menu_frame(PAD).show(ui, |ui| {
                    let (map, resp) =
                        ui.allocate_exact_size(map_size, egui::Sense::click_and_drag());
                    self.minimap_map(ui, map, &resp, content);
                });
            });
    }

    /// The map itself: input first, then paint, so the box drawn is this frame's.
    fn minimap_map(
        &mut self,
        ui: &egui::Ui,
        map: egui::Rect,
        resp: &egui::Response,
        content: Option<Rect>,
    ) {
        let ctx = ui.ctx().clone();
        let view = self.session.camera.viewport(self.canvas_px).view;
        let fit = match self.minimap.drag {
            Some(d) => d.fit,
            None => Fit::new(minimap_extent(content, view), map),
        };

        // A press: inside the box picks it up, outside first centres the box there.
        let pressed = ctx.input(|i| i.pointer.primary_pressed());
        if pressed
            && resp.is_pointer_button_down_on()
            && let Some(p) = ctx.input(|i| i.pointer.interact_pos())
        {
            let at = fit.to_world(p);
            let from = self.session.camera.center;
            let grab = if view.contains(at) {
                at - from
            } else {
                self.session.camera.center = at;
                Vec2::ZERO
            };
            self.minimap.drag = Some(Drag {
                fit,
                grab,
                from,
                ghost: view,
            });
        }
        if let Some(drag) = self.minimap.drag {
            let (down, secondary) = ctx.input(|i| {
                (
                    i.pointer.primary_down(),
                    i.pointer.button_pressed(egui::PointerButton::Secondary),
                )
            });
            if secondary {
                // Right-click abandons a drag, as on the canvas.
                self.session.camera.center = drag.from;
                self.minimap.drag = None;
            } else if !down {
                self.minimap.drag = None;
            } else if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
                self.session.camera.center = drag.fit.to_world(p) - drag.grab;
            }
        }
        if crate::ui::double_clicked(ui, resp) {
            self.zoom_to(self.document_bounds());
        }
        // The wheel zooms, on any axis and with any modifier, around the world point
        // under the pointer.
        if let Some(p) = resp.hover_pos()
            && self.minimap.drag.is_none()
        {
            let (pan, zoom) = crate::canvas::wheel_travel(&ctx, map.height());
            let amount = pan.x + pan.y + zoom;
            if amount != 0.0 {
                let anchor = self
                    .session
                    .camera
                    .world_to_screen(fit.to_world(p), self.canvas_px);
                self.session.camera.zoom_by(
                    crate::canvas::wheel_zoom_factor(amount),
                    anchor,
                    self.canvas_px,
                );
            }
        }

        #[cfg(test)]
        {
            self.minimap.seen = Some((map, fit));
        }

        let view = self.session.camera.viewport(self.canvas_px).view;
        let painter = ui.painter_at(map);
        painter.rect_filled(map, 4.0, color::BG);
        let boxes = self.minimap_boxes(fit.scale);
        for b in &boxes {
            let r = fit.rect(b.bounds);
            if b.text {
                let (bar, round) = text_bar(r);
                painter.rect_filled(bar, round, b.fill);
            } else {
                painter.rect_filled(r, 1.0, b.fill);
            }
        }
        for id in self.session.selection.ids() {
            if let Some(r) = self.session.preview_world_bounds(*id) {
                painter.rect_stroke(
                    fit.rect(r),
                    1.0,
                    egui::Stroke::new(1.0, color::SELECT),
                    egui::StrokeKind::Outside,
                );
            }
        }
        let held = self.minimap.drag.is_some();
        if let Some(drag) = self.minimap.drag {
            let g = fit.rect(drag.ghost);
            let stroke = egui::Stroke::new(1.0, color::ACCENT.gamma_multiply(0.55));
            let corners = [
                g.left_top(),
                g.right_top(),
                g.right_bottom(),
                g.left_bottom(),
            ];
            for i in 0..4 {
                painter.extend(egui::Shape::dashed_line(
                    &[corners[i], corners[(i + 1) % 4]],
                    stroke,
                    3.0,
                    2.0,
                ));
            }
        }
        let vp = fit.rect(view);
        if held {
            painter.rect_stroke(
                vp,
                2.0,
                egui::Stroke::new(3.0, color::ACCENT.gamma_multiply(0.25)),
                egui::StrokeKind::Outside,
            );
        }
        painter.rect_filled(
            vp,
            2.0,
            color::ACCENT.gamma_multiply(if held { 0.2 } else { 0.12 }),
        );
        painter.rect_stroke(
            vp,
            2.0,
            if held {
                egui::Stroke::new(2.0, color::ACCENT_300)
            } else {
                egui::Stroke::new(1.5, color::ACCENT)
            },
            egui::StrokeKind::Inside,
        );

        if held {
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if let Some(p) = resp.hover_pos() {
            if vp.contains(p) {
                ctx.set_cursor_icon(egui::CursorIcon::Grab);
            } else if let Some(name) = self.minimap_frame_at(fit.to_world(p)) {
                // A frame's name, the only text the map shows.
                resp.clone().on_hover_text_at_pointer(name);
            }
        }
    }

    /// The boxes the map draws at `scale` map points per world unit, in paint order.
    ///
    /// The walk goes down through frames and groups while a container is at least
    /// [`DESCEND`] on the map, so detail follows what the map can show rather than a
    /// fixed depth; anything under [`SMALLEST`] is left out. A layout's sections are
    /// often unfilled groups, which draw nothing themselves and show through their
    /// contents.
    fn minimap_boxes(&self, scale: f64) -> Vec<MapBox> {
        let Some(page) = self.session.doc.get(self.session.page) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for &top in page.children() {
            self.walk_minimap(top, 0, None, scale, &mut out);
        }
        out
    }

    /// One layer of [`Self::minimap_boxes`]'s walk, its box cut to `clip`, the box of
    /// the nearest clipping frame above it.
    fn walk_minimap(
        &self,
        id: NodeId,
        depth: usize,
        clip: Option<Rect>,
        scale: f64,
        out: &mut Vec<MapBox>,
    ) {
        // A mask is not artwork: the canvas never paints it as itself.
        let Some(node) = self
            .session
            .display_node(id)
            .filter(|n| n.visible() && !self.session.doc.get(id).is_some_and(|d| d.mask()))
        else {
            return;
        };
        let Some(own) = self.minimap_box_of(id) else {
            return;
        };
        let bounds = match clip {
            Some(c) => own.intersect(c),
            None => own,
        };
        if clip.is_some() && (bounds.width() <= 0.0 || bounds.height() <= 0.0) {
            // Wholly outside the frame that clips it.
            return;
        }
        let (w, h) = (bounds.width() * scale, bounds.height() * scale);
        if w.max(h) < SMALLEST {
            return;
        }
        let kind = node.kind();
        let text = matches!(kind, NodeKind::Text { .. });
        let fill = self.minimap_fill(id);
        let frame = matches!(kind, NodeKind::Artboard { .. });
        let container = frame || matches!(kind, NodeKind::Group);
        let descend = container && depth < MAX_DEPTH && w.min(h) >= DESCEND;
        match fill {
            Some(fill) => out.push(MapBox { bounds, fill, text }),
            // A frame always marks its box, and a container too small to open marks
            // where its contents are.
            None if frame || !descend => out.push(MapBox {
                bounds,
                fill: UNFILLED,
                text,
            }),
            None => {}
        }
        if descend {
            let clip = match self.session.doc.get(id) {
                Some(n) if n.clip() && kind.clips_children() => Some(bounds),
                _ => clip,
            };
            for &kid in self.session.doc.get(id).map_or(&[][..], |n| n.children()) {
                self.walk_minimap(kid, depth + 1, clip, scale, out);
            }
        }
    }

    /// A layer's box on the map: a frame's own rectangle, not its contents' union
    /// (a layer dragged out of it is still its child until the drop), and every
    /// other layer's preview world bounds, so a live drag shows.
    fn minimap_box_of(&self, id: NodeId) -> Option<Rect> {
        let doc = &self.session.doc;
        if matches!(doc.get(id)?.kind(), NodeKind::Artboard { .. }) {
            let world = self.session.preview_world_transform(id)?;
            let local = self.session.preview_local_box(id)?;
            return Some(ondin_core::geometry::transform_rect(world, local));
        }
        self.session.preview_world_bounds(id)
    }

    /// A layer's colour on the map: its topmost visible fill, else its topmost visible
    /// stroke; a bound colour resolved, a gradient as its first stop, a picture as
    /// grey. `None` for a layer that paints neither, a group among them.
    fn minimap_fill(&self, id: NodeId) -> Option<egui::Color32> {
        let doc = &self.session.doc;
        let node = doc.get(id)?;
        let paint = doc.resolved_paint(id, node.paint());
        let brush = paint
            .fills
            .iter()
            .rev()
            .filter(|f| f.visible)
            .find_map(|f| f.brush.literal())
            .or_else(|| {
                paint
                    .strokes
                    .iter()
                    .rev()
                    .filter(|s| s.visible)
                    .find_map(|s| s.brush.literal())
            })?;
        Some(match brush {
            ondin_core::Brush::Solid(c) => crate::ui::color_to_egui(*c),
            ondin_core::Brush::Gradient(g) => g.gradient.stops.first().map_or(UNFILLED, |s| {
                crate::ui::color_to_egui(s.color.to_alpha_color())
            }),
            ondin_core::Brush::Image(_) => IMAGE_GREY,
        })
    }

    /// The name of the topmost top-level frame at `world`, for the map's tooltip.
    fn minimap_frame_at(&self, world: Point) -> Option<String> {
        let doc = &self.session.doc;
        self.root_artboards()
            .into_iter()
            .rev()
            .filter(|id| self.shown_visible(*id))
            .find(|id| self.minimap_box_of(*id).is_some_and(|b| b.contains(world)))
            .and_then(|id| doc.get(id).map(|n| n.name().to_string()))
    }
}

/// What the map fits: the page's content and the viewport together, so the box
/// never leaves the map. An empty page fits the viewport at a size that leaves it
/// room to move.
fn minimap_extent(content: Option<Rect>, view: Rect) -> Rect {
    match content {
        Some(c) => c.union(view),
        None => Rect::from_center_size(view.center(), view.size() * 2.4),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fit keeps both the content and the viewport inside the map, however far
    /// apart they are.
    #[test]
    fn the_fit_keeps_the_work_and_the_viewport_on_the_map() {
        let map = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(208.0, 132.0));
        let work = Rect::new(0.0, 0.0, 3470.0, 2500.0);
        let view = Rect::new(6400.0, 3600.0, 8560.0, 4940.0);
        let fit = Fit::new(minimap_extent(Some(work), view), map);
        for r in [work, view] {
            let m = fit.rect(r);
            assert!(map.contains_rect(m), "{m:?} inside {map:?}");
        }
        // And round trips.
        let p = Point::new(1234.0, 567.0);
        let back = fit.to_world(fit.to_map(p));
        assert!((back - p).hypot() < 1e-2, "{back:?}");
    }

    /// One whole app frame at a fixed window size, with `events`.
    fn frame(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1320.0, 820.0),
            )),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
    }

    fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// A headless app with the minimap on, a frame far to the right of the page's
    /// first so the map has room around the box, the frame selected, and the map
    /// drawn once.
    fn app_with_map(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let page = app.session.doc.first_page();
        let far = app.session.ids.mint();
        let index = app.session.doc.get(page).map_or(0, |p| p.children().len());
        assert!(app.session.commit(ondin_core::Transaction(vec![
            ondin_core::Operation::CreateNode {
                id: far,
                parent: page,
                index,
                kind: NodeKind::Artboard {
                    size: ondin_core::kurbo::Size::new(400.0, 300.0),
                },
                transform: Some(ondin_core::kurbo::Affine::translate((6000.0, 2000.0))),
                name: Some("Far".into()),
            }
        ])));
        app.session.selection.set_one(far);
        app.show_minimap = true;
        for _ in 0..3 {
            frame(ctx, &mut app, Vec::new());
        }
        assert!(app.minimap.seen.is_some(), "fixture: the map is drawn");
        (app, far)
    }

    /// A click on the map away from the box centres the canvas there, and reaches
    /// neither the canvas (the selection stands) nor the document (no step). The
    /// last claim is the control: the same click with the map off deselects.
    ///
    /// No flip in this module reaches "the selection stands": with the `Area` at
    /// `Order::Background` or `interactable(false)`, egui still gives the press to
    /// the map's widget alone.
    #[test]
    fn a_click_on_the_map_centres_the_canvas_there_and_edits_nothing() {
        let ctx = egui::Context::default();
        let (mut app, far) = app_with_map(&ctx);
        let revision = app.session.revision();
        let (map, fit) = app.minimap.seen.expect("drawn");
        let view = app.session.camera.viewport(app.canvas_px).view;
        let p = fit.to_map(Point::new(6200.0, 2150.0));
        assert!(
            map.contains(p) && !view.contains(fit.to_world(p)),
            "fixture: off the box"
        );

        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(p)]);
        frame(&ctx, &mut app, vec![button(p, true)]);
        frame(&ctx, &mut app, vec![button(p, false)]);

        let c = app.session.camera.center;
        assert!(
            (c - Point::new(6200.0, 2150.0)).hypot() < 1.0,
            "centred: {c:?}"
        );
        assert_eq!(app.session.selection.ids(), &[far], "the selection stands");
        assert_eq!(app.session.revision(), revision, "no edit");

        app.show_minimap = false;
        frame(&ctx, &mut app, Vec::new());
        frame(&ctx, &mut app, vec![button(p, true)]);
        frame(&ctx, &mut app, vec![button(p, false)]);
        assert!(
            app.session.selection.is_empty(),
            "control: the same click on the bare canvas deselects"
        );
    }

    /// Dragging the box pans the canvas with it, and the map holds still under the
    /// pointer until the release.
    ///
    /// flip: `minimap_map` refitting during a drag (`fit` always `Fit::new`) →
    /// fails at "the map holds still".
    #[test]
    fn dragging_the_box_pans_the_canvas_and_the_map_holds_still() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_map(&ctx);
        let (_, fit) = app.minimap.seen.expect("drawn");
        let start = app.session.camera.center;
        let p = fit.to_map(start);

        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(p)]);
        frame(&ctx, &mut app, vec![button(p, true)]);
        assert_eq!(app.session.camera.center, start, "picked up, not moved");
        let mut q = p;
        for _ in 0..6 {
            q -= egui::vec2(10.0, 4.0);
            frame(&ctx, &mut app, vec![egui::Event::PointerMoved(q)]);
            assert_eq!(
                app.minimap.seen.expect("drawn").1,
                fit,
                "the map holds still"
            );
        }
        let moved = app.session.camera.center - start;
        let want = Vec2::new(-60.0, -24.0) / fit.scale;
        assert!((moved - want).hypot() < 1.0, "{moved:?} vs {want:?}");
        frame(&ctx, &mut app, vec![button(q, false)]);
        assert!(app.minimap.drag.is_none(), "let go");
    }

    /// The wheel over the map zooms the canvas around the world point under the
    /// pointer: that point keeps its place on the canvas.
    ///
    /// flip: `zoom_by` anchored at the canvas's corner → fails at the last assertion.
    #[test]
    fn the_wheel_over_the_map_zooms_around_the_point_under_it() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_map(&ctx);
        let (_, fit) = app.minimap.seen.expect("drawn");
        let target = Point::new(6200.0, 2150.0);
        let p = fit.to_map(target);
        let (zoom, before) = (
            app.session.camera.zoom,
            app.session.camera.world_to_screen(target, app.canvas_px),
        );

        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(p)]);
        frame(
            &ctx,
            &mut app,
            vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, 60.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            }],
        );

        assert!(app.session.camera.zoom > zoom, "a scroll up zooms in");
        let after = app.session.camera.world_to_screen(target, app.canvas_px);
        assert!((after - before).hypot() < 1e-2, "{after:?} vs {before:?}");
    }

    /// Focus mode keeps the map; the switch off hides it.
    ///
    /// flip: `minimap_ui` called under the frame's `!self.focus` guard → fails at
    /// "drawn in focus mode".
    #[test]
    fn focus_mode_keeps_the_map() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_map(&ctx);
        app.focus = true;
        app.minimap.seen = None;
        frame(&ctx, &mut app, Vec::new());
        assert!(app.minimap.seen.is_some(), "drawn in focus mode");
        app.show_minimap = false;
        app.minimap.seen = None;
        frame(&ctx, &mut app, Vec::new());
        assert!(app.minimap.seen.is_none(), "not drawn when off");
    }

    /// A text layer's bar runs its box's full width through the middle of its
    /// height, fully rounded, and a hairline box still gets a point of bar.
    #[test]
    fn a_text_layer_is_a_bar_through_its_box() {
        let r = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(40.0, 10.0));
        let (bar, round) = text_bar(r);
        assert_eq!(
            (bar.center(), bar.width(), bar.height()),
            (r.center(), 40.0, 6.0)
        );
        assert_eq!(round, 3.0);
        let thin = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(5.0, 1.0));
        assert_eq!(text_bar(thin).0.height(), 1.0);
    }

    /// A frame at (20000, 0), 1000 × 2000, holding an unfilled group around a red
    /// 400 × 200 rect, and a second red rect at x 1200 that overhangs the frame.
    fn app_with_sections(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId, NodeId) {
        use ondin_core::kurbo::{Affine, Size};
        use ondin_core::{Brush, Fill, Operation, Transaction, keyed_by_position};
        let mut app = OndinApp::headless(ctx);
        let page = app.session.doc.first_page();
        let ids = &mut app.session.ids;
        let (frame, group, inner, over) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
        let index = app.session.doc.get(page).map_or(0, |p| p.children().len());
        let rect = |id, parent, index, x: f64| Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Rect {
                size: Size::new(400.0, 200.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((x, 100.0))),
            name: None,
        };
        let red = |id| Operation::SetFills {
            id,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(ondin_core::peniko::Color::from_rgba8(255, 0, 0, 255)).into(),
                visible: true,
            }]),
        };
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: page,
                index,
                kind: NodeKind::Artboard {
                    size: Size::new(1000.0, 2000.0),
                },
                transform: Some(Affine::translate((20_000.0, 0.0))),
                name: Some("Home".into()),
            },
            Operation::CreateNode {
                id: group,
                parent: frame,
                index: 0,
                kind: NodeKind::Group,
                transform: Some(Affine::IDENTITY),
                name: None,
            },
            rect(inner, group, 0, 100.0),
            red(inner),
            rect(over, frame, 1, 1200.0),
            red(over),
        ])));
        (app, frame, group, inner)
    }

    /// The map opens a frame through the unfilled group inside it and draws what the
    /// group holds, not a blank box for the group; a clipping frame cuts off a child
    /// that overhangs it; and mid-drag the frame stays its own rectangle, not widened
    /// by the child being dragged out.
    ///
    /// flip: `minimap_box_of` returning `preview_world_bounds` for a frame → fails
    /// at "mid-drag"; `descend` always false → at "the group's rect is drawn"; the
    /// clipping frame not passed down → at "cut off".
    #[test]
    fn a_frame_opens_through_its_groups_and_keeps_its_own_box() {
        let ctx = egui::Context::default();
        let (mut app, frame, group, inner) = app_with_sections(&ctx);
        let clip = |app: &mut OndinApp, clip| {
            assert!(app.session.commit(ondin_core::Transaction(vec![
                ondin_core::Operation::SetClip { id: frame, clip }
            ])));
        };
        clip(&mut app, false);
        let own = Rect::new(20_000.0, 0.0, 21_000.0, 2000.0);
        let boxes = app.minimap_boxes(0.05);
        assert!(boxes.iter().any(|b| b.bounds == own), "the frame's own box");
        let rect = app.session.preview_world_bounds(inner).expect("bounds");
        assert!(
            boxes
                .iter()
                .any(|b| b.bounds == rect && b.fill == egui::Color32::RED),
            "the group's rect is drawn"
        );
        let group = app.session.preview_world_bounds(group).expect("bounds");
        assert!(
            !boxes
                .iter()
                .any(|b| b.bounds == group && b.fill == UNFILLED),
            "the opened group draws nothing of its own"
        );
        let past = |boxes: &[MapBox]| boxes.iter().any(|b| b.bounds.max_x() > own.max_x());
        assert!(
            past(&boxes),
            "the overhang shows past a frame that does not clip"
        );
        clip(&mut app, true);
        assert!(
            !past(&app.minimap_boxes(0.05)),
            "and is cut off by one that does"
        );

        // A live drag of the group's rect off to the left, not yet dropped: still
        // the frame's child, so the frame's preview bounds take it in.
        app.session.set_preview(&ondin_core::Transaction(vec![
            ondin_core::Operation::SetTransform {
                id: inner,
                transform: ondin_core::kurbo::Affine::translate((-3000.0, 100.0)),
            },
        ]));
        let union = app.session.preview_world_bounds(frame).expect("bounds");
        assert!(
            union.min_x() < own.min_x(),
            "fixture: the drag widens the union"
        );
        assert!(
            app.minimap_boxes(0.05).iter().any(|b| b.bounds == own),
            "the frame's own box, mid-drag"
        );
    }

    /// A frame too small on the map to open is drawn alone, nothing inside it.
    ///
    /// flip: `descend` ignoring `DESCEND` → fails at "not its contents".
    #[test]
    fn a_frame_too_small_to_open_draws_alone() {
        let ctx = egui::Context::default();
        let (app, _, _, inner) = app_with_sections(&ctx);
        let rect = app.session.preview_world_bounds(inner).expect("bounds");
        // 1000 × 2000 at 0.004 is 4 × 8 pt, under `DESCEND`.
        let boxes = app.minimap_boxes(0.004);
        assert!(
            boxes
                .iter()
                .any(|b| b.bounds == Rect::new(20_000.0, 0.0, 21_000.0, 2000.0)),
            "the frame"
        );
        assert!(!boxes.iter().any(|b| b.bounds == rect), "not its contents");
    }

    /// A mask is not drawn on the map as a box of its own.
    ///
    /// flip: `walk_minimap` without its `mask()` test → fails at "no box".
    #[test]
    fn a_mask_is_not_drawn_on_the_map() {
        let ctx = egui::Context::default();
        let (mut app, _, group, inner) = app_with_sections(&ctx);
        let rect = app.session.preview_world_bounds(inner).expect("bounds");
        assert!(
            app.minimap_boxes(0.1).iter().any(|b| b.bounds == rect),
            "fixture: drawn while it is no mask"
        );
        // Something above the mask for it to mask, smaller than it.
        let masked = app.session.ids.mint();
        assert!(app.session.commit(ondin_core::Transaction(vec![
            ondin_core::Operation::CreateNode {
                id: masked,
                parent: group,
                index: 1,
                kind: NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(300.0, 150.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(ondin_core::kurbo::Affine::translate((150.0, 120.0))),
                name: None,
            },
            ondin_core::Operation::SetMask {
                id: inner,
                mask: true,
            },
        ])));
        let boxes: Vec<_> = app.minimap_boxes(0.1).iter().map(|b| b.bounds).collect();
        let masked = app.session.preview_world_bounds(masked).expect("bounds");
        assert!(
            boxes.contains(&masked),
            "fixture: the masked layer is drawn: {boxes:?}"
        );
        assert!(!boxes.contains(&rect), "no box for the mask: {boxes:?}");
    }

    /// The card's height follows the page's shape and stays in 90–160 pt.
    #[test]
    fn the_card_follows_the_page_shape_within_its_bounds() {
        let card = |w, h| map_height(Rect::new(0.0, 0.0, w, h)) + 2.0 * PAD;
        assert_eq!(card(1000.0, 1000.0), MAX_H);
        assert_eq!(card(10_000.0, 10.0), MIN_H);
        let mid = card(1600.0, 1000.0);
        assert!(MIN_H < mid && mid < MAX_H, "{mid}");
    }
}
