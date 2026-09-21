//! Mode-aware input routing (§9.3, §13 "seams v1 must reserve").
//!
//! Every keystroke resolves against an explicit [`Mode`] before it becomes an
//! [`Action`]. v1's modes are deliberately trivial, but the routing is not: the
//! architecture calls this out as the seam that is expensive to retrofit, and
//! Command Mode (§13) is just a third arm of the same match.
//!
//! It also fixes a concrete bug. Shortcuts used to be read straight off
//! `egui::Context` at the top of the frame, before any widget had run, so keys
//! meant for a focused text field also fired canvas commands — typing a layer
//! name and pressing Backspace deleted the layer being renamed. Chrome focus is
//! now a first-class part of routing: if egui owns the keyboard, the canvas
//! sees nothing.

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

/// A resolved intent. Actions are produced by [`resolve`] and consumed by the
/// app; no key handling happens anywhere else, so the whole keymap is readable
/// in one place.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Action {
    Undo,
    Redo,
    /// `Ctrl+S` — write the document **and pin a version** (§15 D364).
    ///
    /// **There is no `SaveAs`, and the chord it had is free.** Save As asked
    /// where to put a file, and nothing in the library asks that any more: a
    /// document lives in the base folder, is created from the dashboard and is
    /// autosaved there without a dialog. What Save As was really used for —
    /// branching off a copy — is *Duplicate* in the dashboard's file menu, which
    /// mints a new document id rather than leaving two files claiming one
    /// history.
    ///
    /// So this chord's job changed rather than shrank. With autosave writing
    /// every thirty seconds, "save" on its own no longer means anything the user
    /// can feel; pinning a version is the thing they actually want a key for.
    Save,
    Open,
    /// `Ctrl+N` — create a document in the library and open it (§15 D383).
    ///
    /// **The dashboard's *New file* verb, reached from the editor**, so there is
    /// one function rather than two answers to "where does a new document go":
    /// `OndinApp::new_library_document` files it in the base folder before opening
    /// it, because a document that is not in the library yet is a library that
    /// lies about its own contents.
    ///
    /// ⚠️ **The project it lands in is computed at the door rather than inside
    /// the function**, because the two doors know different things: the
    /// dashboard's reads the nav being viewed, and this one reads the open
    /// document's own project. Same rule — *the project in front of you* — and the
    /// alternative was letting the editor file into whichever nav the dashboard
    /// happened to be left on, which is a destination nothing on screen names.
    NewDocument,
    /// `Ctrl+W` — close the document and go back to the library (§15 D383).
    ///
    /// **Close means the walk back, not the window.** This app has one window and
    /// the library is what is behind a document, so `Ctrl+W` lands where
    /// `OndinApp::go_to_dashboard` lands — autosaved, snapshot dropped, library
    /// re-scanned. Which makes it Figma's meaning of the chord rather than a
    /// second spelling of Alt+F4.
    ///
    /// ⚠️ **It is therefore a fourth door onto the screen `Action::Open` already
    /// opens, and that is the cost, stated.** The two verbs differ in what the
    /// user is thinking rather than in where they arrive — "I am done with this"
    /// against "take me to my files" — and a chord that does the right thing under
    /// either intent is worth more than the tidiness of one door.
    CloseDocument,
    /// `Ctrl+,` — the Settings modal (`crate::settings`, §15 D330).
    ///
    /// **Open, not toggle, and that is the honest name for what it does.** While the
    /// modal is up, [`resolve`] is not called at all — a modal owns the keyboard —
    /// so this chord can never arrive to close one. Escape, the ✕, *Cancel* and a
    /// click on the backdrop are the ways out, which is four already.
    ///
    /// The comma is VS Code's and every editor that copied it. Nothing else in this
    /// keymap wanted it: `,` was unspent in every combination, so the chord cost
    /// nothing and had no collision to argue about (`docs/shortcuts.md` §8).
    OpenSettings,
    /// Re-run every export setting in the document (§7). `Ctrl+Alt+E`.
    ///
    /// **`ExportSelection` used to sit above this on `Ctrl+Shift+E`** and was
    /// removed with its menu row on 2026-08-22: it asked where, what format and at
    /// what size every time and remembered none of it, which the inspector's Export
    /// panel (§15 D274) answers from the layer. `Ctrl+Shift+E` is now unbound —
    /// deliberately, and `Ctrl+E`'s `!m.shift` guard is what keeps it that way
    /// rather than letting it fall through to *Flatten*.
    ExportAll,
    Copy,
    Cut,
    Paste,
    /// `Ctrl`+`Shift`+`V`: the clipboard back at the coordinates it was copied from.
    ///
    /// **Resolved from `Event::Paste` plus the frame's `Shift`, not from a key**, for
    /// the reason the whole trio is (`docs/shortcuts.md` §0): `egui_winit`'s
    /// `is_paste_command` tests `command && Key::V` and never examines Shift, so the
    /// press is swallowed and the event is all that arrives. That is also what makes
    /// this one variant rather than a flag on [`Self::Paste`] — the discrimination
    /// happens where the event is read, and `dispatch` should not be re-deriving a
    /// modifier from a frame it no longer has.
    PasteInPlace,
    /// A paste witnessed only by `Ctrl`+`V` coming back **up**.
    ///
    /// Not the same intent as [`Self::Paste`] and deliberately a second variant: it is
    /// the *weaker* signal, and it must stand down whenever the stronger one fired.
    /// `egui_winit` emits `Event::Paste` exactly when the clipboard yields non-empty
    /// text, so "the clipboard has no text" is the precise complement of "the event
    /// existed" — which is what [`crate::app::OndinApp::dispatch`] tests, rather than a
    /// window or a frame count. §15 D183 is why the release is read at all: an
    /// image-only clipboard produces no event whatsoever.
    PasteRelease,
    /// `Ctrl`+`Alt`+`C`: lift the key layer's appearance — fills, strokes, opacity
    /// — into [`crate::app::OndinApp::property_clipboard`] (`docs/shortcuts.md`
    /// §7, Figma's chord).
    ///
    /// **Resolved from `Event::Copy` plus the frame's `Alt`**, for the same reason
    /// [`Self::PasteInPlace`] reads `Shift`: `egui_winit`'s `is_copy_command` tests
    /// `command && Key::C` and never looks at a modifier, so the press is swallowed
    /// upstream and the event is the only witness that arrives. It is a second
    /// variant rather than a flag on [`Self::Copy`] because the discrimination
    /// happens where the event is read and `dispatch` no longer has the frame.
    CopyProperties,
    /// `Ctrl`+`Alt`+`V`: give the selection the appearance
    /// [`Self::CopyProperties`] lifted (`build::paste_properties`).
    ///
    /// **The release arm below needs no widening for it**, which is worth stating
    /// because `docs/shortcuts.md` §7 predicted it would: `cmd_only` is already
    /// `command && !alt`, so `Ctrl`+`Alt`+`V` never reaches
    /// [`Self::PasteRelease`] and cannot fall through to an ordinary paste. The
    /// prediction was made against the image-paste work (§15 D183, D204) and the
    /// guard it feared had already been written.
    PasteProperties,
    Duplicate,
    Delete,
    SelectAll,
    Group,
    /// *Use as mask*: make the selection a mask, or release the one it already
    /// has — `inspector::mask_action` decides which, and it is the same toggle
    /// the identity row's button and the context-menu row reach.
    Mask,
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
    /// Move the selection by a world-space delta (arrow keys).
    Nudge(Vec2),
    /// Grow or shrink the selection's box by this many points on each axis —
    /// `Ctrl`+arrows, with `Shift` for the coarse step ([`OndinApp::size_step`]).
    ///
    /// **A `Vec2` on the same four arrows as [`Self::Nudge`], with the same signs,
    /// and that is not a coincidence.** The box is held by its top-left corner, so
    /// it grows rightwards and downwards — which makes "the arrow you press is the
    /// direction the edge moves" true of both actions at once, off one table. `→`
    /// widens and `←` narrows; `↓` makes it taller and `↑` shorter.
    ///
    /// Only ever one axis at a time as the keymap resolves it, but expressed as a
    /// vector rather than an axis-and-scalar because that is what the arithmetic
    /// downstream wants: a box takes both deltas in one edit, and splitting them
    /// would be two resizes of one shape per press.
    ///
    /// **The same two distances the bare arrows move by**, read out of the same
    /// [`NudgeStep`]. A second preference for "how far does an arrow key act" would
    /// be one more thing to set and one more thing for the two to disagree about,
    /// and nothing about a size makes 1 and 10 the wrong pair when they are the
    /// right one for a nudge.
    ///
    /// [`OndinApp::size_step`]: crate::app::OndinApp
    SizeStep(Vec2),
    /// Step the point selection one anchor along the path it belongs to — `Tab`
    /// forwards, `Shift`+`Tab` back (`OndinApp::step_point`).
    ///
    /// Only the node tool has a use for it; `dispatch` drops it otherwise, which
    /// is what leaves `Tab` to egui's focus ring everywhere else — *except* in a live
    /// text session, where it nests the list items the selection touches. That one
    /// never reaches this enum: [`resolve`] returns nothing at all in
    /// [`Mode::TextInsert`], so the canvas editor's own key loop reads it directly
    /// (§15 D173), and the two claimants cannot see the same key.
    StepPoint(bool),
    /// Restack the selection within its parent.
    Restack(build::ZMove),
    /// Traverse a path's subpaths the other way round —
    /// `OndinApp::reverse_subpaths`, on `Shift+D` for *direction*.
    ///
    /// **The verb non-zero filling makes necessary** (§15 D125): a subpath drawn
    /// the same way round as the one containing it fills solid and the opposite way
    /// cuts a hole, and nothing on screen says which you drew. Without this, a hole
    /// the wrong way round could only be fixed by deleting the run and drawing it
    /// again.
    ReverseSubpaths,
    ChooseTool(Tool),
    ZoomIn,
    ZoomOut,
    ZoomReset,
    /// Fit **everything** in the view — `Ctrl+1`, `Shift+1`.
    ///
    /// Everything, unconditionally: this used to fall back to the selection when
    /// there was one, which left "fit the whole document" unreachable while any
    /// layer was picked. [`Self::ZoomSelection`] is the other half, and splitting
    /// them is the point of the two bindings rather than a tidy-up
    /// (`docs/shortcuts.md` §3).
    ZoomFit,
    /// Fit the **selection** in the view — `Shift+2` (Figma), `Ctrl+2` (Sketch).
    ///
    /// A no-op with nothing selected, deliberately: the key that fits everything
    /// is one press away and silently doing its job here would put the two
    /// bindings back into the one action the split just separated.
    ZoomSelection,
    /// Flip one workspace switch — the same set the View and Snap menus show,
    /// so a row and its chord cannot come to mean different things
    /// (`docs/shortcuts.md` §6).
    ToggleView(ViewSwitch),
    /// Leave text editing / cancel the current gesture.
    Escape,
    /// Step **into** the selection, or back out of what you stepped into —
    /// today, point editing on a path (`OndinApp::enter_action`).
    ///
    /// **Not an `Escape` alias.** Escape is a ladder that unwinds one rung per
    /// press; Enter is a toggle that goes in with one press and out with one,
    /// which is what makes the pair legible. The pen also reads Enter directly
    /// (it finishes the path there), so `dispatch` hands this to the pen when one
    /// is in flight rather than acting twice on one key.
    Enter,
    /// Hide or show the selection — `Ctrl+Shift+H`, which Figma and Sketch
    /// agree on.
    ///
    /// **One switch for the whole selection, not one per layer** — a per-layer
    /// flip would make the chord's effect depend on a count the user cannot see.
    /// A mixed selection **hides**: *Hide* is a verb rather than a toggle, and
    /// someone pressing it on five layers of which two are already hidden is
    /// tidying up. `OndinApp::set_switch` carries the argument, including the one
    /// that lost.
    ToggleHidden,
    /// Lock or unlock the selection — `Ctrl+Shift+L`, Figma's and Sketch's.
    /// Mixed **locks**, for [`Self::ToggleHidden`]'s reason.
    ToggleLocked,
    /// Open the rename field on the selected layer — `Ctrl+R` (Figma) and `F2`
    /// (the Windows convention). The same field a double-click in the layers
    /// panel opens, so there is one rename rather than a second one.
    ///
    /// **`Ctrl+R` is rename here, not rulers.** Illustrator and Photoshop both
    /// put rulers on it and this app does not: rulers keep Figma's `Shift+R`,
    /// which is already built, so aliasing `Ctrl+R` to them would spend a free
    /// chord on a capability that already has a key (`docs/shortcuts.md` §4 and its
    /// *Unresolvable* list, where this is recorded so it is not re-argued).
    Rename,
    /// Mirror the selection across the centreline of the box it occupies —
    /// `Shift+H` and `Shift+V`, Figma's.
    ///
    /// Clear of the tool letters because the tool block is gated on Shift being
    /// *up* — the same thing that keeps `Shift+R` off the Rect tool.
    Flip(build::Axis),
    /// Align the selection to one edge of the box `OndinApp::align_target`
    /// picks — `Alt`+`A`/`D`/`W`/`S`/`H`/`V`, Figma exact.
    ///
    /// A bare `Alt` costs nothing here: there is no system menu bar for it to
    /// poke, and the block is Alt-*without*-Ctrl, so `Alt+S` cannot be confused
    /// with `Ctrl+Alt+S` (subtract).
    Align(build::Axis, build::Edge),
    /// Space the selection evenly along one axis — `Alt+Shift+H` and
    /// `Alt+Shift+V`.
    ///
    /// **Invented, and on purpose.** Figma puts these on `Ctrl+Alt+H`/`V`, which
    /// collide head-on with paste-properties *and* sit inside this keymap's
    /// boolean namespace; muddying `Ctrl+Alt` is worse than picking a fresh
    /// chord, and `Alt+Shift+H` reads as the bigger version of centring on that
    /// axis (`docs/shortcuts.md` §5).
    Distribute(build::Axis),
    /// A plain digit, which sets the selection's opacity (`docs/shortcuts.md` §2).
    ///
    /// **The digit, not the percentage.** Two digits typed within 600 ms mean
    /// one exact value — `4`,`5` is 45% — so what a *key* means cannot be
    /// decided without knowing what the key before it was. That state belongs to
    /// the app (`OndinApp::opacity_digit`, which owns the window and its
    /// repaint), and keeping it out of here is what leaves [`resolve`] the pure
    /// function of one frame's input that everything else in this file assumes.
    ///
    /// `0`–`9`, from the main row and the numpad alike: `Key::Num0`–`Num9` cover
    /// both, so there is no second spelling to accept.
    OpacityDigit(u8),
    /// One of the styling chords a live text session admits — see [`TextChord`].
    TextStyle(TextChord),
}

