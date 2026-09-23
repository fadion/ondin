//! Snapping, and the lines and numbers that explain it.
//!
//! While a move or draw gesture is live, the dragged box's edges and centres
//! are compared against the same features of nearby nodes, and against the
//! ruler guides on the page; if one lands within a few screen pixels, the
//! gesture is nudged onto it and a line is drawn.
//!
//! **Two kinds of snap, and the difference is what they draw.** An *alignment*
//! puts a feature on a coordinate — an edge on an edge, a centre on a guide — and
//! explains itself with a bare [`Guide`], a line with no number because the number
//! would be zero. A *spacing* snap ([`spacing_candidates`]) equalizes the **gaps**
//! either side of the box instead, and a gap is only worth drawing with its number,
//! so it explains itself with [`Measure`]s — the same type the Alt-hover measure
//! produces, drawn by the same function. Both arrive together in [`Marks`].
//!
//! They compete on one scale: whichever is nearer claims the axis, with alignment
//! taking ties. See the comment at the spacing loop in [`snap_rect`] for why that
//! way round.
//!
//! **Two things here are called a guide and they are not the same.** [`Guide`]
//! in this module is a *transient* line drawn for one frame to explain a snap
//! that is happening. A `ondin_core::Guide` is a **ruler guide** — a persistent
//! line the designer placed, saved with the document (`rulers.rs`). This module
//! snaps *to* the latter and draws the former.
//!
//! Everything is computed in world space but the tolerance is in *screen*
//! pixels, so snapping feels equally sticky at every zoom level — at 10% zoom a
//! fixed world tolerance would snap across half the canvas.
//!
//! This is pure geometry over `Resolved` bounds: no document mutation, no egui.
//! The gesture applies the returned adjustment and draws the returned guides.

use crate::measure::{self, Axis, Measure};
use ondin_core::kurbo::{Point, Rect, Vec2};
use ondin_core::{Document, NodeId, Resolved};

/// How close (in screen pixels) an edge must come before it snaps.
const TOLERANCE_PX: f64 = 6.0;

/// Round to the pixel grid, whose pitch is **one world unit**.
///
/// That equivalence — a world unit is a pixel at 100% zoom — is how the whole
/// app has always behaved (`Viewport` derives zoom from the ratio of a world
/// rect to a pixel size, and the design's 51.94% shows 100 units every 51.94px)
/// but it was never written down as a rule. This function and the grid in
/// `grid.rs` are the first two things to *depend* on it, so it is stated here
/// rather than left implicit: they must round and draw the same lattice, or the
/// app shows one grid and honours another.
fn to_pixel(value: f64) -> f64 {
    value.round()
}

/// The delta that puts `value` on the pixel grid.
///
/// **The fallback under every snap, never a competitor to one.** An alignment
/// or guide match on an axis wins outright and this is not consulted: a shape
/// lined up with another shape's edge at 100.5 belongs at 100.5, and rounding
/// it to 100 afterwards would break the very alignment that was just found. So
/// pixel rounding only fills the axes nothing else claimed — which is most of
/// them, most of the time, and is what keeps geometry whole without the
/// designer thinking about it.
///
/// Skipped on an axis a Shift-constrained gesture has locked, along with the
/// rest of snapping: the layer must not creep onto the grid perpendicular to the
/// line the user is holding it to, however fractional it was to begin with.
fn pixel_delta(value: f64) -> f64 {
    to_pixel(value) - value
}

/// What a gesture is allowed to snap to — the toggles in the top bar's **Snap**
/// menu (`design/Editor.dc.html`), resolved into the sources they admit.
///
/// A struct rather than one parameter each, because they are one decision —
/// "what may snapping consult" — and threading them separately is how a call
/// site ends up honouring some and quietly forgetting one. That is not
/// hypothetical arithmetic: this said "the three" until baselines made it four
/// (§15 D355), and a **count** in a doc comment is a fact that goes stale on the
/// next row without anything failing. Owned
/// rather than borrowed: `targets` is a fresh `Vec` every gesture frame anyway
/// (it is culled to the viewport), so there is nothing to save by lending it.
///
/// A source that is switched off arrives **empty**, not flagged. The functions
/// below then need no `if` for it: an empty candidate list cannot win, so the
/// toggle is enforced by construction at one site instead of being re-checked at
/// every comparison.
pub struct Snapping {
    /// Node bounds. Empty when *Snap to shapes* is off.
    pub targets: Vec<Rect>,
    /// Ruler guides. Empty when *Snap to guides* is off.
    pub guides: GuideLines,
    /// Text baselines in world space, as bare ys (§15 D355). Empty when *Snap to
    /// baselines* is off, or when nothing in view is a text node.
    ///
    /// **Its own source rather than more `guides.y`**, though the shape is
    /// identical, because the two differ in every way that matters downstream: a
    /// ruler guide is *already a drawn line* and a baseline is not, so a match on
    /// one wants a mark and a match on the other does not (see [`Matched`]); and a
    /// guide is something the user placed where a baseline is inherent, which is
    /// why they answer to different switches.
    pub baselines: Vec<f64>,
    /// Whether an axis nothing else claimed falls to the whole-pixel lattice —
    /// *Snap to grid*. The one source that has to be a flag rather than an empty
    /// list, because the lattice is everywhere.
    pub pixel: bool,
    /// Logical points per world unit, which turns the screen-space tolerance into
    /// world units. Deliberately not `Default`-able: a zero zoom would make the
    /// tolerance effectively infinite and snap everything to everything.
    ///
    /// ⚠️ **This said device pixels, and was built from `camera.zoom`, until §15
    /// D853** — so the snap tolerance shrank with the display scale while the
    /// pointer it is aimed by is measured in points. `OndinApp::snapping` passes
    /// `points_per_world` now; at 100% scaling the two are the same number.
    pub zoom: f64,
}

impl Snapping {
    /// Snapping that consults nothing — the base a test adds one source to.
    ///
    /// `#[cfg(test)]` because that is now the whole truth about it. This used to
    /// be the Shift case, built by `OndinApp::snapping` whenever the key was
    /// down; Shift constrains a gesture instead of silencing snapping now (§15),
    /// and the menu's own everything-off state goes through the ordinary path with
    /// empty sources rather than through here. Left `pub` without the gate
    /// it would be a constructor that looks like production API and is reachable
    /// only from tests, which is the kind of decoy §15 D2 exists to warn about.
    #[cfg(test)]
    pub fn none(zoom: f64) -> Self {
        Self {
            targets: Vec::new(),
            guides: GuideLines::default(),
            baselines: Vec::new(),
            pixel: false,
            zoom,
        }
    }
}

/// The ruler guides worth snapping to, split by the axis each one pins.
///
/// Coordinates rather than `ondin_core::Guide`s, so this module keeps knowing
/// nothing about the document: a guide is a number on an axis as far as
/// snapping is concerned, and `x`/`y` here mean the same thing they do on a
/// `Rect`'s features, which is what makes the comparison one loop.
#[derive(Clone, Debug, Default)]
pub struct GuideLines {
    /// Xs — from *vertical* guides, which pin an x.
    pub x: Vec<f64>,
    /// Ys — from *horizontal* guides, which pin a y.
    pub y: Vec<f64>,
}

/// A transient line drawn while a snap is active, in world space. Not a ruler
/// guide — see the module docs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guide {
    pub axis: Axis,
    /// The world coordinate of the line (x for vertical, y for horizontal).
    pub at: f64,
    /// The extent to draw along the other axis — the union of the moving box
    /// and the thing it snapped to, so the guide visibly connects them.
    pub from: f64,
    pub to: f64,
}

/// What a snap drew, so the gesture can hand the whole explanation to the overlay
/// as one thing.
///
/// Two lists rather than one, because the two say different kinds of thing and are
/// drawn differently. A [`Guide`] is an *alignment*: a bare line saying "these
/// coincide", with no number because the number would be zero. A [`Measure`] is a
/// **gap**, which is only worth drawing *with* its number — and it is the same type
/// the Alt-hover measure produces, so both go through `canvas::draw_measure` and a
/// red number on the canvas means one thing everywhere.
#[derive(Default, Debug)]
pub struct Marks {
    pub guides: Vec<Guide>,
    /// The gaps a spacing snap equalized, labelled. Empty for every snap that is
    /// not a spacing one, which today is every snap but a move's.
    pub gaps: Vec<Measure>,
}

impl From<Vec<Guide>> for Marks {
    fn from(guides: Vec<Guide>) -> Self {
        Marks {
            guides,
            gaps: Vec::new(),
        }
    }
}

/// The result of a snap: how much further to move, and what to draw.
#[derive(Default, Debug)]
pub struct Snap {
    pub adjust: Vec2,
    pub marks: Marks,
}

/// What the best candidate on an axis turned out to be.
///
/// The distinction is only about what to *draw*. A node match needs a line
/// spanning both boxes to show the relationship; a ruler guide **is already a
/// drawn line**, so a second one laid exactly along it would say nothing and
/// muddy the one that is already there.
///
/// It used to say "a line across the whole canvas in its own colour", and both
/// halves have since stopped being true — a frame-scoped guide spans only its
/// owner's box, and a selected one is drawn in the selection blue (§15 D139). The
/// rule survives all of that because what it rests on is only that the line is
/// there at all.
#[derive(Clone, Copy)]
enum Matched {
    Node(Rect),
    RulerGuide,
    /// A text baseline (§15 D355). **Draws a mark, unlike `RulerGuide`**, and for
    /// that variant's own stated reason read the other way: the rule there is
    /// "the line is already there", and a baseline's is not — the overlay draws
    /// baselines only for a *selected* text node, and the node being snapped to is
    /// by definition not the one being dragged. Without a mark, a baseline snap is
    /// the box jumping to a coordinate with nothing on screen explaining it.
    ///
    /// **Carries the y it matched**, where `Node` carries a rect and re-derives
    /// the feature with `nearest_of`. A rect has three features per axis and the
    /// winner has to be recovered; a baseline *is* the coordinate, so passing it
    /// through is both simpler and exact — there is nothing to re-derive and so
    /// nothing to re-derive wrongly.
    Baseline(f64),
    Spacing(Spacing),
}

