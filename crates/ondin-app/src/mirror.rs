//! Mirror while drawing (`docs/design/app-pen-and-nodes.md`, *Mirror while
//! drawing*): the pen and the pencil draw the other half of a new path as they go,
//! reflected across an axis through the frame being drawn in.
//!
//! The halves join where they meet the axis. A run that starts and ends on it
//! closes as one symmetric path; one end on it makes one open path joined there;
//! neither leaves two subpaths in the one layer. *Both axes* is the vertical rule
//! then the horizontal one over what that made, so a quarter drawn from axis to
//! axis closes into a whole.
//!
//! A path drawn mirrored is saved mirrored (`ondin_core::symmetry`), and point
//! editing keeps it so ([`symmetric_edit`]), its axis points locked to the axes;
//! a new run drawn inside it mirrors across its own axes ([`OndinApp::mirror_frame`]).

use crate::app::OndinApp;
use crate::preview::{PenAnchor, PenSubpath, PointRef};
use crate::theme::color;
use crate::ui;
use eframe::egui;
use ondin_core::kurbo::{Affine, Point, Vec2};
use ondin_core::symmetry::PathMirror;
use ondin_core::{NodeId, NodeKind, Operation, Transaction};

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

/// The axes a new run is mirrored across, in the space they are axis-aligned
/// in: the world for the drawing switch, a mirrored path's own space for its axes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame {
    pub(crate) axes: Axes,
    /// Where the axes cross, in this frame's space.
    pub(crate) at: Point,
    pub(crate) to_world: Affine,
    /// A mirrored path's own axes, which are where its points put them and so
    /// have no grab pill.
    pub(crate) own: bool,
}

/// The axes' grab pills as last painted, which a press on leaves to them: the pen,
/// the pencil and the *On axis* chip stay out of a pill and its drag.
#[derive(Clone, Debug, Default)]
pub(crate) struct Grips {
    /// Each pill's hit rect in screen pt, and whether it moves the vertical axis.
    pub(crate) pills: Vec<(bool, egui::Rect)>,
    /// The axis a pill drag is moving, while one is: `true` for the vertical.
    pub(crate) held: Option<bool>,
    /// Where the axes were when the held pill was pressed, which a right-click
    /// puts back (`OndinApp::cancel_gesture`). Kept across frames while held.
    pub(crate) from: Option<Option<Point>>,
}

/// The path axes a run drawn across `axes` lands mirrored across.
pub(crate) fn path_mirror_of(axes: Axes) -> PathMirror {
    PathMirror {
        vertical: axes.vertical(),
        horizontal: axes.horizontal(),
    }
}

