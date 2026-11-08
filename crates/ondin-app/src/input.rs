//! Mode-aware input routing (§9.3, §13 "seams v1 must reserve").
//!
//! Every keystroke resolves against an explicit [`Mode`] before it becomes an
//! [`Action`]; Command Mode (§13) would be a third arm of the same match.
//!
//! Chrome focus is part of routing: if egui owns the keyboard (a focused text
//! field), the canvas sees nothing.

use crate::tools::Tool;
use eframe::egui;
use ondin_core::build;
use ondin_core::kurbo::Vec2;

/// The editing mode. vim-for-design: navigate, insert, and (later) command,
/// with a visible indicator — not a pile of context-dependent shortcuts.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    /// Selecting, drawing, transforming. Single-key tool switches live here.
    #[default]
    Normal,
    /// Typing into a text node on the canvas. Keys are content, not commands.
    TextInsert,
    // Command — the embedded CLI (§13) — slots in here.
}

impl Mode {
    /// Short label for the status line's mode indicator.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::TextInsert => "INSERT",
        }
    }
}

/// What *Select same* compares between the key layer and every other layer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Same {
    /// The whole fill list, by value.
    Fill,
    /// The whole stroke list, by value.
    Stroke,
    /// A text layer's family, its own or any run's.
    Font,
    /// The whole effect stack, by value.
    Effect,
}

impl Same {
    pub const ALL: [Same; 4] = [Same::Fill, Same::Stroke, Same::Font, Same::Effect];
}

/// A resolved intent. Actions are produced by [`resolve`] and consumed by the
/// app; no key handling happens anywhere else, so the whole keymap is readable
/// in one place.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Action {
    Undo,
    Redo,
    /// `Ctrl+S` — write the document **and pin a version**.
    ///
    /// There is no `SaveAs`: documents live in the base folder and autosave there;
    /// branching a copy is *Duplicate* in the dashboard's file menu.
    Save,
    Open,
    /// `Ctrl+N` — create a document in the library and open it.
    ///
    /// The dashboard's *New file* verb (`OndinApp::new_library_document`). The
    /// project it lands in is computed at the door: the dashboard reads the nav
    /// being viewed, this reads the open document's own project.
    NewDocument,
    /// `Ctrl+W` — close the tab in front (`OndinApp::close_tab`); the next tab
    /// comes forward, else the library.
    CloseDocument,
    /// A tab key, from the palette. [`resolve`] never yields it: the chords are read
    /// by `tabs::read_keys` before the keymap, on both screens.
    Tab(crate::tabs::TabKey),
    /// `Ctrl+,` — the Settings modal (`crate::settings`).
    ///
    /// Open, not toggle: while the modal is up [`resolve`] is not called, so the
    /// chord can never arrive to close it.
    OpenSettings,
    /// Re-run every export setting in the document (§7). `Ctrl+Alt+E`.
    ///
    /// `Ctrl+Shift+E` is deliberately unbound; `Ctrl+E`'s `!m.shift` guard keeps it
    /// from falling through to *Flatten*.
    ExportAll,
    /// Show the top-level frames one at a time, fullscreen (`present.rs`).
    /// `Ctrl+Alt+Enter`.
    Present,
    /// *Tools ▸ Check contrast*: start or end the contrast check (`contrast.rs`). No
    /// chord.
    CheckContrast,
    /// *Tools ▸ Simulate vision*: start or end the vision simulation
    /// (`vision.rs`). No chord.
    SimulateVision,
    /// *Export as HTML / React / Vue…*: the selection's code export, one zip
    /// through a save dialog (`OndinApp::export_code`). No chord and no menu row:
    /// the command palette is its door.
    ExportCode(ondin_export::code::Target),
    /// *Export as Penpot file…* and *Export as Sketch file…*: the whole document
    /// in another tool's editable format, through a save dialog
    /// (`OndinApp::export_editable`). No chord and no menu row: the command
    /// palette is its door.
    ExportEditable(crate::panels::EditableFormat),
    /// `Ctrl+K` and `Ctrl+/` — open the command palette, or close it.
    Palette,
    /// `Ctrl+Shift+/` — the shortcut cheatsheet.
    Cheatsheet,
    Copy,
    Cut,
    Paste,
    /// `Ctrl`+`Shift`+`V`: the clipboard back at the coordinates it was copied from.
    ///
    /// Resolved from `Event::Paste` plus the frame's `Shift`, not from a key:
    /// `egui_winit`'s `is_paste_command` ignores Shift and swallows the press
    /// (`docs/shortcuts.md` §0). A separate variant because `dispatch` no longer
    /// has the frame to re-derive the modifier.
    PasteInPlace,
    /// *Paste to replace* (`OndinApp::paste_to_replace`). Unbound: the layer menu and
    /// the palette are its doors.
    PasteToReplace,
    /// A paste witnessed only by `Ctrl`+`V` coming back **up**.
    ///
    /// The weaker signal: it must stand down whenever `Event::Paste` fired, which
    /// `egui_winit` emits exactly when the clipboard yields non-empty text
    /// ([`crate::app::OndinApp::dispatch`] tests that). It exists because an
    /// image-only clipboard produces no event at all (§15 D183).
    PasteRelease,
    /// `Ctrl`+`Alt`+`C`: lift the key layer's appearance — fills, strokes, effects,
    /// opacity — into [`crate::app::OndinApp::property_clipboard`] (`docs/shortcuts.md`
    /// §7, Figma's chord).
    ///
    /// Resolved from `Event::Copy` plus the frame's `Alt`, for the reason
    /// [`Self::PasteInPlace`] reads `Shift`: `is_copy_command` ignores modifiers.
    CopyProperties,
    /// `Ctrl`+`Alt`+`V`: give the selection the appearance
    /// [`Self::CopyProperties`] lifted (`build::paste_properties`).
    ///
    /// `cmd_only` is `command && !alt`, so this chord never reaches
    /// [`Self::PasteRelease`].
    PasteProperties,
    Duplicate,
    Delete,
    SelectAll,
    /// Select every layer in the document that shares the key layer's `Same`
    /// (`OndinApp::select_same`). Unbound: the palette is its door.
    SelectSame(Same),
    /// Select the same layer in each copy of the selection's repeated blocks
    /// (`OndinApp::select_matching`). Unbound, as *Select same* is.
    SelectMatching,
    /// Select the siblings of the selection that are not in it
    /// (`OndinApp::select_inverse`). Unbound.
    SelectInverse,
    Group,
    /// *Use as mask*: make the selection a mask, or release the one it already
    /// has — `inspector::mask_action` decides which, and it is the same toggle
    /// the identity row's button and the context-menu row reach.
    Mask,
    /// *Frame selection* (`Ctrl+Alt+G`): wrap the selection in a frame
    /// at the union's corner — `build::frame`, the menu row's verb.
    FrameSelection,
    /// *Add flex layout* (`Shift+A`): a flex layout read from where the children
    /// sit, on the selected frames and groups that have none, or on a frame
    /// wrapped around a loose selection — `OndinApp::add_layout`.
    AddLayout,
    /// *Create component* (`Ctrl+Alt+K`): the selected frame or group
    /// becomes a main, or the selection is framed and the frame becomes one.
    CreateComponent,
    /// *Detach instance* (`Ctrl+Alt+B`): the instances the selection
    /// sits in become ordinary layers.
    DetachInstance,
    /// Wrap the selection in a boolean container, or switch a selected one's
    /// operation — `inspector::apply_boolean` decides which from the selection.
    Boolean(ondin_core::BoolOp),
    /// Replace the selection with the single path its outlines add up to
    /// (`build::flatten`) — destructive, and the counterpart to [`Self::Ungroup`]
    /// on a boolean.
    Flatten,
    Ungroup,
    /// Pick image files and load the cursor with them (`Ctrl+Shift+K`, and the
    /// rail's image button, which is the same action).
    PlaceImage,
    /// Open or close the insert picker (`Shift+I`, and the rail's hexagon — §5.3e).
    InsertPicker,
    /// Open or close the assets browser: the rail's button and the palette's
    /// *Assets…*, unbound.
    AssetsBrowser,
    /// *Credits…*: the document's credits card, from the Document menu and the
    /// palette.
    Credits,
    /// Open or close the styles and variables browser (`Shift+S`, and the rail's
    /// swatches button — §5.3f).
    VariablesBrowser,
    /// *Import tokens…*, *Export tokens…* and *Unlink tokens*, unbound: the
    /// variables browser's `+` menu and the palette's.
    ImportTokens,
    ExportTokens,
    UnlinkTokens,
    /// *Publish kit…* (`Ctrl+Alt+P` — §5.3e).
    PublishKit,
    /// The Document menu's rows, unbound: *Make this a kit*, *Kits used…*, *Stop
    /// sharing as kit…* and *Show in library*.
    MakeKit,
    KitsUsed,
    StopSharingKit,
    ShowInLibrary,
    /// *Version history…*, the Document menu's first row.
    VersionHistory,
    /// *Compare with version…*: the version history card with its compare
    /// buttons lit (`crate::compare`).
    CompareVersion,
    /// *Save named version…*: save, pin a version and ask for its name.
    SaveNamedVersion,
    /// *Colour review…*, unbound: the card that merges near-identical colours
    /// (`crate::colour_review`).
    ColourReview,
    /// *Clean up…*, unbound: the box that takes away what draws nothing
    /// (`crate::clean_up_box`).
    CleanUp,
    /// *Fill with content…*, unbound: the box that fills text layers with
    /// realistic values (`crate::fill_box`).
    FillContent,
    /// A quarter turn of the selection, each layer about its own origin; `true` is
    /// clockwise. Unbound, as the Transform card's buttons are.
    RotateQuarter(bool),
    /// *Transform again* (`crate::again`). Unbound: Ctrl+Alt+D is AltGr+D, which
    /// types a letter on several European layouts.
    TransformAgain,
    /// Open *Repeat around origin…* (`crate::repeat`). Unbound.
    RepeatAround,
    /// Move the selection by a world-space delta (arrow keys).
    Nudge(Vec2),
    /// Grow or shrink the selection's box by this many points on each axis —
    /// `Ctrl`+arrows, with `Shift` for the coarse step ([`OndinApp::size_step`]).
    ///
    /// Same four arrows and signs as [`Self::Nudge`]: the box is held by its
    /// top-left corner, so `→` widens, `←` narrows, `↓` heightens, `↑` shortens.
    /// One axis at a time as resolved, but a `Vec2` because a box takes both deltas
    /// in one edit.
    ///
    /// Uses the same two distances as the bare arrows, from the same [`NudgeStep`].
    ///
    /// [`OndinApp::size_step`]: crate::app::OndinApp
    SizeStep(Vec2),
    /// Step the point selection one anchor along the path it belongs to — `Tab`
    /// forwards, `Shift`+`Tab` back (`OndinApp::step_point`).
    ///
    /// Only the node tool uses it; `dispatch` drops it otherwise, leaving `Tab` to
    /// egui's focus ring. In a live text session `Tab` nests list items, but
    /// [`resolve`] returns nothing in [`Mode::TextInsert`], so the canvas editor
    /// reads it directly and the two claimants never see the same key.
    StepPoint(bool),
    /// Restack the selection within its parent.
    Restack(build::ZMove),
    /// Traverse a path's subpaths the other way round —
    /// `OndinApp::reverse_subpaths`, on `Shift+D` for *direction*.
    ///
    /// Non-zero filling makes it necessary: a subpath's winding decides solid or
    /// hole, and nothing on screen says which was drawn.
    ReverseSubpaths,
    ChooseTool(Tool),
    ZoomIn,
    ZoomOut,
    ZoomReset,
    /// Fit **everything** in the view — `Ctrl+1`, `Shift+1`.
    ///
    /// Everything, unconditionally, even with a selection; [`Self::ZoomSelection`]
    /// is the other half (`docs/shortcuts.md` §3).
    ZoomFit,
    /// Fit the **selection** in the view — `Shift+2` (Figma), `Ctrl+2` (Sketch).
    ///
    /// A no-op with nothing selected, so the two bindings stay distinct actions.
    ZoomSelection,
    /// `Ctrl+PageDown` (`true`) / `Ctrl+PageUp` — the next or previous page
    /// (`crate::pages`), stopping at either end. Plain `PageDown`/`PageUp` stay
    /// present mode's.
    StepPage(bool),
    /// Flip one workspace switch — the same set the View and Snap menus show,
    /// so a row and its chord cannot come to mean different things
    /// (`docs/shortcuts.md` §6).
    ToggleView(ViewSwitch),
    /// Leave text editing / cancel the current gesture.
    Escape,
    /// Step **into** the selection, or back out of what you stepped into —
    /// today, point editing on a path (`OndinApp::enter_action`).
    ///
    /// Not an `Escape` alias: Escape unwinds one rung per press, Enter toggles in
    /// and out. The pen also reads Enter directly (it finishes the path), so
    /// `dispatch` hands this to the pen when one is in flight.
    Enter,
    /// Hide or show the selection — `Ctrl+Shift+H`, which Figma and Sketch
    /// agree on.
    ///
    /// One switch for the whole selection; a mixed selection **hides**, since
    /// *Hide* is a verb (`OndinApp::set_switch` carries the argument).
    ToggleHidden,
    /// Lock or unlock the selection — `Ctrl+Shift+L`, Figma's and Sketch's.
    /// Mixed **locks**, for [`Self::ToggleHidden`]'s reason.
    ToggleLocked,
    /// Open the rename field on the selected layer — `Ctrl+R` (Figma) and `F2`
    /// (the Windows convention). The same field a double-click in the layers
    /// panel opens, so there is one rename rather than a second one.
    ///
    /// `Ctrl+R` is rename here, not rulers: rulers keep Figma's `Shift+R`
    /// (`docs/shortcuts.md` §4 and its *Unresolvable* list). With two or more
    /// layers selected it opens [`Self::BatchRename`] rather than refusing.
    Rename,
    /// Open the batch rename box — `Ctrl+Shift+R`. Live with any selection or
    /// none.
    BatchRename,
    /// Open find and replace on Find — `Ctrl+F`; with `replace`, its Replace row
    /// showing — `Ctrl+H`.
    FindText {
        replace: bool,
    },
    /// Mirror the selection across the centreline of the box it occupies —
    /// `Shift+H` and `Shift+V`, Figma's.
    ///
    /// Clear of the tool letters because the tool block is gated on Shift being
    /// *up*.
    Flip(build::Axis),
    /// Align the selection to one edge of the box `OndinApp::align_target`
    /// picks — `Alt`+`A`/`D`/`W`/`S`/`H`/`V`, Figma exact.
    ///
    /// The block is Alt-*without*-Ctrl, so `Alt+S` is not `Ctrl+Alt+S` (subtract).
    Align(build::Axis, build::Edge),
    /// Space the selection evenly along one axis — `Alt+Shift+H` and
    /// `Alt+Shift+V`.
    ///
    /// Figma's `Ctrl+Alt+H`/`V` would collide with paste-properties and the
    /// boolean namespace (`docs/shortcuts.md` §5).
    Distribute(build::Axis),
    /// Lay the selection out as the grid it is roughly in — `Alt+Shift+T`.
    TidyUp,
    /// A plain digit, which sets the selection's opacity (`docs/shortcuts.md` §2).
    ///
    /// The digit, not the percentage: two digits within 600 ms mean one value, but
    /// that state lives in the app (`OndinApp::opacity_digit`) so [`resolve`]
    /// stays a pure function of one frame's input. `Key::Num0`–`Num9` cover the
    /// main row and numpad.
    OpacityDigit(u8),
    /// `Shift`+`3`…`9`, which sets the selection's blend mode
    /// (`BlendMode::of_chord`, `docs/shortcuts.md` §2).
    BlendChord(u8),
    /// One of the styling chords a live text session admits — see [`TextChord`].
    TextStyle(TextChord),
}

/// A workspace switch: something the View or Snap menu shows a tick against
/// (all but [`Self::Lasso`], which `Q` and the palette carry), and something
/// `docs/shortcuts.md` §6 gives a chord.
///
/// One enum rather than a label per call site: a menu row renders its label
/// *from* the variant and a chord names the variant directly, so neither drifts.
/// The View menu's *Panels ▸* list alone shortens the label.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewSwitch {
    /// `Shift+R` — Figma's, and the one switch here that was already bound.
    Rulers,
    /// `Ctrl+;` — Illustrator's.
    Guides,
    /// `Ctrl+Alt+;` — Illustrator's and Photoshop's.
    ///
    /// A **workspace** switch, not a per-guide property, so one chord toggles both
    /// ways (`OndinApp::set_guides_locked`).
    GuideLock,
    /// `Ctrl+'` — Illustrator's and Photoshop's.
    Grid,
    /// The per-frame layout grids, all of them at once — *View ▸ Show layout
    /// grid*.
    ///
    /// Separate from [`Self::Grid`]: that is the pixel grid drawn above 8×
    /// (`grid::VISIBLE_AT_ZOOM`), this is the column and row bands a frame carries
    /// (`ondin_core::layout::LayoutGrid`). A grid also draws only while its own eye
    /// is on and on a frame `shown_visible` admits; this sits above both.
    ///
    /// Deliberately unbound, on [`Self::SnapBaselines`]'s reasoning: no convention
    /// to match (`docs/shortcuts.md` §6, "none, on purpose").
    LayoutGrid,
    /// Every outline as a hairline, nothing filled, clipped or masked
    /// (`RenderOverrides::set_outline`). Deliberately unbound: Figma's and
    /// Illustrator's `Ctrl+Y` is redo here, and `Ctrl+Shift+Y` stays unspent
    /// beside it (`ctrl_shift_is_not_a_second_spelling_of_ctrl`).
    Outline,
    /// `Ctrl+Alt+Y` — Illustrator's. The canvas drawn at one pixel per unit and
    /// enlarged with hard pixel edges, so the anti-aliasing a viewer at 1x gets is
    /// visible (`canvas::pixel_preview`).
    PixelPreview,
    /// `Ctrl+Alt+\`.
    Layers,
    /// The History section at the foot of the layers pane (`panels::history`).
    /// Off by default and deliberately unbound: a list few people keep open.
    History,
    /// `Ctrl+Shift+M` — the minimap card at the canvas's bottom left
    /// (`minimap.rs`). Off by default.
    Minimap,
    /// `Ctrl+Shift+\`, which reaches egui as `Key::Pipe` (§0).
    Toolbar,
    /// The floating status bar at the canvas's bottom left, on by default.
    /// Deliberately unbound, on [`Self::SnapBaselines`]'s reasoning.
    StatusMessages,
    /// `Ctrl+\` — Figma's show/hide-UI chord.
    ///
    /// Mapped onto focus mode, which already hides all chrome and restores it on
    /// leaving. Escape still leaves it.
    Focus,
    /// `Ctrl+U` — Illustrator's Smart Guides. Clear of `Ctrl+Alt+U` (union); it
    /// means underline in `Mode::TextInsert`, and only the mode tells them apart.
    SnapShapes,
    /// `Ctrl+Shift+;`, which reaches egui as `Key::Colon` (§0).
    SnapGuides,
    /// `Ctrl+Shift+'` — Illustrator's `Ctrl+"` is literally this chord.
    SnapGrid,
    /// Text baselines. Deliberately unbound: no app has this, so there is no
    /// convention to match; the three above are Illustrator's.
    SnapBaselines,
    /// `Q` — the marquee draws a loop instead of a box, for Select and the Node
    /// tool alike (`crate::lasso`). Q is free: no tool uses it.
    Lasso,
    /// `Ctrl+Shift+N`, a browser's private window — every drawn name masked
    /// (`crate::private`). A preference, not a workspace switch, so it survives a
    /// restart.
    Private,
}

