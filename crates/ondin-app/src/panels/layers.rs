//! The layers tree: z-order, structure, visibility and locking.
//!
//! Rows are painted by hand: a 26px row with hover-revealed controls and a size
//! badge is simpler to draw directly than to coax out of egui widget layout.
//!
//! Dragging a row reorders or reparents it through `build::reparent_preserving_world`,
//! not the raw `Reparent` op, which would teleport the node by the difference
//! between the two parents' transforms.
//!
//! The drop follows the indent: a gap between two rows is a run of insertion
//! points, one per level the two rows share, and the pointer's horizontal travel
//! from the pick-up point chooses between them. [`depth_range`] is the rule and
//! [`OndinApp::drop_hint`] is where it is read.

use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use eframe::egui;
use ondin_core::{NodeId, NodeKind, Operation, Transaction, build};
use std::collections::{HashMap, HashSet};

/// Row height. The rows do not touch: `item_spacing.y` is 1 in the tree, so the
/// pitch is 27 and every guide and drop line has a gap to cross or centre in.
const ROW_H: f32 = 26.0;

/// The header's left and right margin, in points (`i8` because `egui::Margin`
/// takes whole points). It does not govern a row: the header frame is a sibling of
/// the `ScrollArea`, and [`TREE_PAD_X`] is what indents the tree.
pub(crate) const PAD_X: i8 = 12;

/// The tree's left and right gutter, in points — the `ScrollArea`'s inner frame,
/// and so the term that actually narrows a row (§15 D444). A row at [`MIN_W`] runs
/// `6.0..204.0`, i.e. 198pt.
pub(crate) const TREE_PAD_X: i8 = 6;
/// Room left for the caret column on a row that has one, and on a top-level row
/// whether it has one or not.
const CARET_ROOM: f32 = 16.0;
/// [`CARET_ROOM`] for a row that gets none: a nested leaf.
const NO_CARET_ROOM: f32 = 2.0;
/// Half the kind glyph's 15pt box — `icon_x` is a *centre*.
const ICON_HALF: f32 = 8.0;
/// From the kind glyph's centre to where the name starts: half the glyph's 15pt
/// advance already sits to the right, leaving a 7pt gap.
const NAME_GAP: f32 = 15.0;
/// The lock glyph's centre, in from the row's right edge.
const LOCK_INSET: f32 = 14.0;

/// The room a component mark takes at a row's right edge (§15 D981) — the dot
/// for an override, `+` for a layer of the instance's own. With a mark, a badge's
/// right edge sits this far in from the row's edge rather than 4.
const MARK_SLOT: f32 = 9.0;

/// A row's component mark (§15 D981).
///
/// A removed or reordered child marks its parent's row, with a tooltip naming what
/// ([`child_drift_tip`]); *Restore children* in the row's menu is offered on
/// exactly those rows and puts back what the tooltip names (§15 D1003 (9)).
/// The scope is the row's fold (§15 D1004 (2)): collapsed, every removed child and
/// changed order anywhere in the subtree (`reset::subtree_child_drift`); expanded,
/// the row's own children only (`reset::child_drift`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowMark {
    /// An override — on the row, or, collapsed, anything inside it differing; and
    /// on a parent whose own children differ, either way.
    Override,
    /// A layer of the instance's own.
    Local,
}

/// The tooltip on a parent row whose children differ from its source's —
/// *"1 removed · order changed"* — or `None` when they do not.
pub(crate) fn child_drift_tip(d: ondin_core::reset::ChildDrift) -> Option<String> {
    let mut parts = Vec::new();
    if d.removed > 0 {
        parts.push(format!("{} removed", d.removed));
    }
    if d.order {
        parts.push("order changed".to_string());
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The tooltip a row with `mark` wears, `own` the units of its own overrides:
/// every mark has its words, since it steps aside while the row is hovered. A
/// parent whose children differ says how first.
fn mark_tip(
    mark: Option<RowMark>,
    children: ondin_core::reset::ChildDrift,
    own: usize,
) -> Option<String> {
    child_drift_tip(children).or_else(|| match mark? {
        RowMark::Local => Some("A layer of this instance's own".to_string()),
        RowMark::Override if own > 0 => Some(match own {
            1 => "1 override of its main".to_string(),
            n => format!("{n} overrides of its main"),
        }),
        RowMark::Override => Some("Something inside differs from its main".to_string()),
    })
}

/// What a row's tooltip counts and its *Restore children* restores, by its fold:
/// `folded`, the whole subtree; else `id`'s own children. Never a field.
pub(crate) fn child_drift_scoped(
    doc: &ondin_core::Document,
    id: NodeId,
    folded: bool,
) -> ondin_core::reset::ChildDrift {
    if folded {
        ondin_core::reset::subtree_child_drift(doc, id)
    } else {
        ondin_core::reset::child_drift(doc, id)
    }
}

/// Whether a picture row shows its thumbnail in the icon slot: not when the row
/// wears the hexagon (`Some` for a main or an instance root), its one component
/// mark (§15 D981). A free function because a headless app never decodes the
/// texture, so the row cannot be asked.
fn wears_thumbnail(hexagon: Option<bool>) -> bool {
    hexagon.is_none()
}

/// From the lock's centre to the eye's.
const EYE_PITCH: f32 = 23.0;

/// The capsule a bound layer's eye sits in (§15 D1060), clear of the lock glyph
/// by 4.5 at [`EYE_PITCH`].
const BOUND_EYE: egui::Vec2 = egui::vec2(22.0, 18.0);

/// Where a row's kind glyph is centred, measured from the row rect's left edge.
/// Shared with [`MIN_W`]'s argument so both use the numbers the row is drawn with.
fn icon_offset(depth: usize, caret_room: bool) -> f32 {
    indent_of(depth)
        + if caret_room {
            CARET_ROOM
        } else {
            NO_CARET_ROOM
        }
        + ICON_HALF
}

/// The size of the eye, lock and kind glyphs, in points.
const ICON_PT: f32 = 15.0;

/// The panel's width when nothing has been dragged — a default, not an exact size
/// (§15 D330, D349).
pub(crate) const DEFAULT_W: f32 = 300.0;

/// How narrow the drag may take the panel.
///
/// Derived from the row walk's own items: `w − 2·`[`PAD_X`]` − `[`icon_offset`]` −
/// `[`NAME_GAP`]` − `[`LOCK_INSET`]` − `[`EYE_PITCH`]` − `[`ICON_PT`]`/2` is the room a
/// hovered row's name lives in, so retuning any of them moves this rather than
/// silently invalidating it. Hovered, because at rest a row draws nothing on its
/// right and a minimum argued from that breaks when the pointer arrives.
///
/// 210 is chosen by one reading: a top-level row keeps 108.5pt of name while
/// hovered, so `Rectangle 12` (75.06) fits and `Button primary hover` (124.34)
/// runs on under the eye, the same names as
/// `panel_width_tests::the_minimum_keeps_a_real_layer_name_on_a_top_level_row`.
/// The other reading, a four-deep row's hovered name room reaching zero, crosses at
/// 194.5; 210 is left above it on purpose. Below this a nested row's name starts
/// under the eye's cover. The header is not binding: the search field flexes and the
/// eyebrow plus its three 20pt buttons want well under the 198pt of content.
pub(crate) const MIN_W: f32 = 210.0;

/// How wide the drag may take the panel: twice the default, so a panel dragged
/// over the whole window is not a state with no way out. At 600 a row eight deep
/// still has 281.5pt of name hovered (326 at rest, 322 with a `RowBadge`).
pub(crate) const MAX_W: f32 = 600.0;

/// The default has to sit inside the pair, or the panel opens at a width the user
/// cannot drag back to; a `const` assertion fails the build at the definition.
const _: () = assert!(MIN_W < DEFAULT_W && DEFAULT_W < MAX_W);

/// The side of a layer row's image thumbnail, in points: 16 matches the 15-point
/// glyph it replaces, so an image row is not taller than the rest. It clears the
/// caret by 5.5 and the name by 7; the row's own 26 would come within half a point
/// of the caret.
const THUMB: f32 = 16.0;
/// The thumbnail's corner radius, in points. Not shared with the inspector's chip
/// (4), whose radius belongs to `ui::swatch`.
const THUMB_RADIUS: u8 = 3;
/// The indent added per tree level. See [`indent_of`], where the arithmetic lives,
/// and [`caret_column`], which the guides hang off.
///
/// 31pt (§15 D331): at 16 a nested row was one caret-width
/// right of its parent and nothing told "inside" from "next to". Settled; the guides
/// are drawn from [`caret_column`] at whatever this says.
const INDENT: f32 = 31.0;
/// The tree guides' dash: 1.5pt of ink to every 2.5pt of gap.
///
/// Every run starts with a dash: `Shape::dashed_line` opens with ink, so a run
/// beginning at a junction (the elbow, or a row's top) puts ink through it. Phasing
/// the below-elbow run to continue the lattice left a visible gap at the corner.
const GUIDE_DASH: f32 = 1.5;
/// The gap between two of [`GUIDE_DASH`]'s dashes.
const GUIDE_GAP: f32 = 2.5;
/// How far clear of its trunk a row's elbow starts: one dash gap past the trunk's
/// own ink (half a point either side of its column), so the corner reads as the
/// pattern turning.
const ELBOW_CLEAR: f32 = 0.5 + GUIDE_GAP;
/// A tree guide at rest — 8.5% of the text colour. Below
/// [`theme::text`]'s dimmest tier (38%) so the gutter does not compete with names.
fn guide_rest() -> egui::Color32 {
    theme::color::text_a(22)
}
/// [`guide_rest`] lit: the trunk from the hovered row's parent down to its elbow,
/// at 34%.
fn guide_hover() -> egui::Color32 {
    theme::color::text_a(87)
}
/// How close to a row's edge counts as "drop between" rather than "drop into".
const EDGE_FRACTION: f32 = 0.3;
/// How far, in indents, the pointer has to leave the level the drag is holding
/// before the drop line moves to another one. See [`depth_asked_for`].
///
/// 0.65 rather than plain rounding's 0.5, which is where two levels are equally
/// close: a still or vertically moving hand sits on it and the line flickers. 0.65
/// (20pt) puts the band outside that jitter while a deliberate sideways move still
/// lands in under one indent.
const STICK: f32 = 0.65;
/// How long a drag has to rest on a collapsed group before it opens, in seconds.
/// Long enough that dragging past a group does not open it. The branch stays open:
/// it writes the same `collapsed` set a caret click does.
const SPRING_SECS: f64 = 1.0;

/// A layer row being dragged, and where it would land if released now.
pub struct LayerDrag {
    /// Every row being carried. Dragging a row that is part of the selection takes
    /// the whole selection, reduced to [`build::outermost`] so a group and
    /// something inside it do not move the inner node twice (§5.7a).
    pub ids: Vec<NodeId>,
    pub target: Option<DropTarget>,
    /// Where the pointer was when the drag started, and the depth of the grabbed
    /// row: the origin the indent gesture is measured from (see
    /// [`depth_asked_for`]).
    pub grab: (f32, usize),
    /// The level the indent gesture is holding — [`depth_asked_for`]'s answer from
    /// last frame, before the boundary's own range clamped it.
    pub level: usize,
    /// The container the drop would land in, as of last frame — the row that paints
    /// its name and glyph in [`color::select_ink`].
    ///
    /// A frame behind by necessity: rows are painted before the walk works out the
    /// drop ([`OndinApp::drop_hint`]). Not merged into [`target`], which is cleared
    /// every frame so that releasing off the tree drops nothing (§15 D19); this is
    /// for painting only and must never decide where a layer lands.
    ///
    /// [`target`]: LayerDrag::target
    pub lit: Option<NodeId>,
    /// The collapsed group the pointer is resting on, and the time it arrived —
    /// [`SPRING_SECS`] later the branch opens. `None` whenever the pointer is off
    /// one, so the wait restarts rather than accumulates.
    pub spring: Option<(NodeId, f64)>,
    /// Why the drop under the pointer is refused, when the reason is one kind per
    /// set (§15 D1003 (5), [`OndinApp::set_refusal`]) — said by the release.
    /// Cleared every frame with [`target`].
    ///
    /// [`target`]: LayerDrag::target
    pub refused: Option<&'static str>,
}

/// Where a dragged row would be inserted: a slot in one parent's child list.
/// The affordance is chosen from [`DropSpot`]; `plan_layer_move` only wants the
/// parent and the index.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DropTarget {
    pub parent: NodeId,
    pub index: usize,
}

/// The glyph for one boolean operation. Beside [`node_icon`] because the tree and
/// the inspector's dropdown both draw it and must show the same picture.
pub(crate) fn op_glyph(op: ondin_core::BoolOp) -> &'static str {
    match op {
        ondin_core::BoolOp::Union => icon::UNITE_SQUARE,
        ondin_core::BoolOp::Subtract => icon::SUBTRACT_SQUARE,
        ondin_core::BoolOp::Intersect => icon::INTERSECT_SQUARE,
        ondin_core::BoolOp::Exclude => icon::EXCLUDE_SQUARE,
    }
}

/// Whether any paint on `node` names a picture `store` cannot draw (§15 D179).
///
/// Asked of the decode cache rather than the document, so a dead link and bytes
/// that will not decode count as well as a dangling id. Fills and strokes both:
/// the row speaks for the layer, and a missing border picture is as broken as a
/// missing middle. Hidden paints are skipped.
pub(crate) fn picture_missing(store: &ondin_render::ImageStore, node: &ondin_core::Node) -> bool {
    let paint = node.paint();
    paint
        .fills
        .iter()
        .filter(|f| f.visible)
        .map(|f| &f.brush)
        .chain(paint.strokes.iter().filter(|s| s.visible).map(|s| &s.brush))
        // A bound paint is a solid, never a picture.
        .any(|b| match b.literal() {
            Some(ondin_core::Brush::Image(img)) => store.get(&img.image.id).is_none(),
            _ => false,
        })
}

/// The picture a row draws a thumbnail of: the first visible fill that is one.
///
/// Narrower than [`picture_missing`]: a stroke's picture is nothing in a small
/// square. The first fill rather than the topmost-opaque one, to avoid alpha
/// arithmetic here. It answers with the whole brush because [`thumb_tint`] needs
/// its alpha (§15 D784).
fn picture_of(node: &ondin_core::Node) -> Option<&ondin_core::ImageBrush> {
    node.paint()
        .fills
        .iter()
        .filter(|f| f.visible)
        .find_map(|f| match f.brush.literal() {
            Some(ondin_core::Brush::Image(img)) => Some(img),
            _ => None,
        })
}

/// The tint a row's thumbnail is drawn with: the row's dimming (the 40% a dimmed
/// glyph takes) and the picture's own alpha, composed rather than one winning
/// (§15 D784). A named function so a headless test can reach the decision.
fn thumb_tint(dimmed: bool, picture: Option<&ondin_core::ImageBrush>) -> egui::Color32 {
    let base = match dimmed {
        true => egui::Color32::from_white_alpha(102),
        false => egui::Color32::WHITE,
    };
    base.gamma_multiply(picture.map_or(1.0, crate::panels::paint::image_brush_alpha))
}

/// Half the width of a row control's hit band, either side of its centre x. One
/// number for click and hover, so a control cannot light up where it cannot be
/// pressed; [`RowTargets`] is the one answer both ask.
const CONTROL_HIT: f32 = 9.0;

/// Which of a row's three hand-drawn controls a point lands on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RowHit {
    Caret,
    Eye,
    Lock,
}

/// Where a row's controls are, and which of them it is showing.
///
/// `None` means the row is not drawing that one at all — a childless row has no
/// caret, and a resting visible row shows no eye — so it cannot be pressed and
/// must not light up either.
#[derive(Clone, Copy, Default)]
struct RowTargets {
    caret: Option<f32>,
    eye: Option<f32>,
    lock: Option<f32>,
}

impl RowTargets {
    /// The control at `x`, resolved caret, then eye, then lock — the click's own
    /// order, which matters where bands overlap on a deep, narrow panel.
    fn at(&self, x: f32) -> Option<RowHit> {
        let near = |c: &Option<f32>| c.is_some_and(|at| (x - at).abs() <= CONTROL_HIT);
        if near(&self.caret) {
            Some(RowHit::Caret)
        } else if near(&self.eye) {
            Some(RowHit::Eye)
        } else if near(&self.lock) {
            Some(RowHit::Lock)
        } else {
            None
        }
    }
}

/// The control a key on the focused row `resp` presses: `→` opens a folded group
/// and `←` folds an open one (the caret), `Ctrl+Shift+H` the eye, `Ctrl+Shift+L`
/// the lock, the chords hide and lock answer to everywhere. And the row `↑`/`↓`
/// step the focus by, which the tree moves (`OndinApp::tree_rows`).
fn row_keys(
    ui: &egui::Ui,
    resp: &egui::Response,
    has_children: bool,
    expanded: bool,
) -> (Option<RowHit>, i8) {
    let keys = crate::ui::arrow_keys(ui, resp, true, true);
    if has_children && (keys.x > 0 && !expanded || keys.x < 0 && expanded) {
        return (Some(RowHit::Caret), keys.y);
    }
    if !resp.has_focus() {
        return (None, 0);
    }
    let hit = ui.input(|i| {
        let chord = i.modifiers.command && i.modifiers.shift && !i.modifiers.alt;
        if chord && i.key_pressed(egui::Key::H) {
            Some(RowHit::Eye)
        } else if chord && i.key_pressed(egui::Key::L) {
            Some(RowHit::Lock)
        } else {
            None
        }
    });
    (hit, keys.y)
}

/// The one remark a resting row makes at its right-hand end.
enum RowBadge {
    /// An artboard's `800 × 600`.
    Text(String),
    /// An icon glyph and the type size that puts it on the word's ink height.
    /// The size travels with the glyph because it differs per glyph
    /// ([`BADGE_BOUND`], [`BADGE_BASE`], [`BADGE_MASK`]).
    Icon(&'static str, f32),
}

/// How far short of what the row draws on its right a long name starts to be
/// covered.
const NAME_CLEAR: f32 = 6.0;

/// What a row draws on its right, for [`name_limit`]: each is an x on the row, or
/// absent when this row does not draw it.
struct NameLimit {
    row_right: f32,
    /// A component mark at the row's very edge.
    mark: bool,
    /// The lock's centre, drawn hovered or locked.
    lock: Option<f32>,
    /// The eye's centre, and whether it wears the bound capsule.
    eye: Option<(f32, bool)>,
    /// Where a resting badge (a frame's size) begins.
    badge_left: Option<f32>,
}

/// Where a layer row's name room ends: short of the leftmost thing the row draws on
/// its right, and a point inside the row's own edge, which is the panel's. A name
/// running past it is covered from here ([`cover_row_end`]).
fn name_limit(l: NameLimit) -> f32 {
    let mut right = match l.mark {
        true => l.row_right - MARK_SLOT,
        false => l.row_right - 1.0,
    };
    if let Some(lock) = l.lock {
        right = right.min(lock - ICON_PT / 2.0 - NAME_CLEAR);
    }
    if let Some((eye, bound)) = l.eye {
        let half = if bound { BOUND_EYE.x } else { ICON_PT } / 2.0;
        right = right.min(eye - half - NAME_CLEAR);
    }
    if let Some(badge) = l.badge_left {
        right = right.min(badge - NAME_CLEAR);
    }
    right
}

/// How far a name fades out before the row's pinned controls.
const NAME_FADE: f32 = 24.0;
/// The solid ground a covered name leaves before the controls, on top of
/// [`NAME_CLEAR`].
const COVER_PAD: f32 = 6.0;

/// The tint a row lays over the panel: dragged, selected, hovered, or none.
fn row_tint(dragged: bool, selected: bool, hovered: bool) -> Option<egui::Color32> {
    if dragged {
        Some(theme::color::text_a(20))
    } else if selected {
        Some(color::SELECT_ROW)
    } else if hovered {
        // The list rows' one hover ground (`ui::list_row_ground`).
        Some(color::hover())
    } else {
        None
    }
}

/// What a row looks like under its tint, opaque, for covering a name with.
fn row_ground(dragged: bool, selected: bool, hovered: bool) -> egui::Color32 {
    let panel = color::panel();
    row_tint(dragged, selected, hovered).map_or(panel, |t| panel.blend(t))
}

/// Hide a name that runs on under the row's pinned controls: the row's own ground
/// from [`COVER_PAD`] before `from` to the right of `row`, the row's part in view,
/// faded in over [`NAME_FADE`] before that.
fn cover_row_end(p: &egui::Painter, row: egui::Rect, from: f32, ground: egui::Color32) {
    let from = from - COVER_PAD;
    let solid = egui::Rect::from_min_max(egui::pos2(from, row.top()), row.right_bottom());
    p.rect_filled(
        solid,
        egui::CornerRadius {
            ne: 5,
            se: 5,
            ..Default::default()
        },
        ground,
    );
    // A point into the solid, over its anti-aliased left edge, or the name shows
    // through the seam between the two.
    let fade = egui::Rect::from_min_max(
        egui::pos2(from - NAME_FADE, row.top()),
        egui::pos2(from + 1.0, row.bottom()),
    );
    let clear = egui::Color32::TRANSPARENT;
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(fade.left_top(), clear);
    mesh.colored_vertex(egui::pos2(from, row.top()), ground);
    mesh.colored_vertex(egui::pos2(from, row.bottom()), ground);
    mesh.colored_vertex(fade.left_bottom(), clear);
    mesh.colored_vertex(fade.right_top(), ground);
    mesh.colored_vertex(fade.right_bottom(), ground);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    mesh.add_triangle(1, 4, 5);
    mesh.add_triangle(1, 5, 2);
    p.add(egui::Shape::mesh(mesh));
}

/// Type size for the subtract base's `diamond`, so it inks the same height as the
/// worded badge beside it (`800 × 600` at 10.5). Measured from the galley's
/// `uv_rect`; the two match at 100%–200% scaling (pixel snapping makes the
/// neighbouring quarter points miss by a row), and
/// `the_base_badge_inks_as_tall_as_the_worded_one` holds it. It matches the word,
/// not the 15pt hover controls: a badge is an annotation.
const BADGE_BASE: f32 = 9.75;

/// Type size for a bound layer's `property-bound`, measured the same way.
const BADGE_BOUND: f32 = 11.0;

/// Type size for a mask's `mask`, measured the same way; at 10.0 it inks
/// the word's height at all four scalings (11.0 is too tall at 125% and 150%).
const BADGE_MASK: f32 = 10.0;

/// Whether `node` is the shape a `Subtract` cuts into — its base operand, the
/// one thing about a boolean the tree shows without saying (§15 D244). Operand
/// order is child order (§15 D113) and only `Subtract` reads it, so the base is
/// `children[0]`, on the tree's bottom row. Asked of the parent, because being a
/// base is not a property of the shape.
fn subtract_base(doc: &ondin_core::Document, node: &ondin_core::Node) -> bool {
    let Some(parent) = node.parent().and_then(|p| doc.get(p)) else {
        return false;
    };
    matches!(
        parent.kind(),
        NodeKind::Boolean {
            op: ondin_core::BoolOp::Subtract
        }
    ) && parent.children().first() == Some(&node.id())
}

/// The layers-tree icon (glyph + resting colour) for a node kind.
pub(crate) fn node_icon(kind: &NodeKind) -> (&'static str, egui::Color32) {
    let (glyph, ink) = node_glyph(kind);
    (glyph, ink.colour())
}

/// A kind glyph's resting ink as a role, read against the palette when painted,
/// so a caller that keeps it across frames follows a theme change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GlyphInk {
    Muted,
    Accent,
}

impl GlyphInk {
    pub(crate) fn colour(self) -> egui::Color32 {
        match self {
            Self::Muted => theme::text::muted(),
            Self::Accent => color::accent_400(),
        }
    }
}

/// [`node_icon`] with the ink as its role.
pub(crate) fn node_glyph(kind: &NodeKind) -> (&'static str, GlyphInk) {
    use GlyphInk::{Accent, Muted};
    match kind {
        NodeKind::Root => (icon::SELECTION, Muted),
        NodeKind::Artboard { .. } => (icon::FRAME, Accent),
        NodeKind::Group => (icon::SELECTION, Muted),
        NodeKind::Rect { .. } => (icon::SQUARE, Muted),
        NodeKind::Ellipse { .. } => (icon::CIRCLE, Muted),
        NodeKind::Polygon { .. } => (icon::TRIANGLE, Muted),
        NodeKind::Star { .. } => (icon::STAR, Muted),
        NodeKind::Line { .. } => (icon::LINE_SEGMENT, Muted),
        NodeKind::Path { .. } => (icon::POLYGON, Accent),
        NodeKind::Text { .. } => (icon::TEXT_T, Muted),
        NodeKind::Slice { .. } => (icon::KNIFE, Muted),
        // Accent, like `Path`, with the operation's own glyph: the name can be
        // renamed away, the icon cannot.
        NodeKind::Boolean { op } => (op_glyph(*op), Accent),
    }
}

/// Every layer on `page` whose name contains `needle`, case folded, top to bottom
/// as the layers panel lists them (children reversed, parents before their
/// children).
///
/// The one name-match rule: the panel's filter keeps these plus their ancestors,
/// and the command palette lists them. The page's root can match; callers that
/// list layers drop it.
pub(crate) fn name_hits(doc: &ondin_core::Document, page: NodeId, needle: &str) -> Vec<NodeId> {
    let needle = needle.to_lowercase();
    let mut out = Vec::new();
    let mut stack = vec![page];
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get(id) else {
            continue;
        };
        if node.name().to_lowercase().contains(&needle) {
            out.push(id);
        }
        // Pushed in child order so the last child, the panel's top row, pops first.
        stack.extend(node.children().iter().copied());
    }
    out
}

/// The egui id of a row's rename field, stable across frames so focus can be
/// asked for before the widget exists. `pub(crate)` because `Ctrl+R`, `F2` and the
/// context menu open a rename from outside this file (§15 D216).
pub(crate) fn rename_field_id(id: NodeId) -> egui::Id {
    egui::Id::new(("layer-rename", id))
}

/// The egui id of a row's widget: its layer's, so it holds still however the tree
/// scrolls ([`OndinApp::tree_rows`]).
fn row_id(id: NodeId) -> egui::Id {
    egui::Id::new(("layer-row", id))
}

/// Where the [`crate::ui::inline_edit`] rename field goes on a row whose name is
/// painted from `left`: the full row height, starting
/// [`crate::ui::INLINE_EDIT_PAD_X`] left of the name, so the text does not shift
/// when the rename opens.
fn rename_field_rect(row: egui::Rect, left: f32) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(left - crate::ui::INLINE_EDIT_PAD_X, row.top()),
        egui::pos2(row.right() - 8.0, row.bottom()),
    )
}

/// The operations a layer-tree drop turns into, and whether any of them crossed
/// a parent boundary (which is only used to word the status line).
pub(crate) struct MovePlan {
    pub ops: Vec<Operation>,
    pub reparented: bool,
}

/// Plan the drop of `moving` into `target`, as one list of operations.
///
/// Every index is computed against the list the previous operations leave behind
/// (as `build::z_order` does), so the touched child lists are simulated. A free
/// function so it can be tested without an `OndinApp`.
pub(crate) fn plan_layer_move(
    doc: &ondin_core::Document,
    res: &ondin_core::Resolved,
    moving: &[NodeId],
    target: DropTarget,
) -> Result<MovePlan, ondin_core::OpError> {
    let mut sim: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    let list_for = |sim: &mut HashMap<NodeId, Vec<NodeId>>, p: NodeId| {
        sim.entry(p).or_insert_with(|| {
            doc.get(p)
                .map(|n| n.children().to_vec())
                .unwrap_or_default()
        });
    };
    list_for(&mut sim, target.parent);

    let mut ops = Vec::new();
    let mut reparented = false;
    let mut slot = target.index;
    for id in moving {
        let Some(from) = doc.get(*id).and_then(|n| n.parent()) else {
            continue;
        };
        list_for(&mut sim, from);
        let Some(at_from) = sim[&from].iter().position(|c| c == id) else {
            continue;
        };
        sim.get_mut(&from).expect("just listed").remove(at_from);
        // Lifting a row out of the destination list ahead of the drop slot
        // shifts everything after it down by one.
        if from == target.parent && at_from < slot {
            slot -= 1;
        }
        let to = sim.get_mut(&target.parent).expect("listed first");
        let at = slot.min(to.len());
        to.insert(at, *id);

        if from == target.parent {
            if at != at_from {
                ops.push(Operation::Reorder { id: *id, index: at });
            }
        } else {
            reparented = true;
            ops.extend(build::reparent_preserving_world(doc, res, *id, target.parent, at)?.0);
        }
        slot = at + 1;
    }
    // A plan whose composition is the identity is no plan (§15 D532): the per-row
    // test above misses a run dropped right after itself, and no per-operation
    // guard can see it. Only when nothing was reparented, since `sim` does not
    // model a reparent's transform ops.
    if !reparented
        && sim
            .iter()
            .all(|(p, list)| doc.get(*p).is_some_and(|n| n.children() == list.as_slice()))
    {
        ops.clear();
    }
    Ok(MovePlan { ops, reparented })
}

