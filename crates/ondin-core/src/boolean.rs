//! Boolean path operations — the geometry behind [`crate::NodeKind::Boolean`].
//!
//! kurbo has no path booleans, so the arithmetic comes from `flo_curves`, which
//! works on cubic Bézier paths and keeps curves as curves. Polyline-based libraries
//! (`i_overlay`) are faster but flatten first: a union of two circles would leave a
//! thousand-segment polygon in the save file and the SVG.
//!
//! The arithmetic is pure: paths in, path out, no document. The cache of results is
//! [`crate::Resolved`]'s — a boolean's outline is not a function of its own kind, so
//! it cannot come from [`crate::geometry::local_path`] and is derived per node.
//! [`failures`] and [`poison_next`] are the observable thread-local exceptions.
//!
//! ## The conversion
//!
//! - **flo_curves paths are all-cubic and implicitly closed.** Lines and quads are
//!   raised on the way in; an unclosed subpath is closed, since an open path has no
//!   inside to combine.
//! - **flo_curves works even-odd; Ondin fills non-zero.** A hole comes back wound
//!   the *same* way as its outline, which under non-zero fills solid. [`from_flo`]
//!   forces each subpath's direction to alternate with its nesting depth so the
//!   result means the same under either rule.
//! - **A self-intersecting operand is still read even-odd**: normalizing it with
//!   `path_remove_interior_points` would also fill a legitimate hole.
//!
//! ## The operations
//!
//! Union, Subtract and Intersect are flo_curves' own. **`Exclude` is not
//! `sub(add(a,b), intersect(a,b))`**: the intersection's boundary is made of pieces
//! of the operands' own edges, so that subtraction hands the traversal the same
//! graph as `a ∪ b` and returns the operands. It is `exclude_of`: every outline in
//! one path read even-odd, with no arithmetic. The other three grow about linearly
//! and stay inside a frame's budget far past any realistic selection; nesting splits
//! the work rather than multiplying it.
//!
//! ## The unwind guard
//!
//! flo_curves 0.8.0 panicked on some operand sets (a non-transitive comparator in
//! `GraphPath::exterior_paths`, which every operation calls). 0.8.1 fixed it, but
//! the failing input was a property of the geometry rather than the count, so no
//! sweep can show that nothing else unwinds. [`evaluate`] therefore catches an
//! unwind and answers `None`, a value every caller already handles; a boolean that
//! trips it draws nothing and undo brings the operands back.
//!
//! [`failures`] is what tells that apart from a correct empty result (an
//! intersection of disjoint shapes also answers `None`): the counter is read either
//! side of a call. `Resolved` keeps the answer per node (`boolean_failed`), the
//! layers row shows the warning colour, and a commit that trips it says so in the
//! status bar.
//!
//! The guard depends on unwinding; under `panic = "abort"` it is inert, which is a
//! build failure (see `compile_error!` below).

use crate::node::BoolOp;
use flo_curves::Coord2;
use flo_curves::bezier::path::{BezierPath, SimpleBezierPath, path_add, path_intersect, path_sub};
use kurbo::{BezPath, PathEl, Point};

/// A hundredth of a world unit — the tolerance this module's tests read. No
/// production line reads it, which `cfg(test)` enforces; `FLO_ACCURACY` is what
/// flo_curves is given. Production docs name it in plain backticks, never a link.
///
/// A constant, not a parameter: an accuracy that varied with zoom or shape size
/// would make the geometry a boolean produces depend on the camera, and the result
/// is written into the document.
#[cfg(test)]
const ACCURACY: f64 = 0.01;

/// The factor [`c`] multiplies every coordinate by on the way into flo_curves, and
/// [`k`] divides by on the way back out.
///
/// flo_curves compiles in absolute tolerances (`CLOSE_DISTANCE = 0.01`,
/// `SMALL_DISTANCE = 0.001`) and `GraphPath` merges points closer than the first, so
/// an edge shorter than a hundredth of a unit is erased, not approximated: a thin
/// `Intersect` result pinches into a bow-tie with half the area, or into `None`.
/// Neither lever fixes it alone — a finer accuracy leaves `CLOSE_DISTANCE`, and
/// scaling the geometry with a proportionally scaled tolerance leaves the tolerance
/// the size of the feature. Scaling by 1000 with a fixed accuracy puts the floor at
/// 10⁻⁵ world units. It is a constant factor, so the same two shapes still combine
/// to the same path wherever they sit and whatever the zoom.
const FLO_SCALE: f64 = 1000.0;

/// The tolerance handed to every flo_curves call, in flo_curves' own space — so it
/// is [`FLO_SCALE`] times finer than it reads, 10⁻⁵ of a world unit.
///
/// Not `ACCURACY`: a tolerance the size of a feature deletes it rather than drawing
/// it coarsely, so the rendering argument that justifies `ACCURACY` does not apply.
const FLO_ACCURACY: f64 = 0.01;

/// Combine `operands` with `op`, in the order given (which is z-order, bottom
/// first). `None` when there is nothing to combine or the result is empty.
///
/// `Subtract` is the only operation that reads the order: the bottom shape minus
/// everything above it (child index 0 is the bottom of the stack). The key layer
/// (§9.4) reorders operands only for `Subtract`.
///
/// Symmetric difference is commutative, but only in the mathematics: past a handful
/// of operands the accumulated geometry differs with the fold order, so reordering a
/// large `Exclude` changes its area by percents. Do not reorder one.
///
/// An unwind from flo_curves (see the module docs) is caught here and becomes `None`
/// — which every caller already handles — and bumps [`failures`], the channel that
/// tells it from a correct empty result. Every call into flo_curves goes through the
/// fold below, including [`operand_of`]'s union of a group's contents, so one guard
/// covers them all. A panic inside a group operand is caught by that inner call, so
/// the group drops out of the boolean above it. The guard needs `panic = "unwind"`,
/// which the `compile_error!` above `guarded` enforces.
pub fn evaluate(op: BoolOp, operands: &[BezPath]) -> Option<BezPath> {
    // A magnitude bound, because the guard below cannot see a hang: over operands of
    // ~10³⁰⁴ world units flo_curves spins without panicking, and `evaluate` runs on
    // the UI thread (document open, per-pointer-move re-evaluation while dragging).
    // Nothing on the way in bounds magnitude — `geometry_is_finite` only tests
    // `is_finite()` — so a hand-written or pasted `.ondin` can carry such a value.
    if operands.iter().any(|p| !within_bool_range(p)) {
        return None;
    }
    // Nothing observable crosses the unwind boundary: the arguments are borrowed
    // read-only and flo_curves' state is local to the call.
    guarded(op, operands, fold_operands)
}

/// The largest coordinate magnitude a boolean operand may carry.
///
/// 10¹⁵⁰ because its square is 10³⁰⁰, still finite: areas, determinants and cross
/// products are the natural intermediates of a curve intersection, so a coordinate
/// whose square overflows has already stopped meaning anything. Far above anything
/// that produces a result and far below the hang.
const MAX_BOOL_COORD: f64 = 1e150;

