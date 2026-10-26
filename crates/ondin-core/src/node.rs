//! Node model (§5.3–5.4).
//!
//! Node fields are private (invariant 2): the only way to mutate is via
//! `Document::apply`. Geometry is authored in the node's LOCAL space; the
//! `transform` positions it under its parent (invariant 5/6). Read access is
//! through the getters below.

use crate::id::NodeId;
use crate::image::Brush;
use crate::item::Keyed;
use kurbo::{Affine, BezPath, Point, RoundedRectRadii, Size, Vec2};
use serde::{Deserialize, Serialize};

// The text attribute model lives in `typography.rs` (§5.4); re-exported so
// `node::TextStyle` names what a text node carries.
pub use crate::typography::{
    BlockStyle, BoxTrim, CharAttr, CharAttrKind, CharSpan, CharSpans, Decoration, JustifyLast,
    Length, LengthUnit, LineStyle, OverflowWrap, ParaAttr, ParaAttrKind, ParaSpan, ParaSpans,
    ParagraphStyle, Span, SpanAttr, Spans, TextAlign, TextCase, TextDirection, TextOverflow,
    TextStyle, VerticalAlign, WordBreak, WrapMode,
};

/// A single node in the document tree.
///
/// **Constructed by struct literal, inside this crate only** — the fields are
/// `pub(crate)` (invariant 2) and there is deliberately no constructor: a
/// named-field literal is the parts struct a long positional constructor would
/// need, and a new field still breaks every site.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub(crate) id: NodeId,
    pub(crate) parent: Option<NodeId>,
    /// Ordered = z-order (last = topmost within parent).
    pub(crate) children: Vec<NodeId>,
    pub(crate) kind: NodeKind,
    /// LOCAL to parent. Translation, rotation, skew and flip — never scale
    /// (§5.6).
    ///
    /// Scale is kept out so a layer's size is one number in one place (its
    /// geometry) and a stroke does not thicken when a group is resized.
    /// `build::Basis` enforces the split.
    pub(crate) transform: Affine,
    pub(crate) name: String,
    pub(crate) visible: bool,
    pub(crate) locked: bool,
    /// Whether resizing holds the aspect ratio (`canvas::keep_ratio`, the
    /// Transform panel's W and H). Independent of `locked`, which refuses edits.
    pub(crate) proportions_locked: bool,
    /// 0.0..=1.0.
    pub(crate) opacity: f32,
    /// Whether this node clips what its children draw to its own frame.
    /// Node-level like `visible`/`locked`; ignored by kinds with no frame.
    pub(crate) clip: bool,
    /// Whether this node is a **mask**: it draws nothing itself, and its outline
    /// clips the *run* of siblings painted above it, up to the next mask. Unlike
    /// [`Self::clip`], which cuts children; a frame cannot do it
    /// ([`NodeKind::can_mask`]).
    pub(crate) mask: bool,
    /// *How* this node masks. A field beside the flag so the mode survives the
    /// mask being switched off and on; inert while `mask` is false.
    pub(crate) mask_mode: MaskMode,
    /// How this node's geometry decides its own interior (§15 D239). On the node
    /// rather than on a [`Fill`]: the hit test and the mask ask it, and a node
    /// with no fill still has an inside.
    pub(crate) fill_rule: FillRule,
    /// Meaningful for shape/text kinds; empty for Root/Group/Artboard.
    pub(crate) paint: Paint,
    /// The effect stack (§5.3a), in compositing order.
    ///
    /// A node field rather than part of [`Paint`] because it is meaningful on a
    /// container. It affects what is drawn, so `Operation::SetEffects` answers
    /// `true` to `changes_ink`, and an effect reaching outside its layer widens
    /// the cached bounds ([`crate::effect::stack_escape`]).
    pub(crate) effects: Vec<Keyed<crate::effect::Effect>>,
    /// Where this node's transforms turn and mirror about; `None` is the centre
    /// of its own box, so "not moved" is a state the model can say.
    pub(crate) pivot: Option<Pivot>,
    /// The files this layer produces — the Export panel's list (§7). Read only
    /// by the export path, so `Operation::SetExports` answers `false` to
    /// `changes_ink`.
    pub(crate) exports: Vec<Keyed<crate::export::ExportSpec>>,
    /// The columns and rows drawn over this layer (`crate::layout`); chrome, so
    /// `Operation::SetLayoutGrids` answers `false` to `changes_ink`. Offered only
    /// on a frame: a group's box is derived from its contents, so a grid on it
    /// would move whenever a child did.
    pub(crate) grids: Vec<Keyed<crate::layout::LayoutGrid>>,
    /// This layer's CSS insets and auto margins inside its frame
    /// (`crate::container`). **Specified, never used**: where the layer is drawn
    /// is derived in [`crate::Resolved`]. Inert outside a frame.
    pub(crate) insets: crate::container::Insets,
    /// How this layer lays out its children — CSS `display` (`crate::container`).
    /// Read only on a frame or a group.
    pub(crate) display: Option<crate::container::Display>,
    /// This layer's properties as a layout item (`crate::container::LayoutItem`).
    /// Read only when its parent has a `display`.
    pub(crate) item: crate::container::LayoutItem,
    /// Whether this layer is a **main component** (§5.3d), the source its
    /// instances are copies of. Only on an `Artboard` or a `Group` that is not
    /// itself linked or inside a main or instance (`crate::component::check`).
    pub(crate) component: bool,
    /// The node this one was **copied from**, when part of an instance (§5.3d):
    /// a main's node, or a node of a nested instance inside an outer main.
    /// Checked after the last op of every transaction (`crate::component::check`).
    pub(crate) link: Option<NodeId>,
    /// The main whose **contents** this nested copy shows instead of its link's
    /// (an instance swap). [`Self::link`] keeps the slot's identity, so placement,
    /// size and visibility come from the slot (`crate::swap`).
    pub(crate) swap: Option<NodeId>,
    /// This frame's **variant properties**, when it is a component set (§5.3d);
    /// every main directly inside it is a variant.
    pub(crate) set: Option<crate::variant::VariantSet>,
    /// This main's **values**, one per property of its set; the variant's name
    /// is derived from them (`crate::variant::settle`).
    pub(crate) variant: Vec<String>,
    /// The **component properties** this main or set defines. Empty on a
    /// variant, whose properties are its set's.
    pub(crate) props: Vec<Keyed<crate::variant::Property>>,
    /// What this main or set is for, in the user's words; empty is none. A copy
    /// never keeps it: an instance reads its main's.
    pub(crate) description: String,
    /// Where this node was **vendored from**, when part of a kit main in the
    /// document's vault (§5.3e). A copy never keeps it (`remap_subtree`); only
    /// the kit verbs write one (`crate::kit::check`).
    pub(crate) origin: Option<crate::kit::Origin>,
    /// The **mode** this frame sets for itself and everything under it (§5.3f);
    /// `None` inherits. Only on an `Artboard` (`crate::variable::check`).
    pub(crate) mode: Option<crate::variable::ModeId>,
    /// The variables this rectangle's corners are bound to.
    pub(crate) corner_vars: crate::variable::CornerVars,
    /// The variables this container's gaps and padding are bound to.
    pub(crate) spacing_vars: crate::variable::SpacingVars,
    /// The boolean variable this layer's visibility is bound to; `visible` is
    /// its literal, never read while bound.
    pub(crate) visible_var: Option<crate::variable::VarId>,
    /// The number variables this layer's width and height are bound to.
    pub(crate) size_vars: crate::variable::SizeVars,
    /// The string variables this text's content and family are bound to.
    pub(crate) text_vars: crate::variable::TextVars,
    /// The number variable this layer's opacity is bound to, read as a
    /// percentage; `opacity` is its literal, 0 while bound and never drawn.
    pub(crate) opacity_var: Option<crate::variable::VarId>,
    /// The styles this layer's fills, strokes and effects are bound to (§5.3f) —
    /// the lists hold the style's values, written there by the commit
    /// (`crate::style::settle`).
    pub(crate) styles: crate::style::StyleRefs,
}