/// What a **spacing** match was, kept so the gaps can be drawn once the delta it
/// asked for has actually been applied.
///
/// The neighbours rather than the finished measures, deliberately: a candidate is
/// judged before it wins, and the gaps it produces are only true at the position it
/// would move the box to. Building the measures at candidate time would have them
/// quoting the *unsnapped* gaps — off by the very delta the snap is about, which is
/// a label that is wrong by exactly the amount that makes it interesting.
#[derive(Clone, Copy)]
struct Spacing {
    /// The neighbour below the moving box on this axis, when the gap to it is part
    /// of the match.
    low: Option<Rect>,
    /// The neighbour above it.
    high: Option<Rect>,
    /// The gap that already existed and that this match *copied*, drawn beside the
    /// new one so "the same as that" is a thing you can see rather than infer.
    ///
    /// `None` for a centring, where both drawn gaps are new and equal to each other
    /// — there the equality *is* the two labels agreeing.
    rhythm: Option<Measure>,
}

/// How near two boxes have to come, in world units, before the gap between them
/// counts as a gap at all.
///
/// A spacing snap over a zero gap is not spacing, it is two boxes touching — which
/// alignment already handles and labels with nothing. Without this floor, a row of
/// flush boxes offers "equalize these two zero gaps" candidates at every position,
/// all of them a no-op with two `0` labels on them.
const MIN_GAP: f64 = 0.5;

/// The **spacing** candidates on one axis: put the moving box halfway between its
/// two neighbours, or a neighbour's own gap away from it.
///
/// This is the other half of "figure out distances", and the half that acts rather
/// than reports. Two candidates, and between them they cover what a designer is
/// actually doing when they nudge a box in a row:
///
/// - **Centre it between the two neighbours it sits between.** The gap on each side
///   ends up the average of the two, which is the only position that equalizes them
///   without moving anything else.
/// - **Match the rhythm already there.** Past the end of a run, the gap between the
///   last two members is what the next gap wants to be — so a third box lands the
///   same distance from the second as the second is from the first, and a row spaces
///   itself as it is built.
///
/// **Only boxes that share a band with the moving one.** Two shapes whose vertical
/// extents miss each other entirely are not in a row, and the horizontal "gap"
/// between them is a number about nothing. Requiring a strict overlap is also what
/// keeps the candidate set small on a busy canvas: it is a row, not the viewport.
///
/// **Nothing here is offered for a box that overlaps a neighbour on this axis.**
/// There is no gap to equalize when the two are on top of each other, and the sign
/// of the "gap" would flip under the pointer.
fn spacing_candidates(moving: Rect, targets: &[Rect], axis: Axis) -> Vec<(f64, Spacing)> {
    let along = |r: &Rect| match axis {
        Axis::Horizontal => (r.min_x(), r.max_x()),
        Axis::Vertical => (r.min_y(), r.max_y()),
    };
    let band = |r: &Rect| match axis {
        Axis::Horizontal => (r.min_y(), r.max_y()),
        Axis::Vertical => (r.min_x(), r.max_x()),
    };
    let (m_lo, m_hi) = along(&moving);
    let m_band = band(&moving);

    // The row the moving box is in, split by which side of it each member is on.
    // A target overlapping it along the axis is in neither list, which is the "no
    // gap to equalize" rule.
    let mut before: Vec<Rect> = Vec::new();
    let mut after: Vec<Rect> = Vec::new();
    for t in targets {
        let t_band = band(t);
        if t_band.0.max(m_band.0) >= t_band.1.min(m_band.1) {
            continue; // not in the same row
        }
        let (t_lo, t_hi) = along(t);
        if t_hi <= m_lo {
            before.push(*t);
        } else if t_lo >= m_hi {
            after.push(*t);
        }
    }
    // Nearest first on each side.
    before.sort_by(|a, b| along(b).1.total_cmp(&along(a).1));
    after.sort_by(|a, b| along(a).0.total_cmp(&along(b).0));

    let mut out = Vec::new();

    // Centre between the two nearest neighbours. The delta that equalizes the two
    // gaps is half their difference, and the gap it lands on is their average — so
    // it is offered only when that average is a real gap.
    if let (Some(low), Some(high)) = (before.first(), after.first()) {
        let gap_low = m_lo - along(low).1;
        let gap_high = along(high).0 - m_hi;
        if (gap_low + gap_high) / 2.0 > MIN_GAP {
            out.push((
                (gap_high - gap_low) / 2.0,
                Spacing {
                    low: Some(*low),
                    high: Some(*high),
                    rhythm: None,
                },
            ));
        }
    }

    // Match the rhythm on either side: the gap between the two nearest neighbours
    // on that side is what the gap to the nearer of them wants to become.
    for (side, sign) in [(&before, 1.0), (&after, -1.0)] {
        let (Some(near), Some(next)) = (side.first(), side.get(1)) else {
            continue;
        };
        // The existing gap has to be a real one, and the pair has to be separated
        // along the axis for it to exist at all.
        let Some(rhythm) = measure::gap(*near, *next, axis) else {
            continue;
        };
        if rhythm.distance() <= MIN_GAP {
            continue;
        }
        let mine = match sign > 0.0 {
            true => m_lo - along(near).1,
            false => along(near).0 - m_hi,
        };
        out.push((
            sign * (rhythm.distance() - mine),
            Spacing {
                low: (sign > 0.0).then_some(*near),
                high: (sign < 0.0).then_some(*near),
                rhythm: Some(rhythm),
            },
        ));
    }
    out
}

/// The gaps to draw for a spacing match, measured at the position the snap moved
/// the box to.
fn spacing_marks(moved: Rect, sp: Spacing, axis: Axis) -> Vec<Measure> {
    let mut out = Vec::new();
    // The copied gap first, so it reads as the thing being matched rather than as a
    // third measurement of the moving box.
    out.extend(sp.rhythm);
    for neighbour in [sp.low, sp.high].into_iter().flatten() {
        out.extend(measure::gap(moved, neighbour, axis));
    }
    out
}

