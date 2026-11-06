//! Mirror while drawing (`docs/design/app-pen-and-nodes.md`, *Mirror while
//! drawing*): the pen and the pencil draw the other half of a new path as they go,
//! reflected across an axis through the frame being drawn in.
//!
//! The halves join where they meet the axis. A run that starts and ends on it
//! closes as one symmetric path; one end on it makes one open path joined there;
//! neither leaves two subpaths in the one layer. *Both axes* is the vertical rule
//! then the horizontal one over what that made, so a quarter drawn from axis to
//! axis closes into a whole.

use crate::app::OndinApp;
use crate::preview::PenSubpath;
use crate::theme::color;
use crate::ui;
use eframe::egui;
use ondin_core::kurbo::{Affine, Point};

/// Which axes the pen and the pencil mirror across.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Axes {
    #[default]
    Off,
    Vertical,
    Horizontal,
    Both,
}

impl Axes {
    /// The flyout's rows, in order.
    pub(crate) const ALL: [Axes; 4] = [Axes::Off, Axes::Vertical, Axes::Horizontal, Axes::Both];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Axes::Off => "Off",
            Axes::Vertical => "Vertical axis",
            Axes::Horizontal => "Horizontal axis",
            Axes::Both => "Both axes",
        }
    }

    pub(crate) fn vertical(self) -> bool {
        matches!(self, Axes::Vertical | Axes::Both)
    }

    pub(crate) fn horizontal(self) -> bool {
        matches!(self, Axes::Horizontal | Axes::Both)
    }
}

/// The document's mirror: the axes, and where they cross once dragged there.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Mirror {
    pub(crate) axes: Axes,
    /// `None` until an axis is dragged: the middle of the frame being drawn in,
    /// or of the view on open canvas.
    pub(crate) at: Option<Point>,
}

/// The reflection across the vertical line `x = c` or the horizontal `y = c`.
fn reflection(vertical: bool, c: f64) -> Affine {
    match vertical {
        true => Affine::new([-1.0, 0.0, 0.0, 1.0, 2.0 * c, 0.0]),
        false => Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, 2.0 * c]),
    }
}

/// `drawn` and its mirror images across `axes` crossing at `at`, joined where an
/// end lies within `tol` of an axis (pulled onto it exactly).
pub(crate) fn mirrored(drawn: PenSubpath, axes: Axes, at: Point, tol: f64) -> Vec<PenSubpath> {
    let mut subs = vec![drawn];
    if axes.vertical() {
        subs = subs
            .into_iter()
            .flat_map(|s| across(s, true, at.x, tol))
            .collect();
    }
    if axes.horizontal() {
        subs = subs
            .into_iter()
            .flat_map(|s| across(s, false, at.y, tol))
            .collect();
    }
    subs
}

/// One subpath mirrored across one axis, by the joining rule.
fn across(mut s: PenSubpath, vertical: bool, c: f64, tol: f64) -> Vec<PenSubpath> {
    let reflect = reflection(vertical, c);
    let image = |s: &PenSubpath| {
        let mut one = [s.clone()];
        crate::tools::map_subpaths(&mut one, reflect);
        let [m] = one;
        m
    };
    let n = s.anchors.len();
    if s.closed || n < 2 {
        let m = image(&s);
        return vec![s, m];
    }
    let off = |p: Point| match vertical {
        true => (p.x - c).abs(),
        false => (p.y - c).abs(),
    };
    let onto = |p: Point| match vertical {
        true => Point::new(c, p.y),
        false => Point::new(p.x, c),
    };
    let start_on = off(s.anchors[0].at) <= tol;
    let end_on = off(s.anchors[n - 1].at) <= tol;
    if start_on {
        s.anchors[0].at = onto(s.anchors[0].at);
    }
    if end_on {
        s.anchors[n - 1].at = onto(s.anchors[n - 1].at);
    }
    // The image run backwards: from the end's image to the start's.
    let mut back = image(&s);
    back.reverse();
    match (start_on, end_on) {
        (false, false) => {
            let m = image(&s);
            vec![s, m]
        }
        (_, true) => {
            let mut anchors = s.anchors;
            let b = back.anchors;
            if let Some(last) = anchors.last_mut() {
                last.out = b[0].out;
            }
            anchors.extend_from_slice(&b[1..]);
            if start_on {
                // The image of the start comes round onto the start: one anchor.
                if let Some(tail) = anchors.pop() {
                    anchors[0].back = tail.back;
                }
            }
            vec![PenSubpath {
                anchors,
                closed: start_on,
            }]
        }
        (true, false) => {
            let mut anchors = back.anchors;
            if let Some(last) = anchors.last_mut() {
                last.out = s.anchors[0].out;
            }
            anchors.extend_from_slice(&s.anchors[1..]);
            vec![PenSubpath {
                anchors,
                closed: false,
            }]
        }
    }
}