impl Node {
    pub fn id(&self) -> NodeId {
        self.id
    }
    /// Whether this layer is a main component.
    pub fn component(&self) -> bool {
        self.component
    }
    /// The node this one was copied from, if it is part of an instance.
    pub fn link(&self) -> Option<NodeId> {
        self.link
    }
    /// The main this nested copy's contents are swapped to, if any.
    pub fn swap(&self) -> Option<NodeId> {
        self.swap
    }
    /// Make this freshly remapped node a **linked copy** of `source`, clearing
    /// everything main-only (and any swap) in one place for every door that makes
    /// an instance node.
    pub(crate) fn make_copy_of(&mut self, source: NodeId) {
        self.link = Some(source);
        self.swap = None;
        self.component = false;
        self.set = None;
        self.variant.clear();
        self.props.clear();
        self.description.clear();
    }
    /// What this main or set is for, in the user's words; empty is none.
    pub fn description(&self) -> &str {
        &self.description
    }
    /// The variant properties, if this frame is a component set.
    pub fn set(&self) -> Option<&crate::variant::VariantSet> {
        self.set.as_ref()
    }
    /// This variant's values, one per property of its set; empty unless it is one.
    pub fn variant(&self) -> &[String] {
        &self.variant
    }
    /// The component properties this main or set defines.
    pub fn props(&self) -> &[Keyed<crate::variant::Property>] {
        &self.props
    }
    /// Where this node was vendored from, when it is in the vault.
    pub fn origin(&self) -> Option<&crate::kit::Origin> {
        self.origin.as_ref()
    }
    /// The mode this frame sets, if any (§5.3f) — not the mode in force here,
    /// which is [`crate::Document::mode_at`].
    pub fn mode(&self) -> Option<crate::variable::ModeId> {
        self.mode
    }
    /// The variables this rectangle's corners are bound to.
    pub fn corner_vars(&self) -> crate::variable::CornerVars {
        self.corner_vars
    }
    /// The variables this container's gaps and padding are bound to.
    pub fn spacing_vars(&self) -> crate::variable::SpacingVars {
        self.spacing_vars
    }
    /// The boolean variable this layer's visibility is bound to; whether it is
    /// shown is [`crate::Document::shown`], not [`Self::visible`].
    pub fn visible_var(&self) -> Option<crate::variable::VarId> {
        self.visible_var
    }
    /// The variables this layer's width and height are bound to; the size drawn
    /// is [`crate::Document::resolved_kind`]'s, the kind's `size` the literal.
    pub fn size_vars(&self) -> crate::variable::SizeVars {
        self.size_vars
    }
    /// The variables this text's content and family are bound to; what is drawn
    /// is [`crate::Document::resolved_kind`]'s.
    pub fn text_vars(&self) -> crate::variable::TextVars {
        self.text_vars
    }
    /// The variable this layer's opacity is bound to; the opacity drawn is
    /// [`crate::Document::drawn_opacity`]'s, [`Self::opacity`] the literal.
    pub fn opacity_var(&self) -> Option<crate::variable::VarId> {
        self.opacity_var
    }
    /// The styles this layer's cards are bound to.
    pub fn styles(&self) -> crate::style::StyleRefs {
        self.styles
    }
    /// Whether any field of this node reads a variable; what a variable's edit
    /// dirties (`Document::bound_nodes`).
    pub fn reads_variables(&self) -> bool {
        self.paint.fills.iter().any(|f| f.brush.is_bound())
            || self
                .paint
                .strokes
                .iter()
                .any(|s| s.brush.is_bound() || s.width_var.is_some())
            || self.corner_vars.iter().any(Option::is_some)
            || self.spacing_vars.iter().any(Option::is_some)
            || self.visible_var.is_some()
            || self.size_vars.iter().any(Option::is_some)
            || self.text_vars.iter().any(Option::is_some)
            || self.item.limit_vars.iter().any(Option::is_some)
            || self.opacity_var.is_some()
            || self
                .effects
                .iter()
                .any(|e| e.vars.iter().any(Option::is_some))
    }
    /// Refuse a node whose keyed lists (`props` included) repeat an item id
    /// within one list — `Document::apply`'s post-condition and the loader's
    /// check. Across lists and nodes an id may repeat. Files saved with a repeat
    /// are re-keyed on load (`item::rekey_repeats`).
    pub(crate) fn check_item_ids(&self) -> Result<(), crate::op::OpError> {
        use crate::item::first_duplicate;
        let dup = first_duplicate(&self.paint.fills)
            .or_else(|| first_duplicate(&self.paint.strokes))
            .or_else(|| first_duplicate(&self.effects))
            .or_else(|| first_duplicate(&self.exports))
            .or_else(|| first_duplicate(&self.grids))
            .or_else(|| first_duplicate(&self.props));
        match dup {
            Some(item) => Err(crate::op::OpError::DuplicateItemId(self.id, item)),
            None => Ok(()),
        }
    }
    pub fn parent(&self) -> Option<NodeId> {
        self.parent
    }
    /// Children in z-order (last = topmost within parent).
    pub fn children(&self) -> &[NodeId] {
        &self.children
    }
    pub fn kind(&self) -> &NodeKind {
        &self.kind
    }
    /// Local transform (relative to parent).
    pub fn transform(&self) -> Affine {
        self.transform
    }
    /// This node with `transform` in place of its own, everything else the same
    /// — for comparing a node's content wherever it sits. Not a write: the
    /// document is changed by ops only.
    pub fn with_transform(&self, transform: Affine) -> Node {
        Node {
            transform,
            ..self.clone()
        }
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    /// The stored visibility — **the literal**, which a layer bound to a variable
    /// does not draw by. Whether it is drawn is [`crate::Document::shown`].
    pub fn visible(&self) -> bool {
        self.visible
    }
    pub fn locked(&self) -> bool {
        self.locked
    }
    /// Whether a resize of this node holds its aspect ratio — the Transform
    /// panel's chain link. Not [`Self::locked`]: that one refuses the edit,
    /// this one shapes it.
    pub fn proportions_locked(&self) -> bool {
        self.proportions_locked
    }
    /// The literal opacity — 0 while [`Self::opacity_var`] binds it; what the
    /// layer is drawn at is [`crate::Document::drawn_opacity`].
    pub fn opacity(&self) -> f32 {
        self.opacity
    }
    /// Whether this node clips its children to its own frame. Only containers
    /// with a frame — artboards — act on it today.
    pub fn clip(&self) -> bool {
        self.clip
    }
    /// Whether this node is a mask: it paints nothing, and its outline clips the
    /// run of siblings above it. Not [`Self::clip`], which is about children.
    pub fn mask(&self) -> bool {
        self.mask
    }
    /// How this node masks. Meaningless — but remembered — while
    /// [`Self::mask`] is false.
    pub fn mask_mode(&self) -> MaskMode {
        self.mask_mode
    }
    /// How this node's geometry decides its interior (§15 D239) — the **effective**
    /// rule, which for one kind is not the stored one.
    ///
    /// A `Boolean` with [`BoolOp::Exclude`] is always even-odd, whatever the
    /// field says: its outline is its operands concatenated, a symmetric
    /// difference only when read even-odd. The stored field is what a *user* set
    /// and [`crate::io`] writes; every consumer asks this.
    pub fn fill_rule(&self) -> FillRule {
        match self.kind {
            NodeKind::Boolean {
                op: BoolOp::Exclude,
            } => FillRule::EvenOdd,
            _ => self.fill_rule,
        }
    }
    pub fn paint(&self) -> &Paint {
        &self.paint
    }
    /// The effect stack, in compositing order.
    pub fn effects(&self) -> &[Keyed<crate::effect::Effect>] {
        &self.effects
    }
    /// Where this node's transforms pivot, or `None` if it still sits on the
    /// centre of the node's box. Resolve it to a point with
    /// [`crate::geometry::pivot_point`].
    pub fn pivot(&self) -> Option<Pivot> {
        self.pivot
    }
    /// The export specs this layer carries, in the order the panel shows them.
    pub fn exports(&self) -> &[Keyed<crate::export::ExportSpec>] {
        &self.exports
    }
    /// The layout grids drawn over this layer, in the order the panel shows
    /// them (`crate::layout`).
    pub fn grids(&self) -> &[Keyed<crate::layout::LayoutGrid>] {
        &self.grids
    }
    /// This layer's insets inside its frame (`crate::container`) — the specified
    /// values; where they put it is [`crate::Resolved::used_local`]'s answer.
    pub fn insets(&self) -> &crate::container::Insets {
        &self.insets
    }
    /// How this layer lays out its children (`crate::container::Display`), or
    /// `None` for a container whose children are placed by their own transforms.
    pub fn display(&self) -> Option<&crate::container::Display> {
        self.display.as_ref()
    }
    /// This layer's properties as a layout item — the specified values.
    pub fn item(&self) -> &crate::container::LayoutItem {
        &self.item
    }
}

/// How a [`Node::mask`] decides what survives of the layers it masks.
///
/// One mode asks *where* the mask is, the other *how much of it* is there.
///
/// **Luminance** (brightness is the amount) is absent because the GPU backend
/// cannot express it: vello's `push_layer` takes no mask and no compose or blend
/// mode moves colour into alpha, while `vello_cpu` could. Shipping it would make
/// the canvas and the export disagree, which §3 exists to prevent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaskMode {
    /// The mask's **outline** clips: a hard edge, and everything inside it
    /// survives whole. Figma's *vector mask*.
    ///
    /// The default, and the cheaper of the two — one clip layer, where `Alpha`
    /// needs two and an offscreen composite.
    #[default]
    Shape,
    /// The mask's own **alpha** is the amount that survives, per pixel; its
    /// colour is not read.
    Alpha,
}

