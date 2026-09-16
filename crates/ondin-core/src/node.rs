//! Node model (§5.3–5.4).
//!
//! Node fields are private (invariant 2): the only way to mutate is via
//! `Document::apply`. Geometry is authored in the node's LOCAL space; the
//! `transform` positions it under its parent (invariant 5/6). Read access is
//! through the getters below.

use crate::id::NodeId;
use crate::image::Brush;
use kurbo::{Affine, BezPath, Point, RoundedRectRadii, Size, Vec2};
use serde::{Deserialize, Serialize};

// The text attribute model lives in `typography.rs` — three scopes, three types
// (§5.4) — and is re-exported here so `node::TextStyle` still names the thing a
// text node carries.
pub use crate::typography::{
    BlockStyle, BoxTrim, CharAttr, CharAttrKind, CharSpan, CharSpans, Decoration, JustifyLast,
    Length, LengthUnit, LineStyle, OverflowWrap, ParaAttr, ParaAttrKind, ParaSpan, ParaSpans,
    ParagraphStyle, Span, SpanAttr, Spans, TextAlign, TextCase, TextDirection, TextOverflow,
    TextStyle, VerticalAlign, WordBreak, WrapMode,
};

/// A single node in the document tree.
///
/// **Constructed by struct literal, inside this crate only** — the fields are
/// `pub(crate)` (invariant 2) and there is deliberately no constructor. There
/// used to be one, `Node::from_parts`, and it grew from eleven positional
/// arguments to thirteen, three of them consecutive bare `bool`s; §15 D41 and
/// D60 both recorded the run getting longer and asked for a parts struct before
/// a fourth bool arrived. A named-field literal *is* that parts struct, and two
/// of the five sites (`Document::new`, `Document::op_create`) were already
/// written that way — so the answer was to delete the second door rather than
/// widen it (§15 D240). A new field still breaks every site, which is the
/// property the constructor was there for.
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
    /// Scale is the one component kept out, so that a layer's size is one number
    /// in one place (its geometry) and a stroke does not thicken when a group is
    /// resized. Skew has no geometry field it could live in instead, so it lives
    /// here; `build::Basis` is the split that enforces the division, and the one
    /// thing that may read a basis back apart.
    pub(crate) transform: Affine,
    pub(crate) name: String,
    pub(crate) visible: bool,
    pub(crate) locked: bool,
    /// Whether resizing this node holds its aspect ratio: a handle drag keeps
    /// the proportions as though Shift were held (`canvas::keep_ratio`), and the
    /// Transform panel's W and H scale together.
    ///
    /// Nothing to do with `locked` above, which locks the *layer* against being
    /// edited at all. This one constrains how an edit that is allowed behaves,
    /// and the two are independent in both directions.
    ///
    /// Meaningful on anything resizable and simply inert on the rest — unlike
    /// `clip` there is no kind gate, because there is no kind for which "keep
    /// the ratio" would mean the wrong thing rather than nothing.
    pub(crate) proportions_locked: bool,
    /// 0.0..=1.0.
    pub(crate) opacity: f32,
    /// Whether this node clips what its children draw to its own frame.
    ///
    /// Node-level rather than a field of `NodeKind::Artboard`, for the same
    /// reason `visible`/`locked`/`opacity` are: it is a property of the *layer*,
    /// and the one container that honours it today is not obviously the last one
    /// that will (a clipping group is the same switch). Ignored by every kind
    /// that has no frame to clip to.
    pub(crate) clip: bool,
    /// Whether this node is a **mask**: it draws nothing itself, and its outline
    /// clips the siblings painted above it.
    ///
    /// The scope is a *run* of siblings — everything after this node in the
    /// parent's child list, up to the next mask or the end of it — which is
    /// Figma's rule and the reason a second mask below simply starts a second
    /// run instead of nesting inside the first.
    ///
    /// **Node-level like [`Self::clip`], and for the same reason**, but they are
    /// not the same switch and must not be confused: `clip` cuts a container's
    /// *children* to its own frame, this cuts its *siblings* to its own outline.
    /// A frame can do the first and cannot do the second
    /// ([`NodeKind::can_mask`]).
    pub(crate) mask: bool,
    /// *How* this node masks, when [`Self::mask`] says it does.
    ///
    /// **A field beside the flag rather than a payload inside it**, so the mode
    /// survives the mask being switched off and on. A designer who has set a mask
    /// to `Alpha`, released it to look underneath and put it back should get the
    /// alpha mask back; an `Option<MaskMode>` has nowhere to keep that while the
    /// mask is off, which quietly makes the toggle destructive.
    ///
    /// Inert while `mask` is false, exactly as [`Self::clip`] is inert on a kind
    /// with no frame.
    pub(crate) mask_mode: MaskMode,
    /// How this node's geometry decides its own interior (§15 D239).
    ///
    /// **On the node rather than on a [`Fill`], though SVG puts it on the paint.**
    /// It is a property of the *geometry*: the hit test asks it, the mask asks it
    /// when this node clips, and a node with no fill at all still has an inside. A
    /// per-fill rule would let two fills of one shape disagree about what the shape
    /// is, which is not a question anyone has.
    ///
    /// Defaulted, so every file written before it existed reads back unchanged.
    pub(crate) fill_rule: FillRule,
    /// Meaningful for shape/text kinds; empty for Root/Group/Artboard.
    pub(crate) paint: Paint,
    /// The effect stack (§5.3a) — shadows, blur and colour filters, in
    /// compositing order.
    ///
    /// **Unlike [`Self::paint`] this is meaningful on a container**, and that is
    /// the whole reason it is a node field rather than something inside
    /// [`Paint`]: a drop shadow on a group, or a blur over a whole frame, is one
    /// of the commonest things anyone does with an effect, and neither has a fill
    /// it could have hung from. A `Group` with no paint at all still composites,
    /// which is exactly what an effect needs.
    ///
    /// **Empty on every node until someone adds one**, like [`Self::exports`] —
    /// skipped by the save format and by every walk that has nothing to do. The
    /// difference from `exports` is that these do affect what is drawn, so
    /// `Operation::SetEffects` answers `true` to `changes_ink`, and an effect
    /// that reaches outside its layer widens the cached bounds
    /// ([`crate::effect::stack_escape`]).
    pub(crate) effects: Vec<crate::effect::Effect>,
    /// Where this node's transforms turn and mirror about, or `None` for the
    /// centre of its own box.
    ///
    /// `None` rather than a stored centre, so that "the user has not moved it"
    /// is a state the model can *say* — which is what lets the canvas show the
    /// marker only when it is somewhere unexpected, and what keeps the field out
    /// of every saved file that never touched it.
    pub(crate) pivot: Option<Pivot>,
    /// The files this layer produces — the Export panel's list (§7).
    ///
    /// **Empty on every node until someone adds one**, and empty is the state
    /// that costs nothing: it is skipped by the save format, ignored by every
    /// walk, and read only by the export path. Nothing here affects what is
    /// drawn, which is why `Operation::SetExports` answers `false` to
    /// `changes_ink`.
    pub(crate) exports: Vec<crate::export::ExportSpec>,
    /// The columns and rows drawn over this layer (`crate::layout`).
    ///
    /// **Empty on every node until someone adds one**, on exactly [`Self::exports`]'
    /// terms — skipped by the save format, ignored by every walk — and chrome for
    /// the same reason, so `Operation::SetLayoutGrids` answers `false` to
    /// `changes_ink` too. Nothing about a grid reaches the scene, the export
    /// writers or the snapshot.
    ///
    /// ⚠️ **Node-level, and offered only on a frame**, which is [`Self::clip`]'s
    /// arrangement rather than a disagreement with it: the field is a property of
    /// the layer and inert anywhere the app does not offer it. What confines the
    /// offer is that a grid is measured against an **authored** size —
    /// `NodeKind::Artboard` has one, and a group's box is derived from its
    /// contents, so a grid on a group would move whenever a child did.
    pub(crate) grids: Vec<crate::layout::LayoutGrid>,
}

