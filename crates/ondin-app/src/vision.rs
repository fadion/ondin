//! Vision simulation mode: one frame of the canvas shown as someone sees it
//! with a condition of colour vision or of sight, at a severity
//! (`ondin_render::vision`), the rest of the canvas and every piece of edit
//! chrome as they are.
//!
//! *Tools ▸ Simulate vision* or the palette, no chord. View state, as the
//! contrast check is: off when a document opens, never saved, never exported,
//! never drawn in present mode. The two modes do not run together, so their
//! bars never stack: starting one ends the other. The condition last chosen and
//! each condition's severity are preferences (`prefs.vision`).

use ondin_core::NodeId;
use ondin_core::kurbo::Rect;
use ondin_render::vision::{Condition, Effect, MIN_SEVERITY};

use crate::app::OndinApp;
use crate::contrast::{BAR_H, bar_frame, cell, scope_chip, separator, top_level};
use crate::theme::{self, color, icon};

/// The severity slider's width in the bar.
const SLIDER_W: f32 = 88.0;
/// The condition menu's width: room for *Blurred vision · Out of focus*.
const MENU_W: f32 = 240.0;

/// The mode while it is on (`OndinApp::vision`).
pub(crate) struct Vision {
    /// The page it started on; another page takes the frame in view there.
    page: NodeId,
    /// The frame simulated, or `None` for the whole canvas on a page with none.
    frame: Option<NodeId>,
    menu: bool,
    /// The condition head's bottom left, which the menu hangs from.
    menu_at: egui::Pos2,
    /// Set on the frame the head is clicked, whose click is not a click away.
    menu_opening: bool,
}

/// What the canvas is seen through this frame (`OndinApp::vision_region`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Seen {
    pub effect: Effect,
    /// Device px to the design px: a blur's or a ghost's lengths scale with the
    /// zoom, so they read the same at any zoom.
    pub scale: f32,
    /// The texels covered, `x0, y0, x1, y1`, the far edges exclusive.
    pub region: [u32; 4],
}

impl OndinApp {
    /// *Tools ▸ Simulate vision*: start the mode on the selection's
    /// frame, or the frame in view, ending the contrast check; or end it.
    pub(crate) fn toggle_vision(&mut self) {
        if self.vision.is_some() {
            self.vision = None;
            return;
        }
        self.end_contrast();
        self.vision = Some(Vision {
            page: self.session.page,
            frame: self.vision_target(),
            menu: false,
            menu_at: egui::Pos2::ZERO,
            menu_opening: false,
        });
    }

    /// The frame a simulation shows: the top-level frame of the first selected
    /// layer, else the frame in view; `None` on a page with no frame.
    pub(crate) fn vision_target(&self) -> Option<NodeId> {
        let doc = &self.session.doc;
        let page = self.session.page;
        let is_frame = |id: NodeId| {
            doc.get(id)
                .is_some_and(|n| matches!(n.kind(), ondin_core::NodeKind::Artboard { .. }))
        };
        self.session
            .selection
            .ids()
            .first()
            .map(|&id| top_level(doc, page, id))
            .filter(|&id| is_frame(id))
            .or_else(|| self.frame_in_view())
    }

    /// What the canvas is seen through, at the scale and over the texels of the
    /// texture `camera` draws at `px`, or `None` while the mode is off. Runs
    /// before the canvas; a page switch, or its frame going away, moves the mode
    /// to the frame in view.
    pub(crate) fn vision_region(
        &mut self,
        camera: &crate::view::Camera,
        px: (u32, u32),
    ) -> Option<Seen> {
        let page = self.session.page;
        let doc = &self.session.doc;
        // A frame deleted or undone away is retargeted as a page switch is.
        let stale = |v: &Vision| v.page != page || v.frame.is_some_and(|f| doc.get(f).is_none());
        if self.vision.as_ref().is_some_and(stale) {
            let frame = self.vision_target();
            if let Some(v) = &mut self.vision {
                v.page = page;
                v.frame = frame;
            }
        }
        let v = self.vision.as_ref()?;
        let condition = self.prefs.vision.condition();
        let effect = condition.effect(self.prefs.vision.severity(condition));
        // `zoom` is device px to the world unit, which is the design px.
        let scale = camera.zoom as f32;
        let (w, h) = px;
        let region = match v.frame {
            None => [0, 0, w, h],
            Some(frame) => {
                let vp = camera.viewport(px);
                self.session
                    .resolved
                    .world_bounds(frame)
                    .map_or([0, 0, 0, 0], |b| device_region(b, vp.view, (w, h)))
            }
        };
        Some(Seen {
            effect,
            scale,
            region,
        })
    }