/// How a path's interior is decided where it crosses itself or another subpath
/// (§15 D239).
///
/// Non-zero counts *turns* about the point, even-odd counts *crossings* to
/// infinity; they differ where a subpath overlaps another wound the same way.
///
/// It is what makes a boolean `Exclude` exact: a symmetric difference is the
/// even-odd reading of its operands concatenated, so no path arithmetic is
/// needed (the flo_curves fold is visibly wrong on a ring of overlapping circles).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FillRule {
    /// A point is inside when the path winds about it a non-zero number of times.
    /// Two overlapping subpaths wound the same way fill solid.
    ///
    /// The default, which keeps the field out of files that never set it.
    #[default]
    NonZero,
    /// A point is inside when a ray from it crosses the path an odd number of times.
    /// Overlapping subpaths cancel where they meet, and a hole is a hole whatever
    /// its winding.
    EvenOdd,
}

impl FillRule {
    /// The label a menu or a dropdown shows.
    pub fn label(self) -> &'static str {
        match self {
            FillRule::NonZero => "Non-zero",
            FillRule::EvenOdd => "Even-odd",
        }
    }

    /// Whether `path` contains `p`, under this rule.
    ///
    /// `BezPath::contains` is always non-zero, so hit-testing a boolean with it
    /// under an even-odd fill would be clickable in holes it does not draw.
    /// Even-odd is the parity of the same winding number, so no second traversal.
    /// The winding is `geometry::winding`'s, not kurbo's, which misreads points
    /// near some curves.
    pub fn contains(self, path: &BezPath, p: Point) -> bool {
        let w = crate::geometry::winding(path, p);
        match self {
            FillRule::NonZero => w != 0,
            FillRule::EvenOdd => w % 2 != 0,
        }
    }
}

impl MaskMode {
    /// The label a menu or a dropdown shows.
    pub fn label(self) -> &'static str {
        match self {
            MaskMode::Shape => "Shape",
            MaskMode::Alpha => "Alpha",
        }
    }

    /// One line saying what the mode does, for the control that offers it.
    pub fn describe(self) -> &'static str {
        match self {
            MaskMode::Shape => "The mask's outline clips, with a hard edge",
            MaskMode::Alpha => "The mask's own transparency fades what it masks",
        }
    }

    pub const ALL: [MaskMode; 2] = [MaskMode::Shape, MaskMode::Alpha];
}

/// A node's transform origin, in the node's own local space.
///
/// **Normalized where the box is authored, absolute where it is emergent.** A
/// rectangle's pivot stays proportionally placed when its size changes. A group's
/// box is the union of its children and a line's is degenerate, so there is no
/// authored extent for a fraction to be *of*; those kinds store the point itself.
///
/// [`crate::geometry::pivot_for`] decides which variant a kind gets, once, from
/// `resizable_size`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Pivot {
    /// A fraction of the node's own box: `(0, 0)` its top-left, `(1, 1)` its
    /// bottom-right, `(0.5, 0.5)` the centre `None` already means.
    Normalized(Vec2),
    /// A point in the node's local space, unscaled by anything.
    Local(Point),
}

impl Pivot {
    /// Invariant 8's predicate for this field (§15 D639).
    ///
    /// The operation layer must refuse a non-finite pivot: `serde_json` writes
    /// `NaN` as `null` and then refuses `null` where an `f64` is wanted, so such a
    /// document saves cleanly and never opens again. The downstream division is
    /// already floored; what nothing else guards is a coordinate that was never
    /// finite, from a singular world transform (`canvas::pivot_at`) or a
    /// hand-authored write.
    #[must_use]
    pub fn is_finite(&self) -> bool {
        match self {
            Self::Normalized(v) => v.x.is_finite() && v.y.is_finite(),
            Self::Local(p) => p.is_finite(),
        }
    }
}