/// A workspace switch: something the View or Snap menu shows a tick against,
/// and something `docs/shortcuts.md` §6 gives a chord.
///
/// **One enum rather than a label per call site.** Both menus used to be keyed
/// by their own row strings — `set_view_toggle("Show grid", on)` — which held
/// the row and the state it drives in step only for as long as nobody typed a
/// different string. The chords are a second caller arriving at the same
/// switches, so the key became a type: a row renders its label *from* the
/// variant, and a binding names the variant directly. Neither can drift.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewSwitch {
    /// `Shift+R` — Figma's, and the one switch here that was already bound.
    Rulers,
    /// `Ctrl+;` — Illustrator's.
    Guides,
    /// `Ctrl+Alt+;` — Illustrator's and Photoshop's.
    ///
    /// A **workspace** switch, not a per-guide property, which is what makes one
    /// chord a legitimate way in *and* out: the toggle that locked them is the
    /// toggle that unlocks them (`OndinApp::set_guides_locked`).
    GuideLock,
    /// `Ctrl+'` — Illustrator's and Photoshop's.
    Grid,
    /// The per-frame layout grids, all of them at once — *View ▸ Show layout
    /// grid* (§15 D755).
    ///
    /// **A second grid switch rather than a second meaning for [`Self::Grid`]**,
    /// because the two are different objects: `Grid` is the pixel grid the canvas
    /// rules over everything above 8× (`grid::VISIBLE_AT_ZOOM`), and this is the
    /// column and row bands a *frame* carries in its own model
    /// (`ondin_core::layout::LayoutGrid`). Folding them into one row would make
    /// hiding the columns cost the pixel grid as well.
    ///
    /// ⚠️ **This paragraph cited Illustrator's *Show Column Guides* as the
    /// precedent and the citation could not be corroborated** (§15 D755): nothing
    /// in `docs/` names such a row, and the argument above does not need one. A
    /// true point resting on an unchecked premise is D754's shape — the reader who
    /// checks the premise discredits the point with it.
    ///
    /// ⚠️ **Necessary and not sufficient, twice over.** A grid also draws only
    /// while its own eye is on (`LayoutGrid::visible`, the per-grid flag the
    /// Layout grid panel shows) and only on a frame `shown_visible` admits. This
    /// switch sits above both: it is the workspace-level *"show me the artwork"*
    /// that §15 D385 declined — *"no global «show layout grids» toggle"*, on the
    /// argument that present mode already was one — and that D750 reopened by
    /// measuring that argument false.
    ///
    /// **Deliberately unbound**, on [`Self::SnapBaselines`]'s reasoning (§15
    /// D355): a chord is spent to match a convention, and there is no convention
    /// here to match. Nothing in `docs/shortcuts.md` had ever reserved or declined
    /// one for this — grepped before the switch was built, not assumed — and §6
    /// now carries the *"none, on purpose"* row, the second of two.
    LayoutGrid,
    /// `Ctrl+Alt+\`.
    Layers,
    /// `Ctrl+Shift+\`, which reaches egui as `Key::Pipe` (§0).
    Toolbar,
    /// `Ctrl+\` — Figma's show/hide-UI chord.
    ///
    /// Mapped onto present mode rather than onto a fourth panel toggle, because
    /// present mode already *is* "hide every piece of chrome at once and restore
    /// what the user had on leaving", which is what Figma's chord does. Escape
    /// still leaves it, and stays the one row here reachable another way.
    Present,
    /// `Ctrl+U` — Illustrator's Smart Guides, which is semantically this same
    /// switch. Clear of `Ctrl+Alt+U` (union), and it means underline in
    /// `Mode::TextInsert` — nothing but the mode tells those two apart.
    SnapShapes,
    /// `Ctrl+Shift+;`, which reaches egui as `Key::Colon` (§0).
    SnapGuides,
    /// `Ctrl+Shift+'` — Illustrator's `Ctrl+"` is literally this chord.
    SnapGrid,
    /// Text baselines (§15 D355). **Deliberately unbound**: no app has this, so
    /// there is no convention to match and no chord it is worth spending. The
    /// three above earned theirs by being Illustrator's.
    SnapBaselines,
}

impl ViewSwitch {
    /// The menu row's text, and the single source both menus label themselves
    /// from — which is what the enum bought.
    pub fn label(self) -> &'static str {
        match self {
            Self::Rulers => "Show rulers",
            Self::Guides => "Show guides",
            Self::GuideLock => "Lock guides",
            Self::Grid => "Show grid",
            Self::LayoutGrid => "Show layout grid",
            Self::Layers => "Show layers",
            Self::Toolbar => "Show toolbar",
            Self::Present => "Present mode",
            Self::SnapShapes => "Snap to shapes",
            Self::SnapGuides => "Snap to guides",
            Self::SnapGrid => "Snap to grid",
            Self::SnapBaselines => "Snap to baselines",
        }
    }

    /// The chord this switch also answers to, as `docs/shortcuts.md` spells it,
    /// or `None` where it has none.
    ///
    /// 🚨 **Here rather than in `menu.rs`, because `menu.rs` had a wildcard**
    /// (§15 D675, `[S15.2-L3-07]`). `Item::spec`'s accelerator arm spelled three
    /// variants and closed with `_ => "Ctrl+'"`, so the other **eight** fell
    /// through it — `Grid`, correctly, and **seven** that would have rendered with
    /// a chord that is not theirs, including the four `Snap` rows, which have
    /// chords of their own. (⚠️ Eight and seven are both right and are about
    /// different things, which is why they are spelled together here and in
    /// `Item::spec`'s comment.) Beside [`Self::label`] for the reason that function
    /// gives: the enum was bought so a switch's text lives in one place, and its
    /// chord is the same kind of fact.
    ///
    /// ⚠️ **This is the menu's *spelling* and `resolve` is the binding**, and
    /// nothing makes them agree — the strings here are read off the chords
    /// `resolve` tests for, which is a hand check. **Two variants are `None`**:
    /// `SnapBaselines`, for the reason [`Self::SNAP_MENU`]'s doc gives, and
    /// `LayoutGrid`, for the reason its own gives (§15 D755). ⚠️ This sentence
    /// named one of them and said *"`SnapBaselines` is `None`"* in the singular
    /// until the second arrived; the count is here rather than in a test because
    /// `no_two_view_switches_wear_one_chord` asserts only that a chordless switch
    /// stays *expressible*, which is the property that matters and is satisfied
    /// by any number of them above zero.
    pub fn accel(self) -> Option<&'static str> {
        Some(match self {
            Self::Rulers => "Shift+R",
            Self::Guides => "Ctrl+;",
            Self::GuideLock => "Ctrl+Alt+;",
            Self::Grid => "Ctrl+'",
            Self::Layers => "Ctrl+Alt+\\",
            Self::Toolbar => "Ctrl+Shift+\\",
            Self::Present => "Ctrl+\\",
            Self::SnapShapes => "Ctrl+U",
            Self::SnapGuides => "Ctrl+Shift+;",
            Self::SnapGrid => "Ctrl+Shift+'",
            Self::LayoutGrid | Self::SnapBaselines => return None,
        })
    }

    /// The View menu's rows, in the order Illustrator's View menu reads them:
    /// rulers, then the guides that are pulled out of them, then the grid — and
    /// then the layout grid (§15 D755). ⚠️ **The first four are Illustrator's
    /// order and the fifth is not claimed to be**: the two grid rows are adjacent
    /// on purpose, the question *"which grid did I just hide"* being answered by
    /// reading one row against the other, and the *Show Column Guides* precedent
    /// this sentence first cited could not be corroborated.
    pub const VIEW_MENU: [Self; 8] = [
        Self::Rulers,
        Self::Guides,
        Self::GuideLock,
        Self::Grid,
        Self::LayoutGrid,
        Self::Layers,
        Self::Toolbar,
        Self::Present,
    ];

    /// Every variant, for the tests that have to walk them (§15 D675).
    ///
    /// ⚠️ **Written by hand, so it cannot police the enum on its own** — the same
    /// admission `Item::ALL`'s doc makes two files over, and `node::kind_fixture`'s
    /// in core. What makes it answerable is that [`Self::label`] and
    /// [`Self::accel`] are exhaustive `match`es in this same `impl`: a **thirteenth**
    /// variant does not compile three lines above this, and the author is already
    /// on the screen the list is on.
    ///
    /// ⚠️ **The ordinal here is a count in prose and it has already gone stale
    /// once** — it read *"a twelfth"* while the enum held eleven, and §15 D755
    /// made it a twelve. It is kept rather than dropped because it is the one
    /// sentence that says *why* a hand-written list is answerable at all; the
    /// number itself is checked by `every_view_switch_is_on_exactly_one_menu`,
    /// which is membership rather than cardinality and does not care what the
    /// count is.
    ///
    /// `#[cfg(test)]`, because the tests below are its only readers and saying so
    /// is more honest than an `allow` — `Item::ALL`'s own words, and the shape §15
    /// D672 had to reach for in `theme::icon` after finding that an `#[allow]` on a
    /// list like this keeps every item it names alive.
    #[cfg(test)]
    pub const ALL: [Self; 12] = [
        Self::Rulers,
        Self::Guides,
        Self::GuideLock,
        Self::Grid,
        Self::LayoutGrid,
        Self::Layers,
        Self::Toolbar,
        Self::Present,
        Self::SnapShapes,
        Self::SnapGuides,
        Self::SnapGrid,
        Self::SnapBaselines,
    ];

    /// The Snap menu's rows.
    ///
    /// Baselines last: it is the only one of the four with no accelerator and the
    /// only one that is not in Illustrator, so it reads as the addition it is
    /// rather than displacing a row someone already knows the position of.
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
/// **A struct rather than two `f64` parameters, because they are the same type
/// and the swap compiles.** One is a hair and the other is ten of them, so
/// getting them the wrong way round is a keymap that feels broken in a way no
/// gate can see — and this crosses three seams to get here (the settings modal
/// writes it, [`crate::prefs::Prefs`] persists it, [`resolve`] reads it), each of
/// which is a chance to pass them in the other order.
///
/// Serialized as part of `prefs.json`, which is why the fields are named rather
/// than a tuple: a `[1.0, 10.0]` in a config file says nothing about which is
/// which.
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
    /// Named here rather than in `settings.rs` because the *reader* is what has
    /// to hold the range — see [`Self::step`].
    pub const MIN: f64 = 1.0;
    pub const MAX: f64 = 1000.0;

    /// The step an arrow press means, given whether Shift was down.
    ///
    /// ⚠️ **Clamped at use, because `prefs.json` is untrusted input** — which is
    /// §15 D349's ruling about this same file, made for the other number in it
    /// that has a range: *"a plain file that can be hand-edited, copied between
    /// machines or written by a future version"*. The layers panel's stored width
    /// is clamped by `size_range` at its reader for that reason and has a test
    /// named for it; the nudge step is the second such number and had neither.
    ///
    /// **And the bad value is not hypothetical — this build's predecessor wrote
    /// it.** The Settings field shipped as `Scrub::fine(_, 2).range(0.01..=1000)`
    /// before somebody reported *"nudging by 0.1px is crazy"*, and `Prefs` has no
    /// version field and no migration, so an upgraded install deserializes the
    /// stored `0.1` verbatim. What the user then saw was the Settings field
    /// painting **1** — the modal clamps its *draft* — over a document where an
    /// arrow key still moved 0.1px, and *Save changes* lit on a form nobody had
    /// touched. Both clamps were on the copy that Cancel throws away.
    ///
    /// ⚠️ **Not clamped in `Prefs::load`**, deliberately: §5.3's rule is
    /// *"clamped where the outline is built … **not** on the way in"*, and
    /// rewriting what is stored is the disease rather than the cure — it is the
    /// same rewrite `ui::value_field` was doing to every out-of-range number in
    /// the app until it stopped.
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
/// ⚠️ **It is not last frame's focus, and the difference is a whole feature**
/// (§15 D317). This used to say `wants_keyboard_input` reflected "the focus
/// established by the previous frame's widgets". egui updates focus in
/// `begin_pass` of the *same* frame, so a key that surrenders focus is seen here
/// with the focus **already gone** and its own press still in `input` — which is
/// how `Escape` in a focused field reaches [`OndinApp::escape`] and cancels the
/// gesture, rather than being swallowed as text. The old reading predicts the
/// opposite and was believed for a day; it is what D315 recorded as an
/// unexplained tool switch, and D26's account of the `Tab` ring says the same
/// thing in passing.
///
/// `nudge` is the only thing in the keymap the user can change — see
/// [`NudgeStep`]. It is a parameter rather than a read of `Prefs` so that this
/// function stays what its own callers rely on it being: a pure resolution of
/// this frame's events, with no state of its own to get out of date.
///
/// [`OndinApp::escape`]: crate::app::OndinApp
pub fn resolve(ctx: &egui::Context, mode: Mode, nudge: NudgeStep) -> Vec<Action> {
    if ctx.egui_wants_keyboard_input() {
        return Vec::new();
    }
    match mode {
        // Insert mode is **near** exclusive: the canvas text editor owns every
        // bare key, so the buffer can never drift from the document behind its
        // back, and only *modified* chords are resolved here (`docs/shortcuts.md`
        // §10). `Tab` is the one modified-key exception in the other direction —
        // it nests a list, and is read in the editor's loop rather than here,
        // which is what keeps it clear of `StepPoint` (§15 D173).
        Mode::TextInsert => text_insert_mode(ctx),
        Mode::Normal => normal_mode(ctx, nudge),
    }
}

