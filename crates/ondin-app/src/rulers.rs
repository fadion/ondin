//! The rulers, and the guides pulled out of them (the design canvas's *Editor*
//! artboard).
//!
//! Two 20pt bars along the top and left of the canvas, ticked and labelled in
//! world units, plus the guides dragged off them. The guides are document
//! state (`ondin_core::guide`); this file is the chrome that shows and edits them.
//!
//! **Drawn as egui chrome, not scene content**, so a guide stays one hairline at
//! every zoom, sits above a clipping frame, and is invisible to `ondin export`.
//!
//! **The tick step is chosen, not fixed:** the smallest 1/2/5×10ⁿ step whose
//! labels clear [`MIN_LABEL_PX`].

use crate::app::OndinApp;
use crate::cursor::Bitmap;
use crate::preview::{Drag, GuideDrag};
use crate::theme::{self, color};
use eframe::egui;
use ondin_core::kurbo::{Affine, Point, Vec2};
use ondin_core::{Guide, GuideAxis, GuideId, NodeId, Operation, Transaction};

/// Thickness of each ruler bar, in points.
pub const THICKNESS: f32 = 20.0;

/// Tick length, measured in from the ruler's inner edge.
const TICK_LEN: f32 = 6.0;
/// Tick hairline width.
const TICK_W: f32 = 1.0;
/// The label beside a tick, and its offset along the ruler from that tick.
const LABEL_SIZE: f32 = 7.5;
const LABEL_GAP: f32 = 3.0;
/// Inset of a label from the ruler's outer edge (the design's `top:3px`).
const LABEL_INSET: f32 = 3.0;

/// The narrowest gap between two labelled ticks; below it the ruler picks a
/// coarser step.
const MIN_LABEL_PX: f64 = 50.0;

/// The position chip a guide puts in the ruler: thickness along the guide's own
/// axis, and the type inside it.
const CHIP_THICKNESS: f32 = 13.0;
const CHIP_TEXT: f32 = 8.0;
/// Padding either side of a chip's number.
///
/// 2.5 so a three-digit number (14.75pt at [`CHIP_TEXT`]) plus 5 clamps up to
/// exactly [`THICKNESS`]; only a longer number grows the chip.
const CHIP_PAD_X: f32 = 2.5;

/// How close a tick's number may come to one of the selection band's own
/// numbers before it is dropped in favour of it. Roughly the width of a
/// three-digit label, so the two never overprint.
const EXTENT_LABEL_CLEAR: f32 = 16.0;

/// The band across a ruler covering what is selected, clipped to the bar.
/// `None` when none of it is on screen.
///
/// It answers "how wide is it" straight off the ruler. A tinted ground rather
/// than an outline, so it reads as a region of the ruler.
fn extent_band(axis: GuideAxis, bar: egui::Rect, from: f32, to: f32) -> Option<egui::Rect> {
    let (lo, hi) = (from.min(to), from.max(to));
    // Clamped rather than skipped, so a selection running off the viewport
    // still shows its on-screen part.
    let band = match axis {
        GuideAxis::Vertical => egui::Rect::from_min_max(
            egui::pos2(lo.max(bar.left()), bar.top()),
            egui::pos2(hi.min(bar.right()), bar.bottom()),
        ),
        GuideAxis::Horizontal => egui::Rect::from_min_max(
            egui::pos2(bar.left(), lo.max(bar.top())),
            egui::pos2(bar.right(), hi.min(bar.bottom())),
        ),
    };
    (band.width() > 0.0 && band.height() > 0.0).then_some(band)
}

/// The along-the-bar component of a label position: `x` on the top bar, `y` on
/// the left one.
fn label_at_along(axis: GuideAxis, at: egui::Pos2) -> f32 {
    match axis {
        GuideAxis::Vertical => at.x,
        GuideAxis::Horizontal => at.y,
    }
}

/// How close (screen pt) the pointer must come to a guide to grab it; the same
/// as the selection box's edge band (`canvas::EDGE_PICK_PX`).
const GUIDE_PICK_PT: f32 = 6.0;

/// The dash and gap of a frame-scoped guide's continuation beyond its frame, in
/// screen points.
const EXTENSION_DASH: f32 = 3.0;
const EXTENSION_GAP: f32 = 3.0;

/// How far from square a frame's world basis may be and still count as upright,
/// so a frame turned and turned back stays upright.
const UPRIGHT_TOL: f64 = 1e-9;

/// Whether `world` leaves a frame square with the world axes.
///
/// A guide in a rotated frame draws, leaning with its frame, but neither snaps
/// (`OndinApp::guide_lines` skips it) nor extends; one predicate keeps those two
/// from drifting apart. A mirror does not disqualify a frame: it only flips a
/// sign.
fn upright(world: Affine) -> bool {
    let o = ondin_core::build::Orientation::of(world);
    o.angle.abs() < UPRIGHT_TOL && o.skew.abs() < UPRIGHT_TOL
}

/// A world point on the line a guide of `axis` draws at world coordinate
/// `position`. The other coordinate is arbitrary and unused.
fn on_axis(axis: GuideAxis, position: f64) -> Point {
    ondin_core::guide_point(axis, position)
}

/// One guide resolved into screen geometry: the solid line it draws, the number
/// its chip quotes, and whether it has a dotted continuation.
///
/// Drawing and picking are both built from this, so the hit test agrees with
/// what is drawn. A scoped guide's line is its frame's span carried through the
/// frame's world transform.
struct GuideLine {
    axis: GuideAxis,
    /// The solid segment's ends: the visible canvas for a global guide, the
    /// owner frame's own span for a scoped one.
    a: egui::Pos2,
    b: egui::Pos2,
    /// The guide's own number: a world coordinate when global, a frame-local
    /// offset when scoped (the number the inspector's field holds).
    label: f64,
    /// Whether the line continues past both ends, dashed, while hovered or
    /// selected. Scoped guides in upright frames only.
    extends: bool,
}

/// How far a guide about to be thrown away is faded.
const DISCARD_ALPHA: f32 = 0.35;

/// The ink a guide is drawn in, for its state.
///
/// `color::SELECT` replaces the guide's own colour while it is picked, so the
/// discard fade must be applied after that choice; fading first puts the alpha
/// on a value the blue then discards (pinned by
/// `a_discarding_guide_is_faded_even_though_it_is_drawn_as_selected`).
fn guide_ink(colour: ondin_core::peniko::Color, selected: bool, fade: bool) -> egui::Color32 {
    let ink = match selected {
        true => color::SELECT,
        false => crate::ui::color_to_egui(colour),
    };
    match fade {
        true => ink.gamma_multiply(DISCARD_ALPHA),
        false => ink,
    }
}

/// A coordinate snapped onto the device-pixel grid, so a hairline stays a
/// hairline. `grid::draw_pixel_grid` calls it too.
///
/// A line whose device width is odd wants its centre on a pixel centre and an
/// even one on a pixel edge; backwards turns a 1px line into a 2px grey smear.
/// The parity is `floor(width × ppp)`, decided by
/// `the_two_guide_weights_keep_a_solid_core_at_every_scale`: reading the point
/// width is wrong at 150%/175%, and `round` leaves a resting guide with no solid
/// pixel (1.5 rounds to 2, giving `[0.75, 0.75]`).
///
/// `ui::hairline_on_pixel_centre` is the shared parity predicate. The base differs
/// from `ui::hairline_in_column` on purpose: a chrome rule must stay inside its 1pt
/// allocation (floor), while a guide takes the nearest grid line (round).
pub(crate) fn snap_across_axis(v: f32, ppp: f32, width: f32) -> f32 {
    let snapped = (v * ppp).round() / ppp;
    match crate::ui::hairline_on_pixel_centre(ppp, width) {
        true => snapped + 0.5 / ppp,
        false => snapped,
    }
}

/// The two dashed runs that carry a guide's line out to the edges of the canvas,
/// past the ends of its frame.
///
/// Only for an axis-aligned guide ([`upright`]). Either run can come back empty
/// when the frame already reaches that edge.
fn extension_ends(
    axis: GuideAxis,
    inner: egui::Rect,
    a: egui::Pos2,
    b: egui::Pos2,
) -> Vec<(egui::Pos2, egui::Pos2)> {
    let (lo, hi) = match axis {
        GuideAxis::Horizontal => (a.x.min(b.x), a.x.max(b.x)),
        GuideAxis::Vertical => (a.y.min(b.y), a.y.max(b.y)),
    };
    let (near, far) = match axis {
        GuideAxis::Horizontal => (inner.left(), inner.right()),
        GuideAxis::Vertical => (inner.top(), inner.bottom()),
    };
    let at = |v: f32| match axis {
        GuideAxis::Horizontal => egui::pos2(v, a.y),
        GuideAxis::Vertical => egui::pos2(a.x, v),
    };
    [(near, lo), (hi, far)]
        .into_iter()
        .filter(|(from, to)| to - from > EXTENSION_DASH)
        .map(|(from, to)| (at(from), at(to)))
        .collect()
}

/// Cull a guide whose solid segment is entirely off the visible canvas.
///
/// A bounding-box test, not a per-end one: a leaning guide can have both ends
/// outside `inner` and still cross it.
fn in_view(inner: egui::Rect, line: GuideLine) -> Option<GuideLine> {
    let hull = egui::Rect::from_two_pos(line.a, line.b);
    inner.intersects(hull).then_some(line)
}

impl GuideLine {
    /// The along-the-bar coordinate the guide's chip sits at.
    ///
    /// The midpoint of the segment on the guide's own axis: exact for a global or
    /// upright guide, approximate placement for a rotated one (the number in the
    /// chip stays exact).
    fn chip_along(&self) -> f32 {
        match self.axis {
            GuideAxis::Horizontal => (self.a.y + self.b.y) / 2.0,
            GuideAxis::Vertical => (self.a.x + self.b.x) / 2.0,
        }
    }

    /// Screen distance from `p` to the solid segment — the pick test.
    ///
    /// To the segment, not the infinite line, so a scoped guide is grabbable
    /// exactly where it is drawn. The dotted continuation is not a target.
    fn distance_to(&self, p: egui::Pos2) -> f32 {
        let (a, b) = (self.a, self.b);
        let along = b - a;
        let len_sq = along.length_sq();
        if len_sq < f32::EPSILON {
            return (p - a).length();
        }
        let t = (((p - a).dot(along)) / len_sq).clamp(0.0, 1.0);
        (p - (a + along * t)).length()
    }
}

/// The smallest 1/2/5×10ⁿ step whose labels are at least [`MIN_LABEL_PX`]
/// apart, given how many screen points one world unit currently occupies.
fn nice_step(points_per_unit: f64) -> f64 {
    let raw = MIN_LABEL_PX / points_per_unit.max(f64::EPSILON);
    let magnitude = 10f64.powf(raw.log10().floor());
    for m in [1.0, 2.0, 5.0] {
        if magnitude * m >= raw {
            return magnitude * m;
        }
    }
    magnitude * 10.0
}

/// How many decimals a tick label needs at `step`, so a ruler zoomed in past
/// whole units does not print four identical `0`s.
fn label_decimals(step: f64) -> usize {
    (-step.log10().floor()).max(0.0) as usize
}

/// A tick's number. `-0` is spelled `0`: it is the same place, and the minus
/// sign in front of the origin reads as a bug.
fn tick_label(value: f64, decimals: usize) -> String {
    let v = if value == 0.0 { 0.0 } else { value };
    format!("{v:.decimals$}")
}

/// The same number in thousands (`20000` → `20k`), for a bar too narrow to
/// read the long form across. See [`OndinApp::tick_number`].
fn tick_label_thousands(value: f64, step: f64) -> String {
    let scaled = tick_label(value / 1000.0, label_decimals(step / 1000.0));
    format!("{}k", trim_zeroes(&scaled))
}

/// Drop a fixed-point string's trailing zeroes, and the point with them if
/// nothing is left after it.
fn trim_zeroes(s: &str) -> &str {
    match s.contains('.') {
        true => s.trim_end_matches('0').trim_end_matches('.'),
        false => s,
    }
}

/// A guide chip's number: the position to two decimals, with trailing zeroes
/// dropped.
///
/// Not the ruler's own precision, and never abbreviated: the chip quotes one
/// exact coordinate, the number the inspector holds, so a guide at 12.5 must not
/// read `13`. The chip grows to fit its number rather than the reverse.
fn chip_label(position: f64) -> String {
    let v = if position == 0.0 { 0.0 } else { position };
    trim_zeroes(&format!("{v:.2}")).to_string()
}

/// [`chip_label`] in thousands (`1080` → `1.1k`), for the left bar, which gives
/// a label 17pt to read across.
///
/// One decimal, not [`chip_label`]'s two, to fit where four digits do not.
/// [`tick_label_thousands`] is the tick's version; it needs a `step`, which a
/// band has not.
fn chip_label_thousands(position: f64) -> String {
    format!("{}k", trim_zeroes(&format!("{:.1}", position / 1000.0)))
}

/// Which ruler bar a screen position is over — the geometry alone; callers use
/// [`OndinApp::ruler_at`], which also checks the bars are shown.
///
/// The corner square belongs to the left ruler, which is painted over it.
fn ruler_at(rect: egui::Rect, p: egui::Pos2) -> Option<GuideAxis> {
    if !rect.contains(p) {
        return None;
    }
    if p.x <= rect.left() + THICKNESS {
        // The left ruler runs vertically and pulls out vertical guides.
        Some(GuideAxis::Vertical)
    } else if p.y <= rect.top() + THICKNESS {
        Some(GuideAxis::Horizontal)
    } else {
        None
    }
}

impl OndinApp {
    /// Whether the ruler bars are on screen: switched on (*View ▸ Show rulers*)
    /// and not suppressed by focus mode.
    ///
    /// Every reader goes through this, so the hit test and the paint agree.
    pub(crate) fn rulers_on(&self) -> bool {
        self.show_rulers && !self.focus
    }

