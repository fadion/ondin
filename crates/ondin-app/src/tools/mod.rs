//! Interactive tools (§9.4). Each gesture produces one `Transaction` committed
//! on release (§9.3). The transaction-building logic is pure and unit-tested;
//! the canvas supplies the egui events.
//!
//! World-space projections that more than one caller needs (move, reparent, group)
//! live in `ondin_core::build`; what stays here is tool-shaped.
//!
//! Much of this file and of `crate::preview` is headless geometry MCP will need, but
//! `ondin-app` is a binary-only crate and `ondin-mcp` sits below it, so nothing outside can
//! link it. Everything taking only `Document` / `Resolved` / kurbo types moves to `ondin_core`
//! once the first MCP tool handler needs `set_geometry`; gesture state stays.

use crate::preview::{Handle, LineEnd, PenAnchor, PenSubpath, PointRef, Side};
use ondin_core::Brush;
use ondin_core::build::{self, Axis, Basis, Edge};
use ondin_core::kurbo::{
    Affine, BezPath, CubicBez, ParamCurve, PathEl, Point, Rect, RoundedRectRadii, Size, Vec2,
};
use ondin_core::peniko::Color;
use ondin_core::{
    Document, Fill, GeometryPatch, ImageEntry, ImageId, NodeId, NodeKind, Operation, Resolved,
    Stroke, TextSizing, Transaction, geometry, keyed_by_position,
};

/// The active canvas tool. `Select` manipulates existing nodes; the others draw
/// something new (§9.4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    /// Drag anywhere to pan, without having to find empty canvas or hold space.
    Hand,
    /// Select's gestures, with the handle drag scaling the layer *photographically*
    /// instead of editing its geometry — see [`Scaling`].
    Scale,
    Frame,
    Rect,
    Ellipse,
    Polygon,
    Star,
    Line,
    Pen,
    /// Edits the *points* of a path rather than the path as a layer — anchors,
    /// their handles, add and remove.
    ///
    /// A tool, not a mode: it has a rail button, a letter and canvas chrome. The pen is
    /// reachable from inside it (`P` arms `app::pen_bias`), which reroutes no key.
    Node,
    Text,
    /// Picks a colour off the rendered canvas onto the selection
    /// (`crate::eyedropper`). One shot: a pick hands back to the tool it was
    /// armed from (`OndinApp::dropper_return`), unless Shift held it.
    Eyedropper,
    /// Places images the cursor is loaded with, one per click, and disarms itself when it
    /// runs out. Arming it with nothing loaded opens the file dialog, so the rail button
    /// and `Ctrl+Shift+K` are the same action.
    Image,
    /// Image editing: the layer's box becomes a window onto its picture. Dragging a
    /// handle crops, dragging the picture pans it, and a card holds the rest.
    ///
    /// No rail button or letter: it cannot be entered without a picture. It stays a `Tool`
    /// because input routing, the cursor, `chrome_hidden`, the `Escape` ladder and `Enter`
    /// all key off `self.tool`; the subject stays derived (`canvas::edited_image`).
    ImageEdit,
}

impl Tool {
    /// Whether this tool draws by click-dragging a box (as opposed to the
    /// click-driven pen, and Select's own gesture machinery).
    ///
    /// Text is also a click tool: a drag that returns to its start has no box under
    /// `MIN_CREATE_PX`, so the text release plants a caret (`canvas::text_gesture`); a
    /// shape abandons it.
    pub fn drags_a_box(self) -> bool {
        matches!(
            self,
            Tool::Frame
                | Tool::Rect
                | Tool::Ellipse
                | Tool::Polygon
                | Tool::Star
                | Tool::Line
                | Tool::Text
                // A drag draws the box the image fills; a click places it fitted.
                | Tool::Image
        )
    }

    /// What a plain click with this tool makes, or `None` for a tool whose click
    /// means something else — or nothing.
    ///
    /// Exhaustive with no wildcard, so a new tool is a build error here. Frame and Line
    /// answer `None` deliberately: a default frame guesses at a layout, and a default
    /// line would invent a direction.
    pub fn click_makes(self) -> Option<ClickCreate> {
        match self {
            Tool::Rect | Tool::Ellipse | Tool::Polygon | Tool::Star => {
                Some(ClickCreate::DefaultBox)
            }
            Tool::Image => Some(ClickCreate::FittedImage),
            Tool::Text => Some(ClickCreate::TextNode),
            Tool::Frame | Tool::Line => None,
            Tool::Select
            | Tool::Scale
            | Tool::Pen
            | Tool::Node
            | Tool::ImageEdit
            | Tool::Hand
            | Tool::Eyedropper => None,
        }
    }

    /// Whether this tool picks, moves and transforms existing layers — Select and
    /// Scale, which differ only in what a handle drag *means*.
    ///
    /// The cursor matches `Tool::Select | Tool::Scale` itself, since the two have
    /// different rest states (`canvas::rest_cursor`).
    pub fn selects(self) -> bool {
        matches!(self, Tool::Select | Tool::Scale)
    }

    /// What a handle drag with this tool does to the layer.
    pub fn scaling(self) -> Scaling {
        match self {
            Tool::Scale => Scaling::Photographic,
            _ => Scaling::Geometry,
        }
    }
}

/// What a plain click with a creation tool makes ([`Tool::click_makes`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClickCreate {
    /// A [`CLICK_CREATE_SIZE`] square carrying the tool's own defaults.
    DefaultBox,
    /// The loaded picture at its intrinsic size, shrunk only if it exceeds the
    /// frame it lands in ([`fitted_image_size`]).
    FittedImage,
    /// A text node that sizes itself to what is typed — or, over existing text, the
    /// caret placed in it.
    TextNode,
}

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

/// The square a shape tool's *click* gets, in world units so a click makes the same
/// shape at every zoom ([`ClickCreate::DefaultBox`]).
pub const CLICK_CREATE_SIZE: f64 = 100.0;

/// Sides a fresh polygon is drawn with — a triangle, the smallest shape.
pub const POLYGON_DEFAULT_SIDES: u32 = 3;
/// Points a fresh star is drawn with, and its inner radius: five points, inner ring at
/// the golden-section radius where the spokes' edges line up straight through the middle.
pub const STAR_DEFAULT_POINTS: u32 = 5;
pub const STAR_DEFAULT_RATIO: f64 = 0.382;