/// A styling chord read **inside a live text session** (`docs/shortcuts.md` §10).
///
/// **These reach the document, which is why they are `Action`s at all.**
/// `Mode::TextInsert` is otherwise exclusive — the canvas editor owns every key
/// so the buffer cannot drift from the document behind its back — and the rule
/// that keeps that true while admitting these is *bare keys stay exclusive,
/// modified chords come through*. `Ctrl+B` does not touch the buffer. `Tab` is
/// the counter-example and stays in the editor's own loop, because what it
/// changes is the paragraph the caret is in rather than its styling (§15 D173).
///
/// Three of these keys mean something else in `Mode::Normal` — `Ctrl+U` is
/// snap-to-shapes, `Ctrl+Shift+L` is lock, `Alt`+arrows are the nudge — and
/// nothing tells them apart but the mode. That is what [`Mode`] routing was
/// built for, and this is the first time the keymap has leaned on it for more
/// than swallowing keys.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TextChord {
    /// `Ctrl+U` — underline on or off.
    ///
    /// **Exclusive with strikethrough**, per the 2026-07-31 redesign: the model
    /// carries two attributes but the UI offers one decoration, so this
    /// *replaces* a strikethrough rather than adding to it. The one row in §10
    /// whose semantics differ from every app it is borrowed from.
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
    /// Node-level rather than spannable, and that is the model's design rather
    /// than an unfinished job: parley aligns a whole `Layout` at once, so a span
    /// carrying an alignment would be a value the renderer had to ignore
    /// (`ParagraphStyle`'s doc comment). Figma does not offer per-paragraph
    /// alignment either.
    Align(ondin_core::TextAlign),
}

/// The chords a live text session admits (`docs/shortcuts.md` §10).
///
/// **Every binding here carries a modifier, and that is the invariant rather
/// than a coincidence.** A bare key in a session is a character; the moment one
/// of these claimed an unmodified key, the buffer would have a second author.
fn text_insert_mode(ctx: &egui::Context) -> Vec<Action> {
    ctx.input(|i| {
        let m = i.modifiers;
        // `Ctrl` without `Alt`, for `normal_mode`'s reason: the two are told
        // apart by Alt throughout this keymap, and a chord that states only half
        // of that fires on both.
        let cmd_only = m.command && !m.alt;
        let alt_only = !m.command && m.alt;
        let mut out = Vec::new();
        let on = |pressed: bool, chord: TextChord, out: &mut Vec<Action>| {
            if pressed {
                out.push(Action::TextStyle(chord));
            }
        };

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

        // `Alt`+arrows. The editor's own loop reads the four arrows to move the
        // caret and is gated on Alt being *up* for exactly this — without that
        // gate, widening the tracking would walk the caret at the same time.
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
            // **`C` is read from the release**, for §L2's reason: `egui_winit`'s
            // `is_copy_command` tests `command && Key::C` without looking at
            // Shift, so `Ctrl+Shift+C` never arrives as a press at all. The
            // release falls past that guard, which is the same seam Exclude and
            // the image paste are read at (§15 D183, D204).
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

        // --- the four document chords, which were dead here by omission ---
        //
        // 🚨 **`Undo`, `Redo`, `Save` and `Open` were in `normal_mode` alone, and
        // nothing had decided that** (§15 D815, `[S16.4-L1-02]`). §9.3 records
        // only that `Mode::TextInsert` is *near* exclusive — *"no **bare** key
        // resolves to an `Action` there"* — so a chord was never what that rule
        // was about, and these four fell out of it by accident. Meanwhile the top
        // bar's buttons for three of them were live the whole time and have
        // finished the session before acting since §15 D466, so the button and its
        // own chord disagreed: clicking Undo rewound the document, `Ctrl+Z` 30px
        // away did nothing.
        //
        // **Resolved here and finished at the seam.** `OndinApp::dispatch` calls
        // `finish_text_first` on the three arms that act on the document, which is
        // the same spelling the buttons take — so the chord and the button are one
        // behaviour rather than two that have to be kept equal. `Open` needs
        // nothing of its own: `go_to_dashboard` has always finished the session.
        //
        // ⚠️ **The guards are `normal_mode`'s, term for term**, including every
        // `!m.shift`: `Ctrl+Shift+O` and `Ctrl+Shift+Y` are unspent and must not
        // become silent second spellings here, §15 D710 having removed them
        // there. `Ctrl+Shift+S` is **not** one of D710's eight — `Save` carried
        // `!m.shift` already, and its comment is the precedent D710 generalised
        // from — so the guard here is that rule reaching a second mode rather
        // than a fix repeated. `Ctrl+Shift+Z` is the documented alternate Redo
        // and is the one arm that must *accept* Shift.
        // `ctrl_shift_is_not_a_second_spelling_of_ctrl` covers `normal_mode`'s
        // arms and not these, which is why they are written out rather than
        // shared — the two modes' lists are deliberately different lengths.
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

        out
    })
}

