//! The pencil: a path drawn freehand.
//!
//! While the button is down the pencil leaves a trail of samples, drawn as a thin
//! line in the selection blue rather than as the final stroke, so nothing looks
//! fitted before it is. On release the trail is fitted as *Simplify* fits
//! (`build::freehand_outline`) at [`FIT_PT`] screen pt, so zoomed in you draw finer.
//! What lands is an ordinary stroked `Path` the node tool edits like a pen path.
//!
//! The pencil makes geometry, not ink: no pressure, brush, variable width or
//! smoothing setting. *Simplify path* already smooths after the fact.

use crate::app::OndinApp;
use crate::canvas::{PEN_STROKE_COLOR, PEN_STROKE_WIDTH};
use crate::preview::{PenAnchor, PenEdit, PenSubpath};
use crate::theme::{color, icon};
use crate::tools::{self, Tool};
use eframe::egui;
use ondin_core::kurbo::{Affine, Cap, Join, Point, Vec2};
use ondin_core::{NodeId, Stroke, build};

/// How far the fitted curve may stray from the trail, in screen pt.
const FIT_PT: f64 = 1.5;

/// How near its start, or the extended path's other end, a stroke must end to close
/// the path, in screen pt.
const CLOSE_PT: f64 = 8.0;

/// A drag that never gets further than this from its press, in screen pt, is a
/// click: it draws a dot.
const MIN_DRAG_PT: f64 = 3.0;

/// The least the pointer moves, in screen pt, before the trail takes another sample.
const STEP_PT: f64 = 1.0;

/// A pencil stroke in flight, in world space.
pub(crate) struct PencilStroke {
    /// Where a new path lands: the frame under the press, or the page.
    parent: NodeId,
    parent_world: Affine,
    /// The trail, one sample per pointer move of at least [`STEP_PT`].
    samples: Vec<Point>,
    /// Samples the fit keeps as corner anchors: both ends of every Shift run.
    corners: Vec<usize>,
    /// The sample a held Shift runs straight from; the run is the one sample after it.
    straight_from: Option<usize>,
    /// The selected open path this stroke carries on from, grabbed at an end.
    extend: Option<PenEdit>,
}

impl OndinApp {
    /// The pencil's gesture: press starts a trail, dragging extends it, release fits
    /// it. Escape mid-drag drops it (`OndinApp::escape`).
    pub(crate) fn pencil_input(
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
        if pressed
            && resp.contains_pointer()
            && let Some(p) = at
        {
            self.pencil_press(self.to_world(p, rect, ppp));
        }
        if self.pencil.is_some()
            && let Some(p) = at
        {
            // Every move the OS reported this frame, not only the last: a fast
            // stroke keeps its curve whatever the frame rate.
            for m in moves.into_iter().chain([p]) {
                self.pencil_sample(self.to_world(m, rect, ppp), shift);
            }
            ui.ctx().request_repaint();
        }
        if released {
            self.finish_pencil();
        }
    }

    fn pencil_press(&mut self, world: Point) {
        let extend = self.pencil_extend_target(world);
        let start = extend.as_ref().and_then(PenEdit::endpoint).unwrap_or(world);
        let (parent, parent_world) = self.draw_target(start);
        self.pencil = Some(PencilStroke {
            parent,
            parent_world,
            samples: vec![start],
            corners: Vec::new(),
            straight_from: None,
            extend,
        });
    }

    /// The end of the selected open path a press at `world` would carry on, read
    /// into the edit that extends it. Asked by the press and the hover ring.
    pub(crate) fn pencil_extend_target(&self, world: Point) -> Option<PenEdit> {
        let id = self.session.selection.single()?;
        if ondin_core::is_effectively_locked(&self.session.doc, id) {
            return None;
        }
        self.pen_resume_target_in(world, Some(id))
    }