/// Whether every coordinate of `path` is inside [`MAX_BOOL_COORD`].
///
/// Tests the bounding box, not the segments: one pass, and a box inside the bound
/// implies every control point is. A non-finite coordinate fails the comparison, so
/// this also refuses what `geometry_is_finite` should have caught.
fn within_bool_range(path: &BezPath) -> bool {
    let b = <BezPath as kurbo::Shape>::bounding_box(path);
    [b.x0, b.y0, b.x1, b.y1]
        .iter()
        .all(|v| v.is_finite() && v.abs() <= MAX_BOOL_COORD)
}

// The guard below is inert under `panic = "abort"` (`catch_unwind` never returns),
// so the setting is a build failure rather than a note in a manifest.
#[cfg(panic = "abort")]
compile_error!(
    "ondin-core requires panic = \"unwind\": boolean::guarded's catch_unwind is the only thing \
     between a boolean over degenerate geometry and the editor dying with the document open. \
     Under panic = \"abort\" it is inert. See Cargo.toml's profile note and §15 D239."
);

/// The unwind guard and the failure counter, round whichever fold the caller wants.
///
/// One guard for both entry points: [`evaluate`] takes paths and [`evaluate_node`]
/// reads them off a node, and a panic must become `None`, move the counter and be
/// reported once identically for both, or a boolean would be recoverable from one
/// door and fatal from the other.
fn guarded(
    op: BoolOp,
    operands: &[BezPath],
    fold: impl FnOnce(BoolOp, &[BezPath]) -> Option<BezPath>,
) -> Option<BezPath> {
    // Taken before the closure: the `Err` arm also needs to know whether the panic
    // was synthetic.
    let poisoned = POISON.with(|p| {
        let n = p.get();
        p.set(n.saturating_sub(1));
        n > 0
    });
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if poisoned {
            panic!("ondin: synthetic boolean failure, armed by boolean::poison_next");
        }
        fold(op, operands)
    })) {
        Ok(path) => path,
        Err(_) => {
            // Once per process (the preview re-evaluates while dragging) and never for
            // a synthetic panic (tests share a process). The default panic hook still
            // prints the panic; this adds what it means and what was done about it.
            static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !poisoned && !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!(
                    "ondin: the boolean arithmetic panicked and was caught — this \
                     boolean draws nothing. This is NOT the defect the guard was \
                     written for: flo_curves 0.8.1 fixed that one, so an unwind \
                     here is something new and unreported. See docs/archive/ledger.md \
                     §15 D239. Reported once per run."
                );
            }
            FAILURES.with(|n| n.set(n.get() + 1));
            None
        }
    }
}

thread_local! {
    /// See [`poison_next`].
    static POISON: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Make the next `n` calls to [`evaluate`] on this thread unwind inside the guard,
/// as the flo_curves 0.8.0 panic did.
///
/// Test scaffolding: no known operand set panics flo_curves any more, so tests of
/// what happens downstream of an abandoned boolean (the counter, the layers mark,
/// the click target, the writers' placeholder) inject the panic. That proves the
/// guard and its consequences, not that flo_curves can still produce one. `pub`
/// rather than `cfg(test)` because most callers are integration tests in other
/// crates; nothing in the app calls it, and `dead_code` will not say so for a `pub`
/// item, so re-check by grep.
#[doc(hidden)]
pub fn poison_next(n: u32) {
    POISON.with(|p| p.set(n));
}

thread_local! {
    /// See [`failures`].
    static FAILURES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many booleans this thread has abandoned to an unwind in [`evaluate`]. Never
/// decreases.
///
/// The signal `None` cannot carry: it means both "nothing to draw" (disjoint
/// intersection, correct) and "the arithmetic gave up". Reading this either side of
/// a call — a whole [`evaluate_node`], a whole `Resolved::update` — says which.
///
/// A count and not a flag because the recursion nests: a group operand's inner
/// `evaluate` catches its own unwind, so a flag would be cleared by the outer call.
/// Thread-local because the renderer's preview evaluates off the edit path and
/// threads would see each other's failures inside their own brackets.
pub fn failures() -> u64 {
    FAILURES.with(std::cell::Cell::get)
}

/// [`evaluate`] without its guard: the pairwise fold that does the arithmetic.
///
/// Split out only so the guard has something to wrap.
fn fold_operands(op: BoolOp, operands: &[BezPath]) -> Option<BezPath> {
    if op == BoolOp::Exclude {
        return exclude_of(operands);
    }
    let mut live = operands.iter().filter(|p| !p.is_empty());
    let mut acc: Vec<SimpleBezierPath> = to_flo(live.next()?);

    // `Union` and `Intersect` fold in a balanced tree: a left fold hands flo_curves
    // a path of i subpaths at step i, which is quadratic and superlinear within each
    // step, and a mask group of many icons pays it per repaint. Both operations are
    // associative, so only the floating-point path to the same shape (within
    // `FLO_ACCURACY`) depends on the bracketing.
    //
    // `Subtract` is not associative, so it keeps the left fold.
    if op == BoolOp::Subtract {
        for next in live {
            acc = combine(op, &acc, &to_flo(next));
            // Empty stays empty however many operands follow.
            if acc.is_empty() {
                return None;
            }
        }
        return from_flo(&acc);
    }

    let mut level: Vec<Vec<SimpleBezierPath>> =
        std::iter::once(acc).chain(live.map(to_flo)).collect();
    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        let (pairs, rest) = level.as_chunks::<2>();
        for pair in pairs {
            let merged = combine(op, &pair[0], &pair[1]);
            // Only `Intersect` can reach this (a union of non-empty operands is never
            // empty); it stays unconditional so a further associative operator
            // folded here is covered.
            if merged.is_empty() {
                return None;
            }
            next.push(merged);
        }
        // The odd one out rides up a level untouched rather than being folded into
        // its neighbour, which is what keeps the tree balanced at odd counts.
        next.extend(rest.iter().cloned());
        level = next;
    }
    acc = level.pop()?;
    from_flo(&acc)
}

/// The symmetric difference: every operand's outline in one path, to be read
/// **even-odd**.
///
/// This does no arithmetic, by definition: a point is in the symmetric difference
/// when an odd number of closed operands cover it, which is what an even-odd ray
/// crossing counts. The caller owes it [`crate::FillRule::EvenOdd`] on the node,
/// which `build::boolean` and `build::set_boolean_op` write. `path_full_intersect`'s
/// fold, which this replaced, accumulated large error on dense operand sets.
/// `None` for no operands, matching the fold.
fn exclude_of(operands: &[BezPath]) -> Option<BezPath> {
    let mut out = BezPath::new();
    for p in operands.iter().filter(|p| !p.is_empty()) {
        out.extend(p.iter());
    }
    (!out.is_empty()).then_some(out)
}

/// One step of the fold: `acc` combined with one more operand.
fn combine(
    op: BoolOp,
    acc: &Vec<SimpleBezierPath>,
    rhs: &Vec<SimpleBezierPath>,
) -> Vec<SimpleBezierPath> {
    match op {
        BoolOp::Union => path_add(acc, rhs, FLO_ACCURACY),
        BoolOp::Subtract => path_sub(acc, rhs, FLO_ACCURACY),
        BoolOp::Intersect => path_intersect(acc, rhs, FLO_ACCURACY),
        // Unreachable: `fold_operands` answers `Exclude` through `exclude_of`.
        BoolOp::Exclude => unreachable!("fold_operands answers Exclude through exclude_of"),
    }
}

/// How a caller reads the state a boolean is evaluated against.
///
/// Lookups, not a `Document`, because three callers see three different worlds and a
/// second copy of the recursion would drift:
///
/// - [`crate::Resolved`] reads the committed tree and its own cache.
/// - `RenderOverrides` reads a gesture's pending state, so a boolean reshapes while
///   an operand is dragged.
/// - `render::ghost_subtree` reads a captured subtree the document has never seen,
///   which is why the walk goes through `children_of` and `visible_of`.
pub struct Operands<'a> {
    /// The node's local transform.
    pub local_of: &'a dyn Fn(crate::NodeId) -> kurbo::Affine,
    /// The node's kind, including a pending geometry edit: the committed kind would
    /// move operands without resizing them during a boolean resize.
    pub kind_of: &'a dyn Fn(crate::NodeId) -> Option<crate::node::NodeKind>,
    /// The node's children, bottom of the stack first — which is operand order.
    pub children_of: &'a dyn Fn(crate::NodeId) -> Vec<crate::NodeId>,
    /// Whether the node is visible. A hidden operand contributes nothing, so
    /// hiding one is a live edit of the shape.
    pub visible_of: &'a dyn Fn(crate::NodeId) -> bool,
    /// A nested boolean's already-computed outline. A lookup rather than a
    /// recursion, so evaluation has to be **children-first**.
    pub nested: &'a dyn Fn(crate::NodeId) -> Option<BezPath>,
}