fn normal_mode(ctx: &egui::Context, nudge: NudgeStep) -> Vec<Action> {
    ctx.input(|i| {
        let m = i.modifiers;
        let cmd = m.command;
        // **Ctrl *without* Alt** — what every command below except the booleans
        // means, and what none of them used to say. The booleans are the keymap's
        // first `Ctrl+Alt` chords, and adding them exposed that: `Ctrl+Alt+S` fired
        // `Save` **and** `Subtract`, so every subtract also wrote the file. Silent in
        // the worst way, since the save succeeds and nothing looks wrong until the
        // artwork is. Pinned by `save_and_cut_are_not_booleans`.
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
        // 🚨 **`!m.shift` on the `Y` half** (§15 D710, `[S17-L1-05]`). The `Z`
        // half *needs* Shift and says so; `Y` did not exclude it, so
        // `Ctrl+Shift+Y` was a second, undocumented spelling of Redo. That is
        // the rule `Save` states four lines down and that this file states three
        // times without ever generalising — **eight arms carried `cmd_only`
        // alone**, which is the count
        // `ctrl_shift_is_not_a_second_spelling_of_ctrl`'s loop enumerates.
        on(
            cmd_only
                && ((m.shift && i.key_pressed(egui::Key::Z))
                    || (!m.shift && i.key_pressed(egui::Key::Y))),
            Action::Redo,
            &mut out,
        );
        // ⚠️ **`!m.shift` stays** even though nothing answers `Ctrl+Shift+S` any
        // more. Dropping it would make the freed chord a *second* spelling of
        // Save — silently, and only for the user who still has the old habit in
        // their fingers. A chord that does nothing is the honest state for one
        // whose action was removed, and it leaves the combination available.
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
        // `!m.shift` on both, like `Save` and `OpenSettings`: `Ctrl+Shift+N` and
        // `Ctrl+Shift+W` are unspent and should stay that way rather than becoming
        // silent second spellings of the two rows below. Nothing here needs `!alt`
        // said twice — `cmd_only` is that guard — but `Alt+W` is *align top* one
        // block down, which is why the Alt term is the one that cannot be dropped.
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
        // `cmd_only` and `!m.shift`, like `Save` above: `Ctrl+Shift+,` and
        // `Ctrl+Alt+,` stay unbound rather than becoming second doors onto the same
        // modal, which is what this keymap's Alt/Shift discipline is for.
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::Comma),
            Action::OpenSettings,
            &mut out,
        );
        // **`Ctrl+Shift+E` is deliberately unbound** since 2026-08-22, when
        // *Export as…* was removed. It is worth a comment rather than a gap because
        // the chord is *not* free in the way an unbound chord usually is: plain
        // `Ctrl+E` is *Flatten*, which dissolves the selection into a path, so the
        // `!m.shift` on that arm is the only thing standing between a hand that
        // still reaches for the old export chord and a destructive answer to it.
        // `a_stray_shift_on_ctrl_e_does_nothing_rather_than_flattening` is the pin.
        //
        // `Ctrl+Alt+E` — *Export all*, the repeat (§7). **Alt without Shift**, so
        // it cannot fire alongside the row above: this keymap tells `Ctrl` chords
        // apart from `Ctrl+Alt` ones by Alt alone, and a chord that states only
        // half of that fires on both — which is exactly how `Ctrl+Alt+S` once saved
        // the file on every boolean subtract. The Shift term is the second half of
        // the same guard, since `Ctrl+Alt+Shift+E` should be neither of these
        // rather than both.
        on(
            cmd && m.alt && !m.shift && i.key_pressed(egui::Key::E),
            Action::ExportAll,
            &mut out,
        );

        // --- clipboard / structure ---
        //
        // The clipboard trio never arrives as a key press. `egui_winit`
        // intercepts Ctrl+C/X/V (and their Insert-key synonyms) before the
        // `Key` event is built and emits `Event::Cut`/`Copy`/`Paste` instead,
        // returning early — so `key_pressed(Key::C)` is dead code in the real
        // app however right it looks. Reading the events is the only way to
        // see them, and it also picks up the platform clipboard keys for free.
        // **…but only the *press* is intercepted, and that is what makes an image
        // paste reachable at all.** `egui_winit`'s three guards sit inside
        // `if pressed`, and the `Event::Paste` it emits is pushed *only* when the
        // clipboard yields non-empty **text** — so `Ctrl+V` over a clipboard
        // holding a picture and nothing else produces no event whatsoever, and
        // the plan's trap ("read the clipboard on the frame the paste is
        // detected") had nothing to hang on — §15 D183 records that the trap
        // understated it, there being nothing to detect. The **release** falls past those
        // guards and arrives as an ordinary `Key { V, pressed: false }`, which is
        // the only witness the app gets. So a paste is taken from either signal
        // and `paste_image` decides which of the two payloads is there (§15 D183).
        //
        // **The trio owes `cmd_only` too, and this loop is where that was missed.**
        // `is_cut_command` tests `command && X` and never looks at Alt, so
        // `Ctrl+Alt+X` arrived here as an `Event::Cut` and **deleted** the selection
        // instead of excluding it, while Exclude itself was unreachable from the
        // keyboard (`docs/shortcuts.md` §L2) — one early return, two symptoms, the other
        // handled at the boolean loop below. Destructive and silent, because a cut
        // looks exactly like a boolean that consumed its operands. So an event is
        // read only when Alt is up, which is the rule every `on(…)` call already
        // states for itself; a `Ctrl+Alt` chord belongs to the booleans.
        //
        // **Shift is read here and Alt is not, and that asymmetry is the point.**
        // `Ctrl`+`Shift`+`V` is *paste in place* (`docs/shortcuts.md` §7): the same
        // event, discriminated on the frame's own `shift`, because the event carries
        // no modifiers and the key press is gone. It needs none of the widening the
        // two `Ctrl`+`Alt` clipboard rows do — those want the guard above turned into
        // a branch, which is what has kept them grouped as one job. This is not part
        // of that job and does not wait on it (§15 D248).
        //
        // **And that widening is now done, which is what the branch below is.** The
        // guard was `if !m.alt { … }` — one refusal covering all three events —
        // because `Ctrl`+`Alt`+`X` arriving as an `Event::Cut` **deleted** the
        // selection where Exclude was meant to fire. Alt now selects a *row* rather
        // than dropping the event, and the three go three different ways, which is
        // the whole of why they had to be one job:
        //
        // - `Ctrl`+`Alt`+`C` is **copy properties**, and the event is a sound witness
        //   for it: `egui_winit` emits `Event::Copy` on the chord unconditionally,
        //   reading no clipboard first.
        // - `Ctrl`+`Alt`+`V` is **paste properties**, and the event is *not* a sound
        //   witness for it — see the release arm below. Swallowed here so it cannot
        //   fall through to an ordinary paste, exactly as `Ctrl`+`Alt`+`X` is.
        // - `Ctrl`+`Alt`+`X` is still **nothing here**. It belongs to Exclude, which
        //   the boolean loop below reads off the key *release*; emitting anything for
        //   it would put the original destructive cut straight back.
        //
        // Order matters inside the `Paste` arms: Alt is tested before Shift, so a
        // hand still on Shift from a previous chord cannot turn a property paste
        // into a paste-in-place.
        for ev in &i.events {
            match ev {
                egui::Event::Copy if m.alt => out.push(Action::CopyProperties),
                // 🚨 **No Shift guard on copy or cut, deliberately, and
                // `[S17-L1-05]` is wrong to list them** (§15 D710). The finding
                // counts *"`Event::Copy` / `Event::Cut` with shift held →
                // `[Copy]` / `[Cut]`"* among its twelve silent doors.
                // `shift_makes_the_paste_event_a_paste_in_place` already pins
                // this and gives the reason: copy and cut are **not given a
                // Shift reading**, because `Ctrl+Shift+C` is the *text session's*
                // centre-align — and this is `normal_mode`, where that chord
                // does not exist. Adding a guard here would not free an unspent
                // combination; it would make `Ctrl+Shift+C` fail to copy outside
                // a text session, for a collision that is not in this function.
                // Tried; the test failed at `Copy under Shift: []`.
                egui::Event::Copy => out.push(Action::Copy),
                // The two arms Alt only ever *silences*, and they must stay that way.
                egui::Event::Cut if m.alt => {}
                egui::Event::Cut => out.push(Action::Cut),
                egui::Event::Paste(_) if m.alt => {}
                egui::Event::Paste(_) if m.shift => out.push(Action::PasteInPlace),
                egui::Event::Paste(_) => out.push(Action::Paste),
                _ => {}
            }
        }
        // **`Ctrl`+`Alt`+`V` off the release, and it is not symmetry with the copy
        // above — it is that the event cannot be trusted here.** `egui_winit` pushes
        // `Event::Paste` *only* when the clipboard yields non-empty text (§15 D183),
        // and a property paste has nothing to do with the OS clipboard at all. Read
        // from the event, the chord would work when a sentence happened to be copied
        // in another application and do nothing the rest of the time — a defect that
        // would look intermittent from the chair.
        //
        // So it is the same seam Exclude is read at: the swallow sits inside
        // `if pressed`, and the release falls past it. `cmd_only` excludes Alt, so
        // this cannot double up with [`Action::PasteRelease`] below.
        // 🚨 **No `!m.shift` here, deliberately, and `[S17-L1-05]` is wrong to
        // list it** (§15 D710). The finding calls `Ctrl+Alt+Shift+V` *"the one
        // `Ctrl+Alt` row of six without it"*. It is the one row that must not
        // have it: `a_property_paste_is_not_also_a_paste_in_place` exists to pin
        // exactly this, and its subject sentence is **"Alt beats Shift on a
        // paste, so a hand still resting on Shift cannot turn a property paste
        // into a paste-in-place."** Adding the guard does not free an unspent
        // chord — it makes `Ctrl+Alt+Shift+V` do **nothing**, which is the
        // hand-still-on-Shift case the rule is written for. Tried; the test
        // failed at `left: []`.
        if cmd && m.alt && i.key_released(egui::Key::V) {
            out.push(Action::PasteProperties);
        }
        // **The release is its own action, because it is a *weaker* witness than the
        // event and the difference cannot be resolved here.** Both signals fire for
        // one text paste — the event on the press frame, the release a frame or more
        // later — and a same-frame `pasted` flag was the guard until 2026-08-18, which
        // could only ever see the case where the two land together. It read as
        // covering the real one; a two-frame probe said `press=[Paste]
        // release=[Paste]` (§15 D219).
        //
        // It cannot be fixed with state here: `resolve` is a pure function of the
        // keymap and the mode, and there is nothing to key a window on either, since
        // `egui_winit` swallows the V *press* outright — `key_down(V)` is false on the
        // frame the event arrives, so "while the key is held" does not exist as a
        // question. `Action::PasteRelease` hands the discrimination to the one place
        // that can make it, on the one fact that settles it (`OndinApp::dispatch`).
        // ⚠️ **The ninth arm with no Shift term, and it is the seam rather than a
        // door** (§15 D710). `arch-scribe` raised it while writing that entry —
        // eight arms took the guard and this one did not — so the answer is
        // written here rather than left to be re-derived. `Ctrl+Shift+V`'s
        // release does push `PasteRelease`, and `dispatch` acts on it **only
        // when `system_clipboard_text().is_none()`**, which is exactly the case
        // in which `Event::Paste` did not fire. So it cannot double with the
        // in-place paste. And in that one case `paste` and `paste_in_place` do
        // the *same* thing — both open on `paste_image(None)`, and an image is
        // what a clipboard with no text usually holds — so the plain call is not
        // a wrong answer for a shifted chord either. **Checked, both arms read.**
        if cmd_only && i.key_released(egui::Key::V) {
            out.push(Action::PasteRelease);
        }
        // ⚠️ **`Ctrl+Shift+D` was a document edit** (§15 D710). `docs/shortcuts.md`
        // §11 binds `Shift+D` to *reverse subpath direction* and leaves
        // `Ctrl+Shift+D` unbound, so a hand still on Shift from the neighbouring
        // chord duplicated the selection and spent an undo step — the exact
        // hand-slips-onto-a-neighbour hazard §8's `Ctrl+Shift+E` row is guarded
        // against by name.
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
        // Figma's chord for placing an image, and a chord rather than a tool
        // letter because it opens a file dialog instead of arming a drawing mode —
        // every letter in `docs/shortcuts.md` §1 is a persistent tool, and this is the
        // one rail tool that is not. Plain `K` is the Scale tool, which `plain`
        // keeps clear of every modified chord. Illustrator's `Ctrl+Shift+P` is
        // deliberately not aliased: one binding, and the Figma one.
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

        // *Use as mask*, on Figma's chord. **`M` was completely unspent** — not
        // one binding in this file or in `shortcuts.md` used the key in any
        // combination — so this is the rare case where the chord costs nothing and
        // there is no collision to argue about. Alt-modified like the booleans
        // below, which is the company it keeps: `Ctrl+Alt+U/S/I/X` all make or
        // change a container, and with the group-wrapping arm so does this.
        on(
            cmd && m.alt && !m.shift && i.key_pressed(egui::Key::M),
            Action::Mask,
            &mut out,
        );

        // Flatten, on Figma's chord. `Ctrl+E` is free — plain `E` is the ellipse
        // tool, which `plain` keeps clear of every modified chord — and it belongs in
        // this group rather than with the booleans below because it is not one of
        // them: `Ctrl+Alt+U/S/I/X` all *make or change* a container, and this one
        // dissolves whatever is selected into a single path.
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::E),
            Action::Flatten,
            &mut out,
        );

        // --- booleans (Figma's chords) ---
        //
        // `Ctrl+Alt+U/S/I/X`. All four are Alt-modified, which is what keeps them
        // clear of `Ctrl+S` (save) and `Ctrl+X` (cut) — the two they would otherwise
        // collide with, and the collision would be silent for `Ctrl+S`: a save that
        // also united the selection.
        //
        // **`X` is read from the *release*, the other three from the press**, and
        // being clear of `Ctrl+X` is not enough to make it work: `egui_winit` never
        // builds a `Key` event for a `Ctrl+X` press at all, whatever else is held,
        // because `is_cut_command` ignores Alt and it returns after emitting
        // `Event::Cut`. So `key_pressed(X)` reads correctly and fires never, and
        // Exclude had no keyboard route. The release falls past that guard — the
        // guard sits inside `if pressed` — which is the same seam `Ctrl+V` is read
        // at above for an image paste (§15 D183). Its other half, the cut that fired
        // in Exclude's place, is the `!m.alt` guard on the event loop above.
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
        // Each direction needs **both** spellings of its key, and this is the
        // reason: `Shift`+`[` is `{`, and egui reports that as its own
        // `Key::OpenCurlyBracket` rather than as a shifted `OpenBracket`. So the
        // bring-to-front and send-to-back chords read perfectly and fired never
        // — D17's failure mode exactly, and invisible for the same reason: the
        // code names the key a human would name.
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
            // ⚠️ **`|| m.alt`, which every other `Ctrl` arm in this function has
            // had since §15 D207 and this one did not** (§15 D545).
            // `[S17-L1-03]`: this was the single site in the file that opened
            // `if !cmd` rather than testing `cmd_only = cmd && !m.alt`, so
            // **eight `Ctrl+Alt` spellings restacked the selection and committed
            // an undo step** for a chord `docs/shortcuts.md` binds to nothing —
            // `Ctrl+Alt+]`, `[`, `}`, `{` and each with Shift. Measured with
            // every other `Ctrl`-modified chord in the function as the in-run
            // control: `Ctrl+Alt+S` gives `[Boolean(Subtract)]` alone and
            // `Ctrl+Alt+{Z,A,D,O,0,1,2}` all give `[]`.
            //
            // **Nothing collides today and that is exactly the state D207
            // describes as the collision waiting to return**: *"`cmd_only` is not
            // a verbose spelling of `cmd` — collapsing it back is how the
            // collision returns, destructively and without a symptom."*
            // `Ctrl+Alt+S` once fired *Save* and *Subtract* together for
            // precisely this reason. D38 is the bracket entry and is about the
            // `{`/`}` glyph-under-Shift bug only.
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
        // 🚨 **`ZoomIn` is the one arm here that must NOT exclude Shift, and
        // `[S17-L1-05]` gets it wrong** (§15 D710). The finding lists
        // `Ctrl+Shift+=` among its twelve silent second doors; it is the
        // *primary* spelling of `Ctrl++`, because on a US layout `+` is
        // `Shift+=` and egui reports the **logical** key — so `Key::Plus` is
        // unreachable without Shift. `docs/shortcuts.md` §3 says so in the row
        // itself, *"both spellings already accepted"*, and §0's L1 worked
        // example is this same mechanism for `Shift+1` → `!`. A blanket
        // `!m.shift` here would have removed zoom-in outright for anyone who
        // types the chord the way it is written.
        //
        // So the guard goes on the **unshifted** spelling only: `Equals` needs
        // no Shift and must not accept one, `Plus` cannot be produced without.
        on(
            cmd_only
                && (i.key_pressed(egui::Key::Plus)
                    || (!m.shift && i.key_pressed(egui::Key::Equals))),
            Action::ZoomIn,
            &mut out,
        );
        // `-` needs no Shift to reach and §3 records no shifted spelling, so
        // this one takes the ordinary guard.
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
        // **Both spellings of the digit, for the reason the brackets above give.**
        // `Shift`+`1` is `!`, and egui reports the *logical* key — `key_pressed`
        // matches on `Event::Key`'s `key` and never looks at `physical_key`, which is
        // where the `Num1` would be. So `Key::Num1` under Shift read perfectly and
        // fired never, and Figma's zoom-to-fit chord has never worked. `Num1` stays
        // in the pair rather than being swapped out, because it is the spelling a
        // layout that passes the unshifted digit through would send.
        on(
            plain
                && m.shift
                && (i.key_pressed(egui::Key::Exclamationmark) || i.key_pressed(egui::Key::Num1)),
            Action::ZoomFit,
            &mut out,
        );
        // `Shift+0` and `Shift+2` — Figma's — beside Sketch's `Ctrl+0`/`Ctrl+1`
        // above, plus Sketch's own `Ctrl+2`.
        //
        // **Neither of these needs the two-spellings treatment `Shift+1` above
        // needs**, and the asymmetry is `egui::Key`'s rather than ours: `Shift+1`
        // is `!`, which *has* a variant, so the logical key wins and `Num1` never
        // arrives. `Shift+0` is `)` and `Shift+2` is `@`, and egui has a variant
        // for neither — so `logical_key.or(physical_key)` falls through to the
        // physical digit and the obvious spelling is the only one there is (§0).
        // A UK layout sends `"` for `Shift+2`, which also has no variant and also
        // falls back, so this holds off-US too.
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
        // The guides, on Illustrator's pair (`docs/shortcuts.md` §6). **The two are
        // told apart by Alt and must therefore be mutually exclusive**, which is
        // what `cmd_only` buys on the first arm: without it `Ctrl+Alt+;` would fire
        // both, hiding the guides *and* locking them in one press — the
        // `Ctrl+Alt+S` failure §L2's neighbours describe, in its quietest form.
        //
        // `Key::Semicolon` is the right spelling for both because neither chord
        // involves Shift: `;` reaches egui as itself, where the shifted `:` would
        // have arrived as `Key::Colon` (§0). That is `Ctrl+Shift+;` — *Snap to
        // guides* — which is a third chord on this key and is not this feature's.
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

        // The grid and the three snapping switches, which are Illustrator's set
        // — Figma has almost nothing here, so this whole group follows the oldest
        // and deepest of the conventions rather than being invented
        // (`docs/shortcuts.md` §6).
        //
        // **Three of these are shifted punctuation and two of the three need
        // both spellings.** `Ctrl+Shift+;` is `:` and `Ctrl+Shift+\` is `|`, and
        // `egui::Key` has a variant for each — so the logical key wins and the
        // plain spelling never arrives, exactly as it did not for `{`/`}` and `!`.
        // `Ctrl+Shift+'` is `"`, which has **no** variant, so it falls back to the
        // physical `Quote` and the single spelling is right. Accepting both is
        // correct on every layout; accepting one is correct on some (§0).
        //
        // The shifted variant is accepted unconditionally because it cannot be
        // produced *without* Shift; the plain spelling is read against the
        // modifier, for a layout that passes the unshifted key through. That is
        // the bracket-restack block's shape, one group up.
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
        // Illustrator's Smart Guides chord. Clear of `Ctrl+Alt+U` (union) by
        // `cmd_only`, and it means underline in `Mode::TextInsert` — a different
        // mode, so not a collision, but the first chord in this keymap whose
        // meaning is genuinely carried by `Mode` (`docs/shortcuts.md` §10).
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
        // present mode, which already *is* "hide everything and restore it on the
        // way out"; the other two are this app's, for the two pieces of chrome it
        // has that are worth toggling one at a time.
        on(
            cmd_only && !m.shift && i.key_pressed(egui::Key::Backslash),
            Action::ToggleView(ViewSwitch::Present),
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
        // `Shift+D` for *direction*, in the same plain-plus-Shift shape `Shift+R`
        // established. Nothing else claims it: `Ctrl+D` is Duplicate, which is
        // `cmd_only`, and plain `D` is not a tool letter — the shape tools stop at
        // `L`, and a *plain* letter is reserved for switching tool by convention
        // (`single-key tool switches` below), which is the reason this is a chord
        // and not a bare `D`. No app this borrows from has a shortcut for it at all
        // — Illustrator and Affinity both bury it in a menu — so there is no parity
        // to match, only the app's own pattern.
        on(
            plain && m.shift && i.key_pressed(egui::Key::D),
            Action::ReverseSubpaths,
            &mut out,
        );

        // --- layer state (Figma and Sketch, which agree here) ---
        //
        // `Ctrl+Shift+H` and `Ctrl+Shift+L`. Neither collides: plain `H` is the
        // Hand tool and plain `L` the Line tool, both gated on `plain`, and the
        // `Ctrl+Shift+L` that means *align left* is a `Mode::TextInsert` binding
        // (`docs/shortcuts.md` §10) — a different mode, and the third chord after
        // `Ctrl+U` and `Alt`+arrows whose meaning nothing but `Mode` carries.
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
        // Rename, on Figma's chord with the Windows convention as an alias.
        // **Not rulers** — see [`Action::Rename`], where the reason is recorded
        // so it is not re-argued from the Adobe side.
        on(
            (cmd_only && !m.shift && i.key_pressed(egui::Key::R))
                || (plain && i.key_pressed(egui::Key::F2)),
            Action::Rename,
            &mut out,
        );
        // Flip, on Figma's pair. `plain` keeps these off `Alt+H`/`Alt+V`
        // (align centres) below, and `!m.shift` on the tool block keeps them off
        // the Hand and Select letters.
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

        // --- align and distribute (Alt, without Ctrl) ---
        //
        // The six aligns are Figma exact; the two distributes are ours, because
        // Figma's `Ctrl+Alt+H`/`V` sit in this keymap's boolean namespace
        // (`docs/shortcuts.md` §5). `Alt+Shift` is what separates them, which is why
        // the aligns are `!m.shift` rather than shift-agnostic.
        //
        // A bare `Alt` chord is safe here — there is no system menu bar to poke —
        // and `!cmd` is what keeps `Alt+S` clear of `Ctrl+Alt+S` (subtract), the
        // one pair in this block that would otherwise both fire.
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
        // **Tab steps the point selection along a path**, Shift+Tab back — and it
        // is resolved here, in the keymap, so the focus guard above covers it:
        // Tab is egui's own focus-cycling key, and a rename field or a numeric
        // field has to keep it. `dispatch` does nothing with this unless the node
        // tool has a path, so outside point editing Tab reaches egui untouched.
        on(
            plain && i.key_pressed(egui::Key::Tab),
            Action::StepPoint(!m.shift),
            &mut out,
        );

        // --- arrows: nudge bare, resize with Ctrl (Shift for the coarse step) ---
        //
        // **One table, because the two actions want the same four signed pairs.** A
        // nudge moves the box in the direction of the arrow; a resize holds the box's
        // top-left and moves the opposite edge, so it *also* grows in the direction of
        // the arrow. Written as two loops the shared signs would be two tables that
        // happen to agree, and a later edit to one of them would be silent.
        //
        // **`Ctrl` was free because the nudge is gated on `!cmd`**, so that chord has
        // always fallen through to nothing — and nothing in `docs/shortcuts.md` claims
        // it on any of the four arrows. `cmd_only` rather than `cmd`, so
        // `Ctrl`+`Alt`+arrow stays unclaimed too: `Alt`+arrow is already the nudge (the
        // bare arm is not gated on `!alt`), and a chord that means one thing with Ctrl
        // and another without it is the kind of pair that gets bound twice later.
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
        // Universal across Figma, Photoshop and Illustrator, and free here
        // because nothing plain-digit was bound. `!m.shift` is what keeps them
        // off the zoom chords above — `Shift+0/1/2` — and off `Shift`+`3`…`9`,
        // which are reserved for blend modes and must stay unclaimed rather than
        // fall through to an opacity they were never meant to set.
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
                // **`O` is the ellipse's published letter and `E` is an alias.**
                // Figma and Sketch both say `O`; `E` costs nothing to keep, has
                // hands that already know it, and leaves `Ctrl+E` (flatten)
                // alone. The rail's tooltip shows `O` only — an alias is not a
                // second published binding.
                (egui::Key::O, Tool::Ellipse),
                (egui::Key::E, Tool::Ellipse),
                // G and S: Figma has no polygon/star shortcut to match, and the
                // obvious letters are taken (P is the pen, T is text).
                (egui::Key::G, Tool::Polygon),
                (egui::Key::S, Tool::Star),
                (egui::Key::L, Tool::Line),
                (egui::Key::P, Tool::Pen),
                // **Illustrator's and Sketch's direct-selection letter**, which
                // is the right reason for it. This comment used to claim `A` was
                // "Figma's letter for the node/vector-edit tool": Figma binds `A`
                // to the **Frame** tool alongside `F` and has no letter for vector
                // editing at all. The binding was right and the justification was
                // not, which is the shape of thing that gets a good binding
                // "corrected" later.
                (egui::Key::A, Tool::Node),
                (egui::Key::T, Tool::Text),
                // **`C` is free again.** It was cropping's, on the argument that a
                // built feature outranks an inert rail button; image editing
                // replaced the crop tool and deliberately has no letter (§15
                // D268), so the mnemonic goes back to the comment tool that will
                // want it. Nothing is bound in its place: `Enter` on a selected
                // picture is the keyboard door, and it is a door that cannot be
                // opened onto nothing.
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

    /// Drive `resolve` against a synthetic frame of egui input, with the default
    /// nudge — which is what every chord but the arrows is indifferent to.
    pub(super) fn actions(
        mode: Mode,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) -> Vec<Action> {
        actions_nudging(NudgeStep::default(), mode, events, modifiers)
    }

    /// `actions` with the nudge step spelled out, for the two cases that are
    /// about it.
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

    /// The key coming back up. Worth a helper of its own because for `C`, `X` and
    /// `V` it is the **only** event the real app ever sees — `egui_winit`'s three
    /// clipboard guards sit inside `if pressed`, so a synthetic press for one of
    /// those three tests a signal that is never delivered.
    pub(super) fn release(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers,
        }
    }

    /// The clipboard trio arrives as `Event::Cut`/`Copy`/`Paste`, **not** as
    /// key presses: `egui_winit` swallows Ctrl+C/X/V and re-emits them in that
    /// form. Watching for the keys instead is the bug this pins — it reads
    /// correctly and fires never.
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
        // The keys themselves must stay inert, or a platform that *does* let
        // them through would fire the action twice.
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

    /// `Ctrl+N` and `Ctrl+W` — the two rows `shortcuts.md` §8 carried as *waiting
    /// on the dashboard* from before there was one.
    ///
    /// ⚠️ **The `Alt+W` assertion is the one that is not about the new chords.**
    /// *Align top* is `Alt+W`, one block below them in the same function, and
    /// nothing tells the two apart but Alt — which is the discipline `Ctrl+Alt+S`
    /// once broke by firing *Save* on every boolean subtract. `cmd_only` carries
    /// the `!alt` term for both new arms, and this is what proves it does.
    ///
    /// The `Ctrl+Shift+` pair asserts the **empty** result, like
    /// `save_takes_the_bare_chord_and_the_shifted_one_is_free` above: an unspent
    /// combination should stay unspent rather than become a silent second
    /// spelling.
    ///
    /// Two flips, both run, both at the predicted site. Dropping `!m.shift` from
    /// the `N` arm fails at *"Ctrl+Shift+N is unspent and stays unspent"*.
    /// Weakening `W`'s `cmd_only` to a bare `cmd` fails at *"Ctrl+Alt+W belongs to
    /// neither row"* — and note what it does **not** fail: the `Alt+W` assertion
    /// above stays green, because align-top needs `!cmd` and is untouched by a
    /// chord that has gained Ctrl. A test asserting only that align still works
    /// would have passed the broken build.
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

    /// **`Ctrl`+`Shift`+`V` is a second paste, told apart by the frame's modifiers
    /// rather than by anything on the event** (§15 D248).
    ///
    /// `egui_winit`'s `is_paste_command` tests `command && Key::V` and never examines
    /// Shift, so the press is swallowed exactly as a plain one is and `Event::Paste`
    /// is the whole signal. It carries no modifiers of its own, which is why the
    /// discrimination has to be read here, in the frame the event arrived in.
    ///
    /// ⚠️ Three flips. Dropping the `m.shift` arm makes `Ctrl`+`Shift`+`V` an
    /// ordinary paste — the state before this, and the one that reads as working
    /// because something does happen. Matching on `m.shift` *before* testing Alt (or
    /// hoisting the arm above the `!m.alt` guard) makes `Ctrl`+`Alt`+`Shift`+`V`
    /// paste, which is the shape of §L2's live bug — a chord meant for something else
    /// arriving as a clipboard verb. And inverting it makes plain `Ctrl`+`V` the
    /// in-place one, which no assertion below would catch without the first case.
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
        // Alt still owns the whole trio's exclusion: `Ctrl`+`Alt`+`Shift`+`V` is not
        // a paste of either kind, whatever else it may come to mean.
        assert!(
            actions(
                Mode::Normal,
                vec![egui::Event::Paste("x".into())],
                alt_shift
            )
            .is_empty(),
            "Alt excludes the trio however it is combined"
        );
        // Copy and cut are *not* given a Shift reading — `Ctrl`+`Shift`+`C` is the
        // text session's centre-align (§L2), and a second meaning here would collide
        // with it the moment a session is not open.
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

        // ⚠️ **And `Alt` excludes all eight spellings** (§15 D545,
        // `[S17-L1-03]`). This block was the **single site in the file** that
        // opened `if !cmd` rather than `cmd && !m.alt`, so `Ctrl+Alt+]` restacked
        // the selection and spent an undo step for a chord `shortcuts.md` binds
        // to nothing. Nothing collides today, and §15 D207 says that is precisely
        // the state the collision returns from: *"`cmd_only` is not a verbose
        // spelling of `cmd` — collapsing it back is how the collision returns,
        // destructively and without a symptom."*
        //
        // **All four keys × Shift**, because the curly forms reach a different
        // arm of the same loop and a fix applied to one `if` covers both only
        // because the guard is at the top — which is the thing worth pinning.
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
        // The control, in the shape `save_and_cut_are_not_booleans` uses: a chord
        // that *is* a deliberate `Ctrl+Alt` row still fires, so this is about the
        // bracket block and not about Alt suppressing everything.
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::S, cmd_alt)], cmd_alt),
            vec![Action::Boolean(ondin_core::BoolOp::Subtract)],
            "control: Ctrl+Alt+S is a real row and still resolves"
        );
    }

    /// `Shift`+`[` is `{`, and egui gives that its own `Key`. Watching only for
    /// `OpenBracket` therefore reads correctly and fires never — the bug this
    /// pins, and the reason both spellings are accepted.
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
        // A curly key means "all the way" whatever the reported modifiers, since
        // it cannot be produced without Shift in the first place.
        let cmd = egui::Modifiers::COMMAND;
        assert_eq!(
            actions(
                Mode::Normal,
                vec![key(egui::Key::CloseCurlyBracket, cmd)],
                cmd
            ),
            vec![Action::Restack(build::ZMove::Front)]
        );
        // And it is still a command only with Ctrl: `{` on its own is a
        // character.
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

    /// **`Shift+1` had never once zoomed to fit** — the same failure as the curly
    /// brackets above, one key along: `Shift`+`1` is `!`, egui reports the logical
    /// key, and `key_pressed(Num1)` therefore reads perfectly and fires never.
    /// `docs/shortcuts.md` §L1.
    ///
    /// `Ctrl+1` is the control here, and it is the reason the bug survived review:
    /// the *unshifted* chord on the very same digit works, so the pair reads as one
    /// binding written twice rather than as one binding and one dead line.
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
        // Ctrl+1 is the same action and was never broken — it carries no Shift, so
        // the digit arrives as itself.
        let cmd = egui::Modifiers::COMMAND;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Num1, cmd)], cmd),
            vec![Action::ZoomFit]
        );
        // And `!` is a character, not a command, without the modifier.
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

    /// Shift+R toggles the rulers and plain R still picks the Rect tool. The
    /// two share a letter, and the only thing keeping them apart is the shift
    /// gate on the tool block — which is exactly the sort of thing a later edit
    /// removes by accident.
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
        // And it is a normal-mode command, so "R" typed into a text node is an
        // R rather than a ruler toggle.
        assert!(actions(Mode::TextInsert, vec![key(egui::Key::R, shift)], shift).is_empty());
    }

    /// `Shift+D` reverses a path's direction, and it has to stay clear of the two
    /// things a `D` already means: `Ctrl+D` duplicates, and the plain letter is
    /// reserved for a tool switch. Duplicate is the dangerous one — a reverse that
    /// also duplicated the layer would be `Ctrl+Alt+S`'s silent double-fire again
    /// (`save_and_cut_are_not_booleans`), from the other side.
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
        // Plain D is nobody's, and must stay that way: the tool block is plain
        // letters, so a `D` that reached this action would be a tool switch away
        // from meaning two things.
        assert!(
            actions(Mode::Normal, vec![key(egui::Key::D, none)], none).is_empty(),
            "plain D reached an action"
        );
        // And it is a normal-mode command, so a typed D stays a D.
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
        // `K` is Figma's letter for the Scale tool, and it has to reach *only* that:
        // a letter bound twice would fire two actions from one press, and the second
        // would silently win.
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::K, none)], none),
            vec![Action::ChooseTool(Tool::Scale)]
        );
        // `A` is the node tool, and it is the one tool letter that also appears in
        // a chord — `Ctrl+A` is Select All. The binding is `plain`, so the two
        // cannot both fire; asserting the chord as well as the letter is what
        // would catch a `plain` that stopped excluding the modifier.
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

    /// Enter is the counterpart to Escape — into point editing and back out —
    /// and it has to stay out of the way of the two places Enter already means
    /// something: a text node being typed into, and any chrome field with focus
    /// (`resolve` returns nothing at all for the second, which is why only the
    /// first is asserted here).
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
        // Plain only, so a chord that happens to include Enter cannot also step
        // into the selection.
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

    /// **`Ctrl`+arrows resize, and are carved out of the arrows without taking
    /// anything from them.** Four claims, and the last two are the ones that would go
    /// wrong quietly:
    ///
    /// - **all four directions, with the same signs the nudge uses** — the box is
    ///   held by its top-left, so the edge that moves is the one the arrow points at.
    ///   ⚠️ Asserted against the *nudge's* own vector for each key rather than
    ///   against written-out numbers: the two now share one table, so what has to
    ///   hold is that they agree, and a test that spelled the numbers twice would
    ///   pass while the table drifted from the thing it is a table of.
    /// - `Shift` reaches it, and reaches it through the same `NudgeStep` the bare
    ///   arrows use, so a user who has retuned the nudge has retuned this too;
    /// - a bare arrow is **still only a nudge** — the shared arm must not emit both;
    /// - and `Ctrl`+`Alt`+arrow stays unclaimed.
    ///
    /// Flip-checked twice. Gating on `cmd` rather than `cmd_only`, the spelling that
    /// reads as equivalent, lets `Ctrl`+`Alt`+`→` resolve to a size step — the last
    /// assertion catches it. And swapping the two arms of the `if` so a bare arrow
    /// resizes fails the nudge assertion, which is the one that stops a merged table
    /// from quietly merging the actions with it.
    #[test]
    fn ctrl_arrows_size_the_selection_and_leave_the_nudge_alone() {
        let none = egui::Modifiers::NONE;
        let ctrl = egui::Modifiers::COMMAND;
        let ctrl_shift = egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT);
        let ctrl_alt = egui::Modifiers::COMMAND.plus(egui::Modifiers::ALT);
        let at = |k, m| actions(Mode::Normal, vec![key(k, m)], m);

        // Every arrow, against the nudge it shares its table with: same key, same
        // vector, different verb. `→` widens and `←` narrows; `↓` makes it taller and
        // `↑` shorter, because the top-left is held.
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
        // Spelled out once, so the signs are readable and not only self-consistent.
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

    /// ⚠️ **The two distances must not be swapped on the way in**, which is the
    /// whole reason `NudgeStep` is a struct — and the reason this asserts on a
    /// *lopsided* pair. With `{ small: 1, large: 10 }` the plain arrow and the
    /// Shift arrow move by numbers that could each have come from either field
    /// under some other bug; `{ small: 0.5, large: 64 }` is a pair where the wrong
    /// field is unmistakable.
    ///
    /// ⚠️ **The small step read `0.5` until `NudgeStep::step` started
    /// clamping**, and the fixture had to move rather than the clamp:
    /// `NudgeStep::MIN` is 1, so 0.5 is now exactly the out-of-range value the
    /// clamp exists for, and this test would have been asserting the clamp
    /// instead of the swap it is named for. 2 keeps the pair lopsided and keeps
    /// both ends legal.
    ///
    /// Flip-checked by reversing the two arms of `NudgeStep::step`: both
    /// assertions fail, the plain arrow reporting 64 and the Shift arrow 2.
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

    /// A stored nudge step outside the Settings field's range is clamped **at
    /// the reader**, so the arrow key and the field agree.
    ///
    /// ⚠️ **`prefs.json` is untrusted input and the design already says so** —
    /// §15 D349, about this same file, for the layers panel's stored width:
    /// *"a plain file that can be hand-edited, copied between machines or
    /// written by a future version"*. That number is clamped at its reader and
    /// has a test named for it. This was the second such number and had neither.
    ///
    /// **And the bad value is one this project shipped.** The Settings field was
    /// `Scrub::fine(_, 2).range(0.01..=1000)` until somebody reported *"nudging
    /// by 0.1px is crazy"*; `Prefs` has no version and no migration, so an
    /// upgraded install reads the stored `0.1` verbatim. Both clamps then lived
    /// on the Settings *draft* — the copy Cancel and Escape throw away — so the
    /// field painted **1** over a document where an arrow key moved 0.1px, and
    /// *Save changes* lit on a form nobody had edited.
    ///
    /// **The in-range control is what stops this asserting that nudging is
    /// broken**: 4px must come through as 4px.
    ///
    /// Flip-check, run: dropping the `.round().clamp(…)` from `NudgeStep::step`
    /// fails on the first assertion, reporting `Nudge((0.1, 0.0))` — the symptom
    /// itself.
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

        // What an install upgraded from the old field range has on disk.
        assert_eq!(
            arrow(NudgeStep {
                small: 0.1,
                large: 10.0
            }),
            vec![Action::Nudge(Vec2::new(NudgeStep::MIN, 0.0))],
            "an arrow key may not move less than the field's own floor"
        );
        // And the other end, plus a fraction, plus a value no field could show.
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

    /// ⚠️ **The shift arm is the assertion, and it asserts a *nothing*.**
    /// `Ctrl+Shift+S` was Save As until the library removed the idea of choosing
    /// where a file goes; the chord is now unbound, and the failure worth
    /// guarding is that it quietly becomes a second Save — which is what
    /// dropping `!m.shift` from the guard does, invisibly, for anyone whose
    /// fingers still reach for it.
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

    /// **`Ctrl+E` flattens and `Ctrl+Shift+E` does nothing**, which is a sharper
    /// claim than the one this test made until 2026-08-22 and guards a worse
    /// failure.
    ///
    /// It used to assert that Shift reached *Export as…*, with the load-bearing
    /// half being that *Flatten* was **absent** from the shifted answer — a
    /// *Flatten* written without its `!m.shift` would have fired alongside the
    /// export and dissolved the selection into a path behind a file dialog, unseen
    /// until the dialog closed. **Removing the export makes that guard the only
    /// thing left**, and the hand that reaches for `Ctrl+Shift+E` out of habit is
    /// now reaching for a chord with nothing on it. If `!m.shift` were ever dropped
    /// as tidying — it reads like a redundant guard once the chord it excluded is
    /// gone — that habit would flatten the selection instead.
    ///
    /// ⚠️ Flipped by deleting `!m.shift` from *Flatten*'s arm, which is exactly the
    /// tidying this is here to stop: the first assertion still passes and the
    /// second fails with `[Flatten]`.
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
        // And plain `E` is still the ellipse tool, which is what keeps `Ctrl+E`
        // free in the first place.
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

    /// **`Ctrl+Alt+M` masks, and nothing else in the app answers to `M`.**
    ///
    /// The second half is the assertion worth having. `M` was completely unspent
    /// when this was bound — no tool letter, no other chord — so the risk is not a
    /// collision that exists today but one introduced later by someone giving `M` a
    /// tool without noticing this. Both the bare key and the chord are checked, so
    /// that day fails here rather than on a user's machine.
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
        // Alt is what separates it from anything `Ctrl+M` might become.
        let cmd = egui::Modifiers::COMMAND;
        assert!(
            actions(Mode::Normal, vec![key(egui::Key::M, cmd)], cmd).is_empty(),
            "Ctrl+M is not an alias — one chord, and the Figma one"
        );
    }

    /// **`K` and `Ctrl+Shift+K` are two different things**, and the letter is the
    /// one at risk: every tool letter is a *plain* key, so a chord built on one has
    /// to not also arm its tool. Both directions are asserted, because the failure
    /// is silent in either — placing an image while switching to Scale, or a
    /// dialog opening every time someone reaches for the Scale tool.
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

    /// **A paste is taken from the key *release* as well as from the event, and
    /// never from both.**
    ///
    /// The release is the only witness an image paste gets: `egui_winit`'s three
    /// clipboard guards sit inside `if pressed` and it pushes `Event::Paste` only
    /// when the clipboard yields non-empty **text**, so `Ctrl+V` over a clipboard
    /// holding a picture and nothing else emits nothing at all. The release falls
    /// past those guards as an ordinary `Key { V, pressed: false }`.
    ///
    /// **Half of that is read from `egui-winit`'s source and cannot be tested
    /// here**, because it is the backend→egui direction and `run_ui` only shows
    /// egui→backend. It was **confirmed by hand on 2026-08-11** — a copied image
    /// pasted into the canvas and placed a layer — so the release really does
    /// arrive; treat it as a fact rather than a reading, and if this ever stops
    /// working suspect an `egui-winit` upgrade moving those guards outside
    /// `if pressed`. What this pins is our half: that the release maps to a paste,
    /// and that a *text* paste — which emits the event *and* the release, one
    /// frame apart — does not paste twice.
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
        // The image case: no event, just the release egui forgot to swallow. It is a
        // paste of its own kind, because at this layer nothing knows yet whether the
        // event also fired — see `Action::PasteRelease`.
        assert_eq!(
            actions(Mode::Normal, vec![release(egui::Key::V)], cmd),
            vec![Action::PasteRelease],
            "a Ctrl+V release must be read as a paste, or an image-only \
             clipboard is unreachable"
        );
        // Both signals in one frame: two *distinguishable* actions, not one. The old
        // shape collapsed them here and that is precisely what hid the two-frame case
        // below — the guard could only ever see this one.
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

    /// **One text paste resolves twice, a frame apart, and the two must be tellable
    /// apart** — the bug the previous shape of this loop could not see (§15 D219).
    ///
    /// `Ctrl+V` over text emits `Event::Paste` on the press frame and the V release a
    /// frame or more later. The guard was a `let mut pasted` **local**, so it only ever
    /// covered the two landing together — and the case it claimed to cover, "one frame
    /// apart", was the real one. A probe over two frames of one `Context` said
    /// `press=[Paste] release=[Paste]`: two pastes, two undo steps, the second copy
    /// exactly on top of the first.
    ///
    /// **Two frames of one `Context`, not two `actions` calls.** The distinction is the
    /// test: separate contexts would prove nothing about frame-to-frame state, which is
    /// exactly what was missing. `Modifiers::COMMAND` on the release frame is the
    /// reachability condition too — Ctrl still down when V comes up, which is why the
    /// bug was intermittent rather than constant.
    ///
    /// What this pins is that the two signals stay **distinct**. Which of them acts is
    /// `dispatch`'s call, on whether the clipboard holds text, and needs an `OndinApp`
    /// — read-verified.
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
        // Ctrl+V is paste, not "the select tool" — and the paste arrives as an
        // event beside the (suppressed) key, so both halves are checked here.
        let cmd = egui::Modifiers::COMMAND;
        let got = actions(
            Mode::Normal,
            vec![key(egui::Key::V, cmd), egui::Event::Paste("x".into())],
            cmd,
        );
        assert_eq!(got, vec![Action::Paste]);
    }

    /// Insert mode is still exclusive once the trio comes in as events: the
    /// canvas text editor owns all three, so the keymap must not also see them.
    ///
    /// **This is the reason `canvas::text_mode_input` had to grow `Copy` and `Cut`
    /// arms of its own** (§15 D217). The keymap returning nothing here is right —
    /// the events address the buffer — but for as long as the session's loop had
    /// only a `Paste` arm, "the editor owns them" described an owner that was not
    /// reading two of the three, and `Ctrl+C` in a session did nothing at all.
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

    /// **The `!m.alt` guard is a branch now, and the three events go three
    /// different ways** (`docs/shortcuts.md` §7's two deferred rows).
    ///
    /// The guard existed because `Ctrl+Alt+X` arrived as an `Event::Cut` and
    /// **deleted** the selection where Exclude was asked for. Widening it is what
    /// the property rows waited on, and what makes the widening safe is that Alt
    /// now picks a row rather than being dropped wholesale — so the test has to
    /// assert the arm that still refuses as hard as the two that answer.
    ///
    /// ⚠️ Flipped against the spelling somebody would actually reach for — one arm
    /// `Event::Cut if m.alt => out.push(Action::Cut)`, or simply deleting the guard
    /// — where the third assertion fails and the other three pass.
    #[test]
    fn alt_turns_the_clipboard_events_into_the_property_rows_except_the_cut() {
        assert_eq!(
            actions(Mode::Normal, vec![egui::Event::Copy], cmd_alt()),
            vec![Action::CopyProperties],
            "Ctrl+Alt+C copies properties and must not also copy the layer"
        );
        // The release, not the event — see the next test for why.
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
        // And the unmodified trio is untouched, which is the other half of a
        // branch: the guard was never meant to disable the clipboard.
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

    /// **`Ctrl+Alt+V` is read from the key release, because the *event* only exists
    /// when the OS clipboard holds text** (§15 D183) — and a property paste has
    /// nothing to do with the OS clipboard at all.
    ///
    /// This is the vacuous shape §15 D219 warns about, arriving from the other
    /// direction: read from `Event::Paste`, the chord would work whenever a sentence
    /// happened to be copied in another application and do nothing the rest of the
    /// time. From the chair that is an intermittent bug, and a test driving a
    /// synthetic `Event::Paste` would call it fixed.
    ///
    /// ⚠️ Flipped against `Event::Paste(_) if m.alt => push(PasteProperties)`, where
    /// the first assertion passes (the event is synthetic and always present) and
    /// the **second** fails — the empty-clipboard case, which is the real one.
    #[test]
    fn a_property_paste_does_not_need_the_os_clipboard_to_hold_anything() {
        // A clipboard with text in it: the event fires *and* the release does. The
        // event must be swallowed, or one chord means two things.
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
        // A clipboard with nothing in it: no event at all, and the chord still works.
        assert_eq!(
            actions(
                Mode::Normal,
                vec![release(egui::Key::V, cmd_alt())],
                cmd_alt()
            ),
            vec![Action::PasteProperties],
        );
        // And with Alt up the release is still the *ordinary* paste's weak witness,
        // which is the arm this one must not have stolen.
        let cmd = egui::Modifiers::COMMAND;
        assert_eq!(
            actions(Mode::Normal, vec![release(egui::Key::V, cmd)], cmd),
            vec![Action::PasteRelease],
        );
    }

    /// **Alt beats Shift on a paste**, so a hand still resting on Shift cannot turn
    /// a property paste into a paste-in-place.
    ///
    /// One arm's order, and it is only visible as a decision when both keys are
    /// down. Since the Alt arm now *silences* the event and the chord is read off
    /// the release, the failure this guards is `Ctrl+Alt+Shift+V` resolving to
    /// **both** — which is what the arms produce if Shift is tested first.
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

    /// **`Ctrl+Shift+…` is not a second spelling of `Ctrl+…`** (§15 D710,
    /// `[S17-L1-05]`) — and the three exceptions are asserted beside the rule,
    /// because each is one somebody would otherwise "fix".
    ///
    /// This file states the rule three times and never generalised it: `Save`,
    /// `NewDocument`, `CloseDocument`, `OpenSettings`, `Flatten`, `Grid`,
    /// `Guides`, `SnapShapes`, `Present`, `Mask`, `ExportAll` and the four
    /// booleans all carry `!m.shift`; eight arms carried `cmd_only` alone.
    /// `Ctrl+Shift+D` was the sharpest — a **document edit** and an undo step,
    /// next door to §11's `Shift+D` (*reverse subpath direction*).
    ///
    /// 🚨 **The finding names twelve doors and four of them must stay open.**
    /// Checking them was worth more than closing the other eight:
    ///
    /// - **`Ctrl+Shift+=`** is the *primary* spelling of `Ctrl++` — `+` is
    ///   `Shift+=` on a US layout and egui reports the **logical** key, so
    ///   `Key::Plus` is unreachable without Shift. `docs/shortcuts.md` §3 says
    ///   *"both spellings already accepted"* in the row itself. A blanket guard
    ///   would have removed zoom-in for anyone typing the chord as written.
    /// - **`Event::Copy` / `Event::Cut` under Shift** are pinned by
    ///   `shift_makes_the_paste_event_a_paste_in_place` with the reason: the
    ///   collision is the *text session's* centre-align, and this is
    ///   `normal_mode`, where that chord does not exist.
    /// - **`Ctrl+Alt+Shift+V`** is pinned by
    ///   `a_property_paste_is_not_also_a_paste_in_place`, whose subject sentence
    ///   is *"Alt beats Shift on a paste, so a hand still resting on Shift
    ///   cannot turn a property paste into a paste-in-place."*
    ///
    /// All three were tried and all three failed a pre-existing test, which is
    /// what a deliberate decision is supposed to do.
    ///
    /// ⚠️ **`Ctrl+Shift+Z` stays Redo** — it is the documented alternate
    /// spelling, so only the `Y` half of that arm took the guard.
    ///
    /// (Plain backticks per §15 D319 — `cargo doc` builds without the `test`
    /// cfg.)
    #[test]
    fn ctrl_shift_is_not_a_second_spelling_of_ctrl() {
        let cmd = egui::Modifiers::COMMAND;
        let shift = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::COMMAND
        };

        // The rule: each of these answers under `Ctrl` and must answer nothing
        // under `Ctrl+Shift`.
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

    /// All four chords resolve, and to the right operation.
    ///
    /// **`X` is driven as a release, because that is the only event the real app
    /// gets for it** (see `ctrl_alt_x_excludes_and_does_not_cut`). This test fed a
    /// synthetic *press* for `X` until 2026-08-18, and so was green for the whole
    /// time Exclude was unreachable from the keyboard: it was asserting on a signal
    /// `egui_winit` never delivers, which is the vacuous shape to watch for
    /// anywhere C/X/V appear in a keymap test.
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

    /// **`Ctrl+Alt+X` cut the selection instead of excluding it** — destructive, and
    /// silent, because a cut looks exactly like a boolean that consumed its
    /// operands. `docs/shortcuts.md` §L2.
    ///
    /// One root cause with two symptoms, and both are asserted here: `egui_winit`'s
    /// `is_cut_command` tests `command && X` and never looks at Alt, so it pushes
    /// `Event::Cut` and returns — which both fired the wrong action *and* meant no
    /// `Key::X` press ever existed for Exclude to be read from. So the press is
    /// checked as the `Event::Cut` the app really receives, and Exclude is checked on
    /// the release that falls past the guard.
    ///
    /// The half read out of `egui-winit`'s source — that a `Ctrl+Alt+X` press
    /// becomes `Event::Cut` and nothing else — cannot be tested here, for the same
    /// reason the paste test gives: `run_ui` only shows the egui→backend direction.
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

        // Alt up is an ordinary cut, unchanged. The guard has to separate the two
        // chords, not disable the clipboard.
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

    /// **The collision that would have been silent.** `Ctrl+S` saves and
    /// `Ctrl+Alt+S` subtracts; if the boolean chord ignored Alt, every save would
    /// also have combined the selection — and the file would still have been
    /// written, so nothing would look wrong until the artwork was.
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

        // And the reverse: the boolean chord must not also save.
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
        // The release rather than the press, because that is the signal Exclude is
        // now read from — a tripwire on the press would not cover the live route.
        assert!(
            actions(
                Mode::TextInsert,
                vec![release(egui::Key::X, cmd_alt)],
                cmd_alt
            )
            .is_empty()
        );
    }

    /// **`Ctrl+E` flattens and plain `E` still draws an ellipse.** The pair is the
    /// point: `E` is a single-key tool switch, so a chord that shared it would either
    /// swap tools while flattening or be swallowed by the tool. `plain` is what keeps
    /// them apart, and this is what says so.
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

    /// **Any focused widget silences the whole keymap, and `Tab` focuses one.**
    ///
    /// `egui_wants_keyboard_input` is `focused().is_some()` — not "a text field is
    /// taking characters" — so egui's own focus ring, which `Tab` drives, closes
    /// the guard `resolve` opens with. Reported as the point-stepping `Tab` working
    /// exactly once: the first press found nothing focused and stepped, the pass
    /// that followed moved focus onto a widget, and every press after that was
    /// dropped before `normal_mode` ever ran.
    ///
    /// The fix cannot be to consume the key — egui reads focus movement out of the
    /// `RawInput` at the start of the pass, before the app runs — so `OndinApp`
    /// gives the focus back at the **end** of a frame in which it claimed a `Tab`.
    /// This is the fact that makes that necessary, pinned so an egui upgrade that
    /// narrows `wants_keyboard_input` shows up here rather than as a key that
    /// quietly stops working.
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
        // One frame of two buttons, with a Tab in it; returns whether the keymap
        // was open *before* the frame ran.
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

        // Giving the focus back at the end of each frame keeps it open for every
        // press, which is the whole of the fix.
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

    /// **`Ctrl+;` and `Ctrl+Alt+;` are told apart by Alt, and must therefore be
    /// mutually exclusive.** Without `!m.alt` on the show arm, `Ctrl+Alt+;` fires
    /// both — hiding the guides *and* locking them in one press, which is the
    /// `Ctrl+Alt+S` double-fire (`save_and_cut_are_not_booleans`) in its quietest
    /// form: nothing errors, and the guides are simply gone and unrecoverable by
    /// the chord that took them away.
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

    /// The same key with Shift is a **third** chord — *Snap to guides* — and it
    /// must reach that and nothing else. Three chords on one key is where the
    /// `!m.shift` gates earn their keep: each arm has to exclude the other two,
    /// not merely include itself.
    ///
    /// **Both spellings, and the shifted one is the one that arrives.** `;`
    /// shifted is `:`, which `egui_winit` delivers as `Key::Colon` (§0) — so a
    /// binding naming only `Semicolon` would read perfectly and fire never, which
    /// is exactly how `Shift+1` was dead for as long as it was. The plain
    /// spelling is kept for a layout that passes the unshifted key through.
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
        // Unmodified, the key is nobody's.
        let none = egui::Modifiers::NONE;
        assert!(actions(Mode::Normal, vec![key(egui::Key::Semicolon, none)], none).is_empty());
    }

    /// Both are normal-mode commands, so a semicolon typed into a text node is a
    /// semicolon.
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

