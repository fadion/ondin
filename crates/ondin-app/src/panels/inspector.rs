//! The property inspector.
//!
//! Reads through `display_node()` so every control reflects an in-progress
//! gesture, and writes through `edit_valve` so a slider drag is one undo step
//! rather than one per frame (§9.3).
//!
//! X/Y and rotation are shown and edited in world space; the document stores
//! local transforms (invariant 6), so each such edit projects back through
//! `build::place_at_world`. W/H stay geometry edits (§5.6).
//!
//! Each section is its own collapsible floating card; collapse state is keyed
//! by panel title and survives selection changes.
use super::component::OverrideMark as Mark;
use super::layout::SizeMode;
use super::paint::{self, PaintDrag, PaintKind, PaintList, PaintSlot};
use super::picker::Picker;
use crate::app::OndinApp;
use crate::colour_text;
use crate::preview::{Handle, PointRef, Side};
use crate::session::EditorSession;
use crate::theme::{self, color, icon};
use crate::tools;
use crate::ui::{
    self, Prefix, Scrub, Swatch, card, field_row_active, hex_of, icon_button, segment_label,
    segmented, swatch, value_field,
};
use eframe::egui;
use ondin_core::Brush;
use ondin_core::build::{Axis, Edge};
use ondin_core::kurbo::{Affine, Cap, Join, Point, Rect, RoundedRectRadii, Size, Vec2};
use ondin_core::style::StyleSlot;
use ondin_core::{
    Adjustment, DashStyle, Effect, EffectKind, Fill, GeometryPatch, GridAlign, GridAxis, GuideAxis,
    Keyed, LayoutGrid, NodeId, NodeKind, Operation, OrientOp, PaintShown, PaintTarget, Stroke,
    StrokeAlign, StrokeSides, TextSizing, Transaction, build,
};
/// What the identity row's *Use as mask* would do to the current selection.
///
/// The control is lit on [`Self::Release`], so the button's look and its click
/// cannot disagree.
enum MaskAction {
    /// Make a mask out of these, keyed on `key` if one is designated.
    Make {
        members: Vec<NodeId>,
        key: Option<NodeId>,
    },
    /// Clear the flag on every mask the selection is or contains — a list,
    /// because a group can hold two mask runs.
    Release(Vec<NodeId>),
}

/// One slot in the Align row. The design's 24×22 button, or the hairline
/// between two groups of them.
enum AlignCell {
    /// Align to an edge of the target box.
    To(&'static str, Axis, Edge, &'static str),
    /// Even out the gaps along an axis.
    Spread(&'static str, Axis, &'static str),
    Rule,
}
/// The Align row's button box: 24 wide, 22 tall.
const ALIGN_CELL: egui::Vec2 = egui::vec2(24.0, 22.0);
/// Whether anything is happening to a control, i.e. whether its transaction is
/// worth building.
///
/// Not the set [`OndinApp::edit_valve`] acts on: a focused, idle field is engaged
/// for the valve but not interacting here.
pub(crate) fn interacting(resp: &egui::Response) -> bool {
    // valve-gate-not-a-commit: decides whether to build a transaction; the valves commit on their own latch.
    resp.dragged() || resp.drag_stopped() || resp.lost_focus() || resp.changed()
}
/// The colour every guide in a set is drawn in, or `None` where they disagree.
///
/// Compared as the drawn colour rather than the `Option` override, so a guide on
/// the default and one explicitly set to it agree, as the swatch shows.
fn shared_guide_colour(guides: &[ondin_core::Guide]) -> Option<ondin_core::peniko::Color> {
    let [first, rest @ ..] = guides else {
        return None;
    };
    let colour = first.color();
    rest.iter().all(|g| g.color() == colour).then_some(colour)
}
/// Drive every layer in `ids` to one proportion-lock state, as one transaction — the
/// multi Transform card's chain button.
///
/// Set, never toggle: a mixed selection toggled member by member stays mixed, and
/// the button reads all-or-nothing. A free function so a test can reach it.
fn lock_all_tx(ids: &[NodeId], locked: bool) -> Transaction {
    Transaction(
        ids.iter()
            .map(|id| Operation::SetProportionsLocked { id: *id, locked })
            .collect(),
    )
}

/// A `PaintSlot::GroupColor` key back as a colour.
fn key_color([r, g, b, a]: [u8; 4]) -> ondin_core::peniko::Color {
    ondin_core::peniko::Color::from_rgba8(r, g, b, a)
}
/// The vertical pitch between every control in image editing's card — the
/// panel's own row gap.
const IMAGE_CARD_GAP: f32 = ui::CARD_ROW_GAP;
/// The air under each family's replace dropdown in *Missing fonts*, before the
/// next family's name.
const MISSING_FONT_GAP: f32 = ui::CARD_ROW_GAP;

/// Which tab of image editing's card is showing (§5.5a): framing and colour are
/// separate jobs, and stacking them makes the card too tall.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum ImageCard {
    /// How the picture sits in its frame: the mode, the mirrors, the tile scale,
    /// the rectangle's own actions, and the asset.
    #[default]
    Settings,
    /// The seven per-pixel adjustments.
    Adjustments,
}

impl ImageCard {
    /// Tab order, left to right.
    pub(crate) const ALL: [ImageCard; 2] = [ImageCard::Settings, ImageCard::Adjustments];

    /// The tab's label, which is also the card's heading when it is showing.
    fn title(self) -> &'static str {
        match self {
            ImageCard::Settings => "Settings",
            ImageCard::Adjustments => "Adjustments",
        }
    }
}

/// The Transform card's marks — `OndinApp::transform_marks`.
#[derive(Default)]
struct TransformMarks {
    x: Option<Mark>,
    y: Option<Mark>,
    r: Option<Mark>,
    w: Option<Mark>,
    h: Option<Mark>,
    /// A line's length (`L`, in W's slot).
    l: Option<Mark>,
}

/// `resp` with the field's own hover text, unless an override mark gives it one —
/// the mark's names the main's value, and two tooltips over one field would stack.
pub(super) fn tip_unless_marked(
    resp: egui::Response,
    mark: &Option<Mark>,
    tip: &str,
) -> egui::Response {
    match mark {
        None => resp.on_hover_text(tip),
        Some(_) => resp,
    }
}

/// The Appearance card's marks — `OndinApp::appearance_marks`. `corners` is in
/// [`Corner`]'s order.
#[derive(Default)]
pub(super) struct AppearanceMarks {
    pub(super) opacity: Option<Mark>,
    pub(super) radius: Option<Mark>,
    pub(super) corners: [Option<Mark>; 4],
    pub(super) count: Option<Mark>,
    pub(super) ratio: Option<Mark>,
    /// A polygon's or star's one radius; a rect's is `radius`.
    pub(super) vertex_radius: Option<Mark>,
    pub(super) clip: Option<Mark>,
}

/// A signed integer for an adjustment field: `+12`, `-8`, `0` (offsets from a
/// neutral zero, so zero takes no sign).
fn signed_number(v: f64, _: std::ops::RangeInclusive<usize>) -> String {
    let n = v.round() as i64;
    match n > 0 {
        true => format!("+{n}"),
        false => n.to_string(),
    }
}

/// A numeric field of the multi-selection Transform panel.
///
/// Named so the in-flight drag total (`OndinApp::multi_transform_drag`) can say
/// which field it belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MultiField {
    X,
    Y,
    W,
    H,
    Rotation,
    Skew,
}
/// What a multi-selection Transform field is asking for.
///
/// Only rotation and skew tell them apart, where the layers may disagree: a nudge
/// keeps the disagreement, an absolute resolves it.
#[derive(Clone, Copy, Debug)]
enum MultiEdit {
    /// Put the value here — typed into the field.
    Absolute(f64),
    /// Shift it by this much — dragged.
    Nudge(f64),
}
/// One corner of a rect, for the per-corner radius fields. Named rather than
/// indexed so the field and the glyph beside it cannot drift apart.
#[derive(Clone, Copy)]
enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}
impl Corner {
    const ALL: [Corner; 4] = [
        Corner::TopLeft,
        Corner::TopRight,
        Corner::BottomLeft,
        Corner::BottomRight,
    ];

    fn get(self, r: &RoundedRectRadii) -> f64 {
        match self {
            Corner::TopLeft => r.top_left,
            Corner::TopRight => r.top_right,
            Corner::BottomLeft => r.bottom_left,
            Corner::BottomRight => r.bottom_right,
        }
    }
    fn set(self, r: &mut RoundedRectRadii, v: f64) {
        let slot = match self {
            Corner::TopLeft => &mut r.top_left,
            Corner::TopRight => &mut r.top_right,
            Corner::BottomLeft => &mut r.bottom_left,
            Corner::BottomRight => &mut r.bottom_right,
        };
        *slot = v;
    }
    /// This corner's slot in a `CornerVars` — **TL, TR, BR, BL**, `kurbo`'s
    /// clockwise order, which is not [`Corner::ALL`]'s reading order: a cast of
    /// the variant would swap the bottom two.
    fn var_slot(self) -> usize {
        match self {
            Corner::TopLeft => 0,
            Corner::TopRight => 1,
            Corner::BottomRight => 2,
            Corner::BottomLeft => 3,
        }
    }
}
/// What a vertex-counted shape puts in the Appearance panel.
#[derive(Clone, Copy)]
enum Vertices {
    Polygon {
        sides: u32,
        radius: f64,
    },
    Star {
        points: u32,
        inner_ratio: f64,
        radius: f64,
    },
}
impl Vertices {
    /// The one corner radius every vertex is rounded by.
    fn radius(self) -> f64 {
        match self {
            Vertices::Polygon { radius, .. } | Vertices::Star { radius, .. } => radius,
        }
    }
}
/// The radius all four corners share, or `None` when they differ.
fn uniform(r: RoundedRectRadii) -> Option<f64> {
    ondin_core::geometry::uniform_radius(&r)
}
/// The largest of the four — what the single radius field shows while the
/// corners disagree, so dragging it starts from the shape's own scale.
fn largest(r: RoundedRectRadii) -> f64 {
    r.top_left
        .max(r.top_right)
        .max(r.bottom_right)
        .max(r.bottom_left)
}
/// Radians as an angle field should show them — and never as `-0`: angles
/// decomposed from a matrix come back a few ulps off zero, so tiny magnitudes snap.
fn shown_degrees(radians: f64) -> f64 {
    let deg = radians.to_degrees();
    if deg.abs() < 1e-9 { 0.0 } else { deg }
}

/// `world` turned a further `delta` radians about `pivot`, which is in **world**
/// space. What the Rotation field writes.
///
/// Composed, never decomposed — the same `rotate_about · old_world` as the canvas
/// ring (`tools::rotate_node`), so mirror, skew and scale survive exactly.
fn turned_about(world: Affine, pivot: Point, delta: f64) -> Affine {
    Affine::rotate_about(delta, pivot) * world
}

/// What a Rotation scrub holds for the length of its gesture.
///
/// The one inspector control that keeps state across frames. The angle re-read
/// from the matrix jumps 180° as a mirrored layer crosses ±90° (§5.7a) while
/// `DragValue` keeps its own accumulator, so without the latch the field flashes.
#[derive(Clone, Copy, Debug, PartialEq)]
struct AngleScrub {
    /// The angle the field displayed when the drag began, in degrees; the delta is
    /// measured from here, never from a re-read of the document.
    base: f64,
    /// The world transform when the drag began, which the gesture's whole turn is
    /// applied to.
    world: Affine,
    /// The number the field is showing now, fed back in as next frame's input.
    shown: f64,
}

impl AngleScrub {
    /// The `(base, world)` this frame measures from: the latch while a gesture is
    /// in flight, the document's own reading otherwise.
    fn reference(latch: Option<Self>, at_rest: f64, world: Affine) -> (f64, Affine) {
        match latch {
            Some(l) => (l.base, l.world),
            None => (at_rest, world),
        }
    }
}
/// How far a hidden paint row is faded.
///
/// Faded rather than moved or struck through: the row is still what you edit to
/// bring the paint back. 0.6 rather than the design's 0.7, which read too close
/// to a live row.
const HIDDEN_OPACITY: f32 = 0.6;

/// How far an effect's blur may be scrubbed or typed: core's cap, which the
/// bound chip's retype stops at too.
pub(super) const MAX_BLUR: f64 = ondin_core::effect::MAX_BLUR;
/// The reach of an effect's offset or spread, in both directions.
pub(super) const MAX_OFFSET: f64 = 10_000.0;

/// The range an effect number's field takes in `slot` — a blur's
/// `0..=MAX_BLUR`, an offset's or a spread's `±MAX_OFFSET` — so the bound
/// chip's retype stops where the card's fields do.
pub(super) fn effect_number_range(slot: usize) -> std::ops::RangeInclusive<f64> {
    if slot == ondin_core::EFFECT_BLUR {
        0.0..=MAX_BLUR
    } else {
        -MAX_OFFSET..=MAX_OFFSET
    }
}

/// Everything a paint row needs beyond the brush it is showing.
struct RowCtx {
    /// The node the slot hangs off — the picker's key, and the reorder's anchor.
    id: NodeId,
    /// Which paint the row's controls write to.
    slot: PaintSlot,
    /// Whether the paint is on. `false` fades the row to [`HIDDEN_OPACITY`].
    visible: bool,
    /// The list and index this row belongs to, or `None` for a paint outside a
    /// list (canvas ground, guide, decoration, shadow colour). A list earns the
    /// grip and the eye.
    grip: Option<(PaintList, usize)>,
    /// Whether the grip actually drags. `false` on a list of one; drawn the same
    /// either way so the row does not jump.
    reorderable: bool,
    /// How many layers a Group Colors row's colour reaches, or `None` on every
    /// other row. Takes the opacity and remove slot.
    census: Option<usize>,
    /// Set on the one row standing for every paint in a disagreeing selection.
    mixed: Option<MixedRow>,
    /// What the item is to an instance, for the row's reserved 14-pt trailing
    /// slot (empty while it follows, the dot when overridden, `+` when its own).
    /// `None` outside an instance, where no slot is reserved.
    item: Option<ondin_core::reset::ItemState>,
}
/// What a paint row cannot say because it stands for many paints, per property:
/// only what they genuinely disagree about reads as mixed (`build::shared_over_fills`).
#[derive(Clone, Copy)]
struct MixedRow {
    /// They do not agree on a brush: the chip shows the mixed dash, the hex `Mixed`.
    brush: bool,
    /// They do not agree on an alpha: the percentage says `Mixed`. Its own flag,
    /// since red and blue at 100% disagree on brush but not alpha.
    alpha: bool,
}
/// One property, as it stands across every paint in a selection.
///
/// What a control may say is [`Spread::agreed`]; what it operates from is
/// [`Spread::base`], which must be a real value even where they disagree.
struct Spread<T> {
    /// Each paint's value, in document order. Empty when nothing in scope is
    /// painted.
    values: Vec<T>,
    /// Some target in scope has no paint at all — a layer with no stroke selected
    /// beside one with a stroke.
    bare: bool,
}
impl<T> Default for Spread<T> {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            bare: false,
        }
    }
}
impl<T: Clone> Spread<T> {
    /// The same spread under another property, so one walk answers several
    /// questions.
    fn map<U>(&self, of: impl Fn(&T) -> U) -> Spread<U> {
        Spread {
            values: self.values.iter().map(of).collect(),
            bare: self.bare,
        }
    }
    /// The value a control **operates from**: the first paint in scope, whatever the
    /// rest of them say.
    ///
    /// It must be live: a scrubbed field or the picker re-reads its base every
    /// frame, so a placeholder constant makes the edit snap back each frame.
    fn base(&self) -> Option<T> {
        self.values.first().cloned()
    }
}
impl<T: Clone + PartialEq> Spread<T> {
    /// The one value they all agree on, or `None`.
    ///
    /// A `bare` target agrees with nothing, so it makes the answer `None` however
    /// uniform the rest are.
    fn agreed(&self) -> Option<T> {
        let first = self.values.first()?;
        (!self.bare && self.values.iter().all(|v| v == first)).then(|| first.clone())
    }
}
/// What a paint row's controls asked for. The caller owns the list, so a row only
/// reports; which of two rows' requests in one frame wins is the panel's call.
struct RowOut {
    remove: bool,
    toggle: bool,
    /// A Group Colors row's reticle was clicked: select every layer using it.
    select: bool,
    /// An instance's overridden item asked for its main's value back.
    reset: bool,
    /// What the row occupied, for the reorder to measure a drop against.
    rect: egui::Rect,
}
impl Default for RowOut {
    fn default() -> Self {
        Self {
            remove: false,
            toggle: false,
            select: false,
            reset: false,
            rect: egui::Rect::NOTHING,
        }
    }
}
/// What an effect row's controls asked for, on [`RowOut`]'s terms: the row
/// reports and the panel decides. No `rect`, since effect rows do not drag.
#[derive(Default, Clone, Copy)]
struct EffectRowOut {
    remove: bool,
    toggle: bool,
    /// The row itself was clicked: show or hide its popover.
    open: bool,
    /// An instance's overridden effect asked for its main's back.
    reset: bool,
    /// Its grip began a drag: the row is being carried to a new place in the stack.
    grabbed: bool,
}
/// The glyph an effect row wears; the design's four read as one set.
fn effect_glyph(kind: &EffectKind) -> &'static str {
    match kind {
        EffectKind::DropShadow(_) => icon::DROP_HALF_BOTTOM,
        EffectKind::InnerShadow(_) => icon::DROP_HALF,
        EffectKind::LayerBlur { .. } => icon::CIRCLE_HALF,
        EffectKind::Filters(_) => icon::SUN,
    }
}
/// The one number at the right-hand end of an effect row.
///
/// A shadow's alpha, a blur's radius, or how many filter channels are off
/// neutral ([`ondin_core::Filters::changed`]).
fn effect_summary(kind: &EffectKind) -> String {
    match kind {
        EffectKind::DropShadow(s) | EffectKind::InnerShadow(s) => {
            format!("{:.0}%", f64::from(s.color.components[3]) * 100.0)
        }
        EffectKind::LayerBlur { radius } => ui::number(2)(*radius, 0..=2),
        EffectKind::Filters(f) => f.changed().to_string(),
    }
}
/// What a row's tooltip says: what the popover holds, and what the number is.
fn effect_hint(kind: &EffectKind) -> &'static str {
    match kind {
        EffectKind::DropShadow(_) => {
            "Cast behind the layer — offset, blur, spread and colour. The number is its opacity"
        }
        EffectKind::InnerShadow(_) => {
            "Cast inside the layer — offset, blur, spread and colour. The number is its opacity"
        }
        EffectKind::LayerBlur { .. } => {
            "Blurs the layer's own drawing, children included. The number is the radius"
        }
        EffectKind::Filters(_) => {
            "Brightness, contrast, saturation and hue. The number is how many are off their \
             neutral value"
        }
    }
}
/// The first field drawn inside `card` this frame, for *Show ‹card›* to focus: the
/// first widget on this layer that senses click, drag and focus, which a value
/// field (`DragValue`) and a text field do and a button (click) or a reorder grip
/// (drag) does not. A focused `DragValue` becomes its text field.
fn first_field(ui: &egui::Ui, card: egui::Rect) -> Option<egui::Id> {
    let layer = ui.layer_id();
    ui.ctx().viewport(|v| {
        v.this_pass
            .widgets
            .get_layer(layer)
            .find(|w| {
                w.enabled
                    && card.contains_rect(w.rect)
                    && w.sense.senses_click()
                    && w.sense.senses_drag()
                    && w.sense.is_focusable()
            })
            .map(|w| w.id)
    })
}
/// [`OndinApp::sync_paint_collapse`]'s rule, over the two sets it is about — a
/// free function so a test can drive it.
fn sync_paint_collapse(
    collapsed: &mut std::collections::HashSet<&'static str>,
    auto: &mut std::collections::HashSet<&'static str>,
    title: &'static str,
    empty: bool,
    fresh: bool,
) {
    // The transition rule below only ever closes, so a new subject needs the resting state outright.
    if fresh {
        if empty {
            collapsed.insert(title);
            auto.insert(title);
        } else {
            collapsed.remove(title);
            auto.remove(title);
        }
        return;
    }
    if !empty {
        auto.remove(title);
    } else if auto.insert(title) {
        collapsed.insert(title);
    }
}
/// [`OndinApp::open_paint_panel`]'s rule, for the reason above.
fn open_paint_panel(
    collapsed: &mut std::collections::HashSet<&'static str>,
    auto: &mut std::collections::HashSet<&'static str>,
    title: &'static str,
) {
    collapsed.remove(title);
    auto.remove(title);
}
/// Lay out a paint entry that is **two rows** — a stroke's colour over its
/// properties — so that the pair reads as one entry and two entries read as two.
///
/// The rows sit [`ui::CARD_ROW_GAP`] apart and a non-`last` unit ends with a
/// second gap, so entries are twice the inner pitch apart. Gaps are allocated
/// under a zero `item_spacing`; callees inherit it and must state their own pitch.
fn paint_unit<S, A, B>(
    ui: &mut egui::Ui,
    state: &mut S,
    last: bool,
    first: impl FnOnce(&mut S, &mut egui::Ui) -> A,
    second: impl FnOnce(&mut S, &mut egui::Ui) -> B,
) -> (A, B) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let a = first(state, ui);
        ui.allocate_space(egui::vec2(0.0, ui::CARD_ROW_GAP));
        let b = second(state, ui);
        if !last {
            ui.allocate_space(egui::vec2(0.0, ui::CARD_ROW_GAP));
        }
        (a, b)
    })
    .inner
}

/// Lay out one entry of a paint list, carried under the pointer while it is the
/// one being dragged. Returns the body's value and the **slot** the entry
/// occupies — untranslated, which is what a drop has to be measured against.
///
/// `carrying` translates a tooltip-order layer's shapes vertically so the visuals
/// follow the cursor while the layout slot stays put. Widgets in that layer get
/// empty responses, so the release is read from the pointer
/// ([`OndinApp::finish_paint_drag`]).
///
/// `salt` identifies the entry through `UiBuilder::id`; `id_salt` would fold in the
/// positional counter, so a reorder would move ids and drop focus.
fn carried_unit<R>(
    ui: &mut egui::Ui,
    carrying: bool,
    salt: (&'static str, usize),
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> (R, egui::Rect) {
    let id = ui.id().with(salt);
    let builder = egui::UiBuilder::new().id(id);
    if !carrying {
        let out = ui.scope_builder(builder, body);
        return (out.inner, out.response.rect);
    }
    let layer = egui::LayerId::new(egui::Order::Tooltip, id);
    let out = ui.scope_builder(builder.layer_id(layer), body);
    if let Some(p) = ui.ctx().pointer_interact_pos() {
        let dy = p.y - out.response.rect.center().y;
        ui.ctx().transform_layer_shapes(
            layer,
            egui::emath::TSTransform::from_translation(egui::vec2(0.0, dy)),
        );
    }
    (out.inner, out.response.rect)
}
/// Who a paint edit is for, so one Fill and Stroke implementation serves a single
/// shape and a whole selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PaintScope {
    /// One layer's own paint.
    Node(NodeId),
    /// The paint every target in the selection shares
    /// (`build::paint_targets`) — a multi-selection, or a single group, whose
    /// contents *are* the selection as far as paint is concerned.
    Selection,
}
impl PaintScope {
    /// The node a slot hangs off. A selection has none, so it borrows the page as
    /// a placeholder; the slot carries the meaning.
    fn anchor(self, app: &OndinApp) -> NodeId {
        match self {
            PaintScope::Node(id) => id,
            PaintScope::Selection => app.session.page,
        }
    }
    /// The one node the scope is, or `None` for a selection.
    fn node(self) -> Option<NodeId> {
        match self {
            PaintScope::Node(id) => Some(id),
            PaintScope::Selection => None,
        }
    }
    fn slot(self, list: PaintList, index: usize) -> PaintSlot {
        match (self, list) {
            (PaintScope::Node(_), PaintList::Fill) => PaintSlot::Fill(index),
            (PaintScope::Node(_), PaintList::Stroke) => PaintSlot::Stroke(index),
            (PaintScope::Selection, l) => PaintSlot::Selection(l.target(), index),
        }
    }
}
/// One Appearance card holding a single colour and the reset beside it — the
/// guide's and the canvas ground's.
struct ColourCard<'a> {
    slot: PaintSlot,
    /// What the swatch shows and the picker starts from — real and live even while
    /// the row says *Mixed*.
    base: ondin_core::peniko::Color,
    /// The colours in scope disagree, so the chip is the mixed dash and the hex is
    /// the word. The canvas is one surface and always passes `false`.
    mixed: bool,
    swatch_hover: &'a str,
    /// The reset's hover: **live first, then dead.**
    reset_hover: (&'a str, &'a str),
    /// Whether there is anything to put back. Decides how the reset is painted, and
    /// is re-tested before the click commits.
    resettable: bool,
}
/// Which layers the Transform card's quarter-turn and mirror buttons act on, so
/// the single and multi Transform cards share one row.
#[derive(Clone, Copy)]
enum TurnScope {
    /// One layer, turned about its own transform origin.
    Node(NodeId),
    /// Every outermost selected layer.
    Selection,
}
/// Size of the kind glyph on an identity card.
const IDENTITY_ICON: f32 = 18.0;
/// Height of an identity card's first row, both kinds — measured from the theme's
/// `interact_size.y`, not set. Test-only: nothing in the layout reads it.
#[cfg(test)]
const IDENTITY_ROW_H: f32 = 24.0;
/// The identity card's second row, from the design: a 46pt dropdown then **three**
/// 28×28 action cells, `gap:7px`, everything 28pt tall.
const BOOL_COMBO_W: f32 = 46.0;
const BOOL_ROW_H: f32 = 28.0;
const BOOL_ROW_GAP: f32 = ui::CARD_COL_GAP;
/// Glyph size inside the identity row's 28×28 actions.
const ACTION_GLYPH: f32 = 16.0;
/// Outer height of the boolean dropdown's menu: the rows come to 166, the rest is
/// headroom so a retuned row height does not add a scroll area.
const BOOL_MENU_H: f32 = 220.0;

/// A full-width hairline parting one group of rows in a dropdown from the next.
///
/// [`ui::rule`] transposed, including the device-pixel snap. The gaps subtract
/// `item_spacing.y` back out so they stay absolute.
pub(super) fn popup_rule(ui: &mut egui::Ui) {
    /// Clear space above the hairline and below it, measured from the neighbouring row.
    const ABOVE: f32 = 5.0;
    const BELOW: f32 = 4.0;
    let spacing = ui.spacing().item_spacing.y;
    ui.add_space(ABOVE - spacing);
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::empty());
    let ppp = ui.ctx().pixels_per_point();
    let y = ui::hairline_in_column(rect.center().y, ppp, 1.0);
    ui.painter().line_segment(
        [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
        egui::Stroke::new(1.0, theme::color::text_a(26)),
    );
    ui.add_space(BELOW - spacing);
}

use super::op_glyph;
/// The kind glyph on an identity card, at its **optical** position.
///
/// `CIRCLE` is drawn a point lower: Phosphor's circle sits higher in the em box
/// than the square glyphs, so the card would appear to step as the subject changes.
fn identity_glyph(ui: &mut egui::Ui, glyph: &'static str, colour: egui::Color32) {
    let dy = if glyph == icon::CIRCLE { 1.0 } else { 0.0 };
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(IDENTITY_ICON), egui::Sense::empty());
    ui.painter().text(
        rect.center() + egui::vec2(0.0, dy),
        egui::Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(IDENTITY_ICON),
        colour,
    );
}
/// [`OndinApp::mask_card_targets`]'s rule, as a function of the document alone so
/// a test can ask it.
fn mask_card_targets(doc: &ondin_core::Document, id: NodeId) -> Vec<NodeId> {
    let Some(node) = doc.get(id) else {
        return Vec::new();
    };
    if node.mask() {
        return vec![id];
    }
    node.children()
        .iter()
        .copied()
        .filter(|c| doc.get(*c).is_some_and(|n| n.mask()))
        .collect()
}

/// The inspector's **first** card — the identity or summary of whatever is
/// selected.
///
/// All five headers ([`OndinApp::page_header`], [`OndinApp::inspector_header`],
/// [`OndinApp::guide_header`], [`OndinApp::points_header`],
/// [`OndinApp::inspector_multi`]) share one id salt so the ids stay stable across
/// a change of subject.
fn identity_card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.push_id("identity", |ui| card(ui, add)).inner
}
impl OndinApp {
    /// The heading card the current page gets with nothing selected, in place of a
    /// layer's identity card (`crate::pages`): its name, renamed here as in the
    /// Pages section, where it sits in the order, its *Mode* and its *Draft*
    /// switch. No eye or lock: a page is not a layer.
    fn page_header(&mut self, ui: &mut egui::Ui) {
        let id = self.session.page;
        let name0 = self.page_name(id);
        let doc = &self.session.doc;
        let place = format!(
            "Page {} of {}",
            doc.page_index(id).map_or(1, |i| i + 1),
            doc.pages().len()
        );
        let draft = doc.page(id).is_some_and(|p| p.draft);
        identity_card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 9.0;
                identity_glyph(ui, icon::FILE, color::ACCENT_300);
                // Buffered, committed once on defocus, as a layer's name is.
                let editing = matches!(&self.name_edit, Some((nid, _)) if *nid == id);
                let mut name = match &self.name_edit {
                    Some((_, buf)) if editing => buf.clone(),
                    _ => name0.clone(),
                };
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut name)
                        .frame(egui::Frame::NONE)
                        .desired_width(140.0)
                        .font(egui::FontId::proportional(13.0)),
                );
                // valve-gate-not-a-commit: a TextEdit; changed() only stashes, lost_focus() commits the typed name once.
                if resp.gained_focus() || resp.changed() {
                    self.name_edit = Some((id, name.clone()));
                }
                if resp.lost_focus()
                    && let Some((nid, val)) = self.name_edit.take()
                    && nid == id
                    && ui::defocus_commits(&resp)
                {
                    let name = val.trim().to_string();
                    if !name.is_empty() && name != name0 {
                        self.commit_edit(Transaction(vec![Operation::SetName { id, name }]));
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(place)
                            .size(10.5)
                            .color(theme::text::FAINT),
                    );
                });
            });
            self.frame_mode_row(ui, id);
            if ui::switch_row(ui, "Draft", draft, 28.0)
                .on_hover_text("Left out of kit publishing, Export all, exporting frames from all pages, the library thumbnail and presenting on past it")
                .clicked()
            {
                self.toggle_page_draft(id);
            }
        });
    }

    pub(crate) fn inspector_ui(&mut self, ui: &mut egui::Ui) {
        // A parked request only survives while its layer is still on show, or it
        // would ambush whoever selected that layer next.
        self.pending_picker
            .take_if(|(id, _)| !self.session.selection.contains(*id));
        // Before any card is drawn.
        self.gather_card_overrides();
        let guides = self.selected_guides();
        if !guides.is_empty() {
            self.inspector_guide(ui, &guides);
            return;
        }
        if let Some(id) = self.edited_path() {
            self.inspector_points(ui, id);
            return;
        }
        match self.session.selection.len() {
            0 => self.inspector_empty(ui),
            1 => {
                let id = self.session.selection.single().expect("len == 1");
                self.inspector_single(ui, id);
            }
            _ => self.inspector_multi(ui),
        }
    }
    /// The inspector for the selected ruler guides: what they are, where they
    /// are, and what colour they are drawn in — under the same three cards a
    /// layer gets.
    fn inspector_guide(&mut self, ui: &mut egui::Ui, guides: &[ondin_core::Guide]) {
        self.guide_header(ui, guides);
        self.panel(ui, "Transform", None, |app, ui| {
            app.guide_position_fields(ui, guides);
        });
        let agreed = shared_guide_colour(guides);
        // Live when any of them has been recoloured.
        let any_override = guides.iter().any(|g| g.color.is_some());
        self.colour_panel(
            ui,
            ColourCard {
                slot: PaintSlot::Guide,
                base: agreed.unwrap_or(ondin_core::DEFAULT_GUIDE_COLOR),
                mixed: agreed.is_none(),
                swatch_hover: "Edit the guide colour",
                reset_hover: ("Reset the guide colour", "Already the default guide colour"),
                resettable: any_override,
            },
            // `None`, so the guide goes back to following the default.
            || {
                Transaction(
                    guides
                        .iter()
                        .filter(|g| g.color.is_some())
                        .map(|g| Operation::SetGuideColor {
                            id: g.id,
                            color: None,
                        })
                        .collect(),
                )
            },
        );
    }
    /// The position field, or fields — one per **axis** present in the selection.
    ///
    /// A frame-scoped guide shows its offset from the frame, not a world
    /// coordinate: in a rotated frame it has none. The ruler chip quotes the same.
    fn guide_position_fields(&mut self, ui: &mut egui::Ui, guides: &[ondin_core::Guide]) {
        // Drop a target whose axis is gone, or the next drag would start from it.
        if let Some((axis, _)) = self.in_flight.guide_position_drag
            && !guides.iter().any(|g| g.axis == axis)
        {
            self.in_flight.guide_position_drag = None;
        }
        for axis in [GuideAxis::Vertical, GuideAxis::Horizontal] {
            let on_axis: Vec<&ondin_core::Guide> =
                guides.iter().filter(|g| g.axis == axis).collect();
            let [first, rest @ ..] = on_axis.as_slice() else {
                continue;
            };
            let shared = rest
                .iter()
                .all(|g| g.position == first.position)
                .then_some(first.position);
            // Held, because the layout reads the committed document and the release step is zero.
            let live = self
                .in_flight
                .guide_position_drag
                .filter(|(a, _)| *a == axis)
                .map(|(_, v)| v);
            // Real even while it says `Mixed`, so a scrub has somewhere to count from.
            let mut position = live.or(shared).unwrap_or(first.position);
            // Mixed only until the drag names a value for all of them.
            let mixed = shared.is_none() && live.is_none();
            let resp = value_field(
                ui,
                egui::vec2(ui.available_width(), 28.0),
                Prefix::Text(axis.position_label()),
                &mut position,
                Scrub::whole(0.5),
                move |d| match mixed {
                    true => d.custom_formatter(|_, _| "Mixed".into()),
                    false => d.custom_formatter(ui::number(2)),
                },
            );
            if resp.dragged() {
                self.in_flight.guide_position_drag = Some((axis, position));
            }
            // `continue`, not `return`, so the other axis is still laid out.
            if self.gesture_cancelled {
                self.in_flight.guide_position_drag = None;
                continue;
            }
            if !interacting(&resp) {
                continue;
            }
            let ops: Vec<Operation> = on_axis
                .iter()
                .map(|g| Operation::SetGuidePosition { id: g.id, position })
                .collect();
            self.edit_valve(&resp, Transaction(ops));
            if !resp.dragged() {
                self.in_flight.guide_position_drag = None;
            }
        }
    }
    /// The guides' identity card: what they are, how many, and what can be done
    /// to them. No eye, lock or rename — a guide is not a layer.
    fn guide_header(&mut self, ui: &mut egui::Ui, guides: &[ondin_core::Guide]) {
        let title = match guides.len() {
            0 | 1 => "Guide".to_string(),
            n => format!("{n} Guides"),
        };
        identity_card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 9.0;
                identity_glyph(ui, icon::RULER, color::ACCENT_300);
                ui.label(
                    egui::RichText::new(title)
                        .size(13.0)
                        .color(theme::text::STRONG),
                );
            });
            self.guide_actions(ui, guides);
        });
    }
    /// Remove, and Remove all Guides — the guide card's action row, in two glyph
    /// families so the pair is not told apart only by tooltip.
    fn guide_actions(&mut self, ui: &mut egui::Ui, guides: &[ondin_core::Guide]) {
        let total = self.session.doc.guides().len();
        let mut remove = false;
        let mut remove_all = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = BOOL_ROW_GAP;
            ui.spacing_mut().interact_size.y = BOOL_ROW_H;
            ui.spacing_mut().button_padding.y = 0.0;
            remove = ui::icon_button(ui, icon::X, BOOL_ROW_H, ACTION_GLYPH, false, true)
                .on_hover_text(match guides.len() {
                    0 | 1 => "Remove this guide".to_string(),
                    n => format!("Remove these {n} guides"),
                })
                .clicked();
            // Live even when every guide is selected, where it equals Remove.
            remove_all = ui::icon_button(
                ui,
                icon::TRASH_SIMPLE,
                BOOL_ROW_H,
                ACTION_GLYPH,
                false,
                true,
            )
            .on_hover_text(format!("Remove all {total} guides in this document"))
            .clicked();
        });
        // After the row, so the borrow of `guides` is over first.
        if remove_all {
            self.remove_all_guides();
        } else if remove {
            self.remove_selected_guides();
        }
    }
    /// A paint row for something that has exactly one colour: swatch and hex, no
    /// opacity and no remove — the guide's row and the canvas ground's.
    ///
    /// `mixed` puts the chip on the mixed dash and the hex on the word; the canvas
    /// is never mixed.
    fn single_paint_row(
        &mut self,
        ui: &mut egui::Ui,
        size: egui::Vec2,
        slot: PaintSlot,
        base: ondin_core::peniko::Color,
        mixed: bool,
        hover: &str,
    ) -> bool {
        let targeted = self.picker.as_ref().is_some_and(|p| p.slot == slot);
        let mut open = false;
        field_row_active(ui, size, targeted, |ui| {
            // The recessed ground shrink-wraps; with no opacity field it would stop past the hex.
            ui.set_min_width(ui.available_width());
            ui.spacing_mut().item_spacing.x = 9.0;
            let chip = match mixed {
                true => Swatch::Mixed,
                false => Swatch::Solid(ui::color_to_egui(base)),
            };
            if swatch(ui, 16.0, color::FIELD, chip)
                .on_hover_text(hover)
                .clicked()
            {
                open = true;
            }
            // A real brush while it says `Mixed`, so the picker has somewhere to start.
            let brush = Brush::Solid(base);
            self.paint_hex_field(ui, self.session.page, slot, &brush, mixed);
        });
        open
    }
    /// With nothing selected, the inspector describes the current page: its card,
    /// then its ground colour, and a reset to the default that is dead while
    /// already there.
    fn inspector_empty(&mut self, ui: &mut egui::Ui) {
        let ground = self.session.preview_ground();
        let page = self.session.page;
        let is_default = ground.to_rgba8().to_u8_array()
            == crate::session::DEFAULT_CANVAS_BACKGROUND
                .to_rgba8()
                .to_u8_array();
        self.page_header(ui);
        self.colour_panel(
            ui,
            ColourCard {
                slot: PaintSlot::Canvas,
                base: ground,
                mixed: false,
                swatch_hover: "Edit the canvas colour",
                reset_hover: (
                    "Reset the canvas colour",
                    "Already the default canvas colour",
                ),
                resettable: !is_default,
            },
            || {
                Transaction(vec![Operation::SetPageGround {
                    id: page,
                    ground: crate::session::DEFAULT_CANVAS_BACKGROUND,
                }])
            },
        );
        self.missing_fonts_card(ui);
    }

    /// The families the document draws with that this machine can never have,
    /// each with the layers set in it.
    pub(crate) fn missing_fonts(&mut self) -> ondin_core::families::FamilyUses {
        let revision = self.session.revision();
        if self
            .font_uses
            .as_ref()
            .is_none_or(|(at, _)| *at != revision)
        {
            let uses = ondin_core::families::families_in_use(&self.session.doc);
            self.font_uses = Some((revision, uses));
        }
        let uses = self
            .font_uses
            .as_ref()
            .map(|(_, u)| u.clone())
            .unwrap_or_default();
        uses.into_iter()
            .filter(|(name, _)| {
                self.fonts.family_status(name) == crate::fonts::FamilyStatus::Missing
            })
            .collect()
    }

    /// *Missing fonts*, on the canvas's inspector while the document has any: a
    /// row per family — its name selects the layers set in it — and a dropdown
    /// that replaces it everywhere (`families::replace_family`), one undo step.
    fn missing_fonts_card(&mut self, ui: &mut egui::Ui) {
        let missing = self.missing_fonts();
        if missing.is_empty() {
            return;
        }
        let mut pick: Option<(String, String)> = None;
        let mut select: Option<Vec<NodeId>> = None;
        self.panel_head(
            ui,
            "Missing fonts",
            Some(missing.len()),
            None,
            None,
            |app, ui| {
                for (name, layers) in &missing {
                    // The count first, from the right, so a long name truncates
                    // into what it leaves rather than pushing it off the card.
                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.label(
                                egui::RichText::new(match layers.len() {
                                    1 => "1 layer".to_string(),
                                    n => format!("{n} layers"),
                                })
                                .size(11.0)
                                .color(theme::text::FAINT),
                            );
                            ui.with_layout(
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    let label =
                                        ui::glyph_and_text_tinted(icon::TEXT_AA, name, color::WARN);
                                    let resp = ui
                                        .add(
                                            egui::Label::new(label)
                                                .sense(egui::Sense::click())
                                                .truncate(),
                                        )
                                        .on_hover_text("Select the layers set in it");
                                    if resp.clicked() {
                                        select = Some(layers.clone());
                                    }
                                },
                            );
                        });
                    });
                    ui.scope(|ui| {
                        ui.spacing_mut().interact_size.y = ui::CONTROL_H;
                        ui.spacing_mut().button_padding.y = 0.0;
                        egui::ComboBox::from_id_salt(("replace-font", name.as_str()))
                            .icon(ui::combo_chevron)
                            .width(ui.available_width())
                            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                            .selected_text(
                                egui::RichText::new("Replace with…").color(theme::text::FAINT),
                            )
                            .show_ui(ui, |ui| {
                                ui::menu_rows(ui);
                                ui.spacing_mut().item_spacing.y = 3.0;
                                if let Some(to) = app.family_list(ui, "") {
                                    pick = Some((name.clone(), to));
                                }
                            });
                    });
                    ui.add_space(MISSING_FONT_GAP);
                }
            },
        );
        if let Some(mut ids) = select {
            ids.retain(|id| self.may_select_in_bulk(*id));
            self.session.selection.set(ids);
        }
        if let Some((from, to)) = pick {
            self.fonts.ensure_loaded(&to);
            let tx = ondin_core::families::replace_family(&self.session.doc, &from, &to);
            if tx.0.is_empty() {
                // What is left names it through a kit's style or variable.
                self.session.info(format!(
                    "{from} is set by a kit; its next update replaces it"
                ));
            } else {
                self.commit_edit(tx);
                self.session.info(format!("Replaced {from} with {to}"));
            }
        }
    }
    /// An Appearance card holding one colour and the reset beside it — the guide
    /// card's and the canvas card's.
    ///
    /// `resettable` is tested twice and both matter: a disabled `field_button`
    /// still reports its click, so `enabled_if` only paints it and the
    /// `&& resettable` refuses the write.
    fn colour_panel(
        &mut self,
        ui: &mut egui::Ui,
        card: ColourCard<'_>,
        reset: impl FnOnce() -> Transaction,
    ) {
        let anchor = ui.cursor().min;
        self.panel(ui, "Appearance", None, |app, ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                let row = egui::vec2(ui.available_width() - 28.0 - ui::CARD_COL_GAP, 28.0);
                let opened = app.single_paint_row(
                    ui,
                    row,
                    card.slot,
                    card.base,
                    card.mixed,
                    card.swatch_hover,
                );
                let (live, dead) = card.reset_hover;
                if ui::field_button(
                    ui,
                    icon::ARROW_COUNTER_CLOCKWISE,
                    28.0,
                    15.0,
                    ui::FieldButton::enabled_if(card.resettable),
                )
                .on_hover_text(if card.resettable { live } else { dead })
                .clicked()
                    && card.resettable
                {
                    app.commit_edit(reset());
                }
                if opened {
                    app.open_picker(ui, app.session.page, card.slot, anchor);
                }
            });
        });
    }
    /// Multi-selection: counts plus the operations that only make sense on
    /// several layers at once.
    ///
    /// The identity card is the single-layer one minus what cannot apply: the name
    /// is a heading, and the eye and lock go.
    fn inspector_multi(&mut self, ui: &mut egui::Ui) {
        let n = self.session.selection.len();
        identity_card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 9.0;
                // Not `SELECTION`, which the layers tree spends on a `Group`.
                identity_glyph(ui, icon::BOUNDING_BOX, color::ACCENT_300);
                ui.label(
                    egui::RichText::new(format!("{n} layers"))
                        .size(13.0)
                        .color(theme::text::STRONG),
                );
            });
            self.identity_boolean_row(ui);
            self.inspector_component(ui, super::component::ComponentPart::Identity);
        });
        self.inspector_component(ui, super::component::ComponentPart::Cards);
        // Same panels, same order as on a single layer; each draws nothing unless
        // the selection has something it describes.
        self.inspector_frame_templates(ui);
        self.inspector_align(ui);
        self.inspector_multi_transform(ui);
        self.inspector_insets(ui);
        self.inspector_item(ui);
        self.inspector_container(ui);
        self.inspector_layout_grids(ui);
        self.inspector_multi_appearance(ui);
        // One text layer among others gets its own Type card; several get the
        // card that writes them all.
        let texts: Vec<NodeId> = self
            .session
            .selection
            .ids()
            .iter()
            .copied()
            .filter(|id| {
                self.session
                    .doc
                    .get(*id)
                    .is_some_and(|n| matches!(n.kind(), NodeKind::Text { .. }))
            })
            .collect();
        let subject = match texts.as_slice() {
            [one] => super::TypeSubject::of(self, *one),
            many => super::TypeSubject::several(self, many),
        };
        if let Some(subject) = subject {
            self.inspector_type(ui, subject);
        }
        let fresh = self.paint_subject_changed();
        self.inspector_paint_panels(ui, PaintScope::Selection, fresh);
        self.inspector_group_colors(ui);
        // Under the paint panels: an effect applies to the painted layer (§6.4).
        let subjects = self.session.selection.ids().to_vec();
        self.inspector_effects(ui, &subjects);
        self.inspector_export(ui);
    }
    fn inspector_single(&mut self, ui: &mut egui::Ui, id: NodeId) {
        // Snapshot everything needed, then drop the borrow before mutating.
        let Some(node) = self.session.display_node(id) else {
            return;
        };
        let name0 = node.name().to_string();
        let visible0 = node.visible();
        let locked0 = node.locked();
        let opacity0 = node.opacity();
        // World, not local; projected back through `build::local_for_world`.
        let world = self
            .session
            .preview_world_transform(id)
            .unwrap_or(Affine::IDENTITY)
            .as_coeffs();
        let (glyph, _) = super::layers_icon(node.kind());
        let size0 = match node.kind() {
            NodeKind::Rect { size, .. }
            | NodeKind::Ellipse { size }
            | NodeKind::Polygon { size, .. }
            | NodeKind::Star { size, .. }
            | NodeKind::Artboard { size, .. } => Some(*size),
            // Read from `local_box` so the row agrees with the handles.
            NodeKind::Text { .. } => self.session.preview_local_box(id).map(|b| b.size()),
            // A derived box; an empty group has none, so the row does not appear.
            NodeKind::Path { .. } | NodeKind::Boolean { .. } | NodeKind::Group => {
                self.session.preview_local_box(id).map(|b| b.size())
            }
            _ => None,
        };
        // As drawn: a bound corner's literal is 0, so the fields read its
        // variable's number in the mode in force.
        let corners0 = match node.kind() {
            NodeKind::Rect { corner_radii, .. } => Some(
                self.session
                    .doc
                    .resolve_corners(id, *corner_radii)
                    .unwrap_or(*corner_radii),
            ),
            _ => None,
        };
        let vertices0 = match node.kind() {
            NodeKind::Polygon {
                sides,
                corner_radius,
                ..
            } => Some(Vertices::Polygon {
                sides: *sides,
                radius: *corner_radius,
            }),
            NodeKind::Star {
                points,
                inner_ratio,
                corner_radius,
                ..
            } => Some(Vertices::Star {
                points: *points,
                inner_ratio: *inner_ratio,
                radius: *corner_radius,
            }),
            _ => None,
        };
        // Only frames clip; `None` keeps the checkbox out of other layers' panels.
        let clip0 = matches!(node.kind(), NodeKind::Artboard { .. }).then(|| node.clip());
        // `takes_paint`, not a list of kinds spelled again.
        let paintable = node.kind().takes_paint();
        let is_text = matches!(node.kind(), NodeKind::Text { .. });
        let is_container = matches!(node.kind(), NodeKind::Group | NodeKind::Artboard { .. });
        let node_kind = node.kind().clone();
        self.inspector_header(ui, id, glyph, &name0, visible0, locked0);
        // What the layer is, so under its name and above the mask.
        self.inspector_component(ui, super::component::ComponentPart::Cards);
        // A mask draws nothing, so it is a fact about what the layer is.
        self.inspector_mask(ui, id);
        // Cards run from the layer in its parent, to its children, to paint. None is
        // gated on kind: each draws nothing for a selection it cannot describe.
        self.inspector_frame_templates(ui);
        self.inspector_align(ui);
        self.inspector_transform(ui, id, world, size0);
        // Under the numbers it changes the meaning of: a pinned layer's X and W
        // are where its insets put it.
        self.inspector_insets(ui);
        self.inspector_item(ui);
        self.inspector_container(ui);
        self.inspector_layout_grids(ui);
        // A selected path gets the corner radius without entering point editing,
        // writing to every corner.
        if matches!(node_kind, NodeKind::Path { .. }) {
            self.points_appearance(ui, id);
        } else {
            self.inspector_appearance(ui, id, opacity0, corners0, vertices0, clip0);
        }
        // After the borrow above is dropped: the subject also reads the editing
        // session's selection.
        if is_text && let Some(subject) = super::TypeSubject::of(self, id) {
            self.inspector_type(ui, subject);
        }
        // A group has no paint of its own, so its Fill and Stroke are its
        // contents', as for a multi-selection; a frame's paint is its own.
        if matches!(node_kind, NodeKind::Group) {
            let fresh = self.paint_subject_changed();
            self.inspector_paint_panels(ui, PaintScope::Selection, fresh);
        } else if paintable {
            let fresh = self.paint_subject_changed();
            self.inspector_paint_panels(ui, PaintScope::Node(id), fresh);
        }
        // A container gets the colours of what is inside it (`group_color_scope`).
        if is_container {
            self.inspector_group_colors(ui);
        }
        // Under the paint panels: an effect applies to the painted layer (§6.4).
        self.inspector_effects(ui, &[id]);
        // Last: it describes what comes out of the layer.
        self.inspector_export(ui);
    }
    /// The inspector while a path's **points** are the subject.
    ///
    /// A point has only a position, so Transform keeps X and Y. Appearance, Fill
    /// and Stroke stay pointed at the node; [`Self::inspector_align`] aligns the
    /// selected points.
    fn inspector_points(&mut self, ui: &mut egui::Ui, id: NodeId) {
        self.points_header(ui);
        self.inspector_align(ui);
        self.points_transform(ui, id);
        self.points_appearance(ui, id);
        let fresh = self.paint_subject_changed();
        self.inspector_paint_panels(ui, PaintScope::Node(id), fresh);
    }

    /// The identity card in point-editing mode: "Edit Path", and every action on
    /// it dead.
    ///
    /// Disabled rather than absent, so they read as "not for this subject".
    fn points_header(&mut self, ui: &mut egui::Ui) {
        identity_card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 9.0;
                identity_glyph(ui, icon::BEZIER_CURVE, color::ACCENT_300);
                ui.label(
                    egui::RichText::new("Edit Path")
                        .size(13.0)
                        .color(theme::text::STRONG),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let _ = icon_button(ui, icon::LOCK_SIMPLE_OPEN, 22.0, 16.0, false, false);
                    let _ = icon_button(ui, icon::EYE, 22.0, 16.0, false, false);
                });
            });
            // The live row, inert, so it cannot drift from it.
            ui.add_enabled_ui(false, |ui| self.identity_boolean_row(ui));
        });
    }

    /// X and Y of the selected points.
    ///
    /// The box the selection occupies; a typed number moves the set by the
    /// difference. Disabled with nothing selected, showing the path's own corner.
    fn points_transform(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let selected = self.selected_point_positions(id);
        let origin = selected
            .iter()
            .copied()
            .reduce(|a, b| Point::new(a.x.min(b.x), a.y.min(b.y)))
            .or_else(|| {
                let world = self.session.preview_world_transform(id)?;
                let b = self.session.preview_local_box(id)?;
                Some(world * Point::new(b.x0, b.y0))
            })
            .unwrap_or(Point::ZERO);
        let enabled = !selected.is_empty();
        self.panel(ui, "Transform", None, |app, ui| {
            let fw = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
            let row = egui::vec2(fw, 28.0);
            ui.add_enabled_ui(enabled, |ui| {
                ui.horizontal(|ui| {
                    for (label, axis) in [("X", Axis::X), ("Y", Axis::Y)] {
                        let was = match axis {
                            Axis::X => origin.x,
                            Axis::Y => origin.y,
                        };
                        let mut v = was;
                        let resp = value_field(
                            ui,
                            row,
                            Prefix::Text(label),
                            &mut v,
                            Scrub::whole(0.5),
                            |d| d.custom_formatter(ui::number(2)),
                        );
                        if !enabled {
                            continue;
                        }
                        let delta = match axis {
                            Axis::X => Vec2::new(v - was, 0.0),
                            Axis::Y => Vec2::new(0.0, v - was),
                        };
                        // The increment form, because a scrub reports one frame's
                        // step (`canvas::nudge_points_tx`).
                        let tx = app.nudge_points_tx(id, delta);
                        app.edit_valve(&resp, tx);
                        if interacting(&resp) {
                            app.reveal_selection();
                        }
                    }
                });
                app.point_handle_rows(ui, id, row);
            });
        });
    }

    /// The selected anchor's two control points, as X/Y — the row pair below the
    /// anchor's own position.
    ///
    /// X/Y prefixed `←`/`→` for incoming and outgoing — two glyphs at most, since a
    /// field whose digits do not fit grows. Dimmed when there is no single anchor
    /// with handles.
    fn point_handle_rows(&mut self, ui: &mut egui::Ui, id: NodeId, row: egui::Vec2) {
        for (side, prefix, name) in [
            (
                Side::Back,
                ["←X", "←Y"],
                "Incoming handle — the curve arrives along it",
            ),
            (
                Side::Out,
                ["→X", "→Y"],
                "Outgoing handle — the curve leaves along it",
            ),
        ] {
            let handle = self.selected_handle(id, side);
            let enabled = handle.is_some();
            let at = handle.map(|(at, _)| at);
            let position = handle.map(|(_, p)| p).unwrap_or(Point::ZERO);
            ui.add_enabled_ui(enabled, |ui| {
                ui.horizontal(|ui| {
                    for (label, axis) in [(prefix[0], Axis::X), (prefix[1], Axis::Y)] {
                        let was = match axis {
                            Axis::X => position.x,
                            Axis::Y => position.y,
                        };
                        let mut v = was;
                        let resp = value_field(
                            ui,
                            row,
                            Prefix::Text(label),
                            &mut v,
                            Scrub::whole(0.5),
                            |d| d.custom_formatter(ui::number(2)),
                        )
                        // `on_hover_text` never fires inside a disabled scope.
                        .on_hover_text(format!("{name}, {axis:?}"))
                        .on_disabled_hover_text("Select one point to edit its handles");
                        let Some(at) = at.filter(|_| enabled) else {
                            continue;
                        };
                        let delta = match axis {
                            Axis::X => Vec2::new(v - was, 0.0),
                            Axis::Y => Vec2::new(0.0, v - was),
                        };
                        // The increment form, as the anchor's own fields use.
                        let tx = self.nudge_handle_tx(id, at, side, delta);
                        self.edit_valve(&resp, tx);
                        if interacting(&resp) {
                            self.reveal_selection();
                        }
                    }
                });
            });
        }
    }

    /// Opacity and corner radius, while points are the subject.
    ///
    /// Opacity is the layer's. The radius writes only to corner points and dims
    /// when none is selected. The per-corner button is permanently dead and holds
    /// the slot. The radius does not bind to a variable: a path's radii are indexed
    /// by anchor, and point editing adds and removes anchors (§15 D1057).
    fn points_appearance(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let opacity0 = self
            .session
            .display_node(id)
            .map(|n| n.opacity())
            .unwrap_or(1.0);
        // The subject, so a selected segment rounds its two corners.
        let picked: Vec<PointRef> = self.subject_points(id);
        let scope = (!picked.is_empty()).then_some(picked);
        let radius0 = self.corner_radius_of(id, scope.as_deref());
        self.panel(ui, "Appearance", None, |app, ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                let fw = (ui.available_width() - 28.0 - ui::CARD_COL_GAP * 2.0) / 2.0;
                // The layer's own opacity field, chip and all.
                app.opacity_field(ui, id, opacity0, fw, &None);

                let enabled = radius0.is_some();
                ui.add_enabled_ui(enabled, |ui| {
                    let mut r = radius0.unwrap_or(0.0);
                    let rr = value_field(
                        ui,
                        egui::vec2(fw, 28.0),
                        Prefix::Icon(icon::CORNERS_OUT),
                        &mut r,
                        Scrub::whole(0.5).range(0.0..=f64::MAX),
                        |d| d.custom_formatter(ui::number(2)),
                    )
                    // Split: `on_hover_text` never fires while disabled.
                    .on_hover_text(match scope.is_some() {
                        true => "Round the selected corner points",
                        false => "Round every corner point of this path",
                    })
                    .on_disabled_hover_text(
                        "No corner point selected — a smooth point has no corner to round",
                    );
                    if enabled {
                        let tx = app.corner_radius_tx(id, scope.as_deref(), r);
                        app.edit_valve(&rr, tx);
                    }
                });
                let _ = ui::field_button(
                    ui,
                    icon::SQUARE_SPLIT_HORIZONTAL,
                    28.0,
                    15.0,
                    ui::FieldButton::enabled_if(false),
                )
                .on_hover_text("Per-corner radii: select the points you want and set each");
            });
        });
    }

    /// The identity card for a *layer*: kind icon, editable name, visibility
    /// and lock.
    ///
    /// No kind pill: the generated name and the icon already say what it is.
    fn inspector_header(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        glyph: &'static str,
        name0: &str,
        visible0: bool,
        locked0: bool,
    ) {
        identity_card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 9.0;
                identity_glyph(ui, glyph, color::ACCENT_300);
                // Name — buffered, committed once on defocus.
                let editing = matches!(&self.name_edit, Some((nid, _)) if *nid == id);
                let mut name = match &self.name_edit {
                    Some((_, buf)) if editing => buf.clone(),
                    _ => name0.to_string(),
                };
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut name)
                        .frame(egui::Frame::NONE)
                        .desired_width(140.0)
                        .font(egui::FontId::proportional(13.0)),
                );
                // The I-beam is kept: the frameless field has no other cue.
                if resp.gained_focus() || resp.changed() {
                    self.name_edit = Some((id, name.clone()));
                }
                // The `take` sits before `defocus_commits`, so Escape still drops the buffer.
                if resp.lost_focus()
                    && let Some((nid, val)) = self.name_edit.take()
                    && nid == id
                    && ui::defocus_commits(&resp)
                {
                    let name = val.trim().to_string();
                    if !name.is_empty() && name != name0 {
                        self.commit_edit(Transaction(vec![Operation::SetName { id, name }]));
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let lock = if locked0 {
                        icon::LOCK_SIMPLE
                    } else {
                        icon::LOCK_SIMPLE_OPEN
                    };
                    if icon_button(ui, lock, 22.0, 16.0, locked0, true)
                        .on_hover_text(if locked0 { "Unlock" } else { "Lock" })
                        .clicked()
                    {
                        self.commit_edit(Transaction(vec![Operation::SetLocked {
                            id,
                            locked: !locked0,
                        }]));
                    }
                    let eye = if visible0 { icon::EYE } else { icon::EYE_SLASH };
                    // Bound to a boolean variable, the eye sits in the chip's capsule;
                    // a click toggles and so detaches.
                    let key = super::binding::FieldKey::Visible { id };
                    let resp = match self.field_var(key) {
                        Some(var) => {
                            let (rect, resp) = ui
                                .allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
                            let cap =
                                egui::Rect::from_center_size(rect.center(), egui::vec2(22.0, 18.0));
                            let fill = if resp.hovered() { 44 } else { 28 };
                            ui.painter().rect_filled(cap, 9, color::text_a(fill));
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                eye,
                                theme::icon_font(14.0),
                                color::TEXT,
                            );
                            let doc = &self.session.doc;
                            let table = doc.variables();
                            let name = table.get(var).map(|v| v.name.clone()).unwrap_or_default();
                            let mode = doc.mode_at(id).and_then(|m| table.mode(m));
                            let tip =
                                super::binding::chip_tip(&name, doc.resolve_var(id, var), mode);
                            resp.on_hover_text(format!("{tip}\nClick to toggle and detach"))
                        }
                        None => icon_button(ui, eye, 22.0, 16.0, false, true)
                            .on_hover_text("Toggle visibility"),
                    };
                    if resp.clicked() {
                        // Detached in the edit itself, as the tooltip promises.
                        let mut ops = ondin_core::build::detach_visible_ops(&self.session.doc, id);
                        ops.push(Operation::SetVisible {
                            id,
                            visible: !visible0,
                        });
                        self.commit_edit(Transaction(ops));
                    }
                    if resp.secondary_clicked()
                        && let Some(at) = resp.interact_pointer_pos()
                    {
                        self.field_menu = Some((key, at, resp.rect));
                    }
                });
            });
            // A frame's *Mode*, under its name.
            self.frame_mode_row(ui, id);
            self.identity_boolean_row(ui);
            // What the layer is inside a component, in the same card.
            self.inspector_component(ui, super::component::ComponentPart::Identity);
        });
    }
    /// The identity card's second row: the boolean
    /// operations dropdown, then Mask, Group and Ungroup as 28×28 actions.
    ///
    /// Disabled rather than hidden when it does not apply. A selected boolean
    /// switches its op (`build::set_boolean_op`); two or more layers get wrapped
    /// (`build::boolean`). Flatten sits under a rule: it is an action, not a state.
    fn identity_boolean_row(&mut self, ui: &mut egui::Ui) {
        let existing = self.selected_boolean();
        let refusal = self.booleanable_selection().err();
        let can_make = existing.is_none() && refusal.is_none();
        let shown = existing.map(|(_, op)| op).unwrap_or_default();
        let enabled = existing.is_some() || can_make;

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = BOOL_ROW_GAP;
            ui.spacing_mut().interact_size.y = BOOL_ROW_H;
            ui.spacing_mut().button_padding.y = 0.0;
            let mut pick = None;
            let mut flatten = false;
            ui::disable_unless(ui, enabled, |ui| {
                egui::ComboBox::from_id_salt("boolean-op")
                    .icon(ui::combo_chevron)
                    .width(BOOL_COMBO_W)
                    // egui's default `combo_height` made this fixed list scroll.
                    .height(BOOL_MENU_H)
                    .selected_text(ui::glyph_only(op_glyph(shown)))
                    .show_ui(ui, |ui| {
                        ui::menu_rows(ui);
                        for op in ondin_core::BoolOp::ALL {
                            let row = ui::glyph_and_text(op_glyph(op), op.label());
                            if ui
                                .selectable_label(op == shown && existing.is_some(), row)
                                .clicked()
                            {
                                pick = Some(op);
                            }
                        }
                        popup_rule(ui);
                        let row = ui::glyph_and_text(icon::ARROWS_MERGE, "Flatten");
                        flatten = ui
                            .selectable_label(false, row)
                            .on_hover_text(
                                "Replace the selection with one path. \
                                 The layers that made it are discarded.",
                            )
                            .clicked();
                    })
                    .response
                    // The builder's own reason.
                    .on_disabled_hover_text(
                        refusal.unwrap_or("Boolean operations need two or more layers selected"),
                    );
            });
            if let Some(op) = pick {
                self.apply_boolean(op);
            }
            if flatten {
                self.flatten_selection();
            }

            // Mask, Group, Ungroup. Mask is a state, lit while the selection is
            // masked, and clicking it again releases.
            let mask = self.mask_action();
            let masked = matches!(mask, Ok(MaskAction::Release(_)));
            // Two layers or more get wrapped, so the tooltip says so.
            let wraps = matches!(&mask, Ok(MaskAction::Make { members, .. }) if members.len() > 1);
            if ui::disable_unless(ui, mask.is_ok(), |ui| {
                ui::icon_button(
                    ui,
                    icon::CIRCLE_HALF,
                    BOOL_ROW_H,
                    ACTION_GLYPH,
                    masked,
                    mask.is_ok(),
                )
                .on_hover_text(if masked {
                    "Release mask — the layer draws again  (Ctrl+Alt+M)"
                } else if wraps {
                    "Use as mask — groups them, and the lowest clips the rest  (Ctrl+Alt+M)"
                } else {
                    "Use as mask — the layer clips the siblings above it  (Ctrl+Alt+M)"
                })
                .on_disabled_hover_text(mask.err().unwrap_or_default())
            })
            .inner
            .clicked()
            {
                self.toggle_mask();
            }

            let (can_group, can_ungroup) = self.group_actions();
            if ui::disable_unless(ui, can_group, |ui| {
                ui::icon_button(
                    ui,
                    icon::SELECTION_PLUS,
                    BOOL_ROW_H,
                    ACTION_GLYPH,
                    false,
                    can_group,
                )
                .on_hover_text("Group selection")
                .on_disabled_hover_text("Already a group — nothing to wrap it in")
            })
            .inner
            .clicked()
            {
                self.group_selection();
            }
            if ui::disable_unless(ui, can_ungroup, |ui| {
                ui::icon_button(
                    ui,
                    icon::SELECTION_SLASH,
                    BOOL_ROW_H,
                    ACTION_GLYPH,
                    false,
                    can_ungroup,
                )
                .on_hover_text("Ungroup")
                .on_disabled_hover_text("Nothing in the selection is a group")
            })
            .inner
            .clicked()
            {
                self.ungroup_selection();
            }
        });
    }

    /// Why *Use as mask* would refuse the current selection — [`Self::mask_action`]'s
    /// sentence, for the context menu's row.
    pub(crate) fn mask_refusal(&self) -> Option<&'static str> {
        self.mask_action().err()
    }

    /// What *Use as mask* would do to the current selection, or the sentence
    /// saying why the control is dim.
    ///
    /// Release is read on the layer and on the group the make-arm wraps it in, so
    /// the make is undoable from the selection it leaves. Only refusals with
    /// something specific to say are checked here; `build::mask` has the rest.
    fn mask_action(&self) -> Result<MaskAction, &'static str> {
        const PICK: &str = "Select a layer to use as a mask";
        let doc = &self.session.doc;
        let ids = self.session.selection.ids();
        let (first, rest) = ids.split_first().ok_or(PICK)?;

        if rest.is_empty() {
            // The Mask card's own query, so the button and the card agree.
            let targets = mask_card_targets(doc, *first);
            if !targets.is_empty() {
                return Ok(MaskAction::Release(targets));
            }
            doc.get(*first).ok_or(PICK)?;
        }

        let parent = doc.get(*first).and_then(|n| n.parent()).ok_or(PICK)?;
        if rest
            .iter()
            .any(|id| doc.get(*id).and_then(|n| n.parent()) != Some(parent))
        {
            return Err("A mask and what it masks have to be in the same group");
        }
        // Inside a boolean the flag would take and do nothing: its children are
        // operands, not artwork.
        if matches!(
            doc.get(parent).map(|n| n.kind()),
            Some(NodeKind::Boolean { .. })
        ) {
            return Err(
                "A boolean's operands are combined rather than drawn, so nothing there can mask",
            );
        }
        // `build::mask` asks the same function, so the control and the verb agree.
        let key = self.session.selection.key().filter(|k| ids.contains(k));
        let target = build::mask_target(doc, ids, key).ok_or(PICK)?;
        if !doc.get(target).ok_or(PICK)?.kind().can_mask() {
            return Err("A frame already clips its own contents and cannot mask its siblings");
        }
        // `mask_target` passes over a frame-holding group when another member can
        // mask, so reaching here means none can.
        if doc.holds_a_frame(target) {
            return Err("A group holding a frame cannot be a mask; use a shape above it instead");
        }
        // A dry run, so the control is never live over a click that would fail.
        let mut probe = ondin_core::IdSource::new(0);
        if build::mask(doc, &self.session.resolved, &mut probe, ids, key).is_err() {
            return Err("These layers cannot be wrapped in a group, so they cannot be masked");
        }
        Ok(MaskAction::Make {
            members: ids.to_vec(),
            key,
        })
    }

    /// Run whatever [`Self::mask_action`] says the click means.
    ///
    /// Making selects the new group; releasing leaves the selection alone.
    pub(crate) fn toggle_mask(&mut self) {
        match self.mask_action() {
            Ok(MaskAction::Release(ids)) => {
                // One transaction over every target: one undo step.
                let n = ids.len();
                let tx = Transaction(
                    ids.into_iter()
                        .map(|id| Operation::SetMask { id, mask: false })
                        .collect(),
                );
                if self.commit_edit(tx) {
                    self.session.info(match n {
                        1 => "Mask released".to_string(),
                        n => format!("{n} masks released"),
                    });
                }
            }
            Ok(MaskAction::Make { members, key }) => {
                let grouped = members.len() > 1;
                match build::mask(
                    &self.session.doc,
                    &self.session.resolved,
                    &mut self.session.ids,
                    &members,
                    key,
                ) {
                    Ok((tx, made)) => {
                        if self.commit_edit(tx) {
                            self.session.selection.set_one(made);
                            self.session.info(if grouped {
                                "Masked into a group"
                            } else {
                                "Masked"
                            });
                        }
                    }
                    Err(e) => self.session.fail(format!("Cannot mask: {e}")),
                }
            }
            // Spoken, because only the chord gets here and silence would look broken.
            Err(why) => self.session.info(why),
        }
    }

    /// The **Mask** card: how the selection masks.
    ///
    /// Present only when there is a mask to describe ([`Self::mask_card_targets`]),
    /// including on the mask group the operation leaves selected.
    fn inspector_mask(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let targets = self.mask_card_targets(id);
        if targets.is_empty() {
            return;
        }
        // Snapshotted before the card is drawn, since `panel` takes `&mut self`.
        let modes: Vec<(NodeId, ondin_core::MaskMode)> = targets
            .iter()
            .filter_map(|t| self.session.doc.get(*t).map(|n| (*t, n.mask_mode())))
            .collect();
        // The mode they agree on, or `None` for "Mixed".
        let shared = modes
            .first()
            .map(|(_, m)| *m)
            .filter(|m| modes.iter().all(|(_, other)| other == m));
        // The mark reads the card's subjects (the masks), not the selection.
        let mark = self.sub_mark(
            &targets,
            |op| match op {
                Operation::SetMaskMode { mode, .. } => Some(*mode),
                _ => None,
            },
            |n| Some(n.mask_mode()),
            |m| *m,
            |c, m| *c = m,
            |id, mode| Operation::SetMaskMode { id, mode },
            |m| m.label().to_owned(),
        );

        let mut pick = None;
        let mut reset = false;
        self.panel(ui, "Mask", None, |_app, ui| {
            ui.spacing_mut().interact_size.y = 28.0;
            ui.spacing_mut().button_padding.y = 0.0;
            let label = match shared {
                Some(m) => m.label(),
                None => "Mixed",
            };
            let face = egui::ComboBox::from_id_salt("mask-mode")
                .icon(ui::combo_chevron)
                .width(ui.available_width())
                .selected_text(ui::glyph_and_text(icon::CIRCLE_HALF, label))
                .show_ui(ui, |ui| {
                    ui::menu_rows(ui);
                    ui.spacing_mut().button_padding.y = 2.0;
                    if let Some(m) = &mark {
                        reset = super::component::menu_reset_row(ui, m);
                    }
                    // "Mixed" is a report, not a value, so it is not an option.
                    for option in ondin_core::MaskMode::ALL {
                        if ui
                            .selectable_label(shared == Some(option), option.label())
                            .on_hover_text(option.describe())
                            .clicked()
                        {
                            pick = Some(option);
                        }
                    }
                })
                .response;
            if mark.is_some() {
                super::component::combo_dot(ui.painter(), face.rect);
            }
        });
        self.reset_marked(reset, &mark);
        if let Some(mode) = pick {
            // Only the targets that would change, so a no-op pick is no undo step.
            let ops: Vec<Operation> = modes
                .iter()
                .filter(|(_, was)| *was != mode)
                .map(|(t, _)| Operation::SetMaskMode { id: *t, mode })
                .collect();
            if !ops.is_empty() {
                self.commit_edit(Transaction(ops));
            }
        }
    }

    /// The layers the **Mask** card describes, given what is selected.
    ///
    /// The selected layer if it is a mask, else the masks directly inside it —
    /// direct children only, so an ancestor never rewrites a mask further down. A
    /// list because a group can hold two runs, which report "Mixed".
    fn mask_card_targets(&self, id: NodeId) -> Vec<NodeId> {
        mask_card_targets(&self.session.doc, id)
    }

    /// Whether Group and Ungroup apply to what is selected.
    ///
    /// Ungroup is live when anything selected is a container, which is what
    /// `OndinApp::ungroup_selection` acts on. Group is the builder's dry run, minus
    /// a selection that is exactly one container (a group of one group).
    fn group_actions(&self) -> (bool, bool) {
        let ids = self.session.selection.ids();
        let is_container = |id: &NodeId| {
            matches!(
                self.session.doc.get(*id).map(|n| n.kind()),
                Some(NodeKind::Group | NodeKind::Boolean { .. })
            )
        };
        let can_ungroup = ids.iter().any(is_container);
        let lone_container = ids.len() == 1 && can_ungroup;
        let mut probe = ondin_core::IdSource::new(0);
        let can_group = !lone_container
            && build::group(&self.session.doc, &self.session.resolved, &mut probe, ids).is_ok();
        (can_group, can_ungroup)
    }

    /// Collapse the selection into a single `Path` (`build::flatten`).
    ///
    /// The result is selected. The refusal is the dropdown's, since `Ctrl+E`
    /// bypasses its disabled state; only the count refusal is worded for this verb.
    pub(crate) fn flatten_selection(&mut self) {
        if self.selected_boolean().is_none()
            && let Err(why) = self.booleanable_selection()
        {
            let few = build::outermost(&self.session.doc, self.session.selection.ids()).len() < 2;
            self.session.info(if few {
                "Select a boolean, or two or more layers, to flatten"
            } else {
                why
            });
            return;
        }
        let ids = self.session.selection.ids().to_vec();
        match build::flatten(
            &self.session.doc,
            &self.session.resolved,
            &mut self.session.ids,
            &ids,
        ) {
            Ok((tx, made)) => {
                if self.commit_edit(tx) {
                    self.session.selection.set_one(made);
                    self.session.info("Flattened");
                }
            }
            Err(e) => self.session.fail(format!("Cannot flatten: {e}")),
        }
    }

    /// *Outline shape*: `id` becomes the `Path` its outline traces. The result is
    /// selected, ready for `Enter` into its points.
    pub(crate) fn outline_shape(&mut self, id: NodeId) {
        match build::outline(
            &self.session.doc,
            &self.session.resolved,
            &mut self.session.ids,
            id,
        ) {
            Ok((tx, made)) => {
                if self.commit_edit(tx) {
                    self.session.selection.set_one(made);
                    self.session.info("Outlined");
                }
            }
            Err(e) => self.session.fail(format!("Cannot outline: {e}")),
        }
    }

    /// *Outline stroke*: `id`'s strokes become filled paths (`build::outline_stroke`);
    /// what it made is selected.
    pub(crate) fn outline_stroke(&mut self, id: NodeId) {
        match build::outline_stroke(
            &self.session.doc,
            &self.session.resolved,
            &mut self.session.ids,
            id,
        ) {
            Ok((tx, made)) => {
                if self.commit_edit(tx) {
                    self.session.selection.set_one(made);
                    self.session.info("Outlined the stroke");
                }
            }
            Err(e) => self.session.fail(format!("Cannot outline the stroke: {e}")),
        }
    }

    /// *Convert to path*: a text layer becomes the `Path` its glyphs trace
    /// (`build::outline_text`, `context-menus.md` §5.6).
    ///
    /// The status line says the conversion is one-way and names undo.
    pub(crate) fn outline_text(&mut self, id: NodeId) {
        match build::outline_text(
            &self.session.doc,
            &self.session.resolved,
            &mut self.session.ids,
            id,
        ) {
            Ok((tx, made)) => {
                if self.commit_edit(tx) {
                    self.session.selection.set_one(made);
                    self.session
                        .info("Converted to a path — the text is gone; Ctrl+Z to undo");
                }
            }
            Err(e) => self.session.fail(format!("Cannot convert: {e}")),
        }
    }

    /// *Detach from path* — take the selected text off its rail, which comes back
    /// as a path layer.
    ///
    /// The new path takes the selection, since it is drawn with no paint.
    pub(crate) fn detach_text_path(&mut self) {
        let Some(id) = self.session.selection.single() else {
            return;
        };
        match build::detach_text_path(&self.session.doc, &mut self.session.ids, id) {
            Ok((tx, made)) => {
                if self.commit_edit(tx) {
                    self.session.selection.set_one(made);
                    self.session
                        .info("Detached — the path is back as a layer of its own");
                }
            }
            Err(e) => self.session.fail(format!("Cannot detach: {e}")),
        }
    }

    /// *Flip to other side* — run the type the other way along its rail. Reads
    /// the current value, so it is a true toggle; no status line.
    pub(crate) fn flip_text_path(&mut self) {
        let Some(id) = self.session.selection.single() else {
            return;
        };
        let Some(NodeKind::Text {
            on_path: Some(_),
            on_path_flip,
            ..
        }) = self.session.display_node(id).map(|n| n.kind())
        else {
            return;
        };
        let flipped = !*on_path_flip;
        self.commit_edit(Transaction(vec![Operation::SetGeometry {
            id,
            geometry: GeometryPatch::TextPathFlip(flipped),
        }]));
    }

    /// The selected layer when it is a boolean container, with its operation.
    fn selected_boolean(&self) -> Option<(NodeId, ondin_core::BoolOp)> {
        let id = self.session.selection.single()?;
        match self.session.display_node(id)?.kind() {
            NodeKind::Boolean { op } => Some((id, *op)),
            _ => None,
        }
    }

    /// The selection, when `build::boolean` would accept it — or why not, one
    /// sentence per refusal, worded like [`Self::mask_action`]'s.
    fn booleanable_selection(&self) -> Result<Vec<NodeId>, &'static str> {
        const FEW: &str = "Boolean operations need two or more layers selected";
        let ids = build::outermost(&self.session.doc, self.session.selection.ids());
        if ids.len() < 2 {
            return Err(FEW);
        }
        // Dry run on a throwaway id source.
        let mut probe = ondin_core::IdSource::new(0);
        build::boolean(
            &self.session.doc,
            &self.session.resolved,
            &mut probe,
            &ids,
            ondin_core::BoolOp::Union,
            // The key changes the result's order, never whether there is one.
            None,
        )
        .map(|_| ids)
        .map_err(|e| match e {
            ondin_core::OpError::WrongKindForOp => "Text and artboards cannot be boolean operands",
            ondin_core::OpError::InvalidParent => {
                "A boolean and its operands have to be in the same group"
            }
            ondin_core::OpError::ArtboardPlacement => {
                "A group holding a frame cannot be a boolean operand"
            }
            _ => FEW,
        })
    }

    /// Switch the selected boolean's operation, or wrap the selection in a new one.
    pub(crate) fn apply_boolean(&mut self, op: ondin_core::BoolOp) {
        if let Some((id, _)) = self.selected_boolean() {
            match build::set_boolean_op(&self.session.doc, id, op) {
                Ok(tx) if tx.0.is_empty() => {}
                Ok(tx) => {
                    self.commit_edit(tx);
                }
                Err(e) => self.session.fail(format!("Boolean failed: {e}")),
            }
            return;
        }
        // Reported, not dropped: only the keyboard reaches here when this fails.
        let ids = match self.booleanable_selection() {
            Ok(ids) => ids,
            Err(why) => {
                self.session.info(why);
                return;
            }
        };
        // The key becomes the base operand, if it survived `outermost`.
        let key = self.session.selection.key().filter(|k| ids.contains(k));
        match build::boolean(
            &self.session.doc,
            &self.session.resolved,
            &mut self.session.ids,
            &ids,
            op,
            key,
        ) {
            Ok((tx, made)) => {
                if self.commit_edit(tx) {
                    self.session.selection.set_one(made);
                }
            }
            Err(e) => self.session.fail(format!("Boolean failed: {e}")),
        }
    }

    /// Align and distribute, in the design's single row: three horizontal
    /// alignments, a rule, three vertical, a rule, then the two distributes.
    ///
    /// [`Self::align_target`] picks the box; a lone top-level layer has none, and
    /// the card is drawn dimmed rather than skipped.
    fn inspector_align(&mut self, ui: &mut egui::Ui) {
        // In point-editing mode the same buttons act on the points.
        let points = self.edited_path().map(|id| self.subject_points(id).len());
        // Naming the gap needs two members where spreading needs three.
        let (can_align, can_distribute, can_space) = match points {
            Some(n) => (n >= 2, n >= 3, n >= 2),
            None => {
                // No target dims the row rather than hiding the card.
                let n = build::outermost(&self.session.doc, self.session.selection.ids()).len();
                // Distribute needs an interior to space out.
                (self.align_target().is_some(), n >= 3, n >= 2)
            }
        };
        let on_points = points.is_some();
        self.panel(ui, "Align", None, |app, ui| {
            ui.horizontal(|ui| {
                // Spread across the row, as the design's `space-between`; 3px floor.
                let ink = 8.0 * ALIGN_CELL.x + 2.0;
                ui.spacing_mut().item_spacing.x = ((ui.available_width() - ink) / 9.0).max(3.0);
                for cell in [
                    AlignCell::To(icon::ALIGN_LEFT, Axis::X, Edge::Min, "Align left"),
                    AlignCell::To(
                        icon::ALIGN_CENTER_HORIZONTAL,
                        Axis::X,
                        Edge::Mid,
                        "Align centres",
                    ),
                    AlignCell::To(icon::ALIGN_RIGHT, Axis::X, Edge::Max, "Align right"),
                    AlignCell::Rule,
                    AlignCell::To(icon::ALIGN_TOP, Axis::Y, Edge::Min, "Align top"),
                    AlignCell::To(
                        icon::ALIGN_CENTER_VERTICAL,
                        Axis::Y,
                        Edge::Mid,
                        "Align middles",
                    ),
                    AlignCell::To(icon::ALIGN_BOTTOM, Axis::Y, Edge::Max, "Align bottom"),
                    AlignCell::Rule,
                    AlignCell::Spread(
                        icon::COLUMNS,
                        Axis::X,
                        if on_points {
                            "Distribute horizontally — needs three points"
                        } else {
                            "Distribute horizontal spacing — needs three layers"
                        },
                    ),
                    AlignCell::Spread(
                        icon::ROWS,
                        Axis::Y,
                        if on_points {
                            "Distribute vertically — needs three points"
                        } else {
                            "Distribute vertical spacing — needs three layers"
                        },
                    ),
                ] {
                    match cell {
                        AlignCell::Rule => ui::rule(ui, ALIGN_CELL.y),
                        AlignCell::To(glyph, axis, at, tip) => {
                            if ui::icon_button_sized(ui, glyph, ALIGN_CELL, 15.0, false, can_align)
                                .on_hover_text(match (can_align, on_points) {
                                    (true, _) => tip,
                                    (false, true) => "Select two or more points",
                                    (false, false) => {
                                        "Nothing to align to — this comes down to one \
                                         top-level layer, with no container and \
                                         nothing beside it. Select another layer, or \
                                         put this one in a frame"
                                    }
                                })
                                .clicked()
                                && can_align
                            {
                                if on_points {
                                    app.align_points(axis, at);
                                } else {
                                    app.align(axis, at);
                                }
                            }
                        }
                        AlignCell::Spread(glyph, axis, tip) => {
                            if ui::icon_button_sized(
                                ui,
                                glyph,
                                ALIGN_CELL,
                                15.0,
                                false,
                                can_distribute,
                            )
                            .on_hover_text(tip)
                            .clicked()
                                && can_distribute
                            {
                                if on_points {
                                    app.distribute_points(axis);
                                } else {
                                    app.distribute(axis);
                                }
                            }
                        }
                    }
                }
            });
            app.spacing_row(ui, on_points, can_space);
        });
    }

    /// The Align card's second row: the gap between neighbours, **named** rather
    /// than equalised (`build::distribute_spacing`). With the node tool up it
    /// spaces the selected points (`tools::space_points`).
    fn spacing_row(&mut self, ui: &mut egui::Ui, on_points: bool, enabled: bool) {
        let fw = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
        let size = egui::vec2(fw, 28.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
            for (axis, glyph, live, dimmed) in [
                (
                    Axis::X,
                    icon::COLUMNS,
                    if on_points {
                        "Gap between neighbouring points, left to right"
                    } else {
                        "Gap between neighbours, left to right — held at the key layer, else at the leftmost"
                    },
                    if on_points {
                        "Select two or more points"
                    } else {
                        "Select two or more layers"
                    },
                ),
                (
                    Axis::Y,
                    icon::ROWS,
                    if on_points {
                        "Gap between neighbouring points, top to bottom"
                    } else {
                        "Gap between neighbours, top to bottom — held at the key layer, else at the topmost"
                    },
                    if on_points {
                        "Select two or more points"
                    } else {
                        "Select two or more layers"
                    },
                ),
            ] {
                let gaps = if on_points {
                    self.point_spacings(axis)
                } else {
                    layer_spacings(&self.session, axis)
                };
                let (uniform, mean) = shown_gap(&gaps);
                let mut shown = uniform.unwrap_or(mean);
                let mixed = uniform.is_none();
                let scoped = ui.scope(|ui| {
                    if !enabled {
                        ui.disable();
                    }
                    value_field(ui, size, Prefix::Icon(glyph), &mut shown, Scrub::whole(0.5), move |d| {
                        d.custom_formatter(move |v, _| {
                            if mixed {
                                "Mixed".to_string()
                            } else {
                                crate::ui::number(2)(v, 0..=0)
                            }
                        })
                    })
                });
                // A disabled widget reports no hover, so the dimmed tooltip needs its own claimant.
                if !enabled {
                    ui.interact(
                        scoped.inner.rect,
                        ui.id().with(("dimmed-spacing", matches!(axis, Axis::Y))),
                        egui::Sense::empty(),
                    )
                    .on_hover_text(dimmed);
                    continue;
                }
                let resp = scoped.inner.on_hover_text(live);
                // Building costs an operation per member, so skip it while idle.
                if !interacting(&resp) {
                    continue;
                }
                let tx = if on_points {
                    self.point_spacing_tx(axis, shown)
                } else {
                    self.spacing_tx(axis, shown)
                };
                self.edit_valve(&resp, tx);
            }
        });
    }
    /// Opacity, and corner radius when the selection holds any rect.
    ///
    /// Both are absolute. Opacity reaches only the outermost members, since it
    /// composes down the tree (`build::set_opacity_all`); radius every rect in scope.
    fn inspector_multi_appearance(&mut self, ui: &mut egui::Ui) {
        let ids = self.session.selection.ids().to_vec();
        let opacity = self.shared_opacity_shown();
        let radius = self.shared_radius_shown();
        let has_rect = ondin_core::any_rect_in(&self.session.doc, &ids);
        let (opacity_mark, radius_mark) = self.multi_appearance_marks(&ids, has_rect);
        self.multi_appearance_card(
            ui,
            &ids,
            opacity,
            radius,
            has_rect,
            opacity_mark,
            radius_mark,
        );
    }

    /// The several-layer Appearance card's override marks: opacity over `ids`,
    /// the radius over every rect in their subtree, each its binding's first.
    fn multi_appearance_marks(
        &self,
        ids: &[NodeId],
        has_rect: bool,
    ) -> (Option<Mark>, Option<Mark>) {
        let table = self.session.doc.variables();
        let say_vars = |v: &[Option<ondin_core::VarId>]| match v {
            [first, rest @ ..] if rest.iter().all(|x| x == first) => match first {
                Some(var) => table
                    .get(*var)
                    .map_or_else(|| "a variable".to_owned(), |x| x.name.clone()),
                None => "unbound".to_owned(),
            },
            _ => "per corner".to_owned(),
        };
        // The binding is compared first, since a copy can differ by the binding alone.
        let opacity_mark = Mark::and(
            self.sub_mark(
                ids,
                |op| match op {
                    Operation::SetOpacityVar { var, .. } => Some(*var),
                    _ => None,
                },
                |n| Some(n.opacity_var()),
                |v| *v,
                |c, v| *c = v,
                |id, var| Operation::SetOpacityVar { id, var },
                |v| say_vars(std::slice::from_ref(v)),
            ),
            self.sub_mark(
                ids,
                |op| match op {
                    Operation::SetOpacity { opacity, .. } => Some(*opacity),
                    _ => None,
                },
                |n| Some(n.opacity()),
                |v| *v,
                |c, v| *c = v,
                |id, opacity| Operation::SetOpacity { id, opacity },
                |v| format!("{}%", super::component::mark_num(f64::from(*v) * 100.0)),
            ),
        );
        // Every rect the field writes, compared on all four radii.
        let rects: Vec<NodeId> = match has_rect {
            true => ondin_core::subtree_nodes(&self.session.doc, ids)
                .into_iter()
                .filter(|id| {
                    self.session
                        .doc
                        .get(*id)
                        .is_some_and(|n| matches!(n.kind(), NodeKind::Rect { .. }))
                })
                .collect(),
            false => Vec::new(),
        };
        // The corners' binding first.
        let radius_mark = Mark::and(
            self.sub_mark(
                &rects,
                |op| match op {
                    Operation::SetCornerVars { vars, .. } => Some(*vars),
                    _ => None,
                },
                |n| Some(n.corner_vars()),
                |v| *v,
                |c, v| *c = v,
                |id, vars| Operation::SetCornerVars { id, vars },
                |v| say_vars(&v[..]),
            ),
            self.sub_mark(
                &rects,
                |op| match op {
                    Operation::SetGeometry {
                        geometry: GeometryPatch::CornerRadii(r),
                        ..
                    } => Some(*r),
                    _ => None,
                },
                |n| match n.kind() {
                    NodeKind::Rect { corner_radii, .. } => Some(*corner_radii),
                    _ => None,
                },
                |r| *r,
                |c, v| *c = v,
                |id, r| Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::CornerRadii(r),
                },
                |r| uniform(*r).map_or_else(|| "per corner".to_owned(), super::component::mark_num),
            ),
        );
        (opacity_mark, radius_mark)
    }

    /// The several-layer Appearance card, drawn with its marks.
    #[allow(clippy::too_many_arguments)]
    fn multi_appearance_card(
        &mut self,
        ui: &mut egui::Ui,
        ids: &[NodeId],
        opacity: Option<f32>,
        radius: Option<f64>,
        has_rect: bool,
        opacity_mark: Option<Mark>,
        radius_mark: Option<Mark>,
    ) {
        self.panel(ui, "Appearance", None, |app, ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                // The single-layer panel's arithmetic, so fields do not shift.
                let button_space = if has_rect {
                    28.0 + ui::CARD_COL_GAP
                } else {
                    0.0
                };
                let fields = if has_rect { 2.0 } else { 1.0 };
                let fw = (ui.available_width() - button_space - ui::CARD_COL_GAP * (fields - 1.0))
                    / fields;
                let mixed = opacity.is_none();
                let mut pct = f64::from(opacity.unwrap_or(1.0)) * 100.0;
                let (ro, hit) = ui::value_field_marked(
                    ui,
                    egui::vec2(fw, 28.0),
                    Prefix::Icon(icon::DROP_HALF),
                    opacity_mark.as_ref().map(Mark::field),
                    &mut pct,
                    Scrub::whole(0.5).range(0.0..=100.0),
                    |d| {
                        d.custom_formatter(move |v, _| {
                            if mixed {
                                "Mixed".to_string()
                            } else {
                                format!("{v:.0}%")
                            }
                        })
                    },
                );
                app.reset_marked(hit, &opacity_mark);
                if interacting(&ro) {
                    let doc = &app.session.doc;
                    let to = (pct / 100.0).clamp(0.0, 1.0) as f32;
                    let set = ondin_core::set_opacity_all(doc, ids, to);
                    // Each bound layer the write changes is detached in the edit itself.
                    let mut ops: Vec<Operation> = set
                        .0
                        .iter()
                        .filter_map(|op| match op {
                            Operation::SetOpacity { id, .. } => Some(*id),
                            _ => None,
                        })
                        .flat_map(|id| ondin_core::detach_opacity_ops(doc, id))
                        .collect();
                    ops.extend(set.0);
                    app.edit_valve(&ro, Transaction(ops));
                }
                if !has_rect {
                    return;
                }
                let mixed = radius.is_none();
                let mut r = radius.unwrap_or(0.0);
                let (rr, hit) = ui::value_field_marked(
                    ui,
                    egui::vec2(fw, 28.0),
                    Prefix::Icon(icon::CORNERS_OUT),
                    radius_mark.as_ref().map(Mark::field),
                    &mut r,
                    Scrub::whole(0.5).range(0.0..=f64::MAX),
                    |d| {
                        d.custom_formatter(move |v, _| {
                            if mixed {
                                "Mixed".to_string()
                            } else {
                                // Two decimals: half-unit radii are common.
                                crate::ui::number(2)(v, 0..=0)
                            }
                        })
                    },
                );
                // A marked field's tip names the main's radius instead.
                let rr = tip_unless_marked(
                    rr,
                    &radius_mark,
                    "Corner radius of every rectangle in the selection",
                );
                app.reset_marked(hit, &radius_mark);
                if interacting(&rr) {
                    let tx = ondin_core::set_corner_radius_all(&app.session.doc, ids, r.max(0.0));
                    app.edit_valve(&rr, tx);
                }
                // Present and inert: per-corner rows are keyed by node, and a
                // vanishing button would move the fields.
                ui::field_button(
                    ui,
                    icon::FRAME_CORNERS,
                    28.0,
                    15.0,
                    ui::FieldButton::Disabled,
                )
                .on_hover_text("One rectangle at a time: each corner is set on its own layer");
            });
        });
    }
    /// **Group Colors** — every distinct colour inside the selection, listed as
    /// its own editable row.
    ///
    /// Editing a row recolours every use of that colour in scope, fills and
    /// strokes, in one transaction. Serves a multi-selection and a single
    /// container; the scope excludes a selected container itself
    /// ([`Self::group_color_scope`]). Gradients are listed under the solids, keyed
    /// by place (`PaintSlot::GroupGradient`) and edited through `build::repaint`.
    fn inspector_group_colors(&mut self, ui: &mut egui::Ui) {
        let ids = self.group_color_scope();
        let colors = ondin_core::colors_in(&self.session.doc, &ids);
        let gradients = ondin_core::gradients_in(&self.session.doc, &ids);
        if colors.is_empty() && gradients.is_empty() {
            return;
        }
        let root = self.session.page;
        let mut select: Option<Vec<NodeId>> = None;
        self.panel(ui, "Group Colors", None, |app, ui| {
            for use_ in &colors {
                let key = use_.color.to_rgba8().to_u8_array();
                let brush = Brush::Solid(use_.color);
                // A census entry: no grip or eye, a count and a select button instead.
                let out = app.paint_row(
                    ui,
                    RowCtx {
                        id: root,
                        slot: PaintSlot::GroupColor(key),
                        visible: true,
                        grip: None,
                        reorderable: false,
                        census: Some(use_.nodes.len()),
                        mixed: None,
                        item: None,
                    },
                    &brush,
                );
                if out.select {
                    select = Some(use_.nodes.clone());
                }
            }
            // Under the solids, not interleaved: each list is sorted on its own.
            for use_ in &gradients {
                let out = app.paint_row(
                    ui,
                    RowCtx {
                        id: root,
                        slot: PaintSlot::GroupGradient(use_.at),
                        visible: true,
                        grip: None,
                        reorderable: false,
                        census: Some(use_.nodes.len()),
                        mixed: None,
                        item: None,
                    },
                    &use_.brush,
                );
                if out.select {
                    select = Some(use_.nodes.clone());
                }
            }
        });
        if let Some(ids) = select {
            self.select_by_color(ids);
        }
    }
    /// Replace the selection with every layer using one colour.
    ///
    /// Resets `entered_group`, since the new members can be anywhere in the tree.
    /// Locked layers are counted and not selected, and the message says so; when
    /// every match is locked nothing moves but the message.
    fn select_by_color(&mut self, ids: Vec<NodeId>) {
        if ids.is_empty() {
            return;
        }
        let counted = ids.len();
        let taken: Vec<NodeId> = ids
            .into_iter()
            .filter(|id| self.may_select_in_bulk(*id))
            .collect();
        let locked = counted - taken.len();
        if taken.is_empty() {
            self.session.info(match counted {
                1 => "The 1 layer with this colour is locked".to_string(),
                n => format!("All {n} layers with this colour are locked"),
            });
            return;
        }
        let n = taken.len();
        self.entered_group = None;
        self.session.selection.set(taken);
        self.session.info(match (n, locked) {
            (1, 0) => "Selected 1 layer with this colour".to_string(),
            (n, 0) => format!("Selected {n} layers with this colour"),
            (n, l) => format!("Selected {n} of {counted} layers with this colour — {l} locked"),
        });
    }
    /// The opacity every outermost member shares, read through any live edit,
    /// within `build::OPACITY_AGREEMENT`.
    fn shared_opacity_shown(&self) -> Option<f32> {
        let mut shared: Option<f32> = None;
        for id in build::outermost(&self.session.doc, self.session.selection.ids()) {
            let o = self.session.display_node(id)?.opacity();
            match shared {
                None => shared = Some(o),
                Some(s) if (s - o).abs() < build::OPACITY_AGREEMENT => {}
                Some(_) => return None,
            }
        }
        shared
    }
    /// The uniform corner radius every rect in scope shares, read through any live
    /// edit, within `build::RADIUS_AGREEMENT`. `None` when they differ, one is
    /// unevenly rounded, or there is no rect.
    fn shared_radius_shown(&self) -> Option<f64> {
        let mut shared: Option<f64> = None;
        for id in ondin_core::subtree_nodes(&self.session.doc, self.session.selection.ids()) {
            let Some(node) = self.session.display_node(id) else {
                continue;
            };
            let NodeKind::Rect { corner_radii, .. } = node.kind() else {
                continue;
            };
            // As drawn, a bound corner's 0 literal aside.
            let drawn = self
                .session
                .doc
                .resolve_corners(id, *corner_radii)
                .unwrap_or(*corner_radii);
            let r = uniform(drawn)?;
            match shared {
                None => shared = Some(r),
                Some(s) if (s - r).abs() < build::RADIUS_AGREEMENT => {}
                Some(_) => return None,
            }
        }
        shared
    }
    /// One row of a Transform card, identified by name rather than position.
    ///
    /// The optional Scale row would otherwise renumber every row beneath it, and a
    /// `DragValue`'s half-typed text would pass to the wrong field. `UiBuilder::id`,
    /// not `id_salt`, as in `carried_unit`.
    fn transform_row<R>(
        ui: &mut egui::Ui,
        name: &'static str,
        body: impl FnOnce(&mut egui::Ui) -> R,
    ) -> R {
        let id = ui.id().with(name);
        // Wraps a `horizontal`: a bare `scope_builder` inherits the parent's whole `max_rect`.
        ui.scope_builder(egui::UiBuilder::new().id(id), |ui| {
            ui.horizontal(body).inner
        })
        .inner
    }

    /// Whether a canvas handle drag is scaling the selection right now — the one
    /// state in which the Scale field is a readout rather than a control.
    fn canvas_scaling(&self) -> bool {
        self.tool == crate::tools::Tool::Scale
            && matches!(self.drag, crate::preview::Drag::Resize { .. })
    }

    /// The outermost members of the selection and the world box they share — the
    /// reduction `align_target` makes, and the box `scale_row` and multi W/H resize.
    pub(crate) fn selection_union(&self) -> Option<(Vec<NodeId>, ondin_core::kurbo::Rect)> {
        let roots = build::outermost(&self.session.doc, self.session.selection.ids());
        let union = roots
            .iter()
            .filter_map(|id| self.session.resolved.world_bounds(*id))
            .reduce(|a, b| a.union(b))?;
        Some((roots, union))
    }

    /// `pct` per cent of the selection's union box, photographically, holding the
    /// box's top-left.
    ///
    /// A method rather than the row's body so a test can reach what the field
    /// decides: which box, which anchor, which verb.
    pub(crate) fn scale_tx(&self, pct: f64) -> Option<Transaction> {
        let (roots, union) = self.selection_union()?;
        let s = pct / 100.0;
        Some(tools::resize_selection(
            &self.session.doc,
            &self.session.resolved,
            &roots,
            // The upright union, the box this panel reports.
            tools::SelectionBox::upright(union),
            // Holds the top-left, as the W and H fields do.
            Handle::BottomRight,
            Point::new(
                union.min_x() + union.width() * s,
                union.min_y() + union.height() * s,
            ),
            tools::Resize {
                keep_ratio: false,
                symmetric: false,
                scaling: tools::Scaling::Photographic,
            },
        ))
    }

    /// The Transform card's Scale row: one percentage, photographic (strokes,
    /// radii and type scale too), over the selection's union box, in W's slot.
    ///
    /// Returns the field's `Response` when drawn, for the tests to drive.
    fn scale_row(&mut self, ui: &mut egui::Ui, fw: f32) -> Option<egui::Response> {
        // Only while the Scale tool is held, so it never reads as an ordinary resize.
        if self.tool != crate::tools::Tool::Scale || self.selection_union().is_none() {
            // Forget an uncommitted percentage when the row goes away.
            self.scale_pct = 100.0;
            return None;
        }
        Some(Self::transform_row(ui, "scale", |ui| {
            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
            // Measured at the row's start, before the gaps are charged.
            let cell = (ui.available_width() - fw - ui::CARD_COL_GAP * 2.0) / 2.0;
            let mut pct = self.scale_pct;
            let resp = value_field(
                ui,
                egui::vec2(fw, 28.0),
                Prefix::Icon(icon::RESIZE),
                &mut pct,
                // Whole units: this resets to 100 after every commit.
                Scrub::whole(0.5).range(1.0..=10_000.0),
                |d| d.suffix("%").custom_formatter(ui::number(2)),
            )
            .on_hover_text("Scale the selection — strokes, corner radii and type scale with it");
            self.scale_pct = pct;
            if let Some(tx) = self.scale_tx(pct) {
                self.edit_valve(&resp, tx);
            }
            // Back to 100 once the valve has closed (`pct` applies to the committed
            // box every frame), and not while a canvas handle drag writes the readout.
            if !resp.dragged() && !resp.has_focus() && !self.canvas_scaling() {
                self.scale_pct = 100.0;
            }
            // Halve and double: plain buttons, since a scale is applied and gone.
            for (label, factor) in [("0.5x", 0.5_f64), ("2x", 2.0)] {
                let hit = ui::label_button(ui, label, egui::vec2(cell, 28.0), ui::FieldButton::Off)
                    .on_hover_text(format!(
                        "Scale the selection to {label} its current size — strokes, \
                         corner radii and type with it"
                    ))
                    .clicked();
                if hit && let Some(tx) = self.scale_tx(factor * 100.0) {
                    self.commit_edit(tx);
                }
            }
            resp
        }))
    }

    /// The Transform panel for several layers at once.
    ///
    /// Position and size describe the union box; rotation and skew are a property
    /// editor over the layers, `Mixed` where they disagree. Dragging rotation
    /// nudges each layer, typing flattens them; both turn each layer about its own
    /// origin.
    fn inspector_multi_transform(&mut self, ui: &mut egui::Ui) {
        let Some(union) = self.multi_union() else {
            return;
        };
        // The pivot toggle, present but inert: a selection has no shared pivot.
        let action = ui::HeadAction::toggle(
            icon::CROSSHAIR_SIMPLE,
            false,
            false,
            "One layer at a time: a selection has no shared transform origin",
        );
        // Inert while any member is laid out, or a typed position would move only the free ones.
        let members = ondin_core::build::outermost(&self.session.doc, self.session.selection.ids());
        let laid_out = laid_out_xy(&self.session.doc, &members);
        self.panel(ui, "Transform", Some(action), |app, ui| {
            let fw = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
            let row = egui::vec2(fw, 28.0);
            let xy = ui::disable_unless(ui, laid_out.is_none(), |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                    app.multi_number(
                        ui,
                        row,
                        MultiField::X,
                        Prefix::Text("X"),
                        Some(union.min_x()),
                    );
                    app.multi_number(
                        ui,
                        row,
                        MultiField::Y,
                        Prefix::Text("Y"),
                        Some(union.min_y()),
                    );
                })
            });
            if let Some(why) = laid_out {
                // Beside the disabled scope, which reports no hover of its own.
                ui.interact(
                    xy.response.rect,
                    ui.id().with("multi-xy-laid-out"),
                    egui::Sense::hover(),
                )
                .on_hover_text(why);
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                // The single-layer panel's split: W as wide as X, H the rest.
                let rest = egui::vec2(
                    ui.available_width() - fw - 28.0 - ui::CARD_COL_GAP * 2.0,
                    28.0,
                );
                app.multi_number(
                    ui,
                    row,
                    MultiField::W,
                    Prefix::Text("W"),
                    Some(union.width()),
                );
                app.multi_number(
                    ui,
                    rest,
                    MultiField::H,
                    Prefix::Text("H"),
                    Some(union.height()),
                );
                // A set is locked when every member is.
                let all_locked =
                    tools::proportions_locked(&app.session.doc, app.session.selection.ids());
                let glyph = if all_locked {
                    icon::LINK_SIMPLE
                } else {
                    icon::LINK_SIMPLE_BREAK
                };
                if ui::field_button(ui, glyph, 28.0, 15.0, ui::FieldButton::on_if(all_locked))
                    .on_hover_text(if all_locked {
                        "Unlock proportions on every selected layer"
                    } else {
                        "Lock proportions — W and H scale together when every layer is locked"
                    })
                    .clicked()
                {
                    let tx = lock_all_tx(app.session.selection.ids(), !all_locked);
                    app.commit_edit(tx);
                }
            });
            app.scale_row(ui, fw);
            let rotation = app.shared_angle(MultiField::Rotation);
            let skew = app.shared_angle(MultiField::Skew);
            Self::transform_row(ui, "rot", |ui| {
                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                app.multi_number(
                    ui,
                    row,
                    MultiField::Rotation,
                    Prefix::Icon(icon::ARROW_CLOCKWISE),
                    rotation.map(shown_degrees),
                );
                app.multi_number(
                    ui,
                    row,
                    MultiField::Skew,
                    Prefix::Icon(icon::ANGLE),
                    skew.map(shown_degrees),
                );
            });
            Self::quarters_row(app, ui, TurnScope::Selection);
        });
    }
    /// Turn every outermost selected layer a whole quarter about **its own** origin.
    ///
    /// One transaction, so one undo step.
    pub(crate) fn rotate_selection_quarter(&mut self, angle: f64) {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        let mut ops = Vec::new();
        for id in build::outermost(doc, self.session.selection.ids()) {
            let Some(pivot) = ondin_core::pivot_world(doc, res, id) else {
                continue;
            };
            ops.extend(tools::rotate_node(doc, res, id, pivot, angle).0);
        }
        self.commit_edit(Transaction(ops));
    }
    /// The Transform card's four buttons — two quarter turns, then the two mirrors.
    ///
    /// One row for both cards, scoped by [`TurnScope`].
    ///
    /// Quarter turns compose through `rotate_node` about the committed pivot, not
    /// the angle field, which would turn a mirrored layer the wrong way; so they
    /// work over a mixed rotation too.
    fn quarters_row(app: &mut Self, ui: &mut egui::Ui, scope: TurnScope) {
        Self::transform_row(ui, "quarters", |ui| {
            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
            let quarter = (ui.available_width() - 3.0 * ui::CARD_COL_GAP) / 4.0;
            let bw = egui::vec2(quarter, 28.0);
            let (turn_tips, flip_tips) = match scope {
                TurnScope::Node(_) => (
                    ["Rotate 90° counter-clockwise", "Rotate 90° clockwise"],
                    ["Flip horizontal", "Flip vertical"],
                ),
                TurnScope::Selection => (
                    [
                        "Rotate 90° counter-clockwise — each layer about its own origin",
                        "Rotate 90° clockwise — each layer about its own origin",
                    ],
                    [
                        "Flip horizontal — the selection swaps sides as one arrangement",
                        "Flip vertical — the selection swaps sides as one arrangement",
                    ],
                ),
            };
            for (i, (glyph, turn)) in [
                (icon::ARROW_COUNTER_CLOCKWISE, -std::f64::consts::FRAC_PI_2),
                (icon::ARROW_CLOCKWISE, std::f64::consts::FRAC_PI_2),
            ]
            .into_iter()
            .enumerate()
            {
                if ui::field_button_sized(ui, glyph, bw, 15.0, ui::FieldButton::Off)
                    .on_hover_text(turn_tips[i])
                    .clicked()
                {
                    match scope {
                        TurnScope::Node(id) => app.rotate_quarter(id, turn),
                        TurnScope::Selection => app.rotate_selection_quarter(turn),
                    }
                }
            }
            for (i, (glyph, axis)) in [
                (icon::FLIP_HORIZONTAL, Axis::X),
                (icon::FLIP_VERTICAL, Axis::Y),
            ]
            .into_iter()
            .enumerate()
            {
                if ui::field_button_sized(ui, glyph, bw, 15.0, ui::FieldButton::Off)
                    .on_hover_text(flip_tips[i])
                    .clicked()
                {
                    app.flip(axis);
                }
            }
        });
    }
    /// The world box the selection occupies, from the **committed** tree.
    ///
    /// Committed, not previewed: it is the base a drag measures its total against
    /// (`OndinApp::multi_transform_drag`), or the gesture would run away.
    fn multi_union(&self) -> Option<Rect> {
        self.selection_union().map(|(_, union)| union)
    }
    /// The rotation (or skew) every selected layer shares, in radians, or `None`
    /// when they differ — which is what shows as `Mixed`.
    fn shared_angle(&self, which: MultiField) -> Option<f64> {
        /// Half a tenth of a degree: below what the field can show.
        const EPS: f64 = 0.000_87;
        let mut shared: Option<f64> = None;
        for id in build::outermost(&self.session.doc, self.session.selection.ids()) {
            let world = self.session.resolved.world_transform(id)?;
            let o = build::Basis::of(world).orientation;
            let v = match which {
                MultiField::Skew => o.skew,
                _ => o.angle,
            };
            match shared {
                None => shared = Some(v),
                Some(s) if (s - v).abs() <= EPS => {}
                Some(_) => return None,
            }
        }
        shared
    }
    /// One field of the multi-selection Transform panel, from reading the value
    /// through to committing the edit.
    ///
    /// `base` is `None` only for disagreeing rotation or skew; it still drags,
    /// saying `Mixed`. Returns the field's `Response` for tests.
    fn multi_number(
        &mut self,
        ui: &mut egui::Ui,
        size: egui::Vec2,
        field: MultiField,
        prefix: Prefix,
        base: Option<f64>,
    ) -> egui::Response {
        let angular = matches!(field, MultiField::Rotation | MultiField::Skew);
        // The total dragged so far; zero unless the drag in flight is this field's.
        let live = self
            .multi_transform_drag
            .filter(|(f, _)| *f == field)
            .map_or(0.0, |(_, d)| d);
        let mut shown = base.unwrap_or(0.0) + live;
        let before = shown;
        let mixed = base.is_none();
        let unit = if angular { "°" } else { "" };
        let resp = value_field(ui, size, prefix, &mut shown, Scrub::whole(0.5), move |d| {
            d.custom_formatter(move |v, _| {
                if mixed {
                    "Mixed".to_string()
                } else {
                    format!("{}{unit}", crate::ui::number(2)(v, 0..=0))
                }
            })
            // `expr::eval` drops the degree sign when the text is parsed back.
        });
        let step = shown - before;
        let dragging = resp.dragged();
        let delta = if dragging { live + step } else { live };
        if dragging {
            self.multi_transform_drag = Some((field, delta));
        }
        // Before the early return below, which would strand the total.
        if self.gesture_cancelled {
            self.multi_transform_drag = None;
            return resp;
        }
        // A W or H edit recurses into every descendant, so skip building while idle.
        if !interacting(&resp) {
            return resp;
        }
        // A typed value is absolute, and remembered under the widget's id: the
        // valve's commit frame arrives one frame before the typed number does.
        let typed_key = resp.id.with("multi-typed");
        let pending = resp
            .ctx
            .data(|d| d.get_temp::<Option<f64>>(typed_key))
            .flatten();
        let edit = if dragging || resp.drag_stopped() {
            MultiEdit::Nudge(delta)
        } else if step != 0.0 {
            resp.ctx.data_mut(|d| d.insert_temp(typed_key, Some(shown)));
            MultiEdit::Absolute(shown)
        } else if let Some(v) = pending {
            MultiEdit::Absolute(v)
        } else {
            MultiEdit::Nudge(delta)
        };
        let tx = self.multi_transform_tx(field, edit);
        self.multi_valve(&resp, tx);
        // Only the fields that move the selection keep it on screen.
        if matches!(field, MultiField::X | MultiField::Y) {
            self.reveal_selection();
        }
        resp
    }
    /// Preview→commit for a multi-selection Transform field, clearing the drag
    /// total once the gesture is over.
    ///
    /// Not [`OndinApp::edit_valve`] because the drag total must outlive the
    /// drag's frames but not the drag. Otherwise it is that valve in the same
    /// order: engagement latch (commit on its falling edge), chrome-hide note,
    /// then preview or commit.
    fn multi_valve(&mut self, resp: &egui::Response, tx: Transaction) {
        // Read before anything can bail: a cancelled frame must spend the
        // transition, or the next frame commits what the cancel threw away.
        let engaged = resp.dragged() || resp.has_focus();
        let was_engaged = resp.ctx.data_mut(|d| {
            let id = resp.id.with("multi-valve-engaged");
            let was = d.get_temp::<bool>(id).unwrap_or(false);
            d.insert_temp(id, engaged);
            was
        });
        if self.gesture_cancelled {
            self.multi_transform_drag = None;
            return;
        }
        // Noted every frame, not only on commit, so the chrome hides during the scrub.
        self.note_edit(Some(resp), &tx);
        if engaged {
            self.session.set_preview(&tx);
        } else if was_engaged {
            // Escape abandons the edit; `cancel_gesture` cannot cover this, its gate
            // runs before the panels.
            if !resp.ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.commit_edit(tx);
            }
            // Not `clear_preview`, which would also drop a live text session's content.
            self.session.clear_gesture_preview();
            self.multi_transform_drag = None;
            resp.ctx
                .data_mut(|d| d.insert_temp::<Option<f64>>(resp.id.with("multi-typed"), None));
        }
    }
    /// The transaction one multi-selection Transform edit produces.
    ///
    /// Always built from the committed document, so every frame of a drag,
    /// release included, produces the same transaction for the same total.
    fn multi_transform_tx(&self, field: MultiField, edit: MultiEdit) -> Transaction {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        let roots = build::outermost(doc, self.session.selection.ids());
        let Some(union) = self.multi_union() else {
            return Transaction(Vec::new());
        };
        let to = |now: f64| match edit {
            MultiEdit::Absolute(v) => v,
            MultiEdit::Nudge(d) => now + d,
        };
        // The builders emit one op per member unconditionally.
        let unmoved = |d: f64| d == 0.0;
        match field {
            MultiField::X => {
                let d = to(union.min_x()) - union.min_x();
                if unmoved(d) {
                    return Transaction(Vec::new());
                }
                build::move_by_world(doc, res, &roots, Vec2::new(d, 0.0)).unwrap_or_default()
            }
            MultiField::Y => {
                let d = to(union.min_y()) - union.min_y();
                if unmoved(d) {
                    return Transaction(Vec::new());
                }
                build::move_by_world(doc, res, &roots, Vec2::new(0.0, d)).unwrap_or_default()
            }
            // Scaled from the box's top-left, keeping proportions when every
            // member is locked (`tools::proportions_locked`), as the handles do.
            MultiField::W | MultiField::H => {
                let now = Size::new(union.width(), union.height());
                let drove_x = matches!(field, MultiField::W);
                let asked = if drove_x {
                    Size::new(to(union.width()).max(1.0), union.height())
                } else {
                    Size::new(union.width(), to(union.height()).max(1.0))
                };
                let size = if tools::proportions_locked(doc, self.session.selection.ids()) {
                    tools::paired_size(now, asked, drove_x)
                } else {
                    asked
                };
                // After the lock, which can turn a zero on one axis into a change.
                if size == now {
                    return Transaction(Vec::new());
                }
                tools::resize_selection(
                    doc,
                    res,
                    &roots,
                    // The upright union, the box W and H report for a set.
                    tools::SelectionBox::upright(union),
                    Handle::BottomRight,
                    // Both coordinates off `min`, since the lock may pair the axes.
                    Point::new(union.min_x() + size.width, union.min_y() + size.height),
                    tools::Resize::geometry(false, false),
                )
            }
            MultiField::Rotation | MultiField::Skew => self.multi_angle_tx(&roots, field, edit),
        }
    }
    /// Rotation and skew across the selection, per layer and about each layer's
    /// own origin.
    ///
    /// A nudge adds to each layer's own value, so differences survive; an absolute
    /// lands every layer on the number. The scale is carried through, or a layer
    /// under a sheared ancestor would resize on every nudge.
    fn multi_angle_tx(&self, roots: &[NodeId], field: MultiField, edit: MultiEdit) -> Transaction {
        let mut ops = Vec::new();
        for id in roots {
            let Some(world) = self.session.resolved.world_transform(*id) else {
                continue;
            };
            let [_, _, _, _, e, f] = world.as_coeffs();
            // `exact`, because this rebuilds the transform: a lossy basis would
            // collapse a rank-1 layer. Such a member is skipped.
            let Some(split) = build::Basis::exact(world) else {
                continue;
            };
            let mut o = split.orientation;
            let now = if field == MultiField::Skew {
                o.skew
            } else {
                o.angle
            };
            let next = match edit {
                MultiEdit::Absolute(v) => v.to_radians(),
                MultiEdit::Nudge(d) => now + d.to_radians(),
            };
            if field == MultiField::Skew {
                o.skew = next.clamp(-tools::MAX_SKEW, tools::MAX_SKEW);
            } else {
                o.angle = next;
            }
            let target = Affine::translate(Vec2::new(e, f))
                * o.to_affine()
                * Affine::scale_non_uniform(split.scale.x, split.scale.y);
            // An unedited defocus arrives as a zero nudge.
            for op in self.place_at(*id, target).0 {
                let unchanged = matches!(
                    (&op, self.session.doc.get(*id)),
                    (Operation::SetTransform { transform, .. }, Some(node))
                        if transform.as_coeffs()
                            == self.session.resolved.used_local_of(node).as_coeffs()
                );
                if !unchanged {
                    ops.push(op);
                }
            }
        }
        Transaction(ops)
    }

    /// The Transform card's override marks for `id`: which of X, Y, rotation, W
    /// and H an instance's copy holds differently from its source, each with its
    /// reset and a tooltip naming what the field will then read.
    ///
    /// Compared on stored local values, never the world readings; the tip
    /// re-projects through the copy's parent. An instance root's placement is its
    /// own (`reset::placement_is_own`), so it gets no X, Y or rotation mark.
    fn transform_marks(&self, id: NodeId, world: [f64; 6], box_min: Point) -> TransformMarks {
        let doc = &self.session.doc;
        // Placement and size compare with the slot (`reset::slot_source_of`), so a
        // swapped copy stays in its slot; text sizing and a line's end compare with `content`.
        let (Some(copy), Some(src), Some(content)) = (
            doc.get(id),
            ondin_core::reset::slot_source_of(doc, id).and_then(|s| doc.get(s)),
            ondin_core::reset::source_of(doc, id).and_then(|s| doc.get(s)),
        ) else {
            return TransformMarks::default();
        };
        let num = super::component::mark_num;
        let mut marks = TransformMarks::default();
        let cur = copy.transform().as_coeffs();
        let theirs = src.transform().as_coeffs();
        let parent = Affine::new(world) * copy.transform().inverse();
        // A placement reset carries the main's insets on its axis: for a pinned
        // layer they are the placement, or `keep_insets` would re-pin it where it was.
        let own_place = ondin_core::reset::placement_is_own(doc, id);
        let mine_in = *copy.insets();
        let main_in = if own_place { mine_in } else { *src.insets() };
        let insets = |horizontal: bool, vertical: bool| {
            let mut i = mine_in;
            if horizontal {
                i.left = main_in.left;
                i.right = main_in.right;
                i.margin_auto.left = main_in.margin_auto.left;
                i.margin_auto.right = main_in.margin_auto.right;
            }
            if vertical {
                i.top = main_in.top;
                i.bottom = main_in.bottom;
                i.margin_auto.top = main_in.margin_auto.top;
                i.margin_auto.bottom = main_in.margin_auto.bottom;
            }
            Operation::SetInsets { id, insets: i }
        };
        let set = |c: [f64; 6], horizontal: bool, vertical: bool| {
            Transaction(vec![
                Operation::SetTransform {
                    id,
                    transform: Affine::new(c),
                },
                insets(horizontal, vertical),
            ])
        };
        // On a pinned layer differing insets *are* the axis's placement.
        let pinned_differs = |horizontal: bool| {
            if horizontal {
                (
                    mine_in.left,
                    mine_in.right,
                    mine_in.margin_auto.left,
                    mine_in.margin_auto.right,
                ) != (
                    main_in.left,
                    main_in.right,
                    main_in.margin_auto.left,
                    main_in.margin_auto.right,
                )
            } else {
                (
                    mine_in.top,
                    mine_in.bottom,
                    mine_in.margin_auto.top,
                    mine_in.margin_auto.bottom,
                ) != (
                    main_in.top,
                    main_in.bottom,
                    main_in.margin_auto.top,
                    main_in.margin_auto.bottom,
                )
            }
        };
        if !own_place {
            for (axis, slot) in [(4usize, &mut marks.x), (5, &mut marks.y)] {
                if cur[axis] != theirs[axis] || pinned_differs(axis == 4) {
                    let mut c = cur;
                    c[axis] = theirs[axis];
                    let shown = parent * Affine::new(c) * box_min;
                    let v = if axis == 4 { shown.x } else { shown.y };
                    *slot = Some(Mark {
                        tip: format!("Reset to main · {}", num(v)),
                        tx: set(c, axis == 4, axis == 5),
                    });
                }
            }
            if cur[..4] != theirs[..4] {
                // The main's basis, translation rebuilt so the pivot stays put.
                let pv = self.session.preview_pivot(id).unwrap_or(Point::ZERO);
                let held = Affine::new(cur) * pv;
                let turned =
                    Affine::new([theirs[0], theirs[1], theirs[2], theirs[3], 0.0, 0.0]) * pv;
                let c = [
                    theirs[0],
                    theirs[1],
                    theirs[2],
                    theirs[3],
                    held.x - turned.x,
                    held.y - turned.y,
                ];
                let basis = parent * Affine::new(c);
                let [a, b, cc, d, ..] = basis.as_coeffs();
                let angle = build::Basis::of(Affine::new([a, b, cc, d, 0.0, 0.0]))
                    .orientation
                    .angle;
                // The copy's own insets, stated so `keep_insets` stays off.
                marks.r = Some(Mark {
                    tip: format!("Reset to main · {}°", num(shown_degrees(angle))),
                    tx: set(c, false, false),
                });
            }
        }
        let size_of = |k: &NodeKind| match k {
            NodeKind::Rect { size, .. }
            | NodeKind::Ellipse { size }
            | NodeKind::Polygon { size, .. }
            | NodeKind::Star { size, .. }
            | NodeKind::Artboard { size } => Some(*size),
            _ => None,
        };
        if let (Some(mine), Some(main)) = (size_of(copy.kind()), size_of(src.kind())) {
            // A bound axis's literal is 0, so it can differ by its binding alone.
            let (my_vars, main_vars) = (copy.size_vars(), src.size_vars());
            let table = doc.variables();
            let say = |axis: usize, v: f64| match main_vars[axis] {
                Some(var) => table
                    .get(var)
                    .map_or_else(|| "a variable".to_owned(), |x| x.name.clone()),
                None => num(v),
            };
            // A layer pinned both sides is sized by its insets, so a size carries them.
            let resize = |s: Size, horizontal: bool| {
                let mut vars = my_vars;
                let axis = if horizontal { 0 } else { 1 };
                vars[axis] = main_vars[axis];
                Transaction(vec![
                    Operation::SetGeometry {
                        id,
                        geometry: GeometryPatch::Size(s),
                    },
                    Operation::SetSizeVars { id, vars },
                    insets(horizontal, !horizontal),
                ])
            };
            if mine.width != main.width || my_vars[0] != main_vars[0] {
                marks.w = Some(Mark {
                    tip: format!("Reset to main · {}", say(0, main.width)),
                    tx: resize(Size::new(main.width, mine.height), true),
                });
            }
            if mine.height != main.height || my_vars[1] != main_vars[1] {
                marks.h = Some(Mark {
                    tip: format!("Reset to main · {}", say(1, main.height)),
                    tx: resize(Size::new(mine.width, main.height), false),
                });
            }
        }
        // Marked here only where the modes agree; a differing mode is
        // `typography::sizing_mark`'s.
        if let (NodeKind::Text { sizing: mine_s, .. }, NodeKind::Text { sizing: main_s, .. }) =
            (copy.kind(), content.kind())
        {
            let resize = |s: TextSizing, horizontal: bool| {
                Transaction(vec![
                    Operation::SetGeometry {
                        id,
                        geometry: GeometryPatch::TextSizing(s),
                    },
                    insets(horizontal, !horizontal),
                ])
            };
            let tip = |v: f64| format!("Reset to main · {}", num(v));
            match (*mine_s, *main_s) {
                (TextSizing::AutoHeight(w), TextSizing::AutoHeight(mw)) if w != mw => {
                    marks.w = Some(Mark {
                        tip: tip(mw),
                        tx: resize(TextSizing::AutoHeight(mw), true),
                    });
                }
                (TextSizing::Fixed(s), TextSizing::Fixed(ms)) => {
                    if s.width != ms.width {
                        marks.w = Some(Mark {
                            tip: tip(ms.width),
                            tx: resize(TextSizing::Fixed(Size::new(ms.width, s.height)), true),
                        });
                    }
                    if s.height != ms.height {
                        marks.h = Some(Mark {
                            tip: tip(ms.height),
                            tx: resize(TextSizing::Fixed(Size::new(s.width, ms.height)), false),
                        });
                    }
                }
                _ => {}
            }
        }
        // A line's end is its length (`L`) and its angle (R); each reset writes one
        // of them, so the two together write the main's end.
        if let (NodeKind::Line { end: mine_e }, NodeKind::Line { end: main_e }) =
            (copy.kind(), content.kind())
            && mine_e != main_e
        {
            let (len, main_len) = (mine_e.to_vec2().hypot(), main_e.to_vec2().hypot());
            let reend = |p: Point| {
                Transaction(vec![
                    Operation::SetGeometry {
                        id,
                        geometry: GeometryPatch::LineEnd(p),
                    },
                    insets(false, false),
                ])
            };
            if len != main_len && len > 1e-9 {
                marks.l = Some(Mark {
                    tip: format!("Reset to main · {}", num(main_len)),
                    tx: reend((mine_e.to_vec2() * (main_len / len)).to_point()),
                });
            }
            let (dir, main_dir) = (mine_e.y.atan2(mine_e.x), main_e.y.atan2(main_e.x));
            // Where the basis differs too, R is the basis's mark: one ↺, one write.
            if dir != main_dir && main_len > 1e-9 && marks.r.is_none() {
                let shown = parent * copy.transform() * Point::ZERO;
                let to =
                    parent * copy.transform() * (main_e.to_vec2() * (len / main_len)).to_point();
                marks.r = Some(Mark {
                    tip: format!(
                        "Reset to main · {}°",
                        num(shown_degrees((to.y - shown.y).atan2(to.x - shown.x)))
                    ),
                    tx: reend((main_e.to_vec2() * (len / main_len)).to_point()),
                });
            }
        }
        // The item's `width` and `height`, which a laid-out W and H also stand for.
        for (horizontal, slot) in [(true, &mut marks.w), (false, &mut marks.h)] {
            let item = self.sub_mark(
                &[id],
                super::layout::item_of,
                |n| Some(*n.item()),
                move |i| if horizontal { i.width } else { i.height },
                move |i, d| {
                    if horizontal {
                        i.width = d;
                    } else {
                        i.height = d;
                    }
                },
                |id, item| Operation::SetLayoutItem { id, item },
                super::layout::dimension_words,
            );
            *slot = Mark::and(slot.take(), item);
        }
        marks
    }

    /// `world` is the node's world transform coefficients. X/Y/rotation are
    /// shown and edited in world space, projected back with
    /// `build::local_for_world`.
    pub(super) fn inspector_transform(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        world: [f64; 6],
        size0: Option<Size>,
    ) {
        // The pivot toggle arms a canvas handle; one layer only.
        let single = self.session.selection.ids().len() <= 1;
        let action = ui::HeadAction::toggle(
            icon::CROSSHAIR_SIMPLE,
            self.show_pivot && single,
            single,
            if single {
                "Move the transform origin — the point rotation and mirroring turn about"
            } else {
                "One layer at a time: a selection has no shared transform origin"
            },
        );
        // The box's corner, not the translation: a derived box starts off its local origin.
        let shown = Affine::new(world)
            * self
                .session
                .preview_local_box(id)
                .map(|b| Point::new(b.x0, b.y0))
                .unwrap_or(Point::ZERO);
        let line_ends = match self.session.display_node(id).map(|n| n.kind()) {
            Some(NodeKind::Line { end }) => {
                let (a, b) = (Affine::new(world) * Point::ZERO, Affine::new(world) * *end);
                ((b - a).hypot() > 1e-9).then_some((a, b))
            }
            _ => None,
        };
        let shown_line = line_ends.map(|(a, b)| (b.y - a.y).atan2(b.x - a.x));
        let laid_out = ondin_core::build::is_flex_item(&self.session.doc, id);
        let size_menu = self.session.display_node(id).and_then(|n| {
            let modes = super::layout::size_modes(n.kind(), n.display().is_some(), laid_out);
            let item = n.item();
            (modes.len() > 1).then(|| {
                let pct = |d: ondin_core::container::Dimension| match d {
                    ondin_core::container::Dimension::Percent(v) => Some(v),
                    _ => None,
                };
                SizeMenu {
                    w: super::layout::size_mode_of(n.kind(), item.width),
                    h: super::layout::size_mode_of(n.kind(), item.height),
                    w_pct: pct(item.width),
                    h_pct: pct(item.height),
                    modes,
                }
            })
        });
        let box_min = self
            .session
            .preview_local_box(id)
            .map(|b| Point::new(b.x0, b.y0))
            .unwrap_or(Point::ZERO);
        let marks = self.transform_marks(id, world, box_min);
        let toggled = self.panel(ui, "Transform", Some(action), |app, ui| {
            let fw = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
            // The world basis with translation stripped.
            let basis = [world[0], world[1], world[2], world[3], 0.0, 0.0];
            let row = egui::vec2(fw, 28.0);
            // Inert for an in-flow item, whose container places it.
            let xy = ui::disable_unless(ui, !laid_out, |ui| ui.horizontal(|ui| {
                let mut x = shown.x;
                let (rx, reset_x) = ui::value_field_marked(
                    ui,
                    row,
                    Prefix::Text("X"),
                    marks.x.as_ref().map(Mark::field),
                    &mut x,
                    Scrub::whole(0.5),
                    |d| d.custom_formatter(ui::number(2)),
                );
                if let (true, Some(m)) = (reset_x, &marks.x) {
                    app.commit_reset(m.tx.clone());
                }
                // A world-space nudge, so the corner lands on the typed number whatever the basis.
                let tx = app.place_at(
                    id,
                    Affine::translate(Vec2::new(x - shown.x, 0.0)) * Affine::new(world),
                );
                app.edit_valve(&rx, tx);
                if interacting(&rx) {
                    app.reveal_selection();
                }
                let mut y = shown.y;
                let (ry, reset_y) = ui::value_field_marked(
                    ui,
                    row,
                    Prefix::Text("Y"),
                    marks.y.as_ref().map(Mark::field),
                    &mut y,
                    Scrub::whole(0.5),
                    |d| d.custom_formatter(ui::number(2)),
                );
                if let (true, Some(m)) = (reset_y, &marks.y) {
                    app.commit_reset(m.tx.clone());
                }
                let ty = app.place_at(
                    id,
                    Affine::translate(Vec2::new(0.0, y - shown.y)) * Affine::new(world),
                );
                app.edit_valve(&ry, ty);
                if interacting(&ry) {
                    app.reveal_selection();
                }
            }));
            if laid_out {
                ui.interact(
                    xy.response.rect,
                    ui.id().with("xy-laid-out"),
                    egui::Sense::hover(),
                )
                .on_hover_text("Placed by its container's layout — drag it on the canvas to reorder");
            }
            if let Some(size) = size0 {
                // A text's sizing mode decides what a typed number means.
                let sizing0 = match (
                    app.session.display_node(id).map(|n| n.kind().clone()),
                    app.session.doc.get(id),
                ) {
                    (Some(NodeKind::Text { sizing, .. }), Some(node)) => {
                        Some(tools::gesture_text_sizing(node, sizing))
                    }
                    _ => None,
                };
                // An authored `size` is replaced; a derived box goes through `tools::resize_box_to`.
                let authored = app
                    .session
                    .display_node(id)
                    .is_some_and(|n| tools::resizable_size(n.kind()).is_some());
                // `authors_height` is `Handle::anchored`'s `holds_y`, so field and
                // handle agree on whether the height becomes the user's.
                let size_tx = |app: &Self, w: f64, h: f64, authors_height: bool| {
                    if !authored {
                        return tools::resize_box_to(
                            &app.session.doc,
                            &app.session.resolved,
                            id,
                            Size::new(w, h),
                        );
                    }
                    let geometry = match sizing0 {
                        Some(sizing) => GeometryPatch::TextSizing(tools::resized_text_sizing(
                            sizing,
                            Size::new(w, h),
                            authors_height,
                        )),
                        None => GeometryPatch::Size(Size::new(w, h)),
                    };
                    Transaction(vec![Operation::SetGeometry { id, geometry }])
                };
                let locked = app
                    .session
                    .display_node(id)
                    .is_some_and(|n| n.proportions_locked());
                // W exactly as wide as X and H the rest, so the rows share an edge.
                let wide = egui::vec2(fw, 28.0);
                let rest = egui::vec2(
                    ui.available_width() - fw - 28.0 - ui::CARD_COL_GAP * 2.0,
                    28.0,
                );
                let now = size;
                // The lock stands aside while either side is a keyword or percentage.
                let both_px = size_menu
                    .as_ref()
                    .is_none_or(|m| m.w == SizeMode::Px && m.h == SizeMode::Px);
                let pairs = locked && both_px;
                let paired = |want: Size, drove_x: bool| {
                    if pairs {
                        tools::paired_size(now, want, drove_x)
                    } else {
                        want
                    }
                };
                // A percentage typed into W or H is the item's `width`/`height`.
                let percent_tx = |app: &Self, horizontal: bool, v: f64| {
                    let Some(node) = app.session.doc.get(id) else {
                        return Transaction(Vec::new());
                    };
                    let mut item = *node.item();
                    let d = ondin_core::container::Dimension::Percent(v.max(0.0));
                    if horizontal {
                        item.width = d;
                    } else {
                        item.height = d;
                    }
                    Transaction(vec![Operation::SetLayoutItem { id, item }])
                };
                let mut picks: Vec<(bool, SizeMode)> = Vec::new();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                    for horizontal in [true, false] {
                        let (letter, slot, drawn) = if horizontal {
                            ("W", wide, size.width)
                        } else {
                            ("H", rest, size.height)
                        };
                        let mode = size_menu.as_ref().map(|m| {
                            if horizontal {
                                (m.w, m.w_pct)
                            } else {
                                (m.h, m.h_pct)
                            }
                        });
                        // Bound, the field is a chip; only an axis the user sets takes a binding.
                        let axis = usize::from(!horizontal);
                        let key = super::binding::FieldKey::Size { id, axis };
                        let bindable = ondin_core::bindable_size_axes(&app.session.doc, id)[axis];
                        let held = if horizontal { &marks.w } else { &marks.h };
                        if (bindable || app.field_var(key).is_some())
                            && app.bound_field_ui(ui, key, slot.x, held)
                        {
                            continue;
                        }
                        let mut v = drawn;
                        let (resp, typed) = match (&size_menu, mode) {
                            (Some(m), Some((mode, pct))) => {
                                let mark = if horizontal { &marks.w } else { &marks.h };
                                let e = super::layout::size_field(
                                    ui,
                                    slot,
                                    Prefix::Text(letter),
                                    Some(mode),
                                    // Always a number: W and H are real sizes.
                                    Some(pct.unwrap_or(drawn)),
                                    drawn,
                                    &m.modes,
                                    mark.as_ref().map(Mark::field),
                                );
                                app.reset_marked(e.reset, mark);
                                if let Some(p) = e.picked {
                                    picks.push((horizontal, p));
                                }
                                if let Some(t) = e.typed {
                                    v = t;
                                }
                                (e.resp, e.typed.is_some())
                            }
                            _ => {
                                let mark = if horizontal { &marks.w } else { &marks.h };
                                let (r, reset) = ui::value_field_marked(
                                    ui,
                                    slot,
                                    Prefix::Text(letter),
                                    mark.as_ref().map(Mark::field),
                                    &mut v,
                                    Scrub::whole(0.5).range(1.0..=f64::MAX),
                                    |d| d.custom_formatter(ui::number(2)),
                                );
                                if let (true, Some(m)) = (reset, mark) {
                                    app.commit_reset(m.tx.clone());
                                }
                                let moved = super::layout::edited(&r, v != drawn);
                                (r, moved)
                            }
                        };
                        let tx = match mode {
                            Some((SizeMode::Percent, _)) if typed => percent_tx(app, horizontal, v),
                            // A click in and out would fix a keyword in px or change a text's mode.
                            _ if !typed => Transaction(Vec::new()),
                            _ if horizontal => {
                                let want = paired(Size::new(v.max(1.0), size.height), true);
                                // W authors the height only when the lock drives H too.
                                size_tx(app, want.width, want.height, pairs)
                            }
                            _ => {
                                let want = paired(Size::new(size.width, v.max(1.0)), false);
                                size_tx(app, want.width, want.height, true)
                            }
                        };
                        app.edit_valve(&resp, tx);
                        if bindable {
                            app.field_doors(ui, key, &resp);
                        }
                    }
                    let glyph = if locked {
                        icon::LINK_SIMPLE
                    } else {
                        icon::LINK_SIMPLE_BREAK
                    };
                    let state = if both_px {
                        ui::FieldButton::on_if(locked)
                    } else {
                        ui::FieldButton::Disabled
                    };
                    if ui::field_button(ui, glyph, 28.0, 15.0, state)
                        .on_hover_text(if !both_px {
                            "Proportions do not lock while a side is sized by a keyword or %"
                        } else if locked {
                            "Unlock proportions — Shift while dragging a handle also releases it"
                        } else {
                            "Lock proportions — W and H scale together, and handles resize proportionally"
                        })
                        .clicked()
                        && both_px
                    {
                        app.commit_edit(Transaction(vec![Operation::SetProportionsLocked {
                            id,
                            locked: !locked,
                        }]));
                    }
                });
                for (horizontal, mode) in picks {
                    let tx = app.size_mode_tx(id, horizontal, mode, size);
                    app.commit_edit(tx);
                }
            }
            // A line gets a length in W's slot: its box is not a property of the line.
            if let Some((a, b)) = line_ends {
                let wide = egui::vec2(fw, 28.0);
                ui.horizontal(|ui| {
                    let mut len = (b - a).hypot();
                    let (rl, reset_l) = ui::value_field_marked(
                        ui,
                        wide,
                        Prefix::Text("L"),
                        marks.l.as_ref().map(Mark::field),
                        &mut len,
                        Scrub::whole(0.5).range(0.01..=f64::MAX),
                        |d| d.custom_formatter(ui::number(2)),
                    );
                    if let (true, Some(m)) = (reset_l, &marks.l) {
                        app.commit_reset(m.tx.clone());
                    }
                    // Through `move_line_end`, the endpoint handle's own verb.
                    if let Some(target) = line_end_for_length(a, b, len) {
                        let tx = tools::move_line_end(
                            &app.session.doc,
                            &app.session.resolved,
                            id,
                            crate::preview::LineEnd::End,
                            target,
                        );
                        app.edit_valve(&rl, tx);
                    }
                });
            }
            app.scale_row(ui, fw);
            // Angles are read through `Basis`, not `atan2(b, a)`, which would lose a
            // mirror or misread a shear; the residual ancestor scale is multiplied
            // back on, or a nudge would resize. The round trip is exact only for an
            // invertible basis: `skew_exact` false stops the skew write.
            let split = build::Basis::of(Affine::new(basis));
            let skew_exact = build::Basis::exact(Affine::new(basis)).is_some();
            let scale = Affine::scale_non_uniform(split.scale.x, split.scale.y);
            let pv = app.session.preview_pivot(id).unwrap_or(Point::ZERO);
            let held = Affine::new(world) * pv;
            // Only skew rebuilds, replacing `skew` alone; rotation composes (`turned_about`).
            let rebuilt = |o: build::Orientation| {
                Affine::translate(held.to_vec2())
                    * o.to_affine()
                    * scale
                    * Affine::translate(-pv.to_vec2())
            };
            Self::transform_row(ui, "rot", |ui| {
                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                // A line's rotation is the world start-to-end angle; its direction
                // lives in the endpoint, not the basis.
                let line_angle = shown_line;
                // The document's angle, shown only at rest; a gesture shows its latch.
                let at_rest = shown_degrees(line_angle.unwrap_or(split.orientation.angle));
                let scrub_id = egui::Id::new(("rotation-scrub", id));
                let latch: Option<AngleScrub> = ui.data(|d| d.get_temp(scrub_id));
                let mut angle_deg = latch.map_or(at_rest, |l| l.shown);
                let (ra, reset_r) = ui::value_field_marked(
                    ui,
                    row,
                    Prefix::Icon(icon::ARROW_CLOCKWISE),
                    marks.r.as_ref().map(Mark::field),
                    &mut angle_deg,
                    Scrub::whole(0.5),
                    |d| d.suffix("°").custom_formatter(ui::number(2)),
                );
                let ra = match marks.r {
                    None => ra.on_hover_text("Rotate about the transform origin"),
                    Some(_) => ra,
                };
                if let (true, Some(m)) = (reset_r, &marks.r) {
                    app.commit_reset(m.tx.clone());
                }
                let (base, from) = AngleScrub::reference(latch, at_rest, Affine::new(world));
                let tr = app.place_at(
                    id,
                    turned_about(from, from * pv, (angle_deg - base).to_radians()),
                );
                // Held only while dragging; the release frame already read it above.
                let keep = ra.dragged().then_some(AngleScrub {
                    base,
                    world: from,
                    shown: angle_deg,
                });
                ui.data_mut(|d| match keep {
                    Some(s) => {
                        d.insert_temp(scrub_id, s);
                    }
                    None => d.remove::<AngleScrub>(scrub_id),
                });
                app.edit_valve(&ra, tr);
                let mut skew_deg = shown_degrees(split.orientation.skew);
                let rs = value_field(
                    ui,
                    row,
                    Prefix::Icon(icon::ANGLE),
                    &mut skew_deg,
                    // Short of a quarter turn, where the matrix collapses.
                    Scrub::whole(0.5)
                        .range(-tools::MAX_SKEW.to_degrees()..=tools::MAX_SKEW.to_degrees()),
                    |d| d.suffix("°").custom_formatter(ui::number(2)),
                )
                .on_hover_text("Lean the layer sideways — Ctrl-drag a side does the same");
                // An unrepresentable basis writes nothing but still goes through the valve.
                let ts = match skew_exact {
                    true => app.place_at(
                        id,
                        rebuilt(build::Orientation {
                            skew: skew_deg.to_radians().clamp(-tools::MAX_SKEW, tools::MAX_SKEW),
                            ..split.orientation
                        }),
                    ),
                    false => Transaction::default(),
                };
                app.edit_valve(&rs, ts);
            });
            Self::quarters_row(app, ui, TurnScope::Node(id));
        });
        if toggled {
            self.show_pivot = !self.show_pivot;
        }
    }
    /// Turn `id` a whole quarter about its transform origin, read from the
    /// committed tree like `tools::rotate_node`'s world transform.
    fn rotate_quarter(&mut self, id: NodeId, angle: f64) {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        let Some(pivot) = ondin_core::pivot_world(doc, res, id) else {
            return;
        };
        let tx = tools::rotate_node(doc, res, id, pivot, angle);
        self.commit_edit(tx);
    }
    /// The Appearance card's override marks for `id`: opacity, the
    /// radius — the single field over all four corners, and each corner's own —
    /// the vertex count and ratio, and *Clip content*.
    pub(super) fn appearance_marks(&self, id: NodeId) -> AppearanceMarks {
        use super::component::mark_num;
        let s = &[id];
        let radii_of = |op: &Operation| match op {
            Operation::SetGeometry {
                geometry: GeometryPatch::CornerRadii(r),
                ..
            } => Some(*r),
            _ => None,
        };
        let my_radii = |n: &ondin_core::Node| match n.kind() {
            NodeKind::Rect { corner_radii, .. } => Some(*corner_radii),
            _ => None,
        };
        let write_radii = |id, r| Operation::SetGeometry {
            id,
            geometry: GeometryPatch::CornerRadii(r),
        };
        let vars_of = |op: &Operation| match op {
            Operation::SetCornerVars { vars, .. } => Some(*vars),
            _ => None,
        };
        let my_vars = |n: &ondin_core::Node| Some(n.corner_vars());
        let write_vars = |id, vars| Operation::SetCornerVars { id, vars };
        // The tip names the variable shared, *per corner* where they differ, *unbound* if none.
        let table = self.session.doc.variables();
        let say_vars = |v: &[Option<ondin_core::VarId>]| match v {
            [first, rest @ ..] if rest.iter().all(|x| x == first) => match first {
                Some(var) => table
                    .get(*var)
                    .map_or_else(|| "a variable".to_owned(), |x| x.name.clone()),
                None => "unbound".to_owned(),
            },
            _ => "per corner".to_owned(),
        };
        let patch = |want: fn(&GeometryPatch) -> Option<f64>| {
            move |op: &Operation| match op {
                Operation::SetGeometry { geometry, .. } => want(geometry),
                _ => None,
            }
        };
        let sides = patch(|g| match g {
            GeometryPatch::Sides(n) => Some(f64::from(*n)),
            _ => None,
        });
        let ratio = patch(|g| match g {
            GeometryPatch::InnerRatio(r) => Some(*r),
            _ => None,
        });
        let my_sides = |n: &ondin_core::Node| match n.kind() {
            NodeKind::Polygon { sides, .. } => Some(f64::from(*sides)),
            NodeKind::Star { points, .. } => Some(f64::from(*points)),
            _ => None,
        };
        let my_ratio = |n: &ondin_core::Node| match n.kind() {
            NodeKind::Star { inner_ratio, .. } => Some(*inner_ratio),
            _ => None,
        };
        let vertex_radius = patch(|g| match g {
            GeometryPatch::CornerRadius(r) => Some(*r),
            _ => None,
        });
        let my_vertex_radius = |n: &ondin_core::Node| match n.kind() {
            NodeKind::Polygon { corner_radius, .. } | NodeKind::Star { corner_radius, .. } => {
                Some(*corner_radius)
            }
            _ => None,
        };
        AppearanceMarks {
            // The binding first: a bound literal is 0, so a copy can differ by it alone.
            opacity: Mark::and(
                self.sub_mark(
                    s,
                    |op| match op {
                        Operation::SetOpacityVar { var, .. } => Some(*var),
                        _ => None,
                    },
                    |n| Some(n.opacity_var()),
                    |v| *v,
                    |c, v| *c = v,
                    |id, var| Operation::SetOpacityVar { id, var },
                    |v| say_vars(std::slice::from_ref(v)),
                ),
                self.sub_mark(
                    s,
                    |op| match op {
                        Operation::SetOpacity { opacity, .. } => Some(*opacity),
                        _ => None,
                    },
                    |n| Some(n.opacity()),
                    |v| *v,
                    |c, v| *c = v,
                    |id, opacity| Operation::SetOpacity { id, opacity },
                    |v| format!("{}%", mark_num(f64::from(*v) * 100.0)),
                ),
            ),
            radius: Mark::and(
                self.sub_mark(
                    s,
                    vars_of,
                    my_vars,
                    |v| *v,
                    |c, v| *c = v,
                    write_vars,
                    |v| say_vars(&v[..]),
                ),
                self.sub_mark(
                    s,
                    radii_of,
                    my_radii,
                    |r| *r,
                    |c, v| *c = v,
                    write_radii,
                    |r| uniform(*r).map_or_else(|| "per corner".to_owned(), mark_num),
                ),
            ),
            corners: Corner::ALL.map(|c| {
                let k = c.var_slot();
                Mark::and(
                    self.sub_mark(
                        s,
                        vars_of,
                        my_vars,
                        |v| v[k],
                        |v, x| v[k] = x,
                        write_vars,
                        |x| say_vars(std::slice::from_ref(x)),
                    ),
                    self.sub_mark(
                        s,
                        radii_of,
                        my_radii,
                        |r| c.get(r),
                        |r, v| c.set(r, v),
                        write_radii,
                        |v| mark_num(*v),
                    ),
                )
            }),
            count: self.sub_mark(
                s,
                sides,
                my_sides,
                |v| *v,
                |c, v| *c = v,
                |id, n| Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::Sides(n as u32),
                },
                |v| mark_num(*v),
            ),
            ratio: self.sub_mark(
                s,
                ratio,
                my_ratio,
                |v| *v,
                |c, v| *c = v,
                |id, r| Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::InnerRatio(r),
                },
                |v| format!("{}%", mark_num(*v * 100.0)),
            ),
            vertex_radius: self.sub_mark(
                s,
                vertex_radius,
                my_vertex_radius,
                |v| *v,
                |c, v| *c = v,
                |id, r| Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::CornerRadius(r),
                },
                |v| mark_num(*v),
            ),
            clip: self.sub_mark(
                s,
                |op| match op {
                    Operation::SetClip { clip, .. } => Some(*clip),
                    _ => None,
                },
                |n| Some(n.clip()),
                |v| *v,
                |c, v| *c = v,
                |id, clip| Operation::SetClip { id, clip },
                |v| if *v { "on" } else { "off" }.to_owned(),
            ),
        }
    }
    /// One layer's opacity field, `fw` wide, for both Appearance cards. Bound, it
    /// is a chip.
    fn opacity_field(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        opacity0: f32,
        fw: f32,
        mark: &Option<Mark>,
    ) {
        let key = super::binding::FieldKey::Opacity { id };
        if self.bound_field_ui(ui, key, fw, mark) {
            return;
        }
        let mut pct = opacity0 * 100.0;
        let (ro, _, hit) = ui::value_field_unit_marked(
            ui,
            egui::vec2(fw, 28.0),
            Prefix::Icon(icon::DROP_HALF),
            None,
            mark.as_ref().map(Mark::field),
            &mut pct,
            Scrub::whole(0.5).range(0.0..=100.0),
            |d| d.suffix("%").max_decimals(0),
        );
        self.reset_marked(hit, mark);
        self.edit_valve(
            &ro,
            Transaction(vec![Operation::SetOpacity {
                id,
                opacity: (pct / 100.0).clamp(0.0, 1.0),
            }]),
        );
        self.field_doors(ui, key, &ro);
    }
    /// Opacity, corner radius, and the vertex counts — the design's Appearance panel.
    #[allow(clippy::too_many_arguments)]
    fn inspector_appearance(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        opacity0: f32,
        corners0: Option<RoundedRectRadii>,
        vertices0: Option<Vertices>,
        clip0: Option<bool>,
    ) {
        let marks = self.appearance_marks(id);
        self.panel(ui, "Appearance", None, |app, ui| {
            let per_corner = app.per_corner_radius.contains(&id);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                // Opacity always; radius and its toggle only on a rect.
                let button_space = if corners0.is_some() {
                    28.0 + ui::CARD_COL_GAP
                } else {
                    0.0
                };
                let fields = if corners0.is_some() { 2.0 } else { 1.0 };
                let fw = (ui.available_width() - button_space - ui::CARD_COL_GAP * (fields - 1.0))
                    / fields;
                if let Some(vertices) = vertices0 {
                    let fw = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
                    app.opacity_field(ui, id, opacity0, fw, &marks.opacity);
                    app.vertex_radius_field(ui, id, vertices.radius(), fw, &marks.vertex_radius);
                    return;
                }
                app.opacity_field(ui, id, opacity0, fw, &marks.opacity);
                let Some(corners) = corners0 else { return };
                // Bound, the field is a chip.
                let key = super::binding::FieldKey::Radius { id };
                if !app.bound_field_ui(ui, key, fw, &marks.radius) {
                    // Edits all four at once; shows the shared value, or the largest.
                    let shared = uniform(corners);
                    let mut r = shared.unwrap_or_else(|| largest(corners));
                    let seed = r;
                    let (rr, hit) = ui::value_field_marked(
                        ui,
                        egui::vec2(fw, 28.0),
                        Prefix::Icon(icon::CORNERS_OUT),
                        marks.radius.as_ref().map(Mark::field),
                        &mut r,
                        Scrub::whole(0.5).range(0.0..=f64::MAX),
                        |d| d.custom_formatter(ui::number(2)),
                    );
                    app.reset_marked(hit, &marks.radius);
                    // Seeded from one corner of a disagreeing set, it may not write
                    // until moved, or a bare click would flatten the other corners.
                    if shared.is_some() || r != seed {
                        app.edit_valve(
                            &rr,
                            Transaction(vec![Operation::SetGeometry {
                                id,
                                geometry: GeometryPatch::CornerRadius(r.max(0.0)),
                            }]),
                        );
                    }
                    app.field_doors(ui, key, &rr);
                }
                if ui::field_button(
                    ui,
                    icon::FRAME_CORNERS,
                    28.0,
                    15.0,
                    ui::FieldButton::on_if(per_corner),
                )
                .on_hover_text("Set each corner separately")
                .clicked()
                {
                    if per_corner {
                        app.per_corner_radius.remove(&id);
                    } else {
                        app.per_corner_radius.insert(id);
                    }
                }
            });
            if let Some(vertices) = vertices0 {
                app.vertex_fields(ui, id, vertices, &marks);
            }
            // A frame's "clip content", as a switch. An overridden clip is reset by
            // the toggle itself, so its tooltip names the main's value.
            if let Some(clip) = clip0
                && ui::switch_row_marked(ui, "Clip content", clip, BOOL_ROW_H, marks.clip.is_some())
                    .on_hover_text(match &marks.clip {
                        Some(m) => m.tip.as_str(),
                        None => "Hide anything that falls outside this frame",
                    })
                    .clicked()
            {
                app.commit_edit(Transaction(vec![Operation::SetClip { id, clip: !clip }]));
            }
            let Some(corners) = corners0 else { return };
            if !per_corner {
                return;
            }
            // Laid out as the corners sit on the shape.
            let fw = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
            for pair in [
                [
                    (icon::ARROW_ELBOW_LEFT_DOWN, Corner::TopLeft, "Top left"),
                    (icon::ARROW_ELBOW_RIGHT_DOWN, Corner::TopRight, "Top right"),
                ],
                [
                    (icon::ARROW_ELBOW_LEFT_UP, Corner::BottomLeft, "Bottom left"),
                    (
                        icon::ARROW_ELBOW_RIGHT_UP,
                        Corner::BottomRight,
                        "Bottom right",
                    ),
                ],
            ] {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                    for (glyph, corner, tip) in pair {
                        let mark = &marks.corners[corner as usize];
                        // A bound corner is a chip of its own, keyed by corner.
                        let key = super::binding::FieldKey::Corner {
                            id,
                            slot: corner.var_slot(),
                        };
                        if app.bound_field_ui(ui, key, fw, mark) {
                            continue;
                        }
                        let mut v = corner.get(&corners);
                        let (resp, hit) = ui::value_field_marked(
                            ui,
                            egui::vec2(fw, 28.0),
                            Prefix::Icon(glyph),
                            mark.as_ref().map(Mark::field),
                            &mut v,
                            Scrub::whole(0.5).range(0.0..=f64::MAX),
                            |d| d.custom_formatter(ui::number(2)),
                        );
                        let resp = tip_unless_marked(resp, mark, tip);
                        app.reset_marked(hit, mark);
                        let mut next = corners;
                        corner.set(&mut next, v.max(0.0));
                        app.edit_valve(
                            &resp,
                            Transaction(vec![Operation::SetGeometry {
                                id,
                                geometry: GeometryPatch::CornerRadii(next),
                            }]),
                        );
                        app.field_doors(ui, key, &resp);
                    }
                });
            }
        });
    }
    /// A polygon's or star's corner radius, one value for every vertex. Unlike a
    /// rect's it takes no variable, so it has no binding doors.
    fn vertex_radius_field(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        radius0: f64,
        fw: f32,
        mark: &Option<Mark>,
    ) {
        let mut r = radius0;
        let (rr, hit) = ui::value_field_marked(
            ui,
            egui::vec2(fw, 28.0),
            Prefix::Icon(icon::CORNERS_OUT),
            mark.as_ref().map(Mark::field),
            &mut r,
            Scrub::whole(0.5).range(0.0..=f64::MAX),
            |d| d.custom_formatter(ui::number(2)),
        );
        let rr = tip_unless_marked(rr, mark, "Corner radius");
        self.reset_marked(hit, mark);
        self.edit_valve(
            &rr,
            Transaction(vec![Operation::SetGeometry {
                id,
                geometry: GeometryPatch::CornerRadius(r.max(0.0)),
            }]),
        );
    }
    /// The vertex-count row inside Appearance: a polygon's sides, or a star's
    /// points and inner ratio.
    ///
    /// The ratio is shown as a percentage, floored at `geometry::MIN_INNER_RATIO`.
    fn vertex_fields(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        vertices: Vertices,
        marks: &AppearanceMarks,
    ) {
        use ondin_core::geometry::{MAX_SIDES, MIN_INNER_RATIO, MIN_SIDES};
        // Half width either way.
        let fw = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
            let (glyph, count, tip) = match vertices {
                Vertices::Polygon { sides, .. } => (icon::TRIANGLE, sides, "Sides"),
                Vertices::Star { points, .. } => (icon::ASTERISK, points, "Points"),
            };
            let mut n = f64::from(count);
            let (rn, hit) = ui::value_field_marked(
                ui,
                egui::vec2(fw, 28.0),
                Prefix::Icon(glyph),
                marks.count.as_ref().map(Mark::field),
                &mut n,
                Scrub::whole(0.08).range(f64::from(MIN_SIDES)..=f64::from(MAX_SIDES)),
                |d| d.max_decimals(0),
            );
            let rn = tip_unless_marked(rn, &marks.count, tip);
            self.reset_marked(hit, &marks.count);
            self.edit_valve(
                &rn,
                Transaction(vec![Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::Sides((n.round() as u32).clamp(MIN_SIDES, MAX_SIDES)),
                }]),
            );
            let Vertices::Star { inner_ratio, .. } = vertices else {
                return;
            };
            // `fine`, not `whole`: the default 38.2% must be typeable and reachable.
            let mut pct = inner_ratio * 100.0;
            let (rr, hit) = ui::value_field_marked(
                ui,
                egui::vec2(fw, 28.0),
                Prefix::Icon(icon::ANGLE),
                marks.ratio.as_ref().map(Mark::field),
                &mut pct,
                Scrub::fine(0.25, 1).range(MIN_INNER_RATIO * 100.0..=100.0),
                |d| d.suffix("%").max_decimals(1),
            );
            let rr = tip_unless_marked(rr, &marks.ratio, "How far the inner points are pulled in");
            self.reset_marked(hit, &marks.ratio);
            self.edit_valve(
                &rr,
                Transaction(vec![Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::InnerRatio((pct / 100.0).clamp(MIN_INNER_RATIO, 1.0)),
                }]),
            );
        });
    }
    // --- fill and stroke ---------------------------------------------------
    /// The Fill and Stroke panels for a scope, shown only where they have
    /// somewhere to write (`takes_paint`).
    fn inspector_paint_panels(&mut self, ui: &mut egui::Ui, scope: PaintScope, fresh: bool) {
        let fills = self.fills_shown(scope);
        let strokes = self.strokes_shown(scope);
        let paints = match scope {
            PaintScope::Node(id) => match self.session.doc.get(id).map(|n| n.kind()) {
                Some(kind) => kind.takes_paint(),
                None => return,
            },
            PaintScope::Selection => {
                ondin_core::any_paint_in(&self.session.doc, self.session.selection.ids())
            }
        };
        if fresh {
            // Index-keyed Stroke chrome would now name another layer's stroke.
            self.forget_stroke_chrome();
        }
        if paints {
            self.inspector_fill(ui, scope, &fills, fresh);
            self.inspector_stroke(ui, scope, &strokes, fresh);
        }
    }
    /// The Fill panel, for one layer or for the whole selection.
    ///
    /// One row per fill, newest on top, and a `+` in the header that adds one. Rows
    /// open the detached picker, because a gradient has to be judged against the
    /// artwork. A disagreeing `shown` draws one stand-in row
    /// ([`Self::mixed_paint_row`]) writing each property to every paint in scope.
    fn inspector_fill(
        &mut self,
        ui: &mut egui::Ui,
        scope: PaintScope,
        shown: &PaintShown<Keyed<Fill>>,
        fresh: bool,
    ) {
        let fills = shown.list();
        // An instance's ghost rows are something to show.
        let ghosts = self.has_ghosts(scope.node(), |n| &n.paint().fills);
        self.sync_paint_collapse("Fill", shown.unpainted() && !ghosts, fresh);
        let action = Some(if shown.is_mixed() {
            ui::HeadAction::inert(
                icon::PLUS,
                "These layers are filled differently: no list to add to",
            )
        } else {
            ui::HeadAction::new(icon::PLUS, "Add a fill")
        });
        let targets = self.paint_style_targets(scope);
        let add = self.style_card(ui, "Fill", StyleSlot::Fills, &targets, action, |app, ui| {
            if shown.is_mixed() {
                app.mixed_paint_row(ui, PaintList::Fill);
                return;
            }
            // Asked before the empty list is: an instance that removed every item still has ghosts.
            let source = match scope {
                PaintScope::Node(id) => app.source_list(id, |n| &n.paint().fills),
                PaintScope::Selection => None,
            };
            if fills.is_empty() && source.as_ref().is_none_or(Vec::is_empty) {
                ui.label(
                    egui::RichText::new("No fill")
                        .size(11.0)
                        .color(theme::text::FAINT),
                );
                return;
            }
            let anchor = scope.anchor(app);
            let states = source
                .as_ref()
                .map(|s| ondin_core::reset::item_states(s, fills));
            let (mut remove, mut toggle, mut reset) = (None, None, None);
            let carrying = app.carrying(PaintList::Fill, anchor);
            let order = app.paint_order(ui, PaintList::Fill, anchor, fills.len());
            let mut slots = paint::Slots::default();
            for &index in &order {
                let salt = ("fill-row", index);
                let shown = app
                    .session
                    .doc
                    .resolve_brush(anchor, &fills[index].brush)
                    .into_owned();
                let (out, rect) = carried_unit(ui, carrying == Some(index), salt, |ui| {
                    app.paint_row(
                        ui,
                        RowCtx {
                            id: anchor,
                            slot: scope.slot(PaintList::Fill, index),
                            visible: fills[index].visible,
                            grip: Some((PaintList::Fill, index)),
                            reorderable: fills.len() > 1,
                            census: None,
                            mixed: None,
                            item: states.as_ref().map(|s| s[index]),
                        },
                        &shown,
                    )
                });
                slots.push(rect.top(), rect.bottom());
                remove = remove.or(out.remove.then_some(index));
                toggle = toggle.or(out.toggle.then_some(index));
                reset = reset.or(out.reset.then_some(index));
            }
            // The main's fills this instance removed, as ghost rows.
            let mut restore = None;
            for gone in source
                .as_deref()
                .map(|s| ondin_core::reset::removed_items(s, fills))
                .unwrap_or_default()
            {
                let lead = super::component::GhostLead::Chip;
                let label = paint::source_label(&app.session.doc, &gone.brush);
                if super::component::ghost_row(ui, lead, &label) {
                    restore = Some(gone.id);
                }
            }
            if let Some((from, to)) = app.finish_paint_drag(ui, PaintList::Fill, anchor, slots) {
                app.write_fill_list(scope, paint::reordered(fills, from, to));
                let n = fills.len();
                app.remap_paint_indices(anchor, PaintList::Fill, |i| {
                    paint::reordered_index(n, from, to, i)
                });
            } else if let (Some(_), Some(src)) = (restore, source.as_deref()) {
                // A ghost row's restore is the whole list back as the main has it.
                app.write_fill_list(scope, src.to_vec());
            } else if let (Some(item), Some(src)) = (reset.map(|i| fills[i].id), source.as_deref())
            {
                app.write_fill_list(scope, ondin_core::reset::reset_item(src, fills, item));
            } else if let Some(i) = toggle {
                let mut next = fills.to_vec();
                next[i].visible = !next[i].visible;
                app.write_fill_list(scope, next);
            } else if let Some(i) = remove {
                let mut next = fills.to_vec();
                next.remove(i);
                app.write_fill_list(scope, next);
                app.remap_paint_indices(anchor, PaintList::Fill, |j| paint::removed_index(i, j));
            }
        });
        if add {
            self.open_paint_panel("Fill");
            let mut next = fills.to_vec();
            // Over a selection every layer gets this one id, so a main and its
            // instance added to together share the item.
            next.push(Keyed::new(
                self.session.ids.mint_item(),
                Fill {
                    brush: paint::new_solid().into(),
                    visible: true,
                },
            ));
            self.write_fill_list(scope, next);
            // Opened next frame by the row, which knows where it lands.
            self.pending_picker =
                Some((scope.anchor(self), scope.slot(PaintList::Fill, fills.len())));
        }
    }
    /// The Stroke panel: one two-row unit per stroke — its colour, then its
    /// position, weight and the button holding the rest.
    ///
    /// The two rows travel together through a reorder. Over a disagreeing
    /// selection it collapses to one stand-in unit, as [`Self::inspector_fill`]
    /// does, plus [`Self::mixed_stroke_properties`].
    fn inspector_stroke(
        &mut self,
        ui: &mut egui::Ui,
        scope: PaintScope,
        shown: &PaintShown<Keyed<Stroke>>,
        fresh: bool,
    ) {
        let strokes = shown.list();
        let ghosts = self.has_ghosts(scope.node(), |n| &n.paint().strokes);
        self.sync_paint_collapse("Stroke", shown.unpainted() && !ghosts, fresh);
        let action = Some(if shown.is_mixed() {
            ui::HeadAction::inert(
                icon::PLUS,
                "These layers are stroked differently: no list to add to",
            )
        } else {
            ui::HeadAction::new(icon::PLUS, "Add a stroke")
        });
        let targets = self.paint_style_targets(scope);
        let add = self.style_card(
            ui,
            "Stroke",
            StyleSlot::Strokes,
            &targets,
            action,
            |app, ui| {
                if shown.is_mixed() {
                    paint_unit(
                        ui,
                        app,
                        true,
                        |app, ui| app.mixed_paint_row(ui, PaintList::Stroke),
                        |app, ui| app.mixed_stroke_properties(ui, scope),
                    );
                    return;
                }
                let source = match scope {
                    PaintScope::Node(id) => app.source_list(id, |n| &n.paint().strokes),
                    PaintScope::Selection => None,
                };
                if strokes.is_empty() && source.as_ref().is_none_or(Vec::is_empty) {
                    ui.label(
                        egui::RichText::new("No stroke")
                            .size(11.0)
                            .color(theme::text::FAINT),
                    );
                    return;
                }
                let anchor = scope.anchor(app);
                let states = source
                    .as_ref()
                    .map(|s| ondin_core::reset::item_states(s, strokes));
                let (mut remove, mut toggle, mut reset) = (None, None, None);
                let mut edit: Option<(usize, Stroke)> = None;
                let carrying = app.carrying(PaintList::Stroke, anchor);
                let order = app.paint_order(ui, PaintList::Stroke, anchor, strokes.len());
                let mut slots = paint::Slots::default();
                for (pos, &index) in order.iter().enumerate() {
                    let s = &strokes[index];
                    let shown = app.session.doc.resolve_brush(anchor, &s.brush).into_owned();
                    let salt = ("stroke-row", index);
                    let last = pos + 1 == order.len();
                    let ((out, changed), rect) =
                        carried_unit(ui, carrying == Some(index), salt, |ui| {
                            paint_unit(
                                ui,
                                app,
                                last,
                                |app, ui| {
                                    app.paint_row(
                                        ui,
                                        RowCtx {
                                            id: anchor,
                                            slot: scope.slot(PaintList::Stroke, index),
                                            visible: s.visible,
                                            grip: Some((PaintList::Stroke, index)),
                                            reorderable: strokes.len() > 1,
                                            census: None,
                                            mixed: None,
                                            item: states.as_ref().map(|s| s[index]),
                                        },
                                        &shown,
                                    )
                                },
                                |app, ui| app.stroke_properties(ui, scope, index, s),
                            )
                        });
                    slots.push(rect.top(), rect.bottom());
                    remove = remove.or(out.remove.then_some(index));
                    toggle = toggle.or(out.toggle.then_some(index));
                    edit = edit.or(changed.map(|s| (index, s)));
                    reset = reset.or(out.reset.then_some(index));
                }
                // The main's strokes this instance removed, as ghost rows.
                let mut restore = None;
                for gone in source
                    .as_deref()
                    .map(|s| ondin_core::reset::removed_items(s, strokes))
                    .unwrap_or_default()
                {
                    let lead = super::component::GhostLead::Chip;
                    let label = paint::source_label(&app.session.doc, &gone.brush);
                    if super::component::ghost_row(ui, lead, &label) {
                        restore = Some(gone.id);
                    }
                }
                if let Some((from, to)) =
                    app.finish_paint_drag(ui, PaintList::Stroke, anchor, slots)
                {
                    app.write_strokes(scope, paint::reordered(strokes, from, to));
                    // Index-keyed chrome is renumbered so open popups follow their stroke.
                    let n = strokes.len();
                    app.remap_paint_indices(anchor, PaintList::Stroke, |i| {
                        paint::reordered_index(n, from, to, i)
                    });
                } else if let (Some(_), Some(src)) = (restore, source.as_deref()) {
                    app.write_strokes(scope, src.to_vec());
                } else if let (Some(item), Some(src)) =
                    (reset.map(|i| strokes[i].id), source.as_deref())
                {
                    app.write_strokes(scope, ondin_core::reset::reset_item(src, strokes, item));
                } else if let Some(i) = toggle {
                    let mut next = strokes.to_vec();
                    next[i].visible = !next[i].visible;
                    app.write_strokes(scope, next);
                } else if let Some(i) = remove {
                    let mut next = strokes.to_vec();
                    next.remove(i);
                    app.write_strokes(scope, next);
                    app.remap_paint_indices(anchor, PaintList::Stroke, |j| {
                        paint::removed_index(i, j)
                    });
                } else if let Some((i, s)) = edit {
                    let mut next = strokes.to_vec();
                    next[i].value = s;
                    app.write_strokes(scope, next);
                }
            },
        );
        if add {
            self.open_paint_panel("Stroke");
            let mut next = strokes.to_vec();
            next.push(Keyed::new(
                self.session.ids.mint_item(),
                Stroke {
                    brush: paint::new_solid().into(),
                    ..Default::default()
                },
            ));
            self.write_strokes(scope, next);
            self.pending_picker = Some((
                scope.anchor(self),
                scope.slot(PaintList::Stroke, strokes.len()),
            ));
        }
    }
    /// The one row a Fill or Stroke panel shows over a selection whose targets
    /// disagree: the colour they share or the mixed chip, an opacity, an eye, an `x`.
    ///
    /// The same [`Self::paint_row`], told which values it may not state; its grip
    /// is inert. The `x` clears every target, the way back to a live `+`.
    fn mixed_paint_row(&mut self, ui: &mut egui::Ui, list: PaintList) {
        let target = list.target();
        let (brush, mixed) = self.mixed_row(target);
        let visible = self.mixed_visible(target);
        let anchor = self.session.page;
        let out = self.paint_row(
            ui,
            RowCtx {
                id: anchor,
                slot: PaintSlot::SelectionAll(target),
                visible,
                grip: Some((list, 0)),
                reorderable: false,
                census: None,
                mixed: Some(mixed),
                item: None,
            },
            &brush,
        );
        let ids = self.session.selection.ids().to_vec();
        if out.remove {
            let tx = match target {
                ondin_core::PaintTarget::Fill => {
                    ondin_core::set_fills_all(&self.session.doc, &ids, &[], &mut self.session.ids)
                }
                ondin_core::PaintTarget::Stroke => {
                    ondin_core::set_strokes_all(&self.session.doc, &ids, &[], &mut self.session.ids)
                }
            };
            self.commit_edit(tx);
            self.picker = None;
        } else if out.toggle {
            let tx = match target {
                ondin_core::PaintTarget::Fill => {
                    ondin_core::edit_fills_all(&self.session.doc, &ids, |f| f.visible = !visible)
                }
                ondin_core::PaintTarget::Stroke => {
                    ondin_core::edit_strokes_all(&self.session.doc, &ids, |s| s.visible = !visible)
                }
            };
            self.commit_edit(tx);
        }
    }
    /// The stroke-position dropdown, for both stroke rows.
    ///
    /// `shown` is `None` when the strokes disagree: the combo reads *Mixed*, which
    /// is not an option, and nothing is lit. Returns what was picked this frame.
    ///
    /// The height goes in through `interact_size.y` at the full 28: a combo strokes
    /// its border inside its rect, where a `Frame` counts it outside
    /// ([`ui::FIELD_BORDER_H`]). With `aligns` false (an open shape) the combo
    /// opens onto a sentence instead.
    fn align_combo(
        ui: &mut egui::Ui,
        salt: impl std::hash::Hash + std::fmt::Debug,
        aligns: bool,
        combo_w: f32,
        shown: Option<StrokeAlign>,
    ) -> Option<StrokeAlign> {
        let mut picked = None;
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = 28.0;
            ui.spacing_mut().button_padding.y = 0.0;
            let label = match shown {
                Some(a) => align_label(a),
                None => "Mixed",
            };
            let combo = egui::ComboBox::from_id_salt(salt)
                .icon(ui::combo_chevron)
                .width(combo_w)
                .height(140.0)
                .selected_text(ui::glyph_and_text(icon::SQUARE_HALF, label));
            if aligns {
                combo.show_ui(ui, |ui| {
                    ui::menu_rows(ui);
                    ui.spacing_mut().button_padding.y = 2.0;
                    for a in [
                        StrokeAlign::Center,
                        StrokeAlign::Inside,
                        StrokeAlign::Outside,
                    ] {
                        if ui
                            .selectable_label(shown == Some(a), align_label(a))
                            .clicked()
                        {
                            picked = Some(a);
                        }
                    }
                });
            } else {
                combo
                    .show_ui(ui, |ui| {
                        ui.label(
                            egui::RichText::new("Open shapes stroke centred")
                                .size(11.0)
                                .color(theme::text::FAINT),
                        );
                    })
                    .response
                    .on_hover_text("Only closed shapes have an inside");
            }
        });
        picked
    }
    /// The width the position dropdown takes in either stroke row.
    ///
    /// The design's `1.2fr 1fr 28px 28px`: a 1.2 : 1 split of what the two buttons leave.
    fn align_combo_width(ui: &egui::Ui) -> f32 {
        let free = ui.available_width() - 28.0 * 2.0 - ui::CARD_COL_GAP * 3.0;
        free * 1.2 / 2.2
    }
    /// The mixed unit's second row: the position and the weight every stroke in
    /// scope shares, or "Mixed" for whichever they do not — laid out exactly as the
    /// per-stroke row, its two buttons dimmed rather than dropped (they have no
    /// mixed reading and are keyed by list index), keeping the widths.
    fn mixed_stroke_properties(&mut self, ui: &mut egui::Ui, scope: PaintScope) {
        let ids = self.session.selection.ids().to_vec();
        let align = self.shown_over_strokes(|s| s.align);
        let widths = self.shown_over_strokes(|s| s.width);
        let shared_align = align.agreed();
        let shared_width = widths.agreed();
        let aligns = self.stroke_rule(scope, ondin_core::geometry::stroke_align_applies);
        let mut picked: Option<StrokeAlign> = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
            let combo_w = Self::align_combo_width(ui);
            picked = Self::align_combo(ui, "stroke-align-mixed", aligns, combo_w, shared_align);
            // Dragged from `Spread::base`, not zero, so a scrub over a `bare` target accumulates.
            let mut width = shared_width.or_else(|| widths.base()).unwrap_or(0.0);
            let rw = value_field(
                ui,
                egui::vec2(
                    ui.available_width() - 28.0 * 2.0 - ui::CARD_COL_GAP * 2.0,
                    28.0,
                ),
                Prefix::Icon(icon::LINE_SEGMENT),
                &mut width,
                Scrub::whole(0.5).range(0.0..=ui::MAX_STROKE_WIDTH),
                |d| {
                    if shared_width.is_none() {
                        return d.custom_formatter(|_, _| "Mixed".into());
                    }
                    d.custom_formatter(ui::number(2))
                },
            )
            .on_hover_text("Weight of every stroke in the selection");
            if interacting(&rw) {
                // Weight is master over per-side widths (`weight_all_tx`).
                let tx = weight_all_tx(&self.session.doc, &ids, width);
                self.edit_valve(&rw, tx);
            }
            ui::field_button(
                ui,
                icon::SLIDERS_HORIZONTAL,
                28.0,
                15.0,
                ui::FieldButton::Disabled,
            )
            .on_hover_text("One stroke at a time: dash, cap and join");
            ui::field_button(ui, icon::CORNERS_OUT, 28.0, 15.0, ui::FieldButton::Disabled)
                .on_hover_text("One stroke at a time: which sides the stroke is on");
        });
        if let Some(a) = picked {
            let tx = ondin_core::edit_strokes_all(&self.session.doc, &ids, |s| s.align = a);
            self.commit_edit(tx);
        }
    }
    /// One stroke's second row: where the width sits relative to the outline, the
    /// width itself, and the button holding dash, cap and join.
    ///
    /// Returns the stroke as edited, or `None`; the caller owns the list and
    /// writes it.
    fn stroke_properties(
        &mut self,
        ui: &mut egui::Ui,
        scope: PaintScope,
        index: usize,
        stroke: &Stroke,
    ) -> Option<Stroke> {
        let aligns = self.stroke_rule(scope, ondin_core::geometry::stroke_align_applies);
        let sides_apply = self.stroke_rule(scope, ondin_core::geometry::stroke_sides_apply);
        let joins = self.stroke_rule(scope, ondin_core::geometry::stroke_join_applies);
        // Open, not "open or set": a collapsed row keeps its setting (`FieldButton::Set`).
        let shown = sides_apply && self.stroke_sides_open.contains(&index);
        let mut out = None;
        let dim = if stroke.visible { 1.0 } else { HIDDEN_OPACITY };
        ui.scope(|ui| {
            ui.multiply_opacity(dim);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                let combo_w = Self::align_combo_width(ui);
                let picked = Self::align_combo(
                    ui,
                    ("stroke-align", index),
                    aligns,
                    combo_w,
                    Some(stroke.align),
                );
                if let Some(align) = picked.filter(|a| *a != stroke.align) {
                    out = Some(Stroke {
                        align,
                        ..stroke.clone()
                    });
                }
                let field_w = ui.available_width() - 28.0 * 2.0 - ui::CARD_COL_GAP * 2.0;
                // Bound, the weight is a chip — a layer's own stroke only.
                let key = match (scope, self.session.doc.get(scope.anchor(self))) {
                    // Not on `Custom` sides, where a bind would not land.
                    (PaintScope::Node(id), Some(n)) => n
                        .paint()
                        .strokes
                        .get(index)
                        .filter(|s| !ondin_core::build::draws_own_sides(n.kind(), s))
                        .map(|s| super::binding::FieldKey::StrokeWidth { id, item: s.id }),
                    _ => None,
                };
                if !key.is_some_and(|k| self.bound_field_ui(ui, k, field_w, &None)) {
                    let mut width = stroke.width;
                    let rw = value_field(
                        ui,
                        egui::vec2(field_w, 28.0),
                        Prefix::Icon(icon::LINE_SEGMENT),
                        &mut width,
                        Scrub::whole(0.5).range(0.0..=ui::MAX_STROKE_WIDTH),
                        |d| d.custom_formatter(ui::number(2)),
                    );
                    if let Some(k) = key {
                        self.field_doors(ui, k, &rw);
                    }
                    // Through the valve, not `out`, so a drag previews every frame.
                    // Editing the weight flattens `Custom` per-side widths onto it.
                    self.stroke_valve(&rw, scope, index, |s| set_stroke_weight(s, width));
                }
                let open = self.stroke_menu == Some(index);
                let more = ui::field_button(
                    ui,
                    icon::SLIDERS_HORIZONTAL,
                    28.0,
                    15.0,
                    ui::FieldButton::on_if(open),
                )
                .on_hover_text("Dash, cap and join");
                if more.clicked() {
                    self.stroke_menu = if open { None } else { Some(index) };
                }
                if let Some(s) = self.stroke_menu_popup(ui, &more, scope, index, stroke, joins) {
                    out = Some(s);
                }
                let state = sides_button_state(sides_apply, shown, stroke.sides);
                if ui::field_button(ui, icon::CORNERS_OUT, 28.0, 15.0, state)
                    .on_hover_text(match state {
                        ui::FieldButton::Disabled => "Only a rectangle has four sides".into(),
                        ui::FieldButton::On => "Hide the sides selector".to_string(),
                        ui::FieldButton::Set => format!("Stroke on: {}", sides_label(stroke.sides)),
                        // `Danger` is unreachable; the arm keeps the match exhaustive.
                        ui::FieldButton::Off | ui::FieldButton::Danger => {
                            "Which sides the stroke is on".into()
                        }
                    })
                    .clicked()
                    && sides_apply
                {
                    if shown {
                        self.stroke_sides_open.retain(|i| *i != index);
                    } else {
                        self.stroke_sides_open.push(index);
                    }
                }
            });
            if shown && let Some(s) = self.stroke_sides_row(ui, scope, index, stroke) {
                out = Some(s);
            }
        });
        out
    }
    /// The sides selector under one stroke's properties row, and — on `Custom` —
    /// the four per-side widths. Rows in the card, not a popup, so the values read
    /// beside the weight and travel with a reordering drag.
    fn stroke_sides_row(
        &mut self,
        ui: &mut egui::Ui,
        scope: PaintScope,
        index: usize,
        stroke: &Stroke,
    ) -> Option<Stroke> {
        let mut out = None;
        let mut sides = stroke.sides;
        let drawn_width = self.drawn_stroke_width(scope, stroke);
        // Inside `paint_unit` the parent's spacing is zero, and a scope's spacing
        // only goes between its own items.
        ui.add_space(ui::CARD_ROW_GAP);
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
            ui.spacing_mut().interact_size.y = 28.0;
            ui.spacing_mut().button_padding.y = 0.0;
            let combo = egui::ComboBox::from_id_salt(("stroke-sides", index))
                .icon(ui::combo_chevron)
                .width(ui.available_width())
                .selected_text(ui::glyph_and_text(
                    icon::SQUARE_SPLIT_HORIZONTAL,
                    sides_label(sides),
                ));
            combo.show_ui(ui, |ui| {
                ui::menu_rows(ui);
                ui.spacing_mut().button_padding.y = 2.0;
                for option in [
                    StrokeSides::All,
                    StrokeSides::Top,
                    StrokeSides::Right,
                    StrokeSides::Bottom,
                    StrokeSides::Left,
                    // Seeded from what the stroke draws, so picking Custom changes nothing.
                    stroke.sides.as_custom(drawn_width),
                ] {
                    // By label, since the Custom entry carries live widths.
                    let lit = sides_label(sides) == sides_label(option);
                    if ui
                        .selectable_label(lit, sides_label(option))
                        .on_hover_text(sides_hint(option))
                        .clicked()
                    {
                        sides = option;
                    }
                }
            });
        });
        if sides != stroke.sides {
            // Made `Custom`, a bound weight detaches in the edit, at the width it drew.
            let custom = matches!(sides, StrokeSides::Custom { .. });
            out = Some(Stroke {
                sides,
                width: if custom { drawn_width } else { stroke.width },
                width_var: if custom { None } else { stroke.width_var },
                ..stroke.clone()
            });
        }
        // The four independent widths; `0` switches a side off.
        if let StrokeSides::Custom {
            top,
            right,
            bottom,
            left,
        } = sides
        {
            let mut values = [top, right, bottom, left];
            // Outside the scope above, so the gap and pitch are owed again.
            ui.add_space(ui::CARD_ROW_GAP);
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
                for pair in [
                    [(0_usize, icon::ARROW_UP), (1, icon::ARROW_RIGHT)],
                    [(3_usize, icon::ARROW_LEFT), (2, icon::ARROW_DOWN)],
                ] {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                        let w = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
                        for (slot, glyph) in pair {
                            let mut v = values[slot];
                            let resp = value_field(
                                ui,
                                egui::vec2(w, 28.0),
                                Prefix::Icon(glyph),
                                &mut v,
                                Scrub::whole(0.5).range(0.0..=ui::MAX_STROKE_WIDTH),
                                |d| d.custom_formatter(ui::number(2)),
                            );
                            values[slot] = v.max(0.0);
                            let [t, r, b, l] = values;
                            self.stroke_valve(&resp, scope, index, move |s| {
                                s.sides = StrokeSides::Custom {
                                    top: t,
                                    right: r,
                                    bottom: b,
                                    left: l,
                                };
                            });
                        }
                    });
                }
            });
        }
        out
    }
    /// Drop every piece of Stroke-panel chrome keyed by list index.
    ///
    /// For a change of subject only; a reorder or removal uses
    /// [`Self::remap_paint_indices`] so the chrome follows its stroke.
    fn forget_stroke_chrome(&mut self) {
        self.stroke_menu = None;
        self.stroke_sides_open.clear();
        self.stroke_dash_text = None;
    }
    /// Renumber everything that names a paint by its index in `list`: the picker,
    /// the hex buffer, and for strokes the options popup, sides rows and dash
    /// buffer. `map` answers "where did the paint at this index go", `None` for
    /// one that has gone. Slots in the other list are left alone.
    fn remap_paint_indices(
        &mut self,
        anchor: NodeId,
        list: PaintList,
        map: impl Fn(usize) -> Option<usize>,
    ) {
        let moved = |slot: PaintSlot| -> Option<PaintSlot> {
            match (slot, list) {
                (PaintSlot::Fill(i), PaintList::Fill) => map(i).map(PaintSlot::Fill),
                (PaintSlot::Stroke(i), PaintList::Stroke) => map(i).map(PaintSlot::Stroke),
                (PaintSlot::Selection(t, i), l) if t == l.target() => {
                    map(i).map(|i| PaintSlot::Selection(t, i))
                }
                (untouched, _) => Some(untouched),
            }
        };
        self.remap_slots(anchor, moved);
        if list == PaintList::Stroke {
            self.stroke_menu = self.stroke_menu.and_then(&map);
            self.stroke_sides_open = self
                .stroke_sides_open
                .iter()
                .filter_map(|i| map(*i))
                .collect();
            self.stroke_dash_text = self
                .stroke_dash_text
                .take()
                .and_then(|(i, text)| map(i).map(|i| (i, text)));
        }
    }
    /// Whether a per-kind stroke rule holds for everything the panel is writing to.
    ///
    /// Over a selection, only when every target can honour it.
    fn stroke_rule(&self, scope: PaintScope, applies: fn(&NodeKind) -> bool) -> bool {
        match scope {
            PaintScope::Node(id) => applies(
                self.session
                    .display_node(id)
                    .map(|n| n.kind())
                    .unwrap_or(&NodeKind::Group),
            ),
            PaintScope::Selection => {
                let targets =
                    ondin_core::paint_targets(&self.session.doc, self.session.selection.ids());
                !targets.is_empty()
                    && targets.iter().all(|id| {
                        self.session
                            .display_node(*id)
                            .is_some_and(|n| applies(n.kind()))
                    })
            }
        }
    }
    /// Build the transaction that writes `strokes` over whatever the panel is
    /// showing — one node, or every paint target in the selection.
    ///
    /// Nothing when the committed list already says this (not the display one,
    /// which mid-drag carries the value). Over a selection, `strokes` is
    /// [`Self::strokes_in_scope`]'s anchor after the change, retargeted per target.
    fn strokes_tx(&mut self, scope: PaintScope, strokes: Vec<Keyed<Stroke>>) -> Transaction {
        match scope {
            PaintScope::Node(id) => {
                let committed = self
                    .session
                    .doc
                    .get(id)
                    .map(|n| n.paint().strokes.as_slice());
                if committed == Some(strokes.as_slice()) {
                    return Transaction(Vec::new());
                }
                Transaction(vec![Operation::SetStrokes { id, strokes }])
            }
            PaintScope::Selection => {
                let anchor = self.strokes_in_scope(PaintScope::Selection);
                ondin_core::retarget_strokes_all(
                    &self.session.doc,
                    self.session.selection.ids(),
                    &anchor,
                    &strokes,
                    &mut self.session.ids,
                )
            }
        }
    }
    /// The width `stroke` draws at: a bound one's number on a layer's own strokes,
    /// its literal otherwise (a selection's list is resolved already).
    fn drawn_stroke_width(&self, scope: PaintScope, stroke: &Stroke) -> f64 {
        let (PaintScope::Node(id), Some(var)) = (scope, stroke.width_var) else {
            return stroke.width;
        };
        let doc = &self.session.doc;
        match doc.get(id) {
            Some(_) => doc.stroke_width_drawn_in(doc.variables(), id, var),
            None => stroke.width,
        }
    }
    /// Put a scrubbed number on stroke `index` through the edit valve.
    ///
    /// Every draggable stroke number goes through here rather than the row's `out`,
    /// so a drag previews every frame.
    fn stroke_valve(
        &mut self,
        resp: &egui::Response,
        scope: PaintScope,
        index: usize,
        edit: impl FnOnce(&mut Stroke),
    ) {
        let mut next = self.strokes_in_scope(scope);
        if let Some(slot) = next.get_mut(index) {
            edit(slot);
        }
        let tx = self.strokes_tx(scope, next);
        self.edit_valve(resp, tx);
    }
    /// One stroke's dash, cap and join, behind the sliders button.
    ///
    /// An `Area`, so opening it does not push the strokes below down.
    fn stroke_menu_popup(
        &mut self,
        ui: &mut egui::Ui,
        head: &egui::Response,
        scope: PaintScope,
        index: usize,
        s: &Stroke,
        joins: bool,
    ) -> Option<Stroke> {
        // Every popover's width, which the picker's clearance also reads.
        const MENU_W: f32 = crate::app::POPOVER_W;
        const MENU_PAD: f32 = 11.0;
        /// One control height for all the chrome.
        const CELL: f32 = ui::CONTROL_H;
        if self.stroke_menu != Some(index) {
            return None;
        }
        let mut out = None;
        let mut close = false;
        let ctx = ui.ctx().clone();
        // So a click in the cap combo's own list does not read as a click outside;
        // asked by layer order, since the combo has already closed itself.
        let over_overlay = ctx
            .pointer_interact_pos()
            .and_then(|p| ctx.layer_id_at(p))
            .is_some_and(|l| l.order >= egui::Order::Foreground);
        let anchor = Self::popover_anchor(&ctx, head.rect, MENU_W);
        let drawn_width = self.drawn_stroke_width(scope, s);
        let locked = !ui.is_enabled();
        let menu = egui::Area::new(egui::Id::new(("stroke-menu", index)))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            // A low stroke row could otherwise hang the popover off the bottom.
            .constrain_to(ctx.input(|i| i.content_rect()).shrink(12.0))
            .show(&ctx, |ui| {
                ui::menu_frame(MENU_PAD).show(ui, |ui| {
                    if locked {
                        super::styles::lock(ui);
                    }
                    let inner = ui::menu_inner_w(MENU_W, MENU_PAD);
                    ui.set_width(inner);
                    ui.spacing_mut().item_spacing.y = ui::POPOVER_SECTION_GAP;

                    // --- which kind of pattern ---------------------------------
                    let style = s.dash_style();
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                        let tabs = inner - 18.0 - ui::CARD_COL_GAP;
                        let current = DashStyle::ALL.iter().position(|d| *d == style).unwrap_or(0);
                        if let Some(i) = segmented(
                            ui,
                            tabs,
                            ui::SEGMENT_CELL_H,
                            4,
                            current,
                            |p, i, rect, on| segment_label(p, rect, DashStyle::ALL[i].label(), on),
                        ) {
                            let sized = Stroke {
                                width: drawn_width,
                                ..s.clone()
                            };
                            out = Some(Stroke {
                                dashes: pattern_for(DashStyle::ALL[i], &sized),
                                ..s.clone()
                            });
                            self.stroke_dash_text = None;
                        }
                        if icon_button(ui, icon::X, 18.0, 13.0, false, true)
                            .on_hover_text("Close")
                            .clicked()
                        {
                            close = true;
                        }
                    });

                    // --- the numbers this kind of pattern needs ----------------
                    match style {
                        DashStyle::Solid => {}
                        DashStyle::Dotted => {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                                let mut spacing = s.dot_spacing();
                                let resp = value_field(
                                    ui,
                                    egui::vec2(inner - CELL - ui::CARD_COL_GAP, CELL),
                                    Prefix::Icon(icon::ARROWS_LEFT_RIGHT),
                                    &mut spacing,
                                    Scrub::whole(0.5).range(0.0..=ui::MAX_STROKE_WIDTH),
                                    |d| d.custom_formatter(ui::number(2)),
                                )
                                .on_hover_text("Centre-to-centre spacing");
                                // A dot is a zero-length dash, so the gap is centre-to-centre.
                                self.stroke_valve(&resp, scope, index, move |st| {
                                    st.dashes = vec![0.0, spacing.max(0.0)];
                                });
                                if let Some(st) = self.dash_fit_button(ui, CELL, s) {
                                    out = Some(st);
                                }
                            });
                        }
                        DashStyle::Dashed => {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                                let (mut length, mut spacing) = s.dash_length_spacing();
                                let fw = (inner - CELL - ui::CARD_COL_GAP * 2.0) / 2.0;
                                let rl = value_field(
                                    ui,
                                    egui::vec2(fw, CELL),
                                    Prefix::Icon(icon::RULER),
                                    &mut length,
                                    Scrub::whole(0.5).range(0.0..=ui::MAX_STROKE_WIDTH),
                                    |d| d.custom_formatter(ui::number(2)),
                                )
                                .on_hover_text("Dash length");
                                let held = spacing;
                                self.stroke_valve(&rl, scope, index, move |st| {
                                    st.dashes = vec![length.max(0.0), held];
                                });
                                let rs = value_field(
                                    ui,
                                    egui::vec2(fw, CELL),
                                    Prefix::Icon(icon::ARROWS_LEFT_RIGHT),
                                    &mut spacing,
                                    Scrub::whole(0.5).range(0.0..=ui::MAX_STROKE_WIDTH),
                                    |d| d.custom_formatter(ui::number(2)),
                                )
                                .on_hover_text("Gap between dashes");
                                self.stroke_valve(&rs, scope, index, move |st| {
                                    st.dashes = vec![length.max(0.0), spacing.max(0.0)];
                                });
                                if let Some(st) = self.dash_fit_button(ui, CELL, s) {
                                    out = Some(st);
                                }
                            });
                        }
                        DashStyle::Custom => {
                            if let Some(st) = self.dash_list_field(ui, inner, CELL, index, s) {
                                out = Some(st);
                            }
                        }
                    }
                    // Where along the path the pattern starts, for every patterned style;
                    // signed, as SVG's `stroke-dashoffset` is.
                    if style != DashStyle::Solid {
                        let mut offset = s.dash_offset;
                        let resp = value_field(
                            ui,
                            egui::vec2(inner, CELL),
                            Prefix::Icon(icon::ARROW_LINE_RIGHT),
                            &mut offset,
                            Scrub::whole(0.5).range(-ui::MAX_STROKE_WIDTH..=ui::MAX_STROKE_WIDTH),
                            |d| d.custom_formatter(ui::number(2)),
                        )
                        .on_hover_text("Dash offset — slides the pattern along the path");
                        self.stroke_valve(&resp, scope, index, move |st| {
                            st.dash_offset = offset;
                        });
                    }

                    // --- the ends and the corners, common to all four ----------
                    ui.scope(|ui| {
                        ui.spacing_mut().interact_size.y = CELL;
                        ui.spacing_mut().button_padding.y = 0.0;
                        let mut cap = s.cap;
                        egui::ComboBox::from_id_salt(("stroke-cap", index))
                            .icon(ui::combo_chevron)
                            .width(inner)
                            .selected_text(ui::glyph_and_text(icon::LINE_SEGMENT, cap_label(cap)))
                            .show_ui(ui, |ui| {
                                ui::menu_rows(ui);
                                ui.spacing_mut().button_padding.y = 2.0;
                                for c in [Cap::Butt, Cap::Round, Cap::Square] {
                                    ui.selectable_value(&mut cap, c, cap_label(c));
                                }
                            });
                        if cap != s.cap {
                            out = Some(Stroke { cap, ..s.clone() });
                        }
                    });

                    let join_row = ui.scope(|ui| {
                        // The label's gap; the miter field is topped up to `CARD_ROW_GAP` below.
                        ui.spacing_mut().item_spacing.y = ui::SECTION_LABEL_GAP;
                        // `Ui::disable` also fades `segmented`, which paints straight
                        // onto the painter.
                        if !joins {
                            ui.disable();
                        }
                        let joins_all = [Join::Miter, Join::Round, Join::Bevel];
                        let current = joins_all.iter().position(|j| *j == s.join).unwrap_or(0);
                        ui.label(ui::eyebrow("Join"));
                        if let Some(i) = segmented(
                            ui,
                            inner,
                            ui::SEGMENT_CELL_H,
                            3,
                            current,
                            |p, i, rect, on| segment_label(p, rect, join_label(joins_all[i]), on),
                        ) {
                            out = Some(Stroke {
                                join: joins_all[i],
                                ..s.clone()
                            });
                        }
                        // Only under Miter, and only where a join means anything: `Miter`
                        // is the default, so without the `joins` test an ellipse would
                        // show a dimmed field that looks usable.
                        if joins && s.join == Join::Miter {
                            // The scope carries the label's gap; this tops up to the
                            // Transform card's row pitch (§15 D386).
                            ui.add_space(ui::CARD_ROW_GAP - ui::SECTION_LABEL_GAP);
                            let shown = ondin_core::miter_degrees_of_ratio(s.miter_limit);
                            let mut degrees = shown;
                            let resp = value_field(
                                ui,
                                egui::vec2(inner, CELL),
                                Prefix::Icon(icon::ANGLE),
                                &mut degrees,
                                Scrub::whole(0.25).range(ondin_core::MIN_MITER_DEGREES..=180.0),
                                |d| d.suffix("°").custom_formatter(ui::number(2)),
                            )
                            // A smaller angle permits a longer spike; 180° bevels everything.
                            .on_hover_text("Corners sharper than this are bevelled");
                            // Stored as the ratio kurbo and SVG take. Written only when
                            // the angle moved: the `asin`/`sin` round trip is not
                            // bit-exact, so converting unconditionally would write a
                            // ratio a few ulps off on every defocus, past `strokes_tx`'s
                            // equality guard and into the undo stack.
                            self.stroke_valve(&resp, scope, index, move |st| {
                                if degrees != shown {
                                    st.miter_limit = ondin_core::miter_ratio_of_degrees(degrees);
                                }
                            });
                        }
                    });
                    // Through a claimant of its own: a `Ui::scope`'s response never
                    // reports a hover, and `on_hover_text` is gated on `enabled()`,
                    // false inside a disabled scope.
                    if !joins {
                        ui.interact(
                            join_row.response.rect,
                            ui.id().with("dimmed-join-row"),
                            egui::Sense::empty(),
                        )
                        .on_hover_text("Only a shape with corners takes a join");
                    }
                });
            })
            .response;
        // `on_picker` matters: a stroke row's swatch opens the detached picker, an
        // `egui::Window` at `Order::Middle` — below this `Area`, so the overlay term
        // says no, and a separate layer, so `on_menu` says no too.
        let on_picker = ctx
            .pointer_interact_pos()
            .and_then(|p| ctx.layer_id_at(p))
            .is_some_and(|l| l.id == super::picker::layer_id());
        // Recorded every frame, never from inside the short-circuiting `&&`.
        let press_away = super::press_began_away(&ctx, menu.layer_id.id, menu.rect, head.rect);
        if close
            || super::dismissed_by_click(super::ClickAway {
                // The primary button only: this popover holds scrubbable fields, and a
                // scrub is cancelled with a right-click, which must not also dismiss.
                clicked: ctx.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary)),
                over_overlay,
                on_menu: menu.contains_pointer(),
                on_head: head.contains_pointer(),
                on_picker,
                in_gesture: ctx.dragged_id().is_some() || self.gesture_cancelled,
                press_away,
            })
        {
            self.stroke_menu = None;
            self.stroke_dash_text = None;
        }
        // One Escape closes the popover and the cap list with it. Heard, for the
        // router's sake (§15 D847, `OndinApp::popover_heard`).
        self.popover_heard = true;
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.stroke_menu = None;
            self.stroke_dash_text = None;
        }
        out
    }
    /// The fit-to-corners toggle, shared by the dotted and dashed rows.
    ///
    /// A flag honoured at the render boundary, so it stays true across a resize
    /// and the fields keep showing what was typed (§6.3).
    fn dash_fit_button(&mut self, ui: &mut egui::Ui, cell: f32, s: &Stroke) -> Option<Stroke> {
        let resp = ui::field_button(
            ui,
            icon::BOUNDING_BOX,
            cell,
            14.0,
            ui::FieldButton::on_if(s.dash_fit),
        )
        .on_hover_text("Stretch the pattern so it lands on the corners");
        resp.clicked().then(|| Stroke {
            dash_fit: !s.dash_fit,
            ..s.clone()
        })
    }
    /// The custom dash list: comma-separated lengths read as on, off, on, off…
    ///
    /// Buffered and applied on defocus, or each keystroke would be an undo step.
    /// An unparseable list leaves the model alone rather than turning the stroke solid.
    fn dash_list_field(
        &mut self,
        ui: &mut egui::Ui,
        width: f32,
        cell: f32,
        index: usize,
        s: &Stroke,
    ) -> Option<Stroke> {
        let editing = matches!(&self.stroke_dash_text, Some((i, _)) if *i == index);
        let mut text = match &self.stroke_dash_text {
            Some((i, t)) if *i == index => t.clone(),
            _ => dash_list_text(&s.dashes),
        };
        let mut out = None;
        let resp = ui::field_row(ui, egui::vec2(width, cell), |ui| {
            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
            ui.label(theme::icon_text(
                icon::LIST_DASHES,
                14.0,
                theme::text::FAINT,
            ));
            ui.add(
                egui::TextEdit::singleline(&mut text)
                    .id(egui::Id::new(("stroke-dash-list", index)))
                    .desired_width(ui.available_width())
                    .frame(egui::Frame::NONE)
                    .hint_text("5, 10")
                    .font(egui::FontId::proportional(11.5)),
            )
        });
        // valve-gate-not-a-commit: a TextEdit; changed() only stashes, lost_focus() parses.
        if resp.gained_focus() || resp.changed() {
            self.stroke_dash_text = Some((index, text.clone()));
        }
        if editing && resp.lost_focus() {
            self.stroke_dash_text = None;
            // Compared against the printed text: the print rounds, so a bare click
            // would rewrite an imported `5.0000000001` to `5`.
            if let Some(dashes) = parse_dash_list(&text)
                && text != dash_list_text(&s.dashes)
                && ui::defocus_commits(&resp)
            {
                out = Some(Stroke {
                    dashes,
                    ..s.clone()
                });
            }
        }
        resp.on_hover_text("Lengths alternating ink and gap, e.g. 5, 10, 15");
        out
    }
    /// Which entry of `list` this panel is currently carrying, if any.
    ///
    /// Keyed on the anchor too: a drag whose selection changed carries nothing.
    fn carrying(&self, list: PaintList, anchor: NodeId) -> Option<usize> {
        self.in_flight
            .paint_drag
            .as_ref()
            .filter(|d| d.list == list && d.anchor == anchor)
            .map(|d| d.from)
    }
    /// Advance a paint-row drag against the slots the panel laid out last frame,
    /// and return the model indices to draw the rows in.
    ///
    /// Runs before the rows are drawn, so the slot geometry is last frame's —
    /// exact, since reordering contents does not move the slots.
    fn paint_order(
        &mut self,
        ui: &egui::Ui,
        list: PaintList,
        anchor: NodeId,
        n: usize,
    ) -> Vec<usize> {
        let y = ui.ctx().input(|i| i.pointer.interact_pos()).map(|p| p.y);
        let Some(drag) = self
            .in_flight
            .paint_drag
            .as_mut()
            .filter(|d| d.list == list && d.anchor == anchor)
        else {
            return paint::display_order(n, None);
        };
        if let Some(y) = y
            && !drag.slots.is_empty()
            && n > 0
        {
            let shown_at = n - 1 - drag.to.min(n - 1);
            drag.to = paint::landing(n, shown_at, drag.slots.gap(y));
        }
        paint::display_order(n, Some((drag.from, drag.to)))
    }
    /// Record the slots the rows just landed in, and resolve the drag on release
    /// into `(from, to)` model indices.
    ///
    /// `None` except on the release frame, and then only if the paint moved. The
    /// release is read from the pointer: a carried row's `Response`s are empty
    /// ([`carried_unit`]), so `drag_stopped` never arrives.
    fn finish_paint_drag(
        &mut self,
        ui: &egui::Ui,
        list: PaintList,
        anchor: NodeId,
        slots: paint::Slots,
    ) -> Option<(usize, usize)> {
        {
            let drag = self
                .in_flight
                .paint_drag
                .as_mut()
                .filter(|d| d.list == list && d.anchor == anchor)?;
            drag.slots = slots;
        }
        if ui.ctx().input(|i| i.pointer.primary_down()) {
            ui.ctx().request_repaint();
            return None;
        }
        let drag = self.in_flight.paint_drag.take()?;
        (drag.to != drag.from).then_some((drag.from, drag.to))
    }
    /// [`Self::paint_order`] for the Effects card, which draws its stack top-first.
    fn effect_order(&mut self, ui: &egui::Ui, anchor: NodeId, n: usize) -> Vec<usize> {
        let y = ui.ctx().input(|i| i.pointer.interact_pos()).map(|p| p.y);
        let Some(drag) = self
            .in_flight
            .effect_drag
            .as_mut()
            .filter(|d| d.anchor == anchor)
        else {
            return paint::forward_order(n, None);
        };
        if let Some(y) = y
            && !drag.slots.is_empty()
            && n > 0
        {
            let shown_at = drag.to.min(n - 1);
            drag.to = paint::forward_landing(n, shown_at, drag.slots.gap(y));
        }
        paint::forward_order(n, Some((drag.from, drag.to)))
    }
    /// [`Self::finish_paint_drag`] for the Effects card. A drag carried from another
    /// layer's card lands nothing and ends at the release, so it cannot stay in flight.
    fn finish_effect_drag(
        &mut self,
        ui: &egui::Ui,
        anchor: NodeId,
        slots: paint::Slots,
    ) -> Option<(usize, usize)> {
        self.in_flight.effect_drag.as_ref()?;
        if let Some(drag) = self
            .in_flight
            .effect_drag
            .as_mut()
            .filter(|d| d.anchor == anchor)
        {
            drag.slots = slots;
        }
        if ui.ctx().input(|i| i.pointer.primary_down()) {
            ui.ctx().request_repaint();
            return None;
        }
        let drag = self.in_flight.effect_drag.take()?;
        (drag.anchor == anchor && drag.to != drag.from).then_some((drag.from, drag.to))
    }
    /// Put a paint panel in the state its contents call for: closed with nothing in
    /// it, open with something.
    ///
    /// Asserted outright on a new subject and on the empty/non-empty transition
    /// otherwise, so a hand toggle within one selection wins.
    pub(super) fn sync_paint_collapse(&mut self, title: &'static str, empty: bool, fresh: bool) {
        sync_paint_collapse(
            &mut self.collapsed_panels,
            &mut self.paint_auto_collapsed,
            title,
            empty,
            fresh,
        );
    }
    /// Whether the paint panels are looking at a different subject than the last
    /// frame that drew them, recording this one.
    ///
    /// Asked once per frame above [`Self::inspector_paint_panels`] and handed down:
    /// a panel that consumed the edge would leave the next told nothing happened.
    fn paint_subject_changed(&mut self) -> bool {
        self.subject_changed("Fill")
    }

    /// Whether the panel `key` names is looking at a different subject than the
    /// last frame it asked, recording this one — one entry each in
    /// [`OndinApp::synced_selection`].
    pub(super) fn subject_changed(&mut self, key: &'static str) -> bool {
        let now = self.session.selection.ids();
        if self.synced_selection.get(key).is_some_and(|s| s == now) {
            return false;
        }
        self.synced_selection.insert(key, now.to_vec());
        true
    }
    /// Open a paint panel because a `+` just put something in it.
    pub(super) fn open_paint_panel(&mut self, title: &'static str) {
        open_paint_panel(
            &mut self.collapsed_panels,
            &mut self.paint_auto_collapsed,
            title,
        );
    }

    /// The frame templates — a panel of named sizes, each drawn at its own
    /// proportions, that fills in W and H.
    ///
    /// The scroll box is capped at [`TEMPLATE_LIST_H`]. The template matching
    /// [`Self::shared_frame_size`] lights, orientation-sensitively, so a landscape
    /// frame lights nothing rather than a template applying would rotate.
    fn inspector_frame_templates(&mut self, ui: &mut egui::Ui) {
        let subjects = self.frame_subjects();
        if subjects.is_empty() {
            return;
        }
        // A component part is never a page: any such frame hides the card.
        let doc = &self.session.doc;
        if subjects.iter().any(|id| {
            doc.get(*id)
                .is_some_and(|n| n.component() || n.set().is_some())
                || ondin_core::component::instance_root(doc, *id) == Some(*id)
        }) {
            return;
        }
        let size = self.shared_frame_size(&subjects);
        let current = size.and_then(preset_named);
        let mut pick: Option<Size> = None;
        self.panel(ui, "Frame templates", None, |_app, ui| {
            egui::ScrollArea::vertical()
                .max_height(TEMPLATE_LIST_H)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    // Measured outside the loop: inside a `horizontal` it shrinks per cell.
                    let cell = (ui.available_width() - TEMPLATE_CELL_GAP) / 2.0;
                    for (gi, (group, entries)) in FRAME_PRESETS.iter().enumerate() {
                        if gi > 0 {
                            ui.add_space(ui::CARD_ROW_GAP);
                        }
                        ui.label(ui::eyebrow(group));
                        ui.add_space(ui::SECTION_LABEL_GAP);
                        for (ri, pair) in entries.chunks(2).enumerate() {
                            if ri > 0 {
                                ui.add_space(TEMPLATE_CELL_GAP);
                            }
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = TEMPLATE_CELL_GAP;
                                for (name, w, h) in pair {
                                    if template_cell(ui, cell, name, *w, *h, current == Some(*name))
                                    {
                                        pick = Some(Size::new(*w, *h));
                                    }
                                }
                            });
                        }
                    }
                });
        });
        // A click on the lit cell writes nothing, so it is not an empty undo step.
        if let Some(next) = pick.filter(|next| Some(*next) != size) {
            self.apply_frame_template(&subjects, next);
        }
    }

    /// Set every subject's own box to `size`, as one undoable step.
    fn apply_frame_template(&mut self, subjects: &[NodeId], size: Size) {
        let tx = Transaction(
            subjects
                .iter()
                .map(|id| Operation::SetGeometry {
                    id: *id,
                    geometry: GeometryPatch::Size(size),
                })
                .collect(),
        );
        if !tx.0.is_empty() {
            self.commit_edit(tx);
        }
    }

    /// The size every selected frame already is, or `None` where they differ, so
    /// nothing lights. Reads the committed document, so no template lights mid-drag.
    fn shared_frame_size(&self, subjects: &[NodeId]) -> Option<Size> {
        let mut sizes = subjects
            .iter()
            // Resolved: a bound width stores 0.
            .filter_map(|id| match self.session.doc.resolved_kind(*id).as_deref() {
                Some(NodeKind::Artboard { size, .. }) => Some(*size),
                _ => None,
            });
        let first = sizes.next()?;
        sizes.all(|s| s == first).then_some(first)
    }

    /// The frame's columns and rows (`ondin_core::layout`).
    ///
    /// A frame's panel though `Node::grids` is a layer's field: a grid is measured
    /// against an authored size, which only `NodeKind::Artboard` has. A whole-list
    /// edit, so every change is one undo step. Its colour goes through a
    /// [`PaintSlot`]; a grid is chrome, so it previews through [`OndinApp::preview_grid`].
    fn inspector_layout_grids(&mut self, ui: &mut egui::Ui) {
        let subjects = self.frame_subjects();
        if subjects.is_empty() {
            return;
        }
        let shared = self.shared_grids_keyed(&subjects);
        // Closed when empty; a disagreement counts as not empty, so "Mixed" is seen.
        let fresh = self.grids_subject_changed();
        let ghosts = match subjects.as_slice() {
            [one] => self.has_ghosts(Some(*one), |n| n.grids()),
            _ => false,
        };
        self.sync_paint_collapse(
            "Layout grid",
            shared.as_ref().is_some_and(Vec::is_empty) && !ghosts,
            fresh,
        );
        let mixed = shared.is_none();
        let action = Some(ui::HeadAction::new(
            icon::PLUS,
            match (subjects.len(), mixed) {
                (1, _) => "Add a layout grid",
                (_, false) => "Add a layout grid to every selected frame",
                (_, true) => "Replace every selected frame's layout grids with one",
            },
        ));
        // Collected, not committed in the loop, which later rows are still reading.
        let mut next: Option<Vec<Keyed<LayoutGrid>>> = None;
        let add = self.panel(ui, "Layout grid", action, |app, ui| {
            let Some(grids) = shared.clone() else {
                ui.label(
                    egui::RichText::new("Mixed layout grids. Add to override.")
                        .size(11.0)
                        .color(theme::text::FAINT),
                );
                return;
            };
            let source = match subjects.as_slice() {
                [one] => app.source_list(*one, |n| n.grids()),
                _ => None,
            };
            if grids.is_empty() && source.as_ref().is_none_or(Vec::is_empty) {
                ui.label(
                    egui::RichText::new("No layout grid")
                        .size(11.0)
                        .color(theme::text::FAINT),
                );
                return;
            }
            let states = source
                .as_ref()
                .map(|s| ondin_core::reset::item_states(s, &grids));
            let mut reset = None;
            let mut restore = false;
            for (index, grid) in grids.iter().enumerate() {
                let item = states.as_ref().map(|s| s[index]);
                let out = app.grid_rows(ui, &subjects, index, grid.value, item);
                if out.reset {
                    reset = Some(grid.id);
                }
                if out.remove {
                    let mut list = grids.clone();
                    list.remove(index);
                    // Before the write, so the picker and hex buffer leave the row.
                    app.remap_grid_slots(subjects[0], index);
                    next = Some(list);
                } else if let Some(set) = out.set {
                    let mut list = grids.clone();
                    list[index].value = set;
                    next = Some(list);
                }
                if index + 1 < grids.len() {
                    ui.add_space(GRID_ROW_GAP);
                }
            }
            for gone in source
                .as_deref()
                .map(|s| ondin_core::reset::removed_items(s, &grids))
                .unwrap_or_default()
            {
                ui.add_space(GRID_ROW_GAP);
                let label = format!("{} · {}", grid_axis_word(gone.axis), gone.count);
                if super::component::ghost_row(ui, super::component::GhostLead::Plain, &label) {
                    restore = true;
                }
            }
            // Written whole, not through `retarget_grids`: a restored grid keeps the main's id.
            let list = match (restore, reset, source.as_deref()) {
                (true, _, Some(src)) => Some(src.to_vec()),
                (false, Some(item), Some(src)) => {
                    Some(ondin_core::reset::reset_item(src, &grids, item))
                }
                _ => None,
            };
            if let (Some(grids), [id]) = (list, subjects.as_slice()) {
                app.commit_edit(Transaction(vec![Operation::SetLayoutGrids {
                    id: *id,
                    grids,
                }]));
                next = None;
            }
        });
        if let Some(grids) = next {
            let anchor = shared.clone().unwrap_or_default();
            self.retarget_grids(&subjects, &anchor, &grids);
        }
        if add {
            let values = shared.as_deref().map(ondin_core::item::values);
            match grid_add(values.as_deref()) {
                GridAdd::Append(seed) => self.append_grid(&subjects, seed),
                GridAdd::Write(seed) => self.write_grids(&subjects, &[seed]),
            }
            self.open_paint_panel("Layout grid");
        }
    }

    /// The Position card: where the selected layers are pinned inside their
    /// containers — a frame, or a group with a layout — as CSS insets and
    /// `margin: auto` (`ondin_core::container`).
    ///
    /// A pin diagram and four fields. Clicking a strut pins that edge at its current
    /// distance (`container::with_edge`), so a pin never moves anything. Applies to
    /// every [`Self::inset_subjects`] layer, each at its own distances.
    fn inspector_insets(&mut self, ui: &mut egui::Ui) {
        use ondin_core::container::Edge as Pin;
        let subjects = self.inset_subjects();
        let Some(&anchor) = subjects.first() else {
            return;
        };
        let Some(place) = self.inset_placement(anchor) else {
            return;
        };
        let insets = self
            .session
            .display_node(anchor)
            .map(|n| n.insets())
            .unwrap_or_default();
        let mixed = subjects
            .iter()
            .any(|id| self.session.display_node(*id).map(|n| n.insets()) != Some(insets));
        // Outside a layout every child is absolute, held at top and left when it has no insets.
        let laid = self
            .session
            .doc
            .get(anchor)
            .and_then(|n| n.parent())
            .and_then(|p| self.session.doc.get(p))
            .is_some_and(|p| p.display().is_some());
        // Only inside a layout, where a pin takes the layer out of the flow.
        let badge = (laid && !mixed && insets.is_authored()).then_some("Absolute");
        // Outside a layout an axis with no inset is shown held, without writing one:
        // an authored inset would take the layer out of a future layout.
        let held = (
            !laid && insets.left.is_none() && insets.right.is_none(),
            !laid && insets.top.is_none() && insets.bottom.is_none(),
        );
        self.panel_badged(ui, "Position", None, badge, |app, ui| {
            if mixed {
                ui.label(
                    egui::RichText::new("Pinned differently. A pin applies to every layer.")
                        .size(11.0)
                        .color(theme::text::FAINT),
                );
            }
            let full = ui.available_width();
            let gap = ui::CARD_COL_GAP;
            let side = ui::CONTROL_H;
            let field = egui::vec2((full - INSET_DIAGRAM_W - gap * 2.0) / 2.0, side);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                let diagram = egui::vec2(INSET_DIAGRAM_W, 2.0 * side + ui::CARD_ROW_GAP);
                if let Some(edge) = inset_diagram(ui, diagram, &insets, held) {
                    app.toggle_pin(&subjects, edge, inset_of(&insets, edge).is_none());
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
                    for pair in [[Pin::Left, Pin::Right], [Pin::Top, Pin::Bottom]] {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = gap;
                            for edge in pair {
                                let is_held = match edge {
                                    Pin::Left => held.0,
                                    Pin::Top => held.1,
                                    Pin::Right | Pin::Bottom => false,
                                };
                                app.inset_field(
                                    ui, &subjects, edge, field, &insets, &place, is_held,
                                );
                            }
                        });
                    }
                });
            });
            let button = egui::vec2((full - gap) / 2.0, side);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for horizontal in [true, false] {
                    let on = if horizontal {
                        insets.margin_auto.left && insets.margin_auto.right
                    } else {
                        insets.margin_auto.top && insets.margin_auto.bottom
                    };
                    let (glyph, tip) = match (horizontal, on) {
                        (true, false) => (
                            icon::ALIGN_CENTER_VERTICAL,
                            "Centre in the container horizontally",
                        ),
                        (true, true) => (icon::ALIGN_CENTER_VERTICAL, "Stop centring horizontally"),
                        (false, false) => (
                            icon::ALIGN_CENTER_HORIZONTAL,
                            "Centre in the container vertically",
                        ),
                        (false, true) => {
                            (icon::ALIGN_CENTER_HORIZONTAL, "Stop centring vertically")
                        }
                    };
                    let state = if on {
                        ui::FieldButton::On
                    } else {
                        ui::FieldButton::Off
                    };
                    let mark = app.centre_mark(&subjects, horizontal);
                    let (resp, reset) = ui::field_button_marked(
                        ui,
                        glyph,
                        button,
                        15.0,
                        state,
                        mark.as_ref().map(Mark::field),
                    );
                    let resp = tip_unless_marked(resp, &mark, tip);
                    if reset {
                        app.reset_marked(true, &mark);
                    } else if resp.clicked() {
                        app.toggle_centre(&subjects, horizontal, !on);
                    }
                }
            });
        });
    }

    /// The selected layers a container can pin: those whose parent is a frame or a
    /// group with a layout and whose kind takes insets (`container::takes_insets`).
    fn inset_subjects(&self) -> Vec<NodeId> {
        let doc = &self.session.doc;
        self.session
            .selection
            .ids()
            .iter()
            .copied()
            .filter(|id| {
                doc.get(*id).is_some_and(|n| {
                    ondin_core::container::takes_insets(n.kind())
                        && n.parent().and_then(|p| doc.get(p)).is_some_and(|p| {
                            matches!(p.kind(), NodeKind::Artboard { .. })
                                || (matches!(p.kind(), NodeKind::Group) && p.display().is_some())
                        })
                })
            })
            .collect()
    }

    /// Where `id` is drawn inside its container, all through the preview so the
    /// distances follow a drag in flight.
    fn inset_placement(&self, id: NodeId) -> Option<InsetPlacement> {
        let parent = self.session.doc.get(id)?.parent()?;
        let frame = match *self.session.display_node(parent)?.kind() {
            NodeKind::Artboard { size } => size,
            NodeKind::Group => self.session.preview_frame(parent)?,
            _ => return None,
        };
        let local = self.session.preview_local_transform(id)?;
        let kind = self.session.display_node(id)?.kind().clone();
        let bx = ondin_core::geometry::local_bounds(&kind, self.session.preview_text_layout(id))?;
        Some(InsetPlacement {
            frame,
            local,
            bx,
            kind,
        })
    }

    /// Pin or unpin `edge` on every subject, each at its own current distance, as
    /// one commit.
    fn toggle_pin(&mut self, subjects: &[NodeId], edge: ondin_core::container::Edge, pin: bool) {
        let ops = subjects
            .iter()
            .filter_map(|id| {
                let p = self.inset_placement(*id)?;
                let was = *self.session.doc.get(*id)?.insets();
                let insets = ondin_core::container::with_edge(
                    &was, edge, pin, p.frame, p.local, p.bx, &p.kind,
                );
                let mut ops = self.baked_placement(*id, &p);
                ops.push(Operation::SetInsets { id: *id, insets });
                Some(ops)
            })
            .flatten()
            .collect();
        let ops = self.kept_in_place(ops, subjects);
        self.commit_edit(Transaction(ops));
    }

    /// `ops` — a pin or a centre from the card — with every subject it takes out of
    /// its container's flow re-pinned where it is drawn after the container re-lays
    /// without it, so a pin never moves anything even in a hugging container.
    ///
    /// Applies the edit to a scratch copy, resolves it, and re-derives each such
    /// layer's insets (`container::inverse`). One pass is exact: a layer out of the
    /// flow does not size its container.
    fn kept_in_place(&self, mut ops: Vec<Operation>, subjects: &[NodeId]) -> Vec<Operation> {
        let doc = &self.session.doc;
        let res = &self.session.resolved;
        let leaving: Vec<NodeId> = subjects
            .iter()
            .copied()
            .filter(|id| ondin_core::build::is_flex_item(doc, *id))
            .collect();
        if leaving.is_empty() {
            return ops;
        }
        let mut after = doc.clone();
        if after.apply(&Transaction(ops.clone())).is_err() {
            return ops;
        }
        let res_after = ondin_core::Resolved::rebuild(&after);
        for id in leaving {
            let Some(node) = after.get(id) else { continue };
            if !node.insets().is_authored() {
                continue;
            }
            let Some(parent) = node.parent() else {
                continue;
            };
            let frame = match res_after.used_kind(&after, parent) {
                Some(NodeKind::Artboard { size }) => *size,
                Some(NodeKind::Group) => match res_after.used_frame(parent) {
                    Some(size) => size,
                    None => continue,
                },
                _ => continue,
            };
            let (Some(world), Some(parent_world), Some(kind)) = (
                res.world_transform(id),
                res_after.world_transform(parent),
                res.used_kind(doc, id).cloned(),
            ) else {
                continue;
            };
            let Some(bx) = ondin_core::geometry::local_bounds(&kind, None) else {
                continue;
            };
            let local = parent_world.inverse() * world;
            let insets = ondin_core::container::inverse(node.insets(), frame, local, bx, &kind);
            let mut placed = false;
            for op in ops.iter_mut() {
                match op {
                    Operation::SetInsets { id: i, insets: to } if *i == id => *to = insets,
                    Operation::SetTransform { id: i, transform } if *i == id => {
                        *transform = local;
                        placed = true;
                    }
                    _ => {}
                }
            }
            if !placed {
                ops.push(Operation::SetTransform {
                    id,
                    transform: local,
                });
            }
        }
        ops
    }

    /// The ops that write where `id` is drawn now into its stored transform and
    /// size — so a pin change never moves it. This makes unpinning safe: an axis
    /// that loses its insets falls back to the stored transform, which a frame
    /// resize since the pin may have left stale.
    fn baked_placement(&self, id: NodeId, p: &InsetPlacement) -> Vec<Operation> {
        match self.session.doc.get(id) {
            Some(node) => build::baked_ops(id, node, p.local, &p.kind),
            None => Vec::new(),
        }
    }

    /// The override mark on a centre button: an instance's layer centred where its
    /// main's is not, or the reverse. Reset as the whole axis, since auto margins
    /// between edges not both pinned centre nothing.
    fn centre_mark(&self, subjects: &[NodeId], horizontal: bool) -> Option<Mark> {
        use ondin_core::Insets;
        // Equal when both are centred or neither is, whatever the distances.
        #[derive(Clone)]
        struct Axis(Insets, bool);
        impl PartialEq for Axis {
            fn eq(&self, other: &Self) -> bool {
                self.centred() == other.centred()
            }
        }
        impl Axis {
            fn centred(&self) -> bool {
                let m = self.0.margin_auto;
                match self.1 {
                    true => m.left && m.right,
                    false => m.top && m.bottom,
                }
            }
        }
        self.sub_mark(
            subjects,
            |op| match op {
                Operation::SetInsets { insets, .. } => Some(*insets),
                _ => None,
            },
            |n| Some(*n.insets()),
            move |i| Axis(*i, horizontal),
            |i, Axis(from, h)| {
                if h {
                    i.left = from.left;
                    i.right = from.right;
                    i.margin_auto.left = from.margin_auto.left;
                    i.margin_auto.right = from.margin_auto.right;
                } else {
                    i.top = from.top;
                    i.bottom = from.bottom;
                    i.margin_auto.top = from.margin_auto.top;
                    i.margin_auto.bottom = from.margin_auto.bottom;
                }
            },
            |id, insets| Operation::SetInsets { id, insets },
            |a| match a.centred() {
                true => "centred".to_owned(),
                false => "not centred".to_owned(),
            },
        )
    }

    /// Centre every subject on an axis the CSS way — both insets, each at its
    /// current distance, and auto margins between them — or stop, which unpins
    /// the axis's end edge and its margins. Neither moves anything.
    fn toggle_centre(&mut self, subjects: &[NodeId], horizontal: bool, on: bool) {
        use ondin_core::container::Edge as Pin;
        let (start, end) = match horizontal {
            true => (Pin::Left, Pin::Right),
            false => (Pin::Top, Pin::Bottom),
        };
        let ops = subjects
            .iter()
            .filter_map(|id| {
                let p = self.inset_placement(*id)?;
                let was = *self.session.doc.get(*id)?.insets();
                let with = |i: &ondin_core::Insets, e, pin| {
                    ondin_core::container::with_edge(i, e, pin, p.frame, p.local, p.bx, &p.kind)
                };
                let insets = if on {
                    let mut i = with(&with(&was, start, true), end, true);
                    if horizontal {
                        i.margin_auto.left = true;
                        i.margin_auto.right = true;
                    } else {
                        i.margin_auto.top = true;
                        i.margin_auto.bottom = true;
                    }
                    i
                } else {
                    with(&was, end, false)
                };
                let mut ops = self.baked_placement(*id, &p);
                ops.push(Operation::SetInsets { id: *id, insets });
                Some(ops)
            })
            .flatten()
            .collect();
        let ops = self.kept_in_place(ops, subjects);
        self.commit_edit(Transaction(ops));
    }

    /// One inset's field: its value in its own unit when pinned, with the accent
    /// edge; `auto` when not — except an edge `held` by default, which shows the
    /// distance it is kept at.
    ///
    /// A click through an unpinned field pins nothing until the number differs. The
    /// unit suffix commits directly, not through the valve.
    #[allow(clippy::too_many_arguments)]
    fn inset_field(
        &mut self,
        ui: &mut egui::Ui,
        subjects: &[NodeId],
        edge: ondin_core::container::Edge,
        size: egui::Vec2,
        insets: &ondin_core::Insets,
        place: &InsetPlacement,
        held: bool,
    ) {
        use ondin_core::LengthPct;
        let set = inset_of(insets, edge);
        let distance = place.distance(edge);
        let shown = match set {
            Some(LengthPct::Px(v)) | Some(LengthPct::Percent(v)) => v,
            None => distance,
        };
        let mut v = shown;
        let unit = match set {
            Some(LengthPct::Px(_)) => Some(("px", "Switch to a percentage of the container")),
            Some(LengthPct::Percent(_)) => Some(("%", "Switch to px")),
            None => None,
        };
        let percent = matches!(set, Some(LengthPct::Percent(_)));
        let at = egui::Rect::from_min_size(ui.cursor().min, size);
        // Reads `auto` over the real distance, so a scrub starts from the edge.
        let unpinned = set.is_none() && !held;
        let mark = self.sub_mark(
            subjects,
            |op| match op {
                Operation::SetInsets { insets, .. } => Some(*insets),
                _ => None,
            },
            |n| Some(*n.insets()),
            move |i| inset_of(i, edge),
            move |i, v| {
                use ondin_core::container::Edge as Pin;
                *match edge {
                    Pin::Top => &mut i.top,
                    Pin::Right => &mut i.right,
                    Pin::Bottom => &mut i.bottom,
                    Pin::Left => &mut i.left,
                } = v;
            },
            |id, insets| Operation::SetInsets { id, insets },
            |v| match v {
                Some(LengthPct::Px(x)) => super::component::mark_num(*x),
                Some(LengthPct::Percent(x)) => format!("{}%", super::component::mark_num(*x)),
                None => "auto".to_owned(),
            },
        );
        let (resp, unit_resp, hit) = ui::value_field_unit_marked(
            ui,
            size,
            ui::Prefix::Text(inset_label(edge)),
            unit.map(|(text, tooltip)| ui::Suffix {
                text,
                clickable: true,
                tooltip,
            }),
            mark.as_ref().map(Mark::field),
            &mut v,
            if percent {
                ui::Scrub::fine(0.25, 1)
            } else {
                ui::Scrub::whole(0.5)
            },
            |d| {
                if unpinned {
                    d.custom_formatter(|_, _| "auto".into())
                } else {
                    d.custom_formatter(ui::number(usize::from(percent)))
                }
            },
        );
        let flip = unit_resp.is_some_and(|u| u.clicked());
        let resp = if unpinned {
            tip_unless_marked(
                resp,
                &mark,
                "Not pinned — type a value, drag, or click the diagram to pin",
            )
        } else if set.is_none() {
            tip_unless_marked(
                resp,
                &mark,
                "Held at this distance by default — nothing is pinned on this axis. \
                 Type or drag to pin it",
            )
        } else {
            // The pinned accent, painted after because the field's ground is opaque.
            ui.painter()
                .rect_filled(at, 5.0, color::ACCENT.gamma_multiply(0.09));
            ui.painter().rect_stroke(
                at,
                5.0,
                egui::Stroke::new(1.0, color::ACCENT.gamma_multiply(0.55)),
                egui::StrokeKind::Inside,
            );
            resp
        };
        // The valve is still told nothing moved, since it owns the engagement latch.
        if hit && let Some(m) = &mark {
            self.commit_reset(m.tx.clone());
            self.edit_valve(&resp, Transaction::default());
            return;
        }
        if flip && let Some(was) = set {
            let ops = self.inset_flip_ops(subjects, edge, matches!(was, LengthPct::Px(_)));
            self.commit_edit(Transaction(ops));
            return;
        }
        let ops = if set.is_none() && v == shown {
            Vec::new()
        } else {
            let value = if percent {
                LengthPct::Percent(v)
            } else {
                LengthPct::Px(v)
            };
            self.typed_inset_ops(subjects, edge, value)
        };
        self.edit_valve(&resp, Transaction(ops));
    }

    /// The unit suffix's edit: every subject's pinned `edge` converted to `%` of
    /// its container (`to_percent`) or to px, so nothing moves. A switch never
    /// makes a pin, so a subject with the edge unpinned is left out.
    fn inset_flip_ops(
        &self,
        subjects: &[NodeId],
        edge: ondin_core::container::Edge,
        to_percent: bool,
    ) -> Vec<Operation> {
        use ondin_core::LengthPct;
        subjects
            .iter()
            .filter_map(|id| {
                let mut i = *self.session.doc.get(*id)?.insets();
                let own = inset_of(&i, edge)?;
                let extent = self.inset_placement(*id)?.extent(edge);
                let px = own.resolve(extent);
                *inset_mut(&mut i, edge) = Some(if to_percent {
                    LengthPct::Percent(0.0).rewritten(px, extent)
                } else {
                    LengthPct::Px(px)
                });
                Some(Operation::SetInsets { id: *id, insets: i })
            })
            .collect()
    }

    /// A typed or dragged inset's edit: `edge` pinned at `value` on every subject.
    ///
    /// An in-flow item this pins leaves its flow where it is drawn, so its drawn
    /// placement is written first (the unpinned axis would fall back to the stored
    /// transform).
    fn typed_inset_ops(
        &self,
        subjects: &[NodeId],
        edge: ondin_core::container::Edge,
        value: ondin_core::LengthPct,
    ) -> Vec<Operation> {
        subjects
            .iter()
            .filter_map(|id| {
                let mut i = self.session.display_node(*id)?.insets();
                *inset_mut(&mut i, edge) = Some(value);
                let mut ops = if ondin_core::build::is_flex_item(&self.session.doc, *id) {
                    self.inset_placement(*id)
                        .map(|p| self.baked_placement(*id, &p))
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                ops.push(Operation::SetInsets { id: *id, insets: i });
                Some(ops)
            })
            .flatten()
            .collect()
    }

    /// The frames a frame panel is acting on: the selected artboards, in document
    /// order; empty gates the panel. `pub(crate)` for `picker_ui`'s orphan match.
    pub(crate) fn frame_subjects(&self) -> Vec<NodeId> {
        ondin_core::build::in_document_order(&self.session.doc, self.session.selection.ids())
            .into_iter()
            .filter(|id| {
                matches!(
                    self.session.doc.get(*id).map(|n| n.kind()),
                    Some(NodeKind::Artboard { .. })
                )
            })
            .collect()
    }

    /// Whether the selection has moved since the Layout grid panel last read it —
    /// the edge `sync_paint_collapse` re-asserts the resting state on.
    fn grids_subject_changed(&mut self) -> bool {
        self.subject_changed("Layout grid")
    }

    /// The grid list every subject agrees on, or `None` where they disagree.
    ///
    /// The committed document: `SetLayoutGrids` has no preview, so a scrub's value
    /// lives in `OndinApp::grid_scrub`. `pub(crate)` for `picker_ui`'s orphan match.
    pub(crate) fn shared_grids(&self, subjects: &[NodeId]) -> Option<Vec<LayoutGrid>> {
        self.shared_grids_keyed(subjects)
            .map(|l| ondin_core::item::values(&l))
    }

    /// [`Self::shared_grids`] with the first subject's item ids — the anchor a row
    /// edit is retargeted from. Agreement is by value: two frames given one grid
    /// separately carry different ids.
    fn shared_grids_keyed(&self, subjects: &[NodeId]) -> Option<Vec<Keyed<LayoutGrid>>> {
        let mut nodes = subjects.iter().filter_map(|id| self.session.doc.get(*id));
        let first = nodes.next()?.grids().to_vec();
        nodes
            .all(|n| ondin_core::same_values(n.grids(), &first))
            .then_some(first)
    }

    /// A row **edit** of the list every subject shares, onto each subject's own
    /// item ids (`ondin_core::item::retarget`), as one step.
    fn retarget_grids(
        &mut self,
        subjects: &[NodeId],
        anchor: &[Keyed<LayoutGrid>],
        edited: &[Keyed<LayoutGrid>],
    ) {
        let session = &mut self.session;
        let ops: Vec<Operation> = subjects
            .iter()
            .filter_map(|id| {
                let own = session.doc.get(*id)?.grids();
                let grids = ondin_core::item::retarget(anchor, own, edited, &mut session.ids);
                (grids != own).then_some(Operation::SetLayoutGrids { id: *id, grids })
            })
            .collect();
        if !ops.is_empty() {
            self.commit_edit(Transaction(ops));
        }
    }

    /// Write one list to every subject, as a single undoable step.
    ///
    /// A wholesale replacement, so each subject keeps its own item ids by position
    /// (`ondin_core::item::rekey_by_position`); a row edit goes through
    /// [`Self::retarget_grids`].
    fn write_grids(&mut self, subjects: &[NodeId], grids: &[LayoutGrid]) {
        let session = &mut self.session;
        let mut fresh = ondin_core::item::PastTheEnd::default();
        let tx = Transaction(
            subjects
                .iter()
                .map(|id| {
                    let own = session.doc.get(*id).map(|n| n.grids()).unwrap_or(&[]);
                    Operation::SetLayoutGrids {
                        id: *id,
                        grids: ondin_core::item::rekey_by_position(
                            own,
                            grids.iter().copied(),
                            &mut fresh,
                            &mut session.ids,
                        ),
                    }
                })
                .collect(),
        );
        if !tx.0.is_empty() {
            self.commit_edit(tx);
        }
    }

    /// Append one grid to whatever each subject already has — the `+` over a
    /// selection that agrees; a disagreeing one goes through [`Self::write_grids`].
    fn append_grid(&mut self, subjects: &[NodeId], grid: LayoutGrid) {
        let session = &mut self.session;
        // Minted once for every frame: per frame, a main and its instance given a
        // grid together would get two items.
        let added = Keyed::new(session.ids.mint_item(), grid);
        let ops: Vec<Operation> = subjects
            .iter()
            .filter_map(|id| {
                let node = session.doc.get(*id)?;
                let mut grids = node.grids().to_vec();
                grids.push(added);
                Some(Operation::SetLayoutGrids { id: *id, grids })
            })
            .collect();
        if !ops.is_empty() {
            self.commit_edit(Transaction(ops));
        }
    }

    /// One grid's controls: the axis and its two buttons, then its colour, then the
    /// numbers, then the alignment.
    ///
    /// Rows sit at [`ui::CARD_ROW_GAP`]; two grids are [`GRID_ROW_GAP`] apart. A
    /// hidden grid's controls go dead through [`ui::disable_unless`] rather than
    /// the paint rows' [`HIDDEN_OPACITY`] fade.
    ///
    /// The numeric fields are hand-wound, not valved: `SetLayoutGrids` has no
    /// preview, so the in-flight grid lives in `OndinApp::grid_scrub`, which the
    /// canvas reads too, and the release commits it once.
    fn grid_rows(
        &mut self,
        ui: &mut egui::Ui,
        subjects: &[NodeId],
        index: usize,
        committed: LayoutGrid,
        item: Option<ondin_core::reset::ItemState>,
    ) -> GridRowOut {
        let mut out = GridRowOut::default();
        // The panel draws only where the subjects agree, so the first names them all.
        let anchor = subjects[0];
        // Read before the buffer: on a cancel frame egui may still report `changed()`.
        let cancelled = self.gesture_cancelled;
        let held = match &self.in_flight.grid_scrub {
            Some((nodes, i, g)) if nodes.as_slice() == subjects && *i == index && !cancelled => {
                Some(*g)
            }
            _ => None,
        };
        let mut grid = held.unwrap_or(committed);
        let gap = ui::CARD_COL_GAP;
        let full = ui.available_width();
        let half = egui::vec2((full - gap) / 2.0, GRID_CELL);
        let track = GRID_CELL - 4.0;

        // Asked before the row is laid out (`component::item_slot`'s caveat).
        let hot = ui.rect_contains_pointer(egui::Rect::from_min_size(
            ui.cursor().min,
            egui::vec2(full, GRID_CELL),
        ));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            let slot = match item {
                Some(_) => 14.0 + gap,
                None => 0.0,
            };
            let axis_w = full - 2.0 * (GRID_CELL + gap) - slot;
            let current = GridAxis::ALL
                .iter()
                .position(|a| *a == grid.axis)
                .unwrap_or(0);
            // The eye and ✕ stay live, or the eye would have no way back.
            if let Some(i) = ui::disable_unless(ui, grid.visible, |ui| {
                ui::segmented(ui, axis_w, track, 2, current, |p, i, rect, on| {
                    grid_glyph(p, rect, axis_glyph(GridAxis::ALL[i]), on);
                })
            })
            .inner
            {
                grid.axis = GridAxis::ALL[i];
                out.set = Some(grid);
            }
            // `FieldButton::Off` in both states: the glyph carries the state.
            let eye = match grid.visible {
                true => icon::EYE,
                false => icon::EYE_SLASH,
            };
            if ui::field_button(ui, eye, GRID_CELL, 15.0, ui::FieldButton::Off)
                .on_hover_text(match grid.visible {
                    true => "Hide this grid",
                    false => "Show this grid",
                })
                .clicked()
            {
                grid.visible = !grid.visible;
                out.set = Some(grid);
            }
            if let Some(state) = item {
                out.reset = super::component::item_slot(ui, state, hot);
            }
            if ui::field_button(ui, icon::X, GRID_CELL, 13.0, ui::FieldButton::Off)
                .on_hover_text("Remove this grid")
                .clicked()
            {
                out.remove = true;
            }
        });

        let mut dragged = false;
        let mut changed = false;
        let mut number = |ui: &mut egui::Ui,
                          size: egui::Vec2,
                          prefix: ui::Prefix,
                          value: &mut f64,
                          scrub: ui::Scrub,
                          decimals: usize| {
            let r = value_field(ui, size, prefix, value, scrub, |d| {
                d.custom_formatter(ui::number(decimals))
            });
            dragged |= r.dragged();
            changed |= r.changed();
        };
        let mut count = f64::from(grid.count);
        // The picker's drag colour (`OndinApp::grid_paint`), kept out of `grid`, or
        // the scrub machine below would commit it once a frame.
        let shown = match &self.in_flight.grid_paint {
            Some((nodes, i, color)) if nodes.as_slice() == subjects && *i == index => *color,
            _ => grid.color,
        };
        ui::disable_unless(ui, grid.visible, |ui| {
            self.slot_color_row(
                ui,
                anchor,
                PaintSlot::Grid(index),
                egui::vec2(full, GRID_CELL),
                shown,
                "Edit this grid's colour",
            );
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                number(
                    ui,
                    half,
                    ui::Prefix::Text("N"),
                    &mut count,
                    // Floor of one: zero is legal in the model but draws nothing.
                    ui::Scrub::whole(0.25).range(1.0..=1000.0),
                    0,
                );
                number(
                    ui,
                    half,
                    ui::Prefix::Text("G"),
                    &mut grid.gutter,
                    ui::Scrub::whole(0.5).range(0.0..=f64::MAX),
                    0,
                );
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                // Nested `disable_unless` does not fade twice.
                ui::disable_unless(ui, grid.align.reads_margin(), |ui| {
                    number(
                        ui,
                        half,
                        ui::Prefix::Text("M"),
                        &mut grid.margin,
                        ui::Scrub::whole(0.5).range(0.0..=f64::MAX),
                        0,
                    );
                });
                ui::disable_unless(ui, grid.align.reads_size(), |ui| {
                    number(
                        ui,
                        half,
                        ui::Prefix::Text("W"),
                        &mut grid.size,
                        ui::Scrub::whole(0.5).range(1.0..=f64::MAX),
                        0,
                    );
                });
            });
            let current = GridAlign::ALL
                .iter()
                .position(|a| *a == grid.align)
                .unwrap_or(0);
            let axis = grid.axis;
            if let Some(i) = ui::segmented(ui, full, track, 4, current, |p, rect_i, rect, on| {
                grid_glyph(p, rect, align_glyph(GridAlign::ALL[rect_i], axis), on);
            }) {
                grid.align = GridAlign::ALL[i];
                out.set = Some(grid);
            }
        });

        grid.count = count.round().clamp(0.0, 1000.0) as u32;
        // Only the numbers go through the scrub machine.
        if out.set.is_none() && !out.remove {
            match grid_step(
                GridScrub {
                    cancelled,
                    dragged,
                    changed,
                    was_held: held.is_some(),
                },
                grid,
                committed,
            ) {
                GridStep::Hold => {
                    self.in_flight.grid_scrub = Some((subjects.to_vec(), index, grid))
                }
                GridStep::Commit(g) => {
                    self.in_flight.grid_scrub = None;
                    out.set = Some(g);
                }
                GridStep::Nothing => {}
            }
        } else {
            // A click ends any scrub, or the buffer would be re-applied over it.
            self.in_flight.grid_scrub = None;
        }
        out
    }
    /// What the Stroke panel is showing for a scope: the layer's own list, the one
    /// the selection shares, or a disagreement.
    fn strokes_shown(&self, scope: PaintScope) -> PaintShown<Keyed<Stroke>> {
        match scope {
            // One layer never disagrees with itself.
            PaintScope::Node(id) => PaintShown::List(
                self.session
                    .display_node(id)
                    .map(|n| n.paint().strokes.clone())
                    .unwrap_or_default(),
            ),
            PaintScope::Selection => {
                ondin_core::shared_strokes(&self.session.doc, self.session.selection.ids())
            }
        }
    }
    /// What the Fill panel is showing for a scope, with each entry's visibility, so
    /// a write that only changes brushes does not switch a hidden paint back on.
    fn fills_shown(&self, scope: PaintScope) -> PaintShown<Keyed<Fill>> {
        match scope {
            PaintScope::Node(id) => PaintShown::List(
                self.session
                    .display_node(id)
                    .map(|n| n.paint().fills.clone())
                    .unwrap_or_default(),
            ),
            PaintScope::Selection => {
                ondin_core::shared_fills(&self.session.doc, self.session.selection.ids())
            }
        }
    }
    /// The stroke rows a scope is showing — none when they disagree, since a mixed
    /// panel draws no indexed row.
    fn strokes_in_scope(&self, scope: PaintScope) -> Vec<Keyed<Stroke>> {
        self.strokes_shown(scope).list().to_vec()
    }
    /// The fill rows a scope is showing. [`Self::strokes_in_scope`]'s twin.
    fn fills_in_scope(&self, scope: PaintScope) -> Vec<Keyed<Fill>> {
        self.fills_shown(scope).list().to_vec()
    }
    /// A transaction that rewrites the brush of **every** fill (or stroke) of every
    /// paint target in the selection.
    ///
    /// The mixed row's colour writes all come through here. None replaces a list,
    /// so none can cost a layer its second fill. `pub(crate)` for `picker::hex_row`
    /// and `picker::document_swatches`.
    pub(crate) fn edit_paints_all(
        &self,
        target: ondin_core::PaintTarget,
        brush_of: impl Fn(&Brush) -> Brush,
    ) -> Transaction {
        let (doc, ids) = (&self.session.doc, self.session.selection.ids());
        ondin_core::edit_brushes_all(doc, ids, target, brush_of)
    }
    /// Every fill in scope's value for one property, as shown — a [`Spread`].
    ///
    /// Through `display_node`, or a scrub on the mixed row would re-read the
    /// pre-gesture value every frame. As in `build::shared_over_fills`, a target
    /// with no fill disagrees with every value.
    fn shown_over_fills<T>(&self, of: impl Fn(&Fill) -> T) -> Spread<T> {
        self.shown_over(|p| &p.fills, of)
    }
    /// [`Self::shown_over_fills`]'s twin for strokes, over the same targets —
    /// frames included, whose outline strokes.
    fn shown_over_strokes<T>(&self, of: impl Fn(&Stroke) -> T) -> Spread<T> {
        self.shown_over(|p| &p.strokes, of)
    }
    /// The walk both `shown_over_*` are, so the `bare` fold has one home. Reads
    /// `display_node`, so a value mid-drag reads as what is on screen.
    fn shown_over<P, T>(
        &self,
        list: impl Fn(&ondin_core::Paint) -> &[Keyed<P>],
        of: impl Fn(&P) -> T,
    ) -> Spread<T> {
        let mut spread = Spread::default();
        for id in ondin_core::paint_targets(&self.session.doc, self.session.selection.ids()) {
            let Some(node) = self.session.display_node(id) else {
                continue;
            };
            let paint = self.session.doc.resolved_paint(id, node.paint());
            let paints = list(&paint);
            // A bare target contributes no value, so it would read as agreement (§9.4).
            spread.bare |= paints.is_empty();
            spread.values.extend(paints.iter().map(|k| of(&k.value)));
        }
        spread
    }
    /// Every paint's brush, as shown — what the mixed row's controls and the picker read.
    fn shown_brushes(&self, target: ondin_core::PaintTarget) -> Spread<Brush> {
        match target {
            ondin_core::PaintTarget::Fill => self.shown_over_fills(|f| f.brush.drawn().clone()),
            ondin_core::PaintTarget::Stroke => self.shown_over_strokes(|s| s.brush.drawn().clone()),
        }
    }
    /// What the mixed row says about itself, and what its controls operate from.
    /// The alpha is compared as a byte, the precision the hex field and the save
    /// format work in.
    fn mixed_row(&self, target: ondin_core::PaintTarget) -> (Brush, MixedRow) {
        let brushes = self.shown_brushes(target);
        let byte = |b: &Brush| (paint::alpha_of(b) * 255.0).round() as u8;
        let alpha = brushes.map(byte);
        let mixed = MixedRow {
            brush: brushes.agreed().is_none(),
            alpha: alpha.agreed().is_none(),
        };
        // `base`, not `agreed`: the controls need a real value while the row says "Mixed".
        let brush = brushes
            .base()
            .unwrap_or_else(|| paint::with_alpha(&paint::new_solid(), 1.0));
        (brush, mixed)
    }
    /// Whether any paint in scope is switched on — what the mixed row's eye shows.
    ///
    /// `any`, not `all`: with some hidden there is still ink, and the click hides
    /// the rest. `!= Some(false)` is that "any".
    fn mixed_visible(&self, target: ondin_core::PaintTarget) -> bool {
        match target {
            ondin_core::PaintTarget::Fill => {
                self.shown_over_fills(|f| f.visible).agreed() != Some(false)
            }
            ondin_core::PaintTarget::Stroke => {
                self.shown_over_strokes(|s| s.visible).agreed() != Some(false)
            }
        }
    }
    /// A single paint row: swatch (opens the picker), hex, opacity, remove.
    /// Returns whether the user asked to remove it.
    ///
    /// The hex beside the swatch is a text field (a gradient's is a label that
    /// opens the picker). Hidden dims the row to [`HIDDEN_OPACITY`] but never the
    /// swatch: [`swatch`] masks its corners in the ground colour, which a
    /// multiplied opacity would turn translucent.
    fn paint_row(&mut self, ui: &mut egui::Ui, ctx: RowCtx, brush: &Brush) -> RowOut {
        let RowCtx {
            id,
            slot,
            visible,
            grip,
            reorderable,
            census,
            mixed,
            item,
        } = ctx;
        let targeted = self
            .picker
            .as_ref()
            .is_some_and(|p| p.node == id && p.slot == slot);
        let mut out = RowOut::default();
        let mut open = false;
        // Acted on below, outside the scope, since the closure holds `&mut self`.
        let mut enter_image = false;
        let anchor = ui.cursor().min;
        let scope = ui.scope(|ui| {
            ui.multiply_opacity(if visible { 1.0 } else { HIDDEN_OPACITY });
            ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                let size = egui::vec2(ui.available_width() - 28.0 - ui::CARD_COL_GAP, 28.0);
                // Asked before the field is laid out.
                let hot =
                    ui.rect_contains_pointer(egui::Rect::from_min_size(ui.cursor().min, size));
                field_row_active(ui, size, targeted, |ui| {
                    ui.spacing_mut().item_spacing.x = 9.0;
                    if let Some((list, index)) = grip {
                        // Only the start is read here; once carried the responses are empty.
                        let g = ui::grip(ui, reorderable, size.y);
                        if g.drag_started() {
                            self.in_flight.paint_drag = Some(PaintDrag {
                                anchor: id,
                                list,
                                from: index,
                                to: index,
                                slots: paint::Slots::default(),
                            });
                            // The stroke menu's `Area` would not follow the row.
                            self.stroke_menu = None;
                            self.stroke_dash_text = None;
                        }
                    }
                    let stops = super::picker::ramp(brush);
                    const CHIP: f32 = 16.0;
                    // The row's content height; 28 would grow a bound row by 2.
                    const CHIP_ROW_H: f32 = ui::CONTROL_H - ui::FIELD_BORDER_H;
                    let unknown = mixed.is_some_and(|m| m.brush);
                    // Asked here so the `&mut self.thumbs` and `&self.canvas` borrows stay disjoint.
                    let thumb = match brush {
                        ondin_core::Brush::Image(img) if !unknown => {
                            self.thumbs
                                .texture(ui.ctx(), self.canvas.images(), &img.image, CHIP)
                        }
                        _ => None,
                    };
                    let paint = match paint::kind_of(brush) {
                        _ if unknown => Swatch::Mixed,
                        PaintKind::Solid => Swatch::Solid(stops[0].1),
                        // An undecodable or budget-deferred picture shows the ramp.
                        PaintKind::Image => match thumb {
                            Some(id) => Swatch::Picture {
                                id,
                                alpha: paint::image_alpha(brush),
                            },
                            None => Swatch::Ramp(&stops),
                        },
                        _ => Swatch::Ramp(&stops),
                    };
                    let chip = ui
                        .scope(|ui| {
                            ui.set_opacity(1.0);
                            swatch(ui, CHIP, color::FIELD, paint)
                        })
                        .inner;
                    // An image's chip opens image editing, not the picker. Only the
                    // topmost visible picture is the mode's subject, and a lock makes
                    // `begin_image_edit` refuse.
                    let picture = !unknown && paint::kind_of(brush) == PaintKind::Image;
                    let locked_layer = ondin_core::is_effectively_locked(&self.session.doc, id);
                    let door = picture
                        && !locked_layer
                        && row_edits_the_mode_s_picture(&self.session.doc, id, slot);
                    let chip = chip.on_hover_text(if unknown {
                        "These layers are painted differently — pick one colour for all of them"
                    } else if picture && locked_layer {
                        "This layer is locked — unlock it to edit the picture"
                    } else if door {
                        "Edit this picture — crop it, move it, scale it"
                    } else if picture {
                        "Image paint — editing works on the topmost picture"
                    } else {
                        "Edit this paint"
                    });
                    if chip.clicked() {
                        if picture {
                            enter_image = true;
                        } else {
                            open = true;
                        }
                    }
                    let key = super::binding::FieldKey::Paint { id, slot };
                    let typing = matches!(&self.paint_hex, Some((n, s, _)) if *n == id && *s == slot)
                        || self.var_retype.as_ref().is_some_and(|r| r.key == key);
                    // Every layer bound, not all to one: the *Mixed* capsule.
                    let mixed_vars = self.field_mixed_vars(key).filter(|_| !typing);
                    match paint::kind_of(brush) {
                        _ if let Some(vars) = &mixed_vars => {
                            let reserve = if item.is_some() { 106.0 } else { 82.0 };
                            let w = (ui.available_width() - reserve).max(40.0);
                            let (rect, _) =
                                ui.allocate_exact_size(egui::vec2(w, CHIP_ROW_H), egui::Sense::hover());
                            self.mixed_chip(ui, key, vars, rect);
                        }
                        // An unknown colour always takes the hex, even over a gradient stand-in.
                        _ if unknown => self.paint_hex_field(ui, id, slot, brush, true),
                        // Bound, the chip replaces the hex until a key is typed onto it.
                        PaintKind::Solid => {
                            match self.field_var(key).filter(|_| !typing) {
                                Some(var) => {
                                    // Keeps room for the `%`, the × and the item slot.
                                    let reserve = if item.is_some() { 106.0 } else { 82.0 };
                                    let w = (ui.available_width() - reserve).max(40.0);
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(w, CHIP_ROW_H),
                                        egui::Sense::hover(),
                                    );
                                    self.field_chip(ui, key, var, rect);
                                }
                                None => self.paint_hex_field(ui, id, slot, brush, false),
                            }
                        }
                        // A clickable label, no hex, stepping into image editing.
                        PaintKind::Image => {
                            let label = ui
                                .add(
                                    egui::Label::new(
                                        egui::RichText::new(paint::label_of(brush))
                                            .size(12.0)
                                            .color(theme::text::STRONG),
                                    )
                                    .sense(egui::Sense::click()),
                                )
                                .on_hover_text(if door {
                                    "Edit this picture — crop it, move it, scale it"
                                } else if locked_layer {
                                    "This layer is locked — unlock it to edit the picture"
                                } else {
                                    "Image paint — editing works on the topmost picture"
                                });
                            if label.clicked() {
                                enter_image = true;
                            }
                        }
                        _ => {
                            let label = ui
                                .add(
                                    egui::Label::new(
                                        egui::RichText::new(paint::label_of(brush))
                                            .size(12.0)
                                            .color(theme::text::STRONG),
                                    )
                                    .sense(egui::Sense::click()),
                                )
                                .on_hover_text("Edit this gradient");
                            if label.clicked() {
                                open = true;
                            }
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // A census row ends in a count: no one paint to fade or remove.
                        if let Some(layers) = census {
                            ui.label(
                                egui::RichText::new(match layers {
                                    1 => "1 match".to_string(),
                                    n => format!("{n} matches"),
                                })
                                .size(11.0)
                                .color(theme::text::DIM),
                            );
                            return;
                        }
                        // An instance's item slot: nothing, a dot, `+`, or ↺ on hover.
                        if let Some(state) = item {
                            out.reset = super::component::item_slot(ui, state, hot);
                        }
                        if icon_button(ui, icon::X, 18.0, 13.0, false, true)
                            .on_hover_text("Remove")
                            .clicked()
                        {
                            out.remove = true;
                        }
                        let hazy = mixed.is_some_and(|m| m.alpha);
                        // A bound paint's `%` is the binding's multiplier, not the alpha drawn.
                        let bound_alpha = self.field_alpha(super::binding::FieldKey::Paint { id, slot });
                        let mut pct = match bound_alpha {
                            Some(a) => a * 100.0,
                            None => paint::alpha_of(brush) * 100.0,
                        };
                        let value = egui::DragValue::new(&mut pct)
                            .speed(ui::scrub(0.5))
                            .max_decimals(0)
                            .range(0.0..=100.0);
                        // egui appends a suffix after the formatter.
                        let value = if hazy {
                            value.custom_formatter(|_, _| "Mixed".into())
                        } else {
                            value.suffix("%")
                        };
                        // `ui::bare_drag_value`'s parser drops a typed `%` on both branches.
                        let resp = ui::bare_drag_value(ui, value);
                        match slot {
                            // Into each paint, not the slot's write, which would carry
                            // the stand-in brush to every layer.
                            PaintSlot::SelectionAll(target) => {
                                // Seeded from `Spread::base()`, a Mixed field writes
                                // nothing until moved.
                                let moved = pct != paint::alpha_of(brush) * 100.0;
                                if !hazy || moved {
                                    let a = (pct / 100.0).clamp(0.0, 1.0);
                                    let tx = self
                                        .edit_paints_all(target, move |b| paint::with_alpha(b, a));
                                    self.edit_valve(&resp, tx);
                                }
                            }
                            _ if bound_alpha.is_some() => {
                                let a = (pct / 100.0).clamp(0.0, 1.0);
                                let tx = self.rebind_alpha_tx(id, slot, a);
                                self.edit_valve(&resp, tx);
                            }
                            _ => {
                                let next = paint::with_alpha(brush, pct / 100.0);
                                self.valve_slot(&resp, id, slot, next);
                            }
                        }
                    });
                });
                // The eye sits outside the field: it is about the row, not the colour.
                if grip.is_some() {
                    let (glyph, tip) = if visible {
                        (icon::EYE, "Hide this paint")
                    } else {
                        (icon::EYE_SLASH, "Show this paint")
                    };
                    if ui::field_button(ui, glyph, 28.0, 14.0, ui::FieldButton::Off)
                        .on_hover_text(tip)
                        .clicked()
                    {
                        out.toggle = true;
                    }
                } else if let Some(layers) = census {
                    // The eye's slot: a reticle that selects the colour's layers.
                    if ui::field_button(
                        ui,
                        icon::SCAN,
                        28.0,
                        14.0,
                        ui::FieldButton::enabled_if(layers > 0),
                    )
                    .on_hover_text(match layers {
                        1 => "Select the layer using this colour".to_string(),
                        n => format!("Select all {n} layers using this colour"),
                    })
                    .clicked()
                    {
                        out.select = true;
                    }
                }
            });
            self.image_card_anchor(ui, id, slot, brush, mixed.is_some_and(|m| m.brush));
        });
        out.rect = scope.response.rect;
        // A `+` on the panel header asked for this row before it existed.
        if open || self.pending_picker.take_if(|p| *p == (id, slot)).is_some() {
            self.open_picker(ui, id, slot, anchor);
        }
        if enter_image && row_edits_the_mode_s_picture(&self.session.doc, id, slot) {
            self.begin_image_edit(id);
        }
        out
    }
    /// Anchors image editing's card to the image row.
    ///
    /// The mode opens the card; the row supplies the id, slot, brush and — through
    /// `ui.min_rect()` — the height to hang it at. Nothing for a non-image or mixed row.
    fn image_card_anchor(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        slot: paint::PaintSlot,
        brush: &Brush,
        mixed: bool,
    ) {
        if mixed {
            return;
        }
        if paint::image_of(brush).is_none() {
            return;
        }
        self.image_edit_card(ui, ui.min_rect(), id, slot, brush);
    }

    /// Image editing's card: everything about a picture that wants a control
    /// rather than a gesture (§5.5a), under two tabs in one `Area`.
    ///
    /// The mode owns it: it appears when a picture is double-clicked and goes when
    /// `Enter` or `Escape` leaves, so there is no close button and no click-away rule.
    #[allow(clippy::too_many_lines)]
    fn image_edit_card(
        &mut self,
        ui: &mut egui::Ui,
        row: egui::Rect,
        id: NodeId,
        slot: PaintSlot,
        brush: &Brush,
    ) {
        const MENU_W: f32 = crate::app::POPOVER_W;
        const MENU_PAD: f32 = 11.0;
        /// Every control's height.
        const CELL: f32 = ui::CONTROL_H;
        /// What a segmented track is asked for to paint [`CELL`]: `ui::segmented`
        /// pads two points either side.
        const TRACK_CELL: f32 = CELL - 4.0;
        let PaintSlot::Fill(index) = slot else {
            return;
        };
        if self.edited_image() != Some((id, index)) {
            return;
        }
        let Some(img) = paint::image_of(brush).cloned() else {
            return;
        };
        let card = self.image_tab;
        // `Some(keep_framing)`, deferred past the card: each opens a modal file
        // dialog, which mid-`show` would block with the popover half-painted.
        let mut replace: Option<bool> = None;
        // Unreachable while the guard above requires `PaintSlot::Fill`; kept so a
        // future multi-selection entry cannot replace three pictures with one file.
        let replace_scoped = matches!(
            slot,
            paint::PaintSlot::Selection(..) | paint::PaintSlot::SelectionAll(..)
        );
        let ctx = ui.ctx().clone();
        let anchor = Self::popover_anchor(&ctx, row, MENU_W);
        // An `Area` does not inherit the styled card's disable, so its writes would
        // detach the style.
        let locked = !ui.is_enabled();
        let mut picked_tab: Option<ImageCard> = None;
        egui::Area::new(egui::Id::new(("image-edit", id)))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            .constrain_to(ctx.input(|i| i.content_rect()).shrink(12.0))
            .show(&ctx, |ui| {
                ui::menu_frame(MENU_PAD).show(ui, |ui| {
                    let inner = ui::menu_inner_w(MENU_W, MENU_PAD);
                    ui.set_width(inner);
                    if locked {
                        super::styles::lock(ui);
                    }
                    // Before the first control: egui spaces past a widget by the
                    // pitch in effect when it is placed.
                    ui.spacing_mut().item_spacing.y = IMAGE_CARD_GAP;

                    // The strip is also the card's heading.
                    let current = ImageCard::ALL.iter().position(|c| *c == card).unwrap_or(0);
                    if let Some(i) = segmented(
                        ui,
                        inner,
                        TRACK_CELL,
                        ImageCard::ALL.len(),
                        current,
                        |p, i, r, on| segment_label(p, r, ImageCard::ALL[i].title(), on),
                    ) {
                        picked_tab = Some(ImageCard::ALL[i]);
                    }

                    if card == ImageCard::Settings {
                        // Leaving Crop re-frames the picture in the box the crop left;
                        // `with_fit` seeds a rectangle on the way in so nothing moves.
                        ui.scope(|ui| {
                            ui.spacing_mut().item_spacing.y = 5.0;
                            ui.label(ui::eyebrow("Mode"));
                            let fit = img.fit;
                            let current = ondin_core::ImageFit::ALL
                                .iter()
                                .position(|f| *f == fit)
                                .unwrap_or(0);
                            if let Some(i) = segmented(
                                ui,
                                inner,
                                TRACK_CELL,
                                ondin_core::ImageFit::ALL.len(),
                                current,
                                |p, i, r, on| {
                                    segment_label(p, r, fit_label(ondin_core::ImageFit::ALL[i]), on)
                                },
                            ) && ondin_core::ImageFit::ALL[i] != fit
                            {
                                let frame = self.crop_seed(id, slot, brush);
                                self.write_slot(
                                    id,
                                    slot,
                                    paint::with_fit(brush, ondin_core::ImageFit::ALL[i], frame),
                                );
                            }
                        });
                        // Words, not the flip glyphs: these mirror the picture, not the
                        // layer. Actions, not toggles, since `ImageOrient` composes.
                        ui.scope(|ui| {
                            ui.spacing_mut().item_spacing.y = 5.0;
                            ui.label(ui::eyebrow("Mirror"));
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                                let bw = egui::vec2((inner - ui::CARD_COL_GAP) / 2.0, CELL);
                                for (label, op, tip) in [
                                    (
                                        "Horizontal",
                                        OrientOp::FlipH,
                                        "Mirror the picture in its frame, left to right — \
                                     the layer itself does not move",
                                    ),
                                    (
                                        "Vertical",
                                        OrientOp::FlipV,
                                        "Mirror the picture in its frame, top to bottom — \
                                     the layer itself does not move",
                                    ),
                                ] {
                                    if ui::label_button(ui, label, bw, ui::FieldButton::Off)
                                        .on_hover_text(tip)
                                        .clicked()
                                    {
                                        // `reoriented` carries the crop through the same symmetry.
                                        self.write_slot(
                                            id,
                                            slot,
                                            paint::map_image(brush, |r| *r = r.reoriented(op)),
                                        );
                                    }
                                }
                            });
                        });

                        // Scale for Crop, Tile for Tile: one measurement (local units
                        // per source pixel), read by one mode each. Dimmed rather
                        // than hidden off-mode: they are stored values.
                        let cropping = img.fit == ondin_core::ImageFit::Crop;
                        // An uncropped picture is seeded through `cover_crop`, as
                        // `paint::with_fit` and the canvas gesture do.
                        let scale = self.crop_seed(id, slot, brush).and_then(|(f, w, h)| {
                            let shown = ondin_core::ImageRef {
                                fit: ondin_core::ImageFit::Crop,
                                crop: match cropping {
                                    true => img.crop,
                                    false => img.cover_crop(f, w, h),
                                },
                                ..img.clone()
                            };
                            Some((shown.crop_scale(f, w, h)?, img.cover_scale(f, w, h)?))
                        });
                        let tiling = img.fit == ondin_core::ImageFit::Tile;
                        // Percent under the hand; the stored numbers are multipliers.
                        let mut crop_pct = scale.map_or(100.0, |(s, _)| s * 100.0);
                        let mut tile_pct = img.tile_scale * 100.0;
                        let half = egui::vec2((inner - ui::CARD_COL_GAP) / 2.0, CELL);
                        let mut fields = Vec::new();
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                            // Floor `cover_scale`: below it the crop leaves the source.
                            let floor = scale.map_or(1.0, |(_, c)| (c * 100.0).max(0.01));
                            // `cover_scale` is uncapped (a tiny icon on a big frame
                            // gives 12 000), so the ceiling must not fall below it.
                            let ceiling = floor.max(10_000.0);
                            let live = cropping && scale.is_some();
                            fields.push((
                                live,
                                "dimmed-crop-scale",
                                ui.scope(|ui| {
                                    if !live {
                                        ui.disable();
                                    }
                                    value_field(
                                        ui,
                                        half,
                                        Prefix::Icon(icon::RESIZE),
                                        &mut crop_pct,
                                        // Hundredths: whole steps lurch a multiplicative scale.
                                        Scrub::fine(0.5, 2).range(floor..=ceiling),
                                        |d| d.suffix("%").custom_formatter(ui::number(2)),
                                    )
                                })
                                .inner,
                            ));
                            fields.push((
                                tiling,
                                "dimmed-tile-scale",
                                ui.scope(|ui| {
                                    if !tiling {
                                        ui.disable();
                                    }
                                    value_field(
                                        ui,
                                        half,
                                        Prefix::Icon(icon::GRID_FOUR),
                                        &mut tile_pct,
                                        Scrub::whole(0.5).range(1.0..=10_000.0),
                                        |d| d.suffix("%").custom_formatter(ui::number(2)),
                                    )
                                })
                                .inner,
                            ));
                        });
                        // A dimmed field's tooltip needs its own claimant.
                        for (live, salt, resp) in fields {
                            if !live {
                                ui.interact(resp.rect, ui.id().with(salt), egui::Sense::empty())
                                    .on_hover_text(match salt {
                                        "dimmed-crop-scale" => {
                                            "Scale — only Crop zooms the picture"
                                        }
                                        _ => "Tile — only Tile repeats the picture",
                                    });
                                continue;
                            }
                            // Live fields only: the valve's `note_edit` would arm the chrome hide.
                            let (hint, edit) = match salt {
                                "dimmed-crop-scale" => (
                                    "Scale — how big the picture is inside the crop, \
                                     against its own pixels",
                                    paint::map_image(brush, |r| {
                                        if let Some((now, _)) = scale
                                            && crop_pct > 0.0
                                        {
                                            r.crop = ondin_core::crop_scaled(
                                                r.crop,
                                                now / (crop_pct / 100.0),
                                            );
                                        }
                                    }),
                                ),
                                _ => (
                                    "Tile — how big one repeat is, against the \
                                     picture's own pixels",
                                    paint::map_image(brush, |r| r.tile_scale = tile_pct / 100.0),
                                ),
                            };
                            self.valve_slot(&resp.on_hover_text(hint), id, slot, edit);
                        }

                        // Restores the `whole_crop` sentinel ("never cropped"), so the next
                        // entry into Crop seeds cover.
                        let cropped = img.crop != ondin_core::whole_crop();
                        ui.scope(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            if menu_action(
                                ui,
                                cropped,
                                icon::ARROW_COUNTER_CLOCKWISE,
                                "Reset crop",
                                &|on| {
                                    if on {
                                        "Show the whole picture again".into()
                                    } else {
                                        "This picture has not been cropped".into()
                                    }
                                },
                            ) {
                                self.write_slot(
                                    id,
                                    slot,
                                    paint::map_image(brush, |r| r.crop = ondin_core::whole_crop()),
                                );
                            }
                        });

                        // *Embed*/*Link* and *Relink…* are absent, not dead: they wait on
                        // a linked source nothing can author.
                        ui.scope(|ui| {
                            ui.spacing_mut().item_spacing.y = 5.0;
                            ui.label(ui::eyebrow("Asset"));
                            ui.scope(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                // Also the repair for a missing picture's placeholder.
                                if menu_action(
                                    ui,
                                    !replace_scoped,
                                    icon::SWAP,
                                    "Replace…",
                                    &|on| {
                                        if on {
                                            "Pick a file to show here, keeping the framing".into()
                                        } else {
                                            "Replacing works on one layer at a time".into()
                                        }
                                    },
                                ) {
                                    replace = Some(true);
                                }
                                // The framing dropped, for a picture of another shape;
                                // the adjustments are kept (`replace_image_action`).
                                if menu_action(
                                    ui,
                                    !replace_scoped,
                                    icon::BROOM,
                                    "Replace and reset",
                                    &|on| {
                                        if on {
                                            "Pick a file and show all of it, \
                                             dropping the crop and the mode"
                                                .into()
                                        } else {
                                            "Replacing works on one layer at a time".into()
                                        }
                                    },
                                ) {
                                    replace = Some(false);
                                }
                            });
                        });
                    }

                    if card == ImageCard::Adjustments {
                        // Rows built from `Adjustment::ALL`, so order and words are the
                        // model's. Percent under the hand, `-1..=1` in the file.
                        let adjusted = !img.adjust.is_default();
                        ui.scope(|ui| {
                            for k in Adjustment::ALL {
                                let at_rest = k.of(&img.adjust) == 0.0;
                                let mut pct = f64::from(k.of(&img.adjust)) * 100.0;
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                                    let resp = ui::value_field_metered(
                                        ui,
                                        egui::vec2(inner - CELL - ui::CARD_COL_GAP, CELL),
                                        Prefix::Text(k.label()),
                                        // The signed fraction of the reachable range.
                                        (pct / 100.0) as f32,
                                        &mut pct,
                                        Scrub::whole(0.25).range(-100.0..=100.0),
                                        |d| d.custom_formatter(signed_number),
                                    )
                                    .on_hover_text(k.hint());
                                    self.valve_slot(
                                        &resp,
                                        id,
                                        slot,
                                        paint::map_image(brush, |r| {
                                            k.set(&mut r.adjust, (pct / 100.0) as f32)
                                        }),
                                    );
                                    // Dimmed at rest; re-asked after the click because a dim
                                    // `icon_button` still senses one.
                                    if ui::icon_button(
                                        ui,
                                        icon::ARROW_COUNTER_CLOCKWISE,
                                        CELL,
                                        12.0,
                                        false,
                                        !at_rest,
                                    )
                                    .on_hover_text(match at_rest {
                                        true => format!("{} is already at rest", k.label()),
                                        false => format!("Reset {}", k.label().to_lowercase()),
                                    })
                                    .clicked()
                                        && !at_rest
                                    {
                                        self.write_slot(
                                            id,
                                            slot,
                                            paint::map_image(brush, |r| k.set(&mut r.adjust, 0.0)),
                                        );
                                    }
                                });
                            }
                        });
                        ui.scope(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            if menu_action(
                                ui,
                                adjusted,
                                icon::ARROW_COUNTER_CLOCKWISE,
                                "Reset adjustments",
                                &|on| match on {
                                    true => "Put all seven back to zero".into(),
                                    false => "Nothing on this picture is adjusted".into(),
                                },
                            ) {
                                self.write_slot(
                                    id,
                                    slot,
                                    paint::map_image(brush, |r| r.adjust.reset()),
                                );
                            }
                        });
                    }
                });
            });
        // No click-away and no `Escape` here: a canvas click is often the next crop
        // drag. Written after the `Area`, whose closure holds `&mut self`.
        if let Some(tab) = picked_tab {
            self.image_tab = tab;
        }
        if let Some(keep_framing) = replace {
            self.replace_image_action(id, slot, keep_framing);
        }
    }

    /// The intrinsic pixel size of the picture on this row, for *original size*.
    ///
    /// Declines on a selection slot, which would resize every member to one
    /// picture. Unoriented: the caller applies [`ondin_core::ImageRef::orient`].
    pub(crate) fn image_pixels(&self, slot: paint::PaintSlot, brush: &Brush) -> Option<(u32, u32)> {
        if matches!(
            slot,
            paint::PaintSlot::Selection(..) | paint::PaintSlot::SelectionAll(..)
        ) {
            return None;
        }
        let img = paint::image_of(brush)?;
        let e = self.session.doc.image(&img.id)?;
        Some((e.width, e.height))
    }

    /// Put `id`'s box at `w` × `h`, the picture's own pixels.
    ///
    /// The W/H fields' three spellings: an authored `size`, a `Text` node's
    /// `TextSizing` (reachable: a drop or paste can put a picture on one), and
    /// `resize_box_to` for a kind with no size field.
    pub(crate) fn resize_to_original(&mut self, id: NodeId, w: f64, h: f64) {
        let size = Size::new(w, h);
        let kind = self.session.display_node(id).map(|n| n.kind().clone());
        let tx = match kind {
            Some(NodeKind::Text { .. }) => Transaction(vec![Operation::SetGeometry {
                id,
                geometry: GeometryPatch::TextSizing(TextSizing::Fixed(size)),
            }]),
            Some(k) if tools::resizable_size(&k).is_some() => {
                Transaction(vec![Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::Size(size),
                }])
            }
            Some(_) => tools::resize_box_to(&self.session.doc, &self.session.resolved, id, size),
            None => Transaction(Vec::new()),
        };
        self.commit_edit(tx);
    }

    /// The editable hex for a solid paint row.
    ///
    /// Buffered and applied on defocus, so "6D" is not committed on the way to
    /// "6D8CD9". Any CSS colour reads (`colour_text`). Alpha is the row's opacity
    /// field. `mixed` shows the word, which does not parse, so typing over it needs
    /// no special case.
    pub(super) fn paint_hex_field(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        slot: PaintSlot,
        brush: &Brush,
        mixed: bool,
    ) {
        let current = paint::stops_of(brush)[0].1;
        // A bound chip typed over hands its first key here to start the buffer.
        let key = super::binding::FieldKey::Paint { id, slot };
        let retyped = self.var_retype.as_ref().is_some_and(|r| r.key == key);
        if let Some(rt) = self.var_retype.as_mut().filter(|r| r.key == key && r.focus) {
            rt.focus = false;
            self.paint_hex = Some((id, slot, rt.text.clone()));
        }
        let bound = self.field_var(key);
        let editing = matches!(&self.paint_hex, Some((n, s, _)) if *n == id && *s == slot);
        let mut text = match &self.paint_hex {
            Some((_, _, buf)) if editing => buf.clone(),
            _ if mixed => "Mixed".to_string(),
            _ => hex_of(current),
        };
        let edit_id = egui::Id::new(("paint-hex", id, format!("{slot:?}")));
        // `--name` is longer than any hex.
        let limit = if text.starts_with('-') {
            usize::MAX
        } else {
            ui::COLOUR_CHAR_LIMIT
        };
        // A refused `--name` tail dims as it is typed.
        use ondin_core::variable::VarKind;
        let font = egui::FontId::proportional(12.0);
        let table = self.session.doc.variables();
        let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
            let at = super::binding::refused(table, VarKind::Color, buf.as_str()).map(|r| r.1);
            super::binding::refusal_galley(ui, buf.as_str(), at, &font)
        };
        let out = egui::TextEdit::singleline(&mut text)
            .id(edit_id)
            .frame(egui::Frame::NONE)
            .desired_width(62.0)
            .char_limit(limit)
            .font(font.clone())
            .layouter(&mut layouter)
            .show(ui);
        let at = super::binding::refused(table, VarKind::Color, &text).map(|r| r.1);
        super::binding::underline_refusal(ui, &out, at);
        let resp = out.response;
        if retyped && !resp.has_focus() && editing {
            resp.request_focus();
        }
        if retyped && resp.gained_focus() {
            self.var_retype = None;
            super::binding::caret_to_end(ui.ctx(), edit_id, &text);
        } else {
            ui::select_all_on_focus(ui, &resp, &text);
        }
        if resp.gained_focus() || resp.changed() {
            self.paint_hex = Some((id, slot, text.clone()));
        }
        // `Enter` on a refused `--` goes back through the retype buffer, which
        // refocuses it, since a single-line `TextEdit` drops focus.
        let refused_enter =
            resp.lost_focus() && at.is_some() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if refused_enter {
            self.var_retype = Some(super::binding::Retype {
                key,
                text: text.clone(),
                focus: true,
            });
        }
        if resp.has_focus() || retyped || refused_enter {
            self.bind_typing(ui.ctx(), key, &text, resp.rect);
        }
        if resp.secondary_clicked()
            && let Some(at) = resp.interact_pointer_pos()
            && !mixed
        {
            self.field_menu = Some((key, at, resp.rect));
        }
        // On every way out, `Escape` included; falling back to `hex_of` strips a typed `#`.
        if resp.lost_focus() && !refused_enter {
            self.paint_hex = None;
        }
        // Not valved: `6D8` is a real colour on the way to `6D8CD9`.
        if ui::defocus_commits(&resp) {
            // Compared as bytes: an off-lattice colour would round-trip changed by a
            // bare click, and `lost_focus()` fires on two frames. `mixed` always
            // writes. A hex over a binding is a literal that detaches it, so a bound
            // field writes outright rather than through `edited_brush`.
            // A typed alpha sets the opacity too; a colour without one keeps it.
            let unchanged = !mixed && bound.is_none() && colour_text::matches(&text, current);
            let typed = colour_text::parse(&text);
            if let Some(why) = colour_text::refusal(&text) {
                self.session.info(why);
            }
            let typed_alpha = typed.and_then(|t| t.alpha).filter(|_| slot.takes_alpha());
            if let Some(t) = typed.filter(|_| !unchanged) {
                let rgb = t.color(1.0);
                self.remember_colour(rgb);
                if bound.is_some() {
                    let lit = rgb.with_alpha(typed_alpha.unwrap_or(current.components[3]));
                    let ids = match slot {
                        PaintSlot::Selection(..) => self.session.selection.ids().to_vec(),
                        _ => vec![id],
                    };
                    let (target, index) = match slot {
                        PaintSlot::Fill(i) => (PaintTarget::Fill, i),
                        PaintSlot::Stroke(i) => (PaintTarget::Stroke, i),
                        PaintSlot::Selection(t, i) => (t, i),
                        _ => return,
                    };
                    let tx = ondin_core::set_brush_at_all(
                        &self.session.doc,
                        &ids,
                        target,
                        index,
                        |_, _| lit.into(),
                    );
                    self.commit_edit(tx);
                    return;
                }
                // Without a typed alpha, each paint keeps its own opacity.
                if let PaintSlot::SelectionAll(target) = slot {
                    let tx = self.edit_paints_all(target, move |b| {
                        Brush::Solid(
                            rgb.with_alpha(typed_alpha.unwrap_or_else(|| paint::alpha_of(b))),
                        )
                    });
                    self.commit_edit(tx);
                    return;
                }
                let alpha = typed_alpha.unwrap_or(current.components[3]);
                let next = paint::with_stops(brush, vec![(0.0, rgb.with_alpha(alpha))]);
                self.write_slot(id, slot, next);
            }
        }
    }
    // --- effects -----------------------------------------------------------
    /// The Effects panel: one row per effect, a popover behind each row, and a
    /// `+` in the header that adds one (§5.3a, §9.4).
    ///
    /// Model order, first at the top: a pipeline, not a stack in space. Over a
    /// disagreement the `+` replaces every layer's stack, in one undo step.
    fn inspector_effects(&mut self, ui: &mut egui::Ui, subjects: &[NodeId]) {
        let Some(&anchor) = subjects.first() else {
            return;
        };
        // `None` is a disagreement; read through the preview so a scrub over
        // several layers keeps them agreeing.
        let shared = self.shared_effects(subjects);
        let mixed = shared.is_none();
        let effects = shared.unwrap_or_default();
        // Dropped, not ignored, so returning to that layer does not reopen it.
        if self.effect_menu.is_some_and(|(n, _)| n != anchor) {
            self.effect_menu = None;
        }
        // Fill and Stroke's resting state; a disagreement counts as having
        // something. Its own edge: the paint panels consumed theirs.
        let fresh = self.effects_subject_changed();
        let ghosts = match subjects {
            [one] => self.has_ghosts(Some(*one), |n| n.effects()),
            _ => false,
        };
        self.sync_paint_collapse("Effects", !mixed && effects.is_empty() && !ghosts, fresh);
        let mut out = EffectRowOut::default();
        let mut acted: Option<usize> = None;
        let mut reorder: Option<(usize, usize)> = None;
        let source = match subjects {
            [one] => self.source_list(*one, |n| n.effects()),
            _ => None,
        };
        let mut restore = None;
        let action = Some(if mixed {
            // Live, or there would be no way out of the state.
            ui::HeadAction::new(
                icon::PLUS,
                "Replace every selected layer's effects with a new one",
            )
        } else {
            ui::HeadAction::new(icon::PLUS, "Add an effect")
        });
        let add = self.style_card(
            ui,
            "Effects",
            StyleSlot::Effects,
            subjects,
            action,
            |app, ui| {
                if mixed {
                    ui.label(
                        egui::RichText::new("These layers have different effects")
                            .size(11.0)
                            .color(theme::text::FAINT),
                    );
                    return;
                }
                // An instance that removed every effect still has its ghost rows.
                if effects.is_empty() && source.as_ref().is_none_or(Vec::is_empty) {
                    ui.label(
                        egui::RichText::new("No effects")
                            .size(11.0)
                            .color(theme::text::FAINT),
                    );
                    return;
                }
                let mut head = None;
                let states = source
                    .as_ref()
                    .map(|s| ondin_core::reset::item_states(s, &effects));
                // The row's headline number as drawn (a bound radius resolved); the
                // popover seeds from the stored list.
                let drawn = app
                    .session
                    .doc
                    .resolved_effects(anchor, &effects)
                    .into_owned();
                let carrying = app
                    .in_flight
                    .effect_drag
                    .as_ref()
                    .filter(|d| d.anchor == anchor)
                    .map(|d| d.from);
                let order = app.effect_order(ui, anchor, effects.len());
                let mut slots = paint::Slots::default();
                for &index in &order {
                    let effect = &effects[index];
                    let open = app.effect_menu == Some((anchor, index));
                    let item = states.as_ref().map(|s| s[index]);
                    let shown = drawn.get(index).unwrap_or(effect);
                    let reorderable = effects.len() > 1;
                    let ((row, resp), rect) =
                        carried_unit(ui, carrying == Some(index), ("effect-row", index), |ui| {
                            Self::effect_row(ui, index, shown, open, item, reorderable)
                        });
                    slots.push(rect.top(), rect.bottom());
                    if row.grabbed {
                        app.in_flight.effect_drag = Some(paint::EffectDrag {
                            anchor,
                            from: index,
                            to: index,
                            slots: paint::Slots::default(),
                        });
                        // Its `Area` would not follow the row.
                        app.effect_menu = None;
                    }
                    if row.remove || row.toggle || row.open || row.reset {
                        out = row;
                        acted = Some(index);
                    }
                    if open {
                        head = Some(resp);
                    }
                }
                reorder = app.finish_effect_drag(ui, anchor, slots);
                for gone in source
                    .as_deref()
                    .map(|s| ondin_core::reset::removed_items(s, &effects))
                    .unwrap_or_default()
                {
                    let lead = super::component::GhostLead::Glyph(effect_glyph(&gone.kind));
                    if super::component::ghost_row(ui, lead, gone.kind.label()) {
                        restore = Some(gone.id);
                    }
                }
                // After every row, so the popover cannot claim a press meant for a
                // row below its own.
                if let (Some(head), Some((_, index))) = (head, app.effect_menu)
                    && let Some(effect) = effects.get(index)
                {
                    app.effect_menu_popup(ui, &head, subjects, index, effect);
                }
            },
        );
        if add {
            self.open_paint_panel("Effects");
            let [drop, ..] = EffectKind::all_defaults();
            // Over a disagreement `effects` is empty, so this append is the
            // replacement.
            let mut next = effects.clone();
            let at = next.len();
            next.push(Keyed::new(self.session.ids.mint_item(), Effect::new(drop)));
            self.write_effects(subjects, next);
            // Straight open: the popover anchors on the row's response next frame.
            self.effect_menu = Some((anchor, at));
            return;
        }
        if let Some((from, to)) = reorder {
            self.write_effects(subjects, paint::reordered(&effects, from, to));
            // The picker and hex buffer follow their effect to its new place; the
            // popover closed at the grab.
            let n = effects.len();
            let moved = |i| paint::reordered_index(n, from, to, i);
            self.remap_slots(anchor, |slot| match slot {
                PaintSlot::Effect(i) => moved(i).map(PaintSlot::Effect),
                untouched => Some(untouched),
            });
            return;
        }
        // Written verbatim, not through `write_effects`, so a restored effect keeps
        // the main's id; the source exists for one subject only.
        let reset = acted.filter(|_| out.reset).map(|i| effects[i].id);
        let list = match (restore, reset, source.as_deref()) {
            (Some(_), _, Some(src)) => Some(src.to_vec()),
            (None, Some(item), Some(src)) => {
                Some(ondin_core::reset::reset_item(src, &effects, item))
            }
            _ => None,
        };
        if let (Some(effects), [one]) = (list, subjects) {
            self.commit_edit(Transaction(vec![Operation::SetEffects {
                id: *one,
                effects,
            }]));
            return;
        }
        let Some(index) = acted else { return };
        if out.remove {
            let mut next = effects.clone();
            next.remove(index);
            self.write_effects(subjects, next);
            // The popover, picker and hex buffer are keyed by index into a list
            // that just got shorter.
            self.effect_menu = self
                .effect_menu
                .and_then(|(n, i)| Some((n, paint::removed_index(index, i)?)));
            self.remap_effect_slots(anchor, index);
        } else if out.toggle {
            let mut next = effects.clone();
            next[index].visible = !next[index].visible;
            self.write_effects(subjects, next);
        } else if out.open {
            // A toggle: the dismissal rule exempts a press on the head, so
            // closing on the opening row happens here.
            self.effect_menu =
                (self.effect_menu != Some((anchor, index))).then_some((anchor, index));
        }
    }
    /// Whether the Effects panel is looking at a different subject than the last
    /// frame that drew it — and record this one from now on. Its own
    /// [`OndinApp::synced_selection`] entry, since asking consumes the edge.
    fn effects_subject_changed(&mut self) -> bool {
        self.subject_changed("Effects")
    }
    /// The stack every subject agrees on, or `None` where they disagree.
    ///
    /// Through the preview, unlike `shared_exports`, so a scrub over several layers
    /// does not read as mixed. `pub(crate)` for `picker_ui`.
    pub(crate) fn shared_effects(&self, subjects: &[NodeId]) -> Option<Vec<Keyed<Effect>>> {
        let mut nodes = subjects
            .iter()
            .filter_map(|id| self.session.display_node(*id));
        let first = nodes.next()?.effects().to_vec();
        nodes
            .all(|n| ondin_core::same_values(n.effects(), &first))
            .then_some(first)
    }
    /// Write one stack to every subject, as a single undo step.
    ///
    /// `effects` is an edit of the first subject's list, retargeted onto each
    /// subject's own item ids (`ondin_core::item::retarget`).
    fn write_effects(&mut self, subjects: &[NodeId], effects: Vec<Keyed<Effect>>) {
        let anchor = subjects
            .first()
            .and_then(|id| self.session.display_node(*id))
            .map(|n| n.effects().to_vec())
            .unwrap_or_default();
        let session = &mut self.session;
        let tx = Transaction(
            subjects
                .iter()
                .map(|id| {
                    let own = session.doc.get(*id).map(|n| n.effects()).unwrap_or(&[]);
                    Operation::SetEffects {
                        id: *id,
                        effects: ondin_core::item::retarget(
                            &anchor,
                            own,
                            &effects,
                            &mut session.ids,
                        ),
                    }
                })
                .collect(),
        );
        if !tx.0.is_empty() {
            self.commit_edit(tx);
        }
    }
    /// Move the picker and the hex buffer off a removed effect row, or drop them.
    /// Not through [`Self::remap_paint_indices`], whose [`PaintList`] names only
    /// the lists that reorder.
    fn remap_effect_slots(&mut self, anchor: NodeId, gone: usize) {
        self.remap_slots(anchor, |slot| match slot {
            PaintSlot::Effect(i) => paint::removed_index(gone, i).map(PaintSlot::Effect),
            untouched => Some(untouched),
        });
    }

    /// The same for a removed layout grid ([`PaintSlot::Grid`]). Without it a
    /// picker on `Grid(1)` after grid 0 goes would silently edit the wrong grid.
    fn remap_grid_slots(&mut self, anchor: NodeId, gone: usize) {
        self.remap_slots(anchor, |slot| match slot {
            PaintSlot::Grid(i) => paint::removed_index(gone, i).map(PaintSlot::Grid),
            untouched => Some(untouched),
        });
    }

    /// Carry the picker and the hex buffer across a renumbering of an index-keyed
    /// list, or drop them where the row they named went.
    fn remap_slots(&mut self, anchor: NodeId, moved: impl Fn(PaintSlot) -> Option<PaintSlot>) {
        let next = self
            .picker
            .as_ref()
            .filter(|p| p.node == anchor)
            .map(|p| moved(p.slot));
        match next {
            Some(None) => self.picker = None,
            Some(Some(slot)) => {
                if let Some(p) = &mut self.picker {
                    p.slot = slot;
                }
            }
            None => {}
        }
        if let Some((node, slot, buf)) = self.paint_hex.take() {
            // A buffer hanging off another node is not in this list at all.
            self.paint_hex = if node == anchor {
                moved(slot).map(|slot| (node, slot, buf))
            } else {
                Some((node, slot, buf))
            };
        }
    }
    /// Preview an effect's edit while a control is held and commit once when it
    /// ends — [`Self::stroke_valve`]'s twin over the effect stack.
    fn effect_valve(
        &mut self,
        resp: &egui::Response,
        subjects: &[NodeId],
        index: usize,
        edit: impl FnOnce(&mut EffectKind),
    ) {
        // The rows only draw where every subject agrees, so the anchor's stack is
        // the shared one.
        let Some(&anchor) = subjects.first() else {
            return;
        };
        let mut effects = match self.session.display_node(anchor) {
            Some(n) => n.effects().to_vec(),
            None => return,
        };
        let anchor_list = effects.clone();
        let Some(entry) = effects.get_mut(index) else {
            return;
        };
        edit(&mut entry.kind);
        let tx = self.effects_tx(subjects, &anchor_list, &effects, |_, _| {});
        self.edit_valve(resp, tx);
    }

    /// `SetEffects` writing `edited` (an edit of `anchor`'s list) onto every subject,
    /// retargeted as [`Self::write_effects`] does, then `spare`d against that
    /// subject's own list.
    fn effects_tx(
        &mut self,
        subjects: &[NodeId],
        anchor: &[Keyed<Effect>],
        edited: &[Keyed<Effect>],
        spare: impl Fn(NodeId, &mut Vec<Keyed<Effect>>),
    ) -> Transaction {
        let session = &mut self.session;
        Transaction(
            subjects
                .iter()
                .map(|id| {
                    let own = session.doc.get(*id).map(|n| n.effects()).unwrap_or(&[]);
                    let mut effects =
                        ondin_core::item::retarget(anchor, own, edited, &mut session.ids);
                    spare(*id, &mut effects);
                    Operation::SetEffects { id: *id, effects }
                })
                .collect(),
        )
    }
    /// One of effect `index`'s numbers in the popover (`slot` an `EffectVars`
    /// place, `prefix` the field's word).
    ///
    /// A bound number on one layer is a chip; otherwise a scrub field over the
    /// anchor's resolved number, whose write clears the slot's binding on every subject.
    #[allow(clippy::too_many_arguments)]
    fn effect_number_field(
        &mut self,
        ui: &mut egui::Ui,
        subjects: &[NodeId],
        index: usize,
        slot: usize,
        prefix: &'static str,
        size: egui::Vec2,
        value: f64,
        range: std::ops::RangeInclusive<f64>,
    ) -> Option<egui::Response> {
        let key = match subjects {
            [id] => self
                .session
                .doc
                .get(*id)
                .and_then(|n| n.effects().get(index))
                .map(|e| super::binding::FieldKey::EffectNumber {
                    id: *id,
                    item: e.id,
                    slot,
                }),
            _ => None,
        };
        if key.is_some_and(|k| self.bound_field_ui(ui, k, size.x, &None)) {
            return None;
        }
        // Seeded from what the layers draw: several layers bound alike store a
        // literal 0 and show no chip.
        let anchor = subjects.first().copied();
        let drawn = anchor
            .and_then(|a| {
                let d = self.session.display_node(a)?;
                let fx = self
                    .session
                    .doc
                    .resolved_effects(a, d.effects())
                    .into_owned();
                fx.get(index)?.numbers()[slot]
            })
            .unwrap_or(value);
        let mut v = drawn;
        let resp = value_field(
            ui,
            size,
            Prefix::Text(prefix),
            &mut v,
            Scrub::whole(0.5).range(range),
            |d| d.custom_formatter(ui::number(2)),
        );
        // Unmoved from the committed number (not `drawn`, which on a release frame
        // is the preview) writes nothing, or every bound layer would detach. Moved,
        // a layer already drawing the number keeps its binding.
        let committed = anchor.and_then(|a| {
            let n = self.session.doc.get(a)?;
            let fx = self.session.doc.resolved_effects(a, n.effects());
            fx.get(index)?.numbers()[slot]
        });
        if let Some(a) = anchor
            && let Some(anchor_list) = self.session.display_node(a).map(|n| n.effects().to_vec())
            && anchor_list.get(index).is_some()
        {
            let tx = if Some(v) == committed {
                Transaction::default()
            } else {
                let mut edited = anchor_list.clone();
                edited[index].value.set_number(slot, v);
                edited[index].vars[slot] = None;
                let doc = &self.session.doc;
                let keep: Vec<(NodeId, ondin_core::VarId, f64)> = subjects
                    .iter()
                    .filter_map(|id| {
                        let own = doc.get(*id)?.effects().get(index)?;
                        let var = own.vars[slot]?;
                        let shown = doc
                            .resolved_effects(*id, std::slice::from_ref(own))
                            .first()
                            .and_then(|e| e.numbers()[slot]);
                        (shown == Some(v)).then_some((*id, var, own.numbers()[slot]?))
                    })
                    .collect();
                self.effects_tx(subjects, &anchor_list, &edited, |id, list| {
                    if let Some((_, var, literal)) = keep.iter().find(|k| k.0 == id)
                        && let Some(e) = list.get_mut(index)
                    {
                        e.value.set_number(slot, *literal);
                        e.vars[slot] = Some(*var);
                    }
                })
            };
            self.edit_valve(&resp, tx);
        }
        if let Some(k) = key {
            self.field_doors(ui, k, &resp);
        }
        Some(resp)
    }
    /// One effect row: the kind's glyph and name, one headline number, a `×`, and an eye.
    ///
    /// Returns what was pressed ([`EffectRowOut`]) and the field's response, the
    /// popover's anchor and [`super::dismissed_by_click`]'s head. No `self`; `item`
    /// is the instance's trailing slot.
    fn effect_row(
        ui: &mut egui::Ui,
        index: usize,
        effect: &Effect,
        open: bool,
        item: Option<ondin_core::reset::ItemState>,
        reorderable: bool,
    ) -> (EffectRowOut, egui::Response) {
        /// The `×`'s box, the paint rows' size so the cards' right edges line up.
        const CLOSE_W: f32 = 18.0;
        let mut out = EffectRowOut::default();
        let mut grip_right = f32::NEG_INFINITY;
        let head = ui
            .scope(|ui| {
                ui.multiply_opacity(if effect.visible { 1.0 } else { HIDDEN_OPACITY });
                ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                    let size = egui::vec2(ui.available_width() - 28.0 - ui::CARD_COL_GAP, 28.0);
                    let hot =
                        ui.rect_contains_pointer(egui::Rect::from_min_size(ui.cursor().min, size));
                    let field = ui.scope(|ui| {
                        field_row_active(ui, size, open, |ui| {
                            ui.spacing_mut().item_spacing.x = 9.0;
                            // The paint rows' grip; only the start is read, since a
                            // carried row's responses are empty.
                            let grip = ui::grip(ui, reorderable, size.y);
                            out.grabbed = grip.drag_started();
                            grip_right = grip.rect.right();
                            ui.label(theme::icon_text(
                                effect_glyph(&effect.kind),
                                15.0,
                                theme::text::MUTED,
                            ));
                            ui.label(
                                egui::RichText::new(effect.kind.label())
                                    .size(12.0)
                                    .color(theme::text::STRONG),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if let Some(state) = item {
                                        out.reset = super::component::item_slot(ui, state, hot);
                                    }
                                    if icon_button(ui, icon::X, CLOSE_W, 13.0, false, true)
                                        .on_hover_text("Remove")
                                        .clicked()
                                    {
                                        out.remove = true;
                                    }
                                    ui.label(
                                        egui::RichText::new(effect_summary(&effect.kind))
                                            .size(11.0)
                                            .color(theme::text::DIM),
                                    );
                                },
                            );
                        });
                    });
                    // The whole row opens the popover, minus the `×`'s corner: this
                    // later claimant would otherwise win every press on the button.
                    // Nor the grip's, which is a drag of its own.
                    let mut hit = field.response.rect;
                    hit.min.x = hit.min.x.max(grip_right);
                    hit.max.x -= CLOSE_W + ui::FIELD_PAD_X;
                    if item.is_some() {
                        hit.max.x -= 14.0 + 9.0;
                    }
                    let resp = ui
                        .interact(
                            hit,
                            ui.id().with(("effect-row", index)),
                            egui::Sense::click(),
                        )
                        .on_hover_text(effect_hint(&effect.kind));
                    if resp.clicked() {
                        out.open = true;
                    }
                    let (glyph, tip) = if effect.visible {
                        (icon::EYE, "Hide this effect")
                    } else {
                        (icon::EYE_SLASH, "Show this effect")
                    };
                    if ui::field_button(ui, glyph, 28.0, 14.0, ui::FieldButton::Off)
                        .on_hover_text(tip)
                        .clicked()
                    {
                        out.toggle = true;
                    }
                    resp
                })
                .inner
            })
            .inner;
        (out, head)
    }
    /// One effect's numbers, behind its row.
    ///
    /// An `Area` in the popover lane ([`OndinApp::popover_anchor`]), so opening it
    /// does not push the card down. No `Blend` dropdown: blend modes are not built.
    fn effect_menu_popup(
        &mut self,
        ui: &mut egui::Ui,
        head: &egui::Response,
        subjects: &[NodeId],
        index: usize,
        effect: &Effect,
    ) {
        let Some(&id) = subjects.first() else {
            return;
        };
        const MENU_W: f32 = crate::app::POPOVER_W;
        const MENU_PAD: f32 = 11.0;
        /// The control height inside a popover.
        const CELL: f32 = ui::CONTROL_H;
        /// Between two fields on one line, as in the stroke popover.
        const COL: f32 = 6.0;
        /// The largest a `Filters` factor goes: twice its neutral, so the
        /// centre-growing meter is the same scale on both sides.
        const MAX_FACTOR: f64 = 2.0;
        /// Half a turn either way, symmetric for [`MAX_FACTOR`]'s reason.
        const MAX_HUE: f64 = 180.0;
        let ctx = ui.ctx().clone();
        let mut close = false;
        let mut retype: Option<EffectKind> = None;
        // The kind combo's list is its own `Area` above this one, so a pick there
        // misses `contains_pointer`; asked by layer order.
        let over_overlay = ctx
            .pointer_interact_pos()
            .and_then(|p| ctx.layer_id_at(p))
            .is_some_and(|l| l.order >= egui::Order::Foreground);
        let anchor = Self::popover_anchor(&ctx, head.rect, MENU_W);
        // The row's style lock, carried in: an `Area` does not inherit it.
        let locked = !ui.is_enabled();
        let menu = egui::Area::new(egui::Id::new(("effect-menu", index)))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            // A low row's popover would otherwise hang off the bottom.
            .constrain_to(ctx.input(|i| i.content_rect()).shrink(12.0))
            .show(&ctx, |ui| {
                ui::menu_frame(MENU_PAD).show(ui, |ui| {
                    if locked {
                        super::styles::lock(ui);
                    }
                    let inner = ui::menu_inner_w(MENU_W, MENU_PAD);
                    ui.set_width(inner);
                    ui.spacing_mut().item_spacing.y = ui::POPOVER_SECTION_GAP;
                    let half = (inner - COL) * 0.5;

                    // --- which kind of effect ----------------------------------
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = COL;
                        ui.scope(|ui| {
                            ui.spacing_mut().interact_size.y = CELL;
                            ui.spacing_mut().button_padding.y = 0.0;
                            egui::ComboBox::from_id_salt(("effect-kind", index))
                                .icon(ui::combo_chevron)
                                // `width` is what a combo paints, not what it reserves.
                                .width(inner - 18.0 - COL)
                                .selected_text(ui::glyph_and_text(
                                    effect_glyph(&effect.kind),
                                    effect.kind.label(),
                                ))
                                .show_ui(ui, |ui| {
                                    ui::menu_rows(ui);
                                    ui.spacing_mut().button_padding.y = 2.0;
                                    for kind in EffectKind::all_defaults() {
                                        let on = effect.kind.same_kind(&kind);
                                        let row = ui.selectable_label(
                                            on,
                                            ui::glyph_and_text(
                                                effect_glyph(&kind),
                                                kind.label(),
                                            ),
                                        );
                                        // `!on`: re-picking the current kind would be
                                        // an empty undo step.
                                        if row.clicked() && !on {
                                            retype = Some(kind);
                                        }
                                    }
                                });
                        });
                        if icon_button(ui, icon::X, 18.0, 13.0, false, true)
                            .on_hover_text("Close")
                            .clicked()
                        {
                            close = true;
                        }
                    });

                    // --- the numbers that kind has -----------------------------
                    match &effect.kind {
                        EffectKind::DropShadow(s) | EffectKind::InnerShadow(s) => {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = COL;
                                let cell = egui::vec2(half, CELL);
                                let offset = -MAX_OFFSET..=MAX_OFFSET;
                                // The layer's own space: the shadow rotates with it.
                                if let Some(r) = self.effect_number_field(
                                    ui, subjects, index, ondin_core::EFFECT_X, "X", cell,
                                    s.offset.x, offset.clone(),
                                ) {
                                    let _ = r.on_hover_text(
                                        "How far the shadow is thrown right, in the layer's own space",
                                    );
                                }
                                if let Some(r) = self.effect_number_field(
                                    ui, subjects, index, ondin_core::EFFECT_Y, "Y", cell,
                                    s.offset.y, offset.clone(),
                                ) {
                                    let _ = r.on_hover_text(
                                        "How far the shadow is thrown down, in the layer's own space",
                                    );
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = COL;
                                let cell = egui::vec2(half, CELL);
                                // The radius as typed, not the gaussian deviation
                                // (`effect::BLUR_DEVIATION` relates them).
                                if let Some(r) = self.effect_number_field(
                                    ui, subjects, index, ondin_core::EFFECT_BLUR, "Blur", cell,
                                    s.blur, 0.0..=MAX_BLUR,
                                ) {
                                    let _ = r.on_hover_text("How soft the shadow's edge is");
                                }
                                if let Some(r) = self.effect_number_field(
                                    ui, subjects, index, ondin_core::EFFECT_SPREAD, "Spread",
                                    cell, s.spread, -MAX_OFFSET..=MAX_OFFSET,
                                ) {
                                    let _ = r.on_hover_text(
                                        "Grow the shadow's silhouette before it is blurred — negative shrinks it",
                                    );
                                }
                            });
                            self.slot_color_row(
                                ui,
                                id,
                                PaintSlot::Effect(index),
                                egui::vec2(inner, CELL),
                                s.color,
                                "Edit this shadow's colour",
                            );
                        }
                        EffectKind::LayerBlur { radius } => {
                            if let Some(r) = self.effect_number_field(
                                ui,
                                subjects,
                                index,
                                ondin_core::EFFECT_BLUR,
                                "Radius",
                                egui::vec2(inner, CELL),
                                *radius,
                                0.0..=MAX_BLUR,
                            ) {
                                let _ = r.on_hover_text(
                                    "Blurs the layer's own drawing, children included",
                                );
                            }
                        }
                        EffectKind::Filters(f) => {
                            let f = *f;
                            for channel in ondin_core::FilterChannel::ALL {
                                let value = channel.of(&f);
                                let at_rest = value == channel.neutral();
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = COL;
                                    // Percent for a factor, degrees for the hue; the meter
                                    // is a signed fraction of the channel's reach.
                                    let (mut shown, scrub, suffix, meter) = if channel.degrees() {
                                        (
                                            value,
                                            Scrub::whole(0.25).range(-MAX_HUE..=MAX_HUE),
                                            "°",
                                            (value / MAX_HUE) as f32,
                                        )
                                    } else {
                                        (
                                            value * 100.0,
                                            Scrub::whole(0.25)
                                                .range(0.0..=MAX_FACTOR * 100.0),
                                            "%",
                                            (value - 1.0) as f32,
                                        )
                                    };
                                    let resp = ui::value_field_metered(
                                        ui,
                                        egui::vec2(inner - CELL - COL, CELL),
                                        Prefix::Text(channel.label()),
                                        meter,
                                        &mut shown,
                                        scrub,
                                        |d| d.suffix(suffix).max_decimals(0),
                                    )
                                    .on_hover_text(channel.hint());
                                    let write = if channel.degrees() {
                                        shown
                                    } else {
                                        shown / 100.0
                                    };
                                    self.effect_valve(&resp, subjects, index, move |k| {
                                        if let EffectKind::Filters(fs) = k {
                                            channel.set(fs, write);
                                        }
                                    });
                                    // A reset per channel; re-asked after the click
                                    // because a dim `icon_button` still senses one.
                                    if icon_button(
                                        ui,
                                        icon::ARROW_COUNTER_CLOCKWISE,
                                        CELL,
                                        12.0,
                                        false,
                                        !at_rest,
                                    )
                                    .on_hover_text(match at_rest {
                                        true => format!("{} is already at rest", channel.label()),
                                        false => {
                                            format!("Reset {}", channel.label().to_lowercase())
                                        }
                                    })
                                    .clicked()
                                        && !at_rest
                                    {
                                        let mut next = f;
                                        channel.set(&mut next, channel.neutral());
                                        let mut effects =
                                            match self.session.display_node(id) {
                                                Some(n) => n.effects().to_vec(),
                                                None => return,
                                            };
                                        if let Some(entry) = effects.get_mut(index) {
                                            entry.kind = EffectKind::Filters(next);
                                            self.write_effects(subjects, effects);
                                        }
                                    }
                                });
                            }
                        }
                    }
                });
            })
            .response;
        // `on_picker`: the shadow swatch's detached picker is a separate layer at
        // `Order::Middle`, so neither the overlay nor the menu term covers it.
        let on_picker = ctx
            .pointer_interact_pos()
            .and_then(|p| ctx.layer_id_at(p))
            .is_some_and(|l| l.id == super::picker::layer_id());
        // Recorded every frame, never from inside the short-circuiting `&&`.
        let press_away = super::press_began_away(&ctx, menu.layer_id.id, menu.rect, head.rect);
        if close
            || super::dismissed_by_click(super::ClickAway {
                // Primary only: a right-click cancelling a scrub is not a click away.
                clicked: ctx.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary)),
                over_overlay,
                on_menu: menu.contains_pointer(),
                on_head: head.contains_pointer(),
                on_picker,
                in_gesture: ctx.dragged_id().is_some() || self.gesture_cancelled,
                press_away,
            })
        {
            self.effect_menu = None;
        }
        // Escape closes the popover and any open kind list in one press
        // (`OndinApp::popover_heard` tells the router).
        self.popover_heard = true;
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.effect_menu = None;
        }
        // After the `Area`, whose closure holds `&mut self`.
        if let Some(kind) = retype {
            let mut effects = match self.session.display_node(id) {
                Some(n) => n.effects().to_vec(),
                None => return,
            };
            if let Some(entry) = effects.get_mut(index) {
                entry.kind = entry.kind.retyped(&kind);
                let numbers = entry.value.numbers();
                for (slot, number) in numbers.iter().enumerate() {
                    if number.is_none() {
                        entry.vars[slot] = None;
                    }
                }
                self.write_effects(subjects, effects);
            }
        }
    }
    /// A single colour on the Fill row's pattern (chip, hex, opacity) for a shadow
    /// ([`PaintSlot::Effect`]) or a layout grid ([`PaintSlot::Grid`]). `tip` names
    /// what the chip edits.
    fn slot_color_row(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        slot: PaintSlot,
        size: egui::Vec2,
        color: ondin_core::peniko::Color,
        tip: &'static str,
    ) {
        let targeted = self
            .picker
            .as_ref()
            .is_some_and(|p| p.node == id && p.slot == slot);
        let mut open = false;
        let anchor = ui.cursor().min;
        field_row_active(ui, size, targeted, |ui| {
            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
            // Outside the caller's fade, for `paint_row`'s corner-mask reason.
            let chip = ui
                .scope(|ui| {
                    ui.set_opacity(1.0);
                    swatch(
                        ui,
                        14.0,
                        color::FIELD,
                        Swatch::Solid(ui::color_to_egui(color)),
                    )
                })
                .inner;
            if chip.on_hover_text(tip).clicked() {
                open = true;
            }
            let brush = Brush::Solid(color);
            self.paint_hex_field(ui, id, slot, &brush, false);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut pct = f64::from(color.components[3]) * 100.0;
                let resp = ui::bare_drag_value(
                    ui,
                    egui::DragValue::new(&mut pct)
                        .suffix("%")
                        .speed(ui::scrub(0.5))
                        .max_decimals(0)
                        .range(0.0..=100.0),
                );
                let next = Brush::Solid(color.with_alpha((pct / 100.0) as f32));
                self.valve_slot(&resp, id, slot, next);
            });
        });
        if open {
            self.open_picker(ui, id, slot, anchor);
        }
    }
    // --- paint slot plumbing ----------------------------------------------
    /// The brush currently in a slot, read through the preview so the picker
    /// tracks a live drag (§9.3). `None` when the slot no longer exists.
    pub(crate) fn slot_brush(&self, id: NodeId, slot: PaintSlot) -> Option<Brush> {
        // Character-scoped slots first, through the write verbs' predicate; `None`
        // tells the picker its slot is gone.
        if let Some(cs) = slot.char_scoped() {
            return self.char_slot_color(id, cs).map(Brush::Solid);
        }
        match slot {
            PaintSlot::Canvas => {
                return Some(Brush::Solid(self.session.preview_ground()));
            }
            PaintSlot::VarCell(var, mode) => {
                if let Some((v, m, c)) = self.var_cell_preview
                    && (v, m) == (var, mode)
                {
                    return Some(Brush::Solid(c));
                }
                return match self.session.doc.variables().resolve(var, mode)? {
                    ondin_core::Value::Color(c) => Some(Brush::Solid(c)),
                    ondin_core::Value::Number(_)
                    | ondin_core::Value::Boolean(_)
                    | ondin_core::Value::String(_) => None,
                };
            }
            PaintSlot::Guide => {
                let guides = self.selected_guides();
                let colour =
                    shared_guide_colour(&guides).or_else(|| guides.first().map(|g| g.color()))?;
                return Some(Brush::Solid(colour));
            }
            PaintSlot::Selection(target, i) => {
                // As drawn, at the first selected layer — the list is shared, and
                // a binding resolves in the mode in force where it sits.
                let at = *self.session.selection.ids().first()?;
                let drawn = |b: &ondin_core::BrushSource| {
                    self.session.doc.resolve_brush(at, b).into_owned()
                };
                return match target {
                    PaintTarget::Fill => self
                        .fills_in_scope(PaintScope::Selection)
                        .get(i)
                        .map(|f| drawn(&f.brush)),
                    PaintTarget::Stroke => self
                        .strokes_in_scope(PaintScope::Selection)
                        .get(i)
                        .map(|s| drawn(&s.brush)),
                };
            }
            // Gone once the panel stops being mixed; `retarget_mixed_paint` has
            // usually moved the picker to the real row first.
            PaintSlot::SelectionAll(target) => {
                let mixed = match target {
                    PaintTarget::Fill => self.fills_shown(PaintScope::Selection).is_mixed(),
                    PaintTarget::Stroke => self.strokes_shown(PaintScope::Selection).is_mixed(),
                };
                return mixed.then(|| self.mixed_row(target).0);
            }
            PaintSlot::GroupColor(key) => return self.group_color_shown(key),
            // A location key: the place stays the place when its paint changes.
            PaintSlot::GroupGradient(at) => {
                return self.shown_paint_at(at);
            }
            _ => {}
        }
        let node = self.session.display_node(id)?;
        let drawn =
            |b: &ondin_core::BrushSource| self.session.doc.resolve_brush(id, b).into_owned();
        // A layer's gradients as drawn on the box layout gives it, which is the box
        // `local_box` reports and the picker and the canvas handles edit against.
        let on_box = |b: Brush| match self.paint_box_map(id) {
            Some(m) => ondin_core::paint_box::carry_brush(&b, m),
            None => b,
        };
        match slot {
            PaintSlot::Fill(i) => node.paint().fills.get(i).map(|f| on_box(drawn(&f.brush))),
            PaintSlot::Stroke(i) => node.paint().strokes.get(i).map(|s| on_box(drawn(&s.brush))),
            // `None` for a colourless kind closes a picker whose row was retyped.
            PaintSlot::Effect(i) => match node.effects().get(i).map(|e| &e.kind) {
                Some(EffectKind::DropShadow(s) | EffectKind::InnerShadow(s)) => {
                    Some(Brush::Solid(s.color))
                }
                _ => None,
            },
            // Through `grid_paint` first: `DisplayNode` has no grids, so the buffer
            // is the preview; the committed list says whether the row exists.
            PaintSlot::Grid(i) => self.session.doc.get(id)?.grids().get(i).map(|g| {
                let held = match &self.in_flight.grid_paint {
                    Some((nodes, at, color)) if nodes.contains(&id) && *at == i => Some(*color),
                    _ => None,
                };
                Brush::Solid(held.unwrap_or(g.color))
            }),
            PaintSlot::Canvas
            | PaintSlot::Guide
            | PaintSlot::Selection(..)
            | PaintSlot::SelectionAll(_)
            | PaintSlot::GroupColor(_)
            | PaintSlot::GroupGradient(_)
            | PaintSlot::TextDecoration(_)
            | PaintSlot::TextColor
            | PaintSlot::VarCell(..) => unreachable!("answered above"),
        }
    }
    /// The brush at a location, read through the preview so the picker tracks a live
    /// drag (§9.3) — `build::paint_at`'s twin over the display tree.
    fn shown_paint_at(&self, at: ondin_core::PaintAt) -> Option<Brush> {
        let node = self.session.display_node(at.node)?;
        let drawn =
            |b: &ondin_core::BrushSource| self.session.doc.resolve_brush(at.node, b).into_owned();
        match at.target {
            PaintTarget::Fill => node.paint().fills.get(at.index).map(|f| drawn(&f.brush)),
            PaintTarget::Stroke => node.paint().strokes.get(at.index).map(|s| drawn(&s.brush)),
        }
    }
    /// Whether `brush` is exactly the solid colour `key` of its own — the census
    /// lists no bound paint (`build::census`).
    fn is_key(brush: &ondin_core::BrushSource, key: [u8; 4]) -> bool {
        matches!(brush.literal(), Some(Brush::Solid(c)) if c.to_rgba8().to_u8_array() == key)
    }
    /// The layers the Group Colors census is taken over: the contents of a single
    /// selected container, or the selection itself.
    ///
    /// The listing, `recolor`, `repaint` and [`Self::group_color_shown`] must all
    /// take this scope, or a row could count one layer and recolour two.
    fn group_color_scope(&self) -> Vec<NodeId> {
        let ids = self.session.selection.ids();
        if let [only] = ids
            && let Some(node) = self.session.doc.get(*only)
            && matches!(node.kind(), NodeKind::Group | NodeKind::Artboard { .. })
        {
            return node.children().to_vec();
        }
        ids.to_vec()
    }
    /// What a Group Colors row currently looks like, following a live edit.
    ///
    /// Keyed by the committed colour, but the swatch shows the one being dragged:
    /// the committed document finds the colour's first place, the preview is read there.
    fn group_color_shown(&self, key: [u8; 4]) -> Option<Brush> {
        // The census's scope, or this could find the colour on a layer the row
        // does not stand for.
        for id in ondin_core::subtree_nodes(&self.session.doc, &self.group_color_scope()) {
            let Some(committed) = self.session.doc.get(id) else {
                continue;
            };
            let Some(shown) = self.session.display_node(id) else {
                continue;
            };
            if let Some(i) = committed
                .paint()
                .fills
                .iter()
                .position(|f| Self::is_key(&f.brush, key))
            {
                return shown
                    .paint()
                    .fills
                    .get(i)
                    .map(|f| self.session.doc.resolve_brush(id, &f.brush).into_owned())
                    .or(Some(Brush::Solid(key_color(key))));
            }
            if let Some(i) = committed
                .paint()
                .strokes
                .iter()
                .position(|s| Self::is_key(&s.brush, key))
            {
                return shown
                    .paint()
                    .strokes
                    .get(i)
                    .map(|s| self.session.doc.resolve_brush(id, &s.brush).into_owned())
                    .or(Some(Brush::Solid(key_color(key))));
            }
        }
        None
    }
    /// The selected guides as they should be shown, with any live edit read
    /// through them. Empty unless guides are what is selected.
    pub(crate) fn selected_guides(&self) -> Vec<ondin_core::Guide> {
        self.session
            .selection
            .guides()
            .iter()
            .filter_map(|id| self.session.doc.guide(*id))
            .map(|g| self.shown_guide(g))
            .collect()
    }
    /// Remove every selected guide, as one transaction.
    ///
    /// Refuses, out loud, while the guides are hidden: the selection survives
    /// hiding, so `Delete` would destroy a guide with no line on screen.
    pub(crate) fn remove_selected_guides(&mut self) {
        if !self.guides_on() {
            self.session
                .info("Guides are hidden — show them to delete one".to_string());
            return;
        }
        let ops: Vec<Operation> = self
            .session
            .selection
            .guides()
            .iter()
            .map(|id| Operation::RemoveGuide { id: *id })
            .collect();
        let n = ops.len();
        // `commit_edit`, not `session.commit`: panels commit through it.
        if n > 0 && self.commit_edit(Transaction(ops)) {
            self.session.info(match n {
                1 => "Guide removed".to_string(),
                n => format!("Removed {n} guides"),
            });
        }
    }
    /// The transaction that puts `brush` in a slot, preserving everything else
    /// about the paint list (a stroke's width, the other fills).
    pub(crate) fn slot_transaction(
        &self,
        id: NodeId,
        slot: PaintSlot,
        brush: Brush,
    ) -> Transaction {
        let Some(node) = self.session.display_node(id) else {
            return Transaction(Vec::new());
        };
        // `brush` is what the slot is to draw; `edited_brush` keeps a binding when
        // only its `%` moved and detaches it otherwise.
        let doc = &self.session.doc;
        match slot {
            // `brush` is on the drawn box (`slot_brush`), stored back on the stored one.
            PaintSlot::Fill(i) => {
                let mut fills = node.paint().fills.clone();
                match fills.get_mut(i) {
                    Some(f) => {
                        let brush = self.stored_on_box(id, &f.brush, brush);
                        f.brush = ondin_core::edited_brush(doc, id, &f.brush, |_| brush.clone())
                    }
                    None => return Transaction(Vec::new()),
                }
                Transaction(vec![Operation::SetFills { id, fills }])
            }
            PaintSlot::Stroke(i) => {
                let mut strokes = node.paint().strokes.clone();
                match strokes.get_mut(i) {
                    Some(s) => {
                        let brush = self.stored_on_box(id, &s.brush, brush);
                        s.brush = ondin_core::edited_brush(doc, id, &s.brush, |_| brush.clone())
                    }
                    None => return Transaction(Vec::new()),
                }
                Transaction(vec![Operation::SetStrokes { id, strokes }])
            }
            // Opaque: there is nothing behind the canvas to show through.
            PaintSlot::Canvas => match paint::stops_of(&brush).first() {
                Some((_, colour)) => Transaction(vec![Operation::SetPageGround {
                    id: self.session.page,
                    ground: colour.with_alpha(1.0),
                }]),
                None => Transaction(Vec::new()),
            },
            // A variable's cell takes the colour, alpha and all, ending an alias it
            // held; a gradient is refused.
            PaintSlot::VarCell(var, mode) => match brush {
                Brush::Solid(c) => Transaction(vec![Operation::SetVariableCell {
                    id: var,
                    mode,
                    cell: ondin_core::Cell::Color(c),
                }]),
                _ => Transaction(Vec::new()),
            },
            PaintSlot::Selection(target, i) => {
                // Row `i` on each target in place, so each keeps its item ids. A
                // `%`-only change keeps each target's own drawn colour, which may
                // differ from the anchor's in another mode.
                let ids = self.session.selection.ids();
                let doc = &self.session.doc;
                let committed = ids.first().and_then(|at| {
                    let paint = doc.get(*at)?.paint();
                    let b = match target {
                        PaintTarget::Fill => &paint.fills.get(i)?.brush,
                        PaintTarget::Stroke => &paint.strokes.get(i)?.brush,
                    };
                    Some(doc.resolve_brush(*at, b).into_owned())
                });
                let rgb = |c: &ondin_core::peniko::Color| {
                    let [r, g, b, _] = c.to_rgba8().to_u8_array();
                    [r, g, b]
                };
                let alpha_only = match (&committed, &brush) {
                    (Some(Brush::Solid(was)), Brush::Solid(now)) => rgb(was) == rgb(now),
                    _ => false,
                };
                ondin_core::edit_brush_at_all(doc, ids, target, i, |drawn| match drawn {
                    Brush::Solid(c) if alpha_only => {
                        let Brush::Solid(now) = &brush else {
                            unreachable!()
                        };
                        Brush::Solid(c.with_alpha(now.components[3]))
                    }
                    _ => brush.clone(),
                })
            }
            PaintSlot::SelectionAll(target) => self.edit_paints_all(target, |_| brush.clone()),
            // Recolour by colour: every use of the key. A non-solid brush is
            // dropped, not flattened.
            PaintSlot::GroupColor(key) => match &brush {
                Brush::Solid(to) => ondin_core::recolor(
                    &self.session.doc,
                    &self.group_color_scope(),
                    key_color(key),
                    *to,
                ),
                _ => Transaction(Vec::new()),
            },
            // The same edit by value: `from` is read from the committed place, since
            // mid-drag the preview already holds the new brush.
            PaintSlot::GroupGradient(at) => match ondin_core::paint_at(&self.session.doc, at)
                .as_ref()
                .and_then(ondin_core::BrushSource::literal)
            {
                Some(from) => {
                    ondin_core::repaint(&self.session.doc, &self.group_color_scope(), from, &brush)
                }
                None => Transaction(Vec::new()),
            },
            // Diverted to `write_char_slot` by every slot verb before this; empty
            // rather than `unreachable!`, so being wrong is not a panic.
            PaintSlot::TextDecoration(_) | PaintSlot::TextColor => Transaction(Vec::new()),
            // Opaque (`PaintSlot::takes_alpha`), on every selected guide.
            PaintSlot::Guide => match paint::stops_of(&brush).first() {
                Some((_, colour)) => Transaction(
                    self.session
                        .selection
                        .guides()
                        .iter()
                        .map(|id| Operation::SetGuideColor {
                            id: *id,
                            color: Some(colour.with_alpha(1.0)),
                        })
                        .collect(),
                ),
                None => Transaction(Vec::new()),
            },
            // A shadow's colour, alpha included, on every selected layer's own list. A
            // subject whose entry `i` is not a shadow is skipped: the detached picker
            // can outlive the card's agreement and must change one colour, not copy the stack.
            PaintSlot::Effect(i) => match paint::stops_of(&brush).first() {
                Some((_, colour)) => {
                    // The anchor through `display_node`, so a live preview is what
                    // the picker edits; other subjects committed.
                    let anchor_ok = self
                        .session
                        .display_node(id)
                        .and_then(|n| n.effects().get(i).map(|e| e.kind.clone()))
                        .is_some_and(|k| {
                            matches!(k, EffectKind::DropShadow(_) | EffectKind::InnerShadow(_))
                        });
                    // Retyped or removed under the picker, which `slot_brush`
                    // closes next frame; write nothing meanwhile.
                    if !anchor_ok {
                        return Transaction(Vec::new());
                    }
                    let subjects = self.session.selection.ids().to_vec();
                    Transaction(
                        subjects
                            .iter()
                            .filter_map(|s| {
                                let mut effects = if *s == id {
                                    self.session.display_node(*s)?.effects().to_vec()
                                } else {
                                    self.session.doc.get(*s)?.effects().to_vec()
                                };
                                match effects.get_mut(i).map(|e| &mut e.kind) {
                                    Some(
                                        EffectKind::DropShadow(sh) | EffectKind::InnerShadow(sh),
                                    ) => sh.color = *colour,
                                    _ => return None,
                                }
                                Some(Operation::SetEffects { id: *s, effects })
                            })
                            .collect(),
                    )
                }
                None => Transaction(Vec::new()),
            },
            // A grid's colour (alpha is the wash's strength) on row `i` of every selected
            // frame, from the committed document: the override tree cannot carry a grid.
            PaintSlot::Grid(i) => match paint::stops_of(&brush).first() {
                Some((_, colour)) => {
                    let subjects = self.frame_subjects();
                    let ops: Vec<Operation> = subjects
                        .iter()
                        .filter_map(|s| {
                            let mut grids = self.session.doc.get(*s)?.grids().to_vec();
                            grids.get_mut(i)?.color = *colour;
                            Some(Operation::SetLayoutGrids { id: *s, grids })
                        })
                        .collect();
                    Transaction(ops)
                }
                None => Transaction(Vec::new()),
            },
        }
    }
    /// Commit a slot edit as one undo step. For discrete choices (a kind tab, a
    /// swatch); drags go through [`Self::valve_slot`].
    pub(crate) fn write_slot(&mut self, id: NodeId, slot: PaintSlot, brush: Brush) {
        if let Some(cs) = slot.char_scoped() {
            self.write_char_slot(id, cs, &brush, super::paint::CharWrite::Commit);
            return;
        }
        // Cloned: the retarget below reads the brush after the commit.
        let tx = self.slot_transaction(id, slot, brush.clone());
        self.commit_edit(tx);
        // The grid preview buffer ends with the commit, as `edit_valve` drops it
        // for a drag.
        self.in_flight.grid_paint = None;
        self.retarget_after(slot, &brush);
    }
    /// Show a slot edit without committing it — the third slot verb, for the frames a
    /// pointer is **down** on a colour control but has not yet travelled far enough
    /// for egui to call it a drag. Returns whether it had anything to preview.
    pub(crate) fn preview_slot(&mut self, id: NodeId, slot: PaintSlot, brush: Brush) -> bool {
        if let Some(cs) = slot.char_scoped() {
            return self.write_char_slot(id, cs, &brush, super::paint::CharWrite::Preview);
        }
        let tx = self.slot_transaction(id, slot, brush);
        self.session.set_preview(&tx);
        // Guides, variable cells and grids have no render override, so each has
        // its own preview fork, as in `edit_valve`.
        self.preview_guide(&tx);
        self.preview_var_cell(&tx);
        self.preview_grid(&tx);
        // The answer, not `true`: `picker::pointer_slot` falls through to the
        // valve on a refused write, rather than arming the chrome retreat.
        !tx.0.is_empty()
    }
    /// Preview a slot edit while a control is dragged, commit once on release.
    /// Whether it committed this frame; for a character slot, whether the gesture
    /// ended, its session deciding the commit.
    pub(crate) fn valve_slot(
        &mut self,
        resp: &egui::Response,
        id: NodeId,
        slot: PaintSlot,
        brush: Brush,
    ) -> bool {
        // A character attribute has its own valve: a live editing session is the
        // preview and commits at its end (§9.3).
        if let Some(cs) = slot.char_scoped() {
            self.write_char_slot(id, cs, &brush, super::paint::CharWrite::Valve(resp));
            return resp.drag_stopped() || ui::defocus_commits(resp);
        }
        let tx = self.slot_transaction(id, slot, brush.clone());
        // Asked of the valve, not predicted from the response: a prediction would
        // retarget on every keystroke of a typed colour.
        let committing = self.edit_valve(resp, tx);
        // A Group Colors row is keyed by the colour it replaces; once that commits,
        // the row and its picker move onto the new colour or the picker would close.
        if committing {
            self.retarget_after(slot, &brush);
        }
        committing
    }

    /// Follow the picker onto whatever the edit just turned its slot into, so the
    /// click (`write_slot`) and the drag (`valve_slot`) cannot disagree.
    ///
    /// A `GroupColor` key is the colour, so changing it renames the row; a
    /// `SelectionAll` slot stands for disagreement, so agreement deletes it.
    fn retarget_after(&mut self, slot: PaintSlot, brush: &Brush) {
        if let (PaintSlot::GroupColor(key), Brush::Solid(c)) = (slot, brush) {
            let next = c.to_rgba8().to_u8_array();
            if next != key {
                self.retarget_group_color(key, next);
            }
        }
        if let PaintSlot::SelectionAll(target) = slot {
            self.retarget_mixed_paint(target);
        }
    }
    /// Follow the picker off the mixed row and onto row 0 of the list that
    /// replaced it.
    fn retarget_mixed_paint(&mut self, target: PaintTarget) {
        let shown = match target {
            PaintTarget::Fill => self.fills_shown(PaintScope::Selection).is_mixed(),
            PaintTarget::Stroke => self.strokes_shown(PaintScope::Selection).is_mixed(),
        };
        let rows = match target {
            PaintTarget::Fill => self.fills_in_scope(PaintScope::Selection).len(),
            PaintTarget::Stroke => self.strokes_in_scope(PaintScope::Selection).len(),
        };
        // Still mixed: the slot is right. No rows: `slot_brush` closes it.
        if shown || rows == 0 {
            return;
        }
        let from = PaintSlot::SelectionAll(target);
        let to = PaintSlot::Selection(target, 0);
        if let Some(p) = &mut self.picker
            && p.slot == from
        {
            p.slot = to;
        }
        if let Some((_, slot, _)) = &mut self.paint_hex
            && *slot == from
        {
            *slot = to;
        }
    }
    /// Follow a Group Colors row from the colour it was to the colour it is.
    fn retarget_group_color(&mut self, from: [u8; 4], to: [u8; 4]) {
        if let Some(p) = &mut self.picker
            && p.slot == PaintSlot::GroupColor(from)
        {
            p.slot = PaintSlot::GroupColor(to);
        }
        if let Some((_, slot, _)) = &mut self.paint_hex
            && *slot == PaintSlot::GroupColor(from)
        {
            *slot = PaintSlot::GroupColor(to);
        }
    }
    /// Replace a scope's fill (or stroke) list outright — the panel's one list writer.
    ///
    /// Over a selection `list` is an edit of the shared `anchor`, which `all`
    /// retargets onto each target's own item ids. No empty-transaction guard:
    /// `commit_inner` drops one.
    fn write_paint_list<P>(
        &mut self,
        scope: PaintScope,
        list: Vec<Keyed<P>>,
        anchor: Vec<Keyed<P>>,
        one: impl Fn(NodeId, Vec<Keyed<P>>) -> Operation,
        all: impl Fn(
            &ondin_core::Document,
            &[NodeId],
            &[Keyed<P>],
            &[Keyed<P>],
            &mut ondin_core::IdSource,
        ) -> Transaction,
    ) {
        let tx = match scope {
            PaintScope::Node(id) => Transaction(vec![one(id, list)]),
            PaintScope::Selection => all(
                &self.session.doc,
                self.session.selection.ids(),
                &anchor,
                &list,
                &mut self.session.ids,
            ),
        };
        self.commit_edit(tx);
    }
    /// The layers a Fill or Stroke card in `scope` writes — what its style is
    /// read off and applied to.
    fn paint_style_targets(&self, scope: PaintScope) -> Vec<NodeId> {
        match scope {
            PaintScope::Node(id) => vec![id],
            PaintScope::Selection => {
                ondin_core::build::paint_targets(&self.session.doc, self.session.selection.ids())
            }
        }
    }
    fn write_fill_list(&mut self, scope: PaintScope, fills: Vec<Keyed<Fill>>) {
        let anchor = self.fills_in_scope(scope);
        self.write_paint_list(
            scope,
            fills,
            anchor,
            |id, fills| Operation::SetFills { id, fills },
            ondin_core::retarget_fills_all,
        );
    }
    fn write_strokes(&mut self, scope: PaintScope, strokes: Vec<Keyed<Stroke>>) {
        let anchor = self.strokes_in_scope(scope);
        self.write_paint_list(
            scope,
            strokes,
            anchor,
            |id, strokes| Operation::SetStrokes { id, strokes },
            ondin_core::retarget_strokes_all,
        );
    }
    /// Open (or retarget) the detached picker on a slot.
    ///
    /// A new picker lands just left of `anchor`, the top-left of the row it came
    /// from; where it was last dragged is not remembered (`Picker::spawn`). An
    /// already-open picker keeps its position and is retargeted.
    pub(crate) fn open_picker(
        &mut self,
        ui: &egui::Ui,
        id: NodeId,
        slot: PaintSlot,
        anchor: egui::Pos2,
    ) {
        match &mut self.picker {
            Some(p) => p.retarget(id, slot),
            None => {
                const GAP: f32 = 22.0;
                let screen = ui.ctx().input(|i| i.content_rect());
                let left = screen.min.x + 12.0;
                let top = screen.min.y + 12.0;
                // Left of the row and of any open popover (`picker_lane_right`).
                let from = self.picker_lane_right(ui.ctx(), anchor.x);
                let spawn = egui::pos2(
                    (from - super::picker::WIDTH - GAP).max(left),
                    // A little above the row, clear of the bottom edge.
                    (anchor.y - 40.0)
                        .clamp(top, (screen.max.y - super::picker::HEIGHT_HINT).max(top)),
                );
                self.picker = Some(Picker::new(id, slot, spawn));
            }
        }
    }
    // --- panels -----------------------------------------------------------
    /// A floating, collapsible panel. `title` doubles as the collapse key, so
    /// the state survives changing selection. Returns whether the header's
    /// action icon was clicked.
    ///
    /// Open, the header row is the toggle target ([`ui::section_head`]); collapsed,
    /// the whole card is. Never both, or one click would toggle twice.
    pub(super) fn panel(
        &mut self,
        ui: &mut egui::Ui,
        title: &'static str,
        action: Option<ui::HeadAction>,
        body: impl FnOnce(&mut Self, &mut egui::Ui),
    ) -> bool {
        self.panel_badged(ui, title, action, None, body)
    }

    /// [`Self::panel`] with a badge in its header (`ui::section_head_badged`) —
    /// the Position card's *Absolute*.
    pub(super) fn panel_badged(
        &mut self,
        ui: &mut egui::Ui,
        title: &'static str,
        action: Option<ui::HeadAction>,
        badge: Option<&str>,
        body: impl FnOnce(&mut Self, &mut egui::Ui),
    ) -> bool {
        self.panel_head(ui, title, None, action, badge, body).acted
    }

    /// [`Self::panel_badged`], answering with the whole of the header's clicks (for a
    /// header `+` that opens a menu) and with `size`, a count after the title
    /// (*Variants 4*). The title alone stays the card's key.
    pub(super) fn panel_head(
        &mut self,
        ui: &mut egui::Ui,
        title: &'static str,
        size: Option<usize>,
        action: Option<ui::HeadAction>,
        badge: Option<&str>,
        body: impl FnOnce(&mut Self, &mut egui::Ui),
    ) -> ui::HeadClicks {
        self.panel_full(ui, title, size, action, badge, false, body)
    }

    /// [`Self::panel_head`] with a `⋯` after the action icon when `more` — a
    /// style slot's card, its click in [`ui::HeadClicks::more`].
    #[allow(clippy::too_many_arguments)]
    pub(super) fn panel_full(
        &mut self,
        ui: &mut egui::Ui,
        title: &'static str,
        size: Option<usize>,
        action: Option<ui::HeadAction>,
        badge: Option<&str>,
        more: bool,
        body: impl FnOnce(&mut Self, &mut egui::Ui),
    ) -> ui::HeadClicks {
        let shown = match size {
            Some(n) => format!("{title} {n}"),
            None => title.to_string(),
        };
        // What the palette's *Show ‹card›* rows read next frame.
        self.cards_drawn.push(title);
        self.cards_seen.insert(title);
        let reveal = self.reveal_card == Some(title);
        if reveal {
            open_paint_panel(
                &mut self.collapsed_panels,
                &mut self.paint_auto_collapsed,
                title,
            );
        }
        let open = !self.collapsed_panels.contains(title);
        let (count, resets) = self
            .card_override(title)
            .map(|(n, ops)| (n, ops.to_vec()))
            .unwrap_or_default();
        let reset_label = format!("Reset {}", title.to_lowercase());
        let overrides = (count > 0).then_some(ui::HeadOverrides {
            count,
            reset: &reset_label,
        });
        // Salted by title, not position: the panel list changes with the subject,
        // and an index-derived id would clash for a frame (egui's red flash).
        let (mut clicks, card_rect) = ui
            .push_id(title, |ui| {
                ui::card_at(ui, |ui| {
                    let clicks = ui::section_head_full(
                        ui, &shown, open, action, badge, overrides, more, open,
                    );
                    if open {
                        body(self, ui);
                    }
                    clicks
                })
            })
            .inner;
        if reveal {
            ui.scroll_to_rect(card_rect, Some(egui::Align::TOP));
            if let Some(field) = first_field(ui, card_rect) {
                ui.memory_mut(|m| m.request_focus(field));
            }
            self.reveal_card = None;
        }
        if !open {
            // Everything but the action icon's corner, so a collapsed card can take
            // its `+`; the later claimant would otherwise win every press there.
            let mut hit = card_rect;
            if action.is_some() || more {
                hit.max.x -= ui::HEAD_ACTION_W + ui::HEAD_ACTION_PAD + ui::CARD_MARGIN_X;
            }
            if action.is_some() && more {
                hit.max.x -= ui::HEAD_MORE_W;
            }
            let whole = ui.interact(
                hit,
                ui.id().with(("panel-card", title)),
                egui::Sense::click(),
            );
            clicks.toggled = whole.clicked();
        }
        if clicks.toggled {
            if open {
                self.collapsed_panels.insert(title);
            } else {
                self.collapsed_panels.remove(title);
            }
        }
        if clicks.reset {
            self.commit_reset(Transaction(resets));
        }
        clicks
    }
    /// What `paint::with_fit` needs to seed a crop: the frame this picture is
    /// shown in, and the picture's intrinsic size.
    ///
    /// `None` where it is unknowable (no node, lost image), and for a slot spanning
    /// a selection: a write there sends one brush to every node, so seeding from
    /// the anchor would frame differently-shaped layers as the anchor.
    fn crop_seed(
        &self,
        id: NodeId,
        slot: paint::PaintSlot,
        brush: &Brush,
    ) -> Option<(ondin_core::kurbo::Rect, u32, u32)> {
        if matches!(
            slot,
            paint::PaintSlot::Selection(..) | paint::PaintSlot::SelectionAll(..)
        ) {
            return None;
        }
        let ondin_core::Brush::Image(img) = brush else {
            return None;
        };
        let e = self.session.doc.image(&img.image.id)?;
        let node = self.session.display_node(id)?;
        let frame =
            ondin_core::geometry::local_bounds(node.kind(), self.session.preview_text_layout(id))?;
        Some((frame, e.width, e.height))
    }

    /// The map from the box `id`'s paint is stored against (`paint_box::paint_box`)
    /// to the one it draws on, as the scene walk draws it (`paint_box::drawn_paint`);
    /// `None` when they agree.
    pub(crate) fn paint_box_map(&self, id: NodeId) -> Option<ondin_core::kurbo::Affine> {
        let node = self.session.display_node(id)?;
        let vars = self.session.doc.get(id)?.size_vars();
        let stored = ondin_core::paint_box::paint_box(&self.session.doc, vars, node.stored_kind())?;
        ondin_core::paint_box::box_map(stored, node.kind().box_size()?)
            .filter(|m| ondin_core::build::affine_is_invertible(*m))
    }

    /// `drawn`, a brush on the drawn box, as `id` stores it: mapped back through
    /// [`Self::paint_box_map`]. A transform the edit left alone is `held`'s own,
    /// exactly, so a colour edit leaves no rounding behind in it.
    fn stored_on_box(&self, id: NodeId, held: &ondin_core::BrushSource, drawn: Brush) -> Brush {
        let (Some(m), Brush::Gradient(g)) = (self.paint_box_map(id), &drawn) else {
            return drawn;
        };
        if let Some(Brush::Gradient(h)) = held.literal()
            && m * h.transform == g.transform
        {
            let mut g = g.clone();
            g.transform = h.transform;
            return Brush::Gradient(g);
        }
        ondin_core::paint_box::carry_brush(&drawn, m.inverse())
    }

    /// The node's local geometry box, used to place gradient coordinates.
    pub(crate) fn local_box(&self, id: NodeId) -> Option<Size> {
        let node = self.session.display_node(id)?;
        let rect =
            ondin_core::geometry::local_bounds(node.kind(), self.session.preview_text_layout(id))?;
        Some(Size::new(rect.width(), rect.height()))
    }
    /// A transaction putting `id` at `world`, projected back to the local
    /// transform the model actually stores (§5.7a).
    fn place_at(&self, id: NodeId, world: Affine) -> Transaction {
        build::place_at_world(&self.session.doc, &self.session.resolved, id, world)
            .unwrap_or_default()
    }
    // --- multi-selection actions ------------------------------------------
    /// The box the selection aligns against: the key layer's box when one is
    /// designated, the selection's union when several layers are picked and none
    /// is, and the container's box when there is one. `None` for a lone top-level
    /// layer, where the only candidate is the whole document.
    fn align_target(&self) -> Option<ondin_core::kurbo::Rect> {
        let ids = build::outermost(&self.session.doc, self.session.selection.ids());
        let res = &self.session.resolved;
        match ids.len() {
            0 => None,
            1 => {
                let parent = self.session.doc.get(ids[0]).and_then(|n| n.parent())?;
                if parent == self.session.page {
                    return None;
                }
                res.world_bounds(parent)
            }
            // The key must survive `outermost`: a key nested in another selected layer is
            // no operand, and aligning to a box inside a member moves its ancestor oddly.
            _ => match self.session.selection.key().filter(|k| ids.contains(k)) {
                Some(key) => res.world_bounds(key),
                None => ids
                    .iter()
                    .filter_map(|id| res.world_bounds(*id))
                    .reduce(|a, b| a.union(b)),
            },
        }
    }
    /// Align the selection along one axis, against [`Self::align_target`].
    pub(crate) fn align(&mut self, axis: Axis, edge: Edge) {
        let Some(target) = self.align_target() else {
            return;
        };
        let ids = self.session.selection.ids().to_vec();
        let tx = build::align(
            &self.session.doc,
            &self.session.resolved,
            &ids,
            target,
            axis,
            edge,
        );
        if !tx.0.is_empty() && self.commit_edit(tx) {
            self.session.info("Aligned");
        }
    }
    /// Mirror the selection across the centreline of the box it occupies.
    pub(crate) fn flip(&mut self, axis: Axis) {
        let ids = self.session.selection.ids().to_vec();
        let tx = build::flip(&self.session.doc, &self.session.resolved, &ids, axis);
        if !tx.0.is_empty() && self.commit_edit(tx) {
            self.session.info(match axis {
                Axis::X => "Flipped horizontally",
                Axis::Y => "Flipped vertically",
            });
        }
    }
    /// Space the selection evenly along one axis.
    pub(crate) fn distribute(&mut self, axis: Axis) {
        let ids = self.session.selection.ids().to_vec();
        let tx = build::distribute(&self.session.doc, &self.session.resolved, &ids, axis);
        if tx.0.is_empty() {
            self.session.info("Already evenly spaced");
            return;
        }
        if self.commit_edit(tx) {
            self.session.info("Distributed");
        }
    }
    /// Lay the selection out as the grid it is roughly in (`build::tidy_up`).
    pub(crate) fn tidy_up(&mut self) {
        let ids = self.session.selection.ids().to_vec();
        if ids.len() < 2 {
            self.session.info("Select two or more layers to tidy");
            return;
        }
        let tx = build::tidy_up(&self.session.doc, &self.session.resolved, &ids);
        if tx.0.is_empty() {
            self.session.info("Already tidy");
            return;
        }
        if self.commit_edit(tx) {
            self.session.info("Tidied up");
        }
    }
    /// The transaction one numeric-spacing edit produces for the layer subject
    /// (`build::distribute_spacing`, held at the key layer).
    ///
    /// Read off the committed tree, as `resize_tx` does: the target is absolute, so
    /// a base re-derived from the scrub's own preview would compound.
    pub(crate) fn spacing_tx(&self, axis: Axis, gap: f64) -> Transaction {
        let ids = self.session.selection.ids().to_vec();
        build::distribute_spacing(
            &self.session.doc,
            &self.session.resolved,
            &ids,
            axis,
            self.session.selection.key(),
            gap,
        )
    }
}

/// The gaps the spacing field shows over the selection, read through the preview.
///
/// A scrubbed field re-reads its base every frame, so a base from the committed
/// tree cannot move while the drag previews. An edit computed from a fixed origin
/// (`OndinApp::spacing_tx`) wants the committed tree. Free for headless tests.
pub(crate) fn layer_spacings(session: &EditorSession, axis: Axis) -> Vec<f64> {
    let boxes: Vec<ondin_core::kurbo::Rect> =
        build::outermost(&session.doc, session.selection.ids())
            .into_iter()
            .filter_map(|id| session.preview_world_bounds(id))
            .collect();
    build::gaps_along(&boxes, axis)
}

/// What the spacing field shows, and what a drag on it starts from.
///
/// The first is `None` (Mixed) unless every gap agrees to the precision the field
/// prints (half its last digit); the second is the mean. Not [`Spread`]: that
/// agrees by `PartialEq` (Mixed forever over dragged gaps).
pub(crate) fn shown_gap(gaps: &[f64]) -> (Option<f64>, f64) {
    /// Half of the last digit `ui::number(2)` prints.
    const SAME: f64 = 0.005;
    if gaps.is_empty() {
        return (None, 0.0);
    }
    let mean = gaps.iter().sum::<f64>() / gaps.len() as f64;
    let (lo, hi) = gaps
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), g| {
            (lo.min(*g), hi.max(*g))
        });
    ((hi - lo < SAME).then_some(mean), mean)
}

/// Whether clicking this paint row's swatch or label should step into image
/// editing: is this row the picture the mode will act on?
///
/// Asked of `croppable_fill`, not the row's own brush: with two image fills it
/// answers the first visible one, so a click on the second would put the handles
/// on another picture. A stroke is never edited.
fn row_edits_the_mode_s_picture(
    doc: &ondin_core::Document,
    id: NodeId,
    slot: paint::PaintSlot,
) -> bool {
    let paint::PaintSlot::Fill(i) = slot else {
        return false;
    };
    doc.get(id)
        .and_then(|n| crate::tools::croppable_fill(n.paint()))
        == Some(i)
}

/// One of image editing's card action rows: a [`ui::menu_item`] that can be
/// unavailable and says why.
///
/// A widget inside a `disable()`d `Ui` reports no hover, and `Ui::scope`'s own
/// response is never hovered, so a dimmed row allocates a `Sense::empty()`
/// claimant over its rectangle, last so it sits above the top interactive widget,
/// to carry the "why" tooltip. Live, the row itself is the claimant.
pub(super) fn menu_action(
    ui: &mut egui::Ui,
    enabled: bool,
    glyph: &str,
    label: &str,
    tip: &dyn Fn(bool) -> String,
) -> bool {
    let scoped = ui.scope(|ui| {
        if !enabled {
            ui.disable();
        }
        ui::menu_item(ui, glyph, label, ui::MENU_ROW_H)
    });
    if enabled {
        return scoped.inner.on_hover_text(tip(true)).clicked();
    }
    // Salted by the label so rows keep distinct ids as they enable and disable.
    ui.interact(
        scoped.inner.rect,
        ui.id().with(("dimmed-row", label)),
        egui::Sense::empty(),
    )
    .on_hover_text(tip(false));
    false
}

fn join_label(j: Join) -> &'static str {
    match j {
        Join::Miter => "Miter",
        Join::Round => "Round",
        Join::Bevel => "Bevel",
    }
}

/// One named size: the label the dropdown shows, and the box it writes.
type Preset = (&'static str, f64, f64);
/// A heading and the sizes under it.
type PresetGroup = (&'static str, &'static [Preset]);

/// The frame sizes the Frame templates panel offers, by group, two cells across
/// ([`OndinApp::inspector_frame_templates`]).
///
/// One world unit is one pixel; print sizes are points at 72/inch (A4 is 595×842).
/// Portrait where a device has one. Two entries may not share a box, since
/// [`preset_named`] answers with the first match (`frame_grid_tests` checks).
const FRAME_PRESETS: &[PresetGroup] = &[
    (
        "PHONE",
        &[
            ("iPhone 16", 393.0, 852.0),
            ("iPhone 16 Pro Max", 440.0, 956.0),
            ("iPhone SE", 375.0, 667.0),
            ("Android compact", 412.0, 917.0),
        ],
    ),
    (
        "TABLET",
        &[
            ("iPad mini", 744.0, 1133.0),
            ("iPad Pro 11\"", 834.0, 1194.0),
        ],
    ),
    (
        "DESKTOP",
        &[
            ("Desktop", 1440.0, 1024.0),
            ("MacBook Air", 1280.0, 832.0),
            ("Laptop 13\"", 1280.0, 800.0),
            ("Full HD", 1920.0, 1080.0),
        ],
    ),
    (
        "PRINT",
        &[
            ("A3", 842.0, 1191.0),
            ("A4", 595.0, 842.0),
            ("A5", 420.0, 595.0),
            ("US Letter", 612.0, 792.0),
        ],
    ),
    (
        "SOCIAL",
        &[
            ("Square post", 1080.0, 1080.0),
            ("Story", 1080.0, 1920.0),
            ("Link preview", 1200.0, 630.0),
            ("Banner", 1500.0, 500.0),
        ],
    ),
];

/// How tall the Frame templates panel's scroll box may grow.
///
/// A cap, not a height: the full table would push Transform below the fold. It
/// lands between rows on purpose, so a cut-off cell says there is more.
const TEMPLATE_LIST_H: f32 = 184.0;
/// One template cell's height, and the gap between two of them.
///
/// Not [`ui::CARD_ROW_GAP`]: cells of one group are one control, and the card's
/// own pitch returns at the group boundary ([`OndinApp::inspector_frame_templates`]).
const TEMPLATE_CELL_H: f32 = 38.0;
/// See [`TEMPLATE_CELL_H`].
const TEMPLATE_CELL_GAP: f32 = 4.0;
/// The box a template's proportional preview is fitted into, and the smallest a
/// side of that preview may be drawn.
///
/// The floor keeps extreme aspect ratios readable; the dimensions beside it stay exact.
const TEMPLATE_BOX: egui::Vec2 = egui::vec2(20.0, 28.0);
/// See [`TEMPLATE_BOX`].
const TEMPLATE_BOX_MIN: f32 = 5.0;

/// One template: its proportional picture, its name and its box, lit when the
/// frame already *is* that size.
///
/// Painted rather than built from [`ui::button_face`]: a resting cell has no
/// ground (a list, not a wall of buttons).
///
/// The name is elided to the space left after the picture, using the galley that
/// will be painted (`ui::action_button`'s trap).
fn template_cell(ui: &mut egui::Ui, width: f32, name: &str, w: f64, h: f64, on: bool) -> bool {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(width, TEMPLATE_CELL_H), egui::Sense::click());
    let hovered = resp.hovered();
    let r5 = egui::CornerRadius::same(ui::BUTTON_R);
    let ground = match (on, hovered) {
        (true, false) => Some(color::ACCENT_900),
        (true, true) => Some(color::ACCENT_800),
        (false, true) => Some(theme::color::HOVER),
        (false, false) => None,
    };
    if let Some(bg) = ground {
        ui.painter().rect_filled(rect, r5, bg);
    }
    if on {
        ui.painter().rect_stroke(
            rect,
            r5,
            egui::Stroke::new(1.0, color::ACCENT_700),
            egui::StrokeKind::Inside,
        );
    }
    // A fixed slot keeps every name in a column at the same x.
    let slot = egui::Rect::from_center_size(
        egui::pos2(
            rect.left() + ui::FIELD_PAD_X + TEMPLATE_BOX.x / 2.0,
            rect.center().y,
        ),
        TEMPLATE_BOX,
    );
    let k = (f64::from(TEMPLATE_BOX.x) / w).min(f64::from(TEMPLATE_BOX.y) / h);
    let picture = egui::vec2(
        ((w * k) as f32).max(TEMPLATE_BOX_MIN),
        ((h * k) as f32).max(TEMPLATE_BOX_MIN),
    );
    ui.painter().rect_filled(
        egui::Rect::from_center_size(slot.center(), picture),
        egui::CornerRadius::same(1),
        match on {
            true => color::ACCENT_300,
            false => theme::text::DIM,
        },
    );
    let ink = match on {
        true => color::ACCENT_200,
        false => theme::text::MUTED,
    };
    let left = slot.right() + ui::CARD_COL_GAP;
    let galley = ui.ctx().fonts_mut(|f| {
        let mut job = egui::text::LayoutJob::simple_singleline(
            name.to_owned(),
            egui::FontId::proportional(TEMPLATE_NAME_PT),
            ink,
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(
            (rect.right() - ui::FIELD_PAD_X - left).max(0.0),
        );
        f.layout_job(job)
    });
    // Placed against the cell's centre so elided and whole names share a baseline.
    let p = ui.painter();
    p.galley(
        egui::pos2(
            left,
            rect.center().y - galley.size().y - TEMPLATE_LINE_GAP / 2.0,
        ),
        galley,
        ink,
    );
    p.text(
        egui::pos2(left, rect.center().y + TEMPLATE_LINE_GAP / 2.0),
        egui::Align2::LEFT_TOP,
        format!("{w} × {h}"),
        egui::FontId::proportional(TEMPLATE_DIMS_PT),
        theme::text::FAINT,
    );
    // The name repeats in the tooltip because the longest entry is elided on the cell.
    resp.on_hover_text(format!("{name} — resize this frame to {w} × {h}"))
        .clicked()
}

/// A template's name, and the size under it.
///
/// Half a point over the mock's 10.5 and 9, as `ui`'s `PREFIX_PT` is.
const TEMPLATE_NAME_PT: f32 = 11.0;
/// See [`TEMPLATE_NAME_PT`].
const TEMPLATE_DIMS_PT: f32 = 9.5;
/// Between a template's two lines.
const TEMPLATE_LINE_GAP: f32 = 2.0;

/// The preset a frame's box is, or `None` for one that matches nothing.
///
/// The comparison is on whole units: a frame dragged to 393.0000001 is the preset.
fn preset_named(size: Size) -> Option<&'static str> {
    FRAME_PRESETS.iter().find_map(|(_, entries)| {
        entries
            .iter()
            .find(|(_, w, h)| (size.width - w).abs() < 0.5 && (size.height - h).abs() < 0.5)
            .map(|(name, _, _)| *name)
    })
}

/// The Layout grid panel's control height, and the gap between two grids' blocks.
///
/// The card's own 28, not the popovers' 26.
const GRID_CELL: f32 = 28.0;
/// What goes between one grid's block and the next, on top of the card's own
/// [`ui::CARD_ROW_GAP`], so two grids read as two blocks. Equal to the stroke
/// list's pitch (`paint_unit`).
const GRID_ROW_GAP: f32 = ui::CARD_ROW_GAP;

/// The Transform card's W and H sizing modes for one layer: which
/// `layout::size_modes` offers, which each side is in, and the percentage a `%`
/// side holds — the number its field shows in place of the drawn size.
struct SizeMenu {
    modes: Vec<SizeMode>,
    w: SizeMode,
    h: SizeMode,
    w_pct: Option<f64>,
    h_pct: Option<f64>,
}

/// Where a layer is drawn inside its frame, as the Position card measures it
/// (`OndinApp::inset_placement`): everything `container::with_edge` needs to pin
/// an edge without moving anything.
struct InsetPlacement {
    frame: Size,
    local: Affine,
    bx: Rect,
    kind: NodeKind,
}

impl InsetPlacement {
    /// How far `edge` sits from the frame's matching edge, in the frame's own
    /// local units (the space its insets are in) — what an unpinned field shows,
    /// marked `auto`.
    fn distance(&self, edge: ondin_core::container::Edge) -> f64 {
        use ondin_core::container::Edge as Pin;
        let slot = ondin_core::container::slot_of(self.local, self.bx);
        match edge {
            Pin::Left => slot.x,
            Pin::Top => slot.y,
            Pin::Right => self.frame.width - slot.x - self.bx.width(),
            Pin::Bottom => self.frame.height - slot.y - self.bx.height(),
        }
    }

    /// The frame dimension `edge`'s percentage is a share of: its width for left
    /// and right, its height for top and bottom — CSS's rule.
    fn extent(&self, edge: ondin_core::container::Edge) -> f64 {
        use ondin_core::container::Edge as Pin;
        match edge {
            Pin::Left | Pin::Right => self.frame.width,
            Pin::Top | Pin::Bottom => self.frame.height,
        }
    }
}

/// Whether dragging `members` on the canvas reorders them: they are all in-flow
/// items of one container, so the X/Y tooltip must not promise it otherwise.
fn drags_as_a_block(doc: &ondin_core::Document, members: &[NodeId]) -> bool {
    let parent = |id: &NodeId| doc.get(*id).and_then(|n| n.parent());
    !members.is_empty()
        && members
            .iter()
            .all(|id| ondin_core::build::is_flex_item(doc, *id))
        && members.windows(2).all(|w| parent(&w[0]) == parent(&w[1]))
}

/// Why the multi-selection Transform card's X and Y are off; `None` when they
/// are on. Its answer is both the gate and the hover text, which a headless frame
/// never shows.
fn laid_out_xy(doc: &ondin_core::Document, members: &[NodeId]) -> Option<String> {
    let laid_out = members
        .iter()
        .filter(|id| ondin_core::build::is_flex_item(doc, **id))
        .count();
    (laid_out > 0).then(|| {
        format!(
            "{laid_out} of these {} placed by a container's layout — {}",
            if laid_out == 1 { "is" } else { "are" },
            if drags_as_a_block(doc, members) {
                "drag them on the canvas to reorder"
            } else {
                "select a container's items on their own to reorder them by dragging"
            }
        )
    })
}

/// The inset `insets` holds for `edge`.
fn inset_of(
    insets: &ondin_core::Insets,
    edge: ondin_core::container::Edge,
) -> Option<ondin_core::LengthPct> {
    use ondin_core::container::Edge as Pin;
    match edge {
        Pin::Top => insets.top,
        Pin::Right => insets.right,
        Pin::Bottom => insets.bottom,
        Pin::Left => insets.left,
    }
}

/// The slot in `insets` that holds `edge`.
fn inset_mut(
    insets: &mut ondin_core::Insets,
    edge: ondin_core::container::Edge,
) -> &mut Option<ondin_core::LengthPct> {
    use ondin_core::container::Edge as Pin;
    match edge {
        Pin::Top => &mut insets.top,
        Pin::Right => &mut insets.right,
        Pin::Bottom => &mut insets.bottom,
        Pin::Left => &mut insets.left,
    }
}

/// A Position field's prefix: CSS's own initial for the inset it edits.
fn inset_label(edge: ondin_core::container::Edge) -> &'static str {
    use ondin_core::container::Edge as Pin;
    match edge {
        Pin::Top => "T",
        Pin::Right => "R",
        Pin::Bottom => "B",
        Pin::Left => "L",
    }
}

/// The width of the Position card's pin diagram ([`inset_diagram`]); its height is
/// two field rows and the gap between them.
const INSET_DIAGRAM_W: f32 = 74.0;

/// The Position card's pin diagram: a layer in a field-coloured well and a strut
/// from each of its edges out towards the well's. A pinned strut is solid accent,
/// an unpinned one dashed and faint, brighter under the pointer. Clicking a strut
/// answers its edge.
///
/// Drawn, not a widget per strut: four `interact` rects over one picture, each the
/// whole band between the layer's edge and the well's. A pinned strut between two
/// auto margins (centring) is solid in [`color::MEASURE`]. `held` is whether the
/// left and the top edge are held by default (an axis with no inset outside a
/// layout keeps its offset from the top-left); such a strut is drawn solid.
fn inset_diagram(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    insets: &ondin_core::Insets,
    held: (bool, bool),
) -> Option<ondin_core::container::Edge> {
    use ondin_core::container::Edge as Pin;
    // Struts start 6 in from the well's edge, stop 3 short of the layer's, 2 wide;
    // the layer is 28 × 20.
    const INSET: f32 = 6.0;
    const CLEAR: f32 = 3.0;
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 5.0, color::FIELD);
    painter.rect_stroke(
        rect,
        5.0,
        egui::Stroke::new(1.0, color::FIELD_BORDER),
        egui::StrokeKind::Inside,
    );
    let layer = egui::Rect::from_center_size(rect.center(), egui::vec2(28.0, 20.0));
    let (fill, edge_colour) = if insets.is_authored() || held.0 || held.1 {
        (
            color::ACCENT.gamma_multiply(0.14),
            color::ACCENT.gamma_multiply(0.7),
        )
    } else {
        (theme::color::text_a(13), theme::color::text_a(64))
    };
    painter.rect_filled(layer, 3.0, fill);
    painter.rect_stroke(
        layer,
        3.0,
        egui::Stroke::new(1.0, edge_colour),
        egui::StrokeKind::Inside,
    );
    let well = rect.shrink(INSET);
    let near = layer.expand(CLEAR);
    let mid = layer.center();
    // Each band reaches from the well's edge to the layer's and a few points
    // either side of the strut, so a target is not a two-point line.
    const REACH: f32 = 5.0;
    let struts = [
        (
            Pin::Left,
            egui::pos2(well.left(), mid.y),
            egui::pos2(near.left(), mid.y),
            egui::Rect::from_min_max(
                egui::pos2(rect.left(), layer.top() - REACH),
                egui::pos2(layer.left(), layer.bottom() + REACH),
            ),
        ),
        (
            Pin::Right,
            egui::pos2(near.right(), mid.y),
            egui::pos2(well.right(), mid.y),
            egui::Rect::from_min_max(
                egui::pos2(layer.right(), layer.top() - REACH),
                egui::pos2(rect.right(), layer.bottom() + REACH),
            ),
        ),
        (
            Pin::Top,
            egui::pos2(mid.x, well.top()),
            egui::pos2(mid.x, near.top()),
            egui::Rect::from_min_max(
                egui::pos2(layer.left() - REACH, rect.top()),
                egui::pos2(layer.right() + REACH, layer.top()),
            ),
        ),
        (
            Pin::Bottom,
            egui::pos2(mid.x, near.bottom()),
            egui::pos2(mid.x, well.bottom()),
            egui::Rect::from_min_max(
                egui::pos2(layer.left() - REACH, layer.bottom()),
                egui::pos2(layer.right() + REACH, rect.bottom()),
            ),
        ),
    ];
    let centred_h = insets.margin_auto.left && insets.margin_auto.right;
    let centred_v = insets.margin_auto.top && insets.margin_auto.bottom;
    let mut clicked = None;
    for (edge, a, b, hit) in struts {
        let id = ui.id().with(("inset-strut", inset_label(edge)));
        let resp = ui.interact(hit, id, egui::Sense::click());
        let pinned = inset_of(insets, edge).is_some();
        let by_default = match edge {
            Pin::Left => held.0,
            Pin::Top => held.1,
            Pin::Right | Pin::Bottom => false,
        };
        let centred = match edge {
            Pin::Left | Pin::Right => centred_h,
            Pin::Top | Pin::Bottom => centred_v,
        };
        let colour = if pinned && centred {
            color::MEASURE
        } else if pinned || by_default {
            color::ACCENT_300
        } else if resp.hovered() {
            theme::color::text_a(140)
        } else {
            theme::color::text_a(64)
        };
        let stroke = egui::Stroke::new(2.0, colour);
        if pinned || by_default {
            painter.line_segment([a, b], stroke);
        } else {
            painter.add(egui::Shape::dashed_line(&[a, b], stroke, 3.0, 2.0));
        }
        let tip = match (pinned, by_default, edge) {
            (true, _, Pin::Left) => "Unpin the left edge",
            (true, _, Pin::Right) => "Unpin the right edge",
            (true, _, Pin::Top) => "Unpin the top edge",
            (true, _, Pin::Bottom) => "Unpin the bottom edge",
            (false, true, Pin::Left) => {
                "Held at the left by default — click to pin it, or pin the right edge to \
                 follow that instead"
            }
            (false, true, _) => {
                "Held at the top by default — click to pin it, or pin the bottom edge to \
                 follow that instead"
            }
            (false, false, Pin::Left) => "Pin the left edge",
            (false, false, Pin::Right) => "Pin the right edge",
            (false, false, Pin::Top) => "Pin the top edge",
            (false, false, Pin::Bottom) => "Pin the bottom edge",
        };
        if resp.on_hover_text(tip).clicked() {
            clicked = Some(edge);
        }
    }
    clicked
}

/// What the Layout grid panel's `+` does to the frames it is over.
///
/// Two verbs rather than a bool and a grid: one adds to what is there, the other
/// throws it away.
#[derive(Clone, Copy, Debug, PartialEq)]
enum GridAdd {
    /// Add to whatever each frame already has — the selection agrees.
    Append(LayoutGrid),
    /// Replace every frame's list with this one grid — the selection disagrees.
    Write(LayoutGrid),
}

/// The `+`'s whole decision, as a function of the shared list and nothing else, so
/// a test exercises the rule itself.
///
/// Columns for the first grid, rows for the second, and a colour the frames are not
/// already using. Over a disagreement (`None`) the `+` overrides rather than
/// appends: appending leaves the frames disagreeing with no way out, as Export's
/// `+` knows.
fn grid_add(shared: Option<&[LayoutGrid]>) -> GridAdd {
    let seen = shared.unwrap_or(&[]);
    let axis = match seen.iter().any(|g| g.axis == GridAxis::Columns) {
        true => GridAxis::Rows,
        false => GridAxis::Columns,
    };
    let seed = LayoutGrid {
        color: ondin_core::next_grid_color(seen),
        ..LayoutGrid::new(axis)
    };
    match shared {
        Some(_) => GridAdd::Append(seed),
        None => GridAdd::Write(seed),
    }
}

/// What one grid's controls decided this frame.
#[derive(Default)]
struct GridRowOut {
    /// The grid as it should be committed, or `None` for "nothing settled".
    set: Option<LayoutGrid>,
    remove: bool,
    /// An instance's overridden grid asked for its main's back.
    reset: bool,
}

/// The inputs [`grid_step`] decides from.
///
/// A near-copy of `panels::export`'s `ScrubInput`, deliberately unshared.
struct GridScrub {
    cancelled: bool,
    dragged: bool,
    changed: bool,
    was_held: bool,
}

/// What the frame should do about a grid's numeric fields.
#[derive(Clone, Copy, Debug, PartialEq)]
enum GridStep {
    /// Keep the value in `OndinApp::grid_scrub`; commit nothing.
    Hold,
    /// The edit is finished — commit this, once.
    Commit(LayoutGrid),
    Nothing,
}

/// The grid fields' whole decision, as `panels::export`'s `quality_step` is.
///
/// The cancel is answered first: egui may still report that frame's motion as
/// `changed()`. A drag holds and never commits, since `cancel_gesture` undoes a
/// scrub by dropping what holds it. The release is recognised by the buffer, not
/// `drag_stopped()`, which is not reported on every path out of a drag.
fn grid_step(input: GridScrub, value: LayoutGrid, current: LayoutGrid) -> GridStep {
    if input.cancelled {
        return GridStep::Nothing;
    }
    if input.dragged {
        return GridStep::Hold;
    }
    if input.was_held || input.changed {
        return match value == current {
            true => GridStep::Nothing,
            false => GridStep::Commit(value),
        };
    }
    GridStep::Nothing
}

/// The glyph a grid's axis switch shows: the dashboard's grid/list pair, the same
/// distinction.
fn axis_glyph(axis: GridAxis) -> &'static str {
    match axis {
        GridAxis::Columns => icon::COLUMNS,
        GridAxis::Rows => icon::ROWS,
    }
}

/// The glyph an alignment shows, per axis, because an edge is named differently on
/// each axis. The align glyphs are the Align panel's; *Stretch* takes out-arrows.
fn align_glyph(align: GridAlign, axis: GridAxis) -> &'static str {
    match (align, axis) {
        (GridAlign::Stretch, GridAxis::Columns) => icon::ARROWS_OUT_LINE_HORIZONTAL,
        (GridAlign::Stretch, GridAxis::Rows) => icon::ARROWS_OUT_LINE_VERTICAL,
        (GridAlign::Start, GridAxis::Columns) => icon::ALIGN_LEFT,
        (GridAlign::Start, GridAxis::Rows) => icon::ALIGN_TOP,
        (GridAlign::Center, GridAxis::Columns) => icon::ALIGN_CENTER_HORIZONTAL,
        (GridAlign::Center, GridAxis::Rows) => icon::ALIGN_CENTER_VERTICAL,
        (GridAlign::End, GridAxis::Columns) => icon::ALIGN_RIGHT,
        (GridAlign::End, GridAxis::Rows) => icon::ALIGN_BOTTOM,
    }
}

/// A layout grid's axis in words, for its ghost row: *Columns · 12*.
fn grid_axis_word(axis: GridAxis) -> &'static str {
    match axis {
        GridAxis::Columns => "Columns",
        GridAxis::Rows => "Rows",
    }
}

/// [`ui::segment_label`] for a cell whose label is a glyph.
fn grid_glyph(painter: &egui::Painter, rect: egui::Rect, glyph: &'static str, on: bool) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(13.0),
        if on { color::TEXT } else { theme::text::DIM },
    );
}

/// Where a line's end goes so that the line is `length` long, keeping its
/// direction; `None` for a zero-length line.
///
/// The start is the anchor: a line's X and Y report its start point, so growing
/// from the far end would move two other fields.
fn line_end_for_length(start: Point, end: Point, length: f64) -> Option<Point> {
    let v = end - start;
    let current = v.hypot();
    (current > 1e-9 && length.is_finite() && length > 0.0).then(|| start + v * (length / current))
}

// The transform origin's numbers live in `canvas.rs`, beside the marker they read.
/// Figma's four words, deliberately: Fill and Fit are the familiar pair.
fn fit_label(f: ondin_core::ImageFit) -> &'static str {
    match f {
        ondin_core::ImageFit::Fill => "Fill",
        ondin_core::ImageFit::Fit => "Fit",
        ondin_core::ImageFit::Crop => "Crop",
        ondin_core::ImageFit::Tile => "Tile",
    }
}
fn align_label(a: StrokeAlign) -> &'static str {
    match a {
        StrokeAlign::Center => "Centre",
        StrokeAlign::Inside => "Inside",
        StrokeAlign::Outside => "Outside",
    }
}
/// Designer wording rather than kurbo's `Butt`/`Round`/`Square`.
fn cap_label(c: Cap) -> &'static str {
    match c {
        Cap::Butt => "Straight",
        Cap::Round => "Rounded",
        Cap::Square => "Extended",
    }
}
fn sides_label(s: StrokeSides) -> &'static str {
    match s {
        StrokeSides::All => "All sides",
        StrokeSides::Top => "Top side",
        StrokeSides::Right => "Right side",
        StrokeSides::Bottom => "Bottom side",
        StrokeSides::Left => "Left side",
        StrokeSides::Custom { .. } => "Custom",
    }
}
/// What the sides button says about itself, given whether the kind takes per-side
/// strokes at all, whether the row is open, and what the stroke is set to.
/// [`ui::FieldButton::Set`] is reachable only when closed and not on every side.
fn sides_button_state(applies: bool, open: bool, sides: StrokeSides) -> ui::FieldButton {
    if !applies {
        ui::FieldButton::Disabled
    } else if open {
        ui::FieldButton::On
    } else if sides.is_all() {
        ui::FieldButton::Off
    } else {
        ui::FieldButton::Set
    }
}

/// Set a stroke's weight, re-broadcasting it to every side.
///
/// The weight is the master: the per-side row is an override of it, so editing the
/// weight flattens a `Custom` set of four sides onto the new value, zero sides
/// included. The named single sides carry no widths and are left alone.
pub(super) fn set_stroke_weight(stroke: &mut Stroke, width: f64) {
    let width = width.max(0.0);
    stroke.width = width;
    if let StrokeSides::Custom { .. } = stroke.sides {
        stroke.sides = StrokeSides::Custom {
            top: width,
            right: width,
            bottom: width,
            left: width,
        };
    }
}

/// The several-layer weight row's write: `width` onto every stroke in scope
/// through [`set_stroke_weight`], each bound width detached in the edit itself,
/// except a stroke whose bound width already draws `width` (as the opacity row
/// does). Resolved in each layer's own mode, hence
/// [`ondin_core::edit_strokes_at_all`], not the id-blind `edit_strokes_all`.
pub(super) fn weight_all_tx(doc: &ondin_core::Document, ids: &[NodeId], width: f64) -> Transaction {
    ondin_core::edit_strokes_at_all(doc, ids, |id, s| {
        let drawn = s
            .width_var
            .map(|v| doc.stroke_width_drawn_in(doc.variables(), id, v));
        if drawn.is_some_and(|d| (d - width.max(0.0)).abs() < 1e-9) {
            return;
        }
        set_stroke_weight(s, width);
        s.width_var = None;
    })
}

fn sides_hint(s: StrokeSides) -> &'static str {
    match s {
        StrokeSides::All => "The whole outline, with joined corners",
        StrokeSides::Custom { .. } => "A width per side; 0 for no stroke",
        _ => "This side only, at the weight above",
    }
}

/// The pattern switching to `style` should produce, keeping whatever the current
/// one can supply: Dotted → Dashed → Dotted must come back where it started.
fn pattern_for(style: DashStyle, s: &Stroke) -> Vec<f64> {
    let unit = s.width.max(1.0);
    let (length, spacing) = match s.dash_style() {
        DashStyle::Solid => (unit * 3.0, unit * 2.0),
        DashStyle::Dotted => (unit * 3.0, s.dot_spacing()),
        DashStyle::Dashed => s.dash_length_spacing(),
        DashStyle::Custom => (
            s.dashes.first().copied().unwrap_or(unit * 3.0),
            s.dashes.get(1).copied().unwrap_or(unit * 2.0),
        ),
    };
    match style {
        DashStyle::Solid => Vec::new(),
        // Zero length: a true dot, so `spacing` is centre-to-centre. `.max(0.0)`, not
        // `.max(unit)`: a width floor would corrupt an inherited spacing smaller than
        // the width (re-clicking the lit tab too); the `Solid` arm supplies defaults.
        DashStyle::Dotted => vec![0.0, spacing.max(0.0)],
        DashStyle::Dashed => vec![length.max(0.0), spacing.max(0.0)],
        DashStyle::Custom if s.dashes.len() > 2 => s.dashes.clone(),
        DashStyle::Custom => vec![length, spacing, unit, spacing],
    }
}

/// A dash list as the custom field shows it.
fn dash_list_text(dashes: &[f64]) -> String {
    dashes
        .iter()
        .map(|d| {
            // Trailing `.0` dropped so `5, 10` reads as what was typed.
            if (d.fract()).abs() < 1e-9 {
                format!("{}", d.round())
            } else {
                format!("{d}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Parse a comma-separated dash list, or `None` if it says nothing usable.
///
/// `None` rather than an empty vec on failure, so a mistyped character cannot
/// silently turn the stroke solid. All zeroes and negative numbers are failures too.
fn parse_dash_list(text: &str) -> Option<Vec<f64>> {
    let mut out = Vec::new();
    for part in text.split([',', ';', ' ', '\t']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let v: f64 = part.parse().ok()?;
        if !v.is_finite() || v < 0.0 {
            return None;
        }
        out.push(v);
    }
    // One number is legal in SVG but is likelier half of a pair still being typed.
    if out.len() < 2 || out.iter().all(|d| *d <= 0.0) {
        return None;
    }
    Some(out)
}
#[cfg(test)]
mod collapse_tests {
    use super::*;
    use std::collections::HashSet;
    /// The panel state the two rules act on: what the user has collapsed, and what
    /// this has already auto-collapsed.
    #[derive(Default)]
    struct Panels {
        collapsed: HashSet<&'static str>,
        auto: HashSet<&'static str>,
    }
    impl Panels {
        /// A frame with the same subject as the last one.
        fn sync(&mut self, title: &'static str, empty: bool) {
            sync_paint_collapse(&mut self.collapsed, &mut self.auto, title, empty, false);
        }
        /// The first frame after the selection moved.
        fn select(&mut self, title: &'static str, empty: bool) {
            sync_paint_collapse(&mut self.collapsed, &mut self.auto, title, empty, true);
        }
        fn add(&mut self, title: &'static str) {
            open_paint_panel(&mut self.collapsed, &mut self.auto, title);
        }
        /// What `panel` reads to decide whether to draw a body.
        fn open(&self, title: &str) -> bool {
            !self.collapsed.contains(title)
        }
    }
    /// An empty paint panel collapses once, stays collapsed, and can still be expanded by hand.
    #[test]
    fn an_empty_paint_panel_collapses_once_and_then_obeys_the_user() {
        let mut p = Panels::default();
        p.sync("Stroke", true);
        assert!(!p.open("Stroke"), "an empty panel should start collapsed");
        p.sync("Stroke", true);
        assert!(!p.open("Stroke"), "and stay collapsed on its own");
        p.collapsed.remove("Stroke");
        for _ in 0..3 {
            p.sync("Stroke", true);
            assert!(
                p.open("Stroke"),
                "the auto-collapse fought the user's own toggle"
            );
        }
    }
    /// Adding a paint opens the panel, and emptying it again re-arms the collapse.
    #[test]
    fn adding_a_paint_opens_the_panel_and_emptying_it_rearms_the_collapse() {
        let mut p = Panels::default();
        p.sync("Fill", true);
        assert!(!p.open("Fill"));
        p.add("Fill");
        assert!(p.open("Fill"), "`+` must put the new paint on screen");
        p.sync("Fill", false);
        assert!(p.open("Fill"), "a panel with paint in it must stay open");
        p.sync("Fill", true);
        assert!(!p.open("Fill"), "emptying it should collapse it again");
    }
    /// `+` opens a panel the user had collapsed while it had paint in it.
    #[test]
    fn adding_a_paint_opens_a_panel_the_user_had_collapsed() {
        let mut p = Panels::default();
        p.sync("Fill", false);
        p.collapsed.insert("Fill");
        p.add("Fill");
        assert!(p.open("Fill"));
    }
    /// The two panels are independent — collapsing Stroke must not touch Fill.
    #[test]
    fn the_two_paint_panels_collapse_independently() {
        let mut p = Panels::default();
        p.sync("Fill", false);
        p.sync("Stroke", true);
        assert!(p.open("Fill"), "Fill has paint and should be open");
        assert!(!p.open("Stroke"), "Stroke is empty and should be closed");
    }
    /// Walking from a painted layer to an unpainted one and back reopens the panel.
    #[test]
    fn selecting_a_painted_layer_reopens_a_panel_an_empty_one_closed() {
        let mut p = Panels::default();
        p.select("Fill", false);
        assert!(p.open("Fill"), "a layer with a fill starts open");
        p.select("Fill", true);
        assert!(!p.open("Fill"), "a layer with no fill closes it");
        p.select("Fill", false);
        assert!(
            p.open("Fill"),
            "back on a painted layer the panel must open again"
        );
    }
    /// A hand collapse holds for every frame the user stays on that layer.
    #[test]
    fn a_hand_collapse_survives_every_frame_of_the_selection_that_earned_it() {
        let mut p = Panels::default();
        p.select("Stroke", false);
        p.collapsed.insert("Stroke");
        for _ in 0..3 {
            p.sync("Stroke", false);
            assert!(!p.open("Stroke"), "re-reading the room fought the user");
        }
        p.select("Stroke", false);
        assert!(p.open("Stroke"));
    }
}
#[cfg(test)]
mod carry_tests {
    use super::*;
    /// Lay out three 28px units, optionally carrying `carry`, and report the slot
    /// rects `carried_unit` hands back.
    fn slots(carry: Option<usize>, pointer: Option<egui::Pos2>) -> Vec<egui::Rect> {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut out = Vec::new();
        let raw = egui::RawInput {
            events: pointer.map(egui::Event::PointerMoved).into_iter().collect(),
            ..Default::default()
        };
        let _ = ctx.run_ui(raw, |ui| {
            ui.set_max_width(220.0);
            ui.spacing_mut().item_spacing.y = 9.0;
            for i in 0..3 {
                let salt = ("probe", i);
                let (_, rect) = carried_unit(ui, carry == Some(i), salt, |ui| {
                    ui::field_row(ui, egui::vec2(200.0, 28.0), |ui| {
                        ui.label(format!("row {i}"));
                    });
                });
                out.push(rect);
            }
        });
        out
    }
    /// Shrinking a paint list paints no red id-change outline (egui's
    /// `warn_if_rect_changes_id`, which `theme::install` turns off).
    ///
    /// Debug-only: egui puts `Style::debug` behind `debug_assertions`.
    #[test]
    fn shrinking_a_paint_list_paints_no_warning_outline() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        // The walk `inspector_fill` does: top of the stack first, salted by index.
        let frame = |n: usize| -> Vec<egui::epaint::ClippedShape> {
            ctx.run_ui(Default::default(), |ui| {
                ui.set_max_width(220.0);
                ui.spacing_mut().item_spacing.y = 9.0;
                for i in (0..n).rev() {
                    carried_unit(ui, false, ("fill-row", i), |ui| {
                        ui::field_row(ui, egui::vec2(200.0, 28.0), |ui| {
                            let mut v = 50.0_f64;
                            ui::bare_drag_value(ui, egui::DragValue::new(&mut v));
                        });
                    });
                }
            })
            .shapes
        };
        let reds = |shapes: &[egui::epaint::ClippedShape]| {
            shapes
                .iter()
                .filter(|c| {
                    matches!(&c.shape, egui::Shape::Rect(r)
                        if r.stroke.color == egui::Color32::RED && r.stroke.width > 0.0)
                })
                .count()
        };
        for n in [2usize, 2, 1, 1, 3, 3, 2, 2] {
            assert_eq!(
                reds(&frame(n)),
                0,
                "a list of {n} painted egui's id-change warning"
            );
        }
    }
    /// A carried unit keeps its slot: its shapes move, its layout does not (`paint::Slots` reuses the rects).
    #[test]
    fn carrying_a_unit_does_not_reflow_the_list() {
        let resting = slots(None, None);
        assert_eq!(resting.len(), 3);
        let pitch = resting[1].top() - resting[0].top();
        assert!(pitch > 28.0, "expected a real pitch, got {pitch}");
        let carried = slots(Some(1), Some(egui::pos2(100.0, 200.0)));
        assert_eq!(carried.len(), 3);
        for (i, (a, b)) in resting.iter().zip(&carried).enumerate() {
            assert_eq!(
                a.top(),
                b.top(),
                "slot {i} moved when the middle unit was picked up: {resting:?} vs {carried:?}"
            );
            assert_eq!(a.height(), b.height(), "slot {i} changed height");
        }
    }
    /// The carried unit's ink moves to the pointer (compared against resting, as the translation happens after layout).
    #[test]
    fn a_carried_units_ink_follows_the_pointer() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let grounds = |carry: bool| -> Vec<egui::Rect> {
            ctx.run_ui(
                egui::RawInput {
                    events: vec![egui::Event::PointerMoved(egui::pos2(100.0, 300.0))],
                    ..Default::default()
                },
                |ui| {
                    ui.set_max_width(220.0);
                    carried_unit(ui, carry, ("probe", 0), |ui| {
                        ui::field_row(ui, egui::vec2(200.0, 28.0), |ui| {
                            ui.label("row");
                        });
                    });
                },
            )
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r) if r.fill == color::FIELD => Some(r.rect),
                _ => None,
            })
            .collect()
        };
        let resting = grounds(false);
        let carried = grounds(true);
        assert_eq!(resting.len(), 1, "one ground at rest: {resting:?}");
        assert_eq!(carried.len(), 1, "one ground while carried: {carried:?}");
        assert!(
            (carried[0].center().y - 300.0).abs() < 1.0,
            "the carried row was painted at {:?}, not under the pointer at y=300",
            carried[0]
        );
        assert!(
            (resting[0].center().y - 300.0).abs() > 100.0,
            "the resting row was already at the pointer, so this proves nothing: {resting:?}"
        );
        assert_eq!(
            resting[0].min.x, carried[0].min.x,
            "the carried row drifted horizontally"
        );
    }
}
#[cfg(test)]
mod angle_tests {
    use super::*;
    /// An angle field never reads `-0°`: angles from `build::Basis` can be a signed or ulp-scale zero.
    #[test]
    fn a_zero_angle_never_reads_as_negative_zero() {
        for radians in [
            -0.0_f64,
            0.0,
            // An upright matrix built through a mirror, and RQ-split residue.
            (-0.0_f64).atan2(1.0),
            -1e-17,
            1e-17,
            -1e-12,
        ] {
            let shown = shown_degrees(radians);
            assert_eq!(shown, 0.0, "{radians:e} should show as zero");
            assert!(
                shown.is_sign_positive(),
                "{radians:e} came back as negative zero, which formats as -0"
            );
            assert_eq!(format!("{shown:.0}"), "0", "{radians:e} formats wrongly");
        }
    }
    /// An angle somebody asked for is untouched, even a small one.
    #[test]
    fn a_real_angle_is_left_alone() {
        for degrees in [-90.0_f64, -45.0, -0.5, -0.01, 0.01, 30.0, 179.9] {
            let shown = shown_degrees(degrees.to_radians());
            assert!(
                (shown - degrees).abs() < 1e-9,
                "{degrees} came back as {shown}"
            );
        }
    }
}
#[cfg(test)]
mod carry_id_tests {
    use super::*;
    /// A row's widget ids follow the paint, not the slot: `carried_unit` salts the child `Ui` by entry, so a live reorder keeps focus.
    #[test]
    fn a_units_widget_ids_survive_the_rows_changing_places() {
        let ids = |order: [usize; 2]| -> Vec<(usize, egui::Id)> {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            let _ = ctx.run_ui(Default::default(), |_| {});
            let mut out = Vec::new();
            let _ = ctx.run_ui(Default::default(), |ui| {
                for i in order {
                    let (id, _) = carried_unit(ui, false, ("row", i), |ui| {
                        ui.allocate_response(egui::vec2(40.0, 20.0), egui::Sense::click())
                            .id
                    });
                    out.push((i, id));
                }
            });
            out
        };
        let mut top_first = ids([1, 0]);
        let mut swapped = ids([0, 1]);
        top_first.sort_by_key(|(i, _)| *i);
        swapped.sort_by_key(|(i, _)| *i);
        assert_eq!(
            top_first, swapped,
            "the widget inside each unit changed id when the rows changed places"
        );
        assert_ne!(
            top_first[0].1, top_first[1].1,
            "two units shared an id, which egui would flag as a clash"
        );
    }
}

#[cfg(test)]
mod dash_list_tests {
    use super::*;

    /// The custom field reads a comma list as on, off, on, off… verbatim.
    #[test]
    fn a_custom_dash_list_parses_the_way_the_hint_says() {
        assert_eq!(parse_dash_list("5, 10"), Some(vec![5.0, 10.0]));
        assert_eq!(
            parse_dash_list("5, 10, 15"),
            Some(vec![5.0, 10.0, 15.0]),
            "an odd list is legal — it repeats, so the third number is ink again"
        );
        assert_eq!(parse_dash_list("5 10"), Some(vec![5.0, 10.0]));
        assert_eq!(parse_dash_list("  5 ,10 , "), Some(vec![5.0, 10.0]));
        assert_eq!(parse_dash_list("2.5, 1.25"), Some(vec![2.5, 1.25]));
    }

    /// An unusable list is rejected rather than emptied, so a typo cannot turn the stroke solid.
    #[test]
    fn an_unusable_dash_list_is_rejected_rather_than_emptied() {
        for text in [
            "", "   ", "abc", "5, x",
            // A negative length has no meaning, and `-` is a plausible typo for a
            // comma on a numeric keypad.
            "5, -10", "5, nan", // All zeroes would draw nothing at all.
            "0, 0",
            // One number is legal `stroke-dasharray`, but typed it is likelier half
            // of a pair still being typed.
            "5",
        ] {
            assert_eq!(parse_dash_list(text), None, "{text:?} must be rejected");
        }
    }

    /// The field shows what was typed (`5, 10`) rather than what an `f64` prints.
    #[test]
    fn the_dash_list_round_trips_through_its_own_text() {
        for dashes in [
            vec![5.0, 10.0],
            vec![5.0, 10.0, 15.0, 5.0],
            vec![2.5, 1.25],
            vec![0.0, 6.0],
        ] {
            let text = dash_list_text(&dashes);
            assert_eq!(parse_dash_list(&text), Some(dashes.clone()), "{text}");
        }
        assert_eq!(dash_list_text(&[5.0, 10.0]), "5, 10");
        assert_eq!(dash_list_text(&[2.5, 1.0]), "2.5, 1");
    }

    /// Switching modes keeps the numbers: Dotted → Dashed → Dotted comes back where it started.
    #[test]
    fn the_dash_modes_hand_their_numbers_to_each_other() {
        let stroke = |dashes: Vec<f64>| Stroke {
            dashes,
            width: 2.0,
            ..Default::default()
        };
        let dotted = stroke(vec![0.0, 9.0]);
        assert_eq!(dotted.dash_style(), DashStyle::Dotted);
        let dashed = stroke(pattern_for(DashStyle::Dashed, &dotted));
        assert_eq!(dashed.dash_style(), DashStyle::Dashed);
        assert_eq!(
            dashed.dash_length_spacing().1,
            9.0,
            "the dot's period became the dash's gap, so the ink stays put"
        );
        let back = stroke(pattern_for(DashStyle::Dotted, &dashed));
        assert_eq!(back.dashes, dotted.dashes, "round trip lost the spacing");

        let solid = stroke(pattern_for(DashStyle::Solid, &dashed));
        assert_eq!(solid.dash_style(), DashStyle::Solid);
        let fresh = stroke(pattern_for(DashStyle::Dashed, &solid));
        assert_eq!(fresh.dashes, vec![6.0, 4.0], "3 and 2 stroke widths");
        assert!(
            fresh.dashes.iter().all(|d| *d > 0.0),
            "a mode with nothing to inherit must not open on zeroes: {fresh:?}"
        );

        let long = stroke(vec![5.0, 10.0, 15.0, 5.0]);
        assert_eq!(pattern_for(DashStyle::Custom, &long), long.dashes);
        assert_eq!(pattern_for(DashStyle::Custom, &dashed).len(), 4);
    }

    /// A dotted spacing under the stroke width survives the tabs, and re-picking the lit tab is a no-op.
    ///
    /// flip: `.max(unit)` restored in `pattern_for`'s Dotted arm → fails at the one-click `[0.0, 4.0]` assertion.
    #[test]
    fn a_dotted_spacing_under_the_stroke_width_survives_a_mode_round_trip() {
        let stroke = |dashes: Vec<f64>| Stroke {
            dashes,
            width: 4.0,
            ..Default::default()
        };
        let tight = stroke(vec![0.0, 2.0]);
        assert_eq!(
            tight.dash_style(),
            DashStyle::Dotted,
            "fixture: a spacing under the width is still a dotted stroke"
        );
        assert!(
            tight.dot_spacing() < tight.width,
            "fixture: and it must actually be under it, or the clamp cannot fire"
        );

        // One click: the tab that is already lit is a live button.
        assert_eq!(
            pattern_for(DashStyle::Dotted, &tight),
            tight.dashes,
            "re-picking the lit Dotted tab must not move the spacing"
        );
        // The other modes, each asked of a stroke already in it (a setup that
        // clamped first would pass under the defect).
        for style in [DashStyle::Dashed, DashStyle::Custom, DashStyle::Solid] {
            let s = stroke(pattern_for(style, &tight));
            assert_eq!(
                pattern_for(s.dash_style(), &s),
                s.dashes,
                "{style:?}: re-picking the lit tab is a no-op"
            );
        }

        let dashed = stroke(pattern_for(DashStyle::Dashed, &tight));
        let back = stroke(pattern_for(DashStyle::Dotted, &dashed));
        assert_eq!(
            back.dashes, tight.dashes,
            "and the round trip comes back where it started"
        );

        let solid = stroke(pattern_for(DashStyle::Solid, &tight));
        let fresh = stroke(pattern_for(DashStyle::Dotted, &solid));
        assert_eq!(
            fresh.dashes,
            vec![0.0, 8.0],
            "from Solid, the dotted default is two stroke widths"
        );
    }
}

#[cfg(test)]
mod stroke_row_tests {
    use super::*;

    /// A context with the theme's fonts installed.
    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        ctx
    }

    /// Every rect a pass painted whose height is in `range`, top-left first.
    fn painted_rows(output: &egui::FullOutput, range: std::ops::Range<f32>) -> Vec<egui::Rect> {
        let mut out: Vec<egui::Rect> = output
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r) if range.contains(&r.rect.height()) => Some(r.rect),
                _ => None,
            })
            .collect();
        out.sort_by(|a, b| {
            a.top()
                .total_cmp(&b.top())
                .then(a.left().total_cmp(&b.left()))
        });
        out
    }

    /// A stroke's two rows sit `ui::CARD_ROW_GAP` apart and two strokes twice that, asserted as exact gaps
    /// (a ratio would pass with banked spacing); `paint_unit` is driven directly.
    #[test]
    fn two_rows_of_one_stroke_sit_half_as_far_apart_as_two_strokes() {
        let ctx = context();
        let mut rows: Vec<egui::Rect> = Vec::new();
        let _ = ctx.run_ui(Default::default(), |ui| {
            ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
            let row = |rows: &mut Vec<egui::Rect>, ui: &mut egui::Ui| {
                rows.push(
                    ui.allocate_exact_size(egui::vec2(200.0, 28.0), egui::Sense::empty())
                        .0,
                );
            };
            for unit in 0..3 {
                paint_unit(ui, &mut rows, unit == 2, row, row);
            }
        });
        assert_eq!(rows.len(), 6, "three strokes of two rows each");
        let gaps: Vec<f32> = rows
            .windows(2)
            .map(|w| w[1].top() - w[0].bottom())
            .collect();
        let (within, between) = (gaps[0], gaps[1]);
        assert_eq!(
            gaps,
            vec![within, between, within, between, within],
            "the rhythm is not the same down the list: {gaps:?}"
        );
        assert_eq!(
            within,
            ui::CARD_ROW_GAP,
            "a stroke's two rows must sit exactly as far apart as an image fill's two lines"
        );
        assert_eq!(
            between,
            within * 2.0,
            "two strokes {between} apart against {within} within one is not a group"
        );
    }

    /// A dimmed control is hovered only through a claimant of its own, not the widget or the
    /// scope around it, for both a `ui::menu_item` and a `ui::value_field`.
    #[test]
    fn a_dimmed_control_can_be_hovered_only_through_a_claimant_of_its_own() {
        /// `(the widget, the scope around it, an explicit claimant over the same
        /// rect)` once the pointer has settled on `content` drawn `enabled` or not.
        fn hovers(
            ctx: &egui::Context,
            enabled: bool,
            content: &dyn Fn(&mut egui::Ui) -> egui::Response,
        ) -> (bool, bool, bool) {
            let mut at = None;
            let mut out = (false, false, false);
            for _ in 0..4 {
                let events = at
                    .map(|p| vec![egui::Event::PointerMoved(p)])
                    .unwrap_or_default();
                let _ = ctx.run_ui(
                    egui::RawInput {
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        ui.set_max_width(250.0);
                        let scoped = ui.scope(|ui| {
                            if !enabled {
                                ui.disable();
                            }
                            content(ui)
                        });
                        at = Some(scoped.inner.rect.center());
                        let claim = ui.interact(
                            scoped.inner.rect,
                            ui.id().with("dimmed-claim"),
                            egui::Sense::empty(),
                        );
                        out = (
                            scoped.inner.hovered(),
                            scoped.response.hovered(),
                            claim.hovered(),
                        );
                    },
                );
            }
            out
        }
        let ctx = context();
        let row = |ui: &mut egui::Ui| ui::menu_item(ui, icon::CROP, "Edit crop…", ui::MENU_ROW_H);
        let field = |ui: &mut egui::Ui| {
            let mut v = 100.0_f64;
            value_field(
                ui,
                egui::vec2(250.0, 26.0),
                Prefix::Icon(icon::RESIZE),
                &mut v,
                Scrub::whole(0.5).range(1.0..=10_000.0),
                |d| d.suffix("%"),
            )
        };

        for (what, content) in [
            (
                "a menu row",
                &row as &dyn Fn(&mut egui::Ui) -> egui::Response,
            ),
            ("a value field", &field),
        ] {
            // The fixture must reach the hovered state at all.
            let (widget, scope, claim) = hovers(&ctx, true, content);
            assert!(
                widget && claim,
                "{what}: the pointer never landed on it — widget {widget}, claim {claim}"
            );
            assert!(
                !scope,
                "{what}: `Ui::scope`'s response was hovered while live — if this \
                 ever becomes true, the popover is showing two tooltips at once"
            );

            let (widget, scope, claim) = hovers(&ctx, false, content);
            assert!(
                !widget,
                "{what}: a widget inside a disabled Ui reported a hover — if this \
                 becomes true, the second claimant is no longer needed"
            );
            assert!(
                !scope,
                "{what}: `Ui::scope`'s response was hovered — this is the spelling \
                 both dimmed tooltips had first, and it showed nothing"
            );
            assert!(
                claim,
                "{what}: an explicit `Sense::empty()` claimant over the same rect \
                 must see the pointer; it is the only thing that can explain a \
                 dimmed control, and it works by being allocated **last** rather \
                 than by sensing anything (§15 D688)"
            );
        }
    }

    /// The widest number the Scale field can reach (`9999.99%`, since `ui::number(2)` trims
    /// zeros) still clears its half-width field's edge; a `value_field` lays the number out after its prefix.
    #[test]
    fn the_scale_fields_widest_value_clears_its_label() {
        /// The right edge of the rightmost thing a field painted, in its own coordinates.
        fn ink(ctx: &egui::Context, width: f32, mut v: f64) -> f32 {
            let out = ctx.run_ui(Default::default(), |ui| {
                ui.set_max_width(width);
                ui::value_field(
                    ui,
                    egui::vec2(width, 26.0),
                    Prefix::Icon(icon::RESIZE),
                    &mut v,
                    Scrub::fine(0.5, 2).range(1.0..=10_000.0),
                    |d| d.suffix("%").custom_formatter(ui::number(2)),
                );
            });
            out.shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Text(t) => Some(t.pos.x + t.galley.size().x),
                    _ => None,
                })
                .fold(0.0_f32, f32::max)
        }

        let ctx = context();
        // Mirrors `image_card`'s own `(inner - ui::CARD_COL_GAP) / 2.0`.
        let half = (ui::menu_inner_w(crate::app::POPOVER_W, 11.0) - ui::CARD_COL_GAP) / 2.0;
        let end = ink(&ctx, half, 9_999.99);
        assert!(
            end < half,
            "`Scale 9999.99%` runs to {end} in a {half}pt field — the number is \
             over the edge and the field is clipping it"
        );
        assert!(
            half - end > 4.0,
            "only {} of right padding against the 10 on the left; the field reads \
             as overfull even though nothing is cut",
            half - end
        );

        // The same field narrowed must overflow, or the test cannot tell a fit from a clip.
        let narrow = ink(&ctx, 70.0, 9_999.99);
        assert!(
            narrow > 70.0,
            "a 70pt field must overflow (ink ends at {narrow}), or this test \
             cannot tell a fit from a clip"
        );
    }

    /// Image editing's card is one column of one height: `ui::segmented` pads a cell by two points
    /// a side, so `TRACK_CELL` is `CELL` minus that. Reproduces the card's column arithmetic;
    /// `image_edit_card` itself is not driven.
    #[test]
    fn the_image_cards_tracks_and_fields_are_one_height() {
        /// What the card's column paints: the two tracks and the field under
        /// them, top to bottom.
        struct Column {
            tabs: egui::Rect,
            modes: egui::Rect,
            field: egui::Rect,
            /// Two more fields inside one bare `ui.scope`, the adjustment rows' structure.
            scoped: (egui::Rect, egui::Rect),
        }
        /// `track_cell` is what the tracks are asked for; `cell` is the fields' height.
        fn column(ctx: &egui::Context, track_cell: f32, cell: f32) -> Column {
            let out = ctx.run_ui(Default::default(), |ui| {
                // `POPOVER_W` less the card's padding and border: the `inner` its controls use.
                ui.set_max_width(ui::menu_inner_w(crate::app::POPOVER_W, 11.0));
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = IMAGE_CARD_GAP;
                    let inner = ui.available_width();
                    segmented(ui, inner, track_cell, 2, 0, |p, i, r, on| {
                        segment_label(p, r, ["Settings", "Adjustments"][i], on)
                    });
                    segmented(ui, inner, track_cell, 4, 0, |p, i, r, on| {
                        segment_label(p, r, ["Fill", "Fit", "Crop", "Tile"][i], on)
                    });
                    let field = |ui: &mut egui::Ui, name: &str| {
                        ui::field_row(ui, egui::vec2(inner, cell), |ui| {
                            ui.label(name);
                            // The right-to-left tail stretches the frame to its allocated width.
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label("100%");
                                },
                            );
                        });
                    };
                    field(ui, "Scale");
                    // It has to be two rows: a scope's own pitch only shows between items inside it.
                    ui.scope(|ui| {
                        field(ui, "Exposure");
                        field(ui, "Contrast");
                    });
                });
            });
            // Picked by width, ordered by top, de-duplicated because a track paints its
            // box twice (ground, then hairline), which would shift `full[2]`.
            let want = ui::menu_inner_w(crate::app::POPOVER_W, 11.0);
            let mut full: Vec<egui::Rect> = Vec::new();
            for r in painted_rows(&out, 0.0..200.0) {
                if (r.width() - want).abs() < 1.01 && !full.contains(&r) {
                    full.push(r);
                }
            }
            assert!(
                full.len() >= 5,
                "the fixture painted {} full-width rects, not the two tracks and \
                 three fields: {full:?}",
                full.len()
            );
            Column {
                tabs: full[0],
                modes: full[1],
                field: full[2],
                scoped: (full[3], full[4]),
            }
        }
        let ctx = context();

        let c = column(&ctx, 26.0 - 4.0, 26.0);
        assert!(
            c.tabs.top() < c.modes.top() && c.modes.top() < c.field.top(),
            "the fixture is not a column: {:?}",
            (c.tabs, c.modes, c.field)
        );
        assert!(
            (c.tabs.height() - c.field.height()).abs() < 0.51,
            "the tab strip paints {} beside a {} field — `segmented` names the \
             cell and pads two either side, so the number it is asked for is four \
             less than the height wanted",
            c.tabs.height(),
            c.field.height()
        );
        assert!(
            (c.modes.height() - c.field.height()).abs() < 0.51,
            "the mode strip paints {} beside a {} field",
            c.modes.height(),
            c.field.height()
        );
        assert!(
            (c.field.height() - 26.0).abs() < 0.51,
            "the fixture's fields are {} tall, not the card's 26 — this test is \
             measuring against the wrong yardstick",
            c.field.height()
        );
        assert!(
            (c.tabs.left() - c.field.left()).abs() < 0.51
                && (c.tabs.right() - c.field.right()).abs() < 0.51
                && (c.modes.left() - c.field.left()).abs() < 0.51,
            "the card's column is ragged: {:?}",
            (c.tabs, c.modes, c.field)
        );

        // One pitch down the whole column, `IMAGE_CARD_GAP`; rows inside a bare
        // `ui.scope` must not use a pitch of the scope's own.
        for (what, gap) in [
            ("tabs → modes", c.modes.top() - c.tabs.bottom()),
            ("modes → field", c.field.top() - c.modes.bottom()),
            ("field → the scope", c.scoped.0.top() - c.field.bottom()),
            (
                "two rows inside one scope",
                c.scoped.1.top() - c.scoped.0.bottom(),
            ),
        ] {
            assert!(
                (gap - IMAGE_CARD_GAP).abs() < 0.51,
                "{what} sits {gap} apart, not the card's {IMAGE_CARD_GAP} — the \
                 report was that seven rows at a pitch of their own read as a \
                 table rather than as seven controls"
            );
        }

        // The likely mistake: `CELL` passed straight through paints 30 against 26.
        let bare = column(&ctx, 26.0, 26.0);
        assert!(
            (bare.tabs.height() - 30.0).abs() < 0.51
                && (bare.tabs.height() - bare.field.height() - 4.0).abs() < 0.51,
            "a cell of 26 must paint 30, four proud of its field — got {} against \
             {}; if this drifts the comment on TRACK_CELL is describing something \
             else",
            bare.tabs.height(),
            bare.field.height()
        );
    }

    /// A stroke's second-row controls are all as tall as its fields: a `ComboBox` left alone
    /// paints 26px against 28. Asserts nothing is off-height rather than counting 28px rects.
    #[test]
    fn a_strokes_controls_are_all_the_same_height_as_its_fields() {
        /// A stroke's second line, with `fix` deciding whether the combos are told their height.
        fn row(ctx: &egui::Context, fix: bool) -> Vec<egui::Rect> {
            let out = ctx.run_ui(Default::default(), |ui| {
                ui.set_max_width(268.0);
                for (salt, text) in [("probe-align", "Inside"), ("probe-sides", "All sides")] {
                    ui.scope(|ui| {
                        if fix {
                            ui.spacing_mut().interact_size.y = 28.0;
                            ui.spacing_mut().button_padding.y = 0.0;
                        }
                        egui::ComboBox::from_id_salt(salt)
                            .width(120.0)
                            .selected_text(text)
                            .show_ui(ui, |_| {});
                    });
                }
                ui::field_row(ui, egui::vec2(80.0, 28.0), |ui| {
                    ui.label("1");
                });
                ui::field_button(
                    ui,
                    icon::SLIDERS_HORIZONTAL,
                    28.0,
                    15.0,
                    ui::FieldButton::Off,
                );
                ui::field_button(ui, icon::CORNERS_OUT, 28.0, 15.0, ui::FieldButton::Off);
            });
            painted_rows(&out, 20.0..40.0)
        }
        let ctx = context();
        let odd = |rects: &[egui::Rect]| -> Vec<f32> {
            rects
                .iter()
                .map(|r| r.height())
                .filter(|h| (h - 28.0).abs() > 0.51)
                .collect()
        };
        // Unfixed, a `ComboBox` comes out short; pinned so the probe cannot pass vacuously.
        let broken = odd(&row(&ctx, false));
        assert!(
            !broken.is_empty(),
            "a combo left to size itself should not already match the fields"
        );
        let fixed = odd(&row(&ctx, true));
        assert!(
            fixed.is_empty(),
            "these controls painted {fixed:?} instead of 28 — the combos sat off the \
             fields' baseline"
        );
    }

    /// Every state of a menu row is the same height and width, so nothing in a dropdown moves
    /// when the pointer crosses it, at several display scales.
    ///
    /// egui 0.35 builds a `Button` frame from the widget state, so a resting row drops the
    /// stroke from layout while a hovered or selected one keeps it. The painted height must
    /// be `ui::MENU_ROW_H` itself: a floor only raises, so this is the tripwire for a caller
    /// that leaves the padding alone. It covers rows egui sizes, not `ui::menu_item` rows.
    #[test]
    fn a_menu_rows_height_does_not_depend_on_its_state() {
        for (ppp, pad) in [
            (1.0_f32, 0.0_f32),
            (1.0, 2.0),
            (1.25, 0.0),
            (1.25, 2.0),
            (1.5, 2.0),
            (2.0, 0.0),
            (2.0, 2.0),
        ] {
            let ctx = context();
            ctx.set_pixels_per_point(ppp);
            let heights = std::cell::RefCell::new(Vec::<(&str, f32)>::new());
            let widths = std::cell::RefCell::new(Vec::<(&str, f32)>::new());
            let _ = ctx.run_ui(Default::default(), |ui| {
                ui.spacing_mut().interact_size.y = 28.0;
                ui.spacing_mut().button_padding.y = pad;
                ui.scope(|ui| {
                    ui::menu_rows(ui);
                    // Call sites set the padding beside `menu_rows`, not inside it.
                    ui.spacing_mut().button_padding.y = pad;
                    // Selected and resting differ without a pointer, so no synthetic hover.
                    for (name, selected) in [("selected", true), ("resting", false)] {
                        let resp = ui.selectable_label(
                            selected,
                            ui::glyph_and_text(icon::UNITE_SQUARE, "Union"),
                        );
                        heights.borrow_mut().push((name, resp.rect.height()));
                        widths.borrow_mut().push((name, resp.rect.width()));
                    }
                    // A plain string too: the bug was egui's frame arithmetic.
                    let resp = ui.selectable_label(true, "Selected plain");
                    heights
                        .borrow_mut()
                        .push(("selected plain", resp.rect.height()));
                    let resp = ui.selectable_label(false, "Resting plain");
                    heights
                        .borrow_mut()
                        .push(("resting plain", resp.rect.height()));
                });
            });
            let got = heights.borrow();
            let first = got[0].1;
            for (name, h) in got.iter() {
                assert!(
                    (h - first).abs() < 0.01,
                    "{ppp}x pad={pad}: the {name} row is {h} against {first} — a row whose height \
                     depends on its state shifts every row below it"
                );
            }
            // The floor must be the one deciding, or lowering it does nothing.
            assert!(
                (first - ui::MENU_ROW_H).abs() < 0.01,
                "{ppp}x pad={pad}: a row paints at {first} where the floor is {} — a floor only \
                 ever raises, so a floor under the natural height is not a height at \
                 all and lowering it moves nothing",
                ui::MENU_ROW_H
            );
            let w = widths.borrow();
            let wide = w[0].1;
            for (name, got) in w.iter() {
                // Width matters as much as height: a hovered row wanting more width
                // grows the popup and re-lays out every row, and no floor fixes that axis.
                assert!(
                    (got - wide).abs() < 0.01,
                    "{ppp}x pad={pad}: the {name} row is {got} wide against {wide} — a row whose \
                     width depends on its state widens the popup and moves every row in it"
                );
            }
        }
    }

    /// A `ComboBox`'s `width` is what it paints (its border goes inside), so the sides combo
    /// takes the card's full width; the `ui::FIELD_BORDER_H` allowance would leave a 2px notch.
    #[test]
    fn a_combo_paints_the_width_it_is_given_hairline_included() {
        const CARD: f32 = 268.0;
        let ctx = context();
        let painted = |sub: f32| {
            let out = ctx.run_ui(Default::default(), |ui| {
                ui.set_max_width(CARD);
                ui.scope(|ui| {
                    ui.spacing_mut().interact_size.y = 28.0;
                    ui.spacing_mut().button_padding.y = 0.0;
                    egui::ComboBox::from_id_salt(("probe-width", sub as i32))
                        .width(ui.available_width() - sub)
                        .selected_text("All sides")
                        .show_ui(ui, |_| {});
                });
            });
            painted_rows(&out, 27.5..28.5)
                .first()
                .map(|r| r.width())
                .unwrap_or(0.0)
        };
        assert!(
            (painted(0.0) - CARD).abs() < 0.51,
            "the full width painted {}, want {CARD}",
            painted(0.0)
        );
        assert!(
            (painted(ui::FIELD_BORDER_H) - (CARD - ui::FIELD_BORDER_H)).abs() < 0.51,
            "taking the hairline off should visibly shorten it, so the allowance is \
             wrong here rather than harmless"
        );
    }

    /// The custom per-side widths are two rows of two, each half the card less the gap (`1fr 1fr`).
    #[test]
    fn the_custom_side_widths_lay_out_two_by_two() {
        const CARD: f32 = 268.0;
        let ctx = context();
        let out = ctx.run_ui(Default::default(), |ui| {
            ui.set_max_width(CARD);
            for pair in [
                [(0_usize, icon::ARROW_UP), (1, icon::ARROW_RIGHT)],
                [(3_usize, icon::ARROW_LEFT), (2, icon::ARROW_DOWN)],
            ] {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                    let w = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
                    for (i, glyph) in pair {
                        let mut v = i as f64;
                        value_field(
                            ui,
                            egui::vec2(w, 28.0),
                            Prefix::Icon(glyph),
                            &mut v,
                            Scrub::whole(0.5),
                            |d| d,
                        );
                    }
                });
            }
        });
        let painted = painted_rows(&out, 27.5..28.5);
        assert_eq!(painted.len(), 4, "four fields: {painted:?}");
        let tops: Vec<f32> = painted.iter().map(|r| r.top()).collect();
        assert!(
            (tops[0] - tops[1]).abs() < 0.51,
            "the first two must share a line: {tops:?}"
        );
        assert!(
            (tops[2] - tops[3]).abs() < 0.51,
            "and so must the second two: {tops:?}"
        );
        assert!(
            tops[2] > tops[0] + 20.0,
            "the second line has to be below the first: {tops:?}"
        );
        let want = (CARD - 6.0) / 2.0;
        for r in &painted {
            assert!(
                (r.width() - want).abs() < 1.0,
                "field {r:?} is {} wide, want {want}",
                r.width()
            );
        }
        let span = painted[1].right() - painted[0].left();
        assert!(
            (span - CARD).abs() < 1.0,
            "the pair spans {span} of a {CARD} card"
        );
    }

    /// The stroke popover fits four tabs and a close button inside its padding; reads `POPOVER_W`.
    #[test]
    fn the_settings_popup_fits_four_tabs_and_a_close_button() {
        const MENU_W: f32 = crate::app::POPOVER_W;
        const MENU_PAD: f32 = 11.0;
        let inner = ui::menu_inner_w(MENU_W, MENU_PAD);
        // Mirrors the popover's own `inner - 18.0 - ui::CARD_COL_GAP`.
        let tabs = inner - 18.0 - ui::CARD_COL_GAP;
        let cell = (tabs - 4.0 - 2.0 * 3.0) / 4.0;
        assert!(
            cell >= 44.0,
            "a {cell}px cell is too narrow for \"Custom\" at 10.5pt"
        );
        assert!(
            tabs + ui::CARD_COL_GAP + 18.0 <= inner + 0.01,
            "the tab row plus the close button overflows the popup"
        );

        let ctx = context();
        let widest = ctx.fonts_mut(|f| {
            DashStyle::ALL
                .iter()
                .map(|d| {
                    f.layout_no_wrap(
                        d.label().to_owned(),
                        egui::FontId::proportional(10.0),
                        egui::Color32::WHITE,
                    )
                    .size()
                    .x
                })
                .fold(0.0_f32, f32::max)
        });
        assert!(
            widest <= cell - 4.0,
            "the widest tab label inks {widest}px in a {cell}px cell"
        );
    }
}

#[cfg(test)]
mod polish_tests {
    use super::*;

    /// A join is offered only where there is a corner to shape; gated on the kind, not the
    /// radii, so the control does not flicker while the radius is scrubbed.
    #[test]
    fn a_join_is_offered_only_where_there_is_a_corner_to_shape() {
        use ondin_core::geometry::stroke_join_applies;
        use ondin_core::kurbo::{BezPath, RoundedRectRadii, Size};

        let rect = |r: f64| NodeKind::Rect {
            size: Size::new(40.0, 20.0),
            corner_radii: RoundedRectRadii::from_single_radius(r),
        };
        assert!(stroke_join_applies(&rect(0.0)));
        assert!(
            stroke_join_applies(&rect(10.0)),
            "a rounded rect keeps the control: gating on the radii would make it \
             flicker while the radius is scrubbed"
        );
        assert!(stroke_join_applies(&NodeKind::Polygon {
            size: Size::new(10.0, 10.0),
            sides: 3,
            corner_radius: 0.0,
        }));
        assert!(stroke_join_applies(&NodeKind::Star {
            size: Size::new(10.0, 10.0),
            points: 5,
            inner_ratio: 0.382,
            corner_radius: 0.0,
        }));
        assert!(stroke_join_applies(&NodeKind::Path {
            path: BezPath::new(),
            corner_radii: Vec::new(),
        }));
        assert!(
            stroke_join_applies(&NodeKind::Artboard {
                size: Size::new(10.0, 10.0),
            }),
            "a frame is a box, and it strokes now (§15 D144), so its four corners \
             take a join like a rect's"
        );

        for kind in [
            NodeKind::Ellipse {
                size: Size::new(10.0, 10.0),
            },
            NodeKind::Line {
                end: ondin_core::kurbo::Point::new(10.0, 0.0),
            },
            NodeKind::Group,
            NodeKind::Root,
        ] {
            assert!(!stroke_join_applies(&kind), "{kind:?} has no corners");
        }
    }

    /// A boolean gets a Fill and a Stroke panel (via `NodeKind::takes_paint`), with stroke
    /// alignment offered since its derived outline is closed.
    #[test]
    fn a_boolean_takes_paint_and_offers_a_closed_shapes_stroke_controls() {
        use ondin_core::geometry::{stroke_align_applies, stroke_join_applies, stroke_sides_apply};
        let boolean = NodeKind::Boolean {
            op: ondin_core::BoolOp::Subtract,
        };
        assert!(
            boolean.takes_paint(),
            "the one container that paints — and what the panel now gates on"
        );
        assert!(
            stroke_align_applies(&boolean),
            "its derived outline is closed by construction, so inside/outside mean something"
        );
        assert!(
            stroke_join_applies(&boolean),
            "wherever two operands' edges cross is a corner"
        );
        assert!(
            !stroke_sides_apply(&boolean),
            "…but it has no top edge to name, so per-side widths stay off"
        );

        for kind in [NodeKind::Group, NodeKind::Root] {
            assert!(!kind.takes_paint(), "{kind:?} carries no fills or strokes");
        }
        // A frame paints: `inspector_paint_panels` gates both of its cards on this.
        let frame = NodeKind::Artboard {
            size: ondin_core::kurbo::Size::new(10.0, 10.0),
        };
        assert!(
            frame.takes_paint(),
            "a frame fills and strokes along its own box, like every other outline"
        );
        // `can_mask` is `takes_paint`-based, so a frame must still be refused it.
        assert!(
            !frame.can_mask(),
            "a frame clips its children; it does not clip its siblings"
        );
    }

    /// A collapsed sides selector still says it is set: `Set` is reachable only when closed and not on every side.
    #[test]
    fn a_collapsed_sides_selector_still_says_it_is_set() {
        use ondin_core::StrokeSides;

        let state = sides_button_state;
        let custom = StrokeSides::Custom {
            top: 1.0,
            right: 0.0,
            bottom: 4.0,
            left: 2.0,
        };

        assert_eq!(
            state(true, false, StrokeSides::Top),
            ui::FieldButton::Set,
            "a collapsed row with a side set has to say so"
        );
        assert_eq!(state(true, false, custom), ui::FieldButton::Set);
        assert_eq!(state(true, false, StrokeSides::All), ui::FieldButton::Off);
        assert_eq!(state(true, true, StrokeSides::All), ui::FieldButton::On);
        assert_eq!(state(true, true, custom), ui::FieldButton::On);
        assert_eq!(state(false, false, custom), ui::FieldButton::Disabled);

        assert!(ui::FieldButton::Set.is_enabled());
        assert!(!ui::FieldButton::Disabled.is_enabled());
    }

    /// Editing the weight flattens all four `Custom` sides onto it, a zero side included.
    #[test]
    fn editing_the_weight_re_broadcasts_it_to_every_side() {
        use ondin_core::{Stroke, StrokeSides};

        let apply = |stroke: &Stroke, width: f64| {
            let mut s = stroke.clone();
            set_stroke_weight(&mut s, width);
            s
        };

        let custom = Stroke {
            width: 1.0,
            sides: StrokeSides::Custom {
                top: 1.0,
                right: 0.0,
                bottom: 4.0,
                left: 2.0,
            },
            ..Default::default()
        };
        let after = apply(&custom, 6.0);
        assert_eq!(after.width, 6.0);
        assert_eq!(
            after.sides,
            StrokeSides::Custom {
                top: 6.0,
                right: 6.0,
                bottom: 6.0,
                left: 6.0
            },
            "the weight has to reach every side, including the one that was 0"
        );

        for sides in [
            StrokeSides::All,
            StrokeSides::Top,
            StrokeSides::Right,
            StrokeSides::Bottom,
            StrokeSides::Left,
        ] {
            let s = apply(
                &Stroke {
                    sides,
                    ..Default::default()
                },
                6.0,
            );
            assert_eq!(s.sides, sides, "{sides:?} should be left alone");
            assert_eq!(s.width, 6.0);
        }
    }
}

#[cfg(test)]
mod mask_card_tests {
    //! Which layer the Mask card describes: a pure function of the tree.

    use super::{MaskAction, mask_card_targets};
    use ondin_core::kurbo::{RoundedRectRadii, Size};
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    struct Tree {
        doc: Document,
        ids: IdSource,
    }

    impl Tree {
        fn new() -> (Self, NodeId) {
            let mut ids = IdSource::new(0x3A5C);
            let root = ids.mint();
            let doc = Document::new(root);
            (Tree { doc, ids }, root)
        }
        fn add(&mut self, parent: NodeId, kind: NodeKind) -> NodeId {
            let id = self.ids.mint();
            let index = self.doc.get(parent).unwrap().children().len();
            self.doc
                .apply(&Transaction(vec![Operation::CreateNode {
                    id,
                    parent,
                    index,
                    kind,
                    transform: None,
                    name: None,
                }]))
                .unwrap();
            id
        }
        fn shape(&mut self, parent: NodeId) -> NodeId {
            self.add(
                parent,
                NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::default(),
                },
            )
        }
        fn mask(&mut self, id: NodeId) {
            self.doc
                .apply(&Transaction(vec![Operation::SetMask { id, mask: true }]))
                .unwrap();
        }
    }

    /// The card follows the mask group, which is the layer masking leaves selected.
    #[test]
    fn a_group_holding_a_mask_describes_the_mask_inside_it() {
        let (mut t, root) = Tree::new();
        let group = t.add(root, NodeKind::Group);
        let mask = t.shape(group);
        let _subject = t.shape(group);
        assert!(
            mask_card_targets(&t.doc, group).is_empty(),
            "fixture: an ordinary group describes nothing"
        );

        t.mask(mask);
        assert_eq!(
            mask_card_targets(&t.doc, group),
            vec![mask],
            "selecting the group reaches the mask it holds"
        );
        assert_eq!(
            mask_card_targets(&t.doc, mask),
            vec![mask],
            "and selecting the mask itself still answers itself"
        );
    }

    /// Direct children only: a mask nested in an inner group is not reached from the outer one.
    #[test]
    fn a_mask_two_levels_down_is_not_reached_from_the_top() {
        let (mut t, root) = Tree::new();
        let outer = t.add(root, NodeKind::Group);
        let inner = t.add(outer, NodeKind::Group);
        let mask = t.shape(inner);
        let _subject = t.shape(inner);
        t.mask(mask);

        assert_eq!(
            mask_card_targets(&t.doc, inner),
            vec![mask],
            "the group that actually holds it describes it"
        );
        assert!(
            mask_card_targets(&t.doc, outer).is_empty(),
            "and the one above reaches nothing — a group is not a mask"
        );
    }

    /// A group that is a mask and also holds one describes itself.
    #[test]
    fn a_mask_group_that_holds_a_mask_describes_itself() {
        let (mut t, root) = Tree::new();
        let outer = t.add(root, NodeKind::Group);
        let group = t.add(outer, NodeKind::Group);
        let _above = t.shape(outer);
        let inner_mask = t.shape(group);
        let _subject = t.shape(group);
        t.mask(inner_mask);
        t.mask(group);

        assert_eq!(
            mask_card_targets(&t.doc, group),
            vec![group],
            "its own flag wins over the one it contains"
        );
    }

    /// Two masks in one group are two targets, which the card reports as Mixed and writes together.
    #[test]
    fn two_masks_in_one_group_are_both_targets() {
        let (mut t, root) = Tree::new();
        let group = t.add(root, NodeKind::Group);
        let first = t.shape(group);
        let _a = t.shape(group);
        let second = t.shape(group);
        let _b = t.shape(group);
        t.mask(first);
        t.mask(second);

        assert_eq!(
            mask_card_targets(&t.doc, group),
            vec![first, second],
            "both, in child order"
        );
    }

    /// The Release button and the Mask card answer the same question: it releases every mask the
    /// card reports, in one press and one undo step.
    ///
    /// flip: `mask_action`'s Release arm back to `.find(…)` in a `vec![]` → fails at "the button releases every mask the card reports".
    #[test]
    fn releasing_a_mask_releases_every_one_the_card_reports() {
        let ctx = egui::Context::default();
        let mut app = crate::app::OndinApp::headless(&ctx);
        let (mut t, root) = Tree::new();
        let group = t.add(root, NodeKind::Group);
        let first = t.shape(group);
        let _a = t.shape(group);
        let second = t.shape(group);
        let _b = t.shape(group);
        t.mask(first);
        t.mask(second);
        let doc = t.doc.clone();
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![group]);

        assert_eq!(
            app.mask_card_targets(group).len(),
            2,
            "the fixture must reach the two-run state the card already reports"
        );
        let Ok(MaskAction::Release(ids)) = app.mask_action() else {
            panic!("a group holding masks offers Release");
        };
        assert_eq!(
            ids,
            app.mask_card_targets(group),
            "the button releases every mask the card reports"
        );

        app.toggle_mask();
        assert!(
            app.mask_card_targets(group).is_empty(),
            "one press, not two"
        );
        assert!(
            !matches!(app.mask_action(), Ok(MaskAction::Release(_))),
            "so the button is no longer lit"
        );
        app.session.undo();
        assert_eq!(
            app.mask_card_targets(group).len(),
            2,
            "and one undo brings both back"
        );
    }

    /// Each `mask_action` refusal gives its own sentence (not just `is_err()`), and the
    /// same-group check runs before the boolean-parent one.
    ///
    /// flip: the boolean-parent refusal disabled → fails at "a boolean's operands cannot be masked"
    /// with the generic "cannot be wrapped" sentence (redundant for correctness, load-bearing for the message).
    #[test]
    fn every_mask_refusal_says_which_thing_to_fix() {
        let ctx = egui::Context::default();
        let mut app = crate::app::OndinApp::headless(&ctx);
        let (mut t, root) = Tree::new();
        let group = t.add(root, NodeKind::Group);
        let a = t.shape(group);
        let b = t.shape(group);
        let elsewhere = t.shape(root);
        let boolean = t.add(
            root,
            NodeKind::Boolean {
                op: ondin_core::BoolOp::Union,
            },
        );
        let inside_a = t.shape(boolean);
        let inside_b = t.shape(boolean);
        let frame = t.add(
            root,
            NodeKind::Artboard {
                size: Size::new(100.0, 100.0),
            },
        );
        let row = t.add(root, NodeKind::Group);
        t.add(
            row,
            NodeKind::Artboard {
                size: Size::new(100.0, 100.0),
            },
        );
        app.session.adopt_document(t.doc.clone(), None);

        let refusal = |app: &mut crate::app::OndinApp, sel: Vec<NodeId>| {
            app.session.selection.set(sel);
            app.mask_action().err()
        };

        assert_eq!(
            refusal(&mut app, Vec::new()),
            Some("Select a layer to use as a mask"),
            "nothing selected"
        );
        assert_eq!(
            refusal(&mut app, vec![a, elsewhere]),
            Some("A mask and what it masks have to be in the same group"),
            "two layers in different parents"
        );
        assert_eq!(
            refusal(&mut app, vec![inside_a, inside_b]),
            Some("A boolean's operands are combined rather than drawn, so nothing there can mask"),
            "a boolean's operands cannot be masked"
        );
        // A frame beside a shape masks (asserted below); the refusal a frame still earns is
        // a group holding one being the mask itself. flip: `holds_a_frame` disabled → the
        // dry run still refuses, with the generic "cannot be wrapped" sentence.
        assert_eq!(
            refusal(&mut app, vec![row]),
            Some("A group holding a frame cannot be a mask; use a shape above it instead"),
            "a group holding a frame cannot mask"
        );

        // Controls: stop the refusals above being met by a function that refuses everything.
        app.session.selection.set(vec![a, b]);
        assert!(
            matches!(app.mask_action(), Ok(MaskAction::Make { .. })),
            "two shapes in one group is the case the whole control exists for"
        );
        app.session.selection.set(vec![frame, elsewhere]);
        assert!(
            matches!(app.mask_action(), Ok(MaskAction::Make { .. })),
            "a frame beside a shape is masked by it"
        );
    }

    /// Each `booleanable_selection` refusal gives its own sentence, not one count sentence
    /// that is false for a shape plus text or for shapes in different groups.
    ///
    /// flip: both arms collapsed back to the count sentence → fails at the second assertion.
    #[test]
    fn every_boolean_refusal_says_which_thing_to_fix() {
        let ctx = egui::Context::default();
        let mut app = crate::app::OndinApp::headless(&ctx);
        let (mut t, root) = Tree::new();
        let group = t.add(root, NodeKind::Group);
        let a = t.shape(group);
        let b = t.shape(group);
        let elsewhere = t.shape(root);
        let text = t.add(
            group,
            NodeKind::Text {
                content: "hi".into(),
                style: Box::default(),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing: ondin_core::TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
        );
        let holder = t.add(root, NodeKind::Group);
        t.add(
            holder,
            NodeKind::Artboard {
                size: Size::new(100.0, 100.0),
            },
        );
        app.session.adopt_document(t.doc.clone(), None);

        let refusal = |app: &mut crate::app::OndinApp, sel: Vec<NodeId>| {
            app.session.selection.set(sel);
            app.booleanable_selection().err()
        };

        assert_eq!(
            refusal(&mut app, vec![a]),
            Some("Boolean operations need two or more layers selected"),
            "one layer — the sentence that used to answer for all three"
        );
        assert_eq!(
            refusal(&mut app, vec![a, text]),
            Some("Text and artboards cannot be boolean operands"),
            "a shape and a text layer: two ARE selected, so the count sentence was \
             a lie about a selection anyone can make"
        );
        assert_eq!(
            refusal(&mut app, vec![a, elsewhere]),
            Some("A boolean and its operands have to be in the same group"),
            "two shapes in different parents — the case where the mask button in \
             the same row named the reason and this one did not"
        );
        // flip: the frame-holding-group arm removed → fails here with the count sentence.
        assert_eq!(
            refusal(&mut app, vec![holder, elsewhere]),
            Some("A group holding a frame cannot be a boolean operand"),
            "a group holding a frame beside a shape"
        );

        // Control: stops the refusals above being met by a function that refuses everything.
        app.session.selection.set(vec![a, b]);
        assert!(
            app.booleanable_selection().is_ok(),
            "two shapes in one group is the case the dropdown exists for"
        );

        // Ctrl+E's Flatten says the same. flip: `flatten_selection` posting its fixed
        // sentence for every refusal → fails at the kind sentence.
        let flatten = |app: &mut crate::app::OndinApp, sel: Vec<NodeId>| {
            app.session.selection.set(sel);
            app.flatten_selection();
            app.session.status().text.clone()
        };
        assert_eq!(
            flatten(&mut app, vec![a, text]),
            "Text and artboards cannot be boolean operands"
        );
        assert_eq!(
            flatten(&mut app, vec![a]),
            "Select a boolean, or two or more layers, to flatten"
        );
    }

    /// `group_actions` adds one rule over the builder: a lone container offers ungroup and not
    /// group. Both `can_group` and `can_ungroup` are asserted in every row.
    ///
    /// flip: dropping `!lone_container &&` → fails at "a lone group is not something to group".
    #[test]
    fn a_lone_container_offers_ungroup_and_not_group() {
        let ctx = egui::Context::default();
        let mut app = crate::app::OndinApp::headless(&ctx);
        let (mut t, root) = Tree::new();
        let group = t.add(root, NodeKind::Group);
        let inner = t.shape(group);
        let a = t.shape(root);
        let b = t.shape(root);
        app.session.adopt_document(t.doc.clone(), None);

        let actions = |app: &mut crate::app::OndinApp, sel: Vec<NodeId>| {
            app.session.selection.set(sel);
            app.group_actions()
        };

        assert_eq!(
            actions(&mut app, vec![group]),
            (false, true),
            "a lone group is not something to group, and is something to ungroup"
        );
        assert_eq!(
            actions(&mut app, vec![group, a]),
            (true, true),
            "a group beside a layer is both"
        );
        assert_eq!(
            actions(&mut app, vec![a, b]),
            (true, false),
            "two plain layers group and have nothing to ungroup"
        );
        assert_eq!(
            actions(&mut app, vec![inner]),
            (true, false),
            "and a lone *shape* still groups — the rule is about containers"
        );
    }
}

#[cfg(test)]
mod identity_row_tests {
    //! The identity card's first row is the same height for a single layer and for a
    //! multi-selection, so the inspector below does not shift.

    use super::*;

    fn row_height(build: impl Fn(&mut egui::Ui)) -> f32 {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut h = 0.0;
        for _ in 0..3 {
            let _ = ctx.run_ui(Default::default(), |ui| {
                let r = ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 9.0;
                    build(ui);
                });
                h = r.response.rect.height();
            });
        }
        h
    }

    #[test]
    fn the_two_identity_rows_are_the_same_height() {
        let single = row_height(|ui| {
            identity_glyph(ui, icon::SQUARE, color::ACCENT_300);
            let mut name = "Rectangle".to_string();
            ui.add(
                egui::TextEdit::singleline(&mut name)
                    .frame(egui::Frame::NONE)
                    .desired_width(140.0)
                    .font(egui::FontId::proportional(13.0)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                icon_button(ui, icon::LOCK_SIMPLE, 22.0, 16.0, false, true);
                icon_button(ui, icon::EYE, 22.0, 16.0, false, true);
            });
        });
        let multi = row_height(|ui| {
            identity_glyph(ui, icon::BOUNDING_BOX, color::ACCENT_300);
            ui.label(
                egui::RichText::new("3 layers")
                    .size(13.0)
                    .color(theme::text::STRONG),
            );
        });
        assert_eq!(
            single, multi,
            "the first card must not change height between one layer and several \
             — the whole inspector below it moves when it does"
        );
        assert_eq!(
            single, IDENTITY_ROW_H,
            "and the constant should still be describing what they actually measure"
        );
    }

    /// The name field keeps the I-beam: it is frameless, so the cursor is the only signal that
    /// the heading is editable (unlike the chrome eyebrows, `ui::head_tests`).
    #[test]
    fn the_inspector_name_still_offers_the_i_beam_it_is_editable_with() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut name = "Rectangle".to_string();
        let mut at = egui::Pos2::ZERO;
        let mut icon = egui::CursorIcon::default();
        // Three passes: interaction state is a frame behind the layout.
        for _ in 0..3 {
            let out = ctx.run_ui(
                egui::RawInput {
                    events: vec![egui::Event::PointerMoved(at)],
                    ..Default::default()
                },
                |ui| {
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut name)
                            .frame(egui::Frame::NONE)
                            .desired_width(140.0)
                            .font(egui::FontId::proportional(13.0)),
                    );
                    at = resp.rect.center();
                },
            );
            icon = out.platform_output.cursor_icon;
        }
        assert_eq!(
            icon,
            egui::CursorIcon::Text,
            "the name is the one place in the card that can be typed in, and the \
             I-beam is the whole of what says so"
        );
    }
}

#[cfg(test)]
mod press_away_tests {
    use super::*;

    const CARD: egui::Rect = egui::Rect {
        min: egui::pos2(40.0, 30.0),
        max: egui::pos2(290.0, 430.0),
    };
    const HEAD: egui::Rect = egui::Rect {
        min: egui::pos2(300.0, 30.0),
        max: egui::pos2(328.0, 58.0),
    };

    /// Press at `from`, travel to `to`, release, asking `press_began_away` every frame (as the
    /// popover does, so its recording half runs). Returns the release frame's answer.
    fn dragged(from: egui::Pos2, to: egui::Pos2) -> bool {
        let ctx = egui::Context::default();
        let id = egui::Id::new("card");
        let btn = egui::PointerButton::Primary;
        let frame = |events: Vec<egui::Event>| {
            let _ = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |_| {},
            );
            crate::panels::press_began_away(&ctx, id, CARD, HEAD)
        };
        frame(vec![egui::Event::PointerMoved(from)]);
        frame(vec![egui::Event::PointerButton {
            pos: from,
            button: btn,
            pressed: true,
            modifiers: Default::default(),
        }]);
        frame(vec![egui::Event::PointerMoved(to)]);
        frame(vec![egui::Event::PointerButton {
            pos: to,
            button: btn,
            pressed: false,
            modifiers: Default::default(),
        }])
    }

    /// A scrub that starts on the card and ends off it is not a click away: only where the
    /// press began tells that apart. egui clears `press_origin` on the release frame, so the
    /// recording is the subject, and a version that read it directly fails the first assertion.
    #[test]
    fn a_drag_that_began_on_the_card_is_never_a_click_away() {
        assert!(
            !dragged(egui::pos2(160.0, 300.0), egui::pos2(600.0, 700.0)),
            "a scrub off the edge of the card dismissed it"
        );
        assert!(!dragged(egui::pos2(310.0, 40.0), egui::pos2(600.0, 700.0)));
    }

    /// The other half, so the term cannot be a constant `false`.
    #[test]
    fn a_press_that_began_elsewhere_still_dismisses() {
        assert!(dragged(egui::pos2(600.0, 700.0), egui::pos2(610.0, 700.0)));
        assert!(dragged(egui::pos2(700.0, 500.0), egui::pos2(700.0, 500.0)));
    }

    /// Nothing recorded (the first click of a session) counts as away.
    #[test]
    fn a_context_that_has_seen_no_press_counts_as_away() {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(Default::default(), |_| {});
        assert!(crate::panels::press_began_away(
            &ctx,
            egui::Id::new("card"),
            CARD,
            HEAD
        ));
    }
}

#[cfg(test)]
mod spread_tests {
    use super::*;

    fn spread<T>(values: Vec<T>, bare: bool) -> Spread<T> {
        Spread { values, bare }
    }

    /// What a control may say (`agreed`) and what it operates from (`base`) are different
    /// questions: a scrubbed control adds its delta to the base, so a bare target (no stroke
    /// beside one with a 4pt stroke) has no agreement but still a base.
    #[test]
    fn a_bare_target_removes_the_agreement_and_not_the_base() {
        let bare = spread(vec![4.0_f64], true);
        assert_eq!(bare.agreed(), None, "absence agrees with nothing");
        assert_eq!(
            bare.base(),
            Some(4.0),
            "and the field still has a real number to drag from"
        );

        let uniform = spread(vec![4.0_f64, 4.0], false);
        assert_eq!(uniform.agreed(), Some(4.0), "agreeing needs no bare target");
        assert_eq!(uniform.base(), Some(4.0), "and then the two coincide");

        let differ = spread(vec![4.0_f64, 20.0], false);
        assert_eq!(differ.agreed(), None);
        assert_eq!(differ.base(), Some(4.0), "the first, so it is predictable");

        let nothing: Spread<f64> = spread(Vec::new(), true);
        assert_eq!(nothing.agreed(), None);
        assert_eq!(
            nothing.base(),
            None,
            "nothing painted anywhere has nothing to offer either, and the caller \
             falls back to a placeholder — which is only safe because no control is \
             drawn in that state"
        );
    }

    /// `map` asks a second question of the same walk, and `bare` travels with it.
    #[test]
    fn mapping_a_spread_carries_the_bare_flag() {
        let brushes = spread(vec![1.0_f64, 1.0], true);
        let doubled = brushes.map(|v| v * 2.0);
        assert_eq!(doubled.values, vec![2.0, 2.0]);
        assert_eq!(doubled.agreed(), None, "still bare, so still no agreement");
        assert_eq!(doubled.base(), Some(2.0));
    }

    /// The spacing field agrees to the precision it prints (where `Spread::agreed` would say
    /// Mixed for ever) and drags from the mean rather than the first gap.
    #[test]
    fn the_spacing_field_agrees_to_what_it_prints_and_drags_from_the_mean() {
        let (uniform, base) = shown_gap(&[24.0, 24.0]);
        assert_eq!((uniform, base), (Some(24.0), 24.0));

        let (uniform, _) = shown_gap(&[24.0, 24.0001]);
        assert!(
            uniform.is_some_and(|g| (g - 24.0).abs() < 0.001),
            "gaps identical on screen read as {uniform:?}"
        );

        let (uniform, base) = shown_gap(&[10.0, 30.0]);
        assert_eq!(uniform, None, "10 and 30 are not one number");
        assert!(
            (base - 20.0).abs() < 1e-9,
            "the drag base was {base}, not the mean"
        );

        assert_eq!(shown_gap(&[]), (None, 0.0));
    }
}

#[cfg(test)]
mod rotation_field_tests {
    use super::*;

    /// A 100×60 rect's top-centre point, in the node's own space.
    const TOP: Point = Point::new(50.0, -30.0);

    /// The Rotation field's per-frame loop, run headlessly (the arithmetic only; that an
    /// untouched frame writes nothing is covered in `canvas::headless_app_tests`).
    ///
    /// Returns each frame's resulting world transform.
    fn drag(mut world: Affine, pivot: Point, step_deg: f64, frames: usize) -> Vec<Affine> {
        let mut latch: Option<AngleScrub> = None;
        let mut accumulator: Option<f64> = None;
        let mut out = Vec::new();
        for _ in 0..frames {
            let at_rest = shown_degrees(build::Basis::of(world).orientation.angle);
            let shown = latch.map_or(at_rest, |l| l.shown);
            let angle_deg = accumulator.unwrap_or(shown) + step_deg;
            accumulator = Some(angle_deg);
            let (base, from) = AngleScrub::reference(latch, at_rest, world);
            world = turned_about(from, from * pivot, (angle_deg - base).to_radians());
            latch = Some(AngleScrub {
                base,
                world: from,
                shown: angle_deg,
            });
            out.push(world);
        }
        out
    }

    /// A mirrored layer dragged past ±90° turns by exactly what the drag asked for each frame,
    /// not half a circle: §5.7a relabels `Mirror::X` at θ as `Mirror::Y` at θ−π there, and the
    /// field holds state through `AngleScrub` rather than re-reading a decomposition.
    #[test]
    fn a_mirrored_layer_turns_by_the_drag_and_not_half_a_circle() {
        let pivot = Point::new(10.0, 5.0);
        let world = Affine::translate(Vec2::new(200.0, 150.0))
            * Affine::rotate(80.0_f64.to_radians())
            * Affine::FLIP_X;

        let split = build::Basis::of(world);
        assert_eq!(
            split.orientation.mirror,
            build::Mirror::X,
            "the fixture has to be mirrored, or there is no second reading to flip to"
        );
        assert!(
            (split.orientation.angle.to_degrees() - 80.0).abs() < 1e-9,
            "and has to start below 90°, or the drag never crosses the threshold: {}",
            split.orientation.angle.to_degrees()
        );

        let step = 1.0;
        let frames = 21; // 81° through 101°, so the threshold is crossed mid-drag.
        let pivot_world = world * pivot;
        let angle_of = |w: &Affine| {
            let v = (*w * TOP) - pivot_world;
            v.y.atan2(v.x)
        };

        let mut prev = angle_of(&world);
        for (n, w) in drag(world, pivot, step, frames).iter().enumerate() {
            let now = angle_of(w);
            // Wrapped, so the measurement's ±180° seam is not mistaken for a jump.
            let turned = {
                let d = now - prev;
                let t = std::f64::consts::TAU;
                (d + std::f64::consts::PI).rem_euclid(t) - std::f64::consts::PI
            };
            assert!(
                (turned.to_degrees() - step).abs() < 1e-9,
                "frame {n} turned the layer {:.3}° for a {step}° drag — a mirrored \
                 layer used to jump 180° per frame past ±90°, which is the reported \
                 flashing",
                turned.to_degrees()
            );
            prev = now;
        }
    }

    /// Typing back the number already showing moves nothing, so a field may rebuild the
    /// transform every frame. Checked on a mirrored and sheared basis, which a
    /// decomposition-and-rebuild would drop.
    #[test]
    fn a_zero_delta_writes_the_transform_back_unchanged() {
        let world = Affine::translate(Vec2::new(-13.0, 7.5))
            * Affine::rotate(2.1)
            * build::skew_x(0.4)
            * Affine::FLIP_Y
            * Affine::scale_non_uniform(1.5, 0.75);
        let pivot = Point::new(3.0, -4.0);
        assert_eq!(
            turned_about(world, world * pivot, 0.0).as_coeffs(),
            world.as_coeffs(),
            "a zero delta has to be the identity edit, coefficient for coefficient"
        );
    }

    /// Two flips never leave a layer mirrored: each contributes a factor of −1 to the
    /// determinant.
    #[test]
    fn flipping_both_axes_is_a_half_turn_and_not_a_mirror() {
        let odd = Affine::rotate(0.7) * build::skew_x(0.3) * Affine::scale_non_uniform(2.0, 0.5);
        for (name, base) in [("upright", Affine::IDENTITY), ("already turned", odd)] {
            let both = Affine::FLIP_X * Affine::FLIP_Y * base;
            assert_eq!(
                build::Basis::of(both).orientation.mirror,
                build::Basis::of(base).orientation.mirror,
                "{name}: two flips must leave the mirror tag alone"
            );
            // It really is the half turn, not a no-op; the exact matrix, since
            // `Affine::rotate(PI)` has a `sin` of 1.2e-16.
            let half_turn = Affine::new([-1.0, 0.0, 0.0, -1.0, 0.0, 0.0]);
            assert_eq!(
                both.as_coeffs(),
                (half_turn * base).as_coeffs(),
                "{name}: and must be exactly a half turn"
            );
        }
        assert_ne!(
            build::Basis::of(Affine::FLIP_X).orientation.mirror,
            build::Mirror::None,
            "a single flip does mirror, or the test above is about nothing"
        );
    }

    /// The latch puts both sides of the subtraction in one frame; without it the reference
    /// straddles §5.7a's relabel.
    #[test]
    fn a_gesture_measures_from_where_it_began() {
        let world = Affine::rotate(1.4);
        let started = AngleScrub {
            base: 80.0,
            world,
            shown: 92.0,
        };
        assert_eq!(
            AngleScrub::reference(Some(started), -89.0, Affine::rotate(9.9)),
            (80.0, world),
            "a drag in flight ignores the document's own reading, which is the \
             reading that jumps"
        );
        assert_eq!(
            AngleScrub::reference(None, -89.0, world),
            (-89.0, world),
            "and with no gesture in flight the document is the reference — which is \
             also the first frame of a drag, before the widget has said it started"
        );
    }
}

#[cfg(test)]
mod line_length_tests {
    use super::*;

    /// Typing a length moves the end along the line's direction: the start does not move, the
    /// direction is kept exactly, and a degenerate line has no answer.
    ///
    /// flip: anchoring the far end (`end - v * k`) → fails at the shortening assertion (answers
    /// (22, 36)); dropping the `/ current` → fails there too.
    #[test]
    fn a_typed_length_moves_the_end_and_leaves_the_start_and_the_angle_alone() {
        let a = Point::new(10.0, 20.0);
        let b = Point::new(28.0, 44.0);
        assert_eq!((b - a).hypot(), 30.0, "the fixture is 30 long");

        // A third, not a half: halving lands on the midpoint from either end.
        let third = line_end_for_length(a, b, 10.0).expect("a line with direction");
        assert_eq!(
            third,
            Point::new(16.0, 28.0),
            "measured from the start, which does not move — anchoring the far end \
             would answer (22, 36)"
        );
        assert!(
            ((third - a).hypot() - 10.0).abs() < 1e-12,
            "and it is the length that was asked for: {third:?}"
        );

        let twice = line_end_for_length(a, b, 60.0).expect("a line with direction");
        assert_eq!(
            (twice.y - a.y).atan2(twice.x - a.x),
            (b.y - a.y).atan2(b.x - a.x),
            "the direction has to survive exactly, or every edit churns the file"
        );

        assert_eq!(line_end_for_length(a, b, 30.0), Some(b));

        let d = Point::new(1.0, 1.0);
        let out = line_end_for_length(Point::ZERO, d, 2.0f64.sqrt() * 3.0).expect("diagonal");
        assert!(
            (out.x - out.y).abs() < 1e-12,
            "a 45° line stays at 45°: {out:?}"
        );

        assert_eq!(line_end_for_length(a, a, 10.0), None);
        assert_eq!(line_end_for_length(a, b, 0.0), None);
        assert_eq!(line_end_for_length(a, b, f64::NAN), None);
    }

    /// Which size fields the card draws: a lone line draws `L` and neither `W` nor `H`, a rect
    /// draws `W` and `H`, and a degenerate line (end on its origin) draws none. Drives
    /// `inspector_single`, since the `size0` match lives in the caller.
    ///
    /// flip: `NodeKind::Line` added to `size0`'s derived-box arm → fails at the line assertion
    /// (`["W", "H", "L"]`); dropping the `> 1e-9` gate on `line_ends` → fails at the point
    /// assertion (`["L"]`).
    #[test]
    fn a_line_trades_w_and_h_for_one_l_and_a_point_gets_no_size_field_at_all() {
        use crate::app::OndinApp;

        /// The size-row prefixes (`W`, `H`, `L`) the single-layer panel paints for `kind`, top down.
        fn size_prefixes(kind: NodeKind) -> Vec<String> {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let mut app = OndinApp::headless(&ctx);
            let mut ids = ondin_core::IdSource::new(1);
            let root = ids.mint();
            let mut doc = ondin_core::Document::new(root);
            let id = ids.mint();
            doc.apply(&Transaction(vec![Operation::CreateNode {
                id,
                parent: root,
                index: 0,
                kind,
                // Off the origin, so position bugs stay visible to later additions.
                transform: Some(Affine::translate((10.0, 20.0))),
                name: None,
            }]))
            .expect("the fixture node");
            app.session.adopt_document(doc, None);
            app.session.selection.set(vec![id]);

            let mut found = Vec::new();
            // Three passes: ids and sizes settle on the first two.
            for _ in 0..3 {
                let out = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(280.0, 2000.0),
                        )),
                        ..Default::default()
                    },
                    |ui| app.inspector_single(ui, id),
                );
                found = out
                    .shapes
                    .iter()
                    .filter_map(|c| match &c.shape {
                        egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                        _ => None,
                    })
                    .filter(|t| matches!(t.as_str(), "W" | "H" | "L"))
                    .collect();
            }
            found
        }

        // The control: the kind that keeps both dimensions.
        let rect = size_prefixes(NodeKind::Rect {
            size: Size::new(100.0, 40.0),
            corner_radii: Default::default(),
        });
        assert_eq!(
            rect,
            ["W", "H"],
            "every other kind keeps its two dimensions; without this the line cases \
             below pass against a card that draws no size row at all"
        );

        let line = size_prefixes(NodeKind::Line {
            end: Point::new(18.0, 24.0),
        });
        assert_eq!(
            line,
            ["L"],
            "a line trades W and H for one length (§15 D229), and gets exactly one \
             field rather than three"
        );

        let point = size_prefixes(NodeKind::Line { end: Point::ZERO });
        assert!(
            point.is_empty(),
            "a zero-length line has no direction to grow along, so the field is \
             withheld rather than offered at 0 over a write that cannot happen; \
             this card painted {point:?}"
        );
    }
}

#[cfg(test)]
mod frame_fill_tests {
    //! A frame's paint panel is headed *Fill*, like every other layer's, exactly once. The
    //! strings are shouted because the title is uppercased in `crate::ui::eyebrow`.
    //!
    //! flip: `"Frame background"` put back → fails at "the old title is gone".

    use super::*;
    use crate::app::OndinApp;

    /// The panel headers `inspector_single` paints for `kind`, top down, filtered from the
    /// galleys to the words that are panel titles here.
    fn panel_titles(kind: NodeKind) -> Vec<String> {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let id = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind,
            transform: Some(Affine::translate((10.0, 20.0))),
            name: None,
        }]))
        .expect("the fixture node");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![id]);

        let mut found = Vec::new();
        // Three passes: ids and sizes settle on the first two.
        for _ in 0..3 {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(280.0, 4000.0),
                    )),
                    ..Default::default()
                },
                |ui| app.inspector_single(ui, id),
            );
            found = out
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .filter(|t| {
                    matches!(
                        t.as_str(),
                        "FILL" | "STROKE" | "FRAME BACKGROUND" | "GROUP COLORS"
                    )
                })
                .collect();
        }
        found
    }

    #[test]
    fn a_frames_paint_panel_is_headed_fill_exactly_once_and_never_frame_background() {
        // The control: without it the frame assertions would pass against a probe that
        // collects no headers.
        let rect = panel_titles(NodeKind::Rect {
            size: Size::new(100.0, 40.0),
            corner_radii: Default::default(),
        });
        assert_eq!(
            rect.iter().filter(|t| *t == "FILL").count(),
            1,
            "a rect has one Fill panel: {rect:?}"
        );

        let frame = panel_titles(NodeKind::Artboard {
            size: Size::new(200.0, 200.0),
        });
        assert!(
            !frame.iter().any(|t| t == "FRAME BACKGROUND"),
            "the old title is gone: {frame:?}"
        );
        assert_eq!(
            frame.iter().filter(|t| *t == "FILL").count(),
            1,
            "and it is Fill — once, not twice: {frame:?}"
        );
        let paint: Vec<&String> = frame
            .iter()
            .filter(|t| *t == "FILL" || *t == "STROKE")
            .collect();
        assert_eq!(
            paint,
            ["FILL", "STROKE"],
            "Fill above Stroke, as on every other layer: {frame:?}"
        );
    }

    /// Every galley `inspector_single` paints for a node of `kind` carrying
    /// `fills`, in draw order. `panel_titles`' sibling, unfiltered.
    fn galleys(kind: NodeKind, fills: Vec<Fill>) -> Vec<String> {
        galleys_holding(kind, fills, Vec::new())
    }

    /// `galleys` with a rectangle inside the subject filled with `inside`, or no child when empty.
    /// A child makes the *Group Colors* card appear.
    fn galleys_holding(kind: NodeKind, fills: Vec<Fill>, inside: Vec<Fill>) -> Vec<String> {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let id = ids.mint();
        // The paint is a second, conditional transaction: `SetFills` on a `Group` is
        // `WrongKindForOp` however empty the list.
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind,
            transform: Some(Affine::translate((10.0, 20.0))),
            name: None,
        }]))
        .expect("the fixture node");
        if !fills.is_empty() {
            doc.apply(&Transaction(vec![Operation::SetFills {
                id,
                fills: ondin_core::keyed_by_position(fills),
            }]))
            .expect("the fixture node's paint");
        }
        if !inside.is_empty() {
            let child = ids.mint();
            doc.apply(&Transaction(vec![
                Operation::CreateNode {
                    id: child,
                    parent: id,
                    index: 0,
                    kind: NodeKind::Rect {
                        size: Size::new(20.0, 20.0),
                        corner_radii: Default::default(),
                    },
                    transform: None,
                    name: None,
                },
                Operation::SetFills {
                    id: child,
                    fills: ondin_core::keyed_by_position(inside),
                },
            ]))
            .expect("the child");
        }
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![id]);

        let mut found = Vec::new();
        for _ in 0..3 {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(280.0, 4000.0),
                    )),
                    ..Default::default()
                },
                |ui| app.inspector_single(ui, id),
            );
            found = out
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect();
        }
        found
    }

    /// A frame's Fill card draws a row per fill, the same rows a rect's does (counted by the
    /// hex field each colour row carries), and the frame-only word *Transparent* is gone.
    #[test]
    fn a_frames_fill_card_draws_one_row_per_fill_just_as_a_rects_does() {
        let two = vec![
            Fill {
                brush: Brush::Solid(ondin_core::peniko::Color::from_rgb8(0x01, 0x02, 0x03)).into(),
                visible: true,
            },
            Fill {
                brush: Brush::Solid(ondin_core::peniko::Color::from_rgb8(0x07, 0x08, 0x09)).into(),
                visible: true,
            },
        ];
        let rect = NodeKind::Rect {
            size: Size::new(100.0, 40.0),
            corner_radii: Default::default(),
        };
        let frame = NodeKind::Artboard {
            size: Size::new(200.0, 200.0),
        };

        let hexes = |texts: Vec<String>| -> Vec<String> {
            texts
                .into_iter()
                .filter(|t| ["010203", "070809"].contains(&t.as_str()))
                .collect()
        };
        let on_a_rect = hexes(galleys(rect, two.clone()));
        assert_eq!(
            on_a_rect,
            ["070809", "010203"],
            "a rect draws a row per fill, newest on top"
        );
        let on_a_frame = galleys(frame.clone(), two);
        assert_eq!(
            hexes(on_a_frame.clone()),
            on_a_rect,
            "and a frame draws exactly the same rows, with nothing over: {on_a_frame:?}"
        );
        assert!(
            !on_a_frame.iter().any(|t| t == "GROUP COLORS"),
            "an empty frame has no contents to list: {on_a_frame:?}"
        );

        // An empty Fill card auto-collapses (`sync_paint_collapse`), so *No fill* is
        // never laid out; the claim is that the card behaves like a rect's.
        let bare_frame = galleys(frame, Vec::new());
        let bare_rect = galleys(
            NodeKind::Rect {
                size: Size::new(100.0, 40.0),
                corner_radii: Default::default(),
            },
            Vec::new(),
        );
        assert!(
            !bare_frame.iter().any(|t| t == "Transparent"),
            "the frame-only word is gone: {bare_frame:?}"
        );
        let after_fill = |texts: &[String]| -> Option<String> {
            let i = texts.iter().position(|t| t == "FILL")?;
            texts.get(i + 1).cloned()
        };
        assert_eq!(
            after_fill(&bare_frame),
            after_fill(&bare_rect),
            "an empty Fill card looks the same on both: {bare_frame:?}"
        );
    }

    /// Group Colors on a selected frame lists what is inside it, not the frame's own fills
    /// (a frame filled `010203` holding a rect filled `070809`); the `1 match` count is read
    /// back out of the row.
    ///
    /// flip: `group_color_scope` returning `ids.to_vec()` unconditionally → fails at the hex list.
    #[test]
    fn group_colors_on_a_frame_lists_its_contents_and_not_its_own_fill() {
        let frame_ink = "010203";
        let child_ink = "070809";
        let solid = |r, g, b| {
            vec![Fill {
                brush: Brush::Solid(ondin_core::peniko::Color::from_rgb8(r, g, b)).into(),
                visible: true,
            }]
        };
        let texts = galleys_holding(
            NodeKind::Artboard {
                size: Size::new(200.0, 200.0),
            },
            solid(0x01, 0x02, 0x03),
            solid(0x07, 0x08, 0x09),
        );

        let head = texts
            .iter()
            .position(|t| t == "GROUP COLORS")
            .unwrap_or_else(|| panic!("a frame with a child gets a Group Colors card: {texts:?}"));

        let below: Vec<&String> = texts[head..]
            .iter()
            .filter(|t| [frame_ink, child_ink].contains(&t.as_str()))
            .collect();
        assert_eq!(
            below,
            [child_ink],
            "the child's colour, and only it: {texts:?}"
        );
        assert!(
            texts[..head].iter().any(|t| t == frame_ink),
            "the frame's own fill is in the Fill card: {texts:?}"
        );

        assert!(
            texts[head..].iter().any(|t| t == "1 match"),
            "one layer carries it — the rect, not the frame as well: {texts:?}"
        );
    }

    /// A group is unaffected: it has no paint, so excluding the selected container from its own
    /// census removes nothing. The flip does not bite (reverting `group_color_scope` leaves this
    /// green); it guards a non-regression.
    #[test]
    fn group_colors_inside_a_group_are_unchanged_by_the_narrowing() {
        let child_ink = "070809";
        let texts = galleys_holding(
            NodeKind::Group,
            Vec::new(),
            vec![Fill {
                brush: Brush::Solid(ondin_core::peniko::Color::from_rgb8(0x07, 0x08, 0x09)).into(),
                visible: true,
            }],
        );
        let head = texts
            .iter()
            .position(|t| t == "GROUP COLORS")
            .unwrap_or_else(|| panic!("a group with a child gets the card: {texts:?}"));
        assert!(
            texts[head..].iter().any(|t| t == child_ink),
            "its contents' colour is listed exactly as before: {texts:?}"
        );
        assert!(
            texts[head..].iter().any(|t| t == "1 match"),
            "one layer carries it: {texts:?}"
        );
    }

    /// The Group Colors edit is scoped like the census, so the match count is true of what pressing
    /// it changes: a frame and the rect inside it carrying the same colour tell the scopes
    /// apart. Asserted on the transaction, one `SetFills` naming the child and none the frame.
    ///
    /// flip: `self.session.selection.ids()` put back in `slot_transaction`'s `GroupColor` arm →
    /// fails at the length assertion (two `SetFills`); the other tests in the module stay green.
    #[test]
    fn the_group_colors_edit_reaches_exactly_the_layers_the_row_counted() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let (frame, child) = (ids.mint(), ids.mint());
        let red = ondin_core::peniko::Color::from_rgb8(0xEB, 0x6E, 0x5A);
        let blue = ondin_core::peniko::Color::from_rgb8(0x3C, 0x78, 0xDC);
        let solid = |c| {
            ondin_core::keyed_by_position([Fill {
                brush: Brush::Solid(c).into(),
                visible: true,
            }])
        };
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(200.0, 200.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: child,
                parent: frame,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 20.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            },
        ]))
        .expect("the fixture");
        doc.apply(&Transaction(vec![
            Operation::SetFills {
                id: frame,
                fills: solid(red),
            },
            Operation::SetFills {
                id: child,
                fills: solid(red),
            },
        ]))
        .expect("both red");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![frame]);

        // The control: the census counts one layer, so the assertion below cannot pass
        // against an empty scope.
        let key = red.to_rgba8().to_u8_array();
        let census = ondin_core::colors_in(&app.session.doc, &app.group_color_scope());
        let counted = census
            .iter()
            .find(|u| u.color.to_rgba8().to_u8_array() == key)
            .expect("red is in the census");
        assert_eq!(counted.nodes, vec![child], "the child, not the frame");

        let tx = app.slot_transaction(root, PaintSlot::GroupColor(key), Brush::Solid(blue));
        assert_eq!(
            tx.0.len(),
            counted.nodes.len(),
            "one operation per counted layer: {:?}",
            tx.0
        );
        assert!(
            matches!(&tx.0[0], Operation::SetFills { id, .. } if *id == child),
            "and it is the child's fills: {:?}",
            tx.0
        );
        assert!(
            !tx.0.iter().any(|op| op.overwrites() == Some(frame)),
            "the frame's own ground is the Fill card's business, not this row's: {:?}",
            tx.0
        );
    }

    /// A click over a Group Colors row moves the picker onto the colour it just made, as a drag
    /// does: a `GroupColor` key is the colour, so `write_slot` must retarget it or `slot_brush`
    /// finds nothing and `picker_ui` shuts the picker. Asserts the slot moved and that
    /// `slot_brush` resolves (the picker stays open); `SelectionAll` is not exercised here.
    ///
    /// flip: `retarget_after`'s `GroupColor` arm removed → fails at "the slot follows the colour it became".
    #[test]
    fn a_click_over_a_group_colors_row_takes_the_picker_with_it() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(11);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let (a, b) = (ids.mint(), ids.mint());
        let red = ondin_core::peniko::Color::from_rgb8(0xEB, 0x6E, 0x5A);
        let blue = ondin_core::peniko::Color::from_rgb8(0x3C, 0x78, 0xDC);
        let solid = |c| {
            ondin_core::keyed_by_position([Fill {
                brush: Brush::Solid(c).into(),
                visible: true,
            }])
        };
        let mut ops = Vec::new();
        for (i, id) in [a, b].into_iter().enumerate() {
            ops.push(Operation::CreateNode {
                id,
                parent: root,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 20.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            });
        }
        doc.apply(&Transaction(ops)).expect("two rects");
        doc.apply(&Transaction(vec![
            Operation::SetFills {
                id: a,
                fills: solid(red),
            },
            Operation::SetFills {
                id: b,
                fills: solid(red),
            },
        ]))
        .expect("both red");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![a, b]);

        let key = red.to_rgba8().to_u8_array();
        let next = blue.to_rgba8().to_u8_array();
        let slot = PaintSlot::GroupColor(key);
        assert!(
            app.slot_brush(root, slot).is_some(),
            "the fixture: the Group Colors row is showing red"
        );
        app.picker = Some(Picker::new(root, slot, egui::pos2(0.0, 0.0)));

        app.write_slot(root, slot, Brush::Solid(blue));

        let moved = app.picker.as_ref().expect("the picker is still open").slot;
        assert_eq!(
            moved,
            PaintSlot::GroupColor(next),
            "the slot follows the colour it became"
        );
        assert!(
            app.slot_brush(root, moved).is_some(),
            "and it resolves — which is the picker staying open rather than shutting \
             itself on the next frame"
        );
    }
}

// The origin row's tests live in `canvas.rs` (`origin_field_tests`, `origin_pill_tests`).

#[cfg(test)]
mod spacing_readout_tests {
    //! Which tree the spacing field reads. `EditorSession` is device-free, so this is
    //! testable where the panel that draws the field is not.

    use super::layer_spacings;
    use crate::session::EditorSession;
    use ondin_core::build::Axis;
    use ondin_core::kurbo::{RoundedRectRadii, Size};
    use ondin_core::{NodeId, NodeKind, Operation, Transaction, build};

    /// Three 10-wide rects at x = 0, 30 and 60 inside the starter artboard, all
    /// selected — two gaps of 20.
    fn three_selected() -> (EditorSession, Vec<NodeId>) {
        let mut s = EditorSession::new();
        let parent = s.doc.get(s.doc.first_page()).unwrap().children()[0];
        let mut made = Vec::new();
        for (i, x) in [0.0, 30.0, 60.0].into_iter().enumerate() {
            let id = s.ids.mint();
            assert!(s.commit(Transaction(vec![Operation::CreateNode {
                id,
                parent,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::default(),
                },
                transform: Some(ondin_core::kurbo::Affine::translate((x, 0.0))),
                name: None,
            }])));
            made.push(id);
        }
        s.selection.set(made.clone());
        (s, made)
    }

    /// The spacing readout follows a live preview: a scrubbed field re-reads its base every frame,
    /// so a base from the committed tree would reset the gap each frame.
    #[test]
    fn the_spacing_readout_follows_a_live_preview_rather_than_the_committed_tree() {
        let (mut s, ids) = three_selected();
        assert_eq!(
            layer_spacings(&s, Axis::X),
            vec![20.0, 20.0],
            "the fixture should start at two gaps of 20"
        );

        let tx = build::distribute_spacing(&s.doc, &s.resolved, &ids, Axis::X, None, 40.0);
        assert!(!tx.0.is_empty(), "the preview transaction moved nothing");
        s.set_preview(&tx);

        let shown = layer_spacings(&s, Axis::X);
        assert!(
            shown.iter().all(|g| (g - 40.0).abs() < 1e-6),
            "the field would read {shown:?} while previewing 40 — the base cannot \
             move, so the scrub resets to the committed gap every frame"
        );
        assert_eq!(
            build::gaps_along(
                &ids.iter()
                    .filter_map(|id| s.resolved.world_bounds(*id))
                    .collect::<Vec<_>>(),
                Axis::X
            ),
            vec![20.0, 20.0],
            "the committed tree should not have moved"
        );
    }
}

#[cfg(test)]
mod scale_row_tests {
    //! The Transform card's Scale field: which verb, which box.
    use super::*;
    use crate::app::OndinApp;
    use ondin_core::kurbo::{Affine, Size};

    /// A headless app holding `n` rects 100 wide, 200 apart, all selected, each with a 2pt stroke
    /// only if `stroke` is set (`world_bounds` includes the ink, which offsets every number).
    fn app_with_rects(ctx: &egui::Context, n: usize, stroke: bool) -> (OndinApp, Vec<NodeId>) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let made: Vec<NodeId> = (0..n)
            .map(|i| {
                let id = ids.mint();
                doc.apply(&Transaction(vec![Operation::CreateNode {
                    id,
                    parent: root,
                    index: i,
                    kind: NodeKind::Rect {
                        size: Size::new(100.0, 100.0),
                        corner_radii: Default::default(),
                    },
                    transform: Some(Affine::translate((200.0 * i as f64, 0.0))),
                    name: None,
                }]))
                .expect("a rect");
                if stroke {
                    doc.apply(&Transaction(vec![Operation::SetStrokes {
                        id,
                        strokes: ondin_core::keyed_by_position([Stroke {
                            width: 2.0,
                            ..Default::default()
                        }]),
                    }]))
                    .expect("a stroke");
                }
                id
            })
            .collect();
        app.session.adopt_document(doc, None);
        app.session.selection.set(made.clone());
        (app, made)
    }

    fn stroke_width(app: &OndinApp, id: NodeId) -> f64 {
        app.session
            .doc
            .get(id)
            .and_then(|n| n.paint().strokes.first().map(|s| s.width))
            .expect("a stroke")
    }

    /// The rect's authored size, where a §5.6 resize writes; read instead of `world_bounds` where ink is in the way.
    fn authored(app: &OndinApp, id: NodeId) -> Size {
        match app.session.doc.get(id).map(|n| n.kind()) {
            Some(NodeKind::Rect { size, .. }) => *size,
            other => panic!("the fixture makes rects, not {other:?}"),
        }
    }

    /// The Scale field is the Scale tool's verb, not W and H's: it takes the stroke with it,
    /// where every W/H path passes `Resize::geometry` and leaves it.
    ///
    /// flip: `Resize::geometry(false, false)` as in the W field → box doubles, stroke stays at 2.
    #[test]
    fn scaling_takes_the_stroke_with_it_where_a_width_edit_does_not() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, ids) = app_with_rects(&ctx, 1, true);
        let id = ids[0];
        assert_eq!(stroke_width(&app, id), 2.0, "the fixture's own stroke");
        assert_eq!(authored(&app, id).width, 100.0, "the fixture's own box");

        let tx = app.scale_tx(200.0).expect("a box to scale");
        app.session.try_commit(tx).expect("the scale to take");

        assert_eq!(
            authored(&app, id).width,
            200.0,
            "200% should double the box"
        );
        assert_eq!(
            stroke_width(&app, id),
            4.0,
            "and take the stroke with it — that is the whole difference from W"
        );
        // Everything scaled by one factor, so the inked box doubles too (202 under
        // `Resize::geometry`).
        assert_eq!(
            app.session.resolved.world_bounds(id).map(|b| b.width()),
            Some(204.0),
            "the picture doubles, ink and all: 102 was the fixture's inked width"
        );
    }

    /// Typing `50` one digit at a time compounds (5% then 50% of that is 2.5%): the scale
    /// transaction is relative to the box it is about to write, so a per-keystroke commit is
    /// wrong. The fix is asserted by `typing_a_value_leaves_exactly_one_step_in_the_history`.
    #[test]
    fn scaling_compounds_which_is_why_a_commit_per_keystroke_was_wrong() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);

        let (mut typed, ids) = app_with_rects(&ctx, 1, false);
        assert_eq!(
            authored(&typed, ids[0]).width,
            100.0,
            "the fixture's own box"
        );
        for pct in [5.0, 50.0] {
            let tx = typed.scale_tx(pct).expect("a box to scale");
            typed.session.try_commit(tx).expect("the scale to take");
        }

        let (mut once, ids2) = app_with_rects(&ctx, 1, false);
        let tx = once.scale_tx(50.0).expect("a box to scale");
        once.session.try_commit(tx).expect("the scale to take");

        assert_eq!(
            authored(&once, ids2[0]).width,
            50.0,
            "50% of 100 is the answer the field is asking for"
        );
        assert_eq!(
            authored(&typed, ids[0]).width,
            2.5,
            "and 50% of the 5% the previous keystroke left is what it got"
        );
    }

    /// The Scale row costs the card exactly one row: 37 points, a 28pt row plus the 9 between
    /// rows. A geometry test beside the id probe, since giving `OndinApp::transform_row`'s
    /// `UiBuilder` the row's layout directly keeps ids right but inflates every row.
    #[test]
    fn the_scale_row_costs_the_card_one_row() {
        use ondin_core::kurbo::Affine;
        fn card_height(tool: crate::tools::Tool) -> f32 {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            let (mut app, ids) = app_with_rects(&ctx, 1, false);
            app.tool = tool;
            let id = ids[0];
            let world = app
                .session
                .resolved
                .world_transform(id)
                .unwrap_or(Affine::IDENTITY)
                .as_coeffs();
            let size0 = authored(&app, id);
            let mut h = 0.0;
            // Three passes: ids and sizes settle on the first two.
            for _ in 0..3 {
                let _ = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(280.0, 900.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        let before = ui.min_rect().height();
                        app.inspector_transform(ui, id, world, Some(size0));
                        h = ui.min_rect().height() - before;
                    },
                );
            }
            h
        }
        let (with, without) = (
            card_height(crate::tools::Tool::Scale),
            card_height(crate::tools::Tool::Select),
        );
        assert_eq!(
            with - without,
            37.0,
            "the Scale row should cost one row and its gap: {with} armed against              {without} not"
        );
    }
    /// A typed value lands when the field is clicked away from, not only on `Enter`: egui's
    /// write-back can arrive a frame after focus is lost, and the shared valve must not lose the
    /// number (a bare `egui::DragValue` does).
    #[test]
    fn a_typed_value_lands_when_the_field_is_clicked_away_from() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, ids) = app_with_rects(&ctx, 1, false);
        app.tool = crate::tools::Tool::Scale;
        let depth0 = app.session.history.undo_depth();
        let mut at = egui::Pos2::ZERO;
        let run = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let mut got = None;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(280.0, 900.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    got = app.scale_row(ui, 120.0).map(|r| r.rect);
                },
            );
            got
        };
        for _ in 0..2 {
            at = run(&mut app, Vec::new())
                .expect("the row to be drawn under the Scale tool")
                .center();
        }
        let click = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        run(&mut app, vec![egui::Event::PointerMoved(at)]);
        run(&mut app, vec![click(at, true)]);
        run(&mut app, vec![click(at, false)]);
        run(&mut app, Vec::new());
        for ch in "50".chars() {
            run(&mut app, vec![egui::Event::Text(ch.to_string())]);
        }
        let away = egui::pos2(5.0, 850.0);
        run(&mut app, vec![click(away, true)]);
        run(&mut app, vec![click(away, false)]);
        run(&mut app, Vec::new());
        run(&mut app, Vec::new());

        assert_eq!(
            authored(&app, ids[0]).width,
            50.0,
            "the typed number landed without an Enter to confirm it"
        );
        assert_eq!(
            app.session.history.undo_depth() - depth0,
            1,
            "and clicking away is still one step, not one per keystroke"
        );
    }

    /// Drive the Scale field the way a hand does: focus it, type `text`, then press `finish`.
    /// Returns the rect's authored width afterwards.
    fn typed_into_the_field(text: &str, finish: egui::Key) -> f64 {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, ids) = app_with_rects(&ctx, 1, false);
        app.tool = crate::tools::Tool::Scale;
        drive_field(&ctx, &mut app, text, finish);
        authored(&app, ids[0]).width
    }

    /// The driving half of `typed_into_the_field`, over an app the caller built.
    fn drive_field(ctx: &egui::Context, app: &mut OndinApp, text: &str, finish: egui::Key) {
        let mut at = egui::Pos2::ZERO;
        let run = |app: &mut OndinApp, events: Vec<egui::Event>, pos: Option<egui::Pos2>| {
            let mut got = None;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(280.0, 900.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    got = app.scale_row(ui, 120.0).map(|r| r.rect);
                },
            );
            let _ = pos;
            got
        };

        for _ in 0..2 {
            at = run(app, Vec::new(), None)
                .expect("the row to be drawn under the Scale tool")
                .center();
        }

        // A press and release puts a `DragValue` into text-edit mode; a drag would scrub it.
        let click = |pressed: bool| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        run(app, vec![egui::Event::PointerMoved(at)], None);
        run(app, vec![click(true)], None);
        run(app, vec![click(false)], None);
        run(app, Vec::new(), None);

        for ch in text.chars() {
            run(app, vec![egui::Event::Text(ch.to_string())], None);
        }
        run(
            app,
            vec![egui::Event::Key {
                key: finish,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            None,
        );
        run(app, Vec::new(), None);
    }

    /// A typed edit is one undo step, not one per keystroke: `edit_valve` must not commit on
    /// per-keystroke `changed()` nor on `lost_focus()`, which is true on two consecutive frames.
    /// Driven on the Scale field, but the valve under test is the shared one.
    #[test]
    fn typing_a_value_leaves_exactly_one_step_in_the_history() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, ids) = app_with_rects(&ctx, 1, false);
        app.tool = crate::tools::Tool::Scale;
        let depth0 = app.session.history.undo_depth();

        drive_field(&ctx, &mut app, "50", egui::Key::Enter);

        assert_eq!(
            authored(&app, ids[0]).width,
            50.0,
            "the edit should have landed"
        );
        assert_eq!(
            app.session.history.undo_depth() - depth0,
            1,
            "one edit is one step: `5`, `50` and two frames of losing focus are \
             not four edits"
        );
        app.session.undo();
        assert_eq!(
            authored(&app, ids[0]).width,
            100.0,
            "and one undo puts it all the way back"
        );
    }

    /// Enter commits what was typed, once: the good path, and the control for the two below.
    #[test]
    fn typing_fifty_and_pressing_enter_halves_the_layer_exactly_once() {
        assert_eq!(
            typed_into_the_field("50", egui::Key::Enter),
            50.0,
            "50% of a 100pt rect is 50 — not 5, which is what a commit per \
             keystroke leaves"
        );
    }

    /// The field reads out a drag in flight: the previewed box against the committed one. Driven
    /// through a preview, the only state where they differ, over two rects so the box is a union.
    #[test]
    fn the_field_reads_out_a_scale_that_is_still_a_preview() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, _) = app_with_rects(&ctx, 2, false);
        assert_eq!(
            app.live_scale_pct(),
            Some(100.0),
            "with nothing previewed the two boxes are the same box"
        );

        let tx = app.scale_tx(200.0).expect("a box to scale");
        app.session.set_preview(&tx);
        assert_eq!(
            app.live_scale_pct(),
            Some(200.0),
            "a doubling in flight should read 200%, and nothing is committed yet"
        );
        assert_eq!(
            authored(&app, _ids_first(&app)).width,
            100.0,
            "…which is the point: the document has not moved"
        );
    }

    /// The first selected rect, for the assertion above.
    fn _ids_first(app: &OndinApp) -> NodeId {
        app.session.selection.ids()[0]
    }

    /// A cancelled frame spends the edit; it does not defer it. `edit_valve` must update its
    /// engagement latch before returning on `gesture_cancelled`, or the next frame commits the
    /// edit the cancel threw away.
    ///
    /// flip: the latch moved back below the `gesture_cancelled` guard → the layer is scaled on the second pass after the cancel.
    #[test]
    fn a_cancelled_edit_does_not_commit_on_the_next_frame() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, ids) = app_with_rects(&ctx, 1, false);
        app.tool = crate::tools::Tool::Scale;
        let id = ids[0];

        let mut at = egui::Pos2::ZERO;
        let pass = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let mut got = None;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(280.0, 900.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| got = app.scale_row(ui, 120.0).map(|r| r.rect),
            );
            got
        };
        for _ in 0..2 {
            at = pass(&mut app, Vec::new()).expect("the row").center();
        }
        let click = |pressed: bool| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        pass(&mut app, vec![egui::Event::PointerMoved(at)]);
        pass(&mut app, vec![click(true)]);
        pass(&mut app, vec![click(false)]);
        for ch in "50".chars() {
            pass(&mut app, vec![egui::Event::Text(ch.to_string())]);
        }
        assert_eq!(
            authored(&app, id).width,
            100.0,
            "typing previews and commits nothing"
        );

        // The cancelled pass must be the pass that loses focus (as `Escape` does), or there
        // is no transition for the next frame to inherit.
        app.gesture_cancelled = true;
        pass(
            &mut app,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        app.gesture_cancelled = false;
        for _ in 0..3 {
            pass(&mut app, Vec::new());
        }

        assert_eq!(
            authored(&app, id).width,
            100.0,
            "the cancelled edit must not land a frame later"
        );
    }

    /// Escape abandons the edit rather than committing it. This passes only because of
    /// `edit_valve`'s own `Escape` guard: egui's `DragValue` updates while being edited, so the
    /// valve sees the typed number on that frame. The guard looks redundant and is load-bearing.
    #[test]
    fn escaping_the_field_abandons_the_edit() {
        assert_eq!(
            typed_into_the_field("50", egui::Key::Escape),
            100.0,
            "Escape should leave the rect at 100 — it was reported committing the \
             scale invisibly, which was the valve firing twice rather than Escape \
             meaning commit"
        );
    }

    /// A named row (`OndinApp::transform_row`) keeps its widgets' ids when a row above it is not
    /// drawn, unlike the plain `ui.horizontal` it replaced.
    #[test]
    fn a_conditional_row_does_not_renumber_the_rows_below_it() {
        fn second_row_id(ctx: &egui::Context, keyed: bool, first_row_drawn: bool) -> egui::Id {
            let mut got = None;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(280.0, 900.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let mut v = 0.0_f64;
                    if first_row_drawn {
                        if keyed {
                            OndinApp::transform_row(ui, "first", |ui| {
                                ui.label("x");
                            });
                        } else {
                            ui.horizontal(|ui| {
                                ui.label("x");
                            });
                        }
                    }
                    let resp = if keyed {
                        OndinApp::transform_row(ui, "second", |ui| {
                            ui.add(egui::DragValue::new(&mut v))
                        })
                    } else {
                        ui.horizontal(|ui| ui.add(egui::DragValue::new(&mut v)))
                            .inner
                    };
                    got = Some(resp.id);
                },
            );
            got.expect("the second row to hold a widget")
        }

        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        assert_eq!(
            second_row_id(&ctx, true, true),
            second_row_id(&ctx, true, false),
            "a row identified by name keeps its widgets' ids whether or not the \
             row above it was drawn"
        );
        assert_ne!(
            second_row_id(&ctx, false, true),
            second_row_id(&ctx, false, false),
            "and the positional version does not — this inequality is the bug, and \
             asserting it is what keeps the equality above meaningful"
        );
    }

    /// `0.5x` then `2x` puts a set back where it started (sizes, positions and stroke widths).
    /// It does not pin the anchor (any consistently held point round trips);
    /// `a_selection_scales_about_its_union_and_the_gap_grows_too` does. Two rects, as one
    /// layer's union is its box.
    #[test]
    fn halving_then_doubling_puts_the_selection_back() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, ids) = app_with_rects(&ctx, 2, true);
        let before: Vec<_> = ids
            .iter()
            .map(|id| {
                (
                    app.session.resolved.world_bounds(*id),
                    stroke_width(&app, *id),
                )
            })
            .collect();

        for pct in [50.0, 200.0] {
            let tx = app.scale_tx(pct).expect("a box to scale");
            app.session.try_commit(tx).expect("the scale to take");
        }

        let after: Vec<_> = ids
            .iter()
            .map(|id| {
                (
                    app.session.resolved.world_bounds(*id),
                    stroke_width(&app, *id),
                )
            })
            .collect();
        assert_eq!(
            after, before,
            "0.5x then 2x is a round trip: the held corner is the same corner both times"
        );
    }

    /// The Scale row is there only while the Scale tool is: a permanent percentage under W and H
    /// would read as an ordinary resize, and this one takes strokes, radii and type with it.
    ///
    /// flip: the tool test dropped → the row measures the same height under Select as under Scale.
    #[test]
    fn the_scale_row_is_offered_only_while_the_scale_tool_is_held() {
        fn height(tool: crate::tools::Tool) -> f32 {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            let (mut app, _) = app_with_rects(&ctx, 1, false);
            app.tool = tool;
            let mut h = 0.0;
            // Two passes, so ids and sizes are stored.
            for _ in 0..2 {
                let _ = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(280.0, 900.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        let before = ui.min_rect().height();
                        app.scale_row(ui, 120.0);
                        h = ui.min_rect().height() - before;
                    },
                );
            }
            h
        }
        assert_eq!(
            height(crate::tools::Tool::Select),
            0.0,
            "Select draws no scale row"
        );
        assert!(
            height(crate::tools::Tool::Scale) > 0.0,
            "and Scale draws one"
        );
    }

    /// A set scales about its union (the box X/Y/W/H move), so the gaps go with it. Two rects,
    /// as with one layer every anchor lands in the same place.
    #[test]
    fn a_selection_scales_about_its_union_and_the_gap_grows_too() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, ids) = app_with_rects(&ctx, 2, false);
        let left = app.session.resolved.world_bounds(ids[0]).expect("a box");
        let right = app.session.resolved.world_bounds(ids[1]).expect("a box");
        assert_eq!(
            (left.min_x(), right.min_x()),
            (0.0, 200.0),
            "the fixture should be two rects 200 apart"
        );

        let tx = app.scale_tx(200.0).expect("a box to scale");
        app.session.try_commit(tx).expect("the scale to take");

        assert_eq!(
            app.session
                .resolved
                .world_bounds(ids[0])
                .map(|b| (b.min_x(), b.width())),
            Some((0.0, 200.0)),
            "the held corner stays and the layer doubles"
        );
        assert_eq!(
            app.session.resolved.world_bounds(ids[1]).map(|b| b.min_x()),
            Some(400.0),
            "the second layer's distance from the anchor doubles as well — a set \
             scaling in place would leave it at 200"
        );
    }
}

#[cfg(test)]
mod several_layer_binding_tests {
    //! Bound fields written over several layers at once: the effect popover's numbers and a
    //! shared paint row.
    use super::*;
    use crate::app::OndinApp;
    use ondin_core::item::Keyed;
    use ondin_core::kurbo::Size;
    use ondin_core::variable::{Cell, Mode, VarKind, Variable};

    /// Two rects, each a drop shadow under its own item id, both shadows' X
    /// bound to `x/md` (6), both selected. Answers the rects and the variable.
    fn bound_pair(ctx: &egui::Context) -> (OndinApp, Vec<NodeId>, ondin_core::VarId) {
        crate::theme::install(ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(0xE7);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let (mode, var) = (ids.mint_mode(), ids.mint_var());
        doc.apply(&Transaction(vec![
            Operation::AddMode {
                mode: Mode {
                    id: mode,
                    name: "Default".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: var,
                    name: "x/md".into(),
                    kind: VarKind::Number,
                    cells: [(mode, Cell::Number(6.0))].into(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ]))
        .unwrap();
        let mut made = Vec::new();
        for i in 0..2 {
            let (id, item) = (ids.mint(), ids.mint_item());
            doc.apply(&Transaction(vec![
                Operation::CreateNode {
                    id,
                    parent: root,
                    index: i,
                    kind: NodeKind::Rect {
                        size: Size::new(40.0, 40.0),
                        corner_radii: Default::default(),
                    },
                    transform: None,
                    name: None,
                },
                Operation::SetEffects {
                    id,
                    effects: vec![Keyed::new(
                        item,
                        Effect::new(EffectKind::DropShadow(ondin_core::Shadow::default())),
                    )],
                },
            ]))
            .unwrap();
            let bind = ondin_core::bind_effect_ops(&doc, id, item, ondin_core::EFFECT_X, var);
            doc.apply(&Transaction(bind)).unwrap();
            made.push(id);
        }
        app.session.adopt_document(doc, None);
        app.session.selection.set(made.clone());
        (app, made, var)
    }

    /// One frame of the X field over `subjects`, focused or not, `typed` put in
    /// its edit buffer first. Answers the field's id.
    fn frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        subjects: &[NodeId],
        focus: Option<(egui::Id, bool)>,
        typed: Option<&str>,
    ) -> egui::Id {
        if let Some((id, on)) = focus {
            ctx.memory_mut(|m| {
                if on {
                    m.request_focus(id)
                } else {
                    m.surrender_focus(id)
                }
            });
            if let Some(t) = typed {
                ctx.data_mut(|d| d.insert_temp(id, t.to_string()));
            }
        }
        let mut out = egui::Id::NULL;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(320.0, 200.0),
                )),
                ..Default::default()
            },
            |ui| {
                out = app
                    .effect_number_field(
                        ui,
                        subjects,
                        0,
                        ondin_core::EFFECT_X,
                        "X",
                        egui::vec2(80.0, 24.0),
                        0.0,
                        -1e4..=1e4,
                    )
                    .expect("the plain field over several layers")
                    .id;
            },
        );
        out
    }

    /// A `%` change on a shared row of layers bound to one colour keeps every binding, a layer in
    /// another mode included.
    ///
    /// flip: the `Selection` arm handing every target the anchor's brush → fails "the Dark layer keeps its binding".
    #[test]
    fn a_percent_on_a_shared_bound_row_keeps_each_layers_binding() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0xE8);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let (light, dark, brand) = (ids.mint_mode(), ids.mint_mode(), ids.mint_var());
        let [fa, fb, a, b] = [(); 4].map(|_| ids.mint());
        let white = ondin_core::peniko::Color::WHITE;
        let black = ondin_core::peniko::Color::BLACK;
        let mode = |id, name: &str, index| Operation::AddMode {
            mode: Mode {
                id,
                name: name.into(),
            },
            index,
            cells: vec![],
        };
        let frame = |id, mode| {
            [
                Operation::CreateNode {
                    id,
                    parent: root,
                    index: 0,
                    kind: NodeKind::Artboard {
                        size: Size::new(50.0, 50.0),
                    },
                    transform: None,
                    name: None,
                },
                Operation::SetMode {
                    id,
                    mode: Some(mode),
                },
            ]
        };
        let rect = |id, parent| {
            [
                Operation::CreateNode {
                    id,
                    parent,
                    index: 0,
                    kind: NodeKind::Rect {
                        size: Size::new(10.0, 10.0),
                        corner_radii: Default::default(),
                    },
                    transform: None,
                    name: None,
                },
                Operation::SetFills {
                    id,
                    fills: ondin_core::keyed_by_position([ondin_core::Fill {
                        brush: ondin_core::BrushSource::Bound {
                            var: brand,
                            alpha: 1.0,
                        },
                        visible: true,
                    }]),
                },
            ]
        };
        let mut ops = vec![
            mode(light, "Light", 0),
            mode(dark, "Dark", 1),
            Operation::AddVariable {
                var: Variable {
                    id: brand,
                    name: "brand".into(),
                    kind: VarKind::Color,
                    cells: [(light, Cell::Color(white)), (dark, Cell::Color(black))].into(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ];
        ops.extend(frame(fa, light));
        ops.extend(frame(fb, dark));
        ops.extend(rect(a, fa));
        ops.extend(rect(b, fb));
        doc.apply(&Transaction(ops)).unwrap();
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![a, b]);
        let tx = app.slot_transaction(
            a,
            PaintSlot::Selection(PaintTarget::Fill, 0),
            Brush::Solid(white.with_alpha(0.5)),
        );
        assert!(app.session.commit(tx));
        let brush = |id: NodeId| {
            app.session.doc.get(id).unwrap().paint().fills[0]
                .brush
                .clone()
        };
        let bound = ondin_core::BrushSource::Bound {
            var: brand,
            alpha: 0.5,
        };
        assert_eq!(brush(a), bound, "the anchor keeps its binding");
        assert_eq!(brush(b), bound, "the Dark layer keeps its binding");
    }

    /// The several-layer card marks a copy whose opacity is bound apart from its main.
    #[test]
    fn the_several_layer_card_marks_an_opacity_bound_apart() {
        let ctx = egui::Context::default();
        let (mut app, ids, var) = bound_pair(&ctx);
        let (frame, other) = (app.session.ids.mint(), app.session.ids.mint_var());
        let mode = app.session.doc.variables().modes()[0].id;
        let root = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![
            Operation::AddVariable {
                var: Variable {
                    id: other,
                    name: "o/2".into(),
                    kind: VarKind::Number,
                    cells: [(mode, Cell::Number(40.0))].into(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 2,
                kind: NodeKind::Artboard {
                    size: Size::new(100.0, 100.0),
                },
                transform: None,
                name: None,
            },
            Operation::Reparent {
                id: ids[0],
                new_parent: frame,
                index: 0,
            },
            Operation::SetComponent {
                id: frame,
                component: true,
            },
        ])));
        let ops = ondin_core::bind_opacity_ops(&app.session.doc, ids[0], var);
        assert!(app.session.commit(Transaction(ops)));
        let place = ondin_core::Placement {
            nodes: app.session.doc.capture_subtree(frame).unwrap(),
            parent: root,
            index: None,
        };
        let (tx, made) = ondin_core::insert_subtrees(
            &app.session.doc,
            &mut app.session.ids,
            &[place],
            Default::default(),
        );
        assert!(app.session.commit(tx));
        let copy = app.session.doc.get(made[0]).unwrap().children()[0];
        let ops = ondin_core::bind_opacity_ops(&app.session.doc, copy, other);
        assert!(app.session.commit(Transaction(ops)));
        let (opacity, _) = app.multi_appearance_marks(&[copy, ids[1]], false);
        assert!(opacity.is_some(), "the copy's binding is marked");
    }

    fn x_var(app: &OndinApp, id: NodeId) -> Option<ondin_core::VarId> {
        app.session.doc.get(id).unwrap().effects()[0].vars[ondin_core::EFFECT_X]
    }

    /// A focus and a leave over several bound layers writes nothing, even under a frame in another mode.
    #[test]
    fn a_focus_and_a_leave_keeps_every_layers_binding() {
        let ctx = egui::Context::default();
        let (mut app, ids, var) = bound_pair(&ctx);
        let (dusk, frame_id) = (app.session.ids.mint_mode(), app.session.ids.mint());
        let root = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![
            Operation::AddMode {
                mode: Mode {
                    id: dusk,
                    name: "Dusk".into(),
                },
                index: 1,
                cells: vec![(var, Cell::Number(10.0))],
            },
            Operation::CreateNode {
                id: frame_id,
                parent: root,
                index: 2,
                kind: NodeKind::Artboard {
                    size: Size::new(100.0, 100.0),
                },
                transform: None,
                name: None,
            },
            Operation::SetMode {
                id: frame_id,
                mode: Some(dusk),
            },
            Operation::Reparent {
                id: ids[1],
                new_parent: frame_id,
                index: 0,
            },
        ])));
        app.session.selection.set(ids.clone());
        let depth0 = app.session.history.undo_depth();
        let id = frame(&ctx, &mut app, &ids, None, None);
        for _ in 0..2 {
            frame(&ctx, &mut app, &ids, Some((id, true)), None);
        }
        for _ in 0..2 {
            frame(&ctx, &mut app, &ids, Some((id, false)), None);
        }
        assert_eq!(app.session.history.undo_depth(), depth0, "no step spent");
        assert_eq!(
            (x_var(&app, ids[0]), x_var(&app, ids[1])),
            (Some(var), Some(var)),
            "both stay bound"
        );
    }

    /// A number written over several layers keeps each layer's own item id.
    #[test]
    fn a_number_written_over_several_layers_keeps_their_item_ids() {
        let ctx = egui::Context::default();
        let (mut app, ids, _) = bound_pair(&ctx);
        let items: Vec<ondin_core::ItemId> = ids
            .iter()
            .map(|id| app.session.doc.get(*id).unwrap().effects()[0].id)
            .collect();
        assert_ne!(items[0], items[1], "fixture: distinct item ids");
        let id = frame(&ctx, &mut app, &ids, None, None);
        for _ in 0..2 {
            frame(&ctx, &mut app, &ids, Some((id, true)), Some("9"));
        }
        for _ in 0..2 {
            frame(&ctx, &mut app, &ids, Some((id, false)), None);
        }
        let x = |n: NodeId| {
            app.session.doc.get(n).unwrap().effects()[0].numbers()[ondin_core::EFFECT_X]
        };
        assert_eq!(
            (x(ids[0]), x(ids[1])),
            (Some(9.0), Some(9.0)),
            "fixture: written"
        );
        assert_eq!(
            app.session.doc.get(ids[1]).unwrap().effects()[0].id,
            items[1],
            "the second layer keeps its own id"
        );
    }
}

#[cfg(test)]
mod align_card_tests {
    //! The Align card is drawn for every selection and dims what does not apply.
    use super::*;
    use crate::app::OndinApp;
    use ondin_core::kurbo::{Affine, Size};

    const AREA: egui::Rect = egui::Rect {
        min: egui::pos2(0.0, 0.0),
        max: egui::pos2(280.0, 900.0),
    };

    /// Two rects side by side (x 0 and x 200), top-level or in a group, the right-hand one selected.
    ///
    /// The second rect gives the nested case somewhere to move to: a group of one child aligns to
    /// that child's own bounds and could not tell an inert card from a live one.
    fn app_with_pair(ctx: &egui::Context, in_group: bool) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let parent = if in_group {
            let g = ids.mint();
            doc.apply(&Transaction(vec![Operation::CreateNode {
                id: g,
                parent: root,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            }]))
            .expect("a group");
            g
        } else {
            root
        };
        let mut rect = |x: f64| {
            let id = ids.mint();
            doc.apply(&Transaction(vec![Operation::CreateNode {
                id,
                parent,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 100.0),
                    corner_radii: Default::default(),
                },
                transform: Some(Affine::translate((x, 0.0))),
                name: None,
            }]))
            .expect("a rect");
            id
        };
        let _left = rect(0.0);
        let right = rect(200.0);
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(right);
        (app, right)
    }

    /// The height the Align card occupies, drawn on the real panel.
    fn card_height(in_group: bool) -> f32 {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, _) = app_with_pair(&ctx, in_group);
        let mut height = 0.0;
        // Two passes: ids and sizes settle on the first.
        for _ in 0..2 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(AREA),
                    ..Default::default()
                },
                |ui| {
                    let before = ui.min_rect().height();
                    app.inspector_align(ui);
                    height = ui.min_rect().height() - before;
                },
            );
        }
        height
    }

    /// A lone top-level layer gets the same (dimmed) card height as a nested one, so the panel does not jump.
    #[test]
    fn a_lone_top_level_layer_gets_the_same_card_as_a_nested_one() {
        let (top, nested) = (card_height(false), card_height(true));
        assert!(
            top > 0.0 && nested > 0.0,
            "the Align card should be drawn for both: {top} at the top level, \
             {nested} inside a group"
        );
        assert_eq!(
            top, nested,
            "the same card, dimmed or live: {top} at the top level against \
             {nested} inside a group"
        );
    }

    /// A frame picked with its own children draws the dimmed card too: `inspector` routes on the
    /// raw selection length (three) while `align_target` measures after `build::outermost` (one).
    #[test]
    fn a_frame_selected_with_its_children_dims_the_card_too() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, right) = app_with_pair(&ctx, true);
        let group = app
            .session
            .doc
            .get(right)
            .and_then(|n| n.parent())
            .expect("the pair's group");
        let kids = app
            .session
            .doc
            .get(group)
            .map(|n| n.children().to_vec())
            .expect("two children");
        assert_eq!(kids.len(), 2, "the fixture should hold two rects");
        let mut ids = vec![group];
        ids.extend(kids);
        app.session.selection.set(ids);
        assert_eq!(
            app.session.selection.len(),
            3,
            "the panel routes on this count, and it has to be the multi one"
        );

        let mut height = 0.0;
        for _ in 0..2 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(AREA),
                    ..Default::default()
                },
                |ui| {
                    let before = ui.min_rect().height();
                    app.inspector_align(ui);
                    height = ui.min_rect().height() - before;
                },
            );
        }
        assert!(
            height > 0.0,
            "the Align card should be drawn for a frame and its children too"
        );
    }

    /// `OndinApp::align` refuses with no target, so the dimmed card is inert; the nested half is
    /// the control that proves align works at all. It pins the guard in `align`, not the card.
    #[test]
    fn align_moves_a_nested_layer_and_leaves_a_top_level_one_alone() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);

        let (mut app, id) = app_with_pair(&ctx, false);
        let before = app.session.resolved.world_bounds(id);
        app.align(Axis::X, Edge::Min);
        assert_eq!(
            app.session.resolved.world_bounds(id),
            before,
            "a top-level layer has no target, so Align left must do nothing"
        );

        let (mut app, id) = app_with_pair(&ctx, true);
        app.align(Axis::X, Edge::Min);
        assert_eq!(
            app.session.resolved.world_bounds(id).map(|b| b.min_x()),
            Some(0.0),
            "inside a group the same press should land it on the group's left edge"
        );
    }
}

#[cfg(test)]
mod stroke_card_geometry_tests {
    //! The Stroke card's own height, measured on the real card.
    use super::*;
    use crate::app::OndinApp;

    const AREA: egui::Rect = egui::Rect {
        min: egui::pos2(0.0, 0.0),
        max: egui::pos2(280.0, 900.0),
    };

    /// A headless app with one rect carrying `n` strokes, selected.
    fn app_with_strokes(ctx: &egui::Context, n: usize) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let id = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: ondin_core::kurbo::Size::new(100.0, 100.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        }]))
        .expect("a rect");
        app.session.adopt_document(doc, None);
        if n > 0 {
            app.session
                .try_commit(Transaction(vec![Operation::SetStrokes {
                    id,
                    strokes: ondin_core::keyed_by_position((0..n).map(|i| Stroke {
                        width: 1.0 + i as f64,
                        ..Default::default()
                    })),
                }]))
                .expect("strokes to draw");
        }
        app.session.selection.set_one(id);
        (app, id)
    }

    /// The height the Stroke card occupies for a layer with `n` strokes.
    fn card_height(n: usize) -> f32 {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, id) = app_with_strokes(&ctx, n);
        let mut height = 0.0;
        // Two passes: ids and sizes settle on the first.
        for _ in 0..2 {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(AREA),
                    ..Default::default()
                },
                |ui| {
                    let scope = PaintScope::Node(id);
                    let shown = app.strokes_shown(scope);
                    let before = ui.min_rect().height();
                    app.inspector_stroke(ui, scope, &shown, false);
                    height = ui.min_rect().height() - before;
                },
            );
            let _ = out;
        }
        height
    }

    /// The card with nothing in its body: the header, the frame and the padding —
    /// everything whose height is not the stroke list.
    fn chrome_height() -> f32 {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, _) = app_with_strokes(&ctx, 0);
        let mut height = 0.0;
        for _ in 0..2 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(AREA),
                    ..Default::default()
                },
                |ui| {
                    let before = ui.min_rect().height();
                    app.panel(ui, "Stroke", None, |_, _| {});
                    height = ui.min_rect().height() - before;
                },
            );
        }
        height
    }

    /// The last stroke in the card gets no trailing gap: `inspector_stroke` passes `paint_unit`'s
    /// `last` flag correctly.
    ///
    /// A wrong flag adds one gap at every `n`, so differences between card heights agree in both
    /// worlds and only the absolute height past `chrome_height` tells them apart.
    #[test]
    fn the_last_stroke_in_the_card_gets_no_trailing_gap() {
        let (chrome, h1, h2, h3) = (
            chrome_height(),
            card_height(1),
            card_height(2),
            card_height(3),
        );

        assert_eq!(
            h3 - h2,
            h2 - h1,
            "every stroke past the first has to cost the same: {h1}, {h2}, {h3}"
        );

        // One unit is what a card grows by, less the gap that joins it to the one above.
        let unit = (h2 - h1) - ui::CARD_ROW_GAP;
        assert_eq!(
            h1 - chrome,
            unit,
            "a card holding one stroke must be exactly one unit tall past its \
             chrome ({chrome}); {} means the last unit is still allocating its \
             trailing gap",
            h1 - chrome
        );
    }

    /// A stroke unit's two rows end on the same x, a relation between rows rather than a number.
    #[test]
    fn a_stroke_units_two_rows_end_on_the_same_x() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, id) = app_with_strokes(&ctx, 1);
        let mut rights: Vec<f32> = Vec::new();
        for _ in 0..2 {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(AREA),
                    ..Default::default()
                },
                |ui| {
                    let scope = PaintScope::Node(id);
                    let shown = app.strokes_shown(scope);
                    app.inspector_stroke(ui, scope, &shown, false);
                },
            );
            // Every stroked control box in the card, grouped into rows by centre y,
            // keeping each row's **rightmost** edge: the last control says where the
            // column ends.
            let mut boxes: Vec<(f32, f32)> = Vec::new();
            fn walk(s: &egui::Shape, v: &mut Vec<(f32, f32)>) {
                match s {
                    egui::Shape::Rect(r)
                        if r.stroke.width > 0.0
                            && r.rect.height() >= 18.0
                            && r.rect.height() <= 34.0
                            && r.rect.width() >= 14.0 =>
                    {
                        v.push((r.rect.center().y, r.rect.right()))
                    }
                    egui::Shape::Vec(x) => x.iter().for_each(|s| walk(s, v)),
                    _ => {}
                }
            }
            for cs in &out.shapes {
                walk(&cs.shape, &mut boxes);
            }
            boxes.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mut rows: Vec<(f32, f32)> = Vec::new();
            for (y, right) in boxes {
                match rows.last_mut() {
                    Some(row) if (row.0 - y).abs() < 4.0 => row.1 = row.1.max(right),
                    _ => rows.push((y, right)),
                }
            }
            rights = rows.into_iter().map(|(_, r)| r).collect();
        }
        assert!(
            rights.len() >= 2,
            "the fixture: a stroke unit draws more than one row of controls, got \
             {rights:?}"
        );
        let mut edges = rights.clone();
        edges.sort_by(|a, b| a.partial_cmp(b).unwrap());
        edges.dedup();
        assert_eq!(
            edges.len(),
            1,
            "every row of one stroke unit ends on the same x; {rights:?} means a \
             row is computing its own gap"
        );
    }
}

#[cfg(test)]
mod fill_row_door_tests {
    //! Clicking a fill row's swatch or its *Image* label steps into image editing;
    //! the predicate under test decides which row does.
    use super::row_edits_the_mode_s_picture;
    use crate::panels::paint::PaintSlot;
    use ondin_core::kurbo::Size;
    use ondin_core::peniko::Color;
    use ondin_core::{
        Brush, Document, Fill, IdSource, NodeId, NodeKind, Operation, Transaction, image_brush,
    };

    /// A rect whose fills are the given brushes, in order.
    fn layer_with(fills: Vec<Fill>) -> (Document, NodeId) {
        let mut ids = IdSource::new(0xF1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let rect = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: rect,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(80.0, 60.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        }]))
        .expect("build");
        doc.apply(&Transaction(vec![Operation::SetFills {
            id: rect,
            fills: ondin_core::keyed_by_position(fills),
        }]))
        .expect("paint");
        (doc, rect)
    }

    fn picture(visible: bool) -> Fill {
        Fill {
            brush: image_brush(ondin_core::ImageId("a-photo".into())).into(),
            visible,
        }
    }

    fn colour() -> Fill {
        Fill {
            brush: Brush::Solid(Color::from_rgba8(1, 2, 3, 255)).into(),
            visible: true,
        }
    }

    /// The ordinary layer: one picture, and its row is the door.
    #[test]
    fn the_only_picture_is_the_one_the_mode_takes() {
        let (doc, id) = layer_with(vec![picture(true)]);
        assert!(row_edits_the_mode_s_picture(&doc, id, PaintSlot::Fill(0)));
    }

    /// `croppable_fill` answers the first visible image fill, so with two pictures the second row
    /// must be inert rather than put the handles on the other photograph.
    #[test]
    fn a_second_picture_on_the_same_layer_is_not_a_door() {
        let (doc, id) = layer_with(vec![picture(true), picture(true)]);
        assert!(row_edits_the_mode_s_picture(&doc, id, PaintSlot::Fill(0)));
        assert!(
            !row_edits_the_mode_s_picture(&doc, id, PaintSlot::Fill(1)),
            "the mode takes the first visible picture, so the second row opens nothing"
        );
    }

    /// And *visible* is the word: a hidden first picture is not the one the mode
    /// takes, so the door moves down with it rather than staying put.
    #[test]
    fn a_hidden_picture_hands_the_door_to_the_next_one() {
        let (doc, id) = layer_with(vec![picture(false), picture(true)]);
        assert!(!row_edits_the_mode_s_picture(&doc, id, PaintSlot::Fill(0)));
        assert!(row_edits_the_mode_s_picture(&doc, id, PaintSlot::Fill(1)));
    }

    /// A colour row is not a door, and neither is a stroke: `Stroke(0)` is refused by the slot
    /// pattern before the predicate is asked.
    #[test]
    fn a_colour_and_a_stroke_are_not_doors() {
        let (doc, id) = layer_with(vec![colour(), picture(true)]);
        assert!(!row_edits_the_mode_s_picture(&doc, id, PaintSlot::Fill(0)));
        assert!(row_edits_the_mode_s_picture(&doc, id, PaintSlot::Fill(1)));
        assert!(!row_edits_the_mode_s_picture(
            &doc,
            id,
            PaintSlot::Stroke(0)
        ));
    }
}

/// The Effects card, driven headlessly with real pointer events: what is pinned is where a
/// press lands, which the row's returned value does not show.
#[cfg(test)]
mod effects_panel_tests {
    use super::*;
    use crate::app::OndinApp;

    /// A tall, wide screen: the popover hangs left of the inspector column, and a narrow one
    /// would clamp it onto the card.
    const SCREEN: egui::Rect = egui::Rect {
        min: egui::pos2(0.0, 0.0),
        max: egui::pos2(1200.0, 900.0),
    };
    /// The width the card is laid out into — the real inspector column's.
    const COLUMN: f32 = 280.0;

    struct Card {
        ctx: egui::Context,
        app: OndinApp,
        /// The layer the card is drawn for.
        id: NodeId,
        /// Its neighbour, which carries no effects.
        plain: NodeId,
        /// Which of the two the card is drawn for; kept apart from the selection so the harness
        /// never draws one layer while another is selected.
        subject: NodeId,
        time: f64,
    }

    impl Card {
        fn new(kinds: Vec<EffectKind>) -> Self {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            let mut app = OndinApp::headless(&ctx);
            let mut ids = ondin_core::IdSource::new(1);
            let root = ids.mint();
            let mut doc = ondin_core::Document::new(root);
            let mut rect = |doc: &mut ondin_core::Document, index: usize| {
                let id = ids.mint();
                doc.apply(&Transaction(vec![Operation::CreateNode {
                    id,
                    parent: root,
                    index,
                    kind: NodeKind::Rect {
                        size: ondin_core::kurbo::Size::new(100.0, 100.0),
                        corner_radii: Default::default(),
                    },
                    transform: None,
                    name: None,
                }]))
                .expect("a rect");
                id
            };
            let id = rect(&mut doc, 0);
            let plain = rect(&mut doc, 1);
            app.session.adopt_document(doc, None);
            if !kinds.is_empty() {
                app.session
                    .try_commit(Transaction(vec![Operation::SetEffects {
                        id,
                        effects: ondin_core::keyed_by_position(kinds.into_iter().map(Effect::new)),
                    }]))
                    .expect("effects to draw");
            }
            app.session.selection.set_one(id);
            let mut card = Self {
                ctx,
                app,
                id,
                plain,
                subject: id,
                time: 0.0,
            };
            // Settled first: ids and sizes arrive on the first pass and interaction state is last frame's.
            card.settle();
            card
        }

        /// Select a layer and draw the card for it.
        fn select(&mut self, id: NodeId) {
            self.app.session.selection.set_one(id);
            self.subject = id;
            self.settle();
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
            self.time += 0.1;
            let (app, id) = (&mut self.app, self.subject);
            self.ctx.run_ui(
                egui::RawInput {
                    time: Some(self.time),
                    screen_rect: Some(SCREEN),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.set_max_width(COLUMN);
                    app.inspector_effects(ui, &[id]);
                },
            )
        }

        fn settle(&mut self) -> egui::FullOutput {
            let mut out = self.frame(Vec::new());
            for _ in 0..2 {
                out = self.frame(Vec::new());
            }
            out
        }

        /// Press and release at `at`, then let the frames after it settle.
        fn click(&mut self, at: egui::Pos2) {
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            self.frame(vec![egui::Event::PointerMoved(at), button(at, true)]);
            self.frame(vec![button(at, false)]);
            self.settle();
        }

        /// The recessed grounds of the effect rows, top-first (the only 28-point rects that wide).
        fn row_fields(&mut self) -> Vec<egui::Rect> {
            let out = self.settle();
            let mut rects: Vec<egui::Rect> = out
                .shapes
                .iter()
                .filter_map(|cs| match &cs.shape {
                    egui::epaint::Shape::Rect(r) => Some(r.rect),
                    _ => None,
                })
                .filter(|r| (r.height() - 28.0).abs() < 0.5 && r.width() > 100.0)
                .collect();
            rects.sort_by(|a, b| a.top().total_cmp(&b.top()));
            rects.dedup_by(|a, b| a.top() == b.top());
            rects
        }

        /// The stack as the document holds it, not as the panel previews it.
        fn committed(&self) -> Vec<Effect> {
            ondin_core::item::values(
                self.app
                    .session
                    .doc
                    .get(self.id)
                    .expect("the layer")
                    .effects(),
            )
        }
    }

    /// Every text run the card paints, as `(text, x0, x1)`.
    ///
    /// `TextShape::pos` is not the galley's left edge: a right-aligned galley's `rect` runs from
    /// `−width` to `0`, so the ink is `pos + galley.rect`.
    fn text_runs(out: &egui::FullOutput) -> Vec<(String, f32, f32)> {
        ink(out)
            .into_iter()
            .map(|(t, r)| (t, r.min.x, r.max.x))
            .collect()
    }

    /// Every text run and the box its ink occupies (`text_runs` with the y kept).
    fn ink(out: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
        out.shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Text(t) => Some((
                    t.galley.text().to_owned(),
                    t.galley.rect.translate(t.pos.to_vec2()),
                )),
                _ => None,
            })
            .collect()
    }

    /// Where the one run reading exactly `text` is, or a panic naming what the frame did paint.
    fn word(out: &egui::FullOutput, text: &str) -> egui::Rect {
        let runs = ink(out);
        match runs.iter().find(|(t, _)| t == text) {
            Some((_, r)) => *r,
            None => panic!(
                "no run reads {text:?}; the frame painted {:?}",
                runs.iter().map(|(t, _)| t).collect::<Vec<_>>()
            ),
        }
    }

    /// The centre of a row's `×`: `ui::FIELD_PAD_X` in from the field's right edge and 18 wide.
    fn close_button(field: egui::Rect) -> egui::Pos2 {
        egui::pos2(field.right() - 18.0, field.center().y)
    }

    /// A filter row's meter grows from the middle of the field at one scale on both sides
    /// (the two lengths are in the ratio of the two offsets), and rests as a one-point tick.
    #[test]
    fn a_filter_rows_meter_grows_from_the_middle_at_one_scale() {
        // The bar is the widest translucent rect the height of a field's content; the height
        // separates it from the neutral tick, which is translucent too.
        let content = ui::CONTROL_H - ui::FIELD_BORDER_H;
        let bar = |brightness: f64| -> egui::Rect {
            let mut card = Card::new(vec![EffectKind::Filters(ondin_core::Filters {
                brightness,
                ..Default::default()
            })]);
            card.app.effect_menu = Some((card.id, 0));
            card.settle()
                .shapes
                .iter()
                .filter_map(|cs| match &cs.shape {
                    egui::epaint::Shape::Rect(r) => Some((r.rect, r.fill)),
                    _ => None,
                })
                .filter(|(rect, fill)| {
                    fill.a() > 0
                        && fill.a() < 255
                        && (content - 4.0..=content).contains(&rect.height())
                })
                .map(|(rect, _)| rect)
                .max_by(|a, b| a.width().total_cmp(&b.width()))
                .expect("the brightness row draws a meter")
        };
        let rest = bar(1.0);
        assert!(
            rest.width() <= 1.5,
            "at rest the meter is a tick at the neutral, not a bar: {rest:?}"
        );
        let middle = rest.center().x;

        let dark = bar(0.4);
        let bright = bar(1.7);
        assert!(
            (dark.max.x - middle).abs() < 1.0 && dark.min.x < middle - 1.0,
            "below neutral it fills left from the middle: {dark:?} against {middle}"
        );
        assert!(
            (bright.min.x - middle).abs() < 1.0 && bright.max.x > middle + 1.0,
            "above neutral it fills right from the middle: {bright:?} against {middle}"
        );

        let ratio = f64::from(dark.width()) / f64::from(bright.width());
        assert!(
            (ratio - 6.0 / 7.0).abs() < 0.05,
            "the two sides are one scale: {:.1} against {:.1} is a ratio of {ratio:.3},              where 0.857 is the two offsets'",
            dark.width(),
            bright.width()
        );
    }

    /// A shadow's chip opens the detached picker on its own slot and the picker writes back to it:
    /// `PaintSlot::Effect(i)` must reach both `slot_brush` and `slot_transaction`.
    #[test]
    fn a_shadows_chip_opens_the_picker_on_its_own_slot_and_the_picker_writes_there() {
        let mut card = Card::new(vec![EffectKind::DropShadow(Default::default())]);
        card.app.effect_menu = Some((card.id, 0));
        let out = card.settle();
        let chip = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Rect(r) => Some(r.rect),
                _ => None,
            })
            .find(|r| (r.width() - 14.0).abs() < 0.5 && (r.height() - 14.0).abs() < 0.5)
            .expect("the colour row draws a chip");
        card.click(chip.center());
        let picker = card.app.picker.as_ref().expect("the chip opens the picker");
        assert_eq!(picker.node, card.id);
        assert_eq!(picker.slot, PaintSlot::Effect(0));

        let now = card.app.slot_brush(card.id, PaintSlot::Effect(0));
        assert_eq!(
            now,
            Some(Brush::Solid(ondin_core::Shadow::default().color)),
            "the picker is pointed at the shadow's own colour"
        );
        let red = ondin_core::peniko::Color::from_rgba8(255, 0, 0, 128);
        card.app
            .write_slot(card.id, PaintSlot::Effect(0), Brush::Solid(red));
        let EffectKind::DropShadow(s) = &card.committed()[0].kind else {
            panic!("the kind changed");
        };
        assert_eq!(s.color, red, "and the write landed on it, alpha and all");
        assert_eq!(
            s.blur,
            ondin_core::Shadow::default().blur,
            "leaving every other number alone — the op writes the whole stack"
        );
    }

    /// Each kind's popover holds that kind's numbers and no others: a field a kind does not have
    /// is absent rather than dimmed. Field prefixes are checked, not the title.
    #[test]
    fn each_kinds_popover_holds_that_kinds_numbers() {
        let want: [(EffectKind, &[&str]); 4] = [
            (
                EffectKind::DropShadow(Default::default()),
                &["X", "Y", "Blur", "Spread"],
            ),
            (
                EffectKind::InnerShadow(Default::default()),
                &["X", "Y", "Blur", "Spread"],
            ),
            (EffectKind::LayerBlur { radius: 6.0 }, &["Radius"]),
            (
                EffectKind::Filters(Default::default()),
                &["Brightness", "Contrast", "Saturation", "Hue"],
            ),
        ];
        // Every prefix any kind uses, so "absent" can be asserted.
        let all: Vec<&str> = want
            .iter()
            .flat_map(|(_, fields)| fields.iter().copied())
            .collect();
        for (kind, fields) in want {
            let mut card = Card::new(vec![kind.clone()]);
            card.app.effect_menu = Some((card.id, 0));
            let out = card.settle();
            let painted: Vec<String> = ink(&out).into_iter().map(|(t, _)| t).collect();
            for name in &all {
                let here = painted.iter().any(|t| t == name);
                assert_eq!(
                    here,
                    fields.contains(name),
                    "{}'s popover and the field {name:?}: painted {painted:?}",
                    kind.label()
                );
            }
            let hex = painted.iter().any(|t| t == "000000");
            assert_eq!(
                hex,
                matches!(kind, EffectKind::DropShadow(_) | EffectKind::InnerShadow(_)),
                "{}'s popover and its colour row",
                kind.label()
            );
        }
    }

    /// The kind combo retypes a row and carries across what the two kinds share. The rule lives
    /// in `ondin_core::EffectKind::retyped`; this pins the wiring and that the popover survives
    /// the click (the `over_overlay` exemption in `dismissed_by_click`).
    #[test]
    fn the_kind_combo_retypes_a_row_and_keeps_the_shared_numbers() {
        let tuned = ondin_core::Shadow {
            offset: ondin_core::kurbo::Vec2::new(9.0, -2.0),
            blur: 21.0,
            spread: 4.0,
            color: ondin_core::peniko::Color::from_rgba8(10, 20, 30, 200),
        };
        let mut card = Card::new(vec![EffectKind::DropShadow(tuned.clone())]);
        card.app.effect_menu = Some((card.id, 0));
        let out = card.settle();
        let closed = format!("{}Drop shadow", icon::DROP_HALF_BOTTOM);
        card.click(word(&out, &closed).center());
        // The list is its own `Area`, drawn from the frame after the press.
        let out = card.settle();
        let row = format!("{}Inner shadow", icon::DROP_HALF);
        card.click(word(&out, &row).center());

        let now = card.committed();
        assert_eq!(now.len(), 1, "retyping is not adding");
        let EffectKind::InnerShadow(kept) = &now[0].kind else {
            panic!("the row is {:?}, not an inner shadow", now[0].kind);
        };
        assert_eq!(
            *kept, tuned,
            "the two shadows share every field, so nothing is lost"
        );
        assert!(
            card.app.effect_menu.is_some(),
            "and the popover is still open — you retype in order to look at it"
        );
    }

    /// A scrub in the popover previews every frame and commits once: the offset accumulates
    /// (`inspector_effects` reads the stack through `display_node`, so the field is re-seeded from
    /// the preview) and one drag is one undo step.
    #[test]
    fn scrubbing_a_shadows_offset_accumulates_and_lands_as_one_undo_step() {
        let mut card = Card::new(vec![EffectKind::DropShadow(Default::default())]);
        card.app.effect_menu = Some((card.id, 0));
        let out = card.settle();
        // The X field: the topmost, leftmost 26pt ground narrower than the popover (the combo above is 26 too).
        let mut cells: Vec<egui::Rect> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Rect(r) => Some(r.rect),
                _ => None,
            })
            .filter(|r| (r.height() - 26.0).abs() < 0.5 && r.width() < 130.0)
            .collect();
        cells.sort_by(|a, b| {
            a.top()
                .total_cmp(&b.top())
                .then(a.left().total_cmp(&b.left()))
        });
        let x_field = *cells.first().expect("the popover draws an X field");
        assert_eq!(
            card.committed()[0].kind,
            EffectKind::DropShadow(Default::default()),
            "the fixture starts on the default shadow"
        );

        let depth = card.app.session.history.undo_depth();
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let from = x_field.center();
        card.frame(vec![egui::Event::PointerMoved(from), button(from, true)]);
        for step in 1..=4 {
            let at = from + egui::vec2(20.0 * step as f32, 0.0);
            card.frame(vec![egui::Event::PointerMoved(at)]);
        }
        let held = card.committed();
        assert_eq!(
            held[0].kind,
            EffectKind::DropShadow(Default::default()),
            "nothing is committed while the pointer is down"
        );
        let at = from + egui::vec2(80.0, 0.0);
        card.frame(vec![button(at, false)]);
        card.settle();

        let EffectKind::DropShadow(now) = &card.committed()[0].kind else {
            panic!("the kind changed under a scrub");
        };
        // A threshold between the right answer (20) and the committed-reseed answer (0), so a
        // retuned `SCRUB_SCALE` does not break it.
        let moved = now.offset.x;
        assert!(
            moved > 10.0,
            "the drag moved the offset by {moved}, where the whole 80 points is 20"
        );
        assert_eq!(
            now.offset.y, 4.0,
            "and only the field that was dragged moved"
        );
        assert_eq!(
            card.app.session.history.undo_depth(),
            depth + 1,
            "one drag, one undo step"
        );
        card.app.session.undo();
        assert_eq!(
            card.committed()[0].kind,
            EffectKind::DropShadow(Default::default()),
            "and undoing it once puts the whole drag back"
        );
    }

    /// The `×` removes its own effect and does not open the popover: the row's click target is
    /// registered after the button over the same field, so it shrinks its rect by the button's corner.
    #[test]
    fn a_rows_close_button_removes_that_effect_and_leaves_the_popover_shut() {
        let mut card = Card::new(vec![
            EffectKind::DropShadow(Default::default()),
            EffectKind::LayerBlur { radius: 6.0 },
        ]);
        let fields = card.row_fields();
        assert_eq!(fields.len(), 2, "two effects, two rows");
        card.click(close_button(fields[0]));
        let left = card.committed();
        assert_eq!(left.len(), 1, "the pressed row went");
        assert!(
            matches!(left[0].kind, EffectKind::LayerBlur { .. }),
            "and it was the drop shadow, not its neighbour"
        );
        assert_eq!(card.app.effect_menu, None, "no popover was opened by it");
    }

    /// Pressing a row shows its popover; pressing it again puts it away. The dismissal rule
    /// exempts a press on the head, so the toggle is the panel's.
    #[test]
    fn pressing_a_row_shows_its_popover_and_pressing_it_again_puts_it_away() {
        let mut card = Card::new(vec![
            EffectKind::DropShadow(Default::default()),
            EffectKind::LayerBlur { radius: 6.0 },
        ]);
        let fields = card.row_fields();
        let body = egui::pos2(fields[1].left() + 40.0, fields[1].center().y);
        card.click(body);
        assert_eq!(
            card.app.effect_menu,
            Some((card.id, 1)),
            "the second row's popover, not the first's"
        );
        card.click(body);
        assert_eq!(card.app.effect_menu, None, "the same press puts it away");
    }

    /// A removed row does not hand the open colour picker the effect that slid into its index:
    /// `PaintSlot::Effect` is an index and `SetEffects` renumbers. The picker is tested rather
    /// than the popover because `dismissed_by_click` already closes the popover.
    #[test]
    fn removing_a_row_moves_the_picker_off_the_effect_that_went() {
        let stack = || {
            vec![
                EffectKind::DropShadow(Default::default()),
                EffectKind::InnerShadow(Default::default()),
            ]
        };
        let mut card = Card::new(stack());
        let fields = card.row_fields();
        card.app.picker = Some(super::super::Picker::new(
            card.id,
            PaintSlot::Effect(1),
            egui::pos2(400.0, 200.0),
        ));
        card.click(close_button(fields[0]));
        let picker = card.app.picker.as_ref().expect("the picker is still open");
        assert_eq!(
            picker.slot,
            PaintSlot::Effect(0),
            "the second shadow is index 0 now, and the picker followed it"
        );

        let mut card = Card::new(stack());
        let fields = card.row_fields();
        card.app.picker = Some(super::super::Picker::new(
            card.id,
            PaintSlot::Effect(1),
            egui::pos2(400.0, 200.0),
        ));
        card.click(close_button(fields[1]));
        assert!(card.app.picker.is_none(), "its own row went");
    }

    /// The card opens on a layer with effects and shuts on one without, in both directions
    /// (`sync_paint_collapse`'s transition rule only ever closes).
    #[test]
    fn the_card_follows_the_subject_open_and_shut() {
        let mut card = Card::new(vec![EffectKind::DropShadow(Default::default())]);
        assert!(
            !card.app.collapsed_panels.contains("Effects"),
            "a layer with a shadow shows it"
        );
        let plain = card.plain;
        card.select(plain);
        assert!(
            card.app.collapsed_panels.contains("Effects"),
            "a layer with none does not"
        );
        let id = card.id;
        card.select(id);
        assert!(
            !card.app.collapsed_panels.contains("Effects"),
            "and back again — the transition rule alone leaves this one shut"
        );
    }

    /// A popover belonging to another layer is dropped, so walking away and back does not reopen it.
    #[test]
    fn changing_the_subject_puts_an_open_popover_away() {
        let mut card = Card::new(vec![EffectKind::DropShadow(Default::default())]);
        card.app.effect_menu = Some((card.id, 0));
        card.settle();
        // Drawn for the other layer, as the inspector does the frame after the selection moves.
        let plain = card.plain;
        card.select(plain);
        assert_eq!(card.app.effect_menu, None);
    }

    /// A row's headline number stops short of its `×`; the number is the one that grows (`100%`
    /// against a blur's `6`). Ink is read as `text_runs` describes.
    #[test]
    fn a_rows_number_stops_short_of_its_close_button() {
        let mut card = Card::new(vec![EffectKind::DropShadow(ondin_core::Shadow {
            // The widest a shadow's summary gets: opaque rather than the default's 40%.
            color: ondin_core::peniko::Color::from_rgba8(0, 0, 0, 255),
            ..Default::default()
        })]);
        let out = card.settle();
        let runs = text_runs(&out);
        let number = runs
            .iter()
            .find(|(t, ..)| t == "100%")
            .expect("the row states its opacity");
        let cross = runs
            .iter()
            .find(|(t, ..)| t == icon::X)
            .expect("the row offers a remove");
        assert!(
            number.2 <= cross.1,
            "the summary ({}..{}) runs into the × ({}..{})",
            number.1,
            number.2,
            cross.1,
            cross.2
        );
    }

    /// The header's `+` adds a drop shadow, opens the card, and brings up the new row's numbers.
    #[test]
    fn the_header_plus_adds_a_shadow_and_brings_up_its_numbers() {
        let mut card = Card::new(Vec::new());
        assert!(
            card.app.collapsed_panels.contains("Effects"),
            "an empty stack rests shut"
        );
        let x = COLUMN - ui::CARD_MARGIN_X - ui::HEAD_MORE_W - ui::HEAD_ACTION_W * 0.5;
        card.click(egui::pos2(x, 19.0));
        let now = card.committed();
        assert_eq!(now.len(), 1);
        assert!(matches!(now[0].kind, EffectKind::DropShadow(_)));
        assert!(now[0].visible);
        assert!(!card.app.collapsed_panels.contains("Effects"));
        assert_eq!(card.app.effect_menu, Some((card.id, 0)));
    }

    /// The eye hides a row without touching its numbers, which delete-to-hide would throw away.
    #[test]
    fn the_eye_hides_an_effect_and_keeps_it() {
        let tuned = ondin_core::Shadow {
            offset: ondin_core::kurbo::Vec2::new(9.0, -2.0),
            spread: 4.0,
            ..Default::default()
        };
        let mut card = Card::new(vec![EffectKind::DropShadow(tuned.clone())]);
        let fields = card.row_fields();
        card.click(egui::pos2(
            fields[0].right() + 6.0 + 14.0,
            fields[0].center().y,
        ));
        let now = card.committed();
        assert_eq!(now.len(), 1, "hiding is not removing");
        assert!(!now[0].visible);
        let EffectKind::DropShadow(kept) = &now[0].kind else {
            panic!("the kind changed");
        };
        assert_eq!(*kept, tuned, "and every number is still there");
    }
}

/// The Effects card over a multi-selection: none has effects (adding applies to all), all share
/// one stack (edits reach all), or they disagree (the card says so and the `+` replaces every stack).
#[cfg(test)]
mod effects_multi_tests {
    use super::*;
    use crate::app::OndinApp;

    const SCREEN: egui::Rect = egui::Rect {
        min: egui::pos2(0.0, 0.0),
        max: egui::pos2(1200.0, 900.0),
    };
    const COLUMN: f32 = 280.0;

    struct Card {
        ctx: egui::Context,
        app: OndinApp,
        ids: Vec<NodeId>,
        time: f64,
    }

    impl Card {
        /// Three rects, each given the stack `stacks[i]`, all selected.
        fn new(stacks: Vec<Vec<EffectKind>>) -> Self {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            let mut app = OndinApp::headless(&ctx);
            let mut src = ondin_core::IdSource::new(1);
            let root = src.mint();
            let mut doc = ondin_core::Document::new(root);
            let mut ids = Vec::new();
            for (index, kinds) in stacks.iter().enumerate() {
                let id = src.mint();
                doc.apply(&Transaction(vec![Operation::CreateNode {
                    id,
                    parent: root,
                    index,
                    kind: NodeKind::Rect {
                        size: ondin_core::kurbo::Size::new(100.0, 100.0),
                        corner_radii: Default::default(),
                    },
                    transform: None,
                    name: None,
                }]))
                .expect("a rect");
                if !kinds.is_empty() {
                    doc.apply(&Transaction(vec![Operation::SetEffects {
                        id,
                        effects: ondin_core::keyed_by_position(
                            kinds.iter().cloned().map(Effect::new),
                        ),
                    }]))
                    .expect("effects");
                }
                ids.push(id);
            }
            app.session.adopt_document(doc, None);
            app.session.selection.set(ids.clone());
            app.collapsed_panels.remove("Effects");
            let mut card = Self {
                ctx,
                app,
                ids,
                time: 0.0,
            };
            card.settle();
            card
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
            self.time += 0.1;
            let (app, ids) = (&mut self.app, self.ids.clone());
            self.ctx.run_ui(
                egui::RawInput {
                    time: Some(self.time),
                    screen_rect: Some(SCREEN),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.set_max_width(COLUMN);
                    app.inspector_effects(ui, &ids);
                },
            )
        }

        fn settle(&mut self) -> egui::FullOutput {
            let mut out = self.frame(Vec::new());
            for _ in 0..2 {
                out = self.frame(Vec::new());
            }
            out
        }

        fn click(&mut self, at: egui::Pos2) {
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            self.frame(vec![egui::Event::PointerMoved(at), button(at, true)]);
            self.frame(vec![button(at, false)]);
            self.settle();
        }

        /// Press the header's `+`, left of the `⋯`.
        fn add(&mut self) {
            let x = COLUMN - ui::CARD_MARGIN_X - ui::HEAD_MORE_W - ui::HEAD_ACTION_W * 0.5;
            self.click(egui::pos2(x, 19.0));
        }

        fn stack(&self, i: usize) -> Vec<Effect> {
            ondin_core::item::values(
                self.app
                    .session
                    .doc
                    .get(self.ids[i])
                    .expect("the layer")
                    .effects(),
            )
        }

        fn says(&mut self, text: &str) -> bool {
            let out = self.settle();
            out.shapes.iter().any(|cs| match &cs.shape {
                egui::epaint::Shape::Text(t) => t.galley.text() == text,
                _ => false,
            })
        }
    }

    fn drop_shadow() -> EffectKind {
        EffectKind::DropShadow(Default::default())
    }

    /// None of them has effects: adding gives it to all of them, as one step, from a collapsed header.
    #[test]
    fn adding_over_an_agreeing_empty_selection_reaches_every_layer() {
        let mut card = Card::new(vec![Vec::new(), Vec::new(), Vec::new()]);
        assert!(
            card.app.collapsed_panels.contains("Effects"),
            "three layers agreeing on nothing rest shut"
        );
        assert!(!card.says("No effects"), "so the body is not drawn at all");
        let depth = card.app.session.history.undo_depth();
        card.add();
        for i in 0..3 {
            let s = card.stack(i);
            assert_eq!(s.len(), 1, "layer {i} got the effect");
            assert!(matches!(s[0].kind, EffectKind::DropShadow(_)));
        }
        assert_eq!(
            card.app.session.history.undo_depth(),
            depth + 1,
            "three layers, one undo step"
        );
        assert!(
            !card.app.collapsed_panels.contains("Effects"),
            "and the `+` opened the card it added into"
        );
        assert!(card.says("Drop shadow"), "so the new row is on screen");
        card.app.session.undo();
        assert!(card.stack(0).is_empty() && card.stack(2).is_empty());
    }

    /// They agree, so the card shows the stack and an edit (the eye toggle) reaches all of them.
    #[test]
    fn an_edit_over_an_agreeing_selection_reaches_every_layer() {
        let same = vec![drop_shadow()];
        let mut card = Card::new(vec![same.clone(), same.clone(), same]);
        assert!(card.says("Drop shadow"), "the shared row is shown");
        let out = card.settle();
        let field = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Rect(r) => Some(r.rect),
                _ => None,
            })
            .find(|r| (r.height() - 28.0).abs() < 0.5 && r.width() > 100.0)
            .expect("a row");
        card.click(egui::pos2(field.right() + 6.0 + 14.0, field.center().y));
        for i in 0..3 {
            let s = card.stack(i);
            assert_eq!(s.len(), 1, "hiding is not removing, on layer {i}");
            assert!(!s[0].visible, "layer {i}'s effect is hidden");
        }
    }

    /// They disagree: the card says so, draws no rows, and the `+` replaces every layer's stack
    /// (unlike the Fill panel, whose `+` is inert); the bite is `s.len() == 1` on the two-effect layer.
    #[test]
    fn a_disagreement_is_stated_and_the_plus_replaces_every_stack() {
        let mut card = Card::new(vec![
            vec![drop_shadow()],
            Vec::new(),
            vec![EffectKind::LayerBlur { radius: 4.0 }, drop_shadow()],
        ]);
        assert!(
            card.says("These layers have different effects"),
            "the card states the disagreement"
        );
        assert!(
            !card.says("Drop shadow"),
            "and draws no rows, which would be one layer's list standing for three"
        );
        card.add();
        for i in 0..3 {
            let s = card.stack(i);
            assert_eq!(
                s.len(),
                1,
                "layer {i}'s whole stack is the one new effect, not an append"
            );
            assert!(matches!(s[0].kind, EffectKind::DropShadow(_)));
        }
        assert!(
            card.says("Drop shadow"),
            "and they agree now, so the row is drawn"
        );
    }

    /// A disagreement leaves the card open, where an agreed empty selection shuts it.
    #[test]
    fn the_card_opens_itself_over_a_disagreement() {
        let mut card = Card::new(vec![vec![drop_shadow()], Vec::new()]);
        assert!(!card.app.collapsed_panels.contains("Effects"));
        let ids = card.ids.clone();
        card.app
            .session
            .try_commit(Transaction(vec![Operation::SetEffects {
                id: ids[0],
                effects: Vec::new(),
            }]))
            .expect("clear");
        card.app.session.selection.set(ids.clone());
        card.app.synced_selection.remove("Effects");
        card.settle();
        assert!(
            card.app.collapsed_panels.contains("Effects"),
            "agreeing on nothing shuts it again"
        );
    }

    /// The detached picker does not outlive its row, and a click through it does not replace
    /// another layer's stack when the selection widens to a disagreement. Two halves:
    /// `slot_transaction`'s `Effect` arm reads each subject's own list, and `picker_ui` orphans a
    /// `PaintSlot::Effect` when `shared_effects` is `None`.
    #[test]
    fn a_widened_selection_cannot_replace_another_layers_effect_stack() {
        use ondin_core::peniko::Color;

        let shadow = |c: Color| {
            EffectKind::DropShadow(ondin_core::Shadow {
                offset: ondin_core::kurbo::Vec2::new(0.0, 4.0),
                blur: 4.0,
                spread: 0.0,
                color: c,
            })
        };
        let red = Brush::Solid(Color::from_rgba8(255, 0, 0, 255));

        let mut card = Card::new(vec![
            vec![shadow(Color::BLACK)],
            vec![
                EffectKind::LayerBlur { radius: 8.0 },
                EffectKind::InnerShadow(ondin_core::Shadow::default()),
            ],
            Vec::new(),
        ]);
        let (a, b) = (card.ids[0], card.ids[1]);
        card.app.session.selection.set(vec![a, b]);

        assert!(
            card.app.shared_effects(&[a, b]).is_none(),
            "fixture: the layers must disagree, or the card would be drawing rows"
        );
        let before = card.app.session.doc.get(b).expect("b").effects().to_vec();
        assert_eq!(before.len(), 2, "fixture: B has two effects");

        card.app.picker = Some(crate::panels::picker::Picker::new(
            a,
            PaintSlot::Effect(0),
            egui::pos2(400.0, 300.0),
        ));
        for _ in 0..2 {
            let app = &mut card.app;
            let _ = card.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(SCREEN),
                    ..Default::default()
                },
                |ui| app.picker_ui(ui.ctx()),
            );
        }
        assert!(
            card.app.picker.is_none(),
            "the picker must not outlive the row that opened it — the card is \
             drawing no rows at all over this disagreement"
        );

        card.app.write_slot(a, PaintSlot::Effect(0), red.clone());

        assert_eq!(
            card.app.session.doc.get(b).expect("b").effects(),
            &before[..],
            "a click on A's shadow colour must leave B's stack alone — B has a \
             layer blur and an inner shadow and neither is A's shadow"
        );

        let mut card = Card::new(vec![
            vec![shadow(Color::BLACK)],
            vec![shadow(Color::BLACK)],
            Vec::new(),
        ]);
        let (a, b) = (card.ids[0], card.ids[1]);
        card.app.session.selection.set(vec![a, b]);
        assert!(
            card.app.shared_effects(&[a, b]).is_some(),
            "control fixture: these two agree"
        );
        card.app.write_slot(a, PaintSlot::Effect(0), red);
        let painted =
            |app: &OndinApp, id: NodeId| match &app.session.doc.get(id).expect("node").effects()[0]
                .kind
            {
                EffectKind::DropShadow(s) => s.color,
                other => panic!("the control's entry is a drop shadow, got {other:?}"),
            };
        assert_eq!(
            painted(&card.app, b),
            Color::from_rgba8(255, 0, 0, 255),
            "control: over an agreeing selection the colour still reaches every layer"
        );
        assert_eq!(painted(&card.app, a), painted(&card.app, b));
    }
}

/// The multi-selection W and H fields read the proportion lock (a set is locked when every
/// member is), agreeing with the canvas handles and the `Ctrl`+arrows.
#[cfg(test)]
mod multi_lock_tests {
    use super::{MultiEdit, MultiField};
    use crate::app::OndinApp;
    use crate::theme;
    use eframe::egui;
    use ondin_core::kurbo::{Affine, Rect, Size};
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    /// Two 20×20 rects at (0, 0) and (80, 40), both selected: a 100 × 60 union, aspect 0.6.
    fn two_rects(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x10C);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (a, b) = (ids.mint(), ids.mint());
        let square = |id, at: (f64, f64)| Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate(at)),
            name: None,
        };
        doc.apply(&Transaction(vec![
            square(a, (0.0, 0.0)),
            square(b, (80.0, 40.0)),
        ]))
        .expect("build");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![a, b]);
        (app, a, b)
    }

    fn span(app: &OndinApp, a: NodeId, b: NodeId) -> Rect {
        let w = |id| app.session.resolved.world_bounds(id).expect("bounds");
        w(a).union(w(b))
    }

    fn lock(app: &mut OndinApp, ids: &[NodeId]) {
        app.session.commit(Transaction(
            ids.iter()
                .map(|id| Operation::SetProportionsLocked {
                    id: *id,
                    locked: true,
                })
                .collect(),
        ));
    }

    /// Typing a width over a locked set carries the height, and over a mixed one does not.
    #[test]
    fn a_typed_width_over_a_locked_set_carries_the_height() {
        let ctx = egui::Context::default();

        let (mut app, a, b) = two_rects(&ctx);
        assert_eq!(
            (span(&app, a, b).width(), span(&app, a, b).height()),
            (100.0, 60.0),
            "the fixture"
        );
        lock(&mut app, &[a, b]);
        let tx = app.multi_transform_tx(MultiField::W, MultiEdit::Absolute(150.0));
        app.session.commit(tx);
        let locked = span(&app, a, b);
        assert_eq!(locked.width(), 150.0, "the field's own axis");
        assert_eq!(locked.height(), 90.0, "and the height follows at 0.6");

        let (mut app, a, b) = two_rects(&ctx);
        lock(&mut app, &[a]);
        let tx = app.multi_transform_tx(MultiField::W, MultiEdit::Absolute(150.0));
        app.session.commit(tx);
        let mixed = span(&app, a, b);
        assert_eq!(mixed.width(), 150.0);
        assert_eq!(
            mixed.height(),
            60.0,
            "a set is only as constrained as its least constrained member (D50)"
        );
    }

    /// The chain button sets, it does not toggle: over a mixed set one press locks every member.
    #[test]
    fn the_chain_button_drives_a_mixed_set_to_one_state_in_one_step() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = two_rects(&ctx);
        lock(&mut app, &[a]);
        let locked = |app: &OndinApp, id| {
            app.session
                .doc
                .get(id)
                .expect("in the tree")
                .proportions_locked()
        };
        assert!(
            locked(&app, a) && !locked(&app, b),
            "the fixture: a mixed set"
        );
        assert!(
            !crate::tools::proportions_locked(&app.session.doc, app.session.selection.ids()),
            "and it reads as unlocked, which is what the press is answering"
        );

        let depth = app.session.history.undo_depth();
        let tx = super::lock_all_tx(app.session.selection.ids(), true);
        app.session.commit(tx);
        assert!(
            locked(&app, a) && locked(&app, b),
            "one press locks every member, the one that already was included"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            depth + 1,
            "and it is one undo step, not one per layer"
        );

        let tx = super::lock_all_tx(app.session.selection.ids(), false);
        app.session.commit(tx);
        assert!(
            !locked(&app, a) && !locked(&app, b),
            "and the other way round"
        );
    }

    /// The H field pairs the other way and holds the same corner: the axis it does not drive is
    /// anchored at the union's top-left, not its far edge, or the box would move instead of resize.
    #[test]
    fn a_typed_height_over_a_locked_set_carries_the_width() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = two_rects(&ctx);
        lock(&mut app, &[a, b]);
        let tx = app.multi_transform_tx(MultiField::H, MultiEdit::Absolute(90.0));
        app.session.commit(tx);
        let after = span(&app, a, b);
        assert_eq!(after.height(), 90.0, "the field's own axis");
        assert_eq!(
            after.width(),
            150.0,
            "and the width follows back through 0.6"
        );
        assert_eq!(
            (after.min_x(), after.min_y()),
            (0.0, 0.0),
            "anchored at the union's top-left, not floated off it"
        );
    }
}

/// What each of the multi-selection Transform card's fields means: the X and Y arms' direction
/// and all of `multi_angle_tx`.
#[cfg(test)]
mod multi_field_tests {
    use super::{MultiEdit, MultiField};
    use crate::app::OndinApp;
    use crate::theme;
    use eframe::egui;
    use ondin_core::build;
    use ondin_core::kurbo::{Affine, Rect, Size};
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    /// Two 20×20 rects at (0, 0) and (80, 40) under the transforms given, both selected.
    fn two(ctx: &egui::Context, ta: Affine, tb: Affine) -> (OndinApp, NodeId, NodeId) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x10C);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (a, b) = (ids.mint(), ids.mint());
        let square = |id, at: (f64, f64), t: Affine| Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate(at) * t),
            name: None,
        };
        doc.apply(&Transaction(vec![
            square(a, (0.0, 0.0), ta),
            square(b, (80.0, 40.0), tb),
        ]))
        .expect("build");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![a, b]);
        (app, a, b)
    }

    /// The two members turned by `a_deg` and `b_deg` about their own origins.
    fn turned(ctx: &egui::Context, a_deg: f64, b_deg: f64) -> (OndinApp, NodeId, NodeId) {
        two(
            ctx,
            Affine::rotate(a_deg.to_radians()),
            Affine::rotate(b_deg.to_radians()),
        )
    }

    fn span(app: &OndinApp, a: NodeId, b: NodeId) -> Rect {
        let w = |id| app.session.resolved.world_bounds(id).expect("bounds");
        w(a).union(w(b))
    }

    /// The angle the card would show for one layer, in degrees.
    fn deg(app: &OndinApp, id: NodeId) -> f64 {
        let w = app.session.resolved.world_transform(id).expect("world");
        build::Basis::of(w).orientation.angle.to_degrees()
    }

    /// The lean the Skew field would show for one layer, in radians (the units of `tools::MAX_SKEW`).
    fn lean(app: &OndinApp, id: NodeId) -> f64 {
        let w = app.session.resolved.world_transform(id).expect("world");
        build::Basis::of(w).orientation.skew
    }

    /// X moves along x, Y along y, and neither resizes: `multi_transform_tx`'s arms pick the `Vec2` component.
    #[test]
    fn a_nudged_x_moves_the_union_sideways_and_a_nudged_y_moves_it_down() {
        let ctx = egui::Context::default();

        let (mut app, a, b) = turned(&ctx, 0.0, 0.0);
        let before = span(&app, a, b);
        assert_eq!(
            (
                before.min_x(),
                before.min_y(),
                before.width(),
                before.height()
            ),
            (0.0, 0.0, 100.0, 60.0),
            "the fixture"
        );
        let tx = app.multi_transform_tx(MultiField::X, MultiEdit::Nudge(25.0));
        app.session.commit(tx);
        let after = span(&app, a, b);
        assert_eq!(
            (after.min_x(), after.min_y()),
            (25.0, 0.0),
            "X moves along x, and only along x"
        );
        assert_eq!(
            (after.width(), after.height()),
            (100.0, 60.0),
            "and it is a move, not a resize"
        );

        let (mut app, a, b) = turned(&ctx, 0.0, 0.0);
        let tx = app.multi_transform_tx(MultiField::Y, MultiEdit::Nudge(25.0));
        app.session.commit(tx);
        let after = span(&app, a, b);
        assert_eq!(
            (after.min_x(), after.min_y()),
            (0.0, 25.0),
            "and Y along y, and only along y"
        );

        let (mut app, a, b) = turned(&ctx, 0.0, 0.0);
        let tx = app.multi_transform_tx(MultiField::X, MultiEdit::Absolute(-40.0));
        app.session.commit(tx);
        assert_eq!(
            span(&app, a, b).min_x(),
            -40.0,
            "an absolute X puts the union's left edge on the number"
        );
    }

    /// A nudge keeps the differences (0° and 40° become 5° and 45°); an absolute lands both at 30°.
    #[test]
    fn a_nudged_angle_keeps_the_differences_and_an_absolute_lands_them_together() {
        let ctx = egui::Context::default();

        let (mut app, a, b) = turned(&ctx, 0.0, 40.0);
        assert!(
            (deg(&app, a) - 0.0).abs() < 1e-9 && (deg(&app, b) - 40.0).abs() < 1e-9,
            "the fixture: {} and {}",
            deg(&app, a),
            deg(&app, b)
        );
        let tx = app.multi_transform_tx(MultiField::Rotation, MultiEdit::Nudge(5.0));
        app.session.commit(tx);
        assert!(
            (deg(&app, a) - 5.0).abs() < 1e-6,
            "the layer at 0° goes to 5°, at {}",
            deg(&app, a)
        );
        assert!(
            (deg(&app, b) - 45.0).abs() < 1e-6,
            "and the one at 40° to 45° — the arrangement survives, at {}",
            deg(&app, b)
        );

        let (mut app, a, b) = turned(&ctx, 0.0, 40.0);
        let tx = app.multi_transform_tx(MultiField::Rotation, MultiEdit::Absolute(30.0));
        app.session.commit(tx);
        assert!(
            (deg(&app, a) - 30.0).abs() < 1e-6 && (deg(&app, b) - 30.0).abs() < 1e-6,
            "an absolute brings a mixed set into line, at {} and {}",
            deg(&app, a),
            deg(&app, b)
        );
    }

    /// A rotation carries each layer's scale rather than rebuilding it: a 20 × 20 rect scaled to
    /// 40 × 60 and turned a quarter turn measures 60 × 40, not 20 × 20.
    #[test]
    fn a_rotation_nudge_carries_each_layers_scale_rather_than_rebuilding_it() {
        let ctx = egui::Context::default();
        let (mut app, a, _b) = two(
            &ctx,
            Affine::scale_non_uniform(2.0, 3.0),
            Affine::scale_non_uniform(2.0, 3.0),
        );
        let size = |app: &OndinApp, id| {
            let r = app.session.resolved.world_bounds(id).expect("bounds");
            (
                (r.width() * 1e6).round() / 1e6,
                (r.height() * 1e6).round() / 1e6,
            )
        };
        assert_eq!(size(&app, a), (40.0, 60.0), "the fixture");

        let tx = app.multi_transform_tx(MultiField::Rotation, MultiEdit::Nudge(90.0));
        app.session.commit(tx);
        assert_eq!(
            size(&app, a),
            (60.0, 40.0),
            "a quarter turn swaps the box and keeps the scale"
        );
        let split = build::Basis::of(app.session.resolved.world_transform(a).expect("world"));
        assert!(
            (split.scale.x - 2.0).abs() < 1e-9 && (split.scale.y - 3.0).abs() < 1e-9,
            "and the scale is carried, not recomputed: {:?}",
            split.scale
        );
    }

    /// The Skew field stops at `tools::MAX_SKEW`, short of the vertical where the matrix has no
    /// inverse; unclamped, 200° would silently lean 20° (`tan` has period π).
    #[test]
    fn a_skew_past_the_limit_stops_at_max_skew() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = turned(&ctx, 0.0, 0.0);
        let tx = app.multi_transform_tx(MultiField::Skew, MultiEdit::Absolute(200.0));
        app.session.commit(tx);
        for id in [a, b] {
            assert!(
                (lean(&app, id) - crate::tools::MAX_SKEW).abs() < 1e-6,
                "clamped to MAX_SKEW ({}), got {}",
                crate::tools::MAX_SKEW,
                lean(&app, id)
            );
        }

        let (mut app, a, _b) = turned(&ctx, 0.0, 0.0);
        let tx = app.multi_transform_tx(MultiField::Skew, MultiEdit::Absolute(-200.0));
        app.session.commit(tx);
        assert!(
            (lean(&app, a) + crate::tools::MAX_SKEW).abs() < 1e-6,
            "and −200° clamps to −MAX_SKEW, got {}",
            lean(&app, a)
        );
    }

    /// `shared_angle` calls a set `Mixed` only past `EPS` (about 0.05°): bracketed at 0.04° and 0.06°.
    #[test]
    fn a_shared_angle_reports_mixed_only_past_what_the_field_can_show() {
        let ctx = egui::Context::default();

        let (app, _a, _b) = turned(&ctx, 0.0, 0.04);
        assert!(
            app.shared_angle(MultiField::Rotation).is_some(),
            "0.04° apart is inside the window and shows a number"
        );

        let (app, _a, _b) = turned(&ctx, 0.0, 0.06);
        assert!(
            app.shared_angle(MultiField::Rotation).is_none(),
            "0.06° apart is outside it and shows Mixed"
        );

        let (app, _a, _b) = turned(&ctx, 0.0, 0.06);
        assert!(
            app.shared_angle(MultiField::Skew).is_some(),
            "two turned layers still agree about their lean, which is zero for both"
        );
    }

    /// A member whose basis cannot be split is skipped, not flattened: `multi_angle_tx` rebuilds
    /// from `Basis::of`, which would flatten a rank-1 layer even on a zero nudge.
    #[test]
    fn a_member_with_no_exact_basis_is_skipped_rather_than_flattened() {
        let ctx = egui::Context::default();
        let rank_one = Affine::new([1.0, 2.0, 2.0, 4.0, 0.0, 0.0]);
        assert_eq!(rank_one.determinant(), 0.0, "the fixture is singular");

        let (mut app, a, b) = two(&ctx, rank_one, Affine::rotate(0.4));
        let stored = |app: &OndinApp, id| {
            app.session
                .doc
                .get(id)
                .expect("node")
                .transform()
                .as_coeffs()
        };
        let (a_before, b_before) = (stored(&app, a), stored(&app, b));

        let tx = app.multi_transform_tx(MultiField::Rotation, MultiEdit::Nudge(0.0));
        app.session.commit(tx);
        assert_eq!(
            stored(&app, a),
            a_before,
            "the rank-1 member is left exactly where it was"
        );
        // A zero nudge on a rotated layer round-trips through `Basis::of` one ulp off, which is not this test's subject.
        for (got, want) in stored(&app, b).iter().zip(b_before.iter()) {
            assert!(
                (got - want).abs() < 1e-9,
                "its ordinary sibling stays put too, this nudge being zero: \
                 {:?} against {:?}",
                stored(&app, b),
                b_before
            );
        }

        let tx = app.multi_transform_tx(MultiField::Rotation, MultiEdit::Nudge(15.0));
        app.session.commit(tx);
        assert_eq!(
            stored(&app, a),
            a_before,
            "a turn it cannot represent leaves it alone rather than collapsing it"
        );
        assert_ne!(
            stored(&app, b),
            b_before,
            "and the rest of the selection still turns — a refused member must not \
             refuse the gesture"
        );
    }
}

/// The multi-selection Transform card's typed path, driven through real pointer and key events,
/// since the subject is which frame a `DragValue` reports `changed()`. The field's rect is found
/// by drawing it: `multi_number` returns its `Response`.
#[cfg(test)]
mod multi_valve_tests {
    use super::{MultiEdit, MultiField, Prefix};
    use crate::app::OndinApp;
    use crate::theme;
    use eframe::egui;
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    fn two_rects(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x10C);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (a, b) = (ids.mint(), ids.mint());
        let square = |id, at: (f64, f64)| Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate(at)),
            name: None,
        };
        doc.apply(&Transaction(vec![
            square(a, (0.0, 0.0)),
            square(b, (80.0, 40.0)),
        ]))
        .expect("build");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![a, b]);
        (app, a, b)
    }

    /// The union's width, which is what the W field reads and writes.
    fn union_width(app: &OndinApp) -> f64 {
        app.multi_union().expect("a union over two rects").width()
    }

    /// One rect's authored width: 20 in the fixture, a fifth of the union.
    fn rect_width(app: &OndinApp, id: NodeId) -> f64 {
        match app.session.doc.get(id).map(|n| n.kind()) {
            Some(NodeKind::Rect { size, .. }) => size.width,
            other => panic!("the fixture makes rects, not {other:?}"),
        }
    }

    /// One frame with the W field alone in a `Ui`, returning the rect it was laid out in.
    fn frame(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) -> egui::Rect {
        let mut out = egui::Rect::NOTHING;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(280.0, 600.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                let w = app.multi_union().map(|u| u.width());
                out = app
                    .multi_number(
                        ui,
                        egui::vec2(120.0, 28.0),
                        MultiField::W,
                        Prefix::Text("W"),
                        w,
                    )
                    .rect;
            },
        );
        out
    }

    /// A press or release on `pos`; without movement it puts a `DragValue` into text-edit mode.
    fn click(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// The fixture with the W field laid out twice, returning its centre.
    fn settled(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId, egui::Pos2) {
        let (mut app, a, b) = two_rects(ctx);
        let mut at = egui::Pos2::ZERO;
        for _ in 0..2 {
            at = frame(ctx, &mut app, Vec::new()).center();
        }
        (app, a, b, at)
    }

    /// Click into the field and type `text`, leaving it focused.
    fn type_into(ctx: &egui::Context, app: &mut OndinApp, at: egui::Pos2, text: &str) {
        frame(ctx, app, vec![egui::Event::PointerMoved(at)]);
        frame(ctx, app, vec![click(at, true)]);
        frame(ctx, app, vec![click(at, false)]);
        frame(ctx, app, Vec::new());
        for ch in text.chars() {
            frame(ctx, app, vec![egui::Event::Text(ch.to_string())]);
        }
    }

    /// Click somewhere the field is not, as when the user moves to the field beside it.
    fn click_away(ctx: &egui::Context, app: &mut OndinApp) {
        let away = egui::pos2(5.0, 500.0);
        frame(ctx, app, vec![click(away, true)]);
        frame(ctx, app, vec![click(away, false)]);
        for _ in 0..2 {
            frame(ctx, app, Vec::new());
        }
    }

    /// A typed number on the multi card is one undo step: keystrokes preview, and the `pending`
    /// arm in `multi_number` commits once on the falling edge.
    #[test]
    fn typing_a_multi_width_leaves_exactly_one_step_in_the_history() {
        let ctx = egui::Context::default();
        let (mut app, a, b, at) = settled(&ctx);
        let depth0 = app.session.history.undo_depth();
        assert_eq!(union_width(&app), 100.0, "the fixture's union");

        type_into(&ctx, &mut app, at, "500");
        assert_eq!(
            app.session.history.undo_depth(),
            depth0,
            "nothing is committed while the field is engaged: the keystrokes preview"
        );
        click_away(&ctx, &mut app);

        assert_eq!(
            app.session.history.undo_depth() - depth0,
            1,
            "one number is one step: `5`, `50`, `500` and two frames of losing \
             focus are not five edits"
        );
        assert_eq!(union_width(&app), 500.0, "and the number landed");
        assert_eq!(
            (rect_width(&app, a), rect_width(&app, b)),
            (100.0, 100.0),
            "both members scaled, from the union's own left edge"
        );
        app.session.undo();
        assert_eq!(union_width(&app), 100.0, "and one undo puts all of it back");
    }

    /// `Escape` abandons a typed multi edit: egui hands back what was typed and drops focus, so
    /// the valve must refuse itself. The width assertion bites; the count does not.
    #[test]
    fn escape_abandons_a_typed_multi_edit() {
        let ctx = egui::Context::default();
        let (mut app, _a, _b, at) = settled(&ctx);
        let depth0 = app.session.history.undo_depth();

        type_into(&ctx, &mut app, at, "500");
        frame(
            &ctx,
            &mut app,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        for _ in 0..3 {
            frame(&ctx, &mut app, Vec::new());
        }

        assert_eq!(union_width(&app), 100.0, "the typed number was abandoned");
        assert_eq!(
            app.session.history.undo_depth(),
            depth0,
            "and nothing reached the history to be undone"
        );
    }

    /// A bare click through a field commits nothing. A symptom guard over three independent doors
    /// (the engagement latch, the box fields' no-op guard, `commit_inner` dropping a
    /// `changes_nothing` transaction): green when any one is removed, so no evidence for any.
    #[test]
    fn a_bare_click_through_a_multi_field_commits_nothing() {
        let ctx = egui::Context::default();
        let (mut app, _a, _b, at) = settled(&ctx);
        let depth0 = app.session.history.undo_depth();
        let dirty0 = app.session.is_dirty();

        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        frame(&ctx, &mut app, vec![click(at, true)]);
        frame(&ctx, &mut app, vec![click(at, false)]);
        frame(&ctx, &mut app, Vec::new());
        click_away(&ctx, &mut app);

        assert_eq!(
            app.session.history.undo_depth(),
            depth0,
            "a click that changed nothing is not an edit"
        );
        assert_eq!(union_width(&app), 100.0, "and it moved nothing");
        assert_eq!(
            app.session.is_dirty(),
            dirty0,
            "so no autosave and no crash-snapshot write are scheduled either"
        );
    }

    /// The builder half: the box fields skip a no-op write, or a zero nudge would build one op per
    /// member (`build::move_by_world`, `tools::resize_selection`). W and H share one guard.
    #[test]
    fn a_zero_nudge_builds_no_transaction_at_all() {
        let ctx = egui::Context::default();
        let (app, _a, _b) = two_rects(&ctx);
        for field in [
            MultiField::X,
            MultiField::Y,
            MultiField::W,
            MultiField::H,
            MultiField::Rotation,
            MultiField::Skew,
        ] {
            let tx = app.multi_transform_tx(field, MultiEdit::Nudge(0.0));
            assert!(
                tx.0.is_empty(),
                "{field:?} builds {} ops for a nudge of nothing",
                tx.0.len()
            );
        }
        let tx = app.multi_transform_tx(MultiField::X, MultiEdit::Nudge(10.0));
        assert_eq!(tx.0.len(), 2, "a real move writes both members");
        assert!(
            matches!(tx, Transaction(_)),
            "and it is an ordinary transaction"
        );
    }

    /// A scrub previews while dragging and commits once on `drag_stopped()`, clearing the total so
    /// the next field does not start from it; the last assertion sees the clear.
    #[test]
    fn a_scrubbed_multi_width_is_one_step_and_leaves_no_total_behind() {
        let ctx = egui::Context::default();
        let (mut app, _a, _b, at) = settled(&ctx);
        let depth0 = app.session.history.undo_depth();

        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        frame(&ctx, &mut app, vec![click(at, true)]);
        for i in 1..=3 {
            let to = at + egui::vec2(10.0 * i as f32, 0.0);
            frame(&ctx, &mut app, vec![egui::Event::PointerMoved(to)]);
            assert_eq!(
                app.session.history.undo_depth(),
                depth0,
                "a drag previews; it does not write a frame at a time"
            );
        }
        frame(
            &ctx,
            &mut app,
            vec![click(at + egui::vec2(30.0, 0.0), false)],
        );

        assert_eq!(
            app.session.history.undo_depth() - depth0,
            1,
            "one scrub is one step"
        );
        assert!(
            union_width(&app) > 100.0,
            "and it widened the union, at {}",
            union_width(&app)
        );
        assert!(
            app.multi_transform_drag.is_none(),
            "and the total is spent, or the next field picked up starts from it"
        );
    }

    /// One layer and two print the same number: a shared corner radius of 4.5 reads 4.5, not 4,
    /// on the painted galleys of both cards; the two assertions guard separate formatters.
    #[test]
    fn a_number_reads_the_same_with_one_layer_selected_and_with_two() {
        fn text_of(shape: &egui::Shape, into: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(t) => into.push(t.galley.text().to_string()),
                egui::Shape::Vec(v) => v.iter().for_each(|s| text_of(s, into)),
                _ => {}
            }
        }
        let painted = |app: &mut OndinApp, ctx: &egui::Context, multi: bool, id: NodeId| {
            let mut out = Vec::new();
            for _ in 0..2 {
                let full = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(320.0, 900.0),
                        )),
                        ..Default::default()
                    },
                    |ui| match multi {
                        true => app.inspector_multi(ui),
                        false => app.inspector_single(ui, id),
                    },
                );
                out = Vec::new();
                for cs in &full.shapes {
                    text_of(&cs.shape, &mut out);
                }
            }
            out
        };

        let ctx = egui::Context::default();
        let (mut app, a, b) = two_rects(&ctx);
        let tx = ondin_core::set_corner_radius_all(&app.session.doc, &[a, b], 4.5);
        app.session.commit(tx);
        assert!(
            matches!(
                app.session.doc.get(a).map(|n| n.kind()),
                Some(NodeKind::Rect { corner_radii, .. }) if corner_radii.top_left == 4.5
            ),
            "the fixture holds 4.5, or this test is about nothing"
        );

        let multi = painted(&mut app, &ctx, true, a);
        assert!(
            multi.iter().any(|t| t == "4.5"),
            "a shared radius of 4.5 must read 4.5 and not 4 — the panel would be \
             reporting a radius the document does not hold. Painted: {multi:?}"
        );
        assert!(
            !multi
                .iter()
                .any(|t| t.ends_with(".00") || t.ends_with(".00°")),
            "and no number gains trailing zeros on the way from one layer to two. \
             Painted: {multi:?}"
        );

        app.session.selection.set(vec![a]);
        let single = painted(&mut app, &ctx, false, a);
        assert!(
            single.iter().any(|t| t == "4.5"),
            "the single-layer card is the thing being matched, and it reads 4.5. \
             Painted: {single:?}"
        );
    }
}

/// One rhythm for the whole inspector — the Transform card's, everywhere.
#[cfg(test)]
mod rhythm_tests {
    use super::*;
    use crate::app::{OndinApp, TypeTab};
    use ondin_core::kurbo::Size;

    const SCREEN: egui::Vec2 = egui::vec2(1400.0, 2400.0);
    /// The column the inspector is drawn into.
    const PANE: f32 = 296.0;

    /// Every control-shaped box a pass painted, as rows top to bottom and each row left to right.
    /// Picked by its hairline (stroked, 18 to 34 points tall), so a new control is measured
    /// without being added.
    fn control_rows(out: &egui::FullOutput) -> Vec<Vec<egui::Rect>> {
        let mut v: Vec<egui::Rect> = Vec::new();
        for cs in &out.shapes {
            if let egui::Shape::Rect(r) = &cs.shape
                && r.stroke.width > 0.0
                && r.rect.height() >= 18.0
                && r.rect.height() <= 34.0
                && r.rect.width() >= 14.0
                && !v.contains(&r.rect)
            {
                v.push(r.rect);
            }
        }
        v.sort_by(|a, b| {
            a.center()
                .y
                .partial_cmp(&b.center().y)
                .unwrap()
                .then(a.left().partial_cmp(&b.left()).unwrap())
        });
        let mut rows: Vec<Vec<egui::Rect>> = Vec::new();
        for r in v {
            // A popover `Area` beside the panel can share a y, so anything further than a panel's width starts a new row.
            let far = |row: &Vec<egui::Rect>| r.left() - row[row.len() - 1].right() > PANE;
            match rows.last_mut() {
                Some(row) if (row[0].center().y - r.center().y).abs() < 4.0 && !far(row) => {
                    row.push(r)
                }
                _ => rows.push(vec![r]),
            }
        }
        rows
    }

    /// Four passes, because interaction state is last frame's and panels size from their own responses.
    fn pass(app: &mut OndinApp, ctx: &egui::Context) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input.clone(), |ui| {
            ui.set_max_width(PANE);
            app.inspector_ui(ui)
        });
        for _ in 0..3 {
            out = ctx.run_ui(input.clone(), |ui| {
                ui.set_max_width(PANE);
                app.inspector_ui(ui)
            });
        }
        out
    }

    /// A rect painted before its first frame: `sync_paint_collapse` shuts the Fill and Stroke cards when a node arrives unpainted.
    fn painted_rect(app: &mut OndinApp) -> NodeId {
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let id = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 100.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id,
                fills: ondin_core::keyed_by_position([ondin_core::Fill {
                    brush: ondin_core::peniko::Brush::Solid(ondin_core::peniko::Color::from_rgb8(
                        200, 90, 60,
                    ))
                    .into(),
                    visible: true,
                }]),
            },
            Operation::SetStrokes {
                id,
                strokes: ondin_core::keyed_by_position([ondin_core::Stroke {
                    width: 2.0,
                    sides: ondin_core::StrokeSides::Custom {
                        top: 1.0,
                        right: 1.0,
                        bottom: 1.0,
                        left: 1.0,
                    },
                    ..Default::default()
                }]),
            },
        ]))
        .expect("a painted rect");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        id
    }

    fn text_node(app: &mut OndinApp) {
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let id = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Text {
                content: "gypsy".into(),
                style: Box::new(ondin_core::TextStyle {
                    font_family: "Inter".into(),
                    font_size: 20.0,
                    ..Default::default()
                }),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing: ondin_core::TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("a text node");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
    }

    /// Every panel and popover stacks and spaces its controls as the Transform card does:
    /// `ui::CARD_COL_GAP` between two on a row, `ui::CARD_ROW_GAP` between two rows of a card.
    /// Only row gaps under 20 points count as within a card; `IMAGE_CARD_GAP` needs a decoded
    /// texture, so its equality with the panel pitch is asserted at the end.
    #[test]
    fn every_panel_keeps_the_transform_cards_rhythm() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut app = OndinApp::headless(&ctx);

        let mut wrong: Vec<String> = Vec::new();
        let mut check = |tag: &str, out: &egui::FullOutput| {
            let rows = control_rows(out);
            assert!(
                rows.len() >= 5,
                "{tag} painted {} rows of controls, so this measured almost nothing",
                rows.len()
            );
            let mut prev: Option<&Vec<egui::Rect>> = None;
            for row in &rows {
                for r in row {
                    if (r.height() - ui::CONTROL_H).abs() >= 0.26 {
                        wrong.push(format!(
                            "{tag}: y{:.0} a control {:.1} tall",
                            r.top(),
                            r.height()
                        ));
                    }
                }
                for (i, pair) in row.windows(2).enumerate() {
                    let g = pair[1].left() - pair[0].right();
                    if (g - ui::CARD_COL_GAP).abs() >= 0.26 {
                        wrong.push(format!(
                            "{tag}: y{:.0} pair {i} across at {g:.1}",
                            row[0].top()
                        ));
                    }
                }
                if let Some(p) = prev {
                    let below = p.iter().fold(f32::MIN, |a, r| a.max(r.bottom()));
                    let g = row[0].top() - below;
                    // Rows stack only within a column; a popover's rows interleave with the panel's in y.
                    let span = |r: &Vec<egui::Rect>| {
                        (
                            r.iter().fold(f32::MAX, |a, k| a.min(k.left())),
                            r.iter().fold(f32::MIN, |a, k| a.max(k.right())),
                        )
                    };
                    let ((al, ar), (bl, br)) = (span(p), span(row));
                    if al.max(bl) >= ar.min(br) {
                        prev = Some(row);
                        continue;
                    }
                    // Zero counts (flush rows); only an overlapping popover's negative gaps are excluded.
                    if (0.0..20.0).contains(&g) && (g - ui::CARD_ROW_GAP).abs() >= 0.26 {
                        wrong.push(format!("{tag}: y{:.0} down at {g:.1}", row[0].top()));
                    }
                }
                prev = Some(row);
            }
        };

        painted_rect(&mut app);
        // Set after a first pass: the frame on which the subject changes runs `forget_stroke_chrome`, which clears it.
        let _ = pass(&mut app, &ctx);
        app.stroke_sides_open.push(0);
        let out = pass(&mut app, &ctx);
        check("rect, filled and stroked", &out);

        app.stroke_menu = Some(0);
        let out = pass(&mut app, &ctx);
        check("stroke popover", &out);
        // Dashed: the pattern's row and the offset row below it.
        let rect_id = app.session.selection.single().expect("the rect");
        let mut dashed = app
            .session
            .doc
            .get(rect_id)
            .unwrap()
            .paint()
            .strokes
            .to_vec();
        dashed[0].dashes = vec![4.0, 2.0];
        let tx = app.strokes_tx(PaintScope::Node(rect_id), dashed);
        assert!(app.session.commit(tx), "the fixture's stroke is dashed");
        let out = pass(&mut app, &ctx);
        check("stroke popover, dashed", &out);
        let glyphs = |out: &egui::FullOutput| {
            fn walk(s: &egui::Shape, v: &mut Vec<String>) {
                match s {
                    egui::Shape::Text(t) => v.push(t.galley.text().to_string()),
                    egui::Shape::Vec(x) => x.iter().for_each(|s| walk(s, v)),
                    _ => {}
                }
            }
            let mut v = Vec::new();
            out.shapes.iter().for_each(|c| walk(&c.shape, &mut v));
            v
        };
        assert!(
            glyphs(&out).iter().any(|t| t == icon::ARROW_LINE_RIGHT),
            "a dashed stroke's popover offers the dash offset"
        );
        app.stroke_menu = None;

        text_node(&mut app);
        let out = pass(&mut app, &ctx);
        check("text", &out);

        for tab in TypeTab::ALL {
            app.type_menu = Some(tab);
            let out = pass(&mut app, &ctx);
            check(&format!("type popover, {}", tab.label()), &out);
        }

        assert!(
            wrong.is_empty(),
            "{} gaps are not the Transform card's {} across / {} down:\n  {}",
            wrong.len(),
            ui::CARD_COL_GAP,
            ui::CARD_ROW_GAP,
            wrong.join("\n  ")
        );

        // The two pitches nothing above reaches: the image card needs a texture, a popover's sections an eyebrow.
        assert_eq!(
            IMAGE_CARD_GAP,
            ui::CARD_ROW_GAP,
            "image editing's card stacks its rows at its own pitch again"
        );
        assert_eq!(
            ui::POPOVER_SECTION_GAP,
            ui::CARD_ROW_GAP,
            "a popover's sections are spaced by a number of their own again"
        );
    }

    /// A headless app showing one frame carrying `n` layout grids on alternating axes.
    fn frame_with_grids(app: &mut OndinApp, n: usize) -> NodeId {
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let id = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(393.0, 852.0),
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id,
                fills: ondin_core::keyed_by_position([Fill {
                    brush: super::paint::new_solid().into(),
                    visible: true,
                }]),
            },
        ]))
        .expect("a frame");
        app.session.adopt_document(doc, None);
        if n > 0 {
            app.session
                .try_commit(Transaction(vec![Operation::SetLayoutGrids {
                    id,
                    grids: ondin_core::keyed_by_position((0..n).map(|i| {
                        LayoutGrid::new(match i % 2 {
                            0 => GridAxis::Columns,
                            _ => GridAxis::Rows,
                        })
                    })),
                }]))
                .expect("grids to draw");
        }
        app.session.selection.set_one(id);
        id
    }

    /// Three frames and a rectangle, all selected, with each frame's grid list set from `lists`;
    /// returns the frame ids. The rectangle makes the `frame_subjects` filter observable.
    fn frames_with_lists(app: &mut OndinApp, lists: &[Vec<LayoutGrid>]) -> Vec<NodeId> {
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let mut frames = Vec::new();
        let mut ops = Vec::new();
        for i in 0..lists.len() {
            let id = ids.mint();
            ops.push(Operation::CreateNode {
                id,
                parent: root,
                index: i,
                kind: NodeKind::Artboard {
                    size: Size::new(393.0, 852.0),
                },
                // Side by side: coincident frames would make "the union grew" vacuous.
                transform: Some(Affine::translate((500.0 * i as f64, 0.0))),
                name: None,
            });
            frames.push(id);
        }
        let rect = ids.mint();
        ops.push(Operation::CreateNode {
            id: rect,
            parent: root,
            index: lists.len(),
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        });
        doc.apply(&Transaction(ops)).expect("frames and a rect");
        app.session.adopt_document(doc, None);
        for (id, grids) in frames.iter().zip(lists) {
            if !grids.is_empty() {
                app.session
                    .try_commit(Transaction(vec![Operation::SetLayoutGrids {
                        id: *id,
                        grids: ondin_core::keyed_by_position(grids.clone()),
                    }]))
                    .expect("grids to draw");
            }
        }
        let mut all = frames.clone();
        all.push(rect);
        app.session.selection.set(all);
        frames
    }

    /// Draw the Layout grid panel four times over the selection, optionally pressing `+`, and
    /// report what the card said and whether it is collapsed.
    fn grid_panel_pass(
        ctx: &egui::Context,
        app: &mut OndinApp,
        click_plus: bool,
    ) -> (Vec<String>, bool) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
            ..Default::default()
        };
        let mut said = Vec::new();
        for i in 0..4 {
            let out = ctx.run_ui(input.clone(), |ui| {
                ui.set_max_width(PANE);
                app.inspector_layout_grids(ui);
            });
            if i == 3 {
                said = out
                    .shapes
                    .iter()
                    .filter_map(|cs| match &cs.shape {
                        egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                        _ => None,
                    })
                    .collect();
            }
        }
        // The `+` click is stood in for by `grid_add`, not a copy of its rule.
        if click_plus {
            let subjects = app.frame_subjects();
            match grid_add(app.shared_grids(&subjects).as_deref()) {
                GridAdd::Append(seed) => app.append_grid(&subjects, seed),
                GridAdd::Write(seed) => app.write_grids(&subjects, &[seed]),
            }
            app.open_paint_panel("Layout grid");
        }
        let collapsed = app.collapsed_panels.contains("Layout grid");
        (said, collapsed)
    }

    /// The three states a multi-selection of frames puts the panel in (nothing, agreeing,
    /// disagreeing) and what the `+` does in each, as the Export panel does.
    #[test]
    fn a_multi_selection_of_frames_agrees_disagrees_or_has_nothing() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let cols = LayoutGrid::new(GridAxis::Columns);
        let rows = LayoutGrid::new(GridAxis::Rows);
        let grids = |app: &OndinApp, id: NodeId| {
            ondin_core::item::values(app.session.doc.get(id).expect("a frame").grids())
        };

        let mut app = OndinApp::headless(&ctx);
        let frames = frames_with_lists(&mut app, &[vec![], vec![], vec![]]);
        let (said, collapsed) = grid_panel_pass(&ctx, &mut app, false);
        assert!(collapsed, "an empty panel should rest closed");
        // Opened by hand it stays open: `sync_paint_collapse` re-asserts only on a fresh subject and the empty transition.
        assert!(
            !said.iter().any(|t| t == "No layout grid"),
            "a closed card drew its body: {said:?}"
        );
        app.collapsed_panels.remove("Layout grid");
        let (said, _) = grid_panel_pass(&ctx, &mut app, false);
        assert!(
            said.iter().any(|t| t == "No layout grid"),
            "the opened empty card said {said:?}"
        );
        app.collapsed_panels.insert("Layout grid");

        let (_, collapsed) = grid_panel_pass(&ctx, &mut app, true);
        assert!(!collapsed, "the + left the panel shut");
        for f in &frames {
            assert_eq!(grids(&app, *f).len(), 1, "a frame missed the +");
        }

        // 2. All three agree: the card shows the rows, and an edit reaches all of them.
        let mut app = OndinApp::headless(&ctx);
        let two = vec![cols, rows];
        let frames = frames_with_lists(&mut app, &[two.clone(), two.clone(), two.clone()]);
        let subjects = app.frame_subjects();
        assert_eq!(subjects, frames, "the rectangle is a subject");
        assert_eq!(app.shared_grids(&subjects), Some(two.clone()));
        let (said, collapsed) = grid_panel_pass(&ctx, &mut app, false);
        assert!(!collapsed, "a panel with grids in it rested closed");
        assert!(
            !said.iter().any(|t| t.starts_with("Mixed")),
            "an agreeing selection reported a disagreement: {said:?}"
        );
        app.write_grids(&subjects, &[cols]);
        for f in &frames {
            assert_eq!(grids(&app, *f), vec![cols], "an edit missed a frame");
        }

        // 3. One frame differs: open, says so, no rows; the `+` replaces.
        let mut app = OndinApp::headless(&ctx);
        let frames = frames_with_lists(&mut app, &[vec![cols], vec![cols], vec![rows]]);
        let (said, collapsed) = grid_panel_pass(&ctx, &mut app, false);
        assert!(!collapsed, "a disagreement must be visible to be explained");
        assert!(
            said.iter()
                .any(|t| t == "Mixed layout grids. Add to override."),
            "the mixed card said {said:?}"
        );
        let (_, collapsed) = grid_panel_pass(&ctx, &mut app, true);
        assert!(!collapsed);
        for f in &frames {
            assert_eq!(
                grids(&app, *f).len(),
                1,
                "the + over a disagreement appended instead of overriding"
            );
        }
        assert!(
            app.shared_grids(&app.frame_subjects()).is_some(),
            "the + left the frames still disagreeing, which is the dead end it \
             exists to avoid"
        );

        let rect = *app.session.selection.ids().last().expect("the rectangle");
        assert!(
            app.session
                .doc
                .get(rect)
                .expect("a rect")
                .grids()
                .is_empty(),
            "a layer with no authored box was given a layout grid"
        );
    }

    /// Over several frames a template lights only where they agree, and applying one sets each
    /// frame's own box (Transform's W and H resize the set as a rigid body).
    #[test]
    fn a_template_over_several_frames_sets_each_of_their_own_boxes() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut app = OndinApp::headless(&ctx);
        let frames = frames_with_lists(&mut app, &[vec![], vec![], vec![]]);
        let subjects = app.frame_subjects();
        assert_eq!(subjects, frames);
        assert_eq!(
            app.shared_frame_size(&subjects),
            Some(Size::new(393.0, 852.0)),
            "the fixture's frames do not agree, so nothing below is about agreement"
        );

        app.session
            .try_commit(Transaction(vec![Operation::SetGeometry {
                id: frames[1],
                geometry: GeometryPatch::Size(Size::new(500.0, 500.0)),
            }]))
            .expect("one frame resized");
        assert_eq!(
            app.shared_frame_size(&subjects),
            None,
            "one frame off the size still reported a shared one"
        );

        let ipad = Size::new(834.0, 1194.0);
        app.apply_frame_template(&subjects, ipad);
        for (i, f) in frames.iter().enumerate() {
            let Some(NodeKind::Artboard { size, .. }) = app.session.doc.get(*f).map(|n| n.kind())
            else {
                panic!("frame {i} stopped being a frame");
            };
            assert_eq!(*size, ipad, "frame {i} kept its old size");
        }
        assert_eq!(app.shared_frame_size(&subjects), Some(ipad));
        assert_eq!(preset_named(ipad), Some("iPad Pro 11\""));

        let union = app.multi_union().expect("a union");
        assert!(
            union.width() > ipad.width,
            "the frames were scaled into one template-sized box between them \
             ({} wide) rather than each being set to {}",
            union.width(),
            ipad.width
        );
    }

    /// A grid's colour all the way round: read, written, previewed, and carried off a removed row.
    /// Four `PaintSlot` seams with no drawing. `grids[0]` is asserted every time, since checking
    /// only grid 1 passes for a write that colours every entry.
    #[test]
    fn a_grids_colour_reads_writes_and_previews_through_its_own_slot() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let id = frame_with_grids(&mut app, 2);
        let slot = PaintSlot::Grid(1);
        let default = LayoutGrid::new(GridAxis::Rows).color;
        let colour = |b: Option<Brush>| super::paint::stops_of(&b.expect("a brush"))[0].1;
        let grids = |app: &OndinApp| {
            ondin_core::item::values(app.session.doc.get(id).expect("the frame").grids())
        };

        assert_eq!(colour(app.slot_brush(id, slot)), default);
        assert!(
            app.slot_brush(id, PaintSlot::Grid(2)).is_none(),
            "a slot past the end of the list must read back as gone — that is what \
             closes a picker left open on a removed grid"
        );

        let red = ondin_core::peniko::Color::from_rgba8(220, 40, 40, 128);
        app.write_slot(id, slot, Brush::Solid(red));
        let after = grids(&app);
        assert_eq!(
            after.len(),
            2,
            "the write replaced the list rather than a row"
        );
        assert_eq!(after[1].color, red);
        assert_eq!(
            after[0].color, default,
            "the write reached the other grid too"
        );
        assert_eq!(
            after[1],
            LayoutGrid {
                color: red,
                ..LayoutGrid::new(GridAxis::Rows)
            },
            "the write moved something other than the colour"
        );

        // A drag previews into the canvas's buffer; the document keeps the committed colour.
        let blue = ondin_core::peniko::Color::from_rgba8(40, 90, 220, 64);
        let tx = app.slot_transaction(id, slot, Brush::Solid(blue));
        app.preview_grid(&tx);
        assert_eq!(app.in_flight.grid_paint, Some((vec![id], 1, blue)));
        assert_eq!(
            grids(&app)[1].color,
            red,
            "the preview committed itself to the document"
        );
        assert_eq!(
            colour(app.slot_brush(id, slot)),
            blue,
            "the row went on showing the committed colour through a drag"
        );

        // A non-colour transaction must leave this buffer alone, or the picker and `grid_scrub` fight over one grid.
        app.in_flight.grid_paint = None;
        let mut list = app.session.doc.get(id).expect("the frame").grids().to_vec();
        list[1].count = 3;
        app.preview_grid(&Transaction(vec![Operation::SetLayoutGrids {
            id,
            grids: list,
        }]));
        assert!(
            app.in_flight.grid_paint.is_none(),
            "a field edit wrote into the picker's preview buffer"
        );

        // A picker on grid 1 follows it to 0 when grid 0 goes, and is dropped when its own row goes.
        app.picker = Some(Picker::new(id, PaintSlot::Grid(1), egui::pos2(0.0, 0.0)));
        app.remap_grid_slots(id, 0);
        assert_eq!(
            app.picker.as_ref().map(|p| p.slot),
            Some(PaintSlot::Grid(0)),
            "the picker stayed on index 1 and is now editing a different grid"
        );
        app.remap_grid_slots(id, 0);
        assert!(
            app.picker.is_none(),
            "the picker outlived the row it was opened on"
        );
    }

    /// The Frame templates panel lights the frame's own size and nothing else, counted off the lit
    /// ground (a resting cell paints none).
    #[test]
    fn the_templates_panel_lights_the_frames_own_size_and_nothing_else() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
            ..Default::default()
        };
        let lit = |size: Size| {
            let mut app = OndinApp::headless(&ctx);
            // Opened by hand: the panel starts closed.
            app.collapsed_panels.remove("Frame templates");
            let id = frame_with_grids(&mut app, 0);
            app.session
                .try_commit(Transaction(vec![Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::Size(size),
                }]))
                .expect("the frame to take a size");
            let mut out = ctx.run_ui(input.clone(), |ui| {
                ui.set_max_width(PANE);
                app.inspector_frame_templates(ui);
            });
            for _ in 0..3 {
                out = ctx.run_ui(input.clone(), |ui| {
                    ui.set_max_width(PANE);
                    app.inspector_frame_templates(ui);
                });
            }
            out.shapes
                .iter()
                .filter(|cs| match &cs.shape {
                    egui::Shape::Rect(r) => {
                        r.fill == color::ACCENT_900
                            && (r.rect.height() - TEMPLATE_CELL_H).abs() < 0.5
                    }
                    _ => false,
                })
                .count()
        };
        assert_eq!(
            lit(Size::new(393.0, 852.0)),
            1,
            "iPhone 16 does not light up"
        );
        assert_eq!(lit(Size::new(1080.0, 1920.0)), 1, "Story does not light up");
        assert_eq!(
            lit(Size::new(852.0, 393.0)),
            0,
            "a landscape phone lit a portrait template"
        );
        assert_eq!(lit(Size::new(321.0, 654.0)), 0, "a custom size lit a cell");
    }

    /// The vertical gap between each pair of consecutive control rows of the Layout grid panel, drawn on its own.
    fn grid_panel_gaps(n: usize) -> Vec<f32> {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut app = OndinApp::headless(&ctx);
        // The panel finds its subject through `frame_subjects`, so the fixture sets the selection.
        let _ = frame_with_grids(&mut app, n);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input.clone(), |ui| {
            ui.set_max_width(PANE);
            app.inspector_layout_grids(ui);
        });
        for _ in 0..3 {
            out = ctx.run_ui(input.clone(), |ui| {
                ui.set_max_width(PANE);
                app.inspector_layout_grids(ui);
            });
        }
        control_rows(&out)
            .windows(2)
            .map(|w| {
                let below = w[0].iter().fold(f32::MIN, |a, r| a.max(r.bottom()));
                w[1][0].top() - below
            })
            .collect()
    }

    /// One grid's five rows sit at the card's own pitch, two grids `GRID_ROW_GAP` apart; the row
    /// count is asserted too, since a missing row changes the length and not any gap.
    #[test]
    fn a_layout_grids_rows_keep_the_cards_pitch_and_two_grids_sit_twice_it_apart() {
        let one = grid_panel_gaps(1);
        assert_eq!(
            one.len(),
            4,
            "one grid draws five rows of controls — axis, colour, N/G, M/W, \
             alignment — and so four gaps, not {}",
            one.len() + 1
        );
        let two = grid_panel_gaps(2);
        assert_eq!(two.len(), 9, "two grids draw ten rows");

        // Collected and asserted once, so a regression reports every gap it moved.
        let mut wrong: Vec<String> = Vec::new();
        let mut want = |tag: &str, gaps: &[f32], join: Option<usize>| {
            for (i, g) in gaps.iter().enumerate() {
                let expect = match join == Some(i) {
                    true => ui::CARD_ROW_GAP * 2.0,
                    false => ui::CARD_ROW_GAP,
                };
                if (g - expect).abs() >= 0.26 {
                    wrong.push(format!("{tag} gap {i} is {g:.1}, wanted {expect}"));
                }
            }
        };
        want("one grid:", &one, None);
        want("two grids:", &two, Some(4));
        assert!(
            wrong.is_empty(),
            "{} gaps in the Layout grid panel are not the card's {} \
             (twice it between two grids):\n  {}",
            wrong.len(),
            ui::CARD_ROW_GAP,
            wrong.join("\n  ")
        );
    }

    /// A hidden grid's controls are inert, and its eye and its `x` are not. Asserted on egui's
    /// per-widget `enabled` flag, against the empty panel plus exactly two.
    #[test]
    fn switching_a_grid_off_leaves_only_its_eye_and_its_x_live() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
            ..Default::default()
        };
        // Counts how many of the panel's controls would answer a click after four draws.
        let live = |grids: Vec<LayoutGrid>| {
            let mut app = OndinApp::headless(&ctx);
            let id = frame_with_grids(&mut app, 0);
            if !grids.is_empty() {
                app.session
                    .try_commit(Transaction(vec![Operation::SetLayoutGrids {
                        id,
                        grids: ondin_core::keyed_by_position(grids),
                    }]))
                    .expect("grids to draw");
            }
            for _ in 0..4 {
                // Held open by hand: `sync_paint_collapse` shuts an empty panel, which would skew the baseline.
                app.collapsed_panels.remove("Layout grid");
                let _ = ctx.run_ui(input.clone(), |ui| {
                    ui.set_max_width(PANE);
                    app.inspector_layout_grids(ui);
                });
            }
            let mut n = 0;
            ctx.viewport(|vp| {
                for (_, ws) in vp.this_pass.widgets.layers() {
                    n += ws
                        .iter()
                        .filter(|w| w.enabled && w.sense.senses_click())
                        .count();
                }
            });
            n
        };
        let chrome = live(Vec::new());
        let on = live(vec![LayoutGrid::new(GridAxis::Columns)]);
        let off = live(vec![LayoutGrid {
            visible: false,
            ..LayoutGrid::new(GridAxis::Columns)
        }]);
        assert!(
            on > chrome + 2,
            "a visible grid draws only {} controls of its own",
            on - chrome
        );
        assert_eq!(
            off,
            chrome + 2,
            "a hidden grid should answer on its eye and its x alone; it answers on \
             {} controls of its own, out of the {} a visible one has",
            off - chrome,
            on - chrome
        );
    }

    /// Drawing the Layout grid panel does not repair the grid it draws: out-of-range stored
    /// values (`count` 0, negative `margin`, `size` 0.5) stay, with no undo step and no dirty flag.
    /// A symptom guard over `ui::value_field_f64`'s `clamp_existing_to_range(false)`.
    #[test]
    fn drawing_the_grid_panel_does_not_repair_the_grid_it_draws() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
            ..Default::default()
        };
        // The grid goes in before `adopt_document`, so a dirty session means the panel did it.
        let after = |grid: LayoutGrid| {
            let mut app = OndinApp::headless(&ctx);
            let id = frame_with_grids(&mut app, 0);
            let mut doc = app.session.doc.clone();
            doc.apply(&Transaction(vec![Operation::SetLayoutGrids {
                id,
                grids: ondin_core::keyed_by_position([grid]),
            }]))
            .expect("the grid to draw");
            app.session.adopt_document(doc, None);
            // Re-selected: `adopt_document` clears the selection, and an empty one draws nothing.
            app.session.selection.set_one(id);
            assert!(
                !app.session.is_dirty(),
                "fixture: the session starts clean, or `is_dirty` says nothing"
            );
            let depth0 = app.session.history.undo_depth();
            let mut drawn = 0;
            for _ in 0..3 {
                app.collapsed_panels.remove("Layout grid");
                let _ = ctx.run_ui(input.clone(), |ui| {
                    ui.set_max_width(PANE);
                    app.inspector_layout_grids(ui);
                });
                ctx.viewport(|vp| {
                    for (_, ws) in vp.this_pass.widgets.layers() {
                        drawn += ws.iter().filter(|w| w.sense.senses_click()).count();
                    }
                });
            }
            // Precondition: a card showing a grid has far more clickable controls than an empty one.
            assert!(
                drawn > 20,
                "fixture: the panel drew only {drawn} clickable controls over three \
                 frames — it is not showing a grid, and nothing below means anything"
            );
            let stored = app.session.doc.get(id).expect("the frame").grids()[0].value;
            (
                stored,
                app.session.history.undo_depth() - depth0,
                app.session.is_dirty(),
            )
        };

        let base = LayoutGrid::new(GridAxis::Columns);
        for (label, grid, read) in [
            (
                "count 0 — layout.rs: \"Zero draws nothing rather than being refused\"",
                LayoutGrid { count: 0, ..base },
                (|g: &LayoutGrid| g.count as f64) as fn(&LayoutGrid) -> f64,
            ),
            (
                "margin -8.0",
                LayoutGrid {
                    margin: -8.0,
                    ..base
                },
                |g: &LayoutGrid| g.margin,
            ),
            (
                "size 0.5",
                LayoutGrid { size: 0.5, ..base },
                |g: &LayoutGrid| g.size,
            ),
        ] {
            let want = read(&grid);
            let (stored, steps, dirty) = after(grid);
            assert_eq!(read(&stored), want, "{label}: the panel rewrote it");
            assert_eq!(steps, 0, "{label}: and put it in the history");
            assert!(!dirty, "{label}: and armed autosave and the crash snapshot");
        }

        // Control: an in-range grid says only that the harness reaches the panel.
        let (stored, steps, dirty) = after(base);
        assert_eq!(stored, base, "control: an in-range grid is untouched");
        assert_eq!(steps, 0);
        assert!(!dirty);
    }
}

/// The two rules the Frame templates panel and the layout-grid rows are made of, checked without drawing.
#[cfg(test)]
mod frame_grid_tests {
    use super::*;

    /// Every template resolves to its own name, and a size that is nobody's resolves to nothing.
    #[test]
    fn every_preset_names_itself_and_anything_else_is_custom() {
        for (_, entries) in FRAME_PRESETS {
            for (name, w, h) in *entries {
                assert_eq!(
                    preset_named(Size::new(*w, *h)),
                    Some(*name),
                    "{name} does not answer to its own size"
                );
            }
        }
        assert_eq!(preset_named(Size::new(123.0, 456.0)), None);
        // A preset turned on its side is not that preset (either order would collide Full HD
        // and Story).
        assert_eq!(preset_named(Size::new(852.0, 393.0)), None);
        // The tolerance is small: a handle-dragged 393.2 is still an iPhone, a 394 is not.
        assert_eq!(preset_named(Size::new(393.2, 852.0)), Some("iPhone 16"));
        assert_eq!(preset_named(Size::new(394.0, 852.0)), None);
    }

    /// The grid scrub's state machine. The cancel case is asserted with `changed` set: egui can
    /// still report that frame's motion when a right-click lands.
    #[test]
    fn a_grid_scrub_holds_while_dragged_commits_once_and_obeys_a_cancel() {
        let base = LayoutGrid::new(GridAxis::Columns);
        let moved = LayoutGrid { count: 7, ..base };
        let input = |cancelled, dragged, changed, was_held| GridScrub {
            cancelled,
            dragged,
            changed,
            was_held,
        };

        assert_eq!(
            grid_step(input(true, false, true, true), moved, base),
            GridStep::Nothing,
            "a cancelled frame commits nothing, even carrying a change"
        );
        assert_eq!(
            grid_step(input(false, true, true, false), moved, base),
            GridStep::Hold,
            "a live drag holds"
        );
        assert_eq!(
            grid_step(input(false, false, false, true), moved, base),
            GridStep::Commit(moved),
            "and the release is recognised by the buffer, not by a drag-stopped flag"
        );
        assert_eq!(
            grid_step(input(false, false, true, false), moved, base),
            GridStep::Commit(moved),
            "a typed number commits without ever having been held"
        );
        assert_eq!(
            grid_step(input(false, false, true, true), base, base),
            GridStep::Nothing,
            "a scrub that ended where it started is not an edit"
        );
    }

    /// The alignment glyphs are distinct within an axis, so the four-cell strip reads without
    /// labels; they are not required to differ across axes (*Stretch* is the same arrows turned).
    #[test]
    fn each_axis_gets_four_distinct_alignment_glyphs() {
        for axis in GridAxis::ALL {
            let glyphs: Vec<&str> = GridAlign::ALL
                .iter()
                .map(|a| align_glyph(*a, axis))
                .collect();
            for (i, g) in glyphs.iter().enumerate() {
                assert!(
                    !glyphs[i + 1..].contains(g),
                    "{:?} reuses a glyph on {axis:?}: {glyphs:?}",
                    GridAlign::ALL[i]
                );
            }
        }
        assert_ne!(
            axis_glyph(GridAxis::Columns),
            axis_glyph(GridAxis::Rows),
            "the axis switch's two cells must differ"
        );
    }
}

#[cfg(test)]
mod mixed_opacity_tests {
    //! A Mixed opacity field the user has not moved writes nothing: `paint_row` seeds its `pct`
    //! from `Spread::base()`, and `edit_valve` commits on the transition a bare click produces.
    //! An agreeing selection is the control.

    use crate::app::OndinApp;
    use crate::panels::paint::PaintList;
    use crate::theme;
    use eframe::egui;
    use ondin_core::peniko::Color;
    use ondin_core::{
        Brush, Document, Fill, IdSource, NodeId, NodeKind, Operation, Transaction, kurbo::Size,
    };

    /// Two rects, one solid fill each, at the two given alphas, both selected.
    fn two_rects(ctx: &egui::Context, alphas: (f32, f32)) -> (OndinApp, NodeId, NodeId) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x0A1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (a, b) = (ids.mint(), ids.mint());
        let rect = |id| Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        };
        let paint = |id, alpha: f32| Operation::SetFills {
            id,
            fills: ondin_core::keyed_by_position([Fill {
                brush: Brush::Solid(Color::new([1.0, 0.0, 0.0, alpha])).into(),
                visible: true,
            }]),
        };
        doc.apply(&Transaction(vec![
            rect(a),
            rect(b),
            paint(a, alphas.0),
            paint(b, alphas.1),
        ]))
        .expect("build");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![a, b]);
        (app, a, b)
    }

    /// The alpha of `id`'s first fill.
    fn alpha_of(app: &OndinApp, id: NodeId) -> f32 {
        match app.session.doc.get(id).expect("node").paint().fills[0]
            .brush
            .literal()
        {
            Some(Brush::Solid(c)) => c.components[3],
            other => panic!("the fixture's fill is a solid colour, got {other:?}"),
        }
    }

    /// Draw the mixed row, click into its opacity field, and finish the gesture. In a 320-wide row
    /// the field spans about x = 200 to 255, the `x` 260 to 285, the eye 290 to 315.
    fn click_the_opacity_field(ctx: &egui::Context, app: &mut OndinApp) {
        click_at(ctx, app, egui::pos2(225.0, 14.0));
    }

    fn click_at(ctx: &egui::Context, app: &mut OndinApp, at: egui::Pos2) {
        let pass = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::pos2(0.0, 0.0),
                        egui::vec2(320.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| app.mixed_paint_row(ui, PaintList::Fill),
            );
            let _ = out;
        };
        pass(app, Vec::new());
        let key = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        pass(app, vec![egui::Event::PointerMoved(at), key(true)]);
        pass(app, vec![key(false)]);
        // `edit_valve` commits on the `!engaged && was_engaged` transition, so only `Enter` finishes the gesture.
        pass(
            app,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        pass(app, Vec::new());
        pass(app, Vec::new());
    }

    /// A bare click through a mixed opacity field writes nothing; the agreeing control shows the
    /// harness reaches the field. The `Enter` is load-bearing.
    #[test]
    fn a_bare_click_through_a_mixed_opacity_field_writes_nothing() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = two_rects(&ctx, (1.0, 0.4));
        click_the_opacity_field(&ctx, &mut app);
        assert_eq!(
            app.session.doc.get(a).expect("node").paint().fills.len(),
            1,
            "fixture: the aim point must be the opacity field and not the row's `x`, \
             which clears every fill"
        );
        assert_eq!(
            (alpha_of(&app, a), alpha_of(&app, b)),
            (1.0, 0.4),
            "a click that typed nothing and dragged nothing must leave both alphas"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            0,
            "and must not be an undo step"
        );

        // The control: the same events over an agreeing selection.
        let ctx = egui::Context::default();
        let (mut app, a, b) = two_rects(&ctx, (0.4, 0.4));
        click_the_opacity_field(&ctx, &mut app);
        assert_eq!((alpha_of(&app, a), alpha_of(&app, b)), (0.4, 0.4));
        assert_eq!(app.session.history.undo_depth(), 0);
    }

    /// The mixed row's `x` clears the paint on every target (and drops the picker), and its eye
    /// hides it. One test, since the aim points are close and each confirms the other's.
    #[test]
    fn the_mixed_rows_x_clears_every_fill_and_its_eye_hides_them() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = two_rects(&ctx, (1.0, 0.4));
        let fills =
            |app: &OndinApp, id: NodeId| app.session.doc.get(id).expect("node").paint().fills.len();
        let visible = |app: &OndinApp, id: NodeId| {
            app.session.doc.get(id).expect("node").paint().fills[0].visible
        };
        assert_eq!(
            (fills(&app, a), fills(&app, b)),
            (1, 1),
            "the fixture: one fill each"
        );
        assert!(
            visible(&app, a) && visible(&app, b),
            "the fixture: both showing"
        );

        // The eye first, because the `x` takes the fills the eye needs.
        click_at(&ctx, &mut app, egui::pos2(302.0, 14.0));
        assert!(
            !visible(&app, a) && !visible(&app, b),
            "one press of the eye hides the fill on every target in the selection"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            1,
            "as one undo step, not one per layer"
        );

        let ctx = egui::Context::default();
        let (mut app, a, b) = two_rects(&ctx, (1.0, 0.4));
        app.picker = Some(super::Picker::new(
            app.session.doc.first_page(),
            super::PaintSlot::SelectionAll(ondin_core::PaintTarget::Fill),
            egui::Pos2::ZERO,
        ));
        click_at(&ctx, &mut app, egui::pos2(272.0, 14.0));
        assert_eq!(
            (fills(&app, a), fills(&app, b)),
            (0, 0),
            "the x empties the fill list on every target — the only way back to \
             an empty panel"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            1,
            "as one undo step, not one per layer"
        );
        assert!(
            app.picker.is_none(),
            "and the picker goes with them, or it is editing a paint that is gone"
        );
    }
}

#[cfg(test)]
mod paint_hex_field_tests {
    //! What a bare click through the hex field costs.

    use super::*;

    /// `drive_exiting` by the ordinary door, which is `Enter`.
    fn drive(typed: Option<&str>) -> ([f32; 4], usize) {
        let (colour, depth, _) = drive_exiting(typed, egui::Key::Enter);
        (colour, depth)
    }

    /// Drive the field once, optionally typing `typed`, leave it by pressing `exit`, and report the
    /// colour, the undo depth and whether the buffer survived. A fresh `egui::Context` per run, or
    /// the second run's click never takes focus.
    fn drive_exiting(typed: Option<&str>, exit: egui::Key) -> ([f32; 4], usize, bool) {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = crate::app::OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0xC0);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let id = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(80.0, 40.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id,
                fills: ondin_core::keyed_by_position([ondin_core::Fill {
                    // Off the 8-bit lattice, the only kind of colour quantisation shows on.
                    brush: Brush::Solid(ondin_core::peniko::Color::new([0.5, 0.25, 0.125, 1.0]))
                        .into(),
                    visible: true,
                }]),
            },
        ]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);

        let brush = app.session.doc.get(id).unwrap().paint().fills[0]
            .brush
            .literal()
            .expect("the fixture's fill is literal")
            .clone();
        let at = egui::Pos2::new(20.0, 20.0);
        let frame = |app: &mut crate::app::OndinApp, events: Vec<egui::Event>| {
            let _ = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| app.paint_hex_field(ui, id, PaintSlot::Fill(0), &brush, false),
            );
        };
        frame(&mut app, vec![]);
        frame(&mut app, vec![egui::Event::PointerMoved(at)]);
        for pressed in [true, false] {
            frame(
                &mut app,
                vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                }],
            );
        }
        assert!(
            app.paint_hex.is_some(),
            "the click has to reach the field, or this drives nothing"
        );
        if let Some(t) = typed {
            // `select_all_on_focus` has selected the seeded text, so this replaces it.
            frame(&mut app, vec![egui::Event::Text(t.into())]);
        }
        // One more frame than `lost_focus()` is true on, so the settling frame is driven too.
        for _ in 0..3 {
            frame(
                &mut app,
                vec![egui::Event::Key {
                    key: exit,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
            );
        }
        let brush = app.session.doc.get(id).expect("a live node").paint().fills[0]
            .brush
            .literal()
            .expect("the fill stays literal");
        (
            paint::stops_of(brush)[0].1.components,
            app.session.history.undo_depth(),
            app.paint_hex.is_some(),
        )
    }

    /// A click into the hex field that types nothing changes nothing: not the colour, the undo
    /// history or the saved bytes (the text is seeded from 8-bit `ui::hex_of`, so compare bytes).
    /// The typed edit is the control.
    #[test]
    fn a_bare_click_through_the_hex_field_changes_nothing() {
        let (colour, depth) = drive(None);
        assert_eq!(
            colour,
            [0.5, 0.25, 0.125, 1.0],
            "a bare click does not change the colour"
        );
        assert_eq!(depth, 0, "and spends no undo step");

        let (typed, depth) = drive(Some("336699"));
        let want = ondin_core::peniko::Color::from_rgba8(0x33, 0x66, 0x99, 255).components;
        assert!(
            typed
                .iter()
                .zip(want.iter())
                .all(|(a, b)| (a - b).abs() < 1e-6),
            "control: a typed colour is written — got {typed:?}, wanted {want:?}"
        );
        assert_eq!(depth, 1, "control: and once, not twice");
    }

    /// A typed `RRGGBBAA` writes its alpha as the fill's opacity; a six-digit hex keeps it.
    /// flip: `typed_alpha` forced to `None` → fails at the eight-digit alpha.
    #[test]
    fn an_eight_digit_hex_sets_the_opacity_too() {
        let (colour, depth) = drive(Some("33669980"));
        assert!(
            (colour[3] - 128.0 / 255.0).abs() < 1e-6,
            "the eight-digit alpha lands — got {colour:?}"
        );
        assert!(
            (colour[0] - 0x33 as f32 / 255.0).abs() < 1e-6,
            "with its colour"
        );
        assert_eq!(depth, 1);
        let (colour, _) = drive(Some("336699"));
        assert_eq!(colour[3], 1.0, "control: a six-digit hex keeps the opacity");
        // The fixture is `[0.5, 0.25, 0.125]`, which `hex_of` rounds to `804020`: an alpha
        // typed over the same colour is still a change.
        let (colour, depth) = drive(Some("80402040"));
        assert!(
            (colour[3] - 64.0 / 255.0).abs() < 1e-6,
            "an alpha-only edit lands"
        );
        assert_eq!(depth, 1);
    }

    /// `Escape` abandons a typed colour: `lost_focus()` alone reads a cancel as a confirm, so
    /// `ui::defocus_commits` says the rule once. It drives the field alone and cannot see the
    /// `Escape` ladder in `OndinApp::escape`.
    #[test]
    fn escape_abandons_a_typed_hex_instead_of_committing_it() {
        let (colour, depth, buffered) = drive_exiting(Some("336699"), egui::Key::Escape);
        assert_eq!(
            colour,
            [0.5, 0.25, 0.125, 1.0],
            "Escape left the colour alone"
        );
        assert_eq!(depth, 0, "and spent no undo step doing it");
        assert!(
            !buffered,
            "and dropped the buffer, so the next click into the row does not \
             reopen the abandoned text"
        );
    }
}

#[cfg(test)]
mod mixed_corner_radius_tests {
    //! A bare click through the single corner-radius field does not flatten a mixed set.
    //!
    //! The field shows `largest(corners)` while the four disagree, and writing that back as
    //! `GeometryPatch::CornerRadius` would set all four; only the mixed case shows it.

    use crate::app::OndinApp;
    use crate::theme;
    use eframe::egui;
    use ondin_core::kurbo::{RoundedRectRadii, Size};
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    /// The corner-radius field's centre; a mis-aimed press writes nothing and goes green for the
    /// wrong reason. In a 320-wide card the row is y = 46 to 74, the opacity field x = 47 to 134,
    /// the radius field x = 178 to 265, the per-corner button x = 272 to 310, at every pixel scale.
    const RADIUS_FIELD: egui::Pos2 = egui::pos2(200.0, 60.0);

    /// One 80×60 rect with the given radii, selected.
    fn one_rect(ctx: &egui::Context, radii: RoundedRectRadii) -> (OndinApp, NodeId) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x0C4);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let rect = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: rect,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(80.0, 60.0),
                corner_radii: radii,
            },
            transform: None,
            name: None,
        }]))
        .expect("build");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![rect]);
        (app, rect)
    }

    fn corners_of(app: &OndinApp, id: NodeId) -> (f64, f64, f64, f64) {
        match app.session.doc.get(id).expect("the rect").kind() {
            NodeKind::Rect {
                corner_radii: r, ..
            } => (r.top_left, r.top_right, r.bottom_right, r.bottom_left),
            other => panic!("the fixture is a rect, got {other:?}"),
        }
    }

    /// One frame of the Appearance card. It reads `display_node`, not `doc`: a live drag lives in
    /// the preview, and a committed re-read would make the guard vacuous.
    fn frame(ctx: &egui::Context, app: &mut OndinApp, id: NodeId, events: Vec<egui::Event>) {
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(320.0, 600.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                // Snapshot, then drop the borrow before the `&mut self` call.
                let (opacity0, corners0) = {
                    let node = app.session.display_node(id).expect("the rect");
                    let corners = match node.kind() {
                        NodeKind::Rect { corner_radii, .. } => Some(*corner_radii),
                        _ => None,
                    };
                    (node.opacity(), corners)
                };
                app.inspector_appearance(ui, id, opacity0, corners0, None, None);
            },
        );
    }

    fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    fn enter() -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }
    }

    /// Press, release, `Enter`, settle; the `Enter` is load-bearing, as in `click_the_opacity_field`.
    fn bare_click(ctx: &egui::Context, app: &mut OndinApp, id: NodeId) {
        frame(ctx, app, id, Vec::new());
        frame(
            ctx,
            app,
            id,
            vec![
                egui::Event::PointerMoved(RADIUS_FIELD),
                button(RADIUS_FIELD, true),
            ],
        );
        frame(ctx, app, id, vec![button(RADIUS_FIELD, false)]);
        frame(ctx, app, id, vec![enter()]);
        frame(ctx, app, id, Vec::new());
        frame(ctx, app, id, Vec::new());
    }

    /// The same field, moved `dx` points right over six steps, then released. `dx` must be at
    /// least 8: below egui's drag threshold the press is a bare click.
    fn drag(ctx: &egui::Context, app: &mut OndinApp, id: NodeId, dx: f32) {
        frame(ctx, app, id, Vec::new());
        frame(
            ctx,
            app,
            id,
            vec![
                egui::Event::PointerMoved(RADIUS_FIELD),
                button(RADIUS_FIELD, true),
            ],
        );
        for i in 1..=6 {
            let at = RADIUS_FIELD + egui::vec2(dx * i as f32 / 6.0, 0.0);
            frame(ctx, app, id, vec![egui::Event::PointerMoved(at)]);
        }
        let end = RADIUS_FIELD + egui::vec2(dx, 0.0);
        frame(ctx, app, id, vec![button(end, false)]);
        frame(ctx, app, id, Vec::new());
    }

    /// A bare click through a mixed corner-radius field writes nothing; the drag control shows the aim is right.
    #[test]
    fn a_bare_click_through_a_mixed_corner_radius_field_writes_nothing() {
        let ctx = egui::Context::default();
        let (mut app, id) = one_rect(&ctx, RoundedRectRadii::new(10.0, 0.0, 0.0, 0.0));
        bare_click(&ctx, &mut app, id);
        assert_eq!(
            corners_of(&app, id),
            (10.0, 0.0, 0.0, 0.0),
            "a click that typed nothing and dragged nothing must leave all four \
             corners — the field's seed is a drag's starting point, not an edit"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            0,
            "and must not be an undo step"
        );
        assert!(
            !app.session.is_dirty(),
            "nor an autosave and a crash-snapshot write"
        );

        let ctx = egui::Context::default();
        let (mut app, id) = one_rect(&ctx, RoundedRectRadii::new(10.0, 0.0, 0.0, 0.0));
        drag(&ctx, &mut app, id, 60.0);
        assert_eq!(
            corners_of(&app, id),
            (25.0, 25.0, 25.0, 25.0),
            "control: a drag is an edit and still sets all four — the aim point \
             is the radius field and the guard has not turned it off"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            1,
            "control: as one undo step, on the release frame"
        );
    }

    /// An agreeing set is untouched either way (`changes_nothing` absorbs the write): a control only.
    #[test]
    fn a_bare_click_through_an_agreeing_corner_radius_field_was_always_clean() {
        let ctx = egui::Context::default();
        let (mut app, id) = one_rect(&ctx, RoundedRectRadii::from_single_radius(10.0));
        bare_click(&ctx, &mut app, id);
        assert_eq!(corners_of(&app, id), (10.0, 10.0, 10.0, 10.0));
        assert_eq!(app.session.history.undo_depth(), 0);
    }

    /// A mixed field shows the largest of the four, the seed a drag counts from (a seed of `0`
    /// would drop every corner to nothing). Two fixtures use different corners.
    #[test]
    fn a_mixed_corner_radius_field_shows_the_largest_of_the_four() {
        let ctx = egui::Context::default();

        let shown = |radii: RoundedRectRadii| -> Vec<String> {
            let (mut app, id) = one_rect(&ctx, radii);
            let mut out = Vec::new();
            for _ in 0..2 {
                out = galleys(&ctx, &mut app, id);
            }
            out
        };

        let one_corner = shown(RoundedRectRadii::new(10.0, 0.0, 0.0, 0.0));
        assert!(
            one_corner.iter().any(|t| t == "10"),
            "a rect with one 10pt corner reads 10, in {one_corner:?}"
        );

        let far_corner = shown(RoundedRectRadii::new(0.0, 0.0, 7.0, 0.0));
        assert!(
            far_corner.iter().any(|t| t == "7"),
            "and one with a 7pt bottom-right reads 7 — the largest, whichever \
             corner holds it, in {far_corner:?}"
        );
    }

    /// Every galley one frame of the Appearance card paints, recursively (an `egui::Frame` paints
    /// as a `Shape::Vec`).
    fn galleys(ctx: &egui::Context, app: &mut OndinApp, id: NodeId) -> Vec<String> {
        fn text_of(shape: &egui::Shape, into: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(t) => into.push(t.galley.text().to_string()),
                egui::Shape::Vec(v) => v.iter().for_each(|s| text_of(s, into)),
                _ => {}
            }
        }
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(320.0, 600.0),
                )),
                ..Default::default()
            },
            |ui| {
                let (opacity0, corners0) = {
                    let node = app.session.display_node(id).expect("the rect");
                    let corners = match node.kind() {
                        NodeKind::Rect { corner_radii, .. } => Some(*corner_radii),
                        _ => None,
                    };
                    (node.opacity(), corners)
                };
                app.inspector_appearance(ui, id, opacity0, corners0, None, None);
            },
        );
        let mut v = Vec::new();
        for cs in &out.shapes {
            text_of(&cs.shape, &mut v);
        }
        v
    }
}

#[cfg(test)]
mod dash_list_field_tests {
    //! Opening the custom dash list and closing it must not rewrite the numbers. Focus is taken
    //! with `request_focus`/`surrender_focus` on the field's explicit id.

    use super::{dash_list_text, parse_dash_list};
    use crate::app::OndinApp;
    use crate::theme;
    use eframe::egui;
    use ondin_core::Stroke;

    fn field_id() -> egui::Id {
        egui::Id::new(("stroke-dash-list", 0_usize))
    }

    /// Draw the dash field one frame, `focused` deciding whether it holds the keyboard.
    fn frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        s: &Stroke,
        focused: bool,
        typed: Option<&str>,
    ) -> Option<Stroke> {
        if focused {
            ctx.memory_mut(|m| m.request_focus(field_id()));
        } else {
            ctx.memory_mut(|m| m.surrender_focus(field_id()));
        }
        let mut out = None;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(320.0, 600.0),
                )),
                ..Default::default()
            },
            |ui| {
                if let Some(t) = typed {
                    app.stroke_dash_text = Some((0, t.to_string()));
                }
                out = app.dash_list_field(ui, 260.0, 24.0, 0, s);
            },
        );
        out
    }

    fn app_and_stroke(ctx: &egui::Context, dashes: Vec<f64>) -> (OndinApp, Stroke) {
        theme::install(ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        (
            OndinApp::headless(ctx),
            Stroke {
                dashes,
                width: 2.0,
                ..Default::default()
            },
        )
    }

    /// A click in and out with nothing typed writes nothing, even where the printed text is lossy.
    ///
    /// flip: dropping the `text != dash_list_text(&s.dashes)` term → fails at the first `assert_eq!` after the frames.
    #[test]
    fn opening_the_dash_list_and_closing_it_does_not_round_the_numbers() {
        let ctx = egui::Context::default();
        let (mut app, s) = app_and_stroke(&ctx, vec![5.000_000_000_1, 10.0]);

        // The fixture's printed text does not parse back to the value it came from.
        let printed = dash_list_text(&s.dashes);
        assert_eq!(printed, "5, 10");
        assert_ne!(parse_dash_list(&printed), Some(s.dashes.clone()));

        // Staged, not typed: `request_focus` before a frame leaves `gained_focus` false.
        assert_eq!(frame(&ctx, &mut app, &s, true, None), None, "the click in");
        let out = frame(&ctx, &mut app, &s, false, Some(&printed));
        assert!(
            app.stroke_dash_text.is_none(),
            "fixture: the field reported lost_focus and cleared its buffer"
        );
        assert_eq!(out, None, "and it wrote nothing back");
    }

    /// The control: a typed change still lands.
    #[test]
    fn a_dash_list_the_user_retypes_still_reaches_the_stroke() {
        let ctx = egui::Context::default();
        let (mut app, s) = app_and_stroke(&ctx, vec![5.0, 10.0]);
        assert_eq!(frame(&ctx, &mut app, &s, true, None), None);
        let out = frame(&ctx, &mut app, &s, false, Some("5, 20"));
        assert_eq!(
            out.map(|w| w.dashes),
            Some(vec![5.0, 20.0]),
            "a real edit is not a no-op"
        );
    }
}

#[cfg(test)]
mod shipped_shared_readout_tests {
    //! The four `_shown` readouts the panel calls (`shown_over_fills`, `shown_over_strokes`,
    //! `shared_opacity_shown`, `shared_radius_shown`), read off `display_node`.

    use super::*;
    use crate::app::OndinApp;
    use ondin_core::peniko::Color;
    use ondin_core::{Brush, Fill, IdSource, Stroke};

    /// `radii.len()` rects, all selected, each with its own radius and opacity.
    fn app_with_rects(
        ctx: &egui::Context,
        radii: &[f64],
        opacities: &[f32],
    ) -> (OndinApp, Vec<NodeId>) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x5A);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let mut made = Vec::new();
        let mut ops = Vec::new();
        for (i, r) in radii.iter().enumerate() {
            let id = ids.mint();
            made.push(id);
            ops.push(Operation::CreateNode {
                id,
                parent: root,
                index: i,
                kind: NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(50.0, 50.0),
                    corner_radii: ondin_core::kurbo::RoundedRectRadii::from_single_radius(*r),
                },
                transform: None,
                name: None,
            });
            ops.push(Operation::SetOpacity {
                id,
                opacity: opacities[i],
            });
        }
        doc.apply(&Transaction(ops)).expect("the fixture");
        app.session.adopt_document(doc, None);
        app.session.selection.set(made.clone());
        (app, made)
    }

    /// Radii a billionth apart agree; a thousandth apart read Mixed.
    ///
    /// flip: `build::RADIUS_AGREEMENT` = `0.0` → fails at the first assertion.
    #[test]
    fn the_radius_readout_agrees_to_the_tolerance_the_panel_ships() {
        let ctx = egui::Context::default();
        theme::install(&ctx);

        let (app, _) = app_with_rects(&ctx, &[6.0, 6.0 + 1e-10], &[1.0, 1.0]);
        assert_eq!(
            app.shared_radius_shown(),
            Some(6.0),
            "a billionth apart is the same corner"
        );

        let (app, _) = app_with_rects(&ctx, &[6.0, 6.001], &[1.0, 1.0]);
        assert_eq!(
            app.shared_radius_shown(),
            None,
            "and a thousandth apart is Mixed — the control"
        );
    }

    /// The same for opacity, at its coarser tolerance (the field shows `{v:.0}%`).
    ///
    /// flip: `build::OPACITY_AGREEMENT` = `0.0` → fails at the first assertion.
    #[test]
    fn the_opacity_readout_agrees_to_the_tolerance_the_panel_ships() {
        let ctx = egui::Context::default();
        theme::install(&ctx);

        let (app, _) = app_with_rects(&ctx, &[0.0, 0.0], &[0.5, 0.5 + 1e-7]);
        assert!(
            app.shared_opacity_shown().is_some(),
            "a ten-millionth apart is the same percentage"
        );

        let (app, _) = app_with_rects(&ctx, &[0.0, 0.0], &[0.5, 0.75]);
        assert_eq!(
            app.shared_opacity_shown(),
            None,
            "and a quarter apart is Mixed — the control"
        );
    }

    /// A target with no fill disagrees with every colour (`Spread::bare`).
    ///
    /// flip: deleting `spread.bare |= fills.is_empty()` from `shown_over_fills` → fails at the third assertion.
    #[test]
    fn a_layer_with_no_fill_disagrees_with_every_colour() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let solid = |c: Color| Fill {
            brush: Brush::Solid(c).into(),
            visible: true,
        };
        let red = || solid(Color::from_rgb8(220, 30, 40));
        let blue = || solid(Color::from_rgb8(30, 40, 220));

        let paint = |app: &mut OndinApp, ids: &[NodeId], each: Vec<Vec<Fill>>| {
            let ops = ids
                .iter()
                .zip(each)
                .map(|(id, fills)| Operation::SetFills {
                    id: *id,
                    fills: ondin_core::keyed_by_position(fills),
                })
                .collect();
            app.session.doc.apply(&Transaction(ops)).expect("paint");
        };
        let brush = |app: &OndinApp| app.shown_over_fills(|f| f.brush.clone()).agreed();

        let (mut app, ids) = app_with_rects(&ctx, &[0.0, 0.0], &[1.0, 1.0]);
        paint(&mut app, &ids, vec![vec![red()], vec![red()]]);
        assert!(brush(&app).is_some(), "two reds are one red");

        let (mut app, ids) = app_with_rects(&ctx, &[0.0, 0.0], &[1.0, 1.0]);
        paint(&mut app, &ids, vec![vec![red()], vec![blue()]]);
        assert!(brush(&app).is_none(), "red and blue are Mixed");

        let (mut app, ids) = app_with_rects(&ctx, &[0.0, 0.0], &[1.0, 1.0]);
        paint(&mut app, &ids, vec![vec![red()], vec![]]);
        assert!(
            brush(&app).is_none(),
            "a layer with no fill has not got that colour"
        );
    }

    /// The stroke twin: a layer with no stroke does not share the other's width.
    ///
    /// flip: deleting `spread.bare |= …strokes.is_empty()` from `shown_over_strokes` → fails at the second width assertion.
    #[test]
    fn a_layer_with_no_stroke_has_not_got_the_others_width() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let four = Stroke {
            width: 4.0,
            ..Stroke::default()
        };
        let width = |app: &OndinApp| app.shown_over_strokes(|s| s.width).agreed();

        let (mut app, ids) = app_with_rects(&ctx, &[0.0, 0.0], &[1.0, 1.0]);
        app.session
            .doc
            .apply(&Transaction(vec![
                Operation::SetStrokes {
                    id: ids[0],
                    strokes: ondin_core::keyed_by_position([four.clone()]),
                },
                Operation::SetStrokes {
                    id: ids[1],
                    strokes: ondin_core::keyed_by_position([four.clone()]),
                },
            ]))
            .expect("stroke both");
        assert_eq!(width(&app), Some(4.0), "two 4pt strokes are one width");

        app.session
            .doc
            .apply(&Transaction(vec![Operation::SetStrokes {
                id: ids[1],
                strokes: vec![],
            }]))
            .expect("bare the second");
        assert_eq!(
            width(&app),
            None,
            "and one of them having none is not 4pt either"
        );
    }
}

/// The inspector subjects other than a painted layer (guides, point editing, the empty canvas),
/// driven through `inspector_ui`. Unreached: `points_transform`'s increment form and
/// `guide_actions`' remove-all/remove precedence.
#[cfg(test)]
mod subject_tests {
    use super::*;
    use crate::app::OndinApp;
    use ondin_core::kurbo::Size;
    use ondin_core::{Guide, GuideAxis, GuideId};

    const PANE: f32 = 296.0;

    /// Every galley a pass painted, recursively (an `egui::Frame` paints as a `Shape::Vec`).
    fn text_of(shape: &egui::Shape, into: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(t) => into.push(t.galley.text().to_string()),
            egui::Shape::Vec(v) => v.iter().for_each(|s| text_of(s, into)),
            _ => {}
        }
    }

    /// One painted rect and two guides (vertical at 40, horizontal at 90), nothing selected.
    fn stage(ctx: &egui::Context) -> (OndinApp, NodeId, GuideId, GuideId) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(0x6D1);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let rect = ids.mint();
        let (v, h) = (GuideId(ids.mint()), GuideId(ids.mint()));
        let guide = |id, axis, position| Operation::AddGuide {
            guide: Guide {
                id,
                axis,
                position,
                color: None,
                owner: root,
            },
        };
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: rect,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 100.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id: rect,
                fills: ondin_core::keyed_by_position([ondin_core::Fill {
                    brush: ondin_core::peniko::Brush::Solid(ondin_core::peniko::Color::from_rgb8(
                        200, 90, 60,
                    ))
                    .into(),
                    visible: true,
                }]),
            },
            guide(v, GuideAxis::Vertical, 40.0),
            guide(h, GuideAxis::Horizontal, 90.0),
        ]))
        .expect("a rect and two guides");
        app.session.adopt_document(doc, None);
        (app, rect, v, h)
    }

    /// Four passes of the whole inspector, returning every galley painted.
    fn drawn(ctx: &egui::Context, app: &mut OndinApp) -> Vec<String> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1400.0, 2400.0),
            )),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input.clone(), |ui| {
            ui.set_max_width(PANE);
            app.inspector_ui(ui)
        });
        for _ in 0..3 {
            out = ctx.run_ui(input.clone(), |ui| {
                ui.set_max_width(PANE);
                app.inspector_ui(ui)
            });
        }
        let mut v = Vec::new();
        for cs in &out.shapes {
            text_of(&cs.shape, &mut v);
        }
        v
    }

    /// A selected guide answers the whole inspector, layer or no layer.
    ///
    /// flip: dispatching on `selection.len()` before the guide arm → fails at the "Guide" assertion (the canvas panel is drawn).
    #[test]
    fn a_selected_guide_answers_the_whole_inspector() {
        let ctx = egui::Context::default();

        let (mut app, rect, v, _h) = stage(&ctx);
        app.session.selection.set(vec![rect]);
        let layer = drawn(&ctx, &mut app);
        assert!(
            layer.iter().any(|t| t == "FILL"),
            "the fixture: a selected rect draws a Fill card, in {layer:?}"
        );

        let (mut app, _rect, v2, _h2) = stage(&ctx);
        app.session.selection.toggle_guide(v2);
        let guide = drawn(&ctx, &mut app);
        assert!(
            guide.iter().any(|t| t == "Guide"),
            "the guide's identity card, in {guide:?}"
        );
        assert!(
            !guide.iter().any(|t| t == "FILL"),
            "and no layer panel underneath it, in {guide:?}"
        );
        assert!(
            guide.iter().any(|t| t == "X"),
            "a vertical guide's position is an X, in {guide:?}"
        );
        assert!(
            !guide.iter().any(|t| t == "Y"),
            "and only the axis that has guides on it gets a field, in {guide:?}"
        );
        let _ = v;
    }

    /// Two guides on one axis show their number when they agree and `Mixed` when they do not.
    ///
    /// flip: dropping the `.then_some` in `guide_position_fields` → fails at the `Mixed` assertion.
    #[test]
    fn two_guides_on_one_axis_say_their_number_only_while_they_agree() {
        let ctx = egui::Context::default();

        let (mut app, _rect, v, _h) = stage(&ctx);
        let v2 = GuideId(ondin_core::IdSource::new(0x9000).mint());
        app.session
            .doc
            .apply(&Transaction(vec![Operation::AddGuide {
                guide: Guide {
                    id: v2,
                    axis: GuideAxis::Vertical,
                    position: 40.0,
                    color: None,
                    owner: app.session.page,
                },
            }]))
            .expect("a second vertical guide, in the same place");
        app.session.selection.toggle_guide(v);
        app.session.selection.toggle_guide(v2);
        let agreeing = drawn(&ctx, &mut app);
        assert!(
            agreeing.iter().any(|t| t == "2 Guides"),
            "the fixture: two of them, in {agreeing:?}"
        );
        assert!(
            agreeing.iter().any(|t| t == "40"),
            "two guides in one place read as that place, in {agreeing:?}"
        );
        assert!(
            !agreeing.iter().any(|t| t == "Mixed"),
            "and are not Mixed, in {agreeing:?}"
        );

        app.session
            .doc
            .apply(&Transaction(vec![Operation::SetGuidePosition {
                id: v2,
                position: 41.0,
            }]))
            .expect("move one of them");
        let apart = drawn(&ctx, &mut app);
        assert!(
            apart.iter().any(|t| t == "Mixed"),
            "a point apart is Mixed, and the word rather than a dash, in {apart:?}"
        );
    }

    /// A guide on the default and one naming that default agree: the comparison is the drawn colour.
    ///
    /// flip: `g.color()` → `g.color` in `shared_guide_colour` → fails at the second assertion.
    #[test]
    fn two_guides_agree_about_a_colour_one_of_them_names_and_the_other_inherits() {
        let default = ondin_core::DEFAULT_GUIDE_COLOR;
        let other = ondin_core::peniko::Color::from_rgb8(9, 200, 30);
        let g = |i: u64, color| Guide {
            id: GuideId(ondin_core::IdSource::new(i).mint()),
            axis: GuideAxis::Vertical,
            position: 0.0,
            color,
            owner: ondin_core::NodeId::from_wire("1:1").unwrap(),
        };

        assert_eq!(
            super::shared_guide_colour(&[g(1, None), g(2, None)]),
            Some(default),
            "two guides on the default agree about it"
        );
        assert_eq!(
            super::shared_guide_colour(&[g(1, None), g(2, Some(default))]),
            Some(default),
            "and so do one on it and one that names it"
        );
        assert_eq!(
            super::shared_guide_colour(&[g(1, None), g(2, Some(other))]),
            None,
            "two visibly different guides are Mixed"
        );
        assert_eq!(
            super::shared_guide_colour(&[]),
            None,
            "and an empty set has no colour to share"
        );
    }

    /// A position drag whose axis is no longer on screen is dropped (a guide deleted or deselected mid-drag).
    ///
    /// flip: deleting the `if let` block in `guide_position_fields` → fails at the first assertion.
    #[test]
    fn a_position_drag_loses_its_target_when_the_axis_it_was_on_goes_away() {
        let ctx = egui::Context::default();

        let (mut app, _rect, v, h) = stage(&ctx);
        app.session.selection.toggle_guide(v);
        app.in_flight.guide_position_drag = Some((GuideAxis::Horizontal, 999.0));
        drawn(&ctx, &mut app);
        assert!(
            app.in_flight.guide_position_drag.is_none(),
            "only a vertical guide is selected, so the horizontal target is stranded"
        );

        let (mut app, _rect, _v, h2) = stage(&ctx);
        app.session.selection.toggle_guide(h2);
        app.in_flight.guide_position_drag = Some((GuideAxis::Horizontal, 999.0));
        drawn(&ctx, &mut app);
        assert_eq!(
            app.in_flight.guide_position_drag,
            Some((GuideAxis::Horizontal, 999.0)),
            "a live drag on an axis that still has a field keeps its value"
        );
        let _ = h;
    }

    /// An empty selection draws the current page's card, not an empty panel.
    ///
    /// flip: pointing the `0 =>` arm at `inspector_multi` → fails at the page card assertion.
    #[test]
    fn an_empty_selection_draws_the_canvas_and_its_ground() {
        let ctx = egui::Context::default();
        let (mut app, _rect, _v, _h) = stage(&ctx);
        app.session.selection.clear();
        let empty = drawn(&ctx, &mut app);
        assert!(
            empty.iter().any(|t| t == "Page 1 of 1"),
            "the page's identity card, in {empty:?}"
        );
        assert!(
            !empty.iter().any(|t| t == "FILL"),
            "and no layer's Fill card, in {empty:?}"
        );
    }

    /// Arming the Node tool over a path changes the subject to its points, Align card included.
    ///
    /// flip: never taking the `edited_path` arm → fails at "Edit Path"; deleting `self.inspector_align(ui)` from `inspector_points` → fails at ALIGN.
    #[test]
    fn arming_the_node_tool_over_a_path_changes_the_subject_to_its_points() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0x9A7);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let path = ids.mint();
        let mut bez = ondin_core::kurbo::BezPath::new();
        bez.move_to((0.0, 0.0));
        bez.line_to((60.0, 0.0));
        bez.line_to((60.0, 40.0));
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: path,
            parent: root,
            index: 0,
            kind: NodeKind::Path {
                path: bez,
                corner_radii: Vec::new(),
            },
            transform: None,
            name: None,
        }]))
        .expect("a path");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![path]);

        let layer = drawn(&ctx, &mut app);
        assert!(
            !layer.iter().any(|t| t == "Edit Path"),
            "the fixture: the Select tool shows the layer, in {layer:?}"
        );

        app.tool = crate::tools::Tool::Node;
        assert_eq!(
            app.edited_path(),
            Some(path),
            "the fixture: the tool is armed on a path it will accept"
        );
        let points = drawn(&ctx, &mut app);
        assert!(
            points.iter().any(|t| t == "Edit Path"),
            "the point-editing identity card, in {points:?}"
        );
        assert!(
            points.iter().any(|t| t == "ALIGN"),
            "and Align is drawn here — the sixth card, aimed at the anchors, \
             in {points:?}"
        );
    }

    /// One frame of the guide Transform card's fields alone, returning the first one's rect: the
    /// topmost stroked box between 18 and 34 points tall.
    fn guide_frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        events: Vec<egui::Event>,
    ) -> egui::Rect {
        let guides = app.selected_guides();
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(PANE, 600.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                ui.set_max_width(PANE);
                app.guide_position_fields(ui, &guides);
            },
        );
        let mut best = egui::Rect::NOTHING;
        for cs in &out.shapes {
            if let egui::Shape::Rect(r) = &cs.shape
                && r.stroke.width > 0.0
                && r.rect.height() >= 18.0
                && r.rect.height() <= 34.0
                && r.rect.width() >= 14.0
                && (best == egui::Rect::NOTHING || r.rect.top() < best.top())
            {
                best = r.rect;
            }
        }
        best
    }

    fn click(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// A bare click through a guide's position field commits nothing; typing into it commits once.
    ///
    /// Two guards (the `!interacting` gate, `changes_nothing` absorbing a no-op `SetGuidePosition`)
    /// each suffice, so only removing both fails, at `1` against `0`.
    #[test]
    fn a_bare_click_through_a_guide_position_field_commits_nothing() {
        let ctx = egui::Context::default();
        let (mut app, _rect, v, _h) = stage(&ctx);
        app.session.selection.toggle_guide(v);

        let mut at = egui::Pos2::ZERO;
        for _ in 0..2 {
            at = guide_frame(&ctx, &mut app, Vec::new()).center();
        }
        assert!(
            at.x > 0.0 && at.y > 0.0,
            "the fixture: a field was laid out"
        );

        let depth = app.session.history.undo_depth();
        guide_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        guide_frame(&ctx, &mut app, vec![click(at, true)]);
        guide_frame(&ctx, &mut app, vec![click(at, false)]);
        for _ in 0..2 {
            guide_frame(&ctx, &mut app, Vec::new());
        }
        let away = egui::pos2(5.0, 560.0);
        guide_frame(&ctx, &mut app, vec![click(away, true)]);
        guide_frame(&ctx, &mut app, vec![click(away, false)]);
        for _ in 0..2 {
            guide_frame(&ctx, &mut app, Vec::new());
        }
        assert_eq!(
            app.session.history.undo_depth(),
            depth,
            "a click through the field and away again is not an edit"
        );

        let (mut app, _rect, v, _h) = stage(&ctx);
        app.session.selection.toggle_guide(v);
        for _ in 0..2 {
            at = guide_frame(&ctx, &mut app, Vec::new()).center();
        }
        let depth = app.session.history.undo_depth();
        guide_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        guide_frame(&ctx, &mut app, vec![click(at, true)]);
        guide_frame(&ctx, &mut app, vec![click(at, false)]);
        guide_frame(&ctx, &mut app, Vec::new());
        for ch in "77".chars() {
            guide_frame(&ctx, &mut app, vec![egui::Event::Text(ch.to_string())]);
        }
        guide_frame(&ctx, &mut app, vec![click(away, true)]);
        guide_frame(&ctx, &mut app, vec![click(away, false)]);
        for _ in 0..2 {
            guide_frame(&ctx, &mut app, Vec::new());
        }
        assert!(
            app.session.history.undo_depth() > depth,
            "the control: a typed number is an edit, so the click was on the field"
        );
        assert_eq!(
            app.session.doc.guide(v).expect("still there").position,
            77.0,
            "and it is the number that was typed"
        );
    }

    /// A cancelled gesture still lays out the field after it (`continue`, not `return`).
    ///
    /// flip: making it a `return` → fails at the Y assertion.
    #[test]
    fn a_cancelled_gesture_still_lays_out_the_field_after_it() {
        let ctx = egui::Context::default();
        let (mut app, _rect, v, h) = stage(&ctx);
        app.session.selection.toggle_guide(v);
        app.session.selection.toggle_guide(h);

        let calm = drawn(&ctx, &mut app);
        assert!(
            calm.iter().any(|t| t == "X") && calm.iter().any(|t| t == "Y"),
            "the fixture: one guide on each axis, so both fields, in {calm:?}"
        );

        app.gesture_cancelled = true;
        let cancelled = drawn(&ctx, &mut app);
        assert!(
            cancelled.iter().any(|t| t == "X"),
            "the axis the cancel was on still has its field, in {cancelled:?}"
        );
        assert!(
            cancelled.iter().any(|t| t == "Y"),
            "and so does the one after it — `continue`, not `return`, \
             in {cancelled:?}"
        );
    }

    /// Remove all takes the unselected guides too, as one undo step.
    ///
    /// flip: `remove_all_guides` iterating `self.session.selection.guides()` instead of `doc.guides()` → fails at the count.
    #[test]
    fn remove_all_guides_takes_the_unselected_ones_too() {
        let ctx = egui::Context::default();
        let (mut app, _rect, v, _h) = stage(&ctx);
        app.session.selection.toggle_guide(v);
        assert_eq!(
            app.session.doc.guides().len(),
            2,
            "the fixture: two guides, one of them selected"
        );

        let depth = app.session.history.undo_depth();
        app.remove_all_guides();
        assert_eq!(
            app.session.doc.guides().len(),
            0,
            "the unselected one goes as well"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            depth + 1,
            "and one press is one undo step, not one per guide"
        );

        let (mut app, _rect, v, _h) = stage(&ctx);
        app.session.selection.toggle_guide(v);
        app.remove_selected_guides();
        assert_eq!(
            app.session.doc.guides().len(),
            1,
            "Remove leaves the guide that was not picked"
        );
    }
}

/// The paint panels' write half: `strokes_tx` and `remap_paint_indices`, on a headless app.
#[cfg(test)]
mod paint_write_tests {
    use super::*;
    use crate::app::OndinApp;
    use ondin_core::kurbo::Size;

    /// One rect carrying `n` strokes of widths `1.0, 2.0, …`, selected; the widths identify each stroke.
    fn striped(ctx: &egui::Context, n: usize) -> (OndinApp, NodeId) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(0x5A1);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let id = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 100.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            },
            Operation::SetStrokes {
                id,
                strokes: ondin_core::keyed_by_position((0..n).map(|i| ondin_core::Stroke {
                    width: i as f64 + 1.0,
                    ..Default::default()
                })),
            },
        ]))
        .expect("a striped rect");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![id]);
        (app, id)
    }

    fn strokes(app: &OndinApp, id: NodeId) -> Vec<ondin_core::Keyed<Stroke>> {
        app.session
            .doc
            .get(id)
            .expect("node")
            .paint()
            .strokes
            .clone()
    }

    /// A fill list edited over a selection lands on each layer's own item ids.
    ///
    /// flip: `write_fill_list` handing the anchor's ids to every layer → fails at the second rect's id assertion.
    #[test]
    fn a_selection_fill_edit_keeps_each_layers_item_ids() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0x5A2);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let solid = |r: u8| Fill {
            brush: Brush::Solid(ondin_core::peniko::Color::from_rgba8(r, 0, 0, 255)).into(),
            visible: true,
        };
        let mut ops = Vec::new();
        let mut rects = Vec::new();
        for i in 0..2 {
            let id = ids.mint();
            ops.push(Operation::CreateNode {
                id,
                parent: root,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            });
            ops.push(Operation::SetFills {
                id,
                fills: vec![
                    Keyed::new(ids.mint_item(), solid(10)),
                    Keyed::new(ids.mint_item(), solid(20)),
                ],
            });
            rects.push(id);
        }
        doc.apply(&Transaction(ops)).expect("two rects");
        app.session.adopt_document(doc, None);
        app.session.selection.set(rects.clone());
        let fills_of = |app: &OndinApp, id: NodeId| {
            app.session.doc.get(id).expect("node").paint().fills.clone()
        };
        let (a, b) = (fills_of(&app, rects[0]), fills_of(&app, rects[1]));
        assert_ne!(a[1].id, b[1].id, "the fixture's rects carry different ids");

        let mut next = app.fills_in_scope(PaintScope::Selection);
        assert_eq!(
            next.len(),
            2,
            "the selection agrees, so the panel shows a list"
        );
        next.remove(0);
        app.write_fill_list(PaintScope::Selection, next);

        let (a2, b2) = (fills_of(&app, rects[0]), fills_of(&app, rects[1]));
        assert_eq!(a2.len(), 1);
        assert_eq!(a2[0].id, a[1].id, "the first rect keeps its survivor's id");
        assert_eq!(
            b2[0].id, b[1].id,
            "the second rect keeps *its* survivor's id"
        );
    }

    /// The five list writers over a main, its instance and a plain frame, selected together:
    /// removing row 0 keeps each layer's own survivor id, the `+` gives all three one id, and a
    /// longer write keeps ids by position.
    ///
    /// flip: a step-1 writer handed the anchor's ids verbatim → fails at "the plain frame's own survivor".
    #[test]
    fn every_list_writer_keeps_each_layers_ids_and_shares_an_added_one() {
        use ondin_core::{ExportFormat, ExportScale, ExportSpec, ItemId, Placement};
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0x5A3);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let [m, p] = [(); 2].map(|_| ids.mint());
        let frame = |id, index| Operation::CreateNode {
            id,
            parent: root,
            index,
            kind: NodeKind::Artboard {
                size: Size::new(100.0, 100.0),
            },
            transform: None,
            name: None,
        };
        doc.apply(&Transaction(vec![
            frame(m, 0),
            frame(p, 1),
            Operation::SetComponent {
                id: m,
                component: true,
            },
        ]))
        .expect("a main and a plain frame");
        let (tx, made) = ondin_core::insert_subtrees(
            &doc,
            &mut ids,
            &[Placement {
                nodes: doc.capture_subtree(m).unwrap(),
                parent: root,
                index: None,
            }],
            Default::default(),
        );
        doc.apply(&tx).expect("an instance");
        let i = made[0];
        app.session.adopt_document(doc, None);

        let fill = |g: u8| Fill {
            brush: Brush::Solid(ondin_core::peniko::Color::from_rgb8(10, g, 30)).into(),
            visible: true,
        };
        let stroke = |width: f64| Stroke {
            width,
            ..Default::default()
        };
        let [ea, eb, ec, ..] = EffectKind::all_defaults();
        let grid = |count| LayoutGrid {
            count,
            ..LayoutGrid::new(GridAxis::Columns)
        };
        let export = |s| ExportSpec::new(ExportFormat::Png, ExportScale::Times(s));
        // The main's items are minted, the plain frame's positional; the instance takes the main's.
        let s = &mut app.session.ids;
        let two = |s: &mut ondin_core::IdSource| [s.mint_item(), s.mint_item()];
        let [f0, f1] = two(s);
        let [s0, s1] = two(s);
        let [e0, e1] = two(s);
        let [g0, g1] = two(s);
        let [x0, x1] = two(s);
        use ondin_core::keyed_by_position as pos;
        assert!(app.session.commit(Transaction(vec![
            Operation::SetFills {
                id: m,
                fills: vec![Keyed::new(f0, fill(1)), Keyed::new(f1, fill(2))],
            },
            Operation::SetStrokes {
                id: m,
                strokes: vec![Keyed::new(s0, stroke(1.0)), Keyed::new(s1, stroke(2.0))],
            },
            Operation::SetEffects {
                id: m,
                effects: vec![
                    Keyed::new(e0, Effect::new(ea.clone())),
                    Keyed::new(e1, Effect::new(eb.clone())),
                ],
            },
            Operation::SetLayoutGrids {
                id: m,
                grids: vec![Keyed::new(g0, grid(4)), Keyed::new(g1, grid(6))],
            },
            Operation::SetExports {
                id: m,
                exports: vec![Keyed::new(x0, export(1.0)), Keyed::new(x1, export(2.0))],
            },
            Operation::SetFills {
                id: p,
                fills: pos([fill(1), fill(2)]),
            },
            Operation::SetStrokes {
                id: p,
                strokes: pos([stroke(1.0), stroke(2.0)]),
            },
            Operation::SetEffects {
                id: p,
                effects: pos([Effect::new(ea), Effect::new(eb)]),
            },
            Operation::SetLayoutGrids {
                id: p,
                grids: pos([grid(4), grid(6)]),
            },
            Operation::SetExports {
                id: p,
                exports: pos([export(1.0), export(2.0)]),
            },
        ])));
        fn ids_of(n: &ondin_core::Node) -> [Vec<ItemId>; 5] {
            fn of<T>(l: &[Keyed<T>]) -> Vec<ItemId> {
                l.iter().map(|k| k.id).collect()
            }
            [
                of(&n.paint().fills),
                of(&n.paint().strokes),
                of(n.effects()),
                of(n.grids()),
                of(n.exports()),
            ]
        }
        let read = |app: &OndinApp, id| ids_of(app.session.doc.get(id).expect("node"));
        let lists = ["fills", "strokes", "effects", "grids", "exports"];
        assert_eq!(read(&app, i), read(&app, m), "the fixture: it propagated");
        let plain = read(&app, p);
        assert!(
            plain.iter().all(|l| l.len() == 2 && !l.contains(&f1)),
            "the fixture: the plain frame's own ids"
        );
        let subjects = [m, i, p];
        app.session.selection.set(subjects.to_vec());

        let mut next = app.fills_in_scope(PaintScope::Selection);
        assert_eq!(next.len(), 2, "the selection agrees on its fills");
        next.remove(0);
        app.write_fill_list(PaintScope::Selection, next);
        let mut next = app.strokes_in_scope(PaintScope::Selection);
        assert_eq!(next.len(), 2, "… and its strokes");
        next.remove(0);
        app.write_strokes(PaintScope::Selection, next);
        let mut next = app.shared_effects(&subjects).expect("… and its effects");
        next.remove(0);
        app.write_effects(&subjects, next);
        let anchor = app.shared_grids_keyed(&subjects).expect("… and its grids");
        app.retarget_grids(&subjects, &anchor, &anchor[1..]);
        let anchor = app.session.doc.get(m).unwrap().exports().to_vec();
        app.retarget_exports(&subjects, &anchor, &anchor[1..]);
        let main = [f1, s1, e1, g1, x1];
        for (k, name) in lists.iter().enumerate() {
            assert_eq!(
                read(&app, m)[k],
                vec![main[k]],
                "the main's survivor, {name}"
            );
            assert_eq!(
                read(&app, i)[k],
                vec![main[k]],
                "the copy's survivor, {name}"
            );
            assert_eq!(
                read(&app, p)[k],
                vec![plain[k][1]],
                "the plain frame's own survivor, {name}"
            );
        }

        let mut next = app.fills_in_scope(PaintScope::Selection);
        next.push(Keyed::new(app.session.ids.mint_item(), fill(3)));
        app.write_fill_list(PaintScope::Selection, next);
        let mut next = app.strokes_in_scope(PaintScope::Selection);
        next.push(Keyed::new(app.session.ids.mint_item(), stroke(3.0)));
        app.write_strokes(PaintScope::Selection, next);
        let mut next = app.shared_effects(&subjects).expect("still agrees");
        next.push(Keyed::new(app.session.ids.mint_item(), Effect::new(ec)));
        app.write_effects(&subjects, next);
        app.append_grid(&subjects, grid(8));
        app.append_exports(&subjects, &[export(3.0)]);
        for (k, name) in lists.iter().enumerate() {
            let added = |id| read(&app, id)[k].get(1).copied();
            assert!(added(m).is_some(), "the fixture: a {name} row landed");
            assert_eq!(
                (added(i), added(p)),
                (added(m), added(m)),
                "one id for the added {name}"
            );
        }
        assert_eq!(
            ondin_core::reset::drift(&app.session.doc, i),
            Default::default(),
            "an edit made to the main and its instance together is no override"
        );

        let before = [read(&app, m), read(&app, p)];
        app.write_grids(&subjects, &[grid(1), grid(2), grid(3)]);
        let tx = app.exports_tx(&subjects, &[export(1.0), export(2.0), export(4.0)]);
        app.commit_edit(tx);
        for (k, name) in [(3, "grids"), (4, "exports")] {
            let (now_m, now_p) = (&read(&app, m)[k], &read(&app, p)[k]);
            assert_eq!(
                now_m[..2],
                before[0][k][..],
                "the main keeps its {name} ids"
            );
            assert_eq!(
                now_p[..2],
                before[1][k][..],
                "the plain frame keeps its own"
            );
            assert_eq!(read(&app, i)[k], *now_m, "the copy is the main's, {name}");
            assert_eq!(now_p[2], now_m[2], "one id past the end of {name}");
        }
        assert_eq!(
            ondin_core::reset::drift(&app.session.doc, i),
            Default::default(),
            "and still no override"
        );
    }

    /// Writing a stroke list that is already there builds no transaction, compared against the committed tree.
    ///
    /// flip: `if false && committed == …` in `strokes_tx` → fails at the first assertion.
    #[test]
    fn writing_the_stroke_list_that_is_already_there_builds_nothing() {
        let ctx = egui::Context::default();
        let (mut app, id) = striped(&ctx, 3);
        let now = strokes(&app, id);
        assert_eq!(
            now.iter().map(|s| s.width).collect::<Vec<_>>(),
            vec![1.0, 2.0, 3.0],
            "the fixture"
        );

        let tx = app.strokes_tx(PaintScope::Node(id), now.clone());
        assert!(
            tx.0.is_empty(),
            "the same list builds nothing, got {} ops",
            tx.0.len()
        );

        let mut changed = now.clone();
        changed[1].width = 20.0;
        let tx = app.strokes_tx(PaintScope::Node(id), changed.clone());
        assert_eq!(
            tx.0.len(),
            1,
            "the control: a real change still writes the list"
        );

        // Mid-drag the preview already carries the new value; comparing against it would drop the release.
        app.session.commit(tx);
        let (mut app, id) = striped(&ctx, 3);
        let preview = app.strokes_tx(PaintScope::Node(id), changed.clone());
        app.session.set_preview(&preview);
        let tx = app.strokes_tx(PaintScope::Node(id), changed);
        assert_eq!(
            tx.0.len(),
            1,
            "a value the preview already shows is still an edit to commit"
        );
    }

    /// A reorder renumbers the paint chrome (picker, hex buffer, options popup, sides rows, dash text);
    /// a removal drops the chrome naming the paint that went; a fill's reorder leaves stroke chrome alone.
    ///
    /// flip: `moved` short-circuited to `Some(slot)` → fails at the picker assertions; the `stroke_menu`/`stroke_sides_open`/`stroke_dash_text` block bypasses `moved`.
    #[test]
    fn a_reorder_carries_the_paint_chrome_and_a_removal_drops_it() {
        let ctx = egui::Context::default();

        let (mut app, id) = striped(&ctx, 3);
        app.picker = Some(Picker::new(id, PaintSlot::Stroke(0), egui::Pos2::ZERO));
        app.paint_hex = Some((id, PaintSlot::Stroke(0), "ff0000".into()));
        app.stroke_menu = Some(0);
        app.stroke_sides_open = vec![0, 2];
        app.stroke_dash_text = Some((0, "4 2".into()));
        app.remap_paint_indices(id, PaintList::Stroke, |i| match i {
            0 => Some(2),
            1 => Some(0),
            2 => Some(1),
            _ => None,
        });
        assert_eq!(
            app.picker.as_ref().map(|p| p.slot),
            Some(PaintSlot::Stroke(2)),
            "the picker follows its stroke rather than staying on the index"
        );
        assert_eq!(
            app.paint_hex.as_ref().map(|(_, s, _)| *s),
            Some(PaintSlot::Stroke(2)),
            "and so does the hex buffer being typed into"
        );
        assert_eq!(app.stroke_menu, Some(2), "and the options popup");
        assert_eq!(
            app.stroke_sides_open,
            vec![2, 1],
            "and every open sides row, each by its own new number"
        );
        assert_eq!(
            app.stroke_dash_text.as_ref().map(|(i, _)| *i),
            Some(2),
            "and the dash text mid-typing"
        );

        let (mut app, id) = striped(&ctx, 3);
        app.picker = Some(Picker::new(id, PaintSlot::Stroke(1), egui::Pos2::ZERO));
        app.paint_hex = Some((id, PaintSlot::Stroke(1), "ff0000".into()));
        app.stroke_menu = Some(1);
        app.stroke_sides_open = vec![0, 1, 2];
        app.stroke_dash_text = Some((1, "4 2".into()));
        app.remap_paint_indices(id, PaintList::Stroke, |i| match i {
            0 => Some(0),
            1 => None,
            2 => Some(1),
            _ => None,
        });
        assert!(
            app.picker.is_none(),
            "the picker on the paint that went is dropped, not left aiming at its neighbour"
        );
        assert!(app.paint_hex.is_none(), "and the hex buffer with it");
        assert_eq!(app.stroke_menu, None, "and the popup");
        assert_eq!(
            app.stroke_sides_open,
            vec![0, 1],
            "and the surviving sides rows renumber — 2 becomes 1, and 1 is gone"
        );
        assert_eq!(app.stroke_dash_text, None, "and the dash text");

        let (mut app, id) = striped(&ctx, 3);
        app.picker = Some(Picker::new(id, PaintSlot::Stroke(1), egui::Pos2::ZERO));
        app.stroke_menu = Some(1);
        app.remap_paint_indices(id, PaintList::Fill, |_| Some(9));
        assert_eq!(
            app.picker.as_ref().map(|p| p.slot),
            Some(PaintSlot::Stroke(1)),
            "a fill's renumbering leaves the stroke slots exactly as they are"
        );
        assert_eq!(
            app.stroke_menu,
            Some(1),
            "and does not reach the stroke chrome block at all"
        );
    }

    /// Chrome hanging off another node is left alone rather than renumbered.
    ///
    /// flip: dropping `.filter(|p| p.node == anchor)` → fails at the picker assertion; making the hex buffer's arm unconditional → fails at the hex assertion.
    #[test]
    fn paint_chrome_belonging_to_another_layer_is_left_where_it_is() {
        let ctx = egui::Context::default();
        let (mut app, id) = striped(&ctx, 3);
        let elsewhere = ondin_core::IdSource::new(0xBEEF).mint();

        app.picker = Some(Picker::new(
            elsewhere,
            PaintSlot::Stroke(0),
            egui::Pos2::ZERO,
        ));
        app.paint_hex = Some((elsewhere, PaintSlot::Stroke(0), "00ff00".into()));
        app.remap_paint_indices(id, PaintList::Stroke, |i| match i {
            0 => Some(2),
            _ => Some(0),
        });
        assert_eq!(
            app.picker.as_ref().map(|p| p.slot),
            Some(PaintSlot::Stroke(0)),
            "the other layer's picker is not this list's business"
        );
        assert_eq!(
            app.paint_hex.as_ref().map(|(n, s, _)| (*n, *s)),
            Some((elsewhere, PaintSlot::Stroke(0))),
            "nor is the other layer's hex buffer"
        );
    }

    /// Two rects with one stroke each, aligned as given, both selected.
    fn two_stroked(
        ctx: &egui::Context,
        aligns: (StrokeAlign, StrokeAlign),
    ) -> (OndinApp, NodeId, NodeId) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(0x5A2);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let (a, b) = (ids.mint(), ids.mint());
        let rect = |id| Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        };
        let stroked = |id, align| Operation::SetStrokes {
            id,
            strokes: ondin_core::keyed_by_position([Stroke {
                width: 2.0,
                align,
                ..Default::default()
            }]),
        };
        doc.apply(&Transaction(vec![
            rect(a),
            rect(b),
            stroked(a, aligns.0),
            stroked(b, aligns.1),
        ]))
        .expect("two stroked rects");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![a, b]);
        (app, a, b)
    }

    /// The mixed alignment dropdown lands its pick on every selected stroke, and offers exactly three rows (no `Mixed`).
    /// In a 320-wide row the combo opens at (60, 14) and its popup rows draw at y = 38.5, 66.5, 94.5 (x = 15).
    ///
    /// flip: neutering the `if let Some(a) = picked` commit → fails at the first alignment assertion.
    #[test]
    fn the_mixed_alignment_dropdown_lands_its_pick_on_every_stroke() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = two_stroked(&ctx, (StrokeAlign::Center, StrokeAlign::Inside));
        let align = |app: &OndinApp, id: NodeId| {
            app.session.doc.get(id).expect("node").paint().strokes[0].align
        };
        assert_eq!(
            (align(&app, a), align(&app, b)),
            (StrokeAlign::Center, StrokeAlign::Inside),
            "the fixture: a set that disagrees"
        );

        let pass = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::pos2(0.0, 0.0),
                        egui::vec2(320.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| app.mixed_stroke_properties(ui, PaintScope::Selection),
            );
            fn walk(s: &egui::Shape, v: &mut Vec<String>) {
                match s {
                    egui::Shape::Text(t) => v.push(t.galley.text().to_string()),
                    egui::Shape::Vec(x) => x.iter().for_each(|s| walk(s, v)),
                    _ => {}
                }
            }
            let mut v = Vec::new();
            for cs in &out.shapes {
                walk(&cs.shape, &mut v);
            }
            v
        };
        let key = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };

        let shut = pass(&mut app, Vec::new());
        assert!(
            shut.iter().any(|t| t.ends_with("Mixed")),
            "the combo reports the disagreement, in {shut:?}"
        );

        let combo = egui::pos2(60.0, 14.0);
        pass(
            &mut app,
            vec![egui::Event::PointerMoved(combo), key(combo, true)],
        );
        pass(&mut app, vec![key(combo, false)]);
        let open = pass(&mut app, Vec::new());
        let rows: Vec<&String> = open
            .iter()
            .filter(|t| ["Centre", "Inside", "Outside", "Mixed"].contains(&t.as_str()))
            .collect();
        assert_eq!(
            rows,
            vec!["Centre", "Inside", "Outside"],
            "three real answers and no Mixed, in {open:?}"
        );

        let outside = egui::pos2(60.0, 100.0);
        let depth = app.session.history.undo_depth();
        pass(
            &mut app,
            vec![egui::Event::PointerMoved(outside), key(outside, true)],
        );
        pass(&mut app, vec![key(outside, false)]);
        pass(&mut app, Vec::new());
        assert_eq!(
            (align(&app, a), align(&app, b)),
            (StrokeAlign::Outside, StrokeAlign::Outside),
            "the pick lands on every stroke in the selection"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            depth + 1,
            "as one undo step"
        );

        let ctx = egui::Context::default();
        let (mut app, a, b) = two_stroked(&ctx, (StrokeAlign::Center, StrokeAlign::Inside));
        let pass = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::pos2(0.0, 0.0),
                        egui::vec2(320.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| app.mixed_stroke_properties(ui, PaintScope::Selection),
            );
        };
        pass(&mut app, Vec::new());
        pass(
            &mut app,
            vec![egui::Event::PointerMoved(outside), key(outside, true)],
        );
        pass(&mut app, vec![key(outside, false)]);
        pass(&mut app, Vec::new());
        assert_eq!(
            (align(&app, a), align(&app, b)),
            (StrokeAlign::Center, StrokeAlign::Inside),
            "control: the row's own y = 100 is outside the card and writes nothing"
        );
    }

    /// The per-stroke alignment dropdown reports its pick back to the caller, at the mixed row's coordinates.
    ///
    /// flip: `OndinApp::align_combo` returning `None` → fails at the picked assertion.
    #[test]
    fn the_per_stroke_alignment_dropdown_lands_its_pick() {
        let ctx = egui::Context::default();
        let (mut app, a, _) = two_stroked(&ctx, (StrokeAlign::Center, StrokeAlign::Center));
        app.session.selection.set_one(a);
        let stroke = app.session.doc.get(a).expect("node").paint().strokes[0].clone();

        let pass = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let mut out = None;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::pos2(0.0, 0.0),
                        egui::vec2(320.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    out = app.stroke_properties(ui, PaintScope::Node(a), 0, &stroke);
                },
            );
            out
        };
        let key = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };

        let popup_rows = |app: &mut OndinApp| {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::pos2(0.0, 0.0),
                        egui::vec2(320.0, 600.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    app.stroke_properties(ui, PaintScope::Node(a), 0, &stroke);
                },
            );
            fn walk(s: &egui::Shape, v: &mut Vec<(String, egui::Pos2)>) {
                match s {
                    egui::Shape::Text(t) => v.push((t.galley.text().to_string(), t.pos)),
                    egui::Shape::Vec(x) => x.iter().for_each(|s| walk(s, v)),
                    _ => {}
                }
            }
            let mut v = Vec::new();
            for cs in &out.shapes {
                walk(&cs.shape, &mut v);
            }
            v.retain(|(t, _)| ["Centre", "Inside", "Outside", "Mixed"].contains(&t.as_str()));
            v
        };

        // Warm-up: egui hit-tests a press against the previous frame's rects.
        pass(&mut app, Vec::new());
        let combo = egui::pos2(60.0, 14.0);
        pass(
            &mut app,
            vec![egui::Event::PointerMoved(combo), key(combo, true)],
        );
        pass(&mut app, vec![key(combo, false)]);
        assert_eq!(
            popup_rows(&mut app),
            vec![
                ("Centre".to_string(), egui::pos2(15.0, 38.5)),
                ("Inside".to_string(), egui::pos2(15.0, 66.5)),
                ("Outside".to_string(), egui::pos2(15.0, 94.5)),
            ],
            "three real answers, no Mixed, at the mixed row's own coordinates"
        );

        let outside = egui::pos2(60.0, 100.0);
        pass(
            &mut app,
            vec![egui::Event::PointerMoved(outside), key(outside, true)],
        );
        let picked = pass(&mut app, vec![key(outside, false)]);
        assert_eq!(
            picked.map(|s| s.align),
            Some(StrokeAlign::Outside),
            "the pick is reported back to the caller"
        );

        let ctx = egui::Context::default();
        let (mut app, a, _) = two_stroked(&ctx, (StrokeAlign::Center, StrokeAlign::Center));
        app.session.selection.set_one(a);
        let pass = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let mut out = None;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::pos2(0.0, 0.0),
                        egui::vec2(320.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    out = app.stroke_properties(ui, PaintScope::Node(a), 0, &stroke);
                },
            );
            out
        };
        pass(
            &mut app,
            vec![egui::Event::PointerMoved(outside), key(outside, true)],
        );
        let nothing = pass(&mut app, vec![key(outside, false)]);
        assert!(
            nothing.is_none(),
            "with the popup shut that click is on nothing: {nothing:?}"
        );
    }

    /// The picker leaves the mixed row only once it has gone: still mixed keeps the slot, agreed
    /// moves to `Selection(Fill, 0)`, no fills leaves it for `slot_brush` to close.
    ///
    /// flip: removing the `p.slot = to` write in `retarget_mixed_paint` → fails at the middle (agreed) case only.
    #[test]
    fn the_picker_leaves_the_mixed_row_only_once_the_mixed_row_has_gone() {
        let ctx = egui::Context::default();
        let red = ondin_core::peniko::Color::from_rgb8(220, 30, 30);
        let solid = |c| Fill {
            brush: Brush::Solid(c).into(),
            visible: true,
        };
        let staged = |a_fills: Vec<Fill>, b_fills: Vec<Fill>| {
            theme::install(&ctx);
            let mut app = OndinApp::headless(&ctx);
            let mut ids = ondin_core::IdSource::new(0x5A3);
            let root = ids.mint();
            let mut doc = ondin_core::Document::new(root);
            let (a, b) = (ids.mint(), ids.mint());
            let rect = |id| Operation::CreateNode {
                id,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 20.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            };
            doc.apply(&Transaction(vec![
                rect(a),
                rect(b),
                Operation::SetFills {
                    id: a,
                    fills: ondin_core::keyed_by_position(a_fills),
                },
                Operation::SetFills {
                    id: b,
                    fills: ondin_core::keyed_by_position(b_fills),
                },
            ]))
            .expect("two rects");
            app.session.adopt_document(doc, None);
            app.session.selection.set(vec![a, b]);
            app.picker = Some(super::Picker::new(
                app.session.doc.first_page(),
                PaintSlot::SelectionAll(ondin_core::PaintTarget::Fill),
                egui::Pos2::ZERO,
            ));
            app
        };

        let mut app = staged(vec![solid(red)], vec![solid(red), solid(red)]);
        assert!(
            app.fills_shown(PaintScope::Selection).is_mixed(),
            "the fixture: two lists of different lengths are Mixed"
        );
        app.retarget_mixed_paint(ondin_core::PaintTarget::Fill);
        assert_eq!(
            app.picker.as_ref().map(|p| p.slot),
            Some(PaintSlot::SelectionAll(ondin_core::PaintTarget::Fill)),
            "the stand-in is still what the panel draws, so the slot stays"
        );

        let mut app = staged(vec![solid(red)], vec![solid(red)]);
        assert!(
            !app.fills_shown(PaintScope::Selection).is_mixed(),
            "the fixture: one fill each, the same, so they agree"
        );
        app.retarget_mixed_paint(ondin_core::PaintTarget::Fill);
        assert_eq!(
            app.picker.as_ref().map(|p| p.slot),
            Some(PaintSlot::Selection(ondin_core::PaintTarget::Fill, 0)),
            "and the picker follows onto the row that replaced it"
        );

        let mut app = staged(Vec::new(), Vec::new());
        app.retarget_mixed_paint(ondin_core::PaintTarget::Fill);
        assert_eq!(
            app.picker.as_ref().map(|p| p.slot),
            Some(PaintSlot::SelectionAll(ondin_core::PaintTarget::Fill)),
            "with no rows to move to, the slot is left for slot_brush to close"
        );
    }

    /// A click over the mixed row, through `write_slot`, carries the picker onto the row that replaces it.
    ///
    /// flip: removing the `SelectionAll` branch of `retarget_after` → fails at the picker slot assertion.
    #[test]
    fn a_click_over_the_mixed_row_carries_the_picker_onto_the_row_that_replaces_it() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0x5A4);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let (a, b) = (ids.mint(), ids.mint());
        let rect = |id| Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        };
        let paint = |id, c| Operation::SetFills {
            id,
            fills: ondin_core::keyed_by_position([Fill {
                brush: Brush::Solid(c).into(),
                visible: true,
            }]),
        };
        let blue = ondin_core::peniko::Color::from_rgb8(30, 120, 200);
        let green = ondin_core::peniko::Color::from_rgb8(20, 180, 90);
        let red = ondin_core::peniko::Color::from_rgb8(220, 30, 30);
        doc.apply(&Transaction(vec![
            rect(a),
            rect(b),
            paint(a, blue),
            paint(b, green),
        ]))
        .expect("two differently painted rects");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![a, b]);
        let mixed = PaintSlot::SelectionAll(ondin_core::PaintTarget::Fill);
        app.picker = Some(super::Picker::new(root, mixed, egui::Pos2::ZERO));
        assert!(
            app.fills_shown(PaintScope::Selection).is_mixed(),
            "the fixture: the panel is drawing the stand-in row"
        );

        app.write_slot(root, mixed, Brush::Solid(red));

        assert!(
            !app.fills_shown(PaintScope::Selection).is_mixed(),
            "the write brought them into agreement, so the stand-in has gone"
        );
        assert_eq!(
            app.picker.as_ref().map(|p| p.slot),
            Some(PaintSlot::Selection(ondin_core::PaintTarget::Fill, 0)),
            "and the picker went with it rather than being left on a dead row"
        );
    }

    /// A `Selection`-scoped list write (fills and strokes) reaches every target; a `Node`-scoped one reaches exactly one.
    ///
    /// flip: a `Selection` arm handed `&[]`, or a `Node` arm pointed at the first member → fails at that arm's assertion.
    #[test]
    fn a_selection_scoped_list_write_reaches_every_target_and_a_node_one_reaches_one() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0x5A5);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let (a, b) = (ids.mint(), ids.mint());
        let rect = |id| Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        };
        doc.apply(&Transaction(vec![rect(a), rect(b)]))
            .expect("two rects");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![a, b]);

        let red = ondin_core::peniko::Color::from_rgb8(220, 30, 30);
        let one_fill = ondin_core::keyed_by_position([Fill {
            brush: Brush::Solid(red).into(),
            visible: true,
        }]);
        let counts = |app: &OndinApp| {
            let n = |id| {
                let p = app.session.doc.get(id).expect("node").paint();
                (p.fills.len(), p.strokes.len())
            };
            (n(a), n(b))
        };
        assert_eq!(counts(&app), ((0, 0), (0, 0)), "the fixture: bare rects");

        app.write_fill_list(PaintScope::Selection, one_fill.clone());
        assert_eq!(
            counts(&app),
            ((1, 0), (1, 0)),
            "a Selection write reaches every target in the selection"
        );

        app.write_strokes(
            PaintScope::Selection,
            ondin_core::keyed_by_position([ondin_core::Stroke {
                width: 3.0,
                ..Default::default()
            }]),
        );
        assert_eq!(
            counts(&app),
            ((1, 1), (1, 1)),
            "and so does its stroke twin, which is the same function re-typed"
        );

        app.write_fill_list(PaintScope::Node(b), Vec::new());
        app.write_strokes(PaintScope::Node(b), Vec::new());
        assert_eq!(
            counts(&app),
            ((1, 1), (0, 0)),
            "a Node write reaches that node and nothing else, both lists"
        );
    }

    /// `preview_slot` returns whether it had anything to preview: `Fill(0)` does, `Fill(9)` on the same layer does not.
    ///
    /// flip: restoring the bare `true` → fails at the second assertion.
    #[test]
    fn previewing_a_slot_that_has_nothing_to_write_says_so() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0x5A7);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let id = ids.mint();
        let red = ondin_core::peniko::Color::from_rgb8(220, 30, 30);
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 20.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id,
                fills: ondin_core::keyed_by_position([Fill {
                    brush: Brush::Solid(red).into(),
                    visible: true,
                }]),
            },
        ]))
        .expect("a rect with one fill");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![id]);

        let blue = Brush::Solid(ondin_core::peniko::Color::from_rgb8(30, 120, 200));
        assert!(
            app.preview_slot(id, PaintSlot::Fill(0), blue.clone()),
            "the control: a row that exists previews, and says it did"
        );
        assert!(
            !app.preview_slot(id, PaintSlot::Fill(9), blue),
            "a row past the end of the list has nothing to preview and says so"
        );
    }

    /// An effect colour reaches every selected layer's own stack (layer B carries an extra blur).
    ///
    /// flip: `selection.ids()` → `&[id]` → fails at the widening; reading the anchor's effects for every layer → fails at B's stack length.
    #[test]
    fn an_effect_colour_reaches_every_selected_layers_own_stack() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0x5A6);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let (a, b) = (ids.mint(), ids.mint());
        let rect = |id| Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        };
        let red = ondin_core::peniko::Color::from_rgb8(220, 30, 30);
        let green = ondin_core::peniko::Color::from_rgb8(20, 180, 90);
        let blue = ondin_core::peniko::Color::from_rgb8(30, 120, 200);
        let shadow = |c| {
            ondin_core::Effect::new(ondin_core::EffectKind::DropShadow(ondin_core::Shadow {
                color: c,
                ..Default::default()
            }))
        };
        doc.apply(&Transaction(vec![
            rect(a),
            rect(b),
            Operation::SetEffects {
                id: a,
                effects: ondin_core::keyed_by_position([shadow(red)]),
            },
            Operation::SetEffects {
                id: b,
                effects: ondin_core::keyed_by_position([
                    shadow(green),
                    ondin_core::Effect::new(ondin_core::EffectKind::LayerBlur { radius: 12.0 }),
                ]),
            },
        ]))
        .expect("two shadowed rects, one of them also blurred");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![a, b]);

        let stack = |app: &OndinApp, id: NodeId| {
            app.session
                .doc
                .get(id)
                .expect("node")
                .effects()
                .iter()
                .map(|e| match &e.kind {
                    ondin_core::EffectKind::DropShadow(s) => format!("shadow {:?}", s.color),
                    other => format!("{other:?}"),
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            (stack(&app, a).len(), stack(&app, b).len()),
            (1, 2),
            "the fixture: the second layer carries an effect the anchor does not"
        );

        let tx = app.slot_transaction(a, PaintSlot::Effect(0), Brush::Solid(blue));
        app.session.commit(tx);

        assert_eq!(
            stack(&app, a)[0],
            format!("shadow {blue:?}"),
            "the anchor's own row takes the colour"
        );
        assert_eq!(
            stack(&app, b)[0],
            format!("shadow {blue:?}"),
            "and so does the same row on every other selected layer (D340)"
        );
        assert_eq!(
            stack(&app, b).len(),
            2,
            "while that layer keeps the rest of its own stack (D476)"
        );

        app.session.commit(Transaction(vec![Operation::SetEffects {
            id: a,
            effects: ondin_core::keyed_by_position([ondin_core::Effect::new(
                ondin_core::EffectKind::LayerBlur { radius: 4.0 },
            )]),
        }]));
        let tx = app.slot_transaction(a, PaintSlot::Effect(0), Brush::Solid(red));
        assert!(
            tx.0.is_empty(),
            "a colour aimed at a row that is no longer a shadow builds nothing, \
             got {} ops",
            tx.0.len()
        );
    }
}

/// What the mixed paint row reports, over a headless app and a selection. Not covered:
/// `paint_row`'s `door`, a hover-text guard (a tooltip's ink never reaches `FullOutput`).
#[cfg(test)]
mod mixed_spread_tests {
    use super::*;
    use crate::app::OndinApp;
    use ondin_core::PaintTarget;
    use ondin_core::kurbo::Size;
    use ondin_core::peniko::Color;

    /// Two rects, each with the fills given, both selected.
    fn two(ctx: &egui::Context, a_fills: Vec<Fill>, b_fills: Vec<Fill>) -> OndinApp {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(0x0F1);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let (a, b) = (ids.mint(), ids.mint());
        let rect = |id| Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        };
        doc.apply(&Transaction(vec![
            rect(a),
            rect(b),
            Operation::SetFills {
                id: a,
                fills: ondin_core::keyed_by_position(a_fills),
            },
            Operation::SetFills {
                id: b,
                fills: ondin_core::keyed_by_position(b_fills),
            },
        ]))
        .expect("two rects");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![a, b]);
        app
    }

    fn fill(alpha: f32, visible: bool) -> Fill {
        Fill {
            brush: Brush::Solid(Color::new([1.0, 0.0, 0.0, alpha])).into(),
            visible,
        }
    }

    /// The mixed row's eye reads open while any paint in the selection is showing (`any`, not `all`).
    ///
    /// flip: `!= Some(false)` → `== Some(true)` in `mixed_visible` → fails only at the one-showing-one-hidden case.
    #[test]
    fn the_mixed_eye_is_open_while_anything_in_the_selection_is_showing() {
        let ctx = egui::Context::default();

        let app = two(&ctx, vec![fill(1.0, true)], vec![fill(1.0, true)]);
        assert!(app.mixed_visible(PaintTarget::Fill), "both showing: open");

        let app = two(&ctx, vec![fill(1.0, true)], vec![fill(1.0, false)]);
        assert!(
            app.mixed_visible(PaintTarget::Fill),
            "one showing and one hidden: still open, because there is ink"
        );

        let app = two(&ctx, vec![fill(1.0, false)], vec![fill(1.0, false)]);
        assert!(
            !app.mixed_visible(PaintTarget::Fill),
            "and only all-hidden closes it"
        );
    }

    /// A layer with no fill disagrees with one that has one, so `mixed_row` reports the brush as Mixed.
    ///
    /// flip: deleting `spread.bare |= fills.is_empty()` → fails at the second `row.brush` assertion; the stroke twin keeps its own line.
    #[test]
    fn a_layer_with_no_paint_disagrees_with_the_one_that_has_some() {
        let ctx = egui::Context::default();

        let app = two(&ctx, vec![fill(1.0, true)], vec![fill(1.0, true)]);
        let (_, row) = app.mixed_row(PaintTarget::Fill);
        assert!(
            !row.brush,
            "the fixture: two identical fills agree about the brush"
        );

        let app = two(&ctx, vec![fill(1.0, true)], Vec::new());
        let (_, row) = app.mixed_row(PaintTarget::Fill);
        assert!(
            row.brush,
            "a layer with no fill disagrees with the one that has one"
        );
        assert!(
            app.shown_over_fills(|f| f.visible).bare,
            "and the spread says so directly"
        );

        assert!(
            app.shown_over_strokes(|s| s.visible).bare,
            "two layers with no strokes are bare on that side too"
        );
    }

    /// The row's alpha is compared as a byte: `0.5` and `0.5 + 1/2048` both round to 128 and do not read Mixed.
    ///
    /// flip: replacing the byte map with `paint::alpha_of` itself → fails at the first assertion.
    #[test]
    fn two_alphas_that_print_the_same_are_not_mixed() {
        let ctx = egui::Context::default();
        let close = 0.5 + 1.0 / 2048.0;
        assert_ne!(close, 0.5_f32, "the fixture: two distinct f32 alphas");
        assert_eq!(
            (
                (close * 255.0).round() as u8,
                (0.5_f32 * 255.0).round() as u8
            ),
            (128, 128),
            "the fixture: that print as one byte"
        );

        let app = two(&ctx, vec![fill(0.5, true)], vec![fill(close, true)]);
        let (_, row) = app.mixed_row(PaintTarget::Fill);
        assert!(
            !row.alpha,
            "two alphas that write the same file do not disagree on the row"
        );

        let app = two(&ctx, vec![fill(1.0, true)], vec![fill(0.4, true)]);
        let (_, row) = app.mixed_row(PaintTarget::Fill);
        assert!(
            row.alpha,
            "the control: a real difference still reads Mixed"
        );
    }

    /// `original size` declines over a selection, since a selection write reaches every node in it.
    ///
    /// flip: removing the `Selection`/`SelectionAll` decline in `image_pixels` → fails at the two selection slots.
    #[test]
    fn original_size_declines_over_a_selection_and_answers_for_one_row() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0x1A6);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let id = ids.mint();
        let image = ondin_core::ImageId("probe".into());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 20.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            },
            Operation::AddImage {
                id: image.clone(),
                entry: ondin_core::ImageEntry {
                    source: ondin_core::ImageSource::Embedded(vec![0u8; 16].into()),
                    format: ondin_core::ImageFormat::Png,
                    width: 4,
                    height: 3,
                },
            },
        ]))
        .expect("a rect and a 4x3 image");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![id]);

        let brush = ondin_core::image_brush(image);
        assert_eq!(
            app.image_pixels(PaintSlot::Fill(0), &brush),
            Some((4, 3)),
            "the control: one row of one layer answers with the file's own pixels"
        );
        assert_eq!(
            app.image_pixels(PaintSlot::Selection(PaintTarget::Fill, 0), &brush),
            None,
            "and a selection row declines (D181)"
        );
        assert_eq!(
            app.image_pixels(PaintSlot::SelectionAll(PaintTarget::Fill), &brush),
            None,
            "as does the mixed row"
        );
    }
}

/// Slot decisions reachable on a headless app: align target, crop seed, effect colour, dash patterns.
#[cfg(test)]
mod slot_decision_tests {
    use super::*;
    use crate::app::OndinApp;
    use ondin_core::kurbo::Size;
    use ondin_core::{Effect, EffectKind, PaintTarget};

    /// Three rects at (0,0), (200,0) and (400,0), 20 × 20 each, all selected.
    fn three(ctx: &egui::Context) -> (OndinApp, Vec<NodeId>) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(0x51D);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let made: Vec<NodeId> = (0..3).map(|_| ids.mint()).collect();
        let ops: Vec<Operation> = made
            .iter()
            .enumerate()
            .map(|(i, id)| Operation::CreateNode {
                id: *id,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 20.0),
                    corner_radii: Default::default(),
                },
                transform: Some(Affine::translate((200.0 * i as f64, 0.0))),
                name: None,
            })
            .collect();
        doc.apply(&Transaction(ops)).expect("three rects");
        app.session.adopt_document(doc, None);
        app.session.selection.set(made.clone());
        (app, made)
    }

    /// A key layer that `outermost` drops (selected beside its ancestor) is not the box the set aligns to.
    ///
    /// flip: dropping `.filter(|k| ids.contains(k))` in `align_target` → fails at the union assertion.
    #[test]
    fn a_key_layer_that_outermost_drops_is_not_the_box_the_set_aligns_to() {
        let ctx = egui::Context::default();

        let (mut app, ids) = three(&ctx);
        app.session.selection.set_key(ids[1]);
        assert_eq!(
            app.align_target().map(|r| (r.min_x(), r.width())),
            Some((200.0, 20.0)),
            "the control: a surviving key is the box the set aligns to"
        );

        // `set_key` ignores an id outside the selection.
        let (mut app, ids) = three(&ctx);
        let mut src = ondin_core::IdSource::new(0x51E);
        let (tx, group) = build::group(
            &app.session.doc,
            &app.session.resolved,
            &mut src,
            &[ids[0], ids[1]],
        )
        .expect("two siblings group");
        app.session.commit(tx);
        app.session.selection.set(vec![group, ids[0], ids[2]]);
        app.session.selection.set_key(ids[0]);
        assert_eq!(
            app.session.selection.key(),
            Some(ids[0]),
            "the fixture: the key is set, because it is in the selection"
        );
        assert_eq!(
            build::outermost(&app.session.doc, app.session.selection.ids()).len(),
            2,
            "the fixture: and dropped by outermost, leaving two operands"
        );
        let target = app.align_target().expect("a target");
        assert_eq!(
            (target.min_x(), target.max_x()),
            (0.0, 420.0),
            "the union of the operands, not the box of a key inside one of them"
        );
    }

    /// `crop_seed` declines over a selection, since one brush goes to every node.
    ///
    /// flip: deleting the guard in `crop_seed` → fails at the two selection slots.
    #[test]
    fn crop_seed_declines_over_a_selection_and_answers_for_one_row() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0x51F);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let id = ids.mint();
        let image = ondin_core::ImageId("seed".into());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(80.0, 40.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            },
            Operation::AddImage {
                id: image.clone(),
                entry: ondin_core::ImageEntry {
                    source: ondin_core::ImageSource::Embedded(vec![0u8; 16].into()),
                    format: ondin_core::ImageFormat::Png,
                    width: 4,
                    height: 3,
                },
            },
        ]))
        .expect("a rect and an image");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![id]);
        let brush = ondin_core::image_brush(image);

        assert_eq!(
            app.crop_seed(id, PaintSlot::Fill(0), &brush)
                .map(|(f, w, h)| (f.width(), f.height(), w, h)),
            Some((80.0, 40.0, 4, 3)),
            "the control: one row seeds from that row's own frame and picture"
        );
        assert!(
            app.crop_seed(id, PaintSlot::Selection(PaintTarget::Fill, 0), &brush)
                .is_none(),
            "a selection row declines (D181)"
        );
        assert!(
            app.crop_seed(id, PaintSlot::SelectionAll(PaintTarget::Fill), &brush)
                .is_none(),
            "as does the mixed row"
        );
    }

    /// An effect row with no colour answers `None`, not a placeholder, so a picker left on a retyped row closes.
    ///
    /// flip: replacing the colourless arm with a solid black → fails at the blur assertion.
    #[test]
    fn an_effect_row_with_no_colour_answers_nothing_rather_than_a_placeholder() {
        let ctx = egui::Context::default();
        let (mut app, ids) = three(&ctx);
        let id = ids[0];
        let red = ondin_core::peniko::Color::from_rgb8(220, 30, 30);
        app.session.commit(Transaction(vec![Operation::SetEffects {
            id,
            effects: ondin_core::keyed_by_position([
                Effect::new(EffectKind::DropShadow(ondin_core::Shadow {
                    color: red,
                    ..Default::default()
                })),
                Effect::new(EffectKind::LayerBlur { radius: 12.0 }),
            ]),
        }]));

        assert_eq!(
            app.slot_brush(id, PaintSlot::Effect(0)),
            Some(Brush::Solid(red)),
            "the control: a shadow's row has a colour and gives it"
        );
        assert!(
            app.slot_brush(id, PaintSlot::Effect(1)).is_none(),
            "a layer blur has no colour, so the row answers nothing"
        );
        assert!(
            app.slot_brush(id, PaintSlot::Effect(9)).is_none(),
            "and so does a row that is not there — the same answer by a \
             different door"
        );
    }

    /// The Dashed and Custom arms of `pattern_for` keep the numbers they are handed.
    ///
    /// flip: Dashed's floors `0.0` → `1.0` → fails at the zero-gap assertion; Custom's `> 2` → `> 3` → fails at the three-number one.
    #[test]
    fn the_dashed_and_custom_arms_keep_the_numbers_they_are_handed() {
        let stroke = |dashes: Vec<f64>| Stroke {
            width: 4.0,
            dashes,
            ..Default::default()
        };

        let zero_gap = stroke(vec![10.0, 0.0]);
        assert_eq!(
            super::pattern_for(DashStyle::Dashed, &zero_gap),
            vec![10.0, 0.0],
            "a dashed stroke with a zero gap keeps it"
        );

        let three = stroke(vec![8.0, 4.0, 2.0]);
        assert_eq!(
            super::pattern_for(DashStyle::Custom, &three),
            vec![8.0, 4.0, 2.0],
            "a three-number custom list is kept whole"
        );
    }
}

/// `TEMPLATE_BOX_MIN`, the floor that keeps an extreme template a picture rather than a hairline.
/// A resting cell paints one `rect_filled`, the fitted picture.
#[cfg(test)]
mod template_cell_tests {
    use super::*;

    /// The picture a template of `w` × `h` is drawn as, in points.
    fn picture(w: f64, h: f64) -> egui::Vec2 {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(280.0, 200.0),
                )),
                ..Default::default()
            },
            |ui| {
                super::template_cell(ui, 240.0, "Probe", w, h, false);
            },
        );
        let rects: Vec<egui::Rect> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Rect(r) if r.stroke.width == 0.0 => Some(r.rect),
                _ => None,
            })
            .collect();
        assert_eq!(
            rects.len(),
            1,
            "a resting cell paints exactly one filled rect and it is the picture, \
             got {rects:?}"
        );
        rects[0].size()
    }

    /// An ordinary template (16:9) is fitted honestly; a 40:1 strip and a 1:40 tower stop at the floor of 5.
    ///
    /// flip: removing both `.max(TEMPLATE_BOX_MIN)` calls → fails at the strip's height (`0.5` against `5.0`).
    #[test]
    fn an_extreme_template_is_clamped_to_a_picture_rather_than_a_hairline() {
        let ordinary = picture(1600.0, 900.0);
        assert_eq!(
            (ordinary.x, ordinary.y),
            (20.0, 11.25),
            "the control: a 16:9 template is width-bound and needs no floor"
        );

        let strip = picture(40.0, 1.0);
        assert_eq!(
            (strip.x, strip.y),
            (20.0, 5.0),
            "a 40:1 strip fits to 0.5 tall and is held at the floor"
        );

        let tower = picture(1.0, 40.0);
        assert_eq!(
            (tower.x, tower.y),
            (5.0, 28.0),
            "and a 1:40 tower the same way on the other axis"
        );
    }
}

#[cfg(test)]
mod quarters_row_tests {
    //! The Transform card's four-button row, driven on the real `quarters_row`.
    use super::*;
    use crate::app::OndinApp;

    /// The card width the real panel lays a Transform row out in.
    const CARD: f32 = 222.0;

    /// The four button faces of one `quarters_row`, left to right, after two passes.
    fn buttons(scope: TurnScope) -> Vec<egui::Rect> {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut out = None;
        for _ in 0..2 {
            out = Some(ctx.run_ui(Default::default(), |ui| {
                ui.set_max_width(CARD);
                ui.set_min_width(CARD);
                OndinApp::quarters_row(&mut app, ui, scope);
            }));
        }
        let mut rects: Vec<egui::Rect> = out
            .expect("two passes ran")
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r) if r.fill == crate::theme::color::FIELD => Some(r.rect),
                _ => None,
            })
            .collect();
        rects.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
        rects
    }

    /// The four buttons split the card evenly and fill it.
    ///
    /// flip: divisor `4.0` → `3.0` → fails at the fill assertion only; the evenness loop catches a button of a different width.
    #[test]
    fn the_four_buttons_split_the_card_evenly_and_fill_it() {
        let bs = buttons(TurnScope::Selection);
        assert_eq!(bs.len(), 4, "four buttons: {bs:?}");
        for b in &bs {
            assert_eq!(b.width(), bs[0].width(), "uneven: {bs:?}");
            assert_eq!(b.height(), 28.0, "{b:?} is not 28 tall");
        }
        assert!(
            (bs[3].max.x - bs[0].min.x - CARD).abs() < 0.01,
            "the row does not fill the card: {:?}..{:?} of {CARD}",
            bs[0].min.x,
            bs[3].max.x
        );
    }

    /// Both scopes lay the row out identically.
    ///
    /// flip: giving `TurnScope::Node` its own `/ 5.0` split inside `quarters_row` → fails here, not above.
    #[test]
    fn both_scopes_lay_the_row_out_identically() {
        // Any id: the scope's id reaches `rotate_quarter` on a click, not the layout.
        let one = buttons(TurnScope::Node(ondin_core::IdSource::new(1).mint()));
        let many = buttons(TurnScope::Selection);
        assert_eq!(one.len(), 4, "one layer: {one:?}");
        assert_eq!(many.len(), 4, "a selection: {many:?}");
        for (a, b) in one.iter().zip(&many) {
            assert!(
                (a.width() - b.width()).abs() < 0.01 && (a.min.x - b.min.x).abs() < 0.01,
                "the two cards' rows disagree: {a:?} vs {b:?}"
            );
        }
    }
}

#[cfg(test)]
mod colour_panel_tests {
    //! The Appearance card that holds one colour and a reset (`OndinApp::colour_panel`).
    use super::*;
    use crate::app::OndinApp;

    const AREA: egui::Rect = egui::Rect {
        min: egui::pos2(0.0, 0.0),
        max: egui::pos2(320.0, 600.0),
    };

    /// A headless app showing the canvas card (nothing selected, so `inspector_empty` draws).
    fn canvas_app(ctx: &egui::Context) -> OndinApp {
        crate::theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(0xCA7);
        let root = ids.mint();
        app.session
            .adopt_document(ondin_core::Document::new(root), None);
        app.session.selection.clear();
        app
    }

    /// Draw the card and report where the reset glyph was painted and its colour, located by glyph.
    fn reset_glyph(
        ctx: &egui::Context,
        app: &mut OndinApp,
        events: Vec<egui::Event>,
    ) -> (egui::Pos2, Option<egui::Color32>) {
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(AREA),
                events,
                ..Default::default()
            },
            |ui| app.inspector_empty(ui),
        );
        type Glyph = (String, egui::Pos2, Option<egui::Color32>);
        fn walk(s: &egui::Shape, v: &mut Vec<Glyph>) {
            match s {
                egui::Shape::Text(t) => {
                    // `painter().text` bakes the colour into the job's section format, so `override_text_color` is `None`.
                    let colour = t
                        .override_text_color
                        .or_else(|| t.galley.job.sections.first().map(|s| s.format.color));
                    v.push((t.galley.text().to_string(), t.pos, colour));
                }
                egui::Shape::Vec(x) => x.iter().for_each(|s| walk(s, v)),
                _ => {}
            }
        }
        let mut v = Vec::new();
        for cs in &out.shapes {
            walk(&cs.shape, &mut v);
        }
        let (_, pos, colour) = v
            .iter()
            .find(|(t, _, _)| t == icon::ARROW_COUNTER_CLOCKWISE)
            .unwrap_or_else(|| panic!("the reset glyph is drawn, in {v:?}"));
        // The glyph's top-left; a few points in lands inside the 28×28 button.
        (egui::pos2(pos.x + 4.0, pos.y + 6.0), *colour)
    }
    /// Where the reset is, ignoring how it is painted.
    fn reset_at(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) -> egui::Pos2 {
        reset_glyph(ctx, app, events).0
    }

    /// The canvas reset is painted dead on the default and puts the ground back otherwise.
    ///
    /// flip: `enabled_if(resettable)` → `enabled_if(false)` → fails at the glyph colour assertion only;
    /// the `&& resettable` re-test after `.clicked()` is unpinned (`changes_nothing` absorbs the commit).
    #[test]
    fn the_canvas_reset_is_dead_on_the_default_and_puts_the_ground_back_otherwise() {
        let ctx = egui::Context::default();
        let mut app = canvas_app(&ctx);
        let ground = |app: &OndinApp| app.session.preview_ground().to_rgba8().to_u8_array();
        let default = crate::session::DEFAULT_CANVAS_BACKGROUND
            .to_rgba8()
            .to_u8_array();
        let click = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };

        assert_eq!(ground(&app), default, "the fixture starts on the default");
        let (at, dead_colour) = reset_glyph(&ctx, &mut app, Vec::new());
        let depth = app.session.history.undo_depth();
        reset_at(
            &ctx,
            &mut app,
            vec![egui::Event::PointerMoved(at), click(at, true)],
        );
        reset_at(&ctx, &mut app, vec![click(at, false)]);
        assert_eq!(
            (ground(&app), app.session.history.undo_depth()),
            (default, depth),
            "a dead reset writes nothing"
        );

        let moved = ondin_core::peniko::Color::from_rgba8(90, 20, 20, 255);
        app.session
            .try_commit(Transaction(vec![Operation::SetPageGround {
                id: app.session.page,
                ground: moved,
            }]))
            .expect("a ground the user picked");
        assert_ne!(ground(&app), default, "the fixture: the ground has moved");
        let (at, live_colour) = reset_glyph(&ctx, &mut app, Vec::new());
        // The glyph colour is what pins `enabled_if`; the ground and undo checks read the same either way.
        assert_ne!(
            live_colour, dead_colour,
            "a reset with something to undo is not painted like one without"
        );
        let depth = app.session.history.undo_depth();
        reset_at(
            &ctx,
            &mut app,
            vec![egui::Event::PointerMoved(at), click(at, true)],
        );
        reset_at(&ctx, &mut app, vec![click(at, false)]);
        assert_eq!(ground(&app), default, "the reset puts the ground back");
        assert_eq!(
            app.session.history.undo_depth(),
            depth + 1,
            "as one undo step"
        );
    }
}

#[cfg(test)]
mod select_by_color_lock_tests {
    //! The Group Colors reticle counts locked layers without selecting them; the status message reconciles the two.

    use crate::app::OndinApp;
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    /// A 10×10 rect under `parent` at `index`.
    fn rect(doc: &mut Document, ids: &mut IdSource, parent: NodeId, index: usize) -> NodeId {
        let id = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate((20.0 * index as f64, 0.0))),
            name: None,
        }]))
        .expect("a rect");
        id
    }

    /// Three rects under a frame; `lock` says which of them are locked.
    fn fixture(ctx: &egui::Context, lock: [bool; 3]) -> (OndinApp, [NodeId; 3]) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0xC01);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let board = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: board,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(200.0, 200.0),
            },
            transform: Some(Affine::IDENTITY),
            name: None,
        }]))
        .expect("an artboard");
        let three = [
            rect(&mut doc, &mut ids, board, 0),
            rect(&mut doc, &mut ids, board, 1),
            rect(&mut doc, &mut ids, board, 2),
        ];
        let ops: Vec<Operation> = three
            .iter()
            .zip(lock)
            .filter(|(_, l)| *l)
            .map(|(id, _)| Operation::SetLocked {
                id: *id,
                locked: true,
            })
            .collect();
        if !ops.is_empty() {
            doc.apply(&Transaction(ops)).expect("lock them");
        }
        app.session.adopt_document(doc, None);
        (app, three)
    }

    /// Two of three are selected, and the message says which and why.
    ///
    /// flip: `may_select_in_bulk` returning `true` → fails at the selection assertion (and the other doors' lock tests);
    /// dropping the `(n, l)` arm of the message match → fails at the text assertion only.
    #[test]
    fn the_reticle_counts_a_locked_layer_and_leaves_it_out_of_the_selection() {
        let ctx = egui::Context::default();
        let (mut app, [a, locked, c]) = fixture(&ctx, [false, true, false]);

        assert!(
            ondin_core::query::is_effectively_locked(&app.session.doc, locked),
            "the fixture's middle layer really is locked"
        );

        app.select_by_color(vec![a, locked, c]);

        assert_eq!(
            app.session.selection.ids(),
            [a, c],
            "the two unlocked layers, in the order the census gave them"
        );
        assert_eq!(
            app.session.status().text,
            "Selected 2 of 3 layers with this colour — 1 locked",
            "the message reconciles the census count with the selection"
        );
    }

    /// Every match locked: the selection is left unchanged (not cleared) and the message says so.
    #[test]
    fn the_reticle_changes_nothing_when_every_match_is_locked() {
        let ctx = egui::Context::default();
        let (mut app, [a, b, c]) = fixture(&ctx, [true, true, true]);
        app.session.selection.set(vec![a]);

        app.select_by_color(vec![a, b, c]);

        assert_eq!(
            app.session.selection.ids(),
            [a],
            "the selection the user already had is untouched"
        );
        assert_eq!(
            app.session.status().text,
            "All 3 layers with this colour are locked",
            "and the click is accounted for"
        );
    }

    /// Nothing locked: the message carries no "— 0 locked" tail and the singular is spelled out.
    #[test]
    fn the_reticle_says_what_it_always_did_when_nothing_is_locked() {
        let ctx = egui::Context::default();
        let (mut app, [a, b, c]) = fixture(&ctx, [false, false, false]);

        app.select_by_color(vec![a, b, c]);
        assert_eq!(app.session.selection.ids(), [a, b, c]);
        assert_eq!(
            app.session.status().text,
            "Selected 3 layers with this colour"
        );

        app.select_by_color(vec![a]);
        assert_eq!(
            app.session.status().text,
            "Selected 1 layer with this colour",
            "and the singular is still spelled out rather than formatted"
        );
    }

    /// Reticle then `Delete` leaves the locked layer in the document (`delete_selection` reads no lock).
    #[test]
    fn the_reticle_then_delete_leaves_a_locked_layer_where_it_was() {
        let ctx = egui::Context::default();
        let (mut app, [a, locked, c]) = fixture(&ctx, [false, true, false]);

        app.select_by_color(vec![a, locked, c]);
        app.delete_selection();

        assert!(
            app.session.doc.get(locked).is_some(),
            "the locked layer is still in the document"
        );
        assert!(
            app.session.doc.get(a).is_none() && app.session.doc.get(c).is_none(),
            "and the two the reticle did select are gone, which is the control — \
             without it this passes against a Delete that did nothing at all"
        );
    }
}

/// The Position card, driven through the app's own methods on a headless app. The fixture is a
/// 300×200 frame holding a 100×50 rect at (40, 30); assertions are on where the rect is drawn.
#[cfg(test)]
mod inset_card_tests {
    use super::*;
    use ondin_core::container::Edge as Pin;
    use ondin_core::kurbo::Rect as KRect;

    fn app_with_pinnable() -> (OndinApp, NodeId, NodeId) {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(7);
        let root = ids.mint();
        let (frame, rect) = (ids.mint(), ids.mint());
        let mut doc = ondin_core::Document::new(root);
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(300.0, 200.0),
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
            Operation::CreateNode {
                id: rect,
                parent: frame,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 50.0),
                    corner_radii: RoundedRectRadii::default(),
                },
                transform: Some(Affine::translate((40.0, 30.0))),
                name: None,
            },
        ]))
        .unwrap();
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![rect]);
        (app, frame, rect)
    }

    fn drawn(app: &OndinApp, id: NodeId) -> KRect {
        app.session.resolved.world_bounds(id).expect("measured")
    }

    fn resize_frame(app: &mut OndinApp, frame: NodeId, w: f64) {
        app.session.commit(Transaction(vec![Operation::SetGeometry {
            id: frame,
            geometry: GeometryPatch::Size(Size::new(w, 200.0)),
        }]));
    }

    fn create_rect(id: NodeId, parent: NodeId, index: usize, w: f64, at: (f64, f64)) -> Operation {
        Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Rect {
                size: Size::new(w, 30.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: Some(Affine::translate(at)),
            name: None,
        }
    }

    /// Typing an inset into an in-flow item leaves its other axis where it is drawn (`b` is stored at y 120, laid at y 0).
    ///
    /// flip: dropping `baked_placement` in `typed_inset_ops` → fails at "y kept" (120 against 0).
    #[test]
    fn a_typed_inset_on_an_in_flow_item_keeps_its_other_axis() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(7);
        let (root, frame, a, b) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
        let mut doc = ondin_core::Document::new(root);
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 200.0),
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
            create_rect(a, frame, 0, 40.0, (0.0, 0.0)),
            create_rect(b, frame, 1, 60.0, (250.0, 120.0)),
            Operation::SetDisplay {
                id: frame,
                display: Some(ondin_core::container::Display::Flex(
                    ondin_core::container::Flex {
                        align_items: ondin_core::container::AlignItems::Start,
                        ..Default::default()
                    },
                )),
            },
        ]))
        .unwrap();
        app.session.adopt_document(doc, None);
        assert_eq!(drawn(&app, b).origin(), (40.0, 0.0).into(), "the fixture");
        let ops = app.typed_inset_ops(&[b], Pin::Left, ondin_core::LengthPct::Px(100.0));
        app.session.commit(Transaction(ops));
        assert_eq!(drawn(&app, b).x0, 100.0, "x as typed");
        assert_eq!(drawn(&app, b).y0, 0.0, "y kept");
    }

    /// The unit suffix over several layers converts each one's own distance and pins nothing.
    ///
    /// flip: writing the anchor's converted value to every subject → fails at "B stays" (x0 40 against 100).
    #[test]
    fn a_unit_switch_over_several_layers_converts_each_and_pins_nothing() {
        use ondin_core::LengthPct;
        for b_pinned in [true, false] {
            let (mut app, frame, a) = app_with_pinnable();
            let b = app.session.ids.mint();
            assert!(app.session.commit(Transaction(vec![create_rect(
                b,
                frame,
                1,
                50.0,
                (100.0, 100.0)
            )])));
            app.toggle_pin(&[a], Pin::Left, true);
            if b_pinned {
                app.toggle_pin(&[b], Pin::Left, true);
            }
            let (a0, b0) = (drawn(&app, a), drawn(&app, b));
            let ops = app.inset_flip_ops(&[a, b], Pin::Left, true);
            app.session.commit(Transaction(ops));
            let near =
                |r: KRect, s: KRect| (r.x0 - s.x0).abs() < 1e-9 && (r.x1 - s.x1).abs() < 1e-9;
            assert!(near(drawn(&app, a), a0), "A stays: {:?}", drawn(&app, a));
            assert!(near(drawn(&app, b), b0), "B stays: {:?}", drawn(&app, b));
            let left = |id| app.session.doc.get(id).unwrap().insets().left;
            assert!(
                matches!(left(a), Some(LengthPct::Percent(p)) if (p - 40.0 / 3.0).abs() < 1e-3)
            );
            if b_pinned {
                assert!(
                    matches!(left(b), Some(LengthPct::Percent(p)) if (p - 100.0 / 3.0).abs() < 1e-3),
                    "B converted at its own distance: {:?}",
                    left(b)
                );
            } else {
                assert_eq!(left(b), None, "an unpinned B is not pinned");
            }
        }
    }

    /// A pin keeps the layer where it is, then holds it to its edge when the frame is resized.
    #[test]
    fn a_pin_never_moves_the_layer_and_holds_it_through_a_frame_resize() {
        let (mut app, frame, rect) = app_with_pinnable();
        let before = drawn(&app, rect);
        app.toggle_pin(&[rect], Pin::Right, true);
        assert_eq!(drawn(&app, rect), before, "pinning moved it");
        assert_eq!(
            app.session.doc.get(rect).unwrap().insets().right,
            Some(ondin_core::LengthPct::Px(160.0)),
            "300 - 40 - 100"
        );
        resize_frame(&mut app, frame, 500.0);
        assert_eq!(
            drawn(&app, rect).x0,
            240.0,
            "and it followed the right edge"
        );
    }

    /// Unpinning after the frame has moved a pinned layer leaves it where it is drawn.
    ///
    /// flip: dropping `baked_placement` from `toggle_pin` → fails at the first assertion (40 against 240).
    #[test]
    fn unpinning_after_a_frame_resize_leaves_the_layer_where_it_is_drawn() {
        let (mut app, frame, rect) = app_with_pinnable();
        app.toggle_pin(&[rect], Pin::Right, true);
        resize_frame(&mut app, frame, 500.0);
        let pinned_at = drawn(&app, rect);
        app.toggle_pin(&[rect], Pin::Right, false);
        assert_eq!(drawn(&app, rect), pinned_at, "unpinning moved it");
        assert!(app.session.doc.get(rect).unwrap().insets().is_unset());
        app.session.undo();
        assert_eq!(
            app.session.doc.get(rect).unwrap().insets().right,
            Some(ondin_core::LengthPct::Px(160.0)),
            "and one undo brings the pin back"
        );
    }

    /// Unpinning one side of a stretch keeps the size the layer is drawn at.
    ///
    /// flip: removing `baked_placement` from `toggle_pin` → fails at the width assertion (100 against 300).
    #[test]
    fn unpinning_one_side_of_a_stretch_keeps_the_stretched_size() {
        let (mut app, frame, rect) = app_with_pinnable();
        app.toggle_pin(&[rect], Pin::Left, true);
        app.toggle_pin(&[rect], Pin::Right, true);
        resize_frame(&mut app, frame, 500.0);
        assert_eq!(drawn(&app, rect).width(), 300.0, "stretched by the frame");
        app.toggle_pin(&[rect], Pin::Right, false);
        assert_eq!(
            drawn(&app, rect).width(),
            300.0,
            "and still 300 wide with one side let go"
        );
    }

    /// Centring keeps the layer where it is, then follows the frame by half of every change (`margin: auto`).
    #[test]
    fn centring_keeps_the_layer_put_and_follows_the_frame_by_half() {
        let (mut app, frame, rect) = app_with_pinnable();
        let before = drawn(&app, rect);
        app.toggle_centre(&[rect], true, true);
        assert_eq!(drawn(&app, rect), before, "centring moved it");
        let insets = *app.session.doc.get(rect).unwrap().insets();
        assert!(insets.margin_auto.left && insets.margin_auto.right);
        resize_frame(&mut app, frame, 500.0);
        assert_eq!(drawn(&app, rect).x0, 140.0, "40 plus half of 200");
        app.toggle_centre(&[rect], true, false);
        assert!(
            app.session
                .doc
                .get(rect)
                .unwrap()
                .insets()
                .margin_auto
                .is_none(),
            "and switching it off takes the margins away"
        );
    }

    /// The card is offered to a frame's own layers and not to a top-level frame, and draws without panicking.
    #[test]
    fn the_card_is_offered_to_a_frames_own_layers_only() {
        let (mut app, frame, rect) = app_with_pinnable();
        assert_eq!(app.inset_subjects(), vec![rect]);
        app.session.selection.set(vec![frame]);
        assert!(
            app.inset_subjects().is_empty(),
            "a top-level frame is not inside one"
        );

        app.session.selection.set(vec![rect]);
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let _ = ctx.run_ui(Default::default(), |ui| {
            ui.set_max_width(240.0);
            app.inspector_insets(ui);
        });
    }

    /// The card is offered to a child of a group with a layout and pins it against the group's box.
    ///
    /// flip: deleting `inset_subjects`' group arm → fails at the first assertion; `inset_placement`'s → fails at "the top is pinned where it is".
    #[test]
    fn a_laid_groups_child_gets_the_card_and_pins_against_the_groups_box() {
        use ondin_core::container::{Display, Flex};
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0xAB);
        let root = ids.mint();
        let (group, item, pinned) = (ids.mint(), ids.mint(), ids.mint());
        let rect = |id, index, w, at: (f64, f64)| Operation::CreateNode {
            id,
            parent: group,
            index,
            kind: NodeKind::Rect {
                size: Size::new(w, 30.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: Some(Affine::translate(at)),
            name: None,
        };
        let mut doc = ondin_core::Document::new(root);
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: 0,
                kind: NodeKind::Group,
                transform: Some(Affine::translate((50.0, 50.0))),
                name: None,
            },
            rect(item, 0, 60.0, (0.0, 0.0)),
            rect(pinned, 1, 40.0, (0.0, 15.0)),
            Operation::SetDisplay {
                id: group,
                display: Some(Display::Flex(Flex {
                    padding: [20.0; 4],
                    ..Default::default()
                })),
            },
            Operation::SetInsets {
                id: pinned,
                insets: ondin_core::Insets {
                    right: Some(ondin_core::LengthPct::Px(0.0)),
                    ..Default::default()
                },
            },
        ]))
        .unwrap();
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![pinned]);
        assert_eq!(app.inset_subjects(), vec![pinned]);
        let before = drawn(&app, pinned);
        assert_eq!(
            before,
            KRect::new(110.0, 65.0, 150.0, 95.0),
            "the fixture: right 0 in a 100-wide box, y 15 from its transform"
        );

        app.toggle_pin(&[pinned], Pin::Top, true);
        assert_eq!(drawn(&app, pinned), before, "pinning moved it");
        assert_eq!(
            app.session.doc.get(pinned).unwrap().insets().top,
            Some(ondin_core::LengthPct::Px(15.0)),
            "the top is pinned where it is"
        );

        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let out = ctx.run_ui(Default::default(), |ui| {
            ui.set_max_width(284.0);
            app.inspector_insets(ui);
        });
        // A pin takes the layer out of the flow, so the header says Absolute.
        assert!(
            out.shapes.iter().any(|cs| matches!(
                &cs.shape,
                egui::epaint::Shape::Text(t) if t.galley.text() == "Absolute"
            )),
            "the badge, in a container with a layout"
        );
    }

    /// Pinning an item out of a hugging container never moves it: the inset is measured against the group's future box (−20, not 20).
    ///
    /// flip: `toggle_pin` committing its ops without `OndinApp::kept_in_place` → fails at "pinning moved it" (x 90 against 130).
    #[test]
    fn pinning_out_of_a_hugging_container_leaves_the_item_where_it_is() {
        use ondin_core::container::{Display, Flex};
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0xAD);
        let root = ids.mint();
        let (group, first, second) = (ids.mint(), ids.mint(), ids.mint());
        let rect = |id, index, w| Operation::CreateNode {
            id,
            parent: group,
            index,
            kind: NodeKind::Rect {
                size: Size::new(w, 30.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: Some(Affine::IDENTITY),
            name: None,
        };
        let mut doc = ondin_core::Document::new(root);
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: 0,
                kind: NodeKind::Group,
                transform: Some(Affine::translate((50.0, 50.0))),
                name: None,
            },
            rect(first, 0, 60.0),
            rect(second, 1, 40.0),
            Operation::SetDisplay {
                id: group,
                display: Some(Display::Flex(Flex {
                    padding: [20.0; 4],
                    ..Default::default()
                })),
            },
        ]))
        .unwrap();
        app.session.adopt_document(doc, None);
        assert_eq!(drawn(&app, group).width(), 140.0, "the fixture hugs");
        let before = drawn(&app, second);
        assert_eq!(before.x0, 130.0, "and lays the second item at 50 + 20 + 60");

        app.toggle_pin(&[second], Pin::Right, true);
        assert_eq!(
            drawn(&app, group).width(),
            100.0,
            "the group shrank without it"
        );
        assert_eq!(drawn(&app, second), before, "pinning moved it");
        assert_eq!(
            app.session.doc.get(second).unwrap().insets().right,
            Some(ondin_core::LengthPct::Px(-20.0)),
            "measured against the box the group has now"
        );
    }

    /// Centring an item out of a hugging container never moves it either (the pinning test's group).
    ///
    /// flip: `toggle_centre` committing without `kept_in_place` → fails at "centring moved it".
    #[test]
    fn centring_out_of_a_hugging_container_leaves_the_item_where_it_is() {
        use ondin_core::container::{Display, Flex};
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0xB0);
        let root = ids.mint();
        let (group, first, second) = (ids.mint(), ids.mint(), ids.mint());
        let rect = |id, index, w| Operation::CreateNode {
            id,
            parent: group,
            index,
            kind: NodeKind::Rect {
                size: Size::new(w, 30.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: Some(Affine::IDENTITY),
            name: None,
        };
        let mut doc = ondin_core::Document::new(root);
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: 0,
                kind: NodeKind::Group,
                transform: Some(Affine::translate((50.0, 50.0))),
                name: None,
            },
            rect(first, 0, 60.0),
            rect(second, 1, 40.0),
            Operation::SetDisplay {
                id: group,
                display: Some(Display::Flex(Flex {
                    padding: [20.0; 4],
                    ..Default::default()
                })),
            },
        ]))
        .unwrap();
        app.session.adopt_document(doc, None);
        let before = drawn(&app, second);
        assert_eq!(before.x0, 130.0, "the fixture");

        app.toggle_centre(&[second], true, true);
        assert_eq!(drawn(&app, group).width(), 100.0, "the group shrank");
        assert_eq!(drawn(&app, second), before, "centring moved it");
        let insets = *app.session.doc.get(second).unwrap().insets();
        assert!(
            insets.margin_auto.left && insets.margin_auto.right,
            "and it is centred the CSS way"
        );
    }

    /// The card layout: the diagram 74 wide spanning the two field rows, four fields beside it,
    /// `auto` on unpinned fields, the top held at its distance, no Absolute badge in a plain frame.
    ///
    /// flip: the `auto` formatter off, or the top never held → fails at "left and bottom, unpinned"; the badge's `laid` dropped → fails at "no badge in a plain frame".
    #[test]
    fn the_card_is_laid_out_as_the_mockup() {
        let (mut app, _frame, rect) = app_with_pinnable();
        app.toggle_pin(&[rect], Pin::Right, true);
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut out = ctx.run_ui(Default::default(), |_| {});
        for _ in 0..3 {
            out = ctx.run_ui(Default::default(), |ui| {
                ui.set_max_width(284.0);
                app.inspector_insets(ui);
            });
        }
        let texts: Vec<(String, egui::Rect)> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Text(t) => Some((
                    t.galley.text().to_owned(),
                    t.galley.rect.translate(t.pos.to_vec2()),
                )),
                _ => None,
            })
            .collect();
        let grounds: Vec<egui::Rect> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Rect(r) if r.fill == color::FIELD => Some(r.rect),
                _ => None,
            })
            .collect();
        let well = *grounds
            .iter()
            .find(|r| r.width() == INSET_DIAGRAM_W)
            .expect("the diagram's well");
        let fields: Vec<egui::Rect> = grounds
            .iter()
            .copied()
            // Beside the well: the second centre button is also a 28-tall ground.
            .filter(|r| {
                r.height() == ui::CONTROL_H && r.left() > well.right() && r.top() < well.bottom()
            })
            .collect();
        assert_eq!(
            fields.len(),
            4,
            "four fields beside the diagram: {fields:?}"
        );
        let top = fields.iter().map(|r| r.top()).fold(f32::MAX, f32::min);
        let bottom = fields.iter().map(|r| r.bottom()).fold(f32::MIN, f32::max);
        assert_eq!(
            (well.top(), well.bottom()),
            (top, bottom),
            "spanning both rows"
        );
        let word = |w: &str| texts.iter().filter(|(t, _)| t == w).count();
        assert_eq!(word("auto"), 2, "left and bottom, unpinned");
        assert_eq!(
            word("30"),
            1,
            "the top, held by default at its distance (§15 D891)"
        );
        assert_eq!(
            (word("160"), word("px")),
            (1, 1),
            "the pinned right, 300 - 40 - 100"
        );
        assert_eq!(
            word("Absolute"),
            0,
            "no badge in a plain frame, where everything is absolute (§15 D891)"
        );
    }

    /// A point where nothing in the card is drawn; a click here takes focus away from a field.
    const AWAY: egui::Pos2 = egui::pos2(5.0, 850.0);

    /// One frame of the Position card at the inspector column's 284, with `events`; every text run painted, with its rect.
    fn card_frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        time: &mut f64,
        events: Vec<egui::Event>,
    ) -> Vec<(String, egui::Rect)> {
        *time += 0.1;
        let out = ctx.run_ui(
            egui::RawInput {
                time: Some(*time),
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 900.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                ui.set_max_width(284.0);
                app.inspector_insets(ui);
            },
        );
        out.shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Text(t) => Some((
                    t.galley.text().to_owned(),
                    t.galley.rect.translate(t.pos.to_vec2()),
                )),
                _ => None,
            })
            .collect()
    }

    /// A press and a release at `at`, and a frame after for focus to settle.
    fn click_card(ctx: &egui::Context, app: &mut OndinApp, time: &mut f64, at: egui::Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        card_frame(ctx, app, time, vec![egui::Event::PointerMoved(at)]);
        card_frame(ctx, app, time, vec![button(true)]);
        card_frame(ctx, app, time, vec![button(false)]);
        card_frame(ctx, app, time, Vec::new());
    }

    /// The centre of the run reading `text` in the field whose prefix is `label`:
    /// on the prefix's row and right of it, the nearest such.
    fn in_field(runs: &[(String, egui::Rect)], label: &str, text: &str) -> egui::Pos2 {
        let prefix = runs
            .iter()
            .find(|(t, _)| t == label)
            .unwrap_or_else(|| panic!("no {label:?} prefix in {runs:?}"))
            .1;
        runs.iter()
            .filter(|(t, r)| {
                t == text
                    && (r.center().y - prefix.center().y).abs() < 3.0
                    && r.left() > prefix.right()
            })
            .min_by(|a, b| a.1.left().total_cmp(&b.1.left()))
            .unwrap_or_else(|| panic!("no {text:?} in the {label} field: {runs:?}"))
            .1
            .center()
    }

    /// An inset field only clicked through pins nothing (`inset_field`'s guard `set.is_none() && v == shown`).
    /// Each click is asserted to have focused the field, or a miss would pass with the guard gone.
    ///
    /// flip: guard made `false && set.is_none() && v == shown` → fails at "a click through R pinned nothing" (`right: Some(Px(160.0))`).
    #[test]
    fn clicking_through_an_unpinned_inset_field_pins_nothing() {
        let (mut app, _frame, rect) = app_with_pinnable();
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut time = 0.0;
        let before = drawn(&app, rect);
        let depth = app.session.history.undo_depth();
        let mut runs = Vec::new();
        for _ in 0..3 {
            runs = card_frame(&ctx, &mut app, &mut time, Vec::new());
        }
        for (label, shown) in [("R", "auto"), ("L", "40")] {
            let at = in_field(&runs, label, shown);
            click_card(&ctx, &mut app, &mut time, at);
            assert!(
                ctx.memory(|m| m.focused()).is_some(),
                "the fixture: clicking {label} focused it"
            );
            click_card(&ctx, &mut app, &mut time, AWAY);
            assert!(
                ctx.memory(|m| m.focused()).is_none(),
                "the fixture: clicking away let {label} go"
            );
            let insets = *app.session.doc.get(rect).unwrap().insets();
            assert!(
                insets.is_unset(),
                "a click through {label} pinned nothing: {insets:?}"
            );
            assert_eq!(app.session.history.undo_depth(), depth, "and left no step");
            assert_eq!(drawn(&app, rect), before, "and moved nothing");
            runs = card_frame(&ctx, &mut app, &mut time, Vec::new());
        }
    }

    /// A number typed into an unpinned inset field pins it in one step, and the unit suffix then converts it in place.
    ///
    /// flip: guard cut to `set.is_none()` → fails at "typed into R"; the suffix arm made `if false && flip && …` → fails at "the suffix converted R".
    #[test]
    fn a_typed_inset_pins_it_and_its_unit_suffix_converts_it() {
        use ondin_core::LengthPct;
        let (mut app, _frame, rect) = app_with_pinnable();
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut time = 0.0;
        let depth = app.session.history.undo_depth();
        let mut runs = Vec::new();
        for _ in 0..3 {
            runs = card_frame(&ctx, &mut app, &mut time, Vec::new());
        }
        click_card(&ctx, &mut app, &mut time, in_field(&runs, "R", "auto"));
        assert!(
            ctx.memory(|m| m.focused()).is_some(),
            "the fixture: clicking R focused it"
        );
        for ch in "20".chars() {
            card_frame(
                &ctx,
                &mut app,
                &mut time,
                vec![egui::Event::Text(ch.into())],
            );
        }
        click_card(&ctx, &mut app, &mut time, AWAY);
        let right = |app: &OndinApp| app.session.doc.get(rect).unwrap().insets().right;
        assert_eq!(right(&app), Some(LengthPct::Px(20.0)), "typed into R");
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
        let pinned = drawn(&app, rect);
        assert_eq!(pinned.x0, 180.0, "and placed by it");

        for _ in 0..3 {
            runs = card_frame(&ctx, &mut app, &mut time, Vec::new());
        }
        click_card(&ctx, &mut app, &mut time, in_field(&runs, "R", "px"));
        assert!(
            matches!(right(&app), Some(LengthPct::Percent(p)) if (p - 20.0 / 3.0).abs() < 1e-9),
            "the suffix converted R: {:?}",
            right(&app)
        );
        assert_eq!(
            app.session.history.undo_depth(),
            depth + 2,
            "in one step of its own"
        );
        let after = drawn(&app, rect);
        assert!(
            (after.x0 - pinned.x0).abs() < 1e-9 && (after.x1 - pinned.x1).abs() < 1e-9,
            "and nothing moved: {after:?}"
        );
    }
}

#[cfg(test)]
mod multi_xy_flex_tests {
    //! The multi-selection Transform card's X and Y over flex items.
    use super::*;

    /// X and Y do nothing to a selection mixing an in-flow item with a free rect (no move, no undo step).
    ///
    /// flip: `inspector_multi_transform`'s `ui::disable_unless(ui, laid_out.is_none(), …)` made `true` → fails at "nothing moved";
    /// `laid_out_xy` answering `None` → fails there too.
    #[test]
    fn x_and_y_are_inert_over_a_selection_with_an_in_flow_item() {
        use ondin_core::container::{AlignItems, Display, Flex};
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0xAF);
        let root = ids.mint();
        let (row, a, b) = (ids.mint(), ids.mint(), ids.mint());
        let rect = |id, parent, index, w, at: (f64, f64)| Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Rect {
                size: Size::new(w, 30.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: Some(Affine::translate(at)),
            name: None,
        };
        let mut doc = ondin_core::Document::new(root);
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: row,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 200.0),
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
            rect(a, row, 0, 40.0, (0.0, 0.0)),
            rect(b, root, 1, 60.0, (500.0, 300.0)),
            Operation::SetDisplay {
                id: row,
                display: Some(Display::Flex(Flex {
                    column_gap: 10.0,
                    padding: [30.0, 20.0, 30.0, 20.0],
                    align_items: AlignItems::Start,
                    ..Default::default()
                })),
            },
        ]))
        .unwrap();
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![a, b]);
        let before = (
            app.session.doc.get(a).unwrap().transform(),
            app.session.doc.get(b).unwrap().transform(),
        );
        let depth = app.session.history.undo_depth();

        let mut time = 0.0;
        let mut frame = |app: &mut OndinApp, events: Vec<egui::Event>| {
            time += 0.1;
            ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200.0, 900.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.set_max_width(284.0);
                    app.inspector_multi_transform(ui);
                },
            )
        };
        let mut out = frame(&mut app, Vec::new());
        for _ in 0..2 {
            out = frame(&mut app, Vec::new());
        }
        let from = out
            .shapes
            .iter()
            .find_map(|cs| match &cs.shape {
                egui::epaint::Shape::Text(t) if t.galley.text() == "20" => {
                    Some(t.galley.rect.translate(t.pos.to_vec2()).center())
                }
                _ => None,
            })
            .expect("X reads 20");
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(
            &mut app,
            vec![egui::Event::PointerMoved(from), button(from, true)],
        );
        for step in 1..=4 {
            let at = from + egui::vec2(20.0 * step as f32, 0.0);
            frame(&mut app, vec![egui::Event::PointerMoved(at)]);
        }
        let at = from + egui::vec2(80.0, 0.0);
        frame(&mut app, vec![button(at, false)]);
        for _ in 0..3 {
            frame(&mut app, Vec::new());
        }
        assert_eq!(
            (
                app.session.doc.get(a).unwrap().transform(),
                app.session.doc.get(b).unwrap().transform(),
            ),
            before,
            "nothing moved"
        );
        assert_eq!(app.session.history.undo_depth(), depth, "and no step");
    }

    /// The X/Y tooltip promises a reorder only where a drag makes one: `drags_as_a_block` decides and
    /// `laid_out_xy` words it (asserted directly: a tooltip's text never reaches a headless frame).
    ///
    /// flip: dropping the same-container clause → fails at "items of two rows"; the in-flow clause → fails only at "two free rects";
    /// `laid_out_xy` asking the opposite of `drags_as_a_block` → fails at "one row's items are told to drag".
    #[test]
    fn only_one_containers_items_are_told_to_drag_to_reorder() {
        use ondin_core::container::{Display, Flex};
        let mut ids = ondin_core::IdSource::new(0xAE);
        let root = ids.mint();
        let (row1, row2, a1, a2, b1, free, free2) = (
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
        );
        let node = |id, parent, index, kind| Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: None,
            name: None,
        };
        let rect = || NodeKind::Rect {
            size: Size::new(20.0, 20.0),
            corner_radii: RoundedRectRadii::default(),
        };
        let frame = || NodeKind::Artboard {
            size: Size::new(200.0, 100.0),
        };
        let flex = |id| Operation::SetDisplay {
            id,
            display: Some(Display::Flex(Flex::default())),
        };
        let mut doc = ondin_core::Document::new(root);
        doc.apply(&Transaction(vec![
            node(row1, root, 0, frame()),
            node(row2, root, 1, frame()),
            node(free, root, 2, rect()),
            node(free2, root, 3, rect()),
            node(a1, row1, 0, rect()),
            node(a2, row1, 1, rect()),
            node(b1, row2, 0, rect()),
            flex(row1),
            flex(row2),
        ]))
        .unwrap();
        assert!(drags_as_a_block(&doc, &[a1, a2]), "one row's items");
        assert!(drags_as_a_block(&doc, &[a1]), "one item");
        assert!(
            !drags_as_a_block(&doc, &[a1, free]),
            "an item and a free rect"
        );
        assert!(!drags_as_a_block(&doc, &[a1, b1]), "items of two rows");
        assert!(
            !drags_as_a_block(&doc, &[free, free2]),
            "two free rects, one parent and no layout"
        );
        assert!(!drags_as_a_block(&doc, &[]), "nothing");

        // What the card says, through the function it calls for its gate and hover.
        let said = |m: &[NodeId]| laid_out_xy(&doc, m);
        assert_eq!(
            said(&[a1, a2]).as_deref(),
            Some(
                "2 of these are placed by a container's layout — drag them on the canvas to reorder"
            ),
            "one row's items are told to drag"
        );
        assert_eq!(
            said(&[a1, free]).as_deref(),
            Some(
                "1 of these is placed by a container's layout — select a container's items on \
                 their own to reorder them by dragging"
            ),
            "a mixed selection is not"
        );
        assert_eq!(said(&[free, free2]), None, "no laid member: X and Y are on");
    }
}

#[cfg(test)]
mod effect_reorder_tests {
    use super::*;

    /// An Effects row carried below the last one lands at the bottom of the stack on
    /// release, in one undo step, and a picker on its shadow colour follows it; the
    /// stack reads top-first, so the first effect becomes the last.
    /// flip: `effect_order` built with `paint::display_order` (bottom-first) → fails
    /// at the order.
    #[test]
    fn an_effect_row_dragged_to_the_bottom_lands_last() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0xEF0);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let id = ids.mint();
        let kinds = EffectKind::all_defaults();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 100.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            },
            Operation::SetEffects {
                id,
                effects: ondin_core::keyed_by_position(kinds[..3].iter().cloned().map(Effect::new)),
            },
        ]))
        .expect("three effects");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        let labels = |app: &OndinApp| -> Vec<&'static str> {
            app.session
                .doc
                .get(id)
                .unwrap()
                .effects()
                .iter()
                .map(|e| e.kind.label())
                .collect()
        };
        let before = labels(&app);
        let frame = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 2400.0),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                ui.set_max_width(296.0);
                app.inspector_ui(ui)
            });
        };
        for _ in 0..3 {
            frame(&mut app, vec![]);
        }
        // As the first row's grip leaves it on the press, its shadow's picker open.
        app.picker = Some(crate::panels::Picker::new(
            id,
            PaintSlot::Effect(0),
            egui::pos2(900.0, 300.0),
        ));
        app.in_flight.effect_drag = Some(paint::EffectDrag {
            anchor: id,
            from: 0,
            to: 0,
            slots: paint::Slots::default(),
        });
        let low = egui::pos2(100.0, 2300.0);
        let button = |pressed| egui::Event::PointerButton {
            pos: low,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut app, vec![egui::Event::PointerMoved(low), button(true)]);
        frame(&mut app, vec![]);
        let depth = app.session.history.undo_depth();
        frame(&mut app, vec![button(false)]);
        assert!(
            app.in_flight.effect_drag.is_none(),
            "the release ends the carry"
        );
        assert_eq!(
            labels(&app),
            vec![before[1], before[2], before[0]],
            "the first effect is now last"
        );
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one undo step");
        assert_eq!(
            app.picker.as_ref().map(|p| p.slot),
            Some(PaintSlot::Effect(2)),
            "the picker follows its effect"
        );
    }
}