/// Evaluate the outline of the `Boolean` node `id` against `view`.
///
/// The same arithmetic [`evaluate`] does, over the operands `view` reports; `id` is
/// the node whose children to read.
pub fn evaluate_node(id: crate::NodeId, op: BoolOp, view: &Operands<'_>) -> Option<BezPath> {
    let operands: Vec<BezPath> = (view.children_of)(id)
        .into_iter()
        .filter_map(|c| operand_of(c, view))
        .collect();
    guarded(op, &operands, fold_operands)
}

/// What one subtree contributes to the boolean above it, in that boolean's space.
///
/// The recursion is where the kinds are decided:
///
/// - A **shape** contributes its outline. The ordinary case.
/// - A **nested boolean** contributes its own result, via `nested`.
/// - A **group** contributes the *union* of what is inside it. Not a
///   concatenation: a set of subpaths is read even-odd, so appending two
///   overlapping members would punch a hole where they cross instead of merging
///   them.
/// - A **frame** contributes its own box, which is the rectangle it looks like.
/// - **Text contributes nothing**, and that is a gap rather than a decision:
///   including it means converting glyphs to outlines, a feature of its own.
///   `build::boolean` refuses a selection containing text, so nothing disappears
///   into a boolean silently.
pub fn operand_of(id: crate::NodeId, view: &Operands<'_>) -> Option<BezPath> {
    use crate::node::NodeKind;
    if !(view.visible_of)(id) {
        return None;
    }
    // The kind as the caller sees it, so a pending resize is part of the answer.
    let kind = (view.kind_of)(id)?;
    let own = match &kind {
        NodeKind::Boolean { .. } => (view.nested)(id),
        NodeKind::Group | NodeKind::Root => {
            let inner: Vec<BezPath> = (view.children_of)(id)
                .into_iter()
                .filter_map(|c| operand_of(c, view))
                .collect();
            evaluate(BoolOp::Union, &inner)
        }
        // `Artboard` included: a frame's box is its outline, from `local_path`.
        kind => crate::geometry::local_path(kind),
    }?;
    Some((view.local_of)(id) * own)
}

/// kurbo → flo_curves: one all-cubic closed subpath per `MoveTo`.
fn to_flo(path: &BezPath) -> Vec<SimpleBezierPath> {
    let mut out: Vec<SimpleBezierPath> = Vec::new();
    let mut start = Point::ZERO;
    let mut cur = Point::ZERO;
    let mut sections: Vec<(Coord2, Coord2, Coord2)> = Vec::new();

    // A subpath ends at the next `MoveTo` or at the end of the path; closing it
    // here rather than at `ClosePath` is what makes an unclosed one usable.
    let flush = |start: Point, cur: Point, sections: &mut Vec<_>, out: &mut Vec<_>| {
        if sections.is_empty() {
            return;
        }
        if (cur - start).hypot() > 1e-9 {
            sections.push(line_to(cur, start));
        }
        out.push((c(start), std::mem::take(sections)));
    };

    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                flush(start, cur, &mut sections, &mut out);
                start = p;
                cur = p;
            }
            PathEl::LineTo(p) => {
                sections.push(line_to(cur, p));
                cur = p;
            }
            PathEl::QuadTo(q, p) => {
                // Raise the quadratic: the cubic through the same points has its
                // controls two thirds of the way to the quad's single one.
                sections.push((
                    c(cur + (q - cur) * (2.0 / 3.0)),
                    c(p + (q - p) * (2.0 / 3.0)),
                    c(p),
                ));
                cur = p;
            }
            PathEl::CurveTo(c1, c2, p) => {
                sections.push((c(c1), c(c2), c(p)));
                cur = p;
            }
            PathEl::ClosePath => {
                flush(start, cur, &mut sections, &mut out);
                cur = start;
            }
        }
    }
    flush(start, cur, &mut sections, &mut out);
    out
}

/// flo_curves → kurbo, with hole windings corrected. Every subpath is closed.
///
/// flo_curves expresses a hole as a separate subpath wound the same way as its
/// outline (it reads even-odd); under Ondin's non-zero fill that fills solid. Each
/// subpath's nesting depth is counted and its direction forced to alternate with it:
/// outermost counter-clockwise, a hole clockwise, an island counter-clockwise again.
/// The result then means the same under either fill rule, which also makes it safe
/// for SVG.
fn from_flo(paths: &[SimpleBezierPath]) -> Option<BezPath> {
    let subpaths: Vec<BezPath> = paths
        .iter()
        .filter_map(|subpath| {
            let mut out = BezPath::new();
            let mut points = subpath.points();
            let first = points.next()?;
            out.move_to(k(subpath.start_point()));
            out.curve_to(k(first.0), k(first.1), k(first.2));
            for (c1, c2, p) in points {
                out.curve_to(k(c1), k(c2), k(p));
            }
            out.close_path();
            Some(out)
        })
        .collect();

    let mut out = BezPath::new();
    for (i, sub) in subpaths.iter().enumerate() {
        // How many of the others enclose this one. `contains` is a winding test, so
        // it answers regardless of which way either subpath happens to run.
        // A subpath whose interior cannot be located is kept, not dropped: losing the
        // winding correction fills a hole solid, losing the geometry deletes shape.
        let Some(inside) = representative_point(sub) else {
            out.extend(sub.iter());
            continue;
        };
        // Bigger-than, not merely contains-a-point-of: a ring's bounding-box centre
        // sits inside its hole, so containment alone would nest every subpath in
        // every other. Only a strictly larger subpath can enclose this one, since
        // boolean output never has crossing subpaths.
        let own_area = <BezPath as kurbo::Shape>::area(sub).abs();
        let depth = subpaths
            .iter()
            .enumerate()
            .filter(|(j, other)| {
                *j != i
                    && <BezPath as kurbo::Shape>::area(other).abs() > own_area
                    && <BezPath as kurbo::Shape>::contains(other, inside)
            })
            .count();
        // Even depth is solid and odd is a hole; `Shape::area` is signed, so its
        // sign is the direction, and a mismatch is reversed.
        let want_positive = depth % 2 == 0;
        let sub = if (<BezPath as kurbo::Shape>::area(sub) > 0.0) == want_positive {
            sub.clone()
        } else {
            sub.reverse_subpaths()
        };
        out.extend(sub.iter());
    }
    (!out.is_empty()).then_some(out)
}

