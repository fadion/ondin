//! The rulers, and the guides pulled out of them (`design/Editor.dc.html`).
//!
//! Two 20pt bars along the top and left of the canvas, ticked and labelled in
//! world units, plus the lines a designer drags off them to line work up
//! against. The guides themselves are document state (`ondin_core::guide`);
//! everything in this file is the chrome that shows and edits them.
//!
//! **Drawn as egui chrome, not as scene content.** A guide never enters the
//! Vello scene: it must stay one hairline wide at every zoom, sit above the
//! artwork including a frame that clips, and be invisible to `ondin export`,
//! which renders the document rather than the editor. All three fall out of
//! drawing it here, next to the selection box, and none of them would if it
//! were a node.
//!
//! **The tick step is chosen, not fixed.** The design freezes one zoom level
//! (100 units at 51.94%, so 51.94px between labels); the real ruler picks the
//! smallest 1/2/5×10ⁿ step whose labels clear [`MIN_LABEL_PX`], which reproduces
//! the design's 100 at the design's zoom and stays readable either side of it.

use crate::app::OndinApp;
use crate::cursor::Bitmap;
use crate::preview::{Drag, GuideDrag};
use crate::theme::{self, color};
use eframe::egui;
use ondin_core::kurbo::{Affine, Point, Vec2};
use ondin_core::{Guide, GuideAxis, GuideId, NodeId, Operation, Transaction};

/// Thickness of each ruler bar, in points. The design's 20px, and egui points
/// are CSS px here (see `theme`).
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

/// The narrowest gap between two labelled ticks. Below this the ruler picks a
/// coarser step. The design's own gap is 51.94px, so this reproduces it.
const MIN_LABEL_PX: f64 = 50.0;

/// The position chip a guide puts in the ruler: thickness along the guide's own
/// axis, and the type inside it.
const CHIP_THICKNESS: f32 = 13.0;
const CHIP_TEXT: f32 = 8.0;
/// Padding either side of a chip's number.
///
/// 2.5 and not a rounder number because of what it has to reproduce: the
/// design's chip is 20pt across with `332` in it, and `332` at [`CHIP_TEXT`]
/// measures 14.75pt. 14.75 + 5 = 19.75, which the clamp to [`THICKNESS`] lifts
/// to exactly the design's 20 — so a three-digit chip is the design's chip, and
/// only a number too big for that grows the block.
const CHIP_PAD_X: f32 = 2.5;

/// How close a tick's number may come to one of the selection band's own
/// numbers before it is dropped in favour of it. Roughly the width of a
/// three-digit label, so the two never overprint.
const EXTENT_LABEL_CLEAR: f32 = 16.0;

