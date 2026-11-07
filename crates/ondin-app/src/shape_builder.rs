//! The shape builder: drag across the faces the selection's closed shapes cut each
//! other into, open paths and lines cutting them further, and release merges them
//! into one path; with Alt held it cuts them out instead. A click lifts the one
//! face under it out as its own path.
//!
//! The faces are `ondin_core::shape_builder`'s, found once per selection and
//! document revision and kept in [`OndinApp::builder_faces`]; hovering only looks
//! up the face under the pointer. The tool stays armed, and after an edit the
//! selection is what the edit left, so building carries on.

use crate::app::OndinApp;
use crate::theme::{color, icon};
use crate::tools::Tool;
use eframe::egui;
use ondin_core::NodeId;
use ondin_core::kurbo::{BezPath, Point};
use ondin_core::shape_builder::{Arrangement, Refusal};

/// The refusal the status line gives when there are no faces to build with.
pub const NEEDS: &str =
    "Select overlapping shapes, or a shape and a line across it, then drag across them";

/// Why the tool cannot be armed while the selection has too few closed shapes: the
/// rail button's tooltip, the palette row's reason and `Shift+M`'s status line.
pub const UNAVAILABLE: &str =
    "Shape builder needs overlapping shapes, or a shape and a line across it";

/// What the status line says while the tool is armed over faces it can build.
const HINT: &str = "Drag across regions to merge them · Alt-drag to remove";

/// The furthest apart, in screen pt, two points of the trail are tested for the
/// face under them, so a fast drag does not jump a thin face.
const STEP_PT: f64 = 2.0;

/// The trail's width, and the dot that marks where the press landed, in screen pt.
const TRAIL_PT: f32 = 3.0;
const TRAIL_DOT_PT: f32 = 4.0;

/// The face outline's width in screen pt, over the hovered or gathered faces.
const EDGE_PT: f32 = 2.0;

/// The pitch and width of the hatch over faces about to be removed, in screen pt.
const HATCH_PITCH: f32 = 6.0;
const HATCH_PT: f32 = 2.5;

/// The selection's faces as last found, and what they were found for.
pub(crate) struct Faces {
    revision: u64,
    selection: Vec<NodeId>,
    pub(crate) found: Result<Arrangement, Refusal>,
}

/// A drag in flight: the faces gathered so far, in the order the trail met them.
pub(crate) struct BuilderDrag {
    trail: Vec<Point>,
    picked: Vec<usize>,
    /// The union of the gathered faces, redrawn as one outline.
    outline: Option<BezPath>,
    /// The count and verb the status line last gave, so it is said once per change.
    said: Option<(usize, bool)>,
}

/// The tail every report of the tool's and of *Divide*'s carries when the
/// arrangement left some of the selection out, else nothing.
pub fn left_out_note(arr: &Arrangement) -> &'static str {
    if arr.left_out {
        " — only unlocked shapes, paths and lines take part; the rest were left out"
    } else {
        ""
    }
}

impl OndinApp {
    /// The faces of the current selection, found again only when the selection or
    /// the document changed since they were last found.
    pub(crate) fn builder_faces(&mut self) -> &Result<Arrangement, Refusal> {
        let revision = self.session.revision();
        let selection = self.session.selection.ids();
        let stale = self
            .builder_cache
            .as_ref()
            .is_none_or(|c| c.revision != revision || c.selection != selection);
        if stale {
            let found = ondin_core::shape_builder::arrange(
                &self.session.doc,
                &self.session.resolved,
                selection,
            );
            self.builder_cache = Some(Faces {
                revision,
                selection: selection.to_vec(),
                found,
            });
        }
        &self.builder_cache.as_ref().expect("filled above").found
    }

    /// Whether the rail's Shape builder button takes a click: the selection holds
    /// two closed shapes, or one and a line to cut it, or the tool is already
    /// armed, so the slot still shows which tool is in hand whatever the selection
    /// becomes.
    pub(crate) fn builder_slot_usable(&self) -> bool {
        self.tool == Tool::ShapeBuilder
            || ondin_core::shape_builder::can_arrange(
                &self.session.doc,
                self.session.selection.ids(),
            )
    }

