//! The context-menu registry (`docs/context-menus.md`).
//!
//! One list, not a list per menu: a row's label, glyph, chord, group and verb live
//! here once, and every menu is a filtered projection of the canonical order in
//! [`Group`] (`docs/context-menus.md` §3), so the canvas and layers-panel menus
//! show identical rows in identical order. Nothing here may know it is drawn as a
//! menu; the command palette and cheatsheet read the same shape.
//!
//! Kind decides presence; state decides enabled. A row whose only purpose is to
//! undo a non-default state (*Reset crop*, *Reset origin*) is omitted when the
//! state is default rather than shown dim.
//!
//! Opening, closing and placing the menu is `app.rs`'s `context_menu_ui`; painting
//! a row is `ui::menu_row`. This module answers which rows apply to a target, in
//! what state.
//!
//! Both *Export* rows were withdrawn (a decided non-goal) and the ruler's unit menu
//! was struck. A frame's *Background…* row (§5.1) is absent: the background is now
//! its fill list, and whether the row returns against it is open for the
//! maintainer.

use crate::theme::icon;
use ondin_core::{BoolOp, GuideId, NodeId, NodeKind, build};

use crate::input::ViewSwitch;

/// Which door the menu was opened through (`docs/context-menus.md` §1).
///
/// Both doors show the same rows; the door only decides whether *Paste* means
/// "here, at the pointer" or "as a sibling above this row".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Door {
    Canvas,
    Panel,
}

/// What the menu was opened on (`docs/context-menus.md` §2 and §6).
///
/// Decided by what is under the pointer, never by what is selected (C3): a
/// right-click on empty canvas opens the canvas menu even with five layers selected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    /// Empty canvas (§6.1).
    Canvas,
    /// A layer, through either door. `id` is what a left-click there would have
    /// selected (C1). Most rows act on the selection, not this id; `id` is read
    /// by rows needing a single layer (*Make this the key layer*, the panel's *Paste*).
    Layer { id: NodeId, door: Door },
    /// The layers panel's background, below the last row (§6.2).
    PanelBackground,
    /// A guide, which out-ranks the layer under it while guides are unlocked
    /// (C7, §6.3).
    Guide(GuideId),
    /// Inside a live text session (§6.4).
    TextSession,
    /// A point selection in the node tool (§6.5).
    Points,
}

/// §3's canonical order, which is the order a menu is built in.
///
/// The projection cannot reorder, only drop; empty groups collapse with their
/// separator. [`Self::View`] is not one of §3's nine: §3 is a layer's menu and
/// those switches are the page's.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Group {
    /// The kind's own "one level in" verbs — *Edit text*, *Enter group*, *Edit image*.
    Head,
    Clipboard,
    /// *Copy properties* · *Paste properties*: a second clipboard carrying an
    /// appearance rather than a layer.
    Properties,
    Structure,
    Order,
    Transform,
    State,
    Navigate,
    /// *Copy as SVG* · *Copy as CSS* · *Copy as PNG* · *Copy as PNG 2×*, semantic
    /// then raster.
    Export,
    /// The workspace switches §6.1 and §6.3 carry (rulers, guides, grid). Last.
    View,
    // The app verbs' groups, which no context menu emits.
    History,
    Tools,
    File,
    Kits,
    Text,
    Panels,
}

impl Group {
    /// The canonical order, and the order [`fn@build`] emits in.
    const ORDER: [Group; 16] = [
        Group::Head,
        Group::Clipboard,
        Group::Properties,
        Group::Structure,
        Group::Order,
        Group::Transform,
        Group::State,
        Group::Navigate,
        Group::Export,
        Group::View,
        Group::History,
        Group::Tools,
        Group::File,
        Group::Kits,
        Group::Text,
        Group::Panels,
    ];

    /// The name the palette shows beside a row, so two similar labels can be told
    /// apart by where they come from.
    pub fn label(self) -> &'static str {
        match self {
            Group::Head | Group::State => "Layer",
            Group::Clipboard => "Clipboard",
            Group::Properties => "Properties",
            Group::Structure => "Structure",
            Group::Order => "Order",
            Group::Transform => "Transform",
            Group::Navigate | Group::View => "View",
            Group::Export => "Export",
            Group::History => "Edit",
            Group::Tools => "Tools",
            Group::File => "File",
            Group::Kits => "Components",
            Group::Text => "Text",
            Group::Panels => "Panels",
        }
    }
}

/// One verb the registry can dispatch.
///
/// Most are an [`crate::input::Action`] under another name, handed straight to
/// `dispatch` by [`crate::app::OndinApp::perform_menu_item`] so a row and its chord
/// cannot diverge. The rest depend on where the menu was opened (`PasteHere`,
/// `MakeKeyLayer`) or on a target the keymap cannot name (`DeleteGuide`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Item {
    // --- head -------------------------------------------------------------
    /// §5.2 — `entered_group` plus one step of the group chain, as a double-click does.
    EnterGroup,
    /// §5.3 — the operands stay selectable and editable.
    EnterBoolean,
    /// §5.3 — the base operand is bottom of the list, so this reorders to index 0.
    MakeBaseOperand,
    /// §5.8 — the layer align aligns to, the base operand of a boolean, and which
    /// member of a selection becomes the mask; the keyed layer wears
    /// `canvas::draw_key_outline`.
    MakeKeyLayer,
    /// §5.1 — a frame's signature property, the only kind where `Node::clip` means anything.
    ClipContent,
    /// §5.6 — `begin_edit_text`.
    EditText,
    /// §5.6's three checkable rows over `TextSizing`, carrying the panel's cell index.
    ///
    /// A deliberate duplicate of the Type panel's segmented control; both call
    /// `set_text_sizing`. The labels are this menu's own, not `TextSizing::LABELS`,
    /// whose abbreviations exist only because a segmented control has no room.
    TextSizing(u8),
    /// §5.4 — the node tool.
    EditPoints,
    /// §5.7 — `Tool::ImageEdit`. Named for the mode and carries no ellipsis, since
    /// it opens no dialog, like *Edit text* and *Edit points*.
    EditImage,
    /// §5.7 — the picture's own pixels, *turned* for a turned picture.
    OriginalSize,

    // --- clipboard --------------------------------------------------------
    Cut,
    Copy,
    /// Canvas: at the pointer. Panel: as a sibling above the clicked row.
    PasteHere,
    /// Canvas only (§6.1): the clipboard back at the coordinates it was copied from.
    ///
    /// Not a door on [`Self::PasteHere`]: both are offered on empty canvas at once.
    PasteInPlace,
    /// Each selected layer gives way to a copy of the whole clipboard, centred where it
    /// was, in its slot (`OndinApp::paste_to_replace`).
    PasteToReplace,
    Duplicate,
    Delete,

    // --- properties (§3's third group) ------------------------------------
    /// Lift the key layer's appearance — fills, strokes, effects, opacity — onto a
    /// clipboard of its own (`Ctrl+Alt+C`, `build::properties_of`).
    ///
    /// Both chord and row go through [`crate::app::OndinApp::properties_source`],
    /// so the row cannot copy from a different layer than the key does.
    CopyProperties,
    /// Give the selection the appearance the clipboard is holding (`Ctrl+Alt+V`,
    /// `build::paste_properties`). Dimmed, not omitted, with nothing copied.
    PasteProperties,

    // --- structure --------------------------------------------------------
    Group,
    /// Wrap the selection in a new frame sized to its union (`build::frame`).
    FrameSelection,
    /// *Create component* (`Ctrl+Alt+K`) — on an ordinary layer or a selection; a
    /// frame or group becomes the main, anything else is framed first.
    CreateComponent,
    /// *Duplicate as component* — on a main: a copy that is a new main, where
    /// *Duplicate* makes an instance.
    DuplicateAsComponent,
    /// *Select all instances* — on a main, and on a set, every variant's.
    SelectAllInstances,
    /// *Go to main component* — on an instance, or a linked layer in one.
    GoToMain,
    /// *Detach instance* (`Ctrl+Alt+B`) — on an instance.
    DetachInstance,
    /// *Accept kit updates* (§5.3e) — on a kit instance whose kit has a newer publish.
    AcceptKitUpdates,
    /// *Make local…* (§5.3e) — on a kit instance: make its vendored main the
    /// document's own, after a confirmation.
    MakeLocal,
    /// *Reset all* — on an instance: its fields, removed children and order back to
    /// the main's, its own layers kept (`reset::reset_all`).
    ResetInstance,
    /// *Reset Label* — on a linked layer inside an instance: [`Self::ResetInstance`]
    /// scoped to that layer.
    ResetChild,
    /// *Restore children* — on an instance or linked layer whose children differ
    /// from its source's: its own children while expanded
    /// (`reset::restore_own_children`), its whole subtree while collapsed
    /// (`reset::restore_structure`).
    RestoreChildren,
    /// *Reset Label text* — on a linked layer whose property-driven field differs
    /// from its main.
    ResetProperty,
    /// *Combine as variants* — on several mains: wrap them in a set.
    CombineAsVariants,
    /// *Add variant* — on a set or a variant: a copy taking the first free combination.
    AddVariant,
    Ungroup,
    /// *Use as mask* — the layer clips the ones above it (`build::mask`).
    ///
    /// Checkable rather than two labels: the verb is a toggle and the booleans
    /// beside it already spend the check mark on "is it already that".
    Mask,
    /// Read this layer's own geometry even-odd instead of by winding.
    ///
    /// Offered only on a `Path`, the one authored kind whose shape can differ
    /// between the rules. Not on a boolean: `Node::fill_rule` derives it from the
    /// operation, so a toggle there would make the shape wrong.
    EvenOdd,
    /// Wrap the selection in a boolean, or switch a selected one's operation;
    /// `apply_boolean` decides which, so one item covers both.
    Boolean(BoolOp),
    Flatten,
    /// §4 / §5.4 / §5.5 — one shape becomes the `Path` its outline traces
    /// (`build::outline`).
    ///
    /// Not a second spelling of [`Self::Flatten`], which discards a boolean's or
    /// set's operands; `build::flatten` refuses a lone non-boolean, so the two rows
    /// are never both offered.
    OutlineShape,
    /// Each visible stroke becomes a filled `Path` (`build::outline_stroke`).
    OutlineStroke,
    /// Two path layers become one, their nearest open ends spliced
    /// (`OndinApp::join_layers`). The point menu's [`Self::Join`] fuses ends within one.
    JoinPaths,
    /// §5.6 — a text layer becomes the `Path` its glyphs trace (`build::outline_text`).
    ///
    /// A separate item from [`Self::OutlineShape`] because text's outline needs the
    /// resolved layout (`build::can_outline`); the two are mutually exclusive by
    /// kind. Labelled *Convert to path* because the string is gone afterwards.
    OutlineText,
    /// §5.6 — take a text layer off its rail, leaving the rail as a path layer.
    ///
    /// Offered on a single layer only (`one && cx.on_a_rail`). There is no
    /// *Text on path* row: setting type on a curve is the Text tool's own gesture.
    DetachTextPath,
    /// §5.6 — run the type along the other side of its rail.
    ///
    /// Checkable, like [`Self::ClipContent`]: which side is "the other" depends on
    /// how the curve was drawn, so two named rows could not both be true.
    FlipTextPath,

    // --- order / transform / state / navigate -----------------------------
    Restack(build::ZMove),
    Flip(build::Axis),
    /// `SetPivot { pivot: None }` — "not moved", a state the model can say and a
    /// resolved point cannot. Omitted unless the pivot has been moved (§3's
    /// exception); it undoes a whole selection in one click.
    ResetOrigin,
    ToggleHidden,
    ToggleLocked,
    Rename,
    ZoomSelection,

    // --- export (§3's ninth group) ----------------------------------------
    /// Put the selection on the system clipboard as an SVG document
    /// (`ondin_export::svg::svg_of`), so the other application receives editable
    /// groups and shapes.
    CopyAsSvg,
    /// Each selected layer's CSS as text, under a comment naming it — a container
    /// with its contents, as *Copy as PNG* (`ondin_export::code::css_of`).
    CopyAsCss,
    /// The selection as a picture on the clipboard at this multiplier, 1 or 2
    /// (`ondin_export::png::raster_of`).
    ///
    /// It does not encode a PNG: `arboard::ImageData` takes the raw RGBA that
    /// `raster_of` returns, and handing it `png_of`'s bytes pastes as garbage.
    CopyAsPng(u8),

    // --- canvas and panel background (§6.1, §6.2) -------------------------
    SelectAll,
    ZoomFit,
    /// Re-run every export setting in the document, into the folder the last
    /// export went to (§7). Asks nothing.
    ExportAll,
    /// Every top-level frame into a folder the user picks: a frame's own export
    /// settings, else a 1× PNG.
    ExportFrames,
    ViewSwitch(ViewSwitch),

    // --- guides (§6.3) ----------------------------------------------------
    DeleteGuide,
    /// A loop of `RemoveGuide` over everything the document holds.
    ClearGuides,

    // --- points (§6.5) ----------------------------------------------------
    /// Add an anchor to the path's outline where the menu was opened.
    ///
    /// Carries no accelerator: only a menu can supply which point on the ink. Dim
    /// rather than absent when the click missed the ink, since the target always
    /// has an outline this could apply to.
    AddPointHere,
    /// Delete the point selection, healing each subpath — or the selected segment,
    /// leaving the gap; the same dispatch.
    ///
    /// One row that renames itself to the verb the selection has armed, because
    /// `PointSet` holds anchors or segments, never both, so a pair would leave one
    /// row dim on every open.
    DeletePoints,
    /// *Make corner* / *Make smooth* over the point selection (`context-menus.md` §6.5).
    ///
    /// Two rows, unlike [`Self::DeletePoints`]: a selection can hold a corner and a
    /// smooth point at once, so both are live and mean something.
    SetSmooth(bool),
    ReverseSubpaths,
    /// Fuse the two selected open ends — close one run, or splice two into one
    /// (`tools::join_ends`).
    ///
    /// The subject is the point selection rather than the pointer; it shares
    /// `tools::join_runs` with the pen so both orient a splice the same way.
    Join,

    // --- app verbs: no context menu builds these ----------------------------
    // The command palette and the cheatsheet read them; each is an `Action`
    // ([`Self::action`]) so a row and its chord cannot diverge.
    Undo,
    Redo,
    Save,
    NewDocument,
    CloseDocument,
    /// `Action::Open`, the library.
    Library,
    Settings,
    ExportCode(ondin_export::code::Target),
    /// `Action::Present`: the top-level frames one at a time, fullscreen.
    Present,
    Tool(Tool),
    PlaceImage,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Align(build::Axis, build::Edge),
    Distribute(build::Axis),
    InsertPicker,
    VariablesBrowser,
    PublishKit,
    /// The Document menu's kit rows and its way back to the library.
    MakeKit,
    KitsUsed,
    StopSharingKit,
    ShowInLibrary,
    VersionHistory,
    /// A quarter turn of each selected layer about its own origin, as the Transform
    /// card's buttons do; `true` is clockwise.
    RotateQuarter(bool),
    /// *Select same fill* and its siblings (`OndinApp::select_same`).
    SelectSame(crate::input::Same),
    /// *Select inverse* (`OndinApp::select_inverse`).
    SelectInverse,
    /// One of the chords a live text session admits.
    TextChord(crate::input::TextChord),
    /// Scroll the inspector to the card with this title and open it.
    ShowCard(&'static str),
    Palette,
    Cheatsheet,
}

/// A layer's kind, as much of it as a menu needs.
///
/// Not `NodeKind`, which carries a `BezPath` and a text buffer: the menu is rebuilt
/// every frame it is open. `Copy`, so the selection's kinds are a plain slice.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Frame,
    Group,
    Boolean(BoolOp),
    Path,
    Text,
    /// Rect, ellipse, polygon, star, line — §5.5's "no head at all".
    Shape,
}

impl Kind {
    pub fn of(kind: &NodeKind) -> Self {
        match kind {
            NodeKind::Artboard { .. } => Kind::Frame,
            NodeKind::Group => Kind::Group,
            NodeKind::Boolean { op } => Kind::Boolean(*op),
            NodeKind::Path { .. } => Kind::Path,
            NodeKind::Text { .. } => Kind::Text,
            _ => Kind::Shape,
        }
    }
}

/// The five View switches a context menu can show, read once by the caller.
///
/// A struct rather than a closure over the app, so building a menu borrows nothing
/// and a test can build one from five bools.
#[derive(Clone, Copy, Default)]
pub struct ViewState {
    pub rulers: bool,
    pub guides: bool,
    pub guide_lock: bool,
    pub grid: bool,
    pub layout_grid: bool,
}

impl ViewState {
    /// This struct's answer for `sw`, or `None` where it has none.
    ///
    /// Not reachable for the `None` switches today (`view_rows` and `guide_menu`
    /// ask only the five held here); the match is spelled out so a new
    /// `ViewSwitch` is a compile error rather than a wrong tick.
    pub fn get(self, sw: ViewSwitch) -> Option<bool> {
        match sw {
            ViewSwitch::Rulers => Some(self.rulers),
            ViewSwitch::Guides => Some(self.guides),
            ViewSwitch::GuideLock => Some(self.guide_lock),
            ViewSwitch::Grid => Some(self.grid),
            ViewSwitch::LayoutGrid => Some(self.layout_grid),
            ViewSwitch::Layers
            | ViewSwitch::Toolbar
            | ViewSwitch::StatusMessages
            | ViewSwitch::Focus
            | ViewSwitch::SnapShapes
            | ViewSwitch::SnapGuides
            | ViewSwitch::SnapGrid
            | ViewSwitch::SnapBaselines => None,
        }
    }
}

/// The fixed half of a registry entry: what a row says and where it belongs,
/// before any state is consulted. All `'static`, so `Item::ALL` can be walked by a
/// test asserting every row has a glyph and a group.
pub struct Spec {
    pub label: &'static str,
    pub glyph: &'static str,
    /// The chord this row also answers to, as `docs/shortcuts.md` spells it.
    pub accel: Option<&'static str>,
    pub group: Group,
    /// Destroys something: drawn red.
    pub danger: bool,
    /// Draw the filled hexagon in the glyph's place (`ui::paint_hexagon_filled`);
    /// the font has no filled weight, so the outline glyph is not drawn.
    pub filled_hexagon: bool,
}