/// How near an axis, in screen pt, an end is taken as on it.
pub(crate) const ON_AXIS_PT: f64 = 8.0;

/// The grab pill at an axis's end, in screen pt.
const PILL: egui::Vec2 = egui::vec2(16.0, 10.0);

impl OndinApp {
    /// The axes and where they cross, while mirroring is on: the dragged point, or
    /// the middle of the frame under `near` (the run's first point while one is
    /// drawn), or of the view.
    pub(crate) fn mirror_axes(&self, near: Option<Point>) -> Option<(Axes, Point)> {
        let m = self.session.mirror;
        if m.axes == Axes::Off {
            return None;
        }
        let near = self.drawing_from().or(near);
        let at =
            m.at.or_else(|| {
                let (frame, _) = self.draw_target(near?);
                (frame != self.session.page)
                    .then(|| self.session.resolved.world_bounds(frame))
                    .flatten()
                    .map(|b| b.center())
            })
            .unwrap_or(self.session.camera.center);
        Some((m.axes, at))
    }

    /// Where the run being drawn began, pen or pencil.
    fn drawing_from(&self) -> Option<Point> {
        match (&self.pen, &self.pencil) {
            (Some(p), _) if p.editing.is_none() => p.anchors.first().map(|a| a.at),
            (_, Some(s)) => s.origin(),
            _ => None,
        }
    }

    /// The new run `drawn` as the mirror makes it, in world space; just `drawn`
    /// with the mirror off.
    pub(crate) fn mirror_run(&self, drawn: PenSubpath) -> Vec<PenSubpath> {
        let near = drawn.anchors.first().map(|a| a.at);
        match self.mirror_axes(near) {
            Some((axes, at)) => {
                let tol = ON_AXIS_PT / self.points_per_world();
                mirrored(drawn, axes, at, tol)
            }
            None => vec![drawn],
        }
    }

    /// `p` pulled onto an axis it lies within [`ON_AXIS_PT`] of, and whether it
    /// was: where a pen anchor snaps.
    pub(crate) fn snap_to_axis(&self, p: Point) -> (Point, bool) {
        let Some((axes, at)) = self.mirror_axes(Some(p)) else {
            return (p, false);
        };
        let tol = ON_AXIS_PT / self.points_per_world();
        let mut q = p;
        let mut on = false;
        if axes.vertical() && (p.x - at.x).abs() <= tol {
            q.x = at.x;
            on = true;
        }
        if axes.horizontal() && (p.y - at.y).abs() <= tol {
            q.y = at.y;
            on = true;
        }
        (q, on)
    }

    /// The words the status line keeps while the pen or the pencil mirrors.
    pub(crate) fn mirror_status(&self) -> Option<String> {
        let axes = self.session.mirror.axes;
        let across = match axes {
            Axes::Off => return None,
            Axes::Vertical => "the vertical axis",
            Axes::Horizontal => "the horizontal axis",
            Axes::Both => "both axes",
        };
        let close = match axes {
            Axes::Both => "draw from one axis to the other to close the shape",
            _ => "start and end on the axis to close the shape",
        };
        Some(format!("Mirroring across {across} · {close}"))
    }

