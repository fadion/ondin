//! The context-menu registry (`docs/context-menus.md`).
//!
//! **One list, not a list per menu.** A row's label, glyph, chord, group and verb
//! live in exactly one place here, and every menu is a *filtered projection* of
//! the canonical order in [`Group`] (`docs/context-menus.md` §3). That is what makes
//! the canvas's menu and the layers panel's menu the same menu — identical rows,
//! identical order, identical wording — which is the whole point of a context
//! menu: a *Flatten* that is fourth in one door and seventh in the other is worse
//! than no menu at all.
//!
//! The same shape is what the command palette and the shortcut cheatsheet will
//! read when they arrive (`docs/roadmap.md`, *Command palette*), so nothing here is
//! allowed to know that it is being drawn as a menu.
//!
//! # The two questions a row is asked
//!
//! **Kind decides presence; state decides enabled** (§3). *Ungroup* is not on a
//! rectangle's menu at all, because it could never apply to one; *Paste
//! properties* with nothing copied is present, dimmed, and says why. The one
//! exception is a row whose only purpose is to undo a non-default state —
//! *Reset crop*, *Reset origin* — which is **omitted** when the state is default
//! rather than permanently dim, because a row that is dim on every open is
//! indistinguishable from one that is broken.
//!
//! # What this module is not
//!
//! It does not open, close or place the menu — that is `app.rs`'s
//! `context_menu_ui` and the four rules in §0 — and it does not paint a row, which
//! is `ui::menu_row`. It answers one question: *which rows apply to this target,
//! in what state*.
//!
//! # What is not here
//!
//! The spec's ledger (§9) scores every row it names, and **§9.3 and §9.4 are both
//! empty** — the registry is complete against §9.1–§9.4. The standing rule that
//! emptied them is still the rule: a row that cannot run is a worse promise than a
//! row that is not there.
//!
//! Two rows are absent on purpose and will not come back. Both *Export* rows were
//! **built and then withdrawn** (§15 D264 — a decided non-goal, not work pending
//! an export UI), and the ruler's unit menu went when units were decided against,
//! which **struck** §6.6 rather than reserving it (§15 D358).
//!
//! ⚠️ **One spec'd row is genuinely missing and its absence is recorded nowhere:**
//! a frame's ***Background…*** (§5.1). D400 made a frame's background its fill
//! list and deleted `Operation::SetArtboardBackground`, which is what the row
//! opened a picker onto — but no §15 entry mentions the row, and
//! `docs/context-menus.md` §9.5 still says *"Landed: all of §9.1"*. **Open for the
//! maintainer**: either the row returns against the fill list or the spec line is
//! struck. Do not read its absence here as either answer.
//!
//! ⚠️ This section was headed *"Not yet built"* and listed *Copy/Paste
//! properties*, *Frame selection*, *Outline shape*, *Convert to path* and *Paste
//! in place* as absent until 2026-09-06. **All five ship** (§15 D257, D249, D230,
//! D260, D248), and had for a fortnight. A module head asserting the *absence* of
//! verbs that exist is the same drift as one asserting a feature that was never
//! built, and no gate can see it: every name in the list is prose, not a link.

use crate::theme::icon;
use ondin_core::{BoolOp, GuideId, NodeId, NodeKind, build};

use crate::input::ViewSwitch;

/// Which door the menu was opened through (`docs/context-menus.md` §1).
///
/// The two doors show the *same* rows; today it decides exactly one thing, which
/// is the only difference §1's table has left: whether *Paste* means "here, at
/// the pointer" or "as a sibling above this row".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Door {
    Canvas,
    Panel,
}

/// What the menu was opened on (`docs/context-menus.md` §2 and §6).
///
/// **Decided by what is under the pointer, never by what is selected** (C3).
/// That is the consequence worth stating outright: with five layers selected, a
/// right-click on empty canvas still opens the canvas menu, and that menu carries
/// no selection rows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    /// Empty canvas (§6.1).
    Canvas,
    /// A layer, through either door. `id` is what a left-click at that pixel
    /// would have selected (C1) — the canvas door resolves it through
    /// `pick_for_click`, the panel door is simply the row.
    ///
    /// The *subject* of most rows is still the selection, not this id: a hit
    /// inside the selection leaves it whole (C2), so a menu opened on one of five
    /// picked layers acts on all five. `id` is what the rows that need a single
    /// layer read — *Make this the key layer*, the panel's *Paste*.
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
/// A row is always in the same place relative to the rows that survive beside
/// it, because the projection cannot reorder — it can only drop. Empty groups
/// collapse and take their separator with them.
///
/// **Ten: §3's nine, and one more.** It was eight until 2026-08-20 — *Properties*
/// and *Export* had no built row to hold, so they were absent rather than
/// present-and-dead — and both filled the same day, *Properties* with the
/// `Ctrl+Alt` pair (§15 D257) and *Export* with *Copy as SVG* (§15 D259).
/// **[`Self::Export`] holds both of its rows** — *Copy as SVG* and *Copy as PNG*.
/// It said *"nine-tenths empty … its other row, Export selection…, waits on an
/// export UI nobody has designed"* until 2026-09-06; that row was built and then
/// **withdrawn** (§15 D264), so it is a decided non-goal rather than pending work,
/// and §9.4 is empty.
///
/// [`Self::View`] is the tenth and is not one of §3's at all, because §3 is a
/// *layer*'s menu and those switches are the page's — §9.6 records it as the
/// one remaining difference between this list and that table.
///
/// ⚠️ **"Those four switches" until §15 D757 made them five**, and the number is
/// dropped rather than corrected: nothing here depends on how many there are, and
/// this was the fourth copy of that count in this file. `view_rows` is the list.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Group {
    /// The kind's own verbs — *Edit text*, *Enter group*, *Edit image*. At the
    /// **top**, because these are the "one level in" verbs that are reachable
    /// only by double-click today, and because it is what the user right-clicked
    /// *this particular thing* for.
    Head,
    Clipboard,
    /// *Copy properties* · *Paste properties* — §3's third group, and empty in the
    /// code until 2026-08-20. It sits between the clipboard and the structural
    /// verbs because that is what it is: a second clipboard, carrying an
    /// appearance rather than a layer.
    Properties,
    Structure,
    Order,
    Transform,
    State,
    Navigate,
    /// §3's ninth — *Copy as SVG* · *Copy as PNG*, ordered semantic-then-raster.
    /// *Export selection…* was §3's own first row here and is gone for good
    /// (§15 D264).
    Export,
    /// The workspace switches §6.1 and §6.3 carry — *Show rulers*, *Show
    /// guides*, *Lock guides*, *Show grid*.
    ///
    /// **Not one of §3's nine**, because §3 is about a *layer*'s menu and these
    /// are about the page. Last, which is where §6.1 puts them, and which is also
    /// where §3 says a trim should start.
    View,
}

impl Group {
    /// The canonical order, and the order [`fn@build`] emits in.
    const ORDER: [Group; 10] = [
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
    ];
}

/// One verb the registry can dispatch.
///
/// Most of these are an [`crate::input::Action`] under another name, and
/// [`crate::app::OndinApp::perform_menu_item`] hands them straight to `dispatch`
/// — which is the point: a menu row and its chord must not be able to come to
/// mean different things. The ones that are not are the ones a keyboard cannot
/// express, because they are about *where the menu was opened* (`PasteHere`,
/// `MakeKeyLayer`) or about a target the keymap has no name for (`DeleteGuide`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Item {
    // --- head -------------------------------------------------------------
    /// §5.2 — `entered_group` plus one step of the group chain, which is exactly
    /// what a double-click does.
    EnterGroup,
    /// §5.3 — the operands stay selectable and editable, like Figma's.
    EnterBoolean,
    /// §5.3 — the base operand is *bottom of the list*, so this is a reorder to
    /// index 0. This row and `Alt+Shift`+click are the two ways in; the *result*
    /// is announced, by the layers panel's stack badge (§15 D244).
    MakeBaseOperand,
    /// §5.8 — the layer align aligns *to*, the base operand of a boolean, and,
    /// since §15 D286, **which member of a selection becomes the mask**. Three
    /// consumers, one designation, and the keyed layer wears
    /// `canvas::draw_key_outline`'s trace of its own outline in `color::KEY`.
    MakeKeyLayer,
    /// §5.1 — a frame's signature property, and the only kind where `Node::clip`
    /// means anything.
    ClipContent,
    /// §5.6 — `begin_edit_text`, today double-click only.
    EditText,
    /// §5.6's three checkable rows over `TextSizing` — auto width, auto height,
    /// fixed size — carrying the panel's cell index.
    ///
    /// **A deliberate duplicate of the Type panel's segmented control**, which is
    /// the one place this registry repeats a control rather than reaching a verb
    /// nothing else offers. §5.6 says why: *Auto width* is the most reached-for
    /// text command in Figma and right-click is where a Figma hand goes for it.
    /// Both doors call `set_text_sizing`, so the row and the segment cannot come
    /// to mean different things — §8's rule, and the reason the panel's body was
    /// lifted into a method to build this.
    ///
    /// **The labels are this menu's own, not `TextSizing::LABELS`.** Those read
    /// "Auto W" / "Auto H" / "Fixed" because three cells of a segmented control
    /// have no room; a menu row has a whole line, and an abbreviation there would
    /// be a panel's constraint leaking into a surface that does not share it.
    TextSizing(u8),
    /// §5.4 — the node tool.
    ///
    /// The pair with [`Self::EditImage`] was broken for a while and is symmetric
    /// again: *Edit crop…* stood beside this row until it went with the crop tool
    /// (§15 D268), which left image editing the one "one level in" verb reachable
    /// only by double-click or `Enter`. That was left as a question to re-ask
    /// rather than an oversight, and re-asking it in 2026-08-22 put the row back
    /// under the mode's new name.
    EditPoints,
    /// §5.7 — `Tool::ImageEdit`, the mode that replaced the crop tool (§15 D268).
    ///
    /// **The head verb for a picture**, and the repair `context-menus.md` §5
    /// argues the head exists to provide: every other "one level in" verb has a
    /// row here, and for two days this one did not.
    ///
    /// **Named for the mode rather than for the crop.** The design wrote it
    /// *Edit image…* — inherited from *Edit crop…*, which earned its ellipsis
    /// back when it opened something. This one steps into a mode exactly as
    /// *Edit text* and *Edit points* do, and neither of those carries one; an
    /// ellipsis here would promise a dialog that does not exist.
    EditImage,
    /// §5.7 — the picture's own pixels, *turned* for a turned picture.
    ///
    /// **The whole of §5.7's head since 2026-08-23**, beside [`Self::EditImage`].
    /// It is here rather than in the image-editing card because it is the one verb
    /// of the five that a hand reaches for *after* playing with a picture and
    /// wanting the layer back at its true size — which is a thing you want without
    /// being in the mode, and the card required entering one to offer it.
    ///
    /// **Three rows went the other way**, into the Settings tab alone: *Reset crop*
    /// and *Replace…*, which are useful but not often enough to earn a place in a
    /// menu that opens on every layer, and *Export original…*, which was removed
    /// from both doors — the Export panel offers several sizes and formats, so a
    /// row writing the stored bytes back out unchanged is not what a hand reaches
    /// for (§15 D225 built it, and this is where it ends).
    OriginalSize,

    // --- clipboard --------------------------------------------------------
    Cut,
    Copy,
    /// Canvas: at the pointer. Panel: as a sibling above the clicked row.
    ///
    /// **The one row whose behaviour the door changes**, and the reason the
    /// canvas menu exists on empty canvas at all (C3).
    PasteHere,
    /// Canvas only (§6.1): the clipboard back at the coordinates it was copied from.
    ///
    /// **Not a door on [`Self::PasteHere`]**, the way the panel's paste is, because
    /// the two answer the same question differently rather than in different places:
    /// one is aimed at the pointer and one refuses to be aimed at all. Both are
    /// offered on empty canvas at once, so a door could not tell them apart anyway.
    PasteInPlace,
    Duplicate,
    Delete,

    // --- properties (§3's third group) ------------------------------------
    /// Lift the key layer's appearance — fills, strokes, opacity — onto a
    /// clipboard of its own (`Ctrl+Alt+C`, `build::properties_of`).
    ///
    /// **A chord *and* a row, unlike the pointer-aimed pastes**, because there is
    /// nothing positional about it: `shortcuts.md` §7 has owed Figma's chord since
    /// the section was written, and the row is the discoverable half of the same
    /// verb. Both go through [`crate::app::OndinApp::properties_source`], so the
    /// row cannot copy from a different layer than the key does.
    CopyProperties,
    /// Give the selection the appearance the clipboard is holding (`Ctrl+Alt+V`,
    /// `build::paste_properties`).
    ///
    /// **§3's example of a row dimmed by state**, and it is where that rule was
    /// first written down: *Paste properties* with nothing copied is the case the
    /// spec names. It says so rather than vanishing, because the row is how anyone
    /// learns the pair exists at all.
    PasteProperties,

    // --- structure --------------------------------------------------------
    Group,
    /// Wrap the selection in a new **frame** sized to its union (`build::frame`,
    /// §15 D249).
    ///
    /// [`Self::Group`]'s sibling, in the slot `context-menus.md` §4 gives it —
    /// directly after it, before *Ungroup*. Where *Group selection* is absent
    /// because a frame cannot be grouped, this one is present: frames nest.
    FrameSelection,
    Ungroup,
    /// *Use as mask* — the layer clips the ones above it (`build::mask`, §15
    /// D282, D286).
    ///
    /// **A row here reverses `context-menus.md` §9.3's non-row**, which refused it
    /// on the grounds that nothing in the model expressed a mask. That reason
    /// expired when the model did express one.
    ///
    /// **Checkable, not two labels.** The verb is a toggle, and the row it sits
    /// among — the four booleans — already spends the check mark on exactly this
    /// question: is the thing in front of you already that. A second label would
    /// make the row read as two different commands sharing a slot.
    Mask,
    /// Read this layer's own geometry **even-odd** instead of by winding
    /// (§15 D239).
    ///
    /// **A toggle rather than two rows**, exactly as *Use as mask* is: there are two
    /// rules and the off-state is worth showing, where four booleans need four
    /// labels. It is offered only on a `Path`, which is the one authored kind whose
    /// shape can differ between the rules — a rect, an ellipse, a star and a polygon
    /// are single non-crossing outlines, so a rule on them would be a control that
    /// visibly does nothing.
    ///
    /// ⚠️ **Not offered on a boolean, and that is not an oversight.** An `Exclude`
    /// is even-odd because that is what a symmetric difference *is*, and
    /// `Node::fill_rule` derives it from the operation — a row that appeared to
    /// toggle it would be offering to make the shape wrong.
    EvenOdd,
    /// Wrap the selection in a boolean, **or** switch a selected one's operation
    /// — `apply_boolean` decides which from the selection, so one item covers
    /// both and the two cannot drift apart.
    Boolean(BoolOp),
    Flatten,
    /// §4 / §5.4 / §5.5 — one shape becomes the `Path` its outline traces
    /// (`build::outline`, §15 D230).
    ///
    /// **Not a second spelling of [`Self::Flatten`]**, which is why it is a second
    /// item: *Flatten* means "throw the operands away and keep the answer" and its
    /// subject is a boolean or a set, where this means "stop describing this shape
    /// parametrically". `build::flatten` still refuses a lone non-boolean, and the
    /// two rows are never both offered.
    OutlineShape,
    /// §5.6 — a text layer becomes the `Path` its **glyphs** trace
    /// (`build::outline_text`). Affinity's *Convert Text to Curves*.
    ///
    /// **A third row rather than a second kind for [`Self::OutlineShape`]**, and
    /// the split is the one `build::can_outline` already drew: every other kind's
    /// outline is a pure function of its own geometry, and text's is a *shaped*
    /// thing that needs the resolved layout. The two sit in the same slot and are
    /// mutually exclusive by kind, so no menu ever shows both.
    ///
    /// **Its label is *Convert to path*, not *Outline text*.** §5.6 names it, and
    /// the wording matters more here than it does next door: this is a one-way
    /// door — the string is gone and no amount of anchor editing brings it back —
    /// and "convert" is the word that says a thing became another thing where
    /// "outline" reads as a decoration you could take off again.
    OutlineText,
    /// §5.6 — take a text layer off its rail, leaving the rail as a path layer.
    ///
    /// **A one-layer verb, and offered on a single layer only**: `layer_menu`
    /// pushes it under `one && cx.on_a_rail`, so the row exists exactly when the
    /// one thing clicked is type that is already on a curve.
    ///
    /// ⚠️ **There is no *Text on path* row to pair with, and its absence is the
    /// decision** (§15 D409): setting type on a curve is the Text tool's own
    /// gesture now — hover an edge, click, type — so a menu row asking for a text
    /// layer and a shape to be selected *together first* was a second, worse way
    /// in. This is the way back out and has no gesture of its own, which is what
    /// keeps it here.
    DetachTextPath,
    /// §5.6 — run the type the other way along its rail, and so along the other
    /// side of it (§15 D406).
    ///
    /// **Checkable rather than two rows**, because it is one state with two
    /// values and the tick is what says which — the shape [`Self::ClipContent`]
    /// already has. A pair of rows would have to name the two sides, and *which*
    /// side is "the other" depends on which way the user happened to draw the
    /// curve, so neither name would be true of every rail.
    FlipTextPath,

    // --- order / transform / state / navigate -----------------------------
    Restack(build::ZMove),
    Flip(build::Axis),
    /// `SetPivot { pivot: None }` — "not moved", which is a state the model can say
    /// and which resolving the pivot to a point cannot report back. Omitted unless
    /// the pivot has been moved (§3's exception).
    ///
    /// **No longer the only door**: the Transform card's origin row un-says it too,
    /// by putting both percentages on 50 — the same rule the marker follows when it
    /// is dragged back to the middle, and one function in core
    /// (`geometry::pivot_placed_at`) so the three cannot disagree. This stays
    /// because it says so in one click over a whole selection, where the row is one
    /// layer's and only while its handle is armed.
    ResetOrigin,
    ToggleHidden,
    ToggleLocked,
    Rename,
    ZoomSelection,

    // --- export (§3's ninth group) ----------------------------------------
    //
    // **There was an *Export as…* row here and it was removed on 2026-08-22**, with
    // `Ctrl+Shift+E` and `OndinApp::export_selection` behind it. It asked where,
    // what format and at what size every time and remembered none of it; the
    // inspector's Export panel (§15 D274) answers all three from the layer and has
    // a button that runs the export, so the one-off had become the slower of two
    // doors onto the same file rather than the only one. Removed as a whole verb
    // rather than as a row — a chord with no menu row is the reachability failure
    // `context-menus.md` §5 argues against, wearing different clothes.
    /// Put the selection on the system clipboard as an SVG document
    /// (`ondin_export::svg::svg_of`).
    ///
    /// **It was the only way artwork left this app without the CLI**, for the few
    /// hours until *Export as…* landed the same day (§15 D264) — which is why it
    /// was worth having before the export UI §9.4's two *Export* rows waited on:
    /// those two needed a file dialog, a format picker and a scale, and this needed
    /// a clipboard. Same renderer either way — the semantic writer, so what lands in
    /// the other application is editable groups and shapes rather than a picture of
    /// them.
    ///
    /// [`Self::CopyAsPng`] is the raster half, and for two days it did not exist:
    /// this comment scoped it out because the raster path was viewport-scoped with
    /// no way to say "only these layers". That was true and stopped being true
    /// when D264 built `scene::build_of` for *Export as…*.
    CopyAsSvg,
    /// §3's Export group — the selection as a **picture** on the clipboard, 1:1
    /// (`ondin_export::png::raster_of`). Figma's *Copy as PNG*.
    ///
    /// **Scoped out by [`Self::CopyAsSvg`] until 2026-08-22, on a ground that had
    /// gone.** A PNG of a selection needs a raster walk that can name layers, and
    /// D264 had to build one — `scene::build_of` — for *Export as…*; the objection
    /// was a *renderer* gap and the renderer closed it for another row (§15 D259,
    /// D264).
    ///
    /// **It does not encode a PNG.** `png_of` returns encoded bytes and the
    /// clipboard does not want those: `arboard::ImageData` takes raw RGBA, which is
    /// what `raster_of` returns before either encoder sees it. "PNG" names what the
    /// *receiving* application is handed once the OS converts, not anything this
    /// row writes — reaching for `png_of` here is the plausible mistake, and it
    /// pastes as garbage rather than failing.
    ///
    /// **No scale submenu.** Figma offers one *Copy as PNG* and no 1×/2×/3×
    /// (checked on the machine, 2026-08-22); a multiplier is what the inspector's
    /// Export panel is for (§15 D274), and this row keeps no settings because a
    /// clipboard write has nowhere to keep them — the reason *Export as…* gave for
    /// keeping none was the same, and it is the row the panel then replaced.
    CopyAsPng,

    // --- canvas and panel background (§6.1, §6.2) -------------------------
    SelectAll,
    ZoomFit,
    /// Re-run every export setting in the document, into the folder the last
    /// export went to (§7).
    ///
    /// **The repeat, and since 2026-08-22 the only menu row that writes a file.**
    /// It asks nothing, because the layers already carry the answers and the
    /// destination is remembered — which is what makes a `.ondin` file a build
    /// input rather than a place assets are copied out of by hand. The one-off it
    /// used to be contrasted with, *Export as…*, went when the Export panel's own
    /// button made it the slower of two doors onto the same file.
    ExportAll,
    ViewSwitch(ViewSwitch),

    // --- guides (§6.3) ----------------------------------------------------
    DeleteGuide,
    /// A loop of `RemoveGuide` over everything the document holds.
    ClearGuides,

    // --- points (§6.5) ----------------------------------------------------
    /// Add an anchor to the path's outline **where the menu was opened**.
    ///
    /// One of the handful of rows a keyboard cannot express (§15 D215): the verb
    /// is `insert_point`, which the pen bias and a double-click both already
    /// reach, and what only a menu can supply is *which* point on the ink. So it
    /// carries no accelerator and never will.
    ///
    /// **Dim rather than absent when the click missed the ink.** §3's split says
    /// kind decides presence and state decides enabled, and where the right-click
    /// landed is state — a point selection always has an outline this could apply
    /// to, so the row belongs to the target. It is live on most opens, which is
    /// the test §3 sets against a row that is dim every time.
    AddPointHere,
    /// Delete the point selection, healing each subpath — **or the selected
    /// segment, leaving the gap**, which is the same dispatch.
    ///
    /// `docs/context-menus.md` §6.5 asks for two rows so that the two verbs `Del`
    /// carries (§15 D120) can finally be told apart. The model will not allow two
    /// rows: `PointSet` holds anchors **or** segments and never both, so one of
    /// the pair would be dim on every open — which §3 says is indistinguishable
    /// from a broken row. So it is one row that **renames itself** to whichever
    /// verb the selection has armed, which shows the distinction where the spec
    /// wanted it shown and cannot offer the one that is unreachable.
    DeletePoints,
    /// *Make corner* / *Make smooth* over the point selection — Illustrator's
    /// *Convert to corner/smooth*, and `context-menus.md` §6.5 (§15 D250).
    ///
    /// **Two rows, unlike [`Self::DeletePoints`] above**, and the difference is
    /// worth stating because the two look like the same problem: `Del`'s pair
    /// cannot both be offered — `PointSet` holds anchors *or* segments — where a
    /// point selection can perfectly well hold a corner and a smooth point at once.
    /// Both rows are then live and both mean something, so collapsing them into one
    /// that renames itself would have to pick a name for a mixed selection and would
    /// take away half of what the user can ask for.
    SetSmooth(bool),
    ReverseSubpaths,
    /// Fuse the two selected open ends — close one run, or splice two into one
    /// (`context-menus.md` §6.5, `tools::join_ends`).
    ///
    /// **A second door onto a behaviour that had only a gesture.** The pen has
    /// joined since the pen existed, by being drawn *into* an open endpoint; what
    /// that cannot express is two ends already sitting where they belong, which is
    /// what an import or a flattened boolean leaves. So the subject here is the
    /// point selection rather than the pointer, and the two share
    /// `tools::join_runs` so they cannot come to orient a splice differently.
    Join,
}