impl Item {
    /// Every item in the registry, for the table-driven test. Hand-written, so the
    /// test also builds every menu and demands each row it finds is in here.
    #[cfg(test)]
    pub const ALL: &'static [Item] = &[
        Item::EnterGroup,
        Item::EnterBoolean,
        Item::MakeBaseOperand,
        Item::MakeKeyLayer,
        Item::ClipContent,
        Item::EditText,
        Item::TextSizing(0),
        Item::TextSizing(1),
        Item::TextSizing(2),
        Item::EditPoints,
        Item::EditImage,
        Item::OriginalSize,
        Item::Cut,
        Item::Copy,
        Item::PasteHere,
        Item::PasteInPlace,
        Item::PasteToReplace,
        Item::Duplicate,
        Item::Delete,
        Item::CopyProperties,
        Item::PasteProperties,
        Item::Group,
        Item::FrameSelection,
        Item::CreateComponent,
        Item::DuplicateAsComponent,
        Item::SelectAllInstances,
        Item::GoToMain,
        Item::DetachInstance,
        Item::AcceptKitUpdates,
        Item::MakeLocal,
        Item::ResetInstance,
        Item::ResetChild,
        Item::RestoreChildren,
        Item::ResetProperty,
        Item::CombineAsVariants,
        Item::AddVariant,
        Item::Ungroup,
        Item::Mask,
        Item::EvenOdd,
        Item::Boolean(BoolOp::Union),
        Item::Boolean(BoolOp::Subtract),
        Item::Boolean(BoolOp::Intersect),
        Item::Boolean(BoolOp::Exclude),
        Item::Flatten,
        Item::OutlineShape,
        Item::OutlineStroke,
        Item::JoinPaths,
        Item::OutlineText,
        Item::DetachTextPath,
        Item::FlipTextPath,
        Item::Restack(build::ZMove::Front),
        Item::Restack(build::ZMove::Forward),
        Item::Restack(build::ZMove::Backward),
        Item::Restack(build::ZMove::Back),
        Item::Flip(build::Axis::X),
        Item::Flip(build::Axis::Y),
        Item::ResetOrigin,
        Item::ToggleHidden,
        Item::ToggleLocked,
        Item::Rename,
        Item::ZoomSelection,
        Item::CopyAsSvg,
        Item::CopyAsCss,
        Item::CopyAsPng(1),
        Item::CopyAsPng(2),
        Item::SelectAll,
        Item::ZoomFit,
        Item::ExportAll,
        Item::ExportFrames,
        Item::ViewSwitch(ViewSwitch::Rulers),
        Item::ViewSwitch(ViewSwitch::Guides),
        Item::ViewSwitch(ViewSwitch::GuideLock),
        Item::ViewSwitch(ViewSwitch::Grid),
        Item::ViewSwitch(ViewSwitch::LayoutGrid),
        Item::DeleteGuide,
        Item::ClearGuides,
        Item::AddPointHere,
        Item::DeletePoints,
        Item::SetSmooth(false),
        Item::SetSmooth(true),
        Item::ReverseSubpaths,
        Item::Join,
    ];

    pub fn spec(self) -> Spec {
        let s = |label, glyph, accel, group| Spec {
            label,
            glyph,
            accel,
            group,
            danger: false,
            filled_hexagon: false,
        };
        match self {
            Item::EnterGroup => s("Enter group", icon::SIGN_IN, None, Group::Head),
            Item::EnterBoolean => s("Enter boolean", icon::SIGN_IN, None, Group::Head),
            Item::MakeBaseOperand => s(
                "Make this the base operand",
                icon::DIAMOND,
                None,
                Group::Head,
            ),
            Item::MakeKeyLayer => s("Make this the key layer", icon::DIAMOND, None, Group::Head),
            Item::ClipContent => s("Clip content", icon::BOUNDING_BOX, None, Group::Head),
            Item::EditText => s("Edit text", icon::TEXT_T, None, Group::Head),
            Item::TextSizing(0) => s(
                "Auto width",
                icon::ARROWS_OUT_LINE_HORIZONTAL,
                None,
                Group::Head,
            ),
            Item::TextSizing(1) => s(
                "Auto height",
                icon::ARROWS_OUT_LINE_VERTICAL,
                None,
                Group::Head,
            ),
            Item::TextSizing(_) => s("Fixed size", icon::BOUNDING_BOX, None, Group::Head),
            Item::EditPoints => s("Edit points", icon::BEZIER_CURVE, None, Group::Head),
            // `CROP`, not `IMAGE`: the rail's *Place image* button already wears `IMAGE`.
            Item::EditImage => s("Edit image", icon::CROP, None, Group::Head),
            Item::OriginalSize => s("Original size", icon::NUMBER_SQUARE_ONE, None, Group::Head),

            Item::Cut => s("Cut", icon::SCISSORS, Some("Ctrl+X"), Group::Clipboard),
            Item::Copy => s("Copy", icon::COPY, Some("Ctrl+C"), Group::Clipboard),
            Item::PasteHere => s("Paste", icon::CLIPBOARD, Some("Ctrl+V"), Group::Clipboard),
            Item::PasteInPlace => s(
                "Paste in place",
                icon::CLIPBOARD,
                Some("Ctrl+Shift+V"),
                Group::Clipboard,
            ),
            Item::PasteToReplace => s("Paste to replace", icon::CLIPBOARD, None, Group::Clipboard),
            Item::Duplicate => s(
                "Duplicate",
                icon::COPY_SIMPLE,
                Some("Ctrl+D"),
                Group::Clipboard,
            ),
            // Both rows wear the same glyph: one clipboard read two ways.
            Item::CopyProperties => s(
                "Copy properties",
                icon::SLIDERS_HORIZONTAL,
                Some("Ctrl+Alt+C"),
                Group::Properties,
            ),
            Item::PasteProperties => s(
                "Paste properties",
                icon::SLIDERS_HORIZONTAL,
                Some("Ctrl+Alt+V"),
                Group::Properties,
            ),
            Item::Delete => Spec {
                danger: true,
                ..s("Delete", icon::TRASH, Some("Del"), Group::Clipboard)
            },

            Item::Group => s(
                "Group selection",
                icon::SELECTION_PLUS,
                Some("Ctrl+G"),
                Group::Structure,
            ),
            Item::FrameSelection => s(
                "Frame selection",
                icon::FRAME_CORNERS,
                Some("Ctrl+Alt+G"),
                Group::Structure,
            ),
            // Component rows and chords (`shortcuts.md` §12); the filled hexagon
            // marks the main each of the first two makes.
            Item::CreateComponent => Spec {
                filled_hexagon: true,
                ..s(
                    "Create component",
                    icon::HEXAGON,
                    Some("Ctrl+Alt+K"),
                    Group::Structure,
                )
            },
            Item::DuplicateAsComponent => Spec {
                filled_hexagon: true,
                ..s(
                    "Duplicate as component",
                    icon::HEXAGON,
                    None,
                    Group::Structure,
                )
            },
            Item::SelectAllInstances => s(
                "Select all instances",
                icon::SELECTION_ALL,
                None,
                Group::Structure,
            ),
            Item::GoToMain => s(
                "Go to main component",
                icon::ARROW_UP_RIGHT,
                None,
                Group::Structure,
            ),
            Item::DetachInstance => s(
                "Detach instance",
                icon::LINK_BREAK,
                Some("Ctrl+Alt+B"),
                Group::Structure,
            ),
            Item::AcceptKitUpdates => s(
                "Accept kit updates",
                icon::ARROW_CIRCLE_DOWN,
                None,
                Group::Structure,
            ),
            Item::MakeLocal => s("Make local…", icon::PACKAGE, None, Group::Structure),
            Item::ResetInstance => s(
                "Reset all",
                icon::ARROW_COUNTER_CLOCKWISE,
                None,
                Group::Structure,
            ),
            // Renamed for its layer by `Row::label`; this is the label the table sees.
            Item::ResetChild => s(
                "Reset",
                icon::ARROW_COUNTER_CLOCKWISE,
                None,
                Group::Structure,
            ),
            // One row for both things the dot's tooltip can name (removed, reordered).
            Item::RestoreChildren => s(
                "Restore children",
                icon::ARROW_COUNTER_CLOCKWISE,
                None,
                Group::Structure,
            ),
            // Renamed for its property, as *Reset* is for its layer.
            Item::ResetProperty => s(
                "Reset property",
                icon::ARROW_COUNTER_CLOCKWISE,
                None,
                Group::Structure,
            ),
            Item::CombineAsVariants => s(
                "Combine as variants",
                icon::SQUARES_FOUR,
                None,
                Group::Structure,
            ),
            Item::AddVariant => s("Add variant", icon::PLUS, None, Group::Structure),
            Item::Ungroup => s(
                "Ungroup",
                icon::SELECTION_SLASH,
                Some("Ctrl+Shift+G"),
                Group::Structure,
            ),
            Item::Mask => s(
                "Use as mask",
                icon::CIRCLE_HALF,
                Some("Ctrl+Alt+M"),
                Group::Structure,
            ),
            Item::EvenOdd => s("Even-odd fill", icon::CIRCLE_HALF, None, Group::Structure),
            Item::Boolean(op) => s(
                match op {
                    BoolOp::Union => "Union",
                    BoolOp::Subtract => "Subtract",
                    BoolOp::Intersect => "Intersect",
                    BoolOp::Exclude => "Exclude",
                },
                crate::panels::layers::op_glyph(op),
                Some(match op {
                    BoolOp::Union => "Ctrl+Alt+U",
                    BoolOp::Subtract => "Ctrl+Alt+S",
                    BoolOp::Intersect => "Ctrl+Alt+I",
                    BoolOp::Exclude => "Ctrl+Alt+X",
                }),
                Group::Structure,
            ),
            Item::Flatten => s(
                "Flatten",
                icon::STACK_SIMPLE,
                Some("Ctrl+E"),
                Group::Structure,
            ),
            // Deliberately unbound: a one-way conversion is not one keystroke away.
            Item::OutlineShape => s("Outline shape", icon::POLYGON, None, Group::Structure),
            Item::OutlineStroke => s("Outline stroke", icon::POLYGON, None, Group::Structure),
            Item::JoinPaths => s("Join paths", icon::ARROWS_MERGE, None, Group::Structure),
            // A verb whose whole result is a kind wears that kind's picture (§8):
            // `POLYGON` for the two that make a `Path`, `TEXT_T` for the two that keep text.
            Item::OutlineText => s("Convert to path", icon::POLYGON, None, Group::Structure),
            Item::DetachTextPath => s("Detach from path", icon::TEXT_T, None, Group::Structure),
            // Named for the side a designer sees, not the direction mechanism.
            Item::FlipTextPath => s("Flip to other side", icon::TEXT_T, None, Group::Structure),

            Item::Restack(mv) => s(
                match mv {
                    build::ZMove::Front => "Bring to front",
                    build::ZMove::Forward => "Bring forward",
                    build::ZMove::Backward => "Send backward",
                    build::ZMove::Back => "Send to back",
                },
                // Arrow-to-a-bar for all the way, plain arrow for one step: no
                // picture is used twice, including against *Flatten*, which
                // co-occurs with the Order four.
                match mv {
                    build::ZMove::Front => icon::ARROW_LINE_UP,
                    build::ZMove::Forward => icon::ARROW_UP,
                    build::ZMove::Backward => icon::ARROW_DOWN,
                    build::ZMove::Back => icon::ARROW_LINE_DOWN,
                },
                Some(match mv {
                    build::ZMove::Front => "Ctrl+Shift+]",
                    build::ZMove::Forward => "Ctrl+]",
                    build::ZMove::Backward => "Ctrl+[",
                    build::ZMove::Back => "Ctrl+Shift+[",
                }),
                Group::Order,
            ),

            Item::Flip(build::Axis::X) => s(
                "Flip horizontal",
                icon::FLIP_HORIZONTAL,
                Some("Shift+H"),
                Group::Transform,
            ),
            Item::Flip(_) => s(
                "Flip vertical",
                icon::FLIP_VERTICAL,
                Some("Shift+V"),
                Group::Transform,
            ),
            Item::ResetOrigin => s(
                "Reset origin",
                icon::CROSSHAIR_SIMPLE,
                None,
                Group::Transform,
            ),

            // Labelled *Hide* and *Lock* here and renamed by `build` where the
            // state says otherwise — the spec is the default, not the truth.
            Item::ToggleHidden => s("Hide", icon::EYE_SLASH, Some("Ctrl+Shift+H"), Group::State),
            Item::ToggleLocked => s(
                "Lock",
                icon::LOCK_SIMPLE,
                Some("Ctrl+Shift+L"),
                Group::State,
            ),
            Item::Rename => s("Rename", icon::TEXT_AA, Some("Ctrl+R"), Group::State),

            Item::ZoomSelection => s(
                "Zoom to selection",
                icon::SELECTION,
                Some("Shift+2"),
                Group::Navigate,
            ),

            Item::CopyAsSvg => s("Copy as SVG", icon::SHARE_NETWORK, None, Group::Export),
            Item::CopyAsCss => s("Copy as CSS", icon::BRACKETS_CURLY, None, Group::Export),
            // `FILE_IMAGE` is the Export panel's raster-format glyph.
            Item::CopyAsPng(2) => s("Copy as PNG 2×", icon::FILE_IMAGE, None, Group::Export),
            Item::CopyAsPng(_) => s("Copy as PNG", icon::FILE_IMAGE, None, Group::Export),
            Item::ExportAll => s(
                "Export all",
                icon::EXPORT,
                Some("Ctrl+Alt+E"),
                Group::Export,
            ),
            Item::ExportFrames => s("Export all frames…", icon::EXPORT, None, Group::Export),

            Item::SelectAll => s("Select all", icon::SCAN, Some("Ctrl+A"), Group::Clipboard),
            Item::ZoomFit => s(
                "Zoom to fit",
                icon::FRAME_CORNERS,
                Some("Ctrl+1"),
                Group::Navigate,
            ),
            Item::ViewSwitch(sw) => s(
                sw.label(),
                match sw {
                    ViewSwitch::Rulers => icon::RULER,
                    ViewSwitch::Guides => icon::LINE_VERTICAL,
                    ViewSwitch::GuideLock => icon::LOCK_SIMPLE,
                    ViewSwitch::Grid => icon::GRID_FOUR,
                    // The glyph the Layout grid card already draws for columns.
                    ViewSwitch::LayoutGrid => icon::COLUMNS,
                    // Placeholders: no menu builds a row for these, and spelling
                    // them out makes a new switch a compile error here.
                    ViewSwitch::Layers
                    | ViewSwitch::Toolbar
                    | ViewSwitch::StatusMessages
                    | ViewSwitch::Focus
                    | ViewSwitch::SnapShapes
                    | ViewSwitch::SnapGuides
                    | ViewSwitch::SnapGrid
                    | ViewSwitch::SnapBaselines => icon::SLIDERS_HORIZONTAL,
                },
                sw.accel(),
                Group::View,
            ),

            Item::DeleteGuide => Spec {
                danger: true,
                ..s("Delete guide", icon::TRASH, Some("Del"), Group::Clipboard)
            },
            Item::ClearGuides => Spec {
                danger: true,
                ..s("Clear all guides", icon::X, None, Group::Clipboard)
            },

            // Head puts it above *Delete points* (§6.5): the point menu's own verb.
            Item::AddPointHere => s("Add point here", icon::PLUS, None, Group::Head),
            Item::DeletePoints => Spec {
                danger: true,
                ..s("Delete points", icon::TRASH, Some("Del"), Group::Clipboard)
            },
            // Structure, not Clipboard: these change what the path is rather than removing part of it.
            Item::SetSmooth(true) => s("Make smooth", icon::BEZIER_CURVE, None, Group::Structure),
            Item::SetSmooth(false) => s("Make corner", icon::LINE_SEGMENT, None, Group::Structure),
            // Label and accelerator are painted at fixed anchors, so a longer label overlaps the chord.
            Item::ReverseSubpaths => s(
                "Reverse subpath",
                icon::ARROWS_CLOCKWISE,
                Some("Shift+D"),
                Group::Structure,
            ),
            // Shares `ARROWS_MERGE` with the inspector's *Flatten*; never in the same menu.
            Item::Join => s("Join", icon::ARROWS_MERGE, None, Group::Structure),

            Item::Undo => s(
                "Undo",
                icon::ARROW_COUNTER_CLOCKWISE,
                Some("Ctrl+Z"),
                Group::History,
            ),
            Item::Redo => s(
                "Redo",
                icon::ARROW_CLOCKWISE,
                Some("Ctrl+Shift+Z"),
                Group::History,
            ),
            Item::Save => s(
                "Save a version",
                icon::FLOPPY_DISK,
                Some("Ctrl+S"),
                Group::File,
            ),
            Item::NewDocument => s("New file", icon::PLUS_SQUARE, Some("Ctrl+N"), Group::File),
            Item::CloseDocument => s("Close file", icon::X, Some("Ctrl+W"), Group::File),
            Item::Library => s(
                "Go to the library",
                icon::SQUARES_FOUR,
                Some("Ctrl+O"),
                Group::File,
            ),
            Item::Settings => s("Settings", icon::SLIDERS, Some("Ctrl+,"), Group::File),
            Item::ExportCode(t) => s(
                match t {
                    ondin_export::code::Target::Html => "Export as HTML…",
                    ondin_export::code::Target::React => "Export as React…",
                    ondin_export::code::Target::Vue => "Export as Vue…",
                },
                icon::BRACKETS_CURLY,
                None,
                Group::Export,
            ),
            Item::Tool(tool) => {
                let (label, glyph, accel) = match tool {
                    Tool::Select => ("Tool: Select", icon::CURSOR, "V"),
                    Tool::Hand => ("Tool: Hand", icon::HAND, "H"),
                    Tool::Scale => ("Tool: Scale", icon::RESIZE, "K"),
                    Tool::Frame => ("Tool: Frame", icon::FRAME_CORNERS, "F"),
                    Tool::Rect => ("Tool: Rectangle", icon::SQUARE, "R"),
                    Tool::Ellipse => ("Tool: Ellipse", icon::CIRCLE, "O"),
                    Tool::Polygon => ("Tool: Polygon", icon::TRIANGLE, "G"),
                    Tool::Star => ("Tool: Star", icon::STAR, "S"),
                    Tool::Line => ("Tool: Line", icon::LINE_SEGMENT, "L"),
                    Tool::Pen => ("Tool: Pen", icon::PEN_NIB, "P"),
                    Tool::Node => ("Tool: Edit points", icon::BEZIER_CURVE, "A"),
                    Tool::Text => ("Tool: Text", icon::TEXT_T, "T"),
                    Tool::Eyedropper => ("Tool: Eyedropper", icon::EYEDROPPER, "I"),
                    // Neither has a letter; [`Self::PlaceImage`] is the image tool's
                    // door and image editing needs a subject.
                    Tool::Image | Tool::ImageEdit => ("Tool: Image", icon::IMAGE, ""),
                };
                s(
                    label,
                    glyph,
                    (!accel.is_empty()).then_some(accel),
                    Group::Tools,
                )
            }
            Item::PlaceImage => s(
                "Place image…",
                icon::IMAGE,
                Some("Ctrl+Shift+K"),
                Group::Tools,
            ),
            Item::ZoomIn => s(
                "Zoom in",
                icon::MAGNIFYING_GLASS_PLUS,
                Some("Ctrl+="),
                Group::Navigate,
            ),
            Item::ZoomOut => s(
                "Zoom out",
                icon::MAGNIFYING_GLASS_MINUS,
                Some("Ctrl+-"),
                Group::Navigate,
            ),
            Item::ZoomReset => s(
                "Zoom to 100%",
                icon::NUMBER_SQUARE_ONE,
                Some("Ctrl+0"),
                Group::Navigate,
            ),
            Item::Align(axis, edge) => {
                use build::{Axis, Edge};
                let (label, glyph, accel) = match (axis, edge) {
                    (Axis::X, Edge::Min) => ("Align left", icon::ALIGN_LEFT, "Alt+A"),
                    (Axis::X, Edge::Mid) => (
                        "Align horizontal centres",
                        icon::ALIGN_CENTER_HORIZONTAL,
                        "Alt+H",
                    ),
                    (Axis::X, Edge::Max) => ("Align right", icon::ALIGN_RIGHT, "Alt+D"),
                    (Axis::Y, Edge::Min) => ("Align top", icon::ALIGN_TOP, "Alt+W"),
                    (Axis::Y, Edge::Mid) => (
                        "Align vertical centres",
                        icon::ALIGN_CENTER_VERTICAL,
                        "Alt+V",
                    ),
                    (Axis::Y, Edge::Max) => ("Align bottom", icon::ALIGN_BOTTOM, "Alt+S"),
                };
                s(label, glyph, Some(accel), Group::Transform)
            }
            Item::Distribute(build::Axis::X) => s(
                "Distribute horizontally",
                icon::ARROWS_HORIZONTAL,
                Some("Alt+Shift+H"),
                Group::Transform,
            ),
            Item::Distribute(_) => s(
                "Distribute vertically",
                icon::ARROWS_VERTICAL,
                Some("Alt+Shift+V"),
                Group::Transform,
            ),
            Item::InsertPicker => s("Insert", icon::HEXAGON, Some("Shift+I"), Group::Kits),
            Item::VariablesBrowser => s(
                "Styles and variables",
                icon::SWATCHES,
                Some("Shift+S"),
                Group::Kits,
            ),
            Item::PublishKit => s(
                "Publish kit…",
                icon::UPLOAD_SIMPLE,
                Some("Ctrl+Alt+P"),
                Group::Kits,
            ),
            Item::MakeKit => s("Make this a kit", icon::PACKAGE, None, Group::Kits),
            Item::KitsUsed => s("Kits used…", icon::PACKAGE, None, Group::Kits),
            Item::StopSharingKit => s("Stop sharing as kit…", icon::PACKAGE, None, Group::Kits),
            Item::ShowInLibrary => s("Show in library", icon::FOLDER_OPEN, None, Group::File),
            Item::VersionHistory => s(
                "Version history…",
                icon::CLOCK_COUNTER_CLOCKWISE,
                None,
                Group::File,
            ),
            Item::RotateQuarter(false) => s(
                "Rotate 90° counter-clockwise",
                icon::ARROW_COUNTER_CLOCKWISE,
                None,
                Group::Transform,
            ),
            Item::RotateQuarter(true) => s(
                "Rotate 90° clockwise",
                icon::ARROW_CLOCKWISE,
                None,
                Group::Transform,
            ),
            Item::SelectSame(same) => {
                use crate::input::Same;
                let label = match same {
                    Same::Fill => "Select same fill",
                    Same::Stroke => "Select same stroke",
                    Same::Font => "Select same font",
                    Same::Effect => "Select same effects",
                };
                s(label, icon::SELECTION_ALL, None, Group::Clipboard)
            }
            Item::SelectInverse => s("Select inverse", icon::SELECTION, None, Group::Clipboard),
            Item::TextChord(chord) => {
                use crate::input::TextChord;
                use ondin_core::TextAlign;
                let (label, glyph, accel) = match chord {
                    TextChord::Bold => ("Bold", icon::TEXT_BOLD, "Ctrl+B"),
                    TextChord::Italic => ("Italic", icon::TEXT_ITALIC, "Ctrl+I"),
                    TextChord::Underline => ("Underline", icon::TEXT_UNDERLINE, "Ctrl+U"),
                    TextChord::Size(n) if n > 0 => ("Larger text", icon::TEXT_AA, "Ctrl+Shift+."),
                    TextChord::Size(_) => ("Smaller text", icon::TEXT_AA, "Ctrl+Shift+,"),
                    TextChord::Tracking(n) if n > 0 => {
                        ("Wider letter spacing", icon::ARROWS_HORIZONTAL, "Alt+→")
                    }
                    TextChord::Tracking(_) => {
                        ("Tighter letter spacing", icon::ARROWS_HORIZONTAL, "Alt+←")
                    }
                    TextChord::Leading(n) if n > 0 => {
                        ("Taller line height", icon::ARROWS_VERTICAL, "Alt+↑")
                    }
                    TextChord::Leading(_) => {
                        ("Shorter line height", icon::ARROWS_VERTICAL, "Alt+↓")
                    }
                    TextChord::Align(TextAlign::Center) => {
                        ("Centre text", icon::TEXT_ALIGN_CENTER, "Ctrl+Shift+C")
                    }
                    TextChord::Align(TextAlign::End | TextAlign::Right) => {
                        ("Align text right", icon::TEXT_ALIGN_RIGHT, "Ctrl+Shift+R")
                    }
                    TextChord::Align(TextAlign::Justify) => {
                        ("Justify text", icon::TEXT_ALIGN_JUSTIFY, "Ctrl+Shift+J")
                    }
                    TextChord::Align(TextAlign::Start | TextAlign::Left) => {
                        ("Align text left", icon::TEXT_ALIGN_LEFT, "Ctrl+Shift+L")
                    }
                };
                s(label, glyph, Some(accel), Group::Text)
            }
            // Renamed for its card by the palette; this is the label the table sees.
            Item::ShowCard(_) => s("Show card", icon::SLIDERS_HORIZONTAL, None, Group::Panels),
            Item::Palette => s(
                "Command palette",
                icon::MAGNIFYING_GLASS,
                Some("Ctrl+K"),
                Group::File,
            ),
            Item::Cheatsheet => s(
                "Keyboard shortcuts",
                icon::LIST,
                Some("Ctrl+Shift+/"),
                Group::File,
            ),
            Item::Present => s("Present", icon::PLAY, Some("Ctrl+Alt+Enter"), Group::View),
        }
    }

    /// The chord's second spelling, where `docs/shortcuts.md` binds two — the
    /// cheatsheet shows both, joined by "or".
    pub fn alt_accel(self) -> Option<&'static str> {
        match self {
            Item::Redo => Some("Ctrl+Y"),
            Item::Rename => Some("F2"),
            Item::ZoomFit => Some("Shift+1"),
            Item::ZoomSelection => Some("Ctrl+2"),
            Item::ZoomReset => Some("Shift+0"),
            Item::Palette => Some("Ctrl+/"),
            Item::Delete => Some("Backspace"),
            _ => None,
        }
    }

    /// Other words the palette finds this row by, beyond its label.
    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            Item::CopyAsSvg => &["svg", "vector"],
            Item::CopyAsCss => &["css", "style", "code"],
            Item::CopyAsPng(_) => &["png", "bitmap", "raster", "image"],
            Item::ExportFrames => &["artboards", "png", "export"],
            Item::ExportCode(_) => &["code", "html", "react", "vue", "zip"],
            Item::ToggleHidden => &["hide", "show", "visibility"],
            Item::ToggleLocked => &["lock", "unlock"],
            Item::Delete => &["remove"],
            Item::Library => &["open", "files", "dashboard", "home"],
            Item::Settings => &["preferences", "options"],
            Item::Save => &["version", "pin"],
            Item::CreateComponent => &["main", "symbol"],
            Item::InsertPicker => &["component", "kit", "asset"],
            Item::VariablesBrowser => &["tokens", "colours", "colors"],
            Item::Boolean(_) | Item::Flatten => &["boolean", "combine"],
            Item::Restack(_) => &["order", "arrange", "z"],
            Item::Cheatsheet => &["shortcuts", "keys", "keymap", "help"],
            Item::Present => &["play", "slideshow", "presentation", "fullscreen"],
            Item::ViewSwitch(ViewSwitch::Focus) => &["zen"],
            Item::ZoomFit => &["fit", "all"],
            Item::SelectSame(_) => &["matching", "similar", "colour", "color", "family"],
            Item::SelectInverse => &["invert", "others", "rest"],
            Item::TextChord(_) => &["text", "type"],
            Item::Tool(Tool::Rect) => &["square", "box"],
            Item::Tool(Tool::Node) => &["node", "direct"],
            _ => &[],
        }
    }

    /// The keymap's own verb for this row, where it has one: what its chord
    /// resolves to. [`OndinApp::perform_menu_item`] dispatches it for the app verbs;
    /// the menu rows have their own arms there, some door-dependent (*Paste here*).
    /// The cheatsheet's test holds every chord on the sheet to this.
    pub fn action(self) -> Option<crate::input::Action> {
        use crate::input::Action;
        Some(match self {
            Item::Cut => Action::Cut,
            Item::Copy => Action::Copy,
            Item::PasteHere => Action::Paste,
            Item::PasteInPlace => Action::PasteInPlace,
            Item::PasteToReplace => Action::PasteToReplace,
            Item::CopyProperties => Action::CopyProperties,
            Item::PasteProperties => Action::PasteProperties,
            Item::Duplicate => Action::Duplicate,
            Item::Delete => Action::Delete,
            Item::SelectAll => Action::SelectAll,
            Item::Group => Action::Group,
            Item::Ungroup => Action::Ungroup,
            Item::FrameSelection => Action::FrameSelection,
            Item::CreateComponent => Action::CreateComponent,
            Item::DetachInstance => Action::DetachInstance,
            Item::Mask => Action::Mask,
            Item::Boolean(op) => Action::Boolean(op),
            Item::Flatten => Action::Flatten,
            Item::ReverseSubpaths => Action::ReverseSubpaths,
            Item::Restack(mv) => Action::Restack(mv),
            Item::Flip(axis) => Action::Flip(axis),
            Item::ToggleHidden => Action::ToggleHidden,
            Item::ToggleLocked => Action::ToggleLocked,
            Item::Rename => Action::Rename,
            Item::Undo => Action::Undo,
            Item::Redo => Action::Redo,
            Item::Save => Action::Save,
            Item::NewDocument => Action::NewDocument,
            Item::CloseDocument => Action::CloseDocument,
            Item::Library => Action::Open,
            Item::Settings => Action::OpenSettings,
            Item::ExportCode(t) => Action::ExportCode(t),
            Item::Tool(tool) => Action::ChooseTool(tool),
            Item::PlaceImage => Action::PlaceImage,
            Item::ZoomIn => Action::ZoomIn,
            Item::ZoomOut => Action::ZoomOut,
            Item::ZoomReset => Action::ZoomReset,
            Item::Align(axis, edge) => Action::Align(axis, edge),
            Item::Distribute(axis) => Action::Distribute(axis),
            Item::InsertPicker => Action::InsertPicker,
            Item::VariablesBrowser => Action::VariablesBrowser,
            Item::PublishKit => Action::PublishKit,
            Item::MakeKit => Action::MakeKit,
            Item::KitsUsed => Action::KitsUsed,
            Item::StopSharingKit => Action::StopSharingKit,
            Item::ShowInLibrary => Action::ShowInLibrary,
            Item::VersionHistory => Action::VersionHistory,
            Item::RotateQuarter(clockwise) => Action::RotateQuarter(clockwise),
            Item::SelectSame(same) => Action::SelectSame(same),
            Item::SelectInverse => Action::SelectInverse,
            Item::TextChord(chord) => Action::TextStyle(chord),
            Item::Palette => Action::Palette,
            Item::Cheatsheet => Action::Cheatsheet,
            Item::Present => Action::Present,
            Item::ViewSwitch(sw) => Action::ToggleView(sw),
            Item::ZoomFit => Action::ZoomFit,
            Item::ZoomSelection => Action::ZoomSelection,
            Item::ExportAll => Action::ExportAll,
            _ => return None,
        })
    }
}

/// One row, as a particular menu open resolved it.
pub struct Row {
    pub item: Item,
    /// Usually the spec's, overridden where the state renames the verb (*Show* for
    /// *Hide*) or names its subject (*Reset Label*), hence an owned string.
    pub label: std::borrow::Cow<'static, str>,
    /// `Some` on a checkable row; the glyph column carries it (see
    /// [`crate::ui::MenuRow::checked`]).
    pub checked: Option<bool>,
    pub enabled: bool,
    /// Why this row is dim. Required whenever `enabled` is false.
    pub why: Option<&'static str>,
    /// Where this row goes, when that is not [`Spec::group`]: a row a kind promotes
    /// into its head (*Ungroup* on a group, *Flatten* on a boolean) is moved, not
    /// duplicated.
    pub group: Option<Group>,
}

impl Row {
    fn new(item: Item) -> Self {
        Self {
            item,
            label: item.spec().label.into(),
            checked: None,
            enabled: true,
            why: None,
            group: None,
        }
    }
    /// Move this row into the head (§3), where its kind wants it read.
    fn promote(mut self) -> Self {
        self.group = Some(Group::Head);
        self
    }
    fn label(mut self, label: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        self.label = label.into();
        self
    }
    /// Give this row a tick box in the state `on`. A `None` (from
    /// [`ViewState::get`]) leaves the row with no tick rather than a borrowed one.
    fn checked(mut self, on: impl Into<Option<bool>>) -> Self {
        self.checked = on.into();
        self
    }
    fn dim_if(mut self, dim: bool, why: &'static str) -> Self {
        if dim {
            self.enabled = false;
            self.why = Some(why);
        }
        self
    }
}

