//! Shape-parameter handles on the canvas: a selected rectangle's corner dots,
//! which set its corner radius, and a selected ellipse's arc dots —
//! the arc's start and end on the outline, and the ratio dot on the inner edge.
//!
//! Shown under Select for one unlocked, shown layer while the pointer is over it
//! or one of its dots is dragged; hidden during any other drag and while the
//! layer's shorter side is under [`MIN_SIDE_PX`] on screen. Everything is worked
//! in the layer's local space, so a rotated layer's dots turn with it.
//!
//! A corner dot sits inside its corner, inset by the radius along both edges, or
//! by [`ZERO_INSET_PX`] while the radius is 0 so it clears the resize handle.
//! Dragging one sets all four corners to one radius in whole points, from 0 to
//! half the shorter side; Alt sets that corner alone and opens the per-corner
//! disclosure. A drag unbinds the radius it writes from its variable.
//!
//! Dragging the arc's end sets Sweep, its start moves Start with the end held,
//! and the ratio dot sets Ratio. Shift snaps an angle to 15°.

use crate::app::OndinApp;
use crate::preview::Drag;
use crate::theme::{color, icon};
use eframe::egui;
use ondin_core::kurbo::{Affine, Point, RoundedRectRadii, Size, Vec2};
use ondin_core::{EllipseArc, GeometryPatch, NodeId, NodeKind, Operation, Transaction};

/// A dot's radius, and how near it a press must land, in screen points: the
/// reach is kept well past the drawn dot so a small dot stays easy to grab.
const DOT_R: f32 = 3.5;
const DOT_PICK_PX: f32 = 8.0;
/// A corner dot's inset while its radius is 0, in screen points.
const ZERO_INSET_PX: f64 = 12.0;
/// The shorter side, on screen, below which the dots hide; the loose run's centre
/// dots share it.
pub(crate) const MIN_SIDE_PX: f64 = 48.0;
/// Shift's angle step, in degrees.
const SNAP_DEG: f64 = 15.0;

/// What a press on the dots grabs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Grab {
    /// A corner dot: top left, top right, bottom right, bottom left.
    Corner(usize),
    ArcStart,
    ArcEnd,
    ArcRatio,
}

/// The corners' names, in [`Grab::Corner`]'s order.
const CORNER_NAMES: [&str; 4] = ["Top left", "Top right", "Bottom right", "Bottom left"];

/// The corner of a `w × h` box at index `k`, and the inward direction along its
/// diagonal, one unit along each edge.
fn corner_at(size: Size, k: usize) -> (Point, Vec2) {
    let (w, h) = (size.width, size.height);
    match k {
        0 => (Point::new(0.0, 0.0), Vec2::new(1.0, 1.0)),
        1 => (Point::new(w, 0.0), Vec2::new(-1.0, 1.0)),
        2 => (Point::new(w, h), Vec2::new(-1.0, -1.0)),
        _ => (Point::new(0.0, h), Vec2::new(1.0, -1.0)),
    }
}

fn radii_array(r: RoundedRectRadii) -> [f64; 4] {
    [r.top_left, r.top_right, r.bottom_right, r.bottom_left]
}

fn radii_of(a: [f64; 4]) -> RoundedRectRadii {
    RoundedRectRadii::new(a[0], a[1], a[2], a[3])
}

/// How far `p` sits in from corner `k`, measured along the corner's diagonal as
/// an inset along each edge.
fn inset(size: Size, k: usize, p: Point) -> f64 {
    let (c, d) = corner_at(size, k);
    (p - c).dot(d) * 0.5
}

/// The angle of local `p` on the ellipse filling `size`, in degrees clockwise
/// from three o'clock, `0..360`: taken on the circle before it is stretched.
fn angle_of(size: Size, p: Point) -> f64 {
    let (rx, ry) = (size.width * 0.5, size.height * 0.5);
    let u = Vec2::new(
        (p.x - rx) / rx.max(f64::EPSILON),
        (p.y - ry) / ry.max(f64::EPSILON),
    );
    u.y.atan2(u.x).to_degrees().rem_euclid(360.0)
}