impl Node {
    pub fn id(&self) -> NodeId {
        self.id
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
    pub fn name(&self) -> &str {
        &self.name
    }
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
    /// ⚠️ **A `Boolean` with [`BoolOp::Exclude`] is always even-odd, whatever the
    /// field says**, and deriving it here rather than storing it is what makes the
    /// change safe for documents that already exist. An `Exclude`'s outline is its
    /// operands concatenated — that *is* the symmetric difference, read even-odd —
    /// so a file written before this field existed has no key, would default to
    /// non-zero, and would draw its exclusion as a **union**. Every such file is
    /// correct without being touched, because the rule is a consequence of the
    /// operation rather than a choice about it.
    ///
    /// It also means no migration, no `SetFillRule` inside `build::boolean`, and
    /// nothing for a directly-constructed test fixture to remember. The stored field
    /// is what a *user* set on a shape, and is what [`crate::io`] writes; this is
    /// what every consumer asks.
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
    pub fn effects(&self) -> &[crate::effect::Effect] {
        &self.effects
    }
    /// Where this node's transforms pivot, or `None` if it still sits on the
    /// centre of the node's box. Resolve it to a point with
    /// [`crate::geometry::pivot_point`].
    pub fn pivot(&self) -> Option<Pivot> {
        self.pivot
    }
    /// The export specs this layer carries, in the order the panel shows them.
    pub fn exports(&self) -> &[crate::export::ExportSpec] {
        &self.exports
    }
    /// The layout grids drawn over this layer, in the order the panel shows
    /// them (`crate::layout`).
    pub fn grids(&self) -> &[crate::layout::LayoutGrid] {
        &self.grids
    }
}

/// How a [`Node::mask`] decides what survives of the layers it masks.
///
/// **Two, and the pair is a real choice rather than a setting**: one asks *where*
/// the mask is and the other asks *how much of it* is there.
///
/// The third mode every other editor offers — **luminance**, where the mask's
/// brightness is the amount — is deliberately absent, and it is a dependency
/// limit rather than a design one. `vello_cpu` has `Mask::new_luminance` and the
/// GPU backend has nothing equivalent: vello's `push_layer` takes no mask, and no
/// Porter-Duff compose or blend mode moves colour into alpha, so the conversion
/// is not expressible in the compositor at all. Shipping it would mean the canvas
/// and the export disagreeing, which is the one thing §3 exists to prevent. See
/// §15 for the measurement and what it would cost to close.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaskMode {
    /// The mask's **outline** clips: a hard edge, and everything inside it
    /// survives whole. Figma's *vector mask*.
    ///
    /// The default, and the cheaper of the two — one clip layer, where `Alpha`
    /// needs two and an offscreen composite. It is also what every mask made
    /// before this mode existed meant, which is what keeps the field out of
    /// existing files.
    #[default]
    Shape,
    /// The mask's own **alpha** is the amount that survives, per pixel. A
    /// gradient fading to transparent fades what it masks; a photograph with an
    /// alpha channel cuts to its own edges.
    ///
    /// The mask's *colour* is not read — only its alpha — which is the difference
    /// between this and the luminance mode that is not here.
    Alpha,
}