/// What the registry needs to know about the app to answer a menu open.
///
/// A snapshot passed in rather than `&OndinApp`, so building a menu cannot touch
/// the document and a test can build one without an app. Every field is a question
/// the app already answered elsewhere.
pub struct Context<'a> {
    pub target: Target,
    /// The selection the rows act on. A hit inside it leaves it whole (C2), so it
    /// already contains the target.
    pub selection: &'a [NodeId],
    /// `true` when a *Paste* row has anything to do — what dims every one except
    /// the text session's. Composed over three payloads (picture, in-app, OS text)
    /// in [`crate::app::OndinApp::menu_context`].
    pub can_paste: bool,
    /// Whether the app's own layers are on the clipboard and still current — what
    /// dims *Paste to replace*, which has no answer for a picture or text.
    pub can_replace: bool,
    /// The kind of every selected layer, in `selection` order. A head row appears
    /// only when every member qualifies; a tail row when any does.
    pub kinds: &'a [Kind],
    /// The target layer's flags (locked, has a picture, moved pivot, clips…), and
    /// what *Hide* / *Show* and *Lock* / *Unlock* read off the selection's set.
    pub state: LayerState,
    /// Whether the document holds any guide at all — what makes *Clear all guides* offered.
    pub any_guides: bool,
    /// Each of the five View switches this menu can show, as it stands.
    pub view: ViewState,
    /// Whether focus mode is on — read by *Rename*, whose field focus mode
    /// hides. Not in [`ViewState`]: no context menu draws a
    /// *Focus mode* row (the top bar's View menu does), so `ViewState::get` must
    /// not answer for it.
    pub focus: bool,
    /// Whether a segment (rather than only anchors) is part of the point
    /// selection — what separates §6.5's two Delete rows.
    pub segment_selected: bool,
    /// Whether the right-click landed on the edited path's ink — what makes *Add
    /// point here* live. Answered by `canvas::segment_at`, so the row cannot offer
    /// what a click there would decline.
    pub on_segment: bool,
    /// `tools::joinable_ends` said yes — the point selection is exactly two open
    /// ends. The builder's own predicate, so the row cannot offer what `join_ends` declines.
    pub can_join: bool,
    /// Why `OndinApp::join_layers_subject` refused the two selected paths, if it did
    /// (a lock on either, no open end) — what dims *Join paths*, in its words.
    pub join_paths_refused: Option<&'static str>,
    /// `OndinApp::properties_source` found a layer to lift an appearance off. Two
    /// refusals sit behind it; the row's sentence picks between them on the
    /// selection's size.
    pub can_copy_props: bool,
    /// Whether the selection has a key layer; read only to pick which sentence a
    /// *Copy properties* refusal shows, alongside [`Self::can_copy_props`].
    pub has_key: bool,
    /// Whether [`crate::app::OndinApp::property_clipboard`] is holding anything —
    /// what dims *Paste properties*.
    pub has_props: bool,
    /// `build::can_outline_text` said yes. A snapshot taken at open; see
    /// [`crate::app::ContextMenu::text_outlineable`].
    pub text_outlineable: bool,
    /// The one selected layer is text already on a rail — what makes *Detach from
    /// path* offered.
    pub on_a_rail: bool,
    /// Whether that rail is run backwards — the tick on *Flip to other side*;
    /// meaningless while [`Self::on_a_rail`] is false.
    pub rail_flipped: bool,
    /// Whether the system clipboard held text when the menu opened — what dims the
    /// text session's *Paste* (§6.4). See [`crate::app::ContextMenu::system_text`].
    pub system_text: bool,
    /// Whether the live text session has a selection rather than a bare caret —
    /// what dims *Cut* and *Copy* (§6.4).
    pub text_selection: bool,
    /// Whether the live text session's buffer is empty — what dims *Select all*.
    pub text_empty: bool,
    /// Whether any layer carries export settings — what makes *Export all* offered
    /// (§7). A document question: it reads neither the selection nor the pointer.
    pub any_exports: bool,
    /// Whether a frame sits straight on the canvas — what makes *Export all
    /// frames…* offered.
    pub any_frames: bool,
    /// `build::can_frame` said yes — the selection has one parent that can hold a
    /// frame. The builder's own predicate; an `Artboard` may sit under neither a
    /// boolean nor a mask.
    pub can_frame: bool,
    /// Why *Use as mask* would refuse the selection, when it would — the
    /// inspector's own sentence (`OndinApp::mask_refusal`), so the row dims where
    /// the click would fail.
    pub mask_refused: Option<&'static str>,
    /// What a single selected layer is to the components machinery; decides which
    /// component rows appear.
    pub role: Role,
    /// Whether a reset of that one layer would change anything
    /// (`reset::Drift::any`) — what offers *Reset all* and *Reset Label*; omitted
    /// at zero.
    pub drifted: bool,
    /// Whether that one layer's children differ from its source's
    /// (`reset::ChildDrift::any`) — what offers *Restore children*, omitted at
    /// zero. Own children while expanded, whole subtree while collapsed
    /// (`OndinApp::child_drift_of`).
    pub children_differ: bool,
    /// That one layer's name, which *Reset Label* is named for. Empty for several.
    pub layer_name: String,
    /// When the one selected instance is a kit instance: its kit's name, and
    /// whether a newer publish waits (§5.3e). Renames *Go to main* to *Go to main
    /// in Atlas* and offers *Accept kit updates* and *Make local…*.
    pub kit: Option<(String, bool)>,
    /// The selection is two or more mains, none in a set, sharing a parent.
    pub combinable: bool,
    /// The one selected main is a variant — what offers it *Add variant*.
    pub in_set: bool,
    /// The selection holds a main or a set — what withholds *Create component*.
    pub holds_main: bool,
    /// Why the commit would refuse *Create component* on this selection, when it
    /// would (`component::why_not_main` / `why_not_wrap`, [`create_refusal`]).
    pub create_refused: Option<&'static str>,
    /// Why *Combine as variants* will not take these mains, when it will not —
    /// [`COMBINE_MIXED`].
    pub combine_refused: Option<&'static str>,
    /// The property the one selected linked layer drives, when its field differs
    /// from the main's — what offers *Reset Label text*.
    pub property_reset: Option<String>,
}

/// A layer's part in a component, for the menu's component rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Role {
    /// Not a main and not inside an instance — or a selection of several.
    #[default]
    Plain,
    /// A main component.
    Main,
    /// A component set — the frame around variants.
    Set,
    /// An instance root.
    Instance,
    /// A layer inside an instance, linked to its counterpart in the main.
    Member,
    /// A layer inside an instance that exists only there.
    Local,
}

/// The sentence for why *Create component* will not take the selection — the
/// dimmed row's reason and the chord's failure message alike.
pub(crate) fn create_refusal(why: ondin_core::component::NotMain) -> &'static str {
    use ondin_core::component::NotMain;
    match why {
        NotMain::Kind => "Only a frame or a group can become a component",
        NotMain::Already => "Already a component",
        NotMain::Inside => "A component cannot sit inside a component or an instance",
        NotMain::Instance => "An instance can't become a component — detach it first (Ctrl+Alt+B).",
        NotMain::Holds => "A component cannot hold another component or a component set",
        NotMain::SetKind => {
            "A component set holds one kind of layer — its variants are another kind"
        }
    }
}

/// Why *Combine as variants* will not make one set of the selected mains: one
/// kind per set. The dimmed row's reason and the verb's message.
pub(crate) const COMBINE_MIXED: &str =
    "A component set holds one kind of layer — these mains mix frames and groups";

/// The parts of the layer under the pointer that decide a row's label or its
/// presence.
#[derive(Clone, Copy, Default)]
pub struct LayerState {
    /// Every layer *Hide* / *Show* acts on (the selection's outermost layers) is
    /// hidden, so the row reads *Show*.
    pub all_hidden: bool,
    /// Every layer *Lock* / *Unlock* acts on is locked, so the row reads *Unlock*.
    pub all_locked: bool,
    /// This layer's own lock flag, which picks the editing rows' refusal sentence.
    /// What dims an editing row is [`Self::locked_within`]; the *Lock* / *Unlock*
    /// label reads [`Self::all_locked`].
    pub locked: bool,
    /// Some ancestor is locked.
    ///
    /// Stored apart from [`Self::locked`] so [`Self::locked_within`] derives the
    /// combination; a fixture written `LayerState { locked: true, ..default() }`
    /// then cannot spell "locked but not effectively locked".
    pub locked_by_ancestor: bool,
    pub clips: bool,
    /// This layer is a mask, or a container holding one — the question
    /// `inspector::mask_action` asks to decide whether the verb makes or releases.
    pub masked: bool,
    /// This layer's stored even-odd rule, not `Node::fill_rule`'s derived answer;
    /// the row is only offered on a `Path`, where the two agree.
    pub even_odd: bool,
    /// `croppable_fill` said yes — the same predicate image editing acts on (§5.7).
    pub has_picture: bool,
    pub pivot_moved: bool,
    /// The layer is inside a `Boolean`, so it can be made the base operand.
    pub in_boolean: bool,
    /// Which of `TextSizing`'s three cells this layer is in, or `None` when it is
    /// not text (`TextSizing::cell`). An `Option` so the no-answer state cannot be
    /// spelled as a default index.
    pub text_sizing: Option<u8>,
    /// `build::can_outline` said yes — outlining would produce a layer different
    /// from this one. State rather than kind: a `Path` answers yes only while its
    /// corner radii are still in the model rather than in its geometry.
    pub outlineable: bool,
    /// `build::can_outline_stroke` said yes — the layer has a visible stroke.
    pub stroke_outlineable: bool,
}

impl LayerState {
    /// Whether an edit of this layer is refused: its own lock, or a lock on
    /// anything it sits inside. Every `dim_if` reads this.
    pub fn locked_within(&self) -> bool {
        self.locked || self.locked_by_ancestor
    }
}

/// Build the menu for one open: groups in canonical order, empty ones dropped.
///
/// Each menu says which rows it wants; [`Spec::group`] says where they go, once
/// per row. Within a group the emitted order is kept. The return is
/// `Vec<Vec<Row>>` so an empty group collapsing with its separator is true by
/// construction.
pub fn build(cx: &Context<'_>) -> Vec<Vec<Row>> {
    let mut rows = match cx.target {
        Target::Canvas => canvas_menu(cx),
        Target::PanelBackground => panel_background_menu(cx),
        Target::Guide(_) => guide_menu(cx),
        Target::TextSession => text_session_menu(cx),
        Target::Points => points_menu(cx),
        Target::Layer { .. } => layer_menu(cx),
    };
    let mut out: Vec<Vec<Row>> = Vec::new();
    for group in Group::ORDER {
        let (mine, rest): (Vec<Row>, Vec<Row>) = rows
            .into_iter()
            .partition(|r| r.group.unwrap_or_else(|| r.item.spec().group) == group);
        rows = rest;
        if !mine.is_empty() {
            out.push(mine);
        }
    }
    debug_assert!(rows.is_empty(), "a row landed in no group at all");
    out
}

/// §6.1 — the page menu, trimmed to what this app has.
///
/// Carries no selection rows even while something is selected (C3).
fn canvas_menu(cx: &Context<'_>) -> Vec<Row> {
    let mut rows = vec![
        Row::new(Item::PasteHere)
            .label("Paste here")
            .dim_if(!cx.can_paste, "Nothing has been copied yet"),
        Row::new(Item::PasteInPlace).dim_if(!cx.can_paste, "Nothing has been copied yet"),
        Row::new(Item::SelectAll),
        Row::new(Item::ZoomFit),
        Row::new(Item::ExportAll).dim_if(
            !cx.any_exports,
            "No layer in this document has export settings",
        ),
        Row::new(Item::ExportFrames).dim_if(!cx.any_frames, "No frames on the canvas"),
    ];
    // *Lock guides* is here because with guides locked a guide is not a target (C7).
    rows.extend(view_rows(cx));
    rows
}

/// §6.2 — only the two rows about the tree as a whole; *Collapse all* / *Expand
/// all* is already a header button.
fn panel_background_menu(cx: &Context<'_>) -> Vec<Row> {
    vec![
        Row::new(Item::PasteHere).dim_if(!cx.can_paste, "Nothing has been copied yet"),
        Row::new(Item::SelectAll),
    ]
}

/// §6.3 — the guide's own verbs, then the two switches that govern all guides.
///
/// Colour and position stay in the inspector, which shows both for a selected
/// guide and has the picker beside them.
fn guide_menu(cx: &Context<'_>) -> Vec<Row> {
    vec![
        Row::new(Item::DeleteGuide),
        Row::new(Item::ClearGuides).dim_if(!cx.any_guides, "There are no guides to clear"),
        Row::new(Item::ViewSwitch(ViewSwitch::GuideLock))
            .checked(cx.view.get(ViewSwitch::GuideLock)),
        Row::new(Item::ViewSwitch(ViewSwitch::Guides)).checked(cx.view.get(ViewSwitch::Guides)),
    ]
}

/// §6.4 — four rows, all acting on the editor rather than the document:
/// `perform_menu_item` routes the shared [`Item`]s by target. *Paste* reads the OS
/// clipboard's text, where the layer menus paste [`crate::app::OndinApp::clipboard`]'s
/// captured subtrees.
fn text_session_menu(cx: &Context<'_>) -> Vec<Row> {
    vec![
        Row::new(Item::Cut).dim_if(!cx.text_selection, "Nothing is selected"),
        Row::new(Item::Copy).dim_if(!cx.text_selection, "Nothing is selected"),
        Row::new(Item::PasteHere)
            .label("Paste")
            .dim_if(!cx.system_text, "The clipboard holds no text"),
        Row::new(Item::SelectAll).dim_if(cx.text_empty, "There is no text yet"),
    ]
}

/// §6.5 — the anchors, the segment or the subpath under the pointer.
///
/// Both conversion rows are offered whatever the selection holds: over a set of
/// anchors the verb almost always has something to do, and converting an already
/// smooth point is idempotent.
fn points_menu(cx: &Context<'_>) -> Vec<Row> {
    vec![
        Row::new(Item::AddPointHere).dim_if(
            !cx.on_segment,
            "Right-click on the path's outline to add a point to it",
        ),
        // The row's name shows which of `Del`'s two verbs the selection has armed.
        Row::new(Item::DeletePoints).label(if cx.segment_selected {
            "Delete segment"
        } else {
            "Delete points"
        }),
        Row::new(Item::SetSmooth(false)),
        Row::new(Item::SetSmooth(true)),
        Row::new(Item::ReverseSubpaths),
        // Last within its group (§6.5).
        Row::new(Item::Join).dim_if(!cx.can_join, "Select two open ends of this path to join"),
    ]
}

/// A layer, through either door — §4's invariant tail with §5's head on top.
fn layer_menu(cx: &Context<'_>) -> Vec<Row> {
    let st = cx.state;
    let panel = matches!(
        cx.target,
        Target::Layer {
            door: Door::Panel,
            ..
        }
    );
    let one = cx.selection.len() == 1;

    // §5.9 — a locked layer's editing rows are dimmed with one sentence while the
    // way-back rows stay live. Only the panel can deliver it (C4). `locked_within`,
    // not `locked`: a locked group locks its contents. `locked` picks the sentence.
    let locked = st.locked_within();
    // The reason names the thing that would have to change.
    let why = if st.locked {
        "This layer is locked"
    } else {
        "A group containing this layer is locked"
    };

    let mut rows = head_rows(cx);

    // --- clipboard --------------------------------------------------------
    rows.extend([
        Row::new(Item::Cut).dim_if(locked, why),
        Row::new(Item::Copy),
        Row::new(Item::PasteHere)
            .label(if panel { "Paste" } else { "Paste here" })
            .dim_if(!cx.can_paste, "Nothing has been copied yet"),
        Row::new(Item::PasteToReplace)
            .dim_if(!cx.can_replace, "No layers have been copied")
            .dim_if(locked, why),
        Row::new(Item::Duplicate).dim_if(locked, why),
        Row::new(Item::Delete).dim_if(locked, why),
    ]);

    // --- properties (§3's third group) ------------------------------------
    // Present for every kind (a group is a valid destination); the source rule is state, so it dims.
    rows.extend([
        Row::new(Item::CopyProperties).dim_if(
            !cx.can_copy_props,
            // Three refusals: empty, ambiguous (no key), and a key with no paint.
            match (cx.selection.len(), cx.has_key) {
                (0 | 1, _) => "This layer has no fill or stroke of its own",
                (_, false) => "Make one of these the key layer to copy from it",
                (_, true) => "The key layer has no fill or stroke of its own",
            },
        ),
        Row::new(Item::PasteProperties)
            .dim_if(!cx.has_props, "No properties have been copied yet")
            .dim_if(locked, why),
    ]);

    // --- structure --------------------------------------------------------
    rows.push(Row::new(Item::Group).dim_if(locked, why));
    // Dimmed rather than absent where the parent chain cannot hold a frame: the
    // user can fix that by moving the layers.
    rows.push(
        Row::new(Item::FrameSelection)
            .dim_if(
                !cx.can_frame,
                "A frame cannot go inside a boolean or a mask",
            )
            .dim_if(locked, why),
    );
    // Component rows by `Role`. A reset is omitted when nothing differs (§3's
    // exception). Mains of two kinds get *Combine as variants* dimmed with the
    // reason, since the user can fix it.
    if cx.combinable {
        rows.push(
            Row::new(Item::CombineAsVariants)
                .dim_if(
                    cx.combine_refused.is_some(),
                    cx.combine_refused.unwrap_or(""),
                )
                .dim_if(locked, why),
        );
    }
    match cx.role {
        // Not over a main or a set (`component::check` refuses nesting), and dimmed
        // with the commit's own reason where the commit would refuse anyway.
        Role::Plain | Role::Local if !cx.holds_main => {
            rows.push(
                Row::new(Item::CreateComponent)
                    .dim_if(cx.create_refused.is_some(), cx.create_refused.unwrap_or(""))
                    .dim_if(locked, why),
            );
        }
        Role::Plain | Role::Local => {}
        Role::Set => {
            rows.push(Row::new(Item::AddVariant).dim_if(locked, why));
            rows.push(Row::new(Item::SelectAllInstances));
        }
        Role::Main => {
            if cx.in_set {
                rows.push(Row::new(Item::AddVariant).dim_if(locked, why));
            }
            rows.push(Row::new(Item::DuplicateAsComponent).dim_if(locked, why));
            rows.push(Row::new(Item::SelectAllInstances));
        }
        Role::Instance => {
            match &cx.kit {
                Some((kit, waiting)) => {
                    rows.push(Row::new(Item::GoToMain).label(format!("Go to main in {kit}")));
                    if *waiting {
                        rows.push(Row::new(Item::AcceptKitUpdates).dim_if(locked, why));
                    }
                }
                None => rows.push(Row::new(Item::GoToMain)),
            }
            // A nested copy bound to a swap or visibility property is an instance
            // root whose property is its outer instance's (`overridden_property_and_root`).
            if let Some(prop) = &cx.property_reset {
                rows.push(
                    Row::new(Item::ResetProperty)
                        .label(format!("Reset {prop}"))
                        .dim_if(locked, why),
                );
            }
            if cx.drifted {
                rows.push(Row::new(Item::ResetInstance).dim_if(locked, why));
            }
            if cx.children_differ {
                rows.push(Row::new(Item::RestoreChildren).dim_if(locked, why));
            }
            rows.push(Row::new(Item::DetachInstance).dim_if(locked, why));
            if cx.kit.is_some() {
                rows.push(Row::new(Item::MakeLocal).dim_if(locked, why));
            }
        }
        Role::Member => {
            rows.push(Row::new(Item::GoToMain));
            if let Some(prop) = &cx.property_reset {
                rows.push(
                    Row::new(Item::ResetProperty)
                        .label(format!("Reset {prop}"))
                        .dim_if(locked, why),
                );
            }
            if cx.drifted {
                rows.push(
                    Row::new(Item::ResetChild)
                        .label(format!("Reset {}", cx.layer_name))
                        .dim_if(locked, why),
                );
            }
            if cx.children_differ {
                rows.push(Row::new(Item::RestoreChildren).dim_if(locked, why));
            }
        }
    }
    // *Ungroup* and *Flatten* on a group or boolean are promoted into the head
    // (§3) and must not appear twice.
    let promoted_ungroup = one && matches!(cx.kinds.first(), Some(Kind::Group | Kind::Boolean(_)));
    let promoted_flatten = one && matches!(cx.kinds.first(), Some(Kind::Boolean(_)));
    if !promoted_ungroup
        && cx
            .kinds
            .iter()
            .any(|k| matches!(k, Kind::Group | Kind::Boolean(_)))
    {
        rows.push(Row::new(Item::Ungroup).dim_if(locked, why));
    }
    // *Use as mask*: absent where the verb has no answer (all frames, which cannot
    // be a mask; inside a boolean, where operands are combined not drawn), dim
    // where it merely refuses. A frame beside a shape is masked content
    // (`build::mask_target` passes over the frame for the shape).
    if !cx.kinds.iter().all(|k| *k == Kind::Frame) && !st.in_boolean {
        rows.push(
            Row::new(Item::Mask)
                .checked(one && st.masked)
                .dim_if(cx.mask_refused.is_some(), cx.mask_refused.unwrap_or(""))
                .dim_if(locked, why),
        );
    }
    // `Path` only (see `Item::EvenOdd`), one layer at a time: the tick reports this shape's rule.
    if one && matches!(cx.kinds.first(), Some(Kind::Path)) {
        rows.push(
            Row::new(Item::EvenOdd)
                .checked(st.even_odd)
                .dim_if(locked, why),
        );
    }
    // Checkable while the target is a boolean, a plain verb over two or more layers otherwise.
    let on_boolean = one && matches!(cx.kinds.first(), Some(Kind::Boolean(_)));
    if on_boolean || cx.selection.len() >= 2 {
        let current = match cx.kinds.first() {
            Some(Kind::Boolean(op)) if on_boolean => Some(*op),
            _ => None,
        };
        for op in [
            BoolOp::Union,
            BoolOp::Subtract,
            BoolOp::Intersect,
            BoolOp::Exclude,
        ] {
            let mut row = Row::new(Item::Boolean(op)).dim_if(locked, why);
            if let Some(current) = current {
                row = row.checked(current == op);
            }
            rows.push(row);
        }
    }
    if !promoted_flatten && (cx.selection.len() >= 2 || on_boolean) {
        rows.push(Row::new(Item::Flatten).dim_if(locked, why));
    }
    // Outline rows sit in Structure, not the text head: §3 groups by what a verb
    // does. *Convert to path* and *Outline shape* are mutually exclusive by kind,
    // and neither is Flatten's (a boolean or set) since `one` excludes a set.
    if one && cx.kinds.first() == Some(&Kind::Text) {
        rows.push(
            Row::new(Item::OutlineText)
                .dim_if(!cx.text_outlineable, "This text has no glyphs to convert")
                // Locked last, so it wins when both apply.
                .dim_if(locked, why),
        );
    }
    // Neither rail row is ever dim but for the lock; Flip is above Detach.
    if one && cx.on_a_rail {
        rows.push(
            Row::new(Item::FlipTextPath)
                .checked(cx.rail_flipped)
                .dim_if(locked, why),
        );
        rows.push(Row::new(Item::DetachTextPath).dim_if(locked, why));
    }
    if one && matches!(cx.kinds.first(), Some(Kind::Shape | Kind::Path)) {
        rows.push(
            Row::new(Item::OutlineShape)
                .dim_if(!st.outlineable, "This path is already an outline")
                // Locked last, so "unlock it" wins; `dim_if` overwrites.
                .dim_if(locked, why),
        );
    }
    if one
        && matches!(
            cx.kinds.first(),
            Some(Kind::Shape | Kind::Path | Kind::Boolean(_))
        )
    {
        rows.push(
            Row::new(Item::OutlineStroke)
                .dim_if(!st.stroke_outlineable, "This layer has no stroke")
                .dim_if(locked, why),
        );
    }
    if cx.kinds == [Kind::Path, Kind::Path] {
        rows.push(
            Row::new(Item::JoinPaths)
                .dim_if(
                    cx.join_paths_refused.is_some(),
                    cx.join_paths_refused.unwrap_or_default(),
                )
                .dim_if(locked, why),
        );
    }

    // --- order ------------------------------------------------------------
    // Never dimmed for being already frontmost: `z_order` reports an empty transaction as an info line.
    rows.extend(
        [
            build::ZMove::Front,
            build::ZMove::Forward,
            build::ZMove::Backward,
            build::ZMove::Back,
        ]
        .into_iter()
        .map(|mv| Row::new(Item::Restack(mv)).dim_if(locked, why)),
    );

    // --- transform --------------------------------------------------------
    rows.extend([
        Row::new(Item::Flip(build::Axis::X)).dim_if(locked, why),
        Row::new(Item::Flip(build::Axis::Y)).dim_if(locked, why),
    ]);
    // Omitted at default (§3's exception), not dim.
    if st.pivot_moved && one {
        rows.push(Row::new(Item::ResetOrigin).dim_if(locked, why));
    }

    // --- state ------------------------------------------------------------
    // The labels say what the action does over the whole selection.
    rows.extend([
        Row::new(Item::ToggleHidden).label(if st.all_hidden { "Show" } else { "Hide" }),
        Row::new(Item::ToggleLocked).label(if st.all_locked { "Unlock" } else { "Lock" }),
        // One layer only, and not in focus mode, which hides the panel holding
        // the field. Never dimmed by the lock: a name is not the layer's artwork.
        Row::new(Item::Rename)
            .dim_if(!one, "Pick one layer to rename it")
            .dim_if(cx.focus, "Leave focus mode to rename"),
    ]);

    // --- navigate ---------------------------------------------------------
    rows.push(Row::new(Item::ZoomSelection));

    // Export group: never dim, since no row changes the layer.
    rows.extend([
        Row::new(Item::CopyAsSvg),
        Row::new(Item::CopyAsCss),
        Row::new(Item::CopyAsPng(1)),
        Row::new(Item::CopyAsPng(2)),
    ]);

    rows
}