/// A layer's kind, as much of it as a menu needs.
///
/// **Not `NodeKind`**, which carries a `BezPath` and a text buffer: a menu is
/// rebuilt on every frame it is open, and cloning a path per frame to ask "is
/// this a path" is the kind of cost that never shows up in a profile and is
/// still wrong. `Copy`, so the selection's kinds are a plain slice.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Frame,
    Group,
    Boolean(BoolOp),
    Path,
    Text,
    /// Rect, ellipse, polygon, star, line — §5.5's "no head at all", and the
    /// shortest menu in the app.
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
/// A struct rather than a closure over the app, so that building a menu takes no
/// borrow of anything and a test can build one from five bools.
///
/// ⚠️ **Four until §15 D757.** *Show layout grid* was added to the top-bar View
/// menu by D755 and deliberately not to this one, which left *Show grid* in both
/// menus and its neighbour in one; the maintainer's answer is that the canvas menu
/// carries it too. **The four-ness was never a design fact** — `context-menus.md`
/// §6.1 states which rows are in and which three are deliberately out, and says
/// nothing about this struct.
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
    /// 🚨 **The `_ => self.grid` this replaces answered for *seven* other
    /// variants with the grid's state** (§15 D675, `[S15.2-L3-07]`). The struct has
    /// four fields and `crate::input::ViewSwitch` had **eleven at the time**, so
    /// there was no value it *could* return for `Layers`, `Toolbar`, `Present` or
    /// the four `Snap` switches — and the wildcard made the absence look like an
    /// answer. The finding counted the enum at seven variants and it was eleven,
    /// which is the same defect one level up: **a number in prose is not a gate**,
    /// and this match now is one.
    ///
    /// ⚠️ **Both numbers are as-of-D675 and are left in the past tense on
    /// purpose** — the enum is twelve now (§15 D755 added `LayoutGrid`, which
    /// answers `None` here for the same reason the seven do). Rewriting them to
    /// today's count would make the account of the defect wrong, and updating them
    /// every time the enum grows is the maintenance this paragraph is *about*.
    ///
    /// Not reachable today — `view_rows` and `guide_menu` between them ask only
    /// the five this holds, and `Item::ALL` lists only those — so this is a trap
    /// rather than a bug, and the file is otherwise built on the opposite habit and
    /// says so repeatedly: `LayerState::text_sizing` is an `Option` *"so the one
    /// state that has no answer cannot be spelled"*. `context-menus.md` §6.1 keeps
    /// *Show layers*, *Show toolbar* and *Present mode* out of the canvas menu
    /// deliberately, so the day that is reversed the row shipped with the grid's
    /// tick and the grid's chord and nothing said a word.
    pub fn get(self, sw: ViewSwitch) -> Option<bool> {
        match sw {
            ViewSwitch::Rulers => Some(self.rulers),
            ViewSwitch::Guides => Some(self.guides),
            ViewSwitch::GuideLock => Some(self.guide_lock),
            ViewSwitch::Grid => Some(self.grid),
            ViewSwitch::LayoutGrid => Some(self.layout_grid),
            // Spelled out rather than left to a wildcard, which is the whole
            // repair: the next `ViewSwitch` is a compile error here. It has already
            // been one once — `LayoutGrid` (§15 D755) landed on the arm below
            // because the compiler put its author on this line, and §15 D757 then
            // moved it up here when the maintainer gave it a row. **Both steps went
            // through this `match` and neither could have been forgotten**, which
            // is what the wildcard cost.
            ViewSwitch::Layers
            | ViewSwitch::Toolbar
            | ViewSwitch::Present
            | ViewSwitch::SnapShapes
            | ViewSwitch::SnapGuides
            | ViewSwitch::SnapGrid
            | ViewSwitch::SnapBaselines => None,
        }
    }
}

/// The fixed half of a registry entry: what a row says and where it belongs,
/// before any state is consulted.
///
/// Everything here is `'static`, which is what lets `Item::ALL` be walked by a
/// test asserting that no row was ever added without a glyph or a group (§10).
pub struct Spec {
    pub label: &'static str,
    pub glyph: &'static str,
    /// The chord this row also answers to, exactly as `docs/shortcuts.md` spells it.
    /// `None` where there is no chord — *Enter group*, *Make this the key layer*,
    /// *Clear all guides* — which is most of what a menu is *for*.
    pub accel: Option<&'static str>,
    pub group: Group,
    /// Destroys something: red, per `design/Editor.dc.html`'s one red.
    pub danger: bool,
}

impl Item {
    /// Every item in the registry, for the table-driven test in §10 — which is
    /// what stops the twelfth row being added without a glyph.
    ///
    /// **`cfg(test)`, because that test is its only consumer and saying so is
    /// more honest than an `allow`.** The list is written by hand, so it cannot
    /// police the enum on its own; what makes it answerable is the other half of
    /// that test, which builds every menu this app can open and demands each row
    /// it finds is *in* here.
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
        Item::Duplicate,
        Item::Delete,
        Item::CopyProperties,
        Item::PasteProperties,
        Item::Group,
        Item::FrameSelection,
        Item::Ungroup,
        Item::Mask,
        Item::EvenOdd,
        Item::Boolean(BoolOp::Union),
        Item::Boolean(BoolOp::Subtract),
        Item::Boolean(BoolOp::Intersect),
        Item::Boolean(BoolOp::Exclude),
        Item::Flatten,
        Item::OutlineShape,
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
        Item::CopyAsPng,
        Item::SelectAll,
        Item::ZoomFit,
        Item::ExportAll,
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
            // Three glyphs rather than one repeated, because the three states are
            // *pictures* — a box that grows sideways, one that grows downwards, one
            // that does neither — and a menu with the same icon three times over
            // three different labels reads as generated.
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
            // `icon::CROP` and not `IMAGE`: the rail's *Place image* button already
            // wears `IMAGE`, and this row is the one that steps *into* a picture
            // rather than putting one down. No accel, matching *Edit text*, which
            // `Enter` also opens — the head's keyboard doors are contextual and
            // none of them is spelled out in the column.
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
            Item::Duplicate => s(
                "Duplicate",
                icon::COPY_SIMPLE,
                Some("Ctrl+D"),
                Group::Clipboard,
            ),
            // `SLIDERS_HORIZONTAL` for the pair, which is the panel-of-settings
            // picture and the nearest thing to "the properties" in the set. Both
            // rows wear the *same* glyph deliberately: they are one clipboard read
            // two ways, exactly as *Copy* and *Paste* above share nothing but sit
            // together, and the accelerator column is what tells the pair apart.
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
                // **No accelerator, and that is a statement rather than an
                // omission.** `shortcuts.md` is the keymap's source of truth and it
                // does not bind this; Figma's `Ctrl+Alt+G` is free here, but a chord
                // invented at a menu row is a chord that file does not know about
                // (§15 D249).
                None,
                Group::Structure,
            ),
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
            // No accelerator: `shortcuts.md` reserves none, and this is a property
            // of a shape rather than a verb anyone reaches for repeatedly.
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
            // **Deliberately unbound** (`shortcuts.md`), and this is the third reason
            // an `Item` is not an `Action`: not "the keyboard cannot express it" but
            // "no key was spent on it". A one-way structural conversion is not a
            // thing to make one keystroke away.
            Item::OutlineShape => s("Outline shape", icon::POLYGON, None, Group::Structure),
            // The same `POLYGON` its sibling wears, because §8's rule that a verb
            // whose whole result is a kind should wear that kind's picture applies
            // to both — what they produce is a `Path` either way. The two are never
            // in one menu, so there is nothing to tell apart.
            Item::OutlineText => s("Convert to path", icon::POLYGON, None, Group::Structure),
            // `TEXT_T`, not `POLYGON`: §8's rule is that a verb whose whole result
            // is a kind wears that kind's picture, and what these two produce is a
            // text layer — straightened or turned over, but text either way. The
            // pair above produce a `Path` and wear its picture for the same reason.
            Item::DetachTextPath => s("Detach from path", icon::TEXT_T, None, Group::Structure),
            // **"Flip to other side", not "Reverse direction"**, though it is both:
            // the side is what a designer is looking at and the direction is the
            // mechanism that gets there. Naming the mechanism would leave the row
            // sounding like it does half of what it does.
            Item::FlipTextPath => s("Flip to other side", icon::TEXT_T, None, Group::Structure),

            Item::Restack(mv) => s(
                match mv {
                    build::ZMove::Front => "Bring to front",
                    build::ZMove::Forward => "Bring forward",
                    build::ZMove::Backward => "Send backward",
                    build::ZMove::Back => "Send to back",
                },
                // **Arrow-to-a-bar for *all the way*, plain arrow for *one
                // step*** — the maintainer's ruling on §15 D672, recorded as D761.
                // `Front` and `Back` both drew `icon::STACK`, two rows apart in one
                // emitted group, which is D225's *"one picture for two verbs"*.
                //
                // ⚠️ **It is a deliberate deviation from `design/Editor.dc.html`**,
                // which §8 makes normative for what a row *looks like* and which
                // gives `ph-stack` / `ph-stack-simple`. The export's own answer
                // does not remove the duplication, it moves it: `stack-simple` is
                // *Flatten* there, and `restack_rows` emits the Order four
                // unconditionally while *Flatten* is pushed on any 2+ selection, so
                // the two co-occur. This scheme is the only one of the three with
                // no picture used twice.
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

            // `SHARE_NETWORK` — the picture of a thing leaving for somewhere else,
            // and unused in this registry. It was picked to stay off `EXPORT`,
            // which was then *Export original…*'s — a row that copies a stored file
            // out where this one renders (§15 D225). That row is gone (§15 D305)
            // and the distinction it was avoiding with it; the glyph stays because
            // it is the better picture for a clipboard write, not because anything
            // else has a claim on `EXPORT`.
            Item::CopyAsSvg => s("Copy as SVG", icon::SHARE_NETWORK, None, Group::Export),
            // `FILE_IMAGE`, which the Export panel's format dropdown already wears
            // for the raster formats — the two rows are the same choice made in two
            // places, and `SHARE_NETWORK` beside it is the *semantic* writer.
            Item::CopyAsPng => s("Copy as PNG", icon::FILE_IMAGE, None, Group::Export),
            // **`EXPORT` is this registry's alone now, and its justification has
            // expired twice rather than been decided.** It was first defended as
            // *the same verb as* Export as… *at another subject* (that row went,
            // §15 D264), then as harmlessly colliding with *Export original…*
            // because a layer row and a page row cannot meet (that row went too,
            // §15 D305). No second claimant is left, so nothing here is being
            // argued — §8's "same verb, two pictures" question was never answered,
            // only made moot, and the next row wanting `EXPORT` has to argue it
            // rather than inherit this.
            Item::ExportAll => s(
                "Export all",
                icon::EXPORT,
                Some("Ctrl+Alt+E"),
                Group::Export,
            ),

            Item::SelectAll => s("Select all", icon::SCAN, Some("Ctrl+A"), Group::Clipboard),
            Item::ZoomFit => s(
                "Zoom to fit",
                icon::FRAME_CORNERS,
                Some("Ctrl+1"),
                Group::Navigate,
            ),
            // 🚨 **Both matches used to close on a wildcard, and the accelerator's
            // gave seven other switches the grid's chord** (§15 D675,
            // `[S15.2-L3-07]`). The chord now comes from `ViewSwitch::accel`, beside
            // the label it already came from, so there is nothing here to fall
            // through. The glyph stays local because it is this registry's choice
            // rather than the switch's.
            Item::ViewSwitch(sw) => s(
                sw.label(),
                match sw {
                    ViewSwitch::Rulers => icon::RULER,
                    ViewSwitch::Guides => icon::LINE_VERTICAL,
                    ViewSwitch::GuideLock => icon::LOCK_SIMPLE,
                    ViewSwitch::Grid => icon::GRID_FOUR,
                    // **The glyph the feature already wears**, rather than a new
                    // pick (§15 D757). `inspector`'s Layout grid card draws
                    // `GridAxis::Columns` with this very constant, so the menu row
                    // and the panel it is about are one picture — which is
                    // `context-menus.md` §8's direction, and the converse of §15
                    // D225's *one glyph cannot be two verbs*. `GRID_FOUR` is spent
                    // on the row directly above and `SQUARE_SPLIT_HORIZONTAL`, the
                    // other candidate, is the stroke-alignment glyph.
                    //
                    // ⚠️ **It reads as the columns axis and the row is about both
                    // axes**, which is a real cost and is accepted: the label says
                    // *Show layout grid*, and the alternative was a glyph with no
                    // tie to the feature at all.
                    ViewSwitch::LayoutGrid => icon::COLUMNS,
                    // ⚠️ **Not a pick, a placeholder.** No menu builds a row for
                    // any of these — `view_rows` and `guide_menu` ask for the five
                    // above and `Item::ALL` lists only those — so none of them has
                    // ever been drawn. Spelled out rather than left to a `_` so the
                    // day one lands, the compiler has already put the author on
                    // this line; `context-menus.md` §8 wants the verb's own glyph
                    // named in the registry, and `SLIDERS_HORIZONTAL` is what a
                    // switch looks like when nobody has chosen yet.
                    //
                    // ⚠️ **That day came for `LayoutGrid` and the arm worked** — it
                    // sat in this list wearing the placeholder from §15 D755 until
                    // D757 gave it a row, and giving it one meant moving it out,
                    // which is the line above.
                    ViewSwitch::Layers
                    | ViewSwitch::Toolbar
                    | ViewSwitch::Present
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

            // **`Head`, which is what puts it above *Delete points*** — §6.5's own
            // order, and the group's meaning holds: it is this target's own verb,
            // the one thing the point menu can do that nothing else is a door onto.
            Item::AddPointHere => s("Add point here", icon::PLUS, None, Group::Head),
            Item::DeletePoints => Spec {
                danger: true,
                ..s("Delete points", icon::TRASH, Some("Del"), Group::Clipboard)
            },
            // In `Structure` beside *Reverse subpath direction*: both change what
            // the path *is* rather than removing part of it, which is what keeps
            // them out of the Clipboard group *Delete points* sits in.
            Item::SetSmooth(true) => s("Make smooth", icon::BEZIER_CURVE, None, Group::Structure),
            // `LINE_SEGMENT` against `BEZIER_CURVE`: a straight run against a
            // curved one is the picture of the difference, where two variants of
            // one glyph would need reading rather than seeing.
            Item::SetSmooth(false) => s("Make corner", icon::LINE_SEGMENT, None, Group::Structure),
            // **"Reverse subpath", not "Reverse subpath direction".** The longer
            // name ran *under* its own accelerator: label and accel are two
            // `Painter::text` calls at fixed anchors, so nothing clips and nothing
            // reflows — the two simply overlap. Reported from the machine with the
            // shorter name attached, and it says the same thing: a subpath has one
            // property you can reverse.
            Item::ReverseSubpaths => s(
                "Reverse subpath",
                icon::ARROWS_CLOCKWISE,
                Some("Shift+D"),
                Group::Structure,
            ),
            // **`ARROWS_MERGE`, which the inspector already spends on *Flatten*** —
            // two lines converging into one, and the picture is right for both
            // because both verbs *are* that. It is unused elsewhere in this
            // registry, and the two can never appear together: one is a layer's row
            // and this is the point menu's.
            Item::Join => s("Join", icon::ARROWS_MERGE, None, Group::Structure),
        }
    }
}