    /// Take the pointer at `world` into the trail. With Shift held the trail runs
    /// straight from where Shift went down, in eighths of a turn.
    fn pencil_sample(&mut self, world: Point, shift: bool) {
        let ppw = self.points_per_world();
        let Some(s) = self.pencil.as_ref() else {
            return;
        };
        let last = s.samples.len() - 1;
        let from = match (shift, s.straight_from) {
            (true, from) => Some(from.unwrap_or(last)),
            (false, _) => None,
        };
        let aim = from.map(|f| self.axial_corner(s.samples[f], world));
        let Some(s) = self.pencil.as_mut() else {
            return;
        };
        match (s.straight_from, from) {
            // Shift went down: the run starts at a corner.
            (None, Some(f)) => s.corners.push(f),
            // Shift came up: the run ends at one.
            (Some(_), None) => s.corners.push(last),
            _ => {}
        }
        s.straight_from = from;
        match (from, aim) {
            (Some(f), Some(aim)) => {
                s.samples.truncate(f + 1);
                if aim != s.samples[f] {
                    s.samples.push(aim);
                }
            }
            _ => {
                if (world - s.samples[last]).hypot() * ppw >= STEP_PT {
                    s.samples.push(world);
                }
            }
        }
    }

    /// Fit the trail and commit it as one step, *Draw path*: a new path, or the
    /// grabbed one extended. The tool stays on and the path is selected.
    pub(crate) fn finish_pencil(&mut self) {
        let Some(mut stroke) = self.pencil.take() else {
            return;
        };
        let ppw = self.points_per_world();
        let start = stroke.samples[0];
        let travel = stroke
            .samples
            .iter()
            .map(|p| (*p - start).hypot() * ppw)
            .fold(0.0, f64::max);
        if travel < MIN_DRAG_PT {
            // A click on a path's end only picks it up; anywhere else it leaves a dot,
            // a run short against the stroke's width that the round caps draw round
            // at any zoom.
            if stroke.extend.is_some() {
                return;
            }
            let run = (self.pencil_style().width * 0.01).max(1e-3);
            stroke.samples = vec![start, start + Vec2::new(run, 0.0)];
            stroke.corners.clear();
        }
        let accuracy = FIT_PT / ppw;
        let (id, tx) = match stroke.extend.take() {
            None => {
                let closes = closing(&mut stroke.samples, start, ppw, false);
                let fitted =
                    build::freehand_outline(&stroke.samples, closes, &stroke.corners, accuracy);
                let Some(sub) = tools::pen_anchors(&fitted, &[]).into_iter().next() else {
                    return;
                };
                let id = self.session.ids.mint();
                let style = self.pencil_style();
                let tx = tools::create_path(
                    id,
                    stroke.parent,
                    self.child_count(stroke.parent),
                    stroke.parent_world,
                    &sub.anchors,
                    sub.closed,
                    style.clone(),
                );
                self.pencil_last = Some((id, style));
                (id, tx)
            }
            Some(mut edit) => {
                let working = edit.working();
                let closes =
                    working.len() >= 2 && closing(&mut stroke.samples, working[0].at, ppw, true);
                let fitted =
                    build::freehand_outline(&stroke.samples, false, &stroke.corners, accuracy);
                let Some(sub) = tools::pen_anchors(&fitted, &[]).into_iter().next() else {
                    return;
                };
                let mut run = extended(working, &sub.anchors, closes);
                // Back into the direction it was read in (`PenEdit::subpaths`).
                if edit.from_start {
                    run.reverse();
                }
                edit.subpaths[edit.active] = run;
                let current = self.stored_path(edit.id);
                let tx = tools::rewrite_path(edit.id, edit.world, &edit.subpaths, current);
                (edit.id, tx)
            }
        };
        if !tx.0.is_empty()
            && self
                .session
                .commit_aside(tx, "Draw path", icon::PENCIL_SIMPLE)
        {
            self.session.selection.set_one(id);
        }
    }

