//! Operations, transactions, and errors (§5.7).
//!
//! Ids are minted by the caller and carried inside ops; `apply` never allocates
//! (invariant 3). A `Transaction` applies atomically = one undo entry.

use crate::effect::Effect;
use crate::export::ExportSpec;
use crate::guide::{Guide, GuideId};
use crate::id::NodeId;
use crate::image::{ImageEntry, ImageId};
use crate::item::Keyed;
use crate::node::{Fill, FillRule, MaskMode, Node, NodeKind, Pivot, Stroke, TextStyle};
use kurbo::{Affine, BezPath, Point, RoundedRectRadii, Size};
use peniko::Color;
use rustc_hash::FxHashSet;

/// What a description belongs to ([`Operation::SetDescription`]): a main (or a
/// set), a style or a variable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Described {
    Node(NodeId),
    Style(crate::style::StyleId),
    Variable(crate::variable::VarId),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
    CreateNode {
        id: NodeId,
        parent: NodeId,
        index: usize,
        kind: NodeKind,
        transform: Option<Affine>,
        name: Option<String>,
    },
    DeleteNode {
        id: NodeId,
    },
    /// Paste / duplicate / import: nodes arrive with freshly-minted ids and are
    /// validated as a well-formed subtree.
    InsertSubtree {
        nodes: Vec<Node>,
        parent: NodeId,
        index: usize,
    },
    Reparent {
        id: NodeId,
        new_parent: NodeId,
        index: usize,
    },
    Reorder {
        id: NodeId,
        index: usize,
    },
    SetTransform {
        id: NodeId,
        transform: Affine,
    },
    SetGeometry {
        id: NodeId,
        geometry: GeometryPatch,
    },
    /// New content **and both span lists that go with it**.
    ///
    /// A span is a byte range into the content, so committing the string alone
    /// leaves every override past the edit point on the wrong characters.
    /// `TextEdit` re-bases both lists ([`crate::Spans::edited`]); the paragraph
    /// list needs it too, as a `ParaSpans` entry is a byte range as well.
    SetText {
        id: NodeId,
        content: String,
        spans: crate::node::CharSpans,
        para_spans: crate::node::ParaSpans,
    },
    /// The node's character *defaults* — what a byte with no span carries.
    ///
    /// **`spans` is how an undo puts back an override this dropped**: applying
    /// re-states the list against the new defaults (§15 D163). `None` means
    /// "re-state"; `Some` installs exactly this list, and the inverse carries one.
    SetTextStyle {
        id: NodeId,
        style: TextStyle,
        spans: Option<crate::node::CharSpans>,
    },
    /// The node's character overrides, without touching the content.
    ///
    /// Separate from [`Operation::SetText`] so an undo of "make this bold" cannot
    /// restore different text.
    SetTextSpans {
        id: NodeId,
        spans: crate::node::CharSpans,
    },
    /// The node's paragraph *defaults* (§5.4). `spans` is
    /// [`Operation::SetTextStyle`]'s, for the same reason (§15 D163).
    SetParagraphStyle {
        id: NodeId,
        paragraph: crate::node::ParagraphStyle,
        spans: Option<crate::node::ParaSpans>,
    },
    /// The paragraph twin of [`Operation::SetTextSpans`].
    SetParagraphSpans {
        id: NodeId,
        spans: crate::node::ParaSpans,
    },
    /// Vertical alignment, box trim, overflow and the line limit — the whole
    /// node, never a range.
    SetBlockStyle {
        id: NodeId,
        block: crate::node::BlockStyle,
    },
    SetName {
        id: NodeId,
        name: String,
    },
    SetVisible {
        id: NodeId,
        visible: bool,
    },
    SetLocked {
        id: NodeId,
        locked: bool,
    },
    /// The Transform panel's chain link: whether a resize of the node holds its
    /// aspect ratio as though Shift were held (`canvas::keep_ratio`). Not
    /// [`Operation::SetLocked`], which locks the layer against editing.
    SetProportionsLocked {
        id: NodeId,
        locked: bool,
    },
    SetOpacity {
        id: NodeId,
        opacity: f32,
    },
    /// Move the node's transform origin — the point rotation and mirroring hold
    /// still. `None` puts it back on the centre of the node's box (§5.3, [`Pivot`]).
    ///
    /// **Resize and skew deliberately ignore it**: each has a fixed point of its
    /// own, the opposite edge or the facing side (§15 D60).
    ///
    /// **Nothing composes this into the world transform.** The gesture reads the
    /// pivot and bakes it into the transform, so the render boundary, `Resolved`,
    /// hit-testing and the SVG writer never see it.
    SetPivot {
        id: NodeId,
        pivot: Option<Pivot>,
    },
    /// Whether a container clips its children to its own frame (§5.3).
    SetClip {
        id: NodeId,
        clip: bool,
    },
    /// Whether this layer is a mask for the siblings above it (§5.3).
    ///
    /// **One node, not a run.** The masked layers carry nothing — being masked
    /// is a fact about where they sit, so moving one out is a reorder.
    SetMask {
        id: NodeId,
        mask: bool,
    },
    /// How this layer masks — its outline, or its own alpha (§5.3).
    ///
    /// **Separate from `SetMask`, and writable while the layer is not a mask.**
    /// The mode is remembered across the flag going off and on.
    SetMaskMode {
        id: NodeId,
        mode: MaskMode,
    },
    /// How this layer's geometry decides its interior — non-zero or even-odd
    /// (§15 D239).
    ///
    /// Its own operation rather than part of a geometry patch, because it belongs
    /// to no particular kind and `GeometryPatch` is matched against the kind.
    /// `build::boolean` gives an `Exclude` an even-odd rule.
    SetFillRule {
        id: NodeId,
        rule: FillRule,
    },
    SetFills {
        id: NodeId,
        fills: Vec<Keyed<Fill>>,
    },
    SetStrokes {
        id: NodeId,
        strokes: Vec<Keyed<Stroke>>,
    },
    /// The whole effect stack (§5.3a): every edit of it is one operation with one
    /// inverse.
    SetEffects {
        id: NodeId,
        effects: Vec<Keyed<Effect>>,
    },
    /// The list of files this layer produces (§7), written whole like the other
    /// keyed lists.
    SetExports {
        id: NodeId,
        exports: Vec<Keyed<ExportSpec>>,
    },
    /// The columns and rows drawn over this layer (`crate::layout`), written whole.
    /// **Chrome, like the guide operations**: `false` to [`Self::changes_ink`].
    SetLayoutGrids {
        id: NodeId,
        grids: Vec<Keyed<crate::layout::LayoutGrid>>,
    },
    /// The layer's CSS insets and auto margins inside its frame
    /// (`crate::container`, §15 D871) — the whole set, as four insets and four
    /// margins are one property to the user.
    ///
    /// **Ink, unlike a grid**: an inset moves the layer, so it answers `true` to
    /// [`Self::changes_ink`].
    SetInsets {
        id: NodeId,
        insets: crate::container::Insets,
    },
    /// The layer's layout — CSS `display` with its properties, or `None` for a
    /// container whose children are placed by their own transforms
    /// (`crate::container::Display`, §15 D867). The whole value.
    SetDisplay {
        id: NodeId,
        display: Option<crate::container::Display>,
    },
    /// The layer's properties as a layout item, flex or grid
    /// (`crate::container::LayoutItem`) — the whole set.
    SetLayoutItem {
        id: NodeId,
        item: crate::container::LayoutItem,
    },
    /// Make a frame or group a main component, or stop it being one (§15 D978).
    /// The rules are checked after the last op (`crate::component::check`), so a
    /// verb may order the change and its consequences either way.
    SetComponent {
        id: NodeId,
        component: bool,
    },
    /// Link a node to the node it was copied from, or cut the link (§15 D978).
    /// Checked after the last op, as [`Self::SetComponent`] is.
    SetLink {
        id: NodeId,
        link: Option<NodeId>,
    },
    /// Swap a nested copy's contents to another main, or back to its slot's
    /// (§15 D983). The field alone: the commit expands it into the copy's
    /// rewritten children (`crate::swap::settle`). Checked after the last op.
    SetSwap {
        id: NodeId,
        swap: Option<NodeId>,
    },
    /// Make a frame a component set with these variant properties, or stop it
    /// being one (§15 D982). Checked after the last op (`crate::variant::check`).
    SetVariantSet {
        id: NodeId,
        set: Option<crate::variant::VariantSet>,
    },
    /// A variant's values, one per property of its set (§15 D982).
    SetVariant {
        id: NodeId,
        values: Vec<String>,
    },
    /// The component properties a main or a set defines (§15 D982), written whole.
    SetProperties {
        id: NodeId,
        props: Vec<Keyed<crate::variant::Property>>,
    },
    /// The description a main, a style or a variable carries: free text for the
    /// people and the agents using it, never drawn. Empty is none.
    SetDescription {
        of: Described,
        text: String,
    },
    /// The ground behind and around a page's frames (`crate::page`). A solid
    /// colour, not a `Brush`: nothing lies behind it for a gradient or an alpha
    /// to act on.
    SetPageGround {
        id: NodeId,
        ground: Color,
    },
    /// Add an empty page at `index` of the page order. Inverts to
    /// [`Self::RemovePage`]. The caller mints the id, as for a node.
    ///
    /// **Carries everything a page has besides its layers**, so the inverse of a
    /// removal brings back the page that was there, not a default one.
    AddPage {
        id: NodeId,
        index: usize,
        name: String,
        ground: Color,
        draft: bool,
        mode: Option<crate::variable::ModeId>,
    },
    /// Remove a page. Refused for the last page and for one still holding layers
    /// or guides: deleting a page is those deletes first, then this, so undo
    /// re-adds the page before anything goes back on it.
    RemovePage {
        id: NodeId,
    },
    /// Move a page to `index` of the page order.
    MovePage {
        id: NodeId,
        index: usize,
    },
    /// Mark a page as a draft or clear it (`crate::page::Page::draft`).
    SetPageDraft {
        id: NodeId,
        draft: bool,
    },
    /// Place a ruler guide. The caller mints the id, as for a node (§5.2,
    /// invariant 3).
    AddGuide {
        guide: Guide,
    },
    RemoveGuide {
        id: GuideId,
    },
    /// Move a guide along the axis it constrains.
    SetGuidePosition {
        id: GuideId,
        position: f64,
    },
    /// Recolour a guide, or (`None`) put it back on the shared default.
    SetGuideColor {
        id: GuideId,
        color: Option<Color>,
    },
    /// Rescope a guide: which frame or page owns it, and where it sits in that
    /// owner's space (a page's is world space).
    ///
    /// **The two fields move together or not at all.** `position` is read in the
    /// owner's space (`Guide::position`), so changing the owner alone would
    /// reinterpret the same coordinate in another space, and an inverse built from
    /// that state would restore the jump.
    ///
    /// Dragging a guide across a frame's boundary does this, and it inverts to
    /// itself carrying the old pair.
    SetGuideScope {
        id: GuideId,
        owner: NodeId,
        position: f64,
    },
    /// Put an image's bytes in the document's table (`crate::image`).
    ///
    /// **The bytes travel in the operation, which is what makes undo whole**: this
    /// inverts to [`Self::RemoveImage`] and that back to *this*, entry included,
    /// so no inverse restores a reference to nothing.
    ///
    /// The caller mints the id, a content hash of the encoded bytes, so placing
    /// the same file twice deduplicates.
    AddImage {
        id: ImageId,
        entry: ImageEntry,
    },
    /// Take an image out of the table.
    ///
    /// **Not reached by deleting a layer** — nothing collects the table (see
    /// `Document::images`); this is *Purge unused*, *Unlink* and half a replace.
    RemoveImage {
        id: ImageId,
    },
    /// Open the document's **vault** — the parentless `Root` that holds vendored
    /// kit mains, beside the document's own (§5.3e, §15 D1012). Inverts to
    /// [`Self::CloseVault`]; refused when a vault is open.
    ///
    /// **An operation rather than a side effect of the first vendoring insert**,
    /// so the insert's undo takes the vault away with it.
    OpenVault {
        id: NodeId,
    },
    /// Close the vault. Refused unless it is open, is `id`, and is empty.
    CloseVault {
        id: NodeId,
    },
    /// Set or clear a node's kit provenance (§5.3e, §15 D1016). `crate::kit::check`
    /// holds where an origin may be, after the last op.
    SetOrigin {
        id: NodeId,
        origin: Option<crate::kit::Origin>,
    },
    /// Put a variable in the document's table (§5.3f, §15 D1039). Cells must cover
    /// every mode and the name be free (`crate::variable::check`, after the last
    /// op). Inverts to [`Self::RemoveVariable`], which carries the whole variable
    /// back.
    AddVariable {
        var: crate::variable::Variable,
    },
    /// Take a variable out of the table. **Refused at the commit while anything is
    /// bound to it**; the delete verb detaches its users in the same transaction.
    RemoveVariable {
        id: crate::variable::VarId,
    },
    /// Replace a variable whole — name, cells and origin — as a kit update does
    /// (§15 D1064). The id must be there; its kind may not change. Inverts to the
    /// variable it replaced.
    SetVariable {
        var: crate::variable::Variable,
    },
    /// One kit's modes and its mapping onto the document's (§15 D1064), `None` to
    /// drop them. Written whole; inverts to what it replaced.
    SetKitModes {
        kit: String,
        modes: Option<crate::variable::KitModes>,
    },
    /// Rename a variable — a slash path, whose groups are the browser's.
    SetVariableName {
        id: crate::variable::VarId,
        name: String,
    },
    /// One cell of a variable: its value, or an alias, in one mode.
    SetVariableCell {
        id: crate::variable::VarId,
        mode: crate::variable::ModeId,
        cell: crate::variable::Cell,
    },
    /// Add a mode at `index`, with **every variable's cell in it**, so the table
    /// is whole after this one op. Inverts to [`Self::RemoveMode`].
    AddMode {
        mode: crate::variable::Mode,
        index: usize,
        cells: Vec<(crate::variable::VarId, crate::variable::Cell)>,
    },
    /// Remove a mode and its column. Inverts to [`Self::AddMode`] with the column
    /// it took; frames set to it must return to *Inherit* in the same
    /// transaction, or the commit refuses.
    RemoveMode {
        id: crate::variable::ModeId,
    },
    SetModeName {
        id: crate::variable::ModeId,
        name: String,
    },
    /// The mode a frame sets for itself and its contents, `None` to inherit
    /// (§5.3f). Only on a frame.
    SetMode {
        id: NodeId,
        mode: Option<crate::variable::ModeId>,
    },
    /// The variables a rectangle's corners are bound to (§15 D1039). Writes the
    /// bindings alone; `build::bind_corners_ops` writes the literals beside it.
    SetCornerVars {
        id: NodeId,
        vars: crate::variable::CornerVars,
    },
    /// The variables a container's gaps and padding are bound to (§15 D1058).
    /// Writes the bindings alone (`build::bind_spacing_ops` writes the literals).
    SetSpacingVars {
        id: NodeId,
        vars: crate::variable::SpacingVars,
    },
    /// The boolean variable a layer's visibility is bound to, `None` to read its
    /// literal (§15 D1059). Writes the binding alone.
    SetVisibleVar {
        id: NodeId,
        var: Option<crate::variable::VarId>,
    },
    /// The number variables a layer's width and height are bound to (§15
    /// D1061). Writes the bindings alone (`build::bind_size_ops` writes the literals).
    SetSizeVars {
        id: NodeId,
        vars: crate::variable::SizeVars,
    },
    /// The string variables a text's content and family are bound to (§15
    /// D1069). Writes the bindings alone (`build::bind_text_ops` writes the literals).
    SetTextVars {
        id: NodeId,
        vars: crate::variable::TextVars,
    },
    /// The number variable a layer's opacity is bound to (§15 D1072). Writes
    /// the binding alone (`build::bind_opacity_ops` writes the literal).
    SetOpacityVar {
        id: NodeId,
        var: Option<crate::variable::VarId>,
    },
    /// Put a style in the document's table (§15 D1062). Its name must be free in
    /// its slot — `crate::style::check`, after the last op. Inverts to
    /// [`Self::RemoveStyle`], and that back to this carrying the whole style.
    AddStyle {
        style: crate::style::Style,
    },
    /// Take a style out of the table. **Refused at the commit while any layer is
    /// bound to it**; `crate::style::remove_ops` detaches its users in the same
    /// transaction.
    RemoveStyle {
        id: crate::style::StyleId,
    },
    /// Rename a style — a slash path, whose groups are the browser's.
    SetStyleName {
        id: crate::style::StyleId,
        name: String,
    },
    /// A style's whole value, of the slot it already has. Its users are rewritten
    /// by the commit, live to every one of them (`crate::style::settle`).
    SetStyleValue {
        id: crate::style::StyleId,
        value: crate::style::StyleValue,
    },
    /// A kit's style's origin — its version moved by an update (§15 D1064).
    SetStyleOrigin {
        id: crate::style::StyleId,
        origin: Option<crate::style::StyleOrigin>,
    },
    /// The style a layer's fills are bound to, `None` to detach. Writes the
    /// binding alone; the commit writes the style's values.
    SetFillStyle {
        id: NodeId,
        style: Option<crate::style::StyleId>,
    },
    /// The style a layer's strokes are bound to, as [`Self::SetFillStyle`].
    SetStrokeStyle {
        id: NodeId,
        style: Option<crate::style::StyleId>,
    },
    /// The style a layer's effects are bound to, as [`Self::SetFillStyle`].
    SetEffectsStyle {
        id: NodeId,
        style: Option<crate::style::StyleId>,
    },
    /// The text style a text layer's type is bound to, as [`Self::SetFillStyle`]
    /// (§15 D1067). Not [`Self::SetTextStyle`], which writes the character
    /// defaults themselves.
    SetTypeStyle {
        id: NodeId,
        style: Option<crate::style::StyleId>,
    },
    /// The layout style a frame or a group's container is bound to, as
    /// [`Self::SetFillStyle`] (§15 D1075). Not [`Self::SetDisplay`], which writes
    /// the layout itself.
    SetLayoutStyle {
        id: NodeId,
        style: Option<crate::style::StyleId>,
    },
}