/// The authored local box of a node, or `None` for the kinds with no such field
/// (lines, paths, groups, booleans), which scale through [`resize_geometry`] and
/// `scale_geometry` instead of a `GeometryPatch::Size`.
pub fn resizable_size(kind: &NodeKind) -> Option<Size> {
    match kind {
        NodeKind::Rect { size, .. }
        | NodeKind::Ellipse { size }
        | NodeKind::Polygon { size, .. }
        | NodeKind::Star { size, .. }
        | NodeKind::Artboard { size, .. } => Some(*size),
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
fn anchored_box(kind: &NodeKind, text: Option<&ondin_core::TextLayout>) -> Option<Rect> {
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
pub fn gesture_text_sizing(node: &ondin_core::Node, used: TextSizing) -> TextSizing {
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
fn railed_box(kind: &NodeKind, rail: &ondin_core::kurbo::BezPath) -> Option<Rect> {
    let content = ondin_core::TextRef::of(kind)?.content.to_owned();
    let mut parts = ondin_core::TextParts::of(kind)?;
    parts.on_path = Some(rail.clone());
    Some(ondin_core::text::layout(parts.as_ref(&content)).bounds())
}

/// On a rail a resize edits the rail: the patch both resize doors write
/// ([`resize_to_handle`] and [`scale_geometry`]), from the drawn box to `want`,
/// holding `hold`'s corner. Solved against the type this transaction will leave
/// ([`kind_to_solve_against`]), `bound` the node's text bindings; `drawn` stays
/// the unscaled box.
fn railed_patch(
    kind: &NodeKind,
    bound: &ondin_core::TextVars,
    rail: &ondin_core::kurbo::BezPath,
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
    rail: &ondin_core::kurbo::BezPath,
    drawn: Rect,
    want: Rect,
    hold: Handle,
) -> ondin_core::kurbo::BezPath {
    let measure =
        |candidate: &ondin_core::kurbo::BezPath| -> Option<Rect> { railed_box(kind, candidate) };
    let about = drawn.origin().to_vec2();
    let scaled = |sx: f64, sy: f64| -> ondin_core::kurbo::BezPath {
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
    let Some(local) = ondin_core::local_box(doc, res, id) else {
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
    let Some(local) = ondin_core::local_box(doc, res, id) else {
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
    let NodeKind::Line { end } = *res.used_kind_of(node) else {
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

/// Where the two ends of the line `id` are in world space, near end first.
pub fn line_ends_world(doc: &Document, res: &Resolved, id: NodeId) -> Option<(Point, Point)> {
    let node = doc.get(id)?;
    let NodeKind::Line { end } = *res.used_kind_of(node) else {
        return None;
    };
    let world = res.world_transform(id)?;
    Some((world * Point::ZERO, world * end))
}

/// Resize a leaf whose geometry is its own shape — a `Line` or a `Path` — by
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
    let (sx, sy, anchor) = resize_factors(
        local,
        handle,
        world.inverse() * pointer_world,
        opts.keep_ratio,
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
            NodeKind::Path { path, corner_radii } => {
                let mut scaled = path.clone();
                scaled.apply_affine(Affine::scale_non_uniform(csx, csy));
                ops.push(Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::Path {
                        path: scaled,
                        // Carried: only the Scale tool multiplies it (`scale_scalars`).
                        corner_radii: corner_radii.clone(),
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
                    if let Some(item) = ondin_core::build::sized_flex_item(doc, res, id, size, to) {
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
        ondin_core::paint_box::carry_into(doc, id, Affine::scale_non_uniform(csx, csy), ops);
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
fn scaled_text(kind: &NodeKind, s: f64, bound: &ondin_core::TextVars) -> Option<NodeKind> {
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
    for (k, var) in bound
        .iter()
        .enumerate()
        .skip(ondin_core::variable::TEXT_SIZE)
    {
        if var.is_some() {
            ondin_core::variable::copy_text_slot(k, kind, &mut out);
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
    bound: &ondin_core::TextVars,
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
                if let ondin_core::StrokeSides::Custom {
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
        NodeKind::Rect { corner_radii, .. } => {
            let r = *corner_radii;
            if !ondin_core::geometry::is_square_cornered(&r) {
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
        NodeKind::Path { path, corner_radii } if corner_radii.iter().any(|r| *r > 0.0) => {
            ops.push(Operation::SetGeometry {
                id,
                geometry: GeometryPatch::Path {
                    path: path.clone(),
                    corner_radii: corner_radii.iter().map(|r| r * s).collect(),
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
                ondin_core::GuideAxis::Vertical => csx,
                ondin_core::GuideAxis::Horizontal => csy,
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
    use ondin_core::container::{Display, Track, TrackBreadth, TrackSize};
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
    let new_world = turn_about(pivot, angle) * old_world;
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
/// `box_` is the node's local box ([`ondin_core::local_box`]). The shear is in the
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
    parts: &ondin_core::TextParts,
) -> Transaction {
    let (origin, size) = local_box(parent_world, a, b);
    // `line_height: None` means the font decides, so shape an empty string for it.
    let mut auto = parts.clone();
    auto.sizing = TextSizing::Auto;
    let line = ondin_core::text::layout(auto.as_ref("")).line_height;
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
/// [`ondin_core::ImageFit::Fill`] crops rather than squashes.
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
            brush: ondin_core::image_brush(image).into(),
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
/// ([`ondin_core::Paint::image_fill`], shared with `build::mask_target`).
pub fn croppable_fill(paint: &ondin_core::Paint) -> Option<usize> {
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
            kind: NodeKind::Ellipse { size },
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

/// The anchor count of each subpath, for `PointSet::retain_valid`. Dead in production
/// and kept deliberately, allowed rather than `cfg(test)`-ed like its partner.
#[allow(dead_code)]
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

/// Create a `Path` node from world-space pen `anchors` under `parent`, stroked
/// with `color` at `width`.
///
/// The node origin sits at the first anchor. `close` joins the last anchor back to
/// the first, through their handles. Fewer than two anchors yields an empty transaction.
#[allow(clippy::too_many_arguments)]
pub fn create_path(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    anchors: &[PenAnchor],
    close: bool,
    color: Color,
    width: f64,
) -> Transaction {
    if anchors.len() < 2 {
        return Transaction(vec![]);
    }
    let origin = parent_world.inverse() * anchors[0].at;
    let mut path = pen_outline(anchors, close);
    path.apply_affine(Affine::translate(-origin.to_vec2()) * parent_world.inverse());
    Transaction(vec![
        Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Path {
                path,
                corner_radii: pen_radii(&[PenSubpath {
                    anchors: anchors.to_vec(),
                    closed: close,
                }]),
            },
            transform: Some(Affine::translate(origin.to_vec2())),
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

/// Write world-space `subpaths` back into the existing `Path` node `id`, whose
/// own world transform is `node_world`.
///
/// The partner of [`create_path`] for the resume case, except that the transform is
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
        geometry: GeometryPatch::Path { path, corner_radii },
    }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::{IdSource, TextStyle};

    /// A crop handle holds the opposite edges and stops at the picture.
    #[test]
    fn a_crop_handle_holds_the_opposite_edges_and_stops_at_the_picture() {
        let frame = Rect::new(0.0, 0.0, 100.0, 100.0);
        let limit = Rect::new(-20.0, -10.0, 160.0, 140.0);

        let in_ = crop_resized(frame, Handle::BottomRight, Point::new(60.0, 70.0), limit);
        assert_eq!(in_, Rect::new(0.0, 0.0, 60.0, 70.0));

        let out = crop_resized(frame, Handle::BottomRight, Point::new(500.0, 500.0), limit);
        assert_eq!(out, Rect::new(0.0, 0.0, limit.x1, limit.y1));

        // An edge handle owns one axis only.
        let side = crop_resized(frame, Handle::Left, Point::new(-50.0, 999.0), limit);
        assert_eq!(
            side,
            Rect::new(limit.x0, 0.0, 100.0, 100.0),
            "a left drag moved an edge it does not own"
        );

        // A mirrored rectangle also has `x0 < x1`, hence the exact one.
        let past = crop_resized(
            frame,
            Handle::BottomRight,
            Point::new(-500.0, -500.0),
            limit,
        );
        assert_eq!(
            past,
            Rect::new(0.0, 0.0, MIN_CROP, MIN_CROP),
            "a handle run across the box must collapse onto the held corner, not \
             mirror about it"
        );
        let past_tl = crop_resized(frame, Handle::TopLeft, Point::new(500.0, 500.0), limit);
        assert_eq!(
            past_tl,
            Rect::new(100.0 - MIN_CROP, 100.0 - MIN_CROP, 100.0, 100.0),
            "the top-left handle must collapse onto the bottom-right corner"
        );
    }

    /// The cropped fill is the first visible image; hidden images and solids are skipped.
    #[test]
    fn the_cropped_fill_is_the_first_visible_image() {
        use ondin_core::{Fill, Paint, image_brush};
        let img = |v: bool| Fill {
            brush: image_brush(ondin_core::ImageId("a".into())).into(),
            visible: v,
        };
        let solid = Fill {
            brush: ondin_core::Brush::Solid(ondin_core::peniko::Color::BLACK).into(),
            visible: true,
        };
        let paint = |fills: Vec<Fill>| Paint {
            fills: ondin_core::keyed_by_position(fills),
            strokes: Vec::new(),
        };
        assert_eq!(
            croppable_fill(&paint(vec![solid.clone(), img(true)])),
            Some(1)
        );
        assert_eq!(
            croppable_fill(&paint(vec![img(false), img(true)])),
            Some(1),
            "a hidden picture is not the one being cropped"
        );
        assert_eq!(croppable_fill(&paint(vec![solid.clone()])), None);
        assert_eq!(croppable_fill(&paint(vec![img(false)])), None);
        assert_eq!(croppable_fill(&paint(Vec::new())), None);
    }

    /// The box `crop_resized` computes is the box `resize_layer` writes, compared in
    /// world space since a resize re-origins the local box.
    #[test]
    fn a_crop_resize_writes_the_very_box_its_crop_was_computed_against() {
        let (doc, res, rect) = artboard_with_rect();
        let frame = ondin_core::local_box(&doc, &res, rect).expect("a box");
        let world = res.world_transform(rect).expect("a transform");
        let limit = frame.inflate(200.0, 200.0);

        for (handle, at) in [
            (Handle::BottomRight, Point::new(30.0, 20.0)),
            (Handle::TopLeft, Point::new(11.0, 7.0)),
            (Handle::Left, Point::new(-40.0, 0.0)),
            (Handle::Bottom, Point::new(0.0, 90.0)),
            // Past the held edge, where both floors must agree.
            (Handle::TopLeft, Point::new(999.0, 999.0)),
        ] {
            let next = crop_resized(frame, handle, at, limit);
            let (ux, uy) = handle.unit();
            let corner = Point::new(
                next.min_x() + next.width() * ux,
                next.min_y() + next.height() * uy,
            );
            let tx = resize_layer(
                &doc,
                &res,
                rect,
                handle,
                world * corner,
                Resize::geometry(false, false),
            );
            let mut after = doc.clone();
            after.apply(&tx).expect("the resize applies");
            let res_after = Resolved::rebuild(&after);
            let local = ondin_core::local_box(&after, &res_after, rect).expect("a box after");
            let world_after = res_after.world_transform(rect).expect("a transform after");
            let want = world.transform_rect_bbox(next);
            let got = world_after.transform_rect_bbox(local);
            assert!(
                (got.x0 - want.x0).abs() < 1e-6
                    && (got.y0 - want.y0).abs() < 1e-6
                    && (got.x1 - want.x1).abs() < 1e-6
                    && (got.y1 - want.y1).abs() < 1e-6,
                "{handle:?} to {at:?}: the crop was computed for {want:?} in world \
                 space and the layer became {got:?} — the picture slides by the \
                 difference"
            );
        }
    }

    /// A crop drag leaves the photograph where it was: the source-to-page transform
    /// is identical before and after, and the crop stays inside the source.
    #[test]
    fn a_crop_drag_leaves_the_photograph_exactly_where_it_was() {
        let (doc, res, rect) = artboard_with_rect();
        let frame = ondin_core::local_box(&doc, &res, rect).expect("a box");
        let world = res.world_transform(rect).expect("a transform");
        let (w, h) = (400u32, 300u32);
        let img = ondin_core::ImageRef {
            fit: ondin_core::ImageFit::Crop,
            crop: Rect::new(0.2, 0.1, 0.9, 0.6),
            ..ondin_core::ImageRef::new(ondin_core::ImageId("x".into()))
        };
        let limit = img.picture_box(frame, w, h).expect("a picture");
        let before = world * img.framing(frame, w, h).transform;

        for (handle, at) in [
            (Handle::BottomRight, Point::new(30.0, 20.0)),
            (Handle::TopLeft, Point::new(12.0, 9.0)),
            (Handle::Left, Point::new(-30.0, 0.0)),
            (Handle::Bottom, Point::new(0.0, 120.0)),
            // Out to the picture's own edge, where the clamp decides.
            (Handle::TopLeft, Point::new(-9_999.0, -9_999.0)),
        ] {
            let next = crop_resized(frame, handle, at, limit);
            let after_img = ondin_core::ImageRef {
                crop: ondin_core::crop_reframed(img.crop, frame, next),
                ..img.clone()
            };
            let (ux, uy) = handle.unit();
            let corner = Point::new(
                next.min_x() + next.width() * ux,
                next.min_y() + next.height() * uy,
            );
            let tx = resize_layer(
                &doc,
                &res,
                rect,
                handle,
                world * corner,
                Resize::geometry(false, false),
            );
            let mut after = doc.clone();
            after.apply(&tx).expect("the resize applies");
            let res_after = Resolved::rebuild(&after);
            let frame_after = ondin_core::local_box(&after, &res_after, rect).expect("a box after");
            let world_after = res_after.world_transform(rect).expect("a transform after");
            let got = world_after * after_img.framing(frame_after, w, h).transform;

            let (a, b) = (before.as_coeffs(), got.as_coeffs());
            assert!(
                a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-6),
                "{handle:?} to {at:?}: the picture was at {a:?} and is now at \
                 {b:?} — cropping moved the photograph"
            );
            let c = after_img.crop;
            assert!(
                c.x0 >= -1e-9 && c.y0 >= -1e-9 && c.x1 <= 1.0 + 1e-9 && c.y1 <= 1.0 + 1e-9,
                "{handle:?} to {at:?}: the crop left the source: {c:?}"
            );
        }
    }

    fn artboard_with_rect() -> (Document, Resolved, NodeId) {
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let ab = ids.mint();
        let rect = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 400.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: rect,
                parent: ab,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(50.0, 50.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((100.0, 100.0))),
                name: None,
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, rect)
    }

    /// A group holding one text node in `sizing`, offset inside it.
    fn group_with_text(sizing: TextSizing) -> (Document, Resolved, NodeId, NodeId) {
        let mut ids = IdSource::new(7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let group = ids.mint();
        let text = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: text,
                parent: group,
                index: 0,
                kind: NodeKind::Text {
                    content: "Hello there".into(),
                    style: Box::new(ondin_core::TextStyle {
                        font_family: "Inter".into(),
                        font_size: 16.0,
                        weight: 400,
                        italic: false,
                        line_height: Some(ondin_core::Length::Em(1.2)),
                        ..Default::default()
                    }),
                    spans: Default::default(),
                    para_spans: Default::default(),
                    paragraph: Default::default(),
                    block: Default::default(),
                    sizing,
                    on_path: None,
                    on_path_flip: false,
                    on_path_offset: 0.0,
                },
                transform: Some(Affine::translate((10.0, 10.0))),
                name: None,
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, group, text)
    }

    /// The text child's sizing after the group's bottom-right corner is dragged out
    /// to double the box on both axes.
    fn double_group(sizing: TextSizing) -> TextSizing {
        let (mut doc, res, group, text) = group_with_text(sizing);
        let local = ondin_core::local_box(&doc, &res, group).unwrap();
        doc.apply(&resize_group(
            &doc,
            &res,
            group,
            local,
            Handle::BottomRight,
            Point::new(
                local.min_x() + 2.0 * local.width(),
                local.min_y() + 2.0 * local.height(),
            ),
            Resize::geometry(false, false),
        ))
        .unwrap();
        sizing_of(&doc, text)
    }

    /// Resizing a group leaves an auto-sized text child auto (not a 1×1 fixed box).
    #[test]
    fn scaling_a_group_leaves_auto_sized_text_alone() {
        let sizing = double_group(TextSizing::Auto);
        assert!(
            matches!(sizing, TextSizing::Auto),
            "auto text was converted to {sizing:?} by a group resize"
        );
    }

    /// A group resize scales an auto-height child's width and keeps it auto-height.
    #[test]
    fn scaling_a_group_scales_an_auto_height_child_without_fixing_it() {
        let sizing = double_group(TextSizing::AutoHeight(50.0));
        assert!(
            matches!(sizing, TextSizing::AutoHeight(w) if (w - 100.0).abs() < 1e-9),
            "expected auto-height at 100, got {sizing:?}"
        );
    }

    /// A group of two rects not starting at its own origin: local box 10,5..160,80.
    fn group_of_two_rects() -> (Document, Resolved, NodeId) {
        let mut ids = IdSource::new(11);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let group = ids.mint();
        let (a, b) = (ids.mint(), ids.mint());
        let rect = |size: Size| NodeKind::Rect {
            size,
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        };
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: 0,
                kind: NodeKind::Group,
                transform: Some(Affine::translate((30.0, 20.0))),
                name: None,
            },
            Operation::CreateNode {
                id: a,
                parent: group,
                index: 0,
                kind: rect(Size::new(40.0, 20.0)),
                transform: Some(Affine::translate((10.0, 5.0))),
                name: None,
            },
            Operation::CreateNode {
                id: b,
                parent: group,
                index: 1,
                kind: rect(Size::new(60.0, 30.0)),
                transform: Some(Affine::translate((100.0, 50.0))),
                name: None,
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, group)
    }

    /// Typing a group's W and H resizes it holding its top-left; the offset fixture
    /// makes the position assertion catch scaling about (0,0).
    /// flip: `resize_box_to` holds `Handle::TopLeft` → fails at the size assertion
    #[test]
    fn typing_a_width_on_a_group_resizes_it_from_its_own_top_left() {
        let (mut doc, res, group) = group_of_two_rects();
        let before = ondin_core::local_box(&doc, &res, group).unwrap();
        assert!(
            (before.width() - 150.0).abs() < 1e-9 && (before.height() - 75.0).abs() < 1e-9,
            "the fixture's group measured {before:?} rather than 150 × 75"
        );
        doc.apply(&resize_box_to(&doc, &res, group, Size::new(300.0, 150.0)))
            .unwrap();
        let after = ondin_core::local_box(&doc, &Resolved::rebuild(&doc), group).unwrap();
        assert!(
            (after.width() - 300.0).abs() < 1e-6 && (after.height() - 150.0).abs() < 1e-6,
            "asked for 300 × 150 and the group measured {after:?}"
        );
        assert!(
            (after.min_x() - before.min_x()).abs() < 1e-6
                && (after.min_y() - before.min_y()).abs() < 1e-6,
            "the group's top-left moved from {:?} to {:?}",
            before.origin(),
            after.origin()
        );
    }

    /// Dragging an auto-sized label's left edge widens it leftward and holds the
    /// right edge, asserted there since both behaviours give the same new origin.
    #[test]
    fn dragging_an_auto_labels_left_edge_holds_its_right_edge() {
        let (mut doc, res, _group, text) = group_with_text(TextSizing::Auto);
        let measured = ondin_core::local_box(&doc, &res, text).unwrap();
        let right = 10.0 + measured.width();
        let tx = resize_to_handle(
            &doc,
            &res,
            text,
            Handle::Left,
            Point::new(-30.0, 10.0),
            Resize::geometry(false, false),
        );
        doc.apply(&tx).unwrap();

        let sizing = sizing_of(&doc, text);
        let wanted = right + 30.0;
        assert!(
            matches!(sizing, TextSizing::AutoHeight(w) if (w - wanted).abs() < 0.5),
            "expected auto-height at {wanted} (the box from -30 to {right}), got {sizing:?}"
        );
        let res = Resolved::rebuild(&doc);
        let world = res.world_transform(text).unwrap();
        let origin = world * Point::ZERO;
        assert!(
            (origin.x - -30.0).abs() < 1e-9,
            "the dragged edge is not under the pointer: {origin:?}"
        );
        let far = world * Point::new(wanted, 0.0);
        assert!(
            (far.x - right).abs() < 0.5,
            "the held edge moved from {right} to {}",
            far.x
        );
    }

    /// A top drag on a trimmed auto label holds the bottom edge the handles drew
    /// (the trimmed box starts below the local origin).
    #[test]
    fn dragging_the_top_of_a_trimmed_label_holds_the_bottom_the_handles_drew() {
        let (mut doc, _res, _group, text) = group_with_text(TextSizing::Auto);
        doc.apply(&Transaction(vec![Operation::SetBlockStyle {
            id: text,
            block: ondin_core::BlockStyle {
                trim: ondin_core::BoxTrim::CapToBaseline,
                ..Default::default()
            },
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let drawn = ondin_core::local_box(&doc, &res, text).unwrap();
        assert!(
            drawn.min_y() > 0.1,
            "cap-to-baseline trim must inset the box from the origin: {drawn:?}"
        );
        let bottom = 10.0 + drawn.max_y();

        let tx = resize_to_handle(
            &doc,
            &res,
            text,
            Handle::Top,
            Point::new(10.0, -20.0),
            Resize::geometry(false, false),
        );
        doc.apply(&tx).unwrap();

        let TextSizing::Fixed(size) = sizing_of(&doc, text) else {
            panic!("a top drag authors the height");
        };
        let res = Resolved::rebuild(&doc);
        let after = ondin_core::local_box(&doc, &res, text).unwrap();
        let world = res.world_transform(text).unwrap();
        let far = world * Point::new(0.0, after.max_y());
        assert!(
            (far.y - bottom).abs() < 0.5,
            "the held edge moved from {bottom} to {} (box {size:?}, drawn {after:?})",
            far.y
        );
        let top = world * Point::new(0.0, after.min_y());
        assert!(
            (top.y - -20.0).abs() < 0.5,
            "and the dragged edge must be under the pointer, not at {}",
            top.y
        );
    }

    #[test]
    fn resize_from_bottom_right_only_changes_size() {
        let (mut doc, res, rect) = artboard_with_rect();
        let tx = resize_to_handle(
            &doc,
            &res,
            rect,
            Handle::BottomRight,
            Point::new(260.0, 180.0),
            Resize::geometry(false, false),
        );
        doc.apply(&tx).unwrap();
        let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
            panic!();
        };
        assert!((size.width - 160.0).abs() < 1e-9);
        assert!((size.height - 80.0).abs() < 1e-9);
        let origin = Resolved::rebuild(&doc).world_transform(rect).unwrap() * Point::ZERO;
        assert!((origin.x - 100.0).abs() < 1e-9 && (origin.y - 100.0).abs() < 1e-9);
    }

    #[test]
    fn resize_from_top_left_moves_the_origin_and_holds_the_far_corner() {
        let (mut doc, res, rect) = artboard_with_rect();
        let tx = resize_to_handle(
            &doc,
            &res,
            rect,
            Handle::TopLeft,
            Point::new(120.0, 110.0),
            Resize::geometry(false, false),
        );
        doc.apply(&tx).unwrap();
        let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
            panic!();
        };
        assert!((size.width - 30.0).abs() < 1e-9, "w={}", size.width);
        assert!((size.height - 40.0).abs() < 1e-9, "h={}", size.height);

        let res = Resolved::rebuild(&doc);
        let w = res.world_transform(rect).unwrap();
        let origin = w * Point::ZERO;
        let far = w * Point::new(size.width, size.height);
        assert!((origin.x - 120.0).abs() < 1e-9 && (origin.y - 110.0).abs() < 1e-9);
        assert!(
            (far.x - 150.0).abs() < 1e-9 && (far.y - 150.0).abs() < 1e-9,
            "anchor corner moved: {far:?}"
        );
    }

    /// Every kind flips past its anchor, and the two resize paths agree: a `Rect` lands
    /// in the same world box as a `Path`, so the assertion is on `Mirror`. Alt is the
    /// same rule about the centre.
    /// flip: `resized_box`'s `crossed` returns `false` → fails at the rect's `Mirror::X`
    #[test]
    fn a_rect_flips_past_its_anchor_exactly_as_a_path_does() {
        use ondin_core::build::{Basis, Mirror};

        let rect = || NodeKind::Rect {
            size: Size::new(100.0, 100.0),
            corner_radii: ondin_core::kurbo::RoundedRectRadii::new(16.0, 0.0, 0.0, 0.0),
            corner_smoothing: 0.0,
        };
        let path = || {
            let mut pen = BezPath::new();
            pen.move_to((0.0, 0.0));
            pen.line_to((100.0, 0.0));
            pen.line_to((100.0, 100.0));
            pen.line_to((0.0, 100.0));
            pen.close_path();
            NodeKind::Path {
                path: pen,
                corner_radii: Vec::new(),
            }
        };

        // `(what, handle, pointer, symmetric)`: each drag runs against both kinds.
        let cases: [(&str, Handle, Point, bool); 3] = [
            (
                "the right handle dragged past the left edge",
                Handle::Right,
                Point::new(-50.0, 50.0),
                false,
            ),
            (
                "the right handle dragged past the centre with Alt",
                Handle::Right,
                Point::new(20.0, 50.0),
                true,
            ),
            // `Handle::Left` holds the right edge: only a drag past 100 crosses.
            (
                "the left handle dragged past the right edge",
                Handle::Left,
                Point::new(150.0, 50.0),
                false,
            ),
        ];

        for (what, handle, at, symmetric) in cases {
            let mut seen: Vec<(Mirror, Rect)> = Vec::new();
            for (label, kind) in [("rect", rect()), ("path", path())] {
                let mut ids = IdSource::new(0x1F3);
                let root = ids.mint();
                let mut doc = Document::new(root);
                let id = ids.mint();
                doc.apply(&Transaction(vec![Operation::CreateNode {
                    id,
                    parent: root,
                    index: 0,
                    kind,
                    transform: None,
                    name: None,
                }]))
                .expect("a node");
                let res = Resolved::rebuild(&doc);
                let tx = resize_layer(
                    &doc,
                    &res,
                    id,
                    handle,
                    at,
                    Resize::geometry(false, symmetric),
                );
                let mut after = doc.clone();
                after.apply(&tx).expect("applies");
                let ra = Resolved::rebuild(&after);
                let world = ra.world_transform(id).expect("a transform");
                let local = ondin_core::local_box(&after, &ra, id).expect("a box");
                let ink: BezPath = ondin_core::kurbo::Shape::to_path(&local, 0.01);
                let world_box = ondin_core::kurbo::Shape::bounding_box(&(world * ink));
                let mirror = Basis::of(world).orientation.mirror;
                assert_eq!(
                    mirror,
                    Mirror::X,
                    "{what}: the {label} is turned over — §5.6's flip, unqualified"
                );
                seen.push((mirror, world_box));
            }
            // Control: the two kinds land in the same box.
            let (_, rect_box) = seen[0];
            let (_, path_box) = seen[1];
            assert!(
                (rect_box.x0 - path_box.x0).abs() < 1e-6
                    && (rect_box.x1 - path_box.x1).abs() < 1e-6
                    && (rect_box.y0 - path_box.y0).abs() < 1e-6
                    && (rect_box.y1 - path_box.y1).abs() < 1e-6,
                "{what}: the two kinds land in the same world box — \
                 rect {rect_box:?} against path {path_box:?}"
            );
        }
    }

    /// A drag that stops short of the anchor flips nothing: the control for the test
    /// above, which `crossed` returning `true` would pass.
    /// flip: `crossed` spelled `at < anchor` → fails at "the left handle pulled in"
    #[test]
    fn an_ordinary_resize_flips_nothing() {
        use ondin_core::build::{Basis, Mirror};

        for (what, handle, at) in [
            ("the right handle pulled out", Handle::Right, 150.0),
            (
                "the right handle pulled in, short of the edge",
                Handle::Right,
                30.0,
            ),
            ("the left handle pulled out", Handle::Left, -50.0),
            (
                "the left handle pulled in, short of the edge",
                Handle::Left,
                70.0,
            ),
        ] {
            let mut ids = IdSource::new(0x1F4);
            let root = ids.mint();
            let mut doc = Document::new(root);
            let id = ids.mint();
            doc.apply(&Transaction(vec![Operation::CreateNode {
                id,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 100.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            }]))
            .expect("a rect");
            let res = Resolved::rebuild(&doc);
            let tx = resize_layer(
                &doc,
                &res,
                id,
                handle,
                Point::new(at, 50.0),
                Resize::geometry(false, false),
            );
            let mut after = doc.clone();
            after.apply(&tx).expect("applies");
            let ra = Resolved::rebuild(&after);
            let world = ra.world_transform(id).expect("a transform");
            assert_eq!(
                Basis::of(world).orientation.mirror,
                Mirror::None,
                "{what}: nothing was dragged past anything"
            );
        }
    }

    /// Dragging a handle past the opposite one flips a path: the bottom-right corner
    /// of a 0..20 box dragged to -20 occupies -20..0 in world space, not a collapse.
    #[test]
    fn dragging_a_handle_past_the_opposite_corner_flips_a_path() {
        let mut ids = IdSource::new(0x1F1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let path = ids.mint();
        let mut pen = BezPath::new();
        pen.move_to((0.0, 0.0));
        pen.line_to((20.0, 0.0));
        pen.line_to((20.0, 20.0));
        pen.close_path();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: path,
            parent: root,
            index: 0,
            kind: NodeKind::Path {
                path: pen,
                corner_radii: Vec::new(),
            },
            transform: None,
            name: None,
        }]))
        .expect("a path");
        let res = Resolved::rebuild(&doc);
        let local = ondin_core::local_box(&doc, &res, path).expect("a box");

        let tx = resize_geometry(
            &doc,
            &res,
            path,
            local,
            Handle::BottomRight,
            Point::new(-20.0, -20.0),
            Resize::default(),
        );
        let mut after = doc.clone();
        after.apply(&tx).expect("applies");
        let res_after = Resolved::rebuild(&after);
        let world = res_after.world_transform(path).unwrap();
        let NodeKind::Path { path: shape, .. } = after.get(path).unwrap().kind() else {
            panic!("still a path")
        };
        let now = ondin_core::kurbo::Shape::bounding_box(&(world * shape.clone()));

        assert!(
            (now.x1 - 0.0).abs() < 1e-6 && (now.y1 - 0.0).abs() < 1e-6,
            "the held corner stayed at the origin: {now:?}"
        );
        assert!(
            (now.x0 + 20.0).abs() < 1e-6 && (now.y0 + 20.0).abs() < 1e-6,
            "and the shape is now on the other side of it, not collapsed: {now:?}"
        );
        assert!(
            now.width() > 19.0 && now.height() > 19.0,
            "it kept its size through the flip: {now:?}"
        );
    }

    /// The sign survives the clamp, and the magnitude is still clamped at both ends.
    #[test]
    fn a_scale_factor_keeps_its_sign_and_clamps_its_magnitude() {
        assert_eq!(scale_along(10.0, 20.0), 2.0);
        assert_eq!(
            scale_along(10.0, -20.0),
            -2.0,
            "a mirror is a negative factor"
        );
        assert_eq!(
            scale_along(10.0, 0.0),
            MIN_EXTENT,
            "and zero is still floored"
        );
        assert_eq!(
            scale_along(1.0, -1e9),
            -MAX_SCALE,
            "the cap applies to the magnitude, not to the value"
        );
        assert_eq!(scale_along(0.0, 5.0), 1.0, "no extent to have scaled");
    }
    /// Dragging a line's far end moves only that end.
    #[test]
    fn dragging_a_lines_end_moves_only_that_end() {
        let (doc, res, line) = line_fixture(Affine::translate((40.0, 10.0)));
        let (start_before, _) = line_ends_world(&doc, &res, line).unwrap();

        let tx = move_line_end(&doc, &res, line, LineEnd::End, Point::new(200.0, 90.0));
        let mut after = doc.clone();
        after.apply(&tx).expect("applies");
        let res_after = Resolved::rebuild(&after);
        let (start, end) = line_ends_world(&after, &res_after, line).unwrap();

        approx_pt(
            end,
            Point::new(200.0, 90.0),
            "the dragged end went to the pointer",
        );
        approx_pt(start, start_before, "and the other end did not move");
    }

    /// Dragging a line's start holds the far end still: the start is the node origin
    /// (§5.3), so `end` must be re-expressed.
    #[test]
    fn dragging_a_lines_start_holds_the_far_end_still() {
        let (doc, res, line) = line_fixture(Affine::translate((40.0, 10.0)));
        let (_, end_before) = line_ends_world(&doc, &res, line).unwrap();

        let tx = move_line_end(&doc, &res, line, LineEnd::Start, Point::new(-30.0, 55.0));
        let mut after = doc.clone();
        after.apply(&tx).expect("applies");
        let res_after = Resolved::rebuild(&after);
        let (start, end) = line_ends_world(&after, &res_after, line).unwrap();

        approx_pt(
            start,
            Point::new(-30.0, 55.0),
            "the start went to the pointer",
        );
        approx_pt(
            end,
            end_before,
            "and the far end stayed exactly where it was",
        );
    }

    /// Moving either end of a rotated line keeps its basis.
    #[test]
    fn moving_an_end_of_a_rotated_line_keeps_its_basis() {
        // A quarter turn: the local x axis runs down the world y axis.
        let turn = Affine::rotate(std::f64::consts::FRAC_PI_2) * Affine::translate((0.0, 0.0));
        let (doc, res, line) = line_fixture(Affine::translate((40.0, 10.0)) * turn);
        let basis_before = doc.get(line).unwrap().transform().as_coeffs();

        for (which, target) in [
            (LineEnd::End, Point::new(90.0, 120.0)),
            (LineEnd::Start, Point::new(-10.0, 5.0)),
        ] {
            let tx = move_line_end(&doc, &res, line, which, target);
            let mut after = doc.clone();
            after.apply(&tx).expect("applies");
            let res_after = Resolved::rebuild(&after);
            let (start, end) = line_ends_world(&after, &res_after, line).unwrap();
            let moved = match which {
                LineEnd::Start => start,
                LineEnd::End => end,
            };
            approx_pt(moved, target, &format!("{which:?} landed on the pointer"));
            let basis = after.get(line).unwrap().transform().as_coeffs();
            for i in 0..4 {
                assert!(
                    (basis[i] - basis_before[i]).abs() < 1e-9,
                    "{which:?}: the basis changed — coefficient {i} went {} → {}",
                    basis_before[i],
                    basis[i]
                );
            }
        }
    }

    /// Root → line at `at`, running 100 along its own x axis.
    fn line_fixture(at: Affine) -> (Document, Resolved, NodeId) {
        let mut ids = IdSource::new(0x11D);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let line = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: line,
            parent: root,
            index: 0,
            kind: NodeKind::Line {
                end: Point::new(100.0, 0.0),
            },
            transform: Some(at),
            name: None,
        }]))
        .expect("a line");
        let res = Resolved::rebuild(&doc);
        (doc, res, line)
    }

    fn approx_pt(got: Point, want: Point, what: &str) {
        assert!(
            (got.x - want.x).abs() < 1e-6 && (got.y - want.y).abs() < 1e-6,
            "{what}: {got:?} vs {want:?}"
        );
    }
    /// A lone line and a lone pen path scale from a handle. The path's box is away from
    /// its origin, so the held edge stays put only with the `anchor·(1 − S)` correction.
    #[test]
    fn a_lone_line_and_path_scale_from_a_handle() {
        let mut ids = IdSource::new(0x11E);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (line, path) = (ids.mint(), ids.mint());
        let mut pen = BezPath::new();
        pen.move_to((10.0, 0.0));
        pen.line_to((30.0, 0.0));
        pen.line_to((30.0, 40.0));
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: line,
                parent: root,
                index: 0,
                kind: NodeKind::Line {
                    end: Point::new(100.0, 0.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: path,
                parent: root,
                index: 1,
                kind: NodeKind::Path {
                    path: pen,
                    corner_radii: Vec::new(),
                },
                transform: None,
                name: None,
            },
        ]))
        .expect("a line and a path");
        let res = Resolved::rebuild(&doc);
        let opts = Resize::default();

        let local = ondin_core::local_box(&doc, &res, line).expect("a line has a box");
        let tx = resize_geometry(
            &doc,
            &res,
            line,
            local,
            Handle::Right,
            Point::new(250.0, 0.0),
            opts,
        );
        assert!(!tx.0.is_empty(), "a line's handle drag must do something");
        let mut after = doc.clone();
        after.apply(&tx).expect("applies");
        let NodeKind::Line { end } = *after.get(line).unwrap().kind() else {
            panic!("still a line")
        };
        assert!(
            (end.x - 250.0).abs() < 1e-6 && end.y.abs() < 1e-6,
            "the endpoint follows the handle: {end:?}"
        );

        let local = ondin_core::local_box(&doc, &res, path).expect("a path has a box");
        assert_eq!(
            (local.x0, local.x1),
            (10.0, 30.0),
            "the box is off the origin"
        );
        let tx = resize_geometry(
            &doc,
            &res,
            path,
            local,
            Handle::Right,
            Point::new(50.0, 20.0),
            opts,
        );
        assert!(!tx.0.is_empty(), "a path's handle drag must do something");
        let mut after = doc.clone();
        after.apply(&tx).expect("applies");
        // Measured in world space: only geometry and transform together hold the edge.
        let res_after = Resolved::rebuild(&after);
        let world = res_after.world_transform(path).unwrap();
        let NodeKind::Path { path: shape, .. } = after.get(path).unwrap().kind() else {
            panic!("still a path")
        };
        let box_now = ondin_core::kurbo::Shape::bounding_box(&(world * shape.clone()));
        assert!(
            (box_now.x0 - 10.0).abs() < 1e-6,
            "the held left edge stayed at 10: {box_now:?}"
        );
        assert!(
            (box_now.x1 - 50.0).abs() < 1e-6,
            "and the dragged right edge reached 50: {box_now:?}"
        );
        assert!(
            (box_now.y1 - 40.0).abs() < 1e-6,
            "a side drag left the other axis alone: {box_now:?}"
        );
    }

    /// Resizing a group scales its children's positions and geometry, not its own
    /// transform (§5.6).
    #[test]
    fn resizing_a_group_scales_its_children_and_their_spacing() {
        let mut ids = IdSource::new(0x6E);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (ab, g, a, b) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 400.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: g,
                parent: ab,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: a,
                parent: g,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 20.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: b,
                parent: g,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 20.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((80.0, 0.0))),
                name: None,
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let local = ondin_core::local_box(&doc, &res, g).unwrap();
        assert_eq!((local.width(), local.height()), (100.0, 20.0));

        doc.apply(&resize_group(
            &doc,
            &res,
            g,
            local,
            Handle::BottomRight,
            Point::new(200.0, 20.0),
            Resize::geometry(false, false),
        ))
        .unwrap();

        let res = Resolved::rebuild(&doc);
        for (id, want_x, want_w) in [(a, 0.0, 40.0), (b, 160.0, 40.0)] {
            let NodeKind::Rect { size, .. } = doc.get(id).unwrap().kind() else {
                panic!();
            };
            assert!(
                (size.width - want_w).abs() < 1e-9 && (size.height - 20.0).abs() < 1e-9,
                "child geometry {size:?}, want {want_w}x20"
            );
            let origin = res.world_transform(id).unwrap() * Point::ZERO;
            assert!(
                (origin.x - want_x).abs() < 1e-9,
                "child at {origin:?}, want x {want_x}"
            );
        }
        let after = ondin_core::local_box(&doc, &res, g).unwrap();
        assert!((after.width() - 200.0).abs() < 1e-9, "{after:?}");
    }

    /// A group's own transform comes out untouched by a resize (§5.6).
    #[test]
    fn resizing_a_group_leaves_the_groups_own_transform_alone() {
        let mut ids = IdSource::new(0x77);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (ab, g, a) = (ids.mint(), ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 400.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: g,
                parent: ab,
                index: 0,
                kind: NodeKind::Group,
                transform: Some(Affine::translate((30.0, 40.0))),
                name: None,
            },
            Operation::CreateNode {
                id: a,
                parent: g,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(50.0, 50.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let before = doc.get(g).unwrap().transform().as_coeffs();
        let local = ondin_core::local_box(&doc, &res, g).unwrap();

        doc.apply(&resize_group(
            &doc,
            &res,
            g,
            local,
            Handle::BottomRight,
            Point::new(130.0, 140.0), // world → 100×100 in group space
            Resize::geometry(false, false),
        ))
        .unwrap();

        assert_eq!(doc.get(g).unwrap().transform().as_coeffs(), before);
        let NodeKind::Rect { size, .. } = doc.get(a).unwrap().kind() else {
            panic!();
        };
        assert!((size.width - 100.0).abs() < 1e-9, "{size:?}");
    }

    /// Dragging a side moves one axis and leaves the other, origin included, alone.
    #[test]
    fn a_side_handle_resizes_one_axis_and_leaves_the_other_untouched() {
        let size = Size::new(100.0, 60.0);
        for (handle, pointer, want_origin, want_size) in [
            (
                Handle::Right,
                Point::new(160.0, 12.0),
                Point::new(0.0, 0.0),
                Size::new(160.0, 60.0),
            ),
            (
                Handle::Left,
                Point::new(30.0, 12.0),
                Point::new(30.0, 0.0),
                Size::new(70.0, 60.0),
            ),
            (
                Handle::Bottom,
                Point::new(-40.0, 90.0),
                Point::new(0.0, 0.0),
                Size::new(100.0, 90.0),
            ),
            (
                Handle::Top,
                Point::new(-40.0, 20.0),
                Point::new(0.0, 20.0),
                Size::new(100.0, 40.0),
            ),
        ] {
            let (origin, got, _) = resized_box(size, handle, pointer, false, false);
            assert_eq!(got, want_size, "{handle:?} size");
            assert_eq!(origin, want_origin, "{handle:?} origin");
        }
    }

    /// A corner still drives both axes, and holds the opposite corner.
    #[test]
    fn a_corner_handle_still_drives_both_axes() {
        let size = Size::new(100.0, 60.0);
        let (origin, got, _) =
            resized_box(size, Handle::TopLeft, Point::new(20.0, 10.0), false, false);
        assert_eq!(got, Size::new(80.0, 50.0));
        assert_eq!(origin, Point::new(20.0, 10.0));
    }

    /// With Shift, a side drag carries the other axis with it.
    #[test]
    fn shift_on_a_side_scales_the_other_axis_too() {
        let (_, got, _) = resized_box(
            Size::new(100.0, 50.0),
            Handle::Right,
            Point::new(200.0, 0.0),
            true,
            false,
        );
        assert_eq!(got, Size::new(200.0, 100.0));
    }

    /// A moved pivot does not change what an Alt-resize does, asserted on the whole
    /// transaction.
    #[test]
    fn a_moved_pivot_does_not_change_an_alt_resize() {
        use ondin_core::Pivot;
        use ondin_core::kurbo::Vec2;

        let alt_resize = |doc: &Document, res: &Resolved, id: NodeId| {
            resize_to_handle(
                doc,
                res,
                id,
                Handle::Right,
                Point::new(85.0, 30.0),
                Resize::geometry(false, true),
            )
        };

        let (doc, res, rect) = artboard_with_rect();
        let plain = alt_resize(&doc, &res, rect);

        for pivot in [
            Pivot::Normalized(Vec2::new(1.0, 1.0)),
            Pivot::Normalized(Vec2::new(0.98, 0.5)),
            Pivot::Normalized(Vec2::new(0.0, 0.0)),
            Pivot::Local(Point::new(3.0, 3.0)),
        ] {
            let (mut doc, res, rect) = artboard_with_rect();
            doc.apply(&Transaction(vec![Operation::SetPivot {
                id: rect,
                pivot: Some(pivot),
            }]))
            .expect("set pivot");
            assert_eq!(
                alt_resize(&doc, &res, rect),
                plain,
                "{pivot:?} changed the resize"
            );
        }
    }

    /// Alt holds the centre still: dragging one side moves the opposite one out to
    /// match.
    #[test]
    fn alt_resizes_about_the_centre() {
        let size = Size::new(100.0, 50.0);
        let (origin, got, _) = resized_box(size, Handle::Right, Point::new(80.0, 0.0), false, true);
        assert_eq!(got, Size::new(60.0, 50.0));
        assert_eq!(origin, Point::new(20.0, 0.0));
        assert_eq!(origin.y, 0.0);

        // Without Alt the same drag holds the left edge.
        let (plain_origin, plain, _) =
            resized_box(size, Handle::Right, Point::new(80.0, 0.0), false, false);
        assert_eq!(plain, Size::new(80.0, 50.0));
        assert_eq!(plain_origin, Point::new(0.0, 0.0));

        let (o, s, _) = resized_box(
            size,
            Handle::BottomRight,
            Point::new(80.0, 40.0),
            false,
            true,
        );
        assert_eq!(s, Size::new(60.0, 30.0));
        assert_eq!(o, Point::new(20.0, 10.0));

        let (o, s, crossed) = resized_box(size, Handle::Right, Point::new(20.0, 0.0), false, true);
        assert_eq!(s, Size::new(60.0, 50.0));
        assert_eq!(o, Point::new(20.0, 0.0));
        assert_eq!(
            crossed,
            (true, false),
            "the pointer went past the centre on x and nowhere near it on y"
        );
    }
    /// Alt and Shift compose: proportional and centred.
    #[test]
    fn alt_and_shift_resize_proportionally_about_the_centre() {
        let size = Size::new(100.0, 50.0);
        let (origin, got, _) = resized_box(size, Handle::Right, Point::new(80.0, 0.0), true, true);
        assert_eq!(got, Size::new(60.0, 30.0));
        assert_eq!(origin, Point::new(20.0, 10.0));
    }

    /// Alt+Shift on a side handle centres the followed axis for a path too, as
    /// `resized_box` does; the interesting half is the y pair.
    /// flip: `resize_factors`' `pin` guards on `Some(_) if symmetric` → fails at the y assertion
    #[test]
    fn alt_and_shift_on_a_side_handle_centre_the_followed_axis_for_a_path_too() {
        let mut ids = IdSource::new(0x5B1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let path = ids.mint();
        let mut pen = BezPath::new();
        pen.move_to((0.0, 0.0));
        pen.line_to((100.0, 0.0));
        pen.line_to((100.0, 50.0));
        pen.line_to((0.0, 50.0));
        pen.close_path();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: path,
            parent: root,
            index: 0,
            kind: NodeKind::Path {
                path: pen,
                corner_radii: Vec::new(),
            },
            transform: None,
            name: None,
        }]))
        .expect("a path");
        let res = Resolved::rebuild(&doc);

        let before = ondin_core::local_box(&doc, &res, path).expect("a box");
        assert_eq!(before, Rect::new(0.0, 0.0, 100.0, 50.0));

        let tx = resize_layer(
            &doc,
            &res,
            path,
            Handle::Right,
            Point::new(80.0, 0.0),
            Resize::geometry(true, true),
        );
        let mut after = doc.clone();
        after.apply(&tx).expect("applies");
        let res_after = Resolved::rebuild(&after);
        let world = res_after.world_transform(path).unwrap();
        let NodeKind::Path { path: shape, .. } = after.get(path).unwrap().kind() else {
            panic!("still a path")
        };
        let now = ondin_core::kurbo::Shape::bounding_box(&(world * shape.clone()));

        assert!(
            (now.x0 - 20.0).abs() < 1e-9 && (now.x1 - 80.0).abs() < 1e-9,
            "the dragged axis is centred, as it always was: {now:?}"
        );
        assert!(
            (now.y0 - 10.0).abs() < 1e-9 && (now.y1 - 40.0).abs() < 1e-9,
            "and so is the axis the ratio lock drove: {now:?}"
        );
    }

    #[test]
    fn resize_never_goes_below_one_unit() {
        let (mut doc, res, rect) = artboard_with_rect();
        let tx = resize_to_handle(
            &doc,
            &res,
            rect,
            Handle::BottomRight,
            Point::new(100.0, 100.0),
            Resize::geometry(false, false),
        );
        doc.apply(&tx).unwrap();
        let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
            panic!();
        };
        assert!(size.width >= 1.0 && size.height >= 1.0);
    }

    #[test]
    fn shift_resize_keeps_the_aspect_ratio() {
        let (mut doc, res, rect) = artboard_with_rect(); // 50x50, ratio 1
        let tx = resize_to_handle(
            &doc,
            &res,
            rect,
            Handle::BottomRight,
            Point::new(300.0, 160.0),
            Resize::geometry(true, false),
        );
        doc.apply(&tx).unwrap();
        let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
            panic!();
        };
        assert!(
            (size.width - size.height).abs() < 1e-9,
            "expected a square, got {size:?}"
        );
    }

    /// A 400×400 artboard with one text node in `sizing` at its origin.
    fn artboard_with_text(sizing: TextSizing) -> (Document, Resolved, NodeId) {
        let mut ids = IdSource::new(9);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let ab = ids.mint();
        let t = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 400.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: t,
                parent: ab,
                index: 0,
                kind: NodeKind::Text {
                    content: "wrap me".into(),
                    style: Box::new(ondin_core::TextStyle {
                        font_family: "Inter".into(),
                        font_size: 16.0,
                        weight: 400,
                        italic: false,
                        line_height: Some(ondin_core::Length::Em(1.2)),
                        ..Default::default()
                    }),
                    spans: Default::default(),
                    para_spans: Default::default(),
                    paragraph: Default::default(),
                    block: Default::default(),
                    sizing,
                    on_path: None,
                    on_path_flip: false,
                    on_path_offset: 0.0,
                },
                transform: None,
                name: None,
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, t)
    }

    fn sizing_of(doc: &Document, id: NodeId) -> TextSizing {
        let NodeKind::Text { sizing, .. } = doc.get(id).unwrap().kind() else {
            panic!("not text");
        };
        *sizing
    }

    /// Dragging `handle` of the text node to `p`, and what its sizing became.
    fn drag_text(sizing: TextSizing, handle: Handle, p: Point) -> TextSizing {
        let (mut doc, res, t) = artboard_with_text(sizing);
        let tx = resize_to_handle(&doc, &res, t, handle, p, Resize::geometry(false, false));
        doc.apply(&tx).unwrap();
        sizing_of(&doc, t)
    }

    #[test]
    fn resizing_auto_text_from_a_corner_converts_it_to_a_fixed_box() {
        let sizing = drag_text(
            TextSizing::Auto,
            Handle::BottomRight,
            Point::new(80.0, 40.0),
        );
        let TextSizing::Fixed(size) = sizing else {
            panic!("expected a fixed box, got {sizing:?}");
        };
        assert!((size.width - 80.0).abs() < 1e-9 && (size.height - 40.0).abs() < 1e-9);
    }

    /// A side handle gives auto height, not a fixed box. The widths differ because the
    /// right edge holds the origin while the left holds the far side (`anchored_box`).
    #[test]
    fn dragging_a_side_of_auto_text_gives_auto_height() {
        let (doc, res, t) = artboard_with_text(TextSizing::Auto);
        let ink = ondin_core::local_box(&doc, &res, t).unwrap().width();
        for (handle, p, wanted) in [
            (Handle::Right, Point::new(80.0, 40.0), 80.0),
            (Handle::Left, Point::new(-30.0, 40.0), ink + 30.0),
        ] {
            let sizing = drag_text(TextSizing::Auto, handle, p);
            assert!(
                matches!(sizing, TextSizing::AutoHeight(w) if (w - wanted).abs() < 1e-9),
                "{handle:?} to {p:?} gave {sizing:?}, wanted auto-height at {wanted}"
            );
        }
    }

    /// The same drag on an already auto-height node keeps the mode (not a 1px-high box).
    #[test]
    fn dragging_a_side_of_auto_height_text_keeps_the_mode() {
        let sizing = drag_text(
            TextSizing::AutoHeight(50.0),
            Handle::Right,
            Point::new(120.0, 40.0),
        );
        assert!(
            matches!(sizing, TextSizing::AutoHeight(w) if (w - 120.0).abs() < 1e-9),
            "expected auto-height at 120, got {sizing:?}"
        );
    }

    /// The vertical handles fix an auto-height box, since the height becomes the user's.
    #[test]
    fn dragging_the_bottom_of_auto_height_text_fixes_the_box() {
        let sizing = drag_text(
            TextSizing::AutoHeight(50.0),
            Handle::Bottom,
            Point::new(0.0, 90.0),
        );
        assert!(
            matches!(sizing, TextSizing::Fixed(s) if (s.height - 90.0).abs() < 1e-9),
            "expected a 90-high fixed box, got {sizing:?}"
        );
    }

    /// Dragging a handle on railed text moves the ink: `TextSizing` is inert on a rail,
    /// so the assertion is on the box. Dragging the bottom-right corner to twice the
    /// width roughly doubles it and the held corner does not move (the rail arm writes
    /// the translation itself, so the `SetTransform` shift must not apply it twice).
    #[test]
    fn dragging_railed_text_scales_its_rail() {
        use ondin_core::kurbo::{BezPath, Shape};

        fn rail_box(doc: &Document, id: NodeId) -> Rect {
            let NodeKind::Text {
                on_path: Some(r), ..
            } = doc.get(id).unwrap().kind()
            else {
                panic!("not railed");
            };
            r.bounding_box()
        }

        let (mut doc, _, t) = artboard_with_text(TextSizing::Auto);
        let mut rail = BezPath::new();
        rail.move_to((0.0, 0.0));
        rail.curve_to((40.0, -20.0), (110.0, 20.0), (150.0, 0.0));
        doc.apply(&Transaction(vec![Operation::SetGeometry {
            id: t,
            geometry: GeometryPatch::TextPath(Some(rail)),
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let before = ondin_core::local_box(&doc, &res, t).expect("a railed box");
        let rail_before = rail_box(&doc, t);
        assert!(
            before.width() > 1.0,
            "the fixture must have a box: {before:?}"
        );

        let target = Point::new(
            before.x0 + before.width() * 2.0,
            before.y0 + before.height(),
        );
        let tx = resize_to_handle(
            &doc,
            &res,
            t,
            Handle::BottomRight,
            target,
            Resize::geometry(false, false),
        );
        doc.apply(&tx).unwrap();
        let res = Resolved::rebuild(&doc);
        let after = ondin_core::local_box(&doc, &res, t).expect("a railed box");
        let rail_after = rail_box(&doc, t);

        assert!(
            (after.width() - before.width() * 2.0).abs() < 0.5,
            "the box must land where the drag asked: {} -> {} (wanted {})",
            before.width(),
            after.width(),
            before.width() * 2.0
        );
        assert!(
            (after.x0 - before.x0).abs() < 0.01 && (after.y0 - before.y0).abs() < 0.01,
            "and the held corner must not move at all: {before:?} -> {after:?}"
        );
        // The drag owned `x` only; the residual is the solve's, not float noise.
        assert!(
            (rail_after.height() - rail_before.height()).abs() < 1e-3,
            "the rail must not move on the axis the drag held: {} -> {}",
            rail_before.height(),
            rail_after.height()
        );
        assert!(
            rail_after.width() > rail_before.width() * 1.5,
            "and it must actually have grown: {} -> {}",
            rail_before.width(),
            rail_after.width()
        );
    }

    /// Every handle holds its own corner on a rail, not just the `BottomRight` where
    /// the origin is the anchor. Each handle shrinks the box to a quarter, since the
    /// drift scales with distance from 1.
    /// flip: `railed_resize` anchors `want.origin() - got.origin()` → fails at `BottomLeft`
    #[test]
    fn every_handle_holds_its_own_corner_on_a_rail() {
        use ondin_core::kurbo::{BezPath, Vec2};

        for handle in [
            Handle::BottomRight,
            Handle::BottomLeft,
            Handle::TopRight,
            Handle::TopLeft,
        ] {
            let (mut doc, _, t) = artboard_with_text(TextSizing::Auto);
            let mut rail = BezPath::new();
            rail.move_to((0.0, 0.0));
            rail.curve_to((40.0, -20.0), (110.0, 20.0), (150.0, 0.0));
            doc.apply(&Transaction(vec![Operation::SetGeometry {
                id: t,
                geometry: GeometryPatch::TextPath(Some(rail)),
            }]))
            .unwrap();
            let res = Resolved::rebuild(&doc);
            let before = ondin_core::local_box(&doc, &res, t).expect("a railed box");

            let (w, h) = (before.width() * 0.25, before.height() * 0.25);
            let (hx, hy) = handle.anchored(before.width(), before.height());
            // The corner this handle holds, and the pointer for a quarter-size box.
            let held = before.origin() + Vec2::new(hx.unwrap_or(0.0), hy.unwrap_or(0.0));
            let pointer = Point::new(
                match hx {
                    Some(v) if v > 0.0 => held.x - w,
                    _ => held.x + w,
                },
                match hy {
                    Some(v) if v > 0.0 => held.y - h,
                    _ => held.y + h,
                },
            );

            let tx = resize_to_handle(
                &doc,
                &res,
                t,
                handle,
                pointer,
                Resize::geometry(false, false),
            );
            doc.apply(&tx).unwrap();
            let res = Resolved::rebuild(&doc);
            let after = ondin_core::local_box(&doc, &res, t).expect("a railed box");

            // The fixture must have shrunk, or "did not move" is vacuous.
            assert!(
                after.width() < before.width() * 0.5,
                "{handle:?}: the fixture must have shrunk: {} -> {}",
                before.width(),
                after.width()
            );

            let (ax, ay) = handle.anchored(after.width(), after.height());
            let held_after = after.origin() + Vec2::new(ax.unwrap_or(0.0), ay.unwrap_or(0.0));
            assert!(
                (held_after - held).hypot() < 0.05,
                "{handle:?}: the held corner slid {:.3} units — {held:?} -> {held_after:?}",
                (held_after - held).hypot()
            );
        }
    }

    /// The Scale tool on a rail holds the corner the Select tool holds: the solve must
    /// run against the type the transaction leaves, since `scale_scalars` also
    /// multiplies the font size. The anchor is asserted tightly, the extent loosely.
    /// flip: `kind_to_solve_against` returns `None` unconditionally → fails at the anchor assertion
    #[test]
    fn the_scale_tool_on_a_rail_holds_the_corner_the_select_tool_holds() {
        use ondin_core::kurbo::BezPath;

        let railed = || {
            let (mut doc, _, t) = artboard_with_text(TextSizing::Auto);
            let mut rail = BezPath::new();
            rail.move_to((0.0, 0.0));
            rail.curve_to((40.0, -20.0), (110.0, 20.0), (150.0, 0.0));
            doc.apply(&Transaction(vec![Operation::SetGeometry {
                id: t,
                geometry: GeometryPatch::TextPath(Some(rail)),
            }]))
            .unwrap();
            let res = Resolved::rebuild(&doc);
            (doc, res, t)
        };

        let run = |scaling: Scaling| {
            let (mut doc, res, t) = railed();
            let before = ondin_core::local_box(&doc, &res, t).expect("a railed box");
            let target = Point::new(
                before.x0 + before.width() * 2.0,
                before.y0 + before.height() * 2.0,
            );
            let tx = resize_to_handle(
                &doc,
                &res,
                t,
                Handle::BottomRight,
                target,
                Resize {
                    scaling,
                    ..Resize::geometry(false, false)
                },
            );
            doc.apply(&tx).unwrap();
            let res = Resolved::rebuild(&doc);
            let after = ondin_core::local_box(&doc, &res, t).expect("a railed box");
            (before, after)
        };

        let (before, select) = run(Scaling::Geometry);
        let (_, scale) = run(Scaling::Photographic);

        // The Select control lands the held corner exactly; Scale must match it.
        assert!(
            (select.x0 - before.x0).abs() < 0.01 && (select.y0 - before.y0).abs() < 0.01,
            "the Select control must hold the corner: {before:?} -> {select:?}"
        );
        assert!(
            (scale.x0 - before.x0).abs() < 0.01 && (scale.y0 - before.y0).abs() < 0.01,
            "the Scale tool must hold the same corner: {before:?} -> {scale:?}"
        );
        // The far corner is a target, so it is asserted loosely.
        let asked = before.height() * 2.0;
        assert!(
            (scale.height() - asked).abs() / asked < 0.05,
            "and the height must land near the drag: wanted {asked}, got {}",
            scale.height()
        );
    }

    /// Resizing a group that holds a railed text edits the rail too. The rect in the
    /// group is the control that the gesture reached the subtree; the child's growth is
    /// asserted loosely since the rail solve owns the exact width.
    /// flip: delete the `on_path` arm of `scale_geometry` → fails at the railed text's width
    #[test]
    fn resizing_a_group_carries_a_railed_text_with_it() {
        use ondin_core::kurbo::BezPath;

        let (mut doc, _, t) = artboard_with_text(TextSizing::Auto);
        let ab = doc.get(t).unwrap().parent().expect("the artboard");
        let mut ids = IdSource::new(0x9A11);
        let (group, rect) = (ids.mint(), ids.mint());
        let mut rail = BezPath::new();
        rail.move_to((0.0, 0.0));
        rail.curve_to((40.0, -20.0), (110.0, 20.0), (150.0, 0.0));
        doc.apply(&Transaction(vec![
            Operation::SetGeometry {
                id: t,
                geometry: GeometryPatch::TextPath(Some(rail)),
            },
            Operation::CreateNode {
                id: group,
                parent: ab,
                index: 1,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: rect,
                parent: group,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(80.0, 40.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((0.0, 120.0))),
                name: None,
            },
            Operation::Reparent {
                id: t,
                new_parent: group,
                index: 0,
            },
        ]))
        .expect("group a railed text with a rect");

        let res = Resolved::rebuild(&doc);
        let g0 = ondin_core::local_box(&doc, &res, group).expect("the group has a box");
        let t0 = ondin_core::local_box(&doc, &res, t).expect("the text has a box");
        let r0 = ondin_core::local_box(&doc, &res, rect).expect("the rect has a box");

        let world = res.world_transform(group).expect("world");
        let target = world * Point::new(g0.x0 + g0.width() * 2.0, g0.y0 + g0.height() * 2.0);
        let tx = resize_group(
            &doc,
            &res,
            group,
            g0,
            Handle::BottomRight,
            target,
            Resize::geometry(false, false),
        );
        doc.apply(&tx).expect("the group resize applies");
        let res = Resolved::rebuild(&doc);
        let t1 = ondin_core::local_box(&doc, &res, t).expect("still a box");
        let r1 = ondin_core::local_box(&doc, &res, rect).expect("still a box");

        assert!(
            (r1.width() - r0.width() * 2.0).abs() < 0.5,
            "control: the plain rect must double, {} -> {}",
            r0.width(),
            r1.width()
        );
        assert!(
            t1.width() > t0.width() * 1.5,
            "the railed text must grow with its sibling rather than stand still: \
             {} -> {} (unchanged is the defect)",
            t0.width(),
            t1.width()
        );
    }

    /// A fixed box is not un-fixed by a side drag: an authored height stays authored.
    #[test]
    fn dragging_a_side_of_fixed_text_keeps_it_fixed() {
        let sizing = drag_text(
            TextSizing::Fixed(Size::new(60.0, 30.0)),
            Handle::Right,
            Point::new(150.0, 0.0),
        );
        assert!(
            matches!(sizing, TextSizing::Fixed(s)
                if (s.width - 150.0).abs() < 1e-9 && (s.height - 30.0).abs() < 1e-9),
            "expected a 150×30 fixed box, got {sizing:?}"
        );
    }

    #[test]
    fn create_rect_lands_at_world_drag_box_under_translated_parent() {
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let ab = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
            },
            transform: Some(Affine::translate((50.0, 20.0))),
            name: None,
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let parent_world = res.world_transform(ab).unwrap();

        let rect = ids.mint();
        let tx = create_rect(
            rect,
            ab,
            0,
            parent_world,
            Point::new(100.0, 100.0),
            Point::new(160.0, 140.0),
            Color::from_rgba8(1, 2, 3, 255),
        );
        doc.apply(&tx).unwrap();
        let res = Resolved::rebuild(&doc);

        let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
            panic!("expected a rect");
        };
        assert!((size.width - 60.0).abs() < 1e-9);
        assert!((size.height - 40.0).abs() < 1e-9);
        let origin = res.world_transform(rect).unwrap() * Point::ZERO;
        assert!((origin.x - 100.0).abs() < 1e-9, "x={}", origin.x);
        assert!((origin.y - 100.0).abs() < 1e-9, "y={}", origin.y);
        assert!(doc.get(rect).unwrap().paint().fills.len() == 1);
    }

    /// A frame drawn inside a moved frame becomes its child and lands where it was
    /// drawn (§5.3), which needs the parent's world transform.
    #[test]
    fn a_frame_drawn_inside_a_moved_frame_lands_where_it_was_drawn() {
        let mut ids = IdSource::new(0x5EED);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let page = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: page,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
            },
            transform: Some(Affine::translate((50.0, 20.0))),
            name: None,
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let parent_world = res.world_transform(page).unwrap();

        let card = ids.mint();
        let tx = create_artboard(
            card,
            page,
            0,
            parent_world,
            Point::new(100.0, 100.0),
            Point::new(160.0, 140.0),
            Color::WHITE,
        );
        doc.apply(&tx).expect("a frame nests inside a frame");
        let res = Resolved::rebuild(&doc);

        assert_eq!(doc.get(card).unwrap().parent(), Some(page));
        let NodeKind::Artboard { size, .. } = doc.get(card).unwrap().kind() else {
            panic!("expected a frame");
        };
        assert!((size.width - 60.0).abs() < 1e-9);
        assert!((size.height - 40.0).abs() < 1e-9);
        let origin = res.world_transform(card).unwrap() * Point::ZERO;
        assert!((origin.x - 100.0).abs() < 1e-9, "x={}", origin.x);
        assert!((origin.y - 100.0).abs() < 1e-9, "y={}", origin.y);
    }

    /// A dragged text box is `Fixed` at what was drawn and lands where it was drawn
    /// under a moved parent; the click plants `TextSizing::Auto` instead.
    #[test]
    fn a_dragged_text_box_is_fixed_at_the_drawn_size() {
        let mut ids = IdSource::new(0x7E87);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let page = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: page,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
            },
            transform: Some(Affine::translate((50.0, 20.0))),
            name: None,
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let parent_world = res.world_transform(page).unwrap();

        let parts = ondin_core::TextParts {
            style: ondin_core::TextStyle {
                font_family: "Inter".into(),
                font_size: 16.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let t = ids.mint();
        let tx = create_text(
            t,
            page,
            0,
            parent_world,
            Point::new(100.0, 100.0),
            Point::new(260.0, 180.0),
            &parts,
        );
        doc.apply(&tx).expect("a text box nests inside a frame");
        let res = Resolved::rebuild(&doc);

        assert_eq!(doc.get(t).unwrap().parent(), Some(page));
        let NodeKind::Text {
            content, sizing, ..
        } = doc.get(t).unwrap().kind()
        else {
            panic!("expected a text node");
        };
        assert!(content.is_empty(), "created empty, to be typed into");
        let ondin_core::TextSizing::Fixed(size) = sizing else {
            panic!("a drag asked for a width, so the mode is Fixed: {sizing:?}");
        };
        assert!((size.width - 160.0).abs() < 1e-9, "w={}", size.width);
        assert!((size.height - 80.0).abs() < 1e-9, "h={}", size.height);
        let origin = res.world_transform(t).unwrap() * Point::ZERO;
        assert!((origin.x - 100.0).abs() < 1e-9, "x={}", origin.x);
        assert!((origin.y - 100.0).abs() < 1e-9, "y={}", origin.y);
    }

    /// A thin drag still gets a box one line tall; the width has no floor.
    #[test]
    fn a_thin_text_drag_still_clears_one_line() {
        let mut ids = IdSource::new(0x7E88);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let parts = ondin_core::TextParts {
            style: ondin_core::TextStyle {
                font_family: "Inter".into(),
                font_size: 40.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let t = ids.mint();
        doc.apply(&create_text(
            t,
            root,
            0,
            Affine::IDENTITY,
            Point::new(10.0, 10.0),
            // Two units tall, well under one 40pt line.
            Point::new(38.0, 12.0),
            &parts,
        ))
        .unwrap();
        let NodeKind::Text { sizing, .. } = doc.get(t).unwrap().kind() else {
            panic!("expected a text node");
        };
        let ondin_core::TextSizing::Fixed(size) = sizing else {
            panic!("{sizing:?}");
        };
        // "At least the type size", so this does not pin a metric.
        assert!(
            size.height >= 40.0,
            "a 2-unit drag should still hold a line of 40pt type, got {}",
            size.height
        );
        assert!(
            (size.width - 28.0).abs() < 1e-9,
            "and the width is left exactly as drawn: {}",
            size.width
        );
    }

    /// Text typed into a dragged box wraps to the drawn width, asserted on the shaped
    /// layout; the box does not grow to fit, and `TextOverflow::Visible` lets text
    /// hang out the bottom.
    #[test]
    fn a_dragged_text_box_wraps_what_is_typed_into_it() {
        let mut ids = IdSource::new(0x7E89);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let parts = ondin_core::TextParts {
            style: ondin_core::TextStyle {
                font_family: "Inter".into(),
                font_size: 16.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let t = ids.mint();
        doc.apply(&create_text(
            t,
            root,
            0,
            Affine::IDENTITY,
            Point::new(0.0, 0.0),
            Point::new(90.0, 200.0),
            &parts,
        ))
        .unwrap();

        let kind = doc.get(t).unwrap().kind();
        let node_parts = ondin_core::TextParts::of(kind).expect("a text node");
        let sentence = "wrap this sentence over several lines";
        let mut edit = ondin_core::TextEdit::new("", node_parts.clone());
        edit.insert(sentence);
        let laid = edit.text_layout();

        let mut auto = node_parts;
        auto.sizing = ondin_core::TextSizing::Auto;
        let unwrapped = ondin_core::text::layout(auto.as_ref(sentence));
        assert!(
            unwrapped.size.width > 150.0,
            "the fixture has to be wider than the box to prove anything: {}",
            unwrapped.size.width
        );
        assert!(
            (laid.size.width - 90.0).abs() < 1e-9,
            "the box keeps the drawn width: {}",
            laid.size.width
        );
        assert!(
            (laid.size.height - 200.0).abs() < 1e-9,
            "and the drawn height, rather than growing to the text: {}",
            laid.size.height
        );
        // Distinct baselines is the wrap, without pinning how many lines Inter takes.
        let mut baselines: Vec<i32> = laid
            .runs
            .iter()
            .filter_map(|r| r.glyphs.first().map(|g| (g.y * 100.0) as i32))
            .collect();
        baselines.sort_unstable();
        baselines.dedup();
        assert!(
            baselines.len() > 1,
            "a 90-unit box must wrap this sentence: {baselines:?}"
        );
    }

    /// `drags_a_box` is a `matches!` list the compiler does not check against intent:
    /// Text is in it, and the selecting and editing tools are not.
    #[test]
    fn text_both_drags_a_box_and_stays_a_click_tool() {
        assert!(Tool::Text.drags_a_box());
        for tool in [
            Tool::Frame,
            Tool::Rect,
            Tool::Ellipse,
            Tool::Polygon,
            Tool::Star,
            Tool::Line,
        ] {
            assert!(tool.drags_a_box(), "{tool:?}");
        }
        // This predicate also decides which tools clear the selection when armed;
        // `Node` and `ImageEdit` are entered from a selection, so a wrong answer would
        // empty the tool as it opened.
        for tool in [
            Tool::Pen,
            Tool::Node,
            Tool::ImageEdit,
            Tool::Select,
            Tool::Scale,
            Tool::Hand,
        ] {
            assert!(!tool.drags_a_box(), "{tool:?}");
        }
    }

    /// The whole click table, tool by tool, and the containment that anything
    /// answering a click also drags a box. The `match` in `click_makes` makes a missing
    /// arm a build error; this makes a wrong arm a failure.
    #[test]
    fn every_tool_answers_a_click_with_exactly_what_it_should() {
        use ClickCreate::*;
        for (tool, want) in [
            (Tool::Rect, Some(DefaultBox)),
            (Tool::Ellipse, Some(DefaultBox)),
            (Tool::Polygon, Some(DefaultBox)),
            (Tool::Star, Some(DefaultBox)),
            (Tool::Image, Some(FittedImage)),
            (Tool::Text, Some(TextNode)),
            // Box tools deliberately without a click answer, then non-creation tools.
            (Tool::Frame, None),
            (Tool::Line, None),
            (Tool::Select, None),
            (Tool::Scale, None),
            (Tool::Pen, None),
            (Tool::Node, None),
            (Tool::ImageEdit, None),
            (Tool::Hand, None),
        ] {
            assert_eq!(tool.click_makes(), want, "{tool:?}");
            if want.is_some() {
                assert!(
                    tool.drags_a_box(),
                    "{tool:?} answers a click but cannot be dragged out, so it has \
                     only one way to be given a size"
                );
            }
        }
    }

    #[test]
    fn create_shape_box_is_order_independent() {
        let pw = Affine::IDENTITY;
        let (o1, s1) = local_box(pw, Point::new(10.0, 10.0), Point::new(50.0, 30.0));
        let (o2, s2) = local_box(pw, Point::new(50.0, 30.0), Point::new(10.0, 10.0));
        assert_eq!(o1, o2);
        assert_eq!(s1, s2);
        assert_eq!(o1, Point::new(10.0, 10.0));
        assert_eq!(s1, Size::new(40.0, 20.0));
    }

    #[test]
    fn create_path_points_map_back_to_world() {
        use ondin_core::kurbo::PathEl;
        let mut ids = IdSource::new(4);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let ab = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
            },
            transform: Some(Affine::translate((12.0, 34.0))),
            name: None,
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let pw = res.world_transform(ab).unwrap();

        let path = ids.mint();
        let world_pts = [
            Point::new(50.0, 60.0),
            Point::new(120.0, 60.0),
            Point::new(120.0, 140.0),
        ];
        let anchors: Vec<PenAnchor> = world_pts.iter().copied().map(PenAnchor::corner).collect();
        doc.apply(&create_path(
            path,
            ab,
            0,
            pw,
            &anchors,
            false,
            Color::BLACK,
            2.0,
        ))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let w = res.world_transform(path).unwrap();
        let NodeKind::Path { path: bez, .. } = doc.get(path).unwrap().kind() else {
            panic!("expected a path");
        };
        let local_pts: Vec<Point> = bez
            .elements()
            .iter()
            .filter_map(|el| match el {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => Some(*p),
                _ => None,
            })
            .collect();
        assert_eq!(local_pts.len(), 3);
        for (local, expected) in local_pts.iter().zip(world_pts.iter()) {
            let got = w * *local;
            assert!(
                (got.x - expected.x).abs() < 1e-9 && (got.y - expected.y).abs() < 1e-9,
                "{got:?} vs {expected:?}"
            );
        }
    }

    /// The close snap radius is screen pt, so the world tolerance shrinks as the view
    /// zooms in; comparing world distance directly would fail only the zoom case.
    #[test]
    fn the_pen_snaps_closed_within_a_screen_radius_not_a_world_one() {
        const R: f64 = 5.0;
        let square = [
            PenAnchor::corner(Point::new(0.0, 0.0)),
            PenAnchor::corner(Point::new(100.0, 0.0)),
            PenAnchor::corner(Point::new(100.0, 100.0)),
        ];

        assert_eq!(
            pen_close_snap(&square, Point::new(4.0, 0.0), 1.0, R),
            Some(Point::new(0.0, 0.0))
        );
        assert_eq!(pen_close_snap(&square, Point::new(40.0, 0.0), 1.0, R), None);

        // At 4x the same 4 units is 16 screen pt away.
        assert_eq!(
            pen_close_snap(&square, Point::new(4.0, 0.0), 4.0, R),
            None,
            "the radius is screen pt: zooming in must tighten the world tolerance"
        );
        assert_eq!(
            pen_close_snap(&square, Point::new(4.0, 0.0), 0.5, R),
            Some(Point::new(0.0, 0.0))
        );

        // `create_path` refuses a one-anchor path, so it must not offer a close.
        let lone = [PenAnchor::corner(Point::new(0.0, 0.0))];
        assert_eq!(pen_close_snap(&lone, Point::new(0.0, 0.0), 1.0, R), None);
        assert_eq!(pen_close_snap(&[], Point::new(0.0, 0.0), 1.0, R), None);

        assert_eq!(
            pen_close_snap(&square, Point::new(100.0, 0.0), 1.0, R),
            None,
            "the second anchor is not a close target"
        );
    }

    /// A pen anchor with handles produces a curve and one without produces a line,
    /// so a clicked-out path stays a polyline.
    #[test]
    fn pen_anchors_curve_only_where_they_have_handles() {
        use ondin_core::kurbo::{PathEl, Vec2};
        let corner = |x, y| PenAnchor::corner(Point::new(x, y));
        let smooth = |x, y, hx, hy| PenAnchor::smooth(Point::new(x, y), Vec2::new(hx, hy));

        let straight = pen_outline(&[corner(0.0, 0.0), corner(10.0, 0.0)], false);
        assert!(
            matches!(straight.elements()[1], PathEl::LineTo(_)),
            "two corners are a line: {:?}",
            straight.elements()
        );

        let curved = pen_outline(&[smooth(0.0, 0.0, 5.0, 5.0), corner(10.0, 0.0)], false);
        let PathEl::CurveTo(c1, c2, end) = curved.elements()[1] else {
            panic!("a handled anchor should curve: {:?}", curved.elements());
        };
        assert_eq!(c1, Point::new(5.0, 5.0), "leaves along the handle");
        assert_eq!(c2, Point::new(10.0, 0.0), "a corner has no incoming handle");
        assert_eq!(end, Point::new(10.0, 0.0));

        let closed = pen_outline(
            &[corner(0.0, 0.0), corner(10.0, 0.0), corner(10.0, 10.0)],
            true,
        );
        assert!(
            matches!(closed.elements().last(), Some(PathEl::ClosePath)),
            "{:?}",
            closed.elements()
        );
    }

    /// A broken handle pair arrives straight and leaves on a curve, which a mirrored
    /// anchor cannot say.
    #[test]
    fn an_anchor_can_break_its_handles_and_bend_only_one_side() {
        use ondin_core::kurbo::{PathEl, Vec2};

        let broken = PenAnchor {
            at: Point::new(10.0, 0.0),
            out: Vec2::new(5.0, 5.0),
            back: Vec2::ZERO,
            radius: 0.0,
        };
        let path = pen_outline(
            &[
                PenAnchor::corner(Point::new(0.0, 0.0)),
                broken,
                PenAnchor::corner(Point::new(20.0, 0.0)),
            ],
            false,
        );

        // Into the broken anchor there is no incoming handle, so the segment is straight.
        assert_eq!(
            path.elements()[1],
            PathEl::LineTo(Point::new(10.0, 0.0)),
            "the incoming side was broken, so the segment into it must not bend: {:?}",
            path.elements()
        );
        let PathEl::CurveTo(c1, _, _) = path.elements()[2] else {
            panic!(
                "out of a handled anchor should curve: {:?}",
                path.elements()
            );
        };
        assert_eq!(c1, Point::new(15.0, 5.0), "leaves along `out`");

        // Mirrored, the same anchor bends both ways.
        let mirrored = pen_outline(
            &[
                PenAnchor::corner(Point::new(0.0, 0.0)),
                PenAnchor::smooth(Point::new(10.0, 0.0), Vec2::new(5.0, 5.0)),
                PenAnchor::corner(Point::new(20.0, 0.0)),
            ],
            false,
        );
        let PathEl::CurveTo(_, c2m, _) = mirrored.elements()[1] else {
            panic!("{:?}", mirrored.elements());
        };
        assert_eq!(
            c2m,
            Point::new(5.0, -5.0),
            "mirrored, the incoming control point is the reflection of `out`"
        );
    }

    /// `pull_within` mirrors the handle pair, breaks it under Alt, re-mirrors when Alt is
    /// released mid-drag, and retracts a handle inside the radius so a snapped click stays a corner.
    #[test]
    fn pulling_a_handle_mirrors_unless_the_pair_is_broken() {
        use ondin_core::kurbo::Vec2;
        let at = Point::new(10.0, 10.0);
        let pull = |a: &mut PenAnchor, to: Point, alt: bool| a.pull_within(Side::Out, to, alt, 0.0);

        let mut a = PenAnchor::corner(at);
        pull(&mut a, Point::new(15.0, 10.0), false);
        assert_eq!(a.out, Vec2::new(5.0, 0.0));
        assert_eq!(a.back, Vec2::new(-5.0, 0.0), "mirrored");

        // Alt: the incoming handle stays exactly where the mirror left it.
        pull(&mut a, Point::new(10.0, 20.0), true);
        assert_eq!(a.out, Vec2::new(0.0, 10.0));
        assert_eq!(
            a.back,
            Vec2::new(-5.0, 0.0),
            "a broken pair leaves the incoming handle alone"
        );

        // Releasing Alt re-mirrors, rather than leaving the anchor broken for good.
        pull(&mut a, Point::new(10.0, 20.0), false);
        assert_eq!(a.back, Vec2::new(0.0, -10.0));

        a.pull_within(Side::Out, Point::new(12.0, 11.0), false, 7.0);
        assert!(a.is_corner(), "out={:?} back={:?}", a.out, a.back);
        a.pull_within(Side::Out, Point::new(30.0, 10.0), false, 7.0);
        assert_eq!(a.out, Vec2::new(20.0, 0.0));
    }
    /// A corner needs both handles to be zero; an incoming-only anchor would straighten a curve.
    #[test]
    fn an_anchor_with_only_an_incoming_handle_is_not_a_corner() {
        use ondin_core::kurbo::Vec2;
        let incoming_only = PenAnchor {
            at: Point::new(1.0, 1.0),
            out: Vec2::ZERO,
            back: Vec2::new(-3.0, 0.0),
            radius: 0.0,
        };
        assert!(!incoming_only.is_corner());
        assert!(PenAnchor::corner(Point::new(1.0, 1.0)).is_corner());
    }

    /// `create_path` keeps a curve's control points in step with its anchors through the
    /// local-space conversion.
    #[test]
    fn a_curved_pen_path_keeps_its_control_points_in_world_space() {
        use ondin_core::kurbo::{PathEl, Vec2};
        let mut ids = IdSource::new(11);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let ab = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
            },
            transform: Some(Affine::translate((17.0, 23.0))),
            name: None,
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let pw = res.world_transform(ab).unwrap();

        let anchors = [
            PenAnchor::smooth(Point::new(50.0, 60.0), Vec2::new(20.0, -20.0)),
            PenAnchor::corner(Point::new(150.0, 60.0)),
        ];
        let want = pen_outline(&anchors, false);

        let id = ids.mint();
        doc.apply(&create_path(
            id,
            ab,
            0,
            pw,
            &anchors,
            false,
            Color::BLACK,
            2.0,
        ))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let world = res.world_transform(id).unwrap();
        let NodeKind::Path { path, .. } = doc.get(id).unwrap().kind() else {
            panic!("expected a path");
        };

        let mut back = path.clone();
        back.apply_affine(world);
        let (a, b) = (want.elements(), back.elements());
        assert_eq!(a.len(), b.len(), "{a:?} vs {b:?}");
        for (x, y) in a.iter().zip(b.iter()) {
            let pts = |el: &PathEl| match *el {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => vec![p],
                PathEl::QuadTo(c, p) => vec![c, p],
                PathEl::CurveTo(c1, c2, p) => vec![c1, c2, p],
                PathEl::ClosePath => vec![],
            };
            let (px, py) = (pts(x), pts(y));
            assert_eq!(px.len(), py.len(), "{x:?} vs {y:?}");
            for (p, q) in px.iter().zip(py.iter()) {
                assert!(
                    (p.x - q.x).abs() < 1e-9 && (p.y - q.y).abs() < 1e-9,
                    "{p:?} vs {q:?}"
                );
            }
        }
    }

    /// Points of a path element, for comparing two paths element by element.
    fn el_points(el: &PathEl) -> Vec<Point> {
        match *el {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => vec![p],
            PathEl::QuadTo(c, p) => vec![c, p],
            PathEl::CurveTo(c1, c2, p) => vec![c1, c2, p],
            PathEl::ClosePath => vec![],
        }
    }

    /// Two paths describe the same curve, element for element.
    fn assert_same_path(a: &BezPath, b: &BezPath) {
        let (ea, eb) = (a.elements(), b.elements());
        assert_eq!(
            ea.len(),
            eb.len(),
            "different element counts:\n{ea:#?}\nvs\n{eb:#?}"
        );
        for (x, y) in ea.iter().zip(eb.iter()) {
            assert_eq!(
                std::mem::discriminant(x),
                std::mem::discriminant(y),
                "{x:?} vs {y:?}"
            );
            for (p, q) in el_points(x).iter().zip(el_points(y).iter()) {
                assert!(
                    (p.x - q.x).abs() < 1e-9 && (p.y - q.y).abs() < 1e-9,
                    "{x:?} vs {y:?}"
                );
            }
        }
    }

    /// `pen_anchors` inverts `pen_outline`; a closed path's repeated final point must not
    /// read as an extra anchor, hence the anchor count.
    #[test]
    fn reading_a_pen_path_back_gives_the_anchors_that_drew_it() {
        use ondin_core::kurbo::Vec2;

        let anchors = vec![
            PenAnchor::corner(Point::new(0.0, 0.0)),
            PenAnchor::smooth(Point::new(40.0, 30.0), Vec2::new(10.0, 0.0)),
            // A broken pair: arrives straight, leaves on a curve.
            PenAnchor {
                at: Point::new(80.0, 0.0),
                out: Vec2::new(0.0, -12.0),
                back: Vec2::ZERO,
                radius: 0.0,
            },
            PenAnchor::corner(Point::new(120.0, 20.0)),
        ];

        for closed in [false, true] {
            let drawn = pen_outline(&anchors, closed);
            let read = pen_anchors(&drawn, &[]);
            assert_eq!(read.len(), 1, "one `MoveTo` is one subpath");
            assert_eq!(read[0].closed, closed);
            assert_eq!(
                read[0].anchors.len(),
                anchors.len(),
                "closed={closed}: the closing segment is not an anchor of its own"
            );
            for (want, got) in anchors.iter().zip(read[0].anchors.iter()) {
                assert_eq!((want.at, want.out, want.back), (got.at, got.out, got.back));
            }
            assert_same_path(&drawn, &pen_path(&read));
        }
    }

    /// The reader keeps every subpath of a general path and elevates quads to the same
    /// curve as cubics, the way kurbo does.
    #[test]
    fn reading_a_general_path_elevates_quads_and_keeps_every_subpath() {
        use ondin_core::kurbo::QuadBez;

        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((10.0, 0.0));
        path.quad_to((20.0, 10.0), (30.0, 0.0));
        path.move_to((100.0, 0.0));
        path.line_to((140.0, 0.0));
        path.line_to((140.0, 40.0));
        path.close_path();

        let read = pen_anchors(&path, &[]);
        assert_eq!(read.len(), 2, "two `MoveTo`s are two subpaths");
        assert!(!read[0].closed);
        assert_eq!(read[0].anchors.len(), 3);
        assert!(read[1].closed);
        assert_eq!(
            read[1].anchors.len(),
            3,
            "an implied closing edge adds no anchor"
        );

        let raised = QuadBez::new((10.0, 0.0), (20.0, 10.0), (30.0, 0.0)).raise();
        let (a, b) = (&read[0].anchors[1], &read[0].anchors[2]);
        assert_eq!(a.leaving(), raised.p1);
        assert_eq!(b.arriving(), raised.p2);
        assert!(a.back.hypot() < 1e-9, "a line arrived at it");

        let out = pen_path(&read);
        let mut want = BezPath::new();
        want.move_to((0.0, 0.0));
        want.line_to((10.0, 0.0));
        want.curve_to(raised.p1, raised.p2, raised.p3);
        want.move_to((100.0, 0.0));
        want.line_to((140.0, 0.0));
        want.line_to((140.0, 40.0));
        want.line_to((100.0, 0.0));
        want.close_path();
        assert_same_path(&out, &want);
    }

    /// Reversing a subpath swaps its handles so the same curve is drawn backwards.
    #[test]
    fn reversing_a_subpath_draws_the_same_curve_backwards() {
        use ondin_core::kurbo::Vec2;

        let mut sub = PenSubpath {
            anchors: vec![
                PenAnchor::smooth(Point::new(0.0, 0.0), Vec2::new(10.0, 10.0)),
                PenAnchor::smooth(Point::new(60.0, 0.0), Vec2::new(10.0, -10.0)),
            ],
            closed: false,
        };
        let forward = pen_path(std::slice::from_ref(&sub));
        sub.reverse();
        let back = pen_path(std::slice::from_ref(&sub));

        let mut want = BezPath::new();
        want.move_to((60.0, 0.0));
        want.curve_to((50.0, 10.0), (10.0, 10.0), (0.0, 0.0));
        assert_same_path(&back, &want);

        // Reversal is an involution.
        sub.reverse();
        assert_same_path(&pen_path(std::slice::from_ref(&sub)), &forward);
    }

    /// `rewrite_path` goes back through the node's own world transform, so extending a
    /// path's start leaves its transform and its other subpaths alone.
    #[test]
    fn rewriting_a_path_leaves_its_transform_and_its_other_subpaths_alone() {
        let mut ids = IdSource::new(21);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let ab = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
            },
            transform: Some(Affine::translate((17.0, 23.0))),
            name: None,
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let pw = res.world_transform(ab).unwrap();

        let drawn = [
            PenAnchor::corner(Point::new(50.0, 60.0)),
            PenAnchor::corner(Point::new(150.0, 60.0)),
        ];
        let id = ids.mint();
        doc.apply(&create_path(
            id,
            ab,
            0,
            pw,
            &drawn,
            false,
            Color::BLACK,
            2.0,
        ))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let before = doc.get(id).unwrap().transform();
        let node_world = res.world_transform(id).unwrap();

        let NodeKind::Path { path, .. } = doc.get(id).unwrap().kind() else {
            panic!("expected a path");
        };
        let current = path.clone();
        let mut subpaths = pen_anchors(&current, &[]);
        map_subpaths(&mut subpaths, node_world);
        subpaths.push(PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(200.0, 200.0)),
                PenAnchor::corner(Point::new(260.0, 200.0)),
            ],
            closed: true,
        });

        // Resuming and changing nothing is not an edit, even after a reverse round trip.
        let mut untouched = subpaths[0].clone();
        untouched.reverse();
        untouched.reverse();
        let idle = PenSubpath {
            anchors: untouched.anchors,
            closed: false,
        };
        let mut idle_all = subpaths.clone();
        idle_all[0] = idle;
        assert!(
            rewrite_path(id, node_world, &idle_all[..1], Some((&current, &[])))
                .0
                .is_empty(),
            "a resume that changed nothing must not become an undo step"
        );

        let mut working = subpaths[0].clone();
        working.reverse();
        working
            .anchors
            .push(PenAnchor::corner(Point::new(50.0, 140.0)));
        working.reverse();
        subpaths[0] = working;

        doc.apply(&rewrite_path(
            id,
            node_world,
            &subpaths,
            Some((&current, &[])),
        ))
        .unwrap();
        let res = Resolved::rebuild(&doc);

        assert_eq!(
            doc.get(id).unwrap().transform(),
            before,
            "extending the start of a path must not move the node"
        );
        let NodeKind::Path { path, .. } = doc.get(id).unwrap().kind() else {
            panic!("expected a path");
        };
        let mut world = path.clone();
        world.apply_affine(res.world_transform(id).unwrap());
        let read = pen_anchors(&world, &[]);
        assert_eq!(read.len(), 2, "the untouched subpath is still there");
        let ends: Vec<Point> = read[0].anchors.iter().map(|a| a.at).collect();
        assert_eq!(
            ends,
            vec![
                Point::new(50.0, 140.0),
                Point::new(50.0, 60.0),
                Point::new(150.0, 60.0)
            ],
            "extending the start prepends, leaving the path's direction alone"
        );
        assert!(read[1].closed);
        assert_eq!(read[1].anchors[0].at, Point::new(200.0, 200.0));
    }

    /// Moving a point carries its handle offsets with it, so the curve leaves it the same way.
    #[test]
    fn moving_a_point_carries_its_handles_and_leaves_its_neighbours_shaped() {
        use ondin_core::kurbo::Vec2;
        let mut subs = vec![PenSubpath {
            anchors: vec![
                PenAnchor::smooth(Point::new(0.0, 0.0), Vec2::new(10.0, 0.0)),
                PenAnchor::smooth(Point::new(50.0, 0.0), Vec2::new(10.0, 10.0)),
                PenAnchor::smooth(Point::new(100.0, 0.0), Vec2::new(10.0, 0.0)),
            ],
            closed: false,
        }];
        let before = subs[0].anchors[0].leaving();

        move_points(&mut subs, &[(0, 1)], Vec2::new(5.0, -20.0));

        let moved = subs[0].anchors[1];
        assert_eq!(moved.at, Point::new(55.0, -20.0));
        assert_eq!(
            (moved.out, moved.back),
            (Vec2::new(10.0, 10.0), Vec2::new(-10.0, -10.0)),
            "the handles are offsets, so the point keeps its shape"
        );
        assert_eq!(
            moved.leaving(),
            Point::new(65.0, -10.0),
            "and the control points travelled with it"
        );
        assert_eq!(
            subs[0].anchors[0].leaving(),
            before,
            "the neighbour did not move, so its own handle must not have"
        );

        // A stale index is ignored rather than panicking.
        move_points(&mut subs, &[(9, 9), (0, 9)], Vec2::new(1.0, 1.0));
    }

    /// Deleting an anchor heals the path through the neighbours' own handles and drops a
    /// subpath left with fewer than two anchors.
    #[test]
    fn deleting_a_point_heals_the_path_and_drops_a_subpath_that_cannot_draw() {
        use ondin_core::kurbo::Vec2;
        let subs = vec![
            PenSubpath {
                anchors: vec![
                    PenAnchor::corner(Point::new(0.0, 0.0)),
                    PenAnchor::smooth(Point::new(50.0, 40.0), Vec2::new(10.0, 0.0)),
                    PenAnchor::smooth(Point::new(100.0, 0.0), Vec2::new(0.0, -10.0)),
                    PenAnchor::corner(Point::new(150.0, 0.0)),
                ],
                closed: false,
            },
            // Two anchors: removing one leaves a point, which draws nothing.
            PenSubpath {
                anchors: vec![
                    PenAnchor::corner(Point::new(200.0, 0.0)),
                    PenAnchor::corner(Point::new(240.0, 0.0)),
                ],
                closed: false,
            },
        ];

        let left = remove_points(&subs, &[(0, 1), (1, 0)]);
        assert_eq!(left.len(), 1, "the two-anchor subpath is gone, not emptied");
        assert_eq!(subpath_lengths(&left), vec![3]);

        let healed = pen_path(&left);
        let PathEl::CurveTo(c1, c2, end) = healed.elements()[1] else {
            panic!("the heal should curve: {:?}", healed.elements());
        };
        assert_eq!(c1, Point::new(0.0, 0.0), "a corner leaves straight");
        assert_eq!(
            c2,
            Point::new(100.0, 10.0),
            "into the survivor's own handle"
        );
        assert_eq!(end, Point::new(100.0, 0.0));

        assert!(remove_points(&subs, &[(0, 0), (0, 1), (0, 2), (0, 3), (1, 0), (1, 1)]).is_empty());
    }

    /// Dragging a handle within the retract radius snaps it onto its anchor, since
    /// `is_corner` needs exact zeros; the mirror collapses the other handle too.
    #[test]
    fn a_handle_dragged_near_its_anchor_retracts_and_makes_a_corner() {
        use ondin_core::kurbo::Vec2;
        let at = Point::new(50.0, 50.0);
        let mut subs = vec![PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(0.0, 0.0)),
                PenAnchor::smooth(at, Vec2::new(20.0, 0.0)),
                PenAnchor::corner(Point::new(100.0, 0.0)),
            ],
            closed: false,
        }];

        // Just outside the radius: still smooth.
        pull_handle(
            &mut subs,
            (0, 1),
            Side::Out,
            at + Vec2::new(8.0, 0.0),
            false,
            7.0,
            0.0,
        );
        assert!(!subs[0].anchors[1].is_corner());
        assert_eq!(subs[0].anchors[1].out, Vec2::new(8.0, 0.0));

        pull_handle(
            &mut subs,
            (0, 1),
            Side::Out,
            at + Vec2::new(5.0, 2.0),
            false,
            7.0,
            0.0,
        );
        assert!(subs[0].anchors[1].is_corner());
        let path = pen_path(&subs);
        assert!(
            path.elements()[1..]
                .iter()
                .all(|el| matches!(el, PathEl::LineTo(_))),
            "a retracted anchor between two corners leaves two lines: {:?}",
            path.elements()
        );

        // Alt breaks the pair, so a one-sided retract stays possible.
        let mut broken = vec![PenSubpath {
            anchors: vec![PenAnchor::smooth(at, Vec2::new(20.0, 0.0))],
            closed: false,
        }];
        pull_handle(&mut broken, (0, 0), Side::Out, at, true, 7.0, 0.0);
        assert_eq!(broken[0].anchors[0].out, Vec2::ZERO);
        assert_eq!(
            broken[0].anchors[0].back,
            Vec2::new(-20.0, 0.0),
            "the incoming handle is untouched"
        );
        assert!(!broken[0].anchors[0].is_corner());
    }

    /// Align and distribute over points use the extent the points themselves occupy.
    #[test]
    fn aligning_and_distributing_points_works_on_their_own_extent() {
        use ondin_core::kurbo::Vec2;
        let mut subs = vec![PenSubpath {
            anchors: vec![
                PenAnchor::smooth(Point::new(0.0, 10.0), Vec2::new(5.0, 5.0)),
                PenAnchor::corner(Point::new(30.0, 40.0)),
                PenAnchor::corner(Point::new(90.0, 70.0)),
                // Not selected, so nothing below may touch it.
                PenAnchor::corner(Point::new(45.0, 200.0)),
            ],
            closed: false,
        }];
        let picked = [(0, 0), (0, 1), (0, 2)];

        align_points(&mut subs, &picked, Axis::Y, Edge::Min);
        let ys: Vec<f64> = subs[0].anchors.iter().map(|a| a.at.y).collect();
        assert_eq!(
            ys,
            vec![10.0, 10.0, 10.0, 200.0],
            "to the topmost, and only the three"
        );
        assert_eq!(
            subs[0].anchors[0].leaving(),
            Point::new(5.0, 15.0),
            "the handles travelled with the anchor"
        );

        // X is untouched by a Y align, so distribute has something to space.
        distribute_points(&mut subs, &picked, Axis::X);
        let xs: Vec<f64> = subs[0].anchors.iter().map(|a| a.at.x).collect();
        assert_eq!(
            xs,
            vec![0.0, 45.0, 90.0, 45.0],
            "evenly between the two extremes, which stay put"
        );

        let before: Vec<Point> = subs[0].anchors.iter().map(|a| a.at).collect();
        distribute_points(&mut subs, &picked[..2], Axis::X);
        align_points(&mut subs, &picked[..1], Axis::X, Edge::Max);
        assert_eq!(
            subs[0].anchors.iter().map(|a| a.at).collect::<Vec<_>>(),
            before
        );
    }

    /// `rotate_points` carries each handle through the transform via its control point, so
    /// a zero-length handle stays exactly zero; a quarter turn about an off-origin pivot.
    ///
    /// flip: `rotate_points` reduced to `a.at = t * a.at` → fails at "the handle turned with its anchor"
    #[test]
    fn rotating_points_carries_their_handles_round_with_them() {
        use ondin_core::kurbo::Vec2;
        let mut subs = vec![PenSubpath {
            anchors: vec![
                PenAnchor::smooth(Point::new(20.0, 10.0), Vec2::new(5.0, 0.0)),
                PenAnchor::corner(Point::new(30.0, 10.0)),
                // Not selected.
                PenAnchor::corner(Point::new(50.0, 50.0)),
            ],
            closed: false,
        }];
        let picked = [(0, 0), (0, 1)];
        let pivot = Point::new(10.0, 10.0);

        rotate_points(&mut subs, &picked, pivot, std::f64::consts::FRAC_PI_2);

        let near = |a: Point, b: Point| (a - b).hypot() < 1e-9;
        assert!(
            near(subs[0].anchors[0].at, Point::new(10.0, 20.0)),
            "the anchor turned about the pivot: {:?}",
            subs[0].anchors[0].at
        );
        assert!(
            near(subs[0].anchors[0].leaving(), Point::new(10.0, 25.0)),
            "the handle turned with its anchor: {:?}",
            subs[0].anchors[0].out
        );
        assert!(
            (subs[0].anchors[0].out.hypot() - 5.0).abs() < 1e-9,
            "and kept its length: {}",
            subs[0].anchors[0].out.hypot()
        );
        assert_eq!(
            (subs[0].anchors[1].out, subs[0].anchors[1].back),
            (Vec2::ZERO, Vec2::ZERO),
            "a zero-length handle must stay at zero rather than at a rounding error"
        );
        assert_eq!(
            subs[0].anchors[2].at,
            Point::new(50.0, 50.0),
            "and the unselected anchor did not move"
        );
    }

    /// `space_points` lays the run out from the first point on the axis, whatever order
    /// the selection was clicked in (0 / 10 / 40 at gap 5 gives 0 / 5 / 10).
    ///
    /// flip: `order` reversed → fails at the "leftmost stays" assertion
    #[test]
    fn spacing_points_holds_the_first_one_along_the_axis() {
        let mut subs = vec![PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(0.0, 0.0)),
                PenAnchor::corner(Point::new(10.0, 0.0)),
                PenAnchor::corner(Point::new(40.0, 0.0)),
                // Not selected.
                PenAnchor::corner(Point::new(90.0, 0.0)),
            ],
            closed: false,
        }];
        // Scrambled on purpose: the axis decides the order.
        let picked = [(0, 2), (0, 0), (0, 1)];

        space_points(&mut subs, &picked, Axis::X, 5.0);

        let xs: Vec<f64> = subs[0].anchors.iter().map(|a| a.at.x).collect();
        assert_eq!(
            xs,
            vec![0.0, 5.0, 10.0, 90.0],
            "the leftmost stays and the rest come to it, five apart"
        );

        // Fewer than two points, or a NaN gap, are no-ops.
        let before = xs;
        space_points(&mut subs, &picked[..1], Axis::X, 5.0);
        space_points(&mut subs, &picked, Axis::X, f64::NAN);
        assert_eq!(
            subs[0].anchors.iter().map(|a| a.at.x).collect::<Vec<_>>(),
            before
        );
    }

    /// Making a corner smooth mirrors the longer handle of a shaped anchor, or derives an
    /// exactly mirrored pair from the chord `next − prev` at a third of the shorter leg.
    #[test]
    fn making_a_point_smooth_mirrors_a_handle_or_derives_one_from_the_neighbours() {
        use ondin_core::kurbo::Vec2;
        // Legs of different lengths (10 across, 20 down) expose a handle taken from the wrong one.
        let mut subs = vec![PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(0.0, 0.0)),
                PenAnchor::corner(Point::new(10.0, 0.0)),
                PenAnchor::corner(Point::new(10.0, 20.0)),
            ],
            closed: false,
        }];
        set_smoothness(&mut subs, &[(0, 1)], true);
        let mid = subs[0].anchors[1];
        assert!(
            mid.is_smooth(),
            "the pair has to mirror *exactly*, or the next drag breaks it: {mid:?}"
        );
        let dir = Vec2::new(10.0, 20.0) / Vec2::new(10.0, 20.0).hypot();
        let want_out = dir * (10.0 / 3.0);
        assert!(
            (mid.out - want_out).hypot() < 1e-9,
            "out {:?} wanted {want_out:?}",
            mid.out
        );
        assert_eq!(subs[0].anchors[0].out, Vec2::ZERO);
        assert_eq!(subs[0].anchors[2].out, Vec2::ZERO);

        let mut broken = vec![PenSubpath {
            anchors: vec![PenAnchor {
                at: Point::ZERO,
                out: Vec2::new(9.0, 0.0),
                back: Vec2::new(0.0, 2.0),
                radius: 4.0,
            }],
            closed: false,
        }];
        set_smoothness(&mut broken, &[(0, 0)], true);
        let a = broken[0].anchors[0];
        assert_eq!(a.out, Vec2::new(9.0, 0.0), "the longer handle survives");
        assert_eq!(a.back, Vec2::new(-9.0, 0.0), "and the other mirrors it");
        assert_eq!(
            a.radius, 4.0,
            "the radius is left alone — it already draws nothing on a smooth anchor"
        );

        set_smoothness(&mut broken, &[(0, 0)], false);
        let a = broken[0].anchors[0];
        assert_eq!((a.out, a.back), (Vec2::ZERO, Vec2::ZERO));
        assert_eq!(a.radius, 4.0);
    }

    /// An open subpath's endpoint gets one handle and is not `is_smooth()`; closing the
    /// subpath gives it two. Both `smoothed_handles` branches are driven (derived and
    /// already-shaped), each end separately.
    ///
    /// flip: delete either one-sided case of the already-shaped arm → fails at its own end's assertion
    #[test]
    fn an_open_subpaths_endpoint_smooths_on_the_side_it_has() {
        use ondin_core::kurbo::Vec2;
        let anchors = vec![
            PenAnchor::corner(Point::new(0.0, 0.0)),
            PenAnchor::corner(Point::new(30.0, 0.0)),
            PenAnchor::corner(Point::new(30.0, 30.0)),
        ];
        let mut open = vec![PenSubpath {
            anchors: anchors.clone(),
            closed: false,
        }];
        set_smoothness(&mut open, &[(0, 0)], true);
        let end = open[0].anchors[0];
        assert_eq!(end.back, Vec2::ZERO, "there is no segment behind it");
        assert_eq!(end.out, Vec2::new(10.0, 0.0), "a third of its one leg");
        assert!(!end.is_smooth(), "and an end is not a joint");

        let mut closed = vec![PenSubpath {
            anchors,
            closed: true,
        }];
        set_smoothness(&mut closed, &[(0, 0)], true);
        let joint = closed[0].anchors[0];
        assert!(
            joint.is_smooth() && joint.back.hypot() > 0.0,
            "a closed subpath has no ends: {joint:?}"
        );

        // A lone anchor is left as it was.
        let mut lone = vec![PenSubpath {
            anchors: vec![PenAnchor::corner(Point::new(5.0, 5.0))],
            closed: false,
        }];
        set_smoothness(&mut lone, &[(0, 0)], true);
        assert_eq!(lone[0].anchors[0].at, Point::new(5.0, 5.0));
        assert_eq!(lone[0].anchors[0].out, Vec2::ZERO);

        // Endpoints that already carry a handle take the already-shaped branch.
        let mut shaped = vec![PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(0.0, 0.0)),
                PenAnchor::corner(Point::new(30.0, 0.0)),
            ],
            closed: false,
        }];
        shaped[0].anchors[0].out = Vec2::new(10.0, 0.0);
        assert_eq!(shaped[0].anchors[0].back, Vec2::ZERO);
        set_smoothness(&mut shaped, &[(0, 0)], true);
        let end = shaped[0].anchors[0];
        assert_eq!(
            end.out,
            Vec2::new(10.0, 0.0),
            "the handle that was aimed on purpose is kept"
        );
        assert_eq!(
            end.back,
            Vec2::ZERO,
            "and no second one is invented over a segment that does not exist"
        );
        assert!(!end.is_smooth(), "an end is still not a joint");

        // The other end, where the missing side is `out`.
        let mut tail = vec![PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(0.0, 0.0)),
                PenAnchor::corner(Point::new(30.0, 0.0)),
            ],
            closed: false,
        }];
        tail[0].anchors[1].back = Vec2::new(-10.0, 0.0);
        assert_eq!(tail[0].anchors[1].out, Vec2::ZERO);
        set_smoothness(&mut tail, &[(0, 1)], true);
        let end = tail[0].anchors[1];
        assert_eq!((end.back, end.out), (Vec2::new(-10.0, 0.0), Vec2::ZERO));
        assert!(!end.is_smooth());
    }

    /// `pen_anchors` and `geometry::anchor_runs` number a path's anchors and segments alike,
    /// since `corner_radii` is written by one and read by the other.
    #[test]
    fn the_app_and_core_number_a_paths_anchors_alike() {
        let mut cases: Vec<BezPath> = Vec::new();
        let mut a = BezPath::new();
        a.move_to((0.0, 0.0));
        a.line_to((10.0, 0.0));
        a.curve_to((15.0, 5.0), (15.0, 5.0), (10.0, 10.0));
        a.line_to((0.0, 0.0));
        a.close_path();
        cases.push(a);
        // A `ClosePath` with an implied final edge.
        let mut b = BezPath::new();
        b.move_to((0.0, 0.0));
        b.line_to((10.0, 0.0));
        b.line_to((10.0, 10.0));
        b.close_path();
        cases.push(b);
        // Several subpaths, a quad, and an open run.
        let mut c = BezPath::new();
        c.move_to((0.0, 0.0));
        c.quad_to((5.0, 5.0), (10.0, 0.0));
        c.move_to((50.0, 50.0));
        c.line_to((60.0, 50.0));
        c.line_to((60.0, 60.0));
        c.close_path();
        cases.push(c);
        cases.push(BezPath::new());

        for path in &cases {
            let subs = pen_anchors(path, &[]);
            let runs = ondin_core::geometry::anchor_runs(path);
            let mine: Vec<Point> = subs
                .iter()
                .flat_map(|s| s.anchors.iter().map(|a| a.at))
                .collect();
            let theirs: Vec<Point> = runs.iter().flat_map(|r| r.points.clone()).collect();
            assert_eq!(mine, theirs, "for {:?}", path.elements());
            assert_eq!(mine.len(), ondin_core::geometry::anchor_count(path));

            // Segments are named by the anchor they leave: one per anchor when
            // closed, one fewer when open.
            assert_eq!(subs.len(), runs.len(), "for {:?}", path.elements());
            for (i, (sub, run)) in subs.iter().zip(runs.iter()).enumerate() {
                let want = match sub.closed {
                    true => sub.anchors.len(),
                    false => sub.anchors.len().saturating_sub(1),
                };
                assert_eq!(run.segs.len(), want, "subpath {i} segment count");
                for j in 0..run.segs.len() {
                    let (from, to) =
                        segment_ends(&subs, (i, j)).unwrap_or_else(|| panic!("segment {i}.{j}"));
                    assert_eq!(from, (i, j), "a segment is named by the anchor it leaves");
                    let seg = run.segs[j];
                    assert_eq!(seg.start(), sub.anchors[from.1].at, "subpath {i} seg {j}");
                    assert_eq!(seg.end(), sub.anchors[to.1].at, "subpath {i} seg {j}");
                }
                // One past the last segment names nothing.
                assert!(
                    segment_ends(&subs, (i, run.segs.len())).is_none(),
                    "subpath {i} claims a segment it does not have"
                );
            }
        }

        let mut subs = pen_anchors(&cases[0], &[0.0, 4.0, 7.0]);
        assert_eq!(subs[0].anchors[1].radius, 4.0);
        assert_eq!(subs[0].anchors[2].radius, 7.0);
        assert_eq!(pen_radii(&subs), vec![0.0, 4.0, 7.0]);
        // Trailing zeros are trimmed so equal shapes serialize alike.
        subs[0].anchors[1].radius = 0.0;
        subs[0].anchors[2].radius = 0.0;
        assert!(pen_radii(&subs).is_empty());
    }

    #[test]
    fn create_line_endpoints_map_to_world() {
        let mut ids = IdSource::new(3);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let ab = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
            },
            transform: Some(Affine::translate((5.0, 7.0))),
            name: None,
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let pw = res.world_transform(ab).unwrap();

        let line = ids.mint();
        let (a, b) = (Point::new(20.0, 30.0), Point::new(120.0, 80.0));
        doc.apply(&create_line(line, ab, 0, pw, a, b, Color::BLACK, 2.0))
            .unwrap();
        let res = Resolved::rebuild(&doc);
        let world = res.world_transform(line).unwrap();
        let NodeKind::Line { end } = doc.get(line).unwrap().kind() else {
            panic!("expected a line");
        };
        let w_start = world * Point::ZERO;
        let w_end = world * *end;
        assert!((w_start.x - a.x).abs() < 1e-9 && (w_start.y - a.y).abs() < 1e-9);
        assert!((w_end.x - b.x).abs() < 1e-9 && (w_end.y - b.y).abs() < 1e-9);
    }

    #[test]
    fn rotate_about_pivot_moves_points_correctly() {
        let (mut doc, res, rect) = artboard_with_rect();
        let tx = rotate_node(
            &doc,
            &res,
            rect,
            Point::new(100.0, 100.0),
            std::f64::consts::FRAC_PI_2,
        );
        doc.apply(&tx).unwrap();
        let res2 = Resolved::rebuild(&doc);
        let w = res2.world_transform(rect).unwrap();
        let p = w * Point::new(50.0, 0.0);
        assert!((p.x - 100.0).abs() < 1e-6, "x={}", p.x);
        assert!((p.y - 150.0).abs() < 1e-6, "y={}", p.y);
    }

    /// A rotation under a skewed parent stores no scale in the transform; the world
    /// placement is asserted as a control, and an unskewed parent as the second half.
    ///
    /// flip: `rotate_node` writes `new_local` without the scale split → fails at "a rotation under a skewed parent stores no scale"
    #[test]
    fn a_rotation_under_a_skewed_parent_leaves_the_scale_in_the_geometry() {
        let scale_after = |lean: f64| {
            let mut ids = IdSource::new(0x5CE);
            let root = ids.mint();
            let mut doc = Document::new(root);
            let (group, rect) = (ids.mint(), ids.mint());
            doc.apply(&Transaction(vec![
                Operation::CreateNode {
                    id: group,
                    parent: root,
                    index: 0,
                    kind: NodeKind::Group,
                    // A 45° lean at 1.0, none at 0.0.
                    transform: Some(Affine::new([1.0, 0.0, lean, 1.0, 0.0, 0.0])),
                    name: None,
                },
                Operation::CreateNode {
                    id: rect,
                    parent: group,
                    index: 0,
                    kind: NodeKind::Rect {
                        size: Size::new(100.0, 100.0),
                        corner_radii: RoundedRectRadii::default(),
                        corner_smoothing: 0.0,
                    },
                    transform: None,
                    name: None,
                },
            ]))
            .expect("the fixture");
            let res = Resolved::rebuild(&doc);
            let before = res.world_transform(rect).expect("a world basis");
            let probe = before * Point::new(100.0, 0.0);

            let pivot = Point::new(50.0, 50.0);
            let angle = std::f64::consts::FRAC_PI_6;
            let tx = rotate_node(&doc, &res, rect, pivot, angle);
            doc.apply(&tx).expect("the rotation applies");
            let res = Resolved::rebuild(&doc);

            // Read the corner through the node's own `size`: the split moves scale
            // into the geometry, so the same local coordinate names a different place.
            let node = doc.get(rect).expect("a live node");
            let after = res.world_transform(rect).expect("a world basis");
            let NodeKind::Rect { size, .. } = node.kind() else {
                panic!("a rect");
            };
            let want = turn_about(pivot, angle) * probe;
            let got = after * Point::new(size.width, 0.0);
            (Basis::of(node.transform()).scale, (got - want).hypot())
        };

        let (skewed, moved) = scale_after(1.0);
        assert!(
            moved < 1e-6,
            "the control: the rotation still puts the point where it promised, \
             off by {moved}"
        );
        assert!(
            (skewed.x - 1.0).abs() < 1e-6 && (skewed.y - 1.0).abs() < 1e-6,
            "a rotation under a skewed parent stores no scale: {skewed:?}"
        );

        let (upright, moved) = scale_after(0.0);
        assert!(moved < 1e-6, "the upright control's placement: {moved}");
        assert!(
            (upright.x - 1.0).abs() < 1e-6 && (upright.y - 1.0).abs() < 1e-6,
            "and neither does one under an unskewed parent — which is the arm that \
             always worked: {upright:?}"
        );
    }

    // --- transforming a whole selection ------------------------------------

    /// Two sibling 20×20 squares at x = 0 and x = 80, whose union is 100×20.
    fn two_squares() -> (Document, Resolved, NodeId, NodeId) {
        let mut ids = IdSource::new(0x5E1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (ab, a, b) = (ids.mint(), ids.mint(), ids.mint());
        let square = || NodeKind::Rect {
            size: Size::new(20.0, 20.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        };
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 400.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: a,
                parent: ab,
                index: 0,
                kind: square(),
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: b,
                parent: ab,
                index: 1,
                kind: square(),
                transform: Some(Affine::translate((80.0, 0.0))),
                name: None,
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, a, b)
    }

    fn union_of(res: &Resolved, ids: &[NodeId]) -> Rect {
        ids.iter()
            .filter_map(|id| res.world_bounds(*id))
            .reduce(|x, y| x.union(y))
            .unwrap()
    }

    /// A selection resize scales both the gaps between members and each member's geometry.
    #[test]
    fn resizing_a_selection_scales_its_members_and_the_gaps_between_them() {
        let (mut doc, res, a, b) = two_squares();
        let roots = [a, b];
        let union = union_of(&res, &roots);
        assert_eq!((union.width(), union.height()), (100.0, 20.0));

        doc.apply(&resize_selection(
            &doc,
            &res,
            &roots,
            SelectionBox::upright(union),
            Handle::BottomRight,
            Point::new(200.0, 20.0),
            Resize::geometry(false, false),
        ))
        .unwrap();

        let res = Resolved::rebuild(&doc);
        for (id, want_x, want_w) in [(a, 0.0, 40.0), (b, 160.0, 40.0)] {
            let NodeKind::Rect { size, .. } = doc.get(id).unwrap().kind() else {
                panic!("not a rect");
            };
            assert!(
                (size.width - want_w).abs() < 1e-9 && (size.height - 20.0).abs() < 1e-9,
                "{id:?} geometry {size:?}, want {want_w}x20"
            );
            let origin = res.world_transform(id).unwrap() * Point::ZERO;
            assert!(
                (origin.x - want_x).abs() < 1e-9,
                "{id:?} at {origin:?}, want x {want_x}"
            );
        }
        let after = union_of(&res, &roots);
        assert!(
            (after.width() - 200.0).abs() < 1e-9 && (after.height() - 20.0).abs() < 1e-9,
            "the union box came out {after:?}"
        );
    }

    /// A selection resize holds the opposite corner still.
    #[test]
    fn resizing_a_selection_holds_the_opposite_corner_still() {
        let (mut doc, res, a, b) = two_squares();
        let roots = [a, b];
        let union = union_of(&res, &roots);
        doc.apply(&resize_selection(
            &doc,
            &res,
            &roots,
            SelectionBox::upright(union),
            Handle::BottomRight,
            Point::new(50.0, 10.0),
            Resize::geometry(false, false),
        ))
        .unwrap();
        let after = union_of(&Resolved::rebuild(&doc), &roots);
        assert!(
            (after.min_x() - union.min_x()).abs() < 1e-9
                && (after.min_y() - union.min_y()).abs() < 1e-9,
            "the held corner moved: {union:?} -> {after:?}"
        );
    }

    /// A selection turns about the shared centre, not each member's own.
    #[test]
    fn rotating_a_selection_turns_it_about_the_shared_centre() {
        let (mut doc, res, a, b) = two_squares();
        let roots = [a, b];
        let union = union_of(&res, &roots);
        let pivot = union.center();

        doc.apply(&rotate_selection(
            &doc,
            &res,
            &roots,
            pivot,
            std::f64::consts::FRAC_PI_2,
        ))
        .unwrap();

        let res = Resolved::rebuild(&doc);
        let after = union_of(&res, &roots);
        assert!(
            (after.width() - 20.0).abs() < 1e-9 && (after.height() - 100.0).abs() < 1e-9,
            "the box did not turn: {after:?}"
        );
        assert!(
            (after.center().x - pivot.x).abs() < 1e-9 && (after.center().y - pivot.y).abs() < 1e-9,
            "the selection drifted off its pivot: {:?} -> {:?}",
            pivot,
            after.center()
        );
        for id in roots {
            let NodeKind::Rect { size, .. } = doc.get(id).unwrap().kind() else {
                panic!("not a rect");
            };
            assert_eq!((size.width, size.height), (20.0, 20.0));
        }
        let (ca, cb) = (
            res.world_bounds(a).unwrap().center(),
            res.world_bounds(b).unwrap().center(),
        );
        assert!(
            (ca.y - cb.y).abs() > 50.0,
            "the two squares did not separate along y: {ca:?} {cb:?}"
        );
    }

    /// A member at an odd angle under a non-uniform selection scale leans, and the selection
    /// lands exactly where the box says; asserted on world bounds, not node fields.
    #[test]
    fn a_selection_member_at_an_odd_angle_leans_instead_of_evening_out() {
        let (mut doc, _, a, b) = two_squares();
        doc.apply(&Transaction(vec![Operation::SetTransform {
            id: b,
            transform: Affine::translate((80.0, 0.0)) * Affine::rotate(30f64.to_radians()),
        }]))
        .unwrap();
        let res2 = Resolved::rebuild(&doc);
        let roots = [a, b];
        let union = union_of(&res2, &roots);
        let before = res2.world_bounds(b).unwrap();

        doc.apply(&resize_selection(
            &doc,
            &res2,
            &roots,
            SelectionBox::upright(union),
            Handle::BottomRight,
            Point::new(union.max_x() * 3.0, union.max_y()),
            Resize::geometry(false, false),
        ))
        .unwrap();
        let after = Resolved::rebuild(&doc);

        let hold = Point::new(union.min_x(), union.min_y());
        let want = Rect::new(
            hold.x + (before.min_x() - hold.x) * 3.0,
            before.min_y(),
            hold.x + (before.max_x() - hold.x) * 3.0,
            before.max_y(),
        );
        let got = after.world_bounds(b).unwrap();
        for (g, w) in [
            (got.min_x(), want.min_x()),
            (got.min_y(), want.min_y()),
            (got.max_x(), want.max_x()),
            (got.max_y(), want.max_y()),
        ] {
            assert!(
                (g - w).abs() < 1e-6,
                "the turned member did not follow the box: {got:?} vs {want:?}"
            );
        }

        // It leans rather than stretching along its own axes.
        let skew = build::Orientation::of(after.world_transform(b).unwrap()).skew;
        assert!(
            skew.abs() > 1f64.to_radians(),
            "the turned member came out unsheared: {}°",
            skew.to_degrees()
        );

        // The axis-aligned member took the scale as pure geometry.
        let NodeKind::Rect { size: flat, .. } = doc.get(a).unwrap().kind() else {
            panic!("not a rect");
        };
        assert!(
            flat.width > flat.height * 1.5,
            "the axis-aligned member should have stretched: {flat:?}"
        );
        assert!(
            build::Orientation::of(after.world_transform(a).unwrap())
                .skew
                .abs()
                < 1e-9,
            "an axis-aligned member must not pick up a lean"
        );
    }

    /// Ctrl-dragging a side leans the box and holds the side facing it still.
    #[test]
    fn skewing_a_side_holds_the_one_facing_it() {
        let (mut doc, res, a, _) = two_squares();
        let before = res.world_bounds(a).unwrap();

        doc.apply(&skew_to_handle(
            &doc,
            &res,
            a,
            Rect::new(0.0, 0.0, 20.0, 20.0),
            Handle::Top,
            SideDrag {
                press: Point::new(before.center().x, before.min_y()),
                at: Point::new(before.center().x + 20.0, before.min_y()),
            },
            false,
        ))
        .unwrap();
        let after = Resolved::rebuild(&doc);

        // −45, not 45: `skewX` displaces +x with +y and y points down, matching the
        // inspector's Skew field and the SVG writer.
        let o = build::Orientation::of(after.world_transform(a).unwrap());
        assert!(
            (o.skew.to_degrees() + 45.0).abs() < 1e-6,
            "a top side dragged one height right is a −45° lean, not {}°",
            o.skew.to_degrees()
        );
        let got = after.world_bounds(a).unwrap();
        assert!(
            (got.max_y() - before.max_y()).abs() < 1e-6,
            "the held side moved: {before:?} -> {got:?}"
        );
        assert!(
            got.width() > before.width() + 10.0,
            "the box did not lean at all: {before:?} -> {got:?}"
        );
    }

    /// A side grabbed off its midpoint leans by the drag delta from the press, not by the
    /// grab's offset from the box centre.
    ///
    /// flip: measure from the centre (`p.x - c.x`) instead of `p.x - press.x` → fails at the no-lean assertion
    #[test]
    fn a_side_grabbed_off_centre_does_not_lean_until_the_pointer_moves() {
        let mut ids = IdSource::new(7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(200.0, 120.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("the reported rectangle");
        let res = Resolved::rebuild(&doc);
        let box_ = Rect::new(0.0, 0.0, 200.0, 120.0);
        let press = Point::new(160.5, 0.0);

        let lean_after = |dx: f64| {
            let tx = skew_to_handle(
                &doc,
                &res,
                id,
                box_,
                Handle::Top,
                SideDrag {
                    press,
                    at: Point::new(press.x + dx, press.y),
                },
                false,
            );
            let mut d = doc.clone();
            if !tx.0.is_empty() {
                d.apply(&tx).expect("the skew applies");
            }
            let r = Resolved::rebuild(&d);
            build::Orientation::of(r.world_transform(id).unwrap())
                .skew
                .to_degrees()
        };

        assert!(
            lean_after(0.0).abs() < 1e-9,
            "pressing without moving leant it {}° — this is the reported jump",
            lean_after(0.0)
        );

        let small = lean_after(1.0);
        let want = (1.0f64 / -120.0).atan().to_degrees();
        assert!(
            (small - want).abs() < 1e-6,
            "a 1-unit drag should lean {want}°, got {small}°"
        );

        let full = lean_after(60.5);
        assert!(
            (full + 26.76).abs() < 0.01,
            "dragging the old grab-offset should still reach the reported −26.76°, \
             got {full}° — the angle is not gone, it just takes a drag now"
        );
    }

    /// A selection leans as one parallelogram even with a turned member; compared on each
    /// member's unit-square-to-world map, since the split divides scale between matrix and size.
    #[test]
    fn a_leaning_selection_carries_a_turned_member_exactly() {
        let (mut doc, _, a, b) = two_squares();
        doc.apply(&Transaction(vec![Operation::SetTransform {
            id: b,
            transform: Affine::translate((80.0, 0.0)) * Affine::rotate(37f64.to_radians()),
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let roots = [a, b];
        let union = union_of(&res, &roots);
        let placement = |d: &Document, r: &Resolved, id: NodeId| {
            let NodeKind::Rect { size, .. } = d.get(id).unwrap().kind() else {
                panic!("not a rect")
            };
            r.world_transform(id).unwrap() * Affine::scale_non_uniform(size.width, size.height)
        };
        let before: Vec<Affine> = roots.iter().map(|id| placement(&doc, &res, *id)).collect();

        let shear = side_shear(
            union,
            Handle::Top,
            SideDrag {
                press: Point::new(union.center().x, union.min_y()),
                at: Point::new(union.center().x + union.height(), union.min_y()),
            },
            false,
        )
        .expect("a lean this box can take");
        doc.apply(&skew_selection(&doc, &res, &roots, shear))
            .unwrap();
        let after = Resolved::rebuild(&doc);

        let want_shear = Affine::translate((0.0, union.max_y()))
            * build::skew_x(-45f64.to_radians())
            * Affine::translate((0.0, -union.max_y()));
        for (id, was) in roots.iter().zip(before) {
            let want = want_shear * was;
            let got = placement(&doc, &after, *id);
            for (g, w) in got.as_coeffs().iter().zip(want.as_coeffs().iter()) {
                assert!(
                    (g - w).abs() < 1e-6,
                    "member landed off the shared shear: {:?} vs {:?}",
                    got.as_coeffs(),
                    want.as_coeffs()
                );
            }
        }
    }

    /// A lean preserves the shape's area.
    #[test]
    fn a_lean_does_not_change_how_much_shape_there_is() {
        let (mut doc, res, a, _) = two_squares();
        let area = |d: &Document| {
            let NodeKind::Rect { size, .. } = d.get(a).unwrap().kind() else {
                panic!("not a rect")
            };
            let det = d.get(a).unwrap().transform().determinant().abs();
            size.width * size.height * det
        };
        let before = area(&doc);

        doc.apply(&skew_to_handle(
            &doc,
            &res,
            a,
            Rect::new(0.0, 0.0, 20.0, 20.0),
            Handle::Right,
            SideDrag {
                press: Point::new(
                    res.world_bounds(a).unwrap().max_x(),
                    res.world_bounds(a).unwrap().center().y,
                ),
                at: Point::new(res.world_bounds(a).unwrap().max_x(), 30.0),
            },
            false,
        ))
        .unwrap();

        assert!(
            (area(&doc) - before).abs() < 1e-6,
            "the lean changed the area: {before} -> {}",
            area(&doc)
        );
    }

    // --- the Scale tool ----------------------------------------------------

    /// A rounded, stroked, dotted rect under an artboard, for scaling.
    fn artboard_with_a_decorated_rect() -> (Document, Resolved, NodeId) {
        let (mut doc, _, rect) = artboard_with_rect();
        doc.apply(&Transaction(vec![
            Operation::SetGeometry {
                id: rect,
                geometry: GeometryPatch::CornerRadii(RoundedRectRadii::new(4.0, 4.0, 8.0, 8.0)),
            },
            Operation::SetStrokes {
                id: rect,
                strokes: ondin_core::keyed_by_position([Stroke {
                    width: 2.0,
                    dashes: vec![0.0, 6.0],
                    dash_offset: 3.0,
                    sides: ondin_core::StrokeSides::Custom {
                        top: 1.0,
                        right: 0.0,
                        bottom: 3.0,
                        left: 2.0,
                    },
                    ..Default::default()
                }]),
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, rect)
    }

    fn radii(doc: &Document, id: NodeId) -> RoundedRectRadii {
        let NodeKind::Rect { corner_radii, .. } = doc.get(id).unwrap().kind() else {
            panic!("not a rect");
        };
        *corner_radii
    }

    /// A photographic scale takes stroke width, corner radii, dashes and per-side widths
    /// with it, and a geometry resize leaves them; asserted as a pair so an unread flag fails.
    #[test]
    fn scaling_takes_the_stroke_radii_and_type_that_a_resize_leaves_alone() {
        for (scaling, want) in [(Scaling::Geometry, 2.0), (Scaling::Photographic, 4.0)] {
            let (mut doc, res, rect) = artboard_with_a_decorated_rect();
            // 50 -> 100 on both axes: a clean doubling.
            doc.apply(&resize_to_handle(
                &doc,
                &res,
                rect,
                Handle::BottomRight,
                Point::new(200.0, 200.0),
                Resize {
                    scaling,
                    ..Default::default()
                },
            ))
            .unwrap();

            let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
                panic!()
            };
            assert_eq!(size, &Size::new(100.0, 100.0), "{scaling:?}: the box");
            let stroke = &doc.get(rect).unwrap().paint().strokes[0];
            assert_eq!(stroke.width, want, "{scaling:?}: stroke width");
            let s = want / 2.0;
            // Each corner scales from its own radius.
            assert_eq!(radii(&doc, rect).top_left, 4.0 * s, "{scaling:?}: top-left");
            assert_eq!(
                radii(&doc, rect).bottom_right,
                8.0 * s,
                "{scaling:?}: bottom-right"
            );
            assert_eq!(stroke.dashes, vec![0.0, 6.0 * s], "{scaling:?}: dashes");
            assert_eq!(stroke.dash_offset, 3.0 * s, "{scaling:?}: dash offset");
            // A side of 0 stays 0.
            assert_eq!(
                stroke.sides,
                ondin_core::StrokeSides::Custom {
                    top: s,
                    right: 0.0,
                    bottom: 3.0 * s,
                    left: 2.0 * s,
                },
                "{scaling:?}: per-side widths"
            );
        }
    }

    /// Scaling a square-cornered, unstroked rect emits no radius or stroke operation.
    #[test]
    fn scaling_a_square_cornered_rect_writes_no_radius_operation() {
        let (doc, res, rect) = artboard_with_rect();
        let tx = resize_to_handle(
            &doc,
            &res,
            rect,
            Handle::BottomRight,
            Point::new(200.0, 200.0),
            Resize {
                scaling: Scaling::Photographic,
                ..Default::default()
            },
        );
        assert!(
            !tx.0.iter().any(|op| matches!(
                op,
                Operation::SetGeometry {
                    geometry: GeometryPatch::CornerRadii(_),
                    ..
                }
            )),
            "a square corner needs no radius patch: {tx:?}"
        );
        assert!(
            !tx.0
                .iter()
                .any(|op| matches!(op, Operation::SetStrokes { .. })),
            "an unstroked layer needs no stroke patch: {tx:?}"
        );
    }

    /// A photographic scale takes a polygon's corner radius with it, and a geometry
    /// resize leaves it, as it does a rect's.
    #[test]
    fn scaling_takes_a_polygons_radius_that_a_resize_leaves_alone() {
        for (scaling, want) in [(Scaling::Geometry, 5.0), (Scaling::Photographic, 10.0)] {
            let mut ids = IdSource::new(1);
            let root = ids.mint();
            let mut doc = Document::new(root);
            let poly = ids.mint();
            doc.apply(&Transaction(vec![Operation::CreateNode {
                id: poly,
                parent: root,
                index: 0,
                kind: NodeKind::Polygon {
                    size: Size::new(50.0, 50.0),
                    sides: 6,
                    corner_radius: 5.0,
                },
                transform: Some(Affine::translate((100.0, 100.0))),
                name: None,
            }]))
            .unwrap();
            let res = Resolved::rebuild(&doc);
            doc.apply(&resize_to_handle(
                &doc,
                &res,
                poly,
                Handle::BottomRight,
                Point::new(200.0, 200.0),
                Resize {
                    scaling,
                    ..Default::default()
                },
            ))
            .unwrap();
            let NodeKind::Polygon {
                size,
                corner_radius,
                ..
            } = doc.get(poly).unwrap().kind()
            else {
                panic!()
            };
            assert_eq!(size, &Size::new(100.0, 100.0), "{scaling:?}: the box");
            assert_eq!(*corner_radius, want, "{scaling:?}: the radius");
        }
    }

    /// A non-uniform scale moves the scalars by the geometric mean of the two factors.
    #[test]
    fn a_non_uniform_scale_moves_the_scalars_by_the_geometric_mean() {
        let (mut doc, res, rect) = artboard_with_a_decorated_rect();
        doc.apply(&resize_to_handle(
            &doc,
            &res,
            rect,
            Handle::BottomRight,
            Point::new(300.0, 150.0),
            Resize {
                scaling: Scaling::Photographic,
                ..Default::default()
            },
        ))
        .unwrap();
        let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
            panic!()
        };
        assert_eq!(size, &Size::new(200.0, 50.0), "the box took both factors");
        assert_eq!(
            doc.get(rect).unwrap().paint().strokes[0].width,
            4.0,
            "sqrt(4 * 1) = 2, so a 2px stroke becomes 4px"
        );
    }

    /// Scaling a group carries the treatment down to every leaf.
    #[test]
    fn scaling_a_group_scales_its_childrens_strokes_too() {
        let (mut doc, _, rect) = artboard_with_a_decorated_rect();
        let parent = doc.get(rect).unwrap().parent().expect("artboard");
        let mut ids = IdSource::new(99);
        let group = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: group,
            parent,
            index: 1,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        }]))
        .unwrap();
        doc.apply(&Transaction(vec![Operation::Reparent {
            id: rect,
            new_parent: group,
            index: 0,
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let local = ondin_core::local_box(&doc, &res, group).expect("group box");

        doc.apply(&resize_group(
            &doc,
            &res,
            group,
            local,
            Handle::BottomRight,
            Point::new(local.max_x() * 2.0, local.max_y() * 2.0),
            Resize {
                scaling: Scaling::Photographic,
                ..Default::default()
            },
        ))
        .unwrap();
        assert!(
            doc.get(rect).unwrap().paint().strokes[0].width > 2.5,
            "the child's stroke stayed at {}",
            doc.get(rect).unwrap().paint().strokes[0].width
        );
    }

    /// A group doubled under the Scale tool scales a card frame's contents with it, and
    /// under a resize only the frame's box doubles.
    ///
    /// flip: delete the `Scaling::Photographic` arm in `scale_subtree` → fails at the rect size under Scale
    #[test]
    fn a_frame_in_a_scaled_group_takes_its_contents_with_it_and_a_resize_does_not() {
        for (scaling, rect_size, rect_at, stroke) in [
            (
                Scaling::Photographic,
                Size::new(40.0, 20.0),
                Point::new(20.0, 20.0),
                4.0,
            ),
            (
                Scaling::Geometry,
                Size::new(20.0, 10.0),
                Point::new(10.0, 10.0),
                2.0,
            ),
        ] {
            let mut ids = IdSource::new(0xF4A);
            let root = ids.mint();
            let mut doc = Document::new(root);
            let (group, card, rect) = (ids.mint(), ids.mint(), ids.mint());
            doc.apply(&Transaction(vec![
                Operation::CreateNode {
                    id: group,
                    parent: root,
                    index: 0,
                    kind: NodeKind::Group,
                    transform: None,
                    name: None,
                },
                Operation::CreateNode {
                    id: card,
                    parent: group,
                    index: 0,
                    kind: NodeKind::Artboard {
                        size: Size::new(100.0, 50.0),
                    },
                    transform: None,
                    name: None,
                },
                Operation::CreateNode {
                    id: rect,
                    parent: card,
                    index: 0,
                    kind: NodeKind::Rect {
                        size: Size::new(20.0, 10.0),
                        corner_radii: Default::default(),
                        corner_smoothing: 0.0,
                    },
                    transform: Some(Affine::translate((10.0, 10.0))),
                    name: None,
                },
                Operation::SetStrokes {
                    id: rect,
                    strokes: ondin_core::keyed_by_position([Stroke {
                        width: 2.0,
                        ..Default::default()
                    }]),
                },
            ]))
            .expect("a frame may sit in a group");
            let res = Resolved::rebuild(&doc);
            let local = ondin_core::local_box(&doc, &res, group).expect("group box");
            assert_eq!(local, Rect::new(0.0, 0.0, 100.0, 50.0), "the fixture");

            doc.apply(&resize_group(
                &doc,
                &res,
                group,
                local,
                Handle::BottomRight,
                Point::new(200.0, 100.0),
                Resize {
                    scaling,
                    ..Default::default()
                },
            ))
            .unwrap();
            let NodeKind::Artboard { size } = doc.get(card).unwrap().kind() else {
                panic!("not a frame")
            };
            assert_eq!(
                size,
                &Size::new(200.0, 100.0),
                "{scaling:?}: the card's box"
            );
            let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
                panic!("not a rect")
            };
            assert_eq!(size, &rect_size, "{scaling:?}: the rect inside the card");
            assert_eq!(
                doc.get(rect).unwrap().transform().translation(),
                rect_at.to_vec2(),
                "{scaling:?}: where the rect sits in the card"
            );
            assert_eq!(
                doc.get(rect).unwrap().paint().strokes[0].width,
                stroke,
                "{scaling:?}: the rect's stroke"
            );
        }
    }

    /// A flex group dragged to twice its hugged size is resized by its box (rects untouched),
    /// and the Scale tool also doubles the rects, padding and gap.
    ///
    /// flip: delete `resize_layer`'s `laid_group` routing → fails at "the box" under resize
    #[test]
    fn a_group_with_a_layout_is_resized_by_its_box() {
        use ondin_core::container::{Dimension, Display, Flex};
        for scaling in [Scaling::Geometry, Scaling::Photographic] {
            let mut ids = IdSource::new(0xF1E);
            let root = ids.mint();
            let mut doc = Document::new(root);
            let (row, a, b) = (ids.mint(), ids.mint(), ids.mint());
            let square = |id, index| Operation::CreateNode {
                id,
                parent: row,
                index,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 20.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            };
            doc.apply(&Transaction(vec![
                Operation::CreateNode {
                    id: row,
                    parent: root,
                    index: 0,
                    kind: NodeKind::Group,
                    transform: Some(Affine::translate((100.0, 100.0))),
                    name: None,
                },
                square(a, 0),
                square(b, 1),
                Operation::SetDisplay {
                    id: row,
                    display: Some(Display::Flex(Flex {
                        column_gap: 10.0,
                        padding: [5.0; 4],
                        ..Default::default()
                    })),
                },
            ]))
            .expect("the row");
            let res = Resolved::rebuild(&doc);
            assert_eq!(
                res.used_frame(row),
                Some(Size::new(60.0, 30.0)),
                "the fixture hugs"
            );

            doc.apply(&resize_layer(
                &doc,
                &res,
                row,
                Handle::BottomRight,
                Point::new(220.0, 160.0),
                Resize {
                    scaling,
                    ..Default::default()
                },
            ))
            .unwrap();
            let res = Resolved::rebuild(&doc);
            let node = doc.get(row).unwrap();
            assert_eq!(
                (node.item().width, node.item().height),
                (Dimension::Px(120.0), Dimension::Px(60.0)),
                "{scaling:?}: the box"
            );
            assert_eq!(
                res.used_frame(row),
                Some(Size::new(120.0, 60.0)),
                "{scaling:?}: laid out"
            );
            assert_eq!(
                res.world_transform(row).map(|t| t.translation()),
                Some(Vec2::new(100.0, 100.0)),
                "{scaling:?}: the held corner stayed"
            );
            let (side, padding, gap) = match scaling {
                Scaling::Geometry => (20.0, 5.0, 10.0),
                Scaling::Photographic => (40.0, 10.0, 20.0),
            };
            let NodeKind::Rect { size, .. } = doc.get(a).unwrap().kind() else {
                panic!("not a rect")
            };
            assert_eq!(size, &Size::new(side, side), "{scaling:?}: the rects");
            let Some(Display::Flex(flex)) = node.display() else {
                panic!("the layout went")
            };
            assert_eq!(flex.padding, [padding; 4], "{scaling:?}: the padding");
            assert_eq!(flex.column_gap, gap, "{scaling:?}: the gap");
        }
    }

    /// A pinned child of a flex group doubled by the Scale tool stays flush with the scaled
    /// box's corner, committed through `build::keep_insets` and `keep_flex_sizes`.
    ///
    /// flip: `keep_insets` measures a laid group by its committed `used_frame` → fails at "flush with the scaled box's corner"
    #[test]
    fn a_pinned_child_of_a_flex_group_scaled_with_it_stays_pinned_to_the_scaled_box() {
        use ondin_core::container::{Display, Flex};
        let mut ids = IdSource::new(0xF1F);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (row, a, b, pinned) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
        let square = |id, index, side| Operation::CreateNode {
            id,
            parent: row,
            index,
            kind: NodeKind::Rect {
                size: Size::new(side, side),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        };
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: row,
                parent: root,
                index: 0,
                kind: NodeKind::Group,
                transform: Some(Affine::translate((100.0, 100.0))),
                name: None,
            },
            square(a, 0, 20.0),
            square(b, 1, 20.0),
            square(pinned, 2, 10.0),
            Operation::SetDisplay {
                id: row,
                display: Some(Display::Flex(Flex {
                    column_gap: 10.0,
                    padding: [5.0; 4],
                    ..Default::default()
                })),
            },
            Operation::SetInsets {
                id: pinned,
                insets: ondin_core::Insets {
                    right: Some(ondin_core::LengthPct::Px(0.0)),
                    bottom: Some(ondin_core::LengthPct::Px(0.0)),
                    ..Default::default()
                },
            },
        ]))
        .expect("the row");
        let res = Resolved::rebuild(&doc);
        assert_eq!(
            res.world_bounds(pinned),
            Some(Rect::new(150.0, 120.0, 160.0, 130.0)),
            "the fixture: pinned to the 60 × 30 box's corner"
        );

        let tx = resize_layer(
            &doc,
            &res,
            row,
            Handle::BottomRight,
            Point::new(220.0, 160.0),
            Resize {
                scaling: Scaling::Photographic,
                ..Default::default()
            },
        );
        let tx = ondin_core::build::keep_insets(&doc, &res, tx);
        let tx = ondin_core::build::keep_flex_sizes(&doc, &res, tx);
        doc.apply(&tx).unwrap();
        let res = Resolved::rebuild(&doc);
        assert_eq!(
            res.used_frame(row),
            Some(Size::new(120.0, 60.0)),
            "the box doubled"
        );
        assert_eq!(
            res.world_bounds(pinned),
            Some(Rect::new(200.0, 140.0, 220.0, 160.0)),
            "flush with the scaled box's corner"
        );
    }

    /// Auto-sized text takes the scaled font size but keeps its auto sizing mode.
    #[test]
    fn scaling_auto_sized_text_moves_its_type_size_but_not_its_sizing_mode() {
        let (mut doc, res, group, text) = group_with_auto_text();
        let local = ondin_core::local_box(&doc, &res, group).expect("group box");
        doc.apply(&resize_group(
            &doc,
            &res,
            group,
            local,
            Handle::BottomRight,
            Point::new(local.max_x() * 2.0, local.max_y() * 2.0),
            Resize {
                scaling: Scaling::Photographic,
                ..Default::default()
            },
        ))
        .unwrap();
        let NodeKind::Text { sizing, style, .. } = doc.get(text).unwrap().kind() else {
            panic!("not text")
        };
        assert!(
            matches!(sizing, TextSizing::Auto),
            "auto text must keep its sizing mode: {sizing:?}"
        );
        assert!(
            style.font_size > 16.5,
            "the type size stayed at {}",
            style.font_size
        );
    }

    /// A photographic scale leaves a bound size at its literal 0, so the binding
    /// still reads it as its own and the text draws its variable.
    ///
    /// flip: `scaled_text`'s copy of the bound slots dropped → fails at *"still
    /// bound"*, the size clamped to 1 and detached.
    #[test]
    fn scaling_a_text_keeps_its_bound_size() {
        use ondin_core::variable::TEXT_SIZE;
        let (mut doc, _, group, text) = group_with_auto_text();
        let mut ids = IdSource::new(8);
        let (mode, var) = (ids.mint_mode(), ids.mint_var());
        doc.apply(&Transaction(vec![
            Operation::AddMode {
                mode: ondin_core::Mode {
                    id: mode,
                    name: "Mode 1".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: ondin_core::Variable {
                    id: var,
                    name: "type/size".into(),
                    kind: ondin_core::VarKind::Number,
                    cells: [(mode, ondin_core::Cell::Number(32.0))].into(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ]))
        .unwrap();
        let bind = ondin_core::bind_text_ops(
            &doc,
            text,
            ondin_core::variable::text_vars_at(TEXT_SIZE, var),
        );
        doc.apply(&Transaction(bind)).unwrap();
        let res = Resolved::rebuild(&doc);
        let local = ondin_core::local_box(&doc, &res, group).expect("group box");
        let tx = resize_group(
            &doc,
            &res,
            group,
            local,
            Handle::BottomRight,
            Point::new(local.max_x() * 2.0, local.max_y() * 2.0),
            Resize {
                scaling: Scaling::Photographic,
                ..Default::default()
            },
        );
        // The commit's pass, which judges the size written against the binding.
        let tx = ondin_core::build::keep_text_bindings(&doc, tx);
        doc.apply(&tx).unwrap();
        let node = doc.get(text).unwrap();
        assert_eq!(node.text_vars()[TEXT_SIZE], Some(var), "still bound");
        let NodeKind::Text { style, .. } = node.kind() else {
            panic!("not text")
        };
        assert_eq!(style.font_size, 0.0, "the literal 0");
        let Some(NodeKind::Text { style, .. }) = doc.resolved_kind(text).map(|k| k.into_owned())
        else {
            panic!("not text")
        };
        assert_eq!(style.font_size, 32.0, "the variable drawn");
    }

    /// A group with an auto-sized text layer and a rect beside it, so the group has
    /// a box to scale.
    fn group_with_auto_text() -> (Document, Resolved, NodeId, NodeId) {
        let mut ids = IdSource::new(7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let group = ids.mint();
        let text = ids.mint();
        let rect = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: text,
                parent: group,
                index: 0,
                kind: NodeKind::Text {
                    content: "Label".into(),
                    style: Box::new(TextStyle {
                        font_family: "Inter".into(),
                        font_size: 16.0,
                        weight: 400,
                        italic: false,
                        line_height: Some(ondin_core::Length::Em(1.2)),
                        ..Default::default()
                    }),
                    spans: Default::default(),
                    para_spans: Default::default(),
                    paragraph: Default::default(),
                    block: Default::default(),
                    sizing: TextSizing::Auto,
                    on_path: None,
                    on_path_flip: false,
                    on_path_offset: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: rect,
                parent: group,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 100.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((0.0, 100.0))),
                name: None,
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, group, text)
    }

    /// A scale of one writes no scalar operations; `Scaling::scalar` guards factors a few ulps off 1.
    #[test]
    fn a_scale_of_one_writes_no_scalar_operations() {
        let (doc, res, rect) = artboard_with_a_decorated_rect();
        let tx = resize_to_handle(
            &doc,
            &res,
            rect,
            Handle::BottomRight,
            Point::new(150.0, 150.0),
            Resize {
                scaling: Scaling::Photographic,
                ..Default::default()
            },
        );
        assert!(
            !tx.0
                .iter()
                .any(|op| matches!(op, Operation::SetStrokes { .. })),
            "an unchanged box must not rewrite the strokes: {tx:?}"
        );
    }

    // --- segments: insert, remove, move, bend ------------------------------

    /// A closed run with one curved side, as `pen_anchors` would hand it over.
    fn quad_with_a_curve() -> Vec<PenSubpath> {
        use ondin_core::kurbo::Vec2;
        vec![PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(0.0, 0.0)),
                PenAnchor::corner(Point::new(100.0, 0.0)),
                // Arrives straight, leaves on a curve: easy for an insert to get wrong.
                PenAnchor {
                    at: Point::new(100.0, 100.0),
                    out: Vec2::new(-40.0, 20.0),
                    back: Vec2::ZERO,
                    radius: 0.0,
                },
                PenAnchor {
                    at: Point::new(0.0, 100.0),
                    out: Vec2::ZERO,
                    back: Vec2::new(40.0, 20.0),
                    radius: 0.0,
                },
            ],
            closed: true,
        }]
    }

    /// Asserts two paths trace the same shape however each is cut into segments: every
    /// sample of each lies on the other, within the nearest-point solve's accuracy.
    fn assert_same_shape(a: &BezPath, b: &BezPath) {
        for (from, onto, which) in [(a, b, "a→b"), (b, a, "b→a")] {
            for (i, seg) in from.segments().enumerate() {
                for f in 0..=8 {
                    let p = seg.eval(f as f64 / 8.0);
                    let hit = ondin_core::geometry::nearest_segment(onto, p)
                        .expect("the other path has segments");
                    assert!(
                        hit.distance < 1e-3,
                        "{which}: segment {i} at t={} is {:.6} off the other path",
                        f as f64 / 8.0,
                        hit.distance
                    );
                }
            }
        }
    }

    /// Inserting a point leaves the shape unchanged (sampled, since one cubic becomes two)
    /// and survives the `pen_anchors` round trip.
    #[test]
    fn inserting_a_point_splits_a_segment_without_moving_the_curve() {
        for (at, t) in [((0, 2), 0.5), ((0, 2), 0.25), ((0, 0), 0.5), ((0, 3), 0.5)] {
            let before = quad_with_a_curve();
            let mut after = before.clone();
            let new = insert_point(&mut after, at, t).expect("a segment to split");
            assert_eq!(new, (at.0, at.1 + 1), "the new anchor follows the old one");
            assert_eq!(after[0].anchors.len(), before[0].anchors.len() + 1);
            assert!(after[0].closed, "closure survives an insert");

            let (a, b) = (pen_path(&before), pen_path(&after));
            assert_same_shape(&a, &b);
            assert_same_path(&b, &pen_path(&pen_anchors(&b, &[])));
        }
    }

    /// Splitting a straight segment writes two lines and a corner, not two flat cubics.
    #[test]
    fn splitting_a_line_leaves_two_lines_and_a_corner() {
        let mut subs = quad_with_a_curve();
        insert_point(&mut subs, (0, 0), 0.5).unwrap();
        assert!(subs[0].anchors[1].is_corner(), "the new point is a corner");
        assert_eq!(subs[0].anchors[1].at, Point::new(50.0, 0.0));
        let els = pen_path(&subs).elements().to_vec();
        assert!(
            matches!(els[1], PathEl::LineTo(_)) && matches!(els[2], PathEl::LineTo(_)),
            "{els:#?}"
        );
    }

    /// A radius rides on its anchor, so an insert cannot slide one onto the neighbouring corner.
    #[test]
    fn an_insert_leaves_every_radius_on_the_corner_that_owned_it() {
        let mut subs = quad_with_a_curve();
        subs[0].anchors[0].radius = 3.0;
        subs[0].anchors[1].radius = 8.0;
        insert_point(&mut subs, (0, 0), 0.5).unwrap();
        let radii: Vec<f64> = subs[0].anchors.iter().map(|a| a.radius).collect();
        assert_eq!(
            radii,
            vec![3.0, 0.0, 8.0, 0.0, 0.0],
            "the new point is at 1"
        );
        assert_eq!(pen_radii(&subs), vec![3.0, 0.0, 8.0]);
    }

    /// Removing a segment opens a closed run starting after the cut, including the closing segment.
    #[test]
    fn removing_a_segment_opens_a_closed_run_at_the_cut() {
        use ondin_core::kurbo::Vec2;
        let subs = quad_with_a_curve();
        let corners: Vec<Point> = subs[0].anchors.iter().map(|a| a.at).collect();

        let left = remove_segments(&subs, &[(0, 1)]);
        assert_eq!(left.len(), 1);
        assert!(!left[0].closed, "a cut run cannot still be closed");
        let got: Vec<Point> = left[0].anchors.iter().map(|a| a.at).collect();
        assert_eq!(got, vec![corners[2], corners[3], corners[0], corners[1]]);
        // The handles of the removed segment are cleared, or they would reappear on closing.
        assert_eq!(left[0].anchors[0].back, Vec2::ZERO);
        assert_eq!(left[0].anchors[3].out, Vec2::ZERO);

        // The closing segment: the same order, merely open.
        let closing = remove_segments(&subs, &[(0, 3)]);
        let got: Vec<Point> = closing[0].anchors.iter().map(|a| a.at).collect();
        assert_eq!(got, corners);
        assert!(!closing[0].closed);
    }

    /// On an open run removal splits it, and a remainder too short to draw is dropped.
    #[test]
    fn removing_a_segment_splits_an_open_run_and_drops_a_lone_point() {
        let mut subs = quad_with_a_curve();
        subs[0].closed = false;

        let two = remove_segments(&subs, &[(0, 1)]);
        assert_eq!(two.len(), 2, "one run became two");
        assert_eq!(two[0].anchors.len(), 2);
        assert_eq!(two[1].anchors.len(), 2);

        let one = remove_segments(&subs, &[(0, 0)]);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].anchors.len(), 3);

        // The last anchor of an open run has no segment leaving it: nothing is removed.
        assert!(segment_ends(&subs, (0, 3)).is_none());
        let none = remove_segments(&subs, &[(0, 3)]);
        assert_eq!(none.len(), 1);
        assert_eq!(none[0].anchors.len(), 4);

        let pair = vec![PenSubpath {
            anchors: subs[0].anchors[..2].to_vec(),
            closed: false,
        }];
        assert!(remove_segments(&pair, &[(0, 0)]).is_empty());
    }

    /// A segment drag is `move_points` over the ends `segment_ends` names: both anchors
    /// move, handles ride along, nothing else does.
    #[test]
    fn moving_a_segment_carries_both_its_anchors_and_no_others() {
        use ondin_core::kurbo::Vec2;
        let mut subs = quad_with_a_curve();
        let before = subs.clone();
        let delta = Vec2::new(10.0, -5.0);
        let (a, b) = segment_ends(&subs, (0, 2)).expect("a segment");
        move_points(&mut subs, &[a, b], delta);
        for i in 0..4 {
            let (a, b) = (before[0].anchors[i], subs[0].anchors[i]);
            let want = if i == 2 || i == 3 { a.at + delta } else { a.at };
            assert_eq!(b.at, want, "anchor {i}");
            assert_eq!(b.out, a.out, "anchor {i}: handles ride along unchanged");
            assert_eq!(b.back, a.back, "anchor {i}");
        }
    }

    /// Two cuts in a closed quad leave two open runs; the anchor order is asserted, since
    /// the closed-to-open rotation is where an off-by-one would live.
    #[test]
    fn removing_several_segments_at_once_leaves_a_run_between_each_pair() {
        let subs = quad_with_a_curve();
        let p: Vec<Point> = subs[0].anchors.iter().map(|a| a.at).collect();

        let two = remove_segments(&subs, &[(0, 0), (0, 2)]);
        assert_eq!(two.len(), 2);
        let got: Vec<Vec<Point>> = two
            .iter()
            .map(|s| s.anchors.iter().map(|a| a.at).collect())
            .collect();
        assert_eq!(got, vec![vec![p[1], p[2]], vec![p[3], p[0]]], "{got:?}");
        assert!(two.iter().all(|s| !s.closed));

        assert!(remove_segments(&subs, &[(0, 0), (0, 1), (0, 2), (0, 3)]).is_empty());

        // An index naming no segment is ignored (an open run's last anchor has none).
        let mut open = subs.clone();
        open[0].closed = false;
        let one = remove_segments(&open, &[(0, 3), (0, 1)]);
        let sizes: Vec<usize> = one.iter().map(|s| s.anchors.len()).collect();
        assert_eq!(sizes, vec![2, 2], "only the real cut took");
    }

    /// A bend puts the grabbed point exactly under the pointer at any `t` and moves no anchor.
    #[test]
    fn bending_a_segment_moves_the_grabbed_point_by_the_whole_delta() {
        use ondin_core::kurbo::Vec2;
        for t in [0.25, 0.5, 0.75] {
            let mut subs = quad_with_a_curve();
            let before = segment_point(&subs, (0, 0), t).unwrap();
            let ends = (subs[0].anchors[0].at, subs[0].anchors[1].at);
            let delta = Vec2::new(6.0, -30.0);
            bend_segment(&mut subs, (0, 0), t, delta);

            let after = segment_point(&subs, (0, 0), t).unwrap();
            assert!(
                (after - (before + delta)).hypot() < 1e-9,
                "t={t}: {after:?} vs {:?}",
                before + delta
            );
            assert_eq!(
                (subs[0].anchors[0].at, subs[0].anchors[1].at),
                ends,
                "t={t}: a bend moves no anchor"
            );
            assert!(!subs[0].anchors[0].is_corner(), "t={t}: it curves now");
            // The other side of each anchor is untouched.
            assert_eq!(subs[0].anchors[0].back, Vec2::ZERO);
            assert_eq!(subs[0].anchors[1].out, Vec2::ZERO);
        }

        // At the ends the control points have no leverage, so the bend declines.
        let mut subs = quad_with_a_curve();
        let before = subs[0].anchors[0].out;
        bend_segment(&mut subs, (0, 0), 0.0, Vec2::new(10.0, 10.0));
        assert_eq!(subs[0].anchors[0].out, before);
    }

    /// The point box is the selected anchors' extent; a handle drag scales the set about
    /// the opposite corner under `resize_factors`' rule.
    #[test]
    fn a_point_box_scales_the_selected_points_about_the_held_corner() {
        let subs = quad_with_a_curve();
        let all: Vec<PointRef> = (0..4).map(|i| (0, i)).collect();
        let box_ = points_box(&subs, &all).expect("four points make a box");
        assert_eq!(box_, Rect::new(0.0, 0.0, 100.0, 100.0));
        // One point or a flat pair is no box: its handles would be unreachable.
        assert!(points_box(&subs, &[(0, 0)]).is_none());
        assert!(points_box(&subs, &[(0, 0), (0, 1)]).is_none());

        let mut scaled = subs.clone();
        resize_points(
            &mut scaled,
            &all,
            box_,
            Handle::BottomRight,
            Point::new(200.0, 200.0),
            Resize::geometry(false, false),
        );
        assert_eq!(scaled[0].anchors[0].at, Point::new(0.0, 0.0));
        assert_eq!(scaled[0].anchors[2].at, Point::new(200.0, 200.0));
        assert_eq!(scaled[0].anchors[2].out, subs[0].anchors[2].out * 2.0);

        // Only the selected points move.
        let picked = [(0, 0), (0, 2)];
        let mut some = subs.clone();
        resize_points(
            &mut some,
            &picked,
            points_box(&subs, &picked).unwrap(),
            Handle::BottomRight,
            Point::new(200.0, 200.0),
            Resize::geometry(false, false),
        );
        assert_eq!(some[0].anchors[2].at, Point::new(200.0, 200.0));
        assert_eq!(some[0].anchors[1].at, subs[0].anchors[1].at, "unselected");
        assert_eq!(some[0].anchors[3].at, subs[0].anchors[3].at, "unselected");
    }

    /// Every handle dropped where it was picked up scales by one; a min-side handle starts
    /// at −extent from its anchor, so the identity is asked of all eight.
    #[test]
    fn a_handle_dropped_where_it_was_grabbed_scales_by_one() {
        let box_ = Rect::new(10.0, 20.0, 110.0, 70.0);
        for handle in Handle::CORNERS.into_iter().chain(Handle::EDGES) {
            let (ux, uy) = handle.unit();
            let at = Point::new(
                box_.min_x() + box_.width() * ux,
                box_.min_y() + box_.height() * uy,
            );
            for symmetric in [false, true] {
                let (sx, sy, _) = resize_factors(box_, handle, at, false, symmetric);
                assert!(
                    (sx - 1.0).abs() < 1e-9 && (sy - 1.0).abs() < 1e-9,
                    "{handle:?} symmetric={symmetric} at rest gave ({sx}, {sy})"
                );
            }
        }
    }

    /// A dragged min-side handle follows the pointer and holds the opposite side.
    #[test]
    fn a_min_side_handle_scales_rather_than_mirroring() {
        let box_ = Rect::new(0.0, 0.0, 100.0, 50.0);
        let (sx, sy, anchor) = resize_factors(
            box_,
            Handle::TopLeft,
            Point::new(-100.0, -50.0),
            false,
            false,
        );
        assert!(
            (sx - 2.0).abs() < 1e-9 && (sy - 2.0).abs() < 1e-9,
            "{sx}, {sy}"
        );
        assert_eq!(anchor, Point::new(100.0, 50.0));

        let (sx, sy, anchor) =
            resize_factors(box_, Handle::Left, Point::new(50.0, 25.0), false, false);
        assert!(
            (sx - 0.5).abs() < 1e-9 && (sy - 1.0).abs() < 1e-9,
            "{sx}, {sy}"
        );
        assert_eq!(anchor.x, 100.0);

        // Dragged past the anchor it holds, the sign flips.
        let (sx, _, _) =
            resize_factors(box_, Handle::TopLeft, Point::new(200.0, 0.0), false, false);
        assert!(sx < 0.0, "a handle dragged past its anchor mirrors: {sx}");
    }

    /// `geometry::nearest_segment`'s `t` and `segment_cubic`'s agree on a line as on a
    /// curve, since a straight segment is degree-elevated to keep the linear parameter.
    #[test]
    fn a_segments_parameter_means_the_same_to_the_hit_test_and_to_the_app() {
        use ondin_core::kurbo::Vec2;
        let subs = vec![PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(0.0, 0.0)),
                PenAnchor::corner(Point::new(300.0, 0.0)),
                PenAnchor::smooth(Point::new(400.0, 200.0), Vec2::new(0.0, 90.0)),
                PenAnchor::corner(Point::new(100.0, 260.0)),
            ],
            closed: false,
        }];
        let path = pen_path(&subs);
        for seg in 0..3usize {
            let cubic = segment_cubic(&subs, (0, seg)).expect("a segment");
            for k in 1..8 {
                let u = k as f64 / 8.0;
                let pointer = cubic.eval(u) + Vec2::new(0.0, -2.0);
                let hit = ondin_core::geometry::nearest_segment(&path, pointer).expect("a hit");
                let mine = segment_point(&subs, (hit.subpath, hit.anchor), hit.t).expect("a point");
                assert!(
                    (mine - hit.at).hypot() < 1e-6,
                    "segment {seg} at u={u}: core says {:?}, the app says {mine:?}",
                    hit.at
                );
            }
        }

        // The straight one is exactly a lerp.
        let line = segment_cubic(&subs, (0, 0)).unwrap();
        for k in 0..=4 {
            let t = k as f64 / 4.0;
            assert!((line.eval(t).x - 300.0 * t).abs() < 1e-9, "t={t}");
        }
    }

    /// A bend on a straight segment grabs the point the pointer took, and a zero bend writes nothing.
    #[test]
    fn bending_a_straight_segment_moves_the_point_the_pointer_took() {
        use ondin_core::kurbo::Vec2;
        for t in [0.25, 0.5, 0.75] {
            let mut subs = vec![PenSubpath {
                anchors: vec![
                    PenAnchor::corner(Point::new(0.0, 0.0)),
                    PenAnchor::corner(Point::new(300.0, 0.0)),
                ],
                closed: false,
            }];
            let before = segment_point(&subs, (0, 0), t).unwrap();
            assert!((before.x - 300.0 * t).abs() < 1e-9, "the grab is a lerp");
            let delta = Vec2::new(10.0, -40.0);
            bend_segment(&mut subs, (0, 0), t, delta);
            let after = segment_point(&subs, (0, 0), t).unwrap();
            assert!(
                (after - (before + delta)).hypot() < 1e-9,
                "t={t}: {after:?} vs {:?}",
                before + delta
            );
        }

        // A Ctrl-press that never travelled must not elevate the line.
        let mut subs = vec![PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(0.0, 0.0)),
                PenAnchor::corner(Point::new(300.0, 0.0)),
            ],
            closed: false,
        }];
        bend_segment(&mut subs, (0, 0), 0.5, Vec2::ZERO);
        assert!(subs[0].anchors[0].is_corner(), "still a plain line");
    }

    /// A dragged handle snaps to the direction that makes its anchor smooth, keeping its
    /// length; the tolerance is a perpendicular distance, not an angle.
    #[test]
    fn a_dragged_handle_snaps_to_the_direction_that_makes_its_anchor_smooth() {
        use ondin_core::kurbo::Vec2;
        let at = Point::new(100.0, 100.0);
        // The smooth direction for the outgoing handle is the opposite of the incoming one.
        let broken = PenAnchor {
            at,
            out: Vec2::new(5.0, 0.0),
            back: Vec2::new(-30.0, -40.0),
            radius: 0.0,
        };
        let smooth_dir = Vec2::new(30.0, 40.0).normalize();

        let want = at + smooth_dir * 50.0;
        let perp = Vec2::new(-smooth_dir.y, smooth_dir.x) * 3.0;
        let got = tangent_snap(&broken, Side::Out, want + perp, 4.0);
        assert!((got - want).hypot() < 1e-9, "{got:?} vs {want:?}");

        // 6 units off is past the tolerance, and free aim wins.
        let free = want + perp * 2.0;
        assert_eq!(tangent_snap(&broken, Side::Out, free, 4.0), free);

        // The same offset at four times the length still snaps; an angular tolerance would not.
        let far = at + smooth_dir * 200.0;
        let got = tangent_snap(&broken, Side::Out, far + perp, 4.0);
        assert!((got - far).hypot() < 1e-9, "{got:?} vs {far:?}");

        // The eight compass directions are candidates too.
        let axis = at + Vec2::new(60.0, 0.0);
        let got = tangent_snap(&broken, Side::Back, axis + Vec2::new(0.0, 2.0), 4.0);
        assert!((got - axis).hypot() < 1e-9, "{got:?} vs {axis:?}");

        // A candidate behind the pointer is not offered: it would flip the handle through its anchor.
        let behind = at - smooth_dir * 50.0;
        let got = tangent_snap(&broken, Side::Out, behind, 4.0);
        assert!(
            (got - behind).hypot() < 1e-9 || (got - at).dot(smooth_dir) < 0.0,
            "the handle jumped to the other side: {got:?}"
        );

        // A corner offers no smooth direction, only the compass points.
        let corner = PenAnchor::corner(at);
        let off = at + Vec2::new(50.0, 20.0);
        assert_eq!(tangent_snap(&corner, Side::Out, off, 4.0), off);

        // Nor does an already-smooth anchor: its own direction would make a slow drag stick, then jump.
        let mirrored = PenAnchor::smooth(at, Vec2::new(40.0, 10.0));
        let nudged = at + Vec2::new(41.0, 11.5);
        assert_eq!(
            tangent_snap(&mirrored, Side::Out, nudged, 4.0),
            nudged,
            "a smooth anchor snapped its own handle back to where it was"
        );

        // Through `pull_handle`, which the drag calls; Alt so the mirror does not realign it.
        let mut subs = vec![PenSubpath {
            anchors: vec![broken],
            closed: false,
        }];
        pull_handle(&mut subs, (0, 0), Side::Out, want + perp, true, 0.0, 4.0);
        assert!(
            (subs[0].anchors[0].leaving() - want).hypot() < 1e-9,
            "the drag did not ask for the tangent snap: {:?}",
            subs[0].anchors[0].leaving()
        );
    }

    /// A join meets the endpoint the press grabbed, at either end of the target, with the
    /// opposite orientation to the pen's resume; checked against `PenEdit::endpoint`, the ring's point.
    #[test]
    fn a_join_meets_the_endpoint_the_press_grabbed() {
        use crate::preview::PenEdit;
        use ondin_core::kurbo::Affine;

        let head = [
            PenAnchor::corner(Point::new(0.0, 0.0)),
            PenAnchor::corner(Point::new(10.0, 0.0)),
        ];
        let target = PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(100.0, 0.0)),
                PenAnchor::corner(Point::new(150.0, 50.0)),
                PenAnchor::corner(Point::new(200.0, 0.0)),
            ],
            closed: false,
        };
        for from_start in [true, false] {
            let edit = PenEdit {
                id: IdSource::new(1).mint(),
                world: Affine::IDENTITY,
                subpaths: vec![target.clone()],
                active: 0,
                from_start,
                minted: false,
            };
            let joined = join_runs(&head, &target, from_start);
            assert_eq!(joined.anchors.len(), head.len() + target.anchors.len());
            assert!(!joined.closed, "a join produces an open run");
            assert_eq!(
                joined.anchors[head.len()].at,
                edit.endpoint().expect("a run with anchors has an endpoint"),
                "from_start={from_start}: the runs must meet at the ringed endpoint"
            );
            // The head is untouched and comes first; the target's far end ends the join.
            assert_eq!(joined.anchors[0].at, head[0].at);
            assert_eq!(joined.anchors[head.len() - 1].at, head[head.len() - 1].at);
            let other = match from_start {
                true => target.anchors[target.anchors.len() - 1].at,
                false => target.anchors[0].at,
            };
            assert_eq!(joined.anchors[joined.anchors.len() - 1].at, other);
        }
    }

    /// A run of `n` anchors along the x axis, offset so two runs are tellable apart.
    fn run(n: usize, x0: f64, closed: bool) -> PenSubpath {
        PenSubpath {
            anchors: (0..n)
                .map(|i| PenAnchor::corner(Point::new(x0 + i as f64 * 10.0, 0.0)))
                .collect(),
            closed,
        }
    }

    /// Join takes two open ends and refuses the rest, including both ends of a two-anchor
    /// run, whose closing segment would retrace its only one (`context-menus.md` §6.5).
    ///
    /// flip: drop the `anchors.len() < 3` arm → the last case comes back `Some`; check `sub.closed` on only the first ref → the second-ref closed case fails
    #[test]
    fn join_takes_two_open_ends_and_refuses_everything_else() {
        let two_runs = [run(3, 0.0, false), run(3, 100.0, false)];
        let ok = |points: &[PointRef]| joinable_ends(&two_runs, points).is_some();

        assert!(ok(&[(0, 2), (1, 0)]));
        assert!(ok(&[(0, 0), (1, 2)]), "either end of either run");
        assert!(ok(&[(0, 0), (0, 2)]));

        // A middle anchor, the wrong count, nothing, and a missing index all refuse.
        assert!(!ok(&[(0, 1), (1, 0)]));
        assert!(!ok(&[(0, 0)]));
        assert!(!ok(&[(0, 0), (0, 2), (1, 0)]));
        assert!(!ok(&[]));
        assert!(!ok(&[(0, 9), (1, 0)]));

        // A closed run has no ends, in either position of the pair.
        let closed = [run(3, 0.0, true), run(3, 100.0, false)];
        assert!(!joinable_ends(&closed, &[(0, 0), (1, 0)]).is_some());
        assert!(!joinable_ends(&closed, &[(1, 0), (0, 0)]).is_some());

        let short = [run(2, 0.0, false), run(3, 100.0, false)];
        assert!(
            joinable_ends(&short, &[(0, 0), (0, 1)]).is_none(),
            "closing a two-anchor run retraces its only segment"
        );
        assert!(
            joinable_ends(&short, &[(0, 0), (1, 0)]).is_some(),
            "but splicing that same run onto another one is fine"
        );
    }

    /// A splice runs from one far end to the other through the seam for all four end
    /// combinations; asserted on the position sequence, since a wrong turn keeps the length.
    ///
    /// flip: reverse the head on `a.1 == last` instead of `a.1 == 0` → the two `(0, 0)` cases fail
    #[test]
    fn a_splice_reads_end_to_end_through_the_seam() {
        let subpaths = [run(3, 0.0, false), run(3, 100.0, false)];
        let xs = |sub: &PenSubpath| sub.anchors.iter().map(|a| a.at.x).collect::<Vec<_>>();

        for (a, b, want) in [
            // grabbed at 0/20 → the head turns; grabbed at 100 → the tail does not.
            ((0, 2), (1, 0), vec![0.0, 10.0, 20.0, 100.0, 110.0, 120.0]),
            ((0, 2), (1, 2), vec![0.0, 10.0, 20.0, 120.0, 110.0, 100.0]),
            ((0, 0), (1, 0), vec![20.0, 10.0, 0.0, 100.0, 110.0, 120.0]),
            ((0, 0), (1, 2), vec![20.0, 10.0, 0.0, 120.0, 110.0, 100.0]),
        ] {
            let (list, seam) = join_ends(&subpaths, a, b).expect("these are open ends");
            assert_eq!(list.len(), 1, "{a:?}→{b:?}: the two runs became one");
            assert_eq!(xs(&list[0]), want, "{a:?}→{b:?}");
            assert!(!list[0].closed, "a splice leaves the run open");
            assert_eq!(seam, (0, 2), "{a:?}→{b:?}");
        }
    }

    /// The joined run keeps the lower slot, other subpaths are not renumbered, and the seam
    /// names the surviving slot.
    ///
    /// flip: write the joined run to `a.0` and remove `b.0` → fails with the pair reversed
    #[test]
    fn a_splice_keeps_the_lower_slot_whichever_end_was_grabbed_first() {
        let untouched = run(4, 500.0, true);
        let subpaths = [run(3, 0.0, false), untouched.clone(), run(3, 100.0, false)];
        for (a, b) in [((2, 0), (0, 2)), ((0, 2), (2, 0))] {
            let (list, seam) = join_ends(&subpaths, a, b).expect("these are open ends");
            assert_eq!(list.len(), 2, "{a:?}→{b:?}");
            assert_eq!(list[0].anchors.len(), 6, "the join is in slot 0");
            assert_eq!(
                list[1].anchors.iter().map(|p| p.at.x).collect::<Vec<_>>(),
                untouched.anchors.iter().map(|p| p.at.x).collect::<Vec<_>>(),
                "{a:?}→{b:?}: the run nobody touched came through unchanged"
            );
            assert_eq!(seam.0, 0, "{a:?}→{b:?}: the seam names the surviving slot");
        }
    }

    /// Both ends of one run close it, and the closing segment is named by the run's last anchor.
    ///
    /// flip: return `(a.0, 0)` for the seam → fails at the seam assertion
    #[test]
    fn closing_a_run_adds_no_anchors_and_names_the_closing_segment() {
        let subpaths = [run(4, 0.0, false), run(3, 100.0, false)];
        let (list, seam) = join_ends(&subpaths, (0, 0), (0, 3)).expect("its own two ends");
        assert_eq!(list.len(), 2, "closing removes no subpath");
        assert_eq!(list[0].anchors.len(), 4, "and adds no anchor");
        assert!(list[0].closed);
        assert!(!list[1].closed, "the other run is left alone");
        assert_eq!(seam, (0, 3));
    }

    /// Reversing a run flips its winding (the signed area's sign) and nothing else: the
    /// magnitude survives, the radius rides along, and unnamed runs are left alone.
    #[test]
    fn reversing_a_subpath_flips_its_winding_and_leaves_the_others_alone() {
        use ondin_core::kurbo::Shape;

        let ring = |x: f64, y: f64, w: f64| PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(x, y)),
                PenAnchor::corner(Point::new(x + w, y)),
                PenAnchor::corner(Point::new(x + w, y + w)),
                PenAnchor::corner(Point::new(x, y + w)),
            ],
            closed: true,
        };
        let mut subs = vec![ring(0.0, 0.0, 100.0), ring(25.0, 25.0, 50.0)];
        subs[1].anchors[2].radius = 6.0;
        let area = |s: &PenSubpath| pen_outline(&s.anchors, s.closed).area();
        let (outer0, inner0) = (area(&subs[0]), area(&subs[1]));
        assert!(
            outer0.signum() == inner0.signum(),
            "the fixture must start with both runs wound the same way"
        );

        reverse_subpaths(&mut subs, &[1]);
        let (outer1, inner1) = (area(&subs[0]), area(&subs[1]));
        assert_eq!(
            inner1.signum(),
            -inner0.signum(),
            "the inner run still winds {inner0} — non-zero fills it solid instead of \
             cutting a hole"
        );
        assert!(
            (inner1.abs() - inner0.abs()).abs() < 1e-9,
            "the reversal changed the shape: |area| {} → {}",
            inner0.abs(),
            inner1.abs()
        );
        assert_eq!(outer1, outer0, "the run nobody named was turned round too");
        assert_eq!(subs[1].anchors[1].radius, 6.0, "the radius left its corner");
        assert_eq!(pen_radii(&subs), vec![0.0, 0.0, 0.0, 0.0, 0.0, 6.0]);
    }

    /// `subpaths_touched` lists the runs a point selection names, ascending and deduplicated.
    #[test]
    fn the_subpaths_a_selection_touches_are_listed_once_each() {
        assert_eq!(
            subpaths_touched(&[(2, 0), (0, 5), (2, 1), (0, 0), (1, 3)]),
            vec![0, 1, 2]
        );
        assert!(subpaths_touched(&[]).is_empty());
    }
    // --- placing an image (§5.5a) --------------------------------------------

    fn entry_of(w: u32, h: u32) -> ImageEntry {
        ImageEntry {
            source: ondin_core::ImageSource::Embedded(vec![1, 2, 3].into()),
            format: ondin_core::ImageFormat::Png,
            width: w,
            height: h,
        }
    }

    /// Fit shrinks an oversized image and never scales a small one up.
    #[test]
    fn a_placed_image_shrinks_to_fit_and_is_never_scaled_up() {
        let frame = Size::new(1000.0, 800.0);

        assert_eq!(
            fitted_image_size(Size::new(32.0, 32.0), frame),
            Size::new(32.0, 32.0)
        );
        assert_eq!(fitted_image_size(frame, frame), frame);

        let wide = fitted_image_size(Size::new(4000.0, 2000.0), frame);
        assert_eq!(wide, Size::new(1000.0, 500.0));

        let tall = fitted_image_size(Size::new(1000.0, 4000.0), frame);
        assert_eq!(tall, Size::new(200.0, 800.0));
        assert!(
            tall.height <= frame.height && tall.width <= frame.width,
            "a fitted image must be inside the box it was fitted to, got {tall:?}"
        );
    }

    /// A frame with no size falls back to the image's intrinsic size.
    #[test]
    fn a_degenerate_frame_places_the_image_at_its_own_size() {
        let intrinsic = Size::new(64.0, 48.0);
        assert_eq!(fitted_image_size(intrinsic, Size::ZERO), intrinsic);
        assert_eq!(
            fitted_image_size(intrinsic, Size::new(-10.0, 100.0)),
            intrinsic
        );
    }

    /// The click point is the placed image's centre.
    #[test]
    fn a_placed_image_is_centred_on_the_click() {
        let mut ids = IdSource::new(1);
        let (node, parent) = (ids.mint(), ids.mint());
        let tx = create_image(
            node,
            parent,
            0,
            Affine::IDENTITY,
            Point::new(500.0, 400.0),
            Size::new(1000.0, 1000.0),
            ImageId("sha256:x".into()),
            Some(entry_of(200, 100)),
            Size::new(200.0, 100.0),
            None,
        );
        let Operation::CreateNode {
            kind, transform, ..
        } =
            tx.0.iter()
                .find(|o| matches!(o, Operation::CreateNode { .. }))
                .unwrap()
        else {
            unreachable!()
        };
        assert_eq!(
            *kind,
            NodeKind::Rect {
                size: Size::new(200.0, 100.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            "it fits, so it keeps its pixel size"
        );
        assert_eq!(
            transform.unwrap().translation(),
            Vec2::new(400.0, 350.0),
            "the click should be the centre of the placed box"
        );
    }

    /// The `NodeKind` a creation builder writes, for the tests below.
    fn made(tx: &Transaction) -> NodeKind {
        match tx
            .0
            .iter()
            .find(|o| matches!(o, Operation::CreateNode { .. }))
        {
            Some(Operation::CreateNode { kind, .. }) => kind.clone(),
            _ => panic!("a creation builder makes a node"),
        }
    }

    /// `create_polygon` and `create_star` clamp their count at the commit door as
    /// `step_sides` does at the keyboard door. Both are driven: different fields.
    ///
    /// flip: delete either `clamp` → fails at that builder's out-of-range case
    #[test]
    fn the_creation_builders_clamp_their_side_count_at_both_ends() {
        let mut ids = IdSource::new(1);
        let (parent, a, b) = (ids.mint(), Point::ZERO, Point::new(100.0, 100.0));
        let mut poly = |sides| {
            create_polygon(
                ids.mint(),
                parent,
                0,
                Affine::IDENTITY,
                a,
                b,
                sides,
                Color::BLACK,
            )
        };
        for (asked, want) in [
            (0, 3),
            (2, 3),
            (3, 3),
            (7, 7),
            (64, 64),
            (65, 64),
            (u32::MAX, 64),
        ] {
            match made(&poly(asked)) {
                NodeKind::Polygon { sides, .. } => assert_eq!(
                    sides, want,
                    "a polygon asked for {asked} sides must be made with {want}"
                ),
                other => panic!("create_polygon makes a polygon, not {other:?}"),
            }
        }

        let mut star = |points| {
            create_star(
                ids.mint(),
                parent,
                0,
                Affine::IDENTITY,
                a,
                b,
                points,
                Color::BLACK,
            )
        };
        for (asked, want) in [(0, 3), (2, 3), (5, 5), (65, 64), (u32::MAX, 64)] {
            match made(&star(asked)) {
                NodeKind::Star { points, .. } => assert_eq!(
                    points, want,
                    "a star asked for {asked} points must be made with {want}"
                ),
                other => panic!("create_star makes a star, not {other:?}"),
            }
        }

        // The only place `STAR_DEFAULT_RATIO` reaches a node.
        match made(&star(5)) {
            NodeKind::Star { inner_ratio, .. } => assert_eq!(inner_ratio, STAR_DEFAULT_RATIO),
            other => panic!("not {other:?}"),
        }
    }

    /// `create_image_in_box` neither fits nor locks the aspect: the layer is the box that
    /// was drawn, with a fixture `create_image` would shrink.
    ///
    /// flip: insert `fitted_image_size` into `create_image_in_box` → fails at the layer size
    #[test]
    fn a_dragged_image_takes_the_box_it_was_drawn_at() {
        let mut ids = IdSource::new(1);
        let (node, parent) = (ids.mint(), ids.mint());
        // A 400x200 image drawn into a 50x300 box: a fit would change both numbers.
        let intrinsic = Size::new(400.0, 200.0);
        let drawn = Size::new(50.0, 300.0);
        assert_ne!(
            fitted_image_size(intrinsic, drawn),
            drawn,
            "the fixture must be a box the fit rule would argue with, or this \
             test cannot see the difference it is about"
        );

        let tx = create_image_in_box(
            node,
            parent,
            0,
            Affine::IDENTITY,
            Point::new(10.0, 20.0),
            Point::new(60.0, 320.0),
            ImageId("sha256:x".into()),
            Some(entry_of(400, 200)),
            None,
        );
        match made(&tx) {
            NodeKind::Rect { size, .. } => assert_eq!(
                size, drawn,
                "the layer is the box that was drawn, at the aspect it was drawn"
            ),
            other => panic!("an image is placed as a Rect, not {other:?}"),
        }
    }

    /// `create_ellipse` passes the dragged box through to an `Ellipse`; the shared
    /// `local_box` is covered by `create_shape_box_is_order_independent`.
    #[test]
    fn an_ellipse_is_made_at_the_box_it_was_dragged() {
        let mut ids = IdSource::new(1);
        let tx = create_ellipse(
            ids.mint(),
            ids.mint(),
            0,
            Affine::IDENTITY,
            Point::new(60.0, 320.0),
            Point::new(10.0, 20.0),
            Color::BLACK,
        );
        match made(&tx) {
            NodeKind::Ellipse { size } => assert_eq!(size, Size::new(50.0, 300.0)),
            other => panic!("create_ellipse makes an ellipse, not {other:?}"),
        }
    }

    /// The transaction adds the image bytes before the node that refers to them, so its
    /// inverse removes the node first.
    #[test]
    fn placing_adds_the_image_before_the_layer_and_fills_with_it() {
        let mut ids = IdSource::new(2);
        let (node, parent) = (ids.mint(), ids.mint());
        let id = ImageId("sha256:y".into());
        let tx = create_image(
            node,
            parent,
            0,
            Affine::IDENTITY,
            Point::ZERO,
            Size::new(500.0, 500.0),
            id.clone(),
            Some(entry_of(10, 10)),
            Size::new(10.0, 10.0),
            Some("hero.png".into()),
        );
        assert!(
            matches!(tx.0[0], Operation::AddImage { .. }),
            "the bytes come first: {:?}",
            tx.0[0]
        );
        assert!(matches!(tx.0[1], Operation::CreateNode { .. }));
        let Operation::SetFills { fills, .. } = &tx.0[2] else {
            panic!("expected the fill last, got {:?}", tx.0[2]);
        };
        assert_eq!(fills[0].brush, ondin_core::image_brush(id).into());

        // The layer is named for the file.
        let Operation::CreateNode { name, .. } = &tx.0[1] else {
            unreachable!()
        };
        assert_eq!(name.as_deref(), Some("hero.png"));
    }

    /// Placing a file the document already carries adds a reference, not a second copy.
    #[test]
    fn placing_an_image_the_document_already_has_adds_no_second_copy() {
        let mut ids = IdSource::new(3);
        let (node, parent) = (ids.mint(), ids.mint());
        let id = ImageId("sha256:z".into());
        let tx = create_image(
            node,
            parent,
            0,
            Affine::IDENTITY,
            Point::ZERO,
            Size::new(500.0, 500.0),
            id.clone(),
            None,
            Size::new(10.0, 10.0),
            None,
        );
        assert_eq!(tx.0.len(), 2, "expected no AddImage: {:?}", tx.0);
        assert!(!tx.0.iter().any(|o| matches!(o, Operation::AddImage { .. })));
        let Operation::SetFills { fills, .. } = &tx.0[1] else {
            unreachable!()
        };
        assert_eq!(
            fills[0].brush,
            ondin_core::image_brush(id).into(),
            "the second layer still points at the one copy"
        );
    }
}

#[cfg(test)]
mod guide_scale_tests {
    use super::*;
    use ondin_core::{Guide, GuideAxis, GuideId, IdSource};

    /// A frame with one guide down it and one across it, both scoped to the frame.
    fn framed_guides() -> (Document, Resolved, NodeId, GuideId, GuideId) {
        let mut ids = IdSource::new(0x6D1DE);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let frame = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 200.0),
            },
            transform: None,
            name: None,
        }]))
        .unwrap();
        let (down, across) = (GuideId(ids.mint()), GuideId(ids.mint()));
        doc.apply(&Transaction(vec![
            Operation::AddGuide {
                guide: Guide {
                    id: down,
                    axis: GuideAxis::Vertical,
                    position: 200.0,
                    color: None,
                    owner: frame,
                },
            },
            Operation::AddGuide {
                guide: Guide {
                    id: across,
                    axis: GuideAxis::Horizontal,
                    position: 100.0,
                    color: None,
                    owner: frame,
                },
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, frame, down, across)
    }

    /// A resize holds a frame-scoped guide's offset; the Scale tool multiplies it by its own
    /// axis's factor, not the geometric mean, which would drift a guide off its feature.
    #[test]
    fn the_scale_tool_multiplies_a_frames_guides_and_a_resize_does_not() {
        let corner = Point::new(800.0, 200.0);

        let (mut doc, res, frame, down, across) = framed_guides();
        doc.apply(&resize_to_handle(
            &doc,
            &res,
            frame,
            Handle::BottomRight,
            corner,
            Resize::default(),
        ))
        .unwrap();
        assert_eq!(
            doc.guide(down).unwrap().position,
            200.0,
            "a resize leaves an absolute length alone (§5.6)"
        );
        assert_eq!(doc.guide(across).unwrap().position, 100.0);

        // The Scale tool.
        let (mut doc, res, frame, down, across) = framed_guides();
        doc.apply(&resize_to_handle(
            &doc,
            &res,
            frame,
            Handle::BottomRight,
            corner,
            Resize {
                scaling: Scaling::Photographic,
                ..Default::default()
            },
        ))
        .unwrap();
        assert_eq!(
            doc.guide(down).unwrap().position,
            400.0,
            "the vertical guide takes sx = 2, so the guide down the middle of a \
             400-wide frame is down the middle of an 800-wide one"
        );
        assert_eq!(
            doc.guide(across).unwrap().position,
            100.0,
            "the horizontal guide takes sy = 1 — not the geometric mean, which \
             would have moved it to 141.4 under a scale that did not touch its axis"
        );
    }

    /// A photographic scale whose geometric mean is exactly 1 still moves the guides: the
    /// `scaled_guides` call is not gated on `Scaling::scalar`, and each guide takes its own axis.
    ///
    /// flip: put the `scaled_guides` call behind the `scalar` guard → fails at the first assertion
    #[test]
    fn a_photographic_scale_with_a_mean_of_one_still_moves_the_guides() {
        // 400x200 -> 800x100: sx = 2, sy = 0.5, and sqrt(|2 * 0.5|) == 1.
        let corner = Point::new(800.0, 100.0);
        let (mut doc, res, frame, down, across) = framed_guides();
        doc.apply(&resize_to_handle(
            &doc,
            &res,
            frame,
            Handle::BottomRight,
            corner,
            Resize {
                scaling: Scaling::Photographic,
                ..Default::default()
            },
        ))
        .unwrap();

        assert_eq!(
            doc.guide(down).unwrap().position,
            400.0,
            "the vertical guide takes sx = 2 whatever the mean of the two factors is"
        );
        assert_eq!(
            doc.guide(across).unwrap().position,
            50.0,
            "and the horizontal one takes sy = 0.5 — each guide its own axis, \
             which is the half a single scalar cannot express"
        );
    }

    /// A guide on the frame's origin has nothing to scale, so scaling writes no operation for it.
    #[test]
    fn scaling_a_frame_writes_no_operation_for_a_guide_at_its_origin() {
        let mut ids = IdSource::new(0x6D1DE);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let frame = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 200.0),
            },
            transform: None,
            name: None,
        }]))
        .unwrap();
        let at_origin = GuideId(ids.mint());
        doc.apply(&Transaction(vec![Operation::AddGuide {
            guide: Guide {
                id: at_origin,
                axis: GuideAxis::Vertical,
                position: 0.0,
                color: None,
                owner: frame,
            },
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let tx = resize_to_handle(
            &doc,
            &res,
            frame,
            Handle::BottomRight,
            Point::new(800.0, 400.0),
            Resize {
                scaling: Scaling::Photographic,
                ..Default::default()
            },
        );
        assert!(
            !tx.0
                .iter()
                .any(|op| matches!(op, Operation::SetGuidePosition { .. })),
            "a guide on the origin needs no patch: {tx:?}"
        );
    }

    /// Scaling a frame leaves another frame's guides and global guides alone (the `owner` filter).
    #[test]
    fn scaling_a_frame_leaves_other_frames_and_global_guides_alone() {
        let mut ids = IdSource::new(0x6D1DE);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (scaled, other) = (ids.mint(), ids.mint());
        let board = |id, index| Operation::CreateNode {
            id,
            parent: root,
            index,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 200.0),
            },
            transform: None,
            name: None,
        };
        doc.apply(&Transaction(vec![board(scaled, 0), board(other, 1)]))
            .unwrap();
        let (elsewhere, global) = (GuideId(ids.mint()), GuideId(ids.mint()));
        let guide = |id, owner| Operation::AddGuide {
            guide: Guide {
                id,
                axis: GuideAxis::Vertical,
                position: 120.0,
                color: None,
                owner,
            },
        };
        doc.apply(&Transaction(vec![
            guide(elsewhere, other),
            guide(global, root),
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        doc.apply(&resize_to_handle(
            &doc,
            &res,
            scaled,
            Handle::BottomRight,
            Point::new(800.0, 400.0),
            Resize {
                scaling: Scaling::Photographic,
                ..Default::default()
            },
        ))
        .unwrap();
        assert_eq!(doc.guide(elsewhere).unwrap().position, 120.0);
        assert_eq!(doc.guide(global).unwrap().position, 120.0);
    }

    /// `scaled_layout` scales a grid's px lengths by their own axis (×2 across, ×0.5 down):
    /// tracks, `minmax()` ends, `repeat()` tracks, gaps and padding; `fr` and `%` stay.
    ///
    /// flip: the grid arm scales the columns by `csy` → fails at "the columns, across"
    #[test]
    fn a_grid_scales_its_tracks_gaps_and_padding_by_their_own_axis() {
        use ondin_core::container::{Display, Grid, Track, TrackBreadth as B, TrackSize as S};
        let (mut doc, _res, frame, ..) = framed_guides();
        let one = |b| Track::Size(S::Breadth(b));
        doc.apply(&Transaction(vec![Operation::SetDisplay {
            id: frame,
            display: Some(Display::Grid(Grid {
                columns: vec![
                    one(B::Px(100.0)),
                    one(B::Fr(1.0)),
                    Track::Repeat {
                        repeat: 2,
                        tracks: vec![S::MinMax {
                            min: B::Px(20.0),
                            max: B::Percent(10.0),
                        }],
                    },
                ],
                rows: vec![one(B::Px(50.0)), one(B::Percent(25.0))],
                column_gap: 10.0,
                row_gap: 20.0,
                padding: [1.0, 2.0, 3.0, 4.0],
                ..Default::default()
            })),
        }]))
        .unwrap();
        let Some(Operation::SetDisplay {
            display: Some(Display::Grid(g)),
            ..
        }) = scaled_layout(&doc, frame, 2.0, 0.5)
        else {
            panic!("a grid's SetDisplay");
        };
        assert_eq!(
            g.columns,
            vec![
                one(B::Px(200.0)),
                one(B::Fr(1.0)),
                Track::Repeat {
                    repeat: 2,
                    tracks: vec![S::MinMax {
                        min: B::Px(40.0),
                        max: B::Percent(10.0),
                    }],
                },
            ],
            "the columns, across"
        );
        assert_eq!(
            g.rows,
            vec![one(B::Px(25.0)), one(B::Percent(25.0))],
            "the rows, down"
        );
        assert_eq!((g.column_gap, g.row_gap), (20.0, 10.0));
        assert_eq!(g.padding, [0.5, 4.0, 1.5, 8.0]);
    }
}

/// `↑`/`↓` re-counting a polygon or star mid-draw (`docs/shortcuts.md` §9).
#[cfg(test)]
mod step_sides_tests {
    use super::{POLYGON_DEFAULT_SIDES, STAR_DEFAULT_POINTS, step_sides};
    use ondin_core::geometry::{MAX_SIDES, MIN_SIDES};

    /// One press is one side, in the direction the key points.
    #[test]
    fn each_press_is_one_side() {
        assert_eq!(step_sides(5, true, false), 6);
        assert_eq!(step_sides(5, false, true), 4);
        assert_eq!(step_sides(5, false, false), 5);
        // Both at once cancel.
        assert_eq!(step_sides(5, true, true), 5);
    }

    /// `↓` at the floor stays at the floor; a naive `u32` `sides - 1` would wrap.
    #[test]
    fn the_floor_holds_instead_of_wrapping() {
        assert_eq!(step_sides(MIN_SIDES, false, true), MIN_SIDES);
        assert_eq!(step_sides(POLYGON_DEFAULT_SIDES, false, true), MIN_SIDES);
        assert_eq!(step_sides(MIN_SIDES + 1, false, true), MIN_SIDES);
        // Zero is not reachable by the gesture but is what an uninitialised field holds.
        assert_eq!(step_sides(0, false, true), MIN_SIDES);
    }

    /// `↑` stops at the ceiling.
    #[test]
    fn the_ceiling_holds_too() {
        assert_eq!(step_sides(MAX_SIDES, true, false), MAX_SIDES);
        assert_eq!(step_sides(MAX_SIDES - 1, true, false), MAX_SIDES);
        assert_eq!(step_sides(MAX_SIDES + 100, true, false), MAX_SIDES);
    }

    /// Held down, both directions converge on their bound and stay there.
    #[test]
    fn holding_a_key_settles_at_the_bound() {
        let mut up = STAR_DEFAULT_POINTS;
        let mut down = STAR_DEFAULT_POINTS;
        for _ in 0..(MAX_SIDES * 2) {
            up = step_sides(up, true, false);
            down = step_sides(down, false, true);
        }
        assert_eq!(up, MAX_SIDES);
        assert_eq!(down, MIN_SIDES);
    }
}

#[cfg(test)]
mod overflow_tests {
    //! A resize whose product overflows, and the member that cannot take it.

    use super::*;
    use ondin_core::kurbo::{RoundedRectRadii, Size};
    use ondin_core::{Document, IdSource, NodeKind, Operation, Resolved, Transaction};

    /// Two rects in a frame: one storing `Affine::scale(1e308)` over a size of `1e-306`
    /// (all coefficients finite, world bounds `(0,0)–(100,100)`), and a plain 20×20 at x=80.
    fn a_pathological_member_and_an_ordinary_one() -> (Document, Resolved, NodeId, NodeId) {
        let mut ids = IdSource::new(0x5E1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (ab, a, b) = (ids.mint(), ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 400.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: a,
                parent: ab,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(1e-306, 1e-306),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::scale(1e308)),
                name: None,
            },
            Operation::CreateNode {
                id: b,
                parent: ab,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 20.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((80.0, 0.0))),
                name: None,
            },
        ]))
        .expect("the fixture is an ordinary, all-finite document");
        let res = Resolved::rebuild(&doc);
        (doc, res, a, b)
    }

    /// A member whose scaled basis overflows to `inf` sits the resize out alone: the ordinary
    /// rect still scales, the pathological one is unchanged, and the transaction applies.
    ///
    /// flip: `scaled_about`'s `then_some` replaced by `Some(out)` → fails at the `applied.is_ok()` assertion
    #[test]
    fn one_overflowing_member_does_not_refuse_the_whole_resize() {
        let (mut doc, res, a, b) = a_pathological_member_and_an_ordinary_one();
        let before = doc.get(a).expect("a is there").transform().as_coeffs();
        let union = [a, b]
            .iter()
            .filter_map(|id| res.world_bounds(*id))
            .reduce(|x, y| x.union(y))
            .expect("both resolve");
        assert_eq!(
            (union.width(), union.height()),
            (100.0, 100.0),
            "the fixture's union has to be ordinary, or the drag below is not one"
        );

        let tx = resize_selection(
            &doc,
            &res,
            &[a, b],
            SelectionBox::upright(union),
            Handle::BottomRight,
            Point::new(200.0, 200.0),
            Resize::geometry(false, false),
        );
        let applied = doc.apply(&tx);

        assert!(
            tx.0.iter().any(|op| matches!(
                op,
                Operation::SetGeometry { id, .. } | Operation::SetTransform { id, .. } if *id == b
            )),
            "the ordinary member has to be scaled — refusing per gesture is what \
             this fixes: {:?}",
            tx.0
        );
        assert_eq!(
            doc.get(a).expect("a survives").transform().as_coeffs(),
            before,
            "and the member that cannot take the scale is left exactly as it was"
        );
        assert!(
            applied.is_ok(),
            "the transaction has to reach the document: {applied:?}"
        );
    }

    /// The document the resize leaves behind still saves and loads. Coverage note: this
    /// passes without the per-member fix, because the op layer refuses non-finite transforms.
    #[test]
    fn the_document_a_resize_leaves_behind_still_loads() {
        let (mut doc, res, a, b) = a_pathological_member_and_an_ordinary_one();
        let union = [a, b]
            .iter()
            .filter_map(|id| res.world_bounds(*id))
            .reduce(|x, y| x.union(y))
            .expect("both resolve");
        let _ = doc.apply(&resize_selection(
            &doc.clone(),
            &res,
            &[a, b],
            SelectionBox::upright(union),
            Handle::BottomRight,
            Point::new(200.0, 200.0),
            Resize::geometry(false, false),
        ));
        let bytes = ondin_core::io::save(&doc).expect("it saves");
        assert!(
            ondin_core::io::load(&bytes).is_ok(),
            "a document the app just wrote has to be one the app can read"
        );
    }
}

