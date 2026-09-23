//! Boolean path operations — the geometry behind [`crate::NodeKind::Boolean`].
//!
//! **kurbo has no path booleans** (its tracking issue is still open), so the
//! arithmetic comes from `flo_curves`, which works on cubic Bézier paths and
//! therefore keeps curves as curves. The alternative family of libraries
//! (`i_overlay` and friends) is far faster and flattens everything to polylines
//! first — a union of two circles would leave a thousand-segment polygon in the
//! save file, in the SVG, and in front of anyone who edited it afterwards.
//!
//! The arithmetic here is pure: paths in, path out, no document. The cache of
//! *results* is [`crate::Resolved`]'s, for the same reason text layouts are cached
//! there — a boolean's outline is not a function of its own kind, so it cannot
//! come from [`crate::geometry::local_path`] and has to be *derived* per node.
//!
//! ⚠️ **It kept a fold cache and a spatial cull for part of 2026-08-31 and keeps
//! neither**, which is worth a sentence so the next reader does not rebuild them by
//! accident. Both existed to make `Exclude`'s fold cheaper; the fill rule (§15 D239)
//! then deleted that fold outright — an `Exclude` is a concatenation now — and what
//! was left was two mechanisms serving three operations that had never needed them
//! and a *third* implementation of the fold to keep in step with this one. They were
//! removed the same day and the table below is measured without them. The purity
//! claim is still qualified, though: [`failures`] and [`poison_next`] are
//! thread-locals, and unlike the cache was, they are *observable*.
//!
//! ## Three things about the conversion
//!
//! - **flo_curves paths are all-cubic and implicitly closed.** Lines and quads are
//!   raised on the way in; an unclosed subpath is closed. A boolean of an open path
//!   is not a meaningful question — there is no inside — so closing it is the only
//!   answer that produces a shape rather than nothing.
//! - **flo_curves works even-odd; Ondin fills non-zero.** A hole comes back as a
//!   second subpath wound the *same* way as the outline around it, which under
//!   non-zero is not a hole at all — it fills solid. [`from_flo`] therefore forces
//!   each subpath's direction to alternate with its nesting depth, so the result
//!   means the same thing under either rule. Without it, `Exclude` looked identical
//!   to `Union` and `Subtract` only worked when the bite reached an edge.
//! - **A self-intersecting operand is still read even-odd**, and we deliberately do
//!   not normalize it with `path_remove_interior_points`, because that would also
//!   fill a legitimate hole in the operand.
//!
//! ## And one about the operations
//!
//! Union, Subtract and Intersect are flo_curves' own. **`Exclude` is not
//! `sub(add(a,b), intersect(a,b))`** however much it looks like it should be: the
//! intersection's boundary is made entirely of pieces of the operands' own edges, so
//! that subtraction hands the traversal the same graph as `a ∪ b` and gets the two
//! operands back. It **was** `path_full_intersect`'s two exterior paths — `a \ b`
//! and `b \ a` from one cut, disjoint by construction, concatenated — and that fold
//! was replaced for accumulating error on dense operand sets: it is `exclude_of`
//! now, every outline in one path read even-odd, with no arithmetic at all (§15
//! D239). This paragraph read in the present tense until §15 D856, above a
//! `combine` arm nothing could reach.
//!
//! ## What it costs, measured
//!
//! Measured 2026-08-19, re-measured 2026-08-31 on flo_curves 0.8.1, and measured
//! **again the same day after the fill rule and the removal of the cull and the
//! cache**, which is the state this table describes. A ring of overlapping circles
//! (`tests::ring`), release profile, best of three, milliseconds for one whole
//! [`evaluate`]:
//!
//! | operands | Union | Subtract | Intersect | Exclude | Exclude, when it folded |
//! | --- | --- | --- | --- | --- | --- |
//! | 2 | 0.01 | 0.01 | 0.00 | 0.00 | 0.01 |
//! | 5 | 0.07 | 0.03 | 0.01 | 0.00 | 0.24 |
//! | 10 | 0.21 | 0.08 | 0.03 | 0.00 | 2.00 |
//! | 20 | 0.58 | 0.19 | 0.06 | 0.00 | 18.81 |
//! | 40 | 1.63 | 0.39 | 0.14 | 0.00 | 197.58 |
//! | 64 | 3.32 | 0.61 | 0.24 | 0.00 | 1087.24 |
//!
//! **The flo_curves 0.8.1 upgrade cost nothing** — every cell reproduced within
//! noise, so the comparator fix was not a trade. **`Exclude` is a concatenation and
//! rounds to zero at every count**, which is the whole of what the fill rule bought:
//! the operation that was three orders of magnitude dearer than the others is now
//! the cheapest of the four.
//!
//! ⚠️ **Removing the spatial cull moved the other three by about a tenth of a
//! millisecond, in *both* directions**, three runs, and the direction is the part
//! worth knowing: `Subtract` at sixty-four went 0.48 → 0.61 and `Union` went
//! 3.47 → **3.32**, i.e. the cull was paying for its own bookkeeping on the operation
//! it helped least. Neither number is material — the worst of them is 4% of a 60 Hz
//! frame — and *that* is the argument for the removal rather than any saving.
//!
//! **All four are a non-problem now.** They grow about linearly and are inside a
//! frame's budget at sixty-four operands, which is well past what anyone selects. The
//! frame-budget warning this section carried for `Exclude` — it left the budget at
//! nineteen operands folding, twenty-nine with the cull — is spent, and the preview
//! path that re-evaluates a boolean on every frame of a drag no longer has a case
//! that hurts it.
//!
//! **A boolean of booleans costs nothing extra**, which was the other open
//! question: four unions of five, then a union of those four results, comes to
//! 0.58 ms against 0.60 ms for the same twenty flat. Nesting splits the work, it
//! does not multiply it, so the shape of a tree is not what to look at.
//!
//! ## An upstream panic that *was* live, and the guard that outlived it
//!
//! ⚠️ **Fixed upstream in flo_curves 0.8.1, taken 2026-08-31.** Everything below
//! describes 0.8.0 and is kept because the guard, the counter and four downstream
//! features are all still here and make no sense without it (§15 D239).
//!
//! Some operand sets made flo_curves **panic**: *"user-provided comparison
//! function does not correctly implement a total order"*, out of
//! `GraphPath::exterior_paths` (`graph_path/mod.rs:1051` in 0.8.0). It reproduced
//! in both profiles.
//!
//! **The cause was exact and it was not a NaN.** That sort's comparator treated two
//! points as equal in x when they were within 0.01 and compared their y instead,
//! and otherwise compared x — which is not transitive: three points 0.008 apart
//! give `a ≈ b`, `b ≈ c` and `a < c`. Rust's sort has detected inconsistent
//! comparators since 1.81 and panics rather than sorting to nonsense, so **the
//! fault was older than the panic** and what changed is that it became loud.
//!
//! **Every operation goes through that function** — `add.rs`, `sub.rs`,
//! `intersect.rs` and `full_intersect.rs` all call `exterior_paths` — so nothing
//! here was exempt, and swapping `Exclude`'s implementation would only have made it
//! rarer. What made `Exclude` the one that showed is volume: `path_full_intersect`
//! calls `exterior_paths` three times per operation where the others call it once,
//! and its accumulator is an order of magnitude larger, so it had far more points
//! to cluster. Sweeping the same ring from 2 to 64 operands, `Union` never panicked
//! and `Exclude` panicked at 40, 42, 44, 47, 49, 59, 60 and 63. **The gaps were the
//! tell**: the real variable is the geometry, not the count, so no count that
//! passed ruled anything out.
//!
//! **What 0.8.1 changed** is exactly that comparator: `exterior_paths` now sorts on
//! `f64::total_cmp` of x alone — a total order by construction — and *then* walks
//! the sorted array re-sorting runs of close-x points by y. The epsilon still
//! exists; it segments the array instead of living inside the comparison. Re-run
//! on 0.8.1, all eight of the counts above return a real path, and so do NaN,
//! infinite and 1e300 operands, which is unsurprising once the sort is `total_cmp`.
//! ⚠️ **That survey's claim is *"nothing unwinds"*, and its 1e300 case no longer
//! describes this module** (§15 D843): [`MAX_BOOL_COORD`] is 1e150, so such
//! operands are refused by [`evaluate`] and never reach flo_curves at all.
//!
//! **What is done about it: [`evaluate`] catches the unwind and answers `None`.**
//! Of the three options — a patched fork of flo_curves, catching here, or living
//! with it — this is the one that did not put us upstream of a dependency, and
//! `None` is a value every caller already handles. A boolean that trips it draws
//! nothing, and undo brings the operands back.
//!
//! ⚠️ **The guard stays, and its meaning has changed.** It is no longer a
//! workaround for a known defect but a general refusal to let an upstream panic
//! take the document with it — so an unwind reaching it now is *unknown*, which is
//! what its stderr message says. D239's own finding is the argument for keeping it:
//! the variable was the geometry and not the count, so "no set panics" was never
//! something a sweep could establish, only "none of the ones we tried". What the
//! fix did cost is the *reproduction* — see [`poison_next`].
//!
//! **And what makes it a wrong shape rather than a silent one is [`failures`]**,
//! because the same property that made `None` safe — every caller already handles
//! it — is what made the failure invisible: an intersection that came out empty
//! answers `None` too, and drawing nothing is *correct* for that one. The counter
//! is read either side of a call to say which happened. `Resolved` keeps the
//! answer per node (`boolean_failed`), the layers row wears the same warning
//! colour a picture that cannot be drawn wears, and a commit that trips it says so
//! in the status bar (§15 D298).
//!
//! **It depends on unwinding.** Under `panic = "abort"` the guard is inert and the
//! process goes down as before; nothing in the code can detect that, so the
//! workspace's profiles carry a note where somebody would set it.