/// Snap `moving` (the dragged box, already at its un-snapped position) against
/// the world bounds of everything in `targets` and the ruler guides in
/// `guides`.
///
/// `zoom` converts the screen-pixel tolerance to world units.
///
/// `axes` says which world axes the gesture is free to move along, as it does
/// for [`snap_point`]. Both are true for an ordinary move; a Shift-constrained
/// one is locked to a single axis, and the locked axis must come back with an
/// adjustment of exactly zero — otherwise a snap would slide the box off the
/// line the user is holding it to. Gating it here rather than zeroing the result
/// afterwards is what also suppresses the *guide* for that axis: a line drawn to
/// explain a snap that was then discarded is worse than no line.
///
/// **A ruler guide wins a tie with a node.** Someone placed the guide on
/// purpose, and to line work up against; a node's edge that happens to sit at
/// the same coordinate is a coincidence. The two would land the box in the same
/// place anyway, so the tie only decides which line is drawn — but preferring
/// the guide is also what stops a snap flickering between the two as a node
/// moves under it.
pub fn snap_rect(moving: Rect, axes: (bool, bool), s: &Snapping) -> Snap {
    let tolerance = TOLERANCE_PX / s.zoom.max(f64::EPSILON);
    let mut snap = Snap::default();

    // Candidate features on each axis: near edge, centre, far edge.
    let moving_x = [moving.min_x(), moving.center().x, moving.max_x()];
    let moving_y = [moving.min_y(), moving.center().y, moving.max_y()];

    let mut best_x: Option<(f64, f64, Matched)> = None; // (distance, delta, what)
    let mut best_y: Option<(f64, f64, Matched)> = None;

    for target in &s.targets {
        if axes.0 {
            for m in moving_x {
                for t in [target.min_x(), target.center().x, target.max_x()] {
                    consider(&mut best_x, m, t, tolerance, Matched::Node(*target), false);
                }
            }
        }
        if axes.1 {
            for m in moving_y {
                for t in [target.min_y(), target.center().y, target.max_y()] {
                    consider(&mut best_y, m, t, tolerance, Matched::Node(*target), false);
                }
            }
        }
    }
    // **Baselines on the nodes' terms** (§15 D355): they must *beat* the best so
    // far rather than tie it, so a ruler guide the user placed still wins a dead
    // heat against a baseline they did not. Only `moving_y` — a baseline is a y
    // and has no x to offer.
    //
    // ⚠️ **The position of this block is not what enforces that, though it reads
    // as if it were.** Guides tie (`true`) and baselines do not (`false`), and
    // that pair settles the dead heat in *either* order: baselines first, the
    // guide equals and takes it; guides first, the baseline cannot beat and the
    // guide keeps it. Measured — moving this block below the guide one changes no
    // test, and so does flipping the flag on its own; only doing both together
    // hands the tie to the baseline. So the property is over-determined, which is
    // robust and means **neither mechanism is individually pinned**. Read the flag
    // as the rule and this position as convention.
    if axes.1 {
        for m in moving_y {
            for t in &s.baselines {
                consider(&mut best_y, m, *t, tolerance, Matched::Baseline(*t), false);
            }
        }
    }
    // Guides last, and allowed to equal rather than beat the best so far, which
    // is what gives them the tie.
    if axes.0 {
        for m in moving_x {
            for t in &s.guides.x {
                consider(&mut best_x, m, *t, tolerance, Matched::RulerGuide, true);
            }
        }
    }
    if axes.1 {
        for m in moving_y {
            for t in &s.guides.y {
                consider(&mut best_y, m, *t, tolerance, Matched::RulerGuide, true);
            }
        }
    }

    // Spacing last, and it has to be **strictly** closer than both to win.
    //
    // The order is the precedence, exactly as it is for the guides above: an
    // alignment lands an edge or a centre on a coordinate, which is an exact
    // statement about two things; a spacing snap invents a position. Where the two
    // are equally near, the alignment is the more useful answer and the more
    // predictable one, because the edge it names is visible and the gap it would
    // have equalized is not. Where spacing is nearer it wins outright, which is what
    // makes dropping a box into the middle of a row feel like it snaps there.
    //
    // A disabled *Snap to shapes* leaves `targets` empty, so this is switched off by
    // construction along with the alignment candidates — one toggle, one source,
    // no second thing to remember (see [`Snapping`]).
    for (axis, best, free) in [
        (Axis::Horizontal, &mut best_x, axes.0),
        (Axis::Vertical, &mut best_y, axes.1),
    ] {
        if !free {
            continue;
        }
        for (delta, spacing) in spacing_candidates(moving, &s.targets, axis) {
            consider_delta(best, delta, tolerance, Matched::Spacing(spacing));
        }
    }

    if let Some((_, delta, what)) = best_x {
        snap.adjust.x = delta;
        let snapped = moving + Vec2::new(delta, 0.0);
        match what {
            Matched::Node(target) => {
                // Which of the three features actually matched, so the line sits on
                // it.
                let at = nearest_of(
                    [snapped.min_x(), snapped.center().x, snapped.max_x()],
                    [target.min_x(), target.center().x, target.max_x()],
                );
                snap.marks.guides.push(Guide {
                    axis: Axis::Vertical,
                    at,
                    from: snapped.min_y().min(target.min_y()),
                    to: snapped.max_y().max(target.max_y()),
                });
            }
            // Measured at `snapped`, not at `moving`: the gaps a spacing snap
            // reports are the ones it just created.
            Matched::Spacing(sp) => {
                snap.marks
                    .gaps
                    .extend(spacing_marks(snapped, sp, Axis::Horizontal))
            }
            Matched::RulerGuide => {}
            // Unreachable: a baseline is a y, and is only ever offered to
            // `best_y`. Spelled out rather than merged into the arm above so that
            // adding an x-bearing source later has to come here and decide.
            Matched::Baseline(_) => {}
        }
    } else {
        // Nothing claimed this axis, so the pixel grid takes it — if it is
        // switched on, and if the gesture is free to move along it at all. The
        // box's *leading* edge is what gets rounded: that is the node's position,
        // the number the inspector's X field holds, and rounding it leaves the
        // width alone, so a whole-pixel shape stays one.
        if s.pixel && axes.0 {
            snap.adjust.x = pixel_delta(moving.min_x());
        }
    }
    if let Some((_, delta, what)) = best_y {
        snap.adjust.y = delta;
        let snapped = moving + Vec2::new(0.0, delta);
        match what {
            Matched::Node(target) => {
                let at = nearest_of(
                    [snapped.min_y(), snapped.center().y, snapped.max_y()],
                    [target.min_y(), target.center().y, target.max_y()],
                );
                snap.marks.guides.push(Guide {
                    axis: Axis::Horizontal,
                    at,
                    from: snapped.min_x().min(target.min_x()),
                    to: snapped.max_x().max(target.max_x()),
                });
            }
            Matched::Spacing(sp) => {
                snap.marks
                    .gaps
                    .extend(spacing_marks(snapped, sp, Axis::Vertical))
            }
            Matched::RulerGuide => {}
            // **A baseline gets a mark where a ruler guide does not**, and the
            // span is the moving box's own: there is no target rect to reach
            // towards, because what was matched is a line inside a text node
            // rather than that node's box, and a mark stretched to the text's
            // bounds would point at the wrong edge of it.
            Matched::Baseline(at) => snap.marks.guides.push(Guide {
                axis: Axis::Horizontal,
                at,
                from: snapped.min_x(),
                to: snapped.max_x(),
            }),
        }
    } else if s.pixel && axes.1 {
        snap.adjust.y = pixel_delta(moving.min_y());
    }

    snap
}

/// Which axes a move may travel along: both, or — while Shift is held — only
/// the one the drag has gone furthest along.
///
/// **Shift constrains, it does not switch snapping off.** Locking a drag to one
/// axis is what Shift does in every design tool, and it is the more useful of the
/// two meanings: snapping already has switches of its own in the Snap menu,
/// where a modifier held down mid-drag cannot be (§15).
///
/// Re-read from the live delta on every frame rather than fixed when Shift went
/// down, so a drag that starts sideways and turns downward follows the pointer to
/// the axis it is now mostly travelling along. The tie goes to x, which is what
/// makes the first pixel of a Shift-drag horizontal rather than undefined.
pub fn move_axes(raw: Vec2, shift: bool) -> (bool, bool) {
    if !shift {
        return (true, true);
    }
    let horizontal = raw.x.abs() >= raw.y.abs();
    (horizontal, !horizontal)
}

/// `raw` with the axes `move_axes` forbids zeroed out.
pub fn constrain(raw: Vec2, axes: (bool, bool)) -> Vec2 {
    Vec2::new(
        if axes.0 { raw.x } else { 0.0 },
        if axes.1 { raw.y } else { 0.0 },
    )
}

/// The result of snapping a single dragged point.
#[derive(Default, Debug)]
pub struct PointSnap {
    pub at: Point,
    pub guides: Vec<Guide>,
}

/// Snap a dragged **point** — a resize handle — onto nearby features, on the
/// axes it is free to move.
///
/// The resize counterpart of [`snap_rect`], and it has to be a different
/// function rather than a reuse of it. A move translates the whole box, so
/// every one of its edges is a candidate and any of them may pull it; a resize
/// moves *one corner or edge* and must leave the opposite one exactly where it
/// is. Snapping the box would drag the anchored side along with it, which is
/// the one thing a resize may not do.
///
/// `axes` says which world axes the handle actually travels along — a side
/// handle on an upright node moves in one, a corner in both, and a rotated
/// node's side in both (`OndinApp::resize_axes`). Snapping an axis the handle
/// cannot move would move the anchored edge.
pub fn snap_point(at: Point, axes: (bool, bool), s: &Snapping) -> PointSnap {
    let tolerance = TOLERANCE_PX / s.zoom.max(f64::EPSILON);
    let mut out = PointSnap {
        at,
        guides: Vec::new(),
    };

    let mut best_x: Option<(f64, f64, Matched)> = None;
    let mut best_y: Option<(f64, f64, Matched)> = None;
    for target in &s.targets {
        if axes.0 {
            for t in [target.min_x(), target.center().x, target.max_x()] {
                consider(
                    &mut best_x,
                    at.x,
                    t,
                    tolerance,
                    Matched::Node(*target),
                    false,
                );
            }
        }
        if axes.1 {
            for t in [target.min_y(), target.center().y, target.max_y()] {
                consider(
                    &mut best_y,
                    at.y,
                    t,
                    tolerance,
                    Matched::Node(*target),
                    false,
                );
            }
        }
    }
    if axes.0 {
        for t in &s.guides.x {
            consider(&mut best_x, at.x, *t, tolerance, Matched::RulerGuide, true);
        }
    }
    if axes.1 {
        for t in &s.guides.y {
            consider(&mut best_y, at.y, *t, tolerance, Matched::RulerGuide, true);
        }
    }

    if let Some((_, delta, what)) = best_x {
        out.at.x += delta;
        // A line from the target to the handle, so the edge being dragged and
        // the thing it caught are visibly connected. Nothing for a ruler guide,
        // as in `snap_rect`: it is already a drawn line.
        if let Matched::Node(t) = what {
            out.guides.push(Guide {
                axis: Axis::Vertical,
                at: out.at.x,
                from: t.min_y().min(out.at.y),
                to: t.max_y().max(out.at.y),
            });
        }
    } else if s.pixel && axes.0 {
        // The dragged edge lands on a whole pixel, so a resize against an
        // already-whole opposite edge produces a whole width.
        //
        // ⚠️ **`s.pixel`, which this arm and the one below it used to omit**
        // (§15 D440).
        // The other three pixel-fallback sites all ask — `snap_rect` writes
        // `if s.pixel && axes.0`, `snap_guide` writes `None if s.pixel =>`, and
        // `canvas::axial_corner` writes `if self.snap_grid` — and `pen_point`'s
        // own doc quotes that last one as *"still rounds along the constrained
        // direction **when the grid is on**"*. So the field `Snapping::pixel`
        // documents itself as *Snap to grid* and five gestures ignored it:
        // resize handles, line endpoints, the pen, point drags and segment
        // drags all rounded to the whole-unit lattice with every snap source
        // switched off. At 8× zoom that is a visible click onto a lattice the
        // user has turned off, and a fractional resize was unreachable by hand.
        out.at.x += pixel_delta(out.at.x);
    }
    if let Some((_, delta, what)) = best_y {
        out.at.y += delta;
        if let Matched::Node(t) = what {
            out.guides.push(Guide {
                axis: Axis::Horizontal,
                at: out.at.y,
                from: t.min_x().min(out.at.x),
                to: t.max_x().max(out.at.x),
            });
        }
    } else if s.pixel && axes.1 {
        out.at.y += pixel_delta(out.at.y);
    }
    out
}