impl ViewSwitch {
    /// The menu row's text, which the menus label themselves from; the View menu's
    /// *Panels ▸* list drops the panes' "Show".
    pub fn label(self) -> &'static str {
        match self {
            Self::Rulers => "Show rulers",
            Self::Guides => "Show guides",
            Self::GuideLock => "Lock guides",
            Self::Grid => "Show grid",
            Self::LayoutGrid => "Show layout grid",
            Self::Outline => "Outline mode",
            Self::PixelPreview => "Pixel preview",
            Self::Layers => "Show layers",
            Self::History => "Show history",
            Self::Minimap => "Show minimap",
            Self::Toolbar => "Show toolbar",
            Self::StatusMessages => "Show messages",
            Self::Focus => "Focus mode",
            Self::SnapShapes => "Snap to shapes",
            Self::SnapGuides => "Snap to guides",
            Self::SnapGrid => "Snap to grid",
            Self::SnapBaselines => "Snap to baselines",
            Self::Lasso => "Lasso select",
            Self::Private => "Private mode",
        }
    }

    /// The View menu's first group, what the canvas draws, in the order Illustrator's
    /// View menu reads it: rulers, the guides pulled out of them, then the grid
    /// beside the layout grid so the two read against each other.
    pub const VIEW_CANVAS: [Self; 5] = [
        Self::Rulers,
        Self::Guides,
        Self::GuideLock,
        Self::Grid,
        Self::LayoutGrid,
    ];

    /// The View menu's modes, which *Present mode* follows as a verb.
    pub const VIEW_MODES: [Self; 4] = [
        Self::Outline,
        Self::PixelPreview,
        Self::Private,
        Self::Focus,
    ];

    /// The panes behind the View menu's *Panels ▸* row.
    pub const VIEW_PANELS: [Self; 5] = [
        Self::Layers,
        Self::History,
        Self::Minimap,
        Self::Toolbar,
        Self::StatusMessages,
    ];

    /// Every variant, for the tests that have to walk them.
    ///
    /// Written by hand, so it cannot police the enum on its own; what makes it
    /// answerable is that `Self::label` is an exhaustive `match` in this same
    /// `impl`, so a new variant fails to compile beside it.
    /// `every_view_switch_is_on_exactly_one_menu` checks membership, not count.
    ///
    /// `#[cfg(test)]` because the tests are its only readers; an `#[allow]` would
    /// keep every item it names alive.
    #[cfg(test)]
    pub const ALL: [Self; 19] = [
        Self::Rulers,
        Self::Guides,
        Self::GuideLock,
        Self::Grid,
        Self::LayoutGrid,
        Self::Outline,
        Self::PixelPreview,
        Self::Layers,
        Self::History,
        Self::Minimap,
        Self::Toolbar,
        Self::StatusMessages,
        Self::Focus,
        Self::SnapShapes,
        Self::SnapGuides,
        Self::SnapGrid,
        Self::SnapBaselines,
        Self::Lasso,
        Self::Private,
    ];

    /// The Snap menu's rows.
    ///
    /// Baselines last: the only row with no accelerator and not in Illustrator.
    pub const SNAP_MENU: [Self; 4] = [
        Self::SnapShapes,
        Self::SnapGuides,
        Self::SnapGrid,
        Self::SnapBaselines,
    ];
}

/// The modifiers a [`Binding`] holds; it answers only with exactly these down.
///
/// `ctrl` is egui's `command`, so ⌘ on a Mac. The physical Control key is never
/// read here: the macOS chords on it are host-dependent and stay code.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Mods {
    ctrl: bool,
    alt: bool,
    shift: bool,
}

impl Mods {
    const NONE: Self = Self::of(false, false, false);
    const SHIFT: Self = Self::of(false, false, true);
    const ALT: Self = Self::of(false, true, false);
    const ALT_SHIFT: Self = Self::of(false, true, true);
    const CTRL: Self = Self::of(true, false, false);
    const CTRL_SHIFT: Self = Self::of(true, false, true);
    const CTRL_ALT: Self = Self::of(true, true, false);

    const fn of(ctrl: bool, alt: bool, shift: bool) -> Self {
        Self { ctrl, alt, shift }
    }

    fn held(self, m: egui::Modifiers) -> bool {
        m.command == self.ctrl && m.alt == self.alt && m.shift == self.shift
    }
}

/// The modes a [`Binding`] is read in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Scope {
    Normal,
    /// A live text session only, where a chord styles the text.
    Text,
    Both,
}

impl Scope {
    fn admits(self, mode: Mode) -> bool {
        match self {
            Self::Both => true,
            Self::Normal => mode == Mode::Normal,
            Self::Text => mode == Mode::TextInsert,
        }
    }
}

/// One plain chord: `key` pressed with exactly `mods` held means `action`.
#[derive(Clone, Copy, Debug)]
struct Binding {
    mods: Mods,
    key: egui::Key,
    action: Action,
    scope: Scope,
    /// Answered but never spelled: no label shows this chord.
    unshown: bool,
}

/// A normal-mode [`Binding`].
const fn bind(mods: Mods, key: egui::Key, action: Action) -> Binding {
    Binding {
        mods,
        key,
        action,
        scope: Scope::Normal,
        unshown: false,
    }
}

impl Binding {
    const fn in_scope(self, scope: Scope) -> Self {
        Self { scope, ..self }
    }

    const fn unshown(self) -> Self {
        Self {
            unshown: true,
            ..self
        }
    }

    /// The chord as every label spells it, `Ctrl`, `Alt`, `Shift` in that order
    /// before the key (`Ctrl+Alt+K`); [`keys`] rewrites it for a Mac.
    fn spelled(&self) -> String {
        let mut out = String::new();
        for (held, name) in [
            (self.mods.ctrl, "Ctrl"),
            (self.mods.alt, "Alt"),
            (self.mods.shift, "Shift"),
        ] {
            if held {
                out.push_str(name);
                out.push('+');
            }
        }
        out.push_str(match self.key {
            egui::Key::ArrowLeft => "←",
            egui::Key::ArrowRight => "→",
            egui::Key::ArrowUp => "↑",
            egui::Key::ArrowDown => "↓",
            // egui's own symbol is U+2212, which no label spells.
            egui::Key::Minus => "-",
            egui::Key::Quote => "'",
            egui::Key::Delete => "Del",
            key => key.symbol_or_name(),
        });
        out
    }
}

/// Every chord that is a key plus exact modifiers, and what it does: [`resolve`]
/// reads it, and the menus, palette and cheatsheet spell from it ([`chords`]).
///
/// An action's first shown chord is its menu accelerator, its second the
/// cheatsheet's "or". Chords that need more than a key and exact modifiers are
/// read in code after the table, each family with its reason.
const BINDINGS: &[Binding] = {
    use Action as A;
    use egui::Key as K;
    use ondin_core::BoolOp;
    const NONE: Mods = Mods::NONE;
    const SHIFT: Mods = Mods::SHIFT;
    const ALT: Mods = Mods::ALT;
    const ALT_SHIFT: Mods = Mods::ALT_SHIFT;
    const CTRL: Mods = Mods::CTRL;
    const CTRL_SHIFT: Mods = Mods::CTRL_SHIFT;
    const CTRL_ALT: Mods = Mods::CTRL_ALT;
    const fn both(b: Binding) -> Binding {
        b.in_scope(Scope::Both)
    }
    const fn text(mods: Mods, key: K, chord: TextChord) -> Binding {
        bind(mods, key, A::TextStyle(chord)).in_scope(Scope::Text)
    }
    &[
        // File and history; the four a text session also admits finish it first.
        both(bind(CTRL, K::Z, A::Undo)),
        both(bind(CTRL_SHIFT, K::Z, A::Redo)),
        both(bind(CTRL, K::Y, A::Redo)),
        both(bind(CTRL, K::S, A::Save)),
        both(bind(CTRL, K::O, A::Open)),
        bind(CTRL, K::N, A::NewDocument),
        bind(CTRL, K::W, A::CloseDocument),
        bind(CTRL, K::Comma, A::OpenSettings),
        both(bind(CTRL, K::K, A::Palette)),
        both(bind(CTRL, K::Slash, A::Palette)),
        bind(CTRL_ALT, K::E, A::ExportAll),
        bind(CTRL_ALT, K::Enter, A::Present),
        bind(CTRL, K::PageDown, A::StepPage(true)),
        bind(CTRL, K::PageUp, A::StepPage(false)),
        // Structure. The fourth boolean, `Ctrl+Alt+X`, is read off its release.
        bind(CTRL, K::D, A::Duplicate),
        bind(CTRL, K::A, A::SelectAll),
        bind(CTRL, K::G, A::Group),
        bind(CTRL_SHIFT, K::G, A::Ungroup),
        bind(CTRL_SHIFT, K::K, A::PlaceImage),
        bind(CTRL_ALT, K::M, A::Mask),
        bind(CTRL_ALT, K::K, A::CreateComponent),
        bind(CTRL_ALT, K::B, A::DetachInstance),
        bind(CTRL_ALT, K::G, A::FrameSelection),
        bind(CTRL, K::E, A::Flatten),
        bind(CTRL_ALT, K::U, A::Boolean(BoolOp::Union)),
        bind(CTRL_ALT, K::S, A::Boolean(BoolOp::Subtract)),
        bind(CTRL_ALT, K::I, A::Boolean(BoolOp::Intersect)),
        // Z-order, on the bracket convention.
        bind(CTRL_SHIFT, K::CloseBracket, A::Restack(build::ZMove::Front)),
        bind(CTRL, K::CloseBracket, A::Restack(build::ZMove::Forward)),
        bind(CTRL, K::OpenBracket, A::Restack(build::ZMove::Backward)),
        bind(CTRL_SHIFT, K::OpenBracket, A::Restack(build::ZMove::Back)),
        // Zoom.
        bind(CTRL, K::Equals, A::ZoomIn),
        bind(CTRL, K::Minus, A::ZoomOut),
        bind(CTRL, K::Num0, A::ZoomReset),
        bind(SHIFT, K::Num0, A::ZoomReset),
        bind(CTRL, K::Num1, A::ZoomFit),
        bind(SHIFT, K::Num1, A::ZoomFit),
        bind(SHIFT, K::Num2, A::ZoomSelection),
        bind(CTRL, K::Num2, A::ZoomSelection),
        // The workspace switches.
        bind(SHIFT, K::R, A::ToggleView(ViewSwitch::Rulers)),
        bind(CTRL, K::Semicolon, A::ToggleView(ViewSwitch::Guides)),
        bind(CTRL_ALT, K::Semicolon, A::ToggleView(ViewSwitch::GuideLock)),
        bind(CTRL_ALT, K::Y, A::ToggleView(ViewSwitch::PixelPreview)),
        bind(CTRL, K::Quote, A::ToggleView(ViewSwitch::Grid)),
        bind(CTRL_SHIFT, K::Quote, A::ToggleView(ViewSwitch::SnapGrid)),
        bind(CTRL, K::U, A::ToggleView(ViewSwitch::SnapShapes)),
        bind(
            CTRL_SHIFT,
            K::Semicolon,
            A::ToggleView(ViewSwitch::SnapGuides),
        ),
        bind(CTRL, K::Backslash, A::ToggleView(ViewSwitch::Focus)),
        bind(CTRL_ALT, K::Backslash, A::ToggleView(ViewSwitch::Layers)),
        bind(CTRL_SHIFT, K::Backslash, A::ToggleView(ViewSwitch::Toolbar)),
        bind(CTRL_SHIFT, K::M, A::ToggleView(ViewSwitch::Minimap)),
        bind(CTRL_SHIFT, K::N, A::ToggleView(ViewSwitch::Private)),
        bind(NONE, K::Q, A::ToggleView(ViewSwitch::Lasso)),
        // Paths, layer state and the selection's verbs.
        bind(SHIFT, K::D, A::ReverseSubpaths),
        bind(CTRL_SHIFT, K::H, A::ToggleHidden),
        bind(CTRL_SHIFT, K::L, A::ToggleLocked),
        bind(CTRL, K::R, A::Rename),
        bind(CTRL_SHIFT, K::R, A::BatchRename),
        bind(CTRL, K::F, A::FindText { replace: false }),
        bind(SHIFT, K::H, A::Flip(build::Axis::X)),
        bind(SHIFT, K::V, A::Flip(build::Axis::Y)),
        bind(SHIFT, K::I, A::InsertPicker),
        bind(SHIFT, K::S, A::VariablesBrowser),
        bind(SHIFT, K::A, A::AddLayout),
        bind(CTRL_ALT, K::P, A::PublishKit),
        // Align and distribute: Alt without Ctrl.
        bind(ALT, K::A, A::Align(build::Axis::X, build::Edge::Min)),
        bind(ALT, K::D, A::Align(build::Axis::X, build::Edge::Max)),
        bind(ALT, K::W, A::Align(build::Axis::Y, build::Edge::Min)),
        bind(ALT, K::S, A::Align(build::Axis::Y, build::Edge::Max)),
        bind(ALT, K::H, A::Align(build::Axis::X, build::Edge::Mid)),
        bind(ALT, K::V, A::Align(build::Axis::Y, build::Edge::Mid)),
        bind(ALT_SHIFT, K::H, A::Distribute(build::Axis::X)),
        bind(ALT_SHIFT, K::V, A::Distribute(build::Axis::Y)),
        bind(ALT_SHIFT, K::T, A::TidyUp),
        // Enter steps in; Tab steps the point selection, which only the node tool
        // takes (`dispatch` leaves Tab to egui's focus ring otherwise).
        bind(NONE, K::Enter, A::Enter),
        bind(NONE, K::Tab, A::StepPoint(true)),
        bind(SHIFT, K::Tab, A::StepPoint(false)),
        // The single-key tools. `C` is unbound: image editing's door is `Enter`.
        bind(NONE, K::V, A::ChooseTool(Tool::Select)),
        bind(NONE, K::H, A::ChooseTool(Tool::Hand)),
        bind(NONE, K::K, A::ChooseTool(Tool::Scale)),
        bind(NONE, K::F, A::ChooseTool(Tool::Frame)),
        bind(SHIFT, K::F, A::ChooseTool(Tool::Slice)),
        bind(NONE, K::R, A::ChooseTool(Tool::Rect)),
        bind(NONE, K::O, A::ChooseTool(Tool::Ellipse)),
        // `O` is the ellipse's published letter; `E` answers too, unshown.
        bind(NONE, K::E, A::ChooseTool(Tool::Ellipse)).unshown(),
        bind(NONE, K::G, A::ChooseTool(Tool::Polygon)),
        bind(NONE, K::S, A::ChooseTool(Tool::Star)),
        bind(NONE, K::L, A::ChooseTool(Tool::Line)),
        // The spiral shares the line's slot as the pencil shares the pen's.
        bind(SHIFT, K::L, A::ChooseTool(Tool::Spiral)),
        bind(SHIFT, K::M, A::ChooseTool(Tool::ShapeBuilder)),
        bind(NONE, K::P, A::ChooseTool(Tool::Pen)),
        bind(SHIFT, K::P, A::ChooseTool(Tool::Pencil)),
        bind(NONE, K::A, A::ChooseTool(Tool::Node)),
        bind(NONE, K::T, A::ChooseTool(Tool::Text)),
        bind(NONE, K::I, A::ChooseTool(Tool::Eyedropper)),
        // A live text session's styling chords (`docs/shortcuts.md` §10).
        text(CTRL, K::B, TextChord::Bold),
        text(CTRL, K::I, TextChord::Italic),
        text(CTRL, K::U, TextChord::Underline),
        text(CTRL_SHIFT, K::Period, TextChord::Size(1)),
        text(CTRL_SHIFT, K::Comma, TextChord::Size(-1)),
        text(
            CTRL_SHIFT,
            K::L,
            TextChord::Align(ondin_core::TextAlign::Start),
        ),
        text(
            CTRL_SHIFT,
            K::R,
            TextChord::Align(ondin_core::TextAlign::End),
        ),
        text(
            CTRL_SHIFT,
            K::J,
            TextChord::Align(ondin_core::TextAlign::Justify),
        ),
    ]
};

/// Every binding's spelling, in [`BINDINGS`] order, built once so a label can
/// hold it as `&'static str`.
fn spellings() -> &'static [String] {
    static SPELLED: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    SPELLED.get_or_init(|| BINDINGS.iter().map(Binding::spelled).collect())
}

/// Every shown table chord for `action`, spelled, in table order.
pub(crate) fn chords(action: Action) -> impl Iterator<Item = &'static str> {
    BINDINGS
        .iter()
        .zip(spellings())
        .filter(move |(b, _)| !b.unshown && b.action == action)
        .map(|(_, s)| s.as_str())
}

/// The chord a menu row shows for `action`: its first in [`BINDINGS`].
pub(crate) fn accel(action: Action) -> Option<&'static str> {
    chords(action).next()
}

/// Push the action of every [`BINDINGS`] chord this frame pressed in `mode`.
fn table_chords(i: &egui::InputState, mode: Mode, out: &mut Vec<Action>) {
    out.extend(
        BINDINGS
            .iter()
            .filter(|b| b.scope.admits(mode) && b.mods.held(i.modifiers) && i.key_pressed(b.key))
            .map(|b| b.action),
    );
}