/// Which edge of a row an insertion line sits on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RowEdge {
    Top,
    Bottom,
}

/// Width of the insertion line, in points. 1pt is a hairline the eye has to hunt for
/// on the most important thing a drag says; the 1pt "inside" outline is already the
/// heavier mark by area.
const DROP_LINE_PT: f32 = 2.0;

/// Where the insertion line goes: the middle of the `gap` between the two rows the
/// boundary separates, not either row's own edge. The rows do not touch, so a
/// boundary has two y coordinates one `item_spacing.y` apart;
/// `upper.bottom() + gap/2` and `lower.top() - gap/2` are the same number.
/// [`boundary_y`] asks for only one edge per boundary, but using `rect.top()` would
/// move the line a pixel at every boundary.
fn drop_line_y(rect: egui::Rect, edge: RowEdge, gap: f32) -> f32 {
    match edge {
        RowEdge::Top => rect.top() - gap / 2.0,
        RowEdge::Bottom => rect.bottom() + gap / 2.0,
    }
}

/// The y of screen boundary `boundary`, the gap above `rows[boundary]`: `0` is the
/// top of the list and `rows.len()` the bottom, so `n` rows have `n + 1`.
///
/// Naming a boundary from the screen gives exactly one line per boundary; two
/// strokes at one y composite into a heavier line, and two lines around one row
/// read as the "inside" box (§9.4). It is also the unit [`depth_range`] needs.
fn boundary_y(rows: &[TreeRow], boundary: usize, gap: f32) -> Option<f32> {
    match boundary.checked_sub(1) {
        // Below the row above it — which is every boundary but the topmost.
        Some(above) => rows
            .get(above)
            .map(|r| drop_line_y(r.rect, RowEdge::Bottom, gap)),
        None => rows.first().map(|r| drop_line_y(r.rect, RowEdge::Top, gap)),
    }
}

/// A horizontal band of `width` points centred on `y`, snapped to whole device
/// pixels: the centre and the width to hand the painter, so every boundary draws the
/// same line (as `ui::rule` does). Parity matters: an even pixel count wants its
/// centre on a pixel boundary, an odd one in the middle of a pixel.
fn snap_across(y: f32, width: f32, ppp: f32) -> (f32, f32) {
    let px = (width * ppp).round().max(1.0);
    let centre = if (px as i64) % 2 == 0 {
        (y * ppp).round()
    } else {
        (y * ppp).floor() + 0.5
    };
    (centre / ppp, px / ppp)
}

/// The same colour at a fixed alpha, for the 40% treatment a locked or hidden
/// row gets. `Color32` is premultiplied, so the components are rescaled with the
/// alpha rather than overwriting `a`.
fn dim_to(c: egui::Color32, alpha: u8) -> egui::Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    egui::Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

/// How far a row at this depth is pushed in, before its caret's own column. Depth
/// 1 is a top-level row — the root is implicit and has no row of its own.
fn indent_of(depth: usize) -> f32 {
    6.0 + (depth as f32 - 1.0) * INDENT
}

/// The x a row at this depth centres its caret on. Named because the tree guides
/// run down it: a level-`n` trunk is drawn in its descendants' rows, which must
/// agree with the caret about the column.
fn caret_column(left: f32, depth: usize) -> f32 {
    left + indent_of(depth) + 5.0
}

/// Where a row's elbow stops: short of its leading glyph (caret, else kind icon).
/// Two points off an 11pt caret box and four off a 15pt icon's (an icon
/// glyph's ink stops well inside its box).
fn row_lead(left: f32, depth: usize, has_children: bool) -> f32 {
    if has_children {
        caret_column(left, depth) - 7.5
    } else {
        // A childless row reclaims the caret's room except at the top level.
        left + icon_offset(depth, depth == 1) - 11.5
    }
}

/// The depth the pointer is asking for, before the tree gets a say: one level per
/// [`INDENT`] moved sideways from where the drag was picked up.
///
/// Relative to the grab, never absolute: a row is picked up by its name, which at
/// depth 1 already starts 45pt in, so an absolute reading would jump a level before
/// the pointer moved.
///
/// `held` is the level this returned last frame, giving hysteresis: the level only
/// changes once the pointer is [`STICK`] of an indent from the held one, then
/// re-rounds to the nearest. It must be this function's own answer, not the clamped
/// one the drop used, or a one-level gap would re-origin the gesture. Floored at 1
/// only because the arithmetic runs through a `usize`; [`drop_spot`] clamps into
/// [`depth_range`].
fn depth_asked_for(grab_depth: usize, grab_x: f32, x: f32, held: usize) -> usize {
    let raw = grab_depth as f32 + (x - grab_x) / INDENT;
    let level = if (raw - held as f32).abs() > STICK {
        raw.round()
    } else {
        held as f32
    };
    level.max(1.0) as usize
}

/// The depths a drop at screen boundary `boundary` may take, shallowest first.
///
/// This is the whole of indent-aware dropping: a gap offers one insertion point per
/// level the two rows share.
///
/// - The shallowest is the depth of the row below, a hard floor (anything shallower
///   would close containers the rows below are still inside). The last boundary has
///   no row below and bottoms out at the top level.
/// - The deepest is the depth of the row above, or the floor if that is deeper. That
///   covers "drop in as first child" without recording whether a container is open:
///   an expanded container's children are drawn directly below it, and a collapsed
///   one offers only its own level (the outline on its row is the way in, §9.4).
///
/// The ends never cross; when the floor is deeper it becomes both ends.
fn depth_range(rows: &[TreeRow], boundary: usize) -> std::ops::RangeInclusive<usize> {
    let above = boundary.checked_sub(1).and_then(|i| rows.get(i));
    let below = rows.get(boundary);
    // No row below is the bottom of the tree (top level only); no row above is the
    // top, where the first row's own level is the only offer.
    let shallowest = below.map_or(1, |b| b.depth);
    let deepest = above.map_or(shallowest, |a| a.depth).max(shallowest);
    shallowest..=deepest
}

/// Where in the tree the pointer is, before the document says whether the drop is
/// legal. Pure screen geometry; [`OndinApp::drop_hint`] turns one into a
/// [`DropTarget`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DropSpot {
    /// Inside the container row at this index in `rows`, at the top of its list.
    Into(usize),
    /// Between rows, at [`boundary_y`]'s boundary and the given depth.
    Between { boundary: usize, depth: usize },
}

/// What one frame's pointer says: where the drop would go, and the level the indent
/// gesture is now holding. `spot`'s depth is clamped into what the boundary offers;
/// `level` is not, and is what feeds [`depth_asked_for`] next frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct DropRead {
    spot: DropSpot,
    level: usize,
}

/// Read the pointer against the drawn rows: which boundary it is at, or which
/// container it is inside.
///
/// [`EDGE_FRACTION`] of a container row at each end means "between", the middle
/// "inside" (§9.4). Horizontally the pointer may leave the panel to the left: the
/// whole row is the drag handle, so a gutter grab is a few points from the edge and
/// one [`INDENT`] of leftward travel leaves the panel; refusing it would make the
/// outdent work from some grab points only
/// (`the_outdent_still_works_when_the_pointer_has_left_the_panel`). To the right
/// the rect bounds it.
fn drop_spot(
    rows: &[TreeRow],
    pointer: egui::Pos2,
    grab: (f32, usize),
    held: usize,
) -> Option<DropRead> {
    let (grab_x, grab_depth) = grab;
    // The first row the pointer has not passed the bottom of, not the row it is
    // inside: containment would leave a dead band in each inter-row gap. A gap picks
    // the row below, whose `t` is negative. Off the top of the list is not a drop.
    let (i, row) = rows
        .iter()
        .enumerate()
        .find(|(_, r)| pointer.y <= r.rect.bottom())?;
    if i == 0 && pointer.y < row.rect.top() {
        return None;
    }
    if pointer.x > row.rect.right() {
        return None;
    }
    let t = (pointer.y - row.rect.top()) / row.rect.height();
    if row.container && t > EDGE_FRACTION && t < 1.0 - EDGE_FRACTION {
        // The level is carried through: zeroing it would re-origin the gesture.
        return Some(DropRead {
            spot: DropSpot::Into(i),
            level: held,
        });
    }
    // A row's top edge is the boundary of the same index; its bottom edge is the next one.
    let boundary = if t < 0.5 { i } else { i + 1 };
    let range = depth_range(rows, boundary);
    let level = depth_asked_for(grab_depth, grab_x, pointer.x, held);
    Some(DropRead {
        spot: DropSpot::Between {
            boundary,
            depth: level.clamp(*range.start(), *range.end()),
        },
        level,
    })
}

/// Where a row's elbow crosses it: the y the horizontal run sits on and where the
/// vertical is split. Snapped to a whole device pixel here, not in the painter, so
/// the two runs meet rather than sit half a pixel out of true.
fn elbow_y(rect: egui::Rect, ppp: f32) -> f32 {
    snap_across(rect.center().y, 1.0, ppp).0
}

/// One drawn row, as everything painted after the walk sees it.
///
/// Collected during the walk and read after it, because every guide is a statement
/// about other rows (an ancestor's trunk, a row below, the hovered row), and the
/// drop hint needs the rows above and below a boundary ([`depth_range`]). Guides
/// are drawn over the row grounds, the drop hint over both.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TreeRow {
    /// The layer this row draws. Only the drop pass reads it.
    id: NodeId,
    /// The row's own box.
    rect: egui::Rect,
    /// 1 for a top-level row, which has no guides at all.
    depth: usize,
    /// Where the elbow has to stop — see [`tree_guide_segments`].
    lead: f32,
    /// Where the elbow crosses the row, [`elbow_y`].
    elbow: f32,
    selected: bool,
    hovered: bool,
    /// A `Group`, `Artboard` or `Boolean` — something the middle of whose row means
    /// "inside it". Openness is read off the row below ([`depth_range`]).
    container: bool,
    /// A container with children that are not on screen — the one thing the drop
    /// pass cannot work out from the row list (a closed container looks like a
    /// leaf), and what spring-loading acts on.
    closed: bool,
}

static NEXT_FOLD_STAMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The layer-tree rows the user has folded, stamped afresh on every change so
/// [`FlatTree`] can tell when to rebuild. The stamps are drawn from one counter, so
/// a set swapped in from a parked tab never repeats one the cache holds.
#[derive(Clone, Debug)]
pub(crate) struct Folds {
    set: HashSet<NodeId>,
    stamp: u64,
}

impl Default for Folds {
    fn default() -> Self {
        Self {
            set: HashSet::new(),
            stamp: NEXT_FOLD_STAMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }
}

impl Folds {
    fn touch(&mut self) {
        self.stamp = NEXT_FOLD_STAMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn contains(&self, id: &NodeId) -> bool {
        self.set.contains(id)
    }

    // Each write restamps only when the set changed: a selection change unfolds
    // every ancestor, and a stamp moved for nothing rebuilds the flattened tree.
    pub(crate) fn insert(&mut self, id: NodeId) -> bool {
        let changed = self.set.insert(id);
        if changed {
            self.touch();
        }
        changed
    }

    pub(crate) fn remove(&mut self, id: &NodeId) -> bool {
        let changed = self.set.remove(id);
        if changed {
            self.touch();
        }
        changed
    }

    pub(crate) fn extend(&mut self, ids: impl IntoIterator<Item = NodeId>) {
        let before = self.set.len();
        self.set.extend(ids);
        if self.set.len() != before {
            self.touch();
        }
    }

    pub(crate) fn clear(&mut self) {
        if !self.set.is_empty() {
            self.touch();
        }
        self.set.clear();
    }

    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl Iterator<Item = &NodeId> {
        self.set.iter()
    }
}

/// One row of the flattened tree: what the drawing needs of it before any widget
/// exists.
#[derive(Clone, Copy, Debug)]
struct FlatRow {
    id: NodeId,
    /// 1 for a top-level row; the page is not a row.
    depth: usize,
    has_children: bool,
    /// A `Group`, `Artboard` or `Boolean` (see [`TreeRow::container`]).
    container: bool,
    expanded: bool,
}

/// What the cached rows were flattened from: the document revision (unique across
/// sessions), the page, the folds' stamp and the search.
type FlatKey = (u64, NodeId, u64, Option<String>);

/// The layers tree flattened into rows, top to bottom, rebuilt only when its key
/// changes: the walk is linear in the document, and a frame draws only the rows in
/// view.
#[derive(Default)]
pub(crate) struct FlatTree {
    key: Option<FlatKey>,
    rows: std::rc::Rc<[FlatRow]>,
    /// Each row's index in `rows`.
    index: HashMap<NodeId, usize>,
    /// Whether every container is folded, keyed without the search, which does
    /// not change the answer.
    all_folded: Option<((u64, NodeId, u64), bool)>,
}

/// A dashed run of tree guide, and what it is saying.
#[derive(Clone, Copy, Debug, PartialEq)]
struct GuideSeg {
    from: egui::Pos2,
    to: egui::Pos2,
    ink: GuideInk,
}

/// What a run of guide is saying — and, read as a ranking, which claim wins where
/// two of them cover one run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GuideInk {
    /// Structure and nothing else.
    Rest,
    /// On the path to the row under the pointer. Beats [`Selected`]: hover is a
    /// question asked now, a selection persists.
    ///
    /// [`Selected`]: GuideInk::Selected
    Hovered,
    /// On the path to a selected row: `color::SELECT`, the blue of the row's kind
    /// glyph, at full strength (a dashed hairline inks 37% of its length). A
    /// selected row under the pointer keeps this.
    Selected,
}

impl GuideInk {
    fn colour(self) -> egui::Color32 {
        match self {
            Self::Rest => guide_rest(),
            Self::Hovered => guide_hover(),
            Self::Selected => color::SELECT,
        }
    }
}

/// What one row's guides run over: the level its parent's trunk is on, and the
/// half-open range of rows above it that trunk crosses. The range stops before the
/// row itself (the trunk below its elbow belongs to later siblings). `None` for a
/// top-level row. `parents` is [`guide_parents`] of the same rows.
fn guide_path(
    rows: &[TreeRow],
    parents: &[Option<usize>],
    row: usize,
) -> Option<(usize, std::ops::Range<usize>)> {
    let level = rows[row].depth.checked_sub(1).filter(|l| *l >= 1)?;
    // The parent's own row — or, in a filtered tree whose parent is absent, the row
    // itself, which then claims nothing above it.
    let start = parents[row].map_or(row, |p| p + 1);
    Some((level, start..row))
}

/// For each row, the nearest row above it one level up — its parent's row — in
/// one pass: a row at that depth further up would be the parent's sibling, which
/// comes after the parent's whole branch.
fn guide_parents(rows: &[TreeRow]) -> Vec<Option<usize>> {
    let mut last: Vec<Option<usize>> = Vec::new();
    rows.iter()
        .enumerate()
        .map(|(i, r)| {
            let parent = r
                .depth
                .checked_sub(1)
                .and_then(|l| last.get(l).copied().flatten());
            if last.len() <= r.depth {
                last.resize(r.depth + 1, None);
            }
            last[r.depth] = Some(i);
            parent
        })
        .collect()
}

/// For each row, the levels whose trunk carries on *below* it — bit `n - 1` for
/// level `n`.
///
/// The trunk of a level-`n` ancestor carries on past a row when that ancestor has
/// another child after the branch this row sits in: scanning down, the first row at
/// depth `n + 1` or less is at exactly `n + 1`.
///
/// One pass up the list, so nesting cannot make this quadratic: a row at depth `d`
/// sets the bit for level `d - 1` and clears every level from `d` down. Levels past
/// 64 are not tracked (they start 1959pt from the panel's edge).
fn trunks_below(rows: &[TreeRow]) -> Vec<u64> {
    let mut out = vec![0u64; rows.len()];
    let mut mask = 0u64;
    for (i, row) in rows.iter().enumerate().rev() {
        out[i] = mask;
        let d = row.depth.clamp(1, 64);
        mask &= (1u64 << (d - 1)) - 1;
        if d >= 2 {
            mask |= 1 << (d - 2);
        }
    }
    out
}

/// Every dashed run the tree guides need, for rows in the order they are drawn
/// (§15 D331): the trunks (one per ancestor whose branch is still open, down the
/// level's [`caret_column`]), the connector (the parent's trunk above this row's
/// elbow) and the elbow (a horizontal run out to `lead`).
///
/// A trunk whose branch has already closed is not drawn: a stub for every level
/// would leave a fragment under a line that stopped a row earlier.
///
/// Hovering lights only the path from the parent to the row's elbow, not the whole
/// ancestry, and a selection lights the same path in `color::SELECT`. Where the two
/// overlap hover wins, one run at a time (see [`GuideInk`]).
///
/// Only the rows in `emit` get runs — the ones in view; the rest of `rows` is read
/// for the trunks and paths that cross them.
fn tree_guide_segments(rows: &[TreeRow], gap: f32, emit: std::ops::Range<usize>) -> Vec<GuideSeg> {
    let below = trunks_below(rows);
    let parents = guide_parents(rows);
    // A hovered row that is itself selected has no hover path.
    let hovered = rows
        .iter()
        .position(|r| r.hovered && !r.selected)
        .and_then(|h| guide_path(rows, &parents, h).map(|(l, over)| (h, l, over)));
    // Every selected row whose path reaches `emit`: two selected siblings light two
    // legs off one trunk.
    let selected: Vec<(usize, usize, std::ops::Range<usize>)> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.selected)
        .filter_map(|(i, _)| guide_path(rows, &parents, i).map(|(l, over)| (i, l, over)))
        .filter(|(i, _, over)| emit.contains(i) || (over.start < emit.end && emit.start < over.end))
        .collect();
    // Is this run on that path? The whole trunk for a row the path merely crosses,
    // and only the connector for the row it ends at.
    let on = |(owner, plevel, over): &(usize, usize, std::ops::Range<usize>),
              i: usize,
              level: usize,
              connector: bool| {
        *plevel == level && (over.contains(&i) || (connector && i == *owner))
    };
    let mut out = Vec::new();
    for (i, row) in rows.iter().enumerate().take(emit.end).skip(emit.start) {
        let (top, elbow) = (row.rect.top(), row.elbow);
        let trunk_ink = |level: usize, connector: bool| {
            if hovered.as_ref().is_some_and(|p| on(p, i, level, connector)) {
                GuideInk::Hovered
            } else if selected.iter().any(|p| on(p, i, level, connector)) {
                GuideInk::Selected
            } else {
                GuideInk::Rest
            }
        };
        for level in 1..row.depth.min(64) {
            let x = caret_column(row.rect.left(), level);
            let carries_on = below[i] & (1 << (level - 1)) != 0;
            if level + 1 == row.depth {
                // The parent's trunk: drawn down to the elbow whether the branch
                // continues past this row or ends on it.
                out.push(GuideSeg {
                    from: egui::pos2(x, top),
                    to: egui::pos2(x, elbow),
                    ink: trunk_ink(level, true),
                });
                if carries_on {
                    out.push(GuideSeg {
                        from: egui::pos2(x, elbow),
                        to: egui::pos2(x, row.rect.bottom() + gap),
                        ink: trunk_ink(level, false),
                    });
                }
            } else if carries_on {
                out.push(GuideSeg {
                    from: egui::pos2(x, top),
                    to: egui::pos2(x, row.rect.bottom() + gap),
                    ink: trunk_ink(level, false),
                });
            }
        }
        if row.depth >= 2 {
            // The elbow belongs to this row alone, so a lit path is a line, not a comb.
            let x = caret_column(row.rect.left(), row.depth - 1);
            out.push(GuideSeg {
                from: egui::pos2(x + ELBOW_CLEAR, elbow),
                to: egui::pos2(row.lead, elbow),
                ink: match hovered {
                    Some((h, ..)) if h == i => GuideInk::Hovered,
                    _ if row.selected => GuideInk::Selected,
                    _ => GuideInk::Rest,
                },
            });
        }
    }
    out
}

/// Paint what [`tree_guide_segments`] worked out. The across-the-run device-pixel
/// snap lives here rather than in the geometry (a 1pt line on a whole point smudges
/// across two pixels); the elbow's own y is snapped in [`elbow_y`], so for a
/// horizontal run this is idempotent.
fn paint_tree_guides(painter: &egui::Painter, rows: &[TreeRow], gap: f32, ppp: f32) {
    // A dashed run is a shape per dash, so a run outside the clip is skipped here
    // rather than left for the tessellator to cull.
    let clip = painter.clip_rect().expand(1.0);
    // A row's runs stay within its own pitch, so only the rows crossing the clip
    // can have one in it.
    let first = rows.partition_point(|r| r.rect.bottom() + gap < clip.top());
    let end = rows.partition_point(|r| r.rect.top() <= clip.bottom());
    for seg in tree_guide_segments(rows, gap, first..end.max(first)) {
        if !clip.intersects(egui::Rect::from_two_pos(seg.from, seg.to)) {
            continue;
        }
        let vertical = seg.from.x == seg.to.x;
        let across = if vertical { seg.from.x } else { seg.from.y };
        // 1pt wide.
        let (across, width) = snap_across(across, 1.0, ppp);
        let (from, to) = if vertical {
            (egui::pos2(across, seg.from.y), egui::pos2(across, seg.to.y))
        } else {
            (egui::pos2(seg.from.x, across), egui::pos2(seg.to.x, across))
        };
        painter.extend(egui::Shape::dashed_line(
            &[from, to],
            egui::Stroke::new(width, seg.ink.colour()),
            GUIDE_DASH,
            GUIDE_GAP,
        ));
    }
}

impl OndinApp {
    pub(crate) fn layers_tree(&mut self, ui: &mut egui::Ui) {
        // Re-derive the drop target each frame so releasing off the tree drops
        // nothing. The container it named is kept one frame in `lit` for the walk's
        // ink (see [`LayerDrag::lit`]).
        if let Some(drag) = self.layer_drag.as_mut() {
            drag.lit = drag.target.map(|t| t.parent);
            drag.target = None;
            drag.refused = None;
            ui.ctx().request_repaint();
        }
        self.sync_tree_to_selection();
        // An empty search box is not a search.
        let needle = self
            .layer_filter
            .as_ref()
            .map(|f| f.trim().to_lowercase())
            .filter(|f| !f.is_empty());
        let searching = needle.is_some();
        let flat = self.flat_rows(needle);

        // Measured afresh by the rows in view; the scroll area lays out next frame's.
        let was_w = std::mem::take(&mut self.layers_tree_w);
        // Every row, for the guides and the drop hint ([`TreeRow`]).
        let drawn = if searching && flat.is_empty() {
            ui.add_space(6.0);
            crate::ui::empty_note(ui, "No layers match");
            Vec::new()
        } else {
            self.tree_rows(ui, &flat)
        };
        if self.layers_tree_w != was_w {
            ui.ctx().request_repaint();
        }
        if self.prefs.tree_guides {
            // Kept off the gutter beside the rows' tint, as the rows are.
            let clip = ui.painter().clip_rect();
            let band = egui::Rect::from_x_y_ranges(self.tree_band_x(clip), clip.y_range());
            paint_tree_guides(
                &ui.painter().with_clip_rect(band.intersect(clip)),
                &drawn,
                ui.spacing().item_spacing.y,
                ui.ctx().pixels_per_point(),
            );
        }
        // After the guides, so the hint is on top of them.
        self.drop_hint(ui, &drawn);

        // Only the primary button resolves the drag (§15 D581): a middle click
        // mid-drag must not land the drop. The secondary button is answered by
        // `cancel_gesture` in `OndinApp::ui`.
        if ui.input(|i| i.pointer.primary_released()) {
            self.finish_layer_drag();
        }
    }

    /// What a click on a row selects, given the modifiers: Shift takes a range,
    /// Ctrl/Cmd toggles one row. The canvas's Shift adds and removes instead, as
    /// there is no order to take a range along. The range runs along the rows as
    /// displayed: collapsed and filtered-out rows are not in it.
    fn select_from_row(&mut self, id: NodeId, ui: &egui::Ui) {
        let (shift, ctrl) =
            ui.input(|i| (i.modifiers.shift, i.modifiers.ctrl || i.modifiers.command));
        if ctrl && !shift {
            self.session.selection.toggle(id);
            self.layers_range_anchor = Some(id);
            return;
        }
        if shift {
            let flat = &self.layers_flat;
            let anchor = self
                .layers_range_anchor
                .and_then(|a| flat.index.get(&a).copied());
            let here = flat.index.get(&id).copied();
            if let (Some(a), Some(b)) = (anchor, here) {
                let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                let range = flat.rows[lo..=hi].iter().map(|r| r.id).collect();
                self.session.selection.set(range);
                // The anchor stays put so the range can be stretched either way.
                return;
            }
            // No anchor (first Shift-click, or it was collapsed away): toggle.
            self.session.selection.toggle(id);
            self.layers_range_anchor = Some(id);
            return;
        }
        self.session.selection.set_one(id);
        self.layers_range_anchor = Some(id);
    }

    /// The rows the tree is showing, top to bottom, flattened once per change of
    /// [`FlatKey`] and shared by the drawing and a Shift-click's range, so a range
    /// cannot take a row the user cannot see.
    fn flat_rows(&mut self, needle: Option<String>) -> std::rc::Rc<[FlatRow]> {
        let key = (
            self.session.revision(),
            self.session.page,
            self.collapsed.stamp,
            needle,
        );
        if self.layers_flat.key.as_ref() != Some(&key) {
            let shown = key.3.as_deref().map(|n| self.matching_rows(n));
            let mut rows = Vec::new();
            self.flatten(self.session.page, 0, shown.as_ref(), &mut rows);
            self.layers_flat.index = rows.iter().enumerate().map(|(i, r)| (r.id, i)).collect();
            self.layers_flat.rows = rows.into();
            self.layers_flat.key = Some(key);
        }
        self.layers_flat.rows.clone()
    }

    fn flatten(
        &self,
        id: NodeId,
        depth: usize,
        shown: Option<&HashSet<NodeId>>,
        out: &mut Vec<FlatRow>,
    ) {
        if let Some(rows) = shown
            && !rows.contains(&id)
        {
            return;
        }
        let Some(node) = self.session.doc.get(id) else {
            return;
        };
        let children: Vec<NodeId> = match shown {
            Some(rows) => node
                .children()
                .iter()
                .copied()
                .filter(|c| rows.contains(c))
                .collect(),
            None => node.children().to_vec(),
        };
        if depth > 0 {
            let has_children = !children.is_empty();
            // A filter forces every branch open.
            let expanded = has_children && (shown.is_some() || !self.collapsed.contains(&id));
            out.push(FlatRow {
                id,
                depth,
                has_children,
                // A boolean takes a drop like a group does: its children are its
                // operands.
                container: matches!(
                    node.kind(),
                    NodeKind::Group | NodeKind::Artboard { .. } | NodeKind::Boolean { .. }
                ),
                expanded,
            });
            if !expanded {
                return;
            }
        }
        // Reversed: `children` is z-order with the last entry nearest the viewer
        // (§5.3). Every index this file computes stays a model index; only the
        // walk is flipped, so the drop-target maths mirrors it.
        for child in children.into_iter().rev() {
            self.flatten(child, depth + 1, shown, out);
        }
    }

