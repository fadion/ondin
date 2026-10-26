//! The gradient handles on the canvas (the design canvas's *Gradient Editing*
//! artboard).
//!
//! **The open picker is the mode.** While it edits a gradient fill or stroke of the
//! one selected layer under a selecting tool, the canvas draws the gradient's guide
//! (a linear's line, a radial's axes and outline, a sweep's ring), its handles and
//! its stops, and the selection box steps aside (`OndinApp::editing_in_place`). The
//! stops are the picker's stop bar's: selecting one here selects it there.
//!
//! Every edit goes through the picker's slot verbs on the brush **as drawn**
//! (`OndinApp::slot_brush`), so a gradient layout stretches is edited on the box it
//! is drawn on. A drag re-derives from the brush at the press each frame, previews
//! through `preview_slot` and commits once through `write_slot`.

use crate::app::OndinApp;
use crate::input::Mode;
use crate::panels::paint::{self, PaintKind, PaintSlot};
use crate::preview::Drag;
use crate::theme::color;
use crate::ui::color_to_egui;
use eframe::egui;
use ondin_core::kurbo::{Affine, Point, Rect, Vec2};
use ondin_core::peniko::Color;
use ondin_core::{Brush, NodeId};

/// A handle's drawn radius, and the dots' that stand for the stops between.
const HANDLE_R: f32 = 6.0;
const DOT_R: f32 = 4.5;
/// How near a press must land to a handle or a dot to grab it, and to the guide to
/// add a stop there, in screen points.
const PICK_PX: f32 = 8.0;
const GUIDE_PICK_PX: f32 = 5.0;
/// How near an end or a centre must come to one of the box's nine points to snap
/// to it, in screen points.
const SNAP_PX: f32 = 6.0;
/// A sweep has no radius, so its stops sit on a ring of this screen radius.
const RING_PX: f32 = 50.0;
/// An offset this near either end is the handle's, drawn as no dot of its own.
const END_EPS: f32 = 1e-4;

/// What a press on the gradient grabbed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Grab {
    /// A linear's offset-0 end.
    Start,
    /// A linear's offset-1 end.
    End,
    /// A radial's or a sweep's centre.
    Centre,
    /// A radial's first axis: its radius, and its turn.
    AxisA,
    /// A radial's second axis: the other radius only.
    AxisB,
    /// A sweep's start, on the ring.
    Turn,
    /// The stop at this index, between the ends.
    Stop(usize),
}

/// The guide the stops sit on, in screen points.
enum Guide {
    Segment(egui::Pos2, egui::Pos2),
    Ring(egui::Pos2, f32),
}

/// The edited gradient laid out on screen for one frame.
struct Marks {
    id: NodeId,
    slot: PaintSlot,
    /// As drawn, in the layer's space.
    brush: Brush,
    /// The layer's space to world.
    world: Affine,
    handles: Vec<(Grab, egui::Pos2, Option<Color>)>,
    stops: Vec<(usize, egui::Pos2, Color)>,
    guide: Guide,
    /// A radial's outer edge, as a closed polyline.
    outline: Vec<egui::Pos2>,
}

impl Marks {
    /// The handle or dot within [`PICK_PX`] of `at`, handles first.
    fn grab_at(&self, at: egui::Pos2) -> Option<Grab> {
        let nearest = |items: &mut dyn Iterator<Item = (Grab, egui::Pos2)>| {
            items
                .map(|(g, p)| (g, p.distance(at)))
                .filter(|(_, d)| *d <= PICK_PX)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(g, _)| g)
        };
        nearest(&mut self.handles.iter().map(|(g, p, _)| (*g, *p)))
            .or_else(|| nearest(&mut self.stops.iter().map(|(i, p, _)| (Grab::Stop(*i), *p))))
    }

    /// Whether `at` is on the guide's bare stretch, where a click adds a stop.
    fn on_guide(&self, at: egui::Pos2) -> bool {
        match self.guide {
            Guide::Segment(a, b) => segment_distance(at, a, b) <= GUIDE_PICK_PX,
            Guide::Ring(c, r) => (at.distance(c) - r).abs() <= GUIDE_PICK_PX,
        }
    }
}

fn segment_distance(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    let t = match len2 > 1e-6 {
        true => ((p - a).dot(ab) / len2).clamp(0.0, 1.0),
        false => 0.0,
    };
    p.distance(a + ab * t)
}

/// `v` turned a quarter, clockwise on screen.
fn perp(v: Vec2) -> Vec2 {
    Vec2::new(-v.y, v.x)
}

/// `d` held to the nearest eighth of a turn, its length along that direction kept.
fn eighths(d: Vec2) -> Vec2 {
    let step = std::f64::consts::FRAC_PI_4;
    let u = Vec2::from_angle((d.y.atan2(d.x) / step).round() * step);
    u * d.dot(u).max(0.0)
}