#[cfg(test)]
mod unrepresentable_basis_tests {
    //! A member whose finite basis has no exact `orientation · scale` split, which gestures
    //! must leave alone; distinct from `overflow_tests`, whose basis overflows to `inf`.

    use super::*;
    use ondin_core::kurbo::{RoundedRectRadii, Size};
    use ondin_core::{Document, IdSource, NodeKind, Operation, Resolved, Transaction};

    /// One rect storing a rank-1 transform (both columns on `y = 2x`), which is finite and
    /// so stored by the op layer, beside an ordinary sibling.
    fn a_rank_one_member() -> (Document, Resolved, NodeId, NodeId) {
        let mut ids = IdSource::new(0x5E2);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (ab, a, b) = (ids.mint(), ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 400.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: a,
                parent: ab,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(40.0, 40.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::new([1.0, 2.0, 2.0, 4.0, 0.0, 0.0])),
                name: None,
            },
            // The ordinary sibling that still resizes.
            Operation::CreateNode {
                id: b,
                parent: ab,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 20.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((80.0, 0.0))),
                name: None,
            },
        ]))
        .expect("a rank-1 transform is finite, so the op layer stores it");
        let res = Resolved::rebuild(&doc);
        (doc, res, a, b)
    }

    /// A rotation or multi-select resize leaves a member with no exact basis split
    /// untouched instead of collapsing it, while its ordinary sibling still resizes.
    /// The determinant is pinned first so the fixture really is singular.
    ///
    /// flip: `Basis::of` back inside `split_geometry_scale` → fails at "a basis with no exact split refuses the turn"
    /// flip: the old splitter at `resize_selection`'s call site alone → fails at "the rank-1 member is untouched"
    #[test]
    fn a_rank_one_member_is_left_alone_rather_than_collapsed() {
        let (doc, res, a, b) = a_rank_one_member();
        let before = doc.get(a).expect("a is there").transform().as_coeffs();
        assert_eq!(
            Affine::new(before).determinant(),
            0.0,
            "the fixture is singular, or this test is about nothing"
        );

        let turned = rotate_node(&doc, &res, a, Point::new(0.0, 0.0), 0.5);
        assert!(
            turned.0.is_empty(),
            "a basis with no exact split refuses the turn; got {:?}",
            turned.0
        );

        let mut doc = doc.clone();
        let union = [a, b]
            .iter()
            .filter_map(|id| res.world_bounds(*id))
            .reduce(|x, y| x.union(y))
            .expect("both resolve");
        let b_before = doc.get(b).expect("b is there").transform().as_coeffs();
        let tx = resize_selection(
            &doc.clone(),
            &res,
            &[a, b],
            SelectionBox::upright(union),
            Handle::BottomRight,
            Point::new(400.0, 400.0),
            Resize::geometry(false, false),
        );
        doc.apply(&tx).expect("the surviving ops are all finite");
        assert_eq!(
            doc.get(a).expect("a is there").transform().as_coeffs(),
            before,
            "the rank-1 member is untouched rather than rewritten"
        );
        assert_ne!(
            doc.get(b).expect("b is there").transform().as_coeffs(),
            b_before,
            "and its ordinary sibling still resized"
        );
    }
}