/// Which boolean a [`NodeKind::Boolean`] container performs on its children.
///
/// **Order is layer order** for the non-commutative `Subtract`: the bottom child
/// less everything above it, children being stored bottom-first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BoolOp {
    /// Everything covered by any child.
    #[default]
    Union,
    /// The bottom child, less every child above it.
    Subtract,
    /// Only what every child covers.
    Intersect,
    /// Covered by an odd number of children — everything in one but not both.
    Exclude,
}

impl BoolOp {
    /// The label a menu or a button shows.
    pub fn label(self) -> &'static str {
        match self {
            BoolOp::Union => "Union",
            BoolOp::Subtract => "Subtract",
            BoolOp::Intersect => "Intersect",
            BoolOp::Exclude => "Exclude",
        }
    }

    pub const ALL: [BoolOp; 4] = [
        BoolOp::Union,
        BoolOp::Subtract,
        BoolOp::Intersect,
        BoolOp::Exclude,
    ];
}

/// The kind-specific payload of a node. All sizes/points are in local space.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum NodeKind {
    Root,
    /// A frame. Its fill is [`Node::paint`]'s fill list, like every painted
    /// layer's.
    Artboard {
        size: Size,
    },
    Group,
    /// `corner_radii` is per corner (§5.3). A uniform radius is the same value
    /// four times — there is no separate scalar, so nothing has to decide which
    /// of two fields wins.
    Rect {
        size: Size,
        corner_radii: RoundedRectRadii,
    },
    Ellipse {
        size: Size,
    },
    /// A regular polygon inscribed in `size`, first vertex at the top. `sides`
    /// below 3 is not a shape; the outline builder clamps rather than the model,
    /// so dragging the field through 2 and back does not lose the count.
    /// `corner_radius` rounds every vertex alike, as a path's anchor radius does.
    Polygon {
        size: Size,
        sides: u32,
        #[serde(default, skip_serializing_if = "is_zero")]
        corner_radius: f64,
    },
    /// A star inscribed in `size`, first point at the top. `points` outer
    /// vertices alternate with inner ones at `inner_ratio` of the radius, so
    /// 0.5 is the familiar five-pointed star and 1.0 degenerates to a polygon
    /// with twice the vertices. `corner_radius` rounds inner and outer vertices.
    Star {
        size: Size,
        points: u32,
        inner_ratio: f64,
        #[serde(default, skip_serializing_if = "is_zero")]
        corner_radius: f64,
    },
    /// Start is always the local origin; only the endpoint is stored (§5.3).
    Line {
        end: Point,
    },
    /// An authored outline, plus a rounding radius per **anchor** of it.
    ///
    /// The radii are a parallel list indexed by [`crate::geometry::anchor_runs`]'
    /// order; entries past the end are zero, so an unrounded path carries none
    /// (invariant 9). An anchor's identity is its index (§15 D114), so the app
    /// carries the radius on its anchor struct (`preview::PenAnchor::radius`) and
    /// flattens to this list only at the model boundary. Resolved at read time by
    /// [`crate::geometry::local_path`], never baked.
    Path {
        path: BezPath,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        corner_radii: Vec<f64>,
    },
    /// A container whose children are combined into one outline (`boolean.rs`).
    ///
    /// **Non-destructive**: the children stay in the tree and the result is
    /// *derived*. The outline is not stored; it lives in [`crate::Resolved`],
    /// cached and invalidated by the dirty set. It carries no `size`; a boolean
    /// is scaled by scaling what is inside it.
    Boolean {
        op: BoolOp,
    },
    /// Content plus the three scopes of typographic attribute (§5.4,
    /// `typography.rs`): `style` is the node's character defaults, `spans` the
    /// per-range overrides of them, `paragraph` and `block` the two node-wide
    /// sets. `sizing` stays a field of its own because it is edited through
    /// [`crate::GeometryPatch`] — it is the node's *box*, which the resize
    /// gesture writes, not a style anything reads through a span.
    Text {
        content: String,
        /// **Boxed**, as is `on_path`: an enum is as large as its largest variant,
        /// and an unboxed `TextStyle` (288 bytes against 176 for the next-largest
        /// variant) would make every node in the document that much bigger. Only
        /// text nodes pay the indirection. Transparent on disk.
        style: Box<TextStyle>,
        #[serde(default, skip_serializing_if = "CharSpans::is_empty")]
        spans: CharSpans,
        /// Per-paragraph overrides of `paragraph`, keyed by byte range like
        /// `spans` — which is why they travel with the content
        /// ([`crate::Operation::SetText`]) rather than on their own.
        #[serde(default, skip_serializing_if = "ParaSpans::is_empty")]
        para_spans: ParaSpans,
        #[serde(default, skip_serializing_if = "is_default_paragraph")]
        paragraph: ParagraphStyle,
        #[serde(default, skip_serializing_if = "is_default_block")]
        block: BlockStyle,
        sizing: TextSizing,
        /// The rail this text is set along, in the node's **own local space**, or
        /// `None` for ordinary horizontal type.
        ///
        /// The path belongs to the text node rather than being referenced by id,
        /// which would be the model's first cross-node reference (§15 D405).
        /// Boxed like [`TextStyle`]: this is `NodeKind`'s largest variant.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_path: Option<Box<BezPath>>,
        /// Whether the type runs along the rail the *other* way (§15 D406): one
        /// flag, since reversing the tangent reverses the normal with it.
        /// Beside [`Self::Text::on_path`] so *Detach from path* does not discard
        /// it; inert while `on_path` is `None`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        on_path_flip: bool,
        /// Where along the rail the type starts, as a **fraction of the rail's
        /// length** (§15 D409), so it survives the rail being scaled. Composed
        /// with [`crate::ParagraphStyle::align`], not replacing it. Inert while
        /// `on_path` is `None`.
        #[serde(default, skip_serializing_if = "is_zero")]
        on_path_offset: f64,
    },
}

/// `serde`'s skip predicate for a `f64` that means "no offset".
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// Whether every coordinate in a path is finite.
///
/// Checks every point of every element, control points included, since a NaN in
/// one is the same unopenable document. `PathEl` is matched exhaustively so a new
/// element cannot slip past.
pub(crate) fn path_is_finite(path: &BezPath) -> bool {
    let finite = |p: &Point| p.x.is_finite() && p.y.is_finite();
    path.elements().iter().all(|el| match el {
        kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p) => finite(p),
        kurbo::PathEl::QuadTo(a, b) => finite(a) && finite(b),
        kurbo::PathEl::CurveTo(a, b, c) => finite(a) && finite(b) && finite(c),
        kurbo::PathEl::ClosePath => true,
    })
}

fn is_default_paragraph(p: &ParagraphStyle) -> bool {
    *p == ParagraphStyle::default()
}

fn is_default_block(b: &BlockStyle) -> bool {
    *b == BlockStyle::default()
}