/// The chords `docs/shortcuts.md` §3–§6 added, and the neighbours each of them had
/// to be kept clear of.
///
/// **Every test here is a *pair*, not a single assertion**, because every one of
/// these chords shares its key with something already bound: `Shift+H` with the
/// Hand tool, `Alt+S` with subtract, `Ctrl+U` with union, `Ctrl+R` with
/// `Shift+R`. A test that only proves the new chord fires is passed just as well
/// by a binding that fires *both*, which is the failure this keymap has actually
/// had twice (`save_and_cut_are_not_booleans`, and §L2's `Ctrl+Alt+X`).
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

    /// Fit-everything and fit-selection are **two** actions, and the digit
    /// chords have to land on the right one of them.
    ///
    /// The thing worth pinning is the asymmetry with `Shift+1` next door, which
    /// needs `Exclamationmark` accepted as well as `Num1`: `!` has an
    /// `egui::Key` variant and `)`/`@` do not, so for these two the physical
    /// digit is what arrives and the obvious spelling is the only one there is
    /// (§0). Spelling these the way `Shift+1` is spelled would be harmless;
    /// spelling `Shift+1` the way these are is the bug §L1 records.
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
        // And the fit chord next door still means everything, not the selection —
        // which is what the split was for.
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Num1, cmd)], cmd),
            vec![Action::ZoomFit]
        );
    }

    /// `Shift+H` / `Shift+V` flip; **plain `H` and `V` still pick their tools.**
    ///
    /// The pairing is the test. The tool block is gated on `!m.shift`, and if it
    /// were not, `Shift+H` would flip the selection *and* switch to the Hand —
    /// the shape `shift_r_toggles_rulers_without_stealing_the_rect_tool` already
    /// pins for the rulers.
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

    /// `Alt` aligns, `Alt+Shift` distributes, and **`Ctrl+Alt+S` is still
    /// subtract**.
    ///
    /// That last one is the whole reason the align block is `!cmd` rather than
    /// merely `m.alt`. `Alt+S` is align-bottom and `Ctrl+Alt+S` is the boolean;
    /// an align block that only asked for Alt would fire both on the boolean
    /// chord, which is `save_and_cut_are_not_booleans` again with a different
    /// pair of victims.
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

    /// `Ctrl+Shift+H`/`L` are layer state; the same letters plain are tools, and
    /// with Shift alone `H` is a flip.
    ///
    /// Three chords on `H` — tool, flip, hide — is the densest key in this
    /// keymap, so all three are asserted together rather than one per test.
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

    /// `Ctrl+R` renames and `Shift+R` still toggles the rulers — the two
    /// readings of `R` this app deliberately keeps apart (`docs/shortcuts.md`'s
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

    /// The three backslash chords are three **different** switches, and the
    /// shifted one arrives as `Key::Pipe`.
    ///
    /// `Ctrl+Shift+\` is `|`, which has an `egui::Key` variant — so the logical
    /// key wins and a binding naming only `Backslash` fires never (§0). Both
    /// spellings are asserted; the `Pipe` one is what the real app sends.
    #[test]
    fn the_three_backslash_chords_are_three_distinct_switches() {
        let cmd = egui::Modifiers::COMMAND;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::Backslash, cmd)], cmd),
            vec![Action::ToggleView(ViewSwitch::Present)]
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
                 it must not also toggle present mode"
            );
        }
    }

    /// `Ctrl+'` is the grid and `Ctrl+Shift+'` is snap-to-grid — **one spelling
    /// each**, because `"` is the shifted punctuation `egui::Key` has no variant
    /// for, so it falls back to the physical `Quote` (§0).
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
    ///
    /// The same `cmd_only`-versus-`cmd && alt` split the booleans needed against
    /// save and cut, on the letter §6 newly added to that collision set.
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

    /// `O` is the ellipse's published letter and `E` is the alias kept beside it.
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
        // `Ctrl+E` is flatten, which is what keeping the `E` alias had to leave
        // alone.
        let cmd = egui::Modifiers::COMMAND;
        assert_eq!(
            actions(Mode::Normal, vec![key(egui::Key::E, cmd)], cmd),
            vec![Action::Flatten]
        );
    }

    /// The chords that mean **two different things** depending on the mode, and
    /// the ones that mean nothing in a text session at all.
    ///
    /// This is the paired test `docs/shortcuts.md` §10 asks each colliding chord for:
    /// the chord in Normal mode, and the same chord in TextInsert. Nothing but
    /// `Mode` tells them apart, so a routing change that let one leak is
    /// invisible in the source — `Ctrl+U` snapping to shapes *while typing*
    /// would read as the editor randomly moving the artwork.
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

    /// Everything else this module added is Normal-mode only, so a text session
    /// never sees it.
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