    /// The stroke a new pencil path draws with: the last pencil path's colour and
    /// width as it stands now, so a width changed on it carries to the next; the
    /// pen's before any. Round caps and joins, as a hand-drawn line has.
    fn pencil_style(&self) -> Stroke {
        let base = match &self.pencil_last {
            Some((id, kept)) => self
                .session
                .doc
                .get(*id)
                .and_then(|n| n.paint().strokes.first())
                .map_or_else(|| kept.clone(), |k| k.value.clone()),
            None => tools::solid_stroke(PEN_STROKE_COLOR, PEN_STROKE_WIDTH),
        };
        Stroke {
            brush: base.brush,
            width: base.width,
            width_var: base.width_var,
            cap: Cap::Round,
            join: Join::Round,
            ..tools::solid_stroke(PEN_STROKE_COLOR, PEN_STROKE_WIDTH)
        }
    }

    /// The trail as it is being drawn, the close ring when letting go would close,
    /// and, before a press, the ring on an end a stroke would carry on.
    pub(crate) fn draw_pencil(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        resp: &egui::Response,
    ) {
        let ring = |at: Point| {
            let at = self.to_screen(at, rect, ppp);
            crate::canvas::paint_anchor(painter, at, color::POINT);
            painter.circle_stroke(
                at,
                crate::canvas::SNAP_RING_PX,
                egui::Stroke::new(crate::canvas::SNAP_RING_STROKE_PX, color::SELECT),
            );
        };
        let Some(stroke) = &self.pencil else {
            if self.tool == Tool::Pencil
                && let Some(p) = resp.hover_pos()
                && let Some(end) = self
                    .pencil_extend_target(self.to_world(p, rect, ppp))
                    .and_then(|e| e.endpoint())
            {
                ring(end);
            }
            return;
        };
        let pts: Vec<egui::Pos2> = stroke
            .samples
            .iter()
            .map(|p| self.to_screen(*p, rect, ppp))
            .collect();
        if let Some(last) = pts.last() {
            painter.circle_filled(*last, 2.0, color::SELECT);
        }
        painter.add(egui::Shape::line(
            pts,
            egui::Stroke::new(1.0, color::SELECT),
        ));
        let ppw = self.points_per_world();
        let target = match &stroke.extend {
            Some(edit) => edit.working().first().map(|a| a.at),
            None => stroke.samples.first().copied(),
        };
        if let Some(target) = target
            && let Some(end) = stroke.samples.last()
            && reaches(&stroke.samples, *end, target, ppw)
        {
            ring(target);
        }
    }
}

/// Whether a trail ending at `end` closes on `target`: within [`CLOSE_PT`] of it,
/// having been well away from it first, so a short scribble stays open.
fn reaches(samples: &[Point], end: Point, target: Point, ppw: f64) -> bool {
    let away = samples
        .iter()
        .map(|p| (*p - target).hypot() * ppw)
        .fold(0.0, f64::max);
    samples.len() >= 3 && (end - target).hypot() * ppw <= CLOSE_PT && away > 2.0 * CLOSE_PT
}

/// Whether the trail `samples` closes on `target`, and if so the trail trimmed of
/// the samples that came back within [`CLOSE_PT`] of it. With `land` the trail
/// then ends exactly on `target`, an anchor it joins; without, the fit closes the
/// gap itself and leaves no seam.
fn closing(samples: &mut Vec<Point>, target: Point, ppw: f64, land: bool) -> bool {
    let Some(end) = samples.last().copied() else {
        return false;
    };
    if !reaches(samples, end, target, ppw) {
        return false;
    }
    while samples.len() > 3
        && samples
            .last()
            .is_some_and(|p| (*p - target).hypot() * ppw <= CLOSE_PT)
    {
        samples.pop();
    }
    if land {
        samples.push(target);
    }
    true
}

