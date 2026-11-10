//! The headless editing arithmetic (§9.4): resize, rotate, skew and scale, the
//! create builders, and the pen's anchors with every point edit on them. Each
//! answer is a `Transaction` or a value to build one from, taking only the
//! `Document`, `Resolved` and kurbo types, so the canvas's gestures and the verb
//! table call the same functions. Gesture state stays in the app.
//!
//! World-space projections that more than one caller needs (move, reparent, group)
//! live in [`crate::build`].

mod types;

pub use types::{Handle, LineEnd, PenAnchor, PenSubpath, PointRef, Side};

use crate::Brush;
use crate::build::{self, Axis, Basis, Edge};
use crate::kurbo::{
    Affine, BezPath, CubicBez, ParamCurve, PathEl, Point, Rect, RoundedRectRadii, Size, Vec2,
};
use crate::peniko::Color;
use crate::{
    Document, Fill, GeometryPatch, ImageEntry, ImageId, NodeId, NodeKind, Operation, Resolved,
    Stroke, TextSizing, Transaction, geometry, keyed_by_position,
};

/// What a handle drag changes — the whole difference between Select and Scale.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Scaling {
    /// The layer's geometry changes and nothing else: a 1px border, a 4px radius
    /// and 16pt type stay as they are (§5.6).
    #[default]
    Geometry,
    /// Everything scales with the box: geometry, stroke widths, corner radii and font size.
    Photographic,
}

impl Scaling {
    /// The single factor the scalars take, given the two the box was scaled by.
    ///
    /// The geometric mean `sqrt(|sx·sy|)`, exactly `s` under a uniform scale.
    /// Absolute, so a mirror never negates a width.
    fn scalar(self, csx: f64, csy: f64) -> Option<f64> {
        if self != Scaling::Photographic {
            return None;
        }
        let s = (csx.abs() * csy.abs()).sqrt();
        (s.is_finite() && (s - 1.0).abs() > SCALE_NOOP).then_some(s)
    }
}

/// Sides a fresh polygon is drawn with — a triangle, the smallest shape.
pub const POLYGON_DEFAULT_SIDES: u32 = 3;
/// Points a fresh star is drawn with, and its inner radius: five points, inner ring at
/// the golden-section radius where the spokes' edges line up straight through the middle.
pub const STAR_DEFAULT_POINTS: u32 = 5;
pub const STAR_DEFAULT_RATIO: f64 = 0.382;
/// Half turns a fresh spiral is drawn with (three and a half turns), and the most `↑`
/// reaches.
pub const SPIRAL_DEFAULT_HALF_TURNS: u32 = 7;
/// The most half turns `↑` gives a spiral being drawn.
pub const SPIRAL_MAX_HALF_TURNS: u32 = 40;
/// A drawn arc's sweep moves in steps of this many degrees up to one short of a
/// whole turn, either way round; below the first step it takes [`ARC_LEAST_DEG`]
/// and then crosses to the other side, so one drag reaches both bulges.
pub const ARC_STEP_DEG: u32 = 15;
/// The shallowest a drawn arc bulges, either way.
const ARC_LEAST_DEG: f64 = 5.0;
/// Rungs of the arc's sweep ladder ([`arc_sweep`]) on each side.
pub const ARC_SIDE_RUNGS: u32 = 360 / ARC_STEP_DEG;
/// The highest rung: one step short of a whole turn clockwise.
pub const ARC_MAX_STEPS: u32 = 2 * ARC_SIDE_RUNGS - 1;
/// A fresh arc is a quarter turn clockwise.
pub const ARC_DEFAULT_STEPS: u32 = ARC_SIDE_RUNGS + 90 / ARC_STEP_DEG;

/// The authored local box of a node, or `None` for the kinds with no such field
/// (lines, arcs, paths, groups, booleans), which scale through [`resize_geometry`] and
/// `scale_geometry` instead of a `GeometryPatch::Size`.
pub fn resizable_size(kind: &NodeKind) -> Option<Size> {
    match kind {
        NodeKind::Rect { size, .. }
        | NodeKind::Ellipse { size, .. }
        | NodeKind::Polygon { size, .. }
        | NodeKind::Star { size, .. }
        | NodeKind::Spiral { size, .. }
        | NodeKind::Artboard { size, .. }
        | NodeKind::Slice { size } => Some(*size),
        // Auto text reports a zero box and auto-height a zero height, so handles
        // still take a width drag and the height follows the content.
        NodeKind::Text { sizing, .. } => Some(match sizing {
            TextSizing::Fixed(size) => *size,
            TextSizing::AutoHeight(w) => Size::new(*w, 0.0),
            TextSizing::Auto => Size::ZERO,
        }),
        _ => None,
    }
}

/// The box a resize gesture anchors on, in the node's own local space: the box the
/// handles were drawn on (`query::local_box`). Not [`resizable_size`], whose zero
/// extent for auto text would put the opposite edge at the origin; with no cached
/// layout it falls back to it.
///
/// The origin is not always zero: a trimmed auto text box starts at `(0, trim_top)`,
/// and [`resize_to_handle`] shifts the pointer by it.
fn anchored_box(kind: &NodeKind, text: Option<&crate::TextLayout>) -> Option<Rect> {
    let size = resizable_size(kind)?;
    let (NodeKind::Text { sizing, .. }, Some(layout)) = (kind, text) else {
        return Some(Rect::from_origin_size(Point::ZERO, size));
    };
    // On a rail the sizing modes are inert; `layout.bounds()` is what the canvas draws.
    if layout.warp.is_some() {
        return Some(layout.bounds());
    }
    let size = match sizing {
        TextSizing::Fixed(s) => *s,
        TextSizing::AutoHeight(w) => Size::new(*w, layout.size.height),
        TextSizing::Auto => layout.size,
    };
    Some(Rect::from_origin_size(layout.origin.to_point(), size))
}

/// The [`TextSizing`] a resize to `new_size` leaves a text layer in.
///
/// The mode follows whether the grabbed thing owns `y` (`authors_height`): side
/// handles and the panel's `W` field with the lock off do not. An auto-width label
/// made wider becomes `AutoHeight`, given a height it becomes `Fixed`, and a `Fixed`
/// box stays fixed.
pub fn resized_text_sizing(sizing: TextSizing, new_size: Size, authors_height: bool) -> TextSizing {
    if authors_height || sizing.height_is_authored() {
        TextSizing::Fixed(new_size)
    } else {
        TextSizing::AutoHeight(new_size.width)
    }
}

/// The sizing a text gesture starts from: the mode the node stores, at the extents
/// it is drawn at (`used`).
///
/// A flex or grid container turns stretched text into a `Fixed` box
/// (`container::flexed_text`); reading that as the mode would turn a dragged
/// auto-width label into a fixed box. A stored `AutoHeight` takes the drawn width.
pub fn gesture_text_sizing(node: &crate::Node, used: TextSizing) -> TextSizing {
    let NodeKind::Text { sizing: stored, .. } = node.kind() else {
        return used;
    };
    match (*stored, used) {
        (TextSizing::Auto, _) => TextSizing::Auto,
        (TextSizing::AutoHeight(_), TextSizing::Fixed(s)) => TextSizing::AutoHeight(s.width),
        (TextSizing::AutoHeight(w), TextSizing::Auto) => TextSizing::AutoHeight(w),
        (_, used) => used,
    }
}

/// The box a resize gesture produces, in the node's own local space, split out of
/// [`resize_to_handle`] so the anchored-axis rule is testable without a `Document`.
///
/// `size` is the box being resized and `p` the pointer in the same local space.
/// Returns `(origin, size, mirrored)`; `mirrored` is per axis, since the box lands on
/// the far side of the anchor with no sign. `symmetric` is about the box's own
/// centre, not the node's transform origin.
fn resized_box(
    size: Size,
    handle: Handle,
    p: Point,
    keep_ratio: bool,
    symmetric: bool,
) -> (Point, Size, (bool, bool)) {
    let (hold_x, hold_y) = handle.anchored(size.width, size.height);
    let centre = Point::new(size.width / 2.0, size.height / 2.0);

    // An unowned axis keeps its extent; `symmetric` doubles the distance from the centre.
    let extent = |hold: Option<f64>, at: f64, mid: f64, current: f64| match hold {
        None => current,
        Some(_) if symmetric => 2.0 * (at - mid).abs(),
        Some(held) => (at - held).abs(),
    };
    let mut w = extent(hold_x, p.x, centre.x, size.width);
    let mut h = extent(hold_y, p.y, centre.y, size.height);

    if keep_ratio && size.width > 0.0 && size.height > 0.0 {
        let ratio = size.width / size.height;
        match (hold_x.is_some(), hold_y.is_some()) {
            (true, true) => {
                if w / ratio >= h {
                    h = w / ratio;
                } else {
                    w = h * ratio;
                }
            }
            (true, false) => h = w / ratio,
            (false, true) => w = h * ratio,
            (false, false) => {}
        }
    }
    let (w, h) = (w.max(1.0), h.max(1.0));

    // Symmetric centres both axes, owned or not (`keep_ratio` can drive an unowned one);
    // anchored keeps the held edge.
    let edge = |hold: Option<f64>, at: f64, mid: f64, extent: f64| {
        if symmetric {
            return mid - extent / 2.0;
        }
        match hold {
            None => 0.0,
            Some(held) if at < held => held - extent,
            Some(held) => held,
        }
    };
    // Whether the pointer went past the anchor (the centre with Alt). Not `at < held`,
    // which is right only for a high-side handle: the box extends in the direction
    // `span - 2 * held`.
    let crossed = |hold: Option<f64>, at: f64, mid: f64, span: f64| match hold {
        None => false,
        Some(held) => {
            let anchor = if symmetric { mid } else { held };
            (at - anchor) * (span - 2.0 * held) < 0.0
        }
    };
    (
        Point::new(
            edge(hold_x, p.x, centre.x, w),
            edge(hold_y, p.y, centre.y, h),
        ),
        Size::new(w, h),
        (
            crossed(hold_x, p.x, centre.x, size.width),
            crossed(hold_y, p.y, centre.y, size.height),
        ),
    )
}

/// Resize `id` by dragging `handle` to `pointer_world`, holding the opposite
/// corner or side still.
///
/// Per §5.6 this edits *geometry*, never the transform's scale. Dragging a
/// top/left handle also translates the node, since geometry starts at the local
/// origin. `opts.keep_ratio` (Shift) locks the aspect ratio; `opts.symmetric` (Alt)
/// holds the box's centre; they compose.
pub fn resize_to_handle(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    handle: Handle,
    pointer_world: Point,
    opts: Resize,
) -> Transaction {
    let Some(node) = doc.get(id) else {
        return Transaction(vec![]);
    };
    let Some(drawn) = anchored_box(res.used_kind_of(node), res.text_layout(id)) else {
        return Transaction(vec![]);
    };
    let Some(world) = res.world_transform(id) else {
        return Transaction(vec![]);
    };
    let size = drawn.size();

    // The pointer is shifted into the drawn box's space (`resized_box` measures from
    // (0, 0)): a no-op except for a trimmed auto text node.
    let inset = drawn.origin().to_vec2();
    let (origin, new_size, mirrored) = resized_box(
        size,
        handle,
        (world.inverse() * pointer_world) - inset,
        opts.keep_ratio,
        opts.symmetric,
    );

    let geometry = match res.used_kind_of(node) {
        NodeKind::Text {
            on_path: Some(rail),
            ..
        } => railed_patch(
            res.used_kind_of(node),
            &node.text_vars(),
            rail,
            opts.scaling,
            Rect::from_origin_size(inset.to_point(), size),
            Rect::from_origin_size(inset.to_point() + origin.to_vec2(), new_size),
            handle,
        ),
        NodeKind::Text { sizing, .. } => {
            let (_, holds_y) = handle.anchored(size.width, size.height);
            GeometryPatch::TextSizing(resized_text_sizing(
                gesture_text_sizing(node, *sizing),
                new_size,
                holds_y.is_some(),
            ))
        }
        _ => GeometryPatch::Size(new_size),
    };

    let geometry_moved_itself = matches!(geometry, GeometryPatch::TextPath(_));
    let mut ops = vec![Operation::SetGeometry { id, geometry }];
    let shift = origin.to_vec2() + inset;
    // The flip reflects about the new box's own centre, composed on the right (it lives
    // in the node's new local space). Not on the rail arm: its flip is `on_path_flip`.
    let mirror = |on: bool, centre: f64| -> Affine {
        if !on {
            return Affine::IDENTITY;
        }
        Affine::translate((centre, 0.0))
            * Affine::scale_non_uniform(-1.0, 1.0)
            * Affine::translate((-centre, 0.0))
    };
    let flip = if geometry_moved_itself {
        Affine::IDENTITY
    } else {
        let swap = Affine::new([0.0, 1.0, 1.0, 0.0, 0.0, 0.0]);
        mirror(mirrored.0, inset.x + new_size.width / 2.0)
            * (swap * mirror(mirrored.1, inset.y + new_size.height / 2.0) * swap)
    };
    let moved = shift.x.abs() > f64::EPSILON || shift.y.abs() > f64::EPSILON;
    if !geometry_moved_itself && (moved || flip != Affine::IDENTITY) {
        ops.push(Operation::SetTransform {
            id,
            transform: res.used_local_of(node) * Affine::translate(shift) * flip,
        });
    }
    scale_scalars(
        doc,
        id,
        opts.scaling,
        ratio(size.width, new_size.width),
        ratio(size.height, new_size.height),
        &mut ops,
    );
    Transaction(ops)
}

/// The box `rail` would give this node, through the same layout the canvas draws.
///
/// Shared by [`railed_resize`]'s correction loop and [`scale_geometry`].
fn railed_box(kind: &NodeKind, rail: &crate::kurbo::BezPath) -> Option<Rect> {
    let content = crate::TextRef::of(kind)?.content.to_owned();
    let mut parts = crate::TextParts::of(kind)?;
    parts.on_path = Some(rail.clone());
    Some(crate::text::layout(parts.as_ref(&content)).bounds())
}

/// On a rail a resize edits the rail: the patch both resize doors write
/// ([`resize_to_handle`] and [`scale_geometry`]), from the drawn box to `want`,
/// holding `hold`'s corner. Solved against the type this transaction will leave
/// ([`kind_to_solve_against`]), `bound` the node's text bindings; `drawn` stays
/// the unscaled box.
fn railed_patch(
    kind: &NodeKind,
    bound: &crate::TextVars,
    rail: &crate::kurbo::BezPath,
    scaling: Scaling,
    drawn: Rect,
    want: Rect,
    hold: Handle,
) -> GeometryPatch {
    let solve_against = kind_to_solve_against(
        kind,
        bound,
        scaling,
        ratio(drawn.width(), want.width()),
        ratio(drawn.height(), want.height()),
    );
    GeometryPatch::TextPath(Some(railed_resize(
        solve_against.as_ref().unwrap_or(kind),
        rail,
        drawn,
        want,
        hold,
    )))
}

/// The rail a resize from `drawn` to `want` leaves a text node with, both boxes in
/// the node's own local space.
///
/// A railed node's box is the ribbon the rail sweeps, not a linear function of the
/// rail, so the factor is solved: scale, measure, correct, twice. Then the rail is
/// translated so the corner `hold` holds lands exactly where `want` puts it:
/// placement is exact, extent is close.
fn railed_resize(
    kind: &NodeKind,
    rail: &crate::kurbo::BezPath,
    drawn: Rect,
    want: Rect,
    hold: Handle,
) -> crate::kurbo::BezPath {
    let measure =
        |candidate: &crate::kurbo::BezPath| -> Option<Rect> { railed_box(kind, candidate) };
    let about = drawn.origin().to_vec2();
    let scaled = |sx: f64, sy: f64| -> crate::kurbo::BezPath {
        (Affine::translate(about) * Affine::scale_non_uniform(sx, sy) * Affine::translate(-about))
            * rail.clone()
    };

    let (mut sx, mut sy) = (
        ratio(drawn.width(), want.width()),
        ratio(drawn.height(), want.height()),
    );
    let mut out = scaled(sx, sy);
    for _ in 0..2 {
        let Some(got) = measure(&out) else { return out };
        sx *= ratio(got.width(), want.width());
        sy *= ratio(got.height(), want.height());
        out = scaled(sx, sy);
    }
    // Pin the held corner, not the origin.
    let anchor = |r: Rect| {
        let (hx, hy) = hold.anchored(r.width(), r.height());
        r.origin() + Vec2::new(hx.unwrap_or(0.0), hy.unwrap_or(0.0))
    };
    match measure(&out) {
        Some(got) => Affine::translate(anchor(want) - anchor(got)) * out,
        None => out,
    }
}

