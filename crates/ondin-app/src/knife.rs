//! The Knife: drag a line through closed shapes and release to cut each into the
//! pieces it parts (`ondin_core::knife`). With a selection the line cuts only it;
//! with none, every unlocked shape under it. Shift draws the cut straight from the
//! press. The tool stays armed, as the shape builder does, and a cut is one step.
//!
//! The line shows as a dashed line in the snaps' red, with an anchor's marker
//! where it crosses each outline it will cut and a pill counting the shapes.

use crate::app::OndinApp;
use crate::theme::{color, icon};
use crate::ui;
use eframe::egui;
use ondin_core::kurbo::Point;

/// What the status line says when the tool is armed.
pub const HINT: &str = "Drag through shapes to cut them · Shift for a straight cut";

/// The least a free cut's line moves, in screen pt, before it takes a new point:
/// enough to keep a slow drag's line short, little enough to follow a curve.
const STEP_PT: f64 = 2.0;

/// The dashed line's dash and gap, in screen pt.
const DASH_PT: f32 = 5.0;
const GAP_PT: f32 = 4.0;

/// How far the count's pill sits right of and above the line's end, in screen pt.
const PILL_GAP: f32 = 10.0;

/// A cut being drawn.
pub(crate) struct KnifeDrag {
    /// The line so far, in world space, from the press.
    trail: Vec<Point>,
    /// Where a Shift-straight cut ends; `None` while the cut follows the pointer.
    straight: Option<Point>,
    /// Where the line crosses the outlines it would cut, and how many shapes those
    /// are: the preview, worked out again whenever the line changes.
    dots: Vec<Point>,
    shapes: usize,
}

impl KnifeDrag {
    /// The line the release cuts along.
    fn line(&self) -> Vec<Point> {
        match (self.straight, self.trail.first()) {
            (Some(end), Some(start)) => vec![*start, end],
            _ => self.trail.clone(),
        }
    }
}

impl OndinApp {
    /// Said when the tool is armed.
    pub(crate) fn knife_armed(&mut self) {
        self.session.info(HINT);
    }