/// One row, as a particular menu open resolved it.
pub struct Row {
    pub item: Item,
    /// Usually the spec's, overridden where the state renames the verb — *Show*
    /// for *Hide*, *Unlock* for *Lock*.
    pub label: &'static str,
    /// `Some` on a checkable row; the glyph column carries it (see
    /// [`crate::ui::MenuRow::checked`]).
    pub checked: Option<bool>,
    pub enabled: bool,
    /// Why this row is dim. **Required whenever `enabled` is false**, because a
    /// dimmed row that does not say why is the state where the sentence carries
    /// the most information and the one where it is easiest to leave out.
    pub why: Option<&'static str>,
    /// Where this row goes, when that is not [`Spec::group`].
    ///
    /// **The one thing a menu is allowed to say about placement**, and it exists
    /// for exactly one rule: §3's *a row a kind promotes into its head is moved,
    /// not duplicated*. *Ungroup* on a group and *Flatten* on a boolean belong to
    /// Structure and appear in the head instead, where the eye goes on that kind —
    /// and are then **gone from Structure**, because the same verb twice in one
    /// menu is the first thing that makes a menu look generated rather than
    /// designed.
    ///
    /// An override rather than a second group in the spec, so the default stays
    /// the answer for every other menu these rows appear in.
    pub group: Option<Group>,
}