    /// Lay the tree out: one allocation for the whole column, a [`TreeRow`] for
    /// every row by arithmetic, and widgets and ink only for the rows in view.
    ///
    /// Each row's widget id is its layer's ([`row_id`]), not the layout's next auto
    /// id, which would shift with the first row in view and so drop a held drag.
    /// Two kinds of row outside the view still take theirs: the one being renamed,
    /// painted so its field keeps focus, and a dragged one, so the drag survives
    /// scrolling it away.
    fn tree_rows(&mut self, ui: &mut egui::Ui, flat: &[FlatRow]) -> Vec<TreeRow> {
        if flat.is_empty() {
            return Vec::new();
        }
        let gap = ui.spacing().item_spacing.y;
        let pitch = ROW_H + gap;
        let ppp = ui.ctx().pixels_per_point();
        let width = ui.available_width();
        let (_, column) = ui.allocate_space(egui::vec2(width, flat.len() as f32 * pitch - gap));
        let mut rows: Vec<TreeRow> = flat
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let rect = egui::Rect::from_min_size(
                    egui::pos2(column.left(), column.top() + i as f32 * pitch),
                    egui::vec2(width, ROW_H),
                );
                TreeRow {
                    id: f.id,
                    rect,
                    depth: f.depth,
                    lead: row_lead(rect.left(), f.depth, f.has_children),
                    elbow: elbow_y(rect, ppp),
                    selected: false,
                    hovered: false,
                    container: f.container,
                    closed: f.container && f.has_children && !f.expanded,
                }
            })
            .collect();
        let mut topmost = None::<usize>;
        for id in self.session.selection.ids() {
            if let Some(&i) = self.layers_flat.index.get(id) {
                rows[i].selected = true;
                topmost = Some(topmost.map_or(i, |t| t.min(i)));
            }
        }
        if let Some(i) = topmost {
            self.scroll_to_selected_row(ui, rows[i].rect);
        }

        let index = &self.layers_flat.index;
        let clip = ui.clip_rect();
        let first = rows.partition_point(|r| r.rect.bottom() < clip.top());
        let end = rows
            .partition_point(|r| r.rect.top() <= clip.bottom())
            .max(first);
        let mut paint: Vec<usize> = (first..end).collect();
        if let Some((n, _)) = &self.renaming_layer
            && let Some(&i) = index.get(n)
            && !paint.contains(&i)
        {
            paint.push(i);
        }
        // The row the keyboard is on, too, so scrolling it away keeps its focus.
        let focused = ui.ctx().memory(|m| m.focused());
        if let Some(i) = focused.and_then(|f| flat.iter().position(|r| row_id(r.id) == f))
            && !paint.contains(&i)
        {
            paint.push(i);
        }
        if let Some(drag) = &self.layer_drag {
            for id in &drag.ids {
                if let Some(&i) = index.get(id)
                    && !paint.contains(&i)
                {
                    ui.interact(rows[i].rect, row_id(*id), egui::Sense::click_and_drag());
                }
            }
        }
        let stamp = self.collapsed.stamp;
        for i in paint {
            self.layers_row(ui, flat[i], &mut rows[i]);
        }
        // A focused row's `↑`/`↓`: the next row may be out of view and undrawn, so
        // it is registered here, focused and scrolled to. Registered first: AccessKit
        // panics on a focus that names no widget of this frame.
        if let Some((from, step)) = self.layers_focus_step.take()
            && let Some(i) = flat.iter().position(|r| r.id == from)
        {
            // The nearest row that way whose layer is still there, else the other way
            // after a delete: one also takes an open group's own rows.
            let doc = &self.session.doc;
            let alive = |j: usize| doc.get(flat[j].id).is_some();
            let nearest = |step: isize| {
                let mut j = i;
                loop {
                    let k = j.saturating_add_signed(step).min(flat.len() - 1);
                    if k == j {
                        return None;
                    }
                    j = k;
                    if alive(j) {
                        return Some(j);
                    }
                }
            };
            let step = isize::from(step);
            let to = nearest(step).or_else(|| (!alive(i)).then(|| nearest(-step)).flatten());
            if let Some(j) = to {
                let to = row_id(flat[j].id);
                ui.interact(rows[j].rect, to, egui::Sense::click_and_drag());
                ui.memory_mut(|m| m.request_focus(to));
                ui.scroll_to_rect(rows[j].rect, None);
            }
        }
        // A caret clicked this frame folds rows the walk has already placed.
        if self.collapsed.stamp != stamp {
            ui.ctx().request_repaint();
        }
        rows
    }

    /// The in-place rename field, laid over the row where its name was.
    ///
    /// Buffered, so one rename is one undo step. Enter and clicking away keep it;
    /// Escape or an empty name discards it.
    fn layer_rename_field(&mut self, ui: &mut egui::Ui, id: NodeId, row: egui::Rect, left: f32) {
        let Some((_, text)) = &self.renaming_layer else {
            return;
        };
        let mut buf = text.clone();
        // Focus is requested here, once, on the frame the field first draws:
        // requesting it when a door opens the rename hands AccessKit a
        // focused id with no node, and `accesskit_consumer` panics. It must stay a
        // one-shot; re-requesting whenever focus is absent would regrab it on the
        // frame Enter or Escape takes it away.
        let fid = rename_field_id(id);
        if crate::ui::widget_rect(ui.ctx(), fid).is_none() {
            ui.ctx().memory_mut(|m| m.request_focus(fid));
        }
        // Sits where the painted name sat — see `rename_field_rect`.
        let field = rename_field_rect(row, left);
        let resp = crate::ui::inline_edit(ui, field, &mut buf, fid);
        if let Some((_, stored)) = &mut self.renaming_layer {
            *stored = buf.clone();
        }

        // A press anywhere else ends the rename: hand-painted surfaces never take
        // focus themselves. Gated on the field holding focus, so the double-click
        // that opened it cannot close it.
        if resp.has_focus() {
            let pressed_elsewhere = ui.input(|i| i.pointer.any_pressed())
                && ui
                    .ctx()
                    .pointer_interact_pos()
                    .is_none_or(|p| !field.contains(p));
            if pressed_elsewhere {
                ui.ctx()
                    .memory_mut(|m| m.surrender_focus(rename_field_id(id)));
            }
        }

        // Escape is read before the commit: once focus is gone it is the only
        // thing saying which of the two happened.
        if resp.lost_focus() {
            let cancelled = ui.input(|i| i.key_pressed(egui::Key::Escape));
            let name = buf.trim().to_string();
            self.renaming_layer = None;
            if !cancelled
                && !name.is_empty()
                && self.session.doc.get(id).is_some_and(|n| n.name() != name)
            {
                self.session.name_step("Rename", icon::CURSOR_TEXT);
                self.session
                    .commit(Transaction(vec![Operation::SetName { id, name }]));
            }
        }
    }

    /// Follow a selection made somewhere else — almost always the canvas: on the
    /// frame it changed, open what it is buried in and ask to scroll to it. Only a
    /// change of selection re-opens anything; a frame the user collapsed with the
    /// selection inside stays closed.
    fn sync_tree_to_selection(&mut self) {
        let current = self.session.selection.ids().to_vec();
        if current == self.layers_synced_selection {
            return;
        }
        self.layers_synced_selection = current.clone();
        for id in &current {
            // The whole chain, no early exit: an open frame can sit inside a closed one.
            let mut cursor = self.session.doc.get(*id).and_then(|n| n.parent());
            while let Some(p) = cursor {
                self.collapsed.remove(&p);
                cursor = self.session.doc.get(p).and_then(|n| n.parent());
            }
        }
        self.scroll_layers_to_selection = !current.is_empty();
    }

    // The drag-to-edge autoscroll lives in `app::layers_panel`, not here (§15 D576).

    /// Fold every container, or unfold them all when nothing is folded.
    pub(crate) fn toggle_collapse_all(&mut self) {
        let containers = self.tree_containers();
        let anything_open = containers.iter().any(|id| !self.collapsed.contains(id));
        if anything_open {
            self.collapsed.extend(containers);
        } else {
            self.collapsed.clear();
        }
    }

    /// Fold every container, whatever state the tree is in. Unlike
    /// [`Self::toggle_collapse_all`] it is unconditional, which
    /// `prefs::Prefs::collapse_groups_on_open` needs right after
    /// `reset_transient_state` empties `collapsed`.
    pub(crate) fn collapse_all(&mut self) {
        let containers = self.tree_containers();
        self.collapsed.extend(containers);
    }

    /// Every node in the tree that has children, root excluded — the things a
    /// caret can fold.
    fn tree_containers(&self) -> Vec<NodeId> {
        let root = self.session.page;
        ondin_core::subtree_nodes(&self.session.doc, &[root])
            .into_iter()
            .filter(|id| {
                *id != root
                    && self
                        .session
                        .doc
                        .get(*id)
                        .is_some_and(|n| !n.children().is_empty())
            })
            .collect()
    }

    /// Whether every container in the tree is folded — what the collapse-all
    /// button reads to decide which way it points. Goes through
    /// [`Self::tree_containers`] so the glyph and [`Self::toggle_collapse_all`]'s
    /// action cannot disagree about what counts as a container (§15 D674); it is the
    /// exact negation of `anything_open`.
    ///
    /// Asked every frame by the header, so the answer is kept until the document,
    /// the page or the folds change.
    pub(crate) fn tree_is_all_collapsed(&mut self) -> bool {
        let key = (
            self.session.revision(),
            self.session.page,
            self.collapsed.stamp,
        );
        if let Some((k, folded)) = self.layers_flat.all_folded
            && k == key
        {
            return folded;
        }
        let folded = self
            .tree_containers()
            .iter()
            .all(|id| self.collapsed.contains(id));
        self.layers_flat.all_folded = Some((key, folded));
        folded
    }

    /// Rows to show for a name filter: every node whose name matches, plus the
    /// ancestors needed to reach it. Non-matching descendants are dropped.
    fn matching_rows(&self, needle: &str) -> HashSet<NodeId> {
        let doc = &self.session.doc;
        let mut keep = HashSet::new();
        for id in name_hits(doc, self.session.page, needle) {
            keep.insert(id);
            let mut cursor = doc.get(id).and_then(|n| n.parent());
            // Stop at the first ancestor already recorded: whenever one was
            // added, its own ancestors were added with it.
            while let Some(p) = cursor {
                if !keep.insert(p) {
                    break;
                }
                cursor = doc.get(p).and_then(|n| n.parent());
            }
        }
        keep
    }

    /// One row in view: its widget, its ink and what a press on it does. `row` is
    /// its [`TreeRow`], placed by [`Self::tree_rows`]; the row records whether it is
    /// selected and hovered for the guides.
    fn layers_row(&mut self, ui: &mut egui::Ui, f: FlatRow, row: &mut TreeRow) {
        let FlatRow {
            id,
            depth,
            has_children,
            expanded,
            ..
        } = f;
        let Some(node) = self.session.doc.get(id) else {
            return;
        };
        // What the layer shows, a bound visibility resolved (§15 D1059); the eye's
        // click writes the opposite, which detaches the binding.
        let visible = self.session.doc.shown(id);
        // Bound to a boolean variable, the eye stays drawn in the capsule (§15 D1060).
        let bound_eye = node.visible_var().is_some();
        let locked = node.locked();
        let name = node.name().to_string();
        let (glyph, mut glyph_col) = node_icon(node.kind());
        // A main or an instance wears the hexagon in place of its kind's glyph (§15
        // D981): filled for a main (`ui::paint_hexagon_filled`), outline for an
        // instance, neutral selected or not.
        let hexagon = if node.component() {
            Some(true)
        } else if ondin_core::component::instance_root(&self.session.doc, id) == Some(id) {
            Some(false)
        } else {
            None
        };
        let glyph = if hexagon.is_some() {
            icon::HEXAGON
        } else {
            glyph
        };
        // A layer whose picture cannot be drawn wears the warning colour (§15 D179),
        // and so does a boolean the arithmetic gave up on (§15 D239). That is
        // `boolean_failed`, never "the outline is missing": an empty intersection is
        // on purpose.
        if picture_missing(self.canvas.images(), node) || self.session.resolved.boolean_failed(id) {
            glyph_col = theme::color::warn();
        }
        // Cloned so the node's borrow ends before the painting below.
        let picture = picture_of(node).cloned();
        let is_slice = matches!(node.kind(), NodeKind::Slice { .. });
        // The right-hand end of a resting row: one slot (the one the eye and lock
        // take over on hover) for several remarks, in priority order — mask (a mask
        // draws nothing, so its row is the only sign the layer exists), a folded
        // set's variant count, `{}` for a component-property binding (§15 D982),
        // then a frame's size or a subtract's base.
        let doc = &self.session.doc;
        let set_count = (node.set().is_some() && self.collapsed.contains(&id))
            .then(|| ondin_core::variant::variants(doc, id).len());
        let bound = ondin_core::variant::owner_above(doc, id)
            .and_then(|o| doc.get(o))
            .is_some_and(|o| o.props().iter().any(|p| p.bound.contains(&id)));
        let badge = if node.mask() {
            Some(RowBadge::Icon(icon::MASK, BADGE_MASK))
        } else if let Some(n) = set_count {
            Some(RowBadge::Text(match n {
                1 => "1 variant".to_string(),
                n => format!("{n} variants"),
            }))
        } else if bound {
            Some(RowBadge::Icon(icon::PROPERTY_BOUND, BADGE_BOUND))
        } else {
            // The frame's used size (§15 D868): where container layout put it.
            match self.session.resolved.used_kind_of(node) {
                NodeKind::Artboard { size, .. } => Some(RowBadge::Text(format!(
                    "{} × {}",
                    size.width as i64, size.height as i64
                ))),
                _ => subtract_base(&self.session.doc, node)
                    .then_some(RowBadge::Icon(icon::DIAMOND, BADGE_BASE)),
            }
        };
        // A glyph badge's words, for the row's tooltip: hovered, the eye and lock
        // take its slot, so the badge itself is never under the pointer.
        let badge_tip = match &badge {
            Some(RowBadge::Icon(g, _)) if *g == icon::MASK => Some("A mask"),
            Some(RowBadge::Icon(g, _)) if *g == icon::PROPERTY_BOUND => {
                Some("Bound to a component property")
            }
            Some(RowBadge::Icon(g, _)) if *g == icon::DIAMOND => Some("The subtraction's base"),
            _ => None,
        };
        // (node borrow ends here — safe to mutate self below)

        let selected = self.session.selection.contains(id);
        // The row's component mark (§15 D981): `+` for a layer of an instance's
        // own, a dot for an override — on the holding row while expanded, on a
        // collapsed row for anything inside differing from its main. A parent
        // whose children differ (removed or reordered) carries the dot too, with
        // a tooltip naming what; collapsed, it counts the whole subtree (§15
        // D1003 (9), D1004 (2), `child_drift_of`).
        let (mark, children_tip) = {
            let doc = &self.session.doc;
            let in_instance = doc
                .get(id)
                .and_then(|n| n.parent())
                .and_then(|p| ondin_core::component::instance_root(doc, p))
                .is_some();
            let local = in_instance
                && doc.get(id).is_some_and(|n| {
                    n.link()
                        .is_none_or(|l| doc.get(l).is_some_and(|l| l.component()))
                });
            let copy = in_instance || hexagon == Some(false);
            let children = if copy && !local {
                self.child_drift_of(id)
            } else {
                Default::default()
            };
            let own: usize = ondin_core::reset::overrides(&self.session.doc, id)
                .iter()
                .map(|o| o.units)
                .sum();
            let mark = if local {
                Some(RowMark::Local)
            } else if has_children && !expanded && copy {
                let d = self.drift_of(id);
                (d.any() || d.local > 0).then_some(RowMark::Override)
            } else {
                (children.any() || own > 0).then_some(RowMark::Override)
            };
            (mark, mark_tip(mark, children, own))
        };

        let rect = row.rect;
        let resp = ui.interact(rect, row_id(id), egui::Sense::click_and_drag());
        // The mark's and the badge's words, on the row they sit on.
        let tip = match (&children_tip, badge_tip) {
            (Some(m), Some(b)) => Some(format!("{b}\n{m}")),
            (Some(m), None) => Some(m.clone()),
            (None, Some(b)) => Some(b.to_string()),
            (None, None) => None,
        };
        let resp = match &tip {
            Some(tip) => resp.on_hover_text(tip),
            None => resp,
        };
        let hovered = resp.hovered();
        let being_dragged = self
            .layer_drag
            .as_ref()
            .is_some_and(|d| d.ids.contains(&id));

        // The drop target is read in [`OndinApp::drop_hint`] from the rects this
        // walk collects, against `pointer_interact_pos`, never `resp.hovered()`:
        // during a drag egui reports only the dragged widget as hovered (§15 D19).

        // Asked before the painter is taken, for the borrows. `None` for no
        // picture, one the store cannot draw, or one the pass's budget deferred
        // (all fall back to the glyph), and never for a main or an instance,
        // whose hexagon is the row's component mark (§15 D981).
        let thumb = picture
            .as_ref()
            .filter(|_| wears_thumbnail(hexagon))
            .and_then(|b| {
                self.thumbs
                    .texture(ui.ctx(), self.canvas.images(), &b.image, THUMB)
            });
        // The part of the row in view, inset by the tree's padding: the row is as
        // wide as the widest name in view and scrolls sideways under it, while
        // the tint, the controls, the badge and the mark stay pinned to it.
        // The view itself, not its overlap with the row: scrolled to the end, the
        // row can stop a few points short of the view, and the controls with it.
        let band = egui::Rect::from_x_y_ranges(self.tree_band_x(rect), rect.y_range());
        // Scrolled sideways, nothing of the row draws in the gutter beside its tint.
        let clip = ui.painter().clip_rect();
        let clipped = ui.painter().with_clip_rect(
            egui::Rect::from_x_y_ranges(band.x_range(), clip.y_range()).intersect(clip),
        );
        let p = &clipped;
        let r5 = egui::CornerRadius::same(5);
        if let Some(tint) = row_tint(being_dragged, selected, hovered) {
            p.rect_filled(band, r5, tint);
        }
        let cy = rect.center().y;
        let left = rect.left();
        let caret_x = caret_column(left, depth);
        // Right-side controls: eye / lock (on hover, or when toggled off),
        // [`EYE_PITCH`] apart. Their x's are worked out before the caret is
        // painted because all three go into one [`RowTargets`].
        let right = band.right();
        let lock_x = right - LOCK_INSET;
        let eye_x = lock_x - EYE_PITCH;
        // Also while bound (§15 D1060), as a hidden layer's eye is.
        // The keyboard on the row shows them as the pointer does: they are its keys.
        let near = hovered || resp.has_focus();
        let show_eye = near || !visible || bound_eye;
        let show_lock = near || locked;
        let targets = RowTargets {
            caret: has_children.then_some(caret_x),
            eye: show_eye.then_some(eye_x),
            // Not gated on `show_lock`: a press only arrives with the pointer on
            // the row, which is what `show_lock` already means.
            lock: Some(lock_x),
        };
        // The control the pointer is on, as opposed to the row (§15 D246).
        let under = resp.hover_pos().and_then(|q| targets.at(q.x));
        // A hovered control lights to `strong()` from every resting colour, with
        // no ground behind it (the row has its own hover ground).
        let lit = |on: bool, resting: egui::Color32| {
            if on { theme::text::strong() } else { resting }
        };
        if has_children {
            p.text(
                egui::pos2(caret_x, cy),
                egui::Align2::CENTER_CENTER,
                if expanded {
                    icon::CARET_DOWN
                } else {
                    icon::CARET_RIGHT
                },
                theme::icon_font(11.0),
                lit(under == Some(RowHit::Caret), theme::text::dim()),
            );
        }
        // A childless row reclaims the caret's 16px, except at the top level,
        // where a frame with a caret would sit right of the loose shape above
        // it and look like its child: depth 1 always leaves the room.
        let caret_room = has_children || depth == 1;
        let icon_x = left + icon_offset(depth, caret_room);
        row.selected = selected;
        row.hovered = hovered;
        // Not during a row drag: the drop line is the answer then.
        if hovered && !selected && self.layer_drag.is_none() {
            self.layers_hover = Some(id);
        }
        // A locked or hidden layer is drawn at 40% throughout, glyph and name,
        // and so is a slice, which never shows.
        let dimmed = locked || !visible || is_slice;
        // The container an in-flight drop would land in: the line says where
        // but not which group, so its text and icon go blue. A frame behind the
        // line ([`LayerDrag::lit`]).
        let drop_host = self.layer_drag.as_ref().is_some_and(|d| d.lit == Some(id));
        // A selected row wears the selection hue, not the chrome accent. The
        // drop host outranks selection and dimming (a drag is a question asked
        // now, §15 D331; a hidden group still takes children). The name takes
        // its glyph's ink: blue, or white beside a component's hexagon.
        let name_col = match (drop_host, selected, dimmed) {
            (true, ..) => color::select_ink(),
            (_, true, _) if hexagon.is_some() => color::text(),
            (_, true, _) => color::select_ink(),
            (_, false, true) => theme::text::off(),
            (_, false, false) => theme::text::muted(),
        };
        let glyph_col = match (drop_host, selected, dimmed) {
            (true, ..) => color::select_ink(),
            (_, true, _) if hexagon.is_some() => theme::text::strong(),
            (_, true, _) => color::select_ink(),
            (_, false, true) => dim_to(glyph_col, 102),
            (_, false, false) => glyph_col,
        };
        match thumb {
            // Centred where the glyph would be, as it replaces it. Rounded by
            // the tessellator, not `ui::mask_corners`, which paints corners in
            // the ground colour and a row has three grounds.
            Some(id) => {
                p.add(egui::Shape::Rect(
                    egui::epaint::RectShape::filled(
                        egui::Rect::from_center_size(
                            egui::pos2(icon_x, cy),
                            egui::vec2(THUMB, THUMB),
                        ),
                        egui::CornerRadius::same(THUMB_RADIUS),
                        thumb_tint(dimmed, picture.as_ref()),
                    )
                    .with_texture(id, crate::thumbs::FULL_UV),
                ));
            }
            None if hexagon == Some(true) => {
                crate::ui::paint_hexagon_filled(p, egui::pos2(icon_x, cy), ICON_PT, glyph_col);
            }
            None => {
                p.text(
                    egui::pos2(icon_x, cy),
                    egui::Align2::CENTER_CENTER,
                    glyph,
                    theme::icon_font(ICON_PT),
                    glyph_col,
                );
            }
        }
        // The badge is laid out before the name, which is covered short of it. Drawn
        // only while neither lock is: they share the row's end.
        let badge = badge.filter(|_| !locked && !show_lock).map(|b| {
            // One layout for both, differing only in the font.
            let (s, font, worded) = match b {
                RowBadge::Text(s) => (s, egui::FontId::proportional(crate::ui::CAPTION_PT), true),
                RowBadge::Icon(g, size) => (g.to_string(), theme::icon_font(size), false),
            };
            // Clear of a component mark, which holds the very edge.
            let edge = if mark.is_some() { MARK_SLOT } else { 4.0 };
            // A worded badge ends before a resting eye (§15 D1060; see
            // `bound_eye_tests`); a glyph badge clears it where it is.
            let right = match worded && show_eye {
                true => eye_x - BOUND_EYE.x / 2.0 - 4.0,
                false => right - edge,
            };
            (p.layout_no_wrap(s, font, theme::text::faint()), right)
        });
        let name_right = name_limit(NameLimit {
            row_right: right,
            mark: mark.is_some(),
            lock: (show_lock || locked).then_some(lock_x),
            eye: show_eye.then_some((eye_x, bound_eye)),
            badge_left: badge.as_ref().map(|(g, right)| right - g.size().x),
        });
        let renaming = matches!(&self.renaming_layer, Some((n, _)) if *n == id);
        if !renaming {
            // Whole, never cut: the tree scrolls sideways to the rest.
            let x = icon_x + NAME_GAP;
            let galley = p.layout_no_wrap(
                name.to_string(),
                egui::FontId::proportional(crate::ui::CONTROL_PT),
                name_col,
            );
            let name_end = x + galley.size().x;
            // Clipped where the cover turns solid: the cover's right corners are
            // rounded to the row's, and a name running under them would show
            // through as a sliver at the panel's edge.
            let mut name_clip = p.clip_rect();
            name_clip.max.x = name_clip.max.x.min(name_right - COVER_PAD + 1.0);
            p.with_clip_rect(name_clip).galley(
                egui::pos2(x, cy - galley.size().y / 2.0),
                galley,
                name_col,
            );
            // Room for the whole name and, scrolled to its end, a hovered row's
            // eye and lock after it. Hovered whatever the row is now, so a
            // hover does not change the width.
            let controls = right
                - name_limit(NameLimit {
                    row_right: right,
                    mark: mark.is_some(),
                    lock: Some(lock_x),
                    eye: Some((eye_x, bound_eye)),
                    badge_left: None,
                });
            self.layers_tree_w = self.layers_tree_w.max(name_end - rect.left() + controls);
            // From where the clip cuts, so no name ends in a hard cut.
            if name_end > name_right - COVER_PAD {
                let ground = row_ground(being_dragged, selected, hovered);
                cover_row_end(p, band, name_right, ground);
            }
        }

        if show_eye {
            // Bound, in the capsule (§15 D1060): `text_a(28)`, 44 under the
            // pointer. The click still writes the other value, which detaches.
            if bound_eye {
                let fill = if under == Some(RowHit::Eye) { 44 } else { 28 };
                p.rect_filled(
                    egui::Rect::from_center_size(egui::pos2(eye_x, cy), BOUND_EYE),
                    BOUND_EYE.y / 2.0,
                    theme::color::text_a(fill),
                );
            }
            p.text(
                egui::pos2(eye_x, cy),
                egui::Align2::CENTER_CENTER,
                if visible { icon::EYE } else { icon::EYE_SLASH },
                theme::icon_font(ICON_PT),
                lit(
                    under == Some(RowHit::Eye),
                    if visible {
                        theme::text::dim()
                    } else {
                        theme::text::off()
                    },
                ),
            );
        }
        // An optical lift: a point up sits the lock level with the eye beside it.
        let lock_at = egui::pos2(lock_x, cy - 1.0);
        if locked {
            // A locked layer wears a filled lock permanently, legible while
            // scanning rather than only on hover.
            crate::ui::paint_lock_filled(
                p,
                lock_at,
                ICON_PT,
                lit(under == Some(RowHit::Lock), theme::text::muted()),
                0.0,
            );
        } else if show_lock {
            p.text(
                lock_at,
                egui::Align2::CENTER_CENTER,
                icon::LOCK_SIMPLE_OPEN,
                theme::icon_font(ICON_PT),
                lit(under == Some(RowHit::Lock), theme::text::dim()),
            );
        } else if let Some((galley, right)) = badge {
            let at = egui::Align2::RIGHT_CENTER
                .anchor_size(egui::pos2(right, cy), galley.size())
                .min;
            p.galley(at, galley, theme::text::faint());
        }
        // The mark sits after the lock and eye at the row's very edge, and steps
        // aside while the row is hovered so the hover's controls stand alone; its
        // slot stays reserved, so the eye and lock do not move. The row's tooltip
        // says what the mark stood for (`mark_tip`).
        if let Some(m) = mark.filter(|_| !hovered) {
            let at = egui::pos2(right - MARK_SLOT / 2.0 + 1.0, cy);
            match m {
                RowMark::Override => {
                    p.circle_filled(at, 2.0, theme::text::muted());
                }
                RowMark::Local => {
                    let s = egui::Stroke::new(1.2, theme::text::muted());
                    p.line_segment([at - egui::vec2(2.5, 0.0), at + egui::vec2(2.5, 0.0)], s);
                    p.line_segment([at - egui::vec2(0.0, 2.5), at + egui::vec2(0.0, 2.5)], s);
                }
            }
        }

        // Not while renaming: the text field owns the pointer then.
        if resp.drag_started() && !renaming {
            // Dragging something selected carries the set; anything else is
            // selected first, as the canvas does (`begin_select_drag`).
            let ids = if self.session.selection.contains(id) {
                build::outermost(&self.session.doc, self.session.selection.ids())
            } else {
                self.session.selection.set_one(id);
                // Tell the panel (§15 D581), or `sync_tree_to_selection` reads
                // this as news and recentres a partly visible row, scrolling the
                // tree out from under a held pointer.
                self.layers_synced_selection = self.session.selection.ids().to_vec();
                vec![id]
            };
            // The press origin, not the pointer now: egui reports a drag only
            // past its threshold, which would bake that slop into the indent.
            let grab_x = ui
                .input(|i| i.pointer.press_origin())
                .map_or(rect.left(), |p| p.x);
            self.layer_drag = Some(LayerDrag {
                ids,
                target: None,
                grab: (grab_x, depth),
                level: depth,
                lit: None,
                spring: None,
                refused: None,
            });
        }

        // Double-click renames in place. Counted by `ui::double_clicked`, so a
        // click on one row and a fast double-click on the next still renames
        // it (§15 D986).
        // So do `F2` and `Ctrl+R` on the focused row, the keys rename has elsewhere.
        if crate::ui::double_clicked(ui, &resp) || (!renaming && crate::ui::rename_key(ui, &resp)) {
            self.renaming_layer = Some((id, name.clone()));
            // Focus is the field's business: see `layer_rename_field`.
        }
        if renaming {
            self.layer_rename_field(ui, id, rect, icon_x + NAME_GAP);
        }

        // The click that dismisses an open menu is spent dismissing it (R4), the
        // tab menu's included.
        let clicked =
            resp.clicked() && !renaming && self.context_menu.is_none() && !self.tabs.menu_was_up;
        // The focused row's keys for its own controls: `→`/`←` open and fold it,
        // and the hide and lock chords act on this row, as its eye and lock do.
        let (keyed, step) = if renaming {
            (None, 0)
        } else {
            row_keys(ui, &resp, has_children, expanded)
        };
        if step != 0 {
            self.layers_focus_step = Some((id, step));
        }
        // `Delete` on the focused row deletes as the keymap's does: the selection when
        // the row is in it, else the row alone. The focus goes to the row that takes
        // its place, or the one above when it was the last.
        let delete = !renaming
            && resp.has_focus()
            && ui.input(|i| {
                i.modifiers.is_none()
                    && (i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
            });
        if delete {
            if !self.session.selection.contains(id) {
                self.session.selection.set_one(id);
                self.layers_synced_selection = self.session.selection.ids().to_vec();
            }
            self.delete_selection();
            if self.session.doc.get(id).is_none() {
                self.layers_focus_step = Some((id, 1));
            }
            return;
        }
        if clicked || keyed.is_some() {
            // `interact_pointer_pos`, not the hover-based `under`: they can
            // differ on a touch or click-through frame. A keyboard click has none
            // and selects.
            let hit = keyed.or_else(|| resp.interact_pointer_pos().and_then(|q| targets.at(q.x)));
            match hit {
                Some(RowHit::Caret) => {
                    if expanded {
                        self.collapsed.insert(id);
                    } else {
                        self.collapsed.remove(&id);
                    }
                }
                Some(RowHit::Eye) => {
                    match visible {
                        true => self.session.name_step("Hide", icon::EYE_SLASH),
                        false => self.session.name_step("Show", icon::EYE),
                    }
                    self.session.commit(Transaction(vec![Operation::SetVisible {
                        id,
                        visible: !visible,
                    }]));
                }
                Some(RowHit::Lock) => {
                    match locked {
                        true => self.session.name_step("Unlock", icon::LOCK_SIMPLE_OPEN),
                        false => self.session.name_step("Lock", icon::LOCK_SIMPLE),
                    }
                    self.session.commit(Transaction(vec![Operation::SetLocked {
                        id,
                        locked: !locked,
                    }]));
                }
                None => self.select_from_row(id, ui),
            }
            // The panel made this selection itself, so it must not read as news
            // and scroll the row away.
            self.layers_synced_selection = self.session.selection.ids().to_vec();
        }

        // The panel's door (`docs/context-menus.md` §1): the only one that
        // reaches a locked or hidden layer, so its menu carries the full set,
        // including *Unlock* (§5.9, C4). C6 needs no code: the press already
        // committed any live rename (`layer_rename_field`).
        // Not after a cancelling right-click, which would reselect with no menu.
        if resp.secondary_clicked() && !self.gesture_cancelled {
            // C2, in the panel: a right-click on a row that is already part of
            // the selection acts on all of it.
            if !self.session.selection.contains(id) {
                self.session.selection.set_one(id);
                self.layers_synced_selection = self.session.selection.ids().to_vec();
            }
            self.open_context_menu(
                ui.ctx(),
                crate::menu::Target::Layer {
                    id,
                    door: crate::menu::Door::Panel,
                },
                None,
            );
        }
    }

    /// Scroll to the selection once, on the frame it changed; the topmost selected
    /// row wins. Only when the row is not wholly in view, and a jump, not a glide
    /// (§15 D1026). Up and down only: the row is as wide as the widest name, so
    /// asked of its whole box the scroll would also jump sideways every time.
    fn scroll_to_selected_row(&mut self, ui: &egui::Ui, rect: egui::Rect) {
        if !self.scroll_layers_to_selection {
            return;
        }
        self.scroll_layers_to_selection = false;
        let clip = ui.clip_rect();
        if rect.top() < clip.top() || rect.bottom() > clip.bottom() {
            ui.scroll_to_rect_animation(
                egui::Rect::from_x_y_ranges(clip.x_range(), rect.y_range()),
                Some(egui::Align::Center),
                egui::style::ScrollAnimation::none(),
            );
        }
    }

    /// Hold a drag on a collapsed group for [`SPRING_SECS`] and the branch opens.
    ///
    /// `on` is the group the pointer is resting on; `None` restarts the wait. The
    /// repaint request is the timing: a still hand wakes egui for nothing, and
    /// `layers_tree`'s per-frame repaint during a drag would otherwise be the only
    /// thing reading the clock.
    fn spring_load(&mut self, ui: &egui::Ui, on: Option<NodeId>) {
        let now = ui.input(|i| i.time);
        let Some(drag) = self.layer_drag.as_mut() else {
            return;
        };
        let Some(id) = on else {
            drag.spring = None;
            return;
        };
        match drag.spring {
            // Still on the same group: open it once the wait is up.
            Some((waiting, since)) if waiting == id => {
                if now - since >= SPRING_SECS {
                    drag.spring = None;
                    self.collapsed.remove(&id);
                } else {
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_secs_f64(
                            SPRING_SECS - (now - since),
                        ));
                }
            }
            // A different group, or none until now: start the wait here.
            _ => {
                drag.spring = Some((id, now));
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs_f64(SPRING_SECS));
            }
        }
    }

    /// Turn a [`DropSpot`] into the parent and index the document will be asked
    /// for, or `None` where that drop is not allowed.
    ///
    /// The reversed walk is the whole of the arithmetic: a slot below a row on
    /// screen is before it in the model, and the top of a container's list is
    /// `child_count`. A drop indented out of a container lands beside an ancestor,
    /// so the row is walked up to the depth the pointer asked for and that node's
    /// slot is the answer (the row itself at its own level).
    fn resolve_drop(
        &self,
        rows: &[TreeRow],
        spot: DropSpot,
        dragged: &[NodeId],
    ) -> Option<DropTarget> {
        let target = self.drop_slot(rows, spot)?;
        // The destination must not be inside what is carried. Asked of the parent,
        // not the row under the pointer, which sits on a dragged row's own child
        // throughout an outdent.
        if dragged
            .iter()
            .any(|d| *d == target.parent || self.is_ancestor_of(*d, target.parent))
        {
            return None;
        }
        // A frame never sits under a boolean or a mask, nor in a shape (§5.3, §15
        // D876); every carried row must be welcome, and a set takes them together
        // (`variant::takes_together`, §15 D1003 (5)).
        (dragged
            .iter()
            .all(|d| self.drop_is_legal(*d, target.parent))
            && ondin_core::variant::takes_together(&self.session.doc, target.parent, dragged))
        .then_some(target)
    }

    /// [`Self::resolve_drop`]'s geometry half: the parent and index `spot` names,
    /// split out so a refused drop can still say which parent refused it
    /// ([`Self::set_refusal`]).
    fn drop_slot(&self, rows: &[TreeRow], spot: DropSpot) -> Option<DropTarget> {
        let target = match spot {
            // Into the container, at the end of its list: the top of it on screen.
            DropSpot::Into(i) => {
                let parent = rows.get(i)?.id;
                DropTarget {
                    parent,
                    index: self.child_count(parent),
                }
            }
            DropSpot::Between { boundary, depth } => {
                match boundary.checked_sub(1).and_then(|i| rows.get(i)) {
                    // One level in from the row above: inside it, at the top of its
                    // list. [`depth_range`] only offers this for an open container.
                    Some(above) if depth > above.depth => DropTarget {
                        parent: above.id,
                        index: self.child_count(above.id),
                    },
                    Some(above) => {
                        let anchor = self.ancestor_at_depth(above.id, above.depth, depth)?;
                        let parent = self.session.doc.get(anchor)?.parent()?;
                        let index = self
                            .session
                            .doc
                            .get(parent)?
                            .children()
                            .iter()
                            .position(|c| *c == anchor)?;
                        DropTarget { parent, index }
                    }
                    // The top of the whole list, the end of the root's; the depth test
                    // is belt and braces, as [`depth_range`] offers only level 1 here.
                    None if depth == 1 => {
                        let root = self.session.page;
                        DropTarget {
                            parent: root,
                            index: self.child_count(root),
                        }
                    }
                    None => return None,
                }
            }
        };
        Some(target)
    }

    /// Why the layers in `dragged` may not go into `parent`, when the reason is one
    /// kind per set (§15 D1003 (5)) — the message the release says, as the canvas's
    /// move does (`canvas::SET_OF_ANOTHER_KIND`, `canvas::SET_OF_MIXED_KINDS`).
    /// `None` for any other refusal, which the missing hint already says.
    fn set_refusal(&self, parent: NodeId, dragged: &[NodeId]) -> Option<&'static str> {
        let doc = &self.session.doc;
        if !ondin_core::variant::is_set(doc, parent) {
            return None;
        }
        let another = dragged.iter().filter_map(|d| doc.get(*d)).any(|n| {
            n.component()
                && n.parent() != Some(parent)
                && !ondin_core::variant::fits_set(doc, parent, n.kind(), Some(n.id()))
        });
        if another {
            Some(crate::canvas::SET_OF_ANOTHER_KIND)
        } else if !ondin_core::variant::takes_together(doc, parent, dragged) {
            Some(crate::canvas::SET_OF_MIXED_KINDS)
        } else {
            None
        }
    }

    /// What differs among the row `id`'s children, from `child_drift_cache` while
    /// the document is unchanged — the parent row's dot and tooltip (§15 D1003 (9))
    /// and what its *Restore children* is offered for. Its scope is the row's fold
    /// (§15 D1004 (2)): a [collapsed](Self::row_folded) row's whole subtree, an
    /// expanded row's own children.
    pub(crate) fn child_drift_of(&mut self, id: NodeId) -> ondin_core::reset::ChildDrift {
        let folded = self.row_folded(id);
        if let Some(d) = self.child_drift_cached(id, folded) {
            return d;
        }
        let d = child_drift_scoped(&self.session.doc, id, folded);
        self.child_drift_cache.1.insert((id, folded), d);
        d
    }

    /// [`Self::child_drift_of`] for a `&self` caller (the context menu), from the
    /// cache when it is current, computed on a miss.
    pub(crate) fn child_drift_read(&self, id: NodeId) -> ondin_core::reset::ChildDrift {
        let folded = self.row_folded(id);
        (self.child_drift_cache.0 == self.session.revision())
            .then(|| self.child_drift_cache.1.get(&(id, folded)).copied())
            .flatten()
            .unwrap_or_else(|| child_drift_scoped(&self.session.doc, id, folded))
    }

    /// The cache's answer for `(id, folded)`, emptying it first if the document
    /// has changed since it was filled.
    fn child_drift_cached(
        &mut self,
        id: NodeId,
        folded: bool,
    ) -> Option<ondin_core::reset::ChildDrift> {
        let rev = self.session.revision();
        if self.child_drift_cache.0 != rev {
            self.child_drift_cache = (rev, Default::default());
        }
        self.child_drift_cache.1.get(&(id, folded)).copied()
    }

    /// Whether the row `id` is drawn collapsed — it has children, the user folded
    /// it, and no search is open. Scopes the row's tooltip and *Restore children*
    /// (§15 D1004 (2)) from either door, canvas menu included.
    pub(crate) fn row_folded(&self, id: NodeId) -> bool {
        let searching = self
            .layer_filter
            .as_ref()
            .is_some_and(|f| !f.trim().is_empty());
        !searching
            && self.collapsed.contains(&id)
            && self
                .session
                .doc
                .get(id)
                .is_some_and(|n| !n.children().is_empty())
    }

    /// Walk `id`, which is at `depth`, up to the ancestor sitting at `want`. `id` itself
    /// when the two are equal; `None` if `want` is deeper, which [`depth_range`] never
    /// asks for.
    fn ancestor_at_depth(&self, id: NodeId, depth: usize, want: usize) -> Option<NodeId> {
        let mut cursor = id;
        for _ in 0..depth.checked_sub(want)? {
            cursor = self.session.doc.get(cursor)?.parent()?;
        }
        Some(cursor)
    }

    fn drop_is_legal(&self, dragged: NodeId, parent: NodeId) -> bool {
        let Some(node) = self.session.doc.get(dragged) else {
            return false;
        };
        let Some(parent_node) = self.session.doc.get(parent) else {
            return false;
        };
        let doc = &self.session.doc;
        // The ancestor half (§15 D876) is what a kind pair cannot see, and the
        // component half (`component::can_hold`) what neither sees (§15 D1003 (5)).
        // The canvas asks the same predicate.
        build::can_parent(parent_node.kind(), node.kind())
            // A slice has no outline to be an operand with.
            && !(matches!(parent_node.kind(), NodeKind::Boolean { .. })
                && matches!(node.kind(), NodeKind::Slice { .. }))
            && (doc.frame_may_sit_under(parent) || !doc.holds_a_frame(dragged))
            && ondin_core::component::can_hold(doc, parent, dragged)
    }

    /// The x-range of the tree in view, inset by its padding: where a row's tint,
    /// controls and drop hints sit, whatever the sideways scroll. `row` stands in
    /// before the view is known.
    fn tree_band_x(&self, row: egui::Rect) -> egui::Rangef {
        let pad = f32::from(TREE_PAD_X);
        self.layers_view_x.map_or(row.x_range(), |v| {
            egui::Rangef::new(v.min + pad, v.max - pad)
        })
    }

    /// Read the pointer against the rows the walk just drew: work out where the drag
    /// would land, record it for the release, and draw the affordance that says so.
    ///
    /// One pass after the walk, because an indent-aware drop is a claim about the row
    /// above a boundary and the row below it at once. The hint is drawn only where
    /// the drop is legal.
    fn drop_hint(&mut self, ui: &egui::Ui, rows: &[TreeRow]) {
        let Some(drag) = self.layer_drag.as_ref() else {
            return;
        };
        // A drag with no button behind it paints nothing and lands nothing (§15
        // D471, D582): a drag stranded by the view-switch chord would otherwise turn
        // a resting pointer into a live drop that the next click's release commits.
        // The release frame is admitted, since egui clears the button state on the
        // frame that processes it; any other frame with no button drops the drag.
        let (down, released) =
            ui.input(|i| (i.pointer.primary_down(), i.pointer.primary_released()));
        if !down && !released {
            self.layer_drag = None;
            return;
        }
        let (grab, held) = (drag.grab, drag.level);
        let dragged = drag.ids.clone();
        // `pointer_interact_pos` against the row rects, never `resp.hovered()`
        // (§15 D19; see `layers_row`).
        let Some(pointer) = ui.ctx().pointer_interact_pos() else {
            return;
        };
        let Some(read) = drop_spot(rows, pointer, grab, held) else {
            // Off the tree the level is still carried, so coming back does not
            // restart the gesture.
            self.spring_load(ui, None);
            return;
        };
        let spot = read.spot;
        // Only the "inside this one" zone springs: opening a branch from a row's
        // edge would shift the rows below out from under an aimed drop.
        self.spring_load(
            ui,
            match spot {
                DropSpot::Into(i) if rows[i].closed => Some(rows[i].id),
                _ => None,
            },
        );
        let Some(target) = self.resolve_drop(rows, spot, &dragged) else {
            let why = self
                .drop_slot(rows, spot)
                .and_then(|t| self.set_refusal(t.parent, &dragged));
            if let Some(d) = self.layer_drag.as_mut() {
                d.refused = why;
            }
            return;
        };
        if let Some(d) = self.layer_drag.as_mut() {
            d.target = Some(target);
            d.level = read.level;
        }

        let painter = ui.painter();
        // Framing the row's tint, not its content, which runs as wide as the
        // widest name.
        let band =
            |row: egui::Rect| egui::Rect::from_x_y_ranges(self.tree_band_x(row), row.y_range());
        match spot {
            DropSpot::Into(i) => {
                painter.rect_stroke(
                    band(rows[i].rect),
                    egui::CornerRadius::same(5),
                    egui::Stroke::new(1.0, color::SELECT),
                    egui::StrokeKind::Inside,
                );
            }
            DropSpot::Between { boundary, depth } => {
                // Which container the line lands in is said by the destination
                // row's blue ink, not here (`drop_host` in [`OndinApp::layers_row`],
                // [`LayerDrag::lit`]).
                let Some(y) = boundary_y(rows, boundary, ui.spacing().item_spacing.y) else {
                    return;
                };
                let (y, width) = snap_across(y, DROP_LINE_PT, ui.ctx().pixels_per_point());
                // Every row spans the panel, so any gives the two ends.
                let rect = rows[boundary.min(rows.len() - 1)].rect;
                let ends = band(rect);
                // The line starts at the level's own indent: that is what says
                // which depth an ambiguous drop takes.
                painter.line_segment(
                    [
                        egui::pos2((rect.left() + indent_of(depth)).max(ends.left()), y),
                        egui::pos2(ends.right(), y),
                    ],
                    egui::Stroke::new(width, color::SELECT),
                );
            }
        }
    }

    /// Land the dragged rows, as one transaction and so one undo step. The index
    /// arithmetic is [`plan_layer_move`]'s.
    ///
    /// A dragged id gone from the document is dropped, not fatal. The carried set
    /// is re-sorted into document order. An empty plan commits nothing. The status
    /// line names the verb (*Moved* when `plan.reparented`, else *Reordered*) and
    /// the count.
    fn finish_layer_drag(&mut self) {
        let Some(drag) = self.layer_drag.take() else {
            return;
        };
        let Some(target) = drag.target else {
            // A drop refused for one kind per set says why, as the canvas's does
            // (§15 D1003 (5)); any other refusal is the hint that never lit.
            if let Some(why) = drag.refused {
                self.session.info(why);
            }
            return;
        };

        // Document order, not click order.
        let order = ondin_core::subtree_nodes(&self.session.doc, &[self.session.page]);
        let mut moving: Vec<NodeId> = drag
            .ids
            .into_iter()
            .filter(|id| self.session.doc.contains(*id))
            .collect();
        let rank: std::collections::HashMap<NodeId, usize> =
            order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        moving.sort_by_key(|id| rank.get(id).copied().unwrap_or(usize::MAX));
        if moving.is_empty() {
            return;
        }

        let n = moving.len();
        match plan_layer_move(&self.session.doc, &self.session.resolved, &moving, target) {
            Ok(plan) if plan.ops.is_empty() => {}
            Ok(plan) => {
                self.session.name_step("Move", icon::ARROWS_OUT_CARDINAL);
                if self.session.commit(Transaction(plan.ops)) {
                    self.session.info(match (plan.reparented, n) {
                        (true, 1) => "Moved layer".to_string(),
                        (true, n) => format!("Moved {n} layers"),
                        (false, 1) => "Reordered".to_string(),
                        (false, n) => format!("Reordered {n} layers"),
                    });
                }
            }
            Err(e) => self.session.fail(format!("Cannot move layer: {e}")),
        }
    }

    /// Whether `ancestor` is on `id`'s parent chain.
    fn is_ancestor_of(&self, ancestor: NodeId, id: NodeId) -> bool {
        let mut cursor = self.session.doc.get(id).and_then(|n| n.parent());
        while let Some(p) = cursor {
            if p == ancestor {
                return true;
            }
            cursor = self.session.doc.get(p).and_then(|n| n.parent());
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
    use ondin_core::{Document, IdSource, Resolved};

    /// Root → one frame holding `n` rects, in document order.
    fn tree(n: usize) -> (Document, Resolved, NodeId, Vec<NodeId>) {
        let mut ids = IdSource::new(0x1AEA);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let frame = ids.mint();
        let mut ops = vec![Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 300.0),
                corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((10.0, 10.0))),
            name: None,
        }];
        let kids: Vec<NodeId> = (0..n).map(|_| ids.mint()).collect();
        for (i, id) in kids.iter().enumerate() {
            ops.push(Operation::CreateNode {
                id: *id,
                parent: frame,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            });
        }
        doc.apply(&Transaction(ops)).unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, frame, kids)
    }

    fn children(doc: &Document, parent: NodeId) -> Vec<NodeId> {
        doc.get(parent).unwrap().children().to_vec()
    }

    /// Three rows dropped at one index arrive in order and stay together.
    #[test]
    fn several_rows_dropped_together_keep_their_order() {
        let (mut doc, res, frame, k) = tree(5);
        // Move c, d, e to the very front, in that order.
        let plan = plan_layer_move(
            &doc,
            &res,
            &[k[2], k[3], k[4]],
            DropTarget {
                parent: frame,
                index: 0,
            },
        )
        .unwrap();
        doc.apply(&Transaction(plan.ops)).unwrap();
        assert_eq!(
            children(&doc, frame),
            vec![k[2], k[3], k[4], k[0], k[1]],
            "the three moved rows should lead, in their own order"
        );
    }

    /// A drop at an index after the lifted rows accounts for their removal.
    #[test]
    fn a_drop_after_the_moved_rows_accounts_for_their_removal() {
        let (mut doc, res, frame, k) = tree(5);
        // Move a and b to sit between d and e.
        let plan = plan_layer_move(
            &doc,
            &res,
            &[k[0], k[1]],
            DropTarget {
                parent: frame,
                index: 4,
            },
        )
        .unwrap();
        doc.apply(&Transaction(plan.ops)).unwrap();
        assert_eq!(children(&doc, frame), vec![k[2], k[3], k[0], k[1], k[4]]);
    }

    /// Rows from two different parents land in one, in order.
    #[test]
    fn rows_gathered_from_two_parents_land_in_order() {
        let (mut doc, _res, frame, k) = tree(2);
        // A second frame with two rects of its own.
        let mut ids = IdSource::new(0x2BEB);
        let other = ids.mint();
        let x = ids.mint();
        let y = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: other,
                parent: doc.first_page(),
                index: 1,
                kind: NodeKind::Artboard {
                    size: Size::new(100.0, 100.0),
                    corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((500.0, 0.0))),
                name: None,
            },
            Operation::CreateNode {
                id: x,
                parent: other,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
            Operation::CreateNode {
                id: y,
                parent: other,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);

        // One row from each frame, into the first frame at the front.
        let plan = plan_layer_move(
            &doc,
            &res,
            &[k[1], y],
            DropTarget {
                parent: frame,
                index: 0,
            },
        )
        .unwrap();
        assert!(plan.reparented, "one of them crossed a parent");
        doc.apply(&Transaction(plan.ops)).unwrap();
        assert_eq!(children(&doc, frame), vec![k[1], y, k[0]]);
        assert_eq!(children(&doc, other), vec![x]);
    }

    /// A reparented row does not move on screen (a raw `Reparent` would teleport it).
    #[test]
    fn a_reparented_row_does_not_move_on_screen() {
        let (mut doc, res, frame, k) = tree(1);
        let before = res.world_bounds(k[0]).unwrap();
        let root = doc.first_page();
        let plan = plan_layer_move(
            &doc,
            &res,
            &[k[0]],
            DropTarget {
                parent: root,
                index: 0,
            },
        )
        .unwrap();
        doc.apply(&Transaction(plan.ops)).unwrap();
        let after = Resolved::rebuild(&doc).world_bounds(k[0]).unwrap();
        assert!(
            (after.min_x() - before.min_x()).abs() < 1e-9
                && (after.min_y() - before.min_y()).abs() < 1e-9,
            "the layer teleported: {before:?} -> {after:?}"
        );
        let _ = frame;
    }

    /// A drop that lands the rows where they already are plans nothing (§15 D532).
    /// flip: remove the composition test in `plan_layer_move` → fails at index 2
    /// with two rows (indices 0 and 1 stay green either way).
    #[test]
    fn a_drop_that_moves_nothing_plans_nothing() {
        let (doc, res, frame, k) = tree(3);
        let plan = |moving: &[NodeId], index: usize| {
            plan_layer_move(
                &doc,
                &res,
                moving,
                DropTarget {
                    parent: frame,
                    index,
                },
            )
            .expect("a plan")
        };

        // The whole run, dropped at each boundary that leaves it where it is.
        for index in [0, 1, 2] {
            assert!(
                plan(&[k[0], k[1]], index).ops.is_empty(),
                "two rows dropped at {index} land where they already are"
            );
        }
        // Controls: a real move still plans, and the single-row case stays correct.
        assert_eq!(
            plan(&[k[0], k[1]], 3).ops.len(),
            2,
            "and a drop that does move still plans both rows"
        );
        assert!(plan(&[k[0]], 0).ops.is_empty());
        assert_eq!(plan(&[k[0]], 3).ops.len(), 1);
    }

    /// One boundary, one y, whichever row's edge is asked.
    #[test]
    fn both_sides_of_a_boundary_put_the_line_in_the_same_place() {
        for gap in [0.0_f32, 1.0, 2.0, 6.0] {
            let upper = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(240.0, ROW_H));
            let lower = egui::Rect::from_min_size(
                egui::pos2(0.0, upper.bottom() + gap),
                egui::vec2(240.0, ROW_H),
            );
            let from_above = drop_line_y(upper, RowEdge::Bottom, gap);
            let from_below = drop_line_y(lower, RowEdge::Top, gap);
            assert!(
                (from_above - from_below).abs() < 1e-4,
                "gap {gap}: {from_above} from above, {from_below} from below"
            );
            assert!(
                from_above >= upper.bottom() - 1e-4 && from_above <= lower.top() + 1e-4,
                "gap {gap}: {from_above} is outside [{}, {}]",
                upper.bottom(),
                lower.top()
            );
        }
    }

    /// The drop line is the same weight wherever it lands: both edges sit on device
    /// pixel boundaries.
    #[test]
    fn the_drop_line_lands_on_whole_device_pixels() {
        for ppp in [1.0_f32, 1.25, 1.5, 1.75, 2.0] {
            for y in [40.0_f32, 66.5, 92.25, 118.7, 145.05] {
                let (snapped, width) = snap_across(y, DROP_LINE_PT, ppp);
                let px = width * ppp;
                assert!(
                    (px - px.round()).abs() < 1e-4 && px >= 1.0,
                    "width {width} is {px} device px at {ppp}"
                );
                for edge in [snapped - width / 2.0, snapped + width / 2.0] {
                    let e = edge * ppp;
                    assert!(
                        (e - e.round()).abs() < 1e-4,
                        "edge {edge} is {e} device px at {ppp} (y={y})"
                    );
                }
                assert!(
                    (snapped - y).abs() <= 1.0 / ppp,
                    "snapping moved the line from {y} to {snapped} at {ppp}"
                );
            }
        }
    }
}