    /// The faces as last found, for drawing, which cannot find them.
    fn cached_faces(&self) -> Option<&Arrangement> {
        self.builder_cache.as_ref()?.found.as_ref().ok()
    }

    /// Said when the tool is armed: the two gestures, or why there is nothing to
    /// build with.
    pub(crate) fn builder_armed(&mut self) {
        let said = match self.builder_faces() {
            Ok(arr) => format!("{HINT}{}", left_out_note(arr)),
            Err(Refusal::Abandoned) => "The shapes could not be cut into regions".into(),
            Err(_) => NEEDS.into(),
        };
        self.session.info(said);
    }

    /// The shape builder's gesture: a press starts a trail, dragging gathers the
    /// faces it crosses, release merges them, or removes them with Alt held.
    pub(crate) fn builder_input(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: egui::Rect,
        ppp: f32,
    ) {
        let (pressed, released, alt, at, moves) = ui.input(|i| {
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
                i.modifiers.alt,
                i.pointer.interact_pos(),
                moves,
            )
        });
        let pressed = pressed && resp.contains_pointer();
        if pressed && let Some(p) = at {
            self.builder_press(self.to_world(p, rect, ppp), alt);
        }
        // On the press's own frame the moves came before it, so they are not trail.
        if self.builder.is_some() && !pressed {
            if let Some(p) = at {
                let world: Vec<Point> = moves
                    .into_iter()
                    .chain([p])
                    .map(|m| self.to_world(m, rect, ppp))
                    .collect();
                self.builder_drag_to(&world, alt);
            }
            ui.ctx().request_repaint();
        } else {
            // Faces found on the frame the tool arms, not on the first press.
            let _ = self.builder_faces();
        }
        if released {
            self.finish_builder(alt);
        }
    }

    /// A press at `world`: the start of a drag over the faces, or, with none to
    /// build with, the reason on the status line. `removes` is Alt held.
    pub(crate) fn builder_press(&mut self, world: Point, removes: bool) {
        if self.builder_faces().is_err() {
            return self.builder_armed();
        }
        self.builder = Some(BuilderDrag {
            trail: Vec::new(),
            picked: Vec::new(),
            outline: None,
            said: None,
        });
        self.builder_drag_to(&[world], removes);
    }

    /// The drag carried through `world`'s points in turn, and the status line told
    /// what a release would do whenever that changes: `removes` is Alt held now.
    pub(crate) fn builder_drag_to(&mut self, world: &[Point], removes: bool) {
        let before = self.builder.as_ref().map(|d| d.picked.len());
        for p in world {
            self.builder_sample(*p);
        }
        let Some(drag) = self.builder.as_ref() else {
            return;
        };
        let n = drag.picked.len();
        if Some(n) != before {
            self.builder_regather();
        }
        let Some(drag) = self.builder.as_mut() else {
            return;
        };
        if n > 0 && drag.said != Some((n, removes)) {
            drag.said = Some((n, removes));
            let verb = if removes { "remove" } else { "merge" };
            self.session
                .info(format!("Release to {verb} {} · Esc to cancel", regions(n)));
        }
    }

    /// Take the pointer at `world` into the trail, gathering every face on the way
    /// from the last sample.
    fn builder_sample(&mut self, world: Point) {
        let ppw = self.points_per_world();
        let Some(arr) = self.cached_faces() else {
            return;
        };
        let Some(drag) = self.builder.as_ref() else {
            return;
        };
        let from = drag.trail.last().copied().unwrap_or(world);
        let steps = ((world - from).hypot() * ppw / STEP_PT).ceil().max(1.0) as usize;
        let met: Vec<usize> = (1..=steps)
            .map(|k| from.lerp(world, k as f64 / steps as f64))
            .chain(drag.trail.is_empty().then_some(world))
            .filter_map(|p| arr.region_at(p))
            .collect();
        let Some(drag) = self.builder.as_mut() else {
            return;
        };
        for face in met {
            if !drag.picked.contains(&face) {
                drag.picked.push(face);
            }
        }
        drag.trail.push(world);
    }

    /// Redraw the gathered faces' outline as one.
    fn builder_regather(&mut self) {
        let Some(arr) = self.cached_faces() else {
            return;
        };
        let Some(drag) = self.builder.as_ref() else {
            return;
        };
        let faces: Vec<BezPath> = drag
            .picked
            .iter()
            .filter_map(|i| arr.regions.get(*i).map(|r| r.path.clone()))
            .collect();
        let outline = ondin_core::evaluate_boolean(ondin_core::BoolOp::Union, &faces);
        if let Some(drag) = self.builder.as_mut() {
            drag.outline = outline;
        }
    }

    /// Commit the drag: the gathered faces merged into one path, or with `remove`
    /// cut out of every shape over them, as one step, *Shape builder*. What the
    /// edit left is selected.
    pub(crate) fn finish_builder(&mut self, remove: bool) {
        let Some(drag) = self.builder.take() else {
            return;
        };
        if drag.picked.is_empty() {
            return;
        }
        let Some(arr) = self.cached_faces().cloned() else {
            return;
        };
        let made = match remove {
            true => arr.remove(
                &self.session.doc,
                &self.session.resolved,
                &mut self.session.ids,
                &drag.picked,
            ),
            false => arr.merge(
                &self.session.doc,
                &self.session.resolved,
                &mut self.session.ids,
                &drag.picked,
            ),
        };
        let (tx, built) = match made {
            Ok(made) => made,
            Err(e) => {
                return self.session.fail(format!("Cannot build: {e}"));
            }
        };
        if !self
            .session
            .commit_aside(tx, "Shape builder", icon::SHAPE_BUILDER)
        {
            return;
        }
        // The faces were the old document's; they are found again next frame.
        self.builder_cache = None;
        let kept: Vec<NodeId> = built.merged.into_iter().chain(built.kept).collect();
        self.session.selection.set(kept);
        let n = regions(drag.picked.len());
        let verb = if remove { "Removed" } else { "Merged" };
        self.session
            .info(format!("{verb} {n}{}", left_out_note(&arr)));
    }

    /// Whether a press of the shape builder would remove rather than merge: Alt is
    /// held. The cursor's badge reads it.
    pub(crate) fn builder_removes(&self, ctx: &egui::Context) -> bool {
        ctx.input(|i| i.modifiers.alt)
    }

    /// The face under the pointer before a press, and during a drag the trail, the
    /// gathered faces as one, and the count beside the pointer — in red and hatched
    /// while Alt would remove them.
    pub(crate) fn draw_builder(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        resp: &egui::Response,
    ) {
        let Some(arr) = self.cached_faces() else {
            return;
        };
        let removes = self.builder_removes(painter.ctx());
        let tol = 0.25 / self.points_per_world();
        let screen = |path: &BezPath| -> Vec<Vec<egui::Pos2>> {
            polylines(path, tol)
                .into_iter()
                .map(|l| {
                    l.into_iter()
                        .map(|p| self.to_screen(p, rect, ppp))
                        .collect()
                })
                .collect()
        };
        let Some(drag) = &self.builder else {
            if let Some(p) = resp.hover_pos()
                && let Some(face) = arr.region_at(self.to_world(p, rect, ppp))
            {
                mark(painter, &screen(&arr.regions[face].path), removes);
            }
            return;
        };
        if let Some(outline) = &drag.outline {
            mark(painter, &screen(outline), removes);
        }
        let ink = if removes {
            color::danger()
        } else {
            color::POINT
        };
        let trail: Vec<egui::Pos2> = drag
            .trail
            .iter()
            .map(|p| self.to_screen(*p, rect, ppp))
            .collect();
        if let Some(first) = trail.first() {
            painter.circle_filled(*first, TRAIL_DOT_PT, ink);
        }
        painter.add(egui::Shape::line(
            trail.clone(),
            egui::Stroke::new(TRAIL_PT, ink),
        ));
        if let Some(last) = trail.last()
            && !drag.picked.is_empty()
        {
            let n = drag.picked.len();
            let (text, hue) = match removes {
                true => (format!("Remove {n}"), color::danger()),
                false => (regions(n), color::BADGE),
            };
            crate::spacing::paint_label(painter, *last, &text, None, hue);
        }
    }
}