/// `after / before`, or 1 when there is no extent to have scaled.
fn ratio(before: f64, after: f64) -> f64 {
    if before.abs() < MIN_EXTENT {
        return 1.0;
    }
    after / before
}

/// Resize the single layer `id` by dragging `handle` to `pointer_world`: the routing
/// `canvas::resize_tx` applies, shared with the inspector's W/H fields:
///
/// - an authored `size` is replaced ([`resize_to_handle`]);
/// - a childless leaf or a group with a layout has its geometry scaled about the held
///   corner ([`resize_geometry`]);
/// - any other container recurses into its contents ([`resize_group`]).
pub fn resize_layer(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    handle: Handle,
    pointer_world: Point,
    opts: Resize,
) -> Transaction {
    let Some(node) = doc.get(id) else {
        return Transaction::default();
    };
    if resizable_size(node.kind()).is_some() {
        return resize_to_handle(doc, res, id, handle, pointer_world, opts);
    }
    let Some(local) = crate::local_box(doc, res, id) else {
        return Transaction::default();
    };
    // `resize_group` would scale a laid-out group's children instead of setting its box.
    let laid_group = matches!(node.kind(), NodeKind::Group) && node.display().is_some();
    if node.children().is_empty() || laid_group {
        resize_geometry(doc, res, id, local, handle, pointer_world, opts)
    } else {
        resize_group(doc, res, id, local, handle, pointer_world, opts)
    }
}

/// Whether a resize over `ids` has to keep its proportions: all-or-nothing, so one
/// locked member never silently constrains the others. Empty is not locked.
/// Pass the whole selection, not [`build::outermost`]'s reduction of it.
pub fn proportions_locked(doc: &Document, ids: &[NodeId]) -> bool {
    !ids.is_empty()
        && ids
            .iter()
            .all(|id| doc.get(*id).is_some_and(|n| n.proportions_locked()))
}

/// The size a locked box wants: the axis the caller drove keeps its number and the
/// other follows the aspect. `drove_x` cannot be inferred from the two sizes. A
/// zero-width box falls back to aspect 1; a zero aspect leaves the width alone. The
/// floor applies only to the derived value.
pub fn paired_size(now: Size, want: Size, drove_x: bool) -> Size {
    let aspect = if now.width > 0.0 {
        now.height / now.width
    } else {
        1.0
    };
    match (drove_x, aspect > 0.0) {
        (true, _) => Size::new(want.width, (want.width * aspect).max(1.0)),
        (false, true) => Size::new((want.height / aspect).max(1.0), want.height),
        (false, false) => Size::new(now.width, want.height),
    }
}

/// Make `id`'s box exactly `want`, holding its top-left corner — the inspector's W and H
/// fields for a shape whose extent is derived rather than authored.
///
/// A synthesized bottom-right handle drag through [`resize_layer`].
pub fn resize_box_to(doc: &Document, res: &Resolved, id: NodeId, want: Size) -> Transaction {
    let Some(local) = crate::local_box(doc, res, id) else {
        return Transaction::default();
    };
    let Some(world) = res.world_transform(id) else {
        return Transaction::default();
    };
    let corner = world * Point::new(local.x0 + want.width, local.y0 + want.height);
    resize_layer(
        doc,
        res,
        id,
        Handle::BottomRight,
        corner,
        // No modifier: the panel applies the proportion lock to `want` already.
        Resize::geometry(false, false),
    )
}

/// Make `id` `want` wide and high, whatever its kind: an authored size is set, a
/// text box becomes a fixed one, and any other kind is resized by its box
/// ([`resize_box_to`]).
pub fn resize_to(doc: &Document, res: &Resolved, id: NodeId, want: Size) -> Transaction {
    let Some(node) = doc.get(id) else {
        return Transaction::default();
    };
    let geometry = match node.kind() {
        NodeKind::Text { .. } => GeometryPatch::TextSizing(TextSizing::Fixed(want)),
        k if resizable_size(k).is_some() => GeometryPatch::Size(want),
        _ => return resize_box_to(doc, res, id, want),
    };
    Transaction(vec![Operation::SetGeometry { id, geometry }])
}

/// Put one end of the line `id` at the world point `target`.
///
/// A `Line` stores only its far point; the near one is the node's origin (§5.3).
/// Moving the start is a translation-only transform edit plus a re-expressed `end`
/// so the far point stays put. Read off the committed tree, so previews never compound.
pub fn move_line_end(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    which: LineEnd,
    target: Point,
) -> Transaction {
    let Some(node) = doc.get(id) else {
        return Transaction::default();
    };
    // An arc's ends are a line's: the start the origin, the end its one point.
    let (NodeKind::Line { end } | NodeKind::Arc { end, .. }) = *res.used_kind_of(node) else {
        return Transaction::default();
    };
    let Some(world) = res.world_transform(id) else {
        return Transaction::default();
    };
    match which {
        LineEnd::End => Transaction(vec![Operation::SetGeometry {
            id,
            geometry: GeometryPatch::LineEnd(world.inverse() * target),
        }]),
        LineEnd::Start => {
            let end_world = world * end;
            let parent_world = node
                .parent()
                .and_then(|p| res.world_transform(p))
                .unwrap_or(Affine::IDENTITY);
            let local = parent_world.inverse() * target;
            let [a, b, c, d, _, _] = res.used_local_of(node).as_coeffs();
            let transform = Affine::new([a, b, c, d, local.x, local.y]);
            Transaction(vec![
                Operation::SetTransform { id, transform },
                Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::LineEnd(
                        (parent_world * transform).inverse() * end_world,
                    ),
                },
            ])
        }
    }
}

/// Where the two ends of the line or arc `id` are in world space, near end first.
pub fn line_ends_world(doc: &Document, res: &Resolved, id: NodeId) -> Option<(Point, Point)> {
    let node = doc.get(id)?;
    let (NodeKind::Line { end } | NodeKind::Arc { end, .. }) = *res.used_kind_of(node) else {
        return None;
    };
    let world = res.world_transform(id)?;
    Some((world * Point::ZERO, world * end))
}

/// Resize a leaf whose geometry is its own shape — a `Line`, an `Arc` or a `Path` — by
/// dragging `handle`.
///
/// `scale_geometry` scales about the node's origin, and a path's bounding box is
/// usually elsewhere, so the node's transform takes the correction that puts the held
/// corner back; [`split_geometry_scale`] makes a mirror land as a flip, not a negative size.
pub fn resize_geometry(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    local: Rect,
    handle: Handle,
    pointer_world: Point,
    opts: Resize,
) -> Transaction {
    let Some(world) = res.world_transform(id) else {
        return Transaction(vec![]);
    };
    let Some(node) = doc.get(id) else {
        return Transaction(vec![]);
    };
    // A circle cannot stretch, and an arc's box holds its bulge: only an even scale
    // keeps the held edge still and brings the dragged one to the pointer.
    let arc = matches!(res.used_kind_of(node), NodeKind::Arc { .. });
    let (sx, sy, anchor) = resize_factors(
        local,
        handle,
        world.inverse() * pointer_world,
        opts.keep_ratio || arc,
        opts.symmetric,
    );
    // Cannot overflow here (identity basis, capped factors); a refusal is an empty edit.
    let Some(scaled) = scaled_about(Affine::IDENTITY, anchor, sx, sy) else {
        return Transaction(Vec::new());
    };
    // A basis with no exact split refuses the edit too.
    let Some((rest, csx, csy)) = split_geometry_scale(scaled) else {
        return Transaction(Vec::new());
    };
    let mut ops = Vec::new();
    let before = res.used_local_of(node);
    let transform = before * rest;
    if transform.as_coeffs() != before.as_coeffs() {
        ops.push(Operation::SetTransform { id, transform });
    }
    scale_geometry(doc, res, id, csx, csy, opts.scaling, &mut ops);
    Transaction(ops)
}

/// Resize a **container** by dragging `handle`, scaling everything inside it.
///
/// §5.6's group-resize rule: the edit recurses into geometry rather than
/// multiplying a scale into the group's transform, so stroke widths, corner
/// radii and text sizes are unaffected. Exact at every angle:
/// [`split_geometry_scale`] puts a rotated descendant's skew in its basis.
pub fn resize_group(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    local: Rect,
    handle: Handle,
    pointer_world: Point,
    opts: Resize,
) -> Transaction {
    let Some(world) = res.world_transform(id) else {
        return Transaction(vec![]);
    };
    let (sx, sy, anchor) = resize_factors(
        local,
        handle,
        world.inverse() * pointer_world,
        opts.keep_ratio,
        opts.symmetric,
    );
    let mut ops = Vec::new();
    scale_subtree(doc, res, id, anchor, sx, sy, opts.scaling, &mut ops);
    Transaction(ops)
}

/// What a handle drag was asked for, beyond where the pointer is: the two
/// modifiers and the tool's verb.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Resize {
    /// Shift, or a layer with its proportions locked: hold the box's aspect ratio.
    pub keep_ratio: bool,
    /// Alt: hold the box's own centre still rather than the opposite edge.
    pub symmetric: bool,
    /// Select's verb or Scale's.
    pub scaling: Scaling,
}

impl Resize {
    /// A §5.6 resize — geometry only, which is what every gesture but the Scale
    /// tool's asks for.
    pub fn geometry(keep_ratio: bool, symmetric: bool) -> Self {
        Self {
            keep_ratio,
            symmetric,
            scaling: Scaling::Geometry,
        }
    }
}

/// The scale factors a handle drag implies for `box_`, and the point held still
/// while they are applied, in the space `box_` and `p` are measured in.
///
/// Shared by [`resize_group`] and [`resize_selection`]. `symmetric` (Alt) holds
/// `box_`'s own centre, not the node's transform origin.
fn resize_factors(
    box_: Rect,
    handle: Handle,
    p: Point,
    keep_ratio: bool,
    symmetric: bool,
) -> (f64, f64, Point) {
    let (hold_x, hold_y) = handle.anchored(box_.width(), box_.height());
    let hold_x = hold_x.map(|x| box_.min_x() + x);
    let hold_y = hold_y.map(|y| box_.min_y() + y);

    // Factors are the pointer's distance from the anchor over the dragged corner's
    // starting distance, both signed: an unsigned divisor would mirror the selection
    // the moment a minimum-side handle was grabbed. Dragging past the anchor mirrors,
    // which `split_geometry_scale` peels into the orientation.
    let centre = box_.center();
    let (ux, uy) = handle.unit();
    // Anchored: a whole extent from the far side. Symmetric: half from the middle.
    let signed_from_anchor = |u: f64, extent: f64| match symmetric {
        true => (u - 0.5) * extent,
        false => (2.0 * u - 1.0) * extent,
    };
    let (mut sx, mut sy) = if symmetric {
        (
            hold_x.map_or(1.0, |_| {
                scale_along(signed_from_anchor(ux, box_.width()), p.x - centre.x)
            }),
            hold_y.map_or(1.0, |_| {
                scale_along(signed_from_anchor(uy, box_.height()), p.y - centre.y)
            }),
        )
    } else {
        (
            hold_x.map_or(1.0, |hx| {
                scale_along(signed_from_anchor(ux, box_.width()), p.x - hx)
            }),
            hold_y.map_or(1.0, |hy| {
                scale_along(signed_from_anchor(uy, box_.height()), p.y - hy)
            }),
        )
    };
    if keep_ratio {
        let s = match (hold_x.is_some(), hold_y.is_some()) {
            (true, true) => sx.abs().max(sy.abs()),
            (true, false) => sx.abs(),
            (false, true) => sy.abs(),
            (false, false) => 1.0,
        };
        // Each axis keeps its own sign, so a proportional drag can flip one axis only.
        sx = s.copysign(sx);
        sy = s.copysign(sy);
    }

    // `symmetric` must be asked before `hold`: `keep_ratio` can drive the unowned axis.
    let pin = |hold: Option<f64>, mid: f64, min: f64| match hold {
        _ if symmetric => mid,
        Some(held) => held,
        None => min,
    };
    let anchor = Point::new(
        pin(hold_x, centre.x, box_.min_x()),
        pin(hold_y, centre.y, box_.min_y()),
    );
    (sx, sy, anchor)
}

/// The box a whole selection is resized about: the frame it is turned by, and its
/// extent measured in that frame's own axes. [`Self::upright`] is the world-axis
/// reading the inspector's W/H fields ask for; the canvas asks for the oriented one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionBox {
    /// The transform the set is turned by — `Affine::IDENTITY` for an upright one.
    pub frame: Affine,
    /// The extent, measured in `frame`'s own axes rather than the world's.
    pub extent: Rect,
}

impl SelectionBox {
    /// A set measured on the world axes: the upright union, with an identity frame.
    pub fn upright(union: Rect) -> Self {
        Self {
            frame: Affine::IDENTITY,
            extent: union,
        }
    }
}

/// Resize a whole selection about one box drawn over all of it.
///
/// `roots` must already be [`build::outermost`], or a layer inside a selected group
/// would be scaled twice, once on its own and once by its container (§5.7a).
///
/// The factors and the anchor are computed in `sel.frame`'s space: the pointer is
/// mapped through `frame⁻¹` and each root is scaled there and mapped back, so a
/// corner lands on the pointer when the frame is turned.
pub fn resize_selection(
    doc: &Document,
    res: &Resolved,
    roots: &[NodeId],
    sel: SelectionBox,
    handle: Handle,
    pointer_world: Point,
    opts: Resize,
) -> Transaction {
    let to_frame = sel.frame.inverse();
    let (sx, sy, anchor) = resize_factors(
        sel.extent,
        handle,
        to_frame * pointer_world,
        opts.keep_ratio,
        opts.symmetric,
    );
    let mut ops = Vec::new();
    for id in roots {
        let Some(node) = doc.get(*id) else { continue };
        let Some(world) = res.world_transform(*id) else {
            continue;
        };
        let parent_world = node
            .parent()
            .and_then(|p| res.world_transform(p))
            .unwrap_or(Affine::IDENTITY);
        // Scale in the frame's space, project into the parent, and only then split;
        // splitting in world space would leave the parent's basis wrapped round the
        // result. An overflowing member is left alone and the rest still resize.
        let Some(scaled) = scaled_about(to_frame * world, anchor, sx, sy) else {
            continue;
        };
        let desired = sel.frame * scaled;
        let Some((transform, csx, csy)) = split_geometry_scale(parent_world.inverse() * desired)
        else {
            continue;
        };
        if transform.as_coeffs() != res.used_local_of(node).as_coeffs() {
            ops.push(Operation::SetTransform { id: *id, transform });
        }
        scale_geometry(doc, res, *id, csx, csy, opts.scaling, &mut ops);
    }
    Transaction(ops)
}

/// A world-space turn of `angle` about `pivot`.
///
/// One expression so the chrome and the shapes cannot disagree: a set's frame is
/// drawn with this affine mid-rotation and every member's new world transform is
/// this affine times its old one.
pub fn turn_about(pivot: Point, angle: f64) -> Affine {
    Affine::translate(pivot.to_vec2()) * Affine::rotate(angle) * Affine::translate(-pivot.to_vec2())
}

