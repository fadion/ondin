//! Colour-vision simulation mode: one frame of the canvas shown as someone with
//! a colour-vision deficiency sees it (`ondin_render::vision`), the rest of the
//! canvas and every piece of edit chrome as they are.
//!
//! *View ▸ Simulate colour blindness* or the palette, no chord. View state, as
//! the contrast check is: off when a document opens, never saved, never
//! exported, never drawn in present mode. The two modes do not run together,
//! so their bars never stack: starting one ends the other. The deficiency last
//! chosen is a preference (`prefs.vision`).

use ondin_core::NodeId;
use ondin_core::kurbo::Rect;
use ondin_render::vision::Deficiency;

use crate::app::OndinApp;
use crate::contrast::{BAR_H, bar_frame, scope_chip, separator, top_level};
use crate::theme::{self, color, icon};

/// The mode while it is on (`OndinApp::vision`).
pub(crate) struct Vision {
    /// The page it started on; another page takes the frame in view there.
    page: NodeId,
    /// The frame recoloured, or `None` for the whole canvas on a page with none.
    frame: Option<NodeId>,
    menu: bool,
    /// The deficiency head's bottom left, which the menu hangs from.
    menu_at: egui::Pos2,
    /// Set on the frame the head is clicked, whose click is not a click away.
    menu_opening: bool,
}

impl OndinApp {
    /// *View ▸ Simulate colour blindness*: start the mode on the selection's
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

    /// The deficiency the canvas is seen through and the texels it covers
    /// (`x0, y0, x1, y1`) of the texture `camera` draws at `px`, or `None` while
    /// the mode is off. Runs before the canvas; a page switch, or its frame going
    /// away, moves the mode to the frame in view.
    pub(crate) fn vision_region(
        &mut self,
        camera: &crate::view::Camera,
        px: (u32, u32),
    ) -> Option<(Deficiency, [u32; 4])> {
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
        let deficiency = Deficiency::from_key(&self.prefs.vision);
        let (w, h) = px;
        let all = [0, 0, w, h];
        let Some(frame) = v.frame else {
            return Some((deficiency, all));
        };
        let vp = camera.viewport(px);
        let region = self
            .session
            .resolved
            .world_bounds(frame)
            .map_or([0, 0, 0, 0], |b| device_region(b, vp.view, (w, h)));
        Some((deficiency, region))
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
        let deficiency = Deficiency::from_key(&self.prefs.vision);
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
                                        .size(12.0)
                                        .color(color::text()),
                                );
                            }
                        }
                        // The head's ground carries its own inset, so the gaps
                        // either side of it are short by that much.
                        ui.spacing_mut().item_spacing.x = 10.0 - crate::ui::HEAD_LEAD;
                        separator(ui);
                        ui.spacing_mut().item_spacing.x = 10.0 - crate::ui::HEAD_PAD;
                        let head = crate::ui::menu_head(
                            ui,
                            egui::RichText::new(deficiency.label())
                                .size(12.0)
                                .color(theme::text::muted()),
                            4.0,
                            open,
                        );
                        if head.clicked() {
                            act = Some(Act::Menu(head.rect.left_bottom()));
                        }
                        ui.spacing_mut().item_spacing.x = 10.0;
                        separator(ui);
                        if crate::ui::icon_button(ui, icon::X, 24.0, 13.0, false, true)
                            .on_hover_text("Stop simulating colour blindness")
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
        const ITEM_H: f32 = 26.0;
        let at = head + egui::vec2(0.0, crate::ui::MENU_GAP);
        let current = Deficiency::from_key(&self.prefs.vision);
        let mut chosen = None;
        let menu = egui::Area::new(egui::Id::new("vision-menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ctx, |ui| {
                crate::ui::menu_frame(5.0).show(ui, |ui| {
                    ui.set_width(crate::ui::menu_inner_w(220.0, 5.0));
                    ui.spacing_mut().item_spacing.y = 1.0;
                    for d in Deficiency::ALL {
                        let label = format!("{}  ·  {}", d.label(), d.gloss());
                        if crate::ui::menu_check(ui, &label, d == current, ITEM_H) {
                            chosen = Some(d);
                        }
                    }
                });
            })
            .response;
        if let Some(d) = chosen {
            self.prefs.vision = d.key().into();
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

    /// Whether the deficiency menu is open, which an Escape closes before it
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
    fn region(app: &mut OndinApp) -> Option<(Deficiency, [u32; 4])> {
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
    /// the whole canvas; and the deficiency is the one last chosen.
    #[test]
    fn the_region_is_the_frame_or_the_whole_canvas() {
        let ctx = egui::Context::default();
        let (mut app, near, _, _) = fixture(&ctx);
        app.prefs.vision = Deficiency::Tritanopia.key().into();
        app.zoom_to(Some(Rect::new(-50.0, -50.0, 350.0, 250.0)));
        app.toggle_vision();
        assert_eq!(app.vision_frame(), Some(Some(near)));
        let (d, [x0, y0, x1, y1]) = region(&mut app).expect("on");
        assert_eq!(d, Deficiency::Tritanopia);
        assert!(
            x0 > 0 && y0 > 0 && x1 < 800 && y1 < 600,
            "inside the canvas"
        );
        assert!(x1 > x0 && y1 > y0, "covers the frame");
        if let Some(v) = &mut app.vision {
            v.frame = None;
        }
        assert_eq!(region(&mut app).map(|r| r.1), Some([0, 0, 800, 600]));
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
        let (_, [x0, _, x1, _]) = region(&mut app).expect("on");
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

    /// The deficiency menu hangs `MENU_GAP` under its head, as every dropdown
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
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 800.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| app.vision_chrome(ui.ctx()),
            );
        };
        run(&mut app, Vec::new());
        run(&mut app, Vec::new());
        let bar = ctx
            .memory(|m| m.area_rect(egui::Id::new("vision-bar")))
            .expect("the bar is drawn");
        // The head sits right of the bar's centre: the eye, the scope and a
        // separator come before it.
        let p = egui::pos2(bar.center().x + 40.0, bar.center().y);
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
}