    /// The bar and its menu: floating chrome over the canvas, drawn after it.
    pub(crate) fn vision_chrome(&mut self, ctx: &egui::Context) {
        if self.vision.is_none() || !self.canvas_rect.is_positive() {
            return;
        }
        self.vision_bar(ctx);
        self.vision_menu(ctx);
        if self.vision.as_ref().is_some_and(|v| v.menu) {
            self.popover_heard = true;
        }
    }

    fn vision_bar(&mut self, ctx: &egui::Context) {
        let Some(v) = &self.vision else { return };
        let condition = self.prefs.vision.condition();
        let mut severity = f64::from(self.prefs.vision.severity(condition));
        // Named as drawn now, so private mode turned on during it masks it.
        let name = v
            .frame
            .filter(|&f| self.session.doc.get(f).is_some())
            .map(|f| crate::private::layer(&self.session.doc, f).into_owned());
        let open = v.menu;
        let selected = !self.session.selection.ids().is_empty();
        let ruler = if self.rulers_on() {
            crate::rulers::THICKNESS
        } else {
            0.0
        };
        let mut act = None;
        egui::Area::new(egui::Id::new("vision-bar"))
            .order(egui::Order::Middle)
            .fade_in(false)
            .pivot(egui::Align2::CENTER_TOP)
            .fixed_pos(self.canvas_rect.center_top() + egui::vec2(0.0, 12.0 + ruler))
            .show(ctx, |ui| {
                bar_frame().show(ui, |ui| {
                    ui.set_height(BAR_H - 2.0);
                    ui.horizontal_centered(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        ui.label(theme::icon_text(icon::EYE, 15.0, color::accent_300()));
                        match &name {
                            Some(name) => {
                                if scope_chip(ui, name, selected) {
                                    act = Some(Act::Retarget);
                                }
                            }
                            None => {
                                ui.label(
                                    egui::RichText::new("Whole canvas")
                                        .size(crate::ui::Scale::Toolbar.text_pt())
                                        .color(color::text()),
                                );
                            }
                        }
                        // The head's ground carries its own inset, so the gaps
                        // either side of it are short by that much.
                        ui.spacing_mut().item_spacing.x = 10.0 - crate::ui::HEAD_LEAD;
                        separator(ui);
                        ui.spacing_mut().item_spacing.x = 10.0 - crate::ui::HEAD_PAD;
                        let pt = crate::ui::Scale::Toolbar.text_pt();
                        // As wide as the longest label, so the slider never moves
                        // under the pointer when the condition changes.
                        let widest = Condition::ALL
                            .iter()
                            .map(|c| text_w(ui, c.label(), pt))
                            .fold(0.0, f32::max);
                        let head = crate::ui::menu_head_min(
                            ui,
                            egui::RichText::new(condition.label())
                                .size(pt)
                                .color(theme::text::muted()),
                            widest.ceil(),
                            4.0,
                            open,
                        );
                        if head.clicked() {
                            act = Some(Act::Menu(head.rect.left_bottom()));
                        }
                        ui.spacing_mut().item_spacing.x = 10.0;
                        let slider = crate::ui::slider(
                            ui,
                            SLIDER_W,
                            &mut severity,
                            f64::from(MIN_SEVERITY)..=1.0,
                        )
                        .on_hover_text("How strong the simulation is");
                        if slider.changed() {
                            act = Some(Act::Severity {
                                save: !slider.dragged(),
                            });
                        } else if slider.drag_stopped() {
                            act = Some(Act::Severity { save: true });
                        }
                        // A cell as wide as the widest reading, so the × holds still.
                        ui.spacing_mut().item_spacing.x = 6.0;
                        cell(ui, text_w(ui, "100%", pt).ceil() + 2.0, |ui| {
                            ui.label(
                                egui::RichText::new(format!("{:.0}%", severity * 100.0))
                                    .size(pt)
                                    .color(theme::text::dim()),
                            );
                        });
                        ui.spacing_mut().item_spacing.x = 10.0;
                        separator(ui);
                        if crate::ui::icon_button(ui, icon::X, 24.0, 13.0, false, true)
                            .on_hover_text("Stop simulating vision")
                            .clicked()
                        {
                            act = Some(Act::Close);
                        }
                    });
                });
            });
        match act {
            None => {}
            Some(Act::Close) => self.vision = None,
            Some(Act::Severity { save }) => {
                self.prefs
                    .vision
                    .severity
                    .insert(condition.key().into(), severity as f32);
                // Written when a drag ends, not on every frame of it.
                if save {
                    self.prefs.save();
                }
                ctx.request_repaint();
            }
            Some(Act::Menu(at)) => {
                if let Some(v) = &mut self.vision {
                    v.menu = !v.menu;
                    v.menu_at = at;
                    v.menu_opening = v.menu;
                }
            }
            Some(Act::Retarget) => {
                let frame = self.vision_target();
                if let Some(v) = &mut self.vision {
                    v.frame = frame;
                }
            }
        }
    }

    fn vision_menu(&mut self, ctx: &egui::Context) {
        let Some(head) = self.vision.as_ref().filter(|v| v.menu).map(|v| v.menu_at) else {
            return;
        };
        let at = head + egui::vec2(0.0, crate::ui::MENU_GAP);
        let current = self.prefs.vision.condition();
        let mut chosen = None;
        let menu = egui::Area::new(egui::Id::new("vision-menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ctx, |ui| {
                crate::ui::menu_frame(crate::ui::MENU_PAD).show(ui, |ui| {
                    ui.set_width(crate::ui::menu_inner_w(MENU_W, crate::ui::MENU_PAD));
                    ui.spacing_mut().item_spacing.y = 1.0;
                    for c in Condition::ALL {
                        // A hairline parts colour vision from sight.
                        if c == Condition::Blur {
                            crate::ui::menu_sep(ui);
                        }
                        let label = format!("{}  ·  {}", c.label(), c.gloss());
                        if crate::ui::menu_check(ui, &label, c == current, crate::ui::MENU_ROW_H) {
                            chosen = Some(c);
                        }
                    }
                });
            })
            .response;
        if let Some(c) = chosen {
            self.prefs.vision.condition = c.key().into();
            self.prefs.save();
        }
        let away = ctx.input(|i| i.pointer.any_click()) && !menu.contains_pointer();
        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if let Some(v) = &mut self.vision {
            if chosen.is_some() || (away && !v.menu_opening) || escape {
                v.menu = false;
            }
            v.menu_opening = false;
        }
    }

    /// Whether the condition menu is open, which an Escape closes before it
    /// reaches the canvas.
    pub(crate) fn vision_owns_escape(&self) -> bool {
        self.vision.as_ref().is_some_and(|v| v.menu)
    }

    /// The frame a simulation shows, for tests.
    #[cfg(test)]
    pub(crate) fn vision_frame(&self) -> Option<Option<NodeId>> {
        self.vision.as_ref().map(|v| v.frame)
    }
}

