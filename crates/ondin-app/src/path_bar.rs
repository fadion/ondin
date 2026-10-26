//! The path bar: a short rail beside the tool rail while a path's points are being
//! edited, holding the node tool's verbs — select or add points, corner and smooth,
//! join, reverse, simplify, offset — and the way out.
//!
//! It floats over the canvas, the ruler included, centred on the rail's Pen button,
//! so it reads as that button's drawer. *Offset* opens a card with one scrubbable
//! distance field, previewed through the edit valve and always measured from the
//! outline the card opened on, so scrubbing back to zero gives the path back.

use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use crate::tools::Tool;
use crate::ui::{self, Prefix, Scrub, icon_button, value_field};
use ondin_core::kurbo::BezPath;
use ondin_core::{GeometryPatch, NodeId, NodeKind, Operation, Transaction, build};

/// The gap between the tool rail's edge and the bar, and between the bar and the
/// Offset card.
const GAP: f32 = 8.0;
/// The bar's width: the tool rail's.
const BAR_W: f32 = 46.0;
/// The Offset card's width.
const CARD_W: f32 = 176.0;

/// The Offset card's subject: the path, as stored and as drawn when the card
/// opened, and the distance the field shows.
pub(crate) struct OffsetCard {
    id: NodeId,
    /// Written back at distance zero, so the corner radii come back too.
    stored: (BezPath, Vec<f64>),
    /// What every distance offsets: the stored path with its radii applied.
    drawn: BezPath,
    distance: f64,
    /// Whether the last distance left nothing (a shrink past the path's middle).
    empty: bool,
}

impl OffsetCard {
    /// The transaction that puts the path `distance` from where the card found it.
    fn tx(&mut self) -> Transaction {
        let (path, corner_radii) = if self.distance == 0.0 {
            self.empty = false;
            self.stored.clone()
        } else {
            match build::offset_outline(&self.drawn, self.distance) {
                Ok(path) => {
                    self.empty = false;
                    (path, Vec::new())
                }
                Err(_) => {
                    self.empty = true;
                    return Transaction::default();
                }
            }
        };
        Transaction(vec![Operation::SetGeometry {
            id: self.id,
            geometry: GeometryPatch::Path { path, corner_radii },
        }])
    }
}

/// One entry of the bar.
enum E {
    Sep,
    Btn {
        glyph: &'static str,
        tip: &'static str,
        /// Shown instead of `tip` while the button is dimmed.
        dimmed: &'static str,
        active: bool,
        enabled: bool,
        verb: Verb,
    },
}

#[derive(Clone, Copy)]
enum Verb {
    Points,
    Pen,
    Smooth(bool),
    Join,
    Reverse,
    Simplify,
    Offset,
    Done,
}