#[cfg(test)]
mod rename_layout_tests {
    use super::*;

    use crate::app::OndinApp;
    use crate::input::Action;
    use ondin_core::kurbo::Size;

    /// A headless app holding one rect at the root, and that rect's id.
    fn app_with_a_layer(ctx: &egui::Context) -> (OndinApp, NodeId) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(9);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let rect = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: rect,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("a rect");
        app.session.adopt_document(doc, None);
        (app, rect)
    }

    /// One frame of the layers tree, wide enough for a row.
    fn frame(app: &mut OndinApp, ctx: &egui::Context) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(300.0, 600.0),
            )),
            ..Default::default()
        };
        ctx.run_ui(input, |ui| app.layers_tree(ui))
    }

    /// The field asks for the caret on the frame it first draws, and never again.
    /// flip: re-request whenever the field lacks focus → the rename can never be
    /// committed or left.
    #[test]
    fn the_rename_field_asks_for_the_caret_on_the_frame_it_first_draws() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_layer(&ctx);
        app.renaming_layer = Some((id, "Ellipse".to_owned()));

        let _ = frame(&mut app, &ctx);
        assert_eq!(
            ctx.memory(|m| m.focused()),
            Some(rename_field_id(id)),
            "the field has to arrive focused, or the rename opens a box nobody is \
             typing into"
        );

        // What Enter and Escape both do first.
        ctx.memory_mut(|m| m.surrender_focus(rename_field_id(id)));
        let _ = frame(&mut app, &ctx);
        assert_ne!(
            ctx.memory(|m| m.focused()),
            Some(rename_field_id(id)),
            "a field that re-takes focus can never be committed or cancelled"
        );
    }

    /// No frame ends with focus on a widget that frame did not draw (AccessKit
    /// panics on it); the order asserted is the context menu's, drawn after the panel.
    /// flip: request focus in `OndinApp::rename_selection` → fails at the first
    /// `in_the_tree`.
    #[test]
    fn a_rename_opened_after_the_panel_has_drawn_leaves_no_dangling_focus() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let (mut app, id) = app_with_a_layer(&ctx);
        app.session.selection.set_one(id);

        let in_the_tree = |full: &egui::FullOutput, when: &str| {
            let update = full
                .platform_output
                .accesskit_update
                .as_ref()
                .expect("the fixture: AccessKit is on, or this test asserts nothing");
            assert!(
                update.nodes.iter().any(|(nid, _)| *nid == update.focus),
                "{when}: the focus is not among the {} nodes the frame registered, \
                 which is the panic",
                update.nodes.len()
            );
        };

        let opened = {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(300.0, 600.0),
                )),
                ..Default::default()
            };
            ctx.run_ui(input, |ui| {
                app.layers_tree(ui);
                app.dispatch(ui.ctx(), Action::Rename);
            })
        };
        assert!(
            app.renaming_layer.is_some(),
            "the fixture: the rename did open"
        );
        in_the_tree(&opened, "the frame the rename opened in");

        let arrived = frame(&mut app, &ctx);
        in_the_tree(&arrived, "the frame the field arrived in");
        assert_eq!(
            ctx.memory(|m| m.focused()),
            Some(rename_field_id(id)),
            "and the caret still lands in it, one frame later"
        );
    }

    /// The rename field's text lands exactly on the name it replaces, the rect
    /// starting `INLINE_EDIT_PAD_X` left of it (measured against a real
    /// `Align2::LEFT_CENTER` galley).
    #[test]
    fn the_rename_field_sits_where_the_painted_name_sits() {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(Default::default(), |ui| {
            let row = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(240.0, ROW_H));
            let left = 60.0;
            let painted = ui.painter().text(
                egui::pos2(left, row.center().y),
                egui::Align2::LEFT_CENTER,
                "Ellipse",
                egui::FontId::proportional(crate::ui::CONTROL_PT),
                egui::Color32::WHITE,
            );
            let field = rename_field_rect(row, left);

            let text_left = field.left() + crate::ui::INLINE_EDIT_PAD_X;
            assert!(
                (text_left - painted.left()).abs() < 0.01,
                "the field's text starts at {text_left} but the name starts at {}",
                painted.left()
            );
            assert!(
                (field.center().y - painted.center().y).abs() < 0.01,
                "the field centres on {} but the name centres on {}",
                field.center().y,
                painted.center().y
            );
            assert!(
                (field.height() - ROW_H).abs() < 0.01,
                "the field is {} tall, not the row's {ROW_H}",
                field.height()
            );
        });
    }
}

#[cfg(test)]
mod missing_picture_tests {
    use super::picture_missing;
    use ondin_core::kurbo::{RoundedRectRadii, Size};
    use ondin_core::peniko::Color;
    use ondin_core::{
        Brush, Document, Fill, IdSource, ImageId, NodeId, NodeKind, Operation, Stroke, StrokeAlign,
        Transaction, image_brush,
    };