/// Everything one text node's layout depends on, borrowed from its `NodeKind`.
///
/// One argument instead of several for `text::layout`, `measure`, `local_bounds`
/// and the render paths, so none passes the wrong style positionally.
/// [`TextRef::of`] is the single place a `NodeKind` is taken apart.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextRef<'a> {
    pub content: &'a str,
    pub style: &'a TextStyle,
    pub spans: &'a CharSpans,
    pub para_spans: &'a ParaSpans,
    pub paragraph: &'a ParagraphStyle,
    pub block: &'a BlockStyle,
    pub sizing: &'a TextSizing,
    /// The rail, if this node is set along one ([`NodeKind::Text::on_path`]).
    ///
    /// Here rather than a second argument to `text::layout`, so every consumer of
    /// [`TextRef::of`] — renderer, hit test, bounds, SVG writer, edit session —
    /// bends along it without being told to.
    pub on_path: Option<&'a BezPath>,
    /// Whether the rail is traversed backwards ([`NodeKind::Text::on_path_flip`]).
    pub on_path_flip: bool,
    /// Where along the rail the type starts, as a fraction of its length
    /// ([`NodeKind::Text::on_path_offset`]).
    pub on_path_offset: f64,
}

impl<'a> TextRef<'a> {
    /// The text parts of `kind`, or `None` for any other kind.
    pub fn of(kind: &'a NodeKind) -> Option<Self> {
        match kind {
            NodeKind::Text {
                content,
                style,
                spans,
                para_spans,
                paragraph,
                block,
                sizing,
                on_path,
                on_path_flip,
                on_path_offset,
            } => Some(TextRef {
                content,
                style,
                spans,
                para_spans,
                paragraph,
                block,
                sizing,
                on_path: on_path.as_deref(),
                on_path_flip: *on_path_flip,
                on_path_offset: *on_path_offset,
            }),
            _ => None,
        }
    }
}

impl NodeKind {
    /// The stored `size` of a frame, rectangle, ellipse, polygon or star — the
    /// five kinds whose width and height bind — and `None` for every other kind,
    /// a text box included: its size is its sizing mode's.
    pub fn box_size(&self) -> Option<Size> {
        match self {
            NodeKind::Artboard { size }
            | NodeKind::Rect { size, .. }
            | NodeKind::Ellipse { size }
            | NodeKind::Polygon { size, .. }
            | NodeKind::Star { size, .. } => Some(*size),
            _ => None,
        }
    }

    /// [`Self::box_size`], to write.
    pub fn box_size_mut(&mut self) -> Option<&mut Size> {
        match self {
            NodeKind::Artboard { size }
            | NodeKind::Rect { size, .. }
            | NodeKind::Ellipse { size }
            | NodeKind::Polygon { size, .. }
            | NodeKind::Star { size, .. } => Some(size),
            _ => None,
        }
    }

    /// Whether every **geometry** number in this kind is finite (§15 D421).
    ///
    /// A non-finite number here makes the document permanently unopenable:
    /// `serde_json` writes it as `null` and the reader refuses `null` where an
    /// `f64` is wanted, so `apply` and `save` succeed and `load` fails forever.
    /// An SVG import reaches it (`L 1e999 10` parses to infinity through
    /// `BezPath::from_svg`).
    ///
    /// Geometry only; other `f64`s a node carries are guarded by their own ops.
    /// [`crate::Node::transform`] is checked by `op_set_transform`, being a field
    /// of the node rather than of the kind.
    pub fn geometry_is_finite(&self) -> bool {
        fn size(s: &Size) -> bool {
            s.width.is_finite() && s.height.is_finite()
        }
        match self {
            NodeKind::Root | NodeKind::Group | NodeKind::Boolean { .. } => true,
            NodeKind::Artboard { size: s } | NodeKind::Ellipse { size: s } => size(s),
            NodeKind::Polygon {
                size: s,
                corner_radius,
                ..
            } => size(s) && corner_radius.is_finite(),
            NodeKind::Rect {
                size: s,
                corner_radii,
            } => {
                size(s)
                    && [
                        corner_radii.top_left,
                        corner_radii.top_right,
                        corner_radii.bottom_right,
                        corner_radii.bottom_left,
                    ]
                    .iter()
                    .all(|r| r.is_finite())
            }
            NodeKind::Star {
                size: s,
                inner_ratio,
                corner_radius,
                ..
            } => size(s) && inner_ratio.is_finite() && corner_radius.is_finite(),
            NodeKind::Line { end } => end.x.is_finite() && end.y.is_finite(),
            NodeKind::Path { path, corner_radii } => {
                path_is_finite(path) && corner_radii.iter().all(|r| r.is_finite())
            }
            NodeKind::Text {
                sizing,
                on_path,
                on_path_offset,
                ..
            } => {
                on_path_offset.is_finite()
                    && match sizing {
                        TextSizing::Auto => true,
                        TextSizing::AutoHeight(w) => w.is_finite(),
                        TextSizing::Fixed(s) => size(s),
                    }
                    && on_path.as_deref().is_none_or(path_is_finite)
            }
        }
    }

    /// Whether this kind has a frame to clip its children to, i.e. whether
    /// [`Node::clip`] means anything on it.
    ///
    /// Only artboards today. Three places ask: `op_create`, which seeds a new
    /// node's `clip` from it; the DTO projection, which keeps the flag out of the
    /// file; and the loader, which keeps it out of a document read from one (§15 D663).
    ///
    /// The flag can still be set on a kind that ignores it (§15 D282: dragging a
    /// shape through a frame and back keeps its setting). The scene walk and the
    /// other readers ask [`Node::clip`], not this predicate, so removing the
    /// loader's normalization would cull a group's children against its own bounds.
    pub fn clips_children(&self) -> bool {
        matches!(self, NodeKind::Artboard { .. })
    }

    /// Whether this kind can be used as a **mask**, i.e. whether [`Node::mask`]
    /// means anything on it.
    ///
    /// [`Self::takes_paint`], minus a frame, plus `Group`: `Root` is not a layer,
    /// and a frame is a page that already carries the other clip
    /// ([`Self::clips_children`]), so it must not grow a second clip control.
    ///
    /// Everything else has an outline for `Resolved::mask_path` to derive: a
    /// shape's from its kind, a boolean's from the cache, a text node's from its
    /// glyphs, a group's from its contents unioned. One predicate because the
    /// loader, the inspector and the op layer all ask.
    ///
    /// The `Artboard` exclusion is written out rather than inherited from
    /// `takes_paint`, which answers `true` for a frame.
    pub fn can_mask(&self) -> bool {
        !matches!(self, NodeKind::Artboard { .. })
            && (self.takes_paint() || matches!(self, NodeKind::Group))
    }