    /// Which ruler bar a screen position is over, or `None` — including when the
    /// bars are not on screen at all.
    ///
    /// The method half of the free [`ruler_at`], so no caller forgets the
    /// `rulers_on` check.
    pub(crate) fn ruler_at(&self, p: egui::Pos2, rect: egui::Rect) -> Option<GuideAxis> {
        self.rulers_on().then(|| ruler_at(rect, p)).flatten()
    }

    /// Whether the guide **lines** are drawn: switched on (*View ▸ Show guides*),
    /// and that is the whole of it.
    ///
    /// Focus mode is deliberately not folded in: it hides the app's chrome while
    /// the canvas keeps working, and guides, like the selection box and layout
    /// grids, belong to the canvas. [`Self::rulers_on`] folds it in because the
    /// bars are window furniture.
    ///
    /// So in focus mode the lines stay drawn and pickable (through
    /// [`Self::guides_pickable`]) and only creating one, a drag off a bar, is
    /// lost; the Guide panel's position field is the way in.
    pub(crate) fn guides_on(&self) -> bool {
        self.show_guides
    }

    /// Whether a guide can be picked up at all: drawn, and not locked.
    ///
    /// Locked means not selectable outright; hover, cursor, press, drag and
    /// discard all fall away from this one predicate.
    pub(crate) fn guides_pickable(&self) -> bool {
        self.guides_on() && !self.lock_guides
    }

    /// A point in `owner`'s local space, from a world point. Identity when there
    /// is no owner, since a global guide is already measured in world space.
    ///
    /// `None` when the transform lookup answers nothing (mid-load); the model
    /// does not allow a gone owner (`Document::check_guide_owner`).
    fn guide_local(&self, owner: Option<NodeId>, world: Point) -> Option<Point> {
        match owner {
            None => Some(world),
            Some(id) => Some(self.session.preview_world_transform(id)?.inverse() * world),
        }
    }

    /// The reverse of [`Self::guide_local`].
    fn guide_world(&self, owner: Option<NodeId>, local: Point) -> Option<Point> {
        match owner {
            None => Some(local),
            Some(id) => Some(self.session.preview_world_transform(id)? * local),
        }
    }

    /// The **world** coordinate a guide pins, or `None` when it does not pin one.
    ///
    /// A global guide's position is that coordinate; a scoped guide's is its
    /// offset carried through its frame's transform, meaningful only while the
    /// frame is [`upright`]. In a rotated frame there is none, which is why such
    /// a guide draws but does not attract; `GuideLines` stays plain xs and ys.
    fn guide_world_coord(
        &self,
        axis: GuideAxis,
        owner: Option<NodeId>,
        position: f64,
    ) -> Option<f64> {
        let Some(id) = owner else {
            return Some(position);
        };
        let world = self.session.preview_world_transform(id)?;
        if !upright(world) {
            return None;
        }
        Some(ondin_core::guide_coord_of(
            axis,
            world * on_axis(axis, position),
        ))
    }

    /// A guide as it should currently be *shown*: the committed guide with any
    /// live edit read through it.
    ///
    /// The counterpart of [`EditorSession::display_node`] for guides, which have
    /// no scene fragment to override: the canvas drag and the inspector scrub are
    /// read through here. Drawing and hit-testing both use it, so they agree.
    ///
    /// [`EditorSession::display_node`]: crate::session::EditorSession::display_node
    pub(crate) fn shown_guide(&self, guide: &Guide) -> Guide {
        let mut out = self
            .guide_previews
            .iter()
            .find(|g| g.id == guide.id)
            .copied()
            .unwrap_or(*guide);
        if let Drag::Guide(d) = &self.drag
            && d.id == Some(guide.id)
            // Not during an Alt-copy: this guide is then the original, which stays
            // put while the copy travels.
            && !d.copying
        {
            // Owner and position together: the position is in the owner's space.
            out.position = d.position;
            out.owner = d.owner;
        }
        out
    }

    /// The screen geometry of a guide, or `None` when none of it is in view.
    ///
    /// `inner` is the canvas past both bars: what a global guide spans and what
    /// a scoped one's dotted continuation is bounded by. A scoped guide's solid
    /// segment is bounded by its frame instead.
    fn guide_line(
        &self,
        guide: &Guide,
        rect: egui::Rect,
        inner: egui::Rect,
        ppp: f32,
    ) -> Option<GuideLine> {
        self.guide_line_of(guide.axis, guide.owner, guide.position, rect, inner, ppp)
    }

    /// [`Self::guide_line`] from loose parts, for the guide in flight — which has
    /// no `Guide` in the document to read yet.
    fn guide_line_of(
        &self,
        axis: GuideAxis,
        owner: Option<NodeId>,
        position: f64,
        rect: egui::Rect,
        inner: egui::Rect,
        ppp: f32,
    ) -> Option<GuideLine> {
        let Some(id) = owner else {
            // Global: the full width or height of the visible canvas.
            let (a, b) = match axis {
                GuideAxis::Horizontal => {
                    let y = self.to_screen(Point::new(0.0, position), rect, ppp).y;
                    (egui::pos2(inner.left(), y), egui::pos2(inner.right(), y))
                }
                GuideAxis::Vertical => {
                    let x = self.to_screen(Point::new(position, 0.0), rect, ppp).x;
                    (egui::pos2(x, inner.top()), egui::pos2(x, inner.bottom()))
                }
            };
            return in_view(
                inner,
                GuideLine {
                    axis,
                    a,
                    b,
                    label: position,
                    extends: false,
                },
            );
        };
        // Scoped: the frame's box carried through its world transform, read
        // through the preview layer so it tracks a frame being dragged.
        let world = self.session.preview_world_transform(id)?;
        let box_ = self.session.preview_local_box(id)?;
        let (a, b) = ondin_core::guide_span(axis, position, box_);
        let to_screen = |p: Point| self.to_screen(world * p, rect, ppp);
        in_view(
            inner,
            GuideLine {
                axis,
                a: to_screen(a),
                b: to_screen(b),
                label: position,
                extends: upright(world),
            },
        )
    }

    // --- drawing ----------------------------------------------------------

    /// The rulers, then the guides. Called at the end of the canvas draw, so
    /// both sit above the artwork and the selection chrome.
    pub(crate) fn draw_rulers_and_guides(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        hover: Option<egui::Pos2>,
    ) {
        // Through the shared helper because `guide_at` measures against the same
        // rectangle: the segment a guide is picked by is the one it is drawn as.
        let inner = self.canvas_inner(rect);
        let hovered = hover.and_then(|p| self.guide_at(p, rect, ppp));

        // Bars first: a guide's chip sits inside the ruler and must paint over it.
        if self.rulers_on() {
            self.draw_ruler_bars(painter, rect, ppp, hover);
        }

        // The lines are gated by `guides_on`, not by `rulers_on`.
        if !self.guides_on() {
            return;
        }

        for guide in self.session.doc.guides() {
            // The one being dragged is drawn from the gesture below, unless the
            // drag is an Alt-copy, where the original stays put.
            if matches!(&self.drag, Drag::Guide(d) if d.id == Some(guide.id) && !d.copying) {
                continue;
            }
            let shown = self.shown_guide(guide);
            let selected = self.session.selection.has_guide(guide.id);
            let Some(line) = self.guide_line(&shown, rect, inner, ppp) else {
                continue;
            };
            self.draw_guide(
                painter,
                rect,
                inner,
                ppp,
                &line,
                shown.color(),
                selected,
                selected || hovered == Some(guide.id),
            );
        }

        // The guide in flight. A discarding drag is drawn faded rather than
        // hidden, so the gesture says what releasing will do. The fade is applied
        // inside `guide_ink`, after the selection blue replaces the colour.
        if let Drag::Guide(d) = &self.drag {
            let colour = match d.id.and_then(|id| self.session.doc.guide(id)) {
                Some(g) => self.shown_guide(g).color(),
                None => ondin_core::DEFAULT_GUIDE_COLOR,
            };
            if let Some(line) = self.guide_line_of(d.axis, d.owner, d.position, rect, inner, ppp) {
                self.draw_guide_faded(
                    painter,
                    rect,
                    inner,
                    ppp,
                    &line,
                    colour,
                    d.announces_discard(),
                );
            }
        }
    }

    /// [`Self::draw_guide`] for the guide in hand: always drawn as selected and
    /// always chipped, and faded when letting go would throw it away.
    #[allow(clippy::too_many_arguments)]
    fn draw_guide_faded(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        inner: egui::Rect,
        ppp: f32,
        line: &GuideLine,
        colour: ondin_core::peniko::Color,
        discarding: bool,
    ) {
        self.draw_guide_inner(
            painter, rect, inner, ppp, line, colour, true, true, discarding,
        );
    }

    /// One guide: the line it draws, its dotted continuation where it has one,
    /// and the chip naming its position in the ruler it came from.
    ///
    /// Three states, and only the selected one changes the ink. Resting is 1px in
    /// the guide's own colour; hover adds the chip and the extension, nothing
    /// else; selected is 2px in the selection blue with the chip. The blue
    /// overrides a recoloured guide while picked, matching every other selection
    /// ink in the app: intended.
    #[allow(clippy::too_many_arguments)]
    fn draw_guide(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        inner: egui::Rect,
        ppp: f32,
        line: &GuideLine,
        colour: ondin_core::peniko::Color,
        selected: bool,
        chip: bool,
    ) {
        self.draw_guide_inner(
            painter, rect, inner, ppp, line, colour, selected, chip, false,
        );
    }

    /// The whole of [`Self::draw_guide`], with the fade a discarding drag needs.
    ///
    /// Split only so the ordinary call site need not pass a meaningless `false`.
    #[allow(clippy::too_many_arguments)]
    fn draw_guide_inner(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        inner: egui::Rect,
        ppp: f32,
        line: &GuideLine,
        colour: ondin_core::peniko::Color,
        selected: bool,
        chip: bool,
        fade: bool,
    ) {
        let ink = guide_ink(colour, selected, fade);
        let width = if selected { 2.0 } else { 1.0 };
        let at = |v: f32| snap_across_axis(v, ppp, width);
        // Snapped across the line only; rounding along it would shorten a scoped
        // guide.
        let snap_across = |p: egui::Pos2| match line.axis {
            GuideAxis::Horizontal => egui::pos2(p.x, at(p.y)),
            GuideAxis::Vertical => egui::pos2(at(p.x), p.y),
        };
        // A leaning guide has no single across-axis coordinate; snapping its ends
        // independently would tilt it.
        let leaning = match line.axis {
            GuideAxis::Horizontal => line.a.y != line.b.y,
            GuideAxis::Vertical => line.a.x != line.b.x,
        };
        let (a, b) = match leaning {
            true => (line.a, line.b),
            false => (snap_across(line.a), snap_across(line.b)),
        };

        // Clipped to the canvas past the bars; a leaning line crosses the
        // boundary at an angle, so a coordinate test cannot do this.
        let clipped = painter.with_clip_rect(inner);
        let stroke = egui::Stroke::new(width, ink);
        clipped.line_segment([a, b], stroke);

        // The dashed continuation shares the solid part's stroke so it cannot
        // drift from it.
        if line.extends && chip {
            for (from, to) in extension_ends(line.axis, inner, a, b) {
                for shape in
                    egui::Shape::dashed_line(&[from, to], stroke, EXTENSION_DASH, EXTENSION_GAP)
                {
                    clipped.add(shape);
                }
            }
        }

        if !self.rulers_on() || !chip {
            return;
        }
        let along = line.chip_along();
        let chip_center = match line.axis {
            GuideAxis::Horizontal => {
                if along < inner.top() || along > inner.bottom() {
                    return;
                }
                egui::pos2(rect.left() + THICKNESS / 2.0, along)
            }
            GuideAxis::Vertical => {
                if along < inner.left() || along > inner.right() {
                    return;
                }
                egui::pos2(along, rect.top() + THICKNESS / 2.0)
            }
        };
        self.draw_guide_chip(painter, line.axis, chip_center, line.label, ink);
    }

    /// The number a guide shows in its ruler, on a ground of the guide's own
    /// colour or the selection blue while picked.
    ///
    /// Only for the guide being held, hovered or selected; on every guide the
    /// chips would fill the ruler.
    fn draw_guide_chip(
        &self,
        painter: &egui::Painter,
        axis: GuideAxis,
        center: egui::Pos2,
        position: f64,
        ink: egui::Color32,
    ) {
        let text = chip_label(position);
        let font = egui::FontId::proportional(CHIP_TEXT);
        let galley = painter.layout_no_wrap(text, font, CHIP_INK);
        // Both orientations grow along the reading direction: unlike a tick label
        // (`tick_number`), the chip is never dropped or cut to fit.
        let along = galley.size().x + CHIP_PAD_X * 2.0;
        let size = match axis {
            // Across the left ruler's width, 13pt along the guide's own axis.
            GuideAxis::Horizontal => egui::vec2(along.max(THICKNESS), CHIP_THICKNESS),
            // Mirrored: the top ruler's full height.
            GuideAxis::Vertical => egui::vec2(along.max(THICKNESS), THICKNESS),
        };
        // A horizontal guide's chip grows out of the left bar into the canvas,
        // reading as a tab pulled out of the ruler.
        let chip = match axis {
            GuideAxis::Horizontal => egui::Rect::from_min_size(
                egui::pos2(center.x - THICKNESS / 2.0, center.y - CHIP_THICKNESS / 2.0),
                size,
            ),
            GuideAxis::Vertical => egui::Rect::from_center_size(center, size),
        };
        painter.rect_filled(chip, egui::CornerRadius::ZERO, ink);
        painter.galley(
            chip.center() - galley.size() / 2.0,
            galley,
            egui::Color32::PLACEHOLDER,
        );
    }