/// Snap a ruler guide being dragged onto the edges and centres of nearby nodes.
///
/// The mirror of [`snap_rect`]'s guide handling, and it earns its place through
/// a workflow rather than through symmetry: block a layout out with plain
/// rectangles, drop guides onto their edges, delete the rectangles. Without
/// this, the guides in that sequence are placed by eye and the rectangles were
/// pointless.
///
/// Only nodes, deliberately — not other guides. Two guides at the same
/// coordinate are a mistake rather than a goal, and making them attract each
/// other would make that mistake easier to commit.
pub fn snap_guide(position: f64, axis: Axis, s: &Snapping) -> f64 {
    let tolerance = TOLERANCE_PX / s.zoom.max(f64::EPSILON);
    let mut best: Option<(f64, f64)> = None;
    for t in &s.targets {
        let features = match axis {
            Axis::Vertical => [t.min_x(), t.center().x, t.max_x()],
            Axis::Horizontal => [t.min_y(), t.center().y, t.max_y()],
        };
        for f in features {
            let d = (f - position).abs();
            if d <= tolerance && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, f));
            }
        }
    }
    // A guide that caught nothing still lands on a whole pixel, when the grid is
    // on. A guide half a pixel off is worse than a shape half a pixel off:
    // everything aligned to it inherits the error, which is the opposite of what
    // a guide is for.
    match best {
        Some((_, at)) => at,
        None if s.pixel => to_pixel(position),
        None => position,
    }
}

/// Keep `best` if it is closer than the pair `(m, t)`; otherwise take the new
/// one. `ties` lets an equally-close candidate displace the incumbent.
fn consider(
    best: &mut Option<(f64, f64, Matched)>,
    m: f64,
    t: f64,
    tolerance: f64,
    what: Matched,
    ties: bool,
) {
    consider_delta_with(best, t - m, tolerance, what, ties);
}

/// [`consider`] for a candidate that already knows its delta rather than the pair of
/// coordinates it came from.
///
/// A spacing candidate has no "feature that matched a feature" — its delta is the
/// answer to an arithmetic question about three boxes, not the distance between two
/// numbers — so it cannot be expressed as an `(m, t)` pair. Never takes ties, for the
/// reason at the call site.
fn consider_delta(
    best: &mut Option<(f64, f64, Matched)>,
    delta: f64,
    tolerance: f64,
    what: Matched,
) {
    consider_delta_with(best, delta, tolerance, what, false);
}

fn consider_delta_with(
    best: &mut Option<(f64, f64, Matched)>,
    delta: f64,
    tolerance: f64,
    what: Matched,
    ties: bool,
) {
    let d = delta.abs();
    if d > tolerance {
        return;
    }
    let better = match *best {
        None => true,
        Some((bd, _, _)) if ties => d <= bd,
        Some((bd, _, _)) => d < bd,
    };
    if better {
        *best = Some((d, delta, what));
    }
}

/// The coordinate where a moving feature and a target feature coincide.
fn nearest_of(moving: [f64; 3], target: [f64; 3]) -> f64 {
    let mut best = (f64::INFINITY, target[0]);
    for m in moving {
        for t in target {
            let d = (m - t).abs();
            if d < best.0 {
                best = (d, t);
            }
        }
    }
    best.1
}

/// World bounds worth snapping to: everything visible that the gesture is not
/// itself carrying.
///
/// **The gesture's own subtree, and nothing else.** A node cannot snap to itself
/// or to something it is dragging along with it — those bounds move with the
/// pointer, so a match is guaranteed and means nothing. Every other visible
/// node is fair game, its **container chain included**.
///
/// That last part is the correction. This used to exclude the moving node's
/// ancestors too, on the reasoning that a node is always flush with some feature
/// of its own container and would feel stuck to its parent. True of a group,
/// whose bounds *are* the union of its children — but a group is not in the
/// spatial index ([`ondin_core::nodes_in_view`] yields drawable leaves and
/// artboards), so that exclusion could never fire on one. The only ancestor it
/// ever reached was an **artboard**, whose rect is its own declared size and has
/// nothing to do with what is inside it. So the rule excluded exactly the thing
/// its own comment claimed it kept, and a shape could not snap to the edges of
/// the frame it lived in — which, since a layer joins a frame the moment it is
/// more than half inside one (`canvas::move_tx`), is very nearly every shape on
/// the page (§15).
pub fn targets(
    doc: &Document,
    res: &Resolved,
    moving: &[NodeId],
    view: Rect,
    limit: usize,
) -> Vec<Rect> {
    let mut out = Vec::new();
    for id in ondin_core::nodes_in_view(res, view) {
        if moving.contains(&id) || is_inside_any(doc, id, moving) {
            continue;
        }
        if doc.get(id).is_some_and(|n| !n.visible()) {
            continue;
        }
        if let Some(b) = res.world_bounds(id) {
            out.push(b);
        }
        // Snapping is a comfort feature, not a correctness one: on a very
        // busy viewport, stop rather than spend the frame on it.
        if out.len() >= limit {
            break;
        }
    }
    out
}

fn ancestors(doc: &Document, id: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut cursor = doc.get(id).and_then(|n| n.parent());
    while let Some(p) = cursor {
        out.push(p);
        cursor = doc.get(p).and_then(|n| n.parent());
    }
    out
}