#[cfg(test)]
mod rewrite_no_op_tests {
    //! `rewrite_path`'s no-op guard over a path the pen did not write: both sides are
    //! compared in the pen's canonical form (explicit closing segment, trimmed radii), or a
    //! click would become a `SetGeometry`.

    use super::*;
    use ondin_core::IdSource;

    /// The `<polygon>` shape as `svg_in` stores it: a closed path with an implicit closing segment.
    fn imported_triangle() -> BezPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((80.0, 0.0));
        p.line_to((80.0, 60.0));
        p.close_path();
        p
    }

    fn a_node() -> NodeId {
        IdSource::new(0x5A1).mint()
    }

    /// A click that changed nothing plans nothing over an imported closed path; the fixture
    /// assertion shows the pen round trip does move it.
    ///
    /// flip: replace the canonicalisation with a raw comparison against `current` → fails at "a click that changed nothing rewrote the geometry"
    #[test]
    fn a_no_op_over_an_imported_closed_path_plans_nothing() {
        let stored = imported_triangle();
        let subpaths = pen_anchors(&stored, &[]);

        assert_ne!(
            pen_path(&subpaths).elements(),
            stored.elements(),
            "the fixture: the round trip must move, or there is nothing here to guard"
        );

        let tx = rewrite_path(a_node(), Affine::IDENTITY, &subpaths, Some((&stored, &[])));
        assert_eq!(
            tx,
            Transaction(vec![]),
            "a click that changed nothing rewrote the geometry"
        );
    }

    /// A click over untrimmed stored radii plans nothing; the path is the pen's own
    /// spelling so the radii are the only difference, since `pen_radii` trims trailing zeros.
    #[test]
    fn a_no_op_over_untrimmed_stored_radii_plans_nothing() {
        let mut stored = BezPath::new();
        stored.move_to((0.0, 0.0));
        stored.line_to((80.0, 0.0));
        stored.line_to((80.0, 60.0));
        let radii = [0.0, 0.0, 0.0];
        let subpaths = pen_anchors(&stored, &radii);

        assert_eq!(
            pen_path(&subpaths).elements(),
            stored.elements(),
            "the fixture: an open subpath must round-trip exactly, so the radii \
             are the only difference left"
        );
        assert_ne!(
            pen_radii(&subpaths).as_slice(),
            radii.as_slice(),
            "the fixture: the trim must move the list, or there is nothing to guard"
        );

        let tx = rewrite_path(
            a_node(),
            Affine::IDENTITY,
            &subpaths,
            Some((&stored, &radii)),
        );
        assert_eq!(
            tx,
            Transaction(vec![]),
            "a click that changed nothing rewrote the radii"
        );
    }

    /// Control: a real radius edit still commits (a point move would not catch a guard that swallows radii).
    #[test]
    fn a_radius_actually_set_still_plans_a_write() {
        let stored = imported_triangle();
        let mut subpaths = pen_anchors(&stored, &[]);
        subpaths[0].anchors[1].radius = 4.0;

        let tx = rewrite_path(a_node(), Affine::IDENTITY, &subpaths, Some((&stored, &[])));
        assert_eq!(tx.0.len(), 1, "a real radius edit was swallowed: {tx:?}");
        assert!(matches!(
            tx.0[0],
            Operation::SetGeometry {
                geometry: GeometryPatch::Path { .. },
                ..
            }
        ));
    }
}