impl OndinApp {
    /// Draw the path bar and its Offset card while a path's points are being edited.
    pub(crate) fn path_bar(&mut self, ctx: &egui::Context) {
        let Some(id) = self.edited_path() else {
            self.offset_card = None;
            return;
        };
        if self.offset_card.as_ref().is_some_and(|c| c.id != id) {
            self.offset_card = None;
        }
        let has_points = !self.subject_points(id).is_empty();
        let bar = [
            E::Btn {
                glyph: icon::CURSOR,
                tip: "Select and drag points (A)",
                dimmed: "",
                active: !self.pen_bias,
                enabled: true,
                verb: Verb::Points,
            },
            E::Btn {
                glyph: icon::PEN_NIB,
                tip: "Pen — add a point on a segment, carry on an open end (P)",
                dimmed: "",
                active: self.pen_bias,
                enabled: true,
                verb: Verb::Pen,
            },
            E::Sep,
            E::Btn {
                glyph: icon::ANGLE,
                tip: "Make corner",
                dimmed: "Select points to make corners",
                active: false,
                enabled: has_points,
                verb: Verb::Smooth(false),
            },
            E::Btn {
                glyph: icon::WAVE_SINE,
                tip: "Make smooth",
                dimmed: "Select points to make smooth",
                active: false,
                enabled: has_points,
                verb: Verb::Smooth(true),
            },
            E::Btn {
                glyph: icon::PLUGS_CONNECTED,
                tip: "Join the two selected ends",
                dimmed: "Select two open ends of this path to join",
                active: false,
                enabled: self.join_subject().is_some(),
                verb: Verb::Join,
            },
            E::Btn {
                glyph: icon::ARROWS_CLOCKWISE,
                tip: "Reverse direction (Shift+D)",
                dimmed: "",
                active: false,
                enabled: true,
                verb: Verb::Reverse,
            },
            E::Sep,
            E::Btn {
                glyph: icon::MAGIC_WAND,
                tip: "Simplify — fewer points, the same shape",
                dimmed: "",
                active: false,
                enabled: true,
                verb: Verb::Simplify,
            },
            E::Btn {
                glyph: icon::ARROWS_OUT_CARDINAL,
                tip: "Offset — grow or shrink the outline",
                dimmed: "",
                active: self.offset_card.is_some(),
                enabled: true,
                verb: Verb::Offset,
            },
            E::Sep,
            E::Btn {
                glyph: icon::CHECK,
                tip: "Done (Enter)",
                dimmed: "",
                active: false,
                enabled: true,
                verb: Verb::Done,
            },
        ];

        // Centred on the rail's Pen button; with the rail hidden, on the canvas's edge.
        let centre_y = if self.show_toolbar && self.pen_button.is_finite() {
            self.pen_button.center().y
        } else {
            self.canvas_rect.center().y
        };
        let at = egui::pos2(self.canvas_rect.left() + GAP, centre_y);
        let mut clicked: Option<Verb> = None;
        let mut offset_button = egui::Rect::NOTHING;
        let shown = egui::Area::new(egui::Id::new("path-bar"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::LEFT_CENTER)
            .fixed_pos(at)
            .show(ctx, |ui| {
                bar_frame().show(ui, |ui| {
                    // Less the hairline, which the frame draws outside its content.
                    ui.set_width(BAR_W - 2.0);
                    ui.vertical_centered(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for e in &bar {
                            match e {
                                E::Sep => separator(ui),
                                E::Btn {
                                    glyph,
                                    tip,
                                    dimmed,
                                    active,
                                    enabled,
                                    verb,
                                } => {
                                    let resp =
                                        icon_button(ui, glyph, 30.0, 20.0, *active, *enabled)
                                            .on_hover_text(if *enabled { *tip } else { *dimmed });
                                    if matches!(verb, Verb::Offset) {
                                        offset_button = resp.rect;
                                    }
                                    if *enabled && resp.clicked() {
                                        clicked = Some(*verb);
                                    }
                                }
                            }
                        }
                    });
                });
            });
        let bar_rect = shown.response.rect;

        match clicked {
            Some(Verb::Points) => self.choose_tool(Tool::Node),
            Some(Verb::Pen) => self.choose_tool(Tool::Pen),
            Some(Verb::Smooth(smooth)) => self.set_point_smoothness(smooth),
            Some(Verb::Join) => self.join_points(),
            Some(Verb::Reverse) => self.reverse_subpaths(),
            Some(Verb::Simplify) => self.simplify_edited_path(),
            Some(Verb::Offset) => self.toggle_offset_card(id),
            Some(Verb::Done) => {
                self.choose_tool(Tool::Select);
                return;
            }
            None => {}
        }
        if self.offset_card.is_some() {
            self.offset_card_ui(ctx, bar_rect, offset_button);
        }
    }

    /// *Simplify* over the edited path (`build::simplify_path`), one commit.
    pub(crate) fn simplify_edited_path(&mut self) {
        let Some(id) = self.edited_path() else { return };
        match build::simplify_path(&self.session.doc, id) {
            Ok(tx) if tx.0.is_empty() => self.session.info("Already as simple as it gets"),
            Ok(tx) => {
                if self.session.commit(tx) {
                    // The anchors are new, so the old point references mean nothing.
                    self.points.clear();
                    self.session.info("Simplified");
                }
            }
            Err(e) => self.session.info(format!("Could not simplify: {e}")),
        }
    }

