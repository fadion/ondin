//! *Blend shapes…* (`docs/design/app-booleans.md`, *Blend shapes*): a card over
//! the canvas that makes the steps between two shapes (`ondin_core::blend`),
//! previewed live and committed as one *Blend shapes* step.

use crate::app::OndinApp;
use crate::theme::icon;
use crate::ui::{self, FieldButton, Prefix, Scrub};
use eframe::egui;
use ondin_core::blend::{self, Blend};
use ondin_core::kurbo::Vec2;
use ondin_core::{IdSource, NodeId, Transaction};

/// The card's width: a popover's.
const W: f32 = crate::app::POPOVER_W;

/// The card's settings, kept closed so it reopens on the blend last made.
pub(crate) struct BlendCard {
    pub(crate) open: bool,
    steps: f64,
    paint: bool,
    /// Which of the nine places the outlines pair up from (`canvas::NINE_POINTS`).
    start: usize,
    reverse: bool,
    /// The preview built for the key it was built from.
    built: Option<(Key, Transaction)>,
}

impl Default for BlendCard {
    fn default() -> Self {
        Self {
            open: false,
            steps: 5.0,
            paint: true,
            start: 0,
            reverse: false,
            built: None,
        }
    }
}

/// What a preview was built from: the revision, the selection and the settings.
type Key = (u64, Vec<NodeId>, usize, bool, usize, bool);

impl BlendCard {
    fn blend(&self) -> Blend {
        let (col, row) = (self.start % 3, self.start / 3);
        Blend {
            steps: (self.steps.round() as usize).clamp(1, blend::MAX_STEPS),
            paint: self.paint,
            start: Vec2::new(col as f64 / 2.0, row as f64 / 2.0),
            reverse: self.reverse,
        }
    }
}

enum Act {
    Close,
    Commit,
    Paint,
    Reverse,
    Start(usize),
}