/// The modifiers a chord can be spelled with, as `docs/shortcuts.md` writes them,
/// in the order a Mac menu sets them out (⌃ ⌥ ⇧ ⌘; [`key_name`] gives each
/// symbol). `Ctrl` is egui's `command`, so ⌘ on a Mac; `Control` is the physical
/// key, the spelling only a macOS-only binding uses.
const MODIFIERS: [&str; 4] = ["Control", "Alt", "Shift", "Ctrl"];

/// Every shortcut in `text` as this host writes it: as spelled on Windows and
/// Linux, and in the symbols a Mac menu uses on macOS (`Ctrl+Shift+Z` is `⇧⌘Z`).
/// Every label that shows a chord, bare or inside a sentence, goes through this.
pub fn keys(text: &str) -> std::borrow::Cow<'_, str> {
    keys_for(text, on_mac())
}

/// Whether the keymap is a Mac's: the few chords macOS keeps for itself are
/// bound elsewhere, and labels use its symbols.
pub(crate) fn on_mac() -> bool {
    crate::chrome::Host::current() == crate::chrome::Host::MacOs
}

/// *Replace text…*'s chord: `Ctrl+H`, but `Ctrl+Alt+F` (⌥⌘F) on macOS, whose
/// app menu keeps ⌘H for *Hide*.
pub(crate) fn replace_chord() -> &'static str {
    if on_mac() { "Ctrl+Alt+F" } else { "Ctrl+H" }
}

/// The next and previous tab chords: `Ctrl+Tab`, but `Control+Tab` (⌃⇥) on
/// macOS, where ⌘Tab is the app switcher (`tabs::read_keys`).
pub(crate) fn tab_step_chords() -> [&'static str; 2] {
    if on_mac() {
        ["Control+Tab", "Control+Shift+Tab"]
    } else {
        ["Ctrl+Tab", "Ctrl+Shift+Tab"]
    }
}

/// The cheatsheet's chord: `Ctrl+Shift+/`, but `Control+Shift+/` (⌃⇧/) on macOS,
/// where ⇧⌘/ is the system's *Show Help menu* and never reaches the app.
pub(crate) fn cheatsheet_chord_spelled() -> &'static str {
    if on_mac() {
        "Control+Shift+/"
    } else {
        "Ctrl+Shift+/"
    }
}

/// The modifiers [`cheatsheet_chord_spelled`] holds, as egui reports them.
pub(crate) fn cheatsheet_modifiers() -> egui::Modifiers {
    if on_mac() {
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT
    } else {
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT
    }
}

/// [`keys`] for a host chosen by the caller, so both spellings are testable on any.
pub(crate) fn keys_for(text: &str, mac: bool) -> std::borrow::Cow<'_, str> {
    if !text.contains('+') {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    let mut prev: Option<char> = None;
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        // A name the user gave sits in curly quotes (`Deleted “{name}”`) and is
        // theirs, chord-like or not: copied through to its closing quote.
        if c == '“' {
            let quoted = rest
                .find('”')
                .map_or(rest.len(), |end| end + '”'.len_utf8());
            out.push_str(&rest[..quoted]);
            prev = rest[..quoted].chars().last();
            rest = &rest[quoted..];
            continue;
        }
        // A chord starts a word: `Ctrl` inside `MyCtrl+X` is not one.
        if prev.is_none_or(|p| !p.is_alphanumeric())
            && let Some((chord, len)) = chord_at(rest, mac)
        {
            changed |= chord != rest[..len];
            out.push_str(&chord);
            prev = rest[..len].chars().last();
            rest = &rest[len..];
            continue;
        }
        out.push(c);
        prev = Some(c);
        rest = &rest[c.len_utf8()..];
    }
    if changed {
        std::borrow::Cow::Owned(out)
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

/// The chord `s` starts with, written for the host, and its length in `s`; `None`
/// when `s` does not start with a modifier and `+`. The key is a run of letters
/// and digits (`Z`, `F2`, `Enter`, `drag`) or one other character (`;`, `=`, `+`).
fn chord_at(s: &str, mac: bool) -> Option<(String, usize)> {
    let mut held = [false; MODIFIERS.len()];
    let mut at = 0;
    while let Some(i) = MODIFIERS
        .iter()
        .position(|name| s[at..].starts_with(name) && s[at + name.len()..].starts_with('+'))
    {
        held[i] = true;
        at += MODIFIERS[i].len() + 1;
    }
    if at == 0 {
        return None;
    }
    let first = s[at..].chars().next().filter(|c| !c.is_whitespace())?;
    let key_len = if first.is_alphanumeric() {
        s[at..]
            .char_indices()
            .find(|&(_, c)| !c.is_alphanumeric())
            .map_or(s.len() - at, |(j, _)| j)
    } else {
        first.len_utf8()
    };
    let key = &s[at..at + key_len];
    let len = at + key_len;
    if !mac {
        return Some((s[..len].replace("Control+", "Ctrl+"), len));
    }
    let mut chord: String = MODIFIERS
        .iter()
        .zip(held)
        .filter(|&(_, on)| on)
        .map(|(name, _)| key_name_for(name, true))
        .collect();
    // `Alt+drag`, `Shift+click`: a gesture, not a key, reads apart.
    if first.is_lowercase() {
        chord.push(' ');
    }
    chord.push_str(key_name_for(key, true));
    Some((chord, len))
}

/// One key or modifier's name as this host writes it, for a keycap of its own:
/// `Ctrl` is `⌘` on macOS, `Enter` is `↩`, and `Control` is `Ctrl` elsewhere.
pub fn key_name(name: &str) -> &str {
    key_name_for(name, on_mac())
}

/// [`key_name`] for a host chosen by the caller.
fn key_name_for(name: &str, mac: bool) -> &str {
    if !mac {
        return if name == "Control" { "Ctrl" } else { name };
    }
    match name {
        "Control" => "⌃",
        "Alt" => "⌥",
        "Shift" => "⇧",
        "Ctrl" => "⌘",
        "Enter" => "↩",
        "Esc" | "Escape" => "⎋",
        "Tab" => "⇥",
        "Backspace" => "⌫",
        other => other,
    }
}

/// How far an arrow key moves the selection, and how far with Shift held —
/// the defaults, and what [`NudgeStep::default`] hands out.
const NUDGE: f64 = 1.0;
const NUDGE_LARGE: f64 = 10.0;

/// The two distances an arrow key can move the selection: bare, and with Shift.
///
/// A struct rather than two `f64` parameters, because the same-typed swap
/// compiles and no gate would see it (the settings modal writes it,
/// [`crate::prefs::Prefs`] persists it, [`resolve`] reads it). Serialized into
/// `prefs.json` with named fields.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct NudgeStep {
    /// A bare arrow key.
    pub small: f64,
    /// An arrow key with Shift held.
    pub large: f64,
}

impl Default for NudgeStep {
    fn default() -> Self {
        Self {
            small: NUDGE,
            large: NUDGE_LARGE,
        }
    }
}

impl NudgeStep {
    /// The narrowest and widest step the Settings field offers.
    ///
    /// Held here rather than in `settings.rs` because the reader enforces it —
    /// see [`Self::step`].
    pub const MIN: f64 = 1.0;
    pub const MAX: f64 = 1000.0;

    /// The step an arrow press means, given whether Shift was down.
    ///
    /// Clamped at use, because `prefs.json` is hand-editable untrusted input and
    /// an older build stored sub-pixel steps that the Settings draft clamps but the
    /// stored value does not. Deliberately not clamped in `Prefs::load`: stored
    /// values are not rewritten on the way in (§5.3).
    fn step(self, shift: bool) -> f64 {
        let raw = if shift { self.large } else { self.small };
        if raw.is_finite() {
            raw.round().clamp(Self::MIN, Self::MAX)
        } else {
            NUDGE
        }
    }
}

/// Resolve this frame's keyboard input into actions for `mode`.
///
/// Returns nothing at all when a text entry owns the keyboard: text typed into
/// the inspector is text, never a command. Any other focused control leaves undo,
/// redo and save.
///
/// The focus read here is this frame's, not last frame's: egui updates focus in
/// `begin_pass`, so a key that surrenders focus is seen with focus already gone
/// and its own press still in `input` — which is how `Escape` in a focused field
/// reaches [`OndinApp::escape`] and cancels the gesture (§15 D317).
///
/// `nudge` is the only user-changeable part of the keymap ([`NudgeStep`]); it is
/// a parameter rather than a `Prefs` read so this stays a pure resolution of
/// this frame's events.
///
/// [`OndinApp::escape`]: crate::app::OndinApp
pub fn resolve(ctx: &egui::Context, mode: Mode, nudge: NudgeStep) -> Vec<Action> {
    if ctx.egui_wants_keyboard_input() {
        // A focused control that is no text entry (a button, a layers row) keeps
        // its keys, but undo, redo and save still reach the document: `Ctrl+Z`
        // must answer the `Delete` a focused row just took.
        let text = ctx
            .memory(|m| m.focused())
            .is_none_or(|id| egui::TextEdit::load_state(ctx, id).is_some());
        if text {
            return Vec::new();
        }
        let mut out = normal_mode(ctx, nudge);
        out.retain(|a| matches!(a, Action::Undo | Action::Redo | Action::Save));
        return out;
    }
    match mode {
        // Insert mode is near exclusive: the canvas text editor owns every bare
        // key, and only modified chords are resolved here (`docs/shortcuts.md`
        // §10). `Tab` nests a list and is read in the editor's loop, which keeps
        // it clear of `StepPoint`.
        Mode::TextInsert => text_insert_mode(ctx),
        Mode::Normal => normal_mode(ctx, nudge),
    }
}

/// A styling chord read **inside a live text session** (`docs/shortcuts.md` §10).
///
/// These reach the document, hence `Action`s: bare keys stay exclusive to the
/// canvas editor, modified chords come through. `Tab` stays in the editor's own
/// loop because it changes the paragraph, not the styling.
///
/// Three keys mean something else in `Mode::Normal` (`Ctrl+U` snap-to-shapes,
/// `Ctrl+Shift+L` lock, `Alt`+arrows nudge); only the mode tells them apart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextChord {
    /// `Ctrl+B` — the family's bold face on or off (`text::bold_step`).
    Bold,
    /// `Ctrl+I` — the family's italic on or off (`text::italic_step`).
    Italic,
    /// `Ctrl+U` — underline on or off.
    ///
    /// Exclusive with strikethrough: the model carries two attributes but the UI
    /// offers one decoration, so this *replaces* a strikethrough.
    Underline,
    /// `Ctrl+Shift+.` and `Ctrl+Shift+,` — font size up and down.
    ///
    /// One spelling each: `>` and `<` have no `egui::Key` variant, so the
    /// logical key falls through to the physical `Period`/`Comma` (§0).
    Size(i8),
    /// `Alt+→` and `Alt+←` — letter spacing.
    Tracking(i8),
    /// `Alt+↑` and `Alt+↓` — line height.
    Leading(i8),
    /// `Ctrl+Shift+L`/`C`/`R`/`J` — paragraph alignment.
    ///
    /// Node-level rather than spannable: parley aligns a whole `Layout` at once
    /// (`ParagraphStyle`'s doc comment).
    Align(ondin_core::TextAlign),
}

/// The chords a live text session admits (`docs/shortcuts.md` §10).
///
/// Every binding here carries a modifier: a bare key in a session is a
/// character, and claiming one would give the buffer a second author.
///
/// The table's `Text` and `Both` chords come first. `OndinApp::dispatch` finishes
/// the session before undo, redo and save, as the top bar's buttons do.
fn text_insert_mode(ctx: &egui::Context) -> Vec<Action> {
    ctx.input(|i| {
        let m = i.modifiers;
        let alt_only = !m.command && m.alt;
        let mut out = Vec::new();
        table_chords(i, Mode::TextInsert, &mut out);

        // `Alt`+arrows, code because they answer with Shift either way. The
        // editor's caret loop is gated on Alt being *up*, or tracking would also
        // walk the caret.
        for (key, chord) in [
            (egui::Key::ArrowRight, TextChord::Tracking(1)),
            (egui::Key::ArrowLeft, TextChord::Tracking(-1)),
            (egui::Key::ArrowUp, TextChord::Leading(1)),
            (egui::Key::ArrowDown, TextChord::Leading(-1)),
        ] {
            if alt_only && i.key_pressed(key) {
                out.push(Action::TextStyle(chord));
            }
        }

        // Centre, code because it is read off the release with the release's own
        // modifiers: `egui_winit`'s `is_copy_command` ignores Shift, so
        // `Ctrl+Shift+C` never arrives as a press.
        if released_with(i, egui::Key::C).is_some_and(|e| e.command && !e.alt && e.shift) {
            out.push(Action::TextStyle(TextChord::Align(
                ondin_core::TextAlign::Center,
            )));
        }
        if cheatsheet_pressed(i) {
            out.push(Action::Cheatsheet);
        }

        out
    })
}

/// Whether this frame pressed the cheatsheet's chord — the library's door, which
/// reads the context itself because `resolve` does not run there.
pub(crate) fn cheatsheet_chord(ctx: &egui::Context) -> bool {
    ctx.input(cheatsheet_pressed)
}

/// The cheatsheet's chord, which both modes admit (`docs/shortcuts.md` §8). Code,
/// not table: it is on Control on macOS (`cheatsheet_chord_spelled`).
///
/// `Ctrl+Shift+/` arrives as `Key::Questionmark` on most layouts (§0), so both
/// spellings are read; the palette's `Ctrl+/` is Shift-up so the two never fire
/// together.
fn cheatsheet_pressed(i: &egui::InputState) -> bool {
    let m = i.modifiers;
    let held = if on_mac() {
        m.ctrl && !m.command && !m.alt
    } else {
        m.command && !m.alt
    };
    held && ((m.shift && i.key_pressed(egui::Key::Slash)) || i.key_pressed(egui::Key::Questionmark))
}

/// Where a clipboard key (`V`, `X`, `C`) stands for [`witness_swallowed_presses`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ClipKey {
    /// Up, or pressed in a way no key event showed.
    #[default]
    Up,
    /// Its press arrived as a key event: a release is read as it comes.
    Seen,
    /// Held across a focus change: where it was pressed is unknown.
    Unknown,
}

/// The clipboard keys, in [`ClipKey`] slot order.
const CLIP_KEYS: [egui::Key; 3] = [egui::Key::V, egui::Key::X, egui::Key::C];

/// Give a clipboard key's release the Ctrl its press was held with when that
/// press never arrived: `egui_winit` swallows a Ctrl+V/X/C press (and pushes no
/// `Paste` for a picture), so a key let go after Ctrl, the ordinary rolling
/// release of a quick tap, would read as a plain key. A press that arrived marks
/// the key [`ClipKey::Seen`] and its release is left alone; one held across a
/// focus change is left alone too. Runs on the raw input, before every reader.
pub(crate) fn witness_swallowed_presses(raw: &mut egui::RawInput, keys: &mut [ClipKey; 3]) {
    for e in &mut raw.events {
        match e {
            egui::Event::WindowFocused(_) => {
                for k in keys.iter_mut().filter(|k| **k != ClipKey::Up) {
                    *k = ClipKey::Unknown;
                }
            }
            egui::Event::Key {
                key,
                pressed,
                modifiers,
                ..
            } => {
                let Some(slot) = CLIP_KEYS.iter().position(|k| k == key) else {
                    continue;
                };
                match pressed {
                    true => keys[slot] = ClipKey::Seen,
                    false => {
                        if keys[slot] == ClipKey::Up {
                            modifiers.command = true;
                            if cfg!(target_os = "macos") {
                                modifiers.mac_cmd = true;
                            } else {
                                modifiers.ctrl = true;
                            }
                        }
                        keys[slot] = ClipKey::Up;
                    }
                }
            }
            _ => {}
        }
    }
}

/// The modifiers `key` was let go with this frame, as its own release event carries
/// them. A chord read off its release, because `egui_winit` swallows the press
/// (`V`, `C`, `X` with Ctrl), checks these and never the frame's `i.modifiers`: a
/// quick tap lets go of Ctrl in the same frame, and the frame then reads it as up.
fn released_with(i: &egui::InputState, key: egui::Key) -> Option<egui::Modifiers> {
    i.events.iter().find_map(|e| match e {
        egui::Event::Key {
            key: k,
            pressed: false,
            modifiers,
            ..
        } if *k == key => Some(*modifiers),
        _ => None,
    })
}

