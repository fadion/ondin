//! Slices on the canvas: picking one with the Slice tool, and the dashed outlines
//! that stand in for a layer that draws nothing.
//!
//! A slice is picked by a 4 pt band along its edge (`geometry::contains_local`),
//! by its row in the layers tree, or by a click anywhere inside it with the Slice
//! tool on ([`OndinApp::slice_at`]). Once selected, a press anywhere inside it
//! moves it ([`OndinApp::selected_slice_at`]). A marquee and select-all skip slices
//! (`OndinApp::may_select_in_bulk`).

use crate::app::OndinApp;
use crate::theme::color;
use crate::tools::Tool;
use eframe::egui;
use ondin_core::kurbo::Point;
use ondin_core::{NodeId, NodeKind};

/// How strong the outline of a slice that is neither hovered nor selected is, so
/// what is already cut shows.
const FAINT: f32 = 0.45;

impl OndinApp {
    /// Every slice on the current page, bottom first, from the walk the
    /// [`crate::app::FrameIndex`] memo makes once per revision: the canvas asks
    /// every frame.
    pub(crate) fn page_slices(&self) -> Vec<NodeId> {
        self.with_frames(|i| i.slices.clone())
    }

    /// The topmost shown, unlocked slice whose box holds `world`: what a click with
    /// the Slice tool picks, where any other tool's click passes through it.
    pub(crate) fn slice_at(&self, world: Point) -> Option<NodeId> {
        self.page_slices()
            .into_iter()
            .rev()
            .find(|id| self.slice_holds(*id, world))
    }

    /// The topmost selected slice whose box holds `world`: a press there moves it,
    /// where an unselected slice's inside passes the press through.
    pub(crate) fn selected_slice_at(&self, world: Point) -> Option<NodeId> {
        self.page_slices()
            .into_iter()
            .rev()
            .find(|id| self.session.selection.contains(*id) && self.slice_holds(*id, world))
    }

    /// Whether `world` lies in slice `id`'s box, shown and unlocked.
    fn slice_holds(&self, id: NodeId, world: Point) -> bool {
        let doc = &self.session.doc;
        if !doc.shown_through(id) || ondin_core::is_effectively_locked(doc, id) {
            return false;
        }
        let (Some(world_t), Some(NodeKind::Slice { size })) = (
            self.session.resolved.world_transform(id),
            doc.get(id).map(|n| n.kind()),
        ) else {
            return false;
        };
        let p = world_t.inverse() * world;
        (0.0..=size.width).contains(&p.x) && (0.0..=size.height).contains(&p.y)
    }

    /// *Slice selection*: a slice fitted to the selection (`build::slice_selection`),
    /// selected afterwards. A selection of slices only cuts nothing new.
    pub(crate) fn slice_selection(&mut self) {
        let ids: Vec<NodeId> = self.session.selection.ids().to_vec();
        let doc = &self.session.doc;
        if ids
            .iter()
            .all(|i| matches!(doc.get(*i).map(|n| n.kind()), Some(NodeKind::Slice { .. })))
        {
            return;
        }
        let id = self.session.ids.mint();
        let Some(tx) =
            ondin_core::build::slice_selection(&self.session.doc, &self.session.resolved, &ids, id)
        else {
            return;
        };
        self.session
            .name_step("Slice selection", crate::theme::icon::KNIFE);
        if self.session.commit(tx) {
            self.session.selection.set_one(id);
            self.session
                .info("Sliced the selection; it exports PNG at 1×");
        }
    }