impl OndinApp {
    /// The gradient the open picker is editing on the canvas: its layer, its slot,
    /// and the brush as drawn. `None` unless a selecting tool is up, the picker
    /// shows a gradient face (not *Variables*) for a fill or a stroke, and that
    /// layer is the whole selection.
    pub(crate) fn edited_gradient(&self) -> Option<(NodeId, PaintSlot, Brush)> {
        if self.mode != Mode::Normal || !self.selecting() {
            return None;
        }
        let p = self.picker.as_ref()?;
        if p.browse || !matches!(p.slot, PaintSlot::Fill(_) | PaintSlot::Stroke(_)) {
            return None;
        }
        if self.session.selection.ids() != [p.node] {
            return None;
        }
        let brush = self.slot_brush(p.node, p.slot)?;
        paint::kind_of(&brush)
            .is_gradient()
            .then_some((p.node, p.slot, brush))
    }

    /// The stop the picker's colour controls edit.
    fn gradient_stop(&self) -> usize {
        self.picker.as_ref().map_or(0, |p| p.stop)
    }

    fn select_gradient_stop(&mut self, i: usize) {
        if let Some(p) = &mut self.picker {
            p.stop = i;
            p.hex = None;
        }
    }

    fn gradient_marks(&self, rect: egui::Rect, ppp: f32) -> Option<Marks> {
        let (id, slot, brush) = self.edited_gradient()?;
        let world = self.session.preview_world_transform(id)?;
        let to_s = |p: Point| self.to_screen(world * p, rect, ppp);
        let stops = paint::stops_of(&brush);
        let at = |t: f32| paint::sample(&stops, t);
        let mut handles = Vec::new();
        let mut outline = Vec::new();
        // Where offset `t` sits on screen.
        let place: Box<dyn Fn(f32) -> Option<egui::Pos2> + '_>;
        let guide = match paint::kind_of(&brush) {
            PaintKind::Linear => {
                let (a, b) = paint::linear_ends(&brush)?;
                handles.push((Grab::Start, to_s(a), Some(at(0.0))));
                handles.push((Grab::End, to_s(b), Some(at(1.0))));
                place = Box::new(|t| paint::guide_point(&brush, t).map(to_s));
                Guide::Segment(to_s(a), to_s(b))
            }
            PaintKind::Radial => {
                let (c, a, b) = paint::radial_axes(&brush)?;
                handles.push((Grab::Centre, to_s(c), Some(at(0.0))));
                handles.push((Grab::AxisA, to_s(c + a), Some(at(1.0))));
                handles.push((Grab::AxisB, to_s(c + b), None));
                outline = (0..64)
                    .map(|i| {
                        let (s, k) = (f64::from(i) / 64.0 * std::f64::consts::TAU).sin_cos();
                        to_s(c + a * k + b * s)
                    })
                    .collect();
                place = Box::new(|t| paint::guide_point(&brush, t).map(to_s));
                Guide::Segment(to_s(c), to_s(c + a))
            }
            PaintKind::Sweep => {
                let (c, _) = paint::sweep_geom(&brush)?;
                let cs = to_s(c);
                // A shape-space direction onto the ring, measured on screen so the
                // ring stays round under any transform.
                let on_ring = move |d: Vec2| {
                    let v = to_s(c + d) - cs;
                    (v.length() > 1e-6).then(|| cs + v.normalized() * RING_PX)
                };
                handles.push((Grab::Centre, cs, None));
                handles.push((
                    Grab::Turn,
                    on_ring(paint::sweep_dir(&brush, 0.0)?)?,
                    Some(at(0.0)),
                ));
                let held = &brush;
                place = Box::new(move |t| paint::sweep_dir(held, t).and_then(on_ring));
                Guide::Ring(cs, RING_PX)
            }
            PaintKind::Solid | PaintKind::Image => return None,
        };
        let dots = stops
            .iter()
            .enumerate()
            .filter(|(_, (t, _))| *t > END_EPS && *t < 1.0 - END_EPS)
            .filter_map(|(i, (t, c))| place(*t).map(|p| (i, p, *c)))
            .collect();
        drop(place);
        Some(Marks {
            id,
            slot,
            brush,
            world,
            handles,
            stops: dots,
            guide,
            outline,
        })
    }

    /// Whether the gradient's handles take a press at `screen` — for the guide
    /// ladder's question, `canvas::chrome_claims`.
    pub(crate) fn gradient_claims(&self, screen: egui::Pos2, rect: egui::Rect, ppp: f32) -> bool {
        self.gradient_marks(rect, ppp)
            .is_some_and(|m| m.grab_at(screen).is_some() || m.on_guide(screen))
    }

    /// The cursor over the gradient's handles: a grab over one, and the plain arrow
    /// over the bare guide, where the ghost dot is the affordance for adding a
    /// stop. `None` elsewhere.
    pub(crate) fn gradient_cursor(
        &self,
        screen: Option<egui::Pos2>,
        rect: egui::Rect,
        ppp: f32,
    ) -> Option<egui::CursorIcon> {
        if matches!(self.drag, Drag::Gradient { .. }) {
            return Some(egui::CursorIcon::Grabbing);
        }
        let m = self.gradient_marks(rect, ppp)?;
        let at = screen?;
        if self.selection_locked() {
            return None;
        }
        if m.grab_at(at).is_some() {
            return Some(egui::CursorIcon::Grab);
        }
        m.on_guide(at).then_some(egui::CursorIcon::Default)
    }

