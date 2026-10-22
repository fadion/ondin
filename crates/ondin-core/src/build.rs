//! Composite transaction builders and the world-space projection facade
//! (§5.7 `TransactionBuilder`, §8.4 `place_at_world`).
//!
//! Each function turns a user-level intent ("group these", "put this at these
//! world coordinates") into one `Transaction`, one undo step. It lives in core so
//! the UI, MCP write tools and the command line share one projection. Nothing
//! here mutates: each is a pure `(&Document, &Resolved, …) -> Transaction`.
//!
//! The document stores local transforms only; `local = parent_world⁻¹ · world`
//! is written once, in [`local_for_world`].

use crate::document::Document;
use crate::id::{IdSource, NodeId};
use crate::image::Brush;
use crate::item::{Keyed, same_values};
use crate::node::{Fill, Node, NodeKind, Stroke};
use crate::op::{GeometryPatch, OpError, Operation, Transaction};
use crate::query::pivot_world;
use crate::resolve::Resolved;
use kurbo::{Affine, Point, RoundedRectRadii, Vec2};
use peniko::Color;
use rustc_hash::{FxHashMap, FxHashSet};

/// Whether a node of `child` may be a direct child of a node of `parent`
/// (§5.3): a frame hangs off the root, another frame or a group, and nothing may
/// hold a `Root`.
///
/// The root may hold loose shapes (a shape dragged out of a frame lands there),
/// frames nest, and a frame may sit in any group (a flex row of cards is a group
/// of frames), so removing a group's layout never has to be refused.
///
/// A frame still may not sit under a boolean or a mask at any depth; that is an
/// ancestor question, asked by [`Document::frame_may_sit_under`], and every door
/// that asks this asks that too.
///
/// The one statement of the parent rule: `apply`, the loader, the layer tree's
/// drop greying and render previews all call it.
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
/// A field invariant of the model, asked by the operation that writes the field
/// and by the loader that reads it back.
///
/// [`local_for_world`] produces the value this refuses: `parent_world.inverse()`
/// on a singular parent is all `NaN`. The guard lives here, not in the
/// projection, because the projection has no way to report an error and the
/// operation returns a `Result` every commit path handles.
pub fn affine_is_finite(t: Affine) -> bool {
    t.as_coeffs().iter().all(|c| c.is_finite())
}

/// Whether `t` can be inverted without manufacturing a value the file cannot
/// hold (§15 D713).
///
/// Finite is not enough: a singular matrix has finite coefficients but inverts to
/// all `NaN`, which `serde_json` writes as `null` and the next load rejects.
/// Threshold-free on purpose: it asks whether the inverse is representable, so
/// there is no epsilon to tune, and it is the only spelling of that test (a
/// determinant epsilon is relative, a determinant is in squared world units).
///
/// Not a validator for the model: a singular transform is legal to store (the
/// loader accepts one); this is the guard a reader asks before inverting.
pub fn affine_is_invertible(t: Affine) -> bool {
    affine_is_finite(t) && affine_is_finite(t.inverse())
}

/// Whether every number in a brush is finite (`OpError::NonFinite`) — §15 D451.
///
/// The paint half of invariant 8. It covers everything `serde_json` would refuse
/// to write: a solid colour's components, a gradient's geometry, every stop's
/// offset and colour, and the gradient's transform.
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
                // `valid_opacity`, not `is_finite`: the loader validates nothing,
                // and a finite out-of-range value (`-5`, `1e30`) would silently
                // distort every stop and survive a round trip.
                && valid_opacity(g.opacity)
        }
        peniko::Brush::Image(i) => i.sampler.alpha.is_finite() && image_ref_is_finite(&i.image),
    }
}

/// Invariant 8 for an effect's numbers (§15 D641).
///
/// Also matters before any save: an infinite blur radius makes `effect::escaped`
/// compute `inf − inf = NaN` and return no escape, so the cull, export plan and
/// backend buffer boxes would believe the blur reaches nowhere. Refusing at the
/// operation keeps `escaped`'s input finite. A shadow's `color` is included.
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
/// The reference hanging off an image brush (crop, tile scale, the adjustment
/// sliders) is this crate's, not peniko's, so checking `peniko::Brush` alone stops
/// one level short. This is the second guard behind `image::usable_span`, which
/// stops a crop span overflowing; see
/// `image::tests::a_crop_whose_span_overflows_never_reaches_the_document`.
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
/// Exhaustive over `GradientKind` with no wildcard, so a new peniko kind stops
/// the build here rather than going unchecked.
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
/// already carries them, and emitting both would move them twice.
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
/// Plain [`Operation::Reparent`] keeps the local transform, which teleports the
/// node whenever the two parents differ.
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
/// frame's size, a text's sizing via [`kept_text_mode`]). Nothing for a kind whose
/// size is not in it: a path's, a group's, an image's.
///
/// The one spelling of "keep it where it is drawn" for every edit that takes a
/// layer out of whatever was placing it: the structural verbs through [`baked`],
/// a Position pin change, *Unpin and return to the layout*, a layout set to
/// `none`, and (its `SetGeometry` half alone) W/H `px` from the sizing menu.
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
            NodeKind::Text { sizing, .. } => Some(crate::GeometryPatch::TextSizing(
                kept_text_mode(node, kind, *sizing),
            )),
            _ => None,
        };
        if let Some(geometry) = geometry
            && !matches!((&geometry, node.kind()),
                (crate::GeometryPatch::TextSizing(s), NodeKind::Text { sizing, .. }) if s == sizing)
        {
            ops.push(Operation::SetGeometry { id, geometry });
        }
    }
    ops
}

/// The sizing to bake for a text drawn as `kind` (sizing `drawn`): its stored
/// mode, kept where that draws the same.
///
/// A label a container stretched is drawn as a `Fixed` box, and baking that would
/// stop an auto-width label growing with its words. Where the stored mode is
/// `Auto`/`AutoHeight` and the drawn box is exactly as tall as its lines,
/// `AutoHeight` at the drawn width is the same picture; a taller box stays `Fixed`.
fn kept_text_mode(
    node: &crate::Node,
    kind: &NodeKind,
    drawn: crate::node::TextSizing,
) -> crate::node::TextSizing {
    use crate::node::TextSizing;
    let (NodeKind::Text { sizing: stored, .. }, TextSizing::Fixed(s)) = (node.kind(), drawn) else {
        return drawn;
    };
    if matches!(stored, TextSizing::Fixed(_)) {
        return drawn;
    }
    let mut auto = kind.clone();
    if let NodeKind::Text { sizing, .. } = &mut auto {
        *sizing = TextSizing::AutoHeight(s.width);
    }
    match crate::geometry::local_bounds(&auto, None) {
        Some(b) if (b.height() - s.height).abs() < 1.0 / 128.0 => TextSizing::AutoHeight(s.width),
        _ => drawn,
    }
}

/// The operations that make `id` **store where it is drawn**, for a door that
/// takes it out of the layout that placed it or takes that layout away — so the
/// edit moves nothing on screen (§9's rule for structural edits).
///
/// An in-flow item's stored transform and size are not where and what it is
/// drawn as; group, frame selection, boolean, mask, ungroup and *Display → none*
/// each take items out of a layout and write this first.
///
/// Three parts: the used transform and kind ([`baked_ops`]); and, for a container
/// with a layout of its own, the size its parent's pass gave it on an axis where
/// its own pass would give another (a hugging frame becomes `auto` with the kind
/// carrying the drawn size; a layout group takes the drawn size in px). An axis
/// its own pass sizes alike is left alone.
///
/// Empty for a layer already stored where it is drawn, so documents with no
/// layout are unchanged.
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
/// reparented into it: the topmost member's slot (§15 D286).
///
/// The index counts the non-members below the topmost member, so a
/// non-contiguous selection leaves every unselected layer where it was. It must
/// be `rposition` (the topmost member), not `position`: selecting `back` and
/// `front` of `back`/`mid`/`front` must give `[mid, NEW]`, and the `filter` only
/// means anything against `rposition`.
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
/// siblings; grouping across parents is rejected rather than guessed. Frames can
/// be grouped.
///
/// The group is created with an identity transform in the shared parent's space,
/// so each member keeps its local transform, written first where it is drawn
/// ([`baked`]) so a flex or grid member does not jump.
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

    // One shared parent, and no root member.
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