    /// The dashed outline of every slice that shows, at its live size: in the
    /// selection blue when it is hovered or selected, and faintly for the rest.
    pub(crate) fn draw_slices(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        resp: &egui::Response,
    ) {
        let slicing = self.tool == Tool::Slice;
        let hovered = match resp.hover_pos() {
            Some(p) if self.drag.is_none() && slicing => self.slice_at(self.to_world(p, rect, ppp)),
            Some(_) => self.hover_target(ui, rect, ppp, resp),
            None => None,
        };
        for id in self.page_slices() {
            if !self.session.doc.shown_through(id) {
                continue;
            }
            let lit = hovered == Some(id) || self.session.selection.contains(id);
            let ink = if lit {
                color::SELECT
            } else {
                color::SELECT.gamma_multiply(FAINT)
            };
            // The display node, so a resize or a W/H scrub shows as it happens.
            let (Some(world), Some(NodeKind::Slice { size })) = (
                self.session.preview_world_transform(id),
                self.session.display_node(id).map(|n| n.kind()),
            ) else {
                continue;
            };
            let corners = [
                Point::ZERO,
                Point::new(size.width, 0.0),
                Point::new(size.width, size.height),
                Point::new(0.0, size.height),
                Point::ZERO,
            ];
            let pts: Vec<egui::Pos2> = corners
                .iter()
                .map(|c| self.to_screen(world * *c, rect, ppp))
                .collect();
            painter.extend(egui::Shape::dashed_line(
                &pts,
                egui::Stroke::new(1.0, ink),
                4.0,
                3.0,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::Drag;
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{Document, ExportFormat, ExportScale, Operation, Transaction};

    /// A headless app on a white 400×300 frame at the origin holding a 100×100
    /// rect at (50, 50), at 100%.
    fn app_with_a_frame(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (frame, rect) = (ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 300.0),
                    corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: rect,
                parent: frame,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 100.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((50.0, 50.0))),
                name: None,
            },
        ]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.canvas_px = (800, 600);
        (app, frame, rect)
    }

    /// Drag a slice out with the tool, as the canvas does on release.
    fn cut(app: &mut OndinApp, from: Point, to: Point) -> NodeId {
        app.choose_tool(Tool::Slice);
        app.begin_create(from);
        let Drag::Create { id, start, .. } = app.drag else {
            panic!("the drag began");
        };
        app.commit_create(id, Tool::Slice, start, to);
        id
    }

    /// A cut lands in the frame on whole pixels with one PNG 1× export, selected,
    /// one *Add slice* step, and the tool stays the Slice tool.
    #[test]
    fn a_cut_lands_on_whole_pixels_with_a_png_export_and_the_tool_stays_on() {
        let ctx = egui::Context::default();
        let (mut app, frame, _) = app_with_a_frame(&ctx);
        let id = cut(&mut app, Point::new(10.4, 20.6), Point::new(110.2, 70.7));
        let node = app.session.doc.get(id).expect("cut");
        assert_eq!(node.parent(), Some(frame));
        assert_eq!(
            node.kind(),
            &NodeKind::Slice {
                size: Size::new(100.0, 50.0)
            }
        );
        assert_eq!(node.transform(), Affine::translate((10.0, 21.0)));
        let specs = ondin_core::item::values(node.exports());
        assert_eq!(
            specs,
            vec![ondin_core::ExportSpec::new(
                ExportFormat::Png,
                ExportScale::Times(1.0)
            )]
        );
        assert_eq!(app.session.selection.single(), Some(id));
        assert_eq!(app.tool, Tool::Slice, "slices are cut in runs");
        let step = app.session.history.done().last().expect("one step");
        assert_eq!(step.note.label, "Add slice");
    }

    /// With the Slice tool on, a point inside a slice picks it; select-all and the
    /// marquee's filter leave it out.
    /// flip: `may_select_in_bulk` letting a slice through → fails at "select-all".
    #[test]
    fn the_tool_picks_inside_a_slice_and_select_all_skips_it() {
        let ctx = egui::Context::default();
        let (mut app, _, rect) = app_with_a_frame(&ctx);
        let id = cut(&mut app, Point::new(20.0, 20.0), Point::new(120.0, 120.0));
        assert_eq!(app.slice_at(Point::new(70.0, 70.0)), Some(id));
        assert_eq!(app.slice_at(Point::new(300.0, 200.0)), None);

        app.choose_tool(Tool::Select);
        app.select_all();
        let all = app.session.selection.ids().to_vec();
        assert!(all.contains(&rect));
        assert!(
            !all.contains(&id),
            "select-all leaves the slice out: {all:?}"
        );
    }

    /// *Slice selection* cuts a slice over the selected rect and selects it; over a
    /// selection of slices only it does nothing.
    #[test]
    fn slice_selection_selects_its_slice_and_does_not_slice_a_slice() {
        let ctx = egui::Context::default();
        let (mut app, _, rect) = app_with_a_frame(&ctx);
        app.session.selection.set_one(rect);
        app.slice_selection();
        let id = app.session.selection.single().expect("the new slice");
        assert!(matches!(
            app.session.doc.get(id).map(|n| n.kind()),
            Some(NodeKind::Slice { .. })
        ));
        assert_eq!(
            app.session.resolved.world_bounds(id),
            app.session.resolved.world_bounds(rect)
        );

        let steps = app.session.history.done().len();
        app.slice_selection();
        assert_eq!(
            app.session.history.done().len(),
            steps,
            "a slice on a slice"
        );
    }

    /// The Slice tool picks no hidden and no locked slice.
    /// flip: `slice_holds` without its `shown_through` test → fails at "a hidden
    /// slice is not picked".
    #[test]
    fn the_tool_passes_over_a_hidden_or_locked_slice() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = app_with_a_frame(&ctx);
        let id = cut(&mut app, Point::new(20.0, 20.0), Point::new(120.0, 120.0));
        let inside = Point::new(70.0, 70.0);
        assert_eq!(app.slice_at(inside), Some(id), "fixture");
        app.session.commit(Transaction(vec![Operation::SetVisible {
            id,
            visible: false,
        }]));
        assert_eq!(app.slice_at(inside), None, "a hidden slice is not picked");
        app.session.commit(Transaction(vec![
            Operation::SetVisible { id, visible: true },
            Operation::SetLocked { id, locked: true },
        ]));
        assert_eq!(app.slice_at(inside), None, "nor a locked one");
        assert_eq!(app.selected_slice_at(inside), None);
    }