/// §5 — the rows above group 2, per kind.
///
/// A multi-selection has a head only when every member is the same kind (§3); the
/// exception is *Make this the key layer*, which is about the layer under the pointer.
fn head_rows(cx: &Context<'_>) -> Vec<Row> {
    let st = cx.state;
    let one = cx.selection.len() == 1;
    let locked = st.locked_within();
    let why = if st.locked {
        "This layer is locked"
    } else {
        "A group containing this layer is locked"
    };
    let mut rows = Vec::new();

    let all_are = |want: Kind| !cx.kinds.is_empty() && cx.kinds.iter().all(|k| *k == want);

    if one {
        match cx.kinds.first() {
            Some(Kind::Frame) => {
                // No *Background…* row: a frame's fill is its paint list, which the
                // inspector's Fill row already edits (§15 D800).
                rows.push(
                    Row::new(Item::ClipContent)
                        .checked(st.clips)
                        .dim_if(locked, why),
                );
            }
            Some(Kind::Group) => {
                rows.push(Row::new(Item::EnterGroup));
                rows.push(Row::new(Item::Ungroup).promote().dim_if(locked, why));
            }
            Some(Kind::Boolean(_)) => {
                rows.push(Row::new(Item::EnterBoolean));
                rows.push(Row::new(Item::Flatten).promote().dim_if(locked, why));
                rows.push(Row::new(Item::Ungroup).promote().dim_if(locked, why));
            }
            _ => {}
        }
    }

    // Composed in `double_click_pick`'s order (text, picture, points) so the menu
    // and the double-click agree on what "one level in" means.
    if all_are(Kind::Text) {
        rows.push(Row::new(Item::EditText).dim_if(locked, why));
        // Only over one layer: `text_sizing` is read off the hit node alone, so the
        // tick is a fact about one node, not the selection.
        if one && let Some(cell) = st.text_sizing {
            for i in 0..3u8 {
                rows.push(
                    Row::new(Item::TextSizing(i))
                        .checked(cell == i)
                        .dim_if(locked, why),
                );
            }
        }
    }
    if one && st.has_picture {
        // Dimmed by the lock because the mode itself refuses
        // (`OndinApp::begin_image_edit`), and a crop is a resize like *Original size*.
        rows.push(Row::new(Item::EditImage).dim_if(locked, why));
        rows.push(Row::new(Item::OriginalSize).dim_if(locked, why));
    }
    if one && all_are(Kind::Path) {
        rows.push(Row::new(Item::EditPoints).dim_if(locked, why));
    }

    // §5.3 / §5.8 — the two designations are one gesture (`Alt+Shift`+click)
    // under two names depending on where the layer sits.
    if st.in_boolean {
        rows.push(Row::new(Item::MakeBaseOperand).dim_if(locked, why));
    } else if cx.selection.len() >= 2 {
        rows.push(Row::new(Item::MakeKeyLayer));
    }

    rows
}

/// The five View switches §6.1 and §6.3 share, each carrying its own state.
/// All five stay live in focus mode, which hides none of what they switch.
fn view_rows(cx: &Context<'_>) -> Vec<Row> {
    [
        ViewSwitch::Rulers,
        ViewSwitch::Guides,
        ViewSwitch::GuideLock,
        ViewSwitch::Grid,
        ViewSwitch::LayoutGrid,
    ]
    .into_iter()
    .map(|sw| Row::new(Item::ViewSwitch(sw)).checked(cx.view.get(sw)))
    .collect()
}

/// The layer verbs a menu leaves out for some selections, each with what it
/// needs — the command palette's *Not available here* rows, shown only when typed
/// for. With nothing selected every one reads [`NEEDS_SELECTION`] instead.
pub const KIND_VERBS: &[(Item, &str)] = &[
    (Item::EnterGroup, "Needs one group"),
    (Item::EnterBoolean, "Needs one boolean"),
    (Item::MakeBaseOperand, "Needs a layer inside a boolean"),
    (Item::ClipContent, "Needs one frame"),
    (Item::EditText, "Needs a text layer"),
    (Item::EditPoints, "Needs one path"),
    (Item::EditImage, "Needs one layer with a picture"),
    (Item::OriginalSize, "Needs one layer with a picture"),
    (
        Item::CreateComponent,
        "Needs a layer that is not a component",
    ),
    (Item::DuplicateAsComponent, "Needs one main component"),
    (Item::SelectAllInstances, "Needs a main component or a set"),
    (Item::GoToMain, "Needs an instance"),
    (Item::DetachInstance, "Needs an instance"),
    (Item::CombineAsVariants, "Needs two or more main components"),
    (Item::AddVariant, "Needs a variant or a component set"),
    (Item::Ungroup, "Needs a group or a boolean"),
    (Item::Mask, "Needs a layer that is not a frame"),
    (Item::EvenOdd, "Needs one path"),
    (
        Item::Boolean(BoolOp::Union),
        "Needs two layers or a boolean",
    ),
    (
        Item::Boolean(BoolOp::Subtract),
        "Needs two layers or a boolean",
    ),
    (
        Item::Boolean(BoolOp::Intersect),
        "Needs two layers or a boolean",
    ),
    (
        Item::Boolean(BoolOp::Exclude),
        "Needs two layers or a boolean",
    ),
    (Item::Flatten, "Needs two layers or a boolean"),
    (Item::OutlineShape, "Needs one shape"),
    (Item::OutlineStroke, "Needs one stroked shape"),
    (Item::JoinPaths, "Needs two path layers"),
    (Item::OutlineText, "Needs one text layer"),
    (Item::DetachTextPath, "Needs text on a path"),
    (Item::FlipTextPath, "Needs text on a path"),
    (Item::Cut, NEEDS_SELECTION),
    (Item::Copy, NEEDS_SELECTION),
    (Item::Duplicate, NEEDS_SELECTION),
    (Item::Delete, NEEDS_SELECTION),
    (Item::CopyProperties, NEEDS_SELECTION),
    (Item::PasteProperties, NEEDS_SELECTION),
    (Item::Group, NEEDS_SELECTION),
    (Item::FrameSelection, NEEDS_SELECTION),
    (Item::Restack(build::ZMove::Front), NEEDS_SELECTION),
    (Item::Restack(build::ZMove::Forward), NEEDS_SELECTION),
    (Item::Restack(build::ZMove::Backward), NEEDS_SELECTION),
    (Item::Restack(build::ZMove::Back), NEEDS_SELECTION),
    (Item::Flip(build::Axis::X), NEEDS_SELECTION),
    (Item::Flip(build::Axis::Y), NEEDS_SELECTION),
    (Item::ToggleHidden, NEEDS_SELECTION),
    (Item::ToggleLocked, NEEDS_SELECTION),
    (Item::Rename, NEEDS_SELECTION),
    (Item::ZoomSelection, NEEDS_SELECTION),
    (Item::CopyAsSvg, NEEDS_SELECTION),
    (Item::CopyAsCss, NEEDS_SELECTION),
    (Item::CopyAsPng(1), NEEDS_SELECTION),
    (Item::CopyAsPng(2), NEEDS_SELECTION),
];

/// Why a layer verb is not offered with nothing selected.
pub const NEEDS_SELECTION: &str = "Select a layer first";

/// The height a menu of these groups will paint at, in points.
///
/// Computed rather than measured, because [`crate::ui::menu_place`] needs the size
/// before the `Area` is shown. Exact, since rows and separators are fixed heights.
pub fn height(groups: &[Vec<Row>], row_h: f32, gap: f32, sep_h: f32, pad: f32) -> f32 {
    let rows: usize = groups.iter().map(|g| g.len()).sum();
    let seps = groups.len().saturating_sub(1);
    let gaps = (rows + seps).saturating_sub(1);
    // The frame's stroke grows its outer rect, so the border counts.
    crate::ui::MENU_BORDER * 2.0
        + pad * 2.0
        + rows as f32 * row_h
        + seps as f32 * sep_h
        + gaps as f32 * gap
}

/// One keypress an open menu answers while it holds the keyboard
/// (`docs/context-menus.md` §8, R3).
///
/// A value rather than a call: the keys must be consumed before any panel runs,
/// but the row a `↓` lands on is not known until `build` has run at the bottom of
/// the frame. No type-ahead in v1 (§8).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Nav {
    Move(Step),
    /// `Enter` — run the highlighted row. A no-op with nothing highlighted, so a
    /// stray `Enter` on an accidental menu commits nothing.
    Activate,
}

/// Where a highlight goes (`docs/context-menus.md` §8).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    /// `↓` — the next reachable row, wrapping. From nothing, the first.
    Next,
    /// `↑` — the previous reachable row, wrapping. From nothing, the last.
    Prev,
    First,
    Last,
}

/// Move a keyboard highlight through a built menu.
///
/// The highlight is an [`Item`], not an index: the menu is rebuilt every frame, and
/// an item survives the rebuild and is unique within a menu
/// (`no_menu_offers_the_same_verb_twice`). Only rows a press could run are
/// reachable, since a keyboard cannot hover to read why a dim row is dim; `End` is
/// the last live row.
///
/// Returns `None` when the menu has no live row at all.
pub fn navigate(groups: &[Vec<Row>], from: Option<Item>, step: Step) -> Option<Item> {
    let live: Vec<Item> = groups
        .iter()
        .flatten()
        .filter(|r| r.enabled)
        .map(|r| r.item)
        .collect();
    let last = live.len().checked_sub(1)?;
    let at = from.and_then(|item| live.iter().position(|i| *i == item));
    Some(match (step, at) {
        (Step::First, _) => live[0],
        (Step::Last, _) => live[last],
        // A highlight the rebuild left behind restarts at the end the key was heading for.
        (Step::Next, None) => live[0],
        (Step::Prev, None) => live[last],
        (Step::Next, Some(i)) => live[if i == last { 0 } else { i + 1 }],
        (Step::Prev, Some(i)) => live[if i == 0 { last } else { i - 1 }],
    })
}

// --- the app half: opening, drawing, dispatching --------------------------

use crate::app::{ContextMenu, OndinApp, TopMenu};
use crate::panels::{PaintSlot, paint};
use crate::tools::{self, Tool};
use eframe::egui;
use ondin_core::kurbo::Point;
use ondin_core::{Operation, Transaction};

/// The card's painted width, border included; `crate::ui::menu_inner_w` takes the
/// border and padding back off for the rows.
const MENU_W: f32 = 210.0;
/// The row height, shared with the View menu.
const ROW_H: f32 = crate::ui::MENU_ITEM_H;
/// `menu_frame`'s inner margin, the same 5pt the dropdowns use.
const MENU_PAD: f32 = 5.0;
/// One separator as `ui::menu_sep` allocates it; agreeing with it keeps [`height`] exact.
const SEP_H: f32 = crate::ui::MENU_SEP_H;

/// Take this frame's menu keypress, if there is one (`docs/context-menus.md` §8).
///
/// Called from R3's gate at the top of the frame, and it consumes: the keys must
/// leave `Events` before any panel is drawn, so egui's widgets cannot read an `↑`
/// the menu is using. Bare keys only (`Modifiers::NONE`); one press per frame.
pub(crate) fn read_nav(ctx: &egui::Context) -> Option<Nav> {
    let none = egui::Modifiers::NONE;
    ctx.input_mut(|i| {
        [
            (egui::Key::ArrowDown, Nav::Move(Step::Next)),
            (egui::Key::ArrowUp, Nav::Move(Step::Prev)),
            (egui::Key::Home, Nav::Move(Step::First)),
            (egui::Key::End, Nav::Move(Step::Last)),
            (egui::Key::Enter, Nav::Activate),
        ]
        .into_iter()
        .find(|(key, _)| i.consume_key(none, *key))
        .map(|(_, nav)| nav)
    })
}

impl OndinApp {
    /// Open a context menu on `target` (`docs/context-menus.md` §0).
    ///
    /// Called from a region's `secondary_clicked()`, which egui delivers on the
    /// release (R2).
    ///
    /// A press that cancelled a gesture spends the click (R1): one right-click
    /// cancels or opens a menu, never both. The menu must open on the release,
    /// otherwise that release would land inside it and pick a row. Focus mode
    /// does not refuse a menu; R3 gives an open menu the keyboard before `escape`.
    pub(crate) fn open_context_menu(
        &mut self,
        ctx: &egui::Context,
        target: Target,
        world: Option<Point>,
    ) {
        if self.gesture_cancelled {
            return;
        }
        let at = self
            .secondary_press
            .or_else(|| ctx.pointer_interact_pos())
            .unwrap_or_default();
        // R4: one slot across every floating thing drawn over the canvas. Spelled
        // out rather than routed through `OndinApp::a_popover_is_open`, since the
        // two sets differ by image editing's card.
        self.open_menu = TopMenu::None;
        self.picker = None;
        self.stroke_menu = None;
        self.effect_menu = None;
        self.export_menu = false;
        self.export_row_open = None;
        // Image editing's card is not cleared: it is the mode's own chrome, and
        // clearing it would half-leave the mode.
        self.type_menu = None;
        self.palette = None;
        self.context_menu = Some(self.menu_snapshot(at, target, world));
    }

    /// The state a menu open on `target` reads once rather than per frame: the
    /// clipboards and the outline question. The command palette takes one too.
    pub(crate) fn menu_snapshot(
        &self,
        at: egui::Pos2,
        target: Target,
        world: Option<Point>,
    ) -> ContextMenu {
        let system_text = crate::app::system_clipboard_text();
        ContextMenu {
            at,
            target,
            world,
            just_opened: true,
            // One OS clipboard text read per open, shared by both answers below.
            system_text: system_text.is_some(),
            // Asked once and answered with a `bool`; see `system_clipboard_has_image`.
            system_image: crate::app::system_clipboard_has_image(),
            payload_current: self.owns_the_clipboard(system_text.as_deref()),
            // Asked at open rather than per frame, and only of a layer target.
            text_outlineable: match target {
                Target::Layer { id, .. } => build::can_outline_text(&self.session.resolved, id),
                _ => false,
            },
            // A fresh menu belongs to the pointer; the first `↓` takes it (§8).
            highlight: None,
        }
    }