    /// One rect carrying `fills` and `strokes`.
    fn node_with(fills: Vec<Fill>, strokes: Vec<Stroke>) -> (Document, NodeId) {
        let mut ids = IdSource::new(3);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let rect = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: rect,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("a rect");
        doc.apply(&Transaction(vec![
            Operation::SetFills {
                id: rect,
                fills: ondin_core::keyed_by_position(fills),
            },
            Operation::SetStrokes {
                id: rect,
                strokes: ondin_core::keyed_by_position(strokes),
            },
        ]))
        .expect("paint it");
        (doc, rect)
    }

    fn fill(brush: Brush, visible: bool) -> Fill {
        Fill {
            brush: brush.into(),
            visible,
        }
    }

    fn stroke(brush: Brush, visible: bool) -> Stroke {
        Stroke {
            brush: brush.into(),
            width: 1.0,
            width_var: None,
            join: ondin_core::kurbo::Join::Miter,
            cap: ondin_core::kurbo::Cap::Butt,
            dashes: Vec::new(),
            dash_offset: 0.0,
            sides: Default::default(),
            miter_limit: 4.0,
            dash_fit: false,
            align: StrokeAlign::Center,
            visible,
        }
    }

    /// A visible fill or stroke whose picture is gone marks the row; a hidden paint
    /// or a layer with no picture never does.
    #[test]
    fn a_layer_is_marked_for_any_visible_paint_whose_picture_is_gone() {
        // An empty store: every reference is unresolvable (§15 D179).
        let store = ondin_render::ImageStore::new();
        let missing = || image_brush(ImageId("gone".into()));
        let solid = || Brush::Solid(Color::BLACK);

        let (doc, id) = node_with(vec![fill(missing(), true)], Vec::new());
        assert!(
            picture_missing(&store, doc.get(id).unwrap()),
            "a visible image fill with no pixels has to mark the row"
        );

        let (doc, id) = node_with(vec![fill(solid(), true)], vec![stroke(missing(), true)]);
        assert!(
            picture_missing(&store, doc.get(id).unwrap()),
            "a border painted with a missing picture is as broken as a middle"
        );

        let (doc, id) = node_with(vec![fill(missing(), false)], Vec::new());
        assert!(
            !picture_missing(&store, doc.get(id).unwrap()),
            "a hidden fill draws nothing, so there is nothing missing to say"
        );

        let (doc, id) = node_with(vec![fill(solid(), true)], vec![stroke(solid(), true)]);
        assert!(
            !picture_missing(&store, doc.get(id).unwrap()),
            "a layer with no picture on it must never wear the warning"
        );
    }
}

#[cfg(test)]
mod subtract_base_tests {
    use super::subtract_base;
    use ondin_core::kurbo::{RoundedRectRadii, Size};
    use ondin_core::{BoolOp, Document, IdSource, NodeId, NodeKind, build};

    /// Two rects under the root, then `build::boolean` over them with `op` and
    /// `key`; returns the operands in child order, bottom first.
    fn boolean_of_two(op: BoolOp, key: Option<usize>) -> (Document, Vec<NodeId>, Vec<NodeId>) {
        let mut ids = IdSource::new(23);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let mut made = Vec::new();
        for i in 0..2 {
            let id = ids.mint();
            doc.apply(&ondin_core::Transaction(vec![
                ondin_core::Operation::CreateNode {
                    id,
                    parent: root,
                    index: i,
                    kind: NodeKind::Rect {
                        size: Size::new(10.0, 10.0),
                        corner_radii: RoundedRectRadii::default(),
                        corner_smoothing: 0.0,
                    },
                    transform: None,
                    name: None,
                },
            ]))
            .expect("a rect");
            made.push(id);
        }
        let (tx, container) = build::boolean(
            &doc,
            &ondin_core::Resolved::rebuild(&doc),
            &mut ids,
            &made,
            op,
            key.map(|k| made[k]),
        )
        .expect("a boolean");
        doc.apply(&tx).expect("apply the boolean");
        let operands = doc
            .get(container)
            .expect("the container")
            .children()
            .to_vec();
        (doc, operands, made)
    }

    /// The bottom operand of a `Subtract` is the base, and only that one (§15 D113);
    /// the operand above it is the discriminating assertion.
    #[test]
    fn only_the_bottom_operand_of_a_subtract_is_the_base() {
        let (doc, operands, _) = boolean_of_two(BoolOp::Subtract, None);
        assert_eq!(operands.len(), 2, "the fixture built {operands:?}");
        assert!(
            subtract_base(&doc, doc.get(operands[0]).unwrap()),
            "the bottom operand is the shape being cut into"
        );
        assert!(
            !subtract_base(&doc, doc.get(operands[1]).unwrap()),
            "the operand above it is what cuts, not what is cut"
        );
    }

    /// The other three operations fold commutatively, so there is no base to name;
    /// Union stands in for all three.
    #[test]
    fn a_union_has_no_base_to_mark() {
        let (doc, operands, _) = boolean_of_two(BoolOp::Union, None);
        assert!(
            operands
                .iter()
                .all(|id| !subtract_base(&doc, doc.get(*id).unwrap())),
            "a union marked one of its operands"
        );
    }

    /// The key layer is the base, having been moved to the bottom, so the mark
    /// travels with the designation; asserted by identity, not position.
    #[test]
    fn designating_the_key_moves_the_mark_with_it() {
        let (_, plain_ops, plain_made) = boolean_of_two(BoolOp::Subtract, None);
        assert_eq!(
            plain_ops[0], plain_made[0],
            "with no key the bottom of the z-order is already the base"
        );
        let (doc, operands, made) = boolean_of_two(BoolOp::Subtract, Some(1));
        assert_eq!(
            operands[0], made[1],
            "the designated layer should have gone to the bottom"
        );
        assert!(subtract_base(&doc, doc.get(made[1]).unwrap()));
        assert!(
            !subtract_base(&doc, doc.get(made[0]).unwrap()),
            "the shape that was the base without a key must not still be one"
        );
    }

    /// A shape with no boolean over it is nobody's operand, and the root has no
    /// parent to ask at all.
    #[test]
    fn a_loose_shape_and_the_root_are_never_a_base() {
        let (mut doc, _, _) = boolean_of_two(BoolOp::Subtract, None);
        let root_id = doc.first_page();
        assert!(
            !subtract_base(&doc, doc.get(root_id).unwrap()),
            "the root has no parent"
        );
        // First child of a container, as a base is, and a base of nothing.
        let mut ids = IdSource::new(99);
        let loose = ids.mint();
        doc.apply(&ondin_core::Transaction(vec![
            ondin_core::Operation::CreateNode {
                id: loose,
                parent: root_id,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
        ]))
        .expect("a loose rect");
        assert!(
            !subtract_base(&doc, doc.get(loose).unwrap()),
            "a first child of the root is not the base of anything"
        );
    }
}

#[cfg(test)]
mod badge_ink_tests {
    use super::*;

    /// The ink height of a laid-out string, in points, from the tight glyph bitmap
    /// (`uv_rect`) rather than the galley's line box.
    fn ink_height(ctx: &egui::Context, s: &str, font: egui::FontId) -> f32 {
        let galley = ctx.fonts_mut(|f| f.layout_no_wrap(s.to_string(), font, egui::Color32::WHITE));
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for row in &galley.rows {
            for g in &row.row.glyphs {
                if g.uv_rect.size.y <= 0.0 {
                    continue;
                }
                lo = lo.min(g.uv_rect.offset.y);
                hi = hi.max(g.uv_rect.offset.y + g.uv_rect.size.y);
            }
        }
        hi - lo
    }

    /// Every glyph badge, at its own size, inks as tall as the worded badge across
    /// four display scalings. A new badge belongs in the loop with a measured size.
    /// flip: `BADGE_BASE` at 10 → fails at 1.5×.
    #[test]
    fn the_base_badge_inks_as_tall_as_the_worded_one() {
        for ppp in [1.0_f32, 1.25, 1.5, 2.0] {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let mut input = egui::RawInput::default();
            let vp = input.viewport_id;
            input
                .viewports
                .get_mut(&vp)
                .unwrap()
                .native_pixels_per_point = Some(ppp);
            let _ = ctx.run_ui(input, |_| {});

            let word = ink_height(
                &ctx,
                "800 × 600",
                egui::FontId::proportional(crate::ui::CAPTION_PT),
            );
            for (name, badge, size) in [
                ("diamond", icon::DIAMOND, BADGE_BASE),
                ("property-bound", icon::PROPERTY_BOUND, BADGE_BOUND),
                ("mask", icon::MASK, BADGE_MASK),
            ] {
                let glyph = ink_height(&ctx, badge, theme::icon_font(size));
                assert!(
                    (glyph - word).abs() <= 0.51,
                    "at {ppp}× the {name} glyph inks {glyph:.2} at {size} against \
                     the word's {word:.2}"
                );
            }
        }
    }
}

#[cfg(test)]
mod name_limit_tests {
    use super::*;

    fn limit(lock: Option<f32>, eye: Option<(f32, bool)>, badge: Option<f32>, mark: bool) -> f32 {
        name_limit(NameLimit {
            row_right: 290.0,
            mark,
            lock,
            eye,
            badge_left: badge,
        })
    }

    /// The name's room ends short of the leftmost thing on the row's right, and a
    /// point inside the row when nothing is there.
    #[test]
    fn the_name_stops_short_of_the_rows_right_hand_end() {
        assert_eq!(
            limit(None, None, None, false),
            289.0,
            "a point inside the row"
        );
        assert_eq!(limit(None, None, None, true), 290.0 - MARK_SLOT);
        assert_eq!(limit(None, None, Some(240.0), false), 240.0 - NAME_CLEAR);
        let eye = 253.0;
        assert_eq!(
            limit(Some(276.0), Some((eye, false)), None, false),
            eye - ICON_PT / 2.0 - NAME_CLEAR,
            "the eye, left of the lock"
        );
        assert_eq!(
            limit(None, Some((eye, true)), None, false),
            eye - BOUND_EYE.x / 2.0 - NAME_CLEAR,
            "the bound eye's capsule"
        );
    }

    /// A long name is clipped where its cover turns solid, so none of its ink
    /// reaches under the cover's rounded right corners at the panel's edge.
    ///
    /// flip: the name painted on the row's painter rather than the clipped one →
    /// fails at "clipped short of the size".
    #[test]
    fn a_long_name_is_clipped_where_its_cover_turns_solid() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        app.view = crate::app::View::Editor;
        let frame = app.session.doc.get(app.session.page).unwrap().children()[0];
        let long = "Ko-padel — Home with a name far too long for the pane";
        assert!(app.session.commit(ondin_core::Transaction(vec![
            ondin_core::Operation::SetName {
                id: frame,
                name: long.into(),
            }
        ])));
        let mut kitten = eframe::Frame::_new_kittest();
        let mut out = None;
        for _ in 0..4 {
            out = Some(ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1320.0, 820.0),
                    )),
                    ..Default::default()
                },
                |ui| eframe::App::ui(&mut app, ui, &mut kitten),
            ));
        }
        let out = out.expect("drawn");
        let text = |f: &dyn Fn(&str) -> bool| {
            out.shapes.iter().find_map(|c| match &c.shape {
                egui::Shape::Text(t) if f(t.galley.text()) => Some((c.clip_rect, t)),
                _ => None,
            })
        };
        let (clip, name) = text(&|t| t == long).expect("the name is drawn");
        let (_, size) = text(&|t| t.contains(" × ")).expect("the frame's size is drawn");
        let size_left = size.visual_bounding_rect().left();
        assert!(
            name.visual_bounding_rect().right() > size_left,
            "the fixture: the name runs on under the size"
        );
        assert!(
            clip.right() < size_left,
            "clipped short of the size: the name's clip ends at {}, the size starts at \
             {size_left}",
            clip.right()
        );
    }

    /// A long frame name is drawn whole, running on under the frame's size, which
    /// sits on the row's own ground painted over the name; Shift+wheel scrolls the
    /// name sideways while the size stays at the panel's edge.
    /// flip: `layers_row` without its `cover_row_end` → fails at "covered";
    /// `ScrollArea::vertical` for the tree → fails at "scrolled sideways"; a floor of
    /// offset + view on the tree's width → fails at "no further than the longest name";
    /// the tint painted on the whole row → fails at "the tint stays". The band read
    /// off the content's clip rect does not bite: headless, the panel's clip caps
    /// egui's widening of it, so this frame shows no gap between the two.
    #[test]
    fn a_long_frame_name_is_whole_and_scrolls_under_its_size() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        app.view = crate::app::View::Editor;
        let frame = app.session.doc.get(app.session.page).unwrap().children()[0];
        let long = "Ko-padel — Home with a name far too long for the pane";
        assert!(app.session.commit(ondin_core::Transaction(vec![
            ondin_core::Operation::SetName {
                id: frame,
                name: long.into(),
            }
        ])));
        let mut kitten = eframe::Frame::_new_kittest();
        let mut pass = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let time = ctx.input(|i| i.time) + 0.1;
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1320.0, 820.0),
                    )),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| eframe::App::ui(app, ui, &mut kitten),
            );
            out.shapes.into_iter().map(|c| c.shape).collect::<Vec<_>>()
        };
        let text_at = |shapes: &[egui::Shape], f: &dyn Fn(&str) -> bool| {
            shapes
                .iter()
                .position(|s| matches!(s, egui::Shape::Text(t) if f(t.galley.text())))
        };
        let rect_of = |s: &egui::Shape| match s {
            egui::Shape::Text(t) => t.visual_bounding_rect(),
            _ => unreachable!(),
        };
        let is_size =
            |t: &str| t.contains(" × ") && t.chars().next().is_some_and(|c| c.is_ascii_digit());
        let mut shapes = Vec::new();
        for _ in 0..4 {
            shapes = pass(&mut app, vec![]);
        }
        let n = text_at(&shapes, &|t| t == long).expect("the whole name is drawn");
        let s = text_at(&shapes, &is_size).expect("the frame's size is drawn");
        let (name, size) = (rect_of(&shapes[n]), rect_of(&shapes[s]));
        assert!(
            name.right() > size.left(),
            "the fixture: the name runs on under the size ({} past {})",
            name.right(),
            size.left()
        );
        assert!(
            shapes[n..s]
                .iter()
                .any(|sh| matches!(sh, egui::Shape::Rect(r)
                if r.fill == color::panel()
                    && r.rect.left() <= size.left() - NAME_CLEAR
                    && r.rect.right() >= size.right())),
            "covered: the row's ground lies between the name and its size"
        );

        // Hovered before any scroll: the tint and the eye to hold still against.
        let hover_tint = |shapes: &[egui::Shape]| {
            shapes.iter().find_map(|s| match s {
                egui::Shape::Rect(r)
                    if r.fill == color::hover() && r.rect.y_range().contains(name.center().y) =>
                {
                    Some(r.rect)
                }
                _ => None,
            })
        };
        pass(
            &mut app,
            vec![egui::Event::PointerMoved(name.left_center())],
        );
        for _ in 0..3 {
            shapes = pass(&mut app, vec![]);
        }
        let tint = hover_tint(&shapes).expect("hovered, the row is tinted");
        let eye = rect_of(&shapes[text_at(&shapes, &|t| t == icon::EYE).expect("the eye")]);

        let wheel = |dx: f32| egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, dx),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::SHIFT,
        };
        pass(&mut app, vec![wheel(-60.0)]);
        // Off the row, or it shows its eye and lock in place of the size.
        pass(&mut app, vec![egui::Event::PointerGone]);
        for _ in 0..3 {
            shapes = pass(&mut app, vec![]);
        }
        let moved = rect_of(&shapes[text_at(&shapes, &|t| t == long).expect("still drawn")]);
        let stayed = rect_of(&shapes[text_at(&shapes, &is_size).expect("the size")]);
        assert!(
            moved.left() < name.left() - 10.0,
            "scrolled sideways: the name starts at {} after {}",
            moved.left(),
            name.left()
        );
        assert_eq!(
            stayed.left(),
            size.left(),
            "the size stays at the panel's edge"
        );

        // Hovered at the far end of the scroll, the tint and the eye are where they
        // were before it: pinned to the view, not to the row scrolling under them.
        pass(
            &mut app,
            vec![egui::Event::PointerMoved(name.left_center())],
        );
        pass(&mut app, vec![wheel(-2000.0)]);
        for _ in 0..3 {
            shapes = pass(&mut app, vec![]);
        }
        let far = rect_of(&shapes[text_at(&shapes, &|t| t == long).expect("still drawn")]);
        assert!(
            far.left() < moved.left(),
            "the fixture: scrolled on past {} to {}",
            moved.left(),
            far.left()
        );
        assert_eq!(hover_tint(&shapes), Some(tint), "the tint stays in view");
        let far_eye = rect_of(&shapes[text_at(&shapes, &|t| t == icon::EYE).expect("the eye")]);
        assert_eq!(far_eye, eye, "the eye stays at the panel's edge");
        pass(&mut app, vec![egui::Event::PointerGone]);

        // The scroll reaches no further than the longest name in view: shortened,
        // the name fits and the view comes back, rather than holding empty room.
        let short = "Ko-padel";
        assert!(app.session.commit(ondin_core::Transaction(vec![
            ondin_core::Operation::SetName {
                id: frame,
                name: short.into(),
            }
        ])));
        for _ in 0..3 {
            shapes = pass(&mut app, vec![]);
        }
        let back = rect_of(&shapes[text_at(&shapes, &|t| t == short).expect("the short name")]);
        assert_eq!(
            back.left(),
            name.left(),
            "no further than the longest name: the short one starts where the long one did"
        );
    }
}

#[cfg(test)]
mod row_target_tests {
    use super::{CONTROL_HIT, RowHit, RowTargets};

    /// A row with all three controls out, at plausible x's.
    fn full() -> RowTargets {
        RowTargets {
            caret: Some(11.0),
            eye: Some(163.0),
            lock: Some(186.0),
        }
    }

    /// A control the row is not drawing does not answer, for press or hover (§15 D246).
    #[test]
    fn a_control_the_row_is_not_drawing_is_not_a_target() {
        let bare = RowTargets {
            caret: None,
            eye: None,
            lock: Some(186.0),
        };
        assert_eq!(bare.at(11.0), None, "a childless row has no caret to press");
        assert_eq!(
            bare.at(163.0),
            None,
            "a row showing no eye has no eye to press"
        );
        assert_eq!(bare.at(186.0), Some(RowHit::Lock));
    }

    /// The band is `CONTROL_HIT` either side and nothing beyond it, asserted at the
    /// edge and a hair past it.
    #[test]
    fn the_band_reaches_control_hit_either_side_and_stops() {
        let t = full();
        for d in [-CONTROL_HIT, -CONTROL_HIT + 0.5, 0.0, CONTROL_HIT] {
            assert_eq!(t.at(163.0 + d), Some(RowHit::Eye), "at offset {d}");
        }
        for d in [-CONTROL_HIT - 0.1, CONTROL_HIT + 0.1] {
            assert_ne!(t.at(163.0 + d), Some(RowHit::Eye), "at offset {d}");
        }
    }

    /// Where two bands overlap, the caret wins, the same for hover and press.
    #[test]
    fn overlapping_bands_resolve_the_press_order() {
        let together = RowTargets {
            caret: Some(160.0),
            eye: Some(163.0),
            lock: Some(186.0),
        };
        assert_eq!(together.at(161.0), Some(RowHit::Caret));
    }
}

#[cfg(test)]
mod tree_guide_tests {
    //! The dashed lines that say which container a row is inside, and the path
    //! that lights on hover. Mostly asserted on `tree_guide_segments` over a
    //! synthetic list of depths; the app test at the end proves the walk feeds it.
    use super::*;
    use crate::app::OndinApp;
    use ondin_core::kurbo::Size;
    use ondin_core::{Document, IdSource};

    /// The tree's own row pitch: rows do not touch, `item_spacing.y` is 1.
    const GAP: f32 = 1.0;

    /// `marked` with nothing selected.
    fn rows(depths: &[usize], hovered: Option<usize>) -> Vec<TreeRow> {
        marked(depths, hovered, &[])
    }

    /// Rows at the given depths down a panel whose left edge is 0, `ROW_H` tall on a
    /// `ROW_H + GAP` pitch; `hovered` and `selected` index `depths`. `lead` is the
    /// caret row's, and the elbow goes through `elbow_y`. `container` is read off the
    /// depths (a row the next is deeper than). `pub(super)` so `drop_indent_tests`
    /// shares the same rows.
    pub(super) fn marked(
        depths: &[usize],
        hovered: Option<usize>,
        selected: &[usize],
    ) -> Vec<TreeRow> {
        let mut ids = IdSource::new(0x9A17);
        depths
            .iter()
            .enumerate()
            .map(|(i, &depth)| {
                let rect = egui::Rect::from_min_size(
                    egui::pos2(0.0, i as f32 * (ROW_H + GAP)),
                    egui::vec2(288.0, ROW_H),
                );
                TreeRow {
                    id: ids.mint(),
                    rect,
                    depth,
                    lead: caret_column(0.0, depth) - 7.5,
                    elbow: elbow_y(rect, 1.0),
                    selected: selected.contains(&i),
                    hovered: hovered == Some(i),
                    container: depths.get(i + 1).is_some_and(|&next| next > depth),
                    // A list of depths cannot say this; spring-loading is driven
                    // through the panel over a real document.
                    closed: false,
                }
            })
            .collect()
    }

    /// The vertical runs in one row, top to bottom.
    fn verticals(segs: &[GuideSeg], row: &TreeRow) -> Vec<GuideSeg> {
        let mut v: Vec<GuideSeg> = segs
            .iter()
            .copied()
            .filter(|s| s.from.x == s.to.x && s.from.y >= row.rect.top() - 0.01)
            .filter(|s| s.from.y < row.rect.bottom() + 0.01)
            .collect();
        v.sort_by(|a, b| {
            a.from
                .x
                .total_cmp(&b.from.x)
                .then(a.from.y.total_cmp(&b.from.y))
        });
        v
    }

    /// The horizontal run in one row — its elbow.
    fn elbow(segs: &[GuideSeg], row: &TreeRow) -> GuideSeg {
        *segs
            .iter()
            .find(|s| s.from.y == s.to.y && (s.from.y - row.elbow).abs() < 0.01)
            .expect("an elbow")
    }

    /// A row hangs off the column its parent's caret is on, its elbow reaches its own
    /// glyph, and a top-level row has no guides.
    /// flip: hang the connector off `caret_column(left, depth)` → fails at 42 where 11
    /// belongs.
    #[test]
    fn a_row_hangs_off_the_column_its_parents_caret_is_on() {
        let r = rows(&[1, 2], None);
        let segs = tree_guide_segments(&r, GAP, 0..r.len());
        assert!(
            verticals(&segs, &r[0]).is_empty()
                && !segs.iter().any(|s| (s.from.y - r[0].elbow).abs() < 0.01),
            "a top-level row has nothing to hang off: {segs:#?}"
        );

        let column = caret_column(0.0, 1);
        assert_eq!(
            column, 11.0,
            "the frame's caret column, from the panel edge"
        );
        let v = verticals(&segs, &r[1]);
        assert_eq!(v.len(), 1, "one connector and no trunk past it: {v:#?}");
        assert_eq!(v[0].from, egui::pos2(column, r[1].rect.top()));
        assert_eq!(v[0].to, egui::pos2(column, r[1].elbow));

        let e = elbow(&segs, &r[1]);
        assert_eq!(
            e.from.x,
            column + ELBOW_CLEAR,
            "clear of the parent's column by one gap of the dash pattern, so the \
             corner is not a blot"
        );
        assert_eq!(
            e.to.x, r[1].lead,
            "and up to the row's own leading glyph, not past it"
        );
    }

    /// A trunk runs past every child but the last and stops at that one's elbow.
    /// flip: draw the below-elbow run unconditionally → fails at the last child's
    /// vertical count.
    #[test]
    fn a_trunk_runs_to_the_last_child_and_stops_at_its_elbow() {
        let r = rows(&[1, 2, 2, 2], None);
        let segs = tree_guide_segments(&r, GAP, 0..r.len());
        for (i, carries_on) in [(1, true), (2, true), (3, false)] {
            let v = verticals(&segs, &r[i]);
            assert_eq!(
                v.len(),
                if carries_on { 2 } else { 1 },
                "row {i} of three children: {v:#?}"
            );
            assert_eq!(v[0].to.y, r[i].elbow, "row {i}'s connector");
            if carries_on {
                assert_eq!(
                    v[1].to.y,
                    r[i].rect.bottom() + GAP,
                    "row {i}'s trunk crosses the gap to the row below, or the line \
                     breaks once per row"
                );
            }
        }
    }

    /// A trunk whose branch has already closed is not drawn: in `A → B → C`, C draws
    /// nothing in A's column.
    /// flip: end a closed level's run at the elbow instead of omitting it → fails at
    /// the single-run count (the only test here that bites).
    #[test]
    fn a_trunk_that_already_ended_leaves_no_fragment_in_a_deeper_row() {
        let r = rows(&[1, 2, 3], None);
        let segs = tree_guide_segments(&r, GAP, 0..r.len());
        let v = verticals(&segs, &r[2]);
        assert_eq!(v.len(), 1, "C hangs off B and off nothing else: {v:#?}");
        assert_eq!(
            v[0].from.x,
            caret_column(0.0, 2),
            "and the one run it has is B's column, not A's"
        );
    }

    /// Hovering a row lights the path from its parent down to its own elbow and
    /// nothing else: the parent's trunk between the two, the connector but not the
    /// trunk below the elbow, no intervening elbow, no other level.
    /// flip: light the trunk over `start..=h` rather than `start..h` → fails the
    /// below-elbow claim; drop the `*l == level` test → fails the level-1 claim.
    #[test]
    fn hovering_a_row_lights_the_trunk_it_hangs_off() {
        // frame ▸ group ▸ three deep rows ▸ a second child of the frame. The
        // hovered row needs a sibling after it (a trunk below its elbow), and the
        // frame's second child puts a level-1 trunk through the deep rows.
        let r = rows(&[1, 2, 3, 3, 3, 2], Some(3));
        let segs = tree_guide_segments(&r, GAP, 0..r.len());
        let level2 = caret_column(0.0, 2);
        let level1 = caret_column(0.0, 1);

        let lit = |s: &GuideSeg| s.ink == GuideInk::Hovered;

        let between = verticals(&segs, &r[2]);
        for s in between.iter().filter(|s| s.from.x == level2) {
            assert!(
                lit(s),
                "the whole of the parent's trunk between the two rows lights: {s:?}"
            );
        }
        assert!(
            between.iter().any(|s| s.from.x == level2),
            "the fixture: row 2 does carry the parent's trunk"
        );
        let frames_trunk: Vec<GuideSeg> = between
            .iter()
            .copied()
            .filter(|s| s.from.x == level1)
            .collect();
        assert_eq!(
            frames_trunk.len(),
            1,
            "the fixture: row 2 also carries the frame's trunk: {between:#?}"
        );
        assert_eq!(
            frames_trunk[0].ink,
            GuideInk::Rest,
            "a level the hover is not about"
        );
        assert_eq!(
            elbow(&segs, &r[2]).ink,
            GuideInk::Rest,
            "only the hovered row's own elbow lights — an intervening row's is the \
             path passing through, not a row being pointed at"
        );

        let hovered = verticals(&segs, &r[3]);
        let connector = hovered
            .iter()
            .find(|s| s.from.x == level2 && s.from.y == r[3].rect.top())
            .expect("the hovered row's connector");
        assert!(lit(connector), "the connector into the hovered row lights");
        assert!(lit(&elbow(&segs, &r[3])), "and so does its elbow");
        let under: Vec<GuideSeg> = hovered
            .iter()
            .copied()
            .filter(|s| s.from.x == level2 && s.from.y > r[3].rect.top())
            .collect();
        assert_eq!(
            under.len(),
            1,
            "the fixture: the hovered row has a sibling after it, so there is a \
             trunk under its elbow to be about"
        );
        assert_eq!(
            under[0].ink,
            GuideInk::Rest,
            "below the hovered row's elbow the trunk goes back to rest"
        );

        for (i, row) in r.iter().enumerate() {
            for s in verticals(&segs, row).iter().filter(|s| s.from.x == level1) {
                assert!(!lit(s), "row {i}: the frame's trunk is not on the path");
            }
            if i > 3 {
                assert!(
                    verticals(&segs, row).iter().all(|s| !lit(s)) && !lit(&elbow(&segs, row)),
                    "row {i} is below the hovered one and lights nothing"
                );
            }
        }
    }