/// The normal-mode keymap: the table's chords, then the families that need more
/// than a key and exact modifiers, each with the reason it is code.
fn normal_mode(ctx: &egui::Context, nudge: NudgeStep) -> Vec<Action> {
    ctx.input(|i| {
        let m = i.modifiers;
        let cmd = m.command;
        // Ctrl *without* Alt, and neither: what the code-read chords below hold.
        let cmd_only = cmd && !m.alt;
        let plain = !m.command && !m.alt;
        let mut out = Vec::new();
        let on = |pressed: bool, action: Action, out: &mut Vec<Action>| {
            if pressed {
                out.push(action);
            }
        };

        table_chords(i, Mode::Normal, &mut out);
        if cheatsheet_pressed(i) {
            out.push(Action::Cheatsheet);
        }

        // --- clipboard: code, because it arrives as events and releases ---
        //
        // The clipboard trio never arrives as a key press: `egui_winit` intercepts
        // Ctrl+C/X/V (and the Insert synonyms) before the `Key` event is built and
        // emits `Event::Cut`/`Copy`/`Paste` instead, so `key_pressed(Key::C)` is
        // dead code in the real app.
        //
        // Only the *press* is intercepted, and `Event::Paste` is pushed only when
        // the clipboard yields non-empty text. So an image paste produces no event,
        // but the V *release* falls past the guards and arrives as an ordinary
        // key; a paste is taken from either signal and `paste_image` decides which
        // payload is there (§15 D183).
        //
        // The events are read per Alt/Shift on the frame:
        // - `Ctrl+Alt+C` is copy properties; `Event::Copy` is a sound witness.
        // - `Ctrl+Alt+V` is paste properties, read from the release below;
        //   swallowed here so it cannot fall through to an ordinary paste.
        // - `Ctrl+Alt+X` is nothing here: it is Exclude, read off the key release
        //   by the boolean loop. Emitting a cut would delete the selection.
        // - `Ctrl+Shift+V` is paste in place (`docs/shortcuts.md` §7), since the
        //   event carries no modifiers and the press is gone.
        //
        // Alt is tested before Shift in the `Paste` arms, so a hand still on Shift
        // cannot turn a property paste into a paste-in-place.
        //
        // An event carries no modifiers, and a quick chord in one slow frame lets go
        // of Alt or Shift before the frame's state is read; the key's own release in
        // that frame still holds what was down, so it speaks first.
        let held = |key: egui::Key| released_with(i, key).unwrap_or(m);
        for ev in &i.events {
            let m = match ev {
                egui::Event::Copy => held(egui::Key::C),
                egui::Event::Cut => held(egui::Key::X),
                egui::Event::Paste(_) => held(egui::Key::V),
                _ => m,
            };
            match ev {
                egui::Event::Copy if m.alt => out.push(Action::CopyProperties),
                // No Shift guard on copy or cut, deliberately: `Ctrl+Shift+C` is
                // the text session's centre-align, which does not exist in this
                // mode, so a guard would only stop it copying
                // (`shift_makes_the_paste_event_a_paste_in_place`).
                egui::Event::Copy => out.push(Action::Copy),
                // The two arms Alt only ever silences.
                egui::Event::Cut if m.alt => {}
                egui::Event::Cut => out.push(Action::Cut),
                egui::Event::Paste(_) if m.alt => {}
                egui::Event::Paste(_) if m.shift => out.push(Action::PasteInPlace),
                egui::Event::Paste(_) => out.push(Action::Paste),
                _ => {}
            }
        }
        // `Ctrl+Alt+V` is read off the release, not the event: a property paste
        // has nothing to do with the OS clipboard, and `Event::Paste` fires only
        // when it holds text, so the chord would work intermittently. It needs
        // Alt on the release and [`Action::PasteRelease`] needs it up, so the two
        // cannot double.
        // No `!m.shift` here, deliberately: Alt beats Shift on a paste
        // (`a_property_paste_is_not_also_a_paste_in_place`).
        let v_up = released_with(i, egui::Key::V);
        if v_up.is_some_and(|e| e.command && e.alt) {
            out.push(Action::PasteProperties);
        }
        // The release is its own action because it is a weaker witness than the
        // event: both fire for one text paste, a frame or more apart, and
        // `egui_winit` swallows the V press, so no state here can window them.
        // `OndinApp::dispatch` discriminates.
        //
        // No Shift term, deliberately: `Ctrl+Shift+V`'s release pushes this too,
        // but `dispatch` acts on it only when the clipboard has no text, i.e. when
        // `Event::Paste` did not fire, and then `paste` and `paste_in_place` both
        // open on `paste_image(None)`.
        if v_up.is_some_and(|e| e.command && !e.alt) {
            out.push(Action::PasteRelease);
        }

        // --- `Ctrl+Alt+X`: code, because it is read off the release ---
        //
        // `egui_winit` builds no `Key` event for a `Ctrl+X` press whatever else is
        // held (`is_cut_command` ignores Alt). The release falls past that guard,
        // as `Ctrl+V`'s does above.
        on(
            released_with(i, egui::Key::X).is_some_and(|e| e.command && e.alt && !e.shift),
            Action::Boolean(ondin_core::BoolOp::Exclude),
            &mut out,
        );

        // --- shifted characters: code, because Shift is taken either way ---
        //
        // egui reports a shifted punctuation key as the character it makes (§0):
        // each of these is the second spelling of a table chord, accepted whether
        // or not the layout needed Shift to type it.
        if cmd_only {
            for (key, action) in [
                (egui::Key::Plus, Action::ZoomIn),
                (
                    egui::Key::CloseCurlyBracket,
                    Action::Restack(build::ZMove::Front),
                ),
                (
                    egui::Key::OpenCurlyBracket,
                    Action::Restack(build::ZMove::Back),
                ),
                (egui::Key::Colon, Action::ToggleView(ViewSwitch::SnapGuides)),
                (egui::Key::Pipe, Action::ToggleView(ViewSwitch::Toolbar)),
            ] {
                on(i.key_pressed(key), action, &mut out);
            }
        }
        // `Shift+1` as `!`, which unlike the rest wants Shift really down.
        on(
            plain && m.shift && i.key_pressed(egui::Key::Exclamationmark),
            Action::ZoomFit,
            &mut out,
        );

        // --- `F2`: code, because it renames with Shift either way ---
        on(
            plain && i.key_pressed(egui::Key::F2),
            Action::Rename,
            &mut out,
        );

        // --- *Replace text…*: code, because its chord depends on the host ---
        //
        // A Mac's app menu takes ⌘H for *Hide*, so Replace is ⌥⌘F there
        // (`replace_chord`).
        let replace = if on_mac() {
            m.command && m.alt && !m.shift && i.key_pressed(egui::Key::F)
        } else {
            cmd_only && !m.shift && i.key_pressed(egui::Key::H)
        };
        on(replace, Action::FindText { replace: true }, &mut out);

        // --- Delete and Escape: code, because they answer under any modifier ---
        on(
            i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
            Action::Delete,
            &mut out,
        );
        on(i.key_pressed(egui::Key::Escape), Action::Escape, &mut out);

        // --- arrows: code, because the step is the user's (`NudgeStep`) ---
        //
        // Nudge bare, resize with Ctrl (Shift for the coarse step).
        //
        // One table, because both actions want the same four signed pairs: a resize
        // holds the top-left and moves the opposite edge, so it grows in the arrow's
        // direction too. `cmd_only` rather than `cmd` leaves `Ctrl`+`Alt`+arrow
        // unclaimed; `Alt`+arrow is already the nudge.
        let step = nudge.step(m.shift);
        for (key, dx, dy) in [
            (egui::Key::ArrowLeft, -1.0, 0.0),
            (egui::Key::ArrowRight, 1.0, 0.0),
            (egui::Key::ArrowUp, 0.0, -1.0),
            (egui::Key::ArrowDown, 0.0, 1.0),
        ] {
            if !i.key_pressed(key) {
                continue;
            }
            let delta = Vec2::new(dx * step, dy * step);
            if !cmd {
                out.push(Action::Nudge(delta));
            } else if cmd_only {
                out.push(Action::SizeStep(delta));
            }
        }

        // --- opacity digits: code, because each carries its digit ---
        //
        // Two within 600 ms make one value, counted in the app
        // (`docs/shortcuts.md` §2). `!m.shift` keeps them off the zoom chords
        // (`Shift+0/1/2`) and off the blend chords below.
        if plain && !m.shift {
            for (n, key) in [
                egui::Key::Num0,
                egui::Key::Num1,
                egui::Key::Num2,
                egui::Key::Num3,
                egui::Key::Num4,
                egui::Key::Num5,
                egui::Key::Num6,
                egui::Key::Num7,
                egui::Key::Num8,
                egui::Key::Num9,
            ]
            .into_iter()
            .enumerate()
            {
                on(i.key_pressed(key), Action::OpacityDigit(n as u8), &mut out);
            }
        }

        // --- blend chords: code, because each carries its digit ---
        //
        // The physical digit (`docs/shortcuts.md` §2): `#`, `$`, `%`, `^`, `&`, `*`
        // and `(` have no `egui::Key` variant, as `)` and `@` have none (§0).
        if plain && m.shift {
            for (n, key) in [
                (3, egui::Key::Num3),
                (4, egui::Key::Num4),
                (5, egui::Key::Num5),
                (6, egui::Key::Num6),
                (7, egui::Key::Num7),
                (8, egui::Key::Num8),
                (9, egui::Key::Num9),
            ] {
                on(i.key_pressed(key), Action::BlendChord(n), &mut out);
            }
        }

        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive `resolve` against a synthetic frame of egui input, with the default nudge.
    pub(super) fn actions(
        mode: Mode,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) -> Vec<Action> {
        actions_nudging(NudgeStep::default(), mode, events, modifiers)
    }

    /// `actions` with the nudge step spelled out.
    fn actions_nudging(
        nudge: NudgeStep,
        mode: Mode,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) -> Vec<Action> {
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput {
            modifiers,
            ..Default::default()
        };
        raw.events = events;
        let mut got = Vec::new();
        let _ = ctx.run_ui(raw, |ui| got = resolve(ui.ctx(), mode, nudge));
        got
    }

    pub(super) fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// The key coming back up: for `C`, `X` and `V` the only key event the real
    /// app sees, since `egui_winit`'s clipboard guards sit inside `if pressed`.
    pub(super) fn release(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers,
        }
    }

    /// The clipboard trio arrives as `Event::Cut`/`Copy`/`Paste`, not as key presses.
    #[test]
    fn the_clipboard_trio_comes_in_as_events_not_keys() {
        let cmd = egui::Modifiers::COMMAND;
        for (ev, want) in [
            (egui::Event::Copy, Action::Copy),
            (egui::Event::Cut, Action::Cut),
            (egui::Event::Paste("x".into()), Action::Paste),
        ] {
            assert_eq!(actions(Mode::Normal, vec![ev], cmd), vec![want]);
        }
        // The keys themselves stay inert, or a platform that lets them through fires twice.
        for k in [egui::Key::C, egui::Key::X, egui::Key::V] {
            assert!(
                actions(Mode::Normal, vec![key(k, cmd)], cmd).is_empty(),
                "{k:?} fired an action on its own"
            );
        }
        // Duplicate is not part of the trio and is still a plain key press.
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::D, cmd)], cmd),
            vec![Action::Duplicate]
        );
    }

    /// `Ctrl+N` and `Ctrl+W` resolve, `Ctrl+Shift+N` is private mode and
    /// `Ctrl+Shift+W` unspent, and `Alt+W` (align top) is not shadowed by
    /// `Ctrl+Alt+W`.
    ///
    /// flip: `W`'s `cmd_only` → bare `cmd` → fails at "Ctrl+Alt+W belongs to
    /// neither row" (the `Alt+W` assertion stays green).
    #[test]
    fn ctrl_n_makes_a_document_and_ctrl_w_closes_one() {
        let cmd = egui::Modifiers::COMMAND;
        let shift = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::COMMAND
        };
        let alt = egui::Modifiers::ALT;
        let cmd_alt = egui::Modifiers {
            alt: true,
            ..egui::Modifiers::COMMAND
        };

        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::N, cmd)], cmd),
            vec![Action::NewDocument]
        );
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::W, cmd)], cmd),
            vec![Action::CloseDocument]
        );
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::N, shift)], shift),
            vec![Action::ToggleView(ViewSwitch::Private)],
            "Ctrl+Shift+N is private mode, not a second new document"
        );
        assert!(
            actions(Mode::Normal, vec![key(egui::Key::W, shift)], shift).is_empty(),
            "Ctrl+Shift+W is unspent and stays unspent"
        );
        for k in [egui::Key::N, egui::Key::W] {
            assert!(
                actions(Mode::TextInsert, vec![key(k, cmd)], cmd).is_empty(),
                "and a live text session owns the keyboard — {k:?}"
            );
        }

        // `Alt+W` is align-top and must not have acquired a second meaning; and
        // `Ctrl+Alt+W` is neither of the two rather than both.
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::W, alt)], alt),
            vec![Action::Align(build::Axis::Y, build::Edge::Min)]
        );
        assert!(
            actions(Mode::Normal, vec![key(egui::Key::W, cmd_alt)], cmd_alt).is_empty(),
            "Ctrl+Alt+W belongs to neither row"
        );
    }

    /// `Ctrl`+`Shift`+`V` is paste in place, told apart by the frame's modifiers
    /// since `Event::Paste` carries none; Alt excludes the trio.
    ///
    /// flip: test Shift before Alt in the `Paste` arms → fails at "Alt excludes
    /// the trio however it is combined".
    #[test]
    fn shift_makes_the_paste_event_a_paste_in_place() {
        let cmd = egui::Modifiers::COMMAND;
        let shift = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::COMMAND
        };
        let alt_shift = egui::Modifiers { alt: true, ..shift };

        assert_eq!(
            actions(Mode::Normal, vec![egui::Event::Paste("x".into())], shift),
            vec![Action::PasteInPlace],
            "Ctrl+Shift+V is paste in place"
        );
        assert_eq!(
            actions(Mode::Normal, vec![egui::Event::Paste("x".into())], cmd),
            vec![Action::Paste],
            "and the plain chord is untouched"
        );
        // `Ctrl`+`Alt`+`Shift`+`V` is not a paste of either kind.
        assert!(
            actions(
                Mode::Normal,
                vec![egui::Event::Paste("x".into())],
                alt_shift
            )
            .is_empty(),
            "Alt excludes the trio however it is combined"
        );
        // Copy and cut get no Shift reading (`Ctrl`+`Shift`+`C` is the text
        // session's centre-align).
        for ev in [egui::Event::Copy, egui::Event::Cut] {
            let got = actions(Mode::Normal, vec![ev.clone()], shift);
            assert!(
                matches!(got[..], [Action::Copy] | [Action::Cut]),
                "{ev:?} under Shift: {got:?}"
            );
        }
    }

    /// The bracket pair restacks: bare for one step, Shift for all the way.
    #[test]
    fn brackets_restack_and_shift_goes_all_the_way() {
        let cmd = egui::Modifiers::COMMAND;
        let cmd_shift = egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT);
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::CloseBracket, cmd)], cmd),
            vec![Action::Restack(build::ZMove::Forward)]
        );
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::OpenBracket, cmd)], cmd),
            vec![Action::Restack(build::ZMove::Backward)]
        );
        assert_eq!(
            actions(
                Mode::Normal,
                vec![key(egui::Key::CloseBracket, cmd_shift)],
                cmd_shift
            ),
            vec![Action::Restack(build::ZMove::Front)]
        );
        assert_eq!(
            actions(
                Mode::Normal,
                vec![key(egui::Key::OpenBracket, cmd_shift)],
                cmd_shift
            ),
            vec![Action::Restack(build::ZMove::Back)]
        );
        // Unmodified brackets are not commands — they are characters.
        let none = egui::Modifiers::NONE;
        assert!(actions(Mode::Normal, vec![key(egui::Key::CloseBracket, none)], none).is_empty());

        // `Alt` excludes all eight spellings (four keys × Shift): the curly forms
        // reach a different arm of the same loop, covered only because the guard
        // is at the top.
        let cmd_alt = egui::Modifiers::COMMAND.plus(egui::Modifiers::ALT);
        let cmd_alt_shift = cmd_alt.plus(egui::Modifiers::SHIFT);
        for m in [cmd_alt, cmd_alt_shift] {
            for k in [
                egui::Key::CloseBracket,
                egui::Key::OpenBracket,
                egui::Key::CloseCurlyBracket,
                egui::Key::OpenCurlyBracket,
            ] {
                let got = actions(Mode::Normal, vec![key(k, m)], m);
                assert!(
                    got.is_empty(),
                    "Ctrl+Alt{}+{k:?} restacks: {got:?}",
                    if m.shift { "+Shift" } else { "" }
                );
            }
        }
        // Control: a deliberate `Ctrl+Alt` row still fires.
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::S, cmd_alt)], cmd_alt),
            vec![Action::Boolean(ondin_core::BoolOp::Subtract)],
            "control: Ctrl+Alt+S is a real row and still resolves"
        );
    }

    /// `Shift`+`[` is `{`, which egui gives its own `Key`; both spellings are accepted.
    #[test]
    fn the_shifted_brackets_arrive_as_curly_keys() {
        let cmd_shift = egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT);
        assert_eq!(
            actions(
                Mode::Normal,
                vec![key(egui::Key::CloseCurlyBracket, cmd_shift)],
                cmd_shift
            ),
            vec![Action::Restack(build::ZMove::Front)]
        );
        assert_eq!(
            actions(
                Mode::Normal,
                vec![key(egui::Key::OpenCurlyBracket, cmd_shift)],
                cmd_shift
            ),
            vec![Action::Restack(build::ZMove::Back)]
        );
        // A curly key means "all the way" whatever the reported modifiers.
        let cmd = egui::Modifiers::COMMAND;
        assert_eq!(
            actions(
                Mode::Normal,
                vec![key(egui::Key::CloseCurlyBracket, cmd)],
                cmd
            ),
            vec![Action::Restack(build::ZMove::Front)]
        );
        // `{` on its own is a character.
        let none = egui::Modifiers::NONE;
        assert!(
            actions(
                Mode::Normal,
                vec![key(egui::Key::OpenCurlyBracket, none)],
                none
            )
            .is_empty()
        );
    }

    /// `Shift`+`1` is `!` in egui's logical key and must still zoom to fit;
    /// `Ctrl+1` is the control (`docs/shortcuts.md` §0).
    #[test]
    fn shift_1_zooms_to_fit_and_arrives_as_an_exclamation_mark() {
        let shift = egui::Modifiers::SHIFT;
        assert_eq!(
            actions(
                Mode::Normal,
                vec![key(egui::Key::Exclamationmark, shift)],
                shift
            ),
            vec![Action::ZoomFit],
            "Shift+1 reaches egui as `!` and must still mean zoom-to-fit"
        );
        // The other spelling, for a layout that passes the unshifted digit through.
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Num1, shift)], shift),
            vec![Action::ZoomFit]
        );
        let cmd = egui::Modifiers::COMMAND;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Num1, cmd)], cmd),
            vec![Action::ZoomFit]
        );
        // `!` is a character without the modifier.
        let none = egui::Modifiers::NONE;
        assert!(
            actions(
                Mode::Normal,
                vec![key(egui::Key::Exclamationmark, none)],
                none
            )
            .is_empty()
        );
    }

    /// Shift+R toggles the rulers and plain R still picks the Rect tool; only the
    /// shift gate on the tool block keeps them apart.
    #[test]
    fn shift_r_toggles_rulers_without_stealing_the_rect_tool() {
        let none = egui::Modifiers::NONE;
        let shift = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::NONE
        };
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::R, shift)], shift),
            vec![Action::ToggleView(ViewSwitch::Rulers)]
        );
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::R, none)], none),
            vec![Action::ChooseTool(Tool::Rect)]
        );
        // A normal-mode command: "R" typed into a text node is an R.
        assert!(actions(Mode::TextInsert, vec![key(egui::Key::R, shift)], shift).is_empty());
    }

    /// `Shift+D` reverses a path's direction without disturbing `Ctrl+D`
    /// (duplicate) or the unbound plain `D`.
    #[test]
    fn shift_d_reverses_a_path_and_ctrl_d_still_duplicates() {
        let none = egui::Modifiers::NONE;
        let shift = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::NONE
        };
        let cmd = egui::Modifiers::COMMAND;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::D, shift)], shift),
            vec![Action::ReverseSubpaths]
        );
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::D, cmd)], cmd),
            vec![Action::Duplicate],
            "Ctrl+D must still duplicate, and only duplicate"
        );
        assert!(
            actions(Mode::Normal, vec![key(egui::Key::D, none)], none).is_empty(),
            "plain D reached an action"
        );
        // A typed D stays a D.
        assert!(actions(Mode::TextInsert, vec![key(egui::Key::D, shift)], shift).is_empty());
    }

    /// `Q` alone switches the marquee's shape; with Shift or Ctrl it does nothing.
    #[test]
    fn q_switches_the_marquee_to_a_lasso() {
        let none = egui::Modifiers::NONE;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Q, none)], none),
            vec![Action::ToggleView(ViewSwitch::Lasso)]
        );
        for m in [egui::Modifiers::SHIFT, egui::Modifiers::COMMAND] {
            assert!(actions(Mode::Normal, vec![key(egui::Key::Q, m)], m).is_empty());
        }
    }

    #[test]
    fn plain_keys_choose_tools_in_normal_mode() {
        let none = egui::Modifiers::NONE;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::R, none)], none),
            vec![Action::ChooseTool(Tool::Rect)]
        );
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::H, none)], none),
            vec![Action::ChooseTool(Tool::Hand)]
        );
        // `K` reaches only the Scale tool.
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::K, none)], none),
            vec![Action::ChooseTool(Tool::Scale)]
        );
        // `A` is the node tool and `Ctrl+A` is Select All; the chord assertion
        // catches a `plain` that stopped excluding the modifier.
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::A, none)], none),
            vec![Action::ChooseTool(Tool::Node)]
        );
        let cmd = egui::Modifiers::COMMAND;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::A, cmd)], cmd),
            vec![Action::SelectAll],
            "Ctrl+A must not also change the tool"
        );
    }

    /// Enter is a plain normal-mode action and stays a newline in a text session.
    #[test]
    fn enter_is_a_normal_mode_action_and_stays_a_newline_in_text() {
        let none = egui::Modifiers::NONE;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Enter, none)], none),
            vec![Action::Enter]
        );
        assert!(
            actions(Mode::TextInsert, vec![key(egui::Key::Enter, none)], none).is_empty(),
            "Enter typed into a text node is a newline"
        );
        // Plain only.
        let cmd = egui::Modifiers::COMMAND;
        assert!(actions(Mode::Normal, vec![key(egui::Key::Enter, cmd)], cmd).is_empty());
    }

    #[test]
    fn insert_mode_swallows_every_command() {
        // The whole point of modality: in text insert, "R" is a letter.
        let none = egui::Modifiers::NONE;
        assert!(
            actions(Mode::TextInsert, vec![key(egui::Key::R, none)], none).is_empty(),
            "tool switch leaked into text insert"
        );
        assert!(
            actions(
                Mode::TextInsert,
                vec![key(egui::Key::Backspace, none)],
                none
            )
            .is_empty(),
            "Backspace must edit the buffer, not delete the node"
        );
    }

    #[test]
    fn arrows_nudge_and_shift_makes_the_step_coarse() {
        let none = egui::Modifiers::NONE;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::ArrowRight, none)], none),
            vec![Action::Nudge(Vec2::new(NUDGE, 0.0))]
        );
        let shift = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::NONE
        };
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::ArrowUp, shift)], shift),
            vec![Action::Nudge(Vec2::new(0.0, -NUDGE_LARGE))]
        );
    }

    /// `Ctrl`+arrows resize with the nudge's own signs (asserted against the nudge's
    /// vector per key), `Shift` uses the same `NudgeStep`, a bare arrow is still
    /// only a nudge, and `Ctrl`+`Alt`+arrow stays unclaimed.
    ///
    /// flip: gate on `cmd` instead of `cmd_only` → fails at the last assertion;
    /// swap the arms of the `if` → fails at the nudge assertion.
    #[test]
    fn ctrl_arrows_size_the_selection_and_leave_the_nudge_alone() {
        let none = egui::Modifiers::NONE;
        let ctrl = egui::Modifiers::COMMAND;
        let ctrl_shift = egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT);
        let ctrl_alt = egui::Modifiers::COMMAND.plus(egui::Modifiers::ALT);
        let at = |k, m| actions(Mode::Normal, vec![key(k, m)], m);

        // Every arrow, against the nudge it shares its table with.
        for k in [
            egui::Key::ArrowLeft,
            egui::Key::ArrowRight,
            egui::Key::ArrowUp,
            egui::Key::ArrowDown,
        ] {
            let [Action::Nudge(moved)] = at(k, none)[..] else {
                panic!("a bare {k:?} is still only a nudge, not {:?}", at(k, none));
            };
            assert_eq!(
                at(k, ctrl),
                vec![Action::SizeStep(moved)],
                "Ctrl+{k:?} grows by the same vector the bare key moves by"
            );
        }
        // Spelled out once, so the signs are not only self-consistent.
        assert_eq!(
            at(egui::Key::ArrowRight, ctrl),
            vec![Action::SizeStep(Vec2::new(NUDGE, 0.0))]
        );
        assert_eq!(
            at(egui::Key::ArrowDown, ctrl),
            vec![Action::SizeStep(Vec2::new(0.0, NUDGE))]
        );
        assert_eq!(
            at(egui::Key::ArrowUp, ctrl),
            vec![Action::SizeStep(Vec2::new(0.0, -NUDGE))]
        );

        assert_eq!(
            at(egui::Key::ArrowRight, ctrl_shift),
            vec![Action::SizeStep(Vec2::new(NUDGE_LARGE, 0.0))],
            "and Shift is the coarse step, the same one the arrows take"
        );
        assert_eq!(
            at(egui::Key::ArrowRight, ctrl).len(),
            1,
            "Ctrl+Right must not also nudge"
        );
        assert!(
            actions(
                Mode::Normal,
                vec![key(egui::Key::ArrowRight, ctrl_alt)],
                ctrl_alt
            )
            .is_empty(),
            "Ctrl+Alt+Right stays free: Alt+arrow is already the nudge, so this \
             chord must not acquire a second meaning"
        );
    }

    /// The two distances are not swapped on the way in; the pair is lopsided
    /// (and in range of the clamp) so a wrong field is unmistakable.
    ///
    /// flip: reverse the two arms of `NudgeStep::step` → both assertions fail.
    #[test]
    fn the_arrow_step_is_the_one_the_preference_names() {
        let step = NudgeStep {
            small: 2.0,
            large: 64.0,
        };
        let none = egui::Modifiers::NONE;
        assert_eq!(
            actions_nudging(
                step,
                Mode::Normal,
                vec![key(egui::Key::ArrowRight, none)],
                none
            ),
            vec![Action::Nudge(Vec2::new(2.0, 0.0))],
            "a bare arrow moves by the small step"
        );
        let shift = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::NONE
        };
        assert_eq!(
            actions_nudging(
                step,
                Mode::Normal,
                vec![key(egui::Key::ArrowDown, shift)],
                shift
            ),
            vec![Action::Nudge(Vec2::new(0.0, 64.0))],
            "Shift+arrow moves by the large step"
        );
    }

    /// A stored nudge step outside the Settings field's range is clamped at the
    /// reader, so the arrow key and the field agree; 4px is the in-range control.
    ///
    /// flip: drop `.round().clamp(…)` from `NudgeStep::step` → fails at the
    /// first assertion.
    #[test]
    fn a_stored_nudge_step_below_the_fields_floor_is_clamped_at_the_reader() {
        let none = egui::Modifiers::NONE;
        let arrow = |step: NudgeStep| {
            actions_nudging(
                step,
                Mode::Normal,
                vec![key(egui::Key::ArrowRight, none)],
                none,
            )
        };

        // Sub-pixel value left on disk by an older field range.
        assert_eq!(
            arrow(NudgeStep {
                small: 0.1,
                large: 10.0
            }),
            vec![Action::Nudge(Vec2::new(NudgeStep::MIN, 0.0))],
            "an arrow key may not move less than the field's own floor"
        );
        // The other end, a fraction, and a NaN.
        assert_eq!(
            arrow(NudgeStep {
                small: 5000.0,
                large: 10.0
            }),
            vec![Action::Nudge(Vec2::new(NudgeStep::MAX, 0.0))]
        );
        assert_eq!(
            arrow(NudgeStep {
                small: 3.7,
                large: 10.0
            }),
            vec![Action::Nudge(Vec2::new(4.0, 0.0))],
            "the field shows whole pixels, so the key moves by them"
        );
        assert_eq!(
            arrow(NudgeStep {
                small: f64::NAN,
                large: 10.0
            }),
            vec![Action::Nudge(Vec2::new(NUDGE, 0.0))],
            "a NaN has no clamp — `f64::clamp` returns it unchanged — so it \
             falls back to the default rather than moving the layer nowhere"
        );

        // The control: an ordinary stored value is untouched.
        assert_eq!(
            arrow(NudgeStep {
                small: 4.0,
                large: 10.0
            }),
            vec![Action::Nudge(Vec2::new(4.0, 0.0))]
        );
    }

    /// `Ctrl+Shift+S` is unbound (asserts nothing); dropping `!m.shift` would make
    /// it a second Save.
    #[test]
    fn save_takes_the_bare_chord_and_the_shifted_one_is_free() {
        let cmd = egui::Modifiers::COMMAND;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::S, cmd)], cmd),
            vec![Action::Save]
        );
        let cmd_shift = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::COMMAND
        };
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::S, cmd_shift)], cmd_shift),
            vec![],
            "Ctrl+Shift+S is unbound, not an alias for Save"
        );
    }

    /// `Ctrl+E` flattens and `Ctrl+Shift+E` does nothing: `!m.shift` looks like a
    /// redundant guard now that the chord it excluded is unbound.
    ///
    /// flip: delete `!m.shift` from *Flatten*'s arm → fails at the second
    /// assertion with `[Flatten]`.
    #[test]
    fn a_stray_shift_on_ctrl_e_does_nothing_rather_than_flattening() {
        let cmd = egui::Modifiers::COMMAND;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::E, cmd)], cmd),
            vec![Action::Flatten]
        );
        let cmd_shift = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::COMMAND
        };
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::E, cmd_shift)], cmd_shift),
            vec![],
            "Ctrl+Shift+E is unbound and must not fall through to Flatten"
        );
        // Plain `E` is still the ellipse tool.
        let none = egui::Modifiers::NONE;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::E, none)], none),
            vec![Action::ChooseTool(Tool::Ellipse)]
        );
    }

    #[test]
    fn group_and_ungroup_are_distinguished_by_shift() {
        let cmd = egui::Modifiers::COMMAND;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::G, cmd)], cmd),
            vec![Action::Group]
        );
        let cmd_shift = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::COMMAND
        };
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::G, cmd_shift)], cmd_shift),
            vec![Action::Ungroup]
        );
    }

    /// `Ctrl+Alt+M` masks, and the bare `M` and `Ctrl+M` are unbound.
    #[test]
    fn masking_is_a_chord_and_the_bare_letter_is_still_unspent() {
        let cmd_alt = egui::Modifiers {
            alt: true,
            ..egui::Modifiers::COMMAND
        };
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::M, cmd_alt)], cmd_alt),
            vec![Action::Mask]
        );
        let none = egui::Modifiers::NONE;
        assert!(
            actions(Mode::Normal, vec![key(egui::Key::M, none)], none).is_empty(),
            "plain M is free, and this is what will notice when it stops being"
        );
        let cmd = egui::Modifiers::COMMAND;
        assert!(
            actions(Mode::Normal, vec![key(egui::Key::M, cmd)], cmd).is_empty(),
            "Ctrl+M is not an alias — one chord, and the Figma one"
        );
    }

    /// `Ctrl+Shift+M` toggles the minimap, beside the mask chord on the same letter.
    #[test]
    fn ctrl_shift_m_toggles_the_minimap() {
        let cmd_shift = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::COMMAND
        };
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::M, cmd_shift)], cmd_shift),
            vec![Action::ToggleView(ViewSwitch::Minimap)]
        );
        let all = egui::Modifiers {
            alt: true,
            ..cmd_shift
        };
        assert!(
            actions(Mode::Normal, vec![key(egui::Key::M, all)], all).is_empty(),
            "Ctrl+Alt+Shift+M is neither"
        );
    }

    /// `Ctrl+Alt+K` creates a component and `Ctrl+Alt+B` detaches an instance
    /// (`shortcuts.md` §12); `Ctrl+Shift+K` still places an image.
    #[test]
    fn the_component_chords_land_and_leave_their_neighbours_alone() {
        let cmd_alt = egui::Modifiers {
            alt: true,
            ..egui::Modifiers::COMMAND
        };
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::K, cmd_alt)], cmd_alt),
            vec![Action::CreateComponent]
        );
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::B, cmd_alt)], cmd_alt),
            vec![Action::DetachInstance]
        );
        let cmd_shift = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::COMMAND
        };
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::K, cmd_shift)], cmd_shift),
            vec![Action::PlaceImage],
            "the neighbouring chord is untouched"
        );
        let cmd = egui::Modifiers::COMMAND;
        assert!(
            !actions(Mode::Normal, vec![key(egui::Key::B, cmd)], cmd)
                .contains(&Action::DetachInstance),
            "Ctrl+B is not an alias"
        );
    }

    /// `Shift+I` opens the insert picker and `Ctrl+Alt+P` publishes; plain `I`,
    /// `Ctrl+P` and `Ctrl+Alt+I` (Intersect) keep their own answers.
    #[test]
    fn the_kit_chords_land_and_leave_their_neighbours_alone() {
        let shift = egui::Modifiers::SHIFT;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::I, shift)], shift),
            vec![Action::InsertPicker]
        );
        let cmd_alt = egui::Modifiers {
            alt: true,
            ..egui::Modifiers::COMMAND
        };
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::P, cmd_alt)], cmd_alt),
            vec![Action::PublishKit]
        );
        assert!(
            !actions(Mode::Normal, vec![key(egui::Key::I, cmd_alt)], cmd_alt)
                .contains(&Action::InsertPicker),
            "Ctrl+Alt+I is Intersect's"
        );
        let cmd = egui::Modifiers::COMMAND;
        assert!(
            !actions(Mode::Normal, vec![key(egui::Key::P, cmd)], cmd).contains(&Action::PublishKit),
            "Ctrl+P is not an alias"
        );
        let none = egui::Modifiers::NONE;
        assert!(
            !actions(Mode::Normal, vec![key(egui::Key::I, none)], none)
                .contains(&Action::InsertPicker),
            "plain I is not this"
        );
    }

    /// `Shift+S` opens the styles and variables browser, and leaves `S` the star
    /// tool's, `Ctrl+S` the save's and `Ctrl+Alt+S` Subtract's.
    #[test]
    fn shift_s_opens_the_variables_browser_and_leaves_s_alone() {
        let shift = egui::Modifiers::SHIFT;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::S, shift)], shift),
            vec![Action::VariablesBrowser]
        );
        for m in [
            egui::Modifiers::NONE,
            egui::Modifiers::COMMAND,
            egui::Modifiers {
                alt: true,
                ..egui::Modifiers::COMMAND
            },
        ] {
            assert!(
                !actions(Mode::Normal, vec![key(egui::Key::S, m)], m)
                    .contains(&Action::VariablesBrowser),
                "{m:?}+S is not this"
            );
        }
    }

    /// `Shift+A` adds a flex layout, and leaves `A` the Node tool's and `Ctrl+A`
    /// the select-all's.
    #[test]
    fn shift_a_adds_a_layout_and_leaves_a_alone() {
        let shift = egui::Modifiers::SHIFT;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::A, shift)], shift),
            vec![Action::AddLayout]
        );
        for m in [egui::Modifiers::NONE, egui::Modifiers::COMMAND] {
            assert!(
                !actions(Mode::Normal, vec![key(egui::Key::A, m)], m).contains(&Action::AddLayout),
                "{m:?}+A is not this"
            );
        }
    }

    /// `Shift+F` is the Slice tool and plain `F` stays the frame.
    #[test]
    fn shift_f_is_the_slice_and_f_the_frame() {
        let shift = egui::Modifiers::SHIFT;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::F, shift)], shift),
            vec![Action::ChooseTool(Tool::Slice)]
        );
        let none = egui::Modifiers::NONE;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::F, none)], none),
            vec![Action::ChooseTool(Tool::Frame)]
        );
    }

    /// `Shift+M` arms the shape builder.
    #[test]
    fn shift_m_is_the_shape_builder() {
        let shift = egui::Modifiers::SHIFT;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::M, shift)], shift),
            vec![Action::ChooseTool(Tool::ShapeBuilder)]
        );
    }

    /// `Shift+P` is the pencil and plain `P` stays the pen.
    #[test]
    fn shift_p_is_the_pencil_and_p_the_pen() {
        let shift = egui::Modifiers::SHIFT;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::P, shift)], shift),
            vec![Action::ChooseTool(Tool::Pencil)]
        );
        let none = egui::Modifiers::NONE;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::P, none)], none),
            vec![Action::ChooseTool(Tool::Pen)]
        );
    }

    /// `Shift+L` is the spiral and plain `L` stays the line.
    #[test]
    fn shift_l_is_the_spiral_and_l_the_line() {
        let shift = egui::Modifiers::SHIFT;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::L, shift)], shift),
            vec![Action::ChooseTool(Tool::Spiral)]
        );
        let none = egui::Modifiers::NONE;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::L, none)], none),
            vec![Action::ChooseTool(Tool::Line)]
        );
    }

    /// `Ctrl+Alt+G` frames the selection; `Ctrl+G` still groups, `Ctrl+Shift+G`
    /// ungroups, plain `G` is the Polygon tool.
    ///
    /// flip: drop `m.alt` from the binding → fails at "Ctrl+G still groups".
    #[test]
    fn ctrl_alt_g_frames_and_leaves_the_group_chords_alone() {
        let with = |alt, shift| egui::Modifiers {
            alt,
            shift,
            ..egui::Modifiers::COMMAND
        };
        let on_g = |m| actions(Mode::Normal, vec![key(egui::Key::G, m)], m);
        assert_eq!(on_g(with(true, false)), vec![Action::FrameSelection]);
        assert_eq!(
            on_g(with(false, false)),
            vec![Action::Group],
            "Ctrl+G still groups"
        );
        assert_eq!(
            on_g(with(false, true)),
            vec![Action::Ungroup],
            "Ctrl+Shift+G still ungroups"
        );
        assert!(
            !on_g(egui::Modifiers::NONE).contains(&Action::FrameSelection),
            "plain G is not this"
        );
    }

    /// `Ctrl+Shift+K` places an image without arming the Scale tool, and plain `K`
    /// arms it without placing.
    #[test]
    fn placing_an_image_is_a_chord_and_does_not_arm_the_scale_tool() {
        let cmd_shift = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::COMMAND
        };
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::K, cmd_shift)], cmd_shift),
            vec![Action::PlaceImage]
        );
        assert_eq!(
            actions(
                Mode::Normal,
                vec![key(egui::Key::K, egui::Modifiers::NONE)],
                egui::Modifiers::NONE
            ),
            vec![Action::ChooseTool(crate::tools::Tool::Scale)]
        );
    }

    /// A paste is taken from the key release as well as from the event, and never
    /// from both.
    ///
    /// The release is the only witness an image paste gets (see `normal_mode`).
    /// That `egui_winit` delivers it is the backend→egui direction, which `run_ui`
    /// cannot show; it was confirmed by hand, so suspect an `egui-winit` upgrade
    /// moving its guards outside `if pressed` if image paste stops working. This
    /// pins our half: the release maps to a paste, and a text paste does not
    /// paste twice.
    #[test]
    fn a_paste_is_taken_from_the_release_as_well_as_the_event() {
        let cmd = egui::Modifiers::COMMAND;
        let release = |k| egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: cmd,
        };
        // The image case: no event, just the release (`Action::PasteRelease`).
        assert_eq!(
            actions(Mode::Normal, vec![release(egui::Key::V)], cmd),
            vec![Action::PasteRelease],
            "a Ctrl+V release must be read as a paste, or an image-only \
             clipboard is unreachable"
        );
        // Both signals in one frame: two distinguishable actions, not one.
        assert_eq!(
            actions(
                Mode::Normal,
                vec![egui::Event::Paste("x".into()), release(egui::Key::V)],
                cmd
            ),
            vec![Action::Paste, Action::PasteRelease],
            "the event and the release are two different witnesses and must stay so"
        );
        // Without the modifier it is just a key coming up, not a paste.
        assert!(
            actions(
                Mode::Normal,
                vec![egui::Event::Key {
                    key: egui::Key::V,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                egui::Modifiers::NONE
            )
            .is_empty(),
            "a bare V release must not paste"
        );
    }

    /// One text paste resolves as `Paste` on the press frame and `PasteRelease` a
    /// frame later, so the two stay distinct. Two frames of one `Context`, with
    /// Ctrl still down on the release frame. Which acts is `dispatch`'s call.
    #[test]
    fn the_press_and_the_release_of_one_paste_are_two_different_actions() {
        let cmd = egui::Modifiers::COMMAND;
        let ctx = egui::Context::default();
        let frame = |events: Vec<egui::Event>| {
            let mut raw = egui::RawInput {
                modifiers: cmd,
                ..Default::default()
            };
            raw.events = events;
            let mut got = Vec::new();
            let _ = ctx.run_ui(raw, |ui| {
                got = resolve(ui.ctx(), Mode::Normal, NudgeStep::default());
            });
            got
        };
        // The press: egui swallows the key and emits the event in its place.
        assert_eq!(
            frame(vec![key(egui::Key::V, cmd), egui::Event::Paste("x".into())]),
            vec![Action::Paste],
            "the fixture: the press frame resolves the event and nothing else"
        );
        // A frame later, the release nobody swallowed.
        assert_eq!(
            frame(vec![egui::Event::Key {
                key: egui::Key::V,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: cmd,
            }]),
            vec![Action::PasteRelease],
            "and the release is the weaker witness, not a second Action::Paste"
        );
    }

    #[test]
    fn modified_keys_do_not_also_switch_tools() {
        // Ctrl+V is paste, not "the select tool"; the paste arrives as an event
        // beside the key.
        let cmd = egui::Modifiers::COMMAND;
        let got = actions(
            Mode::Normal,
            vec![key(egui::Key::V, cmd), egui::Event::Paste("x".into())],
            cmd,
        );
        assert_eq!(got, vec![Action::Paste]);
    }

    /// Insert mode is still exclusive once the trio comes in as events: the canvas
    /// text editor owns them (`canvas::text_mode_input` has arms for all three).
    #[test]
    fn insert_mode_swallows_the_clipboard_events_too() {
        let cmd = egui::Modifiers::COMMAND;
        assert!(actions(Mode::TextInsert, vec![egui::Event::Copy], cmd).is_empty());
        assert!(actions(Mode::TextInsert, vec![egui::Event::Paste("x".into())], cmd).is_empty());
    }

    fn cmd_alt() -> egui::Modifiers {
        egui::Modifiers {
            command: true,
            ctrl: true,
            alt: true,
            ..Default::default()
        }
    }

    /// Under Alt the three clipboard events go three ways (`docs/shortcuts.md` §7):
    /// copy properties, paste properties (from the release), and nothing for the
    /// cut, which would delete the selection where Exclude is meant.
    ///
    /// flip: `Event::Cut if m.alt => out.push(Action::Cut)`, or delete the arm →
    /// fails at the third assertion.
    #[test]
    fn alt_turns_the_clipboard_events_into_the_property_rows_except_the_cut() {
        assert_eq!(
            actions(Mode::Normal, vec![egui::Event::Copy], cmd_alt()),
            vec![Action::CopyProperties],
            "Ctrl+Alt+C copies properties and must not also copy the layer"
        );
        // The release, not the event (see the next test).
        assert_eq!(
            actions(
                Mode::Normal,
                vec![release(egui::Key::V, cmd_alt())],
                cmd_alt()
            ),
            vec![Action::PasteProperties],
            "Ctrl+Alt+V pastes properties"
        );
        let got = actions(Mode::Normal, vec![egui::Event::Cut], cmd_alt());
        assert!(
            got.is_empty(),
            "Ctrl+Alt+X is Exclude's, read off its release — widening the guard \
             must not give the cut a way back in: {got:?}"
        );
        // The unmodified trio is untouched.
        let cmd = egui::Modifiers::COMMAND;
        assert_eq!(
            actions(
                Mode::Normal,
                vec![
                    egui::Event::Copy,
                    egui::Event::Cut,
                    egui::Event::Paste("x".into())
                ],
                cmd
            ),
            vec![Action::Copy, Action::Cut, Action::Paste]
        );
    }

    /// `Ctrl+Alt+V` is read from the key release, because `Event::Paste` exists
    /// only when the OS clipboard holds text.
    ///
    /// flip: `Event::Paste(_) if m.alt => push(PasteProperties)` → the first
    /// assertion passes and the second (empty clipboard) fails.
    #[test]
    fn a_property_paste_does_not_need_the_os_clipboard_to_hold_anything() {
        // Text on the clipboard: the event fires and must be swallowed.
        assert_eq!(
            actions(
                Mode::Normal,
                vec![
                    egui::Event::Paste("x".into()),
                    release(egui::Key::V, cmd_alt())
                ],
                cmd_alt()
            ),
            vec![Action::PasteProperties],
            "the event is Alt-silenced, so one keystroke resolves once"
        );
        // Empty clipboard: no event, and the chord still works.
        assert_eq!(
            actions(
                Mode::Normal,
                vec![release(egui::Key::V, cmd_alt())],
                cmd_alt()
            ),
            vec![Action::PasteProperties],
        );
        // With Alt up the release is still the ordinary paste's weak witness.
        let cmd = egui::Modifiers::COMMAND;
        assert_eq!(
            actions(Mode::Normal, vec![release(egui::Key::V, cmd)], cmd),
            vec![Action::PasteRelease],
        );
    }

    /// Alt beats Shift on a paste, so a hand still on Shift cannot turn a property
    /// paste into a paste-in-place (`Ctrl+Alt+Shift+V` must not resolve to both).
    #[test]
    fn a_property_paste_is_not_also_a_paste_in_place() {
        let all = egui::Modifiers {
            shift: true,
            ..cmd_alt()
        };
        assert_eq!(
            actions(
                Mode::Normal,
                vec![egui::Event::Paste("x".into()), release(egui::Key::V, all)],
                all
            ),
            vec![Action::PasteProperties],
        );
    }

    /// `Ctrl+Shift+…` is not a second spelling of `Ctrl+…`, with the exceptions
    /// beside the rule: `Ctrl+Shift+=` is zoom in (the logical key is `+`),
    /// `Ctrl+Shift+Z` is Redo, and copy/cut under Shift and `Ctrl+Alt+Shift+V` are
    /// pinned by their own tests.
    #[test]
    fn ctrl_shift_is_not_a_second_spelling_of_ctrl() {
        let cmd = egui::Modifiers::COMMAND;
        let shift = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::COMMAND
        };

        // Each answers under `Ctrl` and nothing under `Ctrl+Shift`.
        for (k, want) in [
            (egui::Key::O, Action::Open),
            (egui::Key::A, Action::SelectAll),
            (egui::Key::D, Action::Duplicate),
            (egui::Key::Y, Action::Redo),
            (egui::Key::Minus, Action::ZoomOut),
            (egui::Key::Num0, Action::ZoomReset),
            (egui::Key::Num1, Action::ZoomFit),
            (egui::Key::Num2, Action::ZoomSelection),
        ] {
            assert_eq!(
                actions(Mode::Normal, vec![key(k, cmd)], cmd),
                vec![want],
                "the fixture must reach the state: Ctrl+{k:?} still answers"
            );
            assert!(
                actions(Mode::Normal, vec![key(k, shift)], shift).is_empty(),
                "Ctrl+Shift+{k:?} is unspent and must stay so, got {:?}",
                actions(Mode::Normal, vec![key(k, shift)], shift)
            );
        }

        // Exception 1: `Ctrl+Shift+=` is how `Ctrl++` is typed.
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Plus, shift)], shift),
            vec![Action::ZoomIn],
            "Ctrl+Shift+= is zoom in, not a stray door — the logical key is `+`"
        );
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Equals, cmd)], cmd),
            vec![Action::ZoomIn],
            "and the unshifted spelling still works"
        );
        assert!(
            actions(Mode::Normal, vec![key(egui::Key::Equals, shift)], shift).is_empty(),
            "while `Equals` *with* Shift is not a third spelling — a layout that \
             sends the unshifted key under Shift gets nothing rather than a \
             surprise"
        );

        // Exception 2: `Ctrl+Shift+Z` is the documented alternate Redo.
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Z, shift)], shift),
            vec![Action::Redo],
            "Ctrl+Shift+Z is Redo and only the Y half took the guard"
        );
    }
}

