//! Composite transaction builders and the world-space projection facade
//! (§5.7 `TransactionBuilder`, §8.4 `place_at_world`, §13 decision 2).
//!
//! Everything here turns an intent expressed the way a *user* (or an agent)
//! thinks about it — "group these", "put this at these world coordinates",
//! "move the selection right by 10" — into one `Transaction` of primitive
//! operations. One intent = one transaction = one undo step (invariant 5).
//!
//! It lives in core, not in the app, for the reason §8.4 and §13 both give:
//! the UI, MCP write tools, and the future command line all need the exact same
//! projections, and a second implementation would drift. Nothing here mutates —
//! each function is a pure `(&Document, &Resolved, …) -> Transaction`.
//!
//! **World vs. local.** The document stores local transforms only (invariant 6).
//! Users and agents overwhelmingly think in world coordinates. The conversion
//! `local = parent_world⁻¹ · world` appears exactly once, in [`local_for_world`],
//! and everything else routes through it.

use crate::document::Document;
use crate::id::{IdSource, NodeId};
use crate::image::Brush;
use crate::node::{Fill, Node, NodeKind, Stroke};
use crate::op::{GeometryPatch, OpError, Operation, Transaction};
use crate::query::pivot_world;
use crate::resolve::Resolved;
use kurbo::{Affine, Point, Vec2};
use peniko::Color;
use rustc_hash::{FxHashMap, FxHashSet};

/// Whether a node of `child` may be a direct child of a node of `parent`
/// (§5.3): a frame hangs off the root, another frame or a group, and nothing may
/// hold a `Root`.
///
/// The root **may** hold loose shapes. A frame is a glorified group, so a shape
/// dragged more than halfway out of one has to land somewhere — and the only
/// honest destination is "no frame", i.e. the root. Forbidding that made the
/// gesture impossible to express rather than making documents tidier.
///
/// **Frames nest.** A frame is a page, and a page inside a
/// page is how every design tool expresses a card, a component, a state — the rule
/// that kept them at the root made "put this frame in that one" inexpressible, which
/// is the same shape of mistake as the loose-shape rule above and was fixed for the
/// same reason (§15).
///
/// **And inside groups, since container layout** (§15 D870, D876): a flex row of
/// cards is a group whose cards are frames, for their fill and their clip. It
/// held a frame out of every group until then, on two reasons of unequal weight —
/// *"a clipping page inside one would be clipped by something with no edges"*,
/// which was never true, since a group does not clip; and `paint_targets`, which
/// already stops at a frame it reaches through a group. Any group, not only one
/// with a `display`, or removing a group's layout would have to be refused
/// whenever it held a frame.
///
/// ⚠️ **What a frame still may not sit under is a boolean or a mask, at any
/// depth** — and that is not this function's question, because it is about an
/// *ancestor* rather than a parent: a frame two groups inside an operand is as
/// wrong as one directly in it. [`Document::frame_may_sit_under`] asks it, and
/// every door that asks this asks that too.
///
/// The one statement of the parent rule. `apply` enforces it, the loader enforces
/// it, the layer tree greys out illegal drops with it, and render previews refuse
/// to draw a create that would be rejected — all from here, so they cannot
/// disagree about what is legal.
pub fn can_parent(parent: &NodeKind, child: &NodeKind) -> bool {
    match child {
        // There is exactly one root and it is nobody's child.
        NodeKind::Root => false,
        NodeKind::Artboard { .. } => {
            matches!(
                parent,
                NodeKind::Root | NodeKind::Artboard { .. } | NodeKind::Group
            )
        }
        _ => true,
    }
}

/// Whether `opacity` is in the range nodes accept (§5.7, `BadOpacity`).
pub fn valid_opacity(opacity: f32) -> bool {
    (0.0..=1.0).contains(&opacity)
}

/// Whether every coefficient of a transform is finite (`OpError::NonFinite`).
///
/// Beside [`valid_opacity`] because it is the same kind of thing: a field
/// invariant of the model, spelled once, asked by the operation that writes the
/// field *and* by the loader that reads it back.
///
/// ⚠️ **[`local_for_world`] below is what produces the value this refuses**, and
/// the two functions being neighbours is not a coincidence: `parent_world
/// .inverse()` on a singular parent divides by a zero determinant, so all six
/// coefficients come back `NaN`. Guarding here rather than inside
/// `local_for_world` is deliberate — the projection has no better answer to give
/// and its callers have no way to report one, whereas the operation returns a
/// `Result` that every commit path already handles.
pub fn affine_is_finite(t: Affine) -> bool {
    t.as_coeffs().iter().all(|c| c.is_finite())
}

/// Whether `t` can be inverted without manufacturing a value the file cannot
/// hold (§15 D713).
///
/// **Finite is not the same question, and the gap between them is a whole
/// class.** `Affine::inverse` divides by the determinant, so a *singular* matrix
/// — every coefficient finite, [`affine_is_finite`] perfectly happy — returns
/// `NaN` in all six. Anything that then maps a point through it writes `NaN`
/// into the model, `serde_json` writes `NaN` as `null`, and the next load
/// answers *"invalid type: null, expected f64"*. That is invariant 8's failure
/// arriving through a door invariant 8's own predicate holds open.
///
/// **Threshold-free on purpose.** The question a caller actually has is *"will
/// the inverse be representable"*, so that is the question this asks: no
/// constant to tune, and it is right by construction rather than by choosing a
/// number small enough.
///
/// 🚨 **This is the only spelling now, and it was one of five** (§15 D762). D713
/// counted the other four and left them: `determinant().abs() > 1e-12` and
/// `determinant() == 0.0 || !finite` in `svg_in`,
/// `determinant().abs() < f64::EPSILON` and `!det.is_finite() || det == 0.0` in
/// the SVG writer. Each was an argument about an epsilon nobody can defend, and
/// the `f64::EPSILON` one was not even that: ≈2.22e-16 is a **relative**
/// quantity — the gap between 1.0 and the next double — where a determinant is
/// in **squared world units**, so it stopped guarding at large coordinates and
/// its refusal point had no relation to what it measured. ⚠️ **They were not
/// four distinct tests**: `svg_in`'s transform fold and the SVG writer's
/// `sweep_radius` are the same test with the operands reversed, in two crates.
/// ⚠️ **Count rather than trust the count** —
/// `grep -rn 'determinant' crates/ --include=*.rs` is the whole check, and most
/// of its hits are prose or an area measure rather than a guard.
///
/// ⚠️ **Not a validator for the model.** A singular transform is a legal thing
/// for a `.ondin` to contain — `svg_in` drops one to the identity on the way in
/// (`svg_in`'s `transform=` fold) and `image.rs` answers `Framing::identity()`
/// for a degenerate framing — so this is the guard a *reader* asks before
/// inverting, not a rule about what may be stored. The loader still accepts one.
pub fn affine_is_invertible(t: Affine) -> bool {
    affine_is_finite(t) && affine_is_finite(t.inverse())
}

/// Whether every number in a brush is finite (`OpError::NonFinite`) — §15 D451.
///
/// **The paint half of invariant 8, which had only the coordinate half.**
/// `OpError::NonFinite`'s own doc scoped itself to *"a node's geometry, its
/// transform, or a guide's position"* until 2026-09-07 — it names paint now — and
/// `op_set_fills`/`op_set_strokes` stored whatever brush they were handed. So a `NaN` reached the model through paint,
/// `Document::apply` accepted it, `serde_json` wrote it as `null`, and the file
/// answered *"invalid type: null, expected f32"* on the way back in — §2's
/// *"the last place such a value can be refused while the user still has their
/// work"* was true of coordinates and not of paint (§15 D448 found it at the
/// importer; this is the same value one door further in).
///
/// **Everything a brush can carry, because the question is what `serde_json`
/// will refuse to write**, not what looks like a coordinate: a solid colour's
/// four components, a gradient's ramp geometry *and* every stop's offset and
/// colour, and the gradient's own transform. A colour channel is as capable of
/// being `NaN` as a radius is, and it is written to the same file.
pub fn brush_is_finite(brush: &crate::Brush) -> bool {
    fn color_ok(c: peniko::color::AlphaColor<peniko::color::Srgb>) -> bool {
        c.components.iter().all(|f| f.is_finite())
    }
    fn stops_ok(g: &peniko::Gradient) -> bool {
        g.stops
            .iter()
            .all(|s| s.offset.is_finite() && color_ok(s.color.to_alpha_color()))
    }
    match brush {
        peniko::Brush::Solid(c) => color_ok(*c),
        peniko::Brush::Gradient(g) => {
            affine_is_finite(g.transform)
                && gradient_kind_is_finite(&g.gradient)
                && stops_ok(&g.gradient)
                // 🚨 **`valid_opacity`, not `is_finite`** (§15 D767). The field is
                // `#[serde(default)]` and the loader validates nothing, so any six
                // characters in a `.ondin` become it — D713's shape exactly, one
                // field later. A non-finite one dies at serde, so what has to be
                // refused here is the **finite out-of-range** one: `-5` would
                // subtract alpha and `1e30` would saturate every stop opaque, both
                // silently, and both survive a round trip.
                //
                // Same predicate as a layer's `opacity` — `schema.rs`'s own words:
                // *"so there is one answer rather than two spellings of it"*.
                && valid_opacity(g.opacity)
        }
        peniko::Brush::Image(i) => i.sampler.alpha.is_finite() && image_ref_is_finite(&i.image),
    }
}

/// Invariant 8 for an effect's numbers (§15 D641).
///
/// The third family after coordinates (D421) and paint (D451, D452), and it is
/// here for the same reason both of those are: every one of these is written to
/// the file as an ordinary `f64`, `serde_json` writes a non-finite one as `null`,
/// and the reader has no arm for `null` where a number is wanted.
///
/// 🚨 **And this one has a consequence before the file is ever saved, which the
/// other two do not.** A blur radius feeds `effect::stack_escape` and then
/// `effect::escaped`, and an *infinite* escape comes back as **no** escape —
/// `escaped` splits the insets into an offset and a radius, `inf − inf` is `NaN`,
/// and `f64::min` resolves `NaN.min(0.0)` to the non-`NaN` operand. So the box
/// comes back unchanged and the viewport cull, the export plan and both
/// backends' buffer boxes all believe an unbounded blur reaches nowhere
/// (`[S10.1-L1-05]`). Refusing at the operation is what makes `escaped`'s input
/// finite; see its own doc for why the alternative — answering an infinite box —
/// is the bug §15 D495 removed rather than the fix.
///
/// ⚠️ **A shadow's `color` is in here and it is the arm a reader adds last.**
/// It is the same argument as `brush_is_finite`'s: the question is what
/// `serde_json` will refuse to write, not what looks like a distance.
pub fn effect_is_finite(effect: &crate::Effect) -> bool {
    fn shadow_ok(s: &crate::Shadow) -> bool {
        s.offset.x.is_finite()
            && s.offset.y.is_finite()
            && s.blur.is_finite()
            && s.spread.is_finite()
            && s.color.components.iter().all(|f| f.is_finite())
    }
    match &effect.kind {
        crate::EffectKind::DropShadow(s) | crate::EffectKind::InnerShadow(s) => shadow_ok(s),
        crate::EffectKind::LayerBlur { radius } => radius.is_finite(),
        crate::EffectKind::Filters(f) => {
            f.brightness.is_finite()
                && f.contrast.is_finite()
                && f.saturation.is_finite()
                && f.hue.is_finite()
        }
    }
}

/// The numbers an [`crate::ImageRef`] carries, which are not peniko's.
///
/// ⚠️ **This arm read `i.sampler.alpha` alone until `[S9.1-L5-01]` was picked
/// up**, which is a gap in D451 that D451 could not have seen: `ImageBrush`'s own
/// fields are peniko's, and the *reference* hanging off it — the crop rectangle,
/// the tile scale, the seven adjustment sliders — is this crate's, so a check
/// written by reading `peniko::Brush` stops one level short. **The one number
/// that reaches this by an ordinary route is the crop**: `crop_clamped`'s guards
/// asked `> 0.0` alone, which `inf` passes, so a crop whose span overflowed came
/// back `NaN` and one crop-pan drag committed it through `SetFills`.
/// `image::usable_span` — plain backticks, because it is private to that module
/// and a link here is decoration no gate can validate (§15 D319) — closed that
/// manufacturer in the same change
/// (D452); **this stays as the second of the two guards**, because it is the one
/// that holds if anything else ever makes such a number — see
/// `image::tests::a_crop_whose_span_overflows_never_reaches_the_document`, which
/// asserts them separately for that reason.
fn image_ref_is_finite(r: &crate::ImageRef) -> bool {
    let a = &r.adjust;
    [r.crop.x0, r.crop.y0, r.crop.x1, r.crop.y1, r.tile_scale]
        .iter()
        .all(|f| f.is_finite())
        && [
            a.exposure,
            a.contrast,
            a.saturation,
            a.temperature,
            a.tint,
            a.highlights,
            a.shadows,
        ]
        .iter()
        .all(|f| f.is_finite())
}

/// The coordinates a gradient's own kind carries, in the space its transform
/// maps from.
///
/// Exhaustive over `GradientKind` with no wildcard, so a fourth kind in a future
/// peniko stops the build here rather than silently going unchecked — which is
/// the same argument `Operation::changes_nothing` makes (§15 D428).
fn gradient_kind_is_finite(g: &peniko::Gradient) -> bool {
    match &g.kind {
        peniko::GradientKind::Linear(p) => [p.start.x, p.start.y, p.end.x, p.end.y]
            .iter()
            .all(|f| f.is_finite()),
        peniko::GradientKind::Radial(p) => {
            [
                p.start_center.x,
                p.start_center.y,
                p.end_center.x,
                p.end_center.y,
            ]
            .iter()
            .all(|f| f.is_finite())
                && p.start_radius.is_finite()
                && p.end_radius.is_finite()
        }
        peniko::GradientKind::Sweep(p) => {
            p.center.x.is_finite()
                && p.center.y.is_finite()
                && p.start_angle.is_finite()
                && p.end_angle.is_finite()
        }
    }
}

/// The local transform that puts `id` at `world`, given where its parent sits.
/// The single place the world→local projection is written (§13 decision 2).
pub fn local_for_world(doc: &Document, res: &Resolved, id: NodeId, world: Affine) -> Affine {
    let parent_world = doc
        .get(id)
        .and_then(|n| n.parent())
        .and_then(|p| res.world_transform(p))
        .unwrap_or(Affine::IDENTITY);
    parent_world.inverse() * world
}

/// Place `id` so its world transform becomes `world` (§8.4).
pub fn place_at_world(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    world: Affine,
) -> Result<Transaction, OpError> {
    if !doc.contains(id) {
        return Err(OpError::NoSuchNode(id));
    }
    Ok(Transaction(vec![Operation::SetTransform {
        id,
        transform: local_for_world(doc, res, id, world),
    }]))
}

/// Translate every node in `ids` by `delta` in world space.
///
/// Nodes whose ancestor is also in `ids` are skipped: moving the ancestor
/// already carries them, and emitting both would move them twice. That makes
/// this correct for arbitrary multi-selections, which is why the app must not
/// hand-roll the per-node version.
pub fn move_by_world(
    doc: &Document,
    res: &Resolved,
    ids: &[NodeId],
    delta: Vec2,
) -> Result<Transaction, OpError> {
    let translate = Affine::translate(delta);
    let ops = outermost(doc, ids)
        .into_iter()
        .filter_map(|id| {
            let old_world = res.world_transform(id)?;
            Some(Operation::SetTransform {
                id,
                transform: local_for_world(doc, res, id, translate * old_world),
            })
        })
        .collect();
    Ok(Transaction(ops))
}

/// Move `id` under `new_parent` at `index` without moving it on screen: the
/// local transform is re-projected through the new parent's world transform.
///
/// Plain [`Operation::Reparent`] keeps the *local* transform, which teleports
/// the node whenever the two parents differ — never what a layer-tree drag or
/// an agent's `reparent` means.
pub fn reparent_preserving_world(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    new_parent: NodeId,
    index: usize,
) -> Result<Transaction, OpError> {
    if !doc.contains(id) {
        return Err(OpError::NoSuchNode(id));
    }
    let world = res.world_transform(id).ok_or(OpError::NoSuchNode(id))?;
    let new_parent_world = res
        .world_transform(new_parent)
        .ok_or(OpError::NoSuchNode(new_parent))?;
    Ok(Transaction(vec![
        Operation::Reparent {
            id,
            new_parent,
            index,
        },
        Operation::SetTransform {
            id,
            transform: new_parent_world.inverse() * world,
        },
    ]))
}

/// The operations that write `local` and `kind` into `node` as its own — a
/// `SetTransform` where `local` is not its stored transform, and a `SetGeometry`
/// where `kind` is not its stored kind and has a size to write (a shape's or a
/// frame's size, a text's sizing mode). Nothing for a kind whose size is not in
/// it: a path's, a group's, an image's.
///
/// **The one spelling of "keep it where it is drawn"**, for every edit that
/// takes a layer out of whatever was placing it: the structural verbs through
/// [`baked`], and in the app a Position pin changed (`OndinApp::baked_placement`),
/// the Item card's *Unpin and return to the layout* and a container's layout set
/// to `none` (§15
/// D878) — and, its `SetGeometry` half alone, W's or H's `px` picked from the
/// sizing menu, which fixes a hugged or percentage size where it is drawn
/// (`size_mode_tx`, §15 D879). One function because the kind-to-patch table is a
/// definition of a shape's size, and a second copy would be the first to miss a
/// kind — it lived in the app's inspector until the structural verbs needed it
/// too (§15 D929), and moved rather than being copied.
pub fn baked_ops(id: NodeId, node: &crate::Node, local: Affine, kind: &NodeKind) -> Vec<Operation> {
    let mut ops = Vec::new();
    if node.transform() != local {
        ops.push(Operation::SetTransform {
            id,
            transform: local,
        });
    }
    if kind != node.kind() {
        let geometry = match kind {
            NodeKind::Rect { size, .. }
            | NodeKind::Ellipse { size }
            | NodeKind::Polygon { size, .. }
            | NodeKind::Star { size, .. }
            | NodeKind::Artboard { size } => Some(crate::GeometryPatch::Size(*size)),
            NodeKind::Text { sizing, .. } => Some(crate::GeometryPatch::TextSizing(*sizing)),
            _ => None,
        };
        if let Some(geometry) = geometry {
            ops.push(Operation::SetGeometry { id, geometry });
        }
    }
    ops
}

/// The operations that make `id` **store where it is drawn**, for a door that
/// takes it out of the layout that placed it or takes that layout away — so the
/// edit moves nothing on screen (§9's rule for structural edits; §15 D929).
///
/// An in-flow item's stored transform is not where it is drawn — it is where it
/// goes back to when it leaves (§15 D875) — and a stretched or grown item's
/// stored size is not the size it is drawn at. Group, *Frame selection*, a
/// boolean, a mask, ungroup and *Display → none* each take items out of a
/// layout, and each wrapped or spliced them by their *stored* geometry: the
/// members of a flex row grouped with Ctrl+G jumped back to wherever they were
/// created and lost their stretch. So each writes this first.
///
/// Three parts: the used transform and the used kind ([`baked_ops`]); and, for
/// a container with a layout of its own, **the size its parent's pass gave it,
/// on an axis where its own pass would give another** — once nothing lays it
/// out it is a layout root sized by its own `width` and `height`, and a grown
/// item hugging its children would shrink back to them. A frame's size is its
/// kind, so a frame there stops hugging (`fit-content` → `auto`, the kind
/// carrying the drawn size); a group with a layout has no size in its kind, so
/// it takes the drawn size in px. An axis its own pass already sizes alike is
/// left alone, so a hugging container that was not grown goes on hugging.
///
/// Empty for a layer already stored where it is drawn — every layer outside a
/// layout and outside a frame's insets — which is what keeps the doors that
/// call this unchanged on a document with no layout in it.
pub fn baked(doc: &Document, res: &Resolved, id: NodeId) -> Vec<Operation> {
    use crate::container::Dimension;
    let Some(node) = doc.get(id) else {
        return Vec::new();
    };
    let local = res.used_local(doc, id).unwrap_or(node.transform());
    let kind = res.used_kind(doc, id).unwrap_or(node.kind());
    let mut ops = baked_ops(id, node, local, kind);
    if node.display().is_none() || !is_flex_item(doc, id) {
        return ops;
    }
    let drawn = match kind {
        NodeKind::Artboard { size } => Some(*size),
        NodeKind::Group => res.used_frame(id),
        _ => None,
    };
    let view = crate::resolve::DocView(doc);
    let own = crate::container::lay_out(&view, id)
        .into_iter()
        .find(|l| l.id == id)
        .map(|l| l.size);
    if let (Some(drawn), Some(own)) = (drawn, own) {
        let near = |a: f64, b: f64| (a - b).abs() < 1.0 / 128.0;
        let mut item = *node.item();
        for (dim, d, o) in [
            (&mut item.width, drawn.width, own.width),
            (&mut item.height, drawn.height, own.height),
        ] {
            if near(d, o) {
                continue;
            }
            *dim = match kind {
                NodeKind::Artboard { .. } => Dimension::Auto,
                _ => Dimension::Px(d),
            };
        }
        if item != *node.item() {
            ops.push(Operation::SetLayoutItem { id, item });
        }
    }
    ops
}

/// [`baked`] without its `SetTransform` — for a door that writes the layer's new
/// transform itself, from the drawn one ([`frame`], [`ungroup`]).
fn baked_size(doc: &Document, res: &Resolved, id: NodeId) -> impl Iterator<Item = Operation> {
    baked(doc, res, id)
        .into_iter()
        .filter(|op| !matches!(op, Operation::SetTransform { .. }))
}

/// Where a new container lands among `siblings` once `members` have been
/// reparented into it — **the topmost member's slot** (§15 D286, D439).
///
/// The index counts the non-members *below* the topmost member, which after the
/// reparent are exactly the siblings that still sit below the container. So a
/// non-contiguous selection leaves every unselected layer where it was and puts
/// the container where the front one used to be.
///
/// ⚠️ **`rposition`, and it was `position` in four copies of these four lines.**
/// That lands the container in the **bottom-most** member's slot, which is the
/// same answer whenever the selection is contiguous — so grouping three siblings
/// `back`/`mid`/`front` by selecting `back` and `front` and leaving `mid` out
/// gave `[NEW, mid]` where the design says `[mid, NEW]`: a layer the user did not
/// select jumped in front of one they did, on the ordinary Ctrl+G. Measured on
/// all five verbs — group, frame, boolean, flatten and mask, the last by calling
/// this through `group`.
///
/// ⚠️ **The `filter` beside it was dead code, and that is the tell.** Under
/// `position` no element of `siblings[..first]` can be in `members`, by the
/// definition of `position` — so the filter could never drop anything. It is
/// only meaningful against `rposition`, which is to say **the expression was
/// written for the topmost member and half of it was wrong**. All four copies
/// carried the same dead filter, which is what four copies do.
///
/// One function now, for the reason `is_paintable` gives one file down: a rule
/// spelled four times is a rule that will be fixed three times.
fn container_slot(siblings: &[NodeId], members: &FxHashSet<NodeId>) -> Result<usize, OpError> {
    let top = siblings
        .iter()
        .rposition(|c| members.contains(c))
        .ok_or(OpError::MalformedSubtree)?;
    Ok(siblings[..top]
        .iter()
        .filter(|c| !members.contains(c))
        .count())
}

/// Wrap `members` in a new `Group`, preserving their world positions, relative
/// z-order, and the z-position of the topmost member.
///
/// Returns the transaction and the id minted for the group. Members must be
/// siblings — grouping across parents is ambiguous about which parent wins, so
/// it is rejected rather than guessed.
///
/// **Frames can be grouped** (§15 D870, D876): a row of cards is a group of
/// frames, and selecting the cards and grouping them is the gesture that makes
/// one. The group lands where the members were, so a group of frames under a
/// boolean or a mask is refused by `apply` — as is any frame there — and cannot
/// arise from this function anyway, since a boolean holds no frame to group.
///
/// The group is created with an identity transform in the shared parent's
/// space, so every member keeps its local transform and nothing moves — **its
/// drawn one** ([`baked`], §15 D929). A member laid out by a flex or grid
/// parent, or pinned by insets the group makes inert, is written where it is
/// drawn first; wrapping by the stored transforms sent a flex row's members back
/// to wherever they were created, stacked on each other and unstretched.
pub fn group(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    members: &[NodeId],
) -> Result<(Transaction, NodeId), OpError> {
    if members.is_empty() {
        return Err(OpError::MalformedSubtree);
    }
    let unique: FxHashSet<NodeId> = members.iter().copied().collect();

    // One shared parent, and nothing that may not live inside a group — which is
    // only the root now.
    let mut parent = None;
    for id in &unique {
        let node = doc.get(*id).ok_or(OpError::NoSuchNode(*id))?;
        if matches!(node.kind(), NodeKind::Root) {
            return Err(OpError::WrongKindForOp);
        }
        let p = node.parent().ok_or(OpError::CannotModifyRoot)?;
        match parent {
            None => parent = Some(p),
            Some(existing) if existing != p => return Err(OpError::InvalidParent),
            _ => {}
        }
    }
    let parent = parent.expect("non-empty member set");
    let siblings = doc.get(parent).expect("checked above").children().to_vec();

    // Members in their current z-order, so the group preserves relative order.
    let ordered: Vec<NodeId> = siblings
        .iter()
        .copied()
        .filter(|c| unique.contains(c))
        .collect();
    let final_index = container_slot(&siblings, &unique)?;

    let group_id = ids.mint();
    let mut ops = Vec::with_capacity(ordered.len() + 2);
    // Append first (a valid index regardless of how many members there are),
    // then reorder into place once the members have moved inside.
    ops.push(Operation::CreateNode {
        id: group_id,
        parent,
        index: siblings.len(),
        kind: NodeKind::Group,
        transform: None,
        name: None,
    });
    for (i, member) in ordered.iter().enumerate() {
        ops.push(Operation::Reparent {
            id: *member,
            new_parent: group_id,
            index: i,
        });
        ops.extend(baked(doc, res, *member));
    }
    ops.push(Operation::Reorder {
        id: group_id,
        index: final_index,
    });

    Ok((Transaction(ops), group_id))
}