impl Operation {
    /// The node this operation **overwrites a field of**, or `None` when it is
    /// not that kind of operation.
    ///
    /// [`crate::History`] asks this to decide whether one edit may fold into the
    /// step before it: merging keeps the *older* inverse, which restores the
    /// original state only if the second edit replaced exactly what the first did.
    /// That holds for an absolute setter and fails for everything structural.
    ///
    /// The guide operations answer `None`: they name a `GuideId`, not a `NodeId`.
    /// A guide nudge folds through [`Self::shape_key`] instead — **two questions
    /// that look like one**: the subject of an edit, and the key a run merges on
    /// (§15 D482).
    pub fn overwrites(&self) -> Option<NodeId> {
        match self {
            Operation::SetTransform { id, .. }
            | Operation::SetGeometry { id, .. }
            | Operation::SetText { id, .. }
            | Operation::SetTextStyle { id, .. }
            | Operation::SetTextSpans { id, .. }
            | Operation::SetParagraphStyle { id, .. }
            | Operation::SetParagraphSpans { id, .. }
            | Operation::SetBlockStyle { id, .. }
            | Operation::SetName { id, .. }
            | Operation::SetVisible { id, .. }
            | Operation::SetLocked { id, .. }
            | Operation::SetProportionsLocked { id, .. }
            | Operation::SetOpacity { id, .. }
            | Operation::SetPivot { id, .. }
            | Operation::SetClip { id, .. }
            | Operation::SetMask { id, .. }
            | Operation::SetMaskMode { id, .. }
            | Operation::SetFillRule { id, .. }
            | Operation::SetFills { id, .. }
            | Operation::SetStrokes { id, .. }
            | Operation::SetEffects { id, .. }
            | Operation::SetExports { id, .. }
            | Operation::SetLayoutGrids { id, .. }
            | Operation::SetInsets { id, .. }
            | Operation::SetDisplay { id, .. }
            | Operation::SetLayoutItem { id, .. }
            | Operation::SetComponent { id, .. }
            | Operation::SetLink { id, .. }
            | Operation::SetSwap { id, .. }
            | Operation::SetVariantSet { id, .. }
            | Operation::SetVariant { id, .. }
            | Operation::SetProperties { id, .. }
            | Operation::SetMode { id, .. }
            | Operation::SetCornerVars { id, .. }
            | Operation::SetSpacingVars { id, .. }
            | Operation::SetVisibleVar { id, .. }
            | Operation::SetSizeVars { id, .. }
            | Operation::SetTextVars { id, .. }
            | Operation::SetOpacityVar { id, .. }
            | Operation::SetFillStyle { id, .. }
            | Operation::SetStrokeStyle { id, .. }
            | Operation::SetEffectsStyle { id, .. }
            | Operation::SetTypeStyle { id, .. }
            | Operation::SetLayoutStyle { id, .. }
            | Operation::SetOrigin { id, .. } => Some(*id),
            Operation::CreateNode { .. }
            | Operation::DeleteNode { .. }
            | Operation::InsertSubtree { .. }
            | Operation::Reparent { .. }
            | Operation::Reorder { .. }
            // The page table: a page's ground and draft flag are not node fields,
            // and adding, removing and moving pages is structural.
            | Operation::SetPageGround { .. }
            | Operation::SetPageDraft { .. }
            | Operation::AddPage { .. }
            | Operation::RemovePage { .. }
            | Operation::MovePage { .. }
            | Operation::AddGuide { .. }
            | Operation::RemoveGuide { .. }
            | Operation::SetGuidePosition { .. }
            | Operation::SetGuideColor { .. }
            | Operation::SetGuideScope { .. }
            // The image table is document-level, like the guides and the ground:
            // these name an `ImageId` and no node at all.
            | Operation::AddImage { .. }
            | Operation::RemoveImage { .. }
            // Structural: the vault is a root coming and going (§15 D1012).
            | Operation::OpenVault { .. }
            | Operation::CloseVault { .. }
            // The variables table is document-level too (§5.3f): these name a
            // variable or a mode, never a node.
            | Operation::AddVariable { .. }
            | Operation::RemoveVariable { .. }
            | Operation::SetVariable { .. }
            | Operation::SetVariableName { .. }
            | Operation::SetVariableCell { .. }
            | Operation::AddMode { .. }
            | Operation::RemoveMode { .. }
            | Operation::SetModeName { .. }
            | Operation::SetKitModes { .. }
            // And the styles table (§15 D1062): these name a style.
            | Operation::AddStyle { .. }
            | Operation::RemoveStyle { .. }
            | Operation::SetStyleName { .. }
            | Operation::SetStyleValue { .. }
            | Operation::SetStyleOrigin { .. }
            // A description is text beside the thing, never a field a copy
            // follows or an edit merges on.
            | Operation::SetDescription { .. } => None,
        }
    }