#[cfg(test)]
mod boolean_key_tests {
    use super::tests::{actions, key, release};
    use super::*;
    use ondin_core::BoolOp;

    /// All four chords resolve to the right operation. `X` is driven as a release,
    /// the only event the real app gets for it (see
    /// `ctrl_alt_x_excludes_and_does_not_cut`).
    #[test]
    fn the_four_boolean_chords_resolve() {
        let cmd_alt = egui::Modifiers {
            command: true,
            ctrl: true,
            alt: true,
            ..Default::default()
        };
        for (k, op) in [
            (egui::Key::U, BoolOp::Union),
            (egui::Key::S, BoolOp::Subtract),
            (egui::Key::I, BoolOp::Intersect),
            (egui::Key::X, BoolOp::Exclude),
        ] {
            let ev = if k == egui::Key::X {
                release(k, cmd_alt)
            } else {
                key(k, cmd_alt)
            };
            let got = actions(Mode::Normal, vec![ev], cmd_alt);
            assert!(
                got.contains(&Action::Boolean(op)),
                "Ctrl+Alt+{k:?} should mean {op:?}, got {got:?}"
            );
        }
    }

    /// `Ctrl+Alt+X` excludes and does not cut: `egui_winit`'s `is_cut_command`
    /// ignores Alt, so the press arrives as `Event::Cut` and Exclude is read from
    /// the release. That a press becomes `Event::Cut` alone is the backend→egui
    /// direction, which `run_ui` cannot show.
    #[test]
    fn ctrl_alt_x_excludes_and_does_not_cut() {
        let cmd_alt = egui::Modifiers {
            command: true,
            ctrl: true,
            alt: true,
            ..Default::default()
        };
        let got = actions(Mode::Normal, vec![egui::Event::Cut], cmd_alt);
        assert!(
            !got.contains(&Action::Cut),
            "Ctrl+Alt+X must not cut: it deleted the selection where Exclude was \
             asked for, and the save succeeded either way — {got:?}"
        );
        let got = actions(Mode::Normal, vec![release(egui::Key::X, cmd_alt)], cmd_alt);
        assert_eq!(
            got,
            vec![Action::Boolean(BoolOp::Exclude)],
            "and the release is the only witness Exclude gets, so it must be read \
             as one or the chord is unreachable from the keyboard"
        );

        // Alt up is an ordinary cut.
        let cmd = egui::Modifiers {
            command: true,
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(
            actions(Mode::Normal, vec![egui::Event::Cut], cmd),
            vec![Action::Cut],
            "Ctrl+X still cuts"
        );
    }

    /// `Ctrl+S` saves only and `Ctrl+Alt+S` subtracts only.
    #[test]
    fn save_and_cut_are_not_booleans() {
        let cmd = egui::Modifiers {
            command: true,
            ctrl: true,
            ..Default::default()
        };
        let got = actions(Mode::Normal, vec![key(egui::Key::S, cmd)], cmd);
        assert!(got.contains(&Action::Save), "Ctrl+S still saves: {got:?}");
        assert!(
            !got.iter().any(|a| matches!(a, Action::Boolean(_))),
            "and does not subtract: {got:?}"
        );

        // The boolean chord must not also save.
        let cmd_alt = egui::Modifiers {
            command: true,
            ctrl: true,
            alt: true,
            ..Default::default()
        };
        let got = actions(Mode::Normal, vec![key(egui::Key::S, cmd_alt)], cmd_alt);
        assert!(
            got.contains(&Action::Boolean(BoolOp::Subtract)) && !got.contains(&Action::Save),
            "Ctrl+Alt+S subtracts only: {got:?}"
        );
    }

    /// Insert mode owns every key, booleans included — typing "x" into a text node
    /// must not exclude the selection.
    #[test]
    fn insert_mode_swallows_the_boolean_chords() {
        let cmd_alt = egui::Modifiers {
            command: true,
            ctrl: true,
            alt: true,
            ..Default::default()
        };
        // The release, the signal Exclude is read from.
        assert!(
            actions(
                Mode::TextInsert,
                vec![release(egui::Key::X, cmd_alt)],
                cmd_alt
            )
            .is_empty()
        );
    }

    /// `Ctrl+E` flattens and plain `E` is still the ellipse tool; `plain` keeps
    /// them apart.
    #[test]
    fn ctrl_e_flattens_and_plain_e_is_still_the_ellipse_tool() {
        let cmd = egui::Modifiers {
            command: true,
            ctrl: true,
            ..Default::default()
        };
        let got = actions(Mode::Normal, vec![key(egui::Key::E, cmd)], cmd);
        assert_eq!(got, vec![Action::Flatten], "Ctrl+E flattens only: {got:?}");

        let none = egui::Modifiers::default();
        let got = actions(Mode::Normal, vec![key(egui::Key::E, none)], none);
        assert_eq!(
            got,
            vec![Action::ChooseTool(Tool::Ellipse)],
            "and E on its own is the tool: {got:?}"
        );
    }

    /// Any focused widget silences the whole keymap, and `Tab` focuses one.
    ///
    /// `egui_wants_keyboard_input` is `focused().is_some()`, so egui's focus ring
    /// closes the guard `resolve` opens with. egui reads focus movement from the
    /// `RawInput` before the app runs, so the key cannot be consumed; `OndinApp`
    /// gives the focus back at the end of a frame in which it claimed a `Tab`.
    /// Pinned so an egui upgrade that narrows `wants_keyboard_input` shows here.
    #[test]
    #[expect(clippy::disallowed_methods, reason = "a test's probe widget")]
    fn a_focused_widget_silences_the_keymap_and_tab_is_what_focuses_one() {
        let ctx = egui::Context::default();
        let screen = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(400.0, 300.0),
            )),
            ..Default::default()
        };
        // One frame of two buttons with a Tab; returns whether the keymap was
        // open before the frame ran.
        let frame = |surrender: bool| {
            let mut input = screen.clone();
            input.events.push(egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
            let open = !ctx.egui_wants_keyboard_input();
            let _ = ctx.run_ui(input, |ui| {
                let _ = ui.button("one");
                let _ = ui.button("two");
            });
            // What `OndinApp::frame_body` does at the end of a frame it
            // took Tab in.
            if surrender && let Some(id) = ctx.memory(|m| m.focused()) {
                ctx.memory_mut(|m| m.surrender_focus(id));
            }
            open
        };

        assert!(frame(false), "nothing is focused to begin with");
        assert!(
            !frame(false),
            "the first Tab focused a widget, and the keymap is now shut"
        );

        // Giving the focus back each frame keeps it open for every press.
        ctx.memory_mut(|m| *m = Default::default());
        for i in 0..4 {
            assert!(frame(true), "press {i} found the keymap shut");
        }
    }

    /// A focused button lets undo, redo and save through and nothing else; a
    /// focused text entry lets nothing through.
    #[test]
    #[expect(clippy::disallowed_methods, reason = "a test's probe widgets")]
    fn a_focused_button_still_hears_undo_and_save() {
        use super::tests::key;
        let ctx = egui::Context::default();
        let mut text = String::new();
        let mut frame = |events: Vec<egui::Event>, modifiers, focus: Option<egui::Id>| {
            let input = egui::RawInput {
                events,
                modifiers,
                ..Default::default()
            };
            let mut got = Vec::new();
            let mut ids = (None, None);
            let _ = ctx.run_ui(input, |ui| {
                if let Some(id) = focus {
                    ui.memory_mut(|m| m.request_focus(id));
                }
                got = resolve(ui.ctx(), Mode::Normal, NudgeStep::default());
                ids.0 = Some(ui.button("one").id);
                ids.1 = Some(ui.text_edit_singleline(&mut text).id);
            });
            (got, ids)
        };
        let ctrl = egui::Modifiers::COMMAND;
        let (_, (button, field)) = frame(Vec::new(), egui::Modifiers::NONE, None);
        frame(Vec::new(), egui::Modifiers::NONE, button);
        let (got, _) = frame(
            vec![key(egui::Key::Z, ctrl), key(egui::Key::S, ctrl)],
            ctrl,
            None,
        );
        assert_eq!(
            got,
            [Action::Undo, Action::Save],
            "a focused button: undo and save"
        );
        let (got, _) = frame(
            vec![key(egui::Key::Delete, egui::Modifiers::NONE)],
            egui::Modifiers::NONE,
            None,
        );
        assert!(got.is_empty(), "but not Delete, the control's own: {got:?}");

        frame(Vec::new(), egui::Modifiers::NONE, field);
        frame(Vec::new(), egui::Modifiers::NONE, None);
        let (got, _) = frame(vec![key(egui::Key::Z, ctrl)], ctrl, None);
        assert!(got.is_empty(), "a focused text entry keeps Ctrl+Z: {got:?}");
    }
}