/// Which of `members` becomes the mask: the **key** if one is designated among
/// them, else the bottom-most member that is **not a picture**, else the
/// bottom-most — among the members that **can be a mask** at all, when any can.
///
/// `None` only when `members` is empty or names nothing in the document.
///
/// **"Can be a mask" is [`can_be_mask`], and it only filters since frames could
/// be grouped** (§15 D876). A frame is not a picture, so a card sitting lowest
/// was the bottom-most shape and would have been chosen, and refused — when the
/// circle above it was the obvious mask and the card the obvious thing to crop.
/// Before, no multi-member selection with a frame in it got this far: `group`
/// refused it. When nothing can mask, the rule falls through unchanged and
/// [`mask`] reports the refusal.
///
/// **The middle clause is the one worth arguing.** A "picture" here is an
/// image-filled layer ([`crate::Paint::image_fill`]), and in this model that is a
/// *shape* with a photograph in it — so its outline is a rectangle, and using it
/// as a mask crops everything above it to a rectangle. That is almost never what
/// "mask this photo with that circle" means, and z-order alone gets it wrong
/// exactly when the photograph happens to sit lower, which is the ordinary way to
/// build the picture: drop the photo, draw a shape over it, mask.
///
/// So the rule is not "bottom-most" but "bottom-most **shape**", and it falls back
/// to plain bottom-most when every member is a picture, where there is no better
/// answer and the user has evidently asked for one photograph to crop another.
///
/// **A key beats it, and that is the point of a key.** Masking with a photograph
/// is a real thing to want — you get its box — and `Alt+Shift` is how the user
/// says they meant it, the same gesture that already overrides z-order for a
/// `Subtract`'s base operand. Refusing an image outright would be defensible and
/// is *less* good: it would make the gesture silently do nothing on the one
/// selection where the user had been most explicit.
///
/// Spelled once because two callers ask: this builder, and the inspector's
/// control, which needs the answer to say *why* it is dim.
pub fn mask_target(doc: &Document, members: &[NodeId], key: Option<NodeId>) -> Option<NodeId> {
    let unique: FxHashSet<NodeId> = members.iter().copied().collect();
    if let Some(key) = key.filter(|k| unique.contains(k)) {
        return Some(key);
    }
    // In the parent's child order, which is paint order — the selection's own
    // order is the order things were clicked in and says nothing about the stack.
    let parent = doc.get(*members.first()?)?.parent()?;
    let ordered: Vec<NodeId> = doc
        .get(parent)?
        .children()
        .iter()
        .copied()
        .filter(|c| unique.contains(c))
        .collect();
    let able: Vec<NodeId> = ordered
        .iter()
        .copied()
        .filter(|id| can_be_mask(doc, *id))
        .collect();
    let ordered = if able.is_empty() { ordered } else { able };
    let not_a_picture = |id: &&NodeId| {
        doc.get(**id)
            .is_some_and(|n| n.paint().image_fill().is_none())
    };
    ordered
        .iter()
        .find(not_a_picture)
        .or_else(|| ordered.first())
        .copied()
}

/// Whether `id` may take [`Operation::SetMask`]` { mask: true }`: its kind can
/// mask ([`NodeKind::can_mask`]), and nothing inside it is a frame, since a mask's
/// outline is its contents unioned and a page is not an outline (§15 D876).
/// The same two refusals `Document::apply` makes, asked in advance.
pub fn can_be_mask(doc: &Document, id: NodeId) -> bool {
    doc.get(id).is_some_and(|n| n.kind().can_mask()) && !doc.holds_a_frame(id)
}

/// *Use as mask* — make one of `members` the mask for the rest, **wrapping them
/// in a group** when there is more than one.
///
/// Returns the transaction and the id the caller should select.
///
/// **The group is the feature, not tidiness.** A mask and what it masks stay
/// independent layers, so without it the result of the operation is a set the
/// user has to re-select by hand every time they want to move the picture and its
/// mask together — and nothing in the layer list says which of the two is which
/// beyond a badge. Wrapping them answers both: one thing to drag, and a container
/// whose contents are exactly the mask and its subjects.
///
/// **One member is the exception and takes no group**, because there is nothing to
/// wrap it with: a lone layer masks the siblings it already has, which is what the
/// run rule says and what [`crate::Document::governing_mask`] reads. Grouping it
/// alone would produce a group of one — the shape `group` itself refuses — and
/// would change *which* layers it masks, a group being a new parent.
///
/// **[`mask_target`] picks which member becomes the mask** — the key if there is
/// one, else the bottom-most member that is not a picture, among those that
/// [`can_be_mask`] when any can (§15 D876). Whatever it picks moves
/// to index 0 of the new group and everything else keeps its relative z-order: the
/// smallest rearrangement that satisfies the choice, which is the rule [`boolean`]
/// follows for a `Subtract`'s base operand.
///
/// A mask clips what is drawn *above* it, so "the mask" and "the bottom of the
/// stack" are one position — which is why designating one is a **reorder** rather
/// than a flag on the selection.
///
/// ⚠️ **These twenty-seven lines sat on [`mask_target`] until 2026-09-06**, above
/// that function's own summary and describing a transaction it cannot return. The
/// merged run was 54 lines, and it is the run **CLAUDE.md's own doc-comment-theft
/// ranking carried as one of its five *excused* entries** — so the one block a
/// sweep was told not to investigate was the one that needed it. An entry earns
/// that list by having its run read against the item beneath it, which had
/// evidently not been done here.
pub fn mask(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    members: &[NodeId],
    key: Option<NodeId>,
) -> Result<(Transaction, NodeId), OpError> {
    let unique: FxHashSet<NodeId> = members.iter().copied().collect();
    if unique.is_empty() {
        return Err(OpError::MalformedSubtree);
    }
    // One shared parent — asked here rather than left to `group`, because the
    // choice of mask is made out of the parent's child order and there is no such
    // order across two parents.
    let mut parent = None;
    for id in &unique {
        let node = doc.get(*id).ok_or(OpError::NoSuchNode(*id))?;
        let p = node.parent().ok_or(OpError::CannotModifyRoot)?;
        match parent {
            None => parent = Some(p),
            Some(existing) if existing != p => return Err(OpError::InvalidParent),
            _ => {}
        }
    }
    let parent = parent.expect("non-empty member set");
    // **Nothing masks inside a boolean** — its children are operands rather than
    // artwork, so `governing_mask` answers `None` there and the flag would take and
    // do nothing. Refused here so the entrance that does not go through the
    // inspector's own gate — the chord — does not get a silent no-op.
    if matches!(
        doc.get(parent).map(|n| n.kind()),
        Some(NodeKind::Boolean { .. })
    ) {
        return Err(OpError::WrongKindForOp);
    }
    let ordered: Vec<NodeId> = doc
        .get(parent)
        .ok_or(OpError::NoSuchNode(parent))?
        .children()
        .iter()
        .copied()
        .filter(|c| unique.contains(c))
        .collect();
    let bottom = *ordered.first().ok_or(OpError::MalformedSubtree)?;
    let chosen = mask_target(doc, members, key).ok_or(OpError::MalformedSubtree)?;
    if !doc
        .get(chosen)
        .ok_or(OpError::NoSuchNode(chosen))?
        .kind()
        .can_mask()
    {
        return Err(OpError::WrongKindForOp);
    }
    // A group with a frame in it (§15 D876). `apply` refuses the flag too; asked
    // here so the refusal comes before a group is planned around it.
    if doc.holds_a_frame(chosen) {
        return Err(OpError::ArtboardPlacement);
    }

    if unique.len() == 1 {
        // A mask leaves its container's flow (`container::in_flow`), so a lone
        // in-flow member is written where it is drawn first (§15 D929).
        let mut ops = baked(doc, res, chosen);
        ops.push(Operation::SetMask {
            id: chosen,
            mask: true,
        });
        return Ok((Transaction(ops), chosen));
    }

    // `group` owns the rest of the admission test — no root, a parent that can
    // hold a group — and the placement rule that keeps the result where
    // the topmost member was. Reusing it rather than repeating it is what stops the
    // two verbs drifting about where a new container lands.
    let (Transaction(mut ops), group_id) = group(doc, res, ids, members)?;
    if chosen != bottom {
        ops.push(Operation::Reorder {
            id: chosen,
            index: 0,
        });
    }
    ops.push(Operation::SetMask {
        id: chosen,
        mask: true,
    });
    Ok((Transaction(ops), group_id))
}

/// Wrap `members` in a new **frame** ([`NodeKind::Artboard`]) sized to their union,
/// preserving their world positions, relative z-order, and the z-position of the
/// topmost member ([`container_slot`], §15 D286).
///
/// ⚠️ **This cited D249 for the placement rule and D249 does not contain it.**
/// That entry is about *why frame is `group`'s sibling rather than a flag on it*
/// — the box, the parent predicate, whether frames may be members — and says
/// nothing about z-position. The rule is D286's, and the code behind it was
/// wrong for as long as the citation was (D439).
///
/// Returns the transaction and the id minted for the frame. [`group`]'s sibling,
/// and deliberately a second function rather than a flag on it, because two of
/// the rules differ and each difference is the frame having a *box* where a group
/// has none:
///
/// - **The frame has a size and a position, so it has to be measured.** A group is
///   created with the identity transform and every member keeps its local transform
///   untouched; a frame is created at the union's corner, so every member is moved
///   back by exactly that corner. Both are one statement — *nothing moves* — and
///   they need opposite amounts of work to hold.
/// - **The parent has to accept a frame.** §5.3 lets an `Artboard` hang off the root,
///   another `Artboard` or a group, and never sit under a boolean or a mask (§15
///   D876), so a selection among a boolean's operands, or anywhere under a mask,
///   cannot be framed where it stands. Refused with [`OpError::ArtboardPlacement`]
///   rather than framed somewhere else, because moving the artwork out to satisfy
///   the frame is a second edit the user did not ask for.
///
/// A third difference was listed here until frames could be grouped (§15 D876):
/// *"frames may be members, where [`group`] refuses them"*. Both take them now.
///
/// The correction is a pure translation: the frame and its members-to-be are
/// siblings for the length of the arithmetic, so a member's new local transform
/// is `translate(−origin) · local` in the one space both are already expressed
/// in. `Resolved` is needed to *measure* — a text node's box comes from its
/// shaped layout and a container's from its children — and for `local` itself,
/// which is the member's **used** transform: a flex or grid item's stored one is
/// not where it is drawn, and its stretched size is written as its own too
/// ([`baked`], §15 D929).
///
/// The frame is created with **no background**, which is the rule that framing
/// changes nothing you can see: a fill behind the selection would hide whatever the
/// selection was sitting on, and the inspector's *Fill* card — a frame's own, named
/// like every other layer's since §15 D397 — is one click away for anyone who wants
/// one. It does not clip either, and that costs nothing
/// here — the box is the union, so there is nothing outside it to clip.
///
/// [`can_frame`] is the question a menu row asks before offering this.
pub fn frame(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    members: &[NodeId],
) -> Result<(Transaction, NodeId), OpError> {
    if members.is_empty() {
        return Err(OpError::MalformedSubtree);
    }
    let unique: FxHashSet<NodeId> = members.iter().copied().collect();

    // One shared parent, and nothing that cannot be moved at all. An `Artboard`
    // member is admitted — a frame nests in a frame, as it groups (§15 D876) — so
    // the only kind refused is the root, which `parent()` already answers for.
    let mut parent = None;
    for id in &unique {
        let node = doc.get(*id).ok_or(OpError::NoSuchNode(*id))?;
        if matches!(node.kind(), NodeKind::Root) {
            return Err(OpError::WrongKindForOp);
        }
        let p = node.parent().ok_or(OpError::CannotModifyRoot)?;
        match parent {
            None => parent = Some(p),
            Some(existing) if existing != p => return Err(OpError::InvalidParent),
            _ => {}
        }
    }
    let parent = parent.expect("non-empty member set");
    let parent_node = doc.get(parent).ok_or(OpError::NoSuchNode(parent))?;
    // §5.3's placement rule, asked *before* anything is planned so the failure is
    // the one the caller can explain — `can_parent` is the same predicate
    // `Document::apply` would refuse this with three operations later.
    if !can_parent(
        parent_node.kind(),
        &NodeKind::Artboard {
            size: kurbo::Size::ZERO,
        },
    ) || !doc.frame_may_sit_under(parent)
    {
        return Err(OpError::ArtboardPlacement);
    }
    let siblings = parent_node.children().to_vec();

    let ordered: Vec<NodeId> = siblings
        .iter()
        .copied()
        .filter(|c| unique.contains(c))
        .collect();
    let final_index = container_slot(&siblings, &unique)?;

    // The union **in the shared parent's space**, which is the space the frame's
    // own transform is expressed in. Each member's own box through its own local
    // transform — not `world_bounds`, which would have to be projected back and
    // would pick up an upright axis-aligned box for a rotated member on the way.
    // **Through its used transform**, where it is drawn (§15 D929): the box is
    // already the used one, and a flex item's stored transform is not its place.
    let union = ordered
        .iter()
        .filter_map(|id| {
            let local = crate::local_box(doc, res, *id)?;
            Some(crate::geometry::transform_rect(
                res.used_local(doc, *id)?,
                local,
            ))
        })
        .reduce(|a, b| a.union(b))
        .ok_or(OpError::MalformedSubtree)?;
    let origin = union.origin();
    // The floor `create_artboard` uses, for the case its comment exists for: a
    // selection of one zero-extent line has a degenerate union, and a frame with no
    // extent is a frame nothing can be dropped into or resized from.
    let size = kurbo::Size::new(union.width().max(1.0), union.height().max(1.0));

    let frame_id = ids.mint();
    let mut ops = Vec::with_capacity(ordered.len() * 2 + 2);
    // Appended first and reordered last, as `group` does: an index computed against
    // the sibling list is only valid while that list is the one being counted.
    ops.push(Operation::CreateNode {
        id: frame_id,
        parent,
        index: siblings.len(),
        kind: NodeKind::Artboard { size },
        transform: Some(Affine::translate(origin.to_vec2())),
        name: None,
    });
    for (i, member) in ordered.iter().enumerate() {
        let local = res
            .used_local(doc, *member)
            .ok_or(OpError::NoSuchNode(*member))?;
        ops.push(Operation::Reparent {
            id: *member,
            new_parent: frame_id,
            index: i,
        });
        ops.push(Operation::SetTransform {
            id: *member,
            transform: Affine::translate(-origin.to_vec2()) * local,
        });
        ops.extend(baked_size(doc, res, *member));
    }
    ops.push(Operation::Reorder {
        id: frame_id,
        index: final_index,
    });

    Ok((Transaction(ops), frame_id))
}

/// Whether [`frame`] would accept `members` — one shared parent that can hold a
/// frame, with no boolean or mask above it, and nothing that cannot be moved.
///
/// **The builder's own question, so a row cannot offer what the verb would
/// refuse** (`context-menus.md` §3). It is the same rule `outlineable` follows and
/// the reason `can_outline` exists: a menu that dims with a sentence is a menu you
/// can learn from, and one that fails afterwards is one you cannot.
///
/// It deliberately does **not** ask whether the members' boxes resolve — that needs
/// a [`Resolved`] and it is not a thing a user can act on. A selection with no
/// measurable extent at all is [`frame`]'s error to report.
pub fn can_frame(doc: &Document, members: &[NodeId]) -> bool {
    let mut parent = None;
    for id in members {
        let Some(node) = doc.get(*id) else {
            return false;
        };
        // ⚠️ **Expressive rather than load-bearing, and measured** (§15 D661,
        // `[S3.1-L6-05]`): `Root` is the only node in a `Document` with no parent
        // — `Document::new` is the one place a `Node` is built with
        // `parent: None` — so the `let else` below already refuses it, and
        // deleting this arm leaves `tests/build.rs`'s whole `can_frame` contract
        // green. Kept because a reader asking *"can you frame the root?"* should
        // find the answer here rather than infer it from a field.
        if matches!(node.kind(), NodeKind::Root) {
            return false;
        }
        let Some(p) = node.parent() else {
            return false;
        };
        if *parent.get_or_insert(p) != p {
            return false;
        }
    }
    let Some((parent_id, parent)) = parent.and_then(|p| Some((p, doc.get(p)?))) else {
        return false;
    };
    can_parent(
        parent.kind(),
        &NodeKind::Artboard {
            size: kurbo::Size::ZERO,
        },
    ) && doc.frame_may_sit_under(parent_id)
}

/// Wrap `members` in a [`NodeKind::Boolean`] container performing `op`.
///
/// The same shape as [`group`] — one new container, the members reparented into
/// it in their existing z-order, the container reordered into the topmost member's
/// slot — with three differences that are the operation's own:
///
/// - **Two members minimum.** A boolean of one shape is that shape, so the control
///   is disabled rather than producing a container that does nothing.
/// - **Text is refused.** A boolean's operands are outlines and a text node has
///   none: `resolve::operand_path` answers `None` for it, so including it would
///   silently swallow the text into a shape it contributes nothing to. **There is a
///   route now, and it is the user's rather than this function's** — [`outline_text`]
///   (*Convert to path*, §15 D260) turns the node into a `Path` of its glyph
///   contours, which is an ordinary operand. This said "a feature that does not
///   exist yet" until 2026-08-20, which was true when written and outlived the
///   conversion landing; the *refusal* is unchanged, and deliberately, because
///   converting silently on the user's behalf would take a one-way door for them.
/// - **The result inherits the bottom member's paint.** Figma's behaviour, and the
///   alternative is worse than it sounds: a fresh container has no fills, so the
///   boolean would evaluate correctly and then draw nothing at all, which reads as
///   the operation having deleted the artwork.
///
/// `key`, when given, is the member to place at the **bottom** — the base
/// `Subtract` cuts into, and therefore also the paint the container inherits.
///
/// **It applies to `Subtract` and to nothing else, on purpose.** Union, Intersect
/// and Exclude all fold commutatively (`exclude_gives_the_same_answer_in_any_operand_order`
/// measures the one that reads as though it should not), so reordering their
/// operands would change the layer list and the inherited paint while leaving the
/// shape identical — a designation that appeared to do something the arithmetic
/// cannot see. Where the key means nothing, it does nothing.
///
/// **Creation-time ordering, not a stored property.** Once the container exists,
/// operand order *is* child order and the only way to change it is to reorder —
/// visible in the tree, undoable, draggable. A persistent "primary" index beside
/// it would be a second answer to one question, and the failure mode is dragging a
/// layer to the bottom and watching nothing happen because a marker you cannot see
/// disagrees.
pub fn boolean(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    members: &[NodeId],
    op: crate::node::BoolOp,
    key: Option<NodeId>,
) -> Result<(Transaction, NodeId), OpError> {
    let unique: FxHashSet<NodeId> = members.iter().copied().collect();
    if unique.len() < 2 {
        return Err(OpError::WrongKindForOp);
    }

    let mut parent = None;
    for id in &unique {
        let node = doc.get(*id).ok_or(OpError::NoSuchNode(*id))?;
        if matches!(
            node.kind(),
            NodeKind::Artboard { .. } | NodeKind::Root | NodeKind::Text { .. }
        ) {
            return Err(OpError::WrongKindForOp);
        }
        // A group with a frame in it would carry the frame into an operand
        // (§15 D876); `apply` refuses the reparent, and this refuses first.
        if doc.holds_a_frame(*id) {
            return Err(OpError::ArtboardPlacement);
        }
        let p = node.parent().ok_or(OpError::CannotModifyRoot)?;
        match parent {
            None => parent = Some(p),
            Some(existing) if existing != p => return Err(OpError::InvalidParent),
            _ => {}
        }
    }
    let parent = parent.expect("non-empty member set");
    let siblings = doc.get(parent).expect("checked above").children().to_vec();

    // Members in their current z-order, which is the operand order: index 0 is the
    // bottom of the stack, and `Subtract` takes the bottom one as its base.
    let mut ordered: Vec<NodeId> = siblings
        .iter()
        .copied()
        .filter(|c| unique.contains(c))
        .collect();
    // The key goes to the bottom, and the rest keep their z-order relative to each
    // other — the smallest rearrangement that satisfies the designation, so a
    // three-operand subtract still cuts in the order the layer list showed.
    if op == crate::node::BoolOp::Subtract
        && let Some(key) = key
        && let Some(at) = ordered.iter().position(|c| *c == key)
    {
        let key = ordered.remove(at);
        ordered.insert(0, key);
    }
    let final_index = container_slot(&siblings, &unique)?;

    let bool_id = ids.mint();
    let mut ops = Vec::with_capacity(ordered.len() + 4);
    ops.push(Operation::CreateNode {
        id: bool_id,
        parent,
        index: siblings.len(),
        kind: NodeKind::Boolean { op },
        transform: None,
        name: None,
    });
    // **Nothing sets a fill rule here, and that is deliberate** (§15 D239). An
    // `Exclude` is even-odd because that is what a symmetric difference *is*, so
    // `Node::fill_rule` derives it from the operation rather than storing it — which
    // is also what keeps every `.ondin` written before the rule existed correct
    // without a migration.
    // The bottom member's paint, read before anything moves.
    let base = doc.get(ordered[0]).expect("checked above");
    ops.extend(carried_paint(base, bool_id));
    for (i, member) in ordered.iter().enumerate() {
        ops.push(Operation::Reparent {
            id: *member,
            new_parent: bool_id,
            index: i,
        });
        // Operands are drawn where they were — [`group`]'s rule (§15 D929).
        ops.extend(baked(doc, res, *member));
    }
    ops.push(Operation::Reorder {
        id: bool_id,
        index: final_index,
    });
    Ok((Transaction(ops), bool_id))
}

/// Change which operation a `Boolean` container performs, leaving its children
/// and its paint alone.
///
/// A kind edit rather than a rebuild, so switching Union → Subtract on a boolean
/// the user has already styled keeps the styling — and is one undo step.
pub fn set_boolean_op(
    doc: &Document,
    id: NodeId,
    op: crate::node::BoolOp,
) -> Result<Transaction, OpError> {
    let node = doc.get(id).ok_or(OpError::NoSuchNode(id))?;
    match node.kind() {
        NodeKind::Boolean { op: current } if *current == op => Ok(Transaction(Vec::new())),
        NodeKind::Boolean { .. } => {
            let mut ops = vec![Operation::SetGeometry {
                id,
                geometry: crate::GeometryPatch::BoolOp(op),
            }];
            // **No `SetFillRule` here either**: the rule follows the operation
            // because `Node::fill_rule` derives it, so switching to or from
            // `Exclude` changes how the outline is read with no second op to keep
            // in step — and no way for the two to disagree after an undo.
            // **Rename only a name we gave it.** A layer created by this builder is
            // called after its operation, so leaving it alone means a row reading
            // "Union" over an intersection. But the name is the *user's* the moment
            // they touch it, and silently overwriting "Badge cutout" would be worse
            // than a stale label.
            //
            // **The test had to widen when names got numbered** (§15 D127). It compared
            // the name against the four labels exactly, which was right while the
            // first boolean in a frame was called plainly "Union" — and wrong the
            // moment it became "Union 1", because then switching its operation left
            // a row reading "Union 1" over an intersection and nothing said why.
            // `naming::is_boolean_label` is the widened question and deliberately
            // still the *strict* one: "Badge 2" is numbered but is not a name we
            // ever wrote, so it stays theirs.
            if crate::naming::is_boolean_label(node.name()) {
                // The number comes across when there is one, so flipping the
                // operation back and forth does not walk it — and `free_name` moves
                // off it only if the new label at that number is already taken. A
                // name with no number is one from a document saved before the
                // numbering; it joins the series at 1.
                //
                // `except` is the container itself: without it, its own old name
                // reads as a sibling's and pushes the new one a number along.
                let parent = node.parent().unwrap_or(doc.root());
                let taken = doc.child_names(parent, &[id]);
                let from = crate::naming::numbered(node.name()).map_or(1, |(_, n)| n);
                ops.push(Operation::SetName {
                    id,
                    name: crate::naming::free_name(&taken, op.label(), from),
                });
            }
            Ok(Transaction(ops))
        }
        _ => Err(OpError::WrongKindForOp),
    }
}