    /// Draw the open menu, and close it the two ways every menu closes.
    ///
    /// Runs after every door, so a right-click that replaced the menu has already
    /// written the slot before the click-away test. `nav` is this frame's keypress,
    /// taken out of `Events` by [`read_nav`] and applied here because the row a `↓`
    /// lands on does not exist until `build` has run.
    pub(crate) fn context_menu_ui(&mut self, ctx: &egui::Context, nav: Option<Nav>) {
        let Some(menu) = self.context_menu.as_ref() else {
            return;
        };
        let (at, target, world, just_opened) = (menu.at, menu.target, menu.world, menu.just_opened);
        let mut highlight = menu.highlight;

        // Rebuilt from live state every frame so checkable rows update; the
        // clipboard is the snapshot `menu` carries.
        let kinds: Vec<Kind> = self
            .session
            .selection
            .ids()
            .iter()
            .filter_map(|id| self.session.doc.get(*id).map(|n| Kind::of(n.kind())))
            .collect();
        let groups = build(&self.menu_context(menu, &kinds));
        if groups.is_empty() {
            self.context_menu = None;
            return;
        }
        // The card paints exactly `MENU_W`; the height adds its border in [`height`].
        let size = egui::vec2(MENU_W, height(&groups, ROW_H, 1.0, SEP_H, MENU_PAD));
        let pos = crate::ui::menu_place(ctx, at, size);

        let mut chosen = None;

        // The keyboard half (§8): an arrow moves the highlight, `Enter` runs it, a
        // pointer move gives it back to the pointer. The keyboard wins a frame
        // carrying both.
        let moved = ctx.input(|i| {
            i.events
                .iter()
                .any(|e| matches!(e, egui::Event::PointerMoved(_)))
        });
        match nav {
            Some(Nav::Move(step)) => highlight = navigate(&groups, highlight, step),
            // Re-checked against this frame's build in case the row went dim.
            Some(Nav::Activate) => {
                chosen = groups
                    .iter()
                    .flatten()
                    .find(|r| Some(r.item) == highlight && r.enabled)
                    .map(|r| r.item);
            }
            None if moved => highlight = None,
            None => {}
        }
        if let Some(m) = self.context_menu.as_mut() {
            m.highlight = highlight;
        }
        let area = egui::Area::new(egui::Id::new("context-menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                crate::ui::menu_frame(MENU_PAD).show(ui, |ui| {
                    ui.set_width(crate::ui::menu_inner_w(MENU_W, MENU_PAD));
                    ui.spacing_mut().item_spacing.y = 1.0;
                    for (i, group) in groups.iter().enumerate() {
                        if i > 0 {
                            crate::ui::menu_sep(ui);
                        }
                        for row in group {
                            let spec = row.item.spec();
                            let mut style = crate::ui::MenuRow::new(spec.glyph, &row.label)
                                .accel(spec.accel)
                                .danger(spec.danger)
                                .filled_hexagon(spec.filled_hexagon)
                                .enabled(row.enabled);
                            if let Some(on) = row.checked {
                                style = style.checked(on);
                            }
                            // Set on every row once on any: a row told `false` ignores
                            // the pointer, so no hover paints beside the keyboard's.
                            if let Some(hl) = highlight {
                                style = style.highlight(row.item == hl);
                            }
                            let resp = crate::ui::menu_row(ui, style, ROW_H);
                            // Hung off the row's own response: a `disable()`d `Ui` or
                            // `Ui::scope` is silently dead (`inspector::menu_action`).
                            let resp = match row.why {
                                Some(why) => resp.on_hover_text(why),
                                None => resp,
                            };
                            if resp.clicked() {
                                chosen = Some(row.item);
                            }
                        }
                    }
                });
            })
            .response;

        if let Some(item) = chosen {
            self.context_menu = None;
            self.perform_menu_item(ctx, item, target, world);
            ctx.request_repaint();
            return;
        }

        // Not on the frame that opened it; see [`ContextMenu::just_opened`].
        if !just_opened && ctx.input(|i| i.pointer.any_click()) && !area.contains_pointer() {
            self.context_menu = None;
        } else if let Some(menu) = self.context_menu.as_mut() {
            menu.just_opened = false;
        }
    }

    /// The rows the open menu draws this frame, for a test that wants the menu a
    /// user sees rather than a synthetic `Context`.
    #[cfg(test)]
    pub(crate) fn open_menu_rows(&self) -> Vec<Vec<Row>> {
        let menu = self.context_menu.as_ref().expect("a menu is open");
        let kinds: Vec<Kind> = self
            .session
            .selection
            .ids()
            .iter()
            .filter_map(|id| self.session.doc.get(*id).map(|n| Kind::of(n.kind())))
            .collect();
        build(&self.menu_context(menu, &kinds))
    }

    /// Everything the registry needs to know about the app, read once.
    ///
    /// `open` is the snapshot taken when the menu was opened, not live state: the
    /// clipboard questions are all in it (see [`ContextMenu::system_text`] and
    /// [`ContextMenu::system_image`]).
    pub(crate) fn menu_context<'a>(&'a self, open: &ContextMenu, kinds: &'a [Kind]) -> Context<'a> {
        let target = open.target;
        let state = match target {
            Target::Layer { id, .. } => self.layer_menu_state(id),
            _ => LayerState::default(),
        };
        Context {
            target,
            selection: self.session.selection.ids(),
            // Three payloads in the order both paste paths try them: a picture, the
            // app's own subtrees while the OS clipboard still describes them, then
            // text. A copy from another `ondin` window arrives as text (`io::clip`),
            // so `system_text` already covers it; a non-text channel would break this.
            can_paste: open.system_image
                || (self.clipboard.is_some() || self.guide_clipboard.is_some())
                    && open.payload_current
                || open.system_text,
            can_replace: self.clipboard.is_some() && open.payload_current,
            kinds,
            state,
            any_guides: !self.session.doc.guides().is_empty(),
            view: ViewState {
                rulers: self.view_switch(ViewSwitch::Rulers),
                guides: self.view_switch(ViewSwitch::Guides),
                guide_lock: self.view_switch(ViewSwitch::GuideLock),
                grid: self.view_switch(ViewSwitch::Grid),
                layout_grid: self.view_switch(ViewSwitch::LayoutGrid),
            },
            focus: self.focus,
            segment_selected: self
                .edited_path()
                .is_some_and(|id| self.points.segments(id).next().is_some()),
            // The open's world point, not this frame's pointer, which would let the
            // row go dark as the hand travels down the menu.
            on_segment: open.world.is_some_and(|p| self.segment_at(p).is_some()),
            can_join: self.join_subject().is_some(),
            join_paths_refused: self.join_layers_subject().err(),
            can_copy_props: self.properties_source().is_some(),
            has_key: self.session.selection.key().is_some(),
            has_props: self.property_clipboard.is_some(),
            text_outlineable: open.text_outlineable,
            on_a_rail: self
                .session
                .selection
                .single()
                .and_then(|id| self.session.display_node(id))
                .is_some_and(|n| {
                    matches!(
                        n.kind(),
                        ondin_core::NodeKind::Text {
                            on_path: Some(_),
                            ..
                        }
                    )
                }),
            rail_flipped: self
                .session
                .selection
                .single()
                .and_then(|id| self.session.display_node(id))
                .is_some_and(|n| {
                    matches!(
                        n.kind(),
                        ondin_core::NodeKind::Text {
                            on_path_flip: true,
                            ..
                        }
                    )
                }),
            system_text: open.system_text,
            text_selection: self.text.as_ref().is_some_and(|s| s.editor.has_selection()),
            text_empty: self
                .text
                .as_ref()
                .is_some_and(|s| s.editor.content().is_empty()),
            can_frame: build::can_frame(&self.session.doc, self.session.selection.ids()),
            mask_refused: self.mask_refusal(),
            role: self.component_role(),
            // From the Component card's cache when current, else computed
            // (`OndinApp::drift_of` wants `&mut`).
            drifted: self.session.selection.single().is_some_and(|id| {
                (self.drift_cache.0 == self.session.revision())
                    .then(|| self.drift_cache.1.get(&id).copied())
                    .flatten()
                    .unwrap_or_else(|| ondin_core::reset::drift(&self.session.doc, id))
                    .any()
            }),
            // The layers panel's cache, on the row's fold, from either door (`row_folded`).
            children_differ: self
                .session
                .selection
                .single()
                .is_some_and(|id| self.child_drift_read(id).any()),
            layer_name: self
                .session
                .selection
                .single()
                .and_then(|id| self.session.doc.get(id))
                .map(|n| n.name().to_string())
                .unwrap_or_default(),
            kit: self
                .session
                .selection
                .single()
                .filter(|id| {
                    ondin_core::component::instance_root(&self.session.doc, *id) == Some(*id)
                })
                .and_then(|id| ondin_core::component::main_of(&self.session.doc, id))
                .and_then(|m| self.kit_line(m))
                .map(|k| (k.kit_name, k.waiting.is_some())),
            combinable: self.combinable_mains().is_some(),
            combine_refused: self.combine_refusal(),
            create_refused: self.create_component_refusal().map(create_refusal),
            in_set: self
                .session
                .selection
                .single()
                .is_some_and(|id| ondin_core::variant::set_of(&self.session.doc, id).is_some()),
            property_reset: self.overridden_property_of_selection().map(|p| p.name),
            holds_main: self.session.selection.ids().iter().any(|id| {
                self.session
                    .doc
                    .get(*id)
                    .is_some_and(|n| n.component() || n.set().is_some())
            }),
            // A tree walk per frame that stops at the first layer with a spec.
            any_exports: self.document_exports_anything(),
            any_frames: !self.exported_frames().is_empty(),
        }
    }

    /// The parts of one layer that decide a row's label or its presence.
    fn layer_menu_state(&self, id: NodeId) -> LayerState {
        let Some(node) = self.session.doc.get(id) else {
            return LayerState::default();
        };
        let picture = tools::croppable_fill(node.paint());
        LayerState {
            all_hidden: self.hide_switch_shows(),
            all_locked: !self.lock_switch_locks(),
            locked: node.locked(),
            // Asked of the parent so this node's own flag is not folded in twice.
            locked_by_ancestor: node
                .parent()
                .is_some_and(|p| ondin_core::is_effectively_locked(&self.session.doc, p)),
            clips: node.clip(),
            masked: node.mask()
                || node
                    .children()
                    .iter()
                    .any(|c| self.session.doc.get(*c).is_some_and(|n| n.mask())),
            even_odd: node.fill_rule() == ondin_core::FillRule::EvenOdd,
            has_picture: picture.is_some(),
            pivot_moved: node.pivot().is_some(),
            in_boolean: node.parent().is_some_and(|p| {
                matches!(
                    self.session.doc.get(p).map(|n| n.kind()),
                    Some(NodeKind::Boolean { .. })
                )
            }),
            // Core's own predicate, so the row never offers what `build::outline` refuses.
            outlineable: build::can_outline(node.kind()),
            stroke_outlineable: build::can_outline_stroke(&self.session.doc, id),
            // The display node, so a live text session's uncommitted mode is what
            // the ticks report, as `set_text_sizing` reads it.
            text_sizing: match self.session.display_node(id).map(|n| n.kind()) {
                Some(NodeKind::Text { sizing, .. }) => {
                    Some(u8::try_from(sizing.cell()).unwrap_or(0))
                }
                _ => None,
            },
        }
    }

    /// Run one row. Anything with a chord goes straight to `dispatch`, so row and
    /// key cannot diverge; the rest are about where the menu was opened or a target
    /// the keymap cannot name.
    pub(crate) fn perform_menu_item(
        &mut self,
        ctx: &egui::Context,
        item: Item,
        target: Target,
        world: Option<Point>,
    ) {
        use crate::input::Action;
        let hit = match target {
            Target::Layer { id, .. } => Some(id),
            _ => None,
        };
        let in_text = matches!(target, Target::TextSession);
        match item {
            // --- the text session's four, which act on the editor ----------
            // First and guarded: in a session these chords are the editor's
            // (`input::resolve` resolves nothing in `Mode::TextInsert`), and
            // without the guard they fall through to the layer arms.
            Item::Cut if in_text => self.cut_text_selection(ctx),
            Item::Copy if in_text => self.copy_text_selection(ctx),
            Item::PasteHere if in_text => self.paste_text_selection(),
            Item::SelectAll if in_text => self.select_all_text(),

            // --- straight to the keymap's own verb -------------------------
            Item::Cut => self.dispatch(ctx, Action::Cut),
            Item::Copy => self.dispatch(ctx, Action::Copy),
            Item::Duplicate => self.dispatch(ctx, Action::Duplicate),
            // `Action::Delete` already resolves guide, point or layer selection.
            Item::Delete | Item::DeleteGuide | Item::DeletePoints => {
                self.dispatch(ctx, Action::Delete)
            }
            Item::Group => self.dispatch(ctx, Action::Group),
            Item::Mask => self.dispatch(ctx, Action::Mask),
            // The target, not the selection: a right-click on an unselected path
            // must act on that path.
            Item::EvenOdd => {
                if let Some(id) = hit {
                    let rule = match self.session.doc.get(id).map(|n| n.fill_rule()) {
                        Some(ondin_core::FillRule::EvenOdd) => ondin_core::FillRule::NonZero,
                        _ => ondin_core::FillRule::EvenOdd,
                    };
                    self.commit_edit(Transaction(vec![Operation::SetFillRule { id, rule }]));
                }
            }
            // The selection, not the hit, as *Group selection* does.
            Item::FrameSelection => self.dispatch(ctx, Action::FrameSelection),
            Item::CreateComponent => self.dispatch(ctx, Action::CreateComponent),
            Item::DetachInstance => self.dispatch(ctx, Action::DetachInstance),
            // No chord, so no `Action`.
            Item::DuplicateAsComponent => self.duplicate_as_component(),
            Item::SelectAllInstances => self.select_all_instances(),
            // On a kit instance the row reads *Go to main in Atlas* and goes there.
            Item::GoToMain => {
                let kit = self
                    .session
                    .selection
                    .single()
                    .and_then(|id| ondin_core::component::main_of(&self.session.doc, id))
                    .is_some_and(|m| self.session.doc.in_vault(m));
                if kit {
                    self.go_to_kit_main();
                } else {
                    self.go_to_main();
                }
            }
            Item::AcceptKitUpdates | Item::MakeLocal => {
                let line = self
                    .session
                    .selection
                    .single()
                    .and_then(|id| ondin_core::component::main_of(&self.session.doc, id))
                    .and_then(|m| self.kit_line(m));
                if let Some(k) = line {
                    if item == Item::MakeLocal {
                        self.making_local = Some(k.top);
                    } else {
                        self.accept_kit_updates(Some(&k.kit));
                    }
                }
            }
            // Offered on one selected layer, which `reset_selection` scopes to.
            Item::ResetInstance | Item::ResetChild => {
                self.reset_selection(ondin_core::reset::Kind::All)
            }
            Item::RestoreChildren => self.restore_selected_children(),
            Item::ResetProperty => self.reset_selected_property(),
            Item::CombineAsVariants => self.combine_as_variants(),
            Item::AddVariant => self.add_variant(),
            Item::Ungroup => self.dispatch(ctx, Action::Ungroup),
            Item::Boolean(op) => self.dispatch(ctx, Action::Boolean(op)),
            Item::Flatten => self.dispatch(ctx, Action::Flatten),
            // No chord, so no `Action`. `hit` rather than the selection (C3); the
            // verb re-checks its own kind, so a stale row refuses.
            Item::OutlineText => {
                if let Some(id) = hit {
                    self.outline_text(id);
                }
            }
            // The selection, not `hit`: the pointer can be over only one of the two
            // layers a rail needs. Both verbs re-derive their subjects.
            Item::DetachTextPath => self.detach_text_path(),
            Item::FlipTextPath => self.flip_text_path(),
            // The panel's own verb, which re-reads the node's kind and cell.
            Item::TextSizing(i) => {
                if let Some(id) = hit {
                    self.set_text_sizing(id, usize::from(i));
                }
            }
            Item::OutlineShape => {
                if let Some(id) = hit {
                    self.outline_shape(id);
                }
            }
            Item::OutlineStroke => {
                if let Some(id) = hit {
                    self.outline_stroke(id);
                }
            }
            Item::JoinPaths => self.join_layers(),
            Item::Restack(mv) => self.dispatch(ctx, Action::Restack(mv)),
            Item::Flip(axis) => self.dispatch(ctx, Action::Flip(axis)),
            Item::ToggleHidden => self.dispatch(ctx, Action::ToggleHidden),
            Item::ToggleLocked => self.dispatch(ctx, Action::ToggleLocked),
            Item::Rename => self.dispatch(ctx, Action::Rename),
            Item::ZoomSelection => self.dispatch(ctx, Action::ZoomSelection),
            Item::ZoomFit => self.dispatch(ctx, Action::ZoomFit),
            Item::SelectAll => self.dispatch(ctx, Action::SelectAll),
            Item::ViewSwitch(sw) => self.dispatch(ctx, Action::ToggleView(sw)),
            // No chord, so straight to the canvas verb rather than an `Action`.
            Item::SetSmooth(smooth) => self.set_point_smoothness(smooth),
            // The stored open position: `insert_point` needs a point on the ink and
            // a keyboard has none. `add_point_at` re-asks `segment_at`.
            Item::AddPointHere => {
                if let Some(p) = world {
                    self.add_point_at(p);
                }
            }
            // No chord; the verb re-asks its own subject.
            Item::Join => self.join_points(),
            Item::CopyProperties => self.dispatch(ctx, Action::CopyProperties),
            Item::PasteProperties => self.dispatch(ctx, Action::PasteProperties),
            Item::ExportAll => self.dispatch(ctx, Action::ExportAll),
            Item::CopyAsSvg => self.copy_as_svg(ctx),
            Item::CopyAsCss => self.copy_as_css(ctx),
            Item::CopyAsPng(scale) => self.copy_as_png(scale),
            Item::ExportFrames => self.export_frames(),
            Item::ReverseSubpaths => self.dispatch(ctx, Action::ReverseSubpaths),

            // --- the ones a keyboard cannot express ------------------------
            // Paste at the pointer (`OndinApp::paste_at`); the chord is deliberately
            // position-free. The panel door means a slot (a sibling above the clicked
            // row), so the match is on the target, not on `world`, which is `None` there.
            Item::PasteHere => match target {
                Target::Layer {
                    id,
                    door: Door::Panel,
                } => self.paste_beside(id),
                _ => match world {
                    Some(p) => self.paste_at(p),
                    None => self.dispatch(ctx, Action::Paste),
                },
            },
            // "In place" does not depend on where the menu was opened.
            Item::PasteInPlace => self.dispatch(ctx, Action::PasteInPlace),
            Item::PasteToReplace => self.dispatch(ctx, Action::PasteToReplace),
            // The same two steps as `double_click_pick`'s group arm; `Enter` is the
            // second door onto `enter_container`.
            Item::EnterGroup | Item::EnterBoolean => {
                if let Some(id) = hit {
                    self.enter_container(id, world);
                }
            }
            // Inside a boolean the key is child order (a reorder to index 0);
            // elsewhere a designation on the selection (`designate_key`).
            Item::MakeBaseOperand | Item::MakeKeyLayer => {
                if let Some(id) = hit {
                    self.designate_key(id);
                }
            }
            Item::ClipContent => {
                if let Some(id) = hit
                    && let Some(clip) = self.session.doc.get(id).map(|n| !n.clip())
                    && self
                        .session
                        .commit(Transaction(vec![Operation::SetClip { id, clip }]))
                {
                    self.session
                        .info(if clip { "Clipping" } else { "Not clipping" });
                }
            }
            // "Not moved": a state the model can say and a resolved pivot cannot.
            Item::ResetOrigin => {
                let ops: Vec<Operation> = self
                    .session
                    .selection
                    .ids()
                    .iter()
                    .map(|id| Operation::SetPivot {
                        id: *id,
                        pivot: None,
                    })
                    .collect();
                if self.session.commit(Transaction(ops)) {
                    self.session.info("Origin reset");
                }
            }
            Item::EditText => {
                if let Some(id) = hit {
                    // `world` straight through: the panel door has no caret point and
                    // selects the whole string, as `Enter` does.
                    self.begin_edit_text(Some(id), world);
                }
            }
            Item::EditPoints => {
                if let Some(id) = hit {
                    self.session.selection.set_one(id);
                    self.points.clear();
                    self.choose_tool(Tool::Node);
                }
            }
            // `begin_image_edit` is shared with the other doors.
            Item::EditImage => {
                if let Some(id) = hit {
                    self.begin_image_edit(id);
                }
            }
            // The box the picture fills at one unit per pixel; a quarter turn swaps it.
            Item::OriginalSize => {
                if let Some((id, slot, brush)) = hit.and_then(|id| self.picture_slot(id))
                    && let Some((px, py)) = self.image_pixels(slot, &brush)
                    && let Some(img) = paint::image_of(&brush)
                {
                    let (w, h) = img.oriented_size(px, py);
                    self.resize_to_original(id, w, h);
                }
            }
            // `build::guides_of` covers a subtree; this is the whole document.
            Item::ClearGuides => {
                let ops: Vec<Operation> = self
                    .session
                    .doc
                    .guides()
                    .iter()
                    .map(|g| Operation::RemoveGuide { id: g.id })
                    .collect();
                let n = ops.len();
                if self.session.commit(Transaction(ops)) {
                    self.session.selection.clear_guides();
                    self.session.info(format!("Cleared {n} guide(s)"));
                }
            }
            Item::ShowCard(title) => self.reveal_card = Some(title),
            // The app verbs: each is its chord's `Action`.
            Item::Undo
            | Item::Redo
            | Item::Save
            | Item::NewDocument
            | Item::CloseDocument
            | Item::Library
            | Item::Settings
            | Item::ExportCode(_)
            | Item::Present
            | Item::Tool(_)
            | Item::PlaceImage
            | Item::ZoomIn
            | Item::ZoomOut
            | Item::ZoomReset
            | Item::Align(..)
            | Item::Distribute(_)
            | Item::InsertPicker
            | Item::VariablesBrowser
            | Item::PublishKit
            | Item::MakeKit
            | Item::KitsUsed
            | Item::StopSharingKit
            | Item::ShowInLibrary
            | Item::VersionHistory
            | Item::RotateQuarter(_)
            | Item::SelectSame(_)
            | Item::SelectInverse
            | Item::TextChord(_)
            | Item::Palette
            | Item::Cheatsheet => {
                if let Some(action) = item.action() {
                    self.dispatch(ctx, action);
                }
            }
        }
        ctx.request_repaint();
    }

    /// The first visible image fill of `id`, as *Original size* needs it, asked
    /// through `croppable_fill` like image editing and `double_click_pick`.
    fn picture_slot(&self, id: NodeId) -> Option<(NodeId, PaintSlot, ondin_core::Brush)> {
        let node = self.session.doc.get(id)?;
        let i = tools::croppable_fill(node.paint())?;
        Some((
            id,
            PaintSlot::Fill(i),
            // `croppable_fill` answers an image fill, which is never bound.
            node.paint().fills.get(i)?.brush.literal()?.clone(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(seq: u64) -> NodeId {
        NodeId { actor: 1, seq }
    }

    /// R4: opening a context menu clears every floating thing except image
    /// editing's card, the one member by which this set and
    /// `OndinApp::a_popover_is_open` differ.
    #[test]
    fn opening_a_context_menu_clears_every_floating_thing() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = crate::app::OndinApp::headless(&ctx);
        app.open_menu = crate::app::TopMenu::Zoom;
        app.stroke_menu = Some(0);
        app.effect_menu = Some((id(2), 0));
        app.export_menu = true;
        app.export_row_open = Some(0);
        app.tool = crate::tools::Tool::ImageEdit;

        app.open_context_menu(&ctx, Target::Canvas, None);

        assert!(app.context_menu.is_some(), "the menu opened");
        assert_eq!(app.open_menu, crate::app::TopMenu::None);
        assert!(app.picker.is_none());
        assert!(app.stroke_menu.is_none());
        assert!(app.type_menu.is_none());
        assert!(app.effect_menu.is_none(), "the effects popover survived");
        assert!(!app.export_menu, "the Export menu survived");
        assert!(app.export_row_open.is_none(), "an Export row survived");
        assert_eq!(
            app.tool,
            crate::tools::Tool::ImageEdit,
            "and image editing is not a floating thing somebody opened"
        );
    }

    /// Focus mode opens a menu, and a cancelled gesture still refuses (R1).
    ///
    /// flip: restoring `|| self.focus` in `open_context_menu` → fails at "focus
    /// mode is a chrome switch"; deleting the whole condition → fails at "a press
    /// that cancelled a gesture still spends the click".
    #[test]
    fn focus_mode_opens_a_menu_and_a_cancelled_gesture_still_does_not() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);

        let mut app = crate::app::OndinApp::headless(&ctx);
        app.focus = true;
        app.open_context_menu(&ctx, Target::Canvas, None);
        assert!(
            app.context_menu.is_some(),
            "focus mode is a chrome switch, not a switch over what the canvas \
             answers — the menu is the canvas answering a click (§15 D750, D756)"
        );

        let mut app = crate::app::OndinApp::headless(&ctx);
        app.gesture_cancelled = true;
        app.open_context_menu(&ctx, Target::Canvas, None);
        assert!(
            app.context_menu.is_none(),
            "a press that cancelled a gesture still spends the click (R1, §15 D168)"
        );

        let mut app = crate::app::OndinApp::headless(&ctx);
        app.focus = true;
        app.gesture_cancelled = true;
        app.open_context_menu(&ctx, Target::Canvas, None);
        assert!(
            app.context_menu.is_none(),
            "a cancelled gesture refuses in focus mode too"
        );
    }

    /// A menu open, with everything not named left at its quietest. Fields that
    /// dim a row (`can_frame`, `can_copy_props`, `drifted`, ...) default to the
    /// ordinary case so a test about their dimming cannot pass by accident; fields
    /// that add a row default off so row counts stay stable.
    fn open<'a>(target: Target, selection: &'a [NodeId], kinds: &'a [Kind]) -> Context<'a> {
        Context {
            target,
            selection,
            can_paste: false,
            can_replace: false,
            kinds,
            state: LayerState::default(),
            any_guides: false,
            view: ViewState::default(),
            focus: false,
            segment_selected: false,
            on_segment: false,
            can_join: false,
            join_paths_refused: None,
            text_outlineable: true,
            on_a_rail: false,
            rail_flipped: false,
            can_copy_props: true,
            has_key: false,
            has_props: false,
            system_text: false,
            text_selection: false,
            text_empty: true,
            can_frame: true,
            mask_refused: None,
            role: Role::Plain,
            drifted: true,
            children_differ: false,
            layer_name: String::new(),
            kit: None,
            combinable: false,
            in_set: false,
            holds_main: false,
            create_refused: None,
            combine_refused: None,
            property_reset: None,
            any_exports: true,
            any_frames: true,
        }
    }

    /// No row's label reaches its accelerator at the drawn width (4pt margin). The
    /// label and accelerator are painted at fixed anchors, so an over-long label
    /// overlaps silently.
    ///
    /// flip: restoring the label *Reverse subpath direction* → fails at the clearance assertion.
    #[test]
    fn no_menu_label_runs_into_its_accelerator() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        // One pass so the fonts exist to measure with.
        let _ = ctx.run_ui(Default::default(), |_| {});
        let width = |s: &str, size: f32| -> f32 {
            ctx.fonts_mut(|f| {
                f.layout_no_wrap(
                    s.to_owned(),
                    egui::FontId::proportional(size),
                    egui::Color32::WHITE,
                )
            })
            .rect
            .width()
        };

        // `ui::menu_row`'s own anchors.
        let inner = crate::ui::menu_inner_w(MENU_W, MENU_PAD);
        let label_x = 8.0 + 15.0 + 9.0;
        let accel_x = inner - 8.0;
        const CLEAR: f32 = 4.0;

        for item in Item::ALL {
            let spec = item.spec();
            let label_end = label_x + width(spec.label, 11.5);
            let accel_start = accel_x - spec.accel.map_or(0.0, |a| width(a, 10.5));
            assert!(
                label_end + CLEAR <= accel_start,
                "{:?}: \"{}\" ends at {label_end:.1} and \"{}\" starts at \
                 {accel_start:.1} — the two are painted at fixed anchors, so they \
                 overlap rather than reflowing",
                item,
                spec.label,
                spec.accel.unwrap_or("")
            );
        }
    }

    /// The height a menu is placed by is the height it paints, over shapes that
    /// exercise both separators and row gaps.
    ///
    /// flip: giving `SEP_H` its own value differing from `menu_sep`'s → every shape
    /// with a separator fails; the one-group shape still passes.
    #[test]
    fn a_menus_computed_height_is_the_height_it_paints() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        for shape in [
            vec![1usize],
            vec![3],
            vec![1, 1],
            vec![5, 1, 4],
            vec![2, 3, 1, 6],
        ] {
            let painted = std::cell::Cell::new(0.0_f32);
            let _ = ctx.run_ui(Default::default(), |ui| {
                let out = crate::ui::menu_frame(MENU_PAD).show(ui, |ui| {
                    ui.set_width(crate::ui::menu_inner_w(MENU_W, MENU_PAD));
                    ui.spacing_mut().item_spacing.y = 1.0;
                    for (g, rows) in shape.iter().enumerate() {
                        if g > 0 {
                            crate::ui::menu_sep(ui);
                        }
                        for _ in 0..*rows {
                            crate::ui::menu_row(
                                ui,
                                crate::ui::MenuRow {
                                    label: "Row",
                                    glyph: icon::CIRCLE,
                                    accel: None,
                                    checked: None,
                                    enabled: true,
                                    danger: false,
                                    highlight: None,
                                    filled_hexagon: false,
                                },
                                ROW_H,
                            );
                        }
                    }
                });
                painted.set(out.response.rect.height());
            });

            let groups: Vec<Vec<Row>> = shape
                .iter()
                .map(|n| (0..*n).map(|_| Row::new(Item::Copy)).collect())
                .collect();
            let computed = height(&groups, ROW_H, 1.0, SEP_H, MENU_PAD);
            assert!(
                (computed - painted.get()).abs() < 0.01,
                "{shape:?}: menu_place would put this at {computed} tall and it paints \
                 {} — a menu opened near an edge flips and clamps against the wrong \
                 number and lands off the viewport",
                painted.get()
            );
        }
    }

    fn flat(groups: &[Vec<Row>]) -> Vec<Item> {
        groups.iter().flatten().map(|r| r.item).collect()
    }

    fn labels(groups: &[Vec<Row>]) -> Vec<&str> {
        groups.iter().flatten().map(|r| &*r.label).collect()
    }

    /// Every row a menu can produce is in `Item::ALL`, and every entry there has a
    /// label, a glyph and a group. `ALL` is hand-written, so building every menu
    /// and demanding each row is in it is what makes the list answerable.
    #[test]
    fn every_registry_entry_has_a_label_a_glyph_and_a_group() {
        for item in Item::ALL {
            let spec = item.spec();
            assert!(!spec.label.is_empty(), "{item:?} has no label");
            assert!(!spec.glyph.is_empty(), "{item:?} has no glyph");
            // Reading `group` is the assertion; a wrong group is the ordering test's.
            let _ = spec.group;
        }
        let mut seen: Vec<Item> = Vec::new();
        for item in Item::ALL {
            assert!(!seen.contains(item), "{item:?} is in ALL twice");
            seen.push(*item);
        }

        let all = [id(1), id(2)];
        for target in [
            Target::Canvas,
            Target::PanelBackground,
            Target::Guide(GuideId(id(9))),
            Target::TextSession,
            Target::Points,
            Target::Layer {
                id: id(1),
                door: Door::Canvas,
            },
            Target::Layer {
                id: id(1),
                door: Door::Panel,
            },
        ] {
            for kinds in [
                vec![Kind::Shape],
                vec![Kind::Frame],
                vec![Kind::Group],
                vec![Kind::Boolean(BoolOp::Subtract)],
                vec![Kind::Path],
                vec![Kind::Text],
                vec![Kind::Shape, Kind::Shape],
            ] {
                let sel = &all[..kinds.len().min(2)];
                for row in build(&open(target, sel, &kinds)).iter().flatten() {
                    assert!(
                        Item::ALL.contains(&row.item),
                        "{:?} is produced by {target:?} and missing from Item::ALL",
                        row.item
                    );
                }
            }
        }
    }

    /// A dimmed row always says why, asserted over every menu.
    #[test]
    fn a_dimmed_row_always_carries_its_reason() {
        let sel = [id(1)];
        for kinds in [
            vec![Kind::Shape],
            vec![Kind::Frame],
            vec![Kind::Group],
            vec![Kind::Boolean(BoolOp::Union)],
            vec![Kind::Text],
        ] {
            for state in [
                LayerState::default(),
                LayerState {
                    locked: true,
                    ..LayerState::default()
                },
            ] {
                let mut cx = open(
                    Target::Layer {
                        id: id(1),
                        door: Door::Panel,
                    },
                    &sel,
                    &kinds,
                );
                cx.state = state;
                for row in build(&cx).iter().flatten() {
                    assert_eq!(
                        row.enabled,
                        row.why.is_none(),
                        "{:?}: enabled and `why` must be exact opposites",
                        row.item
                    );
                }
            }
        }
    }

    /// A primitive's menu is 28 rows in the canonical group order, with the empty
    /// head collapsed. Asserts the label sequence, since `build` buckets by
    /// `Spec::group` and only a wrong assignment in `spec()` can still break order.
    #[test]
    fn a_primitives_menu_is_twenty_eight_rows_in_canonical_group_order() {
        let sel = [id(1)];
        let kinds = [Kind::Shape];
        let groups = build(&open(
            Target::Layer {
                id: id(1),
                door: Door::Canvas,
            },
            &sel,
            &kinds,
        ));
        assert_eq!(flat(&groups).len(), 28, "{:?}", labels(&groups));
        assert_eq!(groups.len(), 8, "the head is empty and must collapse");

        assert_eq!(
            labels(&groups),
            [
                "Cut",
                "Copy",
                "Paste here",
                "Paste to replace",
                "Duplicate",
                "Delete",
                "Copy properties",
                "Paste properties",
                "Group selection",
                "Frame selection",
                "Create component",
                "Use as mask",
                "Outline shape",
                "Outline stroke",
                "Bring to front",
                "Bring forward",
                "Send backward",
                "Send to back",
                "Flip horizontal",
                "Flip vertical",
                "Hide",
                "Lock",
                "Rename",
                "Zoom to selection",
                "Copy as SVG",
                "Copy as CSS",
                "Copy as PNG",
                "Copy as PNG 2×",
            ]
        );
    }

    /// A verb promoted into a kind's head is moved, not duplicated (§3).
    ///
    /// flip: `head_rows` pushing *Flatten* and *Ungroup* without Structure dropping
    /// them → fails at the "appears twice" assertion.
    #[test]
    fn a_boolean_promotes_flatten_and_ungroup_without_repeating_them() {
        let sel = [id(1)];
        let kinds = [Kind::Boolean(BoolOp::Subtract)];
        let groups = build(&open(
            Target::Layer {
                id: id(1),
                door: Door::Canvas,
            },
            &sel,
            &kinds,
        ));
        let rows = flat(&groups);
        for want in [Item::Flatten, Item::Ungroup] {
            assert_eq!(
                rows.iter().filter(|i| **i == want).count(),
                1,
                "{want:?} appears twice in {:?}",
                labels(&groups)
            );
        }
        let head = groups.first().expect("a boolean has a head");
        assert!(head.iter().any(|r| r.item == Item::Flatten));
        assert!(head.iter().any(|r| r.item == Item::Ungroup));

        // The four operations are checkable here, and exactly one is ticked.
        let ticked: Vec<&str> = groups
            .iter()
            .flatten()
            .filter(|r| r.checked == Some(true))
            .map(|r| &*r.label)
            .collect();
        assert_eq!(ticked, ["Subtract"]);
    }

    /// A boolean's menu is the long end of §3's range: head 3 · Clipboard 6 ·
    /// Properties 2 · Structure 9 · Order 4 · Transform 2 · State 3 · Navigate 1 ·
    /// Export 4.
    #[test]
    fn a_booleans_menu_is_the_long_end_of_the_range() {
        let sel = [id(1)];
        let kinds = [Kind::Boolean(BoolOp::Subtract)];
        let groups = build(&open(
            Target::Layer {
                id: id(1),
                door: Door::Canvas,
            },
            &sel,
            &kinds,
        ));
        assert_eq!(flat(&groups).len(), 34, "{:?}", labels(&groups));
    }

    /// Row counts for a text layer on a rail and the component roles, with the
    /// arithmetic in each case's message. The linked boolean is the long end, at 36.
    #[test]
    fn the_menus_the_two_ends_do_not_count() {
        let sel = [id(1)];
        let target = Target::Layer {
            id: id(1),
            door: Door::Canvas,
        };
        let count = |kind: Kind, f: &dyn Fn(&mut Context<'_>)| {
            let kinds = [kind];
            let mut cx = open(target, &sel, &kinds);
            f(&mut cx);
            let groups = build(&cx);
            (flat(&groups).len(), format!("{:?}", labels(&groups)))
        };
        let cases: [(&str, usize, (usize, String)); 7] = [
            (
                "text on a rail: the plain 31, plus *Flip to other side* and \
                 *Detach from path*",
                33,
                count(Kind::Text, &|cx| {
                    cx.state.text_sizing = Some(0);
                    cx.on_a_rail = true;
                }),
            ),
            (
                "a main frame: head 1 · Clipboard 6 · Properties 2 · Structure 4 \
                 (*Group selection*, *Frame selection*, *Duplicate as component*, \
                 *Select all instances*) · Order 4 · Transform 2 · State 3 · \
                 Navigate 1 · Export 4",
                27,
                count(Kind::Frame, &|cx| cx.role = Role::Main),
            ),
            (
                "an instance frame: a main's, with *Go to main component*, *Reset \
                 all* and *Detach instance* for its two",
                28,
                count(Kind::Frame, &|cx| cx.role = Role::Instance),
            ),
            (
                "a linked shape in an instance: the primitive's 28, with *Go to \
                 main component* and *Reset <layer>* for *Create component*",
                29,
                count(Kind::Shape, &|cx| {
                    cx.role = Role::Member;
                    cx.layer_name = "Label".into();
                }),
            ),
            (
                "a bound text child: text's 31, with *Go to main component*, \
                 *Reset <property>* and *Reset <layer>* for *Create component*",
                33,
                count(Kind::Text, &|cx| {
                    cx.state.text_sizing = Some(0);
                    cx.role = Role::Member;
                    cx.layer_name = "Label".into();
                    cx.property_reset = Some("Label text".into());
                }),
            ),
            (
                "a bound text child on a rail: the railed 33, with the bound \
                 child's three rows for *Create component*",
                35,
                count(Kind::Text, &|cx| {
                    cx.state.text_sizing = Some(0);
                    cx.on_a_rail = true;
                    cx.role = Role::Member;
                    cx.layer_name = "Label".into();
                    cx.property_reset = Some("Label text".into());
                }),
            ),
            (
                "a linked boolean whose bound visibility differs: the boolean's \
                 34, with the bound child's three rows for *Create component* — \
                 the long end",
                36,
                count(Kind::Boolean(BoolOp::Union), &|cx| {
                    cx.role = Role::Member;
                    cx.layer_name = "Label".into();
                    cx.property_reset = Some("Label".into());
                }),
            ),
        ];
        let wrong: Vec<String> = cases
            .into_iter()
            .filter(|(_, want, (got, _))| got != want)
            .map(|(what, want, (got, labels))| format!("{what}: {got} not {want}, {labels}"))
            .collect();
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    /// Only *Create component* and *Duplicate as component* draw the filled
    /// hexagon, as a six-point polygon in the glyph's place.
    ///
    /// flip: `menu_row` ignoring the flag → fails at "the filled row paints it".
    #[test]
    fn the_rows_that_make_a_main_draw_its_filled_hexagon() {
        for item in Item::ALL {
            assert_eq!(
                item.spec().filled_hexagon,
                matches!(item, Item::CreateComponent | Item::DuplicateAsComponent),
                "{item:?}"
            );
        }
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let hexagons = |filled: bool| {
            let out = ctx.run_ui(Default::default(), |ui| {
                crate::ui::menu_row(
                    ui,
                    crate::ui::MenuRow::new(icon::HEXAGON, "Create component")
                        .filled_hexagon(filled),
                    ROW_H,
                );
            });
            out.shapes
                .iter()
                .filter(|c| {
                    matches!(&c.shape, egui::Shape::Path(p)
                        if p.closed && p.points.len() == 6 && p.fill != egui::Color32::TRANSPARENT)
                })
                .count()
        };
        assert_eq!(hexagons(true), 1, "the filled row paints it");
        assert_eq!(hexagons(false), 0, "an ordinary row paints its glyph");
    }

    /// A text layer's menu is 31 rows: head 4 · Clipboard 6 · Properties 2 ·
    /// Structure 5 · Order 4 · Transform 2 · State 3 · Navigate 1 · Export 4. The
    /// fixture must carry a `text_sizing`, which a real text layer always has.
    #[test]
    fn a_text_layers_menu_is_thirty_one_rows() {
        let sel = [id(1)];
        let kinds = [Kind::Text];
        let mut cx = open(
            Target::Layer {
                id: id(1),
                door: Door::Canvas,
            },
            &sel,
            &kinds,
        );
        cx.state = LayerState {
            text_sizing: Some(0),
            ..LayerState::default()
        };
        let groups = build(&cx);
        assert_eq!(flat(&groups).len(), 31, "{:?}", labels(&groups));
    }

    /// The component rows sit in Structure directly after *Frame selection*, one
    /// role at a time in the mockups' order (`context-menus.md` §4).
    ///
    /// flip: filing *Go to main component* under `Group::Navigate` → fails at "an
    /// instance"; dropping the `holds_main` guard → fails at "several mains:
    /// Combine and nothing else".
    #[test]
    fn the_component_rows_follow_frame_selection_in_structure() {
        let sel = [id(1)];
        let target = Target::Layer {
            id: id(1),
            door: Door::Canvas,
        };
        let cases: [(&str, Kind, Role, &[&str]); 5] = [
            ("a shape", Kind::Shape, Role::Plain, &["Create component"]),
            (
                "a main",
                Kind::Frame,
                Role::Main,
                &["Duplicate as component", "Select all instances"],
            ),
            (
                "a set",
                Kind::Frame,
                Role::Set,
                &["Add variant", "Select all instances"],
            ),
            // A nested copy bound to a property is an instance root, so the
            // instance arm carries the property's row too.
            (
                "an instance",
                Kind::Frame,
                Role::Instance,
                &[
                    "Go to main component",
                    "Reset Label text",
                    "Reset all",
                    "Detach instance",
                ],
            ),
            (
                "a child in an instance",
                Kind::Text,
                Role::Member,
                &["Go to main component", "Reset Label text", "Reset Label"],
            ),
        ];
        let after_frame = |cx: &Context<'_>| -> Vec<String> {
            let groups = build(cx);
            let structure: Vec<&Row> = groups
                .iter()
                .flatten()
                .filter(|r| r.item.spec().group == Group::Structure)
                .collect();
            let at = structure
                .iter()
                .position(|r| r.item == Item::FrameSelection)
                .unwrap_or_else(|| panic!("no Frame selection in {:?}", labels(&groups)));
            structure[at + 1..]
                .iter()
                .map(|r| r.label.to_string())
                .collect()
        };
        for (what, kind, role, want) in cases {
            let kinds = [kind];
            let mut cx = open(target, &sel, &kinds);
            cx.role = role;
            cx.layer_name = "Label".into();
            cx.property_reset = Some("Label text".into());
            let got = after_frame(&cx);
            assert_eq!(&got[..want.len().min(got.len())], want, "{what}: {got:?}");
        }
        // Several mains are `Role::Plain` (`component_role`); `holds_main` withholds
        // *Create component* and `combinable` offers *Combine as variants*.
        let two = [id(1), id(2)];
        let kinds = [Kind::Frame, Kind::Frame];
        let mut cx = open(target, &two, &kinds);
        cx.holds_main = true;
        cx.combinable = true;
        let got = after_frame(&cx);
        assert_eq!(
            got.first().map(String::as_str),
            Some("Combine as variants"),
            "several mains: {got:?}"
        );
        let others = [
            "Create component",
            "Duplicate as component",
            "Select all instances",
            "Add variant",
        ];
        assert!(
            !got.iter().any(|l| others.contains(&l.as_str())),
            "several mains: Combine and nothing else of the block, {got:?}"
        );
    }

    /// A locked layer's menu is whole: every editing row dimmed, the way-back rows
    /// live (§5.9).
    #[test]
    fn a_locked_layer_dims_the_editing_rows_and_keeps_the_way_back() {
        let sel = [id(1)];
        let kinds = [Kind::Shape];
        let mut cx = open(
            Target::Layer {
                id: id(1),
                door: Door::Panel,
            },
            &sel,
            &kinds,
        );
        cx.state = LayerState {
            locked: true,
            all_locked: true,
            ..LayerState::default()
        };
        let groups = build(&cx);
        assert_eq!(flat(&groups).len(), 28, "{:?}", labels(&groups));

        let row = |item: Item| {
            groups
                .iter()
                .flatten()
                .find(|r| r.item == item)
                .unwrap_or_else(|| panic!("{item:?} is missing from a locked layer's menu"))
        };
        // §5.9's live list plus *Copy properties*: reading a locked layer is not
        // editing it. Its partner writes, so it dims with the lock's sentence.
        for live in [
            Item::Copy,
            Item::CopyProperties,
            Item::Rename,
            Item::ToggleHidden,
            Item::ToggleLocked,
            Item::ZoomSelection,
        ] {
            assert!(
                row(live).enabled,
                "{live:?} must stay live on a locked layer"
            );
        }
        assert_eq!(row(Item::ToggleLocked).label, "Unlock");
        for dim in [
            Item::Cut,
            Item::Delete,
            Item::PasteProperties,
            Item::Flip(build::Axis::X),
        ] {
            assert!(
                !row(dim).enabled,
                "{dim:?} must be dimmed on a locked layer"
            );
            assert_eq!(row(dim).why, Some("This layer is locked"));
        }
    }

    /// The canvas menu carries no selection rows, even with layers selected (C3).
    #[test]
    fn the_canvas_menu_ignores_the_selection() {
        let sel = [id(1), id(2)];
        let kinds = [Kind::Shape, Kind::Shape];
        let groups = build(&open(Target::Canvas, &sel, &kinds));
        let rows = flat(&groups);
        assert!(
            !rows.iter().any(|i| matches!(
                i,
                Item::Delete | Item::Group | Item::ToggleHidden | Item::Flip(_)
            )),
            "the canvas menu has selection rows on it: {:?}",
            labels(&groups)
        );
        // *Lock guides* must be reversible from the canvas (C7); the full set is
        // pinned by `the_canvas_menu_carries_every_view_row_live_in_focus_mode_too`.
        assert!(rows.contains(&Item::ViewSwitch(ViewSwitch::GuideLock)));
    }

    /// The canvas menu's View rows are the View menu's less the four chrome-wide
    /// switches §6.1 keeps out, all live in focus mode too. The expected set is
    /// derived from `ViewSwitch::VIEW_MENU`, so a new switch either appears here
    /// or fails here.
    ///
    /// flip: dropping `LayoutGrid` from `view_rows` → fails at the set assertion;
    /// dimming *Show rulers* on `cx.focus` → fails at "and every one live".
    #[test]
    fn the_canvas_menu_carries_every_view_row_live_in_focus_mode_too() {
        let view_rows_of = |cx: &Context<'_>| -> Vec<(ViewSwitch, bool, Option<&'static str>)> {
            build(cx)
                .iter()
                .flatten()
                .filter_map(|r| match r.item {
                    Item::ViewSwitch(sw) => Some((sw, r.enabled, r.why)),
                    _ => None,
                })
                .collect()
        };
        let expected: Vec<ViewSwitch> = ViewSwitch::VIEW_MENU
            .into_iter()
            .filter(|sw| {
                !matches!(
                    sw,
                    ViewSwitch::Layers
                        | ViewSwitch::Toolbar
                        | ViewSwitch::StatusMessages
                        | ViewSwitch::Focus
                )
            })
            .collect();

        let cx = open(Target::Canvas, &[], &[]);
        let got = view_rows_of(&cx);
        assert_eq!(
            got.iter().map(|(sw, _, _)| *sw).collect::<Vec<_>>(),
            expected,
            "the canvas menu's View rows are not the View menu's, less §6.1's four"
        );
        assert!(
            got.iter().all(|(_, enabled, _)| *enabled),
            "every View row is live when focus mode is off"
        );

        let mut cx = open(Target::Canvas, &[], &[]);
        cx.focus = true;
        let got = view_rows_of(&cx);
        assert_eq!(
            got.iter().map(|(sw, _, _)| *sw).collect::<Vec<_>>(),
            expected,
            "focus mode keeps every row — §6 says kind decides presence"
        );
        assert!(
            got.iter().all(|(_, enabled, _)| *enabled),
            "and every one live: focus mode hides panels, not the rulers, guides \
             or grids these rows switch"
        );
    }

    /// Each View row ticks from its own field.
    ///
    /// flip: `ViewState::get` answering `LayoutGrid` with `self.grid` → fails
    /// naming *Show layout grid*.
    #[test]
    fn every_view_row_ticks_from_its_own_field() {
        // `grid` off and `layout_grid` on is the pair a copy-paste would collapse.
        let st = ViewState {
            rulers: true,
            guides: false,
            guide_lock: true,
            grid: false,
            layout_grid: true,
        };
        let mut cx = open(Target::Canvas, &[], &[]);
        cx.view = st;
        for (sw, want) in [
            (ViewSwitch::Rulers, true),
            (ViewSwitch::Guides, false),
            (ViewSwitch::GuideLock, true),
            (ViewSwitch::Grid, false),
            (ViewSwitch::LayoutGrid, true),
        ] {
            let row = build(&cx)
                .iter()
                .flatten()
                .find(|r| r.item == Item::ViewSwitch(sw))
                .map(|r| r.checked)
                .unwrap_or_else(|| panic!("{} has no row in the canvas menu", sw.label()));
            assert_eq!(
                row,
                Some(want),
                "{} ticked from the wrong field",
                sw.label()
            );
        }
    }

    /// A text session's four rows act on the editor, and its *Paste* reads the
    /// system clipboard's text, not the layer clipboard (§6.4).
    ///
    /// flip: the *Paste* predicate becoming `!cx.can_paste` → fails at "there is no
    /// *text* to paste".
    #[test]
    fn a_text_sessions_paste_reads_the_system_clipboard_and_not_the_layer_one() {
        let sel: [NodeId; 0] = [];
        let kinds: [Kind; 0] = [];
        let mut cx = open(Target::TextSession, &sel, &kinds);
        // A pasteable payload in the app and no text on the system clipboard.
        cx.can_paste = true;

        let groups = build(&cx);
        assert_eq!(
            labels(&groups),
            ["Cut", "Copy", "Paste", "Select all"],
            "§6.4's four rows, in §6.4's order, in one group"
        );
        assert_eq!(groups.len(), 1, "all four are Clipboard rows");
        let rows = &groups[0];
        assert!(!rows[2].enabled, "there is no *text* to paste");
        assert!(!rows[0].enabled, "Cut with a bare caret");
        assert!(!rows[1].enabled, "Copy with a bare caret");
        assert!(!rows[3].enabled, "Select all in an empty buffer");

        cx.system_text = true;
        cx.text_selection = true;
        cx.text_empty = false;
        let groups = build(&cx);
        let rows = &groups[0];
        assert!(rows.iter().all(|r| r.enabled), "{:?}", labels(&groups));
    }

    /// Several mains are offered *Combine as variants* and not *Create component*,
    /// which would nest them in a main.
    ///
    /// flip: dropping the `holds_main` guard → offers both.
    #[test]
    fn several_mains_combine_and_are_not_made_a_component() {
        let sel = [id(1), id(2)];
        let kinds = [Kind::Frame, Kind::Frame];
        let target = Target::Layer {
            id: id(1),
            door: Door::Canvas,
        };
        let mut cx = open(target, &sel, &kinds);
        let plain = flat(&build(&cx));
        assert!(plain.contains(&Item::CreateComponent), "two plain frames");
        cx.combinable = true;
        cx.holds_main = true;
        let mains = flat(&build(&cx));
        assert!(mains.contains(&Item::CombineAsVariants));
        assert!(!mains.contains(&Item::CreateComponent));
    }

    /// A refusal the commit would make dims the row with its reason: *Create
    /// component* and *Combine as variants* over mains of two kinds. That the
    /// fields are the commit's answers is `app::component_verb_tests`'.
    ///
    /// flip: deleting the `create_refused` dim → fails at "dim inside a main"; the
    /// `combine_refused` dim → fails at "dim over two kinds".
    #[test]
    fn a_component_row_the_commit_would_refuse_is_dim_with_the_reason() {
        let sel = [id(1)];
        let kinds = [Kind::Shape];
        let target = Target::Layer {
            id: id(1),
            door: Door::Canvas,
        };
        let mut cx = open(target, &sel, &kinds);
        let row = |cx: &Context, item: Item| {
            build(cx)
                .into_iter()
                .flatten()
                .find(|r| r.item == item)
                .map(|r| (r.enabled, r.why))
        };
        assert_eq!(
            row(&cx, Item::CreateComponent),
            Some((true, None)),
            "the fixture: offered and live"
        );
        let inside = create_refusal(ondin_core::component::NotMain::Inside);
        cx.create_refused = Some(inside);
        assert_eq!(
            row(&cx, Item::CreateComponent),
            Some((false, Some(inside))),
            "dim inside a main"
        );

        cx.combinable = true;
        cx.holds_main = true;
        assert_eq!(row(&cx, Item::CombineAsVariants), Some((true, None)));
        cx.combine_refused = Some(COMBINE_MIXED);
        assert_eq!(
            row(&cx, Item::CombineAsVariants),
            Some((false, Some(COMBINE_MIXED))),
            "dim over two kinds"
        );
    }

    /// *Reset origin* is omitted at the default state and present once moved
    /// (§3's exception); both halves, since "always absent" passes the first.
    #[test]
    fn reset_origin_appears_only_once_the_origin_has_been_moved() {
        let sel = [id(1)];
        let kinds = [Kind::Shape];
        let target = Target::Layer {
            id: id(1),
            door: Door::Canvas,
        };
        let plain = build(&open(target, &sel, &kinds));
        assert!(!flat(&plain).contains(&Item::ResetOrigin));

        let mut moved = open(target, &sel, &kinds);
        moved.state = LayerState {
            pivot_moved: true,
            ..LayerState::default()
        };
        let groups = build(&moved);
        assert!(
            flat(&groups).contains(&Item::ResetOrigin),
            "{:?}",
            labels(&groups)
        );
    }

    /// *Reset all* on an instance and *Reset* named for a linked child are each
    /// omitted when nothing differs from the main.
    #[test]
    fn the_reset_rows_name_their_layer_and_go_at_zero() {
        let sel = [id(1)];
        let kinds = [Kind::Shape];
        let target = Target::Layer {
            id: id(1),
            door: Door::Canvas,
        };
        let row = |cx: &Context<'_>, item: Item| {
            build(cx)
                .into_iter()
                .flatten()
                .find(|r| r.item == item)
                .unwrap_or_else(|| panic!("no {item:?} row"))
        };
        let mut cx = open(target, &sel, &kinds);
        cx.role = Role::Instance;
        assert!(row(&cx, Item::ResetInstance).enabled);
        cx.drifted = false;
        assert!(!flat(&build(&cx)).contains(&Item::ResetInstance));
        assert!(
            flat(&build(&cx)).contains(&Item::DetachInstance),
            "only the reset goes"
        );

        cx.role = Role::Member;
        cx.drifted = true;
        cx.layer_name = "Label".into();
        assert_eq!(row(&cx, Item::ResetChild).label, "Reset Label");
        assert!(!flat(&build(&cx)).contains(&Item::ResetInstance));
        cx.drifted = false;
        assert!(!flat(&build(&cx)).contains(&Item::ResetChild));
    }

    /// A kit instance's menu gains its kit rows (*Go to main in Atlas*, *Accept kit
    /// updates* when one waits, *Make local…* last) and a local instance's does not.
    #[test]
    fn a_kit_instance_gains_its_kit_rows_and_a_local_one_does_not() {
        let sel = [id(1)];
        let kinds = [Kind::Frame];
        let target = Target::Layer {
            id: id(1),
            door: Door::Canvas,
        };
        let mut cx = open(target, &sel, &kinds);
        cx.role = Role::Instance;
        let local = flat(&build(&cx));
        assert!(!local.contains(&Item::AcceptKitUpdates) && !local.contains(&Item::MakeLocal));

        cx.kit = Some(("Atlas".into(), true));
        let rows: Vec<Row> = build(&cx).into_iter().flatten().collect();
        let go = rows.iter().find(|r| r.item == Item::GoToMain).unwrap();
        assert_eq!(go.label, "Go to main in Atlas");
        let order: Vec<Item> = rows.iter().map(|r| r.item).collect();
        let at = |i: Item| order.iter().position(|o| *o == i).unwrap();
        assert_eq!(at(Item::AcceptKitUpdates), at(Item::GoToMain) + 1);
        assert!(at(Item::MakeLocal) > at(Item::DetachInstance));

        cx.kit = Some(("Atlas".into(), false));
        assert!(
            !flat(&build(&cx)).contains(&Item::AcceptKitUpdates),
            "only when one waits"
        );
    }

    /// `height` is rows, separators, gaps, padding and border, term by term; the
    /// companion to `a_menus_computed_height_is_the_height_it_paints`, which checks
    /// the total against the drawing.
    #[test]
    fn a_menus_height_is_its_rows_its_separators_and_its_gaps() {
        let border = crate::ui::MENU_BORDER * 2.0;
        let groups = vec![vec![Row::new(Item::Copy)], vec![Row::new(Item::Delete)]];
        // 5 + 5 padding, two 26pt rows, one 9pt separator, two 1pt gaps.
        assert_eq!(
            height(&groups, 26.0, 1.0, 9.0, 5.0),
            border + 10.0 + 52.0 + 9.0 + 2.0
        );
        // One group: no separator, and one fewer gap than there are rows.
        let one = vec![vec![Row::new(Item::Copy), Row::new(Item::Cut)]];
        assert_eq!(
            height(&one, 26.0, 1.0, 9.0, 5.0),
            border + 10.0 + 52.0 + 1.0
        );
    }

    /// The picture head is two rows and a lock dims both (§5.7): *Original size*
    /// resizes the node and a crop is a resize too. The guard is
    /// `app::image_edit_lock_tests::a_locked_picture_refuses_every_door`; the mode
    /// must refuse first and this row follow.
    ///
    /// flip: dropping `dim_if(locked, why)` from either row fails that row's assertion only.
    #[test]
    fn a_lock_dims_both_of_the_picture_rows() {
        let sel = [id(1)];
        let kinds = [Kind::Shape];
        let mut cx = open(
            Target::Layer {
                id: id(1),
                door: Door::Canvas,
            },
            &sel,
            &kinds,
        );
        cx.state = LayerState {
            locked: true,
            has_picture: true,
            ..LayerState::default()
        };
        let groups = build(&cx);
        let row = |item: Item| {
            groups
                .iter()
                .flatten()
                .find(|r| r.item == item)
                .unwrap_or_else(|| panic!("{item:?} is missing: {:?}", labels(&groups)))
        };
        let head: Vec<Item> = groups
            .iter()
            .flatten()
            .map(|r| r.item)
            .filter(|i| matches!(i, Item::EditImage | Item::OriginalSize))
            .collect();
        assert_eq!(
            head,
            vec![Item::EditImage, Item::OriginalSize],
            "the picture head is two rows: {:?}",
            labels(&groups)
        );

        assert!(
            !row(Item::EditImage).enabled,
            "a crop is a resize, and the lock reaches it — the mode refuses first \
             and this row follows: {:?}",
            labels(&groups)
        );
        assert_eq!(row(Item::EditImage).why, Some("This layer is locked"));

        assert!(
            !row(Item::OriginalSize).enabled,
            "resizing the node is a write and the lock reaches it: {:?}",
            labels(&groups)
        );
        assert_eq!(row(Item::OriginalSize).why, Some("This layer is locked"));
    }

    /// A layer inside a locked group dims the same rows with the group's reason,
    /// and *Unlock* does not appear: the label reads `all_locked`, the selection's
    /// own flags, which are clear.
    ///
    /// flip: `locked_within` reading `locked` alone → every row live; `why`
    /// unconditional → wrong sentence; `ToggleLocked`'s label on `locked_within()`
    /// → *Unlock* appears. Each fails one assertion only.
    #[test]
    fn a_layer_inside_a_locked_group_dims_with_the_groups_reason() {
        let sel = [id(1)];
        let kinds = [Kind::Shape];
        let mut cx = open(
            Target::Layer {
                id: id(1),
                door: Door::Panel,
            },
            &sel,
            &kinds,
        );
        cx.state = LayerState {
            locked_by_ancestor: true,
            has_picture: true,
            ..LayerState::default()
        };
        assert!(
            !cx.state.locked && cx.state.locked_within(),
            "the fixture is the ancestor case: own flag clear, effectively locked"
        );
        let groups = build(&cx);
        let row = |item: Item| {
            groups
                .iter()
                .flatten()
                .find(|r| r.item == item)
                .unwrap_or_else(|| panic!("{item:?} is missing: {:?}", labels(&groups)))
        };

        for item in [Item::EditImage, Item::OriginalSize, Item::Delete] {
            assert!(
                !row(item).enabled,
                "{item:?} must dim inside a locked group: {:?}",
                labels(&groups)
            );
            assert_eq!(
                row(item).why,
                Some("A group containing this layer is locked"),
                "{item:?} must name the group rather than this layer, which is not \
                 itself locked"
            );
        }

        assert_eq!(
            row(Item::ToggleLocked).label,
            "Lock",
            "the toggle reads the selection's own flags: offering *Unlock* here \
             would write a flag that is already clear and appear to do nothing"
        );
    }

    /// *Edit image* is the picture's head verb: present only with a picture and
    /// first of the picture rows.
    ///
    /// flip: pushing the row after *Original size* → fails the order assertion
    /// alone; gating it on something but `has_picture` → fails the absence one.
    #[test]
    fn edit_image_heads_the_picture_rows_and_appears_only_with_a_picture() {
        let sel = [id(1)];
        let kinds = [Kind::Shape];
        let menu = |has_picture: bool| {
            let mut cx = open(
                Target::Layer {
                    id: id(1),
                    door: Door::Canvas,
                },
                &sel,
                &kinds,
            );
            cx.state = LayerState {
                has_picture,
                ..LayerState::default()
            };
            build(&cx)
        };

        let with = menu(true);
        let items: Vec<Item> = with.iter().flatten().map(|r| r.item).collect();
        let at = |item: Item| {
            items
                .iter()
                .position(|i| *i == item)
                .unwrap_or_else(|| panic!("{item:?} is missing: {:?}", labels(&with)))
        };
        assert!(
            at(Item::EditImage) < at(Item::OriginalSize),
            "the head verb must come before Original size: {:?}",
            labels(&with)
        );

        let without = menu(false);
        assert!(
            !without.iter().flatten().any(|r| r.item == Item::EditImage),
            "a layer with no picture must not offer to edit one: {:?}",
            labels(&without)
        );
    }

    /// A locked layer's menu: mostly dimmed, which suits the keyboard walk.
    fn locked_menu() -> Vec<Vec<Row>> {
        let sel = [id(1)];
        let kinds = [Kind::Shape];
        let mut cx = open(
            Target::Layer {
                id: id(1),
                door: Door::Panel,
            },
            &sel,
            &kinds,
        );
        cx.state = LayerState {
            locked: true,
            ..LayerState::default()
        };
        build(&cx)
    }

    /// `↓` visits only the rows a press could run (§8).
    ///
    /// flip: dropping the `.filter(|r| r.enabled)` in `navigate` → the first `↓`
    /// lands on the dimmed *Cut*.
    #[test]
    fn the_keyboard_walk_visits_only_the_rows_a_press_could_run() {
        let groups = locked_menu();
        let live: Vec<Item> = groups
            .iter()
            .flatten()
            .filter(|r| r.enabled)
            .map(|r| r.item)
            .collect();
        let all = flat(&groups).len();
        assert_eq!(all, 28, "{:?}", labels(&groups));
        assert!(
            live.len() > 1 && live.len() < all,
            "the fixture needs live rows *and* dimmed ones, and has {} of {all}",
            live.len()
        );
        assert!(
            !groups
                .iter()
                .flatten()
                .take_while(|r| !r.enabled)
                .collect::<Vec<_>>()
                .is_empty(),
            "and a dimmed row *before* the first live one, or `Next` from nothing \
             cannot tell the two walks apart"
        );

        let mut at = None;
        let mut walked = Vec::new();
        for _ in 0..live.len() + 1 {
            at = navigate(&groups, at, Step::Next);
            walked.push(at.expect("a menu with live rows always has a next"));
        }
        assert_eq!(walked[..live.len()], live[..]);
        assert_eq!(
            walked[live.len()],
            live[0],
            "the walk has to wrap, or the last row is a dead end"
        );

        let mut back = Vec::new();
        let mut at = None;
        for _ in 0..live.len() {
            at = navigate(&groups, at, Step::Prev);
            back.push(at.expect("a menu with live rows always has a previous"));
        }
        let mut reversed = live.clone();
        reversed.reverse();
        assert_eq!(back, reversed);
    }

    /// `↑` on a fresh menu is the last live row, and `Home`/`End` are the ends of
    /// the live run (§8); a naive `unwrap_or(0)` gets only the `↑` arm wrong.
    #[test]
    fn up_from_nothing_is_the_last_row_and_the_ends_are_live_ones() {
        let groups = locked_menu();
        let live: Vec<Item> = groups
            .iter()
            .flatten()
            .filter(|r| r.enabled)
            .map(|r| r.item)
            .collect();
        let last = *live.last().expect("the fixture has live rows");

        assert_eq!(navigate(&groups, None, Step::Prev), Some(last));
        assert_eq!(navigate(&groups, None, Step::Next), Some(live[0]));
        assert_eq!(navigate(&groups, None, Step::First), Some(live[0]));
        assert_eq!(navigate(&groups, None, Step::Last), Some(last));
        assert_eq!(navigate(&groups, Some(last), Step::First), Some(live[0]));
        assert_eq!(navigate(&groups, Some(live[0]), Step::Last), Some(last));
        let bottom = flat(&groups).last().copied().expect("16 rows");
        if bottom != last {
            assert_ne!(
                navigate(&groups, None, Step::Last),
                Some(bottom),
                "`End` must be the last *live* row"
            );
        }

        let stale = flat(&groups)
            .into_iter()
            .find(|i| !live.contains(i))
            .expect("the fixture has a dimmed row");
        assert_eq!(navigate(&groups, Some(stale), Step::Next), Some(live[0]));
        assert_eq!(navigate(&groups, Some(stale), Step::Prev), Some(last));

        let dead = vec![vec![Row::new(Item::Cut).dim_if(true, "no")]];
        for step in [Step::Next, Step::Prev, Step::First, Step::Last] {
            assert_eq!(navigate(&dead, None, step), None, "{step:?}");
        }
    }

    /// The even-odd row is offered on a path and nothing else: a shape's rules
    /// agree, a boolean derives its rule, and a multi-selection has no one answer
    /// to tick.
    #[test]
    fn even_odd_is_offered_on_a_path_alone() {
        let two = [id(1), id(2)];
        let menu = |kinds: &[Kind], even_odd: bool| {
            let mut cx = open(
                Target::Layer {
                    id: id(1),
                    door: Door::Canvas,
                },
                &two[..kinds.len().min(2)],
                kinds,
            );
            cx.state = LayerState {
                even_odd,
                ..LayerState::default()
            };
            build(&cx)
        };
        let row = |groups: &'_ [Vec<Row>]| {
            groups
                .iter()
                .flatten()
                .find(|r| r.item == Item::EvenOdd)
                .map(|r| r.checked)
        };

        assert_eq!(
            row(&menu(&[Kind::Path], false)),
            Some(Some(false)),
            "on a path"
        );
        assert_eq!(
            row(&menu(&[Kind::Path], true)),
            Some(Some(true)),
            "and it reports the rule the path has"
        );
        assert_eq!(row(&menu(&[Kind::Shape], false)), None, "not on a shape");
        assert_eq!(
            row(&menu(&[Kind::Boolean(ondin_core::BoolOp::Exclude)], true)),
            None,
            "and never on a boolean, whose rule is its operation's"
        );
        assert_eq!(
            row(&menu(&[Kind::Path, Kind::Path], false)),
            None,
            "nor on two, which have no one answer to tick"
        );
    }

    /// *Join paths* appears on exactly two path layers, dim with the reason where
    /// `join_layers_subject` refused, in its words; *Outline stroke* on one stroked shape.
    /// flip: the *Join paths* condition reading `kinds.len() == 2` → fails at "a path
    /// and a shape".
    #[test]
    fn join_paths_and_outline_stroke_appear_on_their_subjects() {
        let two = [id(1), id(2)];
        let menu = |kinds: &[Kind], f: &dyn Fn(&mut Context<'_>)| {
            let mut cx = open(
                Target::Layer {
                    id: id(1),
                    door: Door::Canvas,
                },
                &two[..kinds.len()],
                kinds,
            );
            f(&mut cx);
            build(&cx)
        };
        let row = |groups: &[Vec<Row>], item: Item| {
            groups
                .iter()
                .flatten()
                .find(|r| r.item == item)
                .map(|r| (r.enabled, r.why))
        };
        let m = menu(&[Kind::Path, Kind::Path], &|_| {});
        assert_eq!(row(&m, Item::JoinPaths), Some((true, None)));
        let m = menu(&[Kind::Path, Kind::Path], &|cx| {
            cx.join_paths_refused = Some("Both paths need an open end")
        });
        assert_eq!(
            row(&m, Item::JoinPaths),
            Some((false, Some("Both paths need an open end")))
        );
        let m = menu(&[Kind::Path, Kind::Shape], &|_| {});
        assert_eq!(row(&m, Item::JoinPaths), None, "a path and a shape");

        let m = menu(&[Kind::Shape], &|cx| cx.state.stroke_outlineable = true);
        assert_eq!(row(&m, Item::OutlineStroke), Some((true, None)));
        let m = menu(&[Kind::Shape], &|_| {});
        assert_eq!(
            row(&m, Item::OutlineStroke),
            Some((false, Some("This layer has no stroke")))
        );
        assert_eq!(
            row(&menu(&[Kind::Text], &|_| {}), Item::OutlineStroke),
            None
        );
    }

    /// *Outline shape* appears on one shape or path, is dim where there is nothing
    /// to bake, and is never offered beside *Flatten*.
    ///
    /// flip: dropping `one` from the row's condition → fails at the pair case,
    /// where *Outline shape* appears next to *Flatten*.
    #[test]
    fn outline_shape_appears_on_one_shape_and_never_beside_flatten() {
        let two = [id(1), id(2)];
        let one_of = |kinds: &[Kind], state: LayerState| {
            let sel = &two[..kinds.len().min(2)];
            let mut cx = open(
                Target::Layer {
                    id: id(1),
                    door: Door::Canvas,
                },
                sel,
                kinds,
            );
            cx.state = state;
            build(&cx)
        };
        let has = |groups: &[Vec<Row>], item: Item| groups.iter().flatten().any(|r| r.item == item);
        let row = |groups: &'_ [Vec<Row>], item: Item| {
            groups
                .iter()
                .flatten()
                .find(|r| r.item == item)
                .map(|r| (r.enabled, r.why))
        };
        let outlineable = LayerState {
            outlineable: true,
            ..LayerState::default()
        };

        let m = one_of(&[Kind::Shape], outlineable);
        assert_eq!(row(&m, Item::OutlineShape), Some((true, None)));
        assert!(!has(&m, Item::Flatten), "and Flatten is not beside it");

        let m = one_of(&[Kind::Path], outlineable);
        assert_eq!(row(&m, Item::OutlineShape), Some((true, None)));

        // Present and dim: the kind can be outlined, this instance has nothing to gain.
        let m = one_of(&[Kind::Path], LayerState::default());
        assert_eq!(
            row(&m, Item::OutlineShape),
            Some((false, Some("This path is already an outline")))
        );

        let m = one_of(
            &[Kind::Path],
            LayerState {
                locked: true,
                ..LayerState::default()
            },
        );
        assert_eq!(
            row(&m, Item::OutlineShape),
            Some((false, Some("This layer is locked")))
        );

        for (kinds, why) in [
            (vec![Kind::Boolean(BoolOp::Union)], "a boolean is Flatten's"),
            (vec![Kind::Group], "a group has no outline of its own"),
            (vec![Kind::Frame], "a frame is a page"),
            (vec![Kind::Text], "text needs its glyphs converted"),
            (
                vec![Kind::Shape, Kind::Shape],
                "a set is Flatten's, however outlineable each member is",
            ),
        ] {
            let m = one_of(&kinds, outlineable);
            assert!(!has(&m, Item::OutlineShape), "{why}: {:?}", labels(&m));
        }
        assert!(
            has(
                &one_of(&[Kind::Shape, Kind::Shape], outlineable),
                Item::Flatten
            ),
            "the fixture: Flatten is what a pair gets, so the exclusion is about \
             something rather than about an empty menu"
        );
    }

    /// The three sizing rows tick exactly one, and only over a single text layer
    /// (§5.6): the tick is a fact about one node.
    ///
    /// flip: ticking from a constant `TextSizing::Auto` → fails at the second
    /// assertion; pushing on `all_are(Kind::Text)` without the `Option` → the pair
    /// grows three rows.
    #[test]
    fn the_three_sizing_rows_tick_one_and_only_over_a_single_text_layer() {
        let two = [id(1), id(2)];
        let menu = |n: usize, cell: Option<u8>| {
            let mut cx = open(
                Target::Layer {
                    id: id(1),
                    door: Door::Canvas,
                },
                &two[..n],
                &[Kind::Text, Kind::Text][..n],
            );
            cx.state = LayerState {
                text_sizing: cell,
                ..LayerState::default()
            };
            build(&cx)
        };

        for cell in 0..3u8 {
            let m = menu(1, Some(cell));
            let ticked: Vec<&str> = m
                .iter()
                .flatten()
                .filter(|r| r.checked == Some(true))
                .map(|r| &*r.label)
                .collect();
            assert_eq!(
                ticked,
                [["Auto width", "Auto height", "Fixed size"][usize::from(cell)]],
                "cell {cell} must tick itself and nothing else"
            );
            assert_eq!(
                m.iter()
                    .flatten()
                    .filter(|r| matches!(r.item, Item::TextSizing(_)))
                    .count(),
                3
            );
        }

        // `Some`, not `None`: `layer_menu_state` reads the field off the hit node
        // alone, so a real two-layer selection never has `None`.
        assert!(
            !menu(2, Some(0))
                .iter()
                .flatten()
                .any(|r| matches!(r.item, Item::TextSizing(_))),
            "two text layers have no single state for the tick to report, \
             whatever the hit layer's own state is"
        );
    }

    /// *Convert to path* is text's own row in *Outline shape*'s slot: on text only,
    /// dim on empty content with a reason, and in Structure rather than under *Edit
    /// text*.
    ///
    /// flip: pushing it in `head_rows` beside *Edit text* → fails the Structure
    /// assertion only; dropping the `can_outline_text` dim → fails the dim one only.
    #[test]
    fn convert_to_path_is_texts_row_and_never_beside_outline_shape() {
        let two = [id(1), id(2)];
        let menu = |kinds: &[Kind], outlineable: bool| {
            let mut cx = open(
                Target::Layer {
                    id: id(1),
                    door: Door::Canvas,
                },
                &two[..kinds.len().min(2)],
                kinds,
            );
            cx.text_outlineable = outlineable;
            cx.state = LayerState {
                outlineable: true,
                ..LayerState::default()
            };
            build(&cx)
        };
        let row = |groups: &'_ [Vec<Row>], item: Item| {
            groups
                .iter()
                .flatten()
                .find(|r| r.item == item)
                .map(|r| (r.enabled, r.why))
        };

        let text = menu(&[Kind::Text], true);
        assert_eq!(row(&text, Item::OutlineText), (Some((true, None))));
        assert_eq!(
            row(&text, Item::OutlineShape),
            None,
            "the two never share a menu"
        );
        assert_eq!(
            row(&menu(&[Kind::Text], false), Item::OutlineText),
            Some((false, Some("This text has no glyphs to convert")))
        );

        for kinds in [
            vec![Kind::Shape],
            vec![Kind::Path],
            vec![Kind::Group],
            vec![Kind::Frame],
            vec![Kind::Boolean(BoolOp::Union)],
            vec![Kind::Text, Kind::Text],
        ] {
            assert_eq!(
                row(&menu(&kinds, true), Item::OutlineText),
                None,
                "{kinds:?}"
            );
        }

        // Asserted as which group, since a position passes under any grouping that sorts the same.
        let structure = text
            .iter()
            .find(|g| g.iter().any(|r| r.item == Item::Group))
            .expect("Structure holds Group selection");
        assert!(
            structure.iter().any(|r| r.item == Item::OutlineText),
            "it belongs beside the other structural verbs, not under Edit text: {:?}",
            labels(&text)
        );
    }

    /// *Frame selection* and *Group selection* are both offered on frames, and
    /// *Frame selection* dims where the parent chain bars a frame (an `Artboard`
    /// cannot go under a boolean or a mask).
    ///
    /// flip: dropping the `can_frame` dim → the row is live under a mask; restoring
    /// a `!cx.kinds.contains(&Kind::Frame)` gate around *Group selection* → fails
    /// at "a frame groups".
    #[test]
    fn frame_selection_is_offered_where_group_is_and_on_the_frames_group_refuses() {
        let two = [id(1), id(2)];
        let menu = |kinds: &[Kind], can_frame: bool| {
            let mut cx = open(
                Target::Layer {
                    id: id(1),
                    door: Door::Canvas,
                },
                &two[..kinds.len().min(2)],
                kinds,
            );
            cx.can_frame = can_frame;
            build(&cx)
        };
        let row = |groups: &'_ [Vec<Row>], item: Item| {
            groups
                .iter()
                .flatten()
                .find(|r| r.item == item)
                .map(|r| (r.enabled, r.why))
        };

        let m = menu(&[Kind::Shape, Kind::Shape], true);
        assert_eq!(row(&m, Item::FrameSelection), Some((true, None)));
        assert!(row(&m, Item::Group).is_some(), "the fixture: Group is here");

        let m = menu(&[Kind::Frame], true);
        assert_eq!(row(&m, Item::Group), Some((true, None)), "a frame groups");
        assert_eq!(row(&m, Item::FrameSelection), Some((true, None)));

        // Under a boolean or a mask: present, dim, and saying why.
        let m = menu(&[Kind::Shape], false);
        assert_eq!(
            row(&m, Item::FrameSelection),
            Some((false, Some("A frame cannot go inside a boolean or a mask")))
        );
    }

    /// *Use as mask* dims with the inspector's sentence wherever the verb would
    /// refuse. The sentence is `mask_action`'s, pinned by the inspector's
    /// `every_mask_refusal_says_which_thing_to_fix`.
    ///
    /// flip: deleting the `mask_refused` dim → fails at "dim, with the sentence".
    #[test]
    fn use_as_mask_dims_with_the_sentence_the_inspector_gives() {
        const WHY: &str = "A group holding a frame cannot be a mask; use a shape above it instead";
        let one = [id(1)];
        let mask_row = |refused: Option<&'static str>| {
            let mut cx = open(
                Target::Layer {
                    id: id(1),
                    door: Door::Canvas,
                },
                &one,
                &[Kind::Group],
            );
            cx.mask_refused = refused;
            build(&cx)
                .iter()
                .flatten()
                .find(|r| r.item == Item::Mask)
                .map(|r| (r.enabled, r.why))
        };
        assert_eq!(mask_row(None), Some((true, None)), "the control: live");
        assert_eq!(
            mask_row(Some(WHY)),
            Some((false, Some(WHY))),
            "dim, with the sentence"
        );
    }

    /// *Add point here* is alone in the point menu's head (a group of its own, so
    /// a hairline separates it from *Delete points*), dim with a sentence off the
    /// ink rather than absent, and live on it.
    ///
    /// flip: `Group::Clipboard` in the spec → fails the head assertion only (order
    /// alone would pass); pushing the row only `if cx.on_segment` → fails the
    /// dim assertion only.
    #[test]
    fn add_point_here_leads_the_point_menu_and_dims_off_the_ink() {
        let sel = [id(1)];
        let kinds = [Kind::Path];
        let menu = |on_segment: bool| {
            let mut cx = open(Target::Points, &sel, &kinds);
            cx.on_segment = on_segment;
            build(&cx)
        };

        let on = menu(true);
        assert_eq!(
            on.first().map(|g| flat(std::slice::from_ref(g))),
            Some(vec![Item::AddPointHere]),
            "§6.5 puts it above Delete points and the head is a group of its own, \
             so a hairline separates the two"
        );

        let row = |groups: &'_ [Vec<Row>]| {
            groups
                .iter()
                .flatten()
                .find(|r| r.item == Item::AddPointHere)
                .map(|r| (r.enabled, r.why))
        };
        assert_eq!(row(&on), Some((true, None)));
        assert_eq!(
            row(&menu(false)),
            Some((
                false,
                Some("Right-click on the path's outline to add a point to it")
            )),
            "off the ink the row stays and says why — it does not disappear"
        );
    }

    /// The two property rows dim on different questions (a layer to read from vs.
    /// anything read yet), and *Copy properties* gives a different reason to each
    /// of its refusals.
    ///
    /// flip: collapsing the sentences into one → fails the key-layer assertions;
    /// hanging *Paste properties* on `can_copy_props` → live with an empty clipboard.
    #[test]
    fn the_property_rows_dim_on_two_different_questions() {
        let two = [id(1), id(2)];
        let kinds = [Kind::Shape, Kind::Shape];
        let menu = |n: usize, can_copy: bool, has: bool| {
            let mut cx = open(
                Target::Layer {
                    id: id(1),
                    door: Door::Canvas,
                },
                &two[..n],
                &kinds[..n],
            );
            cx.can_copy_props = can_copy;
            cx.has_props = has;
            build(&cx)
        };
        let keyed = |n: usize| {
            let mut cx = open(
                Target::Layer {
                    id: id(1),
                    door: Door::Canvas,
                },
                &two[..n],
                &kinds[..n],
            );
            cx.can_copy_props = false;
            cx.has_key = true;
            build(&cx)
        };
        let row = |groups: &'_ [Vec<Row>], item: Item| {
            groups
                .iter()
                .flatten()
                .find(|r| r.item == item)
                .map(|r| (r.enabled, r.why))
                .expect("both property rows are on every layer menu")
        };

        let m = menu(1, true, false);
        assert_eq!(row(&m, Item::CopyProperties), (true, None));
        assert_eq!(
            row(&m, Item::PasteProperties),
            (false, Some("No properties have been copied yet"))
        );

        let m = menu(1, true, true);
        assert_eq!(row(&m, Item::PasteProperties), (true, None));

        assert_eq!(
            row(&menu(1, false, true), Item::CopyProperties),
            (false, Some("This layer has no fill or stroke of its own"))
        );
        assert_eq!(
            row(&menu(2, false, true), Item::CopyProperties),
            (
                false,
                Some("Make one of these the key layer to copy from it")
            ),
            "several layers and no key is a different problem from an empty one"
        );
        // A key is set and has no paint; telling the user to set one would be wrong.
        assert_eq!(
            row(&keyed(2), Item::CopyProperties),
            (
                false,
                Some("The key layer has no fill or stroke of its own")
            )
        );
    }

    /// No menu offers the same verb twice, which is what lets a keyboard highlight
    /// be an `Item` rather than an index (§8); asserted over every menu the app
    /// can open.
    #[test]
    fn no_menu_offers_the_same_verb_twice() {
        let all = [id(1), id(2)];
        for target in [
            Target::Canvas,
            Target::PanelBackground,
            Target::Guide(GuideId(id(9))),
            Target::TextSession,
            Target::Points,
            Target::Layer {
                id: id(1),
                door: Door::Canvas,
            },
            Target::Layer {
                id: id(1),
                door: Door::Panel,
            },
        ] {
            for kinds in [
                vec![Kind::Shape],
                vec![Kind::Frame],
                vec![Kind::Group],
                vec![Kind::Boolean(BoolOp::Subtract)],
                vec![Kind::Path],
                vec![Kind::Text],
                vec![Kind::Shape, Kind::Shape],
            ] {
                let sel = &all[..kinds.len().min(2)];
                let groups = build(&open(target, sel, &kinds));
                let rows = flat(&groups);
                assert!(
                    !rows.is_empty(),
                    "{target:?} over {kinds:?} builds nothing, so this asserts \
                     over an empty menu"
                );
                let mut seen: Vec<Item> = Vec::new();
                for item in rows {
                    assert!(
                        !seen.contains(&item),
                        "{target:?} over {kinds:?} offers {item:?} twice: {:?}",
                        labels(&groups)
                    );
                    seen.push(item);
                }
            }
        }
    }

    /// The two rail rows (*Flip to other side*, *Detach from path*) need a rail; an
    /// ordinary text layer or a text-and-shape pair offers neither, since there is
    /// no *Text on path* row (the Text tool's gesture replaced it).
    #[test]
    fn the_rail_rows_need_a_rail_and_the_pair_offers_nothing() {
        let two = [id(1), id(2)];
        let menu = |n: usize, railed: bool| {
            let mut cx = open(
                Target::Layer {
                    id: id(1),
                    door: Door::Canvas,
                },
                &two[..n],
                &[Kind::Text, Kind::Path][..n],
            );
            cx.on_a_rail = railed;
            build(&cx)
        };
        let has = |groups: &[Vec<Row>], item: Item| groups.iter().flatten().any(|r| r.item == item);

        let plain = menu(1, false);
        assert!(
            !has(&plain, Item::DetachTextPath) && !has(&plain, Item::FlipTextPath),
            "an ordinary text layer offers neither: {:?}",
            labels(&plain)
        );

        let pair = menu(2, false);
        assert!(
            !has(&pair, Item::DetachTextPath) && !has(&pair, Item::FlipTextPath),
            "selecting a text layer with a shape offers no rail verb: {:?}",
            labels(&pair)
        );

        let railed = menu(1, true);
        assert!(
            has(&railed, Item::DetachTextPath) && has(&railed, Item::FlipTextPath),
            "a railed layer offers the way back and the flip: {:?}",
            labels(&railed)
        );
    }

    /// *Flip to other side* ticks the state it is in. Asserted as `Some(false)`
    /// rather than "not `Some(true)`", since `None` is a row that is not checkable
    /// at all.
    ///
    /// flip: hard-coding `checked(false)` → fails at "flipped: ticked".
    #[test]
    fn the_flip_row_ticks_the_state_it_is_in() {
        let one = [id(1)];
        let tick = |flipped: bool| {
            let mut cx = open(
                Target::Layer {
                    id: id(1),
                    door: Door::Canvas,
                },
                &one,
                &[Kind::Text],
            );
            cx.on_a_rail = true;
            cx.rail_flipped = flipped;
            build(&cx)
                .iter()
                .flatten()
                .find(|r| r.item == Item::FlipTextPath)
                .map(|r| r.checked)
                .expect("the row is offered on a railed layer")
        };
        assert_eq!(tick(false), Some(false), "unflipped: checkable, unticked");
        assert_eq!(tick(true), Some(true), "flipped: ticked");
    }
}