    /// The same edit aimed at `to` — every operation [`Self::overwrites`] names a
    /// node for, with its payload untouched; `None` for the rest. How the
    /// propagation pass (`crate::propagate`) carries a main's edit to a copy, and
    /// how it reads a copy's current value through the inverse.
    pub(crate) fn retargeted(&self, to: NodeId) -> Option<Operation> {
        let mut op = self.clone();
        let id = match &mut op {
            Operation::SetTransform { id, .. }
            | Operation::SetGeometry { id, .. }
            | Operation::SetText { id, .. }
            | Operation::SetTextStyle { id, .. }
            | Operation::SetTextSpans { id, .. }
            | Operation::SetParagraphStyle { id, .. }
            | Operation::SetParagraphSpans { id, .. }
            | Operation::SetBlockStyle { id, .. }
            | Operation::SetName { id, .. }
            | Operation::SetVisible { id, .. }
            | Operation::SetLocked { id, .. }
            | Operation::SetProportionsLocked { id, .. }
            | Operation::SetOpacity { id, .. }
            | Operation::SetPivot { id, .. }
            | Operation::SetClip { id, .. }
            | Operation::SetMask { id, .. }
            | Operation::SetMaskMode { id, .. }
            | Operation::SetFillRule { id, .. }
            | Operation::SetFills { id, .. }
            | Operation::SetStrokes { id, .. }
            | Operation::SetEffects { id, .. }
            | Operation::SetExports { id, .. }
            | Operation::SetLayoutGrids { id, .. }
            | Operation::SetInsets { id, .. }
            | Operation::SetDisplay { id, .. }
            | Operation::SetLayoutItem { id, .. }
            | Operation::SetComponent { id, .. }
            | Operation::SetLink { id, .. }
            | Operation::SetSwap { id, .. }
            | Operation::SetVariantSet { id, .. }
            | Operation::SetVariant { id, .. }
            | Operation::SetProperties { id, .. }
            | Operation::SetMode { id, .. }
            | Operation::SetCornerVars { id, .. }
            | Operation::SetSpacingVars { id, .. }
            | Operation::SetVisibleVar { id, .. }
            | Operation::SetSizeVars { id, .. }
            | Operation::SetTextVars { id, .. }
            | Operation::SetOpacityVar { id, .. }
            | Operation::SetFillStyle { id, .. }
            | Operation::SetStrokeStyle { id, .. }
            | Operation::SetEffectsStyle { id, .. }
            | Operation::SetTypeStyle { id, .. }
            | Operation::SetLayoutStyle { id, .. } => id,
            _ => return None,
        };
        *id = to;
        Some(op)
    }

    /// The key `History` merges a run on: the operation, the **field** inside it
    /// where there is one, and the subject (§15 D482).
    ///
    /// `History::commit_into_run` may drop the newer inverse only when the second
    /// edit overwrote *exactly* the fields the first did. The subject plus the
    /// operation's discriminant **is** the field set for every overwriting
    /// operation but one, each writing a fixed group of fields.
    ///
    /// ⚠️ **`SetGeometry` is the one exception, and the reason this function
    /// exists.** Its payload is itself a field selector: two commits carrying
    /// `Size` and `CornerRadius` would look identical to a discriminant-only key,
    /// merge, and one undo would restore the size and strand the radius with an
    /// empty stack.
    ///
    /// ⚠️ **`SetGuidePosition` is admitted here and not by [`Self::overwrites`]**.
    /// A guide's id wraps a `NodeId` (§5.2), so it makes a fine merge key, but it
    /// is not a node, and `overwrites`' callers ask which node an edit writes;
    /// widening it would hand them a guide's id as though it were a layer's. The
    /// ids cannot collide (guides and nodes share one `IdSource`).
    ///
    /// The other guide operations stay out: `AddGuide` and `RemoveGuide` are
    /// structural, and nothing repeats a colour or a scope.
    pub fn shape_key(
        &self,
    ) -> Option<(
        std::mem::Discriminant<Operation>,
        Option<std::mem::Discriminant<GeometryPatch>>,
        NodeId,
    )> {
        let subject = match self {
            Operation::SetGuidePosition { id, .. } => id.0,
            other => other.overwrites()?,
        };
        let field = match self {
            Operation::SetGeometry { geometry, .. } => Some(std::mem::discriminant(geometry)),
            _ => None,
        };
        Some((std::mem::discriminant(self), field, subject))
    }

