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
    /// `Ctrl+W` — close the document and go back to the library.
    ///
    /// Lands where `OndinApp::go_to_dashboard` lands (autosaved, snapshot dropped,
    /// library re-scanned); it is a second door onto the screen `Action::Open`
    /// opens, for the user who means "I am done with this".
    CloseDocument,
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
    /// *Export as HTML / React / Vue…*: the selection's code export, one zip
    /// through a save dialog (`OndinApp::export_code`). No chord and no menu row:
    /// the command palette is its door.
    ExportCode(ondin_export::code::Target),
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
    /// Open or close the styles and variables browser (`Shift+S`, and the rail's
    /// swatches button — §5.3f).
    VariablesBrowser,
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
    /// A quarter turn of the selection, each layer about its own origin; `true` is
    /// clockwise. Unbound, as the Transform card's buttons are.
    RotateQuarter(bool),
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
    /// (`docs/shortcuts.md` §4 and its *Unresolvable* list).
    Rename,
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
    /// One of the styling chords a live text session admits — see [`TextChord`].
    TextStyle(TextChord),
}

/// A workspace switch: something the View or Snap menu shows a tick against,
/// and something `docs/shortcuts.md` §6 gives a chord.
///
/// One enum rather than a label per call site: a menu row renders its label
/// *from* the variant and a chord names the variant directly, so neither drifts.
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
    /// `Ctrl+Alt+\`.
    Layers,
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
}

