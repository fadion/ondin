//! The assistant on the canvas: the ask buttons over the selection and over
//! each attached layer, and while the panel is open a dashed outline on each
//! attached layer.

use crate::app::OndinApp;
use crate::preview::Drag;
use crate::theme::{self, color, icon};
use ondin_core::NodeId;
use ondin_core::kurbo::{Point, Rect};

/// The ask button's height, and its width without a number; the same at every
/// zoom.
pub(crate) const ASK: f32 = 26.0;
/// Clear air between the button and its layer's top edge.
const ASK_GAP: f32 = 8.0;
/// How far outside its layer an attached layer's outline runs.
const MARK_OUT: f32 = 5.0;
/// A lit button's inset either side of its sparkle and number.
const ASK_PAD: f32 = 7.0;
/// Between a lit button's sparkle and its number.
const ASK_NUM_GAP: f32 = 4.0;
/// The sparkle's size on the button.
const ASK_GLYPH_PT: f32 = 14.0;

/// One ask button: where it sits and what a click does.
pub(crate) struct AskButton {
    pub(crate) place: egui::Rect,
    /// The attached layer it stands for and its chip's number; `None` for the
    /// unattached selection's.
    pub(crate) attached: Option<(String, usize)>,
}

impl AskButton {
    /// The button's area, whose layer the canvas hands the wheel back from.
    pub(crate) fn area_id(&self) -> egui::Id {
        match &self.attached {
            Some((wire, _)) => egui::Id::new(("assistant-ask", wire.as_str())),
            None => egui::Id::new("assistant-ask"),
        }
    }
}

impl OndinApp {
    /// A world rect on screen.
    fn screen_rect(&self, r: Rect, ppp: f32) -> egui::Rect {
        let a = self.to_screen(Point::new(r.x0, r.y0), self.canvas_rect, ppp);
        let b = self.to_screen(Point::new(r.x1, r.y1), self.canvas_rect, ppp);
        egui::Rect::from_two_pos(a, b)
    }

    /// Whether ask buttons may show at all: the assistant on, and nothing the
    /// buttons would get in the way of.
    fn ask_live(&self, ctx: &egui::Context) -> bool {
        self.prefs.assistant.harness().is_some()
            && !self.prefs.private
            && self.presenting.is_none()
            && !self.focus
            && self.text.is_none()
            && self.edited_path().is_none()
            && matches!(self.drag, Drag::None)
            && ctx.dragged_id().is_none()
            && !self.session.has_gesture_preview()
    }

    /// An ask button `w` wide for layer bounds `sel` on screen: its right edge
    /// on the layer's, its bottom [`ASK_GAP`] above it; inside the corner where
    /// there is no room above; left of the inspector's cards.
    fn ask_button_place(&self, sel: egui::Rect, screen: egui::Rect, w: f32) -> egui::Rect {
        let column = Self::inspector_column(screen);
        let right = sel.right().min(column.left() - ASK_GAP);
        let mut top = sel.top() - ASK_GAP - ASK;
        if top < self.canvas_rect.top() + ASK_GAP {
            top = sel.top() + ASK_GAP;
        }
        egui::Rect::from_min_size(egui::pos2(right - w, top), egui::vec2(w, ASK))
    }

    /// A lit button's width, carrying number `n`.
    fn ask_lit_w(ctx: &egui::Context, n: usize) -> f32 {
        let num = ctx.fonts_mut(|f| {
            f.layout_no_wrap(
                n.to_string(),
                egui::FontId::proportional(crate::ui::SMALL_PT),
                egui::Color32::PLACEHOLDER,
            )
            .size()
            .x
        });
        ASK_PAD + ASK_GLYPH_PT + ASK_NUM_GAP + num + ASK_PAD
    }