/// Rotate a whole selection by `angle` about one shared `pivot`.
///
/// Per node this is [`rotate_node`]. `roots` must be [`build::outermost`], as for
/// [`resize_selection`].
pub fn rotate_selection(
    doc: &Document,
    res: &Resolved,
    roots: &[NodeId],
    pivot: Point,
    angle: f64,
) -> Transaction {
    let mut ops = Vec::new();
    for id in roots {
        ops.extend(rotate_node(doc, res, *id, pivot, angle).0);
    }
    Transaction(ops)
}

/// `t` scaled by `(sx, sy)` about `anchor`, in whatever space `t` is measured
/// in; both the origin and the basis take the scale.
fn scaled_about(t: Affine, anchor: Point, sx: f64, sy: f64) -> Option<Affine> {
    let [a, b, c, d, e, f] = t.as_coeffs();
    let out = Affine::new([
        a * sx,
        b * sy,
        c * sx,
        d * sy,
        anchor.x + (e - anchor.x) * sx,
        anchor.y + (f - anchor.y) * sy,
    ]);
    // `None` is finite operands whose product overflows (`MAX_SCALE` caps only the
    // factor); an `inf` basis turns `NaN` in `Basis::of`.
    out.as_coeffs().iter().all(|v| v.is_finite()).then_some(out)
}

/// Split the transform a node is *intended* to have into the one it may store
/// and the scale its geometry takes instead (§5.6).
///
/// The two together reproduce `local` exactly. Rotation, skew and flip stay in the
/// transform; only scale goes to the geometry.
///
/// `None` where no split reproduces `local`: a rank-1 or near-zero basis would
/// otherwise collapse a layer or zero its stroke widths, so the caller leaves that
/// member where it was. Reachable only from outside (`op_set_transform`, `svg_in`'s
/// verbatim `transform=`), since `scale_along` floors every factor.
///
/// `local` must be the node's local transform, not its world one.
fn split_geometry_scale(local: Affine) -> Option<(Affine, f64, f64)> {
    let [_, _, _, _, e, f] = local.as_coeffs();
    let basis = Basis::exact(local)?;
    Some((
        Affine::translate((e, f)) * basis.orientation.to_affine(),
        basis.scale.x,
        basis.scale.y,
    ))
}

/// A text node's box under a container scale of `(csx, csy)`, or `None` where
/// there is no box to scale.
///
/// A scale multiplies only the extents the user authored: `Auto` has none,
/// `AutoHeight` its wrap width, `Fixed` both. The clamp keeps a zero factor from
/// authoring a zero-width wrap.
fn scaled_text_sizing(sizing: TextSizing, csx: f64, csy: f64) -> Option<TextSizing> {
    match sizing {
        TextSizing::Auto => None,
        TextSizing::AutoHeight(w) => Some(TextSizing::AutoHeight((w * csx).max(1.0))),
        TextSizing::Fixed(s) => Some(TextSizing::Fixed(Size::new(
            (s.width * csx).max(1.0),
            (s.height * csy).max(1.0),
        ))),
    }
}

/// The geometry edits that scale what `id` is by `(csx, csy)`, measured in `id`'s own
/// axes: a leaf's size, a line's end, a path's points, or a container's contents.
///
/// Text answers first, via [`scaled_text_sizing`]: the generic path would clamp an
/// auto box's zero extent into 1×1. Auto-sized text still takes the font size under
/// [`Scaling::Photographic`] through [`scale_scalars`], before the early return.
fn scale_geometry(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    csx: f64,
    csy: f64,
    scaling: Scaling,
    ops: &mut Vec<Operation>,
) {
    let Some(node) = doc.get(id) else { return };
    scale_scalars(doc, id, scaling, csx, csy, ops);
    // On a rail a resize edits the rail through this door too ([`railed_patch`]).
    if let NodeKind::Text {
        on_path: Some(rail),
        ..
    } = res.used_kind_of(node)
        && let Some(drawn) = railed_box(res.used_kind_of(node), rail)
    {
        let want = Rect::from_origin_size(
            drawn.origin(),
            Size::new(drawn.width() * csx, drawn.height() * csy),
        );
        ops.push(Operation::SetGeometry {
            id,
            geometry: railed_patch(
                res.used_kind_of(node),
                &node.text_vars(),
                rail,
                scaling,
                drawn,
                want,
                Handle::BottomRight,
            ),
        });
        return;
    }
    if let NodeKind::Text { sizing, .. } = res.used_kind_of(node) {
        if let Some(sizing) = scaled_text_sizing(gesture_text_sizing(node, *sizing), csx, csy) {
            ops.push(Operation::SetGeometry {
                id,
                geometry: GeometryPatch::TextSizing(sizing),
            });
        }
        return;
    }
    match resizable_size(res.used_kind_of(node)) {
        Some(size) => {
            let new = Size::new((size.width * csx).max(1.0), (size.height * csy).max(1.0));
            ops.push(Operation::SetGeometry {
                id,
                geometry: GeometryPatch::Size(new),
            });
        }
        // A line's or a path's gradients ride the same scale as its points, here:
        // `paint_box::keep_gradients` cannot tell a path resize from a point edit.
        None => match res.used_kind_of(node) {
            NodeKind::Line { end } => {
                ops.push(Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::LineEnd(Point::new(end.x * csx, end.y * csy)),
                });
                carry_gradients(doc, id, csx, csy, ops);
            }
            // A circle cannot stretch: the ends scale and the sweep stays, so an
            // uneven resize bends a different circle through the new ends.
            NodeKind::Arc { end, .. } => {
                ops.push(Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::LineEnd(Point::new(end.x * csx, end.y * csy)),
                });
                carry_gradients(doc, id, csx, csy, ops);
            }
            NodeKind::Path {
                path, corner_radii, ..
            } => {
                let mut scaled = path.clone();
                scaled.apply_affine(Affine::scale_non_uniform(csx, csy));
                ops.push(Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::Path {
                        path: scaled,
                        // Carried: only the Scale tool multiplies it (`scale_scalars`).
                        corner_radii: corner_radii.clone(),
                        mirror: None,
                    },
                });
                carry_gradients(doc, id, csx, csy, ops);
            }
            // A group with a layout is a box and a resize sets its `width`/`height`;
            // scaling its children would hand the layout bigger items. The Scale tool
            // takes the contents too.
            NodeKind::Group if node.display().is_some() => {
                if let Some(now) = res.used_frame(id) {
                    let size = Size::new((now.width * csx).max(1.0), (now.height * csy).max(1.0));
                    // The held edge decides a released stretch's alignment.
                    let to = ops.iter().rev().find_map(|op| match op {
                        Operation::SetTransform { id: i, transform } if *i == id => {
                            Some(*transform)
                        }
                        _ => None,
                    });
                    if let Some(item) = crate::build::sized_flex_item(doc, res, id, size, to) {
                        ops.push(Operation::SetLayoutItem { id, item });
                    }
                }
                if scaling == Scaling::Photographic {
                    scale_subtree(doc, res, id, Point::ZERO, csx, csy, scaling, ops);
                }
            }
            _ => scale_subtree(doc, res, id, Point::ZERO, csx, csy, scaling, ops),
        },
    }
}

/// `id`'s gradients carried by a scale of `csx` by `csy` about its origin, within
/// `ops` (`paint_box::carry_into`). Nothing for the identity.
fn carry_gradients(doc: &Document, id: NodeId, csx: f64, csy: f64, ops: &mut Vec<Operation>) {
    if (csx, csy) != (1.0, 1.0) && csx.is_finite() && csy.is_finite() {
        crate::paint_box::carry_into(doc, id, Affine::scale_non_uniform(csx, csy), ops);
    }
}

/// Text `kind` with its four scopes as a photographic scale of `s` leaves them;
/// `None` for anything but a text.
///
/// Every absolute length scales (`Length::scaled` multiplies `Px` and leaves `Em`).
/// The span lists are re-normalized against the scaled defaults, so a span with its
/// own indent does not rejoin the node's. A type number `bound` binds keeps the
/// value it came in with, as a bound stroke width's 0 stays 0: a bound size's
/// literal 0, scaled, would clamp to 1 and detach the binding. Split out of
/// [`scale_scalars`] so a rail can be solved against the type it will end up with.
fn scaled_text(kind: &NodeKind, s: f64, bound: &crate::TextVars) -> Option<NodeKind> {
    // Cloned and mutated so a new `NodeKind::Text` field travels through on its own.
    let mut out = kind.clone();
    let NodeKind::Text {
        style, paragraph, ..
    } = &mut out
    else {
        return None;
    };
    **style = style.scaled(s);
    *paragraph = paragraph.scaled(s);
    for (k, var) in bound.iter().enumerate().skip(crate::variable::TEXT_SIZE) {
        if var.is_some() {
            crate::variable::copy_text_slot(k, kind, &mut out);
        }
    }
    // The spans after the defaults they are normalized against are final.
    if let NodeKind::Text {
        style,
        spans,
        paragraph,
        para_spans,
        ..
    } = &mut out
    {
        *spans = spans.scaled(s, style);
        *para_spans = para_spans.scaled(s, paragraph);
    }
    Some(out)
}

/// The node kind [`railed_resize`] should measure against, given the scaling the
/// same gesture is about to apply: a photographic scale multiplies the type, and a
/// rail's box depends on the curve and the type, and a number `bound` binds does
/// not scale. `None` measures the kind unchanged.
fn kind_to_solve_against(
    kind: &NodeKind,
    bound: &crate::TextVars,
    scaling: Scaling,
    csx: f64,
    csy: f64,
) -> Option<NodeKind> {
    scaled_text(kind, scaling.scalar(csx, csy)?, bound)
}

/// The edits that make a scale photographic: stroke widths, corner radii and font
/// size by the geometric mean of the two factors ([`Scaling::scalar`]), and a
/// frame's guides by the axis factor. Nothing under [`Scaling::Geometry`] (§5.6).
///
/// A corner radius is scaled from the stored radius, since `local_path` clamps.
fn scale_scalars(
    doc: &Document,
    id: NodeId,
    scaling: Scaling,
    csx: f64,
    csy: f64,
    ops: &mut Vec<Operation>,
) {
    // Ahead of the `scalar` guard: a non-uniform scale with mean 1 must still move
    // guides and layout.
    if scaling == Scaling::Photographic {
        ops.extend(scaled_guides(doc, id, csx, csy));
        ops.extend(scaled_layout(doc, id, csx, csy));
    }

    let Some(s) = scaling.scalar(csx, csy) else {
        return;
    };
    let Some(node) = doc.get(id) else { return };

    let strokes = node.paint().strokes.clone();
    if !strokes.is_empty() {
        let strokes = strokes
            .into_iter()
            .map(|mut stroke| {
                stroke.width *= s;
                for d in &mut stroke.dashes {
                    *d *= s;
                }
                stroke.dash_offset *= s;
                // A side of 0 stays 0.
                if let crate::StrokeSides::Custom {
                    top,
                    right,
                    bottom,
                    left,
                } = &mut stroke.sides
                {
                    for w in [top, right, bottom, left] {
                        *w *= s;
                    }
                }
                stroke
            })
            .collect();
        ops.push(Operation::SetStrokes { id, strokes });
    }

    match node.kind() {
        NodeKind::Rect { corner_radii, .. } | NodeKind::Artboard { corner_radii, .. } => {
            let r = *corner_radii;
            if !crate::geometry::is_square_cornered(&r) {
                ops.push(Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::CornerRadii(RoundedRectRadii::new(
                        r.top_left * s,
                        r.top_right * s,
                        r.bottom_right * s,
                        r.bottom_left * s,
                    )),
                });
            }
        }
        NodeKind::Polygon { corner_radius, .. } | NodeKind::Star { corner_radius, .. }
            if *corner_radius > 0.0 =>
        {
            ops.push(Operation::SetGeometry {
                id,
                geometry: GeometryPatch::CornerRadius(corner_radius * s),
            });
        }
        // A path's per-anchor radii are absolute lengths like a rect's four:
        // the Scale tool multiplies them and a resize leaves them alone.
        NodeKind::Path {
            path, corner_radii, ..
        } if corner_radii.iter().any(|r| *r > 0.0) => {
            ops.push(Operation::SetGeometry {
                id,
                geometry: GeometryPatch::Path {
                    path: path.clone(),
                    corner_radii: corner_radii.iter().map(|r| r * s).collect(),
                    mirror: None,
                },
            });
        }
        // Both style ops pass `spans: None`; the scaled lists arrive in the span ops.
        NodeKind::Text { .. } => {
            let Some(NodeKind::Text {
                style,
                spans,
                para_spans,
                paragraph,
                ..
            }) = scaled_text(node.kind(), s, &node.text_vars())
            else {
                return;
            };
            ops.push(Operation::SetTextStyle {
                id,
                style: *style,
                spans: None,
            });
            ops.push(Operation::SetTextSpans { id, spans });
            ops.push(Operation::SetParagraphStyle {
                id,
                paragraph,
                spans: None,
            });
            ops.push(Operation::SetParagraphSpans {
                id,
                spans: para_spans,
            });
        }
        _ => {}
    }
}

/// A frame's own guides, multiplied by the scale — the Scale tool only.
///
/// A guide's offset is an absolute length, so a resize leaves it (kept even when it
/// outgrows a shrunk frame) and the Scale tool scales it by the axis factor, not
/// the geometric mean, so it stays on the feature it was placed against.
fn scaled_guides(doc: &Document, id: NodeId, csx: f64, csy: f64) -> Vec<Operation> {
    let mut ops: Vec<Operation> = doc
        .guides_owned_by(id)
        .filter_map(|g| {
            let factor = match g.axis {
                crate::GuideAxis::Vertical => csx,
                crate::GuideAxis::Horizontal => csy,
            };
            let position = g.position * factor;
            // An op that changes nothing is a step the user has to undo past.
            (position != g.position).then_some((g.id, position))
        })
        .map(|(id, position)| Operation::SetGuidePosition { id, position })
        .collect();
    // Deterministic order, as `build::guides_of`.
    ops.sort_by_key(|op| match op {
        Operation::SetGuidePosition { id, .. } => *id,
        _ => unreachable!("only positions are produced here"),
    });
    ops
}

/// A container's padding and gaps under the Scale tool, each by its own axis's
/// factor, and a grid's px tracks with them (columns by `csx`, rows by `csy`). A `%`
/// or `fr` track is a share and scales itself.
fn scaled_layout(doc: &Document, id: NodeId, csx: f64, csy: f64) -> Option<Operation> {
    use crate::container::{Display, Track, TrackBreadth, TrackSize};
    let was = doc.get(id)?.display()?;
    let pad =
        |[top, right, bottom, left]: [f64; 4]| [top * csy, right * csx, bottom * csy, left * csx];
    let breadth = |b: TrackBreadth, k: f64| match b {
        TrackBreadth::Px(v) => TrackBreadth::Px(v * k),
        other => other,
    };
    let size = |s: TrackSize, k: f64| match s {
        TrackSize::Breadth(b) => TrackSize::Breadth(breadth(b, k)),
        TrackSize::MinMax { min, max } => TrackSize::MinMax {
            min: breadth(min, k),
            max: breadth(max, k),
        },
    };
    let tracks = |ts: &[Track], k: f64| -> Vec<Track> {
        ts.iter()
            .map(|t| match t {
                Track::Size(s) => Track::Size(size(*s, k)),
                Track::Repeat { repeat, tracks } => Track::Repeat {
                    repeat: *repeat,
                    tracks: tracks.iter().map(|s| size(*s, k)).collect(),
                },
            })
            .collect()
    };
    let scaled = match was {
        Display::Flex(f) => {
            let mut s = *f;
            s.column_gap *= csx;
            s.row_gap *= csy;
            s.padding = pad(f.padding);
            Display::Flex(s)
        }
        Display::Grid(g) => {
            let mut s = g.clone();
            s.column_gap *= csx;
            s.row_gap *= csy;
            s.padding = pad(g.padding);
            s.columns = tracks(&g.columns, csx);
            s.rows = tracks(&g.rows, csy);
            Display::Grid(s)
        }
    };
    (&scaled != was).then_some(Operation::SetDisplay {
        id,
        display: Some(scaled),
    })
}