    /// The two bars: ground, inner hairline, ticks, labels, and the pointer's
    /// position marked on both.
    fn draw_ruler_bars(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        hover: Option<egui::Pos2>,
    ) {
        let top =
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.top() + THICKNESS));
        let left =
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + THICKNESS, rect.bottom()));

        // Opaque, in the chrome's ground: the near-white ticks and type would be
        // unreadable over a light canvas or artwork.
        for bar in [top, left] {
            painter.rect_filled(bar, egui::CornerRadius::ZERO, color::BG);
        }
        let hairline = egui::Stroke::new(1.0, theme::color::text_a(18));
        painter.line_segment(
            [
                egui::pos2(top.left(), top.bottom() - 0.5),
                egui::pos2(top.right(), top.bottom() - 0.5),
            ],
            hairline,
        );
        painter.line_segment(
            [
                egui::pos2(left.right() - 0.5, left.top()),
                egui::pos2(left.right() - 0.5, left.bottom()),
            ],
            hairline,
        );

        let points_per_unit = self.session.camera.zoom / ppp as f64;
        let step = nice_step(points_per_unit);
        let decimals = label_decimals(step);
        let tick_ink = theme::color::text_a(51);
        let label_ink = theme::color::text_a(77);

        // Where the numbers count from, and what the selection spans.
        let origin = self.ruler_origin();
        let extent = self.selection_extent();

        // The world range each bar covers, as offsets from the origin, so with a
        // frame selected `0` lands on the frame's edge.
        let world_min = self.to_world(rect.min, rect, ppp) - origin;
        let world_max = self.to_world(rect.max, rect, ppp) - origin;

        for (axis, bar, from, to) in [
            (GuideAxis::Vertical, top, world_min.x, world_max.x),
            (GuideAxis::Horizontal, left, world_min.y, world_max.y),
        ] {
            // The canvas painter clips to the whole canvas, so this is the hard
            // backstop under `tick_number`'s choice of form.
            let painter = painter.with_clip_rect(bar);
            let on_screen = |value: f64| {
                let world = match axis {
                    GuideAxis::Vertical => Point::new(origin.x + value, 0.0),
                    GuideAxis::Horizontal => Point::new(0.0, origin.y + value),
                };
                let p = self.to_screen(world, rect, ppp);
                let raw = match axis {
                    GuideAxis::Vertical => p.x,
                    GuideAxis::Horizontal => p.y,
                };
                (raw * ppp).round() / ppp
            };

            // The selection band goes under the ticks.
            let ends = extent.map(|(x, y)| match axis {
                GuideAxis::Vertical => (x.0 - origin.x, x.1 - origin.x),
                GuideAxis::Horizontal => (y.0 - origin.y, y.1 - origin.y),
            });
            if let Some((lo, hi)) = ends
                && let Some(band) = extent_band(axis, bar, on_screen(lo), on_screen(hi))
            {
                painter.rect_filled(band, egui::CornerRadius::ZERO, color::ACCENT_900);
            }

            let first = (from / step).floor() as i64;
            let last = (to / step).ceil() as i64;
            for i in first..=last {
                let value = i as f64 * step;
                let (tick, label_at) = match axis {
                    GuideAxis::Vertical => {
                        let x = on_screen(value);
                        if x < top.left() + THICKNESS || x > top.right() {
                            continue;
                        }
                        (
                            egui::Rect::from_min_size(
                                egui::pos2(x, top.bottom() - TICK_LEN),
                                egui::vec2(TICK_W, TICK_LEN),
                            ),
                            egui::pos2(x + LABEL_GAP, top.top() + LABEL_INSET),
                        )
                    }
                    GuideAxis::Horizontal => {
                        let y = on_screen(value);
                        if y < left.top() + THICKNESS || y > left.bottom() {
                            continue;
                        }
                        (
                            egui::Rect::from_min_size(
                                egui::pos2(left.right() - TICK_LEN, y),
                                egui::vec2(TICK_LEN, TICK_W),
                            ),
                            egui::pos2(left.left() + LABEL_INSET, y + LABEL_GAP),
                        )
                    }
                };
                // A tick label near one of the band's own numbers is dropped, so
                // the two never overprint.
                let along = label_at_along(axis, tick.min);
                if let Some((lo, hi)) = ends
                    && [lo, hi]
                        .iter()
                        .any(|e| (on_screen(*e) - along).abs() < EXTENT_LABEL_CLEAR)
                {
                    painter.rect_filled(tick, egui::CornerRadius::ZERO, tick_ink);
                    continue;
                }
                painter.rect_filled(tick, egui::CornerRadius::ZERO, tick_ink);
                // The bar left beside the label's start: on the left bar the 17pt
                // between the inset and the inner hairline.
                let room = bar.right() - label_at.x;
                if let Some(galley) =
                    self.tick_number(&painter, value, step, decimals, room, label_ink)
                {
                    painter.galley(label_at, galley, egui::Color32::PLACEHOLDER);
                }
            }

            // The band's own two numbers, last so they sit over everything.
            if let Some((lo, hi)) = ends {
                for value in [lo, hi] {
                    self.draw_extent_number(&painter, axis, bar, on_screen(value), value);
                }
            }

            // The pointer, marked on both bars.
            if let Some(p) = self.pointer_mark(hover) {
                let along = label_at_along(axis, p);
                let mark = match axis {
                    GuideAxis::Vertical => egui::Rect::from_min_size(
                        egui::pos2(along, bar.top()),
                        egui::vec2(TICK_W, THICKNESS),
                    ),
                    GuideAxis::Horizontal => egui::Rect::from_min_size(
                        egui::pos2(bar.left(), along),
                        egui::vec2(THICKNESS, TICK_W),
                    ),
                };
                painter.rect_filled(mark, egui::CornerRadius::ZERO, POINTER_MARK);
            }
        }
    }

    /// Where the pointer should be marked on the bars, or `None` for a frame that
    /// should not mark it.
    ///
    /// Suppressed during a guide drag, whose chip already carries the number in
    /// that bar. Every other drag keeps it.
    fn pointer_mark(&self, hover: Option<egui::Pos2>) -> Option<egui::Pos2> {
        match self.drag {
            Drag::Guide(_) => None,
            _ => hover,
        }
    }

    /// One of the band's end coordinates, called out over the ordinary ticks.
    ///
    /// Brighter than a tick's number, in the accent's light tier. Drawn even when
    /// the tick lattice has no label there.
    fn draw_extent_number(
        &self,
        painter: &egui::Painter,
        axis: GuideAxis,
        bar: egui::Rect,
        at: f32,
        value: f64,
    ) {
        let font = egui::FontId::proportional(LABEL_SIZE);
        let lay = |text: String| painter.layout_no_wrap(text, font.clone(), color::ACCENT_200);
        let pos = match axis {
            GuideAxis::Vertical => egui::pos2(at + LABEL_GAP, bar.top() + LABEL_INSET),
            GuideAxis::Horizontal => egui::pos2(bar.left() + LABEL_INSET, at + LABEL_GAP),
        };
        // The room across the bar, distinct from running off its end: on the left
        // bar that is 17pt, less than four digits at 7.5pt. The ladder is
        // `tick_number`'s: plain form, then thousands, then nothing, since a
        // sliced `1000` reads as a different number.
        let across = match axis {
            // The top bar reads along its length; its constraint is the end of
            // the bar, tested below, and it has never been short of width.
            GuideAxis::Vertical => f32::INFINITY,
            GuideAxis::Horizontal => bar.right() - pos.x,
        };
        // The `>= 1000.0` guard also stops a fractional coordinate such as `512.37`
        // from becoming `0.5k`, a number rounded past its magnitude. Such a value
        // whose plain form overflows draws nothing, by choice; do not widen the
        // guard to "whenever the plain form does not fit".
        let mut galley = lay(chip_label(value));
        if galley.size().x > across && value.abs() >= 1000.0 {
            let short = lay(chip_label_thousands(value));
            if short.size().x <= across {
                galley = short;
            }
        }
        if galley.size().x > across {
            return;
        }
        // Off the end of its own bar: the selection reaches past the viewport,
        // and a number pinned at the edge would claim the selection ends there.
        if pos.x + galley.size().x > bar.right() || pos.y + galley.size().y > bar.bottom() {
            return;
        }
        painter.galley(pos, galley, egui::Color32::PLACEHOLDER);
    }

    /// The world point the rulers count from.
    ///
    /// The world origin, unless a single frame is selected: then `0` sits on its
    /// leading edge.
    ///
    /// Only a frame, and only on its own: the inspector shows a shape's X/Y in
    /// world space, so re-origining for a shape inside a frame would make the
    /// ruler and the panel disagree.
    fn ruler_origin(&self) -> Point {
        let Some(id) = self.session.selection.single() else {
            return Point::ZERO;
        };
        let is_frame = self
            .session
            .display_node(id)
            .is_some_and(|n| matches!(n.kind(), ondin_core::NodeKind::Artboard { .. }));
        if !is_frame {
            return Point::ZERO;
        }
        self.session
            .preview_world_transform(id)
            .map_or(Point::ZERO, |w| w * Point::ZERO)
    }

    /// The world x and y ranges the selection covers, as `((x0, x1), (y0, y1))`.
    ///
    /// Read from the preview layer, so the band tracks a live drag or resize.
    fn selection_extent(&self) -> Option<((f64, f64), (f64, f64))> {
        let bounds = self
            .session
            .selection
            .ids()
            .iter()
            .filter_map(|id| self.session.preview_world_bounds(*id))
            .reduce(|a, b| a.union(b))?;
        Some((
            (bounds.min_x(), bounds.max_x()),
            (bounds.min_y(), bounds.max_y()),
        ))
    }

    /// The galley for a tick's number, in whichever form fits `room` — or
    /// `None` when none of them do.
    ///
    /// The left bar gives a label 17pt to read across and four digits at 7.5pt
    /// come to 17.25, so the plain number stops fitting around a thousand. It
    /// falls back to the thousands form (`20000` → `20k`) and then to no label.
    ///
    /// Nothing, rather than a truncated number: `20000` clipped reads `2000`.
    fn tick_number(
        &self,
        painter: &egui::Painter,
        value: f64,
        step: f64,
        decimals: usize,
        room: f32,
        ink: egui::Color32,
    ) -> Option<std::sync::Arc<egui::Galley>> {
        let font = egui::FontId::proportional(LABEL_SIZE);
        let lay = |text: String| painter.layout_no_wrap(text, font.clone(), ink);
        let plain = lay(tick_label(value, decimals));
        if plain.size().x <= room {
            return Some(plain);
        }
        // Below a thousand the short form is the long one: a wasted layout.
        if value.abs() >= 1000.0 {
            let short = lay(tick_label_thousands(value, step));
            if short.size().x <= room {
                return Some(short);
            }
        }
        None
    }
}

/// The cursor for dragging a guide of this axis: a horizontal line moves up and
/// down, a vertical one side to side.
fn axis_cursor(axis: GuideAxis) -> egui::CursorIcon {
    match axis {
        GuideAxis::Horizontal => egui::CursorIcon::ResizeVertical,
        GuideAxis::Vertical => egui::CursorIcon::ResizeHorizontal,
    }
}

/// Type on a guide chip: near-black, since the chrome's near-white would
/// contrast too little with the guide's mid-dark default colour.
const CHIP_INK: egui::Color32 = egui::Color32::from_rgb(0x0d, 0x1c, 0x26);

/// The pointer's mark on a ruler bar: full thickness, brighter than a tick and
/// dimmer than an extent number.
///
/// A full-height line rather than a longer tick, so the eye can follow it among
/// the ticks. It carries no number, which would compete with the guide chip.
const POINTER_MARK: egui::Color32 = theme::color::text_a(120);

// --- interaction ------------------------------------------------------------