#[cfg(test)]
mod view_switch_registry_tests {
    //! No `ViewSwitch` borrows another's chord or another's state. Plain backticks
    //! throughout: this is a `#[cfg(test)]` module `cargo doc` cannot see.

    use super::*;

    /// Every switch's chord comes from `ViewSwitch::accel` and no two are the same;
    /// a chordless switch must stay expressible, or the next one gets a borrowed chord.
    ///
    /// flip: restoring a `_ => "Ctrl+'"` wildcard in `Item::spec` → fails at "took
    /// its chord from somewhere other than the switch".
    #[test]
    fn no_two_view_switches_wear_one_chord() {
        let mut seen: Vec<(&str, ViewSwitch)> = Vec::new();
        for sw in ViewSwitch::ALL {
            let from_registry = Item::ViewSwitch(sw).spec().accel;
            assert_eq!(
                from_registry,
                sw.accel(),
                "{} took its chord from somewhere other than the switch",
                sw.label()
            );
            let Some(accel) = sw.accel() else { continue };
            if let Some((_, other)) = seen.iter().find(|(a, _)| *a == accel) {
                panic!(
                    "{} and {} both answer to {accel}",
                    sw.label(),
                    other.label()
                );
            }
            seen.push((accel, sw));
        }
        assert!(
            ViewSwitch::ALL.iter().any(|sw| sw.accel().is_none()),
            "a chordless switch must stay expressible"
        );
    }