/// What the bar asked for this frame, acted on after it is drawn.
enum Act {
    Close,
    Menu(egui::Pos2),
    Retarget,
    /// The slider moved; `save` unless a drag is still under way.
    Severity {
        save: bool,
    },
}

/// The width `text` lays out at in the bar's type.
fn text_w(ui: &egui::Ui, text: &str, pt: f32) -> f32 {
    ui.painter()
        .layout_no_wrap(
            text.to_owned(),
            egui::FontId::proportional(pt),
            egui::Color32::PLACEHOLDER,
        )
        .size()
        .x
}

/// The device pixels `world`, a box on the page, covers on a canvas of `size`
/// showing `view`, clamped to the canvas, the far edges exclusive and rounded
/// out so the frame's own edge pixels are simulated.
fn device_region(world: Rect, view: Rect, size: (u32, u32)) -> [u32; 4] {
    if view.width() <= 0.0 || view.height() <= 0.0 {
        return [0, 0, 0, 0];
    }
    let sx = f64::from(size.0) / view.width();
    let sy = f64::from(size.1) / view.height();
    let px = |v: f64, max: u32| v.clamp(0.0, f64::from(max)) as u32;
    [
        px(((world.x0 - view.x0) * sx).floor(), size.0),
        px(((world.y0 - view.y0) * sy).floor(), size.1),
        px(((world.x1 - view.x0) * sx).ceil(), size.0),
        px(((world.y1 - view.y0) * sy).ceil(), size.1),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{Document, NodeKind, Operation, Transaction};

    /// A page with two frames far apart, a rect in the first.
    fn fixture(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let page = app.session.ids.mint();
        app.session.adopt_document(Document::new(page), None);
        let ids = &mut app.session.ids;
        let (near, far, dot) = (ids.mint(), ids.mint(), ids.mint());
        let frame = |id, x: f64, name: &str| Operation::CreateNode {
            id,
            parent: page,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(300.0, 200.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((x, 0.0))),
            name: Some(name.into()),
        };
        assert!(app.session.commit(Transaction(vec![
            frame(near, 0.0, "Near"),
            frame(far, 2000.0, "Far"),
            Operation::CreateNode {
                id: dot,
                parent: near,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 20.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((10.0, 10.0))),
                name: Some("Dot".into()),
            },
        ])));
        app.canvas_px = (800, 600);
        (app, near, far, dot)
    }

    /// `vision_region` for the canvas as the session's camera draws it.
    fn region(app: &mut OndinApp) -> Option<Seen> {
        let (camera, px) = (app.session.camera, app.canvas_px);
        app.vision_region(&camera, px)
    }

    /// The simulation takes the selection's frame, else the frame in view, and
    /// the contrast check and it never run together.
    /// flip: `toggle_vision` leaving the contrast check on → fails at "ends the
    /// check".
    #[test]
    fn the_simulation_takes_one_frame_and_ends_the_contrast_check() {
        let ctx = egui::Context::default();
        let (mut app, near, far, dot) = fixture(&ctx);
        app.session.selection.set_one(dot);
        app.toggle_contrast();
        assert!(app.contrast.is_some());
        app.toggle_vision();
        assert!(app.contrast.is_none(), "ends the check");
        assert_eq!(
            app.vision_frame(),
            Some(Some(near)),
            "the selection's frame"
        );
        app.toggle_contrast();
        assert!(app.vision.is_none(), "the check ends it");
        app.toggle_contrast();
        app.session.selection.clear();
        app.zoom_to(Some(Rect::new(1980.0, -20.0, 2320.0, 220.0)));
        app.toggle_vision();
        assert_eq!(app.vision_frame(), Some(Some(far)), "the frame in view");
        app.toggle_vision();
        assert!(app.vision.is_none());
    }

    /// The region is the frame's device pixels; on a page with no frame it is
    /// the whole canvas; the condition is the one last chosen at its own
    /// severity; and the scale is the zoom's.
    #[test]
    fn the_region_is_the_frame_or_the_whole_canvas() {
        let ctx = egui::Context::default();
        let (mut app, near, _, _) = fixture(&ctx);
        app.prefs.vision.condition = Condition::Blur.key().into();
        app.prefs
            .vision
            .severity
            .insert(Condition::Blur.key().into(), 0.25);
        app.zoom_to(Some(Rect::new(-50.0, -50.0, 350.0, 250.0)));
        app.toggle_vision();
        assert_eq!(app.vision_frame(), Some(Some(near)));
        let seen = region(&mut app).expect("on");
        assert_eq!(seen.effect, Condition::Blur.effect(0.25));
        assert_eq!(seen.scale, app.session.camera.zoom as f32);
        let [x0, y0, x1, y1] = seen.region;
        assert!(
            x0 > 0 && y0 > 0 && x1 < 800 && y1 < 600,
            "inside the canvas"
        );
        assert!(x1 > x0 && y1 > y0, "covers the frame");
        if let Some(v) = &mut app.vision {
            v.frame = None;
        }
        assert_eq!(region(&mut app).map(|s| s.region), Some([0, 0, 800, 600]));
    }

    /// Deleting the frame shown moves the simulation to the frame in view,
    /// rather than leaving it recolouring nothing.
    /// flip: `vision_region` retargeting on a page switch only → fails at
    /// "retargeted".
    #[test]
    fn a_deleted_frame_hands_over_to_the_frame_in_view() {
        let ctx = egui::Context::default();
        let (mut app, near, far, _) = fixture(&ctx);
        app.zoom_to(Some(Rect::new(-50.0, -50.0, 350.0, 250.0)));
        app.toggle_vision();
        assert_eq!(app.vision_frame(), Some(Some(near)));
        assert!(
            app.session
                .commit(Transaction(vec![Operation::DeleteNode { id: near }]))
        );
        let [x0, _, x1, _] = region(&mut app).expect("on").region;
        assert_eq!(app.vision_frame(), Some(Some(far)), "retargeted");
        assert!(x1 >= x0);
    }

    /// A box half in view covers the canvas pixels it reaches and stops at the
    /// canvas's edge; one wholly out of view covers none.
    #[test]
    fn a_frames_region_is_its_device_pixels_clamped_to_the_canvas() {
        let view = Rect::new(100.0, 50.0, 300.0, 150.0);
        let size = (400, 200);
        assert_eq!(
            device_region(Rect::new(150.0, 75.0, 400.0, 100.0), view, size),
            [100, 50, 400, 100]
        );
        let [x0, _, x1, _] = device_region(Rect::new(0.0, 0.0, 50.0, 40.0), view, size);
        assert_eq!(x0, x1, "out of view covers nothing");
        assert_eq!(
            device_region(Rect::new(150.25, 75.0, 160.5, 80.0), view, size),
            [100, 50, 121, 60],
            "edge pixels a frame partly covers are its"
        );
    }

    /// The condition menu hangs `MENU_GAP` under its head, as every dropdown
    /// hangs under its control.
    ///
    /// flip: `vision_menu` placing it 6 lower again → fails at the menu's top.
    #[test]
    fn the_menu_hangs_a_menu_gap_under_its_head() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, _n, _f, _d) = fixture(&ctx);
        app.canvas_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        app.toggle_vision();
        let run = |app: &mut OndinApp, events: Vec<egui::Event>| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 800.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| app.vision_chrome(ui.ctx()),
            )
            .shapes
        };
        run(&mut app, Vec::new());
        let shapes = run(&mut app, Vec::new());
        // The head, found by the condition it names.
        let label = app.prefs.vision.condition().label();
        let p = shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == label => {
                    Some(t.pos + t.galley.size() / 2.0)
                }
                _ => None,
            })
            .expect("the head's label");
        let press = |pressed| egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        run(&mut app, vec![egui::Event::PointerMoved(p)]);
        run(&mut app, vec![press(true)]);
        run(&mut app, vec![press(false)]);
        let v = app.vision.as_ref().expect("the mode is on");
        assert!(v.menu, "fixture: the press landed on the head");
        let head = v.menu_at;
        for _ in 0..3 {
            run(&mut app, Vec::new());
        }
        let menu = ctx
            .memory(|m| m.area_rect(egui::Id::new("vision-menu")))
            .expect("the menu is drawn");
        // An area lands on whole points.
        let want = head + egui::vec2(0.0, crate::ui::MENU_GAP);
        assert!(
            (menu.min - want).length() <= 0.5,
            "{menu:?} against {want:?}"
        );
    }

    /// The bar is as wide whichever condition it names, and its slider sets the
    /// shown condition's own severity, leaving the others'.
    ///
    /// flip: the head drawn with `menu_head` (no `min_lead`) → fails at "as wide".
    #[test]
    fn the_slider_sets_the_shown_condition_s_severity() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, _n, _f, _d) = fixture(&ctx);
        app.canvas_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        app.toggle_vision();
        let run = |app: &mut OndinApp, events: Vec<egui::Event>| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 800.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| app.vision_chrome(ui.ctx()),
            )
            .shapes
        };
        let bar_w = |app: &mut OndinApp, c: Condition| {
            app.prefs.vision.condition = c.key().into();
            run(app, Vec::new());
            run(app, Vec::new());
            ctx.memory(|m| m.area_rect(egui::Id::new("vision-bar")))
                .expect("the bar")
                .width()
        };
        let widths: Vec<f32> = [
            Condition::Tritanopia,
            Condition::Achromatopsia,
            Condition::Blur,
            Condition::Ghosting,
        ]
        .into_iter()
        .map(|c| bar_w(&mut app, c))
        .collect();
        assert!(
            widths.iter().all(|w| (w - widths[0]).abs() < 0.5),
            "as wide: {widths:?}"
        );

        // Blur at its default half; the cell reads it, and the slider ends 6pt
        // before the cell.
        app.prefs.vision.condition = Condition::Blur.key().into();
        run(&mut app, Vec::new());
        let shapes = run(&mut app, Vec::new());
        let cell = shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == "50%" => Some(t.pos),
                _ => None,
            })
            .expect("the severity reads 50%");
        let p = egui::pos2(cell.x - 6.0 - SLIDER_W + 1.0, cell.y + 6.0);
        let press = |pressed| egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        run(&mut app, vec![egui::Event::PointerMoved(p)]);
        run(&mut app, vec![press(true)]);
        run(&mut app, vec![press(false)]);
        let blur = app.prefs.vision.severity(Condition::Blur);
        assert!(blur < 0.2, "pressed at its start: {blur}");
        assert_eq!(
            app.prefs.vision.severity(Condition::Ghosting),
            0.5,
            "others kept"
        );
        assert_eq!(
            app.prefs.vision.severity(Condition::Tritanopia),
            1.0,
            "others kept"
        );
    }
}