/// A point strictly inside `subpath`, for the containment tests above.
///
/// The bounding-box centre is inside for a convex subpath and often not for a
/// concave one. The fallback is a scan line: intersect a horizontal ray with every
/// segment, sort the crossings and take the midpoint of the first span that is
/// inside. That is exact for a closed curve of any thickness, whereas a fixed grid
/// over the bounding box misses any subpath thinner than its spacing — and
/// [`from_flo`] would then drop the subpath, silently deleting shape.
fn representative_point(subpath: &BezPath) -> Option<Point> {
    use kurbo::ParamCurve;
    let bounds = <BezPath as kurbo::Shape>::bounding_box(subpath);
    if <BezPath as kurbo::Shape>::contains(subpath, bounds.center()) {
        return Some(bounds.center());
    }
    // Several heights: a single scan line can miss on a tangency or an hourglass
    // pinched exactly there.
    const SCANS: i32 = 15;
    for iy in 1..=SCANS {
        let y = bounds.y0 + bounds.height() * f64::from(iy) / f64::from(SCANS + 1);
        // Overhanging the box on both sides so a crossing at the extreme x is not
        // a parameter-domain edge case.
        let line = kurbo::Line::new((bounds.x0 - 1.0, y), (bounds.x1 + 1.0, y));
        let mut xs: Vec<f64> = subpath
            .segments()
            .flat_map(|seg| {
                seg.intersect_line(line)
                    .into_iter()
                    .map(move |hit| line.eval(hit.line_t).x)
            })
            .collect();
        xs.sort_by(f64::total_cmp);
        // `contains` on each span's midpoint rather than crossing-count parity: a
        // tangency gives two roots at one x and a shared endpoint one from each of
        // two segments.
        for w in xs.windows(2) {
            let mid = Point::new(f64::midpoint(w[0], w[1]), y);
            if w[1] > w[0] && <BezPath as kurbo::Shape>::contains(subpath, mid) {
                return Some(mid);
            }
        }
    }
    None
}

/// A straight section as a cubic: controls at the thirds, so the curve *is* the
/// line and flo_curves has nothing special to handle.
fn line_to(from: Point, to: Point) -> (Coord2, Coord2, Coord2) {
    let d = to - from;
    (c(from + d * (1.0 / 3.0)), c(from + d * (2.0 / 3.0)), c(to))
}

/// Into flo_curves' space, scaled by [`FLO_SCALE`]. With [`k`] it is the whole seam.
fn c(p: Point) -> Coord2 {
    Coord2(p.x * FLO_SCALE, p.y * FLO_SCALE)
}