    /// Start a drag on the gradient's handle or stop under `grab`, selecting the
    /// stop it names. False when the press is not the gradient's.
    pub(crate) fn begin_gradient_drag(
        &mut self,
        grab: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
    ) -> bool {
        if self.selection_locked() {
            return false;
        }
        let Some(m) = self.gradient_marks(rect, ppp) else {
            return false;
        };
        let Some(g) = m.grab_at(grab) else {
            return false;
        };
        if let Some(i) = self.stop_of_grab(&m.brush, g) {
            self.select_gradient_stop(i);
        }
        self.drag = Drag::Gradient {
            id: m.id,
            slot: m.slot,
            grab: g,
            from: Box::new(m.brush),
        };
        true
    }

    /// The stop a grab stands for: a dot's own, or the one at the end a handle
    /// sits on. `None` for a handle that carries no stop.
    fn stop_of_grab(&self, brush: &Brush, g: Grab) -> Option<usize> {
        let stops = paint::stops_of(brush);
        let last = stops.len().saturating_sub(1);
        match (g, paint::kind_of(brush)) {
            (Grab::Stop(i), _) => Some(i),
            (Grab::Start | Grab::Turn, _) | (Grab::Centre, PaintKind::Radial) => Some(0),
            (Grab::End | Grab::AxisA, _) => Some(last),
            (Grab::Centre | Grab::AxisB, _) => None,
        }
    }

    /// A click on the gradient: on a handle or a dot it selects that stop, on the
    /// bare guide it adds one there (sampled, so the ramp does not change) and
    /// selects it. True when the click was the gradient's.
    pub(crate) fn gradient_click(
        &mut self,
        screen: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
    ) -> bool {
        if self.selection_locked() {
            return false;
        }
        let Some(m) = self.gradient_marks(rect, ppp) else {
            return false;
        };
        if let Some(g) = m.grab_at(screen) {
            if let Some(i) = self.stop_of_grab(&m.brush, g) {
                self.select_gradient_stop(i);
            }
            return true;
        }
        if !m.on_guide(screen) {
            return false;
        }
        let local = m.world.inverse() * self.to_world(screen, rect, ppp);
        let Some(t) = paint::offset_at(&m.brush, local) else {
            return true;
        };
        let mut stops = paint::stops_of(&m.brush);
        stops.push((t, paint::sample(&stops, t)));
        let next = paint::with_stops(&m.brush, stops);
        let index = paint::stops_of(&next)
            .iter()
            .position(|s| s.0 == t)
            .unwrap_or(0);
        self.write_slot(m.id, m.slot, next);
        self.select_gradient_stop(index);
        true
    }

    /// A right-click on a stop's dot removes the stop, as on the picker's bar, while
    /// more than two are left. True when it landed on the gradient's handles, which
    /// claim it from the context menu either way.
    pub(crate) fn gradient_secondary(
        &mut self,
        screen: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
    ) -> bool {
        if self.selection_locked() {
            return false;
        }
        let Some(m) = self.gradient_marks(rect, ppp) else {
            return false;
        };
        let Some(g) = m.grab_at(screen) else {
            return false;
        };
        let mut stops = paint::stops_of(&m.brush);
        if let Grab::Stop(i) = g
            && stops.len() > 2
        {
            stops.remove(i);
            let last = stops.len() - 1;
            self.write_slot(m.id, m.slot, paint::with_stops(&m.brush, stops));
            let keep = self.gradient_stop().min(last);
            self.select_gradient_stop(keep);
        }
        true
    }