/// The point at `deg` on the ellipse filling `size`, scaled by `k` toward the
/// centre.
fn on_ellipse(size: Size, deg: f64, k: f64) -> Point {
    let (rx, ry) = (size.width * 0.5, size.height * 0.5);
    let a = deg.to_radians();
    Point::new(rx + rx * k * a.cos(), ry + ry * k * a.sin())
}

/// A sweep of `raw` degrees (0..360) followed on from `last`, the previous
/// frame's unwrapped sweep, by the shorter way round: the sweep drawn, clamped to
/// 0..=360 so it stops at either end rather than wrapping, and the unwrapped value
/// to follow on from next, which keeps the stop until the pointer comes back the
/// way it went (at most a turn past either end).
fn stopped(raw: f64, last: f64) -> (f64, f64) {
    let delta = (raw - last).rem_euclid(360.0);
    let delta = if delta > 180.0 { delta - 360.0 } else { delta };
    let unwrapped = (last + delta).clamp(-360.0, 720.0);
    (unwrapped.clamp(0.0, 360.0), unwrapped)
}

fn snapped(deg: f64, shift: bool) -> f64 {
    if shift {
        (deg / SNAP_DEG).round() * SNAP_DEG
    } else {
        deg
    }
}

/// The layer the dots belong to, its world transform and its kind as drawn.
struct Subject {
    id: NodeId,
    world: Affine,
    kind: NodeKind,
}

impl Subject {
    fn size(&self) -> Option<Size> {
        self.kind.box_size()
    }
}

impl OndinApp {
    /// The one selected layer that shows dots, read through `kind_world`, or
    /// `None` ([module docs](self)).
    fn shape_subject_with(
        &self,
        kind_world: impl Fn(NodeId) -> Option<(NodeKind, Affine)>,
    ) -> Option<Subject> {
        if self.tool != crate::tools::Tool::Select
            || !self.selecting()
            || self.editing_in_place()
            || self.mode != crate::input::Mode::Normal
            || self.chrome_hidden()
            || self.edited_gradient().is_some()
        {
            return None;
        }
        let [id] = *self.session.selection.ids() else {
            return None;
        };
        if ondin_core::is_effectively_locked(&self.session.doc, id) || !self.shown_visible(id) {
            return None;
        }
        let (kind, world) = kind_world(id)?;
        // Not a frame: its radius is rarely set, and its Appearance fields suffice.
        if !matches!(kind, NodeKind::Rect { .. } | NodeKind::Ellipse { .. }) {
            return None;
        }
        let size = kind.box_size()?;
        let c = world.as_coeffs();
        let ppw = self.points_per_world();
        let w = Vec2::new(c[0], c[1]).hypot() * size.width * ppw;
        let h = Vec2::new(c[2], c[3]).hypot() * size.height * ppw;
        if w.min(h) < MIN_SIDE_PX {
            return None;
        }
        Some(Subject { id, world, kind })
    }

    /// The subject as drawn, a live drag included.
    fn shown_shape_subject(&self) -> Option<Subject> {
        self.shape_subject_with(|id| {
            Some((
                self.session.display_node(id)?.kind().clone(),
                self.session.preview_world_transform(id)?,
            ))
        })
    }

    /// The subject as committed: what a drag measures from.
    fn committed_shape_subject(&self) -> Option<Subject> {
        self.shape_subject_with(|id| {
            Some((
                self.session
                    .resolved
                    .used_kind(&self.session.doc, id)?
                    .clone(),
                self.session.resolved.world_transform(id)?,
            ))
        })
    }

    /// Local units per screen point at the subject's scale.
    fn local_per_point(&self, s: &Subject) -> f64 {
        let det = s.world.determinant().abs().sqrt();
        1.0 / (det * self.points_per_world()).max(f64::EPSILON)
    }