/// How a path's interior is decided where it crosses itself or another subpath
/// (§15 D239).
///
/// **The rule SVG and every drawing tool has, and this model did not.** A closed
/// path with more than one subpath, or one that crosses itself, does not say on its
/// own which parts are inside; the two answers are counting *turns* about the point
/// and counting *crossings* to infinity, and they differ exactly where a subpath
/// overlaps another wound the same way.
///
/// ⚠️ **It is the fix for `Exclude`, not a decoration.** A symmetric difference *is*
/// the even-odd reading of its operands concatenated — a point is in it when an odd
/// number of operands cover it, which is what an odd crossing count means — so with
/// this field a boolean `Exclude` needs **no path arithmetic at all** and is exact.
/// Measured before it was built: the fold flo_curves performs is wrong by up to
/// 12,374 area units of 69,810 on a fourteen-circle ring, where the even-odd reading
/// of the same operands is accurate to within the referee's own sampling noise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FillRule {
    /// A point is inside when the path winds about it a non-zero number of times.
    ///
    /// **The default, and what every path in every file written before this field
    /// existed meant** — which is what keeps the field out of those files, exactly
    /// as [`MaskMode::Shape`] does for masks. Two overlapping subpaths wound the
    /// same way fill solid.
    #[default]
    NonZero,
    /// A point is inside when a ray from it crosses the path an odd number of times.
    ///
    /// Two overlapping subpaths cancel where they meet, which is what makes this the
    /// symmetric difference and what makes a hole a hole regardless of its winding.
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
    /// **`BezPath::contains` is non-zero and nothing said so**, which is the trap
    /// this exists to close: kurbo's containment is `winding(p) != 0`, so a boolean
    /// hit-tested with it under an even-odd fill would be clickable in the holes it
    /// does not draw. Even-odd is the *parity* of the same winding number — each
    /// crossing moves it by one, so an odd winding is an odd crossing count — which
    /// is why this needs no second traversal.
    pub fn contains(self, path: &BezPath, p: Point) -> bool {
        use kurbo::Shape;
        match self {
            FillRule::NonZero => path.contains(p),
            FillRule::EvenOdd => path.winding(p) % 2 != 0,
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
/// rectangle's box is a field, so a pivot two thirds along it should stay two
/// thirds along it when the field changes — that is what a fraction means and it
/// is what a designer expects of a resize. A group's box is the union of its
/// children and a line's is degenerate, so there is no authored extent for a
/// fraction to be *of*: editing one child would slide the pivot out from under
/// everything else in the group. Those kinds store the point itself.
///
/// Which variant a kind gets is [`crate::geometry::pivot_for`]'s decision, made
/// once from `resizable_size`, so the two halves of the rule cannot drift apart.
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
    /// A pivot is two coordinates like any other, and the operation layer is
    /// where a non-finite one has to be refused — `serde_json` writes a `NaN`
    /// `f64` as `null` and then refuses `null` where an `f64` is wanted, so a
    /// document that accepts one saves cleanly and never opens again. That is
    /// `[S2.2-L1-06]`, and the same terminal shape as `[A1-L2-01]`'s transform.
    ///
    /// ⚠️ **The arithmetic downstream is already safe and that is not the
    /// question.** `geometry::pivot_for` falls back below `MIN_EXTENT` and
    /// `canvas::origin_pivot` has its own floor, so nothing here divides by a
    /// zero extent; what neither guards is a coordinate that was never finite to
    /// begin with, which arrives from a singular world transform through
    /// `canvas::pivot_at` and from any hand-authored write.
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
/// The four Figma offers, and the same four every vector editor does — the set is
/// not really a choice: they are the four functions of two winding numbers that
/// produce a region.
///
/// **Order is layer order** for the two that are not commutative. `Subtract` is the
/// bottom child less everything above it, and children are stored bottom-first
/// (child index 0 paints first), so the layer list reads the same way the operation
/// does.
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
    /// A frame. **Its fill is [`Node::paint`]'s fill list, like every other
    /// painted layer's** — it used to be an `Option<Brush>` field right here, one
    /// ground or none, and that one field is what made every fill verb in the
    /// workspace carry a frame-shaped special case (§15 D400).
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
    Polygon {
        size: Size,
        sides: u32,
    },
    /// A star inscribed in `size`, first point at the top. `points` outer
    /// vertices alternate with inner ones at `inner_ratio` of the radius, so
    /// 0.5 is the familiar five-pointed star and 1.0 degenerates to a polygon
    /// with twice the vertices.
    Star {
        size: Size,
        points: u32,
        inner_ratio: f64,
    },
    /// Start is always the local origin; only the endpoint is stored (§5.3).
    Line {
        end: Point,
    },
    /// An authored outline, plus a rounding radius per **anchor** of it.
    ///
    /// **The radii are a parallel list, indexed by
    /// [`crate::geometry::anchor_runs`]' order** — the one place that order is
    /// defined, precisely so the app and the file cannot number the same path
    /// differently. `corner_radii[i]` rounds anchor `i`; entries past the end are
    /// zero, so the list is trimmed of trailing zeros and an unrounded path
    /// carries none at all (`skip_serializing_if`, which keeps every existing
    /// file byte-identical — invariant 9).
    ///
    /// **A parallel list is the cost of `BezPath` over a vector network** (§15
    /// D114, D119). A network would give each anchor an identity to hang a radius
    /// on; here the identity *is* the index, so every edit that inserts or removes
    /// an anchor has to renumber this in step. That is why the app carries the
    /// radius **on** its anchor struct (`preview::PenAnchor::radius`) and only
    /// flattens back to this list at the model boundary: moving, deleting or
    /// reordering an anchor then carries its radius automatically, and there is
    /// no second list to keep aligned by hand.
    ///
    /// Resolved into geometry at read time by [`crate::geometry::local_path`],
    /// the way a `Rect`'s `corner_radii` already are — never baked, so scrubbing
    /// the value up and down is lossless.
    Path {
        path: BezPath,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        corner_radii: Vec<f64>,
    },
    /// A container whose children are combined into one outline (`boolean.rs`).
    ///
    /// **Non-destructive, like Figma's**: the children stay in the tree, keep their
    /// own transforms and geometry, and remain selectable and editable — the result
    /// is *derived*, re-evaluated whenever anything under here changes. That is the
    /// whole reason it is a container rather than an operation that rewrites a
    /// `Path` and throws the operands away.
    ///
    /// The outline itself is **not** stored, and could not be: it is a function of
    /// the whole subtree, so it cannot come out of
    /// [`crate::geometry::local_path`] the way every other shape's does. It lives in
    /// [`crate::Resolved`] instead, cached per node and invalidated by the dirty
    /// set — exactly the arrangement a `Text` node's shaped layout already has, and
    /// for exactly the same reason.
    ///
    /// It carries no `size`: the box is whatever the combined outline turns out to
    /// be. So there is nothing here for a resize handle to write, and a boolean is
    /// scaled the way a group is — by scaling what is inside it.
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
        /// **Boxed**, and since §15 D405 not the only field here that is — see
        /// `on_path` below, which is boxed for exactly this reason and measured
        /// the same way. Measured: `TextStyle` is
        /// 288 bytes where the whole of `NodeKind`'s next-largest variant is 176,
        /// and a Rust enum is as large as its largest variant — so unboxed it made
        /// *every* node in the document, a line and a rect included, 288 bytes
        /// bigger than it needs to be (`Node` went 344 → 624). One pointer
        /// indirection on read, paid only by text nodes, buys that back.
        ///
        /// Transparent on disk: `Box<T>` serializes as `T`.
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
        /// **The path belongs to the text node rather than being referenced from
        /// it.** The two alternatives were a container kind holding both (the
        /// `Boolean` shape) and an `Option<NodeId>` pointing at a sibling (SVG's
        /// `<textPath href>` shape). Both were declined: a container puts itself
        /// between every existing text feature and the node it edits, and an id
        /// would be the **first cross-node reference in this model** — nothing
        /// here references anything — so it would have to invent dangling,
        /// delete, duplicate and cross-document-paste semantics that no other
        /// field needs. Applying a path consumes the layer it came from and
        /// *Detach* gives it back, which is what Illustrator and Affinity both
        /// do (§15 D405).
        ///
        /// **Not `Option<BezPath>` but `Option<Box<BezPath>>`**, for the reason
        /// [`TextStyle`] is boxed above, and measured the same way rather than
        /// assumed: this variant *is* `NodeKind`'s largest — its payload is
        /// **200** bytes, which is `NodeKind`'s whole size — and unboxed the rail
        /// would take it to **216**. A Rust enum is as large as its largest
        /// variant, so that is 16 bytes on *every* node in the document, a line
        /// and a rect included, to serve the few that are on a curve. One
        /// indirection, paid only by type on a path, buys it back: `Option<Box<_>>`
        /// is 8 bytes where `Option<BezPath>` is 24.
        ///
        /// Transparent on disk (`Box<T>` serializes as `T`), and absent from
        /// every existing file, so invariant 9 holds byte-for-byte.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_path: Option<Box<BezPath>>,
        /// Whether the type runs along the rail the *other* way — the flip every
        /// tool with type on a path has (§15 D406).
        ///
        /// **One flag for both halves, because they are one gesture.** Flipping
        /// reverses the direction the text runs *and* puts it on the other side of
        /// the curve, and those are not two settings: traversing the rail
        /// backwards negates the tangent, which negates the normal with it, so the
        /// text reverses and changes sides together. That is what a designer means
        /// by "the other side" — text around the bottom of a badge has to read
        /// left-to-right, which it only does if the direction flips too.
        ///
        /// ⚠️ **A field beside [`Self::Text::on_path`] rather than a payload inside
        /// it**, exactly as [`Node::mask_mode`] sits beside [`Node::mask`] and for
        /// the same reason: a flip kept inside the `Option` has nowhere to live
        /// while the rail is off, so *Detach from path* would silently discard it
        /// and putting the text back on a curve would come back the wrong way
        /// round. Inert while `on_path` is `None`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        on_path_flip: bool,
        /// Where along the rail the type starts, as a **fraction of the rail's
        /// length** (§15 D409).
        ///
        /// **Because a rail has a start and nobody can see where it is.** Without
        /// this the type always began at the path's own first point, which for a
        /// triangle drawn from any corner is some other corner: reported as
        /// *"I click on the base, and it always starts the text on the right edge,
        /// at the top"*. The Text tool's click sets this from where the pointer
        /// was, so the type begins under the mark that offered it.
        ///
        /// **A fraction rather than a distance**, so it survives the rail being
        /// scaled: a resize handle scales the curve (§15 D405), and an absolute
        /// offset would slide the type toward the start as the rail grew under it.
        /// It is also the unit the SVG writer already emits, `startOffset` in
        /// percent.
        ///
        /// **Composed with [`crate::ParagraphStyle::align`], not replacing it.**
        /// Alignment says where the text sits relative to the whole rail and this
        /// nudges it from there, so a centred node clicked at its middle stays
        /// centred. Inert while `on_path` is `None`, like the flip beside it.
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
/// **Every point of every element, not just the endpoints.** A `CurveTo`'s
/// control points are written to the file exactly as its endpoint is, so a NaN
/// in one of them is the same unopenable document; kurbo's `PathEl` is matched
/// exhaustively here so a new element cannot be added past this quietly.
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
/// **One argument instead of five.** `text::layout`, `measure`, `local_bounds`
/// and the two render paths all need the same set, and the set grew from two
/// fields to five when the attribute model landed; threading them positionally
/// was how a call site ended up passing the paragraph style where the block one
/// belonged. [`TextRef::of`] is the single place a `NodeKind` is taken apart.
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
    /// **Here rather than as a second argument to `text::layout`**, and that is
    /// the whole of what keeps one drawing: [`TextRef::of`] is the single place a
    /// `NodeKind` is taken apart, so every consumer that already asks it a
    /// question — the renderer, the hit test, `geometry::local_bounds`, the SVG
    /// writer, the live edit session — bends without being told to. A parameter
    /// beside the parts is how the caret would come to sit where the text used
    /// to be.
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
    /// Whether every **geometry** number in this kind is finite (§15 D421).
    ///
    /// ⚠️ **A non-finite number here makes the document permanently unopenable,
    /// and nothing anywhere said so until this existed.** `serde_json` writes a
    /// non-finite `f64` as `null`, and the reader has no arm for `null` where it
    /// wants an `f64` — so `apply` succeeds, `save` succeeds, and `load` fails
    /// forever with *"invalid type: null, expected f64"*. Nothing surfaces an
    /// error at any point in between, and the corrupting write is the one
    /// autosave and the crash snapshot both make, so the user need never press
    /// Ctrl+S.
    ///
    /// **The reachable route is an ordinary import.** `<path d="M 0 0 L 1e999 10
    /// Z"/>` parses to `f64::INFINITY`: `svg_in::parse_len` filters for finite
    /// and `numbers()`/`BezPath::from_svg` do not, so the guard existed on the
    /// one attribute path and the *geometry* paths went round it. Measured end
    /// to end.
    ///
    /// **Geometry only, and the scope is deliberate rather than complete.** The
    /// same `null` mechanism reaches every other `f64` a node carries — a text
    /// style's size, a gradient stop's offset, an export scale — and those are
    /// written by ops of their own with their own guards to add. This is the
    /// surface the measured route reaches; the rest is a queue, not a claim.
    /// [`crate::Node::transform`] is checked separately, by `op_set_transform`,
    /// because it is a field of the node rather than of the kind.
    pub fn geometry_is_finite(&self) -> bool {
        fn size(s: &Size) -> bool {
            s.width.is_finite() && s.height.is_finite()
        }
        match self {
            NodeKind::Root | NodeKind::Group | NodeKind::Boolean { .. } => true,
            NodeKind::Artboard { size: s }
            | NodeKind::Ellipse { size: s }
            | NodeKind::Polygon { size: s, .. } => size(s),
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
                ..
            } => size(s) && inner_ratio.is_finite(),
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
    /// Only artboards today. **Three places ask, and they are the *format's* two
    /// ends plus the default** (§15 D663): `op_create`, which seeds a new node's
    /// `clip` from it; the DTO projection, which keeps the flag out of the file;
    /// and the loader, which keeps it out of a document read from one.
    ///
    /// 🚨 **This said *"the scene walk, the SVG writer, and the loader"* until
    /// 2026-09-09, and two of those three were false** — as was `architecture.md`
    /// §5.3, which drew the stronger conclusion that *"the flag cannot be set on a
    /// kind that would ignore it"*. It can, deliberately: §15 **D282** decided
    /// that an inert `clip` is a switch nothing reads, so dragging a shape through
    /// a frame and back keeps its setting. **What the readers actually ask is
    /// [`Node::clip`]**, and where they narrow it at all they narrow it with an
    /// `Artboard` match arm rather than with this predicate — while
    /// `scene::bounds_cover_subtree`, `canvas`'s copy of it, `menu`'s `clips:` row
    /// and `svg::split_clip` do not narrow it at all. A repair that trusted this
    /// sentence and removed the loader's normalization would have culled a group's
    /// children against the group's own bounds.
    pub fn clips_children(&self) -> bool {
        matches!(self, NodeKind::Artboard { .. })
    }

    /// Whether this kind can be used as a **mask**, i.e. whether [`Node::mask`]
    /// means anything on it.
    ///
    /// [`Self::takes_paint`], minus a frame, plus `Group` — and the two
    /// exclusions are the whole of the rule:
    ///
    /// - **`Root` is not a layer.** It has no siblings to clip and no transform
    ///   anyone authored.
    /// - **A frame is a page.** It already carries the other clip
    ///   ([`Self::clips_children`]), and a page that also cut the pages after it
    ///   down to its own box is not a thing anyone means to ask for — the two
    ///   switches would sit on one layer looking like a pair. Figma allows it;
    ///   here the layers panel is the tell, since a frame's row already spends
    ///   its one badge slot on `800 × 600`.
    ///
    /// Everything else has an outline for `Resolved::mask_path` to derive:
    /// a shape's from its kind, a boolean's from the cache, a text node's from
    /// its glyphs, a group's from its contents unioned.
    ///
    /// Spelled here beside `clips_children` because the loader, the inspector
    /// and the op layer all have to ask it, and three lists of kinds would drift
    /// the first time a kind was added.
    ///
    /// ⚠️ **The `Artboard` exclusion is written out rather than inherited, and it
    /// stopped being free on 2026-09-01.** This was `takes_paint() || Group` while
    /// `takes_paint` answered `false` for a frame; giving frames a real fill list
    /// (§15 D400) made that spelling silently hand every frame a mask switch it
    /// must not have. Nothing would have failed — a frame would simply have grown
    /// a second clip control beside `clips_children`.
    pub fn can_mask(&self) -> bool {
        !matches!(self, NodeKind::Artboard { .. })
            && (self.takes_paint() || matches!(self, NodeKind::Group))
    }

    /// Whether this kind draws its own fills and strokes, i.e. whether
    /// [`Node::paint`] means anything on it.
    ///
    /// The two containers with no outline do not: `Root` is not a layer, and a
    /// `Group` is organisation. Everything with an outline does, **a frame
    /// included** — its outline is its own box ([`crate::geometry::local_path`])
    /// and it fills and strokes along it like any other shape.
    ///
    /// Spelled here rather than in the inspector because a bulk paint edit over
    /// a selection has to ask the same question the panel does — "which of these
    /// can take a colour at all" — and two lists of kinds would drift the first
    /// time a kind was added.
    ///
    /// ⚠️ **A frame joined this list on 2026-09-01 and it used to be the whole
    /// answer to a different question too** (§15 D400). Two predicates read it as
    /// a base and each has since been made to say what it means on its own:
    /// [`Self::can_mask`] excludes a frame explicitly, and `takes_stroke` — which
    /// existed only to add the frame this list was missing — is gone. Where a
    /// frame is now the odd one out, say so at the site.
    pub fn takes_paint(&self) -> bool {
        matches!(
            self,
            // **A frame paints like a shape**, which is what its own box being a
            // real outline means: one fill list, one stroke list, a `+` on each
            // and a reorder grip on both (§15 D400). The one thing that is still
            // a frame's own is *where* the ink lands — its fills sit behind its
            // children and its strokes over them, outside its clip (§15 D144) —
            // and that is the scene walk's business, not this predicate's.
            NodeKind::Artboard { .. }
                | NodeKind::Rect { .. }
                | NodeKind::Ellipse { .. }
                | NodeKind::Polygon { .. }
                | NodeKind::Star { .. }
                | NodeKind::Line { .. }
                | NodeKind::Path { .. }
                | NodeKind::Text { .. }
                // **The one container that paints.** Every other container is
                // excluded for having no outline; a boolean's outline is the entire
                // point of it, and its fills and strokes are what the operation
                // produces. Missed on the first pass, and the symptom was exact:
                // `build::boolean` inherits the bottom operand's paint, and the
                // transaction came back `WrongKindForOp`.
                | NodeKind::Boolean { .. }
        )
    }

    /// Default layer name for a freshly created node of this kind.
    pub fn default_name(&self) -> &'static str {
        match self {
            NodeKind::Root => "Root",
            // "Frame" is what the UI calls this everywhere the user can see —
            // the tool, the tooltips, the inspector. The variant keeps the
            // older name because it is also the tag in the save format.
            NodeKind::Artboard { .. } => "Frame",
            NodeKind::Group => "Group",
            NodeKind::Rect { .. } => "Rectangle",
            NodeKind::Ellipse { .. } => "Ellipse",
            NodeKind::Polygon { .. } => "Polygon",
            NodeKind::Star { .. } => "Star",
            NodeKind::Line { .. } => "Line",
            NodeKind::Path { .. } => "Path",
            NodeKind::Text { .. } => "Text",
            // Named for the operation, as Figma does: the layer list is where you
            // read what a boolean group is doing, and "Boolean" alone would make
            // four different shapes share one name.
            NodeKind::Boolean { op } => op.label(),
        }
    }
}

/// Fills + strokes. **As many of each as the layer carries**, painted in list
/// order: every visible fill, then every visible stroke, so the last element of
/// each list is the one on top (§5.3).
///
/// Two stacks rather than one interleaved appearance list, which is affordable
/// only because [`StrokeAlign::Outside`] covers the outlined-text case that would
/// otherwise need a stroke *under* a fill.
///
/// (This said "v1 uses 0..=1 of each; `Vec` is forward-compat" while the panels
/// could already add a second row and the scene walk drew only the first — so a
/// second stroke widened the cached world bounds and painted nothing. See §15 D56.)
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Paint {
    pub fills: Vec<Fill>,
    pub strokes: Vec<Stroke>,
}

impl Paint {
    /// The index of the first **visible image fill**, i.e. "is this layer a
    /// picture" and, if so, which fill is the picture.
    ///
    /// **Here rather than in the app, because core now asks it too.** It was
    /// `tools::croppable_fill` and had sixteen callers on that side — the crop
    /// mode, the double-click that opens it, the context menu, the inspector's
    /// Asset card — and `build::mask_target` made it a question the *model* has,
    /// namely which of a set of layers is the photograph and which is the shape.
    /// That app-side function is now one line onto this, so there is still one
    /// spelling of the rule and the sixteen call sites did not move.
    ///
    /// **Visible, and the first one.** A hidden image fill is not a picture the
    /// user is looking at, and the first is the one every consumer means: the
    /// crop mode edits it and the mask rule is only asking a yes/no.
    pub fn image_fill(&self) -> Option<usize> {
        self.fills
            .iter()
            .position(|f| f.visible && matches!(f.brush, Brush::Image(_)))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fill {
    pub brush: Brush,
    pub visible: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub brush: Brush,
    pub width: f64,
    pub join: kurbo::Join,
    pub cap: kurbo::Cap,
    /// The dash pattern, as kurbo and `stroke-dasharray` take it: on, off, on…
    /// Empty is a solid stroke.
    ///
    /// **The single source of truth for the pattern, and the only thing the
    /// dash UI stores.** A dotted stroke is a *zero-length* dash — `[0, s]` —
    /// which makes `s` unambiguously the centre-to-centre spacing, and which is
    /// how [`Stroke::dash_style`] can classify the pattern back into the panel's
    /// four cells without storing a mode beside it. The two width-dependent
    /// consequences of that choice — a zero-length dash vanishes under
    /// `Cap::Butt`, and a fitted pattern depends on the path — are resolved at
    /// the render boundary, where the width and the path are both to hand
    /// (§6.3). Storing derived numbers here instead would go stale the moment
    /// the width was scrubbed.
    pub dashes: Vec<f64>,
    pub dash_offset: f64,
    /// Which side of the outline the width is laid on.
    ///
    /// Defaulted rather than schema-versioned: it is a purely additive field, so
    /// a v1 file loads as `Center`, which is what it drew as. See §5.11.
    #[serde(default)]
    pub align: StrokeAlign,
    /// Which of a box's four sides the stroke occupies. Additive like `align`,
    /// and for the same reason: every existing file means [`StrokeSides::All`],
    /// which is the default and is exactly how those files drew.
    #[serde(default, skip_serializing_if = "StrokeSides::is_all")]
    pub sides: StrokeSides,
    /// Mitre limit as the **ratio** kurbo takes and `stroke-miterlimit` emits,
    /// not the angle the panel shows.
    ///
    /// Stored in the renderer's own unit deliberately. The UI works in degrees,
    /// because "corners sharper than this get bevelled" is the question a
    /// designer is actually asking, but converting at the UI edge once beats
    /// converting on every render *and* every export — and beats writing
    /// `3.9998` into files, which is what round-tripping 28.96° through degrees
    /// produces.
    #[serde(default = "default_miter_limit")]
    pub miter_limit: f64,
    /// Scale the dash pattern so a whole number of periods fits the outline —
    /// the "fit to corners" toggle.
    ///
    /// A flag rather than a one-off recompute written back into `dashes`. Write
    /// back and the toggle is true only until the shape next changes size,
    /// while still reading as on; a flag honoured at the render boundary stays
    /// true across a resize and keeps the typed numbers visible in the fields.
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
/// in stroke widths. The pair lives here rather than in the panel because the
/// stored value is the ratio (see [`Stroke::miter_limit`]) and both directions
/// are needed to show it as an angle.
pub fn miter_ratio_of_degrees(degrees: f64) -> f64 {
    let half = (degrees.clamp(MIN_MITER_DEGREES, 180.0) * 0.5).to_radians();
    (1.0 / half.sin()).max(1.0)
}

/// The limiting angle a mitre ratio implies, in degrees.
pub fn miter_degrees_of_ratio(ratio: f64) -> f64 {
    2.0 * (1.0 / ratio.max(1.0)).clamp(0.0, 1.0).asin().to_degrees()
}

/// Sharpest angle the mitre field will accept. The ratio goes to infinity as the
/// angle goes to zero, so the low end needs a floor or the field is a way to ask
/// for an unbounded spike; 1° is ratio ≈ 114.6, far past any shape that is not a
/// hairline.
pub const MIN_MITER_DEGREES: f64 = 1.0;

/// Which sides of a box's outline a stroke occupies — CSS's `border-top` and
/// friends.
///
/// **Only a box has four nameable sides.** [`NodeKind::Rect`] does; an ellipse,
/// a polygon, a star and a path do not, and neither does a line. Anywhere else
/// the stroke is the whole outline whatever this says, which is the same bargain
/// [`StrokeAlign`] makes on an open path — the control is gated in the panel
/// rather than the value being silently rewritten.
///
/// A side stroke stops being the closed outline, so the scene walk and the SVG
/// writer emit an **open** path per side, with each corner arc split at its
/// midpoint so a rounded rect's top edge owns half of each top corner (§6.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum StrokeSides {
    /// The closed outline — what every stroke was before this field existed, and
    /// what a file without it still means.
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
    /// `All` answers `None` four times, because it is not drawn side by side at
    /// all: it is the one case that keeps the closed outline, and a caller that
    /// treated four full-width sides as equivalent would lose the join at every
    /// corner. [`StrokeSides::per_side`] is the question with that case excluded.
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
    /// Deliberately one number for all four sides rather than per-side insets:
    /// bounds may be generous and must never be tight, and the callers
    /// ([`crate::geometry::world_bounds_of_parts`], the hit-test tolerance)
    /// inflate a rect uniformly. A top-only stroke therefore reports a box a
    /// little larger than its ink on three sides, which costs a slightly wide
    /// selection rect and cannot cost a missed hit.
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
/// **Classified from `dashes`, not stored.** The classification this replaced
/// compared the first dash against `width * 1.5`, which worked only while the
/// numbers were derived from the width — a typed 12-long dash on a 9px stroke
/// came back as *dotted*. This one is invariant under width: a dotted stroke is
/// a zero-length dash, which is the shape of the value rather than its size, so
/// there is no stored mode to drift from the pattern it names.
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
    /// The *period* rather than the gap, which is what makes the field mean one
    /// thing: with a zero-length dash the two coincide, and the substitution the
    /// renderer makes for a square dot preserves the period rather than the gap
    /// precisely so this stays true (§6.3).
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
            brush: Brush::Solid(peniko::Color::BLACK),
            width: 1.0,
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
/// Not a renderer concept — no 2D backend in the stack (vello, vello_cpu, SVG)
/// has stroke alignment, and all three build it the same way: stroke at twice
/// the width and clip to the side you want. That construction lives in
/// `ondin-render`'s scene walk and the SVG writer, from this one field.
///
/// Only meaningful for closed shapes; an open path has no inside, so `Line` and
/// unclosed `Path` nodes draw centred whatever this says.
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
/// **Three states, not two.** Auto width and auto height are genuinely different
/// things — one refuses to wrap at all, the other wraps at a width the user set
/// and grows downwards — and collapsing them into one `Auto` meant a node that
/// wrapped could not also grow. `Auto` and `Fixed` keep their names (and so their
/// bytes) because every file written before this held one of the two.
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
/// (§15 D668, `[S2.1-L6-04]`).
///
/// `NodeKind` is the only kind-like enum in this crate with no `ALL`: `MaskMode`,
/// `BoolOp`, `DashStyle` and `Side` all have one, and its variants carry payloads,
/// so it cannot. Six `matches!` predicates decide real behaviour from it and
/// every one of them answers `false` for a variant nobody added to it — with all
/// six gates green, because a `matches!` arm nobody wrote is not a compile error.
/// The codebase has paid for that twice already, and both times the note is still
/// at the site: `resolve::is_indexed`'s *"left out at first, and the symptom was a
/// boolean that could be selected from the layers tree and not from the canvas"*,
/// and `takes_paint`'s *"the transaction came back `WrongKindForOp`"*.
#[cfg(test)]
pub(crate) mod kind_fixture {
    use super::*;

    /// The variant's name, from an exhaustive `match`.
    ///
    /// 🚨 **This function is the gate and the only one there is.** It has no
    /// wildcard, so a twelfth `NodeKind` does not compile here — and the compiler
    /// error is what sends the author to `all` below, which is the list every
    /// membership test iterates. **A new arm here needs a new sample there**;
    /// nothing checks that pairing, so it is stated at the point the error lands
    /// rather than in a doc nobody is reading at the time.
    ///
    /// It is also the label the assertions compare, so a set is read as names
    /// rather than as `Debug` output over payloads that are irrelevant to it.
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
            },
            NodeKind::Star {
                size: one,
                points: 5,
                inner_ratio: 0.5,
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
    /// rejects**.
    ///
    /// 🚨 **The second list is the whole point of the matrix.** Asserting only the
    /// accepted set converts nothing: a twelfth variant answers `false`, the
    /// accepted set is unchanged and the test stays green — which is the failure
    /// being guarded against, reproduced inside the guard. Spelling the rejected
    /// set means the new name has to land in one list or the other, and either way
    /// an assertion goes red naming the predicate it belongs to.
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

    /// The fixture describes each variant once — the one thing about it that can
    /// be checked without a second enumeration to check it against.
    ///
    /// ⚠️ **It does not check that the fixture is *complete*, and cannot.** A
    /// variant added to `tag`'s `match` and forgotten here leaves eleven distinct
    /// names for twelve variants and nothing in the crate disagrees; the compile
    /// error in `tag` is the only thing that ever says so, which is why that is
    /// where the instruction lives.
    ///
    /// **Flip:** duplicate any entry of `all()` and this fails, naming the
    /// repeated tag.
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
    /// One test rather than six, because the *comparison* is half of what is worth
    /// asserting: `is_indexed` and `takes_paint` have identical membership today,
    /// spelled twice in two modules for two unrelated reasons, and `takes_paint`'s
    /// own doc predicts they will drift. Side by side, a change to one that was
    /// meant for both is visible in the diff.
    ///
    /// **Three flips, all three predicted correctly.** Deleting
    /// `NodeKind::Boolean { .. }` from `resolve::is_indexed` fails on the
    /// `is_indexed` pair — which is the regression the comment at that site
    /// records, now a red test instead of a canvas nobody can click. Deleting
    /// `Artboard` from `stroke_sides_apply` fails there and nowhere else.
    ///
    /// ⚠️ **The third is the one that matters and it is a flip of the *expectation*,
    /// because the case cannot be reached from the code.** What is being guarded
    /// against is a twelfth variant, and simulating one means adding one. So the
    /// shape was reproduced instead: `Root` is rejected by all six predicates, i.e.
    /// it is exactly what a new variant looks like here — removing `"Root"` from
    /// `clips_children`'s rejected list fails with the actual list naming it and
    /// the expected one not. That is the whole mechanism, and it is why the
    /// rejected sets are spelled out at all.
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