/// Replace a selection with the single `Path` its outlines add up to (§9.4).
///
/// **The one destructive shape operation, and the counterpart to [`ungroup`].** A
/// boolean container keeps its operands so the operation can be changed or undone;
/// flattening throws them away and keeps the answer. That is worth having for the
/// same reason the container is: a shape that took four operands to describe costs
/// four evaluations every time anything above it moves, and once it is finished
/// there is nothing left to re-decide.
///
/// Two selections reach it, and they differ in what "the outline" means:
///
/// - **One boolean.** Its derived outline, in its own local space, so the result
///   sits exactly where the boolean drew and keeps its transform, paint, name and
///   opacity. Nothing about the picture changes; only the layers under it go away.
/// - **Two or more layers.** Their *union*, in the parent's space — the same answer
///   `boolean(Union)` then flattening would give, in one step and one undo. Groups
///   in the selection contribute their contents unioned and booleans their results,
///   because [`Resolved::operand_path`] is what is asked of each member.
///
/// A single layer that is *not* a boolean is refused rather than converted, and it
/// still is: turning a rect into a path is a **different operation** and it has its
/// own verb now, [`outline`] (§15 D230). This entry point's meaning is unchanged —
/// one boolean, or two or more of anything — which is the whole reason the two are
/// separate: *Flatten* on one rect would have to mean something the word does not
/// say, and the row a user reached for would do it silently.
///
/// Text is refused for [`boolean`]'s reason: it has no outline until its glyphs are
/// converted. `Artboard` and `Root` are refused because neither is a shape that can
/// be replaced by one.
pub fn flatten(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    members: &[NodeId],
) -> Result<(Transaction, NodeId), OpError> {
    match outermost(doc, members).as_slice() {
        [] => Err(OpError::WrongKindForOp),
        [one] => flatten_boolean(doc, res, ids, *one),
        many => flatten_union(doc, res, ids, many),
    }
}

/// One boolean, replaced in place by its own result.
fn flatten_boolean(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    id: NodeId,
) -> Result<(Transaction, NodeId), OpError> {
    let node = doc.get(id).ok_or(OpError::NoSuchNode(id))?;
    if !matches!(node.kind(), NodeKind::Boolean { .. }) {
        return Err(OpError::WrongKindForOp);
    }
    // **The same two `None`s as `flatten_union`'s, asked of the cache instead of the
    // counter** (§15 D736, `[S10.1-L2-06]`). Here the mark already exists: §15 D298
    // has `Resolved` bracket every `evaluate_node` and record the id in `failed`, so
    // a `Boolean` with no cached outline is an abandoned one exactly when the layers
    // row is already wearing its warning colour. Reading the mark rather than the
    // counter is also the *correct* question at this door — the cache was filled on
    // some earlier pass, so this thread's counter has nothing to say about it.
    let path = match res.boolean_path(id).cloned() {
        Some(path) => path,
        None if res.boolean_failed(id) => return Err(OpError::BooleanAbandoned),
        None => return Err(OpError::EmptyGeometry),
    };
    replace_with_path(doc, ids, id, path)
}

/// Whether [`outline`] would produce a layer *different* from the one it replaces.
///
/// **The kind gate and the no-op check in one answer**, because they are one
/// question: a rect, an ellipse, a polygon, a star and a line each have an outline
/// that is not already a `Path`, and a `Path` has one only while its corner radii
/// are still in the model rather than in its geometry (§15 D119).
///
/// The four refusals each have a different reason and none of them is an oversight:
/// a **`Boolean`** is [`flatten`]'s, a **`Group`** has no outline of its own (the
/// union of its contents is what `flatten` gives two or more layers, which is a
/// different verb from outlining *this* shape), an **`Artboard`** is a page rather
/// than a shape that can be replaced by one, and **`Text`** has no outline until its
/// glyphs are converted — which is [`outline_text`], a third verb rather than an
/// arm of this one, because a text node's outline is a *shaped* thing and needs
/// the resolved layout where every other kind's is a pure function of its own
/// geometry (§15 D260).
///
/// A caller offering this as a menu row wants both halves: presence from the kind,
/// and dimming from this. `docs/context-menus.md` §3's rule, applied to the one kind
/// where the two differ.
pub fn can_outline(kind: &NodeKind) -> bool {
    match kind {
        NodeKind::Rect { .. }
        | NodeKind::Ellipse { .. }
        | NodeKind::Polygon { .. }
        | NodeKind::Star { .. }
        | NodeKind::Line { .. } => true,
        // `round_corners` returns the path untouched when **no** radius is `> 0`,
        // so this is exactly "baking the radii would change something".
        //
        // 🚨 **The two predicates were not each other's negation until §15 D640,
        // and the divergence was user-visible.** This arm has always asked
        // `any(> 0)`; `round_corners`' fast path asked `all(<= 0)`, and `NaN`
        // fails both comparisons — so a `NaN` list dimmed this menu row for
        // having nothing to bake **while the canvas drew the shape rounded to
        // its inscribed circle**. The comment that stood here said `all(<= 0)`
        // and was the sentence that would have tempted the next reader to
        // "simplify" the guard back to the broken spelling. `arch-scribe` found
        // it by reading D640's brief against the code; nothing else could.
        NodeKind::Path { corner_radii, .. } => corner_radii.iter().any(|r| *r > 0.0),
        NodeKind::Root
        | NodeKind::Group
        | NodeKind::Artboard { .. }
        | NodeKind::Text { .. }
        | NodeKind::Boolean { .. } => false,
    }
}

/// Replace one shape with the `Path` its own outline traces — *Outline shape*
/// (§9.4, §15 D230).
///
/// **The counterpart to [`flatten`] and deliberately not part of it.** Flatten means
/// "throw the operands away and keep the answer"; this means "stop describing this
/// shape parametrically and start describing it as anchors". The two share a body
/// ([`replace_with_path`]) and nothing else, and keeping the *words* apart is the
/// point: a *Flatten* that quietly outlined a lone rect would be the row a user
/// reached for doing something the word does not say.
///
/// **It was refused until 2026-08-19, and the reason expired rather than being
/// wrong.** Converting a rect traded its W/H and corner fields for a path nothing
/// could edit; anchor editing and per-anchor radii both exist now (§15 D118, D119),
/// so the trade is W/H for anchors — which is the trade the operation *is*.
///
/// **It takes a [`Resolved`] for one lookup**, the kind the node is *sized* with
/// (§15 D930): `geometry::local_path` is a pure function of a kind, but a shape a
/// flex or grid container stretched, or two insets pinned on both sides, is drawn
/// at a size its stored kind does not hold — and outlining the stored kind cut a
/// path the size the shape had before the layout, which a path, unable to stretch,
/// then kept. This said *"It needs no `Resolved`"* until container layout made
/// the used size differ from the stored one.
///
/// The radii are **not** carried onto the result: for a `Path` they have just been
/// baked into the geometry, and for a rect they were never a path's kind of radius —
/// `NodeKind::Path::corner_radii` is indexed by *anchor*, and a rounded rect's
/// outline has no anchors at its corners any more, only the arcs that replaced them.
pub fn outline(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    id: NodeId,
) -> Result<(Transaction, NodeId), OpError> {
    let node = doc.get(id).ok_or(OpError::NoSuchNode(id))?;
    if !can_outline(node.kind()) {
        return Err(OpError::WrongKindForOp);
    }
    let kind = res.used_kind(doc, id).unwrap_or(node.kind());
    let path = crate::geometry::local_path(kind).ok_or(OpError::EmptyGeometry)?;
    replace_with_path(doc, ids, id, path)
}

/// Replace a `Text` layer with the `Path` its glyphs trace — *Convert to path*
/// (`docs/context-menus.md` §5.6).
///
/// **A third verb rather than an arm of [`outline`], and the reason is the one
/// [`can_outline`] gave for refusing text**: every other kind's outline is a pure
/// function of its own geometry, and a text node's is a *shaped* thing. So this
/// takes a [`Resolved`] for the shaping, where `outline` takes one only to look up
/// the size a shape is drawn at (§15 D930) — the difference in cost that separates
/// [`flatten`] from `outline`, arriving for the same reason at a third door.
///
/// **The glyph outlining is not new and is not here.** `text::outline` has existed
/// since outside-aligned type strokes did (§15 D145): it walks the layout's runs,
/// pulls each glyph's contours out of `skrifa` at the run's own size and variable
/// instance, and places them at the glyph's own coordinates. Those are already the
/// node's local coordinates — the same ones the render walk hands a backend — so
/// there is nothing to transform on the way out. `context-menus.md` §5.6 scored
/// this as a *feature* because "glyph outlines have to come out of `skrifa`", and
/// by the time anyone came to build it they already had.
///
/// **A one-way door, and no confirmation.** The text is gone from the model: what
/// is left is anchors, and no amount of editing turns them back into a string.
/// Undo is the way back and is enough — it is what Illustrator, Affinity and
/// Figma all rely on for the same conversion — but a caller should say so in the
/// status line, because a layer that still *looks* like a word is exactly the one
/// whose loss is not visible.
///
/// **Empty content is refused rather than producing an empty path.** A text node
/// with nothing typed in it has no contours, and replacing it with a `Path`
/// holding nothing would leave a layer that draws nothing and can never be
/// selected on the canvas again — `EmptyGeometry` is the same answer `flatten`
/// gives operands that cancel out.
pub fn outline_text(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    id: NodeId,
) -> Result<(Transaction, NodeId), OpError> {
    let node = doc.get(id).ok_or(OpError::NoSuchNode(id))?;
    if !matches!(node.kind(), NodeKind::Text { .. }) {
        return Err(OpError::WrongKindForOp);
    }
    let layout = res.text_layout(id).ok_or(OpError::EmptyGeometry)?;
    let path = crate::text::outline(layout);
    if path.elements().is_empty() {
        return Err(OpError::EmptyGeometry);
    }
    replace_with_path(doc, ids, id, path)
}

/// Whether [`outline_text`] would produce anything — the row's dimming, and the
/// builder's own precondition asked without building it.
///
/// **Not just "is it text".** A text node with nothing typed, or one whose every
/// character is a space, shapes to a layout with no contours in it; the row has to
/// be dim there rather than offering a conversion that answers `EmptyGeometry`.
/// Asking `text::outline` is the honest way to know, and it is what the verb
/// itself asks — the alternative, testing the *content* for emptiness, is a
/// different question that agrees with this one most of the time.
pub fn can_outline_text(res: &Resolved, id: NodeId) -> bool {
    res.text_layout(id)
        .is_some_and(|l| !crate::text::outline(l).elements().is_empty())
}

/// Make a **new, empty** text node set along `path`, consuming it — the Text
/// tool's click on an outline (§15 D408).
///
/// **The only way type gets onto a curve** (§15 D409). There was a second, taking
/// an existing text layer and a shape selected together through a context-menu
/// row; the gesture replaced it — *"we don't even need the menu when selecting a
/// text and a path, it's redundant and the click-on-path affordance is what people
/// know"* — and it is gone rather than kept as a quieter door. The rail is
/// consumed the same way it always was, and *Detach from path* is the way back.
///
/// **The new node takes the path's own transform, so the rail needs no rebasing.**
/// `text_on_path` has to carry a curve from one node's space into another's; here
/// the text node is *created* in the space the curve is already in, which is both
/// simpler and exact — there is no inverse to get wrong.
///
/// ⚠️ **It goes in the consumed layer's own slot**, not on top of its parent's
/// children. A rail is usually drawn as part of an arrangement, and a click that
/// silently sent the result to the front would reorder the drawing as a side
/// effect of typing on it.
pub fn text_on_new_path(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    path: NodeId,
    parts: &crate::text::TextParts,
    at_world: Point,
) -> Result<(Transaction, NodeId), OpError> {
    let node = doc.get(path).ok_or(OpError::NoSuchNode(path))?;
    // **Nothing becomes a rail from inside a boolean** — the same refusal `mask`
    // makes (grep *"Nothing masks inside a boolean"*, some six hundred lines up in
    // this file), in almost the same words and for the same reason: a `Boolean`'s
    // children are operands rather than artwork (§15 D453). ⚠️ This said *"forty
    // lines up"* and the pointer is the whole value of the sentence.
    //
    // ⚠️ **This function consumes the layer it is handed**, which is what makes
    // the missing gate data loss rather than a no-op. Measured: two rects
    // combined with `Subtract` put the subtrahend's edge exactly on the result's
    // silhouette — that is what a boolean *is* — so a Text-tool click on that
    // edge picked the *operand*, deleted it, and installed a `Text` node inside
    // the `Boolean`. Three things at once, none announced: the boolean silently
    // became its base, the text was parented somewhere `scene.rs` returns before
    // ever reaching, and the status line said *"the shape became the rail"*. The
    // caret was live over a blank page and `Ctrl+Z` was the only way back.
    //
    // Refused here rather than only at the pick, because this is the door every
    // caller comes through and `can_parent(Boolean, Text)` is `true`, so
    // `Document::apply` has no reason of its own to say no.
    if matches!(
        node.parent().and_then(|p| doc.get(p)).map(|n| n.kind()),
        Some(NodeKind::Boolean { .. })
    ) {
        return Err(OpError::WrongKindForOp);
    }
    // **And nothing becomes a rail that is a page** — `[S3.1-L2-03]`, and the
    // half of D453 the ancestry test above does *not* reach: a frame's parent is
    // never a boolean (§15 D876), so a frame passes straight through it.
    //
    // ⚠️ **`local_path` answering `Some` is not a licence to consume.** It says
    // `Some` for an `Artboard` deliberately — *"a frame has an outline, and it is
    // its own frame"* (§15 D144) — and this function read "has a local path" as
    // "may be deleted", which is true of every other kind and false of this one.
    // Measured: a frame nested in a frame, carrying one guide scoped to it, came
    // back `Ok`; applied, the frame was gone and the guide was not, because the
    // emitted `DeleteNode` carries no [`guides_of`] ops. `io::save` succeeded and
    // `io::load` answered *"guide … is scoped to …, which is not a frame in this
    // document"* — forever.
    //
    // **Refused here rather than left to `outline_at`'s kind skip**, which is
    // what made this unreachable from a click (D408: a frame is a page, its edge
    // is a boundary rather than a drawn curve) and which is one layer above the
    // builder that needs it. Unreachable is not refused: the MCP write surface,
    // the command line and any second entrance added later all arrive here
    // directly — which is `mask`'s own argument for its refusal, quoted above.
    if matches!(node.kind(), NodeKind::Artboard { .. } | NodeKind::Root) {
        return Err(OpError::WrongKindForOp);
    }
    // **And nothing becomes a rail that is a mask** — `[S4.1-L1-02]`, and the
    // third role in a row that reaches this function as an ordinary kind
    // (§15 D456). Consuming a mask is the one of the three that changes what
    // *other* layers look like: measured on the suite's `mask_fixture`, the mask
    // was deleted, the flag did not travel to the `Text` that replaced it, and
    // the group's bounds went `50×50` → `-11,-11..500,400` — the artwork it was
    // hiding fully visible, with the status line saying *"the shape became the
    // rail"*.
    //
    // **The belt to `query::outline_at`'s braces.** That query no longer offers a
    // mask, which is what a click can reach; this is what the MCP write surface
    // and the command line reach, and it is the same argument as the two guards
    // above.
    if node.mask() {
        return Err(OpError::WrongKindForOp);
    }
    let rail = crate::geometry::local_path(node.kind()).ok_or(OpError::EmptyGeometry)?;
    if rail.elements().is_empty() {
        return Err(OpError::EmptyGeometry);
    }
    // **The type starts where the pointer was** (§15 D409). Without this it always
    // began at the path's own first point, which for a triangle drawn from any
    // corner is some *other* corner — reported as clicking the base and getting
    // the text on the right-hand edge.
    //
    // ⚠️ **Through the *world* transform, not `node.transform()`.** The click
    // arrives in world space and the rail lives in the path node's own; the node's
    // own transform is only local-to-parent, so using it puts the offset out by
    // every ancestor's placement — correct at the root and wrong inside any frame,
    // which is where every real drawing is. That is the arithmetic the verb this
    // replaced warned about, arriving from the other direction.
    let local = res
        .world_transform(path)
        .ok_or(OpError::NoSuchNode(path))?
        .inverse()
        * at_world;
    let offset = crate::text::PathWarp::bare(&rail)
        .filter(|w| w.length() > 0.0)
        .map_or(0.0, |w| w.nearest(local).0 / w.length());
    let parent = node.parent().ok_or(OpError::CannotModifyRoot)?;
    let siblings = doc
        .get(parent)
        .ok_or(OpError::NoSuchNode(parent))?
        .children();
    let at = siblings
        .iter()
        .position(|c| *c == path)
        .ok_or(OpError::MalformedSubtree)?;

    let id = ids.mint();
    Ok((
        Transaction(vec![
            Operation::CreateNode {
                id,
                parent,
                // Appended then reordered into the consumed layer's slot once it is
                // gone, which is `replace_with_path`'s rule and for its reason:
                // `siblings.len()` is the one index always in range.
                index: siblings.len(),
                kind: NodeKind::Text {
                    content: String::new(),
                    style: Box::new(parts.style.clone()),
                    spans: parts.spans.clone(),
                    para_spans: parts.para_spans.clone(),
                    paragraph: parts.paragraph.clone(),
                    block: parts.block,
                    // **`Auto`, whatever the defaults say.** A rail's length is the
                    // measure on a railed node, so the sizing is inert — and
                    // storing a wrap width nothing reads would come back the moment
                    // the type was detached, sized against a box the user never
                    // drew.
                    sizing: crate::node::TextSizing::Auto,
                    on_path: Some(Box::new(rail)),
                    on_path_flip: false,
                    on_path_offset: offset,
                },
                transform: Some(node.transform()),
                // Numbered like any other text layer. The consumed shape's name
                // described a shape; this is a piece of writing.
                name: None,
            },
            Operation::DeleteNode { id: path },
            Operation::Reorder { id, index: at },
        ]),
        id,
    ))
}

/// Take a text node off its rail, leaving the rail behind as a `Path` layer —
/// *Detach from path*.
///
/// **The rail comes back as a layer rather than being dropped**, which is what
/// makes [`text_on_new_path`] a two-way door: the curve a designer drew is work,
/// and a click that consumed it irreversibly would make hovering an edge with the
/// Text tool something to be careful about rather than something to try.
///
/// The new layer carries the text node's own transform, because the rail was
/// stored in that node's space — anything else puts it somewhere the text has
/// never been. It is inserted **above** the text in the same parent, where a path
/// drawn by hand would have been.
///
/// ⚠️ **The path comes back with no paint at all.** The one it had before is gone
/// — `text_on_path` deleted the layer that carried it and the rail is geometry
/// from that point on — so this is a genuine loss and the only one in the round
/// trip. Stated rather than fixed: carrying a dead layer's fill list on a text
/// node to serve a verb that may never be used is the kind of state that outlives
/// its reason.
pub fn detach_text_path(
    doc: &Document,
    ids: &mut IdSource,
    id: NodeId,
) -> Result<(Transaction, NodeId), OpError> {
    let node = doc.get(id).ok_or(OpError::NoSuchNode(id))?;
    let NodeKind::Text { on_path, .. } = node.kind() else {
        return Err(OpError::WrongKindForOp);
    };
    let rail = on_path.as_deref().cloned().ok_or(OpError::WrongKindForOp)?;
    let parent = node.parent().ok_or(OpError::CannotModifyRoot)?;
    let siblings = doc
        .get(parent)
        .ok_or(OpError::NoSuchNode(parent))?
        .children();
    let at = siblings
        .iter()
        .position(|c| *c == id)
        .ok_or(OpError::MalformedSubtree)?;
    let new = ids.mint();
    let tx = Transaction(vec![
        Operation::CreateNode {
            id: new,
            parent,
            // Appended then reordered, as `replace_with_path` does and for the same
            // reason: `siblings.len()` is the one index always in range. `at + 1`
            // is directly above the text, which is where the path was.
            index: siblings.len(),
            kind: NodeKind::Path {
                path: rail,
                corner_radii: Vec::new(),
            },
            transform: Some(node.transform()),
            // Numbered like any other path — the layer this rail came from was
            // deleted, and inventing its old name back would be a guess.
            name: None,
        },
        Operation::Reorder {
            id: new,
            index: at + 1,
        },
        Operation::SetGeometry {
            id,
            geometry: GeometryPatch::TextPath(None),
        },
    ]);
    Ok((tx, new))
}

/// The body [`flatten`]'s single-boolean arm, [`outline`] and [`outline_text`]
/// share: `id` becomes a `Path` holding `path`, in `id`'s own slot, carrying
/// everything about the layer that is not its geometry.
///
/// **One function rather than three that look alike**, because "what survives being
/// redrawn" is a list, and a list written twice drifts — the failure §15 D87 and D92
/// record for the paintable-kinds list, which was written three times.
fn replace_with_path(
    doc: &Document,
    ids: &mut IdSource,
    id: NodeId,
    path: kurbo::BezPath,
) -> Result<(Transaction, NodeId), OpError> {
    let node = doc.get(id).ok_or(OpError::NoSuchNode(id))?;
    let parent = node.parent().ok_or(OpError::CannotModifyRoot)?;
    let siblings = doc
        .get(parent)
        .ok_or(OpError::NoSuchNode(parent))?
        .children();
    let at = siblings
        .iter()
        .position(|c| *c == id)
        .ok_or(OpError::MalformedSubtree)?;

    let new = ids.mint();
    let mut ops = vec![Operation::CreateNode {
        id: new,
        parent,
        // Appended, then reordered into the old node's slot once it is gone —
        // `group` and `boolean` place their containers the same way, and for
        // the same reason: `siblings.len()` is the one index that is always valid.
        index: siblings.len(),
        kind: NodeKind::Path {
            path,
            corner_radii: Vec::new(),
        },
        transform: Some(node.transform()),
        // **The layer's name survives**, even the one we gave it. A flattened
        // "Subtract" is still the subtract the user built and named their way
        // around, and an outlined "Logo" is still the logo; renaming either to
        // "Path" here would lose the only remaining record of what it was, and the
        // row's icon already says it is a path now.
        name: Some(node.name().to_string()),
    }];
    ops.extend(carried_paint(node, new));
    // **This *is* the same layer**, so everything about it that is not its geometry
    // comes across — where `flatten_union` below makes a genuinely new layer out of
    // several and inherits only paint, the way `boolean` does.
    if node.opacity() != 1.0 {
        ops.push(Operation::SetOpacity {
            id: new,
            opacity: node.opacity(),
        });
    }
    if !node.visible() {
        ops.push(Operation::SetVisible {
            id: new,
            visible: false,
        });
    }
    // **A moved pivot comes across too**, and that is the sentence above being kept
    // rather than a new feature: the transform and the local space are unchanged, so
    // a local pivot means exactly what it meant, and dropping it silently reset the
    // rotation centre of a layer the user had deliberately placed it on. It was
    // dropped until 2026-08-19 — a small loss in `flatten` that only became visible
    // when `outline` made this the shared definition of what survives (§15 D230).
    if let Some(pivot) = node.pivot() {
        ops.push(Operation::SetPivot {
            id: new,
            pivot: Some(pivot),
        });
    }
    // **And so does a mask**, for the same reason and with a sharper symptom. The
    // flag is a property of the layer rather than of its outline — the new path is
    // the same shape in the same slot, so it clips the same run — and dropping it
    // would release the mask as a side effect of *Outline shape*, which reads as
    // the artwork above suddenly snapping back to full size with nothing on screen
    // saying why. Exactly the loss the pivot note above records, one step louder.
    if node.mask() {
        ops.push(Operation::SetMask {
            id: new,
            mask: true,
        });
    }
    // **And the effect stack** (§5.3a), which is the same loss again one field
    // later: a shadow is a property of the *layer*, the new path is the same shape
    // in the same slot, and *Outline shape* silently flattening a shadow away is
    // the pivot's symptom with more of the picture missing. Written as the whole
    // stack because that is the only shape `SetEffects` has, and skipped entirely
    // when it is empty so no document without one grows an operation.
    if !node.effects().is_empty() {
        ops.push(Operation::SetEffects {
            id: new,
            effects: node.effects().to_vec(),
        });
    }
    // **And the fill rule, which is the one carry that changes what is *drawn***
    // rather than preserving a property beside the drawing. `Node::fill_rule`
    // **derives** even-odd for an `Exclude` from the operation instead of storing it
    // (§15 D239), and this function's whole job is to throw the operation away — so
    // the derivation dies with the container, and the identical subpaths sitting in
    // a `Path` under the default non-zero rule draw the **union**: every hole the
    // exclusion cut fills in. Carried as the *effective* rule, which is one
    // expression for two cases — a derived one here, and the stored one an
    // [`outline`]d `Path` the user set even-odd on would otherwise lose the same way.
    //
    // Skipped when it is the default so no document grows an operation for the rule
    // it already meant, which is the same shape as the pivot and the effects above.
    if node.fill_rule() != crate::node::FillRule::default() {
        ops.push(Operation::SetFillRule {
            id: new,
            rule: node.fill_rule(),
        });
    }
    // **And its place in a layout** (§15 D930): its insets, which pin it against
    // its frame, and its item properties — a grid cell, a growth, an `align-self`.
    // Dropped, the path came back **unpinned and in the flow**: a pinned layer
    // outlined jumped from its corner into the row, a grid item lost its cell and
    // its siblings reflowed around it, from an edit that changes only how the
    // geometry is held. The three fields arrived on `Node` with container layout
    // and this list was not extended — the drift its own doc warns of. Skipped at
    // the defaults, as the carries above are.
    //
    // ⚠️ **`display` does not come across, on purpose**: a path holds no children,
    // so a layout on one would lay out nothing (`container::is_container`).
    if node.insets().is_authored() {
        ops.push(Operation::SetInsets {
            id: new,
            insets: *node.insets(),
        });
    }
    if *node.item() != crate::container::LayoutItem::default() {
        ops.push(Operation::SetLayoutItem {
            id: new,
            item: *node.item(),
        });
    }
    // **Two things deliberately do not come across.** *Lock* is a property of the
    // layer the user is holding rather than of the picture, and a locked result would
    // refuse the tidying that usually follows. The **proportion lock** is a
    // constraint on resizing an authored box (§15 D41), and the whole of what this
    // operation does is stop the layer having one — carrying it would preserve a rule
    // about a field that no longer exists.
    ops.push(Operation::DeleteNode { id });
    ops.push(Operation::Reorder { id: new, index: at });
    Ok((Transaction(ops), new))
}