    /// Whether this kind draws its own fills and strokes, i.e. whether
    /// [`Node::paint`] means anything on it.
    ///
    /// `Root` (not a layer) and `Group` (organisation) do not; everything with an
    /// outline does, a frame included — its outline is its own box
    /// ([`crate::geometry::local_path`]). One predicate because a bulk paint edit
    /// over a selection asks what the panel asks. Where a frame is the odd one
    /// out, as in [`Self::can_mask`], say so at the site.
    pub fn takes_paint(&self) -> bool {
        matches!(
            self,
            // A frame paints like a shape; where its ink lands (fills behind its
            // children, strokes over them) is the scene walk's business (§15 D144).
            NodeKind::Artboard { .. }
                | NodeKind::Rect { .. }
                | NodeKind::Ellipse { .. }
                | NodeKind::Polygon { .. }
                | NodeKind::Star { .. }
                | NodeKind::Line { .. }
                | NodeKind::Path { .. }
                | NodeKind::Text { .. }
                // The one container that paints: `build::boolean` inherits the
                // bottom operand's paint.
                | NodeKind::Boolean { .. }
        )
    }

    /// Default layer name for a freshly created node of this kind.
    pub fn default_name(&self) -> &'static str {
        match self {
            NodeKind::Root => "Root",
            // The UI says "Frame"; the variant name is the save-format tag.
            NodeKind::Artboard { .. } => "Frame",
            NodeKind::Group => "Group",
            NodeKind::Rect { .. } => "Rectangle",
            NodeKind::Ellipse { .. } => "Ellipse",
            NodeKind::Polygon { .. } => "Polygon",
            NodeKind::Star { .. } => "Star",
            NodeKind::Line { .. } => "Line",
            NodeKind::Path { .. } => "Path",
            NodeKind::Text { .. } => "Text",
            // Named for the operation, so the layer list says what it does.
            NodeKind::Boolean { op } => op.label(),
        }
    }
}

/// Fills + strokes, as many of each as the layer carries, painted in list
/// order: every visible fill, then every visible stroke, so the last element of
/// each list is on top (§5.3).
///
/// Two stacks rather than one interleaved list, which [`StrokeAlign::Outside`]
/// makes affordable by covering outlined text.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Paint {
    pub fills: Vec<Keyed<Fill>>,
    pub strokes: Vec<Keyed<Stroke>>,
}

impl Paint {
    /// The index of the first **visible image fill**, i.e. "is this layer a
    /// picture" and, if so, which fill is the picture.
    ///
    /// In core because `build::mask_target` asks it too; the app's
    /// `tools::croppable_fill` is one line onto this. Visible, and the first: a
    /// hidden image fill is not a picture the user is looking at, and the crop
    /// mode edits the first.
    pub fn image_fill(&self) -> Option<usize> {
        self.fills
            .iter()
            .position(|f| f.visible && matches!(f.brush.literal(), Some(Brush::Image(_))))
    }
}

/// One fill. Its brush may be bound to a colour variable
/// ([`BrushSource`](crate::BrushSource)), which binds the **whole solid paint**,
/// never a gradient stop.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fill {
    pub brush: crate::variable::BrushSource,
    pub visible: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    /// Bindable as a fill's is.
    pub brush: crate::variable::BrushSource,
    /// The literal width — 0 while [`Self::width_var`] binds it; what the stroke
    /// is drawn at is [`crate::Document::resolved_paint`]'s.
    pub width: f64,
    /// The number variable the width is bound to, kept in the item as the
    /// brush's binding is, so a stroke style, a reorder and a copy carry it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width_var: Option<crate::variable::VarId>,
    pub join: kurbo::Join,
    pub cap: kurbo::Cap,
    /// The dash pattern, as kurbo and `stroke-dasharray` take it: on, off, on…
    /// Empty is a solid stroke.
    ///
    /// The single source of truth for the pattern. A dotted stroke is a
    /// *zero-length* dash — `[0, s]` — so `s` is the centre-to-centre spacing and
    /// [`Stroke::dash_style`] classifies the pattern without a stored mode. The
    /// width-dependent consequences (a zero-length dash vanishes under
    /// `Cap::Butt`; a fitted pattern depends on the path) are resolved at the
    /// render boundary (§6.3), since derived numbers stored here would go stale
    /// when the width is scrubbed.
    pub dashes: Vec<f64>,
    pub dash_offset: f64,
    /// Which side of the outline the width is laid on.
    ///
    /// Defaulted rather than schema-versioned: additive, so a v1 file loads as
    /// `Center`, which is what it drew as. See §5.11.
    #[serde(default)]
    pub align: StrokeAlign,
    /// Which of a box's four sides the stroke occupies. Additive like `align`:
    /// the default [`StrokeSides::All`] is how existing files drew.
    #[serde(default, skip_serializing_if = "StrokeSides::is_all")]
    pub sides: StrokeSides,
    /// Mitre limit as the **ratio** kurbo takes and `stroke-miterlimit` emits,
    /// not the angle the panel shows. Converting at the UI edge once beats
    /// converting on every render and export, and avoids writing `3.9998` into
    /// files by round-tripping 28.96° through degrees.
    #[serde(default = "default_miter_limit")]
    pub miter_limit: f64,
    /// Scale the dash pattern so a whole number of periods fits the outline —
    /// the "fit to corners" toggle.
    ///
    /// A flag rather than a recompute written back into `dashes`, which would go
    /// stale on the next resize while still reading as on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dash_fit: bool,
    pub visible: bool,
}

/// kurbo's own default, which is SVG's: ratio 4, i.e. corners sharper than
/// 28.96° are bevelled.
fn default_miter_limit() -> f64 {
    4.0
}

/// The mitre ratio a limiting **angle** implies, and its inverse.
///
/// `ratio = 1 / sin(θ/2)` — the length of the spike a corner of `θ` would throw,
/// in stroke widths. Both directions are needed to show the stored ratio (see
/// [`Stroke::miter_limit`]) as an angle.
pub fn miter_ratio_of_degrees(degrees: f64) -> f64 {
    let half = (degrees.clamp(MIN_MITER_DEGREES, 180.0) * 0.5).to_radians();
    (1.0 / half.sin()).max(1.0)
}

/// The limiting angle a mitre ratio implies, in degrees.
pub fn miter_degrees_of_ratio(ratio: f64) -> f64 {
    2.0 * (1.0 / ratio.max(1.0)).clamp(0.0, 1.0).asin().to_degrees()
}

/// Sharpest angle the mitre field will accept: the ratio goes to infinity as the
/// angle goes to zero, so the low end needs a floor.
pub const MIN_MITER_DEGREES: f64 = 1.0;

/// Which sides of a box's outline a stroke occupies — CSS's `border-top` and
/// friends.
///
/// Only a box ([`NodeKind::Rect`]) has four nameable sides. Anywhere else the
/// stroke is the whole outline whatever this says; the control is gated in the
/// panel rather than the value being rewritten.
///
/// A side stroke is no longer the closed outline, so the scene walk and the SVG
/// writer emit an **open** path per side, each corner arc split at its midpoint
/// (§6.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum StrokeSides {
    /// The closed outline, which a file without the field still means.
    #[default]
    All,
    Top,
    Right,
    Bottom,
    Left,
    /// A width per side, independent of the stroke's own. `0` is no stroke on
    /// that side, and is the only way to ask for two adjacent sides and not the
    /// other two.
    Custom {
        top: f64,
        right: f64,
        bottom: f64,
        left: f64,
    },
}

