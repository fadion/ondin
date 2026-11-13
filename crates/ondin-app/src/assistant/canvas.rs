//! The assistant on the canvas: the ask button over the selection, and while
//! the panel is open a mark on each attached layer.

use crate::app::OndinApp;
use crate::preview::Drag;
use crate::theme::{self, color, icon};
use ondin_core::NodeId;
use ondin_core::kurbo::{Point, Rect};

/// The ask button's side, the same at every zoom.
pub(crate) const ASK: f32 = 26.0;
/// Clear air between the button and the selection's top edge.
const ASK_GAP: f32 = 8.0;
/// How far outside its layer an attached layer's mark runs.
const MARK_OUT: f32 = 5.0;
/// The mark's tab.
const TAB_H: f32 = 18.0;

impl OndinApp {
    /// A world rect on screen.
    fn screen_rect(&self, r: Rect, ppp: f32) -> egui::Rect {
        let a = self.to_screen(Point::new(r.x0, r.y0), self.canvas_rect, ppp);
        let b = self.to_screen(Point::new(r.x1, r.y1), self.canvas_rect, ppp);
        egui::Rect::from_two_pos(a, b)
    }

    /// Whether the ask button shows: a selection, the assistant on, and nothing
    /// the button would get in the way of.
    fn ask_shows(&self, ctx: &egui::Context) -> bool {
        self.show_ask
            && self.prefs.assistant.harness().is_some()
            && !self.prefs.private
            && self.presenting.is_none()
            && !self.focus
            && !self.session.selection.is_empty()
            && self.text.is_none()
            && self.edited_path().is_none()
            && matches!(self.drag, Drag::None)
            && ctx.dragged_id().is_none()
            && !self.session.has_gesture_preview()
    }

    /// The ask button's place for selection bounds `sel` on screen: its right
    /// edge on the selection's, its bottom [`ASK_GAP`] above it; inside the
    /// corner where there is no room above; left of the inspector's cards.
    fn ask_button_place(&self, sel: egui::Rect, screen: egui::Rect) -> egui::Rect {
        let column = Self::inspector_column(screen);
        let right = sel.right().min(column.left() - ASK_GAP);
        let mut top = sel.top() - ASK_GAP - ASK;
        if top < self.canvas_rect.top() + ASK_GAP {
            top = sel.top() + ASK_GAP;
        }
        egui::Rect::from_min_size(egui::pos2(right - ASK, top), egui::vec2(ASK, ASK))
    }

    /// The ask button and the attached layers' marks.
    pub(crate) fn assistant_canvas_ui(&mut self, ctx: &egui::Context) {
        let ppp = ctx.pixels_per_point();
        if !self.ask_shows(ctx) {
            return;
        }
        let Some(bounds) = self.selection_bounds() else {
            return;
        };
        let sel = self.screen_rect(bounds, ppp);
        let screen = ctx.input(|i| i.content_rect());
        let place = self.ask_button_place(sel, screen);
        if !self.canvas_rect.contains_rect(place) {
            return;
        }
        let lit = self.selection_attached();
        let resp = egui::Area::new(egui::Id::new("assistant-ask"))
            .order(egui::Order::Middle)
            .fixed_pos(place.min)
            .show(ctx, |ui| {
                let (r, resp) = ui.allocate_exact_size(place.size(), egui::Sense::click());
                let p = ui.painter();
                let (ground, ink) = match (lit, resp.hovered()) {
                    (true, _) => (color::accent_600(), color::on_accent()),
                    (false, true) => (color::raised(), color::text()),
                    (false, false) => (color::card(), theme::text::strong()),
                };
                let shadow = egui::epaint::Shadow {
                    offset: [0, 4],
                    blur: 12,
                    spread: 0,
                    color: egui::Color32::from_black_alpha(90),
                };
                p.add(shadow.as_shape(r, 6.0));
                p.rect_filled(r, 6.0, ground);
                if !lit {
                    p.rect_stroke(
                        r,
                        6.0,
                        egui::Stroke::new(1.0, color::card_border()),
                        egui::StrokeKind::Inside,
                    );
                }
                p.text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    icon::SPARKLE,
                    theme::icon_font(14.0),
                    ink,
                );
                resp.on_hover_text(match lit {
                    true => "Attached · open the assistant",
                    false => "Ask the assistant",
                })
            })
            .inner;
        if resp.clicked() {
            match lit {
                true => self.open_assistant(),
                false => self.ask_assistant(),
            }
        }
    }

    /// A dashed accent outline [`MARK_OUT`] outside each attached layer, with a
    /// tab carrying the sparkle and its chip's number; the layer a chip under
    /// the pointer names is outlined solid.
    pub(crate) fn draw_assistant_marks(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        if !self.assistant.open || self.prefs.private {
            return;
        }
        let p = painter.with_clip_rect(rect);
        for (n, c) in self.assistant.chips.iter().enumerate() {
            let Some(id) = NodeId::from_wire(&c.id) else {
                continue;
            };
            let Some(r) = self.session.preview_world_bounds(id) else {
                continue;
            };
            let r = self.screen_rect(r, ppp).expand(MARK_OUT);
            let stroke = egui::Stroke::new(1.5, color::accent_400());
            for side in [
                [r.left_top(), r.right_top()],
                [r.right_top(), r.right_bottom()],
                [r.right_bottom(), r.left_bottom()],
                [r.left_bottom(), r.left_top()],
            ] {
                p.extend(egui::Shape::dashed_line(&side, stroke, 5.0, 3.0));
            }
            let label = format!("{}", n + 1);
            let font = egui::FontId::proportional(crate::ui::CAPTION_PT);
            let galley = p.layout_no_wrap(label, font, color::on_accent());
            let w = 5.0 + 11.0 + 3.0 + galley.size().x + 5.0;
            let tab = egui::Rect::from_min_size(
                egui::pos2(r.right() - w, r.top() - TAB_H),
                egui::vec2(w, TAB_H),
            );
            p.rect_filled(
                tab,
                egui::CornerRadius {
                    nw: 4,
                    ne: 4,
                    sw: 0,
                    se: 0,
                },
                color::accent_600(),
            );
            p.text(
                egui::pos2(tab.left() + 5.0 + 5.5, tab.center().y),
                egui::Align2::CENTER_CENTER,
                icon::SPARKLE,
                theme::icon_font(11.0),
                color::on_accent(),
            );
            let gh = galley.size().y;
            p.galley(
                egui::pos2(tab.left() + 5.0 + 11.0 + 3.0, tab.center().y - gh / 2.0),
                galley,
                color::on_accent(),
            );
        }
        if let Some(wire) = &self.assistant.hovered
            && let Some(id) = NodeId::from_wire(wire)
            && let Some(r) = self.session.preview_world_bounds(id)
        {
            let r = self.screen_rect(r, ppp);
            p.rect_stroke(
                r,
                0.0,
                egui::Stroke::new(1.5, color::accent()),
                egui::StrokeKind::Outside,
            );
        }
    }
}