impl OndinApp {
    /// Guide and ruler handling, ahead of everything else the canvas does with
    /// a press. Returns whether it took the event.
    ///
    /// It runs first because a guide is drawn on top of the artwork. Only the
    /// selection chrome outranks it, and `on_handle` (`canvas::chrome_claims`)
    /// says whether the pointer is on any of that.
    ///
    /// The gesture is claimed on the press and hit-tested at `press_origin`, not
    /// at `drag_started` and the current pointer: egui calls a press a drag only
    /// after several points of travel, by when the pointer is off a
    /// [`GUIDE_PICK_PT`] band or off the bar it was dragged out of.
    pub(crate) fn guide_input(
        &mut self,
        resp: &egui::Response,
        rect: egui::Rect,
        ppp: f32,
        on_handle: bool,
    ) -> bool {
        // Shift only adds a guide to the selection (see `update_guide_drag`).
        let (pressed, down, origin, shift, alt) = resp.ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.press_origin(),
                i.modifiers.shift,
                i.modifiers.alt,
            )
        });

        // Mid-gesture the drag owns the pointer whatever it is over now.
        if let Drag::Guide(_) = &self.drag {
            if let Some(p) = resp.interact_pointer_pos().or(resp.hover_pos()) {
                self.update_guide_drag(p, rect, ppp, alt);
            }
            // Ended by the button coming up, not `drag_stopped`, which never fires
            // for a press that did not travel far enough to count as a drag.
            if !down {
                self.finish_guide_drag();
            }
            return true;
        }

        // Another gesture owns the pointer: a marquee dragged over the ruler stays
        // a marquee.
        if !self.drag.is_none() || on_handle || !pressed || !resp.contains_pointer() {
            return false;
        }
        let Some(p) = origin.or_else(|| resp.interact_pointer_pos()) else {
            return false;
        };

        // Pulling a fresh guide out of a ruler, whatever the tool. The bars keep
        // working while guides are locked (the lock stops existing placements
        // being nudged; a new guide lands locked) or hidden.
        if let Some(axis) = self.ruler_at(p, rect) {
            // Hidden is re-shown on the press, so the line is visible while it is
            // being placed. A locked guide is already on screen.
            if !self.show_guides {
                self.show_guides = true;
                self.session.info("Guides shown");
            }
            self.begin_guide_drag(None, axis, p, rect, ppp);
            return true;
        }

        // The lines answer only the selecting tools; a pen or shape tool aimed at
        // artwork under one should draw there.
        if !self.selecting() {
            return false;
        }
        // Selected on the press, which is also what starts the move.
        if let Some(id) = self.guide_at(p, rect, ppp)
            && let Some(axis) = self.session.doc.guide(id).map(|g| g.axis)
        {
            // Shift adds to the selection and starts no move: guides do not share
            // an axis, so several could not be dragged together.
            if shift {
                self.session.selection.toggle_guide(id);
                self.entered_group = None;
                return true;
            }
            self.session.selection.set_guide(id);
            self.entered_group = None;
            self.begin_guide_drag(Some(id), axis, p, rect, ppp);
            return true;
        }
        false
    }

    /// The guide under a screen position, nearest first — or `None` while the
    /// guides are hidden or locked.
    ///
    /// Hover, cursor, press, drag and discard all reach a guide through here, so
    /// a locked or hidden guide falls out of all of them at once. The measure
    /// overlay goes round it through [`Self::guide_under`] on purpose, which is
    /// why a locked guide still reports a distance.
    pub(crate) fn guide_at(&self, p: egui::Pos2, rect: egui::Rect, ppp: f32) -> Option<GuideId> {
        if !self.guides_pickable() {
            return None;
        }
        self.guide_under(p, rect, ppp)
    }

    /// The guide nearest `p` within [`GUIDE_PICK_PT`], ignoring the lock.
    ///
    /// Split out of [`Self::guide_at`]: acting on a guide keeps the lock check,
    /// but measuring must not, since a locked guide still attracts a dragged shape
    /// (see [`Self::measure_guide`]). Both refuse the ruler band, whose presses
    /// pull out a new guide.
    fn guide_under(&self, p: egui::Pos2, rect: egui::Rect, ppp: f32) -> Option<GuideId> {
        if self.ruler_at(p, rect).is_some() {
            return None;
        }
        // The same `inner` as the paint, so the picked segment is the drawn one.
        let inner = self.canvas_inner(rect);
        let mut best: Option<(f32, GuideId)> = None;
        for guide in self.session.doc.guides() {
            let shown = self.shown_guide(guide);
            let Some(line) = self.guide_line(&shown, rect, inner, ppp) else {
                continue;
            };
            let distance = line.distance_to(p);
            if distance <= GUIDE_PICK_PT && best.is_none_or(|(d, _)| distance < d) {
                best = Some((distance, guide.id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// The guide under the pointer as the measure overlay needs it: its axis, and
    /// the **world** coordinate it pins.
    ///
    /// Its `None`s are `guide_lines`' `None`s, not `guide_at`'s: what may be
    /// measured is what may be snapped to. So a locked guide is measured to
    /// (locking stops changes, and measuring changes nothing), a hidden one is not
    /// ([`Self::measurable_guide`]), and one in a rotated frame has no world x or y
    /// to quote. [`Self::shown_guide`] makes a guide mid-drag measure to where it
    /// looks.
    pub(crate) fn measure_guide(
        &self,
        p: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
    ) -> Option<(crate::measure::Axis, f64)> {
        self.measurable_guide(self.guide_under(p, rect, ppp)?)
    }

    /// [`Self::measure_guide`] for a guide named by id rather than found under the
    /// pointer — what a **selected** guide measures *from*.
    ///
    /// Both overlay directions meet here so they agree about where a guide is.
    ///
    /// `guides_on` is checked here because the by-id path does not pass through
    /// `guide_at`: *Hide guides* does not clear the guide selection (only *Lock
    /// guides* does), so a selected guide can be invisible.
    pub(crate) fn measurable_guide(&self, id: GuideId) -> Option<(crate::measure::Axis, f64)> {
        if !self.guides_on() {
            return None;
        }
        let shown = self.shown_guide(self.session.doc.guide(id)?);
        let at = self.guide_world_coord(shown.axis, shown.owner, shown.position)?;
        let axis = match shown.axis {
            GuideAxis::Vertical => crate::measure::Axis::Vertical,
            GuideAxis::Horizontal => crate::measure::Axis::Horizontal,
        };
        Some((axis, at))
    }

    /// The canvas past both ruler bars — where a guide's line is allowed to be.
    /// Spelled once so the paint and the hit test agree.
    pub(crate) fn canvas_inner(&self, rect: egui::Rect) -> egui::Rect {
        if !self.rulers_on() {
            return rect;
        }
        egui::Rect::from_min_max(
            egui::pos2(rect.left() + THICKNESS, rect.top() + THICKNESS),
            rect.max,
        )
    }

    /// The innermost frame whose **own** box contains a world point, or `None` for
    /// open canvas.
    ///
    /// The containment test goes through the frame's inverse world transform, so a
    /// rotated frame's true rectangle counts, not its axis-aligned box.
    ///
    /// `artboards()` is depth-first in paint order, so the last match is the
    /// innermost frame.
    fn frame_at_world(&self, world: Point) -> Option<NodeId> {
        self.artboards().into_iter().rev().find(|id| {
            let Some(t) = self.session.preview_world_transform(*id) else {
                return false;
            };
            self.session
                .preview_local_box(*id)
                .is_some_and(|b| b.contains(t.inverse() * world))
        })
    }

    fn begin_guide_drag(
        &mut self,
        id: Option<GuideId>,
        axis: GuideAxis,
        p: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
    ) {
        let held = id.and_then(|id| self.session.doc.guide(id)).copied();
        // An existing guide keeps its scope until the pointer leaves it; a new one
        // takes what is under the press, corrected on the first move.
        let owner = match held {
            Some(g) => g.owner,
            None => self.frame_at_world(self.to_world(p, rect, ppp)),
        };
        // The world offset from the pointer to the line (`GuideDrag::grab`).
        let grab = held
            .and_then(|g| self.guide_grab(&g, p, rect, ppp))
            .unwrap_or(Vec2::ZERO);
        self.drag = Drag::Guide(GuideDrag {
            id,
            axis,
            owner,
            // Unsnapped at the press, so pickup does not jump to a nearby edge.
            position: self.guide_coord(axis, owner, p, rect, ppp, grab, false),
            grab,
            // A new guide starts inside the ruler; releasing it there cancels.
            discarding: id.is_none(),
            // The same off-the-bar test `update_guide_drag` keeps applying, not
            // `id.is_some()`.
            left_ruler: self.ruler_at(p, rect).is_none(),
            // Read live from `update_guide_drag`; never set on a new guide.
            copying: false,
            // Latched for `GuideDrag::rulers_shown`'s reason.
            rulers_shown: self.rulers_on(),
        });
    }

    /// The world vector from the pointer to the nearest point on a guide's line.
    ///
    /// Built by taking the pointer into the guide's space, replacing the pinned
    /// coordinate and coming back out: exact for any transform. It is a world
    /// vector so it survives the guide's space changing mid-drag.
    fn guide_grab(&self, guide: &Guide, p: egui::Pos2, rect: egui::Rect, ppp: f32) -> Option<Vec2> {
        let pointer = self.to_world(p, rect, ppp);
        let local = self.guide_local(guide.owner, pointer)?;
        let on_line = match guide.axis {
            GuideAxis::Horizontal => Point::new(local.x, guide.position),
            GuideAxis::Vertical => Point::new(guide.position, local.y),
        };
        Some(self.guide_world(guide.owner, on_line)? - pointer)
    }

    /// Shift does not suppress the snap: it is a constraint, a guide has one axis
    /// and nothing to constrain, and snapping is the Snap menu's to switch off.
    /// Alt places a copy and leaves the original, read live so it can be taken
    /// or released mid-drag.
    fn update_guide_drag(&mut self, p: egui::Pos2, rect: egui::Rect, ppp: f32, alt: bool) {
        let Drag::Guide(d) = &self.drag else { return };
        let (axis, grab) = (d.axis, d.grab);
        // Only an existing guide can be copied.
        let copying = alt && d.id.is_some();
        // The scope follows the pointer; the held point is in world space, so
        // nothing jumps when the space the number is read in changes.
        let owner = self.frame_at_world(self.to_world(p, rect, ppp) + grab);
        let position = self.guide_coord(axis, owner, p, rect, ppp, grab, true);
        // Only the guide's own ruler discards it. Both this and `off_rulers` use
        // the latched `d.rulers_shown`, not `rulers_on()`: toggling the bars
        // mid-drag would otherwise turn cancel into create, or announce a discard
        // for a guide never dragged back to a bar.
        let over_own_ruler = d.rulers_shown && ruler_at(rect, p) == Some(axis);
        // Off both bars, the corner counting as on them: a pure geometry test
        // against the layout the press saw, true when the bars were never shown.
        let off_rulers = !d.rulers_shown || ruler_at(rect, p).is_none();
        if let Drag::Guide(d) = &mut self.drag {
            d.owner = owner;
            d.position = position;
            d.discarding = over_own_ruler;
            // Latched, never cleared: coming back to the bar is what the
            // announcement is for.
            d.left_ruler |= off_rulers;
            d.copying = copying;
        }
    }

    /// The coordinate a pointer position puts a guide of `axis` at, in `owner`'s
    /// space, offset by `grab` and snapped onto nearby shape edges unless `raw`.
    ///
    /// `grab` is added before the snap, so the line is snapped, not the hand. The
    /// snap happens in world space, where the targets are, so a guide in a rotated
    /// frame ([`Self::guide_world_coord`] has no coordinate) does not snap; it
    /// still draws, moves and saves (§5.5, §9.4).
    #[allow(clippy::too_many_arguments)]
    fn guide_coord(
        &self,
        axis: GuideAxis,
        owner: Option<NodeId>,
        p: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
        grab: Vec2,
        snap: bool,
    ) -> f64 {
        // The held world point is space-independent, so a scope change re-reads it.
        let target = self.to_world(p, rect, ppp) + grab;
        let raw = self
            .guide_local(owner, target)
            .map(|local| ondin_core::guide_coord_of(axis, local))
            .unwrap_or_else(|| ondin_core::guide_coord_of(axis, target));
        if !snap {
            return raw;
        }
        let Some(world) = self.guide_world_coord(axis, owner, raw) else {
            return raw;
        };
        // Nothing is excluded: a guide is not a node.
        let snapped = crate::snap::snap_guide(
            world,
            match axis {
                GuideAxis::Horizontal => crate::measure::Axis::Horizontal,
                GuideAxis::Vertical => crate::measure::Axis::Vertical,
            },
            &self.snapping(&[]),
        );
        if snapped == world {
            return raw;
        }
        // Through the matrix, not by adding the difference: a mirrored frame
        // flips the offset's sign.
        self.guide_local(owner, on_axis(axis, snapped))
            .map(|local| ondin_core::guide_coord_of(axis, local))
            .unwrap_or(raw)
    }

    /// End the gesture: one transaction, or none at all (invariant 5).
    fn finish_guide_drag(&mut self) {
        let Drag::Guide(d) = std::mem::replace(&mut self.drag, Drag::None) else {
            return;
        };
        match (d.id, d.discarding) {
            // Pulled out and dropped on the canvas: it exists now.
            (None, false) => {
                let guide = Guide {
                    id: GuideId(self.session.ids.mint()),
                    axis: d.axis,
                    position: d.position,
                    color: None,
                    owner: d.owner,
                };
                if self
                    .session
                    .commit(Transaction(vec![Operation::AddGuide { guide }]))
                {
                    // Not selected while locked: it would be selected but
                    // unselectable (`guide_at`).
                    if !self.lock_guides {
                        self.session.selection.set_guide(guide.id);
                    }
                }
            }
            // Pulled out and put straight back: nothing is written to history.
            (None, true) => {}
            // Dropped on the ruler while copying: the copy is thrown away, not
            // `remove_guide`. The bin cursor and fade need no term of their own;
            // they describe the line in flight, which for a clone is the copy.
            (Some(_), true) if d.copying => {}
            (Some(id), true) => self.remove_guide(id),
            // Alt held: the original stays and the copy lands where dragged.
            (Some(id), false) if d.copying => {
                let Some(from) = self.session.doc.guide(id).copied() else {
                    return;
                };
                let guide = Guide {
                    id: GuideId(self.session.ids.mint()),
                    axis: d.axis,
                    position: d.position,
                    color: from.color,
                    owner: d.owner,
                };
                if self
                    .session
                    .commit(Transaction(vec![Operation::AddGuide { guide }]))
                {
                    // The copy becomes the selection, as for a layer's Alt-drag.
                    if !self.lock_guides {
                        self.session.selection.set_guide(guide.id);
                    }
                    self.session.info("Guide copied");
                }
            }
            (Some(id), false) => {
                // A press that never moved is a selection, not an edit. A scope
                // change counts as moved: without that term it commits nothing.
                // `EditorSession::commit_inner` already drops a no-op transaction,
                // so this only avoids building the op.
                let was = self.session.doc.guide(id).copied();
                let moved = was.is_some_and(|g| g.position != d.position || g.owner != d.owner);
                if !moved {
                    return;
                }
                // One op, not two: the position is read in the owner's space, so
                // two ops would pass through a state where the number means
                // something else (`Operation::SetGuideScope`).
                self.session
                    .commit(Transaction(vec![Operation::SetGuideScope {
                        id,
                        owner: d.owner,
                        position: d.position,
                    }]));
            }
        }
    }

    /// Move every selected guide by an arrow-key nudge.
    ///
    /// Only the component on each guide's own axis is used. One transaction, so a
    /// nudge over three guides is one undo, and a burst of taps merges into one
    /// step.
    ///
    /// The step is applied in the guide's own space, unscaled (a transform
    /// carries no scale, §5.6); a skewed frame is not corrected for.
    ///
    /// It refuses while the guides are hidden: *Hide guides* keeps the guide
    /// selection (`measurable_guide` relies on that; only *Lock guides* clears
    /// it), so without the check arrows would move and dirty an invisible guide.
    /// `guides_on`, not `guides_pickable`: the lock already clears the selection.
    pub(crate) fn nudge_guides(&mut self, delta: ondin_core::kurbo::Vec2) {
        if !self.guides_on() {
            return;
        }
        let ops: Vec<Operation> = self
            .session
            .selection
            .guides()
            .iter()
            .filter_map(|id| self.session.doc.guide(*id))
            .filter_map(|guide| {
                let step = match guide.axis {
                    GuideAxis::Horizontal => delta.y,
                    GuideAxis::Vertical => delta.x,
                };
                (step != 0.0).then(|| Operation::SetGuidePosition {
                    id: guide.id,
                    position: guide.position + step,
                })
            })
            .collect();
        if !ops.is_empty() {
            // A burst of taps is one undo step, as in the layer arm of
            // `OndinApp::nudge`; its own verb, so it never merges into a layer nudge.
            self.session.commit_run(Transaction(ops), "nudge-guides");
        }
    }

    /// Delete one guide. Shared by the Delete key, the drag-back-to-the-ruler
    /// gesture, and the inspector's own Remove.
    pub(crate) fn remove_guide(&mut self, id: GuideId) {
        if self
            .session
            .commit(Transaction(vec![Operation::RemoveGuide { id }]))
        {
            self.session.info("Guide removed");
        }
    }

    /// Delete every guide, as one undo step.
    pub(crate) fn remove_all_guides(&mut self) {
        let ops: Vec<Operation> = self
            .session
            .doc
            .guides()
            .iter()
            .map(|g| Operation::RemoveGuide { id: g.id })
            .collect();
        let n = ops.len();
        if n > 0 && self.session.commit(Transaction(ops)) {
            self.session.info(format!("Removed {n} guide(s)"));
        }
    }

    /// The ruler guides as coordinates for [`crate::snap`], split by axis.
    ///
    /// All of them, not just those in view: a document has a handful of guides,
    /// and the tolerance check discards the far ones. Read through
    /// [`Self::shown_guide`], so a dragged guide snaps against where it looks.
    ///
    /// A scoped guide is projected to world here (`snap.rs` knows nothing of the
    /// document); one in a rotated frame contributes nothing
    /// ([`Self::guide_world_coord`]). Hidden guides contribute nothing either: a
    /// shape jumping to an invisible line reads as a bug.
    pub(crate) fn guide_lines(&self) -> crate::snap::GuideLines {
        let mut out = crate::snap::GuideLines::default();
        if !self.guides_on() {
            return out;
        }
        for guide in self.session.doc.guides() {
            let shown = self.shown_guide(guide);
            let Some(at) = self.guide_world_coord(shown.axis, shown.owner, shown.position) else {
                continue;
            };
            match shown.axis {
                GuideAxis::Vertical => out.x.push(at),
                GuideAxis::Horizontal => out.y.push(at),
            }
        }
        out
    }

    /// The cursor over a ruler bar, or over a guide being dragged out of one.
    ///
    /// Answered for every tool, since the ruler claims the press from every tool
    /// ([`Self::guide_input`]).
    ///
    /// A drag about to throw the guide away shows the bin cursor; `discarding` is
    /// computed once ([`Self::update_guide_drag`]), so the cursor cannot disagree
    /// with the fade.
    pub(crate) fn ruler_cursor(
        &self,
        p: egui::Pos2,
        rect: egui::Rect,
    ) -> Option<(egui::CursorIcon, Option<Bitmap>)> {
        if self.discarding_guide() {
            // `NoDrop` is the stock fallback; it says the wrong thing alone (the
            // drop is accepted, but destructive), hence the bin.
            return Some((egui::CursorIcon::NoDrop, Some(Bitmap::Discard)));
        }
        let axis = match &self.drag {
            Drag::Guide(d) => d.axis,
            _ if self.rulers_on() => ruler_at(rect, p)?,
            _ => return None,
        };
        Some((axis_cursor(axis), None))
    }

    /// Whether the guide in flight should *say* that letting go throws it away.
    ///
    /// [`GuideDrag::announces_discard`], not the raw `discarding` flag, which is
    /// set at the press of a guide still over its ruler.
    pub(crate) fn discarding_guide(&self) -> bool {
        matches!(&self.drag, Drag::Guide(d) if d.announces_discard())
    }

    /// The cursor over a guide *line*. Select only, matching what a press
    /// there would do.
    pub(crate) fn guide_cursor(
        &self,
        p: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
    ) -> Option<egui::CursorIcon> {
        let id = self.guide_at(p, rect, ppp)?;
        let guide = self.session.doc.guide(id)?;
        Some(axis_cursor(guide.axis))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The step is 100 at the design's 51.94% zoom.
    #[test]
    fn the_step_matches_the_design_at_the_designs_zoom() {
        assert_eq!(nice_step(0.5194), 100.0);
    }

    /// The step stays on the 1/2/5 ladder, leaving at least `MIN_LABEL_PX`.
    #[test]
    fn the_step_is_the_smallest_one_five_ladder_rung_that_fits() {
        for (points_per_unit, want) in [
            (0.01, 5000.0),
            (0.1, 500.0),
            (1.0, 50.0),
            // 25 is not a rung and 20 would crowd: rounds up.
            (2.0, 50.0),
            (4.0, 20.0),
            (10.0, 5.0),
            (100.0, 0.5),
        ] {
            let got = nice_step(points_per_unit);
            assert!(
                (got - want).abs() < want * 1e-9,
                "at {points_per_unit}: {got}, want {want}"
            );
        }
        // Whatever the zoom, labels never crowd closer than the minimum.
        for i in 1..2000 {
            let ppu = i as f64 / 20.0;
            let gap = nice_step(ppu) * ppu;
            assert!(gap >= MIN_LABEL_PX, "labels {gap}pt apart at {ppu}");
        }
    }

    /// Labels get decimals when zoomed in, and the origin is never `-0`.
    #[test]
    fn labels_carry_enough_decimals_and_never_read_minus_zero() {
        assert_eq!(tick_label(300.0, label_decimals(100.0)), "300");
        assert_eq!(tick_label(-100.0, label_decimals(100.0)), "-100");
        assert_eq!(tick_label(0.5, label_decimals(0.5)), "0.5");
        assert_eq!(tick_label(0.25, label_decimals(0.05)), "0.25");
        assert_eq!(tick_label(-0.0, label_decimals(100.0)), "0");
    }

    /// The chip quotes the exact position, whole numbers staying whole.
    #[test]
    fn a_chip_shows_the_position_it_actually_has() {
        assert_eq!(chip_label(332.0), "332");
        assert_eq!(chip_label(12.5), "12.5");
        assert_eq!(chip_label(-48.25), "-48.25");
        assert_eq!(chip_label(-0.0), "0");
        // Beyond two decimals it rounds.
        assert_eq!(chip_label(1.0 / 3.0), "0.33");
    }

    /// An egui context with the theme's fonts loaded, for tests that measure type.
    fn shaped() -> egui::Context {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        ctx
    }

    fn measure(ctx: &egui::Context, text: &str, size: f32) -> egui::Vec2 {
        ctx.fonts_mut(|f| {
            f.layout_no_wrap(
                text.to_owned(),
                egui::FontId::proportional(size),
                egui::Color32::WHITE,
            )
            .size()
        })
    }

    fn width(ctx: &egui::Context, text: &str, size: f32) -> f32 {
        measure(ctx, text, size).x
    }

    /// Four digits overflow the left bar's 17pt, and every thousands form fits.
    #[test]
    fn a_long_tick_number_has_a_short_form_that_fits_the_left_bar() {
        let ctx = shaped();
        let room = THICKNESS - LABEL_INSET;
        // The measurements the fallback exists for.
        assert!(
            width(&ctx, "-100", LABEL_SIZE) <= room,
            "the design's widest"
        );
        assert!(
            width(&ctx, "1000", LABEL_SIZE) > room,
            "four digits do not fit"
        );

        // Every long value the ruler can reach has a short form that does.
        for (value, step, want) in [
            (1000.0, 100.0, "1k"),
            (-1100.0, 100.0, "-1.1k"),
            (1500.0, 500.0, "1.5k"),
            (10000.0, 1000.0, "10k"),
            (20000.0, 5000.0, "20k"),
            (-45000.0, 5000.0, "-45k"),
        ] {
            let short = tick_label_thousands(value, step);
            assert_eq!(short, want);
            let w = width(&ctx, &short, LABEL_SIZE);
            assert!(w <= room, "{short:?} is {w}pt in a {room}pt bar");
        }
    }

    /// The band's thousands form fits the left bar, and `999` and `1920` still fit
    /// in plain form (controls: the choice is by measured width, not by value).
    /// This pins the label widths only; drawing is pinned by
    /// `the_left_bars_band_says_where_the_selection_ends`.
    #[test]
    fn the_selection_bands_number_has_a_short_form_that_fits_the_left_bar() {
        let ctx = shaped();
        let room = THICKNESS - LABEL_INSET;

        assert!(
            width(&ctx, &chip_label(999.0), LABEL_SIZE) <= room,
            "control: 999 fits and must keep its plain form"
        );
        assert!(
            width(&ctx, &chip_label(1080.0), LABEL_SIZE) > room,
            "1080 does not fit, which is why the band went blank"
        );
        assert!(
            width(&ctx, &chip_label(1920.0), LABEL_SIZE) <= room,
            "control: 1920 fits — the ladder must switch on measured width, not \
             on the value being over a thousand"
        );

        for (value, want) in [
            (1000.0, "1k"),
            (1080.0, "1.1k"),
            (-1080.0, "-1.1k"),
            (1024.75, "1k"),
            (20000.0, "20k"),
        ] {
            let short = chip_label_thousands(value);
            assert_eq!(short, want, "the band's thousands form for {value}");
            let w = width(&ctx, &short, LABEL_SIZE);
            assert!(w <= room, "{short:?} is {w}pt in a {room}pt bar");
        }
    }

    /// The left bar's band paints the ladder's three rungs: plain `999`, thousands
    /// `1.1k` for `1080`, and nothing when neither fits. It collects every
    /// `Shape::Text`, so the closure must call only `draw_extent_number`.
    ///
    /// flip: delete the thousands arm of `draw_extent_number` → fails at the
    /// `1080` assertion.
    #[test]
    fn the_left_bars_band_says_where_the_selection_ends() {
        let ctx = shaped();
        let app_ctx = egui::Context::default();
        crate::theme::install(&app_ctx);
        let _ = app_ctx.run_ui(Default::default(), |_| {});
        let mut app = OndinApp::headless(&app_ctx);
        app.canvas_px = (800, 600);
        let _ = &mut app;

        let bar = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(THICKNESS, 600.0));
        let painted = |value: f64| -> Vec<String> {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::pos2(0.0, 0.0),
                        egui::vec2(800.0, 600.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    app.draw_extent_number(ui.painter(), GuideAxis::Horizontal, bar, 200.0, value);
                },
            );
            out.shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::epaint::Shape::Text(t) => Some(t.galley.text().to_owned()),
                    _ => None,
                })
                .collect()
        };

        assert_eq!(
            painted(999.0),
            vec!["999".to_string()],
            "a number that fits keeps its plain form"
        );
        assert_eq!(
            painted(1080.0),
            vec!["1.1k".to_string()],
            "and one that does not falls back rather than vanishing — this is the \
             1920x1080 frame whose band read `0` and then nothing"
        );
        assert!(
            painted(1_234_567_890.0).is_empty(),
            "and when neither form fits, nothing — a number sliced at the \
             hairline is a different number (§15 D33)"
        );
    }

    /// A chip is sized from its own galley, and a three-digit chip stays 20pt.
    #[test]
    fn a_guide_chip_contains_its_own_number() {
        let ctx = shaped();
        let design = width(&ctx, "332", CHIP_TEXT);
        assert!(
            (design + CHIP_PAD_X * 2.0).max(THICKNESS) == THICKNESS,
            "the design's own chip must still be {THICKNESS}pt, not {design}+pad"
        );
        for position in [0.0, 12.5, 332.0, -100.0, 1000.0, -1024.75, 20000.0] {
            let text = chip_label(position);
            let w = width(&ctx, &text, CHIP_TEXT);
            let chip = (w + CHIP_PAD_X * 2.0).max(THICKNESS);
            assert!(chip >= THICKNESS, "{position} shrank the chip");
            assert!(
                chip - w >= CHIP_PAD_X * 2.0 - 1e-3,
                "{text:?} is {w}pt in a {chip}pt chip"
            );
        }
        let height = measure(&ctx, "-1024.75", CHIP_TEXT).y;
        assert!(
            height <= CHIP_THICKNESS,
            "{height}pt of type in a {CHIP_THICKNESS}pt chip"
        );
    }

    /// The band spans its axis and the bar's thickness, clips rather than drops
    /// off screen, and accepts its ends either way round.
    #[test]
    fn the_selection_band_spans_its_axis_and_clips_to_the_bar() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let top = egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), THICKNESS));
        let left = egui::Rect::from_min_max(rect.min, egui::pos2(THICKNESS, rect.bottom()));

        let band = extent_band(GuideAxis::Vertical, top, 110.0, 543.0).unwrap();
        assert_eq!(band.left(), 110.0);
        assert_eq!(band.right(), 543.0);
        assert_eq!(
            (band.top(), band.bottom()),
            (0.0, THICKNESS),
            "full thickness"
        );

        let band = extent_band(GuideAxis::Horizontal, left, 130.0, 432.0).unwrap();
        assert_eq!((band.top(), band.bottom()), (130.0, 432.0));
        assert_eq!((band.left(), band.right()), (0.0, THICKNESS));

        // Ends the other way round describe the same band.
        assert_eq!(
            extent_band(GuideAxis::Vertical, top, 543.0, 110.0),
            extent_band(GuideAxis::Vertical, top, 110.0, 543.0)
        );

        // Running off both edges: clipped to the bar, not dropped.
        let wide = extent_band(GuideAxis::Vertical, top, -400.0, 2000.0).unwrap();
        assert_eq!((wide.left(), wide.right()), (0.0, 800.0));

        // Entirely off screen: nothing, not a sliver at the edge.
        assert!(extent_band(GuideAxis::Vertical, top, -900.0, -400.0).is_none());
        assert!(extent_band(GuideAxis::Vertical, top, 1200.0, 1600.0).is_none());
        // And a zero-width selection has no band to draw.
        assert!(extent_band(GuideAxis::Vertical, top, 300.0, 300.0).is_none());
    }

    /// The corner square belongs to the left ruler — the one painted over it.
    #[test]
    fn the_rulers_claim_their_bands_and_the_left_one_claims_the_corner() {
        let rect = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(800.0, 600.0));
        let at = |x, y| ruler_at(rect, egui::pos2(x, y));
        assert_eq!(at(105.0, 55.0), Some(GuideAxis::Vertical), "corner");
        assert_eq!(at(400.0, 55.0), Some(GuideAxis::Horizontal), "top bar");
        assert_eq!(at(105.0, 400.0), Some(GuideAxis::Vertical), "left bar");
        assert_eq!(at(400.0, 400.0), None, "open canvas");
        assert_eq!(at(0.0, 0.0), None, "outside the canvas entirely");
    }
}