/// One side of a box, named the way CSS names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::Top, Side::Right, Side::Bottom, Side::Left];

    pub fn label(self) -> &'static str {
        match self {
            Side::Top => "Top side",
            Side::Right => "Right side",
            Side::Bottom => "Bottom side",
            Side::Left => "Left side",
        }
    }
}

impl StrokeSides {
    /// Whether this is the plain closed outline, and so whether the field is
    /// worth writing to a file at all.
    pub fn is_all(&self) -> bool {
        matches!(self, StrokeSides::All)
    }

    /// The width each named side carries, given the stroke's own `width` —
    /// `None` for a side the stroke does not reach.
    ///
    /// `All` answers `None` four times: it keeps the closed outline, and four
    /// full-width sides would lose the join at every corner.
    /// [`StrokeSides::per_side`] excludes that case.
    pub fn widths(self, width: f64) -> [Option<f64>; 4] {
        let one = |side: Side| {
            Side::ALL.map(|s| (s == side).then_some(width)) // [top, right, bottom, left]
        };
        match self {
            StrokeSides::All => [None; 4],
            StrokeSides::Top => one(Side::Top),
            StrokeSides::Right => one(Side::Right),
            StrokeSides::Bottom => one(Side::Bottom),
            StrokeSides::Left => one(Side::Left),
            StrokeSides::Custom {
                top,
                right,
                bottom,
                left,
            } => [top, right, bottom, left].map(|w| (w > 0.0).then_some(w)),
        }
    }

    /// The sides to stroke and at what width, or `None` for the closed outline.
    pub fn per_side(self, width: f64) -> Option<Vec<(Side, f64)>> {
        if self.is_all() {
            return None;
        }
        let widths = self.widths(width);
        Some(
            Side::ALL
                .iter()
                .zip(widths)
                .filter_map(|(s, w)| w.map(|w| (*s, w)))
                .collect(),
        )
    }

    /// The widest side, which is what bounds and hit-testing have to allow for.
    ///
    /// One number for all four sides rather than per-side insets: bounds may be
    /// generous but never tight, and the callers
    /// ([`crate::geometry::world_bounds_of_parts`], the hit-test tolerance)
    /// inflate uniformly.
    pub fn max_width(self, width: f64) -> f64 {
        match self {
            StrokeSides::All => width,
            _ => self
                .widths(width)
                .into_iter()
                .flatten()
                .fold(0.0_f64, f64::max),
        }
    }

    /// The four widths a `Custom` row starts from when the user switches to it:
    /// whatever the stroke already draws, so opening the panel changes nothing.
    pub fn as_custom(self, width: f64) -> StrokeSides {
        if let StrokeSides::Custom { .. } = self {
            return self;
        }
        let [top, right, bottom, left] = match self {
            // Every side already carries the stroke, so Custom starts even.
            StrokeSides::All => [width; 4],
            other => other.widths(width).map(|w| w.unwrap_or(0.0)),
        };
        StrokeSides::Custom {
            top,
            right,
            bottom,
            left,
        }
    }
}

/// How a stroke's dash pattern reads back into the panel's four cells.
///
/// Classified from `dashes`, not stored, and invariant under width: a dotted
/// stroke is a zero-length dash, so no stored mode can drift from the pattern.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DashStyle {
    #[default]
    Solid,
    Dotted,
    Dashed,
    Custom,
}

impl DashStyle {
    pub const ALL: [DashStyle; 4] = [
        DashStyle::Solid,
        DashStyle::Dotted,
        DashStyle::Dashed,
        DashStyle::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DashStyle::Solid => "Solid",
            DashStyle::Dotted => "Dotted",
            DashStyle::Dashed => "Dashed",
            DashStyle::Custom => "Custom",
        }
    }
}

impl Stroke {
    /// Which of the panel's four dash cells this stroke's pattern is.
    pub fn dash_style(&self) -> DashStyle {
        match self.dashes.as_slice() {
            [] => DashStyle::Solid,
            // A zero-length dash is a dot; see [`Stroke::dashes`].
            [length, _] if *length <= 0.0 => DashStyle::Dotted,
            [_, _] => DashStyle::Dashed,
            _ => DashStyle::Custom,
        }
    }

    /// A dotted stroke's centre-to-centre spacing, whatever its cap.
    ///
    /// The *period* rather than the gap: with a zero-length dash they coincide,
    /// and the renderer's square-dot substitution preserves the period (§6.3).
    pub fn dot_spacing(&self) -> f64 {
        self.dashes.iter().take(2).sum()
    }

    /// A dashed stroke's `(length, spacing)`.
    pub fn dash_length_spacing(&self) -> (f64, f64) {
        match self.dashes.as_slice() {
            [length, spacing] => (*length, *spacing),
            _ => (0.0, 0.0),
        }
    }
}

impl Default for Stroke {
    /// A 1px centred solid black stroke on every side — what the `+` in the
    /// Stroke panel adds, minus its brush.
    fn default() -> Self {
        Self {
            brush: peniko::Color::BLACK.into(),
            width: 1.0,
            width_var: None,
            join: kurbo::Join::Miter,
            cap: kurbo::Cap::Butt,
            dashes: Vec::new(),
            dash_offset: 0.0,
            align: StrokeAlign::default(),
            sides: StrokeSides::All,
            miter_limit: default_miter_limit(),
            dash_fit: false,
            visible: true,
        }
    }
}

/// Which side of a shape's outline a stroke's width occupies.
///
/// Not a renderer concept: no backend (vello, vello_cpu, SVG) has stroke
/// alignment, so `ondin-render`'s scene walk and the SVG writer build it by
/// stroking at twice the width and clipping to the wanted side.
///
/// Only meaningful for closed shapes; `Line` and unclosed `Path` nodes draw
/// centred whatever this says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrokeAlign {
    /// Straddles the outline, half either side. The only alignment SVG and every
    /// 2D rasterizer support natively.
    #[default]
    Center,
    /// Entirely within the shape.
    Inside,
    /// Entirely outside the shape.
    Outside,
}

impl StrokeAlign {
    /// How far the painted stroke reaches *outside* the outline, as a fraction
    /// of the stroke width. Bounds and hit-testing use this.
    pub fn outward_fraction(self) -> f64 {
        match self {
            StrokeAlign::Center => 0.5,
            StrokeAlign::Inside => 0.0,
            StrokeAlign::Outside => 1.0,
        }
    }
}

/// How a text node's box is decided.
///
/// Three states: auto width never wraps, auto height wraps at a set width and
/// grows downwards. `Auto` and `Fixed` keep their names, and so their bytes, in
/// existing files.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum TextSizing {
    /// No wrapping; the box grows to the shaped width *and* height.
    Auto,
    /// Wraps at this width; the box grows downwards to fit the lines.
    AutoHeight(f64),
    /// Wraps at `width`; the box stays exactly this size and the content
    /// clips or overflows within it, per [`crate::BlockStyle::overflow`].
    Fixed(Size),
}

impl TextSizing {
    /// The width lines wrap at, or `None` where they do not wrap.
    pub fn wrap_width(self) -> Option<f64> {
        match self {
            TextSizing::Auto => None,
            TextSizing::AutoHeight(w) => Some(w),
            TextSizing::Fixed(s) => Some(s.width),
        }
    }