/// Mark the path `tx` creates as mirrored across `mirror`; the model keeps only
/// the axes that hold.
pub(crate) fn mark_mirrored(tx: &mut Transaction, mirror: PathMirror) {
    for op in &mut tx.0 {
        if let Operation::CreateNode {
            kind: NodeKind::Path { mirror: m, .. },
            ..
        } = op
        {
            *m = mirror;
        }
    }
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

// --- a mirrored path under point editing -----------------------------------

/// A handle offset reflected across a vertical or horizontal axis.
fn flip(v: Vec2, vertical: bool) -> Vec2 {
    match vertical {
        true => Vec2::new(-v.x, v.y),
        false => Vec2::new(v.x, -v.y),
    }
}

/// `a`'s image across the axis `x = c` (or `y = c`): `swap` when the run passes
/// its image the other way round, so the image's arriving handle is the image of
/// `a`'s leaving one.
fn image_of(a: &PenAnchor, vertical: bool, c: f64, swap: bool) -> PenAnchor {
    let (out, back) = match swap {
        true => (flip(a.back, vertical), flip(a.out, vertical)),
        false => (flip(a.out, vertical), flip(a.back, vertical)),
    };
    PenAnchor {
        at: ondin_core::symmetry::reflect(a.at, vertical, c),
        out,
        back,
        radius: a.radius,
    }
}

/// Each anchor's image across one axis among `subs`, by flat index, with
/// whether the run passes it the other way round; `None` where no anchor sits
/// at the image.
///
/// The way round comes from the neighbours, not the handles: a corner has none
/// to tell by, and pulling one out of it must still land on the right side.
fn partners(subs: &[PenSubpath], vertical: bool, c: f64, tol: f64) -> Vec<Option<(usize, bool)>> {
    let flat: Vec<&PenAnchor> = subs.iter().flat_map(|s| &s.anchors).collect();
    // Each anchor's neighbours along its run, by flat index.
    let mut prev = Vec::with_capacity(flat.len());
    let mut next = Vec::with_capacity(flat.len());
    let mut start = 0;
    for s in subs {
        let n = s.anchors.len();
        for i in 0..n {
            let wrap = |j: usize| start + j % n;
            prev.push(match (i, s.closed) {
                (0, false) => None,
                (0, true) => Some(wrap(n - 1)),
                _ => Some(start + i - 1),
            });
            next.push(match (i + 1 == n, s.closed) {
                (true, false) => None,
                _ => Some(wrap(i + 1)),
            });
        }
        start += n;
    }
    let at: Vec<Option<usize>> = flat
        .iter()
        .map(|a| {
            let want = ondin_core::symmetry::reflect(a.at, vertical, c);
            flat.iter().position(|b| (b.at - want).hypot() <= tol)
        })
        .collect();
    (0..flat.len())
        .map(|a| {
            let p = at[a]?;
            let image = |j: Option<usize>| j.and_then(|j| at[j]);
            let swap = match (next[a], prev[a]) {
                (Some(_), _) => image(next[a]).is_some() && image(next[a]) == prev[p],
                (None, Some(_)) => image(prev[a]).is_some() && image(prev[a]) == next[p],
                (None, None) => false,
            };
            Some((p, swap))
        })
        .collect()
}

/// Whether point editing moved, reshaped or re-rounded anchor `a` into `b`.
fn changed(a: &PenAnchor, b: &PenAnchor, eps: f64) -> bool {
    (a.at - b.at).hypot() > eps
        || (a.out - b.out).hypot() > eps
        || (a.back - b.back).hypot() > eps
        || (a.radius - b.radius).abs() > eps
}

/// `edited`, made symmetric again across `mirror`'s axes through `at` after a
/// point edit that started from the symmetric `base`, both in the path's own
/// space. Where an anchor changed, its image takes the reflection of it; where
/// both did, the first in order wins. An anchor on an axis stays on it, its two
/// handles each other's image. An edit that changed how many anchors there are
/// is left alone: the model drops the axis if it no longer holds.
pub(crate) fn symmetric_edit(
    base: &[PenSubpath],
    edited: &mut [PenSubpath],
    mirror: ondin_core::symmetry::PathMirror,
    at: Point,
) {
    let shape = |s: &[PenSubpath]| s.iter().map(|s| s.anchors.len()).collect::<Vec<_>>();
    if shape(base) != shape(edited) {
        return;
    }
    let size = base
        .iter()
        .flat_map(|s| &s.anchors)
        .map(|a| (a.at - at).hypot())
        .fold(0.0, f64::max);
    let tol = 1e-6 * (1.0 + size);
    let flat_base: Vec<PenAnchor> = base.iter().flat_map(|s| s.anchors.clone()).collect();
    for (vertical, c, on) in [
        (true, at.x, mirror.vertical),
        (false, at.y, mirror.horizontal),
    ] {
        if !on {
            continue;
        }
        let pairs = partners(base, vertical, c, tol);
        let mut flat: Vec<PenAnchor> = edited.iter().flat_map(|s| s.anchors.clone()).collect();
        let mut done = vec![false; flat.len()];
        for a in 0..flat.len() {
            let Some((p, swap)) = pairs[a] else { continue };
            if done[a] {
                continue;
            }
            done[a] = true;
            done[p] = true;
            let (ca, cp) = (
                changed(&flat_base[a], &flat[a], tol),
                changed(&flat_base[p], &flat[p], tol),
            );
            if a == p {
                if ca {
                    flat[a] = on_axis(&flat_base[a], &flat[a], vertical, c, swap, tol);
                }
            } else if ca {
                flat[p] = image_of(&flat[a], vertical, c, swap);
            } else if cp {
                flat[a] = image_of(&flat[p], vertical, c, swap);
            }
        }
        let mut it = flat.into_iter();
        for sub in edited.iter_mut() {
            for a in &mut sub.anchors {
                *a = it.next().expect("one anchor per anchor");
            }
        }
    }
}

/// An anchor on the axis after an edit: pulled back onto it, so it slides along
/// it, with the handle the edit moved deciding the other.
fn on_axis(
    was: &PenAnchor,
    now: &PenAnchor,
    vertical: bool,
    c: f64,
    swap: bool,
    tol: f64,
) -> PenAnchor {
    let mut a = *now;
    match vertical {
        true => a.at.x = c,
        false => a.at.y = c,
    }
    // A run crossing the axis meets its image here: each handle is the other's.
    // One running along the axis keeps both handles on it.
    match swap {
        true if (now.back - was.back).hypot() > tol && (now.out - was.out).hypot() <= tol => {
            a.out = flip(a.back, vertical);
        }
        true => a.back = flip(a.out, vertical),
        false => {
            let along = |v: Vec2| match vertical {
                true => Vec2::new(0.0, v.y),
                false => Vec2::new(v.x, 0.0),
            };
            a.out = along(a.out);
            a.back = along(a.back);
        }
    }
    a
}

/// How near an axis, in screen pt, an end is taken as on it.
pub(crate) const ON_AXIS_PT: f64 = 8.0;

/// How far the *On axis* chip sits right of and above its ring, in screen pt.
const AXIS_CHIP_GAP: f32 = 8.0;

/// How far a mirrored path's own axis runs past its box, in screen pt.
const OWN_AXIS_OVERHANG: f64 = 16.0;

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
            .unwrap_or_else(|| self.view_middle());
        Some((m.axes, at))
    }

    /// The middle of the canvas past the rulers, in world space; the camera's
    /// centre before the canvas is first laid out.
    fn view_middle(&self) -> Point {
        let rect = self.canvas_rect;
        if !rect.is_finite() || !rect.is_positive() {
            return self.session.camera.center;
        }
        self.to_world(self.canvas_inner(rect).center(), rect, self.canvas_ppp)
    }

    /// The frame the switch's axes are drawn through, in world space: the one the
    /// run began in, else the one under `near`, when the axes cross inside it.
    /// `None` draws them down the whole view: on open canvas, or once they were
    /// dragged out of that frame.
    fn axis_frame(&self, near: Option<Point>, at: Point) -> Option<ondin_core::kurbo::Rect> {
        let (frame, _) = self.draw_target(self.drawing_from().or(near)?);
        if frame == self.session.page {
            return None;
        }
        self.session
            .resolved
            .world_bounds(frame)
            .filter(|b| b.contains(at))
    }

    /// The axis whose pill claims the pointer at `screen`, or whose pill is being
    /// dragged: `true` for the vertical. `screen` is `None` off the canvas.
    pub(crate) fn mirror_grip(&self, screen: Option<egui::Pos2>) -> Option<bool> {
        self.mirror_grips.held.or_else(|| {
            let p = screen?;
            self.mirror_grips
                .pills
                .iter()
                .find(|(_, r)| r.contains(p))
                .map(|(vertical, _)| *vertical)
        })
    }

    /// Where the run being drawn began, pen or pencil.
    fn drawing_from(&self) -> Option<Point> {
        match (&self.pen, &self.pencil) {
            (Some(p), _) if p.editing.as_ref().is_none_or(|e| e.minted) => {
                p.anchors.first().map(|a| a.at)
            }
            (_, Some(s)) => s.origin(),
            _ => None,
        }
    }

    /// A mirrored path's own axes, where they cross in its space, and its world
    /// transform; `None` for a path that is not mirrored.
    pub(crate) fn path_mirror(&self, id: NodeId) -> Option<(PathMirror, Point, Affine)> {
        let NodeKind::Path { path, mirror, .. } = self.session.doc.get(id)?.kind() else {
            return None;
        };
        if mirror.is_none() {
            return None;
        }
        let world = self.session.resolved.world_transform(id)?;
        Some((*mirror, ondin_core::symmetry::centre(path), world))
    }

    /// The mirror a new run follows, if any: inside a mirrored path the node tool
    /// edits, that path's own axes; otherwise the drawing switch's. Extending an
    /// end follows none.
    pub(crate) fn mirror_frame(&self, near: Option<Point>) -> Option<Frame> {
        let editing = match &self.pen {
            Some(p) => match &p.editing {
                Some(e) if !e.minted => return None,
                Some(e) => Some(e.id),
                None => None,
            },
            None => self.edited_path(),
        };
        if let Some((m, at, to_world)) = editing.and_then(|id| self.path_mirror(id)) {
            let axes = match (m.vertical, m.horizontal) {
                (true, true) => Axes::Both,
                (true, false) => Axes::Vertical,
                _ => Axes::Horizontal,
            };
            return Some(Frame {
                axes,
                at,
                to_world,
                own: true,
            });
        }
        // The node tool shows the switch's axes only once its pen is armed.
        if self.pen.is_none() && self.tool == crate::tools::Tool::Node && !self.pen_bias {
            return None;
        }
        self.mirror_axes(near).map(|(axes, at)| Frame {
            axes,
            at,
            to_world: Affine::IDENTITY,
            own: false,
        })
    }

    /// How near an axis an end is taken as on it, in `frame`'s space.
    fn axis_tol(&self, frame: &Frame) -> f64 {
        let scale = frame.to_world.determinant().abs().sqrt().max(1e-9);
        ON_AXIS_PT / self.points_per_world() / scale
    }

    /// The new run `drawn` as the mirror makes it, in world space; just `drawn`
    /// with the mirror off.
    pub(crate) fn mirror_run(&self, drawn: PenSubpath) -> Vec<PenSubpath> {
        let near = drawn.anchors.first().map(|a| a.at);
        let Some(frame) = self.mirror_frame(near) else {
            return vec![drawn];
        };
        let tol = self.axis_tol(&frame);
        let mut local = [drawn];
        crate::tools::map_subpaths(&mut local, frame.to_world.inverse());
        let [local] = local;
        let mut subs = mirrored(local, frame.axes, frame.at, tol);
        crate::tools::map_subpaths(&mut subs, frame.to_world);
        subs
    }

    /// `p` pulled onto an axis it lies within [`ON_AXIS_PT`] of, and whether it
    /// was: where a pen anchor snaps.
    pub(crate) fn snap_to_axis(&self, p: Point) -> (Point, bool) {
        let Some(frame) = self.mirror_frame(Some(p)) else {
            return (p, false);
        };
        let tol = self.axis_tol(&frame);
        let (axes, at) = (frame.axes, frame.at);
        let local = frame.to_world.inverse() * p;
        let mut q = local;
        let mut on = false;
        if axes.vertical() && (local.x - at.x).abs() <= tol {
            q.x = at.x;
            on = true;
        }
        if axes.horizontal() && (local.y - at.y).abs() <= tol {
            q.y = at.y;
            on = true;
        }
        (frame.to_world * q, on)
    }

    /// `subpaths` (world), a point edit of `id` that started from `base`, made
    /// symmetric again across the path's own axes when it is mirrored
    /// ([`symmetric_edit`]), then written back as `tools::rewrite_path` writes it.
    pub(crate) fn rewrite_points(
        &self,
        id: NodeId,
        world: Affine,
        base: &[PenSubpath],
        mut subpaths: Vec<PenSubpath>,
    ) -> Transaction {
        if let Some((mirror, _, _)) = self.path_mirror(id) {
            let to_local = world.inverse();
            let mut base = base.to_vec();
            crate::tools::map_subpaths(&mut base, to_local);
            crate::tools::map_subpaths(&mut subpaths, to_local);
            let at = ondin_core::symmetry::centre(&crate::tools::pen_path(&base));
            symmetric_edit(&base, &mut subpaths, mirror, at);
            crate::tools::map_subpaths(&mut subpaths, world);
        }
        crate::tools::rewrite_path(id, world, &subpaths, self.stored_path(id))
    }

    /// Where adding a point at `t` along the segment leaving `at` on `id` adds one,
    /// with that point's images on a mirrored path, in the order to insert them:
    /// later anchors first, and on one segment the further point first, the
    /// nearer then measured along what is left. A segment that crosses an axis is
    /// its own image; a point near its middle goes in once, at the middle.
    pub(crate) fn mirrored_inserts(
        &self,
        id: NodeId,
        at: PointRef,
        t: f64,
    ) -> Vec<(PointRef, f64)> {
        let Some((mirror, c, _)) = self.path_mirror(id) else {
            return vec![(at, t)];
        };
        let Some(NodeKind::Path {
            path, corner_radii, ..
        }) = self.session.doc.get(id).map(|n| n.kind())
        else {
            return vec![(at, t)];
        };
        let subs = crate::tools::pen_anchors(path, corner_radii);
        let refs: Vec<PointRef> = subs
            .iter()
            .enumerate()
            .flat_map(|(s, sub)| (0..sub.anchors.len()).map(move |i| (s, i)))
            .collect();
        let flat = |r: PointRef| refs.iter().position(|x| *x == r);
        let b = path.control_box();
        let tol = 1e-6 * (1.0 + b.width().max(b.height()));
        // Within this of the middle, a point on a segment that is its own image is
        // the one point there.
        const MIDDLE: f64 = 0.02;
        let mut segs: Vec<(PointRef, f64)> = vec![(at, t)];
        for (vertical, cv, on) in [
            (true, c.x, mirror.vertical),
            (false, c.y, mirror.horizontal),
        ] {
            if !on {
                continue;
            }
            let pairs = partners(&subs, vertical, cv, tol);
            for (seg, t) in segs.clone() {
                let Some((start, end)) = crate::tools::segment_ends(&subs, seg) else {
                    continue;
                };
                let image = |r: PointRef| flat(r).and_then(|k| pairs[k]).map(|(j, _)| refs[j]);
                let (Some(p0), Some(p1)) = (image(start), image(end)) else {
                    continue;
                };
                let leaves = |from: PointRef, to: PointRef| {
                    crate::tools::segment_ends(&subs, from).is_some_and(|(_, e)| e == to)
                };
                let found = if leaves(p0, p1) {
                    Some((p0, t))
                } else if leaves(p1, p0) {
                    Some((p1, 1.0 - t))
                } else {
                    None
                };
                let Some((s2, t2)) = found else { continue };
                if s2 == seg && (t - 0.5).abs() <= MIDDLE {
                    for x in &mut segs {
                        if x.0 == seg {
                            x.1 = 0.5;
                        }
                    }
                    continue;
                }
                if !segs.iter().any(|(s, u)| *s == s2 && (u - t2).abs() <= 1e-9) {
                    segs.push((s2, t2));
                }
            }
        }
        // Later anchors first, so an insert does not renumber the ones still to go.
        segs.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.total_cmp(&a.1)));
        segs.dedup_by(|a, b| a.0 == b.0 && (a.1 - b.1).abs() <= 1e-9);
        // On one segment each nearer point is measured along the left part the
        // previous insert left, which ends at that insert's parameter.
        let mut prev: Option<(PointRef, f64)> = None;
        segs.into_iter()
            .map(|(seg, t)| {
                let rel = match prev {
                    Some((s, u)) if s == seg && u > 0.0 => t / u,
                    _ => t,
                };
                prev = Some((seg, t));
                (seg, rel)
            })
            .collect()
    }

    /// The image of every point in `points` on `id` across the path's own axes,
    /// with the points themselves: what deleting them takes, so the path stays
    /// symmetric.
    pub(crate) fn with_mirrored_points(&self, id: NodeId, points: &[PointRef]) -> Vec<PointRef> {
        let mut out = points.to_vec();
        let Some((mirror, at, _)) = self.path_mirror(id) else {
            return out;
        };
        let NodeKind::Path {
            path, corner_radii, ..
        } = self
            .session
            .doc
            .get(id)
            .map(|n| n.kind())
            .expect("a mirrored path")
        else {
            return out;
        };
        let subs = crate::tools::pen_anchors(path, corner_radii);
        let refs: Vec<PointRef> = subs
            .iter()
            .enumerate()
            .flat_map(|(s, sub)| (0..sub.anchors.len()).map(move |i| (s, i)))
            .collect();
        let tol = 1e-6 * (1.0 + path.control_box().width().max(path.control_box().height()));
        for (vertical, c, on) in [
            (true, at.x, mirror.vertical),
            (false, at.y, mirror.horizontal),
        ] {
            if !on {
                continue;
            }
            let pairs = partners(&subs, vertical, c, tol);
            for p in out.clone() {
                let Some(k) = refs.iter().position(|r| *r == p) else {
                    continue;
                };
                if let Some((j, _)) = pairs[k]
                    && !out.contains(&refs[j])
                {
                    out.push(refs[j]);
                }
            }
        }
        out
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
            self.session.mirror.at = Some(self.view_middle());
        }
    }

    /// Whether the run in flight extends a path, which the mirror leaves alone; a
    /// new run inside the edited path does not.
    fn extending(&self) -> bool {
        self.pen
            .as_ref()
            .is_some_and(|p| p.editing.as_ref().is_some_and(|e| !e.minted))
            || self.pencil.as_ref().is_some_and(|s| s.extends())
    }

    /// Paint the axes and their grab pills while the pen or the pencil is armed and
    /// mirroring, and the mirrored half of the run being drawn at 45%. Dragging a
    /// pill moves its axis; a double-click puts the axes back. Under the node tool,
    /// a mirrored path's own axes, which have no pill.
    pub(crate) fn mirror_chrome(
        &mut self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
    ) {
        let from = self.mirror_grips.from.take();
        self.mirror_grips = Grips::default();
        if !matches!(
            self.tool,
            crate::tools::Tool::Pen | crate::tools::Tool::Pencil | crate::tools::Tool::Node
        ) || self.extending()
        {
            return;
        }
        let hover = ui
            .ctx()
            .pointer_hover_pos()
            .map(|p| self.to_world(p, rect, ppp));
        let Some(mirror) = self.mirror_frame(hover) else {
            return;
        };
        let ink = color::accent_400();
        let line = egui::Stroke::new(1.0, ink);
        if mirror.own {
            self.own_axes_chrome(painter, rect, ppp, &mirror, line);
            self.mirrored_run_chrome(ui, painter, rect, ppp, hover);
            return;
        }
        let (axes, at) = (mirror.axes, mirror.at);
        // Through the frame being drawn in; down the whole view past the rulers
        // otherwise, so the pill is always where the axis is seen.
        let span = self
            .axis_frame(hover, at)
            .map(|b| self.world_rect_to_screen(b, rect, ppp))
            .unwrap_or_else(|| self.canvas_inner(rect));
        let at_s = self.to_screen(at, rect, ppp);
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
            let hit = r.expand(3.0);
            self.mirror_grips.pills.push((vertical, hit));
            let resp = ui
                .interact(hit, id, egui::Sense::click_and_drag())
                .on_hover_cursor(match vertical {
                    true => egui::CursorIcon::ResizeHorizontal,
                    false => egui::CursorIcon::ResizeVertical,
                })
                .on_hover_text("Drag to move the axis · double-click to put it back");
            if resp.dragged() || resp.is_pointer_button_down_on() {
                self.mirror_grips.held = Some(vertical);
                self.mirror_grips.from = Some(from.unwrap_or(self.session.mirror.at));
            }
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
        self.mirrored_run_chrome(ui, painter, rect, ppp, hover);
    }

    /// A mirrored path's own axes while the node tool edits it: dashed through its
    /// box a little past each end, turned with the layer. They are where the
    /// points put them, so they have no grab pill.
    fn own_axes_chrome(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        mirror: &Frame,
        line: egui::Stroke,
    ) {
        let Some(id) = self
            .pen
            .as_ref()
            .and_then(|p| p.editing.as_ref().map(|e| e.id))
            .or_else(|| self.edited_path())
        else {
            return;
        };
        let Some(NodeKind::Path { path, .. }) = self.session.doc.get(id).map(|n| n.kind()) else {
            return;
        };
        let b = path.control_box();
        // Past the box by a fixed screen distance, whatever the zoom.
        let over = OWN_AXIS_OVERHANG / self.points_per_world() / {
            mirror.to_world.determinant().abs().sqrt().max(1e-9)
        };
        let ends =
            |a: Point, b: Point| [a, b].map(|p| self.to_screen(mirror.to_world * p, rect, ppp));
        if mirror.axes.vertical() {
            let x = mirror.at.x;
            let [a, z] = ends(Point::new(x, b.y0 - over), Point::new(x, b.y1 + over));
            painter.extend(egui::Shape::dashed_line(&[a, z], line, 6.0, 4.0));
        }
        if mirror.axes.horizontal() {
            let y = mirror.at.y;
            let [a, z] = ends(Point::new(b.x0 - over, y), Point::new(b.x1 + over, y));
            painter.extend(egui::Shape::dashed_line(&[a, z], line, 6.0, 4.0));
        }
    }

    /// The other half of the run being drawn, at 45%, and the *On axis* mark where
    /// the next point would land on an axis.
    fn mirrored_run_chrome(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        hover: Option<Point>,
    ) {
        // The other half of the pen's run, as it will land.
        if let Some(pen) = self
            .pen
            .as_ref()
            .filter(|p| p.editing.as_ref().is_none_or(|e| e.minted))
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
        // And the pencil's: its trail as the mirror makes it, joined where it will
        // join, in the trail's own line at 45%. The fit happens on release.
        if let Some(stroke) = self.pencil.as_ref().filter(|s| !s.extends())
            && stroke.samples().len() >= 2
        {
            let drawn = PenSubpath {
                anchors: stroke
                    .samples()
                    .iter()
                    .map(|p| crate::preview::PenAnchor::corner(*p))
                    .collect(),
                closed: false,
            };
            let ghost = egui::Stroke::new(1.0, color::SELECT.gamma_multiply(0.45));
            for sub in &self.mirror_run(drawn) {
                let mut pts: Vec<egui::Pos2> = sub
                    .anchors
                    .iter()
                    .map(|a| self.to_screen(a.at, rect, ppp))
                    .collect();
                if sub.closed
                    && let Some(first) = pts.first().copied()
                {
                    pts.push(first);
                }
                painter.add(egui::Shape::line(pts, ghost));
            }
        }
        // Where the next point would land on an axis: a ring there and *On axis*,
        // in the snaps' red. The node tool places points only with its pen, and
        // nothing places one on a pill, which moves the axis instead.
        let placing =
            (self.tool != crate::tools::Tool::Node || self.pen.is_some() || self.pen_bias)
                && self.mirror_grip(ui.ctx().pointer_hover_pos()).is_none();
        let shift = ui.input(|i| i.modifiers.shift);
        if placing && let Some(on) = hover.and_then(|w| self.lands_on_axis(w, shift)) {
            let at = self.to_screen(on, rect, ppp);
            painter.circle_stroke(
                at,
                crate::canvas::SNAP_RING_PX,
                egui::Stroke::new(crate::canvas::SNAP_RING_STROKE_PX, color::MEASURE),
            );
            ui::canvas_pill(
                painter,
                at + egui::vec2(AXIS_CHIP_GAP, -AXIS_CHIP_GAP),
                egui::Align2::LEFT_BOTTOM,
                "On axis",
                color::MEASURE,
            );
        }
    }

    /// Where a point placed at `world` lands on a mirror axis, if it does: the pen's
    /// next anchor (snapped as its press would be), or where the pencil's stroke
    /// would end, pulled onto the axis by the commit.
    pub(crate) fn lands_on_axis(&self, world: Point, shift: bool) -> Option<Point> {
        let at = match self.tool {
            crate::tools::Tool::Pen | crate::tools::Tool::Node => self.pen_point(world, shift).0,
            _ => world,
        };
        let (on, hit) = self.snap_to_axis(at);
        hit.then_some(on)
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

    const VERTICAL: PathMirror = PathMirror {
        vertical: true,
        horizontal: false,
    };

    /// A closed diamond about x = 0, clockwise from the top: top and bottom on the
    /// axis, right and left each other's image.
    fn diamond() -> Vec<PenSubpath> {
        let mut d = run(&[(0.0, -10.0), (10.0, 0.0), (0.0, 10.0), (-10.0, 0.0)]);
        d.closed = true;
        vec![d]
    }

    /// Whether the model would keep the vertical axis on `subs`.
    fn holds(subs: &[PenSubpath]) -> bool {
        let path = crate::tools::pen_path(subs);
        ondin_core::symmetry::kept(&path, &crate::tools::pen_radii(subs), VERTICAL).vertical
    }

    /// A dragged anchor takes its image with it, mirrored.
    /// flip: `symmetric_edit` returning early → fails at "the image followed".
    #[test]
    fn a_moved_anchor_moves_its_image() {
        let base = diamond();
        let mut edited = base.clone();
        edited[0].anchors[1].at = Point::new(14.0, 2.0);
        symmetric_edit(&base, &mut edited, VERTICAL, Point::ZERO);
        assert_eq!(
            edited[0].anchors[3].at,
            Point::new(-14.0, 2.0),
            "the image followed"
        );
        assert!(holds(&edited));
    }

    /// The image's own edit wins when only it moved.
    #[test]
    fn an_edit_on_the_image_side_carries_back() {
        let base = diamond();
        let mut edited = base.clone();
        edited[0].anchors[3].radius = 4.0;
        symmetric_edit(&base, &mut edited, VERTICAL, Point::ZERO);
        assert_eq!(edited[0].anchors[1].radius, 4.0);
        assert!(holds(&edited));
    }

    /// An anchor on the axis is locked to it: dragged off, it slides along it.
    /// flip: `on_axis` keeping `now.at` → fails at "still on the axis".
    #[test]
    fn an_axis_anchor_slides_along_the_axis() {
        let base = diamond();
        let mut edited = base.clone();
        edited[0].anchors[0].at = Point::new(3.0, -12.0);
        symmetric_edit(&base, &mut edited, VERTICAL, Point::ZERO);
        assert_eq!(
            edited[0].anchors[0].at,
            Point::new(0.0, -12.0),
            "still on the axis"
        );
        assert!(holds(&edited));
    }

    /// A handle pulled out of a corner lands on its image's matching side, which
    /// the neighbours decide since a corner has no handle to tell by; on the axis,
    /// the other handle becomes its image.
    /// flip: `partners` answering `swap` false always → fails at the handle assertion.
    #[test]
    fn a_handle_pulled_from_a_corner_mirrors_onto_the_right_side() {
        let base = diamond();
        let mut edited = base.clone();
        edited[0].anchors[1].out = Vec2::new(0.0, 5.0);
        edited[0].anchors[0].out = Vec2::new(6.0, 0.0);
        symmetric_edit(&base, &mut edited, VERTICAL, Point::ZERO);
        assert_eq!(edited[0].anchors[3].back, Vec2::new(0.0, 5.0));
        assert_eq!(edited[0].anchors[3].out, Vec2::ZERO);
        assert_eq!(edited[0].anchors[0].back, Vec2::new(-6.0, 0.0));
        assert!(holds(&edited));
    }

    /// Both axes: one moved anchor takes all three images with it.
    #[test]
    fn both_axes_carry_an_edit_to_all_four_quarters() {
        let mut sq = run(&[(10.0, 10.0), (-10.0, 10.0), (-10.0, -10.0), (10.0, -10.0)]);
        sq.closed = true;
        let base = vec![sq];
        let mut edited = base.clone();
        edited[0].anchors[0].at = Point::new(12.0, 15.0);
        let both = PathMirror {
            vertical: true,
            horizontal: true,
        };
        symmetric_edit(&base, &mut edited, both, Point::ZERO);
        let ats: Vec<Point> = edited[0].anchors.iter().map(|a| a.at).collect();
        assert_eq!(
            ats,
            [(12.0, 15.0), (-12.0, 15.0), (-12.0, -15.0), (12.0, -15.0)]
                .map(|(x, y)| Point::new(x, y))
        );
    }

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

    /// A headless app with the pen armed and the vertical axis at x = 100, the view
    /// at 100% centred on the origin.
    fn mirroring_pen(ctx: &egui::Context) -> OndinApp {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(1);
        app.session
            .adopt_document(ondin_core::Document::new(ids.mint()), None);
        app.canvas_px = (800, 600);
        app.session.camera.center = Point::ZERO;
        app.session.camera.zoom = 1.0;
        app.choose_tool(crate::tools::Tool::Pen);
        app.session.mirror = Mirror {
            axes: Axes::Vertical,
            at: Some(Point::new(100.0, 0.0)),
        };
        app
    }

    /// A point within `ON_AXIS_PT` of the axis lands on it; one further off, or
    /// with the mirror off, lands nowhere the chip names.
    #[test]
    fn the_chip_names_a_point_that_lands_on_the_axis() {
        let ctx = egui::Context::default();
        let mut app = mirroring_pen(&ctx);
        assert_eq!(
            app.lands_on_axis(Point::new(104.0, 50.0), false),
            Some(Point::new(100.0, 50.0))
        );
        assert_eq!(app.lands_on_axis(Point::new(120.0, 50.0), false), None);
        app.session.mirror.axes = Axes::Off;
        assert_eq!(app.lands_on_axis(Point::new(104.0, 50.0), false), None);
    }

    /// With the pointer by the axis the canvas draws *On axis*; away from it, not.
    ///
    /// flip: `mirror_chrome` skipping the chip → fails at the first assertion.
    #[test]
    fn the_on_axis_chip_is_painted_by_the_axis_only() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = mirroring_pen(&ctx);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let says_on_axis = |app: &mut OndinApp, x: f32| {
            let pointer = app.to_screen(Point::new(f64::from(x), 50.0), rect, 1.0);
            let mut out = None;
            for _ in 0..2 {
                let input = egui::RawInput {
                    screen_rect: Some(rect),
                    events: vec![egui::Event::PointerMoved(pointer)],
                    ..Default::default()
                };
                out = Some(ctx.run_ui(input, |ui| {
                    let painter = ui.painter().clone();
                    app.mirror_chrome(ui, &painter, rect, 1.0);
                }));
            }
            out.expect("ran")
                .shapes
                .iter()
                .any(|c| matches!(&c.shape, egui::Shape::Text(t) if t.galley.text() == "On axis"))
        };
        assert!(says_on_axis(&mut app, 103.0), "by the axis");
        assert!(!says_on_axis(&mut app, 160.0), "away from it");
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

    /// A headless app holding a diamond the pen drew mirrored about x = 100 —
    /// top (100, 10), left (60, 50), bottom (100, 90), right (140, 50) — under
    /// the node tool.
    fn mirrored_diamond(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(1);
        app.session
            .adopt_document(ondin_core::Document::new(ids.mint()), None);
        app.canvas_px = (800, 600);
        app.session.camera.center = Point::new(100.0, 50.0);
        app.session.camera.zoom = 1.0;
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
        app.finish_pen(false);
        let id = app.session.selection.single().expect("the path");
        app.session.mirror.axes = Axes::Off;
        app.choose_tool(crate::tools::Tool::Node);
        app.session.selection.set_one(id);
        (app, id)
    }

    /// The anchors of `id`, in world space, in order.
    fn anchors(app: &OndinApp, id: NodeId) -> Vec<Point> {
        let Some(NodeKind::Path {
            path, corner_radii, ..
        }) = app.session.doc.get(id).map(|n| n.kind())
        else {
            panic!("a path");
        };
        let world = app.session.resolved.world_transform(id).expect("placed");
        crate::tools::pen_anchors(path, corner_radii)
            .iter()
            .flat_map(|s| s.anchors.iter().map(|a| world * a.at))
            .collect()
    }

    fn mirror_of(app: &OndinApp, id: NodeId) -> PathMirror {
        match app.session.doc.get(id).map(|n| n.kind()) {
            Some(NodeKind::Path { mirror, .. }) => *mirror,
            _ => panic!("a path"),
        }
    }

    /// Where `p` sits among `id`'s anchors.
    fn index_of(app: &OndinApp, id: NodeId, p: Point) -> PointRef {
        let i = anchors(app, id)
            .iter()
            .position(|a| (*a - p).hypot() < 1e-6)
            .unwrap_or_else(|| panic!("an anchor at {p:?}"));
        (0, i)
    }

    fn has(app: &OndinApp, id: NodeId, p: Point) -> bool {
        anchors(app, id).iter().any(|a| (*a - p).hypot() < 1e-6)
    }

    /// A path drawn mirrored is saved mirrored, and its size badge says so.
    /// flip: `finish_pen` without `mark_mirrored` → fails at the first assertion.
    #[test]
    fn a_path_drawn_mirrored_is_saved_mirrored_and_badged() {
        let ctx = egui::Context::default();
        let (app, id) = mirrored_diamond(&ctx);
        assert!(mirror_of(&app, id).vertical);
        let marks = crate::canvas::size_badge_marks(&app.session.doc, &[id]);
        assert_eq!(
            marks,
            [crate::theme::icon::FLIP_HORIZONTAL],
            "the size badge"
        );
    }

    /// Dragging the left anchor of a mirrored path takes the right one with it,
    /// mirrored, live and committed; the path stays mirrored, and undo is one step.
    /// flip: `points_tx` writing through `tools::rewrite_path` → fails at "the image followed".
    #[test]
    fn a_point_drag_on_a_mirrored_path_mirrors_onto_the_image() {
        let ctx = egui::Context::default();
        let (mut app, id) = mirrored_diamond(&ctx);
        let left = index_of(&app, id, Point::new(60.0, 50.0));
        app.points.set_one(id, left);
        let tx = app.points_tx(id, Vec2::new(-10.0, 5.0));
        assert!(app.session.commit(tx));
        assert!(has(&app, id, Point::new(50.0, 55.0)));
        assert!(has(&app, id, Point::new(150.0, 55.0)), "the image followed");
        assert!(mirror_of(&app, id).vertical, "still mirrored");
        app.undo();
        assert!(has(&app, id, Point::new(140.0, 50.0)));
    }

    /// The top anchor sits on the axis: dragged sideways, it only slides along it.
    #[test]
    fn an_axis_point_is_locked_to_the_axis() {
        let ctx = egui::Context::default();
        let (mut app, id) = mirrored_diamond(&ctx);
        let top = index_of(&app, id, Point::new(100.0, 10.0));
        app.points.set_one(id, top);
        let tx = app.points_tx(id, Vec2::new(25.0, -4.0));
        assert!(app.session.commit(tx));
        assert!(has(&app, id, Point::new(100.0, 6.0)), "slid along the axis");
        assert!(mirror_of(&app, id).vertical);
    }

    /// Deleting a point deletes its image, and the path stays mirrored.
    /// flip: `delete_points` without `with_mirrored_points` → fails at "the image went".
    #[test]
    fn deleting_a_point_deletes_its_image() {
        let ctx = egui::Context::default();
        let (mut app, id) = mirrored_diamond(&ctx);
        let left = index_of(&app, id, Point::new(60.0, 50.0));
        app.points.set_one(id, left);
        app.delete_points();
        assert!(!has(&app, id, Point::new(140.0, 50.0)), "the image went");
        assert_eq!(anchors(&app, id).len(), 2);
    }

    /// A point added on one side is added on the other, and the new one is the
    /// one selected.
    /// flip: `insert_point` inserting only `(at, t)` → fails at the anchor count.
    #[test]
    fn adding_a_point_adds_its_image() {
        let ctx = egui::Context::default();
        let (mut app, id) = mirrored_diamond(&ctx);
        // On the top-left edge, a quarter of the way from the top.
        app.add_point_at(Point::new(90.0, 20.0));
        assert_eq!(anchors(&app, id).len(), 6);
        assert!(has(&app, id, Point::new(90.0, 20.0)));
        assert!(has(&app, id, Point::new(110.0, 20.0)), "and its image");
        assert!(mirror_of(&app, id).vertical);
        let picked: Vec<PointRef> = app.points.on(id).collect();
        assert_eq!(picked, vec![index_of(&app, id, Point::new(90.0, 20.0))]);
    }

    /// An edit that breaks the symmetry drops the axis, and undo brings it back.
    /// flip: the inverse `GeometryPatch::Path` carrying `mirror: None` → fails at "back".
    #[test]
    fn a_lopsided_edit_drops_the_axis_and_undo_restores_it() {
        let ctx = egui::Context::default();
        let (mut app, id) = mirrored_diamond(&ctx);
        let world = app.session.resolved.world_transform(id).unwrap();
        let (_, mut subs) = {
            let Some(NodeKind::Path {
                path, corner_radii, ..
            }) = app.session.doc.get(id).map(|n| n.kind())
            else {
                panic!()
            };
            let mut s = crate::tools::pen_anchors(path, corner_radii);
            crate::tools::map_subpaths(&mut s, world);
            ((), s)
        };
        subs[0].anchors[1].at.x -= 7.0;
        let tx = crate::tools::rewrite_path(id, world, &subs, app.stored_path(id));
        assert!(app.session.commit(tx));
        assert!(mirror_of(&app, id).is_none(), "dropped");
        app.undo();
        assert!(mirror_of(&app, id).vertical, "back");
    }

    /// The node tool draws a mirrored path's own axis through it, and a plain
    /// path's none.
    /// flip: `mirror_frame` skipping the edited path's own axes → fails at "drawn".
    #[test]
    fn the_node_tool_draws_a_mirrored_paths_axis() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, id) = mirrored_diamond(&ctx);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let axis_x = app.to_screen(Point::new(100.0, 50.0), rect, 1.0).x;
        let dashes = |app: &mut OndinApp| {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ui| {
                    let painter = ui.painter().clone();
                    app.mirror_chrome(ui, &painter, rect, 1.0);
                },
            );
            out.shapes
                .iter()
                .filter(|c| match &c.shape {
                    egui::Shape::LineSegment { points, .. } => {
                        points.iter().all(|p| (p.x - axis_x).abs() < 0.5)
                    }
                    _ => false,
                })
                .count()
        };
        assert!(dashes(&mut app) > 0, "drawn");
        app.flatten_selection();
        assert!(!mirror_of(&app, id).vertical);
        assert_eq!(dashes(&mut app), 0, "a plain path has none");
    }

    /// Flatten takes a lone mirrored path, and leaves it plain and where it was.
    #[test]
    fn flatten_makes_a_mirrored_path_plain() {
        let ctx = egui::Context::default();
        let (mut app, id) = mirrored_diamond(&ctx);
        app.choose_tool(crate::tools::Tool::Select);
        app.session.selection.set_one(id);
        let before = anchors(&app, id);
        app.flatten_selection();
        assert!(mirror_of(&app, id).is_none());
        assert_eq!(anchors(&app, id), before);
    }

    /// A new run drawn inside a mirrored path is mirrored across the path's own
    /// axis, whatever the drawing switch says.
    #[test]
    fn a_new_run_inside_a_mirrored_path_mirrors_across_its_axis() {
        let ctx = egui::Context::default();
        let (mut app, id) = mirrored_diamond(&ctx);
        let world = app.session.resolved.world_transform(id).unwrap();
        let Some(NodeKind::Path {
            path, corner_radii, ..
        }) = app.session.doc.get(id).map(|n| n.kind())
        else {
            panic!()
        };
        let mut subpaths = crate::tools::pen_anchors(path, corner_radii);
        crate::tools::map_subpaths(&mut subpaths, world);
        let active = subpaths.len();
        subpaths.push(PenSubpath {
            anchors: Vec::new(),
            closed: false,
        });
        app.pen = Some(crate::preview::PenState {
            parent: app.session.page,
            parent_world: Affine::IDENTITY,
            anchors: [(70.0, 40.0), (80.0, 60.0)]
                .map(|(x, y)| PenAnchor::corner(Point::new(x, y)))
                .to_vec(),
            shaping: false,
            editing: Some(crate::preview::PenEdit {
                id,
                world,
                subpaths,
                active,
                from_start: false,
                minted: true,
            }),
        });
        app.finish_pen(false);
        assert!(has(&app, id, Point::new(130.0, 40.0)));
        assert!(has(&app, id, Point::new(120.0, 60.0)));
        assert!(mirror_of(&app, id).vertical, "and stays mirrored");
    }

    const RECT: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::Pos2::new(800.0, 600.0),
    };

    /// One frame of the canvas alone, with these events.
    fn canvas_frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let time = ctx.input(|i| i.time) + 0.1;
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(RECT),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| app.canvas_ui(ui),
        )
    }

    /// A few frames with the pointer at `p`, so hover settles; the last's output.
    fn hover(ctx: &egui::Context, app: &mut OndinApp, p: egui::Pos2) -> egui::FullOutput {
        for _ in 0..3 {
            canvas_frame(ctx, app, vec![egui::Event::PointerMoved(p)]);
        }
        canvas_frame(ctx, app, vec![egui::Event::PointerMoved(p)])
    }

    fn primary(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// The vertical pill's hit rect as last painted.
    fn pill(app: &OndinApp) -> egui::Rect {
        let pills = &app.mirror_grips.pills;
        assert_eq!(pills.len(), 1, "one pill: {pills:?}");
        pills[0].1
    }

    /// Over a frame the axis runs through it from its middle, the pill on its top
    /// edge; off it, down the view from the view's middle, the pill under the
    /// ruler — even with the view's middle inside the frame.
    ///
    /// flip: `axis_frame` taking the frame under the axes' crossing → fails at "off the frame".
    #[test]
    fn the_axis_runs_through_the_frame_under_the_pointer_or_down_the_view() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(1);
        app.session
            .adopt_document(ondin_core::Document::new(ids.mint()), None);
        app.session.camera.center = Point::ZERO;
        app.session.camera.zoom = 1.0;
        let page = app.session.page;
        let frame = app.session.ids.mint();
        let tx = crate::tools::create_artboard(
            frame,
            page,
            0,
            Affine::IDENTITY,
            Point::new(-200.0, -100.0),
            Point::new(200.0, 100.0),
            ondin_core::peniko::Color::WHITE,
        );
        assert!(app.session.commit(tx));
        app.choose_tool(crate::tools::Tool::Pen);
        app.session.mirror.axes = Axes::Vertical;
        canvas_frame(&ctx, &mut app, Vec::new());

        let inside = app.to_screen(Point::new(50.0, 0.0), RECT, 1.0);
        hover(&ctx, &mut app, inside);
        let on = pill(&app);
        let top = app.to_screen(Point::new(0.0, -100.0), RECT, 1.0);
        assert!(
            (on.center().x - top.x).abs() < 0.5,
            "the frame's middle: {on:?}"
        );
        assert!((on.top() + 1.0 - top.y).abs() < 0.5, "on the frame: {on:?}");

        let outside = app.to_screen(Point::new(50.0, 250.0), RECT, 1.0);
        hover(&ctx, &mut app, outside);
        let off = pill(&app);
        let inner = app.canvas_inner(RECT);
        assert!(
            (off.top() + 1.0 - inner.top()).abs() < 0.5,
            "off the frame: {off:?}"
        );
        assert!(
            (off.center().x - inner.center().x).abs() < 0.5,
            "the view's middle: {off:?}"
        );
    }

    /// A press on the pill and its drag move the axis: the pen plants nothing, and
    /// the pill shows no *On axis* chip.
    ///
    /// flip: `normal_mode_input` without the pill's claim → fails at "the pen plants nothing".
    #[test]
    fn a_drag_on_the_pill_moves_the_axis_and_draws_nothing() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = mirroring_pen(&ctx);
        canvas_frame(&ctx, &mut app, Vec::new());
        let p = pill(&app).center();
        let out = hover(&ctx, &mut app, p);
        assert!(
            !out.shapes
                .iter()
                .any(|c| matches!(&c.shape, egui::Shape::Text(t) if t.galley.text() == "On axis")),
            "no chip on the pill"
        );
        let to = p + egui::vec2(40.0, 0.0);
        canvas_frame(&ctx, &mut app, vec![primary(p, true)]);
        for i in 1..=4 {
            let at = p + egui::vec2(10.0 * i as f32, 0.0);
            canvas_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        }
        canvas_frame(&ctx, &mut app, vec![primary(to, false)]);
        assert!(app.pen.is_none(), "the pen plants nothing");
        let at = app.session.mirror.at.expect("dragged");
        assert!((at.x - 140.0).abs() < 0.5, "the axis moved: {at:?}");
    }

    /// A right-click during a pill drag puts the axis back where the drag found it,
    /// and the rest of the drag moves nothing.
    ///
    /// flip: `cancel_gesture` without the pill's arm → fails at "put back".
    #[test]
    fn a_right_click_mid_drag_puts_the_axis_back() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = mirroring_pen(&ctx);
        canvas_frame(&ctx, &mut app, Vec::new());
        let p = pill(&app).center();
        hover(&ctx, &mut app, p);
        canvas_frame(&ctx, &mut app, vec![primary(p, true)]);
        for i in 1..=3 {
            let at = p + egui::vec2(10.0 * i as f32, 0.0);
            canvas_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        }
        let moved = app.session.mirror.at.expect("dragged");
        assert!(
            (moved.x - 130.0).abs() < 0.5,
            "the drag moved it: {moved:?}"
        );
        // As the app's frame does on a right press, ahead of the UI.
        assert!(app.cancel_gesture(&ctx));
        assert_eq!(
            app.session.mirror.at,
            Some(Point::new(100.0, 0.0)),
            "put back"
        );
        let on = p + egui::vec2(60.0, 0.0);
        canvas_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(on)]);
        canvas_frame(&ctx, &mut app, vec![primary(on, false)]);
        assert_eq!(
            app.session.mirror.at,
            Some(Point::new(100.0, 0.0)),
            "and stays"
        );
        assert!(app.pen.is_none(), "the pen plants nothing");
    }

    /// Leaving the pen puts a dragged axis back; the pencil and the eyedropper's
    /// round trip keep it.
    ///
    /// flip: `choose_tool` without the reset → fails at "leaving the pen".
    #[test]
    fn leaving_the_pen_puts_the_axis_back() {
        let ctx = egui::Context::default();
        let mut app = mirroring_pen(&ctx);
        app.choose_tool(crate::tools::Tool::Pencil);
        assert!(app.session.mirror.at.is_some(), "the pencil keeps it");
        app.choose_tool(crate::tools::Tool::Eyedropper);
        app.choose_tool(crate::tools::Tool::Eyedropper);
        assert_eq!(app.tool, crate::tools::Tool::Pencil);
        assert!(
            app.session.mirror.at.is_some(),
            "the eyedropper's trip keeps it"
        );
        app.choose_tool(crate::tools::Tool::Select);
        assert_eq!(app.session.mirror.at, None, "leaving the pen");
        assert_eq!(app.session.mirror.axes, Axes::Vertical, "the axes stay on");
    }
}