#[cfg(test)]
mod guide_chord_tests {
    use super::tests::{actions, key};
    use super::*;

    /// `Ctrl+;` and `Ctrl+Alt+;` are told apart by Alt and mutually exclusive;
    /// without `!m.alt` on the show arm `Ctrl+Alt+;` fires both.
    #[test]
    fn the_two_guide_chords_do_not_fire_each_other() {
        let cmd = egui::Modifiers::COMMAND;
        let cmd_alt = egui::Modifiers {
            alt: true,
            ..egui::Modifiers::COMMAND
        };
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Semicolon, cmd)], cmd),
            vec![Action::ToggleView(ViewSwitch::Guides)]
        );
        assert_eq!(
            actions(
                Mode::Normal,
                vec![key(egui::Key::Semicolon, cmd_alt)],
                cmd_alt
            ),
            vec![Action::ToggleView(ViewSwitch::GuideLock)],
            "Ctrl+Alt+; must lock and only lock"
        );
    }

    /// `Ctrl+Alt+Y` is pixel preview and only that; `Ctrl+Y` stays redo.
    ///
    /// flip: the pixel-preview arm without `m.alt` → fails at "Ctrl+Y stays redo".
    #[test]
    fn ctrl_alt_y_is_pixel_preview_and_ctrl_y_stays_redo() {
        let cmd = egui::Modifiers::COMMAND;
        let cmd_alt = egui::Modifiers {
            alt: true,
            ..egui::Modifiers::COMMAND
        };
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Y, cmd_alt)], cmd_alt),
            vec![Action::ToggleView(ViewSwitch::PixelPreview)],
            "Ctrl+Alt+Y is pixel preview, not redo as well"
        );
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Y, cmd)], cmd),
            vec![Action::Redo],
            "Ctrl+Y stays redo"
        );
    }

    /// The same key with Shift is a third chord, *Snap to guides*, and reaches
    /// only that. Both spellings: shifted `;` is `Key::Colon` (§0); the plain one
    /// is kept for layouts that pass the unshifted key through.
    #[test]
    fn shift_is_a_third_chord_on_this_key_and_reaches_only_snap_to_guides() {
        let cmd_shift = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::COMMAND
        };
        for spelling in [egui::Key::Colon, egui::Key::Semicolon] {
            assert_eq!(
                actions(Mode::Normal, vec![key(spelling, cmd_shift)], cmd_shift),
                vec![Action::ToggleView(ViewSwitch::SnapGuides)],
                "Ctrl+Shift+; is Snap to guides — spelled {spelling:?}, it must                  reach that and neither the show nor the lock arm"
            );
        }
        let none = egui::Modifiers::NONE;
        assert!(actions(Mode::Normal, vec![key(egui::Key::Semicolon, none)], none).is_empty());
    }

    /// Both are normal-mode commands; a semicolon typed into a text node stays one.
    #[test]
    fn the_guide_chords_are_inert_while_text_is_being_edited() {
        let cmd = egui::Modifiers::COMMAND;
        let cmd_alt = egui::Modifiers {
            alt: true,
            ..egui::Modifiers::COMMAND
        };
        assert!(actions(Mode::TextInsert, vec![key(egui::Key::Semicolon, cmd)], cmd).is_empty());
        assert!(
            actions(
                Mode::TextInsert,
                vec![key(egui::Key::Semicolon, cmd_alt)],
                cmd_alt
            )
            .is_empty()
        );
    }
}