    /// Every switch is on exactly one of `VIEW_MENU` and `SNAP_MENU`. Membership,
    /// not cardinality: a length sum passes for two arrays that both hold `Grid`
    /// and neither holds `Focus`.
    ///
    /// flip: dropping `Focus` from `VIEW_MENU` → fails naming "Focus mode" on 0 menus.
    #[test]
    fn every_view_switch_is_on_exactly_one_menu() {
        for sw in ViewSwitch::ALL {
            let on = usize::from(ViewSwitch::VIEW_MENU.contains(&sw))
                + usize::from(ViewSwitch::SNAP_MENU.contains(&sw));
            assert_eq!(
                on,
                1,
                "{} is on {on} of the two menus; a chord with no row is what \
                 `ViewSwitch` exists to make impossible",
                sw.label()
            );
        }
    }

    /// `ViewState` answers for the switches it holds (the ones `view_rows` and
    /// `guide_menu` build rows for) and for nothing else; the set is named, not counted.
    ///
    /// flip: restoring a wildcard in `ViewState::get` → fails on `Layers`, which
    /// comes back `Some(true)`.
    #[test]
    fn view_state_answers_only_for_the_switches_it_holds() {
        let st = ViewState {
            rulers: false,
            guides: false,
            guide_lock: false,
            grid: true,
            layout_grid: false,
        };
        let answered: Vec<&str> = ViewSwitch::ALL
            .iter()
            .filter(|sw| st.get(**sw).is_some())
            .map(|sw| sw.label())
            .collect();
        assert_eq!(
            answered,
            vec![
                "Show rulers",
                "Show guides",
                "Lock guides",
                "Show grid",
                "Show layout grid"
            ],
            "the struct's fields and the switches it answers for are not the same set"
        );
        assert_eq!(st.get(ViewSwitch::Grid), Some(true));
        assert_eq!(
            st.get(ViewSwitch::LayoutGrid),
            Some(false),
            "and the fifth field is read rather than defaulted — `Some(true)` here \
             would be the grid's state answering for its neighbour, which is the \
             wildcard this test exists to keep out"
        );
        assert_eq!(st.get(ViewSwitch::Layers), None);
    }
}