    /// Fix the axes where a new run begins at `p` when none was dragged and no frame
    /// is under it, so the view's middle they default to stays put while the run
    /// is drawn, however the view then moves. *Reset axis* lets them follow again.
    pub(crate) fn pin_mirror_axes(&mut self, p: Point) {
        let m = self.session.mirror;
        if m.axes != Axes::Off && m.at.is_none() && self.draw_target(p).0 == self.session.page {
            self.session.mirror.at = Some(self.session.camera.center);
        }
    }

    /// Whether the run in flight extends a path, which the mirror leaves alone.
    fn extending(&self) -> bool {
        self.pen.as_ref().is_some_and(|p| p.editing.is_some())
            || self.pencil.as_ref().is_some_and(|s| s.extends())
    }

    /// Paint the axes and their grab pills while the pen or the pencil is armed and
    /// mirroring, and the mirrored half of the run being drawn at 45%. Dragging a
    /// pill moves its axis; a double-click puts the axes back.
    pub(crate) fn mirror_chrome(
        &mut self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
    ) {
        if !matches!(
            self.tool,
            crate::tools::Tool::Pen | crate::tools::Tool::Pencil
        ) || self.extending()
        {
            return;
        }
        let hover = ui
            .ctx()
            .pointer_hover_pos()
            .map(|p| self.to_world(p, rect, ppp));
        let Some((axes, at)) = self.mirror_axes(hover) else {
            return;
        };
        let (frame, _) = self.draw_target(at);
        // Through the frame being drawn in; across the view on open canvas.
        let span = match frame != self.session.page {
            true => self.session.resolved.world_bounds(frame),
            false => None,
        }
        .map(|b| self.world_rect_to_screen(b, rect, ppp))
        .unwrap_or(rect);
        let at_s = self.to_screen(at, rect, ppp);
        let ink = color::accent_400();
        let line = egui::Stroke::new(1.0, ink);
        let mut pills = Vec::new();
        if axes.vertical() {
            let x = at_s.x;
            painter.extend(egui::Shape::dashed_line(
                &[egui::pos2(x, span.top()), egui::pos2(x, span.bottom())],
                line,
                6.0,
                4.0,
            ));
            pills.push((
                true,
                egui::Rect::from_center_size(egui::pos2(x, span.top() + 2.0 + PILL.y / 2.0), PILL),
            ));
        }
        if axes.horizontal() {
            let y = at_s.y;
            painter.extend(egui::Shape::dashed_line(
                &[egui::pos2(span.left(), y), egui::pos2(span.right(), y)],
                line,
                6.0,
                4.0,
            ));
            let r = egui::Rect::from_center_size(
                egui::pos2(span.left() + 2.0 + PILL.x / 2.0, y),
                egui::vec2(PILL.y, PILL.x),
            );
            pills.push((false, r));
        }
        for (vertical, r) in pills {
            painter.rect_filled(r, egui::CornerRadius::same(3), ink);
            let id = ui.id().with(("mirror-axis", vertical));
            let resp = ui
                .interact(r.expand(3.0), id, egui::Sense::click_and_drag())
                .on_hover_cursor(match vertical {
                    true => egui::CursorIcon::ResizeHorizontal,
                    false => egui::CursorIcon::ResizeVertical,
                })
                .on_hover_text("Drag to move the axis · double-click to put it back");
            if ui::double_clicked(ui, &resp) {
                self.session.mirror.at = None;
            } else if resp.dragged()
                && let Some(p) = resp.interact_pointer_pos()
            {
                let w = self.to_world(p, rect, ppp);
                let mut moved = self.session.mirror.at.unwrap_or(at);
                match vertical {
                    true => moved.x = w.x,
                    false => moved.y = w.y,
                }
                self.session.mirror.at = Some(moved);
            }
        }
        // The other half of the pen's run, as it will land.
        if let Some(pen) = self.pen.as_ref().filter(|p| p.editing.is_none())
            && pen.anchors.len() >= 2
        {
            let drawn = PenSubpath {
                anchors: pen.anchors.clone(),
                closed: false,
            };
            let ghost = egui::Stroke::new(
                ((crate::canvas::PEN_STROKE_WIDTH * self.session.camera.zoom) as f32 / ppp)
                    .max(1.0),
                ui::color_to_egui(crate::canvas::PEN_STROKE_COLOR).gamma_multiply(0.45),
            );
            // The drawn half is in the run too, under the pen's own full stroke.
            for sub in &self.mirror_run(drawn) {
                let outline = crate::tools::pen_outline(&sub.anchors, sub.closed);
                for line in self.flatten_to_screen(&outline, rect, ppp) {
                    painter.add(line.stroked(ghost));
                }
            }
        }
    }