#[cfg(test)]
mod guide_geometry_tests {
    use super::*;
    use ondin_core::kurbo::Rect;

    fn line(axis: GuideAxis, a: (f32, f32), b: (f32, f32)) -> GuideLine {
        GuideLine {
            axis,
            a: egui::pos2(a.0, a.1),
            b: egui::pos2(b.0, b.1),
            label: 0.0,
            extends: false,
        }
    }

    /// A guide is picked along its drawn segment, not the infinite line.
    #[test]
    fn a_guide_is_picked_along_its_segment_and_not_past_its_ends() {
        let horizontal = line(GuideAxis::Horizontal, (100.0, 200.0), (300.0, 200.0));
        assert_eq!(horizontal.distance_to(egui::pos2(200.0, 200.0)), 0.0);
        assert_eq!(horizontal.distance_to(egui::pos2(200.0, 204.0)), 4.0);
        // Past the end the distance grows along the axis too.
        assert_eq!(horizontal.distance_to(egui::pos2(360.0, 200.0)), 60.0);
        assert!(
            horizontal.distance_to(egui::pos2(360.0, 200.0)) > GUIDE_PICK_PT,
            "a click 60pt past the end of a scoped guide must miss it"
        );
        assert!(horizontal.distance_to(egui::pos2(300.0, 200.0)) <= GUIDE_PICK_PT);

        // The same for a vertical and a leaning line.
        let vertical = line(GuideAxis::Vertical, (400.0, 100.0), (400.0, 500.0));
        assert_eq!(vertical.distance_to(egui::pos2(405.0, 300.0)), 5.0);
        assert!(vertical.distance_to(egui::pos2(400.0, 40.0)) > GUIDE_PICK_PT);
        let leaning = line(GuideAxis::Horizontal, (0.0, 0.0), (100.0, 100.0));
        assert!(leaning.distance_to(egui::pos2(50.0, 50.0)) < 1e-4);
        assert!((leaning.distance_to(egui::pos2(100.0, 0.0)) - 70.710_68).abs() < 1e-3);
    }