    /// The brush a gradient drag makes with the pointer at `world`, re-derived from
    /// `from` (the brush at the press), and the index the dragged stop sorts to.
    pub(crate) fn gradient_drag_brush(
        &self,
        grab: Grab,
        from: &Brush,
        world: Point,
        shift: bool,
        rect: egui::Rect,
        ppp: f32,
    ) -> (Brush, Option<usize>) {
        let Some((id, _, _)) = self.edited_gradient() else {
            return (from.clone(), None);
        };
        let Some(xf) = self
            .session
            .preview_world_transform(id)
            .filter(|t| ondin_core::build::affine_is_invertible(*t))
        else {
            return (from.clone(), None);
        };
        let p = xf.inverse() * world;
        // One of the box's nine points within `SNAP_PX` of the pointer on screen.
        let snapped = |p: Point| -> Point {
            if shift {
                return p;
            }
            let Some(b) = self.session.preview_local_box(id) else {
                return p;
            };
            let at = self.to_screen(xf * p, rect, ppp);
            box_points(b)
                .into_iter()
                .map(|q| (q, self.to_screen(xf * q, rect, ppp).distance(at)))
                .filter(|(_, d)| *d <= SNAP_PX)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map_or(p, |(q, _)| q)
        };
        let next = match grab {
            Grab::Start | Grab::End => {
                let Some((a, b)) = paint::linear_ends(from) else {
                    return (from.clone(), None);
                };
                let fixed = if grab == Grab::Start { b } else { a };
                let moved = match shift {
                    true => fixed + eighths(p - fixed),
                    false => snapped(p),
                };
                match grab {
                    Grab::Start => paint::with_linear_ends(from, moved, b),
                    _ => paint::with_linear_ends(from, a, moved),
                }
            }
            Grab::Centre => match paint::kind_of(from) {
                PaintKind::Radial => match paint::radial_axes(from) {
                    Some((_, a, b)) => paint::with_radial_axes(from, snapped(p), a, b),
                    None => from.clone(),
                },
                _ => match paint::sweep_geom(from) {
                    Some((_, deg)) => paint::with_sweep_geom(from, snapped(p), deg),
                    None => from.clone(),
                },
            },
            Grab::AxisA => match paint::radial_axes(from) {
                Some((c, a, b)) => {
                    let a2 = p - c;
                    if a2.hypot() < 1e-6 {
                        return (from.clone(), None);
                    }
                    // The second axis stays square to the first, on the side it was.
                    let side = if a.cross(b) >= 0.0 { 1.0 } else { -1.0 };
                    let len = if shift { a2.hypot() } else { b.hypot() };
                    paint::with_radial_axes(from, c, a2, perp(a2).normalize() * len * side)
                }
                None => from.clone(),
            },
            Grab::AxisB => match paint::radial_axes(from) {
                Some((c, a, b)) => {
                    let n = perp(a).normalize();
                    let n = if n.dot(b) >= 0.0 { n } else { -n };
                    let len = (p - c).dot(n).abs().max(0.5);
                    paint::with_radial_axes(from, c, a, n * len)
                }
                None => from.clone(),
            },
            Grab::Turn => match paint::sweep_geom(from) {
                Some((c, _)) => {
                    let v = p - c;
                    let mut deg = v.y.atan2(v.x).to_degrees();
                    if shift {
                        deg = (deg / 45.0).round() * 45.0;
                    }
                    paint::with_sweep_geom(from, c, deg.rem_euclid(360.0))
                }
                None => from.clone(),
            },
            Grab::Stop(i) => {
                let Some(t) = paint::offset_at(from, p) else {
                    return (from.clone(), None);
                };
                let mut stops = paint::stops_of(from);
                let Some(moved) = stops.get_mut(i) else {
                    return (from.clone(), None);
                };
                moved.0 = t;
                let colour = moved.1;
                let next = paint::with_stops(from, stops);
                let index = paint::stops_of(&next)
                    .iter()
                    .position(|s| s.0 == t && s.1 == colour);
                return (next, index);
            }
        };
        (next, None)
    }

    /// Preview a gradient drag; `world` is the pointer.
    pub(crate) fn update_gradient_drag(&mut self, world: Point, shift: bool) {
        let Drag::Gradient {
            id,
            slot,
            grab,
            from,
        } = &self.drag
        else {
            return;
        };
        let (id, slot, grab, from) = (*id, *slot, *grab, from.clone());
        let (rect, ppp) = (self.canvas_rect, self.canvas_ppp);
        let (next, index) = self.gradient_drag_brush(grab, &from, world, shift, rect, ppp);
        self.preview_slot(id, slot, next);
        if let Some(i) = index {
            self.select_gradient_stop(i);
        }
    }

    /// Commit a gradient drag: one undo step for the whole gesture.
    pub(crate) fn finish_gradient_drag(
        &mut self,
        id: NodeId,
        slot: PaintSlot,
        grab: Grab,
        from: &Brush,
        world: Point,
        shift: bool,
    ) {
        let (rect, ppp) = (self.canvas_rect, self.canvas_ppp);
        let (next, index) = self.gradient_drag_brush(grab, from, world, shift, rect, ppp);
        self.write_slot(id, slot, next);
        if let Some(i) = index {
            self.select_gradient_stop(i);
        }
    }