/// Below this a box has no extent to scale from and the factor is meaningless.
const MIN_EXTENT: f64 = 1e-6;
/// How far a scale factor has to be from 1 to be worth an operation. Not
/// `f64::EPSILON`: factors come out of a square root and land a few ulps off 1.
const SCALE_NOOP: f64 = 1e-9;
/// Cap on a single resize step. It bounds the factor, not the basis it multiplies;
/// [`scaled_about`] guards the product.
const MAX_SCALE: f64 = 1e4;

/// The factor that takes `current` to `wanted`, sign included.
///
/// The sign is the flip: clamping it to `MIN_EXTENT` would collapse the shape and
/// regrow it on the same side. The magnitude is clamped at both ends.
fn scale_along(current: f64, wanted: f64) -> f64 {
    if current.abs() < MIN_EXTENT {
        return 1.0;
    }
    let factor = wanted / current;
    factor.signum() * factor.abs().clamp(MIN_EXTENT, MAX_SCALE)
}

/// Scale everything under `id` by `(sx, sy)` about `anchor`, expressed in the
/// space `id`'s children live in. Each child's base is its used transform.
#[allow(clippy::too_many_arguments)]
fn scale_subtree(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    anchor: Point,
    sx: f64,
    sy: f64,
    scaling: Scaling,
    ops: &mut Vec<Operation>,
) {
    let Some(node) = doc.get(id) else { return };
    for child_id in node.children() {
        let Some(child) = doc.get(*child_id) else {
            continue;
        };
        let t = res.used_local_of(child);

        // Scale bodily, then split; the factors handed down are the child's own-axes
        // ones. An overflowing child is skipped, as in `resize_selection`.
        let Some(scaled) = scaled_about(t, anchor, sx, sy) else {
            continue;
        };
        let Some((transform, csx, csy)) = split_geometry_scale(scaled) else {
            continue;
        };
        if transform.as_coeffs() != t.as_coeffs() {
            ops.push(Operation::SetTransform {
                id: *child_id,
                transform,
            });
        }
        scale_geometry(doc, res, *child_id, csx, csy, scaling, ops);
        // The Scale tool scales a frame's contents as well as its box; here rather
        // than in `scale_geometry`, so a selected frame behaves the same alone or beside others.
        if scaling == Scaling::Photographic
            && matches!(res.used_kind_of(child), NodeKind::Artboard { .. })
        {
            scale_subtree(doc, res, *child_id, Point::ZERO, csx, csy, scaling, ops);
        }
    }
}

/// Rotate a single node by `angle` radians about `pivot` (world space).
///
/// The result goes through [`split_geometry_scale`]: under a skewed parent
/// `P⁻¹ · R · P` is not a rotation, and storing its scale in the transform would
/// later move the stored size. The `SCALE_NOOP` gate keeps ordinary turns from
/// adding a phantom resize to the undo stack.
pub fn rotate_node(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    pivot: Point,
    angle: f64,
) -> Transaction {
    transform_node(doc, res, id, turn_about(pivot, angle))
}

/// `id` carried by the world-space affine `by`, any scale in it written into the
/// layer's geometry as [`rotate_node`]'s is.
pub fn transform_node(doc: &Document, res: &Resolved, id: NodeId, by: Affine) -> Transaction {
    let Some(node) = doc.get(id) else {
        return Transaction(vec![]);
    };
    let Some(old_world) = res.world_transform(id) else {
        return Transaction(vec![]);
    };
    let parent_world = node
        .parent()
        .and_then(|p| res.world_transform(p))
        .unwrap_or(Affine::IDENTITY);
    let new_world = by * old_world;
    let new_local = parent_world.inverse() * new_world;
    let Some((transform, gx, gy)) = split_geometry_scale(new_local) else {
        return Transaction(vec![]);
    };
    let mut ops = Vec::new();
    if transform.as_coeffs() != res.used_local_of(node).as_coeffs() {
        ops.push(Operation::SetTransform { id, transform });
    }
    if (gx - 1.0).abs() > SCALE_NOOP || (gy - 1.0).abs() > SCALE_NOOP {
        scale_geometry(doc, res, id, gx, gy, Scaling::Geometry, &mut ops);
    }
    Transaction(ops)
}

/// How far a skew may lean: a degree short of the quarter turn at which the matrix
/// stops being invertible. Clamped rather than rejected, so a wild drag pins.
pub const MAX_SKEW: f64 = std::f64::consts::FRAC_PI_2 - 0.017_453_292_519_943_295;

/// Skew step with Shift held: 15°, the same step a rotation snaps to.
const SKEW_SNAP: f64 = std::f64::consts::FRAC_PI_2 / 6.0;

/// Where a side-drag began and where its pointer is now, in one space.
///
/// One argument rather than two adjacent `Point`s, since swapping them negates the
/// lean and compiles.
#[derive(Clone, Copy, Debug)]
pub struct SideDrag {
    /// Where the press landed — the origin the lean is measured from.
    pub press: Point,
    /// Where the pointer is now.
    pub at: Point,
}

impl SideDrag {
    /// The same drag in another space, both points through one transform.
    fn mapped(self, by: Affine) -> Self {
        Self {
            press: by * self.press,
            at: by * self.at,
        }
    }
}

/// The lean a side drag implies for `box_`, as an affine in that box's own
/// space. `None` for a corner handle and for a box too thin to lean.
///
/// Skew is a side gesture only: the dragged side follows the pointer along its
/// length while the opposite side stays put. The offset is measured from `press`,
/// not the box centre, so the first frame of an off-centre drag does not jump; the
/// shear is since the press, so a second gesture starts from zero.
pub fn side_shear(
    box_: Rect,
    handle: Handle,
    drag: SideDrag,
    snap_to_steps: bool,
) -> Option<Affine> {
    let (p, press) = (drag.at, drag.press);
    // The snap lands before the clamp so its top step is a round 75°, not 89°.
    let lean = |offset: f64, span: f64| {
        (span.abs() >= MIN_EXTENT).then(|| {
            let a = (offset / span).atan();
            let a = if snap_to_steps {
                (a / SKEW_SNAP).round() * SKEW_SNAP
            } else {
                a
            };
            a.clamp(-MAX_SKEW, MAX_SKEW)
        })
    };
    let (shear, held) = match handle {
        Handle::Top => (
            build::skew_x(lean(p.x - press.x, box_.min_y() - box_.max_y())?),
            Vec2::new(0.0, box_.max_y()),
        ),
        Handle::Bottom => (
            build::skew_x(lean(p.x - press.x, box_.max_y() - box_.min_y())?),
            Vec2::new(0.0, box_.min_y()),
        ),
        Handle::Left => (
            build::skew_y(lean(p.y - press.y, box_.min_x() - box_.max_x())?),
            Vec2::new(box_.max_x(), 0.0),
        ),
        Handle::Right => (
            build::skew_y(lean(p.y - press.y, box_.max_x() - box_.min_x())?),
            Vec2::new(box_.min_x(), 0.0),
        ),
        _ => return None,
    };
    Some(Affine::translate(held) * shear * Affine::translate(-held))
}

/// Skew a single node by dragging `handle`, in the node's own frame.
///
/// `box_` is the node's local box ([`crate::local_box`]). The shear is in the
/// node's own space, so `new_local = old_local · shear` and a container's contents
/// follow; the split afterwards keeps any extent change in the geometry.
pub fn skew_to_handle(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    box_: Rect,
    handle: Handle,
    drag: SideDrag,
    snap_to_steps: bool,
) -> Transaction {
    let Some(node) = doc.get(id) else {
        return Transaction(vec![]);
    };
    let Some(world) = res.world_transform(id) else {
        return Transaction(vec![]);
    };
    let Some(shear) = side_shear(box_, handle, drag.mapped(world.inverse()), snap_to_steps) else {
        return Transaction(vec![]);
    };
    let Some((transform, gx, gy)) = split_geometry_scale(res.used_local_of(node) * shear) else {
        return Transaction(vec![]);
    };
    let mut ops = Vec::new();
    if transform.as_coeffs() != res.used_local_of(node).as_coeffs() {
        ops.push(Operation::SetTransform { id, transform });
    }
    if (gx - 1.0).abs() > SCALE_NOOP || (gy - 1.0).abs() > SCALE_NOOP {
        scale_geometry(doc, res, id, gx, gy, Scaling::Geometry, &mut ops);
    }
    Transaction(ops)
}

/// Apply one world-space `shear` to a whole selection.
///
/// `roots` must be [`build::outermost`], as for [`resize_selection`]. The shear comes
/// from [`side_shear`] so the chrome draws the frame with the same affine.
pub fn skew_selection(
    doc: &Document,
    res: &Resolved,
    roots: &[NodeId],
    shear: Affine,
) -> Transaction {
    let mut ops = Vec::new();
    for id in roots {
        let Some(node) = doc.get(*id) else { continue };
        let Some(world) = res.world_transform(*id) else {
            continue;
        };
        let parent_world = node
            .parent()
            .and_then(|p| res.world_transform(p))
            .unwrap_or(Affine::IDENTITY);
        let Some((transform, gx, gy)) =
            split_geometry_scale(parent_world.inverse() * shear * world)
        else {
            continue;
        };
        if transform.as_coeffs() != res.used_local_of(node).as_coeffs() {
            ops.push(Operation::SetTransform { id: *id, transform });
        }
        if (gx - 1.0).abs() > SCALE_NOOP || (gy - 1.0).abs() > SCALE_NOOP {
            scale_geometry(doc, res, *id, gx, gy, Scaling::Geometry, &mut ops);
        }
    }
    Transaction(ops)
}

/// The parent-local origin + size of the axis-aligned box spanned by two world
/// points, given the parent's world transform.
fn local_box(parent_world: Affine, a: Point, b: Point) -> (Point, Size) {
    let inv = parent_world.inverse();
    let la = inv * a;
    let lb = inv * b;
    let origin = Point::new(la.x.min(lb.x), la.y.min(lb.y));
    let size = Size::new((la.x - lb.x).abs(), (la.y - lb.y).abs());
    (origin, size)
}

/// Create an artboard spanning world points `a`..`b` under `parent` — the root, or
/// the frame it was drawn inside (§5.3, frames nest).
pub fn create_artboard(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    a: Point,
    b: Point,
    background: Color,
) -> Transaction {
    let (origin, size) = local_box(parent_world, a, b);
    Transaction(vec![
        Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Artboard {
                size: Size::new(size.width.max(1.0), size.height.max(1.0)),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate(origin.to_vec2())),
            name: None,
        },
        Operation::SetFills {
            id,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(background).into(),
                visible: true,
            }]),
        },
    ])
}

/// Create a slice spanning world points `a`..`b` under `parent`, its corners
/// rounded to whole world units so a raster export is never cut on half a pixel,
/// and carrying one export spec, PNG at 1×, so *Export all* takes it at once.
pub fn create_slice(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    a: Point,
    b: Point,
) -> Transaction {
    let round = |p: Point| Point::new(p.x.round(), p.y.round());
    let (origin, size) = local_box(parent_world, round(a), round(b));
    Transaction(vec![
        Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Slice {
                size: Size::new(size.width.max(1.0), size.height.max(1.0)),
            },
            transform: Some(Affine::translate(origin.to_vec2())),
            name: None,
        },
        Operation::SetExports {
            id,
            exports: keyed_by_position([crate::ExportSpec::new(
                crate::ExportFormat::Png,
                crate::ExportScale::Times(1.0),
            )]),
        },
    ])
}

/// Create a rectangle spanning world points `a`..`b` under `parent`, with a
/// solid `fill`.
pub fn create_rect(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    a: Point,
    b: Point,
    fill: Color,
) -> Transaction {
    let (origin, size) = local_box(parent_world, a, b);
    Transaction(vec![
        Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Rect {
                size,
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate(origin.to_vec2())),
            name: None,
        },
        Operation::SetFills {
            id,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(fill).into(),
                visible: true,
            }]),
        },
    ])
}

/// Create an empty `Fixed` text node filling the world box `a`..`b` under `parent` —
/// the Text tool's drag. The height has a floor of one line box so a thin drag does
/// not commit a node whose first character overflows; the width has none.
pub fn create_text(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    a: Point,
    b: Point,
    parts: &crate::TextParts,
) -> Transaction {
    let (origin, size) = local_box(parent_world, a, b);
    // `line_height: None` means the font decides, so shape an empty string for it.
    let mut auto = parts.clone();
    auto.sizing = TextSizing::Auto;
    let line = crate::text::layout(auto.as_ref("")).line_height;
    let size = Size::new(size.width, size.height.max(line));
    Transaction(vec![Operation::CreateNode {
        id,
        parent,
        index,
        kind: NodeKind::Text {
            content: String::new(),
            style: Box::new(parts.style.clone()),
            spans: parts.spans.clone(),
            para_spans: parts.para_spans.clone(),
            paragraph: parts.paragraph.clone(),
            block: parts.block,
            sizing: TextSizing::Fixed(size),
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        },
        transform: Some(Affine::translate(origin.to_vec2())),
        name: None,
    }])
}

/// A text layer holding `content` in `parts`' style, measured: its kind, its box's
/// size, and the box's offset from the layer's origin. The text goes in as typing
/// it would (`\r\n` is one break). One line wider than `wrap` wraps at it,
/// growing downward (`AutoHeight`); otherwise the box follows the text.
pub fn text_layer_kind(
    content: &str,
    mut parts: crate::TextParts,
    wrap: Option<f64>,
) -> (NodeKind, Size, Vec2) {
    let mut editor = crate::text::TextEdit::new("", parts.clone());
    editor.insert(content);
    let mut layout = editor.text_layout();
    if let Some(width) = wrap.filter(|w| layout.size.width > *w) {
        parts.sizing = TextSizing::AutoHeight(width);
        editor.restyle(parts.clone());
        layout = editor.text_layout();
    }
    let kind = NodeKind::Text {
        content: editor.content().to_string(),
        style: Box::new(parts.style),
        spans: parts.spans,
        para_spans: parts.para_spans,
        paragraph: parts.paragraph,
        block: parts.block,
        sizing: parts.sizing,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    };
    (kind, layout.size, layout.origin)
}

/// The size to place an image at inside a box of `frame` — shrink to fit, never
/// scale up (§5.5a), preserving aspect. A degenerate box falls back to the
/// intrinsic size, since a zero-sized layer cannot be seen or fixed.
pub fn fitted_image_size(intrinsic: Size, frame: Size) -> Size {
    if frame.width <= 0.0 || frame.height <= 0.0 {
        return intrinsic;
    }
    let scale = (frame.width / intrinsic.width).min(frame.height / intrinsic.height);
    if scale >= 1.0 {
        return intrinsic;
    }
    Size::new(intrinsic.width * scale, intrinsic.height * scale)
}

/// Place `image` as a new Rect centred on the world point `at`, fitted to `frame`.
///
/// A drag goes through [`create_image_in_box`] instead. `entry` is `None` when the
/// document already carries this id, so placing a file twice adds a reference, not a
/// copy; `Some` for an id already in the table gives `OpError::DuplicateImage`.
#[allow(clippy::too_many_arguments)]
pub fn create_image(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    at: Point,
    frame: Size,
    image: ImageId,
    entry: Option<ImageEntry>,
    intrinsic: Size,
    name: Option<String>,
) -> Transaction {
    let size = fitted_image_size(intrinsic, frame);
    let local = parent_world.inverse() * at;
    let origin = Point::new(local.x - size.width / 2.0, local.y - size.height / 2.0);
    image_ops(id, parent, index, origin, size, image, entry, name)
}

/// [`create_image`] for a drag: the drawn box is obeyed, with no fit or aspect lock;
/// [`crate::ImageFit::Fill`] crops rather than squashes.
#[allow(clippy::too_many_arguments)]
pub fn create_image_in_box(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    a: Point,
    b: Point,
    image: ImageId,
    entry: Option<ImageEntry>,
    name: Option<String>,
) -> Transaction {
    let (origin, size) = local_box(parent_world, a, b);
    image_ops(id, parent, index, origin, size, image, entry, name)
}