    /// The chip's along-bar coordinate is the segment's midpoint: the guide's own
    /// coordinate when axis-aligned.
    #[test]
    fn the_chip_sits_on_the_guides_coordinate_wherever_that_is_one_number() {
        let horizontal = line(GuideAxis::Horizontal, (100.0, 332.0), (900.0, 332.0));
        assert_eq!(horizontal.chip_along(), 332.0, "exact for an upright guide");
        let vertical = line(GuideAxis::Vertical, (48.5, 20.0), (48.5, 600.0));
        assert_eq!(vertical.chip_along(), 48.5);
        // A rotated frame's guide has no single coordinate: the midpoint.
        let leaning = line(GuideAxis::Horizontal, (100.0, 300.0), (300.0, 400.0));
        assert_eq!(leaning.chip_along(), 350.0);
    }

    /// A guide off screen is culled; one crossing it with both ends outside is
    /// not (what a per-endpoint test gets wrong).
    #[test]
    fn a_guide_is_culled_only_when_none_of_it_is_in_view() {
        let inner = egui::Rect::from_min_max(egui::pos2(20.0, 20.0), egui::pos2(800.0, 600.0));
        let visible = line(GuideAxis::Horizontal, (100.0, 300.0), (400.0, 300.0));
        assert!(in_view(inner, visible).is_some());

        let above = line(GuideAxis::Horizontal, (100.0, -50.0), (400.0, -50.0));
        assert!(in_view(inner, above).is_none());
        let beside = line(GuideAxis::Vertical, (900.0, 100.0), (900.0, 500.0));
        assert!(in_view(inner, beside).is_none());

        // Both ends outside, crossing the middle: kept.
        let across = line(GuideAxis::Horizontal, (-400.0, 300.0), (1600.0, 300.0));
        assert!(
            in_view(inner, across).is_some(),
            "a guide longer than the viewport must not vanish"
        );
        let diagonal = line(GuideAxis::Horizontal, (-100.0, -100.0), (900.0, 700.0));
        assert!(in_view(inner, diagonal).is_some());
    }

    /// The extension runs from each end of the frame's span out to the canvas
    /// edge, and produces nothing on a side where the frame already reaches it.
    #[test]
    fn the_dotted_extension_fills_the_canvas_either_side_of_the_frame() {
        let inner = egui::Rect::from_min_max(egui::pos2(20.0, 20.0), egui::pos2(800.0, 600.0));
        let runs = extension_ends(
            GuideAxis::Horizontal,
            inner,
            egui::pos2(300.0, 250.0),
            egui::pos2(500.0, 250.0),
        );
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0], (egui::pos2(20.0, 250.0), egui::pos2(300.0, 250.0)));
        assert_eq!(
            runs[1],
            (egui::pos2(500.0, 250.0), egui::pos2(800.0, 250.0))
        );

        // Ends the other way round (a mirrored frame) give the same runs.
        let mirrored = extension_ends(
            GuideAxis::Horizontal,
            inner,
            egui::pos2(500.0, 250.0),
            egui::pos2(300.0, 250.0),
        );
        assert_eq!(mirrored[0].0.x, 20.0);
        assert_eq!(mirrored[1].1.x, 800.0);

        // A frame spanning the canvas leaves no run.
        let full = extension_ends(
            GuideAxis::Vertical,
            inner,
            egui::pos2(400.0, 20.0),
            egui::pos2(400.0, 600.0),
        );
        assert!(full.is_empty());

        // A gap too small for one dash is dropped, not drawn as a stub.
        let tight = extension_ends(
            GuideAxis::Vertical,
            inner,
            egui::pos2(400.0, 21.0),
            egui::pos2(400.0, 599.0),
        );
        assert!(tight.is_empty(), "{tight:?}");
    }

    /// `upright` admits mirrors (still square with the world) and refuses
    /// rotations and skews; it gates both attraction and the extension.
    #[test]
    fn upright_admits_mirrors_and_refuses_rotations_and_skews() {
        assert!(upright(Affine::IDENTITY));
        assert!(upright(Affine::translate((120.0, -40.0))));
        assert!(
            upright(Affine::FLIP_X),
            "a mirrored frame is still axis-aligned"
        );
        assert!(upright(Affine::FLIP_Y));
        assert!(upright(Affine::translate((10.0, 10.0)) * Affine::FLIP_X));

        assert!(!upright(Affine::rotate(0.3)));
        assert!(!upright(ondin_core::build::skew_x(0.2)));
        // A quarter turn is declined on purpose: a horizontal guide would pin a
        // world x and need filing under the other axis.
        assert!(!upright(Affine::rotate(std::f64::consts::FRAC_PI_2)));
        // A turn and a turn back is upright again (the tolerance).
        assert!(upright(Affine::rotate(0.4) * Affine::rotate(-0.4)));
    }

    /// A guide's span is the frame's box along its axis and its own position
    /// across it, unclamped, so an offset outside a shrunk frame keeps its number.
    #[test]
    fn a_guides_span_crosses_its_frame_and_is_not_clamped_to_it() {
        let box_ = Rect::new(0.0, 0.0, 400.0, 300.0);
        let (a, b) = ondin_core::guide_span(GuideAxis::Horizontal, 120.0, box_);
        assert_eq!((a.x, a.y), (0.0, 120.0));
        assert_eq!((b.x, b.y), (400.0, 120.0));

        let (a, b) = ondin_core::guide_span(GuideAxis::Vertical, 250.0, box_);
        assert_eq!((a.x, a.y), (250.0, 0.0));
        assert_eq!((b.x, b.y), (250.0, 300.0));

        // Past the bottom of the frame: still 500.
        let (a, b) = ondin_core::guide_span(GuideAxis::Horizontal, 500.0, box_);
        assert_eq!(a.y, 500.0);
        assert_eq!(b.y, 500.0);
        assert_eq!((a.x, b.x), (0.0, 400.0));
    }

    /// How much of each device pixel a line of `width` points centred at `at` covers.
    fn coverage(at: f32, ppp: f32, width: f32) -> Vec<f32> {
        let centre = at * ppp;
        let (lo, hi) = (centre - width * ppp / 2.0, centre + width * ppp / 2.0);
        let mut out = Vec::new();
        let mut p = lo.floor();
        while p < hi - 1e-6 {
            out.push((p + 1.0).min(hi) - p.max(lo));
            p += 1.0;
        }
        out
    }

    /// Both guide weights keep a fully covered device pixel at every ordinary
    /// display scale, and the selected weight has more solid pixels than the
    /// resting one. Asserts the core, not crispness: a fractional device width
    /// cannot put both edges on the grid.
    #[test]
    fn the_two_guide_weights_keep_a_solid_core_at_every_scale() {
        for ppp in [1.0f32, 1.25, 1.5, 1.75, 2.0] {
            let mut cores = Vec::new();
            for width in [1.0f32, 2.0] {
                let at = snap_across_axis(100.3, ppp, width);
                let cov = coverage(at, ppp, width);
                let core = cov.iter().copied().fold(0.0f32, f32::max);
                assert!(
                    core > 0.999,
                    "ppp {ppp}, {width}pt: no fully covered pixel — coverage {cov:?}. \
                     A guide with no solid core is the grey smear, not a hairline."
                );
                cores.push(cov.iter().filter(|c| **c > 0.999).count());
            }
            assert!(
                cores[1] > cores[0],
                "ppp {ppp}: selected covers {} solid pixel(s) against the resting \
                 guide's {} — the two weights are indistinguishable here",
                cores[1],
                cores[0]
            );
        }
    }

    /// The guide's own colour at rest and on hover, the selection blue while
    /// picked, even over a recoloured guide (deliberate).
    #[test]
    fn a_selected_guide_takes_the_selection_blue_over_its_own_colour() {
        let red = ondin_core::DEFAULT_GUIDE_COLOR;
        let green = ondin_core::peniko::Color::from_rgba8(0x2d, 0xa3, 0x33, 255);
        assert_eq!(guide_ink(red, false, false), crate::ui::color_to_egui(red));
        assert_eq!(
            guide_ink(green, false, false),
            crate::ui::color_to_egui(green),
            "an unselected guide keeps whatever colour it was given"
        );
        assert_eq!(guide_ink(red, true, false), color::SELECT);
        assert_eq!(
            guide_ink(green, true, false),
            color::SELECT,
            "the blue overrides the guide's own colour while it is selected"
        );
    }

    /// A guide about to be discarded is faded though drawn as selected, because
    /// the fade is applied after the state chooses the ink.
    #[test]
    fn a_discarding_guide_is_faded_even_though_it_is_drawn_as_selected() {
        let red = ondin_core::DEFAULT_GUIDE_COLOR;
        let held = guide_ink(red, true, false);
        let discarding = guide_ink(red, true, true);
        assert_ne!(
            held, discarding,
            "a guide over its own ruler must not look like one over the canvas"
        );
        assert!(
            discarding.a() < held.a(),
            "the discarding guide should be the fainter of the two: {discarding:?} \
             against {held:?}"
        );

        // Fading the colour first and then choosing the ink loses the fade.
        let faded_first = guide_ink(red.multiply_alpha(DISCARD_ALPHA), true, false);
        assert_eq!(
            faded_first, held,
            "if this ever differs, fading before the state choice has started \
             working and the ordering comment on `guide_ink` is wrong"
        );
    }

    /// The point-width and rounded-device parity rules each fail at 150% or 175%
    /// where flooring the device width keeps the resting guide solid, so swapping
    /// `floor` for the tidier `round` breaks a hairline only at those scales.
    #[test]
    fn flooring_the_device_width_is_what_keeps_the_resting_guide_solid() {
        let by_point_width = |v: f32, ppp: f32, width: f32| {
            let s = (v * ppp).round() / ppp;
            match width as i32 % 2 == 1 {
                true => s + 0.5 / ppp,
                false => s,
            }
        };
        let by_rounded_device = |v: f32, ppp: f32, width: f32| {
            let s = (v * ppp).round() / ppp;
            match (width * ppp).round() as i32 % 2 == 1 {
                true => s + 0.5 / ppp,
                false => s,
            }
        };
        let core = |at: f32, ppp: f32, width: f32| {
            coverage(at, ppp, width)
                .iter()
                .copied()
                .fold(0.0f32, f32::max)
        };

        // 150%: the point-width test fringes the selected guide's 3 pixels over 4.
        let ppp = 1.5;
        assert_eq!(coverage(by_point_width(100.3, ppp, 2.0), ppp, 2.0).len(), 4);
        assert_eq!(
            coverage(snap_across_axis(100.3, ppp, 2.0), ppp, 2.0).len(),
            3
        );

        // The rounded-device test takes the core off the resting one.
        assert!(
            core(by_rounded_device(100.3, ppp, 1.0), ppp, 1.0) < 0.8,
            "rounding used to leave the 1pt guide with no solid pixel; if this \
             passes, the two rules now agree and the comment on \
             `snap_across_axis` is wrong"
        );
        assert!(core(snap_across_axis(100.3, ppp, 1.0), ppp, 1.0) > 0.999);

        let ppp = 1.75;
        assert!(core(by_rounded_device(100.3, ppp, 1.0), ppp, 1.0) < 0.9);
        assert!(core(snap_across_axis(100.3, ppp, 1.0), ppp, 1.0) > 0.999);
    }
}

#[cfg(test)]
mod guide_grab_tests {
    //! Guide drags on a headless app: pressing a guide must not move it
    //! (`GuideDrag::grab`), and the commit table of `finish_guide_drag`.
    use super::*;
    use crate::app::OndinApp;