    /// The subject's dots in its local space, the arc's end ahead of its start
    /// so a whole ellipse's one dot drags the end.
    fn shape_dots(&self, s: &Subject) -> Vec<(Grab, Point)> {
        let Some(size) = s.size() else {
            return Vec::new();
        };
        match &s.kind {
            NodeKind::Ellipse { arc, .. } => {
                let arc = arc.normalized();
                vec![
                    (Grab::ArcEnd, on_ellipse(size, arc.start + arc.sweep, 1.0)),
                    (Grab::ArcStart, on_ellipse(size, arc.start, 1.0)),
                    (
                        Grab::ArcRatio,
                        on_ellipse(size, arc.start + arc.sweep * 0.5, arc.ratio),
                    ),
                ]
            }
            kind => {
                let Some(radii) = kind.corner_radii() else {
                    return Vec::new();
                };
                let half = size.width.min(size.height).max(0.0) * 0.5;
                let zero = ZERO_INSET_PX * self.local_per_point(s);
                radii_array(radii)
                    .into_iter()
                    .enumerate()
                    .map(|(k, r)| {
                        let r = r.clamp(0.0, half);
                        let (c, d) = corner_at(size, k);
                        (Grab::Corner(k), c + d * if r > 0.0 { r } else { zero })
                    })
                    .collect()
            }
        }
    }

    /// Whether the pointer at `screen` is over the subject or one of its dots.
    fn over_subject(&self, s: &Subject, screen: egui::Pos2, rect: egui::Rect, ppp: f32) -> bool {
        let Some(size) = s.size() else {
            return false;
        };
        let local = s.world.inverse() * self.to_world(screen, rect, ppp);
        let inside =
            local.x >= 0.0 && local.y >= 0.0 && local.x <= size.width && local.y <= size.height;
        inside
            || self.shape_dots(s).iter().any(|(_, p)| {
                self.to_screen(s.world * *p, rect, ppp).distance(screen) <= DOT_PICK_PX
            })
    }