    /// Whether applying this changes what is **drawn** — the artwork, not the
    /// chrome around it.
    ///
    /// It decides whether an edit is worth getting the selection box out of the
    /// way for (§15 D128): an invisible result has nothing to look at, and a
    /// chrome-only result would lose its one visible effect. The proportion lock,
    /// a pivot, a name and a guide are all `false`.
    ///
    /// **Close kin of `RenderOverrides::absorb`'s no-op arms, and not the same
    /// list.** Every op of that arm answers `false` here, but `absorb` also no-ops
    /// the image pair (§15 D659) and some variables-table ops (§15 D1039) that
    /// change ink with no override to carry it, and `absorb` *refuses* the vault's
    /// two and `SetOrigin` (§15 D1012, D1016), which answer `false` here.
    /// `overrides.rs` · `every_no_op_of_absorb_is_a_no_op_and_two_of_them_still_change_ink`
    /// pins the no-op side.
    ///
    /// **The two image operations are the standing gap**: they answer `true` here
    /// and are absorbed as no-ops there, because `RenderOverrides` has no image
    /// table to patch — that arm means "cannot preview this".
    ///
    /// No wildcard, so a new `Operation` stops compiling here and has to be
    /// classified (§15 D87–D88).
    pub fn changes_ink(&self) -> bool {
        match self {
            Operation::SetName { .. }
            | Operation::SetLocked { .. }
            | Operation::SetProportionsLocked { .. }
            | Operation::SetPivot { .. }
            // Export settings say what a file would contain; nothing on the
            // canvas draws from them.
            | Operation::SetExports { .. }
            // A layout grid is drawn over a frame and is never in it.
            | Operation::SetLayoutGrids { .. }
            // Being a main or linked is not drawn: the label chip and the
            // instance glyph are chrome (§15 D981).
            | Operation::SetComponent { .. }
            | Operation::SetLink { .. }
            // A swapped copy draws through the children its commit rewrites
            // (§15 D983).
            | Operation::SetSwap { .. }
            // A set's, variant's and component's properties are read by the
            // components machinery, never drawn (§15 D982).
            | Operation::SetVariantSet { .. }
            | Operation::SetVariant { .. }
            | Operation::SetProperties { .. }
            | Operation::AddGuide { .. }
            | Operation::RemoveGuide { .. }
            | Operation::SetGuidePosition { .. }
            | Operation::SetGuideColor { .. }
            | Operation::SetGuideScope { .. }
            // The vault is never drawn (§15 D1012).
            | Operation::OpenVault { .. }
            | Operation::CloseVault { .. }
            // A page comes and goes empty, and its place in the order and its
            // draft flag are read by the chrome, never drawn.
            | Operation::AddPage { .. }
            | Operation::RemovePage { .. }
            | Operation::MovePage { .. }
            | Operation::SetPageDraft { .. }
            // Provenance is a label on a node, never ink (§15 D1016).
            | Operation::SetOrigin { .. }
            // A variable's name is a row in the browser; a new or removed variable
            // is read by nothing (a removal is refused while bound, and the
            // detaches beside it are the ink). A mode's name is ink: a kit's
            // column is matched to a document mode by name.
            | Operation::AddVariable { .. }
            | Operation::RemoveVariable { .. }
            | Operation::SetVariableName { .. }
            // A style's table and a layer's binding draw nothing themselves; the
            // commit rewrites the layer's own lists beside them (§15 D1062).
            | Operation::AddStyle { .. }
            | Operation::RemoveStyle { .. }
            | Operation::SetStyleName { .. }
            | Operation::SetStyleValue { .. }
            | Operation::SetStyleOrigin { .. }
            | Operation::SetDescription { .. }
            | Operation::SetFillStyle { .. }
            | Operation::SetStrokeStyle { .. }
            | Operation::SetEffectsStyle { .. }
            | Operation::SetTypeStyle { .. }
            | Operation::SetLayoutStyle { .. } => false,
            Operation::CreateNode { .. }
            | Operation::DeleteNode { .. }
            | Operation::InsertSubtree { .. }
            | Operation::Reparent { .. }
            | Operation::Reorder { .. }
            | Operation::SetTransform { .. }
            | Operation::SetGeometry { .. }
            // An inset moves the layer — true even where it is inert, as for
            // `SetMaskMode` below: the answer depending on the parent would be a
            // second layout pass hidden in a predicate.
            | Operation::SetInsets { .. }
            // A layout moves every child of its container; an item's properties
            // move it and its siblings.
            | Operation::SetDisplay { .. }
            | Operation::SetLayoutItem { .. }
            | Operation::SetText { .. }
            | Operation::SetTextStyle { .. }
            | Operation::SetTextSpans { .. }
            | Operation::SetParagraphStyle { .. }
            | Operation::SetParagraphSpans { .. }
            | Operation::SetBlockStyle { .. }
            | Operation::SetVisible { .. }
            | Operation::SetOpacity { .. }
            | Operation::SetClip { .. }
            // **The most visible flag in this list**: it takes one layer off the
            // page and cuts every layer above it down to that outline.
            | Operation::SetMask { .. }
            // **True even when written to a layer that is not a mask**: the
            // question is about the operation, not the state it lands in, as for
            // `SetGeometry` on a shape already that size.
            | Operation::SetMaskMode { .. }
            // **True: a geometry change wearing a paint-shaped op.** The rule
            // decides which parts are inside, so hit testing and masks change too.
            | Operation::SetFillRule { .. }
            | Operation::SetFills { .. }
            | Operation::SetStrokes { .. }
            // **True, and the one paint op that can change what is drawn outside
            // its layer**: a drop shadow reaches past the node's box, which is why
            // `effect::stack_escape` exists and the bounds cache reads it.
            | Operation::SetEffects { .. }
            | Operation::SetPageGround { .. }
            // **True, though neither touches a node**: a fill referring to an id
            // the table lacks draws the placeholder (§15 D179), so the entry going
            // back changes the picture.
            | Operation::AddImage { .. }
            | Operation::RemoveImage { .. }
            // **True, though none names a node**: a cell is what every bound layer
            // draws, and adding or removing a mode can move the default every
            // unset frame reads in.
            | Operation::SetVariableCell { .. }
            // A kit's variable, modes and mapping set which column every bound
            // layer reads (§15 D1064); a document mode's name is a mapping too.
            | Operation::SetVariable { .. }
            | Operation::SetKitModes { .. }
            | Operation::SetModeName { .. }
            | Operation::AddMode { .. }
            | Operation::RemoveMode { .. }
            | Operation::SetMode { .. }
            | Operation::SetCornerVars { .. }
            | Operation::SetSpacingVars { .. }
            | Operation::SetVisibleVar { .. }
            | Operation::SetSizeVars { .. }
            | Operation::SetTextVars { .. }
            | Operation::SetOpacityVar { .. } => true,
        }
    }

    /// Whether applying this to `doc` would leave the document exactly as it
    /// stands.
    ///
    /// The per-operation half of [`Transaction::changes_nothing`], where the
    /// reason for asking is written down.
    ///
    /// **`false` is the safe answer, and every arm that cannot be certain gives
    /// it**: a wrong `true` silently drops an edit, a wrong `false` only commits an
    /// unwanted undo step. The match has no wildcard so a new variant must answer
    /// for itself.
    ///
    /// **An operation whose subject is missing answers `false`**, as do the kind
    /// gates `apply` enforces ([`Self::SetFills`] and [`Self::SetStrokes`] on an
    /// unpaintable kind, [`Self::SetEffects`] on the root), so the error still
    /// surfaces.
    ///
    /// ⚠️ **Equality of the value written, not of the bytes stored**: `-0.0` equals
    /// `0.0`, and `NaN` equals nothing, so a non-finite operation still reaches
    /// [`OpError::NonFinite`].
    pub fn changes_nothing(&self, doc: &crate::Document) -> bool {
        let node = |id: &NodeId| doc.get(*id);
        let kind = |id: &NodeId| doc.get(*id).map(Node::kind);
        match self {
            // **Structural, so never a no-op by inspection**: these change the tree
            // or fail, and `true` for an insert of zero nodes would suppress the
            // `MalformedSubtree`.
            Operation::CreateNode { .. }
            | Operation::DeleteNode { .. }
            | Operation::InsertSubtree { .. }
            | Operation::AddGuide { .. }
            | Operation::RemoveGuide { .. }
            | Operation::AddImage { .. }
            | Operation::RemoveImage { .. }
            | Operation::OpenVault { .. }
            | Operation::CloseVault { .. }
            | Operation::AddPage { .. }
            | Operation::RemovePage { .. }
            | Operation::AddVariable { .. }
            | Operation::RemoveVariable { .. }
            | Operation::AddMode { .. }
            | Operation::RemoveMode { .. }
            | Operation::AddStyle { .. }
            | Operation::RemoveStyle { .. } => false,
            Operation::MovePage { id, index } => doc.page_index(*id) == Some(*index),
            Operation::SetPageDraft { id, draft } => {
                doc.page(*id).is_some_and(|p| p.draft == *draft)
            }

            Operation::SetStyleName { id, name } => {
                doc.styles().get(*id).is_some_and(|s| s.name == *name)
            }
            Operation::SetStyleValue { id, value } => {
                doc.styles().get(*id).is_some_and(|s| s.value == *value)
            }
            Operation::SetStyleOrigin { id, origin } => {
                doc.styles().get(*id).is_some_and(|s| s.origin == *origin)
            }
            Operation::SetVariable { var } => doc.variables().get(var.id) == Some(var),
            Operation::SetKitModes { kit, modes } => {
                doc.variables().kits().get(kit) == modes.as_ref()
            }
            Operation::SetDescription { of, text } => match of {
                Described::Node(id) => node(id).is_some_and(|n| n.description == *text),
                Described::Style(id) => doc
                    .styles()
                    .get(*id)
                    .is_some_and(|s| s.description == *text),
                Described::Variable(id) => doc
                    .variables()
                    .get(*id)
                    .is_some_and(|v| v.description == *text),
            },
            Operation::SetFillStyle { id, style } => {
                node(id).is_some_and(|n| n.styles().fills == *style)
            }
            Operation::SetStrokeStyle { id, style } => {
                node(id).is_some_and(|n| n.styles().strokes == *style)
            }
            Operation::SetEffectsStyle { id, style } => {
                node(id).is_some_and(|n| n.styles().effects == *style)
            }
            Operation::SetTypeStyle { id, style } => {
                node(id).is_some_and(|n| n.styles().text == *style)
            }
            Operation::SetLayoutStyle { id, style } => {
                node(id).is_some_and(|n| n.styles().layout == *style)
            }

            Operation::SetVariableName { id, name } => {
                doc.variables().get(*id).is_some_and(|v| v.name == *name)
            }
            Operation::SetVariableCell { id, mode, cell } => doc
                .variables()
                .get(*id)
                .is_some_and(|v| v.cells.get(mode) == Some(cell)),
            Operation::SetModeName { id, name } => {
                doc.variables().mode(*id).is_some_and(|m| m.name == *name)
            }
            Operation::SetMode { id, mode } => node(id).is_some_and(|n| n.mode() == *mode),
            Operation::SetCornerVars { id, vars } => {
                node(id).is_some_and(|n| n.corner_vars() == *vars)
            }
            Operation::SetSpacingVars { id, vars } => {
                node(id).is_some_and(|n| n.spacing_vars() == *vars)
            }
            Operation::SetVisibleVar { id, var } => {
                node(id).is_some_and(|n| n.visible_var() == *var)
            }
            Operation::SetSizeVars { id, vars } => node(id).is_some_and(|n| n.size_vars() == *vars),
            Operation::SetTextVars { id, vars } => node(id).is_some_and(|n| n.text_vars() == *vars),
            Operation::SetOpacityVar { id, var } => {
                node(id).is_some_and(|n| n.opacity_var() == *var)
            }

            // **The index is read the way `apply` writes it**: remove, then
            // re-insert at `index`, so the list is unchanged exactly when `index`
            // is where the child already sits.
            Operation::Reparent {
                id,
                new_parent,
                index,
            } => {
                node(id).and_then(Node::parent) == Some(*new_parent)
                    && child_index(doc, *id) == Some(*index)
            }
            Operation::Reorder { id, index } => child_index(doc, *id) == Some(*index),

            Operation::SetOrigin { id, origin } => {
                node(id).is_some_and(|n| n.origin() == origin.as_ref())
            }

            Operation::SetTransform { id, transform } => {
                node(id).is_some_and(|n| n.transform() == *transform)
            }
            // **Asked of the result rather than of the patch**, which makes this
            // arm exact for every patch shape: a patch that does not match the
            // kind answers `None` and so `false`.
            Operation::SetGeometry { id, geometry } => node(id).is_some_and(|n| {
                geometry
                    .applied_to(n.kind())
                    .is_some_and(|would_be| would_be == *n.kind())
            }),

            // **The span-carrying arms compare what `apply` would store, not what
            // the caller handed over**: each clamps or re-states its list, so a
            // difference the clamp would drop is a no-op.
            Operation::SetText {
                id,
                content,
                spans,
                para_spans,
            } => matches!(
                kind(id),
                Some(NodeKind::Text {
                    content: c,
                    spans: sp,
                    para_spans: psp,
                    style,
                    paragraph,
                    ..
                }) if c == content
                    && *sp == spans.clamped(content, style)
                    && *psp == para_spans.clamped(content, paragraph)
            ),
            Operation::SetTextStyle { id, style, spans } => matches!(
                kind(id),
                Some(NodeKind::Text {
                    content,
                    style: s,
                    spans: sp,
                    ..
                }) if **s == *style
                    && match spans {
                        // `None` re-states against unchanged defaults, so the
                        // question is whether the stored list is already a fixed
                        // point of the re-statement.
                        None => *sp == sp.restated(style),
                        Some(given) => *sp == given.clamped(content, style),
                    }
            ),
            Operation::SetTextSpans { id, spans } => matches!(
                kind(id),
                Some(NodeKind::Text {
                    content,
                    style,
                    spans: sp,
                    ..
                }) if *sp == spans.clamped(content, style)
            ),
            Operation::SetParagraphStyle {
                id,
                paragraph,
                spans,
            } => matches!(
                kind(id),
                Some(NodeKind::Text {
                    content,
                    paragraph: p,
                    para_spans: sp,
                    ..
                }) if p == paragraph
                    && match spans {
                        None => *sp == sp.restated(paragraph),
                        Some(given) => *sp == given.clamped(content, paragraph),
                    }
            ),
            Operation::SetParagraphSpans { id, spans } => matches!(
                kind(id),
                Some(NodeKind::Text {
                    content,
                    paragraph,
                    para_spans: sp,
                    ..
                }) if *sp == spans.clamped(content, paragraph)
            ),
            Operation::SetBlockStyle { id, block } => matches!(
                kind(id),
                Some(NodeKind::Text { block: b, .. }) if b == block
            ),

            Operation::SetName { id, name } => node(id).is_some_and(|n| n.name() == name),
            Operation::SetVisible { id, visible } => {
                node(id).is_some_and(|n| n.visible() == *visible)
            }
            Operation::SetLocked { id, locked } => node(id).is_some_and(|n| n.locked() == *locked),
            Operation::SetProportionsLocked { id, locked } => {
                node(id).is_some_and(|n| n.proportions_locked() == *locked)
            }
            Operation::SetOpacity { id, opacity } => {
                node(id).is_some_and(|n| n.opacity() == *opacity)
            }
            Operation::SetPivot { id, pivot } => node(id).is_some_and(|n| n.pivot() == *pivot),
            Operation::SetClip { id, clip } => node(id).is_some_and(|n| n.clip() == *clip),
            Operation::SetMask { id, mask } => node(id).is_some_and(|n| n.mask() == *mask),
            Operation::SetMaskMode { id, mode } => node(id).is_some_and(|n| n.mask_mode() == *mode),
            Operation::SetFillRule { id, rule } => node(id).is_some_and(|n| n.fill_rule() == *rule),

            // `takes_paint` is the gate `apply` applies, spelled here so an
            // unpaintable subject reaches its `WrongKindForOp` rather than being
            // called a no-op because both lists happen to be empty.
            Operation::SetFills { id, fills } => {
                node(id).is_some_and(|n| n.kind().takes_paint() && n.paint().fills == *fills)
            }
            Operation::SetStrokes { id, strokes } => {
                node(id).is_some_and(|n| n.kind().takes_paint() && n.paint().strokes == *strokes)
            }
            // The root is the one kind `op_set_effects` refuses; the rest carry
            // the stack whether they paint or not.
            Operation::SetEffects { id, effects } => node(id)
                .is_some_and(|n| !matches!(n.kind(), NodeKind::Root) && n.effects() == effects),
            Operation::SetExports { id, exports } => {
                node(id).is_some_and(|n| n.exports() == exports)
            }
            Operation::SetLayoutGrids { id, grids } => node(id).is_some_and(|n| n.grids() == grids),
            Operation::SetInsets { id, insets } => node(id).is_some_and(|n| n.insets() == insets),
            Operation::SetDisplay { id, display } => {
                node(id).is_some_and(|n| n.display() == display.as_ref())
            }
            Operation::SetLayoutItem { id, item } => node(id).is_some_and(|n| n.item() == item),
            Operation::SetComponent { id, component } => {
                node(id).is_some_and(|n| n.component() == *component)
            }
            Operation::SetLink { id, link } => node(id).is_some_and(|n| n.link() == *link),
            Operation::SetSwap { id, swap } => node(id).is_some_and(|n| n.swap() == *swap),
            Operation::SetVariantSet { id, set } => {
                node(id).is_some_and(|n| n.set() == set.as_ref())
            }
            Operation::SetVariant { id, values } => {
                node(id).is_some_and(|n| n.variant() == values.as_slice())
            }
            Operation::SetProperties { id, props } => {
                node(id).is_some_and(|n| n.props() == props.as_slice())
            }

            Operation::SetPageGround { id, ground } => {
                doc.page(*id).is_some_and(|p| p.ground == *ground)
            }
            Operation::SetGuidePosition { id, position } => {
                doc.guide(*id).is_some_and(|g| g.position == *position)
            }
            Operation::SetGuideColor { id, color } => {
                doc.guide(*id).is_some_and(|g| g.color == *color)
            }
            Operation::SetGuideScope {
                id,
                owner,
                position,
            } => doc
                .guide(*id)
                .is_some_and(|g| g.owner == *owner && g.position == *position),
        }
    }
}