    /// The knife's gesture: a press starts the line, a drag draws it, a release cuts.
    pub(crate) fn knife_input(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: egui::Rect,
        ppp: f32,
    ) {
        let (pressed, released, shift, at, moves) = ui.input(|i| {
            let moves: Vec<egui::Pos2> = i
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::PointerMoved(p) => Some(*p),
                    _ => None,
                })
                .collect();
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.modifiers.shift,
                i.pointer.interact_pos(),
                moves,
            )
        });
        let pressed = pressed && resp.contains_pointer();
        if pressed && let Some(p) = at {
            self.knife = Some(KnifeDrag {
                trail: vec![self.to_world(p, rect, ppp)],
                straight: None,
                dots: Vec::new(),
                shapes: 0,
            });
        } else if self.knife.is_some()
            && let Some(p) = at
        {
            let world: Vec<Point> = moves
                .into_iter()
                .chain([p])
                .map(|m| self.to_world(m, rect, ppp))
                .collect();
            self.knife_drag_to(&world, shift);
            ui.ctx().request_repaint();
        }
        if released {
            self.finish_knife();
        }
    }

    /// Carry the line through `world`'s points, straight from the press while
    /// `straight`, and work the preview out again if it changed.
    pub(crate) fn knife_drag_to(&mut self, world: &[Point], straight: bool) {
        let step = STEP_PT / self.points_per_world();
        let Some(drag) = self.knife.as_mut() else {
            return;
        };
        let before = drag.line();
        for p in world {
            if straight {
                drag.straight = Some(*p);
            } else {
                drag.straight = None;
                if drag.trail.last().is_none_or(|l| l.distance(*p) >= step) {
                    drag.trail.push(*p);
                }
            }
        }
        let line = drag.line();
        if line == before {
            return;
        }
        let (dots, shapes) = ondin_core::knife::preview(
            &self.session.doc,
            &self.session.resolved,
            self.session.page,
            self.session.selection.ids(),
            &line,
        );
        if let Some(drag) = self.knife.as_mut() {
            (drag.dots, drag.shapes) = (dots, shapes);
        }
    }

    /// Cut along the line drawn, one *Knife* step, and say what it did. With a
    /// selection, the selection becomes what it left: the uncut shapes and every piece.
    pub(crate) fn finish_knife(&mut self) {
        let Some(drag) = self.knife.take() else {
            return;
        };
        let line = drag.line();
        if line.len() < 2 {
            return;
        }
        let selection = self.session.selection.ids().to_vec();
        let made = ondin_core::knife::cut(
            &self.session.doc,
            &self.session.resolved,
            &mut self.session.ids,
            self.session.page,
            &selection,
            &line,
        );
        let (tx, cut) = match made {
            Ok(made) => made,
            Err(e) => return self.session.fail(format!("Could not cut: {e}")),
        };
        if tx.0.is_empty() {
            return self.session.info(match cut.instances {
                true => "Detach the instance to cut it",
                false => "Nothing to cut here",
            });
        }
        self.session.name_step("Knife", icon::KNIFE_CUT);
        if !self.commit_edit(tx) {
            return;
        }
        if !selection.is_empty() {
            let doc = &self.session.doc;
            let mut keep: Vec<_> = selection
                .into_iter()
                .filter(|id| doc.get(*id).is_some() && !cut.made.contains(id))
                .collect();
            keep.extend(cut.made.iter().copied());
            self.session.selection.set(keep);
        }
        self.session.info(format!(
            "Cut {} into {}",
            ui::count_as(cut.shapes, "shape", "shapes"),
            cut.pieces
        ));
    }

    /// The line being drawn, the crossings it will cut at, and the count.
    pub(crate) fn draw_knife(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        let Some(drag) = &self.knife else { return };
        let line: Vec<egui::Pos2> = drag
            .line()
            .iter()
            .map(|p| self.to_screen(*p, rect, ppp))
            .collect();
        let ink = egui::Stroke::new(1.5, color::MEASURE);
        painter.extend(egui::Shape::dashed_line(&line, ink, DASH_PT, GAP_PT));
        // Each crossing becomes an anchor of the pieces, so it wears an anchor's marker.
        for p in &drag.dots {
            crate::canvas::paint_anchor(painter, self.to_screen(*p, rect, ppp), color::POINT);
        }
        if drag.shapes > 0
            && let Some(end) = line.last()
        {
            ui::canvas_pill(
                painter,
                *end + egui::vec2(PILL_GAP, -PILL_GAP),
                egui::Align2::LEFT_BOTTOM,
                &format!("Cut {}", ui::count_as(drag.shapes, "shape", "shapes")),
                color::MEASURE,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Tool;
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    /// A headless app with a 100 × 60 rect at (0, 0) on the page, nothing
    /// selected, the knife armed.
    fn app_with_a_rect(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x4B1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let page = doc.first_page();
        let id = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: page,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(100.0, 60.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::IDENTITY),
            name: Some("Card".into()),
        }]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.choose_tool(Tool::Knife);
        (app, id)
    }

    /// Start a cut at `from`.
    fn press(app: &mut OndinApp, from: Point) {
        app.knife = Some(KnifeDrag {
            trail: vec![from],
            straight: None,
            dots: Vec::new(),
            shapes: 0,
        });
    }

    fn page_children(app: &OndinApp) -> Vec<NodeId> {
        let page = app.session.doc.first_page();
        app.session.doc.get(page).unwrap().children().to_vec()
    }

    /// A drag across the rect previews its two crossings and one shape, and the
    /// release cuts it into two paths named as it was, in one *Knife* step.
    #[test]
    fn a_drag_across_a_shape_previews_and_cuts_it() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_a_rect(&ctx);
        assert_eq!(app.session.status().text, HINT, "said on arming");
        press(&mut app, Point::new(40.0, -20.0));
        app.knife_drag_to(&[Point::new(45.0, 30.0), Point::new(50.0, 90.0)], false);
        let drag = app.knife.as_ref().expect("drawing");
        assert_eq!((drag.dots.len(), drag.shapes), (2, 1));
        let depth = app.session.history.undo_depth();
        app.finish_knife();
        assert!(app.knife.is_none());
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
        let step = app.session.history.done().last().expect("a step");
        assert_eq!(step.note.label, "Knife");
        let kids = page_children(&app);
        assert_eq!(kids.len(), 2);
        for id in kids {
            assert_eq!(app.session.doc.get(id).unwrap().name(), "Card");
        }
        assert_eq!(app.session.status().text, "Cut 1 shape into 2");
        assert_eq!(app.tool, Tool::Knife, "the tool stays armed");
    }

    /// With Shift the cut runs straight from the press to the pointer, whatever the
    /// drag wandered through; and a cut that crosses nothing says so.
    #[test]
    fn shift_cuts_straight_and_a_miss_says_so() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_a_rect(&ctx);
        press(&mut app, Point::new(-20.0, 30.0));
        app.knife_drag_to(&[Point::new(30.0, 200.0), Point::new(130.0, 30.0)], true);
        let line = app.knife.as_ref().expect("drawing").line();
        assert_eq!(line, vec![Point::new(-20.0, 30.0), Point::new(130.0, 30.0)]);
        app.finish_knife();
        assert_eq!(page_children(&app).len(), 2, "cut along the straight line");

        press(&mut app, Point::new(-20.0, 300.0));
        app.knife_drag_to(&[Point::new(200.0, 300.0)], false);
        app.finish_knife();
        assert_eq!(app.session.status().text, "Nothing to cut here");
    }

    /// A right-click mid-cut drops the line and cuts nothing.
    ///
    /// flip: `cancel_gesture` not taking `knife` → fails at "there was a cut to drop".
    #[test]
    fn a_right_click_drops_the_cut() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_a_rect(&ctx);
        press(&mut app, Point::new(40.0, -20.0));
        app.knife_drag_to(&[Point::new(50.0, 90.0)], false);
        assert!(app.cancel_gesture(&ctx), "there was a cut to drop");
        app.finish_knife();
        assert_eq!(page_children(&app).len(), 1, "nothing cut");
    }
}