    /// Open the Offset card on `id`, or close it.
    fn toggle_offset_card(&mut self, id: NodeId) {
        if self.offset_card.take().is_some() {
            return;
        }
        let Some(NodeKind::Path { path, corner_radii }) =
            self.session.doc.get(id).map(|n| n.kind().clone())
        else {
            return;
        };
        let drawn = ondin_core::geometry::round_corners(&path, &corner_radii);
        self.offset_card = Some(OffsetCard {
            id,
            stored: (path, corner_radii),
            drawn,
            distance: 0.0,
            empty: false,
        });
    }

    /// The Offset card: beside the bar, its top on the Offset button's. A press
    /// anywhere else closes it.
    fn offset_card_ui(&mut self, ctx: &egui::Context, bar: egui::Rect, button: egui::Rect) {
        let Some(mut card) = self.offset_card.take() else {
            return;
        };
        let at = egui::pos2(bar.right() + GAP, button.top());
        let mut field: Option<egui::Response> = None;
        let shown = egui::Area::new(egui::Id::new("offset-card"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ctx, |ui| {
                ui.set_max_width(CARD_W);
                ui::card(ui, |ui| {
                    ui::labelled(ui, "Offset", |ui| {
                        let size = egui::vec2(ui.available_width(), ui::CONTROL_H);
                        let resp = value_field(
                            ui,
                            size,
                            Prefix::Icon(icon::ARROWS_OUT_CARDINAL),
                            &mut card.distance,
                            Scrub::whole(0.5),
                            |d| d.custom_formatter(|v, _| ui::number(2)(v, 0..=0)),
                        )
                        .on_hover_text("Distance — negative shrinks the outline");
                        field = Some(resp);
                        if card.empty {
                            ui.label(
                                egui::RichText::new("Nothing left at this distance")
                                    .size(11.0)
                                    .color(theme::text::MUTED),
                            );
                        }
                    });
                });
            });
        if let Some(resp) = field
            && crate::panels::interacting(&resp)
        {
            let tx = card.tx();
            if self.edit_valve(&resp, tx) {
                self.points.clear();
            }
        }
        let card_rect = shown.response.rect;
        let pressed_away = ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !card_rect.contains(p) && !button.contains(p))
        });
        if !pressed_away {
            self.offset_card = Some(card);
        }
    }
}

/// The bar's ground: the rail's fill, as a floating card draws it.
fn bar_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(color::CARD)
        .stroke(egui::Stroke::new(1.0, theme::color::text_a(15)))
        .shadow(egui::epaint::Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: egui::Color32::from_black_alpha(70),
        })
        .corner_radius(egui::CornerRadius::same(5))
        .inner_margin(egui::Margin::symmetric(0, 9))
}