/// Where `id` sits in its parent's child list, or `None` when it has no parent
/// (the root) or is not in the document.
///
/// The number [`Operation::Reorder`] is compared against; the reparent arm asks
/// the same question of the same list.
fn child_index(doc: &crate::Document, id: NodeId) -> Option<usize> {
    let parent = doc.get(id)?.parent()?;
    doc.get(parent)?.children().iter().position(|c| *c == id)
}

impl Transaction {
    /// The nodes this transaction's operations name, in order, each once: what
    /// it is *about* — the layer recoloured, moved, created or deleted, the page
    /// added or regrounded — rather than every node it dirties (a moved layer's
    /// old and new parents). Table-only operations name none.
    pub fn subjects(&self) -> Vec<NodeId> {
        let mut out: Vec<NodeId> = Vec::new();
        for op in &self.0 {
            let named = match op {
                Operation::CreateNode { id, .. }
                | Operation::DeleteNode { id }
                | Operation::Reparent { id, .. }
                | Operation::Reorder { id, .. }
                | Operation::SetPageGround { id, .. }
                | Operation::SetPageDraft { id, .. }
                | Operation::AddPage { id, .. }
                | Operation::RemovePage { id }
                | Operation::MovePage { id, .. } => Some(*id),
                // The subtree's root: the one node whose parent is not in it.
                Operation::InsertSubtree { nodes, .. } => nodes
                    .iter()
                    .find(|n| {
                        n.parent()
                            .is_none_or(|p| !nodes.iter().any(|m| m.id() == p))
                    })
                    .map(Node::id),
                other => other.overwrites(),
            };
            if let Some(id) = named.filter(|id| !out.contains(id)) {
                out.push(id);
            }
        }
        out
    }

    /// Whether any operation in this changes what is drawn
    /// ([`Operation::changes_ink`]).
    ///
    /// **Any, not all**: a transaction that moves and renames a layer is visible.
    pub fn changes_ink(&self) -> bool {
        self.0.iter().any(Operation::changes_ink)
    }

    /// Whether applying this would leave `doc` exactly as it stands — a
    /// completed interaction that turned out to change nothing (§15 D428).
    ///
    /// **All, not any, and nothing is ever dropped *from* a transaction.** *Hide*
    /// over a mixed selection emits a `SetVisible` per layer; the already-hidden
    /// ones are individually no-ops, but dropping them would leave the undo step
    /// describing fewer layers than were selected.
    ///
    /// **Sound because a no-op leaves the document it was measured against**:
    /// testing each operation against `doc` rather than its predecessors' result
    /// is the same thing when every earlier operation changes nothing.
    ///
    /// **Empty answers `false`**: an empty transaction is dropped at the commit
    /// seam, so "changes nothing" is only said about work that was attempted.
    pub fn changes_nothing(&self, doc: &crate::Document) -> bool {
        !self.0.is_empty() && self.0.iter().all(|op| op.changes_nothing(doc))
    }
}

/// Geometry edit targeting the kind-specific payload of a node (§5.6).
#[derive(Clone, Debug, PartialEq)]
pub enum GeometryPatch {
    Size(Size),
    /// The same radius on all four corners.
    CornerRadius(f64),
    /// Per-corner radii. The inverse of *either* corner patch is always this one,
    /// so a uniform set can be undone to four unequal values.
    CornerRadii(RoundedRectRadii),
    /// Vertex count: a polygon's sides, or a star's points.
    Sides(u32),
    /// A star's inner radius as a fraction of its outer one, 0..=1.
    InnerRatio(f64),
    LineEnd(Point),
    /// A path's outline **and** its per-anchor radii, together.
    ///
    /// **One patch, not two**: the radii are indexed by anchor
    /// (`NodeKind::Path::corner_radii`), so replacing the outline alone would put
    /// every radius after an inserted or deleted anchor on the wrong corner.
    Path {
        path: BezPath,
        corner_radii: Vec<f64>,
    },
    TextSizing(crate::node::TextSizing),
    /// The rail a `Text` node is set along, or `None` to take it off
    /// (`NodeKind::Text::on_path`, §15 D405).
    ///
    /// **A geometry patch and not a style one**, like `TextSizing`: it changes the
    /// node's *box* (the bounds are the bent line box), where the typographic
    /// scopes change how characters draw inside one.
    TextPath(Option<BezPath>),
    /// Which way round a `Text` node runs along its rail
    /// (`NodeKind::Text::on_path_flip`, §15 D406).
    ///
    /// **Its own patch rather than a second field on [`Self::TextPath`]**: the
    /// flip outlives the rail, so a patch carrying both would make *Detach from
    /// path* (`TextPath(None)`) also reset the flip.
    TextPathFlip(bool),
    /// Where along its rail a `Text` node's type starts, as a fraction of the
    /// rail's length (`NodeKind::Text::on_path_offset`, §15 D409).
    ///
    /// **Its own patch beside the flip, for the flip's reason**: the offset
    /// outlives the rail.
    TextPathOffset(f64),
    /// Which operation a `Boolean` container performs. A geometry patch, so a
    /// dropdown can preview it on hover through the preview→commit valve.
    BoolOp(crate::node::BoolOp),
}