    /// What a press at `screen` would grab: the nearest dot within reach. Only
    /// at rest.
    pub(crate) fn shape_handle_at(
        &self,
        screen: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
    ) -> Option<Grab> {
        if !self.drag.is_none() {
            return None;
        }
        let s = self.shown_shape_subject()?;
        self.shape_dots(&s)
            .into_iter()
            .map(|(g, p)| (g, self.to_screen(s.world * p, rect, ppp).distance(screen)))
            .filter(|(_, d)| *d <= DOT_PICK_PX)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(g, _)| g)
    }

    /// Start a dot drag at the press `grab`, or `false` where it grabs nothing.
    pub(crate) fn begin_shape_drag(
        &mut self,
        grab: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
        alt: bool,
    ) -> bool {
        let Some(what) = self.shape_handle_at(grab, rect, ppp) else {
            return false;
        };
        let Some(s) = self.committed_shape_subject() else {
            return false;
        };
        let press = s.world.inverse() * self.to_world(grab, rect, ppp);
        let last = match &s.kind {
            NodeKind::Ellipse { arc, .. } => arc.normalized().sweep,
            _ => 0.0,
        };
        let alt = alt && matches!(what, Grab::Corner(_));
        if alt {
            self.per_corner_radius.insert(s.id);
            self.corner_smoothing_open.remove(&s.id);
        }
        self.drag = Drag::ShapeHandle {
            id: s.id,
            grab: what,
            press,
            alt,
            last,
        };
        true
    }

    /// The transaction a dot drag at world `world` previews and commits, the
    /// readout beside the pointer, and the sweep it leaves for the next frame.
    pub(crate) fn shape_drag_tx(
        &self,
        world: Point,
        shift: bool,
    ) -> Option<(Transaction, String, f64)> {
        let Drag::ShapeHandle {
            id,
            grab,
            press,
            alt,
            last,
        } = self.drag
        else {
            return None;
        };
        let s = self.committed_shape_subject().filter(|s| s.id == id)?;
        let size = s.size()?;
        let local = s.world.inverse() * world;
        let geo = |geometry| Operation::SetGeometry { id, geometry };
        match (grab, &s.kind) {
            (Grab::Corner(k), kind) => {
                let drawn = radii_array(kind.corner_radii()?);
                let node = self.session.doc.get(id)?;
                let literal = radii_array(node.kind().corner_radii()?);
                let half = size.width.min(size.height).max(0.0) * 0.5;
                // From where the dot is drawn: a radius past half draws at half.
                let from = drawn[k].clamp(0.0, half);
                let travel = inset(size, k, local) - inset(size, k, press);
                let r = (from + travel).round().clamp(0.0, half.floor());
                // A drag that ends where it began changes nothing, its binding
                // included. Judged on the travel too: rounding moves a radius
                // ending in .5 by half a unit even when the pointer stays put.
                if travel.abs() < 0.5 || r == from {
                    return Some((Transaction(Vec::new()), format!("Radius {r}"), last));
                }
                let mut vars = node.corner_vars();
                let mut ops = Vec::new();
                let label = if alt {
                    vars[k] = None;
                    let written: [f64; 4] = std::array::from_fn(|j| match j == k {
                        true => r,
                        false if vars[j].is_some() => literal[j],
                        false => drawn[j],
                    });
                    ops.push(geo(GeometryPatch::CornerRadii(radii_of(written))));
                    format!("{} {}", CORNER_NAMES[k], crate::ui::number(0)(r, 0..=0))
                } else {
                    vars = Default::default();
                    ops.push(geo(GeometryPatch::CornerRadius(r)));
                    format!("Radius {}", crate::ui::number(0)(r, 0..=0))
                };
                // The drag unbinds what it writes; set here, so the commit's
                // binding keeper leaves the transaction alone.
                if vars != node.corner_vars() {
                    ops.push(Operation::SetCornerVars { id, vars });
                }
                Some((Transaction(ops), label, last))
            }
            (grab, NodeKind::Ellipse { arc, .. }) => {
                let arc = arc.normalized();
                let theta = angle_of(size, local);
                let mut next: EllipseArc = arc;
                let mut unwrapped = last;
                let label = match grab {
                    Grab::ArcEnd => {
                        let raw = snapped((theta - arc.start).rem_euclid(360.0), shift);
                        (next.sweep, unwrapped) = stopped(raw.rem_euclid(360.0), last);
                        format!("{}°", crate::ui::number(0)(next.sweep, 0..=0))
                    }
                    Grab::ArcStart => {
                        // The end holds; a stopped sweep holds the start with it.
                        let start = snapped(theta, shift).rem_euclid(360.0);
                        let end = arc.start + arc.sweep;
                        (next.sweep, unwrapped) = stopped((end - start).rem_euclid(360.0), last);
                        next.start = (end - next.sweep).rem_euclid(360.0);
                        format!("Start {}°", crate::ui::number(0)(next.start, 0..=0))
                    }
                    _ => {
                        let (rx, ry) = (size.width * 0.5, size.height * 0.5);
                        let u = Vec2::new(
                            (local.x - rx) / rx.max(f64::EPSILON),
                            (local.y - ry) / ry.max(f64::EPSILON),
                        );
                        let mid = (arc.start + arc.sweep * 0.5).to_radians();
                        let along = u.dot(Vec2::new(mid.cos(), mid.sin()));
                        next.ratio = (along * 100.0).round().clamp(0.0, 99.0) / 100.0;
                        format!("Ratio {}%", crate::ui::number(0)(next.ratio * 100.0, 0..=0))
                    }
                };
                let next = next.normalized();
                Some((
                    Transaction(vec![geo(GeometryPatch::Arc(next))]),
                    label,
                    unwrapped,
                ))
            }
            _ => None,
        }
    }

    /// One frame of a dot drag: preview it and remember the sweep.
    pub(crate) fn preview_shape_drag(&mut self, world: Point, shift: bool) {
        if let Some((tx, _, sweep)) = self.shape_drag_tx(world, shift) {
            self.session.set_preview(&tx);
            if let Drag::ShapeHandle { last, .. } = &mut self.drag {
                *last = sweep;
            }
        }
    }

    /// The release of a dot drag: one step, or nothing where nothing changed.
    pub(crate) fn finish_shape_drag(&mut self, world: Point, shift: bool) {
        let built = self.shape_drag_tx(world, shift);
        let grab = match self.drag {
            Drag::ShapeHandle { grab, .. } => Some(grab),
            _ => None,
        };
        self.drag = Drag::None;
        match (built, grab) {
            (Some((tx, _, _)), Some(grab)) if !tx.changes_nothing(&self.session.doc) => {
                let (label, glyph) = match grab {
                    Grab::Corner(_) => ("Corner radius", icon::CORNERS_OUT),
                    Grab::ArcStart => ("Start", icon::ARC_START),
                    Grab::ArcEnd => ("Sweep", icon::CHART_PIE_SLICE),
                    Grab::ArcRatio => ("Ratio", icon::CIRCLE_DASHED),
                };
                self.session.name_step(label, glyph);
                self.session.commit(tx);
            }
            _ => self.session.clear_preview(),
        }
    }

    /// The cursor over a dot, or while dragging one.
    pub(crate) fn shape_handle_cursor(&self, grab: Option<Grab>) -> Option<egui::CursorIcon> {
        match (&self.drag, grab) {
            (Drag::ShapeHandle { .. }, _) => Some(egui::CursorIcon::Grabbing),
            (_, Some(_)) => Some(egui::CursorIcon::Grab),
            _ => None,
        }
    }

    /// The dots: with the pointer over the subject at rest, or through a drag of
    /// one of them, the dragged dot filled and its value beside the pointer.
    pub(crate) fn draw_shape_handles(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        let dragged = match self.drag {
            Drag::ShapeHandle { grab, .. } => Some(grab),
            Drag::None => None,
            _ => return,
        };
        let Some(pointer) = painter.ctx().input(|i| i.pointer.latest_pos()) else {
            return;
        };
        let Some(s) = self.shown_shape_subject() else {
            return;
        };
        if dragged.is_none() && !self.over_subject(&s, pointer, rect, ppp) {
            return;
        }
        for (grab, p) in self.shape_dots(&s) {
            let at = self.to_screen(s.world * p, rect, ppp);
            if dragged == Some(grab) {
                painter.circle(
                    at,
                    DOT_R,
                    color::SELECT,
                    egui::Stroke::new(1.5, color::POINT),
                );
            } else {
                // The node anchors' near-white, the canvas's one handle white.
                painter.circle(
                    at,
                    DOT_R,
                    color::POINT,
                    egui::Stroke::new(1.5, color::SELECT),
                );
            }
        }
        if dragged.is_some() {
            let shift = painter.ctx().input(|i| i.modifiers.shift);
            let world = self.to_world(pointer, rect, ppp);
            if let Some((_, label, _)) = self.shape_drag_tx(world, shift) {
                let hue = crate::canvas::chrome_hue(&self.session.doc, s.id);
                crate::spacing::paint_label(painter, pointer, &label, None, hue);
            }
        }
    }
}