    /// The path bar's mirror button and its flyout: the axes, ticked like the View
    /// menu's rows, and *Reset axis*.
    pub(crate) fn mirror_flyout(&mut self, ctx: &egui::Context, button: egui::Rect) {
        let id = egui::Id::new("mirror-flyout");
        let current = self.session.mirror.axes;
        let mut pick = None;
        let mut reset = false;
        let card = ui::flyout(id, button, 200.0)
            .order(egui::Order::Foreground)
            .show(ctx, (Axes::ALL.len() + 1, 1), |ui| {
                for axes in Axes::ALL {
                    if ui::menu_check(ui, axes.label(), axes == current, ui::MENU_ROW_H) {
                        pick = Some(axes);
                    }
                }
                ui::menu_sep(ui);
                let placed = self.session.mirror.at.is_some();
                let row = ui::MenuRow {
                    enabled: placed,
                    ..ui::MenuRow::new("", "Reset axis")
                };
                let resp = ui::menu_row(ui, row, ui::MENU_ROW_H);
                let resp = match placed {
                    true => resp,
                    false => resp.on_hover_text("The axis is where it starts"),
                };
                reset = placed && resp.clicked();
            });
        if let Some(axes) = pick {
            self.session.mirror.axes = axes;
            self.mirror_open = false;
        }
        if reset {
            self.session.mirror.at = None;
            self.mirror_open = false;
        }
        let pressed_elsewhere = ctx.input(|i| i.pointer.any_pressed())
            && !card.contains_pointer()
            && !ctx
                .pointer_interact_pos()
                .is_some_and(|p| button.contains(p));
        if pressed_elsewhere {
            self.mirror_open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::PenAnchor;

    fn run(points: &[(f64, f64)]) -> PenSubpath {
        PenSubpath {
            anchors: points
                .iter()
                .map(|&(x, y)| PenAnchor::corner(Point::new(x, y)))
                .collect(),
            closed: false,
        }
    }

    fn ats(s: &PenSubpath) -> Vec<(f64, f64)> {
        s.anchors.iter().map(|a| (a.at.x, a.at.y)).collect()
    }

    /// Starting and ending on the axis closes one symmetric path, the joins
    /// counted once.
    #[test]
    fn both_ends_on_the_axis_close_one_path() {
        let got = mirrored(
            run(&[(100.0, 10.0), (60.0, 50.0), (100.0, 90.0)]),
            Axes::Vertical,
            Point::new(100.0, 0.0),
            1.0,
        );
        assert_eq!(got.len(), 1);
        assert!(got[0].closed);
        assert_eq!(
            ats(&got[0]),
            [(100.0, 10.0), (60.0, 50.0), (100.0, 90.0), (140.0, 50.0)]
        );
    }

    /// One end on the axis joins the halves there into one open path; an end
    /// near enough is pulled onto it.
    #[test]
    fn one_end_on_the_axis_joins_one_open_path() {
        let got = mirrored(
            run(&[(60.0, 10.0), (99.5, 50.0)]),
            Axes::Vertical,
            Point::new(100.0, 0.0),
            1.0,
        );
        assert_eq!(got.len(), 1);
        assert!(!got[0].closed);
        assert_eq!(ats(&got[0]), [(60.0, 10.0), (100.0, 50.0), (140.0, 10.0)]);
        let got = mirrored(
            run(&[(100.0, 50.0), (60.0, 10.0)]),
            Axes::Vertical,
            Point::new(100.0, 0.0),
            1.0,
        );
        assert_eq!(ats(&got[0]), [(140.0, 10.0), (100.0, 50.0), (60.0, 10.0)]);
    }

    /// Neither end on it leaves two subpaths in the one layer.
    #[test]
    fn neither_end_on_the_axis_leaves_two_subpaths() {
        let got = mirrored(
            run(&[(60.0, 10.0), (80.0, 50.0)]),
            Axes::Vertical,
            Point::new(100.0, 0.0),
            1.0,
        );
        assert_eq!(got.len(), 2);
        assert_eq!(ats(&got[1]), [(140.0, 10.0), (120.0, 50.0)]);
    }

    /// Both axes: a quarter drawn from one axis to the other closes into a whole.
    #[test]
    fn a_quarter_from_axis_to_axis_closes_under_both() {
        let got = mirrored(
            run(&[(100.0, 60.0), (70.0, 70.0), (60.0, 100.0)]),
            Axes::Both,
            Point::new(100.0, 100.0),
            1.0,
        );
        assert_eq!(got.len(), 1);
        assert!(got[0].closed);
        assert_eq!(got[0].anchors.len(), 8);
    }

    /// A curve's handle at the join leaves on the mirror of the one arriving, so
    /// the joined path is as smooth across the axis as it was drawn.
    #[test]
    fn the_join_carries_the_mirrored_handle() {
        let mut s = run(&[(60.0, 10.0), (100.0, 50.0)]);
        s.anchors[1].back = ondin_core::kurbo::Vec2::new(-10.0, -4.0);
        let got = mirrored(s, Axes::Vertical, Point::new(100.0, 0.0), 1.0);
        let join = got[0].anchors[1];
        assert_eq!(join.back, ondin_core::kurbo::Vec2::new(-10.0, -4.0));
        assert_eq!(join.out, ondin_core::kurbo::Vec2::new(10.0, -4.0));
    }

    /// On open canvas the axes default to the view's middle, and a run fixes them
    /// there, so a pan mid-run does not move them under the anchors.
    ///
    /// flip: `pin_mirror_axes` doing nothing → fails at the second assertion.
    #[test]
    fn a_run_on_open_canvas_pins_the_axes_where_the_view_was() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(1);
        app.session
            .adopt_document(ondin_core::Document::new(ids.mint()), None);
        app.session.mirror.axes = Axes::Vertical;
        let was = app.session.camera.center;
        assert_eq!(app.mirror_axes(None).map(|(_, at)| at), Some(was));
        app.pin_mirror_axes(Point::new(-5000.0, 0.0));
        app.session.camera.center = Point::new(was.x + 300.0, was.y);
        assert_eq!(app.mirror_axes(None).map(|(_, at)| at), Some(was));
    }

    /// The pen's finish goes through the mirror: a run from the axis and back to it
    /// lands as one closed path, in one step.
    ///
    /// flip: `finish_pen` creating from the pen's anchors alone → fails at the closed assertion.
    #[test]
    fn the_pen_lands_the_mirrored_run_as_one_path() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(1);
        app.session
            .adopt_document(ondin_core::Document::new(ids.mint()), None);
        app.canvas_px = (800, 600);
        app.choose_tool(crate::tools::Tool::Pen);
        app.session.mirror = Mirror {
            axes: Axes::Vertical,
            at: Some(Point::new(100.0, 0.0)),
        };
        let page = app.session.page;
        app.pen = Some(crate::preview::PenState {
            parent: page,
            parent_world: Affine::IDENTITY,
            anchors: [(100.0, 10.0), (60.0, 50.0), (100.0, 90.0)]
                .map(|(x, y)| PenAnchor::corner(Point::new(x, y)))
                .to_vec(),
            shaping: false,
            editing: None,
        });
        let depth = app.session.history.undo_depth();
        app.finish_pen(false);
        assert_eq!(app.session.history.undo_depth(), depth + 1);
        let id = app
            .session
            .selection
            .single()
            .expect("the path is selected");
        let Some(ondin_core::NodeKind::Path { path, .. }) =
            app.session.doc.get(id).map(|n| n.kind())
        else {
            panic!("a path");
        };
        let subs = crate::tools::pen_anchors(path, &[]);
        assert_eq!(subs.len(), 1, "one subpath");
        assert!(subs[0].closed, "closed on the axis");
        assert_eq!(subs[0].anchors.len(), 4);
    }
}