impl Row {
    fn new(item: Item) -> Self {
        Self {
            item,
            label: item.spec().label,
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
    fn label(mut self, label: &'static str) -> Self {
        self.label = label;
        self
    }
    /// Give this row a tick box in the state `on`.
    ///
    /// ⚠️ **`impl Into<Option<bool>>`, so a caller with no answer can say so**
    /// (§15 D675). Six of this method's nine call sites pass a plain `bool` and
    /// read unchanged; the three that ask [`ViewState::get`] pass its `Option`
    /// through, and a `None` leaves the row with no tick rather than with a
    /// borrowed one. `checked` was `Some(on)` unconditionally, which is why
    /// `ViewState::get` had to invent a `bool` for switches it holds no field for.
    ///
    /// ⚠️ **This said *"nine of the ten"* for a commit, and both halves were
    /// wrong.** A grep for `.checked(` across this file returns ten, because
    /// `ui::MenuRow::checked` — a different method taking a plain `bool` — is one
    /// of them. Nine and three do not add up to ten either, which is the tell a
    /// count has been taken over the wrong population.
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
/// **A snapshot passed in rather than `&OndinApp` borrowed**, so that building a
/// menu cannot touch the document and so that a test can build one without an
/// app at all. Everything here is a question the app has already answered
/// somewhere else; nothing is recomputed.
pub struct Context<'a> {
    pub target: Target,
    /// The selection the rows act on. A hit inside it leaves it whole (C2), so by
    /// the time this is built it already contains the target.
    pub selection: &'a [NodeId],
    /// `true` when a *Paste* row has anything to do at all — what dims every one of
    /// them except the text session's.
    ///
    /// **Not "the in-app clipboard is full"**, which is what this was until
    /// 2026-08-18 and is the wrong question in both directions: a payload the OS
    /// clipboard no longer describes is stale and must not be offered, and text
    /// copied in another application is pasteable without the app holding anything
    /// (`docs/decisions.md` §15 D218).
    ///
    /// **Three payloads, not two**, since 2026-08-19: a picture is what both paste
    /// paths try *first*, and a clipboard holding only a screenshot answered no to
    /// the other two — so the row went dim over the payload the chord was happiest
    /// with (§15 D224). The composition lives in
    /// [`crate::app::OndinApp::menu_context`], which is the only place that can ask
    /// all three.
    pub can_paste: bool,
    /// The kind of every selected layer, in `selection` order. A **head** row
    /// appears only when every member qualifies; a **tail** row when any does.
    pub kinds: &'a [Kind],
    /// Of the layer under the pointer: hidden, locked, cropped, has a picture,
    /// has a moved pivot, clips its children.
    pub state: LayerState,
    /// Whether the document holds any guide at all — what makes *Clear all
    /// guides* worth offering.
    pub any_guides: bool,
    /// Each of the five View switches this menu can show, as it stands.
    pub view: ViewState,
    /// Whether present mode is on — read by exactly one row (§15 D758).
    ///
    /// **Not in [`ViewState`], though present mode is a `ViewSwitch` too.** That
    /// struct answers *"what is this switch's state, for the tick beside its row"*,
    /// and no menu draws a *Present mode* row — `context-menus.md` §6.1 keeps it
    /// out deliberately. This asks a different question: *"is another switch's row
    /// worth offering"*. Putting it in `ViewState` would make `ViewState::get`
    /// answer `Some` for a switch that has no row, which is exactly the confusion
    /// §15 D675 removed from that function.
    ///
    /// ⚠️ **It became reachable on 2026-09-15.** No context menu could be open in
    /// present mode until §15 D756 removed `open_context_menu`'s refusal, so this
    /// field would have been dead the day before.
    pub present: bool,
    /// Whether a segment (rather than only anchors) is part of the point
    /// selection — what separates §6.5's two Delete rows.
    pub segment_selected: bool,
    /// Whether the right-click that opened this menu landed on the edited path's
    /// **ink** — what makes *Add point here* live.
    ///
    /// A different question from [`Self::segment_selected`], and the pair is easy
    /// to conflate: that one asks what the *selection* holds, this one asks what
    /// was under the pointer at the one moment the menu was opened. Answered by
    /// `canvas::segment_at`, which is the query a press at that pixel would go
    /// through, so the row cannot offer what a click there would decline.
    pub on_segment: bool,
    /// `tools::joinable_ends` said yes — the point selection is exactly two open
    /// ends, so *Join* has something to fuse (§6.5).
    ///
    /// **The builder's own predicate rather than a rule restated here**, which is
    /// what stops the row offering something `join_ends` would then decline —
    /// notably the two ends of a *two-anchor* run, which cannot be closed without
    /// producing a shape with no interior.
    pub can_join: bool,
    /// `OndinApp::properties_source` found a layer to lift an appearance off —
    /// the key layer, or a lone selected one that has paint of its own (§3's
    /// Properties group).
    ///
    /// **Two refusals behind one bool, and the row's sentence picks between them
    /// on the selection's size**: several layers with no key is the ambiguous case,
    /// one layer with no paint of its own is the empty one.
    pub can_copy_props: bool,
    /// Whether the selection has a **key layer** (§15 D113) — the third term in
    /// *Copy properties*' dimming.
    ///
    /// Only ever read alongside [`Self::can_copy_props`], and only to pick which
    /// sentence a refusal shows: a key that has no paint of its own is a different
    /// problem from no key at all, and the two look identical from the row.
    pub has_key: bool,
    /// Whether [`crate::app::OndinApp::property_clipboard`] is holding anything —
    /// what dims *Paste properties*, and §3's own example of a row dimmed by state.
    ///
    /// A different question from [`Self::can_paste`] in every direction: that one
    /// asks about three payloads across two clipboards including the OS's, and this
    /// asks about one app-internal slot that never reaches the system at all.
    pub has_props: bool,
    /// `build::can_outline_text` said yes — the layer under the pointer is text
    /// with contours to convert (§5.6).
    ///
    /// **The snapshot taken at open, not a live read**; see
    /// [`crate::app::ContextMenu::text_outlineable`] for why building the glyph
    /// outlines sixty times a second is the wrong number.
    pub text_outlineable: bool,
    /// The one selected layer is text that is **already** on a rail — what makes
    /// *Detach from path* worth offering, and its exact inverse.
    pub on_a_rail: bool,
    /// And whether that rail is being run backwards — the tick on *Flip to other
    /// side*, and meaningless while [`Self::on_a_rail`] is false, which is the
    /// same relationship the model's two fields have (§15 D406).
    pub rail_flipped: bool,
    /// Whether the **system** clipboard held text when the menu opened — what dims
    /// the text session's *Paste*, which is a different question from
    /// [`Self::can_paste`] and had been answered by the in-app clipboard (§6.4).
    ///
    /// A snapshot rather than a live read; see [`crate::app::ContextMenu::system_text`].
    pub system_text: bool,
    /// Whether the live text session has a selection rather than a bare caret —
    /// what dims *Cut* and *Copy* (§6.4).
    pub text_selection: bool,
    /// Whether the live text session's buffer is empty — what dims *Select all*.
    pub text_empty: bool,
    /// Whether any layer in the document carries export settings — what makes
    /// *Export all* worth offering (§7).
    ///
    /// **A document question on the page menu**, and the only one here that reads
    /// neither the selection nor the pointer. That is what the row is for: an asset
    /// set is spread over frames nobody wants to select first, so the layers'
    /// stored settings are the selection.
    pub any_exports: bool,
    /// `build::can_frame` said yes — the selection has one parent and that parent
    /// can hold a frame (§5.3, §15 D249).
    ///
    /// **The builder's own predicate rather than a rule restated here**, which is
    /// what stops the row offering something *Frame selection* would then refuse.
    /// The case it dims is a selection inside a `Group`: an `Artboard` may only hang
    /// off the root or another `Artboard`, so there is nowhere for the frame to go
    /// without first lifting the artwork out of its group — a second edit nobody
    /// asked for.
    pub can_frame: bool,
}

/// The parts of the layer under the pointer that decide a row's label or its
/// presence.
#[derive(Clone, Copy, Default)]
pub struct LayerState {
    pub hidden: bool,
    /// This layer's **own** lock flag — what *Lock* / *Unlock* writes and what its
    /// label reads.
    ///
    /// **Not the one that dims an editing row**; that is [`Self::locked_within`].
    /// The two were one field until §15 D321, and they cannot be: a layer inside a
    /// locked group must have its edits refused while its *own* toggle still says
    /// *Lock*, because unlocking a flag that is already clear would appear to do
    /// nothing at all.
    pub locked: bool,
    /// Some **ancestor** is locked — this layer sits inside a locked group.
    ///
    /// **The ancestor alone, not the combined answer**, so that
    /// [`Self::locked_within`] can derive it. Storing the combination instead made
    /// the invariant something every construction site had to remember: this struct
    /// derives `Default` and its fixtures are written `LayerState { locked: true,
    /// ..default() }`, which the compiler accepts while silently meaning "locked,
    /// and not effectively locked" — a state that cannot exist. Four tests failed
    /// that way within a minute of the field being added, and a fifth would have
    /// been written later and not failed at all.
    pub locked_by_ancestor: bool,
    pub clips: bool,
    /// This layer **is** a mask, or is a container holding one.
    ///
    /// Both halves, because that is the question `inspector::mask_action` asks to
    /// decide whether the verb makes or releases — and the second half is the
    /// state the verb itself leaves behind, a group with the mask inside it and
    /// the group selected. A tick that only knew the first half would go out the
    /// instant the operation finished.
    pub masked: bool,
    /// This layer reads its own geometry even-odd (§15 D239).
    ///
    /// **The stored rule and not `Node::fill_rule`'s derived answer**, because the
    /// row this feeds is only offered on a `Path` — where the two are the same — and
    /// a tick that reported the derived rule would show an `Exclude` as ticked for a
    /// row it is never given.
    pub even_odd: bool,
    /// `croppable_fill` said yes — the same predicate image editing acts on, so a
    /// row here cannot offer what the mode would refuse (§5.7).
    pub has_picture: bool,
    // **`picture_refusal` and `cropped` were here and went on 2026-08-23**, with
    // the *Export original…* and *Reset crop* rows that were their only readers.
    // Both carried real reasoning — `picture_refusal` held the *reason* rather than
    // a `bool` so that a document missing its bytes was not told it was linked
    // (§15 D280) — and `tools::original_refusal`, which held it, went the same day
    // when the inspector's door onto that verb went too. The reasoning is written
    // where that function stood; nothing here needs to ask any more (§15 D305).
    pub pivot_moved: bool,
    /// The layer is inside a `Boolean`, so it can be made the base operand.
    pub in_boolean: bool,
    /// Which of `TextSizing`'s three cells this layer is in, or `None` when it is
    /// not text — what the three §5.6 rows tick (`TextSizing::cell`).
    ///
    /// **`Option` rather than a bare index with a separate "is it text"**, so the
    /// one state that has no answer cannot be spelled: a default of `0` would tick
    /// *Auto width* on a rectangle if the rows were ever offered off a text node.
    pub text_sizing: Option<u8>,
    /// `build::can_outline` said yes — outlining would produce a layer different
    /// from this one (§15 D230).
    ///
    /// **State rather than kind, for exactly one kind.** A rect, an ellipse, a
    /// polygon, a star and a line always answer yes; a `Path` answers yes only while
    /// its corner radii are still in the model rather than in its geometry. So the
    /// *row* is present on both kinds — §3's "kind decides presence" — and this is
    /// what dims it on a path that is already an outline.
    pub outlineable: bool,
}

impl LayerState {
    /// Whether an **edit** of this layer is refused: its own lock, or a lock on
    /// anything it sits inside (§15 D321).
    ///
    /// Derived rather than stored so the impossible state — locked but not
    /// effectively locked — cannot be spelled at a construction site. Every
    /// `dim_if` reads this; only *Lock* / *Unlock* reads [`Self::locked`].
    pub fn locked_within(&self) -> bool {
        self.locked || self.locked_by_ancestor
    }
}

/// Build the menu for one open: groups in canonical order, empty ones dropped.
///
/// **Each menu says which rows it wants; this says where they go.** The bucketing
/// is what makes §3's claim structural rather than a habit — a menu function
/// cannot put *Flatten* in a different place from where the menu next door puts
/// it, because it does not choose the place at all. [`Spec::group`] does, once,
/// per row.
///
/// Within a group the emitted order is kept, which is the part a menu *does*
/// choose: the four boolean operations read Union · Subtract · Intersect ·
/// Exclude because `layer_menu` pushes them that way.
///
/// The return is `Vec<Vec<Row>>` rather than a flat list with separator markers
/// in it, because that is what makes "an empty group collapses and takes its
/// separator with it" true by construction instead of by a rule somebody has to
/// remember at the draw site.
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
        // `extract_if` in spirit, spelled with a partition so the order inside a
        // group is the order the menu pushed them in.
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
/// Carries **no selection rows** even while something is selected: the menu is
/// decided by what is under the pointer, never by what is selected (C3).
fn canvas_menu(cx: &Context<'_>) -> Vec<Row> {
    let mut rows = vec![
        Row::new(Item::PasteHere)
            .label("Paste here")
            .dim_if(!cx.can_paste, "Nothing has been copied yet"),
        // Directly under *Paste here*, which is §6.1's own order and reads as the
        // pair it is: the two answers to "where", one taken from the pointer and one
        // from where the copy came from (§15 D248).
        Row::new(Item::PasteInPlace).dim_if(!cx.can_paste, "Nothing has been copied yet"),
        Row::new(Item::SelectAll),
        Row::new(Item::ZoomFit),
        // The one document-wide verb this app has, and the page is the only target
        // it belongs to: every other menu is aimed at something, and this one is
        // aimed at the file. Dimmed rather than hidden when nothing is set up to
        // export, which is §3's rule and here also the only place the feature
        // announces itself to someone who has not found the panel.
        Row::new(Item::ExportAll).dim_if(
            !cx.any_exports,
            "No layer in this document has export settings",
        ),
    ];
    // The View rows a canvas right-click plausibly wants, in the View menu's own
    // order. *Lock guides* is here because C7 needs it: with guides locked, a
    // guide is not a target at all, so this is the only way back.
    //
    // ⚠️ **The count is deliberately gone** (§15 D757). This read *"the four View
    // rows"* and they are five; it is the **third** copy of that number found in
    // this file, after `ViewState`'s doc and `ViewState::get`'s, and it was missed
    // by a sweep that grepped the neighbourhood of the ones it had already
    // corrected. `view_rows` is the list — read it rather than a number beside it.
    rows.extend(view_rows(cx));
    rows
}

/// §6.2 — nothing but the two rows that are about the tree as a whole.
///
/// *Collapse all* / *Expand all* is already a button in the panel's header, and
/// a context menu that repeats a control six pixels away is noise.
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

/// §6.4 — four rows and no more. Type is what the Typography panel is for, and a
/// session already has the panel open beside it.
///
/// **All four rows act on the editor, not on the document**, which is the whole of
/// what makes this target different from every other one: *Cut*, *Copy* and
/// *Select all* share an [`Item`] — and so a label, a glyph and an accelerator —
/// with the layer verbs of the same name, and `perform_menu_item` routes them by
/// target. Same rule `Item::Delete` has always relied on: what a verb acts on is
/// the target's question, not the row's.
///
/// **Its *Paste* reads a different clipboard from every other menu's.** A session
/// pastes the OS clipboard's *text*; the layer menus paste
/// [`crate::app::OndinApp::clipboard`]'s captured subtrees. This row was dimmed by
/// the second of those until 2026-08-18, so copying a word in a browser and
/// right-clicking in a session offered a dead *Paste* — and taking it pasted
/// layers onto the canvas.
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
/// *Add point here* (§15 D255) and *Join* (§15 D256) landed 2026-08-20, alongside
/// *Make corner / Make smooth* (§15 D250) — so §6.5 is the first section of that
/// spec with no unbuilt row left in it.
///
/// **Both conversion rows are offered whatever the selection holds**, rather than
/// dimming *Make smooth* on something already smooth. §3's rule is that a row dims
/// when the verb has nothing to do, and over a *set* of anchors it almost always
/// has: a selection of four points where three are corners is the ordinary case,
/// and a row that went dark only when all four already agreed would be dark
/// exactly when it is least useful to ask about. Idempotent besides — converting a
/// smooth point to smooth leaves it alone.
fn points_menu(cx: &Context<'_>) -> Vec<Row> {
    vec![
        // The one row here that reads *where* the menu was opened rather than what
        // is selected — see [`Item::AddPointHere`] for why it dims instead of
        // vanishing when the click missed.
        Row::new(Item::AddPointHere).dim_if(
            !cx.on_segment,
            "Right-click on the path's outline to add a point to it",
        ),
        // `Del` is two verbs on one key (§15 D120) — remove the anchors, which
        // heals, or remove the segment between them, which leaves the gap — and
        // the row's own name is where that distinction finally shows.
        Row::new(Item::DeletePoints).label(if cx.segment_selected {
            "Delete segment"
        } else {
            "Delete points"
        }),
        Row::new(Item::SetSmooth(false)),
        Row::new(Item::SetSmooth(true)),
        Row::new(Item::ReverseSubpaths),
        // Last, which is where §6.5's table puts it — and within a group the
        // emitted order is what decides, so this line *is* the placement.
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

    // §5.9 — **the state that decides most of a menu.** The whole menu is present
    // and every editing row is dimmed with one sentence, while the rows that are
    // the way back stay live. Reachable only through the panel: `hit_test` filters
    // out anything locked, so the canvas cannot deliver this target (C4) — which
    // is exactly why the panel's menu has to carry the complete set.
    // **`locked_within`, not `locked`** — a locked group locks its contents (§15
    // D321), so every row below dims for a layer *inside* one too. `ToggleLocked` is
    // the one row still reading `st.locked`, because that is the flag it writes.
    let locked = st.locked_within();
    // **Two sentences, because they send the hand to different places.** A layer
    // inside a locked group is not itself locked, so "This layer is locked" points
    // at a toggle that is already off; the reason a dimmed row gives has to name the
    // thing that would have to change.
    let why = if st.locked {
        "This layer is locked"
    } else {
        "A group containing this layer is locked"
    };

    // Emitted in the order they are written; `build` is what puts them in
    // groups, so nothing here has to know where its rows will end up.
    let mut rows = head_rows(cx);

    // --- clipboard --------------------------------------------------------
    rows.extend([
        Row::new(Item::Cut).dim_if(locked, why),
        Row::new(Item::Copy),
        Row::new(Item::PasteHere)
            .label(if panel { "Paste" } else { "Paste here" })
            .dim_if(!cx.can_paste, "Nothing has been copied yet"),
        Row::new(Item::Duplicate).dim_if(locked, why),
        Row::new(Item::Delete).dim_if(locked, why),
    ]);

    // --- properties (§3's third group) ------------------------------------
    //
    // **On every layer target, like the rest of the tail**, and both rows are
    // present whatever the kind: §3 says kind decides presence, and there is no
    // kind for which "make this look like that" could never apply — a group
    // qualifies as a *destination* (the paste walks through it to the shapes
    // inside) even though it cannot be a source. The source rule is `can_copy_props`
    // and it is state, so it dims.
    rows.extend([
        Row::new(Item::CopyProperties).dim_if(
            !cx.can_copy_props,
            // **Three sentences, because there are three refusals and one of them
            // was being told the wrong one.** The pair `selection.len()` alone can
            // tell apart is *ambiguous* against *empty*; what it cannot see is a
            // selection that **has** a key layer which happens to have no paint —
            // a key set to a group is the ordinary way in — where a message saying
            // "make one of these the key layer" tells the user to do a thing they
            // have already done.
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
    //
    // Every row here is kind-gated, and the gates are the code's rather than this
    // file's: `build::ungroup` takes a Group or a Boolean and nothing else, and
    // `build::boolean` wants two or more members.
    if !cx.kinds.contains(&Kind::Frame) {
        // A frame cannot be a member of a group (`build::group` refuses one), so
        // the row is absent rather than present-and-failing.
        rows.push(Row::new(Item::Group).dim_if(locked, why));
    }
    // **Present exactly where *Group selection* is not, as well as everywhere it
    // is**, and that is the difference between the two verbs rather than an
    // inconsistency: a frame nests in a frame (§5.3), so a selection of frames has
    // an honest answer here and none above. Dimmed rather than absent where the
    // *parent* cannot hold a frame — a selection inside a group — because that is
    // about where the layers happen to sit and is fixable by moving them, which is
    // exactly what §3 wants a sentence for.
    rows.push(
        Row::new(Item::FrameSelection)
            .dim_if(!cx.can_frame, "A frame cannot go inside a group")
            .dim_if(locked, why),
    );
    // *Ungroup* and *Flatten* on a group or boolean are **promoted into the
    // head** (§3) and must not appear twice, which is the first thing that makes
    // a menu look generated rather than designed.
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
    // *Use as mask*, which `context-menus.md` §9.3 refused as a non-row while
    // nothing in the model expressed one (§15 D286 reverses that).
    //
    // **Absent where the verb has no answer, dim where it merely refuses.** A
    // selection containing a frame has nothing to offer — a frame can neither be a
    // mask nor be grouped, which is the same pair of reasons *Group selection*
    // above is absent for — and inside a boolean the operands are combined rather
    // than drawn, so a mask there would take and do nothing.
    //
    // Ticked like *Clip content* rather than like the booleans beside it: this is
    // a toggle whose off-state is worth showing, not one of four alternatives.
    if !cx.kinds.contains(&Kind::Frame) && !st.in_boolean {
        rows.push(
            Row::new(Item::Mask)
                .checked(one && st.masked)
                .dim_if(locked, why),
        );
    }
    // **`Path` only** — see `Item::EvenOdd`. One layer at a time, because the check
    // mark reports *this* shape's rule and a mixed selection has no answer to show.
    if one && matches!(cx.kinds.first(), Some(Kind::Path)) {
        rows.push(
            Row::new(Item::EvenOdd)
                .checked(st.even_odd)
                .dim_if(locked, why),
        );
    }
    // The four operations: checkable while the target *is* a boolean (they are
    // the same four the inspector's dropdown offers), a plain verb over two or
    // more layers otherwise. One `Item` either way, because `apply_boolean`
    // already decides which from the selection.
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
    // **The row *Flatten* is never offered beside** (§4, §15 D230). Its subject is
    // one shape, where Flatten's is a boolean or a set, so the two conditions are
    // mutually exclusive by construction rather than by a rule anyone has to keep:
    // `one` excludes a set and `Kind::Shape | Kind::Path` excludes a boolean.
    //
    // Present on a `Path` as well as a primitive, because `build::can_outline` is a
    // question about *state* on that one kind — a path whose radii are already
    // geometry has nothing to bake — and §3 says presence is the kind's answer.
    // §5.6's third row, in *Outline shape*'s slot rather than in the text head:
    // what it does is structural, and §3 groups by what a verb does rather than by
    // the kind that offers it. Mutually exclusive with the row below by kind, so
    // the shared slot never holds both.
    if one && cx.kinds.first() == Some(&Kind::Text) {
        rows.push(
            Row::new(Item::OutlineText)
                .dim_if(!cx.text_outlineable, "This text has no glyphs to convert")
                // Locked last, so it wins when both apply — the row below explains
                // why the order *is* the priority.
                .dim_if(locked, why),
        );
    }
    // **Beside *Convert to path* rather than in the text head**, for that row's own
    // reason: what these do is structural, and §3 groups by what a verb does.
    //
    // Neither is ever dim, `locked` aside. Both appear only on a node that is
    // already on a rail, which is a state with nothing to explain.
    if one && cx.on_a_rail {
        // Flip above Detach: it is the one you reach for while the type is on the
        // curve, and Detach is the one you reach for when you have finished with it.
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
                // **Locked last, so it wins when both apply.** Two reasons can be
                // true of a locked bare path, and "unlock it" is the one the user can
                // act on; `dim_if` overwrites, so the order *is* the priority.
                .dim_if(locked, why),
        );
    }

    // --- order ------------------------------------------------------------
    //
    // **Never dimmed for being already frontmost.** `z_order` returns an empty
    // transaction and the app's idiom for that is an info line, the way
    // `distribute` says "Already evenly spaced". Four predicates run on every
    // menu open to grey four rows is a worse trade than one sentence after a
    // click that did nothing.
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
    // §3's exception: a row whose only purpose is to undo a non-default state is
    // omitted when that state is default, rather than dim on every open.
    if st.pivot_moved && one {
        rows.push(Row::new(Item::ResetOrigin).dim_if(locked, why));
    }

    // --- state ------------------------------------------------------------
    //
    // The label follows the target: a mixed selection reads *Hide* and hides all
    // of it, which is what the chord does (`OndinApp::set_switch`).
    rows.extend([
        Row::new(Item::ToggleHidden).label(if st.hidden { "Show" } else { "Hide" }),
        Row::new(Item::ToggleLocked).label(if st.locked { "Unlock" } else { "Lock" }),
        // Single-selection only and refused in present mode, both of which the
        // built chord already imposes: with several picked there is no one row to
        // lay the field over.
        //
        // ⚠️ **And never dimmed by the lock, which it was until §15 D529.**
        // `docs/context-menus.md` §5.9 lists the five rows that stay live over a
        // locked layer and *Rename* is one of them — a name is not the layer's
        // artwork, which is the argument that keeps *Copy* and *Copy properties*
        // live two groups up. **The row was the only one of the verb's three
        // doors that refused**: `Ctrl+R`, `F2` and the layers panel's
        // double-click all renamed a locked layer without complaint. That is the
        // failure §15 D261 states 380 lines below this line, in the *Edit image*
        // push — *"a menu refusing where a key allows is the failure, and a menu
        // agreeing with one is not"* — and *Edit image* was brought into line by
        // adding the refusal to the verb; here the design says the other way, so
        // the dim goes.
        Row::new(Item::Rename).dim_if(!one, "Pick one layer to rename it"),
    ]);

    // --- navigate ---------------------------------------------------------
    //
    // One row. *Reveal in layers panel* was specified here and is deliberately
    // not built — `docs/context-menus.md` §7 carries the reason.
    rows.push(Row::new(Item::ZoomSelection));

    // §3's ninth group. Never dim: the menu is open on a layer, so there is always
    // something to write, and a locked layer keeps both for *Export original…*'s
    // reason — neither changes the layer.
    //
    // **Two rows since 2026-08-22, not three.** *Export as…* was the file half and
    // went with `Ctrl+Shift+E` when the Export panel's own button made it the
    // slower of two doors onto the same file; what is left is the clipboard pair,
    // ordered semantic-then-raster, which is also least-lossy first — the SVG
    // arrives as editable groups and shapes and the PNG as a picture of them. This
    // tail is §4's, so both reach the canvas door and the layers-panel door from
    // one push.
    rows.extend([Row::new(Item::CopyAsSvg), Row::new(Item::CopyAsPng)]);

    rows
}

/// §5 — the rows above group 2, per kind.
///
/// **A multi-selection has a head only when every member is the same kind** (§3),
/// which is what stops one text node among four rectangles putting *Edit text* on
/// the menu. The one exception is *Make this the key layer*, which is about the
/// layer under the pointer rather than about the selection's kind.
fn head_rows(cx: &Context<'_>) -> Vec<Row> {
    let st = cx.state;
    let one = cx.selection.len() == 1;
    // **`locked_within`, not `locked`** — a locked group locks its contents (§15
    // D321), so every row below dims for a layer *inside* one too. `ToggleLocked` is
    // the one row still reading `st.locked`, because that is the flag it writes.
    let locked = st.locked_within();
    // **Two sentences, because they send the hand to different places.** A layer
    // inside a locked group is not itself locked, so "This layer is locked" points
    // at a toggle that is already off; the reason a dimmed row gives has to name the
    // thing that would have to change.
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
                // *Flatten* is **the** verb on this kind — it turns the boolean
                // into the `Path` its outline already is — and *Ungroup* releases
                // the operands. Both promoted out of Structure, not repeated there.
                rows.push(Row::new(Item::Flatten).promote().dim_if(locked, why));
                rows.push(Row::new(Item::Ungroup).promote().dim_if(locked, why));
            }
            _ => {}
        }
    }

    // §5.4 / §5.6 / §5.7 compose in `double_click_pick`'s own order — text, then
    // the picture, then the points — so the menu and the double-click cannot
    // disagree about what "one level in" means on the same layer.
    if all_are(Kind::Text) {
        rows.push(Row::new(Item::EditText).dim_if(locked, why));
        // §5.6's three, directly under *Edit text* where its table puts them, and
        // **only over one layer**: the tick says which state this node is in, and a
        // selection of two text nodes in different states has no honest answer to
        // put there. §3's head rule already wants every member to qualify; this is
        // the stricter case where even agreeing on the *kind* is not enough.
        //
        // ⚠️ **`one` was missing and this comment was the only thing enforcing
        // it** (§15 D530). Every one of the eight neighbouring pushes in this
        // function is inside an `if one`; this one was not, and
        // `layer_menu_state` reads `text_sizing` off the **hit** node alone, so
        // the `Option` this guard waits on can never be `None` while the hit is
        // text. Measured: two text layers in *different* states, both selected,
        // right-click one — all three rows appear, the hit layer's state is
        // ticked as though it were the selection's, and taking a row writes
        // **only** the hit layer while the other's tick was never shown. §15
        // D261 rules that out in as many words: *"'Every member qualifies' is
        // satisfied by a pair of text layers — both are text — but the tick is a
        // fact about one node."*
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
        // **First, because it is the head verb** and the rows under it operate on
        // the picture this one steps into (`context-menus.md` §5).
        //
        // **`dim_if(locked)` since 2026-08-23, and it dims because the *mode*
        // refuses** — never instead of it. `OndinApp::begin_image_edit` is the one
        // place all four doors pass through and it now returns early on a locked
        // layer, so this row and `Enter` give the same answer. That ordering is the
        // whole of §15 D261's rule: a menu refusing where a key allows is the
        // failure, and a menu agreeing with one is not.
        //
        // **The argument that settles it: a crop is a resize.** `crop_resize_tx`
        // writes through `tools::resize_layer`, which is the write *Original size*
        // — the row immediately below this one — has always been dimmed for, on the
        // grounds that "resizing the node is a write and the lock reaches it". Two
        // rows of one group disagreeing about one verb was the defect; `EditPoints`,
        // the other geometry-editing mode reached from a row, dims two lines down.
        //
        // ⚠️ **This carried the opposite reasoning until 2026-08-23**, and the shape
        // is worth keeping: it said the absence of a dim was "a match rather than an
        // exemption", which was true — it matched a path that read no lock at all.
        // *Matching a gap is not agreement, and a comment that says "consistent with
        // X" is only as good as X.* The question it deferred to `roadmap.md` was
        // answered in the mode, exactly where it said it belonged.
        rows.push(Row::new(Item::EditImage).dim_if(locked, why));
        // **Two rows, and it was five until 2026-08-23.** *Reset crop* and
        // *Replace…* moved to the image-editing card's Settings tab alone — worth
        // having, not worth a place in a menu that opens on every layer — and
        // *Export original…* went from both doors, the Export panel offering
        // several sizes and formats where that row offered the stored bytes
        // unchanged. What is left is the verb a hand reaches for *after* playing
        // with a picture: the layer back at the picture's true size, which is
        // wanted without being in the mode and is why this one kept the menu door
        // rather than the card's.
        rows.push(Row::new(Item::OriginalSize).dim_if(locked, why));
    }
    if one && all_are(Kind::Path) {
        rows.push(Row::new(Item::EditPoints).dim_if(locked, why));
    }

    // §5.3 / §5.8 — the two designations, which are the same gesture
    // (`Alt+Shift`+click) wearing two names depending on where the layer sits.
    //
    // **Both are announced on the canvas**, which this said they were not until
    // 2026-08-22, and the stale half was load-bearing: it was read as a gap and
    // very nearly answered with a redundant layers-panel badge. A key wears
    // `canvas::draw_key_outline`'s 3px `color::KEY` trace of its own outline, and a
    // `Subtract`'s base operand wears the layers panel's stack badge (§15 D244).
    // What is genuinely missing is only a way *in* other than the gesture and this
    // row.
    if st.in_boolean {
        rows.push(Row::new(Item::MakeBaseOperand).dim_if(locked, why));
    } else if cx.selection.len() >= 2 {
        rows.push(Row::new(Item::MakeKeyLayer));
    }

    rows
}

/// The five View switches §6.1 and §6.3 share, each carrying its own state.
///
/// ⚠️ **`LayoutGrid` is the fifth and it arrived a session after the switch did**
/// (§15 D755 built it, D757 gave it this row). *Show grid* was in both menus and
/// *Show layout grid* in one, which is the *"a rule stated in one place and broken
/// in the two beside it"* shape this codebase keeps finding; the maintainer's
/// answer was consistency.
///
/// 🚨 **One row is dimmed here and only one, and the asymmetry is the point**
/// (§15 D758). Present mode overrides exactly one of these five: `rulers_on()` is
/// `show_rulers && !self.present`, so ticking *Show rulers* in present mode writes
/// the flag and changes nothing on screen. The other four are live there —
/// `guides_on()` is `show_guides` alone and `grid_visible()` reads no present flag,
/// both by §15 D750's ruling, and *Lock guides* is a property rather than a
/// visibility. **So this is not "present mode dims the View rows"**; it is one row
/// whose effect present mode happens to swallow, and a version that dimmed the
/// group would be wrong about four of them.
///
/// ⚠️ **Dimmed rather than absent**, per `context-menus.md` §6: *"Kind decides
/// presence … State decides enabled. A row that belongs to the kind but cannot run
/// now is dimmed and says why."* The kind here is the canvas, which has rulers;
/// what it cannot do is *now*. And the reason names the thing that would have to
/// change, which is §5.9's rule for a `why` string.
fn view_rows(cx: &Context<'_>) -> Vec<Row> {
    [
        ViewSwitch::Rulers,
        ViewSwitch::Guides,
        ViewSwitch::GuideLock,
        ViewSwitch::Grid,
        ViewSwitch::LayoutGrid,
    ]
    .into_iter()
    .map(|sw| {
        Row::new(Item::ViewSwitch(sw))
            .checked(cx.view.get(sw))
            .dim_if(
                sw == ViewSwitch::Rulers && cx.present,
                "Present mode hides the rulers",
            )
    })
    .collect()
}

/// The height a menu of these groups will paint at, in points.
///
/// **Computed rather than measured**, because [`crate::ui::menu_place`] needs the
/// size *before* the `Area` is shown and reading last frame's rect back would
/// leave a menu opened near an edge in the wrong place for one frame. Rows are a
/// fixed height and separators are a fixed height, so this is exact rather than
/// an estimate — which is the only reason the approach is allowed.
pub fn height(groups: &[Vec<Row>], row_h: f32, gap: f32, sep_h: f32, pad: f32) -> f32 {
    let rows: usize = groups.iter().map(|g| g.len()).sum();
    let seps = groups.len().saturating_sub(1);
    let gaps = (rows + seps).saturating_sub(1);
    // **The frame's border is part of the height and was missing from it.** A
    // `Frame`'s stroke grows its outer rect, so every menu painted two points taller
    // than this said and `menu_place` clamped the short number — §15 D263. It is
    // added here rather than at the call site because this function's contract is
    // "the height a menu will paint at", and a menu is always in `ui::menu_frame`.
    crate::ui::MENU_BORDER * 2.0
        + pad * 2.0
        + rows as f32 * row_h
        + seps as f32 * sep_h
        + gaps as f32 * gap
}

/// One keypress an open menu answers while it holds the keyboard
/// (`docs/context-menus.md` §8, R3).
///
/// **Read where R3's gate is and applied where the rows are**, which is why this
/// is a value and not a call: the keys have to be *consumed* before any panel
/// runs — a menu holding the keyboard means the layers panel's rename field does
/// not see `↑` either — and the row a `↓` lands on is not known until `build` has
/// run at the bottom of the frame. So `read_nav` takes the press at the top and
/// this carries it down.
///
/// No type-ahead in v1: §8 scopes it out, and a menu whose rows are already
/// keyed by an accelerator column has a second way to say every verb.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Nav {
    Move(Step),
    /// `Enter` — run the highlighted row. A no-op with nothing highlighted,
    /// deliberately: `Enter` is not "take the first row", because a menu opened by
    /// accident is dismissed with `Escape` and a stray `Enter` must not commit an
    /// edit nobody aimed at.
    Activate,
}

/// Where a highlight goes (`docs/context-menus.md` §8).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    /// `↓` — the next reachable row, wrapping. From nothing, the **first**.
    Next,
    /// `↑` — the previous reachable row, wrapping. From nothing, the **last**,
    /// which is what every menu on every platform does and the only reason `↑` on
    /// a fresh menu is worth pressing.
    Prev,
    First,
    Last,
}