    /// A headless app with an 800 × 600 canvas and one guide, plus `rect`/`ppp`.
    fn app_with_guide(
        ctx: &egui::Context,
        axis: GuideAxis,
        position: f64,
    ) -> (OndinApp, GuideId, egui::Rect, f32) {
        let mut app = OndinApp::headless(ctx);
        app.canvas_px = (800, 600);
        let id = GuideId(app.session.ids.mint());
        app.session
            .try_commit(Transaction(vec![Operation::AddGuide {
                guide: Guide {
                    id,
                    axis,
                    position,
                    color: None,
                    owner: None,
                },
            }]))
            .expect("add the guide");
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        (app, id, rect, 1.0)
    }

    /// Pressing a guide anywhere in its pick band leaves it where it was. Swept
    /// across the band in both directions, since a press exactly on the line
    /// passes even without the grab offset.
    #[test]
    fn pressing_a_guide_inside_its_pick_band_does_not_move_it() {
        let ctx = egui::Context::default();
        for (axis, position) in [
            (GuideAxis::Vertical, 200.0_f64),
            (GuideAxis::Horizontal, 150.0_f64),
        ] {
            let (mut app, id, rect, ppp) = app_with_guide(&ctx, axis, position);
            let on_line = match axis {
                GuideAxis::Vertical => Point::new(position, 100.0),
                GuideAxis::Horizontal => Point::new(100.0, position),
            };
            let centre = app.to_screen(on_line, rect, ppp);
            for off in [-GUIDE_PICK_PT, -2.5, 0.0, 2.5, GUIDE_PICK_PT] {
                let p = match axis {
                    GuideAxis::Vertical => egui::pos2(centre.x + off, centre.y),
                    GuideAxis::Horizontal => egui::pos2(centre.x, centre.y + off),
                };
                app.begin_guide_drag(Some(id), axis, p, rect, ppp);
                let Drag::Guide(d) = &app.drag else {
                    panic!("the press has to arm a guide drag");
                };
                assert!(
                    (d.position - position).abs() < 1e-9,
                    "{axis:?} guide at {position} pressed {off}pt off its line \
                     opened the drag at {} — a press is a grab, not a move",
                    d.position
                );
            }
        }
    }

    /// A guide pulled out of a ruler (`id: None`) has no grab and appears under
    /// the pointer.
    #[test]
    fn a_guide_pulled_from_the_ruler_lands_under_the_pointer() {
        let ctx = egui::Context::default();
        let (mut app, _, rect, ppp) = app_with_guide(&ctx, GuideAxis::Vertical, 200.0);
        let p = egui::pos2(345.0, 120.0);
        app.begin_guide_drag(None, GuideAxis::Vertical, p, rect, ppp);
        let Drag::Guide(d) = &app.drag else {
            panic!("the press has to arm a guide drag");
        };
        assert_eq!(
            d.grab,
            Vec2::ZERO,
            "a new guide has nothing to be offset from"
        );
        assert!(
            (d.position - app.to_world(p, rect, ppp).x).abs() < 1e-9,
            "a new guide sits under the pointer, not beside it"
        );
    }

    /// `finish_guide_drag`'s commit table, arm by arm: (1) an Alt-dragged copy
    /// dropped on the ruler leaves the original, (2) a press that changes
    /// nothing is no undo step, (3) a scope change alone is one, (4) a guide
    /// pulled out and put back never existed, (5) an Alt-dragged copy dropped on
    /// the canvas is copied there, colour and all, and selected.
    ///
    /// Flips: collapsing (1)'s `if d.copying` arm into the delete fails at (1);
    /// dropping `|| g.owner != d.owner` fails at (3); making (5)'s arm the plain
    /// move fails at "a copy exists"; (5)'s copy written with `color: None`
    /// fails at "the copy carries the colour". `let moved = true;` does not bite, since
    /// `EditorSession::commit_inner` also drops a no-op transaction.
    #[test]
    fn the_guide_commit_table_answers_each_of_its_arms() {
        let ctx = egui::Context::default();
        let bar = |axis: GuideAxis, rect: egui::Rect| match axis {
            // Inside the guide's own bar, which `discarding` reads.
            GuideAxis::Vertical => egui::pos2(rect.left() + THICKNESS / 2.0, 300.0),
            GuideAxis::Horizontal => egui::pos2(400.0, rect.top() + THICKNESS / 2.0),
        };

        // 1. An Alt-dragged copy dropped on the ruler leaves the original.
        let (mut app, id, rect, ppp) = app_with_guide(&ctx, GuideAxis::Vertical, 200.0);
        app.show_rulers = true;
        let depth = app.session.history.undo_depth();
        let on_line = app.to_screen(Point::new(200.0, 100.0), rect, ppp);
        app.begin_guide_drag(Some(id), GuideAxis::Vertical, on_line, rect, ppp);
        app.update_guide_drag(egui::pos2(400.0, 300.0), rect, ppp, true);
        app.update_guide_drag(bar(GuideAxis::Vertical, rect), rect, ppp, true);
        app.finish_guide_drag();
        assert!(
            app.session.doc.guide(id).is_some(),
            "an Alt-drag was never asking to delete anything, and the original has \
             not moved"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            depth,
            "and throwing the copy away is not an edit"
        );

        // Control: the same gesture without Alt is a delete.
        let (mut app, id, rect, ppp) = app_with_guide(&ctx, GuideAxis::Vertical, 200.0);
        app.show_rulers = true;
        let on_line = app.to_screen(Point::new(200.0, 100.0), rect, ppp);
        app.begin_guide_drag(Some(id), GuideAxis::Vertical, on_line, rect, ppp);
        app.update_guide_drag(egui::pos2(400.0, 300.0), rect, ppp, false);
        app.update_guide_drag(bar(GuideAxis::Vertical, rect), rect, ppp, false);
        app.finish_guide_drag();
        assert!(
            app.session.doc.guide(id).is_none(),
            "control: dropped on the ruler with no Alt, a guide is deleted"
        );

        // 2 and 3 are one sequence: the headless app starts with an artboard and
        // the guide has `owner: None`, so the first press is a scope change and
        // the no-op press must come second.
        let (mut app, id, rect, ppp) = app_with_guide(&ctx, GuideAxis::Vertical, 200.0);
        let depth = app.session.history.undo_depth();
        let on_line = app.to_screen(Point::new(200.0, 100.0), rect, ppp);

        // 3 first: the scope counts as moved though the line does not shift.
        app.begin_guide_drag(Some(id), GuideAxis::Vertical, on_line, rect, ppp);
        app.update_guide_drag(on_line, rect, ppp, false);
        app.finish_guide_drag();
        let scoped = app.session.doc.guide(id).copied().expect("still there");
        assert!(
            scoped.owner.is_some(),
            "a guide dragged into a frame without shifting its line is still a \
             change — the position is read in the owner's space"
        );
        assert!(
            (scoped.position - 200.0).abs() < 1e-9,
            "and the line really did not move: {}",
            scoped.position
        );
        assert_eq!(
            app.session.history.undo_depth(),
            depth + 1,
            "one undo step for the scope change"
        );

        // 2: press it again, with nothing left to change.
        let depth = app.session.history.undo_depth();
        app.begin_guide_drag(Some(id), GuideAxis::Vertical, on_line, rect, ppp);
        app.update_guide_drag(on_line, rect, ppp, false);
        app.finish_guide_drag();
        assert_eq!(
            app.session.history.undo_depth(),
            depth,
            "selecting a guide is not an edit, and an undo step for a guide that \
             is exactly where it was is one the user has to press through to \
             reach a real one"
        );

        // 4. Pulled out of the ruler and put straight back: nothing happened.
        let (mut app, _, rect, ppp) = app_with_guide(&ctx, GuideAxis::Vertical, 200.0);
        app.show_rulers = true;
        let before = app.session.doc.guides().len();
        let depth = app.session.history.undo_depth();
        app.begin_guide_drag(
            None,
            GuideAxis::Vertical,
            bar(GuideAxis::Vertical, rect),
            rect,
            ppp,
        );
        app.update_guide_drag(egui::pos2(400.0, 300.0), rect, ppp, false);
        app.update_guide_drag(bar(GuideAxis::Vertical, rect), rect, ppp, false);
        app.finish_guide_drag();
        assert_eq!(
            app.session.doc.guides().len(),
            before,
            "a guide pulled out and put back never existed"
        );
        assert_eq!(app.session.history.undo_depth(), depth, "and wrote nothing");

        // Control: dropped on the canvas instead, it exists.
        let (mut app, _, rect, ppp) = app_with_guide(&ctx, GuideAxis::Vertical, 200.0);
        app.show_rulers = true;
        let before = app.session.doc.guides().len();
        app.begin_guide_drag(
            None,
            GuideAxis::Vertical,
            bar(GuideAxis::Vertical, rect),
            rect,
            ppp,
        );
        app.update_guide_drag(egui::pos2(400.0, 300.0), rect, ppp, false);
        app.finish_guide_drag();
        assert_eq!(
            app.session.doc.guides().len(),
            before + 1,
            "control: dropped on the canvas, a pulled guide exists"
        );

        // 5. An Alt-dragged guide dropped on the canvas is copied there, with its
        // colour, and the copy is the selection.
        let (mut app, id, rect, ppp) = app_with_guide(&ctx, GuideAxis::Vertical, 200.0);
        app.show_rulers = true;
        let red = Some(ondin_core::peniko::Color::from_rgba8(255, 0, 0, 255));
        app.session
            .try_commit(Transaction(vec![Operation::SetGuideColor {
                id,
                color: red,
            }]))
            .expect("colour the guide");
        assert!(!app.lock_guides, "fixture: guides unlocked");
        let before = app.session.doc.guides().len();
        let depth = app.session.history.undo_depth();
        let on_line = app.to_screen(Point::new(200.0, 100.0), rect, ppp);
        let to = egui::pos2(on_line.x + 60.0, 300.0);
        app.begin_guide_drag(Some(id), GuideAxis::Vertical, on_line, rect, ppp);
        app.update_guide_drag(to, rect, ppp, true);
        app.finish_guide_drag();
        assert_eq!(app.session.doc.guides().len(), before + 1, "a copy exists");
        assert_eq!(
            app.session.doc.guide(id).map(|g| g.position),
            Some(200.0),
            "the original has not moved"
        );
        let copy = app
            .session
            .doc
            .guides()
            .iter()
            .find(|g| g.id != id)
            .copied()
            .expect("the copy");
        assert!(
            (copy.position - 200.0).abs() > 1.0,
            "the copy lands where the line was dragged to: {}",
            copy.position
        );
        assert_eq!(copy.color, red, "the copy carries the colour");
        assert_eq!(
            app.session.selection.guides(),
            &[copy.id],
            "and is selected"
        );
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one undo step");
    }

    /// A guide hidden by *View ▸ Hide guides* (which keeps the guide selection) is
    /// neither nudged by the arrow keys nor deleted by Delete. The visible-guide
    /// control is required: a version that never nudges passes the hidden rows.
    ///
    /// Flips, each gate separately: removing `nudge_guides`' gate fails at the
    /// position (`104.0` against `100.0`); removing `remove_selected_guides`' gate
    /// fails at the Delete assertion.
    #[test]
    fn a_hidden_guide_is_neither_nudged_nor_deleted() {
        let ctx = egui::Context::default();
        let down = ondin_core::kurbo::Vec2::new(0.0, 1.0);

        // The way a guide goes off screen without the selection being cleared.
        for (label, hide) in [(
            "View ▸ Hide guides",
            (|app: &mut OndinApp| {
                app.show_guides = false;
            }) as fn(&mut OndinApp),
        )] {
            let (mut app, id, _, _) = app_with_guide(&ctx, GuideAxis::Horizontal, 100.0);
            app.session.selection.set_guide(id);
            hide(&mut app);
            assert!(
                !app.guides_on(),
                "fixture: {label} must actually take the guides off screen"
            );
            assert_eq!(
                app.session.selection.guides(),
                &[id],
                "fixture: and must leave the selection standing, or this test is \
                 about an empty selection — D201 relies on it surviving"
            );
            let depth0 = app.session.history.undo_depth();

            for _ in 0..4 {
                app.nudge_guides(down);
            }
            assert_eq!(
                app.session.doc.guide(id).expect("the guide").position,
                100.0,
                "{label}: four presses of the arrow key moved a line nobody can see"
            );
            assert_eq!(
                app.session.history.undo_depth(),
                depth0,
                "{label}: and put it in the history"
            );

            app.remove_selected_guides();
            assert!(
                app.session.doc.guide(id).is_some(),
                "{label}: Delete destroyed a guide with no line on screen to have \
                 aimed at"
            );
        }

        // Control: with the guides on, both verbs work.
        let (mut app, id, _, _) = app_with_guide(&ctx, GuideAxis::Horizontal, 100.0);
        app.session.selection.set_guide(id);
        assert!(app.guides_on(), "control fixture: the guides are on screen");
        for _ in 0..4 {
            app.nudge_guides(down);
        }
        assert_eq!(
            app.session.doc.guide(id).expect("the guide").position,
            104.0,
            "control: a visible guide still nudges"
        );
        app.remove_selected_guides();
        assert!(
            app.session.doc.guide(id).is_none(),
            "control: and a visible guide still deletes"
        );
    }