/// The operations both placement gestures produce, given a box.
///
/// `AddImage` goes first so the inverse removes the node before the bytes it
/// refers to.
#[allow(clippy::too_many_arguments)]
fn image_ops(
    id: NodeId,
    parent: NodeId,
    index: usize,
    origin: Point,
    size: Size,
    image: ImageId,
    entry: Option<ImageEntry>,
    name: Option<String>,
) -> Transaction {
    let mut ops = Vec::with_capacity(3);
    if let Some(entry) = entry {
        ops.push(Operation::AddImage {
            id: image.clone(),
            entry,
        });
    }
    ops.push(Operation::CreateNode {
        id,
        parent,
        index,
        kind: NodeKind::Rect {
            size,
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: Some(Affine::translate(origin.to_vec2())),
        name,
    });
    ops.push(Operation::SetFills {
        id,
        fills: keyed_by_position([Fill {
            brush: crate::image_brush(image).into(),
            visible: true,
        }]),
    });
    Transaction(ops)
}

/// The frame after `handle` is dragged to `at`, bounded by the picture, in the
/// layer's local space.
///
/// `limit` is `ImageRef::picture_box`; the frame cannot reach past it, where only
/// `Extend::Pad`'s smear would show. The opposite edges are held ([`resized_box`]'s
/// rule). A handle stays on its own side rather than crossing over (a mirrored window
/// would flip the photograph), so the corner returned is the one `crop_resize_tx`
/// aims [`resize_layer`] at.
pub fn crop_resized(frame: Rect, handle: Handle, at: Point, limit: Rect) -> Rect {
    let (ux, uy) = handle.unit();
    let span = |u: f64, moved: f64, lo: f64, hi: f64, lim_lo: f64, lim_hi: f64| -> (f64, f64) {
        let (mut a, mut b) = if u == 0.0 {
            (moved, hi)
        } else if u == 1.0 {
            (lo, moved)
        } else {
            (lo, hi)
        };
        a = a.clamp(lim_lo, lim_hi);
        b = b.clamp(lim_lo, lim_hi);
        // The floor is given back to the edge that is not moving.
        if b - a < MIN_CROP {
            if u == 0.0 {
                a = b - MIN_CROP;
            } else {
                b = a + MIN_CROP;
            }
        }
        (a, b)
    };
    let (x0, x1) = span(ux, at.x, frame.x0, frame.x1, limit.x0, limit.x1);
    let (y0, y1) = span(uy, at.y, frame.y0, frame.y1, limit.y0, limit.y1);
    Rect::new(x0, y0, x1, y1)
}

/// The smallest a crop window gets: [`resized_box`]'s own floor, so [`crop_resized`]
/// matches what [`resize_layer`] writes.
const MIN_CROP: f64 = 1.0;

/// Which fill of `paint` image editing acts on: the first visible image
/// ([`crate::Paint::image_fill`], shared with `build::mask_target`).
pub fn croppable_fill(paint: &crate::Paint) -> Option<usize> {
    paint.image_fill()
}

/// Create an ellipse inscribed in the world box `a`..`b` under `parent`.
pub fn create_ellipse(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    a: Point,
    b: Point,
    fill: Color,
) -> Transaction {
    let (origin, size) = local_box(parent_world, a, b);
    Transaction(vec![
        Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Ellipse {
                size,
                arc: crate::EllipseArc::default(),
            },
            transform: Some(Affine::translate(origin.to_vec2())),
            name: None,
        },
        Operation::SetFills {
            id,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(fill).into(),
                visible: true,
            }]),
        },
    ])
}

/// One press of `↑` or `↓` on a polygon or star being drawn
/// (`docs/shortcuts.md` §9, Illustrator exact).
///
/// Clamped to the model's bounds; `↓` at the minimum would otherwise wrap a `u32`.
/// Both keys at once is a no-op.
pub fn step_sides(sides: u32, up: bool, down: bool) -> u32 {
    let delta = i64::from(up) - i64::from(down);
    (i64::from(sides) + delta).clamp(
        i64::from(geometry::MIN_SIDES),
        i64::from(geometry::MAX_SIDES),
    ) as u32
}

/// The sweep in degrees on rung `rung` of the arc's ladder: positive bulges to the
/// left of the drag, negative to its right. From the middle out either way:
/// [`ARC_LEAST_DEG`], then whole [`ARC_STEP_DEG`] steps.
pub fn arc_sweep(rung: u32) -> f64 {
    let r = rung.min(ARC_MAX_STEPS);
    let (side, k) = match r >= ARC_SIDE_RUNGS {
        true => (1.0, r - ARC_SIDE_RUNGS),
        false => (-1.0, ARC_SIDE_RUNGS - 1 - r),
    };
    let deg = match k {
        0 => ARC_LEAST_DEG,
        k => f64::from(k * ARC_STEP_DEG),
    };
    side * deg
}

/// Create a spiral dragged from `centre` to `end` in the world: `half_turns` half
/// turns clockwise out to `end`, which sets its outer radius and where it ends.
/// Its box is the true spiral's at that radius, so the stretch that fills it
/// (`geometry::local_path`) is uniform; the node turns to put the end on the drag.
#[allow(clippy::too_many_arguments)]
pub fn create_spiral(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    centre: Point,
    end: Point,
    half_turns: u32,
    stroke: Stroke,
) -> Transaction {
    let turns = f64::from(half_turns) / 2.0;
    let d = end - centre;
    let radius = d.hypot();
    let unit = crate::kurbo::Shape::bounding_box(&geometry::unit_spiral(turns));
    let size = Size::new(unit.width() * radius, unit.height() * radius);
    let transform = parent_world.inverse()
        * Affine::translate(centre.to_vec2())
        * Affine::rotate(d.y.atan2(d.x))
        * Affine::translate(unit.origin().to_vec2() * radius);
    Transaction(vec![
        Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Spiral { size, turns },
            transform: Some(transform),
            name: None,
        },
        Operation::SetStrokes {
            id,
            strokes: keyed_by_position([stroke]),
        },
    ])
}

/// Create an arc dragged from `a` to `b` in the world, bulging `sweep` degrees
/// (positive to the left of the drag), as [`create_line`] places a line: the node
/// at `a`, its end at `b`. A mirroring parent would turn the bulge over on screen,
/// so the sweep is turned with it.
#[allow(clippy::too_many_arguments)]
pub fn create_arc(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    a: Point,
    b: Point,
    sweep: f64,
    stroke: Stroke,
) -> Transaction {
    let inv = parent_world.inverse();
    let (la, lb) = (inv * a, inv * b);
    let sweep = match inv.determinant() < 0.0 {
        true => -sweep,
        false => sweep,
    };
    Transaction(vec![
        Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Arc {
                end: (lb - la).to_point(),
                sweep,
            },
            transform: Some(Affine::translate(la.to_vec2())),
            name: None,
        },
        Operation::SetStrokes {
            id,
            strokes: keyed_by_position([stroke]),
        },
    ])
}

/// Create a regular polygon inscribed in the world box `a`..`b`.
///
/// `sides` is passed in rather than taken from [`POLYGON_DEFAULT_SIDES`] because
/// `↑`/`↓` change it mid-drag (`docs/shortcuts.md` §9).
#[allow(clippy::too_many_arguments)]
pub fn create_polygon(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    a: Point,
    b: Point,
    sides: u32,
    fill: Color,
) -> Transaction {
    let (origin, size) = local_box(parent_world, a, b);
    shape_at(
        id,
        parent,
        index,
        origin,
        NodeKind::Polygon {
            size,
            sides: sides.clamp(geometry::MIN_SIDES, geometry::MAX_SIDES),
            corner_radius: 0.0,
            corner_smoothing: 0.0,
        },
        fill,
    )
}

/// Create a star inscribed in the world box `a`..`b`.
///
/// `points` is live for the same reason as [`create_polygon`]'s `sides`.
#[allow(clippy::too_many_arguments)]
pub fn create_star(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    a: Point,
    b: Point,
    points: u32,
    fill: Color,
) -> Transaction {
    let (origin, size) = local_box(parent_world, a, b);
    shape_at(
        id,
        parent,
        index,
        origin,
        NodeKind::Star {
            size,
            points: points.clamp(geometry::MIN_SIDES, geometry::MAX_SIDES),
            inner_ratio: STAR_DEFAULT_RATIO,
            corner_radius: 0.0,
            corner_smoothing: 0.0,
        },
        fill,
    )
}

/// Create a filled shape at a parent-local origin. The two-op create-then-fill
/// shape every drawn node shares.
fn shape_at(
    id: NodeId,
    parent: NodeId,
    index: usize,
    origin: Point,
    kind: NodeKind,
    fill: Color,
) -> Transaction {
    Transaction(vec![
        Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(Affine::translate(origin.to_vec2())),
            name: None,
        },
        Operation::SetFills {
            id,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(fill).into(),
                visible: true,
            }]),
        },
    ])
}

/// Create a line from world point `a` to `b` under `parent`, stroked with
/// `color` at `width`. The node origin sits at `a`; `end` is `b` in local space.
#[allow(clippy::too_many_arguments)]
pub fn create_line(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    a: Point,
    b: Point,
    color: Color,
    width: f64,
) -> Transaction {
    let inv = parent_world.inverse();
    let la = inv * a;
    let lb = inv * b;
    let end = Point::new(lb.x - la.x, lb.y - la.y);
    Transaction(vec![
        Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Line { end },
            transform: Some(Affine::translate(la.to_vec2())),
            name: None,
        },
        Operation::SetStrokes {
            id,
            strokes: keyed_by_position([Stroke {
                brush: Brush::Solid(color).into(),
                width,
                ..Default::default()
            }]),
        },
    ])
}

/// The outline a set of pen anchors describes, in **world** space.
///
/// Public because the canvas draws the in-progress path with the function it is
/// committed with.
pub fn pen_outline(anchors: &[PenAnchor], close: bool) -> BezPath {
    let mut path = BezPath::new();
    if anchors.is_empty() {
        return path;
    }
    path.move_to(anchors[0].at);
    let segment = |a: &PenAnchor, b: &PenAnchor, path: &mut BezPath| {
        if a.straight_to(b) {
            path.line_to(b.at);
        } else {
            path.curve_to(a.leaving(), b.arriving(), b.at);
        }
    };
    for pair in anchors.windows(2) {
        segment(&pair[0], &pair[1], &mut path);
    }
    if close && anchors.len() >= 2 {
        segment(&anchors[anchors.len() - 1], &anchors[0], &mut path);
        path.close_path();
    }
    path
}

/// [`pen_outline`] for every subpath in `subpaths`.
///
/// The partner of [`pen_anchors`], not its inverse: the round trip is exact only for
/// paths the pen wrote (a quad comes back as a cubic, a closed subpath with its
/// closing `LineTo` written out), which is why [`rewrite_path`]'s no-op guard
/// canonicalises both sides.
pub fn pen_path(subpaths: &[PenSubpath]) -> BezPath {
    let mut path = BezPath::new();
    for sub in subpaths {
        for el in pen_outline(&sub.anchors, sub.closed).elements() {
            path.push(*el);
        }
    }
    path
}

/// Put `subpaths` through `t` — anchors and both handles.
///
/// A handle is an offset, so it takes the linear part only; transforming the control
/// points does that without a second code path.
pub fn map_subpaths(subpaths: &mut [PenSubpath], t: Affine) {
    for sub in subpaths {
        for a in &mut sub.anchors {
            let (leaving, arriving) = (t * a.leaving(), t * a.arriving());
            a.at = t * a.at;
            a.out = leaving - a.at;
            a.back = arriving - a.at;
        }
    }
}

/// Read a `BezPath` back into the anchors that would draw it — the inverse of
/// [`pen_outline`], and what every point edit needs before it can start.
///
/// Flattened booleans, SVG imports and scaled paths can hold quads and several
/// subpaths:
///
/// - A quad is elevated to the same cubic, so it comes back as an editable anchor pair.
/// - A `ClosePath` whose last point is already the first folds: the duplicate goes
///   and its incoming handle moves onto the first anchor, or every read-and-write
///   would grow an anchor.
///
/// `out` is set only when the segment leaving an anchor is a curve, which keeps
/// [`PenAnchor::is_corner`] agreeing with what came in.
pub fn pen_anchors(path: &BezPath, radii: &[f64]) -> Vec<PenSubpath> {
    /// Below this, two points are the same point.
    const SAME: f64 = 1e-9;

    let mut out: Vec<PenSubpath> = Vec::new();
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => out.push(PenSubpath {
                anchors: vec![PenAnchor::corner(p)],
                closed: false,
            }),
            PathEl::LineTo(p) => {
                if let Some(sub) = out.last_mut() {
                    sub.anchors.push(PenAnchor::corner(p));
                }
            }
            PathEl::CurveTo(c1, c2, p) => {
                if let Some(sub) = out.last_mut()
                    && let Some(from) = sub.anchors.last_mut()
                {
                    from.out = c1 - from.at;
                    sub.anchors.push(PenAnchor {
                        at: p,
                        out: Vec2::ZERO,
                        back: c2 - p,
                        radius: 0.0,
                    });
                }
            }
            PathEl::QuadTo(c, p) => {
                if let Some(sub) = out.last_mut()
                    && let Some(from) = sub.anchors.last_mut()
                {
                    let p0 = from.at;
                    from.out = (c - p0) * (2.0 / 3.0);
                    sub.anchors.push(PenAnchor {
                        at: p,
                        out: Vec2::ZERO,
                        back: (c - p) * (2.0 / 3.0),
                        radius: 0.0,
                    });
                }
            }
            PathEl::ClosePath => {
                if let Some(sub) = out.last_mut() {
                    sub.closed = true;
                    let folds = sub.anchors.len() >= 2
                        && (sub.anchors[sub.anchors.len() - 1].at - sub.anchors[0].at).hypot()
                            < SAME;
                    if folds {
                        let dup = sub.anchors.pop().expect("checked len");
                        sub.anchors[0].back = dup.back;
                    }
                }
            }
        }
    }
    // Radii last, after the close fold changes the anchor count.
    let mut base = 0usize;
    for sub in &mut out {
        for (i, a) in sub.anchors.iter_mut().enumerate() {
            a.radius = radii.get(base + i).copied().unwrap_or(0.0).max(0.0);
        }
        base += sub.anchors.len();
    }
    out
}

/// The per-anchor radii of `subpaths`, flattened for the model.
///
/// Trailing zeros are trimmed: an unrounded path must produce an empty vec for
/// byte-stable output (invariant 9), and `rewrite_path`'s no-op comparison relies on it.
pub fn pen_radii(subpaths: &[PenSubpath]) -> Vec<f64> {
    let mut radii: Vec<f64> = subpaths
        .iter()
        .flat_map(|s| s.anchors.iter().map(|a| a.radius.max(0.0)))
        .collect();
    while radii.last().is_some_and(|r| *r <= 0.0) {
        radii.pop();
    }
    radii
}

/// Move every anchor in `points` by `delta`, handles riding along (they are offsets).
pub fn move_points(subpaths: &mut [PenSubpath], points: &[PointRef], delta: Vec2) {
    for (sub, anchor) in points {
        if let Some(a) = subpaths
            .get_mut(*sub)
            .and_then(|s| s.anchors.get_mut(*anchor))
        {
            a.at += delta;
        }
    }
}

/// Aim one anchor's handle at `to`, breaking the mirror when `break_pair`.
///
/// Within `snap` of the anchor the handle retracts onto it, which is how a smooth
/// point becomes a corner again (`is_corner` wants exact zeros); see
/// [`PenAnchor::pull_within`]. `tangent` is the tolerance for [`tangent_snap`].
pub fn pull_handle(
    subpaths: &mut [PenSubpath],
    at: PointRef,
    side: Side,
    to: Point,
    break_pair: bool,
    snap: f64,
    tangent: f64,
) {
    let Some(a) = subpaths.get(at.0).and_then(|s| s.anchors.get(at.1)) else {
        return;
    };
    let to = tangent_snap(a, side, to, tangent);
    if let Some(a) = subpaths.get_mut(at.0).and_then(|s| s.anchors.get_mut(at.1)) {
        a.pull_within(side, to, break_pair, snap);
    }
}