use crate::node::BoolOp;
use flo_curves::Coord2;
use flo_curves::bezier::path::{BezierPath, SimpleBezierPath, path_add, path_intersect, path_sub};
use kurbo::{BezPath, PathEl, Point};

/// A hundredth of a world unit — the tolerance this module's tests read, and
/// until 2026-09-19 the one handed to every boolean.
///
/// 🚨 **It is no longer what flo_curves is given**; `FLO_ACCURACY` is, and it is
/// three orders finer (§15 D794). The paragraph below is still the reason the
/// figure is a *constant* rather than a parameter, and that reason is untouched —
/// but read it as being about which quantities may vary, not about this number
/// reaching the boolean, because it no longer does.
///
/// ⚠️ **And this doc broke that rule two paragraphs above the line stating it**
/// (§15 D841). It named `FLO_ACCURACY` as an intra-doc link, which is exactly
/// what §15 D319's convention forbids *here* — a link in a test's prose is
/// decoration, because `cargo doc` builds without the `test` cfg and this item
/// is absent from the crate rustdoc walks. Plain backticks, then; the correct
/// population of such links is **zero**, an invariant rather than a figure.
///
/// 🚨 **The check is what is new, not the rule.** This is the second instance of
/// the hole §15 D827's own *Fix* named — a `#[cfg(test)]` **item** at module
/// level, rather than one inside a `mod tests`, sits outside every hand-rolled
/// sweep that looks for the module while being absent from rustdoc all the
/// same. D827 wrote that down and the next instance was already in the tree,
/// three lines from here. *A rule stated in a doc comment does not bind the doc
/// comment stating it.*
///
/// ⚠️ **`cfg(test)` rather than `allow(dead_code)`**, so that "no production line
/// reads this" is enforced by the compiler instead of asserted by the sentence
/// above it — which is the half of D794 most likely to rot. The cost is that no
/// production doc may *link* it (§15 D319, D699): two constants below explain
/// themselves by contrast with this one and name it in plain backticks, which the
/// doc gate cannot resolve and therefore cannot check.
///
/// **A constant, not a parameter.** An accuracy that varied with zoom, or with
/// the size of the shape, would make the *geometry a boolean produces* depend on
/// the camera: the same two shapes combined at two zoom levels would give two
/// different paths, written into the document and kept. The reason is about the
/// model, not the export.
///
/// ⚠️ It used to be given as "export is required to be byte-identical across runs
/// and machines (§7.2)", which is wrong twice: §7.2 is the PNG bullet and makes
/// no such promise, and byte-identity *across machines* is not true today — the
/// CPU renderer takes whatever SIMD level it finds and a stroked path moves under
/// AVX2 (§15 D304). The constant is right; the argument for it was borrowed from
/// a guarantee nothing makes. 0.01 is
/// flo_curves' own documented working value and is a hundredth of a world unit —
/// finer than any pixel the canvas shows below 100× zoom.
#[cfg(test)]
const ACCURACY: f64 = 0.01;

/// The factor [`c`] multiplies every coordinate by on the way into flo_curves, and
/// [`k`] divides by on the way back out (§15 D794).
///
/// 🚨 **flo_curves has absolute tolerances of its own that no argument reaches.**
/// `consts.rs` compiles in `CLOSE_DISTANCE = 0.01` and `SMALL_DISTANCE = 0.001`,
/// and `GraphPath` merges two points closer than `CLOSE_DISTANCE` into one. So an
/// edge shorter than a hundredth of a world unit is not approximated, it is
/// *erased* — and erasing the two short ends of a thin `Intersect` result pinches
/// a rectangle into a bow-tie with **exactly half** the area. Measured on a
/// 400-wide slot against a 400×200 rect: correct at a thickness of 0.02, half at
/// 0.01 and at 0.005, and `None` — nothing drawn at all — at 0.001.
///
/// 🚨 **Neither lever fixes this alone, and that is the whole shape of the bug.**
/// There are two absolute tolerances the size of the lost feature, not one, and
/// the four-cell matrix was measured rather than reasoned:
///
/// | scale | what flo_curves is handed | in world units | thin `Intersect` |
/// | --- | --- | --- | --- |
/// | 1 | 0.01 | 0.01 | **half** — the defect as found |
/// | 1 | 0.0001 | 0.0001 | **half**, bit-identical to the row above |
/// | 1000 | 10.0 | 0.01 | **half** |
/// | 1000 | 0.01 | 0.00001 | exact, to 10⁻⁴ thickness |
///
/// The second row is the control that clears `ACCURACY`: taking it a hundred
/// times finer changed not one digit, because `CLOSE_DISTANCE` was doing the
/// erasing. The third row is the control that clears scaling on its own: scale the
/// geometry and scale the tolerance with it and nothing moves, because the
/// tolerance is once again the size of the feature. **Only the fourth works**, and
/// the roadmap's guess that `ACCURACY` was "the first place to look" was half
/// right in a way that would have stalled there.
///
/// At 1000 the effective floor is 10⁻⁵ world units, five orders below anything the
/// model means to represent. It is a **constant** factor, not one derived from the
/// shapes or the camera, so it keeps the invariant `ACCURACY`'s own doc is about:
/// the same two shapes still combine to the same path wherever they sit and
/// whatever the zoom.
///
/// ⚠️ **It costs nothing measurable**, which was not obvious — a tolerance three
/// orders finer could have meant three orders more subdivision on the hot path
/// §15 D614 is about. Timed in release on that entry's own fixture, N disjoint
/// 20×20 rects folded as a `Union`: **0.37 / 1.00 / 2.61 / 7.33 ms** at N = 16,
/// 64, 128, 256 against a baseline of **0.40 / 1.14 / 2.51 / 7.17**. Inside the
/// run-to-run spread at every size, and the subpath count is identical.
const FLO_SCALE: f64 = 1000.0;

/// The tolerance handed to every flo_curves call, **in flo_curves' own space** —
/// so it is [`FLO_SCALE`] times finer than it reads, 10⁻⁵ of a world unit.
///
/// 🚨 **This is not `ACCURACY`** (plain backticks: that constant is `cfg(test)`
/// and a production doc cannot link it), **and it is the half of §15 D794 that
/// contradicts
/// a written rule.** `ACCURACY`'s doc says a hundredth of a world unit is "finer
/// than any pixel the canvas shows below 100× zoom", which is true and is an
/// argument about *rendering*. The failure above is not about rendering: a
/// tolerance the size of the feature does not draw it coarsely, it deletes it.
/// `ACCURACY` stays at 0.01 as the tolerance this module's own tests read for
/// fold-associativity, where a hundredth of a unit is the right claim; what
/// reaches flo_curves is this.
const FLO_ACCURACY: f64 = 0.01;