    /// The gradient's guide, handles and stops, and a drag's value beside the
    /// pointer.
    pub(crate) fn draw_gradient_handles(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
    ) {
        let Some(m) = self.gradient_marks(rect, ppp) else {
            return;
        };
        let under = egui::Stroke::new(3.0, egui::Color32::from_black_alpha(115));
        let line = egui::Stroke::new(1.0, egui::Color32::WHITE);
        let both = |shape: &dyn Fn(egui::Stroke) -> egui::Shape| {
            painter.add(shape(under));
            painter.add(shape(line));
        };
        if !m.outline.is_empty() {
            let pts = m.outline.clone();
            both(&|s| egui::Shape::closed_line(pts.clone(), s));
        }
        match m.guide {
            Guide::Segment(a, b) => both(&|s| egui::Shape::line_segment([a, b], s)),
            Guide::Ring(c, r) => both(&|s| egui::Shape::circle_stroke(c, r, s)),
        }
        // A radial's second axis is a spoke of its own.
        if let (Some((_, c, _)), Some((_, b, _))) = (
            m.handles.iter().find(|h| h.0 == Grab::Centre),
            m.handles.iter().find(|h| h.0 == Grab::AxisB),
        ) {
            let (c, b) = (*c, *b);
            both(&|s| egui::Shape::line_segment([c, b], s));
        }
        let selected = self.gradient_stop();
        let ring = egui::Stroke::new(2.0, color::SELECT);
        let pointer = painter.ctx().input(|i| i.pointer.latest_pos());
        // A hollow ghost where a click on the bare guide would add a stop.
        if self.drag.is_none()
            && let Some(at) = pointer
            && m.grab_at(at).is_none()
            && m.on_guide(at)
            && !self.selection_locked()
        {
            let local = m.world.inverse() * self.to_world(at, rect, ppp);
            if let Some(t) = paint::offset_at(&m.brush, local) {
                let stops = paint::stops_of(&m.brush);
                let ghost = match m.guide {
                    Guide::Segment(a, b) => {
                        let ab = b - a;
                        let k = ((at - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
                        a + ab * k
                    }
                    Guide::Ring(c, r) => c + (at - c).normalized() * r,
                };
                painter.circle_filled(
                    ghost,
                    DOT_R,
                    color_to_egui(paint::sample(&stops, t)).gamma_multiply(0.7),
                );
                painter.circle_stroke(ghost, DOT_R, egui::Stroke::new(1.0, egui::Color32::WHITE));
            }
        }
        for (i, at, c) in &m.stops {
            if *i == selected {
                painter.circle_stroke(*at, DOT_R + 5.0, ring);
            }
            painter.circle_filled(*at, DOT_R, color_to_egui(*c));
            painter.circle_stroke(*at, DOT_R, egui::Stroke::new(1.5, egui::Color32::WHITE));
        }
        let stopped = self.stop_on_handle(&m.brush, selected);
        for (g, at, fill) in &m.handles {
            if stopped == Some(*g) {
                painter.circle_stroke(*at, HANDLE_R + 5.0, ring);
            }
            painter.circle_stroke(
                *at,
                HANDLE_R + 1.5,
                egui::Stroke::new(1.0, egui::Color32::from_black_alpha(90)),
            );
            let ink = fill.map_or(egui::Color32::WHITE, color_to_egui);
            match g {
                // The radii are squares, so they do not read as stops.
                Grab::AxisA | Grab::AxisB => {
                    let r = egui::Rect::from_center_size(*at, egui::vec2(10.0, 10.0));
                    painter.rect_filled(r, egui::CornerRadius::same(2), ink);
                    painter.rect_stroke(
                        r,
                        egui::CornerRadius::same(2),
                        egui::Stroke::new(2.0, egui::Color32::WHITE),
                        egui::StrokeKind::Middle,
                    );
                }
                _ => {
                    painter.circle_filled(*at, HANDLE_R, ink);
                    painter.circle_stroke(
                        *at,
                        HANDLE_R,
                        egui::Stroke::new(2.0, egui::Color32::WHITE),
                    );
                }
            }
        }
        if let (Drag::Gradient { grab, .. }, Some(at)) = (&self.drag, pointer)
            && let Some(text) = gradient_readout(&m.brush, *grab)
        {
            let hue = crate::canvas::chrome_hue(&self.session.doc, m.id);
            crate::spacing::paint_label(painter, at, &text, None, hue);
        }
    }

    /// The handle the selected stop sits on, for its ring: the first stop's at
    /// offset 0, the last's at offset 1.
    fn stop_on_handle(&self, brush: &Brush, selected: usize) -> Option<Grab> {
        let stops = paint::stops_of(brush);
        let t = stops.get(selected)?.0;
        let kind = paint::kind_of(brush);
        match (t <= END_EPS, t >= 1.0 - END_EPS, kind) {
            (true, _, PaintKind::Linear) => Some(Grab::Start),
            (true, _, PaintKind::Radial) => Some(Grab::Centre),
            (true, _, PaintKind::Sweep) | (_, true, PaintKind::Sweep) => Some(Grab::Turn),
            (_, true, PaintKind::Linear) => Some(Grab::End),
            (_, true, PaintKind::Radial) => Some(Grab::AxisA),
            _ => None,
        }
    }
}

/// The box's corners, edge midpoints and centre: where an end or a centre snaps.
fn box_points(b: Rect) -> [Point; 9] {
    let (x0, x1, y0, y1) = (b.x0, b.x1, b.y0, b.y1);
    let (xm, ym) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
    [
        Point::new(x0, y0),
        Point::new(xm, y0),
        Point::new(x1, y0),
        Point::new(x0, ym),
        Point::new(xm, ym),
        Point::new(x1, ym),
        Point::new(x0, y1),
        Point::new(xm, y1),
        Point::new(x1, y1),
    ]
}

/// What a drag's badge says: a linear's angle and length, a radial's radii and
/// turn, a sweep's start, a stop's offset. `None` for a moved centre.
fn gradient_readout(brush: &Brush, grab: Grab) -> Option<String> {
    match grab {
        Grab::Start | Grab::End => {
            let (a, b) = paint::linear_ends(brush)?;
            let deg = paint::linear_angle(brush)?;
            Some(format!("{:.0}° · {:.0}", deg, (b - a).hypot()))
        }
        Grab::AxisA | Grab::AxisB => {
            let (_, a, b) = paint::radial_axes(brush)?;
            let turn = a.y.atan2(a.x).to_degrees();
            let turn = (turn.abs() > 0.05).then(|| format!(" · {:.0}°", -turn));
            Some(format!(
                "{:.0} × {:.0}{}",
                a.hypot(),
                b.hypot(),
                turn.unwrap_or_default()
            ))
        }
        Grab::Turn => paint::sweep_geom(brush).map(|(_, deg)| format!("{deg:.0}°")),
        Grab::Stop(_) | Grab::Centre => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eighths_hold_the_angle_and_keep_the_projected_length() {
        let d = eighths(Vec2::new(10.0, 1.0));
        assert!((d - Vec2::new(10.0, 0.0)).hypot() < 1e-9, "{d:?}");
        let d = eighths(Vec2::new(10.0, 9.0));
        assert!((d.x - d.y).abs() < 1e-9, "a diagonal: {d:?}");
    }

    #[test]
    fn a_point_off_the_segment_measures_to_its_nearest_end() {
        let (a, b) = (egui::pos2(0.0, 0.0), egui::pos2(10.0, 0.0));
        assert_eq!(segment_distance(egui::pos2(5.0, 3.0), a, b), 3.0);
        assert_eq!(segment_distance(egui::pos2(14.0, 3.0), a, b), 5.0);
    }
}

/// The handles driven through the whole canvas with real pointer events — press,
/// drag, release through `canvas_ui`. Plain backticks: `cargo doc` cannot see
/// this module.
#[cfg(test)]
mod canvas_tests {
    use super::*;
    use crate::panels::Picker;
    use ondin_core::kurbo::Size;
    use ondin_core::{Document, IdSource, NodeKind, Operation, Transaction};

    const RECT: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::Pos2::new(800.0, 600.0),
    };

    /// A selected 200 × 100 rect at the origin filled with a black-to-white linear
    /// from (0, 50) to (200, 50), the picker open on that fill, the camera on the
    /// rect's centre at 100%.
    fn app_with_a_ramp(ctx: &egui::Context) -> (OndinApp, NodeId) {
        crate::theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x6AD);
        let root = ids.mint();
        let rect = ids.mint();
        let brush = paint::with_linear_ends(
            &paint::default_brush(
                PaintKind::Linear,
                vec![(0.0, Color::BLACK), (1.0, Color::WHITE)],
                Size::new(200.0, 100.0),
            ),
            Point::new(0.0, 50.0),
            Point::new(200.0, 50.0),
        );
        let mut doc = Document::new(root);
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: rect,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(200.0, 100.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
            Operation::SetFills {
                id: rect,
                fills: ondin_core::keyed_by_position([ondin_core::Fill {
                    brush: brush.into(),
                    visible: true,
                }]),
            },
        ]))
        .expect("the rect");
        app.session.adopt_document(doc, None);
        app.session.camera.center = Point::new(100.0, 50.0);
        app.session.camera.zoom = 1.0;
        app.session.selection.set_one(rect);
        app.picker = Some(Picker::new(rect, PaintSlot::Fill(0), egui::pos2(0.0, 0.0)));
        for _ in 0..2 {
            frame(ctx, &mut app, Vec::new(), Default::default());
        }
        (app, rect)
    }

    fn frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) -> egui::FullOutput {
        let time = ctx.input(|i| i.time) + 0.1;
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(RECT),
                time: Some(time),
                modifiers,
                events,
                ..Default::default()
            },
            |ui| app.canvas_ui(ui),
        )
    }

    fn button(
        at: egui::Pos2,
        button: egui::PointerButton,
        pressed: bool,
        modifiers: egui::Modifiers,
    ) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button,
            pressed,
            modifiers,
        }
    }

    fn at(app: &OndinApp, w: Point) -> egui::Pos2 {
        app.to_screen(w, RECT, 1.0)
    }

    /// Press at world `from`, drag to `to` in eight steps holding `modifiers`,
    /// release.
    fn drag(
        ctx: &egui::Context,
        app: &mut OndinApp,
        from: Point,
        to: Point,
        modifiers: egui::Modifiers,
    ) {
        let start = at(app, from);
        let primary = egui::PointerButton::Primary;
        frame(ctx, app, vec![egui::Event::PointerMoved(start)], modifiers);
        frame(
            ctx,
            app,
            vec![button(start, primary, true, modifiers)],
            modifiers,
        );
        for step in 1..=8 {
            let p = at(app, from + (to - from) * (step as f64 / 8.0));
            frame(ctx, app, vec![egui::Event::PointerMoved(p)], modifiers);
        }
        let end = at(app, to);
        frame(
            ctx,
            app,
            vec![button(end, primary, false, modifiers)],
            modifiers,
        );
        frame(ctx, app, Vec::new(), Default::default());
    }

    /// Move onto world `w`, wait out the double-click window, press and release
    /// `which` there.
    fn click(ctx: &egui::Context, app: &mut OndinApp, w: Point, which: egui::PointerButton) {
        let p = at(app, w);
        frame(
            ctx,
            app,
            vec![egui::Event::PointerMoved(p)],
            Default::default(),
        );
        for _ in 0..40 {
            frame(ctx, app, Vec::new(), Default::default());
        }
        frame(
            ctx,
            app,
            vec![button(p, which, true, Default::default())],
            Default::default(),
        );
        frame(
            ctx,
            app,
            vec![button(p, which, false, Default::default())],
            Default::default(),
        );
        frame(ctx, app, Vec::new(), Default::default());
    }

    fn fill(app: &OndinApp, id: NodeId) -> Brush {
        app.session.doc.get(id).unwrap().paint().fills[0]
            .brush
            .literal()
            .cloned()
            .expect("a literal fill")
    }

    fn near(a: Point, b: Point) -> bool {
        (a - b).hypot() < 1e-6
    }

    /// Dragging the far end moves only that end, as one undo step.
    ///
    /// flip: removing `begin_gradient_drag`'s rung from `begin_select_drag` →
    /// fails at the ends assertion (the press moves the rect instead).
    #[test]
    fn dragging_a_linear_end_moves_only_that_end() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_ramp(&ctx);
        assert!(app.edited_gradient().is_some(), "the handles are up");
        let depth = app.session.history.undo_depth();
        drag(
            &ctx,
            &mut app,
            Point::new(200.0, 50.0),
            Point::new(150.0, 80.0),
            Default::default(),
        );
        let (a, b) = paint::linear_ends(&fill(&app, id)).expect("linear");
        assert!(
            near(a, Point::new(0.0, 50.0)) && near(b, Point::new(150.0, 80.0)),
            "{a:?} {b:?}"
        );
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
        let node = app.session.doc.get(id).unwrap();
        assert_eq!(node.transform(), Affine::IDENTITY, "the rect did not move");
    }

    /// An end snaps to the box's corner within 6 points; with Shift it holds to an
    /// eighth of a turn about the other end instead.
    ///
    /// flip: `snapped` returning `p` unchanged → fails at the snap assertion.
    #[test]
    fn an_end_snaps_to_the_box_and_shift_holds_eighths() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_ramp(&ctx);
        drag(
            &ctx,
            &mut app,
            Point::new(200.0, 50.0),
            Point::new(197.0, 3.0),
            Default::default(),
        );
        let (_, b) = paint::linear_ends(&fill(&app, id)).expect("linear");
        assert!(
            near(b, Point::new(200.0, 0.0)),
            "snapped to the corner: {b:?}"
        );
        drag(
            &ctx,
            &mut app,
            Point::new(200.0, 0.0),
            Point::new(150.0, 58.0),
            egui::Modifiers::SHIFT,
        );
        let (a, b) = paint::linear_ends(&fill(&app, id)).expect("linear");
        assert!(
            (b.y - a.y).abs() < 1e-6,
            "level with the start: {a:?} {b:?}"
        );
    }

    /// A click on the bare line adds a stop there, sampled, and selects it; a
    /// right-click on its dot removes it and opens no menu.
    ///
    /// flip: `gradient_secondary` returning false → fails at the menu assertion.
    #[test]
    fn a_click_on_the_line_adds_a_stop_and_a_right_click_removes_it() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_ramp(&ctx);
        click(
            &ctx,
            &mut app,
            Point::new(100.0, 50.0),
            egui::PointerButton::Primary,
        );
        let stops = paint::stops_of(&fill(&app, id));
        assert_eq!(stops.len(), 3, "{stops:?}");
        assert!((stops[1].0 - 0.5).abs() < 1e-6);
        assert_eq!(
            stops[1].1,
            paint::sample(&[stops[0], stops[2]], 0.5),
            "sampled"
        );
        assert_eq!(app.picker.as_ref().unwrap().stop, 1, "and selected");

        click(
            &ctx,
            &mut app,
            Point::new(100.0, 50.0),
            egui::PointerButton::Secondary,
        );
        assert_eq!(paint::stops_of(&fill(&app, id)).len(), 2, "removed");
        assert!(
            app.context_menu.is_none(),
            "the dot kept the button from the menu"
        );
    }

    /// The box's corner handle does not arm while the gradient's are up: a drag
    /// from the corner leaves the size alone.
    ///
    /// flip: dropping the `edited_gradient` gate from `selection_handles` → fails
    /// at the size assertion.
    #[test]
    fn the_box_handles_step_aside() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_ramp(&ctx);
        drag(
            &ctx,
            &mut app,
            Point::new(200.0, 100.0),
            Point::new(260.0, 140.0),
            Default::default(),
        );
        let node = app.session.doc.get(id).unwrap();
        assert_eq!(node.kind().box_size(), Some(Size::new(200.0, 100.0)));
    }

    /// A radial's second handle sets only the second radius.
    #[test]
    fn a_radial_s_second_handle_sets_only_its_radius() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_ramp(&ctx);
        let radial = paint::convert(&fill(&app, id), PaintKind::Radial, Size::new(200.0, 100.0));
        app.write_slot(id, PaintSlot::Fill(0), radial);
        frame(&ctx, &mut app, Vec::new(), Default::default());
        // Centre (100, 50), radius 50: the second axis ends at (100, 100).
        drag(
            &ctx,
            &mut app,
            Point::new(100.0, 100.0),
            Point::new(100.0, 80.0),
            Default::default(),
        );
        let (c, a, b) = paint::radial_axes(&fill(&app, id)).expect("radial");
        assert!(near(c, Point::new(100.0, 50.0)), "{c:?}");
        assert!((a - Vec2::new(50.0, 0.0)).hypot() < 1e-6, "{a:?}");
        assert!((b - Vec2::new(0.0, 30.0)).hypot() < 1e-6, "{b:?}");
    }

    /// With the pivot shown, a press on a radial's centre, where the hidden pivot
    /// marker also sits, moves the gradient and leaves the pivot alone.
    ///
    /// flip: dropping the `edited_gradient` gate from `on_pivot` → fails at the
    /// centre assertion.
    #[test]
    fn a_hidden_pivot_does_not_take_a_centre_s_press() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_ramp(&ctx);
        app.show_pivot = true;
        let radial = paint::convert(&fill(&app, id), PaintKind::Radial, Size::new(200.0, 100.0));
        app.write_slot(id, PaintSlot::Fill(0), radial);
        frame(&ctx, &mut app, Vec::new(), Default::default());
        drag(
            &ctx,
            &mut app,
            Point::new(100.0, 50.0),
            Point::new(60.0, 30.0),
            Default::default(),
        );
        let (c, _, _) = paint::radial_axes(&fill(&app, id)).expect("radial");
        assert!(near(c, Point::new(60.0, 30.0)), "{c:?}");
        assert_eq!(
            app.session.doc.get(id).unwrap().pivot(),
            None,
            "the pivot stayed"
        );
    }

    /// Dragging a sweep's start round its ring turns where offset 0 begins.
    #[test]
    fn dragging_a_sweep_s_start_turns_it() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_ramp(&ctx);
        let sweep = paint::convert(&fill(&app, id), PaintKind::Sweep, Size::new(200.0, 100.0));
        app.write_slot(id, PaintSlot::Fill(0), sweep);
        frame(&ctx, &mut app, Vec::new(), Default::default());
        // Centre (100, 50), start at three o'clock on a 50-point ring.
        drag(
            &ctx,
            &mut app,
            Point::new(150.0, 50.0),
            Point::new(100.0, 100.0),
            Default::default(),
        );
        let (_, deg) = paint::sweep_geom(&fill(&app, id)).expect("sweep");
        assert!((deg - 90.0).abs() < 1e-3, "straight down: {deg}");
    }

    /// **A handle dragged on a layer drawn wider than its paint box stores the end
    /// back on that box**: a rect bound to a width reading 200 in Light and 400 in
    /// Dark, in a Dark frame, its ramp stored across 200 and drawn across 400. The
    /// end dragged to 300 drawn is 150 stored.
    ///
    /// flip: `stored_on_box` returning `drawn` → fails at the stored end (300).
    #[test]
    fn a_handle_on_a_stretched_layer_stores_the_end_on_its_paint_box() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_ramp(&ctx);
        let root = app.session.doc.first_page();
        let mut ids = IdSource::new(0x5A7);
        let board = ids.mint();
        let (light, dark) = (ids.mint_mode(), ids.mint_mode());
        let w = ids.mint_var();
        let mode = |id, name: &str| ondin_core::Mode {
            id,
            name: name.into(),
        };
        let mut doc = app.session.doc.clone();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: board,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(600.0, 100.0),
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
            Operation::Reparent {
                id,
                new_parent: board,
                index: 0,
            },
            Operation::AddMode {
                mode: mode(light, "Light"),
                index: 0,
                cells: vec![],
            },
            Operation::AddMode {
                mode: mode(dark, "Dark"),
                index: 1,
                cells: vec![],
            },
            Operation::AddVariable {
                var: ondin_core::Variable {
                    id: w,
                    name: "w".into(),
                    kind: ondin_core::VarKind::Number,
                    cells: [
                        (light, ondin_core::Cell::Number(200.0)),
                        (dark, ondin_core::Cell::Number(400.0)),
                    ]
                    .into(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::SetMode {
                id: board,
                mode: Some(dark),
            },
        ]))
        .expect("a Dark frame and a width variable");
        doc.apply(&Transaction(ondin_core::bind_size_ops(
            &doc,
            id,
            [Some(w), None],
        )))
        .expect("the bound width");
        app.session.adopt_document(doc, None);
        app.session.camera.center = Point::new(200.0, 50.0);
        app.session.camera.zoom = 1.0;
        app.session.selection.set_one(id);
        app.picker = Some(Picker::new(id, PaintSlot::Fill(0), egui::pos2(0.0, 0.0)));
        for _ in 0..2 {
            frame(&ctx, &mut app, Vec::new(), Default::default());
        }
        assert_eq!(
            app.session.resolved.world_bounds(id).map(|r| r.width()),
            Some(400.0),
            "fixture: drawn at Dark's 400"
        );
        drag(
            &ctx,
            &mut app,
            Point::new(400.0, 50.0),
            Point::new(300.0, 50.0),
            Default::default(),
        );
        let (a, b) = paint::linear_ends(&fill(&app, id)).expect("linear");
        assert!(
            near(a, Point::new(0.0, 50.0)) && near(b, Point::new(150.0, 50.0)),
            "the stored end: {a:?} {b:?}"
        );
    }

    /// Resizing the layer carries its gradient with the box.
    ///
    /// flip: dropping `keep_gradients` from `commit_inner` → fails at the ends.
    #[test]
    fn a_resize_carries_the_gradient() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_ramp(&ctx);
        app.session.commit(Transaction(vec![Operation::SetGeometry {
            id,
            geometry: ondin_core::GeometryPatch::Size(Size::new(400.0, 50.0)),
        }]));
        let (a, b) = paint::linear_ends(&fill(&app, id)).expect("linear");
        assert!(
            near(a, Point::new(0.0, 25.0)) && near(b, Point::new(400.0, 25.0)),
            "{a:?} {b:?}"
        );
    }
}