/// The plain digits, and the two things they must not disturb
/// (`docs/shortcuts.md` §2).
#[cfg(test)]
mod opacity_key_tests {
    use super::tests::{actions, key};
    use super::*;

    /// Every digit resolves, and to its own number.
    ///
    /// Driven over all ten rather than a sample, because the binding is written
    /// as an `enumerate()` over a key list — the failure it can have is an
    /// off-by-one across the whole range, which any single digit would miss.
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

    /// **The shifted digits are not opacity**, and the reserved ones are not
    /// anything.
    ///
    /// `Shift+0/1/2` are the zoom chords, so a digit block that forgot `!m.shift`
    /// would set the opacity on every zoom — silent, destructive and exactly the
    /// double-fire shape §L2 records. `Shift+3`–`9` are held for blend modes and
    /// must resolve to **nothing**: falling through to opacity is how a reserved
    /// chord quietly stops being reserved.
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

/// The chords a live text session admits (`docs/shortcuts.md` §10), and the rule that
/// bounds them.
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

    /// **The rule: bare keys are characters, modified chords are commands.**
    ///
    /// This is the invariant the whole section rests on — a session's buffer must
    /// have one author. Swept over the letters the chords use *unmodified*,
    /// because that is the direction the mistake goes: adding a chord and
    /// forgetting to require its modifier turns a letter into a command, and the
    /// symptom is a designer typing "underline" and watching the word not appear.
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