impl OndinApp {
    /// Why the selection cannot be blended, if it cannot.
    fn blend_refusal(&self) -> Option<&'static str> {
        blend::pair(&self.session.doc, self.session.selection.ids())
            .err()
            .map(|r| r.says())
    }

    /// Open the card on the two selected shapes, or say why it cannot.
    pub(crate) fn open_blend(&mut self) {
        if let Some(why) = self.blend_refusal() {
            return self.session.info(why);
        }
        self.close_find();
        self.close_repeat();
        self.colour_review.open = false;
        self.blend.open = true;
        self.blend.built = None;
    }

    /// Close the card and take its steps off the canvas.
    pub(crate) fn close_blend(&mut self) {
        if self.blend.open {
            self.blend.open = false;
            self.blend.built = None;
            self.session.clear_gesture_preview();
        }
    }

    /// Whether the gesture preview is the card's steps rather than a gesture.
    pub(crate) fn blend_preview_standing(&self) -> bool {
        self.blend.open && self.drag.is_none()
    }

    /// Build the preview again when what it shows changed, and keep it up between
    /// other gestures. It is the commit's own edit, minted from a copy of the
    /// counter so it spends no ids.
    fn refresh_blend_preview(&mut self) {
        let b = self.blend.blend();
        let ids = self.session.selection.ids().to_vec();
        let key: Key = (
            self.session.revision(),
            ids.clone(),
            b.steps,
            b.paint,
            self.blend.start,
            b.reverse,
        );
        if !self.drag.is_none() {
            return;
        }
        if self.blend.built.as_ref().is_none_or(|(k, _)| *k != key) {
            let mut minter = IdSource::new(self.session.ids.actor());
            minter.skip_to(self.session.ids.next_seq());
            let (doc, res) = (&self.session.doc, &self.session.resolved);
            let tx = blend::blend(doc, res, &mut minter, &ids, b)
                .map(|(tx, _)| tx)
                .unwrap_or_default();
            self.session.set_preview(&tx);
            self.blend.built = Some((key, tx));
        } else if !self.session.has_gesture_preview()
            && let Some((_, tx)) = &self.blend.built
        {
            let tx = tx.clone();
            self.session.set_preview(&tx);
        }
    }

    /// Blend the two shapes as the card says, one *Blend shapes* step; the group
    /// of steps is selected.
    pub(crate) fn commit_blend(&mut self) {
        let b = self.blend.blend();
        let ids = self.session.selection.ids().to_vec();
        let made = blend::blend(
            &self.session.doc,
            &self.session.resolved,
            &mut self.session.ids,
            &ids,
            b,
        );
        let (tx, group) = match made {
            Ok(made) => made,
            Err(r) => return self.session.info(r.says()),
        };
        self.session.clear_gesture_preview();
        self.name_step_as(crate::menu::Item::BlendShapes);
        if !self.commit_edit(tx) {
            return;
        }
        self.session.selection.set_one(group);
        self.session.info(format!(
            "Blended in {}",
            ui::count_as(b.steps, "step", "steps")
        ));
        self.blend.open = false;
        self.blend.built = None;
    }

    /// Draw the card when it is open, keep its preview up, and act on this
    /// frame's clicks.
    pub(crate) fn blend_ui(&mut self, ctx: &egui::Context) {
        if !self.blend.open {
            return;
        }
        if self.blend_refusal().is_some() {
            return self.close_blend();
        }
        self.refresh_blend_preview();
        let canvas = self.canvas_rect;
        let card = &mut self.blend;
        let mut act = None;
        ui::canvas_popup_area(ctx, egui::Id::new("blend-shapes"), canvas).show(ctx, |ui| {
            ui::hold_tab(ui);
            crate::settings::card_frame().show(ui, |ui| {
                ui.set_width(W - 2.0 * ui::POPOVER_PAD);
                ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
                if crate::settings::modal_title(ui, "Blend shapes") {
                    act = Some(Act::Close);
                }
                let w = ui.available_width();
                ui::labelled(ui, "Steps between", |ui| {
                    ui::value_field(
                        ui,
                        egui::vec2(w, ui::CONTROL_H),
                        Prefix::Icon(icon::DOTS_THREE),
                        &mut card.steps,
                        Scrub::whole(0.1).range(1.0..=blend::MAX_STEPS as f64),
                        |d| d.custom_formatter(ui::number(0)),
                    )
                    .on_hover_text("How many shapes go between the two")
                });
                if ui::switch_row(
                    ui,
                    "Blend paint and opacity",
                    card.paint,
                    crate::settings::SWITCH_H,
                )
                .clicked()
                {
                    act = Some(Act::Paint);
                }
                ui::labelled(ui, "Match points from", |ui| {
                    ui.scope(|ui| {
                        ui.spacing_mut().interact_size.y = ui::CONTROL_H;
                        ui.spacing_mut().button_padding.y = 0.0;
                        let mut start = card.start;
                        egui::ComboBox::from_id_salt("blend-start")
                            .icon(ui::combo_chevron)
                            .width(w)
                            .selected_text(crate::canvas::NINE_POINTS[start])
                            .show_ui(ui, |ui| {
                                ui::menu_rows(ui);
                                for (i, name) in crate::canvas::NINE_POINTS.iter().enumerate() {
                                    ui.selectable_value(&mut start, i, *name);
                                }
                            })
                            .response
                            .on_hover_text("Where on each shape the first points pair up");
                        if start != card.start {
                            act = Some(Act::Start(start));
                        }
                    });
                });
                if ui::switch_row(
                    ui,
                    "Reverse the second shape",
                    card.reverse,
                    crate::settings::SWITCH_H,
                )
                .on_hover_text("Run the second outline the other way, for a blend that twists")
                .clicked()
                {
                    act = Some(Act::Reverse);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                    if crate::settings::text_button(ui, "Blend", FieldButton::Primary).clicked() {
                        act = Some(Act::Commit);
                    }
                    if crate::settings::text_button(ui, "Cancel", FieldButton::Off).clicked() {
                        act = Some(Act::Close);
                    }
                });
            });
        });
        let Some(act) = act else {
            return;
        };
        match act {
            Act::Close => self.close_blend(),
            Act::Commit => self.commit_blend(),
            Act::Paint => self.blend.paint = !self.blend.paint,
            Act::Reverse => self.blend.reverse = !self.blend.reverse,
            Act::Start(i) => self.blend.start = i,
        }
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{Document, NodeKind, Operation};

    /// A headless app holding a circle under a square on the page, both selected.
    fn two_shapes(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let page = doc.first_page();
        let (circle, square) = (ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: circle,
                parent: page,
                index: 0,
                kind: NodeKind::Ellipse {
                    size: Size::new(60.0, 60.0),
                    arc: Default::default(),
                },
                transform: Some(Affine::translate((0.0, 0.0))),
                name: None,
            },
            Operation::CreateNode {
                id: square,
                parent: page,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(160.0, 160.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((400.0, 0.0))),
                name: None,
            },
        ]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.canvas_px = (800, 600);
        app.session.selection.set(vec![circle, square]);
        (app, circle, square)
    }

    /// The card's commit lands the group between the two, selects it, says how
    /// many steps, and is one undo step.
    #[test]
    fn blending_lands_one_step_and_selects_the_group() {
        let ctx = egui::Context::default();
        let (mut app, circle, square) = two_shapes(&ctx);
        app.open_blend();
        assert!(app.blend.open);
        app.refresh_blend_preview();
        assert!(app.session.has_gesture_preview(), "the steps preview");
        app.commit_blend();
        let group = app
            .session
            .selection
            .single()
            .expect("the group is selected");
        let page = app.session.doc.first_page();
        assert_eq!(
            app.session.doc.get(page).unwrap().children(),
            &[circle, group, square]
        );
        assert_eq!(app.session.status().text, "Blended in 5 steps");
        app.undo();
        assert_eq!(
            app.session.doc.get(page).unwrap().children(),
            &[circle, square]
        );
    }

    /// One shape is refused with the row's own sentence, and no card opens.
    #[test]
    fn one_shape_is_refused_by_name() {
        let ctx = egui::Context::default();
        let (mut app, circle, _) = two_shapes(&ctx);
        app.session.selection.set_one(circle);
        app.open_blend();
        assert!(!app.blend.open);
        assert_eq!(
            app.session.status().text,
            "Blend shapes needs exactly two shapes"
        );
    }
}