    const RECT: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::Pos2::new(800.0, 600.0),
    };

    /// The outlines skip a hidden slice and draw a selected one at its live size.
    /// flip: `draw_slices` reading the committed node instead of `display_node` →
    /// fails at "at its previewed width".
    #[test]
    fn the_outlines_skip_hidden_slices_and_follow_a_preview() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = app_with_a_frame(&ctx);
        let shown = cut(&mut app, Point::new(20.0, 20.0), Point::new(120.0, 120.0));
        let hidden = cut(&mut app, Point::new(200.0, 150.0), Point::new(260.0, 200.0));
        app.session.commit(Transaction(vec![Operation::SetVisible {
            id: hidden,
            visible: false,
        }]));
        app.session.selection.set_one(shown);
        // Before the preview, which a tool change takes back.
        app.choose_tool(Tool::Select);
        app.session
            .set_preview(&Transaction(vec![Operation::SetGeometry {
                id: shown,
                geometry: ondin_core::GeometryPatch::Size(Size::new(200.0, 100.0)),
            }]));
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(RECT),
                ..Default::default()
            },
            |ui| {
                let resp = ui.allocate_rect(RECT, egui::Sense::hover());
                app.draw_slices(ui, ui.painter(), RECT, 1.0, &resp);
            },
        );
        let segments: Vec<(egui::Pos2, egui::Pos2, egui::Color32)> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::LineSegment { points, stroke } => {
                    Some((points[0], points[1], stroke.color))
                }
                _ => None,
            })
            .collect();
        let lit_right = segments
            .iter()
            .filter(|s| s.2 == color::SELECT)
            .flat_map(|s| [s.0.x, s.1.x])
            .fold(f32::MIN, f32::max);
        let want = app.to_screen(Point::new(220.0, 20.0), RECT, 1.0).x;
        assert!(
            (lit_right - want).abs() < 1.0,
            "at its previewed width: {lit_right} vs {want}"
        );
        let hidden_box = egui::Rect::from_two_pos(
            app.to_screen(Point::new(199.0, 149.0), RECT, 1.0),
            app.to_screen(Point::new(261.0, 201.0), RECT, 1.0),
        );
        assert!(
            !segments
                .iter()
                .any(|s| s.2 != color::SELECT && hidden_box.contains(s.0)),
            "no outline for the hidden slice"
        );
    }

    /// With the Slice tool still on, the handles of the slice just cut resize it
    /// instead of cutting another.
    /// flip: `normal_mode_input` without its `on_handle` arm → fails at "resized,
    /// not cut again".
    #[test]
    fn the_slice_tool_resizes_by_the_handles_it_shows() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = app_with_a_frame(&ctx);
        let id = cut(&mut app, Point::new(20.0, 20.0), Point::new(120.0, 120.0));
        let frame = |app: &mut OndinApp, events| {
            let time = ctx.input(|i| i.time) + 0.1;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(RECT),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| app.canvas_ui(ui),
            );
        };
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let corner = app.to_screen(Point::new(120.0, 120.0), RECT, 1.0);
        let to = app.to_screen(Point::new(160.0, 150.0), RECT, 1.0);
        let slices = |app: &OndinApp| app.page_slices().len();
        let before = slices(&app);
        frame(&mut app, vec![egui::Event::PointerMoved(corner)]);
        frame(&mut app, vec![button(corner, true)]);
        for step in 1..=6 {
            let p = corner + (to - corner) * (step as f32 / 6.0);
            frame(&mut app, vec![egui::Event::PointerMoved(p)]);
        }
        frame(&mut app, vec![button(to, false)]);
        frame(&mut app, Vec::new());
        assert_eq!(slices(&app), before, "resized, not cut again");
        assert_eq!(
            app.session
                .resolved
                .world_bounds(id)
                .map(|b| (b.width(), b.height())),
            Some((140.0, 130.0))
        );
    }
}