/// `to` pulled onto a direction worth having, if it is within `tolerance` of one.
///
/// Two kinds of direction:
///
/// - Collinear with the other handle, which makes the anchor smooth. Offered only
///   when the pair is already broken: on a mirrored anchor the candidate is the
///   direction this handle already points and a slow drag would stick, then jump.
/// - The eight axis and diagonal directions (`canvas::axial_corner`).
///
/// The tolerance is a perpendicular distance, so the target is the same size at both
/// ends of a handle. The length is kept, and a candidate the pointer is behind is
/// skipped, since projecting there would flip the handle through its anchor.
pub fn tangent_snap(a: &PenAnchor, side: Side, to: Point, tolerance: f64) -> Point {
    let v = to - a.at;
    if v.hypot() < 1e-9 || tolerance <= 0.0 {
        return to;
    }
    let other = match side {
        Side::Out => a.back,
        Side::Back => a.out,
    };
    let mut best: Option<(f64, Point)> = None;
    let smooth = (!a.is_smooth() && other.hypot() > 1e-9).then(|| -other.normalize());
    let axes = (0..8).map(|i| {
        let angle = std::f64::consts::FRAC_PI_4 * i as f64;
        Vec2::new(angle.cos(), angle.sin())
    });
    for dir in smooth.into_iter().chain(axes) {
        let along = v.dot(dir);
        if along <= 0.0 {
            continue;
        }
        let landed = a.at + dir * along;
        let off = (to - landed).hypot();
        if off <= tolerance && best.is_none_or(|(d, _)| off < d) {
            best = Some((off, landed));
        }
    }
    best.map_or(to, |(_, p)| p)
}

/// The two anchors a segment runs between — the one it leaves (`at`) and the one
/// it arrives at, which wraps to 0 on the closing segment of a closed subpath.
///
/// `None` when `at` names no segment: an out-of-range index, a subpath too short,
/// or the last anchor of an open run.
pub fn segment_ends(subpaths: &[PenSubpath], at: PointRef) -> Option<(PointRef, PointRef)> {
    let sub = subpaths.get(at.0)?;
    let n = sub.anchors.len();
    if n < 2 || at.1 >= n {
        return None;
    }
    let next = (at.1 + 1) % n;
    (next != 0 || sub.closed).then_some(((at.0, at.1), (at.0, next)))
}

/// Whether the segment leaving `at` is written out as a straight line. Not derivable
/// from [`segment_cubic`], whose cubic for a line has control points at the thirds.
pub fn segment_is_straight(subpaths: &[PenSubpath], at: PointRef) -> bool {
    let Some((a, b)) = segment_ends(subpaths, at) else {
        return false;
    };
    match (
        subpaths.get(a.0).and_then(|s| s.anchors.get(a.1)),
        subpaths.get(b.0).and_then(|s| s.anchors.get(b.1)),
    ) {
        (Some(a), Some(b)) => a.straight_to(b),
        _ => false,
    }
}

/// The segment leaving `at`, as a cubic, so a caller can ask for a position at a
/// parameter without branching on line or curve.
///
/// A straight segment comes back degree-elevated, with control points at the thirds:
/// the degenerate form's parameterisation differs from `geometry::nearest_segment`'s
/// linear `t`; elevated, `B(t)` is `lerp(t)` identically.
pub fn segment_cubic(subpaths: &[PenSubpath], at: PointRef) -> Option<CubicBez> {
    let (a, b) = segment_ends(subpaths, at)?;
    let a = subpaths.get(a.0)?.anchors.get(a.1)?;
    let b = subpaths.get(b.0)?.anchors.get(b.1)?;
    Some(if a.straight_to(b) {
        let third = (b.at - a.at) / 3.0;
        CubicBez::new(a.at, a.at + third, b.at - third, b.at)
    } else {
        CubicBez::new(a.at, a.leaving(), b.arriving(), b.at)
    })
}

/// Where parameter `t` falls on the segment leaving `at`, in `subpaths`' space.
pub fn segment_point(subpaths: &[PenSubpath], at: PointRef, t: f64) -> Option<Point> {
    Some(segment_cubic(subpaths, at)?.eval(t.clamp(0.0, 1.0)))
}

/// Split the segment leaving `at` at parameter `t`, returning the new anchor.
///
/// The curve does not change shape. The new anchor takes no radius and its
/// neighbours keep theirs, since a radius rides on the anchor.
///
/// A straight segment splits by lerp with every handle zero: subdividing the
/// elevated cubic would put control points partway along the line, growing the path
/// on every read and write.
///
/// `None` when `at` names no segment (see [`segment_ends`]).
pub fn insert_point(subpaths: &mut [PenSubpath], at: PointRef, t: f64) -> Option<PointRef> {
    let t = t.clamp(0.0, 1.0);
    let (start, end) = segment_ends(subpaths, at)?;
    let a = *subpaths.get(start.0)?.anchors.get(start.1)?;
    let b = *subpaths.get(end.0)?.anchors.get(end.1)?;
    let inserted = if a.straight_to(&b) {
        PenAnchor::corner(a.at.lerp(b.at, t))
    } else {
        let curve = CubicBez::new(a.at, a.leaving(), b.arriving(), b.at);
        let (left, right) = (curve.subsegment(0.0..t), curve.subsegment(t..1.0));
        let mid = left.p3;
        let sub = subpaths.get_mut(at.0)?;
        sub.anchors[start.1].out = left.p1 - curve.p0;
        sub.anchors[end.1].back = right.p2 - curve.p3;
        PenAnchor {
            at: mid,
            out: right.p1 - mid,
            back: left.p2 - mid,
            radius: 0.0,
        }
    };
    let sub = subpaths.get_mut(at.0)?;
    sub.anchors.insert(at.1 + 1, inserted);
    Some((at.0, at.1 + 1))
}

/// Remove every segment in `cuts`, leaving the anchors either side in place.
///
/// Unlike `remove_points`' healing, a segment removal leaves a gap: a closed subpath
/// becomes open and an open one splits. The handles at each new end are zeroed. A
/// remainder of fewer than two anchors is dropped, as in [`remove_points`]; an empty
/// result means the caller deletes the node.
pub fn remove_segments(subpaths: &[PenSubpath], cuts: &[PointRef]) -> Vec<PenSubpath> {
    /// One run between two cuts, kept only if it still draws something, with the
    /// handles that described the removed segments zeroed off its new ends.
    fn push(out: &mut Vec<PenSubpath>, anchors: &mut Vec<PenAnchor>) {
        let mut anchors = std::mem::take(anchors);
        if anchors.len() < 2 {
            return;
        }
        if let Some(first) = anchors.first_mut() {
            first.back = Vec2::ZERO;
        }
        if let Some(last) = anchors.last_mut() {
            last.out = Vec2::ZERO;
        }
        out.push(PenSubpath {
            anchors,
            closed: false,
        });
    }

    let mut out: Vec<PenSubpath> = Vec::with_capacity(subpaths.len() + 1);
    for (i, sub) in subpaths.iter().enumerate() {
        let mut cut: Vec<usize> = cuts
            .iter()
            .filter(|at| at.0 == i && segment_ends(subpaths, **at).is_some())
            .map(|at| at.1)
            .collect();
        cut.sort_unstable();
        cut.dedup();
        if cut.is_empty() {
            out.push(sub.clone());
            continue;
        }
        let n = sub.anchors.len();
        let order: Vec<usize> = match sub.closed {
            true => (cut[0] + 1..n).chain(0..=cut[0]).collect(),
            false => (0..n).collect(),
        };
        let mut run: Vec<PenAnchor> = Vec::new();
        for j in order {
            run.push(sub.anchors[j]);
            if cut.contains(&j) {
                push(&mut out, &mut run);
            }
        }
        push(&mut out, &mut run);
    }
    out
}

/// Whether a run can be split at the anchor `at`: anywhere on a closed run, and
/// between the ends of an open one, but not at a rounded corner. An open end has
/// nothing to split, and a fillet needs both legs, so an end cannot carry one.
pub fn splittable(subpaths: &[PenSubpath], (s, a): PointRef) -> bool {
    subpaths.get(s).is_some_and(|sub| {
        let n = sub.anchors.len();
        a < n && (sub.closed || (a > 0 && a + 1 < n)) && !rounded(&sub.anchors[a])
    })
}

/// Whether `a` draws a fillet: a radius on a corner or a broken anchor. One on a
/// smooth anchor draws nothing.
fn rounded(a: &PenAnchor) -> bool {
    a.radius > 0.0 && (a.is_corner() || !a.is_smooth())
}

/// Why *Split at point* cannot cut at `points`, or `None` when one of them is
/// [`splittable`].
pub fn split_refusal(subpaths: &[PenSubpath], points: &[PointRef]) -> Option<&'static str> {
    if points.iter().any(|p| splittable(subpaths, *p)) {
        return None;
    }
    let at_a_fillet = points.iter().any(|&(s, a)| {
        subpaths
            .get(s)
            .and_then(|sub| sub.anchors.get(a))
            .is_some_and(rounded)
    });
    Some(match at_a_fillet {
        true => "A rounded corner can't be split",
        false => "An open end has nothing to split",
    })
}

/// Cut each run at the anchors of `points` that [`splittable`] admits — *Split at
/// point*. A closed run opens there and an open one becomes two. The anchor is kept
/// on both sides, its arriving handle on the end that finishes a run and its leaving
/// one on the end that starts the next, so the outline draws as before.
///
/// Also answers, per anchor split, the two ends it left as `(finishing, starting)`
/// in the new numbering.
pub fn split_at_points(
    subpaths: &[PenSubpath],
    points: &[PointRef],
) -> (Vec<PenSubpath>, Vec<(PointRef, PointRef)>) {
    let mut out: Vec<PenSubpath> = Vec::with_capacity(subpaths.len() + points.len());
    let mut pairs = Vec::new();
    for (i, sub) in subpaths.iter().enumerate() {
        let mut cut: Vec<usize> = points
            .iter()
            .filter(|p| p.0 == i && splittable(subpaths, **p))
            .map(|p| p.1)
            .collect();
        cut.sort_unstable();
        cut.dedup();
        if cut.is_empty() {
            out.push(sub.clone());
            continue;
        }
        let n = sub.anchors.len();
        // A closed run is walked from its first cut round to that cut again.
        let order: Vec<usize> = match sub.closed {
            true => (cut[0]..n).chain(0..=cut[0]).collect(),
            false => (0..n).collect(),
        };
        let last = order.len() - 1;
        let mut run: Vec<PenAnchor> = Vec::new();
        // The starting end of the closed run's first cut, paired when the walk
        // comes back round to it.
        let mut opened: Option<PointRef> = None;
        for (k, &j) in order.iter().enumerate() {
            let anchor = sub.anchors[j];
            if !cut.contains(&j) {
                run.push(anchor);
                continue;
            }
            let starting = PenAnchor {
                back: Vec2::ZERO,
                radius: 0.0,
                ..anchor
            };
            if run.is_empty() {
                opened = Some((out.len(), 0));
                run.push(starting);
                continue;
            }
            run.push(PenAnchor {
                out: Vec2::ZERO,
                radius: 0.0,
                ..anchor
            });
            let finishing = (out.len(), run.len() - 1);
            out.push(PenSubpath {
                anchors: std::mem::take(&mut run),
                closed: false,
            });
            if k == last {
                pairs.extend(opened.take().map(|s| (finishing, s)));
            } else {
                pairs.push((finishing, (out.len(), 0)));
                run.push(starting);
            }
        }
        if !run.is_empty() {
            out.push(PenSubpath {
                anchors: run,
                closed: false,
            });
        }
    }
    (out, pairs)
}

/// Bend a segment: pull the point at parameter `t` by `delta`, leaving both
/// anchors where they are.
///
/// The segment's two control points take the whole pull, split so the point at `t`
/// lands under the pointer with the smallest change: with `u = 3(1−t)²t` and
/// `v = 3(1−t)t²`, the minimum-norm solution is `δ₁ = delta·u/(u²+v²)`,
/// `δ₂ = delta·v/(u²+v²)`.
///
/// A straight segment is first elevated as [`segment_cubic`] reports. The anchors'
/// other handles are left alone, so bending beside a smooth point breaks its pair.
/// Near `t = 0` or `1` the bend does nothing rather than diverging.
pub fn bend_segment(subpaths: &mut [PenSubpath], at: PointRef, t: f64, delta: Vec2) {
    let Some((a, b)) = segment_ends(subpaths, at) else {
        return;
    };
    // A bend that has not moved must not elevate a line, which would rewrite the path.
    if delta == Vec2::ZERO {
        return;
    }
    let t = t.clamp(0.0, 1.0);
    let (u, v) = (3.0 * (1.0 - t) * (1.0 - t) * t, 3.0 * (1.0 - t) * t * t);
    let denom = u * u + v * v;
    if denom < 1e-9 {
        return;
    }
    let (d1, d2) = (delta * (u / denom), delta * (v / denom));
    let curve = match segment_cubic(subpaths, at) {
        Some(c) => c,
        None => return,
    };
    // Both read before either is written: both ends can be the same anchor.
    let (from, to) = (curve.p1 + d1, curve.p2 + d2);
    let anchors = (subpaths[a.0].anchors[a.1].at, subpaths[b.0].anchors[b.1].at);
    subpaths[a.0].anchors[a.1].out = from - anchors.0;
    subpaths[b.0].anchors[b.1].back = to - anchors.1;
}

/// The upright box the selected anchors occupy (not their handles). `None` below two
/// points, and for a flat box: its corners would land on the extreme points, where
/// the point wins the press (`canvas::NodeGrab`), so the handles could not be grabbed.
pub fn points_box(subpaths: &[PenSubpath], points: &[PointRef]) -> Option<Rect> {
    if points.len() < 2 {
        return None;
    }
    points
        .iter()
        .filter_map(|(s, a)| Some(subpaths.get(*s)?.anchors.get(*a)?.at))
        .map(|p| Rect::from_points(p, p))
        .reduce(|a, b| a.union(b))
        .filter(|b| b.width() > MIN_EXTENT && b.height() > MIN_EXTENT)
}

/// A run being drawn, with `arriving` attached to its tail.
///
/// The opposite orientation to `PenEdit::working`'s: the pen appends to us, so the
/// grabbed endpoint must come first. `from_start` says the grab was at index 0,
/// which needs no reversal.
pub fn join_runs(head: &[PenAnchor], arriving: &PenSubpath, from_start: bool) -> PenSubpath {
    let mut tail = arriving.clone();
    if !from_start {
        tail.reverse();
    }
    PenSubpath {
        anchors: head.iter().copied().chain(tail.anchors).collect(),
        closed: false,
    }
}

/// The two **open ends** a point selection names, or `None` when it does not name
/// exactly two — the subject *Join* acts on (`context-menus.md` §6.5).
///
/// The menu row's gate and [`join_ends`]' precondition, spelled once so a live row
/// cannot reach a refusal. Three refusals:
///
/// - A closed run has no ends.
/// - An anchor in the middle of a run is not an end; joining there is a split first.
/// - A two-anchor run cannot be closed: the closing segment would retrace its only
///   one, a shape with no interior. Splicing it onto a different run is fine.
pub fn joinable_ends(subpaths: &[PenSubpath], points: &[PointRef]) -> Option<(PointRef, PointRef)> {
    let [a, b] = points else { return None };
    let open_end = |&(s, i): &PointRef| {
        let sub = subpaths.get(s)?;
        let last = sub.anchors.len().checked_sub(1)?;
        (!sub.closed && (i == 0 || i == last)).then_some(())
    };
    open_end(a)?;
    open_end(b)?;
    if a.0 == b.0 && subpaths.get(a.0).is_some_and(|s| s.anchors.len() < 3) {
        return None;
    }
    (a != b).then_some((*a, *b))
}