    /// Font size up and down, on one spelling each.
    ///
    /// `Ctrl+Shift+.` is `>` and `Ctrl+Shift+,` is `<`, and `egui::Key` has a
    /// variant for **neither** — so the logical key falls through to the physical
    /// `Period`/`Comma` and the obvious spelling is the only one there is (§0).
    /// The opposite of `Ctrl+Shift+;`, which needs `Key::Colon` accepted too.
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

    /// The four alignment chords — **and `C` is read from the release**.
    ///
    /// `egui_winit`'s `is_copy_command` tests `command && Key::C` and never looks
    /// at Shift, so a `Ctrl+Shift+C` *press* is swallowed and re-emitted as
    /// `Event::Copy`: a binding written as `key_pressed(C)` reads perfectly and
    /// fires never. That is §L2's `Ctrl+Alt+X` exactly, one letter over. So the
    /// press is driven here as well as the release, and only the release may
    /// resolve — a test that pushed a synthetic press would be green against a
    /// binding the real app can never reach.
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

    /// **`Ctrl+Alt` is not `Ctrl`**, here as everywhere else in this keymap.
    ///
    /// `cmd_only` is stated per binding rather than once, and the failure it
    /// prevents has happened twice already (`Ctrl+Alt+S` saving as well as
    /// subtracting, `Ctrl+Alt+X` cutting as well as excluding). A session is
    /// where it would be quietest: the underline appears, and whatever the
    /// `Ctrl+Alt` chord was meant for happens too.
    #[test]
    fn adding_alt_takes_a_chord_away_rather_than_leaving_it() {
        let cmd_alt = egui::Modifiers {
            alt: true,
            ..egui::Modifiers::COMMAND
        };
        assert!(actions(Mode::TextInsert, vec![key(egui::Key::U, cmd_alt)], cmd_alt).is_empty());
        // And the arrows the other way about: Ctrl+Alt+↑ is neither leading nor
        // a caret move.
        assert!(
            actions(
                Mode::TextInsert,
                vec![key(egui::Key::ArrowUp, cmd_alt)],
                cmd_alt
            )
            .is_empty()
        );
    }