/// Several layers, replaced by the union of their outlines.
fn flatten_union(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    members: &[NodeId],
) -> Result<(Transaction, NodeId), OpError> {
    // The same admission test as `boolean`, because the same kinds have nothing to
    // contribute and the same one-parent rule makes z-order meaningful.
    let mut parent = None;
    for id in members {
        let node = doc.get(*id).ok_or(OpError::NoSuchNode(*id))?;
        if matches!(
            node.kind(),
            NodeKind::Artboard { .. } | NodeKind::Root | NodeKind::Text { .. }
        ) {
            return Err(OpError::WrongKindForOp);
        }
        // **A group with a frame in it, refused as `boolean` refuses it** (§15
        // D876) — and here it matters more than there, because nothing would stop
        // it: the union deletes the members, so no frame is left anywhere for
        // `apply` to object to. `operand_of` would take the frame's *box* for its
        // outline, ignore its contents and then delete them, and the frame's guides
        // would be left owned by a node that no longer exists — a file that then
        // fails to load.
        if doc.holds_a_frame(*id) {
            return Err(OpError::ArtboardPlacement);
        }
        let p = node.parent().ok_or(OpError::CannotModifyRoot)?;
        match parent {
            None => parent = Some(p),
            Some(existing) if existing != p => return Err(OpError::InvalidParent),
            _ => {}
        }
    }
    let parent = parent.expect("non-empty member set");
    let siblings = doc.get(parent).expect("parent of a live node").children();
    let set: FxHashSet<NodeId> = members.iter().copied().collect();
    // Bottom of the stack first, as `boolean` orders its operands — irrelevant to a
    // union's *result*, and kept anyway so the two builders cannot be read as
    // disagreeing about what operand order is.
    let ordered: Vec<NodeId> = siblings
        .iter()
        .copied()
        .filter(|c| set.contains(c))
        .collect();
    let final_index = container_slot(siblings, &set)?;

    // Each member's contribution in the parent's space, which is the space the new
    // node's identity transform puts its path in.
    let outlines: Vec<kurbo::BezPath> = ordered
        .iter()
        .filter_map(|m| res.operand_path(doc, *m))
        .collect();
    // **Bracketed, because `None` here has two meanings and only one of them is the
    // user's to fix** (§15 D736, `[S10.1-L2-06]`). A union of shapes that cancel out
    // entirely is `EmptyGeometry` and is honest; a union flo_curves *unwound* out of
    // is also `None`, and telling the user their geometry was empty sends them to
    // move a shape, which cannot help. `boolean::failures` is the only thing that
    // separates them — see its doc: the counter is the signal, never the `None`.
    //
    // ⚠️ **This door is outside §15 D298's bracket by construction, not by
    // oversight.** D298 reads the counter at `commit_inner` across `Resolved::update`
    // — after a transaction exists. A builder calls `evaluate` while *making* one, so
    // a failure here refuses the edit before that bracket has anything to span.
    let before = crate::boolean::failures();
    let path = match crate::boolean::evaluate(crate::node::BoolOp::Union, &outlines) {
        Some(path) => path,
        None if crate::boolean::failures() != before => return Err(OpError::BooleanAbandoned),
        None => return Err(OpError::EmptyGeometry),
    };

    let new = ids.mint();
    let kind = NodeKind::Path {
        path,
        corner_radii: Vec::new(),
    };
    // **Named here rather than left to `op_create`, because the operands are still
    // present when it runs.** The create comes before the deletes below, so
    // `child_names` would count the very layers this is replacing: flattening a
    // path called "Path" gave "Path 2" and then took the "Path" away, leaving a
    // number 2 with nothing above it. Numbered against the siblings that *survive*
    // (§15 D127).
    let name = crate::naming::free_name(&doc.child_names(parent, &ordered), kind.default_name(), 1);
    let mut ops = vec![Operation::CreateNode {
        id: new,
        parent,
        index: siblings.len(),
        kind,
        transform: None,
        name: Some(name),
    }];
    // The bottom member's paint, read before anything is deleted — `boolean`'s rule,
    // so uniting-then-flattening and flattening directly give the same colour.
    let base = doc.get(ordered[0]).expect("checked above");
    ops.extend(carried_paint(base, new));
    for member in &ordered {
        ops.push(Operation::DeleteNode { id: *member });
    }
    ops.push(Operation::Reorder {
        id: new,
        index: final_index,
    });
    Ok((Transaction(ops), new))
}

/// The paint a node built out of others inherits from the one it stands in for.
///
/// Emitted only where there is something to say, so a shape with no stroke does not
/// carry an op setting an empty list. A fresh node has no fills at all, and the
/// alternative to inheriting is a result that evaluates correctly and then draws
/// nothing — which reads as the operation having deleted the artwork ([`boolean`]
/// makes the same bargain for the same reason).
fn carried_paint(from: &Node, to: NodeId) -> Vec<Operation> {
    let mut ops = Vec::new();
    if !from.paint().fills.is_empty() {
        ops.push(Operation::SetFills {
            id: to,
            fills: from.paint().fills.clone(),
        });
    }
    if !from.paint().strokes.is_empty() {
        ops.push(Operation::SetStrokes {
            id: to,
            strokes: from.paint().strokes.clone(),
        });
    }
    ops
}

/// Dissolve a `Group` or a `Boolean`, splicing its children into the container's
/// parent at its own z-position and folding its transform into each child so
/// nothing moves.
///
/// **A boolean releases through here, and that is the whole of "release".** Its
/// operands were never destroyed — they kept their geometry, their transforms and
/// their paint inside the container, which is what non-destructive means — so
/// taking one apart is the identical splice a group needs and needed no builder of
/// its own. What the user gets back is the shapes they started with, in the same
/// places, with the combined outline gone.
///
/// The one asymmetry is paint, and it is deliberate: the boolean's own fill and
/// stroke governed the result and vanish with it, while each operand's own paint —
/// inert while it was consumed — comes back into view. `build::boolean` gave the
/// container the *bottom* operand's paint precisely so that this round trip lands
/// somewhere recognisable.
///
/// **Folded from where things are drawn** (§15 D929): the container's used
/// transform into each child's used transform, and each child's used size
/// written as its own ([`baked`]). For a group with a layout neither stored
/// number is a place — the group's pass placed its children, and its parent's
/// pass may have placed the group — so folding the stored ones put every in-flow
/// child at its creation point, overlapping, and re-pinned a pinned child from a
/// place it had left. Outside a layout the used and stored transforms are the
/// same numbers, so a boolean's release is the arithmetic it always was.
pub fn ungroup(doc: &Document, res: &Resolved, id: NodeId) -> Result<Transaction, OpError> {
    let node = doc.get(id).ok_or(OpError::NoSuchNode(id))?;
    if !matches!(node.kind(), NodeKind::Group | NodeKind::Boolean { .. }) {
        return Err(OpError::WrongKindForOp);
    }
    let parent = node.parent().ok_or(OpError::CannotModifyRoot)?;
    let group_transform = res.used_local(doc, id).unwrap_or(node.transform());
    let children = node.children().to_vec();
    let at = doc
        .get(parent)
        .expect("parent of a live node")
        .children()
        .iter()
        .position(|c| *c == id)
        .ok_or(OpError::MalformedSubtree)?;

    let mut ops = Vec::with_capacity(children.len() * 2 + 1);
    for (i, child) in children.iter().enumerate() {
        let child_local = res
            .used_local(doc, *child)
            .ok_or(OpError::NoSuchNode(*child))?;
        // The group's parent replaces the group as the frame of reference.
        ops.push(Operation::SetTransform {
            id: *child,
            transform: group_transform * child_local,
        });
        ops.extend(baked_size(doc, res, *child));
        ops.push(Operation::Reparent {
            id: *child,
            new_parent: parent,
            index: at + i,
        });
    }
    ops.push(Operation::DeleteNode { id });
    Ok(Transaction(ops))
}

/// Dissolve several containers as **one** transaction, in the order given.
///
/// One transaction and not one per container, for the reason
/// [`insert_subtrees`] gives for the paste: one press of `Ctrl+Shift+G` is one
/// thing the user did, so it is one undo step (invariant 5), and a failure
/// partway through must leave nothing behind rather than half a command that
/// then reports it failed. Both were live — ungrouping two groups took **two**
/// presses of `Ctrl+Z`, and a container that refused on the second attempt left
/// the first already dissolved (§15 D521).
///
/// ⚠️ **Built against a document advanced as it goes, not against index
/// arithmetic.** [`ungroup`] reads the container's own position among its
/// parent's children, and the splice moves every sibling after it — so the
/// second container's index is wrong the moment the first one's ops are
/// prepended to the same transaction, and a container nested *inside* an
/// earlier one has a different parent and a different local transform by the
/// time its turn comes. Simulating is the same rule
/// `layers::plan_layer_move` follows for a multi-row reorder and the one
/// [`insert_subtrees`] follows for a multi-subtree paste; what is different
/// here is that a whole `Document` is cloned rather than a map of child lists,
/// because an ungroup rewrites transforms as well as parentage. The clone is
/// skipped for the one-container case, which is every ungroup but a
/// multi-selection.
///
/// `ids` are dissolved in the order given and each must be a live `Group` or
/// `Boolean`; anything else comes back as the error [`ungroup`] would have
/// returned for it, with no ops built.
pub fn ungroup_all(doc: &Document, res: &Resolved, ids: &[NodeId]) -> Result<Transaction, OpError> {
    if let [only] = ids {
        return ungroup(doc, res, *only);
    }
    // The derived layer advanced with it, since [`ungroup`] folds from where
    // things are drawn (§15 D929): `res` for the first container, then one
    // rebuild over the advanced document and an incremental update per step after
    // it, as a commit makes. Only a multi-selection pays for the rebuild.
    let mut advanced = doc.clone();
    let mut resolved: Option<Resolved> = None;
    let mut ops = Vec::new();
    for id in ids {
        let tx = ungroup(&advanced, resolved.as_ref().unwrap_or(res), *id)?;
        let outcome = advanced.apply(&tx)?;
        match &mut resolved {
            Some(r) => r.update(&advanced, &outcome.dirty),
            None => resolved = Some(Resolved::rebuild(&advanced)),
        }
        ops.extend(tx.0);
    }
    Ok(Transaction(ops))
}

/// Where one captured subtree should land when pasted or duplicated.
pub struct Placement {
    /// A subtree captured with [`Document::capture_subtree`]; its ids are
    /// remapped before insertion, so the same template can be placed repeatedly.
    pub nodes: Vec<Node>,
    pub parent: NodeId,
    /// `None` appends.
    pub index: Option<usize>,
}

/// Insert several captured subtrees as **one** transaction, each offset by
/// `offset` in its parent's space. Returns the transaction and the new root ids
/// in placement order.
///
/// One transaction, not one per subtree, for two reasons: pasting five layers
/// is one thing the user did and must be one undo step, and a failure partway
/// through must leave nothing behind rather than half a paste.
///
/// Indices are advanced as earlier inserts land, so appending several subtrees
/// to the same parent keeps their relative order instead of stacking them all
/// at the pre-transaction child count.
///
/// **Each index is advanced past the earlier inserts that landed at or below it**, and
/// the "or below" is the whole of it. A plain count of what this transaction has already
/// added to the parent is right only while the placements' indices *ascend*, which is
/// what they did while every caller appended. They no longer do: a copy now lands
/// immediately above its original, so two layers copied top-down arrive as index 2 then
/// index 1 — and a flat count turned the second into 2 as well, stacking one copy above
/// the wrong sibling. Counting only the inserts that already shifted *this* index is
/// correct for either order, and is the same simulate-as-you-go rule
/// `layers::plan_layer_move` uses for a multi-row reorder (§15 D54).
pub fn insert_subtrees(
    doc: &Document,
    ids: &mut IdSource,
    placements: &[Placement],
    offset: Vec2,
) -> (Transaction, Vec<NodeId>) {
    // The indices this transaction has already inserted at, per parent — not a count of
    // them. See the note above on why the count was not enough.
    let mut added: FxHashMap<NodeId, Vec<usize>> = FxHashMap::default();
    let mut ops = Vec::with_capacity(placements.len() * 2);
    let mut created = Vec::with_capacity(placements.len());
    let mut names = CopyNames::default();

    for placement in placements {
        let Some((nodes, new_root)) = crate::document::remap_subtree(&placement.nodes, ids) else {
            continue;
        };
        let Some(root) = nodes.iter().find(|n| n.id() == new_root) else {
            continue;
        };
        let (base, was_called) = (root.transform(), root.name().to_string());
        let base_index = placement.index.unwrap_or_else(|| {
            doc.get(placement.parent)
                .map(|n| n.children().len())
                .unwrap_or(0)
        });
        let done = added.entry(placement.parent).or_default();
        let shift = done.iter().filter(|i| **i <= base_index).count();
        ops.push(Operation::InsertSubtree {
            nodes,
            parent: placement.parent,
            index: base_index + shift,
        });
        done.push(base_index);
        ops.push(Operation::SetTransform {
            id: new_root,
            transform: Affine::translate(offset) * base,
        });
        ops.extend(names.rename(doc, placement.parent, new_root, &was_called));
        created.push(new_root);
    }

    (Transaction(ops), created)
}

/// The names a paste or duplicate has already handed out, per destination parent.
///
/// **A copy's name is decided before its op is built, so several copies in one
/// transaction have to remember each other.** `remap_subtree` carries the name
/// through untouched — it is the same node with new ids — so pasting three
/// "Rectangle 2"s would otherwise make three more "Rectangle 3"s: each asks the
/// document, and the document has not seen the earlier ones yet because nothing in
/// the transaction has been applied.
///
/// Per *parent*, because numbering is per parent (§15 D127). A flat list would let one
/// frame's "Rectangle 3" stop another frame from using the number.
#[derive(Default)]
pub struct CopyNames(FxHashMap<NodeId, Vec<String>>);

impl CopyNames {
    /// The `SetName` a copy of a layer called `was_called` needs to join `parent`,
    /// or nothing when it keeps the name it has.
    ///
    /// **A `SetName` op rather than a rewritten `Node`.** The nodes are an
    /// `InsertSubtree` payload and `Node`'s fields are private with no public
    /// setter — invariant 2, and worth keeping: a `Node::with_name` reachable from
    /// the app would be the one way to change a name without going through
    /// `apply`. Emitting the op instead is how `set_boolean_op` already renames,
    /// and it costs one entry in a transaction that carries two already.
    ///
    /// **The root only.** A duplicated group's children keep their names, because
    /// their parent is the *new* group and they are unique within it — which is
    /// what per-parent numbering means, and what makes a copied component read
    /// like the original rather than like a renumbered stranger.
    pub fn rename(
        &mut self,
        doc: &Document,
        parent: NodeId,
        root: NodeId,
        was_called: &str,
    ) -> Option<Operation> {
        let mine = self.0.entry(parent).or_default();
        let mut taken = doc.child_names(parent, &[]);
        taken.extend(mine.iter().map(String::as_str));
        let name = crate::naming::copy_name(&taken, was_called)?;
        mine.push(name.clone());
        Some(Operation::SetName { id: root, name })
    }
}

/// Every image `nodes` reference, once each and in sorted order.
///
/// **What a captured subtree does *not* carry.** [`Document::capture_subtree`]
/// clones nodes, and a node holds an [`ImageId`](crate::ImageId) — a key into a
/// table that lives on the document, not on the node. So a set of nodes is only
/// half a picture, and the half that is missing is the whole of it: paste that
/// set into a document whose table has no such key and the fill resolves to
/// nothing, which the scene walk draws as the missing-picture placeholder. This
/// is the other half, and it has to be taken while the source document is still
/// open — which for a clipboard means at copy time, not at paste time.
///
/// **Hidden fills count.** The visibility flag says whether to draw the brush,
/// not whether the document needs the bytes; dropping a hidden image fill's
/// picture would turn the eye into a destructive control that only reveals what
/// it destroyed one paste later.
///
/// **Ids are content hashes** (`image.rs`), so one taken from another document
/// is the same key for the same bytes — nothing has to be remapped on arrival,
/// and a picture already present is recognised rather than stored twice.
pub fn image_ids_in(nodes: &[Node]) -> Vec<crate::ImageId> {
    let mut out: Vec<crate::ImageId> = nodes
        .iter()
        .flat_map(|n| {
            let paint = n.paint();
            let fills = paint.fills.iter().map(|f| &f.brush);
            let strokes = paint.strokes.iter().map(|s| &s.brush);
            fills.chain(strokes)
        })
        .filter_map(|brush| match brush {
            Brush::Image(b) => Some(b.image.id.clone()),
            _ => None,
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The `AddImage` ops `doc` needs before it can show `held`.
///
/// **Only what is missing**, which is what makes this safe to run on every
/// paste rather than only on the cross-document one: pasting back into the
/// document a copy came from finds every id already present and produces
/// nothing, so the ordinary case pays one hash lookup per picture and the
/// transaction is unchanged.
///
/// **The ops are meant to go first in the transaction that inserts the nodes**,
/// and this function is where that rule is stated: one gesture is one undo step,
/// and the inverse has to remove the table entry after the layers that were using
/// it, never before. `app::insert_all` is the caller that owes it.
pub fn missing_image_ops(
    doc: &Document,
    held: &[(crate::ImageId, crate::ImageEntry)],
) -> Vec<Operation> {
    held.iter()
        .filter(|(id, _)| !doc.has_image(id))
        .map(|(id, entry)| Operation::AddImage {
            id: id.clone(),
            entry: entry.clone(),
        })
        .collect()
}

/// The members of `ids` that have no ancestor in `ids`, in input order and
/// de-duplicated. Operating on these alone avoids applying an edit twice to a
/// node that is also carried by its ancestor.
pub fn outermost(doc: &Document, ids: &[NodeId]) -> Vec<NodeId> {
    let set: FxHashSet<NodeId> = ids.iter().copied().collect();
    let mut seen = FxHashSet::default();
    ids.iter()
        .copied()
        .filter(|id| seen.insert(*id))
        .filter(|id| doc.contains(*id))
        .filter(|id| {
            let mut cursor = doc.get(*id).and_then(|n| n.parent());
            while let Some(p) = cursor {
                if set.contains(&p) {
                    return false;
                }
                cursor = doc.get(p).and_then(|n| n.parent());
            }
            true
        })
        .collect()
}

/// [`outermost`], re-ordered into the order the document paints them — bottom of
/// the stack first.
///
/// **What every *export* of a selection owes its writer.** `svg_of` and `png_of`
/// keep the caller's order and say so, because SVG has no z-index and the walk has
/// no sort: the last thing emitted is the thing on top. A selection is in **pick
/// order** — `Selection::ids` pushes as you shift-click, and [`outermost`] preserves
/// its input — so clicking the front shape and then the one behind it hands the
/// writer a reversed stack, and the file comes back with the artwork rearranged.
/// Both export doors go through here now.
///
/// **Not `outermost` doing it itself**, deliberately: that function is on every
/// bulk *edit* path, where input order is either irrelevant or is the user's own
/// (the key layer, the z-order verbs), and quietly sorting underneath those would
/// be a change to code that has nothing to do with export.
///
/// One pre-order walk of the tree, emitting a member when it is reached — see
/// [`all_in_document_order`], which is that walk and is all this adds `outermost`
/// to. ⚠️ **This paragraph used to say a matched member is not descended into**,
/// on the argument that `outermost` guarantees nothing sits inside one. The
/// conclusion still holds and the mechanism no longer does: since the walk was
/// split out for §15 D516 it descends unconditionally, and there is simply
/// nothing below a member for it to find.
pub fn in_document_order(doc: &Document, ids: &[NodeId]) -> Vec<NodeId> {
    all_in_document_order(doc, &outermost(doc, ids))
}

/// `ids` in document order, **keeping a member that sits inside another**
/// (§15 D516).
///
/// [`in_document_order`]'s ordering without its filter, and the split is the
/// point: `outermost` is right for a **selection**, where a group and its child
/// both picked would plan the child twice — the app's `export_subjects` gives
/// that reason itself (§15 D267; plain backticks rather than a link, because it
/// lives in `ondin-app` and core cannot resolve one into it). It is
/// wrong for the set of layers *carrying export settings*, where
/// a spec on a nested layer is a deliberate instruction and exporting the group
/// produces a different file from exporting the child. A banner frame with an
/// icon inside it exported separately is the workflow the panel exists for.
///
/// `[S8.3-L1-02]`: *Export all* dropped that icon silently — one planned file
/// where the panel, the menu row and the CLI all say *"every layer with export
/// settings"*, and `write_export`'s `skipped` counter only counts layers with no
/// **bounds**, so the status line read *"Exported 1 file"* with no qualification.
///
/// A matched member **is** descended into here, for the same reason the filter is
/// gone: there can be another below it.
pub fn all_in_document_order(doc: &Document, ids: &[NodeId]) -> Vec<NodeId> {
    let want: FxHashSet<NodeId> = ids.iter().copied().collect();
    let mut out = Vec::with_capacity(want.len());
    let mut stack = vec![doc.root()];
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get(id) else { continue };
        if want.contains(&id) {
            out.push(id);
        }
        stack.extend(node.children().iter().rev().copied());
    }
    out
}

/// The `RemoveGuide` operations that must accompany deleting the subtrees rooted
/// at `ids` — every guide scoped to a frame inside them.
///
/// **These belong at the front of the delete transaction**, ahead of the
/// `DeleteNode`s themselves. `Document::apply` inverts a transaction by reversing
/// it, so guides-then-nodes inverts to nodes-then-guides: the frame is back in
/// the document by the time its guide is re-added, which is what
/// `Document::check_guide_owner` requires. The other order compiles, deletes
/// correctly, and makes the undo fail.
///
/// Sorted by id so the transaction — and therefore the undo step — is
/// deterministic whatever order the guides were drawn in (invariant 9's reasoning
/// applied to an edit rather than to the file).
pub fn guides_of(doc: &Document, ids: &[NodeId]) -> Vec<Operation> {
    let doomed: FxHashSet<NodeId> = subtree_nodes(doc, ids).into_iter().collect();
    let mut guides: Vec<_> = doc
        .guides()
        .iter()
        .filter(|g| g.owner.is_some_and(|o| doomed.contains(&o)))
        .map(|g| g.id)
        .collect();
    guides.sort();
    guides
        .into_iter()
        .map(|id| Operation::RemoveGuide { id })
        .collect()
}

// --- bulk property edits over a selection ---------------------------------

/// Every node in the subtrees rooted at `ids`, each visited exactly once, in a
/// deterministic pre-order.
///
/// The apply-once rule (§5.7a) as it applies to *properties* rather than
/// transforms. [`outermost`] alone is not the right filter for a colour: it
/// would hand back a group, which has no paint of its own, and the shapes the
/// user can plainly see inside it would go unchanged. So the members are
/// reduced to the outermost ones first — which is what stops a node selected
/// alongside its own ancestor being visited twice — and then each subtree is
/// walked in full.
///
/// **Document order, whole — roots included** (§15 D646). Pre-order within each
/// subtree, children pushed in reverse so they pop in document order, and the
/// roots themselves ordered before the walk starts. An operation list has to
/// replay identically on another replica (§12), so it cannot depend on the order
/// a `HashSet` felt like iterating in — and, since `[S3.2-L2-05]`, it cannot
/// depend on the order a *caller* felt like passing its selection in either.
pub fn subtree_nodes(doc: &Document, ids: &[NodeId]) -> Vec<NodeId> {
    // ⚠️ **The *roots* have to be ordered too, and they were not** (§15 D646,
    // `[S3.2-L2-05]`). Pre-order within one subtree was never the whole claim:
    // `outermost`'s own doc says it returns *"in input order"*, so a two-root
    // scope came back in the order the caller happened to hold it. Measured:
    // three rects under one artboard gave `[a, b, c]` from `&[artboard]` and
    // `[c, a, b]` from `&[c, a, b]` — and the second is the shape the Group
    // Colors panel passes, since `group_color_scope` hands over
    // `selection.ids()`, which is **pick order**. Shift-clicking two shapes
    // front-to-back and pressing *Select 2 layers with this colour* re-selected
    // them front-to-back.
    //
    // Only when there is more than one root, because that is the only case it
    // can matter and `all_in_document_order` is a walk of the whole document.
    // The five production root-only callers pay nothing: the layers panel's
    // order index, `layers::tree_containers` and `layers::tree_is_all_collapsed`
    // beside it, the frame index, and `resolve`'s boolean sweep.
    //
    // ⚠️ **That list read as three until `arch-scribe` counted it** (§15 D646),
    // and the two it left out are in the panel it already names — session 14's
    // `settings::card` shape, where the half a scoped sentence omits is the
    // module it is scoped to.
    //
    // ⚠️ **The multi-root cost is per `census` call and there are more of those
    // a frame than the panel suggests.** `census` calls this once, so
    // `colors_in` and `gradients_in` are one walk each — but
    // `inspector_multi_appearance` also calls `shared_radius_shown` and
    // `any_rect_in` on the selection every frame it draws, both through here, so
    // a multi-selection with Appearance and Group Colors open pays four.
    let roots = outermost(doc, ids);
    let roots = if roots.len() > 1 {
        all_in_document_order(doc, &roots)
    } else {
        roots
    };
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = roots.into_iter().rev().collect();
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get(id) else { continue };
        out.push(id);
        stack.extend(node.children().iter().rev().copied());
    }
    out
}

/// One colour found inside a selection, the places that use it, and the layers
/// they belong to.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorUse {
    pub color: Color,
    /// Every fill and every stroke carrying this exact colour — a frame's fills
    /// among them, and no longer as a third kind of place the census had to know
    /// about (§15 D400). A layer
    /// whose fill and stroke are the same colour counts **twice** here, which is
    /// what makes this the wrong number to show beside a "select every layer
    /// using this" button — see `nodes`.
    pub uses: usize,
    /// The distinct layers carrying it, in document order.
    ///
    /// The set the Group Colors panel counts and its select button acts on, so
    /// the number on the row and the selection the button makes cannot disagree.
    /// Carried out of the one census walk rather than re-derived: the panel would
    /// otherwise walk the tree a second time per row, per frame, to answer a
    /// question this walk already had in hand.
    pub nodes: Vec<NodeId>,
}