    /// The ask buttons this frame. Each attached layer on this page has a lit
    /// one carrying its chip's number while the panel is open or the layer is
    /// selected, whatever *Show ask assistant* says. The selection has an unlit
    /// one while that switch is on and some selected layer is not attached; a
    /// lit button it would cover gives way.
    pub(crate) fn ask_buttons(&self, ctx: &egui::Context) -> Vec<AskButton> {
        let mut out = Vec::new();
        if !self.ask_live(ctx) {
            return out;
        }
        let ppp = ctx.pixels_per_point();
        let screen = ctx.input(|i| i.content_rect());
        let unlit = (self.show_ask && !self.selection_attached())
            .then(|| self.selection_bounds())
            .flatten()
            .map(|b| self.ask_button_place(self.screen_rect(b, ppp), screen, ASK))
            .filter(|p| self.canvas_rect.contains_rect(*p));
        for (n, c) in self.assistant.chips.iter().enumerate() {
            let Some(id) = NodeId::from_wire(&c.id) else {
                continue;
            };
            if !(self.assistant.open || self.session.selection.contains(id))
                || self.session.doc.page_of(id) != Some(self.session.page)
            {
                continue;
            }
            let Some(b) = self.session.preview_world_bounds(id) else {
                continue;
            };
            let w = Self::ask_lit_w(ctx, n + 1);
            let place = self.ask_button_place(self.screen_rect(b, ppp), screen, w);
            if !self.canvas_rect.contains_rect(place) || unlit.is_some_and(|u| u.intersects(place))
            {
                continue;
            }
            out.push(AskButton {
                place,
                attached: Some((c.id.clone(), n + 1)),
            });
        }
        if let Some(place) = unlit {
            out.push(AskButton {
                place,
                attached: None,
            });
        }
        out
    }

    /// Whether `layer` is an ask button's, which hands the wheel to the canvas.
    pub(crate) fn is_ask_layer(&self, ctx: &egui::Context, layer: egui::LayerId) -> bool {
        layer.order == egui::Order::Middle
            && self
                .ask_buttons(ctx)
                .iter()
                .any(|b| b.area_id() == layer.id)
    }

    /// A click on an ask button: the selection's attaches it and opens the
    /// panel; a lit one's detaches its layer, a toggle.
    pub(crate) fn ask_button_clicked(&mut self, attached: Option<String>) {
        match attached {
            Some(wire) => self.assistant.chips.retain(|c| c.id != wire),
            None => self.ask_assistant(),
        }
    }