    /// A selected row wears its leg (parent trunk, connector, elbow) in
    /// `GuideInk::Selected`; hovering another row greys only its own path; hovering
    /// the selected row changes nothing.
    /// flip: test `selected` before `hovered` → fails the crossed-hover claim; drop
    /// `&& !r.selected` → fails the same-row claim; mark the whole trunk (no
    /// `connector` term) → fails the below-elbow claim.
    #[test]
    fn a_selected_row_wears_its_leg_in_the_selection_blue() {
        // frame ▸ group ▸ three deep rows, the last selected: its leg crosses two
        // rows and a stretch of trunk below its elbow must not light.
        let level2 = caret_column(0.0, 2);
        let depths = [1, 2, 3, 3, 3];
        let leg = |segs: &[GuideSeg], row: &TreeRow| -> Vec<GuideSeg> {
            verticals(segs, row)
                .into_iter()
                .filter(|s| s.from.x == level2)
                .collect()
        };

        let r = marked(&depths, None, &[3]);
        let segs = tree_guide_segments(&r, GAP, 0..r.len());
        for i in [2, 3] {
            for s in leg(&segs, &r[i]) {
                let below_the_elbow = s.from.y > r[i].rect.top();
                assert_eq!(
                    s.ink,
                    if i == 3 && below_the_elbow {
                        GuideInk::Rest
                    } else {
                        GuideInk::Selected
                    },
                    "row {i}, {}: {s:?}",
                    if below_the_elbow {
                        "under the elbow"
                    } else {
                        "the connector"
                    }
                );
            }
        }
        assert_eq!(
            elbow(&segs, &r[3]).ink,
            GuideInk::Selected,
            "the selected row's own elbow"
        );
        assert_eq!(
            elbow(&segs, &r[2]).ink,
            GuideInk::Rest,
            "and not an intervening row's"
        );

        // The row between the selection and its parent, hovered.
        let crossed = tree_guide_segments(&marked(&depths, Some(2), &[3]), GAP, 0..depths.len());
        assert_eq!(
            leg(&crossed, &r[2])
                .iter()
                .map(|s| s.ink)
                .collect::<Vec<_>>(),
            vec![GuideInk::Hovered, GuideInk::Selected],
            "hover takes the connector it points at and leaves the rest of the leg \
             blue"
        );
        assert_eq!(
            leg(&crossed, &r[3])[0].ink,
            GuideInk::Selected,
            "and the selected row's own connector, below all of that, is untouched"
        );

        // The selected row itself, hovered.
        let same = tree_guide_segments(&marked(&depths, Some(3), &[3]), GAP, 0..depths.len());
        assert_eq!(
            leg(&same, &r[2]).iter().map(|s| s.ink).collect::<Vec<_>>(),
            vec![GuideInk::Selected, GuideInk::Selected],
            "a hover on the selected row adds nothing — the panel is already \
             talking about it"
        );
        assert_eq!(elbow(&same, &r[3]).ink, GuideInk::Selected);
    }

    /// Root → one frame → one group → two rects: three deep, which is the shallowest
    /// tree that has an ancestor trunk *and* a branch that closes.
    fn app_with_a_nested_tree(ctx: &egui::Context) -> OndinApp {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x1AEA);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let frame = ids.mint();
        let group = ids.mint();
        let mut ops = vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 300.0),
                    corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: group,
                parent: frame,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
        ];
        for i in 0..2 {
            ops.push(Operation::CreateNode {
                id: ids.mint(),
                parent: group,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            });
        }
        doc.apply(&Transaction(ops)).expect("the tree");
        app.session.adopt_document(doc, None);
        app
    }

    /// Every line segment the panel paints, with the pointer wherever `at` says.
    /// Five passes, since a widget's interaction state is last frame's.
    fn dashes(
        app: &mut OndinApp,
        ctx: &egui::Context,
        at: Option<egui::Pos2>,
    ) -> Vec<(egui::Pos2, egui::Pos2, egui::Color32)> {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(300.0, 600.0),
            )),
            ..Default::default()
        };
        if let Some(p) = at {
            input.events.push(egui::Event::PointerMoved(p));
        }
        let mut full = ctx.run_ui(input.clone(), |ui| app.layers_tree(ui));
        for _ in 1..5 {
            full = ctx.run_ui(input.clone(), |ui| app.layers_tree(ui));
        }
        full.shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::LineSegment { points, stroke } => {
                    Some((points[0], points[1], stroke.color))
                }
                _ => None,
            })
            .collect()
    }

    /// The trunk is inked through the junction the elbow leaves it at: asserted on
    /// the painted dashes, the first dash under each elbow with a carried-on trunk
    /// starts at the elbow. The ink-from-above half is only a guard.
    /// flip: restore the dash offset `(-(elbow - top)).rem_euclid(4.0)` → fails with
    /// the trunk resuming 2.5pt below the elbow.
    #[test]
    fn the_trunk_is_inked_through_the_junction() {
        let ctx = egui::Context::default();
        let mut app = app_with_a_nested_tree(&ctx);
        let all = dashes(&mut app, &ctx, None);
        let vertical = |c: f32| -> Vec<(egui::Pos2, egui::Pos2)> {
            all.iter()
                .filter(|(a, b, _)| a.x == b.x && (a.x - c).abs() < 0.01)
                .map(|(a, b, _)| (*a, *b))
                .collect()
        };

        // Each elbow, as the y it sits on and the column it leaves. Every dash comes
        // back separately, so a run's start is the leftmost of them.
        let mut ys: Vec<f32> = all
            .iter()
            .filter(|(a, b, _)| a.y == b.y)
            .map(|(a, ..)| a.y)
            .collect();
        ys.sort_by(f32::total_cmp);
        ys.dedup();
        assert_eq!(ys.len(), 3, "the fixture: three rows have elbows");
        let elbows: Vec<(f32, f32)> = ys
            .iter()
            .map(|&y| {
                let start = all
                    .iter()
                    .filter(|(a, b, _)| a.y == b.y && a.y == y)
                    .map(|(a, ..)| a.x)
                    .fold(f32::INFINITY, f32::min);
                (y, snap_across(start - ELBOW_CLEAR, 1.0, 1.0).0)
            })
            .collect();

        let mut carried = 0;
        for (y, column) in elbows {
            let runs = vertical(column);
            assert!(
                runs.iter().any(|(a, b)| a.y < y && b.y >= y - 0.01),
                "the guard: the trunk at x {column} reaches the elbow at y {y} from \
                 above at all"
            );
            let Some(first) = runs
                .iter()
                .filter(|(_, b)| b.y > y + 0.01)
                .map(|(a, _)| a.y)
                .min_by(f32::total_cmp)
            else {
                continue; // a last child: nothing carries on below this elbow
            };
            carried += 1;
            assert!(
                first <= y + 0.01,
                "the trunk at x {column} resumes {:.1}pt below the elbow at y {y}",
                first - y
            );
        }
        assert!(
            carried > 0,
            "the fixture: at least one elbow has a trunk carrying on below it, or \
             the assertion above was never reached"
        );
    }

    /// The pointer on a row lights that row's guides through the panel: the wiring
    /// of `hovered` into `TreeRow`. The pointer goes on the last row, so the lit path
    /// crosses a row; below the tree nothing lights.
    /// flip: push `hovered: false` into every `TreeRow` → the lit count is 0, not 18.
    #[test]
    fn the_pointer_on_a_row_lights_its_guides() {
        let ctx = egui::Context::default();
        let mut app = app_with_a_nested_tree(&ctx);
        let lit = |v: &[(egui::Pos2, egui::Pos2, egui::Color32)]| {
            v.iter().filter(|(.., c)| *c == guide_hover()).count()
        };

        let away = dashes(&mut app, &ctx, Some(egui::pos2(250.0, 500.0)));
        assert!(!away.is_empty(), "the fixture: the panel is drawing guides");
        assert_eq!(
            lit(&away),
            0,
            "with the pointer below the tree nothing is on the path"
        );

        // Four rows on a 27pt pitch: y 100 is the last of them.
        let on_row = dashes(&mut app, &ctx, Some(egui::pos2(120.0, 100.0)));
        assert!(lit(&on_row) > 0, "the pointer on a row lights its path");
        for (a, b, _) in on_row.iter().filter(|(.., c)| *c == guide_hover()) {
            if a.x == b.x {
                assert_eq!(
                    a.x, 42.5,
                    "the lit trunk is the one the row hangs off, not another level"
                );
            }
        }
    }

    /// The walk feeds the guides and the preference reaches the painter, asserted on
    /// the dashes' x's (the snapped columns 11.5 and 42.5) as well as their count.
    /// flip: drop the `self.prefs.tree_guides` test → the second half fails with the
    /// same count as the first.
    #[test]
    fn the_panel_draws_the_guides_and_the_preference_switches_them_off() {
        let ctx = egui::Context::default();
        let mut app = app_with_a_nested_tree(&ctx);
        let on = dashes(&mut app, &ctx, None);
        assert!(
            !on.is_empty(),
            "a tree three deep draws guides, and this one drew none"
        );
        for (a, b, _) in &on {
            if a.x == b.x {
                assert!(
                    [11.5_f32, 42.5].contains(&a.x),
                    "a vertical guide off the caret columns, at x {}",
                    a.x
                );
            } else {
                assert_eq!(a.y, b.y, "a guide that is neither vertical nor horizontal");
            }
        }
        assert!(
            on.iter().any(|(a, b, _)| a.y == b.y && a.x != b.x),
            "and the elbows are drawn too, not only the trunks"
        );

        app.prefs.tree_guides = false;
        assert!(
            dashes(&mut app, &ctx, None).is_empty(),
            "switched off, the panel draws no line at all"
        );
    }
}

/// Dropping follows the indent: one gap between two rows is a *run* of insertion
/// points, one per level, and the pointer's x picks which.
#[cfg(test)]
mod drop_indent_tests {
    use super::*;
    use crate::app::OndinApp;
    use ondin_core::kurbo::Size;
    use ondin_core::{Document, IdSource};

    /// ```text
    /// root
    /// ├ 0 e            ← drawn last, at the bottom
    /// └ 1 frame
    ///     ├ 0 d
    ///     └ 1 group
    ///         ├ 0 c1
    ///         └ 1 c2   ← drawn first inside the group
    /// ```
    ///
    /// The walk reverses each child list (§5.3), so top to bottom the rows are
    /// `frame, group, c2, c1, d, e` at depths `1, 2, 3, 3, 2, 1`. The group holds two
    /// rects (else dragging one to the group's bottom is a drop beside itself) and
    /// the frame holds `d` (else the boundary under the group offers one level).
    struct Fixture {
        app: OndinApp,
        root: NodeId,
        frame: NodeId,
        group: NodeId,
        c1: NodeId,
        c2: NodeId,
        d: NodeId,
        e: NodeId,
    }

    fn fixture(ctx: &egui::Context) -> Fixture {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x0D40);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (frame, group) = (ids.mint(), ids.mint());
        let (c1, c2, d, e) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
        let rect = |id, parent, index| Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        };
        let ops = vec![
            rect(e, root, 0),
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 1,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 300.0),
                    corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            rect(d, frame, 0),
            Operation::CreateNode {
                id: group,
                parent: frame,
                index: 1,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            rect(c1, group, 0),
            rect(c2, group, 1),
        ];
        doc.apply(&Transaction(ops)).expect("the tree");
        app.session.adopt_document(doc, None);
        Fixture {
            app,
            root,
            frame,
            group,
            c1,
            c2,
            d,
            e,
        }
    }

    /// A drop is refused where `apply` would refuse the reparent, ancestors included
    /// (§15 D870, D876): a card's group goes into a plain group, not into a mask.
    /// flip: delete the `frame_may_sit_under || !holds_a_frame` clause → fails at
    /// "a card's group into a mask".
    #[test]
    fn a_frame_cannot_be_dropped_under_a_mask_at_any_depth() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0xD4A);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (page, masked, r, plain, holder, card) = (
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
        );
        let create = |id, parent, kind| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind,
            transform: None,
            name: None,
        };
        let frame = || NodeKind::Artboard {
            size: Size::new(40.0, 40.0),
            corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        };
        doc.apply(&Transaction(vec![
            create(page, root, frame()),
            create(masked, page, NodeKind::Group),
            create(
                r,
                masked,
                NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
            ),
            Operation::SetMask {
                id: masked,
                mask: true,
            },
            create(plain, page, NodeKind::Group),
            create(holder, page, NodeKind::Group),
            create(card, holder, frame()),
        ]))
        .expect("the tree");
        app.session.adopt_document(doc, None);

        assert!(app.drop_is_legal(card, plain), "a card into a group");
        assert!(
            app.drop_is_legal(holder, plain),
            "a card's group into a group"
        );
        assert!(
            !app.drop_is_legal(holder, masked),
            "a card's group into a mask"
        );
        assert!(!app.drop_is_legal(card, masked), "a card into a mask");
    }

    /// A drop is refused where the component rules would refuse it (§15 D1003 (5)):
    /// a main into a main, while a rect into it and a main in its own parent are fine.
    /// flip: delete the `component::can_hold` clause → fails at "a main into a main".
    #[test]
    fn a_main_cannot_be_dropped_into_a_main() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0xD4B);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (k, m, r) = (ids.mint(), ids.mint(), ids.mint());
        let create = |id, kind| Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind,
            transform: None,
            name: None,
        };
        let frame = || NodeKind::Artboard {
            size: Size::new(40.0, 40.0),
            corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        };
        doc.apply(&Transaction(vec![
            create(k, frame()),
            create(m, frame()),
            create(
                r,
                NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
            ),
            Operation::SetComponent {
                id: k,
                component: true,
            },
            Operation::SetComponent {
                id: m,
                component: true,
            },
        ]))
        .expect("the tree");
        app.session.adopt_document(doc, None);

        assert!(!app.drop_is_legal(m, k), "a main into a main");
        assert!(app.drop_is_legal(r, k), "a rect into a main");
        assert!(app.drop_is_legal(m, root), "a main in its own parent");
    }

    /// A set keeps to one kind through the panel, and a refused drop says why (§15
    /// D1003 (5)): mixed kinds onto an empty set, and a group main onto a set of
    /// frames, are refused with the canvas's message (`LayerDrag::refused`).
    /// flip: delete the `takes_together` clause from `resolve_drop` → fails at "a
    /// frame main and a group main together into an empty set"; delete the
    /// `drag.refused` read from `finish_layer_drag` → fails at "the release says why".
    #[test]
    fn a_set_keeps_to_one_kind_and_a_refused_drop_says_why() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0xD4C);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [e, s, v, f, g, r] = [(); 6].map(|_| ids.mint());
        let create = |id, parent, kind| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind,
            transform: None,
            name: None,
        };
        let frame = || NodeKind::Artboard {
            size: Size::new(40.0, 40.0),
            corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        };
        let set = |id| Operation::SetVariantSet {
            id,
            set: Some(Default::default()),
        };
        let main = |id| Operation::SetComponent {
            id,
            component: true,
        };
        doc.apply(&Transaction(vec![
            create(e, root, frame()),
            create(s, root, frame()),
            create(v, s, frame()),
            create(f, root, frame()),
            create(g, root, NodeKind::Group),
            create(
                r,
                g,
                NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
            ),
            set(e),
            set(s),
            main(v),
            main(f),
            main(g),
        ]))
        .expect("the tree");
        app.session.adopt_document(doc, None);

        let row = |id| TreeRow {
            id,
            rect: egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(288.0, 24.0)),
            depth: 1,
            lead: 0.0,
            elbow: 12.0,
            selected: false,
            hovered: false,
            container: true,
            closed: true,
        };
        let into = DropSpot::Into(0);
        assert_eq!(
            app.resolve_drop(&[row(e)], into, &[f, g]),
            None,
            "a frame main and a group main together into an empty set"
        );
        assert_eq!(
            app.set_refusal(e, &[f, g]),
            Some(crate::canvas::SET_OF_MIXED_KINDS)
        );
        assert!(
            app.resolve_drop(&[row(e)], into, &[f]).is_some(),
            "one kind alone goes in"
        );
        assert_eq!(app.resolve_drop(&[row(s)], into, &[g]), None);
        assert_eq!(
            app.set_refusal(s, &[g]),
            Some(crate::canvas::SET_OF_ANOTHER_KIND),
            "a group main onto a set of frames"
        );
        assert_eq!(app.set_refusal(s, &[f]), None, "a frame main fits it");

        // The release of a drop the panel refused for one kind per set.
        app.layer_drag = Some(LayerDrag {
            ids: vec![g],
            target: None,
            grab: (0.0, 1),
            level: 1,
            lit: None,
            spring: None,
            refused: app.set_refusal(s, &[g]),
        });
        app.finish_layer_drag();
        assert_eq!(
            app.session.status().text,
            crate::canvas::SET_OF_ANOTHER_KIND,
            "the release says why"
        );
        assert_eq!(app.session.doc.get(g).unwrap().parent(), Some(root));
    }

    /// Five passes, as `tree_guide_tests::dashes` takes.
    fn pump(app: &mut OndinApp, ctx: &egui::Context, at: Option<egui::Pos2>) -> egui::FullOutput {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(300.0, 600.0),
            )),
            ..Default::default()
        };
        if let Some(p) = at {
            input.events.push(egui::Event::PointerMoved(p));
        }
        // The button goes down on the first pass and stays down: `drop_hint`
        // refuses to read a target without a held primary button (§15 D471).
        let mut first = input.clone();
        if let Some(p) = at {
            first.events.push(press(p, true));
        }
        let mut full = ctx.run_ui(first, |ui| app.layers_tree(ui));
        for _ in 1..5 {
            full = ctx.run_ui(input.clone(), |ui| app.layers_tree(ui));
        }
        full
    }

    /// A primary press or release at `p`.
    fn press(p: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// `pump` with the frame clock pinned: all five passes see the same `t`, so a
    /// spring can only fire on a later call with a later `t`.
    fn pump_at(
        app: &mut OndinApp,
        ctx: &egui::Context,
        at: egui::Pos2,
        t: f64,
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(300.0, 600.0),
            )),
            time: Some(t),
            events: vec![egui::Event::PointerMoved(at)],
            ..Default::default()
        };
        // The button down, for [`pump`]'s reason (§15 D471).
        let mut first = input.clone();
        first.events.push(press(at, true));
        let mut full = ctx.run_ui(first, |ui| app.layers_tree(ui));
        for _ in 1..5 {
            full = ctx.run_ui(input.clone(), |ui| app.layers_tree(ui));
        }
        full
    }

    /// The row boxes of the `light` layers, top to bottom, read out of the paint
    /// rather than derived: selecting a layer makes its row paint a `SELECT_ROW`
    /// ground, one box per row and nothing else in that colour.
    fn row_rects(f: &mut Fixture, ctx: &egui::Context, light: &[NodeId]) -> Vec<egui::Rect> {
        f.app.session.selection.set(light.to_vec());
        let full = pump(&mut f.app, ctx, None);
        f.app.session.selection.clear();
        let mut rects: Vec<egui::Rect> = full
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Rect(r) if r.fill == color::SELECT_ROW => Some(r.rect),
                _ => None,
            })
            .collect();
        rects.sort_by(|a, b| a.top().total_cmp(&b.top()));
        assert_eq!(
            rects.len(),
            light.len(),
            "the fixture: one ground per lit row"
        );
        rects
    }

    /// Every row of an untouched fixture, top to bottom. Which rows are lit is not
    /// free: `sync_tree_to_selection` opens every ancestor of anything selected, so
    /// lighting a row inside a collapsed group re-opens it.
    fn open_rows(f: &mut Fixture, ctx: &egui::Context) -> Vec<egui::Rect> {
        let all = [f.frame, f.group, f.c1, f.c2, f.d, f.e];
        row_rects(f, ctx, &all)
    }

    /// A drag with no button behind it paints nothing and lands nothing (§15 D471):
    /// a button-less pointer resolves no target and draws no line. The release side
    /// is `drag_start_sync_tests::releasing_the_primary_over_the_tree_lands_the_drop`.
    /// flip: delete the `primary_down` early return → fails at the first assertion.
    #[test]
    fn a_drag_with_no_button_behind_it_paints_nothing() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let rows = open_rows(&mut f, &ctx);
        let over_root_row = rows[0].center();

        /// Arm a drag and pump with the button held or up, answering with the target
        /// derived and the drop lines drawn.
        fn stranded(
            f: &mut Fixture,
            ctx: &egui::Context,
            at: egui::Pos2,
            held: bool,
        ) -> (Option<DropTarget>, usize) {
            f.app.layer_drag = Some(LayerDrag {
                ids: vec![f.c1],
                target: None,
                grab: (0.0, 3),
                level: 3,
                lit: None,
                spring: None,
                refused: None,
            });
            let input = |events: Vec<egui::Event>| egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(300.0, 600.0),
                )),
                events,
                ..Default::default()
            };
            let mut events = vec![egui::Event::PointerMoved(at)];
            if held {
                events.push(press(at, true));
            }
            let mut full = ctx.run_ui(input(events), |ui| f.app.layers_tree(ui));
            for _ in 1..5 {
                full = ctx.run_ui(input(vec![egui::Event::PointerMoved(at)]), |ui| {
                    f.app.layers_tree(ui)
                });
            }
            let lines = full
                .shapes
                .iter()
                .filter(|cs| {
                    matches!(&cs.shape,
                        egui::Shape::LineSegment { stroke, .. } if stroke.color == color::SELECT)
                })
                .count();
            let target = f.app.layer_drag.as_ref().and_then(|d| d.target);
            f.app.layer_drag = None;
            (target, lines)
        }

        assert_eq!(
            stranded(&mut f, &ctx, over_root_row, false),
            (None, 0),
            "a pointer merely resting over a row is not a drop: no target, no line"
        );
        let (target, _lines) = stranded(&mut f, &ctx, over_root_row, true);
        assert!(
            target.is_some(),
            "control: with the button held the same position is a live drop, \
             got {target:?}"
        );
        // The control asserts the target, not the line: this aim point is inside a
        // container row, which draws an outline rather than a between-rows line.
    }

    /// Put a drag in flight carrying `ids`, picked up at `grab`, and move the pointer
    /// to `to`. Answers with what the panel decided and where it drew the line.
    fn drag(
        f: &mut Fixture,
        ctx: &egui::Context,
        ids: Vec<NodeId>,
        grab: (f32, usize),
        to: egui::Pos2,
    ) -> (Option<DropTarget>, Vec<(egui::Pos2, egui::Pos2)>) {
        f.app.layer_drag = Some(LayerDrag {
            ids,
            target: None,
            grab,
            // Starts holding the level it was picked up at, as the panel does.
            level: grab.1,
            lit: None,
            spring: None,
            refused: None,
        });
        let full = pump(&mut f.app, ctx, Some(to));
        let lines = full
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::LineSegment { points, stroke } if stroke.color == color::SELECT => {
                    Some((points[0], points[1]))
                }
                _ => None,
            })
            .collect();
        let target = f.app.layer_drag.as_ref().and_then(|d| d.target);
        f.app.layer_drag = None;
        (target, lines)
    }

    /// The indent gesture end to end: the gap under the group's last row gives two
    /// answers by sideways travel (beside the group in the frame, or the group's
    /// bottom), and the line is drawn at that level's own indent, one `INDENT` apart.
    /// flip: name one parent per boundary (the row under the pointer's list) → every
    /// assertion fails.
    #[test]
    fn one_gap_offers_a_level_per_indent_and_the_pointer_picks_it() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let rows = open_rows(&mut f, &ctx);
        // The gap between `c1` (row 3, depth 3) and `d` (row 4, depth 2).
        let under_c1 = rows[3].bottom() - 4.0;
        // Picked up on `c2`'s name, the row above, at the same level.
        let grab_x = 150.0;
        let grab = (grab_x, 3);

        let c2 = f.c2;
        let (inside, inside_lines) =
            drag(&mut f, &ctx, vec![c2], grab, egui::pos2(grab_x, under_c1));
        assert_eq!(
            inside,
            Some(DropTarget {
                parent: f.group,
                index: 0,
            }),
            "straight down from the grab, the drop stays in the group, under `c1` — \
             which is model index 0, the walk being reversed"
        );

        let (outside, outside_lines) = drag(
            &mut f,
            &ctx,
            vec![c2],
            grab,
            egui::pos2(grab_x - INDENT, under_c1),
        );
        assert_eq!(
            outside,
            Some(DropTarget {
                parent: f.frame,
                index: 1,
            }),
            "one indent to the left, the same gap drops the layer out of the group and \
             beside it in the frame"
        );

        // The line moved with it, by exactly one level.
        let left = |lines: &[(egui::Pos2, egui::Pos2)]| {
            assert_eq!(lines.len(), 1, "one drop, one line: {lines:?}");
            assert_eq!(lines[0].0.y, lines[0].1.y, "the drop line is horizontal");
            lines[0].0.x
        };
        let (deep, shallow) = (left(&inside_lines), left(&outside_lines));
        assert_eq!(
            deep - shallow,
            INDENT,
            "the two lines are drawn at the same x, so the panel is not saying which \
             level it means"
        );
        assert_eq!(
            deep - rows[3].left(),
            indent_of(3),
            "the deeper line starts where a depth-3 row starts"
        );
        assert_eq!(
            shallow - rows[3].left(),
            indent_of(2),
            "and the shallower one where a depth-2 row starts"
        );
    }

    /// The middle of a container row means "inside it" whatever the x says, as an
    /// outline never a line (§15 D67); it is the only way into a collapsed group
    /// without opening it. `d` is carried, since dropping a node into its own list
    /// plans nothing.
    /// flip: make the middle of a container row an ordinary boundary → fails at the
    /// leftmost x.
    #[test]
    fn the_middle_of_a_container_row_still_means_inside_it_whatever_the_x_says() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let rows = open_rows(&mut f, &ctx);
        let d = f.d;
        let grab_x = 150.0;
        let centre = rows[1].center().y;
        for x in [grab_x - INDENT * 2.0, grab_x, grab_x + INDENT] {
            let (target, lines) = drag(&mut f, &ctx, vec![d], (grab_x, 2), egui::pos2(x, centre));
            assert_eq!(
                target,
                Some(DropTarget {
                    parent: f.group,
                    // The end of the model list is the top of the group on screen.
                    index: 2,
                }),
                "x {x}: the middle of a container row is `into`, at the top of its list"
            );
            assert!(
                lines.is_empty(),
                "x {x}: `into` is an outline, never a line — two lines around a row read \
                 as a box, and a box is what says \"inside\" (§15 D67)"
            );
        }

        // The outline is drawn on that row and no other.
        f.app.layer_drag = Some(LayerDrag {
            ids: vec![d],
            target: None,
            grab: (grab_x, 2),
            level: 2,
            lit: None,
            spring: None,
            refused: None,
        });
        let full = pump(&mut f.app, &ctx, Some(egui::pos2(grab_x, centre)));
        f.app.layer_drag = None;
        let outlines: Vec<egui::Rect> = full
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Rect(r) if r.stroke.color == color::SELECT => Some(r.rect),
                _ => None,
            })
            .collect();
        assert_eq!(outlines, vec![rows[1]], "one outline, on the group's row");
    }

    /// The gap between two rows is not a dead band: a pointer in it means the
    /// boundary the gap is.
    /// flip: use `rect.y_range().contains(pointer.y)` → the target is `None` and no
    /// line is drawn.
    #[test]
    fn the_gap_between_two_rows_is_the_boundary_rather_than_nothing() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let rows = open_rows(&mut f, &ctx);
        let gap = rows[4].top() - rows[3].bottom();
        assert!(gap > 0.0, "the fixture: the tree's rows do not touch");
        let c2 = f.c2;
        let grab_x = 150.0;
        let in_the_gap = egui::pos2(grab_x, rows[3].bottom() + gap / 2.0);
        let (target, lines) = drag(&mut f, &ctx, vec![c2], (grab_x, 3), in_the_gap);
        assert_eq!(
            target,
            Some(DropTarget {
                parent: f.group,
                index: 0,
            }),
            "the gap under `c1` names the same boundary its lower edge does"
        );
        assert_eq!(lines.len(), 1, "and the line is still drawn: {lines:?}");
    }

    /// The pointer may leave the panel to the left (the outdent needs it) but not to
    /// the right, where the canvas is.
    #[test]
    fn the_outdent_still_works_when_the_pointer_has_left_the_panel() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let rows = open_rows(&mut f, &ctx);
        let under_d = rows[4].bottom() - 4.0;
        // Grabbed at the far left of a depth-2 row, then dragged off the panel.
        let grab_x = 18.0;
        let to = egui::pos2(grab_x - INDENT, under_d);
        assert!(
            to.x < rows[4].left(),
            "the fixture: the pointer is off the panel"
        );
        let d = f.d;
        let (target, lines) = drag(&mut f, &ctx, vec![d], (grab_x, 2), to);
        assert_eq!(
            target,
            Some(DropTarget {
                parent: f.root,
                index: 1,
            }),
            "the gap under `d` at the top level puts the layer beside the frame"
        );
        assert_eq!(lines.len(), 1, "and the line is drawn: {lines:?}");

        // To the right of the panel it drops nothing.
        let (past, no_lines) = drag(
            &mut f,
            &ctx,
            vec![d],
            (grab_x, 2),
            egui::pos2(rows[4].right() + 20.0, under_d),
        );
        assert_eq!(past, None, "off the right-hand edge is not a drop target");
        assert!(no_lines.is_empty(), "and nothing is drawn: {no_lines:?}");
    }

    /// A boundary offers the levels its two rows have in common, and no others.
    /// flip: take the floor from the row above → fails at boundary 1 (before the two
    /// boundaries that matter, which the drag tests catch).
    #[test]
    fn a_boundary_spans_the_levels_its_two_rows_share() {
        let rows = super::tree_guide_tests::marked(&[1, 2, 3, 3, 2, 1], None, &[]);
        let range = |b| {
            let r = depth_range(&rows, b);
            (*r.start(), *r.end())
        };
        assert_eq!(range(0), (1, 1));
        assert_eq!(range(1), (2, 2));
        assert_eq!(range(2), (3, 3));
        assert_eq!(range(3), (3, 3));
        // Under the group's last rect, above `d`: out of the group, or stay in it.
        assert_eq!(range(4), (2, 3));
        assert_eq!(range(5), (1, 2));
        assert_eq!(range(6), (1, 1));

        // A tree whose last row is nested offers every level out to the top.
        let deep_last = super::tree_guide_tests::marked(&[1, 2, 3], None, &[]);
        assert_eq!(
            {
                let r = depth_range(&deep_last, 3);
                (*r.start(), *r.end())
            },
            (1, 3)
        );
    }

    /// The indent is measured from where the row was picked up, never the pointer's
    /// own x.
    #[test]
    fn the_depth_asked_for_is_a_displacement_from_the_grab() {
        for x in [0.0_f32, 45.0, 150.0, 288.0] {
            assert_eq!(
                depth_asked_for(3, x, x, 3),
                3,
                "grabbed at x {x} and held still"
            );
        }
        assert_eq!(depth_asked_for(3, 150.0, 150.0 + INDENT, 3), 4);
        assert_eq!(depth_asked_for(3, 150.0, 150.0 - INDENT, 3), 2);
        assert_eq!(depth_asked_for(3, 150.0, 150.0 - INDENT * 2.0, 3), 1);
        // Far past the left edge it bottoms out rather than wrapping round.
        assert_eq!(depth_asked_for(2, 20.0, -400.0, 2), 1);
    }

    /// The level sticks to the one being held, which stops the flicker: the band is
    /// centred on the held level, so leaving rightwards and returning leftwards
    /// happen at different x (hysteresis, not a wider dead zone).
    /// flip: `STICK = 0.5` (plain rounding) → fails at the ±0.64 ends of the first
    /// loop and at the held-history pair.
    #[test]
    fn the_level_sticks_until_the_pointer_has_really_left_it() {
        let g = 150.0_f32;
        // Held at 3: anywhere inside ±STICK of its column stays 3.
        for f in [-0.64_f32, -0.5, -0.49, -0.2, 0.0, 0.2, 0.49, 0.5, 0.64] {
            assert_eq!(
                depth_asked_for(3, g, g + INDENT * f, 3),
                3,
                "{f} of an indent from the held level is still that level"
            );
        }
        // Past the band it re-rounds to the nearest level.
        assert_eq!(depth_asked_for(3, g, g + INDENT * 0.66, 3), 4);
        assert_eq!(depth_asked_for(3, g, g - INDENT * 0.8, 3), 2);
        assert_eq!(depth_asked_for(3, g, g + INDENT * 2.9, 3), 6);

        // One x, two answers, decided by where the drag already was.
        let at = |f: f32, held| depth_asked_for(3, g, g + INDENT * (f - 3.0), held);
        assert_eq!(at(2.55, 3), 3, "arrived from the right, still 3");
        assert_eq!(at(2.55, 2), 2, "arrived from the left, still 2");
        assert_ne!(
            at(2.55, 3),
            at(2.55, 2),
            "the same x reads two ways depending on where the drag was — without that \
             there is no hysteresis, only a wider edge"
        );
        // The two edges are at different x: 3 is held down to 2.35, not 2.5.
        assert_eq!(at(2.4, 3), 3, "coming back from 3, not yet");
        assert_eq!(at(2.3, 3), 2, "and now");
    }

    /// Resting a drag on a collapsed group opens it after `SPRING_SECS`: not on
    /// arrival, not early, and the wait restarts rather than accumulates.
    /// flip: restart the timer every frame (`spring = Some((id, now))`
    /// unconditionally) → the group never opens and the first assertion fails.
    #[test]
    fn resting_a_drag_on_a_collapsed_group_opens_it() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.collapsed.insert(f.group);
        // Only rows outside the group, or `sync_tree_to_selection` reopens it
        // (see `open_rows`).
        let outside = [f.frame, f.group, f.d, f.e];
        let rows = row_rects(&mut f, &ctx, &outside);
        assert!(
            f.app.collapsed.contains(&f.group),
            "the fixture: measuring the rows left the group collapsed"
        );
        let centre = rows[1].center();
        let e = f.e;
        let start = |f: &mut Fixture| {
            f.app.layer_drag = Some(LayerDrag {
                ids: vec![e],
                target: None,
                grab: (centre.x, 1),
                level: 1,
                lit: None,
                spring: None,
                refused: None,
            });
        };

        start(&mut f);
        pump_at(&mut f.app, &ctx, centre, 10.0);
        assert!(
            f.app.collapsed.contains(&f.group),
            "arriving is not resting: the group does not open on the frame it is reached"
        );
        pump_at(&mut f.app, &ctx, centre, 10.0 + SPRING_SECS * 0.6);
        assert!(
            f.app.collapsed.contains(&f.group),
            "and it does not open early — dragging across a collapsed group must leave \
             the tree as it found it"
        );
        pump_at(&mut f.app, &ctx, centre, 10.0 + SPRING_SECS + 0.05);
        assert!(
            !f.app.collapsed.contains(&f.group),
            "held for the full second, the branch opens"
        );

        // Two visits of 0.6s with a step off the group in between leave it shut.
        f.app.collapsed.insert(f.group);
        f.app.layer_drag = None;
        start(&mut f);
        let away = egui::pos2(centre.x, rows[3].center().y);
        pump_at(&mut f.app, &ctx, centre, 20.0);
        pump_at(&mut f.app, &ctx, centre, 20.0 + SPRING_SECS * 0.6);
        pump_at(&mut f.app, &ctx, away, 20.0 + SPRING_SECS * 0.7);
        pump_at(&mut f.app, &ctx, centre, 20.0 + SPRING_SECS * 0.8);
        pump_at(&mut f.app, &ctx, centre, 20.0 + SPRING_SECS * 1.3);
        assert!(
            f.app.collapsed.contains(&f.group),
            "the second visit is 0.5s old, not 1.1s: leaving the group restarts the wait"
        );
    }

    /// The container a drop would land in says so with its own ink: it marks the
    /// destination parent, not the row under the pointer, and nothing at the top
    /// level (the root has no row). Needs `pump`'s several passes, as the ink is last
    /// frame's (`LayerDrag::lit`).
    /// flip: clear `lit` at the top of the frame instead of carrying it → fails at
    /// the first assertion on `[]`. Lighting the row above the line does not bite
    /// (`lit` is derived in one place).
    #[test]
    fn the_line_names_its_container_by_colouring_that_rows_ink() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let rows = open_rows(&mut f, &ctx);
        let c2 = f.c2;
        let grab_x = 150.0;
        // The gap under `c1`: depth 3 stays in the group, depth 2 lands in the frame.
        let under_c1 = egui::pos2(grab_x, rows[3].bottom() - 4.0);

        /// Every galley the frame painted in `select_ink`, as `(text, y of its
        /// baseline)`. The colour is read off the layout job's section, as
        /// `FullOutput::shapes` is pre-tessellation.
        fn lit_text(full: &egui::FullOutput) -> Vec<(String, f32)> {
            full.shapes
                .iter()
                .filter_map(|cs| match &cs.shape {
                    egui::Shape::Text(t)
                        if t.galley
                            .job
                            .sections
                            .iter()
                            .any(|s| s.format.color == color::select_ink()) =>
                    {
                        Some((t.galley.text().to_string(), t.pos.y))
                    }
                    _ => None,
                })
                .collect()
        }

        let lit = |f: &mut Fixture, at: egui::Pos2| -> Vec<(String, f32)> {
            f.app.layer_drag = Some(LayerDrag {
                ids: vec![c2],
                target: None,
                grab: (grab_x, 3),
                level: 3,
                lit: None,
                spring: None,
                refused: None,
            });
            let full = pump(&mut f.app, &ctx, Some(at));
            f.app.layer_drag = None;
            lit_text(&full)
        };

        // Names and glyphs are asked of the document, not written out.
        let named = |id: NodeId, doc: &ondin_core::Document| {
            let n = doc.get(id).expect("in the tree");
            (n.name().to_string(), node_icon(n.kind()).0.to_string())
        };
        let (group_name, group_glyph) = named(f.group, &f.app.session.doc);
        let (frame_name, frame_glyph) = named(f.frame, &f.app.session.doc);

        let inside = lit(&mut f, under_c1);
        let texts: Vec<&str> = inside.iter().map(|(t, _)| t.as_str()).collect();
        assert!(
            texts.contains(&group_name.as_str()) && texts.contains(&group_glyph.as_str()),
            "a drop inside the group lights that row's name and glyph, not {texts:?}"
        );
        // And on the group's own row, not some row carrying the same word.
        for (t, y) in &inside {
            assert!(
                *y >= rows[1].top() && *y <= rows[1].bottom(),
                "{t:?} was lit at y {y}, off the group's row {:?}",
                rows[1]
            );
        }

        // One indent left, the drop lands in the frame — and the frame is what lights.
        let outdented = lit(&mut f, egui::pos2(grab_x - INDENT, under_c1.y));
        let texts: Vec<&str> = outdented.iter().map(|(t, _)| t.as_str()).collect();
        assert!(
            texts.contains(&frame_name.as_str()) && texts.contains(&frame_glyph.as_str()),
            "outdented, the mark moves to the frame's row, not {texts:?}"
        );
        assert!(
            !texts.contains(&group_name.as_str()),
            "and leaves the group's, or both containers claim the drop: {texts:?}"
        );

        // At the top level the parent is the root, which has no row.
        let at_root = lit(&mut f, egui::pos2(grab_x, rows[5].bottom() - 4.0));
        assert!(
            at_root.is_empty(),
            "a drop at the top level lights no container: {at_root:?}"
        );
    }

    /// A resting pointer on a row names that layer for the canvas's hover outline,
    /// and a selected row names nothing, as the canvas's own hover skips the
    /// selection.
    /// flip: `layers_row` without its `!selected` test → fails at "selected".
    #[test]
    fn hovering_a_row_outlines_its_layer_on_the_canvas() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let rows = open_rows(&mut f, &ctx);
        let hover = |f: &mut Fixture, p: egui::Pos2| {
            let input = |events| egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(300.0, 600.0),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input(vec![egui::Event::PointerMoved(p)]), |ui| {
                f.app.layers_tree(ui)
            });
            for _ in 0..3 {
                f.app.layers_hover = None;
                let _ = ctx.run_ui(input(vec![]), |ui| f.app.layers_tree(ui));
            }
            f.app.layers_hover
        };
        assert_eq!(hover(&mut f, rows[1].center()), Some(f.group));
        assert_eq!(hover(&mut f, rows[3].center()), Some(f.c1));
        f.app.session.selection.set(vec![f.c1]);
        assert_eq!(hover(&mut f, rows[3].center()), None, "selected");
    }
}