/// The chords `docs/shortcuts.md` §3–§6 added, each paired with the neighbour
/// sharing its key, since a binding that fires both would pass a lone assertion.
#[cfg(test)]
mod consolidated_chord_tests {
    use super::tests::{actions, key};
    use super::*;

    fn shift() -> egui::Modifiers {
        egui::Modifiers {
            shift: true,
            ..egui::Modifiers::NONE
        }
    }
    fn cmd_shift() -> egui::Modifiers {
        egui::Modifiers {
            shift: true,
            ..egui::Modifiers::COMMAND
        }
    }
    fn cmd_alt() -> egui::Modifiers {
        egui::Modifiers {
            alt: true,
            ..egui::Modifiers::COMMAND
        }
    }
    fn alt() -> egui::Modifiers {
        egui::Modifiers {
            alt: true,
            ..egui::Modifiers::NONE
        }
    }
    fn alt_shift() -> egui::Modifiers {
        egui::Modifiers {
            alt: true,
            shift: true,
            ..egui::Modifiers::NONE
        }
    }

    /// Fit-everything and fit-selection are two actions, and the digit chords land
    /// on the right one. `Shift+0`/`2` need one spelling: `)` and `@` have no
    /// `egui::Key` variant, unlike `!` (§0).
    #[test]
    fn the_shift_digit_zooms_land_on_the_two_halves_of_the_split() {
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Num0, shift())], shift()),
            vec![Action::ZoomReset],
            "Shift+0 is Figma's 100%"
        );
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Num2, shift())], shift()),
            vec![Action::ZoomSelection],
            "Shift+2 is Figma's zoom-to-selection, and must not reach ZoomFit"
        );
        let cmd = egui::Modifiers::COMMAND;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Num2, cmd)], cmd),
            vec![Action::ZoomSelection],
            "Ctrl+2 is Sketch's spelling of the same thing"
        );
        // Fit still means everything, not the selection.
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Num1, cmd)], cmd),
            vec![Action::ZoomFit]
        );
    }

    /// `Shift+H` / `Shift+V` flip; plain `H` and `V` still pick their tools (the
    /// tool block is gated on `!m.shift`).
    #[test]
    fn the_flip_chords_do_not_steal_the_hand_and_select_tools() {
        let none = egui::Modifiers::NONE;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::H, shift())], shift()),
            vec![Action::Flip(build::Axis::X)]
        );
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::V, shift())], shift()),
            vec![Action::Flip(build::Axis::Y)]
        );
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::H, none)], none),
            vec![Action::ChooseTool(Tool::Hand)]
        );
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::V, none)], none),
            vec![Action::ChooseTool(Tool::Select)]
        );
    }

    /// `Alt` aligns, `Alt+Shift` distributes, and `Ctrl+Alt+S` is still subtract
    /// alone (the align block is `!cmd`, not merely `m.alt`).
    #[test]
    fn alt_aligns_alt_shift_distributes_and_neither_touches_the_booleans() {
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::A, alt())], alt()),
            vec![Action::Align(build::Axis::X, build::Edge::Min)]
        );
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::H, alt())], alt()),
            vec![Action::Align(build::Axis::X, build::Edge::Mid)]
        );
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::S, alt())], alt()),
            vec![Action::Align(build::Axis::Y, build::Edge::Max)]
        );
        assert_eq!(
            actions(
                Mode::Normal,
                vec![key(egui::Key::H, alt_shift())],
                alt_shift()
            ),
            vec![Action::Distribute(build::Axis::X)],
            "Alt+Shift+H is distribute, and must not also align centres"
        );
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::S, cmd_alt())], cmd_alt()),
            vec![Action::Boolean(ondin_core::BoolOp::Subtract)],
            "Ctrl+Alt+S is subtract alone — an align that only asked for Alt \
             would bottom-align the selection on every subtract"
        );
    }

    /// `Alt+Shift+T` is tidy up alone: not the Text tool's `T`, and `Ctrl` added
    /// takes it out of arrangement's namespace.
    #[test]
    fn alt_shift_t_tidies_and_nothing_else() {
        assert_eq!(
            actions(
                Mode::Normal,
                vec![key(egui::Key::T, alt_shift())],
                alt_shift()
            ),
            vec![Action::TidyUp]
        );
        assert!(
            !actions(Mode::Normal, vec![key(egui::Key::T, cmd_alt())], cmd_alt())
                .contains(&Action::TidyUp)
        );
    }

    /// `Ctrl+Shift+H`/`L` are layer state; the same letters plain are tools.
    #[test]
    fn ctrl_shift_h_and_l_are_layer_state_and_nothing_else() {
        assert_eq!(
            actions(
                Mode::Normal,
                vec![key(egui::Key::H, cmd_shift())],
                cmd_shift()
            ),
            vec![Action::ToggleHidden]
        );
        assert_eq!(
            actions(
                Mode::Normal,
                vec![key(egui::Key::L, cmd_shift())],
                cmd_shift()
            ),
            vec![Action::ToggleLocked]
        );
        let none = egui::Modifiers::NONE;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::L, none)], none),
            vec![Action::ChooseTool(Tool::Line)]
        );
    }

    /// `Ctrl+R` renames and `Shift+R` still toggles the rulers (`docs/shortcuts.md`'s
    /// *Unresolvable* entry 2).
    #[test]
    fn ctrl_r_renames_without_disturbing_shift_r() {
        let cmd = egui::Modifiers::COMMAND;
        let none = egui::Modifiers::NONE;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::R, cmd)], cmd),
            vec![Action::Rename]
        );
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::F2, none)], none),
            vec![Action::Rename],
            "F2 is the Windows alias for the same field"
        );
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::R, shift())], shift()),
            vec![Action::ToggleView(ViewSwitch::Rulers)],
            "rulers keep Shift+R, which is why Ctrl+R was free to be rename"
        );
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::R, none)], none),
            vec![Action::ChooseTool(Tool::Rect)]
        );
    }

    /// The three backslash chords are three different switches; the shifted one
    /// arrives as `Key::Pipe` in the real app (§0), and both spellings are asserted.
    #[test]
    fn the_three_backslash_chords_are_three_distinct_switches() {
        let cmd = egui::Modifiers::COMMAND;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Backslash, cmd)], cmd),
            vec![Action::ToggleView(ViewSwitch::Focus)]
        );
        assert_eq!(
            actions(
                Mode::Normal,
                vec![key(egui::Key::Backslash, cmd_alt())],
                cmd_alt()
            ),
            vec![Action::ToggleView(ViewSwitch::Layers)]
        );
        for spelling in [egui::Key::Pipe, egui::Key::Backslash] {
            assert_eq!(
                actions(Mode::Normal, vec![key(spelling, cmd_shift())], cmd_shift()),
                vec![Action::ToggleView(ViewSwitch::Toolbar)],
                "Ctrl+Shift+backslash is the toolbar, spelled {spelling:?} — and \
                 it must not also toggle focus mode"
            );
        }
    }

    /// `Ctrl+'` is the grid and `Ctrl+Shift+'` is snap-to-grid; `"` has no
    /// `egui::Key` variant, so one spelling each (§0).
    #[test]
    fn the_quote_chords_are_the_grid_and_its_snap() {
        let cmd = egui::Modifiers::COMMAND;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Quote, cmd)], cmd),
            vec![Action::ToggleView(ViewSwitch::Grid)]
        );
        assert_eq!(
            actions(
                Mode::Normal,
                vec![key(egui::Key::Quote, cmd_shift())],
                cmd_shift()
            ),
            vec![Action::ToggleView(ViewSwitch::SnapGrid)],
            "Ctrl+Shift+' is snap-to-grid, and must not also show the grid"
        );
    }

    /// `Ctrl+U` is snap-to-shapes and `Ctrl+Alt+U` is still union.
    #[test]
    fn ctrl_u_snaps_to_shapes_and_ctrl_alt_u_still_unites() {
        let cmd = egui::Modifiers::COMMAND;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::U, cmd)], cmd),
            vec![Action::ToggleView(ViewSwitch::SnapShapes)]
        );
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::U, cmd_alt())], cmd_alt()),
            vec![Action::Boolean(ondin_core::BoolOp::Union)],
            "Ctrl+Alt+U is union alone — a snap toggle riding along with every \
             union is the silent kind of double-fire"
        );
    }

    /// `O` is the ellipse's published letter and `E` an alias; `Ctrl+E` stays flatten.
    #[test]
    fn o_and_e_both_choose_the_ellipse() {
        let none = egui::Modifiers::NONE;
        for k in [egui::Key::O, egui::Key::E] {
            assert_eq!(
                actions(Mode::Normal, vec![key(k, none)], none),
                vec![Action::ChooseTool(Tool::Ellipse)],
                "{k:?} chooses the ellipse"
            );
        }
        let cmd = egui::Modifiers::COMMAND;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::E, cmd)], cmd),
            vec![Action::Flatten]
        );
    }

    /// The chords that mean different things per mode: each colliding chord in
    /// Normal and in TextInsert, as `docs/shortcuts.md` §10 asks.
    #[test]
    fn the_colliding_chords_mean_one_thing_per_mode() {
        let cmd = egui::Modifiers::COMMAND;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::U, cmd)], cmd),
            vec![Action::ToggleView(ViewSwitch::SnapShapes)]
        );
        assert_eq!(
            actions(Mode::TextInsert, vec![key(egui::Key::U, cmd)], cmd),
            vec![Action::TextStyle(TextChord::Underline)]
        );
        for (k, chord) in [
            (egui::Key::B, TextChord::Bold),
            (egui::Key::I, TextChord::Italic),
        ] {
            assert_eq!(
                actions(Mode::TextInsert, vec![key(k, cmd)], cmd),
                vec![Action::TextStyle(chord)],
                "{k:?} in a session"
            );
            assert!(
                !actions(Mode::Normal, vec![key(k, cmd)], cmd).contains(&Action::TextStyle(chord)),
                "{k:?} styles text only in a session"
            );
        }

        assert_eq!(
            actions(
                Mode::Normal,
                vec![key(egui::Key::L, cmd_shift())],
                cmd_shift()
            ),
            vec![Action::ToggleLocked]
        );
        assert_eq!(
            actions(
                Mode::TextInsert,
                vec![key(egui::Key::L, cmd_shift())],
                cmd_shift()
            ),
            vec![Action::TextStyle(TextChord::Align(
                ondin_core::TextAlign::Start
            ))]
        );

        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::ArrowUp, alt())], alt()),
            vec![Action::Nudge(Vec2::new(0.0, -1.0))],
            "Alt+arrows nudge in Normal mode — the nudge block is gated on !cmd              but deliberately not on !alt"
        );
        assert_eq!(
            actions(
                Mode::TextInsert,
                vec![key(egui::Key::ArrowUp, alt())],
                alt()
            ),
            vec![Action::TextStyle(TextChord::Leading(1))]
        );
    }

    /// The rest of this module's chords are Normal-mode only.
    #[test]
    fn the_rest_of_the_new_chords_are_inert_in_text_insert() {
        for (k, m) in [
            (egui::Key::Num2, shift()),
            (egui::Key::H, shift()),
            (egui::Key::A, alt()),
            (egui::Key::H, cmd_shift()),
            (egui::Key::R, egui::Modifiers::COMMAND),
            (egui::Key::Quote, egui::Modifiers::COMMAND),
            (egui::Key::Backslash, egui::Modifiers::COMMAND),
            (egui::Key::O, egui::Modifiers::NONE),
        ] {
            assert!(
                actions(Mode::TextInsert, vec![key(k, m)], m).is_empty(),
                "{k:?} must be inert in TextInsert"
            );
        }
    }
}

/// The plain digits (`docs/shortcuts.md` §2).
#[cfg(test)]
mod opacity_key_tests {
    use super::tests::{actions, key};
    use super::*;

    /// Every digit resolves to its own number; all ten are driven because the
    /// binding is an `enumerate()` whose off-by-one a single digit would miss.
    #[test]
    fn every_plain_digit_is_its_own_opacity_digit() {
        let none = egui::Modifiers::NONE;
        for (n, k) in [
            egui::Key::Num0,
            egui::Key::Num1,
            egui::Key::Num2,
            egui::Key::Num3,
            egui::Key::Num4,
            egui::Key::Num5,
            egui::Key::Num6,
            egui::Key::Num7,
            egui::Key::Num8,
            egui::Key::Num9,
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(
                actions(Mode::Normal, vec![key(k, none)], none),
                vec![Action::OpacityDigit(n as u8)],
                "{k:?} is digit {n}"
            );
        }
    }

    /// Shifted digits are zoom or blend, never opacity: `Shift+0/1/2` are zoom
    /// chords and `Shift+3`–`9` the blend chords.
    #[test]
    fn shifted_digits_are_zoom_or_blend_but_never_opacity() {
        let shift = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::NONE
        };
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Num0, shift)], shift),
            vec![Action::ZoomReset]
        );
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Num2, shift)], shift),
            vec![Action::ZoomSelection]
        );
        for (n, k) in [
            (3, egui::Key::Num3),
            (4, egui::Key::Num4),
            (5, egui::Key::Num5),
            (6, egui::Key::Num6),
            (7, egui::Key::Num7),
            (8, egui::Key::Num8),
            (9, egui::Key::Num9),
        ] {
            assert_eq!(
                actions(Mode::Normal, vec![key(k, shift)], shift),
                vec![Action::BlendChord(n)],
                "Shift+{k:?}"
            );
        }
    }

    /// `Ctrl`+digit is zoom, and a digit typed into a text node is a character.
    #[test]
    fn the_modified_and_insert_mode_digits_are_not_opacity() {
        let cmd = egui::Modifiers::COMMAND;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Num0, cmd)], cmd),
            vec![Action::ZoomReset]
        );
        let none = egui::Modifiers::NONE;
        assert!(actions(Mode::TextInsert, vec![key(egui::Key::Num4, none)], none).is_empty());
    }
}

/// The chords a live text session admits (`docs/shortcuts.md` §10).
#[cfg(test)]
mod text_insert_tests {
    use super::tests::{actions, key, release};
    use super::*;