/// Fuse the two open ends [`joinable_ends`] found, and say which segment the join
/// created (Illustrator's *Join*):
///
/// - Both ends of one run: the run closes, and the new segment is the closing one,
///   which `geometry::anchor_runs` names by the last anchor.
/// - Ends of two runs: they splice into one open run at the lower subpath index.
///
/// Orientation is [`join_runs`]'s. Nothing is merged: coincident ends stay two
/// anchors with a zero-length segment, as the pen's own join leaves them
/// (`canvas::pen_join`).
pub fn join_ends(
    subpaths: &[PenSubpath],
    a: PointRef,
    b: PointRef,
) -> Option<(Vec<PenSubpath>, PointRef)> {
    let mut list = subpaths.to_vec();
    if a.0 == b.0 {
        let sub = list.get_mut(a.0)?;
        sub.closed = true;
        let last = sub.anchors.len().checked_sub(1)?;
        return Some((list, (a.0, last)));
    }
    let mut head = list.get(a.0)?.clone();
    if a.1 == 0 {
        head.reverse();
    }
    let seam = head.anchors.len().checked_sub(1)?;
    let joined = join_runs(&head.anchors, list.get(b.0)?, b.1 == 0);
    let (keep, gone) = (a.0.min(b.0), a.0.max(b.0));
    list[keep] = joined;
    list.remove(gone);
    Some((list, (keep, seam)))
}

/// Turn the selected anchors about `pivot` by `angle` radians.
///
/// Handles turn with their anchors and keep their length. As in [`map_subpaths`], the
/// `Affine` goes through the control points, which keeps a zero-length handle at zero.
pub fn rotate_points(subpaths: &mut [PenSubpath], points: &[PointRef], pivot: Point, angle: f64) {
    let t = Affine::translate(pivot.to_vec2())
        * Affine::rotate(angle)
        * Affine::translate(-pivot.to_vec2());
    for at in points {
        let Some(a) = subpaths.get_mut(at.0).and_then(|s| s.anchors.get_mut(at.1)) else {
            continue;
        };
        let (leaving, arriving) = (t * a.leaving(), t * a.arriving());
        a.at = t * a.at;
        a.out = leaving - a.at;
        a.back = arriving - a.at;
    }
}

/// Scale the selected anchors about `box_`, as a handle drag on it asks.
///
/// The arithmetic is [`resize_factors`]'. Handles scale componentwise with their
/// anchors, since they are offsets. Radii are left alone (§5.6).
pub fn resize_points(
    subpaths: &mut [PenSubpath],
    points: &[PointRef],
    box_: Rect,
    handle: Handle,
    p: Point,
    opts: Resize,
) {
    let (sx, sy, anchor) = resize_factors(box_, handle, p, opts.keep_ratio, opts.symmetric);
    for at in points {
        let Some(an) = subpaths.get_mut(at.0).and_then(|s| s.anchors.get_mut(at.1)) else {
            continue;
        };
        an.at = Point::new(
            anchor.x + (an.at.x - anchor.x) * sx,
            anchor.y + (an.at.y - anchor.y) * sy,
        );
        an.out = Vec2::new(an.out.x * sx, an.out.y * sy);
        an.back = Vec2::new(an.back.x * sx, an.back.y * sy);
    }
}

/// One coordinate of an anchor, on `axis`.
fn coord(p: Point, axis: Axis) -> f64 {
    match axis {
        Axis::X => p.x,
        Axis::Y => p.y,
    }
}

fn offset(axis: Axis, d: f64) -> Vec2 {
    match axis {
        Axis::X => Vec2::new(d, 0.0),
        Axis::Y => Vec2::new(0.0, d),
    }
}

/// The extent of `points` along `axis`, or `None` below two points.
fn span(subpaths: &[PenSubpath], points: &[PointRef], axis: Axis) -> Option<(f64, f64)> {
    let mut vals = points
        .iter()
        .filter_map(|(s, a)| subpaths.get(*s)?.anchors.get(*a))
        .map(|an| coord(an.at, axis));
    let first = vals.next()?;
    let (lo, hi) = vals.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v)));
    (points.len() >= 2).then_some((lo, hi))
}

/// Align the selected anchors along `axis`, against the box **they** occupy.
///
/// Against their own extent, the only candidate: unlike layers (`align_target`),
/// points have no container but the path, whose box is derived from them.
pub fn align_points(subpaths: &mut [PenSubpath], points: &[PointRef], axis: Axis, edge: Edge) {
    let Some((lo, hi)) = span(subpaths, points, axis) else {
        return;
    };
    let to = match edge {
        Edge::Min => lo,
        Edge::Mid => (lo + hi) * 0.5,
        Edge::Max => hi,
    };
    for at in points {
        let Some(an) = subpaths.get_mut(at.0).and_then(|s| s.anchors.get_mut(at.1)) else {
            continue;
        };
        an.at += offset(axis, to - coord(an.at, axis));
    }
}

/// Space the selected anchors evenly along `axis`.
///
/// Equal positions, where layers get equal gaps (a point has no width), so this is a
/// lerp between the extremes. Fewer than three does nothing: the end points are pinned.
pub fn distribute_points(subpaths: &mut [PenSubpath], points: &[PointRef], axis: Axis) {
    if points.len() < 3 {
        return;
    }
    let Some((lo, hi)) = span(subpaths, points, axis) else {
        return;
    };
    let order = points_along(subpaths, points, axis);
    let step = (hi - lo) / (order.len() - 1).max(1) as f64;
    for (i, (at, was)) in order.iter().enumerate() {
        let Some(an) = subpaths.get_mut(at.0).and_then(|s| s.anchors.get_mut(at.1)) else {
            continue;
        };
        an.at += offset(axis, lo + step * i as f64 - was);
    }
}

/// How long a handle invented for a smooth anchor is, as a fraction of the chord to
/// the neighbour it faces: the standard third, so it suits any path size.
const SMOOTH_HANDLE_FRACTION: f64 = 1.0 / 3.0;

/// Make the selected anchors corners or smooth points — Illustrator's *Convert to
/// corner / smooth*, and the menu rows over `context-menus.md` §6.5.
///
/// Two cases, and only the second invents anything:
///
/// - The anchor already has a handle: "smooth" means mirror, keeping the longer
///   handle, which was aimed on purpose.
/// - It has none: the tangent is the chord `next − prev`, at
///   [`SMOOTH_HANDLE_FRACTION`] of the shorter leg (a longer handle loops).
///
/// Both handles get the same length: a smooth anchor is `back == −out` exactly
/// (`is_smooth`), and an unequal collinear pair would be pulled apart by the next
/// handle drag. An endpoint of an open subpath gets one handle, since smoothness is a
/// property of a joint between two segments. The corner radius is left alone: on a
/// smooth anchor it draws nothing (`geometry::fillet`) and returns with the corner.
pub fn set_smoothness(subpaths: &mut [PenSubpath], points: &[PointRef], smooth: bool) {
    for at in points {
        let Some(sub) = subpaths.get(at.0) else {
            continue;
        };
        let Some(handles) = (if smooth {
            smoothed_handles(sub, at.1)
        } else {
            sub.anchors.get(at.1).map(|_| (Vec2::ZERO, Vec2::ZERO))
        }) else {
            continue;
        };
        if let Some(an) = subpaths.get_mut(at.0).and_then(|s| s.anchors.get_mut(at.1)) {
            (an.out, an.back) = handles;
        }
    }
}

/// The `(out, back)` a smoothed anchor `i` of `sub` should carry, or `None` when
/// there is nothing to derive one from — a lone anchor, or a chord of zero length.
fn smoothed_handles(sub: &PenSubpath, i: usize) -> Option<(Vec2, Vec2)> {
    let an = sub.anchors.get(i)?;
    let n = sub.anchors.len();
    let prev = if i > 0 {
        Some(i - 1)
    } else if sub.closed && n > 1 {
        Some(n - 1)
    } else {
        None
    };
    let next = if i + 1 < n {
        Some(i + 1)
    } else if sub.closed && n > 1 {
        Some(0)
    } else {
        None
    };
    // Already shaped: mirror the handle that was aimed on purpose. The missing side
    // of an endpoint stays zero; a lone anchor keeps the mirrored pair.
    let (out, back) = (an.out, an.back);
    if out.hypot() > 1e-9 || back.hypot() > 1e-9 {
        let long = if out.hypot() >= back.hypot() {
            out
        } else {
            -back
        };
        return Some(match (prev.is_some(), next.is_some()) {
            (false, true) => (long, Vec2::ZERO),
            (true, false) => (Vec2::ZERO, -long),
            _ => (long, -long),
        });
    }
    fresh_handles(sub, i)
}

/// The `(out, back)` anchor `i` of `sub` gets made smooth from scratch, whatever
/// handles it has: along the line between its neighbours, each a third of the
/// shorter leg, or along its one leg at an open end.
fn fresh_handles(sub: &PenSubpath, i: usize) -> Option<(Vec2, Vec2)> {
    let an = sub.anchors.get(i)?;
    let n = sub.anchors.len();
    let prev = match (i > 0, sub.closed && n > 1) {
        (true, _) => Some(i - 1),
        (false, true) => Some(n - 1),
        (false, false) => None,
    };
    let next = match (i + 1 < n, sub.closed && n > 1) {
        (true, _) => Some(i + 1),
        (false, true) => Some(0),
        (false, false) => None,
    };
    let leg = |j: usize| sub.anchors.get(j).map(|o| o.at - an.at);
    match (prev.and_then(leg), next.and_then(leg)) {
        (Some(back_leg), Some(out_leg)) => {
            let dir = out_leg - back_leg;
            (dir.hypot() > 1e-9).then(|| {
                let unit = dir / dir.hypot();
                let len = out_leg.hypot().min(back_leg.hypot()) * SMOOTH_HANDLE_FRACTION;
                (unit * len, -unit * len)
            })
        }
        (Some(leg), None) => {
            (leg.hypot() > 1e-9).then(|| (Vec2::ZERO, leg * SMOOTH_HANDLE_FRACTION))
        }
        (None, Some(leg)) => {
            (leg.hypot() > 1e-9).then(|| (leg * SMOOTH_HANDLE_FRACTION, Vec2::ZERO))
        }
        (None, None) => None,
    }
}

/// *Smooth selected*: move each anchor of `points`' handles `amount` (0–1) of the
/// way from where they are to [`fresh_handles`]' — all the way is *Make smooth*
/// from scratch, none leaves them.
pub fn smooth_points(subpaths: &mut [PenSubpath], points: &[PointRef], amount: f64) {
    let amount = amount.clamp(0.0, 1.0);
    for &(s, a) in points {
        let Some((out, back)) = subpaths.get(s).and_then(|sub| fresh_handles(sub, a)) else {
            continue;
        };
        if let Some(an) = subpaths.get_mut(s).and_then(|sub| sub.anchors.get_mut(a)) {
            an.out = an.out.lerp(out, amount);
            an.back = an.back.lerp(back, amount);
        }
    }
}

/// How far, in world units, the outline may move for a point to count as changing
/// nothing ([`redundant_points`]).
const REDUNDANT_EPS: f64 = 1e-3;

/// The anchors whose removal leaves the outline as it is, taken out, and where each
/// stood. Three kinds, each checked against the run as it is after the ones before:
///
/// - one on a zero-length segment, the twin of the anchor before it;
/// - one between two lines it lies straight along;
/// - one splitting what is one cubic in two, its handles in line and the curves
///   either side the two halves of a single cubic.
///
/// A rounded corner draws more than its anchor, so it is kept, and so are its two
/// neighbours, since its fillet is clamped to half of each leg and a longer leg
/// would round it further. A run's open ends and its last two anchors stay too.
pub fn redundant_points(subpaths: &[PenSubpath]) -> (Vec<PenSubpath>, Vec<Point>) {
    use std::collections::VecDeque;
    let mut out = Vec::with_capacity(subpaths.len());
    let mut gone = Vec::new();
    for sub in subpaths {
        // The run so far, its last anchor checked against each next one as it
        // comes: a removal there lets the anchor before meet a new neighbour,
        // which the loop then looks at in turn. Linear, where removing from the
        // middle of a `Vec` was quadratic.
        let mut kept: VecDeque<PenAnchor> = VecDeque::with_capacity(sub.anchors.len());
        let mut left = sub.anchors.len();
        for &next in &sub.anchors {
            let mut c = next;
            while kept.len() >= 2 && left + kept.len() > 2 {
                let (a, b) = (kept[kept.len() - 2], kept[kept.len() - 1]);
                let Some((out_a, back_c)) = merged_handles(a, b, c) else {
                    break;
                };
                kept.pop_back();
                if let Some(a) = kept.back_mut() {
                    a.out = out_a;
                }
                c.back = back_c;
                gone.push(b.at);
            }
            kept.push_back(c);
            left -= 1;
        }
        // A closed run's seam: its last anchor between the one before and the
        // first, then its first between the last and the second, until neither goes.
        while sub.closed && kept.len() > 2 {
            let n = kept.len();
            if let Some((out_a, back_c)) = merged_handles(kept[n - 2], kept[n - 1], kept[0]) {
                gone.push(kept[n - 1].at);
                kept.pop_back();
                kept[n - 2].out = out_a;
                kept[0].back = back_c;
            } else if let Some((out_a, back_c)) = merged_handles(kept[n - 1], kept[0], kept[1]) {
                gone.push(kept[0].at);
                kept.pop_front();
                kept[n - 2].out = out_a;
                kept[0].back = back_c;
            } else {
                break;
            }
        }
        out.push(PenSubpath {
            anchors: kept.into(),
            closed: sub.closed,
        });
    }
    (out, gone)
}

/// The handles `a` leaves on and `c` arrives on once `b` between them goes, if `b`
/// changes nothing ([`redundant_points`]): never a rounded `b`, nor one beside a
/// rounded corner.
fn merged_handles(a: PenAnchor, b: PenAnchor, c: PenAnchor) -> Option<(Vec2, Vec2)> {
    if [a.radius, b.radius, c.radius].iter().any(|r| *r != 0.0) {
        return None;
    }
    let near = |u: Vec2| u.hypot() <= REDUNDANT_EPS;
    // A twin of the anchor before: `a` takes over `b`'s way out.
    if near(b.at - a.at) && near(a.out) && near(b.back) {
        return Some((b.out, c.back));
    }
    if near(b.at - c.at) && near(b.out) && near(c.back) {
        return Some((a.out, b.back));
    }
    let straight = [a.out, b.back, b.out, c.back].into_iter().all(near);
    if straight {
        let (ab, bc) = (b.at - a.at, c.at - b.at);
        let off = ab.cross(bc).abs() / (ab + bc).hypot().max(f64::EPSILON);
        return (off <= REDUNDANT_EPS && ab.dot(bc) > 0.0).then_some((Vec2::ZERO, Vec2::ZERO));
    }
    let (lb, lo) = (b.back.hypot(), b.out.hypot());
    if lb <= REDUNDANT_EPS || lo <= REDUNDANT_EPS {
        return None;
    }
    // Where along the one cubic `b` would sit, from how its handles divide.
    let t = lb / (lb + lo);
    let p1 = a.at + a.out / t;
    let p2 = c.at + c.back / (1.0 - t);
    let whole = CubicBez::new(a.at, p1, p2, c.at);
    let left = CubicBez::new(a.at, a.at + a.out, b.at + b.back, b.at);
    let right = CubicBez::new(b.at, b.at + b.out, c.at + c.back, c.at);
    let same = |x: CubicBez, y: CubicBez| {
        [(x.p0, y.p0), (x.p1, y.p1), (x.p2, y.p2), (x.p3, y.p3)]
            .iter()
            .all(|(u, v)| u.distance(*v) <= REDUNDANT_EPS)
    };
    (same(whole.subsegment(0.0..t), left) && same(whole.subsegment(t..1.0), right))
        .then_some((p1 - a.at, p2 - c.at))
}