#[cfg(test)]
mod panel_width_tests {
    //! What `MIN_W` and `MAX_W` are argued from (§15 D349). Every term is the
    //! constant or function the row walk draws with, so retuning one moves these
    //! assertions; only the order they are added in is a copy.

    use super::*;

    /// Room for a hovered row's name at panel width `w`, in points.
    fn hovered_name_room(w: f32, depth: usize) -> f32 {
        let content = w - 2.0 * TREE_PAD_X as f32;
        content - icon_offset(depth, true) - NAME_GAP - (LOCK_INSET + EYE_PITCH + ICON_PT / 2.0)
    }

    /// A top-level row keeps a real name at the narrowest the drag allows:
    /// `Rectangle 12` (75.06pt) fits and `Button primary hover` (124.34) clips.
    /// flip: `MIN_W` = 180 → fails at the first assertion (66.50pt against 75.06);
    /// a minimum above 300 fails the second (and the `const` assertion at build).
    #[test]
    fn the_minimum_keeps_a_real_layer_name_on_a_top_level_row() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        // One pass so the fonts exist to lay names out with.
        let _ = ctx.run_ui(Default::default(), |_| {});
        let lays_out = |s: &str| {
            ctx.fonts_mut(|f| {
                f.layout_no_wrap(
                    s.to_string(),
                    egui::FontId::proportional(crate::ui::CONTROL_PT),
                    egui::Color32::WHITE,
                )
                .size()
                .x
            })
        };

        let room = hovered_name_room(MIN_W, 1);
        let common = lays_out("Rectangle 12");
        assert!(
            common <= room,
            "at the {MIN_W}pt minimum a top-level row has {room:.2}pt of name and \
             the default name of a new shape lays out at {common:.2} — the panel \
             can be dragged to a width where nothing is readable"
        );
        // A long name still runs under the eye, or `MIN_W` is a comfortable width,
        // not a floor.
        let long = lays_out("Button primary hover");
        assert!(
            long > room,
            "at {MIN_W}pt a {long:.2}pt name fits in {room:.2}pt, so the minimum is \
             not a floor — it is wider than the narrowest usable panel and is \
             refusing widths the user should be allowed"
        );
    }

    /// At the minimum a four-deep row (the depth §15 D330 argued from) does not
    /// draw its name underneath the eye; the band is anchored on the 194.5 crossing,
    /// not on the slack.
    /// flip: `MIN_W` = 180 → fails at the first assertion (−14.50pt).
    #[test]
    fn a_four_deep_row_is_not_drawn_under_the_eye_at_the_minimum() {
        let at_min = hovered_name_room(MIN_W, 4);
        assert!(
            at_min >= 0.0,
            "four deep at the {MIN_W}pt minimum, the name has {at_min:.2}pt — it \
             starts under the eye's cover, which is a broken row rather than a tight one"
        );
        // Just under the crossing, so this is a band rather than a one-sided bound.
        let below = hovered_name_room(194.0, 4);
        assert!(
            below < 0.0,
            "half a point under the 194.5 crossing a four-deep name still has {below:.2}pt, \
             so {MIN_W} is not where the overlap starts and the number has drifted \
             from what its doc comment says it was derived from"
        );
    }

    /// The maximum clears the deepest tree anyone would keep open: at eight levels
    /// there is still a full name's worth of space. (`DEFAULT_W` sitting between the
    /// two is a `const` assertion beside the constants.)
    #[test]
    fn the_maximum_still_holds_a_deep_tree() {
        let deep = hovered_name_room(MAX_W, 8);
        assert!(
            deep > 200.0,
            "eight deep at the {MAX_W}pt maximum leaves {deep:.2}pt of name — the \
             cap is rationing room rather than protecting the canvas"
        );
    }
}

#[cfg(test)]
mod autoscroll_ownership_tests {
    //! The tree has one drag-to-edge autoscroller, in `app::layers_panel`, not in
    //! `layers_tree` (§15 D576): two would add, since
    //! `scroll_with_delta_animation` accumulates.

    use super::*;
    use crate::app::OndinApp;
    use ondin_core::kurbo::Size;

    pub(super) const VIEW: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::Pos2::new(300.0, 120.0),
    };

    /// A headless app holding `n` rects at the root, enough to overflow the 120pt
    /// viewport.
    pub(super) fn app_with_rows(ctx: &egui::Context, n: usize) -> OndinApp {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(31);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let mut ops = Vec::new();
        let mut made = Vec::new();
        for i in 0..n {
            let id = ids.mint();
            made.push(id);
            ops.push(Operation::CreateNode {
                id,
                parent: root,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            });
        }
        doc.apply(&Transaction(ops)).expect("the rows");
        app.session.adopt_document(doc, None);
        // A live drag, the precondition for an autoscroller.
        app.layer_drag = Some(LayerDrag {
            ids: vec![made[0]],
            target: None,
            grab: (20.0, 0),
            level: 0,
            lit: None,
            spring: None,
            refused: None,
        });
        app
    }

    /// One frame of `layers_tree` inside a scroll area, with the pointer parked at
    /// `at`; returns the scroll offset after the frame.
    fn frame(ctx: &egui::Context, app: &mut OndinApp, at: egui::Pos2) -> f32 {
        let mut offset = 0.0;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(VIEW),
                events: vec![egui::Event::PointerMoved(at)],
                ..Default::default()
            },
            |ui| {
                let out = egui::ScrollArea::vertical()
                    .id_salt("autoscroll-probe")
                    .show(ui, |ui| app.layers_tree(ui));
                offset = out.state.offset.y;
            },
        );
        offset
    }

    /// `layers_tree` scrolls nothing however hard the pointer is pushed into the
    /// edge; the fixture asserts the rows overflow, or it would be vacuous. Eight
    /// frames, since the offset lags by one.
    /// flip: restore `autoscroll_while_dragging` and its call → fails at "the tree
    /// scrolls itself on pass 1".
    #[test]
    fn the_tree_does_not_scroll_itself_when_a_drag_reaches_its_edge() {
        let ctx = egui::Context::default();
        let mut app = app_with_rows(&ctx, 40);

        let mut content = 0.0;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(VIEW),
                ..Default::default()
            },
            |ui| {
                let out = egui::ScrollArea::vertical()
                    .id_salt("autoscroll-probe")
                    .show(ui, |ui| app.layers_tree(ui));
                content = out.content_size.y;
            },
        );
        assert!(
            content > VIEW.height() * 2.0,
            "the fixture: 40 rows must overflow a 120pt viewport, got {content}"
        );

        // The bottom edge: at the top the area starts at offset 0, so egui clamps
        // the delta to nothing and the fixture could not reach the state.
        let at = egui::pos2(150.0, VIEW.bottom() + 4.0);
        for pass in 0..8 {
            let offset = frame(&ctx, &mut app, at);
            assert_eq!(
                offset, 0.0,
                "the tree scrolls itself on pass {pass}: offset {offset}"
            );
        }
    }
}

#[cfg(test)]
mod virtual_tree_tests {
    //! The tree lays out only the rows in view (`OndinApp::tree_rows`).

    use super::autoscroll_ownership_tests::{VIEW, app_with_rows};
    use super::*;

    /// Only the rows in view take a widget, while the column still spans every row
    /// and a dragged row scrolled out of view keeps its own.
    /// flip: register a widget for every row in `tree_rows` → fails at "a row far
    /// below the view".
    #[test]
    fn only_the_rows_in_view_take_a_widget() {
        let ctx = egui::Context::default();
        let n = 400;
        let mut app = app_with_rows(&ctx, n);
        // Read first: with no button held, the frame ends the drag (`drop_hint`).
        let dragged = app.layer_drag.as_ref().expect("the fixture's drag").ids[0];
        let mut content = 0.0;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(VIEW),
                ..Default::default()
            },
            |ui| {
                let out = egui::ScrollArea::vertical()
                    .id_salt("virtual-probe")
                    .show(ui, |ui| app.layers_tree(ui));
                content = out.content_size.y;
            },
        );
        let rows = app.layers_flat.rows.clone();
        assert_eq!(rows.len(), n, "the fixture: every layer is a row");
        assert!(
            content >= n as f32 * ROW_H,
            "the column spans every row: {content}"
        );
        let has_widget = |id: NodeId| crate::ui::widget_rect(&ctx, row_id(id)).is_some();
        assert!(has_widget(rows[0].id), "the top row, in view");
        assert!(!has_widget(rows[n / 2].id), "a row far below the view");
        assert_eq!(rows[n - 1].id, dragged, "the fixture drags the bottom row");
        assert!(has_widget(dragged), "the dragged row, out of view");
    }
}

#[cfg(test)]
mod drag_start_sync_tests {
    //! All three doors that change the selection from inside the row walk tell the
    //! panel (`layers_synced_selection`), or the recentre moves the tree under a
    //! held pointer; and the drag ends on the primary button (§15 D581).

    use super::*;
    use crate::app::OndinApp;
    use ondin_core::kurbo::Size;

    const VIEW: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::Pos2::new(300.0, 120.0),
    };

    /// A headless app holding `n` rects at the root, and their ids in model
    /// order — bottom of the stack first, which the panel draws last.
    fn app_with_rows(ctx: &egui::Context, n: usize) -> (OndinApp, Vec<NodeId>) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(37);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let mut ops = Vec::new();
        let mut made = Vec::new();
        for i in 0..n {
            let id = ids.mint();
            made.push(id);
            ops.push(Operation::CreateNode {
                id,
                parent: root,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            });
        }
        doc.apply(&Transaction(ops)).expect("the rows");
        app.session.adopt_document(doc, None);
        // `made` is in model order; the panel draws reversed, so `ids[0]` is the
        // last row.
        (app, made)
    }

    /// One frame of `layers_tree` inside a scroll area. Returns the offset the
    /// area reports after the frame and the shapes it produced.
    fn frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        events: Vec<egui::Event>,
    ) -> (f32, Vec<egui::epaint::ClippedShape>) {
        let mut offset = 0.0;
        let full = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(VIEW),
                events,
                ..Default::default()
            },
            |ui| {
                let out = egui::ScrollArea::vertical()
                    .id_salt("drag-sync-probe")
                    .show(ui, |ui| app.layers_tree(ui));
                offset = out.state.offset.y;
            },
        );
        (offset, full.shapes)
    }

    /// A primary press or release at `p`.
    fn press(p: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// The row boxes of a fixture whose every row is selected, top to bottom
    /// (`row_rects`' technique); resets `layers_synced_selection` afterwards.
    /// Measured through a viewport tall enough for every row, since a row out of
    /// view paints no ground; the offset is 0 either way, so the boxes are `VIEW`'s.
    fn lit_rows(ctx: &egui::Context, app: &mut OndinApp, ids: &[NodeId]) -> Vec<egui::Rect> {
        app.session.selection.set(ids.to_vec());
        app.layers_synced_selection = ids.to_vec();
        let full = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    VIEW.min,
                    egui::vec2(VIEW.width(), 2000.0),
                )),
                ..Default::default()
            },
            |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("drag-sync-probe")
                    .show(ui, |ui| app.layers_tree(ui));
            },
        );
        app.session.selection.clear();
        app.layers_synced_selection = Vec::new();
        let mut rects: Vec<egui::Rect> = full
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Rect(r) if r.fill == color::SELECT_ROW => Some(r.rect),
                _ => None,
            })
            .collect();
        rects.sort_by(|a, b| a.top().total_cmp(&b.top()));
        rects
    }

    /// The root's child list, which is what a drop rearranges.
    fn children_of(app: &OndinApp) -> Vec<NodeId> {
        app.session
            .doc
            .get(app.session.doc.first_page())
            .map(|n| n.children().to_vec())
            .unwrap_or_default()
    }

    /// Only the rows in view are painted, though every row keeps its place, and a
    /// selection made out of view is still scrolled to.
    /// flip: `tree_rows` painting every row → fails at "painted out of view";
    /// `tree_rows` without its scroll → fails at "scrolled to the last row".
    #[test]
    fn only_the_rows_in_view_are_painted() {
        let ctx = egui::Context::default();
        let (mut app, ids) = app_with_rows(&ctx, 40);
        app.session.selection.set(ids.clone());
        app.layers_synced_selection = ids.clone();
        let (_, shapes) = frame(&ctx, &mut app, Vec::new());
        let grounds: Vec<egui::Rect> = shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Rect(r) if r.fill == color::SELECT_ROW => Some(r.rect),
                _ => None,
            })
            .collect();
        assert!(!grounds.is_empty(), "the fixture: rows in view are lit");
        assert!(
            grounds.len() < 10 && grounds.iter().all(|r| r.intersects(VIEW)),
            "painted out of view: {} grounds",
            grounds.len()
        );

        // `ids[0]` is the bottom row, far below the viewport.
        app.session.selection.set_one(ids[0]);
        let mut offset = 0.0;
        for _ in 0..3 {
            (offset, _) = frame(&ctx, &mut app, Vec::new());
        }
        assert!(offset > VIEW.height(), "scrolled to the last row: {offset}");
    }

    /// Scrolled sideways past a long name, no row draws text in the gutter left of
    /// the rows' tint.
    /// flip: `layers_row` painting through `ui.painter()` unclipped → fails at the
    /// gutter assertion.
    #[test]
    fn a_row_scrolled_sideways_keeps_out_of_the_gutter() {
        let ctx = egui::Context::default();
        let (mut app, ids) = app_with_rows(&ctx, 3);
        let long = "A layer whose name runs well past the panel's right edge".to_string();
        assert!(app.session.commit(Transaction(vec![Operation::SetName {
            id: ids[1],
            name: long,
        }])));
        app.layers_view_x = Some(egui::Rangef::new(0.0, 300.0));
        let mut shapes = Vec::new();
        for _ in 0..3 {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(VIEW),
                    ..Default::default()
                },
                |ui| {
                    egui::ScrollArea::horizontal()
                        .id_salt("sideways")
                        .scroll_offset(egui::vec2(40.0, 0.0))
                        .show(ui, |ui| app.layers_tree(ui));
                },
            );
            shapes = out.shapes;
        }
        let gutter = f32::from(TREE_PAD_X);
        let texts: Vec<&egui::epaint::ClippedShape> = shapes
            .iter()
            .filter(|c| matches!(c.shape, egui::Shape::Text(_)))
            .collect();
        assert!(!texts.is_empty(), "the fixture: rows drew");
        assert!(
            texts.iter().all(|c| c.clip_rect.min.x >= gutter - 0.01),
            "text clipped into the gutter: {:?}",
            texts.iter().map(|c| c.clip_rect.min.x).collect::<Vec<_>>()
        );
    }

    /// A selection made outside the panel with nothing folded leaves the fold
    /// stamp alone, so the flattened tree is not rebuilt.
    /// flip: `Folds::remove` restamping unconditionally → fails at "unmoved".
    #[test]
    fn a_selection_change_with_nothing_folded_keeps_the_tree() {
        let ctx = egui::Context::default();
        let (mut app, ids) = app_with_rows(&ctx, 40);
        frame(&ctx, &mut app, Vec::new());
        let stamp = app.collapsed.stamp;
        app.session.selection.set_one(ids[5]);
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.collapsed.stamp, stamp, "unmoved");
        app.collapsed.insert(ids[0]);
        assert_ne!(app.collapsed.stamp, stamp, "a real fold moves it");
    }

    /// A row being renamed keeps its field while it is out of view.
    /// flip: `layers_row`'s off-view test without the `renaming_layer` exception →
    /// fails at "the field is laid out".
    #[test]
    fn a_row_being_renamed_keeps_its_field_out_of_view() {
        let ctx = egui::Context::default();
        let (mut app, ids) = app_with_rows(&ctx, 40);
        // `ids[0]` is the bottom row, far below the viewport.
        app.renaming_layer = Some((ids[0], "Renamed".into()));
        let (offset, _) = frame(&ctx, &mut app, Vec::new());
        assert_eq!(offset, 0.0, "the fixture: the row stays out of view");
        assert!(
            crate::ui::widget_rect(&ctx, rename_field_id(ids[0])).is_some(),
            "the field is laid out"
        );
    }

    /// A point inside the visible half of the row straddling the viewport's bottom
    /// edge; the recentre is gated on the row not being wholly inside the clip rect.
    fn clipped_row(rects: &[egui::Rect]) -> egui::Pos2 {
        let r = rects
            .iter()
            .find(|r| r.top() < VIEW.bottom() && r.bottom() > VIEW.bottom())
            .expect("the fixture: some row must straddle the bottom edge");
        egui::pos2(150.0, (r.top() + VIEW.bottom()) * 0.5)
    }

    /// Picking up a partly-clipped row does not move the tree, and a click on the
    /// identical row is the control. `clipped_row`'s `expect` is the fixture
    /// assertion: a row wholly inside the viewport is never recentred.
    /// flip: remove the `layers_synced_selection` line in the `drag_started` arm →
    /// fails on the fourth offset while the click control stays 0.0.
    #[test]
    fn picking_up_a_partly_clipped_row_does_not_scroll_the_tree() {
        let ctx = egui::Context::default();
        let (mut app, ids) = app_with_rows(&ctx, 6);
        let rects = lit_rows(&ctx, &mut app, &ids);
        assert_eq!(rects.len(), ids.len(), "the fixture: one ground per row");
        let at = clipped_row(&rects);

        // The press, then a move past egui's drag threshold.
        let (o0, _) = frame(&ctx, &mut app, vec![press(at, true)]);
        let (o1, _) = frame(
            &ctx,
            &mut app,
            vec![egui::Event::PointerMoved(at + egui::vec2(0.0, -20.0))],
        );
        assert!(
            app.layer_drag.is_some(),
            "the fixture: the press and move must actually start a drag"
        );
        let picked_up = app.session.selection.ids().to_vec();
        assert_eq!(
            picked_up.len(),
            1,
            "the fixture: the drag arm selected the row it picked up"
        );
        // Two more frames: the sync and the scroll offset land on later passes.
        let (o2, _) = frame(&ctx, &mut app, Vec::new());
        let (o3, _) = frame(&ctx, &mut app, Vec::new());
        assert_eq!(
            (o0, o1, o2, o3),
            (0.0, 0.0, 0.0, 0.0),
            "the drag scrolled the tree"
        );

        // The control: the identical row, clicked rather than dragged.
        let (mut app, ids) = app_with_rows(&ctx, 6);
        let rects = lit_rows(&ctx, &mut app, &ids);
        let at = clipped_row(&rects);
        let (c0, _) = frame(&ctx, &mut app, vec![press(at, true)]);
        let (c1, _) = frame(&ctx, &mut app, vec![press(at, false)]);
        assert_eq!(
            app.session.selection.ids(),
            picked_up.as_slice(),
            "the control: the click must end with the identical selection, or the \
             selection is the difference between the two arms rather than the \
             bookkeeping"
        );
        let (c2, _) = frame(&ctx, &mut app, Vec::new());
        assert_eq!(
            (c0, c1, c2),
            (0.0, 0.0, 0.0),
            "the control moved, so the fixture is what is being measured"
        );
    }

    /// A selection made off the panel scrolls its row into view in one step (§15
    /// D1026), with no glide.
    /// flip: put back `ui.scroll_to_rect` → fails at "no glide".
    #[test]
    fn a_canvas_selection_jumps_the_tree_to_its_row() {
        let ctx = egui::Context::default();
        let (mut app, ids) = app_with_rows(&ctx, 30);
        let (start, _) = frame(&ctx, &mut app, Vec::new());
        assert_eq!(start, 0.0, "the fixture: the tree starts at its top");
        // Picked on the canvas, behind the panel's back; `ids[0]` is drawn last.
        app.session.selection.set_one(ids[0]);
        let offsets: Vec<f32> = (0..8)
            .map(|_| frame(&ctx, &mut app, Vec::new()).0)
            .collect();
        let first = offsets.iter().position(|o| *o > 0.0).unwrap_or_else(|| {
            panic!("the fixture: the row is off screen, so the tree scrolls: {offsets:?}")
        });
        assert!(
            offsets[first..].iter().all(|o| *o == offsets[first]),
            "no glide: {offsets:?}"
        );
    }

    /// Releasing the primary over the tree lands the drop (§15 D582): a press, a
    /// move and a release end to end. egui clears `down[Primary]` on the release
    /// frame, which `drop_hint` must still resolve.
    /// flip: narrow `!down && !released` back to `!down` → fails at "the release
    /// landed nothing" (undo_depth 0), and takes the middle-button test's control
    /// with it; `a_drag_with_no_button_behind_it_paints_nothing` stays green.
    #[test]
    fn releasing_the_primary_over_the_tree_lands_the_drop() {
        let ctx = egui::Context::default();
        let (mut app, ids) = app_with_rows(&ctx, 6);
        let rects = lit_rows(&ctx, &mut app, &ids);
        let before = children_of(&app);
        let (from, to) = (rects[2].center(), rects[0].center());

        let _ = frame(&ctx, &mut app, vec![press(from, true)]);
        for _ in 0..5 {
            let _ = frame(&ctx, &mut app, vec![egui::Event::PointerMoved(to)]);
        }
        // The fixture: a drag in flight with somewhere to go.
        assert!(
            app.layer_drag.as_ref().is_some_and(|d| d.target.is_some()),
            "the fixture: the drag must be in flight with a target"
        );

        let _ = frame(&ctx, &mut app, vec![press(to, false)]);
        assert!(app.layer_drag.is_none(), "the release ended the drag");
        assert_eq!(
            app.session.history.undo_depth(),
            1,
            "the release landed nothing"
        );
        assert_ne!(
            children_of(&app),
            before,
            "the release landed nothing: the child list is unchanged"
        );
        assert_eq!(app.session.status().text, "Reordered");
    }

    /// A middle-button release does not land a layer drag; the resolved target is
    /// the fixture assertion, and the primary release is the control.
    /// flip: `primary_released()` back to `any_released()` → fails at "the middle
    /// button landed the drag".
    #[test]
    fn a_middle_button_release_does_not_land_a_layer_drag() {
        let ctx = egui::Context::default();
        let (mut app, ids) = app_with_rows(&ctx, 6);
        let rects = lit_rows(&ctx, &mut app, &ids);
        // The third row up to the first: a real move, not a drop where it started.
        let from = rects[2].center();
        let to = rects[0].center();

        let _ = frame(&ctx, &mut app, vec![press(from, true)]);
        // Five passes, as `drop_indent_tests::pump`.
        for _ in 0..5 {
            let _ = frame(&ctx, &mut app, vec![egui::Event::PointerMoved(to)]);
        }
        assert!(
            app.layer_drag.as_ref().is_some_and(|d| d.target.is_some()),
            "the fixture: the drag must have resolved a target, or the release \
             predicate is not what decides anything"
        );
        let depth = app.session.history.undo_depth();
        let _ = frame(
            &ctx,
            &mut app,
            vec![egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Middle,
                pressed: false,
                modifiers: Default::default(),
            }],
        );
        assert!(
            app.layer_drag.is_some(),
            "the middle button landed the drag"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            depth,
            "the middle button committed a move"
        );

        // The control: the primary release ends it.
        let _ = frame(&ctx, &mut app, vec![press(to, false)]);
        assert!(
            app.layer_drag.is_none(),
            "the control: the primary release must still land the drag"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            depth + 1,
            "the control: the primary release must still commit the move"
        );
    }
}