/// The distinct solid colours inside the subtrees rooted at `ids`, most-used
/// first.
///
/// This is what the Group Colors panel lists and what the picker's palette row
/// shows — the same census at two scopes, which is why it lives here rather than
/// in either panel. Pass the root to ask about the whole document.
///
/// **Solids only.** A gradient is not one colour, so there is no single swatch
/// that could stand for it and nothing sensible for [`recolor`] to do to it. It
/// is left out of the count rather than flattened to a stop, because a list that
/// claimed a gradient was `#EB6E5A` would recolour something the user did not
/// point at.
///
/// **Hidden paints count.** A fill with `visible: false` still carries a colour,
/// and leaving it out would mean a recolour skipped it and the old colour
/// reappeared the moment the fill was switched back on.
pub fn colors_in(doc: &Document, ids: &[NodeId]) -> Vec<ColorUse> {
    census(doc, ids, |brush| match brush {
        Brush::Solid(c) => Some(c.to_rgba8().to_u8_array()),
        _ => None,
    })
    .into_iter()
    // The location the census also found is `ColorUse`'s to ignore: a colour *is* a
    // key, so its row has never needed a place to point at.
    .map(|([r, g, b, a], uses, nodes, _)| ColorUse {
        color: Color::from_rgba8(r, g, b, a),
        uses,
        nodes,
    })
    .collect()
}

/// One place a paint sits: a node, which of its two lists, and where in it.
///
/// **Two lists and an index is the whole of it, on every kind.** It used to need
/// a caveat — a frame's one background stood in for fill 0 — and that caveat went
/// away with the field it was about (§15 D400).
///
/// Small, `Copy` and `Eq` on purpose: this is what a UI slot can carry when the
/// *value* is too big to (`inspector::PaintSlot` is `Copy + Eq`, and a gradient's
/// identity is its whole brush). A location key also survives the edit that a value
/// key does not — after a gradient is changed, the place it was found still holds
/// it, where a key naming the old brush names something the document no longer has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PaintAt {
    pub node: NodeId,
    pub target: PaintTarget,
    pub index: usize,
}

/// One gradient found inside a selection, and where it is used. [`ColorUse`]'s
/// twin, and the fields mean the same things.
#[derive(Clone, Debug, PartialEq)]
pub struct GradientUse {
    /// The gradient itself, exactly as the artwork holds it.
    pub brush: Brush,
    pub uses: usize,
    pub nodes: Vec<NodeId>,
    /// The **first** place this gradient was found, in document order.
    ///
    /// What the Group Colors row hands the picker, since the gradient itself will not
    /// fit in a slot. Any one of its uses would do — the edit reaches all of them
    /// ([`repaint`]) — and the first is the one that is stable to name.
    pub at: PaintAt,
}

/// The distinct **gradients** inside the subtrees rooted at `ids`, most-used first.
///
/// [`colors_in`]'s twin, over the paints it has to leave out. A gradient is not one
/// colour, so it can be neither shown as a swatch nor handed to [`recolor`] — but
/// "which gradients are in here, and which layers use them" is the same question
/// asked of the same walk, and answering only half of it left a selection's
/// gradients uncounted and unfindable.
///
/// Keyed by the whole `Brush`, since a gradient's identity is its stops and its
/// geometry together: two ramps of the same colours at different angles are two
/// gradients, and a panel that merged them would report a count no selection could
/// match.
pub fn gradients_in(doc: &Document, ids: &[NodeId]) -> Vec<GradientUse> {
    census(doc, ids, |brush| match brush {
        Brush::Gradient(_) => Some(brush.clone()),
        _ => None,
    })
    .into_iter()
    .map(|(brush, uses, nodes, at)| GradientUse {
        brush,
        uses,
        nodes,
        at,
    })
    .collect()
}

/// The census walk both public censuses are: every paint in the subtrees, grouped
/// by whatever `key` makes of its brush, most-used first. A `None` key skips the
/// paint.
///
/// One walk with a key function rather than one function per kind of key. The two
/// answers have to agree about what they found — the same paints, the same layers,
/// the same order — and the dedupe rule below is the fiddly part of that.
fn census<K: PartialEq>(
    doc: &Document,
    ids: &[NodeId],
    key: impl Fn(&Brush) -> Option<K>,
) -> Vec<(K, usize, Vec<NodeId>, PaintAt)> {
    // A `Vec` rather than a map: the counts are few, and insertion order is what
    // breaks ties below into something stable.
    let mut counts: Vec<(K, usize, Vec<NodeId>, PaintAt)> = Vec::new();
    let mut bump = |brush: &Brush, at: PaintAt| {
        let Some(k) = key(brush) else { return };
        match counts.iter_mut().find(|(other, _, _, _)| *other == k) {
            Some((_, n, nodes, _)) => {
                *n += 1;
                // The *layers*, not the paint slots: a shape whose fill and
                // stroke are the same colour is one layer using it twice.
                // `subtree_nodes` is pre-order, so checking the tail is enough
                // to keep this list distinct and in document order.
                if nodes.last() != Some(&at.node) {
                    nodes.push(at.node);
                }
                // The location is *not* updated: it is the first place this paint
                // was met, which is what makes it stable to name.
            }
            None => counts.push((k, 1, vec![at.node], at)),
        }
    };
    for id in subtree_nodes(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        for (i, f) in node.paint().fills.iter().enumerate() {
            bump(
                &f.brush,
                PaintAt {
                    node: id,
                    target: PaintTarget::Fill,
                    index: i,
                },
            );
        }
        for (i, s) in node.paint().strokes.iter().enumerate() {
            bump(
                &s.brush,
                PaintAt {
                    node: id,
                    target: PaintTarget::Stroke,
                    index: i,
                },
            );
        }
    }
    // Stable, so equally-used paints keep the order they were met in rather
    // than shuffling between frames under the user's pointer.
    counts.sort_by_key(|(_, n, _, _)| std::cmp::Reverse(*n));
    counts
}

/// The brush at a location, as the committed document holds it.
///
/// `None` when the place has gone — the layer was deleted, the list shrank —
/// which is what tells a UI slot pointing at it to close.
///
/// A frame used to be answered here by a branch of its own, its one background
/// standing in for fill 0. It is an ordinary entry in an ordinary fill list now
/// (§15 D400), so the index means on a frame exactly what it means everywhere
/// else.
pub fn paint_at(doc: &Document, at: PaintAt) -> Option<Brush> {
    let node = doc.get(at.node)?;
    match at.target {
        PaintTarget::Fill => node.paint().fills.get(at.index).map(|f| f.brush.clone()),
        PaintTarget::Stroke => node.paint().strokes.get(at.index).map(|s| s.brush.clone()),
    }
}

/// Replace every use of the brush `from` with `to` inside the subtrees rooted at
/// `ids` — every fill and every stroke, in one transaction.
///
/// **Frames included, and no longer by a branch of their own**: a frame's ground
/// is an entry in its fill list like any other (§15 D400), so the two loops above
/// reach it. Which also means a frame with *two* fills has both of them
/// recoloured, where the old third branch could only ever see one.
///
/// [`recolor`]'s twin for the paints a colour cannot name, and the Group Colors
/// panel's edit for a gradient row. **Not a generalisation of `recolor`**, which
/// matches on the exact RGBA *bytes* the colour census keys by; this matches whole
/// brushes with `==`. Two different match rules for two different keys, and folding
/// them together would make one of the two panels' rows disagree with its own edit.
pub fn repaint(doc: &Document, ids: &[NodeId], from: &Brush, to: &Brush) -> Transaction {
    let mut ops = Vec::new();
    for id in subtree_nodes(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        let swap = |b: &Brush| if b == from { to.clone() } else { b.clone() };
        let fills: Vec<Fill> = node
            .paint()
            .fills
            .iter()
            .map(|f| Fill {
                brush: swap(&f.brush),
                visible: f.visible,
            })
            .collect();
        if fills != node.paint().fills {
            ops.push(Operation::SetFills { id, fills });
        }
        let strokes: Vec<Stroke> = node
            .paint()
            .strokes
            .iter()
            .map(|s| Stroke {
                brush: swap(&s.brush),
                ..s.clone()
            })
            .collect();
        if strokes != node.paint().strokes {
            ops.push(Operation::SetStrokes { id, strokes });
        }
    }
    Transaction(ops)
}

/// Whether `brush` is exactly this solid colour.
fn is_solid(brush: &Brush, key: [u8; 4]) -> bool {
    matches!(brush, Brush::Solid(c) if c.to_rgba8().to_u8_array() == key)
}

/// Replace every use of `from` with `to` inside the subtrees rooted at `ids`.
///
/// The Group Colors edit: one colour, wherever it appears — every fill and every
/// stroke, a frame's among them ([`repaint`] says how that stopped being a third
/// branch) — changed together in **one** transaction and so one undo step
/// (invariant 5). Recolouring by colour rather than by layer is the
/// point of the panel: it is how a palette change is made without hunting for
/// every shape that happens to use the old ink.
///
/// Matched on the exact RGBA bytes, the same key [`colors_in`] counts by, so
/// what the panel lists and what this changes cannot disagree. Nodes with no
/// matching paint contribute no operation, which keeps the undo step to the
/// layers that actually moved.
pub fn recolor(doc: &Document, ids: &[NodeId], from: Color, to: Color) -> Transaction {
    let key = from.to_rgba8().to_u8_array();
    // Opening a Group Colors picker and closing it without moving anything
    // arrives here as a recolour to the same colour. Rewriting every use with
    // what it already says would be an undo step the user cannot see.
    if to.to_rgba8().to_u8_array() == key {
        return Transaction(Vec::new());
    }
    let mut ops = Vec::new();
    for id in subtree_nodes(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        let paint = node.paint();

        if paint.fills.iter().any(|f| is_solid(&f.brush, key)) {
            let fills = paint
                .fills
                .iter()
                .map(|f| Fill {
                    brush: if is_solid(&f.brush, key) {
                        Brush::Solid(to)
                    } else {
                        f.brush.clone()
                    },
                    visible: f.visible,
                })
                .collect();
            ops.push(Operation::SetFills { id, fills });
        }

        if paint.strokes.iter().any(|s| is_solid(&s.brush, key)) {
            let strokes = paint
                .strokes
                .iter()
                .map(|s| {
                    let mut next = s.clone();
                    if is_solid(&s.brush, key) {
                        next.brush = Brush::Solid(to);
                    }
                    next
                })
                .collect();
            ops.push(Operation::SetStrokes { id, strokes });
        }
    }
    Transaction(ops)
}

/// Which of a node's two paint lists a bulk edit is aimed at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaintTarget {
    Fill,
    Stroke,
}

/// The nodes a bulk paint edit over `ids` actually writes to.
///
/// **A group is organisation; a frame is a thing.** That difference is the whole
/// of this walk. A group has no paint of its own and exists to hold layers
/// together, so an edit passes straight through it to the shapes inside —
/// selecting a group and setting a fill is the same act as selecting its
/// contents and setting a fill. A frame is drawn: it has paint of its own and
/// its contents are *its* contents, not the selection's, so it is the target and
/// the walk stops there. Setting a fill over a selected frame colours the frame,
/// not everything in it.
///
/// ⚠️ **A frame had an arm of its own here until 2026-09-01 and does not need
/// one now** (§15 D400): it pushed the id and descended no further, which is
/// exactly what the `takes_paint` arm does since a frame started answering `true`
/// to that. The arm is gone rather than kept as documentation, because a match
/// arm that cannot be reached is a claim the compiler stops checking — but the
/// *fact* it stated is load-bearing and is why this walk exists, so it is said
/// here instead: **a frame is a target, never a container to pass through.**
///
/// This is why paint uses its own walk rather than [`subtree_nodes`], which is
/// still the right one for the colour census: "what colours are in here" *does*
/// mean everything inside a frame, while "paint these" does not.
///
/// **A frame reached by passing through a group is a target too** — the case
/// the walk never met until frames could sit in groups (§15 D870). Selecting a row
/// of cards and setting a fill colours the cards, not what is on them.
pub fn paint_targets(doc: &Document, ids: &[NodeId]) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = outermost(doc, ids).into_iter().rev().collect();
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get(id) else { continue };
        match node.kind() {
            NodeKind::Group | NodeKind::Root => {
                stack.extend(node.children().iter().rev().copied());
            }
            k if k.takes_paint() => out.push(id),
            _ => {}
        }
    }
    out
}

// **`fills_seen_as` was here and is gone** (§15 D400). It answered "a node's
// fills as a bulk edit sees them", and the whole of its body was the frame case:
// a frame's one `background` presented as a one-entry fill list so that "set
// these all to red" could reach a frame and a rectangle in one gesture. Every
// other kind fell through to `paint.fills.clone()`. With a frame's fills in
// `Paint` like everyone else's, the function *is* `paint.fills.clone()`, so its
// six callers read the field — and the app's `DisplayNode`, which took the two
// pieces separately because it is not a `Node`, no longer needs a shared rule to
// agree with the committed tree about what a frame's fill is.

/// What a paint panel has to show over a scope.
///
/// Two states, and the line between them is whether the targets agree. Agreeing on
/// nothing is the empty panel; agreeing on a list is that list; **anything else at
/// all is one stand-in row**, including the case where one target is unpainted and
/// another is not.
///
/// This began as three states collapsed into two, on the reading that a stand-in row
/// needs every target to have something to stand for — so a disagreement with an
/// unpainted target among it showed as the empty panel. That was reported back
/// immediately, because it is the commonest disagreement there is and the panel was
/// denying a stroke that was on screen (§15 D53).
///
/// The whole **list**, not just the first brush: the Fill panel over a selection
/// shows the shared list and lets rows be added and removed like any other, so
/// what counts as agreement has to be the list itself.
#[derive(Clone, Debug, PartialEq)]
pub enum PaintShown<T> {
    /// These rows, and they are true of every target. Empty means no target has
    /// any paint at all — the only reading of "no fill" that is not a lie about
    /// somebody.
    List(Vec<T>),
    /// One stand-in row: the targets disagree, and at least one of them has a
    /// paint for the row to stand for.
    ///
    /// **One is enough, not all of them.** A layer with a stroke selected beside a
    /// layer without one is the commonest disagreement there is, and the panel
    /// used to report it as "No stroke" — which is false of half the selection,
    /// and false in the direction that matters, since the stroke it denied is the
    /// one on screen. It is mixed in every property at once: mixed colour, mixed
    /// position, mixed weight.
    ///
    /// What that row *says* is decided property by property
    /// ([`shared_over_fills`], [`shared_over_strokes`]) — the values they happen
    /// to share are shown as themselves, and only the rest reads as mixed.
    Mixed,
}

impl<T> PaintShown<T> {
    /// The rows to draw, which for [`PaintShown::Mixed`] is none of them: a mixed
    /// panel has one stand-in row and no list, so a caller asking for a list gets
    /// an empty one rather than a plausible lie.
    pub fn list(&self) -> &[T] {
        match self {
            Self::List(v) => v,
            Self::Mixed => &[],
        }
    }

    pub fn is_mixed(&self) -> bool {
        matches!(self, Self::Mixed)
    }

    /// Whether the panel has nothing at all to show — its empty state.
    pub fn unpainted(&self) -> bool {
        matches!(self, Self::List(v) if v.is_empty())
    }
}

/// Fold one list per target into the panel's answer.
fn paint_shown<T: PartialEq>(lists: impl Iterator<Item = Vec<T>>) -> PaintShown<T> {
    let mut first: Option<Vec<T>> = None;
    for list in lists {
        match &first {
            None => first = Some(list),
            // **Any disagreement at all is the mixed row**, including the one where
            // a target has no paint and another has some. There is no third answer
            // to check for: empty lists are all equal to each other, so reaching
            // here at all means something in scope is painted and there is
            // something for the row to stand for.
            Some(f) if *f != list => return PaintShown::Mixed,
            Some(_) => {}
        }
    }
    PaintShown::List(first.unwrap_or_default())
}

/// The fill list every paint target in scope shares — or a disagreement.
pub fn shared_fills(doc: &Document, ids: &[NodeId]) -> PaintShown<Fill> {
    paint_shown(
        paint_targets(doc, ids)
            .into_iter()
            .filter_map(|id| doc.get(id))
            .map(|n| n.paint().fills.clone()),
    )
}

/// The stroke list every paint target in scope shares — or a disagreement.
/// [`shared_fills`]' twin, over the same targets.
///
/// ⚠️ **"The same targets" is new** (§15 D400). A `strokeable_targets` sat between
/// this and [`paint_targets`], filtering it by a `takes_stroke` that existed for
/// one reason: a frame could be stroked and not filled, so the fill scope and the
/// stroke scope genuinely differed on that kind and only that kind. A frame fills
/// now, the two scopes are the same set, and the filter and the predicate are both
/// gone rather than left agreeing with each other.
pub fn shared_strokes(doc: &Document, ids: &[NodeId]) -> PaintShown<Stroke> {
    paint_shown(
        paint_targets(doc, ids)
            .into_iter()
            .filter_map(|id| doc.get(id))
            .map(|n| n.paint().strokes.clone()),
    )
}

/// The one value every fill of every paint target in scope agrees on, or `None`
/// when they differ or there are none.
///
/// Over **all** the fills of all the targets rather than one row of each. The
/// mixed row stands for every one of them, so the only values it may state as
/// facts are the ones they all state — and this is asked property by property,
/// which is what lets a selection that disagrees about how many fills it has
/// still show the colour they happen to share.
///
/// **A target with no fill at all disagrees with every value**, rather than being
/// skipped. That is the case worth spelling out: a layer with no stroke selected
/// beside one with a 4pt stroke does not have a 4pt stroke, and a row reporting one
/// would be stating a property half the selection has not got. Absence is a value
/// here, and the only one that agrees with nothing.
pub fn shared_over_fills<T: PartialEq>(
    doc: &Document,
    ids: &[NodeId],
    of: impl Fn(&Fill) -> T,
) -> Option<T> {
    let mut shared: Option<T> = None;
    let mut bare = false;
    for id in paint_targets(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        let fills = &node.paint().fills;
        bare |= fills.is_empty();
        for fill in fills {
            let v = of(fill);
            match &shared {
                None => shared = Some(v),
                Some(s) if *s == v => {}
                Some(_) => return None,
            }
        }
    }
    if bare && shared.is_some() {
        return None;
    }
    shared
}

/// The one value every stroke of every paint target in scope agrees on, or
/// `None` when they differ or there are none. [`shared_over_fills`]'s twin.
pub fn shared_over_strokes<T: PartialEq>(
    doc: &Document,
    ids: &[NodeId],
    of: impl Fn(&Stroke) -> T,
) -> Option<T> {
    let mut shared: Option<T> = None;
    let mut bare = false;
    for id in paint_targets(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        bare |= node.paint().strokes.is_empty();
        for stroke in &node.paint().strokes {
            let v = of(stroke);
            match &shared {
                None => shared = Some(v),
                Some(s) if *s == v => {}
                Some(_) => return None,
            }
        }
    }
    if bare && shared.is_some() {
        return None;
    }
    shared
}

/// Change every fill of every paint target in scope, in place.
///
/// **The non-destructive half of the bulk paint API**, and the reason a mixed
/// panel is not just a "Mixed" label with one overwrite behind it: each target
/// keeps its own list — its length, its order, and every property `edit` does not
/// touch — so setting a colour over a selection that disagrees sets the colour
/// and nothing else. [`set_fills_all`] is the other half, for when the intent
/// really is *whatever these are now, make them all this*.
///
/// **A frame takes the edit like anything else** (§15 D400). This used to fork on
/// the kind and pull only the *brush* back out of the edited stand-in, because a
/// frame's ground had no visibility flag — so "hide every fill in the selection"
/// silently skipped the frames in it. A frame's fills carry their own `visible`
/// now, and the fork is gone with the asymmetry that needed it.
pub fn edit_fills_all(doc: &Document, ids: &[NodeId], edit: impl Fn(&mut Fill)) -> Transaction {
    let mut ops = Vec::new();
    for id in paint_targets(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        let mut fills = node.paint().fills.clone();
        for fill in &mut fills {
            edit(fill);
        }
        if fills != node.paint().fills {
            ops.push(Operation::SetFills { id, fills });
        }
    }
    Transaction(ops)
}

/// Change every stroke of every paint target in scope, in place.
/// [`edit_fills_all`]'s twin, over the same targets — frames included, see
/// [`shared_strokes`].
pub fn edit_strokes_all(doc: &Document, ids: &[NodeId], edit: impl Fn(&mut Stroke)) -> Transaction {
    let mut ops = Vec::new();
    for id in paint_targets(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        let mut strokes = node.paint().strokes.clone();
        for stroke in &mut strokes {
            edit(stroke);
        }
        if strokes != node.paint().strokes {
            ops.push(Operation::SetStrokes { id, strokes });
        }
    }
    Transaction(ops)
}

/// Give every paint target in scope exactly this fill list.
///
/// **Exactly this list, on a frame too** (§15 D400). A frame used to take the
/// *first* fill and drop the rest, because it held one background rather than a
/// list — so a two-fill selection came back out of a frame as one, and the panel's
/// list stopped being the truth for part of what it was showing. That was the
/// "known limitation" `architecture.md` §5.7a recorded, and it is gone.
pub fn set_fills_all(doc: &Document, ids: &[NodeId], fills: &[Fill]) -> Transaction {
    let mut ops = Vec::new();
    for id in paint_targets(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        if node.paint().fills != fills {
            ops.push(Operation::SetFills {
                id,
                fills: fills.to_vec(),
            });
        }
    }
    Transaction(ops)
}

/// Give every paint target in scope exactly this stroke list — [`set_fills_all`]'s
/// twin, over the same targets, frames included.
pub fn set_strokes_all(doc: &Document, ids: &[NodeId], strokes: &[Stroke]) -> Transaction {
    let mut ops = Vec::new();
    for id in paint_targets(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        if node.paint().strokes != strokes {
            ops.push(Operation::SetStrokes {
                id,
                strokes: strokes.to_vec(),
            });
        }
    }
    Transaction(ops)
}

// **`any_strokeable_in` was here and is gone** (§15 D400). "Can anything in scope
// take a stroke" and "can anything in scope take a fill" were two questions while
// a frame answered them differently; they are one question now, and
// [`any_paint_in`] is it. The Stroke panel asks that instead — which is the only
// thing that ever called this.

/// One layer's **appearance**, lifted off it so another layer can be given the
/// same one — the payload behind *Copy properties* / *Paste properties*
/// (`docs/context-menus.md` §3's Properties group, `docs/shortcuts.md` §7).
///
/// **Three fields, and what is *not* here is the decision.** Position, size,
/// rotation, the name, the pivot and the lock are all excluded, because the verb
/// is "make this look like that" and every one of those is about *which layer this
/// is* rather than how it is drawn — Figma draws the same line. Geometry is
/// excluded for a second reason on top of that: a corner radius lives in
/// `NodeKind` and means nothing carried from a rect to a star, so it would be a
/// per-kind bridge rather than a field, and it is not in v1.
///
/// **A live text node's per-run colours are not here either**, and that is worth
/// knowing before the row is offered on one: character spans override the node's
/// paint, so pasting properties onto text whose runs carry their own colours
/// changes what is underneath them and nothing visible moves.
#[derive(Clone, Debug, PartialEq)]
pub struct Properties {
    /// The layer's own fill list — a frame's included, whole, since a frame keeps
    /// one like everything else (§15 D400). It used to be flattened through a
    /// `fills_seen_as` that gave a frame a one-entry stand-in list, which meant
    /// copying properties *off* a two-fill frame was not possible and pasting them
    /// *onto* one kept the first.
    pub fills: Vec<Fill>,
    pub strokes: Vec<Stroke>,
    pub opacity: f32,
}