/// Move a keyboard highlight through a built menu (§15 D223).
///
/// **The highlight is a verb, not a position.** A menu is rebuilt from live app
/// state on every frame it is open (that is what lets a checkable row tick under
/// the pointer), so an index would silently come to mean a different row the
/// moment a row appeared or went away. An [`Item`] survives the rebuild, and it
/// is unique within a menu — §3's rule that a promoted row is *moved* and not
/// duplicated is what makes that true, and
/// `no_menu_offers_the_same_verb_twice` is what holds it true.
///
/// **Only rows a press could actually run are reachable.** A dimmed row explains
/// itself through a tooltip, and a keyboard cannot hover — so stopping on one
/// would offer a dead end with no way to read why it is dead. `Home`/`End` and
/// the wrap all agree with that: `End` is the last *live* row, not the last row.
///
/// Returns `None` when the menu has no live row at all, which is a menu of
/// nothing but dimmed rows — reachable today over a locked layer, where only
/// *Unlock* saves it.
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
        // A highlight the rebuild left behind — the row went dim or went away —
        // restarts the run rather than staying lost, and restarts it at the end
        // the key was heading for.
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

/// The export's card: 210pt wide against the top bar's dropdowns at 160
/// (`design/Editor.dc.html`). The accelerator column is what buys the extra 50.
///
/// **The width the card paints, border and all** — 210 outer since 2026-08-23,
/// where it was 210 of content and 212 on screen (§15 D307). `crate::ui::menu_inner_w`
/// is what takes the border and the padding back off for the rows.
const MENU_W: f32 = 210.0;
/// The export's row, which is also the View menu's — `ui::MENU_ITEM_H`, which is
/// where the number lives now that three places wanted it.
const ROW_H: f32 = crate::ui::MENU_ITEM_H;
/// `menu_frame`'s inner margin, the same 5pt the dropdowns use.
const MENU_PAD: f32 = 5.0;
/// One separator, as `ui::menu_sep` allocates it — the constant lives there,
/// beside the code that draws it, because these two numbers agreeing is what makes
/// [`height`] exact rather than an estimate.
const SEP_H: f32 = crate::ui::MENU_SEP_H;