    /// Whether the height is the user's rather than the content's — the one
    /// question box trim and vertical alignment both ask.
    pub fn height_is_authored(self) -> bool {
        matches!(self, TextSizing::Fixed(_))
    }

    /// Which of the panel's three cells this is.
    pub fn cell(self) -> usize {
        match self {
            TextSizing::Auto => 0,
            TextSizing::AutoHeight(_) => 1,
            TextSizing::Fixed(_) => 2,
        }
    }

    pub const LABELS: [&'static str; 3] = ["Auto W", "Auto H", "Fixed"];

    /// This sizing switched to the panel cell `i`, adopting `shaped` so the text
    /// does not jump when the mode changes.
    pub fn to_cell(self, i: usize, shaped: Size) -> TextSizing {
        let width = self.wrap_width().unwrap_or(shaped.width);
        match i {
            0 => TextSizing::Auto,
            1 => TextSizing::AutoHeight(width),
            _ => TextSizing::Fixed(Size::new(width, shaped.height)),
        }
    }
}

/// One sample of every `NodeKind`, and the tripwire that keeps the list whole
/// (§15 D668).
///
/// `NodeKind` has no `ALL` because its variants carry payloads. Six `matches!`
/// predicates decide behaviour from it, and each answers `false` for a variant
/// nobody added to it, with every gate green.
#[cfg(test)]
pub(crate) mod kind_fixture {
    use super::*;

    /// The variant's name, from an exhaustive `match`.
    ///
    /// The gate: it has no wildcard, so a new `NodeKind` does not compile here,
    /// and the error sends the author to `all` below. **A new arm here needs a
    /// new sample there**; nothing checks that pairing. Also the label the
    /// assertions compare.
    pub(crate) fn tag(kind: &NodeKind) -> &'static str {
        match kind {
            NodeKind::Root => "Root",
            NodeKind::Artboard { .. } => "Artboard",
            NodeKind::Group => "Group",
            NodeKind::Rect { .. } => "Rect",
            NodeKind::Ellipse { .. } => "Ellipse",
            NodeKind::Polygon { .. } => "Polygon",
            NodeKind::Star { .. } => "Star",
            NodeKind::Line { .. } => "Line",
            NodeKind::Path { .. } => "Path",
            NodeKind::Boolean { .. } => "Boolean",
            NodeKind::Text { .. } => "Text",
        }
    }

    /// One value per variant. The payloads are the smallest legal thing, because
    /// no predicate in the matrix reads one.
    pub(crate) fn all() -> Vec<NodeKind> {
        let one = Size::new(1.0, 1.0);
        vec![
            NodeKind::Root,
            NodeKind::Artboard { size: one },
            NodeKind::Group,
            NodeKind::Rect {
                size: one,
                corner_radii: RoundedRectRadii::default(),
            },
            NodeKind::Ellipse { size: one },
            NodeKind::Polygon {
                size: one,
                sides: 3,
                corner_radius: 0.0,
            },
            NodeKind::Star {
                size: one,
                points: 5,
                inner_ratio: 0.5,
                corner_radius: 0.0,
            },
            NodeKind::Line {
                end: Point::new(1.0, 0.0),
            },
            NodeKind::Path {
                path: BezPath::new(),
                corner_radii: Vec::new(),
            },
            NodeKind::Boolean { op: BoolOp::Union },
            NodeKind::Text {
                content: String::new(),
                style: Default::default(),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing: TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
        ]
    }

    /// `pred`'s membership as two lists of names: what it accepts, and **what it
    /// rejects**. The rejected list is the point: a new variant answers `false`
    /// and leaves the accepted set unchanged, so only a spelled-out rejected set
    /// goes red for it.
    pub(crate) fn split(
        pred: impl Fn(&NodeKind) -> bool,
    ) -> (Vec<&'static str>, Vec<&'static str>) {
        let mut yes = Vec::new();
        let mut no = Vec::new();
        for kind in all() {
            if pred(&kind) {
                yes.push(tag(&kind));
            } else {
                no.push(tag(&kind));
            }
        }
        (yes, no)
    }
}

#[cfg(test)]
mod kind_predicate_tests {
    use super::kind_fixture::{all, split, tag};

    /// The fixture samples each variant once. It cannot check completeness; only
    /// the compile error in `tag` does.
    ///
    /// flip: duplicate an entry of `all()` → fails at the assert, naming the tag.
    #[test]
    fn the_fixture_holds_each_kind_once() {
        let mut names: Vec<&str> = all().iter().map(tag).collect();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), before, "a kind is sampled twice: {names:?}");
    }

    /// The six `matches!`-over-`NodeKind` predicates, each as the pair of sets it
    /// actually divides the enum into.
    ///
    /// One test rather than six, so `is_indexed` and `takes_paint` (identical
    /// membership, spelled in two modules) are compared side by side.
    ///
    /// flip: delete `NodeKind::Boolean { .. }` from `resolve::is_indexed` → fails
    /// at the `is_indexed` assertion. flip: remove `"Root"` from `clips_children`'s
    /// rejected list → fails there; `Root` is rejected by all six, which is what a
    /// new variant looks like here.
    #[test]
    fn every_kind_predicate_divides_the_enum_where_it_says_it_does() {
        assert_eq!(
            split(|k| k.clips_children()),
            (
                vec!["Artboard"],
                vec![
                    "Root", "Group", "Rect", "Ellipse", "Polygon", "Star", "Line", "Path",
                    "Boolean", "Text",
                ]
            ),
            "clips_children"
        );
        assert_eq!(
            split(|k| k.can_mask()),
            (
                vec![
                    "Group", "Rect", "Ellipse", "Polygon", "Star", "Line", "Path", "Boolean",
                    "Text"
                ],
                vec!["Root", "Artboard"]
            ),
            "can_mask"
        );
        assert_eq!(
            split(|k| k.takes_paint()),
            (
                vec![
                    "Artboard", "Rect", "Ellipse", "Polygon", "Star", "Line", "Path", "Boolean",
                    "Text"
                ],
                vec!["Root", "Group"]
            ),
            "takes_paint"
        );
        assert_eq!(
            split(crate::resolve::is_indexed),
            (
                vec![
                    "Artboard", "Rect", "Ellipse", "Polygon", "Star", "Line", "Path", "Boolean",
                    "Text"
                ],
                vec!["Root", "Group"]
            ),
            "is_indexed — identical to takes_paint, and that is the drift surface"
        );
        assert_eq!(
            split(crate::geometry::stroke_sides_apply),
            (
                vec!["Artboard", "Rect"],
                vec![
                    "Root", "Group", "Ellipse", "Polygon", "Star", "Line", "Path", "Boolean",
                    "Text"
                ]
            ),
            "stroke_sides_apply"
        );
        assert_eq!(
            split(crate::query::steps_into),
            (
                vec!["Group", "Boolean"],
                vec![
                    "Root", "Artboard", "Rect", "Ellipse", "Polygon", "Star", "Line", "Path",
                    "Text"
                ]
            ),
            "steps_into (group_chain's arm)"
        );
    }
}