/// `side_shear`'s four edge arms and its Shift snap and clamp, asserted as a bargain: the
/// side facing the grabbed one does not move, and the grabbed one moves by the drag.
#[cfg(test)]
mod side_shear_tests {
    use super::*;
    use crate::preview::Handle;

    /// The corner of `box_` that `handle` grabs and the corner of the side facing it; corners
    /// rather than midpoints, since a midpoint of the held side stays put under any shear.
    fn probe(box_: Rect, handle: Handle) -> (Point, Point) {
        match handle {
            Handle::Top => (
                Point::new(box_.min_x(), box_.min_y()),
                Point::new(box_.min_x(), box_.max_y()),
            ),
            Handle::Bottom => (
                Point::new(box_.min_x(), box_.max_y()),
                Point::new(box_.min_x(), box_.min_y()),
            ),
            Handle::Left => (
                Point::new(box_.min_x(), box_.min_y()),
                Point::new(box_.max_x(), box_.min_y()),
            ),
            Handle::Right => (
                Point::new(box_.max_x(), box_.min_y()),
                Point::new(box_.min_x(), box_.min_y()),
            ),
            _ => unreachable!("only edges shear"),
        }
    }

    /// The middle of the side `handle` owns, where the press goes since the lean is measured from the press.
    fn grip(box_: Rect, handle: Handle) -> Point {
        match handle {
            Handle::Top => Point::new(box_.center().x, box_.min_y()),
            Handle::Bottom => Point::new(box_.center().x, box_.max_y()),
            Handle::Left => Point::new(box_.min_x(), box_.center().y),
            Handle::Right => Point::new(box_.max_x(), box_.center().y),
            _ => unreachable!("only edges shear"),
        }
    }