/// A rule between groups of a rail's buttons — the tool rail's and the bar's.
pub(crate) fn separator(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    let (r, _) = ui.allocate_exact_size(egui::vec2(22.0, 1.0), egui::Sense::empty());
    ui.painter()
        .rect_filled(r, egui::CornerRadius::ZERO, theme::color::text_a(31));
    ui.add_space(4.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::Shape;
    use ondin_core::{Document, IdSource};

    /// A closed path, 64 straight sides round a circle of radius 50, with one radius
    /// on its first corner, selected under the node tool.
    fn app_with_a_ring(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        let mut p = BezPath::new();
        for i in 0..64 {
            let a = i as f64 / 64.0 * std::f64::consts::TAU;
            let at = (50.0 + 50.0 * a.cos(), 50.0 + 50.0 * a.sin());
            if i == 0 { p.move_to(at) } else { p.line_to(at) }
        }
        p.close_path();
        let mut radii = vec![0.0; 64];
        radii[0] = 2.0;
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Path {
                path: p,
                corner_radii: radii,
            },
            transform: None,
            name: None,
        }]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        app.tool = Tool::Node;
        assert_eq!(
            app.edited_path(),
            Some(id),
            "fixture: the ring is being edited"
        );
        (app, id)
    }

    fn stored(app: &OndinApp, id: NodeId) -> (BezPath, Vec<f64>) {
        let Some(NodeKind::Path { path, corner_radii }) = app.session.doc.get(id).map(|n| n.kind())
        else {
            panic!("a Path");
        };
        (path.clone(), corner_radii.clone())
    }

    /// Every distance offsets the outline the card opened on, and zero writes the
    /// stored path back with its radii. flip: `OffsetCard::tx` offsetting at zero
    /// too → fails at "zero gives the radii back".
    #[test]
    fn offset_card_measures_from_where_it_opened() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_ring(&ctx);
        let before = stored(&app, id);
        app.toggle_offset_card(id);
        let card = app.offset_card.as_mut().expect("the card opened");

        card.distance = 10.0;
        let grow = card.tx();
        app.session.commit(grow);
        let b = stored(&app, id).0.bounding_box();
        assert!((b.width() - 120.0).abs() < 0.5, "grown by 10 a side: {b:?}");

        // Measured from the opening outline, not from the one just committed.
        let card = app.offset_card.as_mut().expect("still open");
        card.distance = 5.0;
        let grow = card.tx();
        app.session.commit(grow);
        let b = stored(&app, id).0.bounding_box();
        assert!(
            (b.width() - 110.0).abs() < 0.5,
            "5 from the original: {b:?}"
        );

        let card = app.offset_card.as_mut().expect("still open");
        card.distance = 0.0;
        let back = card.tx();
        app.session.commit(back);
        assert_eq!(stored(&app, id), before, "zero gives the radii back");

        let card = app.offset_card.as_mut().expect("still open");
        card.distance = -80.0;
        assert!(card.tx().0.is_empty(), "nothing left to write");
        assert!(card.empty, "and the card says so");
    }

    /// Simplify refits the ring in one undo step and drops the point selection,
    /// whose anchors are gone.
    #[test]
    fn simplify_is_one_step_and_clears_the_points() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_ring(&ctx);
        app.points.set_one(id, (0, 3));
        let depth = app.session.history.undo_depth();
        let segments = stored(&app, id).0.segments().count();
        app.simplify_edited_path();
        let (path, radii) = stored(&app, id);
        assert!(path.segments().count() < segments / 4, "refitted");
        assert!(radii.is_empty(), "the radius is baked into the curves");
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
        assert!(app.points.is_empty(), "the old anchors are gone");
    }

    /// The bar shows only while a path's points are edited, beside the rail and
    /// centred on its Pen button. flip: `centre_y` read from `canvas_rect` alone →
    /// fails at "centred on Pen".
    #[test]
    fn the_bar_sits_beside_the_rail_on_the_pen_button() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, _) = app_with_a_ring(&ctx);
        app.canvas_rect =
            egui::Rect::from_min_max(egui::pos2(46.0, 40.0), egui::pos2(900.0, 700.0));
        app.pen_button =
            egui::Rect::from_center_size(egui::pos2(23.0, 300.0), egui::vec2(30.0, 30.0));
        let run = |app: &mut OndinApp| {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 700.0),
                    )),
                    ..Default::default()
                },
                |ui| app.path_bar(ui.ctx()),
            );
        };
        for _ in 0..3 {
            run(&mut app);
        }
        let bar = ctx
            .memory(|m| m.area_rect(egui::Id::new("path-bar")))
            .expect("the bar is drawn");
        assert_eq!(bar.left(), 46.0 + GAP, "a gap off the rail");
        assert_eq!(bar.width(), BAR_W, "the rail's width");
        assert!(
            (bar.center().y - 300.0).abs() < 1.0,
            "centred on Pen: {bar:?}"
        );

        app.tool = Tool::Select;
        let visible = |ctx: &egui::Context| {
            ctx.memory(|m| {
                m.areas().visible_last_frame(&egui::LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::new("path-bar"),
                ))
            })
        };
        for _ in 0..2 {
            run(&mut app);
        }
        assert!(!visible(&ctx), "gone once the edit ends");
    }
}