/// "1 region" or "N regions".
fn regions(n: usize) -> String {
    match n {
        1 => "1 region".into(),
        n => format!("{n} regions"),
    }
}

/// `path` flattened into closed polylines within `tol`, one per subpath.
fn polylines(path: &BezPath, tol: f64) -> Vec<Vec<Point>> {
    let mut out: Vec<Vec<Point>> = Vec::new();
    ondin_core::kurbo::flatten(path, tol, |el| match el {
        ondin_core::kurbo::PathEl::MoveTo(p) => out.push(vec![p]),
        ondin_core::kurbo::PathEl::LineTo(p) => {
            if let Some(l) = out.last_mut() {
                l.push(p);
            }
        }
        _ => {}
    });
    out.retain(|l| l.len() > 2);
    out
}

/// A face marked on the canvas: a light wash with a white edge, or, when it is
/// about to be removed, a red hatch with a red edge.
fn mark(painter: &egui::Painter, loops: &[Vec<egui::Pos2>], removes: bool) {
    let (wash, edge) = match removes {
        true => (color::danger().gamma_multiply(0.18), color::danger()),
        false => (egui::Color32::from_white_alpha(77), color::POINT),
    };
    let mut mesh = egui::Mesh::default();
    let (top, bottom) = extent(loops, |p| p.y);
    let mut y = top.floor();
    while y < bottom {
        for (x0, x1) in spans(loops, y + 0.5, |p| (p.x, p.y)) {
            mesh.add_colored_rect(
                egui::Rect::from_min_max(egui::pos2(x0, y), egui::pos2(x1, y + 1.0)),
                wash,
            );
        }
        y += 1.0;
    }
    painter.add(mesh);
    if removes {
        // The hatch runs at 45°: the same scan, in a frame turned an eighth.
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let turned: Vec<Vec<egui::Pos2>> = loops
            .iter()
            .map(|l| {
                l.iter()
                    .map(|p| egui::pos2((p.x + p.y) * s, (p.y - p.x) * s))
                    .collect()
            })
            .collect();
        let back = |u: f32, v: f32| egui::pos2((u - v) * s, (u + v) * s);
        let (top, bottom) = extent(&turned, |p| p.y);
        let mut v = (top / HATCH_PITCH).floor() * HATCH_PITCH;
        while v < bottom {
            for (u0, u1) in spans(&turned, v, |p| (p.x, p.y)) {
                painter.line_segment(
                    [back(u0, v), back(u1, v)],
                    egui::Stroke::new(HATCH_PT, edge.gamma_multiply(0.7)),
                );
            }
            v += HATCH_PITCH;
        }
    }
    for l in loops {
        painter.add(egui::Shape::closed_line(
            l.clone(),
            egui::Stroke::new(EDGE_PT, edge),
        ));
    }
}