/// Take this frame's menu keypress, if there is one (`docs/context-menus.md` §8).
///
/// **Called from R3's gate at the top of the frame, and it consumes.** The gate is
/// already the place that says an open menu holds the keyboard — it is what stops
/// `input::resolve` running — and these five keys are the other half of that
/// claim: `consume_key` takes them out of `Events` before any panel is drawn, so
/// egui's own widgets cannot read an `↑` the menu is using. Reading them at the
/// draw site instead would be too late by a whole panel tree.
///
/// **`Modifiers::NONE`, so a modified arrow is not a menu arrow.** The bare keys
/// are the whole keymap here; anything with a modifier on it belongs to whoever
/// claimed the chord, and while a menu is open that is nobody.
///
/// Only one press per frame, which is what a menu can act on: a frame carrying
/// both `↑` and `↓` is a frame the two would cancel out in anyway.
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
    /// **Called from a region's `secondary_clicked()`, which egui delivers on the
    /// release** (R2) — there is no global "any right-click opens something", and
    /// a widget that already reads the secondary button keeps it.
    ///
    /// **One refusal lives here rather than at each of the call sites**, because a
    /// call site that forgot it would be a bug nobody could see:
    ///
    /// - **A press that cancelled a gesture spends the click** (R1). One
    ///   right-click either cancels or opens a menu, never both. The cancel has to
    ///   stay on the press (§15 D168) and the menu has to be on the release,
    ///   because a menu drawn on the press has that release land *inside itself*
    ///   and pick whatever row is under the pointer.
    ///
    /// 🚨 **There used to be a second refusal here and present mode is no longer
    /// one** (§15 D756). The bullet read *"**Present mode opens nothing.** It hides
    /// the very chrome a menu would sit over — which is also why *Present mode* is
    /// not a row in the canvas menu — and it is what lets `self.present` stay first
    /// in `escape`'s ladder without ever contending with R3."* **Both of its
    /// arguments were wrong, in different ways:**
    ///
    /// - **A context menu is not chrome.** It is the canvas answering a click, and
    ///   present mode leaves the canvas working — the maintainer's ruling in §15
    ///   D750, which this is the second consequence of. The *first* clause is
    ///   nevertheless still true of the row: `docs/context-menus.md` §6.1 keeps
    ///   *Present mode* out of the canvas menu because the **switch** is chrome-wide
    ///   and the View menu is a click away, which is a fact about that row and was
    ///   never a reason to refuse the whole menu. ⚠️ **One sentence carried both, so
    ///   striking the refusal reads as striking the row rule too.** It does not.
    /// - **The ladder claim was never load-bearing**, and is now false in the
    ///   direction that is correct anyway. R3 gives an open menu the keyboard
    ///   *before* `escape` runs at all — `OndinApp::update` answers Escape off
    ///   `context_menu.is_some()` and never reaches `input::resolve` — so with a
    ///   menu up in present mode the first Escape closes the menu and the second
    ///   leaves the mode. `self.present` is still first **inside** `escape`, which
    ///   is all that arm ever said; what changed is that the two states can now
    ///   co-exist, and R3 wins, which is what R3 is for. ⚠️ `architecture.md` §9.4
    ///   and `context-menus.md` §0 R3 both asserted the two *"can never contend"*
    ///   on the strength of this refusal; both are amended by D756 rather than left
    ///   standing.
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
        // **R4, one slot across every floating thing** — not just across the
        // menus. All of these draw over the canvas the menu was aimed at, and the
        // design export clears exactly this set in the call that opens its own
        // (`openCanvasMenu`).
        //
        // ⚠️ **It cleared four of *seven* until §15 D533, and the comment above
        // said five.** The three survivors are `effect_menu` (§5.3a's effects
        // popover), `export_menu` (the Export panel's own) and `export_row_open`
        // (an Export row's ⚙). The first two were measured up over the canvas
        // after a right-click — two real `Order::Foreground` `Area`s left over
        // the thing the menu was aimed at, which is the state R4 exists to make
        // unreachable — and neither dismisses itself on the way, because both
        // read `PointerButton::Primary` only, deliberately.
        //
        // ⚠️ **Seven, and it is worth being exact about, because *"six"* is the
        // number that caused this.** Six is `openCanvasMenu`'s count in the
        // design export, which `context-menus.md` §0 R4 inherited and the app
        // outgrew by three popovers. **This comment said "four of six" in its
        // first draft** — the entry about a miscounted set, miscounting it — and
        // `arch-scribe` caught it. Count the assignments below, not the prose.
        //
        // ⚠️ **The list is spelled out here rather than routed through
        // `OndinApp::a_popover_is_open`, and that is deliberate**: this is a
        // *clear*, that is a *test*, and the two sets differ by exactly one
        // member. Image editing's card is in the lane and is **not** in this set
        // — see below — so a shared list would have to carry an exception, which
        // is how a fifth popover would come to be quietly excluded from one of
        // the two. What is shared is the arithmetic nobody can get wrong: both
        // sites are named in D533 and each says the other exists.
        self.open_menu = TopMenu::None;
        self.picker = None;
        self.stroke_menu = None;
        self.effect_menu = None;
        self.export_menu = false;
        self.export_row_open = None;
        // **Image editing's card is not in this set, and that is the R4 rule
        // applied rather than broken.** R4 clears the *floating things somebody
        // opened*; that card is what the mode looks like, so clearing it would
        // mean a right-click silently half-left a mode — the gestures still armed,
        // the outline still drawn, and the controls gone. Leaving the mode is
        // `Escape` or `Enter`, and the context menu is aimed at the picture the
        // mode is on.
        self.type_menu = None;
        self.context_menu = Some(ContextMenu {
            at,
            target,
            world,
            just_opened: true,
            // One OS clipboard read per open, answering both halves of what a
            // *Paste* row can offer — see the fields for why this is a snapshot
            // where the rest of a menu is not.
            system_text: crate::app::system_clipboard_text().is_some(),
            // The third payload, and the one that costs something to ask about —
            // see `system_clipboard_has_image` for why it is asked once and
            // answered with a `bool` rather than with the picture.
            system_image: crate::app::system_clipboard_has_image(),
            payload_current: self.owns_the_clipboard(),
            // Asked here rather than per frame — see the field for the cost, and
            // only of a layer target, since no other target has a layer to ask
            // about.
            text_outlineable: match target {
                Target::Layer { id, .. } => build::can_outline_text(&self.session.resolved, id),
                _ => false,
            },
            // Nothing highlighted: a fresh menu belongs to the pointer, and the
            // first `↓` is what takes it (§8).
            highlight: None,
        });
    }

    /// Draw the open menu, and close it the two ways every menu closes.
    ///
    /// Runs after every door has had its say, so a right-click that *replaced* the
    /// menu has already written the slot by the time the click-away test would
    /// have run on it.
    ///
    /// `nav` is this frame's menu keypress, already taken out of `Events` by
    /// [`read_nav`] at the top of the frame — it arrives as an argument rather than
    /// being read here because a key the menu is using has to be consumed *before*
    /// the panels are drawn, and it is applied here because the row a `↓` lands on
    /// does not exist until `build` has run.
    pub(crate) fn context_menu_ui(&mut self, ctx: &egui::Context, nav: Option<Nav>) {
        let Some(menu) = self.context_menu.as_ref() else {
            return;
        };
        let (at, target, world, just_opened) = (menu.at, menu.target, menu.world, menu.just_opened);
        let mut highlight = menu.highlight;

        // Rebuilt from live app state every frame, which is what lets a checkable
        // row update under the pointer without the menu having to close. The
        // clipboard is the exception and is the snapshot `menu` carries.
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
        // **`MENU_W` plain on x**, where this once added the border by hand: the
        // card now paints exactly `MENU_W`, so the placer and the drawing are the
        // same number rather than two that have to be kept two apart (§15 D307).
        // The height still adds it, because it is summed from rows rather than
        // named — see [`height`].
        let size = egui::vec2(MENU_W, height(&groups, ROW_H, 1.0, SEP_H, MENU_PAD));
        let pos = crate::ui::menu_place(ctx, at, size);

        let mut chosen = None;

        // **The keyboard half of the menu** (§8). Three states rather than two:
        // an arrow moves the highlight, `Enter` runs it, and a pointer *move*
        // gives the highlight back to the pointer — which is what keeps the menu
        // to one highlight instead of painting the keyboard's beside a hover.
        //
        // The keyboard wins a frame that carries both, because the pointer move in
        // that frame is the hand resting on a mouse rather than an aim.
        let moved = ctx.input(|i| {
            i.events
                .iter()
                .any(|e| matches!(e, egui::Event::PointerMoved(_)))
        });
        match nav {
            Some(Nav::Move(step)) => highlight = navigate(&groups, highlight, step),
            // Only ever a row a press could have run: `navigate` never lands on a
            // dimmed row, and this re-checks against *this* frame's build in case
            // the state under it changed since the last arrow key.
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
                            let mut style = crate::ui::MenuRow::new(spec.glyph, row.label)
                                .accel(spec.accel)
                                .danger(spec.danger)
                                .enabled(row.enabled);
                            if let Some(on) = row.checked {
                                style = style.checked(on);
                            }
                            // **On every row once it is on any of them**, which is
                            // the whole mechanism: a row told `false` does not read
                            // the pointer at all, so a hover cannot paint a second
                            // ground beside the keyboard's.
                            if let Some(hl) = highlight {
                                style = style.highlight(row.item == hl);
                            }
                            let resp = crate::ui::menu_row(ui, style, ROW_H);
                            // **The sentence hangs off the row's own response**,
                            // which is the only place it fires in both states: this
                            // row is neither a widget inside a `disable()`d `Ui` nor
                            // a `Ui::scope`, which are the two spellings that are
                            // silently dead (`inspector::menu_action`).
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

        // **Not on the frame that opened it** — see [`ContextMenu::just_opened`].
        if !just_opened && ctx.input(|i| i.pointer.any_click()) && !area.contains_pointer() {
            self.context_menu = None;
        } else if let Some(menu) = self.context_menu.as_mut() {
            menu.just_opened = false;
        }
    }

    /// Everything the registry needs to know about the app, read once.
    ///
    /// **`open` is the snapshot taken when the menu was opened**, not live state:
    /// the three clipboard questions are all in it, and asking them per frame is
    /// what [`ContextMenu::system_text`] and [`ContextMenu::system_image`] each
    /// explain they must not do. Passed whole rather than as a row of bools,
    /// because it *is* the snapshot and a fourth positional `bool` beside three
    /// others is a swap waiting to happen.
    fn menu_context<'a>(&'a self, open: &ContextMenu, kinds: &'a [Kind]) -> Context<'a> {
        let target = open.target;
        let state = match target {
            Target::Layer { id, .. } => self.layer_menu_state(id),
            _ => LayerState::default(),
        };
        Context {
            target,
            selection: self.session.selection.ids(),
            // **What a *Paste* row can actually do** — three payloads, in the order
            // both paste paths try them: a picture, then the app's own subtrees
            // while they are still what the OS clipboard describes, then text to
            // make a layer out of. Every term matters. Without the picture the row
            // is dim over a screenshot the chord pastes happily (§15 D224); without
            // `payload_current` it offers layers the chord would decline as stale;
            // without the text it goes dim on the commonest paste there is, a
            // sentence copied in another application (§15 D218).
            can_paste: open.system_image
                || (self.clipboard.is_some() || self.guide_clipboard.is_some())
                    && open.payload_current
                || open.system_text,
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
            present: self.present,
            segment_selected: self
                .edited_path()
                .is_some_and(|id| self.points.segments(id).next().is_some()),
            // **The open's world point, not this frame's pointer.** The menu is
            // rebuilt every frame it is up, so reading the live pointer would let
            // the row go dark under the hand as it travelled down the menu — and
            // the position the verb would act on is the one the menu was aimed
            // with, which is what `open.world` is.
            on_segment: open.world.is_some_and(|p| self.segment_at(p).is_some()),
            can_join: self.join_subject().is_some(),
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
            // A walk of the tree per frame the menu is up, which is the same shape
            // `any_guides` above has and cheaper than it looks: it stops at the
            // first layer with a spec on it in every document that has one.
            any_exports: self.document_exports_anything(),
        }
    }

    /// The parts of one layer that decide a row's label or its presence.
    fn layer_menu_state(&self, id: NodeId) -> LayerState {
        let Some(node) = self.session.doc.get(id) else {
            return LayerState::default();
        };
        let picture = tools::croppable_fill(node.paint());
        LayerState {
            hidden: !node.visible(),
            locked: node.locked(),
            // The **ancestors'** answer, asked of the parent so this node's own flag
            // is not folded in twice — `locked_within` derives the combination.
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
            // Core's own predicate, not a copy of it: the row must not offer what
            // `build::outline` would refuse, which is the rule every kind-gated row
            // here follows (§15 D87's lesson at a fifth site).
            outlineable: build::can_outline(node.kind()),
            // The **display** node, so a live text session's uncommitted mode is
            // what the ticks report — `set_text_sizing` reads the same side of the
            // preview, and a tick that disagreed with the verb beside it is worse
            // than no tick.
            text_sizing: match self.session.display_node(id).map(|n| n.kind()) {
                Some(NodeKind::Text { sizing, .. }) => {
                    Some(u8::try_from(sizing.cell()).unwrap_or(0))
                }
                _ => None,
            },
        }
    }

    /// Run one row.
    ///
    /// **Most of these are one line, and that is the design working.** A row and
    /// its chord are the same verb, so everything with a chord goes straight to
    /// `dispatch` and cannot come to mean something different from the key. What
    /// is left is the handful a keyboard has no way to say — the ones about *where
    /// the menu was opened*, and the ones about a target the keymap has no name
    /// for.
    fn perform_menu_item(
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
            //
            // **First, and guarded rather than given items of their own.** A row
            // and its chord must mean the same verb (see [`Item`]), and inside a
            // session these four chords are the editor's, not the keymap's:
            // `input::resolve` resolves nothing in `Mode::TextInsert`. So the verb
            // is shared and the *route* is the target's answer — which is the rule
            // `Item::Delete` already stands on. Without the guard these fell
            // through to the layer arms below, and *Paste* in a session pasted
            // layers onto the canvas.
            Item::Cut if in_text => self.cut_text_selection(ctx),
            Item::Copy if in_text => self.copy_text_selection(ctx),
            Item::PasteHere if in_text => self.paste_text_selection(),
            Item::SelectAll if in_text => self.select_all_text(),

            // --- straight to the keymap's own verb -------------------------
            Item::Cut => self.dispatch(ctx, Action::Cut),
            Item::Copy => self.dispatch(ctx, Action::Copy),
            Item::Duplicate => self.dispatch(ctx, Action::Duplicate),
            // One `Action` for three rows, because `Action::Delete` already asks
            // the same question this menu asked to decide which row to show: a
            // guide selection, a point selection, or the layers.
            Item::Delete | Item::DeleteGuide | Item::DeletePoints => {
                self.dispatch(ctx, Action::Delete)
            }
            Item::Group => self.dispatch(ctx, Action::Group),
            Item::Mask => self.dispatch(ctx, Action::Mask),
            // **The target, not the selection**, which is what a per-layer property
            // wants: the row is only offered on a single `Path` and C2 guarantees a
            // hit inside the selection left it whole, so the two agree — but naming
            // the target is what makes a right-click on an unselected path do what
            // the row says rather than acting on something else.
            Item::EvenOdd => {
                if let Some(id) = hit {
                    let rule = match self.session.doc.get(id).map(|n| n.fill_rule()) {
                        Some(ondin_core::FillRule::EvenOdd) => ondin_core::FillRule::NonZero,
                        _ => ondin_core::FillRule::EvenOdd,
                    };
                    self.commit_edit(Transaction(vec![Operation::SetFillRule { id, rule }]));
                }
            }
            // **The selection, not the hit**, which is *Group selection*'s rule and
            // has to be: the row is named for the selection and C2 guarantees a hit
            // inside it left it whole. `frame_selection` is a method rather than an
            // `Action` because nothing binds a chord to it (§15 D249) — if
            // `shortcuts.md` ever does, this becomes a `dispatch` like its sibling.
            Item::FrameSelection => self.frame_selection(),
            Item::Ungroup => self.dispatch(ctx, Action::Ungroup),
            Item::Boolean(op) => self.dispatch(ctx, Action::Boolean(op)),
            Item::Flatten => self.dispatch(ctx, Action::Flatten),
            // No `Action`, because no chord — see the spec entry. `hit` rather than
            // the selection, for C3's reason: the menu's subject is what is under the
            // pointer, and the row was only offered because that is one shape.
            // `hit` for `Item::OutlineShape`'s reason, and the verb re-checks its own
            // kind, so a row that outlived the state it was built against refuses.
            Item::OutlineText => {
                if let Some(id) = hit {
                    self.outline_text(id);
                }
            }
            // **The selection, not `hit`** — see the item's own doc: the pointer can
            // only be over one of the two layers this needs, so the row would mean
            // different things depending on which. Both verbs re-derive their
            // subjects, so a row that outlived the state it was built against
            // refuses rather than acting on the wrong thing.
            Item::DetachTextPath => self.detach_text_path(),
            Item::FlipTextPath => self.flip_text_path(),
            // Straight to the panel's own verb, which re-reads the node's kind and
            // its current cell — so a row that outlived either does nothing.
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
            Item::Restack(mv) => self.dispatch(ctx, Action::Restack(mv)),
            Item::Flip(axis) => self.dispatch(ctx, Action::Flip(axis)),
            Item::ToggleHidden => self.dispatch(ctx, Action::ToggleHidden),
            Item::ToggleLocked => self.dispatch(ctx, Action::ToggleLocked),
            Item::Rename => self.dispatch(ctx, Action::Rename),
            Item::ZoomSelection => self.dispatch(ctx, Action::ZoomSelection),
            Item::ZoomFit => self.dispatch(ctx, Action::ZoomFit),
            Item::SelectAll => self.dispatch(ctx, Action::SelectAll),
            Item::ViewSwitch(sw) => self.dispatch(ctx, Action::ToggleView(sw)),
            // Straight to the canvas verb rather than through an `Action`: nothing
            // binds a chord to either row, so there is no keyboard intent for a
            // `dispatch` to resolve (§15 D250, and D215's rule about which rows do).
            Item::SetSmooth(smooth) => self.set_point_smoothness(smooth),
            // **The stored open position**, which is the whole reason this row
            // exists rather than a chord: `insert_point` needs a point on the ink
            // and a keyboard has none (§15 D215). `add_point_at` re-asks
            // `segment_at` rather than trusting the gate, so a row that somehow
            // outlived the state it was built against does nothing instead of
            // guessing.
            Item::AddPointHere => {
                if let Some(p) = world {
                    self.add_point_at(p);
                }
            }
            // No chord and no `Action`: `shortcuts.md` spends nothing on it, and
            // the verb re-asks its own subject so a stale row does nothing.
            Item::Join => self.join_points(),
            Item::CopyProperties => self.dispatch(ctx, Action::CopyProperties),
            Item::PasteProperties => self.dispatch(ctx, Action::PasteProperties),
            // No chord: `shortcuts.md` binds nothing to it, so there is no keyboard
            // intent for a `dispatch` to resolve (D215's rule about which rows do).
            Item::ExportAll => self.dispatch(ctx, Action::ExportAll),
            Item::CopyAsSvg => self.copy_as_svg(ctx),
            Item::CopyAsPng => self.copy_as_png(),
            Item::ReverseSubpaths => self.dispatch(ctx, Action::ReverseSubpaths),

            // --- the ones a keyboard cannot express ------------------------
            //
            // **Paste at the pointer** — Figma's: `Ctrl+V` offsets by a fixed 20×20
            // and this aims. Not because a key *cannot* have a position —
            // `canvas::drop_point` reads one off a bare `&Context` — but because the
            // chord is deliberately position-free. See `OndinApp::paste_at`, which
            // carries the whole of that and the §7 row that wants otherwise. This
            // row's own `world` is neither: it is the right-click's screen position
            // through `CanvasState::to_world`, and the panel door supplies `None`.
            // **And the panel's door means a *slot* rather than a point** — as a
            // sibling above the row that was clicked, which is the last line of §1's
            // table and the one thing the two doors disagree about (§15 D226). The
            // match is on the *target*, not on `world`: a panel row has no world
            // point, so reading `None` as "the chord" is what left this row pasting
            // by the ordinary rule.
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
            // No `world` arm and no door: "in place" is the one placement that does
            // not depend on where the menu was opened, which is exactly what makes it
            // a second row rather than a mode of the one above.
            Item::PasteInPlace => self.dispatch(ctx, Action::PasteInPlace),
            // Illustrator's *Isolate Selected Group*, and the discoverable half of
            // a gesture that today has no other announcement. The same two steps as
            // `double_click_pick`'s group arm: step in, then select what is under
            // the pointer in there.
            // One method rather than the body inlined here, because `Enter` is the
            // second door onto the same verb (§15 D228) and a row and its key must not
            // come to mean different things.
            Item::EnterGroup | Item::EnterBoolean => {
                if let Some(id) = hit {
                    self.enter_container(id, world);
                }
            }
            // One gesture, two names: inside a boolean the key *is* child order, so
            // it is a reorder to index 0; everywhere else it is a designation on the
            // selection (`designate_key`). This row and `Alt+Shift`+click are the
            // two ways in; what the result *looks* like is drawn either way — see
            // the note beside the row's push.
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
            // **"Not moved"** — a state the model can say and a resolved pivot point
            // cannot report back. The Transform card's origin row asks for it too,
            // at 50/50; this is the one that reaches a whole selection.
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
                    // `world` straight through, `None` and all: the panel's door has
                    // no point to place a caret from, and it used to invent
                    // `Point::ZERO` for one — which is a position in the document
                    // rather than an absence of one. The row now selects the whole
                    // string there, as `Enter` does (§15 D228).
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
            // Reached only when `has_picture`, which is `tools::croppable_fill`
            // itself and not a second reading of it — so the row cannot be offered
            // on a picture the mode would then refuse. `begin_image_edit` is shared
            // with the other three doors; it used to be this arm's own three lines
            // with a note that a fourth door would earn a function, and one did.
            Item::EditImage => {
                if let Some(id) = hit {
                    self.begin_image_edit(id);
                }
            }
            // The *turned* size for a turned picture: what "original" means here is
            // the box the picture fills at one unit per pixel, and a quarter turn
            // swaps it.
            Item::OriginalSize => {
                if let Some((id, slot, brush)) = hit.and_then(|id| self.picture_slot(id))
                    && let Some((px, py)) = self.image_pixels(slot, &brush)
                    && let Some(img) = paint::image_of(&brush)
                {
                    let (w, h) = img.oriented_size(px, py);
                    self.resize_to_original(id, w, h);
                }
            }
            // `build::guides_of` collects the guides a *subtree* owns; this is the
            // whole-document case, which nothing else has needed.
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
        }
        ctx.request_repaint();
    }

    /// The first visible image fill of `id`, as *Original size* needs it — the one
    /// picture row left that reads the fill, the other three having gone to the
    /// Settings tab or nowhere (§15 D305).
    ///
    /// Asked through `croppable_fill` — the same predicate image editing and
    /// `double_click_pick` use — so a row here cannot offer what the mode would
    /// refuse.
    fn picture_slot(&self, id: NodeId) -> Option<(NodeId, PaintSlot, ondin_core::Brush)> {
        let node = self.session.doc.get(id)?;
        let i = tools::croppable_fill(node.paint())?;
        Some((
            id,
            PaintSlot::Fill(i),
            node.paint().fills.get(i)?.brush.clone(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(seq: u64) -> NodeId {
        NodeId { actor: 1, seq }
    }

    /// **R4 clears every floating thing, and it cleared four of seven** (§15
    /// D533) — `[S15.2-L1-03]`.
    ///
    /// ⚠️ *Seven*, not six: six is `openCanvasMenu`'s count in the design export,
    /// which `context-menus.md` §0 R4 inherited and the app outgrew by three
    /// popovers — and it is the number this doc comment had in its first draft,
    /// which is the entry about a miscounted set miscounting it.
    ///
    /// *"One slot across every floating thing, not just across the menus"*, and
    /// the measured survivors were `effect_menu` and `export_menu`: two real
    /// `Order::Foreground` `Area`s left over the canvas the menu was aimed at,
    /// which is the state R4 exists to make unreachable. Neither dismisses
    /// itself on the way, because both read `PointerButton::Primary` only —
    /// deliberately — so the secondary click that opened the menu is invisible
    /// to them.
    ///
    /// ⚠️ **`edited_image` is *not* in this set and the test asserts that**, which
    /// is the R4 rule applied rather than broken: that card is what the mode
    /// looks like, so clearing it would leave a right-click having silently
    /// half-left a mode — gestures still armed, outline still drawn, controls
    /// gone. It is the one member by which this set and
    /// `OndinApp::a_popover_is_open` differ, and asserting it here is what stops
    /// the two being "simplified" into one list.
    ///
    /// ⚠️ Flipped by dropping the three new clears: red at `effect_menu`, the
    /// first of them checked. `export_row_open` is asserted last and would not
    /// have fired.
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

    /// **Present mode does not refuse a context menu, and the other refusal is
    /// untouched** (§15 D756).
    ///
    /// `open_context_menu` opened `if self.gesture_cancelled || self.present`. The
    /// present half is gone: a context menu is the canvas answering a click, and
    /// the maintainer's D750 ruling is that present mode hides the app's *chrome*
    /// and leaves the canvas working. This is the second consequence of that
    /// ruling; `canvas::layout_grid_tests` holds the first.
    ///
    /// 🚨 **Both halves are asserted because the fix is a deletion inside a
    /// disjunction, which is the edit most likely to take its neighbour with
    /// it.** `if self.gesture_cancelled || self.present` and `if self.present`
    /// differ by three tokens, and a test that only opened a menu in present mode
    /// would be **green for either**. R1 — one right-click cancels *or* opens,
    /// never both — has its own §15 entry (D168) and is not what this change is
    /// about.
    ///
    /// ⚠️ **This asserts the refusal, not the drawing.** `context_menu_ui` is
    /// called outside `update`'s `!present` guard and always was, which is what
    /// made the change one deletion rather than a repair at two sites — see the
    /// comment there. A version that lifted the refusal and left the draw inside
    /// that guard would pass this test and paint nothing.
    ///
    /// **Two flips run.** Putting `|| self.present` back fails at *"present mode
    /// is a chrome switch"* — the predicted site. Deleting the whole condition
    /// fails at *"a press that cancelled a gesture still spends the click"*, which
    /// is the assertion that exists to catch exactly that slip.
    #[test]
    fn present_mode_opens_a_menu_and_a_cancelled_gesture_still_does_not() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);

        let mut app = crate::app::OndinApp::headless(&ctx);
        app.present = true;
        app.open_context_menu(&ctx, Target::Canvas, None);
        assert!(
            app.context_menu.is_some(),
            "present mode is a chrome switch, not a switch over what the canvas \
             answers — the menu is the canvas answering a click (§15 D750, D756)"
        );

        // R1, unchanged and deliberately re-asserted here: the deletion above is
        // one arm of a disjunction, and the neighbouring arm is the thing an
        // over-broad edit removes.
        let mut app = crate::app::OndinApp::headless(&ctx);
        app.gesture_cancelled = true;
        app.open_context_menu(&ctx, Target::Canvas, None);
        assert!(
            app.context_menu.is_none(),
            "a press that cancelled a gesture still spends the click (R1, §15 D168)"
        );

        // And the two are independent rather than one having replaced the other.
        let mut app = crate::app::OndinApp::headless(&ctx);
        app.present = true;
        app.gesture_cancelled = true;
        app.open_context_menu(&ctx, Target::Canvas, None);
        assert!(
            app.context_menu.is_none(),
            "a cancelled gesture refuses in present mode too"
        );
    }

    /// A menu open, with everything not named left at its quietest: nothing
    /// copied, no guides, every View switch off, and a text session with a bare
    /// caret in an empty buffer.
    ///
    /// `can_frame` is the exception and is **true** here, because it is the answer
    /// for artwork sitting on a page — the ordinary case — where every other field
    /// above has an ordinary case of "nothing". A default of `false` would leave
    /// *Frame selection* dim in every fixture in this file and make the one test
    /// that cares about the dimming pass by accident.
    fn open<'a>(target: Target, selection: &'a [NodeId], kinds: &'a [Kind]) -> Context<'a> {
        Context {
            target,
            selection,
            can_paste: false,
            kinds,
            state: LayerState::default(),
            any_guides: false,
            view: ViewState::default(),
            // `false`, which is the quiet default this fixture's doc asks for: it
            // dims nothing, so a test that cares about the present-mode dim has to
            // say so (§15 D758). The one that does is
            // `present_mode_dims_show_rulers_and_leaves_the_other_four_alone`.
            present: false,
            segment_selected: false,
            // Quiet, unlike `can_frame` below: this one has a test on **both** of
            // its states, so a `false` default cannot hide a row the way an
            // untested one would.
            on_segment: false,
            can_join: false,
            // **True**, for `can_frame`'s reason: text with something typed in it is
            // the ordinary case, and every fixture here that builds a text menu
            // wants the row live unless it says otherwise.
            text_outlineable: true,
            // `false`, unlike `text_outlineable` above: these two *add* rows
            // rather than dimming one, so a default of `true` would put *Detach
            // from path* and *Flip to other side* into every fixture in this file
            // and change what several row-count assertions are counting.
            on_a_rail: false,
            rail_flipped: false,
            // **True**, for `can_frame`'s reason: a layer with a fill of its own is
            // the ordinary case, and a `false` default would leave *Copy properties*
            // dim in every fixture and make the test that cares pass by accident.
            can_copy_props: true,
            has_key: false,
            has_props: false,
            system_text: false,
            text_selection: false,
            text_empty: true,
            can_frame: true,
            // **True**, for `can_frame`'s reason: the page menu's fixtures are not
            // about the export row, and a `false` default would leave it dim in all
            // of them and make a test that cares pass by accident.
            any_exports: true,
        }
    }

    /// **No row's label reaches its accelerator**, at the width the menu is
    /// actually drawn at.
    ///
    /// Reported from the machine: *Reverse subpath direction* ran **under**
    /// `Shift+D`. Nothing in `ui::menu_row` prevents that — the label and the
    /// accelerator are two `Painter::text` calls at fixed anchors, one left of
    /// `8 + 15 + 9` and one right of `width − 8`, so there is no layout to reflow,
    /// no wrapping and no clipping. A label that is too long simply overlaps, and
    /// **it does so silently and only for the one row that is too long** — which is
    /// why it survived every menu test in this file.
    ///
    /// Measured rather than eyeballed, because "does it fit" is a question about
    /// glyph advances at 11.5pt and 10.5pt and nobody can answer it by reading.
    /// The margin is 4pt: *Copy properties* and *Paste properties* already sit
    /// within 20pt of the accelerator column, so a tighter bound would fail on the
    /// next reasonable row and a looser one would not have caught the row that was
    /// reported.
    ///
    /// **The card lost two points on 2026-08-23** — `MENU_W` became the width it
    /// paints rather than the width of its content (§15 D307) — and the tightest
    /// row went with it: *Paste properties* clears by 17.7 where it cleared by
    /// 19.7. Measured over all 36 accelerator-carrying rows before the change was
    /// made, because a 2pt narrowing is exactly the size of thing that flips an
    /// assertion nobody re-ran. It does not; the bound is 4.
    ///
    /// ⚠️ Flipped by putting *Reverse subpath direction* back, where the label ends
    /// at 173.8 and `Shift+D` begins at 153.2 — an overlap of 20.6, so the
    /// assertion misses by 24.6 with the clearance on top.
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

        // `ui::menu_row`'s own anchors, against the row it is handed: the frame's
        // inner width, the label's left inset and the accelerator's right inset.
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

    /// **The height a menu is placed by is the height it paints**, which
    /// `height`'s own doc calls the only reason it is allowed to compute rather
    /// than measure.
    ///
    /// `menu_place` flips and clamps against a size taken *before* the `Area` is
    /// shown, so nothing on screen ever contradicts a wrong answer directly: the
    /// menu simply opens in the wrong place, and only near a viewport edge, which
    /// is the one state nobody exercises on purpose. So the arithmetic has to be
    /// checked against the drawing rather than against itself.
    ///
    /// Four numbers meet here and each was a hand-copy at some point —
    /// `ui::MENU_ITEM_H`, `ui::MENU_SEP_H`, the 1pt row gap, and `MENU_PAD` as the
    /// frame's margin. The 2026-08-20 compaction moved two of them: the row height
    /// (§15 D262) and the separator (§15 D263), whose *drawn* height and *computed*
    /// height were two `9.0`s in different files with nothing tying them.
    ///
    /// Driven over several shapes, because a single group exercises no separator
    /// and a single row exercises no gap — the two terms most likely to be wrong.
    ///
    /// ⚠️ Flipped by giving `SEP_H` back its own `9.0` while `menu_sep` draws 7,
    /// which is exactly the drift this pins: every menu with a separator in it
    /// fails, and the one-group case still passes.
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

    fn labels(groups: &[Vec<Row>]) -> Vec<&'static str> {
        groups.iter().flatten().map(|r| r.label).collect()
    }

    /// **Every row a menu can produce is in `Item::ALL`, and every entry there
    /// has a label, a glyph and a group.**
    ///
    /// One table-driven test, which is what stops the twelfth row being added
    /// without one. The second half is the one that matters: `ALL` is written by
    /// hand, so a new variant would be invisible to the first half — building
    /// every menu this app can open and demanding that each row is *in* the list
    /// is what makes the list answerable for the enum.
    #[test]
    fn every_registry_entry_has_a_label_a_glyph_and_a_group() {
        for item in Item::ALL {
            let spec = item.spec();
            assert!(!spec.label.is_empty(), "{item:?} has no label");
            assert!(!spec.glyph.is_empty(), "{item:?} has no glyph");
            // Reading `group` is the assertion: a variant that fell through
            // `spec`'s match would not compile, and one that returned a group it
            // does not belong to is what the ordering test below catches.
            let _ = spec.group;
        }
        let mut seen: Vec<Item> = Vec::new();
        for item in Item::ALL {
            assert!(!seen.contains(item), "{item:?} is in ALL twice");
            seen.push(*item);
        }

        // Every menu this app can open, so a row that exists only in one
        // projection is still held to the table.
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

    /// **A dimmed row always says why.**
    ///
    /// The state where the sentence carries the most information is the one where
    /// it is easiest to leave out, so this is asserted over every menu rather than
    /// trusted per call site.
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

    /// **The groups come out in the canonical order, whatever is filtered out of
    /// them** (§3).
    ///
    /// A row is always in the same place relative to the rows that survive beside
    /// it — a *Flatten* that is fourth in one menu and seventh in another is worse
    /// than no menu at all — and this is the assertion that makes that structural
    /// rather than a habit.
    ///
    /// **The fixture is asserted first.** A projection test over an empty menu
    /// passes for the wrong reason, and 16 is the count `docs/context-menus.md` §3
    /// arrives at for a primitive by hand: Clipboard 5, Structure 1, Order 4,
    /// Transform 2, State 3, Navigate 1. Structure has held **three** since
    /// *Frame selection* landed beside *Group selection* (§15 D249), which was 18 —
    /// and the **Properties** group, empty in the code until 2026-08-20, is the two
    /// rows that made it 20. **Export** was empty too and is the last two: *Copy as
    /// SVG* the same day, then *Copy as PNG* on 2026-08-22. It briefly held three —
    /// *Export as…* sat first, from the day its "waits on an export UI nobody has
    /// designed" stopped being true until the day the Export panel made it the
    /// slower of two doors onto one file (§15 D264).
    /// §3's own hand count has always included all three, so this number moving *up*
    /// to meet it is the file and the code converging rather than drifting — which
    /// is the direction worth noting, because a count that only ever moves up is a
    /// spec being implemented and one that moves down is a spec being abandoned.
    /// **§3's Export group is now complete** and this count had met §3's exactly at
    /// 22. **23 is *Use as mask*** (§15 D286), which is the first row to move this
    /// number since — and it moves it the honest way, a verb `context-menus.md`
    /// listed as a deliberate non-row becoming a real one because the model grew
    /// what it was waiting for.
    #[test]
    fn a_primitives_menu_is_twenty_three_rows_in_canonical_group_order() {
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
        assert_eq!(flat(&groups).len(), 23, "{:?}", labels(&groups));
        assert_eq!(groups.len(), 8, "the head is empty and must collapse");

        // **Asserting the sequence, not that it is sorted.** `build` buckets by
        // `Spec::group`, so "the groups come out in order" is now true by
        // construction and a test of it would pass against every possible
        // assignment of rows *to* groups. What can still be wrong is the
        // assignment — a row given the wrong `Group` in `spec()` lands in the
        // wrong place and the menu is still perfectly sorted. This is that
        // assertion, and it is the one that would have caught the View switches
        // sitting in `Group::State` and pushing *Zoom to fit* below them.
        assert_eq!(
            labels(&groups),
            [
                "Cut",
                "Copy",
                "Paste here",
                "Duplicate",
                "Delete",
                // §3's third group, between the clipboard and the structural verbs
                // — a second clipboard carrying an appearance rather than a layer.
                "Copy properties",
                "Paste properties",
                "Group selection",
                // Directly after its sibling, which is §4's order for it, and the
                // row that took this menu from 17 to 18 (§15 D249).
                "Frame selection",
                // After the group verbs and before the booleans, which is where it
                // belongs on the merits: with the group-wrapping arm it *is* one of
                // the container-making verbs. Took this menu from 22 to 23
                // (§15 D286). *Ungroup* is absent on a shape, so it sits directly
                // after *Frame selection* here and after *Ungroup* elsewhere.
                "Use as mask",
                // Last in Structure, which is §3's order for it — and the row that
                // took this menu from 16 rows to 17 (§15 D230).
                "Outline shape",
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
                // §3's ninth group — **the two clipboard rows and no file row**,
                // semantic before raster: the SVG arrives as editable groups and
                // shapes, the PNG as a picture of them, so the least lossy is the
                // one the hand reaches first.
                //
                // The count went 23 → 24 when *Copy as PNG* landed and back to 23
                // the same day when *Export as…* was removed, the Export panel's
                // own button having made it the slower of two doors onto the same
                // file. The two moves are unrelated and it is a coincidence that
                // they cancel; the *labels* below are the assertion that would have
                // caught either one alone.
                "Copy as SVG",
                "Copy as PNG",
            ]
        );
    }

    /// **A verb promoted into a kind's head is *moved*, not duplicated** (§3).
    ///
    /// The same verb twice in one menu is the first thing that makes a menu look
    /// generated rather than designed. Flipped against a `head_rows` that pushes
    /// *Flatten* and *Ungroup* without the Structure group dropping them, which is
    /// the obvious way to write it and puts both rows in twice.
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
        // And they are in the head, where the eye goes on this kind — before
        // anything from the clipboard group.
        let head = groups.first().expect("a boolean has a head");
        assert!(head.iter().any(|r| r.item == Item::Flatten));
        assert!(head.iter().any(|r| r.item == Item::Ungroup));

        // The four operations are checkable here, and exactly one is ticked.
        let ticked: Vec<&'static str> = groups
            .iter()
            .flatten()
            .filter(|r| r.checked == Some(true))
            .map(|r| r.label)
            .collect();
        assert_eq!(ticked, ["Subtract"]);
    }

    /// **The other end of §3's length range**, which that section quotes as a pair
    /// — a primitive at the short end and a boolean at the long one — and which had
    /// only its short end pinned.
    ///
    /// A range with one measured end is half a claim, and it is the *long* end that
    /// §3's own ceiling argument rests on: "a context menu that needs a scrollbar
    /// has stopped being one". So the number worth watching is this one, and it was
    /// the one nothing was counting.
    ///
    /// The arithmetic, so a future disagreement is a comparison and not an argument:
    /// head 3 (*Enter*, plus *Flatten* and *Ungroup* promoted) · Clipboard 5 ·
    /// Properties 2 · Structure 7 (four operations, *Group selection*, *Frame
    /// selection*, *Use as mask*) · Order 4 · Transform 2 · State 3 · Navigate 1 ·
    /// Export 2. Structure went 6 → 7 with the mask row (§15 D286).
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
        assert_eq!(flat(&groups).len(), 29, "{:?}", labels(&groups));
    }

    /// **The whole menu is present on a locked layer and every editing row is
    /// dimmed, while the rows that are the way back stay live** (§5.9).
    ///
    /// This is the menu worth building first, because it is the one that fixes
    /// something currently unreachable rather than merely faster: `hit_test`
    /// filters out anything locked, so the panel is the only door that can reach
    /// one, and *Unlock* is the only way out.
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
            ..LayerState::default()
        };
        let groups = build(&cx);
        // The fixture: exactly the 23 rows an unlocked primitive has. Nothing is
        // *missing* because it is locked — that is the claim; the whole menu is
        // present and it is the enabling that changes.
        assert_eq!(flat(&groups).len(), 23, "{:?}", labels(&groups));

        let row = |item: Item| {
            groups
                .iter()
                .flatten()
                .find(|r| r.item == item)
                .unwrap_or_else(|| panic!("{item:?} is missing from a locked layer's menu"))
        };
        // *Copy properties* is in the live list for `Item::Copy`'s reason and it is
        // worth naming: **reading a locked layer is not editing it**, and a lock
        // that stopped you looking at a colour would be a lock that made the layer
        // useless as a reference. Its partner is the one that writes, so it dims —
        // and dims with the *lock*'s sentence rather than the clipboard's, because
        // that is the refusal the user has to act on first.
        //
        // ⚠️ **This list named four and §5.9 names five, which is how *Rename*
        // came to be dimmed with nothing saying so** (§15 D529). The spec's
        // sentence is *"every editing row is dimmed while **Unlock**, **Show**,
        // **Rename**, **Copy** and **Zoom to selection** stay live"* — so the
        // list below is now that sentence, plus *Copy properties* for the reason
        // above. `Item::Rename` and `Item::ToggleHidden` were live-by-spec,
        // asserted by nothing, and one of them was wrong.
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

    /// **The canvas menu carries no selection rows, even with layers selected**
    /// (C3).
    ///
    /// The menu is decided by what is under the pointer, never by what is
    /// selected — which is the consequence of C3 worth asserting outright,
    /// because the tempting implementation reads the selection and would put a
    /// whole layer menu here.
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
        // And it does carry the View switches C7 needs, or *Lock guides* could not
        // be reversed from where it was set. ⚠️ **The count is deliberately not
        // spelled here** — it read *"the four"* until §15 D757 made it five, and
        // this assertion is about one row; the set is pinned by
        // `the_canvas_menu_carries_every_view_row_and_present_dims_only_the_rulers`.
        assert!(rows.contains(&Item::ViewSwitch(ViewSwitch::GuideLock)));
    }

    /// **The canvas menu's View rows are the View menu's, less the three §6.1
    /// keeps out — and present mode dims exactly one of them** (§15 D757, D758).
    ///
    /// 🚨 **Nothing pinned this set before.** `the_canvas_menu_ignores_the_selection`
    /// asserts *Lock guides* is present and no test asked what else was, so §15
    /// D755 could add a `ViewSwitch` to the top-bar menu and leave the canvas menu
    /// short with the whole suite green — which is exactly what happened for a
    /// session. The set is derived from `ViewSwitch::VIEW_MENU` rather than typed
    /// out, so the next row added to that array either appears here or fails here.
    ///
    /// ⚠️ **`Layers`, `Toolbar` and `Present` are the exclusion and it is a real
    /// rule, not an omission**: `context-menus.md` §6.1 — *"they are chrome-wide
    /// switches, the View menu is one click away, and Present mode hides the very
    /// chrome the canvas menu sits over"*. Deriving the expectation by subtracting
    /// them is what keeps this test from going green if somebody adds one.
    ///
    /// 🚨 **Only `Rulers` dims, and asserting the other four stay live is the
    /// load-bearing half.** Present mode overrides exactly one of the five —
    /// `rulers_on()` is `show_rulers && !present`, while `guides_on()` is
    /// `show_guides` alone and `grid_visible()` reads no present flag, both by §15
    /// D750. A version that dimmed the group would be wrong about four rows and
    /// would pass any test that only looked at *Show rulers*.
    ///
    /// **Three flips run.** Dropping `LayoutGrid` from `view_rows` fails at the set
    /// assertion — predicted site. Widening the dim to `cx.present` alone fails at
    /// *"present mode overrides only the rulers"*, the predicted site, and **not**
    /// at the rulers assertion, which is still satisfied. Removing the `dim_if`
    /// fails at *"Show rulers cannot do anything in present mode"*.
    #[test]
    fn the_canvas_menu_carries_every_view_row_and_present_dims_only_the_rulers() {
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
        // Derived, not typed: the View menu's rows less the three §6.1 excludes.
        let expected: Vec<ViewSwitch> = ViewSwitch::VIEW_MENU
            .into_iter()
            .filter(|sw| {
                !matches!(
                    sw,
                    ViewSwitch::Layers | ViewSwitch::Toolbar | ViewSwitch::Present
                )
            })
            .collect();

        let cx = open(Target::Canvas, &[], &[]);
        let got = view_rows_of(&cx);
        assert_eq!(
            got.iter().map(|(sw, _, _)| *sw).collect::<Vec<_>>(),
            expected,
            "the canvas menu's View rows are not the View menu's, less §6.1's three"
        );
        assert!(
            got.iter().all(|(_, enabled, _)| *enabled),
            "every View row is live when present mode is off"
        );

        let mut cx = open(Target::Canvas, &[], &[]);
        cx.present = true;
        let got = view_rows_of(&cx);
        assert_eq!(
            got.iter().map(|(sw, _, _)| *sw).collect::<Vec<_>>(),
            expected,
            "present mode removes a row rather than dimming it — §6 says kind \
             decides presence and state decides enabled"
        );
        let rulers = got
            .iter()
            .find(|(sw, _, _)| *sw == ViewSwitch::Rulers)
            .expect("the rulers row");
        assert!(
            !rulers.1,
            "Show rulers cannot do anything in present mode — `rulers_on()` is \
             `show_rulers && !present`, so the tick would move and the bars would not"
        );
        assert_eq!(
            rulers.2,
            Some("Present mode hides the rulers"),
            "a dimmed row has to say why, and the reason names the thing that would \
             have to change (§5.9)"
        );
        let live: Vec<ViewSwitch> = got
            .iter()
            .filter(|(_, enabled, _)| *enabled)
            .map(|(sw, _, _)| *sw)
            .collect();
        assert_eq!(
            live,
            expected
                .iter()
                .copied()
                .filter(|sw| *sw != ViewSwitch::Rulers)
                .collect::<Vec<_>>(),
            "present mode overrides only the rulers — guides and both grids stay \
             live there by §15 D750, and Lock guides is a property, not a visibility"
        );
    }

    /// **Each View row ticks from its own field**, which is the half a fifth field
    /// makes newly wrong-able (§15 D757).
    ///
    /// ⚠️ **The failure this guards is silent.** `ViewState::get`'s `LayoutGrid` arm
    /// returning `self.grid` — a plausible copy-paste, and precisely the wildcard
    /// §15 D675 removed — draws a *Show layout grid* row wearing the pixel grid's
    /// tick. Nothing renders differently enough to notice, and
    /// `view_state_answers_only_for_the_switches_it_holds` asks only whether the
    /// answer is `Some`.
    ///
    /// **Flip:** `ViewSwitch::LayoutGrid => Some(self.grid)` fails here naming
    /// *Show layout grid*, `true` against `false`. Predicted correctly.
    #[test]
    fn every_view_row_ticks_from_its_own_field() {
        // Every field distinct from its neighbours' where it can be: `grid` off and
        // `layout_grid` on is the pair a copy-paste would collapse.
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

    /// **A text session's four rows are about the *editor*, and its *Paste* reads
    /// a different clipboard from every other menu's** (§6.4).
    ///
    /// The second half is the bug this test exists for. The row was dimmed by the
    /// in-app clipboard of captured *layers* — so the state asserted first here is the
    /// one that got it exactly backwards: layers copied, no text on the system
    /// clipboard, and a *Paste* that offered itself and then pasted layers onto the
    /// canvas. Flip the predicate to `!cx.can_paste`, which is what every *other*
    /// *Paste* row now reads, and this fails on the first `enabled` and again on the
    /// last — because `can_paste` is true here and inserting at the caret is still
    /// impossible.
    ///
    /// **The fixture is asserted before the availability**, because a projection
    /// test over a one-row menu would pass three of these four assertions by
    /// finding nothing at all.
    ///
    /// **It holds the line for each new payload `can_paste` learns about**, without
    /// naming any of them: `can_paste` is the only channel by which a payload
    /// reaches the registry, so "true here and the row still dim" is the assertion
    /// whatever made it true. A picture is the third one (§15 D224) and a caret has
    /// no more use for a screenshot than for a captured subtree.
    #[test]
    fn a_text_sessions_paste_reads_the_system_clipboard_and_not_the_layer_one() {
        let sel: [NodeId; 0] = [];
        let kinds: [Kind; 0] = [];
        let mut cx = open(Target::TextSession, &sel, &kinds);
        // The app holding a pasteable payload, and nothing on the system clipboard —
        // which is exactly the state `can_paste` says yes to and this row must not.
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

        // The three states the rows actually turn on, one each.
        cx.system_text = true;
        cx.text_selection = true;
        cx.text_empty = false;
        let groups = build(&cx);
        let rows = &groups[0];
        assert!(rows.iter().all(|r| r.enabled), "{:?}", labels(&groups));
    }

    /// **A row whose only purpose is to undo a non-default state is omitted when
    /// that state is default** (§3's one exception), rather than dim on every
    /// open — which is indistinguishable from broken.
    ///
    /// Both halves, because "always absent" also passes the first one.
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

    /// **`height` counts what the rows actually paint.**
    ///
    /// `menu_place` needs the size before the `Area` is shown, so this number is
    /// computed rather than measured — which is only allowed because it is exact.
    /// Two groups of one row: two rows, one separator, and the gap either side of
    /// it, over the card's padding and its border.
    ///
    /// **The term-by-term companion to
    /// `a_menus_computed_height_is_the_height_it_paints`**, and worth keeping beside
    /// it rather than folding in: that one proves the total against the drawing and
    /// would pass if two terms were wrong in opposite directions, while this one
    /// says which term is which. It is also the test that did *not* catch the
    /// missing border, because it was written from the same arithmetic it checks
    /// (§15 D263) — which is exactly why the other one measures instead.
    #[test]
    fn a_menus_height_is_its_rows_its_separators_and_its_gaps() {
        // The frame's 1pt border, both sides, on top of everything else.
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

    /// **The picture head is two rows and a lock reaches both of them** (§5.7).
    ///
    /// Both write the node's geometry, which is the whole reason they agree:
    /// *Original size* resizes it outright, and a crop is a resize too —
    /// `crop_resize_tx` goes through `tools::resize_layer`.
    ///
    /// ⚠️ **Read the next paragraph before touching this test.** It asserted the
    /// *opposite* until 2026-08-23 — *Edit image* live beside a dimmed *Original
    /// size* — and it carried a warning that the plausible wrong version was now "a
    /// hand seeing a single dimmed row beside a live one and making them agree".
    /// **That warning was right, and this is not that.** The rows do not agree
    /// because somebody tidied them; they agree because
    /// `crate::app::OndinApp::begin_image_edit` — the one funnel all four doors
    /// pass through — now refuses on a locked layer, so `Enter` refuses too (§15
    /// D320). §15
    /// D261's rule is that a menu must not refuse where the chord allows, and it is
    /// kept by the mode changing first and the row following. **The order matters
    /// more than the outcome**: dimming this row while the mode still allowed it
    /// would be the defect even though the assertion below would read identically.
    ///
    /// So the guard on this test is not here. It is
    /// `app::image_edit_lock_tests::a_locked_picture_refuses_every_door`, which pins
    /// the refusal this row is only the visible half of — if that test goes, this
    /// one is asserting a tidy-up.
    ///
    /// ⚠️ Flipped both ways. Dropping `dim_if(locked, why)` from either row fails
    /// that row's assertion and not the other's, so this is still two claims rather
    /// than one shared condition — which is what it was worth having when the two
    /// disagreed and is still worth having now they do not.
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
        // The fixture: the head is exactly these two, in this order, or the claims
        // below are about a menu that never had a picture head at all.
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

    /// **A layer inside a locked group dims the same rows and gives a different
    /// reason** (§15 D321).
    ///
    /// Two claims, and the second is the one worth the test. A locked *group* locks
    /// its contents, so the rows dim — but the layer's **own** flag is clear, which
    /// means "This layer is locked" would send the hand to a toggle that is already
    /// off. The reason a dimmed row gives has to name the thing that would have to
    /// change, so it names the group.
    ///
    /// And *Unlock* must not appear: `ToggleLocked` reads the own flag on purpose,
    /// because a row offering to unlock a layer that is not locked does nothing
    /// visible and reads as a broken control. That is the whole reason `LayerState`
    /// carries two facts rather than one.
    ///
    /// ⚠️ **Flipped three ways.** Making `locked_within` read `locked` alone leaves
    /// every row live; making `why` unconditional gives the wrong sentence; and
    /// pointing `ToggleLocked`'s label at `locked_within()` turns *Lock* into
    /// *Unlock* here. Each fails one assertion below and no other.
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
            // The ancestor's lock, and **not** the layer's own — the whole point.
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
            "the toggle reads the layer's own flag: offering *Unlock* here would \
             write a flag that is already clear and appear to do nothing"
        );
    }

    /// *Edit image* is the picture's head verb: present whenever there is a
    /// picture, absent when there is not, and **first** of the picture rows.
    ///
    /// The order is the assertion worth having. `context-menus.md` §5 argues the
    /// head exists so that a mode entered only by double-click has a menu repair,
    /// and a repair that sits below *Export original…* is one the hand does not
    /// find. Presence alone would pass with the row pushed last.
    ///
    /// ⚠️ Flipped both ways: pushing the row after *Original size* fails the order
    /// assertion alone, and gating it on something other than `has_picture` fails
    /// the absence one — the presence assertion survives both, which is why it is
    /// not the only one here. The order was checked against four rows until
    /// 2026-08-23 and is checked against one now; it is still the assertion worth
    /// having, because "first" is the whole of what makes it a repair.
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

    // **`export_original_dims_for_a_linked_picture_and_says_something_else_for_a_missing_one`
    // was here** and went with the *Export original…* row on 2026-08-23. Its
    // subject was that row's `dim_if`; nothing in this file reads
    // `tools::original_refusal` any more. What it actually proved — that a
    // *missing* picture and a *linked* one get different sentences, D280's finding
    // — was moved to a `tools::refusal_tests` for an hour, until the inspector's
    // door went too and `original_refusal` had no caller left. **A test of a
    // deleted function is not a test**, so it went with it; the finding is written
    // where that function stood, and D305 says to rebuild from there rather than
    // re-derive it, re-deriving being what produced the two-state version.

    /// A locked layer's menu — the one fixture in the file that is *mostly* dimmed,
    /// which is what makes it the right one for the keyboard walk.
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

    /// **`↓` visits only the rows a press could actually run** (§8).
    ///
    /// A dimmed row says why it is dim through a tooltip, and a keyboard cannot
    /// hover — so a highlight that stopped on one would be a dead end with no way
    /// to read *why* it is dead. That is the whole argument, and it is invisible in
    /// the source: a walk over `groups.iter().flatten()` is the obvious spelling
    /// and passes any test that only asks whether the highlight moved.
    ///
    /// **Flipped against exactly that** — dropping the `.filter(|r| r.enabled)` —
    /// where the first `↓` on this fixture lands on *Cut*, which is dimmed because
    /// the layer is locked, and `Enter` there does nothing at all.
    #[test]
    fn the_keyboard_walk_visits_only_the_rows_a_press_could_run() {
        let groups = locked_menu();
        // Assert the fixture: this menu has to hold both kinds of row, or the
        // filter is being tested against nothing. A locked layer keeps seven live.
        let live: Vec<Item> = groups
            .iter()
            .flatten()
            .filter(|r| r.enabled)
            .map(|r| r.item)
            .collect();
        let all = flat(&groups).len();
        assert_eq!(all, 23, "{:?}", labels(&groups));
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

        // One `↓` per live row, from nothing, walks the live rows in draw order
        // and then wraps to the first.
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

        // And `↑` is the same run backwards.
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

    /// **`↑` on a fresh menu is the last row, and `Home`/`End` are the two ends of
    /// the live run rather than of the list** (§8).
    ///
    /// `↑` from nothing is the reason the key is worth pressing at all — *Delete*
    /// and *Rename* live at the bottom of every layer menu — and it is the one arm
    /// that a naive `unwrap_or(0)` gets wrong while every other assertion here
    /// still passes.
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
        // From anywhere, `Home` and `End` are absolute.
        assert_eq!(navigate(&groups, Some(last), Step::First), Some(live[0]));
        assert_eq!(navigate(&groups, Some(live[0]), Step::Last), Some(last));
        // The last row of the *list* is dimmed here, so `End` landing on it would
        // be the same bug the walk test flips against, seen from the other end.
        let bottom = flat(&groups).last().copied().expect("16 rows");
        if bottom != last {
            assert_ne!(
                navigate(&groups, None, Step::Last),
                Some(bottom),
                "`End` must be the last *live* row"
            );
        }

        // A highlight the rebuild left behind restarts the run rather than staying
        // lost — and restarts it at the end the key was heading for.
        let stale = flat(&groups)
            .into_iter()
            .find(|i| !live.contains(i))
            .expect("the fixture has a dimmed row");
        assert_eq!(navigate(&groups, Some(stale), Step::Next), Some(live[0]));
        assert_eq!(navigate(&groups, Some(stale), Step::Prev), Some(last));

        // A menu with nothing live has nowhere to go, and says so rather than
        // picking a dimmed row.
        let dead = vec![vec![Row::new(Item::Cut).dim_if(true, "no")]];
        for step in [Step::Next, Step::Prev, Step::First, Step::Last] {
            assert_eq!(navigate(&dead, None, step), None, "{step:?}");
        }
    }

    /// **Where *Outline shape* appears, where it is dim, and that it is never offered
    /// beside *Flatten*** (§4, §15 D230).
    ///
    /// The last claim is the one worth a test: the two rows are different verbs about
    /// the same part of the menu, and offering both at once would be the menu asking
    /// the user to tell them apart. It is meant to be true *by construction* — `one`
    /// excludes a set and `Shape | Path` excludes a boolean — and "by construction"
    /// is exactly the kind of claim that stops being true when a condition is widened
    /// later.
    ///
    /// ⚠️ **Flipped by dropping `one`**, which is the condition somebody removes when
    /// **The even-odd row is offered on a path and on nothing else** (§15 D239).
    ///
    /// The exclusions are the whole content of the rule and each has its own reason.
    /// A **shape** — rect, ellipse, star, polygon — is a single non-crossing outline,
    /// so the two rules agree and a control there would visibly do nothing. A
    /// **boolean** derives its rule from its operation, so a row appearing to toggle
    /// it would be offering to make an `Exclude` wrong. A **multi-selection** has no
    /// single answer for the check mark to report.
    ///
    /// ⚠️ **The tick reads the stored rule, not `Node::fill_rule`** — which is only
    /// safe *because* the row is path-only, since those are the same there and differ
    /// on exactly the kind excluded above.
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

    /// they want the row on a multi-selection: *Outline shape* then appears next to
    /// *Flatten* on two rects, and the walk below finds both.
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

        // A primitive: present and live.
        let m = one_of(&[Kind::Shape], outlineable);
        assert_eq!(row(&m, Item::OutlineShape), Some((true, None)));
        assert!(!has(&m, Item::Flatten), "and Flatten is not beside it");

        // A path with radii still in the model: the same.
        let m = one_of(&[Kind::Path], outlineable);
        assert_eq!(row(&m, Item::OutlineShape), Some((true, None)));

        // A path with nothing to bake: **present and dim**, which is §3's rule rather
        // than absence — the kind can be outlined, this instance has nothing to gain.
        let m = one_of(&[Kind::Path], LayerState::default());
        assert_eq!(
            row(&m, Item::OutlineShape),
            Some((false, Some("This path is already an outline")))
        );

        // Locked wins over that, because "unlock it" is the actionable half.
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

        // The four kinds it is absent on, each for its own reason — and on the boolean
        // and the pair, *Flatten* is there instead, which is the mutual exclusion.
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

    /// **The three sizing rows tick exactly one, and only over a single text
    /// layer** (§5.6).
    ///
    /// Two claims. **Exactly one tick** is what makes them read as a mode rather
    /// than as three switches — the failure being three unticked rows, which says
    /// the node is in none of three states it must be in one of. And **absent over
    /// a pair**, because the tick is a fact about one node: two text layers in
    /// different states have no honest answer to put there, and §3's head rule
    /// ("every member qualifies") is not strict enough on its own — both members
    /// *are* text.
    ///
    /// ⚠️ Flipped by taking the tick from a constant `TextSizing::Auto` (all three
    /// menus tick *Auto width*, so the second assertion fails on two of the three)
    /// and by pushing the rows on `all_are(Kind::Text)` alone without the
    /// `Option` (the pair grows three rows and the last assertion fails).
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
            let ticked: Vec<&'static str> = m
                .iter()
                .flatten()
                .filter(|r| r.checked == Some(true))
                .map(|r| r.label)
                .collect();
            assert_eq!(
                ticked,
                [["Auto width", "Auto height", "Fixed size"][usize::from(cell)]],
                "cell {cell} must tick itself and nothing else"
            );
            // The fixture: all three rows are there to be ticked, so "exactly one"
            // is a claim about a choice rather than about a menu with one row in it.
            assert_eq!(
                m.iter()
                    .flatten()
                    .filter(|r| matches!(r.item, Item::TextSizing(_)))
                    .count(),
                3
            );
        }

        // ⚠️ **`Some`, not `None`, and that is the whole repair of this
        // assertion** (§15 D530). It closed with `menu(2, None)` — and `None` is
        // the fixture's *own hand-set* `LayerState::text_sizing`, not what
        // `layer_menu_state` returns. Nothing in the suite called that function,
        // and it reads the field off the **hit** node alone, so the `Option` can
        // never be `None` while the hit is text: the state this asserted about is
        // one the app cannot be in. *What would also pass it*: exactly the code
        // that shipped, which pushed the rows on `all_are(Kind::Text)` with no
        // `one`. Handing it the state a real two-layer selection produces is what
        // makes it about the guard.
        assert!(
            !menu(2, Some(0))
                .iter()
                .flatten()
                .any(|r| matches!(r.item, Item::TextSizing(_))),
            "two text layers have no single state for the tick to report, \
             whatever the hit layer's own state is"
        );
    }

    /// ***Convert to path* is text's own row in *Outline shape*'s slot**, and the
    /// two are never in one menu (§5.6, §15 D260).
    ///
    /// Three claims, and the third is why the test exists rather than the first:
    ///
    /// - It is on **text** and nothing else. The kind list above already asserts
    ///   *Outline shape* is absent there; this is the other half of that pair, and
    ///   without it "text needs its glyphs converted" is a comment about something
    ///   that does not exist.
    /// - It **dims on empty content and says why**, which is the state
    ///   `build::can_outline_text` was written for.
    /// - It lands in **Structure**, beside where its sibling lands, rather than in
    ///   the text head with *Edit text*. §3 groups by what a verb *does*, and this
    ///   one replaces the layer — filing it in the head is the obvious reading of
    ///   §5.6's table, which lists it as a text row, and it would put a
    ///   one-way conversion directly under *Edit text* where the eye goes first.
    ///
    /// ⚠️ Flipped by pushing it in `head_rows` beside *Edit text* (the third
    /// assertion fails, the first two pass) and by dropping the `can_outline_text`
    /// dim (only the second fails).
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

        // Absent on every kind that is not text — including the pair, where a set
        // is Flatten's however convertible one member is.
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

        // Structure, not the head — asserted as *which group*, since asserting a
        // position would pass against any grouping that happened to sort the same.
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

    /// ***Frame selection* is offered wherever *Group selection* is, and in one place
    /// it is not** (§4, §15 D249).
    ///
    /// The interesting half is the frame: `build::group` refuses one outright, so the
    /// *Group selection* row is **absent** on a selection of frames — and framing is
    /// the verb that does have an answer there, because frames nest (§5.3). A pair of
    /// rows that agreed about every kind would be one row.
    ///
    /// The other half is the dimming, which is about *where the layers sit* rather
    /// than what they are: an `Artboard` cannot go inside a `Group`, so a selection
    /// in one is dim with a sentence rather than absent — §3's rule, and it is the
    /// difference between a row you can learn from and one that fails afterwards.
    ///
    /// ⚠️ Flipped by pushing the row inside the `!cx.kinds.contains(&Kind::Frame)`
    /// arm above it — which is the spelling somebody arrives at by copying the line
    /// for *Group selection*, and it is wrong in exactly the case that makes this
    /// verb worth having. Also flipped by dropping the `can_frame` dim, where the row
    /// is live inside a group and answers with `Cannot frame: …` after the click.
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

        // Beside *Group selection* on ordinary artwork, and live.
        let m = menu(&[Kind::Shape, Kind::Shape], true);
        assert_eq!(row(&m, Item::FrameSelection), Some((true, None)));
        assert!(row(&m, Item::Group).is_some(), "the fixture: Group is here");

        // On a frame, *Group selection* is gone and this one is not. That is the
        // whole reason it is a second row rather than a rename.
        let m = menu(&[Kind::Frame], true);
        assert_eq!(row(&m, Item::Group), None, "a frame cannot be grouped");
        assert_eq!(row(&m, Item::FrameSelection), Some((true, None)));

        // Inside a group: present, dim, and saying why.
        let m = menu(&[Kind::Shape], false);
        assert_eq!(
            row(&m, Item::FrameSelection),
            Some((false, Some("A frame cannot go inside a group")))
        );
    }

    /// ***Add point here* is the point menu's first row, and it is dim exactly
    /// when the click missed the ink** (§6.5).
    ///
    /// Three assertions and each one is a different way of getting it wrong:
    ///
    /// - **Alone in the head**, which is a claim about the hairline under it and
    ///   *not* about the order. That distinction cost a flip to find: filing the
    ///   row under `Clipboard` beside *Delete points* — the obvious place, since
    ///   both act on points — still puts it first, because `points_menu` emits it
    ///   first and within a group the emitted order is kept. What the group decides
    ///   here is only whether the two share a group, so that is what is asserted;
    ///   an `assert_eq!(first, AddPointHere)` passes under both spellings and would
    ///   have been a test about nothing.
    /// - **Dim with a sentence, not absent.** §3's split, and the failure it is
    ///   guarding is a row that vanishes when the right-click lands two pixels off
    ///   the outline: the menu would then change length under the hand for a reason
    ///   nothing on screen explains.
    /// - **Live on the ink.** The state the row exists for.
    ///
    /// ⚠️ Flipped against the plausible wrong versions: `Group::Clipboard` in the
    /// spec (the first assertion fails, the other two pass), and pushing the row
    /// only `if cx.on_segment` (the second fails, the other two pass). Neither is
    /// caught by the other's assertion, which is why all three are here.
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

    /// **The two property rows dim on different questions, and *Copy properties*
    /// gives a different reason to each of its two refusals** (§3's Properties
    /// group).
    ///
    /// The pair looks like one row twice and is not: one asks whether there is a
    /// layer to read *from*, the other whether anything has been read *yet*. The
    /// interesting half is the first, because it refuses for two unrelated reasons
    /// — several layers with no key layer is ambiguous, one layer with no paint of
    /// its own is empty — and a single sentence would be wrong in one of them.
    ///
    /// ⚠️ Flipped by collapsing the two sentences into one, which passes every
    /// assertion here except the last; and by hanging *Paste properties* on
    /// `can_copy_props` instead of `has_props`, which is the copy-paste slip the
    /// pair invites, and which makes it live with an empty clipboard.
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

        // Nothing copied yet: the source row is live, its partner is not — which is
        // the state the pair is in on every fresh document.
        let m = menu(1, true, false);
        assert_eq!(row(&m, Item::CopyProperties), (true, None));
        assert_eq!(
            row(&m, Item::PasteProperties),
            (false, Some("No properties have been copied yet"))
        );

        // And once something is copied, only the second changes.
        let m = menu(1, true, true);
        assert_eq!(row(&m, Item::PasteProperties), (true, None));

        // The two refusals of the source row, told apart by the selection's size.
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
        // And the third: a key **is** set and has no paint — a key on a group is
        // the ordinary way in. Telling the user to set a key here would be telling
        // them to do a thing they have already done.
        assert_eq!(
            row(&keyed(2), Item::CopyProperties),
            (
                false,
                Some("The key layer has no fill or stroke of its own")
            )
        );
    }

    /// **No menu offers the same verb twice**, which is what lets a keyboard
    /// highlight *be* an `Item` rather than an index (§8).
    ///
    /// `navigate` finds the highlighted row by its verb, so a menu with *Flatten*
    /// on it twice would make `↓` jump between two rows that look the same. §3's
    /// rule that a promoted row is *moved* and not duplicated is what makes this
    /// true; `a_boolean_promotes_flatten_and_ungroup_without_repeating_them` asserts
    /// it for the one kind that promotes, and this asserts it for every menu the app
    /// can open — including the ones nobody thought to promote from.
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

    /// **The two rail rows need a rail, and an ordinary text layer offers
    /// neither** (§15 D405, narrowed by D409).
    ///
    /// ⚠️ **There is no *Text on path* row any more and that is what this test is
    /// now mostly about.** It existed for a selection of a text layer *and* a
    /// shape, and the Text tool's own gesture replaced it (§15 D408) — hover an
    /// edge, click, type. Asserting its absence on the pair is what would catch it
    /// being reinstated by someone who found the gesture and not this note.
    ///
    /// The rows that remain are the two with no gesture of their own: *Flip to
    /// other side* and *Detach from path*, both gated on the same state, because a
    /// text layer either has a rail or does not.
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

        // A lone text layer on no rail: neither row.
        let plain = menu(1, false);
        assert!(
            !has(&plain, Item::DetachTextPath) && !has(&plain, Item::FlipTextPath),
            "an ordinary text layer offers neither: {:?}",
            labels(&plain)
        );

        // A text layer *and* a shape — the selection the deleted row was for.
        // Nothing appears: the gesture is the only way in now.
        let pair = menu(2, false);
        assert!(
            !has(&pair, Item::DetachTextPath) && !has(&pair, Item::FlipTextPath),
            "selecting a text layer with a shape offers no rail verb: {:?}",
            labels(&pair)
        );

        // Already railed: both, and only here.
        let railed = menu(1, true);
        assert!(
            has(&railed, Item::DetachTextPath) && has(&railed, Item::FlipTextPath),
            "a railed layer offers the way back and the flip: {:?}",
            labels(&railed)
        );
    }

    /// ***Flip to other side* ticks itself, and the tick is the state and not the
    /// verb** (§15 D406).
    ///
    /// **The failure this is aimed at is a tick that never moves**, which is what a
    /// row hard-coded to `checked(false)` looks like — it offers the flip, the flip
    /// works, and the menu goes on saying the type is the way round it started.
    /// That reads as the toggle not having taken, and the second click as the fix,
    /// which then flips it back.
    ///
    /// ⚠️ **Asserted as `Some(false)` rather than "not `Some(true)`".** `checked`
    /// is an `Option`, and `None` is a row that is *not checkable at all* — a
    /// perfectly plausible slip when adding a row, and one that draws no tick in
    /// either state while passing any assertion phrased as an inequality.
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
    //! No `ViewSwitch` borrows another's chord or another's state (§15 D675).
    //!
    //! Plain backticks throughout, per §15 D319 — this is a `#[cfg(test)]` module
    //! and `cargo doc` cannot see it.

    use super::*;

    /// **Eleven switches, eleven chords, no two the same** (`[S15.2-L3-07]`).
    ///
    /// `Item::spec`'s accelerator arm closed on `_ => "Ctrl+'"`, so seven of the
    /// eleven would have painted the grid's chord — four of them switches that have
    /// chords of their own. The chord comes from `ViewSwitch::accel` now, which is
    /// an exhaustive `match` beside `label`, and this asserts that the registry
    /// still asks it rather than answering for itself.
    ///
    /// ⚠️ **`SnapBaselines` is the `None`, and it is the assertion that keeps this
    /// from being a distinctness check over ten strings** — a `None` must stay
    /// legal, or the next chordless row gets a borrowed chord to satisfy the test.
    ///
    /// **Flip:** put back `Some(match sw { … _ => "Ctrl+'" })` in `Item::spec` and
    /// the first assertion fails — *"Show layers took its chord from somewhere
    /// other than the switch: left `Some("Ctrl+'")`, right `Some("Ctrl+Alt+\\")`"*.
    /// ⚠️ Site predicted correctly and the **assertion** was not the one expected:
    /// it fails on the registry-asks-the-switch check rather than on the
    /// distinctness scan below it, because that check runs first and is strictly
    /// stronger. The distinctness scan is what would catch a *wrong* chord written
    /// into `accel` itself, which is the other half.
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

    /// **Every switch is on exactly one of the two menus** (§15 D683,
    /// `[S17-L3-08]`).
    ///
    /// `VIEW_MENU` and `SNAP_MENU` are written by hand, with one consumer between
    /// them and **nothing asserting that the two of them cover the enum** before
    /// this test. A new variant compiles green: `label` forces a string for it,
    /// `set_view_switch`/`view_switch` force a field, the keymap can give it a
    /// chord — and it appears in neither menu, silently. That is the exact failure
    /// the type was introduced to prevent, in `Action::ToggleView`'s own words:
    /// *"the same set the View and Snap menus show, so a row and its chord cannot
    /// come to mean different things."*
    ///
    /// 🚨 **Membership, not cardinality.** `assert_eq!(VIEW.len() + SNAP.len(),
    /// ALL.len())` is the obvious test and is the trap `[S6.3-L6-04]` is about: it
    /// passes for two arrays that both hold `Grid` and neither holds `Present`. So
    /// this asks each switch which menus it is on and demands exactly one.
    ///
    /// **Flip:** drop `Present` from `VIEW_MENU`, one row shorter, and this
    /// fails naming *"Present mode"* with `0` menus. Predicted correctly.
    ///
    /// ⚠️ **The lengths were spelled out here as `[Self; 7]` and `[Self; 4]` and
    /// the enum as eleven, and §15 D755's twelfth variant made two of the three
    /// wrong** — the entry that predicted a twelfth variant being the one its
    /// arrival falsified. The numbers are gone rather than corrected: this test is
    /// **membership and not cardinality**, so no count in this comment is
    /// load-bearing and any count in it is a gate nobody built.
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

    /// **`ViewState` answers for the four it holds and for nothing else**
    /// (`[S15.2-L3-07]`).
    ///
    /// The `_ => self.grid` this replaces answered for seven switches with the
    /// grid's state on a struct with four fields — there was no value it *could*
    /// return. The five are the ones `view_rows` and `guide_menu` build rows for.
    ///
    /// ⚠️ **Four when this was written and five since §15 D757**, which is the
    /// assertion doing its job rather than a number to keep in step: adding
    /// `layout_grid` to the struct without adding its arm to `get` leaves it
    /// answering `None` for a switch it now holds, and this test names the set
    /// rather than counting it.
    ///
    /// **Flip:** put the wildcard back and the second half fails on `Layers`, which
    /// comes back `Some(true)` — the grid's state, on a switch this struct has
    /// never held. Predicted correctly.
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