/// Combine `operands` with `op`, in the order given (which is z-order, bottom
/// first). `None` when there is nothing to combine or the result is empty.
///
/// **`Subtract` is the only one of the four that reads the order**, and the order
/// is the layer order: the bottom shape minus everything above it, which is what
/// "subtract selection" means everywhere and what the layer list already shows —
/// child index 0 is the bottom of the stack.
///
/// This said "Subtract and Exclude are not commutative" and Exclude does not
/// belong in that sentence: symmetric difference is commutative *and* associative,
/// so the fold below gives one answer for every permutation
/// (`exclude_gives_the_same_answer_in_any_operand_order` measures it on three
/// operands, since two would pass on the pairwise symmetry alone). It matters
/// beyond pedantry — the key layer (§9.4) reorders operands only for `Subtract`,
/// because a designation that rewrote the tree for an operation that cannot see
/// the difference would promise something the arithmetic does not deliver.
///
/// ⚠️ **"One answer for every permutation" is true of the mathematics and false of
/// this implementation past a handful of operands** (measured 2026-08-31, §15 D239).
/// Moving one operand of a **twenty**-circle `Exclude` to the end of the fold changes
/// the result by about 3% of its area — 2,482 units of 85,850, at an unchanged
/// element count — because the accumulated geometry differs, and the error grows with
/// the count: 0.009 at five operands, 1,654 at twelve. `Union` and `Subtract` move by
/// float noise. **The paragraph above is still right about what it was written to
/// decide** — the key layer may reorder for `Subtract` and not for `Exclude`, and a
/// three-operand `Exclude` really is permutation-independent — but *it must not be
/// read as a licence to reorder a large one*, which is exactly what two callers did:
/// `RenderOverrides::operand_children`, which appended ghost operands and previewed a
/// shape 3% off the commit until it was fixed, and the fast boolean cache that was
/// designed on this sentence and rejected on the measurement.
///
/// **Guarded against the upstream panic in the module docs** (§15 D239). flo_curves
/// sorts with a comparator that is not transitive, and Rust's sort refuses one, so
/// some operand sets abort the arithmetic mid-way — unguarded, on the UI thread,
/// that is the editor going down with the document open in it. An unwind caught here becomes
/// `None`, which is a value every caller already has to handle: it is what an
/// intersection that came out empty answers, and what a boolean whose operands
/// contribute nothing answers. A boolean that trips it draws nothing and undo
/// brings the operands back.
///
/// **That is also why the caught unwind bumps [`failures`].** Answering with a
/// value every caller already handles is what stopped this being a lost document;
/// it is also what makes the failure indistinguishable from a correct empty
/// result, so the *user* was told nothing. The counter is the channel that
/// distinction travels on, and it is read either side of a call rather than
/// returned, so no signature changes and no caller is obliged to care.
///
/// **This is the seam, so the guard belongs here and not further out.** Every call
/// into flo_curves in this crate goes through the fold below — including the one
/// [`operand_of`] makes to union a group's contents — so one guard covers them all,
/// and it costs nothing on the path where nothing panics.
///
/// Two consequences worth stating rather than discovering. It **depends on
/// unwinding**: under `panic = "abort"` the process would go down as before —
/// which is why the setting is now a **build failure** rather than a hope, by the
/// `#[cfg(panic = "abort")] compile_error!` above `guarded` (§15 D445). ⚠️ **This
/// sentence read *"and nothing here can detect that"* until 2026-09-06**, twenty
/// lines from the thing that detects it; it was the third copy of the claim, with
/// `Cargo.toml`'s profile note, and the reason nobody wrote the two-line gate.
/// And a panic *inside* a group operand is caught by
/// that inner call, so the group drops out of the boolean above it rather than
/// failing the whole thing — the same silent contribution-of-nothing that a text
/// operand already makes, and a wrong-looking shape rather than none.
pub fn evaluate(op: BoolOp, operands: &[BezPath]) -> Option<BezPath> {
    // 🚨 **A magnitude bound, because the guard below cannot see a *hang***
    // (§15 D843). §15 D239's `catch_unwind` answers `None` for the input class
    // this module knows it cannot survive — but a spin is not an unwind, and
    // over operands of ~10³⁰⁴ world units `flo_curves` does not come back at
    // all: measured past **590 s** in debug and past **20 s** in release, with
    // no panic, no `failures()` bump and no return. `evaluate` runs on the UI
    // thread — `Resolved::update` calls it when the document opens, and
    // `RenderOverrides` re-evaluates per pointer move while an operand is
    // dragged — so the outcome is the editor frozen with the document open,
    // which is exactly what D239 exists to prevent and is the one shape it is
    // structurally blind to.
    //
    // ⚠️ **`FLO_SCALE` moved the threshold down by its own factor and did not
    // create the band.** At scale 1.0 the hang begins near 10³⁰⁸; at the
    // shipped 1000 it begins between 10³⁰⁴ and 5·10³⁰⁴ — three decades, exactly
    // the multiplier. So this closes the introduced band **and** the
    // pre-existing one above it, which is why the bound is here rather than in
    // `FLO_SCALE`'s neighbourhood.
    //
    // ⚠️ **Nothing on the way in bounds magnitude**: `NodeKind::geometry_is_finite`
    // tests `is_finite()` and nothing else, so such a value passes the loader,
    // `op_insert_subtree`'s per-node check and the clipboard door alike. A
    // hand-written, foreign or pasted `.ondin` can carry it.
    if operands.iter().any(|p| !within_bool_range(p)) {
        return None;
    }
    // `AssertUnwindSafe` because nothing observable crosses the boundary: the
    // arguments are borrowed read-only, and every scrap of state flo_curves builds
    // is local to the call and dropped by the unwind. There is no shared mutable
    // state here for a half-finished operation to leave torn.
    guarded(op, operands, fold_operands)
}

/// The largest coordinate magnitude a boolean operand may carry (§15 D843).
///
/// **10¹⁵⁰ because its square is 10³⁰⁰, which is still finite.** Areas,
/// determinants and cross products are the natural intermediates of a curve
/// intersection, so a coordinate whose square overflows is one whose arithmetic
/// has already stopped meaning anything — that is the quantity this bounds,
/// rather than a number picked to sit below the observed hang.
///
/// ⚠️ **It is far above anything that currently produces a result and far below
/// the hang.** Measured over `Rect::new(m, m, 2m, 2m)` against
/// `Rect::new(1.5m, 1.5m, 2.5m, 2.5m)`: the last magnitude to answer `Some` is
/// **10¹⁰⁰**, `10²⁰⁰` and `10³⁰⁴` already answer `None`, and the hang begins
/// between `10³⁰⁴` and `5·10³⁰⁴`. So **on that fixture** this refuses only
/// geometry that was answering `None` anyway, and what it changes is how long
/// `None` takes to arrive.
///
/// 🚨 **Do not widen that into *"it changes no output anyone has measured"***,
/// which is what this doc said until §15 D843 was written and is false: §15
/// D239's 2026-08-31 survey fed *"NaN, infinite and **1e300** coordinates
/// across three operations"*, and 1e300 is past this bound — those operands are
/// refused at the door now and never reach flo_curves. That survey was a
/// one-off probe rather than a committed test, so nothing broke; its precise
/// claim, *"found nothing that unwinds"*, is untouched. **A measurement over
/// one fixture is not a statement about every fixture**, and the scope is the
/// whole difference between the two sentences.
const MAX_BOOL_COORD: f64 = 1e150;

/// Whether every coordinate of `path` is inside [`MAX_BOOL_COORD`].
///
/// The **bounding box**, not the segments: it is one pass, it is what
/// `bounding_box` already computes for other callers, and a box inside the
/// bound implies every control point is — a Bézier's hull contains its
/// controls. ⚠️ A non-finite coordinate makes the box non-finite and fails the
/// comparison, so this also refuses what `geometry_is_finite` is supposed to
/// have caught, without relying on it.
fn within_bool_range(path: &BezPath) -> bool {
    let b = <BezPath as kurbo::Shape>::bounding_box(path);
    [b.x0, b.y0, b.x1, b.y1]
        .iter()
        .all(|v| v.is_finite() && v.abs() <= MAX_BOOL_COORD)
}

// **The guard below is inert under `panic = "abort"`, so the setting is a build
// failure rather than a note in a manifest** (§15 D445). `catch_unwind` never returns when a
// panic aborts: the editor would go down with the document open in it, on the one
// class of input (§15 D239) this whole module knows it cannot survive. `Cargo.toml`
// carries the reasoning and points here.
//
// ⚠️ **That manifest note argued this gate was impossible** — *"nothing in the code
// can detect the setting or warn about it"* — until 2026-09-06. `#[cfg(panic =
// "abort")]` is a stable rustc cfg; two lines, measured with a probe before being
// written. A record that says a check cannot be built is worse than no record: it
// stops the next reader trying.
#[cfg(panic = "abort")]
compile_error!(
    "ondin-core requires panic = \"unwind\": boolean::guarded's catch_unwind is the only thing \
     between a boolean over degenerate geometry and the editor dying with the document open. \
     Under panic = \"abort\" it is inert. See Cargo.toml's profile note and §15 D239."
);