/// The held run `working`, its grabbed end last, carried on by the fitted
/// `stroke`, whose first anchor sits on that end. `closes` ends it on the run's
/// own first anchor, which `stroke` was landed on.
fn extended(mut working: Vec<PenAnchor>, stroke: &[PenAnchor], closes: bool) -> PenSubpath {
    if let Some((first, rest)) = stroke.split_first() {
        if let Some(end) = working.last_mut() {
            end.out = first.out;
        }
        working.extend_from_slice(rest);
    }
    let closed = closes && working.len() > 2;
    if closed && let Some(last) = working.pop() {
        working[0].back = last.back;
    }
    PenSubpath {
        anchors: working,
        closed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::{BezPath, PathEl, Size, Vec2};
    use ondin_core::{Document, NodeKind, Operation, Transaction};

    /// A headless app on a 400×300 frame at the origin, the pencil armed, at 100%.
    fn app_with_a_frame(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let frame = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
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
        }]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.canvas_px = (800, 600);
        app.choose_tool(Tool::Pencil);
        (app, frame)
    }

    /// An import read while a stroke is in flight waits for the stroke, which
    /// would otherwise lose its trail to the landing's tool change.
    /// flip: `gesture_live` without its pencil term → fails at "the import waits".
    #[test]
    fn an_import_waits_for_a_stroke_in_flight() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_a_frame(&ctx);
        app.pencil_press(Point::new(50.0, 50.0));
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10"/></svg>"#;
        app.start_paste_svg(&ctx, svg.to_string(), None);
        app.settle_imports();
        assert!(app.pencil.is_some(), "the stroke kept");
        assert_eq!(app.tool, Tool::Pencil);
        assert!(!app.imports.is_empty(), "the import waits");
        app.pencil = None;
        app.poll_imports();
        assert!(app.imports.is_empty(), "and lands after");
    }

    /// Press at the first point, drag through the rest, release.
    fn draw(app: &mut OndinApp, trail: &[Point], shift: impl Fn(usize) -> bool) {
        app.pencil_press(trail[0]);
        for (i, p) in trail.iter().enumerate().skip(1) {
            app.pencil_sample(*p, shift(i));
        }
        app.finish_pencil();
    }

    /// A wave across the frame, sampled every 2 units.
    fn wave() -> Vec<Point> {
        (0..100)
            .map(|i| {
                Point::new(
                    40.0 + i as f64 * 2.0,
                    150.0 + (i as f64 * 0.08).sin() * 40.0,
                )
            })
            .collect()
    }

    fn path_of(app: &OndinApp, id: NodeId) -> BezPath {
        match app.session.doc.get(id).map(|n| n.kind()) {
            Some(NodeKind::Path { path, .. }) => path.clone(),
            other => panic!("not a path: {other:?}"),
        }
    }

    /// A stroke lands in the frame under the press as a stroked, unfilled path of a
    /// few anchors, selected, one *Draw path* step, and the tool stays the pencil.
    #[test]
    fn a_stroke_lands_as_a_fitted_path_and_the_pencil_stays_on() {
        let ctx = egui::Context::default();
        let (mut app, frame) = app_with_a_frame(&ctx);
        draw(&mut app, &wave(), |_| false);

        let id = app
            .session
            .selection
            .single()
            .expect("the new path is selected");
        let node = app.session.doc.get(id).expect("created");
        assert_eq!(
            node.parent(),
            Some(frame),
            "it lands in the frame under the press"
        );
        let segs = path_of(&app, id).segments().count();
        assert!(
            (1..=8).contains(&segs),
            "a handful of curves from 100 samples: {segs}"
        );
        assert!(node.paint().fills.is_empty(), "no fill");
        let stroke = &node.paint().strokes[0].value;
        assert_eq!((stroke.cap, stroke.join), (Cap::Round, Join::Round));
        assert_eq!(stroke.width, PEN_STROKE_WIDTH, "the pen's default first");
        assert_eq!(app.tool, Tool::Pencil, "freehand comes in runs");
        let step = app.session.history.done().last().expect("one step");
        assert_eq!(step.note.label, "Draw path");
    }

    /// The next stroke takes the width of the last pencil path as it stands now.
    /// flip: `pencil_style` ignoring `pencil_last` → fails at "the width set on the last".
    #[test]
    fn the_next_stroke_draws_at_the_width_set_on_the_last() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_a_frame(&ctx);
        draw(&mut app, &wave(), |_| false);
        let first = app.session.selection.single().expect("drawn");
        let mut strokes = app.session.doc.get(first).unwrap().paint().strokes.clone();
        strokes[0].value.width = 7.0;
        assert!(app.session.commit(Transaction(vec![Operation::SetStrokes {
            id: first,
            strokes,
        }])));

        let lower: Vec<Point> = wave().iter().map(|p| *p + Vec2::new(0.0, 60.0)).collect();
        draw(&mut app, &lower, |_| false);
        let second = app.session.selection.single().expect("drawn");
        assert_ne!(first, second);
        let width = app.session.doc.get(second).unwrap().paint().strokes[0]
            .value
            .width;
        assert_eq!(width, 7.0, "the width set on the last");
    }

    /// A drag that never leaves a 3 pt radius draws a dot: one *Draw path* step, a
    /// path one pt long from the press, round-capped, selected.
    /// flip: `finish_pencil` returning on a short drag → fails at "one step".
    #[test]
    fn a_click_draws_a_dot() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_a_frame(&ctx);
        let before = app.session.history.done().len();
        draw(
            &mut app,
            &[
                Point::new(50.0, 50.0),
                Point::new(51.5, 51.0),
                Point::new(52.0, 50.0),
            ],
            |_| false,
        );
        assert_eq!(app.session.history.done().len(), before + 1, "one step");
        let id = app.session.selection.single().expect("the dot is selected");
        let b = app.session.resolved.world_bounds(id).expect("drawn");
        let center = b.center();
        assert!(
            (center.x - 50.0).abs() < 0.05 && (center.y - 50.0).abs() < 0.01,
            "centred on the press: {b:?}"
        );
        let stroke = &app.session.doc.get(id).unwrap().paint().strokes[0].value;
        assert_eq!(stroke.cap, Cap::Round);
        assert_eq!(
            path_of(&app, id).elements().len(),
            2,
            "one run: {:?}",
            path_of(&app, id)
        );
    }

    /// Zoomed out, a click still leaves a round dot, not a dash.
    /// flip: the dot's run one screen pt long (`1.0 / ppw`) → fails at "round".
    #[test]
    fn a_click_zoomed_out_is_still_round() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_a_frame(&ctx);
        app.session.camera.zoom = 0.25;
        draw(&mut app, &[Point::new(50.0, 50.0)], |_| false);
        let id = app.session.selection.single().expect("the dot");
        let b = app.session.resolved.world_bounds(id).expect("drawn");
        assert!((b.width() / b.height() - 1.0).abs() < 0.1, "round: {b:?}");
    }

    const RECT: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::Pos2::new(800.0, 600.0),
    };

    /// One frame of the canvas alone, with these events.
    fn canvas_frame(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) {
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
    }

    fn space(pressed: bool) -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Space,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn primary(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// A stroke let go while Space is held still commits, and every move a frame
    /// reports is a sample.
    /// flip: `normal_mode_input`'s pencil gate without its `pencil.is_some()` term →
    /// fails at "committed under Space"; `pencil_input` sampling only the last
    /// position → fails at "a sample per move".
    #[test]
    fn a_stroke_released_under_space_commits() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_a_frame(&ctx);
        let at = |app: &OndinApp, x: f64| app.to_screen(Point::new(x, 100.0), RECT, 1.0);
        let start = at(&app, 50.0);
        canvas_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(start)]);
        canvas_frame(&ctx, &mut app, vec![primary(start, true)]);
        assert!(app.pencil.is_some(), "fixture: a stroke began");
        let moves = (1..=4)
            .map(|i| egui::Event::PointerMoved(at(&app, 50.0 + 10.0 * f64::from(i))))
            .collect();
        canvas_frame(&ctx, &mut app, moves);
        let samples = app.pencil.as_ref().map_or(0, |s| s.samples.len());
        assert!(samples >= 5, "a sample per move: {samples}");

        let before = app.session.history.done().len();
        canvas_frame(&ctx, &mut app, vec![space(true)]);
        let end = at(&app, 90.0);
        canvas_frame(&ctx, &mut app, vec![primary(end, false)]);
        canvas_frame(&ctx, &mut app, vec![space(false)]);
        assert!(app.pencil.is_none(), "committed under Space");
        assert_eq!(app.session.history.done().len(), before + 1);
    }

    /// A click on the selected open path's end draws no dot there.
    #[test]
    fn a_click_on_an_end_draws_nothing() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_a_frame(&ctx);
        draw(&mut app, &wave(), |_| false);
        let id = app.session.selection.single().expect("drawn");
        let before = app.session.history.done().len();
        let end = wave()[99];
        draw(&mut app, &[end, end + Vec2::new(1.0, 0.0)], |_| false);
        assert_eq!(app.session.history.done().len(), before);
        assert_eq!(app.session.selection.single(), Some(id));
    }

    /// A loop let go near its start closes, with no fill.
    #[test]
    fn a_loop_ending_near_its_start_closes() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_a_frame(&ctx);
        // The last sample is 7 units short of the first.
        let ring: Vec<Point> = (0..=61)
            .map(|i| {
                let a = i as f64 / 62.0 * std::f64::consts::TAU;
                Point::new(200.0 + 70.0 * a.cos(), 150.0 + 70.0 * a.sin())
            })
            .collect();
        draw(&mut app, &ring, |_| false);
        let id = app.session.selection.single().expect("drawn");
        assert!(
            matches!(path_of(&app, id).elements().last(), Some(PathEl::ClosePath)),
            "closed"
        );
        assert!(app.session.doc.get(id).unwrap().paint().fills.is_empty());
    }

    /// Shift held mid-stroke runs straight from where it went down; the run's ends
    /// are corners, so the fit leaves it a line.
    #[test]
    fn shift_draws_a_straight_run_between_two_corners() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_a_frame(&ctx);
        let mut trail: Vec<Point> = (0..20)
            .map(|i| {
                Point::new(
                    40.0 + i as f64 * 3.0,
                    200.0 - (i as f64 * 0.08).sin() * 50.0,
                )
            })
            .collect();
        let down = trail.len() - 1;
        // Wobbling off level while Shift is held: the run stays at 0°.
        let from = trail[down];
        trail.extend((1..30).map(|i| from + Vec2::new(i as f64 * 4.0, (i % 3) as f64)));
        let up = trail.len();
        let end = trail[up - 1];
        trail.extend((1..20).map(|i| end + Vec2::new(i as f64 * 3.0, i as f64 * 2.0)));
        // Shift goes down with the pointer at `trail[down]`, on the move after it.
        draw(&mut app, &trail, |i| (down + 1..up).contains(&i));

        let id = app.session.selection.single().expect("drawn");
        let world = app.session.resolved.world_transform(id).unwrap();
        let line = path_of(&app, id)
            .segments()
            .map(|s| world * s)
            .find_map(|s| match s {
                ondin_core::kurbo::PathSeg::Line(l) if (l.p0 - from).hypot() < 1e-6 => Some(l),
                _ => None,
            })
            .expect("a line from where Shift went down");
        assert!((line.p1.y - from.y).abs() < 1e-6, "level: {line:?}");
        assert!((line.p1.x - (from.x + 116.0)).abs() < 1e-6, "{line:?}");
    }

    /// A stroke started on the selected open path's end carries that path on: one
    /// path, one step; ending on the other end closes it.
    /// flip: `pencil_extend_target` → `None` → fails at "the same path".
    #[test]
    fn a_stroke_from_an_open_end_extends_the_path_and_can_close_it() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_a_frame(&ctx);
        let mut drawn = BezPath::new();
        drawn.move_to((50.0, 100.0));
        drawn.line_to((150.0, 100.0));
        let page = app.session.page;
        let id = app.session.ids.mint();
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id,
            parent: page,
            index: 1,
            kind: NodeKind::Path {
                path: drawn,
                corner_radii: Vec::new(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        }])));
        app.session.selection.set_one(id);
        let before = app.session.history.done().len();

        // Down from the right end, round and back up to the left end.
        let back: Vec<Point> = (0..=40)
            .map(|i| {
                let a = i as f64 / 40.0 * std::f64::consts::PI;
                Point::new(100.0 + 50.0 * a.cos(), 100.0 + 50.0 * a.sin())
            })
            .collect();
        draw(&mut app, &back, |_| false);
        assert_eq!(app.session.selection.single(), Some(id), "the same path");
        assert_eq!(app.session.history.done().len(), before + 1, "one step");
        let path = path_of(&app, id);
        assert_eq!(
            path.elements()
                .iter()
                .filter(|e| matches!(e, PathEl::MoveTo(_)))
                .count(),
            1,
            "one run"
        );
        assert!(
            matches!(path.elements().last(), Some(PathEl::ClosePath)),
            "closed"
        );
    }

    /// Escape mid-drag drops the stroke and records nothing; Escape with nothing
    /// being drawn leaves the tool.
    #[test]
    fn escape_drops_the_stroke_then_leaves_the_tool() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_a_frame(&ctx);
        let before = app.session.history.done().len();
        let trail = wave();
        app.pencil_press(trail[0]);
        for p in &trail[1..] {
            app.pencil_sample(*p, false);
        }
        app.escape(&ctx);
        app.finish_pencil();
        assert_eq!(app.session.history.done().len(), before, "nothing recorded");
        assert_eq!(
            app.tool,
            Tool::Pencil,
            "the first Escape only drops the stroke"
        );
        app.escape(&ctx);
        assert_eq!(app.tool, Tool::Select);
    }

    /// A right-click mid-drag drops the stroke, so the primary release commits nothing.
    /// flip: `cancel_gesture` leaving `pencil` alone → fails at "a gesture was in flight".
    #[test]
    fn a_right_click_drops_the_stroke() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_a_frame(&ctx);
        let before = app.session.history.done().len();
        let trail = wave();
        app.pencil_press(trail[0]);
        for p in &trail[1..] {
            app.pencil_sample(*p, false);
        }
        assert!(app.cancel_gesture(&ctx), "a gesture was in flight");
        app.finish_pencil();
        assert_eq!(app.session.history.done().len(), before, "nothing recorded");
    }

    /// A trail that comes back near its start closes only after going well away
    /// from it, and is trimmed of the samples that came back into range.
    #[test]
    fn a_trail_closes_near_its_start_and_a_scribble_does_not() {
        let mut ring: Vec<Point> = (0..40)
            .map(|i| {
                let a = i as f64 / 40.0 * std::f64::consts::TAU;
                Point::new(50.0 * a.cos(), 50.0 * a.sin())
            })
            .collect();
        ring.push(Point::new(50.5, 2.0));
        let start = ring[0];
        assert!(closing(&mut ring, start, 1.0, false), "back within range");
        assert!(
            ring.last().is_some_and(|p| (*p - start).hypot() > CLOSE_PT),
            "trimmed of the samples back in range: {:?}",
            ring.last()
        );

        let mut scribble = vec![Point::ZERO, Point::new(6.0, 0.0), Point::new(1.0, 1.0)];
        assert!(
            !closing(&mut scribble, Point::ZERO, 1.0, false),
            "never far enough away to mean a loop"
        );
        assert_eq!(scribble.len(), 3, "an open trail is left alone");
    }

    /// Carrying a path on keeps its end anchor and hands it the stroke's first
    /// handle; closing folds the stroke's last anchor into the run's first.
    #[test]
    fn extending_joins_at_the_held_end_and_closes_onto_the_first() {
        let working = vec![
            PenAnchor::corner(Point::ZERO),
            PenAnchor::corner(Point::new(100.0, 0.0)),
        ];
        let mut first = PenAnchor::corner(Point::new(100.0, 0.0));
        first.out = Vec2::new(10.0, 10.0);
        let mut last = PenAnchor::corner(Point::new(0.0, 0.0));
        last.back = Vec2::new(-5.0, 5.0);
        let stroke = [first, PenAnchor::corner(Point::new(50.0, 80.0)), last];

        let open = extended(working.clone(), &stroke, false);
        assert_eq!(open.anchors.len(), 4, "the shared end is one anchor");
        assert_eq!(open.anchors[1].out, Vec2::new(10.0, 10.0));
        assert!(!open.closed);

        let shut = extended(working, &stroke, true);
        assert!(shut.closed);
        assert_eq!(
            shut.anchors.len(),
            3,
            "the landing anchor folds into the first"
        );
        assert_eq!(shut.anchors[0].back, Vec2::new(-5.0, 5.0));
    }
}