    /// **The four document chords resolve in a session too**, and they carry
    /// `normal_mode`'s guards rather than looser ones (§15 D815).
    ///
    /// 🚨 **They were dead here by omission, not by a decision.** §9.3 records
    /// that `Mode::TextInsert` is *near* exclusive — *"no **bare** key resolves to
    /// an `Action` there"* — which is a rule about bare keys and never about
    /// chords; `Undo`, `Redo`, `Save` and `Open` simply lived in `normal_mode`
    /// alone. Meanwhile the top bar's buttons for three of them were live the
    /// whole time (§15 D466), so a click rewound the document and the chord 30px
    /// away did nothing.
    ///
    /// ⚠️ **The `Shift` half is the load-bearing part of this test.** §15 D710
    /// removed `Ctrl+Shift+Y` and `Ctrl+Shift+S` as second spellings in
    /// `normal_mode`, and `ctrl_shift_is_not_a_second_spelling_of_ctrl` sweeps
    /// *that* function's arms — not these. A copy of a keymap is where a removed
    /// spelling comes back, so it is swept here separately.
    ///
    /// ⚠️ **What this cannot see is the finishing.** The session is ended by
    /// `OndinApp::dispatch`, one seam further on, because it is a fact about the
    /// action and not about the key.
    ///
    /// **Flip-check, run** by deleting the four-chord loop from
    /// `text_insert_mode`: fails at the first assertion, `Ctrl+Z`, with `[]`
    /// against `[Undo]`. Flipped the other way by dropping `!m.shift` from the
    /// `Save` arm: fails in the Shift sweep at `Ctrl+Shift+S`, which is the
    /// assertion that would otherwise never have been written.
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
            // And it is the same answer the canvas gives, which is the whole
            // claim: one behaviour, not two that have to be kept equal.
            assert_eq!(
                actions(Mode::Normal, vec![key(k, cmd)], cmd),
                vec![want],
                "control: Ctrl+{k:?} outside one"
            );
        }

        // `Ctrl+Shift+Z` is the *other* spelling of Redo and must still be one.
        assert_eq!(
            actions(
                Mode::TextInsert,
                vec![key(egui::Key::Z, cmd_shift())],
                cmd_shift()
            ),
            vec![Action::Redo],
            "Ctrl+Shift+Z is Redo in a session as well"
        );

        // And the three that D710 took away stay away.
        for k in [egui::Key::Y, egui::Key::S, egui::Key::O] {
            assert!(
                actions(Mode::TextInsert, vec![key(k, cmd_shift())], cmd_shift()).is_empty(),
                "Ctrl+Shift+{k:?} is not a second spelling here either"
            );
        }
    }
}