    /// Focus mode hides the app's chrome and leaves the canvas working: the
    /// guide lines stay drawn, pickable, nudgeable and deletable, while the ruler
    /// bars (the control) go. Three claims, because `guides_on()` alone would pass
    /// a version that drew the line and refused the drag.
    ///
    /// flip: restore `!self.focus` in `guides_on` → fails at the first
    /// assertion. The layout grids have their own guard in `draw_layout_grids`
    /// and do not read `guides_on`.
    #[test]
    fn focus_mode_leaves_the_guides_on_screen_and_live() {
        let ctx = egui::Context::default();
        let down = ondin_core::kurbo::Vec2::new(0.0, 1.0);
        let (mut app, id, _, _) = app_with_guide(&ctx, GuideAxis::Horizontal, 100.0);
        app.session.selection.set_guide(id);
        app.show_rulers = true;
        app.focus = true;

        assert!(
            app.guides_on(),
            "focus mode is a chrome switch: the guide lines stay on the canvas"
        );
        assert!(
            app.guides_pickable(),
            "and stay draggable — the mode is for editing with the panels out of \
             the way, not for looking without touching"
        );

        for _ in 0..4 {
            app.nudge_guides(down);
        }
        assert_eq!(
            app.session.doc.guide(id).expect("the guide").position,
            104.0,
            "the arrow keys still move it"
        );
        app.remove_selected_guides();
        assert!(
            app.session.doc.guide(id).is_none(),
            "and Delete still removes it"
        );

        // Control: focus mode still takes the bars away.
        assert!(
            !app.rulers_on(),
            "control: the ruler bars are window furniture and focus mode still \
             takes them away — without this the test is green for a focus mode \
             that does nothing"
        );
    }

    /// Toggling the rulers mid-drag does not change what the release means:
    /// `Shift+R` or `Ctrl+\` with the button down must not turn a cancel into a
    /// create, nor a bar appearing delete a guide never dropped on one, nor
    /// announce a discard for a new guide still on its bar. The control asserts
    /// the bin cursor (`announces_discard`), since a version that never creates
    /// guides would pass an empty-list check.
    ///
    /// flip: `d.rulers_shown` back to `self.rulers_on()` in `update_guide_drag` →
    /// fails at the focus-mode row and at row 3. Reverting only one of the two
    /// readers (`over_own_ruler`, `off_rulers`) leaves the other silencing the
    /// `dead_code` lint, which counts readers and proves nothing about the rule.
    #[test]
    fn toggling_the_rulers_mid_drag_does_not_change_what_the_release_means() {
        let ctx = egui::Context::default();
        let bar = |rect: egui::Rect| egui::pos2(rect.left() + THICKNESS / 2.0, 300.0);

        // Control: rulers on throughout; out and back onto the bar is a discard.
        let (mut app, id, rect, ppp) = app_with_guide(&ctx, GuideAxis::Vertical, 200.0);
        app.show_rulers = true;
        let on_line = app.to_screen(Point::new(200.0, 100.0), rect, ppp);
        app.begin_guide_drag(Some(id), GuideAxis::Vertical, on_line, rect, ppp);
        app.update_guide_drag(egui::pos2(400.0, 300.0), rect, ppp, false);
        app.update_guide_drag(bar(rect), rect, ppp, false);
        let announced = match &app.drag {
            Drag::Guide(d) => d.announces_discard(),
            _ => panic!("control: the drag has to still be live at this point"),
        };
        app.finish_guide_drag();
        assert!(
            announced,
            "control: the un-flipped gesture has to *announce* the discard, or \
             'no guide afterwards' is satisfied by a version that never makes one"
        );
        assert!(
            app.session.doc.guide(id).is_none(),
            "control: and dropping on the bar deletes the guide"
        );

        // 1 and 2: a new guide pulled out of the bar, where cancelling must create
        // nothing (an existing guide dropped on the bar is a legitimate edit).
        let pull = |flip: fn(&mut OndinApp)| {
            let ctx = egui::Context::default();
            let mut app = OndinApp::headless(&ctx);
            app.canvas_px = (800, 600);
            app.show_rulers = true;
            let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
            let ppp = 1.0;
            let depth = app.session.history.undo_depth();
            assert!(
                app.session.doc.guides().is_empty(),
                "the fixture starts with no guides, so anything found afterwards \
                 was made by this gesture"
            );
            // Out of the left bar, onto the canvas, and back onto the bar.
            app.begin_guide_drag(None, GuideAxis::Vertical, bar(rect), rect, ppp);
            app.update_guide_drag(egui::pos2(400.0, 300.0), rect, ppp, false);
            flip(&mut app);
            app.update_guide_drag(bar(rect), rect, ppp, false);
            app.finish_guide_drag();
            (app, depth)
        };

        for (what, flip) in [
            (
                "`Ctrl+\\` (focus mode)",
                (|a: &mut OndinApp| a.focus = true) as fn(&mut OndinApp),
            ),
            ("`Shift+R` (show rulers)", |a: &mut OndinApp| {
                a.show_rulers = false
            }),
        ] {
            let (app, depth) = pull(flip);
            assert!(
                !app.rulers_on(),
                "{what}: the fixture has to actually take the bars away, or this \
                 row is the control again"
            );
            assert!(
                app.session.doc.guides().is_empty(),
                "{what} pressed mid-drag turned the cancel gesture into a create \
                 gesture — a guide committed at whatever coordinate the vanished \
                 bar sat over, on a release that meant 'throw this away'"
            );
            assert_eq!(
                app.session.history.undo_depth(),
                depth,
                "{what}: and it spent an undo step and dirtied the document doing it"
            );
        }

        // Control: the identical pull with nothing toggled creates nothing either.
        let (app, depth) = pull(|_| {});
        assert!(
            app.rulers_on() && app.session.doc.guides().is_empty(),
            "control: pulled out and put back with the bars left alone, no guide"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            depth,
            "control: and no undo step"
        );

        // 3. Rulers off at the press, on mid-drag: a bar appearing must not delete
        // a guide that never went near one.
        let (mut app, id, rect, ppp) = app_with_guide(&ctx, GuideAxis::Vertical, 200.0);
        app.show_rulers = false;
        let on_line = app.to_screen(Point::new(200.0, 100.0), rect, ppp);
        app.begin_guide_drag(Some(id), GuideAxis::Vertical, on_line, rect, ppp);
        app.update_guide_drag(egui::pos2(400.0, 300.0), rect, ppp, false);
        app.show_rulers = true;
        app.update_guide_drag(bar(rect), rect, ppp, false);
        app.finish_guide_drag();
        assert!(
            app.session.doc.guide(id).is_some(),
            "a bar that appeared mid-drag cannot delete a guide the gesture never \
             dropped on one"
        );

        // 4. The announcement: taking the bars away must not latch `left_ruler`
        // without the pointer moving.
        let (mut app, _id, rect, ppp) = app_with_guide(&ctx, GuideAxis::Vertical, 200.0);
        app.show_rulers = true;
        app.begin_guide_drag(None, GuideAxis::Vertical, bar(rect), rect, ppp);
        let announced = |app: &OndinApp| match &app.drag {
            Drag::Guide(d) => d.announces_discard(),
            _ => panic!("the drag has to still be live"),
        };
        assert!(
            !announced(&app),
            "the fixture starts on the bar it came from, so there is nothing to \
             announce yet — which is the whole of what `left_ruler` is for"
        );
        // The pointer does not move. Only the bars go.
        app.focus = true;
        app.update_guide_drag(bar(rect), rect, ppp, false);
        assert!(
            !announced(&app),
            "a guide still sitting where it was pressed has not been 'off both \
             rulers' just because the rulers were switched off underneath it — \
             announcing here draws the bin cursor and the discard fade over a \
             guide that does not exist yet"
        );
    }
}

#[cfg(test)]
mod origin_tests {
    //! The ruler origin and the rough edge a rotated frame puts in it (§15 D36).
    //! The rotated case pins a decided behaviour, not a defect: the origin is a
    //! mapped corner while the band is axis-aligned world bounds, so they
    //! disagree, and the test pins the size of that disagreement.
    use super::*;
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{NodeKind, Operation, Transaction};

    /// A single frame of `size`, under `transform`, selected.
    fn app_with_a_frame(
        ctx: &egui::Context,
        size: Size,
        transform: Option<Affine>,
    ) -> (OndinApp, ondin_core::NodeId) {
        let mut app = OndinApp::headless(ctx);
        let id = app.session.ids.mint();
        let parent = app.session.doc.root();
        app.session
            .try_commit(Transaction(vec![Operation::CreateNode {
                id,
                parent,
                index: 0,
                kind: NodeKind::Artboard { size },
                transform,
                name: None,
            }]))
            .expect("create the frame");
        app.session.selection.set(vec![id]);
        (app, id)
    }

    /// An upright frame puts the origin on its own top-left corner, and the band
    /// then reads 0 to the width (the control for the rotated case below).
    #[test]
    fn an_upright_frame_puts_the_origin_on_its_corner() {
        let ctx = egui::Context::default();
        let (app, _) = app_with_a_frame(
            &ctx,
            Size::new(480.0, 320.0),
            Some(Affine::translate((100.0, 60.0))),
        );

        let origin = app.ruler_origin();
        assert_eq!(
            (origin.x, origin.y),
            (100.0, 60.0),
            "the origin is the frame's local (0,0) carried into world space"
        );

        let ((x0, x1), (y0, y1)) = app.selection_extent().expect("a frame is selected");
        assert_eq!(
            (x0 - origin.x, x1 - origin.x),
            (0.0, 480.0),
            "so the horizontal band reads 0 to the width"
        );
        assert_eq!(
            (y0 - origin.y, y1 - origin.y),
            (0.0, 320.0),
            "and the vertical band reads 0 to the height"
        );
    }

    /// A 480×320 frame rotated 30° reads −160 to 415.7 on its band, a band 575.7
    /// wide: the origin is the frame's own corner while `selection_extent` is the
    /// axis-aligned world bounds, and the two ends come from different corners.
    ///
    /// flip: take the origin from `preview_world_bounds(id).origin()` → fails at
    /// the origin assertion while the upright test stays green. That flip would
    /// not close the rough edge: `to` stays 575.7, because the band would have to
    /// stop being axis-aligned bounds.
    #[test]
    fn a_rotated_frames_band_does_not_start_at_zero() {
        let ctx = egui::Context::default();
        let turn = Affine::rotate(std::f64::consts::FRAC_PI_6);
        let (app, _) = app_with_a_frame(&ctx, Size::new(480.0, 320.0), Some(turn));

        let origin = app.ruler_origin();
        assert_eq!(
            (origin.x, origin.y),
            (0.0, 0.0),
            "the frame's corner is still at the world origin — it is what the \
             rest of the frame did that the band is about"
        );

        let ((x0, x1), _) = app.selection_extent().expect("a frame is selected");
        let (from, to) = (x0 - origin.x, x1 - origin.x);
        assert!(
            (from + 160.0).abs() < 0.1,
            "D36's rough edge: the band starts *before* the origin, at {from:.1} \
             — the corner that used to be the bottom-left has swung out to \
             −320·sin30, and the ruler shows it"
        );
        assert!(
            (to - 415.7).abs() < 0.1,
            "and it ends short of the width, not past it: {to:.1} against a frame \
             480 wide, because the far corner's x is 480·cos30 and the 320·sin30 \
             term went the *other* way, into `from`"
        );
        assert!(
            (to - from - 575.7).abs() < 0.2,
            "so the band is 575.7 wide for a frame of 480 — the axis-aligned \
             bounds of a rotated rectangle, which is what D36 means by honest \
             and not useful"
        );
    }

    /// Only a lone selected frame moves the origin: no selection, a lone
    /// non-frame, and a frame selected with something else leave it at the
    /// document's. Every case sits under the same translate, so each `ZERO` is
    /// real and the frame's own `(100, 60)` is the control.
    ///
    /// Flips: the `!is_frame` return replaced with `let _ = is_frame;` fails at
    /// "a lone rect does not"; `single()` replaced with `ids().first()` fails at
    /// "nor a frame selected with something else".
    #[test]
    fn only_a_lone_selected_frame_moves_the_origin() {
        let ctx = egui::Context::default();
        let at = Affine::translate((100.0, 60.0));
        let (mut app, frame) = app_with_a_frame(&ctx, Size::new(480.0, 320.0), Some(at));
        let rect = app.session.ids.mint();
        let parent = app.session.doc.root();
        app.session
            .try_commit(Transaction(vec![Operation::CreateNode {
                id: rect,
                parent,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(40.0, 40.0),
                    corner_radii: Default::default(),
                },
                transform: Some(at),
                name: None,
            }]))
            .expect("create the rect");
        let zero = ondin_core::kurbo::Point::ZERO;

        app.session.selection.set(vec![frame]);
        let origin = app.ruler_origin();
        assert_eq!(
            (origin.x, origin.y),
            (100.0, 60.0),
            "control: the frame alone moves it"
        );

        app.session.selection.clear();
        assert_eq!(app.ruler_origin(), zero, "nothing selected does not");

        app.session.selection.set(vec![rect]);
        assert_eq!(app.ruler_origin(), zero, "a lone rect does not");

        app.session.selection.set(vec![frame, rect]);
        assert_eq!(
            app.ruler_origin(),
            zero,
            "nor a frame selected with something else"
        );
    }

    /// The rulers' band spans where a flex item is drawn, not where it is stored:
    /// a 40 × 30 rect stored at (300, 150) is the first item of a padded row in a
    /// frame at (100, 60), so it is drawn at (120, 80)–(160, 110).
    #[test]
    fn the_band_spans_where_a_flex_item_is_drawn() {
        use ondin_core::container::{AlignItems, Display, Flex};
        let ctx = egui::Context::default();
        let (mut app, frame) = app_with_a_frame(
            &ctx,
            Size::new(400.0, 200.0),
            Some(Affine::translate((100.0, 60.0))),
        );
        let item = app.session.ids.mint();
        app.session
            .try_commit(Transaction(vec![
                Operation::CreateNode {
                    id: item,
                    parent: frame,
                    index: 0,
                    kind: NodeKind::Rect {
                        size: Size::new(40.0, 30.0),
                        corner_radii: Default::default(),
                    },
                    transform: Some(Affine::translate((300.0, 150.0))),
                    name: None,
                },
                Operation::SetDisplay {
                    id: frame,
                    display: Some(Display::Flex(Flex {
                        padding: [20.0; 4],
                        align_items: AlignItems::Start,
                        ..Default::default()
                    })),
                },
            ]))
            .expect("the row");
        app.session.selection.set(vec![item]);
        assert_eq!(
            app.selection_extent(),
            Some(((120.0, 160.0), (80.0, 110.0))),
            "the band is where the item is drawn"
        );
    }
}