/// Which of `members` becomes the mask: the key if one is designated, else the
/// bottom-most member that is not a picture, else the bottom-most, among the
/// members that [`can_be_mask`] when any can (otherwise [`mask`] reports the
/// refusal). `None` only when `members` is empty or names nothing in the document.
///
/// A "picture" is an image-filled layer ([`crate::Paint::image_fill`]), a shape
/// whose outline is a rectangle; masking with it would crop everything to a
/// rectangle, and z-order alone picks it whenever the photo sits lower. A key
/// (`Alt+Shift`) overrides, the same gesture that overrides a `Subtract` base.
///
/// Two callers ask: this builder and the inspector, which needs to say why its
/// control is dim.
pub fn mask_target(doc: &Document, members: &[NodeId], key: Option<NodeId>) -> Option<NodeId> {
    let unique: FxHashSet<NodeId> = members.iter().copied().collect();
    if let Some(key) = key.filter(|k| unique.contains(k)) {
        return Some(key);
    }
    // Parent's child order is paint order; the selection's order is click order.
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
/// mask ([`NodeKind::can_mask`]), and nothing inside it is a frame, since a page
/// is not an outline. The same two refusals `Document::apply` makes.
pub fn can_be_mask(doc: &Document, id: NodeId) -> bool {
    doc.get(id).is_some_and(|n| n.kind().can_mask()) && !doc.holds_a_frame(id)
}

/// *Use as mask* — make one of `members` the mask for the rest, **wrapping them
/// in a group** when there is more than one.
///
/// Returns the transaction and the id the caller should select.
///
/// The group keeps the mask and its subjects one draggable thing. One member is
/// the exception and takes no group: a lone layer masks the siblings it already
/// has ([`crate::Document::governing_mask`]), and wrapping it would change which
/// layers it masks.
///
/// [`mask_target`] picks the mask; it moves to index 0 of the new group and the
/// rest keep their z-order, the same smallest rearrangement [`boolean`] makes for
/// a `Subtract` base. A mask clips what is drawn above it, so designating one is
/// a reorder, not a flag on the selection.
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
    // One shared parent, asked here because the mask choice uses its child order.
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
    // Nothing masks inside a boolean (operands, not artwork); refused so the chord
    // path gets an error rather than a silent no-op.
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
    // `apply` refuses this too; asked here before a group is planned around it.
    if doc.holds_a_frame(chosen) {
        return Err(OpError::ArtboardPlacement);
    }

    if unique.len() == 1 {
        // A mask leaves its container's flow, so write it where it is drawn first.
        let mut ops = baked(doc, res, chosen);
        ops.push(Operation::SetMask {
            id: chosen,
            mask: true,
        });
        return Ok((Transaction(ops), chosen));
    }

    // `group` owns the rest of the admission test and the placement rule.
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
/// Returns the transaction and the id minted for the frame. [`group`]'s sibling,
/// a second function because a frame has a box:
///
/// - It is created at the union's corner, so every member is moved back by exactly
///   that corner (`translate(−origin) · local`, from the member's used transform;
///   a flex item's stretched size is baked too, [`baked`]). `Resolved` is needed
///   to measure a text node's or container's box.
/// - The parent has to accept a frame (§5.3): never under a boolean or a mask.
///   Refused with [`OpError::ArtboardPlacement`] rather than framed elsewhere.
///
/// The frame is created with no background and does not clip: framing changes
/// nothing you can see. [`can_frame`] is the question a menu row asks first.
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

    // One shared parent; only the root is refused as a member.
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
    // §5.3's placement rule, asked before anything is planned so the failure is
    // one the caller can explain.
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

    // The union in the shared parent's space, from each member's box through its
    // used transform; `world_bounds` would pick up an axis-aligned box for a
    // rotated member.
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
    // The floor `create_artboard` uses: a zero-extent line would give a frame
    // nothing can be dropped into.
    let size = kurbo::Size::new(union.width().max(1.0), union.height().max(1.0));

    let frame_id = ids.mint();
    let mut ops = Vec::with_capacity(ordered.len() * 2 + 2);
    // Appended first and reordered last, as `group` does.
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
/// The builder's own question, so a menu row cannot offer what the verb would
/// refuse (`context-menus.md` §3). It does not ask whether the members' boxes
/// resolve (that needs a [`Resolved`]); [`frame`] reports that.
pub fn can_frame(doc: &Document, members: &[NodeId]) -> bool {
    let mut parent = None;
    for id in members {
        let Some(node) = doc.get(*id) else {
            return false;
        };
        // Expressive, not load-bearing: `Root` is the only parentless node, so the
        // `let else` below already refuses it; kept so the answer is readable here.
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
/// The same shape as [`group`] (one new container, members reparented in z-order,
/// reordered into the topmost member's slot) with three differences:
///
/// - Two members minimum: a boolean of one shape is that shape.
/// - Text is refused: it has no outline (`resolve::operand_path` is `None`). The
///   user converts it first with [`outline_text`]; converting silently would take
///   a one-way door for them.
/// - The result inherits the bottom member's paint, since a fresh container has no
///   fills and would draw nothing.
///
/// `key`, when given, is the member placed at the bottom, the base `Subtract` cuts
/// into and the paint source. It applies to `Subtract` only: Union, Intersect and
/// Exclude fold commutatively, so reordering would change the layer list while
/// leaving the shape identical. It is creation-time ordering, not a stored
/// property: afterwards operand order is child order.
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
        // A group holding a frame would carry it into an operand; refuse before `apply` does.
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

    // Z-order is the operand order: index 0 is the bottom, `Subtract`'s base.
    let mut ordered: Vec<NodeId> = siblings
        .iter()
        .copied()
        .filter(|c| unique.contains(c))
        .collect();
    // The key goes to the bottom; the rest keep their relative order.
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
    // No fill rule is set: `Node::fill_rule` derives it from the operation.
    // The bottom member's paint, read before anything moves.
    let base = doc.get(ordered[0]).expect("checked above");
    ops.extend(carried_paint(base, bool_id));
    for (i, member) in ordered.iter().enumerate() {
        ops.push(Operation::Reparent {
            id: *member,
            new_parent: bool_id,
            index: i,
        });
        // Operands are drawn where they were, as in `group`.
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
            // No `SetFillRule`: it follows the operation. Rename only a name we
            // gave it (`is_boolean_label` is strict: "Badge 2" stays the user's).
            if crate::naming::is_boolean_label(node.name()) {
                // The number comes across so flipping back and forth does not walk
                // it; a name with no number joins the series at 1. The container
                // is excluded from `taken` so its own old name does not push the
                // new one a number along.
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
/// The one destructive shape operation, the counterpart to [`ungroup`]: a boolean
/// container keeps its operands; flattening throws them away and keeps the answer.
///
/// - One boolean: its derived outline in its own local space, keeping its
///   transform, paint, name and opacity.
/// - Two or more layers: their union in the parent's space (as `boolean(Union)`
///   then flattening, in one undo step); groups and booleans contribute through
///   [`Resolved::operand_path`].
///
/// A single non-boolean layer is refused; turning a rect into a path is
/// [`outline`] (§15 D230). Text is refused for [`boolean`]'s reason; `Artboard`
/// and `Root` are not shapes.
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
    // The same two `None`s as `flatten_union`'s, read from the cache's `failed`
    // mark rather than this thread's counter (the cache was filled on an earlier pass).
    let path = match res.boolean_path(id).cloned() {
        Some(path) => path,
        None if res.boolean_failed(id) => return Err(OpError::BooleanAbandoned),
        None => return Err(OpError::EmptyGeometry),
    };
    replace_with_path(doc, ids, id, path)
}

/// Whether [`outline`] would produce a layer *different* from the one it replaces.
///
/// The kind gate and the no-op check in one answer: a rect, ellipse, polygon, star
/// or line has an outline that is not already a `Path`, and a `Path` has one only
/// while its corner radii are still in the model (§15 D119).
///
/// Refused: a `Boolean` ([`flatten`]'s), a `Group` (no outline of its own), an
/// `Artboard` (a page), and `Text`, which needs the resolved layout and so is
/// [`outline_text`] (§15 D260). A menu row takes presence from the kind and
/// dimming from this (`docs/context-menus.md` §3).
pub fn can_outline(kind: &NodeKind) -> bool {
    match kind {
        NodeKind::Rect { .. }
        | NodeKind::Ellipse { .. }
        | NodeKind::Polygon { .. }
        | NodeKind::Star { .. }
        | NodeKind::Line { .. } => true,
        // `round_corners` returns the path untouched when no radius is `> 0`, so
        // this is "baking the radii would change something". Keep `any(> 0)`:
        // `all(<= 0)` is not its negation because `NaN` fails both comparisons.
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
/// The counterpart to [`flatten`], deliberately a separate verb: flatten keeps the
/// answer of several operands; this stops describing one shape parametrically and
/// describes it as anchors. They share only [`replace_with_path`].
///
/// It takes a [`Resolved`] for the kind the node is sized with (§15 D930): a shape
/// a container stretched is drawn at a size its stored kind does not hold, and a
/// path cannot stretch afterwards.
///
/// The radii are not carried onto the result: they are baked into the geometry,
/// and `NodeKind::Path::corner_radii` is indexed by anchor.
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
/// A third verb rather than an arm of [`outline`]: a text node's outline is a
/// shaped thing, so this takes a [`Resolved`] for the shaping. `text::outline`
/// yields contours already in the node's local coordinates.
///
/// A one-way door with no confirmation; undo is the way back, and a caller should
/// say so in the status line. Empty content is refused with `EmptyGeometry`, since
/// an empty `Path` would draw nothing and never be selectable.
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
/// Not just "is it text": empty or all-space content shapes to no contours, so the
/// row must be dim. It asks `text::outline`, as the verb does, not the content.
pub fn can_outline_text(res: &Resolved, id: NodeId) -> bool {
    res.text_layout(id)
        .is_some_and(|l| !crate::text::outline(l).elements().is_empty())
}

/// Whether `id` is **artwork** a rail may be made of — the one question both
/// rail doors ask ([`text_on_new_path`] and `query::outline_at`). Three roles
/// reach them as ordinary kinds and are not: a page (a frame or the root, whose
/// consumption orphans its guides), a boolean's operand (its edge lies on the
/// result's silhouette, and consuming it guts the boolean) and a mask
/// (consuming it unmasks what it hid).
pub fn is_rail_artwork(doc: &Document, id: NodeId) -> bool {
    let Some(node) = doc.get(id) else {
        return false;
    };
    if node.mask() || matches!(node.kind(), NodeKind::Artboard { .. } | NodeKind::Root) {
        return false;
    }
    let mut cursor = node.parent();
    while let Some(c) = cursor.and_then(|c| doc.get(c)) {
        if matches!(c.kind(), NodeKind::Boolean { .. }) {
            return false;
        }
        cursor = c.parent();
    }
    true
}

/// Make a **new, empty** text node set along `path`, consuming it — the Text
/// tool's click on an outline (§15 D408).
///
/// The only way type gets onto a curve (§15 D409); *Detach from path* is the way
/// back.
///
/// The new node takes the path's own transform, so the rail needs no rebasing. It
/// goes in the consumed layer's own slot, not on top, so typing does not reorder
/// the drawing.
pub fn text_on_new_path(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    path: NodeId,
    parts: &crate::text::TextParts,
    at_world: Point,
) -> Result<(Transaction, NodeId), OpError> {
    let node = doc.get(path).ok_or(OpError::NoSuchNode(path))?;
    // Refused here as well as at the pick: MCP and the command line arrive directly.
    if !is_rail_artwork(doc, path) {
        return Err(OpError::WrongKindForOp);
    }
    // The outline as drawn, as `outline` reads it.
    let rail = crate::geometry::local_path(res.used_kind(doc, path).unwrap_or(node.kind()))
        .ok_or(OpError::EmptyGeometry)?;
    if rail.elements().is_empty() {
        return Err(OpError::EmptyGeometry);
    }
    // The type starts where the pointer was. Go through the world transform, not
    // `node.transform()`, which is only local-to-parent and is wrong inside a frame.
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
                // Appended then reordered into the consumed layer's slot, as
                // `replace_with_path` does: `siblings.len()` is always in range.
                index: siblings.len(),
                kind: NodeKind::Text {
                    content: String::new(),
                    style: Box::new(parts.style.clone()),
                    spans: parts.spans.clone(),
                    para_spans: parts.para_spans.clone(),
                    paragraph: parts.paragraph.clone(),
                    block: parts.block,
                    // `Auto`, whatever the defaults say: the rail is the measure, and a
                    // stored wrap width would resurface when the type is detached.
                    sizing: crate::node::TextSizing::Auto,
                    on_path: Some(Box::new(rail)),
                    on_path_flip: false,
                    on_path_offset: offset,
                },
                transform: Some(node.transform()),
                // Numbered like any other text layer.
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
/// The rail comes back as a layer, which makes [`text_on_new_path`] a two-way door.
/// It carries the text node's own transform (the rail was stored in that space)
/// and is inserted above the text.
///
/// The path comes back with no paint: the layer that carried it was deleted, and
/// keeping its fills on the text node would be state that outlives its reason.
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
            // Appended then reordered, as `replace_with_path` does; `at + 1` is
            // directly above the text.
            index: siblings.len(),
            kind: NodeKind::Path {
                path: rail,
                corner_radii: Vec::new(),
            },
            transform: Some(node.transform()),
            // Numbered like any other path.
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
/// One function because "what survives being redrawn" is a list, and a list
/// written three times drifts.
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
        // Appended, then reordered into the old node's slot once it is gone.
        index: siblings.len(),
        kind: NodeKind::Path {
            path,
            corner_radii: Vec::new(),
        },
        transform: Some(node.transform()),
        // The layer's name survives, even one we gave it.
        name: Some(node.name().to_string()),
    }];
    ops.extend(carried_paint(node, new));
    // This is the same layer, so everything that is not geometry comes across
    // (`flatten_union` makes a new layer and inherits only paint). Opacity and
    // visibility are written resolved: the path is unbound.
    let opacity = doc.drawn_opacity(id);
    if opacity != 1.0 {
        ops.push(Operation::SetOpacity { id: new, opacity });
    }
    if !doc.shown(id) {
        ops.push(Operation::SetVisible {
            id: new,
            visible: false,
        });
    }
    // A moved pivot: the local space is unchanged, so it means what it meant.
    if let Some(pivot) = node.pivot() {
        ops.push(Operation::SetPivot {
            id: new,
            pivot: Some(pivot),
        });
    }
    // A mask: same shape in the same slot, so it clips the same run; dropping it
    // would release the mask as a side effect.
    if node.mask() {
        ops.push(Operation::SetMask {
            id: new,
            mask: true,
        });
    }
    // The effect stack (§5.3a), as the whole stack; skipped when empty.
    if !node.effects().is_empty() {
        ops.push(Operation::SetEffects {
            id: new,
            effects: node.effects().to_vec(),
        });
    }
    // The fill rule, the one carry that changes what is drawn: `Node::fill_rule`
    // derives even-odd for an `Exclude` from the operation (§15 D239), so the
    // derivation would die with the container and every hole would fill in. Carried
    // as the effective rule, which also keeps a stored even-odd on an [`outline`]d
    // `Path`. Skipped at the default.
    if node.fill_rule() != crate::node::FillRule::default() {
        ops.push(Operation::SetFillRule {
            id: new,
            rule: node.fill_rule(),
        });
    }
    // Its place in a layout: insets and item properties (grid cell, growth,
    // `align-self`), skipped at the defaults. `display` does not come across: a
    // path holds no children (`container::is_container`).
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
    // Not carried: *Lock* (a locked result would refuse the tidying that follows)
    // and the proportion lock (a path has no authored box).
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
    // The same admission test as `boolean`.
    let mut parent = None;
    for id in members {
        let node = doc.get(*id).ok_or(OpError::NoSuchNode(*id))?;
        if matches!(
            node.kind(),
            NodeKind::Artboard { .. } | NodeKind::Root | NodeKind::Text { .. }
        ) {
            return Err(OpError::WrongKindForOp);
        }
        // A group holding a frame is refused as in `boolean`, and here nothing else
        // would stop it: the union deletes the members, orphaning the frame's
        // guides into a file that fails to load.
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
    // Bottom of the stack first, as `boolean` orders its operands.
    let ordered: Vec<NodeId> = siblings
        .iter()
        .copied()
        .filter(|c| set.contains(c))
        .collect();
    let final_index = container_slot(siblings, &set)?;

    // Each member's contribution in the parent's space.
    let outlines: Vec<kurbo::BezPath> = ordered
        .iter()
        .filter_map(|m| res.operand_path(doc, *m))
        .collect();
    // `None` has two meanings: shapes that cancel out (`EmptyGeometry`) and a
    // union flo_curves unwound out of (`BooleanAbandoned`). Only
    // `boolean::failures` separates them, so bracket it. This door is outside the
    // commit-time bracket because a builder evaluates while making the transaction.
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
    // Named here, not in `op_create`: the create precedes the deletes, so the
    // operands would still count. Numbered against the siblings that survive.
    let name = crate::naming::free_name(&doc.child_names(parent, &ordered), kind.default_name(), 1);
    let mut ops = vec![Operation::CreateNode {
        id: new,
        parent,
        index: siblings.len(),
        kind,
        transform: None,
        name: Some(name),
    }];
    // The bottom member's paint, read before anything is deleted, as `boolean` does.
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
/// Emitted only where there is something to say. A fresh node has no fills, so
/// without inheriting the result would draw nothing.
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
/// A boolean releases through here: its operands kept their geometry, transforms
/// and paint, so taking one apart is the same splice a group needs. The boolean's
/// own fill and stroke vanish with it; each operand's own paint comes back.
///
/// Folded from where things are drawn (§15 D929): the container's used transform
/// into each child's used transform, and each child's used size written as its
/// own ([`baked`]). For a layout group neither stored number is a place.
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
/// One transaction, so one keypress is one undo step and a failure partway leaves
/// nothing behind (§15 D521).
///
/// Built against a document advanced as it goes, not index arithmetic: [`ungroup`]
/// reads the container's position, the splice moves later siblings, and a
/// container nested in an earlier one has a new parent and transform by its turn.
/// A whole `Document` is cloned (an ungroup rewrites transforms too), except for
/// the one-container case.
///
/// Each id must be a live `Group` or `Boolean`; anything else returns [`ungroup`]'s
/// error with no ops built.
pub fn ungroup_all(doc: &Document, res: &Resolved, ids: &[NodeId]) -> Result<Transaction, OpError> {
    if let [only] = ids {
        return ungroup(doc, res, *only);
    }
    // The derived layer advances too, since [`ungroup`] folds from where things
    // are drawn: `res` for the first container, then one rebuild and an
    // incremental update per step.
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
/// One transaction, so a paste is one undo step and a failure leaves nothing behind.
///
/// Each index is advanced past the earlier inserts that landed at or below it, not
/// by a flat count: copies land above their originals, so two layers copied
/// top-down arrive as index 2 then 1 and indices need not ascend (§15 D54).
///
/// Components (§15 D978, D979 (e)): a copy of a main lands as an instance of it,
/// and every copy's links are settled for where it lands (`component::settle_copy`).
/// An instance keeps its main's names, so copy numbering is skipped for one.
/// [`insert_subtrees_as`] with [`crate::component::MainCopy::NewMain`] is
/// *Duplicate as component*.
pub fn insert_subtrees(
    doc: &Document,
    ids: &mut IdSource,
    placements: &[Placement],
    offset: Vec2,
) -> (Transaction, Vec<NodeId>) {
    insert_subtrees_as(
        doc,
        ids,
        placements,
        offset,
        crate::component::MainCopy::Instance,
    )
}

/// [`insert_subtrees`], saying how a copy of a main lands.
pub fn insert_subtrees_as(
    doc: &Document,
    ids: &mut IdSource,
    placements: &[Placement],
    offset: Vec2,
    mains: crate::component::MainCopy,
) -> (Transaction, Vec<NodeId>) {
    // The indices already inserted at, per parent (not a count; see the doc).
    let mut added: FxHashMap<NodeId, Vec<usize>> = FxHashMap::default();
    let mut ops = Vec::with_capacity(placements.len() * 2);
    let mut created = Vec::with_capacity(placements.len());
    let mut names = CopyNames::default();

    for placement in placements {
        let Some((mut nodes, new_root)) = crate::document::remap_subtree(&placement.nodes, ids)
        else {
            continue;
        };
        let instance = crate::component::settle_copy(doc, &placement.nodes, &mut nodes, mains);
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
        if !instance {
            ops.extend(names.rename(doc, placement.parent, new_root, &was_called));
        }
        created.push(new_root);
    }

    (Transaction(ops), created)
}

/// The names a paste or duplicate has already handed out, per destination parent.
///
/// Several copies in one transaction have to remember each other: the document
/// has not seen the earlier ones, so pasting three "Rectangle 2"s would otherwise
/// make three "Rectangle 3"s. Per parent, because numbering is per parent (§15 D127).
#[derive(Default)]
pub struct CopyNames(FxHashMap<NodeId, Vec<String>>);

impl CopyNames {
    /// The `SetName` a copy of a layer called `was_called` needs to join `parent`,
    /// or nothing when it keeps the name it has.
    ///
    /// A `SetName` op rather than a rewritten `Node`: `Node`'s fields are private
    /// (invariant 2). Only the root is renamed; a duplicated group's children keep
    /// their names, being unique within the new group.
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
/// [`Document::capture_subtree`] clones nodes but an [`ImageId`](crate::ImageId)
/// is a key into the document's table, so the pictures must be taken while the
/// source document is open (at copy time for a clipboard), or the fill draws the
/// missing-picture placeholder.
///
/// Hidden fills count: visibility says whether to draw, not whether the bytes are
/// needed. Ids are content hashes (`image.rs`), so nothing is remapped on arrival.
pub fn image_ids_in(nodes: &[Node]) -> Vec<crate::ImageId> {
    let mut out: Vec<crate::ImageId> = nodes
        .iter()
        .flat_map(|n| {
            let paint = n.paint();
            let fills = paint.fills.iter().map(|f| &f.brush);
            let strokes = paint.strokes.iter().map(|s| &s.brush);
            fills.chain(strokes)
        })
        // A bound paint is a solid of its variable's colour, never a picture.
        .filter_map(|brush| match brush.literal() {
            Some(Brush::Image(b)) => Some(b.image.id.clone()),
            _ => None,
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The `AddImage` ops `doc` needs before it can show `held`.
///
/// Only what is missing, so it is safe on every paste: pasting back into the
/// source document produces nothing.
///
/// The ops go first in the transaction that inserts the nodes, so the inverse
/// removes the table entry after the layers using it. `app::insert_all` owes that.
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
/// What every export of a selection owes its writer: SVG has no z-index, so the
/// last thing emitted is on top, but a selection is in pick order. Both export
/// doors go through here. `outermost` does not sort itself because bulk edit paths
/// want the caller's order (the key layer, the z-order verbs).
///
/// One pre-order walk emitting a member when reached; see [`all_in_document_order`].
pub fn in_document_order(doc: &Document, ids: &[NodeId]) -> Vec<NodeId> {
    all_in_document_order(doc, &outermost(doc, ids))
}

/// `ids` in document order, keeping a member that sits inside another (§15 D516).
///
/// [`in_document_order`] without its `outermost` filter: that is right for a
/// selection (a group and its child both picked would plan the child twice), but
/// wrong for the layers carrying export settings, where a spec on a nested layer
/// is deliberate and exporting the group makes a different file. A matched member
/// is descended into for the same reason.
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
/// These belong at the front of the delete transaction, ahead of the
/// `DeleteNode`s: the inverse reverses the order, so the frame is back before its
/// guide is re-added (`Document::check_guide_owner`). The other order deletes
/// correctly and makes the undo fail. Sorted by id so the transaction is
/// deterministic.
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
/// The apply-once rule (§5.7a) for properties: members are reduced to the
/// outermost, then each subtree is walked in full ([`outermost`] alone would hand
/// back a group with no paint of its own).
///
/// Document order, roots included (§15 D646): an operation list must replay
/// identically on another replica (§12), so it cannot depend on hash order or on
/// the order a caller passes its selection in.
pub fn subtree_nodes(doc: &Document, ids: &[NodeId]) -> Vec<NodeId> {
    // The roots are ordered too, since `outermost` keeps input (pick) order. Only
    // with more than one root, because `all_in_document_order` walks the whole
    // document; the multi-root cost is paid per call, and several panels call it
    // every frame.
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
    /// Every fill and stroke carrying this exact colour. A layer whose fill and
    /// stroke match counts twice, so this is the wrong number beside a select
    /// button; see `nodes`.
    pub uses: usize,
    /// The distinct layers carrying it, in document order: the set the Group
    /// Colors panel counts and its select button acts on, taken from the one
    /// census walk.
    pub nodes: Vec<NodeId>,
}

/// The distinct solid colours inside the subtrees rooted at `ids`, most-used
/// first.
///
/// What the Group Colors panel lists and the picker's palette row shows, the same
/// census at two scopes. Pass the root for the whole document.
///
/// Solids only: a gradient has no single swatch and nothing sensible for
/// [`recolor`] to do. Hidden paints count, or a recolour would skip them and the
/// old colour reappear when the fill is switched back on.
pub fn colors_in(doc: &Document, ids: &[NodeId]) -> Vec<ColorUse> {
    census(doc, ids, |brush| match brush {
        Brush::Solid(c) => Some(c.to_rgba8().to_u8_array()),
        _ => None,
    })
    .into_iter()
    // A colour is its own key, so the location is ignored.
    .map(|([r, g, b, a], uses, nodes, _)| ColorUse {
        color: Color::from_rgba8(r, g, b, a),
        uses,
        nodes,
    })
    .collect()
}

/// One place a paint sits: a node, which of its two lists, and where in it.
///
/// Two lists and an index, on every kind (§15 D400). Small, `Copy` and `Eq` so a
/// UI slot can carry it (`inspector::PaintSlot`), and a location survives an edit
/// that a value key (the old brush) does not.
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
    /// The first place this gradient was found, in document order: what the row
    /// hands the picker, stable to name; the edit reaches all uses ([`repaint`]).
    pub at: PaintAt,
}

/// The distinct **gradients** inside the subtrees rooted at `ids`, most-used first.
///
/// [`colors_in`]'s twin, over the paints it leaves out. Keyed by the whole `Brush`,
/// since two ramps of the same colours at different angles are two gradients.
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
/// One walk with a key function so the two answers agree about what they found.
fn census<K: PartialEq>(
    doc: &Document,
    ids: &[NodeId],
    key: impl Fn(&Brush) -> Option<K>,
) -> Vec<(K, usize, Vec<NodeId>, PaintAt)> {
    // A `Vec`: the counts are few, and insertion order makes ties stable.
    let mut counts: Vec<(K, usize, Vec<NodeId>, PaintAt)> = Vec::new();
    let mut bump = |brush: &Brush, at: PaintAt| {
        let Some(k) = key(brush) else { return };
        match counts.iter_mut().find(|(other, _, _, _)| *other == k) {
            Some((_, n, nodes, _)) => {
                *n += 1;
                // The layers, not the paint slots; pre-order makes checking the
                // tail enough to stay distinct and in document order.
                if nodes.last() != Some(&at.node) {
                    nodes.push(at.node);
                }
                // The location stays the first place met.
            }
            None => counts.push((k, 1, vec![at.node], at)),
        }
    };
    // Literal paints only: a bound paint's colour is its variable's, and a
    // recolour would have to detach it.
    for id in subtree_nodes(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        for (i, f) in node.paint().fills.iter().enumerate() {
            let Some(brush) = f.brush.literal() else {
                continue;
            };
            bump(
                brush,
                PaintAt {
                    node: id,
                    target: PaintTarget::Fill,
                    index: i,
                },
            );
        }
        for (i, s) in node.paint().strokes.iter().enumerate() {
            let Some(brush) = s.brush.literal() else {
                continue;
            };
            bump(
                brush,
                PaintAt {
                    node: id,
                    target: PaintTarget::Stroke,
                    index: i,
                },
            );
        }
    }
    // Stable, so equally-used paints do not shuffle between frames.
    counts.sort_by_key(|(_, n, _, _)| std::cmp::Reverse(*n));
    counts
}

/// The brush at a location, as the committed document holds it.
///
/// `None` when the place has gone (layer deleted, list shrank), which tells a UI
/// slot pointing at it to close. As held: a bound paint answers its binding; a
/// caller that draws it asks [`Document::resolve_brush`].
pub fn paint_at(doc: &Document, at: PaintAt) -> Option<crate::BrushSource> {
    let node = doc.get(at.node)?;
    match at.target {
        PaintTarget::Fill => node.paint().fills.get(at.index).map(|f| f.brush.clone()),
        PaintTarget::Stroke => node.paint().strokes.get(at.index).map(|s| s.brush.clone()),
    }
}

/// Replace every use of the brush `from` with `to` inside the subtrees rooted at
/// `ids` — every fill and every stroke, in one transaction.
///
/// Frames are included: a frame's ground is an ordinary fill entry (§15 D400).
///
/// [`recolor`]'s twin for the paints a colour cannot name. Not a generalisation of
/// it: `recolor` matches exact RGBA bytes, this matches whole brushes with `==`,
/// and folding them would make a row disagree with its own edit.
pub fn repaint(doc: &Document, ids: &[NodeId], from: &Brush, to: &Brush) -> Transaction {
    let mut ops = Vec::new();
    for id in subtree_nodes(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        // A bound paint is no brush's use: the census never lists one.
        let swap = |b: &crate::BrushSource| {
            if b.literal() == Some(from) {
                to.clone().into()
            } else {
                b.clone()
            }
        };
        let fills: Vec<Keyed<Fill>> = node
            .paint()
            .fills
            .iter()
            .map(|f| {
                f.map(|f| Fill {
                    brush: swap(&f.brush),
                    visible: f.visible,
                })
            })
            .collect();
        if fills != node.paint().fills {
            ops.push(Operation::SetFills { id, fills });
        }
        let strokes: Vec<Keyed<Stroke>> = node
            .paint()
            .strokes
            .iter()
            .map(|s| {
                s.map(|s| Stroke {
                    brush: swap(&s.brush),
                    ..s.clone()
                })
            })
            .collect();
        if strokes != node.paint().strokes {
            ops.push(Operation::SetStrokes { id, strokes });
        }
    }
    Transaction(ops)
}

/// Whether `brush` is exactly this solid colour of its own; a bound paint never is.
fn is_solid(brush: &crate::BrushSource, key: [u8; 4]) -> bool {
    matches!(brush.literal(), Some(Brush::Solid(c)) if c.to_rgba8().to_u8_array() == key)
}

/// Replace every use of `from` with `to` inside the subtrees rooted at `ids`.
///
/// The Group Colors edit: one colour, wherever it appears, in one transaction.
/// Matched on the exact RGBA bytes [`colors_in`] counts by, so the panel and this
/// cannot disagree. Nodes with no matching paint contribute no operation.
pub fn recolor(doc: &Document, ids: &[NodeId], from: Color, to: Color) -> Transaction {
    let key = from.to_rgba8().to_u8_array();
    // Opening and closing a picker without moving arrives as a same-colour
    // recolour; rewriting would be an invisible undo step.
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
                .map(|f| {
                    f.map(|f| Fill {
                        brush: if is_solid(&f.brush, key) {
                            to.into()
                        } else {
                            f.brush.clone()
                        },
                        visible: f.visible,
                    })
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
                        next.brush = to.into();
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
/// A group is organisation, so an edit passes through it to the shapes inside. A
/// frame is drawn and has paint of its own, so it is the target and the walk stops
/// there, including a frame reached through a group (§15 D400). That is why paint
/// has its own walk rather than [`subtree_nodes`], which the colour census wants.
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

/// What a paint panel has to show over a scope.
///
/// Two states: the targets agree on a list (possibly empty), or anything else at
/// all is one stand-in row, including one unpainted target among painted ones
/// (§15 D53). Agreement is on the whole list, since the panel lets rows be added
/// and removed over a selection.
#[derive(Clone, Debug, PartialEq)]
pub enum PaintShown<T> {
    /// These rows, true of every target. Empty means no target has any paint.
    List(Vec<T>),
    /// One stand-in row: the targets disagree and at least one has a paint for it
    /// to stand for. One is enough: "No stroke" would be false of half the
    /// selection. What the row says is decided property by property
    /// ([`shared_over_fills`], [`shared_over_strokes`]).
    Mixed,
}

impl<T> PaintShown<T> {
    /// The rows to draw; none for [`PaintShown::Mixed`], which has no list.
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
///
/// Agreement is by value, not by item (§15 D980): equal fills added separately
/// carry different item ids. The list returned is the first target's, ids and all,
/// the anchor a multi-selection write retargets from (`item::retarget`), never to
/// be written verbatim onto the others.
fn paint_shown<T: PartialEq>(lists: impl Iterator<Item = Vec<Keyed<T>>>) -> PaintShown<Keyed<T>> {
    let mut first: Option<Vec<Keyed<T>>> = None;
    for list in lists {
        match &first {
            None => first = Some(list),
            // Any disagreement is the mixed row; empty lists are equal, so a
            // disagreement means something in scope is painted.
            Some(f) if !same_values(f, &list) => return PaintShown::Mixed,
            Some(_) => {}
        }
    }
    PaintShown::List(first.unwrap_or_default())
}

/// The fill list every paint target in scope shares — or a disagreement.
pub fn shared_fills(doc: &Document, ids: &[NodeId]) -> PaintShown<Keyed<Fill>> {
    paint_shown(
        paint_targets(doc, ids)
            .into_iter()
            .filter_map(|id| doc.get(id))
            .map(|n| n.paint().fills.clone()),
    )
}

/// The stroke list every paint target in scope shares — or a disagreement.
/// [`shared_fills`]' twin, over the same targets.
pub fn shared_strokes(doc: &Document, ids: &[NodeId]) -> PaintShown<Keyed<Stroke>> {
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
/// Over all the fills of all the targets: the mixed row stands for every one, so
/// it may state only what they all state, property by property. A target with no
/// fill at all disagrees with every value (absence is a value), or a row would
/// state a 4pt stroke half the selection has not got.
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
/// The non-destructive half of the bulk paint API: each target keeps its own
/// list (length, order, every property `edit` does not touch). [`set_fills_all`]
/// is the other half, for "make them all this".
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

/// What an edit of a paint **as it is drawn** leaves it (§15 D1039): `edit` is
/// handed the brush the user sees — a bound paint's resolved solid — and its
/// answer is written back.
///
/// A bound paint keeps its binding when only its `%` moved: the variable carries
/// its own alpha and the fill's `%` is a separate multiplier (D1038), so the same
/// colour at another alpha is a new `%` (drawn alpha over the variable's). Any
/// other answer detaches to a colour of the user's own.
pub fn edited_brush(
    doc: &Document,
    id: NodeId,
    src: &crate::BrushSource,
    edit: impl Fn(&Brush) -> Brush,
) -> crate::BrushSource {
    use crate::BrushSource;
    match src {
        BrushSource::Literal(b) => edit(b).into(),
        BrushSource::Bound { var, alpha } => {
            let Some(crate::Value::Color(c)) = doc.resolve_var(id, *var) else {
                return src.clone();
            };
            let drawn = Brush::Solid(c.multiply_alpha(*alpha));
            let next = edit(&drawn);
            if next == drawn {
                return src.clone();
            }
            let rgb = |c: Color| {
                let [r, g, b, _] = c.to_rgba8().to_u8_array();
                [r, g, b]
            };
            match next {
                Brush::Solid(n) if rgb(n) == rgb(c) && c.components[3] > 0.0 => {
                    BrushSource::Bound {
                        var: *var,
                        alpha: (n.components[3] / c.components[3]).clamp(0.0, 1.0),
                    }
                }
                other => other.into(),
            }
        }
    }
}

/// Every fill's (or stroke's) brush of every paint target in scope, edited as
/// drawn by [`edited_brush`]: the mixed row's colour, hex and `%` writes.
/// [`edit_fills_all`] with the paint's node in hand, which the binding needs.
pub fn edit_brushes_all(
    doc: &Document,
    ids: &[NodeId],
    target: PaintTarget,
    edit: impl Fn(&Brush) -> Brush,
) -> Transaction {
    let mut ops = Vec::new();
    for id in paint_targets(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        match target {
            PaintTarget::Fill => {
                let fills: Vec<Keyed<Fill>> = node
                    .paint()
                    .fills
                    .iter()
                    .map(|f| {
                        f.map(|f| Fill {
                            brush: edited_brush(doc, id, &f.brush, &edit),
                            ..f.clone()
                        })
                    })
                    .collect();
                if fills != node.paint().fills {
                    ops.push(Operation::SetFills { id, fills });
                }
            }
            PaintTarget::Stroke => {
                let strokes: Vec<Keyed<Stroke>> = node
                    .paint()
                    .strokes
                    .iter()
                    .map(|s| {
                        s.map(|s| Stroke {
                            brush: edited_brush(doc, id, &s.brush, &edit),
                            ..s.clone()
                        })
                    })
                    .collect();
                if strokes != node.paint().strokes {
                    ops.push(Operation::SetStrokes { id, strokes });
                }
            }
        }
    }
    Transaction(ops)
}

/// Every paint target's top fill (or stroke), the last visible one, rebrushed with
/// what `top` makes of it as drawn, or a new one put on top where none is visible:
/// the eyedropper's write. `top` gets `None` for a layer with nothing visible.
/// Through [`edited_brush`], so a bound paint keeps its binding.
pub fn set_top_paint_all(
    doc: &Document,
    ids: &[NodeId],
    target: PaintTarget,
    top: impl Fn(Option<&Brush>) -> Brush,
    src: &mut IdSource,
) -> Transaction {
    fn retop<T: Clone + PartialEq>(
        own: &[Keyed<T>],
        visible: impl Fn(&T) -> bool,
        brush: impl Fn(&mut T) -> &mut crate::BrushSource,
        rebrush: impl Fn(Option<&crate::BrushSource>) -> crate::BrushSource,
        fresh_item: impl Fn(crate::BrushSource) -> T,
    ) -> Vec<T> {
        let mut values: Vec<T> = own.iter().map(|k| k.value.clone()).collect();
        match values.iter_mut().rev().find(|v| visible(v)) {
            Some(v) => {
                let next = rebrush(Some(&*brush(v)));
                *brush(v) = next;
            }
            None => values.push(fresh_item(rebrush(None))),
        }
        values
    }
    let mut ops = Vec::new();
    let mut fresh = crate::item::PastTheEnd::default();
    for id in paint_targets(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        let rebrush = |b: Option<&crate::BrushSource>| match b {
            Some(b) => edited_brush(doc, id, b, |drawn| top(Some(drawn))),
            None => top(None).into(),
        };
        match target {
            PaintTarget::Fill => {
                let own = &node.paint().fills;
                let values = retop(
                    own,
                    |f: &Fill| f.visible,
                    |f| &mut f.brush,
                    rebrush,
                    |brush| Fill {
                        brush,
                        visible: true,
                    },
                );
                if own.len() != values.len() || own.iter().zip(&values).any(|(o, v)| o.value != *v)
                {
                    ops.push(Operation::SetFills {
                        id,
                        fills: crate::item::rekey_by_position(own, values, &mut fresh, src),
                    });
                }
            }
            PaintTarget::Stroke => {
                let own = &node.paint().strokes;
                let values = retop(
                    own,
                    |s: &Stroke| s.visible,
                    |s| &mut s.brush,
                    rebrush,
                    |brush| Stroke {
                        brush,
                        ..Stroke::default()
                    },
                );
                if own.len() != values.len() || own.iter().zip(&values).any(|(o, v)| o.value != *v)
                {
                    ops.push(Operation::SetStrokes {
                        id,
                        strokes: crate::item::rekey_by_position(own, values, &mut fresh, src),
                    });
                }
            }
        }
    }
    Transaction(ops)
}

/// Bind rectangle `id`'s corners to `vars` — `None` leaving a corner as it is
/// (§15 D1039). The bound corners' literals are written 0, so a rectangle compares
/// equal to its instance's copy bound to the same variable (overrides compare the
/// binding, §5.3d); the literal is never drawn while bound.
///
/// Empty when `id` is not a rectangle or nothing would change.
pub fn bind_corners_ops(doc: &Document, id: NodeId, vars: crate::CornerVars) -> Vec<Operation> {
    let Some(node) = doc.get(id) else {
        return Vec::new();
    };
    let NodeKind::Rect { corner_radii, .. } = node.kind() else {
        return Vec::new();
    };
    let mut next = node.corner_vars();
    for (k, v) in vars.iter().enumerate() {
        if v.is_some() {
            next[k] = *v;
        }
    }
    let zero = |k: usize, lit: f64| if next[k].is_some() { 0.0 } else { lit };
    let radii = RoundedRectRadii {
        top_left: zero(0, corner_radii.top_left),
        top_right: zero(1, corner_radii.top_right),
        bottom_right: zero(2, corner_radii.bottom_right),
        bottom_left: zero(3, corner_radii.bottom_left),
    };
    let mut ops = Vec::new();
    if radii != *corner_radii {
        ops.push(Operation::SetGeometry {
            id,
            geometry: GeometryPatch::CornerRadii(radii),
        });
    }
    if next != node.corner_vars() {
        ops.push(Operation::SetCornerVars { id, vars: next });
    }
    ops
}

/// Detach rectangle `id`'s bound corners to the radii they draw (D1038's detach):
/// the literal becomes the resolved number and the binding goes. Empty when no
/// corner is bound. Any write of a radius to a bound corner must carry this, or it
/// lands on a literal nobody draws.
pub fn detach_corners_ops(doc: &Document, id: NodeId) -> Vec<Operation> {
    let Some(node) = doc.get(id) else {
        return Vec::new();
    };
    let NodeKind::Rect { corner_radii, .. } = node.kind() else {
        return Vec::new();
    };
    let Some(drawn) = doc.resolve_corners(id, *corner_radii) else {
        return Vec::new();
    };
    vec![
        Operation::SetGeometry {
            id,
            geometry: GeometryPatch::CornerRadii(drawn),
        },
        Operation::SetCornerVars {
            id,
            vars: Default::default(),
        },
    ]
}

/// [`detach_corners_ops`] for the one corner at `slot` of a `CornerVars` (TL,
/// TR, BR, BL) — a per-corner chip's detach (§15 D1049): that corner's literal
/// becomes the radius it draws and its binding goes, the other three as they
/// are. Empty when that corner is not bound.
pub fn detach_corner_ops(doc: &Document, id: NodeId, slot: usize) -> Vec<Operation> {
    let Some(node) = doc.get(id) else {
        return Vec::new();
    };
    let NodeKind::Rect { corner_radii, .. } = node.kind() else {
        return Vec::new();
    };
    let mut vars = node.corner_vars();
    if vars.get(slot).copied().flatten().is_none() {
        return Vec::new();
    }
    let Some(drawn) = doc.resolve_corners(id, *corner_radii) else {
        return Vec::new();
    };
    vars[slot] = None;
    let mut radii = *corner_radii;
    match slot {
        0 => radii.top_left = drawn.top_left,
        1 => radii.top_right = drawn.top_right,
        2 => radii.bottom_right = drawn.bottom_right,
        _ => radii.bottom_left = drawn.bottom_left,
    }
    vec![
        Operation::SetGeometry {
            id,
            geometry: GeometryPatch::CornerRadii(radii),
        },
        Operation::SetCornerVars { id, vars },
    ]
}

/// Item `index` of every target's fill (or stroke) list, given what `brush_of`
/// makes of it, handed the node so a binding can resolve there. The binding
/// verbs' one write (§15 D1041): bind, detach, and a literal typed over a chip,
/// which replaces the binding rather than going through [`edited_brush`].
pub fn set_brush_at_all(
    doc: &Document,
    ids: &[NodeId],
    target: PaintTarget,
    index: usize,
    brush_of: impl Fn(NodeId, &crate::BrushSource) -> crate::BrushSource,
) -> Transaction {
    let mut ops = Vec::new();
    for id in paint_targets(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        match target {
            PaintTarget::Fill => {
                let mut fills = node.paint().fills.clone();
                let Some(f) = fills.get_mut(index) else {
                    continue;
                };
                f.brush = brush_of(id, &f.brush);
                if fills != node.paint().fills {
                    ops.push(Operation::SetFills { id, fills });
                }
            }
            PaintTarget::Stroke => {
                let mut strokes = node.paint().strokes.clone();
                let Some(s) = strokes.get_mut(index) else {
                    continue;
                };
                s.brush = brush_of(id, &s.brush);
                if strokes != node.paint().strokes {
                    ops.push(Operation::SetStrokes { id, strokes });
                }
            }
        }
    }
    Transaction(ops)
}

/// Bind item `index` of every target's list to colour variable `var` (D1038):
/// the paint becomes the variable's colour, **keeping its own `%`** — a literal
/// solid's alpha, a bound paint's binding alpha, full for anything else.
pub fn bind_brush_at_all(
    doc: &Document,
    ids: &[NodeId],
    target: PaintTarget,
    index: usize,
    var: crate::VarId,
) -> Transaction {
    set_brush_at_all(doc, ids, target, index, |_, b| {
        let alpha = match b {
            crate::BrushSource::Literal(Brush::Solid(c)) => c.components[3],
            crate::BrushSource::Bound { alpha, .. } => *alpha,
            crate::BrushSource::Literal(_) => 1.0,
        };
        crate::BrushSource::Bound { var, alpha }
    })
}

/// Detach item `index` of every target's list: each bound paint becomes the
/// solid it draws there (D1038's *Detach variable*).
pub fn detach_brush_at_all(
    doc: &Document,
    ids: &[NodeId],
    target: PaintTarget,
    index: usize,
) -> Transaction {
    set_brush_at_all(doc, ids, target, index, |id, b| {
        doc.resolve_brush(id, b).into_owned().into()
    })
}

/// The corner bindings rectangle `id` keeps when its radii are written `written`
/// (TL, TR, BR, BL) — [`keep_corner_bindings`]' judgement, corner by corner: a
/// bound corner written what it draws, or its own literal, keeps its variable;
/// any other comes back `None`. Shared with the preview (`absorb`'s `SetGeometry`
/// arm), so a kept corner is not drawn square until the commit.
pub fn kept_corner_vars(doc: &Document, id: NodeId, written: [f64; 4]) -> crate::CornerVars {
    let Some(node) = doc.get(id) else {
        return Default::default();
    };
    let NodeKind::Rect { corner_radii, .. } = node.kind() else {
        return Default::default();
    };
    let bound = node.corner_vars();
    let drawn = doc
        .resolve_corners(id, *corner_radii)
        .unwrap_or(*corner_radii);
    let drawn = [
        drawn.top_left,
        drawn.top_right,
        drawn.bottom_right,
        drawn.bottom_left,
    ];
    let literal = [
        corner_radii.top_left,
        corner_radii.top_right,
        corner_radii.bottom_right,
        corner_radii.bottom_left,
    ];
    std::array::from_fn(|k| bound[k].filter(|_| written[k] == drawn[k] || written[k] == literal[k]))
}

/// A radius written over a bound corner detaches it (D1038 (2)): the commit-time
/// half of the binding's rule, so no writer of a radius has to know about
/// bindings. This adds the `SetCornerVars` that lets the written value be drawn.
///
/// Compared against what each corner draws, not its literal: the fields read the
/// corners as drawn, so a per-corner edit hands back all four and a bare click
/// writes what is already drawn. A bound corner written its drawn value stays
/// bound, its literal put back to 0 in the patch; any other value detaches it.
///
/// One written its own literal stays bound too, since some writers build the radii
/// from the document's kind (the Scale tool's `0 × s`, a per-corner ↺). The price:
/// a 0 typed over a bound corner detaches nothing; the chip's retype is that
/// value's door.
///
/// A transaction that sets a node's corner bindings itself is left to say what it
/// means.
pub fn keep_corner_bindings(doc: &Document, mut tx: Transaction) -> Transaction {
    let sets_vars: FxHashSet<NodeId> =
        tx.0.iter()
            .filter_map(|op| match op {
                Operation::SetCornerVars { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
    let mut detached: FxHashMap<NodeId, crate::CornerVars> = FxHashMap::default();
    for op in tx.0.iter_mut() {
        let Operation::SetGeometry { id, geometry } = op else {
            continue;
        };
        if sets_vars.contains(id) {
            continue;
        }
        let Some(node) = doc.get(*id) else { continue };
        let NodeKind::Rect { corner_radii, .. } = node.kind() else {
            continue;
        };
        let bound = node.corner_vars();
        if bound.iter().all(Option::is_none) {
            continue;
        }
        let mut written = match geometry {
            GeometryPatch::CornerRadius(r) => [*r; 4],
            GeometryPatch::CornerRadii(r) => {
                [r.top_left, r.top_right, r.bottom_right, r.bottom_left]
            }
            _ => continue,
        };
        let literal = [
            corner_radii.top_left,
            corner_radii.top_right,
            corner_radii.bottom_right,
            corner_radii.bottom_left,
        ];
        let kept = kept_corner_vars(doc, *id, written);
        let vars = detached.entry(*id).or_insert(bound);
        for k in 0..4 {
            if vars[k].is_none() {
                continue;
            }
            // Its drawn value, or its own literal, leaves it bound.
            if kept[k].is_some() {
                written[k] = literal[k];
            } else {
                vars[k] = None;
            }
        }
        *geometry = GeometryPatch::CornerRadii(RoundedRectRadii {
            top_left: written[0],
            top_right: written[1],
            bottom_right: written[2],
            bottom_left: written[3],
        });
    }
    let mut ids: Vec<NodeId> = detached.keys().copied().collect();
    ids.sort();
    for id in ids {
        let vars = detached[&id];
        if doc.get(id).is_some_and(|n| n.corner_vars() != vars) {
            tx.0.push(Operation::SetCornerVars { id, vars });
        }
    }
    tx
}

/// Bind container `id`'s gaps and padding to `vars` — `None` leaving a slot as
/// it is (§15 D1058), [`bind_corners_ops`]' shape: the bound slots' literals are
/// written 0 and never laid out while bound. Empty when `id` has no layout or
/// nothing would change.
pub fn bind_spacing_ops(doc: &Document, id: NodeId, vars: crate::SpacingVars) -> Vec<Operation> {
    let Some(node) = doc.get(id) else {
        return Vec::new();
    };
    let Some(display) = node.display() else {
        return Vec::new();
    };
    let mut next = node.spacing_vars();
    for (k, v) in vars.iter().enumerate() {
        if v.is_some() {
            next[k] = *v;
        }
    }
    let mut s = display.spacing();
    for k in 0..6 {
        if next[k].is_some() {
            s[k] = 0.0;
        }
    }
    let mut ops = Vec::new();
    if s != display.spacing() {
        let mut d = display.clone();
        d.set_spacing(s);
        ops.push(Operation::SetDisplay {
            id,
            display: Some(d),
        });
    }
    if next != node.spacing_vars() {
        ops.push(Operation::SetSpacingVars { id, vars: next });
    }
    ops
}

/// Detach the slots of container `id` that `which` names to the numbers they
/// lay out at (D1038's detach, §15 D1058): each literal becomes its variable's
/// number and the binding goes, the other slots as they are. Empty when none of
/// them is bound.
pub fn detach_spacing_ops(doc: &Document, id: NodeId, which: [bool; 6]) -> Vec<Operation> {
    let Some(node) = doc.get(id) else {
        return Vec::new();
    };
    let Some(display) = node.display() else {
        return Vec::new();
    };
    let mut vars = node.spacing_vars();
    if !(0..6).any(|k| which[k] && vars[k].is_some()) {
        return Vec::new();
    }
    let drawn = doc
        .spacing_drawn_in(doc.variables(), id, display, vars)
        .spacing();
    let mut s = display.spacing();
    for k in 0..6 {
        if which[k] && vars[k].is_some() {
            s[k] = drawn[k];
            vars[k] = None;
        }
    }
    let mut d = display.clone();
    d.set_spacing(s);
    vec![
        Operation::SetDisplay {
            id,
            display: Some(d),
        },
        Operation::SetSpacingVars { id, vars },
    ]
}

/// The bindings container `id` keeps when its layout is written `written` —
/// [`keep_spacing_bindings`]' judgement, slot by slot (§15 D1058): a bound slot
/// written what it lays out at, or its own literal, keeps its variable; any
/// other slot comes back `None`. Shared with the preview (`absorb`'s `SetDisplay`
/// arm), so a scrub draws the slot where the commit will put it.
pub fn kept_spacing_vars(
    doc: &Document,
    id: NodeId,
    written: &crate::container::Display,
) -> crate::SpacingVars {
    let Some(node) = doc.get(id) else {
        return Default::default();
    };
    let bound = node.spacing_vars();
    let Some(was) = node.display() else {
        return Default::default();
    };
    let literal = was.spacing();
    let drawn = doc
        .spacing_drawn_in(doc.variables(), id, was, bound)
        .spacing();
    let w = written.spacing();
    std::array::from_fn(|k| bound[k].filter(|_| w[k] == drawn[k] || w[k] == literal[k]))
}

/// A gap or padding written over a bound slot detaches it (D1038 (2), §15 D1058):
/// [`keep_corner_bindings`]' rule for a container's spacing, adding the
/// `SetSpacingVars` that lets the written value be laid out. Propagation runs
/// after it (`propagate::owed`), so a copy follows a detach through the main's
/// `SetSpacingVars`.
///
/// The same two halves: a slot written what it lays out at, or its own literal 0
/// (writers that carry the other slots forward, the flex/grid switch, the Scale
/// tool), stays bound with the literal put back to 0; anything else detaches. A 0
/// typed over a bound slot keeps the binding; the chip's retype is its door.
///
/// A layout taken away (`SetDisplay` to `None`) clears every binding. A
/// transaction that sets the node's spacing bindings itself is left alone.
pub fn keep_spacing_bindings(doc: &Document, mut tx: Transaction) -> Transaction {
    let sets_vars: FxHashSet<NodeId> =
        tx.0.iter()
            .filter_map(|op| match op {
                Operation::SetSpacingVars { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
    let mut detached: FxHashMap<NodeId, crate::SpacingVars> = FxHashMap::default();
    for op in tx.0.iter_mut() {
        let Operation::SetDisplay { id, display } = op else {
            continue;
        };
        if sets_vars.contains(id) {
            continue;
        }
        let Some(node) = doc.get(*id) else { continue };
        let bound = node.spacing_vars();
        if bound.iter().all(Option::is_none) {
            continue;
        }
        let vars = detached.entry(*id).or_insert(bound);
        let Some(display) = display.as_mut() else {
            *vars = Default::default();
            continue;
        };
        let kept = kept_spacing_vars(doc, *id, display);
        let literal = node.display().map_or([0.0; 6], |w| w.spacing());
        let mut written = display.spacing();
        for k in 0..6 {
            if vars[k].is_none() {
                continue;
            }
            if kept[k].is_some() {
                written[k] = literal[k];
            } else {
                vars[k] = None;
            }
        }
        display.set_spacing(written);
    }
    let mut ids: Vec<NodeId> = detached.keys().copied().collect();
    ids.sort();
    for id in ids {
        let vars = detached[&id];
        if doc.get(id).is_some_and(|n| n.spacing_vars() != vars) {
            tx.0.push(Operation::SetSpacingVars { id, vars });
        }
    }
    tx
}

/// Bind layer `id`'s visibility to boolean variable `var` (§15 D1059). The literal
/// is written true ([`bind_corners_ops`]' canonical-literal rule) and never read
/// while bound. Empty when nothing would change.
pub fn bind_visible_ops(doc: &Document, id: NodeId, var: crate::VarId) -> Vec<Operation> {
    let Some(node) = doc.get(id) else {
        return Vec::new();
    };
    let mut ops = Vec::new();
    if !node.visible() {
        ops.push(Operation::SetVisible { id, visible: true });
    }
    if node.visible_var() != Some(var) {
        ops.push(Operation::SetVisibleVar { id, var: Some(var) });
    }
    ops
}

/// Detach layer `id`'s visibility to whether it is shown (D1038's detach, §15
/// D1059). Empty when it is not bound.
pub fn detach_visible_ops(doc: &Document, id: NodeId) -> Vec<Operation> {
    let Some(node) = doc.get(id) else {
        return Vec::new();
    };
    if node.visible_var().is_none() {
        return Vec::new();
    }
    vec![
        Operation::SetVisible {
            id,
            visible: doc.shown(id),
        },
        Operation::SetVisibleVar { id, var: None },
    ]
}

/// A visibility written over a bound layer detaches it (D1038 (2), §15 D1059):
/// the commit-time half of the binding, so every writer of `SetVisible` is
/// unchanged.
///
/// One half, not [`keep_corner_bindings`]' two: a write of what the layer shows
/// stays bound, but a write of its own literal is not excused. The literal is the
/// `true` [`bind_visible_ops`] writes, and a hidden bound layer brought back with
/// `visible: true` writes exactly that, so the edit would do nothing.
///
/// A transaction that sets the layer's binding itself is left alone.
pub fn keep_visible_bindings(doc: &Document, mut tx: Transaction) -> Transaction {
    let sets_var: FxHashSet<NodeId> =
        tx.0.iter()
            .filter_map(|op| match op {
                Operation::SetVisibleVar { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
    let mut detached: Vec<NodeId> = Vec::new();
    for op in tx.0.iter_mut() {
        let Operation::SetVisible { id, visible } = op else {
            continue;
        };
        if sets_var.contains(id) {
            continue;
        }
        let Some(node) = doc.get(*id) else { continue };
        if node.visible_var().is_none() {
            continue;
        }
        if *visible == doc.shown(*id) {
            *visible = node.visible();
        } else if !detached.contains(id) {
            detached.push(*id);
        }
    }
    detached.sort();
    for id in detached {
        tx.0.push(Operation::SetVisibleVar { id, var: None });
    }
    tx
}

/// Bind layer `id`'s opacity to number variable `var` (§15 D1072), read as a
/// percentage. The literal is written 0 (D1039's canonical literal), so a reader
/// that skips [`Document::drawn_opacity`] draws the layer invisible rather than
/// nearly right. Empty when nothing would change.
pub fn bind_opacity_ops(doc: &Document, id: NodeId, var: crate::VarId) -> Vec<Operation> {
    let Some(node) = doc.get(id) else {
        return Vec::new();
    };
    let mut ops = Vec::new();
    if node.opacity() != 0.0 {
        ops.push(Operation::SetOpacity { id, opacity: 0.0 });
    }
    if node.opacity_var() != Some(var) {
        ops.push(Operation::SetOpacityVar { id, var: Some(var) });
    }
    ops
}

/// Detach layer `id`'s opacity to what it draws (D1038's detach, §15 D1072).
/// Empty when it is not bound.
pub fn detach_opacity_ops(doc: &Document, id: NodeId) -> Vec<Operation> {
    let Some(node) = doc.get(id) else {
        return Vec::new();
    };
    if node.opacity_var().is_none() {
        return Vec::new();
    }
    vec![
        Operation::SetOpacity {
            id,
            opacity: doc.drawn_opacity(id),
        },
        Operation::SetOpacityVar { id, var: None },
    ]
}

/// An opacity written over a bound layer detaches it (D1038 (2), §15 D1072): the
/// commit-time half of the binding, so every writer of `SetOpacity` (the field,
/// [`set_opacity_all`], number keys, *Paste properties*, MCP) is unchanged.
///
/// One half, [`keep_visible_bindings`]' rule: a write of what the layer draws
/// stays bound, but its own literal is not excused. The literal is the 0
/// [`bind_opacity_ops`] writes, and 0% typed over a bound layer means make it
/// invisible, so excusing it would do nothing.
///
/// A transaction that sets the layer's binding itself is left alone.
pub fn keep_opacity_bindings(doc: &Document, mut tx: Transaction) -> Transaction {
    let sets_var: FxHashSet<NodeId> =
        tx.0.iter()
            .filter_map(|op| match op {
                Operation::SetOpacityVar { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
    let mut detached: Vec<NodeId> = Vec::new();
    for op in tx.0.iter_mut() {
        let Operation::SetOpacity { id, opacity } = op else {
            continue;
        };
        if sets_var.contains(id) {
            continue;
        }
        let Some(node) = doc.get(*id) else { continue };
        if node.opacity_var().is_none() {
            continue;
        }
        if (*opacity - doc.drawn_opacity(*id)).abs() < OPACITY_AGREEMENT {
            *opacity = node.opacity();
        } else if !detached.contains(id) {
            detached.push(*id);
        }
    }
    detached.sort();
    for id in detached {
        tx.0.push(Operation::SetOpacityVar { id, var: None });
    }
    tx
}

/// Bind stroke `item` of layer `id` to number variable `var` (§15 D1073): its
/// width written 0 (D1039's canonical literal), the binding beside it in the item.
/// Empty when nothing would change, the layer has no such stroke, or the stroke
/// draws `Custom` sides ([`draws_own_sides`]), which draw none of `width`
/// (`StrokeSides::widths`), so a binding there would be bound to nothing drawn.
pub fn bind_stroke_width_ops(
    doc: &Document,
    id: NodeId,
    item: crate::ItemId,
    var: crate::VarId,
) -> Vec<Operation> {
    let Some(node) = doc.get(id) else {
        return Vec::new();
    };
    let mut strokes = node.paint().strokes.clone();
    let Some(s) = strokes.iter_mut().find(|s| s.id == item) else {
        return Vec::new();
    };
    if (s.width == 0.0 && s.width_var == Some(var)) || draws_own_sides(node.kind(), s) {
        return Vec::new();
    }
    s.width = 0.0;
    s.width_var = Some(var);
    vec![Operation::SetStrokes { id, strokes }]
}

/// Whether `stroke` on a layer of `kind` draws `Custom` sides — its own four
/// widths and none of `width` (§15 D1073). Through
/// [`crate::geometry::effective_sides`], so a `Custom` stroke on an ellipse or a
/// path, which draws its whole outline at `width`, binds like any other.
pub fn draws_own_sides(kind: &NodeKind, stroke: &Stroke) -> bool {
    matches!(
        crate::geometry::effective_sides(kind, stroke),
        crate::StrokeSides::Custom { .. }
    )
}

/// Detach stroke `item` of layer `id` to the width it draws (D1038's detach,
/// §15 D1073). Empty when it is not bound.
pub fn detach_stroke_width_ops(doc: &Document, id: NodeId, item: crate::ItemId) -> Vec<Operation> {
    let Some(node) = doc.get(id) else {
        return Vec::new();
    };
    let mut strokes = node.paint().strokes.clone();
    let Some(s) = strokes.iter_mut().find(|s| s.id == item) else {
        return Vec::new();
    };
    let Some(var) = s.width_var else {
        return Vec::new();
    };
    s.width = doc.stroke_width_drawn_in(doc.variables(), id, var);
    s.width_var = None;
    vec![Operation::SetStrokes { id, strokes }]
}

/// `strokes`, a stroke list about to be written to layer `id`, with a bound width
/// the write leaves bound kept bound and any other detached (§15 D1073). The
/// commit's rule ([`keep_stroke_width_bindings`]) and the preview's, so they
/// cannot disagree.
///
/// Two halves, [`keep_corner_bindings`]' not the opacity's: a stroke whose binding
/// the write carries unchanged stays bound when its width is written what it
/// draws or its own literal 0 (put back); any other width detaches. The literal
/// half is load-bearing: every stroke-card edit and the Scale tool rebuild the
/// whole list from the stored items, so a bound item arrives with its 0. A 0 typed
/// over a bound width keeps the binding; the chip's retype is its door. A stroke
/// whose binding the write changes is left alone, except one drawing its own sides
/// ([`draws_own_sides`]), which is detached at the width it drew.
pub fn kept_stroke_widths(
    doc: &Document,
    id: NodeId,
    mut strokes: Vec<crate::Keyed<Stroke>>,
) -> Vec<crate::Keyed<Stroke>> {
    let Some(node) = doc.get(id) else {
        return strokes;
    };
    for s in strokes.iter_mut() {
        let Some(var) = s.width_var else { continue };
        // A stroke drawing its own sides holds no width binding, whoever wrote it
        // (a paste of an ellipse's bound `Custom` stroke onto a rect).
        if draws_own_sides(node.kind(), s) {
            s.width = doc.stroke_width_drawn_in(doc.variables(), id, var);
            s.width_var = None;
            continue;
        }
        let Some(old) = node.paint().strokes.iter().find(|o| o.id == s.id) else {
            continue;
        };
        if old.width_var != Some(var) {
            continue;
        }
        let drawn = doc.stroke_width_drawn_in(doc.variables(), id, var);
        if s.width == old.width || (s.width - drawn).abs() < 1e-9 {
            s.width = old.width;
        } else {
            s.width_var = None;
        }
    }
    strokes
}

/// Bind number `slot` ([`crate::EffectVars`]' places) of effect `item` on layer
/// `id` to number variable `var` (§15 D1074): its literal written 0, the binding
/// beside it in the item. Empty when nothing would change, the layer has no such
/// effect, or its kind has no such number (a layer blur's X, Y or spread; any
/// filter's).
pub fn bind_effect_ops(
    doc: &Document,
    id: NodeId,
    item: crate::ItemId,
    slot: usize,
    var: crate::VarId,
) -> Vec<Operation> {
    let Some(node) = doc.get(id) else {
        return Vec::new();
    };
    let mut effects = node.effects().to_vec();
    let Some(e) = effects.iter_mut().find(|e| e.id == item) else {
        return Vec::new();
    };
    match e.numbers().get(slot).copied().flatten() {
        None => return Vec::new(),
        Some(x) if x == 0.0 && e.vars[slot] == Some(var) => return Vec::new(),
        Some(_) => {}
    }
    e.value.set_number(slot, 0.0);
    e.vars[slot] = Some(var);
    vec![Operation::SetEffects { id, effects }]
}

/// Detach number `slot` of effect `item` on layer `id` to what it draws (D1038's
/// detach, §15 D1074). Empty when it is not bound.
pub fn detach_effect_ops(
    doc: &Document,
    id: NodeId,
    item: crate::ItemId,
    slot: usize,
) -> Vec<Operation> {
    let Some(node) = doc.get(id) else {
        return Vec::new();
    };
    let mut effects = node.effects().to_vec();
    let Some(e) = effects.iter_mut().find(|e| e.id == item) else {
        return Vec::new();
    };
    let Some(var) = e.vars.get(slot).copied().flatten() else {
        return Vec::new();
    };
    let x = doc.effect_number_drawn_in(doc.variables(), id, var, slot);
    e.value.set_number(slot, x);
    e.vars[slot] = None;
    vec![Operation::SetEffects { id, effects }]
}

/// `effects`, an effect list about to be written to layer `id`, with each bound
/// number the write leaves bound kept bound and any other detached (§15 D1074).
/// The commit's rule ([`keep_effect_bindings`]) and the preview's,
/// [`kept_stroke_widths`]' shape.
///
/// Corners' two halves: a number whose binding the write carries unchanged stays
/// bound when written what it draws or its own literal 0 (put back); anything else
/// detaches. The literal half is load-bearing: every effects-card edit rebuilds
/// the whole list from the stored items. A 0 typed over a bound number keeps it.
/// A number whose binding the write changes is left alone, except a binding on a
/// number the effect's kind no longer has, which goes whoever wrote it.
pub fn kept_effect_vars(
    doc: &Document,
    id: NodeId,
    mut effects: Vec<crate::Keyed<crate::Effect>>,
) -> Vec<crate::Keyed<crate::Effect>> {
    let Some(node) = doc.get(id) else {
        return effects;
    };
    for e in effects.iter_mut() {
        let numbers = e.numbers();
        let old = node.effects().iter().find(|o| o.id == e.id);
        for (slot, number) in numbers.into_iter().enumerate() {
            let Some(var) = e.vars[slot] else { continue };
            let Some(x) = number else {
                e.vars[slot] = None;
                continue;
            };
            let Some(old) = old.filter(|o| o.vars[slot] == Some(var)) else {
                continue;
            };
            let literal = old.numbers()[slot];
            let drawn = doc.effect_number_drawn_in(doc.variables(), id, var, slot);
            if literal == Some(x) || (x - drawn).abs() < 1e-9 {
                e.value.set_number(slot, literal.unwrap_or(0.0));
            } else {
                e.vars[slot] = None;
            }
        }
    }
    effects
}

/// A number written over a bound effect detaches it (D1038 (2), §15 D1074): the
/// commit-time half of the binding, every `SetEffects` through [`kept_effect_vars`].
pub fn keep_effect_bindings(doc: &Document, mut tx: Transaction) -> Transaction {
    for op in tx.0.iter_mut() {
        if let Operation::SetEffects { id, effects } = op {
            *effects = kept_effect_vars(doc, *id, std::mem::take(effects));
        }
    }
    tx
}

/// A width written over a bound stroke detaches it (D1038 (2), §15 D1073): the
/// commit-time half of the binding, every `SetStrokes` through [`kept_stroke_widths`].
pub fn keep_stroke_width_bindings(doc: &Document, mut tx: Transaction) -> Transaction {
    for op in tx.0.iter_mut() {
        if let Operation::SetStrokes { id, strokes } = op {
            *strokes = kept_stroke_widths(doc, *id, std::mem::take(strokes));
        }
    }
    tx
}

/// Which of `id`'s width and height the user sets: the axes a number variable may
/// bind (§15 D1061, only a hand-set size is bindable). `[false; 2]` for a kind
/// with no stored size ([`NodeKind::box_size`]), a text box among them.
///
/// An axis is computed, and so not bindable, when:
///
/// - its own keyword is `%` or `fit-content`;
/// - it is pinned at both ends by insets in a container with a box;
/// - it is an in-flow flex item's main axis with growth or a basis, or its cross
///   axis drawn stretched ([`stretched_across`]);
/// - it is an in-flow grid item stretched across its cell on that axis (a
///   `stretch` keyword or a box under `normal`, its size `auto`).
///
/// Asked of a [`crate::container::LayoutView`] so the commit can ask it of the
/// tree an edit leaves ([`keep_size_bindings`]). Not a measurement of the laid-out
/// box: shrink and `min-`/`max-` can squeeze an authored axis, and the binding
/// keeps its number (the CSS reading of `width`).
pub fn size_axes_authored(view: &dyn crate::container::LayoutView, id: NodeId) -> [bool; 2] {
    use crate::container::{AlignItems, Dimension, Display};
    let Some(kind) = view.kind(id) else {
        return [false; 2];
    };
    if kind.box_size().is_none() {
        return [false; 2];
    }
    let item = view.item(id);
    let dims = [item.width, item.height];
    let mut out = dims.map(|d| matches!(d, Dimension::Auto | Dimension::Px(_)));
    let boxed = view.parent(id).is_some_and(|p| match view.kind(p) {
        Some(NodeKind::Artboard { .. }) => true,
        Some(NodeKind::Group) => view.display(p).is_some(),
        _ => false,
    });
    let ins = view.insets(id);
    if boxed && ins.left.is_some() && ins.right.is_some() {
        out[0] = false;
    }
    if boxed && ins.top.is_some() && ins.bottom.is_some() {
        out[1] = false;
    }
    if crate::container::parent_lays_out(view, id) && crate::container::in_flow(view, id) {
        let parent = view.parent(id).and_then(|p| view.display(p));
        match parent.as_deref() {
            Some(Display::Flex(flex)) => {
                let main = if flex.direction.is_row() { 0 } else { 1 };
                if item.grow > 0.0 || item.basis != Dimension::Auto {
                    out[main] = false;
                }
                if stretched_across(flex, &item, &kind, false) {
                    out[1 - main] = false;
                }
            }
            Some(Display::Grid(grid)) => {
                let replaced = crate::container::is_replaced(view, id);
                let axes = [
                    (item.justify_self, grid.justify_items),
                    (item.align_self, grid.align_items),
                ];
                for (k, (own, items)) in axes.into_iter().enumerate() {
                    let normal = own.is_none() && items.is_none();
                    let stretched =
                        (normal && !replaced) || own.or(items) == Some(AlignItems::Stretch);
                    if stretched && dims[k] == Dimension::Auto {
                        out[k] = false;
                    }
                }
            }
            None => {}
        }
    }
    out
}

/// [`size_axes_authored`] of `doc` as committed — the inspector's question
/// before it offers a W or H a chip (§15 D1061).
pub fn bindable_size_axes(doc: &Document, id: NodeId) -> [bool; 2] {
    size_axes_authored(&crate::resolve::DocView(doc), id)
}

/// Bind layer `id`'s width and height to `vars` — `None` leaving an axis as it
/// is (§15 D1061), [`bind_corners_ops`]' shape: a bound axis's literal is written
/// 0 and never drawn while bound. Empty for a kind with no stored size, an axis
/// something else sizes ([`size_axes_authored`]), or when nothing would change.
pub fn bind_size_ops(doc: &Document, id: NodeId, vars: crate::SizeVars) -> Vec<Operation> {
    let Some(node) = doc.get(id) else {
        return Vec::new();
    };
    let Some(size) = node.kind().box_size() else {
        return Vec::new();
    };
    let authored = size_axes_authored(&crate::resolve::DocView(doc), id);
    let mut next = node.size_vars();
    for k in 0..2 {
        if vars[k].is_some() && authored[k] {
            next[k] = vars[k];
        }
    }
    let mut s = [size.width, size.height];
    for k in 0..2 {
        if next[k].is_some() {
            s[k] = 0.0;
        }
    }
    let mut ops = Vec::new();
    if s != [size.width, size.height] {
        ops.push(Operation::SetGeometry {
            id,
            geometry: crate::GeometryPatch::Size(kurbo::Size::new(s[0], s[1])),
        });
    }
    if next != node.size_vars() {
        ops.push(Operation::SetSizeVars { id, vars: next });
    }
    ops
}

/// Detach the axes of layer `id` that `which` names to the sizes they are drawn
/// at (D1038's detach, §15 D1061): each literal becomes its variable's number
/// and the binding goes, the other axis as it is. Empty when neither is bound.
pub fn detach_size_ops(doc: &Document, id: NodeId, which: [bool; 2]) -> Vec<Operation> {
    let Some(node) = doc.get(id) else {
        return Vec::new();
    };
    let Some(size) = node.kind().box_size() else {
        return Vec::new();
    };
    let mut vars = node.size_vars();
    if !(0..2).any(|k| which[k] && vars[k].is_some()) {
        return Vec::new();
    }
    let drawn = doc
        .size_drawn_in(doc.variables(), id, node.kind(), vars)
        .box_size()
        .unwrap_or(size);
    let (d, mut s) = ([drawn.width, drawn.height], [size.width, size.height]);
    for k in 0..2 {
        if which[k] && vars[k].is_some() {
            s[k] = d[k];
            vars[k] = None;
        }
    }
    vec![
        Operation::SetGeometry {
            id,
            geometry: crate::GeometryPatch::Size(kurbo::Size::new(s[0], s[1])),
        },
        Operation::SetSizeVars { id, vars },
    ]
}

/// The bindings layer `id` keeps when its size is written `written` —
/// [`keep_size_bindings`]' judgement, axis by axis (§15 D1061): a bound axis
/// written what it draws, or its own literal, keeps its variable; any other comes
/// back `None`. "What it draws" is its variable's number and, given `res`, the
/// used size a layout's shrink or min/max squeezed it to (what a writer of the
/// other axis carries forward, D874). Shared with the preview (`absorb`'s
/// `SetGeometry` arm).
pub fn kept_size_vars(
    doc: &Document,
    res: Option<&crate::resolve::Resolved>,
    id: NodeId,
    written: kurbo::Size,
) -> crate::SizeVars {
    let Some(node) = doc.get(id) else {
        return Default::default();
    };
    let bound = node.size_vars();
    let Some(literal) = node.kind().box_size() else {
        return Default::default();
    };
    let drawn = doc
        .size_drawn_in(doc.variables(), id, node.kind(), bound)
        .box_size()
        .unwrap_or(literal);
    let used = res
        .and_then(|r| r.used_kind(doc, id))
        .and_then(NodeKind::box_size)
        .unwrap_or(drawn);
    let (l, d, u, w) = (
        [literal.width, literal.height],
        [drawn.width, drawn.height],
        [used.width, used.height],
        [written.width, written.height],
    );
    std::array::from_fn(|k| bound[k].filter(|_| w[k] == d[k] || w[k] == l[k] || w[k] == u[k]))
}

/// A size written over a bound axis detaches it (D1038 (2), §15 D1061):
/// [`keep_corner_bindings`]' rule for a width and a height, adding the
/// `SetSizeVars` that lets the written size be drawn. Propagation runs after it
/// (`propagate::owed`).
///
/// The corners' two halves: an axis written what it draws, or its own literal 0
/// (a writer building the size from the stored kind, D874), stays bound with the
/// literal put back to 0; anything else detaches. A 0 typed over a bound axis
/// keeps the binding; the chip's retype is its door.
///
/// An axis an edit makes computed is detached too (only a hand-set size is
/// bindable): a keyword set to `%`/`fit-content`, a second pin, growth, a basis or
/// stretch, a reparent into a layout, a layer shown or unmasked into the flow, a
/// bound layer pasted into a layout. Asked of the tree the transaction leaves
/// ([`size_axes_authored`] on a scratch copy), and only when the transaction
/// touches the layout of a bound layer or its parent. The literal it detaches to
/// is the size drawn before the edit.
///
/// A transaction that sets a layer's size bindings itself is left alone. `res` is
/// the layout `doc` was resolved to, for [`kept_size_vars`]' used size.
pub fn keep_size_bindings(
    doc: &Document,
    res: Option<&crate::resolve::Resolved>,
    mut tx: Transaction,
) -> Transaction {
    let sets_vars: FxHashSet<NodeId> =
        tx.0.iter()
            .filter_map(|op| match op {
                Operation::SetSizeVars { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
    let bound = |id: NodeId| {
        !sets_vars.contains(&id)
            && doc
                .get(id)
                .is_some_and(|n| n.size_vars().iter().any(Option::is_some))
    };
    let mut vars: FxHashMap<NodeId, crate::SizeVars> = FxHashMap::default();
    for op in tx.0.iter_mut() {
        let Operation::SetGeometry {
            id,
            geometry: crate::GeometryPatch::Size(size),
        } = op
        else {
            continue;
        };
        if !bound(*id) {
            continue;
        }
        let Some(node) = doc.get(*id) else { continue };
        let Some(literal) = node.kind().box_size() else {
            continue;
        };
        let v = vars.entry(*id).or_insert(node.size_vars());
        let kept = kept_size_vars(doc, res, *id, *size);
        if v[0].is_some() {
            if kept[0].is_some() {
                size.width = literal.width;
            } else {
                v[0] = None;
            }
        }
        if v[1].is_some() {
            if kept[1].is_some() {
                size.height = literal.height;
            } else {
                v[1] = None;
            }
        }
    }
    // The layout half: a bound layer whose layout fields, insets, parent, parent's
    // layout or place in the flow the transaction writes, or that it brings in.
    let mut touched: Vec<NodeId> = Vec::new();
    let mut arrived: FxHashMap<NodeId, crate::SizeVars> = FxHashMap::default();
    for op in &tx.0 {
        match op {
            Operation::SetLayoutItem { id, .. }
            | Operation::SetInsets { id, .. }
            | Operation::Reparent { id, .. }
            | Operation::SetVisible { id, .. }
            | Operation::SetVisibleVar { id, .. }
            | Operation::SetMask { id, .. } => touched.push(*id),
            Operation::SetDisplay { id, .. } => {
                if let Some(n) = doc.get(*id) {
                    touched.extend(n.children().iter().copied());
                }
            }
            Operation::InsertSubtree { nodes, .. } => {
                for n in nodes {
                    if !sets_vars.contains(&n.id) && n.size_vars.iter().any(Option::is_some) {
                        arrived.insert(n.id, n.size_vars);
                    }
                }
            }
            _ => {}
        }
    }
    touched.retain(|id| bound(*id));
    touched.extend(arrived.keys().copied());
    touched.sort();
    touched.dedup();
    let mut extra: Vec<Operation> = Vec::new();
    if !touched.is_empty() {
        let mut after = doc.clone();
        // Unchecked: a link the commit's later passes cut fails `component::check`
        // until they run.
        if after.apply_unchecked(&tx).is_ok() {
            let view = crate::resolve::DocView(&after);
            for id in touched {
                // Gone by the end of the edit: a write to it would fail the commit.
                let Some(kept) = after.get(id) else {
                    continue;
                };
                let before = doc.get(id).map(|n| n.size_vars());
                let Some(start) = before.or_else(|| arrived.get(&id).copied()) else {
                    continue;
                };
                let authored = size_axes_authored(&view, id);
                let v = vars.entry(id).or_insert(start);
                let which = [
                    v[0].is_some() && !authored[0],
                    v[1].is_some() && !authored[1],
                ];
                if !which.iter().any(|w| *w) {
                    continue;
                }
                // What it drew before the edit, or, brought in, what it draws
                // where it came from — its variables' numbers.
                let drawn = match before {
                    Some(_) => doc.resolved_kind(id),
                    None => after.resolved_kind(id),
                }
                .and_then(|k| k.box_size())
                .unwrap_or_default();
                let Some(mut s) = kept.kind().box_size() else {
                    continue;
                };
                if which[0] {
                    s.width = drawn.width;
                    v[0] = None;
                }
                if which[1] {
                    s.height = drawn.height;
                    v[1] = None;
                }
                extra.push(Operation::SetGeometry {
                    id,
                    geometry: crate::GeometryPatch::Size(s),
                });
            }
        }
    }
    tx.0.extend(extra);
    let mut ids: Vec<NodeId> = vars.keys().copied().collect();
    ids.sort();
    for id in ids {
        let v = vars[&id];
        let had = doc
            .get(id)
            .map(|n| n.size_vars())
            .or_else(|| arrived.get(&id).copied());
        if had.is_some_and(|had| had != v) {
            tx.0.push(Operation::SetSizeVars { id, vars: v });
        }
    }
    tx
}

/// Bind text `id`'s content and family to `vars` — `None` leaving a field as it
/// is (§15 D1069), [`bind_size_ops`]' shape: a bound field's literal is written
/// empty and never drawn while bound.
///
/// Binding the content clears the text's spans and paragraph spans (a byte range
/// cannot index a string whose length changes with the mode); binding the family
/// clears runs with a family of their own. A family is not bound under a text
/// style, which holds a plain family (§15 D1067). Empty for a layer that is no
/// text, or when nothing would change.
pub fn bind_text_ops(doc: &Document, id: NodeId, vars: crate::TextVars) -> Vec<Operation> {
    let Some(node) = doc.get(id) else {
        return Vec::new();
    };
    let NodeKind::Text {
        content,
        style,
        spans,
        para_spans,
        ..
    } = node.kind()
    else {
        return Vec::new();
    };
    let mut next = node.text_vars();
    if vars[0].is_some() {
        next[0] = vars[0];
    }
    if vars[1].is_some() && node.styles().text.is_none() {
        next[1] = vars[1];
    }
    if next == node.text_vars() {
        return Vec::new();
    }
    let mut ops = Vec::new();
    let mut style2 = (**style).clone();
    if next[1].is_some() {
        style2.font_family.clear();
    }
    let mut spans2 = spans.clone();
    if next[0].is_some() {
        spans2 = Default::default();
    } else if next[1].is_some() {
        spans2.clear(
            0..content.len(),
            crate::typography::CharAttrKind::Family,
            &style2,
        );
    }
    if style2 != **style || spans2 != *spans {
        ops.push(Operation::SetTextStyle {
            id,
            style: style2,
            spans: Some(spans2.clone()),
        });
    }
    if next[0].is_some() && (!content.is_empty() || !spans2.is_empty() || !para_spans.is_empty()) {
        ops.push(Operation::SetText {
            id,
            content: String::new(),
            spans: Default::default(),
            para_spans: Default::default(),
        });
    }
    ops.push(Operation::SetTextVars { id, vars: next });
    ops
}

/// Detach the fields of text `id` that `which` names to the strings they are
/// drawn with (D1038's detach, §15 D1069): each literal becomes its variable's
/// string and the binding goes, the other field as it is. Empty when neither is
/// bound.
pub fn detach_text_ops(doc: &Document, id: NodeId, which: [bool; 2]) -> Vec<Operation> {
    detach_text_ops_in(doc, doc.variables(), id, which)
}

/// [`detach_text_ops`] against table `t`, the one a variable's delete detaches
/// against (`crate::variable::remove_ops`).
pub fn detach_text_ops_in(
    doc: &Document,
    t: &crate::Variables,
    id: NodeId,
    which: [bool; 2],
) -> Vec<Operation> {
    let Some(node) = doc.get(id) else {
        return Vec::new();
    };
    let mut vars = node.text_vars();
    if !(0..2).any(|k| which[k] && vars[k].is_some()) {
        return Vec::new();
    }
    let NodeKind::Text {
        style,
        spans,
        para_spans,
        ..
    } = node.kind()
    else {
        return Vec::new();
    };
    let NodeKind::Text {
        content: drawn,
        style: drawn_style,
        ..
    } = doc.text_drawn_in(t, id, node.kind(), vars)
    else {
        return Vec::new();
    };
    let mut ops = Vec::new();
    if which[1] && vars[1].is_some() {
        let mut s = (**style).clone();
        s.font_family = drawn_style.font_family;
        ops.push(Operation::SetTextStyle {
            id,
            style: s,
            spans: Some(spans.clone()),
        });
        vars[1] = None;
    }
    if which[0] && vars[0].is_some() {
        ops.push(Operation::SetText {
            id,
            content: drawn,
            spans: spans.clone(),
            para_spans: para_spans.clone(),
        });
        vars[0] = None;
    }
    ops.push(Operation::SetTextVars { id, vars });
    ops
}

/// An edit of a bound text detaches what it edits (D1038 (2), §15 D1069): the
/// commit-time half of the binding, so every writer of the text ops is unchanged.
///
/// - The content. A `SetText` writing the drawn string with no spans and no
///   paragraph spans stays bound, its literal put back empty (an unedited text
///   session writes exactly that). One half, not the corners' two (D1059's
///   reason): the empty literal is what a text emptied on purpose writes, and
///   excusing it brought the variable's string back on release. Anything else
///   detaches: typing changes the string, and styling a range writes spans that
///   index the drawn string, so a `SetTextSpans` or `SetParagraphSpans` that
///   leaves any is rewritten into a `SetText` of the drawn string carrying them.
/// - The family. A `SetTextStyle` whose family is the drawn one or the empty
///   literal stays bound (every Type card row but the family writes the whole
///   style with the family as stored); any other detaches. A run with its own
///   family is a span, and the default stays bound under it (§15 D1067). Applying
///   a text style detaches it: the style holds a plain family, which
///   `crate::style::settle` then writes.
///
/// A transaction that sets a text's bindings itself is left alone.
pub fn keep_text_bindings(doc: &Document, mut tx: Transaction) -> Transaction {
    let sets_var: FxHashSet<NodeId> =
        tx.0.iter()
            .filter_map(|op| match op {
                Operation::SetTextVars { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
    let mut vars: FxHashMap<NodeId, crate::TextVars> = FxHashMap::default();
    let bound = |id: NodeId| -> Option<(&Node, String, String)> {
        if sets_var.contains(&id) {
            return None;
        }
        let node = doc.get(id)?;
        if node.text_vars().iter().all(Option::is_none) {
            return None;
        }
        match doc.resolved_kind(id)?.into_owned() {
            NodeKind::Text { content, style, .. } => Some((node, content, style.font_family)),
            _ => None,
        }
    };
    for op in tx.0.iter_mut() {
        match op {
            Operation::SetText {
                id,
                content,
                spans,
                para_spans,
            } => {
                let Some((node, drawn, _)) = bound(*id) else {
                    continue;
                };
                if node.text_vars()[0].is_none() {
                    continue;
                }
                let plain = spans.is_empty() && para_spans.is_empty();
                if plain && *content == drawn {
                    content.clear();
                } else {
                    vars.entry(*id).or_insert(node.text_vars())[0] = None;
                }
            }
            Operation::SetTextSpans { id, spans } if !spans.is_empty() => {
                let id = *id;
                let Some((node, drawn, _)) = bound(id) else {
                    continue;
                };
                if node.text_vars()[0].is_none() {
                    continue;
                }
                let NodeKind::Text { para_spans, .. } = node.kind() else {
                    continue;
                };
                let spans = std::mem::take(spans);
                *op = Operation::SetText {
                    id,
                    content: drawn,
                    spans,
                    para_spans: para_spans.clone(),
                };
                vars.entry(id).or_insert(node.text_vars())[0] = None;
            }
            Operation::SetParagraphSpans { id, spans } if !spans.is_empty() => {
                let id = *id;
                let Some((node, drawn, _)) = bound(id) else {
                    continue;
                };
                if node.text_vars()[0].is_none() {
                    continue;
                }
                let NodeKind::Text { spans: chars, .. } = node.kind() else {
                    continue;
                };
                let para_spans = std::mem::take(spans);
                *op = Operation::SetText {
                    id,
                    content: drawn,
                    spans: chars.clone(),
                    para_spans,
                };
                vars.entry(id).or_insert(node.text_vars())[0] = None;
            }
            Operation::SetTextStyle { id, style, .. } => {
                let Some((node, _, family)) = bound(*id) else {
                    continue;
                };
                if node.text_vars()[1].is_none() {
                    continue;
                }
                if style.font_family.is_empty() || style.font_family == family {
                    style.font_family.clear();
                } else {
                    vars.entry(*id).or_insert(node.text_vars())[1] = None;
                }
            }
            Operation::SetTypeStyle { id, style: Some(_) } => {
                let Some((node, _, _)) = bound(*id) else {
                    continue;
                };
                if node.text_vars()[1].is_some() {
                    vars.entry(*id).or_insert(node.text_vars())[1] = None;
                }
            }
            _ => {}
        }
    }
    let mut ids: Vec<NodeId> = vars.keys().copied().collect();
    ids.sort();
    for id in ids {
        let v = vars[&id];
        if doc.get(id).is_some_and(|n| n.text_vars() != v) {
            tx.0.push(Operation::SetTextVars { id, vars: v });
        }
    }
    tx
}

/// [`edit_brushes_all`] for the one item at `index` of each target's list — a row
/// of the shared list, written in place so every target keeps its own item ids
/// (§15 D980).
pub fn edit_brush_at_all(
    doc: &Document,
    ids: &[NodeId],
    target: PaintTarget,
    index: usize,
    edit: impl Fn(&Brush) -> Brush,
) -> Transaction {
    let mut ops = Vec::new();
    for id in paint_targets(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        match target {
            PaintTarget::Fill => {
                let mut fills = node.paint().fills.clone();
                let Some(f) = fills.get_mut(index) else {
                    continue;
                };
                f.brush = edited_brush(doc, id, &f.brush, &edit);
                if fills != node.paint().fills {
                    ops.push(Operation::SetFills { id, fills });
                }
            }
            PaintTarget::Stroke => {
                let mut strokes = node.paint().strokes.clone();
                let Some(s) = strokes.get_mut(index) else {
                    continue;
                };
                s.brush = edited_brush(doc, id, &s.brush, &edit);
                if strokes != node.paint().strokes {
                    ops.push(Operation::SetStrokes { id, strokes });
                }
            }
        }
    }
    Transaction(ops)
}

/// Change every stroke of every paint target in scope, in place.
/// [`edit_fills_all`]'s twin, over the same targets — frames included, see
/// [`shared_strokes`].
pub fn edit_strokes_all(doc: &Document, ids: &[NodeId], edit: impl Fn(&mut Stroke)) -> Transaction {
    edit_strokes_at_all(doc, ids, |_, s| edit(s))
}

/// [`edit_strokes_all`] with the layer each stroke is on — for an edit that
/// reads the layer, as one resolving a bound width in the layer's mode does
/// (the several-layer weight row sparing a binding that already draws the
/// width typed, §15 D1073's fourth amendment).
pub fn edit_strokes_at_all(
    doc: &Document,
    ids: &[NodeId],
    edit: impl Fn(NodeId, &mut Stroke),
) -> Transaction {
    let mut ops = Vec::new();
    for id in paint_targets(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        let mut strokes = node.paint().strokes.clone();
        for stroke in &mut strokes {
            edit(id, stroke);
        }
        if strokes != node.paint().strokes {
            ops.push(Operation::SetStrokes { id, strokes });
        }
    }
    Transaction(ops)
}

/// Give every paint target in scope exactly this fill list.
///
/// Exactly this list, on a frame too (§15 D400). Values in, each target's own ids
/// kept by position (§15 D980, `item::rekey_by_position`): a replacement is not an
/// edit, so there is no anchor to say which item became which; an edit of a shared
/// list goes through [`retarget_fills_all`]. A fill past a target's end takes one
/// id for the whole write, shared by every target that reaches it (§15 D1003 (6)).
pub fn set_fills_all(
    doc: &Document,
    ids: &[NodeId],
    fills: &[Fill],
    src: &mut IdSource,
) -> Transaction {
    let mut ops = Vec::new();
    let mut fresh = crate::item::PastTheEnd::default();
    for id in paint_targets(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        let own = &node.paint().fills;
        if own.len() != fills.len() || own.iter().zip(fills).any(|(o, f)| o.value != *f) {
            ops.push(Operation::SetFills {
                id,
                fills: crate::item::rekey_by_position(own, fills.iter().cloned(), &mut fresh, src),
            });
        }
    }
    Transaction(ops)
}

/// Give every paint target in scope exactly this stroke list — [`set_fills_all`]'s
/// twin, over the same targets, frames included.
pub fn set_strokes_all(
    doc: &Document,
    ids: &[NodeId],
    strokes: &[Stroke],
    src: &mut IdSource,
) -> Transaction {
    let mut ops = Vec::new();
    let mut fresh = crate::item::PastTheEnd::default();
    for id in paint_targets(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        let own = &node.paint().strokes;
        if own.len() != strokes.len() || own.iter().zip(strokes).any(|(o, s)| o.value != *s) {
            ops.push(Operation::SetStrokes {
                id,
                strokes: crate::item::rekey_by_position(
                    own,
                    strokes.iter().cloned(),
                    &mut fresh,
                    src,
                ),
            });
        }
    }
    Transaction(ops)
}

/// Change fill `index` of every paint target in scope, in place — one row of a
/// list the targets share (their lists agree in value, so row `index` is the
/// same fill on each). Every id stays where it is; a target with no fill there
/// is left alone.
pub fn edit_fill_at_all(
    doc: &Document,
    ids: &[NodeId],
    index: usize,
    edit: impl Fn(&mut Fill),
) -> Transaction {
    let mut ops = Vec::new();
    for id in paint_targets(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        let mut fills = node.paint().fills.clone();
        let Some(fill) = fills.get_mut(index) else {
            continue;
        };
        edit(fill);
        if fills != node.paint().fills {
            ops.push(Operation::SetFills { id, fills });
        }
    }
    Transaction(ops)
}

/// [`edit_fill_at_all`]'s twin over strokes.
pub fn edit_stroke_at_all(
    doc: &Document,
    ids: &[NodeId],
    index: usize,
    edit: impl Fn(&mut Stroke),
) -> Transaction {
    let mut ops = Vec::new();
    for id in paint_targets(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        let mut strokes = node.paint().strokes.clone();
        let Some(stroke) = strokes.get_mut(index) else {
            continue;
        };
        edit(stroke);
        if strokes != node.paint().strokes {
            ops.push(Operation::SetStrokes { id, strokes });
        }
    }
    Transaction(ops)
}

/// An **edit** of the fill list every paint target in scope shares, written onto
/// each target with its own ids (§15 D980, `item::retarget`).
///
/// `anchor` is the list the panel showed ([`shared_fills`]' answer) and `edited` is
/// that list after the change. Writing `edited` verbatim would give every target
/// the anchor's ids, cutting an instance's fill from its counterpart in the main;
/// retargeting maps each edited item through the anchor to the target's own item,
/// and gives an added row one id on every target (§15 D1003 (6)).
pub fn retarget_fills_all(
    doc: &Document,
    ids: &[NodeId],
    anchor: &[Keyed<Fill>],
    edited: &[Keyed<Fill>],
    src: &mut IdSource,
) -> Transaction {
    let mut ops = Vec::new();
    for id in paint_targets(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        let own = &node.paint().fills;
        let fills = crate::item::retarget(anchor, own, edited, src);
        if fills != *own {
            ops.push(Operation::SetFills { id, fills });
        }
    }
    Transaction(ops)
}

/// [`retarget_fills_all`]'s twin over strokes.
pub fn retarget_strokes_all(
    doc: &Document,
    ids: &[NodeId],
    anchor: &[Keyed<Stroke>],
    edited: &[Keyed<Stroke>],
    src: &mut IdSource,
) -> Transaction {
    let mut ops = Vec::new();
    for id in paint_targets(doc, ids) {
        let Some(node) = doc.get(id) else { continue };
        let own = &node.paint().strokes;
        let strokes = crate::item::retarget(anchor, own, edited, src);
        if strokes != *own {
            ops.push(Operation::SetStrokes { id, strokes });
        }
    }
    Transaction(ops)
}

/// One layer's appearance, lifted off it so another layer can be given the same
/// one: the payload behind *Copy properties* / *Paste properties*
/// (`docs/context-menus.md` §3, `docs/shortcuts.md` §7).
///
/// Position, size, rotation, name, pivot, lock and geometry are excluded: the verb
/// is "make this look like that", and a corner radius means nothing carried from a
/// rect to a star. A live text node's per-run colours are not here either:
/// character spans override the node's paint, so pasting onto text whose runs carry
/// their own colours changes nothing visible.
#[derive(Clone, Debug, PartialEq)]
pub struct Properties {
    /// The layer's own fill list, a frame's included, whole (§15 D400).
    pub fills: Vec<Fill>,
    pub strokes: Vec<Stroke>,
    /// The opacity the layer draws, a bound one resolved: the detach a paste falls
    /// back to (§15 D1072).
    pub opacity: f32,
    /// The variable it is bound to, landed by the paste's rule as a paint's is.
    pub opacity_var: Option<crate::VarId>,
    /// What a bound paint needs to land in another document (§15 D1047): the name
    /// and kind of every variable the fills and strokes bind, and each paint as it
    /// drew in the copying document. The clipboard outlives a document switch, so a
    /// paste cannot ask the copying document.
    pub carried: Vec<crate::variable::CarriedVar>,
    pub drawn_fills: Vec<Brush>,
    pub drawn_strokes: Vec<Brush>,
    /// Each stroke's width as it drew at the copy: the detach a paste falls back to
    /// (§15 D1073).
    pub drawn_widths: Vec<f64>,
}

/// A layer's appearance, or `None` for a kind that has none to give.
///
/// A group is refused rather than answered with an empty list, which would arrive
/// at the paste as an instruction to clear whatever it landed on; one field
/// (opacity) is not an appearance.
pub fn properties_of(doc: &Document, id: NodeId) -> Option<Properties> {
    let node = doc.get(id)?;
    if !node.kind().takes_paint() {
        return None;
    }
    let fills = crate::item::values(&node.paint().fills);
    let strokes = crate::item::values(&node.paint().strokes);
    let t = doc.variables();
    let mut carried: Vec<crate::variable::CarriedVar> = Vec::new();
    for v in fills
        .iter()
        .map(|f| &f.brush)
        .chain(strokes.iter().map(|s| &s.brush))
        .filter_map(crate::BrushSource::var)
        .chain(strokes.iter().filter_map(|s| s.width_var))
        .chain(node.opacity_var())
    {
        if let Some(var) = t.get(v)
            && !carried.iter().any(|c| c.id == v)
        {
            carried.push(crate::variable::CarriedVar {
                id: v,
                name: var.name.clone(),
                kind: var.kind,
                origin: var.origin.clone(),
            });
        }
    }
    Some(Properties {
        drawn_fills: fills
            .iter()
            .map(|f| doc.resolve_brush(id, &f.brush).into_owned())
            .collect(),
        drawn_strokes: strokes
            .iter()
            .map(|s| doc.resolve_brush(id, &s.brush).into_owned())
            .collect(),
        drawn_widths: strokes
            .iter()
            .map(|s| match s.width_var {
                Some(v) => doc.stroke_width_drawn_in(t, id, v),
                None => s.width,
            })
            .collect(),
        fills,
        strokes,
        opacity: doc.drawn_opacity(id),
        opacity_var: node.opacity_var(),
        carried,
    })
}

/// Give every target in scope the appearance in `props`.
///
/// Three existing bulk verbs composed: fills where a panel fill goes
/// ([`set_fills_all`], through a group, stopping at a frame), strokes likewise
/// ([`set_strokes_all`]), and opacity to the outermost members alone
/// ([`set_opacity_all`], since opacity composes down the tree). Each drops the
/// targets that already agree, so pasting a layer's own properties is empty.
///
/// A bound paint lands by a paste's rule (D1038, §15 D1047): kept where this
/// document has the variable, else rebound to this document's variable of the
/// same name and kind ([`crate::variable::rebind`], as [`crate::variable::land`]
/// does), else detached to what it drew at the copy.
pub fn paste_properties(
    doc: &Document,
    ids: &[NodeId],
    props: &Properties,
    src: &mut IdSource,
) -> Transaction {
    let t = doc.variables();
    let landed = |b: &crate::BrushSource, drawn: Option<&Brush>| -> crate::BrushSource {
        match b {
            crate::BrushSource::Bound { var, alpha } => {
                match crate::variable::rebind(t, &props.carried, *var) {
                    Some(var) => crate::BrushSource::Bound { var, alpha: *alpha },
                    None => drawn
                        .cloned()
                        .unwrap_or(Brush::Solid(peniko::Color::TRANSPARENT))
                        .into(),
                }
            }
            literal => literal.clone(),
        }
    };
    let fills: Vec<Fill> = props
        .fills
        .iter()
        .enumerate()
        .map(|(i, f)| Fill {
            brush: landed(&f.brush, props.drawn_fills.get(i)),
            ..f.clone()
        })
        .collect();
    let strokes: Vec<Stroke> = props
        .strokes
        .iter()
        .enumerate()
        .map(|(i, s)| {
            // A bound width by the same rule: rebound where this document has
            // the variable, else the width it drew.
            let (width, width_var) = match s.width_var {
                Some(v) => match crate::variable::rebind(t, &props.carried, v) {
                    Some(v) => (s.width, Some(v)),
                    None => (props.drawn_widths.get(i).copied().unwrap_or(s.width), None),
                },
                None => (s.width, None),
            };
            Stroke {
                brush: landed(&s.brush, props.drawn_strokes.get(i)),
                width,
                width_var,
                ..s.clone()
            }
        })
        .collect();
    let mut ops = set_fills_all(doc, ids, &fills, src).0;
    ops.extend(set_strokes_all(doc, ids, &strokes, src).0);
    // A bound opacity by the same rule: rebound on the outermost members, else
    // the opacity it drew.
    match props
        .opacity_var
        .and_then(|v| crate::variable::rebind(t, &props.carried, v))
    {
        Some(var) => {
            for id in outermost(doc, ids) {
                ops.extend(bind_opacity_ops(doc, id, var));
            }
        }
        None => ops.extend(set_opacity_all(doc, ids, props.opacity).0),
    }
    Transaction(ops)
}

/// Set the opacity of every **outermost** member of `ids`.
///
/// Outermost, not the whole subtree: opacity composes down the tree, so 50% on a
/// group and on each child would land at 25%.
pub fn set_opacity_all(doc: &Document, ids: &[NodeId], opacity: f32) -> Transaction {
    Transaction(
        outermost(doc, ids)
            .into_iter()
            // Against what each draws (§15 D1072): a bound layer holds 0, so the
            // literal would mishandle a 0% typed over one.
            .filter(|id| doc.get(*id).is_some() && doc.drawn_opacity(*id) != opacity)
            .map(|id| Operation::SetOpacity { id, opacity })
            .collect(),
    )
}

/// How near two opacities have to be for a readout to call them one value.
///
/// The rule lives here, and the panel reads it (§15 D607). The field formats as
/// `{v:.0}%`, so anything this side of a part in a million is the same number on
/// screen and reporting *Mixed* for it would be the readout disagreeing with itself.
pub const OPACITY_AGREEMENT: f32 = 1e-6;

/// [`OPACITY_AGREEMENT`]'s twin for a corner radius (§15 D607). Tighter because a
/// radius is a length in document units: 1e-9 is below anything a drag or parser
/// can author, where 1e-6 of an opacity is a rounding artefact the percentage
/// field produces routinely.
pub const RADIUS_AGREEMENT: f64 = 1e-9;

/// Set the same radius on all four corners of every rect in the subtrees.
///
/// Subtree rather than outermost, unlike opacity: a radius does not compose, so
/// reaching into a selected group and rounding the rects inside it is both safe
/// and what "round these corners" means when a group is what is selected.
pub fn set_corner_radius_all(doc: &Document, ids: &[NodeId], radius: f64) -> Transaction {
    // Rects already at this radius contribute nothing, so an unedited field that
    // loses focus pushes no undo step.
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
            // [`RADIUS_AGREEMENT`], not `==`.
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

/// Whether any node in scope draws fills and strokes of its own, so a Fill or
/// Stroke panel has something to write to (empty groups have none).
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
/// `target` is the caller's business (a UI policy): the selection's union, or the
/// container's box for one layer.
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

/// The outermost members of `ids` that have world bounds, sorted by where they
/// already sit along `axis`, so arranging a set never reorders it: whatever is
/// leftmost stays leftmost. [`distribute`] and [`distribute_spacing`] share it;
/// [`gaps_along`] sorts the same way over boxes it is handed.
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
/// Equal gaps, not equal centres. Needs three layers; fewer returns an empty
/// transaction.
///
/// Equal gaps only while the layers fit in their own span (§15 D487): once they
/// overlap more, equal gaps and left-to-right order are incompatible, and the
/// order is kept by spacing the leading edges evenly instead.
///
/// No key layer here, unlike [`distribute_spacing`]: the extremes are set by where
/// layers already sit, whereas a fixed gap leaves the run free to slide.
pub fn distribute(doc: &Document, res: &Resolved, ids: &[NodeId], axis: Axis) -> Transaction {
    let boxes = sorted_along(doc, res, ids, axis);
    if boxes.len() < 3 {
        return Transaction(Vec::new());
    }
    let first_lead = lead(&boxes[0].1, axis);
    // The greatest trailing edge, not the last box's: a background rect 800 wide
    // at x=0 with two 40-wide icons at 10 and 100 would give a span of 140.
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

    // The layers overlap more than the span allows (§15 D487). With overlap the
    // extremes, equal gaps and the order cannot all hold; the order is the promise
    // to keep (equal gaps would swap two icons), so the leading edges are spaced
    // evenly, which also leaves the first and last where they were.
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
/// One fewer number than there are boxes, empty below two. A gap is negative where
/// the two overlap, which [`distribute_spacing`] writes back for a negative number.
///
/// Returned as the whole list: how close two gaps must be to read as one number is
/// the panel's display question. Over boxes rather than ids because core only has
/// the committed tree and a field showing this would sit still through its own
/// drag (§15 D245); `session::preview_world_bounds` is what the panel feeds it.
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
/// This is what a key layer is for (§15 D243): naming the gap frees the run to
/// slide, so one member, `anchor`, holds still while earlier ones pack backwards
/// from it and later ones forwards. An absent or non-member `anchor` falls back to
/// the first (leftmost or topmost) layer.
///
/// Two layers is enough, unlike `distribute`: one gap is worth typing. Nothing is
/// clamped; a negative `gap` overlaps the layers.
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
    // one; the anchor gets no operation at all.
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
/// `Axis::X` is the horizontal flip: left and right swap, so the mirror line is
/// vertical. One layer flips in place; several flip about their union, as a group.
/// This writes a reflection into the transform, which §5.6 permits.
///
/// A single layer whose pivot has been moved mirrors about the pivot instead
/// ([`crate::Pivot`]). Only when actually placed: an unset pivot resolves to the
/// box centre, which differs from the union centre below for a rotated group.
pub fn flip(doc: &Document, res: &Resolved, ids: &[NodeId], axis: Axis) -> Transaction {
    let members = outermost(doc, ids);
    // A set has no pivot to share; this is a single-selection question.
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
/// Reading a rotation back as `atan2(b, a)` is wrong once a node is mirrored (a
/// reflection has a negative determinant) or skewed, and a rebuild from angle alone
/// drops both.
///
/// The two mirrors are the same reflection 180° apart (`R(θ)·FLIP_Y` is
/// `R(θ+π)·FLIP_X`), so the decomposition reports whichever leaves the smaller
/// angle: flip an upright layer vertically and the rotation field stays at 0°.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Orientation {
    /// Rotation in radians, in `(-π, π]`.
    pub angle: f64,
    /// Skew in radians, in `(-π/2, π/2)`: how far the y axis leans away from
    /// square with the x axis. SVG/CSS `skewX` semantics (coefficient `tan(skew)`),
    /// the convention the SVG writer emits.
    pub skew: f64,
    pub mirror: Mirror,
}

/// A basis split into the part a transform may carry and the part it may not.
///
/// §5.6 keeps scale out of the transform so a layer's size is one number in its
/// geometry; skew has no geometry field, so it lives with rotation and flip.
///
/// An RQ decomposition: every invertible 2×2 factors uniquely as
/// `R(θ)·skew·mirror · diag(sx, sy)`, the scale on the right where the node's own
/// axes can absorb it. Exact for any matrix, so a world-space edit can be pushed
/// onto a node whatever its parents have done (§15 D50).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Basis {
    pub orientation: Orientation,
    /// The residual scale in the node's own axes — always positive, since the
    /// sign of a reflection is carried by [`Orientation::mirror`] instead.
    pub scale: Vec2,
}

/// Below this a basis has no extent to measure an angle or lean against.
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
/// Not a second case in the decomposition: a y-lean comes back out as a rotation
/// plus an x-lean plus a scale. It exists because a gesture on a vertical edge
/// means this one.
pub fn skew_y(radians: f64) -> Affine {
    Affine::new([1.0, radians.tan(), 0.0, 1.0, 0.0, 0.0])
}

impl Basis {
    /// Split a transform's basis, ignoring its translation.
    ///
    /// Gram–Schmidt: the first column gives the rotation and x scale, and the
    /// second is read in that frame (across the x axis is the y scale, along it
    /// is the lean). [`Orientation::to_affine`] times [`Self::scale`] returns the
    /// input.
    pub fn of(transform: Affine) -> Self {
        let [a, b, c, d, _, _] = transform.as_coeffs();
        // A reflection is peeled off first, by negating the x column.
        let mirrored = a * d - b * c < 0.0;
        let (a, b) = if mirrored { (-a, -b) } else { (a, b) };

        let sx = a.hypot(b);
        if sx < DEGENERATE {
            // Report the identity rather than a NaN so a collapsed node still
            // round-trips and the inspector shows 0°.
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
        // Square to the x axis: this component is the height, along `u` is lean.
        let (vx, vy) = (-uy, ux);
        let sy = c * vx + d * vy;
        let skew = if sy.abs() < DEGENERATE {
            0.0
        } else {
            ((c * ux + d * uy) / sy).atan()
        };
        let angle = wrap_pi(uy.atan2(ux));

        // With the reflection peeled off, `sy` is positive and needs no sign.
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
    /// `of` is total and lossy; this is partial and exact. A rank-1 basis (such as
    /// `[1, 2, 2, 4]`) has no exact split, and `of` returns a plausible wrong
    /// matrix for it. Both of `of`'s degenerate arms are refused, including
    /// `Affine::scale(1e-13)`, which falls under `DEGENERATE` and would give
    /// `sx = 0` with an identity orientation (zeroing stroke widths under
    /// `Scaling::Photographic`).
    ///
    /// The thresholds are not restated: this reads the `scale` numbers `of`
    /// computed so the guard cannot drift from the arms it guards. [`affine_is_finite`]
    /// is spelled anyway rather than relying on `NaN` failing both comparisons.
    ///
    /// Anything reporting a basis rather than rewriting one (the inspector's angle
    /// and scale fields) still uses [`Self::of`], which gives 0° for a collapsed node.
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
/// A selection can span several parents; each is restacked independently ("to the
/// front of wherever you are"). Members that are descendants of other members are
/// dropped.
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
        // One step trades places with the nearest unselected neighbour; walking
        // from the far end keeps a run of selected layers together.
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
/// Each op's index is the slot after the node is lifted out of its old one, so
/// every op is computed against the order the previous ops leave behind.
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
/// no-ops so an align that changes nothing is an empty transaction.
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

/// `tx` with each resized flex item's properties set so the size it was resized
/// to holds (§15 D875, "set width and stop growth").
///
/// A resize changing an in-flow item's main-axis size also writes `flex-grow: 0`,
/// `flex-shrink: 0` and `flex-basis: auto` (§15 D935); one changing the cross size
/// of an item drawn stretched ([`stretched_across`]) writes `align-self: start`, or
/// `end` when it held the cross end edge (§15 D905). A kind the layout never sizes
/// holds nothing, and a layer whose item properties the transaction sets itself is
/// left alone. A size on a `%` or `fit-content` axis is set back to `auto` (§15
/// D879).
///
/// The item half is [`flex_holds`], which the gesture preview applies too (§15
/// D904); this adds [`kept_flow_translations`] (§15 D877), which the preview must
/// not, and which can empty a transaction. Run beside [`keep_insets`].
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
/// Split out so the gesture preview (`RenderOverrides`' `flex_relayout`) applies
/// them (§15 D904); otherwise a stretched item dragged shorter is stretched back
/// every frame. The preview must not take [`kept_flow_translations`] too: that
/// drops a move's translation, and a move is what the preview draws.
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
        // Never a vault node, which a kit update writes exactly as published.
        if !geometry.resizes() || explicit.contains(id) || doc.in_vault(*id) {
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
            // Where the box was and where the tool wants it, in the parent's
            // space (a left- or top-handle resize shifts the origin).
            let was = res.used_local(doc, *id).unwrap_or(Affine::IDENTITY);
            let to = written_transform(tx, *id).unwrap_or(was);
            let edges = resized_edges((was, now), (to, then));
            item = match &*display {
                // Only a kind whose drawn size follows the layout is held (§15
                // D935): a path is not stretched, so there is no size to take back.
                Display::Flex(_) if !container::can_stretch(used) => item,
                Display::Flex(flex) => {
                    let from_start = resized_from_cross_start(flex, edges);
                    let stretched = stretched_across(flex, &view.item(*id), used, false);
                    held(flex, item, now.size(), then.size(), from_start, stretched)
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

/// Whether a resize of an item laid out by `flex` kept its cross-axis end edge and
/// moved its start (a row's item dragged by its top; reversed under `wrap-reverse`,
/// §15 D905), read off `edges`, [`resized_edges`]' physical answer, through the flow.
///
/// This decides between `align-self: flex-start` and `flex-end` when a resize
/// releases a stretch, so the item does not jump to the other side of its line.
/// `wrap-reverse` swaps cross-start and cross-end (§15 D909). Every other resize
/// answers `false`, including about the centre.
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

/// For each physical axis (across, then down), whether a resize moved the box's
/// low edge and held its high one (left or top dragged), and whether it did the
/// reverse. `was` and `now` are the box in its own space with the placing local
/// transform. About the centre both answers are `false`.
///
/// Flex reads one axis through its flow ([`resized_from_cross_start`]); grid reads
/// both, physically, having no reversal (§15 D913).
fn resized_edges(was: (Affine, kurbo::Rect), now: (Affine, kurbo::Rect)) -> [(bool, bool); 2] {
    let a = was.0.transform_rect_bbox(was.1);
    let b = now.0.transform_rect_bbox(now.1);
    // "Held" means to within rounding: the anchor goes through a product of transforms.
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
/// to `to` changed, in a container laid out by `grid` (§15 D913), [`held`]'s for
/// grid: a stretch that would undo the resize becomes `start`, or `end` when the
/// resize moved the axis's low edge and held its high one (`low_moved`, across
/// then down). The item keeps its cells.
///
/// "Stretched" is the resolved alignment (the item's own, else the container's);
/// under `normal` a box stretches and a replaced item does not (§15 D915).
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

/// `tx` with every `SetTransform` on an in-flow flex item keeping the item's
/// stored translation: its new rotation, skew and flip taken, its position not
/// (§15 D877). A grid item too, since [`is_flex_item`] answers for any layout (§15
/// D914).
///
/// Tools write where they want a layer drawn, and for an in-flow item that is not
/// where it is drawn: a left- or top-handle resize or a pivot rotation composes on
/// the item's used transform (its slot), so the write would land the item at the
/// slot's translation once the layout is taken away. Every such write keeps the
/// stored translation here, once, rather than in every tool (see
/// `a_tool_write_on_a_mains_flex_item_follows_onto_its_copy_exactly`).
///
/// Not when the same transaction takes the item out of the flow (a reparent,
/// delete, insets, hiding, making it a mask, or its parent's layout changing):
/// then the transform is where it will be drawn, and it stands.
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
    // A write left changing nothing is dropped rather than kept as a no-op step.
    let ops =
        tx.0.into_iter()
            .filter_map(|op| match op {
                // Never a vault node: a kit update's translation is the kit's.
                Operation::SetTransform { id, transform }
                    if is_flex_item(doc, id)
                        && !doc.in_vault(id)
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
/// whose stored size is its CSS size, so `auto` is the size the resize wrote.
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

/// Whether a flex item with `item` and `kind` is drawn stretched across its line:
/// the question [`held`] asks before releasing a stretch (§15 D935).
///
/// The keyword is not enough: CSS stretches only an item whose cross size is
/// `auto`, and a frame's `auto` is its stored size, definite and never stretched
/// (D879). A kind with no size to stretch ([`crate::container::can_stretch`]) is
/// never stretched, and a group is only when it has a layout of its own.
fn stretched_across(
    flex: &crate::container::Flex,
    item: &crate::container::LayoutItem,
    kind: &NodeKind,
    laid: bool,
) -> bool {
    use crate::container::{AlignItems, Dimension};
    let across = if flex.direction.is_row() {
        item.height
    } else {
        item.width
    };
    item.align_self.unwrap_or(flex.align_items) == AlignItems::Stretch
        && across == Dimension::Auto
        && match kind {
            NodeKind::Artboard { .. } => false,
            NodeKind::Group => laid,
            k => crate::container::can_stretch(k),
        }
}

/// `item` with growth stopped on whichever axes a resize from `from` to `to`
/// changed, in a container laid out by `flex` — [`keep_flex_sizes`]' rule (§15
/// D875), spelled once for it and for [`sized_flex_item`]. A released stretch
/// aligns to the end when `from_start` (the resize moved the cross axis's start
/// edge and held its end, [`resized_from_cross_start`]) and to the start
/// otherwise; `stretched` is whether there was a stretch to release
/// ([`stretched_across`], asked before the resize).
///
/// A main-axis resize clears `flex-basis` too (§15 D935): the basis decides the
/// main size before growth does. A px `min-` or `max-` is kept and clamps the
/// resize, a limit somebody set that the preview shows (§15 D904).
fn held(
    flex: &crate::container::Flex,
    mut item: crate::container::LayoutItem,
    from: kurbo::Size,
    to: kurbo::Size,
    from_start: bool,
    stretched: bool,
) -> crate::container::LayoutItem {
    use crate::container::{AlignItems, Dimension};
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
        item.basis = Dimension::Auto;
    }
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
/// A group with a layout has only its box, `width`/`height` on its
/// [`crate::container::LayoutItem`], `auto` until set. A resize writes them in px,
/// each only if changed (§15 D879), and stops its growth here because
/// [`keep_flex_sizes`] leaves a transaction's own `SetLayoutItem` alone.
///
/// `to` is the local transform the same edit writes for the group, if any; it
/// tells a released stretch which edge to keep (§15 D905).
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
    // Only the axes the resize changed (§15 D879), so a side handle on a group
    // that hugs its height keeps it hugging. With no box to compare against, both.
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
        item = match &*display {
            Display::Flex(flex) => held(
                flex,
                item,
                now,
                size,
                resized_from_cross_start(flex, edges),
                stretched_across(flex, node.item(), node.kind(), true),
            ),
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
/// A drag inside a flex container is a reorder, not a move: an in-flow item is
/// placed by its container (§15 D875), so a dropped translation is undone by the
/// next layout pass. The dragged box's centre is read against every other in-flow
/// sibling's used box in reading order, by laid line then along it
/// ([`flow_index`], §15 D934). `row-reverse` and `column-reverse` read the main
/// axis backwards; `wrap-reverse` needs nothing of its own (§15 D883).
///
/// Siblings out of the flow (pinned, hidden, masks) keep their indices relative to
/// the flow; the result is an index into the child list with `id` taken out, which
/// is what `Reorder` means. `None` when `id` is not an in-flow flex item, or would
/// land where it is.
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
    // Where it lands in the flow, not in the child list, decides whether it moved
    // (§15 D934); otherwise a pinned sibling after the item makes a nudge reorder.
    let view = crate::resolve::DocView(doc);
    let was = others
        .iter()
        .take(doc.get(parent)?.children().iter().position(|c| *c == id)?)
        .filter(|c| crate::container::in_flow(&view, **c))
        .count();
    (place != was).then_some(Operation::Reorder { id, index })
}

/// [`flex_reorder`] for several items of one flex container dragged together
/// (§15 D902): they move as a block to where the block's centre falls, keeping
/// their order. `None` unless every one of `ids` is an in-flow item of the same
/// container; otherwise the `Reorder`s that turn the child list into the new one,
/// empty when the block lands where it is.
///
/// The block's centre is the centre of the union of their drawn boxes, read against
/// the other in-flow siblings by [`flow_index`]. The ops are derived by walking the
/// target order and moving whatever is out of place into it; placing each dragged
/// item at its final index in turn is wrong (`[a, d1, b, d2, c]` with `d1, d2`
/// dropped after `c` comes out `[a, b, d1, c, d2]` only by luck of order).
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
    // The flow order unchanged is no reorder (§15 D934), whatever the child list
    // would say: an out-of-flow sibling between members would be pulled out.
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
/// reading-order rule, shared with [`flex_reorder_many`]. Answers the child-list
/// index and the place in the flow (in-flow siblings before it), which says
/// whether anything moved.
///
/// The lines are the laid lines, read off the flow (§15 D934): a new line starts
/// where an in-flow sibling's main-axis start runs back behind the end of the one
/// before it, and a line's cross extent is the union of its items'. A `nowrap` row
/// is one line, and `wrap-reverse` needs no arm (§15 D883). The dragged centre's
/// line is the one whose band holds it, else the nearest; it lands after every
/// earlier line and among that line's items by main-axis centre. Asking each
/// sibling's own cross extent instead misreads items of different heights as
/// different lines.
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
    // Each in-flow sibling's index in `others`, main extent and centre, and cross
    // extent, in flow order.
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
    // The line whose band holds the centre, else the nearest.
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

/// The committed document's grid container `id` as its pass lays it: its tracks
/// and items' areas ([`crate::container::laid_grid`], §15 D916). The public door,
/// for tests; the canvas reads the preview's (`RenderOverrides::laid_grid`), and a
/// drop lays through the committed geometry ([`drop_grid`]). Laid through the
/// stored document, so a grid pinned in a parent drawn at another size is laid at
/// the stored size.
pub fn laid_grid(doc: &Document, id: NodeId) -> Option<crate::container::LaidGrid> {
    crate::container::laid_grid(&crate::resolve::DocView(doc), id)
}

/// The grid container `id` as a drop into it is aimed against: laid through the
/// committed geometry, so a pinned grid is laid at the width it is drawn at (§15
/// D942). [`grid_drop_many`]'s grid and the one the canvas draws under a moved item
/// (§15 D946), so the cell drawn under the pointer is the cell the drop writes.
pub fn drop_grid(doc: &Document, res: &Resolved, id: NodeId) -> Option<crate::container::LaidGrid> {
    crate::container::laid_grid(&crate::resolve::CommittedView { doc, res }, id)
}

/// The `SetLayoutItem` that dragging the grid item `id` by the world-space
/// `delta` commits: explicit lines for the cell its centre is dropped on (§15
/// D913: every drop writes lines, so an auto-placed item becomes explicitly placed).
///
/// The drag moves the item's area by as many tracks as its centre crossed on each
/// axis, read against the laid tracks ([`crate::container::laid_grid`]), so an item
/// spanning two columns keeps both. Both axes are written. The span keeps the
/// author's spelling: an end given as a line moves with the start, a span stays a
/// span, and a one-track area is `start / auto`. A drop lands its whole area on
/// tracks the grid already has (§15 D927); an area in a leading implicit track
/// keeps the negative line that names it ([`grid_drop_many`]).
///
/// `None` when the centre stays in its tracks on both axes (no undo step, as for
/// flex), or `id` is not an in-flow item of a grid.
pub fn grid_drop(doc: &Document, res: &Resolved, id: NodeId, delta: Vec2) -> Option<Operation> {
    grid_drop_many(doc, res, &[id], delta)?.into_iter().next()
}

/// [`grid_drop`] for several items of one grid, dragged as a block (§15 D918):
/// every item's area moves by the same number of tracks, read off the centre of
/// the block's union box, so the arrangement is kept and every item is written
/// explicit lines. A block pushed towards the first track stops there whole (shift
/// clamped to the leftmost or topmost item's room); towards the last track the same
/// way (§15 D927), so a two-wide item dropped on the last column lands on the last
/// two instead of hanging into an implicit track that `fr` sizes to nothing.
///
/// `Some` of no operations when the centre stays in its tracks; `None` when the
/// items are not all in-flow items of one grid. A block of one is [`grid_drop`].
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
    // Through the committed geometry, so a pinned grid is laid at the width it is
    // drawn at, the grid the commit lays and the canvas draws.
    let grid = drop_grid(doc, res, parent)?;
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
    // Stopped whole at either end of the laid tracks, so the block cannot
    // reshuffle against an edge nor hang past one.
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
    // A line before the explicit grid's first (a leading implicit track) has no
    // positive number, so it is written as the negative one: line `p` is
    // `p − (explicit + 2)` from the far end. Clamping to 1 would move an item on
    // an axis the drag had not touched.
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

/// Whether `id` is an item in its container's flow: its parent lays it out and it
/// is not pinned, hidden or a mask. Public for the app's drag, which has to know a
/// reorder from a move.
///
/// True of a grid item too (§15 D914); the name is flex's because flex came first.
/// A grid's drop writes lines, and [`flex_reorder`] answers nothing there.
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

/// The box a group with a layout is laid out to in `doc`: its own layout pass, run
/// from its chain root so a group that is itself an item gets the size its
/// container gives it (§15 D900). `keep_insets`' measure of a laid group after an
/// edit, where `Resolved::used_frame` answers for the document before it.
///
/// Its own pass when the chain root's does not reach it (§15 D910): a laid group
/// inside a plain group that is a flex item, or one hidden or made a mask, is a
/// layout root of its own ([`crate::container::is_layout_root`]).
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
/// The one place an edit to a pinned layer becomes an edit of its insets. Tools
/// compute in used geometry (where a layer is drawn, §15 D868) and write ordinary
/// `SetTransform` and `SetGeometry`; for a pinned layer the insets decide where it
/// is drawn, so each such write is read as "draw it here" and the `SetInsets` that
/// does is appended, each inset in its own unit (`container::inverse`). The tool's
/// ops stay, so the stored transform and size become the placement too. Every other
/// layer passes through, so this is a no-op on a document with no insets.
///
/// Run where commits enter, once, rather than at every site that writes
/// placements. Previews do not need it.
///
/// What the layer is placed against is judged after the transaction: a `Reparent`
/// into a frame pins it against the new frame, and an `InsertSubtree` copy is
/// re-pinned where the edit placed it. The frame's size is its used size when the
/// edit leaves it alone, else its size after the edit (the Scale tool's case).
///
/// A layer whose insets the transaction sets itself is left alone.
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
            // Only a patch that changes the box is a resize (`GeometryPatch::resizes`);
            // a corner radius would re-pin a stretched layer at its stored size.
            Operation::SetGeometry { id, geometry } if geometry.resizes() => {
                placed.entry(*id).or_insert((None, false)).1 = true;
            }
            Operation::SetInsets { id, .. } => {
                explicit.insert(*id);
            }
            _ => {}
        }
    }
    // The cheap refusal first: nothing placed carries an inset, so a document
    // with no insets never pays for the scratch copy below.
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
    // The document as the edit leaves it; a transaction the model refuses is
    // returned as it came, for the commit to refuse. Sizes are read as
    // `resolved_kind` of a document that has had the size-binding pass
    // (`keep_size_bindings` runs later) and the text-binding pass
    // (`keep_text_bindings`: a text's size is its string's).
    let mut after = doc.clone();
    // Unchecked, as `keep_size_bindings`' scratch is.
    let kept = keep_text_bindings(doc, keep_size_bindings(doc, Some(res), tx.clone()));
    if after.apply_unchecked(&kept).is_err() {
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
        // Never a vault node: the vault is never resolved, so `used_local` has
        // nothing for it and the stale stored transform would re-pin a pinned
        // layer from the wrong place. A kit update writes a vault node as published.
        if explicit.contains(&id) || after.in_vault(id) {
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
            after
                .resolved_kind(parent.id())
                .map_or_else(|| parent.kind().clone(), std::borrow::Cow::into_owned)
        } else {
            res.used_kind(doc, parent.id())
                .unwrap_or(parent.kind())
                .clone()
        };
        // A group with a layout pins too (§15 D871, D887), against its laid box
        // (what `resolve::frame_box` places the child against). That box is the
        // one the edit leaves, not the committed one (§15 D900): the layout is
        // run over `after` from the group's chain root, or the group itself where
        // that pass does not reach it (`laid_group_box`). The Scale tool resizes a
        // laid group and moves a pinned child in one edit. A frame's size is in
        // its kind, `parent_kind` above.
        let frame = match parent_kind {
            NodeKind::Artboard { size } => size,
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
            after
                .resolved_kind(id)
                .map_or_else(|| node.kind().clone(), std::borrow::Cow::into_owned)
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
            // Written back to where it is drawn: a handle pressed and released
            // without moving writes the used placement, which for a pinned layer
            // is not its stored transform, and the no-op test would see a change.
            unmoved.insert(id);
        }
    }
    if !unmoved.is_empty() {
        out.0
            .retain(|op| !matches!(op, Operation::SetTransform { id, .. } if unmoved.contains(id)));
    }
    out
}