    /// Every edge leans its own side and holds the one facing it.
    ///
    /// flip: `Handle::Left`'s held edge moved to `box_.min_x()` → fails at "Left: the held side moved"
    /// flip: `Handle::Bottom`'s span sign reversed → fails at "Bottom: the grabbed side moved"
    #[test]
    fn every_edge_leans_its_own_side_and_holds_the_one_facing_it() {
        // 200 by 120, so a transposed span shows as a different magnitude.
        let box_ = Rect::new(0.0, 0.0, 200.0, 120.0);
        for handle in Handle::EDGES {
            let press = grip(box_, handle);
            let at = match handle {
                Handle::Top | Handle::Bottom => press + Vec2::new(20.0, 0.0),
                _ => press + Vec2::new(0.0, 20.0),
            };
            let shear = side_shear(box_, handle, SideDrag { press, at }, false)
                .unwrap_or_else(|| panic!("{handle:?} is an edge and must shear"));

            let (grabbed, held) = probe(box_, handle);
            let still = shear * held;
            assert!(
                (still - held).hypot() < 1e-9,
                "{handle:?}: the held side moved, by {:?}",
                still - held
            );
            let want = at - press;
            let moved = shear * grabbed;
            assert!(
                ((moved - grabbed) - want).hypot() < 1e-9,
                "{handle:?}: the grabbed side moved by {:?}, wanted {want:?}",
                moved - grabbed
            );
        }
    }