/// Whether `id` sits under any of `moving` — the ancestry half of *"everything
/// being transformed is excluded from the snap targets"*.
///
/// **`pub(crate)` because `canvas::baseline_lines` asks the same question**
/// (§15 D468). That function filtered on `moving.contains(&id)` alone, so a text
/// node **inside** a moving group was still offered as a baseline and the group
/// snapped to the baseline of the text it was carrying. One rule with two
/// spellings, one of them incomplete; there is one spelling now.
pub(crate) fn is_inside_any(doc: &Document, id: NodeId, moving: &[NodeId]) -> bool {
    ancestors(doc, id).iter().any(|a| moving.contains(a))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::Rect;

    /// An unconstrained move — both axes free, which is every gesture that is
    /// not holding Shift. The axis lock has its own tests below.
    const FREE: (bool, bool) = (true, true);

    fn r(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        Rect::new(x0, y0, x1, y1)
    }

    #[test]
    fn a_near_edge_snaps_flush() {
        // Moving box's left edge is 2 units from the target's left edge.
        let moving = r(102.0, 300.0, 152.0, 350.0);
        let target = r(100.0, 0.0, 200.0, 50.0);
        let snap = snap_rect(moving, FREE, &shapes(&[target], 1.0));
        assert!((snap.adjust.x + 2.0).abs() < 1e-9, "{:?}", snap.adjust);
        assert_eq!(snap.marks.guides.len(), 1);
        assert_eq!(snap.marks.guides[0].axis, Axis::Vertical);
        assert!((snap.marks.guides[0].at - 100.0).abs() < 1e-9);
    }

    #[test]
    fn distant_edges_do_not_snap() {
        // Already on whole pixels, so the fallback has nothing to do either and
        // the box genuinely does not move.
        let moving = r(400.0, 400.0, 450.0, 450.0);
        let target = r(0.0, 0.0, 50.0, 50.0);
        let snap = snap_rect(moving, FREE, &shapes(&[target], 1.0));
        assert_eq!(snap.adjust, Vec2::ZERO);
        assert!(snap.marks.guides.is_empty());
    }

    /// With nothing in reach, an axis falls to the pixel grid: the box's leading
    /// edge lands on a whole unit and its size is untouched.
    #[test]
    fn an_unclaimed_axis_falls_to_the_pixel_grid() {
        let moving = r(400.3, 400.8, 450.3, 450.8);
        let snap = snap_rect(moving, FREE, &grid(1.0));
        let landed = moving + snap.adjust;
        assert!((landed.min_x() - 400.0).abs() < 1e-9, "{landed:?}");
        assert!((landed.min_y() - 401.0).abs() < 1e-9, "{landed:?}");
        assert!((landed.width() - 50.0).abs() < 1e-9, "size changed");
        // The grid is not a *relationship*, so it explains itself with nothing:
        // the lattice is already on screen wherever it is worth drawing.
        assert!(snap.marks.guides.is_empty());
    }

    /// An alignment match beats the grid outright on its own axis. A shape lined
    /// up with an edge at 100.5 belongs at 100.5 — rounding afterwards would
    /// undo the alignment that was just found.
    #[test]
    fn an_alignment_match_is_not_then_rounded_away() {
        let moving = r(102.0, 300.4, 152.0, 350.4);
        let target = r(100.5, 0.0, 200.5, 50.0);
        // Both sources on, which is the whole point: the grid must not undo what
        // the shapes just did.
        let snap = snap_rect(
            moving,
            FREE,
            &Snapping {
                targets: vec![target],
                pixel: true,
                ..Snapping::none(1.0)
            },
        );
        let landed = moving + snap.adjust;
        assert!(
            (landed.min_x() - 100.5).abs() < 1e-9,
            "x was rounded off its alignment: {landed:?}"
        );
        // y caught nothing, so y alone went to the grid.
        assert!((landed.min_y() - 300.0).abs() < 1e-9, "{landed:?}");

        // The same for a guide at a fractional coordinate: the user put it
        // there, so it is honoured exactly.
        let g = snap_rect(
            moving,
            FREE,
            &Snapping {
                guides: lines(&[100.5], &[]),
                pixel: true,
                ..Snapping::none(1.0)
            },
        );
        assert!(((moving + g.adjust).min_x() - 100.5).abs() < 1e-9);
    }

    /// Each source can be switched off on its own, and off means off — not
    /// "off unless something else is also in reach". This is the Snap menu's
    /// contract, and the reason a disabled source arrives as an empty list.
    #[test]
    fn each_snap_source_can_be_switched_off_independently() {
        let moving = r(102.4, 203.4, 152.4, 253.4);
        let target = r(100.0, 200.0, 200.0, 260.0);

        // Everything off: the box goes exactly where it was put.
        assert_eq!(
            snap_rect(moving, FREE, &Snapping::none(1.0)).adjust,
            Vec2::ZERO
        );

        // Shapes only — lands on the shape, not on the grid.
        let s = snap_rect(moving, FREE, &shapes(&[target], 1.0));
        assert!(((moving + s.adjust).min_x() - 100.0).abs() < 1e-9);

        // Grid only — rounds, and ignores the shape 2.4 away.
        let g = snap_rect(moving, FREE, &grid(1.0));
        assert!(
            ((moving + g.adjust).min_x() - 102.0).abs() < 1e-9,
            "{:?}",
            g.adjust
        );

        // Guides only, with a shape in reach that must not be consulted: the
        // guide at 105 is 2.6 from the box's left edge, the shape's own edge at
        // 100 is 2.4 — nearer, and still ignored.
        let gl = snap_rect(moving, FREE, &guides(&[105.0], &[], 1.0));
        assert!(
            ((moving + gl.adjust).min_x() - 105.0).abs() < 1e-9,
            "{:?}",
            gl.adjust
        );
        assert_eq!(gl.adjust.y, 0.0, "y had no guide and no grid");
    }

    /// A resize rounds the dragged edge, and still only on the axes it moves —
    /// the grid must not become a back door to moving the anchored edge.
    #[test]
    fn a_resize_rounds_the_dragged_edge_only() {
        let at = Point::new(102.4, 203.6);
        let side = snap_point(at, (true, false), &grid(1.0));
        assert!((side.at.x - 102.0).abs() < 1e-9, "{:?}", side.at);
        assert_eq!(side.at.y, 203.6, "the anchored axis was rounded");

        let corner = snap_point(at, (true, true), &grid(1.0));
        assert!((corner.at.x - 102.0).abs() < 1e-9);
        assert!((corner.at.y - 204.0).abs() < 1e-9);

        // Frozen on both: untouched, grid included.
        assert_eq!(snap_point(at, (false, false), &grid(1.0)).at, at);
    }

    /// A guide that catches nothing still lands on a whole pixel — a guide half
    /// a pixel off passes its error to everything aligned against it.
    #[test]
    fn a_guide_that_catches_nothing_lands_on_a_whole_pixel() {
        assert_eq!(snap_guide(400.4, Axis::Vertical, &grid(1.0)), 400.0);
        assert_eq!(snap_guide(400.6, Axis::Horizontal, &grid(1.0)), 401.0);
        // But a shape edge at a fraction still wins, as it does everywhere else.
        let target = r(100.5, 40.0, 200.0, 240.0);
        let got = snap_guide(102.0, Axis::Vertical, &shapes(&[target], 1.0));
        assert!((got - 100.5).abs() < 1e-9, "{got}");
    }

    #[test]
    fn centres_snap_to_centres() {
        // Target centre x = 150; moving centre x = 153.
        let moving = r(128.0, 300.0, 178.0, 350.0);
        let target = r(100.0, 0.0, 200.0, 40.0);
        let snap = snap_rect(moving, FREE, &shapes(&[target], 1.0));
        assert!((snap.adjust.x + 3.0).abs() < 1e-9, "{:?}", snap.adjust);
        assert!((snap.marks.guides[0].at - 150.0).abs() < 1e-9);
    }

    #[test]
    fn tolerance_follows_zoom_not_world_units() {
        // Nearest feature pair is 20 world units apart: far at 1x, but within
        // tolerance at 0.2x zoom (20 * 0.2 = 4 screen px, under the 6px
        // threshold). Same y extent, so only x is in question.
        let moving = r(70.0, 0.0, 120.0, 50.0);
        let target = r(0.0, 0.0, 50.0, 50.0);
        assert_eq!(
            snap_rect(moving, FREE, &shapes(&[target], 1.0)).adjust,
            Vec2::ZERO
        );
        assert!(snap_rect(moving, FREE, &shapes(&[target], 0.2)).adjust.x != 0.0);
    }

    #[test]
    fn both_axes_can_snap_at_once() {
        let moving = r(102.0, 203.0, 152.0, 253.0);
        let target = r(100.0, 200.0, 200.0, 260.0);
        let snap = snap_rect(moving, FREE, &shapes(&[target], 1.0));
        assert!(snap.adjust.x != 0.0 && snap.adjust.y != 0.0);
        assert_eq!(snap.marks.guides.len(), 2);
    }

    #[test]
    fn the_closest_candidate_wins() {
        let moving = r(103.0, 500.0, 153.0, 550.0);
        // Left edge is 3 away from 100, but only 1 away from 104.
        let near = r(104.0, 0.0, 154.0, 20.0);
        let far = r(100.0, 0.0, 150.0, 20.0);
        let snap = snap_rect(moving, FREE, &shapes(&[far, near], 1.0));
        assert!((snap.adjust.x - 1.0).abs() < 1e-9, "{:?}", snap.adjust);
    }

    fn lines(x: &[f64], y: &[f64]) -> GuideLines {
        GuideLines {
            x: x.to_vec(),
            y: y.to_vec(),
        }
    }

    /// Snapping with the shape targets on and the other two off — the shape of
    /// the tests written before the Snap menu existed, so they still say exactly
    /// what they always said.
    fn shapes(targets: &[Rect], zoom: f64) -> Snapping {
        Snapping {
            targets: targets.to_vec(),
            ..Snapping::none(zoom)
        }
    }

    /// Snapping to ruler guides only.
    fn guides(x: &[f64], y: &[f64], zoom: f64) -> Snapping {
        Snapping {
            guides: lines(x, y),
            ..Snapping::none(zoom)
        }
    }

    /// Snapping to the pixel grid only.
    fn grid(zoom: f64) -> Snapping {
        Snapping {
            pixel: true,
            ..Snapping::none(zoom)
        }
    }

    /// A ruler guide snaps like any other feature, on either axis, and with the
    /// same screen-pixel tolerance.
    #[test]
    fn a_box_snaps_to_a_ruler_guide() {
        let moving = r(102.0, 203.0, 152.0, 253.0);
        // A vertical guide at x=100 and a horizontal one at y=200.
        let snap = snap_rect(moving, FREE, &guides(&[100.0], &[200.0], 1.0));
        assert!((snap.adjust.x + 2.0).abs() < 1e-9, "{:?}", snap.adjust);
        assert!((snap.adjust.y + 3.0).abs() < 1e-9, "{:?}", snap.adjust);

        // Out of reach on both axes, and nothing moves.
        let far = snap_rect(moving, FREE, &guides(&[10.0], &[10.0], 1.0));
        assert_eq!(far.adjust, Vec2::ZERO);
    }

    /// The guide is already a drawn line, so snapping to one draws no second line
    /// along it. A node snap on the other axis still draws its own.
    ///
    /// Deliberately *not* "a line across the whole canvas in its own colour" any
    /// more: neither is true of a frame-scoped or a selected guide, and the rule
    /// never depended on either.
    #[test]
    fn snapping_to_a_ruler_guide_draws_no_extra_line() {
        let moving = r(102.0, 203.0, 152.0, 253.0);
        let guide_only = snap_rect(moving, FREE, &guides(&[100.0], &[], 1.0));
        assert!(guide_only.adjust.x != 0.0);
        assert!(
            guide_only.marks.guides.is_empty(),
            "{:?}",
            guide_only.marks.guides
        );

        // x from the guide, y from a node: one line, for the node.
        let target = r(300.0, 200.0, 400.0, 260.0);
        let both = snap_rect(
            moving,
            FREE,
            &Snapping {
                targets: vec![target],
                guides: lines(&[100.0], &[]),
                ..Snapping::none(1.0)
            },
        );
        assert!(both.adjust.x != 0.0 && both.adjust.y != 0.0);
        assert_eq!(both.marks.guides.len(), 1);
        assert_eq!(both.marks.guides[0].axis, Axis::Horizontal);
    }

    /// A nearer node still wins; only a tie goes to the guide.
    #[test]
    fn a_ruler_guide_takes_ties_but_not_a_closer_node() {
        let moving = r(103.0, 500.0, 153.0, 550.0);
        // Node edge 1 away at 104, guide 3 away at 100 — the node wins, and
        // says so by drawing its line.
        let near_node = r(104.0, 0.0, 154.0, 20.0);
        let node_wins = snap_rect(
            moving,
            FREE,
            &Snapping {
                targets: vec![near_node],
                guides: lines(&[100.0], &[]),
                ..Snapping::none(1.0)
            },
        );
        assert!(
            (node_wins.adjust.x - 1.0).abs() < 1e-9,
            "{:?}",
            node_wins.adjust
        );
        assert_eq!(node_wins.marks.guides.len(), 1);

        // Both 3 away, on opposite sides: the guide takes it, so no line.
        let tied_node = r(106.0, 0.0, 156.0, 20.0);
        let tie = snap_rect(
            moving,
            FREE,
            &Snapping {
                targets: vec![tied_node],
                guides: lines(&[100.0], &[]),
                ..Snapping::none(1.0)
            },
        );
        assert!((tie.adjust.x + 3.0).abs() < 1e-9, "{:?}", tie.adjust);
        assert!(
            tie.marks.guides.is_empty(),
            "the guide won, so it draws nothing"
        );
    }

    /// Snapping to text baselines only.
    fn baselines(ys: &[f64], zoom: f64) -> Snapping {
        Snapping {
            baselines: ys.to_vec(),
            ..Snapping::none(zoom)
        }
    }

    /// **A baseline pulls an edge or a centre, and draws a line saying which**
    /// (§15 D355).
    ///
    /// The mark is the half that separates a baseline from a ruler guide: a guide
    /// is already drawn on the canvas, so a second line along it would say
    /// nothing, whereas a baseline is invisible unless its own node is selected —
    /// and the node being dragged is by definition not that one. Without the mark
    /// the box jumps to a coordinate with nothing on screen explaining it.
    ///
    /// ⚠️ The mark sits on the **baseline**, not on the snapped box's edge. Those
    /// coincide in this fixture, which is why the second case moves a *centre*
    /// onto the baseline instead: there the two differ by half the box, so an
    /// implementation that drew `snapped.min_y()` passes the first and fails the
    /// second. That is the arithmetic error `Matched::Baseline` carrying its own
    /// `f64` exists to make unspellable.
    #[test]
    fn a_baseline_pulls_a_box_and_marks_the_line_it_pulled_to() {
        // Top edge 2 above a baseline at 300.
        let moving = r(10.0, 298.0, 60.0, 348.0);
        let snap = snap_rect(moving, FREE, &baselines(&[300.0], 1.0));
        assert!((snap.adjust.y - 2.0).abs() < 1e-9, "{:?}", snap.adjust);
        assert_eq!(snap.marks.guides.len(), 1, "a baseline snap draws its line");
        let mark = snap.marks.guides[0];
        assert_eq!(mark.axis, Axis::Horizontal, "a baseline pins a y");
        assert!(
            (mark.at - 300.0).abs() < 1e-9,
            "the mark is on the baseline"
        );

        // Centre 1 below a baseline at 500: the box moves up 1, and the mark is
        // still the baseline rather than either edge of the box.
        let by_centre = r(10.0, 476.0, 60.0, 526.0);
        let snap = snap_rect(by_centre, FREE, &baselines(&[500.0], 1.0));
        assert!((snap.adjust.y + 1.0).abs() < 1e-9, "{:?}", snap.adjust);
        assert!(
            (snap.marks.guides[0].at - 500.0).abs() < 1e-9,
            "the mark followed the box's edge instead of the baseline: {:?}",
            snap.marks.guides[0]
        );
    }

    /// **A baseline offers no x**, so a vertical edge near one in *value* is not
    /// a candidate.
    ///
    /// Worth its own case because the source is a bare `Vec<f64>` with nothing in
    /// the type saying which axis it belongs to — the only thing that makes it a
    /// y is that `snap_rect` consults it under `axes.1` alone, which is one line
    /// and easy to copy into the wrong block.
    #[test]
    fn a_baseline_never_claims_the_horizontal_axis() {
        // Left edge at 302, two from the *value* 300, and far from it in y.
        let moving = r(302.0, 20.0, 352.0, 70.0);
        let snap = snap_rect(moving, FREE, &baselines(&[300.0], 1.0));
        assert_eq!(
            snap.adjust.x, 0.0,
            "a baseline pulled an x, so it is being consulted on both axes"
        );
        assert!(snap.marks.guides.is_empty());
    }

    /// **A ruler guide still takes a tie against a baseline, and a nearer
    /// baseline still beats a further guide.**
    ///
    /// The precedence the sources were given: baselines are considered on the
    /// nodes' terms — they must *beat* the best so far — and the guide block runs
    /// after them with the tie. So a line the user placed by hand outranks one
    /// that is a property of somebody's text, where the two are equally near.
    ///
    /// ⚠️ **Two flips predicted, neither bit, and the finding is better than the
    /// test.** Moving the baseline block after the guide block changes nothing;
    /// so does giving baselines the tie flag on its own. The dead heat is settled
    /// by the *pair* — guides tie, baselines do not — and that holds in either
    /// order, so each mechanism alone is enough. **Only both together** (baselines
    /// after guides *and* allowed to tie) hands it over, and then this fails on
    /// the first assertion at `y: 3.0` where `-3.0` is due.
    ///
    /// So this test pins the behaviour and pins **neither implementation detail**,
    /// which is worth knowing before anyone "simplifies" one of them on the
    /// strength of a green suite. The comment at the call site now says the same.
    #[test]
    fn a_ruler_guide_takes_a_tie_against_a_baseline() {
        let moving = r(10.0, 303.0, 60.0, 353.0);
        // Guide 3 above at 300, baseline 3 below at 306 — a dead heat.
        let tie = snap_rect(
            moving,
            FREE,
            &Snapping {
                baselines: vec![306.0],
                guides: lines(&[], &[300.0]),
                ..Snapping::none(1.0)
            },
        );
        assert!((tie.adjust.y + 3.0).abs() < 1e-9, "{:?}", tie.adjust);
        assert!(
            tie.marks.guides.is_empty(),
            "the ruler guide won the tie, so nothing is drawn — a mark here means \
             the baseline took it"
        );

        // Baseline 1 away, guide 4 away: the baseline wins outright.
        let nearer = snap_rect(
            moving,
            FREE,
            &Snapping {
                baselines: vec![304.0],
                guides: lines(&[], &[299.0]),
                ..Snapping::none(1.0)
            },
        );
        assert!((nearer.adjust.y - 1.0).abs() < 1e-9, "{:?}", nearer.adjust);
        assert_eq!(nearer.marks.guides.len(), 1, "and it draws its line");

        // **And a node edge keeps a tie too — this is where the flag alone does
        // the work.** Nodes are always considered first and also do not tie, so
        // unlike the guide case above there is no second mechanism propping this
        // up: give baselines `true` and they take it. Flipped exactly that way,
        // and it fails here at 3.0 against −3.0 while every other case stays
        // green, which is what makes this the assertion the flag is pinned by.
        let node_tie = snap_rect(
            moving,
            FREE,
            &Snapping {
                baselines: vec![306.0],
                targets: vec![r(200.0, 300.0, 260.0, 320.0)],
                ..Snapping::none(1.0)
            },
        );
        assert!(
            (node_tie.adjust.y + 3.0).abs() < 1e-9,
            "a baseline took a dead heat off a node edge: {:?}",
            node_tie.adjust
        );
    }

    /// Centres snap to guides too, not just edges — a guide down the middle of
    /// a layout is there to centre things on.
    #[test]
    fn a_box_centres_on_a_ruler_guide() {
        // Centre x = 153; guide at 150.
        let moving = r(128.0, 300.0, 178.0, 350.0);
        let snap = snap_rect(moving, FREE, &guides(&[150.0], &[], 1.0));
        assert!((snap.adjust.x + 3.0).abs() < 1e-9, "{:?}", snap.adjust);
    }

    /// A resize snaps the dragged handle and nothing else. Only the axes the
    /// handle travels along may move: snapping an axis it is anchoring would
    /// drag the opposite edge, which is the one thing a resize may not do.
    #[test]
    fn a_resize_handle_snaps_only_on_the_axes_it_moves() {
        let at = Point::new(102.0, 203.0);
        let both_axes = || guides(&[100.0], &[200.0], 1.0);

        // A corner travels along both, so both catch.
        let corner = snap_point(at, (true, true), &both_axes());
        assert!((corner.at.x - 100.0).abs() < 1e-9, "{:?}", corner.at);
        assert!((corner.at.y - 200.0).abs() < 1e-9, "{:?}", corner.at);

        // A vertical side travels along x only: y must be left exactly alone,
        // even though a horizontal guide is within reach of it.
        let side = snap_point(at, (true, false), &both_axes());
        assert!((side.at.x - 100.0).abs() < 1e-9, "{:?}", side.at);
        assert_eq!(side.at.y, 203.0, "the anchored axis moved");

        // And neither, when the handle is frozen on both.
        assert_eq!(snap_point(at, (false, false), &both_axes()).at, at);
    }

    /// It snaps to shape edges as well as guides, and draws a line for a shape
    /// (which needs one to show what it caught) but not for a guide.
    #[test]
    fn a_resize_handle_snaps_to_shapes_and_explains_only_those() {
        let at = Point::new(102.0, 500.0);
        let target = r(100.0, 0.0, 200.0, 60.0);
        let node = snap_point(at, (true, false), &shapes(&[target], 1.0));
        assert!((node.at.x - 100.0).abs() < 1e-9, "{:?}", node.at);
        assert_eq!(node.guides.len(), 1);
        // The line reaches from the shape it caught to the handle itself.
        let g = node.guides[0];
        assert_eq!(g.axis, Axis::Vertical);
        assert!(g.from <= 0.0 && g.to >= 500.0, "{g:?}");

        let guide = snap_point(at, (true, false), &guides(&[100.0], &[], 1.0));
        assert!((guide.at.x - 100.0).abs() < 1e-9);
        assert!(guide.guides.is_empty(), "a guide is already its own line");
    }

    /// A guide being dragged catches shape edges and centres — the block-out
    /// workflow: rectangles, guides onto their sides, delete the rectangles.
    #[test]
    fn a_dragged_guide_catches_shape_edges_and_centres() {
        let target = r(100.0, 40.0, 200.0, 240.0);
        let boxes = [target];
        // Left edge, centre, right edge on the x axis.
        for (from, want) in [(102.0, 100.0), (148.0, 150.0), (203.0, 200.0)] {
            let got = snap_guide(from, Axis::Vertical, &shapes(&boxes, 1.0));
            assert!((got - want).abs() < 1e-9, "{from} -> {got}, want {want}");
        }
        // And the y features on the other axis.
        for (from, want) in [(43.0, 40.0), (142.0, 140.0), (238.0, 240.0)] {
            let got = snap_guide(from, Axis::Horizontal, &shapes(&boxes, 1.0));
            assert!((got - want).abs() < 1e-9, "{from} -> {got}, want {want}");
        }
        // Out of reach, and the guide stays exactly where the pointer put it.
        assert_eq!(
            snap_guide(400.0, Axis::Vertical, &shapes(&boxes, 1.0)),
            400.0
        );
        // The tolerance is in screen pixels here too.
        assert_eq!(
            snap_guide(120.0, Axis::Vertical, &shapes(&boxes, 1.0)),
            120.0
        );
        assert!((snap_guide(120.0, Axis::Vertical, &shapes(&boxes, 0.2)) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn guides_span_both_boxes_so_the_relationship_is_visible() {
        let moving = r(102.0, 300.0, 152.0, 350.0);
        let target = r(100.0, 0.0, 200.0, 50.0);
        let g = snap_rect(moving, FREE, &shapes(&[target], 1.0))
            .marks
            .guides[0];
        assert!(g.from <= 0.0 && g.to >= 350.0, "{g:?}");
    }

    // --- spacing: equal gaps -----------------------------------------------

    /// Two neighbours far enough apart to sit between, with bands tall enough that
    /// **no alignment candidate is in reach** — otherwise these tests would be
    /// measuring the alignment snap. The moving box is 40 wide, so the position with
    /// equal gaps is `180..220` and every case below is stated as an offset from it.
    fn a_row() -> [Rect; 2] {
        [r(0.0, 0.0, 100.0, 1000.0), r(300.0, 0.0, 400.0, 1000.0)]
    }

    fn gaps_of(snap: &Snap) -> Vec<f64> {
        let mut out: Vec<f64> = snap.marks.gaps.iter().map(|m| m.distance()).collect();
        out.sort_by(f64::total_cmp);
        out
    }

    /// **A box dropped between two others lands halfway between them**, and the two
    /// labels quote the gaps it *made*.
    ///
    /// The second half is the one worth guarding. A candidate is judged before it
    /// wins, so the gaps it produces are only true at the position it moves the box
    /// to; building the measures where the pointer actually is would label them 84
    /// and 76 — off by exactly the delta the snap is about, which is the one error
    /// that makes the numbers worse than no numbers.
    #[test]
    fn a_box_between_two_others_centres_itself_and_says_by_how_much() {
        // 4 right of centre, inside the 6px tolerance.
        let moving = r(184.0, 17.0, 224.0, 73.0);
        let snap = snap_rect(moving, FREE, &shapes(&a_row(), 1.0));
        assert!((snap.adjust.x + 4.0).abs() < 1e-9, "{:?}", snap.adjust);
        let landed = moving + snap.adjust;
        assert!((landed.min_x() - 180.0).abs() < 1e-9, "{landed:?}");

        assert_eq!(
            gaps_of(&snap),
            vec![80.0, 80.0],
            "the labels must quote the gaps the snap made, not the 84/76 it found"
        );
        // Gaps are horizontal distances, so they are drawn as horizontal lines — and
        // an equalized axis draws no alignment line, because nothing aligned.
        assert!(snap.marks.gaps.iter().all(|m| m.axis == Axis::Horizontal));
        assert!(snap.marks.guides.is_empty(), "{:?}", snap.marks.guides);
    }

    /// **Past the end of a run, the gap the run already has is the gap the next box
    /// takes** — so a row spaces itself as it is built rather than only once it is
    /// finished and distributed.
    #[test]
    fn a_third_box_takes_the_rhythm_the_first_two_set() {
        // A 0..100, B 200..300: an existing gap of 100.
        let row = [r(0.0, 0.0, 100.0, 1000.0), r(200.0, 0.0, 300.0, 1000.0)];
        // 3 too far right of the rhythm position, which is 400.
        let moving = r(403.0, 17.0, 453.0, 73.0);
        let snap = snap_rect(moving, FREE, &shapes(&row, 1.0));
        assert!((snap.adjust.x + 3.0).abs() < 1e-9, "{:?}", snap.adjust);
        assert!(((moving + snap.adjust).min_x() - 400.0).abs() < 1e-9);
        // Both the gap it copied and the gap it made, so "the same as that" is
        // visible rather than inferred.
        assert_eq!(gaps_of(&snap), vec![100.0, 100.0]);
    }

    /// **An alignment wins unless spacing is strictly nearer.** An edge landing on a
    /// coordinate is an exact statement about two visible things; a spacing snap
    /// invents a position, so it does not get to displace one for free.
    #[test]
    fn an_alignment_beats_spacing_unless_spacing_is_strictly_nearer() {
        let moving = r(184.0, 17.0, 224.0, 73.0); // spacing wants -4

        // A third box in a band of its own, so it can align but can never be a
        // spacing neighbour — and shaped so its **right edge is its only feature in
        // reach**, which is the whole difficulty of writing this fixture. A box has
        // three features per axis, and the first version of this was 100 wide at
        // x: its *centre* then landed 4 from the moving box's right edge, so the
        // test compared an alignment against an alignment and reported that spacing
        // had lost. Left long and starting far away, the left edge and the centre
        // are both out of range and the delta is the one being asked about.
        let elsewhere = |right: f64| r(100.0, 2000.0, right, 2100.0);

        // Tie at 4: the alignment takes it, and says so by drawing a line and no
        // numbers.
        let tie = snap_rect(
            moving,
            FREE,
            &shapes(&[a_row()[0], a_row()[1], elsewhere(180.0)], 1.0),
        );
        assert!((tie.adjust.x + 4.0).abs() < 1e-9, "{:?}", tie.adjust);
        assert!(tie.marks.gaps.is_empty(), "spacing took a tie");
        assert_eq!(tie.marks.guides.len(), 1, "{:?}", tie.marks.guides);

        // Alignment nearer at 2: it wins on distance as well.
        let nearer = snap_rect(
            moving,
            FREE,
            &shapes(&[a_row()[0], a_row()[1], elsewhere(182.0)], 1.0),
        );
        assert!((nearer.adjust.x + 2.0).abs() < 1e-9, "{:?}", nearer.adjust);
        assert!(nearer.marks.gaps.is_empty());

        // Alignment further at 6: spacing wins outright, which is what makes
        // dropping a box into a row feel like it snaps there.
        let spacing = snap_rect(
            moving,
            FREE,
            &shapes(&[a_row()[0], a_row()[1], elsewhere(178.0)], 1.0),
        );
        assert!(
            (spacing.adjust.x + 4.0).abs() < 1e-9,
            "{:?}",
            spacing.adjust
        );
        assert_eq!(gaps_of(&spacing), vec![80.0, 80.0]);
    }

    /// **Boxes that do not share a band are not a row.** Two shapes whose vertical
    /// extents miss each other entirely have no horizontal gap worth equalizing, and
    /// a snap that thought otherwise would drag a layer sideways to line it up with
    /// something on the other side of the page.
    #[test]
    fn spacing_ignores_boxes_that_are_not_in_the_same_row() {
        let moving = r(184.0, 17.0, 224.0, 73.0);
        // The same row, moved down past the moving box entirely.
        let away = [r(0.0, 500.0, 100.0, 600.0), r(300.0, 500.0, 400.0, 600.0)];
        let snap = snap_rect(moving, FREE, &shapes(&away, 1.0));
        assert!(snap.marks.gaps.is_empty(), "{:?}", snap.marks.gaps);
        // The pixel grid is what claims x instead, the box being already whole.
        assert_eq!(snap.adjust.x, 0.0, "{:?}", snap.adjust);

        // Touching bands are still not a row: flush edges share no interior.
        let flush = [r(0.0, 73.0, 100.0, 600.0), r(300.0, 73.0, 400.0, 600.0)];
        assert!(
            snap_rect(moving, FREE, &shapes(&flush, 1.0))
                .marks
                .gaps
                .is_empty()
        );
    }

    /// **A gap too small to be spacing is left to alignment** — `MIN_GAP`.
    ///
    /// **This has to be tested just above zero, not at it.** The obvious fixture is
    /// a row of *flush* boxes, and it proves nothing: a zero-length gap is dropped by
    /// `measure::gap`'s own epsilon, so `marks.gaps` comes back empty whether the
    /// floor exists or not, and the test passes for a reason that has nothing to do
    /// with it. Checked, and it did exactly that.
    ///
    /// Just above zero the floor is the only thing deciding, and it decides something
    /// visible. Gaps of 0.4 and 0.5 put the *spacing* delta at 0.05 — nearer than
    /// either alignment, which sit at 0.4 and 0.5 — so without the floor spacing wins
    /// a contest it should not be in, moves the box a twentieth of a unit, and labels
    /// two 0.45 gaps that no one was asking about. With it, the box lands flush on the
    /// neighbour it was nearly touching, which is what the designer meant.
    #[test]
    fn a_gap_too_small_to_be_spacing_is_left_to_alignment() {
        let row = [r(0.0, 0.0, 100.0, 1000.0), r(140.9, 0.0, 240.9, 1000.0)];
        let moving = r(100.4, 17.0, 140.4, 73.0); // gaps of 0.4 and 0.5
        let snap = snap_rect(moving, FREE, &shapes(&row, 1.0));
        assert!(
            snap.marks.gaps.is_empty(),
            "0.45 is not spacing: {:?}",
            snap.marks.gaps
        );
        assert!(
            (snap.adjust.x + 0.4).abs() < 1e-9,
            "the left edge should land flush on the neighbour, not 0.05 off it: {:?}",
            snap.adjust
        );

        // The control, so this is not a test about a fixture that could never reach
        // the state: widen the same row and spacing fires on the same geometry.
        let wide = [r(0.0, 0.0, 100.0, 1000.0), r(300.0, 0.0, 400.0, 1000.0)];
        let between = r(184.0, 17.0, 224.0, 73.0);
        assert_eq!(
            gaps_of(&snap_rect(between, FREE, &shapes(&wide, 1.0))),
            vec![80.0, 80.0]
        );
    }

    /// A Shift-locked axis gets no spacing, for the reason it gets no alignment: a
    /// snap there would slide the layer off the line the user is holding it to.
    #[test]
    fn a_locked_axis_gets_no_spacing_either() {
        let moving = r(184.0, 17.0, 224.0, 73.0);
        let locked = snap_rect(moving, (false, true), &shapes(&a_row(), 1.0));
        assert_eq!(locked.adjust.x, 0.0, "{:?}", locked.adjust);
        assert!(locked.marks.gaps.is_empty(), "{:?}", locked.marks.gaps);
        // Free, the same box centres itself.
        assert!((snap_rect(moving, FREE, &shapes(&a_row(), 1.0)).adjust.x + 4.0).abs() < 1e-9);
    }

    /// Spacing is a *move* behaviour and stays one. A resize changes the box's size,
    /// so "equalize these gaps" would have to decide whether to do it by moving the
    /// dragged edge or the anchored one — and the anchored edge is the thing a resize
    /// may not touch (see `snap_point`).
    #[test]
    fn a_resize_handle_gets_no_spacing() {
        let at = Point::new(184.0, 45.0);
        let out = snap_point(at, (true, true), &shapes(&a_row(), 1.0));
        // Only the pixel fallback moved it, and nothing was explained by a gap:
        // `PointSnap` has no gaps to carry.
        assert_eq!(out.at, at);
        assert!(out.guides.is_empty());
    }

    // --- targets: which nodes a gesture may snap to ------------------------

    /// A frame, a shape inside it, and a sibling shape beside the frame.
    fn frame_and_shapes() -> (Document, Resolved, NodeId, NodeId, NodeId) {
        use ondin_core::kurbo::{Affine, Size};
        use ondin_core::{IdSource, NodeKind, Operation, Transaction};

        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (frame, inside, outside) = (ids.mint(), ids.mint(), ids.mint());
        let rect = |size: f64| NodeKind::Rect {
            size: Size::new(size, size),
            corner_radii: Default::default(),
        };
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 400.0),
                },
                transform: Some(Affine::translate((0.0, 0.0))),
                name: None,
            },
            Operation::CreateNode {
                id: inside,
                parent: frame,
                index: 0,
                kind: rect(50.0),
                transform: Some(Affine::translate((100.0, 100.0))),
                name: None,
            },
            Operation::CreateNode {
                id: outside,
                parent: root,
                index: 1,
                kind: rect(50.0),
                transform: Some(Affine::translate((600.0, 600.0))),
                name: None,
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, frame, inside, outside)
    }

    /// **A shape inside a frame can snap to that frame's edges.**
    ///
    /// The regression this exists for: `targets` excluded the moving node's
    /// ancestors, and since the only ancestor kind that reaches the spatial
    /// index is an artboard, the rule excluded frames and nothing else. Because
    /// a layer joins a frame as soon as it is more than half inside one, almost
    /// every shape on the page was in this case — dragging one up to its own
    /// frame's edge felt dead, with no candidate to explain why.
    #[test]
    fn a_shape_snaps_to_the_frame_it_lives_in() {
        let (doc, res, frame, inside, _) = frame_and_shapes();
        let view = r(-1000.0, -1000.0, 1000.0, 1000.0);
        let targets = targets(&doc, &res, &[inside], view, 200);
        let frame_box = res.world_bounds(frame).unwrap();
        assert!(
            targets.contains(&frame_box),
            "the parent frame {frame_box:?} is not a target: {targets:?}"
        );

        // And it snaps: the shape's left edge 2 units off the frame's own.
        let moving = r(2.0, 200.0, 52.0, 250.0);
        let snap = snap_rect(moving, FREE, &shapes(&targets, 1.0));
        assert!((snap.adjust.x + 2.0).abs() < 1e-9, "{:?}", snap.adjust);
    }

    /// The gesture's own subtree still has to stay out — those bounds travel
    /// with the pointer, so they would match at every offset and pin the drag.
    #[test]
    fn a_gesture_never_snaps_to_what_it_is_carrying() {
        let (doc, res, frame, inside, outside) = frame_and_shapes();
        let view = r(-1000.0, -1000.0, 1000.0, 1000.0);

        // Dragging the frame: neither it nor the shape it contains may appear.
        let targets = targets(&doc, &res, &[frame], view, 200);
        for id in [frame, inside] {
            let b = res.world_bounds(id).unwrap();
            assert!(!targets.contains(&b), "{id:?} {b:?} is in {targets:?}");
        }
        // The layer beside the frame is exactly what is left.
        assert_eq!(targets, vec![res.world_bounds(outside).unwrap()]);
    }

    // --- the axis lock -----------------------------------------------------

    /// Shift picks the axis the drag has gone furthest along, and re-picks it as
    /// the drag turns — so a gesture that starts sideways and swings downward
    /// follows the hand rather than staying committed to its first few pixels.
    #[test]
    fn shift_locks_the_axis_the_drag_is_mostly_travelling_along() {
        let v = Vec2::new;
        assert_eq!(move_axes(v(40.0, 3.0), true), (true, false));
        assert_eq!(move_axes(v(-40.0, 3.0), true), (true, false), "leftward");
        assert_eq!(move_axes(v(3.0, 40.0), true), (false, true));
        assert_eq!(move_axes(v(3.0, -40.0), true), (false, true), "upward");
        // The tie, and the first frame of every drag, go to x: an undefined
        // answer here is a layer that will not move at all until the pointer
        // happens to break the tie.
        assert_eq!(move_axes(v(0.0, 0.0), true), (true, false));
        assert_eq!(move_axes(v(9.0, 9.0), true), (true, false));
        // Without Shift nothing is constrained.
        assert_eq!(move_axes(v(3.0, 40.0), false), (true, true));

        // And the constraint really zeroes the other axis.
        assert_eq!(
            constrain(v(40.0, 3.0), move_axes(v(40.0, 3.0), true)),
            v(40.0, 0.0)
        );
    }

    /// **A locked axis comes back with no adjustment and no line.**
    ///
    /// The reason this is gated inside `snap_rect` rather than zeroed by the
    /// caller: a snap on the locked axis would slide the layer off the line the
    /// user is holding it to, and — worse — would draw a guide taking credit for
    /// a move that then got discarded. The box here is 2 from an alignment on
    /// *both* axes, so a leak in either direction shows up.
    #[test]
    fn a_locked_axis_neither_snaps_nor_explains_itself() {
        let moving = r(102.0, 202.0, 152.0, 252.0);
        let target = r(100.0, 200.0, 300.0, 400.0);

        // Free: both axes pull, two lines.
        let free = snap_rect(moving, FREE, &shapes(&[target], 1.0));
        assert!((free.adjust.x + 2.0).abs() < 1e-9, "{:?}", free.adjust);
        assert!((free.adjust.y + 2.0).abs() < 1e-9, "{:?}", free.adjust);
        assert_eq!(free.marks.guides.len(), 2);

        // Locked to x: y is untouched and unexplained.
        let locked = snap_rect(moving, (true, false), &shapes(&[target], 1.0));
        assert!((locked.adjust.x + 2.0).abs() < 1e-9, "{:?}", locked.adjust);
        assert_eq!(locked.adjust.y, 0.0, "y moved on a locked axis");
        assert_eq!(locked.marks.guides.len(), 1);
        assert_eq!(locked.marks.guides[0].axis, Axis::Vertical);

        // And the grid does not sneak in behind it either. Nothing is in reach
        // on y, so the pixel fallback is what would otherwise claim it.
        let off_grid = r(102.0, 202.7, 152.0, 252.7);
        let locked = snap_rect(off_grid, (true, false), &grid(1.0));
        assert_eq!(locked.adjust.y, 0.0, "the grid rounded a locked axis");
        // Free, the same box rounds its leading edge up to 203.
        let free = snap_rect(off_grid, FREE, &grid(1.0));
        assert!((free.adjust.y - 0.3).abs() < 1e-9, "{:?}", free.adjust);
    }

    /// **`[S13.2-L2-01]`'s loss: `snap_point` rounded to the lattice whether
    /// *Snap to grid* was on or off.**
    ///
    /// Three of the four pixel-fallback sites consult the toggle — `snap_rect`
    /// (`if s.pixel && axes.0`), `snap_guide` (`None if s.pixel =>`) and
    /// `canvas::axial_corner` (`if self.snap_grid`), whose guard `pen_point`'s
    /// doc quotes by name — and this one did not, while `Snapping::pixel`'s own
    /// doc calls the field *Snap to grid*. **Five gestures reach it**: resize
    /// handles, line endpoints, the pen, point drags and segment drags. Switch
    /// every snap source off in the top bar's Snap menu and all five still
    /// landed on whole world units; at 8× zoom that is a visible click onto a
    /// lattice the user has turned off, and a fractional resize was unreachable
    /// by hand.
    ///
    /// **Both axes and both directions**, because the two arms are separate
    /// `else if`s and a fix that repaired one would look right in a single-axis
    /// probe. The grid-on case is the control: this is a toggle being honoured,
    /// not a rounding being removed.
    ///
    /// Flip-check, run per arm: dropping `s.pixel` from the **y** arm fails at
    /// the very first assertion, `(102.4, 204.0)` against `(102.4, 203.6)` — the
    /// axis is named by the pair, which is why both coordinates are compared
    /// together rather than one at a time. ⚠️ **The predicted site was the
    /// single-axis assertion further down and it never runs**: the first
    /// assertion covers both axes, so it catches either arm, and the single-axis
    /// pair below is there for the *locked*-axis rule rather than as a second
    /// witness to this one.
    #[test]
    fn a_dragged_point_falls_to_the_grid_only_when_the_grid_is_on() {
        let at = Point::new(102.4, 203.6);

        let off = snap_point(at, FREE, &Snapping::none(1.0));
        assert_eq!(
            (off.at.x, off.at.y),
            (102.4, 203.6),
            "every source is off, so nothing may move the handle"
        );

        let on = snap_point(at, FREE, &grid(1.0));
        assert_eq!(
            (on.at.x, on.at.y),
            (102.0, 204.0),
            "and with the grid on it lands on the lattice, both axes"
        );

        // One axis at a time, because the arms are separate `else if`s.
        let x_only = snap_point(at, (true, false), &Snapping::none(1.0));
        assert_eq!((x_only.at.x, x_only.at.y), (102.4, 203.6));
        let y_only = snap_point(at, (false, true), &grid(1.0));
        assert_eq!(
            (y_only.at.x, y_only.at.y),
            (102.4, 204.0),
            "a locked axis is untouched even with the grid on"
        );
    }
}