/// The least and greatest of `axis` over every point of `loops`.
fn extent(loops: &[Vec<egui::Pos2>], axis: impl Fn(&egui::Pos2) -> f32) -> (f32, f32) {
    loops
        .iter()
        .flatten()
        .map(axis)
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(v), hi.max(v))
        })
}

/// The runs along the horizontal line at `y` that `loops` cover under the non-zero
/// rule, as `(x0, x1)` pairs. `xy` reads a point's coordinates.
fn spans(
    loops: &[Vec<egui::Pos2>],
    y: f32,
    xy: impl Fn(&egui::Pos2) -> (f32, f32),
) -> Vec<(f32, f32)> {
    let mut hits: Vec<(f32, i32)> = Vec::new();
    for l in loops {
        for (i, a) in l.iter().enumerate() {
            let ((ax, ay), (bx, by)) = (xy(a), xy(&l[(i + 1) % l.len()]));
            if (ay <= y) != (by <= y) {
                let t = (y - ay) / (by - ay);
                hits.push((ax + t * (bx - ax), if by > ay { 1 } else { -1 }));
            }
        }
    }
    hits.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = Vec::new();
    let mut winding = 0;
    for (x, dir) in &hits {
        let was = winding;
        winding += dir;
        if was == 0 && winding != 0 {
            out.push((*x, *x));
        } else if was != 0
            && winding == 0
            && let Some(run) = out.last_mut()
        {
            run.1 = *x;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Tool;
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{Document, NodeKind, Operation, Transaction};

    /// A headless app with the design's three circles, r 78, on a frame at the
    /// origin, all three selected and the shape builder armed.
    fn app_with_circles(ctx: &egui::Context, n: usize) -> (OndinApp, NodeId, Vec<NodeId>) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let frame = ids.mint();
        let mut ops = vec![Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(600.0, 400.0),
                corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        }];
        let mut circles = Vec::new();
        for (i, (x, y)) in [(211.0, 140.0), (311.0, 140.0), (261.0, 222.0)]
            .into_iter()
            .take(n)
            .enumerate()
        {
            let id = ids.mint();
            ops.push(Operation::CreateNode {
                id,
                parent: frame,
                index: i,
                kind: NodeKind::Ellipse {
                    size: Size::new(156.0, 156.0),
                    arc: Default::default(),
                },
                transform: Some(Affine::translate((x - 78.0, y - 78.0))),
                name: None,
            });
            circles.push(id);
        }
        doc.apply(&Transaction(ops)).expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.canvas_px = (800, 600);
        app.session.selection.set(circles.clone());
        app.choose_tool(Tool::ShapeBuilder);
        (app, frame, circles)
    }

    fn drag(app: &mut OndinApp, trail: &[Point], remove: bool) {
        app.builder_press(trail[0], remove);
        app.builder_drag_to(&trail[1..], remove);
        app.finish_builder(remove);
    }

    fn last_step(app: &OndinApp) -> String {
        let step = app.session.history.done().last().expect("a step");
        step.note.label.to_string()
    }

    /// A drag across three faces merges them in one *Shape builder* step; the tool
    /// stays armed and the new path and what is left are selected.
    /// flip: `finish_builder` selecting only `built.merged` → fails at "and the rest".
    #[test]
    fn a_drag_merges_the_faces_it_crosses_and_the_tool_stays_armed() {
        let ctx = egui::Context::default();
        let (mut app, frame, circles) = app_with_circles(&ctx, 3);
        let before = app.session.history.done().len();
        let trail = [
            Point::new(160.0, 128.0),
            Point::new(240.0, 98.0),
            Point::new(362.0, 126.0),
        ];
        drag(&mut app, &trail, false);
        assert_eq!(app.session.history.done().len(), before + 1, "one step");
        assert_eq!(last_step(&app), "Shape builder");
        assert_eq!(app.tool, Tool::ShapeBuilder);
        let kids = app.session.doc.get(frame).unwrap().children().to_vec();
        assert_eq!(kids.len(), 4, "the merged path and the three it took from");
        assert_eq!(app.session.selection.ids().len(), 4, "and the rest");
        assert!(app.session.selection.contains(circles[2]));
        assert_eq!(app.session.status_mut().text, "Merged 3 regions");
    }

    /// A click without a drag lifts the one face under it out as its own path, and
    /// an Alt-click removes it, adding no layer.
    #[test]
    fn a_click_lifts_one_face_and_an_alt_click_removes_it() {
        let ctx = egui::Context::default();
        let (mut app, frame, _) = app_with_circles(&ctx, 2);
        drag(&mut app, &[Point::new(261.0, 140.0)], false);
        assert_eq!(app.session.doc.get(frame).unwrap().children().len(), 3);
        assert_eq!(app.session.status_mut().text, "Merged 1 region");

        let (mut app, frame, _) = app_with_circles(&ctx, 2);
        drag(&mut app, &[Point::new(261.0, 140.0)], true);
        assert_eq!(app.session.doc.get(frame).unwrap().children().len(), 2);
        assert_eq!(app.session.status_mut().text, "Removed 1 region");
        assert_eq!(last_step(&app), "Shape builder");
    }

    /// Escape mid-drag drops it and records nothing; a second Escape hands back to
    /// Select.
    /// flip: `cancel_gesture` leaving `builder` alone → fails at "nothing recorded".
    #[test]
    fn escape_drops_the_drag_then_leaves_the_tool() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = app_with_circles(&ctx, 3);
        let before = app.session.history.done().len();
        app.builder_press(Point::new(160.0, 128.0), false);
        app.builder_drag_to(&[Point::new(362.0, 126.0)], false);
        assert_eq!(
            app.session.status_mut().text,
            "Release to merge 3 regions · Esc to cancel"
        );
        // Alt pressed mid-drag flips what a release would do.
        app.builder_drag_to(&[], true);
        assert_eq!(
            app.session.status_mut().text,
            "Release to remove 3 regions · Esc to cancel"
        );
        app.escape(&ctx);
        app.finish_builder(false);
        assert_eq!(app.session.history.done().len(), before, "nothing recorded");
        assert_eq!(app.tool, Tool::ShapeBuilder);
        app.escape(&ctx);
        assert_eq!(app.tool, Tool::Select);
    }

    /// With one shape selected a press starts nothing and the status line says what
    /// the tool needs.
    #[test]
    fn a_press_with_too_few_shapes_says_what_is_needed() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = app_with_circles(&ctx, 1);
        app.builder_press(Point::new(211.0, 140.0), false);
        assert!(app.builder.is_none());
        assert_eq!(app.session.status_mut().text, NEEDS);
    }

    /// The rail button is usable with two shapes selected, not with one, and stays
    /// usable once armed whatever the selection.
    ///
    /// flip: `builder_slot_usable` dropping its armed arm → fails at "armed".
    #[test]
    fn the_rail_button_needs_two_shapes_unless_armed() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = app_with_circles(&ctx, 1);
        assert!(
            app.builder_slot_usable(),
            "armed with one shape, the slot stays usable"
        );
        app.choose_tool(Tool::Select);
        assert!(!app.builder_slot_usable(), "one shape is too few");

        let (mut app, _, _) = app_with_circles(&ctx, 2);
        app.choose_tool(Tool::Select);
        assert!(
            app.builder_slot_usable(),
            "two overlapping shapes can be built from"
        );
    }

    /// `Shift+M`'s action refuses with one shape selected, saying why, and arms with two.
    ///
    /// flip: drop the refusing `ChooseTool(Tool::ShapeBuilder)` arm in `dispatch` →
    /// fails at "one shape arms nothing".
    #[test]
    fn the_shortcut_refuses_as_the_rail_button_does() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = app_with_circles(&ctx, 1);
        app.choose_tool(Tool::Select);
        app.dispatch(&ctx, crate::input::Action::ChooseTool(Tool::ShapeBuilder));
        assert_eq!(app.tool, Tool::Select, "one shape arms nothing");
        assert_eq!(app.session.status_mut().text, UNAVAILABLE);

        let (mut app, _, _) = app_with_circles(&ctx, 2);
        app.choose_tool(Tool::Select);
        app.dispatch(&ctx, crate::input::Action::ChooseTool(Tool::ShapeBuilder));
        assert_eq!(app.tool, Tool::ShapeBuilder, "two shapes arm it");
    }

    /// *Divide* leaves one path per face in a selected *Divided* group and says how
    /// many.
    #[test]
    fn divide_selects_its_group_and_counts_the_paths() {
        let ctx = egui::Context::default();
        let (mut app, frame, _) = app_with_circles(&ctx, 3);
        app.divide_selection();
        let group = app
            .session
            .selection
            .single()
            .expect("the group is selected");
        assert_eq!(app.session.doc.get(frame).unwrap().children(), &[group]);
        assert_eq!(app.session.doc.get(group).unwrap().children().len(), 7);
        assert_eq!(app.session.status_mut().text, "Divided into 7 paths");
    }

    /// A square with a square hole wound against it: the scan covers the two walls
    /// and skips the hole.
    #[test]
    fn a_scan_skips_a_hole_wound_against_its_outline() {
        let square = |a: f32, b: f32| {
            vec![
                egui::pos2(a, a),
                egui::pos2(b, a),
                egui::pos2(b, b),
                egui::pos2(a, b),
            ]
        };
        let outer = square(0.0, 10.0);
        let mut hole = square(3.0, 7.0);
        hole.reverse();
        let got = spans(&[outer.clone(), hole], 5.0, |p| (p.x, p.y));
        assert_eq!(got, vec![(0.0, 3.0), (7.0, 10.0)]);
        let got = spans(&[outer], 5.0, |p| (p.x, p.y));
        assert_eq!(got, vec![(0.0, 10.0)]);
    }
}