#[cfg(test)]
mod collapse_all_tests {
    //! The collapse-all button's glyph and its action ask one question (§15 D674).

    use super::*;
    use crate::app::OndinApp;
    use ondin_core::kurbo::Size;

    /// A headless app over an `Artboard` holding a rect, a `Boolean` holding two,
    /// and a childless `Group`.
    fn app_with_containers(ctx: &egui::Context) -> (OndinApp, Vec<NodeId>) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(41);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let rect = |w: f64| NodeKind::Rect {
            size: Size::new(w, w),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        };
        let (board, inside, boolean, a, b, empty) = (
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
        );
        let mut ops = vec![
            Operation::CreateNode {
                id: board,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(100.0, 100.0),
                    corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: inside,
                parent: board,
                index: 0,
                kind: rect(10.0),
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: boolean,
                parent: board,
                index: 1,
                kind: NodeKind::Boolean {
                    op: ondin_core::node::BoolOp::Union,
                },
                transform: None,
                name: None,
            },
        ];
        for (i, id) in [a, b].into_iter().enumerate() {
            ops.push(Operation::CreateNode {
                id,
                parent: boolean,
                index: i,
                kind: rect(20.0),
                transform: None,
                name: None,
            });
        }
        ops.push(Operation::CreateNode {
            id: empty,
            parent: root,
            index: 1,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        });
        doc.apply(&Transaction(ops)).expect("the tree");
        app.session.adopt_document(doc, None);
        (app, vec![board, boolean, empty])
    }

    /// The arrow and the press agree in every state of the tree: drives the button
    /// and asserts the glyph says the opposite each time. The childless `Group` is
    /// the fixture's point (a fix that folded it is caught only here).
    /// flip: an inlined walk without the `id != root` clause → fails at the second
    /// assertion, the root being a container nothing can collapse.
    #[test]
    fn the_glyph_and_the_press_read_one_definition_of_container() {
        let ctx = egui::Context::default();
        let (mut app, containers) = app_with_containers(&ctx);

        assert!(
            !app.tree_is_all_collapsed(),
            "a fresh tree is open, so the button must offer to fold"
        );
        app.toggle_collapse_all();
        assert!(
            app.tree_is_all_collapsed(),
            "after folding, the button must offer to unfold: collapsed = {:?}",
            app.collapsed
        );
        // The set the press wrote is exactly the containers with children.
        let mut folded: Vec<NodeId> = app.collapsed.iter().copied().collect();
        folded.sort_by_key(|id| format!("{id:?}"));
        let mut want: Vec<NodeId> = containers[..2].to_vec();
        want.sort_by_key(|id| format!("{id:?}"));
        assert_eq!(folded, want, "the childless Group is not a container");

        app.toggle_collapse_all();
        assert!(
            !app.tree_is_all_collapsed(),
            "and back, on the same definition"
        );
    }
}

/// §15 D784 — a faded picture's row thumbnail is faded, and the row's own dimming
/// is a second multiplier rather than a competing one.
#[cfg(test)]
mod thumb_tint_tests {
    use super::*;

    /// An image brush at `alpha`; the `ImageId` need not name anything drawable, as
    /// the tint is computed before the thumbnail cache is asked.
    fn faded(alpha: f32) -> ondin_core::ImageBrush {
        let ondin_core::Brush::Image(mut img) =
            ondin_core::image_brush(ondin_core::ImageId("a".repeat(64)))
        else {
            unreachable!("image_brush builds an image brush")
        };
        img.sampler.alpha = alpha;
        img
    }

    /// A 30% picture fades its row thumbnail: "not opaque" is asserted before the
    /// exact value.
    #[test]
    fn a_faded_picture_fades_its_row_thumbnail() {
        let faint = thumb_tint(false, Some(&faded(0.3)));
        assert_ne!(
            faint,
            egui::Color32::WHITE,
            "a 30% picture drew an opaque thumbnail — the defect this test is for"
        );
        assert_eq!(faint, egui::Color32::WHITE.gamma_multiply(0.3));
        assert_eq!(
            thumb_tint(false, Some(&faded(1.0))),
            egui::Color32::WHITE,
            "and an opaque picture is still the identity multiplier, not a fade"
        );
        assert_eq!(
            thumb_tint(false, None),
            egui::Color32::WHITE,
            "a row with no picture at all is untinted"
        );
    }

    /// The two multipliers compose; a `match` answering dimming or alpha is wrong
    /// only where both are true.
    /// flip: ignore the picture → fails here at `102 vs 102` (and the test above);
    /// fold the alpha into the undimmed arm only → fails only here.
    #[test]
    fn dimming_and_a_faded_picture_compose_rather_than_one_winning() {
        let dim_only = thumb_tint(true, None);
        let faint_only = thumb_tint(false, Some(&faded(0.3)));
        let both = thumb_tint(true, Some(&faded(0.3)));
        assert!(
            both.a() < dim_only.a(),
            "a hidden layer's 30% picture must be fainter than dimming alone \
             ({} vs {})",
            both.a(),
            dim_only.a()
        );
        assert!(
            both.a() < faint_only.a(),
            "…and fainter than the fade alone ({} vs {})",
            both.a(),
            faint_only.a()
        );
    }

    /// `alpha` is not range-checked by the loader (`build::brush_is_finite` asks
    /// only that it is finite), so an out-of-range one is clamped, not a wrap.
    /// Coverage finding: green if `thumb_tint` ignores the picture, as both sides
    /// collapse to `WHITE`; the tests above pin that decision.
    #[test]
    fn an_out_of_range_alpha_is_clamped_at_both_ends() {
        assert_eq!(
            thumb_tint(false, Some(&faded(-3.0))),
            thumb_tint(false, Some(&faded(0.0))),
            "a negative alpha reads as fully transparent, not as a wrap"
        );
        assert_eq!(
            thumb_tint(false, Some(&faded(1e30))),
            egui::Color32::WHITE,
            "and an enormous one is opaque rather than an overflowed tint"
        );
    }
}

/// The rows' component marks for what a row's **children** differ in — a removed
/// child, an order changed, a layer of the instance's own (§15 D981 (e), D1003
/// (9)).
#[cfg(test)]
mod row_mark_tests {
    use crate::app::OndinApp;
    use ondin_core::kurbo::Size;
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    /// Main frame `m` holding rects `a` and `b`, and an instance `i` of it, made
    /// through the app's own *Create component* and *Duplicate*; and `i`'s two
    /// copies, in its child order.
    fn instance(ctx: &egui::Context) -> (OndinApp, NodeId, [NodeId; 2]) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0xD4D);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [m, a, b] = [(); 3].map(|_| ids.mint());
        let create = |id, parent, kind| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind,
            transform: None,
            name: None,
        };
        let rect = || NodeKind::Rect {
            size: Size::new(10.0, 10.0),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        };
        doc.apply(&Transaction(vec![
            create(
                m,
                root,
                NodeKind::Artboard {
                    size: Size::new(100.0, 100.0),
                    corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
            ),
            create(a, m, rect()),
            create(b, m, rect()),
        ]))
        .expect("a frame holding two rects");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(m);
        app.create_component();
        app.duplicate_selection();
        let i = app.session.selection.single().expect("the instance");
        assert_eq!(app.session.doc.get(i).unwrap().link(), Some(m));
        let kids = app.session.doc.get(i).unwrap().children().to_vec();
        assert_eq!(kids.len(), 2, "the fixture: two copies");
        app.session.selection.clear();
        (app, i, [kids[0], kids[1]])
    }

    /// The panel's dots and `+`s over three passes: a dot is
    /// `ui::painted_override_dots`' 2pt muted circle, a `+` two 1.2pt strokes.
    fn marks(app: &mut OndinApp, ctx: &egui::Context) -> (usize, usize) {
        let mut out = None;
        for _ in 0..3 {
            out = Some(ctx.run_ui(Default::default(), |ui| app.layers_tree(ui)));
        }
        let out = out.expect("drawn");
        fn walk(s: &egui::Shape, strokes: &mut usize) {
            match s {
                egui::Shape::LineSegment { stroke, .. } if stroke.width == 1.2 => *strokes += 1,
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, strokes)),
                _ => {}
            }
        }
        let mut strokes = 0;
        for s in &out.shapes {
            walk(&s.shape, &mut strokes);
        }
        (crate::ui::painted_override_dots(&out).len(), strokes / 2)
    }

    /// A parent whose children differ carries the dot, expanded or collapsed, with
    /// a tooltip naming what (§15 D1003 (9)): a copy deleted, then two copies
    /// swapped. The tooltip is asserted on `child_drift_tip`, as its ink never
    /// arrives headless.
    /// flip: drop `children.any()` from the expanded arm of the row's mark → fails
    /// at "expanded: the parent of a removed child" with 0.
    #[test]
    fn a_parent_whose_children_differ_carries_the_dot_expanded_or_collapsed() {
        let ctx = egui::Context::default();

        let (mut app, i, [ca, _]) = instance(&ctx);
        assert_eq!(marks(&mut app, &ctx), (0, 0), "an untouched instance");
        assert!(
            app.session
                .commit(Transaction(vec![Operation::DeleteNode { id: ca }]))
        );
        assert_eq!(
            marks(&mut app, &ctx).0,
            1,
            "expanded: the parent of a removed child"
        );
        assert_eq!(
            super::child_drift_tip(app.child_drift_of(i)).as_deref(),
            Some("1 removed")
        );
        app.collapsed.insert(i);
        assert_eq!(marks(&mut app, &ctx).0, 1, "collapsed: the same dot");

        let (mut app, i, [ca, cb]) = instance(&ctx);
        assert!(app.session.commit(Transaction(vec![
            Operation::Reorder { id: ca, index: 1 },
            Operation::Reorder { id: cb, index: 0 },
        ])));
        assert_eq!(
            app.session.doc.get(i).unwrap().children(),
            &[cb, ca],
            "the fixture: swapped"
        );
        assert_eq!(
            marks(&mut app, &ctx).0,
            1,
            "expanded: the parent of reordered children"
        );
        assert_eq!(
            super::child_drift_tip(app.child_drift_of(i)).as_deref(),
            Some("order changed")
        );
        app.collapsed.insert(i);
        assert_eq!(marks(&mut app, &ctx).0, 1, "collapsed: the same dot");
    }

    /// A row's mark steps aside while the row is hovered, and is back once the
    /// pointer leaves.
    ///
    /// flip: the mark drawn whatever `hovered` says → fails at "hovered: no dot".
    #[test]
    fn a_rows_mark_steps_aside_while_the_row_is_hovered() {
        let ctx = egui::Context::default();
        let (mut app, _, [ca, _]) = instance(&ctx);
        assert!(
            app.session
                .commit(Transaction(vec![Operation::DeleteNode { id: ca }]))
        );
        let pass = |app: &mut OndinApp, events: Vec<egui::Event>| {
            ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| app.layers_tree(ui),
            )
        };
        let mut out = pass(&mut app, vec![]);
        for _ in 0..2 {
            out = pass(&mut app, vec![]);
        }
        let dots = crate::ui::painted_override_dots(&out);
        assert_eq!(dots.len(), 1, "the fixture: one dot");
        for _ in 0..3 {
            out = pass(&mut app, vec![egui::Event::PointerMoved(dots[0])]);
        }
        assert!(
            crate::ui::painted_override_dots(&out).is_empty(),
            "hovered: no dot"
        );
        for _ in 0..3 {
            out = pass(&mut app, vec![egui::Event::PointerGone]);
        }
        assert_eq!(
            crate::ui::painted_override_dots(&out).len(),
            1,
            "unhovered: the dot is back"
        );
    }

    /// Every mark has a tooltip: a local layer's, an override's count, a folded
    /// row's drift, and a parent's child drift first.
    /// flip: `mark_tip` answering only `child_drift_tip` → fails at "a local layer".
    #[test]
    fn every_row_mark_has_its_words() {
        let none = ondin_core::reset::ChildDrift::default();
        assert_eq!(
            super::mark_tip(Some(super::RowMark::Local), none, 0).as_deref(),
            Some("A layer of this instance's own"),
            "a local layer"
        );
        assert_eq!(
            super::mark_tip(Some(super::RowMark::Override), none, 2).as_deref(),
            Some("2 overrides of its main")
        );
        assert_eq!(
            super::mark_tip(Some(super::RowMark::Override), none, 0).as_deref(),
            Some("Something inside differs from its main")
        );
        assert_eq!(super::mark_tip(None, none, 0), None);
    }

    /// A main's or an instance's row wears the hexagon, never its picture's
    /// thumbnail (§15 D981), asked of `wears_thumbnail` as a headless app decodes
    /// no texture.
    /// flip: `wears_thumbnail` answering `true` → fails at "a main".
    #[test]
    fn a_component_row_wears_the_hexagon_not_its_thumbnail() {
        assert!(!super::wears_thumbnail(Some(true)), "a main");
        assert!(!super::wears_thumbnail(Some(false)), "an instance");
        assert!(super::wears_thumbnail(None), "a plain picture layer");
    }

    /// The tooltip names both a removal and a changed order.
    #[test]
    fn the_childrens_tooltip_names_both() {
        let d = ondin_core::reset::ChildDrift {
            removed: 1,
            order: true,
        };
        assert_eq!(
            super::child_drift_tip(d).as_deref(),
            Some("1 removed · order changed")
        );
        assert_eq!(super::child_drift_tip(Default::default()), None);
    }

    /// A collapsed instance's dot is for anything inside differing — a layer of its
    /// own or a removed child, not only a field override (§15 D981 (e)).
    /// flip: narrow the collapsed arm's condition to `d.fields > 0` → fails at
    /// "collapsed: a layer of the instance's own" with `(0, 0)`, and at the collapsed
    /// half of the test above.
    #[test]
    fn a_collapsed_instance_marks_a_local_layer_and_a_removed_child() {
        let ctx = egui::Context::default();

        let (mut app, i, _) = instance(&ctx);
        let extra = app.session.ids.mint();
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id: extra,
            parent: i,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(4.0, 4.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        }])));
        app.collapsed.insert(i);
        assert_eq!(
            marks(&mut app, &ctx),
            (1, 0),
            "collapsed: a layer of the instance's own"
        );

        let (mut app, i, [ca, _]) = instance(&ctx);
        assert!(
            app.session
                .commit(Transaction(vec![Operation::DeleteNode { id: ca }]))
        );
        app.collapsed.insert(i);
        assert_eq!(marks(&mut app, &ctx), (1, 0), "collapsed: a removed child");
    }
}

#[cfg(test)]
mod bound_eye_tests {
    //! A bound layer's eye in the capsule, and a worded badge clear of a resting
    //! eye (§15 D1060).
    use super::*;

    use crate::app::OndinApp;
    use ondin_core::kurbo::Size;
    use ondin_core::variable::{Cell, VarKind, Variable};

    /// One frame of the tree, 300 wide, with the pointer nowhere.
    fn frame(app: &mut OndinApp, ctx: &egui::Context) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(300.0, 600.0),
            )),
            ..Default::default()
        };
        ctx.run_ui(input, |ui| app.layers_tree(ui))
    }

    /// Every filled rect and every text galley the frame painted.
    fn painted(out: &egui::FullOutput) -> (Vec<egui::Rect>, Vec<(String, egui::Rect)>) {
        fn walk(s: &egui::Shape, r: &mut Vec<egui::Rect>, t: &mut Vec<(String, egui::Rect)>) {
            match s {
                egui::Shape::Rect(x) if x.fill.a() > 0 => r.push(x.rect),
                egui::Shape::Text(x) => t.push((
                    x.galley.text().to_string(),
                    x.galley.rect.translate(x.pos.to_vec2()),
                )),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, r, t)),
                _ => {}
            }
        }
        let (mut r, mut t) = (Vec::new(), Vec::new());
        for s in &out.shapes {
            walk(&s.shape, &mut r, &mut t);
        }
        (r, t)
    }

    /// A headless app holding one 40×40 frame at the root, hidden or not, and a
    /// true boolean variable `on`.
    fn app_with_a_frame(
        ctx: &egui::Context,
        hidden: bool,
    ) -> (OndinApp, NodeId, ondin_core::VarId) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(0xBE);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let f = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: f,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(40.0, 40.0),
                    corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::SetVisible {
                id: f,
                visible: !hidden,
            },
        ]))
        .expect("a frame");
        app.session.adopt_document(doc, None);
        let mode = app.session.ids.mint_mode();
        let on = app.session.ids.mint_var();
        assert!(app.commit_edit(Transaction(vec![
            Operation::AddMode {
                collection: ondin_core::variable::CollectionId::FIRST,
                mode: ondin_core::variable::Mode {
                    id: mode,
                    name: "Light".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: on,
                    name: "state/on".into(),
                    kind: VarKind::Boolean,
                    cells: [(mode, Cell::Boolean(true))].into(),
                    scope: None,
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ])));
        (app, f, on)
    }

    /// The capsule's rect: 22×18 on the row.
    fn capsules(out: &egui::FullOutput) -> usize {
        painted(out)
            .0
            .iter()
            .filter(|r| r.size() == BOUND_EYE)
            .count()
    }

    /// A bound layer's eye stays drawn in the capsule; an unbound one at rest draws
    /// neither.
    /// flip: `show_eye` without `|| bound_eye` → fails at "bound: the capsule".
    #[test]
    fn a_bound_layers_eye_rests_in_the_capsule() {
        let ctx = egui::Context::default();
        let (mut app, f, on) = app_with_a_frame(&ctx, false);
        let _ = frame(&mut app, &ctx);
        assert_eq!(capsules(&frame(&mut app, &ctx)), 0, "unbound: no capsule");
        let ops = ondin_core::bind_visible_ops(&app.session.doc, f, on);
        assert!(app.commit_edit(Transaction(ops)));
        assert_eq!(app.session.doc.get(f).unwrap().visible_var(), Some(on));
        let _ = frame(&mut app, &ctx);
        assert_eq!(capsules(&frame(&mut app, &ctx)), 1, "bound: the capsule");
    }

    /// A frame's worded size ends before a resting eye (a hidden frame's eye is
    /// drawn at rest).
    /// flip: the badge's `right` back to `rect.right() - edge` → fails at "the size
    /// ends before the eye".
    #[test]
    fn a_worded_badge_ends_before_a_resting_eye() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = app_with_a_frame(&ctx, true);
        let _ = frame(&mut app, &ctx);
        let (_, texts) = painted(&frame(&mut app, &ctx));
        let at = |s: &str| {
            texts
                .iter()
                .find(|(t, _)| t == s)
                .unwrap_or_else(|| panic!("{s:?} painted: {texts:?}"))
                .1
        };
        let size = at("40 × 40");
        let eye = at(icon::EYE_SLASH);
        assert!(
            size.right() <= eye.left(),
            "the size ends before the eye: {size:?} against {eye:?}"
        );
    }
}

#[cfg(test)]
mod keyboard_row_tests {
    //! A focused layer row answers the keys for its own controls, and `↑`/`↓`
    //! walk the tree past the rows in view.

    use super::autoscroll_ownership_tests::{VIEW, app_with_rows};
    use super::*;
    use crate::app::OndinApp;

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// One frame of the tree in a scroll area; the offset after it. The input's
    /// modifiers are the first key's, as a held chord's are.
    fn frame(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) -> f32 {
        let mut offset = 0.0;
        let modifiers = events
            .iter()
            .find_map(|e| match e {
                egui::Event::Key { modifiers, .. } => Some(*modifiers),
                _ => None,
            })
            .unwrap_or_default();
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(VIEW),
                events,
                modifiers,
                ..Default::default()
            },
            |ui| {
                let out = egui::ScrollArea::vertical()
                    .id_salt("keyboard-probe")
                    .show(ui, |ui| app.layers_tree(ui));
                offset = out.state.offset.y;
            },
        );
        offset
    }

    /// A tree of `n` rows with the top one focused; the rows top to bottom.
    fn focused_tree(ctx: &egui::Context, n: usize) -> (OndinApp, Vec<NodeId>) {
        let mut app = app_with_rows(ctx, n);
        app.layer_drag = None;
        frame(ctx, &mut app, Vec::new());
        let rows: Vec<NodeId> = app.layers_flat.rows.iter().map(|r| r.id).collect();
        ctx.memory_mut(|m| m.request_focus(row_id(rows[0])));
        for _ in 0..2 {
            frame(ctx, &mut app, Vec::new());
        }
        (app, rows)
    }

    /// `↓` walks the focus row by row past the last row in view, scrolling it in.
    /// flip: `tree_rows` stops drawing the focused row out of view → fails at "past
    /// the view". Registering the next row before focusing it is not covered: egui
    /// forgives one undrawn frame, and the AccessKit tree it keeps whole is off here.
    #[test]
    fn down_walks_the_focus_past_the_rows_in_view() {
        let ctx = egui::Context::default();
        let (mut app, rows) = focused_tree(&ctx, 40);
        assert_eq!(
            ctx.memory(|m| m.focused()),
            Some(row_id(rows[0])),
            "the fixture focuses the top row"
        );
        let mut offset = 0.0;
        for _ in 0..12 {
            frame(
                &ctx,
                &mut app,
                vec![key(egui::Key::ArrowDown, egui::Modifiers::NONE)],
            );
            offset = frame(&ctx, &mut app, Vec::new());
        }
        assert_eq!(
            ctx.memory(|m| m.focused()),
            Some(row_id(rows[12])),
            "past the view"
        );
        assert!(offset > 0.0, "the tree scrolled to the focused row");
        frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::ArrowUp, egui::Modifiers::NONE)],
        );
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(
            ctx.memory(|m| m.focused()),
            Some(row_id(rows[11])),
            "and ↑ back"
        );
    }

    /// The hide and lock chords act on the focused row, and `F2` renames it.
    #[test]
    fn the_focused_row_hides_locks_and_renames_from_the_keys() {
        let ctx = egui::Context::default();
        let (mut app, rows) = focused_tree(&ctx, 3);
        let chord = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
        frame(&ctx, &mut app, vec![key(egui::Key::H, chord)]);
        assert!(!app.session.doc.shown(rows[0]), "Ctrl+Shift+H hid the row");
        frame(&ctx, &mut app, vec![key(egui::Key::L, chord)]);
        let locked = app.session.doc.get(rows[0]).is_some_and(|n| n.locked());
        assert!(locked, "Ctrl+Shift+L locked it");
        frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::F2, egui::Modifiers::NONE)],
        );
        assert_eq!(
            app.renaming_layer.as_ref().map(|r| r.0),
            Some(rows[0]),
            "F2 renames it"
        );
    }

    /// `Delete` on a focused row it is not selected with deletes that row alone and
    /// hands the focus to the row below; on the last row, to the one above; on a
    /// selected row, the whole selection goes.
    /// flip: `tree_rows` stops falling back the other way → fails at "the one above".
    #[test]
    fn delete_on_the_focused_row_deletes_and_keeps_the_focus_in_the_tree() {
        let ctx = egui::Context::default();
        let (mut app, rows) = focused_tree(&ctx, 4);
        app.session.selection.set_one(rows[3]);
        let delete = || vec![key(egui::Key::Delete, egui::Modifiers::NONE)];
        frame(&ctx, &mut app, delete());
        frame(&ctx, &mut app, Vec::new());
        assert!(
            app.session.doc.get(rows[0]).is_none(),
            "the focused row went"
        );
        assert!(
            app.session.doc.get(rows[3]).is_some(),
            "not the selection it was not in"
        );
        assert_eq!(
            ctx.memory(|m| m.focused()),
            Some(row_id(rows[1])),
            "the row below"
        );

        ctx.memory_mut(|m| m.request_focus(row_id(rows[3])));
        frame(&ctx, &mut app, Vec::new());
        frame(&ctx, &mut app, Vec::new());
        frame(&ctx, &mut app, delete());
        frame(&ctx, &mut app, Vec::new());
        assert!(app.session.doc.get(rows[3]).is_none(), "the last row went");
        assert_eq!(
            ctx.memory(|m| m.focused()),
            Some(row_id(rows[2])),
            "the one above"
        );

        app.session.selection.set(vec![rows[1], rows[2]]);
        frame(&ctx, &mut app, delete());
        assert!(
            app.session.doc.get(rows[1]).is_none() && app.session.doc.get(rows[2]).is_none(),
            "a selected row's Delete takes the selection"
        );
    }
}