/// The band across a ruler covering what is selected, clipped to the bar.
/// `None` when none of it is on screen.
///
/// Figma's, and it answers a question the ticks alone cannot: not "where is
/// this" but "how wide is it", read straight off the ruler without going to the
/// inspector. A tinted ground rather than an outline, because it has to read as
/// *a region of the ruler* rather than as another object drawn on it.
///
/// A free function, not a method, so the arithmetic is addressable directly
/// rather than through whatever frame state the method would have wanted set up
/// around it.
fn extent_band(axis: GuideAxis, bar: egui::Rect, from: f32, to: f32) -> Option<egui::Rect> {
    let (lo, hi) = (from.min(to), from.max(to));
    // Clamped rather than skipped, so a selection running off the viewport
    // still shows the part of its extent that is on screen — the band stopping
    // at the edge says "it continues", where nothing at all would say "no
    // selection".
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

/// How close (screen pt) the pointer must come to a guide to grab it.
///
/// Wider than the 1px line, because a hairline is not a target — and the same
/// 6pt as the selection box's own edge band (`canvas::EDGE_PICK_PX`), which is
/// the nearest thing to it: an invisible strip along a line you drag sideways.
/// A guide should be no harder to pick up than the edge of a shape.
const GUIDE_PICK_PT: f32 = 6.0;

/// The dash and gap of a frame-scoped guide's continuation beyond its frame, in
/// screen points.
///
/// Short enough to read as the same line carried on rather than as a second,
/// dotted mark of its own — which is the whole claim the extension makes.
const EXTENSION_DASH: f32 = 3.0;
const EXTENSION_GAP: f32 = 3.0;

/// How far from square a frame's world basis may be and still count as upright.
///
/// A world transform is a product of rotations, skews and flips (§5.6 keeps
/// scale out of it), so an untouched frame's angle and skew come back as exact
/// zeroes and a rotated one is nowhere near. The tolerance is only here so a
/// frame turned and turned back does not fall on the wrong side of it.
const UPRIGHT_TOL: f64 = 1e-9;

/// Whether `world` leaves a frame square with the world axes — the one question
/// a scoped guide's *world* behaviour turns on.
///
/// **Two features read it and they must agree.** A guide in a rotated frame
/// draws (frame-local, leaning with its frame) but does not attract
/// (`OndinApp::guide_lines` contributes nothing for it), and its dotted
/// continuation is suppressed for exactly that reason: a dashed line promising an
/// alignment the guide cannot offer is worse than no line. One predicate, so
/// "does it snap" and "does it extend" cannot drift apart.
///
/// A mirror does not disqualify a frame. `Basis::of` peels a reflection into
/// `Orientation::mirror` and leaves the angle at zero, and a mirrored frame is
/// still axis-aligned — the projection through its matrix simply flips a sign,
/// which is arithmetic rather than a lean.
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
/// **Drawing and picking are both built from this**, which is the point of it
/// existing. A scoped guide's line is its frame's span carried through the
/// frame's world transform, so it can lean, stop short of the viewport and move
/// with the frame — none of which the old "screen-space band across the canvas"
/// could express, and all of which the hit test has to agree with or the app
/// claims a press for a line nobody can see (§15 D35's class of bug).
struct GuideLine {
    axis: GuideAxis,
    /// The solid segment's ends: the visible canvas for a global guide, the
    /// owner frame's own span for a scoped one.
    a: egui::Pos2,
    b: egui::Pos2,
    /// The guide's own number — a world coordinate when it is global, a
    /// frame-local offset when it is scoped. The number the inspector's field
    /// holds either way, which is `chip_label`'s standing promise.
    label: f64,
    /// Whether the line continues past both ends, dashed, while the guide is
    /// hovered or selected. Scoped guides in upright frames only: a global guide
    /// already spans the canvas, and a rotated frame's guide must not imply an
    /// alignment it does not offer.
    extends: bool,
}

/// How far a guide about to be thrown away is faded.
const DISCARD_ALPHA: f32 = 0.35;

/// The ink a guide is drawn in, for its state.
///
/// **The order of the two decisions is the whole of this function**, which is why
/// it is one and not two lines at the call site. `color::SELECT` is the canvas's
/// "the app is talking about the selection" blue — the same ink as a hover
/// outline, a bounding box and a snap line (§15 D13) — and it *replaces* the
/// guide's own colour while it is picked. So the discard fade has to be applied
/// **after** that choice: fading the guide's colour first puts the alpha on a
/// value the blue then throws away, and the whole "letting go will delete this"
/// signal silently disappears. That is exactly what happened when selection
/// stopped meaning "the same colour, thicker" — pinned by
/// `a_discarding_guide_is_faded_even_though_it_is_drawn_as_selected`.
///
/// Not the panel accent: a selected guide is canvas chrome and has to read as the
/// same statement the box round a layer makes.
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
/// hairline.
///
/// ⚠️ **Named and written for guides, and it has a second subject since §15 D484**:
/// `grid::draw_pixel_grid` calls it too, having hand-inlined it until then. So the
/// base below is not a guide-only decision any more — whichever answer it
/// eventually gets moves the pixel grid as well.
///
/// A line whose device width is **odd** wants its centre on a pixel *centre* and
/// an even one on a pixel *edge*; getting this backwards is what turns a 1px line
/// into a 2px grey smear, which at 100% zoom is the whole appearance of the guide.
///
/// **The parity is `floor(width × ppp)` — the device width, floored.** All three
/// parts of that were wrong at some point and a probe over 100/125/150/175% is
/// what settled it (`the_two_guide_weights_keep_a_solid_core_at_every_scale`).
///
/// It used to be `width as i32 % 2`, which reads the **point** width and so
/// assumes one point is one device pixel. At 150% and 175% — the two ordinary
/// Windows scales — a 2pt line is 3 and 3.5 device pixels, i.e. *odd*, and was
/// being edge-centred on the strength of a number that no longer described what
/// was drawn: 3 device pixels of ink spread over 4 pixels as `[0.5, 1, 1, 0.5]`
/// instead of landing on 3 as `[1, 1, 1]`.
///
/// `round` was the obvious repair and is worse than either: it rounds 1.5 up to 2,
/// calls a 1pt line even, edge-centres it, and leaves the **resting** guide with
/// no solid pixel at all — `[0.75, 0.75]`, which is precisely the smear above. A
/// fractional device width cannot have both edges on the grid, so the thing to
/// preserve is the *core*, and flooring is what keeps one: 1.5 floors to 1, odd,
/// pixel-centred, `[0.25, 1, 0.25]`.
/// ⚠️ **The parity above is shared with the chrome's rules and the base is not**
/// (§15 D479). `ui::hairline_on_pixel_centre` is the same predicate and is called
/// here rather than re-derived; `ui::hairline_in_column` is the *other* half, and
/// it floors where this rounds. That is deliberate: a chrome rule is drawn into a
/// 1pt allocation between two galleys and must not leave it, and a `round` base
/// moves the ink by up to a whole point at 100%. A guide is drawn on the canvas
/// with nothing to stay inside, so it takes the **nearest** grid line to where the
/// user put it. The review filed the two as one rule written twice
/// (`[S18.1-L3-03]`, G20); they are one rule and two bases, and the difference is
/// measured rather than stylistic.
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
/// Only meaningful for an axis-aligned guide — which is the only kind that gets
/// an extension at all ([`upright`]) — so the run is along the bar's own axis and
/// the guide's coordinate is taken from the solid segment rather than recomputed.
/// Either run can come back empty, when the frame already reaches that edge.
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
/// A bounding-box test rather than a per-end one: a leaning guide in a rotated
/// frame can have both ends outside `inner` and still cross it, and dropping that
/// one would make a guide vanish exactly when the view is zoomed into the middle
/// of it.
fn in_view(inner: egui::Rect, line: GuideLine) -> Option<GuideLine> {
    let hull = egui::Rect::from_two_pos(line.a, line.b);
    inner.intersects(hull).then_some(line)
}

impl GuideLine {
    /// The along-the-bar coordinate the guide's chip sits at.
    ///
    /// The **midpoint** of the segment on the guide's own axis. For a global
    /// guide and for one in an upright frame both ends share that coordinate, so
    /// this is exactly the guide's position on screen and nothing has changed.
    /// For a guide in a rotated frame there is no single coordinate to be exact
    /// about, and the midpoint puts the chip beside the middle of the line —
    /// approximate *placement*, while the number in it stays the guide's own
    /// exact offset. `chip_label`'s rule is about the readout, not about where
    /// the tab is pulled out of the ruler.
    fn chip_along(&self) -> f32 {
        match self.axis {
            GuideAxis::Horizontal => (self.a.y + self.b.y) / 2.0,
            GuideAxis::Vertical => (self.a.x + self.b.x) / 2.0,
        }
    }

    /// Screen distance from `p` to the solid segment — the pick test.
    ///
    /// To the *segment*, not to the infinite line, so a frame-scoped guide is
    /// grabbable exactly where it is drawn and the empty canvas beside a short
    /// guide is not a 6pt band claiming to be one. The dotted continuation is not
    /// a target either, which is consistent: it only appears once the guide is
    /// already hovered or selected.
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
/// Not the ruler's own precision, and never abbreviated. A tick's label
/// describes a whole band of the ruler and can afford to be round; the chip
/// quotes one exact coordinate, which is the number the inspector's field holds
/// and the number the designer will type somewhere else. Rounding a guide at
/// 12.5 to `13` in the one place that claims to say where it is would be a
/// readout that lies — which is also why the chip grows to fit its number
/// rather than the number being cut to fit the chip.
fn chip_label(position: f64) -> String {
    let v = if position == 0.0 { 0.0 } else { position };
    trim_zeroes(&format!("{v:.2}")).to_string()
}

/// [`chip_label`] in thousands (`1080` → `1.1k`), for the left bar, which gives
/// a label 17pt to read across.
///
/// **One decimal, not [`chip_label`]'s two** — `1.08k` is five glyphs and the
/// whole point is to fit where four digits do not. It is the same rounding the
/// tick lattice on that bar already prints, and the band is read against those
/// ticks.
///
/// [`tick_label_thousands`] is the tick's version and takes a `step` to decide
/// its precision; the band has no step, so this one fixes it.
fn chip_label_thousands(position: f64) -> String {
    format!("{}k", trim_zeroes(&format!("{:.1}", position / 1000.0)))
}

/// Which ruler bar a screen position is over, if either — the geometry alone.
///
/// The free-function half of [`OndinApp::ruler_at`], which is the one callers
/// should use: it also asks whether the bars are on screen at all.
///
/// The corner square belongs to the left ruler, because that is the one drawn
/// over it — the answer has to match what the user can see, and the top bar is
/// painted first.
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
    /// and not suppressed by present mode.
    ///
    /// Every reader goes through this rather than `show_rulers` directly. The
    /// bars being absent has to mean the same thing to the hit test as it does
    /// to the paint — otherwise the app claims a press for a bar nobody can see,
    /// which is the same class of mismatch as §15 D35's.
    pub(crate) fn rulers_on(&self) -> bool {
        self.show_rulers && !self.present
    }

    /// Which ruler bar a screen position is over, or `None` — including when the
    /// bars are not on screen at all.
    ///
    /// The method half the free [`ruler_at`] has always claimed to have. Three
    /// callers were asking the same two-part question by hand
    /// (`rulers_on() && ruler_at(..).is_some()`), which is the shape of thing that
    /// stays right until one of them forgets the first half and starts claiming a
    /// press for a bar nobody can see.
    pub(crate) fn ruler_at(&self, p: egui::Pos2, rect: egui::Rect) -> Option<GuideAxis> {
        self.rulers_on().then(|| ruler_at(rect, p)).flatten()
    }

    /// Whether the guide **lines** are drawn: switched on (*View ▸ Show guides*),
    /// and that is the whole of it.
    ///
    /// 🚨 **Present mode is deliberately not folded in here, and this doc used to
    /// argue the opposite** (§15 D750). It read: *"present mode is folded in here
    /// for the same reason it is folded in [`Self::rulers_on`]: present mode
    /// promises to hide every piece of chrome at once, and a guide is chrome by
    /// this module's own definition — it never enters the scene."* The premise was
    /// never true of the code — present mode suppressed **two** overlays of about
    /// a dozen — and the maintainer has now ruled it is not what present mode is
    /// *for*: **it hides the app's chrome, and the canvas goes on working
    /// normally.** Present mode is for editing a large design with the panels out
    /// of the way, or on a laptop screen the panels eat half of; a designer doing
    /// that still wants the guides they are aligning to.
    ///
    /// ⚠️ **"It never enters the scene" is a true sentence that answered the wrong
    /// question.** It distinguishes chrome from *artwork*, and the line present
    /// mode draws is between the app's chrome and the **canvas** — where guides,
    /// layout grids, the selection box, handles and the size badge all sit on the
    /// canvas side, being things you edit *with* rather than parts of the app
    /// around it. [`Self::rulers_on`] keeps its `!self.present` because the bars
    /// are the other kind: furniture at the edge of the window.
    ///
    /// ⚠️ **The bars can be gone while the lines are drawn, and that is intended.**
    /// A guide is still draggable in present mode — this predicate gates the pick
    /// as well as the paint, through [`Self::guides_pickable`] — and what present
    /// mode costs is *creating* a new one, which is a drag off a bar that is not
    /// there. The Guide panel's position field is the way in, exactly as §15 D140
    /// says for the autopan case. ([`Self::guides_pickable`] is the predicate the
    /// pick goes through, and it reads this one.)
    pub(crate) fn guides_on(&self) -> bool {
        self.show_guides
    }

    /// Whether a guide can be picked up at all: drawn, and not locked.
    ///
    /// **Locked means not selectable outright**, which is Adobe's behaviour and
    /// what a *global* lock makes safe — the toggle that locked them is the
    /// toggle that unlocks them, so nothing has to remain reachable through a
    /// locked guide. Hover, the cursor, the press, the drag and the discard all
    /// fall away from this one predicate rather than each testing the flag.
    pub(crate) fn guides_pickable(&self) -> bool {
        self.guides_on() && !self.lock_guides
    }

    /// A point in `owner`'s local space, from a world point. Identity when there
    /// is no owner, since a global guide is already measured in world space.
    ///
    /// `None` only when the owner has gone, which the model does not allow to
    /// happen (`Document::check_guide_owner`) — but the transform lookup can
    /// still answer nothing mid-load, so it is a `None` rather than a panic.
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
    /// A global guide's position already is that coordinate. A scoped guide's is
    /// its offset carried through its frame's transform — which only means
    /// anything while the frame is [`upright`]: in a rotated frame the guide is a
    /// leaning line with no single world x or y, and the honest answer is that
    /// there is nothing to report. That is the whole of "a guide in a rotated
    /// frame draws but does not attract", and it is why `snap.rs` needs no change
    /// at all — the projection happens here, at the collection site, and
    /// `GuideLines` stays plain xs and ys.
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
    /// The counterpart of [`EditorSession::display_node`] for the one piece of
    /// document state the renderer never sees. A node's in-flight edit rides
    /// `RenderOverrides`; a guide has no scene fragment to override, so its two
    /// gestures — a drag on the canvas and a scrub in the inspector — are read
    /// through here instead. Everything that draws or hit-tests a guide goes
    /// through this, so a gesture cannot move the line without also moving what
    /// a click on it will find.
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
            // **Not while the drag is an Alt-copy.** Then this guide is the
            // *original*, which stays exactly where it is; the travelling line is
            // the copy, drawn from the gesture. Without this the original would
            // follow the pointer as well and the copy would be invisible — one
            // line moving where two were promised.
            && !d.copying
        {
            // Owner **and** position, or the two disagree for the length of the
            // drag: the position is read in the owner's space, so a live position
            // read against the committed owner is a number in the wrong space.
            out.position = d.position;
            out.owner = d.owner;
        }
        out
    }

    /// The screen geometry of a guide, or `None` when none of it is in view.
    ///
    /// `inner` is the canvas past both ruler bars, which is what a global guide
    /// spans and what a scoped one's dotted continuation is bounded by. The
    /// *solid* segment of a scoped guide is bounded by its frame instead — both
    /// tests are live at once, and they are different tests.
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
            // Global: the full width or height of the visible canvas, in the
            // screen-space band the ruler counts in.
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
        // Scoped: the frame's own box, spanned in the frame's space and carried
        // through its world transform — so the line rotates with the frame and
        // stops at its edges. Read through the preview layer, so a guide tracks a
        // frame being dragged rather than lagging a commit behind.
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
    /// both sit above the artwork and above the selection chrome — a guide you
    /// cannot see is not a guide, and the ruler must never be painted over.
    pub(crate) fn draw_rulers_and_guides(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        hover: Option<egui::Pos2>,
    ) {
        // Where the artwork actually starts, so a guide's line stops at the
        // ruler rather than running under its ticks. Through the shared helper
        // because `guide_at` measures against the same rectangle — the segment a
        // guide is picked by has to be the segment it was drawn as.
        let inner = self.canvas_inner(rect);
        let hovered = hover.and_then(|p| self.guide_at(p, rect, ppp));

        // The bars first, then the guides: a guide's *chip* sits inside the
        // ruler, so drawing the bars afterwards paints over the very number
        // they are there to show. The lines cannot collide either way — they
        // are confined to `inner`, which is the canvas past both bars.
        if self.rulers_on() {
            self.draw_ruler_bars(painter, rect, ppp, hover);
        }

        // **The lines are gated too, not just the bars.** They used to be drawn
        // unconditionally, so entering present mode hid the rulers and left the
        // guides over the artwork — against what present mode was then taken to
        // promise (§15 D136). Both halves hang off a predicate now.
        //
        // 🚨 **Two different predicates, and this comment claimed one** (§15 D750).
        // It ended *"both halves now hang off predicates that fold present mode
        // in"*; `rulers_on` folds it in and `guides_on` does not, the maintainer
        // having ruled that present mode hides the app's chrome and leaves the
        // canvas working — so the bars go and the lines stay. D136's repair still
        // stands: what it fixed is that the loop was gated by nothing at all, and
        // *View ▸ Hide guides* is the switch that now reaches it.
        if !self.guides_on() {
            return;
        }

        for guide in self.session.doc.guides() {
            // The one being dragged is drawn from the gesture instead, below —
            // otherwise it would appear twice while it moved. **Unless the drag is
            // an Alt-copy**, where the original genuinely does stay put and drawing
            // it is the whole point: the copy is what travels, as in Figma.
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
        // hidden: the gesture has to say what releasing will do, and a line
        // that simply vanished would read as the app having lost it.
        //
        // **The fade is `draw_guide`'s to apply, not this caller's**, and it has to
        // be: a guide in hand is drawn as *selected*, so the ink it ends up with is
        // the selection blue rather than the colour handed in here. Fading the
        // colour on the way in — which is what this used to do — put the alpha on a
        // value the blue then replaced, and the whole discard signal quietly went
        // away the moment selection stopped meaning "same colour, thicker".
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
    /// **Three states, and only the selected one changes the ink.** Resting is 1px
    /// in the guide's own colour; hover is the same 1px line *plus* the chip and
    /// the extension, with no darken and no extra weight — the chip is the
    /// affordance, and Figma's darken-on-hover would have been defeated by a
    /// per-guide colour anyway; selected is 2px in the selection blue, with the
    /// chip.
    ///
    /// **The blue overrides the guide's own colour while it is selected**, and the
    /// colour returns on deselect. This reverses an earlier decision recorded
    /// right here — that a selected guide should thicken rather than recolour,
    /// because the colour is the one thing about a guide the user authors and
    /// announcing selection in it would overrule the property being edited. What
    /// changed is the reading: a selected guide takes the same blue as every
    /// hover outline and bounding box in the app, and a recoloured guide going
    /// blue for as long as it is picked is the *consistent* answer rather than an
    /// exception. Intended, and written down so it is not later filed as a bug.
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
    /// Split only so the ordinary call site does not have to pass a `false` that
    /// means nothing to it — the two are one drawing.
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
        // Snapped on the guide's own axis only. Rounding *along* the line would
        // shorten a frame-scoped guide by up to a pixel at each end for no gain,
        // and on a leaning line it would move the line sideways.
        let snap_across = |p: egui::Pos2| match line.axis {
            GuideAxis::Horizontal => egui::pos2(p.x, at(p.y)),
            GuideAxis::Vertical => egui::pos2(at(p.x), p.y),
        };
        // A leaning guide has no single across-axis coordinate to snap, and
        // nudging its ends independently would tilt it. Left alone; antialiasing
        // is the right answer for a diagonal hairline in any case.
        let leaning = match line.axis {
            GuideAxis::Horizontal => line.a.y != line.b.y,
            GuideAxis::Vertical => line.a.x != line.b.x,
        };
        let (a, b) = match leaning {
            true => (line.a, line.b),
            false => (snap_across(line.a), snap_across(line.b)),
        };

        // Clipped to the canvas past the bars, so a scoped guide's line stops at
        // the ruler rather than running under its ticks. A coordinate test cannot
        // do this job any more: a leaning line crosses the boundary at an angle.
        let clipped = painter.with_clip_rect(inner);
        let stroke = egui::Stroke::new(width, ink);
        clipped.line_segment([a, b], stroke);

        // The continuation: the guide's own line carried on, dashed, so its
        // alignment can be read against things outside its frame without leaving
        // the scope. Exactly the width and colour this state gave the solid part,
        // which is why it takes them from the same two locals rather than having a
        // spec of its own — the extension cannot drift from the line it belongs
        // to.
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
    /// colour — the design's `#a3332d` block with dark type in it, or the
    /// selection blue while the guide is picked.
    ///
    /// Only for the guide the user is holding, hovering or has selected. The
    /// design draws the chip on the one guide it shows, which reads as "always";
    /// with four guides on a page that is four blocks filling the ruler and no
    /// ruler left between them. Tying it to the pointer keeps the design's
    /// picture for the guide you are looking at and gives the number back the
    /// moment you ask for it.
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
        // Both orientations grow along the reading direction. The design's chip
        // is 20×13 and its number is three digits, which is exactly what the
        // clamp below produces — but a guide at -1024.75 has eight glyphs, and
        // a chip that stayed 20pt wide would either hide most of the position
        // it exists to report or spill it over the artwork. The ruler's own
        // labels may be dropped when they will not fit (`tick_number`); the
        // chip may not, because it is the readout for a thing the user is
        // holding.
        let along = galley.size().x + CHIP_PAD_X * 2.0;
        let size = match axis {
            // Across the left ruler's width, 13pt along the guide's own axis.
            GuideAxis::Horizontal => egui::vec2(along.max(THICKNESS), CHIP_THICKNESS),
            // Mirrored: the top ruler's full height.
            GuideAxis::Vertical => egui::vec2(along.max(THICKNESS), THICKNESS),
        };
        // A horizontal guide's chip grows out of the left bar into the canvas,
        // not symmetrically about the bar's midline — it stays anchored to the
        // canvas edge and reads as a tab pulled out of the ruler.
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

        // Opaque, in the chrome's own ground rather than the canvas colour the
        // design happens to show through (they are the same near-black there).
        // The ruler is chrome: its ticks and type are near-white, so letting a
        // light canvas ground or the artwork under it show through would leave
        // the numbers unreadable exactly when the canvas is busiest.
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

        // The world range each bar covers, as offsets from the origin — which is
        // what anchors the tick lattice to it, so with a frame selected `0`
        // lands exactly on the frame's edge rather than a step away from it.
        let world_min = self.to_world(rect.min, rect, ppp) - origin;
        let world_max = self.to_world(rect.max, rect, ppp) - origin;

        for (axis, bar, from, to) in [
            (GuideAxis::Vertical, top, world_min.x, world_max.x),
            (GuideAxis::Horizontal, left, world_min.y, world_max.y),
        ] {
            // Nothing else clips a label to the bar it belongs to: the canvas
            // painter's clip rect is the whole canvas, so an overlong number
            // would be painted in ruler grey over the artwork. This is the hard
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

            // The band under the selection, before the ticks, so the ticks read
            // on top of it.
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
                // A tick label that would sit under one of the band's own
                // numbers is dropped: the band's are the ones being asked for,
                // and two numbers overprinted are worse than either alone.
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
                // How much bar is left beside the label's own start. On the top
                // bar that is the rest of the canvas, so this also drops the
                // number on the last tick rather than slicing it at the edge;
                // on the left bar it is the 17pt between the design's 3pt inset
                // and the inner hairline, which is where numbers run out of
                // room.
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

            // And the pointer, marked on both bars — Photoshop, Illustrator and
            // Figma all do it, and it is the cheapest readout in the app.
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
    /// **Suppressed while a guide drag is in flight.** That gesture already puts a
    /// chip in the very bar this would draw into, and the chip carries the
    /// number — so a second moving mark on the same bar would be the app saying
    /// the same thing twice, in the one moment the bar is busiest. Every other
    /// drag keeps it: a move or a resize wants to be read off the ruler, which is
    /// most of what the mark is for.
    fn pointer_mark(&self, hover: Option<egui::Pos2>) -> Option<egui::Pos2> {
        match self.drag {
            Drag::Guide(_) => None,
            _ => hover,
        }
    }

    /// One of the band's end coordinates, called out over the ordinary ticks.
    ///
    /// Brighter than a tick's number and in the accent's light tier, because
    /// these two are the reason the band is there: with a frame selected they
    /// read `0` and the frame's own width, which is the number the inspector's
    /// W field holds. Drawn even when the tick lattice has no label there —
    /// nothing else would say where the selection ends.
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
        // ⚠️ **The room *across* the bar, which is a different question from
        // running off the end of it** — and the one test below used to be both
        // (§15 D547). `[S19.1-L1-03]`: on the **left** bar `pos.x` is fixed at
        // `bar.left() + LABEL_INSET`, so `pos.x + w > bar.right()` is a *width*
        // test worth `THICKNESS - LABEL_INSET` = 17pt — not the "runs past the
        // viewport" test the comment below describes. Four digits at 7.5pt come
        // to 17.25, so **a 1920×1080 frame's band read `0` and then nothing at
        // all** where it should have said where the selection ends. The ticks
        // beside it were printing `1k` and `1.2k` in the same frame.
        //
        // **The ladder is `tick_number`'s, and this function had none.** Plain
        // form, then thousands, then nothing — §15 D33 scopes that to ticks and
        // nothing covered this third readout on the same bar, so it had silently
        // inherited the least useful half of it. **Nothing rather than a
        // truncated number** still holds, for D33's reason: `1000` sliced at the
        // hairline reads as a different number.
        let across = match axis {
            // The top bar reads along its length; its constraint is the end of
            // the bar, tested below, and it has never been short of width.
            GuideAxis::Vertical => f32::INFINITY,
            GuideAxis::Horizontal => bar.right() - pos.x,
        };
        // ⚠️ **The `>= 1000.0` guard is load-bearing here for a *second* reason,
        // and `tick_number`'s comment gives only the first.** There it is a
        // wasted-layout guard — below a thousand `tick_label`'s short form *is*
        // its long form. Here it is not: `chip_label` keeps two decimals, so
        // `512.37` is five digits and a point where four digits alone already
        // overflow 17pt, and its thousands form would be `0.5k` — a number
        // rounded past its own magnitude, which is worse than the blank.
        //
        // **So there is a deliberate residue**: a *fractional* coordinate under
        // a thousand whose plain form overflows still draws nothing. Far rarer
        // than the case D547 fixed, and chosen. **Do not widen this to
        // "whenever the plain form does not fit" without deciding that second
        // question.**
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
    /// The world origin, unless a **single frame** is selected: a frame is a
    /// content holder, so while one is the subject the rulers read in its own
    /// coordinates and `0` sits on its leading edge (Figma).
    ///
    /// Only a frame, and only on its own. Figma also re-origins for a shape
    /// *inside* a frame, and this deliberately does not: this editor's inspector
    /// shows X/Y in **world** space by decision (`panels::inspector` — "world
    /// in, local out"), so a ruler counting from the frame while the panel
    /// counts from the origin would have the two disagreeing about where the
    /// same shape is. With the frame itself selected there is no such clash —
    /// the band then reads `0` to the frame's width, which is exactly the `W`
    /// the inspector is showing.
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
    /// Read from the preview layer, so the band tracks a live drag or resize
    /// rather than snapping to the committed box a frame late.
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
    /// The left bar gives a label `THICKNESS - LABEL_INSET` = 17pt to read
    /// across, and four digits at 7.5pt already come to 17.25, so the plain
    /// number stops fitting at around a thousand — which is every zoom of 50%
    /// or less, and any pan past a thousand units at the design's own zoom. So
    /// it falls back to the thousands form (`20000` → `20k`) and, if that will
    /// not fit either, to no label at all.
    ///
    /// **Nothing, rather than a truncated number.** The clip above guarantees
    /// an overlong label cannot escape onto the artwork, but a number sliced
    /// off at the hairline is still a *different number* — `20000` clipped
    /// reads as `2000` — and being read is the ruler's entire job. A tick
    /// standing on its own says "a step further" and cannot be misread; its
    /// neighbours say how far a step is.
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
        // Only worth trying when there are thousands to fold away; below that
        // the short form is the long one and this is a wasted layout.
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

/// Type on a guide chip. The chip's ground is the guide's colour — a mid-dark
/// red by default — so the chrome's near-white would sit at too low a contrast;
/// the design uses a near-black instead.
const CHIP_INK: egui::Color32 = egui::Color32::from_rgb(0x0d, 0x1c, 0x26);

/// The pointer's mark on a ruler bar: full thickness, brighter than a tick and
/// dimmer than an extent number.
///
/// A full-height line rather than a longer tick, because it has to be findable
/// while it *moves* — a 6pt tick among a lattice of 6pt ticks is exactly the mark
/// the eye cannot follow. No number beside it: the bar's own ticks say where it
/// is, the inspector says where the selection is, and a number tracking the
/// pointer would be a third readout competing with the guide chip for the same
/// 20pt of bar.
const POINTER_MARK: egui::Color32 = theme::color::text_a(120);

// --- interaction ------------------------------------------------------------

impl OndinApp {
    /// Guide and ruler handling, ahead of everything else the canvas does with
    /// a press. Returns whether it took the event.
    ///
    /// It runs first because a guide is drawn on top: whatever is under the line
    /// is under the *chrome*, and clicking chrome hits the chrome. The one thing
    /// that outranks it is the **selection chrome** — which is chrome too and is
    /// the smaller, more deliberate target of the two — and `on_handle` is the
    /// caller's answer to whether the pointer is on any of it.
    ///
    /// ⚠️ **This said "a selection handle" and that was the whole of §15 D486.**
    /// `canvas::begin_select_drag`'s press ladder has four rungs above the
    /// artwork — the pivot marker, a line's endpoints, the rail handle, then the
    /// box handles — and the gate asked only about the last, so a guide lying
    /// across any of the other three took a press the cursor had just promised
    /// to it. The **call site** was corrected first and this doc was left
    /// carrying the narrow word for another day: the same tell as the defect,
    /// two passages using one word for two different sets. `canvas::chrome_claims`
    /// is what `on_handle` is now.
    ///
    /// **The gesture is claimed on the press, and hit-tested at the press
    /// origin.** Both halves matter, and getting either wrong makes a guide
    /// almost impossible to pick up.
    ///
    /// Claiming it at `drag_started` instead means waiting for egui to decide
    /// the press is a drag, which it only does once the pointer has travelled
    /// several points. Hit-testing at `interact_pointer_pos` then asks where the
    /// pointer is *now* — several points from where it went down, which for a
    /// [`GUIDE_PICK_PT`] band is usually off the guide entirely, and for the
    /// ruler is off the bar whenever the drag went the natural way, straight out
    /// of it. The press fell through to a marquee while the cursor, which
    /// hit-tests on hover and so never saw the discrepancy, went on promising a
    /// drag. `press_origin` is where the user actually aimed, and it is fixed
    /// for the whole press.
    pub(crate) fn guide_input(
        &mut self,
        resp: &egui::Response,
        rect: egui::Rect,
        ppp: f32,
        on_handle: bool,
    ) -> bool {
        // Shift is read for **one** thing here: adding a guide to the selection,
        // the way it adds a layer. It still does not suppress the snap — that is a
        // constraint everywhere now, and a guide has one axis and so nothing to
        // constrain (§15 D47, [`Self::update_guide_drag`]).
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
            // Ended by the button coming up, not by `drag_stopped`: the gesture
            // is armed before egui calls it a drag, so on a press that never
            // travelled that far `drag_stopped` never fires and the guide would
            // stay glued to the pointer for the rest of the session.
            if !down {
                self.finish_guide_drag();
            }
            return true;
        }

        // Some other gesture already owns the pointer — a marquee dragged up
        // over the ruler is still a marquee, and must not be swallowed here on
        // the way past.
        if !self.drag.is_none() || on_handle || !pressed || !resp.contains_pointer() {
            return false;
        }
        let Some(p) = origin.or_else(|| resp.interact_pointer_pos()) else {
            return false;
        };

        // Pulling a fresh guide out of a ruler. Whatever the tool: the ruler is
        // chrome, so a press on it is a press on the app rather than on the
        // canvas, and dragging a rectangle out of the ruler bar was never what
        // anyone meant.
        //
        // **The bars keep working while the guides are locked or hidden.** The
        // lock exists to stop existing placements being nudged, not to block the
        // feature — Photoshop puts the same switch in its own View menu — and a
        // new guide simply lands locked.
        if let Some(axis) = self.ruler_at(p, rect) {
            // **Hidden is the one case that has to answer back, and it answers on
            // the press.** Dragging out of the ruler is an unambiguous request for
            // a guide, and delivering an invisible one is delivering nothing —
            // Photoshop re-shows its extras for the same reason. On the press
            // rather than on the release, so the line is visible *while it is being
            // placed*: switching at the release would have the gesture draw nothing
            // until the moment it ended, which is the one frame it no longer
            // matters. A drag cancelled back onto the ruler leaves the guides
            // shown, which is the honest reading of having asked for one.
            //
            // The lock is deliberately not treated this way: locked guides are
            // still on screen doing their job.
            if !self.show_guides {
                self.show_guides = true;
                self.session.info("Guides shown");
            }
            self.begin_guide_drag(None, axis, p, rect, ppp);
            return true;
        }

        // The guide *lines* only answer the tools that manipulate what is already
        // there. They lie over the artwork, and a pen or shape tool aimed at the
        // artwork under one should draw there rather than pick up the marker.
        if !self.tool.selects() {
            return false;
        }
        // Selecting on the press rather than on the release, because the press
        // is also what starts the move: a guide has to be picked up and shown as
        // picked up in the same instant, as a layer's handles are.
        if let Some(id) = self.guide_at(p, rect, ppp)
            && let Some(axis) = self.session.doc.guide(id).map(|g| g.axis)
        {
            // Shift adds to the selection, as it does on a layer. Adding does not
            // start a move: with several guides picked, dragging one of them would
            // have to move all of them, and they do not share an axis — so the
            // press that *extends* a selection is a click and nothing more.
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
    /// **The one early return the lock needs.** Hover, the cursor, the press, the
    /// drag and the discard all reach a guide through here, so a locked guide
    /// falls out of every one of them at once rather than each of the five
    /// remembering to ask. The same is true of hidden, for `rulers_on`'s reason:
    /// the hit test has to mean what the paint means, or the app claims a press
    /// for a line nobody can see.
    ///
    /// Those five are the things that *act* on a guide, and they are the whole of
    /// this function's constituency. The measure overlay is the sixth reader and goes
    /// round it through [`Self::guide_under`] on purpose (§15 D202) — so if you are
    /// here wondering why a locked guide still reports a distance, that is where it
    /// went.
    pub(crate) fn guide_at(&self, p: egui::Pos2, rect: egui::Rect, ppp: f32) -> Option<GuideId> {
        if !self.guides_pickable() {
            return None;
        }
        self.guide_under(p, rect, ppp)
    }

    /// The guide nearest `p` within [`GUIDE_PICK_PT`], **asking nothing about what
    /// may be done with it**.
    ///
    /// Split out of [`Self::guide_at`] so that the two things a pointer can want from
    /// a guide can want different guides. `guide_at` is about *acting* on one and
    /// keeps the lock check above; measuring is about *reading* one and must not,
    /// because the app already lets a locked guide **attract** a dragged shape
    /// (`guide_lines` gates on `guides_on` and never on the lock). A guide that pulls
    /// a layer onto itself and then declines to say how far away it was is the same
    /// predicate answered two ways — see [`Self::measure_guide`] and §15 D202.
    ///
    /// Both callers still refuse the ruler band, and for a reason neither can drop:
    /// that band is the ruler's, and a guide's grab region overlapping it would take
    /// presses meant for pulling out a new one.
    fn guide_under(&self, p: egui::Pos2, rect: egui::Rect, ppp: f32) -> Option<GuideId> {
        if self.ruler_at(p, rect).is_some() {
            return None;
        }
        // `inner` as `draw_rulers_and_guides` computes it, so the segment a guide
        // is picked by is the segment it was drawn as.
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
    /// Here rather than in canvas.rs because both halves of the answer are this
    /// module's private business — [`Self::shown_guide`], so a guide mid-drag is
    /// measured to where it *looks like it is*, and [`Self::guide_world_coord`],
    /// which is the only thing that knows a frame-scoped guide's position is local.
    ///
    /// **Its `None`s are `guide_lines`' `None`s, not `guide_at`'s**, and that is the
    /// point of it going through [`Self::guide_under`] rather than
    /// [`Self::guide_at`]: what may be *measured* is what may be **snapped to**, not
    /// what may be picked up.
    ///
    /// So a **locked** guide is measured to. It was not, until the user pointed out
    /// that it could not be (§15 D202), and the reason the old behaviour was wrong is
    /// not a preference: `guide_lines` gates on `guides_on` and never on the lock, so
    /// a locked guide already pulls a dragged shape onto itself. One that attracts a
    /// layer and then refuses to say how far away it was is one predicate answered two
    /// ways. Locking exists to stop a guide being *changed* — moved, selected,
    /// deleted — and measuring is the one thing in the app that changes nothing at
    /// all, so it is the last thing a lock should reach.
    ///
    /// Hidden is still `None`, through [`Self::measurable_guide`], for `guide_lines`'
    /// own reason: a line that is not on screen cannot be what the user is aiming at.
    /// And a guide in a rotated frame has no world x or y to quote — it **draws but
    /// cannot be measured to**, exactly as it draws but does not attract.
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
    /// The two directions of the overlay meet here rather than each doing this
    /// lookup: a selected layer hovering a guide and a selected guide hovering a
    /// layer must agree about where a guide is, and the interesting part of that is
    /// the `None`s, which are easy to get right once and easy to forget the second
    /// time.
    ///
    /// **`guides_on` is checked here and not left to `guide_at`**, which is the one
    /// thing the by-id path cannot inherit. *View ▸ Hide guides* does **not** clear
    /// the guide selection — only *Lock guides* does (`set_guides_locked`) — so a
    /// selected guide can be invisible, and without this the overlay would measure
    /// from a line nobody can see to a layer under the pointer. That is the same
    /// argument `guide_lines` makes for hidden guides not attracting: a line that is
    /// not on screen cannot be what the user is aiming at.
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
    ///
    /// Spelled once because the paint and the hit test must agree on it, which is
    /// the same reason [`Self::rulers_on`] exists.
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
    /// The rule `create_artboard` already uses — "the root, or the frame it was
    /// drawn inside" — with one difference that matters: the containment test goes
    /// through the frame's inverse world transform, so a **rotated** frame's true
    /// rectangle is what counts rather than the axis-aligned box around it. A drop
    /// in the corner of that box is outside the frame and has to stay global;
    /// picking artwork already works this way (§15's picking-allowance entry).
    ///
    /// `artboards()` is depth-first in child order, which is paint order, so the
    /// last match is the innermost frame under the pointer.
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
        // An existing guide keeps its own scope until the pointer leaves it; a new
        // one takes whatever is under the press, which for a ruler drag is
        // whatever the bar happens to sit over and is corrected on the first move.
        let owner = match held {
            Some(g) => g.owner,
            None => self.frame_at_world(self.to_world(p, rect, ppp)),
        };
        // The world offset from the pointer to the line — see `GuideDrag::grab`.
        // An existing guide keeps exactly the position it had until the pointer
        // moves; a new one has none to keep and appears under the pointer.
        let grab = held
            .and_then(|g| self.guide_grab(&g, p, rect, ppp))
            .unwrap_or(Vec2::ZERO);
        self.drag = Drag::Guide(GuideDrag {
            id,
            axis,
            owner,
            // Unsnapped at the press: a guide that jumped to a nearby edge the
            // instant it was picked up would look like a misplaced click.
            position: self.guide_coord(axis, owner, p, rect, ppp, grab, false),
            grab,
            // A new guide starts inside the ruler by definition; releasing it
            // there without ever leaving is how you cancel the gesture you
            // began by accident.
            discarding: id.is_none(),
            // Whether the press itself was off the bar — which is the same test
            // `update_guide_drag` keeps applying, so a new guide arms its
            // announcement the moment it leaves and an existing one is armed from
            // the start. Not `id.is_some()`: one rule, evaluated the same way at
            // the press and on every frame after it.
            left_ruler: self.ruler_at(p, rect).is_none(),
            // Read on every frame from here (`update_guide_drag`), so Alt can be
            // taken or released mid-drag as it can on a layer. Never on a *new*
            // guide: there is no original to leave behind, so Alt would be a
            // modifier that changed nothing.
            copying: false,
            // Latched here for `GuideDrag::rulers_shown`'s reason: the bars'
            // existence is a live chord, and `update_guide_drag` has to keep
            // asking the question the press answered.
            rulers_shown: self.rulers_on(),
        });
    }

    /// The world vector from the pointer to the nearest point on a guide's line.
    ///
    /// Built by taking the pointer into the guide's own space, replacing the
    /// coordinate the guide pins, and coming back out — which is exact for any
    /// transform, rotated frames included, and needs no case analysis. What comes
    /// back is a world vector precisely so that it survives the guide's space
    /// changing mid-drag.
    fn guide_grab(&self, guide: &Guide, p: egui::Pos2, rect: egui::Rect, ppp: f32) -> Option<Vec2> {
        let pointer = self.to_world(p, rect, ppp);
        let local = self.guide_local(guide.owner, pointer)?;
        let on_line = match guide.axis {
            GuideAxis::Horizontal => Point::new(local.x, guide.position),
            GuideAxis::Vertical => Point::new(guide.position, local.y),
        };
        Some(self.guide_world(guide.owner, on_line)? - pointer)
    }

    /// **Shift does not suppress the snap here, deliberately.** It used to, which
    /// was the same meaning it had on a move and a resize at the time. It no
    /// longer has that meaning anywhere: Shift is a constraint, and snapping is
    /// the Snap menu's to switch off (§15 D47). A guide has one axis and therefore
    /// nothing to constrain. What Shift *does* mean on a guide is adding one to
    /// the selection ([`Self::guide_input`]), which is a press and not a drag.
    ///
    /// **Alt does mean something**: it makes the drag place a copy and leave the
    /// original where it was, as it does on a layer. Read live, so it can be taken
    /// or released part-way through.
    fn update_guide_drag(&mut self, p: egui::Pos2, rect: egui::Rect, ppp: f32, alt: bool) {
        let Drag::Guide(d) = &self.drag else { return };
        let (axis, grab) = (d.axis, d.grab);
        // Only an existing guide can be copied — a new one has no original to
        // leave behind, so Alt there would be a modifier that changed nothing.
        let copying = alt && d.id.is_some();
        // The scope follows the pointer, so crossing a frame's boundary clips the
        // line to that frame as it happens. The *point* being held is in world
        // space, so nothing jumps when the space the number is read in changes.
        let owner = self.frame_at_world(self.to_world(p, rect, ppp) + grab);
        let position = self.guide_coord(axis, owner, p, rect, ppp, grab, true);
        // Only the ruler this guide belongs to discards it: dragging a
        // horizontal guide sideways past the left ruler is just dragging it.
        // ⚠️ **The latched `d.rulers_shown` and not `self.rulers_on()`** (§15
        // D555, `[S19.1-L1-02]`). That call is `show_rulers && !present`, both
        // halves are live chords with no drag guard on the dispatch, and this is
        // the *only* term feeding `d.discarding` — so `Shift+R` or `Ctrl+\` held
        // down mid-drag turned the cancel gesture into a create gesture, silently
        // and with the cursor still promising the opposite.
        let over_own_ruler = d.rulers_shown && ruler_at(rect, p) == Some(axis);
        // 🚨 **The latched `d.rulers_shown` here too, and the first version of
        // §15 D555 got this half wrong.** It fixed `over_own_ruler` — the term
        // that decides the **commit** — and left this one reading
        // `self.ruler_at`, which is `rulers_on().then(…)` and therefore folds the
        // same two live chords. So taking the bars away mid-drag answered *"the
        // pointer is off both rulers"* on the frame they vanished, latching
        // `left_ruler` **without the pointer ever having left one** — and
        // `announces_discard()` then drew the faded line and the bin cursor over
        // a *new* guide that does not exist yet, which is precisely the state
        // D140 added `left_ruler` to prevent, reached by the other door. Nothing
        // committed differently; the gesture merely lied about what it would do.
        // `arch-scribe` found it reading the fix against the field's own contract.
        //
        // Off **both** bars, not merely off its own — and the corner square counts
        // as *on* them, since `ruler_at` gives it to the left ruler. So a guide
        // dragged along a bar, through the corner and back has plainly not been
        // placed yet and still announces nothing.
        //
        // **With the latch, this is a pure geometry test against the layout the
        // press saw**, which is what the paragraph it replaces was asking for: the
        // bars' *region* does not move, so "is the pointer off the bar" stops
        // depending on whether the bar is currently painted. Rulers off at the
        // press makes it unconditionally true, which is right — the pointer is
        // off bars that are not there, and `begin_guide_drag` seeds `left_ruler`
        // the same way.
        let off_rulers = !d.rulers_shown || ruler_at(rect, p).is_none();
        if let Drag::Guide(d) = &mut self.drag {
            d.owner = owner;
            d.position = position;
            d.discarding = over_own_ruler;
            // Latched, never cleared: the question is "has this guide ever been
            // out there", so coming back to the bar is what the announcement is
            // *for* rather than something that disarms it.
            d.left_ruler |= off_rulers;
            d.copying = copying;
        }
    }

    /// The coordinate a pointer position puts a guide of `axis` at, **in
    /// `owner`'s space**, offset by `grab` and snapped onto nearby shape edges
    /// unless `raw`.
    ///
    /// **`grab` is added before the snap, not after.** Snapping is a statement about
    /// where the *line* is going to sit, so it has to be measured from the line
    /// rather than from the hand holding it — otherwise a guide grabbed 3pt off
    /// centre would snap when the pointer reached an edge and sit 3pt past it.
    ///
    /// **The snap happens in world space and comes back**, because that is the only
    /// space the targets are in. A guide in a rotated frame therefore does not
    /// snap at all — [`Self::guide_world_coord`] has no coordinate to offer for one
    /// — which is the same predicate that stops it attracting other things and
    /// suppresses its dotted extension. Degrading the snap rather than the data is
    /// the trade (§5.5, §9.4): the guide still draws, still moves and still saves,
    /// where Figma deletes a frame's guides outright when the frame is rotated.
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
        // The world point being held, which is space-independent — so a scope
        // change re-reads the same point rather than moving the line.
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
        // Nothing is excluded from the shape targets: a guide is not a node, so
        // it has no self to skip and no ancestors to be stuck to — the whole
        // visible page is fair game.
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
        // Back into the owner's space, through the matrix rather than by adding
        // the difference: an upright frame may still be *mirrored*, and a mirror
        // flips the sign of the offset the two coordinates differ by.
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
                    // **A new guide is not selected while the guides are locked.**
                    // Locked means not selectable (`guide_at`), so leaving the
                    // selection in place would produce a guide that is *selected
                    // but unselectable*: drawn 2px blue, and impossible to pick up
                    // again once anything else was selected. This is the one line
                    // where the lock's "new guides still work" decision bites.
                    if !self.lock_guides {
                        self.session.selection.set_guide(guide.id);
                    }
                }
            }
            // Pulled out and put straight back: nothing happened, and nothing
            // is written to history for it.
            (None, true) => {}
            // Dropped on the ruler while copying: the **copy** is thrown away and
            // the original is untouched. Not `remove_guide` — an Alt-drag was never
            // asking to delete anything, and the original has not moved.
            //
            // The bin cursor and the fade are right here without a term of their
            // own, and that is worth saying because it looks like an omission: the
            // announcement is always about *the line in flight*, and the line in
            // flight is exactly what a release over the ruler discards. For a
            // clone that line is the copy.
            (Some(_), true) if d.copying => {}
            (Some(id), true) => self.remove_guide(id),
            // Alt held: the original stays and the copy lands where the line was
            // dragged to, in whatever scope it was dragged into.
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
                    // The copy becomes the selection, as it does for a layer's
                    // Alt-drag and for Ctrl+D — so the thing under the pointer is
                    // the thing the inspector is describing.
                    if !self.lock_guides {
                        self.session.selection.set_guide(guide.id);
                    }
                    self.session.info("Guide copied");
                }
            }
            (Some(id), false) => {
                // A press that never moved is a selection, not an edit. The
                // *scope* counts as having moved too — dragging a guide into a
                // frame without shifting its line is still a change, and that
                // term is load-bearing: without it the scope change commits
                // nothing and is silently lost.
                //
                // ⚠️ **This is no longer what stops the undo step, and it read as
                // if it were until 2026-09-07** (§15 D474). `[A5-L6-04]` named
                // `let moved = true;` as a mutation that would put a step in the
                // history for every selecting click; **run, it changes nothing** —
                // §15 D428's guard in `EditorSession::commit_inner` already drops
                // a transaction whose every operation writes back the value that
                // is already there, and a `SetGuideScope` to the same owner and
                // the same position is exactly that. What this still buys is the
                // op not being built, and a statement of the rule at the site
                // that has it. The finding was written against a tree where D428
                // had already landed.
                let was = self.session.doc.guide(id).copied();
                let moved = was.is_some_and(|g| g.position != d.position || g.owner != d.owner);
                if !moved {
                    return;
                }
                // One op, not two: the position is read in the owner's space, so a
                // transaction that changed one and then the other would pass
                // through a state where the number meant something else — and
                // build its inverse from that state (`Operation::SetGuideScope`).
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
    /// Only the component on each guide's own axis is used: a horizontal guide
    /// is a `y`, so Left and Right have nothing to say to it and do nothing at
    /// all. Committed as **one** transaction, so a nudge over three guides is one
    /// press of Ctrl+Z — the same rule the removal path follows.
    ///
    /// ⚠️ **And a *burst* of taps is one press too, since §15 D482** — the axis
    /// this doc's own rule was missing. It said "one press of Ctrl+Z" and meant it
    /// across guides, while a held arrow key put an entry on the stack per repeat.
    ///
    /// **The step is applied in the guide's own space, unscaled**, which is right
    /// because a transform carries no scale (§5.6): a nudge of one unit moves a
    /// frame-scoped guide one world unit, as it does a global one. A frame with a
    /// *skew* over it is the exception and is not corrected for — the arrow keys
    /// are a coarse tool and the guide is already declining to snap there.
    /// ⚠️ **And it refuses while the guides are not on screen** (§15 D541).
    /// `[S19.1-L2-01]`: the guide selection survives *View ▸ Hide guides* —
    /// `set_guides_locked` clears it and nothing else does, and nothing may,
    /// because `measurable_guide` **relies** on it surviving — so four presses of
    /// `↓` moved a hidden guide `100.0 → 104.0`, took `undo_depth` `1 → 5` and
    /// marked the document dirty (autosave *and* the crash snapshot) while
    /// `draw_rulers_and_guides` emitted **0 shapes**. It was measured over two
    /// switches, that one and present mode, and they gave identical numbers,
    /// which is the tell that the root is `guides_on()` rather than present mode.
    ///
    /// ⚠️ **Present mode is no longer one of the two** (§15 D750): it leaves the
    /// guides on the canvas, so it can no longer reach this state at all. The
    /// measurement stands and the second route to it is gone — which is the
    /// finding's own conclusion arriving from the other side, the root having
    /// been `guides_on()` all along.
    ///
    /// **`guides_on`, not `guides_pickable`.** The lock is already handled and
    /// handled differently: locking *clears* the guide selection, so a locked
    /// guide can never reach here at all. What is left is the pair of switches
    /// that hide without clearing.
    ///
    /// **Not in `guides_pickable`'s constituency, and that is the point.** That
    /// predicate's doc enumerates *"hover, the cursor, the press, the drag and
    /// the discard"* — every one of which reaches a guide through `guide_at`.
    /// These two verbs reach one through the **selection**, so an enumeration
    /// scoped to the pointer could never have covered them (G15). The principle
    /// is written down one file away for a strictly weaker case: the settings
    /// modal's keyboard gate exists because *"an arrow nudging the selection …
    /// none of which the user can see happening"*, and `rename_selection`
    /// refuses in present mode on the argument *"armed invisible state is worse
    /// than a refused chord"*.
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
            // **A burst of taps is one nudge, so it is one undo step** (§15 D482),
            // which is what the layer arm of `OndinApp::nudge` has always done and
            // what this one did not: a two-second hold of `↓` is ~45 presses at the
            // Windows repeat rate, and each was its own `commit`. Getting back where
            // you started was 45 presses of `Ctrl+Z`, against one for the same
            // keystroke on a layer.
            //
            // Its own verb, so a guide nudge can never merge into a layer nudge
            // — they are different subjects and the run window is per verb.
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
    /// All of them, not just the ones in view: a document has a handful of
    /// guides where it has thousands of nodes, so the culling `snap::targets`
    /// needs would cost more than the comparison it saves — and the tolerance
    /// check throws out anything far away regardless.
    ///
    /// Read through [`Self::shown_guide`], so a guide being dragged snaps
    /// against where it *looks like it is*, not where the document still says.
    ///
    /// **A scoped guide is projected here, and only where that means something.**
    /// `GuideLines` is plain xs and ys and `snap.rs` knows nothing about the
    /// document — which is exactly why frame-local guides and coordinate-only
    /// snapping coexist with no change to the comparison: the projection happens
    /// at the collection site. A guide in a rotated frame has no world x or y and
    /// contributes nothing, so it **draws but does not attract**
    /// ([`Self::guide_world_coord`]).
    ///
    /// Hidden guides contribute nothing either. *Show guides* and *Snap to guides*
    /// are separate switches, but a line that is not on screen cannot be what the
    /// user is aiming at, and a shape jumping to an invisible one reads as a bug
    /// rather than as a snap.
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
    /// Answered for **every** tool, because the ruler claims the press from
    /// every tool ([`Self::guide_input`]) — a crosshair over a bar that will
    /// not draw anything is the cursor telling the user the wrong thing.
    ///
    /// **A drag that is about to throw the guide away says so.** This used to
    /// answer `axis_cursor` whatever the drag was doing — the ordinary move cursor
    /// — so the only signal that releasing would *delete* the guide was the line
    /// being drawn faded, which reads just as easily as the line being under the
    /// ruler. `discarding` is computed in one place
    /// ([`Self::update_guide_drag`]), so the cursor is one more branch off it and
    /// cannot disagree with the fade.
    pub(crate) fn ruler_cursor(
        &self,
        p: egui::Pos2,
        rect: egui::Rect,
    ) -> Option<(egui::CursorIcon, Option<Bitmap>)> {
        if self.discarding_guide() {
            // `NoDrop` as the stock fallback, which is the nearest thing the
            // platform has — though it says the wrong thing on its own (the drop is
            // accepted; it is *destructive*), which is why the bin is drawn.
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
    /// [`GuideDrag::announces_discard`], not the raw `discarding` flag: a guide
    /// being pulled out of a ruler is over that ruler at the press, and announcing
    /// a discard there put the bin cursor up the instant the button went down on a
    /// guide that did not exist yet.
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

    /// The design freezes one zoom: 100 world units every 51.94px, at 51.94%.
    /// The step function has to agree with it there, or the ruler in the app is
    /// not the ruler that was designed.
    #[test]
    fn the_step_matches_the_design_at_the_designs_zoom() {
        assert_eq!(nice_step(0.5194), 100.0);
    }

    /// And stays inside the 1/2/5 ladder either side of it, always leaving at
    /// least `MIN_LABEL_PX` between labels.
    #[test]
    fn the_step_is_the_smallest_one_five_ladder_rung_that_fits() {
        for (points_per_unit, want) in [
            (0.01, 5000.0),
            (0.1, 500.0),
            (1.0, 50.0),
            // 50/2 = 25, which is not a rung: 20 would crowd the labels, so it
            // has to round *up* to 50 rather than to the nearer number.
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

    /// Zoomed in past whole units the labels need decimals, and the origin is
    /// never spelled `-0`.
    #[test]
    fn labels_carry_enough_decimals_and_never_read_minus_zero() {
        assert_eq!(tick_label(300.0, label_decimals(100.0)), "300");
        assert_eq!(tick_label(-100.0, label_decimals(100.0)), "-100");
        assert_eq!(tick_label(0.5, label_decimals(0.5)), "0.5");
        assert_eq!(tick_label(0.25, label_decimals(0.05)), "0.25");
        assert_eq!(tick_label(-0.0, label_decimals(100.0)), "0");
    }

    /// The chip quotes the exact position, so a guide at a half unit says so —
    /// and a whole one still reads as a whole number.
    #[test]
    fn a_chip_shows_the_position_it_actually_has() {
        assert_eq!(chip_label(332.0), "332");
        assert_eq!(chip_label(12.5), "12.5");
        assert_eq!(chip_label(-48.25), "-48.25");
        assert_eq!(chip_label(-0.0), "0");
        // Beyond two decimals it rounds rather than growing without limit; the
        // chip has a ruler's width to live in.
        assert_eq!(chip_label(1.0 / 3.0), "0.33");
    }

    /// An egui context with the theme's fonts loaded, for the two tests below
    /// that have to measure real type.
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

    /// The left bar gives a label 17pt to read across, and four digits at 7.5pt
    /// already come to 17.25 — so the plain number stops fitting at around a
    /// thousand, which is every zoom of 50% or less. This pins the sizes that
    /// make that true, and the thousands fallback that answers it: the ruler
    /// must never be in a position where the only options are a number over the
    /// artwork or a number sliced in half.
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

    /// **The selection band's own numbers have the same fallback the ticks
    /// beside them have** — `[S19.1-L1-03]`, §15 D547.
    ///
    /// `draw_extent_number` laid the plain form and bailed if it did not fit, so
    /// on the **left** bar — where a label has 17pt to read across and four
    /// digits come to 17.25 — a 1920×1080 frame's band read `0` and then
    /// **nothing at all**. The module's own doc calls those two numbers the
    /// reason the band exists: *"Drawn even when the tick lattice has no label
    /// there — nothing else would say where the selection ends."* That sentence
    /// was false for every value over about a thousand, while the lattice in the
    /// same frame happily printed `1k` and `1.2k`.
    ///
    /// ⚠️ **`999` is the control and it is the one that would catch the
    /// over-broad fix**: a version that always used the thousands form would
    /// print `1k` for the frame *and* satisfy every assertion above it. The band
    /// must still read a plain number when a plain number fits.
    ///
    /// ⚠️ **`1920` is the second control, and it is why the ladder is a ladder.**
    /// It is exactly 17.00pt — it fits — so the plain form must win for it. A
    /// fix that switched on `value >= 1000` rather than on measured width would
    /// turn the *top* number of a 1920-wide frame into `1.9k` while the bottom
    /// one stayed `1080`, which is worse than the bug.
    ///
    /// **This asserts the label arithmetic and the widths, not the drawing.**
    /// What `draw_extent_number` does with them is one `if`; what makes the bug
    /// possible is that `chip_label(1080)` is wider than the bar, and that is
    /// what is pinned here. The three-decimal case is included because
    /// `chip_label` keeps two decimals and the thousands form must not.
    ///
    /// ⚠️ **Flipped** by deleting the `chip_label_thousands` arm from
    /// `draw_extent_number`: this test stays green — **and so did all 1066** —
    /// because it is about the labels rather than the draw. That is where the
    /// teeth are *not*, and it is what
    /// `the_left_bars_band_says_where_the_selection_ends` below was written for.
    ///
    /// (Plain backticks rather than `[links]`, §15 D319's convention.)
    #[test]
    fn the_selection_bands_number_has_a_short_form_that_fits_the_left_bar() {
        let ctx = shaped();
        let room = THICKNESS - LABEL_INSET;

        // The measurements the fallback exists for — the finding's own table.
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

    /// **The left bar's band actually paints the number** — the half the test
    /// above cannot reach, and the reason it exists (§15 D547).
    ///
    /// Deleting `draw_extent_number`'s thousands arm left **all 1066 tests
    /// green**, including the label test above, because that one asserts on
    /// `chip_label_thousands` and this function is the only caller. So the
    /// decision — plain form, else thousands, else nothing — was pinned by
    /// nothing at all. This drives the real function and reads the galley back
    /// out of `.shapes`.
    ///
    /// **The three cases are the ladder's three rungs**, and each is a different
    /// answer rather than three samples of one: `999` takes the plain form,
    /// `1080` falls through to the thousands form, and a value too long for
    /// either takes neither. The last is what keeps *"nothing rather than a
    /// truncated number"* (§15 D33) true here as well.
    ///
    /// ⚠️ **`.shapes` is layer-flattened**, so this cannot ask which layer the
    /// text went into — it asks the visible outcome, which is the better
    /// assertion anyway: what text, if any, is on the bar.
    ///
    /// ⚠️ **And it collects *every* `Shape::Text` in the frame, which is only
    /// exhaustive because the closure calls nothing but `draw_extent_number`.**
    /// Add a second painter call to this probe and the assertions become "these
    /// texts, in this order" about a set nobody chose.
    ///
    /// ⚠️ **Flipped** by deleting the thousands arm: fails on the `1080` case
    /// with no galley at all, which is the reported symptom verbatim — the band
    /// reading `0` and then nothing. The `999` case stays green under it, which
    /// is what says this is about the fallback and not about the band drawing.
    #[test]
    fn the_left_bars_band_says_where_the_selection_ends() {
        let ctx = shaped();
        let app_ctx = egui::Context::default();
        crate::theme::install(&app_ctx);
        let _ = app_ctx.run_ui(Default::default(), |_| {});
        let mut app = OndinApp::headless(&app_ctx);
        app.canvas_px = (800, 600);
        let _ = &mut app;

        // The left bar, as `draw_rulers_and_guides` hands it over: `THICKNESS`
        // wide, running the height of the viewport.
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

    /// A chip is sized from its own galley, so the number can never be wider
    /// than the block under it — and a three-digit chip still comes out at the
    /// design's 20×13.
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
        // And the type clears both ways round: 13pt along a horizontal guide's
        // chip, 20pt down a vertical one's.
        let height = measure(&ctx, "-1024.75", CHIP_TEXT).y;
        assert!(
            height <= CHIP_THICKNESS,
            "{height}pt of type in a {CHIP_THICKNESS}pt chip"
        );
    }

    /// The selection band spans its own axis and the bar's full thickness, is
    /// clipped to the bar rather than dropped when the selection runs off
    /// screen, and survives its ends arriving either way round.
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

        // Entirely off screen: nothing at all, rather than a sliver pinned to
        // the edge claiming the selection ends there.
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

    /// A guide is picked by the **segment** it draws, not by the infinite line
    /// through it. That is the whole difference a frame-scoped guide makes to the
    /// hit test: the empty canvas beside a short guide must not be a 6pt band
    /// claiming to be one, or clicking well past the end of a guide inside a small
    /// frame would pick it up.
    #[test]
    fn a_guide_is_picked_along_its_segment_and_not_past_its_ends() {
        let horizontal = line(GuideAxis::Horizontal, (100.0, 200.0), (300.0, 200.0));
        // Straight onto the line, and 4pt off it — inside the 6pt band.
        assert_eq!(horizontal.distance_to(egui::pos2(200.0, 200.0)), 0.0);
        assert_eq!(horizontal.distance_to(egui::pos2(200.0, 204.0)), 4.0);
        // Beyond the right-hand end: the distance grows *along* the axis too, so
        // 60pt past the end is 60pt away however exactly the y lines up.
        assert_eq!(horizontal.distance_to(egui::pos2(360.0, 200.0)), 60.0);
        assert!(
            horizontal.distance_to(egui::pos2(360.0, 200.0)) > GUIDE_PICK_PT,
            "a click 60pt past the end of a scoped guide must miss it"
        );
        // And at the end itself it is still a hit.
        assert!(horizontal.distance_to(egui::pos2(300.0, 200.0)) <= GUIDE_PICK_PT);

        // The same both ways round, and for a leaning line — where the old
        // single-coordinate test had no answer at all.
        let vertical = line(GuideAxis::Vertical, (400.0, 100.0), (400.0, 500.0));
        assert_eq!(vertical.distance_to(egui::pos2(405.0, 300.0)), 5.0);
        assert!(vertical.distance_to(egui::pos2(400.0, 40.0)) > GUIDE_PICK_PT);
        let leaning = line(GuideAxis::Horizontal, (0.0, 0.0), (100.0, 100.0));
        // The midpoint of the diagonal is on it; the corner of its box is not.
        assert!(leaning.distance_to(egui::pos2(50.0, 50.0)) < 1e-4);
        assert!((leaning.distance_to(egui::pos2(100.0, 0.0)) - 70.710_68).abs() < 1e-3);
    }

    /// The chip's along-bar coordinate is the segment's midpoint — which for
    /// everything axis-aligned *is* the guide's own coordinate, so the chip has not
    /// moved for any guide that existed before frames could own one.
    #[test]
    fn the_chip_sits_on_the_guides_coordinate_wherever_that_is_one_number() {
        let horizontal = line(GuideAxis::Horizontal, (100.0, 332.0), (900.0, 332.0));
        assert_eq!(horizontal.chip_along(), 332.0, "exact for an upright guide");
        let vertical = line(GuideAxis::Vertical, (48.5, 20.0), (48.5, 600.0));
        assert_eq!(vertical.chip_along(), 48.5);
        // A guide in a rotated frame has no single coordinate; the midpoint is the
        // honest placement, and the *number* in the chip is still its own exact
        // offset (`GuideLine::label`).
        let leaning = line(GuideAxis::Horizontal, (100.0, 300.0), (300.0, 400.0));
        assert_eq!(leaning.chip_along(), 350.0);
    }

    /// A guide whose segment is off screen is culled, and one that merely *crosses*
    /// the screen with both ends outside it is not. The second half is what a
    /// per-endpoint test gets wrong, and it is exactly the case of zooming into the
    /// middle of a long guide.
    #[test]
    fn a_guide_is_culled_only_when_none_of_it_is_in_view() {
        let inner = egui::Rect::from_min_max(egui::pos2(20.0, 20.0), egui::pos2(800.0, 600.0));
        let visible = line(GuideAxis::Horizontal, (100.0, 300.0), (400.0, 300.0));
        assert!(in_view(inner, visible).is_some());

        // Well above the canvas.
        let above = line(GuideAxis::Horizontal, (100.0, -50.0), (400.0, -50.0));
        assert!(in_view(inner, above).is_none());
        // Off to the right.
        let beside = line(GuideAxis::Vertical, (900.0, 100.0), (900.0, 500.0));
        assert!(in_view(inner, beside).is_none());

        // Both ends outside, crossing the middle: kept.
        let across = line(GuideAxis::Horizontal, (-400.0, 300.0), (1600.0, 300.0));
        assert!(
            in_view(inner, across).is_some(),
            "a guide longer than the viewport must not vanish"
        );
        // And the leaning version of the same thing.
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
        // Left of the frame, then right of it — and both on the guide's own y, so
        // the dashes are the same line carried on.
        assert_eq!(runs[0], (egui::pos2(20.0, 250.0), egui::pos2(300.0, 250.0)));
        assert_eq!(
            runs[1],
            (egui::pos2(500.0, 250.0), egui::pos2(800.0, 250.0))
        );

        // Ends the other way round describe the same two runs: the frame's
        // transform can mirror, which swaps which endpoint is which.
        let mirrored = extension_ends(
            GuideAxis::Horizontal,
            inner,
            egui::pos2(500.0, 250.0),
            egui::pos2(300.0, 250.0),
        );
        assert_eq!(mirrored[0].0.x, 20.0);
        assert_eq!(mirrored[1].1.x, 800.0);

        // A frame spanning the whole canvas has no room for either run, so no
        // dashes are drawn rather than two zero-length ones.
        let full = extension_ends(
            GuideAxis::Vertical,
            inner,
            egui::pos2(400.0, 20.0),
            egui::pos2(400.0, 600.0),
        );
        assert!(full.is_empty());

        // And a gap too small to hold one dash is dropped rather than drawn as a
        // stub, which reads as dirt on the line.
        let tight = extension_ends(
            GuideAxis::Vertical,
            inner,
            egui::pos2(400.0, 21.0),
            egui::pos2(400.0, 599.0),
        );
        assert!(tight.is_empty(), "{tight:?}");
    }

    /// `upright` is what decides both whether a scoped guide attracts and whether
    /// it gets a dotted extension, so what it admits matters twice.
    ///
    /// **A mirror is not a lean.** `Basis::of` peels a reflection out into
    /// `Orientation::mirror` and leaves the angle at zero, and a mirrored frame is
    /// still square with the world — its guide has a perfectly good world
    /// coordinate, reached by the same matrix multiply.
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
        // A quarter turn is *not* treated as upright, deliberately: `GuideLines` is
        // split by axis, so a horizontal guide in a 90°-turned frame pins a world
        // **x** and would have to be filed under the other axis. Declining is the
        // honest degradation, and it keeps one predicate rather than two.
        assert!(!upright(Affine::rotate(std::f64::consts::FRAC_PI_2)));
        // A turn and a turn back is upright again — what the tolerance is for.
        assert!(upright(Affine::rotate(0.4) * Affine::rotate(-0.4)));
    }

    /// A guide's span is the frame's box on the axis it runs along, and its own
    /// position on the axis it pins — **unclamped**, so an offset that has outgrown
    /// a shrunk frame keeps its number and lets the clip hide the line.
    #[test]
    fn a_guides_span_crosses_its_frame_and_is_not_clamped_to_it() {
        let box_ = Rect::new(0.0, 0.0, 400.0, 300.0);
        let (a, b) = ondin_core::guide_span(GuideAxis::Horizontal, 120.0, box_);
        assert_eq!((a.x, a.y), (0.0, 120.0));
        assert_eq!((b.x, b.y), (400.0, 120.0));

        let (a, b) = ondin_core::guide_span(GuideAxis::Vertical, 250.0, box_);
        assert_eq!((a.x, a.y), (250.0, 0.0));
        assert_eq!((b.x, b.y), (250.0, 300.0));

        // Past the bottom of a frame that has been shrunk under it: still 500,
        // still spanning the frame's width. Clamping here is what would lose the
        // number for good.
        let (a, b) = ondin_core::guide_span(GuideAxis::Horizontal, 500.0, box_);
        assert_eq!(a.y, 500.0);
        assert_eq!(b.y, 500.0);
        assert_eq!((a.x, b.x), (0.0, 400.0));
    }

    /// How much of each device pixel a line of `width` points centred at `at`
    /// covers, once `snap_across_axis` has placed it — the coverage the
    /// rasterizer will produce, computed rather than guessed.
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

    /// **Both guide weights keep a fully covered device pixel at every ordinary
    /// display scale**, which is the whole of "a hairline stays a hairline".
    ///
    /// A probe over 100/125/150/175% is what this test is the residue of, and it
    /// overturned two plausible answers on the way. The original parity test read
    /// the **point** width, so at 150% and 175% a 2pt line (3 and 3.5 device
    /// pixels — odd) was edge-centred and spread `[0.5, 1, 1, 0.5]` across four
    /// pixels instead of landing on three. Repairing that with `round` was worse
    /// than either: it calls a 1.5-device-pixel line even, edge-centres it, and
    /// leaves the **resting** guide with no solid pixel at all — `[0.75, 0.75]`,
    /// the exact 2px grey smear the arithmetic exists to prevent. `floor` is what
    /// keeps a core at every scale.
    ///
    /// The assertion is about the core rather than about crispness on purpose: a
    /// fractional device width cannot put both edges on the grid, so demanding that
    /// would be demanding something 125% and 175% cannot give.
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
                // The number of *solid* pixels is what the eye reads as the weight.
                cores.push(cov.iter().filter(|c| **c > 0.999).count());
            }
            // And the selected weight reads as heavier than the resting one at every
            // scale. The failure this guards is a "fix" that rounds back to a no-op
            // at 150% `pixels_per_point` — i.e. on the user's actual machine.
            assert!(
                cores[1] > cores[0],
                "ppp {ppp}: selected covers {} solid pixel(s) against the resting \
                 guide's {} — the two weights are indistinguishable here",
                cores[1],
                cores[0]
            );
        }
    }

    /// The three states' ink: the guide's own colour at rest and on hover, the
    /// selection blue while it is picked.
    ///
    /// The blue **overrides** a recoloured guide, deliberately — a per-guide colour
    /// is the one thing about a guide the user authors, and it still goes blue for
    /// as long as it is held. Recorded so a recoloured guide turning blue is not
    /// later filed as a bug.
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

    /// **A guide about to be discarded is faded, even though it is drawn as
    /// selected** — and that is only true because the fade is applied *after* the
    /// state has chosen the ink.
    ///
    /// This is the regression the selection blue introduced and this test exists
    /// for: the fade used to be applied to the guide's own colour at the call site,
    /// which the blue then replaced wholesale. Both branches returned the same
    /// opaque blue, so dragging a guide onto the ruler looked identical to dragging
    /// it anywhere else and nothing on screen said the release would delete it.
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

        // The pre-blue arrangement, for contrast: fading the *colour* and then
        // letting the state pick the ink loses the fade entirely.
        let faded_first = guide_ink(red.multiply_alpha(DISCARD_ALPHA), true, false);
        assert_eq!(
            faded_first, held,
            "if this ever differs, fading before the state choice has started \
             working and the ordering comment on `guide_ink` is wrong"
        );
    }

    /// The three parity rules, side by side at the two scales that separate them —
    /// so the reason `floor` is in the code is visible rather than asserted.
    ///
    /// Kept because it is the cheapest possible regression test for a change
    /// somebody would make for tidiness: `round` reads more naturally than `floor`
    /// beside a `.round()` on the line above it, and swapping them back breaks only
    /// the appearance of a hairline at 150%.
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

        // 150%: the point-width test edge-centres the *selected* guide's 3 device
        // pixels, fringing them over 4.
        let ppp = 1.5;
        assert_eq!(coverage(by_point_width(100.3, ppp, 2.0), ppp, 2.0).len(), 4);
        assert_eq!(
            coverage(snap_across_axis(100.3, ppp, 2.0), ppp, 2.0).len(),
            3
        );

        // ...and the rounded-device test takes the core off the *resting* one.
        assert!(
            core(by_rounded_device(100.3, ppp, 1.0), ppp, 1.0) < 0.8,
            "rounding used to leave the 1pt guide with no solid pixel; if this \
             passes, the two rules now agree and the comment on \
             `snap_across_axis` is wrong"
        );
        assert!(core(snap_across_axis(100.3, ppp, 1.0), ppp, 1.0) > 0.999);

        // 175% is the same story for the resting guide.
        let ppp = 1.75;
        assert!(core(by_rounded_device(100.3, ppp, 1.0), ppp, 1.0) < 0.9);
        assert!(core(snap_across_axis(100.3, ppp, 1.0), ppp, 1.0) > 0.999);
    }
}

#[cfg(test)]
mod guide_grab_tests {
    //! **The test §15 D35 asked for and could not write.** That entry ends
    //! *"anyone who finds a way to construct an `OndinApp` headlessly should pin
    //! this first"*; `OndinApp::headless` arrived with §15 D303, and this is it.
    //!
    //! What it pins is the **teleport** half of D35, which is the half that was
    //! reported from use: clicking a guide to select it moved the line by a few
    //! points and committed that as an edit, because the press put the line at the
    //! *pointer's* coordinate and the pointer is within `GUIDE_PICK_PT` of the line
    //! rather than on it. `GuideDrag::grab` is the fix and its own doc comment has
    //! been the only thing holding it up since.
    use super::*;
    use crate::app::OndinApp;

    /// A headless app with an 800 × 600 canvas and one guide, plus the `rect`/`ppp`
    /// the input functions take.
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

    /// **Pressing a guide anywhere in its pick band leaves it exactly where it
    /// was.** The whole of the reported bug: select a guide, and it shifts.
    ///
    /// **Swept across the band rather than sampled once**, and in both directions,
    /// because the failure is proportional to the offset — a single press *on* the
    /// line passes against the broken version too, and that is the press nobody
    /// makes. Every offset up to `GUIDE_PICK_PT` is a press that used to move the
    /// line by exactly that much.
    ///
    /// The pointer is placed through `to_screen`, so the assertion says nothing
    /// about the camera: whatever zoom or pan is in force, the round trip has to
    /// come back to the number the guide already had.
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

    /// **A guide pulled out of a ruler has no grab and appears under the pointer**,
    /// which is the other half of the rule and the one that stops the fix above
    /// becoming "guides never follow the mouse".
    ///
    /// `id: None` is what "pulled from the ruler" means here — there is no guide in
    /// the document yet, so there is no line to have been grabbed at an offset from.
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

    /// **The six-arm commit table, which had no test caller at all** (§15 D474).
    ///
    /// `[A5-L6-04]`: `grep -rn "finish_guide_drag\|update_guide_drag" crates/`
    /// returned **one** hit outside the functions themselves, a comment in
    /// `canvas.rs`. The two tests above stop one call earlier — both drive
    /// `begin_guide_drag` and assert on the drag's *opening* state — so **nothing
    /// in the workspace released a guide drag**, and the whole of the decision
    /// that writes to the document was held up by its own comments.
    ///
    /// Three one-line mutations were all green before this, and the first is data
    /// the user did not ask to lose:
    ///
    /// 1. Collapsing `(Some(_), true) if d.copying => {}` into
    ///    `(Some(id), true) => self.remove_guide(id)` — one character — makes
    ///    **Alt-dragging a copy and dropping it back on the ruler delete the
    ///    original**. The arm's own comment is the specification: *"not
    ///    `remove_guide` — an Alt-drag was never asking to delete anything, and
    ///    the original has not moved."*
    /// 2. `let moved = true;` — claimed to make every click that merely *selects*
    ///    a guide push an undo step.
    /// 3. Dropping `|| g.owner != d.owner` — dragging a guide into a frame
    ///    without shifting its line commits nothing and the scope change is
    ///    silently lost.
    ///
    /// ⚠️ **Mutation 2 does not bite, and that is a finding rather than a failed
    /// experiment.** Run, `let moved = true;` leaves every assertion here green:
    /// §15 **D428**'s guard in `EditorSession::commit_inner` already drops a
    /// transaction whose every operation writes back the value that is already
    /// there, and a `SetGuideScope` to the same owner at the same position is
    /// exactly that. **The finding was written against a tree where D428 had
    /// landed**, and its own claim about the consequence is refuted. What `moved`
    /// still buys is the op not being built and the rule stated at the site that
    /// has it; the assertion below is kept, because it is the property, and the
    /// note above `moved` now says which guard is holding it.
    ///
    /// ⚠️ **Mutations 1 and 3 both bite, at the sites named.** They are the two
    /// that matter: one loses the user's guide, and the other loses a scope
    /// change silently.
    ///
    /// The harness was three calls short — `update_guide_drag` and
    /// `finish_guide_drag` are `&mut self` and take no egui context.
    #[test]
    fn the_guide_commit_table_answers_each_of_its_arms() {
        let ctx = egui::Context::default();
        let bar = |axis: GuideAxis, rect: egui::Rect| match axis {
            // Inside the guide's *own* bar, which is what `discarding` reads.
            GuideAxis::Vertical => egui::pos2(rect.left() + THICKNESS / 2.0, 300.0),
            GuideAxis::Horizontal => egui::pos2(400.0, rect.top() + THICKNESS / 2.0),
        };

        // **1. An Alt-dragged copy dropped on the ruler leaves the original.**
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

        // **The control for it**: the *same* gesture without Alt is a delete, so
        // this pair is about `copying` rather than about the ruler.
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

        // **2 and 3 are one sequence, and they have to be.** ⚠️ The obvious
        // spelling of 2 — press a fresh guide on its own line and assert no step
        // — **does not test what it says**: `OndinApp::headless` starts with an
        // artboard, and `app_with_guide` makes a guide with `owner: None`, so the
        // very first press over that frame is a legitimate **scope** change. It
        // committed, and the assertion failed for the right reason. Measured
        // while writing this: `position` drift exactly `0`, `d.owner` the default
        // frame, `was.owner` `None`. So the no-op press has to come *second*, on
        // a guide that has already taken its scope — which is also the sequence a
        // user performs.
        let (mut app, id, rect, ppp) = app_with_guide(&ctx, GuideAxis::Vertical, 200.0);
        let depth = app.session.history.undo_depth();
        let on_line = app.to_screen(Point::new(200.0, 100.0), rect, ppp);

        // **3 first: the scope counts as having moved too.** The line does not
        // shift by so much as a float's width — the drift is 0 — and this is
        // still an edit, which is the boundary a test of 2 alone cannot see.
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

        // **2: now press it again, with nothing left to change.**
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

        // **4. Pulled out of the ruler and put straight back: nothing happened.**
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

        // **Its control**: dropped on the canvas instead, it exists.
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
    }

    /// **A guide the app has decided not to show is not moved by the arrow keys
    /// and not destroyed by Delete** — `[S19.1-L2-01]`, §15 D541.
    ///
    /// The guide *selection* survives *View ▸ Hide guides*. Only
    /// `set_guides_locked` clears it, and nothing else may:
    /// `measurable_guide`'s doc names the asymmetry and **relies** on it —
    /// *"a selected guide can be invisible, and without this the overlay would
    /// measure from a line nobody can see."* So the fix is a gate on the two
    /// verbs, not a clear on the switch.
    ///
    /// 🚨 **This test drove *two* switches until §15 D750 and now drives one.**
    /// The second row was present mode, on the argument that *"the finding's tell
    /// is that they give identical numbers, which is what says the root is
    /// `guides_on()` rather than present mode specifically"* — true when it was
    /// written, and the maintainer has since ruled that **present mode does not
    /// hide guides at all**: it is a chrome switch, and a guide is on the canvas.
    /// So present mode no longer reaches `guides_on`, the row's fixture assertion
    /// (*"must actually take the guides off screen"*) can no longer hold, and the
    /// case moved to `present_mode_leaves_the_guides_on_screen_and_live` below.
    ///
    /// ⚠️ **The reason for driving two switches survives the loss of one of
    /// them.** It was that a fix gating on `self.present` alone would leave *Hide
    /// guides* broken — and with present mode out of `guides_on` entirely, gating
    /// on `self.present` is now a mutation that fails **both** tests rather than
    /// neither. The single row that remains is the one that was always
    /// load-bearing.
    ///
    /// ⚠️ **The visible control is not optional and the finding says so in as
    /// many words**: *"a version that simply never nudges passes the first
    /// assertion alone."* Deleting `nudge_guides`' body entirely satisfies every
    /// hidden-case assertion here.
    ///
    /// ⚠️ **Flipped once per gate, separately, which is what two gates deserve.**
    /// `nudge_guides`' gate removed: fails on the `View ▸ Hide guides` position
    /// at `104.0` against `100.0`. `remove_selected_guides`' gate removed (the
    /// nudge one back in place): fails on the same arm's *Delete* assertion. So
    /// each gate has its own teeth and neither is carrying the other — which a
    /// single both-gates-off flip could not have told me, since it stops at the
    /// first assertion.
    ///
    /// (Plain backticks rather than `[links]`, §15 D319's convention inside a
    /// `cfg(test)` module.)
    #[test]
    fn a_hidden_guide_is_neither_nudged_nor_deleted() {
        let ctx = egui::Context::default();
        let down = ondin_core::kurbo::Vec2::new(0.0, 1.0);

        // The way a guide goes off screen without the selection being cleared.
        // **A loop over one row, kept as a loop**: present mode was the second and
        // left in §15 D750, and a `View ▸ Hide guides` that is a bare block would
        // have to be rewritten back into a table by whoever adds the third.
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

        // **The control**: with the guides on, both verbs are the feature.
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

    /// **Present mode hides the app's chrome and leaves the canvas working** —
    /// §15 D750, `[S12.5-L2-01]`, the maintainer's ruling.
    ///
    /// 🚨 **This is the inverse of a case that used to live in
    /// `a_hidden_guide_is_neither_nudged_nor_deleted`**, and it is here rather than
    /// deleted because the mutation it gates is a *reinstatement*: `guides_on`
    /// read `show_guides && !self.present` for months, its own doc argued for the
    /// `!self.present` at length, and §15 D385 still cites that argument. Somebody
    /// putting it back on the strength of the old prose has to fail something.
    ///
    /// **Three assertions because present mode makes three separate claims**: the
    /// line is drawn, the guide can still be picked up, and the two verbs that
    /// `[S19.1-L2-01]` gated still work. A test asserting only `guides_on()` would
    /// be green for a version that drew the line and refused the drag, which is
    /// the *"claims a press for a bar nobody can see"* mismatch from the other
    /// side — chrome that answers and does not draw, drawn chrome that does not
    /// answer.
    ///
    /// ⚠️ **The rulers are the control, and they are the half that still hides.**
    /// Without that assertion this test passes against a present mode that has
    /// stopped doing anything at all.
    ///
    /// ⚠️ **Flip, run — the site was right and the blast radius was wrong.**
    /// Restoring `!self.present` to `guides_on` fails at the first assertion
    /// (`rulers.rs:3282`) and takes **nothing else** with it. The prediction here
    /// said it would also fail `canvas::layout_grid_tests`; it does not, because
    /// **the grids have their own guard in `draw_layout_grids` and never read
    /// `guides_on`.** One ruling, two independent implementations — so a
    /// reinstatement at either site leaves a full green board at the other, and
    /// that is exactly why both sites are gated rather than one.
    ///
    /// (Plain backticks rather than `[links]`, §15 D319's convention inside a
    /// `cfg(test)` module.)
    #[test]
    fn present_mode_leaves_the_guides_on_screen_and_live() {
        let ctx = egui::Context::default();
        let down = ondin_core::kurbo::Vec2::new(0.0, 1.0);
        let (mut app, id, _, _) = app_with_guide(&ctx, GuideAxis::Horizontal, 100.0);
        app.session.selection.set_guide(id);
        app.show_rulers = true;
        app.present = true;

        assert!(
            app.guides_on(),
            "present mode is a chrome switch: the guide lines stay on the canvas"
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

        // **The control**: present mode still does its job, which is the bars.
        assert!(
            !app.rulers_on(),
            "control: the ruler bars are window furniture and present mode still \
             takes them away — without this the test is green for a present mode \
             that does nothing"
        );
    }

    /// **Turning the rulers off mid-drag does not turn a cancel into a create** —
    /// §15 D555, `[S19.1-L1-02]`.
    ///
    /// `over_own_ruler` was `self.rulers_on() && ruler_at(…) == Some(axis)`, and
    /// `rulers_on` is `show_rulers && !present` — two live chords (`Shift+R`,
    /// `Ctrl+\`) that `OndinApp` dispatches with no drag guard. It is the **only**
    /// term feeding `discarding`, so a toggle with the button still down made the
    /// gesture the module documents as *"the gesture that made it, run
    /// backwards"* commit a guide instead, at whatever coordinate the vanished bar
    /// sat over, selected and dirty — while the rest of the drag drew nothing and
    /// the cursor still promised a live guide drag.
    ///
    /// ⚠️ **The control has to assert the *bin cursor*, not just an empty guide
    /// list**, and the finding says so: a version that never created guides at all
    /// would satisfy "no guide, no undo step" in both rows. `announces_discard()`
    /// being true on the control is what says the un-flipped gesture really did
    /// mean to discard.
    ///
    /// ⚠️ **Both directions, because the term is a conjunction and the bug is
    /// symmetric.** Turning the rulers *on* mid-drag made `over_own_ruler` true,
    /// so a guide dragged to the top of the canvas became discarding without
    /// moving — the third row here.
    ///
    /// ⚠️ **Flip-check, run: `d.rulers_shown` put back to `self.rulers_on()`.**
    /// Fails at the present-mode row with *"turned the cancel gesture into a
    /// create gesture"*, and — measured by skipping that row and re-running —
    /// **also at row 3**, where a bar appearing mid-drag deletes a guide the
    /// gesture never dropped on one. Both directions of the conjunction bite. The
    /// leading control and the pull-with-nothing-toggled control stay green, which
    /// is what says the flip is about the latch and not about the commit table.
    ///
    /// 🚨 **This said the flip raises `dead_code` on `GuideDrag::rulers_shown`,
    /// and called that "a free confirmation … the field is read at exactly one
    /// site, so there is no second copy of this rule to fall out of step". Both
    /// halves were wrong, and the sentence is why the fix looked finished.** The
    /// field now has **two** readers — `over_own_ruler` and `off_rulers` — so
    /// reverting either one leaves the other keeping the lint quiet, and the
    /// confirmation is gone. It was *never* a fact about the rule: the lint counts
    /// **readers of a field**, and this rule's population is the *terms that fold
    /// `rulers_on()`*, which is not the same set. When it fired, there was one
    /// reader because the fix had only latched one of the two terms — the lint was
    /// reporting the bug and I read it as proof there was nothing left to do.
    /// ⚠️ **A lint going quiet is evidence about a name, not about a rule.**
    /// (It fired at all only because `GuideDrag` does not derive `PartialEq` — see
    /// CLAUDE.md on the lint going silent when it does.)
    #[test]
    fn toggling_the_rulers_mid_drag_does_not_change_what_the_release_means() {
        let ctx = egui::Context::default();
        let bar = |rect: egui::Rect| egui::pos2(rect.left() + THICKNESS / 2.0, 300.0);

        // **The control**: rulers on for the whole gesture. A drag out and back
        // onto the bar is a discard, and it says so.
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

        // **1 and 2 are the finding's own gesture: a *new* guide pulled out of the
        // bar, which is where cancelling has to create nothing at all.** An
        // existing guide dropped on the bar is a legitimate edit (the control
        // above spends a step on it), so the undo assertion only means something
        // here.
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
                "`Ctrl+\\` (present mode)",
                (|a: &mut OndinApp| a.present = true) as fn(&mut OndinApp),
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

        // **The control for those two**: the identical pull with nothing toggled
        // creates nothing either — which is what says the two rows above are about
        // the toggle and not about a gesture that never works.
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

        // **3. The other direction**: rulers *off* at the press, on mid-drag. A
        // guide that never went near a bar must not be deleted by one appearing.
        let (mut app, id, rect, ppp) = app_with_guide(&ctx, GuideAxis::Vertical, 200.0);
        app.show_rulers = false;
        let on_line = app.to_screen(Point::new(200.0, 100.0), rect, ppp);
        app.begin_guide_drag(Some(id), GuideAxis::Vertical, on_line, rect, ppp);
        app.update_guide_drag(egui::pos2(400.0, 300.0), rect, ppp, false);
        app.show_rulers = true;
        // Where the bar *now* is — which the press knew nothing about.
        app.update_guide_drag(bar(rect), rect, ppp, false);
        app.finish_guide_drag();
        assert!(
            app.session.doc.guide(id).is_some(),
            "a bar that appeared mid-drag cannot delete a guide the gesture never \
             dropped on one"
        );

        // **4. The announcement, which the first version of this fix missed.**
        // `off_rulers` fed `left_ruler` through `self.ruler_at`, which folds the
        // same live chords — so taking the bars away latched *"this guide has been
        // out there"* without the pointer moving, and the bin cursor appeared over
        // a new guide that does not exist yet.
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
        app.present = true;
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