/// *Add points*: cut every segment into `pieces` of equal length, the new anchors
/// smooth where the segment curves, so the outline draws as before. The two legs
/// of a rounded corner stay whole: its fillet is clamped to half of each leg, so a
/// shorter leg would round it less.
pub fn split_segments(subpaths: &[PenSubpath], pieces: usize) -> Vec<PenSubpath> {
    use crate::kurbo::ParamCurveArclen;
    let pieces = pieces.max(1);
    let mut out = Vec::with_capacity(subpaths.len());
    for (s, sub) in subpaths.iter().enumerate() {
        let n = sub.anchors.len();
        let mut anchors: Vec<PenAnchor> = Vec::with_capacity(n * pieces);
        // The arriving handle the last piece of a curve leaves the next anchor.
        let mut arriving: Option<Vec2> = None;
        for i in 0..n {
            let mut here = sub.anchors[i];
            if let Some(back) = arriving.take() {
                here.back = back;
            }
            anchors.push(here);
            let Some(curve) = segment_cubic(subpaths, (s, i)) else {
                continue;
            };
            let length = curve.arclen(1e-6);
            let j = (i + 1) % n;
            let rounded = sub.anchors[i].radius != 0.0 || sub.anchors[j].radius != 0.0;
            if pieces < 2 || length <= f64::EPSILON || rounded {
                continue;
            }
            let straight = sub.anchors[i].out == Vec2::ZERO && sub.anchors[j].back == Vec2::ZERO;
            let mut cuts = vec![0.0];
            cuts.extend(
                (1..pieces).map(|k| curve.inv_arclen(length * k as f64 / pieces as f64, 1e-6)),
            );
            cuts.push(1.0);
            let parts: Vec<CubicBez> = cuts
                .windows(2)
                .map(|w| curve.subsegment(w[0]..w[1]))
                .collect();
            let handle = |v: Vec2| if straight { Vec2::ZERO } else { v };
            if let Some(last) = anchors.last_mut() {
                last.out = handle(parts[0].p1 - parts[0].p0);
            }
            for w in parts.windows(2) {
                anchors.push(PenAnchor {
                    at: w[1].p0,
                    out: handle(w[1].p1 - w[1].p0),
                    back: handle(w[0].p2 - w[0].p3),
                    radius: 0.0,
                });
            }
            let end = parts[parts.len() - 1];
            let back = handle(end.p2 - end.p3);
            // The closing segment arrives at the run's first anchor, already placed.
            match j == 0 {
                true => anchors[0].back = back,
                false => arriving = Some(back),
            }
        }
        out.push(PenSubpath {
            anchors,
            closed: sub.closed,
        });
    }
    out
}

/// The selected anchors, paired with where they sit on `axis`, sorted along it.
fn points_along(subpaths: &[PenSubpath], points: &[PointRef], axis: Axis) -> Vec<(PointRef, f64)> {
    let mut order: Vec<(PointRef, f64)> = points
        .iter()
        .filter_map(|at| {
            let an = subpaths.get(at.0)?.anchors.get(at.1)?;
            Some((*at, coord(an.at, axis)))
        })
        .collect();
    order.sort_by(|a, b| a.1.total_cmp(&b.1));
    order
}

/// The distance between each neighbouring pair of selected anchors along `axis`:
/// centre-to-centre, since a point has no edges (`build::gaps_along`'s point twin).
pub fn point_spacings(subpaths: &[PenSubpath], points: &[PointRef], axis: Axis) -> Vec<f64> {
    points_along(subpaths, points, axis)
        .windows(2)
        .map(|pair| pair[1].1 - pair[0].1)
        .collect()
}

/// Put exactly `gap` between each neighbouring pair of selected anchors along
/// `axis`, holding the **first on that axis** where it is.
///
/// The point twin of `build::distribute_spacing`, with no anchor argument since points
/// have no key to designate. Two points are enough: they have one gap.
pub fn space_points(subpaths: &mut [PenSubpath], points: &[PointRef], axis: Axis, gap: f64) {
    if points.len() < 2 || !gap.is_finite() {
        return;
    }
    let order = points_along(subpaths, points, axis);
    let Some(start) = order.first().map(|(_, v)| *v) else {
        return;
    };
    for (i, (at, was)) in order.iter().enumerate() {
        let Some(an) = subpaths.get_mut(at.0).and_then(|s| s.anchors.get_mut(at.1)) else {
            continue;
        };
        an.at += offset(axis, start + gap * i as f64 - was);
    }
}

/// Remove `points`, healing each subpath by joining the anchors either side.
///
/// Leaving a gap is [`remove_segments`], chosen by which of the two is selected. A
/// subpath left with fewer than two anchors is dropped; an empty result means the
/// caller deletes the node.
pub fn remove_points(subpaths: &[PenSubpath], points: &[PointRef]) -> Vec<PenSubpath> {
    let mut out: Vec<PenSubpath> = Vec::with_capacity(subpaths.len());
    for (i, sub) in subpaths.iter().enumerate() {
        let anchors: Vec<PenAnchor> = sub
            .anchors
            .iter()
            .enumerate()
            .filter(|(j, _)| !points.contains(&(i, *j)))
            .map(|(_, a)| *a)
            .collect();
        if anchors.len() >= 2 {
            out.push(PenSubpath {
                anchors,
                closed: sub.closed,
            });
        }
    }
    out
}

/// The anchor count of each subpath, for the app's `PointSet::retain_valid`. No
/// production caller, like its partner; kept for the tests that pin the truncation.
pub fn subpath_lengths(subpaths: &[PenSubpath]) -> Vec<usize> {
    subpaths.iter().map(|s| s.anchors.len()).collect()
}

/// Which subpaths a set of point (or segment) references lands in, ascending and
/// without repeats — the bridge to verbs whose subject is a whole run.
pub fn subpaths_touched(points: &[PointRef]) -> Vec<usize> {
    let mut out: Vec<usize> = points.iter().map(|(s, _)| *s).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Traverse the named subpaths the other way round.
///
/// Fills are non-zero (`boolean.rs`), so a contained subpath drawn the same way round
/// fills solid and the opposite way cuts a hole. Direction stays the designer's
/// (auto-reversing breaks for partly overlapping runs); the index remap this forces
/// is `PointSet::reverse_within`.
pub fn reverse_subpaths(subpaths: &mut [PenSubpath], which: &[usize]) {
    for i in which {
        if let Some(sub) = subpaths.get_mut(*i) {
            sub.reverse();
        }
    }
}

/// Where the pen should snap to close the path, at pointer position `world`.
///
/// `Some` is the first anchor's position, used both to draw the pending segment and
/// to decide that a press closes, so the snap is a promise the click keeps. Only the
/// first anchor offers a close. `radius_px` is screen pt, so the world tolerance
/// shrinks as the view zooms in.
pub fn pen_close_snap(
    anchors: &[PenAnchor],
    world: Point,
    zoom: f64,
    radius_px: f64,
) -> Option<Point> {
    let first = anchors.first().filter(|_| anchors.len() >= 2)?;
    let near = (first.at - world).hypot() * zoom < radius_px;
    near.then_some(first.at)
}

/// A visible solid stroke of `color` at `width`, every other setting at its default.
pub fn solid_stroke(color: Color, width: f64) -> Stroke {
    Stroke {
        brush: Brush::Solid(color).into(),
        width,
        ..Default::default()
    }
}

/// Create a `Path` node from world-space pen `anchors` under `parent`, carrying the
/// one `stroke` and no fill.
///
/// The node origin sits at the first anchor. `close` joins the last anchor back to
/// the first, through their handles. Fewer than two anchors yields an empty transaction.
#[cfg(test)]
pub fn create_path(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    anchors: &[PenAnchor],
    close: bool,
    stroke: Stroke,
) -> Transaction {
    let one = [PenSubpath {
        anchors: anchors.to_vec(),
        closed: close,
    }];
    create_paths(id, parent, index, parent_world, &one, stroke)
}

/// Create a `Path` node from world-space pen `subpaths` under `parent`, carrying
/// the one `stroke` and no fill — several when a mirrored run does not join.
///
/// The node origin sits at the first subpath's first anchor. A subpath of fewer
/// than two anchors is left out; none left yields an empty transaction.
pub fn create_paths(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    subpaths: &[PenSubpath],
    stroke: Stroke,
) -> Transaction {
    let subpaths: Vec<PenSubpath> = subpaths
        .iter()
        .filter(|s| s.anchors.len() >= 2)
        .cloned()
        .collect();
    let Some(first) = subpaths.first() else {
        return Transaction(vec![]);
    };
    let origin = parent_world.inverse() * first.anchors[0].at;
    let mut path = pen_path(&subpaths);
    path.apply_affine(Affine::translate(-origin.to_vec2()) * parent_world.inverse());
    Transaction(vec![
        Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Path {
                path,
                corner_radii: pen_radii(&subpaths),
                corner_smoothing: 0.0,
                mirror: crate::symmetry::PathMirror::NONE,
            },
            transform: Some(Affine::translate(origin.to_vec2())),
            name: None,
        },
        Operation::SetStrokes {
            id,
            strokes: keyed_by_position([stroke]),
        },
    ])
}

/// Write world-space `subpaths` back into the existing `Path` node `id`, whose
/// own world transform is `node_world`.
///
/// The partner of [`create_paths`] for the resume case, except that the transform is
/// not touched: extending the start of a path must not move its origin.
///
/// Fewer than two anchors in every subpath yields an empty transaction, and so does
/// a rewrite that matches `current`, the path the node already holds: resuming and
/// pressing Escape must leave the document untouched. It relies on the caller handing
/// the subpath back in the orientation it was read in (`canvas::finish_pen`).
pub fn rewrite_path(
    id: NodeId,
    node_world: Affine,
    subpaths: &[PenSubpath],
    current: Option<(&BezPath, &[f64])>,
) -> Transaction {
    if !subpaths.iter().any(|s| s.anchors.len() >= 2) {
        return Transaction(vec![]);
    }
    let mut path = pen_path(subpaths);
    path.apply_affine(node_world.inverse());
    let corner_radii = pen_radii(subpaths);
    // Both sides go to the pen's canonical form first: the `pen_anchors` → `pen_path`
    // round trip adds a `LineTo` to the ordinary SVG spelling of a closed shape, so
    // comparing raw would rewrite on a bare click. The forms differ only in
    // zero-length segments and zero radii, so a match never drops an edit.
    let canonical = current.map(|(c, r)| {
        let subs = pen_anchors(c, r);
        (pen_path(&subs), pen_radii(&subs))
    });
    if canonical.is_some_and(|(c, r)| c.elements() == path.elements() && r == corner_radii) {
        return Transaction(vec![]);
    }
    Transaction(vec![Operation::SetGeometry {
        id,
        geometry: GeometryPatch::Path {
            path,
            corner_radii,
            mirror: None,
        },
    }])
}

/// The path `id` is drawn as, as world-space subpaths, with its world transform.
///
/// **Drawn, not stored**: a path pinned on both edges of an axis is drawn with
/// its points stretched (`container::inset_stretch`), and an edit must act on
/// what is on screen. The edit's `SetGeometry` then writes the drawn points back
/// as the stored ones, in the edit's own undo step.
pub fn path_subpaths(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
) -> Option<(Affine, Vec<PenSubpath>)> {
    let NodeKind::Path {
        path, corner_radii, ..
    } = res.used_kind(doc, id)?
    else {
        return None;
    };
    let world = res.world_transform(id)?;
    let mut subpaths = pen_anchors(path, corner_radii);
    map_subpaths(&mut subpaths, world);
    Some((world, subpaths))
}

/// Why two paths cannot be joined: one has no open end.
pub const NO_OPEN_ENDS: &str = "Both paths need an open end";

/// The nearest pair of open ends between the paths `keep` and `other`, in world
/// space: an end of `keep`'s subpaths, and one of `other`'s numbered as if its
/// subpaths followed `keep`'s.
pub fn path_join_ends(
    doc: &Document,
    res: &Resolved,
    keep: NodeId,
    other: NodeId,
) -> Result<(PointRef, PointRef), &'static str> {
    let (_, ka) = path_subpaths(doc, res, keep).ok_or(NO_OPEN_ENDS)?;
    let (_, kb) = path_subpaths(doc, res, other).ok_or(NO_OPEN_ENDS)?;
    let ends = |subs: &[PenSubpath]| -> Vec<(PointRef, Point)> {
        let mut out = Vec::new();
        for (s, sub) in subs.iter().enumerate() {
            if sub.closed || sub.anchors.len() < 2 {
                continue;
            }
            let last = sub.anchors.len() - 1;
            out.push(((s, 0), sub.anchors[0].at));
            out.push(((s, last), sub.anchors[last].at));
        }
        out
    };
    let mut best: Option<(f64, PointRef, PointRef)> = None;
    for (ra, pa) in ends(&ka) {
        for (rb, pb) in ends(&kb) {
            let d = (pa - pb).hypot();
            if best.is_none_or(|(bd, ..)| d < bd) {
                best = Some((d, ra, (rb.0 + ka.len(), rb.1)));
            }
        }
    }
    let (_, ra, rb) = best.ok_or(NO_OPEN_ENDS)?;
    Ok((ra, rb))
}

/// *Join paths*: `other`'s subpaths move into `keep`, the nearest pair of open ends
/// between them spliced into one run ([`path_join_ends`], [`join_ends`]). `keep`
/// keeps its paint and name; `other` is deleted as *Delete* deletes it.
pub fn join_paths(
    doc: &Document,
    res: &Resolved,
    keep: NodeId,
    other: NodeId,
) -> Result<Transaction, &'static str> {
    let (a, b) = path_join_ends(doc, res, keep, other)?;
    let (Some((world, mut list)), Some((_, theirs))) = (
        path_subpaths(doc, res, keep),
        path_subpaths(doc, res, other),
    ) else {
        return Err(NO_OPEN_ENDS);
    };
    list.extend(theirs);
    let (list, _) = join_ends(&list, a, b).ok_or(NO_OPEN_ENDS)?;
    let mut tx = rewrite_path(keep, world, &list, None);
    tx.0.push(Operation::DeleteNode { id: other });
    tx.0.extend(crate::component::relink_for_delete(doc, &[other]));
    Ok(tx)
}

/// A point edit of the path `id` written back: `subpaths` is `base` edited, both in
/// world space. A path mirrored across its own axes is made symmetric again first
/// (`symmetry::symmetric_edit`), so editing one side moves its image. Compared
/// against the path as drawn, as [`path_subpaths`] reads it.
pub fn rewrite_points(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    world: Affine,
    base: &[PenSubpath],
    mut subpaths: Vec<PenSubpath>,
) -> Transaction {
    let Some(NodeKind::Path {
        path,
        corner_radii,
        mirror,
        ..
    }) = res.used_kind(doc, id)
    else {
        return Transaction(Vec::new());
    };
    if !mirror.is_none() {
        let to_local = world.inverse();
        let mut base = base.to_vec();
        map_subpaths(&mut base, to_local);
        map_subpaths(&mut subpaths, to_local);
        let at = crate::symmetry::centre(&pen_path(&base));
        crate::symmetry::symmetric_edit(&base, &mut subpaths, *mirror, at);
        map_subpaths(&mut subpaths, world);
    }
    rewrite_path(id, world, &subpaths, Some((path, corner_radii)))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod guide_scale_tests;

/// `↑`/`↓` re-counting a polygon or star mid-draw (`docs/shortcuts.md` §9).
#[cfg(test)]
mod step_sides_tests;

#[cfg(test)]
mod overflow_tests;

#[cfg(test)]
mod unrepresentable_basis_tests;

#[cfg(test)]
mod rewrite_no_op_tests;

/// `side_shear`'s four edge arms and its Shift snap and clamp, asserted as a bargain: the
/// side facing the grabbed one does not move, and the grabbed one moves by the drag.
#[cfg(test)]
mod side_shear_tests;

#[cfg(test)]
mod spiral_arc_tests;