impl GeometryPatch {
    /// Whether this patch can change the layer's **box** — its size, or where its
    /// outline reaches — rather than only its shape inside the box.
    ///
    /// **The question insets ask** (§15 D874): a pinned layer stretched by its
    /// insets draws at a size the document does not hold, so a resize is
    /// re-pinned around the new size, while a corner radius, side count or inner
    /// ratio is applied to the *stored* size and must not be read as a resize.
    ///
    /// No wildcard, so a new patch has to be classified.
    pub fn resizes(&self) -> bool {
        match self {
            GeometryPatch::Size(_)
            | GeometryPatch::LineEnd(_)
            | GeometryPatch::Path { .. }
            | GeometryPatch::TextSizing(_)
            // A rail changes the box a text node is measured in, and so do the
            // flip and the offset.
            | GeometryPatch::TextPath(_)
            | GeometryPatch::TextPathFlip(_)
            | GeometryPatch::TextPathOffset(_)
            // A boolean's box is its result's, which the operator decides.
            | GeometryPatch::BoolOp(_) => true,
            GeometryPatch::CornerRadius(_)
            | GeometryPatch::CornerRadii(_)
            | GeometryPatch::Sides(_)
            | GeometryPatch::InnerRatio(_) => false,
        }
    }

    /// A copy of `kind` with this patch applied, or `None` if the patch does
    /// not match the kind.
    ///
    /// The non-mutating twin of what `apply` does, chiefly for render previews,
    /// which must not touch the `Document` (invariant 5).
    pub fn applied_to(&self, kind: &NodeKind) -> Option<NodeKind> {
        let mut copy = kind.clone();
        crate::document::apply_geometry_patch(&mut copy, self)
            .ok()
            .map(|_| copy)
    }

    /// Whether every number this patch carries is finite.
    ///
    /// **Matched exhaustively on purpose**: a non-finite number written into the
    /// document makes the file unopenable. See [`OpError::NonFinite`].
    pub fn is_finite(&self) -> bool {
        let size = |s: &kurbo::Size| s.width.is_finite() && s.height.is_finite();
        match self {
            Self::Size(s) => size(s),
            Self::CornerRadius(v) | Self::InnerRatio(v) | Self::TextPathOffset(v) => v.is_finite(),
            Self::CornerRadii(r) => [r.top_left, r.top_right, r.bottom_right, r.bottom_left]
                .iter()
                .all(|v| v.is_finite()),
            Self::LineEnd(p) => p.x.is_finite() && p.y.is_finite(),
            Self::Path { path, corner_radii } => {
                crate::node::path_is_finite(path) && corner_radii.iter().all(|v| v.is_finite())
            }
            Self::TextSizing(s) => match s {
                crate::node::TextSizing::Auto => true,
                crate::node::TextSizing::AutoHeight(w) => w.is_finite(),
                crate::node::TextSizing::Fixed(s) => size(s),
            },
            Self::TextPath(p) => p.as_ref().is_none_or(crate::node::path_is_finite),
            // No number in them at all.
            Self::Sides(_) | Self::TextPathFlip(_) | Self::BoolOp(_) => true,
        }
    }
}

/// Applied atomically; one entry in history.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transaction(pub Vec<Operation>);

/// Nodes whose world bounds / appearance changed (including descendants).
#[derive(Clone, Debug, Default)]
pub struct DirtySet(pub FxHashSet<NodeId>);

/// Result of a successful `apply`.
#[derive(Debug)]
pub struct ApplyOutcome {
    /// Apply this to undo.
    pub inverse: Transaction,
    pub dirty: DirtySet,
}

#[derive(thiserror::Error, Debug)]
pub enum OpError {
    #[error("no such node: {0:?}")]
    NoSuchNode(NodeId),
    #[error("duplicate id: {0:?}")]
    DuplicateId(NodeId),
    #[error("invalid parent")]
    InvalidParent,
    #[error("operation would create a cycle")]
    WouldCycle,
    #[error("index out of range")]
    IndexOutOfRange,
    #[error("opacity out of range (expected 0.0..=1.0)")]
    BadOpacity,
    /// An export spec carrying a value that cannot be a size or a quality
    /// (§15 D492).
    ///
    /// `ExportScale::from_label` and the CLI parse in `main.rs` refuse the same
    /// values; without this a `Width(0)` would produce a one-pixel file.
    #[error("an export spec's scale or quality is not a usable value")]
    BadExportSpec,
    /// A blur radius past [`crate::effect::MAX_BLUR`].
    #[error("a blur radius is past {}", crate::effect::MAX_BLUR)]
    BlurTooLarge,
    #[error("operation not valid for this node kind")]
    WrongKindForOp,
    /// A frame asked to sit somewhere only a shape may go. Frames nest inside
    /// frames and groups (§5.3, §15 D870) but not off a shape or inside a boolean
    /// or a mask (§15 D876). Also the answer to turning a group that holds a
    /// frame into a mask.
    #[error("a frame cannot sit inside a boolean, a mask or a shape")]
    ArtboardPlacement,
    #[error("malformed subtree")]
    MalformedSubtree,
    /// The subtree would sit deeper than [`crate::io::MAX_TREE_DEPTH`] once it
    /// is attached (§15 D832).
    ///
    /// **Measured against the *parent's* depth, not the subtree's own**, as the
    /// loader's bound is absolute. Distinct from [`Self::MalformedSubtree`]
    /// because the subtree is well formed; the destination makes it unusable.
    #[error(
        "the subtree would nest deeper than {} once attached, and a document \
         that deep cannot be reloaded",
        crate::io::MAX_TREE_DEPTH
    )]
    TooDeep,
    /// The operation had nothing to make a shape out of — a boolean whose operands
    /// cancel out entirely, asked to flatten. Distinct from [`Self::WrongKindForOp`]:
    /// the kind was right and the user can fix it by moving a shape.
    #[error("there is no geometry here to flatten")]
    EmptyGeometry,
    /// The boolean arithmetic was **abandoned** — [`crate::boolean::evaluate`]'s
    /// `catch_unwind` guard caught an unwind out of flo_curves (§15 D239) — rather
    /// than having answered honestly with nothing.
    ///
    /// 🚨 **Distinct from [`Self::EmptyGeometry`]**: moving a shape does not help
    /// an unwind. Both arrive at the same `ok_or` as `None`; only the counter
    /// ([`crate::boolean::failures`]) tells them apart.
    ///
    /// ⚠️ **The builders are outside §15 D298's bracket.** That reads the counter
    /// across `Resolved::update`, after a transaction exists; a builder calls
    /// `evaluate` while constructing it, so a failure there refuses the edit first
    /// (§15 D736).
    #[error("a boolean could not be computed, so there is nothing to flatten")]
    BooleanAbandoned,
    #[error("a page or the vault cannot be deleted, reparented, or reordered as a node")]
    CannotModifyRoot,
    #[error("no such page: {0:?}")]
    NoSuchPage(NodeId),
    /// A page removed while it holds layers or guides, or the last one.
    #[error("page {0:?} cannot be removed: it is the last page or is not empty")]
    PageNotRemovable(NodeId),
    #[error("no such guide: {0:?}")]
    NoSuchGuide(GuideId),
    #[error("no such variable: {0:?}")]
    NoSuchVariable(crate::variable::VarId),
    #[error("duplicate variable id: {0:?}")]
    DuplicateVariable(crate::variable::VarId),
    #[error("no such mode: {0:?}")]
    NoSuchMode(crate::variable::ModeId),
    #[error("duplicate mode id: {0:?}")]
    DuplicateMode(crate::variable::ModeId),
    /// A variables rule broken (§5.3f, `crate::variable::check`) — a binding to
    /// nothing, a column missing, an alias loop. Checked after the last op, as
    /// [`Self::BadLink`] is.
    #[error("{0}")]
    Variables(crate::variable::VarRule),
    /// A style rule broken (§15 D1062) — a binding to a style that is not there,
    /// a name used twice in a slot. Checked after the last op.
    #[error("{0}")]
    Styles(crate::style::StyleRule),
    #[error("no such style: {0:?}")]
    NoSuchStyle(crate::style::StyleId),
    #[error("duplicate style id: {0:?}")]
    DuplicateStyle(crate::style::StyleId),
    #[error("duplicate guide id: {0:?}")]
    DuplicateGuide(GuideId),
    #[error("no such image: {0}")]
    NoSuchImage(ImageId),
    /// The table already carries this id. **Not placing the same picture twice**:
    /// a caller asks `Document::has_image` first and adds a second *reference*.
    /// Reaching here means two different entries claimed one id.
    #[error("duplicate image id: {0}")]
    DuplicateImage(ImageId),
    /// A guide asked to be scoped to something that is not a live frame — a node
    /// that is not in the document, or one with no box for a guide to be clipped
    /// to.
    ///
    /// Validated because a guide's `position` is read in its owner's space; it
    /// also makes the ordering inside a delete transaction matter — see
    /// [`crate::build::guides_of`].
    #[error("a guide may only be scoped to a live frame or a page, not {0:?}")]
    BadGuideOwner(NodeId),
    /// One of a node's item lists repeats an item id (§15 D980); item ids match an
    /// instance's items to its main's. Checked after the last op.
    #[error("node {0:?} has item id {1:?} twice in one list")]
    DuplicateItemId(NodeId, crate::item::ItemId),
    /// A component or instance rule broken (§15 D978, `crate::component::check`) —
    /// a link to nothing, a main inside an instance, an instance of a main inside
    /// itself. Checked after the last op, so a transaction is judged by where it
    /// ends.
    #[error("node {0:?} {1}")]
    BadLink(NodeId, crate::component::LinkRule),
    /// A vault rule broken (§15 D1012, `crate::kit::check`) — an origin outside
    /// the vault or a vault node without one, a vault child that is not a main or
    /// a set — or a vault opened twice or closed while it held something.
    #[error("node {0:?} {1}")]
    Vault(NodeId, crate::kit::VaultRule),
    /// Any number the document will write that is `NaN` or infinite — in
    /// geometry, a transform, a **pivot** (§15 D639), an **effect** (§15 D641), a
    /// guide's position (§15 D421), or **a paint**: gradient stops, ramp and
    /// transform, colour channels, an image brush's alpha and the
    /// [`crate::ImageRef`] behind it (§15 D452), and a stroke's width and dashes
    /// (§15 D451).
    ///
    /// ⚠️ **Refused rather than repaired**: `serde_json` writes a non-finite `f64`
    /// as `null` and the reader has no arm for it, so such a value would survive
    /// `save` (and autosave) and kill `load` permanently. Invariant 8 puts the
    /// check at the operation, the last place the value can be rejected while the
    /// user still has their work.
    ///
    /// **Not clamped**: no defensible number exists to clamp a coordinate to.
    #[error("a number is not finite")]
    NonFinite,
    /// A layout value CSS itself refuses — a negative track size, `minmax()`
    /// with an `fr` minimum, `repeat(0, …)`, a grid line 0 or `span 0` (§15
    /// D914). Refused at the operation so no control can write one; a file that
    /// carries one anyway is read around it, not refused
    /// (`crate::container::Grid::is_valid`).
    #[error("a layout value CSS does not accept")]
    BadLayout,
}