impl ViewSwitch {
    /// The menu row's text, the single source both menus label themselves from.
    pub fn label(self) -> &'static str {
        match self {
            Self::Rulers => "Show rulers",
            Self::Guides => "Show guides",
            Self::GuideLock => "Lock guides",
            Self::Grid => "Show grid",
            Self::LayoutGrid => "Show layout grid",
            Self::Outline => "Outline mode",
            Self::Layers => "Show layers",
            Self::Toolbar => "Show toolbar",
            Self::StatusMessages => "Show messages",
            Self::Focus => "Focus mode",
            Self::SnapShapes => "Snap to shapes",
            Self::SnapGuides => "Snap to guides",
            Self::SnapGrid => "Snap to grid",
            Self::SnapBaselines => "Snap to baselines",
        }
    }

    /// The chord this switch also answers to, as `docs/shortcuts.md` spells it,
    /// or `None` where it has none.
    ///
    /// Lives beside [`Self::label`] rather than in `menu.rs` so no wildcard arm
    /// there can render a switch with a chord that is not its own.
    ///
    /// This is the menu's *spelling*; `resolve` is the binding, and nothing makes
    /// them agree beyond a hand check. `SnapBaselines`, `LayoutGrid`, `Outline`
    /// and `StatusMessages` are `None` (each variant's doc says why).
    pub fn accel(self) -> Option<&'static str> {
        Some(match self {
            Self::Rulers => "Shift+R",
            Self::Guides => "Ctrl+;",
            Self::GuideLock => "Ctrl+Alt+;",
            Self::Grid => "Ctrl+'",
            Self::Layers => "Ctrl+Alt+\\",
            Self::Toolbar => "Ctrl+Shift+\\",
            Self::Focus => "Ctrl+\\",
            Self::SnapShapes => "Ctrl+U",
            Self::SnapGuides => "Ctrl+Shift+;",
            Self::SnapGrid => "Ctrl+Shift+'",
            Self::LayoutGrid | Self::Outline | Self::StatusMessages | Self::SnapBaselines => {
                return None;
            }
        })
    }

    /// The View menu's rows, in the order Illustrator's View menu reads them:
    /// rulers, then the guides that are pulled out of them, then the grid — and
    /// then the layout grid, adjacent so the two grid rows read against each other;
    /// outline view after them, the last row about the artwork.
    pub const VIEW_MENU: [Self; 10] = [
        Self::Rulers,
        Self::Guides,
        Self::GuideLock,
        Self::Grid,
        Self::LayoutGrid,
        Self::Layers,
        Self::Toolbar,
        Self::StatusMessages,
        Self::Outline,
        Self::Focus,
    ];

    /// Every variant, for the tests that have to walk them.
    ///
    /// Written by hand, so it cannot police the enum on its own; what makes it
    /// answerable is that `Self::label` and `Self::accel` are exhaustive `match`es
    /// in this same `impl`, so a new variant fails to compile beside it.
    /// `every_view_switch_is_on_exactly_one_menu` checks membership, not count.
    ///
    /// `#[cfg(test)]` because the tests are its only readers; an `#[allow]` would
    /// keep every item it names alive.
    #[cfg(test)]
    pub const ALL: [Self; 14] = [
        Self::Rulers,
        Self::Guides,
        Self::GuideLock,
        Self::Grid,
        Self::LayoutGrid,
        Self::Outline,
        Self::Layers,
        Self::Toolbar,
        Self::StatusMessages,
        Self::Focus,
        Self::SnapShapes,
        Self::SnapGuides,
        Self::SnapGrid,
        Self::SnapBaselines,
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
/// Returns nothing at all when a chrome widget owns the keyboard: text typed
/// into the inspector is text, never a command.
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
        return Vec::new();
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
fn text_insert_mode(ctx: &egui::Context) -> Vec<Action> {
    ctx.input(|i| {
        let m = i.modifiers;
        // `Ctrl` without `Alt`, for `normal_mode`'s reason.
        let cmd_only = m.command && !m.alt;
        let alt_only = !m.command && m.alt;
        let mut out = Vec::new();
        let on = |pressed: bool, chord: TextChord, out: &mut Vec<Action>| {
            if pressed {
                out.push(Action::TextStyle(chord));
            }
        };

        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::B),
            TextChord::Bold,
            &mut out,
        );
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::I),
            TextChord::Italic,
            &mut out,
        );
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::U),
            TextChord::Underline,
            &mut out,
        );
        on(
            cmd_only && m.shift && i.key_pressed(egui::Key::Period),
            TextChord::Size(1),
            &mut out,
        );
        on(
            cmd_only && m.shift && i.key_pressed(egui::Key::Comma),
            TextChord::Size(-1),
            &mut out,
        );

        // `Alt`+arrows. The editor's caret loop is gated on Alt being *up* for
        // exactly this, or tracking would also walk the caret.
        for (key, chord) in [
            (egui::Key::ArrowRight, TextChord::Tracking(1)),
            (egui::Key::ArrowLeft, TextChord::Tracking(-1)),
            (egui::Key::ArrowUp, TextChord::Leading(1)),
            (egui::Key::ArrowDown, TextChord::Leading(-1)),
        ] {
            on(alt_only && i.key_pressed(key), chord, &mut out);
        }

        // Paragraph alignment, on Illustrator's and Word's letters.
        for (key, align) in [
            (egui::Key::L, ondin_core::TextAlign::Start),
            (egui::Key::C, ondin_core::TextAlign::Center),
            (egui::Key::R, ondin_core::TextAlign::End),
            (egui::Key::J, ondin_core::TextAlign::Justify),
        ] {
            // `C` is read from the release: `egui_winit`'s `is_copy_command`
            // ignores Shift, so `Ctrl+Shift+C` never arrives as a press.
            let fired = if key == egui::Key::C {
                i.key_released(key)
            } else {
                i.key_pressed(key)
            };
            on(
                cmd_only && m.shift && fired,
                TextChord::Align(align),
                &mut out,
            );
        }

        // The four document chords. `OndinApp::dispatch` finishes the text
        // session first (`finish_text_first`), as the top bar's buttons do;
        // `Open` needs nothing of its own because `go_to_dashboard` already does.
        //
        // The guards are `normal_mode`'s, term for term, so `Ctrl+Shift+O` and
        // `Ctrl+Shift+Y` do not become second spellings; `Ctrl+Shift+Z` is the
        // one arm that accepts Shift. `ctrl_shift_is_not_a_second_spelling_of_ctrl`
        // covers `normal_mode`'s arms and not these.
        for (pressed, action) in [
            (
                cmd_only && !m.shift && i.key_pressed(egui::Key::Z),
                Action::Undo,
            ),
            (
                cmd_only
                    && ((m.shift && i.key_pressed(egui::Key::Z))
                        || (!m.shift && i.key_pressed(egui::Key::Y))),
                Action::Redo,
            ),
            (
                cmd_only && !m.shift && i.key_pressed(egui::Key::S),
                Action::Save,
            ),
            (
                cmd_only && !m.shift && i.key_pressed(egui::Key::O),
                Action::Open,
            ),
        ] {
            if pressed {
                out.push(action);
            }
        }
        out.extend(discovery_chord(i));

        out
    })
}