    /// Draw the ask buttons.
    pub(crate) fn assistant_canvas_ui(&mut self, ctx: &egui::Context) {
        let mut clicked = None;
        for b in self.ask_buttons(ctx) {
            let lit = b.attached.is_some();
            let resp = egui::Area::new(b.area_id())
                .order(egui::Order::Middle)
                .fixed_pos(b.place.min)
                .show(ctx, |ui| {
                    let resp = crate::ui::float_button(ui, b.place.size(), lit, |p, r, ink| {
                        let glyph = theme::icon_font(ASK_GLYPH_PT);
                        match &b.attached {
                            None => {
                                p.text(
                                    r.center(),
                                    egui::Align2::CENTER_CENTER,
                                    icon::SPARKLE,
                                    glyph,
                                    ink,
                                );
                            }
                            Some((_, n)) => {
                                let x = r.left() + ASK_PAD;
                                p.text(
                                    egui::pos2(x + ASK_GLYPH_PT / 2.0, r.center().y),
                                    egui::Align2::CENTER_CENTER,
                                    icon::SPARKLE,
                                    glyph,
                                    ink,
                                );
                                p.text(
                                    egui::pos2(x + ASK_GLYPH_PT + ASK_NUM_GAP, r.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    n.to_string(),
                                    egui::FontId::proportional(crate::ui::SMALL_PT),
                                    ink,
                                );
                            }
                        }
                    });
                    resp.on_hover_text(match lit {
                        true => "Detach from the assistant",
                        false => "Ask the assistant",
                    })
                })
                .inner;
            if resp.clicked() {
                clicked = Some(b.attached.map(|(wire, _)| wire));
            }
        }
        if let Some(attached) = clicked {
            self.ask_button_clicked(attached);
        }
    }

    /// A dashed accent outline [`MARK_OUT`] outside each attached layer; the
    /// layer a chip under the pointer names is outlined solid.
    pub(crate) fn draw_assistant_marks(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        if !self.assistant.open || self.prefs.private {
            return;
        }
        let p = painter.with_clip_rect(rect);
        for c in &self.assistant.chips {
            let Some(id) = NodeId::from_wire(&c.id)
                .filter(|id| self.session.doc.page_of(*id) == Some(self.session.page))
            else {
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
        }
        // A layer on another page has no place here to outline.
        if let Some(wire) = &self.assistant.hovered
            && let Some(id) = NodeId::from_wire(wire)
            && self.session.doc.page_of(id) == Some(self.session.page)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The ask buttons for `app` in a 2000×1400 window.
    fn buttons(ctx: &egui::Context, app: &OndinApp) -> Vec<(Option<usize>, egui::Rect)> {
        let mut out = Vec::new();
        for _ in 0..2 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(2000.0, 1400.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    out = app
                        .ask_buttons(ui.ctx())
                        .into_iter()
                        .map(|b| (b.attached.map(|(_, n)| n), b.place))
                        .collect();
                },
            );
        }
        out
    }

    /// An attached layer has one ask button, lit and numbered, which shows with
    /// *Show ask assistant* off and detaches the layer when clicked.
    /// flip: `ask_buttons` keeping the selection's button once it is attached →
    /// fails at the single lit button.
    #[test]
    fn an_attached_layer_has_one_lit_button_that_detaches_it() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let id = crate::palette::tests::add_rect(&mut app);
        app.session.selection.set_one(id);
        app.prefs.assistant.agent = "claude".into();
        app.canvas_rect =
            egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(1600.0, 1200.0));
        app.canvas_px = (1600, 1200);
        assert!(
            app.ask_live(&ctx),
            "nothing in the fixture hides the buttons"
        );
        let shown = buttons(&ctx, &app);
        assert_eq!(shown.len(), 1, "the selection's button: {shown:?}");
        assert_eq!(shown[0].0, None, "unlit before it is attached");

        app.ask_button_clicked(None);
        assert_eq!(app.assistant.chips.len(), 1, "a click attaches");
        app.show_ask = false;
        let shown = buttons(&ctx, &app);
        assert_eq!(
            shown.iter().map(|b| b.0).collect::<Vec<_>>(),
            vec![Some(1)],
            "one button, lit with the chip's number, despite the switch"
        );

        app.ask_button_clicked(Some(id.to_wire()));
        assert!(
            app.assistant.chips.is_empty(),
            "a lit button's click detaches"
        );
        assert!(buttons(&ctx, &app).is_empty(), "the switch is off");
    }

    /// A link hovered in a reply outlines its layer on this page, and nothing
    /// for a layer on another page.
    /// flip: `draw_assistant_marks` without its page check on the hovered layer →
    /// fails at "nothing for another page".
    #[test]
    fn a_hovered_layer_on_another_page_is_not_outlined_here() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let id = crate::palette::tests::add_rect(&mut app);
        app.assistant.open = true;
        app.assistant.hovered = Some(id.to_wire());
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1200.0));
        let marks = |app: &OndinApp| {
            let out = ctx.run_ui(egui::RawInput::default(), |ui| {
                app.draw_assistant_marks(ui.painter(), rect, 1.0);
            });
            out.shapes.len()
        };
        let here = marks(&app);
        assert!(here > 0, "outlined on its page");
        app.add_page();
        assert_ne!(app.session.doc.page_of(id), Some(app.session.page));
        assert_eq!(marks(&app), 0, "nothing for another page");
    }
}