    fn cmd_shift() -> egui::Modifiers {
        egui::Modifiers {
            shift: true,
            ..egui::Modifiers::COMMAND
        }
    }
    fn alt() -> egui::Modifiers {
        egui::Modifiers {
            alt: true,
            ..egui::Modifiers::NONE
        }
    }

    /// Bare keys are characters, modified chords are commands: swept over the
    /// letters the chords use unmodified, since a chord added without its
    /// modifier turns a letter into a command.
    #[test]
    fn no_bare_key_is_a_command_in_a_text_session() {
        let none = egui::Modifiers::NONE;
        for k in [
            egui::Key::U,
            egui::Key::L,
            egui::Key::C,
            egui::Key::R,
            egui::Key::J,
            egui::Key::Comma,
            egui::Key::Period,
            egui::Key::ArrowLeft,
            egui::Key::ArrowRight,
            egui::Key::ArrowUp,
            egui::Key::ArrowDown,
            egui::Key::Tab,
        ] {
            assert!(
                actions(Mode::TextInsert, vec![key(k, none)], none).is_empty(),
                "a bare {k:?} in a session is a character, never a command"
            );
        }
    }

    /// Font size up and down, on one spelling each: `>` and `<` have no
    /// `egui::Key` variant, so the physical `Period`/`Comma` arrives (§0).
    #[test]
    fn the_size_chords_step_both_ways() {
        assert_eq!(
            actions(
                Mode::TextInsert,
                vec![key(egui::Key::Period, cmd_shift())],
                cmd_shift()
            ),
            vec![Action::TextStyle(TextChord::Size(1))]
        );
        assert_eq!(
            actions(
                Mode::TextInsert,
                vec![key(egui::Key::Comma, cmd_shift())],
                cmd_shift()
            ),
            vec![Action::TextStyle(TextChord::Size(-1))]
        );
    }

    /// `Alt`+arrows: horizontal is tracking, vertical is leading, and the sign
    /// follows the key.
    #[test]
    fn alt_arrows_are_tracking_and_leading() {
        for (k, chord) in [
            (egui::Key::ArrowRight, TextChord::Tracking(1)),
            (egui::Key::ArrowLeft, TextChord::Tracking(-1)),
            (egui::Key::ArrowUp, TextChord::Leading(1)),
            (egui::Key::ArrowDown, TextChord::Leading(-1)),
        ] {
            assert_eq!(
                actions(Mode::TextInsert, vec![key(k, alt())], alt()),
                vec![Action::TextStyle(chord)],
                "Alt+{k:?}"
            );
        }
    }

    /// The four alignment chords; `C` is read from the release, since
    /// `is_copy_command` swallows the `Ctrl+Shift+C` press. Only the release may
    /// resolve.
    #[test]
    fn the_alignment_chords_resolve_and_centre_comes_off_the_release() {
        for (k, align) in [
            (egui::Key::L, ondin_core::TextAlign::Start),
            (egui::Key::R, ondin_core::TextAlign::End),
            (egui::Key::J, ondin_core::TextAlign::Justify),
        ] {
            assert_eq!(
                actions(Mode::TextInsert, vec![key(k, cmd_shift())], cmd_shift()),
                vec![Action::TextStyle(TextChord::Align(align))],
                "Ctrl+Shift+{k:?}"
            );
        }
        assert!(
            actions(
                Mode::TextInsert,
                vec![key(egui::Key::C, cmd_shift())],
                cmd_shift()
            )
            .is_empty(),
            "a Ctrl+Shift+C press never reaches egui in the real app, so reading \
             it here would be reading a signal that is never delivered"
        );
        assert_eq!(
            actions(
                Mode::TextInsert,
                vec![release(egui::Key::C, cmd_shift())],
                cmd_shift()
            ),
            vec![Action::TextStyle(TextChord::Align(
                ondin_core::TextAlign::Center
            ))],
            "the release falls past `is_copy_command`'s guard, which sits inside \
             `if pressed` — the same seam Exclude and the image paste are read at"
        );
    }

    /// `Ctrl+Alt` is not `Ctrl`: `cmd_only` is stated per binding.
    #[test]
    fn adding_alt_takes_a_chord_away_rather_than_leaving_it() {
        let cmd_alt = egui::Modifiers {
            alt: true,
            ..egui::Modifiers::COMMAND
        };
        assert!(actions(Mode::TextInsert, vec![key(egui::Key::U, cmd_alt)], cmd_alt).is_empty());
        // Ctrl+Alt+↑ is neither leading nor a caret move.
        assert!(
            actions(
                Mode::TextInsert,
                vec![key(egui::Key::ArrowUp, cmd_alt)],
                cmd_alt
            )
            .is_empty()
        );
    }

    /// The four document chords resolve in a session too, with `normal_mode`'s
    /// guards. The Shift sweep is separate because
    /// `ctrl_shift_is_not_a_second_spelling_of_ctrl` covers only `normal_mode`'s
    /// arms. Finishing the session happens in `OndinApp::dispatch`, not seen here.
    ///
    /// flip: delete the four-chord loop from `text_insert_mode` → fails at
    /// `Ctrl+Z`; drop `!m.shift` from the `Save` arm → fails at `Ctrl+Shift+S`.
    #[test]
    fn the_four_document_chords_resolve_in_a_session() {
        let cmd = egui::Modifiers::COMMAND;
        for (k, want) in [
            (egui::Key::Z, Action::Undo),
            (egui::Key::Y, Action::Redo),
            (egui::Key::S, Action::Save),
            (egui::Key::O, Action::Open),
        ] {
            assert_eq!(
                actions(Mode::TextInsert, vec![key(k, cmd)], cmd),
                vec![want],
                "Ctrl+{k:?} in a session"
            );
            // Same answer as the canvas gives.
            assert_eq!(
                actions(Mode::Normal, vec![key(k, cmd)], cmd),
                vec![want],
                "control: Ctrl+{k:?} outside one"
            );
        }

        // `Ctrl+Shift+Z` is the other spelling of Redo.
        assert_eq!(
            actions(
                Mode::TextInsert,
                vec![key(egui::Key::Z, cmd_shift())],
                cmd_shift()
            ),
            vec![Action::Redo],
            "Ctrl+Shift+Z is Redo in a session as well"
        );

        // The three unspent shifted chords stay unspent.
        for k in [egui::Key::Y, egui::Key::S, egui::Key::O] {
            assert!(
                actions(Mode::TextInsert, vec![key(k, cmd_shift())], cmd_shift()).is_empty(),
                "Ctrl+Shift+{k:?} is not a second spelling here either"
            );
        }
    }
}

#[cfg(test)]
mod keys_tests {
    use super::keys_for;

    /// A Mac writes chords in its menus' symbols, modifiers in their order and
    /// named keys as glyphs, whether the chord stands alone or sits in a sentence.
    ///
    /// flip: `chord_at` setting the modifiers in the order they were spelled →
    /// fails at `Ctrl+Shift+Z`.
    #[test]
    fn a_mac_writes_chords_as_its_menus_do() {
        for (spelled, mac) in [
            ("Ctrl+Shift+Z", "⇧⌘Z"),
            ("Shift+Ctrl+Z", "⇧⌘Z"),
            ("Ctrl+Alt+Enter", "⌥⌘↩"),
            ("Control+Tab", "⌃⇥"),
            ("Ctrl+;", "⌘;"),
            ("Ctrl+Alt+\\", "⌥⌘\\"),
            ("Ctrl+=", "⌘="),
            ("Ctrl+-", "⌘-"),
            ("Ctrl+'", "⌘'"),
            ("Ctrl+PageDown", "⌘PageDown"),
            ("Shift+F", "⇧F"),
            ("Shift+Esc", "⇧⎋"),
            ("Ctrl+Backspace", "⌘⌫"),
            ("Alt+drag", "⌥ drag"),
            ("Alt+click to open", "⌥ click to open"),
            ("Previous match (Shift+Enter)", "Previous match (⇧↩)"),
            (
                "names are hidden (Ctrl+Shift+N).",
                "names are hidden (⇧⌘N).",
            ),
            ("Ctrl+Alt+V/H", "⌥⌘V/H"),
        ] {
            assert_eq!(keys_for(spelled, true), mac, "{spelled}");
        }
    }

    /// A name the user gave, in the curly quotes the app sets names in, is never
    /// rewritten; the chords around it still are.
    ///
    /// flip: `keys_for` without its `“` arm → fails at the quoted name.
    #[test]
    fn a_quoted_name_is_left_as_the_user_wrote_it() {
        assert_eq!(
            keys_for(
                "“Ctrl+A notes” and its 3 layers go with it. Ctrl+Z brings them back.",
                true
            ),
            "“Ctrl+A notes” and its 3 layers go with it. ⌘Z brings them back."
        );
        assert_eq!(
            keys_for("Deleted “Alt+1 / Shift+2” · Ctrl+Z to undo", false),
            "Deleted “Alt+1 / Shift+2” · Ctrl+Z to undo",
            "and elsewhere too"
        );
        assert_eq!(
            keys_for("an open “Ctrl+X quote with no end", true),
            "an open “Ctrl+X quote with no end",
            "an unclosed quote runs to the end"
        );
    }

    /// Elsewhere a chord stays as spelled, but for the physical `Control`, which
    /// is `Ctrl` there; text with no chord comes back untouched.
    ///
    /// flip: `chord_at` answering `None` for a chord off macOS → fails at
    /// `Control+Tab`.
    #[test]
    fn elsewhere_chords_stay_as_spelled() {
        for (spelled, shown) in [
            ("Ctrl+Shift+Z", "Ctrl+Shift+Z"),
            ("Alt+click to open", "Alt+click to open"),
            ("Control+Tab", "Ctrl+Tab"),
            (
                "Previous match (Control+Shift+Tab)",
                "Previous match (Ctrl+Shift+Tab)",
            ),
        ] {
            assert_eq!(keys_for(spelled, false), shown, "{spelled}");
        }
        for plain in ["Save", "1 + 1", "Shift", "MyCtrl+X", "Ctrl+ alone", "Ctrl+"] {
            assert_eq!(keys_for(plain, true), plain, "{plain} has no chord");
            assert!(
                matches!(keys_for(plain, false), std::borrow::Cow::Borrowed(_)),
                "{plain} is not copied"
            );
        }
    }
}

#[cfg(test)]
mod host_chord_tests {
    use super::tests::{actions, key};
    use super::*;

    /// A chord as `docs/shortcuts.md` spells it, as the key and modifiers egui
    /// reports for it: `Ctrl` is `command`, `Control` the physical key.
    fn pressed(chord: &str) -> (egui::Key, egui::Modifiers) {
        let mut m = egui::Modifiers::NONE;
        let mut parts: Vec<&str> = chord.split('+').collect();
        let key = parts.pop().expect("a key");
        for part in parts {
            match part {
                "Ctrl" => m.command = true,
                "Control" => m.ctrl = true,
                "Alt" => m.alt = true,
                "Shift" => m.shift = true,
                other => panic!("no modifier {other}"),
            }
        }
        (egui::Key::from_name(key).expect("a key egui names"), m)
    }

    /// The chord *Replace text…* shows is the one that opens it, on whichever host
    /// runs the test: `Ctrl+H`, or ⌥⌘F on macOS.
    ///
    /// flip: `resolve` reading Replace on ⌘H on macOS too → fails on a Mac at
    /// "opens Replace" (elsewhere the flip is a no-op, which CI's Mac leg covers).
    #[test]
    fn the_replace_chord_shown_is_the_one_bound() {
        let (k, m) = pressed(replace_chord());
        assert!(
            actions(Mode::Normal, vec![key(k, m)], m).contains(&Action::FindText { replace: true }),
            "{} opens Replace",
            replace_chord()
        );
    }

    /// The tab chords shown are the ones that step: `Ctrl+Tab`, or ⌃⇥ on macOS.
    ///
    /// flip: `tabs::read_keys` stepping on `command` on macOS → fails on a Mac at
    /// "steps forward".
    #[test]
    fn the_tab_chords_shown_are_the_ones_bound() {
        for (chord, step) in tab_step_chords().into_iter().zip([1, -1]) {
            let (k, m) = pressed(chord);
            let ctx = egui::Context::default();
            let mut got = None;
            let _ = ctx.run_ui(
                egui::RawInput {
                    modifiers: m,
                    events: vec![key(k, m)],
                    ..Default::default()
                },
                |ui| got = crate::tabs::read_keys(ui.ctx()),
            );
            let way = if step > 0 { "forward" } else { "back" };
            assert_eq!(
                got,
                Some(crate::tabs::TabKey::Step(step)),
                "{chord} steps {way}"
            );
        }
    }
}

#[cfg(test)]
mod release_chord_tests {
    use super::tests::{actions, release};
    use super::*;

    /// A chord read off its release fires when Ctrl is let go in the same frame:
    /// the release carries Ctrl, the frame already reports it up. A quick ⌘V with
    /// a picture on the clipboard arrives so.
    ///
    /// flip: the `V` arms in `normal_mode` reading `cmd_only` from `i.modifiers`
    /// again → fails at "Ctrl+V".
    #[test]
    fn a_release_chord_reads_its_own_modifiers() {
        let with = |m: egui::Modifiers| egui::Modifiers {
            command: true,
            ctrl: true,
            ..m
        };
        let none = egui::Modifiers::NONE;
        let shift = with(egui::Modifiers::SHIFT);
        let alt = with(egui::Modifiers::ALT);
        let cmd = with(none);
        for (label, mode, k, held, want) in [
            (
                "Ctrl+V",
                Mode::Normal,
                egui::Key::V,
                cmd,
                Action::PasteRelease,
            ),
            (
                "Ctrl+Alt+V",
                Mode::Normal,
                egui::Key::V,
                alt,
                Action::PasteProperties,
            ),
            (
                "Ctrl+Alt+X",
                Mode::Normal,
                egui::Key::X,
                alt,
                Action::Boolean(ondin_core::BoolOp::Exclude),
            ),
            (
                "Ctrl+Shift+C",
                Mode::TextInsert,
                egui::Key::C,
                shift,
                Action::TextStyle(TextChord::Align(ondin_core::TextAlign::Center)),
            ),
        ] {
            let got = actions(mode, vec![release(k, held)], none);
            assert!(got.contains(&want), "{label}: {got:?}");
        }
        assert!(
            actions(Mode::Normal, vec![release(egui::Key::V, none)], cmd).is_empty(),
            "a bare V let go while Ctrl is down is no paste"
        );
    }

    /// A V let go after Ctrl, whose press `egui_winit` swallowed, takes back the
    /// Ctrl it was pressed with; a V whose press arrived stays a plain V, and one
    /// held across a focus change is left alone.
    /// flip: `witness_swallowed_presses` leaving every release as it came → fails
    /// at "a rolling ⌘V".
    #[test]
    fn a_swallowed_press_lends_its_release_the_ctrl() {
        let none = egui::Modifiers::NONE;
        let run = |events: Vec<egui::Event>, keys: &mut [ClipKey; 3]| {
            let mut raw = egui::RawInput {
                events,
                ..Default::default()
            };
            witness_swallowed_presses(&mut raw, keys);
            actions(Mode::Normal, raw.events, none)
        };
        let mut keys = [ClipKey::Up; 3];
        let got = run(vec![release(egui::Key::V, none)], &mut keys);
        assert!(got.contains(&Action::PasteRelease), "a rolling ⌘V: {got:?}");
        let got = run(
            vec![
                super::tests::key(egui::Key::V, none),
                release(egui::Key::V, none),
            ],
            &mut keys,
        );
        assert!(!got.contains(&Action::PasteRelease), "a plain V: {got:?}");
        let _ = run(vec![super::tests::key(egui::Key::V, none)], &mut keys);
        let got = run(
            vec![
                egui::Event::WindowFocused(false),
                release(egui::Key::V, none),
            ],
            &mut keys,
        );
        assert!(
            !got.contains(&Action::PasteRelease),
            "across focus: {got:?}"
        );
    }

    /// A Ctrl+Alt+X tapped inside one slow frame, Alt already up when the frame
    /// is read, is Exclude alone: the cut event reads Alt off X's release.
    /// flip: the clipboard event arms reading the frame's modifiers → `Cut` appears.
    #[test]
    fn a_batched_chord_reads_alt_off_its_release() {
        let ctrl_alt = egui::Modifiers {
            command: true,
            ctrl: true,
            alt: true,
            ..Default::default()
        };
        let got = actions(
            Mode::Normal,
            vec![egui::Event::Cut, release(egui::Key::X, ctrl_alt)],
            egui::Modifiers::NONE,
        );
        assert_eq!(got, vec![Action::Boolean(ondin_core::BoolOp::Exclude)]);
    }
}

#[cfg(test)]
mod binding_table_tests {
    use super::tests::{actions, key};
    use super::*;

    /// Every table chord, pressed in each mode its scope admits, resolves to its
    /// action alone, and in a mode it does not admit to something else.
    ///
    /// flip: `Mods::held` ignoring Shift → fails at "Ctrl+Z" (Redo's
    /// `Ctrl+Shift+Z` also fires Undo).
    #[test]
    fn every_table_chord_resolves_to_its_action_alone() {
        for b in BINDINGS {
            let held = egui::Modifiers {
                alt: b.mods.alt,
                ctrl: b.mods.ctrl,
                shift: b.mods.shift,
                mac_cmd: false,
                command: b.mods.ctrl,
            };
            for mode in [Mode::Normal, Mode::TextInsert] {
                let got = actions(mode, vec![key(b.key, held)], held);
                if b.scope.admits(mode) {
                    assert_eq!(got, vec![b.action], "{} in {mode:?}", b.spelled());
                } else {
                    assert!(!got.contains(&b.action), "{} in {mode:?}", b.spelled());
                }
            }
        }
    }

    /// No two table chords share a key and modifiers in a mode both are read in.
    ///
    /// flip: binding underline's `Ctrl+U` in `Scope::Both` → fails at "Ctrl+U"
    /// (normal mode's *Snap to shapes*).
    #[test]
    fn no_chord_is_bound_twice() {
        for (n, a) in BINDINGS.iter().enumerate() {
            for b in &BINDINGS[n + 1..] {
                let shared = [Mode::Normal, Mode::TextInsert]
                    .iter()
                    .any(|&m| a.scope.admits(m) && b.scope.admits(m));
                assert!(
                    !(shared && a.key == b.key && a.mods == b.mods),
                    "{} is bound to {:?} and {:?}",
                    a.spelled(),
                    a.action,
                    b.action
                );
            }
        }
    }
}
