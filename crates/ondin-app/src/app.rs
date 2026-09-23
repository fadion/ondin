//! The eframe application: egui chrome hosting the Vello canvas (§9.1–9.2).
//!
//! This module owns the shell — window layout, the top bar and tool rail, and
//! the dispatch of resolved [`Action`]s. The pieces it coordinates live next to
//! it: `session` (the document and everything derived from it), `canvas` (the
//! Vello texture and canvas interaction), `panels` (layers and inspector),
//! `input` (mode-aware key routing), `preview` (in-flight gesture state).
//!
//! egui types stay confined to `ondin-app`; nothing here leaks into
//! core/render/export/mcp, which is what keeps a UI-toolkit swap survivable.

use crate::canvas::CanvasRenderer;
use crate::cursor::Cursors;
use crate::fonts::FontService;
use crate::input::{self, Action, Mode, ViewSwitch};
use crate::panels::{ImageCard, LayerDrag, PaintSlot, Picker};
use crate::preview::{Drag, PenState, PointSet, TextSession};
use crate::session::{EditorSession, StatusKind};
use crate::theme::{self, color, icon};
use crate::tools::Tool;
use crate::ui::{eyebrow, field_row, icon_button, panel_frame};
use eframe::egui;
use ondin_core::kurbo::{Point, Rect, Vec2};
use ondin_core::{
    CharSpans, Guide, GuideId, ImageEntry, ImageId, ImageSource, Node, NodeId, NodeKind, Operation,
    ParaSpans, Transaction, build,
};
use std::collections::HashSet;

/// Width of an inspector popover — the stroke options, and the typography tabs.
///
/// **One number for both.** `design/Editor.dc.html` draws every popover card at
/// 272; the stroke one carried 276 from before that file existed, and the two being
/// a few points apart is not a distinction anyone asked for. It is also what
/// [`OndinApp::picker_lane_right`] needs in order to reserve the lane, and a
/// clearance computed from a width that might be the other popover's is a
/// clearance that is sometimes wrong.
///
/// **The width the card paints**, which is a correction rather than a restatement:
/// until 2026-08-23 this was the content width and the border put another two points
/// outside it, so every popover painted 274 and the lane clearance
/// [`OndinApp::popover_left`] computes from this was 8pt where it reads 10 (§15 D307).
/// `crate::ui::menu_inner_w` is what the controls inside are laid out against now.
pub(crate) const POPOVER_W: f32 = 272.0;

/// The three faces of the typography popup.
///
/// **Tabs are scopes, labelled with the scope word**, because the label is what
/// tells the user what a change will hit when the selection is partial.
///
/// **Three, and the words are spelled out.** There was a fourth — `Font`, holding
/// the axes, the OpenType features and the language — split off because those
/// controls are *generated from the family* rather than fixed. That is true of how
/// they are built and irrelevant to the person using them: everything in it was
/// character-scoped, so the tab strip named three scopes and one implementation
/// detail, and a user looking for the weight axis had two character tabs to guess
/// between. Folded back into `Character`, which leaves the strip saying exactly
/// what it means — and at three cells of an inner 250px there is room for the
/// whole word, which is what retired the abbreviations (`Char`/`Para`/`Block`) and
/// the line of hint text that had to sit underneath explaining them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TypeTab {
    /// The tab the popup opens on: the one whose controls a partial selection
    /// most often means to reach.
    #[default]
    Character,
    Paragraph,
    /// The text box itself. Named `Block` for `BlockStyle`, the model type it
    /// edits, and labelled **Box** because that is what a designer calls the
    /// thing on the canvas.
    Block,
}

impl TypeTab {
    pub const ALL: [TypeTab; 3] = [TypeTab::Character, TypeTab::Paragraph, TypeTab::Block];

    pub fn label(self) -> &'static str {
        match self {
            TypeTab::Character => "Character",
            TypeTab::Paragraph => "Paragraph",
            TypeTab::Block => "Box",
        }
    }
}

/// What a layer copy holds.
///
/// **Two fields rather than two clipboard slots, because they must not drift.**
/// The nodes alone are half a picture: a fill stores an image *key*, and the
/// bytes live in a table on the document. Copy an image layer, open another
/// document, paste, and the key resolves to nothing there — the layer arrives as
/// the missing-picture placeholder, silently, and *Export original…* then dims
/// saying the picture is "linked", which it is not. The clipboard is deliberately
/// **not** cleared when a document is opened (`reset_transient_state` clears
/// `alt_clone`, whose ids cannot outlive their document, and leaves this alone),
/// so cross-document paste is a feature rather than an accident — which makes
/// carrying the bytes the fix rather than dropping the payload.
///
/// The entries are taken **at copy time**, the last moment the source document is
/// certainly open — the same reasoning [`OndinApp::clipboard_from`] carries for
/// the box a cut destroys.
#[derive(Clone)]
pub(crate) struct Clip {
    /// One per copied layer, ids still the originals' until `insert_subtrees`
    /// remaps them. A `Vec` of subtrees rather than one, so copying a
    /// multi-selection keeps all of it.
    pub(crate) subtrees: Vec<Vec<Node>>,
    /// The table entries those subtrees reference (`build::image_ids_in`), which
    /// `insert_all` adds to the target document if it does not already have them.
    /// Empty for everything that is not a picture, which is most copies.
    pub(crate) images: Vec<(ondin_core::ImageId, ondin_core::ImageEntry)>,
}

/// What the system clipboard turned out to be holding, when a paste asks
/// (§15 D823).
///
/// **Three answers rather than a `bool`, and the third is the reason.** A paste
/// used to ask one question — *is the in-app payload still what the system
/// clipboard describes* ([`OndinApp::owns_the_clipboard`]) — and a `no` meant
/// "try the other arms". With layers now crossing between windows there is a
/// third state: text that **is** an Ondin copy and that this build cannot read.
/// Falling through on that would hand the payload to
/// [`OndinApp::paste_text_as_layer`], so the user asks for their layers back and
/// gets a text layer holding several megabytes of their own JSON. It is the one
/// outcome worth a variant.
enum Clipboard {
    /// [`OndinApp::clipboard`] is live — either it never left this window, or the
    /// foreign payload has just been adopted into it. The layer arm can run.
    Layers,
    /// Somebody else's: a sentence, some markup, a path, nothing. The caller's
    /// remaining arms are what it is for.
    Foreign,
    /// Ours, and unreadable. Already reported, and the caller must **stop**.
    Unreadable,
}

/// The receipt for the stand-in a copy put on the system clipboard: its length
/// and a 64-bit hash, where it used to be the text itself (§15 D857,
/// `[X1.2-L4-01]`).
///
/// **As strong as the text for the question it answers** — *is the clipboard
/// still exactly what we wrote* — at a collision rate of one in 2⁶⁴, and a
/// collision's cost is a stale in-app paste, not a lost one. `DefaultHasher::new`
/// is deterministic within a process, which is the whole of the receipt's
/// lifetime; it is never written anywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClipStamp {
    len: usize,
    hash: u64,
}

impl ClipStamp {
    pub(crate) fn of(text: &str) -> Self {
        use std::hash::{Hash as _, Hasher as _};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        Self {
            len: text.len(),
            hash: h.finish(),
        }
    }
}

/// The values a gesture holds **while it is running**, in one place so that
/// "is anything in flight?" is answered by the type instead of by a list somebody
/// has to remember to extend (§15 D785, `[A2-L7-02]`).
///
/// 🚨 **The list went stale twice and the cost was silent.** `cancel_gesture`'s
/// gate was a hand-written `||` chain over fields scattered through `OndinApp`'s
/// 101, and `[A1-L2-02]` found it two short: right-clicking to cancel a scrub on
/// the export-quality field or on the inspector's guide-position field **did
/// nothing, and the value committed on release**. Neither shows through
/// `has_gesture_preview` — `SetExports` never reaches `set_preview` and
/// `SetGuidePosition` is absorbed as a no-op — so there was no second signal to
/// fall back on. Three of that chain's clauses had been added one at a time, each
/// after a field was added elsewhere and nothing told the gate, and the comment on
/// one of them says as much: *"which is the shape of bug this whole list is made
/// of"*.
///
/// **[`Self::any`] is exhaustive by construction**, because it is a `PartialEq`
/// against `Default`. An **eighth** field added to this struct is covered on the day
/// it is added, with nothing to remember — which is the whole of what this type is
/// for, and the only reason it is worth moving fields out of `OndinApp` at all.
/// (Seven fields below; `cancel_gesture`'s own note says *eighth* too. The
/// finding's *"fifteen in-flight-gesture fields"* counts `drag`, `layer_drag`,
/// `multi_transform_drag`, `secondary_press` and `alt_clone`, which are not here —
/// see *What is deliberately not here*.)
///
/// # What is deliberately not here
///
/// ⚠️ **`drag` and `layer_drag` stay on `OndinApp`.** They are named separately in
/// `cancel_gesture` because that function *acts* on them — it writes `Drag::None`
/// and drops the row — where everything here is read by its own owner on the
/// `gesture_cancelled` flag. Neither has ever been the one forgotten: they are the
/// two fields any reader of a gesture bug looks at first, and `drag` alone has 101
/// call sites, so folding them in would be ~150 renames buying nothing. **The line
/// is "a value a gesture is holding", not "a gesture".**
///
/// ⚠️ **Clearing is not here either, and that is not an oversight.** The fields
/// below are *not* all dropped by `cancel_gesture`: `grid_scrub` and `grid_paint`
/// are, because their in-flight value is on the canvas, and the rest are read back
/// by their owners through `OndinApp::gesture_cancelled` — which is how a field
/// that needs to *restore* something gets the chance. A blanket
/// `*self = Self::default()` would look tidier and would take that chance away.
#[derive(Default, PartialEq)]
pub(crate) struct InFlight {
    /// The typographic spans as they were when the field scrub now in flight
    /// began — **what a right-click cancel puts back**, and the only record of it,
    /// since a session's own preview is what `clear_preview` hides rather than
    /// undoes. Reported as right-click resetting the text but not the field.
    ///
    /// The node is carried with the lists so a snapshot cannot be applied to a
    /// session that has since moved to a different layer.
    pub(crate) session_scrub: Option<(NodeId, CharSpans, ParaSpans)>,
    /// The Settings modal's nudge pair as it was when the field scrub now in
    /// flight began — **what a right-click cancel puts back**, and the only
    /// record of it. [`Self::session_scrub`]'s problem in a second place and for
    /// the same reason: the modal writes into its own staged draft rather than
    /// through `RenderOverrides`, so `cancel_gesture` has no preview to drop. A
    /// right-click on those two fields did nothing at all until this existed.
    ///
    /// ⚠️ **It is also a term that keeps `cancel_gesture` from *blinking*.** That
    /// function's `field` test leads on `ctx.dragged_id()`, which egui clears on
    /// the release of **any** button — so a right-click whose press and release
    /// fall inside one rendered frame reads an already-empty snapshot and the
    /// cancel is a no-op. Everything in this struct is armed for the whole scrub
    /// and does not blink, which is half of why the gate asks [`Self::any`] rather
    /// than asking egui.
    pub(crate) settings_scrub: Option<crate::input::NudgeStep>,
    /// A Fill or Stroke row being dragged up or down its list.
    ///
    /// 🚨 **The one the old gate reached last** (§15 D588, `[S14.3-L1-02]`): the
    /// clear below `cancel_gesture`'s `||` chain called itself *"the fourth, for
    /// the same reason"* and nothing **above** the chain ever answered for it, so
    /// the function returned before reaching its own clear whenever a paint
    /// reorder was the only gesture in flight.
    pub(crate) paint_drag: Option<crate::panels::PaintDrag>,
    /// The JPEG quality being scrubbed, and which spec it belongs to.
    ///
    /// **The in-flight value of a scrub that cannot preview.** `RenderOverrides`
    /// absorbs `SetExports` as a no-op, so `edit_valve`'s preview shows nothing
    /// and the field would re-read the committed number every frame; committing
    /// every frame instead makes the drag uncancellable, because `cancel_gesture`
    /// undoes one by dropping the preview. Holding the value here gives both —
    /// live under the hand, one commit on release, nothing at all on a
    /// right-click.
    pub(crate) export_quality_scrub: Option<(usize, f64)>,
    /// The layout grid a scrub is holding: the **frames** it is being written to,
    /// the index in their list, and the in-flight value.
    ///
    /// **The third scrub that cannot preview through `RenderOverrides`**, after
    /// [`Self::export_quality_scrub`] and [`Self::session_scrub`] — and the one
    /// with the loudest consequence, because a layout grid *is* drawn on the
    /// canvas (`canvas::draw_layout_grids`). So this buffer is read twice: by the
    /// panel, so the field moves under the hand, and by the overlay, so the bands
    /// move with it. The alternative — committing every frame — is the
    /// arrangement `panels::export`'s note records being reported, since
    /// `cancel_gesture` undoes a scrub by dropping a preview and there is none
    /// here.
    ///
    /// ⚠️ **A list of frames rather than one, because the panel takes a
    /// selection.** The edit is written to every selected frame at once, so a
    /// buffer naming only the anchor would preview the drag on one frame while
    /// committing it to five — the bands under the hand moving and the rest
    /// jumping on release. `panels::inspector`'s `frame_subjects` fills it.
    pub(crate) grid_scrub: Option<(Vec<NodeId>, usize, ondin_core::LayoutGrid)>,
    /// The grid **colour** a picker is dragging: the frames, the index, and the
    /// in-flight `Color`. `OndinApp::preview_grid` fills it; the canvas reads it
    /// beside [`Self::grid_scrub`].
    ///
    /// ⚠️ **A second buffer over the same grid, and the two are not one thing
    /// spelled twice.** `grid_scrub` is the *panel's* own: the numeric fields own
    /// their valve, so anything sitting in it is a value `grid_step` must decide
    /// about, and a colour dropped there would be read on the next frame as a
    /// finished field edit and committed — every frame of the drag, one undo step
    /// each. This one belongs to the **picker**, whose valve is `edit_valve`, and
    /// nothing in `grid_rows` looks at it. They compose without meeting: a pointer
    /// is in one control at a time, and the canvas applies the scrub and then
    /// this.
    ///
    /// `Color` rather than a whole `LayoutGrid`, because that is all a picker can
    /// write and a buffer that could hold more would have to say what happens when
    /// it disagrees with the panel's.
    pub(crate) grid_paint: Option<(Vec<NodeId>, usize, ondin_core::peniko::Color)>,
    /// The value a guide position field is being dragged **to**, and which axis's
    /// field it belongs to.
    ///
    /// **The field is absolute, and it has to hold its own target.** Every frame
    /// the field is laid out from the *committed* document, so a field that knew
    /// only this frame's step would commit the last step and lose the rest of the
    /// drag — and on the release frame the pointer has not moved since the
    /// previous one, so that step is zero and the whole gesture commits nothing.
    /// That is §15 D51's argument, and it applies here for the same reason it
    /// applies to `OndinApp::multi_transform_drag`: holding the target means every
    /// frame of the drag builds the *same* transaction from the same base, so the
    /// preview and the commit are the same edit by construction.
    ///
    /// Keyed by [`ondin_core::GuideAxis`] because a mixed-axis selection lays out
    /// two fields side by side, and a total left over from one would land on the
    /// other.
    ///
    /// ⚠️ A guide dragged on the **canvas** is not this: that sets `Drag::Guide`
    /// and is caught by `OndinApp::drag`, which is exactly why this one read as
    /// working while it did not.
    pub(crate) guide_position_drag: Option<(ondin_core::GuideAxis, f64)>,
}

impl InFlight {
    /// Is any gesture holding a value right now?
    ///
    /// **A comparison against `Default`, not an `||` chain**, and that is the
    /// entire point of the type: the chain is what went stale twice. Adding a
    /// field above extends this with nothing to write.
    ///
    /// ⚠️ **It is `PartialEq` and not `Eq` because two of the fields carry
    /// floats** — an `f64` and a `Color`. That is sound here: every field is an
    /// `Option` whose `None` is what `Default` gives, so this asks *"is any of
    /// them `Some`"* and never compares two payloads. A `NaN` in flight is still
    /// `Some` and still cancels.
    pub(crate) fn any(&self) -> bool {
        *self != Self::default()
    }
}

#[cfg(test)]
mod in_flight_tests {
    //! §15 D785 — `InFlight::any` reads every field.
    //!
    //! ⚠️ **This list is itself hand-written and would go stale exactly as the
    //! `||` chain did** — which is worth saying plainly, because it looks like the
    //! gate and is not. The difference is where the staleness lands: a chain that
    //! forgets a field is a **cancel that silently does nothing**, and a test that
    //! forgets one is a test that checks six of seven while the production answer
    //! stays right by construction. What this pins is that `any` reads every field
    //! *today*, so a reader can confirm the claim once instead of trusting it.
    //!
    //! (Plain backticks per §15 D319.)

    use super::*;

    /// Each field alone arms the gate.
    ///
    /// **Written as seven separate `InFlight::default()`s on purpose.** Setting
    /// them cumulatively would leave every assertion after the first green for the
    /// first field's sake — the vacuous shape `CLAUDE.md` asks *"what would also
    /// pass this"* about.
    ///
    /// **Flipped with the exact shape that caused the bug**: `any` rewritten as an
    /// `||` chain over six of the seven, omitting `export_quality_scrub`. It fails
    /// here at the predicted assertion — and **also fails
    /// `canvas::headless_app_tests::cancelling_needs_a_witness_and_our_two_do_not_blink`**,
    /// which drives the real `cancel_gesture`. Two independent detectors for one
    /// defect, which is the arrangement worth having: this one says the predicate
    /// is short, and that one says a user's right-click does nothing.
    #[test]
    fn every_field_on_its_own_arms_the_gate() {
        assert!(
            !InFlight::default().any(),
            "an idle app holds nothing — a gate any wider than this takes the \
             right-click that removes a gradient stop"
        );

        let id = ondin_core::IdSource::new(0x1234).mint();
        // `..Default::default()` on each, so every case is *exactly one* field set
        // and reads as one.
        for (what, f) in [
            (
                "session_scrub",
                InFlight {
                    session_scrub: Some((id, Default::default(), Default::default())),
                    ..Default::default()
                },
            ),
            (
                "settings_scrub",
                InFlight {
                    settings_scrub: Some(crate::input::NudgeStep::default()),
                    ..Default::default()
                },
            ),
            (
                "export_quality_scrub — one of the two `[A1-L2-02]` found missing",
                InFlight {
                    export_quality_scrub: Some((0, 0.8)),
                    ..Default::default()
                },
            ),
            (
                "guide_position_drag — the other one",
                InFlight {
                    guide_position_drag: Some((ondin_core::GuideAxis::Vertical, 100.0)),
                    ..Default::default()
                },
            ),
            (
                "grid_scrub",
                InFlight {
                    grid_scrub: Some((
                        vec![id],
                        0,
                        ondin_core::LayoutGrid::new(ondin_core::GridAxis::Columns),
                    )),
                    ..Default::default()
                },
            ),
            (
                "grid_paint",
                InFlight {
                    grid_paint: Some((vec![id], 0, ondin_core::peniko::Color::BLACK)),
                    ..Default::default()
                },
            ),
            (
                "paint_drag — the one §15 D588 found the gate reaching last",
                InFlight {
                    paint_drag: Some(crate::panels::PaintDrag {
                        anchor: id,
                        list: crate::panels::paint::PaintList::Fill,
                        from: 0,
                        to: 1,
                        slots: Default::default(),
                    }),
                    ..Default::default()
                },
            ),
        ] {
            assert!(f.any(), "{what} on its own has to arm the gate");
        }
    }

    /// A value that is `Some` but **equal to nothing meaningful** still arms it.
    ///
    /// 🚨 **This is the assertion that says `any` is not comparing payloads.** A
    /// `0.0` guide position and an empty frame list are the values a reader most
    /// suspects of being mistaken for "nothing in flight" — and they are not,
    /// because `Option::None` is what `Default` gives and `Some(anything)` differs
    /// from it whatever the payload. The `f64` in there is why the derive is
    /// `PartialEq` and not `Eq`, and it costs nothing here.
    #[test]
    fn a_zero_valued_gesture_is_still_a_gesture() {
        let at = |v: f64| InFlight {
            guide_position_drag: Some((ondin_core::GuideAxis::Vertical, v)),
            ..Default::default()
        };
        assert!(
            at(0.0).any(),
            "a guide dragged to 0 is a guide being dragged"
        );
        assert!(
            at(f64::NAN).any(),
            "a NaN in flight cancels rather than escaping the gate"
        );
        assert!(
            InFlight {
                grid_scrub: Some((
                    Vec::new(),
                    0,
                    ondin_core::LayoutGrid::new(ondin_core::GridAxis::Columns),
                )),
                ..Default::default()
            }
            .any(),
            "…and an empty subject list is still a scrub in flight"
        );
    }
}

/// The document's artboards and their world boxes, remembered against the
/// revision they were read at.
///
/// 🚨 **`artboards()` is a full-document walk answering a question about four
/// nodes, and a move drag ran it forty times a frame** (§15 D616,
/// `[S12.2-L4-04]`). `move_destination` → `frame_covering` → `artboards` is per
/// *node*, per call; `leaving_their_frames` runs once a frame, `draw_frame_drop_outline`
/// runs `move_destination` over the same roots again on the same frame, and an
/// Alt-drag's `clone_tx` is a third pass. Measured in release at 16,000 nodes,
/// 200 iterations: `artboards()` **0.157 ms**, `leaving_their_frames` **0.156 ms**
/// at one root and **3.037 ms** at twenty — so the whole cost is the walk and it
/// scales with the *document*, not with the artboard count. A twenty-layer move
/// was **6.07 ms/frame**, **9.11 with Alt**, against a 16.7 ms budget and before
/// anything is drawn.
///
/// **Keyed on `EditorSession::revision`, which is an existing invalidation rule
/// rather than a new one.** That counter is bumped by `commit_inner` — including
/// undo and redo, which go through the same door — and by `adopt_document`, and
/// those are exactly the events that can add, remove or move an artboard. A
/// gesture in flight does not bump it and must not: these are the **committed**
/// world bounds, which is what `frame_covering` read before this and what the
/// drop rule is defined against.
#[derive(Default)]
pub(crate) struct FrameIndex {
    /// The revision `frames` was read at, or `None` for never.
    at: Option<u64>,
    /// Every artboard, depth-first in child order — which is paint order, and
    /// which everything reading this list relies on ([`OndinApp::artboards`]).
    frames: Vec<NodeId>,
    /// Those of them with world bounds, paired with the bounds.
    pub(crate) boxes: Vec<(NodeId, Rect)>,
    /// How many times the document walk has actually run.
    ///
    /// **Test-facing, and it is the assertion the finding asked for**: *"one
    /// move-drag frame with 20 roots selected must call `artboards()` once, not
    /// 40 times"*. A wall-clock assertion would say the same thing less durably —
    /// this one still fails if the memo is defeated on a fast machine.
    walks: u64,
}

pub struct OndinApp {
    pub(crate) session: EditorSession,
    /// See [`FrameIndex`]. `RefCell` because every reader is `&self` and the whole
    /// point is that reading may fill it.
    pub(crate) frame_index: std::cell::RefCell<FrameIndex>,
    /// How many times `canvas::node_grab` has reached its segment walk — the one
    /// step in it that allocates a whole `BezPath` and touches every segment.
    ///
    /// **Test-facing, and a counter rather than a clock for a reason the clock
    /// itself gave** (§15 D623, `[S12.3-L4-05]`). The walk's cost dominates
    /// `node_grab` in **release**, which is where the finding measured it and
    /// where a timing assertion has a 5× margin; in **debug** `edited_subpaths`
    /// dominates instead and the same assertion has about 1.6×. So a wall-clock
    /// bound is either too tight for the profile the suite runs in or too loose to
    /// mean anything — and the claim was never about time. It is *"the pointer is
    /// nowhere near the path, so do not walk it"*, which is countable.
    pub(crate) segment_walks: std::cell::Cell<u64>,
    pub(crate) canvas: CanvasRenderer,
    /// Explicit editing mode — every keystroke routes through it (§9.3).
    pub(crate) mode: Mode,
    pub(crate) tool: Tool,
    pub(crate) drag: Drag,
    /// The copy an Alt-drag is carrying, prepared once when Alt went down.
    pub(crate) alt_clone: Option<crate::canvas::AltClone>,
    /// What a layer copy is holding: the subtrees, and the pictures they need.
    pub(crate) clipboard: Option<Clip>,
    /// The world box the copied layers occupied **at the moment they were copied**.
    ///
    /// What *Paste here* measures its offset from, and it is recorded rather than
    /// looked up for one reason: after a **cut** there is nothing left to look up
    /// (§15 D251). Reading it back off the source ids meant a cut-then-paste-here
    /// landed on the original position and stayed there however many times it was
    /// repeated — the pointer had nothing to be measured against, so the row silently
    /// became *Paste in place*.
    ///
    /// **A snapshot, like the payload beside it.** The two are stamped together and
    /// have to be: a payload frozen at copy time whose provenance is read live is a
    /// pair that disagrees the moment the original is moved, which is the same bug
    /// one step milder.
    pub(crate) clipboard_from: Option<Rect>,
    /// A digest of the stand-in text the last in-app copy put on the **system**
    /// clipboard, kept as the receipt [`Self::owns_the_clipboard`] reads back to
    /// tell whether [`Self::clipboard`] and [`Self::guide_clipboard`] still
    /// describe it.
    ///
    /// ⚠️ **A digest and not the text, since §15 D857** (`[X1.2-L4-01]`). It was
    /// the text, which was tens of bytes of layer names until §15 D823 made the
    /// stand-in the whole `io::clip` payload — base64 pictures and all. One 4 MB
    /// photo in a copy was then a ~5.4 MB `String` held for the session, in the
    /// copying window *and* the pasting one, beside the `Arc` of the same bytes
    /// in [`Self::clipboard`], and compared whole on every paste. The receipt's job
    /// is "is the clipboard still exactly what we wrote", which a length and a
    /// 64-bit hash answer as well as the text did.
    pub(crate) clipboard_stamp: Option<ClipStamp>,
    /// The guides Ctrl+C put on the clipboard.
    ///
    /// **A second field rather than a variant, and the two are kept mutually
    /// exclusive by hand** — every copy clears the other, exactly as every
    /// `Selection` mutator clears the other half of *that* pair, and for the same
    /// reason: a selection is layers **or** guides, so a copy is one or the other
    /// and there is no state where holding both means anything. Clearing is what
    /// makes Ctrl+V unambiguous without a discriminant to match on.
    ///
    /// Whole `Guide`s, not captured subtrees: a guide has no children, no
    /// transform and no paint, so the struct *is* the template. Its id is
    /// discarded and a fresh one minted on paste.
    pub(crate) guide_clipboard: Option<Vec<Guide>>,
    /// The appearance *Copy properties* lifted off a layer — `Ctrl+Alt+C`, and
    /// the row beside it (`docs/shortcuts.md` §7, `docs/context-menus.md` §3's
    /// Properties group).
    ///
    /// **A third slot, and unlike the two above it is *not* kept exclusive with
    /// them.** The pair above are two spellings of one clipboard — a copy is
    /// layers or guides, and `Ctrl+V` has to be unambiguous about which. This is a
    /// different clipboard with a different chord and a different verb, so holding
    /// an appearance *and* a set of layers at once is a state that means exactly
    /// what it says: `Ctrl+V` pastes the layers and `Ctrl+Alt+V` paints them.
    /// Clearing one from the other would make copying a layer silently forget an
    /// appearance the user is halfway through applying.
    ///
    /// **Nothing about it reaches the system clipboard**, so it needs no receipt
    /// ([`Self::clipboard_stamp`]): there is no cross-application format for "these
    /// fills and this opacity" to be stale against, and the layer clipboard's
    /// stamp exists only because a *layer* copy also writes names out.
    pub(crate) property_clipboard: Option<ondin_core::build::Properties>,
    /// Which of the app's two screens is showing — the library or the editor.
    ///
    /// **A whole-window switch, not a panel.** The dashboard is not chrome over
    /// the document; it is the other thing the app is, and while it is up the
    /// canvas, the tool rail, the inspector and every keyboard action they
    /// answer are simply not running. `<OndinApp as eframe::App>::ui` returns
    /// early on it — plain backticks because it is a *trait* method, so there is
    /// no inherent item for an intra-doc link to resolve to (`canvas.rs`'s
    /// convention) — which
    /// is what makes that structural rather than a promise each panel has to
    /// keep.
    pub(crate) view: View,
    /// The library: the base folder, its projects and its documents.
    ///
    /// ⚠️ **Scanned when the dashboard is entered, not per frame** — see
    /// `library::state::Library`. Held even while the editor is up, because the
    /// editor needs it too: a save has to know the root, and the top bar's
    /// breadcrumb names the open document's project.
    pub(crate) library: crate::library::state::Library,
    /// When the open document was last written, for the autosave interval.
    ///
    /// **A deadline the app owns rather than a timer**, because there is no
    /// clock thread: it is compared against on each frame the editor draws, and
    /// a document nobody is looking at is a document nobody is editing.
    pub(crate) last_autosave: std::time::Instant,
    /// Crash recovery: this session's snapshot, and any left by a previous run
    /// (`crate::library::recovery`, §15 D377).
    ///
    /// **Beside `last_autosave` because it is the same shape of thing and a
    /// different question.** Autosave writes the document on the user's interval
    /// and refuses to file one that has never been saved; this writes a scratch
    /// copy on a fixed one and covers exactly the case autosave will not.
    pub(crate) recovery: crate::library::recovery::RecoveryState,
    /// The one background thread this app writes whole documents on — the crash
    /// snapshot and the autosave both (`crate::library::writer`, §15 D392, D393).
    ///
    /// **On the app rather than on `RecoveryState`, and it started there.** It
    /// was recovery's own while the snapshot was the only thing off the frame;
    /// autosave joining it made that a naming lie *and* the wrong shape — the
    /// point of one queue is that the two cannot race for a path, which is a fact
    /// about the app and not about either feature. (It read *"a path or a temp
    /// file"* until §15 D548 made the temp file per-write; the path half stands.)
    ///
    /// **`None` until the first job.** A session that never dirties a document
    /// never starts a thread — which is most headless probes — and *no writer* is
    /// a fact the callers use: it means nothing this session queued can be in
    /// flight, so a snapshot removal has nothing to be ordered against and can go
    /// straight to the filesystem.
    pub(crate) writer: Option<crate::library::writer::Writer>,
    /// In-progress rename from the dashboard's file menu (path + buffer).
    /// The document being renamed inline on the library screen, its buffer, and
    /// whether the field has already been handed the caret.
    ///
    /// ⚠️ **The third element is a latch and the rename is unusable without it.**
    /// A field that asks for focus on every frame it draws re-takes the caret in
    /// the same pass that `lost_focus()` would have been read in, which erases the
    /// signal: Enter and a click elsewhere both surrender focus, both are
    /// immediately given it back, and neither ever commits. See
    /// `OndinApp::rename_field`; it is the same fault `NewProject::name_focused`
    /// records, in the one field that was left behind when that one was fixed.
    pub(crate) rename_entry: Option<(std::path::PathBuf, String, bool)>,
    /// The dashboard's own view state — which entry, which sort, which menu.
    pub(crate) dash: crate::panels::dashboard::DashboardState,
    /// Rendered document pictures for the grid's cards
    /// (`crate::library::cover`).
    ///
    /// **Beside `thumbs` rather than inside it**, and the two are not the same
    /// question however alike they look. `ImageThumbs` cuts a *picture the
    /// document already holds* down to a layer row's square; this one renders
    /// the whole document through the headless raster path. They differ in what
    /// they key on, what they cost, and whether the answer survives a restart —
    /// this one has a disk cache and that one has no business having any.
    pub(crate) covers: crate::library::cover::Covers,
    /// Whether the library's Settings modal is up.
    ///
    /// **A separate flag from [`Self::settings`], because they are separate
    /// modals.** The editor's answers questions about editing — the nudge step,
    /// the layer tree, web fonts — none of which mean anything on a screen with
    /// no document open; this one answers where the library lives and how it
    /// opens. One modal holding both would put four irrelevant sections behind
    /// every visit.
    ///
    /// Holds a *draft* while it is up rather than a `bool`, because Save is what
    /// applies it — see [`crate::panels::dashboard::LibrarySettings`].
    pub(crate) library_settings: Option<crate::panels::dashboard::LibrarySettings>,
    /// What the last library migration left behind, if it left anything
    /// (§15 D810).
    ///
    /// 🚨 **The status line named one file and nothing named the rest.**
    /// `library::relocate::Moved::failed` has held every stranded path since §15
    /// D620, and `Moved::summary` puts the *first* of them in a sentence that is
    /// gone at the next status message — so a migration that stranded twenty-three
    /// documents was, a minute later, unrecoverable without hunting the old folder
    /// by hand. `apply_library_settings` re-points the app at the new root whether
    /// the migration succeeded, partly succeeded or did nothing, which is what
    /// makes the loss permanent rather than a retry away.
    ///
    /// **Both roots, not just the list**, because the re-run needs them: the app
    /// is already pointing at the new library by the time this is read, so the
    /// source is not derivable from anything else on `OndinApp`.
    ///
    /// **Session state and not a preference.** It is cleared by a clean migration
    /// and by a successful retry, and it does not survive a relaunch — the files
    /// do, and the old folder is named in the modal for as long as anything is
    /// still in it. A stranded list written to `prefs.json` would be a second copy
    /// of a fact the filesystem already holds, and the copy is the one that rots.
    pub(crate) stranded: Option<crate::panels::dashboard::Stranded>,
    /// Whether a chrome widget held the keyboard at the **end of the last
    /// frame** — the one thing [`Self::escape`] cannot ask egui for (§15 D821).
    ///
    /// 🚨 **`Escape` out of a chrome text field was spent twice.** egui clears
    /// focus in `Memory::begin_pass`, so by the time `input::resolve` runs the
    /// focus is already gone and the key resolves to `Action::Escape` — which is
    /// §15 D317's own reading, and is how a *valved* field's cancel reaches
    /// [`Self::cancel_gesture`] at all. For a plain `TextEdit` there is no preview
    /// to find, so the ladder fell straight through it: renaming a layer in the
    /// inspector and pressing `Escape` abandoned the rename **and cleared the
    /// selection**, which is the ladder's last rung.
    ///
    /// **Recorded rather than asked, because egui has no global question for
    /// it.** `Memory::had_focus_last_frame` takes an `Id` and the ladder has none
    /// — there is no widget to name, the whole point being that *something*
    /// had the keyboard. So this is written at the end of each frame, beside the
    /// `Tab` surrender that is there for the same class of reason.
    ///
    /// ⚠️ **"Anything focused", not "a text field".** That is `egui_wants_keyboard_input`'s
    /// own definition (§15 D123) and it is deliberately not narrowed: a button
    /// reached by the focus ring also spends `Escape` here, which is the reading
    /// the key already has everywhere — *drop what has the keyboard* — and
    /// narrowing it would need a widget kind egui does not report.
    pub(crate) chrome_focus: bool,
    /// Whether an **open** popover's own `Escape` handler ran this frame —
    /// written by the handler, read (and cleared) at the top of the next frame by
    /// the key router (§15 D847, `[X6-L1-01]`).
    ///
    /// 🚨 **The gate used to read the popover's flag, and a flag is not a
    /// popover.** [`Self::a_popover_owns_escape`] reads five plain fields, and
    /// every handler that clears one sits in a panel that can be skipped:
    /// `type_menu_popup` only for a text selection, the stroke and effect
    /// popovers inside their rows' loops, the Export card's inside the card —
    /// and all five behind `if !self.present`. So a Type popover left open while
    /// the user selected a rect, or while they entered present mode, **swallowed
    /// `Escape` for the rest of the session** with nothing on screen to explain
    /// it — in present mode the one key the mode's own toast names as the way
    /// out. §15 D801 met this exact state as a *fixture* hazard (*"a popover that
    /// nothing is drawing answers every key the same way, which is
    /// indistinguishable from ignoring them"*) and never asked it of production.
    ///
    /// **One frame late, which is `chrome_focus`'s shape and for its reason**:
    /// the router runs at the top of the frame and the handlers run inside the
    /// panels, so the only thing the router can know is what happened last time.
    /// A popover drawn last frame is drawn this one; a flag with no handler behind
    /// it last frame has nothing behind it now.
    pub(crate) popover_heard: bool,
    /// In-progress name edit (node id + buffer), so typing persists across
    /// frames and commits once, on defocus.
    pub(crate) name_edit: Option<(NodeId, String)>,
    /// System and web fonts, loaded into core on demand.
    pub(crate) fonts: FontService,
    /// Family names rasterized in their own face, for the picker's rows.
    pub(crate) previews: crate::fonts::FontPreviews,
    /// Pictures cut down to a layer row's square, for the layers tree and the
    /// paint row's chip (§5.5a).
    ///
    /// **Beside `previews` rather than on `CanvasRenderer`**, for the reason
    /// `thumbs.rs` opens with: the pixels are the store's and the texture is the
    /// chrome's, and `CanvasRenderer::images` is a read-only handle on purpose.
    pub(crate) thumbs: crate::thumbs::ImageThumbs,
    /// Substring filter for the font picker (thousands of families otherwise).
    pub(crate) font_filter: String,
    /// In-progress pen path (world-space anchor points), or `None`.
    pub(crate) pen: Option<PenState>,
    /// Whether the pen's verbs are armed **inside** the node tool's edit — what
    /// `P` means while a path's points are the subject (§15 D125).
    ///
    /// **A bias, not a mode and not a tool switch.** The complaint it answers is
    /// that the pen was unreachable from inside an edit: `Tool::Pen` and
    /// `Tool::Node` are siblings on the rail, so pressing `P` was an ordinary tool
    /// switch and dropping the edit is exactly what a tool switch means. This
    /// leaves `tool` at `Tool::Node` — the selection, the point set and the chrome
    /// all survive — and changes only what a press on the canvas *means*
    /// (`canvas::pen_verb`).
    ///
    /// It is not a `Mode`: `input::resolve` never sees it, so no key is rerouted
    /// and no shortcut changes meaning. What announces it is the pen's own cursor,
    /// over exactly the targets where a pen verb is armed, and the pen's own rail
    /// button lit beside the node tool's.
    ///
    /// Read only through [`Self::pen_scope`], which is also what confines it to one
    /// node: everything the bias does happens inside the path being edited.
    pub(crate) pen_bias: bool,
    /// Images picked but not yet placed — what a **loaded cursor** is holding
    /// (`Tool::Image`).
    ///
    /// In pick order, placed front-first, and the tool hands back to Select when
    /// it empties. Held here rather than inside `Tool::Image` because a tool is a
    /// `Copy` enum that every match on it would have to carry, and because this
    /// outlives no gesture: picking files is one event, placing them is several.
    ///
    /// **The bytes are already decoded by the time they land here** — the file
    /// dialog reads and hashes them, and the decode is what supplies the intrinsic
    /// size the fit rule needs. A file that would not decode never becomes an
    /// entry, so nothing in this list can fail to place.
    pub(crate) pending_images: Vec<LoadedImage>,
    /// The text node currently being edited in-canvas.
    pub(crate) text: Option<TextSession>,
    /// Points selected inside a path, for the node tool — a third kind of
    /// subject, beside the layer selection and the guide selection (§15 D118).
    pub(crate) points: PointSet,
    /// Layer-tree rows the user has collapsed (default: everything expanded).
    pub(crate) collapsed: HashSet<NodeId>,
    /// The first of a possible two opacity digits, and when it was typed.
    ///
    /// `None` whenever no digit is waiting, which is almost always — the window
    /// is 600 ms. See [`OndinApp::opacity_digit`].
    pub(crate) opacity_entry: Option<OpacityEntry>,
    /// A layer row being renamed in place, and the text typed so far.
    ///
    /// Buffered rather than written per keystroke, like the paint hex field: one
    /// rename is one undo step, not one per character.
    pub(crate) renaming_layer: Option<(NodeId, String)>,
    /// The selection the layers tree last reacted to.
    ///
    /// The tree has two things to do when the selection changes *elsewhere* —
    /// open whatever the new selection is buried inside, and scroll to it — and
    /// neither should fire on the frames in between. Comparing against this is
    /// how the panel notices, since nothing else tells it.
    pub(crate) layers_synced_selection: Vec<NodeId>,
    /// Whether the tree still owes the selection a scroll into view.
    pub(crate) scroll_layers_to_selection: bool,
    /// The row a Shift-click measures its range from: the last one clicked
    /// *without* Shift. Deliberately not moved by a Shift-click itself, so a
    /// range can be stretched and shrunk from one end.
    pub(crate) layers_range_anchor: Option<NodeId>,
    /// Layer-name filter. `Some` while the search field is open, even when
    /// empty — closing it is what clears the filter, so a search can be emptied
    /// and retyped without the tree jumping back to its collapsed state.
    pub(crate) layer_filter: Option<String>,
    /// A layer row being dragged to reorder or reparent it.
    pub(crate) layer_drag: Option<LayerDrag>,
    /// Whether the panel's drag edge is lit, and since when (§15 D350).
    pub(crate) layers_edge: EdgeDwell,
    /// Set when the window manager asked to close and there are unsaved edits.
    pub(crate) confirming_close: bool,
    /// Canvas size in device pixels as of the last frame — zoom-to-fit needs it.
    pub(crate) canvas_px: (u32, u32),
    /// The display scale the canvas was last laid out at, `pixels_per_point`.
    ///
    /// **What turns a pointer allowance into world units** (§15 D853,
    /// `[X5-L1-03]`). `camera.zoom` is *device pixels* per world unit, because
    /// the canvas renders in device pixels, while every `*_PX` allowance — a
    /// handle's grab, a hairline's pick band, the pen's snap — is aimed at by a
    /// pointer that egui reports in logical points. Divided by the zoom alone, a
    /// 4-point band was 2.67 points at 150% and 2 at 200%, while the layer
    /// handles, measured in screen points, never moved. See
    /// `OndinApp::points_per_world`.
    pub(crate) canvas_ppp: f32,
    /// The canvas widget's rect as of the last frame, in screen points.
    ///
    /// **Last frame's, and that is the honest version of the question it answers.**
    /// [`ChromeHold`] asks whether the pointer moved *over the artwork*, and it asks
    /// before the canvas is laid out — the canvas is drawn ahead of the inspector,
    /// so the rect that exists when the question is asked is the previous frame's.
    /// It only moves when a panel is toggled or the window resizes, and either of
    /// those is a frame nobody is mid-edit in.
    pub(crate) canvas_rect: egui::Rect,
    /// Why the selection chrome is being held back while the inspector is edited.
    pub(crate) chrome_hold: ChromeHold,
    /// An inspector edit noticed since the last [`ChromeHold`] fold, and whether the
    /// gesture making it is a **drag still in flight**.
    ///
    /// Written by [`Self::edit_valve`] and [`Self::commit_edit`], taken once a frame
    /// by `fold_chrome_hold`. Plain app state rather than a note in egui's `data`
    /// map: both writers are methods on this struct, so there is nothing to pass it
    /// through.
    pub(crate) pending_edit: Option<bool>,
    /// How far `canvas::autopan` has scrolled the view during the gesture in
    /// flight, in **world** units — the amount added to `Camera::center`.
    ///
    /// Kept so cancelling can put the view back: right-click abandons the drag and
    /// the layer snaps home, and a view left where the auto-scroll had carried it
    /// would leave the user looking at empty page with no idea which way to go.
    ///
    /// A running total rather than a snapshot of the camera, so a deliberate zoom
    /// or a wheel-pan *during* the drag survives the cancel — only the scrolling
    /// this feature did is undone.
    pub(crate) autopan_total: Vec2,
    /// Inspector panels the user has collapsed, keyed by title. Each right-hand
    /// panel is independent (`design/Editor.dc.html`), and the state is keyed by
    /// title rather than by node so it survives changing the selection.
    pub(crate) collapsed_panels: HashSet<&'static str>,
    /// Rects whose Appearance panel is showing the four per-corner radius
    /// fields. Purely a disclosure: the model always holds four radii, and the
    /// single field above edits all of them, so nothing here changes what a
    /// shape *is* — which is why it is view state and not saved with the file.
    pub(crate) per_corner_radius: HashSet<NodeId>,
    /// The run of Ctrl+D duplicates in progress, if any.
    pub(crate) clones: Option<CloneChain>,
    /// The group the user has stepped into by double-clicking it, if any.
    ///
    /// Purely a *selection* scope: while it is set, a click picks something
    /// inside that group rather than the group itself, and a click on anything
    /// outside it clears the scope again. Nothing about the document changes, so
    /// there is deliberately no affordance for it — Figma has none either, and
    /// what the click selects says plainly enough where you are.
    pub(crate) entered_group: Option<NodeId>,
    /// A gesture the user cancelled, still holding the button down.
    ///
    /// Cancelling cannot end the press — the pointer is the user's, not ours —
    /// so every path that would turn a release into an edit has to know the
    /// gesture is already over. Cleared once nothing is held any more, which
    /// is why it is checked at the *end* of the frame: the release and the
    /// suppression it needs happen in the same one.
    pub(crate) gesture_cancelled: bool,
    /// Every value a gesture is holding while it runs, in one type so that
    /// "is anything in flight?" cannot go stale — see [`InFlight`] (§15 D785).
    ///
    /// ⚠️ **A live text session's spans are in there**, and the reason they are a
    /// *snapshot* rather than a preview is worth keeping in view from here: a
    /// field scrubbed while a session owns the range writes into the **editor**
    /// rather than into a preview (§9.3 — the session is the truth until it ends),
    /// so `cancel_gesture` has no preview to drop for it and there is nothing else
    /// to read the old value out of. Reported as right-click resetting the text
    /// but not the field, which was `clear_preview` hiding the write rather than
    /// undoing it.
    pub(crate) in_flight: InFlight,
    /// The detached colour picker, when one is open.
    pub(crate) picker: Option<Picker>,
    /// A picker the `+` on a Fill or Stroke header asked for, waiting for the
    /// row it belongs to to be laid out.
    ///
    /// The `+` has no row to anchor against: it creates one, and the panel is
    /// drawn before the paint that was just added exists. Anchoring to the
    /// panel instead put the picker a header's height too high. So the request
    /// is parked here and [`Self::paint_row`] claims it on the next frame,
    /// using the same row anchor a click on the swatch would have used.
    pub(crate) pending_picker: Option<(NodeId, PaintSlot)>,
    /// A paint row's hex field mid-edit (which row, and the half-typed text), so
    /// a colour can be typed or pasted without the value being reparsed — and
    /// recommitted — on every keystroke.
    pub(crate) paint_hex: Option<(NodeId, crate::panels::PaintSlot, String)>,
    /// What the Transform card's **Scale** field is showing, as a percentage
    /// (§15 D311).
    ///
    /// **It lives here because it has nowhere else to live, and it is 100 again
    /// the moment an edit commits.** Every other field in that card reads its
    /// value back off the document — a width is a width — but a scale is not
    /// stored anywhere: `Resize` writes geometry, so a rect dragged to twice its
    /// size has an authored size of 200 and a basis still at 1.0, and there is no
    /// number to decompose. So the field is a *verb* with a live readout rather
    /// than a property editor: it shows what is being typed or scrubbed while the
    /// gesture is in flight, applies it against the committed box every frame, and
    /// returns to 100 when the valve closes — because by then the box *is* the new
    /// size and 100% of it is where the next edit starts from.
    pub(crate) scale_pct: f64,
    /// Paint panels this has already collapsed for being empty, so it does not do
    /// it twice.
    ///
    /// An empty Fill or Stroke panel is a header, a `+`, and a line of grey text
    /// saying what the header already says — so it starts collapsed, and adding a
    /// paint opens it. The set is what keeps that from becoming a *rule*: without
    /// it, re-collapsing every frame would make expanding an empty panel by hand
    /// impossible, since the click would be undone before it was drawn. Acting only
    /// on the transition into empty leaves the user's own toggle the last word.
    pub(crate) paint_auto_collapsed: HashSet<&'static str>,
    /// The selection the Fill and Stroke panels last set their open state from.
    ///
    /// Whether a layer has a fill is a fact about *that layer*, so the panels are
    /// re-read from scratch whenever the subject moves rather than inheriting the
    /// last one's state. Nothing else tells them the selection changed — the same
    /// gap `layers_synced_selection` fills for the tree. See
    /// `OndinApp::paint_subject_changed`.
    pub(crate) paint_synced_selection: Vec<NodeId>,
    /// The stroke whose options menu (dash, cap, join) is open, if any.
    ///
    /// Keyed by index into the list the Stroke panel is showing, and at most one
    /// at a time: two of these menus overlapping would leave the one behind
    /// unreachable, which is the same reason the top bar's dropdowns are one
    /// field rather than three flags.
    pub(crate) stroke_menu: Option<usize>,
    /// The effect row whose popover is open — the layer it belongs to, and its
    /// index in that layer's stack (§5.3a).
    ///
    /// **Keyed by node as well as by index, where [`Self::stroke_menu`] is a bare
    /// index.** The two are not the same situation: the stroke panel is handed a
    /// `fresh` edge every frame and clears its chrome through
    /// `forget_stroke_chrome`, and that edge is *consumed* by whichever panel asks
    /// for it first — which is why the export panel already keeps a synced
    /// selection of its own rather than a third caller losing the transition. A
    /// node in the key answers the same question without a fourth copy of that
    /// machinery: an index that names a different layer's effect simply does not
    /// match, so the popover cannot be left editing something it is not pointed at.
    ///
    /// At most one, for `stroke_menu`'s reason: two of these `Area`s overlapping
    /// would leave the one behind unreachable.
    pub(crate) effect_menu: Option<(NodeId, usize)>,
    /// Which tab of image editing's card is showing.
    ///
    /// **Not an `Option`, and not keyed by anything**, which is the whole of what
    /// §15 D268 changed here. It used to be `Option<(NodeId, PaintSlot,
    /// ImageCard)>` — *is a card open, on which layer, on which paint, and which
    /// of the two* — and every one of those questions now has an answer somewhere
    /// else: the card is open exactly while `Tool::ImageEdit` is, and the layer
    /// and the paint are `canvas::edited_image`'s to say. So the housekeeping went
    /// with it: nothing has to clear this when the selection changes, remap it
    /// through a paint reorder, or drop it when a row is dragged, because a stale
    /// value cannot exist.
    ///
    /// What is left is a preference — which tab you were last on — and it is kept
    /// across entries deliberately: a user adjusting the exposure on six pictures
    /// in a row should not land on Settings six times.
    pub(crate) image_tab: ImageCard,
    /// The stroke whose sides selector is showing, if any.
    ///
    /// A `Vec` of indices rather than an `Option` like `stroke_menu`, because this
    /// is a *row in the card* rather than a popup: two of them showing at once are
    /// two rows one above the other, which is fine, where two overlapping `Area`s
    /// would leave the one behind unreachable.
    ///
    /// **The only thing that decides whether the row is showing.** It briefly was
    /// not: the row was also forced open whenever `sides != All`, on the reasoning
    /// that a control carrying a value must not be hideable — which left the button
    /// lit beside a row it could not close. The third button state
    /// ([`crate::ui::FieldButton::Set`]) is what replaced that: a collapsed row
    /// still declares its setting, through the button's border, so the row itself is
    /// free to close (§15 D72).
    ///
    /// So this is chrome, but no longer *pure* chrome: losing it — on a reorder, a
    /// removal, a selection change, via `forget_stroke_chrome` — collapses a row that
    /// may have a value in it. What still holds is the guarantee that matters, that
    /// it can close a row and can never lose a setting.
    pub(crate) stroke_sides_open: Vec<usize>,
    /// Settings that outlive the session — export presets, the last destination
    /// folder, the two export switches, the nudge step, the layer tree's
    /// fold-on-open and the web-font switch. Held rather than re-read, because the
    /// export panel asks for them every frame.
    pub(crate) prefs: crate::prefs::Prefs,
    /// The Settings modal's staged edits while it is open (`crate::settings`).
    ///
    /// **`Some` *is* the modal being open**, and the draft has nowhere else to live
    /// — which is the whole reason it is not a `bool` beside a copy of `Prefs`. A
    /// form whose openness and whose contents were two fields could be open with no
    /// contents, or hold an edit nobody can see.
    pub(crate) settings: Option<crate::settings::Settings>,
    /// The selection the Export panel last read its resting collapse state for.
    ///
    /// **Its own field rather than [`Self::paint_synced_selection`]**, though the
    /// two hold the same thing: `paint_subject_changed` *consumes* the edge, so a
    /// second caller in the same frame gets `false` and one of the two panels
    /// never learns the subject moved. Which of them lost would depend on the
    /// order they happen to be drawn in.
    pub(crate) export_synced_selection: Vec<NodeId>,
    /// The selection the Effects panel last read its resting collapse state for.
    ///
    /// The third of these, and a third field for the second field's reason:
    /// `paint_subject_changed` **consumes** the edge, so the panel that asks second
    /// never learns the subject moved. Fill and Stroke share one answer because one
    /// caller asks for both of them; Export and Effects are each drawn on their own.
    pub(crate) effects_synced_selection: Vec<NodeId>,
    /// The selection the Layout grid panel last read its resting collapse state
    /// for.
    ///
    /// **The fourth of these, and the count is the finding rather than the field.**
    /// Every panel that closes itself when empty needs one, because the edge is
    /// consumed by whoever asks first — so the number of these grows with the
    /// number of self-collapsing panels and always will. Worth merging into one
    /// map keyed by panel title the next time a fifth is wanted; four is not yet
    /// worth the indirection.
    pub(crate) grids_synced_selection: Vec<NodeId>,
    /// The export spec whose settings block is showing, if any.
    ///
    /// An `Option` rather than the `Vec` [`Self::stroke_sides_open`] is: the block
    /// carries five controls where the sides selector carries one row, so two open
    /// at once would push the export button off the bottom of a panel that is
    /// already the longest in the inspector. One at a time is the same call
    /// `stroke_menu` makes, for the neighbouring reason.
    pub(crate) export_row_open: Option<usize>,
    /// Whether the Export panel's own menu — presets, copy/paste, the folder rule
    /// — is showing.
    pub(crate) export_menu: bool,
    /// The export settings *Copy export settings* is holding.
    ///
    /// **Its own slot, not [`Self::property_clipboard`]**, which carries fills,
    /// strokes and opacity — an appearance. An export set is not an appearance and
    /// pasting one alongside a colour would be two verbs behind one row; keeping
    /// them apart is also what lets a paste of either survive the other being
    /// replaced.
    pub(crate) export_clipboard: Option<Vec<ondin_core::ExportSpec>>,
    /// The Export panel's rendered preview, and what it was built from.
    ///
    /// **One slot, not a cache.** The section shows one spec of one layer, so a
    /// map keyed by anything would hold exactly one live entry and a growing pile
    /// of dead ones — this is the shape `thumbs.rs` has to sweep for and this one
    /// does not, because a new key simply replaces the old.
    pub(crate) export_preview: Option<crate::panels::ExportPreview>,
    /// The export prefix being typed, and which spec it belongs to.
    ///
    /// Buffered like [`Self::stroke_dash_text`] and for its second reason alone:
    /// every keystroke would otherwise be an undo step. There is no half-valid
    /// state to protect here — any string is a prefix — so this exists to make
    /// typing one name one edit.
    pub(crate) export_prefix_text: Option<(usize, String)>,
    /// The export suffix being typed, and which spec it belongs to. Its own
    /// buffer, not a shared one keyed by part: both fields are on screen at once.
    pub(crate) export_suffix_text: Option<(usize, String)>,
    /// The custom dash list being typed, and which stroke it belongs to.
    ///
    /// Buffered rather than parsed per keystroke, exactly as `paint_hex` is and for
    /// the same two reasons: typing "5, 10" passes through `5` — a real, different
    /// pattern — on the way, and each keystroke would otherwise be its own undo
    /// step. The text is kept verbatim so a half-typed "5, 1" does not reformat
    /// itself under the caret.
    pub(crate) stroke_dash_text: Option<(usize, String)>,
    /// Which tab of the typography popup is showing, or `None` when it is closed.
    ///
    /// The tab travels with the open/closed state for the same reason the picker's
    /// gradient face does: reopening should land where the user left off within a
    /// session, and a separate "last tab" field would have to be kept in step with
    /// a popup that may never have been opened.
    pub(crate) type_menu: Option<TypeTab>,
    /// The OpenType feature filter being typed in the Character tab, if any.
    pub(crate) feature_filter: String,
    /// Whether the Character tab's OpenType list is showing the features the
    /// registry says a user should not be choosing — required ones, the ones a
    /// shaper applies per script, the vertical-writing ones.
    ///
    /// **Off by default and per-session, not per-document.** It is a way of
    /// looking at the list rather than a property of anything in the file: the
    /// features it reveals are switchable either way (`CharAttr::Features` takes
    /// any tag), so nothing about a saved document depends on it.
    pub(crate) show_all_features: bool,
    /// A character colour's hex field mid-edit — **which row, and the text in it**.
    ///
    /// Buffered rather than parsed per keystroke, exactly as `paint_hex` is: half
    /// of `9184D9` is a different, legal colour, and committing on the way through
    /// would both recolour the text four times and leave four undo steps behind.
    /// `None` means every such field is showing its stored colour rather than a
    /// half-typed value.
    ///
    /// **Keyed by the slot, because the Character tab now shows two of these rows at
    /// once** — the text's own colour and a decoration's (§15 D154). One shared
    /// buffer would put the digits being typed into one row into the other.
    pub(crate) char_hex: Option<(crate::panels::CharSlot, String)>,
    /// A multi-selection Transform field being dragged: which one, and the value
    /// it has been dragged to.
    ///
    /// **The single-layer fields need nothing like this and this one cannot copy
    /// them.** Those read their number back through the render override, so the
    /// preview *is* the running total and each frame's edit is an absolute
    /// `SetTransform` computed from it. That works because the op is absolute and
    /// always emitted, so releasing commits exactly what is on screen.
    ///
    /// Neither half holds for a selection, because `multi_transform_tx` builds
    /// from the **committed** document every frame. A field that knew only this
    /// frame's *step* would commit the last step and lose the rest of the drag —
    /// and on the release frame the pointer has not moved since the previous one,
    /// so that step is zero. On the angle fields it is worse than losing the
    /// tail: `multi_angle_tx` drops the layers a nudge leaves alone, so a zero
    /// step is an **empty** transaction, `try_commit` takes an empty transaction
    /// as a no-op success, and `edit_valve` then clears the preview — the whole
    /// gesture disappears at the moment the button comes up.
    ///
    /// Holding the target here instead means every frame of the drag, the last
    /// one included, builds the *same* transaction from the same committed base.
    /// The preview and the commit are then the same edit by construction rather
    /// than by agreement (§15 D51).
    pub(crate) multi_transform_drag: Option<(crate::panels::MultiField, f64)>,
    /// The drawn canvas cursors, rasterized once (see `cursor.rs` for which ones
    /// the OS does not provide).
    pub(crate) cursors: Cursors,
    /// Whether the rulers are showing (Shift+R).
    ///
    /// View state, unlike the guides they place: whether the bars are on screen
    /// says nothing about the artwork, and the design's own default is on
    /// (`showRulers ?? true`). On by default here for the same reason — the
    /// screen that was designed has them.
    pub(crate) show_rulers: bool,
    /// Whether the guides are showing — *View ▸ Show guides*, `Ctrl+;`.
    ///
    /// View state beside `show_rulers`, not a property of the guides themselves:
    /// the guides are saved with the document (§5.5) and whether they are on
    /// screen right now is not. Every reader goes through
    /// [`OndinApp::guides_on`], for the reason [`OndinApp::rulers_on`] exists —
    /// the absence has to mean the same thing to the hit test as to the paint.
    ///
    /// ⚠️ **This is the whole of that predicate now, and it used to say the
    /// opposite** (§15 D750). It read *"which folds present mode in the same way
    /// `rulers_on` does"*; present mode leaves the guides on the canvas, so
    /// `guides_on` is this flag and nothing else while `rulers_on` keeps its
    /// `!present`. The bars are window furniture and a guide is not.
    pub(crate) show_guides: bool,
    /// Whether the guides are locked — *View ▸ Lock guides*, `Ctrl+Alt+;`.
    ///
    /// **A workspace switch, not a per-guide property**, which is Illustrator's
    /// and Photoshop's model (both on this same chord) and what stops the trap a
    /// per-guide lock creates: the control that unlocks a guide would live on the
    /// card you reach by selecting it, so locking the last guide would hide the
    /// way back. Here the toggle that locked them is the toggle that unlocks
    /// them, so nothing has to be reachable through a locked guide — which is
    /// what lets locked mean **not selectable at all**, in one early return in
    /// [`OndinApp::guide_at`].
    ///
    /// Defaults off: locking guides nobody has placed yet would be a mode with
    /// nothing in it. (Not *the* one that does — [`OndinApp::show_pivot`] and
    /// [`OndinApp::present`] are off too, and this line said otherwise until
    /// 2026-09-06.)
    pub(crate) lock_guides: bool,
    /// The guides being edited from the inspector, held as whole guides.
    ///
    /// The preview half of the valve for the one piece of document state the
    /// renderer never sees; see [`OndinApp::shown_guide`]. Not a `Drag`, because
    /// the inspector's scrub and a drag on the canvas can no more overlap than
    /// two drags can, but they are answered by different code.
    ///
    /// **A list, because an edit reaches every selected guide.** This was one
    /// `Option<Guide>` and `preview_guide` overwrote it per operation, so a
    /// two-guide transaction previewed only whichever guide came last: the other
    /// one still read its committed position, the two disagreed, and the
    /// inspector's field — which reports `Mixed` exactly when they disagree —
    /// flickered between the value and the word for the whole of a drag.
    pub(crate) guide_previews: Vec<Guide>,
    /// Which of the top bar's dropdowns is open, if any.
    ///
    /// One field rather than a `bool` each, because they are mutually exclusive
    /// on screen and the design models them that way too (its `openMenu` prop is
    /// an enum over `none | zoom | view | snap`). Three independent flags would
    /// admit a state where two menus overlap, and nothing would ever close the
    /// one behind.
    pub(crate) open_menu: TopMenu,
    /// The one open context menu (`docs/context-menus.md` §0 R4).
    ///
    /// **One slot, so opening the next one is what closes the last.** Not a
    /// dismissal rule that happens to fire first: a right-click somewhere else
    /// overwrites this field and the old menu is gone by construction, which is
    /// the shape [`TopMenu`] already has and the reason `toggle_menu` can promise
    /// that two dropdowns never overlap without a line of closing code.
    pub(crate) context_menu: Option<ContextMenu>,
    /// Where the secondary press that *may* open a menu landed
    /// (`docs/context-menus.md` §0 R1).
    ///
    /// **The menu opens at the press, not at the release**, so a click that
    /// jitters a few pixels does not shift the menu out from under the pointer.
    /// egui delivers `secondary_clicked` on the release and has already forgotten
    /// where the press was by then, so it is recorded here on the way past.
    pub(crate) secondary_press: Option<egui::Pos2>,
    /// The pixel grid's own switch — *View ▸ Show grid*. The grid also needs
    /// enough magnification to be worth drawing, so this is necessary and not
    /// sufficient; see `grid::VISIBLE_AT_ZOOM`.
    pub(crate) show_grid: bool,
    /// The layout grids' workspace switch — *View ▸ Show layout grid* (§15
    /// D755). Every frame's column and row bands at once, on or off.
    ///
    /// **Three gates and this is the outermost.** A band draws only while its own
    /// grid's eye is on (`ondin_core::layout::LayoutGrid::visible`) and only on a
    /// frame `shown_visible` admits; this one is above both, and is the switch
    /// §15 D385 declined and D750 reopened. It is deliberately *not* per frame:
    /// the question a designer asks is *"let me see this without the grid"* about
    /// the drawing, not about one frame in it, and the per-grid eye already
    /// answers the narrower one.
    ///
    /// ⚠️ **Not read by anything that snaps.** Nothing snaps to a layout grid —
    /// `ViewSwitch::SnapGrid` is the *pixel* grid's — so unlike `show_guides`,
    /// which `rulers::guides_on` and the snapping both consult, this has exactly
    /// one reader and hiding the bands cannot change where anything lands.
    ///
    /// Per session and not in `Prefs`, which is what every other View switch does:
    /// `show_grid`, `show_rulers` and the rest are all constructor defaults.
    pub(crate) show_layout_grids: bool,
    /// *View ▸ Show layers* and *Show toolbar*: whether the left pane and the
    /// tool rail are on screen.
    pub(crate) show_layers: bool,
    pub(crate) show_toolbar: bool,
    /// *View ▸ Present mode*: the app's chrome hidden at once — top bar, layers
    /// pane, tool rail, inspector, picker and the ruler bars — leaving the
    /// artwork **and a canvas that goes on working normally**. Escape comes back.
    ///
    /// 🚨 **It is a chrome switch and not an overlay switch** (§15 D750). This
    /// doc read *"every piece of chrome hidden at once, leaving the artwork and
    /// nothing else"*, which was never true of the code: guides, layout grids,
    /// the selection box, the handles, the size badge, the hover outline and the
    /// frame name tags all stay, and `[S12.5-L2-01]` measured the two that did
    /// not against about a dozen that did. The maintainer's ruling is that the
    /// survivors are right — the mode is for editing a large design with the
    /// panels out of the way, or on a laptop screen the panels eat half of, and a
    /// designer doing that still wants what they are aligning to.
    ///
    /// Not another independent toggle beside the others — it overrides the ones
    /// it reads for as long as it is on and leaves them exactly as they were, so
    /// leaving present mode restores the workspace the user had rather than a
    /// default one. ⚠️ **It reads three of the other six View rows** — `Rulers`,
    /// `Layers` and `Toolbar` — where this doc claimed *"all six"*; `Grid` (the
    /// pixel lattice) and `GuideLock` were never suppressed at all, and `Guides`
    /// stopped being with D750. Deliberately not stated as an ordinal ("a fourth
    /// toggle", which it was when there were three): the count moves every time a
    /// row is added, and what matters is that this one sits *over* the set rather
    /// than *in* it.
    pub(crate) present: bool,
    /// Whether the transform-origin marker is editable — the Transform panel's
    /// crosshair.
    ///
    /// **Sticky and app-wide, not per layer.** Moving pivots is a way of working
    /// (rigging a shape so it swings from a corner), not a property of the thing
    /// being worked on: someone who turns it on is about to place several, and
    /// having it reset with the selection would mean re-arming it for each. The
    /// pivots themselves are per-node and saved; this is only whether the canvas
    /// hands you the handle for them, which is why it lives here beside the other
    /// view switches and not in the document.
    ///
    /// Escape turns it off, after cancelling any gesture — see [`Self::escape`].
    pub(crate) show_pivot: bool,
    /// The four *Snap* menu toggles. Resolved into snap sources by
    /// [`OndinApp::snapping`], which is the only place that reads them.
    pub(crate) snap_shapes: bool,
    pub(crate) snap_guides: bool,
    pub(crate) snap_grid: bool,
    /// Text baselines (§15 D355).
    ///
    /// **Its own switch rather than a refinement of *Snap to shapes*.** Every line
    /// of every text node in view is a candidate, so on a page of copy this is an
    /// order of magnitude more magnetism than the other three put together — and
    /// someone who wants edges and centres without it has no other way to say so.
    /// On by default: the feature is pointless off, and a switch is how we find
    /// out whether it is noise.
    pub(crate) snap_baselines: bool,
}

/// Which top-bar dropdown is showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TopMenu {
    #[default]
    None,
    View,
    Snap,
    Zoom,
}

/// The system clipboard's text, if it holds any.
///
/// **Read directly, because `Event::Paste` only arrives when `Ctrl+V` is
/// pressed.** A context-menu row is app code, not a keystroke, so a row that
/// waited for the event would never run — and the same read decides whether that
/// row is offered at all ([`ContextMenu::system_text`]). Same reason
/// [`OndinApp::paste_image`] opens `arboard` itself (§15 D183); this is the text
/// half of that dependency.
///
/// `None` rather than an empty string for an empty clipboard, so callers cannot
/// paste nothing and call it a paste.
pub(crate) fn system_clipboard_text() -> Option<String> {
    if !clipboard_is_reachable() {
        return None;
    }
    let text = with_clipboard(|c| c.get_text().ok())??;
    (!text.is_empty()).then_some(text)
}

/// Every `arboard` handle **this crate** opens is opened under this, and it is not
/// a tidiness measure (§15 D796).
///
/// ⚠️ **"This crate", not "the process" — this said the process, and there is a
/// second handle** (§15 D859, `[X2-L2-01]`). `egui-winit` depends on `arboard`
/// and keeps its own long-lived `Clipboard`, which serves `ctx.copy_text` —
/// `stamp_clipboard`'s route onto the clipboard, and the one that makes
/// `Event::Paste` fire — and it never passes through `GATE`. It is tolerated for
/// one reason, and the reason is what to re-check before relying on this gate:
/// **both handles are driven from the one UI thread**, so the concurrent open
/// measured below cannot happen in the app. A change that moves a clipboard read
/// onto a worker reopens it with this doc's protection not covering it. (That
/// `arboard`'s Windows backend opens the OS clipboard per operation, so the
/// second live handle is also inert on one thread, is read from its source
/// rather than measured.)
///
/// 🚨 **Two threads opening the clipboard at once corrupt the heap.** Measured on
/// Windows with four `#[test]`s doing nothing but calling the two readers below
/// in a loop — no app, no egui, no document: `STATUS_HEAP_CORRUPTION`, four runs
/// in four, and green the moment the harness is given one thread. `arboard`'s
/// Windows path opens the *global* clipboard, which is a per-process resource
/// with no interior locking, so a second `Clipboard::new()` while the first is
/// live is not a race we are entitled to lose gracefully.
///
/// ⚠️ **The app has one UI thread, so this is not a bug a user can reach today.**
/// What it was reaching is the **test suite**: `ContextMenu` snapshots the
/// clipboard on every open (`system_text`, `system_image`), so any two tests that
/// open a menu could land on it together. The whole suite passed throughout —
/// its tests spread thinly enough that two rarely overlapped — and four new
/// context-menu tests in one module failed four runs in five. **A suite that
/// passes because its tests are spread out is passing by luck**, and the luck
/// was already being spent before those four existed.
///
/// ⚠️ **Poisoning is deliberately ignored.** A panic under this lock says nothing
/// about the OS clipboard's state, and refusing every later paste because one
/// earlier one panicked would turn a transient failure into a permanent one.
fn with_clipboard<T>(f: impl FnOnce(&mut arboard::Clipboard) -> T) -> Option<T> {
    static GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _held = GATE.lock().unwrap_or_else(|e| e.into_inner());
    let mut clipboard = arboard::Clipboard::new().ok()?;
    Some(f(&mut clipboard))
}

/// Whether the OS clipboard is off limits — set once by `OndinApp::headless`
/// and never cleared (§15 D798).
///
/// (Plain backticks: `headless` is `cfg(test)`, so a production doc cannot link
/// it and the doc gate exits 101 on one that tries — §15 D319. This link was
/// written as a link and caught by that gate, which is the gate working.)
///
/// 🚨 **The gate caught this one because the item is *production*. For a link
/// inside a `cfg(test)` module nothing catches it, and the re-check that was
/// supposed to has never been able to** (§15 D827, 2026-09-22). `CLAUDE.md`
/// carried a ``git diff … | grep -E '^\+ *(///|//!).*\[\`'`` for exactly that
/// blind spot and credited it with a catch per session for six sessions. **In
/// GNU grep's ERE a backslash before a backtick matches nothing**, so that
/// command returns no hits on any input whatever — measured against a one-line
/// fixture that the same pattern minus the backslash matches. It is the
/// `grep 'cfg(debug'` shape from §15 D732 again: a check whose output cannot
/// disagree with the claim it is printed under.
///
/// 🚨 **Two more of that file's checks were broken too**, which is why D827 is
/// about the class rather than this grep. `review/`'s census defends against a
/// count reading *high* by taking a second extraction and comparing; the
/// documented pattern returns **0** where the corrected one returns **394**, so
/// it asserted *"nothing to correct for"* and **corroborated** the high count.
/// And §15.0's order-check piped `- **D110**` through `sort -n`, which parses no
/// number and falls back to a whole-line comparison — **194 differing lines on
/// an index in perfect order**, which is how a check gets abandoned. ⚠️ **All
/// three were found by *running* them, none by reading** — the patterns look
/// right. **Ten of that file's commands are controlled now and the sweep is
/// unfinished.** The backslash is gone from both greps and the order-check has
/// its `sed`; what is unchanged is that this doc convention has no gate behind
/// it and is kept by reading.
///
/// 🚨 **A headless app read and could write the developer's real clipboard.**
/// D303 swaps three things so a probe cannot touch the machine it runs on — the
/// wgpu device, `FontService`'s five background threads and the preferences file
/// — and this was a fourth it did not name. `§11`'s *"a test may not write
/// outside the repository"* held on this path by test discipline rather than by
/// construction: nothing stopped a test of `copy_as_png` from replacing whatever
/// the developer had copied, and `owns_the_clipboard` was already reading it.
///
/// **A process-wide flag rather than a field, because the clipboard is a
/// process-wide resource.** The two readers are free functions with eleven call
/// sites across three modules, none of which is a natural place to thread a flag
/// through; and the lock above is already a static for exactly the same reason —
/// there is one OS clipboard per process, so "do not touch it" is a fact about
/// the process, not about an `OndinApp`. ⚠️ It is deliberately **one-way**: a
/// test that could turn it back off could turn it off for every other test in
/// the binary, which is the property this exists to remove.
static CLIPBOARD_OFF: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether this process may open the OS clipboard at all.
///
/// ⚠️ **Checked by the four callers and not inside [`with_clipboard`]**, which is
/// the same split `Prefs::save` uses for `ephemeral` and is load-bearing for the
/// same reason: `clipboard_gate_tests` calls `with_clipboard` directly to prove
/// the lock holds under eight threads, and a check one level down would make that
/// test return early and assert nothing. The seam that *opens* a handle stays
/// honest; the seam that *decides to want one* is where the refusal goes.
fn clipboard_is_reachable() -> bool {
    !CLIPBOARD_OFF.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether the system clipboard holds a picture (§15 D224).
///
/// **The half of a *Paste* row's predicate that had no way to be asked.**
/// `can_paste` was `(payload && current) || system_text` (§15 D218), and a
/// clipboard holding *only* a screenshot answers no to both — so the row went dim
/// over the one payload `Ctrl+V` was happiest with.
///
/// **The cost is the reason this is a `bool` and not the image.** `arboard` has no
/// "which formats are on the clipboard" query, so the only way to ask is
/// `get_image`, which *decodes* — and holding the answer would park a
/// clipboard-sized RGBA buffer on [`ContextMenu`] for as long as a menu is open.
/// So the picture is read again by [`OndinApp::paste_image`] if the row is taken,
/// and that second conversion is small beside the PNG re-encode that arm does
/// anyway. What makes asking affordable at all is that arboard's Windows path
/// checks format availability *before* it reads, so a clipboard with no picture on
/// it — which is nearly every open — costs nothing but the handle.
pub(crate) fn system_clipboard_has_image() -> bool {
    clipboard_is_reachable() && with_clipboard(|c| c.get_image().is_ok()).unwrap_or(false)
}

/// One open context menu (`docs/context-menus.md` §0).
pub(crate) struct ContextMenu {
    /// The **press** position (R1), in screen points.
    pub at: egui::Pos2,
    pub target: crate::menu::Target,
    /// The world point the menu was opened over, where there is one.
    ///
    /// **The one thing a menu knows that a keyboard cannot express.** *Paste
    /// here* pastes at it, and *Edit text* puts the caret where the click landed
    /// — both of which a chord has no way to say.
    pub world: Option<Point>,
    /// True on the frame this menu was opened, and false on every frame after.
    ///
    /// **The frame that opened a menu is exempt from the click-away test**, and
    /// without this the menu shows for exactly one frame and vanishes.
    /// `dropdown`'s test is `any_click() && !menu.contains_pointer() &&
    /// !head.contains_pointer()`, and both exemptions miss here: `any_click()`
    /// counts the secondary button, and a context menu **has no head** — it is
    /// opened by a click on the canvas, which is precisely "outside the menu". So
    /// the release that opens it reads as the click that dismisses it. That is
    /// §15 D82 exactly, and `panels::dismissed_by_click`'s doc comment is the
    /// record of what it cost the first time.
    ///
    /// The mirror of R1's rule that a press which cancelled spends the click:
    /// between them, the two halves of a right-click are always accounted for.
    pub just_opened: bool,
    /// Whether the **system** clipboard held text when this menu opened.
    ///
    /// Two rows read it: the text session's *Paste*, which inserts that text at the
    /// caret (`docs/context-menus.md` §6.4), and every *other* *Paste*, which will make a
    /// text layer out of it when the app is holding nothing of its own
    /// ([`OndinApp::paste`]).
    ///
    /// **A snapshot, where every other row's state is rebuilt each frame.** The rest
    /// of a menu is read from app state that costs nothing to ask twice; this one
    /// means opening an OS clipboard handle, and doing that sixty times a second for
    /// as long as a menu is open is the kind of thing that fights other applications
    /// for the clipboard lock. A clipboard whose contents change while a context menu
    /// is open is not a case worth serving live.
    pub system_text: bool,
    /// Whether the **system** clipboard held a picture when this menu opened
    /// ([`system_clipboard_has_image`], §15 D224).
    ///
    /// A snapshot for [`Self::system_text`]'s reason and one more of its own: asking
    /// means decoding a bitmap, so once per open is the most this can be asked.
    /// Every *Paste* row bar the text session's reads it — a picture is what
    /// `paste_at` and `paste` both try first — and the text session's does not,
    /// because it inserts at a caret and a caret has nothing to do with a picture.
    pub system_image: bool,
    /// Whether [`OndinApp::clipboard`] / [`OndinApp::guide_clipboard`] still describe
    /// what is on the system clipboard — [`OndinApp::owns_the_clipboard`], read in the
    /// same snapshot as [`Self::system_text`] and for the same reason.
    ///
    /// Without it a *Paste* row offers a payload the chord beside it would decline,
    /// which is the one thing a menu must never do: it is the app's own claim about
    /// what a key would do.
    pub payload_current: bool,
    /// Whether the layer this menu opened on is text with glyphs to convert —
    /// what dims *Convert to path* (`build::can_outline_text`, §5.6).
    ///
    /// **A snapshot, and this one is not about a shared resource but about
    /// cost.** The honest answer is "does `text::outline` produce any contours",
    /// and asking it *builds the outlines* — about a microsecond a glyph (§15
    /// D145), which is why the render walk gates the same construction on there
    /// being a visible stroke to use it. A paragraph is a millisecond, every
    /// frame, for as long as the menu is up. Once per open is the right number,
    /// and a text node whose content changes while its own context menu is open
    /// is not a case worth serving live.
    ///
    /// The cheap proxy — "are there glyphs" — was rejected rather than not
    /// considered: a run of spaces has glyphs and no contours, so it would offer
    /// a conversion the builder then refuses with `EmptyGeometry`, which is §3's
    /// one prohibition on a row.
    pub text_outlineable: bool,
    /// The row `↑`/`↓` have walked to, if the keyboard is driving this menu
    /// (`docs/context-menus.md` §8).
    ///
    /// **A verb rather than an index**, because the menu is rebuilt every frame and
    /// an index would come to mean a different row the moment one appeared or went
    /// away — see [`crate::menu::navigate`], which is where the walking is done and
    /// where that reasoning belongs.
    ///
    /// `None` means the *pointer* owns the highlight, which is every menu until the
    /// first arrow key: a fresh menu has nothing highlighted, and the first pointer
    /// move gives the channel back. That is what keeps a menu to one highlight.
    pub highlight: Option<crate::menu::Item>,
}

/// How long the selection chrome stays away after the last keystroke, or after a
/// scrub lets go.
///
/// Long enough to look at what changed, short enough that it is never in the way —
/// and it almost never runs out, because moving the pointer over the canvas brings
/// the chrome back first ([`ChromeHold`]).
const CHROME_HOLD_SECS: f64 = 1.5;

/// Whether the selection chrome is being held back because a value in the
/// inspector is being edited, and until when (§15 D128).
///
/// **The point is to see what you are actually changing without a blue box over
/// it.** Every panel, every kind of layer: scrub a width, type a corner radius,
/// and the box, its handles, the size badge, the pivot marker and the hover
/// outline all go — `canvas::chrome_hidden` is the one place that list lives, and
/// this is a third *term* in that predicate rather than a third copy of the list.
///
/// **Two clocks, because the two gestures are shaped differently.** A scrub has a
/// duration of its own, so the timeout cannot start when it does — it starts when
/// the button comes up, and the chrome stays away for the whole drag however long
/// that is. A keystroke has no duration, so each one restarts the count.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum ChromeHold {
    /// Nothing is being edited; the chrome is the selection's own business.
    #[default]
    Free,
    /// A scrub is in flight. Held until the button comes up, whereupon the count
    /// starts — so this state has no deadline of its own.
    Scrubbing,
    /// Held until this moment on egui's own clock (`InputState::time`).
    ///
    /// Seconds from `i.time` rather than an `Instant`, because that is the clock
    /// `request_repaint_after` is measured against and the one a headless probe can
    /// drive.
    Until(f64),
}

/// How long the pointer rests on the layers panel's drag edge before the edge
/// lights up (§15 D350).
///
/// **Reported from the machine**: the edge went blue on every pass of the pointer
/// across the panel, which is most passes — the tree is a column you scan, and the
/// splitter is on the way to the canvas. A highlight that fires on travel is
/// telling you about a gesture you are not making.
///
/// 200ms is the shortest delay that reads as *intent* rather than as lag: a
/// pointer crossing the **7pt** band on its way somewhere is inside it for a
/// couple of frames, and a hand that has stopped to grab the edge has stopped for
/// longer than this before the button goes down.
///
/// ⚠️ 7 and not 6. Six is *egui's* handle — `expand2` by
/// `resize_grab_radius_side`, 3 either side — and `fold_layers_edge` deliberately
/// adds half a point so its band is never the thinner of the two. This said 6.
const EDGE_DWELL: f64 = 0.2;

/// Whether the layers panel's drag edge is lit, and since when (§15 D350).
///
/// Seconds from `InputState::time` rather than an `Instant`, for
/// [`ChromeHold::Until`]'s reason: it is the clock `request_repaint_after` is
/// measured against and the one a headless probe can drive.
///
/// **The delay is on the highlight only, never on the cursor.** A resize cursor is
/// cheap to look at and is the affordance that says the edge is grabbable at all;
/// what was reported as noise is the blue line. egui claims the cursor off its own
/// resize response, which this does not touch.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub(crate) enum EdgeDwell {
    /// The pointer is not on the edge.
    #[default]
    Off,
    /// It arrived at this moment and the highlight is not due yet.
    Waiting(f64),
    /// Lit — the dwell elapsed, or a press landed on the edge before it did.
    Lit,
}

impl EdgeDwell {
    /// One frame of the machine, as a **function of its inputs and nothing else** —
    /// [`ChromeHold::advance`]'s shape, and for its reason.
    ///
    /// - `in_band` is the pointer inside the edge's grab band.
    /// - `pressed` is the primary button going down *this frame*, which lights the
    ///   edge at once: a drag must never be the one gesture with no feedback, and
    ///   waiting out the dwell during it would be exactly that. **The press edge
    ///   rather than `down`**, because `down` is also true of an unrelated drag
    ///   that merely crosses the band — canvas to panel with the button held.
    /// - `down` does two opposite jobs, and they are two arms rather than one.
    ///   It **holds** a lit edge once the pointer has left the band, which it does
    ///   whenever a drag runs into `MIN_W` or `MAX_W` — the edge stops and the hand
    ///   keeps going. And it **blocks** the dwell, so a gesture that is already in
    ///   flight when it reaches the band can never light the edge by resting there.
    ///
    /// ⚠️ **That second job was missing and the omission was invisible**, because
    /// the obvious bad case — a drag crossing the band — crosses in a few frames
    /// and never reaches the deadline. What reaches it is a drag that *hesitates*:
    /// a layer row carried out to the canvas and held over the splitter, a marquee
    /// paused there. Then the edge lights under a hand that is plainly doing
    /// something else, which is the reported complaint in its rarer form. Found by
    /// `arch-scribe` reading this arm against the sentence above it, and not by the
    /// test, which sampled the crossing case only up to 100ms and so pinned the
    /// *shortcut* rather than the rule.
    pub fn advance(self, now: f64, in_band: bool, pressed: bool, down: bool) -> Self {
        match self {
            Self::Off if in_band && pressed => Self::Lit,
            Self::Off if in_band => Self::Waiting(now),
            Self::Off => Self::Off,
            Self::Waiting(_) if !in_band => Self::Off,
            Self::Waiting(t) if pressed || (!down && now - t >= EDGE_DWELL) => Self::Lit,
            Self::Waiting(t) => Self::Waiting(t),
            Self::Lit if in_band || down => Self::Lit,
            Self::Lit => Self::Off,
        }
    }

    /// Whether the edge draws its accent this frame.
    pub fn lit(self) -> bool {
        self == Self::Lit
    }

    /// When the highlight falls due, so the frame can be woken for it.
    ///
    /// Without this the light arrives on the next event rather than on time —
    /// and a pointer resting on the edge is *producing no events*, which is the
    /// one case this whole state machine exists for.
    pub fn due(self) -> Option<f64> {
        match self {
            Self::Waiting(t) => Some(t + EDGE_DWELL),
            _ => None,
        }
    }
}

/// How long a first opacity digit waits for a second one (`docs/shortcuts.md` §2).
///
/// Figma's window, and short enough that a deliberate `4` then a deliberate `5`
/// are two values rather than one 45%.
const OPACITY_DIGIT_WINDOW: f64 = 0.6;

/// A first opacity digit waiting to find out whether it is the whole value.
///
/// Seconds from `InputState::time` rather than an `Instant`, for
/// [`ChromeHold::Until`]'s reason: it is the clock `request_repaint_after` is
/// measured against and the one a headless probe can drive.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct OpacityEntry {
    /// `0`–`9` as typed. `0` alone means 100%, so this is the digit and not the
    /// value it resolves to.
    first: u8,
    at: f64,
    /// **The layers the digit was typed against** (§15 D470).
    ///
    /// ⚠️ **Without it the commit re-read the live selection at deadline time**,
    /// 600 ms later, and landed on whatever was selected *then*.
    /// `[S16.1-L1-02]`, measured headless with `RawInput::time` driven: two rects
    /// at 1.0, select A, press `4` — A previews at 40% — click B inside the
    /// window, and when the window shuts **A is back at 1.0 and B is committed to
    /// 0.4**, `undo_depth 0 → 1`, status *"Opacity 40%"*. The user typed a digit
    /// against A, watched A change, and B — which they merely selected — keeps the
    /// value. One undo restores B; A never gets what was asked for.
    ///
    /// **The rule was already written in this struct's sibling.**
    /// [`InFlight::session_scrub`] — the other "value in flight that
    /// `RenderOverrides` cannot preview" — says verbatim: *"the node is carried
    /// with the lists so a snapshot cannot be applied to a session that has since
    /// moved to a different layer."* This is that shape and did not carry it.
    ///
    /// ⚠️ **The empty-selection row was clean by accident and one level down**, at
    /// [`OndinApp::set_opacity_pct_on`]'s `if tx.0.is_empty() { return; }`, where
    /// [`OndinApp::opacity_digit`] — the *other* reader of the same entry — has an
    /// explicit guard. The deadline twin had neither, and the selection-*moved*
    /// case had no guard anywhere.
    subject: Vec<NodeId>,
}

/// What a typed digit resolves to, and whether the window is still open.
#[derive(Clone, PartialEq, Debug)]
enum OpacityStep {
    /// Preview this percentage and hold the entry: a second digit may still
    /// arrive and make it exact.
    Wait(f64, OpacityEntry),
    /// Commit this percentage — a second digit shut the window.
    Land(f64),
}

/// The percentage a *lone* digit means: `1`–`9` are 10%–90%, and `0` is 100%.
///
/// `0` is the one value the "digit × 10" reading cannot produce, and 100% is
/// what every app that ships this binding gives it — 0% being reachable instead
/// as `0`,`0`, which the two-digit window already covers.
fn opacity_of_digit(digit: u8) -> f64 {
    if digit == 0 {
        100.0
    } else {
        f64::from(digit) * 10.0
    }
}

/// Whether `entry` is still waiting for a second digit.
///
/// **The single reading of the boundary**, shared by the two places that would
/// otherwise each decide it: [`opacity_step`], which pairs a second digit with
/// it, and [`OndinApp::fold_opacity_entry`], which commits it once the window
/// shuts. If those two disagreed by so much as a float's width there would be an
/// instant at which one digit was both folded into a pair *and* already
/// committed on its own — one keystroke spent twice. Which side of `<` the exact
/// boundary falls on does not matter; that they agree does.
fn opacity_window_open(entry: &OpacityEntry, now: f64) -> bool {
    now - entry.at < OPACITY_DIGIT_WINDOW
}

/// Resolve a digit against whatever digit is already pending.
///
/// Pure so it can be tested as the state machine it is rather than through the
/// keystrokes that drive it. The clock is passed in for the same reason — a test
/// that had to wait out the window would be a slow test of `Instant`.
fn opacity_step(
    pending: Option<OpacityEntry>,
    digit: u8,
    now: f64,
    subject: &[NodeId],
) -> OpacityStep {
    match pending {
        // ⚠️ **The subject has to match, or two digits typed against two
        // different layers pair into one value** (§15 D470). Select A, press
        // `4`, click B, press `5` — that is not `45%` on anything, and reading
        // the window alone made it `45%` on B. A moved subject starts a new
        // entry, which is what the user's second keystroke means.
        Some(entry) if opacity_window_open(&entry, now) && entry.subject == subject => {
            OpacityStep::Land(f64::from(entry.first * 10 + digit))
        }
        _ => OpacityStep::Wait(
            opacity_of_digit(digit),
            OpacityEntry {
                first: digit,
                at: now,
                subject: subject.to_vec(),
            },
        ),
    }
}

/// Whether `tx`, offered by a control with response `resp`, is an inspector edit
/// worth hiding the selection chrome for — and if so, whether the gesture making it
/// is a **drag still in flight** (§15 D128).
///
/// **Two questions, and both halves are needed.** *What does the edit do* is
/// `Transaction::changes_ink`: an edit with no visible result has nothing to get the
/// box out of the way for. *Did an edit happen this frame at all* is the response —
/// and leaving that half out is what broke this the first time it shipped.
///
/// A `Some(resp)` is a **live control, asked every frame whether or not anything
/// happened**, and the transaction beside it is built unconditionally too
/// (`inspector::place_at` returns a `SetTransform` whether or not the number moved,
/// and `changes_ink` classifies the op rather than comparing values). So a resting X
/// field noted an edit on every frame, `ChromeHold::advance` restarted its 1.5s on
/// every frame, and the chrome was away for as long as the Transform panel was on
/// screen — reported as "once I select a layer the bounding box always hides unless
/// I move the mouse", because canvas motion was the only thing left that could
/// clear it.
///
/// A `None` needs no such test: it comes from [`OndinApp::commit_edit`], which is
/// only reached when a discrete control has decided something changed, and it is
/// instantaneous, so its count starts at once.
fn edit_note(resp: Option<&egui::Response>, tx: &Transaction) -> Option<bool> {
    if !tx.changes_ink() {
        return None;
    }
    match resp {
        // Dragged first: a frame that both drags and changes is a scrub, and a
        // scrub's hold has no deadline until the button comes up.
        Some(r) if r.dragged() => Some(true),
        // `changed()` is "the value moved", which for a value field is exactly
        // `nudged || value != before`. It is what leaves the arrow keys, Home and a
        // click-to-focus alone — and `lost_focus()`, which the valve commits on but
        // which is not an edit.
        Some(r) if r.changed() => Some(false),
        Some(_) => None,
        None => Some(false),
    }
}

impl ChromeHold {
    /// Whether the chrome is away — the whole of what `chrome_hidden` asks.
    pub fn holding(self) -> bool {
        self != Self::Free
    }

    /// One frame of the machine, as a **function of its inputs and nothing else**.
    ///
    /// Here rather than inline in `OndinApp::fold_chrome_hold` because this is the
    /// part that can be wrong, and a state machine is worth driving through its own
    /// states rather than through the frames that would produce them.
    /// `fold_chrome_hold` gathers the four inputs from the `Context` and does
    /// nothing else.
    ///
    /// - `edit` is the note a value field left: `Some(true)` a drag in flight,
    ///   `Some(false)` a keystroke that moved the number, `None` nothing.
    /// - `pointer_down` ends a scrub, because the field cannot report the release.
    /// - `moved_on_canvas` is the pointer travelling over the artwork, which brings
    ///   the chrome back early — but **never out of a scrub**, since a long drag
    ///   crosses the canvas by construction.
    pub fn advance(
        self,
        now: f64,
        edit: Option<bool>,
        pointer_down: bool,
        moved_on_canvas: bool,
    ) -> Self {
        let started = match edit {
            Some(true) => Self::Scrubbing,
            Some(false) => Self::Until(now + CHROME_HOLD_SECS),
            None => self,
        };
        match started {
            // The release: the count starts here, not where the drag did, so the
            // chrome stays away for the whole gesture however long it runs.
            Self::Scrubbing if !pointer_down => Self::Until(now + CHROME_HOLD_SECS),
            Self::Scrubbing => Self::Scrubbing,
            Self::Until(deadline) if moved_on_canvas || now >= deadline => Self::Free,
            other => other,
        }
    }
}

impl OndinApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let render_state = cc
            .wgpu_render_state
            .clone()
            .expect("ondin requires the wgpu backend");
        crate::theme::install(&cc.egui_ctx);
        // **Read before the font service, because the service is built from it.**
        // `Prefs::web_fonts` decides whether the catalog thread is spawned at all,
        // which is a startup decision and not one `set_web_fonts` can un-make
        // retroactively: a launch with the switch off must make no request, not one
        // it then ignores.
        let prefs = crate::prefs::Prefs::load();
        let mut app = Self::with(
            CanvasRenderer::new(render_state),
            FontService::new(&cc.egui_ctx, prefs.web_fonts),
            prefs,
        );
        app.open_at_launch();
        app
    }

    /// Say, once, that a record on disk could not be read.
    ///
    /// ⚠️ **A silent latch is worse than the loss it prevents**, which is the
    /// whole reason this exists. Three records the app reads at startup now
    /// refuse to be overwritten with the defaults a failed read produced —
    /// [`crate::prefs::Prefs::unreadable`],
    /// [`crate::library::cache::LocalIndex::unreadable`] and
    /// [`crate::library::state::Library::projects_unreadable`] — and each of
    /// those states looks exactly like an ordinary empty one from the screen: a
    /// library with nothing in it, a Recent list that has forgotten, no
    /// projects. The user's obvious repair for an empty library is to make a new
    /// one, *beside* the real one, which is the loss the latch was protecting
    /// them from arriving by a different road.
    ///
    /// `base_folder` is named in the message because it is the one preference
    /// whose loss is not cosmetic; the others are habits.
    ///
    /// **A `fail`, not a modal.** All three states are survivable and the app
    /// runs. One sentence in a status line is the whole fix — and it is only a
    /// fix at all since the library screen started painting one
    /// (`status_label`), which is why the latches and this landed together.
    pub(crate) fn report_unreadable_records(&mut self) {
        let unreadable: Vec<&str> = [
            (
                self.prefs.unreadable,
                "prefs.json (including your base folder)",
            ),
            (
                self.library.local.unreadable,
                "library.json (Recent and your stars)",
            ),
            (
                self.library.projects_unreadable,
                "projects.json (your project list)",
            ),
        ]
        .iter()
        .filter(|(bad, _)| *bad)
        .map(|(_, name)| *name)
        .collect();
        if unreadable.is_empty() {
            return;
        }
        let them = if unreadable.len() == 1 { "it" } else { "them" };
        self.session.fail(format!(
            "Could not read {} — running on defaults, and not writing over {them} \
             until {them} can be read. Repair or remove {them} to recover.",
            unreadable.join(", "),
        ));
    }

    /// Decide what the window shows on the first frame.
    ///
    /// **The library, unless the user asked otherwise** — that is what the
    /// dashboard is for. `reopen_last` is the escape hatch for someone who
    /// works in one document, and it degrades quietly: a `last_document` that
    /// has been deleted, renamed or is on an unplugged drive lands on the
    /// dashboard, which is the same place a fresh install lands and needs no
    /// error of its own.
    ///
    /// ⚠️ **`open_path` is deliberately not used here.** That one autosaves the
    /// outgoing document, asks about unsaved work and writes `last_document`
    /// back — three things that are wrong before the first frame, where the
    /// outgoing document is the empty starter and there is nothing to preserve.
    fn open_at_launch(&mut self) {
        self.view = View::Dashboard;
        self.report_unreadable_records();
        // ⚠️ **The trash's retention window is enforced here, and until §15 D550
        // it was enforced by pressing *Save* in the library-settings card and by
        // nothing else** (`[S1.2-L5-03]`). `store::purge_trash` had exactly one
        // caller in the workspace, the tail of that handler, so two things were
        // true at once: a user who never opened that modal kept every document
        // they had ever deleted, forever, while the Trash heading told them each
        // was *"kept for 30 days"* and §9.5 of the design said the same; and the
        // user who opened Settings to change the **autosave interval** silently
        // and permanently deleted every trashed document past the window, on
        // pressing Save, with nothing in the modal mentioning it.
        //
        // ⚠️ **At launch rather than on entering the dashboard**, which is what
        // the finding's own sketch offered as the alternative — and which would
        // leave one user out. `reopen_last` goes straight into a document
        // (`Prefs::reopen_last`, three lines below), so somebody who works in one
        // file may never see the dashboard at all, and theirs is the trash that
        // grows without bound. A launch reaches both. It is also the least
        // exposed cadence for a *destructive* sweep: once, before anything is on
        // screen, rather than on every walk back to the library.
        //
        // **The settings save keeps its own sweep**, which is not redundant: it
        // is what makes a window the user has just *shrunk* take effect now
        // rather than at the next launch. What made that call dangerous was the
        // years of accumulation behind it, and this line is why there is no
        // longer any.
        crate::library::store::purge_trash(&self.library.root, self.prefs.trash_keep_days);
        // The sweep just changed what is in `.trash`, and `Library::open` counted
        // it before this ran (§15 D728) — so without this the sidebar's *Trash*
        // count is the pre-sweep one for the rest of the session.
        self.library.refresh_trash();
        // **Collected before `reopen_last` runs, and answered later.** The two are
        // usually about the same document — the one you were editing when the
        // process died is the one `last_document` names — so the question has to
        // be asked *over* whatever this method opens rather than instead of it.
        // Collecting is all that happens here: the modal is a frame's job, and a
        // launch that blocked on a dialog before drawing anything would be a
        // launch that looks like a hang.
        //
        // ⚠️ **Measured: putting this at the *end* of the function instead changes
        // nothing, so the order is not load-bearing today.** Nothing on the reopen
        // path below touches `library.entries` or the document's mtime, which are
        // the only two things `pending` reads. It is written this way against the
        // shape rather than the symptom: the reopen is hand-rolled here precisely
        // because [`Self::open_path`] does more than load — including
        // [`Self::drop_recovery`], which would **delete** the snapshot this
        // function is collecting. The day somebody simplifies these twenty lines
        // into an `open_path` call, the order stops being cosmetic and starts
        // being the thing that saved the work.
        self.recovery.pending =
            crate::library::recovery::pending(&self.library.root, &self.library.entries);
        if !self.prefs.reopen_last {
            return;
        }
        let Some(path) = self.prefs.last_document.clone() else {
            return;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let Ok(doc) = ondin_core::io::load(&bytes) else {
            return;
        };
        if let Some(id) = doc.meta().id.clone() {
            self.library.local.mark_opened(&id);
            self.library.local.save();
        }
        self.session.adopt_document(doc, Some(path));
        if self.prefs.collapse_groups_on_open {
            self.collapse_all();
        }
        self.view = View::Editor;
    }

    /// **An `OndinApp` with no GPU, no font threads and no preferences file** —
    /// the thing this crate could not build until §15 D303, and the reason a long
    /// list of behaviour was "verified by reading" (D35).
    ///
    /// Everything the app *decides* is now reachable from a test: tool routing,
    /// the cursor rules, autopan, `resize_tx`, the line chrome. What is still not
    /// reachable is anything that needs pixels on a device, which is what
    /// `ondin export` and the throwaway-`#[test]` technique in `CLAUDE.md` are
    /// for — this does not replace either.
    ///
    /// **`theme::install` is called, because a headless app that skipped it would
    /// be a different app**: fonts and spacing are read out of the style by half
    /// the chrome, and a test measuring geometry against egui's defaults would
    /// measure the wrong thing.
    ///
    /// The caller supplies the `Context`, rather than one being made here, so a
    /// probe can drive real input through the same `ctx` it later reads output
    /// from.
    #[cfg(test)]
    pub(crate) fn headless(ctx: &egui::Context) -> Self {
        crate::theme::install(ctx);
        // ⚠️ **The base folder is pointed somewhere that does not exist**, because
        // `with` scans it. On the default it would be the real `~/.ondin` — so a
        // probe would read the developer's own documents, take as long as their
        // library is big, and assert against a folder that differs per machine.
        // A path rather than an `Option<Library>`: the app must never have to ask
        // whether it has a library.
        let prefs = crate::prefs::Prefs {
            base_folder: Some(std::env::temp_dir().join("ondin-headless-no-library")),
            // ⚠️ **And it must not be able to write itself out**, or the line
            // above becomes the developer's real base folder the first time a
            // probe reaches any of the app's `prefs.save()` calls — which is
            // exactly what happened, months later, with a temp root that had
            // already been deleted (`prefs::Prefs::ephemeral`).
            ephemeral: true,
            ..Default::default()
        };
        // ⚠️ **A fourth machine-wide resource, and it is not a constructor
        // argument like the three below** (§15 D798). The OS clipboard is one per
        // *process*, so the refusal is a process-wide flag rather than a field —
        // see `CLIPBOARD_OFF`. It is set here and never cleared: once any test in
        // a binary has built a headless app, nothing in that binary reads or
        // writes the developer's clipboard again.
        CLIPBOARD_OFF.store(true, std::sync::atomic::Ordering::Relaxed);
        Self::with(CanvasRenderer::headless(), FontService::inert(), prefs)
    }

    /// The one field list, so the two constructors above cannot drift.
    ///
    /// **Three parameters and not zero**, because those three are the whole of the
    /// difference between an app and a headless one: a device, five background
    /// threads and a file on disk. Everything below is plain state that means the
    /// same thing either way — and a new field added to `OndinApp` still has
    /// exactly one place to be initialised, which is what stops `headless` rotting
    /// into a second app that behaves differently.
    fn with(canvas: CanvasRenderer, fonts: FontService, prefs: crate::prefs::Prefs) -> Self {
        // **Before the struct literal, because it reads `prefs` and the literal
        // moves it.** Resolving the root once here is also what stops a base
        // folder edited mid-session from making two halves of one frame disagree
        // about where the library is (`library::state::Library::root`).
        let root = crate::library::root(prefs.base_folder.as_deref())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let library = crate::library::state::Library::open(root);
        let prefs_page = prefs.dashboard_page.clone();
        let prefs_sort = prefs.dashboard_sort.clone();
        let prefs_list_view = prefs.dashboard_list_view;
        Self {
            session: EditorSession::new(),
            frame_index: Default::default(),
            segment_walks: Default::default(),
            canvas,
            mode: Mode::Normal,
            tool: Tool::Select,
            drag: Drag::None,
            alt_clone: None,
            clipboard: None,
            clipboard_from: None,
            clipboard_stamp: None,
            name_edit: None,
            fonts,
            previews: crate::fonts::FontPreviews::default(),
            thumbs: crate::thumbs::ImageThumbs::default(),
            font_filter: String::new(),
            pen: None,
            pen_bias: false,
            pending_images: Vec::new(),
            text: None,
            points: PointSet::default(),
            collapsed: HashSet::new(),
            opacity_entry: None,
            renaming_layer: None,
            layers_synced_selection: Vec::new(),
            scroll_layers_to_selection: false,
            layers_range_anchor: None,
            layer_filter: None,
            layer_drag: None,
            layers_edge: EdgeDwell::default(),
            confirming_close: false,
            canvas_px: (1, 1),
            canvas_ppp: 1.0,
            canvas_rect: egui::Rect::NOTHING,
            chrome_hold: ChromeHold::default(),
            pending_edit: None,
            autopan_total: Vec2::ZERO,
            // **Preview starts closed, and that is not only tidiness**: opening it
            // is what turns on the one thing in the inspector that renders and
            // encodes the artwork (§7).
            //
            // Effects starts closed because a fresh layer has an empty stack, and
            // from the first frame it draws it is the *panel* that decides — it
            // opens itself on a layer with effects and shuts itself on one without,
            // through the rule Fill and Stroke already follow
            // (`inspector::sync_paint_collapse`). This seed is only what the set
            // holds before any layer has been selected; it used to be the whole
            // answer, on the grounds that the card was a placeholder with nothing
            // in it.
            collapsed_panels: HashSet::from(["Effects", "Preview"]),
            per_corner_radius: HashSet::new(),
            open_menu: TopMenu::None,
            context_menu: None,
            secondary_press: None,
            show_grid: true,
            // On by default: a grid a designer went to the trouble of adding to a
            // frame is one they want to see, and the switch is for taking it away
            // for a moment rather than for turning the feature on (§15 D755).
            show_layout_grids: true,
            show_layers: true,
            show_toolbar: true,
            present: false,
            // Off: a marker on every selection would be permanent chrome for a
            // property most layers never move.
            show_pivot: false,
            snap_shapes: true,
            snap_guides: true,
            snap_grid: true,
            snap_baselines: true,
            clones: None,
            entered_group: None,
            gesture_cancelled: false,
            in_flight: InFlight::default(),
            picker: None,
            pending_picker: None,
            paint_hex: None,
            scale_pct: 100.0,
            paint_auto_collapsed: HashSet::new(),
            paint_synced_selection: Vec::new(),
            stroke_menu: None,
            effect_menu: None,
            image_tab: ImageCard::default(),
            stroke_sides_open: Vec::new(),
            prefs,
            settings: None,
            export_synced_selection: Vec::new(),
            effects_synced_selection: Vec::new(),
            grids_synced_selection: Vec::new(),
            export_row_open: None,
            export_menu: false,
            export_clipboard: None,
            export_preview: None,
            export_prefix_text: None,
            export_suffix_text: None,
            stroke_dash_text: None,
            type_menu: None,
            feature_filter: String::new(),
            show_all_features: false,
            char_hex: None,
            multi_transform_drag: None,
            cursors: Cursors::default(),
            show_rulers: true,
            show_guides: true,
            // Off, because locking guides nobody has placed is a mode with nothing
            // in it — see the field. Deliberately not stated as an ordinal ("the one
            // view switch that is off", which it was not: `present` and `show_pivot`
            // are both off four dozen lines above), for `present`'s own reason.
            lock_guides: false,
            guide_previews: Vec::new(),
            guide_clipboard: None,
            property_clipboard: None,
            // **Editor, and the launch path overrides it** — see
            // `OndinApp::open_at_launch`. Defaulting the other way would make
            // every headless test start on a screen it does not care about, and
            // `with` is what those tests build through.
            view: View::Editor,
            library,
            last_autosave: std::time::Instant::now(),
            recovery: crate::library::recovery::RecoveryState::default(),
            writer: None,
            rename_entry: None,
            dash: crate::panels::dashboard::DashboardState {
                nav: crate::panels::dashboard::Nav::from_id(&prefs_page),
                sort: crate::library::state::Sort::from_id(&prefs_sort),
                list_view: prefs_list_view,
                ..Default::default()
            },
            covers: crate::library::cover::Covers::default(),
            library_settings: None,
            stranded: None,
            chrome_focus: false,
            popover_heard: false,
        }
    }
}

/// Which screen the window is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum View {
    /// The library: projects, documents, and the way into one.
    Dashboard,
    /// A document open on the canvas.
    Editor,
}

impl eframe::App for OndinApp {
    /// Wait for the snapshot writer before the process goes.
    ///
    /// ⚠️ **The two close arms in [`Self::handle_close_request`] already settle,
    /// and this is not a duplicate of them** — it is the path for every *other*
    /// way the window goes away: the ✕ on a clean document, which never reaches
    /// that modal, and a shutdown eframe drives itself. The queued job in those
    /// cases is a *removal*, and losing it leaves a snapshot the next launch has
    /// to reason its way out of offering.
    ///
    /// ⚠️ **This is the primary flush and `library::writer::Writer`'s `Drop` is the
    /// backstop, which is the opposite of how it looks.** A destructor is the
    /// more skippable of the two, by *two* eframe paths and not one: with
    /// `NativeOptions::run_and_return` off, eframe calls `std::process::exit(0)`
    /// and nothing in the process is dropped; and with it on — the 0.35 default,
    /// which `main::run_gui` takes — eframe's own note on `exiting` records that
    /// a macOS `Cmd-Q` exits the loop and then `run_app_on_demand` never
    /// returns, so the app is never dropped either. `on_exit` is reached on all
    /// of them, through `save_and_destroy`. A snapshot is not the thing to make
    /// depend on a default nobody here chose, still less on a platform nobody
    /// here builds for.
    ///
    /// [`Self::handle_close_request`]: OndinApp::handle_close_request
    fn on_exit(&mut self) {
        self.disk_settle();
    }

    /// The frame body: everything the app draws and decides, once per pass.
    ///
    /// 🚨 **It is `ui`, and prose across this project calls it `update`** (§15
    /// D698, `[S17-L3-07]`). `eframe::App`'s conventional method *is* named
    /// `update` and takes an `eframe::Frame`, so the wrong name reads as
    /// obviously right; this impl has exactly two methods, `on_exit` and this,
    /// and `grep -rn "fn update" crates/ondin-app/src/` returns only
    /// `canvas::update_drag` and `rulers::update_guide_drag`. Nothing catches
    /// the drift — every occurrence is plain backticks in prose, which is D319's
    /// hole, so `rustdoc::broken_intra_doc_links` never looks.
    ///
    /// ⚠️ **The cost was not tidiness.** §15 D330 excused an untested decision
    /// on the ground that the guard *"is in `OndinApp::update`, which takes an
    /// `eframe::Frame` and is not reachable from a test"* — and this signature
    /// takes `&mut egui::Ui` plus `&mut eframe::Frame`, is reachable, and was
    /// already being driven by a test at the time. **An excuse argued from a
    /// signature that is not the signature.** D330 carries the correction now.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // **Above the view branch, because a launch can land on either screen**
        // and the question is about neither of them — see
        // [`OndinApp::recovery_modal`].
        self.recovery_modal(&ctx);

        // **Above the view branch for the same reason, and it was below it until
        // §15 D526.** `handle_close_request` is *"intercept the window close so
        // unsaved work is never lost silently"* — a claim about the window, not
        // about the editor — and it sat under the `return` twenty lines down, so
        // on the library screen the ✕ was never intercepted whatever the session
        // held. That state is reachable: `go_to_dashboard` autosaves on the way
        // out only for a document that has a path, and `save_file` returns early
        // after `session.fail` on any write error, which leaves a dirty session
        // behind a screen with no interception. What stopped it being outright
        // loss is the `.recovery/` snapshot, up to `SNAPSHOT_SECS` behind.
        //
        // ⚠️ **The comment below says the font poll is "the one thing given up",
        // and it was wrong by four.** These two, `disk_results` and the two
        // ticks were all under the return; `go_to_dashboard` compensates the
        // tick by hand and says so, and the same argument was never made for
        // the close. Two of the five are lifted here; the rest stay editor-only
        // on purpose and the sentence below now says which.
        self.handle_close_request(&ctx);
        self.close_confirmation(&ctx);

        // ⚠️ **The dashboard returns, and everything below it is the editor.**
        // Not a panel drawn over the canvas and not a branch late in the frame:
        // while the library is up there is no tool, no selection to nudge and no
        // canvas to autopan, so `input::resolve` must not run and the panels must
        // not draw. Putting the boundary here — before the font poll, before the
        // keyboard — is what makes that a property of the frame rather than a
        // rule each of thirty call sites has to remember.
        //
        // **Three things are given up by returning this early, and each is
        // deliberate**: the font poll, which the dashboard needs for nothing
        // (it neither shapes text with document fonts nor previews a family);
        // `disk_results`, whose pill the editor's panels draw; and the two
        // ticks, one of which `go_to_dashboard` compensates by hand and says
        // so. ⚠️ **The close interception used to be a fourth and was not
        // deliberate** — it is above this return now (§15 D526).
        if self.view == View::Dashboard {
            self.dashboard_ui(ui);
            // 🚨 **`chrome_focus` is written here too, because this return is
            // above the only other write** (§15 D850, `[X1.2-L6-01]`). The flag
            // otherwise kept whatever the last editor frame recorded for the whole
            // library visit — `true`, if the user left by a top-bar control, which
            // is focused at the end of the frame that clicks it — and that is what
            // the first editor frame after reopening a document read.
            self.chrome_focus = ui.ctx().memory(|m| m.focused()).is_some();
            return;
        }

        // Merge background font results. **Only font *data* invalidates the
        // shaped-text cache** (§5.4a): a longer family list is a repaint, and
        // re-shaping the document when the web catalog landed was work for a
        // change that added no fonts at all.
        let fonts = self.fonts.poll();
        if fonts.fonts_registered {
            // 🚨 **The document's cache goes only when the document names the
            // family** (§15 D591, `[A4-L4-04]`). The picker fetches preview faces
            // for as long as it is scrolled, and this used to re-shape every text
            // node in the document on every frame one of them landed — ~23 ms on a
            // twenty-block document, for faces that cannot change a glyph of it.
            // The distinction §5.4a already draws between *data* and a longer
            // *list* (§15 D352) needed one more step.
            if fonts.touches(&self.document_families()) {
                self.session.fonts_changed();
            }
            // **The retry is not narrowed, and that is deliberate.** A picker miss
            // is a family whose *preview strip* could not be drawn, and a preview
            // face arriving is precisely the thing it was waiting for — whether or
            // not the document has ever heard of it.
            self.previews.retry_misses();
        }
        if fonts.families_changed {
            // **The same retry, for the document's own fonts.** `ensure_loaded` can
            // only queue a family it can resolve, and `faces_of` resolves against the
            // system scan and the web catalog — both of which arrive here, on a later
            // frame than the open that asked. A document opened before either landed
            // therefore queued nothing and was never asked again, so its text sat in
            // the Inter fallback for the rest of the session however long the download
            // would have taken. Idempotent by construction: `ensure` skips a face
            // already claimed, and this frame happens a handful of times per run.
            self.ensure_document_fonts();
        }
        for family in self.fonts.take_evicted() {
            self.previews.forget(&family);
        }
        if fonts.any() {
            ctx.request_repaint();
        }

        // A pointer press ends any run of coalescing edits: a click is a new
        // intent, and it is also how the subject changes — nudge one point, click
        // another, nudge that one, and the two bursts must be two undo steps even
        // though the verb and the node are the same
        // (`EditorSession::end_edit_run`).
        if ctx.input(|i| i.pointer.any_pressed()) {
            self.session.end_edit_run();
        }

        // Whether this frame's `Tab` was **ours** — the keymap was open and the
        // node tool had a path, or a text session was live — which is asked here and
        // answered at the end of the frame. It has to be read before the panels run,
        // because the focus it is about is the focus they are about to change.
        //
        // **A live session claims `Tab` unconditionally**, where point editing claims it
        // only when there is a path to step: in a session `Tab` nests the list items the
        // selection touches (`TextEdit::step_list_level`), and on ordinary text it does
        // nothing — but *nothing* still has to mean "not the focus ring", because a
        // session that loses the keyboard to a chrome button swallows the next thing
        // typed. Insert mode owning every key is what `input::resolve` already says.
        let took_tab = (self.edited_path().is_some() || self.text.is_some())
            && !ctx.egui_wants_keyboard_input()
            && ctx.input(|i| i.key_pressed(egui::Key::Tab));

        // Keyboard first, but only what the current mode admits and only when
        // no chrome widget owns the keyboard (see `input::resolve`).
        //
        // **An open context menu holds the keyboard, and Escape is its own rung**
        // (`docs/context-menus.md` §0 R3). `input::resolve` must not run at all while
        // one is up, for the same reason it already checks
        // `egui_wants_keyboard_input`: with a menu open over a layer, `Delete`
        // would delete the layer while its own menu was still offering the row.
        //
        // And the press that closes a menu closes **only** the menu. `escape` is a
        // ladder that pays out one rung per press, and a version that closed the
        // menu and then fell through would deselect, or leave the group, or drop
        // back to Select, in the same keystroke that dismissed a menu opened by
        // accident — eating the very state the menu was aimed at. Returning here
        // rather than calling `escape` is what makes that structural: there is no
        // path from this arm to the ladder.
        //
        // **Holding the keyboard is not only refusing to give it away.** The other
        // half is the five keys the menu itself answers — `↑`/`↓`, `Home`/`End`,
        // `Enter` (`docs/context-menus.md` §8) — and they are *consumed* here, at the
        // top of the frame, so that no widget drawn later can read one the menu is
        // using. Which row an arrow lands on is not known until the menu is built
        // at the bottom of the frame, so the press travels there as a value.
        let mut nav = None;
        // Last frame's answer, taken so this frame's handlers write a fresh one —
        // see the field.
        let popover_heard = std::mem::take(&mut self.popover_heard);
        if self.context_menu.is_some() {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.context_menu = None;
            } else {
                nav = crate::menu::read_nav(&ctx);
            }
        } else if self.modal_is_up() {
            // **A modal owns the keyboard, including the keys it has no field
            // for.** Escape belongs to the card, which closes on it at the
            // bottom of the frame; every other key would otherwise reach the
            // document behind the backdrop — an arrow nudging the selection, `R`
            // switching tools, `Delete` deleting a layer — none of which the user
            // can see happening.
            //
            // ⚠️ **This arm named `settings` alone until 2026-09-07, and the other
            // two are the ones with the worst consequence** (§15 D464).
            // `[S16.1-L2-01]`, measured headless through `eframe::App::ui` with
            // real `RawInput`, two passes per case:
            //
            // | modal up | layer alive after `Delete` | undo |
            // | --- | --- | --- |
            // | none — the positive control | **false** | 1 |
            // | `settings` — the one guarded arm | true | **0** |
            // | `confirming_close` | **false** | 1 |
            // | `recovery.pending` | **false** | 1 |
            //
            // Under *Unsaved changes* the accidental deletion is what *Save and
            // close* then writes to disk. Under the recovery card — drawn **above**
            // the view branch (§15 D377), so it can be up over a freshly reopened
            // document — the card blocks the pointer, which makes the keyboard the
            // only door, and `Ctrl+A` then `Delete` emptied a document while the
            // card was still asking about a *different* one's snapshot.
            //
            // ⚠️ **`input::resolve` is not a second guard**: it asks
            // `ctx.egui_wants_keyboard_input()`, which is **false** for a modal
            // holding three buttons and no text field. This `else if` is the whole
            // of it.
            //
            // ⚠️ **And the app's other key router already had all three.** §9.5
            // writes out `dashboard_keys`' guard list and it includes the recovery
            // card by name, with a note that `OndinApp::ui` draws that card and
            // `dashboard.rs` knows nothing about it (D383). The same question was
            // answered on one screen and not the other.
        } else if popover_heard
            && self.a_popover_owns_escape()
            && ctx.input(|i| i.key_pressed(egui::Key::Escape))
        {
            // **The press is spent on the popover and must not also pay out a
            // rung** (§15 D801, D527's rule). Nothing is done here on purpose:
            // the three inspector popovers close themselves further down the
            // frame on the same `key_pressed`, and the Export card's two now do
            // the same. What this arm removes is the *second* thing the press was
            // doing — dropping the tool, leaving a group, clearing the selection
            // — which is the state the user could still see behind the popover.
            //
            // ⚠️ **It gates `Escape` alone and lets every other key through**,
            // unlike the modal arm above. See `a_popover_owns_escape`.
            //
            // 🚨 **And only a popover that was heard last frame** (§15 D847,
            // `[X6-L1-01]`). A flag whose handler did not run is a popover
            // nobody can see; see `popover_heard`.
        } else {
            // **A dead flag is cleared rather than left to swallow the next
            // press** — nothing is drawing it, so nothing else ever would, and a
            // Type popover left over from a text selection would otherwise pop
            // back up the next time one is made. The press then pays out its rung
            // like any other: the user saw no popover to spend it on.
            if self.a_popover_owns_escape() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.type_menu = None;
                self.stroke_menu = None;
                self.stroke_dash_text = None;
                self.effect_menu = None;
                self.export_menu = false;
                self.export_row_open = None;
            }
            for action in input::resolve(&ctx, self.mode, self.prefs.nudge) {
                self.dispatch(&ctx, action);
            }
        }

        // Files dropped on the window, whatever the tool. Not an `Action`,
        // because it is not a key: `raw.dropped_files` is a per-frame list egui
        // fills from winit and nothing else in the app reads.
        //
        // 🚨 **A modal takes this too, and did not until §15 D759.** The arm above
        // is D464's — a modal owns the *keyboard* — and this is the same question
        // for the pointer, which nobody had asked. `raw.dropped_files` is filled
        // from winit before any widget runs, so the `egui::Modal` backdrop that
        // blocks every click behind it does not touch a drop: a picture dragged
        // onto the window while the Settings card, the *Unsaved changes* card or
        // the recovery card was up **committed ops to the document behind the
        // backdrop**, exactly as `Delete` did before D464.
        //
        // ⚠️ **Worst on the recovery card**, for D464's own reason: it is drawn
        // above the view branch, so it can be up over a freshly reopened document
        // while asking about a *different* one's snapshot — and a drop there also
        // marks the session dirty, which mints a second snapshot key and is the
        // route §15 D681 is about.
        if !self.modal_is_up() {
            self.take_dropped_images(&ctx);
        }

        // Right-click abandons whatever is in flight — a layer being dragged, a
        // handle being resized or rotated, a shape being drawn, a value field
        // being scrubbed. The convention comes from CAD and 3D tools rather
        // than from drawing ones, and it earns its place for the same reason it
        // does there: the alternative is finishing a gesture you already know
        // is wrong and then undoing it, which leaves a step in the history for
        // something that never should have been an edit.
        //
        // Before the UI runs, so this frame's widgets already know. A strict
        // no-op when nothing is in flight, which is what leaves the right-click
        // that removes a gradient stop (`picker::stop_bar`) alone.
        // **And the press position is kept for the menu the release may open**
        // (`docs/context-menus.md` §0 R1). egui delivers `secondary_clicked` on the
        // release and has forgotten the press by then, so a menu placed from the
        // release would slide out from under a pointer that jittered a few pixels
        // between the two. Recorded whether or not the press cancels anything —
        // the two halves of the click are decided separately, and this half is
        // just a position.
        if ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary)) {
            self.secondary_press = ctx.pointer_interact_pos();
            self.cancel_gesture(&ctx);
        }

        // **Whatever the background writer finished, before anything reads the
        // state it changes.** One drain for both ticks and for the pill the
        // panels draw — see [`OndinApp::disk_results`].
        self.disk_results();
        // **After the frame's edits have been applied, so an edit made this
        // frame can be the one that trips the interval.** Before the panels,
        // because the save pill they draw should read the state this produced
        // rather than last frame's.
        self.autosave_tick(&ctx);
        // ⚠️ **Still after it, but no longer for the reason it used to be.** This
        // read "so a frame that autosaved leaves no snapshot behind", because
        // `autosave_tick` used to take the session from dirty to clean and this
        // reads exactly that. It cannot any more: a queued save leaves the
        // session dirty until its result arrives, which is a later frame's
        // `disk_results` above. What the order buys now is smaller and still
        // real — the frame the *result* lands on drops the snapshot before the
        // tick could take a new one of a document that has just been written.
        //
        // ⚠️ **And the snapshot does not double up in between**, which is the
        // question this ordering used to answer: the tick is gated on
        // `recovery.at`, so a revision already snapshotted is not snapshotted
        // again just because a save of it is in flight.
        self.recovery_tick(&ctx);

        // Present mode overrides every other chrome switch for as long as it is
        // on, without changing any of them — so leaving it restores the
        // workspace the user had rather than a default one. Escape is the way
        // out, handled in `escape` ahead of everything else it unwinds, because
        // the menu that turned it on is one of the things it hid.
        if !self.present {
            self.top_bar(ui);
            if self.show_layers {
                self.layers_panel(ui);
            }
            if self.show_toolbar {
                self.tool_rail(ui);
            }
        }

        // Before the canvas, because the canvas is what reads the answer. The note
        // it folds was left by the *previous* frame's inspector — see the function.
        self.fold_chrome_hold(&ctx);
        // Beside it because it is the same shape of problem — a deadline nothing
        // else would wake the loop for. See `fold_opacity_entry`.
        self.fold_opacity_entry(&ctx);

        // The canvas takes the whole remaining area; the inspector and the
        // colour picker float over it, so both are shown after it.
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(color::BG))
            .show(ui, |ui| {
                self.canvas_ui(ui);
            });

        if !self.present {
            self.inspector_panel(&ctx);
            self.picker_ui(&ctx);
        }
        // **After every door has had its say**, so a right-click that *replaced*
        // the open menu has already written the slot by the time the click-away
        // test would otherwise have run on the release that opened it (R4).
        //
        // 🚨 **Outside the `!present` guard, and the reason has reversed** (§15
        // D756). This read *"no menu can be open in present mode, since
        // `open_context_menu` refuses there, so the guard would only hide the
        // fact"*. The refusal is gone — a context menu is the canvas answering a
        // click, and present mode leaves the canvas working (D750) — so a menu
        // **can** be open here and this call is now load-bearing rather than
        // merely honest. ⚠️ Being outside the guard is what made the change a
        // deletion at one site instead of a repair at two: had the draw been
        // inside it, lifting the refusal would have opened a menu that never
        // painted, with every gate green.
        self.context_menu_ui(&ctx, nav);
        // Last, and outside the `!present` guard on an argument of its own: the
        // only door onto the settings card is a top-bar button, which present mode
        // has already hidden, so a guard here would only hide that fact. ⚠️ **This
        // used to say *"for the context menu's reason"* and the two reasons have
        // come apart** (§15 D756) — the menu's is now that it genuinely opens in
        // present mode, and this one is still that nothing can open it.
        self.settings_ui(&ctx);

        // **The frame that acts on an inspector edit has to be asked for.** The note
        // was left by a panel a few lines above and is read at the *top* of the next
        // frame, so that frame has to exist — and nothing here guarantees one:
        // `EditorSession::commit` requests no repaint, and a click's release event
        // was consumed by this frame, which is what made `clicked()` true. A scrub
        // rides its own pointer motion and a keystroke usually rides the key
        // release, which is exactly the kind of accident that reads as a guarantee
        // until the one case without it turns up — a button press being that case
        // (§15 D128).
        if self.pending_edit.is_some() {
            ctx.request_repaint();
        }

        // Once the button the cancelled gesture was riding on is up, the next
        // press is a fresh gesture. End of frame, not the start: the release
        // that ends the press is the same event the suppression above exists
        // to swallow.
        if ctx.input(|i| !i.pointer.any_down()) {
            self.gesture_cancelled = false;
            // With nothing pressed there is no scrub left to cancel, so the
            // snapshot a cancel would have wound back is spent. Cleared here rather
            // than on `drag_stopped`, which the session-scoped valves do not
            // reliably see: they are only reached on a frame the field *changed*.
            self.in_flight.session_scrub = None;
            self.in_flight.settings_scrub = None;
        }

        // **`Tab` belongs to the point selection while a path is being edited, and
        // taking it back costs a frame.** egui drives its own focus ring with Tab,
        // and `input::resolve`'s guard is `egui_wants_keyboard_input`, which is
        // "**anything** is focused" rather than "a text field is taking
        // characters" — so the first Tab stepped a point, the pass that followed
        // put focus on some button in the chrome, and every press after that was
        // dropped before the keymap ran. Reported as Tab working exactly once.
        //
        // It cannot be fixed by consuming the key: egui reads focus movement out of
        // the `RawInput` at the start of the pass, before this function is called.
        // And it cannot be fixed *earlier* in the frame either, because the move
        // happens while the panels below are being built. So the focus is handed
        // back here, at the end of a frame in which the key was ours — which also
        // scopes it exactly: outside point editing the ring is untouched, and a
        // rename field that owned the keyboard when the key arrived keeps its own
        // `Tab` (`took_tab` was answered before any of this frame's widgets ran,
        // which is the only moment the two can be told apart).
        if took_tab && let Some(focused) = ctx.memory(|m| m.focused()) {
            ctx.memory_mut(|m| m.surrender_focus(focused));
        }

        // **After the surrender above, which is the only ordering that is true**
        // (§15 D821). A frame whose `Tab` was ours ends with nothing focused, so
        // recording before that line would tell the *next* frame's `Escape` that a
        // field had the keyboard when the app had just taken it away. See
        // [`Self::chrome_focus`] for what this answers and why egui cannot be
        // asked directly.
        self.chrome_focus = ctx.memory(|m| m.focused()).is_some();
    }
}

// --- action dispatch ------------------------------------------------------

impl OndinApp {
    /// `Ctrl+Z`, and the top bar's button — [`EditorSession::undo`] plus the
    /// app-side state that has to go with it.
    pub(crate) fn undo(&mut self) {
        if self.session.undo() {
            self.document_rewound();
        }
    }

    /// `Ctrl+Shift+Z`. Redo is a structural edit by the same argument: it moves
    /// the document under state that indexes into it.
    pub(crate) fn redo(&mut self) {
        if self.session.redo() {
            self.document_rewound();
        }
    }

    /// **The document just moved under us.** Drop the app-side state that names
    /// parts of it *by index* rather than by id (§15 D568).
    ///
    /// `EditorSession::undo` already guards its own half — `Selection` is a set of
    /// `NodeId`s, so `retain_existing` is a truncation and a survivor still means
    /// the same layer. The two fields here are not like that:
    ///
    /// - [`PointSet`] holds `PointRef`s, which are `(subpath, anchor)` **indices**.
    ///   `PointSet::retain_valid`'s own doc states the rule — *"deleting an anchor
    ///   renumbers every anchor after it in that subpath, so a set kept across the
    ///   edit would silently point at the wrong points"* — and every structural
    ///   edit in the node tool obeys it (`canvas::delete_points`,
    ///   `remove_segments`, `insert_point`, `join_points`, `end_pen_gesture`).
    ///   Undo was the one door with nothing.
    /// - [`PenState`] is worse, because it is not merely a reference: `PenEdit`
    ///   carries a **snapshot** of every subpath the node had when the pen resumed,
    ///   and `finish_pen` writes the whole list back. An undo mid-run followed by
    ///   finishing the run therefore *resurrects* the geometry the undo took away.
    ///
    /// ⚠️ **`retain_valid` is not the fix here and truncation would hide it.** The
    /// measured failure keeps every index in range — delete the middle anchor of
    /// three, pick the last one, `Ctrl+Z` — so a re-validation is a no-op and the
    /// set goes on naming a *different* anchor. Only clearing answers it, which is
    /// the argument `end_pen_gesture` already gives for the identical case.
    ///
    /// Not on `EditorSession` because neither field is: both live on `OndinApp`,
    /// which is why the session structurally could not have done this itself.
    fn document_rewound(&mut self) {
        self.points.clear();
        self.pen = None;
    }

    pub(crate) fn dispatch(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            // 🚨 **The three arms that act on the document finish a live text
            // session first** (§15 D815). These chords resolve in
            // `Mode::TextInsert` now, and a `Document` edited while a session is
            // open is one the session's buffer has not been written back into —
            // the two then disagree until the session ends and commits over
            // whatever happened in between. `Save` was the worst of the three: it
            // writes *and pins a version* from `session.doc`, which does not hold
            // the typed text.
            //
            // **At the dispatch seam rather than in `input::text_insert_mode`**,
            // because this is a fact about the *action* and not about the key that
            // produced it: the top bar's buttons already carry it (§15 D466), and
            // a guard in the keymap would be a second copy that the next door onto
            // `Undo` would miss. A no-op outside a session, `finish_text_first`
            // asking `self.text.is_some()` first.
            Action::Undo => {
                self.finish_text_first();
                self.undo();
            }
            Action::Redo => {
                self.finish_text_first();
                self.redo();
            }
            // Pins, which is what `Ctrl+S` means now that autosave writes the
            // file anyway (`input::Action::Save`).
            Action::Save => {
                self.finish_text_first();
                self.save_file(true);
            }
            // No `finish_text_first` of its own: `go_to_dashboard` has called it
            // since §15 D466 and is the one spelling of that walk.
            Action::Open => self.go_to_dashboard(),
            // **The same two functions the dashboard's own buttons call.** `Ctrl+N`
            // is the *New file* card and `Ctrl+W` is the walk back that the brand
            // mark, the folder button and `Ctrl+O` above already take — so neither
            // chord needs a guard of its own: `new_library_document` files before
            // it opens, and `go_to_dashboard` autosaves, drops the crash snapshot
            // and re-scans.
            Action::NewDocument => {
                let project = self.open_document_project();
                self.new_library_document(project);
            }
            Action::CloseDocument => self.go_to_dashboard(),
            // `toggle_settings` rather than an open, because this is the same door
            // the top bar's button uses and it cannot arrive with the modal already
            // up — `resolve` is not called while it is (`Action::OpenSettings`).
            Action::OpenSettings => self.toggle_settings(ctx),
            // No dialog when a destination is already known, which is the whole of
            // what makes this a *repeat* rather than a second spelling of the row
            // above it — but a person pressing the chord is asking, so the first
            // one of a session finds out where.
            Action::ExportAll => self.export_all(crate::panels::ExportAll::Asking),
            Action::PlaceImage => self.place_image_action(),
            Action::Copy => {
                self.copy_selection(ctx);
            }
            Action::Cut => self.cut_selection(ctx),
            Action::Paste => self.paste(),
            Action::PasteInPlace => self.paste_in_place(),
            // **The weaker witness, and the exact condition that makes it the only
            // one.** `egui_winit` pushes `Event::Paste` when and only when the
            // clipboard yields non-empty text, so text on the clipboard *proves* the
            // event fired and this release is the second half of one keystroke; no
            // text proves it did not, and this is the whole signal (§15 D183, D219).
            Action::PasteRelease => {
                if system_clipboard_text().is_none() {
                    self.paste();
                }
            }
            Action::CopyProperties => self.copy_properties(),
            Action::PasteProperties => self.paste_properties(),
            Action::Duplicate => self.duplicate_selection(),
            // **While the pen is drawing, Backspace takes back the last anchor**
            // rather than deleting the selection. Intercepted here rather than in
            // `input::resolve` because that function is deliberately a pure
            // function of the keymap and the mode, and a pen in flight is neither
            // — and here is also the only place early enough, since keyboard
            // actions are dispatched before `pen_input` runs, so a plain
            // `Action::Delete` would already have deleted a layer by then.
            Action::Delete if self.pen.is_some() => self.pen_backspace(),
            // The node tool's Delete removes the selected *points*, healing the
            // path — the same interception as the pen's Backspace above, and for
            // the same reason: by the time the canvas ran, the layer would be
            // gone. The guard is "points are selected", so Delete with the tool
            // armed and nothing picked still deletes the layer, which is what a
            // Delete with no narrower subject has always meant.
            Action::Delete if !self.points.is_empty() && self.edited_path().is_some() => {
                self.delete_points()
            }
            Action::Delete => self.delete_selection(),
            Action::SelectAll => self.select_all(),
            // The pen finishes its path on Enter, and reads the key itself in
            // `pen_input`. Letting this arm run too would act twice on one press
            // — the same shape as the Backspace interception above, from the
            // other side.
            Action::Enter if self.pen.is_some() => {}
            Action::Enter => self.enter_action(),
            // **The press that closes a top-bar dropdown closes only it** (§15
            // D527) — the rule `ui`'s context-menu arm already states in as many
            // words, made structural the same way: this arm consumes the key
            // rather than reaching the ladder. Without it one Escape over an
            // open Zoom, View or Snap menu closed the menu **and** dropped the
            // tool to Select, which is the failure that rule names — *"eating
            // the very state the menu was aimed at"*.
            //
            // ⚠️ `input::resolve`'s `egui_wants_keyboard_input` guard cannot
            // stand in: measured, opening a dropdown by clicking its head leaves
            // `focused() == None`, so egui does not think anything wants the
            // keyboard and the action is resolved as usual.
            Action::Escape if self.open_menu != TopMenu::None => {
                self.open_menu = TopMenu::None;
            }
            Action::Escape => self.escape(ctx),
            Action::Group => self.group_selection(),
            Action::Mask => self.toggle_mask(),
            Action::Boolean(op) => self.apply_boolean(op),
            Action::Flatten => self.flatten_selection(),
            Action::Ungroup => self.ungroup_selection(),
            // With points selected, the arrows move the **points**, not the
            // layer they belong to. Same interception as Delete above, and the
            // same reason: keyboard actions are dispatched before the canvas
            // runs. It also matters more here than anywhere — the arrows are the
            // one way to move a point by an exact amount, so moving the wrong
            // thing is the wrong answer to the question they are being asked.
            // **The arrows belong to a shape being drawn while one is being
            // drawn** — they add and remove its sides (`canvas::reshape_create`,
            // `docs/shortcuts.md` §9). Dropped rather than never resolved, because
            // the keymap is deliberately a pure function of one frame's input
            // and knows nothing about a gesture in flight; this is the same
            // place `Enter` is handed to the pen rather than acted on twice.
            Action::Nudge(_) if matches!(self.drag, Drag::Create { .. }) => {}
            Action::Nudge(delta) if !self.points.is_empty() && self.edited_path().is_some() => {
                self.nudge_points(delta)
            }
            Action::Nudge(delta) => self.nudge(delta),
            // Dropped mid-create for the reason the nudge above is: a shape whose
            // size is being drawn by the pointer must not also be taking it from
            // the keyboard.
            //
            // **No point-editing arm, unlike the nudge.** The arrows move points
            // because a point selection is what the node tool is *about*; a width
            // is a fact about the whole layer whichever of its anchors happen to be
            // picked, so this stays aimed at the selection and needs no second
            // reading.
            Action::SizeStep(_) if matches!(self.drag, Drag::Create { .. }) => {}
            Action::SizeStep(d) => self.size_step(d),
            // Intercepted here for the third time and the same reason as Delete
            // and the arrows: keyboard actions are dispatched before the canvas
            // runs. The todo predicted this one by name.
            Action::StepPoint(forward) => self.step_point(forward),
            Action::Restack(mv) => self.restack(mv),
            // Not intercepted the way Delete and the arrows are: it has a chord of
            // its own, so there is no layer-scoped action underneath it to lose to.
            // It does read the *layer* selection when no points are picked, which is
            // what makes it work outside point editing.
            Action::ReverseSubpaths => self.reverse_subpaths(),
            Action::ChooseTool(tool) => self.choose_tool(tool),
            Action::ZoomIn => {
                self.session
                    .camera
                    .zoom_by(1.2, self.canvas_center(), self.canvas_px)
            }
            Action::ZoomOut => {
                self.session
                    .camera
                    .zoom_by(1.0 / 1.2, self.canvas_center(), self.canvas_px)
            }
            Action::ZoomReset => self.session.camera.zoom = 1.0,
            Action::ZoomFit => self.zoom_to(self.document_bounds()),
            Action::ZoomSelection => self.zoom_to(self.selection_bounds()),
            Action::ToggleView(sw) => self.set_view_switch(sw, !self.view_switch(sw)),
            Action::ToggleHidden => self.toggle_hidden(),
            Action::ToggleLocked => self.toggle_locked(),
            Action::Rename => self.rename_selection(),
            Action::Flip(axis) => self.flip(axis),
            Action::Align(axis, edge) => self.align(axis, edge),
            Action::Distribute(axis) => self.distribute(axis),
            Action::OpacityDigit(d) => self.opacity_digit(ctx, d),
            Action::TextStyle(chord) => self.text_chord(chord),
        }
        ctx.request_repaint();
    }

    /// Where a whole-selection switch moves, given each member's state **in the
    /// sense of the verb** — *hidden* for the hide switch, *locked* for the lock
    /// one.
    ///
    /// Everything moves to applied unless everything already is, which is the
    /// only case that releases. So a **mixed** selection hides, and locks: *Hide*
    /// is a verb rather than a toggle, and someone pressing it on five layers of
    /// which two are already hidden is tidying up, not asking for the two back.
    ///
    /// **This went the other way first, and the argument that lost is worth
    /// keeping**, because it is the one that comes back. A hidden or locked layer
    /// is unpickable — `query.rs`'s hit test skips `!visible() || locked()` — so
    /// hiding a mixed selection puts layers somewhere the canvas cannot reach.
    /// The chord still undoes itself, because neither operation drops the
    /// selection (`retain_existing` prunes only ids the document has lost), but
    /// that recovery is **one stray click wide**: click anywhere else and the
    /// only way back is the layers panel, which may be shut. Releasing on mixed
    /// avoids that and costs a press — and it was rejected anyway, because
    /// normalising *away* from what the verb says is the stranger surprise, and
    /// `Ctrl+Z` is a better answer to a mis-click than a chord that second-guesses
    /// the verb every time.
    ///
    /// The two call sites differ only in polarity, which is the mistake this
    /// exists to prevent: `visible` is the negation of the answer and `locked`
    /// is the answer.
    fn set_switch(applied: impl IntoIterator<Item = bool>) -> bool {
        !applied.into_iter().all(|a| a)
    }

    /// [`Self::set_switch`] for hiding: takes the members' current `visible` and
    /// returns the `visible` they all move to.
    ///
    /// **The negation lives here rather than at the call site**, and that is the
    /// point of the wrapper. `toggle_hidden` reads `n.visible()` — the field's own
    /// name, no `!` — so a polarity slip would mean *adding* a negation for no
    /// reason rather than forgetting one, which is a much harder mistake to make
    /// and a much easier one to see. It also puts the inversion somewhere a test
    /// can hold it: an earlier test undid the polarity itself and so passed
    /// against a `toggle_hidden` with the negation dropped, which is the shape of
    /// vacuous test this file's own notes warn about.
    fn hide_switch(visible: impl IntoIterator<Item = bool>) -> bool {
        !Self::set_switch(visible.into_iter().map(|v| !v))
    }

    /// [`Self::set_switch`] for locking: takes the members' current `locked` and
    /// returns the `locked` they all move to. No inversion — this switch's verb
    /// and its field are the same word — and it exists anyway so the two chords
    /// are spelled alike at their call sites.
    fn lock_switch(locked: impl IntoIterator<Item = bool>) -> bool {
        Self::set_switch(locked)
    }

    /// Hide or show the whole selection — `Ctrl+Shift+H` (`docs/shortcuts.md` §4).
    ///
    /// **One switch for the set, not one per layer** — a per-layer flip would make
    /// the chord's effect depend on a count the user cannot see without the
    /// layers panel open, so it could not be predicted before pressing it. Which
    /// way the set goes is [`Self::set_switch`], where the mixed case is argued.
    ///
    /// `session.commit` rather than [`Self::commit_edit`], **and not for the reason
    /// this comment used to give**. It claimed the commit "must not be gated on
    /// the layer being editable"; `commit_edit` gates nothing — it is `note_edit`
    /// then the same `session.commit`, and its only addition is arming the
    /// selection-chrome hide. The real reason is that the hide is an *inspector*
    /// affordance, for getting the box out of the way of a value being looked at,
    /// and a visibility switch has nothing to look at. `commit_edit`'s own doc
    /// already names this as one of its two sanctioned exceptions — "the layers
    /// panel, which is not the inspector" — and that panel's eye and lock buttons
    /// commit the very same two operations the same way, so the chord and the
    /// button agree.
    fn toggle_hidden(&mut self) {
        let ids = build::outermost(&self.session.doc, self.session.selection.ids());
        let visible = Self::hide_switch(
            ids.iter()
                .filter_map(|id| self.session.doc.get(*id))
                .map(|n| n.visible()),
        );
        let ops: Vec<Operation> = ids
            .iter()
            .map(|id| Operation::SetVisible { id: *id, visible })
            .collect();
        if ops.is_empty() {
            return;
        }
        self.session.commit(Transaction(ops));
        self.session.info(if visible { "Shown" } else { "Hidden" });
    }

    /// Lock or unlock the whole selection — `Ctrl+Shift+L` (`docs/shortcuts.md` §4).
    /// Mixed **locks**, per [`Self::set_switch`].
    fn toggle_locked(&mut self) {
        let ids = build::outermost(&self.session.doc, self.session.selection.ids());
        let locked = Self::lock_switch(
            ids.iter()
                .filter_map(|id| self.session.doc.get(*id))
                .map(|n| n.locked()),
        );
        let ops: Vec<Operation> = ids
            .iter()
            .map(|id| Operation::SetLocked { id: *id, locked })
            .collect();
        if ops.is_empty() {
            return;
        }
        self.session.commit(Transaction(ops));
        self.session
            .info(if locked { "Locked" } else { "Unlocked" });
    }

    /// Open the rename field on the selected layer — `Ctrl+R` / `F2`
    /// (`docs/shortcuts.md` §4).
    ///
    /// **The same field a double-click in the layers panel opens**, armed the same
    /// way: `renaming_layer` is the whole of the state, and `layers::layer_rename_field`
    /// draws it, asks for focus and commits it. So the chord is a second way into
    /// one rename rather than a second rename.
    ///
    /// Two consequences of that, both deliberate. It **needs the layers panel**,
    /// so the panel is shown if it was hidden — a rename armed against a panel
    /// nobody can see would swallow the next keystrokes with nothing on screen to
    /// explain it. And it is a **single**-layer affordance: with several selected
    /// there is no one row to lay the field over, so it renames nothing and says
    /// so.
    fn rename_selection(&mut self) {
        // **Not in present mode, and this was a live bug for the length of one
        // review.** Present mode draws no panels at all — the whole `top_bar` /
        // `layers_panel` / `tool_rail` group sits behind `if !self.present` — so
        // setting `show_layers` there shows nothing, and the rename would arm
        // against a field that never renders: no caret, no way to cancel it, and
        // then a rename box springing open on some layer the moment present mode
        // ended. Armed invisible state is worse than a refused chord, so this is
        // refused, out loud.
        if self.present {
            self.session.info("Leave present mode to rename");
            return;
        }
        let ids = self.session.selection.ids();
        let [id] = ids else {
            if !ids.is_empty() {
                self.session.info("Select one layer to rename");
            }
            return;
        };
        let Some(name) = self.session.doc.get(*id).map(|n| n.name().to_string()) else {
            return;
        };
        self.show_layers = true;
        self.renaming_layer = Some((*id, name));
        // **And the caret goes into it**, which this did not do for as long as the
        // chord existed — reported against the menu row, which is simply the more
        // discoverable of the two doors onto the same function. Opening a field
        // nobody is typing into is worse than not opening one: the panel looks
        // ready and the first keystroke goes to the canvas.
        //
        // ⚠️ **Not asked for here, though it was.** This function ran the request
        // itself until it turned out to crash the app from the *menu* row: the
        // context menu is drawn after the layers panel, so the field this asks about
        // is not in that frame at all, and a focused id with no AccessKit node
        // behind it panics `accesskit_consumer`. `layer_rename_field` asks on the
        // frame it first draws instead — which for the chord is *this* frame, the
        // panel being drawn after the keymap runs, and for the menu is the next one.
        //
        // Which is why this no longer takes a `Context` at all: nothing about
        // opening a rename is a question for egui.
    }

    /// Apply a digit to the selection's opacity — `docs/shortcuts.md` §2.
    ///
    /// **One digit is a coarse value, two within 600 ms are an exact one.** `4`
    /// is 40%, `4`,`5` is 45%, `0` alone is 100% and `0`,`0` is 0%. Universal
    /// across Figma, Photoshop and Illustrator, and the reason the second digit
    /// cannot simply be a second edit is undo: `4`,`5` has to leave *one* step in
    /// the history reading 45%, not a 40% the user never asked for with a 45% on
    /// top of it.
    ///
    /// So the first digit is a **preview** — live on screen, absent from the
    /// history — and the commit happens when the window shuts, whichever way it
    /// shuts: a second digit lands the exact value, and the deadline lands the
    /// coarse one. [`Self::fold_opacity_entry`] is the second of those, and the
    /// only reason this needs a clock at all.
    fn opacity_digit(&mut self, ctx: &egui::Context, digit: u8) {
        if self.session.selection.is_empty() {
            return;
        }
        let now = ctx.input(|i| i.time);
        let subject = self.session.selection.ids().to_vec();
        match opacity_step(self.opacity_entry.take(), digit, now, &subject) {
            OpacityStep::Wait(pct, entry) => {
                self.set_opacity_pct_on(&subject, pct, false);
                self.opacity_entry = Some(entry);
            }
            OpacityStep::Land(pct) => self.set_opacity_pct_on(&subject, pct, true),
        }
    }

    /// Close the two-digit window once it has run out, committing the coarse
    /// value the first digit previewed.
    ///
    /// **A timer in a reactive app has nothing to wake it.** Nothing wakes the
    /// frame loop after the last keypress, so without the `request_repaint_after`
    /// below a lone `4` would sit on screen at 40% for ever, previewed and never
    /// committed — and vanish on the next unrelated repaint. Same stranded-first-wake
    /// shape as `fold_chrome_hold`'s 1.5s and the font-face repaint (§15 D110), and
    /// asked on every pending frame for the same reason: egui keeps the earliest
    /// request, so repeating it is free and cannot be forgotten.
    fn fold_opacity_entry(&mut self, ctx: &egui::Context) {
        let Some(entry) = self.opacity_entry.clone() else {
            return;
        };
        let now = ctx.input(|i| i.time);
        if opacity_window_open(&entry, now) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                OPACITY_DIGIT_WINDOW - (now - entry.at),
            ));
            return;
        }
        self.opacity_entry = None;
        // **`entry.subject`, not the live selection** (§15 D470). The window is
        // 600 ms wide and the selection is free to move inside it; committing
        // against what is selected *now* put the typed value on a layer the user
        // had merely clicked. See `OpacityEntry::subject`.
        self.set_opacity_pct_on(&entry.subject, opacity_of_digit(entry.first), true);
    }

    /// Preview or commit one opacity value across `ids`.
    ///
    /// `build::set_opacity_all` is the same transaction the inspector's opacity
    /// field builds, so a value typed as digits and one scrubbed on the field are
    /// the same edit.
    ///
    /// ⚠️ **The subject is an argument and used to be the live selection**
    /// (§15 D470). Both callers are the two-digit window's, and the one that runs
    /// at the *deadline* is 600 ms after the keystroke — long enough for the
    /// selection to have moved, which is `[S16.1-L1-02]`.
    fn set_opacity_pct_on(&mut self, ids: &[NodeId], pct: f64, commit: bool) {
        let tx = ondin_core::set_opacity_all(
            &self.session.doc,
            ids,
            (pct / 100.0).clamp(0.0, 1.0) as f32,
        );
        if tx.0.is_empty() {
            return;
        }
        if commit {
            self.session.clear_preview();
            if self.commit_edit(tx) {
                self.session.info(format!("Opacity {pct:.0}%"));
            }
        } else {
            self.session.set_preview(&tx);
            self.session.info(format!("Opacity {pct:.0}%"));
        }
    }

    fn canvas_center(&self) -> Vec2 {
        Vec2::new(self.canvas_px.0 as f64 / 2.0, self.canvas_px.1 as f64 / 2.0)
    }

    /// Abandon whatever gesture is in flight and leave no trace of it: the
    /// preview is dropped, so the layer snaps back to what the document
    /// already says, and nothing is committed, so history never learns it
    /// happened. Returns whether there was anything to abandon.
    ///
    /// Covers both kinds of gesture, because the user does not distinguish
    /// them: a drag on the canvas, which is ours (`Drag`), and a value field
    /// being scrubbed in the inspector, which is egui's.
    ///
    /// `gesture_cancelled` is the half that is easy to miss. The pointer is
    /// still down at this point — cancelling does not end the press — so the
    /// release that eventually comes would otherwise arrive as a perfectly
    /// ordinary `drag_stopped` and commit the edit after all.
    pub(crate) fn cancel_gesture(&mut self, ctx: &egui::Context) -> bool {
        // **Three tests for one field scrub, because egui's own answer goes missing
        // on exactly the frame this is asked.** `Context::dragged_id` is the obvious
        // one and it was the only one: egui clears `dragged` on the release of *any*
        // button (`interaction.rs`'s `PointerEvent::Released` arm ignores which), and
        // this runs on `button_pressed(Secondary)` — so a right-click whose press and
        // release fall in the same rendered frame reads a snapshot that has already
        // been cleared. At 60fps that is any click under ~16ms, which is why it was
        // reported as working "almost always" and failing at random with no pattern
        // in where or how. `cancel_gate::a_right_click_short_enough_to_fit_one_frame_takes_the_drag_away`
        // pins the behaviour.
        //
        // The other two are ours and do not blink: a gesture preview is installed by
        // the valve on every frame of a scrub, and `session_scrub` is armed for the
        // one kind of scrub that previews through the *session* instead. Both are
        // absent when nothing is in flight, which is what keeps this a strict no-op
        // and leaves the right-click that removes a gradient stop
        // (`picker::stop_bar`) alone.
        // 🚨 **This was a hand-written `||` chain over nine fields and it went
        // stale twice** (§15 D785, `[A2-L7-02]`). `[A1-L2-02]` found it two short —
        // `export_quality_scrub` and `guide_position_drag`, so a right-click on
        // either field did nothing and the value committed on release — and
        // `paint_drag` had been a third, reached only by the blinking
        // `dragged_id()` term (§15 D588). Three of the chain's clauses had been
        // added one at a time, each after a field appeared elsewhere and nothing
        // told the gate, and one of them said so: *"which is the shape of bug this
        // whole list is made of"*.
        //
        // `InFlight::any` is a `PartialEq` against `Default`, so it is **exhaustive
        // by construction**: an eighth value a gesture holds is covered the day it
        // is added to that struct, with nothing here to remember. The two terms
        // beside it are not fields and cannot move into it.
        let field = ctx.dragged_id().is_some()
            || self.session.has_gesture_preview()
            || self.in_flight.any();
        // ⚠️ **What the chain's own comments recorded, kept because it is evidence
        // about how this failure happens rather than about the code.**
        // `export_quality_scrub` and `guide_position_drag` were missed because
        // neither shows through `has_gesture_preview`, for two different reasons:
        // `SetExports` never reaches `set_preview` at all, and `SetGuidePosition`
        // is *absorbed as a no-op*, so `overrides` stays empty. A guide dragged on
        // the **canvas** was covered — it sets `Drag::Guide`, caught by
        // `self.drag` — which is exactly why the inspector's field read as working.
        // `paint_drag` was covered only by `dragged_id()`, the blinking term above:
        // egui does not re-arm the drag while the primary stays down, so one fast
        // right-click made the gesture uncancellable for good. Measured at the
        // time: `[4.0, 9.0] → [9.0, 4.0]`, `undo_depth +1`, on a release after two
        // cancels.
        if self.drag.is_none() && self.layer_drag.is_none() && !field {
            return false;
        }
        self.drag = Drag::None;
        // The guide valve's preview half. `clear_preview` below does the same
        // for everything that goes through `RenderOverrides`; a guide does not
        // (`rulers.rs`), so it has to be dropped by name or the line stays
        // where the cancelled scrub left it.
        self.guide_previews.clear();
        // A row being dragged in the layers tree is the third kind, and it has
        // to be dropped here rather than left to `finish_layer_drag`: that runs
        // on the release, which is still coming. A paint row being dragged to
        // reorder its list is the fourth, for the same reason.
        self.layer_drag = None;
        self.in_flight.paint_drag = None;
        // And the copy an Alt-drag was carrying, for the same reason: the
        // release that would have dropped it is not coming, and a stale one left
        // here is what the *next* Alt-drag would silently insert.
        self.alt_clone = None;
        // **And the view goes back with the layer.** The gesture snapping home is
        // only half of "leave no trace": if the drag had auto-scrolled its way
        // across the document, the user is left looking at blank page, with the
        // thing they were dragging somewhere off screen and no clue which way.
        // Only this feature's own scrolling is undone — a zoom or a wheel-pan made
        // during the drag was asked for and stays.
        self.session.camera.center -= std::mem::take(&mut self.autopan_total);
        // **Not `clear_preview`**, which would take a live text session's
        // uncommitted content off the canvas along with the cancelled gesture's
        // preview — the two share one override set and only one of them belongs to
        // the gesture. See `EditorSession::clear_gesture_preview`.
        self.session.clear_gesture_preview();
        // And a scrub that was writing into the session gets wound back by hand:
        // there was no preview holding it, so dropping one would undo nothing.
        if let Some((id, spans, para_spans)) = self.in_flight.session_scrub.take()
            && self.text.as_ref().is_some_and(|s| s.id == id)
        {
            self.restyle_session_with(spans, para_spans);
        }
        // And the Settings modal's nudge pair, for the same reason — its draft is
        // not a preview either. Safe to wind back here even though `settings_ui`
        // has not run yet this frame: it reads `self.settings` at its start, so it
        // sees the restored pair, and `gesture_cancelled` stops it applying the
        // motion that arrived on this very frame.
        if let Some(nudge) = self.in_flight.settings_scrub.take()
            && let Some(settings) = &mut self.settings
        {
            settings.set_nudge(nudge);
        }
        // And the layout grid's, which needs no winding back at all — nothing was
        // committed and nothing was previewed, so dropping the buffer is the whole
        // undo. It is dropped **here** rather than left to the panel's own
        // `gesture_cancelled` check because the canvas draws from it and draws
        // *before* the panels: leaving it for the panel would put one frame of the
        // cancelled grid on screen, which is exactly the kind of single bad frame
        // this app gets bug reports about.
        self.in_flight.grid_scrub = None;
        // The colour half, dropped for the same reason and in the same breath.
        self.in_flight.grid_paint = None;
        // **And the pending opacity digit, which is the one entry on this list that
        // used to outlive the cancel and land anyway** (§15 D535).
        // `[S16.2-L1-01]`: press `4`, watch the canvas preview 40%, press `Escape`
        // and watch it revert — and then 600 ms later the layer really does become
        // 40%, one undo step and all. The cancel *fired* (`set_opacity_pct_on` previews
        // through `set_preview`, so `has_gesture_preview` is true and this function
        // is the rung `escape` stops on) and cleared the preview, but the entry
        // itself was absent from every line above, so `fold_opacity_entry` — called
        // unconditionally on every editor frame — still found it and still ran the
        // commit arm on the deadline.
        //
        // **The digit window is a gesture even though nothing is held down**, which
        // is why it belongs here and not on the `escape` ladder: the right-click
        // cancel is the other door onto this function and it had the identical bug.
        // §15 D210's *"the commit happens when the window shuts, **whichever way it
        // shuts**"* enumerates two ways — a second digit, or the deadline — and a
        // cancelled gesture owes zero transactions (§9.3), so this is the third way
        // and it shuts the window without committing.
        self.opacity_entry = None;
        // **And the chrome comes straight back**, without the 1.5s courtesy. The hold
        // exists so you can see the result of an edit; a cancelled scrub has no
        // result, and the box is what you want back the instant the value snaps home
        // (§15 D128).
        self.chrome_hold = ChromeHold::Free;
        self.gesture_cancelled = true;
        ctx.stop_dragging();
        true
    }

    /// Leaving a tool or pressing Escape abandons whatever is half-done, in
    /// order of specificity: open picker, text session, pen path, gesture, pen
    /// bias, points, then selection. Escape unwinds whatever is in flight, one
    /// level per press.
    ///
    /// The order is "most transient first": a floating panel, then an editing
    /// session, then a gesture, then the pen bias armed over an edit, then the
    /// points selected in it, then the group you stepped into, then the tool,
    /// then the selection.
    /// Falling back to Select at the end is what makes Escape safe to lean on —
    /// a tool armed by a stray keystroke is otherwise only escapable by knowing
    /// that `V` exists.
    ///
    /// An in-flight pen path is *committed*, not discarded — Escape is the key
    /// everyone reaches for to mean "I'm done here", and throwing the path away
    /// is a lot to lose to it when Ctrl+Z is right there if it really was a
    /// mistake. It has to happen here rather than in `pen_input`, because
    /// keyboard actions are dispatched before the canvas runs: whichever of the
    /// two saw Escape first would win, and the loser would be this one turning
    /// the pen off underneath it.
    ///
    /// ⚠️ **This paragraph spent months on `cancel_gesture`** (§15 D697,
    /// `[S16.2-L3-05]`) — one more of the doc-comment thefts `CLAUDE.md`
    /// enumerates, and it kept its ordinal there rather than here so there is
    /// one count of them and not two — where it read
    /// as that function's preamble because there was no blank `///` between the
    /// two runs and `cancel_gesture` is one rung of the very ladder described
    /// here. Merged, the run came to 33 lines, well under the length ranking's
    /// floor; what finds it is asking whether the run's *first line* describes
    /// the item beneath it, and *"Leaving a tool or pressing Escape"* plainly
    /// does not describe *"abandon whatever gesture is in flight"*.
    fn escape(&mut self, ctx: &egui::Context) {
        // Present mode first: it is the only state whose own way out is hidden
        // by the state itself, so nothing **in this ladder** may take the key ahead
        // of it.
        //
        // ⚠️ **One thing outside the ladder does, and since §15 D756 it can
        // actually happen.** An open context menu holds the keyboard by R3 and is
        // answered at the top of `update`, which returns without ever calling this
        // function — so with a menu up in present mode the first Escape closes the
        // menu and the second lands here. That is the right order (the menu is the
        // only thing on screen the user can see to dismiss) and it costs no code:
        // it falls out of R3's arm already being a `return`. What it costs is a
        // sentence, because this comment and `architecture.md` §9.4 both used to
        // say the two could never contend, on the strength of a refusal in
        // `open_context_menu` that is gone.
        if self.present {
            self.present = false;
            self.session.info("Left present mode");
        } else if self.picker.take().is_some() {
            // The picker floats over the canvas and owns no gesture, so it is
            // the outermost thing Escape can dismiss.
        } else if self.text.is_some() {
            self.finish_text_edit();
        } else if self.pen.is_some() {
            self.finish_pen(false);
        } else if self.cancel_gesture(ctx) {
            // Whatever was being dragged goes back to where the document says
            // it is, and history never hears about it.
        } else if self.chrome_focus {
            // 🚨 **A chrome field surrendered focus to this very `Escape`, and the
            // key is spent on it** (§15 D821). egui clears focus in `begin_pass`,
            // so the press is still in `input` when the keymap runs and resolves
            // here — which is what makes the rung above work for a *valved* field,
            // whose preview `cancel_gesture` finds and drops. A plain `TextEdit`
            // installs no preview, so the ladder used to fall straight past it:
            // renaming a layer and pressing `Escape` abandoned the rename **and**
            // took the last rung, clearing the selection under the panel that was
            // showing it.
            //
            // ⚠️ **Below `cancel_gesture` and not above it**, which is the whole of
            // the placement: a numeric field's cancel has to go on reaching that
            // function, because it is what sets `gesture_cancelled` and stops the
            // release committing after all (§15 D317). This rung is only ever
            // taken when nothing was in flight — which is exactly the text-field
            // case.
            //
            // Nothing is undone here. The field has already abandoned its own edit
            // (`ui::defocus_commits`, §15 D808); what this adds is that the key
            // stops there.
        } else if self.pen_bias {
            // **The bias, above the points it was armed over.** It is the more
            // transient of the two — one keystroke on, one keystroke off, with the
            // point selection untouched underneath — so the ladder in the node tool
            // is now pen bias → points → deselect → Select (§15 D125). It is also
            // the way out to the *standalone* pen, since `choose_tool` will not
            // switch to it while an edit is live.
            self.pen_bias = false;
            self.session.info("Pen off; still editing points");
        } else if !self.points.is_empty() {
            // Points before the tool: the node tool's ladder is points →
            // deselect, Escape → Select, which is why it needs no Done button
            // and no click-outside rule (todo, *when a mode earns its keep*).
            self.points.clear();
        } else if self.show_pivot {
            // After the gesture, before the group scope: it is a switch the user
            // turned on and can see the effect of, so it unwinds where every other
            // such switch does — outside anything in flight, inside the selection
            // state that survives it. The pivots it was placing stay placed;
            // Escape puts the handle away, it does not undo the work.
            self.show_pivot = false;
        } else if let Some(group) = self.entered_group.take() {
            // Step back out of the group and select it, which is where the
            // double-click that entered it came from.
            self.session.selection.set_one(group);
        } else if self.tool != Tool::Select {
            self.choose_tool(Tool::Select);
        } else {
            self.session.selection.clear();
        }
    }

    /// Step one level into a group or a boolean, selecting something inside it.
    ///
    /// **Two doors, one verb** (§15 D228): the context menu's *Enter group* /
    /// *Enter boolean* rows, and `Enter` with one of those selected. It was the
    /// menu's alone, inlined at the dispatch arm, which is how the key came to be
    /// unbuilt while the row worked.
    ///
    /// `at` is the pointer where there is one. The menu has one and uses it to pick
    /// *what* to land on inside; a keypress does not, and falls back to the **last**
    /// child — the topmost in z, which is the one the eye is already on.
    pub(crate) fn enter_container(&mut self, id: NodeId, at: Option<Point>) {
        self.entered_group = Some(id);
        let inside = at
            .and_then(|p| self.pick_leaf(p))
            .filter(|leaf| ondin_core::is_within(&self.session.doc, *leaf, id))
            .or_else(|| {
                self.session
                    .doc
                    .get(id)
                    .and_then(|n| n.children().last().copied())
            });
        self.session.selection.set_one(inside.unwrap_or(id));
    }

    /// Enter: step into the selection, or step back out.
    ///
    /// **A toggle, where Escape is a ladder.** Escape unwinds one rung per press
    /// — points, then the tool, then the selection — which is right for "undo the
    /// state I am in". Enter is the other verb: one press in, one press out,
    /// symmetric with the double-click that also goes in. Making it a literal
    /// Escape alias would have taken two presses to leave a path with points
    /// selected, and would have had Enter closing the colour picker.
    ///
    /// A path and a picture today. A group would be the obvious third case (Enter
    /// to step inside, which `entered_group` already models), and text the fourth;
    /// neither is built, and adding one is an arm here rather than a new key.
    ///
    /// ⚠️ **This paragraph spent months on `enter_container`** (§15 D697,
    /// `[S16.2-L3-05]`) — the twin of the theft on `escape` above, one rung up
    /// the same ladder; `CLAUDE.md` keeps the running count of the class.
    /// Merged, that run was 22 lines,
    /// far under the length ranking's floor; the tell is that its first line,
    /// *"Enter: step into the selection, or step back out"*, describes a key
    /// binding, and the item beneath it was a function that steps **in** and has
    /// no idea a key exists.
    fn enter_action(&mut self) {
        if self.tool == Tool::Node {
            self.choose_tool(Tool::Select); // clears the points itself
            return;
        }
        // **The same toggle, one arm further along.** Image editing is symmetric
        // with the node tool in both directions: Enter steps out of it, and Enter
        // on a selected picture steps in — which is what makes `Enter` a toggle
        // rather than an alias for Escape, and which since §15 D268 is the only
        // keyboard door there is. The crop that was made stays made; leaving is
        // about the mode, and nothing here touches the fill.
        if self.tool == Tool::ImageEdit {
            self.choose_tool(Tool::Select);
            return;
        }
        let [id] = *self.session.selection.ids() else {
            return;
        };
        // **The arms below are `double_click_pick`'s order, and that is the point**
        // (§15 D228): text, then the picture, then the points, then one group deeper.
        // "One level in" means something different on every kind, and the two gestures
        // that mean it — a double-click and `Enter` — must not disagree about which
        // meaning applies to the same layer. Text first, so a text node with a
        // photograph in it is still type.
        if matches!(
            self.session.doc.get(id).map(|n| n.kind()),
            Some(ondin_core::NodeKind::Text { .. })
        ) {
            // No pointer, so the whole string is selected rather than a caret being
            // placed somewhere arbitrary — see `begin_edit_text`.
            self.begin_edit_text(Some(id), None);
            return;
        }
        // `croppable_fill`, not a copy of its rule — the tool acts on whatever it
        // answers, so an entry arm asking differently could step into a crop with
        // nothing to crop.
        if self
            .session
            .doc
            .get(id)
            .is_some_and(|n| crate::tools::croppable_fill(n.paint()).is_some())
        {
            self.begin_image_edit(id);
            return;
        }
        if matches!(
            self.session.doc.get(id).map(|n| n.kind()),
            Some(ondin_core::NodeKind::Path { .. })
        ) {
            // **A lock refuses here rather than in the tool** (§15 D321), which is
            // the one place in this change where the refusal is *not* in a shared
            // funnel — `Enter` is the only door left, `query::hit_test` having
            // already stopped the double-click. Nothing downstream re-checks: there
            // is no lock read anywhere in `tools/`, so a locked path's anchors are
            // draggable the moment the tool is armed. That makes arming it the thing
            // to refuse.
            if ondin_core::is_effectively_locked(&self.session.doc, id) {
                self.session
                    .info("This layer is locked — unlock it to edit its points");
                return;
            }
            self.choose_tool(Tool::Node);
            self.session.info(
                "Editing points: drag an anchor or a segment, Ctrl+drag to shape the curve, \
                 double-click a segment to add a point",
            );
            return;
        }
        // **Last, and the way out is `Escape` rather than `Enter` again.** The two
        // tool arms at the top of this function are toggles because a tool is a mode
        // you are either in or out of; a group is a *depth*, and `Enter` on a group
        // inside a group has an obvious meaning — go deeper. `Escape`'s ladder is what
        // comes back out, one level per press, and it already was (§15 D125).
        if matches!(
            self.session.doc.get(id).map(|n| n.kind()),
            Some(ondin_core::NodeKind::Group | ondin_core::NodeKind::Boolean { .. })
        ) {
            self.enter_container(id, None);
        }
    }

    /// **Asking for the pen from inside an edit arms the bias instead of switching
    /// tool** — the whole of the "the pen is unreachable from inside an edit"
    /// complaint (§15 D125).
    ///
    /// Here rather than in [`Self::dispatch`], where the node tool's other
    /// keyboard interceptions live, because `P` is not the only way to ask: the
    /// rail's pen button issues the same request, and a rail button that dropped
    /// the edit while the key kept it would be two answers to one question. The
    /// rail lights the pen beside the node tool to say the request was heard
    /// ([`Self::tool_rail`]).
    ///
    /// So the standalone pen is Escape-then-`P` from inside an edit, which is the
    /// app's own idiom for leaving a state before entering another, and what
    /// Escape's new first rung is for.
    pub(crate) fn choose_tool(&mut self, tool: Tool) {
        self.finish_text_first();
        // 🚨 **A tool switch abandons the drag in flight** (§15 D724,
        // `[S12.4-L1-07]`). This cleared five pieces of gesture state by name —
        // the text session, the pen, its bias, the points, the pending images —
        // and never touched `self.drag`. So pressing `V` mid-crop-drag left a live
        // `Drag::CropResize` **and last frame's crop preview installed**, while
        // `edited_image()` went `None` and brought the layer's selection box and
        // handles back: a crop preview of a layer no longer in crop mode, under
        // transform chrome answering a gesture that is not the one in hand.
        // Measured headless — `drag live = true, gesture preview = true` after
        // `choose_tool(Select)`.
        //
        // ⚠️ **`Escape` was already clean, and by luck of ordering rather than by
        // design**: its ladder reaches `cancel_gesture` before its
        // `choose_tool(Tool::Select)` rung, and that gate is `self.drag.is_none()`
        // — kind-agnostic. Every *other* way to change tool skipped it.
        //
        // ⚠️ **`clear_gesture_preview`, not `clear_preview`**, or a live text
        // session's uncommitted content goes with the cancelled gesture: the two
        // share one override set and only one belongs to the drag (§15 D109, and
        // `[A1-L2-04]` is that mistake made once already).
        //
        // ⚠️ **And `alt_clone`, which the release will not clear.** `finish_drag`
        // drops it in exactly two places — its no-pointer arm, and the tail of the
        // `Drag::Move` arm — and with `self.drag` already `None` and a pointer
        // position in hand the match falls through `Drag::None => {}` and neither
        // runs, so a stale copy stays armed: *"what the next Alt-drag would
        // silently insert"*, in `cancel_gesture`'s own words.
        //
        // 🚨 **Not routed through `cancel_gesture`, though the finding's sketch
        // prefers it.** That takes a `&egui::Context` this function has not got,
        // across seventeen call sites, and its gate answers for inspector field
        // scrubs and a Settings nudge — states a tool switch cannot be in the
        // middle of, since the keyboard belongs to the field. What is shared is
        // the *list*, and the three lines below are the part of it a canvas drag
        // owns.
        self.drag = Drag::None;
        self.alt_clone = None;
        self.session.clear_gesture_preview();
        if tool == Tool::Pen && self.edited_path().is_some() {
            if !self.pen_bias {
                self.pen_bias = true;
                self.session.info(
                    "Pen armed inside the edit: click an open end to carry on, \
                     a segment to add a point, empty canvas for a new subpath",
                );
            }
            return;
        }
        if tool != Tool::Pen {
            self.pen = None;
        }
        // The bias belongs to one edit, so it cannot outlive the tool that holds
        // it — including when Escape's last rung is what switched away.
        self.pen_bias = false;
        if tool != Tool::Node {
            // Points are the node tool's subject and mean nothing without it.
            // Left behind, they would light up again on the next `A` against a
            // path that may have been reshaped in between.
            self.points.clear();
        }
        if tool != Tool::Image {
            // **The loaded cursor belongs to the tool that holds it**, the same
            // rule as the pen bias two lines up, and this is the one line that
            // makes Escape, `V`, and any other rail button all put the images
            // down. Without it a cancelled placement would come back the next time
            // the tool was armed and place a file the user had moved on from.
            self.pending_images.clear();
        }
        // **Arming a tool that makes a new layer drops the selection** (§15 D294).
        // You are about to draw something, so there is nothing to be selected — and
        // the chrome of whatever was picked before is in the way of the thing being
        // drawn: its outline, its handles and, until D292 gave the drag its own,
        // its size pill. This is Figma's behaviour and it is what the whole class
        // of "the box I am dragging is behind another box's handles" comes down to.
        //
        // **`drags_a_box`, which is the existing name for exactly this set** —
        // Frame, Rect, Ellipse, Polygon, Star, Line, Text and Image, the tools whose
        // whole verb is *create*. A second list would be one more thing to keep in
        // step, and there is no member of that set the reason does not cover.
        //
        // Three deliberate non-members:
        //
        // - **Select and Scale** operate on the selection; clearing it is the one
        //   thing they must never do.
        // - **Node and ImageEdit** are *entered from* a selected layer — they are
        //   what a double-click switches to, and `edited_path` reads the selection
        //   to know what it is editing. Clearing here would empty the tool as it
        //   opened.
        // - **The Pen**, which looks like it belongs and does not. Reached from
        //   inside a path edit it carries that edit on (the `pen_bias` arm above,
        //   which returns before this line, so the clear could not fire there
        //   anyway) — but the exclusion is deliberate rather than incidental,
        //   because a pen armed on empty canvas has no chrome problem to solve: it
        //   places anchors one click at a time and never drags a box over anything.
        // - **Hand** pans, and losing a selection to a pan is the same mistake as
        //   D293's, one gesture over.
        //
        // ⚠️ It fires on the tool *switch*, so `place_image_action` — `Ctrl+Shift+K`
        // and the rail's image button, which arm `Tool::Image` from an action rather
        // than from a tool pick — clears the selection too. That is the right answer
        // for the same reason, and it is stated because it is the one member of the
        // set a reader will not have pictured.
        if tool.drags_a_box() {
            self.session.selection.clear();
        }
        self.tool = tool;
    }

    pub(crate) fn select_all(&mut self) {
        // Everything inside artboards, plus the loose layers that belong to no
        // frame — selecting the artboards themselves is rarely what Ctrl+A
        // means, and it would make a following nudge move the whole canvas.
        //
        // **Layers, whatever is selected — guides included.** With guides picked
        // this replaces them rather than selecting every guide, and that is the
        // decision rather than an oversight: guides multi-select by Shift+click
        // only. A chord that means "select all layers" in every other state
        // acquiring a second meaning when a guide happens to be selected is the
        // confusing reading, so there is nothing to disambiguate. `Selection::set`
        // clears the guide half, which is where that falls out.
        //
        // **Locked layers are not among them** (§15 D746) — the predicate is
        // `may_select_in_bulk`, the same one `marquee_candidates` retains on, so
        // this door and the marquee cannot drift apart again. It is applied here
        // rather than by taking the filtered list because the fallback arm below
        // selects *frames*, which never pass through that list at all.
        //
        // 🚨 **The fallback is asked of the *unfiltered* range, and that is the
        // whole of this arm.** Filtering first and then testing `is_empty()`
        // conflates two different documents: frames with nothing in them, which
        // is what the fallback exists for, and frames whose contents are **all
        // locked**, where the honest answer is to select nothing. Getting that
        // wrong is not a nicety — `Ctrl+A` would select the *frames*, `Delete`
        // would take them, and the locked layers would go down with their
        // parents. Measured: with the filter in and this test reading the
        // *filtered* list, `select_all_tests`' delete case was **still red**, for
        // that reason. The repair at one door leaked straight through the next
        // one.
        //
        // The fallback's own list keeps the lock filter too, so a document whose
        // only frame is locked selects nothing rather than arming `Delete` on the
        // frame — `root_artboards` is a *kind* filter and knows nothing about the
        // lock.
        let range = self.marquee_range();
        let ids = if range.is_empty() {
            self.root_artboards()
        } else {
            range
        };
        self.session.selection.set(
            ids.into_iter()
                .filter(|id| self.may_select_in_bulk(*id))
                .collect(),
        );
    }

    /// The layers a marquee or Ctrl+A can pick: the direct children of every frame
    /// on the canvas, and the root's own children that are not frames.
    ///
    /// The second half exists because a shape dragged out of a frame lives at
    /// the root now (§5.3, relaxed). Without it, the one gesture that puts a
    /// layer there would also make it unreachable by either command.
    ///
    /// One level in, not all of them ([`Self::root_artboards`]): a nested frame is a
    /// candidate because it is a child of the frame it sits in, and what is inside it
    /// is not, for the same reason the contents of a root-level frame's own children
    /// are not.
    ///
    /// **A locked layer is not a candidate for either door** (§15 D746). The lock is
    /// enforced at *selection* time and nowhere else — `query::hit_test`,
    /// [`OndinApp::apply_marquee`] and `guide_at` are the whole enforcement, and no
    /// keyboard verb re-checks it — so a door that hands a locked id to
    /// `Selection::set` has armed `Delete`, `nudge`, `restack`, `align` and the rest
    /// against a layer the user locked. `apply_marquee` filtered it here and
    /// `select_all` did not, which is exactly how `Ctrl+A` then `Delete` came to
    /// destroy a layer a rubber band would not even pick up.
    ///
    /// ⚠️ **The filter belongs to this helper rather than to its callers**, because
    /// the callers are what disagreed. Two doors reading one list and applying two
    /// predicates is the defect; the doc comment above already promised they were
    /// one list, and now the promise covers what is in it.
    ///
    /// ⚠️ **The layers panel is the exception and it is deliberate** — it does not
    /// come through here. A locked layer can still be selected by clicking its row,
    /// and once selected the inspector's rules apply unchanged (§15 D71, D323: it can
    /// be recoloured; what the lock stops is dragging it about). What D746 settles is
    /// narrower than those entries and does not contradict them: **the canvas never
    /// selects a locked layer, by any gesture**.
    pub(crate) fn marquee_candidates(&self) -> Vec<NodeId> {
        let mut ids = self.marquee_range();
        ids.retain(|id| self.may_select_in_bulk(*id));
        ids
    }

    /// **Whether a door that selects a *set* may put `id` in it** (§15 D746, D748).
    ///
    /// 🚨 **"In bulk" is the rule and "on the canvas" was only its first three
    /// doors.** What every door here has in common is that the user described a
    /// set — everything, everything under the band, everything using this colour —
    /// rather than pointing at a layer. Clicking a row in the layers panel points
    /// at one, which is why that door reads no lock and is not on this list. The
    /// maintainer's ruling is *"a locked layer shouldn't be selected in any way
    /// except clicking on the layers panel"*, and **the surface is not what
    /// separates them** — D748's fourth door is a panel.
    ///
    /// Four doors, one statement: [`Self::marquee_candidates`] (the band),
    /// [`Self::select_all`] — including its empty-range fallback, which selects
    /// *frames* and so cannot reach the rule through the candidate list — and
    /// `inspector::select_by_color`, the Group Colors reticle. Writing the
    /// predicate at each door is what let `select_all` and `apply_marquee` disagree
    /// for as long as they did (`[S16.2-L2-02]`), and a rule with one statement and
    /// several implementations is the shape this codebase keeps finding broken.
    ///
    /// **Effective, not the node's own flag** — the call `apply_marquee` was fixed
    /// to make in §15 D321, so a layer inside a locked group is refused exactly as
    /// a click on it is.
    ///
    /// ⚠️ **Visibility is deliberately *not* here.** A hidden-but-unlocked layer
    /// is fully editable (`context-menus.md` §5.9), and the marquee's own
    /// `selectable` keeps that half as the node's flag. Adding `visible()` to this
    /// predicate would take hidden layers away from `Ctrl+A` as well, which is a
    /// different decision that nobody has made.
    ///
    /// ⚠️ **The refusal has to happen here, not in front of each verb.** Every
    /// production reader of the lock in this crate refuses a gesture, a mode or a
    /// drop; `delete_selection`, `nudge`, the restacks and the aligns read it
    /// nowhere. A door that hands a locked id to `Selection::set` has already
    /// armed `Delete`.
    pub(crate) fn may_select_in_bulk(&self, id: NodeId) -> bool {
        !ondin_core::is_effectively_locked(&self.session.doc, id)
    }

    /// [`Self::marquee_candidates`] before the lock filter: *which layers this
    /// gesture reaches*, as against *which of them it may have*.
    ///
    /// 🚨 **One caller, and it is not a second door onto the selection.**
    /// `select_all`'s empty-set fallback asks this rather than the filtered list,
    /// because the two answer differently for the document that matters: frames
    /// with nothing in them (the fallback's case — select the frames) and frames
    /// whose contents are *all locked* (select nothing). Asking the filtered list
    /// makes the second look like the first, and `Ctrl+A` then `Delete` then takes
    /// the **frames** — locked children and all. That is the same layer lost by a
    /// different route, and it survived the first cut of §15 D746's fix.
    ///
    /// ⚠️ **Do not reach for this to select anything.** It is deliberately
    /// `fn`-private and returns ids a canvas gesture is not entitled to; the only
    /// question it answers is whether the range was empty.
    fn marquee_range(&self) -> Vec<NodeId> {
        let mut ids = Vec::new();
        for artboard in self.root_artboards() {
            if let Some(node) = self.session.doc.get(artboard) {
                ids.extend(node.children().iter().copied());
            }
        }
        if let Some(root) = self.session.doc.get(self.session.doc.root()) {
            ids.extend(root.children().iter().copied().filter(|c| {
                !matches!(
                    self.session.doc.get(*c).map(|n| n.kind()),
                    Some(NodeKind::Artboard { .. })
                )
            }));
        }
        ids
    }

    fn nudge(&mut self, delta: Vec2) {
        // A guide nudges along the one axis it has. The cross-axis arrow is
        // inert rather than an error: a horizontal guide has no x to move, and
        // beeping about Left when Up and Down both work would be noise.
        //
        // With several selected, one press moves all of them — and only the ones
        // whose axis the arrow speaks to, which is what makes Up on a mixed-axis
        // selection move the horizontals and leave the verticals alone rather
        // than doing nothing at all.
        if !self.session.selection.guides().is_empty() {
            self.nudge_guides(delta);
            return;
        }
        if self.session.selection.is_empty() {
            return;
        }
        let ids = self.session.selection.ids().to_vec();
        let tx = build::move_by_world(&self.session.doc, &self.session.resolved, &ids, delta);
        match tx {
            Ok(tx) => {
                // A burst of taps is one nudge, so it is one undo step
                // (`EditorSession::commit_run`) — the same argument the point
                // version makes, under its own verb so the two never merge into
                // each other.
                if self.session.commit_run(tx, "nudge-layers") {
                    // A held arrow key walks the layer off the screen otherwise.
                    self.reveal_selection();
                }
            }
            Err(e) => self.session.fail(format!("Nudge failed: {e}")),
        }
    }

    /// Grow or shrink the selection's box by `delta` points per axis —
    /// `Ctrl`+arrows. Positive is wider and taller.
    ///
    /// **Held by its top-left corner**, which is the one X and Y report and the one
    /// the reader is holding still in their head. It is also what makes a held key
    /// usable: the shape grows away from a fixed point instead of walking, so
    /// unlike [`Self::nudge`] this needs no `reveal_selection` to keep it on screen —
    /// and it is why the arrow pressed is the edge that moves, which lets one table
    /// in the keymap serve both actions.
    ///
    /// ⚠️ **Neither half of this arithmetic is written here, and that is the whole
    /// design.** A width already has exactly one definition per shape — the
    /// bottom-right handle's — and both of the inspector's W fields are spelled as
    /// that drag with the pointer put where the number asks. So this asks for the
    /// same box the panel would: one layer goes through [`crate::tools::resize_box_to`],
    /// which is the local box and so grows a rotated shape along *its own* axes;
    /// several go through [`crate::tools::resize_selection`] over the world union, which is
    /// the only box differently-turned layers agree on. That split is the panel's,
    /// not a new one — a single selection shows the single-layer card. Typing a
    /// number and pressing the key must not come to mean two different things about
    /// one shape, and the only way to guarantee that is to have one of them not
    /// implement it.
    ///
    /// A floor of 1pt per axis, as the panel's W and H fields have, and it is there to
    /// stop the ask going **negative** rather than to stop it reaching zero. Traced
    /// without it: the press that asks for a negative width reflects the layer across
    /// its own left edge, so it reappears the same width on the *wrong side* of where
    /// it was anchored and walks from there — a failure the width alone never shows,
    /// since the number stays positive throughout
    /// (`size_step_tests::narrowing_stops_at_a_point_rather_than_passing_through_zero`).
    ///
    /// An axis with **no** extent — the width of a vertical line — comes out
    /// untouched, still zero rather than floored up to a point it never had. Measured
    /// rather than reasoned from any one helper, and asserted, because that is a claim
    /// about what several layers of resize do to a degenerate box and not about a
    /// guard this code can point at.
    ///
    /// **A locked selection scales both axes**, the arrow naming which one drives —
    /// on either arm, and over a set as readily as over one layer. The reading is
    /// [`crate::tools::proportions_locked`]'s: locked when *every* member is (§15 D50),
    /// which is what a corner drag has always asked and what the inspector's W and H
    /// fields now ask too (§15 D347).
    ///
    /// ⚠️ **This said "only for a single layer, which is where the lock lives", and
    /// that was wrong in the way that is hardest to notice**: the premise — a set can
    /// hold both kinds, so there is no one answer to give — had been answered by D50
    /// long before this door existed, and the handles had been acting on it. It
    /// read as a scope decision because it named a real property of the model; what it
    /// actually was, was a rule nobody had thought to look up.
    ///
    /// Guides are inert. A guide is a coordinate, not a box, and the arrow that
    /// would resize one has nothing to act on — the same reading `nudge` gives the
    /// cross-axis arrow, and for the same reason: beeping about it would be noise.
    fn size_step(&mut self, delta: Vec2) {
        if self.session.selection.is_empty() {
            return;
        }
        let Some((roots, union)) = self.selection_union() else {
            return;
        };
        // One expression for both axes and both arms, so the floor cannot end up on
        // one of the four and not the others.
        let want = |extent: f64, d: f64| (extent + d).max(1.0);
        // **A locked layer keeps its proportions**, so one arrow drives both axes: the
        // one it names takes the step and the other follows the aspect. It is the
        // reading the rest of the app already has — a corner drag on a locked layer
        // scales proportionally, and so do the W and H fields.
        //
        // ⚠️ **Read across the whole selection, not the `outermost` reduction below.**
        // The question D50 asks is what the *user* picked, so a group whose child is
        // also selected is a set of two — which is what `canvas::keep_ratio` asks, and
        // asking a shorter list here would let a handle drag and an arrow key disagree
        // about a nested selection and nothing else.
        let locked =
            crate::tools::proportions_locked(&self.session.doc, self.session.selection.ids());
        // Which arrow was pressed. The keymap never sets both, and `x` winning a tie is
        // arbitrary rather than meaningful.
        let drove_x = delta.x != 0.0;
        let paired = |now: ondin_core::kurbo::Size, asked: ondin_core::kurbo::Size| {
            if locked {
                crate::tools::paired_size(now, asked, drove_x)
            } else {
                asked
            }
        };
        let tx = match roots.as_slice() {
            [id] => {
                let Some(local) =
                    ondin_core::local_box(&self.session.doc, &self.session.resolved, *id)
                else {
                    return;
                };
                let now = ondin_core::kurbo::Size::new(local.width(), local.height());
                let asked = ondin_core::kurbo::Size::new(
                    want(local.width(), delta.x),
                    want(local.height(), delta.y),
                );
                crate::tools::resize_box_to(
                    &self.session.doc,
                    &self.session.resolved,
                    *id,
                    paired(now, asked),
                )
            }
            _ => {
                let now = ondin_core::kurbo::Size::new(union.width(), union.height());
                let asked = ondin_core::kurbo::Size::new(
                    want(union.width(), delta.x),
                    want(union.height(), delta.y),
                );
                let size = paired(now, asked);
                crate::tools::resize_selection(
                    &self.session.doc,
                    &self.session.resolved,
                    &roots,
                    // The **upright** union, because that is the box this step is
                    // measured in: `union.width()` is what the nudge is a step of.
                    // The canvas's handle drag reads the oriented frame instead
                    // (§15 D326); a keyboard step is not a corner drag and has no
                    // frame to be on.
                    crate::tools::SelectionBox::upright(union),
                    crate::preview::Handle::BottomRight,
                    // ⚠️ **Both coordinates from the union's `min`, never `max_y` for a
                    // width step.** Taking the far edge as "the height, unchanged" reads
                    // as equivalent and stops being so the moment the height is paired.
                    Point::new(union.min_x() + size.width, union.min_y() + size.height),
                    // Never `keep_ratio`: that flag takes the *larger* of the two
                    // factors and this only ever drives one of them, so on a shrink it
                    // compares the step against the untouched axis's 1.0 and takes the
                    // 1.0 — measured, and it refuses to shrink at all.
                    crate::tools::Resize::geometry(false, false),
                )
            }
        };
        if tx.0.is_empty() {
            return;
        }
        // A burst of taps is one resize, so it is one undo step — the same argument
        // the nudge makes, under its **own** verb so that a nudge and a resize never
        // coalesce into each other. Holding `Ctrl`+`→` and then tapping an arrow
        // must leave two things on the stack, not one.
        self.session.commit_run(tx, "resize-layers");
    }

    /// Restack the selection. Reported when nothing moved, because a layer
    /// already at the front looks identical to a shortcut that failed to fire.
    pub(crate) fn restack(&mut self, mv: build::ZMove) {
        if self.session.selection.is_empty() {
            return;
        }
        let ids = self.session.selection.ids().to_vec();
        let tx = build::z_order(&self.session.doc, &ids, mv);
        if tx.0.is_empty() {
            self.session.info(match mv {
                build::ZMove::Forward | build::ZMove::Front => "Already at the front",
                build::ZMove::Backward | build::ZMove::Back => "Already at the back",
            });
            return;
        }
        if self.session.commit(tx) {
            self.session.info(match mv {
                build::ZMove::Forward => "Brought forward",
                build::ZMove::Backward => "Sent backward",
                build::ZMove::Front => "Brought to front",
                build::ZMove::Back => "Sent to back",
            });
        }
    }

    /// *Group selection* (`Ctrl+G`), and the identity row's group button.
    ///
    /// **`pub(crate)` because the inspector's button is a second door onto this
    /// verb and used to be a second *implementation* of it** (§15 D562). The two
    /// copies had already diverged — see [`Self::ungroup_selection`], where the
    /// divergence was user-visible — and the group pair differed only in the
    /// committer, which is exactly the state a pair is in the day before it
    /// matters.
    pub(crate) fn group_selection(&mut self) {
        let ids = self.session.selection.ids().to_vec();
        if ids.len() < 2 {
            self.session.info("Select two or more layers to group");
            return;
        }
        match build::group(&self.session.doc, &mut self.session.ids, &ids) {
            Ok((tx, group_id)) => {
                if self.session.commit(tx) {
                    self.finish_grouping(group_id);
                }
            }
            Err(e) => self.session.fail(format!("Cannot group: {e}")),
        }
    }

    /// *Frame selection*: wrap the selection in a new frame sized to its union
    /// (§15 D249).
    ///
    /// **One layer is enough, where grouping wants two.** A group of one is a
    /// container that does nothing, which is why [`Self::group_selection`] refuses
    /// it; a frame of one is a perfectly ordinary thing to want — it is how a single
    /// piece of artwork gets a page to sit on, and it is the gesture Figma's users
    /// reach for constantly.
    ///
    /// Otherwise it is `group_selection`'s shape exactly, including the failure
    /// being *reported* rather than pre-empted: the row dims for the one refusal a
    /// user can act on (`build::can_frame`), and anything else — a selection
    /// straddling two parents, artwork with no measurable extent — comes back
    /// through the status line, which is what `Cannot group:` already does.
    ///
    /// It leaves the frame **open** in the layers tree where grouping leaves the
    /// group shut, and that is the difference in intent: grouping is tidying, so
    /// collapsing the rows is the point, while framing is *making a page* and the
    /// artwork on it is what you are about to work on.
    pub(crate) fn frame_selection(&mut self) {
        let ids = self.session.selection.ids().to_vec();
        if ids.is_empty() {
            self.session.info("Select a layer to frame");
            return;
        }
        match build::frame(
            &self.session.doc,
            &self.session.resolved,
            &mut self.session.ids,
            &ids,
        ) {
            Ok((tx, frame_id)) => {
                if self.session.commit(tx) {
                    self.session.selection.set_one(frame_id);
                    self.session.info("Framed");
                }
            }
            Err(e) => self.session.fail(format!("Cannot frame: {e}")),
        }
    }

    /// Select a freshly made group, and leave it **shut** in the layers tree.
    ///
    /// Grouping is an act of tidying: the whole point is to turn several rows
    /// into one. A tree that expands the new group puts all of them straight
    /// back, which is the opposite of what was asked for.
    pub(crate) fn finish_grouping(&mut self, group: NodeId) {
        self.collapsed.insert(group);
        self.session.selection.set_one(group);
        self.session.info("Grouped");
    }

    /// Dissolve every group **or boolean** in the selection (`Ctrl+Shift+G`).
    ///
    /// A boolean releases through the same command and the same builder: its operands
    /// were kept intact inside it, so taking one apart is the identical splice, and
    /// `build::ungroup` says so. Two commands for one act would have been the odd
    /// choice — Figma spells release as ungroup too — and it is what "non-destructive"
    /// was for all along.
    ///
    /// **One press is one transaction, however many containers it dissolves**
    /// (§15 D521). This used to commit inside the loop, one call per container,
    /// which is the only per-subject commit loop the file had: two groups
    /// dissolved on one `Ctrl+Shift+G` needed **two** `Ctrl+Z` to come back, and
    /// a container that refused on the second attempt left the first already
    /// dissolved under a status line saying the command had failed.
    /// [`build::ungroup_all`] is where the ordering lives.
    ///
    /// ⚠️ **`pub(crate)` because the inspector's identity row is the other door
    /// onto this verb, and it used to have its own six-line copy** (§15 D562).
    /// The copies had diverged in the *aftermath*, which no gate looks at: the
    /// panel's dissolved the group, left the selection **empty** — dropping the
    /// user into the empty-canvas card — never said *"Ungrouped"*, and discarded
    /// `build::ungroup`'s `Err` entirely, so a refusal was silent. It also
    /// committed inside a loop, which is the per-container undo step D521 took
    /// out of *this* function. One implementation, so there is nothing left to
    /// diverge.
    pub(crate) fn ungroup_selection(&mut self) {
        let groups: Vec<NodeId> = self
            .session
            .selection
            .ids()
            .iter()
            .copied()
            .filter(|id| {
                matches!(
                    self.session.doc.get(*id).map(|n| n.kind()),
                    Some(NodeKind::Group | NodeKind::Boolean { .. })
                )
            })
            .collect();
        if groups.is_empty() {
            self.session.info("Select a group or a boolean to ungroup");
            return;
        }
        // Read before the commit, because after it the containers are gone.
        // Every child of every container is a candidate for the new selection;
        // the ones that were containers themselves and have also been dissolved
        // are filtered out below, against the document the commit produced.
        let children: Vec<NodeId> = groups
            .iter()
            .filter_map(|g| self.session.doc.get(*g))
            .flat_map(|n| n.children().to_vec())
            .collect();
        let tx = match build::ungroup_all(&self.session.doc, &groups) {
            Ok(tx) => tx,
            Err(e) => {
                self.session.fail(format!("Cannot ungroup: {e}"));
                return;
            }
        };
        if !self.session.commit(tx) {
            return;
        }
        let freed: Vec<NodeId> = children
            .into_iter()
            .filter(|id| self.session.doc.contains(*id))
            .collect();
        if !freed.is_empty() {
            self.session.selection.set(freed);
            self.session.info("Ungrouped");
        }
    }

    fn zoom_to(&mut self, bounds: Option<Rect>) {
        let Some(bounds) = bounds else {
            return;
        };
        if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
            return;
        }
        let (pw, ph) = (self.canvas_px.0 as f64, self.canvas_px.1 as f64);
        // A margin so the fitted content is not flush against the edges.
        let zoom = (pw / bounds.width()).min(ph / bounds.height()) * 0.9;
        // ⚠️ **Through `set_zoom`, not the field** (§15 D689). This is the one
        // writer with a *computed* zoom, so it is the one that can leave the
        // range — and it used to spell that range as the two literals `0.02` and
        // `256.0`, because `view.rs`'s constants were private. A fit on a very
        // large or very small page is exactly the path that reaches a bound.
        self.session.camera.set_zoom(zoom);
        self.session.camera.center = bounds.center();
    }

    /// Scroll the selection back into view, by the least that will do it.
    ///
    /// **For the edits that move a layer without the pointer**: keyboard nudges, and
    /// the inspector's X and Y. A gesture at the canvas edge has `canvas::autopan`
    /// to keep up with it, but a held arrow key or a scrubbed X field will walk a
    /// layer straight off the screen and leave the user editing something they
    /// cannot see.
    ///
    /// **Minimum pan, and only when it is actually off screen.** Centring on every
    /// nudge would make a 1px arrow-key tap jump the whole view; a layer already
    /// comfortably in frame should not move the world at all. So this is the
    /// scroll-into-view rule a list uses, in two dimensions: nothing while it fits,
    /// and then just enough to bring the offending edge inside a margin.
    ///
    /// Size-only edits deliberately do not call this. Width, height, rotation and
    /// skew change what a layer *is* rather than where it is; a box growing past the
    /// edge of the viewport is a thing the user can see the near side of, and
    /// scrolling on every drag of a W field would fight the hand.
    ///
    /// **Reads [`Self::selection_bounds`], and always meant to.** It shared
    /// `content_bounds` with zoom-to-fit until that pair was split, so with an
    /// empty selection it fell through to the whole document — a scroll-into-view
    /// for a thing that is not selected. Inert in practice, because every caller
    /// has just edited a selection, but it is not what the name says.
    pub(crate) fn reveal_selection(&mut self) {
        let Some(bounds) = self.selection_bounds() else {
            return;
        };
        let view = self.session.camera.viewport(self.canvas_px).view;
        // A margin in world units, so the layer lands *inside* the frame rather
        // than flush against the edge where half its selection box is cut off.
        let margin = (REVEAL_MARGIN_PX / self.session.camera.zoom)
            .min(view.width() / 4.0)
            .min(view.height() / 4.0);
        let view = view.inset(-margin);

        // Per axis and independently: a layer that has run off the right stays put
        // vertically.
        let dx = reveal_axis(bounds.min_x(), bounds.max_x(), view.min_x(), view.max_x());
        let dy = reveal_axis(bounds.min_y(), bounds.max_y(), view.min_y(), view.max_y());
        if dx != 0.0 || dy != 0.0 {
            self.session.camera.center += Vec2::new(dx, dy);
        }
    }

    /// Everything in the document — what `Action::ZoomFit` fits.
    ///
    /// **It used to be this *or* the selection**, whichever there was, under one
    /// `content_bounds`. That made "fit the whole document" unreachable for as
    /// long as anything was selected, with no key to get it back: the only way
    /// out was to deselect first. Splitting the two is what `docs/shortcuts.md` §3's
    /// second binding is *for* — the binding is the visible half of the fix, not
    /// the whole of it.
    fn document_bounds(&self) -> Option<Rect> {
        self.session.preview_world_bounds(self.session.doc.root())
    }

    /// The selection's union — what `Action::ZoomSelection` fits.
    ///
    /// `None` with an empty selection, which leaves the chord a no-op rather
    /// than quietly falling back to [`Self::document_bounds`]; falling back is
    /// exactly the behaviour the split removed.
    fn selection_bounds(&self) -> Option<Rect> {
        self.session
            .selection
            .ids()
            .iter()
            .filter_map(|id| self.session.preview_world_bounds(*id))
            .reduce(|a, b| a.union(b))
    }

    /// Fill [`FrameIndex`] if this revision has not been walked yet, then hand the
    /// caller what it wants out of it.
    ///
    /// **A closure rather than a returned `Ref`**, so no caller can hold the
    /// borrow across something that might want the memo again — which is exactly
    /// what a `RefCell` panic is made of, and what a nested `frame_covering`
    /// inside an `artboards()` loop would do.
    pub(crate) fn with_frames<R>(&self, f: impl FnOnce(&FrameIndex) -> R) -> R {
        let mut index = self.frame_index.borrow_mut();
        if index.at != Some(self.session.revision()) {
            // Depth-first in child order, which is paint order — so a nested frame
            // comes after the frame holding it. Everything that reads this list
            // relies on that: "the topmost frame containing the point" and "the
            // frame that owns these bounds" both take the *last* match, and the
            // last match of two nested frames is the inner one, which is the one
            // the pointer is really in.
            index.frames = ondin_core::subtree_nodes(&self.session.doc, &[self.session.doc.root()])
                .into_iter()
                .filter(|id| {
                    matches!(
                        self.session.doc.get(*id).map(|n| n.kind()),
                        Some(NodeKind::Artboard { .. })
                    )
                })
                .collect();
            index.boxes = index
                .frames
                .iter()
                .filter_map(|id| Some((*id, self.session.resolved.world_bounds(*id)?)))
                .collect();
            index.at = Some(self.session.revision());
            index.walks += 1;
        }
        f(&index)
    }

    /// How many document walks `Self::artboards` has actually paid for.
    ///
    /// Test-facing; see `FrameIndex::walks` — plain backticks because this item is
    /// `#[cfg(test)]` and `cargo doc` cannot see it (§15 D319). ⚠️ **`cfg(test)` on the accessor
    /// and not on the counter**: the increment is one add on the cold path and
    /// gating it would make the production build and the tested build differ in
    /// the one place this memo could go wrong. `clippy --all-targets` reports an
    /// ungated accessor as dead on the `bin` target, which is the gate hole §15
    /// D302 exists for working correctly.
    #[cfg(test)]
    pub(crate) fn frame_walks(&self) -> u64 {
        self.frame_index.borrow().walks
    }

    /// Every artboard, in paint order — see [`FrameIndex`] for why this is a
    /// memo lookup and a four-element clone rather than a walk of the document.
    pub(crate) fn artboards(&self) -> Vec<NodeId> {
        self.with_frames(|i| i.frames.clone())
    }

    /// The frames sitting straight on the canvas, in z-order.
    ///
    /// For the commands that mean "one level in from the canvas" rather than "every
    /// frame there is" — a marquee, Ctrl+A. Descending into nested frames would offer
    /// a frame *and* its contents as candidates for the same gesture, and a marquee
    /// that selects a card and the text inside it has selected the text twice.
    pub(crate) fn root_artboards(&self) -> Vec<NodeId> {
        self.session
            .doc
            .get(self.session.doc.root())
            .map(|root| {
                root.children()
                    .iter()
                    .copied()
                    .filter(|c| {
                        matches!(
                            self.session.doc.get(*c).map(|n| n.kind()),
                            Some(NodeKind::Artboard { .. })
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

// --- clipboard and structure ---------------------------------------------

/// How far a pasted copy sits from its original, in its parent's space.
const PASTE_OFFSET: Vec2 = Vec2::new(20.0, 20.0);

/// How far a pasted or duplicated **guide** sits from its original, along the one
/// axis it has.
///
/// [`PASTE_OFFSET`]'s single component, so a guide and a layer step by the same
/// amount and the two gestures do not read as different features. Enough to be
/// plainly two lines rather than a thickened one at any reasonable zoom, and — this
/// is the useful part — because Ctrl+D leaves the *copy* selected, repeating it
/// walks along in even steps and lays out a row of guides without measuring
/// anything.
const GUIDE_PASTE_OFFSET: f64 = PASTE_OFFSET.x;

// A copy that landed on top of its original would be invisible, which is the whole
// reason either offset exists. Checked at compile time rather than in a test: it is
// a property of the constant, so the useful place to fail is the edit that breaks
// it, not a test run afterwards.
const _: () = assert!(GUIDE_PASTE_OFFSET > 0.0);

/// How far *Paste here* moves the payload: enough to put `from`'s centre on the
/// pointer, or [`PASTE_OFFSET`] when there is no recorded box to aim (§15 D251).
///
/// **It takes the box, not the layers it came from**, and that is the fix rather
/// than a tidy-up. Reading the bounds back off the source ids is reachable *through
/// the API shape* — the ids are gone after a cut, so the aim silently became a fixed
/// step and every repeat landed in the same place. A `Rect` cannot consult a
/// document that no longer holds the source, so the failure has nowhere to happen.
/// `build::gaps_along` replaced `build::spacings` for exactly this reason (§15
/// D245); it is the second instance of the same lesson.
///
/// A free function so the aim is stated over a box and a point — the two things it
/// is actually about (§15 D35's habit).
fn paste_aim(from: Option<Rect>, world: Point) -> Vec2 {
    match from {
        Some(b) => world - b.center(),
        None => PASTE_OFFSET,
    }
}

/// The guide a paste or a duplicate of `from` produces: a fresh id, the resolved
/// `owner`, and the original's position stepped along its own axis.
///
/// **One rule, two callers**, so Ctrl+V and Ctrl+D cannot come to place a copy
/// differently. The axis and the colour come across untouched — a copy of a
/// recoloured guide is a recoloured guide — and the *scope* is the caller's to
/// resolve, because paste has to survive the original's frame having been deleted
/// in between and duplicate does not.
///
/// A free function so the placement can be asked of a guide and a step, without a
/// clipboard or a selection to arrange first (§15 D35's habit).
///
/// `step` is the offset along that axis, and it is a parameter rather than the
/// constant so that **paste in place can ask for none**: "in place" has to mean the
/// same thing to a guide as to a layer, or one chord is two features (§15 D248).
/// Every other caller passes [`GUIDE_PASTE_OFFSET`], which is the rule the doc
/// comment above describes.
fn pasted_guide(from: &Guide, id: GuideId, owner: Option<NodeId>, step: f64) -> Guide {
    Guide {
        id,
        owner,
        position: from.position + step,
        ..*from
    }
}

/// Clearance `reveal_selection` leaves between the layer it scrolled to and the
/// edge of the canvas, in device px. Enough for the selection box and its handles,
/// which are the part the user is about to reach for.
const REVEAL_MARGIN_PX: f64 = 24.0;

/// How far one axis must scroll for `b0..b1` to lie inside `v0..v1`: `0.0` when it
/// already does, otherwise the smaller of the two corrections.
///
/// **A layer bigger than the viewport is not a problem to be solved.** If it
/// overhangs both edges it already fills the view — there is nothing to reveal, and
/// pulling one edge in would push the other out, so the next nudge would scroll
/// straight back. The clamp in each branch is what says so: the correction one edge
/// asks for is never allowed to pass the point where the *other* edge starts
/// complaining.
fn reveal_axis(b0: f64, b1: f64, v0: f64, v1: f64) -> f64 {
    if b0 < v0 {
        (b0 - v0).max((b1 - v1).min(0.0))
    } else if b1 > v1 {
        (b1 - v1).min((b0 - v0).max(0.0))
    } else {
        0.0
    }
}

/// Band at the top and bottom of the layers tree inside which a dragged row
/// scrolls it, in points. Narrower than the canvas's, because the tree is a narrow
/// column and a 32pt band at each end of it would leave little dead space between.
const LAYER_SCROLL_BAND: f32 = 22.0;
/// Fastest the layers tree auto-scrolls, in points per second. Slower than the
/// canvas: the target is one 20px row, not a region, so the last stretch has to be
/// steerable rather than quick.
const LAYER_SCROLL_MAX: f32 = 420.0;

/// The run of duplicates Ctrl+D is currently building.
///
/// Only the *most recent* clone is remembered, which is what makes the feature
/// predictable: duplicating anything else — an original, an older copy, a
/// different layer entirely — starts a new run at zero rather than inheriting a
/// step from something the user has moved on from.
pub(crate) struct CloneChain {
    /// The clone the last Ctrl+D produced.
    last: NodeId,
    /// Its world origin at the moment it was created. The difference between
    /// this and where it sits now is the movement to repeat.
    placed_at: Point,
    /// The step being repeated, carried forward so a run of presses keeps its
    /// spacing instead of collapsing once the user stops nudging.
    step: Vec2,
}

impl OndinApp {
    /// `Ctrl+V`'s whole decision, in one place: a picture, then the app's own
    /// payload, then text as a new layer.
    ///
    /// **The picture wins over the layers**, because a clipboard holding both is one
    /// an image was copied to — the text stand-in a *layer* copy leaves behind
    /// (`copy_selection`) never arrives with one.
    ///
    /// **And the layers win over the text, but only while they are still what the OS
    /// clipboard describes.** That is what [`Self::owns_the_clipboard`] decides, and
    /// it is the whole reason the text arm can exist at all: without it, "copy a
    /// layer, then copy a sentence in a browser, then paste" pastes the *layer* — and
    /// pasting a payload whose stand-in text has been overwritten is the same
    /// staleness the picture rule above already reasons about, one payload over.
    ///
    /// Every door makes the same decision, so the arms live here rather than at each of
    /// them.
    ///
    /// ⚠️ **This described an asymmetry that has since been closed, and said so for
    /// long enough to be worth naming rather than deleting.** It read: "the menu's
    /// *Paste here* has no image arm, so a clipboard holding a picture is reachable
    /// from the chord and not from the row … `paste_image` takes no point; it is
    /// small, and it is not this change." It *was* that change, under §15 D224 —
    /// [`Self::paste_image`] takes an `Option<Point>` and [`Self::paste_at`] asks it
    /// first, in this same order — and `paste_at`'s own doc comment has recorded the
    /// fix the whole time. Two comments in one file disagreeing about one fact, with
    /// every gate green, because prose is the one thing `cargo doc` cannot check
    /// (§15 D296).
    fn paste(&mut self) {
        if self.paste_image(None) {
            return;
        }
        match self.take_clipboard() {
            Clipboard::Layers if self.paste_clipboard() => return,
            Clipboard::Unreadable => return,
            _ => {}
        }
        // **After the app's own payload and before the text layer**, which is the
        // whole routing rule: an in-app copy is layers however it looks on the system
        // clipboard, and anything that is not markup is text (§15 D259).
        if self.paste_svg(None) {
            return;
        }
        self.paste_text_as_layer(None);
    }

    /// `Ctrl+Shift+V`: the clipboard back at the **coordinates it was copied from**
    /// (`docs/shortcuts.md` §7, `docs/context-menus.md` §4, §15 D248).
    ///
    /// **The whole of it is a zero offset**, because [`Self::paste_clipboard_at`]
    /// measures its delta from where the copy came from — so `PASTE_OFFSET` is the
    /// only thing standing between the ordinary paste and this one. Everything else
    /// the chord needs was already built for *Paste here*.
    ///
    /// **The same three payloads in the same order as [`Self::paste`]** — a picture,
    /// the app's own subtrees, then text — because two paste chords that ask
    /// different questions of one clipboard are two features wearing one name (§15
    /// D218, D224). A picture is the arm where "in place" has nothing to say: it
    /// replaces a fill or lands in the middle of the view, and neither has a
    /// position to restore, so the shift is ignored rather than inventing one.
    ///
    /// **Guides are taken here rather than left to `paste_clipboard_at`**, which is
    /// the one structural thing the chord adds: that function hands the guide arm
    /// the fixed step, and a guide pasted 20 units along is not in place. Its axis
    /// is the only coordinate it has, so "same position" is exactly a step of zero.
    fn paste_in_place(&mut self) {
        if self.paste_image(None) {
            return;
        }
        match self.take_clipboard() {
            Clipboard::Layers => {
                if self.paste_guides(0.0) {
                    return;
                }
                if self.paste_clipboard_at(Vec2::ZERO) {
                    return;
                }
            }
            Clipboard::Unreadable => return,
            Clipboard::Foreign => {}
        }
        // **Markup has no "in place" to restore**, exactly as a picture does not: it
        // arrives from another application with no position of ours behind it, so
        // this chord and the plain one give it the same answer rather than inventing
        // a coordinate.
        if self.paste_svg(None) {
            return;
        }
        self.paste_text_as_layer(None);
    }

    /// Whether the in-app clipboard still holds what the **system** clipboard
    /// describes.
    ///
    /// Every in-app copy puts a text stand-in on the system clipboard — layer names,
    /// or a guide's axis and number — for the reason D17 gives: `egui_winit` emits
    /// `Event::Paste` only for a non-empty clipboard, so a copy that left it alone
    /// could not be pasted. **The stand-in doubles as a receipt.** If the system
    /// clipboard no longer reads back as the stand-in we wrote, something else has
    /// copied since, and whatever `clipboard`/`guide_clipboard` hold is a stale
    /// answer to `Ctrl+V`.
    ///
    /// The stamp is written at the same moment as the payload
    /// ([`Self::stamp_clipboard`]) so the two cannot come apart, which is what lets
    /// "no stamp" mean "nothing of ours is on it" rather than "unknown".
    ///
    /// ⚠️ **It takes the text rather than reading it** (§15 D857,
    /// `[X1.2-L4-01]`), so a caller that needs the text as well — a paste door
    /// adopting a foreign copy, a context menu snapshotting both — reads the OS
    /// clipboard **once**. Each used to read it here and again beside the call, and
    /// since §15 D823 a read can be megabytes of base64.
    pub(crate) fn owns_the_clipboard(&self, text: Option<&str>) -> bool {
        let Some(stamp) = self.clipboard_stamp else {
            debug_assert!(
                self.clipboard.is_none() && self.guide_clipboard.is_none(),
                "a clipboard payload was set without its stamp"
            );
            return false;
        };
        text.is_some_and(|t| ClipStamp::of(t) == stamp)
    }

    /// What the clipboard is holding for the layer arm — and, when the answer is
    /// a copy made in **another `ondin` window**, adopt it on the way past
    /// (§15 D823).
    ///
    /// **Every paste door calls this in place of [`Self::owns_the_clipboard`]**,
    /// so the crossing is a property of the clipboard rather than of any one
    /// chord: `Ctrl+V`, *Paste in place*, *Paste here* and the layers panel's
    /// *Paste* all gain it together. ⚠️ **That rule is §15 D224's and not
    /// D218's** — D218 *recorded* the opposite as an honest asymmetry, *Paste
    /// here* having no image arm, and D224 is what closed it by taking the point
    /// `paste_image` had not. D224 also names the one place the row and the chord
    /// **deliberately differ**, which is the *aim*: *Paste here* passes `Some(p)`,
    /// so the fill it replaces must be under the pointer. What must not differ is
    /// what the clipboard is found to **hold**, and that is what this makes
    /// uniform.
    ///
    /// **The receipt is asked first, and that is a fast path rather than a
    /// separate rule.** A copy that never left this window is already sitting in
    /// [`Self::clipboard`] with its images beside it, so re-parsing our own
    /// payload out of the OS clipboard would be the same subtrees at the cost of
    /// a megabyte of base64 — while giving, by construction, the same answer.
    ///
    /// **Adopting rather than pasting directly** is what keeps the placement
    /// rules in one place. [`Self::paste_clipboard_at`] puts a subtree back under
    /// the parent it was copied from *if this document has one*, and a foreign
    /// copy simply does not: ids are minted against a per-process-random actor
    /// (`session`), so `Document::contains` answers no and the existing fallback
    /// — the canvas root, appended — is already the right behaviour with nothing
    /// written for it. ⚠️ **The exception is the case that looks like a bug and
    /// is not**: the *same document* open in two windows does resolve, and the
    /// paste lands back in its own group, which is what someone doing that meant.
    ///
    /// ⚠️ **One OS read, not two** (§15 D857, `[X1.2-L4-01]`). This asked
    /// `owns_the_clipboard`, which read the whole clipboard to compare it and threw
    /// the string away, and then read it again to adopt it — two multi-megabyte
    /// reads per `Ctrl+V` of a foreign copy carrying a picture.
    fn take_clipboard(&mut self) -> Clipboard {
        let Some(text) = system_clipboard_text() else {
            return Clipboard::Foreign;
        };
        if self.owns_the_clipboard(Some(&text)) {
            return Clipboard::Layers;
        }
        self.adopt_clip_text(text)
    }

    /// [`Self::take_clipboard`]'s decision, with the OS read lifted off it.
    ///
    /// **Split for the reason §15 D805 split `pick_from_chain` off `pick_preview`**:
    /// the read is the half no test on this machine can reach — `OndinApp::headless`
    /// sets `CLIPBOARD_OFF`, so [`system_clipboard_text`] answers `None` to every
    /// probe and the whole layer arm is unreachable from a test through the door
    /// above — while the decision is the half worth pinning. A test hands this
    /// function the text a real [`Self::copy_selection`] produced, which is the
    /// crossing itself with one process standing in for two.
    fn adopt_clip_text(&mut self, text: String) -> Clipboard {
        let Some(read) = ondin_core::io::clip::read(&text) else {
            return Clipboard::Foreign;
        };
        let payload = match read {
            Ok(payload) => payload,
            Err(e) => {
                // ⚠️ **One sentence was being told about three different
                // failures, and it was only true of one** (§15 D833). *"Came
                // from a build this one cannot read"* is the version refusal's
                // diagnosis; it was also what a damaged payload and a
                // structurally bad one got, so the user was sent looking for a
                // version mismatch that was not there. `[X2-L1-02]`'s whole
                // user-visible symptom was this sentence, shown for a copy this
                // build had made a second earlier.
                self.session.fail(match &e {
                    ondin_core::io::IoError::UnsupportedVersion(v) => {
                        format!("That copy came from a build this one cannot read (schema {v})")
                    }
                    ondin_core::io::IoError::Integrity(what) => {
                        format!("That copy is not one this build can use ({what})")
                    }
                    ondin_core::io::IoError::Serde(_) => {
                        format!("That copy is damaged and could not be read ({e})")
                    }
                });
                return Clipboard::Unreadable;
            }
        };
        self.clipboard_from = payload.from;
        self.clipboard = Some(Clip {
            subtrees: payload.subtrees,
            images: payload.images,
        });
        // The stamp is written here for the same reason `stamp_clipboard` writes
        // one at a copy: it is what makes `owns_the_clipboard` answer *yes* from
        // now on, so a second paste of the same foreign copy takes the fast path
        // above instead of decoding the payload again.
        self.clipboard_stamp = Some(ClipStamp::of(&text));
        // One clipboard, two typed fields — see `Self::guide_clipboard`. A
        // foreign payload carries no guides, so whatever this window was holding
        // is no longer what the system clipboard describes.
        self.guide_clipboard = None;
        Clipboard::Layers
    }

    /// Put `text` on the system clipboard **and** keep it as the receipt
    /// [`Self::owns_the_clipboard`] reads back.
    ///
    /// One function rather than two lines at each copy site, because a copy that set
    /// the payload and forgot the stamp is a copy `Ctrl+V` silently declines.
    fn stamp_clipboard(&mut self, ctx: &egui::Context, text: String) {
        self.clipboard_stamp = Some(ClipStamp::of(&text));
        ctx.copy_text(text);
    }

    /// Copy the selection into the in-app clipboard, and put a text stand-in on
    /// the **system** clipboard.
    ///
    /// The stand-in is not decoration. `egui_winit` only emits `Event::Paste`
    /// when the OS clipboard has something in it, and that event is the only
    /// form Ctrl+V reaches us in (see `input::normal_mode`) — so a copy that
    /// leaves the system clipboard alone is a copy that cannot be pasted.
    ///
    /// 🚨 **The names are no longer the whole of what goes there** (§15 D823).
    /// The captured subtrees follow them past `ondin_core::io::clip::FENCE`, so
    /// a second `ondin` window pastes them as *layers*; this paragraph read *"the
    /// real payload is the captured subtrees, which have no text form today"*
    /// until that landed. The names stay on the **first line** for the reason
    /// above and because they are what someone pasting an Ondin copy into a note
    /// wants to read — the stand-in was extended, not replaced.
    ///
    /// **Returns the text a layer copy put on the system clipboard**, or `None`
    /// for a guide copy or nothing copied. Nothing in the app reads it; the tests
    /// that model a second window do, since the stamp that used to hand them the
    /// text is a digest now (§15 D857) and a headless app has no OS clipboard to
    /// read it back from (§15 D798). The cost is one clone of the text per copy,
    /// dropped by the caller on the spot — transient, where the stamp it replaced
    /// held the same bytes for the session.
    fn copy_selection(&mut self, ctx: &egui::Context) -> Option<String> {
        // Guides are the other kind of subject Ctrl+C can be aimed at, and they
        // are never selected alongside layers, so this is an early return rather
        // than a second half.
        if self.copy_guides(ctx) {
            return None;
        }
        // The outermost members only: copying a group and its child would
        // paste the child twice.
        let ids = build::outermost(&self.session.doc, self.session.selection.ids());
        let names: Vec<String> = ids
            .iter()
            .filter_map(|id| self.session.doc.get(*id))
            .map(|n| n.name().to_string())
            .collect();
        let templates: Vec<Vec<Node>> = ids
            .iter()
            .filter_map(|id| self.session.doc.capture_subtree(*id))
            .collect();
        if templates.is_empty() {
            return None;
        }
        // **Taken here, while the originals still exist.** `cut_selection` copies
        // and *then* deletes, so this is the last moment either gesture can answer
        // "where did this come from" — which is the whole of §15 D251.
        self.clipboard_from = ids
            .iter()
            .filter_map(|id| self.session.preview_world_bounds(*id))
            .reduce(|a, b| a.union(b));
        // **The pictures come too, and they are taken here for `clipboard_from`'s
        // reason** — this is the last moment the document that holds them is
        // certainly the one on screen. A copy is nodes plus the table entries those
        // nodes key into (`Clip`), because a node carries the key and the document
        // carries the bytes.
        let flat: Vec<Node> = templates.iter().flatten().cloned().collect();
        let images: Vec<_> = build::image_ids_in(&flat)
            .into_iter()
            .filter_map(|id| Some((id.clone(), self.session.doc.image(&id)?.clone())))
            .collect();
        // **The stand-in carries the payload now** (§15 D823). It is still the
        // layer names on the first line, for the reason this function's doc gives
        // and for the reason D17 gave before it — but the real subtrees follow it
        // past `io::clip::FENCE`, which is what lets a second `ondin` window paste
        // them as *layers* rather than as the text of their names.
        //
        // ⚠️ **The names alone on failure, rather than no copy at all.** Nothing
        // in a well-formed document can make `write` fail, so this arm is a fact
        // about the function rather than about today's model — and the honest
        // fallback is the behaviour this call had until D823, which leaves the
        // in-app clipboard working and only the crossing lost.
        let heading = names.join(", ");
        let text = ondin_core::io::clip::write(&templates, &images, self.clipboard_from, &heading)
            .unwrap_or(heading);
        self.stamp_clipboard(ctx, text.clone());
        self.session
            .info(format!("Copied {} layer(s)", templates.len()));
        self.clipboard = Some(Clip {
            subtrees: templates,
            images,
        });
        // One clipboard, two typed fields — see `OndinApp::guide_clipboard`.
        self.guide_clipboard = None;
        Some(text)
    }

    /// Copy the selected guides. Returns whether there were any, so
    /// [`Self::copy_selection`] can treat this as its guide half.
    ///
    /// **The system clipboard gets a text stand-in for the same reason a layer
    /// copy does**, and it is not decoration: `egui_winit` only emits
    /// `Event::Paste` when the OS clipboard has something in it, and that event is
    /// the only form Ctrl+V reaches us in (`input::normal_mode`) — so a copy that
    /// left the system clipboard alone would be a copy that could not be pasted.
    /// A guide has a genuinely useful text form, unlike a captured subtree: the
    /// axis and the number, which is what someone pasting into a note would want.
    fn copy_guides(&mut self, ctx: &egui::Context) -> bool {
        let guides: Vec<Guide> = self
            .session
            .selection
            .guides()
            .iter()
            .filter_map(|id| self.session.doc.guide(*id))
            .copied()
            .collect();
        if guides.is_empty() {
            return false;
        }
        let text: Vec<String> = guides
            .iter()
            .map(|g| format!("{} {}", g.axis.position_label(), g.position))
            .collect();
        self.stamp_clipboard(ctx, text.join(", "));
        self.session
            .info(format!("Copied {} guide(s)", guides.len()));
        self.guide_clipboard = Some(guides);
        self.clipboard = None;
        // Cleared with the payload it belongs to, so a stale box can never be
        // measured against a payload it did not come from.
        self.clipboard_from = None;
        true
    }

    /// Paste the copied guides, each one **into the scope its original had** and
    /// offset along its own axis so the copy is visibly a copy.
    ///
    /// Returns whether there was anything to paste, so [`Self::paste_clipboard`]
    /// can treat this as its guide half.
    ///
    /// **The owner is carried across unchanged, not re-derived from where the
    /// pointer is.** A guide's scope is part of what was copied — a guide down the
    /// middle of a card belongs to that card — and a paste that re-scoped it to
    /// whatever happened to be under the cursor would be answering a question
    /// nobody asked. A frame that has since been deleted is the one case that
    /// cannot be honoured; those guides fall back to the canvas rather than being
    /// dropped, so a paste never silently loses one.
    fn paste_guides(&mut self, step: f64) -> bool {
        let Some(guides) = self.guide_clipboard.clone() else {
            return false;
        };
        let mut ops = Vec::new();
        let mut ids = Vec::new();
        let mut orphaned = 0;
        for guide in guides {
            let owner = guide.owner.filter(|o| self.session.doc.contains(*o));
            if owner != guide.owner {
                orphaned += 1;
            }
            let id = GuideId(self.session.ids.mint());
            ids.push(id);
            ops.push(Operation::AddGuide {
                guide: pasted_guide(&guide, id, owner, step),
            });
        }
        // 🚨 **Counted off the paste, not off the selection** (§15 D680,
        // `[S16.2-L1-06]`). This read `self.session.selection.guides().len()`
        // *after* `select_pasted_guides`, whose first line is
        // `if self.lock_guides { return; }` — a deliberate rule and the right one,
        // since locked means unselectable. So with *Lock guides* on, a paste that
        // added three guides and committed reported **"Pasted 0 guide(s)"**, or, if
        // some other guide happened to be selected, that guide's count. The orphan
        // arm contradicted itself in one line: *"Pasted 0 guide(s) — 2 whose frame
        // is gone are now on the canvas"*.
        //
        // `duplicate_guides` — the twin, sharing `pasted_guide` and nothing else —
        // took `ops.len()` before committing and was right all along.
        let n = ids.len();
        if ops.is_empty() || !self.session.commit(Transaction(ops)) {
            return true;
        }
        self.select_pasted_guides(ids);
        self.session.info(match orphaned {
            0 => format!("Pasted {n} guide(s)"),
            orphaned => format!(
                "Pasted {n} guide(s) — {orphaned} whose frame is gone are now on the canvas"
            ),
        });
        true
    }

    /// Ctrl+D on guides: a copy of each selected guide, in its own scope, offset
    /// along its axis. Returns whether there were any.
    ///
    /// **Not routed through the clipboard.** Duplicate is its own verb everywhere
    /// else in the app, and borrowing the clipboard for it would mean Ctrl+D
    /// silently overwriting whatever Ctrl+C had put there — a side effect nobody
    /// asked for. It shares the *placement rule* with paste and nothing else.
    fn duplicate_guides(&mut self) -> bool {
        let guides: Vec<Guide> = self
            .session
            .selection
            .guides()
            .iter()
            .filter_map(|id| self.session.doc.guide(*id))
            .copied()
            .collect();
        if guides.is_empty() {
            return false;
        }
        let mut ops = Vec::new();
        let mut ids = Vec::new();
        for guide in guides {
            let id = GuideId(self.session.ids.mint());
            ids.push(id);
            // The original's own scope, which exists by construction — the guide
            // being duplicated is in the document, so its owner is too.
            ops.push(Operation::AddGuide {
                guide: pasted_guide(&guide, id, guide.owner, GUIDE_PASTE_OFFSET),
            });
        }
        let n = ops.len();
        if self.session.commit(Transaction(ops)) {
            self.select_pasted_guides(ids);
            self.session.info(format!("Duplicated {n} guide(s)"));
        }
        true
    }

    /// Make `ids` the selection, honouring the guide lock.
    ///
    /// Locked means unselectable (`OndinApp::guide_at`), so selecting a fresh copy
    /// while locked would produce a guide that is *selected but unselectable* —
    /// the state the ruler drag's release is careful not to create, and this is the
    /// third place that has to know it.
    fn select_pasted_guides(&mut self, ids: Vec<GuideId>) {
        if self.lock_guides {
            return;
        }
        self.session.selection.clear();
        for id in ids {
            self.session.selection.toggle_guide(id);
        }
    }

    /// Cut is copy then delete, and one undo step: the copy touches no document
    /// state, and `delete_selection` already commits the whole selection at once.
    ///
    /// Nothing selected means nothing to cut — and in particular the clipboard
    /// is left alone rather than half-cleared, so `Ctrl`+`X` on empty canvas
    /// does not quietly throw away what was copied a minute ago.
    /// **Guides cut too**, and the guard has to ask about them or it would not.
    /// It tested the *layer* selection only, so with a guide selected Ctrl+X
    /// returned while Ctrl+C worked — a gap the user can see, since the two keys
    /// sit beside each other and one of them silently doing nothing reads as
    /// broken rather than as unimplemented.
    fn cut_selection(&mut self, ctx: &egui::Context) {
        let layers = !build::outermost(&self.session.doc, self.session.selection.ids()).is_empty();
        let guides = !self.session.selection.guides().is_empty();
        if !layers && !guides {
            return;
        }
        self.copy_selection(ctx);
        self.delete_selection();
        self.session.info("Cut");
    }

    /// Put the selection on the system clipboard as an SVG document
    /// (`context-menus.md` §3's Export group).
    ///
    /// **The committed document, not the preview.** `ondin_export::svg::svg_of` is
    /// a pure function of `Document` + `Resolved`, which is the same pair
    /// `ondin export` renders from — so what is copied is what a save would write,
    /// and a gesture halfway through a drag cannot leak an uncommitted position
    /// into another application.
    ///
    /// **`build::in_document_order`, which is `outermost` plus the ordering the
    /// writer requires.** The first half is `copy_selection`'s reason — a selection
    /// holding a group and something inside it would otherwise emit that child
    /// twice, once inside its group and once beside it. The second half was missing
    /// until 2026-08-20 and is a bug this shipped with: `Selection::ids` is **pick**
    /// order, so shift-clicking the front shape and then the one behind it handed
    /// `svg_of` a reversed stack, and SVG has no z-index to correct it with.
    ///
    /// **No stamp** ([`Self::clipboard_stamp`]). The stamp is the receipt saying
    /// "the in-app payload still describes what the OS clipboard holds", and this
    /// writes real, complete text that stands on its own — so leaving the stamp
    /// alone is what correctly makes `owns_the_clipboard` answer *no* afterwards
    /// and stops `Ctrl+V` pasting layers the user has since replaced.
    pub(crate) fn copy_as_svg(&mut self, ctx: &egui::Context) {
        let ids = build::in_document_order(&self.session.doc, self.session.selection.ids());
        if ids.is_empty() {
            return;
        }
        // **The reported form** (§15 D780, `[S8.1-L7-05]`). A clipboard payload is
        // the one export with no file to inspect afterwards, so the status line is
        // the only chance to say the markup is not quite the document — and the
        // asymmetry this closes is that *pasting* SVG has said so since it was
        // written (`canvas.rs`'s paste message, off `Import::skipped`).
        let out =
            ondin_export::svg::svg_of_reported(&self.session.doc, &self.session.resolved, &ids);
        let note = crate::panels::fidelity_clauses(&out);
        ctx.copy_text(out.svg);
        let n = ids.len();
        self.session.info(match note {
            None => format!("Copied {n} layer(s) as SVG"),
            Some(note) => format!("Copied {n} layer(s) as SVG — {note}"),
        });
    }

    /// *Copy as PNG* — the selection as a **picture** on the clipboard, 1:1
    /// (`context-menus.md` §3's Export group, §15 D259's other half).
    ///
    /// **`raster_of`, not `png_of`.** `arboard::ImageData` takes raw straight-alpha
    /// RGBA and converts on the way out; `png_of` would hand it *encoded PNG
    /// bytes*, which it would then treat as pixels. That mistake pastes as noise
    /// rather than failing, which is why the function it does not call is named
    /// here.
    ///
    /// **`RasterOpts::default()` is the whole specification**: scale 1.0, no
    /// background, no trim, no padding. One resolution and no submenu — Figma
    /// offers a single *Copy as PNG* too, checked on the machine rather than
    /// assumed. A multiplier belongs to the inspector's Export panel, which is
    /// where a *repeatable* export keeps settings (§15 D274).
    ///
    /// **`build::in_document_order` for [`Self::copy_as_svg`]'s two reasons** — a
    /// selection holding a group and something inside it would paint that child
    /// twice, and a selection picked front-to-back would come out restacked (§15
    /// D267). `raster_of` keeps the caller's order and cannot check it, exactly as
    /// `svg_of` cannot.
    ///
    /// **No `clipboard_stamp`, and that is the load-bearing part**, inherited
    /// deliberately from `copy_as_svg`: writing one would make `owns_the_clipboard`
    /// answer *yes* while the clipboard held a flattened picture, so `Ctrl+V` would
    /// go on offering layers the user has since replaced — and of every row that
    /// writes to that clipboard this is the one whose content cannot be turned back
    /// into layers **at all**. Leaving the stamp alone is what makes the next paste
    /// degrade honestly. (Not "the three rows", which this said first: *Cut* and
    /// *Copy* reach the system clipboard too, through `copy_selection`, and so do
    /// `copy_guides` and a text session's own `Copy`.)
    pub(crate) fn copy_as_png(&mut self) {
        let ids = build::in_document_order(&self.session.doc, self.session.selection.ids());
        if ids.is_empty() {
            return;
        }
        let Some(raster) =
            Self::png_for_the_clipboard(&self.session.doc, &self.session.resolved, &ids)
        else {
            self.session.info("Nothing to copy as PNG");
            return;
        };
        let image = arboard::ImageData {
            width: raster.width as usize,
            height: raster.height as usize,
            bytes: raster.rgba.into(),
        };
        // Opened here rather than through egui, which carries a *text* clipboard
        // only — the same reason `paste_image` opens `arboard` itself (§15 D183).
        // Under `with_clipboard`'s gate like every other handle (§15 D796), and
        // refused outright on a headless app (§15 D798) — this is the one path
        // that would *overwrite* what the developer had copied.
        if !clipboard_is_reachable() {
            self.session
                .info("Copying to the clipboard is off in this build".to_string());
            return;
        }
        let wrote = with_clipboard(|c| c.set_image(image))
            .unwrap_or(Err(arboard::Error::ClipboardNotSupported));
        match wrote {
            Ok(()) => self
                .session
                .info(format!("Copied {} layer(s) as PNG", ids.len())),
            Err(e) => self
                .session
                .info(format!("Could not copy the picture: {e}")),
        }
    }

    /// The picture *Copy as PNG* hands the clipboard — **taking no `self`, so the
    /// choice is testable without an app**, since [`Self::copy_as_png`] itself ends
    /// in the OS clipboard and a test of that would write to the user's.
    ///
    /// Everything this decides is in the two lines: `RasterOpts::default()`, which
    /// is 1:1 and transparent, and `raster_of` rather than `png_of`. Pinned by
    /// `the_clipboard_picture_is_raw_pixels_at_one_to_one`, which asserts the byte
    /// count is `w × h × 4` — encoded PNG bytes are not, and that is the whole
    /// point of the assertion.
    fn png_for_the_clipboard(
        doc: &ondin_core::Document,
        res: &ondin_core::Resolved,
        ids: &[ondin_core::NodeId],
    ) -> Option<ondin_export::png::Raster> {
        ondin_export::png::raster_of(doc, res, ids, &ondin_export::png::RasterOpts::default())
    }

    /// The layer *Copy properties* would lift an appearance off — the **key
    /// layer** when the selection has one, else a lone selected layer.
    ///
    /// **The key layer is already this app's answer to "which of these is the
    /// reference"** (§15 D113), so a multi-selection is not ambiguous here the way
    /// it is in Figma, which simply refuses one. Without a key it *is* ambiguous
    /// and this answers `None`: picking the first in z-order would be a rule with
    /// nothing behind it, and the row says so rather than guessing.
    ///
    /// `None` too for a layer with no appearance to give — a group — which is
    /// `build::properties_of`'s own refusal and not a second rule stated here.
    pub(crate) fn properties_source(&self) -> Option<(NodeId, build::Properties)> {
        let sel = &self.session.selection;
        let id = sel.key().or_else(|| sel.single())?;
        let props = build::properties_of(self.session.doc.get(id)?)?;
        Some((id, props))
    }

    /// Lift the source layer's fills, strokes and opacity into
    /// [`Self::property_clipboard`] — `Ctrl`+`Alt`+`C`.
    ///
    /// **The committed node, not the display one.** A gesture in flight previews
    /// through `RenderOverrides` and its values are not the document's yet; copying
    /// what is on screen mid-drag would put a number on the clipboard that no undo
    /// step ever contained.
    fn copy_properties(&mut self) {
        let Some((_, props)) = self.properties_source() else {
            return;
        };
        let what = match (props.fills.len(), props.strokes.len()) {
            (0, 0) => "opacity",
            (_, 0) => "fill and opacity",
            (0, _) => "stroke and opacity",
            _ => "fill, stroke and opacity",
        };
        self.property_clipboard = Some(props);
        self.session.info(format!("Copied {what}"));
    }

    /// Give the selection the appearance the clipboard is holding —
    /// `Ctrl`+`Alt`+`V`, `build::paste_properties`.
    ///
    /// One transaction and so one undo step, across however many layers and
    /// however many of the three properties actually changed. An empty transaction
    /// — every target already looked like this — commits nothing and says so, which
    /// is better than a silent no-op on a chord whose effect can be subtle.
    fn paste_properties(&mut self) {
        let Some(props) = self.property_clipboard.clone() else {
            return;
        };
        let ids = self.session.selection.ids().to_vec();
        let tx = build::paste_properties(&self.session.doc, &ids, &props);
        if tx.0.is_empty() {
            self.session.info("Already has these properties");
            return;
        }
        // How many layers actually changed, which is not the selection's size:
        // the walk reaches through groups, stops at frames, and drops anything
        // that already agreed. `Operation::overwrites` is the right question for
        // all four op kinds this transaction can hold, and the dedup is because one
        // layer can take a fill *and* an opacity.
        let mut touched: Vec<_> = tx.0.iter().filter_map(|op| op.overwrites()).collect();
        touched.sort_unstable();
        touched.dedup();
        let layers = touched.len();
        if self.session.commit(tx) {
            self.session.info(match layers {
                1 => "Pasted properties".to_owned(),
                n => format!("Pasted properties onto {n} layers"),
            });
        }
    }

    /// Returns whether it took the paste, so [`Self::paste`] can fall through to
    /// text when the app is holding nothing.
    fn paste_clipboard(&mut self) -> bool {
        self.paste_clipboard_at(PASTE_OFFSET)
    }

    /// [`Self::paste_clipboard`] with the placement offset spelled out, which is
    /// the whole of what *Paste here* adds (`docs/context-menus.md` §4).
    fn paste_clipboard_at(&mut self, offset: Vec2) -> bool {
        // The guide half, which is the other thing the clipboard can be holding.
        // The two are mutually exclusive by construction, so whichever is set is
        // the answer and there is nothing to disambiguate.
        //
        // The guide step is the constant rather than anything derived from `offset`:
        // this door's offset is a layer's two-axis delta and a guide has one axis, so
        // there is nothing to project. *Paste in place* is the one caller that needs
        // a different step, and it takes the guide arm itself before reaching here
        // (§15 D248).
        if self.paste_guides(GUIDE_PASTE_OFFSET) {
            return true;
        }
        let Some(clip) = self.clipboard.clone() else {
            return false;
        };
        let templates = &clip.subtrees;
        // **The canvas, not "the first frame".** A layer does not need a frame to
        // live in — every tool that draws on bare canvas creates a root child
        // through `draw_target` — so a paste that refused one was enforcing a rule
        // the rest of the app does not have, and the frame it picked instead was
        // whichever happened to be first in z-order rather than anywhere the user
        // was looking (§15 D221).
        let fallback = self.session.doc.root();
        let mut placements = Vec::new();
        for template in templates {
            // Back where it came from if that parent still exists, else the
            // first frame — pasting into a deleted group should not fail.
            let parent = template
                .first()
                .and_then(|n| n.parent())
                .filter(|p| self.session.doc.contains(*p))
                .unwrap_or(fallback);
            // **Just above what it was copied from, not on top of everything.** A
            // captured subtree keeps the *original's* ids until `insert_subtrees`
            // remaps them, so the node it came from can be looked up and its slot
            // read — and when that node is gone (cut, deleted, or the paste is going
            // somewhere else entirely) there is no "just above it" to name and
            // appending is the only answer left.
            let from = template.first().map(|n| n.id());
            let index = from
                .filter(|id| self.session.doc.get(*id).and_then(|n| n.parent()) == Some(parent))
                .and_then(|id| {
                    self.session
                        .doc
                        .get(parent)?
                        .children()
                        .iter()
                        .position(|c| *c == id)
                })
                .map(|i| i + 1);
            placements.push(build::Placement {
                nodes: template.clone(),
                parent,
                index,
            });
        }
        // Down and right, so the copy is obviously a copy rather than sitting
        // invisibly on top of what it came from.
        self.insert_all(placements, "Pasted", offset, &clip.images);
        true
    }

    /// *Paste*, through the **layers panel's** door: the clipboard as a sibling
    /// immediately above the row that was right-clicked (§15 D226).
    ///
    /// **The one row whose meaning the door changes**, and the last of §1's table to
    /// be built — `docs/context-menus.md` §9.6.3 recorded it as live and correct but
    /// pasting by the ordinary rule. The rule it replaces is *back where it came
    /// from, just above the original*, which is right for a chord that was aimed at
    /// nothing and wrong for a click that was aimed at a row: a panel paste is the
    /// gesture for saying **where in the tree**, and answering it with the source's
    /// own slot ignores the only thing the user said.
    ///
    /// **The same three payloads in the same order as everywhere else** — a picture,
    /// the app's own subtrees, then text (§15 D218, D224) — because a row and the
    /// chord beside it must not come to mean different things. Only the middle one
    /// has a tree slot to place: a picture replaces a fill or lands on the canvas,
    /// and a pasted text layer belongs to the frame the selection is in. Those two
    /// keep their own rules rather than being bent to a row that is about
    /// *ordering*, which is what this door adds.
    pub(crate) fn paste_beside(&mut self, id: NodeId) {
        if self.paste_image(None) {
            return;
        }
        match self.take_clipboard() {
            Clipboard::Layers if self.paste_clipboard_beside(id) => return,
            Clipboard::Unreadable => return,
            _ => {}
        }
        self.paste_text_as_layer(None);
    }

    /// The layer half of [`Self::paste_beside`]. Returns whether it took the paste.
    ///
    /// **Falls back to the ordinary rule rather than refusing**, in the two cases
    /// where "beside this row" has no answer: a row whose parent has gone, and the
    /// root itself. `paste_clipboard_at` is then exactly what the chord would have
    /// done, which is the honest approximation — the alternative is a menu row that
    /// silently does nothing, and §15 D221 is the record of what a placement rule
    /// answering "no" too easily costs.
    fn paste_clipboard_beside(&mut self, id: NodeId) -> bool {
        // Guides are the other payload and have no place in the tree at all, so
        // this door means nothing to them and they take the path they always take.
        if self.paste_guides(GUIDE_PASTE_OFFSET) {
            return true;
        }
        let Some(clip) = self.clipboard.clone() else {
            return false;
        };
        let templates = &clip.subtrees;
        // The slot itself is `crate::canvas::sibling_slot_above`, a free function over
        // the `Document` so the arithmetic — and the two cases with no answer — are
        // testable without an `OndinApp` (§15 D221's habit, D226's rule).
        let Some((parent, above)) = crate::canvas::sibling_slot_above(&self.session.doc, id) else {
            return self.paste_clipboard_at(PASTE_OFFSET);
        };
        // Every template goes to the one slot rather than to a slot each.
        // `insert_subtrees` advances a placement past the indices already inserted
        // at or below it (§15 D100), so several arrive in the order they are listed
        // instead of collapsing onto each other.
        let placements = templates
            .iter()
            .map(|template| build::Placement {
                nodes: template.clone(),
                parent,
                index: Some(above),
            })
            .collect();
        // Offset like every other paste: a copy landing exactly on top of something
        // is invisible, and this door decided the *slot*, not the position.
        self.insert_all(placements, "Pasted", PASTE_OFFSET, &clip.images);
        true
    }

    /// *Paste here*: the clipboard, centred on `world` (`docs/context-menus.md` §4).
    ///
    /// **The positional door.** The chord offsets by a fixed 20×20; a menu was
    /// opened somewhere, so this one can aim.
    ///
    /// ⚠️ **The reason this used to give was "a key has no position to paste at",
    /// and that is false.** A key press arrives on a frame like any other event and
    /// `canvas::drop_point` reads a world point off a bare `&Context`, which is the
    /// whole of what such an arm would need. So the chord *could* aim, and
    /// what keeps it from doing so is a choice rather than a limit: it is
    /// deliberately position-free, which is what makes it land somewhere predictable
    /// wherever the mouse happens to be resting. That is the rule
    /// [`Self::image_drop_target`] already states from the other side — "pressing
    /// the paste keys with a shape selected *is* the act of aim, where a drag is
    /// aimed by the pointer".
    ///
    /// `docs/shortcuts.md` §7 carries a row wanting the chord to aim at the pointer
    /// as well (Figma does), which is not decided here and is not blocked by
    /// anything: it would be one arm, and the off-canvas case already has its
    /// answer below — `Item::PasteHere`'s `None` arm falls through to the ordinary
    /// paste, and `drop_point` filters on `canvas_rect` to produce that `None`.
    ///
    /// The offset is measured from where the copy *came from*, because that is
    /// what `insert_subtrees` applies its delta to — so this needs the source
    /// layers' bounds, and it takes them from [`Self::clipboard_from`], which the
    /// copy recorded.
    ///
    /// ⚠️ **This said a cut "falls back to the fixed offset, and deliberately"
    /// until 2026-08-20, and that was a bug wearing a justification** (§15 D251).
    /// The reasoning — that the alternative is inventing a rectangle for something
    /// the document no longer holds — asked the wrong question: nothing has to be
    /// invented, because the box was *there* at copy time and `cut_selection` copies
    /// before it deletes. What it actually did was turn every cut-then-paste-here
    /// into a *Paste in place*, repeat after repeat, since the templates keep the
    /// original ids and the lookup therefore kept failing.
    ///
    /// **Text falls through to a new layer at the pointer**, which is [`Self::paste`]'s
    /// last arm reaching the second door — otherwise the chord would create a text
    /// layer and the row six pixels away would do nothing.
    ///
    /// **And a picture goes first, in the same order [`Self::paste`] asks its three
    /// questions in** (§15 D224). It was the one payload the row could not reach at
    /// all: the chord placed it and the row six pixels away did nothing, which is
    /// exactly the mismatch the text arm above exists to prevent, left standing for
    /// pictures because `paste_image` took no point. It takes one now.
    pub(crate) fn paste_at(&mut self, world: Point) {
        if self.paste_image(Some(world)) {
            return;
        }
        match self.take_clipboard() {
            // **`clipboard_from` is read after the adoption, not before it** —
            // a foreign copy brings its own source box, so *Paste here* aims a
            // crossing copy exactly as it aims a local one.
            Clipboard::Layers if self.paste_clipboard_at(paste_aim(self.clipboard_from, world)) => {
                return;
            }
            Clipboard::Unreadable => return,
            _ => {}
        }
        // Markup lands where the pointer is, like every other arm of this verb —
        // which is the one placement difference between *Paste here* and the chord,
        // and the reason `paste_svg` takes a point at all.
        if self.paste_svg(Some(world)) {
            return;
        }
        self.paste_text_as_layer(Some(world));
    }

    /// Ctrl+D: a copy **in place**, then — for a copy of a copy — a copy that
    /// repeats however far the last one was moved.
    ///
    /// Duplicate, nudge it across, duplicate again, and every press after that
    /// steps by the same amount: the gesture becomes "make a row of these"
    /// without ever measuring anything. Figma does this and it is one of the
    /// things people miss most when a tool does not.
    ///
    /// The step only exists for a duplicate *of a duplicate* — see
    /// [`CloneChain`] — because a fresh original has no movement to repeat, and
    /// silently offsetting the first copy would be the old behaviour this
    /// deliberately replaces.
    fn duplicate_selection(&mut self) {
        // Guides duplicate too, and take the paste path's placement rule rather
        // than a `CloneChain`: a guide has one number, so there is no "however far
        // you moved the last copy" to repeat that a fixed step does not already
        // give. Because the copy becomes the selection, holding Ctrl+D walks along
        // in even steps — which is the row-of-guides case, for free.
        if self.duplicate_guides() {
            return;
        }
        let ids = build::outermost(&self.session.doc, self.session.selection.ids());
        let step = self.clone_step(&ids);

        let mut placements = Vec::new();
        for id in &ids {
            let Some(template) = self.session.doc.capture_subtree(*id) else {
                continue;
            };
            let Some(parent) = self.session.doc.get(*id).and_then(|n| n.parent()) else {
                continue;
            };
            // Immediately above the original, like Figma.
            let after = self
                .session
                .doc
                .get(parent)
                .and_then(|p| p.children().iter().position(|c| *c == *id))
                .map(|i| i + 1);
            placements.push(build::Placement {
                nodes: template,
                parent,
                index: after,
            });
        }
        // No pictures to carry: a duplicate is made from the document it lands in,
        // so every id it references is already in that document's table.
        self.insert_all(placements, "Duplicated", step, &[]);

        // Remember where this copy landed, so moving it and pressing again
        // repeats that move.
        self.clones = match self.session.selection.single() {
            Some(new_id) => self.origin_of(new_id).map(|at| CloneChain {
                last: new_id,
                placed_at: at,
                step,
            }),
            None => None,
        };
    }

    /// How far the next duplicate of `ids` should be offset.
    ///
    /// Zero unless the single selected node is the clone this chain last made:
    /// then it is however far that clone has been moved since, or — if it has
    /// not been moved — the step already being repeated, so holding Ctrl+D lays
    /// out an evenly spaced row rather than piling copies on the second one.
    fn clone_step(&self, ids: &[NodeId]) -> Vec2 {
        let [id] = ids else { return Vec2::ZERO };
        let Some(chain) = self.clones.as_ref().filter(|c| c.last == *id) else {
            return Vec2::ZERO;
        };
        let Some(now) = self.origin_of(*id) else {
            return Vec2::ZERO;
        };
        let moved = now - chain.placed_at;
        if moved.hypot() > 1e-6 {
            moved
        } else {
            chain.step
        }
    }

    /// A node's origin in world space — the reference point the clone chain
    /// measures movement against.
    fn origin_of(&self, id: NodeId) -> Option<Point> {
        Some(self.session.resolved.world_transform(id)? * Point::ZERO)
    }

    /// Insert several captured subtrees as one transaction (one undo step),
    /// offset by `offset` in their parent's space, and select the results. The
    /// placement arithmetic lives in `build::insert_subtrees`.
    /// Insert `placements`, carrying `images` in with them.
    ///
    /// **`images` is what a set of nodes cannot carry by itself** — the table
    /// entries their fills key into, taken at copy time (`Clip`). Only the ones
    /// this document is missing are added, so pasting back into the document a
    /// copy came from produces no extra ops and the ordinary case is unchanged;
    /// the duplicate path passes none at all, being unable to leave the document
    /// it reads from.
    fn insert_all(
        &mut self,
        placements: Vec<build::Placement>,
        verb: &str,
        offset: Vec2,
        images: &[(ondin_core::ImageId, ondin_core::ImageEntry)],
    ) {
        if placements.is_empty() {
            return;
        }
        let (tx, created) = build::insert_subtrees(
            &self.session.doc,
            &mut self.session.ids,
            &placements,
            offset,
        );
        // **First in the transaction, which is `build::missing_image_ops`'s rule
        // and its reason**: one paste is one undo step, and the inverse has to take
        // the table entry out *after* the layers that were using it.
        let mut ops = build::missing_image_ops(&self.session.doc, images);
        ops.extend(tx.0);
        let tx = Transaction(ops);
        if self.session.commit(tx) && !created.is_empty() {
            self.session
                .info(format!("{verb} {} layer(s)", created.len()));
            self.session.selection.set(created);
        }
    }

    pub(crate) fn delete_selection(&mut self) {
        // Guides are the other thing Delete can be aimed at. They are never
        // selected alongside layers (`Selection`), so this is an early return
        // rather than a second half.
        if !self.session.selection.guides().is_empty() {
            self.remove_selected_guides();
            return;
        }
        let ids = build::outermost(&self.session.doc, self.session.selection.ids());
        if ids.is_empty() {
            return;
        }
        // **A frame's guides go with it**, in the same transaction, so deleting a
        // page does not leave lines scoped to something that is gone — and one
        // Ctrl+Z brings back both.
        //
        // Guides *first*: `apply` inverts a transaction by reversing it, so this
        // order is what puts the frame back before its guide on the way out.
        // `build::guides_of` documents the trap; the other order deletes just as
        // correctly and cannot be undone.
        let mut ops = build::guides_of(&self.session.doc, &ids);
        let guides = ops.len();
        ops.extend(ids.iter().map(|id| Operation::DeleteNode { id: *id }));
        if self.session.commit(Transaction(ops)) {
            self.session.selection.clear();
            self.session.info(match guides {
                0 => format!("Deleted {} layer(s)", ids.len()),
                n => format!("Deleted {} layer(s) and {n} guide(s)", ids.len()),
            });
        }
    }

    pub(crate) fn child_count(&self, parent: NodeId) -> usize {
        self.session
            .doc
            .get(parent)
            .map(|n| n.children().len())
            .unwrap_or(0)
    }
}

// --- file IO and close handling -------------------------------------------

impl OndinApp {
    /// Write the open document to its place in the library, optionally pinning a
    /// version.
    ///
    /// **There is no dialog and no path to choose.** Every document lives in the
    /// base folder, so a save either has a path already or the document has never
    /// been filed — and filing it is [`crate::library::store::file_document`]'s
    /// job, not a question for the user.
    ///
    /// `pin` is what separates the two ways a save happens. `Ctrl+S` pins: it
    /// writes the file *and* copies it into version history, which is what makes
    /// a manual save mean something once autosave is writing every thirty
    /// seconds. Autosave does not, or the history would be a list of moments
    /// nobody chose.
    /// ⚠️ **This one is still synchronous, and that is a decision** (§15 D393).
    /// Autosave queues onto `library::writer`; `Ctrl+S`, the walk to the
    /// dashboard, an open and the close-and-save arm all still write from the
    /// frame. Two reasons, neither of them "not got to it yet". A pin is a *copy
    /// of the file* — `pin_version` reads the bytes back off the disk — so
    /// deferring the write only moves the cost of `Ctrl+S` rather than removing
    /// it. And the three transitions each read the result on the next line: they
    /// close the window, leave the editor, or replace the document, and every one
    /// of them is wrong if the bytes are not down yet.
    ///
    /// ⚠️ **Which makes the settle below load-bearing rather than tidy.** An
    /// autosave may be in flight for this very path, taken at an older revision —
    /// so without the settle it can land *after* this save and put the earlier
    /// document back on disk. This is the line that keeps that from happening.
    ///
    /// ⚠️ **The reason used to be the shared `atomic` temp file and that half is
    /// gone** (§15 D548): `atomic::temp_for` is unique per write now, so two
    /// writers of one path no longer tear the destination. The ordering hazard
    /// above is what is left, and it was always the larger one.
    fn save_file(&mut self, pin: bool) {
        self.disk_settle();
        let path = match self.session.path.clone() {
            Some(p) => p,
            None => match self.file_untitled() {
                Some(p) => p,
                None => return,
            },
        };
        if let Err(e) = crate::library::store::write_document(&path, &self.session.doc) {
            self.session.fail(format!("Save failed: {e}"));
            return;
        }
        // No status message: the pill goes to "Saved · just now" and the top bar
        // already names the file, so echoing the path was one more thing on
        // screen saying what two others said.
        self.session.mark_saved(path.clone());
        self.last_autosave = std::time::Instant::now();

        if pin && self.prefs.version_history {
            // **The scan is what turns a path into an entry, and the entry is
            // what carries the id a version is filed under.** Refreshing first
            // rather than reusing the last scan: this document may have been
            // filed a moment ago by the branch above, in which case the library
            // has never seen it.
            self.library.refresh();
            if let Some(entry) = self.library.entry_at(&path).cloned() {
                match crate::library::store::pin_version(&self.library.root, &entry) {
                    Ok(Some(_)) => self.session.info("Version saved"),
                    // A document with no id — one opened from outside the library
                    // — has no history to pin to. Silent: the save itself worked,
                    // and the pill says so.
                    Ok(None) => {}
                    Err(e) => self.session.fail(format!("Version failed: {e}")),
                }
            }
        }

        // **After the save, and only if it succeeded.** Re-exporting a document
        // that failed to write would put files on disk that correspond to
        // nothing saved — the one state this switch must not be able to produce.
        // It writes its own status line, which is what keeps a side effect of
        // Ctrl+S from being a silent one (`prefs::Prefs::export_on_save`).
        if self.prefs.export_on_save {
            self.export_all(crate::panels::ExportAll::Quietly);
        }
    }

    /// File a document that has never been saved, under whatever name it
    /// carries.
    ///
    /// The starter document has no name and no id, so this is where a session
    /// that began on the canvas rather than from the dashboard acquires both.
    /// Returns the path, or `None` if the write failed — in which case the
    /// failure has already been reported.
    fn file_untitled(&mut self) -> Option<std::path::PathBuf> {
        let name = self
            .session
            .doc
            .meta()
            .name
            .clone()
            .unwrap_or_else(|| "Untitled".to_string());
        let project = self
            .session
            .doc
            .meta()
            .project
            .clone()
            .and_then(|id| self.library.projects.get(&id).cloned());
        let root = self.library.root.clone();
        match crate::library::store::file_document(
            &root,
            project.as_ref(),
            &name,
            &mut self.session.doc,
        ) {
            Ok(path) => {
                self.library.refresh();
                Some(path)
            }
            Err(e) => {
                self.session.fail(format!("Could not create the file: {e}"));
                None
            }
        }
    }

    /// Write the document if the autosave interval has elapsed and there is
    /// something to write.
    ///
    /// ⚠️ **Gated on `is_dirty`, so an idle session writes nothing.** Without
    /// that, a document left open overnight rewrites its own bytes every thirty
    /// seconds — which is invisible locally and is a file change every sync
    /// client in the world will pick up and upload.
    ///
    /// **And it never pins.** See [`Self::save_file`].
    ///
    /// ⚠️ **This queues; it does not save** (`library::writer`, §15 D393). The
    /// serialise is the expensive half — up to 1.15 s in debug for a
    /// photo-carrying document — and it is now the worker's. What stays here is
    /// the same four gates and the clock; what leaves is the write, the
    /// `mark_saved` and the export, all of which move to the *result*
    /// ([`Self::apply_disk`]). The session therefore stays **dirty** across the
    /// gap, with the pill reading *Saving…*, because until the bytes land that
    /// is simply true.
    ///
    /// ⚠️ **A fourth gate, `is_saving`, and it is not redundant with the clock.**
    /// The interval is stamped at queue time, so a second queue is thirty seconds
    /// away in the ordinary case — but `autosave_secs` is the user's and can be
    /// set to one, which is shorter than a 60 MB serialise. Two writes of one path
    /// in flight then finish in whatever order the disk gives them, so the older
    /// revision can be the one that lands. (Until §15 D548 they would also have
    /// shared `atomic`'s one temp file and torn the document outright; that half
    /// is fixed at the write layer, and this gate is what stops the rest.)
    fn autosave_tick(&mut self, ctx: &egui::Context) {
        let interval = self.prefs.autosave_secs;
        if interval == 0 || !self.session.is_dirty() || self.session.is_saving() {
            return;
        }
        if self.last_autosave.elapsed().as_secs() < interval {
            return;
        }
        // A document that has never been filed is *not* autosaved into
        // existence: creating a file on a timer, for a canvas the user may have
        // been doodling on, puts something in their library they did not ask
        // for. The first save of a new document is theirs to make.
        let Some(path) = self.session.path.clone() else {
            return;
        };
        // ⚠️ **Stamped at queue time, like the snapshot's.** On failure that is a
        // back-off, and on success it is the interval doing what it always did —
        // measuring from when the save was *asked for*, not from when a slow disk
        // finished it, which would make a big document autosave less often the
        // slower it got.
        self.last_autosave = std::time::Instant::now();
        let at = self.session.revision();
        let doc = self.session.doc.clone();
        self.session.begin_save(at);
        self.writer(ctx).document(&path, doc, at);
    }

    /// Take a crash snapshot if the interval has elapsed and something has been
    /// committed since the last one (`library::recovery`, §15 D377).
    ///
    /// **Three gates, and each one is a different mistake.** `is_dirty` is
    /// "there is work the file does not have"; the interval is "not more often
    /// than once every [`SNAPSHOT_SECS`]"; and the revision is "something
    /// actually changed since last time" — without the third, a document edited
    /// once and then left alone rewrites identical bytes every ten seconds
    /// forever, which is the sync-thrash `autosave_tick` avoids one level up.
    ///
    /// ⚠️ **No `path.is_none()` gate, which is the one place this deliberately
    /// parts company with autosave.** Autosave refuses to file an unsaved
    /// document because doing so puts a file in the user's library they did not
    /// ask for; a snapshot goes into `.recovery/`, which the scan skips, so
    /// nothing appears anywhere. That leaves the case with the most to lose —
    /// an hour of drawing on a document that has never been saved — as the one
    /// case this covers and autosave cannot.
    ///
    /// ⚠️ **The write itself does not happen here** — it is queued onto
    /// [`writer::Writer`], whose module note carries the measurement that put it
    /// there. What this function keeps is every *decision*: the three gates, the
    /// key, and the clock.
    ///
    /// [`SNAPSHOT_SECS`]: crate::library::recovery::SNAPSHOT_SECS
    /// [`writer::Writer`]: crate::library::writer::Writer
    fn recovery_tick(&mut self, ctx: &egui::Context) {
        use crate::library::recovery;
        // ⚠️ **The results drain used to be here and is now the frame's**
        // ([`OndinApp::disk_results`]), because autosave joined the same queue
        // and both ticks gate on state a result changes. A drain owned by one of
        // them would have decided the other's frame by accident. Everything this
        // function needs from a result is therefore already applied when it runs;
        // a test driving this directly has to say so itself.
        if !self.session.is_dirty() {
            // Clean: the document on disk *is* the document, so a snapshot is a
            // claim about work that no longer exists anywhere else.
            self.drop_recovery();
            return;
        }
        let due = self
            .recovery
            .last
            .is_none_or(|t| t.elapsed().as_secs() >= recovery::SNAPSHOT_SECS);
        if !due || self.recovery.at == Some(self.session.revision()) {
            return;
        }
        let key = match self.recovery.key.clone() {
            Some(key) => key,
            None => {
                // The document's own id when it has one, so the snapshot can be
                // matched back to the file it belongs to; a minted one when it
                // does not, which is the never-filed case.
                let key = self
                    .session
                    .doc
                    .meta()
                    .id
                    .clone()
                    .unwrap_or_else(recovery::mint_key);
                self.recovery.key = Some(key.clone());
                key
            }
        };
        let root = self.library.root.clone();
        // ⚠️ **The clock is stamped whichever way this goes.** On failure that is
        // a back-off: a folder that cannot be written to will not start
        // succeeding within the frame, and retrying every frame would turn a
        // permissions problem into a stutter.
        self.recovery.last = Some(std::time::Instant::now());
        // ⚠️ **`recovery.at` is *not* set here, and that is what keeps the field
        // honest.** It means "this revision is on disk", so it is written from
        // the worker's answer and nowhere else. Nothing re-queues the same
        // revision in the meantime: the clock stamped above holds the next tick
        // off for [`SNAPSHOT_SECS`], which is ten times the worst serialise this
        // was measured at.
        let at = self.session.revision();
        let doc = self.session.doc.clone();
        self.writer(ctx).snapshot(&root, &key, doc, at);
    }

    /// The background writer, started if this is the first job (§15 D392, D393).
    ///
    /// **The `Context` is the whole reason the two ticks take one.** It is cloned
    /// into the worker so an arriving result can `request_repaint` — see
    /// `library::writer`'s module note. Nothing that only *uses* an existing
    /// writer needs one, which is why [`Self::forget_snapshot`] can do without.
    fn writer(&mut self, ctx: &egui::Context) -> &mut crate::library::writer::Writer {
        self.writer
            .get_or_insert_with(|| crate::library::writer::Writer::spawn(ctx))
    }

    /// Apply whatever the writer has finished, without waiting.
    ///
    /// **Called once at the top of the frame, ahead of both ticks**, rather than
    /// inside either. Both of them gate on state a result changes — the snapshot
    /// on `recovery.at`, the autosave on the session being dirty — so a drain
    /// belonging to one of them would decide the other's frame by accident.
    fn disk_results(&mut self) {
        let Some(writer) = self.writer.as_mut() else {
            return;
        };
        let done = writer.drain();
        self.apply_disk(done);
    }

    /// Block until the writer's queue is empty, then apply it.
    ///
    /// ⚠️ **Only where there is no next frame, where this frame is about to write
    /// the same file itself, or where the folder is about to move.** A close ends
    /// the process, so a queued removal that has not run yet becomes a snapshot
    /// the next launch asks about — work the user has already dealt with — and a
    /// queued *write* that has not run yet is the loss both features exist to
    /// prevent. [`Self::save_file`] is the second: it writes from the frame, and
    /// an autosave already in flight for that path can land after it and put the
    /// older document back (its own doc has the rest, including which half of that
    /// argument §15 D548 took away). *Move my library* is
    /// the third — `library::relocate` carries `.recovery/` across on the UI
    /// thread, and a write still aimed at the old root either lands behind it or
    /// gets copied and then deleted from under the copy. Everywhere else, waiting
    /// is the cost the writer was built to stop paying.
    pub(crate) fn disk_settle(&mut self) {
        let Some(writer) = self.writer.as_mut() else {
            return;
        };
        let done = writer.settle();
        self.apply_disk(done);
    }

    /// The one reader of [`writer::Done`], so no caller above can come to mean
    /// something different by a result.
    ///
    /// [`writer::Done`]: crate::library::writer::Done
    fn apply_disk(&mut self, done: Vec<crate::library::writer::Done>) {
        use crate::library::writer::Done;
        for msg in done {
            match msg {
                // ⚠️ **Dropped when the session has no key**, which is a write
                // that finished after [`Self::drop_recovery`] took the snapshot
                // away. `at` gates the *next* write, so a stale one could silence
                // the first snapshot of whatever document replaced this one —
                // and `revision` is a per-session counter, so "a revision this
                // app has already written" is not the same claim after an adopt.
                Done::Snapshotted(at) => {
                    if self.recovery.key.is_some() {
                        self.recovery.at = Some(at);
                    }
                }
                Done::SnapshotFailed(e) => {
                    if !self.recovery.warned {
                        self.recovery.warned = true;
                        self.session
                            .fail(format!("Crash recovery is not running: {e}"));
                    }
                }
                Done::Forgot => {}
                // **The export moves with the write, not with the queueing.**
                // `save_file` runs it after a save that succeeded and only then
                // (`prefs::Prefs::export_on_save`); a queued save's "then" is
                // here. A superseded write does not count as a save, so it does
                // not re-export either — `finish_save` is what knows.
                Done::Saved { path, at } => {
                    if self.session.finish_save(&path, at) && self.prefs.export_on_save {
                        self.export_all(crate::panels::ExportAll::Quietly);
                    }
                }
                // ⚠️ **No latch, unlike the snapshot's.** A snapshot failing is
                // one standing condition worth saying once; a save failing is an
                // event, and the session stays dirty, so the next interval tries
                // again and the user needs to be told again. The pill is not
                // enough on its own — it goes back to *Unsaved*, which is what it
                // says when nothing has been attempted either.
                Done::SaveFailed(e) => {
                    self.session.save_failed();
                    self.session.fail(format!("Save failed: {e}"));
                }
            }
        }
    }

    /// *Recover unsaved work?* — the one question a launch is allowed to ask
    /// (§15 D377).
    ///
    /// **Drawn before the `View::Dashboard` return in [`eframe::App::ui`]**, so it
    /// appears over whichever screen the launch landed on. Order of *drawing* is
    /// not order of *painting* — `settings::card_modal` is an `egui::Modal`, which
    /// is an `Area` above every panel — so being first in the frame costs it
    /// nothing and buys it the only place both views pass through.
    ///
    /// ⚠️ **Recovering does not write the user's document.** It loads the
    /// snapshot into the session with the original's path and marks it *unsaved*
    /// ([`EditorSession::mark_unsaved`]), so the file on disk is untouched and the
    /// next thing that happens is the user's decision: save it, or undo their way
    /// out of it and close. A recovery that silently overwrote the file would be
    /// this feature destroying work in the name of protecting it — the snapshot is
    /// up to [`SNAPSHOT_SECS`] behind, and *behind* is not *better*.
    ///
    /// ⚠️ **The snapshot being recovered *from* is kept, not deleted.** The session
    /// is dirty the moment it loads, so [`Self::recovery_tick`] adopts the same key
    /// and keeps it current; deleting it here would leave the recovered work
    /// protected by nothing for the first ten seconds of its second life, which is
    /// not the moment to remove a safety net.
    ///
    /// 🚨 **But *Discard* is no longer the only thing that deletes** (§15 D768).
    /// [`Self::recover`] drops the **outgoing** session's snapshot, which it used to
    /// strand — and that sentence stood here, and in `architecture.md` §9.5 and §15
    /// D377, for as long as it did not. The drop is safe only because
    /// [`Self::may_replace_current_document`] runs in front of it, which is why the
    /// *Recover* arm below asks **before** it pops `pending`: a *Recover* the user
    /// backed out of is not a *Later*.
    ///
    /// [`SNAPSHOT_SECS`]: crate::library::recovery::SNAPSHOT_SECS
    fn recovery_modal(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.recovery.pending.last().cloned() else {
            return;
        };
        let waiting = self.recovery.pending.len();
        let mut decision: Option<RecoverChoice> = None;
        let modal = crate::settings::card("recover-unsaved", ctx, |ui| {
            ui.set_width(crate::ui::menu_inner_w(
                crate::settings::CARD_W,
                crate::settings::PAD,
            ));
            // ⚠️ **`settings::modal_title`, so this card has a ✕ like the other
            // six.** It nearly did not: the first version argued that leaving the
            // question unanswered is not a *nothing* answer, since the file stays
            // in `.recovery/` and the prompt returns next launch — and then
            // silently swallowed `Escape` and a backdrop click too, which is the
            // part that makes the argument wrong. Every modal in this app closes
            // that way (§15 D368), and one that does not reads as the app having
            // hung rather than as a question insisting on an answer. What is
            // dismissed is *this launch's* asking, never the snapshot; see
            // [`RecoverChoice::Later`].
            if crate::settings::modal_title(ui, "Recover unsaved work") {
                decision = Some(RecoverChoice::Later);
            }
            ui.add_space(8.0);
            // ⚠️ **The time clause goes when there is no time** (§15 D716). A
            // snapshot whose stamp cannot be read used to be deleted, so this
            // sentence could always name a moment; it is *offered* now, and the
            // two obvious fudges are both lies the user would act on — `0` reads
            // as "56 years ago" and `now` as "just now" about a file that may be
            // a week old. Saying less is the only honest option, and the rest of
            // the sentence carries the decision either way.
            ui.label(
                egui::RichText::new(match pending.written {
                    Some(written) => format!(
                        "“{}” has changes from {} that were never saved — Ondin \
                         closed before they reached the file.",
                        pending.name,
                        crate::library::clock::relative_label(crate::library::clock::since(
                            written
                        ))
                        .to_lowercase()
                    ),
                    None => format!(
                        "“{}” has changes that were never saved — Ondin closed \
                         before they reached the file.",
                        pending.name
                    ),
                })
                .size(12.0)
                .color(theme::text::MUTED),
            );
            if pending.target.is_none() {
                ui.add_space(6.0);
                // ⚠️ Worth its own line, because *Recover* means something
                // different here: there is no file to put the work back into, so
                // what comes back is an untitled document the user still has to
                // save. Saying so beats a person recovering, seeing "Unsaved" and
                // assuming the recovery failed.
                ui.label(
                    egui::RichText::new(
                        "This document was never saved, so it comes back untitled.",
                    )
                    .size(12.0)
                    .color(theme::text::FAINT),
                );
            }
            if waiting > 1 {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!("{} more after this one.", waiting - 1))
                        .size(12.0)
                        .color(theme::text::FAINT),
                );
            }
            ui.add_space(crate::settings::FOOTER_GAP);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if crate::settings::text_button(ui, "Recover", crate::ui::FieldButton::Set)
                    .clicked()
                {
                    decision = Some(RecoverChoice::Recover);
                }
                if crate::settings::text_button(ui, "Discard", crate::ui::FieldButton::Off)
                    .clicked()
                {
                    decision = Some(RecoverChoice::Discard);
                }
            });
        });
        // Escape and the backdrop, both *Later* — never *Discard*. The two
        // dismissals a modal gets for free are the ones a hand reaches for
        // without reading, and this is the one card in the app where the reflex
        // could otherwise destroy work.
        if modal.should_close() {
            decision = Some(RecoverChoice::Later);
        }

        match decision {
            Some(RecoverChoice::Recover) => {
                // **`open_path`'s guards, and they run before the pop** (§15 D768).
                // Declining the discard has to leave the card exactly as it was —
                // a *Recover* the user backed out of is not a *Later*, and popping
                // first would answer the question on their behalf.
                if self.may_replace_current_document("recovering another document") {
                    self.recovery.pending.pop();
                    self.recover(&pending);
                }
            }
            Some(RecoverChoice::Discard) => {
                self.recovery.pending.pop();
                // Through the same door as every other removal — see
                // [`Self::forget_snapshot`]. This key is a previous process's, so
                // nothing of this session's can be racing it; going the same way
                // anyway is what stops the two paths drifting apart the day a
                // discard can reach a key this session has written.
                self.forget_snapshot(&pending.key);
            }
            // ⚠️ **Popped without deleting**, which is the whole of what *Later*
            // means: the asking is over for this launch and the file is untouched,
            // so the next launch offers it again. Not popping would redraw the
            // same card on the next frame and make the ✕ look broken.
            Some(RecoverChoice::Later) => {
                self.recovery.pending.pop();
            }
            None => {}
        }
    }

    /// Load a snapshot into the session — see [`Self::recovery_modal`] for what
    /// this deliberately does not do.
    ///
    /// ⚠️ **Not an `open_path`, and three of its steps are missing on purpose.**
    /// That function marks the document opened in the per-machine index (the
    /// *Recent* list), writes `prefs.last_document`, and honours
    /// `collapse_groups_on_open`. None of the three is right here: a recovery is
    /// not the user reaching for a document, so it should not reorder *Recent*
    /// or change what the next launch reopens, and folding the layer tree of work
    /// somebody is about to inspect is the opposite of helpful. The one case
    /// where this reads oddly is a recovered document sitting lower in *Recent*
    /// than its own age suggests — which is true, and is what the list means.
    fn recover(&mut self, pending: &crate::library::recovery::Pending) {
        let bytes = match std::fs::read(&pending.path) {
            Ok(b) => b,
            Err(e) => return self.session.fail(format!("Recovery failed: {e}")),
        };
        let doc = match ondin_core::io::load(&bytes) {
            Ok(d) => d,
            Err(e) => return self.session.fail(format!("Recovery failed: {e}")),
        };
        // **The outgoing session's snapshot, dropped before the adopt** (§15 D768),
        // which is [`Self::open_path`]'s position and for the reason its comment
        // gives verbatim: *"Past this line the app is holding a different document
        // and has no way to name the old one's snapshot."* Without it this function
        // adopts and then **overwrites** `recovery.key` below, so the outgoing
        // snapshot is never named again — it stays in `.recovery/`, is offered at
        // every launch, and *Later* only pops it for that one, so the prompt returns
        // for ever.
        //
        // 🚨 **This line on its own made the reachable case worse, and that is why
        // §15 D681 held it open rather than landing it.** The finding's route —
        // editing the document behind the card — was closed by §15 **D464**. What is
        // live is the **queue**: [`Self::recovery_modal`] says *"N more after this
        // one"*, so *Recover* can be pressed twice with no keyboard, and the second
        // press finds `recovery.key` naming the snapshot the user recovered **one
        // press ago and has not saved**. Dropping it there deleted the last copy,
        // where leaking it merely offered it again.
        //
        // **`may_replace_current_document` is what makes the drop safe**, and the
        // two must not be separated: past that call the work has been saved or the
        // user has been asked, so there is nothing here that exists only in
        // `.recovery/`. It is exactly `open_path`'s argument — *"the two branches
        // above are a save and a confirmed discard, and there is no third way to
        // reach here"* — now true of this door too.
        self.drop_recovery();
        self.session.adopt_document(doc, pending.target.clone());
        // **After `adopt_document`, which sets it clean.** A just-opened document
        // matches its file and that is the right default everywhere else; this one
        // matches nothing on disk, and the pill has to say so.
        self.session.mark_unsaved();
        self.reset_transient_state();
        self.ensure_document_fonts();
        // The snapshot's key, adopted rather than re-minted: the next tick keeps
        // the same file current instead of starting a second one beside it.
        self.recovery.key = Some(pending.key.clone());
        self.recovery.at = None;
        self.recovery.last = None;
        self.view = View::Editor;
        self.session.info("Unsaved changes recovered");
    }

    /// Forget this session's crash snapshot: the work it held is on disk, or the
    /// user has said to throw it away.
    ///
    /// **Called from more places than the tick**, because the tick only runs on
    /// an editor frame — so anything that ends the session or replaces its
    /// document has to say so itself or leave a file behind that the next launch
    /// would ask about. **Five call sites across four functions**
    /// ([`Self::recovery_tick`]'s clean branch is the sixth and is the ordinary
    /// case): [`Self::close_confirmation`]'s *Save* and *Discard* arms, which end
    /// the process and have no next frame; [`Self::go_to_dashboard`], gated on the
    /// session being clean; [`Self::open_path`]; and [`Self::recover`].
    ///
    /// ⚠️ **The last of those is the only one with a precondition** (§15 D768).
    /// `recover` may drop **only** because [`Self::may_replace_current_document`]
    /// has already saved the work or had the user agree to lose it; without that,
    /// the key it drops names the snapshot recovered on the previous press of a
    /// queued card, and the drop deletes the last copy of it.
    ///
    /// ⚠️ **This list said *"a clean quit, a discard, and the walk back to the
    /// dashboard"*** and was short by `open_path` before D768 added `recover`.
    /// It is spelled as links now so `cargo doc` has something to break.
    pub(crate) fn drop_recovery(&mut self) {
        // ⚠️ **`library.root` is read now, not remembered with the key, and the
        // root can move under a live session** — Settings changes the base folder
        // and re-opens the library (`panels::dashboard::library_settings_modal`).
        // Both branches come out right, which is why this is a note rather than a
        // pair of fields: with *Move my library* on, `relocate` carried the
        // snapshot to the new root and this finds it there; with it off, the
        // snapshot stays in the old folder and is offered again if the user ever
        // points back at it — which is the honest answer, since that folder is
        // still a library with unrecovered work in it.
        if let Some(key) = self.recovery.key.take() {
            self.forget_snapshot(&key);
        }
        self.recovery.at = None;
        self.recovery.last = None;
    }

    /// Delete the snapshot filed under `key`, in the one order that is safe.
    ///
    /// ⚠️ **Down the writer's queue when there is one, not straight to the
    /// filesystem.** A write for this key may be in flight, and a removal that
    /// jumped it would let that write land afterwards and put the file back —
    /// which is *Discard* not discarding, and a clean quit still asking about
    /// itself at the next launch.
    ///
    /// **No writer means this session has never written a snapshot**, so there
    /// is nothing that could overtake the removal and nothing to order against.
    /// A recovered session is the case that reaches here: its key came from the
    /// previous process, and until its first tick nothing of this one's has been
    /// queued.
    fn forget_snapshot(&mut self, key: &str) {
        let root = self.library.root.clone();
        match self.writer.as_mut() {
            Some(writer) => writer.forget(&root, key),
            None => crate::library::recovery::remove(&root, key),
        }
    }

    /// Leave the editor for the library.
    ///
    /// **Autosaves on the way out rather than asking.** The dashboard is not a
    /// destructive destination — the document is still there and one click
    /// away — so a confirmation dialog here would be a question with no wrong
    /// answer. What makes that safe is that the work is written first; a
    /// document that has never been filed is the one case that cannot be, and it
    /// keeps its unsaved state in memory until the user saves it.
    ///
    /// ⚠️ **The text session is committed first, and that is not decoration**
    /// (§15 D415).
    /// A live editor holds its content and nothing else does — `preview_session`
    /// only calls `set_session_preview`, and [`Self::finish_text_edit`] is the
    /// sole committer — so a save taken before it ends writes the document
    /// *without* what the user has typed, and the `drop_recovery` below then
    /// takes away the last copy of it. `self.text` survives the walk — this
    /// function does not call [`Self::reset_transient_state`] — only to be
    /// thrown away by [`Self::open_path`], which does, on the way back in.
    ///
    /// ⚠️ **That used to read "thrown away by the `adopt_document`", and it was
    /// wrong in a way worth leaving a mark on**: `adopt_document` is an
    /// `EditorSession` method and cannot see `OndinApp::text` at all. The two
    /// are *adjacent lines* in `open_path`, not a call chain, so the sentence
    /// read as coherent and named a function that could not have done it. It
    /// came in from the review finding's own evidence, which spelled the pair
    /// with an arrow between them. This is the same guard
    /// [`Self::choose_tool`] has carried, for the same reason: a button that
    /// leaves the editor is an exit, and every other exit commits. Both call
    /// [`Self::finish_text_first`] now (§15 D466), so there is one spelling.
    ///
    /// ⚠️ **This said *"since §15 D294"*, and D294 is *Arming a creation tool
    /// clears the selection*, which says nothing about `finish_text_edit`.**
    /// `arch-scribe` caught it, and the same sentence is in **D415**'s own body,
    /// which is where it came from. **No §15 entry records when `choose_tool`
    /// gained that guard**, so the number is dropped rather than replaced: a
    /// citation that resolves and is wrong (`[S18.2-L3-07]`) is worse than none,
    /// and no gate can see either.
    pub(crate) fn go_to_dashboard(&mut self) {
        self.finish_text_first();
        if self.session.is_dirty() && self.session.path.is_some() {
            self.save_file(false);
        }
        // ⚠️ **Said here rather than left to the tick, because the tick is an
        // *editor* frame's job** — `OndinApp::ui` returns at the library — so a
        // snapshot would otherwise survive untouched for as long as the user
        // stayed on the dashboard.
        //
        // ⚠️ **And gated on the save above having worked, which is not the same
        // as it having run.** The line above declines to save a document with no
        // path — the never-filed one — so an unconditional drop here would take
        // the snapshot away from the one document in the app whose entire
        // existence is that snapshot, at the moment its owner walked away from
        // it. A clean session is the only session with nothing to protect.
        if !self.session.is_dirty() {
            self.drop_recovery();
        }
        self.view = View::Dashboard;
        self.library.refresh();
        self.library.reload_projects();
        // **Once per visit, not per refresh.** The sweep is a `read_dir` over the
        // cover cache, and the refreshes that happen *inside* the dashboard —
        // after every rename and delete — would each pay for it while the user
        // was clicking. Entering the screen is the moment where a few
        // milliseconds cost nothing, and a stale cover left for one session is
        // a file nobody sees.
        self.covers.sweep(&self.library.entries);
        // Chrome that belongs to the document behind the dashboard: a picker or
        // a context menu left floating would come back with it, over a screen
        // that has no idea what they refer to.
        self.picker = None;
        self.context_menu = None;
        self.open_menu = TopMenu::None;
        self.rename_entry = None;
        // ⚠️ **And the close card, which was the one piece of editor chrome
        // nothing on the way out cleared** (§15 D526). It is written `true` in
        // one place and cleared in three — the card's own Save, Discard and
        // Cancel arms — so walking to the library with the card up (`Ctrl+O`
        // under it, which is a door §15 D464 leaves open on purpose for the
        // chords it does not gate) left `confirming_close` stuck: measured
        // `true` on the dashboard, and `true` again on coming back into the
        // editor with `is_dirty()` **false**, so the *"has changes that have
        // not been saved"* card returned over a document that had none.
        //
        // Cleared rather than answered: the question was *"are you closing this
        // document?"*, and the user has stopped closing it.
        self.confirming_close = false;
    }

    /// Open a document from the library into the editor.
    ///
    /// Reports failure and stays where it is, which for the dashboard means the
    /// row stays there to be tried again — the alternative, switching to an
    /// empty editor, loses the only place the user could see what went wrong.
    pub(crate) fn open_path(&mut self, path: &std::path::Path) {
        if !self.may_replace_current_document("opening another document") {
            return;
        }
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => return self.session.fail(format!("Read failed: {e}")),
        };
        let doc = match ondin_core::io::load(&bytes) {
            Ok(d) => d,
            Err(e) => return self.session.fail(format!("Load error: {e}")),
        };
        if let Some(id) = doc.meta().id.clone() {
            // **Before `adopt_document`, because that moves the document.** The
            // Recent list is per-machine and lives in the cache, so this is the
            // one place the local index is written on an open.
            self.library.local.mark_opened(&id);
            self.library.local.save();
        }
        // ⚠️ **Before the adopt, because the key belongs to the session being
        // replaced.** Past this line the app is holding a different document and
        // has no way to name the old one's snapshot, so the file would sit in
        // `.recovery/` until the next launch asked about it. The work it held is
        // already on disk — the two branches above are a save and a confirmed
        // discard, and there is no third way to reach here.
        self.drop_recovery();
        // `adopt_document` re-seeds the id source from the loaded document;
        // without that, the next create would collide.
        self.session.adopt_document(doc, Some(path.to_path_buf()));
        self.reset_transient_state();
        // **After the reset, which is what empties `collapsed`.** Before it this
        // would fold the tree and then be undone by the line above, which is the
        // sort of ordering bug that reads as the preference not working at all.
        if self.prefs.collapse_groups_on_open {
            self.collapse_all();
        }
        self.ensure_document_fonts();
        self.last_autosave = std::time::Instant::now();
        self.view = View::Editor;
        self.prefs.last_document = Some(path.to_path_buf());
        self.prefs.save();
    }

    fn reset_transient_state(&mut self) {
        self.drag = Drag::None;
        // Holds ids minted from the old document's stream, so it cannot outlive
        // the document it was captured from.
        //
        // ⚠️ **That rule governs more fields than this list clears, and the list is
        // hand-maintained with nothing checking it.** Thirteen other fields on
        // `OndinApp` hold `NodeId`s from the outgoing document — `entered_group`,
        // `points`, `renaming_layer`, `clones`, `layers_range_anchor`,
        // `effect_menu`, `pending_picker` and the five `*_synced_selection`
        // vectors. Most are cleared elsewhere on the way through an open, and a
        // stale one **dangles rather than collides** — `NodeId` is `(actor, seq)`
        // with a random per-session actor (`ondin_core::id`), so `doc.get(id)`
        // answers `None` and the state is inert rather than wrong. That is why no
        // wrong output has ever been measured from this, and it is written down so
        // nobody re-derives it. The exception was `per_corner_radius`, cleared
        // below.
        self.alt_clone = None;
        self.pen = None;
        self.text = None;
        self.mode = Mode::Normal;
        self.name_edit = None;
        self.collapsed.clear();
        self.layer_filter = None;
        self.layer_drag = None;
        self.picker = None;
        self.paint_hex = None;
        self.guide_previews.clear();
        // Ids from the old document's stream, and the one field in this struct
        // that was cleared **nowhere else in the workspace** (§15 D447) — inserted at the
        // Appearance panel's disclosure and removed only when the same node's
        // disclosure is closed again. Without this line it accumulates for the
        // life of the process, across every document opened.
        self.per_corner_radius.clear();
    }

    /// Whether the app may put a different document in front of the user: **save
    /// what can be saved, then ask about what cannot** (§15 D768).
    ///
    /// `false` means the user declined and the caller must change nothing at all.
    ///
    /// 🚨 **This was `open_path`'s opening four lines and `recover` did not have
    /// them** — which is the whole of `[S16.3-L2-06]`. Both functions replace the
    /// in-memory document; only one of them checked first. Lifting the pair into a
    /// predicate is what makes *"a door that replaces the document"* a thing with
    /// one implementation rather than a thing two functions each remember.
    ///
    /// **The order is load-bearing.** A filed document is saved without being asked
    /// — there is nothing to decide, the work has somewhere to go — and only an
    /// *unfiled* one reaches the dialog, because for that one there is no answer but
    /// the user's. Reversing it would ask about work that could simply have been
    /// kept.
    ///
    /// ⚠️ **It is `is_dirty()` twice on purpose, not once cached**: `save_file`
    /// clears the flag, so the second read is asking *"is there anything left"*
    /// rather than repeating the first question.
    ///
    /// ⚠️ **The dialog is `rfd`, which blocks**, and over the recovery card it is a
    /// native dialog on top of an `egui::Modal` — the only place in the app that
    /// happens. Its window-close ✕ answers `No`, which is the safe arm, so the
    /// reflex gesture does not destroy anything (§15 D621's rule).
    fn may_replace_current_document(&mut self, what: &str) -> bool {
        if self.session.is_dirty() && self.session.path.is_some() {
            self.save_file(false);
        }
        !self.session.is_dirty() || self.confirm_discard(what)
    }

    /// A blocking yes/no dialog for the one destructive question the app asks
    /// outside its own chrome: discarding unsaved work to put a different
    /// document in front of the user.
    ///
    /// ⚠️ **One caller, [`Self::may_replace_current_document`]**, which is what
    /// makes the two doors behind it — `open_path` and `recover` — ask the same
    /// question in the same words (§15 D768). **Closing the window is not one of
    /// them**: that path raises an in-app `egui::Modal` through
    /// [`Self::close_confirmation`], because it has three answers rather than two
    /// and belongs in the app's own vocabulary (§15 D621).
    ///
    /// ⚠️ **This doc was sitting at the head of `may_replace_current_document`'s
    /// run**, that function having been inserted above it, and named the window
    /// close as a second user — so it was in the wrong place *and* wrong. Neither
    /// recorded detector could see it: the merged run was ~27 lines against a
    /// 52-line floor and nothing had lost a `fn`.
    fn confirm_discard(&self, what: &str) -> bool {
        rfd::MessageDialog::new()
            .set_title("Unsaved changes")
            .set_description(format!(
                "“{}” has unsaved changes. Discard them and continue {what}?",
                self.session.document_title()
            ))
            .set_buttons(rfd::MessageButtons::YesNo)
            .show()
            == rfd::MessageDialogResult::Yes
    }

    /// Intercept the window close so unsaved work is never lost silently.
    ///
    /// ⚠️ **Re-armed on every close request, not only the first.** The title bar
    /// is not covered by the in-app [`egui::Modal`] this raises, so a second
    /// click on ✕ — the ordinary reflex when a dialog appears — arrives while
    /// `confirming_close` is already `true`. Gating on that flag skipped the
    /// whole arm, and `CancelClose` has exactly one emitter in the workspace, so
    /// nothing cancelled the close and eframe took the window down with the card
    /// still up and unanswered. Cancelling again is free: the flag is idempotent
    /// and the card is already drawn.
    fn handle_close_request(&mut self, ctx: &egui::Context) {
        let requested = ctx.input(|i| i.viewport().close_requested());
        if !requested {
            return;
        }
        if self.session.is_dirty() {
            self.confirming_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
    }

    /// The in-app confirmation shown after a cancelled close.
    fn close_confirmation(&mut self, ctx: &egui::Context) {
        if !self.confirming_close {
            return;
        }
        let mut decision: Option<CloseChoice> = None;
        // 🚨 **Through `settings::card`, which is what every other modal in the
        // app goes through** (§15 D621, `[S18.1-L3-02]`). This was
        // `egui::Modal::new` with three bare `ui.button`s — the **one** card
        // outside the vocabulary, and the one that decides whether a document's
        // unsaved work is kept or thrown away.
        //
        // **What it was missing, in order of how much it cost:**
        //
        // - **The focus ring.** `ui::focus_ring`'s own doc says *"call it once at
        //   the end of a modal's body; `settings::card` does that for every card
        //   in the app so a new one cannot forget"* — and that sentence was
        //   **false**, because this card did not go through `card`. A keyboard
        //   user tabbing between the three buttons had nothing on screen saying
        //   which one Enter would press, which is the exact gap §15 D382 put the
        //   ring into the modals to close.
        // - **`FieldButton::Danger` on *Discard*.** That variant's own doc is
        //   about this: *"a confirm that read as an ordinary `FieldButton::Off`
        //   button beside Cancel — two grey buttons, one of which removes a
        //   project that has no trash to come back from."* Here the two grey
        //   buttons were *Discard* and *Cancel*, and what the first discards is
        //   the document on screen.
        // - **The card's frame and backdrop** — `color::CARD`, the `text_a(23)`
        //   hairline, the 10pt radius, the 24/60 shadow, and the
        //   `from_black_alpha(128)` backdrop whose own doc argues egui's default
        //   100 leaves *"a light document still reading as the thing in focus"*.
        //   This card got egui's default for all five.
        //
        // ⚠️ **`modal_title`'s ✕ is *Cancel*, not a fourth answer.** It is the
        // same reading `should_close` below gives Escape and the backdrop, and
        // for §15 D377's reason: a reflex gesture must not land in the arm that
        // throws work away.
        let modal = crate::settings::card("confirm-close", ctx, |ui| {
            ui.set_width(crate::ui::menu_inner_w(320.0, crate::settings::PAD));
            if crate::settings::modal_title(ui, "Unsaved changes") {
                decision = Some(CloseChoice::Cancel);
            }
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(format!(
                    "“{}” has changes that have not been saved.",
                    self.session.document_title()
                ))
                .size(12.0)
                .color(theme::text::MUTED),
            );
            ui.add_space(crate::settings::FOOTER_GAP);
            // Right-to-left, like every other card's footer: *Cancel* sits
            // furthest from the pointer's resting place and the destructive verb
            // is not the one under the thumb.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if crate::settings::text_button(ui, "Cancel", crate::ui::FieldButton::Off).clicked()
                {
                    decision = Some(CloseChoice::Cancel);
                }
                if crate::settings::text_button(ui, "Discard", crate::ui::FieldButton::Danger)
                    .clicked()
                {
                    decision = Some(CloseChoice::Discard);
                }
                if crate::settings::text_button(ui, "Save and close", crate::ui::FieldButton::On)
                    .clicked()
                {
                    decision = Some(CloseChoice::Save);
                }
            });
        });
        // ⚠️ **`Escape` and the backdrop mean *Cancel*, and reading them at all
        // is the fix** (§15 D525). This card read only its three buttons and
        // dropped the `ModalResponse` on the floor, so `Escape` over it was
        // **unhandled**: the card stayed up and the key fell through `ui`'s
        // ungated `else` to `OndinApp::escape`, which paid out a rung of the
        // editor's escape ladder underneath it. Measured — `Rect` → `Select`
        // with the card still asking, and held down it kept going: present mode
        // off, picker dismissed, text session finished, gesture cancelled,
        // points cleared, pivot off, group exited, selection cleared. The
        // control is one modal away: `Escape` over Settings closes it and leaves
        // the tool alone.
        //
        // **`Cancel` and not `Discard`**, which is §15 D377's argument arriving
        // at the second modal it applies to: the recovery card maps `Escape` to
        // *Later* deliberately, because collapsing a reflex key into the
        // destructive arm *"would delete somebody's only copy of an hour's
        // drawing because they tapped a key"*. The same reasoning, the same
        // answer — the safe arm — and here it also means the ✕ that opened the
        // card is simply called off.
        if modal.should_close() {
            decision = Some(CloseChoice::Cancel);
        }

        match decision {
            Some(CloseChoice::Save) => {
                self.save_file(false);
                self.confirming_close = false;
                // Only actually close if the save went through.
                if !self.session.is_dirty() {
                    // ⚠️ **Here rather than left to `recovery_tick`'s clean
                    // branch, because there is no next frame.** Both arms of this
                    // match end the process, so the tick that would notice the
                    // session went clean never runs — and a snapshot that outlives
                    // a *deliberate* close is a question at the next launch about
                    // work the user already dealt with.
                    self.drop_recovery();
                    // **And waited for, because the removal is queued.** The
                    // `Drop` on `library::writer::Writer` would also flush it, and
                    // saying so here as well is deliberate: this arm's whole
                    // reason for existing is that there is no next frame, and
                    // that argument should not have to reach through eframe's
                    // shutdown to hold.
                    self.disk_settle();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            Some(CloseChoice::Discard) => {
                self.confirming_close = false;
                // The user said to throw the work away, which is the one thing
                // that has to reach the snapshot too — see the arm above, for
                // the wait as well as the drop.
                self.drop_recovery();
                self.disk_settle();
                self.session.mark_saved(
                    self.session
                        .path
                        .clone()
                        .unwrap_or_else(|| std::path::PathBuf::from("Untitled.ondin")),
                );
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Some(CloseChoice::Cancel) => self.confirming_close = false,
            None => {}
        }
    }

    /// Pick image files and load the cursor with them (`Tool::Image`,
    /// `Ctrl+Shift+K`, and the rail's image button — all one action).
    ///
    /// **The tool is only armed if something loaded.** Cancelling the dialog, or
    /// picking four files none of which decode, must leave the tool where it was:
    /// arming a placement tool with nothing to place is a cursor that does nothing
    /// on click, which reads as the app having hung.
    pub(crate) fn place_image_action(&mut self) {
        let Some(paths) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif"])
            .pick_files()
        else {
            return;
        };
        let (mut loaded, mut failed) = (Vec::new(), Vec::new());
        for path in paths {
            match self.load_image_file(&path) {
                Ok(image) => loaded.push(image),
                // §15 D677 — the picker's copy of the drop's arm, and it had the
                // same lie in it.
                Err(why) => failed.push(
                    why.message(
                        &path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string()),
                    ),
                ),
            }
        }
        if !failed.is_empty() {
            self.session.fail(failed.join("; "));
        }
        if loaded.is_empty() {
            return;
        }
        let n = loaded.len();
        self.pending_images = loaded;
        self.choose_tool(Tool::Image);
        self.session.info(match n {
            1 => "Click to place the image".to_string(),
            n => format!("Click to place {n} images"),
        });
    }

    /// Pick one file and give it to the picture in `slot`, `keep_framing`
    /// deciding whether the crop travels with it.
    ///
    /// **The Asset section's first entry, and the repair the placeholder needed**
    /// (§15 D179). A picture that cannot be drawn now says so on the canvas; this
    /// is what does something about it, and it answers all three unresolvable
    /// states rather than only one — a dangling id, bytes that will not decode
    /// and a link whose file has moved are all repaired by naming a file that is
    /// there. *Relink* was the plan's answer for the third and it repoints a path
    /// instead of embedding: a narrower fix, needing linked-image loading that is
    /// not built, for a state nothing in the app can currently author. §15 D179
    /// records why the substitution was taken; *Relink…* is still owed (§9.4).
    ///
    /// **The framing is kept**, which is what §5.5a requires of *replace image*
    /// by name — the common case is swapping a stand-in for the real
    /// photograph, and re-cropping every time is the annoyance that generates a
    /// second report. It is also what makes this a *repair* rather than a fresh
    /// placement: the crop, the mode, the tile scale and the orientation are the
    /// work that survived the picture going missing.
    ///
    /// **`keep_framing: false` is *Replace and reset*, and what it resets is the
    /// framing alone** (§15 D225). The crop is normalized to the picture, so it
    /// *transfers* to a new one of different dimensions rather than breaking —
    /// which is what makes the repair possible and also the only reason a reset is
    /// wanted: a region chosen on one photograph is arbitrary on the next. The
    /// **adjustments survive**, and that is a decision rather than an oversight:
    /// they are the other card since §15 D190, they are per-pixel rather than
    /// geometric, and a row in the Framing card silently clearing a slider in the
    /// Adjustments card is exactly the surprise the split was made to prevent. The
    /// sampler survives with them, for the same reason — it is not framing either.
    ///
    /// **Written through the slot**, so a fill and a stroke — a frame's fills
    /// among them, since 2026-09-01 (§15 D400) rather than as a third slot kind —
    /// all take it by the one route every other paint control writes through —
    /// with the table entry prepended to the *same* transaction, so one undo puts
    /// back both the reference and the bytes.
    pub(crate) fn replace_image_action(
        &mut self,
        id: NodeId,
        slot: crate::panels::PaintSlot,
        keep_framing: bool,
    ) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif"])
            .pick_file()
        else {
            return;
        };
        let image = match self.load_image_file(&path) {
            Ok(image) => image,
            // §15 D677 — *Replace…*'s own copy of the same arm.
            Err(why) => {
                self.session.fail(why.message(&image_name(&path)));
                return;
            }
        };
        let Some(brush) = self.slot_brush(id, slot) else {
            return;
        };
        let known = self.session.doc.has_image(&image.id);
        let next = match &brush {
            // Keep everything but which picture it is.
            ondin_core::Brush::Image(_) if keep_framing => crate::panels::map_image(&brush, |r| {
                r.id = image.id.clone();
            }),
            // *Replace and reset*: the framing goes back to the defaults a fresh
            // placement would get, and the **adjustments come across** — see the
            // doc comment for why that asymmetry is the point rather than a
            // half-measure. Rebuilt from `ImageRef::new` rather than by clearing
            // four fields by hand, so a fifth framing field added later is reset
            // here without anyone remembering to come back.
            ondin_core::Brush::Image(_) => crate::panels::map_image(&brush, |r| {
                *r = ondin_core::ImageRef {
                    adjust: r.adjust,
                    ..ondin_core::ImageRef::new(image.id.clone())
                };
            }),
            // The slot was not showing a picture. Nothing to keep, nothing to say,
            // and nothing for `keep_framing` to decide.
            _ => ondin_core::image_brush(image.id.clone()),
        };
        let mut ops = Vec::new();
        if !known {
            ops.push(Operation::AddImage {
                id: image.id.clone(),
                entry: image.entry.clone(),
            });
        }
        ops.extend(self.slot_transaction(id, slot, next).0);
        if self.commit_edit(Transaction(ops)) {
            self.session
                .info(format!("Replaced with {}", image.label()));
        }
    }

    /// Step into image editing on `id` — select it, take the tool, say what the
    /// mode expects.
    ///
    /// **Four doors call this and they all say the same sentence**, which is why it
    /// is a function: `Enter` on a selected picture (`app.rs`), a double-click on
    /// one (`canvas.rs`), the *Edit image* context-menu row (`menu.rs`, §15 D268)
    /// and — since 2026-08-23 — clicking a fill row's swatch or its *Image* label
    /// in the inspector. The status line was copied at each of the first three with
    /// a comment saying that if it ever earned a constant all of them wanted it at
    /// once; the fourth is what earned it.
    ///
    /// **`set_one` even where the caller has already selected the layer.** The
    /// `Enter` door has, by construction — it reads the selection to find `id` —
    /// and the redundant write costs nothing, where four doors that each decide
    /// selection differently is how they come to disagree.
    ///
    /// **A lock refuses, and this is the only place that checks it** (§15 D320).
    /// This paragraph said "no lock check, matching every other door" until
    /// 2026-08-23, when the question `roadmap.md` held was answered the other way:
    /// a crop is a resize, so the lock reaches it. Because the refusal is *here*
    /// rather than at a door, no door disagrees with another and *Edit image* may
    /// dim — see the body for why that order is the whole of it.
    pub(crate) fn begin_image_edit(&mut self, id: NodeId) {
        // **A lock refuses the mode, and it is answered here because this is the one
        // place all four doors pass through** — `Enter`'s arm, the canvas
        // double-click, the *Edit image* menu row and the inspector's fill-row click
        // (§15 D320, which reverses D268's verdict on this one row; the funnel is
        // D305's).
        //
        // **A crop is a resize.** `crop_resize_tx` writes through
        // `tools::resize_layer`, which is the very write *Original size* has been
        // dimmed on a locked layer for since it was built — the assertion that pins
        // it says "resizing the node is a write and the lock reaches it". Those two
        // rows sit next to each other in one menu group and disagreed about the same
        // verb; this is the half that was wrong. `EditPoints`, the other mode that
        // edits a layer's geometry from a menu row, dims one line below it.
        //
        // **Refused in the mode rather than at a door**, which is what keeps §15
        // D261's rule rather than breaking it: a menu that refuses where `Enter`
        // allows is the failure, so the row may dim only *because* the mode refuses,
        // never instead of it.
        //
        // **`is_effectively_locked`, so a locked *group* locks the picture inside
        // it** (§15 D321). This read the node's own flag for one day, on the grounds
        // that a group's lock was a wider question nothing in core answered; the
        // maintainer answered it in one sentence — *"a locked group means everything
        // inside is also locked, so it's an expected behavior, not a good to have"* —
        // and core answers it now.
        if ondin_core::is_effectively_locked(&self.session.doc, id) {
            self.session
                .info("This layer is locked — unlock it to edit the picture");
            return;
        }
        self.session.selection.set_one(id);
        self.choose_tool(Tool::ImageEdit);
        self.session.info(
            "Editing the picture: drag a handle to crop, the picture to move it; Enter or \
             Escape when done",
        );
    }

    // **`export_image_action` was here and went on 2026-08-23** with the *Export
    // original…* rows in both its doors (§15 D225 built it). It wrote a picture's
    // **stored bytes** back out — the encoded original, byte-identical to the file
    // that went in, which is the whole point of a document that stores originals
    // rather than decoded buffers. A real capability, and one nobody reaches for:
    // "export this picture" means the layer at a size and a format, which is what
    // the Export panel is.
    //
    // ⚠️ **It took `ImageFormat::extension`'s only caller with it**, which that
    // function had been written for and had waited for. `ondin-core` is a library,
    // so nothing warns.

    /// Read, hash and decode one file, or say why not (§15 D677).
    ///
    /// ⚠️ **It answered `None` until D677**, and its own doc argued that this was
    /// the same answer as *"the file is not there"* — *"both mean nothing can be
    /// placed, and the caller names the file either way"*. That is true of a file
    /// that will not read and false of a picture refused for a renderer bound, and
    /// it is the sentence that kept a 34 KB panorama being called unreadable. The
    /// two that really are the same answer still are; see [`ImageRefusal`].
    fn load_image_file(&mut self, path: &std::path::Path) -> Result<LoadedImage, ImageRefusal> {
        // A file that will not read off disk and a file that is not a picture get
        // the same sentence — *"Could not read: name"* — and it is the true one for
        // both. The refusals worth distinguishing are the ones where the bytes are
        // fine (§15 D677).
        let Ok(bytes) = std::fs::read(path) else {
            return Err(ImageRefusal::NotAnImage);
        };
        self.load_image_bytes(bytes, file_extension(path), Some(image_name(path)))
    }

    /// [`Self::load_image_file`] without the file: hash, sniff, decode and name
    /// bytes that are already in hand.
    ///
    /// **The seam the other two ways in are built on** (§5.5a). A drop usually
    /// carries a path and sometimes only bytes; a paste never carries a path at
    /// all. Everything after "here are the bytes and what to call them" is the
    /// same for all three, and keeping it one function is what stops a drop and a
    /// pick disagreeing about the hash, the format sniffing or the intrinsic size.
    ///
    /// `ext` is the extension's second opinion for a byte run whose magic number
    /// says nothing; `None` where there is no filename to ask, which is a paste.
    fn load_image_bytes(
        &mut self,
        bytes: Vec<u8>,
        ext: Option<String>,
        name: Option<String>,
    ) -> Result<LoadedImage, ImageRefusal> {
        let Some(format) = image_format_of(ext.as_deref(), &bytes) else {
            return Err(ImageRefusal::NotAnImage);
        };
        let id = content_hash(&bytes);
        // **Decoded here rather than at placement**, for two reasons that are one:
        // it is what supplies the intrinsic size the fit rule needs, and it is the
        // only way to know the file is drawable *before* the tool is armed with it.
        let (w, h) = self
            .canvas
            .images_mut()
            .insert(&id, &bytes, format)
            .map_err(ImageRefusal::Decode)?;
        Ok(LoadedImage {
            entry: ImageEntry {
                // The one move into the table, and the last copy these bytes make:
                // from here on `Document::apply`'s working clone shares them rather
                // than duplicating them per commit (§15 D301).
                source: ImageSource::Embedded(bytes.into()),
                format,
                width: w,
                height: h,
            },
            id,
            intrinsic: ondin_core::kurbo::Size::new(f64::from(w), f64::from(h)),
            name,
        })
    }

    /// Take whatever was dropped on the window this frame and place it.
    ///
    /// **Read from `raw.dropped_files`, which nothing read before** — the one
    /// piece of the drop scope that had no machinery at all (§15 D183). egui hands
    /// over a `DroppedFile` per file with a `path` (a real file manager drop) or
    /// `bytes` (a drag out of a browser), so both are tried in that order.
    ///
    /// **The drop point is the pointer where there is one, and on Windows no
    /// event carries it** — neither the drop nor anything else the OS sends
    /// during a file drag, so [`Self::drop_point`] asks the desktop cursor
    /// instead (§15 D184, D241). That is why it is one function rather than four
    /// lines here: the aim, the replace gate and the announcement have to be the
    /// same answer. [`Self::draw_drop_indicator`] is what says which of the two
    /// will happen while there is still a drag to abandon.
    pub(crate) fn take_dropped_images(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        let at = self.drop_point(ctx);
        let (mut loaded, mut failed, mut vector) = (Vec::new(), Vec::new(), 0_usize);
        for file in dropped {
            let name = file
                .path
                .as_deref()
                .map(image_name)
                .unwrap_or_else(|| file.name.clone());
            let bytes = match (&file.bytes, &file.path) {
                (Some(b), _) => Some(b.to_vec()),
                (None, Some(p)) => std::fs::read(p).ok(),
                (None, None) => None,
            };
            let ext = file.path.as_deref().and_then(file_extension);
            match bytes {
                // **Vector art is refused, never rasterized**, and it is counted
                // separately so the message can say which of the two things went
                // wrong — "could not read" would be a lie about a file that reads
                // perfectly well and is simply not an image.
                Some(b) if looks_like_svg(&b) => vector += 1,
                Some(b) => match self.load_image_bytes(b, ext, Some(name.clone())) {
                    Ok(image) => loaded.push(image),
                    // 🚨 **The refusal is carried, not flattened to a name** (§15
                    // D677, `[S9.2-L1-04]`). This arm used to push the name onto a
                    // list that could only ever say *"Could not read"*, which is
                    // the very lie the SVG arm above is written to avoid — a
                    // 70000 × 100 panorama is 34 KB of perfectly good PNG, and
                    // being told it is unreadable sends its owner to re-export a
                    // file that is not the problem.
                    Err(why) => failed.push(why.message(&name)),
                },
                // No bytes at all: the path would not read and egui carried none.
                // There is nothing to be more specific about.
                None => failed.push(ImageRefusal::NotAnImage.message(&name)),
            }
        }
        if vector > 0 {
            self.session.fail(
                "SVG is vector art, not an image — dropping one will create layers \
                 once the parser is built"
                    .to_string(),
            );
        }
        // One line per file rather than one list of names, because the whole point
        // is that two files can now fail for two different reasons in one drop.
        if !failed.is_empty() {
            self.session.fail(failed.join("; "));
        }
        if loaded.is_empty() {
            return;
        }
        self.drop_images_at(loaded, at);
    }

    /// Place dropped images, or hand one to the shape under the pointer.
    ///
    /// **Onto a shape it replaces that shape's picture; onto bare canvas it makes
    /// a layer** (§5.5a). The first is the *Replace image* operation the
    /// Asset section will also want, so it keeps the framing — dropping a real
    /// photograph onto the placeholder you cropped to fit is the case that rule
    /// exists for, and losing the crop every time is what generates round two.
    ///
    /// **The cascade lives here and nowhere else.** A click-placed image needs
    /// none — each one is placed by its own click, at its own point — but N files
    /// arrive at *one* point, so without an offset they stack exactly and the top
    /// one looks like the only one. This is the offset that was written for the
    /// picker, found to have no caller there, and deleted (§15 D177).
    fn drop_images_at(&mut self, loaded: Vec<LoadedImage>, at: Option<Point>) {
        /// World units between one dropped layer and the next, in both
        /// directions. Deliberately **not** scaled by zoom: it is a nudge to stop
        /// N pictures hiding behind one another, and at 400% a screen-constant
        /// step would put the second one a quarter of the way across the document.
        const CASCADE: f64 = 24.0;
        let n = loaded.len();
        // A single picture over a selected shape replaces its fill rather than
        // landing on top of it. Only one: "replace" means *this* picture, and
        // four files replacing one fill has no sensible reading.
        //
        // **And only where the drop actually landed on it** — `Some(p)`, never
        // the paste's `None`. The rule is "dropped *onto* the shape", and a drop
        // with no point reaching `image_drop_target`'s selection-only arm turned
        // that into "anything selected, wherever you let go". That was not a near
        // miss but the common flow, because until §15 D241 gave
        // [`Self::drop_point`] a position on Windows, *every* Windows drop had
        // none — and `create_image_at` selects what it places, so the second
        // picture dropped into a document replaced the first. A paste
        // keeps the selection-only arm because a paste *is* aimed by the
        // selection — pressing the keys with a shape selected is the act of aim,
        // where a drag's aim is the pointer and there is none.
        if n == 1
            && let Some(p) = at
            && let Some(target) = self.image_drop_target(Some(p))
        {
            let image = &loaded[0];
            if self.replace_image_fill(target, image) {
                self.session
                    .info(format!("Replaced with {}", image.label()));
                return;
            }
        }
        // No point to place at: dropped over a panel or outside the canvas, or —
        // on Windows — the desktop cursor could not be read at all
        // ([`Self::drop_point`], §15 D241). The middle of the view is the honest
        // answer, and refusing would throw the files away.
        let p = at.unwrap_or_else(|| self.viewport_centre());
        // **Placed from the local list, never through `pending_images`.** That
        // field is the Image tool's loaded cursor, and borrowing it here threw
        // away whatever the tool was still holding — a drop is not a pick.
        //
        // Placed now rather than by arming the cursor: a drop already said where
        // it wants to go, so asking for a click as well would be asking twice.
        //
        // **One drop is one transaction, however many files it carries** (§15
        // D522). This used to call `create_image_at` per file, which commits its
        // own, so a three-file drop left three entries in the history: one
        // `Ctrl+Z` removed the last picture and left the other two, and only the
        // last was selected, so there was not even a selection to delete the rest
        // with. The document is advanced as each placement is built —
        // `image_tx`'s `over` — because the index, the free name and "is this
        // blob already embedded" are all read off the tree, and three files
        // built against the untouched document would claim one index and one
        // name three times over.
        let mut advanced = self.session.doc.clone();
        let mut ops = Vec::new();
        let mut made = Vec::new();
        for (i, image) in loaded.iter().enumerate() {
            let step = CASCADE * i as f64;
            let (tx, node) = self.image_tx(Some(&advanced), image, p + Vec2::new(step, step));
            if advanced.apply(&tx).is_ok() {
                ops.extend(tx.0);
                made.push(node);
            }
        }
        let placed = made.len();
        if placed == 0 || !self.session.commit(Transaction(ops)) {
            self.session.fail("Could not place the images");
            return;
        }
        // Every picture the drop made, not the last one — which is also what
        // gives the user something to press Delete on.
        self.session.selection.set(made);
        self.session.info(match placed {
            1 => "Placed image".to_string(),
            n => format!("Placed {n} images"),
        });
    }

    /// Put `image` into the fill `target` names, keeping how it was framed.
    ///
    /// Returns whether there was anything to replace. The framing is carried
    /// because that is what §5.5a requires of *replace image* by name: the
    /// common case is swapping a stand-in for the real photograph, and re-cropping
    /// every time is the annoyance that generates a second report.
    fn replace_image_fill(&mut self, target: (NodeId, usize), image: &LoadedImage) -> bool {
        let (id, index) = target;
        let Some(node) = self.session.doc.get(id) else {
            return false;
        };
        let mut fills = node.paint().fills.clone();
        let Some(fill) = fills.get_mut(index) else {
            return false;
        };
        let known = self.session.doc.has_image(&image.id);
        fill.brush = match &fill.brush {
            // Keep the crop, the mode, the tile scale, the orientation and the
            // sampler's alpha — everything but which picture it is.
            ondin_core::Brush::Image(_) => crate::panels::map_image(&fill.brush, |r| {
                r.id = image.id.clone();
            }),
            // A shape that was not showing a picture becomes one that is, at the
            // defaults. There is no framing to keep, so there is nothing to say.
            _ => ondin_core::image_brush(image.id.clone()),
        };
        let mut ops = Vec::new();
        if !known {
            ops.push(Operation::AddImage {
                id: image.id.clone(),
                entry: image.entry.clone(),
            });
        }
        ops.push(Operation::SetFills { id, fills });
        self.commit_edit(Transaction(ops))
    }

    /// Paste an image off the OS clipboard, if there is one.
    ///
    /// Returns whether it took the paste, so the caller can fall through to the
    /// in-app layer paste when the clipboard holds no picture.
    ///
    /// **`at` is *Paste here*'s pointer, and it changes both halves** (§15 D224).
    /// `None` is the chord's: no point to place at, so the fill it replaces is
    /// decided by the selection alone and a new layer lands in the middle of the
    /// view. `Some(p)` is the menu's, and it is aimed the way a **drop** is rather
    /// than the way the chord is — the fill has to be a selected shape *under the
    /// point*, and a new layer lands at the point. The asymmetry is the one
    /// [`Self::image_drop_target`] already states, and this is a third gesture
    /// arriving on the drop's side of it: right-clicking bare canvas and choosing
    /// *paste **here*** cannot sensibly mean "replace the picture over there".
    ///
    /// **The clipboard is read directly, because `egui::Event::Paste` carries a
    /// `String` and has no image variant** — the plan's own trap, and the
    /// reason `arboard` is a direct dependency (§15 D183).
    ///
    /// **What comes back is *decoded* RGBA, so it is re-encoded to PNG here**, and
    /// that is not a detail: the whole format rests on storing the encoded
    /// original and hashing *that* (§5.5a), and a clipboard image has no original
    /// to store. PNG because it is lossless and one of the four v1 accepts — a
    /// screenshot re-encoded as JPEG on the way in would be a quality loss nobody
    /// asked for. The consequence worth knowing: the id of a pasted image is the
    /// hash of *our* encoding, so the same screenshot pasted twice dedupes, while
    /// the same picture pasted and then placed from a file does not.
    pub(crate) fn paste_image(&mut self, at: Option<Point>) -> bool {
        // **The whole read happens inside the gate**, not just the open (§15
        // D796): `get_image` is what actually touches the global clipboard, so
        // taking the handle under the lock and reading outside it would leave the
        // race exactly where it was.
        if !clipboard_is_reachable() {
            return false;
        }
        let Some(Ok(img)) = with_clipboard(|c| c.get_image()) else {
            return false;
        };
        let (w, h) = (img.width as u32, img.height as u32);
        if w == 0 || h == 0 {
            return false;
        }
        let mut png = Vec::new();
        if image::write_buffer_with_format(
            &mut std::io::Cursor::new(&mut png),
            &img.bytes,
            w,
            h,
            image::ExtendedColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .is_err()
        {
            return false;
        }
        // No message: this is the app's own freshly encoded PNG, so a refusal here
        // is a bug rather than something to explain to the user (§15 D677).
        let Ok(image) = self.load_image_bytes(png, None, None) else {
            return false;
        };
        // **The chord's target is the selection, because a chord has no pointer**
        // — dropping addresses a shape by landing on it, where pressing the keys
        // can only mean "the thing I have selected". *Paste here* has a pointer and
        // is therefore aimed like the drop: `Some(p)` makes `image_drop_target`
        // demand the point be *on* the selected shape, so the two aims have to
        // agree before anything is overwritten. See this function's own docs.
        if let Some(target) = self.image_drop_target(at)
            && self.replace_image_fill(target, &image)
        {
            self.session
                .info(format!("Replaced with {}", image.label()));
            return true;
        }
        let landed = self.create_image_at(&image, at.unwrap_or_else(|| self.viewport_centre()));
        if landed {
            self.session.info("Pasted image");
        }
        landed
    }

    /// Load every font family the document's text nodes reference, so an opened
    /// file renders with its real fonts rather than the Inter fallback.
    fn ensure_document_fonts(&mut self) {
        let families: Vec<String> = self.document_families().into_iter().collect();
        for family in families {
            self.fonts.ensure_loaded(&family);
        }
    }

    /// Every family the document's text nodes name, **lowercased** (§15 D591).
    ///
    /// One walk for two callers: [`Self::ensure_document_fonts`], which asks the
    /// service for each of them, and the font-poll gate in `OndinApp::ui`, which
    /// asks whether an arriving face is one of them before invalidating the
    /// shaped-text cache. (Plain backticks for that one: it is in a different
    /// `impl` block and `cargo doc --document-private-items` cannot resolve
    /// `Self::ui` from here — the gate said so, which is what it is for.)
    ///
    /// ⚠️ **Lowercased, which changes what `ensure_loaded` is handed.** That is
    /// safe and was checked: `FontService::keys` is already the lowercased family
    /// list, `faces_of` resolves case-insensitively through it, and core's own
    /// `is_family_available` does the same — a family name's case is not part of
    /// its identity anywhere in this path. Keeping two forms would mean the gate
    /// and the loader could disagree about the same family, which is the failure
    /// this whole finding is a version of.
    ///
    /// 🚨 **The *spans* count, not only the node's default.**
    /// `panels::typography::type_family_row` writes `CharAttr::Family` into the
    /// character spans whenever the range is partial — applying a family to a
    /// selection rather than to the layer — so a document can name a family that
    /// appears in no `TextStyle::font_family` anywhere. Reading the defaults alone
    /// made the gate above decline to invalidate for exactly those, and that run
    /// would have gone on drawing in the fallback face until something else
    /// touched the node. The coarse `fonts_registered` flag covered it by
    /// accident, so the narrowing is what made it reachable; `arch-scribe` caught
    /// it reading this walk against `apply_char_attrs`. `ensure_document_fonts`
    /// had the same gap on the *loading* side and is fixed by the same union.
    fn document_families(&self) -> std::collections::HashSet<String> {
        let mut families = std::collections::HashSet::new();
        let mut stack = vec![self.session.doc.root()];
        while let Some(id) = stack.pop() {
            let Some(node) = self.session.doc.get(id) else {
                continue;
            };
            if let NodeKind::Text { style, spans, .. } = node.kind() {
                families.insert(style.font_family.to_lowercase());
                for span in spans.as_slice() {
                    if let ondin_core::CharAttr::Family(name) = &span.attr {
                        families.insert(name.to_lowercase());
                    }
                }
            }
            stack.extend(node.children().iter().copied());
        }
        families
    }
}

/// A path's extension, lowercased later by [`image_format_of`].
fn file_extension(path: &std::path::Path) -> Option<String> {
    path.extension().and_then(|e| e.to_str()).map(str::to_owned)
}

/// What a layer made from this file is called — its own name, because an auto
/// "Rectangle 4" throws away the one piece of identity an image arrives with.
fn image_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Image".to_string())
}

/// Whether a byte run is SVG, so a drop of vector art can be refused rather than
/// rasterized.
///
/// **Refusing is the decision** (§5.5a): rasterizing vector art in a vector tool
/// is the wrong default, and doing it silently is worse than saying no.
///
/// Sniffed from the bytes rather than the name, because a drop from a browser
/// often has neither an extension nor a filename worth trusting.
///
/// 🚨 **The question itself belongs to `svg_in` and this is the byte-to-`str`
/// adapter over it** (§15 D671, `[S16.3-L3-04]`). It used to be a second
/// implementation, and the two disagreed about the commonest shape there is: a
/// file opening `<!DOCTYPE svg PUBLIC …>` was *"Could not read: logo.svg"* on a
/// drop and imported as layers on a paste of the same bytes. The lie is the one
/// the arm at the call site exists to avoid, in its own words — *"'could not read'
/// would be a lie about a file that reads perfectly well and is simply not an
/// image"*.
///
/// The 512-byte cap and the lossy decode are this side's own and are why the
/// adapter exists at all: a root element past 512 bytes of prolog and comments
/// reads as not-SVG, which is the pre-existing behaviour and is well beyond
/// anything a generator writes. `from_utf8_lossy` rather than a `from_utf8` that
/// gives up, because the cap can split a multi-byte character and did.
fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(512)];
    ondin_core::svg_in::looks_like_svg(&String::from_utf8_lossy(head))
}

/// Why a byte run did not become a placeable picture (§15 D677, `[S9.2-L1-04]`).
///
/// 🚨 **`ondin_render::images::DecodeError` had no reader anywhere in the
/// workspace**, so a picture refused for a *renderer limit* was announced as
/// **"Could not read"** — about a file that reads perfectly well. That is the same
/// lie the SVG arm of `place_image_action` is written to avoid, in the words two
/// lines above it, and `DecodeError`'s own doc states the intent nobody took up:
/// *"Why an image could not be decoded, for the caller that wants to say so."*
/// `dead_code` never fires on a `pub` item in a library crate, so nothing said.
///
/// The two variants are the two questions a user is actually asking. `NotAnImage`
/// is *this is not a picture*; `Decode` is *it is a picture and Ondin will not draw
/// it*, and `DecodeError`'s own three variants then say which of the three reasons.
#[derive(Debug)]
pub(crate) enum ImageRefusal {
    /// Neither the magic bytes nor the extension name a format this app reads.
    NotAnImage,
    /// A format this app reads, refused by the decoder or by a renderer bound.
    Decode(ondin_render::images::DecodeError),
}

impl ImageRefusal {
    /// What to tell the user about `name`, as one sentence.
    ///
    /// ⚠️ **The numbers are in it deliberately.** *"Too large"* sends somebody back
    /// to a file to guess; *"70000 × 100, and the largest Ondin can draw is 65535 on
    /// a side"* tells them what to change and by how much. `TooMuchPixelData` is a
    /// different bound with a different remedy, which is why `DecodeError` keeps
    /// them apart and why this does too.
    fn message(&self, name: &str) -> String {
        use ondin_render::images::{DecodeError, MAX_DECODED_BYTES, MAX_EDGE};
        match self {
            Self::NotAnImage | Self::Decode(DecodeError::Corrupt) => {
                format!("Could not read: {name}")
            }
            Self::Decode(DecodeError::TooLarge { width, height }) => format!(
                "{name} is {width} × {height}; the largest Ondin can draw is {MAX_EDGE} on a side"
            ),
            Self::Decode(DecodeError::TooMuchPixelData {
                width,
                height,
                bytes,
            }) => format!(
                "{name} is {width} × {height}, which is {} of pixels; the limit is {}",
                crate::settings::human_bytes(*bytes),
                crate::settings::human_bytes(MAX_DECODED_BYTES)
            ),
        }
    }
}

/// One image picked and decoded, waiting to be placed (`OndinApp::pending_images`).
///
/// `Clone` so the tool can hand one to `create_image_at` without holding a borrow
/// on the queue it is about to pop. It is one entry and one copy of the encoded
/// bytes — a few hundred KB once per placement, never per frame.
#[derive(Clone)]
pub(crate) struct LoadedImage {
    pub(crate) id: ImageId,
    pub(crate) entry: ImageEntry,
    /// The decoded pixel size, which is what the fit rule measures.
    pub(crate) intrinsic: ondin_core::kurbo::Size,
    /// The file's own name, which becomes the layer's — an auto "Rectangle 4"
    /// would throw away the one piece of identity an image arrives with.
    ///
    /// **[`None`] when the image did not arrive with one**, which is every paste:
    /// the clipboard hands over pixels and nothing else. It used to be
    /// `"Pasted image"` for those, and a document with three of them had three
    /// layers of that name — a label describing *how it got here* rather than
    /// what it is, and not even a unique one. `None` means "number it like any
    /// other new layer", which [`OndinApp::create_image_at`] does once it knows
    /// the siblings.
    pub(crate) name: Option<String>,
}

impl LoadedImage {
    /// What a status message calls this picture.
    ///
    /// Separate from the *layer* name on purpose: a layer with no name of its own
    /// is numbered against its siblings (`canvas::image_layer_name`),
    /// which a one-line message has no business doing — "Replaced with Image 7"
    /// would name a layer that does not exist.
    pub(crate) fn label(&self) -> &str {
        self.name.as_deref().unwrap_or("the pasted image")
    }
}

/// The document-wide id for these bytes: `sha256:<hex>` of the **encoded**
/// original.
///
/// **Encoded, never decoded** (§5.5a): decoders are not bit-exact across
/// versions, so hashing the decode would make a file's identity depend on a
/// library upgrade — the same photograph would dedupe against itself today and
/// not next year.
///
/// **SHA-256 through `ring`, which is already in this crate's tree** via
/// `ureq`/`rustls`, so this adds an edge and no compilation — the same reasoning
/// §3 records for `skrifa` and `read-fonts` in core. A standard digest rather than
/// a fast non-cryptographic one because the id goes *in the file*: `sha256sum` on
/// an exported original reproduces it, which is worth more here than the
/// microseconds. The cost is real and bounded — one pass over the bytes when a
/// file is picked, never per frame.
pub(crate) fn content_hash(bytes: &[u8]) -> ImageId {
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    let mut hex = String::with_capacity(7 + 64);
    hex.push_str("sha256:");
    for b in digest.as_ref() {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
    }
    ImageId(hex)
}

/// Which of the four formats a picked file is.
///
/// **The bytes decide and the extension is the fallback**, not the other way
/// round: a JPEG saved as `.png` is common enough that refusing it would read as a
/// bug, and `image`'s own sniffing is what the decoder will use anyway. The
/// extension only answers for a file whose magic number says nothing.
fn image_format_of(ext: Option<&str>, bytes: &[u8]) -> Option<ondin_core::ImageFormat> {
    use ondin_core::ImageFormat as F;
    let sniffed = image::guess_format(bytes).ok();
    let by_ext = || match ext.map(str::to_ascii_lowercase).as_deref() {
        Some("png") => Some(F::Png),
        Some("jpg" | "jpeg") => Some(F::Jpeg),
        Some("webp") => Some(F::Webp),
        Some("gif") => Some(F::Gif),
        _ => None,
    };
    match sniffed {
        Some(image::ImageFormat::Png) => Some(F::Png),
        Some(image::ImageFormat::Jpeg) => Some(F::Jpeg),
        Some(image::ImageFormat::WebP) => Some(F::Webp),
        Some(image::ImageFormat::Gif) => Some(F::Gif),
        // Sniffed as something real that v1 does not accept — a TIFF, a BMP — or
        // sniffed as nothing at all. The extension gets the second word, and a
        // wrong guess costs a decode failure rather than a wrong picture.
        _ => by_ext(),
    }
}

enum CloseChoice {
    Save,
    Discard,
    Cancel,
}

/// What the user did with a crash snapshot (`OndinApp::recovery_modal`).
///
/// ⚠️ **Three answers where the card shows two buttons**, and the third is the
/// one that matters: `Later` is what the ✕, `Escape` and the backdrop mean, and
/// it must never collapse into `Discard`. Those two dismissals are the ones a
/// hand reaches for without reading the card, and this is the only modal in the
/// app where that reflex could destroy work — so the two are spelled as separate
/// variants rather than as a `bool` with a comment, which is what the first
/// version was.
enum RecoverChoice {
    /// Load the snapshot into the session. The file on disk is not written.
    Recover,
    /// Delete the snapshot. The only thing that does.
    Discard,
    /// Stop asking this launch. The snapshot stays and is offered again next
    /// time.
    Later,
}

/// The save pill's text. `None` means there is nothing on disk matching the
/// current state — either unsaved edits or a document never written at all.
///
/// ⚠️ **`saving` is a third *label*, not a third state of the pill** (§15 D393).
/// A write in flight is still a document that differs from its file, so `ago` is
/// `None` underneath it and the dot stays amber; what the user gets is the
/// difference between *nothing is happening about this* and *something is*. The
/// dot is deliberately left alone: it answers "does the file match", which has
/// not changed, and a third colour would be claiming otherwise.
fn save_label(saving: bool, ago: Option<std::time::Duration>) -> String {
    if saving {
        return "Saving…".into();
    }
    let Some(ago) = ago else {
        return "Unsaved".into();
    };
    let secs = ago.as_secs();
    match secs {
        0..=44 => "Saved · just now".into(),
        // 🚨 **The hours band opens at 45 minutes, and it opened at 90** (§15 D678,
        // `[S16.3-L1-05]`). Every band here rounds half-up, so its opening prints
        // `1` exactly when that opening lies in `[unit/2, 3·unit/2)`. 5400 is
        // `3 × 3600 / 2` — **one second outside** — so the first value
        // `(secs + 1800) / 3600` could produce was **2**, the string
        // *"Saved · 1h ago"* was unreachable for every input, and an hour-old save
        // read **"Saved · 60m ago"** before jumping to `2h`. Measured both ways.
        //
        // ⚠️ **The days band was already right, and it is the reason 2700 is the
        // fix rather than a taste**: it opens at 79200, which is inside
        // `[43200, 129600)`. The rule the file already keeps is *"a band opens where
        // it can first say 1"*, and the hours band was the one that did not.
        //
        // ⚠️ **Two things this comment said and got wrong**, both corrected by
        // `arch-scribe` reading it against the arithmetic. It is *not* "0.75 of the
        // unit" — that fits 45 s and 45 min and not 22 h — and 22 h is not "where
        // half-up first gives one day", which is 12 h. The interval above is the
        // property that actually holds for all three bands. And the cost is
        // `45m`…**`90m`** reading `1h`, not `45m`…`59m`: the old minutes band ran to
        // 5399 and printed up to *"90m ago"*.
        45..=2699 => format!("Saved · {}m ago", (secs + 30) / 60),
        2700..=79199 => format!("Saved · {}h ago", (secs + 1800) / 3600),
        _ => format!("Saved · {}d ago", (secs + 43200) / 86400),
    }
}

// --- chrome ---------------------------------------------------------------

/// Height of the top bar. The floating inspector is positioned in screen
/// coordinates, so it needs to know where the chrome ends.
const TOP_BAR_H: f32 = 46.0;

/// Inner padding of every top-bar dropdown (`design/Editor.dc.html`: `padding:5px`).
/// One value rather than a constant per menu, because the three are the same
/// panel with different rows in it.
const MENU_PAD: f32 = 5.0;

/// The gap between every control in the top bar's right-hand cluster — undo,
/// redo, the two rules, the View and Snap heads, the zoom readout and Settings.
///
/// One value for all of them, including a mark and its own caret: 5pt is what
/// the zoom readout always used before its caret, and the icon heads asking for
/// less made the cluster's gaps unequal in the one place the eye compares them
/// (§15). The rules get [`TOP_RULE_GAP`] on top of this.
const TOP_GAP: f32 = 5.0;

/// Extra space on each side of a rule, over and above [`TOP_GAP`].
///
/// A rule is 1px of ink where every other control is a 26px box, so the same
/// gap that separates two boxes crowds it: `TOP_GAP` alone reads as the rule
/// being stuck to whichever icon is nearer. 3pt more on both sides is what
/// makes undo/redo sit off the divider by the same amount the eye reads between
/// undo and redo themselves.
const TOP_RULE_GAP: f32 = 3.0;

/// One of the top bar's dividers, with [`TOP_RULE_GAP`] to spare on each side.
///
/// Both sides here rather than at the call sites: the cluster is laid out
/// `right_to_left`, so "before" and "after" are mirrored, and a rule padded by
/// hand ends up with the 3pt on the wrong side of one of the two.
fn top_rule(ui: &mut egui::Ui) {
    ui.add_space(TOP_RULE_GAP);
    crate::ui::rule(ui, 16.0);
    ui.add_space(TOP_RULE_GAP);
}

/// The last status message, in the colour its kind calls for (§15 D426).
///
/// ⚠️ **A free function because there are two top bars, and for a long time only
/// one of them drew this.** `EditorSession::status` is a single slot with a
/// single production reader — `OndinApp::status_text`, called from the editor's
/// top bar, which sits *below* `OndinApp::ui`'s `View::Dashboard` return. So the
/// **22** messages `panels::dashboard` writes were never painted: a *Delete
/// project* that could not read `projects.json` closed its confirmation, left
/// the project in the sidebar and said nothing; a *Delete files too* that failed
/// to trash some members removed the row and left those documents on disk
/// carrying a project id nothing lists, silently. The message was not even
/// cleared — it surfaced later, out of context, in the editor's top bar the next
/// time a document was opened.
///
/// **One function rather than two spellings**, so the two screens cannot come to
/// disagree about what a failure looks like. `max_w` is the only difference
/// between them: the editor's bar gives it whatever it needs, and the library's
/// has a search field painted across the middle of the same 46pt strip that a
/// long message would otherwise run under.
///
/// `truncate` rather than a wrap, because the bar is one line tall: a wrapped
/// message would be clipped mid-sentence with nothing to say it had been, and
/// an ellipsis says so.
///
/// 🚨 **The library draws [`status_dot`] instead, and the two screens differing
/// is the decision** (§15 D751). This function took a `max_w` for the library's
/// sake — *"the library's bar has a search field painted across the middle of the
/// same 46pt strip that a long message would otherwise run under"* — and that
/// bound is **half the leftover room less the gear**, which is a function of the
/// window: 382pt at the default 1200, 22pt at 500, and **zero at 436**. There is
/// no `with_min_inner_size` on the viewport, so all three are reachable by
/// dragging an edge. A message that disappears when the window gets small is
/// worst exactly where present mode says the user is working — on a laptop
/// screen. The parameter is gone with the caller that needed it.
pub(crate) fn status_label(ui: &mut egui::Ui, status: &crate::session::Status) {
    if status.text.is_empty() {
        return;
    }
    ui.scope(|ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new(&status.text)
                    .size(11.5)
                    .color(status_color(status)),
            )
            .truncate(),
        );
    });
}

/// The colour a status is drawn in, for whichever of the two surfaces draws it.
///
/// **One statement, because the surfaces are deliberately different and their
/// *colour* must not be** (§15 D751). The library shows a dot and the editor
/// shows the sentence; what a failure looks like is the thing both screens have
/// to agree about, and it was a local `if` inside `status_label` until the second
/// surface existed to disagree with it.
fn status_color(status: &crate::session::Status) -> egui::Color32 {
    if status.kind == StatusKind::Error {
        egui::Color32::from_rgb(226, 138, 138)
    } else {
        theme::text::FAINT
    }
}

/// The library's half of the status line: a dot the width of nothing, with the
/// whole message on hover (§15 D751, `[S16.3-L1-03]`).
///
/// **A painted circle rather than an icon-font glyph**, and that is a measurement
/// rather than a preference: `Phosphor.ttf`'s `post` table is version 3.0, so it
/// carries **no glyph names**, and a wrong codepoint lays out perfectly and
/// renders nothing at all. There is no warning or info picture among the 143
/// constants in `theme::icon` today, and identifying a new one means dumping the
/// atlas and reading it by eye. A dot needs none of that and cannot come out
/// blank.
///
/// **It is the fix for a width, not for a wording.** The library *did* draw the
/// message — §15 D426 gave it a status line, closing `[S20.1-L1-01]`, and
/// `[S16.3-L1-03]`'s premise that the screen draws nothing was already stale when
/// it was ranked. What it drew was bounded by the room left beside the search
/// field, so the message shrank with the window and vanished below ~436pt. A dot
/// is a fixed box: it says *something happened* at every width, and the tooltip
/// carries the sentence in full rather than an ellipsis of it.
///
/// ⚠️ **The hit box is deliberately larger than the ink.** [`STATUS_DOT_R`] is the
/// circle and [`STATUS_DOT_HIT`] is the square allocated for it, because a 4pt
/// target is a tooltip nobody discovers — and because the whole point of this
/// surface is that the message is one hover away rather than on screen.
///
/// ⚠️ **`ui.allocate_response` rather than a `Label`**, so the box is the same
/// whatever the text says. A `Label` sized to its own content is what made the
/// old surface depend on the window in the first place.
pub(crate) fn status_dot(ui: &mut egui::Ui, status: &crate::session::Status) {
    if status.text.is_empty() {
        return;
    }
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(STATUS_DOT_HIT, STATUS_DOT_HIT),
        egui::Sense::hover(),
    );
    ui.painter()
        .circle_filled(rect.center(), STATUS_DOT_R, status_color(status));
    resp.on_hover_text(&status.text);
}

/// The library status dot's radius: an 8pt mark.
///
/// **Sized against the sentence it replaces rather than against the bar.** The
/// status text is 11.5pt, whose cap height is near 8, so a dot of the same
/// height reads as a mark standing *in place of* the words — where a larger one
/// reads as a badge with a count missing from it, and a smaller one reads as a
/// bullet in front of text that failed to draw.
pub(crate) const STATUS_DOT_R: f32 = 4.0;

/// The box [`status_dot`] allocates, which is much bigger than the dot.
///
/// Matched to the header's other controls (`HEADER_CONTROL_H` is 28) so the dot
/// sits on the same baseline as the gear beside it and has a target a pointer can
/// find. **The ink is 8pt across and the target is 28** — see `status_dot`'s ⚠️.
const STATUS_DOT_HIT: f32 = 28.0;

/// Should a panel `width` points wide, in a parent with `room` to spare, replace
/// the `stored` one? Three gates, and each is the answer to a question
/// (§15 D349).
///
/// - **`down` — write on the frame the drag ends, not on every frame of it.**
///   `Prefs::save` serializes the whole file and writes it; a resize is a gesture
///   with a hundred frames in it, and a hundred writes into a synced config
///   directory is the fault the roadmap's *Files* section records about autosave.
///   egui reports the released size on the frame the button comes up — its
///   `Panel` recomputes on `drag_stopped` for exactly this reason — so waiting for
///   the release costs nothing.
/// - **`room` — a window too narrow to hold the panel must not narrow it
///   permanently.** egui clamps a panel to the space its parent has, so shrinking
///   the window shrinks the reported width with no drag involved, and storing that
///   would let a moment of a small window outlive it. When the window is the
///   binding constraint the reported width *is* the room, so comparing the two is
///   the whole test.
/// - **`stored` — do not rewrite the file on frames where nothing happened.**
///
/// **None of the three reads egui's resize `Response`**, which is the obvious
/// spelling and is not reachable: the handle's id is `id.with("__resize")`, a
/// private salt `Panel::show` does not hand back. A guard built on it would fail
/// *silently* the day egui renamed it — it would simply stop persisting — where
/// these are built from numbers the caller already has.
///
/// ⚠️ **The two 0.5s are one thing rather than two.** egui rounds a panel's rect
/// to whole pixels (`round_ui`), so both comparisons are against a number that has
/// been through rounding: an exact `>=` on the room misses the clamp by a
/// fraction, and an exact `!=` on the stored width writes the file every frame.
fn width_to_store(width: f32, room: f32, stored: f32, down: bool) -> Option<f32> {
    let user_did_it = !down && width < room - 0.5;
    (user_did_it && (width - stored).abs() >= 0.5).then_some(width)
}

impl OndinApp {
    /// End a live text session before acting on the document (§15 D466).
    ///
    /// **The two lines [`Self::choose_tool`] and [`Self::go_to_dashboard`] each
    /// carried, said once.** A chrome control that edits the document while a text
    /// session is open is acting on a `Document` the session's buffer has not been
    /// written back into, and the two then disagree until the session ends and
    /// commits over whatever happened in between.
    ///
    /// ⚠️ **`[S16.4-L1-02]`: five top-bar buttons were live in `Mode::TextInsert`
    /// where their own chords are deliberately dead — and this method has *three*
    /// call sites in that bar, because the other two, the folder and the brand
    /// mark, route through [`Self::go_to_dashboard`] and were closed by
    /// `[S16.4-L1-01]`. Five buttons, three arms; the counts are of different
    /// things.** `input::text_insert_mode`
    /// builds its action vector from `Action::TextStyle` alone — `Undo`, `Redo`,
    /// `Save` and `Open` are only in `normal_mode` — and `top_bar` consults
    /// `self.mode` nowhere. Measured: with a live session `Ctrl+Z` left
    /// `undo_depth` at 1, and clicking the Undo glyph 30px away took it to 0,
    /// rewinding the document under an editor whose buffer was unchanged. *Save*
    /// was worse in kind: it writes and **pins a version** from `session.doc`,
    /// which does not hold the typed text.
    ///
    /// ⚠️ **Finishing rather than dimming, which is a choice and is the one this
    /// file had already made.** The tool rail's bare-key chords are equally dead
    /// in `Mode::TextInsert` and `choose_tool` closes it by finishing; the folder
    /// and the brand mark go through `go_to_dashboard`, which closes
    /// `[S16.4-L1-01]` the same way. Dimming would make the button agree with its
    /// chord and would also make a control that visibly does nothing, which is
    /// what a person reports as broken.
    ///
    /// **What this does not settle**, and the finding says so: whether `Ctrl+Z`
    /// *should* be dead mid-session at all. `design-oracle` finds nothing on
    /// either half — §9.3 records only that `TextInsert` is *"near exclusive… no
    /// **bare** key resolves to an `Action` there"*, so the four dead chords fall
    /// out by omission rather than by a decision. That is a gap for the
    /// maintainer; the damage above is not.
    fn finish_text_first(&mut self) {
        if self.text.is_some() {
            self.finish_text_edit();
        }
    }

    /// The top bar: brand mark, breadcrumb, save state, file/edit actions,
    /// zoom readout and the Settings button.
    ///
    /// **There is no *Share*.** It was an accent-outlined button at the right end
    /// until 2026-08-24, dropped when the design regeneration removed it and the
    /// maintainer confirmed it: this is a local file editor with no sharing to do,
    /// so the button was a promise nothing behind it could keep (§15 D330).
    ///
    /// 🚨 **These two paragraphs were on [`Self::finish_text_first`]** until
    /// 2026-09-19 (§15 D815), and this function had no doc at all. That helper was
    /// inserted directly above `top_bar` and anchored on its `fn` line, so it took
    /// this run as the head of its own — `CLAUDE.md`'s first habit exactly, and
    /// the shape §15 D805 describes: the victim is left *still documented*, by a
    /// run that opens by describing something else. Found by reading the run above
    /// `finish_text_first` and asking whether its first line describes it: *"The
    /// top bar: brand mark, breadcrumb…"* plainly does not describe *"end a live
    /// text session"*. **The whole-tree length ranking cannot see this one** — the
    /// merged run is 44 lines against a floor in the fifties.
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("topbar")
            .exact_size(TOP_BAR_H)
            .resizable(false)
            .frame(panel_frame(color::TOPBAR, 14, 0))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;

                    // Brand mark: accent-tinted rounded square with a diamond —
                    // and, since the library landed, **the way back to it**.
                    //
                    // ⚠️ **It sensed nothing and now senses `click`** (spelled
                    // `Sense::hover()` at the time, which is the same value —
                    // §15 D688): a mark
                    // that does nothing and a mark that navigates look identical
                    // at rest, so the lift on hover and the tooltip are what tell
                    // the user it is a control. Both are owed together — dropping
                    // either leaves a button nobody discovers.
                    //
                    // ⚠️ **The third affordance used to be a `PointingHand` and is
                    // gone** (§15 D371). The app shows the arrow over chrome, and
                    // the cursor only changes where the change *means* something —
                    // `Grab` on a reorder grip, `ResizeHorizontal` on a drag edge,
                    // the drawn bitmaps on the canvas (§9.2, and `ui::value_field`
                    // states the rule at length). A hand here said nothing the
                    // brightening did not, and it was the loudest of the three.
                    let (mark, mark_resp) =
                        ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::click());
                    let lit = mark_resp.hovered();
                    ui.painter().rect_filled(
                        mark,
                        egui::CornerRadius::same(7),
                        if lit {
                            color::ACCENT_800
                        } else {
                            color::ACCENT_900
                        },
                    );
                    ui.painter().rect_stroke(
                        mark,
                        egui::CornerRadius::same(7),
                        egui::Stroke::new(1.0, color::ACCENT_700),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().text(
                        mark.center(),
                        egui::Align2::CENTER_CENTER,
                        icon::DIAMOND,
                        theme::icon_font(17.0),
                        color::ACCENT,
                    );
                    if mark_resp.on_hover_text("Library (Ctrl+O)").clicked() {
                        self.go_to_dashboard();
                    }

                    // **`Project / Document`, and the project half is a link.**
                    //
                    // This was the document name and only that, on the grounds
                    // that "there is one canvas per document, so a `/ Canvas`
                    // crumb named nothing the user could navigate to". That
                    // reasoning was right and is now *satisfied* rather than
                    // overturned: a project is somewhere to go, so the crumb has
                    // a first segment that earns its slash. A document in no
                    // project still shows its name alone, because the rule has
                    // not changed — a crumb segment that goes nowhere does not
                    // belong.
                    //
                    // The dirty state stays the save pill's job, so the name
                    // carries no marker.
                    self.document_crumb(ui);

                    // 10px from the name — 8 of it the row's own item spacing.
                    ui.add_space(2.0);
                    self.save_pill(ui);
                    self.status_text(ui);

                    // File / edit actions.
                    //
                    // **Both changed meaning when the library landed, and the
                    // tooltips are the only place a user finds that out.** The
                    // folder no longer opens a file dialog — there are none — it
                    // goes to the library, which is also what the brand mark does
                    // and what `Ctrl+O` does. Three doors onto one screen is not
                    // redundancy here: the mark is where a person looks for
                    // "home", the folder is where they look for "open", and they
                    // are now the same place.
                    ui.add_space(6.0);
                    if icon_button(ui, icon::FOLDER_OPEN, 28.0, 17.0, false, true)
                        .on_hover_text("Library (Ctrl+O)")
                        .clicked()
                    {
                        self.go_to_dashboard();
                    }
                    // Shift no longer does anything here: Save As is gone with
                    // the dialogs (`input::Action::Save`). What the button does
                    // instead is what the chord does — write, and pin a version.
                    if icon_button(ui, icon::FLOPPY_DISK, 28.0, 17.0, false, true)
                        .on_hover_text(if self.prefs.version_history {
                            "Save a version (Ctrl+S)"
                        } else {
                            "Save (Ctrl+S)"
                        })
                        .clicked()
                    {
                        // The typed text first, or the version this pins is the
                        // document without it (§15 D466).
                        self.finish_text_first();
                        self.save_file(true);
                    }
                    // Right cluster, reading left to right in the design as
                    // undo/redo · View · Snap · zoom · Settings. Laid out
                    // `right_to_left`, so everything below is added in the
                    // mirror of that order.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // One gap for the whole cluster, so every control and
                        // both rules are the same distance apart. Set once here
                        // rather than as `add_space` between each pair, which is
                        // how a cluster ends up with six nearly-equal gaps.
                        ui.spacing_mut().item_spacing.x = TOP_GAP;

                        // Rightmost in the cluster, because it is added first into a
                        // `right_to_left` row — which is where the design puts it.
                        self.settings_button(ui);
                        // The design's `margin-left:15px`, of which `TOP_GAP`
                        // supplies 5. The one gap in the cluster that is not the
                        // common one: everything to its left is *this document's*
                        // and this button is the app's, so the extra air is the
                        // only thing saying so.
                        ui.add_space(10.0);

                        self.zoom_control(ui);
                        top_rule(ui);
                        self.view_and_snap_menus(ui);
                        top_rule(ui);

                        // ⚠️ **Both read after any live text session would have
                        // been finished, which is why they are still read here and
                        // not inside the two `clicked()` arms** (§15 D466): a
                        // session's commit is itself an undo step, so `can_undo`
                        // taken before it and acted on after it would be a stale
                        // answer to a question the click changed. The finish
                        // happens in the arm, so these two are only ever the
                        // *enabled* state of a button nobody has pressed yet.
                        let can_undo = self.session.history.can_undo();
                        let can_redo = self.session.history.can_redo();
                        // Redo before undo: the mirror again, so the pair reads
                        // undo-then-redo on screen.
                        if icon_button(ui, icon::ARROW_U_UP_RIGHT, 26.0, 16.0, false, can_redo)
                            .on_hover_text("Redo (Ctrl+Shift+Z)")
                            .clicked()
                            && can_redo
                        {
                            self.finish_text_first();
                            self.redo();
                        }
                        if icon_button(ui, icon::ARROW_U_UP_LEFT, 26.0, 16.0, false, can_undo)
                            .on_hover_text("Undo (Ctrl+Z)")
                            .clicked()
                            && can_undo
                        {
                            self.finish_text_first();
                            self.undo();
                        }
                    });
                });
            });
    }

    /// The zoom readout, and the menu it opens.
    ///
    /// The design collapses what used to be four separate buttons into a
    /// percentage with a caret (`design/Editor.dc.html`), which is the right
    /// trade: zoom in / out / fit / actual are all on the keyboard, so the top
    /// bar only has to make them *discoverable*, not one click away. The menu
    /// carries an icon per row — the same glyphs the buttons wore — so the row
    /// a hand already knows is still findable by shape.
    ///
    /// Laid out inside a `right_to_left` cluster, so the readout is drawn after
    /// the caret and the pair ends up reading `62% ⌄` with the text against the
    /// caret rather than floating.
    fn zoom_control(&mut self, ui: &mut egui::Ui) {
        const ITEM_H: f32 = crate::ui::MENU_ITEM_H;
        const MENU_W: f32 = 150.0;

        let ctx = ui.ctx().clone();
        let zoom = (self.session.camera.zoom * 100.0).round() as i32;
        // The readout and its caret, one rect, `TOP_GAP` apart as the design has
        // it — and the same gap the View and Snap heads leave before theirs, so
        // the three carets in the cluster all sit off their marks alike.
        let head = crate::ui::menu_head(
            ui,
            egui::RichText::new(format!("{zoom}%"))
                .size(11.5)
                .color(theme::text::MUTED),
            TOP_GAP,
        )
        .on_hover_text("Zoom");
        if head.clicked() {
            self.toggle_menu(TopMenu::Zoom);
        }
        if self.open_menu != TopMenu::Zoom {
            return;
        }

        // Right-aligned under the readout, like the design's `right: 0`.
        let anchor = egui::pos2(head.rect.right() - MENU_W, head.rect.bottom() + 6.0);
        self.dropdown(&ctx, "zoom-menu", anchor, MENU_W, &head, |app, ui| {
            for (glyph, text, action) in [
                (
                    icon::MAGNIFYING_GLASS_PLUS,
                    "Zoom in",
                    crate::input::Action::ZoomIn,
                ),
                (
                    icon::MAGNIFYING_GLASS_MINUS,
                    "Zoom out",
                    crate::input::Action::ZoomOut,
                ),
                (
                    icon::NUMBER_SQUARE_ONE,
                    "Actual size",
                    crate::input::Action::ZoomReset,
                ),
                (
                    icon::CORNERS_OUT,
                    "Fit to page",
                    crate::input::Action::ZoomFit,
                ),
                // **The second half of the fit split, and the menu needs it.**
                // "Fit to page" used to quietly fit the *selection* whenever
                // there was one, so this row is not a new capability so much as
                // the one that was hiding inside the row above it
                // (`docs/shortcuts.md` §3). Without it, splitting them would have
                // taken fit-to-selection away from everyone who reached it here
                // rather than by chord.
                //
                // `SELECTION` rather than a `corners-in` to mirror the row
                // above: that codepoint is not in `theme::icon`, and Phosphor's
                // `post` table is version 3.0 with no glyph names in it, so a
                // guessed one lays out perfectly and renders nothing at all.
                // This glyph is already in use for the
                // root and group rows in the layers panel.
                (
                    icon::SELECTION,
                    "Fit selection",
                    crate::input::Action::ZoomSelection,
                ),
            ] {
                if crate::ui::menu_item(ui, glyph, text, ITEM_H).clicked() {
                    app.dispatch(&ctx, action);
                    app.open_menu = TopMenu::None;
                }
            }
        });
    }

    /// Open `which`, or close it if it is already the one showing.
    ///
    /// Opening one closes the others by construction — [`TopMenu`] holds one
    /// value — so there is no path where two dropdowns overlap.
    fn toggle_menu(&mut self, which: TopMenu) {
        self.open_menu = if self.open_menu == which {
            TopMenu::None
        } else {
            which
        };
    }

    /// A top-bar dropdown: the floating panel, plus the two ways every one of
    /// them closes — a click outside it, and Escape.
    ///
    /// Spelled once because the closing half is what gets forgotten. A menu that
    /// only closes by re-clicking its own head is a menu that stays open over the
    /// canvas while the user works around it.
    ///
    /// `width` is the width the card **paints**, border included — the same number
    /// the anchor above right-aligns against, which is the whole reason
    /// [`crate::ui::menu_inner_w`] exists rather than a subtraction here (§15 D307).
    fn dropdown(
        &mut self,
        ctx: &egui::Context,
        id: &str,
        anchor: egui::Pos2,
        width: f32,
        head: &egui::Response,
        body: impl FnOnce(&mut Self, &mut egui::Ui),
    ) {
        let menu = egui::Area::new(egui::Id::new(id))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            .show(ctx, |ui| {
                crate::ui::menu_frame(MENU_PAD).show(ui, |ui| {
                    ui.set_width(crate::ui::menu_inner_w(width, MENU_PAD));
                    ui.spacing_mut().item_spacing.y = 1.0;
                    body(self, ui);
                });
            })
            .response;

        // The head is excluded because it toggles on its own click; counting it
        // here as well would close and reopen in the same frame.
        if ctx.input(|i| i.pointer.any_click())
            && !menu.contains_pointer()
            && !head.contains_pointer()
        {
            self.open_menu = TopMenu::None;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.open_menu = TopMenu::None;
        }
    }

    /// The **View** and **Snap** dropdowns, each an icon and a caret over a
    /// column of checkable rows (`design/Editor.dc.html`).
    ///
    /// Present mode is in the View menu with the rest but is not a peer of them:
    /// it hides every piece of chrome at once and leaves the **other seven**
    /// switches untouched, so leaving it restores the workspace the user had
    /// rather than a default one. It is also the one row that has to stay
    /// reachable another way — Escape — because turning it on hides the menu it
    /// was turned on from.
    ///
    /// ⚠️ **Seven, not six — §15 D755 added *Show layout grid* and this sentence
    /// is the kind that does not get updated.** It is a count in prose standing
    /// next to the array it counts, which is the shape `CLAUDE.md` catalogues; the
    /// thing that actually holds is `ViewSwitch::VIEW_MENU`'s length, and
    /// `menu::view_switch_registry_tests::every_view_switch_is_on_exactly_one_menu`
    /// is the gate — membership rather than cardinality, so it does not care what
    /// the number is and cannot catch this sentence being wrong. **Read the
    /// array.**
    fn view_and_snap_menus(&mut self, ui: &mut egui::Ui) {
        const ITEM_H: f32 = crate::ui::MENU_ITEM_H;
        const MENU_W: f32 = 160.0;

        let ctx = ui.ctx().clone();

        // Snap first: this runs inside the top bar's `right_to_left` cluster, so
        // the one added first ends up rightmost and the pair reads View · Snap.
        // Each head is its own `horizontal`, which always lays out left to right
        // whatever its parent does — otherwise the caret would precede its icon.
        for which in [TopMenu::Snap, TopMenu::View] {
            let (glyph, tip) = match which {
                TopMenu::View => (icon::EYE, "View"),
                _ => (icon::MAGNET, "Snap"),
            };
            // `TOP_GAP` between the mark and its caret, the same as the zoom
            // readout leaves before its own. These three heads are the only
            // place two carets are close enough to compare, and 1pt here
            // against 5pt there read as a mistake rather than as a mark being
            // held tighter to its caret (§15).
            let head = crate::ui::menu_head(
                ui,
                theme::icon_text(glyph, 16.0, theme::text::MUTED),
                TOP_GAP,
            )
            .on_hover_text(tip);
            if head.clicked() {
                self.toggle_menu(which);
            }
            if self.open_menu != which {
                continue;
            }
            // Left-aligned under the icon, as the design's `left: -8px` is.
            let anchor = egui::pos2(head.rect.left() - 8.0, head.rect.bottom() + 6.0);
            self.dropdown(
                &ctx,
                match which {
                    TopMenu::View => "view-menu",
                    _ => "snap-menu",
                },
                anchor,
                MENU_W,
                &head,
                |app, ui| {
                    // The two guide rows sit straight after the rulers they are
                    // pulled out of, which is where Illustrator puts them — its
                    // View menu reads Show Rulers, then the Guides pair, then
                    // Show Grid. That order, and the labels, are
                    // [`ViewSwitch`]'s: the rows are rendered *from* the same
                    // enum the chords in `docs/shortcuts.md` §6 resolve to, so a row
                    // and its chord cannot come to mean different things.
                    let rows: &[ViewSwitch] = match which {
                        TopMenu::View => &ViewSwitch::VIEW_MENU,
                        _ => &ViewSwitch::SNAP_MENU,
                    };
                    for sw in rows {
                        let on = app.view_switch(*sw);
                        if crate::ui::menu_check(ui, sw.label(), on, ITEM_H) {
                            app.set_view_switch(*sw, !on);
                        }
                    }
                },
            );
        }
    }

    /// Whether a modal card is up: the Settings form, the *Unsaved changes*
    /// confirmation, or the recovery prompt (§15 D759).
    ///
    /// **One statement of the rule, because it had two and was about to have
    /// three.** §15 D464 wrote this condition inline in `update`'s keyboard arm
    /// and its own comment noted that *"a modal is up"* was written out twice —
    /// the other copy being `dashboard_keys`' guard list in §9.5. D759 needed it a
    /// third time, for the pointer, and *"a rule with one statement and several
    /// implementations"* is the shape this codebase keeps finding broken. So the
    /// condition is a function and the call sites ask it.
    ///
    /// ⚠️ **`edited_image` is deliberately not here**, which is R4's distinction
    /// from `a_popover_is_open`: image editing is a *mode* the canvas is in, not a
    /// card drawn over it, and a drop while cropping is an ordinary drop. The
    /// members are the three things that paint an `egui::Modal` with a backdrop.
    ///
    /// ⚠️ **It is not `ctx.egui_wants_keyboard_input()` and must not become it** —
    /// D464 measured that false for a modal holding three buttons and no text
    /// field, which is why that question was never a guard here.
    pub(crate) fn modal_is_up(&self) -> bool {
        self.settings.is_some() || self.confirming_close || !self.recovery.pending.is_empty()
    }

    /// Read a workspace switch.
    ///
    /// The menus tick their rows from here and the keymap flips them through
    /// [`Self::set_view_switch`], so both directions go through one place per
    /// switch. This pair replaced a `set_view_toggle(label, on)` keyed by the
    /// row's own string: that held the row and the state it drives in step only
    /// for as long as nobody typed a different string, and `docs/shortcuts.md` §6
    /// added a second caller that never sees the row at all.
    pub(crate) fn view_switch(&self, sw: ViewSwitch) -> bool {
        match sw {
            ViewSwitch::Rulers => self.show_rulers,
            ViewSwitch::Guides => self.show_guides,
            ViewSwitch::GuideLock => self.lock_guides,
            ViewSwitch::Grid => self.show_grid,
            ViewSwitch::LayoutGrid => self.show_layout_grids,
            ViewSwitch::Layers => self.show_layers,
            ViewSwitch::Toolbar => self.show_toolbar,
            ViewSwitch::Present => self.present,
            ViewSwitch::SnapShapes => self.snap_shapes,
            ViewSwitch::SnapGuides => self.snap_guides,
            ViewSwitch::SnapGrid => self.snap_grid,
            ViewSwitch::SnapBaselines => self.snap_baselines,
        }
    }

    /// Set a workspace switch, and say so.
    ///
    /// **The toast is here rather than at the call sites** because a chord has
    /// nothing else to tell the user with: a menu row shows its own tick as it is
    /// clicked, but `Ctrl+Shift+'` toggling snap-to-grid is otherwise completely
    /// silent. Two rows opt out — guide *lock*, whose own setter already reports,
    /// and present mode, which says something longer.
    pub(crate) fn set_view_switch(&mut self, sw: ViewSwitch, on: bool) {
        match sw {
            ViewSwitch::Rulers => self.show_rulers = on,
            ViewSwitch::Guides => self.show_guides = on,
            ViewSwitch::GuideLock => {
                self.set_guides_locked(on);
                return;
            }
            ViewSwitch::Grid => self.show_grid = on,
            ViewSwitch::LayoutGrid => self.show_layout_grids = on,
            ViewSwitch::Layers => self.show_layers = on,
            ViewSwitch::Toolbar => self.show_toolbar = on,
            ViewSwitch::Present => {
                self.present = on;
                // The menu goes with the chrome it lives in.
                self.open_menu = TopMenu::None;
                self.session.info(if on {
                    "Present mode — Escape to leave"
                } else {
                    "Left present mode"
                });
                return;
            }
            ViewSwitch::SnapShapes => self.snap_shapes = on,
            ViewSwitch::SnapGuides => self.snap_guides = on,
            ViewSwitch::SnapGrid => self.snap_grid = on,
            ViewSwitch::SnapBaselines => self.snap_baselines = on,
        }
        self.session
            .info(format!("{} {}", sw.label(), if on { "on" } else { "off" }));
    }

    /// Lock or unlock the guides — *View ▸ Lock guides* and `Ctrl+Alt+;`, both
    /// through here so the menu row and the chord cannot come to mean different
    /// things.
    ///
    /// **Locking drops a guide selection, and has to.** Locked means unselectable
    /// (`OndinApp::guide_at`), so a guide left selected would go on drawing 2px
    /// blue and could never be picked up again once anything else took the
    /// selection — *selected but unselectable*, which is the same state the ruler
    /// drag's release is careful not to create.
    ///
    /// **The guides only, not the whole selection.** This used to call
    /// `Selection::clear`, on the reasoning that a guide selection and a layer
    /// selection are exclusive so there is only ever one of the two to drop. The
    /// first half is true and the conclusion does not follow: with *layers*
    /// selected there are no guides to drop, and clearing anyway deselected the
    /// user's layers as a side effect of a switch that has nothing to say about
    /// them.
    ///
    /// **Not a per-guide property**, and not the inspector header's lock button —
    /// see [`Self::lock_guides`] for why a workspace switch is what avoids the trap
    /// of hiding the control that unlocks the thing.
    fn set_guides_locked(&mut self, on: bool) {
        self.lock_guides = on;
        if on {
            self.session.selection.clear_guides();
        }
        self.session.info(if on {
            "Guides locked"
        } else {
            "Guides unlocked"
        });
    }

    /// The breadcrumb: `Project / Document`, or just the document.
    ///
    /// **The project segment navigates**, which is what makes it a crumb rather
    /// than a decoration — clicking it goes to the library standing on that
    /// project. A document belonging to no project draws its name alone, with no
    /// leading slash: an empty first segment would be a control that goes
    /// nowhere, which is exactly what the old "one canvas per document" argument
    /// against a crumb was about.
    ///
    /// ⚠️ **`ui.label` cannot be the project half**, however much it looks like
    /// one. A label is not interactive, so it reports no hover and no click —
    /// this needs a real `Response`, which means allocating the text's measured
    /// width and interacting on it. Measuring rather than guessing, because the
    /// segment is a project name and can be any length.
    fn document_crumb(&mut self, ui: &mut egui::Ui) {
        let title = self.session.document_title();
        // Read from the *open document's own* block rather than from the
        // library's entry for its path: an unsaved document has no entry, and a
        // document whose file has just been moved has a stale one.
        let project = self
            .session
            .doc
            .meta()
            .project
            .clone()
            .and_then(|id| self.library.projects.get(&id).cloned());

        if let Some(project) = project {
            let font = egui::FontId::proportional(13.0);
            let width = ui
                .ctx()
                .fonts_mut(|f| {
                    f.layout_no_wrap(project.name.clone(), font.clone(), theme::text::MUTED)
                })
                .size()
                .x;
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(width, ui.available_height()),
                egui::Sense::click(),
            );
            // No cursor change: the brightening ink is the affordance, as on the
            // brand mark beside it (§9.2).
            let lit = resp.hovered();
            ui.painter().text(
                egui::pos2(rect.left(), rect.center().y),
                egui::Align2::LEFT_CENTER,
                &project.name,
                font,
                if lit { color::TEXT } else { theme::text::MUTED },
            );
            if resp
                .on_hover_text(format!("Open “{}” in the library", project.name))
                .clicked()
            {
                self.dash.nav = crate::panels::dashboard::Nav::Project(project.id.clone());
                self.go_to_dashboard();
            }
            // The separator is its own label so the two names keep the row's own
            // item spacing on each side of it — a slash glued to either name
            // reads as part of it.
            ui.label(
                egui::RichText::new("/")
                    .size(13.0)
                    .color(theme::text::FAINT),
            );
        }

        ui.label(
            egui::RichText::new(title)
                .size(13.0)
                .color(theme::text::STRONG),
        );
    }

    /// The save pill next to the document name. It reports the save state and
    /// nothing else: "Unsaved", "Saving…", or "Saved · 2m ago". The dot is amber
    /// while there are unsaved edits, accent once the file matches disk.
    ///
    /// ⚠️ **Three labels and two dots, which is deliberate** (§15 D393). *Saving…*
    /// is a queued write ([`EditorSession::is_saving`]), and the document still
    /// differs from its file while one is in flight — so the dot stays amber and
    /// only the sentence changes. The dot answers *does the file match*; a third
    /// colour there would be answering a question nobody asked.
    ///
    /// Laid out by hand rather than with a `Frame`. In a horizontal layout that
    /// centres its cross axis — which the top bar is — egui grows every item's
    /// frame to the full row height, so a `Frame` here came out as a 46px-tall
    /// slab behind an 11px label. Painting into an exactly-sized rect is the way
    /// to get a pill that hugs its text.
    fn save_pill(&self, ui: &mut egui::Ui) {
        /// Padding around the label, and the gap to the state dot.
        const PAD: egui::Vec2 = egui::vec2(9.0, 5.0);
        const GAP: f32 = 6.0;
        const DOT: f32 = 6.0;

        let ago = self.session.saved_ago();
        let galley = ui.painter().layout_no_wrap(
            save_label(self.session.is_saving(), ago),
            egui::FontId::proportional(11.5),
            theme::text::MUTED,
        );
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(
                PAD.x * 2.0 + DOT + GAP + galley.size().x,
                galley.size().y + PAD.y * 2.0,
            ),
            egui::Sense::empty(),
        );

        let p = ui.painter();
        p.rect_filled(rect, egui::CornerRadius::same(6), theme::color::text_a(13));
        p.circle_filled(
            egui::pos2(rect.left() + PAD.x + DOT * 0.5, rect.center().y),
            DOT * 0.5,
            match ago {
                None => egui::Color32::from_rgb(206, 168, 96),
                Some(_) => color::ACCENT_400,
            },
        );
        p.galley(
            egui::pos2(rect.left() + PAD.x + DOT + GAP, rect.top() + PAD.y),
            galley,
            theme::text::MUTED,
        );

        // "2m ago" goes stale on its own, with no input to trigger a repaint.
        if ago.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs(20));
        }
    }

    /// Mode (when it is not the default) and the last status message. Kept
    /// beside the save pill rather than inside it: a failed commit has to be
    /// visible somewhere, and the pill says one thing only.
    ///
    /// Nothing is drawn when there is nothing to report. The status line used to
    /// open on "Ready" and echo the save path, both of which the pill and the
    /// title already carry — so the bar's default state is now empty and the
    /// text only appears when something actually happened.
    ///
    /// The message itself is [`status_label`], shared with the library screen —
    /// see there for why that is a free function rather than four lines here.
    fn status_text(&self, ui: &mut egui::Ui) {
        if self.mode != Mode::Normal {
            ui.label(
                egui::RichText::new(self.mode.label())
                    .size(9.5)
                    .color(color::ACCENT_300),
            );
        }
        status_label(ui, self.session.status());
    }

    /// The 46px vertical tool rail.
    fn tool_rail(&mut self, ui: &mut egui::Ui) {
        enum E {
            Sep,
            Btn(&'static str, Option<Tool>, &'static str),
        }
        let rail = [
            E::Btn(icon::CURSOR, Some(Tool::Select), "Select (V)"),
            E::Btn(
                icon::HAND,
                Some(Tool::Hand),
                "Hand — drag to pan (H; or hold space with any tool)",
            ),
            // Third slot, with Select and Hand rather than with the shape tools:
            // it manipulates what is already there rather than drawing anything.
            E::Btn(
                icon::RESIZE,
                Some(Tool::Scale),
                "Scale — drag a handle to scale strokes, radii and type with the box (K)",
            ),
            E::Sep,
            E::Btn(icon::FRAME_CORNERS, Some(Tool::Frame), "Frame (F)"),
            E::Btn(icon::SQUARE, Some(Tool::Rect), "Rectangle (R)"),
            E::Btn(icon::CIRCLE, Some(Tool::Ellipse), "Ellipse (E)"),
            E::Btn(icon::TRIANGLE, Some(Tool::Polygon), "Polygon (G)"),
            E::Btn(icon::STAR, Some(Tool::Star), "Star (S)"),
            E::Btn(icon::LINE_SEGMENT, Some(Tool::Line), "Line (L)"),
            E::Sep,
            E::Btn(icon::PEN_NIB, Some(Tool::Pen), "Pen (P)"),
            E::Btn(
                icon::BEZIER_CURVE,
                Some(Tool::Node),
                "Edit points — drag anchors and handles of a path (A; or double-click one)",
            ),
            E::Btn(icon::TEXT_T, Some(Tool::Text), "Text (T)"),
            E::Btn(
                icon::IMAGE,
                Some(Tool::Image),
                "Place image — pick files, then click to place each (Ctrl+Shift+K)",
            ),
            // **No crop button, and that is the decision rather than an
            // omission** (§15 D268). Image editing is `Tool::ImageEdit` and is
            // entered by double-clicking a picture or pressing `Enter` on one;
            // arming it from here would arm a mode with no subject, which is what
            // the crop tool did and is half of why *Crop* and the bounding box
            // ended up meaning different things.
            E::Sep,
            E::Btn(icon::EYEDROPPER, None, "Eyedropper (soon)"),
            E::Btn(icon::CHAT, None, "Comment (soon)"),
        ];
        egui::Panel::left("rail")
            .exact_size(46.0)
            .resizable(false)
            .frame(panel_frame(color::CARD, 0, 9))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for e in &rail {
                        match e {
                            E::Sep => {
                                ui.add_space(4.0);
                                let (r, _) = ui.allocate_exact_size(
                                    egui::vec2(22.0, 1.0),
                                    egui::Sense::empty(),
                                );
                                ui.painter().rect_filled(
                                    r,
                                    egui::CornerRadius::ZERO,
                                    theme::color::text_a(31),
                                );
                                ui.add_space(4.0);
                            }
                            E::Btn(glyph, tool, tip) => {
                                // **Two lit buttons while the pen is armed inside
                                // an edit**, and that is the honest reading rather
                                // than a glitch: the node tool is in hand — its
                                // markers are on the canvas and its point selection
                                // is live — *and* the pen's verbs are what a press
                                // does (§15 D125). `active` is a boolean, so there
                                // is no third weight to reach for; lighting both is
                                // what stops the rail contradicting the cursor.
                                let active = tool.is_some_and(|t| {
                                    t == self.tool || (self.pen_bias && t == Tool::Pen)
                                });
                                let enabled = tool.is_some();
                                let resp = icon_button(ui, glyph, 30.0, 20.0, active, enabled)
                                    .on_hover_text(*tip);
                                if resp.clicked() {
                                    match tool {
                                        // **Arming the image tool *is* picking
                                        // files**, so the button runs the action
                                        // rather than setting the tool — the tool
                                        // arms itself if anything loaded, and stays
                                        // where it was if nothing did (`Tool::Image`).
                                        Some(Tool::Image) => self.place_image_action(),
                                        Some(t) => self.choose_tool(*t),
                                        None => self.session.info(*tip),
                                    }
                                }
                            }
                        }
                    }
                });
            });
    }

    /// The layers panel: header + the node tree.
    ///
    /// **300 since 2026-08-24, up from 250** (`design/Editor.dc.html`, §15 D330). A
    /// tree row spends `layers::INDENT` — 31pt — per level before it starts on a
    /// name, so at 250 a layer four deep had about 90pt to be legible in and every
    /// name in a real document elided.
    ///
    /// **And since 2026-08-25 that 300 is a default the user can drag off**
    /// (§15 D349) — `layers::MIN_W ..= MAX_W`, remembered in `prefs.json`. The
    /// inspector column beside it is a change of its own and is not this one: it is
    /// an `Area` of floating cards with no edge to put a handle on.
    ///
    /// ⚠️ **`set_min_width` is what makes the drag hold.** `Panel` takes its final
    /// rect from what the *content* occupied, not from the size the drag asked for
    /// — egui says so in `resizable`'s own doc ("you also need to make the ui use
    /// the available space") — so a panel whose content is narrower than the drag
    /// snaps back to the content on the next frame. Nothing here noticed while the
    /// size was exact, because a `Rangef::point` range cannot move.
    fn layers_panel(&mut self, ui: &mut egui::Ui) {
        // Before the panel is shown, because that is what egui measures its own
        // clamp against — see `remember_layers_width`, which needs the same number.
        let room = ui.available_rect_before_wrap().width();

        // **The drag edge's two lit states, in the theme's own accent ladder**
        // (§15 D349). egui picks the separator's stroke off the *parent* ui's
        // style — `active.fg_stroke` while dragging, `hovered.fg_stroke` under the
        // pointer, `noninteractive.bg_stroke` at rest — and those first two are
        // the **text** strokes, `TEXT` and `ACCENT_100`, both effectively white.
        // A full-height near-white hairline against a divider at 9% alpha is a
        // jump this theme makes nowhere else, so the pair is retuned to the
        // colours every other control's border uses for the same two states
        // (`hovered.bg_stroke` is `ACCENT_700`, `active.bg_stroke` is `ACCENT`) —
        // the splitter says "interactive" in the vocabulary already on screen
        // rather than inventing a louder one. The resting hairline is untouched
        // and is the same line the panel has always drawn.
        //
        // ⚠️ Restored straight after, because this `ui` is the frame's central one
        // and the tool rail, the canvas and the inspector are all still to be
        // drawn into it — the *fg* strokes are what a hovered button's label uses.
        // **And it waits `EDGE_DWELL` before doing it** (§15 D350). Reported from
        // the machine: the edge lit on every pass of the pointer across the panel,
        // which is most passes, because the splitter is on the way from the tree to
        // the canvas.
        //
        // **The delay is applied by handing egui a different colour, not by
        // suppressing its hover.** egui decides the separator's state itself and
        // hands nothing back, so what is adjustable from here is only what
        // `hovered.fg_stroke` *contains* — the resting divider until the dwell is
        // up, the accent after. The line is therefore drawn in both states and is
        // simply indistinguishable from the rest state in the first one, which is
        // also why the **cursor** still changes immediately: that comes off egui's
        // own resize response, which this does not touch, and it was not what was
        // reported as noisy.
        //
        // ⚠️ "Indistinguishable" is an **identity, not a resemblance**:
        // `color::DIVIDER` is the very value `theme::install` puts in
        // `noninteractive.bg_stroke`, which is what the resting edge draws. Naming
        // the same constant is what makes the delay invisible rather than nearly
        // invisible — retune one of the two and the wait acquires a visible first
        // state, which is a different feature nobody asked for.
        self.fold_layers_edge(ui);
        let style = ui.style().clone();
        let inner_style = style.clone();
        {
            let w = &mut ui.style_mut().visuals.widgets;
            let hover = match self.layers_edge.lit() {
                true => color::ACCENT_700,
                false => color::DIVIDER,
            };
            w.hovered.fg_stroke = egui::Stroke::new(1.0, hover);
            w.active.fg_stroke = egui::Stroke::new(1.0, color::ACCENT);
        }
        let shown = egui::Panel::left("layers")
            // ⚠️ **`default_size` before `size_range`, and swapping them breaks the
            // clamp silently.** `default_size` widens the range to admit whatever
            // it is given; `size_range` replaces the range and clamps the default
            // into it. This order therefore bounds a `prefs.json` that has been
            // hand-edited — or written by a future version — and the other order
            // would hand a 5000 straight through.
            .default_size(self.prefs.layers_width)
            .size_range(crate::panels::layers::MIN_W..=crate::panels::layers::MAX_W)
            .resizable(true)
            .frame(panel_frame(color::PANEL, 0, 0))
            .show(ui, |ui| {
                // **The retune above is for the *edge*, and the edge is drawn on
                // the parent** — `resize_panel` runs after this closure, against
                // `parent_ui`'s style. The content would otherwise inherit it, and
                // `WidgetVisuals::text_color()` *is* `fg_stroke.color`: the search
                // field's text would go accent-blue on hover. A child `Ui` holds
                // its own `Arc<Style>`, so putting the theme's back here scopes the
                // override to the one line it was for.
                ui.set_style(inner_style);
                ui.set_min_width(ui.available_width());
                egui::Frame::NONE
                    .inner_margin(egui::Margin {
                        // Named in `layers`, because `MIN_W` is argued from it and
                        // a second spelling here is the one that would go stale.
                        left: crate::panels::layers::PAD_X,
                        right: crate::panels::layers::PAD_X,
                        top: 14,
                        bottom: 9,
                    })
                    .show(ui, |ui| {
                        // A stable id, so opening the search can hand it focus
                        // before the field itself has been built this frame.
                        let field = egui::Id::new("layer-search");
                        let searching = self.layer_filter.is_some();
                        ui.horizontal(|ui| {
                            ui.label(eyebrow("Layers"));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let glyph = if searching {
                                        icon::X
                                    } else {
                                        icon::MAGNIFYING_GLASS
                                    };
                                    if icon_button(ui, glyph, 20.0, 15.0, searching, true)
                                        .on_hover_text(if searching {
                                            "Clear search"
                                        } else {
                                            "Search layers by name"
                                        })
                                        .clicked()
                                    {
                                        if searching {
                                            self.layer_filter = None;
                                        } else {
                                            self.layer_filter = Some(String::new());
                                            ui.ctx().memory_mut(|m| m.request_focus(field));
                                        }
                                    }
                                    // Fold or unfold the whole tree. One button
                                    // reading the room rather than two: with a
                                    // tree of any size, whether it is open is
                                    // obvious at a glance.
                                    let folded = self.tree_is_all_collapsed();
                                    if icon_button(
                                        ui,
                                        if folded {
                                            icon::ARROWS_OUT_LINE_VERTICAL
                                        } else {
                                            icon::ARROWS_IN_LINE_VERTICAL
                                        },
                                        20.0,
                                        15.0,
                                        false,
                                        true,
                                    )
                                    .on_hover_text(if folded {
                                        "Expand all"
                                    } else {
                                        "Collapse all"
                                    })
                                    .clicked()
                                    {
                                        self.toggle_collapse_all();
                                    }
                                },
                            );
                        });

                        if let Some(filter) = &mut self.layer_filter {
                            ui.add_space(4.0);
                            // **A [`crate::ui::field_row`], like every input in the
                            // inspector**, rather than a bare `TextEdit`. egui's own
                            // frame is a different height and has no horizontal
                            // padding to speak of, so the one text field outside the
                            // inspector was also the only one whose text started hard
                            // against its own border — and it sat shorter than the
                            // fields it is read alongside. The row brings the 28pt
                            // height, the 9pt gutter and the 5pt radius with it, so
                            // there is no second set of numbers to keep in step.
                            field_row(ui, egui::vec2(ui.available_width(), 28.0), |ui| {
                                ui.add(
                                    egui::TextEdit::singleline(filter)
                                        .id(field)
                                        .hint_text("Name…")
                                        // The row *is* the frame now; a second one
                                        // inside it draws a box in a box.
                                        .frame(egui::Frame::NONE)
                                        .desired_width(f32::INFINITY)
                                        .font(egui::FontId::proportional(12.0)),
                                );
                            });
                            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                self.layer_filter = None;
                            }
                        }
                    });

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    // **No bar, and it still scrolls.** See the inspector's for the
                    // reasoning; the tree is the panel where it showed most, because
                    // a document of any size overflows it and the bar was then a
                    // permanent second edge beside the panel's own.
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        // **A row dragged to the edge of the tree scrolls it**, the
                        // same way a layer dragged to the edge of the canvas scrolls
                        // that (`canvas::autopan`, and the curve is literally the
                        // same function). Reparenting into something scrolled out of
                        // sight was otherwise impossible: the drop target has to be
                        // on screen to be aimed at, so a deep tree could only be
                        // rearranged within one screenful at a time.
                        //
                        // Inside the scroll area, because `scroll_with_delta` speaks
                        // to the area enclosing the `Ui` it is called on. Instant
                        // rather than animated: the pointer is the clock here, and
                        // egui's default easing would keep gliding after the hand
                        // stopped and overshoot the row being aimed at.
                        if self.layer_drag.is_some()
                            && let Some(p) = ui.ctx().pointer_interact_pos()
                        {
                            let view = ui.clip_rect();
                            let v = crate::ui::edge_scroll_speed(
                                p.y,
                                view.top(),
                                view.bottom(),
                                LAYER_SCROLL_BAND,
                                LAYER_SCROLL_MAX,
                            );
                            if v != 0.0 {
                                let dt = ui.input(|i| i.stable_dt).min(1.0 / 15.0);
                                ui.scroll_with_delta_animation(
                                    // Negative y scrolls *down* — it is the content
                                    // that moves, and it moves against the pointer.
                                    egui::vec2(0.0, -v * dt),
                                    egui::style::ScrollAnimation::none(),
                                );
                                ui.ctx().request_repaint();
                            }
                        }
                        egui::Frame::NONE
                            .inner_margin(egui::Margin::symmetric(
                                crate::panels::layers::TREE_PAD_X,
                                0,
                            ))
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                self.layers_tree(ui);
                                // **The panel's second door** (`docs/context-menus.md`
                                // §6.2): the empty space below the last row, which
                                // is a target in its own right and carries two
                                // rows — *Paste*, at the root, and *Select all*.
                                //
                                // Allocated rather than `interact`ed over the
                                // panel's whole rect, so that it cannot steal the
                                // right-click that belongs to a row: this claims
                                // exactly what the rows left behind.
                                let rest = ui.available_size_before_wrap();
                                if rest.y > 0.0 {
                                    let (_, back) =
                                        ui.allocate_exact_size(rest, egui::Sense::click());
                                    if back.secondary_clicked() {
                                        self.open_context_menu(
                                            ui.ctx(),
                                            crate::menu::Target::PanelBackground,
                                            None,
                                        );
                                    }
                                }
                            });
                    });
            });
        ui.set_style(style);
        self.remember_layers_width(ui.ctx(), shown.response.rect.width(), room);
    }

    /// Gather [`EdgeDwell::advance`]'s inputs from the `Context` and do nothing
    /// else — `fold_chrome_hold`'s shape, and for its reason: the part that can be
    /// wrong is the state machine, and it is worth driving through its own states
    /// rather than through the frames that would produce them.
    ///
    /// **The band is derived here rather than read off egui's resize `Response`**,
    /// the same refusal `width_to_store` makes and for the same reason — the
    /// handle's id is a private `__resize` salt, and a hover built on it would stop
    /// working silently. What this needs instead is public: `PanelState` carries
    /// the panel's outer rect, `Interaction::resize_grab_radius_side` is the reach
    /// egui itself expands the handle by, and the pointer is the pointer. Half a
    /// point is added so the band is never thinner than what egui will actually
    /// claim, which would light the edge a frame late at its very lip.
    ///
    /// ⚠️ **`PanelState` is last frame's**, so mid-drag the band trails the edge by
    /// one frame. That is exactly the staleness egui's own resize interaction runs
    /// on — it resolves the drag against `read_response`, also last frame's — and
    /// it cannot matter here, because by the time the band is stale the state is
    /// already `Lit` and held by `down`.
    ///
    /// ⚠️ **This runs only while the panel is drawn** — its one caller is inside
    /// `layers_panel`, which the frame body gates on `show_layers` — so hiding it
    /// freezes `layers_edge` in whatever state it held. Harmless as things stand,
    /// because the fold runs *before* the panel on the frame it comes back, so a
    /// stale `Lit` resolves to `Off` in the same frame and never paints. Stated
    /// because it stops being harmless the moment anything outside `layers_panel`
    /// reads that field.
    fn fold_layers_edge(&mut self, ui: &egui::Ui) {
        let ctx = ui.ctx();
        let reach = ui.style().interaction.resize_grab_radius_side + 0.5;
        let edge = egui::PanelState::load(ctx, egui::Id::new("layers"))
            .map(|s| s.outer_rect.right())
            .unwrap_or(f32::NAN);
        let (now, pos, pressed, down) = ctx.input(|i| {
            (
                i.time,
                i.pointer.hover_pos(),
                i.pointer.button_pressed(egui::PointerButton::Primary),
                // **Primary, matching `pressed`, and not `any_down`.** The two
                // are one question here — "is a gesture in flight on this edge" —
                // and asking it of different buttons at each end is the shape a
                // later reader tidies in the wrong direction. Concretely,
                // `any_down` let a middle-button pan begun over a lit edge hold it
                // lit off the band, and let a right-drag block the dwell.
                i.pointer.button_down(egui::PointerButton::Primary),
            )
        });
        // `NAN` on the first frame, before the panel has ever stored a rect: every
        // comparison against it is false, so the band is empty rather than
        // everywhere. Stated because that is the one arm a reader would not
        // predict from the arithmetic.
        let in_band = pos.is_some_and(|p| (p.x - edge).abs() <= reach);
        self.layers_edge = self.layers_edge.advance(now, in_band, pressed, down);
        if let Some(due) = self.layers_edge.due() {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64((due - now).max(0.0)));
        }
    }

    /// Persist a dragged panel width, once per gesture and only when the *user*
    /// is what changed it. The wiring; [`width_to_store`] is the decision.
    ///
    /// **Split in two because the write is a side effect on the user's machine.**
    /// `Prefs::save` puts a file in `dirs::config_dir()`, so a test that could
    /// reach this would rewrite the real `prefs.json` of whoever ran it. The pure
    /// half takes the four numbers and answers "store what, if anything"; this
    /// half reads the pointer and does the write, and is the part still verified
    /// by reading.
    fn remember_layers_width(&mut self, ctx: &egui::Context, width: f32, room: f32) {
        let down = ctx.input(|i| i.pointer.any_down());
        if let Some(w) = width_to_store(width, room, self.prefs.layers_width, down) {
            self.prefs.layers_width = w;
            self.prefs.save();
        }
    }

    /// The column the inspector's cards live in, in screen coordinates.
    ///
    /// Named because the panels' **popovers** need it too: they hang to the left of
    /// this rect rather than under the button that opens them, and the only thing
    /// that knows where it is, is the thing that puts it there. See
    /// [`Self::popover_anchor`].
    pub(crate) fn inspector_column(screen: egui::Rect) -> egui::Rect {
        /// The design's card width: a 296px pane with a 12px right gutter and
        /// nothing on the left (`design/Editor.dc.html`). The cards were 12px
        /// narrower than that, which read as a wider margin around every field
        /// than the design has.
        const WIDTH: f32 = 284.0;
        const GUTTER: f32 = 12.0;
        egui::Rect::from_min_max(
            egui::pos2(
                screen.max.x - WIDTH - GUTTER,
                screen.min.y + TOP_BAR_H + GUTTER,
            ),
            egui::pos2(screen.max.x - GUTTER, screen.max.y - GUTTER),
        )
    }

    /// The x an inspector popover starts at.
    pub(crate) fn popover_left(ctx: &egui::Context, width: f32) -> f32 {
        /// Between the popover and the inspector's cards — the same air the picker
        /// leaves, so a popover and a picker read as the same kind of thing.
        const GAP: f32 = 10.0;
        const MARGIN: f32 = 12.0;
        let screen = ctx.input(|i| i.content_rect());
        (Self::inspector_column(screen).left() - width - GAP).max(screen.min.x + MARGIN)
    }

    /// Where a panel's popover goes: **clear of the inspector, to its left.**
    ///
    /// It used to hang right-aligned under the button that opened it, which put it
    /// over the cards — and the two grounds are the same colour by design, so a
    /// 272pt popover on top of a 284pt card read as one continuous surface with a
    /// shadow somewhere in the middle of it. Reported as "it kind of blends with
    /// the background… hard to understand where things are". The detached colour
    /// picker had already solved this by sitting to the left, so the popovers do
    /// what it does; the alternative — giving the popover a different ground — would
    /// have been a second surface colour invented to work around an overlap that
    /// need not happen.
    ///
    /// `head` is the rect of the control that opened it, and only its *top* is
    /// used: the popover lines up with the row it belongs to. The `Area` is
    /// separately `constrain_to` the screen, which is what pulls a tall one back up
    /// when the button is low in the column.
    pub(crate) fn popover_anchor(ctx: &egui::Context, head: egui::Rect, width: f32) -> egui::Pos2 {
        egui::pos2(Self::popover_left(ctx, width), head.top())
    }

    /// The rightmost x the detached picker may start at.
    ///
    /// **Because the popovers now share the picker's side of the screen.** Both
    /// hang to the left of the inspector, so a picker placed relative to the row
    /// that opened it — a Fill chip, say — landed on top of an open stroke or
    /// typography popover, and the popover is `Foreground` where the picker is
    /// `Middle`, so it covered it. Reserving the lane is what keeps three surfaces
    /// legible at once.
    ///
    /// A picker opened from *inside* a popover (a decoration's colour) needs no
    /// special case: its own anchor is already inside the popover, so the same
    /// clamp puts it left of it.
    pub(crate) fn picker_lane_right(&self, ctx: &egui::Context, anchor_x: f32) -> f32 {
        if self.a_popover_is_open() {
            return anchor_x.min(Self::popover_left(ctx, POPOVER_W));
        }
        anchor_x
    }

    /// Whether anything is occupying the popover lane — **every popover, asked
    /// once** (§15 D533).
    ///
    /// ⚠️ **This was a four-term disjunction inside [`Self::picker_lane_right`]
    /// and there are six popovers in that lane.** The two it missed are the
    /// Export card's: `export::export_settings_popup`, gated on
    /// `export_row_open`, and `export::export_menu_popup`, gated on
    /// `export_menu` — both anchored through [`Self::popover_anchor`] at
    /// `POPOVER_W`, which is the same width the lane is computed from. So
    /// opening either and then clicking a Fill chip put the picker **under** it:
    /// the popover is `Order::Foreground` where the picker is `Order::Middle`,
    /// and the arithmetic from the measured column rect puts 236 of the picker's
    /// 240 points inside the popover's span. `picker_lane_right` is consulted
    /// only on the frame the picker is created, so a popover opened *first* — the
    /// ordinary order — is the whole of the exposure.
    ///
    /// **A named query rather than a longer disjunction**, which is the half of
    /// the fix worth having: the seventh popover joins this list or it does not
    /// work, and there is one place to look. The design has the count wrong too
    /// — `docs/context-menus.md` §0 R4 says *"the four inspector popovers"* —
    /// which is what a set nobody owns does to everyone who counts it.
    ///
    /// The image-editing card is asked for the way it is *opened*: by the mode
    /// being on, not by a field recording that it is.
    fn a_popover_is_open(&self) -> bool {
        self.type_menu.is_some()
            || self.stroke_menu.is_some()
            || self.effect_menu.is_some()
            || self.export_menu
            || self.export_row_open.is_some()
            || self.edited_image().is_some()
    }

    /// Whether a popover *flag* is set that would claim an `Escape` press (§15
    /// D801) — **half of the claim**: the router also asks
    /// [`Self::popover_heard`], since a flag nothing is drawing claims nothing
    /// (§15 D847).
    ///
    /// **D527's rule — a press that dismisses a floating thing is spent on it —
    /// applied to the five popovers that were breaking it.** Measured before it
    /// was written, on a headless app with the tool set to `Rect` so the ladder's
    /// last rung is observable: `stroke_menu`, `effect_menu` and `type_menu` each
    /// closed themselves **and** left the tool at `Select`, one press paying out
    /// twice; `export_menu` and `export_row_open` read no key at all, so `Escape`
    /// left them up **and** paid out a rung, which is D527's failure shape exactly.
    ///
    /// 🚨 **This is [`Self::a_popover_is_open`] minus the image-edit card, and the
    /// difference is load-bearing rather than tidy.** Image editing is a
    /// `Tool::ImageEdit`, so *leaving* it is the ladder's own `tool != Select`
    /// rung — gating `Escape` on it would strand the user in the mode with the
    /// key that exits it swallowed. **The two sets differ by exactly one member
    /// for the same reason D533's clear-set does and in the same direction**: the
    /// image card is what the mode looks like, not a floating thing somebody
    /// opened over it.
    ///
    /// ⚠️ **`Escape` only, not the keyboard.** A modal owns every key because the
    /// document behind it cannot be seen; a popover is a small control over a
    /// canvas the user is still looking at, and nothing about it earns `Delete`
    /// or an arrow. The narrow claim is the one D527 actually makes.
    fn a_popover_owns_escape(&self) -> bool {
        self.type_menu.is_some()
            || self.stroke_menu.is_some()
            || self.effect_menu.is_some()
            || self.export_menu
            || self.export_row_open.is_some()
    }

    /// The inspector: a column of individually floating panels over the right of
    /// the canvas.
    ///
    /// Deliberately **not** an `egui::Panel`. A panel would reserve a strip of
    /// the window and paint a ground behind the cards; the design has the
    /// artwork running right to the window edge with nothing but the cards on
    /// top of it (`design/Editor.dc.html`), so this is an `Area` drawn after the
    /// canvas. It shrinks to its content, which is what makes the canvas below
    /// the last panel clickable — and what makes the gaps *between* cards
    /// non-clickable, so aiming at a panel can never deselect by accident.
    fn inspector_panel(&mut self, ctx: &egui::Context) {
        let screen = ctx.input(|i| i.content_rect());
        let column = Self::inspector_column(screen);

        egui::Area::new(egui::Id::new("inspector"))
            .order(egui::Order::Middle)
            .fixed_pos(column.min)
            .default_size(column.size())
            .show(ctx, |ui| {
                // An `Area` hands its contents a `Ui` whose `max_rect` is the
                // area's *own size from the previous frame* — it is built to
                // shrink-wrap content that sizes itself. A `ScrollArea` does the
                // opposite: it fills the space it is offered. Nested naively the
                // two converge on a tiny scrolling box, so re-establish the
                // intended column geometry here instead of inheriting it.
                ui.scope_builder(egui::UiBuilder::new().max_rect(column), |ui| {
                    egui::ScrollArea::vertical()
                        .max_width(column.width())
                        // **Hidden, not disabled** — the wheel, the trackpad and a
                        // programmatic `scroll_to` all still work; what goes is the
                        // *drawing*. These panels are not a document view: the cards
                        // float free over the artwork with no backdrop pane, so a
                        // bar down their side reads as an edge to a container that
                        // is not there, and it appears and vanishes as panels open
                        // and close under it. The popovers keep theirs — a font list
                        // is a list you page through, and the bar is the only thing
                        // saying how far down it you are.
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .show(ui, |ui| {
                            ui.set_width(column.width());
                            // The gap between cards. Tight on purpose: each
                            // card already carries a hairline and a shadow, so
                            // the separation reads at 5px, and the wider gap
                            // this started at spread a five-panel inspector
                            // past the bottom of the window for no gain.
                            ui.spacing_mut().item_spacing.y = 5.0;
                            self.inspector_ui(ui);
                        });
                });
            });
    }

    /// Advance [`ChromeHold`] one frame: pick up whatever the last pass's value
    /// fields left behind, then decide whether the chrome comes back (§15 D128).
    ///
    /// **One frame behind, unavoidably, and it is a latency rather than a
    /// disagreement.** The canvas is drawn before the inspector, and the keystroke
    /// that changes a value is consumed by a field that has not run yet when the
    /// canvas draws — so the earliest the chrome can go is the frame after the one
    /// that changed the number. It really is the *next* frame rather than the next
    /// time something happens to wake the app, but only because the end of
    /// [`eframe::App::ui`] asks for one whenever a note is pending — nothing on the edit
    /// path requests a repaint by itself.
    ///
    /// **The chrome comes back on whichever comes first, the timeout or the pointer
    /// moving over the canvas.** The motion rule is the better half: moving onto the
    /// artwork says you are done editing and about to do something else, which is a
    /// truer signal than any clock. It is deliberately *canvas* motion and not
    /// pointer motion — travelling from one field to the next must not bring the box
    /// back, and "the pointer moved" is the version that gets written by accident.
    ///
    /// **A scrub in flight is not cancelled by it**, which the design did not say and
    /// the mechanism demands: a long drag wraps the pointer round the screen edge
    /// (`ui::wrap_scrub`) and crosses the canvas on the way, so canvas motion during
    /// a scrub is the gesture itself rather than a change of mind.
    fn fold_chrome_hold(&mut self, ctx: &egui::Context) {
        let edit = self.pending_edit.take();
        let canvas = self.canvas_rect;
        let (now, pointer_down, moved_on_canvas) = ctx.input(|i| {
            (
                i.time,
                i.pointer.any_down(),
                i.pointer.delta() != egui::Vec2::ZERO
                    && i.pointer.latest_pos().is_some_and(|p| canvas.contains(p)),
            )
        });
        self.chrome_hold = self
            .chrome_hold
            .advance(now, edit, pointer_down, moved_on_canvas);
        // **A timer in a reactive app has nothing to wake it.** Once the pointer
        // stops the frame loop stops, so without this the chrome never comes back
        // from a typed value the user then leaves alone — the same stranded-first-wake
        // shape as the font-face repaint (§15 D110). Asked every held frame rather
        // than only on the frame that starts the count: egui keeps the earliest
        // request, so repeating it is free and cannot be forgotten on a restart.
        if let ChromeHold::Until(deadline) = self.chrome_hold {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(deadline - now));
        }
    }

    /// Note that an inspector edit is happening, so the selection chrome gets out of
    /// the way while it is looked at (§15 D128).
    ///
    /// **The transaction decides, not the widget.** This began by asking the *field
    /// helpers* — a `value_field` moves a number, a `field_button` and a segmented
    /// cell do not — which was the wrong seam and was reported as such: rotating 90°,
    /// flipping, picking a colour, and choosing a stroke's position all change the
    /// artwork and all left the box sitting over it, while the question "is this
    /// widget a value field" could not see any of them. What the seven reported
    /// controls have in common with the two the design excluded is not their shape
    /// but their **effect**: they change what is drawn, and the proportion lock and
    /// the per-corner disclosure do not (`Transaction::changes_ink`).
    ///
    /// That also makes the design's opt-out rule — *keep the box where the edit
    /// changes the box without changing the ink* — most of the way automatic instead
    /// of a per-field flag. The prop lock is excluded because `SetProportionsLocked`
    /// draws nothing; opening a dialog is excluded because it commits nothing at all.
    /// One case is left over and needs saying by hand ([`Self::commit_chrome_edit`]).
    ///
    /// `resp` is the control's response where there is one. A `None` is a discrete
    /// edit — a button, a swatch, a menu row — which has no duration, so its count
    /// starts at once.
    pub(crate) fn note_edit(&mut self, resp: Option<&egui::Response>, tx: &Transaction) {
        let Some(dragging) = edit_note(resp, tx) else {
            return;
        };
        // Or-ed, not overwritten: a drag in flight has to win over a keystroke
        // landing in another field, because its hold has no deadline yet.
        self.pending_edit = Some(self.pending_edit.unwrap_or(false) || dragging);
    }

    /// [`Self::note_edit`] where the caller already knows the edit is visible and has
    /// no `Transaction` to hand it. `live` is whether a gesture still holds the
    /// pointer, which is a hold with no deadline until the button comes up.
    ///
    /// **Both callers are behind a guard of their own**, which is the only reason this
    /// may be unconditional where `edit_note` may not — move either of them and the
    /// every-frame hold `edit_note` describes comes back:
    ///
    /// - `char_valve`'s **partial** branch, behind `resp.changed()`, where a live text
    ///   session owns the range being restyled. That path previews through the session
    ///   and commits nothing until the session ends (§9.3), so no committer downstream
    ///   would ever arm the hold and the outline would sit over the very text being
    ///   recoloured. The edit it carries is a `SetText`, for which `changes_ink` is
    ///   unconditionally `true`, so asking would be a formality.
    /// - `picker::pointer_slot`'s **press** branch, behind
    ///   `is_pointer_button_down_on`, where the colour has just changed but the
    ///   gesture has not yet become a drag (§15 D129). `live` is `true` there: the
    ///   button is down, so the count must not start until it lifts.
    pub(crate) fn note_live_edit(&mut self, live: bool) {
        self.pending_edit = Some(self.pending_edit.unwrap_or(false) || live);
    }

    /// Commit an inspector edit, arming the chrome hide if it changes anything drawn.
    ///
    /// Most callers are the instantaneous controls — a button, a swatch, a menu row —
    /// which have no drag to preview. The two hand-rolled valves (`multi_valve`,
    /// `char_valve`) also commit through here, and they call
    /// [`Self::note_edit`] themselves every frame besides: a valve that noted only on
    /// its committing frame would leave the box up for the whole drag and hide it
    /// afterwards.
    ///
    /// **The panels commit through this or through [`Self::edit_valve`], never
    /// through `session.commit` directly**, and that is the whole guard against the
    /// chrome hide becoming a roll-call. A control added later gets it by using the
    /// committer its neighbours use; the exceptions are named and say why
    /// ([`Self::commit_chrome_edit`], and the layers panel, which is not the
    /// inspector).
    ///
    /// ⚠️ **The identity row's *Group* and *Ungroup* buttons are a third exception,
    /// and it is deliberate** (§15 D562). They call [`Self::group_selection`] and
    /// [`Self::ungroup_selection`] — the same verbs `Ctrl+G` and `Ctrl+Shift+G` do,
    /// which is the whole of that entry — and those commit through `session.commit`,
    /// so the buttons no longer arm the hide. **That is the correct behaviour and
    /// the old panel copy's `commit_edit` was the wrong half**: grouping does not
    /// change one pixel of the artwork, so there is nothing to get the selection box
    /// out of the way *for*. `Transaction::changes_ink` answers `true` all the same,
    /// because it classifies `CreateNode` and `Reparent` by op rather than by
    /// outcome — which is right for its own purpose and is why this cannot be left
    /// to the predicate.
    ///
    /// ⚠️ **D128 states the enforcement as *"the only `session.commit` left under
    /// `panels/` is in `layers.rs`"*, and that grep still passes** — these two reach
    /// it through an `OndinApp` method declared in `app.rs`. So the rule reads as
    /// unbroken and the exception is invisible to the check that guards it; it is
    /// written here because nothing else can say it.
    ///
    /// 🚨 **There was a fourth, and it was named nowhere at all** (§15 D679,
    /// `[S16.4-L3-06]`): `inspector::remove_selected_guides`, the guide card's Trash
    /// button, committed through `session.commit` directly. It is routed through
    /// here now — behaviourally identical, since `RemoveGuide` is one of the ops
    /// `changes_ink` answers `false` for, so nothing was ever armed — and D128's
    /// grep, which had been **false** rather than merely incomplete, passes again.
    /// ⚠️ **A census that skips test code by watching for `#[cfg(test)]` and never
    /// coming back misses most of `inspector.rs`.** The first `#[cfg(test)]` in that
    /// file is an attribute on a **`const`**, not a module, and it sits near the top
    /// — the first top-level test *module* is most of the file further down — so a
    /// sweep that latches on the first attribute stops before nearly everything.
    /// That is what the sweep this session first ran did, and it reported three
    /// committing sites where there were four.
    ///
    /// ⚠️ **Which is not the reason the finding survived, and the correction is
    /// worth more than the note**: `remove_selected_guides` sits *before* the first
    /// test module, so a module-aware sweep would have found it outright. The
    /// mechanism only bites under the attribute reading. The test below sidesteps
    /// both by discriminating on the **receiver** — `self.session.` — instead of
    /// trying to know where test code begins.
    ///
    /// Returns whether the commit took, as `EditorSession::commit` does, so the
    /// callers that report success keep reading the same way.
    pub(crate) fn commit_edit(&mut self, tx: Transaction) -> bool {
        self.note_edit(None, &tx);
        self.session.commit(tx)
    }

    /// [`Self::commit_edit`] for an edit whose **only** visible result is the
    /// selection chrome itself: it commits without arming the hide.
    ///
    /// One caller, **box trim**. §15 D78 guarantees that trimming never moves the
    /// ink, so what it moves is the box the selection outline draws — and hiding
    /// that box would take away the only thing there was to see. This is the
    /// design's per-field opt-out, in the one place it turns out to have an
    /// instance, spelled as a named committer rather than as a flag threaded
    /// through the shared one.
    pub(crate) fn commit_chrome_edit(&mut self, tx: Transaction) -> bool {
        self.session.commit(tx)
    }

    /// Route an inspector edit through the preview→commit valve: live preview for
    /// as long as the control is held, **one** commit when the edit ends.
    ///
    /// **"Ends" is a transition, not an event, and getting that wrong put every
    /// keystroke in the undo history** (§15 D316). This used to commit on
    /// `drag_stopped() || lost_focus() || changed()`. A `DragValue` being typed
    /// into reports `changed()` on *every keystroke* and `lost_focus()` on **two
    /// consecutive frames**, so typing `100` into a width field and pressing
    /// `Enter` left five entries in the history — `1`, `10`, `100`, and then the
    /// same `100` twice more. Reported from the machine by pressing undo five times
    /// and photographing each step, which is the only way anybody was going to see
    /// it: every one of those commits writes the *same* final number, so the shape
    /// on screen is right and only the history is wrong.
    ///
    /// So the valve asks whether the control is **engaged** — held by the pointer
    /// or holding the keyboard — and commits on the frame that stops being true.
    /// One edit, one commit, however many times egui announces the ending.
    ///
    /// ⚠️ **D316's fix reached this valve and not the other one, and nobody
    /// noticed for two hundred entries.** `panels::inspector::multi_valve` was
    /// still committing on `drag_stopped() || lost_focus() || changed()` — the
    /// expression quoted three paragraphs up — until §15 D519, so the same typed
    /// number left one step on the single-layer card and five on the
    /// multi-selection card. The third hand-rolled valve is
    /// `panels::typography::char_valve`, **fixed the same way at §15 D523** —
    /// [`Self::commit_edit`]'s doc names both — and a change to the *condition*
    /// here is a change all three of them owe. `valve_condition_gate` is what
    /// now says so without anybody remembering.
    ///
    /// **And it previews while it is engaged, which is what keeps typing live.**
    /// Committing per keystroke was also what made a typed width show up on the
    /// canvas as it was typed; the preview does that now, and the document hears
    /// about it once. That is D51's rule — the preview and the commit are the same
    /// transaction, built from the committed document — applied to the keyboard
    /// rather than only to the pointer.
    ///
    /// It also makes `Escape` cancel a typed edit for free: a preview is in flight
    /// while the field has focus, so [`Self::cancel_gesture`] has something to find
    /// and drops it.
    ///
    /// Keyed on the widget's own id in `Context::data`, so the valve stays
    /// free-standing rather than needing a flag per caller. **One accessor, not two
    /// nested** — `data_mut` inside `data_mut` deadlocks (`CLAUDE.md`), so the read
    /// and the write share a closure.
    ///
    /// **Returns whether this frame committed**, which one caller needs and which
    /// it used to compute for itself (§15 D524). A caller that has to act *after*
    /// the commit — `inspector::valve_slot`'s two retargets — was
    /// mirroring the condition here in a `let committing = …` of its own, and the
    /// mirror was the superseded expression: it answered *true* on every keystroke
    /// of a typed colour, where this valve only previews. **A predicate that
    /// predicts what another function will do is a copy of that function**, and
    /// this is the cheapest way not to have one.
    pub(crate) fn edit_valve(&mut self, resp: &egui::Response, tx: Transaction) -> bool {
        // **The latch is updated before anything can bail, and that ordering is the
        // whole of §15 D317.** A cancelled gesture used to return here first, which
        // left the engagement latch below still armed: the frame that cancelled
        // never consumed the transition, so the *next* frame saw `was_engaged`
        // without `engaged`, found the `Escape` key long gone from `input`, and
        // committed the edit the cancel had just thrown away. Traced on the machine
        // — the pass that cancelled printed no valve line at all, and the pass after
        // it printed `engaged=false was=true esc=false → COMMIT`.
        //
        // Reading it here means a cancelled frame *spends* the transition, which is
        // the same rule R1 states for a click: whatever calls the gesture off has
        // used it up.
        let engaged = resp.dragged() || resp.has_focus();
        let was_engaged = resp.ctx.data_mut(|d| {
            let id = resp.id.with("valve-engaged");
            let was = d.get_temp::<bool>(id).unwrap_or(false);
            d.insert_temp(id, engaged);
            was
        });
        // The scrub was called off; the button is still down, but the edit is
        // over. Without this the release lands here as a plain `drag_stopped`.
        if self.gesture_cancelled {
            return false;
        }
        // **Every valved edit arms the chrome hide** — the valve is the §9.3 seam
        // every inspector control routes a transaction through, so asking here
        // catches a colour dragged on the picker's saturation plane as readily as a
        // width scrubbed in a field. `dragged()` is what makes a drag's count start
        // at its release rather than at its first frame (§15 D128).
        self.note_edit(Some(resp), &tx);
        let mut committed = false;
        if engaged {
            self.session.set_preview(&tx);
            self.preview_guide(&tx);
            self.preview_grid(&tx);
        } else if was_engaged {
            self.guide_previews.clear();
            self.in_flight.grid_paint = None;
            // **`Escape` abandons the edit; every other way out of the field
            // confirms it.** egui does not revert a `DragValue`'s number on
            // `Escape` — it hands back what was typed and merely surrenders focus —
            // so without this the key that means *cancel* everywhere else in the
            // app would apply the edit.
            if !resp.ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                committed = self.session.commit(tx);
            }
            // ⚠️ **`clear_gesture_preview`, not `clear_preview`, which is the
            // one spelling §15 D109 asks for: *"the exemption for a live text
            // session is spelled once … add no third spelling"*.** The two
            // previews are different things — a gesture's dies on any commit, a
            // live text session's holds content the document does not have yet —
            // and `clear_preview` drops both. On the commit path `commit_inner`
            // had already cleared the gesture's correctly and this line then
            // clobbered the session's anyway; on the `Escape` path nothing
            // commits, so this was the only clear and it took the typed text off
            // the canvas outright. `preview_session` is event-driven rather than
            // per-frame, so nothing put it back until the next keystroke.
            self.session.clear_gesture_preview();
        }
        committed
    }

    /// The guide half of [`EditorSession::set_preview`].
    ///
    /// `RenderOverrides` cannot carry a guide edit — a guide is chrome, not
    /// scene content (`rulers.rs`) — so the valve every inspector control
    /// already goes through forks here instead of each guide control growing
    /// its own preview path. Ops naming a guide the document does not have are
    /// skipped: the commit will refuse them too.
    ///
    /// [`EditorSession::set_preview`]: crate::session::EditorSession::set_preview
    /// **Every guide the transaction names, not just the last one.** This used to
    /// assign to a single `Option`, so a transaction touching two guides — which
    /// is every edit made with two selected — left only one of them previewed.
    /// The other kept reading its committed value, the pair disagreed, and the
    /// inspector reported `Mixed` at a guide it was in the middle of moving.
    pub(crate) fn preview_guide(&mut self, tx: &Transaction) {
        for op in &tx.0 {
            let id = match op {
                Operation::SetGuidePosition { id, .. }
                | Operation::SetGuideColor { id, .. }
                | Operation::SetGuideScope { id, .. } => *id,
                _ => continue,
            };
            // Build on the pending guide, not the committed one, so several ops
            // in one transaction compose as they will when applied.
            let pending = self
                .guide_previews
                .iter()
                .find(|g| g.id == id)
                .copied()
                .or_else(|| self.session.doc.guide(id).copied());
            let Some(mut guide) = pending else { continue };
            match op {
                Operation::SetGuidePosition { position, .. } => guide.position = *position,
                Operation::SetGuideColor { color, .. } => guide.color = *color,
                Operation::SetGuideScope {
                    owner, position, ..
                } => {
                    guide.owner = *owner;
                    guide.position = *position;
                }
                _ => {}
            }
            match self.guide_previews.iter_mut().find(|g| g.id == id) {
                Some(slot) => *slot = guide,
                None => self.guide_previews.push(guide),
            }
        }
    }

    /// The layout-grid half of [`EditorSession::set_preview`] —
    /// [`Self::preview_guide`]'s twin, and it exists for the identical reason
    /// (§15 D387).
    ///
    /// A grid is chrome (`canvas::draw_layout_grids`), so `RenderOverrides` absorbs
    /// `SetLayoutGrids` as a no-op and the valve every inspector control routes
    /// through has nothing to show. Without this the picker's wheel and its alpha
    /// would move the swatch and leave the bands on the canvas at their committed
    /// colour until the pointer came up — which is the symptom §15 D141 was
    /// reported as, on the guide, one preview path over.
    ///
    /// ⚠️ **Only a colour change is taken, and the entry is found by comparison.**
    /// The transaction carries the whole list (`Operation::SetLayoutGrids` has no
    /// per-entry shape), so what changed has to be read out of it; taking anything
    /// else would let a *field* edit that happened to reach this valve write into
    /// the picker's buffer, which is the one thing `grid_paint`'s own note says the
    /// two buffers must not do to each other. A list that has changed length is
    /// skipped outright: the indices no longer line up, so the comparison would be
    /// against the wrong grid.
    ///
    /// ⚠️ **One op per selected frame, and they are gathered rather than the last
    /// one winning.** A grid colour is written to every selected frame at once, so
    /// this is handed *n* ops that make the same change — and a buffer holding only
    /// the last would preview the drag on one frame while committing it to all of
    /// them, which is exactly what naming a list of frames is for. Ops that do not
    /// agree with the first about the index or the colour are dropped: the panel
    /// only draws rows where the frames already agree, so a disagreement here means
    /// this is not the transaction it looks like.
    ///
    /// [`EditorSession::set_preview`]: crate::session::EditorSession::set_preview
    pub(crate) fn preview_grid(&mut self, tx: &Transaction) {
        let mut held: Option<(Vec<NodeId>, usize, ondin_core::peniko::Color)> = None;
        for op in &tx.0 {
            let Operation::SetLayoutGrids { id, grids } = op else {
                continue;
            };
            let Some(committed) = self.session.doc.get(*id).map(|n| n.grids()) else {
                continue;
            };
            if grids.len() != committed.len() {
                continue;
            }
            let Some(i) = grids.iter().zip(committed).position(|(a, b)| a != b) else {
                continue;
            };
            if grids[i].color == committed[i].color {
                continue;
            }
            match &mut held {
                Some((ids, at, c)) if *at == i && *c == grids[i].color => ids.push(*id),
                Some(_) => {}
                None => held = Some((vec![*id], i, grids[i].color)),
            }
        }
        if let Some(held) = held {
            self.in_flight.grid_paint = Some(held);
        }
    }

    /// World point for a canvas-local egui position.
    pub(crate) fn to_world(&self, p: egui::Pos2, rect: egui::Rect, ppp: f32) -> Point {
        let local = Vec2::new(
            ((p.x - rect.min.x) * ppp) as f64,
            ((p.y - rect.min.y) * ppp) as f64,
        );
        self.session.camera.screen_to_world(local, self.canvas_px)
    }

    /// egui position for a world point.
    pub(crate) fn to_screen(&self, w: Point, rect: egui::Rect, ppp: f32) -> egui::Pos2 {
        let v = self.session.camera.world_to_screen(w, self.canvas_px);
        egui::pos2(rect.min.x + v.x as f32 / ppp, rect.min.y + v.y as f32 / ppp)
    }
}

#[cfg(test)]
mod tests {
    use super::save_label;
    use std::time::Duration;

    // **`the_export_extension_picks_the_writer_and_svg_takes_everything_else` was
    // here** and went with *Export as…* on 2026-08-22. It pinned `wants_raster`,
    // which read the format out of the save dialog's chosen extension — the whole
    // of what that dialog decided. The Export panel picks a format from a control
    // instead, so there is no extension to interpret and nothing left to pin.

    #[test]
    fn the_save_pill_reports_the_state_not_the_seconds() {
        assert_eq!(save_label(false, None), "Unsaved");
        assert_eq!(
            save_label(false, Some(Duration::from_secs(0))),
            "Saved · just now"
        );
        assert_eq!(
            save_label(false, Some(Duration::from_secs(44))),
            "Saved · just now"
        );
        assert_eq!(
            save_label(false, Some(Duration::from_secs(45))),
            "Saved · 1m ago"
        );
        assert_eq!(
            save_label(false, Some(Duration::from_secs(120))),
            "Saved · 2m ago"
        );
        // Rounds to the nearest unit rather than truncating, so 119s is "2m".
        assert_eq!(
            save_label(false, Some(Duration::from_secs(119))),
            "Saved · 2m ago"
        );
        assert_eq!(
            save_label(false, Some(Duration::from_secs(7200))),
            "Saved · 2h ago"
        );
        assert_eq!(
            save_label(false, Some(Duration::from_secs(79_199))),
            "Saved · 22h ago"
        );
        assert_eq!(
            save_label(false, Some(Duration::from_secs(86_400 * 3))),
            "Saved · 3d ago"
        );
    }

    /// **A write in flight is its own sentence, and it beats the other two**
    /// (§15 D393).
    ///
    /// ⚠️ **The second assertion is the one worth having.** `saving` and a stale
    /// `ago` cannot both be live in the app — a queued write leaves the session
    /// dirty, so `saved_ago` is `None` — but the pill is a pure function of two
    /// arguments and nothing at this level enforces that. Pinning the precedence
    /// says which one wins if the wiring ever hands it both, and *Saving…* is the
    /// answer: it is the newer fact.
    ///
    /// **Flip-checked** by moving the `saving` branch below the `ago` one, which
    /// is the version somebody writes by appending the new case to the end. The
    /// first assertion still passes — `ago` is `None` there and falls through to
    /// "Unsaved"… which is wrong, but only the second assertion says so, failing
    /// with "Saved · just now".
    #[test]
    fn a_queued_write_says_saving_whatever_else_is_true() {
        assert_eq!(save_label(true, None), "Saving…");
        assert_eq!(save_label(true, Some(Duration::from_secs(0))), "Saving…");
    }

    /// **Every band can say `1`, and an hour-old save says `1h`** (§15 D678,
    /// `[S16.3-L1-05]`).
    ///
    /// The hours band opened at 90 minutes over a round-half-up formula, so its
    /// smallest possible output was `2` — *"Saved · 1h ago"* was unreachable for
    /// every input in the function's domain, and an hour-old save read *"Saved ·
    /// 60m ago"* before jumping straight to `2h`.
    ///
    /// ⚠️ **The `1d` row is not filler.** It is the band that was already right,
    /// and it is what makes 2700 the correct opening rather than a taste: under
    /// half-up, a band's opening prints `1` exactly when it lies in
    /// `[unit/2, 3·unit/2)`, and 79200 does. Asserting it here is what stops a later
    /// "tidy" moving all three boundaries onto one wrong rule.
    ///
    /// ⚠️ **This paragraph said *"22 h is where half-up first gives one day"* for a
    /// commit and that is false** — half-up first gives one day at **12 h**. The
    /// interval is the property that holds for all three bands; the sentence it
    /// replaces held for none of them.
    ///
    /// **Flip:** move the hours band back to `45..=5399` / `5400..` and this fails
    /// at the **2700** row — *"the hours band opens where it can say 1: left
    /// `"Saved · 45m ago"`, right `"Saved · 1h ago"`"*. ⚠️ Predicted wrongly: the
    /// prediction named the `3600` row, on the strength of *"60m ago"* being the
    /// symptom anybody would report. The row that bites is the band's **edge**,
    /// which is the general shape — a boundary defect is caught at the boundary,
    /// and the memorable middle of the band is caught second.
    #[test]
    fn every_band_of_the_save_pill_can_say_one_of_its_unit() {
        let at = |secs| save_label(false, Some(Duration::from_secs(secs)));
        assert_eq!(at(44), "Saved · just now");
        assert_eq!(
            at(45),
            "Saved · 1m ago",
            "the minutes band opens where it can say 1"
        );
        assert_eq!(
            at(2699),
            "Saved · 45m ago",
            "and closes at its own last value"
        );
        assert_eq!(
            at(2700),
            "Saved · 1h ago",
            "the hours band opens where it can say 1"
        );
        assert_eq!(at(3600), "Saved · 1h ago", "an hour-old save says an hour");
        assert_eq!(at(5400), "Saved · 2h ago");
        assert_eq!(at(79_199), "Saved · 22h ago");
        assert_eq!(
            at(79_200),
            "Saved · 1d ago",
            "the band that was already right"
        );
    }

    /// **`layers.rs` is the only panel that commits for itself** (§15 D679,
    /// `[S16.4-L3-06]`).
    ///
    /// 🚨 **This is a grep that nobody was running, made into a gate.** §15 D128
    /// states the enforcement as *"the only `session.commit` left under `panels/`
    /// is in `layers.rs`"* and `commit_edit`'s doc calls the convention *"the whole
    /// guard against the chrome hide becoming a roll-call"* — and the sentence had
    /// been **false**, not merely incomplete, since `inspector::remove_selected_guides`
    /// was written. Nothing anywhere could have said so.
    ///
    /// ⚠️ **`self.session.` rather than `.session.` is the discriminator**, and it
    /// is not cosmetic: every test in these files commits through a *named* app
    /// (`app.session.commit`, `typed.session…`), so the receiver is what tells
    /// production code from a fixture. A predicate that matched both would report
    /// six files and be abandoned as noise.
    ///
    /// ⚠️ **And this test's own text is in a file it does not read.** `app.rs` is
    /// not under `panels/`, so it is out of the scan by construction — but the next
    /// person widening the scan gets a self-match, which is the trap session 17
    /// recorded for `combo_chevron`'s greps.
    ///
    /// **Flip:** put `session.commit` back in `remove_selected_guides` and this
    /// fails naming `inspector.rs`. Predicted correctly.
    #[test]
    fn only_the_layers_panel_commits_without_the_committer() {
        let panels: [(&str, &str); 7] = [
            ("dashboard.rs", include_str!("panels/dashboard.rs")),
            ("export.rs", include_str!("panels/export.rs")),
            ("inspector.rs", include_str!("panels/inspector.rs")),
            ("layers.rs", include_str!("panels/layers.rs")),
            ("paint.rs", include_str!("panels/paint.rs")),
            ("picker.rs", include_str!("panels/picker.rs")),
            ("typography.rs", include_str!("panels/typography.rs")),
        ];
        let committing: Vec<&str> = panels
            .iter()
            .filter(|(_, src)| {
                src.contains("self.session.commit(") || src.contains("self.session.try_commit(")
            })
            .map(|(name, _)| *name)
            .collect();
        assert_eq!(
            committing,
            vec!["layers.rs"],
            "a panel commits around `commit_edit`; name it in that function's doc, \
             in `architecture.md` and in §15 D128, or route it through"
        );

        // 🚨 **The list above is hand-written, so this is what stops it going
        // short.** `assert_eq!(panels.len(), 7)` was here first and is a
        // tautology — a hand-written array of seven has seven entries whatever the
        // directory holds, so a ninth panel would join the tree unscanned and this
        // test would go on passing. `include_str!` resolves at compile time and
        // takes no glob, so the array cannot be generated; what *can* be checked is
        // that it matches the directory, which is read here at run time.
        //
        // `mod.rs` is deliberately absent from the list — it is the module
        // declaration rather than a panel — and is the one name subtracted.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/panels");
        let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
            .expect("the panels directory")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".rs") && n != "mod.rs")
            .collect();
        on_disk.sort();
        let mut listed: Vec<String> = panels.iter().map(|(n, _)| (*n).to_owned()).collect();
        listed.sort();
        assert_eq!(
            listed, on_disk,
            "a panel joined the directory and not this scan, so it is not being \
             checked at all"
        );
    }
}

#[cfg(test)]
mod reveal_tests {
    use super::reveal_axis;

    /// A layer comfortably in frame must not move the world: a 1px arrow-key tap
    /// that jumped the view would make nudging unusable.
    #[test]
    fn something_already_in_view_does_not_scroll() {
        assert_eq!(reveal_axis(10.0, 20.0, 0.0, 100.0), 0.0);
        // Flush against both edges still counts as inside.
        assert_eq!(reveal_axis(0.0, 100.0, 0.0, 100.0), 0.0);
    }

    /// And when it has left, the *minimum* that brings it back — not a centring,
    /// which would throw the view about on every nudge.
    #[test]
    fn a_layer_off_the_edge_scrolls_by_the_least_that_shows_it() {
        assert_eq!(reveal_axis(105.0, 115.0, 0.0, 100.0), 15.0);
        assert_eq!(reveal_axis(-15.0, -5.0, 0.0, 100.0), -15.0);
    }

    /// **The oscillation case, and it resolves to "do nothing".** A layer
    /// overhanging *both* edges already fills the view, so there is nothing to
    /// reveal — and pulling either edge in would push the other out, which the next
    /// nudge would undo. Left: the view flips between the layer's two ends while an
    /// arrow key is held.
    #[test]
    fn a_selection_that_straddles_the_whole_view_does_not_scroll() {
        assert_eq!(reveal_axis(-20.0, 150.0, 0.0, 100.0), 0.0);
        assert_eq!(reveal_axis(-500.0, 500.0, 0.0, 100.0), 0.0);
    }

    /// A layer larger than the view but entirely outside it *does* scroll — as far
    /// as its near edge and no further, which is the most that can be shown.
    #[test]
    fn an_oversized_layer_off_to_one_side_scrolls_to_its_near_edge() {
        let d = reveal_axis(150.0, 400.0, 0.0, 100.0);
        assert_eq!(d, 150.0, "the near edge lands on the near edge of the view");
        let (v0, v1) = (0.0 + d, 100.0 + d);
        assert_eq!(
            reveal_axis(150.0, 400.0, v0, v1),
            0.0,
            "one correction, not a loop"
        );
    }
}

#[cfg(test)]
mod chrome_hold_tests {
    use super::{CHROME_HOLD_SECS, ChromeHold};

    /// `advance` with nothing happening — the shape most of the assertions below
    /// need for their "and then time passes" step.
    fn idle(state: ChromeHold, now: f64) -> ChromeHold {
        state.advance(now, None, false, false)
    }

    /// **A scrub's count starts at the release, not at the press.** Otherwise a drag
    /// longer than the timeout brings the box back on top of the artwork mid-gesture,
    /// which is the one moment it is most in the way.
    #[test]
    fn a_scrub_holds_for_its_whole_length_however_long_that_is() {
        let mut hold = ChromeHold::Free;
        // Frame 1: the drag begins.
        hold = hold.advance(0.0, Some(true), true, false);
        assert_eq!(hold, ChromeHold::Scrubbing);
        // Ten seconds of dragging, well past the timeout, reporting on some frames
        // and not others — a slow speed spends whole frames below one unit.
        for t in [1.0, 2.5, 5.0, 9.9] {
            hold = hold.advance(t, if t > 2.0 { None } else { Some(true) }, true, false);
            assert_eq!(hold, ChromeHold::Scrubbing, "let go at {t}s");
            assert!(hold.holding());
        }
        // The button lifts, and only now does the clock start.
        hold = idle(hold, 10.0);
        assert_eq!(hold, ChromeHold::Until(10.0 + CHROME_HOLD_SECS));
        assert!(hold.holding());
        assert_eq!(idle(hold, 10.0 + CHROME_HOLD_SECS - 0.01), hold);
        assert_eq!(idle(hold, 10.0 + CHROME_HOLD_SECS), ChromeHold::Free);
    }

    /// **Every keystroke restarts the count**, so typing "1", "2", "0" with pauses
    /// between them does not bring the box back part way through a number.
    #[test]
    fn each_typed_keystroke_restarts_the_count() {
        let mut hold = ChromeHold::Free.advance(0.0, Some(false), false, false);
        assert_eq!(hold, ChromeHold::Until(CHROME_HOLD_SECS));
        // A second digit a second later pushes the deadline out rather than being
        // ignored because a hold is already running.
        hold = hold.advance(1.0, Some(false), false, false);
        assert_eq!(hold, ChromeHold::Until(1.0 + CHROME_HOLD_SECS));
        // Which is past where the first one would have expired.
        assert!(idle(hold, CHROME_HOLD_SECS + 0.01).holding());
        assert_eq!(idle(hold, 1.0 + CHROME_HOLD_SECS), ChromeHold::Free);
    }

    /// **The pointer moving over the canvas beats the clock**, which is the better
    /// half of the rule: arriving on the artwork says you are done editing and about
    /// to do something else.
    #[test]
    fn canvas_motion_brings_the_chrome_back_before_the_timeout() {
        let hold = ChromeHold::Free.advance(0.0, Some(false), false, false);
        assert!(hold.holding());
        assert_eq!(
            hold.advance(0.1, None, false, true),
            ChromeHold::Free,
            "a tenth of a second in, the pointer reaching the canvas is enough"
        );
        // And motion *not* on the canvas is not enough — travelling from one field
        // to the next must not bring the box back.
        assert_eq!(hold.advance(0.1, None, false, false), hold);
    }

    /// **Canvas motion must not cancel a scrub in flight**, which the design did not
    /// say and the mechanism demands: a long drag wraps the pointer round the screen
    /// edge (`ui::wrap_scrub`) and crosses the canvas on the way, so motion there is
    /// the gesture rather than a change of mind.
    #[test]
    fn a_scrub_that_crosses_the_canvas_keeps_the_chrome_away() {
        let mut hold = ChromeHold::Free.advance(0.0, Some(true), true, false);
        assert_eq!(hold, ChromeHold::Scrubbing);
        hold = hold.advance(0.2, Some(true), true, true);
        assert_eq!(
            hold,
            ChromeHold::Scrubbing,
            "the drag was cancelled by its own travel across the canvas"
        );
        // Once it lets go, the same motion counts again.
        hold = idle(hold, 0.3);
        assert_eq!(hold.advance(0.4, None, false, true), ChromeHold::Free);
    }

    /// Nothing happening leaves the chrome alone, in both directions — the state a
    /// wrongly-written machine drifts out of on an idle frame.
    #[test]
    fn an_idle_frame_changes_nothing() {
        assert_eq!(idle(ChromeHold::Free, 0.0), ChromeHold::Free);
        assert_eq!(idle(ChromeHold::Free, 99.0), ChromeHold::Free);
        assert!(!ChromeHold::Free.holding());
        // Including with a button down somewhere else in the app: a canvas drag must
        // not be mistaken for a scrub that has yet to end.
        assert_eq!(
            ChromeHold::Free.advance(1.0, None, true, false),
            ChromeHold::Free
        );
    }
}

/// `edit_note` against real value fields, because it is the half of the chrome
/// hide that shipped wrong and the failure was invisible in the code: every
/// expression read correctly and the box vanished a second and a half after the hand
/// stopped moving.
///
/// The decision is a free function over a `Response` and a `Transaction` — the two
/// things a headless pass can produce — so the note can be asserted without a value
/// field being dragged to produce one.
#[cfg(test)]
mod edit_note_tests {
    use super::{Transaction, edit_note};
    use crate::theme;
    use crate::ui::{self, Prefix, Scrub};
    use ondin_core::Operation;
    use ondin_core::kurbo::Affine;

    /// The transaction a Transform field offers — built unconditionally by
    /// `inspector::place_at`, whether or not the number moved. That is the whole
    /// reason the response has to be consulted.
    fn moves_a_layer() -> Transaction {
        Transaction(vec![Operation::SetTransform {
            id: ondin_core::IdSource::new(1).mint(),
            transform: Affine::IDENTITY,
        }])
    }

    fn renames_a_layer() -> Transaction {
        Transaction(vec![Operation::SetName {
            id: ondin_core::IdSource::new(1).mint(),
            name: "x".into(),
        }])
    }

    /// One pass over a `value_field`, returning what `edit_note` makes of it and the
    /// field's painted ground for the next pass to aim at.
    fn pass(
        ctx: &egui::Context,
        v: &mut f64,
        events: Vec<egui::Event>,
    ) -> (Option<bool>, egui::Rect) {
        let mut note = None;
        let mut rect = egui::Rect::NOTHING;
        let out = ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                let resp = ui::value_field(
                    ui,
                    egui::vec2(120.0, 28.0),
                    Prefix::Text("X"),
                    v,
                    Scrub::whole(0.5),
                    |d| d.max_decimals(2),
                );
                note = edit_note(Some(&resp), &moves_a_layer());
            },
        );
        // The painted ground, as `ui::scrub_tests` takes it: the response is a union
        // that shrinks if the prefix strip stops sensing.
        for c in &out.shapes {
            if let egui::Shape::Rect(r) = &c.shape
                && (r.rect.height() - 28.0).abs() < 0.51
            {
                rect = r.rect;
            }
        }
        (note, rect)
    }

    fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// **A field nobody is touching is not an edit**, and this is the regression.
    ///
    /// `edit_valve` is called every frame with a transaction built every frame, so
    /// without the response half the note fired for ever, `ChromeHold` restarted its
    /// count for ever, and the selection box was away for as long as the Transform
    /// panel was on screen. Reported as "once I select a layer the layer chrome always
    /// hides unless I move the mouse" — canvas motion being the one rule left that
    /// could clear a hold that was being renewed every frame.
    #[test]
    fn a_resting_field_notes_nothing_however_many_frames_pass() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut v = 100.0;
        for frame in 0..10 {
            let (note, _) = pass(&ctx, &mut v, Vec::new());
            assert_eq!(note, None, "frame {frame}: a field at rest noted an edit");
        }
        assert_eq!(v, 100.0, "and the fixture did not move the value");
    }

    /// Hovering is not editing either — the pointer sitting on a field is the state
    /// the hand passes through on its way to every real edit.
    #[test]
    fn hovering_a_field_notes_nothing() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut v = 100.0;
        pass(&ctx, &mut v, Vec::new());
        let (_, rect) = pass(&ctx, &mut v, Vec::new());
        let at = rect.center();
        let (note, _) = pass(&ctx, &mut v, vec![egui::Event::PointerMoved(at)]);
        assert_eq!(note, None);
        let (note, _) = pass(&ctx, &mut v, Vec::new());
        assert_eq!(note, None, "and it stays nothing while the pointer rests");
    }

    /// A drag reports itself **as a drag**, from the frame the button goes down —
    /// which is what gives its hold no deadline until the button comes up.
    #[test]
    fn a_drag_notes_itself_as_a_drag_and_its_release_notes_nothing() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut v = 100.0;
        pass(&ctx, &mut v, Vec::new());
        let (_, rect) = pass(&ctx, &mut v, Vec::new());
        let from = rect.center();
        pass(
            &ctx,
            &mut v,
            vec![egui::Event::PointerMoved(from), button(from, true)],
        );
        let to = egui::pos2(from.x + 30.0, from.y);
        let (note, _) = pass(&ctx, &mut v, vec![egui::Event::PointerMoved(to)]);
        assert_eq!(note, Some(true), "a drag in flight");
        assert_ne!(v, 100.0, "the fixture did not actually scrub");
        let (note, _) = pass(&ctx, &mut v, vec![button(to, false)]);
        assert_eq!(
            note, None,
            "the release notes nothing, which is why the pointer is what ends a scrub"
        );
    }

    /// Typing a digit notes a **typed** edit, so the count starts at once rather than
    /// waiting for a button that is not down.
    #[test]
    fn typing_a_digit_notes_a_typed_edit() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut v = 100.0;
        pass(&ctx, &mut v, Vec::new());
        let (_, rect) = pass(&ctx, &mut v, Vec::new());
        let at = rect.center();
        // A click with no travel puts the `DragValue` into its keyboard face.
        pass(
            &ctx,
            &mut v,
            vec![egui::Event::PointerMoved(at), button(at, true)],
        );
        let (note, _) = pass(&ctx, &mut v, vec![button(at, false)]);
        assert_eq!(note, None, "focusing a field to read it is not editing it");
        let (note, _) = pass(&ctx, &mut v, vec![egui::Event::Text("7".into())]);
        assert_eq!(note, Some(false), "a digit that moved the value");
        assert_eq!(v, 7.0, "the fixture did not actually type");
    }

    /// The other half: an edit that changes nothing **drawn** is not worth taking the
    /// box away for, whatever the control did — and a discrete commit (`None`) needs no
    /// interaction test, because it is only reached once something has changed.
    #[test]
    fn an_invisible_edit_notes_nothing_and_a_discrete_one_always_does() {
        assert_eq!(edit_note(None, &renames_a_layer()), None);
        assert_eq!(edit_note(None, &Transaction(Vec::new())), None);
        assert_eq!(
            edit_note(None, &moves_a_layer()),
            Some(false),
            "a button, a swatch, a menu row: instantaneous, so the count starts now"
        );
    }
}

#[cfg(test)]
mod guide_copy_tests {
    use super::*;
    use ondin_core::{GuideAxis, IdSource};

    /// A copy of a guide keeps everything about it except where it is — the axis
    /// it runs on, the colour someone chose for it, and **the frame it belongs
    /// to** — and steps along its own axis so the copy is visibly a second line.
    ///
    /// The scope is the half worth pinning: a guide down the middle of a card
    /// belongs to that card, and a paste that re-derived the owner from wherever
    /// the pointer happened to be would answer a question nobody asked.
    #[test]
    fn a_pasted_guide_keeps_its_axis_colour_and_scope() {
        let mut ids = IdSource::new(0x9A57E);
        let frame = ids.mint();
        let red = ondin_core::peniko::Color::from_rgba8(200, 30, 60, 255);

        let scoped = Guide {
            id: GuideId(ids.mint()),
            axis: GuideAxis::Vertical,
            position: 120.0,
            color: Some(red),
            owner: Some(frame),
        };
        let fresh = GuideId(ids.mint());
        let copy = pasted_guide(&scoped, fresh, scoped.owner, GUIDE_PASTE_OFFSET);
        assert_eq!(copy.id, fresh, "a copy is its own guide");
        assert_eq!(copy.axis, GuideAxis::Vertical);
        assert_eq!(
            copy.color,
            Some(red),
            "a recoloured guide copies recoloured"
        );
        assert_eq!(copy.owner, Some(frame), "and stays in its frame");
        assert_eq!(copy.position, 120.0 + GUIDE_PASTE_OFFSET);

        // A global guide copies global, and the step is on its own axis whichever
        // that is — the position is one number, so there is only one place to add.
        let global = Guide {
            id: GuideId(ids.mint()),
            axis: GuideAxis::Horizontal,
            position: -40.5,
            color: None,
            owner: None,
        };
        let copy = pasted_guide(&global, GuideId(ids.mint()), None, GUIDE_PASTE_OFFSET);
        assert_eq!(copy.owner, None);
        assert_eq!(
            copy.color, None,
            "still following the default, not frozen to it"
        );
        assert_eq!(copy.position, -40.5 + GUIDE_PASTE_OFFSET);
    }

    /// **Paste resolves the scope, and that is the caller's half of the rule**: a
    /// guide whose frame was deleted between the copy and the paste falls back to
    /// the canvas rather than being dropped, so a paste never silently loses one.
    /// `pasted_guide` honours whatever it is handed, which is what lets paste and
    /// duplicate differ on exactly this point and nowhere else.
    #[test]
    fn a_pasted_guide_whose_frame_is_gone_lands_on_the_canvas() {
        let mut ids = IdSource::new(0x9A57E);
        let gone = ids.mint();
        let orphan = Guide {
            id: GuideId(ids.mint()),
            axis: GuideAxis::Vertical,
            position: 60.0,
            color: None,
            owner: Some(gone),
        };
        // What `paste_guides` computes when `doc.contains(gone)` is false.
        let copy = pasted_guide(&orphan, GuideId(ids.mint()), None, GUIDE_PASTE_OFFSET);
        assert_eq!(copy.owner, None);
        // The number is carried across unchanged rather than reprojected: it was a
        // frame-local offset and is now a world coordinate, which is the honest
        // consequence of the frame being gone — there is nothing left to measure
        // the offset against.
        assert_eq!(copy.position, 60.0 + GUIDE_PASTE_OFFSET);
    }

    /// The step is `PASTE_OFFSET`'s own component, so a guide and a layer move
    /// by the same amount and the two gestures do not read as different features.
    ///
    /// That it is *non-zero* is asserted at the constant instead, as a `const`
    /// block: it is a property of the constant, so the useful place to fail is the
    /// edit that breaks it rather than a test run afterwards.
    #[test]
    fn a_guide_and_a_layer_step_by_the_same_amount() {
        assert_eq!(GUIDE_PASTE_OFFSET, PASTE_OFFSET.x);
    }

    /// ***Paste here* aims from the box the copy was taken from, and after a cut
    /// that box is the only thing left** (§15 D251).
    ///
    /// Reported after living with it: "if I cut a layer, then Paste here, it will be
    /// pasted to its original position — and if I continue doing Paste here, all
    /// copies land in the same spot, acting basically like a Paste in place." Every
    /// clause is one cause. The aim used to be read back off the *source ids*, which
    /// a cut has deleted, so it fell to the fixed 20×20 step — and because the
    /// templates keep the original ids however many times they are pasted, it kept
    /// falling there forever. Copy-then-paste-here worked, which is why it survived:
    /// the originals are still in the tree.
    ///
    /// Pinned here rather than through the app: what makes the bug unreachable is
    /// that `paste_aim` takes a `Rect`, so there is nothing to look up. The
    /// assertions are that a recorded box lands the payload's centre exactly on the
    /// pointer — the same answer whether or not the source still exists, since the
    /// function cannot tell — and that the fallback survives for a payload that has
    /// no box at all.
    ///
    /// ⚠️ The flip is the old shape and cannot be spelled here at all, which is the
    /// point: to reproduce it you have to give the function a document and some ids.
    /// What *can* be flipped is the sign, and `world - centre` against
    /// `centre - world` puts the paste as far the wrong side of the original as the
    /// pointer was the right side.
    #[test]
    fn paste_here_aims_from_the_recorded_box_and_not_from_the_layers() {
        // Offset from the origin on both axes, so a centre confused with a corner
        // — or with the origin — cannot pass.
        let from = Rect::new(100.0, 40.0, 140.0, 60.0);
        assert_eq!(from.center(), Point::new(120.0, 50.0));

        let pointer = Point::new(300.0, 200.0);
        let aim = paste_aim(Some(from), pointer);
        assert_eq!(aim, Vec2::new(180.0, 150.0));
        // Which is the property that matters: the copy's centre ends up under the
        // pointer, wherever it started.
        assert_eq!(from.center() + aim, pointer);

        // A pointer *inside* the source box aims backwards, which a clamp or an
        // `abs` somewhere would quietly break.
        assert_eq!(
            paste_aim(Some(from), Point::new(110.0, 45.0)),
            Vec2::new(-10.0, -5.0)
        );

        // No box: the chord's own step, so the row still puts a visible copy down.
        assert_eq!(paste_aim(None, pointer), PASTE_OFFSET);
    }

    /// **In place means in place, for a guide as much as for a layer** (§15 D248).
    ///
    /// The step is a parameter for exactly one reason — so `paste_in_place` can pass
    /// none — and a guide's axis is the only coordinate it has, so "same position" is
    /// a step of zero and nothing else. The tempting wrong version is a small nudge
    /// "so you can see it landed", which is the ordinary paste wearing a second
    /// name; the rest of the guide comes across untouched either way, so nothing
    /// about the copy would look wrong.
    ///
    /// Which *caller* passes zero is read rather than reproduced, and **that one is
    /// deliberate rather than owed** (§15 D303, where it was queued and then
    /// declined). `paste_in_place` reaches `paste_guides` through
    /// `Self::owns_the_clipboard`, which compares a receipt against the system
    /// clipboard. 🚨 **That argument used to read "a test of the wiring would
    /// read, and could disturb, whatever the person running it had copied", and
    /// since §15 D798 it is false** — a headless app cannot reach the clipboard
    /// at all. The verdict is unchanged and the reason is now **stronger**:
    /// `owns_the_clipboard` is asked of `system_clipboard_text()`, which under
    /// `CLIPBOARD_OFF` is `None`, so the answer is permanently `false` (the
    /// receipt is a digest of the text since §15 D857, which changes nothing
    /// here) and that wiring is
    /// unreachable from a headless test **by construction** rather than merely
    /// non-deterministic. The three call sites pass a named constant or a literal
    /// `0.0` and the compiler checks both; what is worth asserting is that a zero
    /// step is exact, and that is this test.
    #[test]
    fn a_guide_pasted_in_place_keeps_its_own_number() {
        let mut ids = IdSource::new(0x9A57E);
        let guide = Guide {
            id: GuideId(ids.mint()),
            axis: GuideAxis::Horizontal,
            position: -40.5,
            color: None,
            owner: None,
        };
        let copy = pasted_guide(&guide, GuideId(ids.mint()), None, 0.0);
        assert_eq!(
            copy.position, -40.5,
            "a step of zero is the whole of paste in place"
        );
        assert_ne!(copy.id, guide.id, "and it is still its own guide");
    }

    /// **A locked paste says how many guides it pasted** (§15 D680,
    /// `[S16.2-L1-06]`).
    ///
    /// The count came off `selection.guides()` *after* `select_pasted_guides`,
    /// whose first line is `if self.lock_guides { return; }`. That early return is
    /// the correct rule — locked means unselectable, so selecting a fresh copy
    /// would make a guide that is selected and unpickable — and it is exactly what
    /// stranded the count. With guides locked, a paste that added three of them and
    /// committed said **"Pasted 0 guide(s)"**.
    ///
    /// ⚠️ **The unlocked case is the control and it is why the bug survived**: with
    /// guides unlocked the selection *is* the paste, so the wrong expression gives
    /// the right number, and every ordinary use of the feature looks correct.
    ///
    /// **Flip:** put `self.session.selection.guides().len()` back and the locked
    /// case fails with *"Pasted 0 guide(s)"*. Predicted correctly.
    #[test]
    fn a_paste_counts_the_guides_it_made_even_when_they_cannot_be_selected() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);

        for locked in [true, false] {
            let mut app = OndinApp::headless(&ctx);
            app.lock_guides = locked;
            let mut ids = IdSource::new(0x6C0C);
            app.guide_clipboard = Some(
                [40.0, 90.0, 150.0]
                    .into_iter()
                    .map(|position| Guide {
                        id: GuideId(ids.mint()),
                        axis: GuideAxis::Vertical,
                        position,
                        color: None,
                        owner: None,
                    })
                    .collect(),
            );
            assert!(app.paste_guides(GUIDE_PASTE_OFFSET), "the paste ran");
            assert_eq!(
                app.session.status().text,
                "Pasted 3 guide(s)",
                "locked = {locked}"
            );
            assert_eq!(
                app.session.doc.guides().len(),
                3,
                "and the guides really are in the document, locked = {locked}"
            );
        }
    }
}

/// The two-digit opacity window (`docs/shortcuts.md` §2).
///
/// `opacity_step` is pure so it can be driven here — the window is a *clock*
/// question, and a test that reached it through real keystrokes would be waiting on
/// `Instant` rather than asserting on the machine.
#[cfg(test)]
mod opacity_digit_tests {
    use super::{
        OPACITY_DIGIT_WINDOW, OpacityEntry, OpacityStep, opacity_of_digit, opacity_step,
        opacity_window_open,
    };

    /// A lone digit is the coarse value, and `0` is the exception that makes the
    /// binding usable: 10%–90% plus 100%, with no digit left over for 0%.
    #[test]
    fn a_lone_digit_is_tens_except_zero_which_is_full() {
        assert_eq!(opacity_of_digit(1), 10.0);
        assert_eq!(opacity_of_digit(4), 40.0);
        assert_eq!(opacity_of_digit(9), 90.0);
        assert_eq!(opacity_of_digit(0), 100.0);
    }

    /// The first digit **waits** rather than committing, and the second lands the
    /// exact value.
    ///
    /// The waiting is the whole mechanism: `4`,`5` has to leave one history step
    /// reading 45%, so the 40% the user sees while typing must be a preview. A
    /// version that committed both would pass any test that only checked the
    /// final number.
    #[test]
    fn two_digits_inside_the_window_are_one_exact_value() {
        let one = [ondin_core::IdSource::new(1).mint()];
        let OpacityStep::Wait(pct, entry) = opacity_step(None, 4, 10.0, &one) else {
            panic!("a first digit must wait for a second, not commit");
        };
        assert_eq!(pct, 40.0, "and show 40% while it waits");
        assert_eq!(
            opacity_step(Some(entry), 5, 10.0 + OPACITY_DIGIT_WINDOW / 2.0, &one),
            OpacityStep::Land(45.0)
        );
    }

    /// **A digit typed against a different layer starts over** (§15 D470).
    ///
    /// `[S16.1-L1-02]`'s pairing half. Select A, press `4`, click B, press `5`:
    /// that is not `45%` on anything, and reading the window alone made it `45%`
    /// on B. The subject is part of the pending entry now, so a moved selection
    /// begins a new value — which is what the second keystroke means.
    ///
    /// ⚠️ **Flipped** by dropping `entry.subject == subject` from the pairing
    /// arm: fails on the second assertion with `Land(45.0)`. The control above,
    /// which never moves the subject, stays green — so this is about the subject
    /// and not about the window.
    #[test]
    fn a_digit_typed_against_a_different_layer_does_not_pair() {
        let mut ids = ondin_core::IdSource::new(1);
        let (a, b) = ([ids.mint()], [ids.mint()]);
        let OpacityStep::Wait(_, entry) = opacity_step(None, 4, 10.0, &a) else {
            panic!("the first digit waits");
        };
        // Same instant, well inside the window; only the subject moved.
        let now = 10.0 + OPACITY_DIGIT_WINDOW / 2.0;
        assert!(
            matches!(
                opacity_step(Some(entry.clone()), 5, now, &b),
                OpacityStep::Wait(50.0, _)
            ),
            "a digit typed against another layer is a new value, not the second \
             half of one typed against the first"
        );
        assert_eq!(
            opacity_step(Some(entry), 5, now, &a),
            OpacityStep::Land(45.0),
            "control: against the same layer it still pairs"
        );
    }

    /// `0`,`0` is the way to 0% — the value no lone digit can reach.
    #[test]
    fn zero_twice_is_fully_transparent() {
        let one = [ondin_core::IdSource::new(1).mint()];
        let OpacityStep::Wait(pct, entry) = opacity_step(None, 0, 0.0, &one) else {
            panic!("the first zero must wait too");
        };
        assert_eq!(
            pct, 100.0,
            "a lone 0 is 100%, and that is what is previewed"
        );
        assert_eq!(
            opacity_step(Some(entry), 0, 0.1, &one),
            OpacityStep::Land(0.0)
        );
        // And `0`,`5` is 5%, not 50% — the pair is read as written.
        let OpacityStep::Wait(_, entry) = opacity_step(None, 0, 0.0, &one) else {
            unreachable!()
        };
        assert_eq!(
            opacity_step(Some(entry), 5, 0.1, &one),
            OpacityStep::Land(5.0)
        );
    }

    /// **Once the window has run out, the next digit starts a new value.**
    ///
    /// Sampled a hair either side rather than on the boundary itself: `10.6 -
    /// 10.0` is `0.5999999999999996`, so *which* side of `<` that exact instant
    /// falls on is a float artefact rather than a decision anyone made. Asserting
    /// it would be asserting arithmetic.
    #[test]
    fn a_digit_after_the_window_begins_a_new_value() {
        let one = vec![ondin_core::IdSource::new(1).mint()];
        let entry = OpacityEntry {
            first: 4,
            at: 10.0,
            subject: one.clone(),
        };
        assert_eq!(
            opacity_step(
                Some(entry.clone()),
                5,
                10.0 + OPACITY_DIGIT_WINDOW - 0.01,
                &one
            ),
            OpacityStep::Land(45.0),
            "a hair inside the window still pairs"
        );
        assert!(
            matches!(
                opacity_step(Some(entry), 5, 10.0 + OPACITY_DIGIT_WINDOW + 0.01, &one),
                OpacityStep::Wait(50.0, _)
            ),
            "a hair outside it the pending digit is already committed, so this \
             one must start over at 50% rather than land 45%"
        );
    }

    /// **The pairing rule and the commit deadline read the boundary the same
    /// way** — which is the property that actually matters, and the one the
    /// exact-boundary assertion above cannot state.
    ///
    /// They are two separate `now - at` comparisons until `opacity_window_open`
    /// is the thing both of them call. If they ever drift apart there is an
    /// instant at which `opacity_step` folds a digit into a pair that
    /// `fold_opacity_entry` has already committed on its own: one keystroke, two
    /// history steps, and the second one a number nobody typed. Swept across the
    /// boundary far more finely than the window, so no drift of a realistic size
    /// could fall between two samples.
    #[test]
    fn the_pairing_rule_and_the_commit_deadline_agree_everywhere() {
        let one = vec![ondin_core::IdSource::new(1).mint()];
        let entry = OpacityEntry {
            first: 4,
            at: 10.0,
            subject: one.clone(),
        };
        for step in 0..2000 {
            let now = 10.0 + f64::from(step) * OPACITY_DIGIT_WINDOW / 1000.0;
            let pairs = matches!(
                opacity_step(Some(entry.clone()), 5, now, &one),
                OpacityStep::Land(_)
            );
            assert_eq!(
                pairs,
                opacity_window_open(&entry, now),
                "at {now}, a second digit pairs iff the window is still open"
            );
        }
    }
}

#[cfg(test)]
mod cancelled_opacity_digit_tests {
    //! **A cancelled opacity digit does not commit itself when the window runs
    //! out** — `[S16.2-L1-01]`, §15 D535.
    //!
    //! Press `4`, see the canvas preview 40%, press `Escape`, watch it revert —
    //! and 600 ms later the layer became 40% anyway, with an undo step and a
    //! dirty document behind it. The cancel *fired*: `set_opacity_pct_on`
    //! previews through `EditorSession::set_preview`, so `has_gesture_preview` is
    //! true, `cancel_gesture` is the rung `escape` stops on, and it cleared the
    //! preview correctly. What it never cleared was `opacity_entry`, and
    //! `fold_opacity_entry` runs unconditionally on every editor frame.
    //!
    //! ⚠️ **The window is a gesture with nothing held down**, which is why the
    //! whole test is about the clock and not about the pointer. The deadline is
    //! the *only* thing that reveals the bug: every observation taken before it
    //! agrees between the fixed and the broken app, so a test that stopped at
    //! "the preview reverted" would have been green against both.
    //!
    //! **`escape`, not `cancel_gesture`, is what these drive**, because the
    //! finding's claim is about the rung the ladder stops on — and the
    //! right-click's `cancel_gesture` is the same function by the other door.
    //!
    //! (Plain backticks rather than `[links]` — §15 D319's convention: `cargo
    //! doc` builds without the `test` cfg, so a link here is decoration no gate
    //! can validate.)
    use super::{OPACITY_DIGIT_WINDOW, OndinApp};
    use ondin_core::kurbo::Size;
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    /// A headless app holding one opaque 80×60 rect, selected.
    fn app_with_a_rect(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x0D5);
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
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![rect]);
        (app, rect)
    }

    /// The one frame the clock is read from. `opacity_digit` and
    /// `fold_opacity_entry` both take `now` from `ctx.input(|i| i.time)`, which is
    /// last frame's, so the time has to be delivered by a frame rather than passed.
    fn at(ctx: &egui::Context, time: f64) {
        let _ = ctx.run_ui(
            egui::RawInput {
                time: Some(time),
                ..Default::default()
            },
            |_| {},
        );
    }

    fn opacity(app: &OndinApp, id: NodeId) -> f32 {
        app.session.doc.get(id).expect("the rect").opacity()
    }

    /// **`Escape` shuts the digit window without committing it.**
    ///
    /// The three observations are the finding's own table: the digit previews and
    /// leaves the document alone, the cancel clears the preview and leaves the
    /// document alone, and — the assertion that is the whole point — the deadline
    /// passes and *still* leaves the document alone.
    ///
    /// ⚠️ **Flipped** by removing `self.opacity_entry = None;` from
    /// `cancel_gesture`: fails at the predicted site, the post-deadline opacity,
    /// `0.4` against `1.0`. Predicted correctly for once, which is worth saying
    /// because three of this fix phase's predictions have been wrong — the reason
    /// it is easy here is that the two earlier assertions are *identical* in both
    /// versions, so there is only one site it can bite at.
    ///
    /// ⚠️ **The control is the uncancelled digit and it is not decoration.**
    /// Flipped by deleting `fold_opacity_entry`'s commit — the
    /// `set_opacity_pct_on(&entry.subject, …)` on its last line, replaced by a
    /// discard of both arguments — which is the *plausible* wrong fix here, since
    /// "stop the deadline committing" closes this finding and removes the feature
    /// in one line. **This test stays green under it** and the control goes red at
    /// `1.0` against `0.4`. The pair is what says the fix shut the window rather
    /// than nailing it shut.
    #[test]
    fn a_cancelled_digit_does_not_land_when_the_window_runs_out() {
        let ctx = egui::Context::default();
        let (mut app, rect) = app_with_a_rect(&ctx);
        let depth0 = app.session.history.undo_depth();

        at(&ctx, 10.0);
        app.opacity_digit(&ctx, 4);
        assert!(
            app.opacity_entry.is_some(),
            "fixture: a lone digit waits — without a pending entry this test is \
             about nothing"
        );
        assert!(
            app.session.has_gesture_preview(),
            "fixture: and it waits as a *gesture* preview, which is what makes \
             `cancel_gesture` the rung `escape` stops on"
        );
        assert_eq!(opacity(&app, rect), 1.0, "the first digit is a preview");

        app.escape(&ctx);
        assert!(
            !app.session.has_gesture_preview(),
            "the cancel took the 40% off the canvas"
        );
        assert_eq!(opacity(&app, rect), 1.0, "and the document never had it");

        // Past the deadline, which is the only instant the two versions differ at.
        at(&ctx, 10.0 + OPACITY_DIGIT_WINDOW + 0.1);
        app.fold_opacity_entry(&ctx);
        assert_eq!(
            opacity(&app, rect),
            1.0,
            "a cancelled gesture owes zero transactions, and the deadline is not a \
             third way for this one to commit"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            depth0,
            "and spends no undo step"
        );
        assert!(
            !app.session.is_dirty(),
            "nor an autosave and a crash-snapshot write"
        );
    }

    /// **Control: an *uncancelled* digit still lands on the deadline.**
    ///
    /// The same events with the `escape` taken out. Without this the fix above is
    /// indistinguishable from deleting the feature — which is the failure mode
    /// `[S16.2-L1-01]`'s own flip pair was built to rule out, kept here as a
    /// standing assertion rather than as a one-off measurement.
    #[test]
    fn an_uncancelled_digit_still_lands_on_the_deadline() {
        let ctx = egui::Context::default();
        let (mut app, rect) = app_with_a_rect(&ctx);
        let depth0 = app.session.history.undo_depth();

        at(&ctx, 10.0);
        app.opacity_digit(&ctx, 4);
        at(&ctx, 10.0 + OPACITY_DIGIT_WINDOW + 0.1);
        app.fold_opacity_entry(&ctx);

        assert_eq!(opacity(&app, rect), 0.4, "the coarse value lands");
        assert_eq!(
            app.session.history.undo_depth(),
            depth0 + 1,
            "as one undo step (§9.3: one gesture, one transaction)"
        );
    }
}

/// Which way `Ctrl+Shift+H` and `Ctrl+Shift+L` take a whole selection
/// (`docs/shortcuts.md` §4).
#[cfg(test)]
mod set_switch_tests {
    use super::OndinApp;

    /// The three states, and **mixed is the one this test exists for.**
    ///
    /// Uniform-on and uniform-off are the obvious half and no implementation gets
    /// them wrong. Mixed is the decision: it went to *release* first and was
    /// flipped to *apply* on review, so this is the assertion that stops it
    /// drifting back — either by someone re-arguing it, or by an innocent-looking
    /// `all` → `any` while reading the call sites.
    #[test]
    fn a_mixed_selection_applies_the_verb_rather_than_releasing_it() {
        assert!(
            OndinApp::set_switch([false, false, false]),
            "nothing applied yet: the switch applies"
        );
        assert!(
            OndinApp::set_switch([true, false, true]),
            "mixed applies — Hide is a verb, not a toggle, and this is the case \
             that was flipped"
        );
        assert!(
            !OndinApp::set_switch([true, true, true]),
            "everything already applied is the only case that releases"
        );
        // Two members, which is the smallest mixed selection there is.
        assert!(OndinApp::set_switch([true, false]));
        assert!(OndinApp::set_switch([false, true]));
    }

    /// The same three states through the **two real entry points**, in the
    /// natural sense of each field.
    ///
    /// The first version of this test undid the polarity itself —
    /// `set_switch(visible.map(|v| !v))` — and so was passed just as well by a
    /// `toggle_hidden` that had dropped the negation: it re-implemented the call
    /// site instead of testing it. Caught by flipping the call site and watching
    /// it stay green. Reading `hide_switch`/`lock_switch` is what makes the
    /// inversion the thing under test rather than a step the test performs.
    #[test]
    fn the_two_switches_read_their_own_fields() {
        for (visible, want) in [
            (vec![true, true], false),  // all shown -> hide
            (vec![true, false], false), // mixed -> hide all
            (vec![false, false], true), // all hidden -> show
        ] {
            assert_eq!(
                OndinApp::hide_switch(visible.iter().copied()),
                want,
                "hide switch over visible={visible:?}"
            );
        }
        for (locked, want) in [
            (vec![false, false], true), // all free -> lock
            (vec![true, false], true),  // mixed -> lock all
            (vec![true, true], false),  // all locked -> unlock
        ] {
            assert_eq!(
                OndinApp::lock_switch(locked.iter().copied()),
                want,
                "lock switch over locked={locked:?}"
            );
        }
    }
}

#[cfg(test)]
mod image_edit_lock_tests {
    //! **A lock refuses every editing mode, and a locked group refuses for what is
    //! inside it** (§15 D320 for the picture, D321 for the text, the points and the
    //! group rule).
    //!
    //! Two decisions are pinned here. A crop is a *resize* — `crop_resize_tx` writes
    //! through `tools::resize_layer` — so the lock reaches it exactly as it has
    //! always reached *Original size*, the row beside *Edit image* in the same menu
    //! group, which disagreed with it for as long as both existed. And **a locked
    //! group locks its contents**, in the maintainer's words *"an expected behavior,
    //! not a good to have"*, which is why the refusals ask
    //! `query::is_effectively_locked` rather than a node's own flag.
    //!
    //! ⚠️ **`Enter` is the door these tests drive, because it is the only one left.**
    //! `query::hit_test` already refuses a double-click on a locked node at any
    //! depth, and the menu rows dim — so a test that only checked the menu would
    //! have passed against every one of the three bugs this module covers.
    use super::OndinApp;
    use crate::tools::Tool;
    use ondin_core::kurbo::Size;
    use ondin_core::{
        Document, Fill, IdSource, NodeId, NodeKind, Operation, Transaction, image_brush,
    };

    /// A headless app holding one 80×60 rect painted with a picture, selected, with
    /// the Select tool armed — and locked if asked.
    fn app_with_a_picture(ctx: &egui::Context, locked: bool) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0xC1);
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
            fills: vec![Fill {
                brush: image_brush(ondin_core::ImageId("a-photo".into())),
                visible: true,
            }],
        }]))
        .expect("paint");
        if locked {
            doc.apply(&Transaction(vec![Operation::SetLocked {
                id: rect,
                locked: true,
            }]))
            .expect("lock");
        }
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(rect);
        app.tool = Tool::Select;
        (app, rect)
    }

    /// **The funnel refuses, so every door does** — and the fixture is asserted
    /// before the claim, because a rect with no picture would refuse for a reason
    /// that has nothing to do with the lock and this test would pass on it.
    ///
    /// `begin_image_edit` is driven directly *and* through `enter_action`, which is
    /// the door that was live before this landed: `Enter` guarded on
    /// `croppable_fill` alone. The other three doors are the same call — the menu
    /// row and the fill row both reach `begin_image_edit`, and the canvas
    /// double-click never arrives at all, `query::hit_test` skipping a locked node
    /// at any depth — so one refusal covers them and the menu row's dim is only its
    /// visible half (`menu::…::a_lock_dims_both_of_the_picture_rows`).
    ///
    /// ⚠️ **Flipped by deleting the guard in `begin_image_edit`**: the locked layer
    /// arms `ImageEdit` through both the direct call and `Enter`, failing the two
    /// assertions that name it. Flipped again by making the guard unconditional,
    /// which fails the control — without that arm this passes against a mode nobody
    /// can enter at all, which is the vacuous version and the easy one to ship.
    #[test]
    fn a_locked_picture_refuses_every_door() {
        let ctx = egui::Context::default();

        // The control first: unlocked, the mode opens. If this ever stops being
        // true the assertions below stop meaning anything.
        let (mut open, id) = app_with_a_picture(&ctx, false);
        assert!(
            crate::tools::croppable_fill(
                open.session.doc.get(id).expect("the fixture layer").paint()
            )
            .is_some(),
            "the fixture must actually carry a croppable picture, or every refusal \
             below is about the paint rather than the lock"
        );
        open.begin_image_edit(id);
        assert_eq!(
            open.tool,
            Tool::ImageEdit,
            "an unlocked picture enters the mode"
        );

        // Locked, by the same door.
        let (mut shut, id) = app_with_a_picture(&ctx, true);
        shut.begin_image_edit(id);
        assert_eq!(
            shut.tool,
            Tool::Select,
            "a locked picture does not enter the mode — a crop is a resize, and the \
             lock reaches a resize"
        );

        // And by `Enter`, which is the door that was live: its arm asks
        // `croppable_fill` and nothing else, so it refuses only because the funnel
        // does.
        let (mut chord, _) = app_with_a_picture(&ctx, true);
        chord.enter_action();
        assert_eq!(
            chord.tool,
            Tool::Select,
            "`Enter` on a locked picture selected in the layers panel is the door \
             this closed, and it closes in the mode rather than in the arm"
        );
    }

    /// A headless app holding a group with one child of `kind` in it, the child
    /// selected, `Select` armed — and the **group** locked if asked, never the child.
    ///
    /// Locking the *parent* is the whole point: the child's own flag stays clear
    /// throughout, so every refusal below is the ancestor walk and nothing else.
    fn app_with_a_child_in_a_group(
        ctx: &egui::Context,
        kind: NodeKind,
        lock_the_group: bool,
    ) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0xD1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let group = ids.mint();
        let child = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: child,
                parent: group,
                index: 0,
                kind,
                transform: None,
                name: None,
            },
        ]))
        .expect("build the group");
        if lock_the_group {
            doc.apply(&Transaction(vec![Operation::SetLocked {
                id: group,
                locked: true,
            }]))
            .expect("lock the group");
        }
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(child);
        app.tool = Tool::Select;
        (app, child)
    }

    /// **Text and points refuse too, and this is the pair that was the *worse*
    /// direction** (§15 D321): both menu rows dimmed on a locked layer while `Enter`
    /// opened the mode on the same layer, so the app refused where you clicked and
    /// allowed where you typed. For the picture the two doors at least agreed.
    ///
    /// Text asserts on the **session** rather than on the tool, because that is what
    /// `begin_edit_text` opens — there is no `Tool::Text` to check, and asserting a
    /// tool here would pass against the bug.
    ///
    /// ⚠️ **Flip-checked per arm**, and the arms fail independently: deleting the
    /// guard in `begin_edit_text` leaves a live session on the locked layer, and
    /// deleting the one in `enter_action`'s `Path` arm arms `Tool::Node`. Neither
    /// flip disturbs the other, which is what says these are two guards rather than
    /// one asserted twice — they are in two different files.
    #[test]
    fn a_locked_layer_refuses_text_and_point_editing_too() {
        let ctx = egui::Context::default();
        let text = || NodeKind::Text {
            content: "hello".into(),
            style: Box::new(ondin_core::TextStyle {
                font_family: "Inter".into(),
                font_size: 16.0,
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
        };

        // The controls: unlocked, both modes open. Without these the assertions
        // below pass against an `Enter` that does nothing for anybody.
        let (mut live_text, id) = app_with_a_child_in_a_group(&ctx, text(), false);
        live_text.enter_action();
        assert!(
            live_text.text.is_some(),
            "the control: `Enter` on an unlocked text layer opens a session"
        );
        let (mut live_path, _) = app_with_a_child_in_a_group(&ctx, path_kind(), false);
        live_path.enter_action();
        assert_eq!(
            live_path.tool,
            Tool::Node,
            "the control: `Enter` on an unlocked path arms the node tool"
        );
        let _ = id;

        // Locked — and locked on the **group**, so this is the ancestor rule as
        // well as the refusal.
        let (mut shut_text, _) = app_with_a_child_in_a_group(&ctx, text(), true);
        shut_text.enter_action();
        assert!(
            shut_text.text.is_none(),
            "a text layer inside a locked group cannot be retyped — the menu row \
             already said so and `Enter` did not"
        );
        let (mut shut_path, _) = app_with_a_child_in_a_group(&ctx, path_kind(), true);
        shut_path.enter_action();
        assert_eq!(
            shut_path.tool,
            Tool::Select,
            "nor can a path inside a locked group have its anchors dragged: nothing \
             in `tools/` re-checks the lock, so arming the tool is the thing to refuse"
        );
    }

    /// **`Enter` was not the only door onto point editing, and this is the one that
    /// was missed** (§15 D321).
    ///
    /// `A` and the tool rail's node button both reach `choose_tool(Tool::Node)` with
    /// no lock check, and `edited_path` derives its subject from the *layer
    /// selection* — which a locked layer joins from the layers panel, the very route
    /// that made the text half reachable. So arming the tool directly bypassed the
    /// `Enter` guard entirely. Found by `arch-scribe` reading the claim "`Enter` is
    /// the only door left" against `input.rs` and the rail; it was not.
    ///
    /// The assertion is on **`edited_path`**, not on the tool: arming is allowed and
    /// harmless — pressing `A` over a rectangle already arms onto nothing — and what
    /// must be absent is the *subject*, because all twenty-five readers of "which
    /// path is being edited" go through this one query.
    ///
    /// ⚠️ Flipped by removing the guard from `edited_path`: the locked path comes
    /// back as `Some` with the tool armed, which is the state where its anchors drag.
    #[test]
    fn arming_the_node_tool_finds_no_subject_on_a_locked_path() {
        let ctx = egui::Context::default();

        // The control: unlocked, arming the tool finds the path.
        let (mut live, id) = app_with_a_child_in_a_group(&ctx, path_kind(), false);
        live.choose_tool(Tool::Node);
        assert_eq!(
            live.edited_path(),
            Some(id),
            "the control: an unlocked path is the subject once the tool is armed"
        );

        // Locked group, child's own flag clear — the tool may arm, but there is
        // nothing for it to act on.
        let (mut shut, child) = app_with_a_child_in_a_group(&ctx, path_kind(), true);
        shut.choose_tool(Tool::Node);
        assert!(
            !shut.session.doc.get(child).expect("the child").locked(),
            "the fixture locks the group, not the path"
        );
        assert_eq!(
            shut.edited_path(),
            None,
            "a locked path is not a subject, however the tool was armed — `A` and the \
             rail button do not go through `Enter`'s guard"
        );
    }

    /// A two-segment open path, as `NodeKind`.
    fn path_kind() -> NodeKind {
        let mut p = ondin_core::kurbo::BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((40.0, 0.0));
        p.line_to((40.0, 30.0));
        NodeKind::Path {
            path: p,
            corner_radii: Vec::new(),
        }
    }

    /// **The picture obeys the group rule as well**, which is the half §15 D320
    /// explicitly declined to decide and D321 decided.
    #[test]
    fn a_locked_group_refuses_the_picture_inside_it() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0xD2);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let group = ids.mint();
        let child = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: child,
                parent: group,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(80.0, 60.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            },
        ]))
        .expect("build");
        doc.apply(&Transaction(vec![Operation::SetFills {
            id: child,
            fills: vec![Fill {
                brush: image_brush(ondin_core::ImageId("a-photo".into())),
                visible: true,
            }],
        }]))
        .expect("paint");
        doc.apply(&Transaction(vec![Operation::SetLocked {
            id: group,
            locked: true,
        }]))
        .expect("lock the group");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(child);
        app.tool = Tool::Select;

        // The child's own flag is clear — so a guard reading `n.locked()` passes
        // this picture straight through, which is what it did for one day.
        assert!(
            !app.session.doc.get(child).expect("the child").locked(),
            "the fixture locks the group and never the child, or this asserts nothing \
             about ancestors"
        );
        app.begin_image_edit(child);
        assert_eq!(
            app.tool,
            Tool::Select,
            "a locked group locks the picture inside it"
        );
    }
}

#[cfg(test)]
mod copy_as_png_tests {
    use super::OndinApp;
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::peniko::Color;
    use ondin_core::{Brush, Document, Fill, IdSource, NodeKind, Operation, Resolved, Transaction};

    /// A 100×60 rect at world (30, 20), on its own under the root.
    fn one_rect() -> (Document, Resolved, ondin_core::NodeId) {
        let mut ids = IdSource::new(0x9E);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let rect = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: rect,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(100.0, 60.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate((30.0, 20.0))),
            name: None,
        }]))
        .expect("build");
        doc.apply(&Transaction(vec![Operation::SetFills {
            id: rect,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(200, 40, 60, 255)),
                visible: true,
            }],
        }]))
        .expect("paint");
        let res = Resolved::rebuild(&doc);
        (doc, res, rect)
    }

    /// What *Copy as PNG* hands the clipboard is **raw pixels at one device pixel
    /// per world unit**, which is the pair of choices `png_for_the_clipboard`
    /// exists to make.
    ///
    /// ⚠️ **Flipped against `png_of`**, which is the plausible wrong version and the
    /// one the function's own comment names — the shape that compiles is `png_of`
    /// for the bytes and `extent` for the size, and it hands
    /// `arboard::ImageData` an encoded file where it wants RGBA, so the paste is
    /// noise rather than an error. **It passes the size assertion**, `extent` being
    /// where the size honestly comes from, and fails on the byte count alone: 216
    /// encoded bytes against 24,000. Flipped again against
    /// `RasterOpts { scale: 2.0, .. }`, which fails on the size instead — 200 × 120.
    /// Two assertions because they catch different mistakes; neither alone would
    /// have caught both.
    ///
    /// The fixture is asserted before the claim: a raster of the wrong subject
    /// would satisfy the shape assertions on its own.
    #[test]
    fn the_clipboard_picture_is_raw_pixels_at_one_to_one() {
        let (doc, res, rect) = one_rect();
        let bounds = ondin_export::extent(&res, &[rect]).expect("the rect has bounds");
        assert_eq!(
            (bounds.width(), bounds.height()),
            (100.0, 60.0),
            "the fixture is not the rect this test thinks it is"
        );

        let raster = OndinApp::png_for_the_clipboard(&doc, &res, &[rect])
            .expect("a rect with bounds rasterizes");

        assert_eq!(
            (raster.width, raster.height),
            (100, 60),
            "1:1 means one device pixel per world unit and no multiplier"
        );
        assert_eq!(
            raster.rgba.len(),
            100 * 60 * 4,
            "the clipboard takes raw RGBA — an encoded PNG would be some other length"
        );
    }

    /// An empty selection has no extent, and the row says so instead of putting a
    /// 1×1 or a panic on the clipboard.
    #[test]
    fn nothing_selected_is_no_picture_rather_than_an_empty_one() {
        let (doc, res, _) = one_rect();
        assert!(OndinApp::png_for_the_clipboard(&doc, &res, &[]).is_none());
    }
}

/// `Ctrl`+`←`/`→` widen and narrow the selection.
///
/// ⚠️ **The keymap test beside this one proves nothing about a shape.** It proves
/// the chord resolves to an `Action::SizeStep`; whether that then holds the left
/// edge, floors at a point, or does anything at all is a separate question, and it
/// is the one a user would notice.
#[cfg(test)]
mod size_step_tests {
    use super::OndinApp;
    use ondin_core::kurbo::{Affine, Rect, Size, Vec2};
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    /// A headless app holding one 80×60 rect at (10, 20), selected.
    fn app_with_a_rect(ctx: &egui::Context, transform: Option<Affine>) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x51E);
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
            transform: transform.or(Some(Affine::translate((10.0, 20.0)))),
            name: None,
        }]))
        .expect("build");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(rect);
        (app, rect)
    }

    /// Two 20×20 rects, at (0, 0) and (80, 40), both selected — a union box of
    /// 100×60 with its own top-left at the origin.
    ///
    /// ⚠️ **Two, because one takes an entirely different code path.** A single
    /// selection resizes its *local* box through `resize_box_to`; anything more goes
    /// through `resize_selection` over the world union, which is where
    /// `Handle::BottomRight` and the union arithmetic live. A suite of one-layer
    /// tests leaves that whole arm unexercised — which is not hypothetical: flipping
    /// that handle to `TopLeft` passed every test here until this fixture existed.
    fn app_with_two_rects(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x52E);
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

    /// A width step, as `Ctrl`+`→`/`←` resolves one.
    fn wide(d: f64) -> Vec2 {
        Vec2::new(d, 0.0)
    }

    /// A height step, as `Ctrl`+`↓`/`↑` resolves one.
    fn tall(d: f64) -> Vec2 {
        Vec2::new(0.0, d)
    }

    fn world(app: &OndinApp, id: NodeId) -> Rect {
        app.session
            .resolved
            .world_bounds(id)
            .expect("the rect has bounds")
    }

    /// **Bigger, smaller, on both axes, and the top-left corner stays put.**
    ///
    /// The anchor is the claim worth pinning: it is the corner X reports and the one
    /// the reader holds still, and it is also what makes a *held* key usable — a
    /// shape that grew from its centre or its right edge would wander while the key
    /// repeated.
    ///
    /// ⚠️ **Flip-checked by asking for the size without the delta — `local.width()`
    /// in place of `want(local.width(), delta.x)` — which is the shape a refactor
    /// leaves behind, and every assertion here fails on it.** The flip that looks
    /// right and is *not* is `Handle::TopLeft` in place of `BottomRight`: that
    /// constant is only on the several-selected path, so it changes nothing here.
    /// `several_selected_resize_as_one_box_from_the_unions_corner` is where it bites,
    /// and that test exists because this one did not catch it.
    #[test]
    fn ctrl_arrows_size_the_box_from_its_top_left_corner() {
        let ctx = egui::Context::default();
        let (mut app, rect) = app_with_a_rect(&ctx, None);
        let before = world(&app, rect);
        assert_eq!(
            (before.width(), before.min_x()),
            (80.0, 10.0),
            "the fixture"
        );

        app.size_step(wide(1.0));
        let wider = world(&app, rect);
        assert_eq!(wider.width(), 81.0, "Ctrl+Right adds a point of width");
        assert_eq!(wider.min_x(), before.min_x(), "and holds the left edge");
        assert_eq!(wider.min_y(), before.min_y(), "and does not touch y");
        assert_eq!(wider.height(), before.height(), "or the height");

        app.size_step(wide(-10.0));
        let narrower = world(&app, rect);
        assert_eq!(narrower.width(), 71.0, "Ctrl+Shift+Left takes ten off");
        assert_eq!(narrower.min_x(), before.min_x(), "from the same edge");

        // The other axis, and the same corner. ⚠️ **`↓` is taller and `↑` is
        // shorter**, which is the pairing the top-left anchor forces and the opposite
        // of what "up means more" would give — worth an assertion because it is the
        // one direction in the set a reader might expect the other way round.
        let mid = world(&app, rect);
        app.size_step(tall(6.0));
        let taller = world(&app, rect);
        assert_eq!(taller.height(), mid.height() + 6.0, "Ctrl+Down is taller");
        assert_eq!(taller.min_y(), before.min_y(), "and holds the top edge");
        assert_eq!(taller.width(), mid.width(), "leaving the width alone");

        app.size_step(tall(-2.0));
        let shorter = world(&app, rect);
        assert_eq!(
            shorter.height(),
            taller.height() - 2.0,
            "Ctrl+Up is shorter"
        );
        assert_eq!(shorter.min_y(), before.min_y(), "from the same edge");
    }

    /// **A width cannot be pressed away.** Every factor past zero is a factor *of*
    /// zero, so a layer allowed to reach it could never be pressed back — the panel's
    /// W field floors at 1 for the same reason and this is the same floor, reached
    /// from the keyboard.
    ///
    /// ⚠️ **Flip-checked by dropping the `.max(1.0)`, and the failure is not the one
    /// it looks like it should be — which is why `min_x` is asserted at all.** Traced
    /// press by press: the rect narrows cleanly to 10 wide, the next press asks for 0
    /// and something further down floors *that* at 1, and then the eighth press asks
    /// for **−9** and the rect reflects across its own left edge — it comes out 9
    /// wide sitting at x 1..10, to the *left* of where it had been anchored, and
    /// walks left from there. The tenth press leaves it 1 wide at x 0.
    ///
    /// So the width assertion passes on the broken version by coincidence, landing
    /// back on 1 from the other side. A test that checked only the width would have
    /// been green for a shape that had turned inside out and moved.
    #[test]
    fn narrowing_stops_at_a_point_rather_than_passing_through_zero() {
        let ctx = egui::Context::default();
        let (mut app, rect) = app_with_a_rect(&ctx, None);
        for _ in 0..10 {
            app.size_step(wide(-10.0));
        }
        let floored = world(&app, rect);
        assert_eq!(floored.width(), 1.0, "ten coarse presses stop at a point");
        assert_eq!(floored.min_x(), 10.0, "still anchored where it started");
        // And it can be pressed back out, which is the whole reason for the floor.
        app.size_step(wide(10.0));
        assert_eq!(world(&app, rect).width(), 11.0, "and it comes back");

        // ⚠️ **The height, because the floor is per axis and could be applied to one
        // of them.** Both go through one `want` closure precisely so they cannot
        // diverge, and this is what would notice if that were ever unrolled.
        for _ in 0..10 {
            app.size_step(tall(-10.0));
        }
        let flat = world(&app, rect);
        assert_eq!(flat.height(), 1.0, "the same floor on the other axis");
        assert_eq!(flat.min_y(), 20.0, "anchored at the top edge it started on");
    }

    /// ⚠️ **A rotated shape grows along its own axes, not the screen's.**
    ///
    /// This is inherited rather than implemented: one selected layer goes through
    /// `tools::resize_box_to`, which works on the *local* box. Turned 90°, a point
    /// of width therefore shows up as a point of world **height** — and the world
    /// width does not move at all. Asserting on the world box is the point: it is
    /// the one place the two readings visibly disagree, so a version that resized
    /// the world union instead would fail here and pass everywhere else.
    #[test]
    fn a_rotated_layer_grows_along_its_own_axes() {
        let ctx = egui::Context::default();
        let quarter = Affine::translate((10.0, 20.0)) * Affine::rotate(std::f64::consts::FRAC_PI_2);
        let (mut app, rect) = app_with_a_rect(&ctx, Some(quarter));
        let before = world(&app, rect);
        assert!(
            (before.width() - 60.0).abs() < 1e-6 && (before.height() - 80.0).abs() < 1e-6,
            "the fixture: turned a quarter, the 80×60 rect reads 60×80 on screen, \
             not {before:?}"
        );

        app.size_step(wide(4.0));
        let after = world(&app, rect);
        assert!(
            (after.height() - 84.0).abs() < 1e-6,
            "four points of the layer's own width land on the world height: {after:?}"
        );
        assert!(
            (after.width() - before.width()).abs() < 1e-6,
            "and the world width is untouched: {after:?}"
        );

        // The other axis crosses the same way, which is what says the whole box is
        // read in local space rather than one axis of it being special-cased.
        app.size_step(tall(5.0));
        let both = world(&app, rect);
        assert!(
            (both.width() - (after.width() + 5.0)).abs() < 1e-6,
            "and five points of its own height land on the world width: {both:?}"
        );
        assert!(
            (both.height() - after.height()).abs() < 1e-6,
            "leaving the world height where the last press put it: {both:?}"
        );
    }

    /// **A shape with no width at all.** A vertical `Line` is the degenerate case the
    /// floor cannot help with: its local box is zero points wide, so "one point wider"
    /// is a factor of infinity and "hold the left edge" names an edge that is also the
    /// right one.
    ///
    /// ⚠️ **Written after measuring, because the doc comment on `size_step` first
    /// claimed the answer without checking it.** What actually happens is recorded
    /// below rather than reasoned about — the point of the test is that whatever it
    /// is, it is finite, and the line is still a line afterwards.
    #[test]
    fn an_axis_with_no_extent_survives_being_asked_to_grow() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0x11E);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let line = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: line,
            parent: root,
            index: 0,
            // Straight down: zero wide, forty tall.
            kind: NodeKind::Line {
                end: ondin_core::kurbo::Point::new(0.0, 40.0),
            },
            transform: Some(Affine::translate((10.0, 20.0))),
            name: None,
        }]))
        .expect("build");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(line);

        let before = world(&app, line);
        assert_eq!(
            before.width(),
            0.0,
            "the fixture: a vertical line is 0 wide"
        );
        app.size_step(wide(1.0));
        let after = world(&app, line);
        assert_eq!(
            after, before,
            "measured: the line is left exactly alone — not scaled to infinity, and              not floored up to a point of width it never had"
        );
    }

    /// **A locked layer scales both axes, whichever arrow drives.**
    ///
    /// The fixture is 80×60, so the aspect is 0.75 and every paired number below is
    /// one the reader can check by hand — which is the point of picking it over a
    /// square, where a bug that swapped the two axes would be invisible.
    ///
    /// Four claims: the width arrow carries the height, the height arrow carries the
    /// width, the ratio survives both, and the top-left corner is still held. The
    /// last is worth keeping because pairing is the one change here that could have
    /// moved the anchor without anyone noticing — a shape that grew on both axes from
    /// the wrong corner looks much more plausible than one that grew on one.
    ///
    /// Flip-checked by dropping the `locked` test, which is simply the unlocked
    /// behaviour: the *second* assertion catches it, the height staying at 60 while
    /// the width goes to 84 either way.
    ///
    /// ⚠️ **The comment in `size_step` claims `keep_ratio` would refuse to shrink, and
    /// that was run rather than reasoned.** Passing `Resize::geometry(true, false)` on
    /// the several-selected arm and asking for 40 points *off* the union's width left
    /// it at exactly 100×60 — no change at all — because the flag takes the larger of
    /// the two factors and the untouched axis contributes a 1.0. Recorded here because
    /// it is the argument for pairing by hand, and the shape of it is not obvious from
    /// the flag's name.
    #[test]
    fn a_locked_layer_keeps_its_proportions_under_either_arrow() {
        let ctx = egui::Context::default();
        let (mut app, rect) = app_with_a_rect(&ctx, None);
        app.session
            .commit(Transaction(vec![Operation::SetProportionsLocked {
                id: rect,
                locked: true,
            }]));
        let before = world(&app, rect);
        assert_eq!(
            (before.width(), before.height()),
            (80.0, 60.0),
            "the fixture"
        );

        // Width drives: 80 → 84, and the height follows at 0.75 of it.
        app.size_step(wide(4.0));
        let w_driven = world(&app, rect);
        assert_eq!(
            w_driven.width(),
            84.0,
            "the arrow's own axis takes the step"
        );
        assert_eq!(w_driven.height(), 63.0, "and the other follows the aspect");
        assert_eq!(w_driven.min_x(), 10.0, "still held at the top-left");
        assert_eq!(w_driven.min_y(), 20.0);

        // Height drives: 63 → 66, and the width follows back through the same ratio.
        app.size_step(tall(3.0));
        let h_driven = world(&app, rect);
        assert_eq!(
            h_driven.height(),
            66.0,
            "the other arrow drives the other way"
        );
        assert_eq!(h_driven.width(), 88.0, "carrying the width with it");
        assert_eq!(h_driven.min_x(), 10.0, "from the same corner");
        assert_eq!(h_driven.min_y(), 20.0);

        // ...and the ratio is the one it started with, after two presses on two axes.
        assert!(
            (h_driven.height() / h_driven.width() - 0.75).abs() < 1e-9,
            "the aspect survived the pair of presses: {h_driven:?}"
        );
    }

    /// **Unlocked, one arrow moves one axis** — the control the test above needs, and
    /// the thing the lock is a departure from rather than an addition to.
    #[test]
    fn an_unlocked_layer_moves_only_the_axis_the_arrow_names() {
        let ctx = egui::Context::default();
        let (mut app, rect) = app_with_a_rect(&ctx, None);
        assert!(
            !app.session
                .doc
                .get(rect)
                .expect("the rect")
                .proportions_locked(),
            "the fixture: a new layer is unlocked"
        );
        app.size_step(wide(4.0));
        let after = world(&app, rect);
        assert_eq!(after.width(), 84.0);
        assert_eq!(
            after.height(),
            60.0,
            "the height must not follow when nothing is locking it"
        );
    }

    /// Lock the proportions of every id given, as one transaction.
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

    /// **A set is locked when every member is, and then the keyboard pairs the union
    /// too** — §15 D50's rule, which the handles have applied since it was decided and
    /// neither numeric door consulted until now.
    ///
    /// The union is 100 × 60, so the aspect is 0.6 and the paired number is checkable:
    /// half again as wide is 150, and the height follows to 90.
    ///
    /// ⚠️ **The mixed case is the half that says it is D50's rule and not "any locked
    /// layer locks the box".** With one of the two locked the set is *not* locked, and
    /// the height must stay where it was — a set being only as constrained as its least
    /// constrained member. Flip-checked with `any` in place of `all` inside
    /// `tools::proportions_locked`: the second half of this test fails and the first
    /// half passes, which is exactly how that bug would reach the machine.
    #[test]
    fn a_set_pairs_only_when_every_member_is_locked() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = app_with_two_rects(&ctx);
        let span = |app: &OndinApp| world(app, a).union(world(app, b));
        assert_eq!((span(&app).width(), span(&app).height()), (100.0, 60.0));

        lock(&mut app, &[a, b]);
        app.size_step(wide(50.0));
        let both = span(&app);
        assert_eq!(both.width(), 150.0, "the arrow's own axis takes the step");
        assert_eq!(both.height(), 90.0, "and the union's height follows at 0.6");
        assert_eq!((both.min_x(), both.min_y()), (0.0, 0.0), "same corner held");

        // Now a mixed set: `a` locked, `b` not.
        let (mut app, a, b) = app_with_two_rects(&ctx);
        lock(&mut app, &[a]);
        app.size_step(wide(50.0));
        let mixed = span(&app);
        assert_eq!(mixed.width(), 150.0, "the driven axis still steps");
        assert_eq!(
            mixed.height(),
            60.0,
            "but a set with one unlocked member is not a locked set — the height stays"
        );
        let _ = b;
    }

    /// **Several selected resize as one box, anchored at the union's top-left.**
    ///
    /// This is the other arm entirely — `resize_selection` over the world union
    /// rather than `resize_box_to` over one local box — and the claim worth pinning
    /// is that the set keeps its *arrangement*: the layer at the union's own corner
    /// does not move, the far one travels with the edge that grew, and the gap
    /// between them scales with the box rather than staying put.
    ///
    /// ⚠️ **Flip-checked by resizing from `Handle::TopLeft`, which holds the far
    /// corner instead — and that flip is the reason this test exists.** It passed
    /// every single-layer test in the module, because the handle constant is not on
    /// the path a single selection takes at all. A whole arm of the split was
    /// uncovered and the suite looked complete.
    #[test]
    fn several_selected_resize_as_one_box_from_the_unions_corner() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = app_with_two_rects(&ctx);
        let union = world(&app, a).union(world(&app, b));
        assert_eq!(
            (union.min_x(), union.min_y(), union.width(), union.height()),
            (0.0, 0.0, 100.0, 60.0),
            "the fixture"
        );

        // Half as wide again: 100 → 150.
        app.size_step(wide(50.0));
        let (near, far) = (world(&app, a), world(&app, b));
        assert_eq!(
            near.min_x(),
            0.0,
            "the layer on the union's corner stays put"
        );
        assert_eq!(
            near.width(),
            30.0,
            "and is scaled by the box, not translated"
        );
        assert_eq!(
            far.max_x(),
            150.0,
            "the far layer's outer edge is where the box's new one is"
        );
        assert_eq!(near.height(), 20.0, "the untouched axis is untouched");
        assert_eq!(far.min_y(), 40.0, "on both layers");
    }

    /// **Nothing selected, nothing happens** — and, more usefully, no panic and no
    /// entry on the undo stack. `size_step` is reached from a bare keypress, so the
    /// empty case is not an edge case but the state the app spends most of its time
    /// in.
    #[test]
    fn an_empty_selection_is_inert() {
        let ctx = egui::Context::default();
        let (mut app, rect) = app_with_a_rect(&ctx, None);
        app.session.selection.clear();
        let depth = app.session.history.undo_depth();
        let before = world(&app, rect);
        app.size_step(wide(5.0));
        assert_eq!(
            world(&app, rect),
            before,
            "an empty selection leaves the shapes alone"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            depth,
            "and puts nothing on the undo stack"
        );
    }
}

#[cfg(test)]
mod layers_width_tests {
    //! The two halves of a draggable layers panel that are not its bounds
    //! (§15 D349): when a dragged width is worth storing, and the piece of egui's
    //! own behaviour the panel is built on.
    //!
    //! **`Prefs::save` is deliberately out of reach here.** It writes into
    //! `dirs::config_dir()`, so a test that could reach it would rewrite the real
    //! `prefs.json` of whoever ran the suite — which is why the decision was lifted
    //! into `width_to_store` and only the four-line wiring around it is still
    //! verified by reading.

    use super::*;

    /// **The three gates, one case each, and the room gate is the one worth
    /// having.**
    ///
    /// A dragged width is stored only when the pointer is up (one write per
    /// gesture, not one per frame), when the panel is not merely filling the room
    /// it was given (a narrow *window* must not permanently narrow the panel), and
    /// when it actually differs from what is stored (no rewrite on a quiet frame).
    ///
    /// ⚠️ **Flipped by deleting the `width < room - 0.5` term**, which is the one a
    /// reader is most likely to think is redundant — it looks like it can only be
    /// true. It fails on the `window shrank` case: a 400pt window reports a 400pt
    /// panel with no drag anywhere near it, and without the term that gets written
    /// to disk and outlives the window. Predicted there and observed there.
    ///
    /// ⚠️ Deleting the `!down` term instead fails on `mid-drag`, and deleting the
    /// deadband fails on `a hair of rounding` — three terms, three cases, no case
    /// answered by more than one of them.
    #[test]
    fn a_dragged_width_is_stored_once_the_drag_ends_and_the_window_is_not_the_cause() {
        // (width, room, stored, pointer down) -> what should be stored
        for (what, args, want) in [
            ("mid-drag", (360.0, 1200.0, 300.0, true), None),
            ("released", (360.0, 1200.0, 300.0, false), Some(360.0)),
            ("unchanged", (300.0, 1200.0, 300.0, false), None),
            ("a hair of rounding", (300.4, 1200.0, 300.0, false), None),
            // The panel is as wide as everything it was given, so egui's clamp is
            // what set it, not a hand.
            ("window shrank", (400.0, 400.0, 300.0, false), None),
            // Same width, but with room to spare — so this one *is* a drag.
            ("dragged wide", (400.0, 1200.0, 300.0, false), Some(400.0)),
        ] {
            let (width, room, stored, down) = args;
            assert_eq!(
                width_to_store(width, room, stored, down),
                want,
                "{what}: width {width}, room {room}, stored {stored}, pointer down \
                 {down}"
            );
        }
    }

    /// **A `Panel` takes its width from what its *content* occupied, so a resizable
    /// one whose content does not fill snaps back on the next frame.**
    ///
    /// egui says this in `Panel::resizable`'s own doc — "you also need to make the
    /// ui use the available space" — and what that means in practice is not
    /// obvious from the sentence: the panel does not merely fail to grow, it
    /// collapses to `size_range`'s **minimum**, because that is the only floor
    /// `Frame::show` is given. `layers_panel` calls `ui.set_min_width` for this
    /// reason and this is the assertion that says why.
    ///
    /// Pinned as a fact about **egui** rather than about the layers panel, with a
    /// bare label for content, because that is the seam: nothing in this project
    /// decides it, and the day it changes the panel's own tests would all still
    /// pass while the drag silently stopped holding.
    #[test]
    fn a_resizable_panel_collapses_to_its_minimum_unless_the_content_fills_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let width = |fills: bool| {
            let mut w = 0.0;
            // Two passes: the first stores `PanelState`, the second reads it back,
            // which is the frame a snap-back would show up on.
            for _ in 0..2 {
                let _ = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1200.0, 800.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        w = egui::Panel::left("probe")
                            .default_size(420.0)
                            .size_range(crate::panels::layers::MIN_W..=crate::panels::layers::MAX_W)
                            .resizable(true)
                            .show(ui, |ui| {
                                if fills {
                                    ui.set_min_width(ui.available_width());
                                }
                                ui.label("a name");
                            })
                            .response
                            .rect
                            .width();
                    },
                );
            }
            w
        };

        assert_eq!(
            width(true),
            420.0,
            "the panel did not keep the width it was asked for even with the \
             content filling it — `default_size` is not doing what this depends on"
        );
        assert_eq!(
            width(false),
            crate::panels::layers::MIN_W,
            "a panel whose content does not fill it was expected to collapse to \
             `MIN_W`; if this ever stops being true, `layers_panel`'s \
             `set_min_width` has become decoration and the comment on it is wrong"
        );
    }

    /// **A stored width outside the bounds is clamped, and only because
    /// `default_size` is called *before* `size_range`.**
    ///
    /// `prefs.json` is a plain file on disk: it can be hand-edited, copied between
    /// machines, or written by a future version with different bounds. So the
    /// value reaching `default_size` is untrusted input, and what makes it safe is
    /// builder *order* — `default_size` widens the range to admit whatever it is
    /// handed, and `size_range` then replaces the range and clamps the default
    /// into it.
    ///
    /// ⚠️ Flipped by swapping the two calls: 5000 comes straight through and the
    /// panel opens over the whole window. Nothing about the source would look
    /// wrong — both spellings read as "a default and a range" — which is the only
    /// reason this is a test rather than a comment.
    #[test]
    fn a_stored_width_from_outside_the_bounds_is_clamped() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let opened_at = |stored: f32, salt: &'static str| {
            let mut w = 0.0;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(6000.0, 800.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    w = egui::Panel::left(salt)
                        .default_size(stored)
                        .size_range(crate::panels::layers::MIN_W..=crate::panels::layers::MAX_W)
                        .resizable(true)
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.label("a name");
                        })
                        .response
                        .rect
                        .width();
                },
            );
            w
        };

        // A 6000pt window, so the window's own clamp is not what answers this —
        // it has to be `size_range`.
        assert_eq!(
            opened_at(5000.0, "clamp-hi"),
            crate::panels::layers::MAX_W,
            "a nonsense width out of `prefs.json` opened the panel over the window"
        );
        assert_eq!(
            opened_at(1.0, "clamp-lo"),
            crate::panels::layers::MIN_W,
            "a width under the minimum was not raised to it"
        );
    }

    /// **The two facts about egui that decide who owns the cursor** (§15 D349) —
    /// the bug the resizable panel found rather than introduced.
    ///
    /// The panel's drag edge claims `ResizeHorizontal` while `layers_panel` draws;
    /// the canvas draws *after* it and used to answer "not me" by writing
    /// `Default`, erasing the claim in the frame it was made. Two egui facts
    /// decide it, neither visible in the source of either function:
    ///
    /// 1. A cursor **does not survive a frame** — `Context::end_pass` takes the
    ///    viewport's whole `PlatformOutput` with a plain `std::mem::take`, so both
    ///    channels start every frame at their `Default`. **No reset is needed
    ///    anywhere**, which is what makes deleting `canvas_cursor`'s the whole fix
    ///    rather than half of it.
    /// 2. Within a frame the **last** writer wins — which is why a reset that
    ///    *was* written cost the panel its cursor.
    ///
    /// ⚠️ **Fact 1 is the opposite of what `canvas_cursor`'s comment asserted**,
    /// and the comment was not vague about it: "egui never resets either one —
    /// `PlatformOutput` carries both across frames on purpose". There *is* such a
    /// path — `PlatformOutput::take`, whose body says "sticky between frames" in
    /// so many words — and nothing on this route calls it. A true sentence about
    /// the wrong function, which is why this is a test and not a correction:
    /// reading egui twice would give the same wrong answer twice.
    ///
    /// ⚠️ **This test failed on its first run and it was the test that was right.**
    /// It was written asserting stickiness, from that comment; the measurement
    /// said `Default` and the source then said why. Recorded because the reflex
    /// when a probe contradicts a comment is to suspect the probe.
    ///
    /// (See also `the_panel_puts_the_style_back_after_retuning_the_drag_edge`,
    /// which is the other half of "who is still to be drawn into this `ui`".)
    #[test]
    fn a_cursor_does_not_survive_a_frame_and_the_last_writer_wins() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);

        let icon_of = |out: egui::FullOutput| out.platform_output.cursor_icon;

        // (1) Not sticky: claimed in one frame, gone in the next, which wrote
        // nothing at all.
        let claimed = ctx.run_ui(Default::default(), |ui| {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        });
        assert_eq!(
            icon_of(claimed),
            egui::CursorIcon::ResizeHorizontal,
            "a claim did not even reach the frame it was made in"
        );
        assert_eq!(
            icon_of(ctx.run_ui(Default::default(), |_| {})),
            egui::CursorIcon::Default,
            "a cursor survived a frame that says nothing — if egui has gone back \
             to carrying it forward, every claimant needs a withdrawal again and \
             `canvas_cursor`'s deleted reset has to come back somewhere earlier"
        );

        // (2) Last writer wins, which is the whole of why the deleted line cost
        // the panel its cursor rather than merely being redundant.
        assert_eq!(
            icon_of(ctx.run_ui(Default::default(), |ui| {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
            })),
            egui::CursorIcon::Default,
            "claim-then-reset kept the claim, so the shipped bug could not have \
             happened the way it is written up"
        );
    }

    /// **Drawing the layers panel leaves the style exactly as it found it.**
    ///
    /// `layers_panel` retunes two *fg* strokes on the `ui` it is handed, because
    /// egui reads the drag edge's lit colours off the parent's style and its
    /// defaults there are effectively white. That `ui` is the frame's central one,
    /// and the tool rail, the canvas and the inspector are all drawn into it
    /// afterwards — where `hovered.fg_stroke` is what a hovered button's *label*
    /// uses. So the restore is not tidiness; without it every hovered label in the
    /// app below this call goes accent-blue.
    ///
    /// Asserted against the real method rather than against a `set_style`
    /// round-trip, because the failure worth catching is not "restore does not
    /// work" — it is **someone adding an early return** between the retune and the
    /// restore. Only a test that calls `layers_panel` itself can see that.
    ///
    /// ⚠️ Flipped by deleting the `ui.set_style(style)` line: fails on
    /// `hovered.fg_stroke`, reporting the accent where `TEXT` is due.
    #[test]
    fn the_panel_puts_the_style_back_after_retuning_the_drag_edge() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut app = OndinApp::headless(&ctx);

        let (mut before, mut after) = (None, None);
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 800.0),
                )),
                ..Default::default()
            },
            |ui| {
                before = Some(ui.style().visuals.widgets.clone());
                app.layers_panel(ui);
                after = Some(ui.style().visuals.widgets.clone());
            },
        );
        let (before, after) = (before.expect("a frame ran"), after.expect("a frame ran"));
        assert_eq!(
            after.hovered.fg_stroke, before.hovered.fg_stroke,
            "the hovered text stroke was left retuned, so every hovered label \
             drawn after the layers panel is the drag edge's colour"
        );
        assert_eq!(
            after.active.fg_stroke, before.active.fg_stroke,
            "the active text stroke was left retuned"
        );
        // The states the retune does *not* touch, so a restore that overreached —
        // putting back a whole stale `Style` snapshot taken too early, say — is
        // caught here rather than looking like success.
        assert_eq!(after.hovered.bg_stroke, before.hovered.bg_stroke);
        assert_eq!(
            after.noninteractive.bg_stroke,
            before.noninteractive.bg_stroke,
        );
    }
}

#[cfg(test)]
mod edge_dwell_tests {
    //! The layers panel's drag edge waits before it lights (§15 D350).
    //!
    //! Reported from the machine: the edge went blue on every pass of the pointer
    //! across the panel, and the splitter is on the way from the tree to the
    //! canvas, so that is most passes.
    //!
    //! Driven through the machine's **own states** rather than through frames that
    //! would produce them — `chrome_hold`'s pattern, and here it is the only way to
    //! reach two of the four transitions at all: "a press landed on the edge" and
    //! "the pointer left the band with a button still down" both need a pointer
    //! history that a single `run_ui` cannot express.

    use super::*;

    /// **The highlight waits, and the wait is what was asked for.**
    ///
    /// A pointer that crosses the band and leaves inside the dwell never lights it;
    /// one that stays lights at the deadline and not before.
    ///
    /// ⚠️ Sampled a hair **either side** of the deadline rather than at it. This is
    /// the caret-blink trap from `CLAUDE.md` in its other form: asserting only
    /// "lit at `t + 0.2`" passes against a machine that lights the moment it is
    /// touched, since that is also lit at `t + 0.2`. The pair `0.199 → not lit`,
    /// `0.201 → lit` is what pins the deadline to *this* deadline.
    ///
    /// ⚠️ Flipped by returning `Lit` from the `Off if in_band` arm — the
    /// no-delay version, which is what shipped and was reported. It fails on the
    /// **first** assertion of the crossing case, not on the boundary pair, because
    /// a crossing pointer is lit on the frame it arrives. Predicted at the boundary
    /// pair and observed one case earlier, which is the more useful failure: the
    /// bug was never about *when* the light arrived, it was that it arrived at all.
    ///
    /// ⚠️ **It also takes two of the other three tests with it**, which was not
    /// predicted: with no `Waiting` state ever reached, the crossing-drag case
    /// lights at `t=0` and `due` has nothing to return. Worth knowing before
    /// reading a three-test failure here as three faults — the whole module hangs
    /// off one arm, so the other flips below are the ones that say where the teeth
    /// actually are.
    #[test]
    fn the_edge_waits_out_the_dwell_before_it_lights() {
        // A pointer passing through: in the band for two frames, then gone.
        let mut d = EdgeDwell::default();
        d = d.advance(0.0, true, false, false);
        assert!(!d.lit(), "lit on the frame the pointer arrived");
        d = d.advance(0.05, true, false, false);
        assert!(!d.lit(), "lit 50ms in, well inside the dwell");
        d = d.advance(0.10, false, false, false);
        assert_eq!(
            d,
            EdgeDwell::Off,
            "the pointer left and the wait did not reset"
        );

        // A pointer that stays, sampled either side of the deadline.
        let arrive = EdgeDwell::default().advance(1.0, true, false, false);
        assert!(
            !arrive
                .advance(1.0 + EDGE_DWELL - 0.001, true, false, false)
                .lit(),
            "lit a millisecond early, so the deadline is not the one it claims"
        );
        assert!(
            arrive
                .advance(1.0 + EDGE_DWELL + 0.001, true, false, false)
                .lit(),
            "still dark a millisecond late — the highlight never arrives on its own"
        );
    }

    /// **A press lights the edge at once; a drag that merely arrives never does,
    /// however long it stays.**
    ///
    /// A drag must never be the one gesture with no feedback, so the dwell is
    /// skipped when the button goes down in the band. But the button being *down*
    /// is a different question from the gesture being a resize — a drag begun
    /// elsewhere and carried over the band is down the whole way — so
    /// `EdgeDwell::advance` takes the press **edge** for the shortcut and uses
    /// `down` to **block** the dwell.
    ///
    /// ⚠️ **The loop runs past the deadline, and that is the whole point of it.**
    /// It sampled 0, 50 and 100ms in its first draft, all inside the dwell, so it
    /// proved only that the *shortcut* was withheld — an in-flight drag that
    /// hesitated over the band still lit the edge on the ordinary path at 200ms,
    /// which is the reported complaint in its rarer form and which this test as
    /// first written could not see. `arch-scribe` found it by reading the arm
    /// against the sentence above it. **A test that stops short of the boundary is
    /// a test of the shortcut, not of the rule.**
    ///
    /// ⚠️ Flipped two ways. `down` for `pressed` in the shortcut arm: the crossing
    /// case lights at `t = 0`, the first sample. Dropping `!down` from the dwell
    /// arm: the first three samples stay green and it fails on the **fourth**, the
    /// one past the deadline — which is the flip that would have caught the
    /// original omission and the reason the sample list is what it is.
    #[test]
    fn a_press_skips_the_dwell_but_a_drag_that_only_arrives_never_lights_it() {
        let pressed_on_it = EdgeDwell::default().advance(0.0, true, true, true);
        assert!(
            pressed_on_it.lit(),
            "a press on the edge waited out the dwell, so the first 200ms of a \
             resize drag has no feedback at all"
        );

        // A drag that began elsewhere and comes to rest on the band: `down` the
        // whole way, `pressed` never, because the press was not this frame and not
        // here. The last sample is deliberately past `EDGE_DWELL`.
        let mut lingering = EdgeDwell::default();
        for t in [0.0, 0.05, 0.10, EDGE_DWELL + 0.05] {
            lingering = lingering.advance(t, true, false, true);
            assert!(
                !lingering.lit(),
                "a drag already in flight lit the edge at t={t} — the button being \
                 down is not the same question as the gesture being a resize, and \
                 past {EDGE_DWELL} a hesitating drag is exactly the case the delay \
                 was asked for"
            );
        }
    }

    /// **A lit edge stays lit once the pointer leaves the band, while a button is
    /// down.**
    ///
    /// Not a nicety: the pointer leaves the band on every drag that reaches
    /// `MIN_W` or `MAX_W`, because egui stops the edge at the clamp and the hand
    /// keeps going. Without the `down` arm the highlight drops out exactly when the
    /// user is pushing hardest against a limit — the one moment the edge most needs
    /// to still be there.
    ///
    /// ⚠️ Flipped by dropping `|| down` from the `Lit` arm: fails on the second
    /// assertion, the state falling to `Off` the frame the pointer passes the
    /// clamp.
    #[test]
    fn a_drag_past_the_clamp_keeps_the_edge_lit() {
        let lit = EdgeDwell::default()
            .advance(0.0, true, true, true)
            .advance(0.1, true, false, true);
        assert!(
            lit.lit(),
            "the fixture is not in the state this test assumes"
        );

        let past = lit.advance(0.2, false, false, true);
        assert!(
            past.lit(),
            "the edge went dark as the drag ran past its clamp, which is where the \
             pointer leaves the band on every drag that reaches a limit"
        );
        // And it does let go, on the frame the button comes up.
        assert_eq!(
            past.advance(0.3, false, false, false),
            EdgeDwell::Off,
            "released off the band and the edge stayed lit — it would now be lit \
             with the pointer somewhere else entirely"
        );
    }

    /// **The fold reads the *primary* button, not any button** — driven through
    /// the real `OndinApp::layers_panel`, because nothing above this test does.
    ///
    /// ⚠️ **This test exists because a flip did not bite.** Putting `any_down()`
    /// back at the fold left all four state-machine tests green, which is a finding
    /// about the coverage rather than about the change: every one of them calls
    /// `EdgeDwell::advance` directly, so the machine was pinned and the
    /// *gathering* of its inputs was not. Which button the fold asks about is a
    /// decision, and it was reachable only by reading until now.
    ///
    /// The discriminating case is a **middle**-button pan resting on the band past
    /// the dwell: under `any_down` it blocks the dwell and the edge stays dark;
    /// under `button_down(Primary)` it is not a resize gesture, does not block, and
    /// the edge lights like any resting pointer. One assertion tells the two
    /// spellings apart, which is the whole reason to prefer it over a second
    /// `advance` case.
    #[test]
    fn the_fold_blocks_the_dwell_on_the_primary_button_only() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut app = OndinApp::headless(&ctx);

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut frame = |time: f64, events: Vec<egui::Event>| {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| app.layers_panel(ui),
            );
        };

        // One frame to store the panel's rect — the band is read from `PanelState`,
        // which does not exist until the panel has drawn once.
        frame(0.0, vec![]);
        let edge = egui::PanelState::load(&ctx, egui::Id::new("layers"))
            .expect("the panel stored a rect")
            .outer_rect
            .right();
        assert!(
            (edge - crate::panels::layers::DEFAULT_W).abs() < 0.5,
            "the fixture opened at {edge}, not the {}pt default",
            crate::panels::layers::DEFAULT_W
        );

        // Middle button down, pointer resting on the edge, well past the dwell.
        let on_edge = egui::pos2(edge, 400.0);
        frame(
            1.0,
            vec![
                egui::Event::PointerMoved(on_edge),
                egui::Event::PointerButton {
                    pos: on_edge,
                    button: egui::PointerButton::Middle,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
        );
        frame(1.0 + EDGE_DWELL + 0.05, vec![]);
        assert_eq!(
            app.layers_edge,
            EdgeDwell::Lit,
            "a middle-button pan resting on the edge blocked the dwell, so the fold \
             is asking `any_down` where it means the primary button"
        );
    }

    /// **The frame is woken for the deadline, because a resting pointer produces no
    /// events.**
    ///
    /// This is the whole reason the machine needs a `due` at all: the case it
    /// exists for is a hand that has *stopped*, and a stopped hand generates
    /// nothing for egui to repaint on. Without the wake-up the highlight arrives on
    /// whatever event happens next, which may be the press itself — i.e. never,
    /// visibly.
    ///
    /// ⚠️ Flipped by returning `None` from `due` unconditionally: fails on the
    /// first assertion. Note the *shape* of the assertion — `due` is asked of the
    /// waiting state and of both settled ones, because a `due` that fired in `Lit`
    /// too would request a repaint every frame the pointer rests on a lit edge.
    #[test]
    fn a_waiting_edge_asks_to_be_woken_and_a_settled_one_does_not() {
        let waiting = EdgeDwell::default().advance(4.0, true, false, false);
        assert_eq!(
            waiting.due(),
            Some(4.0 + EDGE_DWELL),
            "a waiting edge asked for no repaint, so the highlight only appears if \
             something else happens to redraw the frame"
        );
        assert_eq!(EdgeDwell::Off.due(), None);
        assert_eq!(
            EdgeDwell::Lit.due(),
            None,
            "a lit edge asked to be woken, which is a repaint every frame for as \
             long as the pointer rests on it"
        );
    }
}

/// The gate for the expression itself (§15 D524).
///
/// **Four sites, one string, three separate accidents.** `drag_stopped() ||
/// lost_focus() || changed()` was removed from `edit_valve` by §15 D316 and then
/// survived in `multi_valve` until D519, in `char_valve` until D523, and in
/// `inspector::valve_slot`'s prediction of the valve until D524 — found, in
/// order, by a
/// review pass, by writing a doc comment about the second one, and by grepping
/// for the literal while writing *that*. Nothing in the tree looked for it, and
/// the only reason all four are closed is that somebody happened to grep.
///
/// So this greps. It is the cheapest possible gate and it is the only one that
/// can see this class at all: every one of the four compiled, tested, linted,
/// formatted and passed `cargo doc`.
#[cfg(test)]
mod valve_condition_gate {
    use std::path::{Path, PathBuf};

    /// The marker a line carries to say it is a **gate** — "is anything
    /// happening to this control?" — rather than a **commit condition**.
    ///
    /// Spelled as a comment beside the code rather than as a path in a list
    /// here, for `deps_forbidden`'s lesson: a list of exempt locations rots
    /// the moment a function moves, and a marker travels with the line it
    /// excuses and has to be re-justified by whoever writes the next one.
    const OK: &str = "valve-gate-not-a-commit";

    fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                rs_files(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    /// The file with `//` comments blanked out, line by line, so the prose that
    /// *explains* this expression — D316's own paragraph, and this module's —
    /// does not trip it.
    ///
    /// ⚠️ Blanked rather than removed, so byte offsets still name real lines.
    /// A `//` inside a string literal would be blanked too; there is none in
    /// this workspace and the failure mode is a missed hit, not a false one.
    fn without_comments(src: &str) -> String {
        src.lines()
            .map(|l| match l.find("//") {
                Some(i) => format!("{}{}", &l[..i], " ".repeat(l.len() - i)),
                None => l.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// **No control may decide it is finished by asking `lost_focus()` and
    /// `changed()` together** (§15 D316, D519, D523, D524).
    ///
    /// A `DragValue` being typed into reports `changed()` on every keystroke and
    /// `lost_focus()` on two consecutive frames, so a condition built from those
    /// two commits per keystroke and then twice more. What replaced it is an
    /// engagement latch — `dragged() || has_focus()`, committing on the frame
    /// that stops being true — and the **three** valves in this workspace all
    /// use it now. The fourth site was not a valve at all but a line predicting
    /// what one would do (§15 D524), which is the distinction this gate is
    /// deliberately blind to: it looks for the expression, not for the shape.
    ///
    /// The two terms may still appear together in a **gate**: a test of whether
    /// anything is happening to a control at all, which decides whether to
    /// *build* a transaction rather than whether to commit one. Those carry the
    /// marker and say why.
    ///
    /// ⚠️ **Windowed rather than line-matched, because `cargo fmt` wraps.** A
    /// condition long enough to be a problem is long enough to be split across
    /// three lines, and a line-by-line grep would have found none of the four.
    ///
    /// ⚠️ **Broken on purpose to check it goes red**, which is the move
    /// `CLAUDE.md` recommends for a gate and which found two of this project's
    /// known gate holes. A fourth valve was added to `inspector.rs` committing
    /// on the superseded expression, `cargo fmt` wrapped it across four lines,
    /// and this named the file and the line. Removed again; the run before it
    /// and the run after it are both green.
    ///
    /// ⚠️ **The first run was not green, and none of the five it found was a
    /// bug** — `interacting`, its inline copy in `canvas.rs`, the Type panel's
    /// opacity gate, `CharWrite::Valve`'s gate, and a `TextEdit` whose two terms
    /// are in two different conditions. Every one is now marked, with a sentence
    /// saying why it is not a commit. **That is the cost of this gate and it is
    /// paid once**; what it buys is that the sixth such line has to be justified
    /// by whoever writes it.
    #[test]
    fn nothing_commits_on_the_expression_d316_removed() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/");
        let mut files = Vec::new();
        rs_files(root, &mut files);
        assert!(files.len() > 100, "the sweep found {} files", files.len());

        let mut hits = Vec::new();
        for path in &files {
            let Ok(src) = std::fs::read_to_string(path) else {
                continue;
            };
            let code = without_comments(&src);
            for (at, _) in code.match_indices("lost_focus()") {
                let from = at.saturating_sub(160);
                let to = (at + 160).min(code.len());
                let Some(window) = code.get(from..to) else {
                    continue;
                };
                if !window.contains("changed()") {
                    continue;
                }
                // The marker is looked for in the *original* text, since it
                // lives in a comment and `code` has blanked it.
                let line = code[..at].matches('\n').count() + 1;
                // Sixteen lines back and four forward, because a `cargo
                // fmt`-wrapped condition puts the term being matched a long way
                // below the comment that excuses it — measured at seven, ten
                // and eleven lines on the five sites in the tree when this was
                // written, so a window of four found none of them.
                let near = src
                    .lines()
                    .skip(line.saturating_sub(16))
                    .take(20)
                    .any(|l| l.contains(OK));
                if !near {
                    hits.push(format!("{}:{line}", path.display()));
                }
            }
        }
        // valve-gate-not-a-commit: the message below quotes the two names, so
        // this gate is its own first hit.
        assert!(
            hits.is_empty(),
            "a control is deciding it is finished from `lost_focus()` and \
             `changed()`, which is the expression §15 D316 removed. Use the \
             engagement latch, or mark the line `{OK}` if it is a gate rather \
             than a commit condition:\n  {}",
            hits.join("\n  ")
        );
    }
}

/// What `OndinApp::edit_valve` *reports*, which one caller now depends on
/// (§15 D524).
#[cfg(test)]
mod edit_valve_report_tests {
    use super::OndinApp;
    use ondin_core::kurbo::Size;
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    fn app_with_a_rect(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0xE7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let rect = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: rect,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        }]))
        .expect("a rect");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(rect);
        (app, rect)
    }

    /// One frame: a `DragValue` whose number becomes an opacity, routed through
    /// the valve. Returns the widget's rect and what the valve reported.
    fn frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        id: NodeId,
        events: Vec<egui::Event>,
    ) -> (egui::Rect, bool) {
        let mut out = (egui::Rect::NOTHING, false);
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(280.0, 300.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                let mut pct = app
                    .session
                    .doc
                    .get(id)
                    .map_or(100.0, |n| n.opacity() * 100.0);
                let resp = ui.add(egui::DragValue::new(&mut pct).range(0.0..=100.0));
                let tx = Transaction(vec![Operation::SetOpacity {
                    id,
                    opacity: pct / 100.0,
                }]);
                out = (resp.rect, app.edit_valve(&resp, tx));
            },
        );
        out
    }

    fn click(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// **The valve reports `true` on exactly the frame it commits, and on no
    /// other** — which is what makes a caller able to act *after* a commit
    /// without predicting one.
    ///
    /// The prediction it replaces was `!dragged() && (drag_stopped() ||
    /// lost_focus() || changed())`, the expression §15 D316 removed from the
    /// valve. Over the events below that answers `true` on **every keystroke
    /// frame**, where the valve is only previewing, and the **two** readers of
    /// it — `inspector::valve_slot`'s Group Colors retarget and
    /// `retarget_mixed_paint`, ten lines below it — then moved a row onto a
    /// colour the document did not have.
    ///
    /// ⚠️ The count is asserted before the identity of the frame, and the flip
    /// bears that ordering out — but the number is worse than predicted.
    /// Restoring the prediction gives **four** trues where the valve commits
    /// once: `[…, true, true, false, true, true, false]` — both keystrokes, and
    /// then *both* of the two frames a `DragValue` reports `lost_focus()` on.
    /// The sibling test is redder still: an **abandoned** edit is reported as
    /// committing **twice**, `[true, true, false, false]`, so the row would
    /// retarget onto a colour that was cancelled.
    #[test]
    fn the_valve_reports_true_on_one_frame_and_it_is_not_a_keystroke() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, id) = app_with_a_rect(&ctx);
        let mut at = egui::Pos2::ZERO;
        for _ in 0..2 {
            at = frame(&ctx, &mut app, id, Vec::new()).0.center();
        }

        let mut reports = Vec::new();
        for events in [
            vec![egui::Event::PointerMoved(at)],
            vec![click(at, true)],
            vec![click(at, false)],
            Vec::new(),
        ] {
            reports.push(frame(&ctx, &mut app, id, events).1);
        }
        let typed = reports.len();
        for ch in "50".chars() {
            reports.push(frame(&ctx, &mut app, id, vec![egui::Event::Text(ch.to_string())]).1);
        }
        let after_typing = reports.len();
        let away = egui::pos2(5.0, 260.0);
        for events in [
            vec![click(away, true)],
            vec![click(away, false)],
            Vec::new(),
            Vec::new(),
        ] {
            reports.push(frame(&ctx, &mut app, id, events).1);
        }

        assert_eq!(
            reports.iter().filter(|r| **r).count(),
            1,
            "one edit, one committing frame: {reports:?}"
        );
        assert!(
            reports[typed..after_typing].iter().all(|r| !r),
            "and none of the keystroke frames is it: {reports:?}"
        );
    }

    // ⚠️ **Two things about the probe widget above, both measured, both recorded
    // so the next reader does not take either for a bug.**
    //
    // It is a bare `egui::DragValue`, and it does **not** land the typed number:
    // the frame the valve commits on carries the value the field started with,
    // which is the one-frame-late write-back §15 D519 is about. The real
    // controls do not have that problem, and that was checked rather than
    // assumed — `inspector::scale_row_tests::a_typed_value_lands_when_the_field_is_clicked_away_from`
    // drives a `value_field`, which every production caller uses, through the
    // identical events and lands `50`.
    //
    // Which is why there is no assertion here on `undo_depth`. The transaction
    // the committing frame carries is a no-op, `commit_inner` drops it (§15
    // D428), and the history never moves — while `EditorSession::commit` still
    // returns `true`, because *"the commit took"* is what it has always meant
    // (see `commit_edit`). So the valve reports the **frame**, not a change to
    // the document, and that is the right question for both readers:
    // `inspector::valve_slot` asks *"has the edit ended?"* before comparing the
    // colour itself.

    /// **An abandoned edit reports `false` on every frame**, because nothing was
    /// committed — which the prediction could not express at all: `lost_focus()`
    /// is just as true when `Escape` was the way out.
    #[test]
    fn an_escaped_edit_reports_nothing() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, id) = app_with_a_rect(&ctx);
        let mut at = egui::Pos2::ZERO;
        for _ in 0..2 {
            at = frame(&ctx, &mut app, id, Vec::new()).0.center();
        }
        frame(&ctx, &mut app, id, vec![egui::Event::PointerMoved(at)]);
        frame(&ctx, &mut app, id, vec![click(at, true)]);
        frame(&ctx, &mut app, id, vec![click(at, false)]);
        frame(&ctx, &mut app, id, Vec::new());
        for ch in "50".chars() {
            frame(&ctx, &mut app, id, vec![egui::Event::Text(ch.to_string())]);
        }

        let mut reports = Vec::new();
        reports.push(
            frame(
                &ctx,
                &mut app,
                id,
                vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
            )
            .1,
        );
        for _ in 0..3 {
            reports.push(frame(&ctx, &mut app, id, Vec::new()).1);
        }

        assert!(
            reports.iter().all(|r| !r),
            "nothing committed, so nothing is reported: {reports:?}"
        );
        assert!(
            (app.session.doc.get(id).unwrap().opacity() - 1.0).abs() < 1e-9,
            "and the opacity is untouched"
        );
    }
}

/// A drop of several image files is one gesture (§15 D522).
#[cfg(test)]
mod image_drop_tests {
    use super::{LoadedImage, OndinApp, content_hash};
    use ondin_core::kurbo::{Point, Size};
    use ondin_core::{Document, IdSource, ImageEntry, ImageFormat, ImageSource, NodeId};

    /// One picture, distinct from every other `n`, without going near a decoder.
    ///
    /// `drop_images_at` reads four fields off a `LoadedImage` — the id, the
    /// entry, the intrinsic size and the name — and none of them needs the bytes
    /// to be a real PNG: the decode happens in `load_image_bytes`, upstream, and
    /// what is under test here is how many transactions the placement makes.
    /// Hashing distinct bytes is what makes the ids distinct, which is the part
    /// the dedupe arm reads.
    fn loaded(n: u8, name: Option<&str>) -> LoadedImage {
        let bytes: Vec<u8> = vec![n; 8];
        LoadedImage {
            id: content_hash(&bytes),
            entry: ImageEntry {
                source: ImageSource::Embedded(bytes.into()),
                format: ImageFormat::Png,
                width: 4,
                height: 4,
            },
            intrinsic: Size::new(4.0, 4.0),
            name: name.map(str::to_string),
        }
    }

    fn app_with_an_empty_document(ctx: &egui::Context) -> OndinApp {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x1D1);
        let doc = Document::new(ids.mint());
        app.session.adopt_document(doc, None);
        app
    }

    /// Every layer on the root. In a document this test built empty they are all
    /// the drop's own — **a picture is a `Rect` carrying an image fill**, not a
    /// kind of its own, so there is nothing narrower to ask for.
    fn pictures(app: &OndinApp) -> Vec<NodeId> {
        let doc = &app.session.doc;
        doc.get(doc.root())
            .map(|n| n.children().to_vec())
            .unwrap_or_default()
    }

    /// **One drop is one undo step, however many files it carried** (§15 D522).
    ///
    /// The reported failure: three PNGs dragged in together placed three layers,
    /// said *"Placed 3 images"* once, and left `undo_depth` at **3**. One
    /// `Ctrl+Z` removed the last picture and left the other two, and only the
    /// last was selected — so there was not even a selection to delete the rest
    /// with.
    ///
    /// ⚠️ **`commit_run` cannot merge these**, which the review measured before
    /// this was written: `history::commit_into_run` refuses a transaction unless
    /// every op `overwrites()`, and that is `None` for `CreateNode`. So the
    /// accumulation has to happen before the commit.
    ///
    /// ⚠️ Flipped by putting the loop back on `create_image_at`: red at the
    /// **count**, 3 against 1.
    #[test]
    fn one_drop_of_three_files_is_one_undo_step() {
        let ctx = egui::Context::default();
        let mut app = app_with_an_empty_document(&ctx);
        let depth0 = app.session.history.undo_depth();

        app.drop_images_at(
            vec![loaded(1, None), loaded(2, None), loaded(3, None)],
            Some(Point::ZERO),
        );

        assert_eq!(pictures(&app).len(), 3, "three layers landed");
        assert_eq!(
            app.session.history.undo_depth() - depth0,
            1,
            "one drop is one step"
        );
        assert_eq!(
            app.session.selection.ids().len(),
            3,
            "and all three are selected, so Delete undoes the drop by hand too"
        );

        app.session.undo();
        assert_eq!(
            pictures(&app).len(),
            0,
            "one Ctrl+Z takes the whole drop back, not the last file of it"
        );
    }

    /// **Three files land at three consecutive indices under three distinct
    /// names**, which is what the advancing document buys.
    ///
    /// ⚠️ Flipped by building every placement against the untouched document —
    /// `image_tx(None, …)` inside the loop. Red at the names, all three coming
    /// back **`"Image 1"`**, and green at the count above it: three layers still
    /// land, they are just indistinguishable. The same flip is red in
    /// `one_blob_dropped_twice_is_embedded_once` — plain backticks, §15 D319 —
    /// for a different reason and at
    /// a different assertion, which is why that one is not folded into this.
    #[test]
    fn a_batch_numbers_its_layers_against_the_siblings_it_is_adding() {
        let ctx = egui::Context::default();
        let mut app = app_with_an_empty_document(&ctx);

        app.drop_images_at(
            vec![loaded(1, None), loaded(2, None), loaded(3, None)],
            Some(Point::ZERO),
        );

        let names: Vec<String> = pictures(&app)
            .into_iter()
            .filter_map(|id| app.session.doc.get(id).map(|n| n.name().to_string()))
            .collect();
        assert_eq!(names.len(), 3);
        let mut unique = names.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 3, "three distinct names, not {names:?}");
    }

    /// **The same file dropped twice embeds its bytes once**, which the batch
    /// could easily have broken: `has_image` is read off the tree, and the first
    /// copy's `SetImage` is only in the tree once the placement before it has
    /// been applied.
    ///
    /// ⚠️ Flipped the same way as the test above — `image_tx(None, …)` — and it
    /// is red at the **layer count**, 1 against 2, not at the image count where
    /// it was predicted. Both placements embed the blob, the second transaction
    /// is then refused when it is applied to the advancing document, and the
    /// layer never lands at all. **A duplicated write does not merely duplicate;
    /// here it loses the picture**, which is the sharper claim and is why the
    /// count is asserted first.
    #[test]
    fn one_blob_dropped_twice_is_embedded_once() {
        let ctx = egui::Context::default();
        let mut app = app_with_an_empty_document(&ctx);

        app.drop_images_at(
            vec![loaded(7, Some("a")), loaded(7, Some("b"))],
            Some(Point::ZERO),
        );

        assert_eq!(pictures(&app).len(), 2, "two layers");
        assert_eq!(
            app.session.doc.image_count(),
            1,
            "sharing one entry between them"
        );
    }
}

/// Which floating things occupy the popover lane (§15 D533).
#[cfg(test)]
mod popover_lane_tests {
    use super::{OndinApp, POPOVER_W, TopMenu};

    /// **Every popover in the lane reserves it, and the two Export ones were
    /// missing** — `[S16.4-L1-05]`.
    ///
    /// `picker_lane_right` reserved on four states where six popovers hang in
    /// that lane. Opening the Export card's row settings or the Export menu and
    /// then clicking a Fill chip put the picker **under** it: the popover is
    /// `Order::Foreground`, the picker `Order::Middle`, and 236 of the picker's
    /// 240 points land inside the popover's span.
    ///
    /// ⚠️ **Driven as a table over all six**, because the defect is a list that
    /// is short by two and a test naming only the two would not stop a seventh
    /// from being forgotten the same way. The `None` row is the control: with
    /// nothing open the anchor comes back untouched, so a version of
    /// `picker_lane_right` that reserved unconditionally fails here — and that
    /// is the plausible wrong fix, since it looks like it can only help.
    ///
    /// ⚠️ Flipped by dropping `export_menu` and `export_row_open` from
    /// `a_popover_is_open`: red on those two rows, green on the other four and
    /// on the control.
    #[test]
    fn every_popover_in_the_lane_reserves_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        // One pass so `content_rect` is the screen rather than nothing.
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 900.0),
                )),
                ..Default::default()
            },
            |_| {},
        );
        let lane = OndinApp::popover_left(&ctx, POPOVER_W);
        // A chip well to the right of the lane, so "reserved" and "untouched"
        // cannot be the same number.
        let anchor = lane + 300.0;

        type Arm = (&'static str, fn(&mut OndinApp));
        let open: [Arm; 5] = [
            ("type_menu", |a| {
                a.type_menu = Some(Default::default());
            }),
            ("stroke_menu", |a| {
                a.stroke_menu = Some(Default::default());
            }),
            ("effect_menu", |a| {
                a.effect_menu = Some((ondin_core::IdSource::new(0xF0).mint(), 0));
            }),
            ("export_menu", |a| a.export_menu = true),
            ("export_row_open", |a| a.export_row_open = Some(0)),
        ];
        for (name, arm) in open {
            let mut app = OndinApp::headless(&ctx);
            arm(&mut app);
            assert_eq!(
                app.picker_lane_right(&ctx, anchor),
                lane,
                "{name} must reserve the lane"
            );
        }

        // The control, and it is what stops this passing against a lane reserved
        // unconditionally.
        let clean = OndinApp::headless(&ctx);
        assert_eq!(
            clean.picker_lane_right(&ctx, anchor),
            anchor,
            "with nothing open the picker goes where its chip says"
        );
        let _ = TopMenu::None;
    }
}

/// `Ctrl+Shift+G` over more than one container (§15 D521).
#[cfg(test)]
mod ungroup_tests {
    use super::OndinApp;
    use ondin_core::build;
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    /// Four 20×20 rects in a row on the root, grouped into two pairs, with both
    /// groups selected — the fixture the finding measured.
    pub(super) fn app_with_two_groups(
        ctx: &egui::Context,
    ) -> (OndinApp, NodeId, NodeId, Vec<NodeId>) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x6E0);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let made: Vec<NodeId> = (0..4)
            .map(|i| {
                let id = ids.mint();
                doc.apply(&Transaction(vec![Operation::CreateNode {
                    id,
                    parent: root,
                    index: i,
                    kind: NodeKind::Rect {
                        size: Size::new(20.0, 20.0),
                        corner_radii: Default::default(),
                    },
                    transform: Some(Affine::translate((40.0 * i as f64, 0.0))),
                    name: None,
                }]))
                .expect("a rect");
                id
            })
            .collect();
        app.session.adopt_document(doc, None);
        let (tx, g1) =
            build::group(&app.session.doc, &mut app.session.ids, &made[0..2]).expect("a group");
        app.session.commit(tx);
        let (tx, g2) =
            build::group(&app.session.doc, &mut app.session.ids, &made[2..4]).expect("a group");
        app.session.commit(tx);
        app.session.selection.set(vec![g1, g2]);
        (app, g1, g2, made)
    }

    /// Groups reachable from the root. Counted by walking rather than by
    /// `contains`, because an undone `DeleteNode` need not hand back the same
    /// `NodeId` and the claim is about how many containers there are.
    fn groups_left(app: &OndinApp) -> usize {
        fn walk(doc: &Document, id: NodeId, n: &mut usize) {
            let Some(node) = doc.get(id) else { return };
            if matches!(node.kind(), NodeKind::Group) {
                *n += 1;
            }
            for c in node.children() {
                walk(doc, *c, n);
            }
        }
        let mut n = 0;
        walk(&app.session.doc, app.session.doc.root(), &mut n);
        n
    }

    /// **One press of `Ctrl+Shift+G` is one undo step, however many containers
    /// it dissolves.**
    ///
    /// The reported failure, as the finding's own two-line table: an ungroup of
    /// two groups measured `undo_delta=2, groups_left=0`, and one `Ctrl+Z`
    /// brought **one** group back and left the other dissolved.
    /// `ungroup_selection` was the only per-subject commit loop in the file;
    /// every sibling verb in the same `impl` builds one transaction and commits
    /// once.
    ///
    /// ⚠️ **Not fixable with `commit_run`, which the review measured before
    /// this was written.** `history::commit_into_run` refuses to merge unless
    /// every op `overwrites()`, which is `None` for `Reparent` and
    /// `DeleteNode` — so the run mechanism is structurally unavailable to every
    /// structural command in the app, and the accumulation has to happen in the
    /// builder. That is `build::ungroup_all` — plain backticks, §15 D319: this
    /// is a `#[cfg(test)]` module, so a link here is checked by nothing.
    ///
    /// ⚠️ Flipped by putting the commit back inside a per-container loop: red
    /// at the **first** assertion, reading 2 for 1 — the undo assertion three
    /// lines down never runs, so it is the count that is load-bearing here and
    /// the round trip that corroborates it.
    #[test]
    fn one_ungroup_of_two_groups_is_one_undo_step() {
        let ctx = egui::Context::default();
        let (mut app, _g1, _g2, made) = app_with_two_groups(&ctx);
        let depth0 = app.session.history.undo_depth();
        assert_eq!(groups_left(&app), 2, "the fixture has two groups");

        app.ungroup_selection();

        assert_eq!(
            app.session.history.undo_depth() - depth0,
            1,
            "one press is one step"
        );
        assert_eq!(groups_left(&app), 0, "and both dissolved");
        assert_eq!(
            app.session
                .doc
                .get(app.session.doc.root())
                .unwrap()
                .children(),
            &made[..],
            "the four children landed back in their own order"
        );
        assert_eq!(
            app.session.selection.ids(),
            &made[..],
            "and the freed children are what is selected"
        );

        app.session.undo();
        assert_eq!(
            groups_left(&app),
            2,
            "one Ctrl+Z brings back both, not one of them"
        );
    }

    /// **The inspector's identity row is the same verb, so this is its aftermath
    /// too** — §15 D562.
    ///
    /// `ungroup_selected_from_panel` was a six-line second implementation, and it
    /// diverged in the half no gate looks at: it dissolved the group and left the
    /// selection **empty**, dropping the user into the empty-canvas card where the
    /// keyboard door leaves the freed layers selected and ready to move. It also
    /// never wrote a status line, so nothing on screen said the command had run,
    /// and it discarded `build::ungroup`'s `Err`, so a refusal was silent. It is
    /// deleted; the button calls this.
    ///
    /// ⚠️ **The fixture is one group of one rect, and that is deliberate.** The
    /// test above uses two groups because it is about the *transaction count*;
    /// this is the finding's own fixture, and a single container is the case where
    /// "the selection is empty afterwards" reads as an ordinary deselect rather
    /// than as a bug.
    ///
    /// ⚠️ **There is no test of the button itself, and there cannot usefully be
    /// one here** — the click site is one line inside `inspector_ui`'s identity
    /// row. What replaces it is that there is nothing left to disagree: the second
    /// implementation is gone, so the panel and `Ctrl+Shift+G` are the same code
    /// by construction rather than by assertion. **The flip is the status line** —
    /// dropping `session.info("Ungrouped")` is red at the last assertion and green
    /// at every other one, which is the panel copy's exact signature.
    #[test]
    fn ungrouping_selects_the_freed_layer_and_says_so() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0x6E1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let rect = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: rect,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        }]))
        .expect("a rect");
        app.session.adopt_document(doc, None);
        let (tx, group) =
            build::group(&app.session.doc, &mut app.session.ids, &[rect]).expect("a group");
        app.session.commit(tx);
        app.session.selection.set_one(group);
        // The fixture is in the state the assertions are about: one container,
        // selected, holding the one rect.
        assert_eq!(groups_left(&app), 1);
        assert_eq!(app.session.selection.ids(), &[group]);

        app.ungroup_selection();

        assert_eq!(groups_left(&app), 0, "the container is gone");
        assert_eq!(
            app.session.selection.ids(),
            &[rect],
            "and the freed layer is selected, not nothing"
        );
        assert_eq!(
            app.session.status().text,
            "Ungrouped",
            "the panel's copy said nothing at all"
        );
    }

    // ⚠️ **The half-applied *failure* the finding also names has no test here,
    // because it cannot be reached from this verb — measured while trying to
    // write one.** `ungroup_selection` filters the selection to live
    // `Group`/`Boolean` nodes, which rules out every error `build::ungroup`
    // can return: `WrongKindForOp` and `NoSuchNode` are filtered, and
    // `CannotModifyRoot` cannot arrive because the root is not a `Group` and
    // so never survives the filter either — a first attempt at this test
    // selected the root, watched the filter drop it, and measured the *good*
    // group being ungrouped normally. So "a command that half-applies and then
    // says it failed" was a latent hazard in the loop rather than a reachable
    // one, and the guarantee now lives where it can be stated:
    // `build::ungroup_all` returns `Err` having built nothing, pinned by
    // `one_bad_container_builds_no_ops_at_all` in `ondin-core/tests/build.rs`.
}

/// The library wiring: what happens to the file when the app saves, autosaves,
/// renames or deletes.
///
/// **Reachable at all because of §15 D303** — `OndinApp::headless` builds the
/// whole app with no GPU and no window, so the decisions this module is about
/// are ordinary function calls. What it still cannot see is a pixel; every
/// assertion here is about the *filesystem*, which is the right level for it,
/// since that is where the library's whole contract lives.
#[cfg(test)]
mod library_wiring_tests {
    use super::{OndinApp, TopMenu, View};
    use crate::library::state::Library;
    use eframe::egui;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ondin-wiring-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Put the app in the state where the *next* `autosave_tick` will fire:
    /// autosave on, the interval elapsed.
    ///
    /// ⚠️ **This helper exists because three tests got it wrong the same way.**
    /// They set `autosave_secs = 0` meaning "fire immediately"; it means
    /// **off**, so `autosave_tick` returned on its first line and every one of
    /// those tests was asserting that a function which had done nothing had done
    /// nothing. The flip that found it — making autosave pin a version — left
    /// all three green. A named helper is the fix: there is now one place that
    /// knows what "armed" means, and it cannot be got wrong per-test.
    fn arm_autosave(app: &mut OndinApp) {
        app.prefs.autosave_secs = 1;
        app.last_autosave = std::time::Instant::now() - std::time::Duration::from_secs(3600);
    }

    /// A headless app whose library is an empty temp folder.
    ///
    /// ⚠️ **The library is replaced after construction rather than configured
    /// through `Prefs`,** because `Prefs::base_folder` is only read by
    /// `OndinApp::with` — pointing it afterwards would leave `library.root`
    /// stale and the test would assert against the wrong folder.
    fn app_at(ctx: &egui::Context, root: PathBuf) -> OndinApp {
        let mut app = OndinApp::headless(ctx);
        app.prefs.base_folder = Some(root.clone());
        app.library = Library::open(root);
        app
    }

    /// **Opening a document drops the per-corner-radius disclosure**, which holds
    /// `NodeId`s minted from the document being left.
    ///
    /// `reset_transient_state` is a hand-written list of assignments with nothing
    /// checking it, and this field was the one member of the class cleared
    /// **nowhere in the workspace** — inserted at the Appearance panel's
    /// disclosure, removed only when the same node's disclosure is closed, so it
    /// accumulated for the life of the process across every document opened.
    ///
    /// ⚠️ **No wrong pixel follows from a stale entry and that is worth stating,
    /// because it is why this went unnoticed.** `NodeId` is `(actor, seq)` with a
    /// random per-session actor, so an id from the outgoing document *dangles*
    /// rather than collides: the set simply grows. What is asserted here is
    /// therefore the growth, which is the whole of the defect.
    ///
    /// ⚠️ **Flipped** by removing `self.per_corner_radius.clear()` from
    /// `reset_transient_state`: fails on the emptiness assertion with the stale id
    /// named — the predicted site, because there is only one.
    #[test]
    fn opening_a_document_drops_the_previous_ones_corner_disclosures() {
        let ctx = egui::Context::default();
        let root = temp_root("corner-disclosure");
        let mut app = app_at(&ctx, root.clone());

        // A node of the outgoing document, disclosed.
        let id = app.session.ids.mint();
        app.per_corner_radius.insert(id);
        assert_eq!(app.per_corner_radius.len(), 1, "fixture");

        let other = ondin_core::Document::new(app.session.ids.mint());
        let path = root.join("next.ondin");
        std::fs::write(&path, ondin_core::io::save(&other).unwrap()).unwrap();
        app.open_path(&path);

        assert!(
            app.per_corner_radius.is_empty(),
            "the disclosure set still holds {:?} from the document just left",
            app.per_corner_radius
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_app_opens_on_the_dashboard() {
        let ctx = egui::Context::default();
        let app = OndinApp::headless(&ctx);
        // `headless` goes through `with`, which defaults to the editor; the real
        // launch path calls `open_at_launch`. Asserting `with`'s answer here is
        // what pins that the *default* is the cheap one and the dashboard is a
        // deliberate step — see `OndinApp::open_at_launch`.
        assert_eq!(app.view, View::Editor);
    }

    /// ⚠️ **No test may write the developer's preferences file**, and this is the
    /// one assertion standing between the suite and that.
    ///
    /// It is here rather than in `prefs.rs` because the flag is only useful if
    /// `OndinApp::headless` sets it, and `headless` is what every probe in the
    /// workspace goes through. Reported as *"my base folder is some random
    /// directory: `…\Temp\ondin-wiring-crumb-25960`. I'm doing `cargo run`"* — a
    /// `temp_root` from this very module, written into `%APPDATA%\ondin\prefs.json`
    /// by whichever probe reached one of the app's nine `prefs.save()` calls, and
    /// left pointing at a folder its own test had already deleted.
    ///
    /// **Flip-checked** by removing `ephemeral: true` from `headless`: this fails
    /// immediately, and it is the only gate in the workspace that does — which is
    /// the point of it, since the damage it prevents is invisible to every other.
    /// `app_at` is included because it rewrites `base_folder` after construction,
    /// and that is the exact value that leaked.
    #[test]
    fn a_headless_app_can_never_write_the_preferences_file() {
        let ctx = egui::Context::default();
        let app = OndinApp::headless(&ctx);
        assert!(
            app.prefs.ephemeral,
            "a headless app's preferences are in memory only"
        );

        let root = temp_root("prefs-guard");
        let app = app_at(&ctx, root.clone());
        assert!(
            app.prefs.ephemeral,
            "and a probe that repoints the library keeps it that way — that \
             repointed path is the one that leaked"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Saving an unfiled document files it: a real file appears in the library,
    /// under a slug of its name, with an id and a creation stamp.
    #[test]
    fn a_first_save_files_the_document_with_no_dialog() {
        let ctx = egui::Context::default();
        let root = temp_root("first-save");
        let mut app = app_at(&ctx, root.clone());
        assert!(
            app.session.path.is_none(),
            "the starter document is unfiled"
        );

        let mut meta = app.session.doc.meta().clone();
        meta.name = Some("My cool design".into());
        app.session.doc.set_meta(meta);
        app.save_file(false);

        assert_eq!(app.session.path, Some(root.join("my-cool-design.ondin")));
        assert!(root.join("my-cool-design.ondin").exists());
        assert!(app.session.doc.meta().id.is_some());
        assert!(app.session.doc.meta().created.is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ⚠️ **`Ctrl+S` pins and autosave does not** — the distinction the whole
    /// two-argument shape of `save_file` exists for.
    ///
    /// ⚠️ **Flip-check, run — and the first attempt did not bite, which is how
    /// the whole module's autosave fixture turned out to be wrong.** Making
    /// `autosave_tick` pin left every test green, because three of them set
    /// `autosave_secs = 0` meaning "immediately" when it means **off**: the
    /// function returned on its first line and the tests were asserting that
    /// nothing had happened after nothing had been asked. `arm_autosave` and the
    /// "must actually have autosaved" assertion below are the fix, and with them
    /// the flip fails here, in `autosave_never_creates_a_file_on_its_own` and in
    /// `deleting_the_open_document_stops_the_editor_writing_it_back` — three
    /// sites for a one-line change, which is the shape a genuinely load-bearing
    /// argument has.
    #[test]
    fn only_a_manual_save_pins_a_version() {
        let ctx = egui::Context::default();
        let root = temp_root("pins");
        let mut app = app_at(&ctx, root.clone());
        app.save_file(false);
        let id = app.session.doc.meta().id.clone().unwrap();
        assert!(
            crate::library::store::versions(&root, &id).is_empty(),
            "an ordinary save pins nothing"
        );

        app.save_file(true);
        assert_eq!(crate::library::store::versions(&root, &id).len(), 1);

        // The autosave path, driven the way the frame drives it: dirty, armed,
        // and past the interval.
        app.session.mark_unsaved();
        arm_autosave(&mut app);
        app.autosave_tick(&ctx);
        // ⚠️ **The tick only queues now** (§15 D393), so the assertion below —
        // which is this test's own anti-vacuity control — has to wait for the
        // worker. Without the settle it fails, saying the fixture never
        // autosaved, which is the *right* failure for the wrong reason and would
        // send the next reader after the interval rather than after the queue.
        app.disk_settle();
        assert!(
            !app.session.is_dirty(),
            "the fixture must actually have autosaved, or the next assertion is \
             about nothing — this is what `autosave_secs = 0` got wrong"
        );
        assert_eq!(
            crate::library::store::versions(&root, &id).len(),
            1,
            "autosave must not add to the history"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ⚠️ An idle session writes nothing. Without the `is_dirty` gate a document
    /// left open overnight rewrites its own bytes on every interval — invisible
    /// locally, and a file change every sync client in the world will upload.
    #[test]
    fn autosave_does_not_touch_a_clean_document() {
        let ctx = egui::Context::default();
        let root = temp_root("idle");
        let mut app = app_at(&ctx, root.clone());
        app.save_file(false);
        let path = app.session.path.clone().unwrap();
        assert!(!app.session.is_dirty());

        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        arm_autosave(&mut app);
        app.autosave_tick(&ctx);
        // Settling a queue that was never given anything, which is exactly the
        // claim: a clean document produces no job at all.
        app.disk_settle();
        let after = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(before, after, "a clean document must not be rewritten");

        // ⚠️ **The control.** Without it this passes for a version of
        // `autosave_tick` that never writes at all — which is exactly what the
        // first draft of this test was measuring, since it left autosave off.
        app.session.mark_unsaved();
        arm_autosave(&mut app);
        app.autosave_tick(&ctx);
        app.disk_settle();
        assert!(
            !app.session.is_dirty(),
            "the same armed state must write when the document *is* dirty"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The frame decides to autosave and does not pay for one** (§15 D393) —
    /// the save's half of `a_snapshot_of_a_photo_document_does_not_serialise_on_
    /// the_frame`, over the same 20 MB fixture.
    ///
    /// ⚠️ **The duration assertion is bracketed by three that are not**, and they
    /// are what stop it being about nothing: `is_saving` right after the tick says
    /// a job was actually queued, `is_dirty` beside it says the session has *not*
    /// been told the work is safe yet, and the pair after the settle says the
    /// write really happened. A tick that returned on one of its four gates would
    /// be fast and fail all three.
    ///
    /// **Flip-checked** by putting `self.save_file(false)` back as the whole body
    /// after the gates. The predicted failure site was the duration assertion and
    /// that was **wrong**: it fails one line earlier, on `is_saving`, because a
    /// synchronous save marks the session clean and never enters the third state
    /// at all. Worth keeping as the prediction that missed — the *first*
    /// assertion a wrong implementation trips is not always the one the test is
    /// named for, and here the earlier failure is the more informative of the two.
    #[test]
    fn an_autosave_of_a_photo_document_does_not_serialise_on_the_frame() {
        let ctx = egui::Context::default();
        let root = temp_root("autosave-photo");
        let mut app = app_with_a_photo(&ctx, root.clone());
        // Filed first, since autosave declines a document with no path — and
        // this one write is deliberately the synchronous kind, which is what
        // `Ctrl+S` still does.
        app.save_file(false);
        app.session.mark_unsaved();
        arm_autosave(&mut app);

        let started = std::time::Instant::now();
        app.autosave_tick(&ctx);
        let on_the_frame = started.elapsed();
        assert!(
            app.session.is_saving(),
            "no job was queued, so the timing below is about a tick that did \
             nothing"
        );
        assert!(
            app.session.is_dirty(),
            "a queued write is not a saved document — the pill says Saving…, and \
             the dot stays amber, because the bytes are not down"
        );

        app.disk_settle();
        assert!(!app.session.is_saving(), "and the third state ends with it");
        assert!(
            !app.session.is_dirty(),
            "the write must actually have happened, or the assertions above are \
             about a queue nobody serviced"
        );
        assert!(
            on_the_frame < std::time::Duration::from_millis(50),
            "the frame spent {on_the_frame:?} on an autosave it only had to \
             decide to take — the serialise is back on the UI thread"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **An edit made while the save is in flight leaves the session dirty**
    /// (§15 D393).
    ///
    /// ⚠️ **This is the one thing an asynchronous save can get wrong that a
    /// synchronous one could not.** The worker serialises a clone taken at queue
    /// time, so a commit landing during the serialise means the file will hold a
    /// document the session has already moved past. Marking it clean on arrival
    /// would put *Saved · just now* over work that exists on disk nowhere — and
    /// it would do it silently, since the pill is the only thing that would have
    /// said otherwise.
    ///
    /// The third assertion is what makes the first two mean something: it reads
    /// the file back and finds the *old* document there, so the case really is
    /// "the write succeeded and is behind" rather than "the write failed".
    ///
    /// **Flip-checked** by dropping the `self.revision == at` guard in
    /// `EditorSession::finish_save`, which is the version that trusts the
    /// worker's answer. Predicted the `is_dirty` assertion, and that is where it
    /// fails; the file assertion below stays green, which is the point — nothing
    /// about the *disk* is wrong in the broken version, only what the app
    /// believes about it.
    #[test]
    fn an_edit_during_a_save_leaves_the_session_dirty() {
        use ondin_core::kurbo::Size;
        use ondin_core::{NodeKind, Operation, Transaction};

        let ctx = egui::Context::default();
        let root = temp_root("autosave-overtaken");
        let mut app = app_with_a_photo(&ctx, root.clone());
        app.save_file(false);
        let path = app.session.path.clone().expect("filed");
        app.session.mark_unsaved();
        arm_autosave(&mut app);

        let before = app.session.revision();
        app.autosave_tick(&ctx);
        // The edit the worker's clone will not contain. 20 MB of picture is what
        // makes this a real race rather than a fiction — the serialise is still
        // running when this commits.
        let parent = app.session.doc.root();
        let id = app.session.ids.mint();
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        }])));
        assert_ne!(
            app.session.revision(),
            before,
            "the fixture must actually have moved the revision on, or this test \
             is the ordinary case wearing a different name"
        );

        app.disk_settle();
        assert!(
            app.session.is_dirty(),
            "the file holds the document as it was when the write was queued, so \
             a session that says Saved is claiming the new rect is on disk"
        );
        assert!(
            !app.session.is_saving(),
            "the write is over either way — Saving… must not stick"
        );
        let on_disk = ondin_core::io::load(&std::fs::read(&path).unwrap()).unwrap();
        assert!(
            on_disk.get(id).is_none(),
            "and the file really is the older one, which is what makes the \
             assertion above about staleness rather than about a failed write"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A failed save ends the *Saving…* and leaves autosave running**
    /// (§15 D393).
    ///
    /// ⚠️ **The bug this is really about is a state entered in one place and left
    /// in only one of two.** `begin_save` sets `saving`; the success path clears
    /// it in `finish_save`; the failure path did not, for as long as it took to
    /// write this test. The consequence is not a lost write — the session stays
    /// dirty, correctly — but the pill sticking on *Saving…* for the rest of the
    /// session and `autosave_tick`'s `is_saving` gate never opening again, which
    /// is autosave silently switching itself off after one bad write. That is the
    /// failure the whole feature exists to prevent, reached from the other side.
    ///
    /// ⚠️ **The failure is real rather than injected**: the session's path is a
    /// *directory*, so `atomic::write` writes its temp file happily and then
    /// cannot rename over it. Which is the only reason this arm is reachable at
    /// all — every other way of making a write fail wants a fault injected into
    /// `store::write_document`.
    ///
    /// **Flip-checked** by removing the `session.save_failed()` call from
    /// `apply_disk`'s `SaveFailed` arm. Predicted the `is_saving` assertion after
    /// the settle, and that is where it fails; the *last* pair would have caught
    /// it too, which is the pair that names the actual consequence.
    #[test]
    fn a_failed_save_stops_saying_saving_and_leaves_autosave_armed() {
        let ctx = egui::Context::default();
        let root = temp_root("autosave-fails");
        let mut app = app_at(&ctx, root.clone());

        // A path that cannot be written over, because it is a folder.
        let blocked = root.join("not-really-a-document.ondin");
        std::fs::create_dir_all(&blocked).unwrap();
        app.session.mark_saved(blocked.clone());
        app.session.mark_unsaved();

        arm_autosave(&mut app);
        app.autosave_tick(&ctx);
        assert!(
            app.session.is_saving(),
            "the fixture must have queued a write"
        );
        app.disk_settle();

        assert!(
            !app.session.is_saving(),
            "the pill is stuck on Saving… for the rest of the session, and the \
             gate that reads it has just switched autosave off"
        );
        assert!(
            app.session.is_dirty(),
            "and the work is still only in memory, which is the half that was \
             always right"
        );
        assert!(
            app.session.status().text.contains("Save failed"),
            "a save that did not happen has to say so — the pill going back to \
             Unsaved is what it says when nothing was attempted either. It said \
             {:?}",
            app.session.status().text
        );

        // The consequence the assertions above are really about: the *next*
        // interval still fires.
        arm_autosave(&mut app);
        app.autosave_tick(&ctx);
        assert!(
            app.session.is_saving(),
            "one failed write turned autosave off for the session"
        );
        app.disk_settle();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A writer that dies mid-save is noticed by the frame, not only by the way
    /// out** (§15 D549, `[S1.2-L1-04]`).
    ///
    /// ⚠️ **This is the test above's failure reached by the one road that had no
    /// report at all.** That one makes the *write* fail, which the worker answers
    /// with a `Done::SaveFailed` like any other outcome. Here the worker is gone,
    /// so there is no answer to send: `EditorSession::saving` stayed `Some`
    /// forever, `autosave_tick`'s `is_saving()` gate never opened again, and
    /// because it never opened no further job was queued — so `Writer::send`'s
    /// failure report, which D392 names as the backstop, was unreachable. The user
    /// was told nothing. The one message they did get was about the *other*
    /// feature: `recovery_tick` kept queueing snapshots, each `send` failed, and
    /// *"Crash recovery is not running"* appeared once.
    ///
    /// ⚠️ **`disk_results` and never `disk_settle`, and that is the whole test.**
    /// `Writer::settle`'s `Err(_)` arm has noticed this since D392; `settle` runs
    /// only from `save_file`, `on_exit` and `relocate`, none of which happen on
    /// their own. Swap the call below for `app.disk_settle()` and it passes
    /// against the unfixed code — which is the shape of the bug, and the reason
    /// the finding could sit under a green suite.
    ///
    /// ⚠️ **Flip-check, run: `drain`'s `Disconnected` arm reverted to a bare
    /// `break`** (i.e. back to `while let Ok(msg) = try_recv()`). Predicted the
    /// `is_saving` assertion, and that is where it fails — *"the pill is stuck on
    /// Saving… and autosave is off for the rest of the session"*. The last pair
    /// bites too, which is the pair that names the consequence rather than the
    /// state.
    ///
    /// ⚠️ **And a second flip, which found the assertion order was wrong:** with
    /// the `status()` assertion first, the flipped version failed on *"a save that
    /// vanished has to say so"* — true, but it reports the quieter half. What the
    /// user loses is autosave; that it went unannounced is the second question.
    /// The same correction `atomic`'s `a_failed_write_leaves_the_old_file_intact`
    /// needed, for the same reason.
    #[test]
    fn a_writer_death_mid_save_is_noticed_by_the_frame_and_not_only_by_the_exit() {
        let ctx = egui::Context::default();
        let root = temp_root("writer-dies");
        let mut app = app_at(&ctx, root.clone());

        // A write the worker will panic on rather than answer — the residue of a
        // panic anywhere in `io::save` or `store::write_document`, which is the
        // state and not the cause.
        let path = root.join("landing-v4.ondin");
        crate::library::writer::poison_document(&path);
        app.session.mark_saved(path.clone());
        app.session.mark_unsaved();

        arm_autosave(&mut app);
        app.autosave_tick(&ctx);
        assert!(
            app.session.is_saving(),
            "the fixture must have queued a write"
        );
        app.writer.as_mut().unwrap().wait_for_worker();

        // The frame path, which is the only one an editor session runs by itself.
        app.disk_results();

        assert!(
            !app.session.is_saving(),
            "the pill is stuck on Saving… for the rest of the session, and the \
             gate that reads it has switched autosave off — with no answer coming \
             from anywhere, ever, because the thread that owed one is gone"
        );
        assert!(
            app.session.status().text.contains("Save failed"),
            "and a save that vanished has to say so. It said {:?}",
            app.session.status().text
        );
        assert!(
            app.writer.as_mut().unwrap().drain().is_empty(),
            "reported once and then latched — the channel stays disconnected for \
             the rest of the process, so an unlatched report is a failure message \
             on every frame from here on"
        );

        // The consequence the assertions above are really about: the next
        // interval fires, and now says so through `send` rather than in silence.
        arm_autosave(&mut app);
        app.autosave_tick(&ctx);
        assert!(
            app.session.is_saving(),
            "one lost write turned autosave off for the session"
        );
        app.disk_results();
        assert!(
            app.session.status().text.contains("Save failed"),
            "and every attempt after the death is reported too, which is \
             `Writer::send`'s arm — unreachable until the gate above reopened"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A document that has never been filed is not autosaved into existence.
    /// Creating a file on a timer, for a canvas somebody was doodling on, puts
    /// something in their library they did not ask for.
    #[test]
    fn autosave_never_creates_a_file_on_its_own() {
        let ctx = egui::Context::default();
        let root = temp_root("no-create");
        let mut app = app_at(&ctx, root.clone());
        // Dirty, unfiled, and well past any interval.
        let mut meta = app.session.doc.meta().clone();
        meta.name = Some("Doodle".into());
        app.session.doc.set_meta(meta);
        app.session.mark_unsaved();
        arm_autosave(&mut app);

        app.autosave_tick(&ctx);
        app.disk_settle();
        assert!(app.session.path.is_none());
        assert!(
            crate::library::scan::scan(&root).is_empty(),
            "the library must still be empty"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The snapshot covers the document autosave refuses to touch, and stops
    /// covering it the moment the file does (§15 D377).
    ///
    /// ⚠️ **The unfiled document is the whole point of the first half.** It is
    /// the one case `autosave_tick` returns on by design — filing a doodle on a
    /// timer puts something in the library nobody asked for — and therefore the
    /// one case where a crash costs *everything* rather than an interval. A
    /// snapshot is not a document, so it can cover it without breaking that rule,
    /// and `scan` must still see an empty library afterwards or the two features
    /// have been collapsed into one.
    ///
    /// **Flip-checked** by giving `recovery_tick` autosave's `path.is_none()`
    /// guard, which is the version somebody writes by copying the function above
    /// it. Predicted the file assertion would fail; it does not get that far — the
    /// `expect` on `recovery.key` fires first, because a tick that returns early
    /// never mints one. Which is the more informative failure: it says the
    /// document was never *considered*, where a missing file could also mean a
    /// write that failed.
    #[test]
    fn a_snapshot_covers_the_unfiled_document_and_stops_at_the_save() {
        use crate::library::recovery;
        let ctx = egui::Context::default();
        let root = temp_root("snapshot-unfiled");
        let mut app = app_at(&ctx, root.clone());
        let mut meta = app.session.doc.meta().clone();
        meta.name = Some("Doodle".into());
        app.session.doc.set_meta(meta);
        app.session.mark_unsaved();

        // ⚠️ **`recovery_settle` after every tick that this test then asserts a
        // *file* about.** The tick queues; the worker writes (`library::writer::Writer`).
        // Nothing about the decisions below is asynchronous — the key is minted on
        // the frame — so the settle is only ever the test catching up with the
        // disk, and leaving it out is a race rather than a different answer.
        app.recovery_tick(&ctx);
        app.disk_settle();
        let key = app
            .recovery
            .key
            .clone()
            .expect("an unfiled document still gets a key");
        assert!(
            root.join(recovery::RECOVERY_DIR)
                .join(format!("{key}.ondin"))
                .exists(),
            "the one document autosave will not write is the one this is for"
        );
        assert!(
            crate::library::scan::scan(&root).is_empty(),
            "and it is not a document: the library is still empty"
        );

        // Filed and saved: the file now holds the work, so the snapshot is a
        // claim about something that exists somewhere better.
        //
        // ⚠️ **`save_file` alone, not `file_untitled` and then `save_file`.**
        // `file_untitled` files the document and returns the path *without*
        // setting `session.path` — `save_file` is what does that, from the return
        // value — so calling both files it twice, under `landing-v4.ondin` and
        // then `landing-v4-1.ondin`. The first draft of the test below did
        // exactly that and spent an assertion failure proving it.
        app.save_file(false);
        assert!(!app.session.is_dirty(), "the fixture is a saved document");
        assert_eq!(
            crate::library::scan::scan(&root).len(),
            1,
            "one save, one file"
        );
        app.recovery_tick(&ctx);
        app.disk_settle();
        assert_eq!(app.recovery.key, None);
        assert!(
            !root
                .join(recovery::RECOVERY_DIR)
                .join(format!("{key}.ondin"))
                .exists(),
            "a clean session leaves no snapshot behind"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The whole feature, end to end: a document edited past a save, a process
    /// that never gets to save again, a fresh app that offers the work back —
    /// and, the assertion that matters most, **the file on disk untouched until
    /// the user says otherwise** (§15 D377).
    ///
    /// ⚠️ **The crash is modelled by dropping the app, not by any flag.** A
    /// second `OndinApp` built over the same root is exactly what a relaunch is,
    /// and it is the only way to prove `open_at_launch` finds the snapshot
    /// through `recovery::pending` rather than through state the first app left
    /// in memory. Anything short of that — reusing the app, calling `pending`
    /// directly — would be a test of the function rather than of the wiring.
    ///
    /// ⚠️ **`Recover` must not write the document**, which is the third
    /// assertion. The snapshot is up to `SNAPSHOT_SECS` behind, and behind is not
    /// better: a recovery that overwrote the file would be this feature
    /// destroying the very thing it exists to protect, in the one case where the
    /// user has not agreed to it yet. What it does instead is load the work and
    /// say *unsaved*.
    ///
    /// **Flip-checked** by having `recover` write the snapshot to
    /// `pending.target` — the shape the word "recover" most obviously suggests.
    /// It fails at the file assertion, reporting the file as saying "Landing v5",
    /// and the `is_dirty` assertion below it never runs. Both are worth keeping:
    /// a session mislabelled clean is a bug the user would notice next time they
    /// closed the window, and a file overwritten behind their back is the thing
    /// this whole feature is supposed to be preventing.
    #[test]
    fn work_lost_to_a_crash_comes_back_and_the_file_waits_for_the_user() {
        use crate::library::recovery;
        let ctx = egui::Context::default();
        let root = temp_root("recover");
        let mut app = app_at(&ctx, root.clone());

        // A filed, saved document — the state a crash interrupts.
        let mut meta = app.session.doc.meta().clone();
        meta.name = Some("Landing v4".into());
        app.session.doc.set_meta(meta);
        app.session.mark_unsaved();
        // See the note in `a_snapshot_covers_the_unfiled_document_and_stops_at_
        // the_save`: `save_file` files an untitled document *and* records the
        // path, so calling `file_untitled` first would write two.
        app.save_file(false);
        let path = app.session.path.clone().expect("the fixture is filed");
        let on_disk = std::fs::read(&path).unwrap();

        // Work that never reaches the file: renamed in memory, snapshotted, and
        // then the process dies.
        let mut meta = app.session.doc.meta().clone();
        meta.name = Some("Landing v5".into());
        app.session.doc.set_meta(meta);
        app.session.mark_unsaved();
        app.recovery_tick(&ctx);
        let key = app.recovery.key.clone().expect("a snapshot was taken");
        // ⚠️ **No `recovery_settle` here, deliberately, and it is now part of what
        // this test proves.** The tick only *queues* the snapshot; what puts it on
        // disk before the relaunch below is the `Drop` on `library::writer::Writer`
        // joining its worker. That is the *last* of the three flushes a real quit
        // has — `on_exit` and the close arms come first — and it is the only one
        // a test can reach, which makes it the right one to pin: adding a settle
        // would test the same wiring while hiding the property that a process
        // leaving does not lose the snapshot it was in the middle of.
        drop(app);

        // The relaunch.
        let mut app = app_at(&ctx, root.clone());
        app.library.refresh();
        app.open_at_launch();
        let waiting: Vec<String> = app
            .recovery
            .pending
            .iter()
            .map(|p| p.name.clone())
            .collect();
        assert_eq!(
            waiting,
            vec!["Landing v5".to_string()],
            "the launch found the work the file never got"
        );

        let pending = app.recovery.pending.pop().unwrap();
        assert_eq!(pending.target.as_deref(), Some(path.as_path()));
        app.recover(&pending);
        // ⚠️ **`assert!` on the bytes with a *name* in the message, not
        // `assert_eq!` on two `Vec<u8>`.** The exact comparison is the one worth
        // making, and the first draft made it with `assert_eq!` — whose failure
        // was thirty kilobytes of decimal byte values, in which the one fact that
        // mattered (the file now said "Landing v5") was invisible. A failure
        // nobody can read is a test that costs its own time to diagnose.
        let after = std::fs::read(&path).unwrap();
        assert!(
            after == on_disk,
            "recovering must not write the user's file — the snapshot is behind \
             the file's own history, and overwriting it is this feature losing \
             work rather than saving it. The file now says {:?}",
            ondin_core::io::load(&after)
                .ok()
                .and_then(|d| d.meta().name.clone())
        );
        assert!(
            app.session.is_dirty(),
            "so the session has to say it does not match its file"
        );
        assert_eq!(app.session.saved_ago(), None, "the pill reads Unsaved");
        assert_eq!(
            app.session.doc.meta().name.as_deref(),
            Some("Landing v5"),
            "and the work itself is what came back"
        );
        assert_eq!(
            app.recovery.key.as_deref(),
            Some(key.as_str()),
            "the recovered session adopts the snapshot's key rather than starting \
             a second file beside it"
        );

        // Saving is the user's decision, and it is what ends the recovery.
        app.save_file(false);
        app.recovery_tick(&ctx);
        app.disk_settle();
        assert!(
            !root
                .join(recovery::RECOVERY_DIR)
                .join(format!("{key}.ondin"))
                .exists(),
            "once the file holds the work, the snapshot is a claim about nothing"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A snapshot is offered even when `reopen_last` has already reopened the
    /// very document it belongs to (§15 D377).
    ///
    /// The two are not independent: they are usually about the same document,
    /// because the one you were editing when the process died is the one
    /// `last_document` names. A launch that reopened the file and then decided
    /// there was nothing to ask about would drop the work in exactly the case the
    /// feature exists for, and drop it silently, since the document opens and
    /// looks fine.
    ///
    /// ⚠️ **The flip aimed at the ordering does not bite, and the first version of
    /// this comment claimed it did.** Moving the collect to the *end* of
    /// `open_at_launch` — the plausible wrong version — leaves both assertions
    /// green, because nothing on the reopen path touches `library.entries` or the
    /// document's mtime, which are the only two things `recovery::pending` reads.
    /// What was actually run and reported as "the ordering flip" was a cruder
    /// mutation, the collect moved *inside* the `!reopen_last` guard so it never
    /// ran at all — which of course empties the queue and proves nothing about
    /// order. `arch-scribe` caught the discrepancy by reading the function against
    /// the claim and saying it could not see the mechanism; it was right.
    ///
    /// So what this test pins is the **outcome** — a reopened document does not
    /// swallow its own recovery — and not the statement order that currently
    /// produces it. The order still earns its place for the reason
    /// `open_at_launch` gives: `open_path` calls `drop_recovery`, so a future
    /// simplification of that hand-rolled reopen would delete the snapshot, and
    /// this test is what would fail.
    #[test]
    fn reopening_the_last_document_does_not_swallow_its_own_recovery() {
        use crate::library::recovery;
        let ctx = egui::Context::default();
        let root = temp_root("recover-reopen");
        let mut app = app_at(&ctx, root.clone());
        let mut meta = app.session.doc.meta().clone();
        meta.name = Some("Landing v4".into());
        app.session.doc.set_meta(meta);
        app.session.mark_unsaved();
        app.save_file(false);
        let path = app.session.path.clone().unwrap();
        let id = app.session.doc.meta().id.clone().unwrap();

        // Work the file never got, then the process dies.
        let mut meta = app.session.doc.meta().clone();
        meta.name = Some("Landing v5".into());
        app.session.doc.set_meta(meta);
        recovery::write(&root, &id, &app.session.doc).unwrap();
        drop(app);

        let mut app = app_at(&ctx, root.clone());
        app.prefs.reopen_last = true;
        app.prefs.last_document = Some(path.clone());
        app.open_at_launch();
        assert_eq!(
            app.session.path.as_deref(),
            Some(path.as_path()),
            "the launch reopened the document, which is the interesting half"
        );
        assert_eq!(
            app.recovery
                .pending
                .iter()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>(),
            vec!["Landing v5".to_string()],
            "and still asks about the work that document is missing"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Escape and the backdrop mean *Later*, and *Later* does not delete
    /// (§15 D377).
    ///
    /// ⚠️ **This is the one modal in the app where the free dismissals could
    /// destroy work**, which is why the choice is a three-variant enum rather
    /// than a `bool`. `egui::Modal` hands `should_close()` back for both Escape
    /// and a backdrop click — the two things a hand does without reading the card
    /// — and collapsing either into *Discard* would delete somebody's only copy
    /// of an hour's drawing because they tapped a key.
    ///
    /// **Driven through `OndinApp::ui` rather than by calling `recovery_modal`**,
    /// because the thing being tested is that the card is reachable from a launch
    /// on either screen: `ui` returns at the dashboard, and this asserts the modal
    /// runs above that return.
    ///
    /// **Flip-checked** by mapping `should_close()` to `RecoverChoice::Discard` —
    /// which is what `delete_modal` does one screen away and is exactly the wrong
    /// answer here: the second assertion fails with the snapshot gone.
    #[test]
    fn escape_defers_a_recovery_and_never_deletes_it() {
        use crate::library::recovery;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("recover-escape");
        let mut app = app_at(&ctx, root.clone());
        let mut doc = ondin_core::Document::new(app.session.ids.mint());
        let meta = ondin_core::meta::DocumentMeta {
            name: Some("Doodle".into()),
            ..doc.meta().clone()
        };
        doc.set_meta(meta);
        // ⚠️ **A key with the shape `library::ids::mint` produces**, because
        // `recovery::write` now refuses anything else — the key is joined as a
        // path component, and a document asserting `"id": "../…"` in its own
        // metadata block used to reach it verbatim. This fixture read
        // `"orphan-key"` for as long as nothing checked.
        const ORPHAN_KEY: &str = "0ba9f11ce0ba9f11ce0ba9f11ce0ba9f";
        recovery::write(&root, ORPHAN_KEY, &doc).unwrap();
        app.library.refresh();
        app.open_at_launch();
        assert_eq!(app.recovery.pending.len(), 1, "the fixture is one snapshot");

        let snapshot = root
            .join(recovery::RECOVERY_DIR)
            .join(format!("{ORPHAN_KEY}.ondin"));
        let mut frame = eframe::Frame::_new_kittest();
        let mut pass = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(1320.0, 820.0),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                eframe::App::ui(app, ui, &mut frame);
            });
        };
        // ⚠️ **A frame before the key, because `ModalResponse::should_close`
        // gates Escape on `is_top_modal`** — which egui works out from the modal
        // stack the *previous* pass built. A single-pass probe delivers the key to
        // a modal egui does not yet consider topmost and reports nothing, which is
        // this test's first failure and is invisible in the source.
        pass(&mut app, Vec::new());
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

        assert!(
            app.recovery.pending.is_empty(),
            "Escape closes the card, or the ✕ and the backdrop read as broken"
        );
        assert!(
            snapshot.exists(),
            "but it must not delete the snapshot — the next launch asks again, \
             which is the whole difference between Later and Discard"
        );
        assert!(
            app.session.path.is_none() && !app.session.is_dirty(),
            "and it recovers nothing: the session is untouched"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A modal owns the keyboard, and two of the three did not.**
    ///
    /// `[S16.1-L2-01]`, §15 D464. The gate in `<OndinApp as eframe::App>::ui` is
    /// one `if`/`else if`/`else` chain, and it had arms for the context menu and
    /// for *Settings* alone. `input::resolve` is not a second guard: it asks
    /// `ctx.egui_wants_keyboard_input()`, which is **false** for a modal holding
    /// three buttons and no text field.
    ///
    /// So under *Unsaved changes* a bare `Delete` deleted the selected layer, and
    /// *Save and close* then wrote that to disk. Under the crash-recovery card —
    /// drawn **above** the view branch (§15 D377), so it can be up over a freshly
    /// reopened document — the card blocks the pointer, which makes the keyboard
    /// the only door, and it was open.
    ///
    /// ⚠️ **§15 D330 is a live *Keep* whose body names this hole, issues no
    /// verdict for it, and whose stated reason for leaving it alone was false.**
    /// It says the guard *"is in `OndinApp::update`, which … is not reachable from
    /// a test even with `OndinApp::headless`"*. There is no `update`; the method
    /// is `<OndinApp as eframe::App>::ui`, and it **is** reachable — by
    /// `eframe::Frame::_new_kittest()` plus `ctx.run_ui`, which is exactly how
    /// `escape_defers_a_recovery_and_never_deletes_it` above already drives it,
    /// and that test predates the finding.
    ///
    /// ⚠️ **The positive control is what makes the rest mean anything.** With no
    /// modal up the same harness must *delete* the layer — otherwise every green
    /// row below would be a harness that never delivered the key. And the
    /// `settings` row is the second control: one arm always worked, so this is a
    /// test about the two that did not.
    ///
    /// ⚠️ **Two passes per case**, for the reason the test above records: egui
    /// resolves a modal against the stack the *previous* pass built.
    ///
    /// ⚠️ **Flipped** by narrowing the arm back to `self.settings.is_some()`:
    /// fails on the `confirming_close` row, the layer gone and `undo_depth` at 1.
    #[test]
    fn every_modal_owns_the_keyboard_not_only_settings() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);

        /// Build a one-layer document, raise `modal`, press `Delete`, and report
        /// whether the layer survived.
        fn survives(ctx: &egui::Context, modal: &str) -> (bool, usize) {
            let root = temp_root(&format!("modal-keys-{modal}"));
            let mut app = app_at(ctx, root.clone());
            let mut ids = ondin_core::IdSource::new(1);
            let doc_root = ids.mint();
            let mut doc = ondin_core::Document::new(doc_root);
            let rect = ids.mint();
            doc.apply(&ondin_core::Transaction(vec![
                ondin_core::Operation::CreateNode {
                    id: rect,
                    parent: doc_root,
                    index: 0,
                    kind: ondin_core::NodeKind::Rect {
                        size: ondin_core::kurbo::Size::new(10.0, 10.0),
                        corner_radii: Default::default(),
                    },
                    transform: None,
                    name: None,
                },
            ]))
            .expect("one layer");
            app.session.adopt_document(doc, None);
            app.session.selection.set_one(rect);
            match modal {
                "none" => {}
                // Through the app's own door, so this is the modal the user gets
                // rather than a hand-built one.
                "settings" => app.toggle_settings(ctx),
                "confirming_close" => app.confirming_close = true,
                "recovery" => {
                    app.recovery
                        .pending
                        .push(crate::library::recovery::Pending {
                            key: "0ba9f11ce0ba9f11ce0ba9f11ce0ba9f".into(),
                            path: root.join("nothing.ondin"),
                            name: "Doodle".into(),
                            target: None,
                            written: Some(0),
                        });
                }
                _ => unreachable!(),
            }

            let mut frame = eframe::Frame::_new_kittest();
            let mut pass = |app: &mut OndinApp, events: Vec<egui::Event>| {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::pos2(0.0, 0.0),
                        egui::vec2(1320.0, 820.0),
                    )),
                    events,
                    ..Default::default()
                };
                let _ = ctx.run_ui(input, |ui| {
                    eframe::App::ui(app, ui, &mut frame);
                });
            };
            pass(&mut app, Vec::new());
            pass(
                &mut app,
                vec![egui::Event::Key {
                    key: egui::Key::Delete,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
            );
            let alive = app.session.doc.contains(rect);
            let depth = app.session.history.undo_depth();
            let _ = std::fs::remove_dir_all(&root);
            (alive, depth)
        }

        // **The positive control.** No modal: the key must reach the document, or
        // every row below is a harness that never delivered it.
        assert_eq!(
            survives(&ctx, "none"),
            (false, 1),
            "control: with nothing up, Delete must delete"
        );
        // The one arm that always worked.
        assert_eq!(
            survives(&ctx, "settings"),
            (true, 0),
            "control: Settings already owned the keyboard"
        );

        assert_eq!(
            survives(&ctx, "confirming_close"),
            (true, 0),
            "Unsaved changes must own it too — the accidental deletion is what \
             *Save and close* then writes to disk"
        );
        assert_eq!(
            survives(&ctx, "recovery"),
            (true, 0),
            "and the recovery card, which blocks the pointer and so leaves the \
             keyboard as the only door"
        );
    }

    /// The tick's third gate: nothing committed since the last snapshot means no
    /// write, however long the interval has been.
    ///
    /// ⚠️ **`is_dirty` is not enough and that is the whole test.** It stays true
    /// from the first edit until the next save, so a user who edits once and then
    /// reads their email for five minutes would produce thirty identical writes
    /// into a folder a sync client is watching — which is the exact failure
    /// `autosave_tick`'s own gate exists to avoid one level up.
    ///
    /// **Flip-checked** by dropping the `recovery.at` comparison: the last
    /// assertion fails with a second, byte-identical write. The mtime is not what
    /// is asserted on — a second write inside the same second would not move it —
    /// so the probe watches `recovery.last`, which the tick only advances when it
    /// gets as far as writing.
    #[test]
    fn an_idle_dirty_document_is_snapshotted_once_rather_than_every_interval() {
        let ctx = egui::Context::default();
        let root = temp_root("snapshot-idle");
        let mut app = app_at(&ctx, root.clone());
        app.session.mark_unsaved();

        app.recovery_tick(&ctx);
        // ⚠️ **The settle is load-bearing for the *gate*, not just for the file.**
        // `recovery.at` is written from the worker's answer and nowhere else, so
        // without this the second tick below would find `at` still `None` and
        // write again — which is the very thing this test says cannot happen, and
        // it would say it while passing for the wrong reason. On a real frame the
        // answer has ten seconds to arrive and the tick reads it at the top.
        app.disk_settle();
        let first = app.recovery.last.expect("the first tick writes");
        assert_eq!(app.recovery.at, Some(app.session.revision()));

        // The interval elapses again, with nothing committed in between.
        app.recovery.last = Some(std::time::Instant::now() - std::time::Duration::from_secs(3600));
        app.recovery_tick(&ctx);
        assert!(
            app.recovery.last.is_some_and(|t| t < first),
            "an idle session writes nothing, so the clock is not restamped either"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The frame decides to snapshot and does not pay for one** (§15 D392).
    ///
    /// The document here carries 20 MB of embedded picture, which is the middle
    /// of the four sizes `library::writer::Writer` was measured at: serialising it costs
    /// **387 ms in debug** and 25 ms in release, before the disk write, and this
    /// is a debug binary. So a tick that still serialised would be two orders of
    /// magnitude over the bound below, and the bound is not a performance target
    /// — it is far enough above what a clone and a channel send cost that the
    /// only thing that can breach it is the serialise coming back.
    ///
    /// ⚠️ **A duration assertion, which is the thing to be suspicious of, so it
    /// is paired with two that are not.** A tick that returned early — nothing
    /// dirty, no key minted — would be fast for the wrong reason and prove
    /// nothing at all; the key and the *size of the file on disk* are what say
    /// the 20 MB actually went through the serialiser. Read the three together
    /// or the first one is vacuous.
    ///
    /// **Flip-checked** by putting `recovery::write(&root, &key, &self.session.doc)`
    /// back on the frame in `recovery_tick` — the whole of the old body, so the
    /// snapshot is still written and only its *place* changes. Predicted the
    /// duration assertion, and that is where it fails: **402 ms** against the
    /// 50 ms bound, with both file assertions green, which is the shape this is
    /// meant to have — the snapshot was always correct and what was wrong was
    /// where it got paid for. The passing side measures **313 µs**, and that
    /// number is mostly the *first* tick starting the worker thread rather than
    /// the clone, so the bound has about three orders of magnitude of room and
    /// still cannot be met by anything that serialises.
    #[test]
    fn a_snapshot_of_a_photo_document_does_not_serialise_on_the_frame() {
        use crate::library::recovery;

        let ctx = egui::Context::default();
        let root = temp_root("snapshot-photo");
        let mut app = app_with_a_photo(&ctx, root.clone());

        let started = std::time::Instant::now();
        app.recovery_tick(&ctx);
        let on_the_frame = started.elapsed();
        app.disk_settle();

        let key = app.recovery.key.clone().expect("the tick took a snapshot");
        let at = root
            .join(recovery::RECOVERY_DIR)
            .join(format!("{key}.ondin"));
        let written = std::fs::metadata(&at)
            .expect("and the worker wrote it")
            .len();
        assert!(
            written as usize > 20 * (1 << 20),
            "the snapshot is {written} bytes, so the 20 MB of picture never \
             reached the serialiser and the timing below is about nothing"
        );
        assert!(
            on_the_frame < std::time::Duration::from_millis(50),
            "the frame spent {on_the_frame:?} on a snapshot it only had to decide \
             to take — serialising {written} bytes is back on the UI thread"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A document big enough that a snapshot of it is still being written when
    /// the next thing happens — which is the only fixture in which the ordering
    /// below is observable at all.
    ///
    /// 20 MB of picture, the middle of the four sizes `library::writer::Writer` was
    /// measured at: about 400 ms of serialise in a debug build, so a test can
    /// reliably do something *while* a write is in flight. The bytes are never
    /// decoded — `AddImage` stores the intrinsic size rather than reading it — so
    /// a pattern is as good as a photograph, and what is under test is the base64
    /// of it either way.
    fn app_with_a_photo(ctx: &egui::Context, root: PathBuf) -> OndinApp {
        use ondin_core::{ImageEntry, ImageFormat, ImageId, ImageSource, Operation, Transaction};
        let mut app = app_at(ctx, root);
        let bytes: std::sync::Arc<[u8]> = vec![0xA5u8; 20 * (1 << 20)].into();
        app.session
            .doc
            .apply(&Transaction(vec![Operation::AddImage {
                id: ImageId("a-photo".into()),
                entry: ImageEntry {
                    source: ImageSource::Embedded(bytes),
                    format: ImageFormat::Png,
                    width: 4000,
                    height: 3000,
                },
            }]))
            .expect("the table takes it");
        app.session.mark_unsaved();
        app
    }

    /// **A removal cannot be overtaken by a write already in flight** (§15 D392).
    ///
    /// This is the property that pays for there being one worker rather than a
    /// thread per snapshot, and it is the whole reason `forget_snapshot` exists:
    /// *Discard* and a clean quit both delete a snapshot at a moment when the
    /// last one may still be on its way to the disk. A removal that went straight
    /// to the filesystem would delete a file that is not there yet, and the write
    /// behind it would then put it back — so the user is told the work is gone,
    /// and the next launch asks about it anyway.
    ///
    /// ⚠️ **The 20 MB fixture is not decoration.** With a one-rect document the
    /// worker finishes inside the same microseconds the test spends reaching the
    /// next line, so both orderings pass and the assertion is about nothing. The
    /// picture is what holds the write open long enough for the race to have a
    /// wrong answer to give.
    ///
    /// **Flip-checked** by making `forget_snapshot` call
    /// `crate::library::recovery::remove` unconditionally — the version that
    /// ignores the queue, which is what the code did before the writer existed.
    /// Predicted this test's one assertion, and that is where it fails, reporting
    /// the resurrected snapshot. **And it fails there alone** — the other eight
    /// recovery tests stay green under that flip, including the two that assert a
    /// snapshot is gone, because their documents are one rect and the worker wins
    /// the race every time. Which is the paragraph above stated the other way
    /// round: this ordering has exactly one guard in the suite, and a fixture is
    /// what makes it one.
    #[test]
    fn discarding_a_snapshot_beats_the_write_that_is_still_in_flight() {
        use crate::library::recovery;

        let ctx = egui::Context::default();
        let root = temp_root("snapshot-discard-race");
        let mut app = app_with_a_photo(&ctx, root.clone());

        app.recovery_tick(&ctx);
        let key = app.recovery.key.clone().expect("the tick took a snapshot");
        // No settle in between: the point is that the write is *unfinished* here.
        app.drop_recovery();
        app.disk_settle();

        let at = root
            .join(recovery::RECOVERY_DIR)
            .join(format!("{key}.ondin"));
        assert!(
            !at.exists(),
            "the snapshot came back after being discarded — the write that was in \
             flight landed after the removal, so the next launch will ask about \
             work the user has already thrown away"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ⚠️ **Renaming the *open* document has to move the session's path with
    /// it.** Otherwise the editor keeps saving to a file that no longer exists,
    /// and the next autosave recreates the document under its old name — two
    /// copies, one of which the user has never seen.
    ///
    /// Flip-check, run: dropping the `session.path` update leaves the first two
    /// assertions green and fails on the third — and the predicted site was
    /// wrong, since the *library* assertion passes either way: the rename does
    /// happen on disk. It is only the editor that is left pointing at nothing.
    #[test]
    fn renaming_the_open_document_moves_the_session_with_it() {
        let ctx = egui::Context::default();
        let root = temp_root("rename-open");
        let mut app = app_at(&ctx, root.clone());
        let mut meta = app.session.doc.meta().clone();
        meta.name = Some("Landing v4".into());
        app.session.doc.set_meta(meta);
        app.save_file(false);
        let old = app.session.path.clone().unwrap();

        app.library.refresh();
        app.rename_entry = Some((old.clone(), "Homepage hero".into(), true));
        app.commit_rename();

        assert!(!old.exists(), "the old file is gone");
        assert!(root.join("homepage-hero.ondin").exists());
        assert_eq!(app.session.path, Some(root.join("homepage-hero.ondin")));
        // And the in-memory document knows its new name, or the next save would
        // put the old one straight back.
        assert_eq!(
            app.session.doc.meta().name.as_deref(),
            Some("Homepage hero")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ⚠️ Deleting the open document must clear the session's path, or the next
    /// autosave writes it straight back out of the trash — undoing the delete
    /// silently, thirty seconds later.
    #[test]
    fn deleting_the_open_document_stops_the_editor_writing_it_back() {
        let ctx = egui::Context::default();
        let root = temp_root("delete-open");
        let mut app = app_at(&ctx, root.clone());
        app.save_file(false);
        let path = app.session.path.clone().unwrap();
        app.library.refresh();

        app.trash_or_purge(&path);
        assert!(app.session.path.is_none());
        assert!(crate::library::scan::scan(&root).is_empty());
        assert_eq!(crate::library::store::trashed(&root).len(), 1);

        // The bug this is really about: a dirty document, an elapsed interval,
        // and nothing recreated.
        app.session.mark_unsaved();
        arm_autosave(&mut app);
        app.autosave_tick(&ctx);
        app.disk_settle();
        assert!(
            crate::library::scan::scan(&root).is_empty(),
            "autosave must not resurrect a deleted document"
        );
        assert!(
            app.session.is_dirty(),
            "and it must have declined to write rather than been switched off"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The trash's retention window is enforced at launch** (§15 D550,
    /// `[S1.2-L5-03]`).
    ///
    /// ⚠️ **`store::purge_trash` had one caller in the whole workspace and it was
    /// the tail of the library-settings *Save* handler**, so the *"kept for 30
    /// days"* on the Trash heading — and §9.5 of the design saying the same — were
    /// claims nothing kept. Two failures came out of that one fact, in opposite
    /// directions: a user who never opened that modal accumulated every document
    /// they had ever deleted, and a user who opened it to change the **autosave
    /// interval** permanently deleted every trashed document past the window on
    /// pressing Save, with nothing in the modal saying so.
    ///
    /// ⚠️ **The fixture has to reach the state, so both halves are asserted
    /// before the interesting one.** A test that only checks the aged file is gone
    /// passes against `purge_trash(root, 0)` — which empties the trash outright —
    /// and against a sweep that ignores the window altogether. The fresh
    /// deletion surviving is what says a *window* was applied rather than a
    /// broom.
    ///
    /// ⚠️ **Flip-check, run: the `purge_trash` call removed from
    /// `open_at_launch`.** Predicted the `aged` assertion and that is where it
    /// fails, with the file still in `.trash` — *"a deletion 31 days old is still
    /// there after a launch"*. A second flip, `keep_days` read as `0`, fails on
    /// the `fresh` assertion instead, which is the pair's other half.
    #[test]
    fn the_trash_retention_is_swept_at_launch_and_not_only_from_the_settings_card() {
        let ctx = egui::Context::default();
        let root = temp_root("trash-retention");
        let mut app = app_at(&ctx, root.clone());
        app.prefs.trash_keep_days = 30;

        let mut file = |name: &str| {
            let mut doc = ondin_core::Document::new(app.session.ids.mint());
            crate::library::store::file_document(&root, None, name, &mut doc).unwrap();
            let entry = crate::library::scan::scan(&root)
                .into_iter()
                .find(|e| e.meta.name.as_deref() == Some(name))
                .unwrap();
            crate::library::store::trash(&root, &entry).unwrap()
        };
        let aged = file("Landing v4");
        let fresh = file("Pricing table");

        // The retention clock is the trashed file's mtime (`store::trash_stamp`),
        // so ageing the deletion means setting it — not hoping for one, which is
        // `recovery`'s lesson about tests that ask the clock a question.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&aged)
            .unwrap()
            .set_modified(
                std::time::SystemTime::now() - std::time::Duration::from_secs(31 * 86_400),
            )
            .unwrap();
        assert_eq!(
            crate::library::store::trashed(&root).len(),
            2,
            "the fixture has to have two deletions in the trash to say anything"
        );

        app.open_at_launch();

        assert!(
            !aged.exists(),
            "a deletion 31 days old is still there after a launch, under a heading \
             that says it is kept for 30 days"
        );
        assert!(
            fresh.exists(),
            "and a deletion made a moment ago must survive — a sweep that takes \
             this one is not enforcing a window, it is emptying the trash"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Importing copies, keeps the name that arrived, and mints a fresh id.
    ///
    /// ⚠️ **The id is the assertion that matters.** Two libraries holding
    /// documents that claim one identity would share a version history and a
    /// thumbnail — and importing the same file twice would silently give one
    /// document, not two. The name going the other way is just as deliberate:
    /// a document that arrives called "My cool design" must not come out called
    /// "Shared export final v2" because that was the filename.
    #[test]
    fn importing_copies_a_document_and_gives_it_a_new_identity() {
        let ctx = egui::Context::default();
        let root = temp_root("import");
        let outside = temp_root("import-source");
        let mut app = app_at(&ctx, root.clone());

        // A document with a name, filed somewhere that is not the library.
        let mut doc = ondin_core::Document::new(app.session.ids.mint());
        let source =
            crate::library::store::file_document(&outside, None, "My cool design", &mut doc)
                .unwrap();
        let original_id = doc.meta().id.clone().unwrap();

        app.import_paths(std::slice::from_ref(&source));

        assert!(source.exists(), "importing copies rather than moves");
        let listed = crate::library::scan::scan(&root);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].display_name(), "My cool design");
        assert_ne!(
            listed[0].meta.id.as_deref(),
            Some(original_id.as_str()),
            "an imported document is a new document"
        );

        // The same file again is a second, independent document rather than an
        // overwrite.
        app.import_paths(&[source]);
        let listed = crate::library::scan::scan(&root);
        assert_eq!(listed.len(), 2);
        assert_ne!(listed[0].meta.id, listed[1].meta.id);

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// ⚠️ A file already inside the library is skipped, not re-imported. The
    /// dialog opens on the last-used folder, which may well *be* the base
    /// folder, and importing a document over itself would give it a second id
    /// and a `-1` filename for no reason the user could see.
    ///
    /// Flip-check, run: removing the `starts_with(&root)` guard makes the second
    /// count 2 and fails here — and the first assertion stays green either way,
    /// which is why the count is what this test asserts on.
    #[test]
    fn importing_a_file_that_is_already_in_the_library_does_nothing() {
        let ctx = egui::Context::default();
        let root = temp_root("import-self");
        let mut app = app_at(&ctx, root.clone());
        let mut doc = ondin_core::Document::new(app.session.ids.mint());
        let inside =
            crate::library::store::file_document(&root, None, "Landing v4", &mut doc).unwrap();
        app.library.refresh();
        assert_eq!(app.library.entries.len(), 1);

        app.import_paths(&[inside]);
        assert_eq!(
            crate::library::scan::scan(&root).len(),
            1,
            "the library must not have grown"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A document with no metadata block takes its name from its filename — the
    /// *same* guess the list makes, or importing one would rename it.
    #[test]
    fn importing_an_unfiled_document_names_it_from_its_filename() {
        let ctx = egui::Context::default();
        let root = temp_root("import-bare");
        let outside = temp_root("import-bare-src");
        let mut app = app_at(&ctx, root.clone());

        let doc = ondin_core::Document::new(app.session.ids.mint());
        let source = outside.join("old-hero-sketch.ondin");
        std::fs::write(&source, ondin_core::io::save(&doc).unwrap()).unwrap();

        app.import_paths(&[source]);
        let listed = crate::library::scan::scan(&root);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].display_name(), "Old hero sketch");
        // And it is filed now, so the name is in the file rather than guessed
        // again on every scan.
        assert_eq!(
            listed[0].meta.name.as_deref(),
            Some("Old hero sketch"),
            "the guess is written down once, not re-derived forever"
        );
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// A project with a folder and two files in it, plus a second empty project
    /// to transfer into.
    fn app_with_a_project(ctx: &egui::Context, root: PathBuf) -> OndinApp {
        let mut app = app_at(ctx, root.clone());
        for (id, name, folder) in [("p-1", "Kestrel", Some("kestrel")), ("p-2", "Atlas", None)] {
            app.library
                .projects
                .projects
                .push(crate::library::project::Project {
                    id: id.into(),
                    name: name.into(),
                    color: crate::library::project::PROJECT_COLORS[0].into(),
                    folder: folder.map(str::to_string),
                    created: 0,
                    archived: false,
                });
        }
        let kestrel = app.library.projects.projects[0].clone();
        for name in ["Landing v4", "Pricing table"] {
            let mut doc = ondin_core::Document::new(app.session.ids.mint());
            crate::library::store::file_document(&root, Some(&kestrel), name, &mut doc).unwrap();
        }
        app.library.save_projects();
        app.library.refresh();
        app
    }

    /// **An archived project is not a destination for a new document** (§15 D557,
    /// `[S20.3-L1-03]`).
    ///
    /// Stand on a project's grid, archive it, and the dashed *New file in
    /// {project}* card stayed on screen — clicking it filed a brand-new document
    /// straight into the archived project, which `move_modal`'s own ⚠️ says
    /// cannot be done: *"a document cannot be filed into an archived project
    /// without unarchiving it first."* The ⋮ on that document then offered
    /// **nothing** under *Move to project*, because the project it was in is
    /// archived.
    ///
    /// ⚠️ **Both halves, because they are two functions and they have to agree.**
    /// `new_file_into` decides whether the card is drawn and what it promises;
    /// `nav_project` decides where `Act::NewFile` actually files. A fix to one
    /// alone is either a card that promises a project the act will not use, or an
    /// act that files somewhere no card offered — and `Ctrl+N` reaches the second
    /// without going near the first.
    ///
    /// ⚠️ **The unfiled answer is asserted, not just the absence of the archived
    /// one.** *No project* is the honest destination here — the same as `Ctrl+N`
    /// on *All files* — and asserting it is what distinguishes the fix from one
    /// that refuses to create anything.
    ///
    /// ⚠️ **Flip-check, run: `Projects::destination` reverted to plain `get`.**
    /// Fails on the card assertion with `Some("Kestrel")` — the promise, drawn
    /// over the archived grid. The active-project control runs first and is green
    /// either way, which is what says this is about `archived` and not about the
    /// nav.
    ///
    /// **The second assertion is not separately flip-checked, and that is
    /// deliberate rather than skipped**: those two are one-line wrappers over the
    /// *same* `projects.destination(id)` call, so there is no version of the code
    /// where one answers and the other does not. What the pair is for is the
    /// opposite risk — a future fix applied to one wrapper and not the other,
    /// which is how they came to disagree in the first place.
    ///
    /// 🚨 **And that reasoning said "both functions", as though two were the
    /// population — which is the reading that hid the third door.** `arch-scribe`
    /// found `import_counting` resolving `Nav::Project` through `get` after the
    /// first two were fixed, so an import taken on an archived grid filed **every**
    /// copied document into it. Its row is below, and it *is* separately checked,
    /// because it is a third call site rather than a third wrapper over one call.
    /// ⚠️ **The lesson is the count**: writing *"both"* closed the question, and
    /// nothing in a test can notice a population stated too narrowly.
    #[test]
    fn an_archived_project_is_not_a_destination_for_a_new_document() {
        let ctx = egui::Context::default();
        let root = temp_root("archived-dest");
        let mut app = app_with_a_project(&ctx, root.clone());
        app.dash.nav = crate::panels::dashboard::Nav::Project("p-1".into());

        // **The control first**, so the fixture is known to work at all: an
        // active project is a destination and the card says so.
        assert_eq!(
            app.new_file_into().as_deref(),
            Some("Kestrel"),
            "control: an active project's grid offers the card"
        );
        assert_eq!(
            app.nav_project().map(|p| p.id),
            Some("p-1".to_string()),
            "control: and the act files into it"
        );

        // The flag set directly rather than through `set_archived`, which is
        // private and whose extra work — writing `projects.json`, reloading — is
        // not what either function under test reads. Both read
        // `library.projects`, and the assertion below is the fixture check that
        // says the state was actually reached.
        for p in &mut app.library.projects.projects {
            if p.id == "p-1" {
                p.archived = true;
            }
        }
        assert!(
            app.library.projects.get("p-1").is_some_and(|p| p.archived),
            "the fixture has to actually archive it — `get` still resolves an \
             archived project by design, which is the whole reason this bug \
             existed"
        );

        assert_eq!(
            app.new_file_into(),
            None,
            "the dashed `New file in {{project}}` card must not be drawn over an \
             archived project's grid — it is a promise about where a document \
             will land, and the app refuses that destination one menu away"
        );
        assert_eq!(
            app.nav_project(),
            None,
            "and `Act::NewFile` — which `Ctrl+N` fires without touching the card \
             — must file the document nowhere rather than into the archived \
             project"
        );

        // **The third door**, which the finding did not name and `arch-scribe`
        // found after the first two were fixed: an import taken on this nav.
        let outside = temp_root("archived-dest-src");
        let mut incoming = ondin_core::Document::new(app.session.ids.mint());
        let source =
            crate::library::store::file_document(&outside, None, "Brought in", &mut incoming)
                .unwrap();
        let (ok, failed, already) = app.import_counting(&[source]);
        assert_eq!(
            (ok, failed, already),
            (1, 0, 0),
            "the fixture's import has to succeed — and from outside the base folder, \
             or the third count absorbs it silently (§15 D693)"
        );
        app.library.refresh();
        let brought = app
            .library
            .entries
            .iter()
            .find(|e| e.meta.name.as_deref() == Some("Brought in"))
            .expect("the imported document is in the library");
        assert_eq!(
            brought.meta.project, None,
            "an import taken while standing on an archived project's grid filed \
             every copied document into it — the refused destination reached in \
             bulk, and the door `[S20.3-L1-03]` did not name"
        );
        let _ = std::fs::remove_dir_all(&outside);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A project whose files could not all be moved is kept** (§15 D556,
    /// `[S20.2-L1-03]`).
    ///
    /// ⚠️ **`delete_project`'s own ⚠️ doc argued for the file-first ordering on
    /// the grounds that *"a failure leaves the project standing with some of its
    /// files already moved, which is a state the user can see and repeat"* — and
    /// the `retain` under it ran unconditionally, so the ordering was doing
    /// nothing.** A half-failed run removed the row anyway, which is exactly the
    /// state the ordering was chosen to avoid, reached silently: the `failed`
    /// counter's only consumer is a status line. The documents were left on disk
    /// still carrying `meta.project = Some("p-1")`, which `Library::project_of`
    /// reads as unfiled — so the user watched a project and its files disappear
    /// and the files were still there, wearing a project id nothing lists.
    ///
    /// **The failure is staged rather than injected**: `.trash` is created as a
    /// **file**, so `store::trash`'s `create_dir_all` refuses and every member
    /// fails. That is the same door the finding measured, and it needs no seam.
    ///
    /// ⚠️ **The nav assertion is not decoration.** A reset to *All files* on the
    /// failure path takes the user off the one heading that now holds the files
    /// they have to deal with, which is the screen where the retry is a click
    /// away. It was unconditional too.
    ///
    /// ⚠️ **Flip-check, run: the `failed == 0` guard removed from the `retain`.**
    /// Fails on the first assertion — the row is gone — and the *files* assertion
    /// stays green, which is the whole shape of the bug: nothing was lost, and the
    /// record of where it belonged was. The control below is green either way,
    /// which is what says this is about the failure path and not about deleting.
    #[test]
    fn a_project_whose_files_could_not_move_is_kept() {
        let ctx = egui::Context::default();
        let root = temp_root("del-proj-partial");
        let mut app = app_with_a_project(&ctx, root.clone());
        assert_eq!(app.library.file_count("p-1"), 2);

        // `.trash` as a file, so `create_dir_all` cannot make the directory.
        std::fs::write(
            root.join(crate::library::scan::TRASH_DIR),
            b"not a directory",
        )
        .unwrap();
        app.dash.nav = crate::panels::dashboard::Nav::Project("p-1".into());
        app.delete_project("p-1", None, false);

        assert!(
            app.library.projects.get("p-1").is_some(),
            "the project row has to survive a run that could not move its files — \
             without it the documents are on disk carrying a project id the \
             library does not list, and nothing on screen says so"
        );
        assert_eq!(
            app.library.file_count("p-1"),
            2,
            "and both members are still in it"
        );
        assert_eq!(
            app.dash.nav,
            crate::panels::dashboard::Nav::Project("p-1".into()),
            "and the user is left on the heading that holds them, which is where \
             the retry is"
        );
        assert!(
            app.session.status().text.contains("could not be moved"),
            "and the failure has to be said out loud. It said {:?}",
            app.session.status().text
        );

        // **The control**: the same delete with a writable trash. Without it a
        // version that never deleted anything would pass every assertion above.
        std::fs::remove_file(root.join(crate::library::scan::TRASH_DIR)).unwrap();
        app.delete_project("p-1", None, false);
        assert!(
            app.library.projects.get("p-1").is_none(),
            "control: with the trash writable the row goes"
        );
        assert_eq!(
            crate::library::store::trashed(&root).len(),
            2,
            "control: and both files are in the trash"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Deleting a project transfers its files rather than losing them, and
    /// leaves the folder alone.
    #[test]
    fn deleting_a_project_can_transfer_its_files() {
        let ctx = egui::Context::default();
        let root = temp_root("del-proj-transfer");
        let mut app = app_with_a_project(&ctx, root.clone());
        assert_eq!(app.library.file_count("p-1"), 2);

        app.dash.nav = crate::panels::dashboard::Nav::Project("p-1".into());
        app.delete_project("p-1", Some("p-2"), true);

        assert!(app.library.projects.get("p-1").is_none(), "the row is gone");
        assert_eq!(app.library.file_count("p-2"), 2, "the files moved");
        assert_eq!(app.library.entries.len(), 2, "and none were deleted");
        // ⚠️ The folder stays: it may hold things this app did not put there.
        assert!(root.join("kestrel").exists());
        // And the dashboard is not left standing on a heading that is gone.
        assert_eq!(app.dash.nav, crate::panels::dashboard::Nav::All);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **`[S1.2-L2-02]`'s loss: the transfer arm resurrected the open document.**
    ///
    /// `delete_project`'s follow was written as
    /// `if session.path == Some(entry.path) && !keep_files { session.path = None }`
    /// — so on the *transfer* branch it did nothing, and the session went on
    /// naming a file in the deleted project's folder that no longer existed. The
    /// next autosave recreated it there, from a document still carrying the dead
    /// project id, and the library then held two files with one `meta.id`:
    /// version history, the cover cache and the crash-snapshot match are all
    /// keyed on that id, so all three began describing two files.
    ///
    /// ⚠️ **The three transfer tests beside this one were all green throughout**,
    /// because none of them opens a document. Everything about the *files* was
    /// always right; the bug is entirely in what the app kept pointing at.
    ///
    /// Flip-check, run: putting `&& !keep_files` back on the follow fails at
    /// *"the open document is where the transfer put it"* — the session's path,
    /// the loss. The scan assertion after it is the resurrection, and it needs
    /// the save to have happened, which is why it cannot be first.
    #[test]
    fn transferring_a_project_takes_the_open_document_with_it() {
        let ctx = egui::Context::default();
        let root = temp_root("del-proj-open");
        let mut app = app_with_a_project(&ctx, root.clone());
        let open = app.library.entries[0].path.clone();
        app.open_path(&open);
        assert!(open.starts_with(root.join("kestrel")), "{open:?}");

        app.delete_project("p-1", Some("p-2"), true);

        let now = app.session.path.clone().expect("still editing something");
        assert!(
            !now.starts_with(root.join("kestrel")),
            "the open document is where the transfer put it: {now:?}"
        );
        assert_eq!(
            app.session.doc.meta().project.as_deref(),
            Some("p-2"),
            "and it knows which project it is in now"
        );

        // The save the user makes next. Without the follow this wrote a second
        // file, at the old path, under the id the transferred one already has.
        crate::library::store::write_document(&now, &app.session.doc).unwrap();
        app.library.refresh();
        assert_eq!(
            app.library.entries.len(),
            2,
            "and there are still two documents, not three: {:?}",
            app.library
                .entries
                .iter()
                .map(|e| e.path.clone())
                .collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The other arm: the files go to Trash rather than to another project.
    ///
    /// ⚠️ **Trash, not deletion.** A project delete that removed files outright
    /// would be the only irreversible action in the library, reached from a
    /// checkbox.
    #[test]
    fn deleting_a_project_with_its_files_sends_them_to_the_trash() {
        let ctx = egui::Context::default();
        let root = temp_root("del-proj-files");
        let mut app = app_with_a_project(&ctx, root.clone());

        app.delete_project("p-1", None, false);

        assert!(app.library.entries.is_empty(), "the library is empty");
        assert_eq!(
            crate::library::store::trashed(&root).len(),
            2,
            "but both files are recoverable"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Transferring to *no project* leaves the documents unfiled and still in
    /// the library — the case a `None` target could plausibly be read as "do
    /// nothing" instead.
    #[test]
    fn transferring_to_no_project_leaves_the_files_unfiled() {
        let ctx = egui::Context::default();
        let root = temp_root("del-proj-none");
        let mut app = app_with_a_project(&ctx, root.clone());

        app.delete_project("p-1", None, true);

        assert_eq!(app.library.entries.len(), 2);
        assert!(
            app.library.entries.iter().all(|e| e.meta.project.is_none()),
            "each file's block must say it belongs to nothing"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ⚠️ A broken `projects.json` must stop a project delete **before it moves
    /// any files**.
    ///
    /// ⚠️ **This test found a real bug by way of a flip that did not bite.** The
    /// first version asserted only that the bytes of `projects.json` were
    /// untouched — and removing the latch from `delete_project` left it green,
    /// because `Library::save_projects` latches on its own and the bytes were
    /// never at risk. What *was* at risk was the files: the check sat below the
    /// loop that moves them, so a refused delete would have moved every document
    /// out of a project that then stayed exactly where it was. The check moved
    /// to the top of the function and this test now asserts the thing that was
    /// actually broken.
    ///
    /// Flip-check, re-run against the fixed version: moving the check back below
    /// the loop fails on the file assertions and leaves the bytes assertion
    /// green — the reverse of what the first draft measured.
    #[test]
    fn a_broken_projects_file_stops_a_project_delete_before_it_moves_anything() {
        let ctx = egui::Context::default();
        let root = temp_root("del-proj-latch");
        // Built with a working projects file, so the fixture has real members —
        // a library with no files in the project could not tell the two versions
        // apart.
        let mut app = app_with_a_project(&ctx, root.clone());
        assert_eq!(app.library.file_count("p-1"), 2);

        let broken = b"{ \"projects\": [ ".as_slice();
        std::fs::write(root.join(crate::library::project::PROJECTS_FILE), broken).unwrap();
        app.library.reload_projects();
        assert!(!app.library.may_write_projects());

        app.delete_project("p-1", None, false);

        assert_eq!(
            std::fs::read(root.join(crate::library::project::PROJECTS_FILE)).unwrap(),
            broken,
            "the bytes on disk must be untouched"
        );
        assert!(
            crate::library::store::trashed(&root).is_empty(),
            "and not one file may have been trashed"
        );
        app.library.refresh();
        assert_eq!(
            app.library.entries.len(),
            2,
            "both documents are still in the library"
        );
        assert!(
            app.library
                .entries
                .iter()
                .all(|e| e.meta.project.as_deref() == Some("p-1")),
            "still pointing at the project that was not deleted"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Saving the settings with *Move my existing files there* takes the whole
    /// library with it — and switching folders without it leaves them behind,
    /// which is the state the toggle exists to make a choice rather than a
    /// surprise.
    ///
    /// ⚠️ **Both halves, because they fail in opposite directions.** A migration
    /// that silently never runs loses nothing and looks like the library was
    /// wiped; a migration that runs when it was not asked for moves somebody's
    /// files out of a folder they were deliberately pointing away from.
    #[test]
    fn changing_the_base_folder_moves_the_library_only_when_asked() {
        let ctx = egui::Context::default();
        let old = temp_root("settings-old");
        let new = temp_root("settings-new");
        let mut app = app_with_a_project(&ctx, old.clone());
        assert_eq!(app.library.entries.len(), 2);

        // Without the toggle: the new folder is empty and the old one still has
        // everything.
        app.library_settings = Some(crate::panels::dashboard::LibrarySettings {
            base_folder: new.display().to_string(),
            migrate: false,
            ..crate::panels::dashboard::LibrarySettings::from_prefs(&app.prefs, &old)
        });
        app.apply_library_settings();
        assert_eq!(app.library.root, new);
        assert!(app.library.entries.is_empty(), "the new folder is empty");
        assert_eq!(crate::library::scan::scan(&old).len(), 2, "nothing moved");

        // And back again, this time asking for the move.
        app.library_settings = Some(crate::panels::dashboard::LibrarySettings {
            base_folder: old.display().to_string(),
            migrate: true,
            ..crate::panels::dashboard::LibrarySettings::from_prefs(&app.prefs, &new)
        });
        app.apply_library_settings();
        assert_eq!(app.library.root, old);
        assert_eq!(app.library.entries.len(), 2);
        let _ = std::fs::remove_dir_all(&old);
        let _ = std::fs::remove_dir_all(&new);
    }

    /// **`[S1.3-L5-01]`'s loss: *Move my library* used to fork the open
    /// document.**
    ///
    /// `session.path` is absolute, so a migration left it naming a file in the
    /// folder the library had just abandoned. The next save recreated the
    /// document *there* — while the new root already held the copy `relocate` had
    /// carried across, with the same `meta.id`. Two files, one identity: the
    /// library listed only the migrated copy, and every edit made after the move
    /// went to the one nothing lists.
    ///
    /// ⚠️ **The migration test beside this one has always been green and always
    /// will be**, which is why this is a separate test rather than two more
    /// assertions in it: `changing_the_base_folder_moves_the_library_only_when_asked`
    /// never opens a document, so there is no `session.path` for the move to
    /// leave behind. **Nothing about the migration itself is wrong here.** The
    /// bug is entirely in what the app is still pointing at afterwards, which is
    /// invisible to any fixture that only counts files.
    ///
    /// **Two flips, both run, and they fail at different assertions on purpose.**
    /// Deleting the re-point loop in `apply_library_settings` fails at *"the open
    /// document is in the library it now says it is in"*, naming the old root —
    /// the session's path, which is the loss. Replacing the map lookup with the
    /// obvious `strip_prefix(&old_root).join(&new_root)` fails at *"it is the
    /// file that moved, not the one that was already called that"*, with the two
    /// paths printed identical — a session pointed at a stranger's file. ⚠️ **The
    /// first flip leaves that second assertion green and the second flip leaves
    /// the first one green**, so neither assertion could have been dropped as
    /// redundant.
    #[test]
    fn moving_the_library_takes_the_open_document_with_it() {
        let ctx = egui::Context::default();
        let old = temp_root("settings-open-old");
        let new = temp_root("settings-open-new");
        let mut app = app_with_a_project(&ctx, old.clone());
        // Open one of them, which is the whole difference from the test above.
        let open = app.library.entries[0].path.clone();
        app.open_path(&open);
        app.prefs.last_document = Some(open.clone());
        assert_eq!(app.session.path.as_deref(), Some(open.as_path()));

        // ⚠️ **A document of the same name is already waiting at the
        // destination**, so `relocate` renames the migrated one on the way in.
        // That is what makes a *prefix swap* the wrong fix and a path map the
        // right one: re-joining `kestrel/landing-v4.ondin` under the new root
        // names this stranger, not the file the user is editing — and the
        // resulting session would be silently editing somebody else's document
        // with all three assertions below still green.
        let squatter = new.join("kestrel");
        std::fs::create_dir_all(&squatter).unwrap();
        let mut other = ondin_core::Document::new(app.session.ids.mint());
        crate::library::store::file_document(&new, None, "x", &mut other).unwrap();
        std::fs::rename(new.join("x.ondin"), squatter.join("landing-v4.ondin")).unwrap();
        let stranger = squatter.join("landing-v4.ondin");

        app.library_settings = Some(crate::panels::dashboard::LibrarySettings {
            base_folder: new.display().to_string(),
            migrate: true,
            ..crate::panels::dashboard::LibrarySettings::from_prefs(&app.prefs, &old)
        });
        app.apply_library_settings();

        let now = app.session.path.clone().expect("still editing something");
        assert!(
            now.starts_with(&new),
            "the open document is in the library it now says it is in: {now:?}"
        );
        assert_ne!(
            now, stranger,
            "and it is the file that moved, not the one that was already called that"
        );
        assert_eq!(
            app.prefs.last_document.as_deref(),
            Some(now.as_path()),
            "and so does the document the next launch reopens"
        );

        // The resurrection, which is what the stale path actually cost: save, and
        // count the files carrying this document's id.
        crate::library::store::write_document(&now, &app.session.doc).unwrap();
        assert!(
            crate::library::scan::scan(&old).is_empty(),
            "nothing was written back into the folder the library left: {:?}",
            crate::library::scan::scan(&old)
                .iter()
                .map(|e| e.path.clone())
                .collect::<Vec<_>>()
        );
        app.library.refresh();
        assert_eq!(
            app.library.entries.len(),
            3,
            "and the new library holds the two that moved plus the one already there"
        );
        let _ = std::fs::remove_dir_all(&old);
        let _ = std::fs::remove_dir_all(&new);
    }

    /// Pointing the library at a folder that does not exist **makes** it, which
    /// is what keeps that from being a one-way trip (§15 D384).
    ///
    /// ⚠️ **The second assertion is the one this test exists for.**
    /// `Library::root_unavailable` reads "could not be listed *and* this machine
    /// has documents recorded", and the per-machine index is keyed by document id
    /// rather than by folder — so a user who has opened anything carries the
    /// second half to whatever root they name next. Without the `create_dir_all`
    /// in `apply_library_settings`, the new library opens unavailable and refuses
    /// every write, including the one that would have created the folder: a dead
    /// end reachable by typing a path.
    ///
    /// Flip-check, run: removing that line fails at `may_write`, and leaves the
    /// `root` assertion above it green — the library does move, it simply cannot
    /// be used.
    ///
    /// 🚨 **That flip stopped biting for a range, and nothing said so** (§15
    /// D848, `[X1.1-L6-01]`). §15 D807's `cfg!(test)` gate makes `LocalIndex::load`
    /// answer an empty index, and `apply_library_settings` built the new library
    /// with `Library::open` — so the `mark_opened` below was set on a library that
    /// was then thrown away, `root_unavailable` was false by construction, and the
    /// assertion this test exists for passed for a reason unrelated to the line.
    /// The fixture visibly *tried*, which is what made it easy to believe. The
    /// index is handed across now (`Library::open_with_index`), and the flip was
    /// re-run: red at `may_write` again, the documented site.
    #[test]
    fn a_base_folder_that_does_not_exist_yet_is_created_rather_than_refused() {
        let ctx = egui::Context::default();
        let old = temp_root("settings-make-old");
        let new = temp_root("settings-make-new");
        // The fixture: a folder the user has named and that is not there, on a
        // machine that has seen documents before.
        std::fs::remove_dir_all(&new).unwrap();
        let mut app = app_with_a_project(&ctx, old.clone());
        app.library.local.mark_opened("doc-1");

        app.library_settings = Some(crate::panels::dashboard::LibrarySettings {
            base_folder: new.display().to_string(),
            migrate: false,
            ..crate::panels::dashboard::LibrarySettings::from_prefs(&app.prefs, &old)
        });
        app.apply_library_settings();

        assert_eq!(app.library.root, new);
        assert!(
            app.library.may_write(),
            "a library nothing can be created in is a library that cannot become one"
        );
        assert!(!app.library.root_unavailable);
        assert!(new.is_dir(), "and the folder is really there");
        let _ = std::fs::remove_dir_all(&old);
        let _ = std::fs::remove_dir_all(&new);
    }

    /// ⚠️ **Cancel throws the draft away** — the whole reason there is one, since
    /// this modal's two storage fields together are an operation rather than a
    /// setting.
    #[test]
    fn abandoning_the_settings_draft_changes_nothing() {
        let ctx = egui::Context::default();
        let root = temp_root("settings-cancel");
        let mut app = app_at(&ctx, root.clone());
        let before = app.prefs.autosave_secs;

        app.library_settings = Some(crate::panels::dashboard::LibrarySettings {
            autosave_secs: 999,
            ..crate::panels::dashboard::LibrarySettings::from_prefs(&app.prefs, &root)
        });
        // What Cancel and Escape both do.
        app.library_settings = None;
        assert_eq!(app.prefs.autosave_secs, before);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ⚠️ The draft's *base folder* is seeded from the resolved root, not the raw
    /// preference — they differ for every user who has never set one, and an
    /// empty field there would read as "no library".
    #[test]
    fn the_settings_draft_shows_where_the_library_actually_is() {
        let ctx = egui::Context::default();
        let root = temp_root("settings-seed");
        let mut app = app_at(&ctx, root.clone());
        app.prefs.base_folder = None;

        let draft =
            crate::panels::dashboard::LibrarySettings::from_prefs(&app.prefs, &app.library.root);
        assert_eq!(draft.base_folder, root.display().to_string());
        assert!(!draft.base_folder.is_empty());
        // And re-saving that same text must not be read as a folder change.
        app.library_settings = Some(draft);
        app.apply_library_settings();
        assert_eq!(app.library.root, root);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ⚠️ **The top bar names a document by its *typed* name, not its file.**
    ///
    /// This is the bug the library introduced and nearly shipped:
    /// `document_title` read `path.file_stem()`, which was right while a
    /// filename was whatever somebody typed into a save dialog and is wrong the
    /// moment it is a slug. The failure is quiet — the title bar reads
    /// `landing-v4` and nothing is broken — which is why it is pinned here
    /// rather than left to the eye.
    ///
    /// Flip-check, run: putting `document_title` back on the stem fails on the
    /// first assertion. The *third* is the one that keeps the fix honest — a
    /// document with no block still has to fall back to its filename, so a fix
    /// that simply returned `meta.name` would pass the first two and leave a
    /// pre-library document called "Untitled".
    #[test]
    fn the_title_is_the_typed_name_and_falls_back_to_the_filename() {
        let ctx = egui::Context::default();
        let root = temp_root("title");
        let mut app = app_at(&ctx, root.clone());

        let mut meta = app.session.doc.meta().clone();
        meta.name = Some("Landing v4".into());
        app.session.doc.set_meta(meta);
        app.save_file(false);
        assert_eq!(
            app.session.path.as_ref().unwrap().file_stem().unwrap(),
            "landing-v4"
        );
        assert_eq!(app.session.document_title(), "Landing v4");

        // The library and the title bar must agree about one document, which is
        // why they share `scan::display_name`.
        app.library.refresh();
        assert_eq!(
            app.library.entries[0].display_name(),
            app.session.document_title()
        );

        // A pre-library document has no block, so the filename is all there is.
        let bare = ondin_core::Document::new(app.session.ids.mint());
        let path = root.join("old-hero-sketch.ondin");
        std::fs::write(&path, ondin_core::io::save(&bare).unwrap()).unwrap();
        app.open_path(&path);
        assert_eq!(app.session.document_title(), "Old hero sketch");

        // And the starter document, which has neither.
        let fresh = app_at(&ctx, temp_root("title-fresh"));
        assert_eq!(fresh.session.document_title(), "Untitled");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every string the top bar painted this pass.
    ///
    /// ⚠️ **The crumb is three separate galleys, not one string**, because the
    /// project half has to be a `Response` — a `ui.label` reports no hover and no
    /// click, so it could not be a link. That is why this collects a set rather
    /// than looking for `"Kestrel / Landing v4"` anywhere.
    fn top_bar_text(ctx: &egui::Context, app: &mut OndinApp) -> Vec<String> {
        let out = ctx.run_ui(Default::default(), |ui| app.top_bar(ui));
        out.shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    /// The breadcrumb draws `Project / Document`, and just the document when
    /// there is no project.
    ///
    /// ⚠️ **The absence half is the one worth having.** A crumb that always drew
    /// its separator would show a leading `/` for every document in no project —
    /// a slash with nothing on the left of it — and that reads as a rendering
    /// bug rather than as a design.
    ///
    /// Flip-check, run: lifting the separator out of the `if let` fails on that
    /// assertion and prints the whole top bar, `["◆", "/", "Loose sketch", …]`,
    /// which is the symptom itself in the failure message rather than a count.
    #[test]
    fn the_breadcrumb_names_the_project_only_when_there_is_one() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("crumb");
        let mut app = app_with_a_project(&ctx, root.clone());

        // A document in a project.
        app.library.refresh();
        let filed = app
            .library
            .entries
            .iter()
            .find(|e| e.display_name() == "Landing v4")
            .unwrap()
            .path
            .clone();
        app.open_path(&filed);
        let painted = top_bar_text(&ctx, &mut app);
        assert!(painted.contains(&"Kestrel".to_string()), "{painted:?}");
        assert!(painted.contains(&"/".to_string()), "{painted:?}");
        assert!(painted.contains(&"Landing v4".to_string()), "{painted:?}");

        // And one in none.
        let mut loose = ondin_core::Document::new(app.session.ids.mint());
        let path =
            crate::library::store::file_document(&root, None, "Loose sketch", &mut loose).unwrap();
        app.library.refresh();
        app.open_path(&path);
        let painted = top_bar_text(&ctx, &mut app);
        assert!(painted.contains(&"Loose sketch".to_string()), "{painted:?}");
        assert!(
            !painted.contains(&"/".to_string()),
            "a document in no project must not draw a leading separator: {painted:?}"
        );
        assert!(!painted.contains(&"Kestrel".to_string()), "{painted:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Draw the dashboard for real and read what egui says about it.
    ///
    /// **The only check on this screen that is not about the filesystem**, and
    /// the reason it exists is that nothing else can look at it: launching the
    /// GUI is off the table, and `ondin export` covers the document rather than
    /// the chrome. What a headless `run_ui` *can* say is the two things most
    /// likely to be wrong in a panel built out of hand-painted rows —
    ///
    /// - **id clashes.** Every file row interacts on an id derived from its
    ///   path, every menu row on an index; two widgets sharing an id is the
    ///   classic egui bug and it is invisible until the wrong row responds.
    ///   `warn_on_id_clash` draws a red stroked rectangle into `.shapes`, so it
    ///   is assertable (§15 D65).
    /// - **panics.** A nested `Panel`, a `Modal` over a `CentralPanel`, or the
    ///   `Context`-accessor deadlock `CLAUDE.md` warns about would all take the
    ///   frame down rather than misdraw.
    ///
    /// ⚠️ **It says nothing about whether the screen looks right.** Layout,
    /// spacing and colour are still the maintainer's eye; this is the floor, not
    /// the ceiling.
    ///
    /// Flip-check, run: collapsing the file-menu id from `("file-menu", path)`
    /// to a bare `"file-menu"` — the id every one of these rows would share if
    /// somebody "simplified" it — makes egui draw **4** warning rectangles and
    /// fails here. It fails on the `All` pass rather than the first (`Recent`),
    /// because a machine that has opened nothing has an empty Recent and
    /// therefore no rows to clash: worth knowing, since it means this test's
    /// coverage depends on the fixture having *listed* documents and not merely
    /// having created them.
    #[test]
    fn the_dashboard_draws_without_egui_complaining() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        // 🚨 **Set, not assumed** (§15 D597). egui defaults `warn_on_id_clash` to
        // `cfg!(debug_assertions)` and `check_for_id_clash` returns before
        // painting when it is off — so in a release build the count below is zero
        // *by construction* and this test passed while checking nothing, over the
        // one screen whose row ids come from paths and indices. The flip in the
        // doc above producing **4** rectangles can only ever have been run in
        // debug. No gate runs the tests in release, which is why nothing said so.
        ctx.options_mut(|o| o.warn_on_id_clash = true);
        assert!(
            ctx.options(|o| o.warn_on_id_clash),
            "the clash warning is off, so this test cannot fail"
        );
        let root = temp_root("draw");
        let mut app = app_at(&ctx, root.clone());

        // A library with something in it, in both projects and neither: an empty
        // dashboard would not exercise a single row.
        app.library
            .projects
            .projects
            .push(crate::library::project::Project {
                id: "p-1".into(),
                name: "Kestrel".into(),
                color: crate::library::project::PROJECT_COLORS[0].into(),
                folder: Some("kestrel".into()),
                created: 0,
                archived: false,
            });
        let project = app.library.projects.projects[0].clone();
        for (name, p) in [
            ("Landing v4", Some(&project)),
            ("Pricing table", Some(&project)),
            ("Loose sketch", None),
        ] {
            let mut doc = ondin_core::Document::new(app.session.ids.mint());
            crate::library::store::file_document(&root, p, name, &mut doc).unwrap();
        }
        app.library.refresh();
        assert_eq!(app.library.entries.len(), 3, "the fixture must have rows");

        // Both views, and every sidebar entry — each is a different layout, and
        // the list view is the one with five columns of painted text.
        for nav in [
            crate::panels::dashboard::Nav::Recent,
            crate::panels::dashboard::Nav::All,
            crate::panels::dashboard::Nav::Starred,
            crate::panels::dashboard::Nav::Trash,
            crate::panels::dashboard::Nav::Project("p-1".into()),
        ] {
            for list_view in [false, true] {
                app.dash.nav = nav.clone();
                app.dash.list_view = list_view;
                let out = ctx.run_ui(Default::default(), |ui| app.dashboard_ui(ui));
                let complaints = out
                    .shapes
                    .iter()
                    .filter(|s| match &s.shape {
                        egui::Shape::Rect(r) => r.stroke.color == egui::Color32::RED,
                        _ => false,
                    })
                    .count();
                assert_eq!(
                    complaints, 0,
                    "egui drew {complaints} warning rectangle(s) on {nav:?} \
                     (list_view={list_view}) — an id clash or a rect that moved"
                );
                assert!(
                    !out.shapes.is_empty(),
                    "{nav:?} drew nothing at all, which is not an empty state — \
                     an empty state still paints a sidebar"
                );
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Going to the dashboard writes the work first, so the trip is never
    /// destructive — and comes back with a fresh scan rather than the list it
    /// had before the editor changed anything.
    #[test]
    fn leaving_the_editor_saves_and_rescans() {
        let ctx = egui::Context::default();
        let root = temp_root("leave");
        let mut app = app_at(&ctx, root.clone());
        app.save_file(false);
        app.session.mark_unsaved();

        app.go_to_dashboard();
        assert_eq!(app.view, View::Dashboard);
        assert!(
            !app.session.is_dirty(),
            "the work was written on the way out"
        );
        assert_eq!(app.library.entries.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ⚠️ **Walking to the library mid-sentence must not throw the sentence
    /// away** — the review's only Tier 2 Critical.
    ///
    /// A text session commits nothing until it ends (§9.3): `preview_session`
    /// only sets a preview and `finish_text_edit` is the sole committer. So
    /// `go_to_dashboard`'s save — which reads `session.doc` — used to write the
    /// document *without* what had just been typed, then drop the crash
    /// snapshot on the next line because that save had made the session clean.
    /// The session itself survived the walk and was discarded by
    /// `reset_transient_state` on the way back in, so nothing anywhere held the
    /// text and nothing said so.
    ///
    /// **The canvas's own click-away rule cannot cover this**: `text_mode_input`
    /// ends the session on the *canvas's* `Response`, and a click on a top-bar
    /// button is delivered to that button.
    ///
    /// Flip-check, run: removing the `finish_text_edit` guard from
    /// `go_to_dashboard` fails with `left: "hello"  right: "TYPED"` — the
    /// reported symptom in the failure message.
    ///
    /// ⚠️ **The predicted site was wrong the first time, and the correction is
    /// the useful half.** As first written the test asserted `app.text.is_none()`
    /// *before* re-reading the file, and the flip bit there instead: "the session
    /// ended on the way out", which reports a mechanism rather than a loss. Both
    /// assertions have teeth; the order decides which one a future reader is
    /// told about, so the file — the only thing that survives the walk — is
    /// asserted first.
    #[test]
    fn walking_to_the_library_commits_what_was_typed() {
        let ctx = egui::Context::default();
        let root = temp_root("text-walk");
        let mut app = app_at(&ctx, root.clone());

        let id = app.session.ids.mint();
        let parent = app.session.doc.root();
        let parts = crate::fonts::FontService::default_text_parts();
        app.session
            .try_commit(ondin_core::Transaction(vec![
                ondin_core::Operation::CreateNode {
                    id,
                    parent,
                    index: 0,
                    kind: ondin_core::NodeKind::Text {
                        content: "hello".into(),
                        style: Box::new(parts.style),
                        spans: parts.spans,
                        para_spans: parts.para_spans,
                        paragraph: parts.paragraph,
                        block: parts.block,
                        sizing: parts.sizing,
                        on_path: None,
                        on_path_flip: false,
                        on_path_offset: 0.0,
                    },
                    transform: None,
                    name: None,
                },
            ]))
            .expect("the fixture's text node");
        app.save_file(false);
        let path = app.session.path.clone().expect("the document is filed");

        // No world point, so `begin_edit_text` selects all — the insert then
        // replaces the buffer, which makes the assertion below unambiguous.
        app.begin_edit_text(Some(id), None);
        app.text
            .as_mut()
            .expect("the session is live")
            .editor
            .insert("TYPED");
        assert_eq!(
            app.text.as_ref().unwrap().editor.content(),
            "TYPED",
            "the fixture is in the state this test is about"
        );

        app.go_to_dashboard();
        assert_eq!(app.view, View::Dashboard);

        // ⚠️ **The file is asserted before the session, deliberately.** Both
        // assertions bite under the flip, but only this one is the reported
        // symptom; a suite that failed on `text.is_none()` first would report
        // "the session is still open", which is a mechanism rather than a loss.
        let doc = ondin_core::io::load(&std::fs::read(&path).unwrap())
            .expect("the document that was written on the way out");
        let text = doc
            .get(id)
            .and_then(|n| match n.kind() {
                ondin_core::NodeKind::Text { content, .. } => Some(content.clone()),
                _ => None,
            })
            .expect("the text node survived the save");
        assert_eq!(text, "TYPED", "the typed text reached the file");
        assert!(app.text.is_none(), "and the session ended on the way out");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A typed opacity digit lands on what it was typed against, 600 ms later**
    /// (§15 D470) — `[S16.1-L1-02]`.
    ///
    /// `OpacityEntry` held `first` and `at` and **no subject**, so
    /// `fold_opacity_entry` re-read the live selection at deadline time. Select A,
    /// press `4` — A previews at 40% — click B inside the window, and when the
    /// window shuts A is back at 1.0 and **B** is committed to 0.4, `undo_depth
    /// 0 → 1`, status *"Opacity 40%"*. The user typed a digit against A, watched A
    /// change, and B — which they merely selected — keeps the value. One undo
    /// restores B; A never gets what was asked for.
    ///
    /// ⚠️ **The rule was already written two hundred lines above the type, in the
    /// same struct.** `session_scrub`, the sibling *"value in flight that
    /// `RenderOverrides` cannot preview"*, says verbatim: *"the node is carried
    /// with the lists so a snapshot cannot be applied to a session that has since
    /// moved to a different layer."*
    ///
    /// ⚠️ **The empty-selection row was clean by accident and one level down**, at
    /// `set_opacity_pct_on`'s `if tx.0.is_empty() { return; }` — where
    /// `opacity_digit`, the *other* reader of the same entry, has an explicit
    /// guard. It is asserted here anyway, because "clean for a reason nobody
    /// wrote down" is how the moved case came to have no guard at all.
    ///
    /// **The clock is driven through `RawInput::time`**, which is why this is fast:
    /// `OPACITY_DIGIT_WINDOW` is read against `InputState::time`, not an `Instant`.
    ///
    /// ⚠️ **Flipped** by committing against `self.session.selection` in
    /// `fold_opacity_entry`: fails on the moved row with `(1.0, 0.4)` — A back at
    /// 1.0 and B holding the typed value, the finding's own numbers to the digit.
    /// The **control** — the selection staying on A — is green either way, and is
    /// what says the harness delivers the keystroke and reaches the deadline at
    /// all.
    #[test]
    fn a_typed_opacity_digit_lands_on_the_layer_it_was_typed_against() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("opacity-subject");

        /// Two rects at opacity 1.0, A selected. Returns their alphas after
        /// pressing `4` at t=0.1 and letting the window shut at t=1.0, with
        /// `moved` deciding what the selection is when the deadline arrives.
        fn run(
            ctx: &egui::Context,
            root: &std::path::Path,
            moved: fn(&mut OndinApp, ondin_core::NodeId, ondin_core::NodeId),
        ) -> (f32, f32, usize) {
            let mut app = app_at(ctx, root.to_path_buf());
            let doc_root = app.session.doc.root();
            let (a, b) = (app.session.ids.mint(), app.session.ids.mint());
            let rect = |id| ondin_core::Operation::CreateNode {
                id,
                parent: doc_root,
                index: 0,
                kind: ondin_core::NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(20.0, 20.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            };
            app.session
                .try_commit(ondin_core::Transaction(vec![rect(a), rect(b)]))
                .expect("two rects");
            app.session.selection.set_one(a);
            // The fixture's own creation is an undo step, so what is asserted is
            // the **delta** — the finding's `0 → 1` measured from a document that
            // already had its rects.
            let before = app.session.history.undo_depth();

            let mut frame = eframe::Frame::_new_kittest();
            let mut pass = |app: &mut OndinApp, time: f64, events: Vec<egui::Event>| {
                let input = egui::RawInput {
                    time: Some(time),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::pos2(0.0, 0.0),
                        egui::vec2(1320.0, 820.0),
                    )),
                    events,
                    ..Default::default()
                };
                let _ = ctx.run_ui(input, |ui| {
                    eframe::App::ui(app, ui, &mut frame);
                });
            };
            pass(&mut app, 0.0, Vec::new());
            pass(
                &mut app,
                0.1,
                vec![egui::Event::Key {
                    key: egui::Key::Num4,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
            );
            moved(&mut app, a, b);
            // Well past the 0.6s window, so the fold runs.
            pass(&mut app, 1.0, Vec::new());

            let alpha = |app: &OndinApp, id| app.session.doc.get(id).expect("node").opacity();
            let out = (
                alpha(&app, a),
                alpha(&app, b),
                app.session.history.undo_depth() - before,
            );
            let _ = std::fs::remove_dir_all(root);
            out
        }

        // **The control**: nothing moves, and the digit lands where it was typed.
        assert_eq!(
            run(&ctx, &root, |_, _, _| {}),
            (0.4, 1.0, 1),
            "control: with the selection still on A, the digit commits to A"
        );

        // The finding: the selection moves inside the window.
        assert_eq!(
            run(&ctx, &root, |app, _, b| app.session.selection.set_one(b)),
            (0.4, 1.0, 1),
            "a digit typed against A must land on A however the selection moves"
        );

        // And the empty case, clean for a reason nobody had written down.
        assert_eq!(
            run(&ctx, &root, |app, _, _| app.session.selection.clear()),
            (0.4, 1.0, 1),
            "clearing the selection must not take the typed value with it either"
        );
    }

    /// **The top bar's other three buttons are the same walk, and they went the
    /// same way** — `[S16.4-L1-02]`, §15 D466, the sibling of the Critical above.
    ///
    /// `input::text_insert_mode` builds its action vector from `Action::TextStyle`
    /// alone — `Undo`, `Redo`, `Save` and `Open` live only in `normal_mode` — and
    /// `top_bar` consults `self.mode` nowhere. So with a live session `Ctrl+Z` did
    /// nothing and the Undo glyph 30 px away rewound the document beneath an
    /// editor whose buffer was unchanged, and *Save* wrote **and pinned a
    /// version** from a `session.doc` that did not hold the typed text.
    ///
    /// ⚠️ **Asserted at the seam rather than by clicking the glyph**, and that is
    /// a real limitation of this test worth stating. What is checked is that each
    /// of the three verbs ends the session before touching the document; what is
    /// *not* checked is that the buttons call them — `top_bar` is 200 lines of
    /// layout and a click into it is a coordinate sweep, which
    /// `mixed_opacity_tests` shows is its own hazard. The three call sites are
    /// one line each and were read.
    ///
    /// ⚠️ **Finishing rather than dimming is a choice**, and it is the one this
    /// file had already made twice — `choose_tool` for the tool rail's equally
    /// dead bare keys, and `go_to_dashboard` for the Critical above. It leaves the
    /// button and its chord *still disagreeing*, which the finding is right to
    /// call a gap: nothing in the design says whether `Ctrl+Z` should be dead
    /// mid-session at all. The damage is what this closes.
    ///
    /// **Flip-check, run — and the symptom is worse than the finding described.**
    /// Removing the `finish_text_first()` call from the undo arm does not rewind
    /// the text to `"hello"`: with nothing committed, the first undo step still on
    /// the stack is the **`CreateNode`**, so the panic is *"the fixture's node is
    /// a text node, got None"*. **Clicking Undo mid-session deletes the node you
    /// are typing into**, and the editor stays live over a node the document no
    /// longer has. `[S16.4-L1-02]` measured `undo_depth 1 → 0` and read the
    /// consequence as *"rewind the document beneath an editor whose buffer is
    /// unchanged"*; on this fixture the thing rewound away is the subject itself.
    /// The save half fails on its own line with `"hello"` against `"TYPED"`.
    #[test]
    fn the_top_bars_undo_and_save_commit_what_was_typed_first() {
        let ctx = egui::Context::default();
        let root = temp_root("text-topbar");

        /// A filed document with one text node reading "hello", a live session on
        /// it holding "TYPED", and one prior undo step to rewind into.
        fn typed(ctx: &egui::Context, root: &std::path::Path) -> (OndinApp, ondin_core::NodeId) {
            let mut app = app_at(ctx, root.to_path_buf());
            let id = app.session.ids.mint();
            let parent = app.session.doc.root();
            let parts = crate::fonts::FontService::default_text_parts();
            app.session
                .try_commit(ondin_core::Transaction(vec![
                    ondin_core::Operation::CreateNode {
                        id,
                        parent,
                        index: 0,
                        kind: ondin_core::NodeKind::Text {
                            content: "hello".into(),
                            style: Box::new(parts.style),
                            spans: parts.spans,
                            para_spans: parts.para_spans,
                            paragraph: parts.paragraph,
                            block: parts.block,
                            sizing: parts.sizing,
                            on_path: None,
                            on_path_flip: false,
                            on_path_offset: 0.0,
                        },
                        transform: None,
                        name: None,
                    },
                ]))
                .expect("the fixture's text node");
            app.save_file(false);
            app.begin_edit_text(Some(id), None);
            app.text
                .as_mut()
                .expect("the session is live")
                .editor
                .insert("TYPED");
            assert_eq!(
                app.text.as_ref().unwrap().editor.content(),
                "TYPED",
                "fixture: the session holds text the document does not"
            );
            (app, id)
        }

        let content = |app: &OndinApp, id| match app.session.doc.get(id).map(|n| n.kind()) {
            Some(ondin_core::NodeKind::Text { content, .. }) => content.clone(),
            other => panic!("the fixture's node is a text node, got {other:?}"),
        };

        // **Undo.** The button's verb, run as the button runs it.
        let (mut app, id) = typed(&ctx, &root);
        app.finish_text_first();
        app.session.undo();
        assert_eq!(
            content(&app, id),
            "hello",
            "the first undo undoes the typing, not the node that carried it — \
             which is only true if the session was committed first"
        );
        assert!(app.text.is_none(), "and the session is over");

        // **Save.** The one whose consequence is on disk: a pinned version of a
        // document missing what is on screen.
        let (mut app, id) = typed(&ctx, &root);
        app.finish_text_first();
        app.save_file(true);
        let path = app.session.path.clone().expect("the document is filed");
        let doc = ondin_core::io::load(&std::fs::read(&path).unwrap()).expect("reload");
        let saved = match doc.get(id).map(|n| n.kind()) {
            Some(ondin_core::NodeKind::Text { content, .. }) => content.clone(),
            other => panic!("the saved node is a text node, got {other:?}"),
        };
        assert_eq!(
            saved, "TYPED",
            "the pinned version holds what was on screen"
        );
        let _ = content(&app, id);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **And the *chord* now takes the same path, because it resolves at all**
    /// (§15 D815).
    ///
    /// The test above runs the three verbs as the **button** runs them, and says
    /// in its own doc that this left *"the button and its chord still
    /// disagreeing"* — `Ctrl+Z` mid-session did nothing, by omission rather than
    /// by a decision. The four chords resolve in `Mode::TextInsert` now
    /// (`input::text_insert_tests::the_four_document_chords_resolve_in_a_session`
    /// is that half), and the finishing moved to `OndinApp::dispatch` so that one
    /// seam serves both doors.
    ///
    /// ⚠️ **Driven through `dispatch`, not through the keymap**, which is the
    /// seam this half is about: `resolve` answering `[Undo]` and `dispatch`
    /// finishing the session first are two independent claims, and a test that
    /// pressed a key would pass or fail for either reason.
    ///
    /// **Flip-check, run** by removing `finish_text_first()` from `dispatch`'s
    /// `Undo` arm: fails at ***"the chord ended the session"***, the first
    /// assertion. ⚠️ **The prediction was the sibling test's failure site — a
    /// panic at *"the fixture's node is a text node, got None"*, the undo having
    /// deleted the node being typed into — and it never runs**, because this test
    /// asks about the *session* one line earlier and the sibling does not ask at
    /// all. Both are the same defect; which assertion reports it is decided by
    /// what each test happens to check first. *The loss is still the node, and
    /// only the sibling's message says so.*
    #[test]
    fn a_document_chord_in_a_session_finishes_it_first() {
        let ctx = egui::Context::default();
        let root = temp_root("text-chord");
        let mut app = app_at(&ctx, root.clone());
        let id = app.session.ids.mint();
        let parent = app.session.doc.root();
        let parts = crate::fonts::FontService::default_text_parts();
        app.session
            .try_commit(ondin_core::Transaction(vec![
                ondin_core::Operation::CreateNode {
                    id,
                    parent,
                    index: 0,
                    kind: ondin_core::NodeKind::Text {
                        content: "hello".into(),
                        style: Box::new(parts.style),
                        spans: parts.spans,
                        para_spans: parts.para_spans,
                        paragraph: parts.paragraph,
                        block: parts.block,
                        sizing: parts.sizing,
                        on_path: None,
                        on_path_flip: false,
                        on_path_offset: 0.0,
                    },
                    transform: None,
                    name: None,
                },
            ]))
            .expect("the fixture's text node");
        app.save_file(false);
        app.begin_edit_text(Some(id), None);
        app.text
            .as_mut()
            .expect("the session is live")
            .editor
            .insert("TYPED");
        assert_eq!(
            app.mode,
            super::Mode::TextInsert,
            "the fixture must reach the state: a live session is what this is about"
        );

        app.dispatch(&ctx, crate::input::Action::Undo);

        assert!(app.text.is_none(), "the chord ended the session");
        let content = match app.session.doc.get(id).map(|n| n.kind()) {
            Some(ondin_core::NodeKind::Text { content, .. }) => content.clone(),
            other => panic!("the fixture's node is a text node, got {other:?}"),
        };
        assert_eq!(
            content, "hello",
            "and undid the typing rather than the node that carried it"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **`Escape` out of a chrome field is spent on the field, not on the
    /// selection** (§15 D821).
    ///
    /// 🚨 **It was spent twice.** egui clears focus in `Memory::begin_pass`, so
    /// the press is still in `input` when `input::resolve` runs and the key
    /// resolves to `Action::Escape` — measured directly: with a `TextEdit`
    /// focused, `resolve` answers `[Escape]`. That is what makes a *valved*
    /// numeric field's cancel reach `cancel_gesture`; a plain text field installs
    /// no preview, so the ladder fell through to its last rung and cleared the
    /// selection under the panel that was showing it.
    ///
    /// ⚠️ **The assertion is on the selection, not on the field**, because the
    /// field's own half is `ui::defocus_commits` (§15 D808) and is asserted where
    /// it is decided. What is new here is only that the ladder stops.
    ///
    /// ⚠️ **`chrome_focus` is set by hand rather than by focusing a widget.** The
    /// flag is written at the *end* of `update`, and this test does not run a full
    /// frame — driving one would make the test about `update`'s ordering, which is
    /// a second claim with its own failure modes. The ordering has a comment of
    /// its own at the write site; what this pins is the rung.
    ///
    /// **Flip-check, run** by deleting the `else if self.chrome_focus` rung: fails
    /// at *"the selection survives"*, the predicted site, with an empty selection
    /// — which is the reported symptom exactly. The control below fails instead if
    /// the rung is made unconditional, which is the other way it could be wrong.
    #[test]
    fn escape_out_of_a_chrome_field_does_not_also_clear_the_selection() {
        let ctx = egui::Context::default();
        let root = temp_root("escape-chrome");
        let mut app = app_at(&ctx, root.clone());
        let id = app.session.ids.mint();
        let parent = app.session.doc.root();
        app.session
            .try_commit(ondin_core::Transaction(vec![
                ondin_core::Operation::CreateNode {
                    id,
                    parent,
                    index: 0,
                    kind: ondin_core::NodeKind::Rect {
                        size: ondin_core::kurbo::Size::new(80.0, 40.0),
                        corner_radii: Default::default(),
                    },
                    transform: None,
                    name: None,
                },
            ]))
            .expect("the fixture's rect");
        app.session.selection.set_one(id);

        // A chrome field had the keyboard when the key arrived.
        app.chrome_focus = true;
        app.escape(&ctx);
        assert_eq!(
            app.session.selection.ids(),
            &[id],
            "the selection survives: the key was spent on the field"
        );

        // **Control: with nothing focused the ladder runs as it always has.**
        // Without this the rung could be unconditional and nothing would notice.
        app.chrome_focus = false;
        app.escape(&ctx);
        assert!(
            app.session.selection.ids().is_empty(),
            "control: outside a field, Escape still clears the selection"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every close request cancels while the work is unsaved, not only the
    /// first.
    ///
    /// ⚠️ **The second ✕ is the ordinary reflex, and it used to close the
    /// window.** The title bar is not covered by the in-app modal, so a user who
    /// double-clicks ✕ sends two requests; the guard's `&& !self.confirming_close`
    /// term skipped the whole arm on the second, and `CancelClose` has exactly
    /// one emitter in the workspace, so nothing cancelled and eframe took the
    /// window down with the card unanswered.
    ///
    /// Flip-check, run: putting the `&& !self.confirming_close` term back fails
    /// on the second frame's assertion — the predicted site — while the clean
    /// control below stays green, which is what proves the term was the whole
    /// cause. The control is also what proves the probe can tell `true` from
    /// `false` at all: without it, a test that never sees a `CancelClose` would
    /// pass for the wrong reason.
    #[test]
    fn a_second_close_request_cancels_too() {
        let ctx = egui::Context::default();
        let root = temp_root("close-twice");
        let mut app = app_at(&ctx, root.clone());

        /// One frame in which the OS asked the window to close.
        fn close_request(ctx: &egui::Context, app: &mut OndinApp) -> bool {
            let mut input = egui::RawInput::default();
            input
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default()
                .events = vec![egui::ViewportEvent::Close];
            let out = ctx.run_ui(input, |ui| app.handle_close_request(ui.ctx()));
            out.viewport_output.values().any(|v| {
                v.commands
                    .iter()
                    .any(|c| matches!(c, egui::ViewportCommand::CancelClose))
            })
        }

        app.save_file(false);
        app.session.mark_unsaved();

        assert!(close_request(&ctx, &mut app), "the first ✕ cancels");
        assert!(app.confirming_close, "and raises the card");
        assert!(
            close_request(&ctx, &mut app),
            "and so does the second, taken while the card is still up"
        );

        // Control: a clean session must *not* cancel, or the assertions above
        // would pass for a guard that cancelled unconditionally.
        app.save_file(false);
        assert!(!app.session.is_dirty());
        assert!(
            !close_request(&ctx, &mut app),
            "a saved document closes without a question"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// One frame of the whole app, with `events` delivered to it.
    pub(super) fn whole_frame(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1320.0, 820.0),
            )),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            eframe::App::ui(app, ui, &mut frame);
        });
    }

    /// **A modal owns the pointer too, and a dropped file is the pointer**
    /// (§15 D759).
    ///
    /// 🚨 **This is §15 D464's other half, and nobody had asked it.** D464 found
    /// that `Delete` and `Ctrl+A` reached the document behind a modal backdrop and
    /// guarded the keyboard. `raw.dropped_files` is filled from winit **before any
    /// widget runs**, so the `egui::Modal` backdrop that blocks every click does
    /// not touch a drop — and `take_dropped_images` sat outside the guard chain,
    /// committing ops to the document behind the card.
    ///
    /// ⚠️ **The control is the whole test.** An assertion that a drop behind a
    /// modal changes nothing is satisfied by a drop that never works at all — by a
    /// PNG the loader rejects, a fixture with no document, or a `RawInput` field
    /// egui ignores. The first case asserts the drop **lands**, on the same app,
    /// with the same bytes, one frame earlier.
    ///
    /// ⚠️ **All three modals, not just the one that motivated it.** The recovery
    /// card is the dangerous one (D464's reason: it is drawn above the view branch,
    /// so it can be up over a *different* document than the one it is asking
    /// about, and a drop there also mints a snapshot key — §15 D681's route). But
    /// the guard is one predicate, so a test naming one card would be green for a
    /// version that guarded only that card.
    ///
    /// **Flip, run:** restoring the bare `self.take_dropped_images(&ctx)` fails at
    /// the first card asserted — `settings` — with one child added. Predicted
    /// correctly.
    #[test]
    fn a_file_dropped_on_a_modal_does_not_reach_the_document_behind_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root_dir = temp_root("drop-behind-modal");
        let mut app = app_at(&ctx, root_dir.clone());

        let mut png = Vec::new();
        image::write_buffer_with_format(
            &mut std::io::Cursor::new(&mut png),
            &[255u8, 0, 0, 255],
            1,
            1,
            image::ExtendedColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .expect("a 1×1 red PNG");

        let drop_a_file = |ctx: &egui::Context, app: &mut OndinApp| {
            let mut frame = eframe::Frame::_new_kittest();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(1320.0, 820.0),
                )),
                dropped_files: vec![egui::DroppedFile {
                    name: "drop.png".to_string(),
                    bytes: Some(png.clone().into()),
                    ..Default::default()
                }],
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                eframe::App::ui(app, ui, &mut frame);
            });
        };
        // **The revision, not a child count.** The defect is *"ops reach the
        // document behind the backdrop"*, and the revision is that question
        // exactly — where a child count also has to be right about **which**
        // parent a dropped picture lands under, which `drop_images_at` decides
        // four ways (replace a fill, place at the point, place at the viewport
        // centre, cascade). A first draft counted the root's children and failed
        // its own control for that reason.
        let kids = |app: &OndinApp| app.session.revision();

        // The control: with nothing up, a drop lands. Without this the rest of the
        // test is about a drop that never worked.
        let before = kids(&app);
        drop_a_file(&ctx, &mut app);
        assert_ne!(
            kids(&app),
            before,
            "the control never landed, so this test asserts nothing about modals"
        );

        // And behind each of the three cards, it does not.
        for (name, arm) in [
            ("settings", 0usize),
            ("confirming_close", 1),
            ("recovery.pending", 2),
        ] {
            let mut app = app_at(&ctx, root_dir.clone());
            let settled = kids(&app);
            match arm {
                // Through the real door rather than by assigning the field:
                // `Settings::from_prefs` is private, and widening it for a test
                // would be the test changing the code's shape to suit itself.
                0 => app.toggle_settings(&ctx),
                1 => app.confirming_close = true,
                _ => app
                    .recovery
                    .pending
                    .push(crate::library::recovery::Pending {
                        key: "k".into(),
                        path: root_dir.join("k.ondin"),
                        name: "k".into(),
                        target: None,
                        written: Some(0),
                    }),
            }
            assert!(app.modal_is_up(), "{name} did not raise a modal");
            drop_a_file(&ctx, &mut app);
            assert_eq!(
                kids(&app),
                settled,
                "a file dropped behind {name} reached the document — this is §15 \
                 D464's keyboard hole, for the pointer"
            );
        }
        let _ = std::fs::remove_dir_all(&root_dir);
    }

    fn escape() -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }
    }

    /// **`Escape` answers the *Unsaved changes* card, and answers it *Cancel***
    /// (§15 D525).
    ///
    /// The card read only its three buttons and dropped the `ModalResponse`, so
    /// `Escape` — the universal way out of a dialog — was simply unhandled: the
    /// card stayed up, and the key went on to walk the editor's escape ladder
    /// underneath it. Measured before the fix at `tool` `Rect → Select` with
    /// `confirming_close` still `true`, and held down it kept paying out rungs.
    /// Both sibling modals already did this; Settings maps `should_close()` to
    /// *Cancel* and the recovery card maps it to *Later*, deliberately and with
    /// a test named for it.
    ///
    /// ⚠️ **The tool assertion is a regression guard rather than this fix's
    /// doing, and saying so is the point.** §15 D464 had already stopped the key
    /// reaching `input::resolve` while any of the three modals is up — so the
    /// ladder half of the finding was closed before this was written, and what
    /// was left was a card that would not go away. Flipped by dropping the
    /// `should_close()` arm: red at `confirming_close`, green at the tool.
    #[test]
    fn escape_over_the_close_card_cancels_it_and_leaves_the_tool_alone() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("close-escape");
        let mut app = app_at(&ctx, root.clone());
        app.save_file(false);
        app.session.mark_unsaved();
        app.confirming_close = true;
        app.tool = crate::tools::Tool::Rect;

        // One pass to put the card on screen, then the key. A modal's
        // `should_close` is about an `Area` that has to exist first, and a probe
        // that delivered the key on the frame the card was born measured a card
        // that ignored it — which reads exactly like the defect.
        whole_frame(&ctx, &mut app, Vec::new());
        whole_frame(&ctx, &mut app, vec![escape()]);

        assert!(!app.confirming_close, "the card took the key as *Cancel*");
        assert_eq!(
            app.tool,
            crate::tools::Tool::Rect,
            "and the key did not also pay out a rung underneath it"
        );
        assert!(
            app.session.is_dirty(),
            "*Cancel*, not *Discard*: nothing was thrown away"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The ✕ is intercepted on the library screen too** (§15 D526).
    ///
    /// `handle_close_request` — *"intercept the window close so unsaved work is
    /// never lost silently"* — sat below `ui`'s `View::Dashboard` return, so on
    /// the library screen it never ran, whatever the session held.
    ///
    /// ⚠️ Flipped by putting both calls back below the return: red at
    /// `confirming_close`, which stays `false`, and the `CancelClose` assertion
    /// below it is never reached.
    #[test]
    fn the_close_card_is_reachable_from_the_library() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("close-dashboard");
        let mut app = app_at(&ctx, root.clone());
        app.save_file(false);
        app.session.mark_unsaved();
        app.view = View::Dashboard;

        let mut frame = eframe::Frame::_new_kittest();
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1320.0, 820.0),
            )),
            ..Default::default()
        };
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .events = vec![egui::ViewportEvent::Close];
        let out = ctx.run_ui(input, |ui| {
            eframe::App::ui(&mut app, ui, &mut frame);
        });

        assert!(
            app.confirming_close,
            "the ✕ raised the card from behind the library"
        );
        assert!(
            out.viewport_output.values().any(|v| v
                .commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::CancelClose))),
            "and cancelled the close while it asks"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **One `Escape` over a top-bar dropdown closes the dropdown and nothing
    /// else** (§15 D527).
    ///
    /// The context menu has had this rule since it was written — *"the press
    /// that closes a menu closes **only** the menu. `escape` is a ladder that
    /// pays out one rung per press, and a version that closed the menu and then
    /// fell through would deselect, or leave the group, or drop back to Select,
    /// in the same keystroke that dismissed a menu opened by accident — eating
    /// the very state the menu was aimed at"* — and `TopMenu` had no such arm.
    /// Measured: with the Rect tool armed and the Zoom menu open, one `Escape`
    /// closed the menu **and** dropped the tool to Select.
    ///
    /// ⚠️ `input::resolve`'s `egui_wants_keyboard_input` guard does not stand in
    /// for it, which is measured rather than reasoned: opening a dropdown by
    /// clicking its head leaves `focused() == None`, so egui does not think
    /// anything wants the keyboard.
    ///
    /// ⚠️ Flipped by removing the `Action::Escape if open_menu != None` arm: red
    /// at the **tool**, which is the second assertion — the menu closes either
    /// way, because `dropdown` reads the key itself as well. So the assertion
    /// that names the finding is the one about the state that was *eaten*, not
    /// the one about the menu.
    #[test]
    fn escape_over_a_top_bar_dropdown_closes_only_the_dropdown() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("menu-escape");
        let mut app = app_at(&ctx, root.clone());
        app.tool = crate::tools::Tool::Rect;
        app.open_menu = TopMenu::Zoom;

        whole_frame(&ctx, &mut app, Vec::new());
        whole_frame(&ctx, &mut app, vec![escape()]);

        assert_eq!(app.open_menu, TopMenu::None, "the menu closed");
        assert_eq!(
            app.tool,
            crate::tools::Tool::Rect,
            "and the same press did not also pay out a rung of the ladder"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A context menu outranks present mode on Escape** (§15 D756).
    ///
    /// 🚨 **This is the case the design said could not happen.** `architecture.md`
    /// §9.4 read *"`self.present` stays first inside `escape` regardless:
    /// `open_context_menu` refuses in present mode, so the two can never
    /// contend"*, and `context-menus.md` §0 R3 carried the same sentence. D756
    /// removed that refusal, so they contend now — and the order that falls out is
    /// the right one, at no code cost: R3 answers Escape off `context_menu`
    /// at the top of `update` and **returns**, so `escape` is never entered and
    /// `self.present` being its first arm is simply not reached.
    ///
    /// ⚠️ **The order matters and the alternative is bad, which is why this is
    /// asserted rather than left to fall out.** Had present won, the first Escape
    /// would have restored the whole chrome *underneath a menu that stayed open* —
    /// the user pressing Escape at a menu, and getting the top bar back instead.
    /// The menu is the only thing on screen they can see to dismiss.
    ///
    /// ⚠️ **Both presses are driven, because a ladder is about what the *second*
    /// one does.** A test that stopped after the first would be green for a
    /// version that swallowed every Escape while a menu had ever been open.
    ///
    /// **Flip, run:** moving the `context_menu` arm below the `input::resolve`
    /// call fails at *"the first Escape closed the menu"* — predicted site — with
    /// `present` already `false`, which is the two rungs paying out together and
    /// is the exact shape R3's `return` exists to prevent.
    #[test]
    fn escape_in_present_mode_closes_the_menu_first_and_the_mode_second() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("present-menu-escape");
        let mut app = app_at(&ctx, root.clone());
        app.present = true;
        app.open_context_menu(&ctx, crate::menu::Target::Canvas, None);
        assert!(
            app.context_menu.is_some(),
            "the fixture is not in the state this test is about — D756's deletion \
             is what lets a menu exist here at all"
        );

        whole_frame(&ctx, &mut app, Vec::new());
        whole_frame(&ctx, &mut app, vec![escape()]);
        assert!(
            app.context_menu.is_none(),
            "the first Escape closed the menu"
        );
        assert!(
            app.present,
            "and left present mode alone: one rung per press, and the menu is the \
             rung the user can see"
        );

        whole_frame(&ctx, &mut app, vec![escape()]);
        assert!(
            !app.present,
            "the second Escape leaves the mode, which is the rung underneath"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **Walking to the library takes the close card with it** (§15 D526).
    ///
    /// `confirming_close` is written `true` in one place and was cleared in
    /// three — the card's own arms — so a keyboard door out of the editor left
    /// it stuck: `true` on the dashboard, where the card is not drawn, and
    /// `true` again on coming back with `is_dirty()` false, so the card returned
    /// over a document that had no unsaved changes.
    ///
    /// ⚠️ Flipped by removing the one line: red here. It is the only assertion
    /// in the workspace that can see it — nothing else reads the flag outside
    /// the card.
    #[test]
    fn walking_to_the_library_clears_the_close_card() {
        let ctx = egui::Context::default();
        let root = temp_root("close-walk");
        let mut app = app_at(&ctx, root.clone());
        app.save_file(false);
        app.confirming_close = true;

        app.go_to_dashboard();

        assert!(!app.confirming_close, "the question left with the editor");
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod context_menu_rule_tests {
    //! **The four rules `context-menus.md` §10 called "writable now and
    //! unwritten"** (`[A7-L8-06]`, §15 D795).
    //!
    //! R1's spent click, R4's replacement, R3's single rung, and "no document
    //! action fires while a menu is open". ⚠️ **R1's *spent click* was claimed by
    //! this list for a range and covered by nothing** — its first test never put a
    //! gesture in flight — until
    //! `a_right_click_that_cancels_a_gesture_is_spent_and_the_next_one_opens`
    //! (§15 D849, `[X6-L6-01]`). They are here rather than in `menu.rs`
    //! because every one of them is about a rule that only exists **between**
    //! frames or **between** subsystems: which of a press and a release opens the
    //! menu, whether the dismissal beats the open, and whether `input::resolve`
    //! runs at all. A `menu::Context` built by hand cannot be wrong about any of
    //! those, which is why thirty-odd green tests in `menu.rs` left all four open.
    //!
    //! ⚠️ **Two helpers are borrowed from sibling test modules rather than
    //! copied** — `whole_frame` and `app_with_two_groups`, each widened to
    //! `pub(super)` for this. A second copy of `whole_frame` would be a second
    //! statement of the warm-up rule its doc comment carries, and the rule is the
    //! part worth having once.
    use super::OndinApp;
    use super::library_wiring_tests::whole_frame;
    use super::ungroup_tests::app_with_two_groups;
    use crate::tools::Tool;
    use ondin_core::{Document, NodeId};

    /// A secondary press, as two events, so a test can look between them.
    ///
    /// **The gap is the assertion in R1's test** — `whole_frame` per event is
    /// what makes "the press has not opened it yet" a thing that can be read.
    fn secondary(at: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// A key press with no modifiers, for the two rules that are about what a
    /// keystroke does *not* reach.
    fn key(k: egui::Key) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }
    }

    /// Right-click the canvas at `at`, one event per frame, and leave the menu up.
    ///
    /// ⚠️ **The warm-up frame is not padding.** egui resolves a press against the
    /// widget rects it knew *last* frame, so a press on the frame a widget first
    /// appears is assigned to nothing and the whole test passes while asserting
    /// nothing — the same trap `canvas.rs`'s autopan probe records.
    fn right_click(ctx: &egui::Context, app: &mut OndinApp, at: egui::Pos2) {
        whole_frame(ctx, app, Vec::new());
        whole_frame(ctx, app, vec![egui::Event::PointerMoved(at)]);
        whole_frame(ctx, app, vec![secondary(at, true)]);
        whole_frame(ctx, app, vec![secondary(at, false)]);
    }

    /// **R1 — the release opens the menu, and the press that cancelled spends the
    /// click** (`context-menus.md` §1, `[A7-L8-06]`, §15 D795).
    ///
    /// The first of the four rules that `context-menus.md` §10 called *"writable
    /// now and unwritten"*: this is the first test anywhere in the workspace that
    /// drives a synthetic **secondary** button through the whole app. Everything
    /// before it either called `open_context_menu` by hand or built a
    /// `menu::Context` directly, so **no test had ever been through
    /// `canvas_context_menu`'s door at all**.
    ///
    /// What it pins is the *ordering*, which is the half a unit test cannot see:
    /// after the press frame the menu must still be absent, and after the release
    /// frame it must be there. §10 names the failure exactly — a menu that opens
    /// on the press "then has the release land inside it and activate the row
    /// under the pointer", which is a right-click that silently runs a verb.
    ///
    /// ⚠️ **Flipped**, by giving `canvas_context_menu` the press rather than the
    /// click (`ctx.input(|i| i.pointer.button_pressed(Secondary))` in place of
    /// `resp.secondary_clicked()`, which is the plausible wrong spelling rather
    /// than a deletion): **red on the press assertion**, the predicted site, with
    /// the menu already up a frame early. The release assertion stays green under
    /// that flip, which is the point — a test that only asked "is there a menu at
    /// the end" would have passed against the bug §10 describes.
    #[test]
    fn a_right_click_opens_its_menu_on_the_release_and_not_on_the_press() {
        let ctx = egui::Context::default();
        let (mut app, _, _, _) = app_with_two_groups(&ctx);
        let at = egui::pos2(660.0, 410.0);

        whole_frame(&ctx, &mut app, Vec::new());
        whole_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        assert!(
            app.context_menu.is_none(),
            "the fixture is not in the state this test is about"
        );

        whole_frame(&ctx, &mut app, vec![secondary(at, true)]);
        assert!(
            app.context_menu.is_none(),
            "R1: the press cancels, it does not open — a menu here is one the \
             release would land inside of"
        );

        whole_frame(&ctx, &mut app, vec![secondary(at, false)]);
        let menu = app.context_menu.as_ref().expect("R1: the release opens it");
        assert_eq!(
            menu.at, at,
            "and it opens at the press position, which is why `secondary_press` \
             is recorded on the way past"
        );
    }

    /// **R1's other half — a right-click that cancels a gesture is spent: it opens
    /// no menu *and selects nothing*, and the next one opens a menu** (§15 D849,
    /// `[X6-L6-01]`, `context-menus.md` §10, §15 D315).
    ///
    /// 🚨 **The test above was written to close this bullet and never puts a
    /// gesture in flight**, so `gesture_cancelled` is false on every frame of it
    /// and D315's guard is not on its path: deleting the guard passed the whole
    /// suite. §10 asks for *press · move · press · release* — a primary press, a
    /// move, then the secondary press that cancels.
    ///
    /// **A marquee, from empty canvas to over the first group**, because the
    /// gesture has to leave the pointer on a layer it is *not* about: D315 was
    /// reported as a right-click that cancelled a scrub and then selected the
    /// layer it happened to land on. A layer drag cannot show that — the dragged
    /// layer follows the pointer, so the click lands on it. The selection is set
    /// to the *second* group first, so "selected the one under the pointer" and
    /// "left it alone" are different answers.
    ///
    /// ⚠️ **The second right-click is the control**, with nothing in flight: a
    /// `canvas_context_menu` that never opened anything would pass the first half.
    ///
    /// ⚠️ **Flip-check, run**: deleting `canvas_context_menu`'s
    /// `if self.gesture_cancelled { return; }` fails at *"and it selected
    /// nothing"* — the menu stays shut either way, since `open_context_menu`
    /// refuses on the same flag, which is exactly why a menu-only assertion
    /// could not see the regression D315 fixed.
    #[test]
    fn a_right_click_that_cancels_a_gesture_is_spent_and_the_next_one_opens() {
        let primary = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let ctx = egui::Context::default();
        let (mut app, g1, g2, made) = app_with_two_groups(&ctx);
        whole_frame(&ctx, &mut app, Vec::new());
        app.session.selection.set(vec![g2]);
        let ppp = ctx.pixels_per_point();
        let canvas = app.canvas_rect;
        let empty = app.to_screen(ondin_core::kurbo::Point::new(200.0, 200.0), canvas, ppp);
        let over_g1 = app.to_screen(
            app.session
                .preview_world_bounds(made[0])
                .expect("a rect")
                .center(),
            canvas,
            ppp,
        );
        assert!(
            canvas.contains(empty) && canvas.contains(over_g1),
            "the fixture's points are on the canvas: {empty:?} {over_g1:?} in {canvas:?}"
        );

        whole_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(empty)]);
        whole_frame(&ctx, &mut app, vec![primary(empty, true)]);
        for step in 1..=4 {
            let t = step as f32 / 4.0;
            whole_frame(
                &ctx,
                &mut app,
                vec![egui::Event::PointerMoved(empty.lerp(over_g1, t))],
            );
        }
        assert!(
            !matches!(app.drag, crate::preview::Drag::None),
            "the fixture reached the state: a marquee is in flight"
        );

        whole_frame(&ctx, &mut app, vec![secondary(over_g1, true)]);
        whole_frame(&ctx, &mut app, vec![secondary(over_g1, false)]);
        assert!(
            app.context_menu.is_none(),
            "the cancelling click opens no menu"
        );
        assert_eq!(
            app.session.selection.ids(),
            &[g2],
            "and it selected nothing — not the group it landed on ({g1:?})"
        );

        whole_frame(&ctx, &mut app, vec![primary(over_g1, false)]);
        whole_frame(&ctx, &mut app, vec![secondary(over_g1, true)]);
        whole_frame(&ctx, &mut app, vec![secondary(over_g1, false)]);
        assert!(
            app.context_menu.is_some(),
            "and the next right-click, with nothing in flight, opens one"
        );
    }

    /// **R4 — the right-click that opens the next menu is what closes the last**
    /// (`context-menus.md` §1, `[A7-L8-06]`, §15 D795).
    ///
    /// R4 is "by construction" in the sense that the app holds one
    /// `Option<ContextMenu>`, so two menus cannot coexist whatever the code does.
    /// **That is not the part worth a test.** The part worth a test is that the
    /// second right-click leaves a menu at all: the dismissal in
    /// `context_menu_ui` and the open in `open_context_menu` run in the same
    /// frame, and if they ran in the other order the second right-click would
    /// close the first menu and open nothing. §10 predicts exactly that shape —
    /// the failure is **zero** menus, not two.
    ///
    /// ⚠️ **Flipped** by giving `open_context_menu` an early
    /// `if self.context_menu.is_some() { return; }` — "do not open a second menu
    /// over the first", which is the plausible wrong reading of R4: **red on the
    /// `expect`, the predicted site**, and red with **`None`** rather than with a
    /// menu still at the first position. That is §10's predicted shape exactly,
    /// and it arrives by a route worth naming: the refusal leaves the *first*
    /// menu in the slot, whose `just_opened` is now false, so the same click that
    /// was refused an open is taken as a click-away and dismisses it. **One
    /// click, two rules, and the two answers cancel.**
    ///
    /// ⚠️ **The flip that looks obvious here is too broad to isolate anything.**
    /// Removing `context_menu_ui`'s `!just_opened` guard fails **all four** tests
    /// in this module, at their fixture assertions: without it no menu survives
    /// the release that opened it, so there is nothing left to test a second
    /// click against. `just_opened` holds up every rule here, not one of them.
    #[test]
    fn a_second_right_click_replaces_the_open_menu_rather_than_closing_it() {
        let ctx = egui::Context::default();
        let (mut app, _, _, _) = app_with_two_groups(&ctx);
        let first = egui::pos2(400.0, 300.0);
        let second = egui::pos2(900.0, 200.0);

        right_click(&ctx, &mut app, first);
        assert_eq!(
            app.context_menu.as_ref().map(|m| m.at),
            Some(first),
            "the fixture is not in the state this test is about"
        );

        right_click(&ctx, &mut app, second);
        let menu = app
            .context_menu
            .as_ref()
            .expect("R4: the second right-click replaces the menu, it does not cancel it");
        assert_eq!(
            menu.at, second,
            "and the menu that is up is the new one — the slot holds one, ever"
        );
    }

    /// **R3 — one Escape pays out one rung, and the menu is the rung on top**
    /// (`context-menus.md` §1, `[A7-L8-06]`, §15 D795).
    ///
    /// `escape_in_present_mode_closes_the_menu_first_and_the_mode_second` already
    /// pins the ladder with *present mode* underneath. This one puts a
    /// **selection** underneath instead, and opens the menu by right-clicking
    /// rather than by calling `open_context_menu`, so it is the rule tested
    /// through the door a user actually comes in by.
    ///
    /// ⚠️ **The two are not one test twice, and the reason is the door and the
    /// state underneath rather than a ladder position.** This paragraph used to
    /// read *"present mode is a `bool` that `escape` clears on a later rung; a
    /// selection is cleared on the same rung the menu is"*, which is backwards
    /// about `escape` itself: `self.present` is that function's **first** arm and
    /// the selection clear is its **last**, the fall-through `else` under
    /// everything. What this test adds is a menu opened by a synthetic
    /// **right-click** rather than by a call to `open_context_menu` — the rule
    /// asserted through the door a user comes in by — with the bottom of the
    /// ladder underneath it instead of the top.
    ///
    /// ⚠️ **Whether the present-mode test also bites on the flip below was
    /// asserted here and never run. It was run on 2026-09-19 and it does** —
    /// under that mutation `escape_in_present_mode_closes_the_menu_first_and_the_mode_second`
    /// fails at *"and left present mode alone"*, this one fails on the selection,
    /// and the other six `escape_` tests stay green. So this test is **not** the
    /// only cover for a fall-through and was never the reason to write it; the
    /// door and the state underneath are (§15 D795).
    ///
    /// ⚠️ **Flipped** by running `input::resolve` in the menu arm right after
    /// `self.context_menu = None` — the "closed it and then fell through" version
    /// the arm's own comment warns about: **red on the selection assertion**,
    /// with `[]` against the fixture's two group ids, and **green on the menu
    /// assertion**, since the menu closes either way. That is the whole reason
    /// the selection is asserted and not just the menu.
    #[test]
    fn escape_over_a_menu_closes_the_menu_and_keeps_the_selection() {
        let ctx = egui::Context::default();
        let (mut app, g1, g2, _) = app_with_two_groups(&ctx);
        let before = app.session.selection.ids().to_vec();
        assert_eq!(
            before,
            vec![g1, g2],
            "the fixture is not in the state this test is about — an empty \
             selection would make the assertion below true of nothing"
        );

        right_click(&ctx, &mut app, egui::pos2(660.0, 410.0));
        assert!(app.context_menu.is_some(), "a menu to escape from");

        whole_frame(&ctx, &mut app, vec![key(egui::Key::Escape)]);
        assert!(app.context_menu.is_none(), "R3: the press closed the menu");
        assert_eq!(
            app.session.selection.ids(),
            before.as_slice(),
            "R3: and it closed only the menu — the selection is the rung \
             underneath and one press does not pay out two"
        );
    }

    /// **R3 with the whole ladder underneath it** — the wider fixture
    /// `context-menus.md` §10's Escape bullet asked for and the clause §15 D795
    /// left open (§15 D824).
    ///
    /// §10 words it exactly: *"Open one with a layer selected inside an entered
    /// group and the node tool active, press Escape once, and assert the menu is
    /// gone while the selection, `entered_group` and the tool are all
    /// untouched."* `escape_over_a_menu_closes_the_menu_and_keeps_the_selection`
    /// above asserts the first and the last of those; this asserts all three.
    ///
    /// 🚨 **It is a second test rather than a widening of that one, and the
    /// reason is that the wider fixture moves where the teeth are.** `escape`
    /// pays out **one** rung per press, and its chain reaches `entered_group`
    /// *before* the tool and before the selection clear that is its final `else`.
    /// So in the narrow fixture the fall-through's one extra rung is the
    /// selection — which is why that test's selection assertion bites — and in
    /// this one it is `entered_group`, leaving the selection and the tool
    /// untouched under the very flip this test exists for. **Widening the
    /// original would have taken the teeth out of it and left the suite looking
    /// one test richer.** Both fixtures are needed because the ladder has two
    /// different rungs directly under the menu depending on what is set.
    ///
    /// ⚠️ **Flipped** the same way its neighbour is — the dispatch loop
    /// (`for action in input::resolve(&ctx, self.mode, self.prefs.nudge)`) in the
    /// menu arm right after `self.context_menu = None`, the "closed it and then
    /// fell through" version the arm's own comment warns about: **red on the
    /// `entered_group` assertion**, the predicted site, with `None` against the
    /// entered group — and **green on the selection and the tool**, which is the
    /// paragraph above happening rather than being argued.
    ///
    /// ⚠️ **Two spellings of that mutation were run, and this matters because the
    /// project's rule is that a mutation named by its text names no line.** The
    /// first was `self.escape(&ctx)` in that arm — the ladder called directly,
    /// skipping the routing — and the second is the dispatch loop above, which is
    /// what the neighbour's doc and §15 D795 have always said. **Both came back
    /// red at the same assertion with the same value**, which is the expected
    /// result and not a licence to treat them as one edit: the first reaches the
    /// ladder whatever `input::resolve` decides about the key, so it could have
    /// passed while the real routing refused to produce the action at all.
    ///
    /// 🚨 **The documented run also discharges §15 D795's closing *Fix*.** That
    /// entry asked for this flip to be re-run against
    /// `escape_in_present_mode_closes_the_menu_first_and_the_mode_second` before
    /// either test is leaned on as the only cover for a fall-through, its
    /// argument having been *"asserted from a read and never run"*. Run over the
    /// whole `escape_` filter on 2026-09-22: **three of the twelve go red** —
    /// that test at *"and left present mode alone"*, the R3 selection test, and
    /// this one — and the other **nine stay green**. So no one of the three is
    /// the only cover, which is what the ask was about (§15 D824).
    #[test]
    fn escape_over_a_menu_keeps_the_entered_group_and_the_tool_as_well() {
        let ctx = egui::Context::default();
        let (mut app, g1, _, _) = app_with_two_groups(&ctx);
        app.enter_container(g1, None);
        app.choose_tool(Tool::Node);
        let selection = app.session.selection.ids().to_vec();
        assert_eq!(
            app.entered_group,
            Some(g1),
            "the fixture is not in the state this test is about — with no group \
             entered the assertion below is true of nothing"
        );
        assert_eq!(app.tool, Tool::Node, "nor with the tool left on Select");
        assert!(
            !selection.is_empty(),
            "and entering a group selects something inside it"
        );

        right_click(&ctx, &mut app, egui::pos2(660.0, 410.0));
        assert!(app.context_menu.is_some(), "a menu to escape from");

        whole_frame(&ctx, &mut app, vec![key(egui::Key::Escape)]);
        assert!(app.context_menu.is_none(), "R3: the press closed the menu");
        assert_eq!(
            app.entered_group,
            Some(g1),
            "R3: and it closed only the menu — the group scope is the rung \
             directly underneath and one press does not pay out two"
        );
        assert_eq!(
            app.tool,
            Tool::Node,
            "R3: the tool is a rung below that and is not reached either"
        );
        assert_eq!(
            app.session.selection.ids(),
            selection.as_slice(),
            "R3: and the selection, which is the bottom of the ladder"
        );
    }

    /// **No document action fires while a menu is open** (`context-menus.md` §1's
    /// R3 keyboard clause and §10, `[A7-L8-06]`, §15 D795).
    ///
    /// §10 gives the test and its flip in one line: *"Open one over a layer, press
    /// `Delete`, assert the document is byte-identical. Flip: without R3's gate
    /// the layer is gone and the menu is left pointing at nothing."* The gate is
    /// `input::resolve` not running at all while `context_menu.is_some()`.
    ///
    /// ⚠️ **"Byte-identical" is asserted as the node set plus the dirty flag**,
    /// not as bytes: `Document` has no cheap serialization reachable from here,
    /// and the two together fail for anything `Delete` could have done. The dirty
    /// flag is the load-bearing half — a delete that was undone before the
    /// assertion would leave the node set intact and the flag moved.
    ///
    /// ⚠️ **Flipped** by running `input::resolve` in the menu arm's `else`, so a
    /// non-`Escape` key reaches the document with a menu up: **red on the
    /// node-count assertion**, the predicted site, at **4 against 7** — a group
    /// and both its children, not the one node the prediction said.
    ///
    /// ⚠️ **And the dirty assertion is not "also red" — it is never reached**,
    /// because the count assertion panics first. It earns its place by covering
    /// what the count cannot (an edit undone before the assertion), not by firing
    /// alongside it, and the order is deliberate: §15 D453's lesson is to put the
    /// *loss* before the mechanism, and the lost nodes are the loss. (D614 is the
    /// balanced-tree union and carries no such rule; it was cited here by mistake
    /// — §15 D795.)
    #[test]
    fn a_keystroke_does_not_reach_the_document_while_a_menu_is_open() {
        let ctx = egui::Context::default();
        let (mut app, g1, _, made) = app_with_two_groups(&ctx);
        app.session.selection.set(vec![g1]);
        let before = node_count(&app);
        let dirty = app.session.is_dirty();

        right_click(&ctx, &mut app, egui::pos2(660.0, 410.0));
        assert!(
            app.context_menu.is_some(),
            "the fixture is not in the state this test is about"
        );

        whole_frame(&ctx, &mut app, vec![key(egui::Key::Delete)]);
        assert_eq!(
            node_count(&app),
            before,
            "the menu holds the keyboard: `Delete` must not reach the document \
             while a menu is offering its own row for the same verb"
        );
        assert_eq!(
            app.session.is_dirty(),
            dirty,
            "and nothing else reached it either — a delete undone before this \
             line would leave the count alone and move this"
        );
        assert!(
            app.session.doc.get(made[0]).is_some(),
            "the layer the menu was pointing at is still there"
        );
    }

    /// Every node reachable from the document's root, counted by walking.
    fn node_count(app: &OndinApp) -> usize {
        fn walk(doc: &Document, id: NodeId, n: &mut usize) {
            let Some(node) = doc.get(id) else { return };
            *n += 1;
            for c in node.children() {
                walk(doc, *c, n);
            }
        }
        let mut n = 0;
        walk(&app.session.doc, app.session.doc.root(), &mut n);
        n
    }

    fn probe_app(ctx: &egui::Context) -> (OndinApp, NodeId) {
        use ondin_core::{NodeKind, Operation, Stroke, Transaction};
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
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
        app.session
            .try_commit(Transaction(vec![
                Operation::SetStrokes {
                    id,
                    strokes: vec![Stroke {
                        width: 2.0,
                        ..Default::default()
                    }],
                },
                Operation::SetEffects {
                    id,
                    effects: vec![ondin_core::Effect::new(ondin_core::EffectKind::DropShadow(
                        Default::default(),
                    ))],
                },
                Operation::SetExports {
                    id,
                    exports: vec![ondin_core::ExportSpec::new(
                        ondin_core::ExportFormat::Png,
                        ondin_core::ExportScale::Times(1.0),
                    )],
                },
            ]))
            .expect("a stroke, an effect and an export to draw");
        app.session.selection.set_one(id);
        (app, id)
    }

    /// A text node, for the one popover a shape cannot reach.
    fn probe_text_app(ctx: &egui::Context) -> OndinApp {
        use ondin_core::{NodeKind, Operation, Transaction};
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Text {
                content: "hello".into(),
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
            transform: None,
            name: None,
        }]))
        .expect("a text node");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        app
    }

    /// **One `Escape` closes a popover and pays out no rung of the ladder** — all
    /// five of them (§15 D801, D527's rule, `roadmap.md`'s *Now · Keyboard*).
    ///
    /// The roadmap held this as a question with an instruction attached —
    /// *"measure before writing anything down"* — because D527's own dropdown
    /// case was red at the **tool** and green at the **menu**, so the assertion
    /// has to be on the state that was supposed to survive. Measured first, on a
    /// headless app with the tool set to `Rect` so the ladder's last rung is the
    /// thing that moves:
    ///
    /// | popover | closed on `Escape`? | ladder paid out? |
    /// | --- | --- | --- |
    /// | none — the control | — | **yes**, `Rect` → `Select` |
    /// | `stroke_menu` | yes | **yes** |
    /// | `effect_menu` | yes | **yes** |
    /// | `type_menu` | yes | **yes** |
    /// | `export_menu` | **no** | **yes** |
    /// | `export_row_open` | **no** | **yes** |
    ///
    /// **Both shapes the roadmap predicted were real, twice each.** The three
    /// inspector popovers read `key_pressed`, which does not consume, so they
    /// closed themselves *and* let the press through — D533's second consequence,
    /// one press taking two things. The Export card's two read no key at all, so
    /// the press did the one thing it should not and none of the thing it should.
    ///
    /// 🚨 **The fixture is what nearly made this wrong.** On a *rect*, `type_menu`
    /// measured as `up_after = true` — looking exactly like the Export card's
    /// failure — because a shape draws no Typography panel, so nothing ran the
    /// handler and the flag merely survived. On a text node it measures the
    /// opposite. **A popover that nothing is drawing answers every key the same
    /// way**, which is indistinguishable from ignoring them, and it is why this
    /// test keeps a `up_before` assertion per case rather than one at the end.
    ///
    /// ⚠️ **The control is not optional.** Every assertion here is that the tool
    /// *did not* change, which is exactly what a broken ladder also produces —
    /// so without a case that proves `Escape` still pays out when nothing is up,
    /// the whole test would pass against an `escape` that had stopped working.
    ///
    /// ⚠️ **Flipped** three ways, all run.
    ///
    /// - **`a_popover_owns_escape`'s arm removed from `update`**: red on
    ///   `stroke_menu`'s tool assertion, the first popover case.
    /// - **`a_popover_owns_escape` narrowed to drop `type_menu`**: red on that
    ///   case alone, with `Select` against `Rect`. The gate is per-popover and
    ///   nothing else covers any one of the five.
    /// - **The Export card's new `key_pressed` block removed**: red on
    ///   `export_menu`'s *closed* assertion. ⚠️ **Whether its tool assertion also
    ///   goes red is reasoned rather than measured** — the closed assertion
    ///   panics first and the tool one is never reached. The reasoning:
    ///   `export_menu` is still `true` under that flip, so
    ///   `a_popover_owns_escape` still fires and the ladder stays gated. **The
    ///   two halves are independent** — one arm stops the rung, one block closes
    ///   the popover — which is the argument for asserting both per case and not
    ///   one at the end.
    /// - **The arm swallowing every key, not only `Escape`** (§15 D847,
    ///   `[X6-L6-02]`, added with the tool-letter half of each case): red on
    ///   `stroke_menu`'s *"a tool letter still reaches the keymap"*, `Rect`
    ///   against `Ellipse`. Before that half existed this flip passed the whole
    ///   suite.
    #[test]
    fn escape_over_a_popover_closes_it_and_pays_out_no_rung() {
        use crate::tools::Tool;

        let case = |name: &str,
                    mut app: OndinApp,
                    ctx: &egui::Context,
                    open: &dyn Fn(&mut OndinApp),
                    up: &dyn Fn(&OndinApp) -> bool| {
            app.tool = Tool::Rect;
            // egui resolves widget rects a frame late and the popovers' own
            // click-away predicates read them, so an unsettled frame dismisses
            // the popover before the key ever arrives.
            whole_frame(ctx, &mut app, Vec::new());
            open(&mut app);
            whole_frame(ctx, &mut app, Vec::new());
            assert!(
                up(&app),
                "{name}: the fixture is not in the state this case is about — a \
                 popover nothing draws answers every key the same way"
            );

            whole_frame(ctx, &mut app, vec![key(egui::Key::Escape)]);
            assert!(!up(&app), "{name}: Escape closes the popover");
            assert_eq!(
                app.tool,
                Tool::Rect,
                "{name}: and the press is spent on it — the tool is the rung \
                 underneath and one press does not pay out two"
            );

            // 🚨 **The other half of the arm: every key that is not `Escape`
            // goes through** (§15 D847, `[X6-L6-02]`). A version that swallowed
            // *every* key while a popover was open passed this whole suite, and
            // under it `Delete`, the arrows, every tool letter and every chord
            // were dead.
            open(&mut app);
            whole_frame(ctx, &mut app, Vec::new());
            assert!(up(&app), "{name}: reopened for the second half");
            whole_frame(ctx, &mut app, vec![key(egui::Key::E)]);
            assert_eq!(
                app.tool,
                Tool::Ellipse,
                "{name}: a tool letter still reaches the keymap over an open popover"
            );
        };

        let ctx = egui::Context::default();
        case(
            "stroke_menu",
            probe_app(&ctx).0,
            &ctx,
            &|a| a.stroke_menu = Some(0),
            &|a| a.stroke_menu.is_some(),
        );
        case(
            "effect_menu",
            probe_app(&ctx).0,
            &ctx,
            &|a| {
                let id = a.session.selection.ids()[0];
                a.effect_menu = Some((id, 0));
            },
            &|a| a.effect_menu.is_some(),
        );
        case(
            "type_menu",
            probe_text_app(&ctx),
            &ctx,
            &|a| a.type_menu = Some(crate::app::TypeTab::Character),
            &|a| a.type_menu.is_some(),
        );
        case(
            "export_menu",
            probe_app(&ctx).0,
            &ctx,
            &|a| a.export_menu = true,
            &|a| a.export_menu,
        );
        case(
            "export_row_open",
            probe_app(&ctx).0,
            &ctx,
            &|a| a.export_row_open = Some(0),
            &|a| a.export_row_open.is_some(),
        );

        // **The control, and it carries the whole test.** Every assertion above
        // is that the tool did *not* move, which a broken ladder also produces.
        let mut app = probe_app(&ctx).0;
        app.tool = Tool::Rect;
        whole_frame(&ctx, &mut app, Vec::new());
        whole_frame(&ctx, &mut app, vec![key(egui::Key::Escape)]);
        assert_eq!(
            app.tool,
            Tool::Select,
            "with nothing open the press reaches the ladder and drops the tool — \
             without this the five cases above pass against an `escape` that has \
             stopped working entirely"
        );
    }

    /// **A popover flag nothing is drawing does not own `Escape`** (§15 D847,
    /// `[X6-L1-01]`) — the two cases the finding reproduced, both through
    /// `whole_frame`.
    ///
    /// *Case B, the ordinary one*: the Type popover is open over a text layer
    /// and the selection moves off it. `inspector_type` runs only for a text
    /// selection, so nothing draws the popover and nothing can clear its flag —
    /// and the flag alone gated `Escape`, so the whole ladder was dead, with
    /// nothing on screen to say why. **Here the press pays out its rung** (the
    /// tool drops to Select) **and the dead flag goes**, so it cannot pop back up
    /// on the next text selection.
    ///
    /// *Case A, present mode*: entering it clears `open_menu` and nothing else,
    /// and the inspector is behind `if !self.present`. The toast says *"Escape to
    /// leave"*, and `Escape` was the one key the stale flag swallowed.
    ///
    /// ⚠️ **Flip-checks, run**: dropping `popover_heard` from the gate — the
    /// shipped predicate — fails case B at *"the press reaches the ladder"*,
    /// with `Rect` against `Select`; keeping it and dropping the dead-flag
    /// clearing fails at *"and the dead flag is cleared"*. **Case A has no flip
    /// of its own** — case B's failure stops the test first — so its teeth are
    /// the same predicate's, argued rather than observed.
    #[test]
    fn a_popover_flag_nothing_draws_does_not_own_escape() {
        use crate::input::ViewSwitch;
        use crate::tools::Tool;
        let ctx = egui::Context::default();

        // Case B.
        let mut app = probe_text_app(&ctx);
        app.tool = Tool::Rect;
        whole_frame(&ctx, &mut app, Vec::new());
        app.type_menu = Some(crate::app::TypeTab::Character);
        whole_frame(&ctx, &mut app, Vec::new());
        assert!(app.type_menu.is_some(), "the popover is up over the text");
        app.session.selection.clear();
        whole_frame(&ctx, &mut app, Vec::new());
        assert!(
            app.type_menu.is_some(),
            "the fixture reached the state: the flag survives with nothing drawing it"
        );
        whole_frame(&ctx, &mut app, vec![key(egui::Key::Escape)]);
        assert_eq!(app.tool, Tool::Select, "the press reaches the ladder");
        assert!(app.type_menu.is_none(), "and the dead flag is cleared");

        // Case A.
        let mut app = probe_text_app(&ctx);
        whole_frame(&ctx, &mut app, Vec::new());
        app.type_menu = Some(crate::app::TypeTab::Character);
        whole_frame(&ctx, &mut app, Vec::new());
        app.set_view_switch(ViewSwitch::Present, true);
        whole_frame(&ctx, &mut app, Vec::new());
        assert!(
            app.present && app.type_menu.is_some(),
            "the fixture reached the state: presenting, with the flag still set"
        );
        whole_frame(&ctx, &mut app, vec![key(egui::Key::Escape)]);
        assert!(
            !app.present,
            "one Escape leaves present mode, as its toast says"
        );
    }
}

#[cfg(test)]
mod clipboard_gate_tests {
    //! **Two threads opening the OS clipboard at once corrupt the heap** (§15
    //! D796).
    //!
    //! Found by writing the four context-menu rule tests above: each opens a
    //! menu, `ContextMenu` snapshots the clipboard on every open, and four of
    //! them in one module failed four runs in five with
    //! `STATUS_HEAP_CORRUPTION`. The whole `ondin-app` suite passed throughout —
    //! the tests that could collide were simply spread too thin to meet often.

    /// The gate holds under eight threads doing nothing but opening it.
    ///
    /// **This test spawns its own threads rather than leaning on the harness.**
    /// `cargo test`'s parallelism is what *found* the fault, and it is exactly
    /// the wrong thing to assert against: the number of threads is the machine's,
    /// the interleaving is the scheduler's, and a suite that grows by one test
    /// changes both. What made the original failure so easy to miss is that it
    /// only showed up when the colliding tests were the *only* ones selected.
    ///
    /// ⚠️ **The failure is not a panic, so there is no assertion under it.**
    /// A corrupted heap takes the whole test binary down with
    /// `STATUS_HEAP_CORRUPTION`, which cargo reports as a hard failure of the
    /// target — the same shape as §15 D445's `panic = "abort"` guard, where the
    /// evidence is the process dying rather than a line of `assert!`. The
    /// `join` below is the real assertion: if a thread died, this does.
    ///
    /// ⚠️ **Flipped** by taking `with_clipboard`'s `GATE` lock out and opening
    /// `arboard::Clipboard::new()` bare: the binary aborts, and the run reports
    /// `process didn't exit successfully … (exit code: 0xc0000374,
    /// STATUS_HEAP_CORRUPTION)` with **no test named** — so the flip's failure
    /// message does not say which test did it. That is the argument for the
    /// module doc above carrying the story rather than the assertion.
    ///
    /// ⚠️ **It reads and never writes.** Asserting on the *contents* would mean
    /// setting the clipboard, and a test has no business overwriting what the
    /// developer copied — the same rule that keeps `copy_as_png`'s own test on
    /// `Self::png_for_the_clipboard` rather than on the arm that reaches the OS.
    #[test]
    fn eight_threads_can_read_the_clipboard_at_once() {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(|| {
                    for _ in 0..16 {
                        let _ = super::with_clipboard(|c| c.get_text().ok());
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("a reader thread came back");
        }
    }

    /// A headless app takes the OS clipboard off the table for the whole process.
    ///
    /// **The point of §15 D798**, and the reason `clipboard_is_reachable` is
    /// checked by the four callers rather than inside `with_clipboard`: the test
    /// above has to keep reaching a real `arboard` handle to prove the lock
    /// holds, and this one has to see the refusal. They would be the same test if
    /// the check sat one level down, and it would be the vacuous one.
    ///
    /// ⚠️ **It asserts the readers answer *empty*, not that they were skipped**,
    /// because that is all a caller can see — and it is the behaviour that
    /// matters: a menu built in a probe offers no *Paste* row, deterministically,
    /// instead of offering whatever the developer last copied.
    ///
    /// ⚠️ **Order-independent on purpose.** The flag is one-way and every other
    /// test in this binary that builds a headless app sets it too, so this test
    /// cannot be made to run "before" the others and does not try — it builds one
    /// itself and asserts from there.
    ///
    /// ⚠️ **Flipped** by dropping the `CLIPBOARD_OFF.store` from
    /// `OndinApp::headless`: **red on the first assertion**, which is the one
    /// that does not depend on what the developer has copied. That ordering is
    /// the whole reason the flag is asserted directly before the two readers are
    /// — the reader assertions are the *behaviour*, but on an empty clipboard
    /// they are also green under the flip, so on their own they would be a test
    /// that passes whenever the machine happens to be quiet.
    #[test]
    fn a_headless_app_cannot_reach_the_clipboard() {
        let ctx = egui::Context::default();
        let _app = super::OndinApp::headless(&ctx);

        assert!(
            !super::clipboard_is_reachable(),
            "building a headless app is what closes it"
        );
        assert_eq!(
            super::system_clipboard_text(),
            None,
            "so the text reader answers an empty clipboard"
        );
        assert!(
            !super::system_clipboard_has_image(),
            "and the picture reader answers no"
        );
    }
}

#[cfg(test)]
mod clipboard_crossing_tests {
    //! **A layer copied in one `ondin` window pastes as a layer in another**
    //! (§15 D823, `roadmap.md`'s *Now · Canvas*).
    //!
    //! Until D823 a copy put the layer *names* on the system clipboard and kept
    //! the real subtrees in `OndinApp::clipboard`, so nothing crossed the process
    //! boundary: a second window's `Ctrl+V` saw text, and pasted the names as a
    //! text layer. The payload now follows the names past `io::clip::FENCE`.
    //!
    //! 🚨 **One process standing in for two, and that is the honest limit of
    //! this module.** `OndinApp::headless` sets `CLIPBOARD_OFF` (§15 D798), so
    //! no test here can put anything on the real OS clipboard or read it back —
    //! which is why `OndinApp::adopt_clip_text` exists as its own function. What
    //! is asserted is that the text a real `copy_selection` produced is enough,
    //! on its own, to rebuild the layers in an app that has never seen the
    //! document they came from. What is *not* asserted is that the OS carries
    //! that text between two processes, which is `arboard`'s job and needs two
    //! windows to see.

    use super::{Clipboard, OndinApp};
    use ondin_core::kurbo::Size;
    use ondin_core::{Fill, IdSource, NodeId, NodeKind, Operation, Transaction, image_brush};

    /// Every node reachable from the root, which is what a paste adds to.
    fn node_ids(app: &OndinApp) -> Vec<NodeId> {
        fn walk(doc: &ondin_core::Document, id: NodeId, out: &mut Vec<NodeId>) {
            out.push(id);
            let Some(node) = doc.get(id) else { return };
            for c in node.children() {
                walk(doc, *c, out);
            }
        }
        let mut out = Vec::new();
        walk(&app.session.doc, app.session.doc.root(), &mut out);
        out
    }

    /// **The whole crossing, end to end**: copy two groups in one app, hand the
    /// clipboard text to a second app that has never seen that document, and the
    /// six nodes arrive as layers with ids of the *second* app's minting.
    ///
    /// **The id assertion is not decoration.** A captured subtree keeps the
    /// original's ids until `document::remap_subtree` replaces them, and those ids
    /// are minted against a per-process-random actor (`session::actor_id`) — so a
    /// payload arriving from another window is a template, and a paste that took
    /// its ids at face value would put one process's ids into another process's
    /// document and break invariant 3 the first time the two met.
    ///
    /// ⚠️ **Flipped** by restoring the pre-D823 copy — `stamp_clipboard(ctx,
    /// heading)` in place of the payload write, which is exactly the line this
    /// entry changed rather than a deletion of the feature: **red on the
    /// `Clipboard::Layers` assertion**, the predicted site, with `Foreign`,
    /// because layer names carry no fence. The node assertions below never run
    /// under it, which is the right order — the crossing is the claim and the
    /// arithmetic is its consequence.
    ///
    /// ⚠️ **That flip takes `an_unreadable_copy_is_reported_rather_than_pasted_as_text`
    /// down too, and at its *fixture* assertion rather than its claim** — the
    /// version rewrite finds nothing to rewrite in `"Group 1, Group 2"`, so
    /// `assert_ne!` fires before the test reaches anything it is about. Worth
    /// the sentence because it is the fixture rule earning its keep: without
    /// that line the rewrite would have been a no-op, the unchanged text would
    /// have read as an ordinary copy, and a test whose whole subject is the
    /// refusal would have failed claiming the refusal did not happen.
    #[test]
    fn a_copy_from_another_window_pastes_as_layers_with_fresh_ids() {
        let ctx = egui::Context::default();
        let (mut source, g1, g2, _) = super::ungroup_tests::app_with_two_groups(&ctx);
        assert_eq!(
            source.session.selection.ids(),
            [g1, g2],
            "the fixture is not in the state this test is about — an empty \
             selection copies nothing and every assertion below would be vacuous"
        );

        let text = source
            .copy_selection(&ctx)
            .expect("a layer copy writes the clipboard");
        let from_source = node_ids(&source);

        // A second window: its own app, its own `IdSource`, its own document,
        // which has never held any of the nodes about to arrive.
        let mut target = OndinApp::headless(&ctx);
        let before = node_ids(&target);

        assert!(
            matches!(target.adopt_clip_text(text), Clipboard::Layers),
            "the text a copy leaves behind is enough to recognise it as ours"
        );
        assert!(target.paste_clipboard(), "and enough to paste it");

        let after = node_ids(&target);
        assert_eq!(
            after.len() - before.len(),
            6,
            "two groups of two rects each, and the groups themselves"
        );
        let arrived: Vec<NodeId> = after
            .into_iter()
            .filter(|id| !before.contains(id))
            .collect();
        assert!(
            arrived.iter().all(|id| !from_source.contains(id)),
            "and not one of them kept the id it had in the window it came from"
        );
    }

    /// 🚨 **The picture crosses too, which is the one part of a layer that is
    /// not in the subtree** (§15 D842).
    ///
    /// A fill stores an image *key* and the bytes live in a table on the
    /// document, so a cross-window paste is the **only** route where the target
    /// is guaranteed not to hold the entry already — within one process the
    /// in-app `Clip` and the document share the table, and `insert_all`'s own
    /// doc says the duplicate path passes no images at all. If the bytes do not
    /// make the crossing the layer arrives as the missing-picture placeholder,
    /// or on a stroke as nothing whatever (§15 D179), with the node count and
    /// every id assertion above still green.
    ///
    /// ⚠️ **The module's other three tests pass with `images: Vec::new()`
    /// substituted into `adopt_clip_text`**, because their fixture is two groups
    /// of plain rects with no fill referencing an `ImageId` anywhere — so this
    /// module's own header claim, that the text *"is enough, on its own, to
    /// rebuild the layers"*, was asserted against the one kind of layer that has
    /// nothing to rebuild. **The fixture never reached the state it named.**
    ///
    /// ⚠️ **Pasted twice on purpose.** `build::missing_image_ops` filters on
    /// `!doc.has_image(id)` and nothing else, so "the entry arrives" and "the
    /// entry arrives once" are two claims, and the second is the one a filter
    /// written as `always add` would fail while the first stayed green.
    ///
    /// Flip: `images: Vec::new()` in `adopt_clip_text`'s `Clip`. Red on the
    /// `image(&id)` assertion, the predicted site, with the layers all present
    /// and one of them drawing a placeholder — which is exactly the failure the
    /// node-count assertions cannot see.
    #[test]
    fn a_copy_from_another_window_brings_its_pictures() {
        let ctx = egui::Context::default();
        let mut source = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0xB17E);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let rect = ids.mint();
        let id = ondin_core::ImageId("a-photo".into());
        let entry = ondin_core::ImageEntry {
            source: ondin_core::ImageSource::Embedded(vec![9, 8, 7, 6].into()),
            format: ondin_core::ImageFormat::Png,
            width: 2,
            height: 2,
        };
        doc.apply(&Transaction(vec![
            Operation::AddImage {
                id: id.clone(),
                entry: entry.clone(),
            },
            Operation::CreateNode {
                id: rect,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id: rect,
                fills: vec![Fill {
                    brush: image_brush(id.clone()),
                    visible: true,
                }],
            },
        ]))
        .expect("a rect painted with a picture");
        source.session.adopt_document(doc, None);
        source.session.selection.set_one(rect);

        let text = source
            .copy_selection(&ctx)
            .expect("a layer copy writes the clipboard");

        let mut target = OndinApp::headless(&ctx);
        assert!(
            target.session.doc.image(&id).is_none(),
            "the fixture must reach the state: a target that already held the \
             entry would pass this test without anything crossing"
        );
        assert!(matches!(target.adopt_clip_text(text), Clipboard::Layers));
        assert!(target.paste_clipboard(), "the layer pastes");

        assert_eq!(
            target.session.doc.image(&id),
            Some(&entry),
            "the bytes crossed with the layer that keys into them — a node \
             carries the key and the document carries the picture"
        );

        assert!(target.paste_clipboard(), "and it pastes a second time");
        assert_eq!(
            target.session.doc.image(&id),
            Some(&entry),
            "with the table entry added once, not twice — `missing_image_ops` \
             filters on `!doc.has_image(id)` and this is what says so"
        );
    }

    /// **Someone else's clipboard is still someone else's**, which is the arm
    /// that keeps `Ctrl+V` over a sentence working.
    ///
    /// ⚠️ **Flipped** by having `io::clip::read` answer `Some(parse(text))` for
    /// text with no fence in it — the plausible wrong version, "try the parse and
    /// see": **red on the `Foreign` assertion**, at `Unreadable`, which is the
    /// worse of the two failures it could produce. A paste of an ordinary
    /// sentence would stop with *"that copy came from a build this one cannot
    /// read"* instead of making the text layer the user asked for.
    #[test]
    fn ordinary_text_is_not_mistaken_for_a_copy() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        assert!(
            matches!(
                app.adopt_clip_text("a sentence copied in a browser".to_string()),
                Clipboard::Foreign
            ),
            "no fence, so the caller's markup and text arms get it"
        );
        assert!(
            app.clipboard.is_none(),
            "and nothing was adopted on the way past"
        );
    }

    /// **A copy this build cannot read stops the paste rather than becoming
    /// one**, which is the whole reason `Clipboard` has three variants.
    ///
    /// ⚠️ **Flipped** by folding `Unreadable` into `Foreign` at
    /// `adopt_clip_text`'s error arm — the shape a `bool` return would have
    /// forced: **red on the `Unreadable` assertion**, and the cost the variant
    /// buys is what the second assertion names. `paste` would fall through to
    /// `paste_text_as_layer` and the user would get a text layer holding their
    /// own payload.
    #[test]
    fn an_unreadable_copy_is_reported_rather_than_pasted_as_text() {
        let ctx = egui::Context::default();
        let (mut source, _, _, _) = super::ungroup_tests::app_with_two_groups(&ctx);
        let text = source.copy_selection(&ctx).unwrap();
        let bumped = text.replace(
            &format!(
                "\"schema_version\":{}",
                ondin_core::io::CURRENT_SCHEMA_VERSION
            ),
            "\"schema_version\":9999",
        );
        assert_ne!(bumped, text, "the fixture really was rewritten");

        let mut target = OndinApp::headless(&ctx);
        assert!(
            matches!(target.adopt_clip_text(bumped), Clipboard::Unreadable),
            "ours, and unreadable — which is not the same answer as not ours"
        );
        assert!(
            matches!(
                target.session.status().kind,
                crate::session::StatusKind::Error
            ),
            "and the user is told, because nothing else in the paste path will"
        );
    }

    /// **The receipt answers exactly the text a copy wrote, and holds none of it**
    /// (§15 D857, `[X1.2-L4-01]`).
    ///
    /// The stamp was the text, which since §15 D823 is the whole payload — so a
    /// copy of one photo held megabytes of base64 for the session in both windows.
    /// It is a length and a hash now, and what it must still do is the receipt's
    /// whole job: say *yes* to the text written and *no* to anything else,
    /// including a text that differs by one byte and no text at all. The target
    /// side is asserted too, since adopting a foreign copy stamps it.
    ///
    /// ⚠️ **Flip-check, run**: `ClipStamp::of` hashing nothing (a constant hash,
    /// the length alone) fails at *"a text of the same length that is not ours"*.
    #[test]
    fn the_receipt_answers_exactly_the_text_written_and_holds_none_of_it() {
        let ctx = egui::Context::default();
        let (mut source, _, _, _) = super::ungroup_tests::app_with_two_groups(&ctx);
        let text = source.copy_selection(&ctx).expect("a layer copy");
        assert!(
            text.len() > 200,
            "the fixture: a payload, not a name — {} bytes",
            text.len()
        );

        assert!(
            source.owns_the_clipboard(Some(&text)),
            "the text written is ours"
        );
        let mut same_len = text.clone().into_bytes();
        let last = same_len.len() - 1;
        same_len[last] = if same_len[last] == b'}' { b']' } else { b'}' };
        let same_len = String::from_utf8(same_len).unwrap();
        assert!(
            !source.owns_the_clipboard(Some(&same_len)),
            "a text of the same length that is not ours"
        );
        assert!(
            !source.owns_the_clipboard(Some(&format!("{text} "))),
            "a longer one"
        );
        assert!(!source.owns_the_clipboard(None), "and an empty clipboard");
        assert!(
            std::mem::size_of_val(&source.clipboard_stamp) <= 24,
            "and the receipt is a fixed size, whatever was copied"
        );

        let mut target = OndinApp::headless(&ctx);
        assert!(matches!(
            target.adopt_clip_text(text.clone()),
            Clipboard::Layers
        ));
        assert!(
            target.owns_the_clipboard(Some(&text)),
            "an adopted copy is stamped, so a second paste takes the fast path"
        );
    }
}

#[cfg(test)]
mod undo_rewind_tests {
    //! **Undo is a structural edit, and the app-side state that indexes into the
    //! document has to be told** (`[S19.2-L1-01]`, §15 D568).
    //!
    //! `PointSet::retain_valid`'s doc states the rule these assertions are about:
    //! *"deleting an anchor renumbers every anchor after it in that subpath, so a
    //! set kept across the edit would silently point at the wrong points — near-miss
    //! selections being the kind of bug that only shows up three edits later."*
    //! Every structural edit in the node tool obeys it; undo and redo were the two
    //! doors with nothing, and they are also the two where truncation is not enough.
    //!
    //! ⚠️ **The reason the test asserts on a *position* rather than on the index
    //! set.** Both failures here leave every index in range, so a `retain_valid`
    //! call on the undo path is a no-op and an assertion written against
    //! `points.on(id)` passes under the bug. What moved is what the index *means*.

    use super::{OndinApp, Tool};
    use eframe::egui;
    use ondin_core::kurbo::Point;
    use ondin_core::{Document, NodeId, NodeKind, Operation, Transaction};

    /// An open three-anchor path at `(0,0) (100,0) (100,100)`, selected, with the
    /// node tool armed — `edited_path`'s three preconditions.
    pub(super) fn app_with_three_anchors(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(11);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        let mut p = ondin_core::kurbo::BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((100.0, 0.0));
        p.line_to((100.0, 100.0));
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Path {
                path: p,
                corner_radii: Vec::new(),
            },
            transform: None,
            name: None,
        }]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        app.tool = Tool::Node;
        (app, id)
    }

    /// The path as the document stores it — the fixture assertions' witness.
    fn stored(app: &OndinApp, id: NodeId) -> String {
        let NodeKind::Path { path, .. } = app.session.doc.get(id).expect("a live node").kind()
        else {
            panic!("a Path");
        };
        path.to_svg()
    }

    /// **Delete the middle anchor, pick the last one, `Ctrl+Z` — the selection must
    /// not come back naming a different point.**
    ///
    /// The reported sequence, driven through the app's own verbs. After the delete
    /// the path is `(0,0) (100,100)` and index `(0,1)` means the corner at
    /// `(100,100)`; the undo restores the third anchor, and `(0,1)` now means
    /// `(100,0)` — an anchor the user never clicked, which the inspector's X/Y,
    /// `draw_points`' markers, align and every drag then read, because they all go
    /// through `subject_points`.
    ///
    /// **In-run control:** the pre-undo reading, asserted to be the picked anchor.
    /// Without it this test cannot tell "the fix worked" from "the fixture never
    /// selected anything".
    ///
    /// **Flip run**, `document_rewound`'s `points.clear()` removed: fails on *"the
    /// selection may not survive an undo naming a different anchor"* with
    /// `[(100.0, 0.0)]` against `[]` — the predicted site. ⚠️ **Predicted and
    /// confirmed for once**, and only because the assertion is on the position. The
    /// last assertion in the body is the reason: it runs `retain_valid` — the guard
    /// every other structural edit uses — against the restored path and shows it
    /// keeps `(0, 1)`, so a test written against the *index set* would have been
    /// green under the bug and green under a fix that does not work.
    #[test]
    fn a_point_selection_does_not_survive_an_undo_that_renumbers_it() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_three_anchors(&ctx);

        app.points.set_one(id, (0, 1));
        app.delete_points();
        assert_eq!(
            stored(&app, id),
            "M0,0 L100,100",
            "the fixture: the middle anchor is gone and the path healed"
        );

        app.points.set_one(id, (0, 1));
        assert_eq!(
            app.selected_point_positions(id),
            vec![Point::new(100.0, 100.0)],
            "the control: the click picked the corner, before any undo"
        );

        app.undo();
        assert_eq!(
            stored(&app, id),
            "M0,0 L100,0 L100,100",
            "the fixture: the undo put the middle anchor back"
        );
        assert_eq!(
            app.selected_point_positions(id),
            Vec::<Point>::new(),
            "the selection may not survive an undo naming a different anchor"
        );

        // **`retain_valid` would not have caught this, which is why the fix
        // clears.** The restored path has three anchors, so `(0, 1)` is in range
        // and the truncation every *other* structural edit uses is a no-op here —
        // it would have left the set lit on `(100, 0)`, the anchor nobody picked.
        let mut kept = crate::preview::PointSet::default();
        kept.set_one(id, (0, 1));
        kept.retain_valid(&[3]);
        assert_eq!(
            kept.on(id).collect::<Vec<_>>(),
            vec![(0, 1)],
            "the guard the other doors use keeps an index the rewind has repointed"
        );
    }

    /// **`Ctrl+Z` at the bottom of the stack changes nothing**, which is what makes
    /// the clear conditional on `EditorSession::undo`'s answer rather than on the
    /// keystroke.
    ///
    /// Without the `bool`, pressing undo once more than there is history to take
    /// back would silently deselect the points — a second, smaller version of the
    /// same complaint.
    ///
    /// **Flip run**, `OndinApp::undo` calling `document_rewound` unconditionally:
    /// fails here at `[]` against `[(100.0, 100.0)]`, and leaves the test above
    /// green — so the two assertions are about different halves of the fix and
    /// neither covers the other.
    #[test]
    fn an_undo_with_nothing_to_take_back_leaves_the_point_selection_alone() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_three_anchors(&ctx);

        app.points.set_one(id, (0, 2));
        assert_eq!(
            app.selected_point_positions(id),
            vec![Point::new(100.0, 100.0)],
            "the control: the last anchor is picked"
        );

        app.undo();
        assert!(
            !app.session.history.can_undo(),
            "the fixture: the history was empty, so nothing was rewound"
        );
        assert_eq!(
            app.selected_point_positions(id),
            vec![Point::new(100.0, 100.0)],
            "and a keystroke that moved nothing may not clear the selection"
        );
    }

    /// **A pen resuming an existing path must not put back what an undo took**, the
    /// half of D568 that is not a reference but a *snapshot*.
    ///
    /// `PenEdit::subpaths` holds every subpath the node had when the pen resumed and
    /// `finish_pen` writes the whole list back through `rewrite_path`. So a pen run
    /// that spans an undo is a resurrection: the geometry the undo removed is still
    /// in the pen's copy and goes back into the document when the run ends.
    ///
    /// Driven at the state rather than through the gesture — the assertion is that
    /// the pen is *gone*, because a live `PenState` after a rewind is the bug
    /// whatever the next click does with it.
    ///
    /// **Flip run**, `document_rewound`'s `self.pen = None` removed: fails on *"an
    /// undo drops a pen run whose snapshot it has just invalidated"*.
    #[test]
    fn an_undo_drops_a_live_pen_run() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_three_anchors(&ctx);

        // One committed edit for the undo to take back, and a pen run resumed on
        // the node that edit changed.
        app.points.set_one(id, (0, 1));
        app.delete_points();
        assert_eq!(
            stored(&app, id),
            "M0,0 L100,100",
            "the fixture: there is one edit on the stack, and the pen's snapshot \
             below is taken after it"
        );
        app.pen = Some(pen_resumed_on(id));
        assert!(app.pen.is_some(), "the fixture: a pen run is live");

        app.undo();
        assert!(
            app.pen.is_none(),
            "an undo drops a pen run whose snapshot it has just invalidated"
        );
    }

    /// A `PenState` extending `id`, shaped the way `canvas::pen_resume` shapes one:
    /// the node's own world transform, its subpaths as they stand at the resume, and
    /// the last run active.
    ///
    /// Written out rather than taken from `edited_subpaths`, which is private to
    /// `canvas`. The two anchors are the healed path this fixture is holding — what
    /// matters to the assertion is that the snapshot exists, not what is in it.
    fn pen_resumed_on(id: NodeId) -> crate::preview::PenState {
        let run = crate::preview::PenSubpath {
            anchors: vec![
                crate::preview::PenAnchor::corner(Point::new(0.0, 0.0)),
                crate::preview::PenAnchor::corner(Point::new(100.0, 100.0)),
            ],
            closed: false,
        };
        crate::preview::PenState {
            parent: id,
            parent_world: ondin_core::kurbo::Affine::IDENTITY,
            anchors: run.anchors.clone(),
            shaping: false,
            editing: Some(crate::preview::PenEdit {
                id,
                world: ondin_core::kurbo::Affine::IDENTITY,
                subpaths: vec![run],
                active: 0,
                from_start: false,
                minted: false,
            }),
        }
    }
}

#[cfg(test)]
mod paint_drag_cancel_tests {
    //! **A paint-row reorder is a gesture `cancel_gesture` can find** (§15 D588,
    //! `[S14.3-L1-02]`).
    //!
    //! The function *cleared* `paint_drag` — under a comment saying it is *"the
    //! fourth, for the same reason"* — and the gate above that line never
    //! mentioned it, so the whole body was unreachable whenever a paint reorder
    //! was the only gesture in flight. The one term that ever answered for it was
    //! `ctx.dragged_id()`, and this function's own comment names that as the
    //! blinking one: egui clears `dragged` on the release of **any** button, and
    //! `cancel_gesture` runs on `button_pressed(Secondary)`, so a right-click
    //! whose press and release land in one rendered frame reads a snapshot that
    //! has already been cleared. ⚠️ **Nor does it recover** — egui will not re-arm
    //! the drag while the primary stays down — so one fast right-click made the
    //! reorder uncancellable for the rest of the gesture and the release committed
    //! it: `[4.0, 9.0] → [9.0, 4.0]`, `undo_depth +1`.
    //!
    //! (Plain backticks rather than `[links]`, per §15 D319: `cargo doc` builds
    //! without the `test` cfg, so a link here is checked by nothing.)

    use super::OndinApp;
    use crate::panels::paint::{PaintDrag, PaintList, Slots};
    use eframe::egui;
    use ondin_core::{Document, IdSource};

    /// A headless app with a paint-row reorder in flight and **nothing else**.
    fn app_carrying_a_paint_row(ctx: &egui::Context) -> OndinApp {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x9A1);
        let root = ids.mint();
        app.session.adopt_document(Document::new(root), None);
        app.in_flight.paint_drag = Some(PaintDrag {
            anchor: root,
            list: PaintList::Stroke,
            from: 0,
            to: 1,
            slots: Slots::default(),
        });
        app
    }

    /// **A right-click cancels a paint-row reorder.**
    ///
    /// ⚠️ **The fixture is the whole failure state and it is asserted.**
    /// `ctx.dragged_id()` must be `None` — that is what a right-click short enough
    /// to fit one frame leaves behind, and it is the only condition under which
    /// the gate's other terms had anything to say. A headless context with no
    /// widget in it is in exactly that state for free, which makes this test the
    /// *post-blink* case rather than the pre-blink one; the pre-blink case worked
    /// before the fix and is not what the finding is about.
    ///
    /// **Flip run**, the `|| self.in_flight.paint_drag.is_some()` term removed: fails on
    /// *"a paint-row reorder is not a gesture the cancel can find"*, and the
    /// control below stays green — so the flip did not simply make the gate
    /// answer yes to everything.
    #[test]
    fn a_right_click_cancels_a_paint_row_reorder() {
        let ctx = egui::Context::default();
        let mut app = app_carrying_a_paint_row(&ctx);
        assert!(
            ctx.dragged_id().is_none(),
            "the fixture: this is the state a one-frame right-click leaves, and \
             the only one where the gate's other terms matter"
        );

        assert!(
            app.cancel_gesture(&ctx),
            "a paint-row reorder is not a gesture the cancel can find"
        );
        assert!(
            app.in_flight.paint_drag.is_none(),
            "and the reorder is still in flight afterwards, so the release will \
             commit it"
        );

        // The control: with nothing in flight the cancel still declines, so the
        // new term widened the gate by exactly one gesture rather than opening it.
        let mut idle = OndinApp::headless(&ctx);
        idle.session
            .adopt_document(Document::new(IdSource::new(2).mint()), None);
        assert!(
            !idle.cancel_gesture(&ctx),
            "the control: an idle app has no gesture to cancel"
        );
    }
}

#[cfg(test)]
mod font_arrival_tests {
    //! **A preview face arriving does not re-shape a document that has never
    //! heard of its family** (§15 D591, `[A4-L4-04]`).
    //!
    //! `FontService::poll` set `fonts_registered` on any successful registration,
    //! with no notion of who had asked for the face, and `OndinApp::ui` turned
    //! that into `Resolved::invalidate_text` over the whole document. The picker
    //! fetches preview faces for as long as it is scrolled — a backlog of 48 — so
    //! scrolling it re-shaped every text node in the document, repeatedly, for
    //! faces that cannot change a glyph of it.
    //!
    //! ⚠️ **These drive the decision and the cost separately, and say so.** The
    //! `poll` end of it cannot be driven headlessly without real font bytes for a
    //! second family — `register_fonts` is what decides whether a face counts, and
    //! only Inter is bundled — so what is asserted here is the predicate
    //! `OndinApp::ui` now consults, plus the price of getting it wrong, measured
    //! through `text::shapes`.
    //!
    //! (Plain backticks per §15 D319 — `cargo doc` builds without the `test` cfg.)

    use super::OndinApp;
    use crate::fonts::FontPoll;
    use eframe::egui;
    use ondin_core::kurbo::Size;
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, TextSizing, Transaction};

    /// A headless app whose `n` text nodes are all set in `family`, plus a rect,
    /// so the walk has a non-text node to skip.
    fn app_with_text_in(ctx: &egui::Context, family: &str, n: usize) -> (OndinApp, Vec<NodeId>) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0xF0);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let parts = crate::fonts::FontService::default_text_parts();
        let mut ops = vec![Operation::CreateNode {
            id: ids.mint(),
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        }];
        let text: Vec<NodeId> = (0..n)
            .map(|i| {
                let id = ids.mint();
                let mut style = parts.style.clone();
                style.font_family = family.into();
                ops.push(Operation::CreateNode {
                    id,
                    parent: root,
                    index: i + 1,
                    kind: NodeKind::Text {
                        content: "hello world".into(),
                        style: Box::new(style),
                        spans: parts.spans.clone(),
                        para_spans: parts.para_spans.clone(),
                        paragraph: parts.paragraph.clone(),
                        block: parts.block,
                        sizing: TextSizing::Auto,
                        on_path: None,
                        on_path_flip: false,
                        on_path_offset: 0.0,
                    },
                    transform: None,
                    name: None,
                });
                id
            })
            .collect();
        doc.apply(&Transaction(ops)).expect("the fixture");
        app.session.adopt_document(doc, None);
        (app, text)
    }

    fn poll_for(family: &str) -> FontPoll {
        FontPoll {
            families_changed: false,
            fonts_registered: true,
            registered: vec![family.to_lowercase()],
        }
    }

    /// **The gate: an arriving family the document does not name is not the
    /// document's business.**
    ///
    /// ⚠️ **The `Inter` half is the one that matters and it is not symmetry.** A
    /// document naming a family that was unresolvable and has just landed *must*
    /// still invalidate — that is the case the un-narrowed code was right about,
    /// and a fix that simply stopped invalidating would leave such a document in
    /// the fallback face for the rest of the session with nothing to say so.
    ///
    /// Case is asserted too, because `document_families` lowercases and `poll`
    /// lowercases, and a fix where only one of them did would be invisible until
    /// somebody typed a family name with the wrong case.
    #[test]
    fn only_a_family_the_document_names_touches_it() {
        let ctx = egui::Context::default();
        let (app, _) = app_with_text_in(&ctx, "Inter", 2);
        let wanted = app.document_families();
        assert_eq!(
            wanted,
            ["inter".to_string()].into_iter().collect(),
            "the fixture: the document names one family, lowercased, and the rect \
             contributed nothing"
        );

        assert!(
            !poll_for("Roboto").touches(&wanted),
            "a picker preview face re-shaped a document that never names it"
        );
        assert!(
            poll_for("Inter").touches(&wanted),
            "the family the document is set in must still invalidate"
        );
        assert!(
            poll_for("INTER").touches(&wanted),
            "and case is not part of a family's identity here"
        );
    }

    /// 🚨 **A family applied to a *selection* is a family the document names.**
    ///
    /// `type_family_row` writes `CharAttr::Family` into the character spans
    /// whenever the range is partial, so a document can name a family that
    /// appears in no `TextStyle::font_family` anywhere. Reading the defaults alone
    /// made the gate decline to invalidate for exactly those and that run kept
    /// drawing in the fallback face — a hole the coarse `fonts_registered` flag
    /// had covered by accident, so §15 D591's narrowing is what made it
    /// reachable.
    ///
    /// ⚠️ **The fixture asserts the node's *default* is still Inter**, or the span
    /// would be the only family there and the test would pass against a walk that
    /// had simply started reading spans *instead of* defaults.
    ///
    /// **Flip run**, the `spans.as_slice()` loop removed: fails on *"a family
    /// applied to a selection is not in the set"*.
    #[test]
    fn a_family_applied_to_a_selection_is_one_the_document_names() {
        let ctx = egui::Context::default();
        let (mut app, text) = app_with_text_in(&ctx, "Inter", 1);
        let id = text[0];

        let (style, mut spans) = match app.session.doc.get(id).map(|n| n.kind()) {
            Some(NodeKind::Text { style, spans, .. }) => ((**style).clone(), spans.clone()),
            _ => panic!("the fixture is a text node"),
        };
        spans.set(0..5, ondin_core::CharAttr::Family("Roboto".into()), &style);
        app.session
            .try_commit(Transaction(vec![Operation::SetTextSpans { id, spans }]))
            .expect("the span write");

        let wanted = app.document_families();
        assert!(
            wanted.contains("inter"),
            "the fixture: the node's own default is still Inter, so the span is \
             not the only family in the walk"
        );
        assert!(
            wanted.contains("roboto"),
            "a family applied to a selection is not in the set: {wanted:?}"
        );
        assert!(
            poll_for("Roboto").touches(&wanted),
            "so its face arriving must invalidate"
        );
    }

    /// **The cost the gate is protecting, measured.**
    ///
    /// This is what ran on every frame a preview face landed. Asserted as a shape
    /// count rather than as a duration, so it says *how much work* rather than how
    /// fast this machine is.
    ///
    /// ⚠️ **Not a flip-check of the fix** — it is the price of the branch the fix
    /// stops taking, and it passes in both builds. It is here because a reader who
    /// only saw the predicate test would have no reason to think the branch was
    /// expensive.
    #[test]
    fn invalidating_the_document_reshapes_every_text_node_in_it() {
        let ctx = egui::Context::default();
        let (mut app, text) = app_with_text_in(&ctx, "Inter", 5);
        assert_eq!(text.len(), 5);

        let at = ondin_core::text::shapes();
        app.session.fonts_changed();
        assert_eq!(
            ondin_core::text::shapes() - at,
            5,
            "invalidating the document must re-shape every text node — this is the \
             work an unrelated preview face used to cost"
        );
    }
}

#[cfg(test)]
mod close_confirmation_tests {
    //! **The most destructive modal in the app is built from the app's own modal
    //! vocabulary** (§15 D621, `[S18.1-L3-02]`).
    //!
    //! 🚨 `close_confirmation` was the one card outside it: `egui::Modal::new`
    //! and three bare `ui.button`s. `ui::focus_ring`'s own doc says *"call it
    //! once at the end of a modal's body; `settings::card` does that for every
    //! card in the app so a new one cannot forget"* — and `grep` returned exactly
    //! two production sites for it, the definition and `settings::card`. **Eight**
    //! modals go through `card`; this was the **ninth** and the only one outside,
    //! so the sentence was false and this card painted **no ring at any point in
    //! the tab order**. That is the gap §15 D382 put the ring into the modals to
    //! close.
    //!
    //! ⚠️ **This read "seven … the eighth" until `arch-scribe` counted it**, and
    //! the miss is worth the sentence: a grep for `settings::card(` finds eight
    //! and misses the editor's own Settings card, which calls `card` **unqualified
    //! from inside `settings.rs`**. `architecture.md` §9.5 had the right number
    //! the whole time. *A qualified-path grep undercounts by exactly the module
    //! that owns the function*, which is the one most likely to be the interesting
    //! caller.
    //!
    //! ⚠️ **`FieldButton::Danger` is the other half, and its own doc is about
    //! exactly this shape**: *"a confirm that read as an ordinary
    //! `FieldButton::Off` button beside Cancel — two grey buttons, one of which
    //! removes a project that has no trash to come back from."* Here the two grey
    //! buttons were *Discard* and *Cancel*.
    //!
    //! (Plain backticks per §15 D319 — `cargo doc` builds without the `test` cfg.)

    use super::OndinApp;
    use crate::theme::color;
    use eframe::egui;

    /// Every `RectShape` the last frame painted, **flattened**.
    ///
    /// 🚨 **`FullOutput::shapes` is not flat, and a first draft of both tests
    /// below missed the card because of it.** `CLAUDE.md` records that the list is
    /// already *layer*-flattened; what it does not say is that an individual entry
    /// can be a `Shape::Vec` holding more shapes — which is exactly what an
    /// `egui::Frame` paints its background and shadow as. Scanning the top level
    /// only finds the buttons and not the card they sit on.
    fn rects(
        app: &mut OndinApp,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> Vec<egui::epaint::RectShape> {
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 800.0));
        let with = egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };
        let idle = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        // ⚠️ **The events go in on the first frame and the reading comes off the
        // last**, and the run in between is not superstition: a modal is an `Area`
        // and an `Area` **fades in**, so every colour it paints is multiplied by
        // how far through the fade the pass is. A first draft read two frames in
        // and got the ring at `#29_34_51_5F` — the accent at 0.37 — which matches
        // no constant in the theme. `focus_ring`'s visibility latch survives the
        // idle frames, so the key is still spent.
        let mut out = ctx.run_ui(with, |ui| app.close_confirmation(ui.ctx()));
        for _ in 0..30 {
            out = ctx.run_ui(idle.clone(), |ui| app.close_confirmation(ui.ctx()));
        }
        fn walk(shape: &egui::Shape, into: &mut Vec<egui::epaint::RectShape>) {
            match shape {
                egui::Shape::Rect(r) => into.push(r.clone()),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, into)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        for cs in &out.shapes {
            walk(&cs.shape, &mut found);
        }
        found
    }

    /// Every stroked rect painted in the accent, which is what a focus-visible
    /// ring is and the only thing on this card that is one.
    fn accent_rings(app: &mut OndinApp, ctx: &egui::Context, events: Vec<egui::Event>) -> usize {
        rects(app, ctx, events)
            .iter()
            .filter(|r| r.stroke.color == color::ACCENT && r.stroke.width > 0.0)
            .count()
    }

    /// **One `Tab` and the ring is on screen** — which it was not, at any point
    /// in the tab order, before this.
    ///
    /// ⚠️ **The no-key frame is the control and it is the half that makes this a
    /// test about the *ring* rather than about the accent.** `focus_ring` is
    /// focus-**visible**: it paints only after a `Tab`, so a card that drew an
    /// accent outline unconditionally would pass a one-sided assertion and be a
    /// different feature.
    ///
    /// ⚠️ **Two frames before the count, because focus is last frame's.** egui
    /// resolves `memory().focused()` against widgets that already exist, and
    /// `focus_ring` reads `read_response(id)` — so the first frame after a `Tab`
    /// has the focus and not yet the rect. Pumping is the technique this project
    /// records for every interaction probe.
    ///
    /// **Flip-check, run**: routing the card back through `egui::Modal::new` —
    /// i.e. the version that shipped — leaves the control green and fails the ring
    /// assertion at **0**, the predicted site. Keeping `settings::card` but
    /// deleting the `Tab` from the input fails it the same way, which is what says
    /// the assertion is about the key and not merely about the card.
    #[test]
    fn tabbing_the_unsaved_changes_card_shows_which_button_enter_will_press() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        app.confirming_close = true;

        assert_eq!(
            accent_rings(&mut app, &ctx, Vec::new()),
            0,
            "the control: the ring is focus-*visible*, so a card nobody has tabbed \
             into wears none"
        );

        let tab = egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        assert_eq!(
            accent_rings(&mut app, &ctx, vec![tab]),
            1,
            "one Tab must say which control on the card Enter presses"
        );
    }

    /// **The card is on the app's card, not egui's** — the frame, the hairline
    /// and the backdrop weight the other seven modals have.
    ///
    /// ⚠️ **The two things asserted are the two that *discriminate*, and finding
    /// that out took the flip.** The obvious assertion — that the ground is
    /// `color::CARD` — has **no teeth**: `theme::install` puts that colour in
    /// egui's own window fill, so the version that shipped painted `#252526`
    /// too and passed. What only `settings::card_modal` gives is the **backdrop
    /// weight** (`from_black_alpha(128)` against egui's default 100, whose own
    /// doc argues that at 100 *"a light document still read as the thing in
    /// focus"*) and the card's **hairline**, `text_a(23)` at 1.0 on a 10pt
    /// radius, which egui's default frame does not draw at all.
    ///
    /// **Flip-check, run**: routing back through `egui::Modal::new` fails at the
    /// backdrop with `#00_00_00_64` — alpha 100, egui's own — the predicted site.
    #[test]
    fn the_unsaved_changes_card_is_drawn_on_the_apps_own_card() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        app.confirming_close = true;

        let painted = rects(&mut app, &ctx, Vec::new());
        let fills: Vec<egui::Color32> = painted.iter().map(|r| r.fill).collect();
        assert!(
            fills.contains(&egui::Color32::from_black_alpha(128)),
            "the backdrop is not the app's weight: {fills:?}"
        );
        assert!(
            painted.iter().any(|r| r.fill == color::CARD
                && r.stroke.color == crate::theme::color::text_a(23)
                && r.corner_radius == egui::CornerRadius::same(10)),
            "the card's own frame is missing — ground, hairline and corner: {:?}",
            painted
                .iter()
                .map(|r| (r.fill, r.stroke.color, r.corner_radius))
                .collect::<Vec<_>>()
        );
    }
}

#[cfg(test)]
mod svg_sniff_tests {
    //! What a dropped or pasted byte run is, and what the app says when it is not
    //! a picture it can draw (§15 D671, D677).
    //!
    //! The name is the older half; the module grew the refusal test because the two
    //! findings are the same sentence from two sides — *"could not read"* about a
    //! file that reads perfectly well.
    //!
    //! Plain backticks throughout, per §15 D319.

    use super::{ImageRefusal, OndinApp, looks_like_svg};

    /// **A picture Ondin will not draw is not a picture it could not read**
    /// (§15 D677, `[S9.2-L1-04]`).
    ///
    /// 🚨 `ondin_render::images::DecodeError` had **no reader in the workspace** —
    /// `ImageStore::failure` and the whole distinction were consumed nowhere
    /// outside `images.rs`'s own tests, and both callers of `insert` discarded the
    /// error with `.ok()?`. So a 70000 × 100 panorama, 34 KB of perfectly good PNG
    /// that `image` decodes without complaint, was announced as *"Could not
    /// read"*, sending its owner back to re-export a file that is not the problem.
    /// `dead_code` never fires on a `pub` item in a library crate, which is why
    /// nothing said so.
    ///
    /// ⚠️ **The negative assertion is the one that matters**, and it is the
    /// finding: the old sentence is not merely unhelpful, it is false, and a test
    /// that only checked for the new numbers would pass a message that said both.
    ///
    /// **Flip:** put `.ok().ok_or(ImageRefusal::NotAnImage)` back on
    /// `load_image_bytes`' `insert` and the first assertion fails — `NotAnImage`
    /// against `Decode(TooLarge { .. })`. Predicted correctly.
    #[test]
    fn a_picture_too_big_to_draw_says_so_rather_than_could_not_read() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);

        // Legal PNG, decodable, and past `MAX_EDGE` on one side only — the shape
        // that makes "could not read" a lie rather than a shorthand.
        let (w, h) = (70_000_u32, 100_u32);
        let mut png = Vec::new();
        image::write_buffer_with_format(
            &mut std::io::Cursor::new(&mut png),
            &vec![0u8; (w * h) as usize],
            w,
            h,
            image::ExtendedColorType::L8,
            image::ImageFormat::Png,
        )
        .expect("a uniform grey PNG");

        // `unwrap_err` would want `Debug` on `LoadedImage`, which holds the encoded
        // bytes — a `Debug` there would print a picture into a test log.
        let Err(err) = app.load_image_bytes(png, Some("png".into()), Some("panorama.png".into()))
        else {
            panic!("70000 wide is past MAX_EDGE and was accepted")
        };
        assert!(
            matches!(
                err,
                ImageRefusal::Decode(ondin_render::images::DecodeError::TooLarge { .. })
            ),
            "the refusal lost its reason: {err:?}"
        );

        let said = err.message("panorama.png");
        assert!(
            !said.contains("Could not read"),
            "the file reads perfectly well: {said:?}"
        );
        assert!(
            said.contains("70000") && said.contains("100") && said.contains("65535"),
            "the message has to say what to change and by how much: {said:?}"
        );

        // The control: a byte run that really is unreadable still gets the plain
        // sentence, so the change is about telling two failures apart rather than
        // about replacing one message with another.
        let Err(plain) = app.load_image_bytes(b"not a picture at all".to_vec(), None, None) else {
            panic!("twenty bytes of prose became a picture")
        };
        assert_eq!(plain.message("notes.txt"), "Could not read: notes.txt");
    }

    /// The two doors onto *"is this SVG"* give one answer per file (§15 D671,
    /// `[S16.3-L3-04]`).
    ///
    /// A drop asks this of the **bytes**; a paste asks `svg_in::looks_like_svg` of
    /// a clipboard **string**. They used to be different predicates, and the file
    /// they disagreed about is the ordinary one: `<!DOCTYPE svg PUBLIC …>` first
    /// was refused on a drop with *"Could not read: logo.svg"* and imported as
    /// layers on a paste of the same bytes.
    ///
    /// ⚠️ **Asserting agreement would be worthless now, so this asserts the
    /// answers.** The app's function is three lines over core's, so a test that
    /// compared the two would be comparing a function with itself; what is still
    /// this side's own is the 512-byte cap and the lossy decode, and the last two
    /// rows are those.
    ///
    /// **Flip:** put back the old body — `text.starts_with("<svg") ||
    /// (text.starts_with("<?xml") && text.contains("<svg"))` over a
    /// `str::from_utf8` that gives up — and the first assertion fails. Predicted
    /// correctly.
    ///
    /// ⚠️ **This comment spent one commit on the test above it** (§15 D677), which
    /// is `CLAUDE.md`'s nineteenth recorded instance of the insertion trap: the new
    /// test was anchored on *this* function's `fn` line — the unique string, and
    /// the wrong one — so it landed between this paragraph and the item it
    /// describes. Merged run 38 lines against the length ranking's 52-line floor,
    /// every gate green. Found by `arch-scribe`; **the neighbour grep would have
    /// caught it**, because the item below did lose its comment, and it was not run
    /// on this insertion.
    #[test]
    fn the_drop_door_and_the_paste_door_agree_about_a_file() {
        assert!(looks_like_svg(
            br#"<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "">
<svg/>"#
        ));
        assert!(looks_like_svg(
            b"<!-- Generator: Adobe Illustrator -->\n<svg/>"
        ));
        assert!(looks_like_svg(b"<?xml version=\"1.0\"?>\n<svg/>"));
        assert!(
            !looks_like_svg(b"\xef\xbb\xbf<svg/>"),
            "a BOM is still refused"
        );
        assert!(!looks_like_svg(b"\x89PNG\r\n\x1a\n"));
        assert!(!looks_like_svg(b""));

        // The cap is the adapter's own, and it truncates rather than refusing: a
        // root element past 512 bytes of comment is not recognised, and a
        // multi-byte character cut in half by the cap does not make the whole file
        // unreadable the way `from_utf8` did.
        let mut long = b"<!--".to_vec();
        long.extend(std::iter::repeat_n(b'x', 600));
        long.extend_from_slice(b"--><svg/>");
        assert!(
            !looks_like_svg(&long),
            "past the cap, and that is the cap's job"
        );
        let mut split = "<!-- é".repeat(90).into_bytes();
        split.extend_from_slice(b"--><svg/>");
        assert!(
            !looks_like_svg(&split),
            "no panic on a character cut in half"
        );
    }
}

#[cfg(test)]
mod recover_snapshot_tests {
    //! Recovering does not strand the snapshot it is replacing (§15 D681).
    //!
    //! Plain backticks throughout, per §15 D319.

    use super::*;

    /// **`recover` drops the outgoing session's snapshot, and it is safe to
    /// because the guards ran first** (`[S16.3-L2-06]`, §15 D681 → D768).
    ///
    /// 🚨 **This test asserted the *unfixed* behaviour until the ruling was made,
    /// and the inversion is the whole history.** `recover` adopted and then
    /// overwrote `recovery.key`, so the previous key's file was never named again:
    /// it stayed in `.recovery/`, was offered at every launch, and *Later* only
    /// popped it for that one — the prompt returned for ever.
    ///
    /// 🚨 **The one-line fix was written, tested, and reverted**, because on its own
    /// it makes the *reachable* case worse. The finding's route — editing the
    /// document behind the card — was closed by §15 **D464**. What is live is the
    /// queue: *Recover* can be pressed twice with no keyboard, and the second press
    /// finds `recovery.key` naming the snapshot the user recovered one press ago and
    /// has not saved. **Deleting that is worse than leaking it** — which is why the
    /// drop arrives with `may_replace_current_document` in front of it and not
    /// before.
    ///
    /// ⚠️ **So the two are one change and must not be separated.** If somebody
    /// removes the guard and keeps the drop, this test still passes — it is the
    /// *second press* that it cannot see, because the fixture sets `recovery.key`
    /// by hand and never presses anything. `a_second_recover_asks_before_it_spends
    /// _the_first` is the one that covers that, and the two are a pair.
    #[test]
    fn recovering_drops_the_outgoing_snapshot_now_that_the_guards_are_there() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);

        // A root of this test's own, so nothing touches the developer's library.
        let root = std::env::temp_dir().join(format!(
            "ondin-recover-test-{}",
            crate::library::ids::mint()
        ));
        std::fs::create_dir_all(&root).expect("a scratch root");
        app.library.root = root.clone();

        let doc = ondin_core::Document::new(ondin_core::IdSource::new(3).mint());
        let outgoing = crate::library::ids::mint();
        let offered = crate::library::ids::mint();
        let outgoing_path =
            crate::library::recovery::write(&root, &outgoing, &doc).expect("snapshot B");
        let offered_path =
            crate::library::recovery::write(&root, &offered, &doc).expect("the pending snapshot");
        app.recovery.key = Some(outgoing.clone());

        app.recover(&crate::library::recovery::Pending {
            key: offered.clone(),
            path: offered_path.clone(),
            name: "Untitled".into(),
            target: None,
            written: Some(0),
        });

        assert!(
            !outgoing_path.exists(),
            "the outgoing snapshot was stranded — it is never named again after \
             `recovery.key` is overwritten, so it would be offered at every launch \
             for ever (§15 D768)"
        );
        assert!(
            offered_path.exists(),
            "and the one being recovered from is untouched either way"
        );
        assert_eq!(
            app.recovery.key.as_deref(),
            Some(offered.as_str()),
            "the recovered snapshot's key is adopted rather than re-minted"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// **The guard is what makes the drop safe, and this is the route it guards**
    /// (§15 D768).
    ///
    /// 🚨 **The queue route is the one the one-line fix made worse**, and it needs
    /// no keyboard: `recovery_modal` prints *"N more after this one"*, so *Recover*
    /// is clickable twice. The second press finds `recovery.key` naming the snapshot
    /// recovered on the first press — unsaved, and about to be replaced in memory by
    /// `adopt_document`. Dropping it there deletes the last copy.
    ///
    /// `may_replace_current_document` is what stops that: after the first press the
    /// session is dirty with no path (a recovered snapshot has `target: None`), so
    /// the save arm cannot run and the second press has to ask. **Declining changes
    /// nothing at all** — that is what this asserts, and it is the half a test built
    /// only around the drop cannot see.
    ///
    /// ⚠️ **`confirm_discard` raises a blocking `rfd` dialog, so the decline path
    /// cannot be driven headlessly.** What is asserted instead is the state the
    /// guard reads — dirty, unfiled — which is the input that forces the ask, plus
    /// that the pending entry is still there to be answered. **Naming the limit is
    /// the point**: an assertion that the dialog appeared would be a better test and
    /// is not available, and pretending otherwise is how a test comes to be believed
    /// for more than it checks.
    ///
    /// **Flip, run:** removing the `may_replace_current_document` call from
    /// `recovery_modal`'s *Recover* arm leaves this green — it asserts state, not
    /// the call — which is exactly why the sibling test above exists and why the
    /// pair is named in both docs.
    #[test]
    fn a_second_recover_asks_before_it_spends_the_first() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let root = std::env::temp_dir().join(format!(
            "ondin-recover-queue-{}",
            crate::library::ids::mint()
        ));
        std::fs::create_dir_all(&root).expect("a scratch root");
        app.library.root = root.clone();

        let doc = ondin_core::Document::new(ondin_core::IdSource::new(3).mint());
        let first = crate::library::ids::mint();
        let second = crate::library::ids::mint();
        let first_path = crate::library::recovery::write(&root, &first, &doc).expect("snapshot 1");
        let second_path =
            crate::library::recovery::write(&root, &second, &doc).expect("snapshot 2");

        // Two waiting, which is what puts "N more after this one" on the card.
        for (key, path) in [(&first, &first_path), (&second, &second_path)] {
            app.recovery
                .pending
                .push(crate::library::recovery::Pending {
                    key: key.clone(),
                    path: path.clone(),
                    name: "Untitled".into(),
                    target: None,
                    written: Some(0),
                });
        }
        assert_eq!(app.recovery.pending.len(), 2, "the fixture is a queue");

        // The first press.
        let pending = app.recovery.pending.pop().expect("one waiting");
        app.recover(&pending);

        // 🚨 The state the second press would meet. Dirty, because `recover` marks
        // it so; unfiled, because a recovered snapshot has no `target`. That pair is
        // what `may_replace_current_document` reads, and it is the pair that makes
        // the save arm unavailable and the ask mandatory.
        assert!(
            app.session.is_dirty(),
            "a recovered document is unsaved by construction, or there would be \
             nothing for the second press to spend"
        );
        assert!(
            app.session.path.is_none(),
            "and unfiled, so `may_replace_current_document` cannot save it away \
             and must ask instead — this is the pair that makes the guard fire"
        );
        assert_eq!(
            app.recovery.key.as_deref(),
            Some(pending.key.as_str()),
            "the key names the work the second press would drop"
        );
        assert!(
            !app.recovery.pending.is_empty(),
            "and there is still a card offering that second press"
        );

        std::fs::remove_dir_all(&root).ok();
    }
}

#[cfg(test)]
mod zoom_range_tests {
    use super::OndinApp;
    use crate::view::{MAX_ZOOM, MIN_ZOOM};
    use ondin_core::kurbo::Rect;

    /// *Fit to page* on a box too small to fill the canvas stops at
    /// `MAX_ZOOM`, and on one too large stops at `MIN_ZOOM` — asserted
    /// **by the constants** (§15 D689).
    ///
    /// (Plain backticks per §15 D319 — `cargo doc` builds without the `test`
    /// cfg, so a `[link]` here is decoration no gate can validate. Caught by
    /// the diff grep at the close of the session that wrote it.)
    ///
    /// ⚠️ **Asserting `256.0` here would have been the bug rather than the
    /// test.** `zoom_to` is the one writer with a computed zoom, and it spelled
    /// the range as two bare literals because `view.rs`'s constants were
    /// private — so a test written against the numbers passes whichever of the
    /// two statements of the range it is really reading, which is precisely the
    /// thing `[S16.5-L3-06]` is about.
    ///
    /// **Flip, in two steps, because one edit cannot show it.** Setting
    /// `MAX_ZOOM` to `100.0` alone leaves this green — the fit follows the
    /// constant, which is the property being claimed. Setting it to `100.0`
    /// *and* restoring `zoom.clamp(0.02, 256.0)` at the call site fails on the
    /// first assertion with `left: 256.0, right: 100.0`, which is the fit path
    /// keeping the old ceiling while every wheel and chord zoom moved. Both
    /// halves were run.
    #[test]
    fn a_fit_cannot_leave_the_camera_outside_the_zoom_range() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);

        let mut app = OndinApp::headless(&ctx);
        app.canvas_px = (4000, 4000);

        app.zoom_to(Some(Rect::new(0.0, 0.0, 1.0, 1.0)));
        assert_eq!(
            app.session.camera.zoom, MAX_ZOOM,
            "a 1×1 box in a 4000px canvas wants 3600× and must stop at the ceiling"
        );

        app.zoom_to(Some(Rect::new(0.0, 0.0, 1.0e9, 1.0e9)));
        assert_eq!(
            app.session.camera.zoom, MIN_ZOOM,
            "a 1e9-unit box wants 3.6e-6× and must stop at the floor"
        );
    }
}

#[cfg(test)]
mod select_all_tests {
    //! **What `Ctrl+A` selects, pinned** (§15 D709, `[S16.2-L6-07]`).
    //!
    //! 🚨 **`select_all` decides three separate things and no test in the
    //! workspace reached any of them.** The finding's measurement is the whole
    //! argument: adding a `visible && !is_effectively_locked` filter to it —
    //! which changes the chord's answer for every document containing a locked
    //! or hidden layer — left **969 tests green**. `marquee_candidates` has test
    //! callers only through the marquee's own suite, which asserts the
    //! *filtered* result; the unfiltered list this consumes was asserted
    //! nowhere, and the `root_artboards` fallback arm had no test caller at all.
    //! Per the house rule, the question of a green suite is not whether it
    //! passes but what would also pass it, and the answer here was **any**
    //! implementation of `Ctrl+A`.
    //!
    //! The three decisions, one test each:
    //!
    //! 1. the candidate set — inside artboards, plus loose non-artboard layers,
    //!    and **not** the artboards themselves;
    //! 2. the empty-set fallback — a document whose frames are empty selects the
    //!    frames;
    //! 3. a **locked** layer is left alone and a **hidden** one is taken — the
    //!    two halves of one predicate answering differently, which is the thing
    //!    a reader is most likely to "tidy up" into agreement.
    //!
    //! 🚨 **Point 3 read *"locked and hidden layers are included"* until §15
    //! D746**, and both halves were then decisions by omission. The ruling
    //! settled the lock — the canvas selects a locked layer by no gesture at
    //! all, the layers panel by clicking its row — and left hiding where
    //! `context-menus.md` §5.9 put it. So the fourth test below exists because
    //! the third one's answer now has a consequence: `Ctrl+A` then `Delete` used
    //! to destroy a locked layer, and **no keyboard verb in this range re-checks
    //! the lock**, which is why the repair is at the selection door.
    //!
    //! (Plain backticks per §15 D319 — `cargo doc` builds without the `test`
    //! cfg.)

    use super::OndinApp;
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    /// A rect of `size` under `parent`, at `index`.
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
            transform: Some(Affine::translate((10.0 * index as f64, 0.0))),
            name: None,
        }]))
        .expect("a rect");
        id
    }

    /// An artboard on the root at `index`.
    fn artboard(doc: &mut Document, ids: &mut IdSource, root: NodeId, index: usize) -> NodeId {
        let id = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index,
            kind: NodeKind::Artboard {
                size: Size::new(200.0, 200.0),
            },
            transform: Some(Affine::translate((300.0 * index as f64, 0.0))),
            name: None,
        }]))
        .expect("an artboard");
        id
    }

    fn app(ctx: &egui::Context) -> (OndinApp, IdSource, Document, NodeId) {
        let app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x5A11);
        let root = ids.mint();
        let doc = Document::new(root);
        (app, ids, doc, root)
    }

    fn sorted(mut v: Vec<NodeId>) -> Vec<NodeId> {
        v.sort();
        v
    }

    /// **Everything inside artboards, plus loose non-artboard layers — and not
    /// the artboards.**
    ///
    /// ⚠️ **The negative half is the load-bearing one.** Including the artboards
    /// is the obvious wrong version, and `select_all`'s own comment gives the
    /// reason it is wrong (*"it would make a following nudge move the whole
    /// canvas"*) — so a test asserting only that the children are present would
    /// pass against exactly the implementation the comment argues against.
    ///
    /// ⚠️ **Flip, run — and the predicted site was wrong, which is worth more
    /// than the prediction.** Dropping the `!matches!(… Artboard …)` filter from
    /// `marquee_candidates` was expected to fail the second assertion, the one
    /// written *for* it. It fails the **first**: the set equality already sees
    /// the two extra ids (`left: [5a11:2, 5a11:3, …]`) and the `!contains` never
    /// runs. So the negative assertion has teeth only against a version that
    /// admits artboards **without** changing the rest of the set — which is not
    /// this mutation, and is why it is kept rather than trimmed as redundant.
    #[test]
    fn ctrl_a_takes_the_contents_of_frames_and_the_loose_layers_but_not_the_frames() {
        let ctx = egui::Context::default();
        let (mut app, mut ids, mut doc, root) = app(&ctx);

        let board_a = artboard(&mut doc, &mut ids, root, 0);
        let board_b = artboard(&mut doc, &mut ids, root, 1);
        let inside_a = rect(&mut doc, &mut ids, board_a, 0);
        let inside_b = rect(&mut doc, &mut ids, board_b, 0);
        let loose = rect(&mut doc, &mut ids, root, 2);
        app.session.adopt_document(doc, None);

        app.select_all();
        let got = sorted(app.session.selection.ids().to_vec());

        assert_eq!(
            got,
            sorted(vec![inside_a, inside_b, loose]),
            "the contents of both frames and the loose layer, and nothing else"
        );
        assert!(
            !got.contains(&board_a) && !got.contains(&board_b),
            "and never the frames themselves — a nudge afterwards would move the \
             whole canvas, which is the reason `select_all`'s own comment gives"
        );
    }

    /// **A document whose frames are empty selects the frames**, which is the
    /// fallback arm and had no test caller anywhere in the tree.
    ///
    /// ⚠️ **Flip, run:** replacing `self.root_artboards()` with `Vec::new()`
    /// leaves the selection empty and fails here; it leaves the test above green,
    /// because that fixture never reaches the fallback.
    #[test]
    fn ctrl_a_on_a_document_of_empty_frames_takes_the_frames() {
        let ctx = egui::Context::default();
        let (mut app, mut ids, mut doc, root) = app(&ctx);

        let board_a = artboard(&mut doc, &mut ids, root, 0);
        let board_b = artboard(&mut doc, &mut ids, root, 1);
        app.session.adopt_document(doc, None);

        app.select_all();
        assert_eq!(
            sorted(app.session.selection.ids().to_vec()),
            sorted(vec![board_a, board_b]),
            "with nothing inside them the frames are what there is to select"
        );
    }

    /// **A locked layer is left alone and a hidden one is taken** — one question
    /// answered two ways, on purpose, and the pair a reader is most likely to
    /// "tidy" into agreement.
    ///
    /// 🚨 **This test has been reversed on its locked half, deliberately, and the
    /// note it carried is why it could be.** It was written (§15 D709) pinning
    /// *"locked and hidden layers are selected too"* as a decision by omission,
    /// with the standing offer: *"if the maintainer wants it filtered this test
    /// is the place that says so and the place to change."* The ruling came
    /// (§15 D746) — **a locked layer is not selectable from the canvas by any
    /// gesture, only by clicking its row in the layers panel** — so the locked
    /// third of the assertion is now the opposite of what it was, and the
    /// hidden third is untouched.
    ///
    /// ⚠️ **The two halves were never one rule and this is what separates them.**
    /// `apply_marquee`'s comment has said so since §15 D321: the lock is the half
    /// that was decided, and hiding has *"a hidden-but-unlocked layer is fully
    /// editable"* written against it (`context-menus.md` §5.9). A reader who
    /// takes D746 as licence to filter `visible()` here as well is undoing that.
    #[test]
    fn ctrl_a_skips_locked_layers_and_still_takes_hidden_ones() {
        let ctx = egui::Context::default();
        let (mut app, mut ids, mut doc, root) = app(&ctx);

        let board = artboard(&mut doc, &mut ids, root, 0);
        let plain = rect(&mut doc, &mut ids, board, 0);
        let locked = rect(&mut doc, &mut ids, board, 1);
        let hidden = rect(&mut doc, &mut ids, board, 2);
        doc.apply(&Transaction(vec![
            Operation::SetLocked {
                id: locked,
                locked: true,
            },
            Operation::SetVisible {
                id: hidden,
                visible: false,
            },
        ]))
        .expect("lock one and hide one");
        app.session.adopt_document(doc, None);

        // The fixture must reach the state, or this asserts nothing at all.
        assert!(
            ondin_core::query::is_effectively_locked(&app.session.doc, locked),
            "the fixture's locked layer really is locked"
        );
        assert!(
            !app.session.doc.get(hidden).expect("still there").visible(),
            "and its hidden layer really is hidden"
        );

        app.select_all();
        let got = sorted(app.session.selection.ids().to_vec());
        assert_eq!(
            got,
            sorted(vec![plain, hidden]),
            "Ctrl+A takes the plain layer and the hidden one and leaves the locked \
             one alone (§15 D746)"
        );
        assert!(
            !got.contains(&locked),
            "the locked layer is not in the selection at all — this is the half \
             that arms Delete, and it is why the set equality above is not enough \
             on its own to say what changed"
        );
    }

    /// **The failure the ruling was taken over: `Ctrl+A` then `Delete` used to
    /// destroy a locked layer a rubber band will not even pick up** (§15 D746,
    /// `[S16.2-L2-02]`).
    ///
    /// 🚨 **The assertion order puts the loss first.** `delete_selection` is
    /// asserted on the *node still being in the document*, not on the selection
    /// or the undo depth, because the selection is what the fix changes and the
    /// node is what the user loses. A test asserting `selection.is_empty()`
    /// would pass against a version that selects the locked layer and refuses to
    /// delete it, and against a version that deletes it and clears the selection.
    ///
    /// ⚠️ **Flip, run — two mutations, two different sites, which is what says
    /// this is not the test above written twice.** `may_select_in_bulk` returning
    /// `true` fails **three** tests: this one at its survival assertion
    /// (`app.rs:16160`), the one above at its set equality, and
    /// `canvas::marquee_selectability_tests` at its own. And the narrower
    /// mutation — `select_all` asking `marquee_candidates()` for its emptiness
    /// test instead of `marquee_range()` — fails **only this one**, because it
    /// needs a document whose entire range is locked, where the filtered list is
    /// empty and the unfiltered one is not.
    ///
    /// 🚨 **That second flip is the reason this test exists, and it was red once
    /// already with the fix in.** The first cut of D746 filtered at
    /// `marquee_candidates` and tested `is_empty()` on the result, which turned
    /// "every layer here is locked" into "these frames are empty" — so `Ctrl+A`
    /// selected the **frames** and `Delete` took the locked children down with
    /// their parents. The repair at one door leaked straight through the next
    /// one, and the only thing that said so was this assertion.
    #[test]
    fn ctrl_a_then_delete_leaves_a_locked_layer_where_it_was() {
        let ctx = egui::Context::default();
        let (mut app, mut ids, mut doc, root) = app(&ctx);

        let board = artboard(&mut doc, &mut ids, root, 0);
        let locked = rect(&mut doc, &mut ids, board, 0);
        doc.apply(&Transaction(vec![Operation::SetLocked {
            id: locked,
            locked: true,
        }]))
        .expect("lock it");
        app.session.adopt_document(doc, None);

        // The fixture must reach the state, or this asserts nothing at all.
        assert!(
            ondin_core::query::is_effectively_locked(&app.session.doc, locked),
            "the fixture's layer really is locked"
        );

        app.select_all();
        app.delete_selection();

        assert!(
            app.session.doc.get(locked).is_some(),
            "the locked layer is still in the document — no keyboard verb re-checks \
             the lock, so a door that selects it has already armed Delete"
        );
    }
}

#[cfg(test)]
mod chrome_focus_write_tests {
    //! **Where `chrome_focus` is written, driven through whole frames** (§15 D821,
    //! D850, `[X1.2-L6-01]`).
    //!
    //! 🚨 **The flag's rung was tested and its write site was not.**
    //! `escape_out_of_a_chrome_field_does_not_also_clear_the_selection` sets the
    //! field by hand, and says so — so deleting the write, moving it above the
    //! `Tab` surrender D821 says it must follow, or skipping it for a whole view
    //! all passed. These drive `eframe::App::ui` itself.
    //!
    //! Plain backticks: this module is `cfg(test)` (§15 D319).
    use super::View;
    use super::library_wiring_tests::whole_frame;
    use super::undo_rewind_tests::app_with_three_anchors;

    fn tab() -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }
    }

    /// **The flag is the focus at the end of the frame, and in point editing that
    /// is after the `Tab` surrender.**
    ///
    /// The control is an ordinary editor frame: a `Tab` there is egui's, the
    /// focus ring steps into the chrome, and the flag says so. In point editing
    /// the same `Tab` is the app's — it steps an anchor — and the focus egui moved
    /// is handed back at the end of the frame, so the flag must say *nothing is
    /// focused*, or the next `Escape` is spent on a field the app has just taken
    /// the keyboard away from.
    ///
    /// ⚠️ **Flip-checks, run**: the write moved above the surrender fails at
    /// *"after the surrender"*; the write deleted fails at the control, the flag
    /// staying at the `false` it was built with.
    #[test]
    fn the_flag_is_the_focus_after_the_tab_surrender() {
        let ctx = egui::Context::default();

        let mut app = super::OndinApp::headless(&ctx);
        whole_frame(&ctx, &mut app, Vec::new());
        whole_frame(&ctx, &mut app, vec![tab()]);
        assert!(
            ctx.memory(|m| m.focused()).is_some() && app.chrome_focus,
            "control: an ordinary Tab puts the focus ring in the chrome, and the flag \
             records it"
        );

        let ctx = egui::Context::default();
        let (mut app, _) = app_with_three_anchors(&ctx);
        whole_frame(&ctx, &mut app, Vec::new());
        assert!(app.edited_path().is_some(), "the fixture is point editing");
        whole_frame(&ctx, &mut app, vec![tab()]);
        assert!(
            !app.chrome_focus,
            "in point editing the Tab is ours, and the flag is written after the \
             surrender that gives the focus back"
        );
    }

    /// **The library screen writes the flag as well** — it returns from
    /// `eframe::App::ui` above the editor's write, and until §15 D850 the flag
    /// kept the last editor frame's answer for the whole visit.
    ///
    /// ⚠️ **Two frames, because the first still sees the editor's focus**: egui
    /// drops focus from a widget that was not drawn at the *end* of the pass, which
    /// is after this write. The second frame is the steady state, and it is the one
    /// that was stale.
    ///
    /// ⚠️ **Flip-check, run**: the dashboard's write deleted fails at *"not
    /// stale"*, with the flag still `true`.
    #[test]
    fn the_library_screen_does_not_keep_the_editors_answer() {
        let ctx = egui::Context::default();
        let mut app = super::OndinApp::headless(&ctx);
        whole_frame(&ctx, &mut app, Vec::new());
        whole_frame(&ctx, &mut app, vec![tab()]);
        assert!(
            app.chrome_focus,
            "the fixture: the editor left something focused"
        );

        app.view = View::Dashboard;
        whole_frame(&ctx, &mut app, Vec::new());
        whole_frame(&ctx, &mut app, Vec::new());
        assert!(
            !app.chrome_focus,
            "on the library screen the flag is not stale — nothing there is focused"
        );
    }
}