/// Whether this frame pressed the cheatsheet's chord — the library's door, which
/// reads the context itself because `resolve` does not run there.
pub(crate) fn cheatsheet_chord(ctx: &egui::Context) -> bool {
    ctx.input(|i| discovery_chord(i) == Some(Action::Cheatsheet))
}

/// The palette's and the cheatsheet's chords, which both modes admit
/// (`docs/shortcuts.md` §8).
///
/// `Ctrl+Shift+/` arrives as `Key::Questionmark` on most layouts (§0), so both
/// spellings are read; the palette's `/` is Shift-up so the two never fire together.
fn discovery_chord(i: &egui::InputState) -> Option<Action> {
    let m = i.modifiers;
    let cmd_only = m.command && !m.alt;
    if cmd_only
        && ((m.shift && i.key_pressed(egui::Key::Slash)) || i.key_pressed(egui::Key::Questionmark))
    {
        return Some(Action::Cheatsheet);
    }
    (cmd_only && !m.shift && (i.key_pressed(egui::Key::K) || i.key_pressed(egui::Key::Slash)))
        .then_some(Action::Palette)
}

fn normal_mode(ctx: &egui::Context, nudge: NudgeStep) -> Vec<Action> {
    ctx.input(|i| {
        let m = i.modifiers;
        let cmd = m.command;
        // Ctrl *without* Alt: what every command below except the booleans means.
        // Without it `Ctrl+Alt+S` fired `Save` and `Subtract` together. Pinned by
        // `save_and_cut_are_not_booleans`.
        let cmd_only = cmd && !m.alt;
        let plain = !m.command && !m.alt;
        let mut out = Vec::new();
        let on = |pressed: bool, action: Action, out: &mut Vec<Action>| {
            if pressed {
                out.push(action);
            }
        };

        // --- file / history (modified keys) ---
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::Z),
            Action::Undo,
            &mut out,
        );
        // `!m.shift` on the `Y` half, so `Ctrl+Shift+Y` is not a second spelling
        // of Redo (`ctrl_shift_is_not_a_second_spelling_of_ctrl` enumerates the arms).
        on(
            cmd_only
                && ((m.shift && i.key_pressed(egui::Key::Z))
                    || (!m.shift && i.key_pressed(egui::Key::Y))),
            Action::Redo,
            &mut out,
        );
        // `!m.shift` stays though nothing answers `Ctrl+Shift+S`: dropping it would
        // make the freed chord a second spelling of Save.
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::S),
            Action::Save,
            &mut out,
        );
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::O),
            Action::Open,
            &mut out,
        );
        // `!m.shift` on both keeps `Ctrl+Shift+N`/`W` unspent. `Alt+W` is *align
        // top* below, so the Alt term in `cmd_only` cannot be dropped.
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::N),
            Action::NewDocument,
            &mut out,
        );
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::W),
            Action::CloseDocument,
            &mut out,
        );
        // `cmd_only` and `!m.shift`, like `Save`: `Ctrl+Shift+,` and `Ctrl+Alt+,`
        // stay unbound.
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::Comma),
            Action::OpenSettings,
            &mut out,
        );
        out.extend(discovery_chord(i));
        // `Ctrl+Shift+E` is deliberately unbound: plain `Ctrl+E` is *Flatten*, so
        // that arm's `!m.shift` is all that stops the old export chord flattening
        // (`a_stray_shift_on_ctrl_e_does_nothing_rather_than_flattening`).
        //
        // `Ctrl+Alt+E` — *Export all* (§7). Alt without Shift, so `Ctrl+Alt+Shift+E`
        // is neither.
        on(
            cmd && m.alt && !m.shift && i.key_pressed(egui::Key::E),
            Action::ExportAll,
            &mut out,
        );
        // `Ctrl+Alt+Enter` — present (Figma's chord). Plain `Ctrl+Enter` stays unbound.
        on(
            cmd && m.alt && !m.shift && i.key_pressed(egui::Key::Enter),
            Action::Present,
            &mut out,
        );
        // `Ctrl+PageDown` / `Ctrl+PageUp` — step pages, as a browser steps tabs.
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::PageDown),
            Action::StepPage(true),
            &mut out,
        );
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::PageUp),
            Action::StepPage(false),
            &mut out,
        );

        // --- clipboard / structure ---
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
        for ev in &i.events {
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
        // when it holds text, so the chord would work intermittently. `cmd_only`
        // excludes Alt, so it cannot double with [`Action::PasteRelease`].
        // No `!m.shift` here, deliberately: Alt beats Shift on a paste
        // (`a_property_paste_is_not_also_a_paste_in_place`).
        if cmd && m.alt && i.key_released(egui::Key::V) {
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
        if cmd_only && i.key_released(egui::Key::V) {
            out.push(Action::PasteRelease);
        }
        // `!m.shift`: `docs/shortcuts.md` §11 binds `Shift+D` to reverse subpath
        // direction, so `Ctrl+Shift+D` must not duplicate.
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::D),
            Action::Duplicate,
            &mut out,
        );
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::A),
            Action::SelectAll,
            &mut out,
        );
        on(
            cmd_only && m.shift && i.key_pressed(egui::Key::G),
            Action::Ungroup,
            &mut out,
        );
        // Figma's chord for placing an image; a chord rather than a tool letter
        // because it opens a file dialog. Illustrator's `Ctrl+Shift+P` is
        // deliberately not aliased.
        on(
            cmd_only && m.shift && i.key_pressed(egui::Key::K),
            Action::PlaceImage,
            &mut out,
        );
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::G),
            Action::Group,
            &mut out,
        );

        // *Use as mask*, on Figma's chord. Alt-modified like the booleans, which
        // also make or change a container.
        on(
            cmd && m.alt && !m.shift && i.key_pressed(egui::Key::M),
            Action::Mask,
            &mut out,
        );

        // *Create component* and *Detach instance*, on Figma's chords
        // (`shortcuts.md` §12). `Ctrl+Alt` is `AltGr` on many European layouts,
        // which may type a character instead — a keymap-wide question.
        on(
            cmd && m.alt && !m.shift && i.key_pressed(egui::Key::K),
            Action::CreateComponent,
            &mut out,
        );
        on(
            cmd && m.alt && !m.shift && i.key_pressed(egui::Key::B),
            Action::DetachInstance,
            &mut out,
        );
        // *Frame selection*, on Figma's chord; `Ctrl+G` and `Ctrl+Shift+G` are
        // `cmd_only`, which excludes `Alt`.
        on(
            cmd && m.alt && !m.shift && i.key_pressed(egui::Key::G),
            Action::FrameSelection,
            &mut out,
        );

        // Flatten, on Figma's chord. Not a boolean: it dissolves the selection into
        // a single path rather than making or changing a container.
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::E),
            Action::Flatten,
            &mut out,
        );

        // --- booleans (Figma's chords) ---
        //
        // `Ctrl+Alt+U/S/I/X`. Alt keeps them clear of `Ctrl+S` and `Ctrl+X`.
        //
        // `X` is read from the release, the others from the press: `egui_winit`
        // builds no `Key` event for a `Ctrl+X` press whatever else is held
        // (`is_cut_command` ignores Alt), so `key_pressed(X)` never fires. The
        // release falls past that guard, as `Ctrl+V` does above.
        for (key, op) in [
            (egui::Key::U, ondin_core::BoolOp::Union),
            (egui::Key::S, ondin_core::BoolOp::Subtract),
            (egui::Key::I, ondin_core::BoolOp::Intersect),
            (egui::Key::X, ondin_core::BoolOp::Exclude),
        ] {
            let fired = if key == egui::Key::X {
                i.key_released(key)
            } else {
                i.key_pressed(key)
            };
            on(
                cmd && m.alt && !m.shift && fired,
                Action::Boolean(op),
                &mut out,
            );
        }

        // --- z-order (Illustrator/Sketch bracket convention) ---
        //
        // Each direction needs both spellings of its key: `Shift`+`[` is `{`,
        // which egui reports as its own `Key::OpenCurlyBracket`, not a shifted
        // `OpenBracket`.
        for (plain_key, curly_key, one_step, all_the_way) in [
            (
                egui::Key::CloseBracket,
                egui::Key::CloseCurlyBracket,
                build::ZMove::Forward,
                build::ZMove::Front,
            ),
            (
                egui::Key::OpenBracket,
                egui::Key::OpenCurlyBracket,
                build::ZMove::Backward,
                build::ZMove::Back,
            ),
        ] {
            // `|| m.alt` is `cmd_only` spelled out: without it the `Ctrl+Alt`
            // bracket chords, which `docs/shortcuts.md` binds to nothing, restack
            // the selection. Do not collapse it back to `!cmd`.
            if !cmd || m.alt {
                continue;
            }
            // The curly form only ever arrives with Shift down, so it always
            // means "all the way"; the plain form is read against the modifier,
            // for the platforms that do pass the unshifted key through.
            if i.key_pressed(curly_key) {
                out.push(Action::Restack(all_the_way));
            } else if i.key_pressed(plain_key) {
                out.push(Action::Restack(if m.shift {
                    all_the_way
                } else {
                    one_step
                }));
            }
        }

        // --- zoom ---
        // `ZoomIn` must not exclude Shift: on a US layout `+` is `Shift+=` and
        // egui reports the logical key, so `Key::Plus` is unreachable without
        // Shift (`docs/shortcuts.md` §3, §0). The guard goes on the unshifted
        // `Equals` spelling only.
        on(
            cmd_only
                && (i.key_pressed(egui::Key::Plus)
                    || (!m.shift && i.key_pressed(egui::Key::Equals))),
            Action::ZoomIn,
            &mut out,
        );
        // `-` needs no Shift and §3 records no shifted spelling.
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::Minus),
            Action::ZoomOut,
            &mut out,
        );
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::Num0),
            Action::ZoomReset,
            &mut out,
        );
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::Num1),
            Action::ZoomFit,
            &mut out,
        );
        // Both spellings of the digit: `Shift`+`1` is `!` and egui reports the
        // logical key. `Num1` stays for layouts that pass the unshifted digit through.
        on(
            plain
                && m.shift
                && (i.key_pressed(egui::Key::Exclamationmark) || i.key_pressed(egui::Key::Num1)),
            Action::ZoomFit,
            &mut out,
        );
        // `Shift+0` and `Shift+2` — Figma's — plus Sketch's `Ctrl+2`. Unlike
        // `Shift+1`, `)` and `@` (and the UK `"`) have no `egui::Key` variant, so
        // the logical key falls back to the physical digit and one spelling
        // suffices (§0).
        on(
            plain && m.shift && i.key_pressed(egui::Key::Num0),
            Action::ZoomReset,
            &mut out,
        );
        on(
            (plain && m.shift && i.key_pressed(egui::Key::Num2))
                || (cmd_only && !m.shift && i.key_pressed(egui::Key::Num2)),
            Action::ZoomSelection,
            &mut out,
        );

        // --- view ---
        //
        // Shift+R, Figma's chord. It cannot collide with the Rect tool below,
        // which is gated on Shift being *up* — a plain "R" is still the tool.
        on(
            plain && m.shift && i.key_pressed(egui::Key::R),
            Action::ToggleView(ViewSwitch::Rulers),
            &mut out,
        );
        // The guides, on Illustrator's pair (`docs/shortcuts.md` §6). Told apart by
        // Alt, so `cmd_only` on the first arm keeps `Ctrl+Alt+;` from firing both.
        // Neither involves Shift, so `Key::Semicolon` is right; `Ctrl+Shift+;`
        // arrives as `Key::Colon` and is *Snap to guides* (§0).
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::Semicolon),
            Action::ToggleView(ViewSwitch::Guides),
            &mut out,
        );
        on(
            cmd && m.alt && !m.shift && i.key_pressed(egui::Key::Semicolon),
            Action::ToggleView(ViewSwitch::GuideLock),
            &mut out,
        );

        // The grid and the three snapping switches, Illustrator's set
        // (`docs/shortcuts.md` §6).
        //
        // `Ctrl+Shift+;` is `:` and `Ctrl+Shift+\` is `|`, which `egui::Key` has
        // variants for, so they need both spellings; `Ctrl+Shift+'` is `"`, which
        // has none, so it falls back to the physical `Quote` (§0). The shifted
        // variant is accepted unconditionally, the plain one read against the
        // modifier.
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::Quote),
            Action::ToggleView(ViewSwitch::Grid),
            &mut out,
        );
        on(
            cmd_only && m.shift && i.key_pressed(egui::Key::Quote),
            Action::ToggleView(ViewSwitch::SnapGrid),
            &mut out,
        );
        // Illustrator's Smart Guides chord; underline in `Mode::TextInsert`
        // (`docs/shortcuts.md` §10).
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::U),
            Action::ToggleView(ViewSwitch::SnapShapes),
            &mut out,
        );
        on(
            cmd_only
                && (i.key_pressed(egui::Key::Colon)
                    || (m.shift && i.key_pressed(egui::Key::Semicolon))),
            Action::ToggleView(ViewSwitch::SnapGuides),
            &mut out,
        );

        // Chrome visibility. `Ctrl+\` is Figma's show/hide-UI chord and lands on
        // focus mode; the other two are this app's.
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::Backslash),
            Action::ToggleView(ViewSwitch::Focus),
            &mut out,
        );
        on(
            cmd && m.alt && !m.shift && i.key_pressed(egui::Key::Backslash),
            Action::ToggleView(ViewSwitch::Layers),
            &mut out,
        );
        on(
            cmd_only
                && (i.key_pressed(egui::Key::Pipe)
                    || (m.shift && i.key_pressed(egui::Key::Backslash))),
            Action::ToggleView(ViewSwitch::Toolbar),
            &mut out,
        );

        // --- path direction ---
        //
        // `Shift+D` for *direction*; a plain letter is reserved for switching
        // tool (`single-key tool switches` below). No borrowed-from app has a
        // shortcut for it.
        on(
            plain && m.shift && i.key_pressed(egui::Key::D),
            Action::ReverseSubpaths,
            &mut out,
        );

        // --- layer state (Figma and Sketch, which agree here) ---
        //
        // `Ctrl+Shift+H` and `Ctrl+Shift+L`; `Ctrl+Shift+L` as *align left* is a
        // `Mode::TextInsert` binding (`docs/shortcuts.md` §10).
        on(
            cmd_only && m.shift && i.key_pressed(egui::Key::H),
            Action::ToggleHidden,
            &mut out,
        );
        on(
            cmd_only && m.shift && i.key_pressed(egui::Key::L),
            Action::ToggleLocked,
            &mut out,
        );
        // Rename, on Figma's chord with the Windows convention as an alias; not
        // rulers (see [`Action::Rename`]).
        on(
            (cmd_only && !m.shift && i.key_pressed(egui::Key::R))
                || (plain && i.key_pressed(egui::Key::F2)),
            Action::Rename,
            &mut out,
        );
        // Flip, on Figma's pair. `plain` keeps these off `Alt+H`/`Alt+V` (align
        // centres) below.
        on(
            plain && m.shift && i.key_pressed(egui::Key::H),
            Action::Flip(build::Axis::X),
            &mut out,
        );
        on(
            plain && m.shift && i.key_pressed(egui::Key::V),
            Action::Flip(build::Axis::Y),
            &mut out,
        );
        // The insert picker, on `Shift+I` (`docs/shortcuts.md` §13).
        on(
            plain && m.shift && i.key_pressed(egui::Key::I),
            Action::InsertPicker,
            &mut out,
        );
        // The styles and variables browser, on `Shift+S` (`docs/shortcuts.md` §14).
        on(
            plain && m.shift && i.key_pressed(egui::Key::S),
            Action::VariablesBrowser,
            &mut out,
        );
        // *Add flex layout*, on Figma's `Shift+A`; plain `A` is the Node tool.
        on(
            plain && m.shift && i.key_pressed(egui::Key::A),
            Action::AddLayout,
            &mut out,
        );
        // *Publish kit…*, on `Ctrl+Alt+P`, beside the booleans in the `Ctrl+Alt`
        // namespace.
        on(
            cmd && m.alt && !m.shift && i.key_pressed(egui::Key::P),
            Action::PublishKit,
            &mut out,
        );

        // --- align and distribute (Alt, without Ctrl) ---
        //
        // The six aligns are Figma exact; the two distributes are ours
        // (`docs/shortcuts.md` §5). `Alt+Shift` separates them, so the aligns are
        // `!m.shift`; `!cmd` keeps `Alt+S` clear of `Ctrl+Alt+S` (subtract).
        let alt_only = !cmd && m.alt;
        for (key, axis, edge) in [
            (egui::Key::A, build::Axis::X, build::Edge::Min),
            (egui::Key::D, build::Axis::X, build::Edge::Max),
            (egui::Key::W, build::Axis::Y, build::Edge::Min),
            (egui::Key::S, build::Axis::Y, build::Edge::Max),
            (egui::Key::H, build::Axis::X, build::Edge::Mid),
            (egui::Key::V, build::Axis::Y, build::Edge::Mid),
        ] {
            on(
                alt_only && !m.shift && i.key_pressed(key),
                Action::Align(axis, edge),
                &mut out,
            );
        }
        for (key, axis) in [
            (egui::Key::H, build::Axis::X),
            (egui::Key::V, build::Axis::Y),
        ] {
            on(
                alt_only && m.shift && i.key_pressed(key),
                Action::Distribute(axis),
                &mut out,
            );
        }
        // Figma's `Ctrl+Alt+T` is the boolean namespace here; `Alt+Shift` is
        // arrangement's.
        on(
            alt_only && m.shift && i.key_pressed(egui::Key::T),
            Action::TidyUp,
            &mut out,
        );

        // --- selection ---
        on(
            i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
            Action::Delete,
            &mut out,
        );
        on(i.key_pressed(egui::Key::Escape), Action::Escape, &mut out);
        on(
            plain && !m.shift && i.key_pressed(egui::Key::Enter),
            Action::Enter,
            &mut out,
        );
        // Tab steps the point selection along a path, Shift+Tab back. Resolved
        // here so the focus guard covers it (text fields keep Tab); `dispatch`
        // ignores it unless the node tool has a path.
        on(
            plain && i.key_pressed(egui::Key::Tab),
            Action::StepPoint(!m.shift),
            &mut out,
        );

        // --- arrows: nudge bare, resize with Ctrl (Shift for the coarse step) ---
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

        // --- opacity digits (`docs/shortcuts.md` §2) ---
        //
        // `!m.shift` keeps them off the zoom chords (`Shift+0/1/2`) and off
        // `Shift`+`3`…`9`, which are reserved for blend modes.
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

        // --- single-key tool switches (Figma parity) ---
        if plain && !m.shift {
            for (key, tool) in [
                (egui::Key::V, Tool::Select),
                (egui::Key::H, Tool::Hand),
                // Figma's letter for the scale tool, and free here.
                (egui::Key::K, Tool::Scale),
                (egui::Key::F, Tool::Frame),
                (egui::Key::R, Tool::Rect),
                // `O` is the ellipse's published letter (Figma, Sketch) and `E`
                // an alias; the rail's tooltip shows `O` only.
                (egui::Key::O, Tool::Ellipse),
                (egui::Key::E, Tool::Ellipse),
                // G and S: Figma has no polygon/star shortcut to match.
                (egui::Key::G, Tool::Polygon),
                (egui::Key::S, Tool::Star),
                (egui::Key::L, Tool::Line),
                (egui::Key::P, Tool::Pen),
                // Illustrator's and Sketch's direct-selection letter (Figma's `A`
                // is the Frame tool).
                (egui::Key::A, Tool::Node),
                (egui::Key::T, Tool::Text),
                // Figma's letter; a second `I` hands back (`choose_tool`).
                (egui::Key::I, Tool::Eyedropper),
                // `C` is deliberately unbound: image editing has no letter
                // (`Enter` on a selected picture is its door).
            ] {
                if i.key_pressed(key) {
                    out.push(Action::ChooseTool(tool));
                }
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

    /// `Ctrl+N` and `Ctrl+W` resolve, `Ctrl+Shift+` of each stays unspent, and
    /// `Alt+W` (align top) is not shadowed by `Ctrl+Alt+W`.
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
        for k in [egui::Key::N, egui::Key::W] {
            assert!(
                actions(Mode::Normal, vec![key(k, shift)], shift).is_empty(),
                "Ctrl+Shift+{k:?} is unspent and stays unspent"
            );
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
            // What `<OndinApp as eframe::App>::ui` does at the end of a frame it
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

    /// Shifted digits are zoom or nothing, never opacity: `Shift+0/1/2` are zoom
    /// chords and `Shift+3`–`9` are reserved for blend modes.
    #[test]
    fn shifted_digits_are_zoom_or_nothing_but_never_opacity() {
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
        for k in [
            egui::Key::Num3,
            egui::Key::Num5,
            egui::Key::Num7,
            egui::Key::Num9,
        ] {
            assert!(
                actions(Mode::Normal, vec![key(k, shift)], shift).is_empty(),
                "Shift+{k:?} is reserved for blend modes and must stay unbound"
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