/// A layer's appearance, or `None` for a kind that has none to give.
///
/// **A group is refused rather than answered with an empty list**, which is the
/// distinction [`paint_targets`] already draws and for the same reason: a group is
/// organisation, and copying "no fills and no strokes" off one would arrive at the
/// paste as an instruction to *clear* whatever it landed on. Its opacity is real
/// and is deliberately not a reason to allow it — one field is not an appearance,
/// and the Transform card already has that number.
pub fn properties_of(node: &Node) -> Option<Properties> {
    // One predicate, not a predicate and a frame-shaped `||` beside it (§15 D400).
    if !node.kind().takes_paint() {
        return None;
    }
    Some(Properties {
        fills: node.paint().fills.clone(),
        strokes: node.paint().strokes.clone(),
        opacity: node.opacity(),
    })
}

/// Give every target in scope the appearance in `props`.
///
/// **Three existing bulk verbs composed, not a fourth walk.** That is what makes
/// the row's scope answerable rather than a new rule to learn: the fills go where
/// a fill set from the panel goes ([`set_fills_all`], through a group and stopping
/// at a frame), the strokes where a stroke set goes ([`set_strokes_all`]), and the
/// opacity to the **outermost** members alone ([`set_opacity_all`], because
/// opacity composes down the tree and setting it twice would land at the square).
/// The two scopes differ, and they differ here exactly as they already differ
/// everywhere else in the app.
///
/// Each of the three drops the targets that already agree, so pasting the
/// properties a layer already has produces an empty transaction and no undo step.
pub fn paste_properties(doc: &Document, ids: &[NodeId], props: &Properties) -> Transaction {
    let mut ops = set_fills_all(doc, ids, &props.fills).0;
    ops.extend(set_strokes_all(doc, ids, &props.strokes).0);
    ops.extend(set_opacity_all(doc, ids, props.opacity).0);
    Transaction(ops)
}

/// Set the opacity of every **outermost** member of `ids`.
///
/// Outermost, not the whole subtree, and this is the one bulk edit where the
/// difference is visible: opacity composes down the tree, so setting 50% on a
/// group *and* on each of its children would land at 25% and the user would
/// have asked for one thing and got another.
pub fn set_opacity_all(doc: &Document, ids: &[NodeId], opacity: f32) -> Transaction {
    Transaction(
        outermost(doc, ids)
            .into_iter()
            .filter(|id| doc.get(*id).is_some_and(|n| n.opacity() != opacity))
            .map(|id| Operation::SetOpacity { id, opacity })
            .collect(),
    )
}

/// How near two opacities have to be for a readout to call them one value.
///
/// **The rule lives here rather than in the panel that applies it** (§15 D607).
/// The field formats as `{v:.0}%`, so anything this side of a part in a million is
/// the same number on screen and reporting *Mixed* for it would be the readout
/// disagreeing with itself.
///
/// 🚨 **This was a bare `==` here and `< 1e-6` in `inspector.rs`, and only the
/// panel's copy ever ran.** `[S3.2-L6-03]`: `build::shared_opacity` had **zero
/// callers anywhere in the tree, tests included**, so the exact rule was a claim
/// nothing made and nothing checked. It is deleted, and this constant is what is
/// left of it — the panel reads it, so the tolerance is one fact in one place
/// rather than a literal in the file furthest from the argument for it.
pub const OPACITY_AGREEMENT: f32 = 1e-6;

/// [`OPACITY_AGREEMENT`]'s twin for a corner radius, and the same story: exact
/// here, `< 1e-9` in the panel, and the panel's is what shipped (§15 D607).
///
/// Tighter than the opacity's because a radius is a length in document units
/// rather than a normalized fraction — 1e-9 of a unit is below anything a drag or
/// a parser can author, where 1e-6 of an opacity is a rounding artefact the
/// percentage field produces routinely.
pub const RADIUS_AGREEMENT: f64 = 1e-9;

/// Set the same radius on all four corners of every rect in the subtrees.
///
/// Subtree rather than outermost, unlike opacity: a radius does not compose, so
/// reaching into a selected group and rounding the rects inside it is both safe
/// and what "round these corners" means when a group is what is selected.
pub fn set_corner_radius_all(doc: &Document, ids: &[NodeId], radius: f64) -> Transaction {
    // Rects already at this radius contribute nothing. A field that loses focus
    // without having been edited would otherwise push an undo step made entirely
    // of no-ops.
    let already = |id: NodeId| {
        matches!(
            doc.get(id).map(|n| n.kind()),
            Some(NodeKind::Rect { corner_radii, .. })
                if crate::geometry::uniform_radius(corner_radii) == Some(radius)
        )
    };
    Transaction(
        subtree_nodes(doc, ids)
            .into_iter()
            .filter(|id| matches!(doc.get(*id).map(|n| n.kind()), Some(NodeKind::Rect { .. })))
            .filter(|id| !already(*id))
            .map(|id| Operation::SetGeometry {
                id,
                geometry: crate::op::GeometryPatch::CornerRadius(radius),
            })
            .collect(),
    )
}

/// The one uniform corner radius every rect in scope shares, or `None` when they
/// differ or none of them is a rect.
pub fn shared_corner_radius(doc: &Document, ids: &[NodeId]) -> Option<f64> {
    let mut shared: Option<f64> = None;
    for id in subtree_nodes(doc, ids) {
        let Some(NodeKind::Rect { corner_radii, .. }) = doc.get(id).map(|n| n.kind()) else {
            continue;
        };
        let r = crate::geometry::uniform_radius(corner_radii)?;
        match shared {
            None => shared = Some(r),
            // [`RADIUS_AGREEMENT`], not `==` — which is what the panel's twin has
            // always used, so this is the tested copy being brought onto the
            // shipped rule rather than the other way round (§15 D607).
            Some(s) if (s - r).abs() < RADIUS_AGREEMENT => {}
            Some(_) => return None,
        }
    }
    shared
}

/// Whether any node in scope is a rect, i.e. whether a corner-radius control has
/// anything to act on.
pub fn any_rect_in(doc: &Document, ids: &[NodeId]) -> bool {
    subtree_nodes(doc, ids)
        .into_iter()
        .any(|id| matches!(doc.get(id).map(|n| n.kind()), Some(NodeKind::Rect { .. })))
}

/// Whether any node in scope draws fills and strokes of its own — whether a Fill
/// or Stroke panel over this selection would have anything to write to.
///
/// A selection of nothing but empty groups has no paint anywhere in it, and a
/// colour chip offering to set one would be a control that does nothing when
/// clicked.
pub fn any_paint_in(doc: &Document, ids: &[NodeId]) -> bool {
    !paint_targets(doc, ids).is_empty()
}

// --- arrangement: align, distribute, z-order ------------------------------

/// Which axis an arrangement command works along.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    X,
    Y,
}

/// Which edge of the target box a selection aligns to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    Min,
    Mid,
    Max,
}

/// Align each of `ids` to `target` along one axis.
///
/// `target` is the caller's business — the selection's own union when several
/// layers are picked, the container's box when one is (that is a UI policy, not
/// a geometry one). Everything else is here so the app, MCP writes and the
/// command line cannot disagree about what "align left" moves.
pub fn align(
    doc: &Document,
    res: &Resolved,
    ids: &[NodeId],
    target: kurbo::Rect,
    axis: Axis,
    edge: Edge,
) -> Transaction {
    let mut ops = Vec::new();
    for id in outermost(doc, ids) {
        let Some(b) = res.world_bounds(id) else {
            continue;
        };
        let delta = match (axis, edge) {
            (Axis::X, Edge::Min) => Vec2::new(target.min_x() - b.min_x(), 0.0),
            (Axis::X, Edge::Mid) => Vec2::new(target.center().x - b.center().x, 0.0),
            (Axis::X, Edge::Max) => Vec2::new(target.max_x() - b.max_x(), 0.0),
            (Axis::Y, Edge::Min) => Vec2::new(0.0, target.min_y() - b.min_y()),
            (Axis::Y, Edge::Mid) => Vec2::new(0.0, target.center().y - b.center().y),
            (Axis::Y, Edge::Max) => Vec2::new(0.0, target.max_y() - b.max_y()),
        };
        push_offset(doc, res, id, delta, &mut ops);
    }
    Transaction(ops)
}

/// A box's leading edge along one axis — the edge an arrangement lays out from.
fn lead(r: &kurbo::Rect, axis: Axis) -> f64 {
    match axis {
        Axis::X => r.min_x(),
        Axis::Y => r.min_y(),
    }
}

/// A box's size along one axis.
fn extent(r: &kurbo::Rect, axis: Axis) -> f64 {
    match axis {
        Axis::X => r.width(),
        Axis::Y => r.height(),
    }
}

/// A move along one axis only.
fn along(axis: Axis, d: f64) -> Vec2 {
    match axis {
        Axis::X => Vec2::new(d, 0.0),
        Axis::Y => Vec2::new(0.0, d),
    }
}

/// The outermost members of `ids` that have world bounds, **sorted by where they
/// already sit** along `axis`.
///
/// Sorted rather than taken in selection order, so that arranging a set never
/// also reorders it: whatever is leftmost stays leftmost. Spelled once because
/// [`distribute`] and [`distribute_spacing`] both lay out in this order, and a
/// second copy is how the two would come to disagree about which layer is first.
/// [`gaps_along`] sorts the same way over boxes it is handed rather than over
/// ids it looks up, for the reason its own note gives.
fn sorted_along(
    doc: &Document,
    res: &Resolved,
    ids: &[NodeId],
    axis: Axis,
) -> Vec<(NodeId, kurbo::Rect)> {
    let mut boxes: Vec<(NodeId, kurbo::Rect)> = outermost(doc, ids)
        .into_iter()
        .filter_map(|id| res.world_bounds(id).map(|b| (id, b)))
        .collect();
    boxes.sort_by(|a, b| lead(&a.1, axis).total_cmp(&lead(&b.1, axis)));
    boxes
}

/// Space `ids` evenly along one axis: the outermost two stay put and the gaps
/// between every pair come out equal.
///
/// Equal *gaps*, not equal centres — with layers of different sizes those are
/// different results, and gaps are the one that looks right. Needs three layers
/// to mean anything; fewer returns an empty transaction.
///
/// ⚠️ **Equal gaps only while the layers fit in their own span** (§15 D487). Once
/// they overlap more than that, equal gaps and the selection's left-to-right order
/// are incompatible, and the **order** is what this keeps — the leading edges are
/// spaced evenly instead. `sorted_along` states the rule it is keeping.
///
/// **This one has no use for a key layer, and [`distribute_spacing`] does.**
/// Equalising gaps is bounded by the two extremes, and which layers those are is
/// set by where they already sit rather than by anything the user chose — so
/// there is nothing here for a designation to change. Fixing the gap instead
/// leaves the whole run free to slide, and then *which one holds still* is a
/// real question with no geometric answer.
pub fn distribute(doc: &Document, res: &Resolved, ids: &[NodeId], axis: Axis) -> Transaction {
    let boxes = sorted_along(doc, res, ids, axis);
    if boxes.len() < 3 {
        return Transaction(Vec::new());
    }
    let first_lead = lead(&boxes[0].1, axis);
    // ⚠️ **The greatest *trailing* edge, not the trailing edge of the box with the
    // greatest lead** (§15 D487). Sorted by lead, the last box is not necessarily
    // the one that reaches furthest: a background rect 800 wide at x=0 with two
    // 40-wide icons at 10 and 100 gave a span of 140 for a set that occupies 800.
    let span = boxes
        .iter()
        .map(|(_, b)| lead(b, axis) + extent(b, axis))
        .fold(f64::NEG_INFINITY, f64::max)
        - first_lead;
    let occupied: f64 = boxes.iter().map(|(_, b)| extent(b, axis)).sum();
    let gap = (span - occupied) / (boxes.len() - 1) as f64;

    let mut ops = Vec::new();
    if gap >= 0.0 {
        let mut cursor = first_lead;
        for (id, b) in &boxes {
            push_offset(doc, res, *id, along(axis, cursor - lead(b, axis)), &mut ops);
            cursor += extent(b, axis) + gap;
        }
        return Transaction(ops);
    }

    // **The layers overlap more than the span allows, and then the leading edges
    // are what gets spaced evenly** (§15 D487, `[S3.2-L1-01]`).
    //
    // ⚠️ **Because with overlap you cannot have all three of the things this
    // function promises**: both extremes held still, equal gaps, and the order left
    // alone. Equal gaps were taken and the order was the one that gave — a
    // background rect plus two icons came back with the icons **swapped**, the
    // middle layer travelling 420 units to land 330 to the right of the layer that
    // had been in front of it. One `SetTransform`, one undo step, no message.
    //
    // The order is the promise to keep. `sorted_along`'s own doc states it as a
    // rule — *"arranging a set never also reorders it: whatever is leftmost stays
    // leftmost"* — and a user watching two icons trade places has been handed
    // something they did not ask for, where a slightly uneven overlap is merely a
    // compromise. Even leads keep the extremes too: the first and last leads are
    // exactly where they were, so only the interior moves.
    //
    // This is also what the old comment claimed was already happening — *"the
    // layers then tuck back under each other evenly, which is still the even
    // distribution of what space there is"* — and it was true only for equal
    // extents. Three 100-wide rects at 0/10/20 are a fixed point under both
    // spellings; the moment the extents differ, the old one reorders.
    let step = (lead(&boxes[boxes.len() - 1].1, axis) - first_lead) / (boxes.len() - 1) as f64;
    for (i, (id, b)) in boxes.iter().enumerate() {
        let want = first_lead + step * i as f64;
        push_offset(doc, res, *id, along(axis, want - lead(b, axis)), &mut ops);
    }
    Transaction(ops)
}

/// The gap between each neighbouring pair of these boxes along `axis`, sorting
/// them by where they sit first.
///
/// One fewer number than there are boxes, and empty below two. A gap is
/// **negative where the two overlap**, which is a real state and not an error —
/// it is what [`distribute_spacing`] writes back when it is given a negative
/// number.
///
/// Returned as the whole list rather than as "the spacing, or `None` if they
/// disagree", deliberately: how close two gaps have to be before a panel calls
/// them one number is a question about how that panel *displays* a number, and
/// core has no business holding an opinion about it.
///
/// **Over boxes rather than over ids, and that is the fix for a real bug.** The
/// first spelling took `(doc, res, ids)` and read the *committed* tree, which is
/// the only tree core has — so the field showing this number sat still through
/// its own drag and the layers flickered between the committed arrangement and
/// one frame's worth of it (§15 D245). Which boxes to measure is a question with
/// a preview in it, and a preview is the app's; `session::preview_world_bounds`
/// is what the panel feeds this.
pub fn gaps_along(boxes: &[kurbo::Rect], axis: Axis) -> Vec<f64> {
    let mut sorted = boxes.to_vec();
    sorted.sort_by(|a, b| lead(a, axis).total_cmp(&lead(b, axis)));
    sorted
        .windows(2)
        .map(|pair| lead(&pair[1], axis) - (lead(&pair[0], axis) + extent(&pair[0], axis)))
        .collect()
}

/// Put exactly `gap` between each neighbouring pair of `ids` along `axis`,
/// holding `anchor` where it is.
///
/// **This is what a key layer is for** (§15 D243). [`distribute`] equalises the gaps inside
/// a span the extremes already fix, so nothing there can be chosen; naming the
/// gap frees the whole run to slide, and then one member has to hold still or
/// the arrangement has no position at all. `anchor` is that member — the key
/// layer, in the app — and everything before it on the axis is packed backwards
/// from it while everything after is packed forwards. An `anchor` that is not
/// among the members (or `None`) falls back to the **first**, which is the one
/// choice that cannot surprise: with no key, "exactly 24 apart" leaves the
/// leftmost or topmost layer exactly where it was.
///
/// **Two layers is enough**, unlike `distribute`, and that is not an oversight:
/// two layers have one gap, and one gap is a number worth being able to type.
/// The even distribution needs three because with two there is no interior.
///
/// Nothing is clamped. A negative `gap` overlaps the layers by that much, which
/// is what a designer typing `-2` is asking for.
pub fn distribute_spacing(
    doc: &Document,
    res: &Resolved,
    ids: &[NodeId],
    axis: Axis,
    anchor: Option<NodeId>,
    gap: f64,
) -> Transaction {
    let boxes = sorted_along(doc, res, ids, axis);
    if boxes.len() < 2 || !gap.is_finite() {
        return Transaction(Vec::new());
    }
    let at = anchor
        .and_then(|k| boxes.iter().position(|(id, _)| *id == k))
        .unwrap_or(0);

    let mut ops = Vec::new();
    // Forward from the anchor's trailing edge, then backward from its leading
    // one. The anchor itself is skipped by both, which is what "holds where it
    // is" means — it gets no operation at all rather than one that happens to
    // be zero.
    let mut cursor = lead(&boxes[at].1, axis) + extent(&boxes[at].1, axis) + gap;
    for (id, b) in &boxes[at + 1..] {
        push_offset(doc, res, *id, along(axis, cursor - lead(b, axis)), &mut ops);
        cursor += extent(b, axis) + gap;
    }
    let mut cursor = lead(&boxes[at].1, axis);
    for (id, b) in boxes[..at].iter().rev() {
        let start = cursor - gap - extent(b, axis);
        push_offset(doc, res, *id, along(axis, start - lead(b, axis)), &mut ops);
        cursor = start;
    }
    Transaction(ops)
}

/// Mirror `ids` across the centreline of the box they occupy.
///
/// `Axis::X` is the horizontal flip a designer means by that word: left and
/// right swap, so the mirror line is *vertical*. With one layer selected the box
/// is its own, so it flips in place; with several it is their union, so they
/// swap sides as a group — Figma's behaviour, and the only one that reads as
/// "flip this arrangement" rather than "flip each of these".
///
/// This writes a reflection into the transform, which §5.6 explicitly permits.
/// Nothing about the geometry changes: a mirrored rectangle is the same
/// rectangle seen from the other side.
///
/// **A single layer whose pivot has been moved mirrors about the pivot instead**
/// ([`crate::Pivot`]). Only when it has actually been moved: an unset pivot
/// resolves to the box's own centre, which for a shape is the same point as the
/// union centre below, but for a *rotated group* is not — the union of its
/// children's axis-aligned bounds is not the bounding box of their rotated union
/// — and there is no reason for a feature nobody has touched to move that flip by
/// a pixel. So the question asked is "has the user placed one", not "where does
/// one resolve to".
pub fn flip(doc: &Document, res: &Resolved, ids: &[NodeId], axis: Axis) -> Transaction {
    let members = outermost(doc, ids);
    // A set has no pivot to share — the union centreline is the one line all of
    // them agree on — so this is a single-selection question by construction.
    let placed = match members.as_slice() {
        [id] if doc.get(*id).is_some_and(|n| n.pivot().is_some()) => pivot_world(doc, res, *id),
        _ => None,
    };
    let Some(c) = placed.map(|p| p.to_vec2()).or_else(|| {
        members
            .iter()
            .filter_map(|id| res.world_bounds(*id))
            .reduce(|a, b| a.union(b))
            .map(|b| b.center().to_vec2())
    }) else {
        return Transaction(Vec::new());
    };
    let mirror = Affine::translate(c)
        * match axis {
            Axis::X => Affine::FLIP_X,
            Axis::Y => Affine::FLIP_Y,
        }
        * Affine::translate(-c);

    let ops = members
        .into_iter()
        .filter_map(|id| {
            let world = res.world_transform(id)?;
            Some(Operation::SetTransform {
                id,
                transform: local_for_world(doc, res, id, mirror * world),
            })
        })
        .collect();
    Transaction(ops)
}

/// Which axis a basis mirrors in, after rotating.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mirror {
    None,
    X,
    Y,
}

/// The rotation, skew and mirror a transform's 2×2 basis encodes (§5.6).
///
/// Split out because reading a rotation back as `atan2(b, a)` is wrong the
/// moment a node is mirrored: a reflection has a negative determinant, which
/// makes `atan2` report a nonsense angle, and rebuilding the transform from that
/// angle alone silently drops the mirror. Once skew exists it is wrong twice
/// over — a sheared basis' second column is not perpendicular to its first, so
/// the angle read off the *first* column is the only one that means anything,
/// and a rebuild from angle alone throws the skew away as well.
///
/// The two mirrors are the *same* reflection 180° apart — `R(θ)·FLIP_Y` is
/// `R(θ+π)·FLIP_X` — so a decomposition has to pick one to report. It picks
/// whichever leaves the smaller angle, which is what keeps the inspector honest:
/// flip an upright layer vertically and the rotation field stays at 0°, instead
/// of jumping to −180° for a layer that plainly has not been turned. The skew is
/// the same in both, so the choice does not disturb it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Orientation {
    /// Rotation in radians, in `(-π, π]`.
    pub angle: f64,
    /// Skew in radians, in `(-π/2, π/2)`: how far the y axis leans away from
    /// square with the x axis.
    ///
    /// SVG/CSS `skewX` semantics — the matrix coefficient is `tan(skew)`, so a
    /// point is displaced along +x in proportion to its +y. In the y-down space
    /// the document uses that leans the *bottom* of a shape to the right. The
    /// same convention the SVG writer emits, so an export needs no sign fixing.
    pub skew: f64,
    pub mirror: Mirror,
}

/// A basis split into the part a transform may carry and the part it may not.
///
/// §5.6 keeps *scale* out of the transform, so that a layer's size is one number
/// in one place (its geometry) and a stroke does not thicken when a group is
/// resized. Skew has no such home — there is no geometry field it could live in
/// — so it belongs with rotation and flip in the matrix.
///
/// The split is an RQ decomposition: every invertible 2×2 factors **uniquely**
/// as `R(θ)·skew·mirror · diag(sx, sy)`, with the scale on the right where the
/// node's own axes can absorb it into geometry. Being exact for *any* matrix is
/// the point: it is what lets a world-space edit be pushed onto a node whatever
/// its parents have done to it, instead of being approximated (§15 D50).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Basis {
    pub orientation: Orientation,
    /// The residual scale in the node's own axes — always positive, since the
    /// sign of a reflection is carried by [`Orientation::mirror`] instead.
    pub scale: Vec2,
}

/// A basis with no extent left to measure an angle or a lean against. Below this
/// the decomposition would be dividing by noise.
const DEGENERATE: f64 = 1e-12;

/// Wrap an angle into `(-π, π]`.
fn wrap_pi(a: f64) -> f64 {
    let t = std::f64::consts::TAU;
    let x = (a + std::f64::consts::PI).rem_euclid(t) - std::f64::consts::PI;
    // `rem_euclid` lands −π at −π; the half-open end is the positive one.
    if x <= -std::f64::consts::PI { x + t } else { x }
}

/// The affine that skews by `radians` about the origin (`skewX`).
pub fn skew_x(radians: f64) -> Affine {
    Affine::new([1.0, 0.0, radians.tan(), 1.0, 0.0, 0.0])
}

/// The affine that skews by `radians` about the origin along the *other* axis
/// (`skewY`) — a point displaced along +y in proportion to its +x.
///
/// Not a second case in the decomposition: `Orientation` reports one skew, and a
/// y-lean comes back out of it as a rotation plus an x-lean plus a scale, which
/// is the same matrix said differently. This exists because a *gesture* on a
/// vertical edge means the y one, and saying so directly is clearer than
/// spelling it as a conjugated `skew_x`.
pub fn skew_y(radians: f64) -> Affine {
    Affine::new([1.0, radians.tan(), 0.0, 1.0, 0.0, 0.0])
}

impl Basis {
    /// Split a transform's basis, ignoring its translation.
    ///
    /// Gram–Schmidt, which is the RQ decomposition written out: the first column
    /// gives the rotation and the x scale outright, and the second is then read
    /// in that frame — its component *across* the x axis is the y scale, and its
    /// component *along* it is the lean. Reconstructing with
    /// [`Orientation::to_affine`] times [`Self::scale`] returns the input.
    pub fn of(transform: Affine) -> Self {
        let [a, b, c, d, _, _] = transform.as_coeffs();
        // A reflection is peeled off first, by negating the x column: what is
        // left turns the right way round, so one unmirrored routine reads both.
        let mirrored = a * d - b * c < 0.0;
        let (a, b) = if mirrored { (-a, -b) } else { (a, b) };

        let sx = a.hypot(b);
        if sx < DEGENERATE {
            // Nothing to measure an angle against. Report the identity rather
            // than a NaN, so a collapsed node still round-trips to *something*
            // and the inspector shows 0° instead of blanking.
            return Basis {
                orientation: Orientation {
                    angle: 0.0,
                    skew: 0.0,
                    mirror: if mirrored { Mirror::X } else { Mirror::None },
                },
                scale: Vec2::new(0.0, c.hypot(d)),
            };
        }
        let (ux, uy) = (a / sx, b / sx);
        // Square to the x axis. The y column's component here is the height the
        // node actually has; its component along `u` is pure lean.
        let (vx, vy) = (-uy, ux);
        let sy = c * vx + d * vy;
        let skew = if sy.abs() < DEGENERATE {
            0.0
        } else {
            ((c * ux + d * uy) / sy).atan()
        };
        let angle = wrap_pi(uy.atan2(ux));

        // With the reflection peeled off the determinant is positive, so `sy`
        // came out positive too and the scale needs no sign.
        let orientation = match (mirrored, angle.abs() <= std::f64::consts::FRAC_PI_2) {
            (false, _) => Orientation {
                angle,
                skew,
                mirror: Mirror::None,
            },
            (true, true) => Orientation {
                angle,
                skew,
                mirror: Mirror::X,
            },
            // The far-side reading of the same reflection, which keeps the
            // reported angle small. `R(θ)·K·FLIP_X` is `R(θ+π)·K·FLIP_Y`.
            (true, false) => Orientation {
                angle: wrap_pi(angle - std::f64::consts::PI),
                skew,
                mirror: Mirror::Y,
            },
        };
        Basis {
            orientation,
            scale: Vec2::new(sx, sy),
        }
    }