    /// Shift snaps the lean to 15° steps before the clamp; only a drag past the clamp tells
    /// the orders apart, and the unsnapped 18.43° case shows the snap really moved it.
    #[test]
    fn shift_snaps_a_lean_to_fifteen_degree_steps_before_the_clamp_pulls_it_in() {
        let box_ = Rect::new(0.0, 0.0, 200.0, 120.0);
        let press = Point::new(100.0, 0.0);
        // `skew_x` puts `tan(a)` in the matrix's xy slot.
        let lean_of = |a: Affine| a.as_coeffs()[2].atan();
        let drag = |dx: f64, snap: bool| {
            side_shear(
                box_,
                Handle::Top,
                SideDrag {
                    press,
                    at: press + Vec2::new(dx, 0.0),
                },
                snap,
            )
            .expect("an edge shears")
        };

        // 40 across a 120 drop is atan(1/3) = 18.43°. The sign is asserted: a rightward
        // drag is a negative `skew_x` here, and magnitude alone would pass a wrong lean.
        assert!(
            (lean_of(drag(40.0, false)).to_degrees() + 18.4349).abs() < 1e-3,
            "the fixture's own angle moved: {}",
            lean_of(drag(40.0, false)).to_degrees()
        );
        assert!(
            (lean_of(drag(40.0, true)).to_degrees() + 15.0).abs() < 1e-9,
            "Shift did not snap: {}",
            lean_of(drag(40.0, true)).to_degrees()
        );
        assert!(
            (lean_of(drag(100_000.0, true)) + MAX_SKEW).abs() < 1e-9,
            "the clamp did not run after the snap: {}",
            lean_of(drag(100_000.0, true)).to_degrees()
        );
    }

    /// A corner handle and a box too thin to lean have no lean.
    #[test]
    fn a_corner_and_a_slit_have_no_lean() {
        let box_ = Rect::new(0.0, 0.0, 200.0, 120.0);
        let drag = SideDrag {
            press: Point::new(0.0, 0.0),
            at: Point::new(20.0, 20.0),
        };
        assert!(side_shear(box_, Handle::TopLeft, drag, false).is_none());
        let slit = Rect::new(0.0, 0.0, 200.0, MIN_EXTENT * 0.5);
        assert!(side_shear(slit, Handle::Top, drag, false).is_none());
    }
}