/// A context-menu row's label fitted to the 210pt card.
#[cfg(test)]
mod label_fit_tests {
    use super::*;

    /// The label galley one `ui::menu_row` paints at the menu's content width:
    /// whether it was cut (`Galley::elided`), its right edge, and the row's.
    fn painted(label: &str, accel: Option<&str>) -> (bool, f32, f32) {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut row_right = 0.0;
        let out = ctx.run_ui(Default::default(), |ui| {
            ui.set_width(crate::ui::menu_inner_w(MENU_W, MENU_PAD));
            let row = crate::ui::MenuRow::new(icon::ARROW_COUNTER_CLOCKWISE, label).accel(accel);
            row_right = crate::ui::menu_row(ui, row, crate::ui::MENU_ITEM_H)
                .rect
                .right();
        });
        // The label is the one 11.5pt galley.
        let (cut, right) =
            out.shapes
                .iter()
                .find_map(|c| match &c.shape {
                    egui::Shape::Text(t)
                        if t.galley.job.sections.first().is_some_and(|s| {
                            s.format.font_id == egui::FontId::proportional(11.5)
                        }) =>
                    {
                        Some((t.galley.elided, t.visual_bounding_rect().right()))
                    }
                    _ => None,
                })
                .expect("the label");
        (cut, right, row_right)
    }

    /// A *Reset ‹layer›* label from a long name is cut with an ellipsis inside the
    /// row; every fixed label in the registry still fits whole beside its chord.
    ///
    /// flip: the label's `max_width` made `f32::INFINITY` → fails at "the long
    /// name stays inside the row". The registry loop also bounds `ui::MENU_ACCEL_GAP`.
    #[test]
    fn a_long_reset_label_is_cut_inside_the_row_and_fixed_labels_are_whole() {
        let name = "Card title — a long product name that goes on and on here";
        let label = format!("Reset {name}");
        let (cut, right, row_right) = painted(&label, None);
        assert!(
            right <= row_right - 8.0 + 0.5,
            "the long name stays inside the row: {right} against {row_right}"
        );
        assert!(cut, "and the cut is an ellipsis");

        let (cut, ..) = painted("Reset Label", None);
        assert!(!cut, "a short one is whole");

        for item in Item::ALL {
            let spec = item.spec();
            let (cut, right, row_right) = painted(spec.label, spec.accel);
            assert!(!cut, "{item:?} fits whole");
            assert!(right <= row_right, "{item:?} inside the row");
        }
    }
}

/// *Restore children*, the context-menu row the layers panel's child dot owes.
#[cfg(test)]
mod restore_children_tests {
    use super::*;
    use crate::app::OndinApp;
    use ondin_core::kurbo::Size;
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    /// The labels of the menu a user opens on `id` from the layers panel, through
    /// `open_context_menu` → `open_menu_rows` — not a synthetic `Context`.
    fn labels_on(app: &mut OndinApp, ctx: &egui::Context, id: NodeId) -> Vec<String> {
        labels_through(app, ctx, id, Door::Panel)
    }

    /// `labels_on` through either door.
    fn labels_through(
        app: &mut OndinApp,
        ctx: &egui::Context,
        id: NodeId,
        door: Door,
    ) -> Vec<String> {
        app.session.selection.set_one(id);
        app.open_context_menu(ctx, Target::Layer { id, door }, None);
        let labels = app
            .open_menu_rows()
            .iter()
            .flatten()
            .map(|r| r.label.to_string())
            .collect();
        app.context_menu = None;
        labels
    }

    /// *Hide*/*Show* and *Lock*/*Unlock* say what the action does over the whole
    /// selection, and *Rename* dims in focus mode.
    ///
    /// flip: `layer_menu_state` reading the right-clicked layer's own flags → fails
    /// at the mixed selection's *Hide*; dropping *Rename*'s focus-mode dim →
    /// fails at the last assertion.
    #[test]
    fn the_toggle_labels_read_the_whole_selection() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0xB1D);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [f, a, b, h] = [(); 4].map(|_| ids.mint());
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
        };
        doc.apply(&Transaction(vec![
            create(
                f,
                root,
                NodeKind::Artboard {
                    size: Size::new(100.0, 100.0),
                },
            ),
            create(a, f, rect()),
            create(b, f, rect()),
            create(h, f, rect()),
            Operation::SetVisible {
                id: h,
                visible: false,
            },
            Operation::SetLocked {
                id: h,
                locked: true,
            },
        ]))
        .expect("a frame of three rects, one hidden and locked");
        app.session.adopt_document(doc, None);

        let rows = |app: &mut OndinApp, sel: Vec<NodeId>, id: NodeId| {
            app.session.selection.set(sel);
            app.open_context_menu(
                &ctx,
                Target::Layer {
                    id,
                    door: Door::Panel,
                },
                None,
            );
            let rows: Vec<(Item, String, bool)> = app
                .open_menu_rows()
                .iter()
                .flatten()
                .map(|r| (r.item, r.label.to_string(), r.enabled))
                .collect();
            app.context_menu = None;
            rows
        };
        let label = |rows: &[(Item, String, bool)], item: Item| {
            rows.iter()
                .find(|r| r.0 == item)
                .map(|r| r.1.clone())
                .expect("the row")
        };

        let mixed = rows(&mut app, vec![a, b, h], h);
        assert_eq!(
            label(&mixed, Item::ToggleHidden),
            "Hide",
            "it hides all three"
        );
        assert_eq!(
            label(&mixed, Item::ToggleLocked),
            "Lock",
            "it locks all three"
        );
        let alone = rows(&mut app, vec![h], h);
        assert_eq!(label(&alone, Item::ToggleHidden), "Show");
        assert_eq!(label(&alone, Item::ToggleLocked), "Unlock");

        let rename = |rows: &[(Item, String, bool)]| {
            rows.iter()
                .find(|r| r.0 == Item::Rename)
                .expect("the row")
                .2
        };
        assert!(rename(&rows(&mut app, vec![a], a)), "live on one layer");
        app.focus = true;
        assert!(
            !rename(&rows(&mut app, vec![a], a)),
            "dim in focus mode, where the field's panel is hidden"
        );
    }

    /// The row is offered exactly where the row's dot is and puts back exactly what
    /// its tooltip names. Main `m` holds a rect `A` and a frame `G` holding `X`; in
    /// instance `i` the two top copies are swapped, `G`'s copy of `X` is deleted
    /// and `A`'s copy is renamed. Untouched, nothing offers the row; run on `i`,
    /// only the order comes back; run on `G`'s copy, `X` comes back linked.
    ///
    /// flip: `restore_own_children` built from the subtree-wide resets → fails at
    /// "nothing deeper"; the member arm gated on `drifted` → fails at "a childless
    /// member is not offered it"; the instance arm likewise → fails at the last
    /// assertion.
    #[test]
    fn restore_children_is_offered_where_the_dot_is_and_restores_what_it_names() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0xBCE);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [m, a, g, x] = [(); 4].map(|_| ids.mint());
        let create = |id, parent, kind, name: &str| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind,
            transform: None,
            name: Some(name.into()),
        };
        let rect = || NodeKind::Rect {
            size: Size::new(10.0, 10.0),
            corner_radii: Default::default(),
        };
        let frame = |w: f64| NodeKind::Artboard {
            size: Size::new(w, w),
        };
        doc.apply(&Transaction(vec![
            create(m, root, frame(100.0), "M"),
            create(a, m, rect(), "A"),
            create(g, m, frame(40.0), "G"),
            create(x, g, rect(), "X"),
        ]))
        .expect("a frame holding a rect and a frame");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(m);
        app.create_component();
        app.duplicate_selection();
        let i = app.session.selection.single().expect("the instance");
        assert_eq!(app.session.doc.get(i).unwrap().link(), Some(m));
        let copy_of = |app: &OndinApp, s: NodeId| {
            app.session
                .doc
                .get(i)
                .unwrap()
                .children()
                .iter()
                .copied()
                .find(|c| app.session.doc.get(*c).unwrap().link() == Some(s))
                .expect("a copy")
        };
        let (ai, gi) = (copy_of(&app, a), copy_of(&app, g));
        let xi = app.session.doc.get(gi).unwrap().children()[0];

        for id in [i, gi, ai] {
            assert!(
                !labels_on(&mut app, &ctx, id).contains(&"Restore children".to_string()),
                "untouched: omitted"
            );
        }

        let order = app.session.doc.get(i).unwrap().children().to_vec();
        assert!(app.session.commit(Transaction(vec![
            Operation::Reorder {
                id: order[0],
                index: 1
            },
            Operation::DeleteNode { id: xi },
            Operation::SetName {
                id: ai,
                name: "Mine".into()
            },
        ])));
        assert_ne!(
            app.session.doc.get(i).unwrap().children(),
            order.as_slice(),
            "the fixture: reordered"
        );

        let on_i = labels_on(&mut app, &ctx, i);
        assert!(
            on_i.contains(&"Restore children".to_string()),
            "the instance, its order changed: {on_i:?}"
        );
        let on_gi = labels_on(&mut app, &ctx, gi);
        assert!(
            on_gi.contains(&"Restore children".to_string()),
            "G's copy, a child removed: {on_gi:?}"
        );
        let on_ai = labels_on(&mut app, &ctx, ai);
        assert!(
            on_ai.contains(&"Reset Mine".to_string())
                && !on_ai.contains(&"Restore children".to_string()),
            "a childless member is not offered it: {on_ai:?}"
        );

        app.session.selection.set_one(i);
        let target = Target::Layer {
            id: i,
            door: Door::Panel,
        };
        app.perform_menu_item(&ctx, Item::RestoreChildren, target, None);
        assert_eq!(
            app.session.doc.get(i).unwrap().children(),
            order.as_slice(),
            "the order is back"
        );
        assert_eq!(
            app.session.doc.get(gi).unwrap().children().len(),
            0,
            "nothing deeper: G's copy stays empty"
        );
        assert_eq!(
            app.session.doc.get(ai).unwrap().name(),
            "Mine",
            "no field: A's copy keeps its name"
        );

        app.session.selection.set_one(gi);
        let target = Target::Layer {
            id: gi,
            door: Door::Panel,
        };
        app.perform_menu_item(&ctx, Item::RestoreChildren, target, None);
        let back = app.session.doc.get(gi).unwrap().children().to_vec();
        assert_eq!(back.len(), 1, "X is back");
        assert_eq!(app.session.doc.get(back[0]).unwrap().link(), Some(x));
        assert!(
            !labels_on(&mut app, &ctx, i).contains(&"Restore children".to_string()),
            "and the instance's menu no longer offers it"
        );
    }

    /// A collapsed row's *Restore children* covers what its dot covers. In instance
    /// `i` only grandchildren and a field differ, so expanded `i` is not offered
    /// the row and collapsed `i` is, through both doors, with the tooltip counting
    /// the subtree. Run on the collapsed `i`, `X` comes back linked and `G`'s order
    /// is its source's, but `A`'s copy keeps its name.
    ///
    /// flip: `child_drift_scoped` ignoring `folded` → fails at "collapsed: offered";
    /// the collapsed restore as `reset::reset_all` → fails at "no field"; as
    /// `reset::restore_children` alone → fails at "X is back, linked, and G's
    /// order is its source's".
    #[test]
    fn a_collapsed_rows_restore_children_covers_its_whole_subtree() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0xBCF);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [m, a, g, x, y, z] = [(); 6].map(|_| ids.mint());
        let create = |id, parent, index, kind, name: &str| Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: None,
            name: Some(name.into()),
        };
        let rect = || NodeKind::Rect {
            size: Size::new(10.0, 10.0),
            corner_radii: Default::default(),
        };
        let frame = |w: f64| NodeKind::Artboard {
            size: Size::new(w, w),
        };
        doc.apply(&Transaction(vec![
            create(m, root, 0, frame(100.0), "M"),
            create(a, m, 0, rect(), "A"),
            create(g, m, 1, frame(40.0), "G"),
            create(x, g, 0, rect(), "X"),
            create(y, g, 1, rect(), "Y"),
            create(z, g, 2, rect(), "Z"),
        ]))
        .expect("a frame holding a rect and a frame of three");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(m);
        app.create_component();
        app.duplicate_selection();
        let i = app.session.selection.single().expect("the instance");
        let copy_in = |app: &OndinApp, p: NodeId, s: NodeId| {
            app.session
                .doc
                .get(p)
                .unwrap()
                .children()
                .iter()
                .copied()
                .find(|c| app.session.doc.get(*c).unwrap().link() == Some(s))
        };
        let ai = copy_in(&app, i, a).expect("A's copy");
        let gi = copy_in(&app, i, g).expect("G's copy");
        let (xi, yi) = (copy_in(&app, gi, x).unwrap(), copy_in(&app, gi, y).unwrap());
        let own = app.session.doc.get(i).unwrap().children().to_vec();
        assert!(app.session.commit(Transaction(vec![
            Operation::DeleteNode { id: xi },
            Operation::Reorder { id: yi, index: 1 },
            Operation::SetName {
                id: ai,
                name: "Mine".into()
            },
        ])));
        assert_eq!(
            app.session.doc.get(i).unwrap().children(),
            own.as_slice(),
            "the fixture: i's own children untouched"
        );
        let srcs = |app: &OndinApp, p: NodeId| -> Vec<Option<NodeId>> {
            app.session
                .doc
                .get(p)
                .unwrap()
                .children()
                .iter()
                .map(|c| app.session.doc.get(*c).unwrap().link())
                .collect()
        };
        assert_eq!(
            srcs(&app, gi),
            vec![Some(z), Some(y)],
            "the fixture: X gone, Y and Z swapped"
        );
        let restore = "Restore children".to_string();

        assert!(
            !labels_on(&mut app, &ctx, i).contains(&restore),
            "expanded: its own children are its source's"
        );
        assert!(labels_on(&mut app, &ctx, gi).contains(&restore), "G's copy");

        app.collapsed.insert(i);
        for door in [Door::Panel, Door::Canvas] {
            let on_i = labels_through(&mut app, &ctx, i, door);
            assert!(
                on_i.contains(&restore),
                "collapsed: offered, its dot bubbled from a grandchild ({door:?}): {on_i:?}"
            );
        }
        assert_eq!(
            crate::panels::layers::child_drift_tip(app.child_drift_of(i)).as_deref(),
            Some("1 removed · order changed"),
            "the tooltip counts the subtree"
        );

        app.session.selection.set_one(i);
        let target = Target::Layer {
            id: i,
            door: Door::Panel,
        };
        app.perform_menu_item(&ctx, Item::RestoreChildren, target, None);
        assert_eq!(
            srcs(&app, gi),
            vec![Some(x), Some(y), Some(z)],
            "X is back, linked, and G's order is its source's"
        );
        assert_eq!(
            app.session.doc.get(ai).unwrap().name(),
            "Mine",
            "no field: A's copy keeps its name"
        );

        let on_i = labels_on(&mut app, &ctx, i);
        assert!(
            on_i.contains(&"Reset all".to_string()) && !on_i.contains(&restore),
            "collapsed, lit by a field alone: Reset all only: {on_i:?}"
        );
        assert_eq!(
            crate::panels::layers::child_drift_tip(app.child_drift_of(i)),
            None
        );
    }
}

#[cfg(test)]
mod kit_rows_tests {
    use super::*;
    use crate::app::OndinApp;
    use crate::kits::picker::tests::{app_on, temp_root};
    use ondin_core::kurbo::Size;
    use ondin_core::meta::KitMark;
    use ondin_core::variant::{VariantProp, VariantSet};
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    /// The canvas menu opened on `id`: its kit as `menu_context` computed it, and
    /// the rows' labels.
    fn kit_on(
        app: &mut OndinApp,
        ctx: &egui::Context,
        id: NodeId,
    ) -> (Option<(String, bool)>, Vec<String>) {
        app.session.selection.set_one(id);
        let door = Door::Canvas;
        app.open_context_menu(ctx, Target::Layer { id, door }, None);
        let kinds: Vec<Kind> = app
            .session
            .selection
            .ids()
            .iter()
            .filter_map(|id| app.session.doc.get(*id).map(|n| Kind::of(n.kind())))
            .collect();
        let kit = app
            .menu_context(app.context_menu.as_ref().expect("a menu"), &kinds)
            .kit;
        let labels = app
            .open_menu_rows()
            .iter()
            .flatten()
            .map(|r| r.label.to_string())
            .collect();
        app.context_menu = None;
        (kit, labels)
    }

    /// The kit rows belong to a kit instance's root and do what they say. Kit
    /// *Atlas* publishes a set *Button* of *Small* (holding a *Dot*) and *Large*;
    /// the page holds an instance of *Small*. On the instance the rows read *Go to
    /// main in Atlas* and *Make local…*; on its *Dot* neither is offered.
    /// *Make local…* arms the dialog with the vault top (the set, not the
    /// variant); *Go to main* opens the kit with *Small* selected.
    ///
    /// flip: swapping the two arms of the `AcceptKitUpdates | MakeLocal` dispatch →
    /// fails at "the set, not the variant"; swapping *Go to main*'s arms → fails at
    /// "into the kit".
    ///
    /// Coverage gap: removing `menu_context`'s `instance_root == id` filter leaves
    /// this green, since `main_of` already answers `None` for a member.
    #[test]
    fn the_kit_rows_are_an_instance_roots_and_go_where_they_say() {
        let ctx = egui::Context::default();
        let root = temp_root("menu-kit");
        let mut ids = IdSource::new(0xAE);
        let r = ids.mint();
        let mut kit = Document::new(r);
        let [set, small, dot, large] = [(); 4].map(|_| ids.mint());
        let frame = |id, parent, index, name: &str| Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Artboard {
                size: Size::new(40.0, 30.0),
            },
            transform: None,
            name: Some(name.into()),
        };
        kit.apply(&Transaction(vec![
            frame(set, r, 0, "Button"),
            frame(small, set, 0, "Small"),
            frame(dot, small, 0, "Dot"),
            frame(large, set, 1, "Large"),
            Operation::SetComponent {
                id: small,
                component: true,
            },
            Operation::SetComponent {
                id: large,
                component: true,
            },
            Operation::SetVariantSet {
                id: set,
                set: Some(VariantSet {
                    props: vec![VariantProp {
                        name: "Size".into(),
                        values: vec!["Small".into(), "Large".into()],
                    }],
                }),
            },
            Operation::SetVariant {
                id: small,
                values: vec!["Small".into()],
            },
            Operation::SetVariant {
                id: large,
                values: vec!["Large".into()],
            },
        ]))
        .unwrap();
        let kit_path =
            crate::library::store::file_document(&root, None, "Atlas", &mut kit).unwrap();
        let mut meta = kit.meta().clone();
        meta.kit = Some(KitMark {
            version: 3,
            ..KitMark::default()
        });
        kit.set_meta(meta);
        ondin_core::kit::publish(&mut kit, None, 1).unwrap();
        crate::library::store::write_document(&kit_path, &kit).unwrap();
        let kit_id = kit.meta().id.clone().unwrap();
        let published = kit.published().unwrap().clone();

        let mut app = app_on(&ctx, root.clone());
        let consumer = app.session.path.clone().unwrap();
        let page = app.session.doc.root();
        let (tx, inst) = ondin_core::kit::insert(
            &app.session.doc,
            &mut app.session.ids,
            &kit_id,
            4,
            &published,
            set,
            page,
        )
        .unwrap();
        assert!(app.session.commit(tx));
        let doc = &app.session.doc;
        let main = doc.get(inst).unwrap().link().unwrap();
        let vault_set = doc.get(main).unwrap().parent();
        let child = doc.get(inst).unwrap().children()[0];
        assert!(
            doc.get(child)
                .unwrap()
                .link()
                .is_some_and(|l| doc.in_vault(l)),
            "the fixture: the Dot is a copy of a vault node"
        );

        let (on_root, labels) = kit_on(&mut app, &ctx, inst);
        assert_eq!(on_root, Some(("Atlas".to_string(), false)));
        assert!(
            labels.contains(&"Go to main in Atlas".to_string())
                && labels.contains(&"Make local…".to_string()),
            "{labels:?}"
        );
        let (on_child, labels) = kit_on(&mut app, &ctx, child);
        assert_eq!(
            on_child, None,
            "a layer inside it is not the kit's instance"
        );
        assert!(
            !labels
                .iter()
                .any(|l| l.starts_with("Go to main in") || l == "Make local…"),
            "{labels:?}"
        );

        app.session.selection.set_one(inst);
        let target = Target::Layer {
            id: inst,
            door: Door::Canvas,
        };
        app.perform_menu_item(&ctx, Item::MakeLocal, target, None);
        assert_eq!(app.making_local, vault_set, "the set, not the variant");
        app.making_local = None;

        app.perform_menu_item(&ctx, Item::GoToMain, target, None);
        assert_eq!(
            app.session.path.as_deref(),
            Some(kit_path.as_path()),
            "into the kit"
        );
        assert_eq!(app.session.selection.ids(), &[small], "its main selected");
        app.open_path(&consumer);
        let _ = std::fs::remove_dir_all(&root);
    }
}