/// The dots driven through the whole canvas with real pointer events.
#[cfg(test)]
mod tests {
    use crate::app::OndinApp;
    use ondin_core::kurbo::{Affine, Point, RoundedRectRadii, Size};
    use ondin_core::{Document, EllipseArc, IdSource, NodeId, NodeKind, Operation, Transaction};

    const RECT: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::Pos2::new(800.0, 600.0),
    };

    fn frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        mods: egui::Modifiers,
        events: Vec<egui::Event>,
    ) {
        let time = ctx.input(|i| i.time) + 0.1;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(RECT),
                time: Some(time),
                modifiers: mods,
                events,
                ..Default::default()
            },
            |ui| app.canvas_ui(ui),
        );
    }

    /// Press at world `from`, drag to `to` in eight steps, release, holding `mods`.
    fn drag(
        ctx: &egui::Context,
        app: &mut OndinApp,
        from: Point,
        to: Point,
        mods: egui::Modifiers,
    ) {
        let at = |app: &OndinApp, w: Point| app.to_screen(w, RECT, 1.0);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: mods,
        };
        let start = at(app, from);
        frame(ctx, app, mods, vec![egui::Event::PointerMoved(start)]);
        frame(ctx, app, mods, vec![button(start, true)]);
        for step in 1..=8 {
            let p = at(app, from + (to - from) * (step as f64 / 8.0));
            frame(ctx, app, mods, vec![egui::Event::PointerMoved(p)]);
        }
        frame(ctx, app, mods, vec![button(at(app, to), false)]);
        frame(ctx, app, mods, Vec::new());
    }

    /// A 600 × 300 frame at the origin holding `kind` at (100, 60), selected;
    /// camera on the frame's centre at 100%.
    fn app_with(ctx: &egui::Context, kind: NodeKind) -> (OndinApp, NodeId) {
        crate::theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x5AD1);
        let root = ids.mint();
        let (board, id) = (ids.mint(), ids.mint());
        let mut doc = Document::new(root);
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: board,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(600.0, 300.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
            Operation::CreateNode {
                id,
                parent: board,
                index: 0,
                kind,
                transform: Some(Affine::translate((100.0, 60.0))),
                name: None,
            },
        ]))
        .expect("the frame and the shape");
        app.session.adopt_document(doc, None);
        app.session.camera.center = Point::new(300.0, 150.0);
        app.session.camera.zoom = 1.0;
        app.session.selection.set_one(id);
        for _ in 0..2 {
            frame(ctx, &mut app, egui::Modifiers::NONE, Vec::new());
        }
        (app, id)
    }

    fn square(side: f64) -> NodeKind {
        NodeKind::Rect {
            size: Size::new(side, side),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        }
    }

    fn radii(app: &OndinApp, id: NodeId) -> [f64; 4] {
        super::radii_array(
            app.session
                .doc
                .get(id)
                .unwrap()
                .kind()
                .corner_radii()
                .unwrap(),
        )
    }

    /// A corner dot, 12 pt in at radius 0, dragged 20 pt along the diagonal sets
    /// all four corners to 20 in one *Corner radius* step; Alt then drags the top
    /// left alone and opens the per-corner fields.
    /// flip: `shape_drag_tx` writing only the dragged corner without Alt → fails
    /// at "all four".
    #[test]
    fn a_corner_dot_sets_every_corner_and_alt_one() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with(&ctx, square(120.0));
        drag(
            &ctx,
            &mut app,
            Point::new(112.0, 72.0),
            Point::new(132.0, 92.0),
            egui::Modifiers::NONE,
        );
        assert_eq!(radii(&app, id), [20.0; 4], "all four");
        let step = app.session.history.done().last().expect("a step");
        assert_eq!(step.note.label, "Corner radius");

        // The dot now sits 20 in.
        drag(
            &ctx,
            &mut app,
            Point::new(120.0, 80.0),
            Point::new(136.0, 96.0),
            egui::Modifiers::ALT,
        );
        assert_eq!(radii(&app, id), [36.0, 20.0, 20.0, 20.0]);
        assert!(app.per_corner_radius.contains(&id), "the disclosure opens");
    }

    /// The radius stops at half the shorter side.
    #[test]
    fn a_corner_dot_stops_at_half_the_shorter_side() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with(&ctx, square(120.0));
        drag(
            &ctx,
            &mut app,
            Point::new(112.0, 72.0),
            Point::new(250.0, 210.0),
            egui::Modifiers::NONE,
        );
        assert_eq!(radii(&app, id), [60.0; 4]);
    }

    /// A shape under 48 pt on screen shows no dots.
    #[test]
    fn a_small_shape_has_no_dots() {
        let ctx = egui::Context::default();
        let (app, _) = app_with(&ctx, square(40.0));
        let at = app.to_screen(Point::new(112.0, 72.0), RECT, 1.0);
        assert_eq!(app.shape_handle_at(at, RECT, 1.0), None);
        let (app, _) = app_with(&ctx, square(120.0));
        assert_eq!(
            app.shape_handle_at(at, RECT, 1.0),
            Some(super::Grab::Corner(0)),
            "fixture: a large one does"
        );
    }

    /// The end dot of a whole ellipse dragged round to 225° with Shift sets Sweep
    /// on the 15° step.
    #[test]
    fn the_end_dot_sets_the_sweep() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with(
            &ctx,
            NodeKind::Ellipse {
                size: Size::new(120.0, 120.0),
                arc: EllipseArc::default(),
            },
        );
        // Centre (160, 120), radius 60: the end sits at three o'clock.
        let a = 222.0_f64.to_radians();
        drag(
            &ctx,
            &mut app,
            Point::new(220.0, 120.0),
            Point::new(160.0 + 60.0 * a.cos(), 120.0 + 60.0 * a.sin()),
            egui::Modifiers::SHIFT,
        );
        let NodeKind::Ellipse { arc, .. } = app.session.doc.get(id).unwrap().kind() else {
            panic!("not an ellipse");
        };
        assert_eq!(arc.sweep, 225.0);
        assert_eq!(arc.start, 0.0);
        let step = app.session.history.done().last().expect("a step");
        assert_eq!(step.note.label, "Sweep");
    }

    /// Dragged on past its start, a whole ellipse's end stays at 360 rather than
    /// wrapping round to a thin wedge.
    /// flip: `stopped` returning the raw angle → fails at "stays whole".
    #[test]
    fn the_sweep_stops_at_360() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with(
            &ctx,
            NodeKind::Ellipse {
                size: Size::new(120.0, 120.0),
                arc: EllipseArc::default(),
            },
        );
        let a = 20.0_f64.to_radians();
        drag(
            &ctx,
            &mut app,
            Point::new(220.0, 120.0),
            Point::new(160.0 + 60.0 * a.cos(), 120.0 + 60.0 * a.sin()),
            egui::Modifiers::NONE,
        );
        let NodeKind::Ellipse { arc, .. } = app.session.doc.get(id).unwrap().kind() else {
            panic!("not an ellipse");
        };
        assert_eq!(arc.sweep, 360.0, "stays whole");
    }

    /// Press, move through `path` a frame a point, release at its last point.
    fn drag_through(ctx: &egui::Context, app: &mut OndinApp, path: &[Point]) {
        let mods = egui::Modifiers::NONE;
        let at = |app: &OndinApp, w: Point| app.to_screen(w, RECT, 1.0);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: mods,
        };
        let start = at(app, path[0]);
        frame(ctx, app, mods, vec![egui::Event::PointerMoved(start)]);
        frame(ctx, app, mods, vec![button(start, true)]);
        for p in &path[1..] {
            let p = at(app, *p);
            frame(ctx, app, mods, vec![egui::Event::PointerMoved(p)]);
        }
        let end = at(app, *path.last().unwrap());
        frame(ctx, app, mods, vec![button(end, false)]);
        frame(ctx, app, mods, Vec::new());
    }

    /// Points on the 120 pt circle at (160, 120), from `from`° to `to`° clockwise
    /// in 10° steps.
    fn round(from: f64, to: f64) -> Vec<Point> {
        let n = ((to - from).abs() / 10.0).ceil() as usize;
        (0..=n)
            .map(|i| {
                let a = (from + (to - from) * i as f64 / n as f64).to_radians();
                Point::new(160.0 + 60.0 * a.cos(), 120.0 + 60.0 * a.sin())
            })
            .collect()
    }

    fn arc_of(app: &OndinApp, id: NodeId) -> EllipseArc {
        let NodeKind::Ellipse { arc, .. } = app.session.doc.get(id).unwrap().kind() else {
            panic!("not an ellipse");
        };
        *arc
    }

    /// Dragged half way round past its start, a whole ellipse stays whole; the
    /// stop holds until the pointer comes back the way it went.
    /// flip: `stopped` without its unwrapping, holding only within 90° of the
    /// crossing → fails at "still whole at 180°".
    #[test]
    fn the_sweep_stop_holds_until_the_pointer_comes_back() {
        let ctx = egui::Context::default();
        let whole = NodeKind::Ellipse {
            size: Size::new(120.0, 120.0),
            arc: EllipseArc::default(),
        };
        let (mut app, id) = app_with(&ctx, whole);
        drag_through(&ctx, &mut app, &round(0.0, 180.0));
        assert_eq!(arc_of(&app, id).sweep, 360.0, "still whole at 180°");
    }

    /// Dragged past its end, a start dot stops with the end held where it was.
    /// flip: setting `next.start` to the pointer's angle in the `ArcStart` arm →
    /// fails at "the end held".
    #[test]
    fn a_start_dragged_past_its_end_holds_the_end() {
        let ctx = egui::Context::default();
        let quarter = NodeKind::Ellipse {
            size: Size::new(120.0, 120.0),
            arc: EllipseArc {
                sweep: 90.0,
                ..EllipseArc::default()
            },
        };
        let (mut app, id) = app_with(&ctx, quarter);
        drag_through(&ctx, &mut app, &round(0.0, 120.0));
        let arc = arc_of(&app, id);
        assert_eq!(arc.sweep, 0.0, "stopped at nothing");
        assert!(
            ((arc.start + arc.sweep).rem_euclid(360.0) - 90.0).abs() < 1e-9,
            "the end held: {arc:?}"
        );
    }

    /// On a pill whose radius is past half its shorter side, the corner dot drags
    /// from where it is drawn, and a click on it commits nothing.
    /// flip: basing the drag on the stored radius → fails at "the drag lowers it".
    #[test]
    fn an_over_half_radius_drags_from_its_dot() {
        let ctx = egui::Context::default();
        let pill = NodeKind::Rect {
            size: Size::new(160.0, 60.0),
            corner_radii: RoundedRectRadii::from_single_radius(999.0),
            corner_smoothing: 0.0,
        };
        let (mut app, id) = app_with(&ctx, pill);
        let depth = app.session.history.undo_depth();
        let dot = Point::new(130.0, 90.0);
        let at = app.to_screen(dot, RECT, 1.0);
        assert!(
            matches!(
                app.shape_handle_at(at, RECT, 1.0),
                Some(super::Grab::Corner(_))
            ),
            "fixture: the dot is there"
        );
        drag_through(&ctx, &mut app, &[dot, dot]);
        assert_eq!(
            app.session.history.undo_depth(),
            depth,
            "a click is no step"
        );
        assert_eq!(radii(&app, id), [999.0; 4]);
        // Straight left: the left dots coincide, and either reads this inset.
        drag_through(
            &ctx,
            &mut app,
            &[dot, Point::new(125.0, 90.0), Point::new(120.0, 90.0)],
        );
        assert_eq!(radii(&app, id), [25.0; 4], "the drag lowers it");
    }

    /// A drag on the dot of a pill whose shorter side is odd, so its drawn radius
    /// ends in .5, that comes back to where it began commits nothing.
    /// flip: `shape_drag_tx` judging "no change" on the rounded radius alone →
    /// fails at "no step". A press and release in place starts no drag, so
    /// that path never reaches `shape_drag_tx` either way.
    #[test]
    fn a_drag_back_on_a_half_unit_radius_commits_nothing() {
        let ctx = egui::Context::default();
        let pill = NodeKind::Rect {
            size: Size::new(160.0, 61.0),
            corner_radii: RoundedRectRadii::from_single_radius(999.0),
            corner_smoothing: 0.0,
        };
        let (mut app, id) = app_with(&ctx, pill);
        let depth = app.session.history.undo_depth();
        let dot = Point::new(130.5, 90.5);
        let at = app.to_screen(dot, RECT, 1.0);
        assert!(
            matches!(
                app.shape_handle_at(at, RECT, 1.0),
                Some(super::Grab::Corner(_))
            ),
            "fixture: the dot is there"
        );
        drag_through(&ctx, &mut app, &[dot, Point::new(120.5, 90.5), dot]);
        assert_eq!(app.session.history.undo_depth(), depth, "no step");
        assert_eq!(radii(&app, id), [999.0; 4]);
    }
}