#[cfg(test)]
mod changes_ink_tests {
    use super::*;
    use crate::id::IdSource;

    /// Pins `changes_ink`'s whole `false` arm (§15 D128) as a list, so a new
    /// operation's arm choice is checked; recount the fixture when one is added.
    #[test]
    fn only_the_chrome_operations_change_no_ink() {
        let mut ids = IdSource::new(1);
        let id = ids.mint();
        let guide = crate::guide::GuideId(ids.mint());
        let style = ids.mint_style();
        let invisible = [
            Operation::SetName {
                id,
                name: "x".into(),
            },
            Operation::SetLocked { id, locked: true },
            Operation::SetProportionsLocked { id, locked: true },
            Operation::SetPivot { id, pivot: None },
            Operation::SetExports {
                id,
                exports: vec![],
            },
            Operation::SetLayoutGrids { id, grids: vec![] },
            Operation::SetComponent {
                id,
                component: true,
            },
            Operation::SetLink { id, link: None },
            Operation::SetSwap { id, swap: None },
            Operation::SetVariantSet { id, set: None },
            Operation::SetVariant { id, values: vec![] },
            Operation::SetProperties { id, props: vec![] },
            Operation::AddGuide {
                guide: crate::guide::Guide {
                    id: guide,
                    axis: crate::guide::GuideAxis::Vertical,
                    position: 0.0,
                    color: None,
                    owner: id,
                },
            },
            Operation::RemoveGuide { id: guide },
            Operation::SetGuidePosition {
                id: guide,
                position: 1.0,
            },
            Operation::SetGuideColor {
                id: guide,
                color: None,
            },
            Operation::SetGuideScope {
                id: guide,
                owner: id,
                position: 1.0,
            },
            // The style table and bindings draw nothing; the lists
            // `style::settle` writes beside them are the ink (§15 D1062).
            Operation::AddStyle {
                style: crate::style::Style {
                    id: style,
                    name: "s".into(),
                    value: crate::style::StyleValue::Fills(vec![]),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::RemoveStyle { id: style },
            Operation::SetStyleName {
                id: style,
                name: "t".into(),
            },
            Operation::SetStyleValue {
                id: style,
                value: crate::style::StyleValue::Fills(vec![]),
            },
            Operation::SetFillStyle { id, style: None },
            Operation::SetStrokeStyle { id, style: None },
            Operation::SetEffectsStyle { id, style: None },
            Operation::SetTypeStyle { id, style: None },
            Operation::SetLayoutStyle { id, style: None },
            // A kit's style's version (§15 D1064): bookkeeping, as `SetOrigin` is.
            Operation::SetStyleOrigin {
                id: style,
                origin: None,
            },
            Operation::OpenVault { id },
            Operation::CloseVault { id },
            Operation::AddPage {
                id,
                index: 0,
                name: "p".into(),
                ground: peniko::Color::BLACK,
                draft: false,
                mode: None,
            },
            Operation::RemovePage { id },
            Operation::MovePage { id, index: 0 },
            Operation::SetPageDraft { id, draft: true },
            Operation::SetOrigin { id, origin: None },
            Operation::AddVariable {
                var: crate::variable::Variable {
                    id: crate::variable::VarId(id),
                    name: "v".into(),
                    kind: crate::variable::VarKind::Color,
                    cells: Default::default(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::RemoveVariable {
                id: crate::variable::VarId(id),
            },
            Operation::SetVariableName {
                id: crate::variable::VarId(id),
                name: "w".into(),
            },
            Operation::SetDescription {
                of: Described::Node(id),
                text: "d".into(),
            },
        ];
        assert_eq!(invisible.len(), 38, "the whole false arm");
        for op in &invisible {
            assert!(
                !op.changes_ink(),
                "{op:?} is chrome or bookkeeping and must not arm the chrome hide"
            );
        }

        // The ones that draw, including `SetVisible` and `SetPageGround`,
        // which are easy to file with the bookkeeping above.
        let visible = [
            Operation::SetVisible { id, visible: false },
            Operation::SetPageGround {
                id,
                ground: peniko::Color::BLACK,
            },
            Operation::SetOpacity { id, opacity: 0.5 },
            Operation::SetClip { id, clip: true },
            Operation::SetTransform {
                id,
                transform: kurbo::Affine::IDENTITY,
            },
            Operation::SetFills { id, fills: vec![] },
            Operation::SetStrokes {
                id,
                strokes: vec![],
            },
            // Not filed with `SetExports` above: both are whole-list edits, but
            // only this one draws.
            Operation::SetEffects {
                id,
                effects: vec![],
            },
            Operation::DeleteNode { id },
            Operation::Reorder { id, index: 0 },
            // A kit's variable and a kit's modes are columns bound layers read
            // (§15 D1064), as a cell is.
            Operation::SetVariable {
                var: crate::variable::Variable {
                    id: crate::variable::VarId(id),
                    name: "v".into(),
                    kind: crate::variable::VarKind::Color,
                    cells: Default::default(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::SetKitModes {
                kit: "0".repeat(32),
                modes: None,
            },
            // A document mode's name is the kit mapping's fallback match.
            Operation::SetModeName {
                id: crate::variable::ModeId(id),
                name: "m".into(),
            },
        ];
        for op in &visible {
            assert!(op.changes_ink(), "{op:?} changes what is drawn");
        }
    }

    /// A transaction is visible if any operation is: move plus rename is, a bare
    /// rename is not.
    #[test]
    fn a_transaction_is_visible_if_any_of_its_operations_is() {
        let id = IdSource::new(1).mint();
        let rename = Operation::SetName {
            id,
            name: "x".into(),
        };
        let move_it = Operation::SetTransform {
            id,
            transform: kurbo::Affine::translate((1.0, 0.0)),
        };
        assert!(!Transaction(vec![]).changes_ink(), "nothing is not an edit");
        assert!(!Transaction(vec![rename.clone()]).changes_ink());
        assert!(Transaction(vec![rename.clone(), move_it.clone()]).changes_ink());
        assert!(Transaction(vec![move_it, rename]).changes_ink());
    }
}

#[cfg(test)]
mod changes_nothing_tests {
    //! `Operation::changes_nothing` and its whole-transaction twin, the guard
    //! `EditorSession::commit_inner` asks before making an undo step.
    //!
    //! The flips that matter:
    //! - `any` in place of `all` on the transaction fails
    //!   `a_mixed_selection_still_commits_because_the_test_is_all_and_not_any` at
    //!   the transaction assertion.
    //! - Adjusting the `Reorder` index for the removal fails
    //!   `rows_dropped_where_they_already_sit_change_nothing` at the *negative*
    //!   assertion (a real swap called a no-op, an edit silently dropped).
    //! - Dropping the `takes_paint` gate from the `SetFills` arm fails
    //!   `an_operation_whose_subject_refuses_it_is_not_a_no_op`.

    use super::*;
    use crate::Document;
    use crate::id::IdSource;
    use crate::image::Brush;
    use crate::node::TextSizing;

    /// Root → frame → two rects, at child indices 0 and 1.
    fn fixture() -> (Document, NodeId, NodeId, [NodeId; 2]) {
        let mut ids = IdSource::new(0x5EED);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let frame = ids.mint();
        let a = ids.mint();
        let b = ids.mint();
        let mut tx = vec![Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(200.0, 200.0),
            },
            transform: None,
            name: None,
        }];
        for (i, id) in [a, b].into_iter().enumerate() {
            tx.push(Operation::CreateNode {
                id,
                parent: frame,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::from_single_radius(0.0),
                },
                transform: None,
                name: None,
            });
        }
        doc.apply(&Transaction(tx)).expect("seed");
        (doc, root, frame, [a, b])
    }

    /// Writing back the stored value is a no-op and any other value is an edit.
    /// Each pair is `(the value already there, something else)` of the same field;
    /// the spread is one entry per *shape* of arm, not per variant.
    #[test]
    fn writing_back_the_stored_value_changes_nothing_and_any_other_value_does() {
        let (doc, _root, frame, [a, _b]) = fixture();
        let pairs: Vec<(Operation, Operation)> = vec![
            (
                Operation::SetTransform {
                    id: a,
                    transform: Affine::IDENTITY,
                },
                Operation::SetTransform {
                    id: a,
                    transform: Affine::translate((1.0, 0.0)),
                },
            ),
            (
                Operation::SetGeometry {
                    id: a,
                    geometry: GeometryPatch::Size(Size::new(10.0, 10.0)),
                },
                Operation::SetGeometry {
                    id: a,
                    geometry: GeometryPatch::Size(Size::new(10.0, 10.5)),
                },
            ),
            (
                Operation::SetName {
                    id: a,
                    name: doc.get(a).expect("seeded").name().to_string(),
                },
                Operation::SetName {
                    id: a,
                    name: "something else".into(),
                },
            ),
            (
                Operation::SetVisible {
                    id: a,
                    visible: true,
                },
                Operation::SetVisible {
                    id: a,
                    visible: false,
                },
            ),
            (
                Operation::SetLocked {
                    id: a,
                    locked: false,
                },
                Operation::SetLocked {
                    id: a,
                    locked: true,
                },
            ),
            (
                Operation::SetProportionsLocked {
                    id: a,
                    locked: false,
                },
                Operation::SetProportionsLocked {
                    id: a,
                    locked: true,
                },
            ),
            (
                Operation::SetOpacity {
                    id: a,
                    opacity: 1.0,
                },
                Operation::SetOpacity {
                    id: a,
                    opacity: 0.999,
                },
            ),
            (
                Operation::SetPivot { id: a, pivot: None },
                Operation::SetPivot {
                    id: a,
                    pivot: Some(crate::node::Pivot::Normalized(kurbo::Vec2::new(0.25, 0.25))),
                },
            ),
            (
                Operation::SetClip { id: a, clip: false },
                Operation::SetClip { id: a, clip: true },
            ),
            (
                Operation::SetMask { id: a, mask: false },
                Operation::SetMask { id: a, mask: true },
            ),
            (
                Operation::SetFillRule {
                    id: a,
                    rule: FillRule::default(),
                },
                Operation::SetFillRule {
                    id: a,
                    rule: match FillRule::default() {
                        FillRule::NonZero => FillRule::EvenOdd,
                        FillRule::EvenOdd => FillRule::NonZero,
                    },
                },
            ),
            (
                Operation::SetFills {
                    id: a,
                    fills: vec![],
                },
                Operation::SetFills {
                    id: a,
                    fills: crate::keyed_by_position([Fill {
                        brush: Brush::Solid(peniko::Color::BLACK).into(),
                        visible: true,
                    }]),
                },
            ),
            (
                Operation::SetEffects {
                    id: a,
                    effects: vec![],
                },
                Operation::SetEffects {
                    id: a,
                    effects: crate::keyed_by_position([Effect::new(
                        crate::effect::EffectKind::LayerBlur { radius: 2.0 },
                    )]),
                },
            ),
            (
                Operation::SetExports {
                    id: a,
                    exports: vec![],
                },
                Operation::SetExports {
                    id: a,
                    exports: crate::keyed_by_position([ExportSpec::new(
                        crate::export::ExportFormat::Png,
                        crate::export::ExportScale::Times(1.0),
                    )]),
                },
            ),
            (
                Operation::SetLayoutGrids {
                    id: frame,
                    grids: vec![],
                },
                Operation::SetLayoutGrids {
                    id: frame,
                    grids: crate::keyed_by_position([crate::layout::LayoutGrid::new(
                        crate::layout::GridAxis::Columns,
                    )]),
                },
            ),
            (
                Operation::SetPageGround {
                    id: doc.first_page(),
                    ground: doc.ground(doc.first_page()),
                },
                Operation::SetPageGround {
                    id: doc.first_page(),
                    ground: peniko::Color::from_rgb8(1, 2, 3),
                },
            ),
            (
                Operation::MovePage {
                    id: doc.first_page(),
                    index: 0,
                },
                Operation::MovePage {
                    id: doc.first_page(),
                    index: 1,
                },
            ),
            (
                Operation::SetPageDraft {
                    id: doc.first_page(),
                    draft: false,
                },
                Operation::SetPageDraft {
                    id: doc.first_page(),
                    draft: true,
                },
            ),
        ];
        assert_eq!(pairs.len(), 18, "one pair per shape of arm");
        for (still, moved) in &pairs {
            assert!(
                still.changes_nothing(&doc),
                "writing back the stored value should change nothing: {still:?}"
            );
            assert!(
                !moved.changes_nothing(&doc),
                "writing a different value is an edit: {moved:?}"
            );
        }
    }

    /// A multi-row drop that lands rows where they already are emits net-zero
    /// `Reorder`s, and the index is compared as it stands, with no adjustment for
    /// the removal. ⚠️ The negative assertion is the one that catches an
    /// over-corrected index.
    #[test]
    fn rows_dropped_where_they_already_sit_change_nothing() {
        let (doc, _root, _frame, [a, b]) = fixture();
        let still = Transaction(vec![
            Operation::Reorder { id: a, index: 0 },
            Operation::Reorder { id: b, index: 1 },
        ]);
        assert!(
            still.changes_nothing(&doc),
            "two rows dropped on their own positions are one undo step for nothing"
        );
        assert!(
            !Transaction(vec![Operation::Reorder { id: a, index: 1 }]).changes_nothing(&doc),
            "moving the top row under the bottom one swaps them"
        );
        // The same drop spelled as a reparent into its own parent is a no-op too;
        // one index along is not.
        assert!(
            Transaction(vec![Operation::Reparent {
                id: b,
                new_parent: _frame,
                index: 1,
            }])
            .changes_nothing(&doc)
        );
        assert!(
            !Transaction(vec![Operation::Reparent {
                id: b,
                new_parent: _frame,
                index: 0,
            }])
            .changes_nothing(&doc)
        );
    }

    /// *Hide* over a mixed selection: the already-hidden layers are individually
    /// no-ops but the transaction hides something, so the test is `all`. The loss
    /// is asserted first, the mechanism second.
    #[test]
    fn a_mixed_selection_still_commits_because_the_test_is_all_and_not_any() {
        let (mut doc, _root, _frame, [a, b]) = fixture();
        doc.apply(&Transaction(vec![Operation::SetVisible {
            id: a,
            visible: false,
        }]))
        .expect("hide one of them");
        let hide_both = Transaction(vec![
            Operation::SetVisible {
                id: a,
                visible: false,
            },
            Operation::SetVisible {
                id: b,
                visible: false,
            },
        ]);
        assert!(
            !hide_both.changes_nothing(&doc),
            "hiding a mixed selection hides something and must commit"
        );
        assert!(
            hide_both.0[0].changes_nothing(&doc),
            "and it must do so with a member that is individually a no-op in it"
        );
        // An empty transaction is dropped at the commit seam, so it answers `false`.
        assert!(!Transaction(vec![]).changes_nothing(&doc));
    }

    /// An operation `apply` would refuse is not a no-op: a missing subject, or a
    /// kind that refuses it. The root's empty fills equal the ones written, so
    /// only the `takes_paint` gate tells them apart.
    #[test]
    fn an_operation_whose_subject_refuses_it_is_not_a_no_op() {
        let (doc, root, _frame, _rects) = fixture();
        let ghost = IdSource::new(0xDEAD).mint();
        assert!(
            !Operation::SetName {
                id: ghost,
                name: "n".into(),
            }
            .changes_nothing(&doc)
        );
        assert!(
            !Operation::SetFills {
                id: root,
                fills: vec![],
            }
            .changes_nothing(&doc),
            "the root is unpaintable, so this reaches WrongKindForOp rather than being dropped"
        );
        assert!(
            !Operation::SetEffects {
                id: root,
                effects: vec![],
            }
            .changes_nothing(&doc),
            "the root is the one kind op_set_effects refuses"
        );
        // A non-finite value equals nothing, so it still reaches
        // `OpError::NonFinite`.
        assert!(
            !Operation::SetGuidePosition {
                id: crate::guide::GuideId(ghost),
                position: f64::NAN,
            }
            .changes_nothing(&doc)
        );
    }

    /// The span-carrying arms compare what `apply` would store: a `SetTextStyle`
    /// with `spans: None` is a no-op when the stored list is already a fixed point
    /// of the re-statement.
    #[test]
    fn the_text_arms_compare_the_clamped_list_and_not_the_one_handed_over() {
        let mut ids = IdSource::new(0x7E47);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let t = ids.mint();
        let style = crate::node::TextStyle::default();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: t,
            parent: root,
            index: 0,
            kind: NodeKind::Text {
                content: "abc".into(),
                style: Box::new(style.clone()),
                spans: crate::node::CharSpans::default(),
                para_spans: crate::node::ParaSpans::default(),
                paragraph: crate::node::ParagraphStyle::default(),
                block: crate::node::BlockStyle::default(),
                sizing: TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("seed the text node");

        assert!(
            Operation::SetText {
                id: t,
                content: "abc".into(),
                spans: crate::node::CharSpans::default(),
                para_spans: crate::node::ParaSpans::default(),
            }
            .changes_nothing(&doc)
        );
        assert!(
            !Operation::SetText {
                id: t,
                content: "abd".into(),
                spans: crate::node::CharSpans::default(),
                para_spans: crate::node::ParaSpans::default(),
            }
            .changes_nothing(&doc)
        );
        // Same defaults and an empty stored list: the re-statement is the identity.
        assert!(
            Operation::SetTextStyle {
                id: t,
                style: style.clone(),
                spans: None,
            }
            .changes_nothing(&doc)
        );
        let mut bigger = style.clone();
        bigger.font_size += 1.0;
        assert!(
            !Operation::SetTextStyle {
                id: t,
                style: bigger,
                spans: None,
            }
            .changes_nothing(&doc)
        );
        assert!(
            Operation::SetParagraphStyle {
                id: t,
                paragraph: crate::node::ParagraphStyle::default(),
                spans: None,
            }
            .changes_nothing(&doc)
        );
        assert!(
            Operation::SetBlockStyle {
                id: t,
                block: crate::node::BlockStyle::default(),
            }
            .changes_nothing(&doc)
        );
        // The root has no block style to compare, so the arm answers `false`.
        assert!(
            !Operation::SetBlockStyle {
                id: root,
                block: crate::node::BlockStyle::default(),
            }
            .changes_nothing(&doc)
        );
    }
}