    /// [`Self::of`] for callers that need the split to **reproduce** the input,
    /// `None` where no return value could (§15 D714).
    ///
    /// **`of` is total and lossy; this is partial and exact, and the difference
    /// is a whole class of silent wrong answers.** `Orientation::to_affine` is
    /// `R · skewX · mirror`, whose determinant is always ±1, so
    /// `orientation · diag(sx, sy)` can produce a second column of zero and
    /// nothing else once `sy` is zero. A **rank-1** basis has a second column
    /// that is not zero — `[1, 2, 2, 4]`'s is `(2, 4)` — so `of` cannot be exact
    /// for one and had no way to say so. It returned a plausible matrix instead,
    /// and `tools::split_geometry_scale` handed it on: the layer was rewritten
    /// **1 unit tall at 63°** (`[S13.1-L1-07]`).
    ///
    /// **Both of `of`'s degenerate arms are refused**, and the second is the one
    /// a reader misses: `Affine::scale(1e-13)` is invertible in principle and
    /// falls under `DEGENERATE`, so `of` reports `sx = 0` with an *identity*
    /// orientation — under `Scaling::Photographic` that makes
    /// `s = sqrt(0 · sy) = 0` and zeroes every stroke width in the node.
    ///
    /// ⚠️ **The thresholds are not restated here.** This reads the numbers `of`
    /// already computed — `scale.x` and `scale.y` — rather than recomputing `sx`
    /// and `sy` beside it, so the guard cannot drift away from the arms it is
    /// guarding. A non-finite input makes both comparisons false through `NaN`,
    /// and [`affine_is_finite`] is spelled anyway because relying on that is a
    /// subtlety rather than an argument.
    ///
    /// **Who should still use [`Self::of`]:** anything *reporting* a basis rather
    /// than rewriting one. The inspector's angle and scale fields want a number
    /// for a collapsed node — 0° rather than a blank — which is exactly what `of`
    /// is total for.
    pub fn exact(transform: Affine) -> Option<Self> {
        let basis = Self::of(transform);
        (affine_is_finite(transform)
            && basis.scale.x >= DEGENERATE
            && basis.scale.y.abs() >= DEGENERATE)
            .then_some(basis)
    }
}

impl Orientation {
    /// Read the orientation out of a transform, ignoring its translation and
    /// whatever scale it carries. [`Basis::of`] keeps the scale.
    pub fn of(transform: Affine) -> Self {
        Basis::of(transform).orientation
    }

    /// The basis this describes, with no translation and no scale.
    pub fn to_affine(self) -> Affine {
        let r = Affine::rotate(self.angle) * skew_x(self.skew);
        match self.mirror {
            Mirror::None => r,
            Mirror::X => r * Affine::FLIP_X,
            Mirror::Y => r * Affine::FLIP_Y,
        }
    }
}

/// Where a z-order command moves the selection among its siblings.
///
/// A parent paints its children in list order, so later means nearer the
/// viewer: `Forward` raises the index.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ZMove {
    Forward,
    Backward,
    Front,
    Back,
}

/// Restack `ids` within each of their parents.
///
/// A selection can span several parents; each is restacked independently, since
/// "bring to front" means "to the front of wherever you are". Members that are
/// descendants of other members are dropped — their index means nothing to the
/// user who selected the ancestor.
pub fn z_order(doc: &Document, ids: &[NodeId], mv: ZMove) -> Transaction {
    let mut by_parent: FxHashMap<NodeId, FxHashSet<NodeId>> = FxHashMap::default();
    for id in outermost(doc, ids) {
        if let Some(p) = doc.get(id).and_then(|n| n.parent()) {
            by_parent.entry(p).or_default().insert(id);
        }
    }
    // Sorted: an operation list has to replay identically on another replica
    // (§12), so it cannot depend on hash iteration order.
    let mut parents: Vec<NodeId> = by_parent.keys().copied().collect();
    parents.sort();

    let mut ops = Vec::new();
    for parent in parents {
        let Some(node) = doc.get(parent) else {
            continue;
        };
        let current: Vec<NodeId> = node.children().to_vec();
        let desired = restacked(&current, &by_parent[&parent], mv);
        ops.extend(reorder_ops(&current, &desired));
    }
    Transaction(ops)
}

/// `current` with the selected members restacked. Same members, new order.
fn restacked(current: &[NodeId], sel: &FxHashSet<NodeId>, mv: ZMove) -> Vec<NodeId> {
    let (mut picked, mut rest): (Vec<NodeId>, Vec<NodeId>) =
        current.iter().partition(|c| sel.contains(c));
    match mv {
        ZMove::Front => {
            rest.extend(picked);
            rest
        }
        ZMove::Back => {
            picked.extend(rest);
            picked
        }
        // One step means trading places with the nearest *unselected* neighbour
        // on that side. Walking from the far end keeps a run of adjacent
        // selected layers together instead of shuffling them within the run.
        ZMove::Forward => {
            let mut out = current.to_vec();
            for i in (0..out.len().saturating_sub(1)).rev() {
                if sel.contains(&out[i]) && !sel.contains(&out[i + 1]) {
                    out.swap(i, i + 1);
                }
            }
            out
        }
        ZMove::Backward => {
            let mut out = current.to_vec();
            for i in 1..out.len() {
                if sel.contains(&out[i]) && !sel.contains(&out[i - 1]) {
                    out.swap(i, i - 1);
                }
            }
            out
        }
    }
}

/// The `Reorder` ops that turn `current` into `desired` (same members).
///
/// Each op's index is the slot the node lands in *after* being lifted out of
/// its old one, so the list only composes correctly if every op is computed
/// against the order the previous ops leave behind — hence the simulation.
fn reorder_ops(current: &[NodeId], desired: &[NodeId]) -> Vec<Operation> {
    let mut list = current.to_vec();
    let mut ops = Vec::new();
    for (slot, want) in desired.iter().enumerate() {
        if list.get(slot) == Some(want) {
            continue;
        }
        let from = list
            .iter()
            .position(|c| c == want)
            .expect("desired holds the same members as current");
        list.remove(from);
        list.insert(slot, *want);
        ops.push(Operation::Reorder {
            id: *want,
            index: slot,
        });
    }
    ops
}

/// Push the `SetTransform` that shifts `id` by a world-space `delta`, skipping
/// no-ops so an align that changes nothing is an empty transaction rather than
/// an undo step.
fn push_offset(doc: &Document, res: &Resolved, id: NodeId, delta: Vec2, ops: &mut Vec<Operation>) {
    if delta.hypot() < 1e-9 {
        return;
    }
    let Some(world) = res.world_transform(id) else {
        return;
    };
    ops.push(Operation::SetTransform {
        id,
        transform: local_for_world(doc, res, id, Affine::translate(delta) * world),
    });
}

/// `tx` with each resized **flex item**'s properties set so the size it was
/// resized to holds (§15 D875, the maintainer's ruling: *"set width and stop
/// growth"*).
///
/// A shape's or a frame's stored size is its CSS width and height (§15 D872), so
/// a resize tool's own `SetGeometry` already sets the size — but in a flex row a
/// growing or shrinking item would take the space back and the drag would snap.
/// So a resize that changes an in-flow item's **main-axis** size also writes
/// `flex-grow: 0` and `flex-shrink: 0`; one that changes the **cross** size of an
/// item its container stretches also writes `align-self: start`, since in CSS an
/// explicit cross size is what stops a stretch — or `align-self: end` when the
/// resize held the cross axis's end edge and moved its start, a row's item dragged
/// by its top (§15 D905), so the edge the user did not touch is the one that stays.
/// A layer whose item properties the transaction sets itself is left to it.
///
/// The item half is [`flex_holds`], which the gesture preview applies as well
/// (§15 D904); this adds [`kept_flow_translations`], which the preview must not.
///
/// **And a resize writes the size in px, whatever keyword was there** (§15 D879):
/// a `SetGeometry` that changes a layer's size on an axis whose `width` or
/// `height` is `%` or `fit-content` sets that axis back to `auto` — which, for a
/// kind with a stored size, *is* its stored size — so a drag or a typed number on a
/// hugging frame or a percentage-wide item holds instead of snapping back to the
/// keyword. Any parent, laid out or not: a frame hugging its content at the top
/// of the page is the commonest case of it.
///
/// **And every `SetTransform` on an in-flow item keeps the item's stored
/// translation** ([`kept_flow_translations`], §15 D877's amendment) — the other
/// half of making a tool's edit of a flex item mean what is drawn; a write left
/// changing nothing is dropped, so this can empty a transaction.
///
/// Run beside [`keep_insets`], at the same door and for the same reason: tools
/// compute in drawn geometry and write what they want drawn; this is the one place
/// that turns a resize into what makes it stick.
pub fn keep_flex_sizes(doc: &Document, res: &Resolved, tx: Transaction) -> Transaction {
    let holds = flex_holds(doc, res, &tx);
    let mut out = kept_flow_translations(doc, tx);
    out.0.extend(holds);
    out
}

