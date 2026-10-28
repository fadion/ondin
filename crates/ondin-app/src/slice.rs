//! Slices on the canvas: picking one with the Slice tool, and the dashed outlines
//! that stand in for a layer that draws nothing.
//!
//! A slice is picked by a 4 pt band along its edge (`geometry::contains_local`),
//! by its row in the layers tree, or by a click anywhere inside it with the Slice
//! tool on ([`OndinApp::slice_at`]). A marquee and select-all skip slices
//! (`OndinApp::may_select_in_bulk`).

use crate::app::OndinApp;
use crate::theme::color;
use crate::tools::Tool;
use eframe::egui;
use ondin_core::kurbo::Point;
use ondin_core::{NodeId, NodeKind};

/// How strong the outline of a slice that is neither hovered nor selected is while
/// the Slice tool is on, so what is already cut shows.
const FAINT: f32 = 0.45;

impl OndinApp {
    /// Every slice on the current page, bottom first.
    pub(crate) fn page_slices(&self) -> Vec<NodeId> {
        fn walk(doc: &ondin_core::Document, id: NodeId, out: &mut Vec<NodeId>) {
            let Some(node) = doc.get(id) else { return };
            for c in node.children() {
                if doc
                    .get(*c)
                    .is_some_and(|n| matches!(n.kind(), NodeKind::Slice { .. }))
                {
                    out.push(*c);
                }
                walk(doc, *c, out);
            }
        }
        let mut out = Vec::new();
        walk(&self.session.doc, self.session.page, &mut out);
        out
    }

    /// The topmost shown, unlocked slice whose box holds `world`: what a click with
    /// the Slice tool picks, where any other tool's click passes through it.
    pub(crate) fn slice_at(&self, world: Point) -> Option<NodeId> {
        let doc = &self.session.doc;
        let res = &self.session.resolved;
        self.page_slices().into_iter().rev().find(|id| {
            if !doc.shown_through(*id) || ondin_core::is_effectively_locked(doc, *id) {
                return false;
            }
            let (Some(world_t), Some(NodeKind::Slice { size })) =
                (res.world_transform(*id), doc.get(*id).map(|n| n.kind()))
            else {
                return false;
            };
            let p = world_t.inverse() * world;
            (0.0..=size.width).contains(&p.x) && (0.0..=size.height).contains(&p.y)
        })
    }

    /// The dashed outline of every slice that shows: in the selection blue when it
    /// is hovered or selected, and faintly for the rest while the Slice tool is on.
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
            let ink = match (lit, slicing) {
                (true, _) => color::SELECT,
                (false, true) => color::SELECT.gamma_multiply(FAINT),
                (false, false) => continue,
            };
            let (Some(world), Some(NodeKind::Slice { size })) = (
                self.session.preview_world_transform(id),
                self.session.doc.get(id).map(|n| n.kind()),
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
}