/// The unwind guard and the failure counter, round whichever fold the caller wants.
///
/// **One guard for both entry points.** [`evaluate`] takes its operands as paths and
/// [`evaluate_node`] reads them off a node, and the whole of what the guard
/// promises — a panic becomes `None`, the counter moves, the message is said once —
/// has to be identical for the two or a boolean would be recoverable from one door
/// and fatal from the other. Written twice, that is exactly the difference that
/// would drift. The two took *different folds* while the prefix cache existed, which
/// is when this mattered most; they take the same one again now.
fn guarded(
    op: BoolOp,
    operands: &[BezPath],
    fold: impl FnOnce(BoolOp, &[BezPath]) -> Option<BezPath>,
) -> Option<BezPath> {
    // Taken *before* the closure, because both arms below need to know: the
    // closure needs it to decide whether to panic, and the `Err` arm needs it to
    // decide whether the panic was ours. Reading it inside the closure would put
    // the answer somewhere the `Err` arm cannot reach.
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
            // **Once per process, and never for a synthetic one.** The default
            // panic hook prints on every unwind and a library must not replace it —
            // that would swallow the message of every *real* panic in the app. What
            // this adds is the one thing that message cannot carry, which is what it
            // means and what was done about it. The preview path re-evaluates a
            // boolean while an operand is being dragged, so an unguarded line here
            // would be sixty a second. A poisoned call is skipped because the test
            // suite shares a process and this would fire on whichever test ran
            // first, in a run where nothing had gone wrong.
            static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !poisoned && !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!(
                    "ondin: the boolean arithmetic panicked and was caught — this \
                     boolean draws nothing. This is NOT the defect the guard was \
                     written for: flo_curves 0.8.1 fixed that one, so an unwind \
                     here is something new and unreported. See docs/decisions.md \
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

/// Make the next `n` calls to [`evaluate`] on this thread unwind inside the
/// guard, exactly as an upstream panic did before flo_curves 0.8.1 fixed the one
/// this code was written against (§15 D239).
///
/// **Test scaffolding, and it exists because the real fixture stopped existing.**
/// Six tests across three crates asserted on what happens *downstream* of an
/// abandoned boolean — the counter, the layers mark, the click target, and the
/// placeholder in both writers — and every one of them reached that state by
/// evaluating a 40-circle ring that used to trip the intransitive comparator. On
/// 0.8.1 that ring returns a real path, so the six had no way in. Rather than
/// hunt a new operand set that panics — which would be proving a negative, and
/// a sweep of NaN, infinite and 1e300 coordinates across three operations found
/// nothing that unwinds — the panic is injected here.
///
/// ⚠️ **What this costs is the property the old fixture had for free**: the
/// reproduction used to be *real*, which is why §15 D239 kept it rather than
/// deleting it once the guard went in. A synthetic panic proves the guard catches
/// a panic and every consequence downstream of it; it cannot prove flo_curves
/// still has a way to produce one. **Those are different claims and the tests
/// below now make the second one only.** If a genuine panicking set is ever found
/// again, it belongs back in
/// `a_boolean_that_panics_comes_back_empty_rather_than_taking_the_process` in
/// place of this.
///
/// `pub` and not `#[cfg(test)]` because most of its callers are integration tests
/// in other crates, which cannot see a `cfg(test)` item. Nothing in the app calls
/// it — and `dead_code` will never say so for a `pub` item in this crate, so that
/// is a fact to re-check by grep rather than one a gate defends. ⚠️ **The
/// how-many is deliberately not written here**: it was, as *"four of the six"*,
/// and it was one low within a release — the shape of the caller set is the
/// argument, and the number is not.
#[doc(hidden)]
pub fn poison_next(n: u32) {
    POISON.with(|p| p.set(n));
}

thread_local! {
    /// See [`failures`].
    static FAILURES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many booleans this thread has abandoned to the defect in [`evaluate`]'s
/// module doc. Never decreases.
///
/// **The signal `None` cannot carry.** `None` means *nothing to draw*, and an
/// intersection of two shapes that do not overlap means exactly that and is a
/// correct answer — so a caller cannot tell a shape that came out empty from one
/// the arithmetic gave up on, and drawing nothing is right for the first and a
/// silent wrong result for the second. Reading this **either side of a call** says
/// which happened: `evaluate` is the one seam every path into flo_curves goes
/// through, so a bracket round any caller of it — a whole [`evaluate_node`], a
/// whole `Resolved::update` — answers *did this trip the defect*.
///
/// **A count and not a flag, because the recursion nests.** A group operand is
/// unioned by an inner `evaluate` that catches its own unwind, so the outer call
/// never sees a panic and would clear a flag the inner one had just set. A number
/// that only goes up composes: every bracket asks whether it moved across *its*
/// own span, and none of them can tread on another's answer.
///
/// Thread-local rather than global for the same reason: the renderer's preview
/// evaluates booleans off the edit path, and two threads sharing a counter would
/// each see the other's failures inside their own brackets.
pub fn failures() -> u64 {
    FAILURES.with(std::cell::Cell::get)
}

/// [`evaluate`] without its guard: the pairwise fold that does the arithmetic.
///
/// Split out only so the guard has something to wrap — the two are one function
/// and the doc above covers both.
fn fold_operands(op: BoolOp, operands: &[BezPath]) -> Option<BezPath> {
    if op == BoolOp::Exclude {
        return exclude_of(operands);
    }
    let mut live = operands.iter().filter(|p| !p.is_empty());
    let mut acc: Vec<SimpleBezierPath> = to_flo(live.next()?);

    // 🚨 **`Union` and `Intersect` are folded in a *balanced tree*, and that is the
    // whole of `[S10.1-L4-04]`** (§15 D614). A left fold grows the accumulator by
    // one operand per step, so step *i* hands flo_curves a path of *i* subpaths and
    // the total work is Θ(N²) — worse in practice, because `path_add` is itself
    // superlinear. Measured in release on N disjoint 20×20 rects, which is a group
    // of icons used as a shape mask and is both the common case and the expensive
    // one: **0.56 ms at 16, 26.7 at 128, 103 at 256, 471 at 512.** At 128 that is
    // more than a whole 16.7 ms frame, and it is paid **per repaint** through
    // `scene::mask_geometry` and **per pointer move** through `resolve::mask_path`,
    // neither of which caches.
    //
    // Merging adjacent pairs and then pairs of pairs makes it log N levels of N
    // subpaths each. Same answer: union and intersection are associative, so the
    // *shape* cannot depend on the bracketing — only the floating-point path of
    // getting there, to within `FLO_ACCURACY`, which every operand has already been
    // rounded to on the way in. (It was `ACCURACY` until §15 D794 moved the
    // tolerance flo_curves is handed; the associativity argument is unchanged and
    // the bound it names is now three orders tighter.)
    //
    // ⚠️ **`Subtract` is not associative and keeps the left fold.** `a − b − c` is
    // `(a − b) − c`, and `a − (b − c)` is a different shape; bracketing it freely
    // would silently change what the Subtract tool draws. Nothing in the measured
    // problem is a subtraction — a mask group unions — so the arm that is slow is
    // exactly the arm that can be reassociated.
    //
    // ⚠️ **The measurement that chose this over the finding's own fix sketch.** It
    // proposed hoisting `Shape::area` out of `from_flo`'s O(subpaths²) nesting
    // loop, on the reasoning that the two terms were comparable. They are not:
    // timed separately at N = 256, the fold is **96.2 ms** and `from_flo` is
    // **2.8 ms**, so that fix removes 3% of a 103 ms cost. *The attribution was
    // reasoned in the finding and the finding says so.*
    if op == BoolOp::Subtract {
        for next in live {
            acc = combine(op, &acc, &to_flo(next));
            // A subtraction that came out empty stays empty however many more
            // operands follow, and flo_curves is the expensive part of this loop.
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
        let mut pairs = level.chunks_exact(2);
        for pair in &mut pairs {
            let merged = combine(op, &pair[0], &pair[1]);
            // An intersection that came out empty stays empty however many more
            // operands it would have met, at this level or any above it.
            //
            // ⚠️ **The guard is written for both reassociated ops and only one of
            // them can reach it**, which `arch-scribe` caught by reading this
            // against the comment above it: a `Union` of non-empty operands cannot
            // come out empty, and the empties were filtered on the way in. Left
            // unconditional rather than narrowed to `Intersect` — a guard that is
            // sound for the arm it cannot fire on is one fewer thing to get right
            // if a fourth associative operator is ever folded here — but the
            // comment now says which arm it is *about*.
            if merged.is_empty() {
                return None;
            }
            next.push(merged);
        }
        // The odd one out rides up a level untouched rather than being folded into
        // its neighbour, which is what keeps the tree balanced at odd counts.
        next.extend(pairs.remainder().iter().cloned());
        level = next;
    }
    acc = level.pop()?;
    from_flo(&acc)
}

/// The symmetric difference: every operand's outline in one path, to be read
/// **even-odd** (§15 D239).
///
/// ⚠️ **This does no arithmetic, and that is not a shortcut — it is the definition.**
/// A point is in the symmetric difference when an odd number of operands cover it,
/// and a point is inside an even-odd path when a ray from it crosses the path an odd
/// number of times. For closed operands those are the same sentence, so concatenating
/// the outlines *is* the answer and there is nothing left to compute. The caller owes
/// it one thing: the node must carry [`crate::FillRule::EvenOdd`], which
/// `build::boolean` and `build::set_boolean_op` write.
///
/// **It replaced a fold that was measurably wrong**, which is the reason to prefer it
/// over speed alone. `path_full_intersect` accumulated error on dense operand sets:
/// against an odd-coverage referee, the fold was out by up to 12,374 area units of
/// 69,810 — 18% — on a fourteen-circle ring, while this is accurate at every count
/// tried to within the referee's own sampling noise. It is also the whole of the
/// cost: `Exclude` was 1087 ms at sixty-four operands and is now a memcpy.
///
/// **`None` for no operands at all**, matching the fold: an empty set has no shape,
/// which is different from a shape that came out empty. Unlike the fold there is no
/// early exit to make — nothing can cancel to nothing here, because cancellation is
/// what the *reading* does rather than what this builds.
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
        // 🚨 **Unreachable, and it used to read as live** (§15 D856,
        // `[X8-L6-03]`). `fold_operands` answers `Exclude` through `exclude_of`
        // before it can reach this function, and the only other caller is a
        // test that passes `Union`. The arm this replaced — `path_full_intersect`'s
        // two exterior paths, under twenty lines arguing XOR in the present tense
        // — was the remnant of the fold `exclude_of` replaced for being
        // measurably wrong; a `panic!` in it left every test green. It was also
        // the fourth of the four `FLO_ACCURACY` sites §15 D794 counts as retuned,
        // and so the one a flip of that tolerance could land on and read as *"no
        // teeth"* — §15 D803's decoy. The module doc keeps the argument for why
        // XOR is the two exteriors.
        BoolOp::Exclude => unreachable!("fold_operands answers Exclude through exclude_of"),
    }
}

/// How a caller reads the state a boolean is evaluated against.
///
/// **Lookups, not a `Document`, because there are three callers seeing three
/// different worlds** — and a second copy of the recursion below would be the thing
/// that drifts:
///
/// - [`crate::Resolved`] reads the committed tree and its own cache.
/// - `RenderOverrides` reads a gesture's *pending* state, so a boolean reshapes
///   while an operand is being dragged rather than snapping into shape on release.
/// - `render::ghost_subtree` reads a **captured** subtree that the document has
///   never seen, so the copy an Alt-drag carries previews as the shape it will be.
///   That third caller is why the tree walk goes through `children_of` and
///   `visible_of` rather than through a `Document`: it has no document to offer.
pub struct Operands<'a> {
    /// The node's local transform.
    pub local_of: &'a dyn Fn(crate::NodeId) -> kurbo::Affine,
    /// The node's kind — **including a pending geometry edit**. Reading the
    /// committed kind here is a specific bug with a specific look: resizing a
    /// boolean moved its operands (their transforms were overridden) without
    /// resizing them (their new *sizes* were not), so the shape shuffled around
    /// instead of scaling and then snapped right on release.
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
/// **The same arithmetic [`evaluate`] does**, over the operands `view` reports. It
/// kept a per-node cache of the fold's prefixes between 2026-08-31 and the same day's
/// removal; `id` is now only the node whose children to read.
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
        // `Artboard` included: a frame's box is its outline now that frames stroke
        // (§15 D144), so it comes out of `local_path` like every other kind's. This
        // arm used to build the rect here, which was a second construction of the
        // same primitive with its own flattening tolerance beside it.
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

/// flo_curves → kurbo, **with hole windings corrected**. Every subpath is closed,
/// which is what a boolean result is.
///
/// ## Why the winding has to be fixed here
///
/// flo_curves expresses a hole as a *separate subpath*, wound the same way as the
/// outline containing it, because it reads a set of subpaths **even-odd**. Ondin
/// fills **non-zero** — every other shape in the model does — and under non-zero a
/// same-wound inner subpath is not a hole at all: it fills solid.
///
/// That is the whole of the reported bug. `Exclude` looked exactly like `Union`,
/// because it *is* a union outline with the overlap as an interior subpath, and the
/// overlap was being filled back in. `Subtract` appeared to work only because every
/// subtraction tried had touched an edge — a notch, which is one subpath and needs
/// no hole. Nudging an operand by a pixel so the overlap reached the edge turned a
/// hole into a notch, which is precisely the "it works if I move it 1px" report.
///
/// So each subpath's nesting depth is counted, and its direction forced to alternate
/// with it: outermost counter-clockwise, a hole inside it clockwise, an island
/// inside that counter-clockwise again. The result then means the same thing under
/// either fill rule, which is also what makes it safe to hand to SVG.
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
        // ⚠️ **A subpath whose interior cannot be located is kept, not dropped**
        // (§15 D457). This read `else { continue; }` until 2026-09-07, which turned
        // *"I could not measure the nesting"* into *"delete this part of the
        // shape"* — and where it was the only subpath the whole boolean answered
        // `None` and drew nothing (`[S10.1-L1-01]`). Losing the winding correction
        // is a hole that fills solid; losing the geometry is a shape that is not
        // there. The first is strictly better, and it is what the caller can see.
        let Some(inside) = representative_point(sub) else {
            out.extend(sub.iter());
            continue;
        };
        // **Bigger-than, not merely contains-a-point-of.** A representative point is
        // any interior point, and for a ring the obvious one — the bounding box's
        // centre — sits inside the hole, so "the hole contains my point" is true of
        // the outline as well and every subpath looked nested inside every other.
        // Comparing areas breaks the symmetry: only a strictly larger subpath can
        // enclose this one, which is sound because boolean output never has crossing
        // subpaths.
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
/// Its bounding-box centre is inside for a convex subpath and often not for a
/// concave one. The fallback is a **scan line**: intersect a horizontal ray with
/// every segment, sort the crossings, and take the midpoint of the first span
/// that is actually inside. That is exact for a closed curve of any thickness,
/// because it measures the shape rather than sampling near it — the span it
/// returns is bounded by the subpath's own edges, so it cannot be thinner than
/// the shape is.
///
/// ⚠️ **This walked an 8×8 grid over the bounding box until 2026-09-07, and a
/// subpath thinner than about one eighth of its own box fell through all 49
/// probes** — `[S10.1-L1-01]`, and the cost was not a bad point but a **deleted
/// shape**, because [`from_flo`] read the resulting `None` as *skip this
/// subpath*. Measured in release against a sampled referee: a crescent (r = 200
/// circle minus the same circle offset 5) came back **`None`** — the boolean drew
/// nothing at all — and a 400×400 square minus a 10-unit L-notch came back at the
/// square's full 160 000 rather than 156 100, the notch simply gone. Both silent:
/// nothing panicked, so [`failures`] did not move, the layers row wore no mark and
/// the status bar said nothing (§15 D298).
///
/// ⚠️ **A wider grid is not the fix, and the sweep is what says so.** The grid was
/// *relative* to the bounding box, so the failure was scale-invariant —
/// geometrically similar crescents vanished at r = 10, 20, 40, 80 and 200 alike,
/// which is also the control that separates this from an `ACCURACY` tolerance
/// problem. Any `STEPS` merely has a thinner crescent.
///
/// The old doc's *"it only has to find **one** interior point and it terminates on
/// the first hit"* was true of the loop and false of the shape.
fn representative_point(subpath: &BezPath) -> Option<Point> {
    use kurbo::ParamCurve;
    let bounds = <BezPath as kurbo::Shape>::bounding_box(subpath);
    if <BezPath as kurbo::Shape>::contains(subpath, bounds.center()) {
        return Some(bounds.center());
    }
    // Several heights rather than one: a single scan line can miss — a subpath
    // whose only ink at that `y` is a tangency, or an hourglass pinched exactly
    // there — and a handful of them costs nothing beside the containment tests
    // this is feeding.
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
        // **`contains` on each span's midpoint, rather than trusting the parity of
        // the crossing count.** A tangency contributes two roots at one x and a
        // shared endpoint contributes one from each of two segments, so parity is
        // not reliable here — but a midpoint bounded by two real crossings is a
        // *candidate* worth one exact test, and the exact test is the answer.
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

/// Into flo_curves' space — **scaled**, see [`FLO_SCALE`]. Every coordinate that
/// reaches flo_curves goes through here, so this and [`k`] are the whole seam.
fn c(p: Point) -> Coord2 {
    Coord2(p.x * FLO_SCALE, p.y * FLO_SCALE)
}

/// Back out of flo_curves' space, undoing [`c`]. The nesting and winding pass in
/// [`from_flo`] runs on the other side of this, in world units.
fn k(p: Coord2) -> Point {
    Point::new(p.0 / FLO_SCALE, p.1 / FLO_SCALE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::FillRule;
    use kurbo::{Rect, Shape};

    /// `n` disjoint 20×20 squares on a 32-wide grid at 40-unit pitch — the shape
    /// of a group of icons used as a mask, which is the case
    /// `[S10.1-L4-04]` measured and the expensive one.
    ///
    /// ⚠️ **Disjoint on purpose.** §15 D239's own cost table used an overlapping
    /// ring, whose union collapses to a handful of subpaths; a set that overlaps
    /// hides the cost this fixture is about, because what grows is the number of
    /// subpaths the accumulator carries into the next `path_add`.
    fn disjoint(n: usize) -> Vec<BezPath> {
        (0..n)
            .map(|i| {
                let x = (i % 32) as f64 * 40.0;
                let y = (i / 32) as f64 * 40.0;
                Rect::new(x, y, x + 20.0, y + 20.0).to_path(0.1)
            })
            .collect()
    }

    /// **Folding a union as a balanced tree gives the same shape as folding it
    /// left** (§15 D614, `[S10.1-L4-04]`).
    ///
    /// 🚨 **The left fold was Θ(N²) and it is paid per repaint and per pointer
    /// move.** A group used as a shape mask unions all its members with no cache
    /// anywhere on the path — `scene::mask_geometry` on the scene walk,
    /// `resolve::mask_path` from the hit test — and each step of a left fold hands
    /// flo_curves an accumulator one operand bigger than the last. Measured in
    /// release on this fixture: **0.56 ms at 16, 26.7 at 128, 103 at 256, 471 at
    /// 512.** At 128 a single mask is more than a whole 16.7 ms frame.
    ///
    /// **After: 0.13 ms at 16, 2.1 at 128, 6.4 at 256, 20.3 at 512** — 24× at the
    /// top, and the curve is N log N rather than ≈N².
    ///
    /// ⚠️ **The measurement is what chose the fix, and it contradicted the
    /// finding's own sketch.** That proposed hoisting `Shape::area` out of
    /// `from_flo`'s O(subpaths²) nesting loop, on the reasoning that the two terms
    /// were comparable — the finding says outright that the attribution was
    /// reasoned rather than measured. Timed apart at N = 256 the fold is **96.2 ms**
    /// and `from_flo` **2.8 ms**, so that fix would have removed 3%.
    ///
    /// **What this test can and cannot say.** Reassociating is a pure optimisation,
    /// so no assertion about the *shape* can bite the change — any correct fold
    /// passes. What it pins is the premise the optimisation rests on: that
    /// `combine` is associative for this operator, to within `ACCURACY`. It is
    /// asserted against the left fold computed here by hand rather than against a
    /// literal, so it stays true if flo_curves' accuracy changes.
    ///
    /// ⚠️ **The delta is exactly `0.0` at every count tried** (16 through 512),
    /// which is stronger than the tolerance below and is deliberately not what is
    /// asserted: two bracketings of a floating-point boolean are entitled to
    /// differ, and a test demanding they never do would be pinning luck.
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
            // And it is the whole of the input, which is what says neither fold
            // dropped an operand: n squares of 400 each, none overlapping.
            assert!(
                (b - n as f64 * 400.0).abs() < ACCURACY,
                "n={n}: the union is {b} where {n} disjoint squares are {}",
                n as f64 * 400.0
            );
        }
    }

    /// **`Subtract` keeps its left fold, and the odd counts are why this needs
    /// four operands** (§15 D614, `[S10.1-L4-04]`).
    ///
    /// 🚨 `a − b − c − d` means `((a − b) − c) − d`. A balanced tree computes
    /// `(a − b) − (c − d)`, which is a **different shape** — everything `d` takes
    /// out of `c` comes back. Union and intersection may be reassociated freely
    /// and subtraction may not, which is why `fold_operands` has an arm for it.
    ///
    /// ⚠️ **Three operands cannot see this and four can.** With three, the tree
    /// pairs the first two and carries the third up untouched, so it computes
    /// `(a − b) − c` — exactly the left fold, and green under the flip. The odd
    /// one riding up a level is what makes an odd count agree by accident.
    ///
    /// **The fixture is a chain, not a pile**: `d` overlaps `c` and nothing else,
    /// so the whole difference between the two bracketings is whether `c ∩ d` is
    /// removed from the result or restored to it — a 200-unit square, which is
    /// what the assertion's own numbers are.
    ///
    /// ⚠️ **Flip-check, run**: deleting the `if op == BoolOp::Subtract` arm from
    /// `fold_operands` fails here at **1000 against 800** — the predicted site,
    /// and the predicted gap of 200, at numbers 600 lower than the prediction
    /// said. The write-up was done before the arithmetic and had the base area
    /// wrong; the *shape* of the claim survived and the numbers did not, which is
    /// the ordinary way round for a flip whose site is easy and whose value is
    /// not. **Every other test in the workspace stays green under it**, including
    /// the three-operand subtract cases — which is the paragraph above, measured.
    #[test]
    fn a_subtraction_is_still_bracketed_from_the_left() {
        let a = Rect::new(0.0, 0.0, 40.0, 40.0).to_path(0.1);
        let b = Rect::new(0.0, 0.0, 10.0, 40.0).to_path(0.1);
        let c = Rect::new(10.0, 0.0, 20.0, 40.0).to_path(0.1);
        let d = Rect::new(10.0, 0.0, 20.0, 20.0).to_path(0.1);
        let out = evaluate(BoolOp::Subtract, &[a, b, c, d]).expect("a shape is left");
        let area = <BezPath as Shape>::area(&out).abs();
        // 1600 − 400 (b) − 400 (c) = 800. `d` is wholly inside `c`, so it removes
        // nothing more. Bracketed as a tree, `c − d` is the top half of `c` alone
        // and only that is taken out: 1600 − 400 − 200 = 1000.
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

    /// The area of `path` read **even-odd**, by sampling a grid over its box.
    ///
    /// ⚠️ **`area` below cannot measure an even-odd path and this is why this
    /// exists.** The shoelace sum is a *winding* integral: it adds each subpath's
    /// signed area, so two overlapping squares wound the same way come to two whole
    /// squares — which is the union's area, and precisely the answer even-odd does
    /// not give. Since `Exclude` became a concatenation (§15 D239) that is every
    /// `Exclude` result, and four tests failed on it the moment it landed.
    ///
    /// **Sampled rather than computed, because there is no cheap exact form** — the
    /// arithmetic that would give one is the arithmetic that was removed for being
    /// wrong. The grid is the accuracy: a cell is 1/`steps` of the box per side, and
    /// the error scales with the boundary length times the cell, so callers assert a
    /// *relative* tolerance rather than the absolute one `area` supports. Where an
    /// exact claim is wanted, test a **point** instead — `FillRule::contains` is
    /// exact and says the same thing about the shape.
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

    /// Area by the shoelace formula over a flattened outline — enough to tell a
    /// union from an intersection, which is what these tests are about.
    ///
    /// ⚠️ **Winding, so it is wrong for an even-odd path** — see `even_odd_area`.
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
            // **`Exclude` is measured even-odd, and the tolerance differs with it.**
            // Its result is the operands concatenated (§15 D239), which the shoelace
            // sum reads as 15,000 — the union — because both squares wind the same
            // way. The sampled measure is accurate to about a cell of boundary, so
            // 0.5% rather than one square unit; a wrong answer here is 50% out.
            let (got, tol) = match op {
                BoolOp::Exclude => (even_odd_area(&result, 300), expected * 0.01),
                _ => (area(&result), 1.0),
            };
            assert!(
                (got - expected).abs() < tol,
                "{op:?}: expected area {expected}, got {got}"
            );
        }
        // **And the exact half of the same claim**, which needs no sampling: the
        // overlap is *outside* an exclusion and inside a union. A point test asks
        // `FillRule` the question the renderer and the hit test both ask.
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

    /// **Exclude *is* commutative, and the module doc used to say it was not.**
    ///
    /// It matters because the key layer (§9.4) reorders operands only where the
    /// order changes the answer: a designation that silently rewrote the tree for
    /// an operation that cannot see the difference would be a promise the
    /// arithmetic does not keep. Symmetric difference is commutative and
    /// associative, so the left fold in `evaluate` gives one answer for every
    /// permutation — three operands rather than two, because two would also pass
    /// if only the pairwise step were symmetric.
    ///
    /// ⚠️ **This test certifies three operands and it has twice been quoted as
    /// certifying any number.** It compares **areas within 1.0**, not geometry, and at
    /// twenty operands a single operand moved to the end of the fold changes the area
    /// by 2,482 — three orders of magnitude past this tolerance (§15 D239, measured
    /// 2026-08-31). The claim it supports is therefore *narrow and still true*: a
    /// three-operand `Exclude` does not care about order, which is all the key layer
    /// needed. It does **not** license reordering a large fold, and two callers read it
    /// as though it did — `RenderOverrides::operand_children` and a rejected design for
    /// the prefix cache that has since been removed with it. *A fixture too small to
    /// reach the regime a claim is quoted in is worse than no fixture, because it is
    /// quoted as evidence.* Left at three operands deliberately: growing it would
    /// change what it certifies, and the larger claim is now `Exclude`'s own — a
    /// concatenation is order-independent by construction, which
    /// `exclude_agrees_with_odd_coverage_at_every_sample_point` checks against a
    /// referee rather than against another permutation.
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

    /// Curves survive: a boolean of two circles is still made of curves, not of a
    /// thousand line segments. This is the whole reason for the library choice, so
    /// it is worth an assertion rather than a comment.
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
        // Checked against the closed form rather than a remembered number: the
        // lens of two circles of radius r whose centres are d apart is
        // 2r²·acos(d/2r) − (d/2)·√(4r²−d²) = 3963.3 here. flo_curves lands within
        // 0.05% of it, which is the accuracy claim worth pinning — a boolean
        // library that is merely *plausible* is the thing to be afraid of.
        let (r, d) = (50.0_f64, 40.0_f64);
        let exact = 2.0 * r * r * (d / (2.0 * r)).acos() - (d / 2.0) * (4.0 * r * r - d * d).sqrt();
        let got = area(&result);
        assert!(
            (got - exact).abs() / exact < 5e-4,
            "lens area {got}, exact {exact}"
        );
    }

    /// Nothing to combine is `None`, not an empty path — the caller needs to know
    /// the difference, since one means "not a boolean" and the other "a boolean
    /// whose result is nothing".
    #[test]
    fn a_single_operand_and_no_operands() {
        assert!(evaluate(BoolOp::Union, &[]).is_none());
        let (a, _) = pair();
        let alone = evaluate(BoolOp::Union, std::slice::from_ref(&a)).expect("itself");
        assert!((area(&alone) - 10_000.0).abs() < 1.0);
    }

    /// **Exclude keeps the two crescents and nothing else.**
    ///
    /// Reported as "exclude just unions", then sharply narrowed by "it works if I
    /// move the oval 1px". The configuration was never the point — the composition
    /// was. XOR as `sub(add(a,b), intersect(a,b))` hands `path_sub` a subtrahend
    /// whose boundary is made entirely of the operands' own edges, and the traversal
    /// re-assembles the operands: the result came back as the two original shapes
    /// untouched, which looks exactly like a union. `path_full_intersect`'s two
    /// exteriors avoid the second boolean entirely.
    ///
    /// Checked against the overlap rather than by eye — XOR must equal union less
    /// intersection — and at the offsets that failed, including the aligned one.
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
            // Even-odd since §15 D239, so sampled: `area` would report the union.
            // 0.3% of the union rather than a unit, which is the sampling grid's
            // accuracy and still an order of magnitude inside the failure this
            // test was written for — "exclude just unions" is `xor == union`, and
            // `both` is a fifth of the union at the tightest offset here.
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

    /// A subtraction that leaves a genuine **hole** — the case the winding
    /// normalization in `from_flo` exists for. flo_curves returns a hole as a second
    /// subpath wound the *same* way as the outline containing it, because it reads a
    /// path set even-odd; a non-zero fill then paints the hole solid. Reversing it
    /// makes the result mean the same thing under either rule.
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

    /// `n` circles of radius 90 spaced evenly round a circle of radius 120, so
    /// every operand overlaps several of its neighbours. The shape the cost of
    /// these operations was measured on — see the module docs.
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

    /// **A boolean that panics comes back as `None` rather than taking the process
    /// with it** (§15 D239). This was `#[ignore]`d and red — the reproduction kept
    /// so it would not be lost — until `evaluate` grew its guard.
    ///
    /// ⚠️ **The panic is injected, and it did not used to be.** Until 2026-08-31
    /// this evaluated `Exclude` over a forty-circle ring, which really did trip an
    /// intransitive comparator in flo_curves 0.8.0: the Rust ≥ 1.81 sort refusing
    /// an inconsistent comparator out of `core::slice::sort`, so the underlying
    /// fault was older than the panic and what had changed is that it became loud.
    /// **flo_curves 0.8.1 fixed it** — `exterior_paths` sorts on `total_cmp` and
    /// segments the close-x runs afterwards, instead of putting the epsilon inside
    /// the comparison — and re-running the old sweep confirms it: every one of the
    /// eight counts D239 measured `Exclude` panicking at (40, 42, 44, 47, 49, 59,
    /// 60, 63) now returns a real path. So does every degenerate operand tried in
    /// its place — NaN, infinite and 1e300 coordinates across three operations —
    /// which is unsurprising once the sort is `total_cmp`, since that is a total
    /// order on NaN too. ⚠️ **The claim there is *"nothing unwinds"*, and the
    /// 1e300 case has stopped describing `evaluate`** (§15 D843): `MAX_BOOL_COORD`
    /// refuses that magnitude before the fold, so re-running that half of the
    /// sweep now measures the bound rather than flo_curves.
    ///
    /// **What this test now claims, and what it no longer claims.** It says the
    /// guard turns an unwind into `None` and lets a healthy boolean past; it says
    /// nothing about whether flo_curves can still produce one. The old fixture
    /// asserted both at once and that is the property `poison_next` cost —
    /// deliberately, rather than by hunting a new panicking set, which would have
    /// been proving a negative.
    ///
    /// **The panic message still reaches stderr and that is deliberate**: the
    /// default hook runs before the unwind is caught, and a library replacing it
    /// would swallow every real panic in the app. The hook is silenced *here* only
    /// so this test's own output is not a wall of backtrace, and restored
    /// afterwards, because a test that leaves the process without a panic hook
    /// hides the next failure.
    #[test]
    fn a_boolean_that_panics_comes_back_empty_rather_than_taking_the_process() {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        poison_next(1);
        let caught = evaluate(BoolOp::Exclude, &ring(40));
        std::panic::set_hook(hook);
        assert_eq!(caught, None, "a caught unwind has no path to answer with");

        // And the guard is not swallowing the ordinary answers on the way past:
        // the same ring, unpoisoned, unions to a real shape. **On 0.8.1 it is also
        // what the poisoned call above would have done**, which is the difference
        // from the old version of this test and the reason the arming is one call
        // rather than a mode: forget it and this assertion is what fails.
        let union = evaluate(BoolOp::Union, &ring(40));
        assert!(
            union.is_some_and(|p| !p.is_empty()),
            "the guard ate a boolean that never panicked"
        );
    }

    /// **`None` is two different answers and `failures` is what separates them.**
    /// The guard above made a caught unwind indistinguishable from an intersection
    /// that came out empty — which is a *correct* result, and drawing nothing for it
    /// is right — so nothing downstream could tell the user the difference and the
    /// shape simply vanished.
    ///
    /// Both halves asserted against the same `None`, because that is the whole
    /// difficulty: the empty case must leave the counter alone and the abandoned one
    /// must move it, and a version that increments on every `None` — the obvious
    /// wrong one, since `None` is where the guard already lives — passes neither
    /// this nor `resolve`'s `a_boolean_the_arithmetic_gave_up_on_is_marked_and_an_empty_one_is_not`.
    ///
    /// Read as a *difference* rather than against zero: tests share a process, and
    /// nothing here should depend on being the first to trip the defect.
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
        // Injected since 0.8.1 — see `poison_next`. The operands are still the
        // forty-circle ring rather than anything simpler, because the *empty* half
        // above must stay the only reason the two `None`s could differ.
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

    /// **The poison is one call, not a mode**, which is what stops it leaking into
    /// a later test in the same thread and marking a boolean that never failed.
    ///
    /// Worth its own test because the arming is a decrementing counter rather than
    /// a boolean, and an off-by-one there would be invisible: every test that arms
    /// it asserts on the call *after* arming, so a poison that survived one extra
    /// call would only show up as an unrelated test failing somewhere downstream.
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
    /// `moved` shifts one operand, which is what a drag does to exactly one of them.
    /// The ids are minted rather than constructed: `NodeId` has no raw constructor,
    /// which is why the lookup below searches the list instead of indexing it.
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
    /// **Recorded rather than computed, because computing it costs 23 seconds in a
    /// debug build** — `odd_coverage_area` samples a 4000² grid per count, which is
    /// the right instrument and the wrong thing to run on every `cargo test`. The
    /// values are deterministic, so they are constants here and
    /// `the_recorded_truth_matches_a_fresh_sampling` regenerates them on demand.
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

    /// **The recorded truth is what a fresh sampling says** — `#[ignore]`d because it
    /// is 23 seconds in a debug build, and kept because a table of constants nobody
    /// can regenerate is a table nobody can trust.
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
    /// **XOR is exactly "covered by an odd number of operands"**, so this needs no path
    /// arithmetic at all and is independent of everything under test — which is the
    /// whole reason it can referee. A 4000² grid over the ring's box puts the sampling
    /// error near 20 units in 60,000, which the accurate rows below sit comfortably
    /// inside.
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

    /// ⚠️ **`Exclude` returns a materially wrong shape for some operand geometries,
    /// and this is the test that says so** (§15 D239, found 2026-08-31).
    ///
    /// **Nothing had ever checked `Exclude`'s answer against anything but itself.**
    /// Every other test in this module compares one boolean against another boolean,
    /// or against an area computed from the same arithmetic; the one that looked like
    /// an independent check compares *permutations of the same call*. So a fold that
    /// was wrong the same way every time was indistinguishable from a fold that was
    /// right, and had been since booleans landed. `odd_coverage_area` is the first
    /// referee this module has ever had, and it found the arithmetic wanting on its
    /// first run.
    ///
    /// Measured, release, ring of radius-90 circles on a radius-120 circle, when
    /// `Exclude` still folded. The `Exclude` column is what the fold said **with the
    /// spatial cull**, which was then what shipped; the last column is what it said
    /// without, the cull having halved the error. Both mechanisms are gone and so is
    /// the fold — the table is kept for what it says about `path_full_intersect`:
    ///
    /// | operands | true area | `Exclude` | error | uncalled error |
    /// | --- | --- | --- | --- | --- |
    /// | 10 | 64,182.9 | 64,196.1 | 13 | 13 |
    /// | 11 | 76,259.2 | 76,279.8 | 21 | 21 |
    /// | 12 | 81,424.4 | 82,261.0 | **837 — 1.0%** | 4,156 |
    /// | 13 | 77,888.4 | 84,992.4 | **7,104 — 9.1%** | 14,210 |
    /// | 14 | 69,810.3 | 82,184.2 | **12,374 — 17.7%** | 24,754 |
    /// | 15–18 | — | — | under 20 | under 20 |
    /// | 19 | 77,696.5 | 78,829.6 | **1,133 — 1.5%** | 1,939 |
    /// | 20 | 76,745.1 | 82,552.8 | **5,808 — 7.6%** | 9,129 |
    ///
    /// **The pattern is the signature of D239's original panic**: not monotonic in the
    /// count, fine either side of a bad run, so *the variable is the geometry and not
    /// the number of operands* — the same sentence that entry already carries, about
    /// what is presumably the same fragility in `path_full_intersect`. Where that one
    /// was loud and became a caught unwind, this one is **silent**: the boolean draws,
    /// nothing is marked, `failures` does not move, and the shape is simply wrong.
    ///
    /// ✅ **All of that is history: since the fill rule landed, `Exclude` is exact at
    /// every count in the table** — errors of a few units where they were thousands,
    /// because there is no longer any arithmetic to be wrong. The rows above are kept
    /// as the record of what the fold did, and this test now demands the opposite of
    /// what it was written to pin. *The characterisation test that gets to fail is the
    /// point of writing one.*
    ///
    /// ⚠️ **Asserted point by point rather than by area, and that is the stronger
    /// claim as well as the affordable one.** Two shapes can have equal areas and be
    /// different shapes, so an area match is circumstantial; agreeing about *every
    /// sampled point* is the thing itself. It is also what fits in a debug build —
    /// sampling a path's winding costs a pass over every segment, so a grid fine
    /// enough to measure area to a fraction of a percent is billions of segment tests,
    /// where a few hundred points is nothing. The referee knows only circles and
    /// counting, so it cannot agree with a wrong answer by construction.
    ///
    /// The lattice steps by an irrational fraction of the box so no sample lands on a
    /// circle's centre line or on the ring's own symmetry axes, where a point sits on
    /// a boundary and *both* answers are legitimately undecidable.
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
                // Skip a point within a hair of any circle's edge: there the truth
                // and the path disagree only about which side of the line they are,
                // which is not what this is testing.
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
            // The fixture has to exercise both answers, or the assertion above is
            // satisfied by a shape that contains everything or nothing.
            assert!(
                inside > 20 && checked - inside > 20,
                "n={n}: {inside} inside of {checked} — the sample must straddle the edge"
            );
        }
    }

    /// **A thin concave result is a shape, not a rounding error, and it used to be
    /// deleted.**
    ///
    /// `[S10.1-L1-01]` (§15 D457). `representative_point` walked an 8×8 grid over
    /// the subpath's own bounding box, so a subpath thinner than about one eighth
    /// of that box fell through all 49 probes — and `from_flo` read the resulting
    /// `None` as *skip this subpath*, so the answer was not a bad winding but a
    /// missing shape.
    ///
    /// The two fixtures are the finding's, and neither is contrived: a **crescent
    /// moon** (a circle minus the same circle nudged sideways) came back `None` —
    /// the boolean drew nothing at all — and a **bracket** (a square minus a thin
    /// L-shaped notch) came back at the square's full area, the notch simply gone.
    ///
    /// ⚠️ **The similarity sweep is the anti-vacuity control, and it is the whole
    /// reason a wider grid is not the fix.** The grid was *relative* to the
    /// bounding box, so the failure was scale-invariant: geometrically similar
    /// crescents vanished at every radius alike. A fix that merely raised `STEPS`
    /// passes the two area assertions and fails this. It is also what separates
    /// the defect from an `ACCURACY` tolerance problem, which would appear only at
    /// small radii.
    ///
    /// ⚠️ **The convex control is why the fix is scoped to the fallback.** An
    /// `Intersect` producing a thin sliver *rect* was always correct, because a
    /// convex subpath's bounding-box centre is inside it and the fallback never
    /// ran.
    ///
    /// ⚠️ **A separate defect turned up while writing that control, and it is not
    /// this one.** The finding used a 400 × 0.01 sliver and recorded its area as
    /// *"correctly, 2.0"*; the correct answer is **4.0**, and it is 2.0 both
    /// before and after this change. Swept here at four thicknesses, `Intersect`
    /// of a 400-wide slot with a 400×200 rect:
    ///
    /// | thickness | true area | `evaluate` |
    /// | --- | --- | --- |
    /// | 0.01 | 4 | **2** |
    /// | 0.1 | 40 | 40.0 |
    /// | 1 | 400 | 400.0 |
    /// | 10 | 4 000 | 4 000.0 |
    ///
    /// So an `Intersect` result under about 0.1 units thick loses **half its
    /// area**, sharply rather than gradually, in a shape with no concavity and no
    /// hole — which is neither `representative_point`'s doing nor `from_flo`'s.
    /// It is not fixed here and it is not this finding. The control is at 0.1
    /// rather than 0.01 so it controls for what it is meant to.
    ///
    /// ⚠️ **And it was silent.** Nothing panicked, so `failures()` did not move,
    /// `resolve::reevaluate_boolean` took the `failures() == before` arm and
    /// cleared the node from `failed`, and the layers row wore no mark (§15 D298).
    /// That is exactly the mode D239's third amendment names: *"the boolean draws,
    /// nothing is marked, `failures` does not move, and the shape is simply
    /// wrong."* The `failures()` assertion below is the standing record of it.
    ///
    /// ⚠️ **Flipped in all three combinations, and the predicted site was wrong
    /// once.** The fix is two changes and they do different work:
    ///
    /// - **Both reverted** — the state the finding measured: fails on the very
    ///   first line, `expect("a crescent is a shape, not nothing")`. The boolean
    ///   answers `None` and draws nothing.
    /// - **Scan line reverted, the `else` branch kept**: the crescent **passes** —
    ///   its geometry survives now even with no interior point found — and the
    ///   failure moves to the **notch**, at **163 900** against 156 100. That is
    ///   the winding half on its own: the hole is kept and not reversed, so under
    ///   non-zero it fills solid. *The crescent assertion was the predicted site
    ///   and it is not this one*; the notch is what separates "kept" from
    ///   "kept and wound correctly".
    /// - **`else` branch reverted, the scan line kept**: **every assertion green.**
    ///   That is not a failed experiment — it says the scan line is the
    ///   load-bearing half today and the `else` branch is a belt with nothing
    ///   currently behind it. It is kept anyway, because *"I could not measure the
    ///   nesting"* must never again mean *"delete this part of the shape"*, and
    ///   because nothing in this module guarantees the scan line always succeeds.
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

        // A crescent 200 units across and about 5 thick — under 1/40 of its own
        // bounding box, so every one of the old grid's 49 probes missed it.
        let moon = crescent(200.0, 5.0).expect("a crescent is a shape, not nothing");
        let area = moon.area().abs();
        assert!(
            (area - 2000.0).abs() < 60.0,
            "the crescent's area is about 2000, got {area}"
        );

        // A 400×400 square with a 10-unit L-shaped notch bitten out of it, inset
        // 100 from two edges. The notch is a hole in the middle of the square, so
        // it is the *winding* half of the same defect: the subpath survived, its
        // direction did not get corrected, and under non-zero it filled solid.
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

        // **The similarity sweep.** Same shape, five sizes: a relative grid fails
        // all of them and a scan line passes all of them.
        for r in [10.0f64, 20.0, 40.0, 80.0, 200.0] {
            let m = crescent(r, r / 40.0).unwrap_or_else(|| panic!("r={r}: drew nothing"));
            let a = m.area().abs();
            let want = 2000.0 * (r / 200.0) * (r / 200.0);
            assert!(
                (a - want).abs() < want * 0.1,
                "r={r}: area {a}, expected about {want} by similarity"
            );
        }

        // **The convex control**: a 400 × 0.01 sliver rect, which was always right
        // because its bounding-box centre is inside it and the fallback never ran.
        let wide = kurbo::Rect::new(0.0, 0.0, 400.0, 200.0).to_path(0.001);
        let slot = kurbo::Rect::new(0.0, 99.95, 400.0, 100.05).to_path(0.001);
        let sliver = evaluate(BoolOp::Intersect, &[wide, slot]).expect("a sliver is a shape");
        assert!(
            (sliver.area().abs() - 40.0).abs() < 0.2,
            "the convex control must stay correct: {}",
            sliver.area().abs()
        );

        // **And none of it was ever reported.** Nothing panics on any of these, so
        // the counter cannot be what tells the user — which is why the failure was
        // invisible and why this is asserted rather than assumed.
        assert_eq!(
            failures(),
            before,
            "no operand here panics, so `failures` must not move — a wrong shape \
             from this path is silent by construction"
        );
    }

    /// 🚨 **A boolean over enormous operands returns instead of hanging**
    /// (§15 D843).
    ///
    /// §15 D239's `catch_unwind` answers `None` for the input class this module
    /// cannot survive, and **a spin is not an unwind**: at ~10³⁰⁴ world units
    /// `flo_curves` did not come back — past 590 s in debug, past 20 s in
    /// release — with no panic, no `failures()` bump and no return. `evaluate`
    /// runs on the UI thread, so that is the editor frozen with the document
    /// open. `FLO_SCALE` moved the threshold down three decades and did not
    /// create the band; the bound closes both.
    ///
    /// ⚠️ **No wall-clock assertion, deliberately** (§15 D829, and the same
    /// argument `svg_in`'s selector bomb makes): a timing assertion measures the
    /// machine, while a spin does not return at all, so the harness's own
    /// timeout is the sharper instrument. The fixture is sized to be hopeless
    /// rather than merely slow. **Flip run: with the bound disabled this test
    /// does not return in 150 s**, where the whole `boolean` suite takes 1.28 s
    /// with it — that is the check, and there is nothing else to assert.
    ///
    /// ⚠️ **The control is the point of the second half.** A bound that refused
    /// everything would pass the first assertion, so an ordinary boolean is
    /// asserted to still answer `Some` in the same test — and `1e100`, the
    /// largest magnitude measured to produce a result, is asserted to still
    /// produce one, which is what says the bound was placed above the working
    /// range rather than through it.
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

        // Control: the bound is above everything that ever answered.
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

    /// A thin `Intersect` keeps its area all the way down to 10⁻⁴ world units.
    ///
    /// **The defect this pins was a bow-tie, not a rounding error** (§15 D794).
    /// flo_curves' `GraphPath` merges two points closer than its own compiled-in
    /// `CLOSE_DISTANCE` (0.01) into one, so the two *short* ends of a thin result
    /// — each exactly the result's thickness — were collapsed to single points.
    /// A rectangle pinched at both ends is two triangles, which is **exactly
    /// half** the area, and that is what the sweep measured: 2.0 where 4.0 was
    /// owed, 1.0 where 2.0 was, and at 0.001 — under `SMALL_DISTANCE` — `None`,
    /// nothing drawn at all. [`FLO_SCALE`] is the fix.
    ///
    /// ⚠️ **Flipped both ways, and each flip alone reproduces the defect** —
    /// which is why [`FLO_SCALE`] carries a four-cell matrix rather than a
    /// sentence. `FLO_SCALE` to 1.0 fails here; `FLO_ACCURACY` to
    /// `ACCURACY * FLO_SCALE` — the spelling that looks obviously right, since it
    /// keeps the tolerance meaning a hundredth of a *world* unit — fails here
    /// too, at the same thickness and with the same area. **The fix was written
    /// that way first and this test is what caught it**, so the flip is not
    /// hypothetical: it is the version that shipped for one `cargo test`.
    ///
    /// ⚠️ **The predicted site was right and the prediction under it was wrong.**
    /// `t = 0.01` is where it fails, but only because the sweep runs coarse-first:
    /// every thickness below it fails as well, and 0.0001 fails as `None` — the
    /// `unwrap_or_else` rather than the area assertion. A sweep that stopped at
    /// 0.005 would have reported a halving where the answer is an erasure.
    #[test]
    fn a_thin_intersect_keeps_its_area() {
        use kurbo::Shape;

        let before = failures();
        let wide = kurbo::Rect::new(0.0, 0.0, 400.0, 200.0).to_path(0.001);

        // 0.02 and above were always right; 0.01 and below were the defect, and
        // 0.001 was where the shape disappeared entirely. The sweep spans all
        // three so a regression cannot hide in the half nobody measured.
        //
        // 🚨 **`0.00002` is what pins `FLO_SCALE` to the floor D794 states**
        // (§15 D856, `[X8-L6-02]`). The sweep stopped at 10⁻⁴, ten times above
        // the 10⁻⁵ the entry claims, so any scale clearing `CLOSE_DISTANCE` there
        // passed: the constant could fall from 1000 to 110 with all of core
        // green. At 2 × 10⁻⁵ it has to be roughly 500 or more. **Flip-check,
        // run**: `FLO_SCALE = 200.0` fails here at *"t=0.00002: area 0.0039…,
        // expected 0.008"* — the bow-tie, half the area — and 1000 passes.
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

        // **And the shape is a quadrilateral, which is the half of the claim area
        // cannot make.** The bow-tie had four elements to the rectangle's six,
        // because the two collapsed ends left no edge behind; an area assertion
        // alone would pass against any shape of the right size.
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