/// The `SetLayoutItem`s that make `tx`'s resizes hold — [`keep_flex_sizes`]' rules
/// for growth, `align-self` and size keywords, and in a grid both self-alignments
/// ([`grid_resize_held`], §15 D916), and nothing else — in id order.
///
/// **Split out so the gesture preview can apply them** (§15 D904). The preview
/// re-lays every flex container a transaction touches (`RenderOverrides`'
/// `flex_relayout`), and it did so with the item's properties as stored: a
/// stretched item being dragged shorter was stretched straight back every frame,
/// and landed at the dragged height only on release, when the commit added
/// `align-self`. A growing item dragged narrower, and a percentage-wide one, did
/// the same on the main axis. The Transform card's H scrubbed against the same
/// preview and so could not move at all. The preview must not take
/// [`kept_flow_translations`] too: that drops a move's translation, and a move is
/// what the preview draws.
pub fn flex_holds(doc: &Document, res: &Resolved, tx: &Transaction) -> Vec<Operation> {
    use crate::container::{self, Display, LayoutView};
    let view = crate::resolve::DocView(doc);
    let explicit: FxHashSet<NodeId> =
        tx.0.iter()
            .filter_map(|op| match op {
                Operation::SetLayoutItem { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
    let mut items: FxHashMap<NodeId, crate::container::LayoutItem> = FxHashMap::default();
    for op in &tx.0 {
        let Operation::SetGeometry { id, geometry } = op else {
            continue;
        };
        if !geometry.resizes() || explicit.contains(id) {
            continue;
        }
        let Some(used) = res.used_kind(doc, *id) else {
            continue;
        };
        let Some(now) = crate::geometry::local_bounds(used, None) else {
            continue;
        };
        let Some(then) = geometry
            .applied_to(used)
            .and_then(|k| crate::geometry::local_bounds(&k, None))
        else {
            continue;
        };
        let item = *items.entry(*id).or_insert_with(|| view.item(*id));
        let mut item = sized_in_px(item, now.size(), then.size());
        if container::parent_lays_out(&view, *id)
            && container::in_flow(&view, *id)
            && let Some(display) = view.parent(*id).and_then(|p| view.display(p))
        {
            // Where the box was and where the tool wants it, each in the parent's
            // space: its slot, and the transform this edit writes for it — a
            // left- or top-handle resize shifts the origin to hold the far edge.
            let was = res.used_local(doc, *id).unwrap_or(Affine::IDENTITY);
            let to = written_transform(tx, *id).unwrap_or(was);
            let edges = resized_edges((was, now), (to, then));
            item = match display {
                Display::Flex(flex) => {
                    let from_start = resized_from_cross_start(flex, edges);
                    held(flex, item, now.size(), then.size(), from_start)
                }
                Display::Grid(grid) => grid_resize_held(
                    grid,
                    item,
                    container::is_replaced(&view, *id),
                    (now.size(), then.size()),
                    (edges[0].0, edges[1].0),
                ),
            };
        }
        items.insert(*id, item);
    }
    let mut ids: Vec<(NodeId, crate::container::LayoutItem)> = items
        .into_iter()
        .filter(|(id, item)| doc.get(*id).is_some_and(|n| n.item() != item))
        .collect();
    ids.sort_by_key(|(id, _)| *id);
    ids.into_iter()
        .map(|(id, item)| Operation::SetLayoutItem { id, item })
        .collect()
}

/// The last transform `tx` writes for `id`, if it writes one.
fn written_transform(tx: &Transaction, id: NodeId) -> Option<Affine> {
    tx.0.iter().rev().find_map(|op| match op {
        Operation::SetTransform { id: i, transform } if *i == id => Some(*transform),
        _ => None,
    })
}

/// Whether a resize of an item laid out by `flex` **kept its cross-axis end edge
/// and moved its start** — a row's item dragged by its top, a column's by its left,
/// and the other way round under `wrap-reverse` (§15 D905) — read off `edges`,
/// [`resized_edges`]' physical answer, through the flow.
///
/// This decides between `align-self: flex-start` and `flex-end` when a resize
/// releases a stretch: the edge the user held is the one the alignment keeps, so the
/// item does not jump to the other side of its line on release. Those are the flow's
/// cross-start and cross-end, which `wrap-reverse` swaps — the bottom of a row's line
/// is its start there (§15 D909). This read the parent's physical axes while the
/// model's `Start` was mapped to CSS's physical `start`; the two moved together.
///
/// Every other resize answers `false` — from the end edge, or about the centre,
/// where both edges move and `flex-start` is the rule it always was.
fn resized_from_cross_start(flex: &crate::container::Flex, edges: [(bool, bool); 2]) -> bool {
    let [across, down] = edges;
    let (low_moved, high_moved) = if flex.direction.is_row() {
        down
    } else {
        across
    };
    match flex.wrap {
        crate::container::FlexWrap::WrapReverse => high_moved,
        _ => low_moved,
    }
}

/// For each physical axis — across, then down — whether a resize **moved the
/// box's low edge and held its high one** (left or top dragged), and whether it
/// did the reverse. `was` and `now` are the box in its own space with the local
/// transform placing it, before and after. About the centre, both edges move and
/// both answers are `false`.
///
/// Flex reads one axis of it, through its flow ([`resized_from_cross_start`]);
/// grid reads both, physically, having no reversal (§15 D913's second ruling).
fn resized_edges(was: (Affine, kurbo::Rect), now: (Affine, kurbo::Rect)) -> [(bool, bool); 2] {
    let a = was.0.transform_rect_bbox(was.1);
    let b = now.0.transform_rect_bbox(now.1);
    // A resize holds its anchor through a product of transforms, so "held" means
    // to within rounding, not bit for bit.
    let held = |p: f64, q: f64| (p - q).abs() < 1e-6;
    let axis = |lo: (f64, f64), hi: (f64, f64)| {
        (
            held(hi.0, hi.1) && !held(lo.0, lo.1),
            held(lo.0, lo.1) && !held(hi.0, hi.1),
        )
    };
    [
        axis((a.x0, b.x0), (a.x1, b.x1)),
        axis((a.y0, b.y0), (a.y1, b.y1)),
    ]
}

/// `item` with a released stretch aligned on whichever axes a resize from `from`
/// to `to` changed, in a container laid out by `grid` — §15 D913's second ruling,
/// [`held`]'s for grid: the size is written by the resize itself, and a stretch
/// that would undo it becomes `start`, or `end` when the resize moved the axis's
/// low edge and held its high one (`low_moved`, across then down), D905's rule on
/// both axes. The item keeps its cells.
///
/// **"Stretched" is the resolved alignment** — the item's own, else the
/// container's — and under `normal` a box stretches and a replaced item does not
/// (§15 D915), so a shape resized under the default writes nothing here.
fn grid_resize_held(
    grid: &crate::container::Grid,
    mut item: crate::container::LayoutItem,
    replaced: bool,
    (from, to): (kurbo::Size, kurbo::Size),
    low_moved: (bool, bool),
) -> crate::container::LayoutItem {
    use crate::container::AlignItems;
    let release = |own: &mut Option<AlignItems>, items: Option<AlignItems>, low: bool| {
        let stretched = match own.or(items) {
            Some(a) => a == AlignItems::Stretch,
            None => !replaced,
        };
        if stretched {
            *own = Some(if low {
                AlignItems::End
            } else {
                AlignItems::Start
            });
        }
    };
    if (from.width - to.width).abs() > 1e-9 {
        release(&mut item.justify_self, grid.justify_items, low_moved.0);
    }
    if (from.height - to.height).abs() > 1e-9 {
        release(&mut item.align_self, grid.align_items, low_moved.1);
    }
    item
}

/// `tx` with every `SetTransform` on an **in-flow flex item** keeping the item's
/// stored translation — its new rotation, skew and flip taken, its position not
/// (§15 D877's amendment). A grid item too, since [`is_flex_item`] answers for
/// any layout (§15 D914).
///
/// **The tools write where they want a layer drawn, and for an item in a flow
/// that is not where it is drawn.** A resize from a left or top handle holds the
/// opposite edge by shifting the box's origin, and builds that shift on the
/// item's *used* transform — its slot. Measured on a 40-wide rect laid at (20,
/// 20) with a stored translation of (300, 150): a left-handle resize to 60 wide
/// wrote `SetTransform` translate(0, 20) — the slot, shifted — beside its
/// `SetGeometry`. Drawn nowhere while the item is in the flow, since its
/// container places it (§15 D875); and when the layout is taken away the item
/// would land at (0, 20) rather than where its own transform had it. A rotation
/// about a pivot is expected to do the same — `rotate_node` also composes on the
/// used transform — which is read, not measured. So every such write keeps the stored
/// translation, here, once, rather than in every tool — the move's own answer
/// (§15 D877), which stores nothing at all, arriving for the others.
///
/// ⚠️ **Not when the same transaction takes the item out of the flow** — a
/// reparent, a delete, insets, hiding it, making it a mask, or its parent's
/// layout changing: then the transform is where it will be drawn, and it stands.
fn kept_flow_translations(doc: &Document, tx: Transaction) -> Transaction {
    let leaves: FxHashSet<NodeId> =
        tx.0.iter()
            .filter_map(|op| match op {
                Operation::Reparent { id, .. }
                | Operation::DeleteNode { id }
                | Operation::SetInsets { id, .. }
                | Operation::SetVisible { id, .. }
                | Operation::SetMask { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
    let relaid: FxHashSet<NodeId> =
        tx.0.iter()
            .filter_map(|op| match op {
                Operation::SetDisplay { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
    // A write left changing nothing — a resize's shift, with the linear part as
    // it was — is dropped rather than kept as a no-op step in history.
    let ops =
        tx.0.into_iter()
            .filter_map(|op| match op {
                Operation::SetTransform { id, transform }
                    if is_flex_item(doc, id)
                        && !leaves.contains(&id)
                        && !doc
                            .get(id)
                            .and_then(|n| n.parent())
                            .is_some_and(|p| relaid.contains(&p)) =>
                {
                    let stored = doc.get(id).map_or(transform, |n| n.transform());
                    let kept = transform.with_translation(stored.translation());
                    (kept.as_coeffs() != stored.as_coeffs()).then_some(Operation::SetTransform {
                        id,
                        transform: kept,
                    })
                }
                op => Some(op),
            })
            .collect();
    Transaction(ops)
}

/// `item` with `width` and `height` back at `auto` on whichever axes a resize from
/// `from` to `to` changed — [`keep_flex_sizes`]' px rule (§15 D879), for a kind
/// whose stored size is its CSS size, where `auto` is that stored size and so the
/// size the resize has just written.
fn sized_in_px(
    mut item: crate::container::LayoutItem,
    from: kurbo::Size,
    to: kurbo::Size,
) -> crate::container::LayoutItem {
    use crate::container::Dimension;
    if (from.width - to.width).abs() > 1e-9 {
        item.width = Dimension::Auto;
    }
    if (from.height - to.height).abs() > 1e-9 {
        item.height = Dimension::Auto;
    }
    item
}

/// `item` with growth stopped on whichever axes a resize from `from` to `to`
/// changed, in a container laid out by `flex` — [`keep_flex_sizes`]' rule (§15
/// D875), spelled once for it and for [`sized_flex_item`]. A released stretch
/// aligns to the end when `from_start` — the resize moved the cross axis's start
/// edge and held its end ([`resized_from_cross_start`], §15 D905) — and to the
/// start otherwise.
fn held(
    flex: &crate::container::Flex,
    mut item: crate::container::LayoutItem,
    from: kurbo::Size,
    to: kurbo::Size,
    from_start: bool,
) -> crate::container::LayoutItem {
    use crate::container::AlignItems;
    let dw = (from.width - to.width).abs() > 1e-9;
    let dh = (from.height - to.height).abs() > 1e-9;
    let (main_changed, cross_changed) = if flex.direction.is_row() {
        (dw, dh)
    } else {
        (dh, dw)
    };
    if main_changed {
        item.grow = 0.0;
        item.shrink = 0.0;
    }
    let stretched = match item.align_self {
        Some(a) => a == AlignItems::Stretch,
        None => flex.align_items == AlignItems::Stretch,
    };
    if cross_changed && stretched {
        item.align_self = Some(match from_start {
            true => AlignItems::End,
            false => AlignItems::Start,
        });
    }
    item
}

/// The flex-item properties that make a **group with a layout** `size` — the
/// resize of a box that has no size field of its own (§15 D869, D875).
///
/// A shape's or a frame's stored size is its CSS width and height, so a resize
/// writes its geometry and [`keep_flex_sizes`] holds it. A group with a layout
/// has only its box, which is `width`/`height` on its [`crate::container::LayoutItem`]
/// — `auto`, hugging its contents, until something sets them. So a resize writes
/// them in px — each only if the resize changed it, so an axis left alone keeps
/// hugging (§15 D879) — and **its growth is stopped here rather than by
/// `keep_flex_sizes`**, which leaves a transaction's own `SetLayoutItem` alone.
///
/// `to` is the local transform the same edit writes for the group, if it writes
/// one — a left- or top-handle resize shifts the origin to hold the far edge — and
/// is what tells a released stretch which edge to keep (§15 D905).
///
/// `None` when `id` is not a group with a layout, or nothing changes.
pub fn sized_flex_item(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    size: kurbo::Size,
    to: Option<Affine>,
) -> Option<crate::container::LayoutItem> {
    use crate::container::{self, Dimension, Display, LayoutView};
    let node = doc.get(id)?;
    if !matches!(node.kind(), NodeKind::Group) || node.display().is_none() {
        return None;
    }
    let mut item = *node.item();
    // **Only the axes the resize changed** (§15 D879's px rule, which leaves an
    // unchanged axis's keyword alone): a side handle or a typed W on a group that
    // hugs its height keeps it hugging. With no box to compare against, both.
    let now = res.used_frame(id);
    let changed = |was: Option<f64>, to: f64| was.is_none_or(|w| (w - to).abs() > 1e-9);
    if changed(now.map(|n| n.width), size.width) {
        item.width = Dimension::Px(size.width);
    }
    if changed(now.map(|n| n.height), size.height) {
        item.height = Dimension::Px(size.height);
    }
    let view = crate::resolve::DocView(doc);
    if container::parent_lays_out(&view, id)
        && container::in_flow(&view, id)
        && let Some(display) = node.parent().and_then(|p| view.display(p))
        && let Some(now) = now
    {
        let was = res.used_local(doc, id).unwrap_or(Affine::IDENTITY);
        let edges = resized_edges(
            (was, kurbo::Rect::from_origin_size(kurbo::Point::ZERO, now)),
            (
                to.unwrap_or(was),
                kurbo::Rect::from_origin_size(kurbo::Point::ZERO, size),
            ),
        );
        item = match display {
            Display::Flex(flex) => {
                held(flex, item, now, size, resized_from_cross_start(flex, edges))
            }
            // A group with a layout is a box, never replaced (§15 D915).
            Display::Grid(grid) => {
                grid_resize_held(grid, item, false, (now, size), (edges[0].0, edges[1].0))
            }
        };
    }
    (item != *node.item()).then_some(item)
}

/// The `Reorder` that dragging the flex item `id` by the world-space `delta`
/// asks for — where in its container's flow its centre now falls (§15 D877).
///
/// **A drag inside a flex container is a reorder, not a move.** An in-flow
/// item is placed by its container, not by its transform (§15 D875), so a
/// translation dropped on it is undone by the next layout pass; what the drag
/// can change is the item's place in the flow, which is its place among its
/// siblings. So the dragged box's centre is read against every other in-flow
/// sibling's used box, in the container's own space, in **reading order**: a
/// sibling on the centre's line (its cross-axis extent spans the centre) comes
/// before it when its main-axis centre does, and one on another line when that
/// line does — which is what makes a wrapping row reorder by line, then along it.
/// `row-reverse` and `column-reverse` read the main axis backwards, and
/// `wrap-reverse` the cross axis: its first line is the one at the bottom of a row
/// (the right of a column), so a line further *down* comes first (§15 D883 — read as
/// `wrap` until the Container card made the value reachable, §15 D878).
///
/// Siblings out of the flow — pinned, hidden, masks — keep their indices
/// relative to the flow around them; the result is an index into the child list
/// with `id` taken out, which is what `Reorder` means. `None` when `id` is not an
/// in-flow flex item, or would land where it is.
pub fn flex_reorder(doc: &Document, res: &Resolved, id: NodeId, delta: Vec2) -> Option<Operation> {
    if !is_flex_item(doc, id) {
        return None;
    }
    let parent = doc.get(id)?.parent()?;
    let others: Vec<NodeId> = doc
        .get(parent)?
        .children()
        .iter()
        .copied()
        .filter(|c| *c != id)
        .collect();
    let at = res.world_bounds(id)?.center() + delta;
    let (index, place) = flow_index(doc, res, parent, &others, at)?;
    // **Where it lands in the flow, not in the child list, decides whether it
    // moved** (§15 D934): the index is just before the next in-flow sibling, so
    // a pinned or hidden sibling after the item made a nudge that kept its slot
    // answer a `Reorder` past it — the paint order flipped and an undo step
    // landed with nothing moved in the flow.
    let view = crate::resolve::DocView(doc);
    let was = others
        .iter()
        .take(doc.get(parent)?.children().iter().position(|c| *c == id)?)
        .filter(|c| crate::container::in_flow(&view, **c))
        .count();
    (place != was).then_some(Operation::Reorder { id, index })
}

/// [`flex_reorder`] for **several items of one flex container dragged
/// together** (§15 D902): they move as a block to where the block's centre falls,
/// keeping their order among themselves — Figma's multi-item drag in auto layout.
/// `None` unless every one of `ids` is an in-flow item of the same container;
/// otherwise the `Reorder`s that turn the child list into the new one, empty when
/// the block lands where it is.
///
/// The block's centre is the centre of the union of their drawn boxes, read
/// against the *other* in-flow siblings by [`flow_index`], the same reading order
/// one item is. The ops are derived by walking the target order and moving
/// whatever is out of place into it — correct by construction, where placing each
/// dragged item at its final index in turn is not: worked by hand, `[a, d1, b, d2,
/// c]` with `d1, d2` dropped after `c` goes `[a, b, d2, d1, c]` after the first
/// such op and `[a, b, d1, c, d2]` after the second.
pub fn flex_reorder_many(
    doc: &Document,
    res: &Resolved,
    ids: &[NodeId],
    delta: Vec2,
) -> Option<Vec<Operation>> {
    let first = *ids.first()?;
    let parent = doc.get(first)?.parent()?;
    if ids
        .iter()
        .any(|id| !is_flex_item(doc, *id) || doc.get(*id).and_then(|n| n.parent()) != Some(parent))
    {
        return None;
    }
    let children = doc.get(parent)?.children().to_vec();
    let block: Vec<NodeId> = children
        .iter()
        .copied()
        .filter(|c| ids.contains(c))
        .collect();
    let others: Vec<NodeId> = children
        .iter()
        .copied()
        .filter(|c| !ids.contains(c))
        .collect();
    let at = ids
        .iter()
        .filter_map(|id| res.world_bounds(*id))
        .reduce(|a, b| a.union(b))?
        .center()
        + delta;
    let (index, _) = flow_index(doc, res, parent, &others, at)?;
    let mut target = others;
    for (k, id) in block.iter().enumerate() {
        target.insert(index + k, *id);
    }
    // **The flow order unchanged is no reorder** (§15 D934), whatever the child
    // list would say: an out-of-flow sibling between the block's members was
    // pulled out from between them on any drop, a nudge included.
    let view = crate::resolve::DocView(doc);
    let flow = |list: &[NodeId]| -> Vec<NodeId> {
        list.iter()
            .copied()
            .filter(|c| crate::container::in_flow(&view, *c))
            .collect()
    };
    if flow(&target) == flow(&children) {
        return Some(Vec::new());
    }
    let mut now = children;
    let mut ops = Vec::new();
    for (i, id) in target.iter().enumerate() {
        if now[i] != *id {
            let from = now.iter().position(|c| c == id)?;
            now.remove(from);
            now.insert(i, *id);
            ops.push(Operation::Reorder { id: *id, index: i });
        }
    }
    Some(ops)
}

/// Where in `parent`'s child list — `others`, the list with the dragged taken
/// out — a dragged box whose centre is at world `at` lands: [`flex_reorder`]'s
/// reading-order rule, shared with [`flex_reorder_many`] so the two cannot drift.
/// Answers the child-list index and the **place in the flow** — how many in-flow
/// siblings come before it — which is what says whether anything moved.
///
/// 🚨 **The lines are the laid lines, read off the flow** (§15 D934). A new line
/// starts where an in-flow sibling's main-axis start runs back behind the end of
/// the one before it, in flow order — which is how the engine fills them — and a
/// line's cross extent is the union of its items'. So a `nowrap` row is one line
/// by construction, and `wrap-reverse` needs no arm of its own: its first line is
/// the first in the flow wherever it is drawn (§15 D883). The dragged centre's
/// line is the one whose band holds it, else the nearest; it lands after every
/// line before that one and among that line's items by main-axis centre.
/// "Same line" was asked of each sibling's own cross extent, so in a row aligned
/// to the start a 20-tall item never held a 100-tall one's centre: the taller
/// item read every shorter one as a line above and could never be dropped before
/// it, and a drag that wobbled past half an item's height landed first or last
/// whatever its x.
fn flow_index(
    doc: &Document,
    res: &Resolved,
    parent: NodeId,
    others: &[NodeId],
    at: Point,
) -> Option<(usize, usize)> {
    use crate::container::{self, Display, FlexDirection};
    let view = crate::resolve::DocView(doc);
    // A grid's drop writes lines rather than reordering (§15 D913); not this.
    let Display::Flex(flex) = doc.get(parent)?.display()? else {
        return None;
    };
    let to_parent = res.world_transform(parent)?.inverse();
    let at = to_parent * at;
    let reversed = matches!(
        flex.direction,
        FlexDirection::RowReverse | FlexDirection::ColumnReverse
    );
    let row = flex.direction.is_row();
    let (at_main, at_cross) = main_cross(flex.direction, at);
    // Each in-flow sibling's index in `others`, its main extent and centre, and
    // its cross extent — in flow order.
    type Span = (f64, f64);
    let items: Vec<(usize, Span, f64, Span)> = (0..others.len())
        .filter(|i| container::in_flow(&view, others[*i]))
        .filter_map(|i| {
            let s = others[i];
            let bx = crate::local_box(doc, res, s)
                .zip(res.used_local(doc, s))
                .map(|(b, t)| crate::geometry::transform_rect(t, b))?;
            let (main, cross) = if row {
                ((bx.x0, bx.x1), (bx.y0, bx.y1))
            } else {
                ((bx.y0, bx.y1), (bx.x0, bx.x1))
            };
            Some((i, main, (main.0 + main.1) / 2.0, cross))
        })
        .collect();
    // The lines, as runs of `items`: a new one where the main axis runs back.
    let mut lines: Vec<(usize, usize, (f64, f64))> = Vec::new();
    for (k, (_, main, _, cross)) in items.iter().enumerate() {
        let wraps = k > 0
            && {
                let prev = items[k - 1].1;
                if reversed {
                    main.1 > prev.0 + 1e-6
                } else {
                    main.0 < prev.1 - 1e-6
                }
            }
            && flex.wrap != container::FlexWrap::NoWrap;
        match lines.last_mut() {
            Some((_, end, band)) if !wraps => {
                *end = k + 1;
                *band = (band.0.min(cross.0), band.1.max(cross.1));
            }
            _ => lines.push((k, k + 1, *cross)),
        }
    }
    // The line the centre falls on: the one whose band holds it, else the
    // nearest band.
    let distance = |band: (f64, f64)| (band.0 - at_cross).max(at_cross - band.1).max(0.0);
    let before = match lines
        .iter()
        .min_by(|a, b| distance(a.2).total_cmp(&distance(b.2)))
    {
        Some(&(start, end, _)) => {
            start
                + items[start..end]
                    .iter()
                    .filter(|(_, _, centre, _)| {
                        if reversed {
                            *centre > at_main
                        } else {
                            *centre < at_main
                        }
                    })
                    .count()
        }
        None => 0,
    };
    let index = match items.get(before) {
        Some((i, ..)) => *i,
        None => items.last().map_or(0, |(i, ..)| i + 1),
    };
    Some((index, before))
}

/// The operation dragging the laid-out item `id` by the world-space `delta`
/// commits, **whatever its container's layout** — a flex item's `Reorder`
/// ([`flex_reorder`], §15 D877), a grid item's new lines ([`grid_drop`], §15
/// D913). `None` where it lands where it was, or `id` is not an item of either.
/// The app's one door, so a drag need not know which layout it is in.
pub fn layout_drop(doc: &Document, res: &Resolved, id: NodeId, delta: Vec2) -> Option<Operation> {
    use crate::container::Display;
    match doc.get(doc.get(id)?.parent()?)?.display()? {
        Display::Flex(_) => flex_reorder(doc, res, id, delta),
        Display::Grid(_) => grid_drop(doc, res, id, delta),
    }
}

/// The committed document's grid container `id` as its pass lays it — its
/// tracks and its items' areas ([`crate::container::laid_grid`], §15 D916). The
/// public door, for the canvas's track lines and for tests.
pub fn laid_grid(doc: &Document, id: NodeId) -> Option<crate::container::LaidGrid> {
    crate::container::laid_grid(&crate::resolve::DocView(doc), id)
}

/// The `SetLayoutItem` that dragging the grid item `id` by the world-space
/// `delta` commits — **explicit lines for the cell its centre is dropped on**
/// (§15 D913's third ruling: every drop writes lines, so a dragged item becomes
/// explicitly placed, and an auto-placed one is no exception).
///
/// The drag moves the item's **area** by as many tracks as its centre crossed, on
/// each axis: read against the grid's laid tracks ([`crate::container::laid_grid`])
/// in the container's space, the track under the centre before and after, and
/// the area's start shifted by the difference — so an item spanning two columns
/// keeps both, and one grabbed off-centre is not snapped by half its span. Both
/// axes are written, the one the drag left alone at the line it was laid at.
/// **The span keeps the author's spelling**: an end given as a line moves with the
/// start, one given as a span stays a span, and a one-track area is `start / auto`.
/// A drop lands its whole area on tracks the grid already has — the nearest,
/// past either end, a span stopped where its end meets the last laid track (§15
/// D927; it used to run on into an implicit one, which `fr` tracks size to
/// nothing); an area in a leading implicit track (only a negative line makes
/// one) keeps the negative line that names it ([`grid_drop_many`]).
///
/// `None` when the centre stays in its tracks on both axes — a drop back into its
/// own cell is no operation at all, so no undo step (§15 D877's rule for flex) —
/// or `id` is not an in-flow item of a grid.
pub fn grid_drop(doc: &Document, res: &Resolved, id: NodeId, delta: Vec2) -> Option<Operation> {
    grid_drop_many(doc, res, &[id], delta)?.into_iter().next()
}

/// [`grid_drop`] for **several items of one grid, dragged as a block** (§15
/// D918, D902's reading carried to grid): every item's area moves by the same
/// number of tracks — read off the centre of the block's union box, as a single
/// item's is off its own centre — so the arrangement inside the block is kept,
/// and every item is written explicit lines. A block pushed towards the grid's
/// first track stops there **whole**, its shift clamped to the leftmost (topmost)
/// item's room, so the drag cannot reshuffle it; an area in a leading implicit
/// track is written with the negative line that names it, not moved to line 1.
/// **And towards its last track the same way** (§15 D927): the shift stops where
/// the rightmost (bottommost) area's end meets the last laid track, so an item
/// spanning two dropped on the last column lands on the last two rather than
/// hanging into a new implicit track — one an `fr` grid sizes to nothing, so the
/// item read `3 / span 2` and was drawn spanning one.
///
/// `Some` of no operations when the centre stays in its tracks — the block lands
/// where it was; `None` when the items are not all in-flow items of one grid.
/// A block of one is [`grid_drop`].
pub fn grid_drop_many(
    doc: &Document,
    res: &Resolved,
    ids: &[NodeId],
    delta: Vec2,
) -> Option<Vec<Operation>> {
    use crate::container::{self, GridLines, GridPlacement};
    let parent = doc.get(*ids.first()?)?.parent()?;
    if ids
        .iter()
        .any(|id| !is_flex_item(doc, *id) || doc.get(*id).and_then(|n| n.parent()) != Some(parent))
    {
        return None;
    }
    let view = crate::resolve::DocView(doc);
    let grid = container::laid_grid(&view, parent)?;
    let areas: Vec<(NodeId, [i32; 4])> = ids
        .iter()
        .map(|id| Some((*id, grid.areas.iter().find(|(n, _)| n == id)?.1)))
        .collect::<Option<_>>()?;
    let to_parent = res.world_transform(parent)?.inverse();
    let centre = ids
        .iter()
        .filter_map(|id| res.world_bounds(*id))
        .reduce(|a, b| a.union(b))?
        .center();
    let (was, now) = (to_parent * centre, to_parent * (centre + delta));
    let shift = |tracks: &container::LaidTracks, a: f64, b: f64| -> Option<i32> {
        Some(tracks.index_at(b)? as i32 - tracks.index_at(a)? as i32)
    };
    // Stopped whole at either end of the laid tracks: no further back than the
    // block's first area has room for, no further on than its last area's end
    // has — so the block cannot reshuffle against an edge, nor hang past one.
    let room = |by: i32, tracks: &container::LaidTracks, axis: usize| {
        let first = areas.iter().map(|(_, a)| a[axis]).min().unwrap_or(1);
        let last = areas.iter().map(|(_, a)| a[axis + 1]).max().unwrap_or(1);
        let end = tracks.line_of(tracks.spans.len().saturating_sub(1)) + 1;
        if by < 0 {
            by.max((tracks.line_of(0) - first).min(0))
        } else {
            by.min((end - last).max(0))
        }
    };
    let dc = room(shift(&grid.columns, was.x, now.x)?, &grid.columns, 0);
    let dr = room(shift(&grid.rows, was.y, now.y)?, &grid.rows, 2);
    if dc == 0 && dr == 0 {
        return Some(Vec::new());
    }
    // A line before the explicit grid's first — a leading implicit track, which
    // only a negative line makes — has no positive number, so it is written as
    // the negative one CSS reads the same: line `p` is line `p − (explicit + 2)`
    // counted from the far end (§15 D918's amendment). Clamping it to 1 moved an
    // item on an axis the drag had not touched.
    let line = |n: i32, tracks: &container::LaidTracks| {
        let n = if n >= 1 {
            n
        } else {
            n - (i32::from(tracks.explicit) + 2)
        };
        GridPlacement::Line(n.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16)
    };
    let placed = |lines: GridLines, start: i32, end: i32, by: i32, tracks| -> GridLines {
        let span = (end - start).max(1);
        let to = start + by;
        GridLines {
            start: line(to, tracks),
            end: match lines.end {
                GridPlacement::Line(_) => line(to + span, tracks),
                _ if span > 1 => GridPlacement::Span(span as u16),
                _ => GridPlacement::Auto,
            },
        }
    };
    areas
        .into_iter()
        .map(|(id, area)| {
            let mut item = *doc.get(id)?.item();
            item.grid_column = placed(item.grid_column, area[0], area[1], dc, &grid.columns);
            item.grid_row = placed(item.grid_row, area[2], area[3], dr, &grid.rows);
            Some(Operation::SetLayoutItem { id, item })
        })
        .collect()
}

/// [`layout_drop`] for several items — a flex block's reorder
/// ([`flex_reorder_many`], §15 D902) or a grid block's lines ([`grid_drop_many`],
/// §15 D918). `None` when they are not all in-flow items of one container.
pub fn layout_drop_many(
    doc: &Document,
    res: &Resolved,
    ids: &[NodeId],
    delta: Vec2,
) -> Option<Vec<Operation>> {
    use crate::container::Display;
    match doc.get(doc.get(*ids.first()?)?.parent()?)?.display()? {
        Display::Flex(_) => flex_reorder_many(doc, res, ids, delta),
        Display::Grid(_) => grid_drop_many(doc, res, ids, delta),
    }
}

/// Whether `id` is an item in its container's flow — its parent lays it out and
/// it is not pinned, hidden or a mask. The question [`flex_reorder`] asks first,
/// public for the app's drag, which has to know a reorder from a move before it
/// knows whether the reorder changes anything.
///
/// ⚠️ **True of a grid item too** (§15 D914): the question is "is its parent
/// laying it out", which a grid answers as a flex row does. The name is flex's
/// because flex came first; a grid's drop writes lines rather than reordering
/// (§15 D913), and [`flex_reorder`] answers nothing there.
pub fn is_flex_item(doc: &Document, id: NodeId) -> bool {
    let view = crate::resolve::DocView(doc);
    crate::container::parent_lays_out(&view, id) && crate::container::in_flow(&view, id)
}

/// `p`'s main- and cross-axis coordinates under `direction`.
fn main_cross(direction: crate::container::FlexDirection, p: Point) -> (f64, f64) {
    if direction.is_row() {
        (p.x, p.y)
    } else {
        (p.y, p.x)
    }
}

/// The box a group with a layout is laid out to in `doc` — its own layout pass,
/// run from its chain root so a group that is itself an item gets the size its
/// container gives it (§15 D900). `keep_insets`' measure of a laid group after an
/// edit, where `Resolved::used_frame` would answer for the document before it.
///
/// ⚠️ **Its own pass when the chain root's does not reach it** (§15 D910). A
/// laid group inside a plain group that is a flex item has the flex container as
/// its chain root (§15 D899), whose pass stops at the plain group; one hidden or
/// made a mask is out of its container's flow. Either is a layout root of its own
/// ([`crate::container::is_layout_root`]), and this answered `None` for both — so
/// `keep_insets` skipped its pinned children and a move of one snapped back.
fn laid_group_box(doc: &Document, group: NodeId) -> Option<kurbo::Size> {
    use crate::container::{chain_root, is_layout_root, lay_out};
    let view = crate::resolve::DocView(doc);
    let find = |root| {
        lay_out(&view, root)
            .into_iter()
            .find(|l| l.id == group)
            .map(|l| l.size)
    };
    find(chain_root(&view, group))
        .or_else(|| is_layout_root(&view, group).then(|| find(group)).flatten())
}

/// `tx` with the insets of every **pinned** layer it moves or resizes rewritten,
/// so the layer stays where the edit put it (§15 D871, D874).
///
/// **The one place an edit to a pinned layer becomes an edit of its insets.**
/// Tools compute in *used* geometry — where a layer is drawn, which is what the
/// user sees and drags (§15 D868) — and write the placement they want as ordinary
/// `SetTransform` and `SetGeometry`. For a layer placed by insets that is not
/// enough: its insets, not its transform, decide where it is drawn, so this reads
/// each such write as *"draw it here"* and appends the `SetInsets` that does, each
/// inset kept in its own unit (`container::inverse`). The tool's own ops stay, so
/// the stored transform and size become the placement too, and a layer later
/// un-pinned stays where it was. Every other layer passes through untouched, which
/// is what keeps this a no-op on a document with no insets in it.
///
/// **Run where commits enter**, once, rather than at the ~110 sites that write
/// placements — the alternative was every tool learning what an inset is.
/// Previews do not need it: a tool's own transaction already says where to draw.
///
/// ⚠️ **What the layer is placed against is judged after the transaction**: a
/// `Reparent` into a frame in the same edit pins the layer against its new frame,
/// and a copy an `InsertSubtree` brings in is re-pinned wherever the edit placed
/// it. The frame's size is its *used* size when the edit leaves it alone, and its
/// size after the edit when the edit resizes it too — the Scale tool's case.
///
/// A layer whose insets the transaction sets itself is left to it.
pub fn keep_insets(doc: &Document, res: &Resolved, tx: Transaction) -> Transaction {
    // The placement each node is given, if a `SetTransform` gives one, and
    // whether it was resized; and the nodes whose insets are set outright.
    let mut placed: FxHashMap<NodeId, (Option<Affine>, bool)> = FxHashMap::default();
    let mut explicit: FxHashSet<NodeId> = FxHashSet::default();
    for op in &tx.0 {
        match op {
            Operation::SetTransform { id, transform } => {
                placed.entry(*id).or_insert((None, false)).0 = Some(*transform);
            }
            // Only a patch that changes the box is a resize: a corner radius is
            // applied to a kind still carrying the *stored* size, and reading it
            // as a resize would re-pin a stretched layer at that size
            // (`GeometryPatch::resizes`).
            Operation::SetGeometry { id, geometry } if geometry.resizes() => {
                placed.entry(*id).or_insert((None, false)).1 = true;
            }
            Operation::SetInsets { id, .. } => {
                explicit.insert(*id);
            }
            _ => {}
        }
    }
    // The cheap refusal first: nothing this edit places carries an inset, in the
    // document or in a subtree it inserts. Every commit on a document with no
    // insets ends here, without the scratch copy below.
    let carries = |id: NodeId| {
        doc.get(id).is_some_and(|n| n.insets().is_authored())
            || tx.0.iter().any(|op| match op {
                Operation::InsertSubtree { nodes, .. } => nodes
                    .iter()
                    .any(|n| n.id() == id && n.insets().is_authored()),
                _ => false,
            })
    };
    if !placed
        .keys()
        .any(|id| !explicit.contains(id) && carries(*id))
    {
        return tx;
    }
    // The document as the edit leaves it — parents, insets and sizes all read
    // from here. A transaction the model refuses is returned as it came, for the
    // commit to refuse on its own terms.
    let mut after = doc.clone();
    if after.apply(&tx).is_err() {
        return tx;
    }
    let mut ids: Vec<(NodeId, (Option<Affine>, bool))> = placed.into_iter().collect();
    ids.sort_by_key(|(id, _)| *id);
    let resized: FxHashSet<NodeId> = ids.iter().filter(|(_, p)| p.1).map(|(id, _)| *id).collect();
    // Pinned layers written back to exactly where they are drawn, whose
    // `SetTransform` is dropped below.
    let mut unmoved: FxHashSet<NodeId> = FxHashSet::default();
    let mut out = tx;
    for (id, (to, geometry_changed)) in ids {
        if explicit.contains(&id) {
            continue;
        }
        let Some(node) = after.get(id) else { continue };
        if !node.insets().is_authored() || !crate::container::takes_insets(node.kind()) {
            continue;
        }
        let Some(parent) = node.parent().and_then(|p| after.get(p)) else {
            continue;
        };
        // The frame's used size where the edit leaves the frame alone; its new
        // size where the edit resizes it too.
        let parent_kind = if resized.contains(&parent.id()) {
            parent.kind()
        } else {
            res.used_kind(doc, parent.id()).unwrap_or(parent.kind())
        };
        // **A group with a layout pins too** (§15 D871, D887) — against its laid
        // box, which is what `resolve::frame_box` places the child against. This
        // was frame-only while the placement was not, so a pinned child of a laid
        // group moved by a tool kept its insets and snapped back on release.
        // 🚨 **The box the edit leaves, not the committed one** (§15 D900). A laid
        // group's box is its layout's answer, so it is read by running that
        // layout over `after` — the document as the edit leaves it — from the
        // group's chain root, or from the group itself where that pass does not
        // reach it (`laid_group_box`, §15 D910). It read `res.used_frame`, the
        // committed box, and the Scale tool on a laid group re-sizes the group and
        // moves a pinned child in one edit: doubled, a right-pinned child was
        // re-pinned against the old box and drawn 60 across and 30 down from its
        // corner. A frame's box needed no such pass: its size is in its kind,
        // `parent_kind` above.
        let frame = match parent_kind {
            NodeKind::Artboard { size } => *size,
            NodeKind::Group if parent.display().is_some() => {
                match laid_group_box(&after, parent.id()) {
                    Some(size) => size,
                    None => continue,
                }
            }
            _ => continue,
        };
        // Where the edit wants it drawn: the transform it wrote, else where it is
        // drawn now. At what size: the size it wrote, else the size drawn now.
        let local = to
            .or_else(|| res.used_local(doc, id))
            .unwrap_or(node.transform());
        let kind = if geometry_changed {
            node.kind().clone()
        } else {
            res.used_kind(doc, id)
                .cloned()
                .unwrap_or_else(|| node.kind().clone())
        };
        let Some(bx) = crate::geometry::local_bounds(&kind, None) else {
            continue;
        };
        let insets = crate::container::inverse(node.insets(), frame, local, bx, &kind);
        if insets != *node.insets() {
            out.0.push(Operation::SetInsets { id, insets });
        } else if !geometry_changed
            && let (Some(to), Some(now)) = (to, res.used_local(doc, id))
            && to
                .as_coeffs()
                .iter()
                .zip(now.as_coeffs())
                .all(|(a, b)| (a - b).abs() < 1e-9)
        {
            // **Written back to where it is drawn**: a handle pressed and let go
            // without moving writes the *used* placement, which for a pinned layer
            // is not its stored transform — so without this the no-op test
            // (§15 D428) would see a change and commit an invisible undo step.
            unmoved.insert(id);
        }
    }
    if !unmoved.is_empty() {
        out.0
            .retain(|op| !matches!(op, Operation::SetTransform { id, .. } if unmoved.contains(id)));
    }
    out
}