/// Back out of flo_curves' space, undoing [`c`]; [`from_flo`]'s nesting pass runs in
/// world units.
fn k(p: Coord2) -> Point {
    Point::new(p.0 / FLO_SCALE, p.1 / FLO_SCALE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::FillRule;
    use kurbo::{Rect, Shape};

    /// `n` disjoint 20×20 squares on a 32-wide grid at 40-unit pitch — a group of
    /// icons used as a mask. Disjoint on purpose: an overlapping set collapses to a
    /// few subpaths and hides the accumulator growth the tree fold avoids.
    fn disjoint(n: usize) -> Vec<BezPath> {
        (0..n)
            .map(|i| {
                let x = (i % 32) as f64 * 40.0;
                let y = (i / 32) as f64 * 40.0;
                Rect::new(x, y, x + 20.0, y + 20.0).to_path(0.1)
            })
            .collect()
    }

    /// Folding a union as a balanced tree gives the same shape as folding it left.
    ///
    /// Reassociating is a pure optimisation, so no shape assertion can bite the
    /// change; this pins its premise, that `combine` is associative for `Union` to
    /// within `ACCURACY`, against a left fold computed here rather than a literal.
    /// The tolerance is deliberate: two bracketings of a floating-point boolean may
    /// differ.
    #[test]
    fn a_union_folded_as_a_tree_is_the_shape_the_left_fold_gives() {
        for n in [3usize, 4, 16, 64] {
            let ops = disjoint(n);
            let mut it = ops.iter();
            let mut acc = to_flo(it.next().unwrap());
            for next in it {
                acc = combine(BoolOp::Union, &acc, &to_flo(next));
            }
            let left = from_flo(&acc).expect("the left fold produces a shape");
            let tree = evaluate(BoolOp::Union, &ops).expect("and so does the tree");

            assert_eq!(
                left.elements().len(),
                tree.elements().len(),
                "n={n}: the two bracketings produced different geometry"
            );
            let (a, b) = (
                <BezPath as Shape>::area(&left).abs(),
                <BezPath as Shape>::area(&tree).abs(),
            );
            assert!((a - b).abs() < ACCURACY, "n={n}: left fold {a}, tree {b}");
            // Neither fold dropped an operand: n squares of 400 each, none overlapping.
            assert!(
                (b - n as f64 * 400.0).abs() < ACCURACY,
                "n={n}: the union is {b} where {n} disjoint squares are {}",
                n as f64 * 400.0
            );
        }
    }

    /// `Subtract` keeps its left fold: `((a − b) − c) − d`, not the tree's
    /// `(a − b) − (c − d)`, which restores what `d` takes out of `c`.
    ///
    /// It needs four operands: with three the tree carries the third up untouched
    /// and agrees with the left fold by accident. `d` overlaps `c` alone, so the
    /// bracketings differ by a 200-unit square.
    ///
    /// flip: delete the `Subtract` arm of `fold_operands` → fails at the area
    /// assertion (1000 against 800); no other test in the workspace bites.
    #[test]
    fn a_subtraction_is_still_bracketed_from_the_left() {
        let a = Rect::new(0.0, 0.0, 40.0, 40.0).to_path(0.1);
        let b = Rect::new(0.0, 0.0, 10.0, 40.0).to_path(0.1);
        let c = Rect::new(10.0, 0.0, 20.0, 40.0).to_path(0.1);
        let d = Rect::new(10.0, 0.0, 20.0, 20.0).to_path(0.1);
        let out = evaluate(BoolOp::Subtract, &[a, b, c, d]).expect("a shape is left");
        let area = <BezPath as Shape>::area(&out).abs();
        // 1600 − 400 (b) − 400 (c) = 800; `d` is inside `c`. As a tree, only the top
        // half of `c` is taken out: 1000.
        assert!(
            (area - 800.0).abs() < ACCURACY,
            "a − b − c − d is {area}, not 800 — the fold was reassociated"
        );
    }

    /// Two 100x100 squares overlapping over a 50x100 band.
    fn pair() -> (BezPath, BezPath) {
        (
            Rect::new(0.0, 0.0, 100.0, 100.0).to_path(0.1),
            Rect::new(50.0, 0.0, 150.0, 100.0).to_path(0.1),
        )
    }

    /// The area of `path` read even-odd, by sampling a grid over its box.
    ///
    /// `area` cannot measure an even-odd path: the shoelace sum is a winding
    /// integral, so same-wound overlapping squares sum to the union. Sampled because
    /// there is no cheap exact form; the error scales with boundary length times the
    /// cell, so callers assert a relative tolerance. For an exact claim test a point
    /// with `FillRule::contains`.
    fn even_odd_area(path: &BezPath, steps: usize) -> f64 {
        let b = path.bounding_box();
        if b.width() <= 0.0 || b.height() <= 0.0 {
            return 0.0;
        }
        let (cw, ch) = (b.width() / steps as f64, b.height() / steps as f64);
        let mut inside = 0u64;
        for iy in 0..steps {
            let y = b.y0 + (iy as f64 + 0.5) * ch;
            for ix in 0..steps {
                let x = b.x0 + (ix as f64 + 0.5) * cw;
                if path.winding(Point::new(x, y)) % 2 != 0 {
                    inside += 1;
                }
            }
        }
        inside as f64 * cw * ch
    }

    /// Signed area by the shoelace formula over a flattened outline. Winding, so wrong
    /// for an even-odd path — see `even_odd_area`.
    fn area(path: &BezPath) -> f64 {
        let mut sum = 0.0;
        let mut start = Point::ZERO;
        let mut cur = Point::ZERO;
        kurbo::flatten(path, 0.01, |el| match el {
            PathEl::MoveTo(p) => {
                start = p;
                cur = p;
            }
            PathEl::LineTo(p) => {
                sum += cur.x * p.y - p.x * cur.y;
                cur = p;
            }
            PathEl::ClosePath => {
                sum += cur.x * start.y - start.x * cur.y;
                cur = start;
            }
            _ => unreachable!("flattened"),
        });
        sum / 2.0 // SIGNED: a hole subtracts, which is the point of the probe
    }

    /// The four operations on two overlapping squares, by area. 100x100 each,
    /// overlapping by 50x100 — so the answers are 15000, 5000, 5000 and 10000.
    #[test]
    fn the_four_operations_on_two_squares() {
        let (a, b) = pair();
        let ops = [
            (BoolOp::Union, 15_000.0),
            (BoolOp::Subtract, 5_000.0),
            (BoolOp::Intersect, 5_000.0),
            (BoolOp::Exclude, 10_000.0),
        ];
        for (op, expected) in ops {
            let result = evaluate(op, &[a.clone(), b.clone()]).expect("a result");
            // `Exclude` is measured even-odd (the shoelace sum would read the union),
            // with a tolerance of a cell of boundary; a wrong answer is 50% out.
            let (got, tol) = match op {
                BoolOp::Exclude => (even_odd_area(&result, 300), expected * 0.01),
                _ => (area(&result), 1.0),
            };
            assert!(
                (got - expected).abs() < tol,
                "{op:?}: expected area {expected}, got {got}"
            );
        }
        // The exact half, by point: the overlap is outside an exclusion and inside a
        // union, as `FillRule` answers for the renderer and the hit test.
        let xor = evaluate(BoolOp::Exclude, &[a.clone(), b.clone()]).expect("a result");
        let overlap = Point::new(75.0, 50.0);
        let a_only = Point::new(25.0, 50.0);
        assert!(
            !FillRule::EvenOdd.contains(&xor, overlap),
            "the overlap is the one place an exclusion draws nothing"
        );
        assert!(
            FillRule::EvenOdd.contains(&xor, a_only),
            "and what only one operand covers is inside it"
        );
        assert!(
            FillRule::NonZero.contains(&xor, overlap),
            "read non-zero the very same path is a union — which is what the rule \
             is for, and what a node that lost it would draw"
        );
    }

    /// **Subtract takes the bottom shape and removes what is above it**, so the
    /// operand order is the layer order and swapping it is a different shape.
    #[test]
    fn subtract_is_not_commutative_and_takes_the_first_operand_as_the_base() {
        let a = Rect::new(0.0, 0.0, 100.0, 100.0).to_path(0.1);
        let b = Rect::new(50.0, 0.0, 250.0, 100.0).to_path(0.1);
        let ab = evaluate(BoolOp::Subtract, &[a.clone(), b.clone()]).expect("a result");
        let ba = evaluate(BoolOp::Subtract, &[b, a]).expect("a result");
        assert!(
            (area(&ab) - 5_000.0).abs() < 1.0,
            "100x100 less its right half"
        );
        assert!(
            (area(&ba) - 15_000.0).abs() < 1.0,
            "200x100 less its left 50: {}",
            area(&ba)
        );
    }

    /// `Exclude` gives one answer for every operand permutation, which is why the key
    /// layer (§9.4) reorders operands only for `Subtract`.
    ///
    /// Three operands, because two would pass on pairwise symmetry alone. It compares
    /// areas within 1.0, so it certifies only a small `Exclude`; the large-fold claim
    /// is `exclude_agrees_with_odd_coverage_at_every_sample_point`'s, against a
    /// referee.
    #[test]
    fn exclude_gives_the_same_answer_in_any_operand_order() {
        let a = Rect::new(0.0, 0.0, 100.0, 100.0).to_path(0.1);
        let b = Rect::new(50.0, 0.0, 150.0, 100.0).to_path(0.1);
        let c = Rect::new(80.0, 0.0, 200.0, 100.0).to_path(0.1);
        let forward = area(&evaluate(BoolOp::Exclude, &[a.clone(), b.clone(), c.clone()]).unwrap());
        let reversed =
            area(&evaluate(BoolOp::Exclude, &[c.clone(), b.clone(), a.clone()]).unwrap());
        let middled = area(&evaluate(BoolOp::Exclude, &[b, a, c]).unwrap());
        assert!(
            (forward - reversed).abs() < 1.0 && (forward - middled).abs() < 1.0,
            "one answer per permutation, got {forward}, {reversed}, {middled}"
        );
    }

    /// Curves survive: a boolean of two circles is still made of curves, not a
    /// flattened polyline — the reason for the library choice.
    #[test]
    fn a_boolean_of_two_circles_comes_back_as_curves() {
        let a = kurbo::Circle::new((0.0, 0.0), 50.0).to_path(0.01);
        let b = kurbo::Circle::new((40.0, 0.0), 50.0).to_path(0.01);
        let result = evaluate(BoolOp::Intersect, &[a, b]).expect("the lens shape");
        let curves = result
            .elements()
            .iter()
            .filter(|el| matches!(el, PathEl::CurveTo(..)))
            .count();
        assert!(curves > 0, "the result is made of curves");
        assert!(
            curves < 40,
            "…and of a handful of them, not a flattened polyline: {curves}"
        );
        // The closed-form lens area: 2r²·acos(d/2r) − (d/2)·√(4r²−d²).
        let (r, d) = (50.0_f64, 40.0_f64);
        let exact = 2.0 * r * r * (d / (2.0 * r)).acos() - (d / 2.0) * (4.0 * r * r - d * d).sqrt();
        let got = area(&result);
        assert!(
            (got - exact).abs() / exact < 5e-4,
            "lens area {got}, exact {exact}"
        );
    }

    /// Nothing to combine is `None`, not an empty path, and one operand is itself.
    #[test]
    fn a_single_operand_and_no_operands() {
        assert!(evaluate(BoolOp::Union, &[]).is_none());
        let (a, _) = pair();
        let alone = evaluate(BoolOp::Union, std::slice::from_ref(&a)).expect("itself");
        assert!((area(&alone) - 10_000.0).abs() < 1.0);
    }

    /// `Exclude` equals union less intersection at every offset, including the
    /// aligned one: `sub(add(a,b), intersect(a,b))` re-assembles the operands, which
    /// looks exactly like a union.
    #[test]
    fn exclude_is_the_symmetric_difference_at_every_offset() {
        let rect = Rect::new(0.0, 0.0, 200.0, 120.0).to_path(0.01);
        for dy in [0.0_f64, 0.5, 1.0, -1.0, 59.0] {
            let circle = kurbo::Circle::new((160.0, 120.0 + dy), 60.0).to_path(0.01);
            let pair = [rect.clone(), circle];
            let union = area(&evaluate(BoolOp::Union, &pair).expect("a union"));
            let both = evaluate(BoolOp::Intersect, &pair)
                .map(|p| area(&p))
                .unwrap_or(0.0);
            // Sampled (even-odd): 0.3% of the union is the grid's accuracy, well inside
            // the failure `xor == union`, which is a fifth of the union out at least.
            let xor = even_odd_area(
                &evaluate(BoolOp::Exclude, &pair).expect("a difference"),
                300,
            );
            assert!(both > 1.0, "dy={dy}: the shapes really do overlap");
            assert!(
                (xor - (union - both)).abs() < union * 0.003,
                "dy={dy}: exclude {xor:.1} should be union {union:.1} less overlap {both:.1}"
            );
        }
    }

    /// A subtraction that leaves a genuine hole comes back wound against its outline —
    /// the case `from_flo`'s winding normalization exists for.
    #[test]
    fn a_hole_is_wound_against_the_outline_that_contains_it() {
        let rect = Rect::new(0.0, 0.0, 200.0, 200.0).to_path(0.01);
        let hole = kurbo::Circle::new((100.0, 100.0), 40.0).to_path(0.01);
        let result = evaluate(BoolOp::Subtract, &[rect, hole]).expect("a rect with a hole");

        let mut signed = Vec::new();
        let mut sub = BezPath::new();
        for el in result.iter() {
            if matches!(el, PathEl::MoveTo(_)) && !sub.is_empty() {
                signed.push(kurbo::Shape::area(&sub));
                sub = BezPath::new();
            }
            sub.push(el);
        }
        if !sub.is_empty() {
            signed.push(kurbo::Shape::area(&sub));
        }

        assert_eq!(signed.len(), 2, "an outline and a hole: {signed:?}");
        assert!(
            signed[0] * signed[1] < 0.0,
            "opposite windings, or a non-zero fill paints the hole solid: {signed:?}"
        );
        let net: f64 = signed.iter().sum::<f64>().abs();
        let expected = 200.0 * 200.0 - std::f64::consts::PI * 40.0 * 40.0;
        assert!(
            (net - expected).abs() < 5.0,
            "net area {net:.1} against {expected:.1}"
        );
    }

    /// Disjoint shapes: a union is both of them (two subpaths), an intersection is
    /// nothing at all.
    #[test]
    fn disjoint_operands() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.1);
        let b = Rect::new(100.0, 100.0, 110.0, 110.0).to_path(0.1);
        let union = evaluate(BoolOp::Union, &[a.clone(), b.clone()]).expect("both");
        assert!((area(&union) - 200.0).abs() < 1.0, "{}", area(&union));
        assert_eq!(
            union
                .elements()
                .iter()
                .filter(|el| matches!(el, PathEl::MoveTo(_)))
                .count(),
            2,
            "two subpaths"
        );
        assert!(evaluate(BoolOp::Intersect, &[a, b]).is_none());
    }

    /// `n` circles of radius 90 spaced evenly round a circle of radius 120, so every
    /// operand overlaps several of its neighbours.
    fn ring(n: usize) -> Vec<BezPath> {
        (0..n)
            .map(|i| {
                let a = i as f64 / n as f64 * std::f64::consts::TAU;
                kurbo::Circle::new(
                    Point::new(200.0 + 120.0 * a.cos(), 200.0 + 120.0 * a.sin()),
                    90.0,
                )
                .to_path(0.1)
            })
            .collect()
    }

    /// A boolean that panics comes back as `None` rather than taking the process
    /// with it, and a healthy one still gets past the guard.
    ///
    /// The panic is injected with `poison_next`, so this says nothing about whether
    /// flo_curves can still produce one. The default panic hook runs before the
    /// unwind is caught and a library must not replace it; it is silenced here only
    /// to keep the output readable, and restored so a later failure is not hidden.
    #[test]
    fn a_boolean_that_panics_comes_back_empty_rather_than_taking_the_process() {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        poison_next(1);
        let caught = evaluate(BoolOp::Exclude, &ring(40));
        std::panic::set_hook(hook);
        assert_eq!(caught, None, "a caught unwind has no path to answer with");

        // The guard is not swallowing ordinary answers: the same ring, unpoisoned,
        // unions to a real shape.
        let union = evaluate(BoolOp::Union, &ring(40));
        assert!(
            union.is_some_and(|p| !p.is_empty()),
            "the guard ate a boolean that never panicked"
        );
    }

    /// `failures` separates the two `None`s: an empty intersection leaves the counter
    /// alone and an abandoned boolean moves it. A version that increments on every
    /// `None` fails this and `resolve`'s
    /// `a_boolean_the_arithmetic_gave_up_on_is_marked_and_an_empty_one_is_not`.
    /// Read as a difference, since tests share a process.
    #[test]
    fn the_counter_moves_for_an_abandoned_boolean_and_not_for_an_empty_one() {
        let apart = vec![
            kurbo::Circle::new(Point::new(0.0, 0.0), 10.0).to_path(0.1),
            kurbo::Circle::new(Point::new(500.0, 0.0), 10.0).to_path(0.1),
        ];
        let before = failures();
        assert_eq!(
            evaluate(BoolOp::Intersect, &apart),
            None,
            "circles 500 apart do not intersect — the fixture is the point"
        );
        assert_eq!(
            failures(),
            before,
            "an empty result is an answer, not a failure"
        );

        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        poison_next(1);
        let caught = evaluate(BoolOp::Exclude, &ring(40));
        std::panic::set_hook(hook);
        assert_eq!(caught, None, "the same `None` as above, by construction");
        assert_eq!(
            failures(),
            before + 1,
            "and the one that was abandoned has to be countable"
        );
    }

    /// The poison is one call, not a mode, so it cannot leak into a later test on the
    /// same thread. An off-by-one in the decrementing counter would otherwise show
    /// only as an unrelated downstream failure.
    #[test]
    fn the_poison_is_spent_by_the_call_it_arms() {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        poison_next(1);
        let before = failures();
        assert_eq!(evaluate(BoolOp::Union, &ring(3)), None, "armed");
        std::panic::set_hook(hook);
        assert_eq!(failures(), before + 1, "armed, and counted");

        let after = evaluate(BoolOp::Union, &ring(3));
        assert!(
            after.is_some_and(|p| !p.is_empty()),
            "the next call must be clean — a poison that outlives its call would \
             mark booleans in whichever test ran next"
        );
        assert_eq!(failures(), before + 1, "and must not count a second time");
    }

    /// An `Operands` view over a ring of ellipses, so `evaluate_node` can be
    /// driven without a `Document`.
    ///
    /// `moved` shifts one operand, as a drag does. `NodeId` has no raw constructor,
    /// so the ids are minted.
    fn ring_view(n: usize, moved: Option<(usize, f64)>) -> Vec<(crate::NodeId, kurbo::Affine)> {
        let mut ids = crate::IdSource::new(0xB0CA);
        (0..n)
            .map(|i| {
                let a = i as f64 / n as f64 * std::f64::consts::TAU;
                let (mut x, y) = (
                    200.0 + 120.0 * a.cos() - 90.0,
                    200.0 + 120.0 * a.sin() - 90.0,
                );
                if let Some((which, dx)) = moved
                    && which == i
                {
                    x += dx;
                }
                (ids.mint(), kurbo::Affine::translate((x, y)))
            })
            .collect()
    }

    fn ring_kind() -> crate::node::NodeKind {
        crate::node::NodeKind::Ellipse {
            size: kurbo::Size::new(180.0, 180.0),
        }
    }

    /// The true symmetric-difference area of the `n`-circle ring, for `n` in 10..=20.
    ///
    /// Recorded rather than computed: `odd_coverage_area` takes too long for every
    /// `cargo test`. `the_recorded_truth_matches_a_fresh_sampling` regenerates them.
    const RING_TRUTH: [(usize, f64); 11] = [
        (10, 64182.9),
        (11, 76259.2),
        (12, 81424.4),
        (13, 77888.4),
        (14, 69810.3),
        (15, 59325.5),
        (16, 58068.1),
        (17, 62391.8),
        (18, 70248.9),
        (19, 77696.5),
        (20, 76745.1),
    ];

    /// The recorded truth is what a fresh sampling says; `#[ignore]`d for its cost.
    ///
    /// `cargo test --release -p ondin-core --lib the_recorded_truth -- --ignored`
    #[test]
    #[ignore = "23s in debug; the instrument behind RING_TRUTH, run it on demand"]
    fn the_recorded_truth_matches_a_fresh_sampling() {
        for (n, recorded) in RING_TRUTH {
            let fresh = odd_coverage_area(n);
            assert!(
                (fresh - recorded).abs() < 1.0,
                "n={n}: recorded {recorded}, sampled {fresh:.1}"
            );
        }
    }

    /// The true area of the symmetric difference of `n` ring circles, by sampling.
    ///
    /// XOR is exactly "covered by an odd number of operands", so this needs no path
    /// arithmetic and is independent of everything under test, which lets it referee.
    fn odd_coverage_area(n: usize) -> f64 {
        let centres: Vec<(f64, f64)> = (0..n)
            .map(|i| {
                let a = i as f64 / n as f64 * std::f64::consts::TAU;
                (200.0 + 120.0 * a.cos(), 200.0 + 120.0 * a.sin())
            })
            .collect();
        let (lo, hi, steps) = (-10.0f64, 410.0f64, 4000usize);
        let cell = (hi - lo) / steps as f64;
        let mut odd = 0u64;
        for iy in 0..steps {
            let y = lo + (iy as f64 + 0.5) * cell;
            for ix in 0..steps {
                let x = lo + (ix as f64 + 0.5) * cell;
                let covered = centres
                    .iter()
                    .filter(|(cx, cy)| {
                        let (dx, dy) = (x - cx, y - cy);
                        dx * dx + dy * dy < 90.0 * 90.0
                    })
                    .count();
                if covered % 2 == 1 {
                    odd += 1;
                }
            }
        }
        odd as f64 * cell * cell
    }

    /// `Exclude` agrees with an independent odd-coverage referee (`odd_coverage_area`'s
    /// counting) at every sampled point of the ring at each recorded count. The
    /// referee knows only circles and counting; the old pairwise fold disagreed with
    /// it by up to 18% of area.
    ///
    /// Point by point rather than by area: equal areas do not make equal shapes, and a
    /// few hundred winding queries fit a debug build. The lattice steps by an
    /// irrational fraction so no sample lands on a symmetry axis, where both answers
    /// are undecidable.
    #[test]
    fn exclude_agrees_with_odd_coverage_at_every_sample_point() {
        for (n, _) in RING_TRUTH {
            let centres: Vec<(f64, f64)> = (0..n)
                .map(|i| {
                    let a = i as f64 / n as f64 * std::f64::consts::TAU;
                    (200.0 + 120.0 * a.cos(), 200.0 + 120.0 * a.sin())
                })
                .collect();
            let ops: Vec<BezPath> = ring_view(n, None)
                .iter()
                .map(|(_, t)| *t * crate::geometry::local_path(&ring_kind()).unwrap())
                .collect();
            let path = evaluate(BoolOp::Exclude, &ops).expect("a shape");

            let (mut x, mut y) = (0.31622776601_f64, 0.61803398875_f64);
            let mut checked = 0;
            let mut inside = 0;
            for _ in 0..400 {
                x = (x + 0.61803398875).fract();
                y = (y + 0.31622776601).fract();
                let p = Point::new(-10.0 + x * 420.0, -10.0 + y * 420.0);
                let covered = centres
                    .iter()
                    .filter(|(cx, cy)| {
                        let (dx, dy) = (p.x - cx, p.y - cy);
                        dx * dx + dy * dy < 90.0 * 90.0
                    })
                    .count();
                // Skip a point within a hair of any circle's edge.
                if centres.iter().any(|(cx, cy)| {
                    let (dx, dy) = (p.x - cx, p.y - cy);
                    ((dx * dx + dy * dy).sqrt() - 90.0).abs() < 0.05
                }) {
                    continue;
                }
                checked += 1;
                let want = covered % 2 == 1;
                if want {
                    inside += 1;
                }
                assert_eq!(
                    FillRule::EvenOdd.contains(&path, p),
                    want,
                    "n={n} at {p:?}: covered by {covered} operands, so the symmetric \
                     difference {} it — the fold used to be out by up to 17.7% of area here",
                    if want { "contains" } else { "excludes" }
                );
            }
            // Both answers must occur, or a shape containing everything or nothing
            // would satisfy the assertion above.
            assert!(
                inside > 20 && checked - inside > 20,
                "n={n}: {inside} inside of {checked} — the sample must straddle the edge"
            );
        }
    }

    /// A thin concave result is a shape, not a rounding error: a crescent (a circle
    /// minus itself nudged sideways) and a bracket (a square minus a thin L-shaped
    /// notch) both survive `representative_point` and `from_flo`'s winding pass.
    ///
    /// The similarity sweep is why a wider fixed grid is not a fix: a grid relative
    /// to the bounding box fails scale-invariantly. The convex control (a sliver
    /// rect) never reached the fallback. The `failures()` assertion records that none
    /// of this panics, so a loss here is silent.
    ///
    /// flip: restore a fixed grid in `representative_point` and drop `from_flo`'s
    /// keep-on-failure branch → fails at the crescent's `expect`. The scan line alone
    /// reverted fails at the notch's area (163 900 against 156 100). Reverting only
    /// the keep-on-failure branch stays green; it is a belt for a scan line that
    /// finds nothing.
    #[test]
    fn a_thin_concave_result_survives_the_winding_pass() {
        use kurbo::Shape;

        /// The crescent left by subtracting a circle from itself, offset sideways.
        fn crescent(r: f64, offset: f64) -> Option<BezPath> {
            let a = kurbo::Circle::new((0.0, 0.0), r).to_path(0.001);
            let b = kurbo::Circle::new((offset, 0.0), r).to_path(0.001);
            evaluate(BoolOp::Subtract, &[a, b])
        }

        let before = failures();

        // A crescent 200 units across and about 5 thick, under 1/40 of its box.
        let moon = crescent(200.0, 5.0).expect("a crescent is a shape, not nothing");
        let area = moon.area().abs();
        assert!(
            (area - 2000.0).abs() < 60.0,
            "the crescent's area is about 2000, got {area}"
        );

        // A 400×400 square with a 10-unit L-shaped notch inside it: the winding half,
        // where an uncorrected hole fills solid under non-zero.
        let square = kurbo::Rect::new(0.0, 0.0, 400.0, 400.0).to_path(0.001);
        let mut notch = BezPath::new();
        notch.move_to((100.0, 100.0));
        notch.line_to((300.0, 100.0));
        notch.line_to((300.0, 110.0));
        notch.line_to((110.0, 110.0));
        notch.line_to((110.0, 300.0));
        notch.line_to((100.0, 300.0));
        notch.close_path();
        let bracket = evaluate(BoolOp::Subtract, &[square, notch])
            .expect("a square minus a notch is a shape");
        let net = bracket.area().abs();
        assert!(
            (net - 156_100.0).abs() < 200.0,
            "the notch must be a hole: net area {net}, the solid square is 160000"
        );

        // The similarity sweep: the same shape at five sizes.
        for r in [10.0f64, 20.0, 40.0, 80.0, 200.0] {
            let m = crescent(r, r / 40.0).unwrap_or_else(|| panic!("r={r}: drew nothing"));
            let a = m.area().abs();
            let want = 2000.0 * (r / 200.0) * (r / 200.0);
            assert!(
                (a - want).abs() < want * 0.1,
                "r={r}: area {a}, expected about {want} by similarity"
            );
        }

        // The convex control: a 400 × 0.1 sliver rect, whose bounding-box centre is
        // inside it so the fallback never runs.
        let wide = kurbo::Rect::new(0.0, 0.0, 400.0, 200.0).to_path(0.001);
        let slot = kurbo::Rect::new(0.0, 99.95, 400.0, 100.05).to_path(0.001);
        let sliver = evaluate(BoolOp::Intersect, &[wide, slot]).expect("a sliver is a shape");
        assert!(
            (sliver.area().abs() - 40.0).abs() < 0.2,
            "the convex control must stay correct: {}",
            sliver.area().abs()
        );

        assert_eq!(
            failures(),
            before,
            "no operand here panics, so `failures` must not move — a wrong shape \
             from this path is silent by construction"
        );
    }

    /// A boolean over enormous operands returns instead of hanging: a spin is not an
    /// unwind, so the guard cannot catch it.
    ///
    /// No wall-clock assertion, since a timing assertion measures the machine and a
    /// spin never returns; the harness timeout is the instrument. flip: disable the
    /// bound in `evaluate` → the test does not return. The `Some` controls (an
    /// ordinary boolean, and `1e100`) show the bound sits above the working range
    /// rather than through it.
    #[test]
    fn an_enormous_operand_is_refused_rather_than_spun_on() {
        let pair = |m: f64| {
            (
                Rect::new(m, m, 2.0 * m, 2.0 * m).to_path(0.01),
                Rect::new(1.5 * m, 1.5 * m, 2.5 * m, 2.5 * m).to_path(0.01),
            )
        };

        let (a, b) = pair(1e305);
        assert_eq!(
            evaluate(BoolOp::Union, &[a.clone(), b.clone()]),
            None,
            "an operand past the bound is refused — and the assertion that \
             matters is that this line is reached at all"
        );
        assert_eq!(evaluate(BoolOp::Intersect, &[a, b]), None);

        let (a, b) = pair(1e100);
        assert!(
            evaluate(BoolOp::Union, &[a, b]).is_some(),
            "the largest magnitude measured to produce a result still produces \
             one, so the bound sits above the working range and not through it"
        );
        let (a, b) = pair(1.0);
        assert!(
            evaluate(BoolOp::Union, &[a, b]).is_some(),
            "control: an ordinary boolean is untouched"
        );
    }

    /// A thin `Intersect` keeps its area down to 2 × 10⁻⁵ world units. Without
    /// [`FLO_SCALE`], flo_curves' `CLOSE_DISTANCE` collapses the two short ends into
    /// points, a bow-tie of exactly half the area, or `None` below `SMALL_DISTANCE`.
    ///
    /// flip: `FLO_SCALE` to 1.0, or `FLO_ACCURACY` to `ACCURACY * FLO_SCALE` → fails
    /// at `t = 0.01`, the first thickness of the coarse-first sweep; `FLO_SCALE =
    /// 200.0` fails only at `t = 0.00002`.
    #[test]
    fn a_thin_intersect_keeps_its_area() {
        use kurbo::Shape;

        let before = failures();
        let wide = kurbo::Rect::new(0.0, 0.0, 400.0, 200.0).to_path(0.001);

        // 0.02 and above were never affected; 0.01 and below halve the area and 0.001
        // erases the shape. `0.00002` pins `FLO_SCALE` to roughly 500 or more.
        for t in [1.0f64, 0.1, 0.02, 0.01, 0.005, 0.001, 0.0001, 0.00002] {
            let slot =
                kurbo::Rect::new(0.0, 100.0 - t / 2.0, 400.0, 100.0 + t / 2.0).to_path(0.001);
            let got = evaluate(BoolOp::Intersect, &[wide.clone(), slot])
                .unwrap_or_else(|| panic!("t={t}: a sliver is a shape, not nothing"));
            let want = 400.0 * t;
            let area = got.area().abs();
            assert!(
                (area - want).abs() < want * 0.01,
                "t={t}: area {area}, expected {want} — half of that is the bow-tie"
            );
        }

        // The shape is a quadrilateral, which area cannot show: the bow-tie had four
        // elements to the rectangle's six.
        let slot = kurbo::Rect::new(0.0, 99.995, 400.0, 100.005).to_path(0.001);
        let got = evaluate(BoolOp::Intersect, &[wide, slot]).expect("a sliver is a shape");
        assert_eq!(
            got.elements().len(),
            6,
            "four sides, a move and a close: {:?}",
            got.elements()
        );

        assert_eq!(
            failures(),
            before,
            "nothing here panics, so this class of loss is silent and `failures` \
             cannot be what reports it"
        );
    }
}
