//! The eframe application: egui chrome hosting the Vello canvas (§9.1–9.2).
//!
//! Owns the shell — window layout, top bar, tool rail, dispatch of resolved [`Action`]s.
//! Siblings: `session`, `canvas`, `panels`, `input`, `preview`. egui types stay confined
//! to `ondin-app`, which keeps a UI-toolkit swap survivable.

use crate::canvas::CanvasRenderer;
use crate::cursor::Cursors;
use crate::fonts::FontService;
use crate::input::{self, Action, Mode, Same, ViewSwitch};
use crate::panels::{ImageCard, LayerDrag, PaintSlot, Picker};
use crate::preview::{Drag, PenState, PointSet, TextSession};
use crate::session::{EditorSession, StatusKind};
use crate::theme::{self, color, icon};
use crate::tools::Tool;
use crate::ui::{field_row, icon_button, panel_frame};
use eframe::egui;
use ondin_core::kurbo::{Point, Rect, Vec2};
use ondin_core::{
    CharSpans, Guide, GuideId, ImageEntry, ImageId, ImageSource, Node, NodeId, NodeKind, Operation,
    ParaSpans, Transaction, build,
};
use std::collections::HashSet;

/// Painted width of every inspector popover, border included; [`OndinApp::picker_lane_right`]
/// and [`OndinApp::popover_left`] reserve the lane from it.
pub(crate) const POPOVER_W: f32 = 272.0;

/// Between the inspector's cards and anything opened beside them: a popover, or a
/// fresh colour picker.
pub(crate) const POPOVER_GAP: f32 = 10.0;

/// The three faces of the typography popup. Tabs are scopes, spelled out so the user
/// sees what a change will hit when the selection is partial.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TypeTab {
    /// The tab the popup opens on.
    #[default]
    Character,
    Paragraph,
    /// The text box itself: named for `BlockStyle`, labelled "Box".
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

/// What a layer copy holds: the nodes plus everything they reference on the source
/// document, taken at copy time (the last moment it is certainly open), so a paste into
/// another document is complete. The clipboard survives opening a document on purpose.
#[derive(Clone)]
pub(crate) struct Clip {
    /// One subtree per copied layer, ids still the originals' until
    /// `insert_subtrees` remaps them.
    pub(crate) subtrees: Vec<Vec<Node>>,
    /// The vault mains those subtrees reach (`ondin_core::kit::carried`); a paste
    /// vendors them so a kit instance lands linked.
    pub(crate) kits: Vec<Vec<Node>>,
    /// The image-table entries those subtrees reference (`build::image_ids_in`),
    /// added to the target document if missing.
    pub(crate) images: Vec<(ondin_core::ImageId, ondin_core::ImageEntry)>,
    /// The variables those subtrees read and a detached twin of each reading layer
    /// (`ondin_core::variable::carried`), so a paste rebinds by name or detaches.
    pub(crate) vars: ondin_core::variable::Carried,
}

/// A cut that detached instances, kept so its paste can make it a move
/// ([`OndinApp::cut_move`]): the paste restores the cut layers under their own ids
/// and gives back every link the cut's delete took.
#[derive(Clone, Debug)]
pub(crate) struct CutMove {
    /// A page of the document at the cut, standing for "this document": a paste
    /// into one that does not have it relinks nothing.
    doc_page: NodeId,
    /// The cut layers, in the clipboard's order — the roots of its subtrees.
    roots: Vec<NodeId>,
    /// Each link the delete took.
    links: Vec<TakenLink>,
    /// The clipboard's stamp at the cut; another copy since makes this stale.
    stamp: Option<ClipStamp>,
    /// The undo depth once the cut landed: an undo below it takes the cut back, and the
    /// move with it ([`OndinApp::document_rewound`]).
    depth: usize,
    /// While an undo has taken the cut back: the session's commit count then
    /// ([`EditorSession::commits`]). A redo to `depth` with the count unchanged restores
    /// the move; a commit in between drops it.
    parked: Option<u64>,
}

/// One link a layer delete took ([`Deleted::links`]): a node outside the deleted
/// subtrees that followed a node inside them, and what the delete left it at. A cut's
/// paste gives the link back only where the node still holds what the delete left.
#[derive(Clone, Debug)]
pub(crate) struct TakenLink {
    id: NodeId,
    /// The source it followed and the swap it carried, before the delete.
    was: (NodeId, Option<NodeId>),
    /// Its link and swap as the delete's commit left them.
    left: (Option<NodeId>, Option<NodeId>),
}

/// What a layer delete did, for the message its door writes
/// ([`OndinApp::delete_layers`]).
pub(crate) struct Deleted {
    count: usize,
    guides: usize,
    /// Instance roots the delete detached.
    detached: usize,
    /// The one layer's name, when one was deleted.
    name: Option<String>,
    /// Every link the delete took.
    links: Vec<TakenLink>,
}

/// What the system clipboard turned out to be holding, when a paste asks.
///
/// Three answers rather than a `bool`: an Ondin copy this build cannot read must
/// stop the paste, or [`OndinApp::paste_text_as_layer`] would turn the payload's
/// JSON into a text layer.
enum Clipboard {
    /// [`OndinApp::clipboard`] is live (never left this window, or a foreign payload
    /// was just adopted). The layer arm can run.
    Layers,
    /// Somebody else's text, or nothing; the caller's remaining arms handle it.
    Foreign,
    /// Ours, and unreadable. Already reported; the caller must stop.
    Unreadable,
}

/// The receipt for the stand-in a copy put on the system clipboard: its length and
/// a 64-bit hash. `DefaultHasher::new` is deterministic within a process, which is
/// the receipt's whole lifetime; it is never written anywhere.
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

/// The values a gesture holds while it is running: [`Self::any`] compares against
/// `Default`, so a new field is covered by `cancel_gesture`'s gate automatically. A
/// value in flight carries its subject, so it never lands on a moved selection.
///
/// Clearing is not blanket: `cancel_gesture` drops only `grid_scrub` and `grid_paint`;
/// the other owners read `gesture_cancelled` so they can restore something.
#[derive(Default, PartialEq)]
pub(crate) struct InFlight {
    /// The typographic spans as they were when the field scrub began: what a
    /// right-click cancel puts back, and the only record of it.
    pub(crate) session_scrub: Option<(NodeId, CharSpans, ParaSpans)>,
    /// The Settings modal's nudge pair as it was when the field scrub began (the modal
    /// writes its own draft, not `RenderOverrides`). Also keeps the cancel gate armed
    /// after egui clears `ctx.dragged_id()` on any button's release.
    pub(crate) settings_scrub: Option<crate::input::NudgeStep>,
    /// A Fill or Stroke row being dragged up or down its list.
    pub(crate) paint_drag: Option<crate::panels::PaintDrag>,
    /// An Effects row being dragged up or down its card.
    pub(crate) effect_drag: Option<crate::panels::paint::EffectDrag>,
    /// The JPEG quality being scrubbed, and which spec it belongs to; `RenderOverrides`
    /// cannot preview `SetExports`, and committing every frame would make it uncancellable.
    pub(crate) export_quality_scrub: Option<(usize, f64)>,
    /// The layout grid a scrub is holding: the frames, the index, the in-flight value.
    /// Read by the panel and `canvas::draw_layout_grids`; a list of frames because the
    /// edit goes to every selected frame.
    pub(crate) grid_scrub: Option<(Vec<NodeId>, usize, ondin_core::LayoutGrid)>,
    /// The grid colour a picker is dragging (`OndinApp::preview_grid`). Apart from
    /// [`Self::grid_scrub`] because `grid_step` commits whatever sits there.
    pub(crate) grid_paint: Option<(Vec<NodeId>, usize, ondin_core::peniko::Color)>,
    /// The absolute target a guide position field is being dragged to, by axis. The
    /// field holds its own target: a per-frame step from the committed document would
    /// commit only the last (zero) step on release.
    pub(crate) guide_position_drag: Option<(ondin_core::GuideAxis, f64)>,
}

impl InFlight {
    /// Is any gesture holding a value right now? Sound under `PartialEq` (floats)
    /// because every field is an `Option` defaulting to `None`.
    pub(crate) fn any(&self) -> bool {
        *self != Self::default()
    }
}

#[cfg(test)]
mod in_flight_tests {
    //! The case list is hand-written: a new `InFlight` field needs a new case here.

    use super::*;

    /// Each field alone arms the gate.
    /// flip: `any` as an `||` chain omitting `export_quality_scrub` → fails at that case.
    #[test]
    fn every_field_on_its_own_arms_the_gate() {
        assert!(
            !InFlight::default().any(),
            "an idle app holds nothing — a gate any wider than this takes the \
             right-click that removes a gradient stop"
        );

        let id = ondin_core::IdSource::new(0x1234).mint();
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
            (
                "effect_drag",
                InFlight {
                    effect_drag: Some(crate::panels::paint::EffectDrag {
                        anchor: id,
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

    /// A `Some` with a zero, NaN or empty payload still arms the gate.
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

/// The document's artboards and their world boxes, cached against the revision they
/// were read at (the walk is a full traversal a move drag would run per node, per frame).
///
/// Keyed on `EditorSession::revision`, bumped by every commit, undo, redo and
/// `adopt_document`. A gesture in flight must not bump it: these are the committed
/// bounds the drop rule is defined against.
#[derive(Default)]
pub(crate) struct FrameIndex {
    /// The revision and page `frames` was read at, or `None` for never.
    at: Option<(u64, NodeId)>,
    /// Every artboard on the current page, depth-first in child order (paint order,
    /// which readers rely on; [`OndinApp::artboards`]).
    frames: Vec<NodeId>,
    /// Every layer that carries a canvas tag, in paint order ([`OndinApp::tagged`]).
    tagged: Vec<NodeId>,
    /// Those of them with world bounds, paired with the bounds.
    pub(crate) boxes: Vec<(NodeId, Rect)>,
    /// Whether the document has any main, linked layer or set, so a move's drop
    /// candidates ask `component::can_hold` only when a component rule could refuse.
    pub(crate) components: bool,
    /// How many times the document walk has run; test-facing.
    walks: u64,
}

/// A library file on its way in ([`OndinApp::begin_open`]): which one, the
/// worker's answer, and when it was asked for — the pulse's clock.
pub(crate) struct Opening {
    pub path: std::path::PathBuf,
    rx: std::sync::mpsc::Receiver<Result<Opened, String>>,
    pub at: std::time::Instant,
}

/// A document read for opening, with the undo history kept for exactly its bytes.
pub(crate) type Opened = (ondin_core::Document, Option<ondin_core::SavedHistory>);

/// Read `path` as a document, and the history kept for it under `root` when `keep`
/// says histories are kept.
fn read_opened(
    path: &std::path::Path,
    root: &std::path::Path,
    keep: bool,
) -> Result<Opened, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Read failed: {e}"))?;
    let doc = ondin_core::io::load(&bytes).map_err(|e| format!("Load error: {e}"))?;
    let history = keep
        .then(|| crate::library::history::for_opened(root, &doc, &bytes))
        .flatten();
    Ok((doc, history))
}

pub struct OndinApp {
    pub(crate) session: EditorSession,
    /// See [`FrameIndex`]. `RefCell` because every reader is `&self` and reading may fill it.
    pub(crate) frame_index: std::cell::RefCell<FrameIndex>,
    /// How many times `canvas::node_grab` has reached its segment walk (it allocates a
    /// `BezPath`). Test-facing.
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
    /// The world box the copied layers occupied at copy time, which *Paste here*
    /// measures its offset from (after a cut there is nothing left to look up).
    pub(crate) clipboard_from: Option<Rect>,
    /// What a cut that detached instances owes its first paste: a cut main moves.
    /// Set by [`Self::cut_selection`], taken by [`Self::paste_cut_move`], stale once the
    /// clipboard is replaced.
    pub(crate) cut_move: Option<CutMove>,
    /// A digest of the stand-in text the last in-app copy put on the system clipboard:
    /// the receipt [`Self::owns_the_clipboard`] reads. A digest, since the stand-in can
    /// be megabytes.
    pub(crate) clipboard_stamp: Option<ClipStamp>,
    /// The guides Ctrl+C put on the clipboard. Mutually exclusive with
    /// [`Self::clipboard`] by hand (every copy clears the other). The id is re-minted on paste.
    pub(crate) guide_clipboard: Option<Vec<Guide>>,
    /// The appearance *Copy properties* (`Ctrl+Alt+C`) lifted off a layer. Not exclusive
    /// with the layer clipboard (`Ctrl+V` pastes, `Ctrl+Alt+V` paints); never reaches the
    /// system clipboard.
    pub(crate) property_clipboard: Option<ondin_core::build::Properties>,
    /// Which of the app's two screens is showing, the library or the editor:
    /// `<OndinApp as eframe::App>::ui` returns early on it.
    pub(crate) view: View,
    /// The library: base folder, projects, documents. Scanned when the dashboard is
    /// entered, not per frame; held in the editor for the save root and breadcrumb.
    pub(crate) library: crate::library::state::Library,
    /// When the open document was last written, for the autosave interval (no clock thread).
    pub(crate) last_autosave: std::time::Instant,
    /// Crash recovery: this session's snapshot and any left by a previous run
    /// (`crate::library::recovery`); covers what autosave will not.
    pub(crate) recovery: crate::library::recovery::RecoveryState,
    /// The one background thread that writes whole documents (`crate::library::writer`),
    /// so snapshot and autosave cannot race for a path. `None` until the first job; with
    /// no writer nothing is queued, so a snapshot removal may go straight to the filesystem.
    pub(crate) writer: Option<crate::library::writer::Writer>,
    /// The document being renamed inline on the library screen, its buffer, and a latch
    /// for whether the field has been handed the caret: a field that requests focus every
    /// frame erases `lost_focus()` and never commits (`OndinApp::rename_field`).
    pub(crate) rename_entry: Option<(std::path::PathBuf, String, bool)>,
    /// The dashboard's own view state — which entry, which sort, which menu.
    pub(crate) dash: crate::panels::dashboard::DashboardState,
    /// Rendered whole-document pictures for the grid's cards (`crate::library::cover`),
    /// disk-cached; distinct from `thumbs`.
    pub(crate) covers: crate::library::cover::Covers,
    /// The library's Settings modal draft while it is up, separate from [`Self::settings`];
    /// Save applies it.
    pub(crate) library_settings: Option<crate::panels::dashboard::LibrarySettings>,
    /// What the last library migration left behind, with both roots so the modal can
    /// re-run it. Cleared by a clean migration or a successful retry.
    pub(crate) stranded: Option<crate::panels::dashboard::Stranded>,
    /// Whether a chrome widget held the keyboard at the end of the last frame: egui clears
    /// focus in `Memory::begin_pass`, before `input::resolve`, so [`Self::escape`] cannot
    /// ask it, and `Escape` out of a rename field would clear the selection.
    pub(crate) chrome_focus: bool,
    /// Whether the window had the OS focus last frame; its regaining it is when the
    /// machine's fonts are scanned again (`FontService::rescan_system`).
    window_focused: bool,
    /// A presentation in progress (`present.rs`). While it is, `ui` draws nothing
    /// but the frame and reads no key but its own.
    pub(crate) presenting: Option<crate::present::Presenting>,
    /// Whether an open popover's own `Escape` handler ran this frame; written by the
    /// handler, read and cleared by the key router next frame. Read instead of the open
    /// flag, because a flag whose panel was skipped would swallow `Escape` with nothing
    /// on screen.
    pub(crate) popover_heard: bool,
    /// In-progress name edit (node id + buffer); commits once, on defocus.
    pub(crate) name_edit: Option<(NodeId, String)>,
    /// In-progress description edit on a component card; commits once, on defocus.
    pub(crate) desc_edit: Option<(NodeId, String)>,
    /// System and web fonts, loaded into core on demand.
    pub(crate) fonts: FontService,
    /// The background update check and the staged update it offers a restart for.
    /// Inert unless [`Self::new`] started it.
    pub(crate) updater: crate::update::Updater,
    /// Family names rasterized in their own face, for the picker's rows.
    pub(crate) previews: crate::fonts::FontPreviews,
    /// Pictures cut down to a layer row's square, for the layers tree and the paint
    /// row's chip. Chrome-side, since `CanvasRenderer::images` is read-only.
    pub(crate) thumbs: crate::thumbs::ImageThumbs,
    /// Substring filter for the font picker.
    pub(crate) font_filter: String,
    /// In-progress pen path (world-space anchor points), or `None`.
    pub(crate) pen: Option<PenState>,
    /// Whether the pen's verbs are armed inside the node tool's edit (`P` while a
    /// path's points are the subject). A bias, not a tool switch: `tool` stays
    /// `Tool::Node` and only what a canvas press means changes (`canvas::pen_verb`).
    /// Read only through [`Self::pen_scope`], which confines it to the edited path.
    pub(crate) pen_bias: bool,
    /// Images picked but not yet placed: what a loaded cursor holds (`Tool::Image`).
    /// Placed front-first; the tool hands back to Select when it empties. Already
    /// decoded, so nothing here can fail to place.
    pub(crate) pending_images: Vec<LoadedImage>,
    /// The tool the eyedropper hands back to after a pick, `Escape` or a second `I`.
    pub(crate) dropper_return: Tool,
    /// An eyedropper click waiting for the sample under it (`crate::eyedropper`).
    pub(crate) dropper_pick: Option<crate::eyedropper::PendingPick>,
    /// Numbers each eyedropper click, so its sample key is its own (`SampleKey::pick`).
    pub(crate) dropper_picks: u64,
    /// Where the pointer is while an armed eyedropper hides it from egui
    /// (`OndinApp::gate_dropper_input`).
    pub(crate) dropper_pointer: Option<egui::Pos2>,
    /// The window as it stood when the eyedropper armed: what every pick off the
    /// canvas reads (`crate::eyedropper::Frozen`).
    pub(crate) dropper_frozen: crate::eyedropper::Frozen,
    /// A press off the canvas while armed, which egui never saw; its release is the
    /// pick (primary) or the way out (secondary).
    pub(crate) dropper_press: Option<(egui::Pos2, egui::PointerButton)>,
    /// The text node currently being edited in-canvas.
    pub(crate) text: Option<TextSession>,
    /// Points selected inside a path, for the node tool — a third kind of subject,
    /// beside the layer selection and the guide selection.
    pub(crate) points: PointSet,
    /// Layer-tree rows the user has collapsed (default: everything expanded).
    pub(crate) collapsed: HashSet<NodeId>,
    /// The first of a possible two opacity digits, and when it was typed; `None`
    /// outside the 600 ms window ([`OndinApp::opacity_digit`]).
    pub(crate) opacity_entry: Option<OpacityEntry>,
    /// A layer row being renamed in place; buffered so one rename is one undo step.
    pub(crate) renaming_layer: Option<(NodeId, String)>,
    /// The selection the layers tree last reacted to, so it notices a change made
    /// elsewhere and reveals it once.
    pub(crate) layers_synced_selection: Vec<NodeId>,
    /// Whether the tree still owes the selection a scroll into view.
    pub(crate) scroll_layers_to_selection: bool,
    /// A library file being read and loaded off the frame, its card pulsing meanwhile
    /// ([`OndinApp::begin_open`]).
    pub(crate) opening: Option<Opening>,
    /// The row a Shift-click measures its range from: the last one clicked *without*
    /// Shift, so a range can be stretched and shrunk from one end.
    pub(crate) layers_range_anchor: Option<NodeId>,
    /// Layer-name filter. `Some` while the search field is open, even when empty;
    /// closing it clears the filter, so a search can be emptied and retyped without the
    /// tree jumping back to its collapsed state.
    pub(crate) layer_filter: Option<String>,
    /// A layer row being dragged to reorder or reparent it.
    pub(crate) layer_drag: Option<LayerDrag>,
    /// The Pages section's own state (`crate::pages`): the row being renamed or
    /// dragged, and the page the frame last showed.
    pub(crate) pages: crate::pages::PagesState,
    /// The History section's own state (`crate::panels::history`).
    pub(crate) history_pane: crate::panels::history::HistoryState,
    /// Whether the panel's drag edge is lit, and since when.
    pub(crate) layers_edge: EdgeDwell,
    /// Set when the window manager asked to close and there are unsaved edits.
    pub(crate) confirming_close: bool,
    /// A set's value the user asked to delete while variants use it, until the
    /// confirmation modal is answered.
    pub(crate) deleting_value: Option<crate::panels::ValueDelete>,
    /// The selection the inspector last drew while one of its fields held the keyboard,
    /// kept until that field lets go so a click elsewhere commits what was typed
    /// ([`Self::inspector_panel`]).
    pub(crate) inspector_hold: Option<crate::session::Selection>,
    /// Canvas size in device pixels as of the last frame — zoom-to-fit needs it.
    pub(crate) canvas_px: (u32, u32),
    /// The display scale the canvas was last laid out at (`pixels_per_point`); it turns
    /// a `*_PX` allowance in points into world units (`OndinApp::points_per_world`).
    pub(crate) canvas_ppp: f32,
    /// The canvas widget's rect as of the last frame, in screen points; last frame's
    /// because [`ChromeHold`] asks before the canvas is laid out.
    pub(crate) canvas_rect: egui::Rect,
    /// Why the selection chrome is being held back while the inspector is edited.
    pub(crate) chrome_hold: ChromeHold,
    /// An inspector edit noticed since the last [`ChromeHold`] fold, and whether its
    /// gesture is a drag still in flight. Written by [`Self::edit_valve`] and
    /// [`Self::commit_edit`], taken once a frame by `fold_chrome_hold`.
    pub(crate) pending_edit: Option<bool>,
    /// How far `canvas::autopan` has scrolled the view during the gesture in flight,
    /// in world units, so a cancel can put the view back. A running total rather than a
    /// camera snapshot, so a deliberate zoom or pan mid-drag survives.
    pub(crate) autopan_total: Vec2,
    /// Inspector panels the user has collapsed, keyed by title.
    pub(crate) collapsed_panels: HashSet<&'static str>,
    /// How far each instance asked about has drifted from its main (`reset::drift`),
    /// cached against the session revision for the Component card's per-frame counts.
    pub(crate) drift_cache: (
        u64,
        std::collections::HashMap<NodeId, ondin_core::reset::Drift>,
    ),
    /// What differs among each layers-panel row's children (`OndinApp::child_drift_of`),
    /// on `drift_cache`'s revision key. Keyed by row and whether it is folded: a
    /// collapsed row counts its whole subtree, and folding changes no revision.
    pub(crate) child_drift_cache: (
        u64,
        std::collections::HashMap<(NodeId, bool), ondin_core::reset::ChildDrift>,
    ),
    /// The Component card's property readings, on `drift_cache`'s key (`panels::PropCache`).
    pub(crate) prop_cache: crate::panels::PropCache,
    /// This frame's overrides of the selected instance layers, by inspector card:
    /// title, unit count and the resetting operations. Gathered at the top of
    /// `inspector_ui`.
    pub(crate) card_overrides: Vec<(&'static str, usize, Vec<Operation>)>,
    /// This frame's field resets of each selected linked layer, which a field's own
    /// mark reads its source's value from (`OndinApp::sub_mark`).
    pub(crate) field_overrides: Vec<(NodeId, Vec<Operation>)>,
    /// Rects whose Appearance panel shows the four per-corner radius fields; view state.
    pub(crate) per_corner_radius: HashSet<NodeId>,
    /// Rects whose Appearance panel shows the corner smoothing field; view state. Never
    /// shares a rect with `per_corner_radius`: opening either closes the other.
    pub(crate) corner_smoothing_open: HashSet<NodeId>,
    /// The run of Ctrl+D duplicates in progress, if any.
    pub(crate) clones: Option<CloneChain>,
    /// The group the user has stepped into by double-clicking it. A selection scope
    /// only: a click picks inside it, a click outside clears it.
    pub(crate) entered_group: Option<NodeId>,
    /// A gesture cancelled while the button is still held, so no release path turns it
    /// into an edit. Cleared at the end of the frame once nothing is held.
    pub(crate) gesture_cancelled: bool,
    /// Every value a gesture is holding while it runs ([`InFlight`]).
    pub(crate) in_flight: InFlight,
    /// The detached colour picker, when one is open.
    pub(crate) picker: Option<Picker>,
    /// A picker the `+` on a Fill or Stroke header asked for, parked until the new row
    /// is laid out; [`Self::paint_row`] claims it next frame with the row's anchor.
    pub(crate) pending_picker: Option<(NodeId, PaintSlot)>,
    /// A paint row's hex field mid-edit, so a colour can be typed or pasted without
    /// being reparsed and recommitted on every keystroke.
    pub(crate) paint_hex: Option<(NodeId, crate::panels::PaintSlot, String)>,
    /// What the Transform card's Scale field shows, as a percentage. A verb with a live
    /// readout, not a property: scale is not stored (`Resize` writes geometry), so the
    /// field applies against the committed box and returns to 100 when the valve closes.
    pub(crate) scale_pct: f64,
    /// Paint panels already auto-collapsed for being empty. Acting only on the
    /// transition into empty leaves the user's own toggle the last word.
    pub(crate) paint_auto_collapsed: HashSet<&'static str>,
    /// The selection each self-collapsing panel last set its open state from, keyed by
    /// title (Fill and Stroke share `"Fill"`); see `OndinApp::subject_changed`. One entry
    /// per caller, because asking consumes the edge.
    pub(crate) synced_selection: std::collections::HashMap<&'static str, Vec<NodeId>>,
    /// The stroke whose options menu (dash, cap, join) is open, by index. At most one.
    pub(crate) stroke_menu: Option<usize>,
    /// The effect row whose popover is open: its layer and its index in that stack.
    /// Keyed by node too, so a selection change cannot leave it editing another layer's
    /// effect. At most one.
    pub(crate) effect_menu: Option<(NodeId, usize)>,
    /// Which tab of image editing's card is showing; a preference kept across entries.
    /// The card is open exactly while `Tool::ImageEdit` is.
    pub(crate) image_tab: ImageCard,
    /// The strokes whose sides selector row is showing. The only thing deciding whether
    /// the row shows; a collapsed row still declares its setting through
    /// [`crate::ui::FieldButton::Set`], so losing this never loses a setting.
    pub(crate) stroke_sides_open: Vec<usize>,
    /// Settings that outlive the session. Held rather than re-read, because the export
    /// panel asks every frame.
    pub(crate) prefs: crate::prefs::Prefs,
    /// The Settings modal's staged edits (`crate::settings`); `Some` is the modal open.
    pub(crate) settings: Option<crate::settings::Settings>,
    /// The export spec whose settings block is showing. One at a time: two open blocks
    /// would push the export button off the panel.
    pub(crate) export_row_open: Option<usize>,
    /// Whether the Export panel's own menu — presets, copy/paste, the folder rule — is showing.
    pub(crate) export_menu: bool,
    /// The export settings *Copy export settings* is holding; apart from
    /// [`Self::property_clipboard`], so neither paste is lost to the other.
    pub(crate) export_clipboard: Option<Vec<ondin_core::ExportSpec>>,
    /// The Export panel's rendered preview and what it was built from; one slot.
    pub(crate) export_preview: Option<crate::panels::ExportPreview>,
    /// The export prefix being typed, and which spec it belongs to; buffered so typing
    /// one name is one undo step.
    pub(crate) export_prefix_text: Option<(usize, String)>,
    /// The export suffix being typed; its own buffer, since both fields are on screen.
    pub(crate) export_suffix_text: Option<(usize, String)>,
    /// The custom dash list being typed, buffered verbatim: typing "5, 10" passes
    /// through `5`, a different real pattern, and each keystroke would be an undo step.
    pub(crate) stroke_dash_text: Option<(usize, String)>,
    /// Which tab of the typography popup is showing, or `None` when closed; one field,
    /// so reopening lands where the user left off.
    pub(crate) type_menu: Option<TypeTab>,
    /// The OpenType feature filter being typed in the Character tab, if any.
    pub(crate) feature_filter: String,
    /// Whether the Character tab's OpenType list also shows the features a user should
    /// not be choosing (required, per-script, vertical). Per session, not document state.
    pub(crate) show_all_features: bool,
    /// A character colour's hex field mid-edit; keyed by slot because the text colour
    /// and a decoration's show at once.
    pub(crate) char_hex: Option<(crate::panels::CharSlot, String)>,
    /// A multi-selection Transform field being dragged: which one, and the absolute
    /// value it has been dragged to. `multi_transform_tx` builds from the committed
    /// document every frame, so a per-frame step would commit only the last (zero) step
    /// on release.
    pub(crate) multi_transform_drag: Option<(crate::panels::MultiField, f64)>,
    /// The drawn canvas cursors, rasterized once (`cursor.rs`).
    pub(crate) cursors: Cursors,
    /// Whether the rulers are showing (Shift+R). View state; on by default.
    pub(crate) show_rulers: bool,
    /// Whether the guides are showing — *View ▸ Show guides*, `Ctrl+;`. Every reader goes
    /// through [`OndinApp::guides_on`] so hit test and paint agree.
    pub(crate) show_guides: bool,
    /// Whether the guides are locked — *View ▸ Lock guides*, `Ctrl+Alt+;`. A workspace
    /// switch rather than per guide; locked means not selectable at all
    /// ([`OndinApp::guide_at`]). Off by default.
    pub(crate) lock_guides: bool,
    /// The guides being edited from the inspector, as whole guides: the preview half of
    /// the valve for state the renderer never sees ([`OndinApp::shown_guide`]). A list,
    /// because an edit reaches every selected guide.
    pub(crate) guide_previews: Vec<Guide>,
    /// The colour a picker drag is previewing for a variable's cell, and which cell:
    /// [`Self::guide_previews`]' twin for the table. Dropped by the picker whenever no
    /// button is held, so it cannot outlive its gesture.
    pub(crate) var_cell_preview: Option<(
        ondin_core::VarId,
        ondin_core::ModeId,
        ondin_core::peniko::Color,
    )>,
    /// Which of the top bar's dropdowns is open, if any; one field so two menus
    /// can never overlap.
    pub(crate) open_menu: TopMenu,
    /// What is typed in the zoom dropdown's field while it has focus; `None` otherwise,
    /// when the field shows the live zoom.
    zoom_entry: Option<String>,
    /// The floating insert picker, while it is open.
    pub(crate) kit_picker: Option<crate::kits::picker::Picker>,
    /// The revision the bound families were last loaded at: a bind, a string cell, a new
    /// variable or a paste can name a family not yet requested.
    bound_fonts_at: Option<u64>,
    /// `families::families_in_use` at a revision, for the *Missing fonts* card,
    /// which would otherwise walk the document every frame.
    pub(crate) font_uses: Option<(u64, ondin_core::families::FamilyUses)>,
    /// A picker placement waiting one painted frame; here, not on the picker, so closing
    /// it in the wait keeps the click.
    pub(crate) kit_placing: Option<crate::kits::picker::Placing>,
    /// The library's kits' published states and tile pictures, loaded off the frame
    /// (`kits::index`).
    pub(crate) kit_shelf: crate::kits::index::KitShelf,
    /// The update popover's summary, kept against the document revision and waiting kits'
    /// versions it was read at (a dry run, too dear for every frame). `None` inside when
    /// the dry run failed.
    pub(crate) kit_summary: Option<(
        crate::kits::summary::Key,
        Option<crate::kits::summary::Summary>,
    )>,
    /// When the kits this document uses were last looked at on disk (`kits::updates`).
    pub(crate) kit_watch: crate::kits::updates::KitWatch,
    /// The *Stop sharing* dialog's count of the documents using the kit (`kits::users`).
    pub(crate) kit_users: Option<crate::kits::users::KitUsers>,
    /// Where the rail's insert button was drawn last frame; the picker hangs beside it.
    /// `NOTHING` until the rail has drawn.
    pub(crate) kit_button: egui::Rect,
    /// Where the rail's Pen button was drawn last frame; the path bar centres on it.
    pub(crate) pen_button: egui::Rect,
    /// The path bar's Offset card, while it is open (`crate::path_bar`).
    pub(crate) offset_card: Option<crate::path_bar::OffsetCard>,
    /// The path bar's *Simplify*, while its worker runs.
    pub(crate) simplifying: Option<crate::path_bar::Simplifying>,
    /// Exports rendering and writing on workers (`panels::export`).
    pub(crate) exports: Vec<crate::job::Job<crate::panels::Delivered>>,
    /// The window's context, for work that must wake the frame loop from a worker.
    pub(crate) egui_ctx: egui::Context,
    /// The styles and variables browser; it shares a place with the insert picker.
    pub(crate) var_browser: Option<crate::panels::variables::Browser>,
    /// The browser's collapsed groups per document root; view state, not saved.
    pub(crate) var_collapsed: std::collections::HashMap<NodeId, HashSet<String>>,
    /// Where the rail's swatches button was drawn last frame. `NOTHING` until the rail
    /// has drawn.
    pub(crate) var_button: egui::Rect,
    /// The `--` list, while a bindable field is typing one or the field menu opened it.
    pub(crate) var_complete: Option<crate::panels::binding::Complete>,
    /// A bound field's chip being typed over.
    pub(crate) var_retype: Option<crate::panels::binding::Retype>,
    /// A bindable field's right-click menu: the field, the point, and the field's rect
    /// the `--` list would hang under (`context-menus.md` §6.7).
    pub(crate) field_menu: Option<(crate::panels::binding::FieldKey, egui::Pos2, egui::Rect)>,
    /// A frame's *Mode* menu, open under its row.
    pub(crate) mode_menu: Option<(NodeId, egui::Rect)>,
    /// A style slot card's `⋯` menu.
    pub(crate) style_menu: Option<crate::panels::styles::StyleMenu>,
    /// *Apply style…*'s picker.
    pub(crate) style_picker: Option<crate::panels::styles::StylePicker>,
    /// A style just created, its name being typed in the capsule.
    pub(crate) style_naming: Option<crate::panels::styles::StyleNaming>,
    /// The style slots whose capsule is opened over its greyed rows — per slot, not per layer.
    pub(crate) style_open: HashSet<ondin_core::style::StyleSlot>,
    /// The publish dialog, while it is open.
    pub(crate) publishing: Option<crate::kits::publish::PublishDraft>,
    /// This kit's changes since its last publish, with the session revision and mark
    /// version they were counted at (`OndinApp::kit_changes`).
    pub(crate) kit_changes: Option<(u64, u32, ondin_core::kit::Changes)>,
    /// The vendored main *Make local*'s confirmation is asking about.
    pub(crate) making_local: Option<NodeId>,
    /// The document *Go to main in the kit* came from, and its name — the kit's top bar
    /// links back to it until the user leaves the kit.
    pub(crate) kit_return: Option<(std::path::PathBuf, String)>,
    /// *Kits used…*, while it is open.
    pub(crate) kits_used_open: bool,
    /// *Version history…*, while it is open (`versions.rs`).
    pub(crate) versions_open: bool,
    /// The kit *Make all local*'s confirmation is asking about.
    pub(crate) making_all_local: Option<String>,
    /// *Stop sharing as kit*'s confirmation: `Some(None)` asks about the open document,
    /// `Some(Some(path))` about a library file from its card.
    pub(crate) unsharing_kit: Option<Option<std::path::PathBuf>>,
    /// The one open context menu (`docs/context-menus.md` §0 R4); one slot, so opening
    /// the next closes the last.
    pub(crate) context_menu: Option<ContextMenu>,
    /// The open command palette (`crate::palette`).
    pub(crate) palette: Option<crate::palette::Palette>,
    /// The open shortcut cheatsheet (`crate::cheatsheet`); a modal on either screen.
    pub(crate) cheatsheet: Option<crate::cheatsheet::Cheatsheet>,
    /// A card a palette row asked to show: the inspector opens it and scrolls to it
    /// on the next frame it draws it, then clears this.
    pub(crate) reveal_card: Option<&'static str>,
    /// The inspector cards drawn this frame, by title, and the ones drawn last
    /// frame, which the palette offers to show.
    pub(crate) cards_drawn: Vec<&'static str>,
    pub(crate) cards_shown: Vec<&'static str>,
    /// Every card title drawn this session, so a card the selection does not show
    /// can still be offered, dim.
    pub(crate) cards_seen: std::collections::BTreeSet<&'static str>,
    /// Where the secondary press that may open a menu landed (§0 R1): egui has forgotten
    /// the press by the time `secondary_clicked` fires on release.
    pub(crate) secondary_press: Option<egui::Pos2>,
    /// The pixel grid's own switch — *View ▸ Show grid*. Necessary, not sufficient: the
    /// grid also needs enough magnification (`grid::VISIBLE_AT_ZOOM`).
    pub(crate) show_grid: bool,
    /// The layout grids' workspace switch — *View ▸ Show layout grid*: the outermost of
    /// three gates (with each grid's eye and `shown_visible`). Nothing snaps to a layout
    /// grid, so hiding the bands cannot change where anything lands. Per session.
    pub(crate) show_layout_grids: bool,
    /// *View ▸ Outline mode*: the artwork as hairlines (`RenderOverrides::set_outline`,
    /// installed by `draw_canvas`). Not a preference: a launch into it would look
    /// like a broken document.
    pub(crate) outline_view: bool,
    /// *View ▸ Show layers* and *Show toolbar*: whether the left pane and the tool rail
    /// are on screen.
    pub(crate) show_layers: bool,
    /// *View ▸ Show history*: the History section at the layers pane's foot.
    pub(crate) show_history: bool,
    pub(crate) show_toolbar: bool,
    /// *View ▸ Show messages*: whether the floating status bar reports what just
    /// happened. With it off, a message is still written and read by nothing.
    pub(crate) show_status: bool,
    /// *View ▸ Focus mode*: the panels hidden while the canvas and its overlays keep
    /// working. Escape comes back. Overrides `Rulers`, `Layers` and `Toolbar` without
    /// changing them, so leaving restores the user's workspace.
    pub(crate) focus: bool,
    /// Whether the transform-origin marker is editable — the Transform panel's crosshair.
    /// Sticky and app-wide; the pivots themselves are per node and saved. Escape turns it
    /// off after cancelling any gesture ([`Self::escape`]).
    pub(crate) show_pivot: bool,
    /// The four *Snap* menu toggles, read only by [`OndinApp::snapping`].
    pub(crate) snap_shapes: bool,
    pub(crate) snap_guides: bool,
    pub(crate) snap_grid: bool,
    /// Text baselines: its own switch, since every line of copy in view is a candidate
    /// and that is far more magnetism than the other three. On by default.
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
    /// The document menu — the caret beside the document's name.
    Document,
    /// The kit update notice's popover.
    KitUpdates,
}

/// The system clipboard's text, if it holds any; `None` for an empty one.
///
/// Read directly, because `Event::Paste` only arrives on `Ctrl+V` and a context-menu
/// row is offered from app code ([`ContextMenu::system_text`]).
pub(crate) fn system_clipboard_text() -> Option<String> {
    if !clipboard_is_reachable() {
        return None;
    }
    let text = with_clipboard(|c| c.get_text().ok())??;
    (!text.is_empty()).then_some(text)
}

/// Every `arboard` handle this crate opens is opened under this lock: two threads
/// opening the Windows clipboard at once corrupt the heap.
///
/// `egui-winit` keeps its own handle outside the lock, safe only because both run on
/// the one UI thread; a read moved onto a worker is not covered. Poisoning is ignored.
fn with_clipboard<T>(f: impl FnOnce(&mut arboard::Clipboard) -> T) -> Option<T> {
    static GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _held = GATE.lock().unwrap_or_else(|e| e.into_inner());
    let mut clipboard = arboard::Clipboard::new().ok()?;
    Some(f(&mut clipboard))
}

/// Whether the OS clipboard is off limits — set once by `OndinApp::headless`, never
/// cleared, so no headless test can touch the developer's real clipboard.
static CLIPBOARD_OFF: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether this process may open the OS clipboard at all. Checked by the callers, not
/// inside [`with_clipboard`], which `clipboard_gate_tests` calls directly.
fn clipboard_is_reachable() -> bool {
    !CLIPBOARD_OFF.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether the system clipboard holds a picture. A `bool` rather than the image, since
/// asking decodes it; [`OndinApp::paste_image`] reads it again if the row is taken.
pub(crate) fn system_clipboard_has_image() -> bool {
    clipboard_is_reachable() && with_clipboard(|c| c.get_image().is_ok()).unwrap_or(false)
}

/// One open context menu (`docs/context-menus.md` §0).
pub(crate) struct ContextMenu {
    /// The **press** position (R1), in screen points.
    pub at: egui::Pos2,
    pub target: crate::menu::Target,
    /// The world point the menu was opened over, where there is one: where *Paste here*
    /// pastes and *Edit text* puts the caret.
    pub world: Option<Point>,
    /// True on the frame this menu was opened. That frame is exempt from the click-away
    /// test, or the release that opens the menu would dismiss it.
    pub just_opened: bool,
    /// Whether the system clipboard held text when this menu opened, for the *Paste*
    /// rows ([`OndinApp::paste`]). A snapshot, so an open menu does not open an OS
    /// clipboard handle every frame.
    pub system_text: bool,
    /// Whether the system clipboard held a picture when this menu opened
    /// ([`system_clipboard_has_image`]); a snapshot, since asking decodes a bitmap.
    pub system_image: bool,
    /// Whether [`OndinApp::clipboard`] / [`OndinApp::guide_clipboard`] still describe the
    /// system clipboard ([`OndinApp::owns_the_clipboard`]), so a *Paste* row never offers
    /// what its chord would decline.
    pub payload_current: bool,
    /// Whether the layer this menu opened on is text with contours to convert
    /// (`build::can_outline_text`). A snapshot, because asking builds the outlines.
    pub text_outlineable: bool,
    /// The row `↑`/`↓` have walked to, if the keyboard is driving this menu
    /// (`docs/context-menus.md` §8). An item rather than an index, as the menu is
    /// rebuilt each frame ([`crate::menu::navigate`]).
    pub highlight: Option<crate::menu::Item>,
    /// The *Move to page* row's rect while its list of pages is open beside it.
    pub flyout: Option<egui::Rect>,
}

/// How long the selection chrome stays away after the last keystroke, or after a
/// scrub lets go ([`ChromeHold`]).
const CHROME_HOLD_SECS: f64 = 1.5;

/// Whether the selection chrome is held back because an inspector value is being
/// edited, and until when. A term in `canvas::chrome_hidden`.
///
/// Two clocks: a scrub holds for its whole duration and starts the count on release;
/// a keystroke restarts the count each time.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum ChromeHold {
    /// Nothing is being edited; the chrome is the selection's own business.
    #[default]
    Free,
    /// A scrub is in flight; held until the button comes up, so no deadline of its own.
    Scrubbing,
    /// Held until this moment on egui's own clock (`InputState::time`).
    Until(f64),
}

/// How long the pointer rests on the layers panel's drag edge before it lights, so a
/// pointer merely crossing the 7pt band does not light it.
const EDGE_DWELL: f64 = 0.2;

/// Whether the layers panel's drag edge is lit, and since when (seconds on
/// `InputState::time`). The delay is on the highlight only, not egui's resize cursor.
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
    /// One frame of the machine. `in_band`: the pointer is in the edge's grab band.
    /// `pressed`: the primary press this frame (the edge, not `down`, which an unrelated
    /// drag also has). `down` holds a lit edge after a drag leaves the band and blocks
    /// the dwell, so a gesture already in flight never lights it.
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

    /// When the highlight falls due, so the frame can be woken (a resting pointer
    /// produces no events).
    pub fn due(self) -> Option<f64> {
        match self {
            Self::Waiting(t) => Some(t + EDGE_DWELL),
            _ => None,
        }
    }
}

/// How long a first opacity digit waits for a second one (`docs/shortcuts.md` §2).
const OPACITY_DIGIT_WINDOW: f64 = 0.6;

/// A first opacity digit waiting to find out whether it is the whole value (seconds on
/// `InputState::time`).
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct OpacityEntry {
    /// `0`–`9` as typed: the digit, not the value it resolves to.
    first: u8,
    at: f64,
    /// The layers the digit was typed against, which the deadline commit lands on.
    subject: Vec<NodeId>,
}

/// What a typed digit resolves to, and whether the window is still open.
#[derive(Clone, PartialEq, Debug)]
enum OpacityStep {
    /// Preview this percentage and hold the entry for a possible second digit.
    Wait(f64, OpacityEntry),
    /// Commit this percentage — a second digit shut the window.
    Land(f64),
}

/// The percentage a lone digit means: `1`–`9` are 10%–90%, and `0` is 100%
/// (0% is `0`,`0`).
fn opacity_of_digit(digit: u8) -> f64 {
    if digit == 0 {
        100.0
    } else {
        f64::from(digit) * 10.0
    }
}

/// Whether `entry` is still waiting for a second digit: the one reading of the boundary
/// for [`opacity_step`] and [`OndinApp::fold_opacity_entry`], which must agree.
fn opacity_window_open(entry: &OpacityEntry, now: f64) -> bool {
    now - entry.at < OPACITY_DIGIT_WINDOW
}

/// Resolve a digit against whatever digit is already pending. Pure; the clock is passed in.
fn opacity_step(
    pending: Option<OpacityEntry>,
    digit: u8,
    now: f64,
    subject: &[NodeId],
) -> OpacityStep {
    match pending {
        // The subject must match, or digits typed against two layers pair into one.
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

/// Whether `tx`, offered by a control with response `resp`, is an inspector edit worth
/// hiding the selection chrome for — and if so, whether it is a drag still in flight.
///
/// Both halves are needed: `changes_ink` says the edit is visible, the response says an
/// edit happened this frame (a live control's transaction is built every frame). A
/// `None` response comes from [`OndinApp::commit_edit`], reached only on a change.
fn edit_note(resp: Option<&egui::Response>, tx: &Transaction) -> Option<bool> {
    if !tx.changes_ink() {
        return None;
    }
    match resp {
        // Dragged first: a frame that both drags and changes is a scrub.
        Some(r) if r.dragged() => Some(true),
        // `changed()` leaves arrow keys, focus and `lost_focus()` alone.
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

    /// One frame of the machine; `OndinApp::fold_chrome_hold` gathers the inputs.
    /// `edit` is the note a value field left (`Some(true)` a drag in flight,
    /// `Some(false)` a keystroke). `pointer_down` ends a scrub, since the field cannot
    /// report the release. `moved_on_canvas` brings the chrome back early, never out of
    /// a scrub, since a long drag crosses the canvas.
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
            // The release: the count starts here, not where the drag did.
            Self::Scrubbing if !pointer_down => Self::Until(now + CHROME_HOLD_SECS),
            Self::Scrubbing => Self::Scrubbing,
            Self::Until(deadline) if moved_on_canvas || now >= deadline => Self::Free,
            other => other,
        }
    }
}

impl OndinApp {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Option<std::path::PathBuf>) -> Self {
        let render_state = cc
            .wgpu_render_state
            .clone()
            .expect("ondin requires the wgpu backend");
        // A lost device says why only here: wgpu's error sink drops `DeviceLost`,
        // and every later resource is silently invalid.
        render_state
            .device
            .set_device_lost_callback(|reason, message| {
                log::error!(target: "ondin", "GPU device lost ({reason:?}): {message}");
            });
        crate::theme::install(&cc.egui_ctx);
        // Read before the font service: `Prefs::web_fonts` decides whether the
        // catalog thread is spawned at all.
        let prefs = crate::prefs::Prefs::load();
        let mut app = Self::with(
            &cc.egui_ctx,
            CanvasRenderer::new(render_state),
            FontService::new(&cc.egui_ctx, prefs.web_fonts),
            prefs,
        );
        app.updater = crate::update::Updater::start(&cc.egui_ctx);
        app.open_at_launch(open.as_deref());
        app
    }

    /// Say, once, in the status line, that a record on disk could not be read.
    /// [`crate::prefs::Prefs::unreadable`], [`crate::library::cache::LocalIndex::unreadable`]
    /// and [`crate::library::state::Library::projects_unreadable`] each refuse to be
    /// overwritten and look like an empty state; without a message the user would make a
    /// new library beside the real one.
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

    /// Decide what the window shows on the first frame: `open` when the command line
    /// named a document, else the library, unless `reopen_last` names a document that
    /// still loads.
    ///
    /// The reopen deliberately skips `open_path`, which rewrites `last_document`; a
    /// named document goes through it, so it is remembered and a failure is reported.
    /// Nothing is open yet for its autosave, question or recovery drop to touch.
    fn open_at_launch(&mut self, open: Option<&std::path::Path>) {
        self.view = View::Dashboard;
        self.report_unreadable_records();
        // Enforced at launch to reach users who reopen straight into a document.
        crate::library::store::purge_trash(&self.library.root, self.prefs.trash_keep_days);
        // `Library::open` counted the trash before the sweep.
        self.library.refresh_trash();
        // Collected before the reopen; a later frame's modal asks over whatever opens.
        self.recovery.pending =
            crate::library::recovery::pending(&self.library.root, &self.library.entries);
        if let Some(path) = open {
            self.open_path(path);
            return;
        }
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
        self.reopen_last_page();
        if self.prefs.collapse_groups_on_open {
            self.collapse_all();
        }
        self.view = View::Editor;
    }

    /// An `OndinApp` with no GPU, font threads, preferences file or OS clipboard. The
    /// caller supplies the `Context` so a probe drives input and reads output through it.
    #[cfg(test)]
    pub(crate) fn headless(ctx: &egui::Context) -> Self {
        crate::theme::install(ctx);
        // A base folder that does not exist: the default would read the real library.
        let prefs = crate::prefs::Prefs {
            base_folder: Some(std::env::temp_dir().join("ondin-headless-no-library")),
            // Never written out, or the temp root would become the real base folder.
            ephemeral: true,
            ..Default::default()
        };
        CLIPBOARD_OFF.store(true, std::sync::atomic::Ordering::Relaxed);
        Self::with(ctx, CanvasRenderer::headless(), FontService::inert(), prefs)
    }

    /// The one field list, so the two constructors cannot drift.
    fn with(
        ctx: &egui::Context,
        canvas: CanvasRenderer,
        fonts: FontService,
        prefs: crate::prefs::Prefs,
    ) -> Self {
        let root = crate::library::root(prefs.base_folder.as_deref())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let library = crate::library::state::Library::open(root);
        let prefs_page = prefs.dashboard_page.clone();
        let prefs_sort = prefs.dashboard_sort.clone();
        let view = prefs.view;
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
            cut_move: None,
            clipboard_stamp: None,
            name_edit: None,
            desc_edit: None,
            fonts,
            updater: Default::default(),
            previews: crate::fonts::FontPreviews::default(),
            thumbs: crate::thumbs::ImageThumbs::default(),
            font_filter: String::new(),
            pen: None,
            pen_bias: false,
            pending_images: Vec::new(),
            dropper_return: Tool::Select,
            dropper_pick: None,
            dropper_picks: 0,
            dropper_pointer: None,
            dropper_frozen: Default::default(),
            dropper_press: None,
            text: None,
            points: PointSet::default(),
            collapsed: HashSet::new(),
            opacity_entry: None,
            renaming_layer: None,
            pages: Default::default(),
            history_pane: Default::default(),
            layers_synced_selection: Vec::new(),
            scroll_layers_to_selection: false,
            opening: None,
            layers_range_anchor: None,
            layer_filter: None,
            layer_drag: None,
            layers_edge: EdgeDwell::default(),
            confirming_close: false,
            deleting_value: None,
            inspector_hold: None,
            canvas_px: (1, 1),
            canvas_ppp: 1.0,
            canvas_rect: egui::Rect::NOTHING,
            chrome_hold: ChromeHold::default(),
            pending_edit: None,
            autopan_total: Vec2::ZERO,
            // Preview starts closed because opening it renders and encodes the artwork.
            collapsed_panels: HashSet::from(["Effects", "Preview", "Frame templates"]),
            // `u64::MAX` is no revision a session has, so the first read fills it.
            drift_cache: (u64::MAX, Default::default()),
            child_drift_cache: (u64::MAX, Default::default()),
            prop_cache: Default::default(),
            card_overrides: Vec::new(),
            field_overrides: Vec::new(),
            per_corner_radius: HashSet::new(),
            corner_smoothing_open: HashSet::new(),
            open_menu: TopMenu::None,
            zoom_entry: None,
            kit_picker: None,
            bound_fonts_at: None,
            font_uses: None,
            kit_placing: None,
            kit_shelf: Default::default(),
            kit_summary: None,
            kit_watch: Default::default(),
            kit_users: None,
            kit_button: egui::Rect::NOTHING,
            pen_button: egui::Rect::NOTHING,
            offset_card: None,
            simplifying: None,
            exports: Vec::new(),
            egui_ctx: ctx.clone(),
            var_browser: None,
            var_collapsed: Default::default(),
            var_button: egui::Rect::NOTHING,
            var_complete: None,
            var_retype: None,
            field_menu: None,
            mode_menu: None,
            style_menu: None,
            style_picker: None,
            style_naming: None,
            style_open: HashSet::new(),
            publishing: None,
            kit_changes: None,
            making_local: None,
            kit_return: None,
            kits_used_open: false,
            versions_open: false,
            making_all_local: None,
            unsharing_kit: None,
            context_menu: None,
            palette: None,
            cheatsheet: None,
            reveal_card: None,
            cards_drawn: Vec::new(),
            cards_shown: Vec::new(),
            cards_seen: std::collections::BTreeSet::new(),
            secondary_press: None,
            show_grid: view.grid,
            show_layout_grids: view.layout_grids,
            outline_view: false,
            show_layers: view.layers,
            show_history: view.history,
            show_toolbar: view.toolbar,
            show_status: view.status,
            focus: false,
            // Off: a marker on every selection would be permanent chrome.
            show_pivot: false,
            snap_shapes: view.snap_shapes,
            snap_guides: view.snap_guides,
            snap_grid: view.snap_grid,
            snap_baselines: view.snap_baselines,
            clones: None,
            entered_group: None,
            gesture_cancelled: false,
            in_flight: InFlight::default(),
            picker: None,
            pending_picker: None,
            paint_hex: None,
            scale_pct: 100.0,
            paint_auto_collapsed: HashSet::new(),
            synced_selection: Default::default(),
            stroke_menu: None,
            effect_menu: None,
            image_tab: ImageCard::default(),
            stroke_sides_open: Vec::new(),
            prefs,
            settings: None,
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
            show_rulers: view.rulers,
            show_guides: view.guides,
            lock_guides: view.lock_guides,
            guide_previews: Vec::new(),
            var_cell_preview: None,
            guide_clipboard: None,
            property_clipboard: None,
            // `OndinApp::open_at_launch` overrides it; headless tests start here.
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
            // Starts focused: the launch scan is already under way.
            window_focused: true,
            presenting: None,
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
    /// Wait for the snapshot writer before the process goes, on every exit path that
    /// skips [`Self::handle_close_request`]'s own settle. This is the primary flush and
    /// `library::writer::Writer`'s `Drop` the backstop: eframe may exit without dropping
    /// the app (`process::exit`, macOS `Cmd-Q`) but reaches `on_exit` on all paths.
    ///
    /// [`Self::handle_close_request`]: OndinApp::handle_close_request
    fn on_exit(&mut self) {
        self.disk_settle();
        // After the settle: Velopack's updater stops this process regardless.
        self.updater.on_exit();
    }

    /// Off: eframe's persistence is on for the window's geometry only, and a
    /// restored egui `Memory` would bring back last session's popups and scroll.
    fn persist_egui_memory(&self) -> bool {
        false
    }

    /// An armed eyedropper keeps the panels from seeing the pointer
    /// (`OndinApp::gate_dropper_input`).
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.gate_dropper_input(ctx, raw_input);
    }

    /// The frame body: everything the app draws and decides, once per pass.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // First: this frame's status writes join one another, not the last frame's.
        self.session.next_status_pass();
        self.session.forget_step_name();
        self.history_pane.hover.clear();
        self.remember_view_switches();

        // Before any card: the editor's centre on the canvas, as the palette does.
        let canvas = (self.view != View::Dashboard && self.canvas_rect.is_positive())
            .then(|| self.canvas_rect.center());
        crate::settings::set_card_centre(&ctx, canvas);

        // Above the view branch: a launch can land on either screen.
        self.recovery_modal(&ctx);

        // Above the view branch too: the library screen can hold a dirty session.
        self.handle_close_request(&ctx);
        self.close_confirmation(&ctx);
        self.value_delete_confirmation(&ctx);
        self.page_delete_confirmation(&ctx);

        // Both screens are the same window, and the updater reports to both bars.
        crate::chrome::resize_zones(&ctx, crate::chrome::Chrome::current());
        self.updater.poll();

        // Before the dashboard's early return, so a font installed while the library
        // was up is found; the scan lands at the editor's font poll.
        let focused = ui.ctx().input(|i| i.focused);
        if focused && !self.window_focused {
            self.fonts.rescan_system(ui.ctx());
        }
        self.window_focused = focused;

        // The dashboard returns early: no keyboard routing, no panels, no font poll,
        // `disk_results` or ticks (`go_to_dashboard` compensates one). A library file
        // read off the frame lands here, before the screen it switches to is drawn.
        self.poll_open(ui.ctx());
        if self.view == View::Dashboard {
            // The palette is the editor's; one left open must not greet the next file.
            self.palette = None;
            // So is present mode, and the fullscreen it took.
            self.stop_presenting(ui.ctx());
            self.dashboard_ui(ui);
            self.cheatsheet_ui(ui.ctx());
            // Last of the library's frame, after every card (the editor's twin is at
            // the end of this function).
            crate::chrome::above_modal(ui.ctx(), crate::chrome::Chrome::current());
            // Written here too, or the first editor frame would read a stale value.
            self.chrome_focus = ui.ctx().memory(|m| m.focused()).is_some();
            return;
        }

        // Merge background font results; only font data invalidates the shaped-text
        // cache. A bound family is requested when the document changes, since a
        // variable can name a family the open-time walk missed.
        if self.bound_fonts_at != Some(self.session.revision()) {
            self.bound_fonts_at = Some(self.session.revision());
            let strings = self
                .session
                .doc
                .variables()
                .iter()
                .any(|v| v.kind == ondin_core::VarKind::String);
            if strings {
                self.ensure_document_fonts();
            }
        }
        let fonts = self.fonts.poll();
        if fonts.fonts_registered {
            // Re-shape only when the document names a landed family: the picker
            // fetches preview faces continuously while scrolled.
            if fonts.touches(&self.document_families()) {
                self.session.fonts_changed();
                // The picker's pictures were laid out in the fallback too.
                if let Some(p) = self.kit_picker.as_mut() {
                    p.fonts_changed();
                }
            }
            self.kit_shelf
                .fonts_changed(|families| fonts.touches(families));
            // Not narrowed: a preview miss waits for exactly this face.
            self.previews.retry_misses();
        }
        if fonts.families_changed {
            // A document opened before the font scan landed could not resolve its families.
            self.ensure_document_fonts();
            self.ensure_default_font();
        }
        for family in self.fonts.take_evicted() {
            self.previews.forget(&family);
        }
        if fonts.any() {
            ctx.request_repaint();
        }

        // Present mode draws its frame and reads its own keys, nothing else. A modal
        // (closing the window asks one) ends it: a modal holds every key.
        if self.presenting.is_some() && self.modal_is_up() {
            self.stop_presenting(&ctx);
        }
        if self.presenting.is_some() {
            self.disk_results();
            self.autosave_tick(&ctx);
            self.recovery_tick(&ctx);
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| self.present_ui(ui));
            return;
        }

        // A pointer press ends any run of coalescing edits: a click is a new intent.
        if ctx.input(|i| i.pointer.any_pressed()) {
            self.session.end_edit_run();
        }
        // An undo or redo that followed its step to another page, last frame.
        self.notice_page_change();
        self.leave_a_scope_the_selection_left();

        // Whether this frame's `Tab` was ours (point editing or a live text session),
        // read before the panels change focus and acted on at the end of the frame.
        let took_tab = (self.edited_path().is_some() || self.text.is_some())
            && !ctx.egui_wants_keyboard_input()
            && ctx.input(|i| i.key_pressed(egui::Key::Tab));

        // Keyboard first, only what the mode admits and no chrome widget owns (see
        // `input::resolve`). An open context menu holds the keyboard
        // (`docs/context-menus.md` §0 R3): Escape closes only the menu, and its
        // navigation keys travel to it as a value.
        let mut nav = None;
        let mut palette_keys = crate::palette::Keys::default();
        let popover_heard = std::mem::take(&mut self.popover_heard);
        if self.palette.is_some() && !self.modal_is_up() {
            // The palette owns the keyboard: its keys leave `Events` before its
            // field can read them, and nothing reaches `input::resolve`.
            palette_keys = crate::palette::read_keys(&ctx);
        } else if self.context_menu.is_some() {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.context_menu = None;
            } else {
                nav = crate::menu::read_nav(&ctx);
            }
        } else if self.modal_is_up() {
            // A modal owns every key; `input::resolve` is no second guard, since a modal
            // of buttons does not make egui want the keyboard.
        } else if popover_heard
            && self.a_popover_owns_escape()
            && ctx.input(|i| i.key_pressed(egui::Key::Escape))
        {
            // Escape is spent on a popover heard last frame; other keys pass.
        } else {
            // A popover flag nobody heard is dead: clear it, and let the press pay out.
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

        // Dropped files bypass the `egui::Modal` backdrop, so a modal blocks them here.
        if !self.modal_is_up() {
            self.take_dropped_images(&ctx);
        }

        // Right-click abandons whatever is in flight, before the UI runs so this
        // frame's widgets know; a strict no-op when nothing is, which leaves the
        // right-click that removes a gradient stop alone.
        if ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary)) {
            self.secondary_press = ctx.pointer_interact_pos();
            // A right-click calls off an armed eyedropper and is spent on it.
            if self.disarm_dropper() {
                self.gesture_cancelled = true;
            }
            self.cancel_gesture(&ctx);
        }

        // Before anything reads the state the background writer changes.
        self.disk_results();
        // After this frame's edits, before the panels draw the save dot.
        self.autosave_tick(&ctx);
        // After `disk_results`, so a landed save drops the snapshot before a new one.
        self.recovery_tick(&ctx);

        // Focus mode overrides the chrome switches without changing them.
        if !self.focus {
            self.top_bar(ui);
            if self.show_layers {
                self.layers_panel(ui);
            }
            if self.show_toolbar {
                self.tool_rail(ui);
            }
        } else {
            // The top bar is the title bar too; this brings the window's buttons
            // back on hover at the top edge.
            crate::chrome::focus_strip(&ctx, crate::chrome::Chrome::current());
        }

        // Before the canvas, which reads the answer; the note was left by the
        // previous frame's inspector.
        self.fold_chrome_hold(&ctx);
        self.fold_opacity_entry(&ctx);

        // The canvas takes the remaining area; the inspector and picker float over it.
        let canvas = egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(color::BG))
            .show(ui, |ui| {
                self.canvas_ui(ui);
            })
            .response
            .rect;
        self.status_bar(&ctx, canvas);

        self.watch_kits(&ctx);
        self.poll_simplify();
        self.poll_exports(&ctx);
        if !self.focus {
            self.inspector_panel(&ctx);
            self.path_bar(&ctx);
            self.picker_ui(&ctx);
            self.kit_picker_ui(&ctx);
            self.var_browser_ui(&ctx);
            self.field_menu_ui(&ctx);
            self.var_complete_ui(&ctx);
            self.mode_menu_ui(&ctx);
            self.style_popups_ui(&ctx);
        }
        // Outside the `!focus` guard: shortcuts and the canvas menu open the kit
        // dialogs in focus mode, and every flag `modal_is_up` reads must be drawn.
        self.publish_dialog(&ctx);
        self.make_local_dialog(&ctx);
        self.kits_used_dialog(&ctx);
        self.version_history_dialog(&ctx);
        self.stop_sharing_dialog(&ctx);
        // After every door, so a right-click that replaced the open menu has already
        // written the slot (R4); unguarded, a context menu opens in focus mode.
        self.context_menu_ui(&ctx, nav);
        // Unguarded: its chord works in focus mode, over the canvas alone.
        self.palette_ui(&ctx, canvas, palette_keys);
        // Last, and unguarded: `Ctrl+,` and the palette open it in focus mode too.
        self.settings_ui(&ctx);
        self.cheatsheet_ui(&ctx);

        // A button click's commit requests no repaint of its own.
        if self.pending_edit.is_some() {
            ctx.request_repaint();
        }

        // End of frame, because the release is the event the suppression swallows.
        if ctx.input(|i| !i.pointer.any_down()) {
            self.gesture_cancelled = false;
            // Here rather than on `drag_stopped`, which the session-scoped valves see
            // only on a frame the field changed.
            self.in_flight.session_scrub = None;
            self.in_flight.settings_scrub = None;
        }

        // Hand focus back from egui's focus ring after a `Tab` that was ours, or the
        // next `Tab` would be dropped as chrome-focused. Not consumable earlier: egui
        // moves focus while the panels are built.
        if took_tab && let Some(focused) = ctx.memory(|m| m.focused()) {
            ctx.memory_mut(|m| m.surrender_focus(focused));
        }

        // Last, once every card and the bar are laid: a modal otherwise leaves no way
        // to move, resize or close the window.
        crate::chrome::above_modal(&ctx, crate::chrome::Chrome::current());

        // After the `Tab` surrender, or the next `Escape` would think a field had the keyboard.
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

    /// `Ctrl+Shift+Z`; moves the document under state that indexes into it, like undo.
    pub(crate) fn redo(&mut self) {
        if self.session.redo() {
            self.document_rewound();
        }
    }

    /// A History row's click: [`EditorSession::jump_to`] the step with this serial
    /// (`None`: the document as opened), with what an undo drops.
    ///
    /// The step is found after the text session finishes, since that commit drops the
    /// redo rows and adds one; a row it dropped jumps nowhere.
    pub(crate) fn jump_history(&mut self, serial: Option<u64>) {
        self.finish_text_first();
        let history = &self.session.history;
        let depth = match serial {
            None => Some(0),
            Some(s) => history
                .done()
                .chain(history.undone())
                .position(|v| v.serial == s)
                .map(|i| i + 1),
        };
        if let Some(depth) = depth
            && self.session.jump_to(depth)
        {
            self.document_rewound();
        }
    }

    /// The document just moved under us (undo or redo). Drop the app-side state that
    /// names parts of it by index, and park or drop a cut's move (a claim about one point
    /// in the history).
    ///
    /// - [`PointSet`] holds `(subpath, anchor)` indices that can stay in range and name
    ///   a different anchor, so only clearing is safe.
    /// - [`PenState`] snapshots the subpaths; finishing would resurrect undone geometry.
    /// - [`OndinApp::cut_move`] is parked when undone below its `depth`, restored by a
    ///   redo with no commit between, dropped once a commit branches.
    fn document_rewound(&mut self) {
        self.points.clear();
        self.pen = None;
        // A picker on a slot a style has just landed on closes, or its next write would
        // cut the style again (unless the style itself is being edited).
        if self.session.style_edit.is_none()
            && let Some(p) = &self.picker
        {
            use ondin_core::style::StyleSlot;
            let slot = match p.slot {
                PaintSlot::Fill(_) => Some(StyleSlot::Fills),
                PaintSlot::Stroke(_) => Some(StyleSlot::Strokes),
                PaintSlot::Effect(_) => Some(StyleSlot::Effects),
                _ => None,
            };
            let styled = slot.is_some_and(|s| {
                self.session
                    .doc
                    .get(p.node)
                    .is_some_and(|n| n.styles().get(s).is_some())
            });
            if styled {
                self.picker = None;
            }
        }
        // A commit while parked branched the history; the cut a redo would replay is gone.
        let (depth, commits) = (self.session.history.undo_depth(), self.session.commits());
        let mut branched = false;
        if let Some(c) = self.cut_move.as_mut() {
            match c.parked {
                Some(at) if at != commits => branched = true,
                Some(_) if depth >= c.depth => c.parked = None,
                None if depth < c.depth => c.parked = Some(commits),
                _ => {}
            }
        }
        if branched {
            self.cut_move = None;
        }
    }

    pub(crate) fn dispatch(&mut self, ctx: &egui::Context, action: Action) {
        // A menu row that forwards here has already named its step, more particularly.
        if !self.session.has_step_name()
            && let Some((label, glyph)) = crate::menu::Item::step_name(action)
        {
            self.session.name_step(label, glyph);
        }
        match action {
            // The arms that act on the whole document finish a live text session first,
            // or its buffer and `session.doc` disagree (Save would pin a version without
            // the typed text).
            Action::Undo => {
                self.finish_text_first();
                self.undo();
            }
            Action::Redo => {
                self.finish_text_first();
                self.redo();
            }
            // Pins: autosave already writes the file (`input::Action::Save`).
            Action::Save => {
                self.finish_text_first();
                self.save_file(true);
            }
            // `go_to_dashboard` finishes the text session itself.
            Action::Open => self.go_to_dashboard(),
            // The dashboard's own functions, which carry their guards.
            Action::NewDocument => {
                let project = self.open_document_project();
                self.new_library_document(project);
            }
            Action::CloseDocument => self.go_to_dashboard(),
            // The top bar's door; it cannot arrive with the modal already up.
            Action::OpenSettings => self.toggle_settings(ctx),
            Action::Palette => self.toggle_palette(ctx),
            Action::Cheatsheet => self.open_cheatsheet(crate::cheatsheet::Opened::Editor),
            // No dialog once a destination is known this session.
            Action::ExportAll => self.export_all(crate::panels::ExportAll::Asking),
            Action::Present => self.start_presenting(ctx),
            Action::ExportCode(target) => self.export_code(target),
            Action::PlaceImage => self.place_image_action(),
            Action::InsertPicker => self.toggle_kit_picker(),
            Action::VariablesBrowser => self.toggle_var_browser(),
            Action::PublishKit => self.open_publish(),
            // The Document menu's own guards, so a palette row asks the same questions.
            Action::MakeKit if self.shared_kit().is_none() => self.make_kit(),
            Action::KitsUsed if self.uses_kits() => self.kits_used_open = true,
            Action::StopSharingKit if self.shared_kit().is_some() => {
                self.unsharing_kit = Some(None);
            }
            Action::MakeKit | Action::KitsUsed | Action::StopSharingKit => {}
            Action::ShowInLibrary => self.go_to_dashboard(),
            Action::VersionHistory => self.versions_open = true,
            Action::RotateQuarter(clockwise) => {
                let turn = std::f64::consts::FRAC_PI_2;
                self.rotate_selection_quarter(if clockwise { turn } else { -turn });
            }
            Action::Copy => {
                self.copy_selection(ctx);
            }
            Action::Cut => self.cut_selection(ctx),
            Action::Paste => self.paste(),
            Action::PasteInPlace => self.paste_in_place(),
            Action::PasteToReplace => self.paste_to_replace(),
            // `egui_winit` sends `Event::Paste` exactly when the clipboard has text,
            // so only a text-less clipboard needs this release to paste.
            Action::PasteRelease => {
                if system_clipboard_text().is_none() {
                    self.paste();
                }
            }
            Action::CopyProperties => self.copy_properties(),
            Action::PasteProperties => self.paste_properties(),
            Action::Duplicate => self.duplicate_selection(),
            // While the pen is drawing, Backspace takes back the last anchor; intercepted
            // here because `input::resolve` knows nothing of a gesture in flight.
            Action::Delete if self.pen.is_some() => self.pen_backspace(),
            // With points selected, Delete removes the points, healing the path.
            Action::Delete if !self.points.is_empty() && self.edited_path().is_some() => {
                self.delete_points()
            }
            Action::Delete => self.delete_selection(),
            Action::SelectAll => self.select_all(),
            Action::SelectSame(same) => self.select_same(same),
            Action::SelectInverse => self.select_inverse(),
            // The pen reads Enter itself in `pen_input`.
            Action::Enter if self.pen.is_some() => {}
            Action::Enter => self.enter_action(),
            // The Escape that closes a top-bar dropdown closes only it (an open dropdown
            // focuses nothing, so egui's keyboard guard cannot stand in).
            Action::Escape if self.open_menu != TopMenu::None => {
                self.open_menu = TopMenu::None;
            }
            Action::Escape => self.escape(ctx),
            Action::Group => self.group_selection(),
            Action::Mask => self.toggle_mask(),
            Action::FrameSelection => self.frame_selection(),
            Action::AddLayout => self.add_layout(),
            Action::CreateComponent => self.create_component(),
            Action::DetachInstance => self.detach_instances(),
            Action::Boolean(op) => self.apply_boolean(op),
            Action::Flatten => self.flatten_selection(),
            Action::Ungroup => self.ungroup_selection(),
            // While a shape is drawn the arrows belong to it (`canvas::reshape_create`);
            // with points selected they move the points.
            Action::Nudge(_) if matches!(self.drag, Drag::Create { .. }) => {}
            Action::Nudge(delta) if !self.points.is_empty() && self.edited_path().is_some() => {
                self.nudge_points(delta)
            }
            Action::Nudge(delta) => self.nudge(delta),
            // Dropped mid-create, where the pointer owns the size.
            Action::SizeStep(_) if matches!(self.drag, Drag::Create { .. }) => {}
            Action::SizeStep(d) => self.size_step(d),
            Action::StepPoint(forward) => self.step_point(forward),
            Action::Restack(mv) => self.restack(mv),
            // Reads the layer selection when no points are picked.
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
            Action::StepPage(next) => self.step_page(if next { 1 } else { -1 }),
            Action::ZoomSelection => self.zoom_to(self.selection_bounds()),
            Action::ToggleView(sw) => self.set_view_switch(sw, !self.view_switch(sw)),
            Action::ToggleHidden => self.toggle_hidden(),
            Action::ToggleLocked => self.toggle_locked(),
            Action::Rename => self.rename_selection(),
            Action::Flip(axis) => self.flip(axis),
            Action::Align(axis, edge) => self.align(axis, edge),
            Action::Distribute(axis) => self.distribute(axis),
            Action::TidyUp => self.tidy_up(),
            Action::OpacityDigit(d) => self.opacity_digit(ctx, d),
            Action::TextStyle(chord) => self.text_chord(chord),
        }
        ctx.request_repaint();
    }

    /// Where a whole-selection switch moves, given each member's state in the sense of
    /// the verb (*hidden*, *locked*): everything moves to applied unless everything
    /// already is, so a mixed selection hides, and locks.
    fn set_switch(applied: impl IntoIterator<Item = bool>) -> bool {
        !applied.into_iter().all(|a| a)
    }

    /// [`Self::set_switch`] for hiding: takes the members' `visible` and returns the
    /// `visible` they all move to. The negation lives here so callers pass
    /// `doc.shown(id)` unchanged.
    fn hide_switch(visible: impl IntoIterator<Item = bool>) -> bool {
        !Self::set_switch(visible.into_iter().map(|v| !v))
    }

    /// [`Self::set_switch`] for locking: takes the members' `locked` and returns the
    /// `locked` they all move to. No inversion; it exists so the two chords are spelled alike.
    fn lock_switch(locked: impl IntoIterator<Item = bool>) -> bool {
        Self::set_switch(locked)
    }

    /// The `visible` [`Self::toggle_hidden`] moves the selection's outermost layers to
    /// (a bound visibility resolved), and so whether the menu's row reads *Show*.
    pub(crate) fn hide_switch_shows(&self) -> bool {
        let doc = &self.session.doc;
        Self::hide_switch(
            build::outermost(doc, self.session.selection.ids())
                .into_iter()
                .filter(|id| doc.get(*id).is_some())
                .map(|id| doc.shown(id)),
        )
    }

    /// The `locked` [`Self::toggle_locked`] moves the selection's outermost
    /// layers to, and so whether the menu's row reads *Lock*.
    pub(crate) fn lock_switch_locks(&self) -> bool {
        let doc = &self.session.doc;
        Self::lock_switch(
            build::outermost(doc, self.session.selection.ids())
                .into_iter()
                .filter_map(|id| doc.get(id))
                .map(|n| n.locked()),
        )
    }

    /// Hide or show the whole selection — `Ctrl+Shift+H` (`docs/shortcuts.md` §4).
    /// One switch for the set ([`Self::set_switch`]). `session.commit` rather than
    /// [`Self::commit_edit`], whose only addition is the inspector's chrome hide.
    fn toggle_hidden(&mut self) {
        let ids = build::outermost(&self.session.doc, self.session.selection.ids());
        let visible = self.hide_switch_shows();
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
        let locked = self.lock_switch_locks();
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
    /// The same field a double-click in the layers panel opens (`renaming_layer`, drawn
    /// by `layers::layer_rename_field`). The panel is shown if hidden; with several
    /// layers selected it renames nothing and says so.
    fn rename_selection(&mut self) {
        // Focus mode draws no layers panel: the field would never render.
        if self.focus {
            self.session.info("Leave focus mode to rename");
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
        if self.prefs.layers_folded {
            self.prefs.layers_folded = false;
            self.prefs.save();
        }
        self.renaming_layer = Some((*id, name));
        // Focus is requested by `layer_rename_field` on its first draw: focusing an id
        // with no AccessKit node yet panics `accesskit_consumer`.
    }

    /// Apply a digit to the selection's opacity — `docs/shortcuts.md` §2.
    ///
    /// One digit is a coarse value, two within 600 ms an exact one (`4` is 40%, `4`,`5`
    /// is 45%, `0`,`0` is 0%). The first digit only previews, so `4`,`5` leaves one undo
    /// step; the window commits when it shuts ([`Self::fold_opacity_entry`]).
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

    /// Close the two-digit window once it has run out, committing the coarse value. Asks
    /// for a repaint at the deadline, since nothing else wakes the loop.
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
        self.session.name_step("Opacity", icon::DROP_HALF);
        // `entry.subject`, not the live selection, which may have moved.
        self.set_opacity_pct_on(&entry.subject, opacity_of_digit(entry.first), true);
    }

    /// Preview or commit one opacity value across `ids`, with the same transaction
    /// the inspector's opacity field builds (`build::set_opacity_all`).
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

    /// Abandon whatever gesture is in flight and leave no trace: the preview is dropped
    /// and nothing is committed. Returns whether there was anything to abandon.
    ///
    /// Covers a canvas drag and an inspector field scrub alike. It sets
    /// `gesture_cancelled` because the press is still down and its release would
    /// otherwise commit the edit after all.
    pub(crate) fn cancel_gesture(&mut self, ctx: &egui::Context) -> bool {
        // `dragged_id` blinks (egui clears it on any button's release), so the gesture
        // preview and `InFlight` are read too; both are absent when nothing is in
        // flight, which keeps this a no-op for the right-click that removes a gradient stop.
        let field = ctx.dragged_id().is_some()
            || self.session.has_gesture_preview()
            || self.in_flight.any()
            // A component carried out of the insert picker: its drag is egui's too.
            || self.kit_picker.as_ref().is_some_and(|p| p.dragging());
        if self.drag.is_none() && self.layer_drag.is_none() && !field {
            return false;
        }
        // A live marquee has been selecting as it went; put back what it started from.
        if let Drag::Marquee { base, .. } = std::mem::replace(&mut self.drag, Drag::None) {
            self.session.selection = *base;
        }
        // Guide and variable-cell previews live outside `RenderOverrides`.
        self.guide_previews.clear();
        self.var_cell_preview = None;
        self.layer_drag = None;
        self.in_flight.paint_drag = None;
        self.in_flight.effect_drag = None;
        // `gesture_cancelled` stops the release placing it.
        if let Some(p) = self.kit_picker.as_mut() {
            p.cancel_drag();
        }
        // A stale one would be what the next Alt-drag silently inserts.
        self.alt_clone = None;
        // Only autopan's own scrolling is undone.
        self.session.camera.center -= std::mem::take(&mut self.autopan_total);
        // Not `clear_preview`, which would also hide a live text session's uncommitted content.
        self.session.clear_gesture_preview();
        // A scrub that wrote into the session is wound back by hand.
        if let Some((id, spans, para_spans)) = self.in_flight.session_scrub.take()
            && self.text.as_ref().is_some_and(|s| s.id == id)
        {
            self.restyle_session_with(spans, para_spans);
        }
        // And the Settings modal's nudge pair.
        if let Some(nudge) = self.in_flight.settings_scrub.take()
            && let Some(settings) = &mut self.settings
        {
            settings.set_nudge(nudge);
        }
        // Dropped here, not by the panel: the canvas draws them before the panels run.
        self.in_flight.grid_scrub = None;
        self.in_flight.grid_paint = None;
        // A pending opacity digit is a gesture too, or its deadline would land it.
        self.opacity_entry = None;
        self.chrome_hold = ChromeHold::Free;
        self.gesture_cancelled = true;
        ctx.stop_dragging();
        true
    }

    /// Escape unwinds whatever is in flight, one level per press, most transient first:
    /// floating panels, an editing session, a gesture, the pen bias, points, the entered
    /// group, the tool (back to Select), then the selection.
    ///
    /// An in-flight pen path is committed, not discarded. It happens here rather than in
    /// `pen_input`, because keyboard actions dispatch before the canvas runs.
    pub(crate) fn escape(&mut self, ctx: &egui::Context) {
        // Focus mode first: its own way out is hidden by the state itself.
        if self.focus {
            self.focus = false;
            self.session.info("Left focus mode");
        } else if self.field_menu.take().is_some()
            || self.var_complete.take().is_some()
            || self.mode_menu.take().is_some()
        {
            // They float over everything, and the list's `Escape` restores its preview.
            self.var_retype = None;
            self.session.clear_gesture_preview();
        } else if self.escape_styles() {
            // A style card's menu, its picker, a new style's name, a style edit — one a press.
        } else if self.disarm_dropper() {
            // Above the picker it may be picking for.
        } else if self.picker.take().is_some() {
        } else if self.kit_picker.take().is_some() {
            // A rung of its own so one press closes one thing.
        } else if let Some(b) = self.var_browser.as_mut() {
            // A cell being typed into or an open menu goes first, the browser next press.
            if !b.escape() {
                self.var_browser = None;
            }
        } else if self.text.is_some() {
            self.finish_text_edit();
        } else if self.pen.is_some() {
            self.finish_pen(false);
        } else if self.cancel_gesture(ctx) {
        } else if self.chrome_focus {
            // A chrome field surrendered focus to this `Escape`. Below `cancel_gesture`,
            // which a valved field's cancel must still reach to set `gesture_cancelled`.
        } else if self.offset_card.is_some() {
            // The path bar's card, above the pen bias and the points it was opened over.
            self.offset_card = None;
        } else if self.pen_bias {
            // The bias, above the points it was armed over.
            self.pen_bias = false;
            self.session.info("Pen off; still editing points");
        } else if !self.points.is_empty() {
            self.points.clear();
        } else if self.show_pivot {
            // Puts the pivot handle away; placed pivots stay.
            self.show_pivot = false;
        } else if let Some(group) = {
            self.leave_a_scope_the_selection_left();
            self.entered_group.take()
        } {
            // Step back out of the group and select it.
            self.session.selection.set_one(group);
        } else if self.tool != Tool::Select {
            self.choose_tool(Tool::Select);
        } else {
            self.session.selection.clear();
        }
    }

    /// Clear [`Self::entered_group`] when something is selected and none of it is within
    /// the group. Run at the top of every editor frame and by `Escape`, so one check
    /// covers every door that moves the selection. An empty selection keeps the scope.
    pub(crate) fn leave_a_scope_the_selection_left(&mut self) {
        let Some(g) = self.entered_group else { return };
        let doc = &self.session.doc;
        let ids = self.session.selection.ids();
        if !doc.contains(g)
            || (!ids.is_empty() && !ids.iter().any(|id| ondin_core::is_within(doc, *id, g)))
        {
            self.entered_group = None;
        }
    }

    /// Step one level into a group or a boolean, selecting something inside it. `at` is
    /// the menu's pointer, which picks what to land on; without one, the topmost child.
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

    /// Enter: step into the selection, or step back out. A toggle where Escape is a
    /// ladder; into an instance, text, picture, path or group in the double-click's order.
    fn enter_action(&mut self) {
        if self.tool == Tool::Node {
            self.choose_tool(Tool::Select); // clears the points itself
            return;
        }
        // The crop that was made stays made; leaving is about the mode.
        if self.tool == Tool::ImageEdit {
            self.choose_tool(Tool::Select);
            return;
        }
        let [id] = *self.session.selection.ids() else {
            return;
        };
        // An instance steps in first, whatever its kind, as the double-click does.
        if ondin_core::component::instance_root(&self.session.doc, id) == Some(id)
            && self
                .session
                .doc
                .get(id)
                .is_some_and(|n| !n.children().is_empty())
        {
            self.enter_container(id, None);
            return;
        }
        // `double_click_pick`'s order, so the two gestures agree.
        if matches!(
            self.session.doc.get(id).map(|n| n.kind()),
            Some(ondin_core::NodeKind::Text { .. })
        ) {
            // No pointer, so the whole string is selected (`begin_edit_text`).
            self.begin_edit_text(Some(id), None);
            return;
        }
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
            // A lock refuses here: nothing in `tools/` re-checks it once armed.
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
        // A group is a depth, not a mode: `Enter` goes deeper and `Escape` comes out.
        if matches!(
            self.session.doc.get(id).map(|n| n.kind()),
            Some(ondin_core::NodeKind::Group | ondin_core::NodeKind::Boolean { .. })
        ) {
            self.enter_container(id, None);
        }
    }

    /// Switch tool, abandoning whatever the old one held. Asking for the pen from inside
    /// a path edit arms the bias instead; the standalone pen is Escape then `P`.
    pub(crate) fn choose_tool(&mut self, tool: Tool) {
        // `I` toggles the eyedropper, back to the tool it was armed from.
        let tool = match tool {
            Tool::Eyedropper if self.tool == Tool::Eyedropper => self.dropper_return,
            Tool::Eyedropper => {
                self.dropper_return = self.tool;
                if let Some(p) = &mut self.picker {
                    p.picking = false;
                    p.preview = None;
                }
                tool
            }
            _ => tool,
        };
        self.dropper_pick = None;
        self.finish_text_first();
        // Abandons the drag in flight, its Alt copy and its preview — not `clear_preview`,
        // which would take a live text session's content too.
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
        // The bias belongs to one edit, so it cannot outlive the tool that holds it.
        self.pen_bias = false;
        if tool != Tool::Node {
            self.points.clear();
        }
        if tool != Tool::Image {
            self.pending_images.clear();
        }
        // A tool that creates a layer (`drags_a_box`) drops the selection, whose chrome
        // would sit over the new drawing.
        if tool.drags_a_box() {
            self.session.selection.clear();
        }
        self.tool = tool;
    }

    pub(crate) fn select_all(&mut self) {
        // Everything inside artboards plus loose layers, replacing a guide selection.
        // The frames fallback asks the unfiltered range: frames whose contents are all
        // locked must select nothing, or `Delete` would take the frames with them.
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

    /// The layer *Select same* compares against: the key layer, else a lone selected
    /// one, when it has the property `same` names — else the sentence the palette
    /// dims the row with.
    pub(crate) fn select_same_source(&self, same: Same) -> Result<NodeId, &'static str> {
        let sel = &self.session.selection;
        let Some(id) = sel.key().or_else(|| sel.single()) else {
            return Err(if sel.is_empty() {
                crate::menu::NEEDS_SELECTION
            } else {
                "Make one of these the key layer to match it"
            });
        };
        let doc = &self.session.doc;
        let node = doc.get(id).ok_or(crate::menu::NEEDS_SELECTION)?;
        let has = match same {
            Same::Fill => !node.paint().fills.is_empty(),
            Same::Stroke => !node.paint().strokes.is_empty(),
            Same::Font => !text_families(doc, id).is_empty(),
            Same::Effect => !node.effects().is_empty(),
        };
        if has {
            return Ok(id);
        }
        Err(match same {
            Same::Fill => "This layer has no fill",
            Same::Stroke => "This layer has no stroke",
            Same::Font => "This is not a text layer",
            Same::Effect => "This layer has no effects",
        })
    }

    /// Replace the selection with every layer on the current page sharing the source
    /// layer's fills, strokes, font or effects ([`Self::select_same_source`]),
    /// the source included. Locked layers are left out and counted, as
    /// `select_by_color` does.
    pub(crate) fn select_same(&mut self, same: Same) {
        let Ok(source) = self.select_same_source(same) else {
            return;
        };
        let doc = &self.session.doc;
        let key = doc.get(source).expect("the source is in the document");
        let families = text_families(doc, source);
        let matches = |id: NodeId| -> bool {
            let Some(n) = doc.get(id) else { return false };
            match same {
                Same::Fill => {
                    n.paint().fills.len() == key.paint().fills.len()
                        && n.paint()
                            .fills
                            .iter()
                            .zip(&key.paint().fills)
                            .all(|(a, b)| a.value == b.value)
                }
                Same::Stroke => {
                    n.paint().strokes.len() == key.paint().strokes.len()
                        && n.paint()
                            .strokes
                            .iter()
                            .zip(&key.paint().strokes)
                            .all(|(a, b)| a.value == b.value)
                }
                Same::Font => text_families(doc, id).iter().any(|f| families.contains(f)),
                Same::Effect => {
                    n.effects().len() == key.effects().len()
                        && n.effects()
                            .iter()
                            .zip(key.effects())
                            .all(|(a, b)| a.value == b.value)
                }
            }
        };
        let mut found = Vec::new();
        let mut stack: Vec<NodeId> = doc
            .get(self.session.page)
            .map(|r| r.children().iter().rev().copied().collect())
            .unwrap_or_default();
        while let Some(id) = stack.pop() {
            // The vault's vendored mains are never selected.
            if Some(id) == doc.vault() {
                continue;
            }
            let Some(node) = doc.get(id) else { continue };
            if matches(id) {
                found.push(id);
            }
            stack.extend(node.children().iter().rev().copied());
        }
        let counted = found.len();
        found.retain(|id| self.may_select_in_bulk(*id));
        let locked = counted - found.len();
        let what = match same {
            Same::Fill => "fill",
            Same::Stroke => "stroke",
            Same::Font => "font",
            Same::Effect => "effects",
        };
        let n = found.len();
        self.entered_group = None;
        self.session.selection.set(found);
        self.session.info(match (n, locked) {
            (1, 0) => format!("Selected 1 layer with this {what}"),
            (n, 0) => format!("Selected {n} layers with the same {what}"),
            (n, l) => format!("Selected {n} of {counted} layers with the same {what} — {l} locked"),
        });
    }

    /// Replace the selection with the layers beside it that it leaves out: every child
    /// of each selected layer's parent that is not selected, locked ones skipped.
    pub(crate) fn select_inverse(&mut self) {
        let doc = &self.session.doc;
        let ids = build::outermost(doc, self.session.selection.ids());
        if ids.is_empty() {
            return;
        }
        let mut parents: Vec<NodeId> = Vec::new();
        for p in ids
            .iter()
            .filter_map(|id| doc.get(*id).and_then(|n| n.parent()))
        {
            if !parents.contains(&p) {
                parents.push(p);
            }
        }
        let others: Vec<NodeId> = parents
            .iter()
            .filter_map(|p| doc.get(*p))
            .flat_map(|p| p.children().iter().copied())
            .filter(|c| !ids.contains(c) && Some(*c) != doc.vault())
            .filter(|c| self.may_select_in_bulk(*c))
            .collect();
        if others.is_empty() {
            self.session
                .info("Nothing else to select beside these layers");
            return;
        }
        self.session.selection.set(others);
    }

    /// The layers a marquee or Ctrl+A can pick: the direct children of every frame and
    /// the root's own non-frame children.
    ///
    /// Locked layers are filtered here, once for every caller: the lock is enforced only
    /// at selection time. A row click or a frame's tag can still select one.
    pub(crate) fn marquee_candidates(&self) -> Vec<NodeId> {
        let mut ids = self.marquee_range();
        ids.retain(|id| self.may_select_in_bulk(*id));
        ids
    }

    /// Whether a door that selects a set may put `id` in it: the one statement for
    /// [`Self::marquee_candidates`], [`Self::select_all`] and `inspector::select_by_color`.
    ///
    /// The effective lock, so a layer inside a locked group is refused too. Visibility is
    /// deliberately not part of it: a hidden layer is editable.
    pub(crate) fn may_select_in_bulk(&self, id: NodeId) -> bool {
        !ondin_core::is_effectively_locked(&self.session.doc, id)
    }

    /// [`Self::marquee_candidates`] before the lock filter. Never select from it: it only
    /// tells `select_all`'s fallback whether the range was empty, not all locked.
    fn marquee_range(&self) -> Vec<NodeId> {
        let mut ids = Vec::new();
        for artboard in self.root_artboards() {
            if let Some(node) = self.session.doc.get(artboard) {
                ids.extend(node.children().iter().copied());
            }
        }
        if let Some(root) = self.session.doc.get(self.session.page) {
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
        // A guide nudges along its one axis; the cross-axis arrow is silently inert.
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
                // A burst of taps is one undo step, under its own verb so it never
                // merges with a point nudge.
                if self.session.commit_run(tx, "nudge-layers") {
                    self.reveal_selection();
                }
            }
            Err(e) => self.session.fail(format!("Nudge failed: {e}")),
        }
    }

    /// Grow or shrink the selection's box by `delta` points per axis —
    /// `Ctrl`+arrows. Positive is wider and taller.
    ///
    /// Held by its top-left corner. The arithmetic is the inspector's W/H fields': one
    /// layer through [`crate::tools::resize_box_to`], several through
    /// [`crate::tools::resize_selection`] over the world union.
    ///
    /// A 1pt floor per axis stops the ask going negative (which would reflect the layer
    /// across its anchored edge). A proportion-locked selection
    /// ([`crate::tools::proportions_locked`]) scales both axes, the arrow naming the
    /// driver. Guides are inert.
    fn size_step(&mut self, delta: Vec2) {
        if self.session.selection.is_empty() {
            return;
        }
        let Some((roots, union)) = self.selection_union() else {
            return;
        };
        let want = |extent: f64, d: f64| (extent + d).max(1.0);
        // Read across the whole selection, as `canvas::keep_ratio` does.
        let locked =
            crate::tools::proportions_locked(&self.session.doc, self.session.selection.ids());
        // Which arrow was pressed; the keymap never sets both.
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
                    crate::tools::SelectionBox::upright(union),
                    crate::preview::Handle::BottomRight,
                    // Both from `min`: the far edge stops being "unchanged" once paired.
                    Point::new(union.min_x() + size.width, union.min_y() + size.height),
                    // Never `keep_ratio`: it takes the larger factor, so a one-axis shrink
                    // would compare against 1.0 and refuse to shrink.
                    crate::tools::Resize::geometry(false, false),
                )
            }
        };
        if tx.0.is_empty() {
            return;
        }
        // Its own verb, so a nudge and a resize never coalesce.
        self.session.commit_run(tx, "resize-layers");
    }

    /// Restack the selection. Reported when nothing moved, or a layer already at the
    /// front would look like a shortcut that failed to fire.
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

    /// *Group selection* (`Ctrl+G`) and the identity row's group button.
    pub(crate) fn group_selection(&mut self) {
        let ids = self.session.selection.ids().to_vec();
        if ids.len() < 2 {
            self.session.info("Select two or more layers to group");
            return;
        }
        match build::group(
            &self.session.doc,
            &self.session.resolved,
            &mut self.session.ids,
            &ids,
        ) {
            Ok((tx, group_id)) => {
                self.name_step_as(crate::menu::Item::Group);
                if self.session.commit(tx) {
                    self.finish_grouping(group_id);
                }
            }
            Err(e) => self.session.fail(format!("Cannot group: {e}")),
        }
    }

    /// *Frame selection*: wrap the selection in a new frame sized to its union.
    /// One layer is enough, unlike grouping. The frame is left open in the layers tree,
    /// where grouping leaves its group shut.
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

    /// *Add flex layout* (`Shift+A`): a flex layout read from where the children
    /// sit (`build::flex_from_placement`) on every selected frame and group without
    /// one, or — when the selection holds anything else — on a frame wrapped around
    /// it, in one undo step.
    pub(crate) fn add_layout(&mut self) {
        let doc = &self.session.doc;
        let res = &self.session.resolved;
        let ids = build::outermost(doc, self.session.selection.ids());
        if ids.is_empty() {
            self.session.info("Select a layer to add a layout");
            return;
        }
        let containers = ids.iter().all(|id| {
            doc.get(*id)
                .is_some_and(|n| ondin_core::container::is_container(n.kind()))
        });
        if containers {
            let ops: Vec<Operation> = ids
                .iter()
                .filter_map(|id| build::flex_from_placement(doc, res, *id))
                .flatten()
                .collect();
            if ops.is_empty() {
                self.session.info("Already laid out");
            } else {
                self.name_step_as(crate::menu::Item::AddLayout);
                self.commit_edit(Transaction(ops));
            }
            return;
        }
        let (mut tx, frame) = match build::frame(doc, res, &mut self.session.ids, &ids) {
            Ok(framed) => framed,
            Err(e) => return self.session.fail(format!("Cannot frame: {e}")),
        };
        // The layout is read from the framed document, the frame's children in it,
        // laid out by updating a copy rather than rebuilding every node.
        let mut after = doc.clone();
        let Ok(outcome) = after.apply(&tx) else {
            return self.session.fail("Cannot frame the selection");
        };
        let mut laid = res.clone();
        laid.update(&after, &outcome.dirty);
        tx.0.extend(build::flex_from_placement(&after, &laid, frame).unwrap_or_default());
        self.name_step_as(crate::menu::Item::AddLayout);
        if self.commit_edit(tx) {
            self.session.selection.set_one(frame);
        }
    }

    /// *Create component* (`Ctrl+Alt+K`): make the selected frame or group a main
    /// component, or wrap anything else in a frame first and make that the main, in
    /// one undo step. Only the wrap speaks (the tree changed shape).
    ///
    /// A refusal names its rule ([`crate::menu::create_refusal`]), asked through
    /// [`Self::create_component_refusal`] so the row, the chord and the commit agree.
    pub(crate) fn create_component(&mut self) {
        use ondin_core::component::NotMain;
        let ids = build::outermost(&self.session.doc, self.session.selection.ids());
        let doc = &self.session.doc;
        match (ids.as_slice(), self.create_component_refusal()) {
            ([], _) => self.session.info("Select a layer to make a component"),
            (_, Some(NotMain::Already)) => self.session.info("Already a component"),
            (_, Some(why)) => self.session.fail(crate::menu::create_refusal(why)),
            ([one], None) if create_in_place(doc, *one) => {
                self.session
                    .commit(Transaction(vec![Operation::SetComponent {
                        id: *one,
                        component: true,
                    }]));
            }
            _ => {
                let wrapped = ids.len();
                match build::frame(doc, &self.session.resolved, &mut self.session.ids, &ids) {
                    Ok((mut tx, frame)) => {
                        tx.0.push(Operation::SetComponent {
                            id: frame,
                            component: true,
                        });
                        if self.session.commit(tx) {
                            self.session.selection.set_one(frame);
                            let name = self
                                .session
                                .doc
                                .get(frame)
                                .map(|n| n.name().to_string())
                                .unwrap_or_default();
                            self.session.info(format!(
                                "Created “{name}” — {wrapped} layer{} wrapped in a frame · Ctrl+Z to undo",
                                if wrapped == 1 { "" } else { "s" }
                            ));
                        }
                    }
                    Err(e) => self.session.fail(format!("Cannot make a component: {e}")),
                }
            }
        }
    }

    /// Why *Create component* would refuse the selection, or `None` when it would commit
    /// — asked by [`Self::create_component`] and the menu row's dimming alike. In place
    /// for one frame or group (`component::why_not_main`), else for the wrap.
    pub(crate) fn create_component_refusal(&self) -> Option<ondin_core::component::NotMain> {
        let doc = &self.session.doc;
        let ids = build::outermost(doc, self.session.selection.ids());
        match ids.as_slice() {
            [] => None,
            [one] if create_in_place(doc, *one) => ondin_core::component::why_not_main(doc, *one),
            _ => ondin_core::component::why_not_wrap(doc, &ids),
        }
    }

    /// *Detach instance* (`Ctrl+Alt+B`): every instance the selection sits in becomes
    /// ordinary layers that keep their look (`component::detach`). No toast.
    pub(crate) fn detach_instances(&mut self) {
        let doc = &self.session.doc;
        let mut roots: Vec<NodeId> = self
            .session
            .selection
            .ids()
            .iter()
            .filter_map(|id| ondin_core::component::instance_root(doc, *id))
            .collect();
        roots.sort();
        roots.dedup();
        // Only the outermost are acted on.
        let roots = build::outermost(doc, &roots);
        if roots.is_empty() {
            self.session.info("Select an instance to detach");
            return;
        }
        let ops: Vec<Operation> = roots
            .iter()
            .filter_map(|r| ondin_core::component::detach(doc, *r))
            .flat_map(|tx| tx.0)
            .collect();
        self.name_step_as(crate::menu::Item::DetachInstance);
        self.session.commit(Transaction(ops));
    }

    /// *Go to main component*: select what the selected layer was copied from (the main
    /// for an instance, the counterpart inside it for a child) and bring it into view.
    pub(crate) fn go_to_main(&mut self) {
        let Some(id) = self.session.selection.single() else {
            return;
        };
        let doc = &self.session.doc;
        let target = match doc.get(id).and_then(|n| n.link()) {
            Some(src) if ondin_core::component::instance_root(doc, id) == Some(id) => {
                ondin_core::component::main_of(doc, id).unwrap_or(src)
            }
            Some(src) => src,
            None => return,
        };
        self.select_main(target);
    }

    /// Select `main` (a main or a node inside one) and bring it into view — on its
    /// own page, which becomes current — unless it is a vendored kit main: that
    /// lives in the vault and is never selected. *Go to main in the kit* is the
    /// kit's own verb.
    pub(crate) fn select_main(&mut self, main: NodeId) {
        if self.session.doc.in_vault(main) {
            self.session
                .info("Its main is in a kit, not in this document");
            return;
        }
        if let Some(page) = self.session.doc.page_of(main) {
            self.go_to_page(page);
        }
        self.session.selection.set_one(main);
        self.reveal_selection();
    }

    /// What the one selected layer is to the components machinery, for the menu
    /// rows — [`crate::menu::Role::Plain`] for several layers or none.
    pub(crate) fn component_role(&self) -> crate::menu::Role {
        use crate::menu::Role;
        let doc = &self.session.doc;
        let Some(id) = self.session.selection.single() else {
            return Role::Plain;
        };
        let Some(node) = doc.get(id) else {
            return Role::Plain;
        };
        if node.set().is_some() {
            return Role::Set;
        }
        if node.component() {
            return Role::Main;
        }
        match ondin_core::component::instance_root(doc, id) {
            Some(root) if root == id => Role::Instance,
            Some(_) if node.link().is_some() => Role::Member,
            Some(_) => Role::Local,
            None => Role::Plain,
        }
    }

    /// *Duplicate as component*: a copy of the selected main that is a new, independent
    /// main rather than an instance.
    pub(crate) fn duplicate_as_component(&mut self) {
        let Some(id) = self.session.selection.single() else {
            return;
        };
        let doc = &self.session.doc;
        let (Some(template), Some(parent)) = (
            doc.capture_subtree(id),
            doc.get(id).and_then(|n| n.parent()),
        ) else {
            return;
        };
        let after = doc
            .get(parent)
            .and_then(|p| p.children().iter().position(|c| *c == id))
            .map(|i| i + 1);
        let step = self.clone_step(&[id]);
        let placements = [build::Placement {
            nodes: template,
            parent,
            index: after,
        }];
        let (tx, created) = build::insert_subtrees_as(
            &self.session.doc,
            &mut self.session.ids,
            &placements,
            step,
            ondin_core::component::MainCopy::NewMain,
        );
        self.name_step_as(crate::menu::Item::DuplicateAsComponent);
        if self.session.commit(tx) && !created.is_empty() {
            self.session.info("Duplicated as a new component");
            self.session.selection.set(created);
        }
    }

    /// *Select all instances* of the selected main — or, on a set, of every
    /// variant in it — on the current page: a selection never spans pages, so
    /// the status line counts the ones elsewhere.
    pub(crate) fn select_all_instances(&mut self) {
        let Some(main) = self.session.selection.single() else {
            return;
        };
        let doc = &self.session.doc;
        let mains = if ondin_core::variant::is_set(doc, main) {
            ondin_core::variant::variants(doc, main)
        } else {
            vec![main]
        };
        let (here, elsewhere): (Vec<NodeId>, Vec<NodeId>) = mains
            .iter()
            .flat_map(|m| ondin_core::component::instances_of(doc, *m))
            .partition(|i| doc.page_of(*i) == Some(self.session.page));
        let away = match elsewhere.len() {
            0 => String::new(),
            1 => " — 1 more on other pages".to_string(),
            n => format!(" — {n} more on other pages"),
        };
        if here.is_empty() {
            self.session.info(match away.is_empty() {
                true => "No instances of this component".to_string(),
                false => format!("No instances on this page{away}"),
            });
            return;
        }
        if !away.is_empty() {
            self.session
                .info(format!("Selected {} instance(s){away}", here.len()));
        }
        self.session.selection.set(here);
    }

    /// The selected mains, when *Combine as variants* could take them: two or
    /// more, every one a main not already a variant, all under one parent
    /// (`build::frame`'s rule, which the verb wraps them with).
    pub(crate) fn combinable_mains(&self) -> Option<Vec<NodeId>> {
        let doc = &self.session.doc;
        let ids = self.session.selection.ids();
        let parent = doc.get(*ids.first()?)?.parent();
        let ok = ids.len() >= 2
            && ids.iter().all(|id| {
                doc.get(*id)
                    .is_some_and(|n| n.component() && n.parent() == parent)
                    && ondin_core::variant::set_of(doc, *id).is_none()
            });
        ok.then(|| ids.to_vec())
    }

    /// Why *Combine as variants* would refuse the mains [`Self::combinable_mains`]
    /// offers it (a frame main and a group main cannot be one set): the menu row's
    /// dimmed reason and the verb's message.
    pub(crate) fn combine_refusal(&self) -> Option<&'static str> {
        let mains = self.combinable_mains()?;
        (!ondin_core::variant::one_kind(&self.session.doc, &mains))
            .then_some(crate::menu::COMBINE_MIXED)
    }

    /// *Combine as variants*: the selected mains wrapped in a set, one property
    /// whose values are their names. The toast speaks because the tree changed
    /// shape.
    pub(crate) fn combine_as_variants(&mut self) {
        let mains = self.combinable_mains().or_else(|| {
            // A lone main from the Properties card's *Variant* offer.
            let one = self.session.selection.single()?;
            let doc = &self.session.doc;
            (doc.get(one)?.component() && ondin_core::variant::set_of(doc, one).is_none())
                .then(|| vec![one])
        });
        let Some(mains) = mains else {
            self.session.info("Select main components to combine");
            return;
        };
        // Said before `variant::combine` refuses it as a bare `WrongKindForOp`.
        if !ondin_core::variant::one_kind(&self.session.doc, &mains) {
            self.session.fail(crate::menu::COMBINE_MIXED);
            return;
        }
        match ondin_core::variant::combine(
            &self.session.doc,
            &self.session.resolved,
            &mut self.session.ids,
            &mains,
        ) {
            Ok((tx, set)) => {
                self.name_step_as(crate::menu::Item::CombineAsVariants);
                if self.session.commit(tx) {
                    self.session.selection.set_one(set);
                    let name = self
                        .session
                        .doc
                        .get(set)
                        .map(|n| n.name().to_string())
                        .unwrap_or_default();
                    self.session.info(format!(
                        "Combined {} main{} into set “{name}” · Ctrl+Z to undo",
                        mains.len(),
                        if mains.len() == 1 { "" } else { "s" }
                    ));
                }
            }
            Err(e) => self.session.fail(format!("Cannot combine: {e}")),
        }
    }

    /// *Add variant*, on a set or a variant: a copy of the selected variant (or the set's
    /// last) taking the first free combination, selected.
    pub(crate) fn add_variant(&mut self) {
        let Some(id) = self.session.selection.single() else {
            return;
        };
        let doc = &self.session.doc;
        let (set, from) = if ondin_core::variant::is_set(doc, id) {
            (id, None)
        } else if let Some(set) = ondin_core::variant::set_of(doc, id) {
            (set, Some(id))
        } else {
            self.session.info("Select a component set or a variant");
            return;
        };
        let Some((tx, new)) = ondin_core::variant::add_variant(
            &self.session.doc,
            &self.session.resolved,
            &mut self.session.ids,
            set,
            from,
        ) else {
            self.session.info("This set has no variant to copy");
            return;
        };
        self.name_step_as(crate::menu::Item::AddVariant);
        if self.session.commit(tx) {
            self.session.selection.set_one(new);
        }
    }

    /// The component property the one selected linked layer drives whose field differs
    /// from the main's — what *Reset Label text* resets.
    pub(crate) fn overridden_property_of_selection(&self) -> Option<ondin_core::variant::Property> {
        self.overridden_property_and_root().map(|(p, _)| p)
    }

    /// [`Self::overridden_property_of_selection`] with the instance whose property it is,
    /// climbing outward: a swap property's counterpart is a nested copy, itself an
    /// instance root, so its property is its outer instance's.
    fn overridden_property_and_root(&self) -> Option<(ondin_core::variant::Property, NodeId)> {
        let doc = &self.session.doc;
        let id = self.session.selection.single()?;
        let mut root = ondin_core::component::instance_root(doc, id);
        while let Some(r) = root {
            let found = ondin_core::variant::instance_properties(doc, r)
                .into_iter()
                .map(|p| p.value)
                .find(|p| {
                    ondin_core::variant::counterparts(doc, r, p).contains(&id)
                        && ondin_core::variant::property_state(doc, r, p).is_some_and(|(_, o)| o)
                });
            if let Some(p) = found {
                return Some((p, r));
            }
            root = doc
                .get(r)
                .and_then(|n| n.parent())
                .and_then(|p| ondin_core::component::instance_root(doc, p));
        }
        None
    }

    /// *Reset Label text* (named for the property) from the context menu: that property
    /// back to the main's value on the instance the selected layer sits in.
    pub(crate) fn reset_selected_property(&mut self) {
        let Some((p, root)) = self.overridden_property_and_root() else {
            return;
        };
        let ops = ondin_core::variant::reset_property(&self.session.doc, &[root], &p);
        self.session.commit(Transaction(ops));
    }

    /// *Restore children* from the context menu. Scoped by the row's fold (`row_folded`):
    /// expanded, its own removed children and their order
    /// (`reset::restore_own_children`); collapsed, everything inside it
    /// (`reset::restore_structure`). One transaction; no toast.
    pub(crate) fn restore_selected_children(&mut self) {
        let Some(p) = self.session.selection.single() else {
            return;
        };
        let ops = if self.row_folded(p) {
            ondin_core::reset::restore_structure(&self.session.doc, &[p], &mut self.session.ids)
        } else {
            ondin_core::reset::restore_own_children(&self.session.doc, p, &mut self.session.ids)
        };
        if ops.is_empty() {
            self.session
                .info("Nothing here differs from the main component");
            return;
        }
        // As in `reset_tx`, so a restore is never read as the hand's placement or resize.
        let tx = self.placement_stated(Transaction(ops));
        let tx = self.items_stated(tx);
        if let Err(e) = self.session.try_commit(tx) {
            self.session.fail(format!("Cannot restore: {e}"));
        }
    }

    /// A reset of everything selected that sits in an instance: each selected layer is
    /// the scope of its own reset. One transaction. No toast.
    pub(crate) fn reset_selection(&mut self, kind: ondin_core::reset::Kind) {
        if let Some(tx) = self.reset_tx(kind)
            && let Err(e) = self.session.try_commit(tx)
        {
            self.session.fail(format!("Cannot reset: {e}"));
        }
    }

    /// The transaction [`Self::reset_selection`] commits, or `None` (having said why)
    /// when there is nothing to reset. Split out so the inspector's Component card can
    /// commit it through [`Self::commit_edit`] and arm the chrome hide.
    pub(crate) fn reset_tx(&mut self, kind: ondin_core::reset::Kind) -> Option<Transaction> {
        let doc = &self.session.doc;
        let scopes: Vec<NodeId> = build::outermost(doc, self.session.selection.ids())
            .into_iter()
            .filter(|id| ondin_core::component::instance_root(doc, *id).is_some())
            .collect();
        if scopes.is_empty() {
            self.session.info("Select an instance to reset");
            return None;
        }
        let ops = ondin_core::reset::reset(doc, kind, &scopes, &mut self.session.ids);
        if ops.is_empty() {
            // The card's *Reset all* tooltip's words.
            self.session
                .info("Nothing here differs from the main component");
            return None;
        }
        // A reset is neither a resize nor the hand placing a pinned or laid-out layer;
        // its insets are stated so the commit door does not re-pin or drop it.
        let ops = self.placement_stated(Transaction(ops));
        Some(self.items_stated(ops))
    }

    /// Select a freshly made group, and leave it shut in the layers tree.
    pub(crate) fn finish_grouping(&mut self, group: NodeId) {
        self.collapsed.insert(group);
        self.session.selection.set_one(group);
        self.session.info("Grouped");
    }

    /// Dissolve every group **or boolean** in the selection (`Ctrl+Shift+G`).
    ///
    /// A boolean's operands are kept intact, so releasing one is the same splice. One
    /// press is one transaction ([`build::ungroup_all`]). Also the identity row's door.
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
        // Read before the commit removes the containers; dissolved children are filtered below.
        let children: Vec<NodeId> = groups
            .iter()
            .filter_map(|g| self.session.doc.get(*g))
            .flat_map(|n| n.children().to_vec())
            .collect();
        let tx = match build::ungroup_all(&self.session.doc, &self.session.resolved, &groups) {
            Ok(tx) => tx,
            Err(e) => {
                self.session.fail(format!("Cannot ungroup: {e}"));
                return;
            }
        };
        self.name_step_as(crate::menu::Item::Ungroup);
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

    pub(crate) fn zoom_to(&mut self, bounds: Option<Rect>) {
        let Some(bounds) = bounds else {
            return;
        };
        if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
            return;
        }
        let (pw, ph) = (self.canvas_px.0 as f64, self.canvas_px.1 as f64);
        let zoom = (pw / bounds.width()).min(ph / bounds.height()) * 0.9;
        // `set_zoom` clamps; a computed fit can leave the range.
        self.session.camera.set_zoom(zoom);
        self.session.camera.center = bounds.center();
    }

    /// Scroll the selection back into view, by the least that will do it, for edits that
    /// move a layer without the pointer (keyboard nudges, the inspector's X and Y).
    /// Size-only edits deliberately do not call this; scrolling would fight the hand.
    pub(crate) fn reveal_selection(&mut self) {
        let Some(bounds) = self.selection_bounds() else {
            return;
        };
        let view = self.session.camera.viewport(self.canvas_px).view;
        // A margin in world units, so the layer lands inside the frame.
        let margin = (REVEAL_MARGIN_PX / self.session.camera.zoom)
            .min(view.width() / 4.0)
            .min(view.height() / 4.0);
        let view = view.inset(-margin);

        // Per axis and independently.
        let dx = reveal_axis(bounds.min_x(), bounds.max_x(), view.min_x(), view.max_x());
        let dy = reveal_axis(bounds.min_y(), bounds.max_y(), view.min_y(), view.max_y());
        if dx != 0.0 || dy != 0.0 {
            self.session.camera.center += Vec2::new(dx, dy);
        }
    }

    /// Everything on the current page — what `Action::ZoomFit` fits.
    pub(crate) fn document_bounds(&self) -> Option<Rect> {
        self.session.preview_world_bounds(self.session.page)
    }

    /// The selection's union — what `Action::ZoomSelection` fits. `None` with an
    /// empty selection, with no fallback to [`Self::document_bounds`].
    fn selection_bounds(&self) -> Option<Rect> {
        self.session
            .selection
            .ids()
            .iter()
            .filter_map(|id| self.session.preview_world_bounds(*id))
            .reduce(|a, b| a.union(b))
    }

    /// Fill [`FrameIndex`] if this revision of this page has not been walked yet, then
    /// hand the caller what it wants. A closure rather than a `Ref`, so no caller can
    /// hold the borrow across a nested lookup and panic the `RefCell`.
    pub(crate) fn with_frames<R>(&self, f: impl FnOnce(&FrameIndex) -> R) -> R {
        let mut index = self.frame_index.borrow_mut();
        let at = (self.session.revision(), self.session.page);
        if index.at != Some(at) {
            // Paint order: the last match is the innermost of nested frames.
            let doc = &self.session.doc;
            let all = ondin_core::subtree_nodes(doc, &[self.session.page]);
            index.components = all
                .iter()
                .filter_map(|id| doc.get(*id))
                .any(|n| n.component() || n.link().is_some() || n.set().is_some());
            index.tagged = all
                .into_iter()
                .filter(|id| {
                    doc.get(*id).is_some_and(|n| match n.kind() {
                        NodeKind::Artboard { .. } => true,
                        NodeKind::Group => n.component(),
                        _ => false,
                    })
                })
                .collect();
            index.frames = index
                .tagged
                .iter()
                .copied()
                .filter(|id| {
                    matches!(
                        doc.get(*id).map(|n| n.kind()),
                        Some(NodeKind::Artboard { .. })
                    )
                })
                .collect();
            index.boxes = index
                .frames
                .iter()
                .filter_map(|id| Some((*id, self.session.resolved.world_bounds(*id)?)))
                .collect();
            index.at = Some(at);
            index.walks += 1;
        }
        f(&index)
    }

    /// How many document walks `Self::artboards` has paid for (`FrameIndex::walks`).
    #[cfg(test)]
    pub(crate) fn frame_walks(&self) -> u64 {
        self.frame_index.borrow().walks
    }

    /// Every artboard, in paint order, from the [`FrameIndex`] memo.
    pub(crate) fn artboards(&self) -> Vec<NodeId> {
        self.with_frames(|i| i.frames.clone())
    }

    /// Every layer that carries a tag on the canvas (every artboard, each group that is
    /// a main) in paint order, from the same memo as [`Self::artboards`].
    pub(crate) fn tagged(&self) -> Vec<NodeId> {
        self.with_frames(|i| i.tagged.clone())
    }

    /// The frames sitting straight on the current page, in z-order — for the marquee
    /// and Ctrl+A, which must not offer a frame and its contents together.
    pub(crate) fn root_artboards(&self) -> Vec<NodeId> {
        self.session
            .doc
            .get(self.session.page)
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

/// Whether *Create component* on the one layer `id` makes it the main where it
/// stands — a frame or a group — rather than wrapping it in a new frame first.
fn create_in_place(doc: &ondin_core::Document, id: NodeId) -> bool {
    doc.get(id)
        .is_some_and(|n| matches!(n.kind(), NodeKind::Artboard { .. } | NodeKind::Group))
}

// --- clipboard and structure ---------------------------------------------

/// How far a pasted copy sits from its original, in its parent's space.
const PASTE_OFFSET: Vec2 = Vec2::new(20.0, 20.0);

/// How far a pasted or duplicated **guide** sits from its original, along its one axis:
/// [`PASTE_OFFSET`]'s single component, so repeated Ctrl+D lays out evenly spaced guides.
const GUIDE_PASTE_OFFSET: f64 = PASTE_OFFSET.x;

// A copy landing on its original would be invisible; checked at compile time.
const _: () = assert!(GUIDE_PASTE_OFFSET > 0.0);

/// How far *Paste here* moves the payload: enough to put `from`'s centre on the
/// pointer, or [`PASTE_OFFSET`] when there is no recorded box. Takes the box, not the
/// source ids, which are gone after a cut.
fn paste_aim(from: Option<Rect>, world: Point) -> Vec2 {
    match from {
        Some(b) => world - b.center(),
        None => PASTE_OFFSET,
    }
}

/// The guide a paste or a duplicate of `from` produces: a fresh id, the resolved
/// `owner`, and the original's position stepped along its own axis.
///
/// One rule for Ctrl+V and Ctrl+D. The scope is the caller's to resolve, since paste
/// must survive the original's frame being deleted. `step` is [`GUIDE_PASTE_OFFSET`],
/// or zero for paste in place.
fn pasted_guide(from: &Guide, id: GuideId, owner: NodeId, step: f64) -> Guide {
    Guide {
        id,
        owner,
        position: from.position + step,
        ..*from
    }
}

/// Clearance `reveal_selection` leaves between the layer and the canvas edge, in device
/// px: enough for the selection box and its handles.
const REVEAL_MARGIN_PX: f64 = 24.0;

/// How far one axis must scroll for `b0..b1` to lie inside `v0..v1`: `0.0` when it
/// already does. The clamp in each branch leaves a layer bigger than the viewport alone.
fn reveal_axis(b0: f64, b1: f64, v0: f64, v1: f64) -> f64 {
    if b0 < v0 {
        (b0 - v0).max((b1 - v1).min(0.0))
    } else if b1 > v1 {
        (b1 - v1).min((b0 - v0).max(0.0))
    } else {
        0.0
    }
}

/// Band at the top and bottom of the layers tree inside which a dragged row scrolls it,
/// in points. Narrower than the canvas's: a 32pt band would leave little dead space.
const LAYER_SCROLL_BAND: f32 = 22.0;
/// Fastest the layers tree auto-scrolls, in points per second. Slower than the canvas:
/// the target is one 20px row, so the last stretch has to be steerable.
const LAYER_SCROLL_MAX: f32 = 420.0;

/// The run of duplicates Ctrl+D is currently building. Only the most recent clones are
/// remembered, so duplicating anything else starts a new run at zero.
pub(crate) struct CloneChain {
    /// The clones the last Ctrl+D produced, one per duplicated layer, sorted: the run
    /// continues while exactly these are selected.
    last: Vec<NodeId>,
    /// The first clone's world origin when created; the difference from where it sits
    /// now is the movement to repeat (a selection moves as one).
    placed_at: Point,
    /// The step being repeated, carried forward so a run of presses keeps its spacing.
    step: Vec2,
}

impl OndinApp {
    /// `Ctrl+V`'s whole decision: a picture, then the app's own payload, then text as a
    /// new layer. The layers win over text only while they are still what the OS
    /// clipboard describes ([`Self::owns_the_clipboard`]). [`Self::paste_at`] asks in
    /// the same order.
    fn paste(&mut self) {
        if self.paste_image(None) {
            return;
        }
        match self.take_clipboard() {
            Clipboard::Layers
                if self.paste_into_selected_frame(PASTE_OFFSET, true) || self.paste_clipboard() =>
            {
                return;
            }
            Clipboard::Unreadable => return,
            _ => {}
        }
        // Anything that is not markup is text.
        if self.paste_svg(None) {
            return;
        }
        self.paste_text_as_layer(None);
    }

    /// `Ctrl+Shift+V`: the clipboard back at the **coordinates it was copied from**
    /// (`docs/shortcuts.md` §7): a zero offset through [`Self::paste_clipboard_at`], in
    /// [`Self::paste`]'s payload order. A picture or markup has no position to restore.
    /// Guides are taken here with a zero step, since `paste_clipboard_at` would step them.
    fn paste_in_place(&mut self) {
        if self.paste_image(None) {
            return;
        }
        match self.take_clipboard() {
            Clipboard::Layers => {
                if self.paste_guides(0.0) {
                    return;
                }
                if self.paste_into_selected_frame(Vec2::ZERO, false)
                    || self.paste_clipboard_at(Vec2::ZERO)
                {
                    return;
                }
            }
            Clipboard::Unreadable => return,
            Clipboard::Foreign => {}
        }
        // Markup has no "in place" to restore, as a picture does not.
        if self.paste_svg(None) {
            return;
        }
        self.paste_text_as_layer(None);
    }

    /// Whether the in-app clipboard still holds what the **system** clipboard describes.
    ///
    /// Every in-app copy puts a text stand-in on the system clipboard, doubling as a
    /// receipt: if the clipboard no longer reads back as it, something else has copied
    /// since. "No stamp" means nothing of ours is on it ([`Self::stamp_clipboard`]).
    /// Takes the text, so callers read the OS clipboard once.
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

    /// What the clipboard is holding for the layer arm, adopting a copy made in another
    /// `ondin` window on the way past. Every paste door calls this. The receipt is a
    /// fast path; adopting rather than pasting keeps placement in
    /// [`Self::paste_clipboard_at`], whose fallback to the canvas root handles a foreign
    /// copy's unknown parent. One OS read per call.
    fn take_clipboard(&mut self) -> Clipboard {
        let Some(text) = system_clipboard_text() else {
            return Clipboard::Foreign;
        };
        if self.owns_the_clipboard(Some(&text)) {
            return Clipboard::Layers;
        }
        self.adopt_clip_text(text)
    }

    /// [`Self::take_clipboard`]'s decision with the OS read lifted off, so a test can
    /// hand it the text a real [`Self::copy_selection`] produced.
    fn adopt_clip_text(&mut self, text: String) -> Clipboard {
        let Some(read) = ondin_core::io::clip::read(&text) else {
            return Clipboard::Foreign;
        };
        let payload = match read {
            Ok(payload) => payload,
            Err(e) => {
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
            kits: payload.kits,
            images: payload.images,
            vars: payload.vars,
        });
        // So a second paste of the same foreign copy takes the fast path.
        self.clipboard_stamp = Some(ClipStamp::of(&text));
        // One clipboard, two typed fields — see `Self::guide_clipboard`.
        self.guide_clipboard = None;
        Clipboard::Layers
    }

    /// Put `text` on the system clipboard **and** keep it as the receipt
    /// [`Self::owns_the_clipboard`] reads back, so no copy site can forget the stamp.
    fn stamp_clipboard(&mut self, ctx: &egui::Context, text: String) {
        self.clipboard_stamp = Some(ClipStamp::of(&text));
        ctx.copy_text(text);
    }

    /// Copy the selection into the in-app clipboard, and put a text stand-in on the
    /// **system** clipboard: `egui_winit` emits `Event::Paste`, Ctrl+V's only form, only
    /// for a non-empty clipboard. The layer names on the first line, then the payload past
    /// `ondin_core::io::clip::FENCE`, so another `ondin` window pastes layers.
    ///
    /// Returns that text, or `None` for a guide copy or nothing copied; only tests
    /// modelling a second window read it.
    fn copy_selection(&mut self, ctx: &egui::Context) -> Option<String> {
        // Guides are never selected alongside layers.
        if self.copy_guides(ctx) {
            return None;
        }
        // Outermost only: copying a group and its child would paste the child twice.
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
        // Taken while the originals still exist: `cut_selection` deletes next.
        self.clipboard_from = ids
            .iter()
            .filter_map(|id| self.session.preview_world_bounds(*id))
            .reduce(|a, b| a.union(b));
        // Kit mains and image entries (`Clip`), taken while their document is on screen.
        let kits = ondin_core::kit::carried(&self.session.doc, &templates);
        let flat: Vec<Node> = templates.iter().chain(&kits).flatten().cloned().collect();
        let images: Vec<_> = build::image_ids_in(&flat)
            .into_iter()
            .filter_map(|id| Some((id.clone(), self.session.doc.image(&id)?.clone())))
            .collect();
        // Only this document can say what a binding draws.
        let vars = ondin_core::variable::carried(&self.session.doc, &templates);
        // The names alone if `write` fails: only the cross-window paste is lost.
        let heading = names.join(", ");
        let text = ondin_core::io::clip::write_carrying(
            &templates,
            &kits,
            &images,
            &vars,
            self.clipboard_from,
            &heading,
        )
        .unwrap_or(heading);
        self.stamp_clipboard(ctx, text.clone());
        self.session
            .info(format!("Copied {} layer(s)", templates.len()));
        self.clipboard = Some(Clip {
            subtrees: templates,
            kits,
            images,
            vars,
        });
        // One clipboard, two typed fields — see `OndinApp::guide_clipboard`.
        self.guide_clipboard = None;
        Some(text)
    }

    /// Copy the selected guides; returns whether there were any. The system clipboard
    /// gets a stand-in of each guide's axis and number.
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
        // Cleared with the payload it belongs to.
        self.clipboard_from = None;
        true
    }

    /// Paste the copied guides, each **into the scope its original had**, offset along
    /// its own axis. Returns whether there was anything to paste. A canvas-wide guide
    /// lands on the current page; a guide whose frame has since been deleted, or is on
    /// another page, falls back to the current page's canvas rather than being dropped.
    fn paste_guides(&mut self, step: f64) -> bool {
        let Some(guides) = self.guide_clipboard.clone() else {
            return false;
        };
        let page = self.session.page;
        let mut ops = Vec::new();
        let mut ids = Vec::new();
        let mut orphaned = 0;
        for guide in guides {
            let doc = &self.session.doc;
            let framed = !doc.is_page(guide.owner);
            let owner = if framed && doc.page_of(guide.owner) == Some(page) {
                guide.owner
            } else {
                page
            };
            if framed && owner == page {
                orphaned += 1;
            }
            let id = GuideId(self.session.ids.mint());
            ids.push(id);
            ops.push(Operation::AddGuide {
                guide: pasted_guide(&guide, id, owner, step),
            });
        }
        // Counted off the paste, not the selection, which locked guides leave alone.
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

    /// Ctrl+D on guides: a copy of each selected guide, in its own scope, offset along its
    /// axis. Returns whether there were any. Not routed through the clipboard, which
    /// Ctrl+D must not overwrite.
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

    /// Make `ids` the selection, honouring the guide lock: locked guides are
    /// unselectable (`OndinApp::guide_at`), so a fresh copy is not selected.
    fn select_pasted_guides(&mut self, ids: Vec<GuideId>) {
        if self.lock_guides {
            return;
        }
        self.session.selection.clear();
        for id in ids {
            self.session.selection.toggle_guide(id);
        }
    }

    /// Cut is copy then delete, and one undo step. Nothing selected leaves the clipboard
    /// alone.
    ///
    /// A cut main moves: its delete detaches the instances, the cut remembers each link
    /// it took ([`Self::cut_move`]), and the first paste in this document restores the
    /// main under its own ids and relinks them ([`Self::paste_cut_move`]).
    fn cut_selection(&mut self, ctx: &egui::Context) {
        let layers = !build::outermost(&self.session.doc, self.session.selection.ids()).is_empty();
        let guides = !self.session.selection.guides().is_empty();
        if !layers && !guides {
            return;
        }
        self.copy_selection(ctx);
        if guides {
            self.remove_selected_guides();
            self.session.info("Cut");
            return;
        }
        self.cut_move = None;
        let Some(d) = self.delete_layers() else {
            return;
        };
        if !d.links.is_empty() {
            self.cut_move = Some(CutMove {
                doc_page: self.session.page,
                roots: self
                    .clipboard
                    .iter()
                    .flat_map(|c| &c.subtrees)
                    .filter_map(|s| s.first().map(|n| n.id()))
                    .collect(),
                links: d.links,
                stamp: self.clipboard_stamp,
                depth: self.session.history.undo_depth(),
                parked: None,
            });
        }
        self.session.info(match (d.detached, d.name) {
            (0, _) => "Cut".to_string(),
            (n, Some(name)) => format!(
                "Cut “{name}” — {n} instance{} will follow it on paste",
                if n == 1 { "" } else { "s" }
            ),
            (n, None) => format!(
                "Cut {} layer(s) — {n} instance(s) will follow on paste",
                d.count
            ),
        });
    }

    /// Put the selection on the system clipboard as an SVG document, from the committed
    /// document so a gesture in flight cannot leak into it.
    ///
    /// `build::in_document_order` drops children of selected groups and restores z-order
    /// (`Selection::ids` is pick order). No [`Self::clipboard_stamp`]: `owns_the_clipboard`
    /// must answer no afterwards.
    pub(crate) fn copy_as_svg(&mut self, ctx: &egui::Context) {
        let ids = build::in_document_order(&self.session.doc, self.session.selection.ids());
        if ids.is_empty() {
            return;
        }
        // The reported form: the status line is the only place to say the markup is not
        // quite the document.
        let out =
            ondin_export::svg::svg_of_reported(&self.session.doc, &self.session.resolved, &ids);
        let note = crate::panels::fidelity_clauses(&out.approximated, &out.round_trip);
        ctx.copy_text(out.svg);
        let n = ids.len();
        self.session.info(match note {
            None => format!("Copied {n} layer(s) as SVG"),
            Some(note) => format!("Copied {n} layer(s) as SVG — {note}"),
        });
    }

    /// Put each selected layer's CSS on the clipboard as text, under a comment
    /// naming it: a leaf's declarations, a container's rules for its subtree
    /// (`ondin_export::code::css_of`).
    pub(crate) fn copy_as_css(&mut self, ctx: &egui::Context) {
        let ids = build::in_document_order(&self.session.doc, self.session.selection.ids());
        if ids.is_empty() {
            return;
        }
        let css = ondin_export::code::css_of(&self.session.doc, &self.session.resolved, &ids);
        ctx.copy_text(css);
        self.session
            .info(format!("Copied {} layer(s) as CSS", ids.len()));
    }

    /// *Copy as PNG* — the selection as a picture on the clipboard at `scale`×,
    /// transparent.
    ///
    /// `raster_of`, not `png_of`: `arboard::ImageData` takes raw RGBA, and encoded PNG
    /// bytes would paste as noise. Ordered as in [`Self::copy_as_svg`]. No
    /// `clipboard_stamp`: it would make `owns_the_clipboard` answer yes for a flat picture.
    pub(crate) fn copy_as_png(&mut self, scale: u8) {
        let ids = build::in_document_order(&self.session.doc, self.session.selection.ids());
        if ids.is_empty() {
            return;
        }
        let Some(raster) =
            Self::png_for_the_clipboard(&self.session.doc, &self.session.resolved, &ids, scale)
        else {
            self.session.info("Nothing to copy as PNG");
            return;
        };
        let (w, h) = (raster.width, raster.height);
        let image = arboard::ImageData {
            width: w as usize,
            height: h as usize,
            bytes: raster.rgba.into(),
        };
        // `arboard` directly, since egui's clipboard is text-only; refused on a headless app.
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
                .info(format!("Copied {} layer(s) as PNG, {w}×{h}", ids.len())),
            Err(e) => self
                .session
                .info(format!("Could not copy the picture: {e}")),
        }
    }

    /// The picture *Copy as PNG* hands the clipboard: raw RGBA at `scale` device
    /// pixels per world unit. Takes no `self`, so it is testable without the OS
    /// clipboard.
    fn png_for_the_clipboard(
        doc: &ondin_core::Document,
        res: &ondin_core::Resolved,
        ids: &[ondin_core::NodeId],
        scale: u8,
    ) -> Option<ondin_export::png::Raster> {
        let opts = ondin_export::png::RasterOpts {
            scale: f64::from(scale.max(1)),
            ..Default::default()
        };
        ondin_export::png::raster_of(doc, res, ids, &opts)
    }

    /// The layer *Copy properties* would lift an appearance off: the key layer, else a
    /// lone selected layer. `None` for an unkeyed multi-selection and for a layer with no
    /// appearance, such as a group.
    pub(crate) fn properties_source(&self) -> Option<(NodeId, build::Properties)> {
        let sel = &self.session.selection;
        let id = sel.key().or_else(|| sel.single())?;
        let props = build::properties_of(&self.session.doc, id)?;
        Some((id, props))
    }

    /// Lift the source layer's fills, strokes, effects and opacity into
    /// [`Self::property_clipboard`] (`Ctrl`+`Alt`+`C`). Reads the committed node, not a
    /// gesture's preview.
    fn copy_properties(&mut self) {
        let Some((_, props)) = self.properties_source() else {
            return;
        };
        let mut parts: Vec<&str> = [
            (!props.fills.is_empty()).then_some("fill"),
            (!props.strokes.is_empty()).then_some("stroke"),
            (!props.effects.is_empty()).then_some("effects"),
        ]
        .into_iter()
        .flatten()
        .collect();
        parts.push("opacity");
        let what = match parts.split_last() {
            Some((last, [])) => last.to_string(),
            Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
            None => unreachable!("opacity is always copied"),
        };
        self.property_clipboard = Some(props);
        self.session.info(format!("Copied {what}"));
    }

    /// Give the selection the appearance the clipboard is holding (`Ctrl`+`Alt`+`V`,
    /// `build::paste_properties`). One undo step; an empty transaction says so.
    fn paste_properties(&mut self) {
        let Some(props) = self.property_clipboard.clone() else {
            return;
        };
        let ids = self.session.selection.ids().to_vec();
        let tx = build::paste_properties(&self.session.doc, &ids, &props, &mut self.session.ids);
        if tx.0.is_empty() {
            self.session.info("Already has these properties");
            return;
        }
        // Layers actually changed (one layer can take several ops, hence the dedup).
        let mut touched: Vec<_> = tx.0.iter().filter_map(|op| op.overwrites()).collect();
        touched.sort_unstable();
        touched.dedup();
        let layers = touched.len();
        // Read before the commit, which cannot report either.
        let (rebound, mut cut) = properties_landing(&self.session.doc, &props);
        for name in paints_overwritten(&self.session.doc, &tx) {
            if !cut.contains(&name) {
                cut.push(name);
            }
        }
        if self.session.commit(tx) {
            let (styles, mut vars) = self.session.last_detached().clone();
            for name in cut {
                if !vars.contains(&name) {
                    vars.push(name);
                }
            }
            vars.sort();
            let mut said = match layers {
                1 => "Pasted properties".to_owned(),
                n => format!("Pasted properties onto {n} layers"),
            };
            match rebound.as_slice() {
                [] => {}
                [one] => said.push_str(&format!(" · Rebound {one}")),
                many => said.push_str(&format!(" · Rebound {} variables", many.len())),
            }
            if let Some(detached) = crate::session::said_detached(styles, vars) {
                said.push_str(&format!(" · {detached}"));
            }
            self.session.info(said);
        }
    }

    /// Returns whether it took the paste, so [`Self::paste`] can fall through to text.
    fn paste_clipboard(&mut self) -> bool {
        self.paste_clipboard_at(PASTE_OFFSET)
    }

    /// [`Self::paste_clipboard`] with the placement offset spelled out (*Paste here*).
    fn paste_clipboard_at(&mut self, offset: Vec2) -> bool {
        // The guide step is the constant, not `offset`; *Paste in place* takes the guide
        // arm itself.
        if self.paste_guides(GUIDE_PASTE_OFFSET) {
            return true;
        }
        let Some(clip) = self.clipboard.clone() else {
            return false;
        };
        let templates = &clip.subtrees;
        let fallback = self.session.page;
        let mut placements = Vec::new();
        for template in templates {
            // Back where it came from if that parent is still on this page, else this
            // page's canvas: a paste never lands on a page out of sight.
            let parent = template
                .first()
                .and_then(|n| n.parent())
                .filter(|p| self.session.doc.page_of(*p) == Some(fallback))
                .unwrap_or(fallback);
            // Just above the original (the template still carries its id); appended
            // when the original is gone.
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
                // Rebound for this document, or detached where it has no such variable.
                nodes: ondin_core::variable::land(&self.session.doc, &clip.vars, template.clone()),
                parent,
                index,
            });
        }
        self.insert_all(
            placements,
            "Pasted",
            offset,
            &clip.images,
            &clip.kits,
            &clip.vars.tokens,
        );
        true
    }

    /// Where a clipboard template drew, in world space: its original's placement while
    /// that is still in the document, else its stored transform under its old parent
    /// (the canvas for a parent that is gone or foreign, as [`Self::paste_clipboard_at`]
    /// falls back).
    fn template_world(&self, template: &[Node]) -> Option<ondin_core::kurbo::Affine> {
        let root = template.first()?;
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        if doc.contains(root.id())
            && let Some(world) = res.world_transform(root.id())
        {
            return Some(world);
        }
        let parent = root
            .parent()
            .filter(|p| doc.contains(*p))
            .and_then(|p| res.world_transform(p))
            .unwrap_or(ondin_core::kurbo::Affine::IDENTITY);
        Some(parent * root.transform())
    }

    /// The box the clipboard's layers occupy where [`Self::template_world`] places them:
    /// their originals' boxes now, while every one is still in the document, else the
    /// box recorded at the copy. A copy-time box alone would aim off by however far an
    /// original moved since.
    fn clip_box(&self, clip: &Clip) -> Option<Rect> {
        let now: Option<Vec<Rect>> = clip
            .subtrees
            .iter()
            .map(|t| {
                let id = t.first()?.id();
                self.session.doc.contains(id).then_some(())?;
                self.session.resolved.world_bounds(id)
            })
            .collect();
        match now {
            Some(boxes) if !boxes.is_empty() => boxes.into_iter().reduce(|a, b| a.union(b)),
            _ => self.clipboard_from,
        }
    }

    /// `templates`, landed for this document, each root re-placed under `parent` at
    /// where it drew moved by `shift` (world space), at `index` of each.
    fn rebased_placements(
        &self,
        clip: &Clip,
        parent: NodeId,
        shift: Vec2,
        index: impl Fn(&[Node]) -> Option<usize>,
    ) -> Vec<build::Placement> {
        let Some(parent_world) = self.session.resolved.world_transform(parent) else {
            return Vec::new();
        };
        clip.subtrees
            .iter()
            .filter_map(|template| {
                let root = template.first()?.id();
                let world = self.template_world(template)?;
                let local =
                    parent_world.inverse() * ondin_core::kurbo::Affine::translate(shift) * world;
                let mut nodes =
                    ondin_core::variable::land(&self.session.doc, &clip.vars, template.clone());
                let landed = nodes.iter_mut().find(|n| n.id() == root)?;
                *landed = landed.with_transform(local);
                Some(build::Placement {
                    nodes,
                    parent,
                    index: index(template),
                })
            })
            .collect()
    }

    /// `Ctrl+V` and `Ctrl+Shift+V` with one frame selected: the clipboard goes inside
    /// it, on top. Pasted layers that would overlap the frame keep their place,
    /// `offset` from it; with `centre`, layers that would land outside it are centred
    /// on it instead.
    ///
    /// `false` leaves the ordinary paste to run: no layers on the clipboard, no lone
    /// unlocked frame, a clipboard holding that frame or what holds it (a duplicate,
    /// not a nesting), every layer copied from inside it already, or a paste the
    /// component rules refuse there.
    fn paste_into_selected_frame(&mut self, offset: Vec2, centre: bool) -> bool {
        let Some(clip) = self.clipboard.clone() else {
            return false;
        };
        let doc = &self.session.doc;
        let Some(frame) = self.session.selection.single() else {
            return false;
        };
        if !matches!(
            doc.get(frame).map(|n| n.kind()),
            Some(NodeKind::Artboard { .. })
        ) || ondin_core::is_effectively_locked(doc, frame)
        {
            return false;
        }
        let roots: Vec<NodeId> = clip
            .subtrees
            .iter()
            .filter_map(|t| t.first().map(|n| n.id()))
            .collect();
        let holds_frame = std::iter::successors(Some(frame), |c| doc.get(*c)?.parent())
            .any(|c| roots.contains(&c));
        let from_inside = clip
            .subtrees
            .iter()
            .all(|t| t.first().and_then(|n| n.parent()) == Some(frame));
        if holds_frame || from_inside {
            return false;
        }
        let Some(frame_box) = self.session.resolved.world_bounds(frame) else {
            return false;
        };
        let shift = match self.clip_box(&clip) {
            Some(from) if centre && !(from + offset).overlaps(frame_box) => {
                frame_box.center() - from.center()
            }
            _ => offset,
        };
        let placements = self.rebased_placements(&clip, frame, shift, |template| {
            // A layer copied from this frame still lands above its original.
            let id = template.first()?.id();
            let siblings = self.session.doc.get(frame)?.children();
            siblings.iter().position(|c| *c == id).map(|i| i + 1)
        });
        if placements.is_empty()
            || !self.paste_is_held(&placements, &clip.images, &clip.kits, &clip.vars.tokens)
        {
            return false;
        }
        self.insert_all(
            placements,
            "Pasted",
            Vec2::ZERO,
            &clip.images,
            &clip.kits,
            &clip.vars.tokens,
        );
        true
    }

    /// *Paste*, through the **layers panel's** door: the clipboard as a sibling
    /// immediately above the right-clicked row. Same payload order as [`Self::paste`];
    /// only layers take the row's slot.
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

    /// The layer half of [`Self::paste_beside`]; returns whether it took the paste. Falls
    /// back to the ordinary paste where "beside this row" has no answer (the root, or a
    /// row whose parent is gone).
    fn paste_clipboard_beside(&mut self, id: NodeId) -> bool {
        // Guides have no place in the tree.
        if self.paste_guides(GUIDE_PASTE_OFFSET) {
            return true;
        }
        let Some(clip) = self.clipboard.clone() else {
            return false;
        };
        let templates = &clip.subtrees;
        let Some((parent, above)) = crate::canvas::sibling_slot_above(&self.session.doc, id) else {
            return self.paste_clipboard_at(PASTE_OFFSET);
        };
        // Every template targets the one slot; `insert_subtrees` advances each past those
        // already inserted, so they keep their order.
        let placements: Vec<build::Placement> = templates
            .iter()
            .map(|template| build::Placement {
                nodes: ondin_core::variable::land(&self.session.doc, &clip.vars, template.clone()),
                parent,
                index: Some(above),
            })
            .collect();
        // Same fallback where the component rules refuse the slot (e.g. `m` pasted
        // inside main `m` would be a `ComponentCycle`).
        if !self.paste_is_held(&placements, &clip.images, &clip.kits, &clip.vars.tokens) {
            return self.paste_clipboard_at(PASTE_OFFSET);
        }
        // Offset like every other paste: this door decided the *slot*, not the position.
        self.insert_all(
            placements,
            "Pasted",
            PASTE_OFFSET,
            &clip.images,
            &clip.kits,
            &clip.vars.tokens,
        );
        true
    }

    /// Whether the document would take `placements` as an ordinary paste, asked by
    /// [`Self::paste_clipboard_beside`], whose slot is the user's. Only the component
    /// rules can refuse, and only under a main or an instance; there the paste is built,
    /// settled as the commit would (`propagate::owed`) and applied to a clone.
    fn paste_is_held(
        &mut self,
        placements: &[build::Placement],
        images: &[(ondin_core::ImageId, ondin_core::ImageEntry)],
        kits: &[Vec<Node>],
        tokens: &ondin_core::tokens::Tokens,
    ) -> bool {
        let doc = &self.session.doc;
        let under_a_component = placements.iter().any(|p| {
            std::iter::successors(doc.get(p.parent), |n| n.parent().and_then(|a| doc.get(a)))
                .any(|n| n.component() || n.link().is_some())
        });
        if !under_a_component {
            return true;
        }
        // The transaction `insert_all` will commit, kit mains included, or a carried instance
        // would be judged detached. A refused carry is `insert_all`'s to report.
        let Ok((tx, _)) = ondin_core::kit::insert_carrying(
            doc,
            &mut self.session.ids,
            kits,
            tokens,
            placements,
            PASTE_OFFSET,
        ) else {
            return true;
        };
        let mut ops = build::missing_image_ops(doc, images);
        ops.extend(tx.0);
        let mut tx = Transaction(ops);
        ondin_core::propagate::owed(doc, &mut tx, &mut self.session.ids);
        doc.clone().apply(&tx).is_ok()
    }

    /// *Paste here*: the clipboard, centred on `world`. The offset is measured from the
    /// bounds recorded in [`Self::clipboard_from`], so a cut aims too. Payloads go in
    /// [`Self::paste`]'s order, each placed at the pointer.
    pub(crate) fn paste_at(&mut self, world: Point) {
        if self.paste_image(Some(world)) {
            return;
        }
        match self.take_clipboard() {
            // `clipboard_from` is read after the adoption: a foreign copy aims by its own box.
            Clipboard::Layers if self.paste_clipboard_at(paste_aim(self.clipboard_from, world)) => {
                return;
            }
            Clipboard::Unreadable => return,
            _ => {}
        }
        if self.paste_svg(Some(world)) {
            return;
        }
        self.paste_text_as_layer(Some(world));
    }

    /// Ctrl+D: a copy in place, then — for a copy of a copy ([`CloneChain`]) — a copy
    /// that repeats however far the last one was moved, so repeated presses lay out a row.
    pub(crate) fn duplicate_selection(&mut self) {
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
        // A duplicate lands in its own document: no pictures or kits to carry.
        self.insert_all(
            placements,
            "Duplicated",
            step,
            &[],
            &[],
            &Default::default(),
        );

        let mut made = build::outermost(&self.session.doc, self.session.selection.ids());
        made.sort();
        self.clones = made
            .first()
            .and_then(|&first| self.origin_of(first))
            .map(|at| CloneChain {
                last: made,
                placed_at: at,
                step,
            });
    }

    /// How far the next duplicate of `ids` is offset: zero unless they are the chain's
    /// last clones, then how far they have moved since (or, if unmoved, the step
    /// already being repeated).
    fn clone_step(&self, ids: &[NodeId]) -> Vec2 {
        let mut ids = ids.to_vec();
        ids.sort();
        let Some(chain) = self.clones.as_ref().filter(|c| c.last == ids) else {
            return Vec2::ZERO;
        };
        let Some(now) = ids.first().and_then(|&id| self.origin_of(id)) else {
            return Vec2::ZERO;
        };
        let moved = now - chain.placed_at;
        if moved.hypot() > 1e-6 {
            moved
        } else {
            chain.step
        }
    }

    /// A node's origin in world space, which the clone chain measures movement against.
    fn origin_of(&self, id: NodeId) -> Option<Point> {
        Some(self.session.resolved.world_transform(id)? * Point::ZERO)
    }

    /// Insert captured subtrees as one undo step, offset by `offset` in their parent's
    /// space, and select the results. `images` and `kits` are what nodes cannot carry
    /// themselves (image-table entries, vendored kit mains); only what this document is
    /// missing is added.
    fn insert_all(
        &mut self,
        placements: Vec<build::Placement>,
        verb: &str,
        offset: Vec2,
        images: &[(ondin_core::ImageId, ondin_core::ImageEntry)],
        kits: &[Vec<Node>],
        tokens: &ondin_core::tokens::Tokens,
    ) {
        if placements.is_empty() {
            return;
        }
        if self.paste_cut_move(&placements, verb, offset, images) {
            return;
        }
        let (tx, created) = match ondin_core::kit::insert_carrying(
            &self.session.doc,
            &mut self.session.ids,
            kits,
            tokens,
            &placements,
            offset,
        ) {
            Ok(built) => built,
            Err(e) => {
                let why = self.carry_refused(&e);
                return self.session.fail(why);
            }
        };
        // Image entries first, so the undo removes them after the layers using them.
        let mut ops = build::missing_image_ops(&self.session.doc, images);
        ops.extend(tx.0);
        let tx = Transaction(ops);
        if self.session.commit(tx) && !created.is_empty() {
            let detached = landed_detached(&self.session.doc, &placements, &created);
            let mut said = format!("{verb} {} layer(s)", created.len());
            if detached > 0 {
                said.push_str(&format!(
                    " · {detached} instance{} detached — {} not in this document",
                    if detached == 1 { "" } else { "s" },
                    if detached == 1 {
                        "its main is"
                    } else {
                        "their mains are"
                    },
                ));
            }
            self.session.info(said);
            self.session.selection.set(created);
        }
    }

    /// The paste that makes a cut a move: when `placements` are the clipboard a
    /// [`CutMove`] was taken for, in the document it was cut from, they go in under their
    /// own ids and every link the cut's delete took is given back. Taken once.
    ///
    /// `false` leaves the ordinary paste to run, including for a move the commit refuses.
    fn paste_cut_move(
        &mut self,
        placements: &[build::Placement],
        verb: &str,
        offset: Vec2,
        images: &[(ondin_core::ImageId, ondin_core::ImageEntry)],
    ) -> bool {
        let doc = &self.session.doc;
        // A parked move is a cut the history has taken back (`document_rewound`).
        let Some(cut) = self.cut_move.as_ref().filter(|c| c.parked.is_none()) else {
            return false;
        };
        let roots: Vec<NodeId> = placements
            .iter()
            .filter_map(|p| p.nodes.first().map(|n| n.id()))
            .collect();
        if cut.stamp != self.clipboard_stamp
            || !doc.is_page(cut.doc_page)
            || cut.roots != roots
            || placements
                .iter()
                .flat_map(|p| &p.nodes)
                .any(|n| doc.contains(n.id()))
        {
            return false;
        }
        let cut = self.cut_move.take().expect("read above");
        let mut ops = build::missing_image_ops(doc, images);
        let mut added: std::collections::HashMap<NodeId, usize> = Default::default();
        for p in placements {
            let root = p.nodes[0].id();
            let at = p
                .index
                .unwrap_or_else(|| doc.get(p.parent).map(|n| n.children().len()).unwrap_or(0));
            let shift = added.entry(p.parent).or_default();
            ops.push(Operation::InsertSubtree {
                nodes: p.nodes.clone(),
                parent: p.parent,
                index: at + *shift,
            });
            *shift += 1;
            ops.push(Operation::SetTransform {
                id: root,
                transform: ondin_core::kurbo::Affine::translate(offset) * p.nodes[0].transform(),
            });
        }
        // Every link the delete took where the node still holds what it left (a nested
        // copy was climbed to its own main, not just unlinked). A node relinked since, or
        // whose swap's main has gone, is left as it is.
        for t in &cut.links {
            let Some(n) = doc.get(t.id) else { continue };
            if (n.link(), n.swap()) != t.left
                || t.was
                    .1
                    .is_some_and(|s| !doc.get(s).is_some_and(|s| s.component()))
            {
                continue;
            }
            ops.push(Operation::SetLink {
                id: t.id,
                link: Some(t.was.0),
            });
            if n.swap() != t.was.1 {
                ops.push(Operation::SetSwap {
                    id: t.id,
                    swap: t.was.1,
                });
            }
        }
        if !self.session.commit(Transaction(ops)) {
            return false;
        }
        self.session
            .info(format!("{verb} {} layer(s)", roots.len()));
        self.session.selection.set(roots);
        true
    }

    pub(crate) fn delete_selection(&mut self) {
        if !self.session.selection.guides().is_empty() {
            self.remove_selected_guides();
            return;
        }
        let Some(d) = self.delete_layers() else {
            return;
        };
        self.session.info(match (d.guides, d.detached, d.name) {
            (_, n @ 1.., Some(name)) => format!(
                "Deleted “{name}” — {n} instance{} detached · Ctrl+Z to undo",
                if n == 1 { "" } else { "s" }
            ),
            (_, n @ 1.., None) => format!(
                "Deleted {} layer(s) — {n} instance(s) detached · Ctrl+Z to undo",
                d.count
            ),
            (0, _, _) => format!("Deleted {} layer(s)", d.count),
            (n, _, _) => format!("Deleted {} layer(s) and {n} guide(s)", d.count),
        });
    }

    /// *Paste to replace*: each selected layer gives way to a copy of the whole clipboard,
    /// the copy's box centred on the layer's and in its slot of its parent, so several
    /// copied layers replace each target together. Locked targets are skipped. One undo
    /// step; the copies end selected.
    pub(crate) fn paste_to_replace(&mut self) {
        if matches!(self.take_clipboard(), Clipboard::Layers) {
            self.replace_with_clipboard();
        }
    }

    /// [`Self::paste_to_replace`] once the clipboard is known to hold layers.
    fn replace_with_clipboard(&mut self) {
        let Some(clip) = self.clipboard.clone() else {
            return;
        };
        let Some(from) = self.clip_box(&clip) else {
            return;
        };
        let doc = &self.session.doc;
        let targets: Vec<NodeId> = build::outermost(doc, self.session.selection.ids())
            .into_iter()
            .filter(|id| !ondin_core::is_effectively_locked(doc, *id))
            .collect();
        let mut placements = Vec::new();
        for target in &targets {
            let Some(parent) = doc.get(*target).and_then(|n| n.parent()) else {
                continue;
            };
            let Some(bounds) = self.session.resolved.world_bounds(*target) else {
                continue;
            };
            // Above the target, which goes: the copies take its slot.
            let index = doc
                .get(parent)
                .and_then(|p| p.children().iter().position(|c| c == target))
                .map(|i| i + 1);
            placements.extend(self.rebased_placements(
                &clip,
                parent,
                bounds.center() - from.center(),
                |_| index,
            ));
        }
        if placements.is_empty() {
            return;
        }
        let (tx, created) = match ondin_core::kit::insert_carrying(
            &self.session.doc,
            &mut self.session.ids,
            &clip.kits,
            &clip.vars.tokens,
            &placements,
            Vec2::ZERO,
        ) {
            Ok(built) => built,
            Err(e) => {
                let why = self.carry_refused(&e);
                return self.session.fail(why);
            }
        };
        let doc = &self.session.doc;
        let mut ops = build::missing_image_ops(doc, &clip.images);
        ops.extend(tx.0);
        // A frame's guides go before the frame, as `delete_layers` orders them.
        ops.extend(build::guides_of(doc, &targets));
        ops.extend(targets.iter().map(|id| Operation::DeleteNode { id: *id }));
        ops.extend(ondin_core::component::relink_for_delete(doc, &targets));
        if self.session.commit(Transaction(ops)) {
            self.session.info(match targets.len() {
                1 => "Replaced 1 layer".to_string(),
                n => format!("Replaced {n} layers"),
            });
            self.session.selection.set(created);
        }
    }

    /// Delete the selected layers in one commit and say what that did; `None` when
    /// nothing was selected or the commit refused. Shared by [`Self::delete_selection`]
    /// and [`Self::cut_selection`].
    fn delete_layers(&mut self) -> Option<Deleted> {
        let ids = build::outermost(&self.session.doc, self.session.selection.ids());
        if ids.is_empty() {
            return None;
        }
        // A frame's guides go with it, and first: the inverse runs reversed, so this order
        // restores the frame before its guides (the other order cannot be undone).
        let mut ops = build::guides_of(&self.session.doc, &ids);
        let guides = ops.len();
        ops.extend(ids.iter().map(|id| Operation::DeleteNode { id: *id }));
        // Links into what is deleted climb past it, or `component::check` refuses the commit.
        let relinks = ondin_core::component::relink_for_delete(&self.session.doc, &ids);
        let detached = relinks
            .iter()
            .filter(|op| {
                matches!(op, Operation::SetLink { id, link: None }
                    if ondin_core::component::instance_root(&self.session.doc, *id) == Some(*id))
            })
            .count();
        ops.extend(relinks);
        let name = match ids.as_slice() {
            [one] => self.session.doc.get(*one).map(|n| n.name().to_string()),
            _ => None,
        };
        // Every link into what goes, read before the commit, so a cut can give them back.
        let doc = &self.session.doc;
        let going: HashSet<NodeId> = build::subtree_nodes(doc, &ids).into_iter().collect();
        let pages: Vec<NodeId> = doc.page_ids().collect();
        let held: Vec<(NodeId, NodeId, Option<NodeId>)> = build::subtree_nodes(doc, &pages)
            .into_iter()
            .filter(|id| !going.contains(id))
            .filter_map(|id| {
                let n = doc.get(id)?;
                Some((id, n.link()?, n.swap()))
            })
            .filter(|(_, src, _)| going.contains(src))
            .collect();
        if !self.session.commit(Transaction(ops)) {
            return None;
        }
        self.session.selection.clear();
        let links = held
            .into_iter()
            .filter_map(|(id, src, swap)| {
                let n = self.session.doc.get(id)?;
                (n.link() != Some(src)).then(|| TakenLink {
                    id,
                    was: (src, swap),
                    left: (n.link(), n.swap()),
                })
            })
            .collect();
        Some(Deleted {
            count: ids.len(),
            guides,
            detached,
            name,
            links,
        })
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
    /// Write the open document to its place in the library (filing it first if new),
    /// pinning a version when `pin`. Answers whether the document was written.
    ///
    /// Synchronous on purpose: a pin reads the bytes back off disk, and the close,
    /// dashboard and open transitions need them down. The `disk_settle` first is
    /// load-bearing: an older in-flight autosave could otherwise land after this.
    pub(crate) fn save_file(&mut self, pin: bool) -> bool {
        self.disk_settle();
        let path = match self.session.path.clone() {
            Some(p) => p,
            None => match self.file_untitled() {
                Some(p) => p,
                None => return false,
            },
        };
        match crate::library::store::write_document_digest(&path, &self.session.doc) {
            Ok(file) => {
                if let Some(history) = self.kept_history() {
                    history.write(&file);
                }
            }
            Err(e) => {
                self.session.fail(format!("Save failed: {e}"));
                return false;
            }
        }
        self.session.mark_saved(path.clone());
        self.last_autosave = std::time::Instant::now();

        if pin && self.prefs.version_history {
            // The document may have just been filed; the entry carries the version's id.
            self.library.refresh();
            if let Some(entry) = self.library.entry_at(&path).cloned() {
                match crate::library::store::pin_version(&self.library.root, &entry) {
                    Ok(Some(_)) => self.session.info("Version saved"),
                    // No id (opened from outside the library): nothing to pin to.
                    Ok(None) => {}
                    Err(e) => self.session.fail(format!("Version failed: {e}")),
                }
            }
        }

        // Only after a successful save, so exports never describe something unsaved.
        if self.prefs.export_on_save {
            self.export_all(crate::panels::ExportAll::Quietly);
        }
        true
    }

    /// File a document that has never been saved, under whatever name it carries.
    /// Returns the path, or `None` after reporting the failure.
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

    /// Queue an autosave (never a pin) if the interval has elapsed and the session is
    /// dirty — an idle session writes nothing, which sync clients would upload.
    ///
    /// `mark_saved` and the export happen on the result ([`Self::apply_disk`]), so the
    /// session stays dirty until the bytes land. `is_saving` is not redundant with the
    /// clock: a short interval could put two writes of one path in flight.
    fn autosave_tick(&mut self, ctx: &egui::Context) {
        let interval = self.prefs.autosave_secs;
        if interval == 0 || !self.session.is_dirty() || self.session.is_saving() {
            return;
        }
        if self.last_autosave.elapsed().as_secs() < interval {
            return;
        }
        // A never-filed document is not autosaved into existence.
        let Some(path) = self.session.path.clone() else {
            return;
        };
        // Stamped at queue time: a back-off on failure.
        self.last_autosave = std::time::Instant::now();
        let at = self.session.revision();
        let doc = self.session.doc.clone();
        let history = self.kept_history();
        self.session.begin_save(at);
        self.writer(ctx).document(&path, doc, at, history);
    }

    /// The undo history to keep beside the document's next write, cut to the *Undo
    /// history* setting: `None` for a document with no id or outside the library — a
    /// copy elsewhere carrying a library document's id would write over that
    /// document's history — and an empty one, which forgets any kept before, for a
    /// size of zero.
    fn kept_history(&self) -> Option<crate::library::writer::KeptHistory> {
        let path = self.session.path.as_deref()?;
        if !path.starts_with(&self.library.root) {
            return None;
        }
        Some(crate::library::writer::KeptHistory {
            root: self.library.root.clone(),
            id: self.session.doc.meta().id.clone()?,
            history: self
                .session
                .history
                .saved(usize::try_from(self.prefs.history_size).unwrap_or(usize::MAX)),
        })
    }

    /// Take a crash snapshot if the interval has elapsed and something has been
    /// committed since the last one, queued onto [`writer::Writer`].
    ///
    /// Three gates: dirty, at most once every [`SNAPSHOT_SECS`], and a new revision. Unlike
    /// autosave there is no path gate: a snapshot goes to `.recovery/`, so a never-saved
    /// document is covered too.
    ///
    /// [`SNAPSHOT_SECS`]: crate::library::recovery::SNAPSHOT_SECS
    /// [`writer::Writer`]: crate::library::writer::Writer
    fn recovery_tick(&mut self, ctx: &egui::Context) {
        use crate::library::recovery;
        // The frame drains results ([`OndinApp::disk_results`]) first; a test driving this
        // directly has to drain them itself.
        if !self.session.is_dirty() {
            // Clean: the file on disk is the document, so the snapshot is stale.
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
                // The document's id (minted for a never-filed one), so the snapshot
                // matches back to its file.
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
        // Stamped whichever way it goes, so a failing folder backs off.
        self.recovery.last = Some(std::time::Instant::now());
        // `recovery.at` means "this revision is on disk", so only the worker's answer sets it.
        let at = self.session.revision();
        let doc = self.session.doc.clone();
        self.writer(ctx).snapshot(&root, &key, doc, at);
    }

    /// The background writer, started on the first job; the `Context` is cloned into the
    /// worker so a result can `request_repaint`.
    fn writer(&mut self, ctx: &egui::Context) -> &mut crate::library::writer::Writer {
        self.writer
            .get_or_insert_with(|| crate::library::writer::Writer::spawn(ctx))
    }

    /// Apply whatever the writer has finished, without waiting. Called at the top of the
    /// frame, ahead of both ticks, which gate on state a result changes.
    fn disk_results(&mut self) {
        let Some(writer) = self.writer.as_mut() else {
            return;
        };
        let done = writer.drain();
        self.apply_disk(done);
    }

    /// Block until the writer's queue is empty, then apply it. Only where there is no
    /// next frame (a close), where this frame writes the same file itself
    /// ([`Self::save_file`]), or where the library folder is about to move.
    pub(crate) fn disk_settle(&mut self) {
        let Some(writer) = self.writer.as_mut() else {
            return;
        };
        let done = writer.settle();
        self.apply_disk(done);
    }

    /// The one reader of [`writer::Done`].
    ///
    /// [`writer::Done`]: crate::library::writer::Done
    fn apply_disk(&mut self, done: Vec<crate::library::writer::Done>) {
        use crate::library::writer::Done;
        for msg in done {
            match msg {
                // Dropped with no key (finished after `drop_recovery`): a stale `at`
                // could silence the replacing document's first snapshot.
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
                // The export follows a landed save; a superseded write is not one.
                Done::Saved { path, at } => {
                    if self.session.finish_save(&path, at) && self.prefs.export_on_save {
                        self.export_all(crate::panels::ExportAll::Quietly);
                    }
                }
                // No latch, unlike the snapshot's: each failed save is reported.
                Done::SaveFailed(e) => {
                    self.session.save_failed();
                    self.session.fail(format!("Save failed: {e}"));
                }
            }
        }
    }

    /// *Recover unsaved work?* — drawn before the `View::Dashboard` return in
    /// [`eframe::App::ui`] so it appears over either screen.
    ///
    /// Recovering never writes the user's file: the snapshot loads with the original's
    /// path, marked unsaved ([`EditorSession::mark_unsaved`]), and is kept, adopted by
    /// [`Self::recovery_tick`]. [`Self::recover`] drops the outgoing session's snapshot,
    /// safe only behind [`Self::may_replace_current_document`], asked before `pending` pops.
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
            // The ✕ dismisses this launch's asking, never the snapshot ([`RecoverChoice::Later`]).
            if crate::settings::modal_title(ui, "Recover unsaved work") {
                decision = Some(RecoverChoice::Later);
            }
            ui.add_space(8.0);
            // No time clause for an unreadable stamp: `0` or `now` would both mislead.
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
        // Escape and the backdrop are *Later*, never *Discard*.
        if modal.should_close() {
            decision = Some(RecoverChoice::Later);
        }

        match decision {
            Some(RecoverChoice::Recover) => {
                // The guard runs before the pop: a *Recover* backed out of is not a *Later*.
                if self.may_replace_current_document("recovering another document") {
                    self.recovery.pending.pop();
                    self.recover(&pending);
                }
            }
            Some(RecoverChoice::Discard) => {
                self.recovery.pending.pop();
                self.forget_snapshot(&pending.key);
            }
            // Popped without deleting: the next launch offers it again.
            Some(RecoverChoice::Later) => {
                self.recovery.pending.pop();
            }
            None => {}
        }
    }

    /// Load a snapshot into the session, unsaved. Not an `open_path`: a recovery does
    /// not touch *Recent*, `prefs.last_document` or `collapse_groups_on_open`.
    fn recover(&mut self, pending: &crate::library::recovery::Pending) {
        let bytes = match std::fs::read(&pending.path) {
            Ok(b) => b,
            Err(e) => return self.session.fail(format!("Recovery failed: {e}")),
        };
        let doc = match ondin_core::io::load(&bytes) {
            Ok(d) => d,
            Err(e) => return self.session.fail(format!("Recovery failed: {e}")),
        };
        // The outgoing snapshot, dropped before the adopt overwrites `recovery.key`, or it
        // is offered at every launch forever. Safe only because the caller ran
        // `may_replace_current_document`; otherwise a second queued *Recover* would delete
        // the unsaved first one.
        self.drop_recovery();
        self.opening = None;
        self.session.adopt_document(doc, pending.target.clone());
        // After `adopt_document`, which sets it clean.
        self.session.mark_unsaved();
        self.reset_transient_state();
        self.ensure_document_fonts();
        // Adopted rather than re-minted, so the next tick keeps the same file current.
        self.recovery.key = Some(pending.key.clone());
        self.recovery.at = None;
        self.recovery.last = None;
        self.view = View::Editor;
        self.session.info("Unsaved changes recovered");
    }

    /// Forget this session's crash snapshot: the work it held is on disk, or the user has
    /// said to throw it away. The tick only runs on an editor frame, so whatever ends the
    /// session or replaces its document calls this itself ([`Self::close_confirmation`],
    /// [`Self::go_to_dashboard`], [`Self::adopt_opened`], [`Self::recover`]).
    pub(crate) fn drop_recovery(&mut self) {
        // `library.root` is read now, not remembered with the key: a moved library carried
        // the snapshot along.
        if let Some(key) = self.recovery.key.take() {
            self.forget_snapshot(&key);
        }
        self.recovery.at = None;
        self.recovery.last = None;
    }

    /// Delete the snapshot filed under `key` down the writer's queue, so an in-flight
    /// write cannot put it back. With no writer, the removal goes straight to disk.
    fn forget_snapshot(&mut self, key: &str) {
        let root = self.library.root.clone();
        match self.writer.as_mut() {
            Some(writer) => writer.forget(&root, key),
            None => crate::library::recovery::remove(&root, key),
        }
    }

    /// Leave the editor for the library, autosaving on the way out rather than asking;
    /// a never-filed document keeps its unsaved state in memory.
    ///
    /// The text session is committed first ([`Self::finish_text_first`], as every exit
    /// does): a save before it would miss what was typed and `drop_recovery` would take
    /// the last copy.
    pub(crate) fn go_to_dashboard(&mut self) {
        self.finish_text_first();
        if self.session.is_dirty() && self.session.path.is_some() {
            self.save_file(false);
        }
        // Gated on clean: a never-filed document was not saved above and its snapshot
        // is all it has.
        if !self.session.is_dirty() {
            self.drop_recovery();
        }
        self.view = View::Dashboard;
        self.library.refresh();
        self.library.reload_projects();
        // Once per visit, not per refresh: the sweep is a `read_dir` over the cover cache.
        self.covers.sweep(&self.library.entries);
        // Editor chrome that would otherwise float over the dashboard. An armed
        // eyedropper first: its input gate runs on every screen, and the library
        // has no canvas and no Escape to disarm it from.
        self.disarm_dropper();
        self.picker = None;
        self.context_menu = None;
        self.open_menu = TopMenu::None;
        self.rename_entry = None;
        // The close card too (reachable via `Ctrl+O`), or it returns over a clean document.
        self.confirming_close = false;
    }

    /// Open a document from the library into the editor. Reports failure and stays where
    /// it is, so the dashboard row can be tried again.
    pub(crate) fn open_path(&mut self, path: &std::path::Path) {
        if !self.may_replace_current_document("opening another document") {
            return;
        }
        // A card still loading loses to this one.
        self.opening = None;
        match read_opened(path, &self.library.root, self.prefs.history_size > 0) {
            Ok(opened) => self.adopt_opened_with(path, opened),
            Err(why) => self.session.fail(why),
        }
    }

    /// Open `path` with the read and the load on a worker, the library's card pulsing
    /// until [`Self::poll_open`] adopts it; a second click replaces the first.
    pub(crate) fn begin_open(&mut self, path: &std::path::Path) {
        if self.opening.as_ref().is_some_and(|o| o.path == path) {
            return;
        }
        if !self.may_replace_current_document("opening another document") {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let at = path.to_path_buf();
        let root = self.library.root.clone();
        let keep = self.prefs.history_size > 0;
        let spawned = std::thread::Builder::new()
            .name("ondin-open".into())
            .spawn(move || {
                let _ = tx.send(read_opened(&at, &root, keep));
            });
        match spawned {
            Ok(_) => {
                self.opening = Some(Opening {
                    path: path.to_path_buf(),
                    rx,
                    at: std::time::Instant::now(),
                });
            }
            // No thread to be had: open on this one.
            Err(_) => self.open_path(path),
        }
    }

    /// Adopt a document [`Self::begin_open`] finished loading, or report why it could not;
    /// keeps frames coming meanwhile, for the pulse.
    pub(crate) fn poll_open(&mut self, ctx: &egui::Context) {
        let Some(o) = &self.opening else {
            return;
        };
        match o.rx.try_recv() {
            Ok(loaded) => {
                let path = o.path.clone();
                self.opening = None;
                match loaded {
                    Ok(opened) => self.adopt_opened_with(&path, opened),
                    Err(why) => self.session.fail(why),
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint(),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.opening = None;
                self.session.fail("Load error: the reader stopped");
            }
        }
    }

    /// Wait for `Self::begin_open`'s worker and adopt what it read (tests only).
    #[cfg(test)]
    pub(crate) fn settle_open(&mut self) {
        let Some(o) = self.opening.take() else {
            return;
        };
        match o.rx.recv_timeout(std::time::Duration::from_secs(20)) {
            Ok(Ok(opened)) => self.adopt_opened_with(&o.path, opened),
            Ok(Err(why)) => self.session.fail(why),
            Err(_) => self.session.fail("Load error: the reader stopped"),
        }
    }

    /// [`Self::adopt_opened`], then take up the undo history kept for the file read.
    fn adopt_opened_with(&mut self, path: &std::path::Path, (doc, history): Opened) {
        self.adopt_opened(path, doc);
        if let Some(history) = history {
            self.session.resume_history(history);
        }
    }

    /// The half of an open after the bytes are a document: the library's bookkeeping, the
    /// session, and the editor.
    pub(crate) fn adopt_opened(&mut self, path: &std::path::Path, doc: ondin_core::Document) {
        if let Some(id) = doc.meta().id.clone() {
            self.library.local.mark_opened(&id);
            self.library.local.save();
        }
        // Before the adopt: past it the old snapshot's key cannot be named. Both callers
        // asked `may_replace_current_document` first.
        self.drop_recovery();
        self.session.adopt_document(doc, Some(path.to_path_buf()));
        self.reset_transient_state();
        self.reopen_last_page();
        // After the reset, which empties `collapsed`.
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
        // Fields holding the old document's ids are cleared by hand; one this list misses
        // dangles rather than collides (`NodeId` carries a random per-session actor).
        self.alt_clone = None;
        self.pen = None;
        self.text = None;
        self.mode = Mode::Normal;
        self.name_edit = None;
        self.desc_edit = None;
        self.collapsed.clear();
        self.layer_filter = None;
        self.layer_drag = None;
        self.pages = Default::default();
        self.picker = None;
        self.kit_picker = None;
        self.kit_placing = None;
        self.var_browser = None;
        self.var_complete = None;
        self.var_retype = None;
        self.field_menu = None;
        self.mode_menu = None;
        self.style_menu = None;
        self.style_picker = None;
        self.style_naming = None;
        // *Go to main in the kit* sets it again after the open that cleared it.
        self.kit_return = None;
        self.making_local = None;
        self.kits_used_open = false;
        self.versions_open = false;
        self.making_all_local = None;
        self.unsharing_kit = None;
        self.paint_hex = None;
        self.guide_previews.clear();
        self.var_cell_preview = None;
        self.per_corner_radius.clear();
        self.corner_smoothing_open.clear();
    }

    /// Whether the app may put a different document in front of the user: **save what
    /// can be saved, then ask about what cannot**. `false` means the user declined and
    /// the caller must change nothing.
    ///
    /// `is_dirty()` is read twice on purpose: `save_file` clears it, so only an unfiled
    /// document reaches the (blocking, `rfd`) dialog.
    fn may_replace_current_document(&mut self, what: &str) -> bool {
        self.finish_text_first();
        if self.session.is_dirty() && self.session.path.is_some() {
            self.save_file(false);
        }
        !self.session.is_dirty() || self.confirm_discard(what)
    }

    /// A blocking yes/no dialog to discard unsaved work. Only
    /// [`Self::may_replace_current_document`] calls it; the window close uses the in-app
    /// [`Self::close_confirmation`] card.
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

    /// The update chip's click, from either screen's bar: ask the window to close and
    /// apply the update once it has, or say why not.
    pub(crate) fn restart_to_update(&mut self) {
        // The count is of processes, headless `ondin export` included.
        if let crate::update::Restart::OthersOpen(n) = self.updater.restart() {
            let (count, them) = if n == 1 {
                ("Another Ondin is", "it")
            } else {
                ("Other copies of Ondin are", "them")
            };
            self.session.info(format!(
                "{count} running: close {them} first, since installing the update stops every Ondin"
            ));
        }
    }

    /// Intercept the window close so unsaved work is never lost silently.
    ///
    /// Re-armed on every close request: the title bar's ✕ is not covered by the card, and
    /// an uncancelled second click would close the window.
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
        // The title's ✕ is *Cancel*: a reflex must not land in the destructive arm.
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
        // Escape and the backdrop mean *Cancel*; unread, Escape would fall through to the
        // editor's ladder beneath the card.
        if modal.should_close() {
            decision = Some(CloseChoice::Cancel);
        }

        match decision {
            Some(CloseChoice::Save) => {
                self.save_file(false);
                self.confirming_close = false;
                // Close only if the save went through; otherwise a chip-asked restart is
                // called off as *Cancel* does.
                if self.session.is_dirty() {
                    self.updater.restart_called_off();
                }
                if !self.session.is_dirty() {
                    // Not in `recovery_tick`: there is no next frame.
                    self.drop_recovery();
                    self.disk_settle();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            Some(CloseChoice::Discard) => {
                self.confirming_close = false;
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
            Some(CloseChoice::Cancel) => {
                self.confirming_close = false;
                // Or the next close for any reason would apply the update.
                self.updater.restart_called_off();
            }
            None => {}
        }
    }

    /// Pick image files and load the cursor with them (`Tool::Image`). The tool is only
    /// armed if something loaded; a placement tool with nothing to place would look hung.
    pub(crate) fn place_image_action(&mut self) {
        let Some(paths) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif", "svg"])
            .pick_files()
        else {
            return;
        };
        let (mut loaded, mut failed, mut vector) = (Vec::new(), Vec::new(), Vec::new());
        for path in paths {
            // Vector art becomes layers now, where a paste would put them; only
            // pictures go on the Image tool's cursor.
            if let Ok(bytes) = std::fs::read(&path)
                && looks_like_svg(&bytes)
            {
                vector.push((image_name(&path), bytes));
                continue;
            }
            match self.load_image_file(&path) {
                Ok(image) => loaded.push(image),
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
        let n = loaded.len();
        if n > 0 {
            self.pending_images = loaded;
            self.choose_tool(Tool::Image);
            self.session.info(match n {
                1 => "Click to place the image".to_string(),
                n => format!("Click to place {n} images"),
            });
        }
        self.import_svg_files(vector, None, Vec::new(), n > 0);
    }

    /// Pick one file and give it to the picture in `slot`; also the repair for a picture
    /// that cannot be drawn (dangling id, undecodable bytes, moved link).
    ///
    /// `keep_framing: false` is *Replace and reset*: only the framing resets; the
    /// adjustments and sampler survive, being another card's. The table entry goes in the
    /// same transaction, so one undo restores both.
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
            // Rebuilt from `ImageRef::new` so a future framing field resets too.
            ondin_core::Brush::Image(_) => crate::panels::map_image(&brush, |r| {
                *r = ondin_core::ImageRef {
                    adjust: r.adjust,
                    ..ondin_core::ImageRef::new(image.id.clone())
                };
            }),
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
        self.session.name_step("Replace image", icon::IMAGE);
        if self.commit_edit(Transaction(ops)) {
            let said = self.replaced_status(image.label(), None);
            self.session.info(said);
        }
    }

    /// *Replaced with ‹label›*, and what the commit just made detached
    /// ([`EditorSession::last_detached`]), with `cut_var` (a variable the door read off
    /// the paint before writing over it) among the variables.
    fn replaced_status(&self, label: &str, cut_var: Option<String>) -> String {
        let (styles, mut vars) = self.session.last_detached().clone();
        if let Some(name) = cut_var
            && !vars.contains(&name)
        {
            vars.push(name);
        }
        let replaced = format!("Replaced with {label}");
        match crate::session::said_detached(styles, vars) {
            Some(detached) => format!("{replaced} · {detached}"),
            None => replaced,
        }
    }

    /// Step into image editing on `id`: select it, take the tool, say what the mode
    /// expects. The one funnel for every door. A lock refuses here, in the mode rather
    /// than at a door, since a crop is a resize; a locked group locks its picture.
    pub(crate) fn begin_image_edit(&mut self, id: NodeId) {
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

    // A picture is exported as a layer, through the Export panel.

    /// Read, hash and decode one file, or say why not ([`ImageRefusal`]).
    fn load_image_file(&mut self, path: &std::path::Path) -> Result<LoadedImage, ImageRefusal> {
        // An unreadable file and a non-picture share one message.
        let Ok(bytes) = std::fs::read(path) else {
            return Err(ImageRefusal::NotAnImage);
        };
        self.load_image_bytes(bytes, file_extension(path), Some(image_name(path)))
    }

    /// [`Self::load_image_file`] without the file: hash, sniff, decode and name bytes in
    /// hand — the one path for pick, drop and paste. `ext` is a second opinion when the
    /// magic number says nothing; `None` for a paste.
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
        // Decoded here: gives the intrinsic size and proves the file drawable.
        let (w, h) = self
            .canvas
            .images_mut()
            .insert(&id, &bytes, format)
            .map_err(ImageRefusal::Decode)?;
        Ok(LoadedImage {
            entry: ImageEntry {
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

    /// Take whatever was dropped on the window this frame and place it. Each
    /// `DroppedFile` carries bytes (a browser drag) or a path (a file manager). The
    /// point comes from [`Self::drop_point`] (no Windows drop event carries one), shared
    /// with [`Self::draw_drop_indicator`].
    pub(crate) fn take_dropped_images(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        let at = self.drop_point(ctx);
        let alone = dropped.len() == 1;
        let (mut loaded, mut failed, mut vector) = (Vec::new(), Vec::new(), Vec::new());
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
                // Vector art becomes layers, never a rasterized picture.
                Some(b) if looks_like_svg(&b) => vector.push((name, b)),
                Some(b) => match self.load_image_bytes(b, ext, Some(name.clone())) {
                    Ok(image) => loaded.push(image),
                    Err(why) => failed.push(why.message(&name)),
                },
                None => failed.push(ImageRefusal::NotAnImage.message(&name)),
            }
        }
        if !failed.is_empty() {
            self.session.fail(failed.join("; "));
        }
        let mut selected = Vec::new();
        let pictures = !loaded.is_empty();
        if pictures {
            self.drop_images_at(loaded, at, alone);
            selected.extend_from_slice(self.session.selection.ids());
        }
        self.import_svg_files(vector, at, selected, pictures);
    }

    /// Import dropped or picked SVG files as layers, cascaded from `at` as dropped
    /// pictures are, and select them beside `selected` (what the same gesture placed
    /// already). Each file says what it made, after the gesture's own report when
    /// `reported` says it wrote one; a file that will not read says why.
    fn import_svg_files(
        &mut self,
        files: Vec<(String, Vec<u8>)>,
        at: Option<Point>,
        mut selected: Vec<NodeId>,
        reported: bool,
    ) {
        if files.is_empty() {
            return;
        }
        let (mut said, mut failed) = (Vec::new(), Vec::new());
        for (i, (name, bytes)) in files.into_iter().enumerate() {
            let step = DROP_CASCADE * i as f64;
            let stem = std::path::Path::new(&name)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| name.clone());
            let outcome = match String::from_utf8(bytes) {
                Ok(text) => {
                    self.import_svg(&text, at.map(|p| p + Vec2::new(step, step)), Some(&stem))
                }
                Err(_) => crate::canvas::SvgImport::Unreadable,
            };
            match outcome {
                crate::canvas::SvgImport::Placed(made) => {
                    said.push(format!("Imported {name} — {made}"));
                    selected.extend_from_slice(self.session.selection.ids());
                }
                crate::canvas::SvgImport::TooDeep => {
                    failed.push(format!("{name} is nested too deeply to read"))
                }
                crate::canvas::SvgImport::Unreadable => {
                    failed.push(format!("{name} could not be read as SVG"))
                }
            }
        }
        if !failed.is_empty() {
            self.session.fail(failed.join("; "));
        }
        if said.is_empty() {
            return;
        }
        // The gesture's own report goes first; an error joins on its own.
        let shown = self.session.status();
        let placed = (reported && shown.kind == StatusKind::Info)
            .then(|| shown.text.clone())
            .filter(|t| !t.is_empty());
        said.splice(0..0, placed);
        self.session.selection.set(selected);
        self.session.info(said.join(crate::session::STATUS_JOIN));
    }

    /// Place dropped images, or hand one to the shape under the pointer: onto a shape it
    /// replaces that shape's picture, keeping the framing; onto bare canvas it makes
    /// layers, cascaded since N files arrive at one point. `may_replace` says the
    /// whole drop was this one file, the gate the drop pill reads (`drop_label`): a
    /// picture that came with an SVG or a refused file never replaces.
    fn drop_images_at(&mut self, loaded: Vec<LoadedImage>, at: Option<Point>, may_replace: bool) {
        let n = loaded.len();
        // A single picture replaces the fill under it, only with a real drop point:
        // the paste's selection-only `None` arm would replace whatever was selected.
        if n == 1
            && may_replace
            && let Some(p) = at
            && let Some(target) = self.image_drop_target(Some(p))
        {
            let image = &loaded[0];
            if let Some(said) = self.replace_image_fill(target, image) {
                self.session.info(said);
                return;
            }
        }
        // No point (over a panel, or an unreadable cursor): the view's centre.
        let p = at.unwrap_or_else(|| self.viewport_centre());
        // Never through `pending_images` (the Image tool's cursor). One drop is one
        // transaction; the document is advanced per file because index, free name and
        // embedded-blob checks read the tree.
        let mut advanced = self.session.doc.clone();
        let mut ops = Vec::new();
        let mut made = Vec::new();
        for (i, image) in loaded.iter().enumerate() {
            let step = DROP_CASCADE * i as f64;
            let (tx, node) = self.image_tx(Some(&advanced), image, p + Vec2::new(step, step));
            if advanced.apply(&tx).is_ok() {
                ops.extend(tx.0);
                made.push(node);
            }
        }
        let placed = made.len();
        if !self.session.has_step_name() {
            self.session.name_step("Place image", icon::IMAGE);
        }
        if placed == 0 || !self.session.commit(Transaction(ops)) {
            self.session.fail("Could not place the images");
            return;
        }
        self.session.selection.set(made);
        self.session.info(match placed {
            1 => "Placed image".to_string(),
            n => format!("Placed {n} images"),
        });
    }

    /// Put `image` into the fill `target` names, keeping how it was framed. Returns the
    /// status (*Replaced with ‹file›* plus what it detached), or `None` when there was
    /// nothing to replace. A fill bound to a colour variable is replaced whole, which the
    /// commit cannot report, so the variable is read here before the write.
    fn replace_image_fill(
        &mut self,
        target: (NodeId, usize),
        image: &LoadedImage,
    ) -> Option<String> {
        let (id, index) = target;
        let node = self.session.doc.get(id)?;
        let mut fills = node.paint().fills.clone();
        let fill = fills.get_mut(index)?;
        let known = self.session.doc.has_image(&image.id);
        let cut_var = fill
            .brush
            .var()
            .and_then(|v| self.session.doc.variables().get(v))
            .map(|v| v.name.clone());
        fill.brush = match fill.brush.literal() {
            Some(b @ ondin_core::Brush::Image(_)) => crate::panels::map_image(b, |r| {
                r.id = image.id.clone();
            }),
            // Not a picture yet (a bound one is detached): the defaults.
            _ => ondin_core::image_brush(image.id.clone()),
        }
        .into();
        let mut ops = Vec::new();
        if !known {
            ops.push(Operation::AddImage {
                id: image.id.clone(),
                entry: image.entry.clone(),
            });
        }
        ops.push(Operation::SetFills { id, fills });
        if !self.session.has_step_name() {
            self.session.name_step("Replace image", icon::IMAGE);
        }
        if !self.commit_edit(Transaction(ops)) {
            return None;
        }
        Some(self.replaced_status(image.label(), cut_var))
    }

    /// Paste an image off the OS clipboard, if there is one; returns whether it took the
    /// paste, so the caller can fall through to the in-app layer paste.
    ///
    /// `at` is *Paste here*'s pointer: `None` (the chord) aims by the selection and lands
    /// mid-view; `Some(p)` aims like a drop ([`Self::image_drop_target`]).
    ///
    /// Read through `arboard`, since `egui::Event::Paste` is text-only. The decoded RGBA
    /// is re-encoded to lossless PNG, as a document stores encoded originals, so a pasted
    /// image's id hashes our encoding.
    pub(crate) fn paste_image(&mut self, at: Option<Point>) -> bool {
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
        // No message: a refusal of our own fresh PNG is a bug, not user news.
        let Ok(image) = self.load_image_bytes(png, None, None) else {
            return false;
        };
        if let Some(target) = self.image_drop_target(at)
            && let Some(said) = self.replace_image_fill(target, &image)
        {
            self.session.info(said);
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

    /// Load the Settings modal's default font ahead of the first text layer, so
    /// that layer is drawn in it from its first frame. Called when the font lists
    /// change, the launch scan's landing included (a family nothing lists yet cannot
    /// load), and when the preference is saved.
    pub(crate) fn ensure_default_font(&mut self) {
        self.fonts.ensure_loaded(&self.prefs.default_font);
    }

    /// The attributes a new text layer starts with, in the default font
    /// (`FontService::new_text_parts`).
    pub(crate) fn new_text_parts(&self) -> ondin_core::text::TextParts {
        self.fonts.new_text_parts(&self.prefs.default_font)
    }

    /// Every family the document's text nodes name, lowercased, for
    /// [`Self::ensure_document_fonts`] and the font-poll gate in `OndinApp::ui`. Reads
    /// the character spans and bound families too: a partial-range family lives only in
    /// a `CharAttr::Family` span.
    fn document_families(&self) -> std::collections::HashSet<String> {
        let mut families = std::collections::HashSet::new();
        // Every page: switching to one must not wait for its fonts.
        let mut stack: Vec<NodeId> = self.session.doc.page_ids().collect();
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
            bound_families(&self.session.doc, node, &mut families);
            stack.extend(node.children().iter().copied());
        }
        families
    }
}

/// The families a text's bound family can draw — its string variable's value in
/// every mode, lowercased, into `families`, so a mode switch finds the font loading.
pub(crate) fn bound_families(
    doc: &ondin_core::Document,
    node: &ondin_core::Node,
    families: &mut std::collections::HashSet<String>,
) {
    if let Some(var) = node.text_vars()[1] {
        for v in doc.variables().every_value(var) {
            if let Some(s) = v.as_str() {
                families.insert(s.to_lowercase());
            }
        }
    }
}

/// A path's extension, lowercased later by [`image_format_of`].
fn file_extension(path: &std::path::Path) -> Option<String> {
    path.extension().and_then(|e| e.to_str()).map(str::to_owned)
}

/// World units between one dropped layer and the next, not scaled by zoom.
const DROP_CASCADE: f64 = 24.0;

/// What a layer made from this file is called: its own name.
fn image_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Image".to_string())
}

/// Whether a byte run is SVG, sniffed from the bytes (a browser drop's name is not
/// trustworthy). A byte adapter over `svg_in::looks_like_svg`, capped at 512 bytes and
/// decoded lossily because the cap can split a multi-byte character.
fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(512)];
    ondin_core::svg_in::looks_like_svg(&String::from_utf8_lossy(head))
}

/// Why a byte run did not become a placeable picture: not a picture at all, or one
/// Ondin will not draw (with `DecodeError`'s reason).
#[derive(Debug)]
pub(crate) enum ImageRefusal {
    /// Neither the magic bytes nor the extension name a format this app reads.
    NotAnImage,
    /// A format this app reads, refused by the decoder or by a renderer bound.
    Decode(ondin_render::images::DecodeError),
}

impl ImageRefusal {
    /// What to tell the user about `name`, as one sentence with the numbers.
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
/// `Clone` so the tool can hand one on without borrowing the queue.
#[derive(Clone)]
pub(crate) struct LoadedImage {
    pub(crate) id: ImageId,
    pub(crate) entry: ImageEntry,
    /// The decoded pixel size, which is what the fit rule measures.
    pub(crate) intrinsic: ondin_core::kurbo::Size,
    /// The file's own name, which becomes the layer's. [`None`] for a paste, which
    /// [`OndinApp::create_image_at`] numbers like any other new layer.
    pub(crate) name: Option<String>,
}

impl LoadedImage {
    /// What a status message calls this picture (not the layer name, which is numbered
    /// against siblings).
    pub(crate) fn label(&self) -> &str {
        self.name.as_deref().unwrap_or("the pasted image")
    }
}

/// A zoom typed as `150`, `150%` or `37.5 %`; `None` for anything that is not a
/// positive finite number.
fn parse_zoom_percent(typed: &str) -> Option<f64> {
    let n: f64 = typed.trim().trim_end_matches('%').trim_end().parse().ok()?;
    (n.is_finite() && n > 0.0).then_some(n)
}

/// The families text layer `id` is set in, lowercased: its own (through a bound
/// family) and every run's. Empty for any other kind — what *Select same font* matches.
fn text_families(doc: &ondin_core::Document, id: NodeId) -> Vec<String> {
    let Some(kind) = doc.resolved_kind(id) else {
        return Vec::new();
    };
    let NodeKind::Text { style, spans, .. } = kind.as_ref() else {
        return Vec::new();
    };
    let mut out = vec![style.font_family.to_lowercase()];
    for span in spans.as_slice() {
        if let ondin_core::CharAttr::Family(name) = &span.attr {
            let name = name.to_lowercase();
            if !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

/// What *Paste properties*' bindings land as in `doc` (`variable::rebind`): the
/// names rebound to another variable of the same name or kit origin, and the names
/// detached where `doc` has none. A binding kept as itself is neither.
fn properties_landing(
    doc: &ondin_core::Document,
    props: &ondin_core::build::Properties,
) -> (Vec<String>, Vec<String>) {
    let t = doc.variables();
    let bound = props
        .fills
        .iter()
        .map(|f| &f.brush)
        .chain(props.strokes.iter().map(|s| &s.brush))
        .filter_map(ondin_core::BrushSource::var)
        .chain(props.strokes.iter().filter_map(|s| s.width_var))
        .chain(
            props
                .effects
                .iter()
                .flat_map(|e| e.vars.into_iter().flatten()),
        )
        .chain(props.opacity_var);
    let (mut rebound, mut detached) = (Vec::new(), Vec::new());
    for v in bound {
        let Some(c) = props.carried.iter().find(|c| c.id == v) else {
            continue;
        };
        let list = match ondin_core::variable::rebind(t, &props.carried, v) {
            Some(to) if to == v => continue,
            Some(_) => &mut rebound,
            None => &mut detached,
        };
        if !list.contains(&c.name) {
            list.push(c.name.clone());
        }
    }
    rebound.sort();
    detached.sort();
    (rebound, detached)
}

/// The variables `tx`'s fill, stroke, effect and opacity writes take off the layers they
/// write (a target's own bound paint, stroke width, effect number or opacity), which the
/// commit cannot name.
fn paints_overwritten(doc: &ondin_core::Document, tx: &Transaction) -> Vec<String> {
    use ondin_core::BrushSource;
    let t = doc.variables();
    let mut names: Vec<String> = Vec::new();
    for op in &tx.0 {
        let (had, now): (Vec<_>, Vec<_>) = match op {
            Operation::SetFills { id, fills } => match doc.get(*id) {
                Some(n) => (
                    n.paint()
                        .fills
                        .iter()
                        .filter_map(|f| f.brush.var())
                        .collect(),
                    fills.iter().filter_map(|f| f.brush.var()).collect(),
                ),
                None => continue,
            },
            Operation::SetStrokes { id, strokes } => match doc.get(*id) {
                Some(n) => (
                    n.paint()
                        .strokes
                        .iter()
                        .flat_map(|s| s.brush.var().into_iter().chain(s.width_var))
                        .collect(),
                    strokes
                        .iter()
                        .flat_map(|s| BrushSource::var(&s.brush).into_iter().chain(s.width_var))
                        .collect(),
                ),
                None => continue,
            },
            Operation::SetOpacityVar { id, var } => match doc.get(*id) {
                Some(n) => (
                    n.opacity_var().into_iter().collect(),
                    var.iter().copied().collect(),
                ),
                None => continue,
            },
            Operation::SetEffects { id, effects } => match doc.get(*id) {
                Some(n) => (
                    n.effects()
                        .iter()
                        .flat_map(|e| e.vars.into_iter().flatten())
                        .collect(),
                    effects
                        .iter()
                        .flat_map(|e| e.vars.into_iter().flatten())
                        .collect(),
                ),
                None => continue,
            },
            _ => continue,
        };
        for v in had.into_iter().filter(|v| !now.contains(v)) {
            if let Some(var) = t.get(v)
                && !names.contains(&var.name)
            {
                names.push(var.name.clone());
            }
        }
    }
    names.sort();
    names
}

/// The document-wide id for these bytes: `sha256:<hex>` of the encoded original, never
/// the decode (not bit-exact across decoder versions), so `sha256sum` on an exported
/// original reproduces the stored id.
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

/// Which of the four formats a picked file is. The bytes decide, as the decoder
/// will; the extension answers only when the magic number says nothing.
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
        // Unaccepted or unknown: a wrong extension guess costs only a decode failure.
        _ => by_ext(),
    }
}

enum CloseChoice {
    Save,
    Discard,
    Cancel,
}

/// What the user did with a crash snapshot (`OndinApp::recovery_modal`). `Later` is what
/// the ✕, `Escape` and the backdrop mean, and must never collapse into `Discard`.
enum RecoverChoice {
    /// Load the snapshot into the session. The file on disk is not written.
    Recover,
    /// Delete the snapshot. The only thing that does.
    Discard,
    /// Stop asking this launch; the snapshot is offered again next time.
    Later,
}

/// The save dot's tooltip. `ago` is `None` when nothing on disk matches the current
/// state. `saving` is a third label, not a third dot state.
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
        // Each band rounds half-up, so it must open inside `[unit/2, 3·unit/2)` to be
        // able to print `1`.
        45..=2699 => format!("Saved · {}m ago", (secs + 30) / 60),
        2700..=79199 => format!("Saved · {}h ago", (secs + 1800) / 3600),
        _ => format!("Saved · {}d ago", (secs + 43200) / 86400),
    }
}

// --- chrome ---------------------------------------------------------------

/// Height of the top bar; the floating inspector positions itself below it.
const TOP_BAR_H: f32 = 46.0;

/// Inner padding of every top-bar dropdown.
const MENU_PAD: f32 = 5.0;

/// The gap between every control in the top bar's right-hand cluster, including a mark
/// and its own caret, so the gaps read equal. The rules get [`TOP_RULE_GAP`] on top. The
/// library's bar uses it between Settings and the update pill.
pub(crate) const TOP_GAP: f32 = 5.0;

/// Extra space on each side of a rule, over and above [`TOP_GAP`]: a 1px rule
/// between 26px boxes otherwise reads as stuck to the nearer icon.
const TOP_RULE_GAP: f32 = 3.0;

/// One of the top bar's dividers, with [`TOP_RULE_GAP`] on each side.
fn top_rule(ui: &mut egui::Ui) {
    ui.add_space(TOP_RULE_GAP);
    crate::ui::rule(ui, 16.0);
    ui.add_space(TOP_RULE_GAP);
}

/// How long the floating status bar holds `status`: a base plus a reading allowance per
/// character, and longer for a failure.
pub(crate) fn status_hold(status: &crate::session::Status) -> std::time::Duration {
    let chars = status.text.chars().count() as f32;
    let secs = (2.0 + chars * 0.05).clamp(3.0, 8.0);
    let secs = if status.kind == StatusKind::Error {
        (secs * 1.5).max(5.0)
    } else {
        secs
    };
    std::time::Duration::from_secs_f32(secs)
}

/// The fade at the end of [`status_hold`].
const STATUS_FADE: f32 = 0.3;

/// The colour a status is drawn in, shared by the library's dot and the editor's
/// [`OndinApp::status_bar`].
fn status_color(status: &crate::session::Status) -> egui::Color32 {
    if status.kind == StatusKind::Error {
        egui::Color32::from_rgb(226, 138, 138)
    } else {
        theme::text::FAINT
    }
}

/// The library's half of the status line: a dot with the whole message on hover, in a
/// fixed box so it shows at every window width. A painted circle, not an icon glyph
/// (`Phosphor.ttf` has no glyph names). The hit box ([`STATUS_DOT_HIT`]) is far larger
/// than the ink ([`STATUS_DOT_R`]).
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

/// The library status dot's radius: an 8pt mark, the cap height of the status text.
pub(crate) const STATUS_DOT_R: f32 = 4.0;

/// The box [`status_dot`] allocates, matched to the header's other controls
/// (`HEADER_CONTROL_H`).
const STATUS_DOT_HIT: f32 = 28.0;

/// Should a panel `width` points wide, in a parent with `room` to spare, replace the
/// `stored` one? Only once the drag is released (`down`), when the window is not what
/// clamped it (`room`), and when it changed (`stored`). Built from numbers because
/// egui's resize `Response` id is a private salt; the 0.5 tolerances absorb whole-pixel
/// rounding of panel rects.
fn width_to_store(width: f32, room: f32, stored: f32, down: bool) -> Option<f32> {
    let user_did_it = !down && width < room - 0.5;
    (user_did_it && (width - stored).abs() >= 0.5).then_some(width)
}

/// How many instances a paste landed detached (linked in what was copied, no link where
/// they landed). Read off the result: each created subtree is matched to its template
/// node for node, and an instance counts once, not per member.
fn landed_detached(
    doc: &ondin_core::Document,
    placements: &[build::Placement],
    created: &[NodeId],
) -> usize {
    placements
        .iter()
        .zip(created)
        .map(|(p, c)| {
            let now = doc.capture_subtree(*c).unwrap_or_default();
            if now.len() != p.nodes.len() {
                return 0;
            }
            let lost: std::collections::HashSet<NodeId> = p
                .nodes
                .iter()
                .zip(&now)
                .filter(|(t, n)| t.link().is_some() && n.link().is_none())
                .map(|(t, _)| t.id())
                .collect();
            p.nodes
                .iter()
                .filter(|t| lost.contains(&t.id()))
                .filter(|t| !t.parent().is_some_and(|q| lost.contains(&q)))
                .count()
        })
        .sum()
}

impl OndinApp {
    /// End a live text session before acting on the document: its buffer is not yet in
    /// the `Document`, so an edit, save or undo under it would lose the typing. Finishing
    /// rather than dimming, since a control that visibly does nothing reads as broken.
    pub(crate) fn finish_text_first(&mut self) {
        if self.text.is_some() {
            self.finish_text_edit();
        }
    }

    /// The top bar: brand mark, breadcrumb, save dot, edit actions, zoom readout and the
    /// Settings button — and the window's own title bar: drag strip, caption buttons and
    /// the update chip.
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let chrome = crate::chrome::Chrome::current();
        egui::Panel::top("topbar")
            .exact_size(TOP_BAR_H)
            .resizable(false)
            .frame(crate::chrome::bar_frame(color::TOPBAR, 14.0, chrome))
            .show(ui, |ui| {
                // Laid first so every control added below keeps its own clicks.
                crate::chrome::drag_strip(ui, crate::chrome::bar_rect(ui, 14.0, chrome), true);
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;

                    // Brand mark: the way back to the library. The 12pt gap to the crumb is
                    // set before the mark is placed, since egui applies spacing at placement.
                    ui.spacing_mut().item_spacing.x = 12.0;
                    let (mark, mark_resp) =
                        ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::click());
                    ui.spacing_mut().item_spacing.x = 8.0;
                    crate::ui::logo(ui, mark, mark_resp.hovered());
                    if mark_resp.on_hover_text("Library (Ctrl+O)").clicked() {
                        self.go_to_dashboard();
                    }

                    // `Project / Document`, the project half a link.
                    self.document_crumb(ui);

                    self.save_dot(ui);
                    // On a kit, its tag and *Publish N* follow the dot as one group.
                    if self.shared_kit().is_some() {
                        crate::kits::kit_tag(ui);
                        ui.add_space(-2.0);
                        self.publish_button(ui);
                    }
                    self.kit_update_pill(ui);
                    // In a kit opened by *Go to main*, the way back.
                    self.kit_back_link(ui);

                    // Right cluster, reading undo/redo · View · Snap · zoom · Settings;
                    // laid out `right_to_left`, so added in mirror order.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = TOP_GAP;

                        // The window's own buttons hard against the right edge, then
                        // the bar's 14pt margin.
                        crate::chrome::caption_buttons(ui, chrome, true);
                        crate::chrome::after_buttons(ui, 14.0, TOP_GAP);

                        self.settings_button(ui);
                        if crate::update::chip(ui, self.updater.state()) {
                            self.finish_text_first();
                            self.restart_to_update();
                        }
                        // Extra air separates the app's controls from the document's.
                        ui.add_space(10.0);

                        self.zoom_control(ui);
                        top_rule(ui);
                        self.view_and_snap_menus(ui);
                        top_rule(ui);

                        // Only the buttons' enabled state; the arms finish the text
                        // session (itself an undo step) before acting.
                        let can_undo = self.session.history.can_undo();
                        let can_redo = self.session.history.can_redo();
                        // Redo first: the mirror again.
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

    /// The zoom readout (`62% ⌄`) and the menu it opens; the actions are all on the
    /// keyboard, so the bar only makes them discoverable.
    fn zoom_control(&mut self, ui: &mut egui::Ui) {
        const ITEM_H: f32 = crate::ui::MENU_ITEM_H;
        const MENU_W: f32 = 150.0;

        let ctx = ui.ctx().clone();
        let zoom = (self.session.camera.zoom * 100.0).round() as i32;
        // Readout and caret `TOP_GAP` apart, like the View and Snap heads.
        let head = crate::ui::menu_head(
            ui,
            egui::RichText::new(format!("{zoom}%"))
                .size(11.5)
                .color(theme::text::MUTED),
            TOP_GAP,
            self.open_menu == TopMenu::Zoom,
        )
        .on_hover_text("Zoom");
        if head.clicked() {
            self.toggle_menu(TopMenu::Zoom);
        }
        if self.open_menu != TopMenu::Zoom {
            self.zoom_entry = None;
            return;
        }
        // Right-aligned under the readout.
        let anchor = egui::pos2(
            head.rect.right() - MENU_W,
            head.rect.bottom() + crate::ui::MENU_GAP,
        );
        self.dropdown(&ctx, "zoom-menu", anchor, MENU_W, &head, |app, ui| {
            // The live zoom until the field is focused, then what is typed; Enter applies it.
            let shown = format!("{zoom}%");
            let mut text = app.zoom_entry.take().unwrap_or_else(|| shown.clone());
            let field = crate::ui::text_field(
                ui,
                egui::vec2(ui.available_width(), ITEM_H),
                &mut text,
                "Zoom",
                11.5,
            );
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                // Untouched, it reads the rounded zoom: applying it would nudge the camera.
                if text != shown {
                    app.apply_typed_zoom(&text);
                }
                app.open_menu = TopMenu::None;
            }
            app.zoom_entry = field.has_focus().then_some(text);
            ui.add_space(3.0);
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
                    "Zoom to fit",
                    crate::input::Action::ZoomFit,
                ),
                // `SELECTION`, not a guessed `corners-in`: a wrong Phosphor codepoint renders blank.
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

    /// Zoom to the percentage typed in the zoom dropdown, about the canvas's centre;
    /// clamped as every zoom is. Anything that does not read as a percentage says so.
    pub(crate) fn apply_typed_zoom(&mut self, typed: &str) {
        let Some(percent) = parse_zoom_percent(typed) else {
            if !typed.trim().is_empty() {
                self.session
                    .info("Type a zoom percentage, like 150".to_string());
            }
            return;
        };
        let factor = percent / 100.0 / self.session.camera.zoom;
        self.session
            .camera
            .zoom_by(factor, self.canvas_center(), self.canvas_px);
    }

    /// Open `which`, or close it if it is already showing; [`TopMenu`] holds one
    /// value, so two dropdowns never overlap.
    pub(crate) fn toggle_menu(&mut self, which: TopMenu) {
        self.open_menu = if self.open_menu == which {
            TopMenu::None
        } else {
            which
        };
    }

    /// A top-bar dropdown: the floating panel, closed by a click outside it or Escape.
    /// `width` is the painted width, border included ([`crate::ui::menu_inner_w`]
    /// derives the inner one).
    pub(crate) fn dropdown(
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

        // The head toggles on its own click; counting it here would reopen it.
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

    /// The **View** and **Snap** dropdowns, each an icon and a caret over a column of
    /// checkable rows. Focus mode is not a peer of the other switches: it hides all
    /// chrome and leaves them untouched, so Escape (this menu being hidden) restores them.
    fn view_and_snap_menus(&mut self, ui: &mut egui::Ui) {
        const ITEM_H: f32 = crate::ui::MENU_ITEM_H;
        const MENU_W: f32 = 160.0;

        let ctx = ui.ctx().clone();

        // Snap first: in the `right_to_left` cluster the pair then reads View · Snap.
        for which in [TopMenu::Snap, TopMenu::View] {
            let (glyph, tip) = match which {
                TopMenu::View => (icon::EYE, "View"),
                _ => (icon::MAGNET, "Snap"),
            };
            // `TOP_GAP` before the caret, matching the zoom readout's.
            let head = crate::ui::menu_head(
                ui,
                theme::icon_text(glyph, 16.0, theme::text::MUTED),
                TOP_GAP,
                self.open_menu == which,
            )
            .on_hover_text(tip);
            if head.clicked() {
                self.toggle_menu(which);
            }
            if self.open_menu != which {
                continue;
            }
            // Left-aligned under the icon.
            let anchor = egui::pos2(
                head.rect.left() - 8.0,
                head.rect.bottom() + crate::ui::MENU_GAP,
            );
            // 160, or wider when a label needs it.
            let labels: Vec<&str> = match which {
                TopMenu::View => ViewSwitch::VIEW_MENU
                    .iter()
                    .map(|s| s.label())
                    .chain([crate::menu::Item::Present.spec().label])
                    .collect(),
                _ => ViewSwitch::SNAP_MENU.iter().map(|s| s.label()).collect(),
            };
            let width = crate::ui::menu_check_width(&ctx, &labels, MENU_PAD, MENU_W);
            self.dropdown(
                &ctx,
                match which {
                    TopMenu::View => "view-menu",
                    _ => "snap-menu",
                },
                anchor,
                width,
                &head,
                |app, ui| {
                    // From [`ViewSwitch`], which the chords resolve to.
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
                    // A verb after the switches, in their column: no tick, ever.
                    if which == TopMenu::View
                        && crate::ui::menu_check(
                            ui,
                            crate::menu::Item::Present.spec().label,
                            false,
                            ITEM_H,
                        )
                    {
                        app.open_menu = TopMenu::None;
                        let ctx = ui.ctx().clone();
                        app.dispatch(&ctx, crate::input::Action::Present);
                    }
                },
            );
        }
    }

    /// Whether a modal card is up: anything that paints an `egui::Modal` with a backdrop.
    /// Not `edited_image` (a canvas mode), and not `ctx.egui_wants_keyboard_input()`,
    /// which is false for a modal with buttons and no text field.
    pub(crate) fn modal_is_up(&self) -> bool {
        self.settings.is_some()
            || self.confirming_close
            || !self.recovery.pending.is_empty()
            || self.deleting_value.is_some()
            || self.pages.deleting.is_some()
            || self.publishing.is_some()
            || self.making_local.is_some()
            || self.kits_used_open
            || self.versions_open
            || self.making_all_local.is_some()
            || self.unsharing_kit.is_some()
            || self.cheatsheet.is_some()
    }

    /// Write the View and Snap switches to `prefs` when one has changed, so the next
    /// launch opens on them. Once a frame rather than in [`Self::set_view_switch`]: a
    /// guide pulled out of a ruler turns guides on, and a door shows the layers panel.
    fn remember_view_switches(&mut self) {
        let now = crate::prefs::ViewPrefs {
            rulers: self.show_rulers,
            guides: self.show_guides,
            lock_guides: self.lock_guides,
            grid: self.show_grid,
            layout_grids: self.show_layout_grids,
            layers: self.show_layers,
            history: self.show_history,
            toolbar: self.show_toolbar,
            status: self.show_status,
            snap_shapes: self.snap_shapes,
            snap_guides: self.snap_guides,
            snap_grid: self.snap_grid,
            snap_baselines: self.snap_baselines,
        };
        if now != self.prefs.view {
            self.prefs.view = now;
            self.prefs.save();
        }
    }

    /// Read a workspace switch; the menus tick their rows from here.
    pub(crate) fn view_switch(&self, sw: ViewSwitch) -> bool {
        match sw {
            ViewSwitch::Rulers => self.show_rulers,
            ViewSwitch::Guides => self.show_guides,
            ViewSwitch::GuideLock => self.lock_guides,
            ViewSwitch::Grid => self.show_grid,
            ViewSwitch::LayoutGrid => self.show_layout_grids,
            ViewSwitch::Outline => self.outline_view,
            ViewSwitch::Layers => self.show_layers,
            ViewSwitch::History => self.show_history,
            ViewSwitch::Toolbar => self.show_toolbar,
            ViewSwitch::StatusMessages => self.show_status,
            ViewSwitch::Focus => self.focus,
            ViewSwitch::SnapShapes => self.snap_shapes,
            ViewSwitch::SnapGuides => self.snap_guides,
            ViewSwitch::SnapGrid => self.snap_grid,
            ViewSwitch::SnapBaselines => self.snap_baselines,
        }
    }

    /// Set a workspace switch, and say so — a chord has no tick to show. Guide lock
    /// and focus mode report their own.
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
            ViewSwitch::Outline => self.outline_view = on,
            ViewSwitch::Layers => self.show_layers = on,
            ViewSwitch::History => {
                self.show_history = on;
                // History lives in the layers pane, so showing it shows the pane.
                self.show_layers |= on;
            }
            ViewSwitch::Toolbar => self.show_toolbar = on,
            ViewSwitch::StatusMessages => {
                self.show_status = on;
                // Off, the toast would go into the bar it just silenced.
                if !on {
                    return;
                }
            }
            ViewSwitch::Focus => {
                self.focus = on;
                // The menu goes with the chrome it lives in.
                self.open_menu = TopMenu::None;
                self.session.info(if on {
                    "Focus mode — Escape to leave"
                } else {
                    "Left focus mode"
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

    /// Lock or unlock the guides — *View ▸ Lock guides* and `Ctrl+Alt+;`. Locking drops
    /// the guide selection (a locked guide is unselectable, so one left selected could
    /// never be picked up again) but never the user's layers.
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

    /// The breadcrumb: `Project / Document`, or just the document. The project segment
    /// opens the library on that project; it is allocated at its measured width, since a
    /// `ui.label` reports no hover or click.
    fn document_crumb(&mut self, ui: &mut egui::Ui) {
        let title = self.session.document_title();
        // From the document's own meta, not the library entry, which an unsaved or
        // just-moved document lacks or has stale.
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
            // No cursor change: the brightening ink is the affordance.
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
            ui.label(
                egui::RichText::new("/")
                    .size(13.0)
                    .color(theme::text::FAINT),
            );
        }

        self.document_menu(ui, title);
    }

    /// The save dot beside the document name: red while the document differs from its
    /// file (including during a save), green once it matches; the tooltip is
    /// [`save_label`]. The dot sits at the left edge of a larger hover target, so the gap
    /// to the name stays the row's 8pt.
    fn save_dot(&self, ui: &mut egui::Ui) {
        const DOT: f32 = 7.0;
        const HIT: f32 = 14.0;

        let ago = self.session.saved_ago();
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(HIT, HIT), egui::Sense::hover());
        ui.painter().circle_filled(
            egui::pos2(rect.left() + DOT * 0.5, rect.center().y),
            DOT * 0.5,
            match ago {
                None => color::UNSAVED,
                Some(_) => color::SAVED,
            },
        );
        // Formatted only while hovered.
        if resp.hovered() {
            resp.on_hover_text(save_label(self.session.is_saving(), ago));
        }
    }

    /// The floating status bar at the canvas's bottom left: the mode when not the
    /// default, and the last status message for [`status_hold`], then a fade.
    /// *View ▸ Show messages* hides the message, never the mode. Drawn in focus
    /// mode too (its Escape hint). Not interactable; truncated so it never runs under the
    /// inspector.
    fn status_bar(&self, ctx: &egui::Context, canvas: egui::Rect) {
        const MARGIN: f32 = 12.0;
        let status = self.session.status();
        let moded = self.mode != Mode::Normal;
        let age = status.at.elapsed().as_secs_f32();
        let hold = status_hold(status).as_secs_f32();
        let message = self.show_status && !status.text.is_empty() && age < hold;
        if !moded && !message {
            return;
        }
        let fade = if message {
            ((hold - age) / STATUS_FADE).clamp(0.0, 1.0)
        } else {
            1.0
        };
        if message {
            // Wake for the fade, and for the frame the bar goes.
            ctx.request_repaint_after(std::time::Duration::from_secs_f32(if fade < 1.0 {
                1.0 / 60.0
            } else {
                hold - age - STATUS_FADE
            }));
        }
        let max_w = (canvas.width() * 0.6).min(560.0);
        egui::Area::new(egui::Id::new("status-bar"))
            .order(egui::Order::Middle)
            .interactable(false)
            // Its own fade only: egui's appear fade would dim the mode.
            .fade_in(false)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(canvas.left_bottom() + egui::vec2(MARGIN, -MARGIN))
            .show(ctx, |ui| {
                // The card fades only when the message is all it holds.
                let message_fade = if moded {
                    fade
                } else {
                    ui.set_opacity(fade);
                    1.0
                };
                crate::ui::menu_frame(0.0)
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.set_max_width(max_w);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            if moded {
                                ui.label(
                                    egui::RichText::new(self.mode.label())
                                        .size(9.5)
                                        .color(color::ACCENT_300),
                                );
                            }
                            if message {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&status.text).size(11.5).color(
                                            status_color(status).gamma_multiply(message_fade),
                                        ),
                                    )
                                    .truncate(),
                                );
                            }
                        });
                    });
            });
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
            // With Select and Hand: it manipulates rather than draws.
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
            // No points button: like `Tool::ImageEdit`, `Tool::Node` is entered on a
            // subject (double-click, `Enter`, the context menu), and the path bar is
            // its chrome.
            E::Btn(icon::TEXT_T, Some(Tool::Text), "Text (T)"),
            E::Btn(
                icon::IMAGE,
                Some(Tool::Image),
                "Place image — pick files, then click to place each (Ctrl+Shift+K)",
            ),
            // No crop button: `Tool::ImageEdit` needs a subject (double-click or `Enter`).
            E::Sep,
            E::Btn(
                icon::EYEDROPPER,
                Some(Tool::Eyedropper),
                "Eyedropper — pick a colour onto the selection (I)",
            ),
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
                            E::Sep => crate::path_bar::separator(ui),
                            E::Btn(glyph, tool, tip) => {
                                // With the pen armed inside a node edit both are lit.
                                let active = tool.is_some_and(|t| {
                                    t == self.tool || (self.pen_bias && t == Tool::Pen)
                                });
                                let enabled = tool.is_some();
                                let resp = icon_button(ui, glyph, 30.0, 20.0, active, enabled)
                                    .on_hover_text(*tip);
                                // The path bar centres itself on the Pen button.
                                if *tool == Some(Tool::Pen) {
                                    self.pen_button = resp.rect;
                                }
                                if resp.clicked() {
                                    match tool {
                                        // Arming the image tool is picking files.
                                        Some(Tool::Image) => self.place_image_action(),
                                        Some(t) => self.choose_tool(*t),
                                        None => self.session.info(*tip),
                                    }
                                }
                            }
                        }
                    }
                });
                // The insert picker's toggle at the rail's foot.
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    let open = self.kit_picker.is_some();
                    let resp = icon_button(ui, icon::HEXAGON, 30.0, 20.0, open, true)
                        .on_hover_text("Insert components (Shift+I)");
                    self.kit_button = resp.rect;
                    if resp.clicked() {
                        self.toggle_kit_picker();
                    }
                    // The styles and variables browser's, directly above it.
                    let open = self.var_browser.is_some();
                    let resp = icon_button(ui, icon::SWATCHES, 30.0, 20.0, open, true)
                        .on_hover_text("Styles and variables (Shift+S)");
                    self.var_button = resp.rect;
                    if resp.clicked() {
                        self.toggle_var_browser();
                    }
                });
            });
    }

    /// The layers panel: header + the node tree, resizable within
    /// `layers::MIN_W ..= MAX_W` and remembered in `prefs.json`. `set_min_width` is what
    /// makes the drag hold: `Panel` takes its rect from the content, so narrower content
    /// snaps it back.
    fn layers_panel(&mut self, ui: &mut egui::Ui) {
        // Before the panel is shown: egui measures its clamp against this.
        let room = ui.available_rect_before_wrap().width();

        // The drag edge's lit states, retuned to the accent border colours on the parent
        // ui (restored after: the rail, canvas and inspector still draw into it). Hover
        // lights only after `EDGE_DWELL`, by handing egui the resting edge's colour until then.
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
            // `default_size` before `size_range`, which then clamps a hand-edited value.
            .default_size(self.prefs.layers_width)
            .size_range(crate::panels::layers::MIN_W..=crate::panels::layers::MAX_W)
            .resizable(true)
            .frame(panel_frame(color::PANEL, 0, 0))
            .show(ui, |ui| {
                // The retune is for the edge only, or the content's text would go accent-blue.
                ui.set_style(inner_style);
                ui.set_min_width(ui.available_width());
                // History docks at the foot before anything else is laid out, and the
                // sections above take the room it leaves, drawing nothing below it.
                let rest = match self.show_history {
                    true => self.history_section(ui),
                    false => ui.max_rect(),
                };
                ui.scope_builder(egui::UiBuilder::new().max_rect(rest), |ui| {
                    ui.set_clip_rect(rest.intersect(ui.clip_rect()));
                    // Pages first: the tree below is the current page's.
                    self.pages_section(ui);
                    // Folded, the pane keeps its width and shows no layer names, for
                    // a screen shared with people who should not read them.
                    let folded = self.prefs.layers_folded;
                    egui::Frame::NONE
                        .inner_margin(egui::Margin {
                            left: crate::panels::layers::PAD_X,
                            right: crate::panels::layers::PAD_X,
                            // The divider above sits as far from the Pages list.
                            top: 9,
                            bottom: 9,
                        })
                        .show(ui, |ui| {
                            // A stable id, so opening the search can focus it before it is built.
                            let field = egui::Id::new("layer-search");
                            let searching = self.layer_filter.is_some();
                            ui.horizontal(|ui| {
                                if crate::ui::fold_head(ui, "Layers", !folded).0.clicked() {
                                    self.prefs.layers_folded = !folded;
                                    self.prefs.save();
                                }
                                if folded {
                                    return;
                                }
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
                                        // Fold or unfold the whole tree.
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

                            if !folded && let Some(filter) = &mut self.layer_filter {
                                ui.add_space(4.0);
                                field_row(ui, egui::vec2(ui.available_width(), 28.0), |ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(filter)
                                            .id(field)
                                            .hint_text("Name…")
                                            // The row is the frame.
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
                    // What History's divider may not cover: Layers keeps its header.
                    self.history_pane.head_bottom = ui.cursor().top();

                    if folded {
                        return;
                    }
                    let tree = egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        // History may leave the tree no room at all; egui's default floor
                        // would draw it over History.
                        .min_scrolled_height(0.0)
                        // No bar, though it still scrolls.
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .show(ui, |ui| {
                            // A row dragged to the tree's edge scrolls it, on the canvas autopan's
                            // curve. Inside the area, since `scroll_with_delta` targets the
                            // enclosing one; instant, so it stops with the hand.
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
                                        // Negative y scrolls down: the content moves.
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
                                    // The empty space below the last row is a context-menu
                                    // target; allocated, so it cannot steal a row's right-click.
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
                    crate::ui::scroll_fade_top(
                        ui,
                        tree.inner_rect,
                        tree.state.offset.y,
                        color::PANEL,
                    );
                    #[cfg(test)]
                    {
                        self.history_pane.tree_view = Some(tree.inner_rect);
                    }
                });
            });
        ui.set_style(style);
        self.remember_layers_width(ui.ctx(), shown.response.rect.width(), room);
    }

    /// Gather [`EdgeDwell::advance`]'s inputs from the `Context` and do nothing else.
    /// The band comes from public numbers (`PanelState`'s rect from last frame, egui's
    /// grab radius plus half a point), not the private `__resize` response. Runs only
    /// while the panel is drawn, so a hidden panel freezes `layers_edge`.
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
                // Primary, not `any_down` (a middle-button pan would hold the edge lit).
                i.pointer.button_down(egui::PointerButton::Primary),
            )
        });
        // `NAN` before the panel stored a rect: the band is empty.
        let in_band = pos.is_some_and(|p| (p.x - edge).abs() <= reach);
        self.layers_edge = self.layers_edge.advance(now, in_band, pressed, down);
        if let Some(due) = self.layers_edge.due() {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64((due - now).max(0.0)));
        }
    }

    /// Persist a dragged panel width, once per gesture and only when the *user* changed
    /// it. The wiring; [`width_to_store`] is the decision, kept pure because
    /// `Prefs::save` writes the real `prefs.json`.
    fn remember_layers_width(&mut self, ctx: &egui::Context, width: f32, room: f32) {
        let down = ctx.input(|i| i.pointer.any_down());
        if let Some(w) = width_to_store(width, room, self.prefs.layers_width, down) {
            self.prefs.layers_width = w;
            self.prefs.save();
        }
    }

    /// The column the inspector's cards live in, in screen coordinates; the
    /// popovers hang to its left ([`Self::popover_anchor`]).
    pub(crate) fn inspector_column(screen: egui::Rect) -> egui::Rect {
        /// The card width: a 296px pane less the 12px right gutter.
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
        const MARGIN: f32 = 12.0;
        let screen = ctx.input(|i| i.content_rect());
        (Self::inspector_column(screen).left() - width - POPOVER_GAP).max(screen.min.x + MARGIN)
    }

    /// Where a panel's popover goes: clear of the inspector, to its left. Only `head`'s
    /// top is used, lining the popover up with its row; the `Area`'s `constrain_to` pulls
    /// a tall one back on screen.
    pub(crate) fn popover_anchor(ctx: &egui::Context, head: egui::Rect, width: f32) -> egui::Pos2 {
        egui::pos2(Self::popover_left(ctx, width), head.top())
    }

    /// What a new detached picker opens [`POPOVER_GAP`] left of: the inspector's cards,
    /// or any open popover, which would cover it (`Foreground` over `Middle`).
    pub(crate) fn picker_lane_right(&self, ctx: &egui::Context) -> f32 {
        if self.a_popover_is_open() {
            return Self::popover_left(ctx, POPOVER_W);
        }
        Self::inspector_column(ctx.input(|i| i.content_rect())).left()
    }

    /// Whether anything is occupying the popover lane; a new popover must join this list.
    /// The image-editing card is asked for by its mode being on.
    fn a_popover_is_open(&self) -> bool {
        self.type_menu.is_some()
            || self.stroke_menu.is_some()
            || self.effect_menu.is_some()
            || self.export_menu
            || self.export_row_open.is_some()
            || self.edited_image().is_some()
    }

    /// Whether a popover flag is set that would claim an `Escape` press, so one press
    /// dismisses it and pays out no rung of the ladder. Half of the claim: the router
    /// also asks [`Self::popover_heard`]. [`Self::a_popover_is_open`] minus the
    /// image-edit card on purpose: leaving that mode is the ladder's own rung.
    fn a_popover_owns_escape(&self) -> bool {
        self.type_menu.is_some()
            || self.stroke_menu.is_some()
            || self.effect_menu.is_some()
            || self.export_menu
            || self.export_row_open.is_some()
    }

    /// The inspector: a column of individually floating panels over the right of the
    /// canvas. An `Area` drawn after the canvas, not an `egui::Panel`: the artwork runs
    /// to the window edge, the canvas below the last card stays clickable, and the gaps
    /// between cards are not, so aiming at a card can never deselect.
    pub(crate) fn inspector_panel(&mut self, ctx: &egui::Context) {
        // Last frame's cards are what the palette offers to show.
        self.cards_shown = std::mem::take(&mut self.cards_drawn);
        let screen = ctx.input(|i| i.content_rect());
        let column = Self::inspector_column(screen);

        // A focused field keeps the selection it was showing until it lets go of focus,
        // so a canvas click that changes the selection commits its typed text to its own
        // layer first.
        let held = self.inspector_hold.take().filter(|old| {
            old.ids() != self.session.selection.ids()
                && old
                    .ids()
                    .iter()
                    .all(|id| self.session.doc.get(*id).is_some())
        });
        let shown = held.map(|old| std::mem::replace(&mut self.session.selection, old));

        let area = egui::Id::new("inspector");
        egui::Area::new(area)
            .order(egui::Order::Middle)
            .fixed_pos(column.min)
            .default_size(column.size())
            .show(ctx, |ui| {
                // An `Area` shrink-wraps to last frame's size and a `ScrollArea` fills what
                // it is offered; nested naively they converge on a tiny box.
                ui.scope_builder(egui::UiBuilder::new().max_rect(column), |ui| {
                    egui::ScrollArea::vertical()
                        .max_width(column.width())
                        // Bar hidden, not scrolling disabled.
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .show(ui, |ui| {
                            ui.set_width(column.width());
                            // Tight: each card carries a hairline and a shadow.
                            ui.spacing_mut().item_spacing.y = 5.0;
                            self.inspector_ui(ui);
                        });
                });
            });

        // Hold while a widget on the inspector's own layer has focus (a popover's
        // field only if it asked, `variants::hold_inspector`); release otherwise.
        let focused = ctx.memory(|m| m.focused());
        let layer = egui::LayerId::new(egui::Order::Middle, area);
        let popover = ctx
            .data_mut(|d| d.remove_temp::<bool>(crate::panels::variants::hold_id()))
            .is_some();
        if popover
            || focused
                .and_then(|id| crate::ui::widget_rect(ctx, id))
                .is_some_and(|w| w.layer_id == layer)
        {
            self.inspector_hold = Some(self.session.selection.clone());
        }
        if let Some(now) = shown {
            self.session.selection = now;
        }
    }

    /// Advance [`ChromeHold`] one frame: pick up whatever the last pass's value fields
    /// left behind, then decide whether the chrome comes back. One frame behind, since
    /// the canvas draws before the inspector. The chrome returns on the timeout or on
    /// pointer motion over the canvas, not during a scrub (which wraps the pointer).
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
        // Asked every held frame; egui keeps the earliest request.
        if let ChromeHold::Until(deadline) = self.chrome_hold {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(deadline - now));
        }
    }

    /// Note that an inspector edit is happening, so the selection chrome gets out of the
    /// way. The transaction decides, not the widget: only an edit that changes what is
    /// drawn (`Transaction::changes_ink`) hides the chrome. `resp` is the control's
    /// response; `None` is a discrete edit whose count starts at once.
    pub(crate) fn note_edit(&mut self, resp: Option<&egui::Response>, tx: &Transaction) {
        let Some(dragging) = edit_note(resp, tx) else {
            return;
        };
        // Or-ed: a drag in flight (no deadline yet) wins over a keystroke.
        self.pending_edit = Some(self.pending_edit.unwrap_or(false) || dragging);
    }

    /// [`Self::note_edit`] where the caller already knows the edit is visible and has no
    /// `Transaction`. `live` is whether a gesture still holds the pointer (a hold with
    /// no deadline until the button comes up). Unconditional only because both callers
    /// guard it: `char_valve`'s partial branch and `picker::pointer_slot`'s press branch.
    pub(crate) fn note_live_edit(&mut self, live: bool) {
        self.pending_edit = Some(self.pending_edit.unwrap_or(false) || live);
    }

    /// Commit an inspector edit, arming the chrome hide if it changes anything drawn.
    ///
    /// The valves (`multi_valve`, `char_valve`) commit here too and also note every
    /// frame, or the box would stay up for the whole drag.
    ///
    /// The panels commit through this or [`Self::edit_valve`], never `session.commit`
    /// directly. Exceptions: [`Self::commit_chrome_edit`], the layers panel, and the
    /// *Group*/*Ungroup* buttons (grouping changes no pixel, though `changes_ink` says
    /// `true` by op). Returns whether the commit took.
    pub(crate) fn commit_edit(&mut self, tx: Transaction) -> bool {
        self.note_edit(None, &tx);
        self.session.commit(tx)
    }

    /// [`Self::commit_edit`] for an edit whose **only** visible result is the selection
    /// chrome itself (box trim): it commits without arming the hide.
    pub(crate) fn commit_chrome_edit(&mut self, tx: Transaction) -> bool {
        self.session.commit(tx)
    }

    /// Name the next undo step as `item`'s menu row names it, unless the door that
    /// got here already named it — for a verb that buttons as well as menus reach.
    pub(crate) fn name_step_as(&mut self, item: crate::menu::Item) {
        if !self.session.has_step_name() {
            let spec = item.spec();
            self.session.name_step(spec.label, spec.glyph);
        }
    }

    /// Route an inspector edit through the preview→commit valve: live preview while the
    /// control is held, **one** commit when the edit ends.
    ///
    /// "Ends" is a transition: the valve commits on the frame the control stops being
    /// engaged (dragged or focused), since a typed `DragValue` reports `changed()` per
    /// keystroke and `lost_focus()` twice. `multi_valve` and `char_valve` share this
    /// condition; `valve_condition_gate` enforces it.
    ///
    /// `Escape` cancels through [`Self::cancel_gesture`]. The latch lives in
    /// `Context::data` under the widget's id, read and written in one accessor (nesting
    /// deadlocks). Returns whether this frame committed.
    pub(crate) fn edit_valve(&mut self, resp: &egui::Response, tx: Transaction) -> bool {
        // The latch is updated before anything can bail, so a cancelled frame spends the
        // transition instead of committing on the next one.
        let engaged = resp.dragged() || resp.has_focus();
        let was_engaged = resp.ctx.data_mut(|d| {
            let id = resp.id.with("valve-engaged");
            let was = d.get_temp::<bool>(id).unwrap_or(false);
            d.insert_temp(id, engaged);
            was
        });
        // The scrub was called off; the button is still down, but the edit is over.
        if self.gesture_cancelled {
            return false;
        }
        // Every valved edit arms the chrome hide; a drag's count starts at release.
        self.note_edit(Some(resp), &tx);
        let mut committed = false;
        if engaged {
            self.session.set_preview(&tx);
            self.preview_guide(&tx);
            self.preview_grid(&tx);
            self.preview_var_cell(&tx);
        } else if was_engaged {
            self.guide_previews.clear();
            self.var_cell_preview = None;
            self.in_flight.grid_paint = None;
            // `Escape` abandons the edit (egui does not revert a `DragValue` on it).
            if !resp.ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                committed = self.session.commit(tx);
            }
            // Not `clear_preview`, which would also drop a live text session's preview.
            self.session.clear_gesture_preview();
        }
        committed
    }

    /// The guide half of [`EditorSession::set_preview`]: `RenderOverrides` cannot carry a
    /// guide edit (a guide is chrome), so the valve forks here. Ops naming a guide the
    /// document lacks are skipped, as the commit will refuse them.
    ///
    /// [`EditorSession::set_preview`]: crate::session::EditorSession::set_preview
    pub(crate) fn preview_guide(&mut self, tx: &Transaction) {
        for op in &tx.0 {
            let id = match op {
                Operation::SetGuidePosition { id, .. }
                | Operation::SetGuideColor { id, .. }
                | Operation::SetGuideScope { id, .. } => *id,
                _ => continue,
            };
            // Build on the pending guide so several ops in one transaction compose.
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

    /// The variables-table half of [`EditorSession::set_preview`]: a picker drag's colour
    /// for a variable's cell, held in [`Self::var_cell_preview`]; the canvas keeps the
    /// committed colour until release.
    ///
    /// [`EditorSession::set_preview`]: crate::session::EditorSession::set_preview
    pub(crate) fn preview_var_cell(&mut self, tx: &Transaction) {
        for op in &tx.0 {
            if let Operation::SetVariableCell {
                id,
                mode,
                cell: ondin_core::Cell::Color(c),
            } = op
            {
                self.var_cell_preview = Some((*id, *mode, *c));
            }
        }
    }

    /// The layout-grid half of [`EditorSession::set_preview`] ([`Self::preview_guide`]'s
    /// twin: a grid is chrome). Only a colour change is taken, so a field edit never
    /// writes the picker's buffer. One op per selected frame is gathered; ops disagreeing
    /// with the first about index or colour are dropped.
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

    // Save-pill label tests.

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
        // Rounds rather than truncates: 119s is "2m".
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

    /// A write in flight says "Saving…" even when a stale `ago` is also supplied.
    #[test]
    fn a_queued_write_says_saving_whatever_else_is_true() {
        assert_eq!(save_label(true, None), "Saving…");
        assert_eq!(save_label(true, Some(Duration::from_secs(0))), "Saving…");
    }

    /// Every band opens where it can say `1` of its unit.
    /// flip: hours band opens at 5400 → fails at the 2700 row
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

    /// `layers.rs` is the only panel that commits for itself. The `self.session.` receiver
    /// separates production code from fixtures; this file is outside `panels/`, so
    /// widening the scan would self-match.
    #[test]
    fn only_the_layers_panel_commits_without_the_committer() {
        let panels: [(&str, &str); 15] = [
            ("binding.rs", include_str!("panels/binding.rs")),
            ("component.rs", include_str!("panels/component.rs")),
            ("dashboard.rs", include_str!("panels/dashboard.rs")),
            ("export.rs", include_str!("panels/export.rs")),
            ("history.rs", include_str!("panels/history.rs")),
            ("inspector.rs", include_str!("panels/inspector.rs")),
            ("layers.rs", include_str!("panels/layers.rs")),
            ("layout.rs", include_str!("panels/layout.rs")),
            ("layout/grid.rs", include_str!("panels/layout/grid.rs")),
            ("paint.rs", include_str!("panels/paint.rs")),
            ("picker.rs", include_str!("panels/picker.rs")),
            ("styles.rs", include_str!("panels/styles.rs")),
            ("typography.rs", include_str!("panels/typography.rs")),
            ("variables.rs", include_str!("panels/variables.rs")),
            ("variants.rs", include_str!("panels/variants.rs")),
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

        // The array is hand-written (`include_str!` takes no glob): check it against the
        // directory; `mod.rs` is the one name subtracted.
        fn listed_under(dir: &std::path::Path, prefix: &str, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir)
                .expect("a panels directory")
                .flatten()
            {
                let name = e.file_name().to_string_lossy().into_owned();
                let path = format!("{prefix}{name}");
                if e.path().is_dir() {
                    listed_under(&e.path(), &format!("{path}/"), out);
                } else if path.ends_with(".rs") && path != "mod.rs" {
                    out.push(path);
                }
            }
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/panels");
        let mut on_disk: Vec<String> = Vec::new();
        listed_under(&dir, "", &mut on_disk);
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

    /// A layer comfortably in frame does not scroll.
    #[test]
    fn something_already_in_view_does_not_scroll() {
        assert_eq!(reveal_axis(10.0, 20.0, 0.0, 100.0), 0.0);
        // Flush against both edges still counts as inside.
        assert_eq!(reveal_axis(0.0, 100.0, 0.0, 100.0), 0.0);
    }

    /// A layer that has left scrolls by the *minimum* that brings it back, not a centring.
    #[test]
    fn a_layer_off_the_edge_scrolls_by_the_least_that_shows_it() {
        assert_eq!(reveal_axis(105.0, 115.0, 0.0, 100.0), 15.0);
        assert_eq!(reveal_axis(-15.0, -5.0, 0.0, 100.0), -15.0);
    }

    /// A layer overhanging both edges does not scroll (it would oscillate).
    #[test]
    fn a_selection_that_straddles_the_whole_view_does_not_scroll() {
        assert_eq!(reveal_axis(-20.0, 150.0, 0.0, 100.0), 0.0);
        assert_eq!(reveal_axis(-500.0, 500.0, 0.0, 100.0), 0.0);
    }

    /// A layer larger than the view but entirely outside it scrolls to its near edge only.
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

    /// `advance` with nothing happening.
    fn idle(state: ChromeHold, now: f64) -> ChromeHold {
        state.advance(now, None, false, false)
    }

    /// A scrub's count starts at the release, not at the press.
    #[test]
    fn a_scrub_holds_for_its_whole_length_however_long_that_is() {
        let mut hold = ChromeHold::Free;
        hold = hold.advance(0.0, Some(true), true, false);
        assert_eq!(hold, ChromeHold::Scrubbing);
        // Past the timeout, reporting on some frames only (a slow drag spends whole
        // frames below one unit).
        for t in [1.0, 2.5, 5.0, 9.9] {
            hold = hold.advance(t, if t > 2.0 { None } else { Some(true) }, true, false);
            assert_eq!(hold, ChromeHold::Scrubbing, "let go at {t}s");
            assert!(hold.holding());
        }
        hold = idle(hold, 10.0);
        assert_eq!(hold, ChromeHold::Until(10.0 + CHROME_HOLD_SECS));
        assert!(hold.holding());
        assert_eq!(idle(hold, 10.0 + CHROME_HOLD_SECS - 0.01), hold);
        assert_eq!(idle(hold, 10.0 + CHROME_HOLD_SECS), ChromeHold::Free);
    }

    /// Every keystroke restarts the count.
    #[test]
    fn each_typed_keystroke_restarts_the_count() {
        let mut hold = ChromeHold::Free.advance(0.0, Some(false), false, false);
        assert_eq!(hold, ChromeHold::Until(CHROME_HOLD_SECS));
        hold = hold.advance(1.0, Some(false), false, false);
        assert_eq!(hold, ChromeHold::Until(1.0 + CHROME_HOLD_SECS));
        assert!(idle(hold, CHROME_HOLD_SECS + 0.01).holding());
        assert_eq!(idle(hold, 1.0 + CHROME_HOLD_SECS), ChromeHold::Free);
    }

    /// The pointer moving over the canvas ends the hold before the timeout.
    #[test]
    fn canvas_motion_brings_the_chrome_back_before_the_timeout() {
        let hold = ChromeHold::Free.advance(0.0, Some(false), false, false);
        assert!(hold.holding());
        assert_eq!(
            hold.advance(0.1, None, false, true),
            ChromeHold::Free,
            "a tenth of a second in, the pointer reaching the canvas is enough"
        );
        // Motion off the canvas is not enough: travelling between fields keeps it hidden.
        assert_eq!(hold.advance(0.1, None, false, false), hold);
    }

    /// Canvas motion does not cancel a scrub in flight (a long drag wraps the pointer,
    /// `ui::wrap_scrub`).
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
        hold = idle(hold, 0.3);
        assert_eq!(hold.advance(0.4, None, false, true), ChromeHold::Free);
    }

    /// Nothing happening leaves the chrome alone.
    #[test]
    fn an_idle_frame_changes_nothing() {
        assert_eq!(idle(ChromeHold::Free, 0.0), ChromeHold::Free);
        assert_eq!(idle(ChromeHold::Free, 99.0), ChromeHold::Free);
        assert!(!ChromeHold::Free.holding());
        // A button down elsewhere (a canvas drag) is not a scrub.
        assert_eq!(
            ChromeHold::Free.advance(1.0, None, true, false),
            ChromeHold::Free
        );
    }
}

/// `edit_note` against real value fields, in a headless pass.
#[cfg(test)]
mod edit_note_tests {
    use super::{Transaction, edit_note};
    use crate::theme;
    use crate::ui::{self, Prefix, Scrub};
    use ondin_core::Operation;
    use ondin_core::kurbo::Affine;

    /// The transaction a Transform field offers whether or not the number moved.
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

    /// One pass over a `value_field`: what `edit_note` makes of it, and the field's
    /// painted ground to aim at.
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
        // The response is a union that shrinks if the prefix strip stops sensing.
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

    /// A field nobody is touching is not an edit (`edit_valve` builds a transaction every
    /// frame, so the hold would renew for ever).
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

    /// Hovering is not editing either.
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

    /// A drag reports itself as a drag, so its hold has no deadline until release.
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

    /// Typing a digit notes a typed edit, so the count starts at once.
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

    /// An edit that changes nothing drawn notes nothing; a discrete commit (`None`)
    /// always notes.
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

    /// A copy of a guide keeps its axis, colour and owning frame, and steps along its axis.
    #[test]
    fn a_pasted_guide_keeps_its_axis_colour_and_scope() {
        let mut ids = IdSource::new(0x9A57E);
        let frame = ids.mint();
        let page = ids.mint();
        let red = ondin_core::peniko::Color::from_rgba8(200, 30, 60, 255);

        let scoped = Guide {
            id: GuideId(ids.mint()),
            axis: GuideAxis::Vertical,
            position: 120.0,
            color: Some(red),
            owner: frame,
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
        assert_eq!(copy.owner, frame, "and stays in its frame");
        assert_eq!(copy.position, 120.0 + GUIDE_PASTE_OFFSET);

        // A global guide copies global.
        let global = Guide {
            id: GuideId(ids.mint()),
            axis: GuideAxis::Horizontal,
            position: -40.5,
            color: None,
            owner: page,
        };
        let copy = pasted_guide(&global, GuideId(ids.mint()), page, GUIDE_PASTE_OFFSET);
        assert_eq!(copy.owner, page);
        assert_eq!(
            copy.color, None,
            "still following the default, not frozen to it"
        );
        assert_eq!(copy.position, -40.5 + GUIDE_PASTE_OFFSET);
    }

    /// A guide whose frame was deleted falls back to the canvas; `pasted_guide` honours
    /// the owner it is handed.
    #[test]
    fn a_pasted_guide_whose_frame_is_gone_lands_on_the_canvas() {
        let mut ids = IdSource::new(0x9A57E);
        let gone = ids.mint();
        let page = ids.mint();
        let orphan = Guide {
            id: GuideId(ids.mint()),
            axis: GuideAxis::Vertical,
            position: 60.0,
            color: None,
            owner: gone,
        };
        // What `paste_guides` computes when `doc.contains(gone)` is false.
        let copy = pasted_guide(&orphan, GuideId(ids.mint()), page, GUIDE_PASTE_OFFSET);
        assert_eq!(copy.owner, page);
        // The number is carried across unchanged (nothing to reproject against).
        assert_eq!(copy.position, 60.0 + GUIDE_PASTE_OFFSET);
    }

    /// The guide step is `PASTE_OFFSET`'s own component.
    #[test]
    fn a_guide_and_a_layer_step_by_the_same_amount() {
        assert_eq!(GUIDE_PASTE_OFFSET, PASTE_OFFSET.x);
    }

    /// *Paste here* aims from the recorded source box, so it works after a cut; with no
    /// box it falls back to the fixed step.
    #[test]
    fn paste_here_aims_from_the_recorded_box_and_not_from_the_layers() {
        // Off the origin on both axes, so a centre confused with a corner cannot pass.
        let from = Rect::new(100.0, 40.0, 140.0, 60.0);
        assert_eq!(from.center(), Point::new(120.0, 50.0));

        let pointer = Point::new(300.0, 200.0);
        let aim = paste_aim(Some(from), pointer);
        assert_eq!(aim, Vec2::new(180.0, 150.0));
        assert_eq!(from.center() + aim, pointer);

        // A pointer inside the source box aims backwards.
        assert_eq!(
            paste_aim(Some(from), Point::new(110.0, 45.0)),
            Vec2::new(-10.0, -5.0)
        );

        // No box: the chord's own step.
        assert_eq!(paste_aim(None, pointer), PASTE_OFFSET);
    }

    /// A guide pasted in place keeps its own number. Which caller passes zero is not
    /// tested: `owns_the_clipboard` is permanently `false` under `CLIPBOARD_OFF`.
    #[test]
    fn a_guide_pasted_in_place_keeps_its_own_number() {
        let mut ids = IdSource::new(0x9A57E);
        let page = ids.mint();
        let guide = Guide {
            id: GuideId(ids.mint()),
            axis: GuideAxis::Horizontal,
            position: -40.5,
            color: None,
            owner: page,
        };
        let copy = pasted_guide(&guide, GuideId(ids.mint()), page, 0.0);
        assert_eq!(
            copy.position, -40.5,
            "a step of zero is the whole of paste in place"
        );
        assert_ne!(copy.id, guide.id, "and it is still its own guide");
    }

    /// A locked paste still reports the guides it made, though they are not selected.
    /// flip: count `selection.guides()` → fails at the locked "Pasted 3 guide(s)" assertion
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
                        owner: app.session.page,
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

/// The two-digit opacity window (`docs/shortcuts.md` §2); `opacity_step` is pure.
#[cfg(test)]
mod opacity_digit_tests {
    use super::{
        OPACITY_DIGIT_WINDOW, OpacityEntry, OpacityStep, opacity_of_digit, opacity_step,
        opacity_window_open,
    };

    /// A lone digit is tens; `0` is 100%.
    #[test]
    fn a_lone_digit_is_tens_except_zero_which_is_full() {
        assert_eq!(opacity_of_digit(1), 10.0);
        assert_eq!(opacity_of_digit(4), 40.0);
        assert_eq!(opacity_of_digit(9), 90.0);
        assert_eq!(opacity_of_digit(0), 100.0);
    }

    /// The first digit waits (a preview) and the second lands the exact value.
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

    /// A digit typed against a different layer starts over. flip: drop
    /// `entry.subject == subject` from the pairing arm → fails at the first assertion
    #[test]
    fn a_digit_typed_against_a_different_layer_does_not_pair() {
        let mut ids = ondin_core::IdSource::new(1);
        let (a, b) = ([ids.mint()], [ids.mint()]);
        let OpacityStep::Wait(_, entry) = opacity_step(None, 4, 10.0, &a) else {
            panic!("the first digit waits");
        };
        // Well inside the window; only the subject moved.
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
        // `0`,`5` is 5%, not 50%.
        let OpacityStep::Wait(_, entry) = opacity_step(None, 0, 0.0, &one) else {
            unreachable!()
        };
        assert_eq!(
            opacity_step(Some(entry), 5, 0.1, &one),
            OpacityStep::Land(5.0)
        );
    }

    /// Once the window has run out, the next digit starts a new value. Sampled a hair
    /// either side of the boundary (a float artefact, `10.6 - 10.0`).
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

    /// The pairing rule and the commit deadline read the boundary the same way
    /// (`opacity_window_open`), swept finely across it.
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
    //! A cancelled opacity digit does not commit itself when the window runs out: drive
    //! `escape`, then advance time past the deadline.
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
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("build");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![rect]);
        (app, rect)
    }

    /// Delivers the clock by a frame (`now` is read from `ctx.input(|i| i.time)`).
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

    /// `Escape` shuts the digit window without committing it.
    /// flip: remove `self.opacity_entry = None;` from `cancel_gesture` → fails at the
    /// post-deadline opacity assertion
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

    /// Control: an uncancelled digit still lands on the deadline.
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

/// Which way `Ctrl+Shift+H` and `Ctrl+Shift+L` take a whole selection.
#[cfg(test)]
mod set_switch_tests {
    use super::OndinApp;

    /// A mixed selection applies the verb; only an all-applied one releases.
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
        // The smallest mixed selection.
        assert!(OndinApp::set_switch([true, false]));
        assert!(OndinApp::set_switch([false, true]));
    }

    /// The same three states through `hide_switch` and `lock_switch` (the polarity
    /// inversion is under test, not re-implemented).
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
    //! A lock refuses every editing mode, and a locked group refuses for what is inside
    //! it (the refusals ask `query::is_effectively_locked`, not a node's own flag).
    //! `Enter` is the door driven here: `query::hit_test` already refuses a double-click
    //! on a locked node and the menu rows dim.
    use super::OndinApp;
    use crate::tools::Tool;
    use ondin_core::kurbo::Size;
    use ondin_core::{
        Document, Fill, IdSource, NodeId, NodeKind, Operation, Transaction, image_brush,
    };

    /// A headless app holding one 80×60 picture rect, selected, `Select` armed, locked if asked.
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
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("build");
        doc.apply(&Transaction(vec![Operation::SetFills {
            id: rect,
            fills: ondin_core::keyed_by_position([Fill {
                brush: image_brush(ondin_core::ImageId("a-photo".into())).into(),
                visible: true,
            }]),
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

    /// `begin_image_edit` refuses a locked picture, directly and through `Enter`.
    /// flip: delete the guard in `begin_image_edit` → fails at the locked-picture assertions
    #[test]
    fn a_locked_picture_refuses_every_door() {
        let ctx = egui::Context::default();

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

        let (mut shut, id) = app_with_a_picture(&ctx, true);
        shut.begin_image_edit(id);
        assert_eq!(
            shut.tool,
            Tool::Select,
            "a locked picture does not enter the mode — a crop is a resize, and the \
             lock reaches a resize"
        );

        // `Enter`'s arm asks only `croppable_fill`.
        let (mut chord, _) = app_with_a_picture(&ctx, true);
        chord.enter_action();
        assert_eq!(
            chord.tool,
            Tool::Select,
            "`Enter` on a locked picture selected in the layers panel is the door \
             this closed, and it closes in the mode rather than in the arm"
        );
    }

    /// A headless app holding a group with one child of `kind`, the child selected; the
    /// group is locked if asked, never the child, so refusals are the ancestor walk.
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

    /// Text and points refuse on a locked layer too (text asserts on the session).
    /// flip: delete the guard in `begin_edit_text` or in `enter_action`'s `Path` arm →
    /// fails at the matching shut_* assertion
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

        // Controls: unlocked, both modes open.
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

        // Locked on the group: the ancestor rule as well.
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

    /// `A` and the rail button arm `Tool::Node` without `Enter`'s guard, so the lock is
    /// asserted on `edited_path` (the one query every reader uses).
    /// flip: remove the guard from `edited_path` → fails at the locked `None` assertion
    #[test]
    fn arming_the_node_tool_finds_no_subject_on_a_locked_path() {
        let ctx = egui::Context::default();

        let (mut live, id) = app_with_a_child_in_a_group(&ctx, path_kind(), false);
        live.choose_tool(Tool::Node);
        assert_eq!(
            live.edited_path(),
            Some(id),
            "the control: an unlocked path is the subject once the tool is armed"
        );

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

    /// A picture inside a locked group refuses image editing too.
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
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
        ]))
        .expect("build");
        doc.apply(&Transaction(vec![Operation::SetFills {
            id: child,
            fills: ondin_core::keyed_by_position([Fill {
                brush: image_brush(ondin_core::ImageId("a-photo".into())).into(),
                visible: true,
            }]),
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

        // A guard reading `n.locked()` would pass the child.
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
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((30.0, 20.0))),
            name: None,
        }]))
        .expect("build");
        doc.apply(&Transaction(vec![Operation::SetFills {
            id: rect,
            fills: ondin_core::keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(200, 40, 60, 255)).into(),
                visible: true,
            }]),
        }]))
        .expect("paint");
        let res = Resolved::rebuild(&doc);
        (doc, res, rect)
    }

    /// *Copy as PNG* hands the clipboard raw RGBA at one device pixel per world unit.
    /// flip: encode with `png_of` → fails at the byte count; `scale: 2.0` → fails at
    /// the size
    #[test]
    fn the_clipboard_picture_is_raw_pixels_at_one_to_one() {
        let (doc, res, rect) = one_rect();
        let bounds = ondin_export::extent(&res, &[rect]).expect("the rect has bounds");
        assert_eq!(
            (bounds.width(), bounds.height()),
            (100.0, 60.0),
            "the fixture is not the rect this test thinks it is"
        );

        let raster = OndinApp::png_for_the_clipboard(&doc, &res, &[rect], 1)
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

    /// *Copy as PNG 2×* doubles both sides.
    ///
    /// flip: `png_for_the_clipboard` ignoring `scale` → fails at the size.
    #[test]
    fn the_2x_row_copies_twice_the_pixels() {
        let (doc, res, rect) = one_rect();
        let raster = OndinApp::png_for_the_clipboard(&doc, &res, &[rect], 2)
            .expect("a rect with bounds rasterizes");
        assert_eq!((raster.width, raster.height), (200, 120));
    }

    /// An empty selection has no extent, and the row says so instead of putting a
    /// 1×1 or a panic on the clipboard.
    #[test]
    fn nothing_selected_is_no_picture_rather_than_an_empty_one() {
        let (doc, res, _) = one_rect();
        assert!(OndinApp::png_for_the_clipboard(&doc, &res, &[], 1).is_none());
    }
}

/// `Ctrl`+`←`/`→` widen and narrow the selection; the keymap test only proves the
/// chord resolves to `Action::SizeStep`, so the shape behaviour is pinned here.
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
                corner_smoothing: 0.0,
            },
            transform: transform.or(Some(Affine::translate((10.0, 20.0)))),
            name: None,
        }]))
        .expect("build");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(rect);
        (app, rect)
    }

    /// Two 20×20 rects, at (0, 0) and (80, 40), both selected: a 100×60 union box.
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
                corner_smoothing: 0.0,
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

    /// Bigger and smaller on both axes, top-left held still. flip: `local.width()` in
    /// place of `want(local.width(), delta.x)` → fails at the first width assertion
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

    /// A width cannot be pressed away: the floor is 1, as in the W field. flip: drop
    /// the `.max(1.0)` → fails at the `min_x` assertion (the width lands on 1 by
    /// coincidence after reflecting across the edge)
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
        app.size_step(wide(10.0));
        assert_eq!(world(&app, rect).width(), 11.0, "and it comes back");

        for _ in 0..10 {
            app.size_step(tall(-10.0));
        }
        let flat = world(&app, rect);
        assert_eq!(flat.height(), 1.0, "the same floor on the other axis");
        assert_eq!(flat.min_y(), 20.0, "anchored at the top edge it started on");
    }

    /// A rotated shape grows along its own axes: `tools::resize_box_to` works on the
    /// local box, so a world-union resize would diverge on the world box.
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

    /// A vertical `Line` has a zero-wide local box, which the floor cannot help: it
    /// must stay finite and be left exactly alone.
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

    /// A locked layer scales both axes whichever arrow drives, keeping the 0.75
    /// aspect and the top-left corner. flip: drop the `locked` test → fails at the `w_driven.height()` assertion
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

        assert!(
            (h_driven.height() / h_driven.width() - 0.75).abs() < 1e-9,
            "the aspect survived the pair of presses: {h_driven:?}"
        );
    }

    /// Unlocked, one arrow moves one axis — the control for the test above.
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

    /// A set is locked when every member is, and then the keyboard pairs the union
    /// (100 × 60, aspect 0.6); a mixed set does not. flip: `any` for `all` in
    /// `tools::proportions_locked` → fails at the mixed-set height assertion
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

    /// Several selected resize as one box (`resize_selection`) anchored at the union's
    /// top-left, keeping their arrangement. flip: resize from `Handle::TopLeft` →
    /// fails at the `near.min_x()` assertion
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

    /// Nothing selected: no change, no panic, no undo entry.
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
    //! When a dragged layers-panel width is worth storing, and the egui behaviour the
    //! panel is built on. The wiring around `width_to_store` is not tested: `Prefs::save`
    //! writes into `dirs::config_dir()`.

    use super::*;

    /// A dragged width is stored only with the pointer up, when the panel is not just
    /// filling its room, and when it differs from the stored one; one case per term.
    #[test]
    fn a_dragged_width_is_stored_once_the_drag_ends_and_the_window_is_not_the_cause() {
        // (width, room, stored, pointer down) -> what should be stored
        for (what, args, want) in [
            ("mid-drag", (360.0, 1200.0, 300.0, true), None),
            ("released", (360.0, 1200.0, 300.0, false), Some(360.0)),
            ("unchanged", (300.0, 1200.0, 300.0, false), None),
            ("a hair of rounding", (300.4, 1200.0, 300.0, false), None),
            // Full width: egui's clamp set it, not a hand.
            ("window shrank", (400.0, 400.0, 300.0, false), None),
            // Same width with room to spare: a drag.
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

    /// A resizable egui `Panel` whose content does not fill it collapses to
    /// `size_range`'s minimum, which is why `layers_panel` calls `ui.set_min_width`.
    #[test]
    fn a_resizable_panel_collapses_to_its_minimum_unless_the_content_fills_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let width = |fills: bool| {
            let mut w = 0.0;
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

    /// A stored width outside the bounds (`prefs.json` is untrusted) is clamped only
    /// because `default_size` is called before `size_range`. flip: swap the two calls
    /// → fails at the `clamp-hi` assertion
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

    /// Two egui facts decide who owns the cursor: it does not survive a frame, and
    /// within a frame the last writer wins (a canvas reset erases the panel's claim).
    #[test]
    fn a_cursor_does_not_survive_a_frame_and_the_last_writer_wins() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);

        let icon_of = |out: egui::FullOutput| out.platform_output.cursor_icon;

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

    /// `layers_panel` retunes two fg strokes on the shared central `ui` and must
    /// restore them. flip: delete `ui.set_style(style)` → fails at the
    /// `hovered.fg_stroke` assertion
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
        assert_eq!(after.hovered.bg_stroke, before.hovered.bg_stroke);
        assert_eq!(
            after.noninteractive.bg_stroke,
            before.noninteractive.bg_stroke,
        );
    }
}

#[cfg(test)]
mod edge_dwell_tests {
    //! The layers panel's drag edge waits before it lights, driven through the
    //! machine's own states.

    use super::*;

    /// A pointer that crosses the band inside the dwell never lights it; one that
    /// stays lights at the deadline and not before. flip: return `Lit` from the
    /// `Off if in_band` arm → fails at the first crossing assertion
    #[test]
    fn the_edge_waits_out_the_dwell_before_it_lights() {
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

    /// A press lights the edge at once; a drag begun elsewhere never does, however
    /// long it stays. flip: drop `!down` from the dwell arm → fails at the fourth
    /// sample; `down` for `pressed` in the shortcut arm → fails at the first
    #[test]
    fn a_press_skips_the_dwell_but_a_drag_that_only_arrives_never_lights_it() {
        let pressed_on_it = EdgeDwell::default().advance(0.0, true, true, true);
        assert!(
            pressed_on_it.lit(),
            "a press on the edge waited out the dwell, so the first 200ms of a \
             resize drag has no feedback at all"
        );

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

    /// A lit edge stays lit once the pointer leaves the band while a button is down
    /// (egui stops the edge at the clamp and the hand keeps going). flip: drop
    /// `|| down` from the `Lit` arm → fails at the `past.lit()` assertion
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
        assert_eq!(
            past.advance(0.3, false, false, false),
            EdgeDwell::Off,
            "released off the band and the edge stayed lit — it would now be lit \
             with the pointer somewhere else entirely"
        );
    }

    /// The fold reads the primary button, not any: a middle-button pan resting on the
    /// band past the dwell must still light the edge, through the real `layers_panel`.
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

    /// The frame is woken for the deadline, since a resting pointer produces no
    /// events; settled states ask for no repaint. flip: `due` returns `None`
    /// unconditionally → fails at the first assertion
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

/// Greps the workspace for the superseded commit expression (`lost_focus()` with
/// `changed()`), which compiles and passes every other gate.
#[cfg(test)]
mod valve_condition_gate {
    use std::path::{Path, PathBuf};

    /// The marker a line carries to say it is a gate ("is anything happening to this
    /// control?") rather than a commit condition. A marker beside the code travels
    /// with it, where a list of exempt locations rots.
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

    /// The file with `//` comments blanked (not removed, so offsets still name real
    /// lines). A `//` inside a string literal is blanked too; the failure mode is a
    /// missed hit, not a false one.
    fn without_comments(src: &str) -> String {
        src.lines()
            .map(|l| match l.find("//") {
                Some(i) => format!("{}{}", &l[..i], " ".repeat(l.len() - i)),
                None => l.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// No control may decide it is finished from `lost_focus()` and `changed()`
    /// together: a typed `DragValue` reports both on several frames, so it would
    /// commit per keystroke. The engagement latch (`dragged() || has_focus()`)
    /// replaces it; a gate that only decides whether to build a transaction carries
    /// the marker. Windowed rather than line-matched, because `cargo fmt` wraps.
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
                // The marker is looked for in the original text, since `code` has
                // blanked it.
                let line = code[..at].matches('\n').count() + 1;
                // Sixteen lines back and four forward: a wrapped condition puts the
                // term well below the comment that excuses it.
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
        // valve-gate-not-a-commit: the message below quotes the two names.
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

/// What `OndinApp::edit_valve` reports, which a caller depends on.
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
                corner_smoothing: 0.0,
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

    /// The valve reports `true` on exactly the frame it commits and never on a
    /// keystroke frame, so a caller can act after a commit without predicting one.
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

    // No `undo_depth` assertion: the probe's committing transaction is a no-op, and
    // the valve reports the frame, not a document change.

    /// An abandoned edit reports `false` on every frame, though `lost_focus()` is
    /// just as true when `Escape` was the way out.
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

/// A drop of several image files is one gesture.
#[cfg(test)]
mod image_drop_tests {
    use super::{LoadedImage, OndinApp, content_hash};
    use ondin_core::kurbo::{Point, Size};
    use ondin_core::{Document, IdSource, ImageEntry, ImageFormat, ImageSource, NodeId};

    /// One picture, distinct from every other `n`; the bytes need not be a real PNG.
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

    /// Every layer on the root; a picture is a `Rect` with an image fill, so in an
    /// empty document they are all the drop's own.
    fn pictures(app: &OndinApp) -> Vec<NodeId> {
        let doc = &app.session.doc;
        doc.get(doc.first_page())
            .map(|n| n.children().to_vec())
            .unwrap_or_default()
    }

    /// One drop is one undo step however many files it carried. `commit_run` cannot
    /// merge these (`CreateNode` never `overwrites()`), so accumulation happens before
    /// the commit. flip: loop over `create_image_at` → fails at the undo-step count
    #[test]
    fn one_drop_of_three_files_is_one_undo_step() {
        let ctx = egui::Context::default();
        let mut app = app_with_an_empty_document(&ctx);
        let depth0 = app.session.history.undo_depth();

        app.drop_images_at(
            vec![loaded(1, None), loaded(2, None), loaded(3, None)],
            Some(Point::ZERO),
            false,
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

    /// Three files land under three distinct names, numbered against the advancing
    /// document. flip: `image_tx(None, …)` inside the loop → fails at the unique-name
    /// count
    #[test]
    fn a_batch_numbers_its_layers_against_the_siblings_it_is_adding() {
        let ctx = egui::Context::default();
        let mut app = app_with_an_empty_document(&ctx);

        app.drop_images_at(
            vec![loaded(1, None), loaded(2, None), loaded(3, None)],
            Some(Point::ZERO),
            false,
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

    /// The same file dropped twice embeds its bytes once. flip: the same
    /// `image_tx(None, …)` → fails at the layer count (the second transaction is
    /// refused)
    #[test]
    fn one_blob_dropped_twice_is_embedded_once() {
        let ctx = egui::Context::default();
        let mut app = app_with_an_empty_document(&ctx);

        app.drop_images_at(
            vec![loaded(7, Some("a")), loaded(7, Some("b"))],
            Some(Point::ZERO),
            false,
        );

        assert_eq!(pictures(&app).len(), 2, "two layers");
        assert_eq!(
            app.session.doc.image_count(),
            1,
            "sharing one entry between them"
        );
    }

    /// A picture dropped onto a bound or styled fill says what it detached, rather
    /// than the door's own *Replaced with …* overwriting it. flip: `cut_var` forced to
    /// `None` → fails at the first status; drop the `last_detached` read → fails at
    /// the second
    #[test]
    fn a_drop_onto_a_bound_or_styled_fill_says_what_it_detached() {
        use ondin_core::style::{Style, StyleValue};
        use ondin_core::{
            BrushSource, Cell, Fill, Mode, NodeKind, Operation, Transaction, VarKind, Variable,
            keyed_by_position, peniko::Color,
        };
        let ctx = egui::Context::default();
        let mut app = app_with_an_empty_document(&ctx);
        let root = app.session.doc.first_page();
        let (a, b) = (app.session.ids.mint(), app.session.ids.mint());
        let (mode, var) = (app.session.ids.mint_mode(), app.session.ids.mint_var());
        let style = app.session.ids.mint_style();
        let rect = |id, x: f64| Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(100.0, 100.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(ondin_core::kurbo::Affine::translate((x, 0.0))),
            name: None,
        };
        let red = keyed_by_position([Fill {
            brush: Color::from_rgba8(255, 0, 0, 255).into(),
            visible: true,
        }]);
        assert!(app.commit_edit(Transaction(vec![
            rect(a, 0.0),
            rect(b, 200.0),
            Operation::AddMode {
                mode: Mode {
                    id: mode,
                    name: "Mode 1".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: var,
                    name: "brand/primary".into(),
                    kind: VarKind::Color,
                    cells: [(mode, Cell::Color(Color::from_rgba8(0, 0, 255, 255)))].into(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::SetFills {
                id: a,
                fills: keyed_by_position([Fill {
                    brush: BrushSource::Bound { var, alpha: 1.0 },
                    visible: true,
                }]),
            },
            Operation::SetFills {
                id: b,
                fills: red.clone(),
            },
            Operation::AddStyle {
                style: Style {
                    id: style,
                    name: "Surface".into(),
                    value: StyleValue::Fills(red),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::SetFillStyle {
                id: b,
                style: Some(style),
            },
        ])));
        assert_eq!(app.session.doc.get(b).unwrap().styles().fills, Some(style));

        app.session.selection.set_one(a);
        app.drop_images_at(
            vec![loaded(1, Some("photo"))],
            Some(Point::new(50.0, 50.0)),
            true,
        );
        let fill = &app.session.doc.get(a).unwrap().paint().fills[0];
        assert_eq!(
            fill.brush.var(),
            None,
            "the picture replaced the bound fill"
        );
        assert_eq!(
            app.session.status().text,
            "Replaced with photo · Detached brand/primary"
        );

        app.session.selection.set_one(b);
        app.drop_images_at(
            vec![loaded(2, Some("photo"))],
            Some(Point::new(250.0, 50.0)),
            true,
        );
        assert_eq!(app.session.doc.get(b).unwrap().styles().fills, None);
        assert_eq!(
            app.session.status().text,
            "Replaced with photo · Detached Surface"
        );
    }
}

/// *Paste properties* says what its bindings landed as.
#[cfg(test)]
mod paste_properties_report_tests {
    use super::OndinApp;
    use ondin_core::kurbo::Size;
    use ondin_core::peniko::Color;
    use ondin_core::style::{Style, StyleValue};
    use ondin_core::{
        BrushSource, Cell, Document, Fill, IdSource, Mode, NodeId, NodeKind, Operation, Stroke,
        Transaction, VarId, VarKind, Variable, keyed_by_position,
    };

    fn rect(id: NodeId, parent: NodeId) -> Operation {
        Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(50.0, 50.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        }
    }

    fn colour(ids: &mut IdSource, mode: ondin_core::ModeId, name: &str) -> (VarId, Operation) {
        let id = ids.mint_var();
        let op = Operation::AddVariable {
            var: Variable {
                id,
                name: name.into(),
                kind: VarKind::Color,
                cells: [(mode, Cell::Color(Color::from_rgba8(0, 0, 255, 255)))].into(),
                origin: None,
                forked: None,
                description: String::new(),
            },
        };
        (id, op)
    }

    fn bound(var: VarId) -> BrushSource {
        BrushSource::Bound { var, alpha: 1.0 }
    }

    /// A paste over a bound stroke width or opacity names the binding it takes away.
    /// flip: remove `paints_overwritten`'s `SetOpacityVar` arm, or read its strokes
    /// without `width_var` → fails at "names w and x"
    #[test]
    fn a_paste_names_a_bound_width_and_opacity_it_takes_away() {
        use ondin_core::variable::{Cell, Mode, VarKind, Variable};
        let mut ids = ondin_core::IdSource::new(0x7A);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let (rect, mode) = (ids.mint(), ids.mint_mode());
        let [w, x, y] = [(); 3].map(|_| ids.mint_var());
        let number = |id, name: &str| Operation::AddVariable {
            var: Variable {
                id,
                name: name.into(),
                kind: VarKind::Number,
                cells: [(mode, Cell::Number(2.0))].into(),
                origin: None,
                forked: None,
                description: String::new(),
            },
        };
        let stroke = |width_var| {
            ondin_core::keyed_by_position([ondin_core::Stroke {
                width: 2.0,
                width_var,
                ..Default::default()
            }])
        };
        doc.apply(&Transaction(vec![
            Operation::AddMode {
                mode: Mode {
                    id: mode,
                    name: "Default".into(),
                },
                index: 0,
                cells: vec![],
            },
            number(w, "w"),
            number(x, "x"),
            number(y, "y"),
            Operation::CreateNode {
                id: rect,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::SetStrokes {
                id: rect,
                strokes: stroke(Some(w)),
            },
            Operation::SetOpacityVar {
                id: rect,
                var: Some(x),
            },
        ]))
        .unwrap();
        let tx = Transaction(vec![
            Operation::SetStrokes {
                id: rect,
                strokes: stroke(None),
            },
            Operation::SetOpacityVar {
                id: rect,
                var: Some(y),
            },
        ]);
        assert_eq!(
            crate::app::paints_overwritten(&doc, &tx),
            vec!["w".to_string(), "x".to_string()],
            "names w and x"
        );
    }

    /// A paste into another document says what it rebound (found again by name) and
    /// what it detached. flip: `properties_landing` or `paints_overwritten`
    /// answering empty → fails at the status assertion
    #[test]
    fn a_paste_into_another_document_says_what_it_rebound_and_detached() {
        let mut ids = IdSource::new(0xA1A);
        let mut there = Document::new(ids.mint());
        let mode = ids.mint_mode();
        let (brand, add_brand) = colour(&mut ids, mode, "brand/primary");
        let (only, add_only) = colour(&mut ids, mode, "only/there");
        let src = ids.mint();
        there
            .apply(&Transaction(vec![
                Operation::AddMode {
                    mode: Mode {
                        id: mode,
                        name: "Mode 1".into(),
                    },
                    index: 0,
                    cells: vec![],
                },
                add_brand,
                add_only,
                rect(src, there.first_page()),
                Operation::SetFills {
                    id: src,
                    fills: keyed_by_position([Fill {
                        brush: bound(brand),
                        visible: true,
                    }]),
                },
                Operation::SetStrokes {
                    id: src,
                    strokes: keyed_by_position([Stroke {
                        brush: bound(only),
                        width: 2.0,
                        ..Default::default()
                    }]),
                },
            ]))
            .expect("the copying document");
        let props = ondin_core::build::properties_of(&there, src).expect("a rect's properties");

        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0xB2B);
        app.session.adopt_document(Document::new(ids.mint()), None);
        let root = app.session.doc.first_page();
        let ids = &mut app.session.ids;
        let mode = ids.mint_mode();
        let (_, add_brand) = colour(ids, mode, "brand/primary");
        let (local, add_local) = colour(ids, mode, "local/x");
        let (a, b, style) = (ids.mint(), ids.mint(), ids.mint_style());
        let red = keyed_by_position([Fill {
            brush: Color::from_rgba8(255, 0, 0, 255).into(),
            visible: true,
        }]);
        assert!(app.commit_edit(Transaction(vec![
            Operation::AddMode {
                mode: Mode {
                    id: mode,
                    name: "Mode 1".into(),
                },
                index: 0,
                cells: vec![],
            },
            add_brand,
            add_local,
            rect(a, root),
            rect(b, root),
            Operation::SetFills {
                id: a,
                fills: red.clone(),
            },
            Operation::AddStyle {
                style: Style {
                    id: style,
                    name: "Surface".into(),
                    value: StyleValue::Fills(red),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::SetFillStyle {
                id: a,
                style: Some(style),
            },
            Operation::SetFills {
                id: b,
                fills: keyed_by_position([Fill {
                    brush: bound(local),
                    visible: true,
                }]),
            },
        ])));
        assert_eq!(app.session.doc.get(a).unwrap().styles().fills, Some(style));

        app.property_clipboard = Some(props);
        app.session.selection.set(vec![a, b]);
        app.paste_properties();

        let doc = &app.session.doc;
        assert_eq!(doc.get(a).unwrap().styles().fills, None, "the style cut");
        let fill = doc.get(b).unwrap().paint().fills[0].brush.var();
        let here = doc.variables().by_name("brand/primary").map(|v| v.id);
        assert_eq!(
            fill, here,
            "the fill rebound to this document's brand/primary"
        );
        assert_eq!(
            app.session.status().text,
            "Pasted properties onto 2 layers · Rebound brand/primary · \
             Detached 3 styles and variables"
        );
    }
}

/// Which floating things occupy the popover lane.
#[cfg(test)]
mod popover_lane_tests {
    use super::{OndinApp, POPOVER_W, TopMenu};

    /// Every popover in the lane reserves it; the clean row is the control against
    /// reserving unconditionally. flip: drop `export_menu` and `export_row_open` from `a_popover_is_open` →
    /// fails on those two rows
    #[test]
    fn every_popover_in_the_lane_reserves_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
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
                app.picker_lane_right(&ctx),
                lane,
                "{name} must reserve the lane"
            );
        }

        let clean = OndinApp::headless(&ctx);
        let screen = ctx.input(|i| i.content_rect());
        assert_eq!(
            clean.picker_lane_right(&ctx),
            OndinApp::inspector_column(screen).left(),
            "with nothing open the picker goes beside the cards"
        );
        let _ = TopMenu::None;
    }
}

/// `Ctrl+Shift+G` over more than one container.
#[cfg(test)]
mod ungroup_tests {
    use super::OndinApp;
    use ondin_core::build;
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    /// Four 20×20 rects in a row on the root, grouped into two pairs, with both
    /// groups selected.
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
                        corner_smoothing: 0.0,
                    },
                    transform: Some(Affine::translate((40.0 * i as f64, 0.0))),
                    name: None,
                }]))
                .expect("a rect");
                id
            })
            .collect();
        app.session.adopt_document(doc, None);
        let (tx, g1) = build::group(
            &app.session.doc,
            &app.session.resolved,
            &mut app.session.ids,
            &made[0..2],
        )
        .expect("a group");
        app.session.commit(tx);
        let (tx, g2) = build::group(
            &app.session.doc,
            &app.session.resolved,
            &mut app.session.ids,
            &made[2..4],
        )
        .expect("a group");
        app.session.commit(tx);
        app.session.selection.set(vec![g1, g2]);
        (app, g1, g2, made)
    }

    /// Groups reachable from the root, counted by walking: an undone `DeleteNode`
    /// need not hand back the same `NodeId`.
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
        walk(&app.session.doc, app.session.doc.first_page(), &mut n);
        n
    }

    /// One press of `Ctrl+Shift+G` is one undo step however many containers it
    /// dissolves (`build::ungroup_all` accumulates). flip: commit inside a per-container loop → fails at the first assertion
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
                .get(app.session.doc.first_page())
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

    /// Ungrouping selects the freed layer and says "Ungrouped". One group of one rect,
    /// so an empty selection afterwards would read as an ordinary deselect. flip: drop `session.info("Ungrouped")` → fails at the status assertion
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
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("a rect");
        app.session.adopt_document(doc, None);
        let (tx, group) = build::group(
            &app.session.doc,
            &app.session.resolved,
            &mut app.session.ids,
            &[rect],
        )
        .expect("a group");
        app.session.commit(tx);
        app.session.selection.set_one(group);
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

    // No half-applied-failure test: `ungroup_selection` filters out every
    // `build::ungroup` error, and `ungroup_all` builds nothing on `Err`.
}

/// The library wiring: what happens to the file when the app saves, autosaves,
/// renames or deletes. Every assertion is about the filesystem.
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

    /// Put the app in the state where the next `autosave_tick` fires: autosave on,
    /// the interval elapsed (`autosave_secs = 0` means off).
    fn arm_autosave(app: &mut OndinApp) {
        app.prefs.autosave_secs = 1;
        app.last_autosave = std::time::Instant::now() - std::time::Duration::from_secs(3600);
    }

    /// A headless app whose library is an empty temp folder, replaced after
    /// construction because `Prefs::base_folder` is only read by `OndinApp::with`.
    fn app_at(ctx: &egui::Context, root: PathBuf) -> OndinApp {
        let mut app = OndinApp::headless(ctx);
        app.prefs.base_folder = Some(root.clone());
        app.library = Library::open(root);
        app
    }

    /// Opening a document drops the per-corner and smoothing disclosures, whose `NodeId`s
    /// belong to the document being left. flip: remove `self.per_corner_radius.clear()` from `reset_transient_state` →
    /// fails at the emptiness assertion
    #[test]
    fn opening_a_document_drops_the_previous_ones_corner_disclosures() {
        let ctx = egui::Context::default();
        let root = temp_root("corner-disclosure");
        let mut app = app_at(&ctx, root.clone());

        let id = app.session.ids.mint();
        app.per_corner_radius.insert(id);
        app.corner_smoothing_open.insert(app.session.ids.mint());
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
        assert!(
            app.corner_smoothing_open.is_empty(),
            "the smoothing disclosure still holds {:?}",
            app.corner_smoothing_open
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A library document with one page, opened in a fresh app at `root`, and one
    /// named step committed and saved: a rect added.
    fn saved_with_a_step(
        ctx: &egui::Context,
        root: &std::path::Path,
    ) -> (PathBuf, ondin_core::NodeId) {
        use crate::theme::icon;
        use ondin_core::{Operation, Transaction};
        let mut app = app_at(ctx, root.to_path_buf());
        let mut ids = ondin_core::IdSource::new(0x415);
        let mut doc = ondin_core::Document::new(ids.mint());
        let path = crate::library::store::file_document(root, None, "Kept", &mut doc).unwrap();
        app.open_path(&path);
        let page = app.session.page;
        let rect = app.session.ids.mint();
        app.session.name_step("Add rectangle", icon::SQUARE);
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id: rect,
            parent: page,
            index: 0,
            kind: ondin_core::NodeKind::Rect {
                size: ondin_core::kurbo::Size::new(10.0, 10.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some("Card".into()),
        }])));
        assert!(app.save_file(false));
        (path, rect)
    }

    /// The undo history outlives the session: reopened, the document comes back with
    /// its steps, the current one *Saved*, and undoing walks back past the reopen.
    /// flip: `adopt_opened_with` without its `resume_history` → fails at the labels.
    #[test]
    fn a_reopened_document_keeps_its_undo_history() {
        let ctx = egui::Context::default();
        let root = temp_root("kept-history");
        let (path, rect) = saved_with_a_step(&ctx, &root);

        let mut app = app_at(&ctx, root.clone());
        app.open_path(&path);
        let labels: Vec<_> = app
            .session
            .history
            .done()
            .map(|s| s.note.label.clone())
            .collect();
        assert_eq!(labels, ["Add rectangle"]);
        assert_eq!(
            app.session.saved_step(),
            Some(app.session.history.current())
        );
        assert!(app.session.resumed_history());

        app.undo();
        assert!(
            app.session.doc.get(rect).is_none(),
            "undone across the reopen"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A document saved outside the library keeps no history, though it carries a
    /// library document's id: it would write over that document's.
    /// flip: `kept_history` without its root test → fails at the assertion.
    #[test]
    fn a_copy_outside_the_library_keeps_no_history() {
        let ctx = egui::Context::default();
        let root = temp_root("kept-history-outside");
        let (path, _) = saved_with_a_step(&ctx, &root);
        let mut app = app_at(&ctx, root.clone());
        app.open_path(&path);
        assert!(
            app.kept_history().is_some(),
            "fixture: filed in the library"
        );
        app.session.path = Some(std::env::temp_dir().join("elsewhere.ondin"));
        assert!(app.kept_history().is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A file changed while closed drops its kept history; a library rename, which
    /// rewrites only its metadata, keeps it; a *History size* of zero keeps none.
    /// flip: `store::rewrite` without its `restamp` → fails at "a rename keeps the
    /// history".
    #[test]
    fn a_kept_history_is_dropped_for_a_file_that_changed() {
        use ondin_core::{Operation, Transaction};
        let ctx = egui::Context::default();
        let root = temp_root("kept-history-changed");
        let (path, _) = saved_with_a_step(&ctx, &root);

        // Renamed in the library: kept.
        let mut app = app_at(&ctx, root.clone());
        app.library.refresh();
        let entry = app
            .library
            .entry_at(&path)
            .cloned()
            .expect("the file is filed");
        let path = crate::library::store::rename(&root, &entry, "Renamed").unwrap();
        app.open_path(&path);
        assert_eq!(
            app.session.history.undo_depth(),
            1,
            "a rename keeps the history"
        );

        // Changed by something else: dropped.
        let mut doc = app.session.doc.clone();
        let page = doc.first_page();
        doc.apply(&Transaction(vec![Operation::SetName {
            id: page,
            name: "Elsewhere".into(),
        }]))
        .unwrap();
        crate::library::store::write_document(&path, &doc).unwrap();
        let mut app = app_at(&ctx, root.clone());
        app.open_path(&path);
        assert!(
            !app.session.history.can_undo(),
            "a changed file keeps no history"
        );
        assert!(!app.session.resumed_history());

        // A size of zero: the next save forgets what was kept.
        let (path, _) = saved_with_a_step(&ctx, &root);
        let mut app = app_at(&ctx, root.clone());
        app.prefs.history_size = 0;
        app.open_path(&path);
        assert!(!app.session.history.can_undo(), "zero reads nothing back");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_app_opens_on_the_dashboard() {
        let ctx = egui::Context::default();
        let app = OndinApp::headless(&ctx);
        assert_eq!(app.view, View::Editor);
    }

    /// No test may write the developer's preferences file: `headless` and `app_at`
    /// both stay `ephemeral`. flip: remove `ephemeral: true` from `headless` → fails at the
    /// first assertion
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

    /// `Ctrl+S` pins a version and autosave does not (the two-argument shape of
    /// `save_file`). flip: make `autosave_tick` pin → fails at the autosave history
    /// assertion
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

        app.session.mark_unsaved();
        arm_autosave(&mut app);
        app.autosave_tick(&ctx);
        // The tick only queues; wait for the worker.
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

    /// An idle session writes nothing; without the `is_dirty` gate an open document
    /// rewrites its own bytes every interval and sync clients upload it.
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
        app.disk_settle();
        let after = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(before, after, "a clean document must not be rewritten");

        // The control: an `autosave_tick` that never writes would pass the above.
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

    /// The frame decides to autosave and does not pay for one (a 20 MB fixture). The
    /// `is_saving` and `is_dirty` assertions bracket the duration one, since a tick
    /// returning on a gate is fast too. flip: `self.save_file(false)` as the body
    /// after the gates → fails at `is_saving`
    #[test]
    fn an_autosave_of_a_photo_document_does_not_serialise_on_the_frame() {
        let ctx = egui::Context::default();
        let root = temp_root("autosave-photo");
        let mut app = app_with_a_photo(&ctx, root.clone());
        // Filed first: autosave declines a pathless document.
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
            "a queued write is not a saved document — the save dot's tooltip says \
             Saving…, and the dot stays red, because the bytes are not down"
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

    /// An edit made while the save is in flight leaves the session dirty: the worker
    /// serialises a clone taken at queue time, so the file is behind. flip: drop the
    /// `self.revision == at` guard in `EditorSession::finish_save` → fails at
    /// `is_dirty`
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
        // 20 MB of picture keeps the serialise running when this commits.
        let parent = app.session.doc.first_page();
        let id = app.session.ids.mint();
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
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

    /// An undo or redo made while the save is in flight leaves the session dirty and
    /// the crash snapshot standing: `undo`/`redo` must move `revision`, or
    /// `finish_save` marks the session clean and `recovery_tick` deletes the only
    /// copy. flip: drop the bump from `EditorSession::undo` → fails at the revision
    /// assertion
    #[test]
    fn an_undo_during_a_save_leaves_the_session_dirty_and_the_snapshot_standing() {
        use crate::library::recovery;
        use ondin_core::kurbo::Size;
        use ondin_core::{NodeKind, Operation, Transaction};

        let ctx = egui::Context::default();
        let root = temp_root("autosave-undone");
        let mut app = app_with_a_photo(&ctx, root.clone());
        app.save_file(false);
        let path = app.session.path.clone().expect("filed");
        let parent = app.session.doc.first_page();
        let id = app.session.ids.mint();
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        }])));
        let key = app.session.doc.meta().id.clone().expect("a filed document");
        let snapshot = root
            .join(recovery::RECOVERY_DIR)
            .join(format!("{key}.ondin"));

        for redo in [false, true] {
            arm_autosave(&mut app);
            app.autosave_tick(&ctx);
            assert!(app.session.is_saving(), "the fixture must be mid-save");
            let before = app.session.revision();
            assert!(if redo {
                app.session.redo()
            } else {
                app.session.undo()
            });
            assert_ne!(
                app.session.revision(),
                before,
                "an undo or a redo is a new document as far as every cache is \
                 concerned"
            );
            app.recovery.last = None;
            app.recovery_tick(&ctx);
            app.disk_settle();
            assert!(
                app.session.is_dirty(),
                "the file holds the document from before the {}, so a session \
                 that says Saved is claiming work is on disk that is not",
                if redo { "redo" } else { "undo" }
            );
            app.recovery_tick(&ctx);
            app.disk_settle();
            assert!(
                snapshot.exists(),
                "the snapshot is the only copy of the session's document on \
                 disk, and a clean reading must not have deleted it"
            );
            let on_disk = ondin_core::io::load(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(
                on_disk.get(id).is_some(),
                !redo,
                "and the file really is the other document"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A failed save ends the *Saving…* and leaves autosave running: the failure path
    /// must clear `saving`, or the `is_saving` gate switches autosave off. The
    /// session's path is a directory, so the rename really fails. flip: remove `session.save_failed()` from `apply_disk`'s
    /// `SaveFailed` arm → fails at the post-settle `is_saving` assertion
    #[test]
    fn a_failed_save_stops_saying_saving_and_leaves_autosave_armed() {
        let ctx = egui::Context::default();
        let root = temp_root("autosave-fails");
        let mut app = app_at(&ctx, root.clone());

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
            "the save dot's tooltip is stuck on Saving… for the rest of the \
             session, and the gate that reads it has just switched autosave off"
        );
        assert!(
            app.session.is_dirty(),
            "and the work is still only in memory, which is the half that was \
             always right"
        );
        assert!(
            app.session.status().text.contains("Save failed"),
            "a save that did not happen has to say so — the save dot staying red \
             is what it says when nothing was attempted either. It said \
             {:?}",
            app.session.status().text
        );

        arm_autosave(&mut app);
        app.autosave_tick(&ctx);
        assert!(
            app.session.is_saving(),
            "one failed write turned autosave off for the session"
        );
        app.disk_settle();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A writer that dies mid-save is noticed by the frame: with the worker gone no
    /// answer is sent, so `saving` would stay `Some` and autosave stay off. Uses
    /// `disk_results`, never `disk_settle`, which would pass unfixed code. flip: revert `drain`'s `Disconnected` arm to a bare `break` → fails at
    /// `is_saving`
    #[test]
    fn a_writer_death_mid_save_is_noticed_by_the_frame_and_not_only_by_the_exit() {
        let ctx = egui::Context::default();
        let root = temp_root("writer-dies");
        let mut app = app_at(&ctx, root.clone());

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

        app.disk_results();

        assert!(
            !app.session.is_saving(),
            "the save dot's tooltip is stuck on Saving… for the rest of the \
             session, and the gate that reads it has switched autosave off — with \
             no answer coming from anywhere, ever, because the thread that owed \
             one is gone"
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
    #[test]
    fn autosave_never_creates_a_file_on_its_own() {
        let ctx = egui::Context::default();
        let root = temp_root("no-create");
        let mut app = app_at(&ctx, root.clone());
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

    /// The snapshot covers the unfiled document autosave refuses to touch, without
    /// filing it, and stops once the file does. flip: give `recovery_tick` autosave's `path.is_none()` guard →
    /// fails at the `expect` on `recovery.key`
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

        // `save_file` alone: `file_untitled` as well would file the document twice.
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

    /// Recovery end to end: a document edited past a save, a crash (the app is
    /// dropped), a fresh app that offers the work back through `recovery::pending`,
    /// and the file untouched until the user saves. flip: `recover` writes the snapshot to
    /// `pending.target` → fails at the file-bytes assertion
    #[test]
    fn work_lost_to_a_crash_comes_back_and_the_file_waits_for_the_user() {
        use crate::library::recovery;
        let ctx = egui::Context::default();
        let root = temp_root("recover");
        let mut app = app_at(&ctx, root.clone());

        let mut meta = app.session.doc.meta().clone();
        meta.name = Some("Landing v4".into());
        app.session.doc.set_meta(meta);
        app.session.mark_unsaved();
        app.save_file(false);
        let path = app.session.path.clone().expect("the fixture is filed");
        let on_disk = std::fs::read(&path).unwrap();

        // Work that never reaches the file, then the process dies.
        let mut meta = app.session.doc.meta().clone();
        meta.name = Some("Landing v5".into());
        app.session.doc.set_meta(meta);
        app.session.mark_unsaved();
        app.recovery_tick(&ctx);
        let key = app.recovery.key.clone().expect("a snapshot was taken");
        // No settle, deliberately: `Writer`'s `Drop` joining its worker puts the
        // queued snapshot on disk before the relaunch.
        drop(app);

        let mut app = app_at(&ctx, root.clone());
        app.library.refresh();
        app.open_at_launch(None);
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
        // `assert!` rather than `assert_eq!` on two `Vec<u8>`: a failure prints kilobytes.
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
        assert_eq!(app.session.saved_ago(), None, "the save dot reads red");
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
    /// document it belongs to. Pins the outcome, not the statement order: a
    /// simplified reopen through `open_path` would `drop_recovery` and fail it.
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

        let mut meta = app.session.doc.meta().clone();
        meta.name = Some("Landing v5".into());
        app.session.doc.set_meta(meta);
        recovery::write(&root, &id, &app.session.doc).unwrap();
        drop(app);

        let mut app = app_at(&ctx, root.clone());
        app.prefs.reopen_last = true;
        app.prefs.last_document = Some(path.clone());
        app.open_at_launch(None);
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

    /// A document named on the command line opens over `reopen_last`, is remembered as
    /// the last document, and still leaves its recovery to ask about; a file that does
    /// not load leaves the library showing.
    /// flip: `open_at_launch` ignoring `open` → fails at the opened path.
    #[test]
    fn a_document_named_at_launch_opens() {
        use crate::library::recovery;
        let ctx = egui::Context::default();
        let root = temp_root("launch-open");
        let mut app = app_at(&ctx, root.clone());
        let mut meta = app.session.doc.meta().clone();
        meta.name = Some("Named".into());
        app.session.doc.set_meta(meta);
        app.session.mark_unsaved();
        app.save_file(false);
        let named = app.session.path.clone().unwrap();
        let id = app.session.doc.meta().id.clone().unwrap();
        // Work the file is missing, or the snapshot is nothing to ask about.
        let mut meta = app.session.doc.meta().clone();
        meta.name = Some("Named v2".into());
        app.session.doc.set_meta(meta);
        recovery::write(&root, &id, &app.session.doc).unwrap();
        drop(app);

        let mut app = app_at(&ctx, root.clone());
        app.prefs.reopen_last = true;
        app.prefs.last_document = Some(root.join("elsewhere.ondin"));
        app.open_at_launch(Some(&named));
        assert_eq!(app.session.path.as_deref(), Some(named.as_path()));
        assert!(matches!(app.view, View::Editor));
        assert_eq!(app.prefs.last_document.as_deref(), Some(named.as_path()));
        assert_eq!(app.recovery.pending.len(), 1, "its recovery still asks");

        let mut app = app_at(&ctx, root.clone());
        app.open_at_launch(Some(&root.join("missing.ondin")));
        assert!(
            matches!(app.view, View::Dashboard),
            "a missing file stays on the library"
        );
        assert!(app.session.path.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Escape and the backdrop mean *Later*, and *Later* does not delete. Driven
    /// through `OndinApp::ui`: the card runs above the dashboard's early return. flip: map `should_close()` to `RecoverChoice::Discard` → fails at the
    /// snapshot-exists assertion
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
        // Shaped like `library::ids::mint`'s: `recovery::write` refuses anything else.
        const ORPHAN_KEY: &str = "0ba9f11ce0ba9f11ce0ba9f11ce0ba9f";
        recovery::write(&root, ORPHAN_KEY, &doc).unwrap();
        app.library.refresh();
        app.open_at_launch(None);
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
        // `should_close` gates Escape on last pass's modal stack.
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

    /// A modal owns the keyboard: the gate in `<OndinApp as eframe::App>::ui` must
    /// cover every modal, since `egui_wants_keyboard_input()` is false for a modal of
    /// buttons. The no-modal row is the positive control. flip: narrow the arm to `self.settings.is_some()` → fails at the
    /// `confirming_close` row
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
                        corner_smoothing: 0.0,
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

        assert_eq!(
            survives(&ctx, "none"),
            (false, 1),
            "control: with nothing up, Delete must delete"
        );
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
    /// write, however long the interval. Watches `recovery.last`, which only
    /// advances on a write. flip: drop the `recovery.at` comparison → fails at the last assertion
    #[test]
    fn an_idle_dirty_document_is_snapshotted_once_rather_than_every_interval() {
        let ctx = egui::Context::default();
        let root = temp_root("snapshot-idle");
        let mut app = app_at(&ctx, root.clone());
        app.session.mark_unsaved();

        app.recovery_tick(&ctx);
        // `recovery.at` is written only from the worker's answer.
        app.disk_settle();
        let first = app.recovery.last.expect("the first tick writes");
        assert_eq!(app.recovery.at, Some(app.session.revision()));

        app.recovery.last = Some(std::time::Instant::now() - std::time::Duration::from_secs(3600));
        app.recovery_tick(&ctx);
        assert!(
            app.recovery.last.is_some_and(|t| t < first),
            "an idle session writes nothing, so the clock is not restamped either"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The frame decides to snapshot and does not pay for one: a 20 MB document
    /// serialises far over the 50 ms bound. The key and the on-disk size keep the
    /// duration assertion from being vacuous. flip: put
    /// `recovery::write(&root, &key, &self.session.doc)` back on the frame in
    /// `recovery_tick` → fails at the duration assertion
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

    /// A document with 20 MB of picture, so a snapshot of it is still being written
    /// when the next thing happens. The bytes are never decoded.
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

    /// A removal cannot be overtaken by a write already in flight: `forget_snapshot`
    /// queues behind it, where a direct removal would delete a file not there yet and
    /// the write would put it back. flip: call
    /// `crate::library::recovery::remove` unconditionally in `forget_snapshot` →
    /// fails at the only assertion
    #[test]
    fn discarding_a_snapshot_beats_the_write_that_is_still_in_flight() {
        use crate::library::recovery;

        let ctx = egui::Context::default();
        let root = temp_root("snapshot-discard-race");
        let mut app = app_with_a_photo(&ctx, root.clone());

        app.recovery_tick(&ctx);
        let key = app.recovery.key.clone().expect("the tick took a snapshot");
        // No settle: the write must be unfinished here.
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

    /// Renaming the open document moves the session's path with it, or the next
    /// autosave recreates the old name. flip: drop the `session.path` update → fails
    /// at the `session.path` assertion (the on-disk rename happens either way)
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
        assert_eq!(
            app.session.doc.meta().name.as_deref(),
            Some("Homepage hero")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Deleting the open document clears the session's path, or the next autosave
    /// writes it straight back.
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

    /// The trash's retention window is enforced at launch, not only from the
    /// library-settings *Save* handler. flip: remove the `purge_trash` call from
    /// `open_at_launch` → fails at the `aged` assertion; `keep_days` read as `0` →
    /// fails at `fresh`
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

        // The retention clock is the trashed file's mtime (`store::trash_stamp`).
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

        app.open_at_launch(None);

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

    /// Importing copies, keeps the name that arrived, and mints a fresh id (two
    /// documents claiming one identity would share history and thumbnail).
    #[test]
    fn importing_copies_a_document_and_gives_it_a_new_identity() {
        let ctx = egui::Context::default();
        let root = temp_root("import");
        let outside = temp_root("import-source");
        let mut app = app_at(&ctx, root.clone());

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

        app.import_paths(&[source]);
        let listed = crate::library::scan::scan(&root);
        assert_eq!(listed.len(), 2);
        assert_ne!(listed[0].meta.id, listed[1].meta.id);

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// A file already inside the library is skipped, since the dialog may open on the
    /// base folder. flip: remove the `starts_with(&root)` guard → fails at the count
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

    /// A document with no metadata block takes its name from its filename, the same
    /// guess the list makes.
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

    /// An archived project is not a destination for a new document. Three doors
    /// must agree: `new_file_into` (the card), `nav_project` (where `Ctrl+N` files)
    /// and `import_counting`. flip: `Projects::destination` reverted to plain `get`
    /// → fails at the card assertion; the import row is a separate call site
    #[test]
    fn an_archived_project_is_not_a_destination_for_a_new_document() {
        let ctx = egui::Context::default();
        let root = temp_root("archived-dest");
        let mut app = app_with_a_project(&ctx, root.clone());
        app.dash.nav = crate::panels::dashboard::Nav::Project("p-1".into());

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

        // Set directly: `set_archived` is private.
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

    /// A project whose files could not all be moved is kept, and the user stays on
    /// its heading where the retry is. flip: remove the `failed == 0` guard
    /// from the `retain` → fails at the first assertion
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
        assert!(root.join("kestrel").exists());
        assert_eq!(app.dash.nav, crate::panels::dashboard::Nav::All);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A project transfer takes the open document with it: `delete_project` must
    /// re-point `session.path` on the transfer branch too, or the next autosave
    /// recreates the file under the same `meta.id`. flip: put `&& !keep_files` back on the follow → fails at "the open
    /// document is where the transfer put it"
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

    /// The other arm: the files go to Trash, not outright deletion.
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

    /// Transferring to *no project* leaves the documents unfiled and still in the
    /// library, rather than a `None` target meaning "do nothing".
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

    /// A broken `projects.json` stops a project delete before it moves any files.
    /// flip: move the check below the file-moving loop → fails at the
    /// trashed and entries assertions
    #[test]
    fn a_broken_projects_file_stops_a_project_delete_before_it_moves_anything() {
        let ctx = egui::Context::default();
        let root = temp_root("del-proj-latch");
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
    /// library with it, and switching folders without it leaves the files behind.
    #[test]
    fn changing_the_base_folder_moves_the_library_only_when_asked() {
        let ctx = egui::Context::default();
        let old = temp_root("settings-old");
        let new = temp_root("settings-new");
        let mut app = app_with_a_project(&ctx, old.clone());
        assert_eq!(app.library.entries.len(), 2);

        app.library_settings = Some(crate::panels::dashboard::LibrarySettings {
            base_folder: new.display().to_string(),
            migrate: false,
            ..crate::panels::dashboard::LibrarySettings::from_prefs(&app.prefs, &old)
        });
        app.apply_library_settings();
        assert_eq!(app.library.root, new);
        assert!(app.library.entries.is_empty(), "the new folder is empty");
        assert_eq!(crate::library::scan::scan(&old).len(), 2, "nothing moved");

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

    /// *Move my library* takes the open document with it: `session.path` is absolute,
    /// so the next save would otherwise recreate the document in the abandoned folder
    /// under the same `meta.id`. flip: delete the
    /// re-point loop in `apply_library_settings` → fails at "the open document is in
    /// the library it now says it is in"; replace the map lookup with a
    /// `strip_prefix(&old_root).join(&new_root)` swap → fails at "it is the file that
    /// moved", so neither assertion is redundant
    #[test]
    fn moving_the_library_takes_the_open_document_with_it() {
        let ctx = egui::Context::default();
        let old = temp_root("settings-open-old");
        let new = temp_root("settings-open-new");
        let mut app = app_with_a_project(&ctx, old.clone());
        let open = app.library.entries[0].path.clone();
        app.open_path(&open);
        app.prefs.last_document = Some(open.clone());
        assert_eq!(app.session.path.as_deref(), Some(open.as_path()));

        // `relocate` renames the migrated document; a prefix swap would name this one.
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

    /// Pointing the library at a folder that does not exist creates it rather than
    /// opening an unavailable library that refuses every write.
    ///
    /// flip: dropping the `create_dir_all` in `apply_library_settings` → fails at `may_write`
    #[test]
    fn a_base_folder_that_does_not_exist_yet_is_created_rather_than_refused() {
        let ctx = egui::Context::default();
        let old = temp_root("settings-make-old");
        let new = temp_root("settings-make-new");
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

    /// Cancelling the settings modal throws the draft away.
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
        app.library_settings = None;
        assert_eq!(app.prefs.autosave_secs, before);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The draft's base folder is seeded from the resolved root, not the raw preference.
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
        app.library_settings = Some(draft);
        app.apply_library_settings();
        assert_eq!(app.library.root, root);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The top bar names a document by its typed name and falls back to its filename.
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

        app.library.refresh();
        assert_eq!(
            app.library.entries[0].display_name(),
            app.session.document_title()
        );

        let bare = ondin_core::Document::new(app.session.ids.mint());
        let path = root.join("old-hero-sketch.ondin");
        std::fs::write(&path, ondin_core::io::save(&bare).unwrap()).unwrap();
        app.open_path(&path);
        assert_eq!(app.session.document_title(), "Old hero sketch");

        let fresh = app_at(&ctx, temp_root("title-fresh"));
        assert_eq!(fresh.session.document_title(), "Untitled");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The name sits 12 points from the logo mark on both screens. A gap belongs to
    /// the item before it, so the spacing is set before the mark is allocated.
    ///
    /// flip: allocating the editor's mark at its row's 8 → fails at the equality
    #[test]
    fn the_mark_and_the_name_are_as_far_apart_on_both_screens() {
        fn gap(shapes: &[egui::epaint::ClippedShape]) -> f32 {
            let font_atlas = egui::TextureId::default();
            let mut logo = None;
            let mut texts = Vec::new();
            fn walk(
                s: &egui::Shape,
                atlas: egui::TextureId,
                logo: &mut Option<egui::Rect>,
                texts: &mut Vec<egui::Pos2>,
            ) {
                match s {
                    egui::Shape::Mesh(m) if m.texture_id != atlas => {
                        *logo = Some(m.calc_bounds());
                    }
                    egui::Shape::Text(t) => texts.push(t.pos),
                    egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, atlas, logo, texts)),
                    _ => {}
                }
            }
            for cs in shapes {
                walk(&cs.shape, font_atlas, &mut logo, &mut texts);
            }
            let logo = logo.expect("the logo was painted");
            // Only the logo's own row: the library's sidebar labels also sit right of it.
            let first = texts
                .into_iter()
                .filter(|p| p.x >= logo.max.x && p.y < logo.max.y)
                .map(|p| p.x)
                .fold(f32::INFINITY, f32::min);
            first - logo.max.x
        }

        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = app_at(&ctx, temp_root("mark-gap"));
        let mut editor = Vec::new();
        let mut library = Vec::new();
        // Several passes: the texture uploads on the first, row sizes settle after.
        for _ in 0..3 {
            editor = ctx.run_ui(Default::default(), |ui| app.top_bar(ui)).shapes;
            library = ctx
                .run_ui(Default::default(), |ui| app.dashboard_ui(ui))
                .shapes;
        }
        let (editor, library) = (gap(&editor), gap(&library));
        assert!(
            (library - 12.0).abs() < 0.5,
            "the library's gap is 12: {library}"
        );
        assert!(
            (library - editor).abs() < 0.5,
            "the library's gap {library} against the editor's {editor}"
        );
    }

    /// Settings sits `TOP_GAP` from the update pill on both screens, measured from
    /// the painted shapes (only the Ready pill is drawn here).
    ///
    /// flip: placing the library's Settings at its row's 12 → fails at the equality
    #[test]
    fn settings_sits_as_far_from_the_update_pill_on_both_screens() {
        fn gap(shapes: &[egui::epaint::ClippedShape]) -> f32 {
            let mut outlines = Vec::new();
            let mut label = None;
            let mut settings = None;
            for cs in shapes {
                match &cs.shape {
                    egui::Shape::Rect(r) if r.stroke.width > 0.0 => outlines.push(r.rect),
                    egui::Shape::Text(t) if t.galley.text().contains("RESTART") => {
                        label = Some(t.pos + t.galley.size() / 2.0);
                    }
                    egui::Shape::Text(t) if t.galley.text() == crate::theme::icon::SLIDERS => {
                        settings = Some(t.pos.x + t.galley.size().x / 2.0);
                    }
                    _ => {}
                }
            }
            // The pill is the outline around its own label; the search field is stroked too.
            let label = label.expect("the update pill's label was painted");
            let pill = outlines
                .into_iter()
                .find(|r| r.contains(label))
                .expect("the update pill was painted");
            let settings = settings.expect("Settings was painted");
            (settings - crate::settings::SETTINGS_BOX / 2.0) - pill.right()
        }

        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = app_at(&ctx, temp_root("pill-gap"));
        app.updater = crate::update::Updater::ready("9.9.9");
        let mut editor = Vec::new();
        let mut library = Vec::new();
        for _ in 0..3 {
            editor = ctx.run_ui(Default::default(), |ui| app.top_bar(ui)).shapes;
            library = ctx
                .run_ui(Default::default(), |ui| app.dashboard_ui(ui))
                .shapes;
        }
        let (editor, library) = (gap(&editor), gap(&library));
        assert!(
            (editor - crate::app::TOP_GAP).abs() < 0.5,
            "the editor's gap is TOP_GAP: {editor}"
        );
        assert!(
            (library - editor).abs() < 0.5,
            "the library's gap {library} against the editor's {editor}"
        );
    }

    /// Every string the top bar painted this pass; the crumb is three separate
    /// galleys, so callers look for each part.
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
    /// flip: lifting the separator out of the `if let` → fails at the leading-separator assertion
    #[test]
    fn the_breadcrumb_names_the_project_only_when_there_is_one() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("crumb");
        let mut app = app_with_a_project(&ctx, root.clone());

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

    /// The document menu makes a kit the library finds by its first 4 KB; stopping
    /// sharing keeps the version, and the Kit tag shows only while shared.
    ///
    /// flip: `make_kit` without minting an id → fails at "a pre-library kit is found by its id"
    #[test]
    fn the_document_menu_makes_a_kit_the_library_can_find() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("make-kit");
        let mut app = app_at(&ctx, root.clone());
        let mut meta = app.session.doc.meta().clone();
        meta.name = Some("Atlas".into());
        app.session.doc.set_meta(meta);
        assert!(app.session.path.is_none() && app.session.doc.meta().id.is_none());

        // Two frames: an egui `Area` measures itself invisibly on the first.
        app.open_menu = TopMenu::Document;
        top_bar_text(&ctx, &mut app);
        let painted = top_bar_text(&ctx, &mut app);
        assert!(
            painted.contains(&"Make this a kit".to_string()),
            "{painted:?}"
        );
        assert!(
            !painted.contains(&"Kit".to_string()),
            "no tag yet: {painted:?}"
        );

        app.make_kit();
        let id = app.session.doc.meta().id.clone();
        let scanned = crate::library::scan::scan(&root);
        let entry = scanned
            .iter()
            .find(|e| e.meta.id.is_some() && e.meta.id == id)
            .expect("the scan finds the kit by its id");
        assert!(entry.meta.kit.as_ref().is_some_and(|k| k.is_shared()));
        app.open_menu = TopMenu::Document;
        top_bar_text(&ctx, &mut app);
        let painted = top_bar_text(&ctx, &mut app);
        assert!(painted.contains(&"Kit".to_string()), "the tag: {painted:?}");
        assert!(
            painted.contains(&"Stop sharing as kit…".to_string()),
            "{painted:?}"
        );

        let mut meta = app.session.doc.meta().clone();
        meta.kit.as_mut().unwrap().version = 3;
        app.session.doc.set_meta(meta);
        app.stop_sharing_kit();
        assert!(app.shared_kit().is_none());
        let scanned = crate::library::scan::scan(&root);
        let kit = scanned[0].meta.kit.clone().unwrap();
        assert!(!kit.is_shared() && kit.version == 3, "{kit:?}");
        app.make_kit();
        assert_eq!(app.shared_kit().map(|k| k.version), Some(3), "continues");
        assert_eq!(crate::library::scan::scan(&root).len(), 1, "one file");

        let old = root.join("Old.ondin");
        let bare = ondin_core::Document::new(app.session.ids.mint());
        std::fs::write(&old, ondin_core::io::save(&bare).unwrap()).unwrap();
        app.library.refresh();
        app.open_path(&old);
        assert!(app.session.doc.meta().id.is_none(), "the fixture has no id");
        app.make_kit();
        let id = app.session.doc.meta().id.clone();
        assert!(
            crate::library::scan::scan(&root)
                .iter()
                .any(|e| e.path == old && e.meta.id.is_some() && e.meta.id == id),
            "a pre-library kit is found by its id"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A kit's bar reads name, save dot, Kit tag, accent Publish; the tag and Publish
    /// share one height and the status text is not in the bar.
    #[test]
    fn a_kits_bar_reads_dot_then_tag_then_an_accent_publish() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("kit-bar");
        let mut app = app_at(&ctx, root.clone());
        let main = app.session.ids.mint();
        let doc_root = app.session.doc.first_page();
        assert!(app.session.commit(ondin_core::Transaction(vec![
            ondin_core::Operation::CreateNode {
                id: main,
                parent: doc_root,
                index: 0,
                kind: ondin_core::NodeKind::Artboard {
                    size: ondin_core::kurbo::Size::new(80.0, 30.0),
                },
                transform: None,
                name: Some("Button".into()),
            },
            ondin_core::Operation::SetComponent {
                id: main,
                component: true,
            },
        ])));
        app.make_kit();
        assert!(app.shared_kit().is_some());
        app.session.info("SENTINEL message");

        let mut shapes = Vec::new();
        for _ in 0..2 {
            shapes = ctx.run_ui(Default::default(), |ui| app.top_bar(ui)).shapes;
        }
        let mut dot = None;
        let mut tag = None;
        let mut publish = None;
        let mut texts = Vec::new();
        for s in &shapes {
            match &s.shape {
                egui::Shape::Circle(c) if c.radius == 3.5 => dot = Some(c.center.x),
                egui::Shape::Text(t) => {
                    let text = t.galley.text().to_string();
                    if text == "Kit" {
                        tag = Some(t.pos.x);
                    }
                    if text == "Publish" {
                        publish = Some(t.pos.x);
                    }
                    texts.push(text);
                }
                _ => {}
            }
        }
        let (dot, tag, publish) = (
            dot.expect("the save dot"),
            tag.expect("the tag"),
            publish.expect("Publish"),
        );
        assert!(dot < tag, "the dot comes before the tag: {dot} {tag}");
        assert!(tag < publish, "the tag before Publish: {tag} {publish}");
        assert!(
            !texts.iter().any(|t| t.contains("SENTINEL")),
            "no status text in the bar: {texts:?}"
        );
        let grounds: Vec<egui::Rect> = shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Rect(r)
                    if r.fill == crate::theme::color::text_a(20)
                        || r.fill == crate::theme::color::ACCENT_800 =>
                {
                    Some(r.rect)
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            grounds.len(),
            2,
            "the tag's and Publish's grounds: {grounds:?}"
        );
        assert_eq!(grounds[0].height(), grounds[1].height(), "{grounds:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The View menu is wide enough for every row's label.
    ///
    /// flip: the card a fixed 100 wide rather than `menu_check_width` → fails at the
    /// label-inside-card assertion. At `MENU_W` today's labels all fit, so a floor-only
    /// card passes until a row outgrows it.
    #[test]
    fn the_view_menu_fits_its_widest_label() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        app.open_menu = TopMenu::View;
        let mut shapes = Vec::new();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 800.0),
            )),
            ..Default::default()
        };
        for _ in 0..2 {
            shapes = ctx.run_ui(input(), |ui| app.top_bar(ui)).shapes;
        }
        let card = ctx
            .memory(|m| m.area_rect(egui::Id::new("view-menu")))
            .expect("the View menu's area");
        let inset = crate::app::MENU_PAD + crate::ui::MENU_BORDER + 8.0;
        let rows = crate::input::ViewSwitch::VIEW_MENU
            .iter()
            .map(|s| s.label())
            .chain([crate::menu::Item::Present.spec().label]);
        for row in rows {
            let end = shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == row => {
                        Some(t.pos.x + t.galley.size().x)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("the row {row}"));
            assert!(
                end <= card.right() - inset + 0.5,
                "{row} ends at {end}, the card at {}",
                card.right()
            );
        }
    }

    /// The floating status bar shows a message for its hold, then nothing; the
    /// View switch silences it; the mode shows regardless.
    ///
    /// flip: one opacity for the whole card → fails at "the mode does not dip"
    #[test]
    fn the_status_bar_holds_a_message_then_lets_it_go() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let canvas = egui::Rect::from_min_size(egui::pos2(46.0, 46.0), egui::vec2(900.0, 600.0));
        let painted = |app: &mut OndinApp| -> Vec<String> {
            let mut out = Vec::new();
            for _ in 0..2 {
                out = ctx
                    .run_ui(Default::default(), |ui| app.status_bar(ui.ctx(), canvas))
                    .shapes
                    .iter()
                    .filter_map(|s| match &s.shape {
                        egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                        _ => None,
                    })
                    .collect();
            }
            out
        };
        assert!(painted(&mut app).is_empty(), "the control: nothing to say");
        app.session.info("Reordered");
        assert_eq!(painted(&mut app), vec!["Reordered".to_string()]);

        app.show_status = false;
        assert!(painted(&mut app).is_empty(), "the switch silences it");
        app.show_status = true;

        let hold = crate::app::status_hold(app.session.status());
        app.session.status_mut().at =
            std::time::Instant::now() - hold - std::time::Duration::from_millis(10);
        assert!(painted(&mut app).is_empty(), "gone after its hold");

        app.mode = crate::input::Mode::TextInsert;
        assert_eq!(
            painted(&mut app),
            vec!["INSERT".to_string()],
            "the mode stays"
        );

        app.session.info("Reordered");
        app.session.status_mut().at = std::time::Instant::now() - hold
            + std::time::Duration::from_secs_f32(crate::app::STATUS_FADE * 0.5);
        let mut faded = Vec::new();
        for _ in 0..2 {
            faded = ctx
                .run_ui(Default::default(), |ui| app.status_bar(ui.ctx(), canvas))
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some((
                        t.galley.text().to_string(),
                        t.opacity_factor,
                        t.galley.rows[0].visuals.mesh.vertices[0].color.a(),
                    )),
                    _ => None,
                })
                .collect();
        }
        let mode = faded
            .iter()
            .find(|(t, ..)| t == "INSERT")
            .expect("the mode is drawn");
        // A `Ui`'s opacity lands in the galley's mesh colours, not in `opacity_factor`.
        assert_eq!(
            mode.2, 255,
            "the mode does not dip with the message: {faded:?}"
        );
        let message = faded
            .iter()
            .find(|(t, ..)| t == "Reordered")
            .expect("the message is drawn while it fades");
        assert!(message.2 < 255, "and the message does fade: {faded:?}");

        let mut s = app.session.status().clone();
        s.text = "x".repeat(40);
        let info = crate::app::status_hold(&s);
        s.kind = crate::session::StatusKind::Error;
        assert!(crate::app::status_hold(&s) > info);
    }

    /// A failed save is not reported as a made kit; the mark stays in the document.
    ///
    /// flip: `make_kit` ignoring `save_file`'s answer → fails at the "Save failed" status
    #[test]
    fn a_kit_whose_save_failed_says_the_save_failed() {
        let ctx = egui::Context::default();
        let root = temp_root("kit-save-fails");
        let mut app = app_at(&ctx, root.clone());
        let blocked = root.join("blocked.ondin");
        std::fs::create_dir_all(&blocked).unwrap();
        app.session.path = Some(blocked);
        app.make_kit();
        let status = app.session.status().text.clone();
        assert!(status.contains("Save failed"), "{status}");
        assert!(app.shared_kit().is_some(), "the mark is in the document");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Draws the dashboard for real and asserts egui reports no id clash (its red
    /// warning rectangles) and nothing panics; it says nothing about how it looks.
    ///
    /// flip: a bare `"file-menu"` id instead of `("file-menu", path)` → fails on the `All` pass
    #[test]
    fn the_dashboard_draws_without_egui_complaining() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        // Set explicitly: egui defaults the clash warning to `debug_assertions`, so
        // in a release build the count below would be zero by construction.
        ctx.options_mut(|o| o.warn_on_id_clash = true);
        assert!(
            ctx.options(|o| o.warn_on_id_clash),
            "the clash warning is off, so this test cannot fail"
        );
        let root = temp_root("draw");
        let mut app = app_at(&ctx, root.clone());

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

        for nav in [
            crate::panels::dashboard::Nav::Recent,
            crate::panels::dashboard::Nav::All,
            crate::panels::dashboard::Nav::Starred,
            crate::panels::dashboard::Nav::Kits,
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

    /// Going to the dashboard writes the work first and rescans the library.
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

    /// Walking to the library mid-sentence commits the typed text: a text session
    /// commits nothing until it ends, so `go_to_dashboard` must finish it before saving.
    ///
    /// flip: removing the `finish_text_edit` guard in `go_to_dashboard` → fails at the file-text assertion
    #[test]
    fn walking_to_the_library_commits_what_was_typed() {
        let ctx = egui::Context::default();
        let root = temp_root("text-walk");
        let mut app = app_at(&ctx, root.clone());

        let id = app.session.ids.mint();
        let parent = app.session.doc.first_page();
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

        // No world point, so `begin_edit_text` selects all and the insert replaces the buffer.
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

        // The file is asserted before the session: a lost text is the symptom, not the open session.
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

    /// A typed opacity digit lands on the layer it was typed against, not on what is
    /// selected when the window shuts. The clock is driven through `RawInput::time`.
    ///
    /// flip: `fold_opacity_entry` committing against `self.session.selection` → fails on the moved row
    #[test]
    fn a_typed_opacity_digit_lands_on_the_layer_it_was_typed_against() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("opacity-subject");

        /// Two rects at opacity 1.0, A selected. Returns their alphas after pressing
        /// `4` at t=0.1 and the window shutting at t=1.0, with `moved` deciding the
        /// selection at the deadline.
        fn run(
            ctx: &egui::Context,
            root: &std::path::Path,
            moved: fn(&mut OndinApp, ondin_core::NodeId, ondin_core::NodeId),
        ) -> (f32, f32, usize) {
            let mut app = app_at(ctx, root.to_path_buf());
            let doc_root = app.session.doc.first_page();
            let (a, b) = (app.session.ids.mint(), app.session.ids.mint());
            let rect = |id| ondin_core::Operation::CreateNode {
                id,
                parent: doc_root,
                index: 0,
                kind: ondin_core::NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(20.0, 20.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            };
            app.session
                .try_commit(ondin_core::Transaction(vec![rect(a), rect(b)]))
                .expect("two rects");
            app.session.selection.set_one(a);
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

        assert_eq!(
            run(&ctx, &root, |_, _, _| {}),
            (0.4, 1.0, 1),
            "control: with the selection still on A, the digit commits to A"
        );

        assert_eq!(
            run(&ctx, &root, |app, _, b| app.session.selection.set_one(b)),
            (0.4, 1.0, 1),
            "a digit typed against A must land on A however the selection moves"
        );

        assert_eq!(
            run(&ctx, &root, |app, _, _| app.session.selection.clear()),
            (0.4, 1.0, 1),
            "clearing the selection must not take the typed value with it either"
        );
    }

    /// The top bar's Undo and Save verbs end a live text session before touching the
    /// document. Asserted at the verbs, not by clicking the glyphs.
    ///
    /// flip: removing `finish_text_first()` from the undo arm → fails at "the fixture's node is a text node"
    #[test]
    fn the_top_bars_undo_and_save_commit_what_was_typed_first() {
        let ctx = egui::Context::default();
        let root = temp_root("text-topbar");

        /// A filed document with one text node reading "hello", a live session on
        /// it holding "TYPED", and one prior undo step to rewind into.
        fn typed(ctx: &egui::Context, root: &std::path::Path) -> (OndinApp, ondin_core::NodeId) {
            let mut app = app_at(ctx, root.to_path_buf());
            let id = app.session.ids.mint();
            let parent = app.session.doc.first_page();
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

    /// A document chord in a text session finishes it first, because `dispatch`
    /// serves both the chord and the button. Driven through `dispatch`, not the keymap.
    ///
    /// flip: removing `finish_text_first()` from `dispatch`'s `Undo` arm → fails at "the chord ended the session"
    #[test]
    fn a_document_chord_in_a_session_finishes_it_first() {
        let ctx = egui::Context::default();
        let root = temp_root("text-chord");
        let mut app = app_at(&ctx, root.clone());
        let id = app.session.ids.mint();
        let parent = app.session.doc.first_page();
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

    /// `Escape` out of a chrome field is spent on the field, not on the selection.
    /// `chrome_focus` is set by hand, since it is written at the end of `update`.
    ///
    /// flip: deleting the `else if self.chrome_focus` rung → fails at "the selection survives"
    #[test]
    fn escape_out_of_a_chrome_field_does_not_also_clear_the_selection() {
        let ctx = egui::Context::default();
        let root = temp_root("escape-chrome");
        let mut app = app_at(&ctx, root.clone());
        let id = app.session.ids.mint();
        let parent = app.session.doc.first_page();
        app.session
            .try_commit(ondin_core::Transaction(vec![
                ondin_core::Operation::CreateNode {
                    id,
                    parent,
                    index: 0,
                    kind: ondin_core::NodeKind::Rect {
                        size: ondin_core::kurbo::Size::new(80.0, 40.0),
                        corner_radii: Default::default(),
                        corner_smoothing: 0.0,
                    },
                    transform: None,
                    name: None,
                },
            ]))
            .expect("the fixture's rect");
        app.session.selection.set_one(id);

        app.chrome_focus = true;
        app.escape(&ctx);
        assert_eq!(
            app.session.selection.ids(),
            &[id],
            "the selection survives: the key was spent on the field"
        );

        app.chrome_focus = false;
        app.escape(&ctx);
        assert!(
            app.session.selection.ids().is_empty(),
            "control: outside a field, Escape still clears the selection"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every close request cancels while the work is unsaved, not only the first;
    /// the title bar is not covered by the in-app modal, so a double ✕ sends two.
    ///
    /// flip: putting the `&& !self.confirming_close` term back → fails at the second request's assertion
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

        // Control: a clean session must not cancel, or a guard cancelling always would pass.
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

    /// A file dropped while a modal is up does not reach the document behind it:
    /// drops arrive before any widget runs, so the backdrop does not block them.
    ///
    /// flip: restoring the bare `self.take_dropped_images(&ctx)` → fails at the `settings` card
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
        // The revision, not a child count: `drop_images_at` picks the parent four ways.
        let kids = |app: &OndinApp| app.session.revision();

        let before = kids(&app);
        drop_a_file(&ctx, &mut app);
        assert_ne!(
            kids(&app),
            before,
            "the control never landed, so this test asserts nothing about modals"
        );

        for (name, arm) in [
            ("settings", 0usize),
            ("confirming_close", 1),
            ("recovery.pending", 2),
        ] {
            let mut app = app_at(&ctx, root_dir.clone());
            let settled = kids(&app);
            match arm {
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

    /// One drop of an SVG, a picture and a file that is neither: the SVG becomes layers
    /// in a group named for the file, the picture is placed, both end selected, and the
    /// status keeps the refusal ahead of both reports.
    /// flip: `take_dropped_images` skipping `import_svg_files` → fails at the group.
    #[test]
    fn a_dropped_svg_becomes_layers_beside_a_dropped_picture() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root_dir = temp_root("drop-svg");
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
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="red"/></svg>"#;
        let file = |name: &str, bytes: &[u8]| egui::DroppedFile {
            name: name.to_string(),
            bytes: Some(bytes.to_vec().into()),
            ..Default::default()
        };
        let input = egui::RawInput {
            dropped_files: vec![
                file("logo.svg", svg),
                file("drop.png", &png),
                file("fake.png", b"not a picture"),
            ],
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            app.session.next_status_pass();
            app.take_dropped_images(ui.ctx());
        });

        let doc = &app.session.doc;
        let group = ondin_core::subtree_nodes(doc, &[doc.first_page()])
            .into_iter()
            .find(|&id| doc.get(id).unwrap().name() == "logo")
            .expect("the SVG became a group named for its file");
        assert!(
            !doc.get(group).unwrap().children().is_empty(),
            "with its shape"
        );
        assert_eq!(
            app.session.selection.ids().len(),
            2,
            "the picture and the group"
        );
        assert!(app.session.selection.ids().contains(&group));
        let status = app.session.status();
        assert_eq!(status.kind, crate::session::StatusKind::Error);
        assert!(
            status.text.starts_with("Could not read: fake.png"),
            "{}",
            status.text
        );
        assert!(status.text.contains("Placed image"), "{}", status.text);
        assert!(
            status.text.contains("Imported logo.svg — 1 layer"),
            "{}",
            status.text
        );
        let _ = std::fs::remove_dir_all(&root_dir);
    }

    /// A picture dropped with an SVG on a selected shape places beside it, as the
    /// pill's "Place 2 files" says, instead of replacing the shape's fill; the
    /// picture alone replaces it.
    ///
    /// flip: `take_dropped_images` gating the replace on the pictures loaded, not
    /// the files dropped → fails at "the fill stands".
    #[test]
    fn a_picture_dropped_with_an_svg_does_not_replace_the_selected_fill() {
        use ondin_core::kurbo::{Affine, Point, Size};
        use ondin_core::peniko::Color;
        use ondin_core::{Brush, Fill, NodeKind, Operation, Transaction, keyed_by_position};
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root_dir = temp_root("drop-mixed-replace");
        let mut app = app_at(&ctx, root_dir.clone());
        app.canvas_rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let shape = app.session.ids.mint();
        let root = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: shape,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 100.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
            Operation::SetFills {
                id: shape,
                fills: keyed_by_position([Fill {
                    brush: Brush::Solid(Color::from_rgba8(0, 0, 255, 255)).into(),
                    visible: true,
                }]),
            },
        ])));
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
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="red"/></svg>"#;
        let file = |name: &str, bytes: &[u8]| egui::DroppedFile {
            name: name.to_string(),
            bytes: Some(bytes.to_vec().into()),
            ..Default::default()
        };
        app.session.camera.center = Point::new(50.0, 50.0);
        app.session.camera.zoom = 1.0;
        let at = app.to_screen(Point::new(50.0, 50.0), app.canvas_rect, 1.0);
        let drop = |app: &mut OndinApp, files: Vec<egui::DroppedFile>| {
            app.session.selection.set_one(shape);
            let input = egui::RawInput {
                events: vec![egui::Event::PointerMoved(at)],
                dropped_files: files,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                app.session.next_status_pass();
                app.take_dropped_images(ui.ctx());
            });
        };
        let image_fill = |app: &OndinApp| {
            matches!(
                app.session.doc.get(shape).unwrap().paint().fills[0]
                    .brush
                    .literal(),
                Some(Brush::Image(_))
            )
        };

        drop(
            &mut app,
            vec![file("logo.svg", svg), file("photo.png", &png)],
        );
        assert!(!image_fill(&app), "the fill stands");
        assert!(
            app.session.status().text.contains("Placed image"),
            "the picture is placed beside it: {}",
            app.session.status().text
        );

        drop(&mut app, vec![file("photo.png", &png)]);
        assert!(image_fill(&app), "fixture: the picture alone replaces it");
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

    /// `Escape` answers the Unsaved changes card with Cancel and leaves the tool alone.
    ///
    /// flip: dropping the `should_close()` arm → fails at `confirming_close`; the tool assertion is a regression guard and stays green
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

        // `should_close` needs the card's `Area` to exist.
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

    /// The ✕ is intercepted on the library screen too.
    ///
    /// flip: both `handle_close_request` calls back below `ui`'s dashboard return → fails at `confirming_close`
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

    /// One `Escape` over a top-bar dropdown closes the dropdown and nothing else; the
    /// keyboard guard does not cover it, as a click leaves no focused widget.
    ///
    /// flip: removing the `Action::Escape if open_menu != None` arm → fails at the tool assertion (the menu closes either way)
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

    /// A context menu outranks focus mode on Escape: the first press closes the
    /// menu and returns before `escape` runs, the second leaves focus mode.
    ///
    /// flip: moving the `context_menu` arm below the `input::resolve` call → fails at "the first Escape closed the menu"
    #[test]
    fn escape_in_focus_mode_closes_the_menu_first_and_the_mode_second() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("focus-menu-escape");
        let mut app = app_at(&ctx, root.clone());
        app.focus = true;
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
            app.focus,
            "and left focus mode alone: one rung per press, and the menu is the \
             rung the user can see"
        );

        whole_frame(&ctx, &mut app, vec![escape()]);
        assert!(
            !app.focus,
            "the second Escape leaves the mode, which is the rung underneath"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// Present mode, through whole frames and real keys: `Ctrl+Alt+Enter` opens it
    /// on the selected frame, the arrows step, an editing key does nothing, and
    /// Escape leaves with the selection and the view as they were.
    ///
    /// flip: `ui` running the editor below the present branch (the `return`
    /// dropped) → fails at "Delete edits nothing while presenting".
    #[test]
    fn present_mode_steps_frames_and_edits_nothing() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("present");
        let mut app = app_at(&ctx, root.clone());
        let page = app.session.doc.first_page();
        let (b, c) = (app.session.ids.mint(), app.session.ids.mint());
        let frame = |id, index, x: f64, name: &str| ondin_core::Operation::CreateNode {
            id,
            parent: page,
            index,
            kind: ondin_core::NodeKind::Artboard {
                size: ondin_core::kurbo::Size::new(400.0, 300.0),
            },
            transform: Some(ondin_core::kurbo::Affine::translate((x, 0.0))),
            name: Some(name.into()),
        };
        let n = app.root_artboards().len();
        assert!(app.session.commit(ondin_core::Transaction(vec![
            frame(b, n, 5000.0, "B"),
            frame(c, n + 1, 9000.0, "C"),
        ])));
        app.session.selection.set_one(c);
        let camera = app.session.camera;
        let shown = |app: &OndinApp| app.presenting.as_ref().map(|p| p.frames[p.at]);

        whole_frame(&ctx, &mut app, Vec::new());
        let ctrl_alt = egui::Modifiers {
            alt: true,
            ..egui::Modifiers::COMMAND
        };
        // `i.modifiers` is read off the raw input, not the event, so both carry it.
        let mut eframe_frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1320.0, 820.0),
            )),
            modifiers: ctrl_alt,
            events: vec![key(egui::Key::Enter, ctrl_alt)],
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| eframe::App::ui(&mut app, ui, &mut eframe_frame));
        assert_eq!(shown(&app), Some(c), "opens on the selected frame");

        whole_frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::ArrowLeft, Default::default())],
        );
        assert_eq!(shown(&app), Some(b), "the frame to its left");

        whole_frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::Delete, Default::default())],
        );
        assert!(
            app.session.doc.contains(c),
            "Delete edits nothing while presenting (the selected frame stands)"
        );

        let zoom_in = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, 3.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::COMMAND,
        };
        let scroll_down = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, -20.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        };
        let view = |app: &OndinApp| app.presenting.as_ref().and_then(|p| p.view);
        whole_frame(&ctx, &mut app, vec![zoom_in]);
        whole_frame(&ctx, &mut app, vec![scroll_down]);
        let zoomed = view(&app).expect("Ctrl+wheel zooms in on the frame");
        // The window is 820pt tall at one pixel per point, and the frames' tops are at 0.
        let top_gap = |v: crate::view::Camera| v.center.y - 820.0 / v.zoom / 2.0;
        assert!(
            top_gap(zoomed) > 1.0,
            "fixture: scrolled down off the frame's top, at {}",
            top_gap(zoomed)
        );
        assert_eq!(
            app.session.camera, camera,
            "without touching the editor's view"
        );
        whole_frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::ArrowRight, Default::default())],
        );
        let next = view(&app).expect("the next frame keeps the zoom");
        assert_eq!(next.zoom, zoomed.zoom, "at the same level");
        assert!(
            top_gap(next).abs() < 1e-6 && (next.center.x - 9200.0).abs() < 1e-6,
            "and opens at its top, centred across: {:?}",
            next.center
        );

        whole_frame(&ctx, &mut app, vec![escape()]);
        assert!(app.presenting.is_none(), "Escape leaves");
        assert_eq!(app.session.selection.ids(), &[c], "the selection as it was");
        assert_eq!(app.session.camera, camera, "and the view");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A kit dialog opened in focus mode is drawn, so Escape still gets out.
    /// Driven through whole frames with the real `Ctrl+Alt+P` chord.
    ///
    /// flip: the four kit dialogs back inside the `!focus` guard → fails at "the first Escape closed the card"
    #[test]
    fn a_kit_dialog_in_focus_mode_is_drawn_and_escape_leaves() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("focus-publish");
        let mut app = app_at(&ctx, root.clone());
        let main = app.session.ids.mint();
        let page = app.session.doc.first_page();
        assert!(app.session.commit(ondin_core::Transaction(vec![
            ondin_core::Operation::CreateNode {
                id: main,
                parent: page,
                index: 0,
                kind: ondin_core::NodeKind::Artboard {
                    size: ondin_core::kurbo::Size::new(40.0, 20.0),
                },
                transform: None,
                name: Some("Badge".into()),
            },
            ondin_core::Operation::SetComponent {
                id: main,
                component: true,
            },
        ])));
        app.make_kit();
        app.publish_now("First".into());
        assert!(app.session.commit(ondin_core::Transaction(vec![
            ondin_core::Operation::SetOpacity {
                id: main,
                opacity: 0.5,
            }
        ])));
        app.focus = true;

        let chord = egui::Modifiers {
            alt: true,
            ..egui::Modifiers::COMMAND
        };
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1320.0, 820.0),
            )),
            modifiers: chord,
            events: vec![egui::Event::Key {
                key: egui::Key::P,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: chord,
            }],
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| eframe::App::ui(&mut app, ui, &mut frame));
        assert!(
            app.publishing.is_some(),
            "fixture: the chord opened the card"
        );

        whole_frame(&ctx, &mut app, Vec::new());
        whole_frame(&ctx, &mut app, vec![escape()]);
        whole_frame(&ctx, &mut app, Vec::new());
        assert!(app.publishing.is_none(), "the first Escape closed the card");
        whole_frame(&ctx, &mut app, vec![escape()]);
        assert!(!app.focus, "and the second left focus mode");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Walking to the library clears `confirming_close`, so the card does not
    /// return over a clean document. The only assertion that reads the flag outside the card.
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
    //! The four context-menu rules of `context-menus.md` §10: R1's spent click, R4's
    //! replacement, R3's single rung, and no document action while a menu is open.
    //! They span frames and subsystems, so they live here, not in `menu.rs`.
    use super::OndinApp;
    use super::library_wiring_tests::whole_frame;
    use super::ungroup_tests::app_with_two_groups;
    use crate::tools::Tool;
    use ondin_core::{Document, NodeId};

    /// A secondary press or release as one event.
    fn secondary(at: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// A key press with no modifiers.
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
    /// The warm-up frame matters: egui resolves a press against last frame's widget rects.
    fn right_click(ctx: &egui::Context, app: &mut OndinApp, at: egui::Pos2) {
        whole_frame(ctx, app, Vec::new());
        whole_frame(ctx, app, vec![egui::Event::PointerMoved(at)]);
        whole_frame(ctx, app, vec![secondary(at, true)]);
        whole_frame(ctx, app, vec![secondary(at, false)]);
    }

    /// R1: a right-click opens its menu on the release, not the press (`context-menus.md` §1).
    ///
    /// flip: `canvas_context_menu` reading `button_pressed(Secondary)` instead of `secondary_clicked()` → fails at the press assertion
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

    /// R1's other half: a right-click that cancels a gesture is spent (opens no menu
    /// and selects nothing), and the next one opens a menu. The marquee leaves the
    /// pointer over the first group while the second is selected.
    ///
    /// flip: deleting `canvas_context_menu`'s `if self.gesture_cancelled { return; }` → fails at "and it selected nothing"
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

    /// R4: the right-click that opens the next menu replaces the last one; the
    /// dismissal and the open run in one frame, and the wrong order leaves no menu.
    ///
    /// flip: an early `if self.context_menu.is_some() { return; }` in `open_context_menu` → fails at the `expect` with `None`
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

    /// R3: one Escape over a menu opened by a real right-click closes the menu and
    /// keeps the selection underneath.
    ///
    /// flip: running `input::resolve` in the menu arm after `self.context_menu = None` → fails at the selection assertion (the menu closes either way)
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

    /// R3 with the whole ladder underneath: one Escape over a menu leaves the
    /// selection, `entered_group` and the tool untouched.
    ///
    /// flip: the dispatch loop over `input::resolve` in the menu arm after `self.context_menu = None` → fails at the `entered_group` assertion
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

    /// No document action fires while a menu is open: `Delete` over a menu leaves
    /// the node set and the dirty flag unchanged (the flag covers an undone edit).
    ///
    /// flip: running `input::resolve` in the menu arm's `else` → fails at the node-count assertion
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
        walk(&app.session.doc, app.session.doc.first_page(), &mut n);
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
                corner_smoothing: 0.0,
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
                    strokes: ondin_core::keyed_by_position([Stroke {
                        width: 2.0,
                        ..Default::default()
                    }]),
                },
                Operation::SetEffects {
                    id,
                    effects: ondin_core::keyed_by_position([ondin_core::Effect::new(
                        ondin_core::EffectKind::DropShadow(Default::default()),
                    )]),
                },
                Operation::SetExports {
                    id,
                    exports: ondin_core::keyed_by_position([ondin_core::ExportSpec::new(
                        ondin_core::ExportFormat::Png,
                        ondin_core::ExportScale::Times(1.0),
                    )]),
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

    /// One `Escape` closes each of the five popovers and pays out no rung of the
    /// ladder; the no-popover control proves `Escape` still pays out.
    ///
    /// flip: removing `a_popover_owns_escape`'s arm from `update` → fails at `stroke_menu`'s tool assertion
    /// flip: the arm swallowing every key → fails at "a tool letter still reaches the keymap"
    #[test]
    fn escape_over_a_popover_closes_it_and_pays_out_no_rung() {
        use crate::tools::Tool;

        let case = |name: &str,
                    mut app: OndinApp,
                    ctx: &egui::Context,
                    open: &dyn Fn(&mut OndinApp),
                    up: &dyn Fn(&OndinApp) -> bool| {
            app.tool = Tool::Rect;
            // Rects resolve a frame late, so an unsettled frame dismisses the popover early.
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

    /// A popover flag nothing is drawing does not own `Escape` (B: the selection
    /// leaves the text layer; A: focus mode hides the inspector).
    ///
    /// flip: dropping `popover_heard` from the gate → fails case B at "the press reaches the ladder"
    /// flip: dropping the dead-flag clearing → fails at "and the dead flag is cleared"
    #[test]
    fn a_popover_flag_nothing_draws_does_not_own_escape() {
        use crate::input::ViewSwitch;
        use crate::tools::Tool;
        let ctx = egui::Context::default();

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

        let mut app = probe_text_app(&ctx);
        whole_frame(&ctx, &mut app, Vec::new());
        app.type_menu = Some(crate::app::TypeTab::Character);
        whole_frame(&ctx, &mut app, Vec::new());
        app.set_view_switch(ViewSwitch::Focus, true);
        whole_frame(&ctx, &mut app, Vec::new());
        assert!(
            app.focus && app.type_menu.is_some(),
            "the fixture reached the state: in focus mode, with the flag still set"
        );
        whole_frame(&ctx, &mut app, vec![key(egui::Key::Escape)]);
        assert!(
            !app.focus,
            "one Escape leaves focus mode, as its toast says"
        );
    }
}

#[cfg(test)]
mod clipboard_gate_tests {
    //! Two threads opening the OS clipboard at once corrupt the heap, so
    //! `with_clipboard` serialises them behind a gate.

    /// The gate holds under eight threads doing nothing but opening it; the test only
    /// reads, never overwriting the developer's clipboard.
    ///
    /// flip: opening `arboard::Clipboard::new()` without `with_clipboard`'s `GATE` → the binary aborts with `STATUS_HEAP_CORRUPTION`, naming no test
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
    /// The check sits in the callers, so the test above can still reach a real
    /// handle. The flag is one-way, so this builds its own app.
    ///
    /// flip: dropping the `CLIPBOARD_OFF.store` from `OndinApp::headless` → fails at the first assertion (the reader assertions pass on an empty clipboard)
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
    //! A layer copied in one window pastes as a layer in another. One process stands
    //! in for two (`headless` sets `CLIPBOARD_OFF`); the text from `copy_selection`
    //! alone must rebuild the layers.

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
        walk(&app.session.doc, app.session.doc.first_page(), &mut out);
        out
    }

    /// Copy two groups in one app, hand the clipboard text to a second, and the six
    /// nodes arrive as layers with ids minted by the second.
    ///
    /// flip: `stamp_clipboard(ctx, heading)` in place of the payload write → fails at the `Clipboard::Layers` assertion
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

    /// The picture crosses too: the target never holds the image entry, and pasting
    /// twice must add it once (`missing_image_ops`).
    ///
    /// flip: `images: Vec::new()` in `adopt_clip_text`'s `Clip` → fails at the `image(&id)` assertion
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
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id: rect,
                fills: ondin_core::keyed_by_position([Fill {
                    brush: image_brush(id.clone()).into(),
                    visible: true,
                }]),
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

    /// Ordinary text is not mistaken for a copy, which keeps `Ctrl+V` over a sentence working.
    ///
    /// flip: `io::clip::read` answering `Some(parse(text))` for unfenced text → fails at the `Foreign` assertion, at `Unreadable`
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

    /// A copy this build cannot read stops the paste rather than becoming a text layer.
    ///
    /// flip: folding `Unreadable` into `Foreign` at `adopt_clip_text`'s error arm → fails at the `Unreadable` assertion
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

    /// The receipt (a length and a hash) says yes to exactly the text a copy wrote
    /// and no to anything else, and holds none of it.
    ///
    /// flip: `ClipStamp::of` hashing nothing → fails at "a text of the same length that is not ours"
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
    //! Undo is a structural edit, so app-side state that indexes into the document
    //! must be told. The tests assert on a position: the failures leave every index
    //! in range, so `retain_valid` would be a no-op.

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

    /// The path as the document stores it.
    fn stored(app: &OndinApp, id: NodeId) -> String {
        let NodeKind::Path { path, .. } = app.session.doc.get(id).expect("a live node").kind()
        else {
            panic!("a Path");
        };
        path.to_svg()
    }

    /// Delete the middle anchor, pick the last, `Ctrl+Z`: the selection must not
    /// come back naming a different point. The pre-undo reading is the in-run control.
    ///
    /// flip: removing `document_rewound`'s `points.clear()` → fails at "the selection may not survive an undo naming a different anchor"
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

        // `retain_valid` is a no-op here: `(0, 1)` is in range on the restored path.
        let mut kept = crate::preview::PointSet::default();
        kept.set_one(id, (0, 1));
        kept.retain_valid(&[3]);
        assert_eq!(
            kept.on(id).collect::<Vec<_>>(),
            vec![(0, 1)],
            "the guard the other doors use keeps an index the rewind has repointed"
        );
    }

    /// `Ctrl+Z` at the bottom of the stack changes nothing: the clear is conditional
    /// on `EditorSession::undo`'s answer, not on the keystroke.
    ///
    /// flip: `OndinApp::undo` calling `document_rewound` unconditionally → fails at `[]` against `[(100.0, 100.0)]`
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

    /// A pen resuming an existing path must not put back what an undo took:
    /// `PenEdit::subpaths` is a snapshot that `finish_pen` writes back whole.
    ///
    /// flip: removing `document_rewound`'s `self.pen = None` → fails at "an undo drops a pen run whose snapshot it has just invalidated"
    #[test]
    fn an_undo_drops_a_live_pen_run() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_three_anchors(&ctx);

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

    /// A `PenState` extending `id`, shaped as `canvas::pen_resume` shapes one;
    /// written out because `edited_subpaths` is private to `canvas`.
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
    //! A paint-row reorder is a gesture `cancel_gesture` can find: `ctx.dragged_id()`
    //! is cleared by any button's release, so a fast right-click must not leave the
    //! reorder uncancellable.

    use super::OndinApp;
    use crate::panels::paint::{PaintDrag, PaintList, Slots};
    use eframe::egui;
    use ondin_core::{Document, IdSource};

    /// A headless app with a paint-row reorder in flight and nothing else.
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

    /// A right-click cancels a paint-row reorder. The fixture has `ctx.dragged_id()`
    /// as `None`, the state a one-frame right-click leaves behind.
    ///
    /// flip: removing the `|| self.in_flight.paint_drag.is_some()` term → fails at "a paint-row reorder is not a gesture the cancel can find"
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
    //! A preview face arriving does not re-shape a document that has never heard of
    //! its family. `poll` cannot be driven headlessly, so these assert the predicate
    //! `OndinApp::ui` consults and the cost of getting it wrong.

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
                corner_smoothing: 0.0,
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

    /// An arriving family the document does not name is not the document's business,
    /// while one it names (in any case) must still invalidate.
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

    /// A family applied to a selection is a family the document names:
    /// `type_family_row` writes it into character spans, not `TextStyle::font_family`.
    ///
    /// flip: removing the `spans.as_slice()` loop → fails at "a family applied to a selection is not in the set"
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

    /// The cost the gate protects: invalidating re-shapes every text node, asserted
    /// as a shape count rather than a duration. It passes with or without the fix.
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

    /// A family nothing can supply is listed with its layers, and replacing it is
    /// one undo step that empties the list.
    ///
    /// flip: `missing_fonts` keeping every family whatever its status → fails at
    /// "Inter is not missing".
    #[test]
    fn a_missing_family_is_listed_and_replaced_in_one_step() {
        let ctx = egui::Context::default();
        let (mut app, text) = app_with_text_in(&ctx, "Gone Sans", 3);
        // The third layer in Inter, so the list has a family to leave out.
        let mut style = match app.session.doc.get(text[2]).map(|n| n.kind()) {
            Some(NodeKind::Text { style, .. }) => (**style).clone(),
            _ => panic!("the fixture is text"),
        };
        style.font_family = "Inter".into();
        app.session
            .try_commit(Transaction(vec![Operation::SetTextStyle {
                id: text[2],
                style,
                spans: None,
            }]))
            .expect("the fixture");
        app.fonts.settle_for_test();
        assert_eq!(
            ondin_core::families::families_in_use(&app.session.doc).len(),
            2,
            "the fixture: two families in use"
        );
        let missing = app.missing_fonts();
        assert_eq!(missing.len(), 1, "Inter is not missing: {missing:?}");
        assert_eq!(missing[0].0, "Gone Sans");
        assert_eq!(missing[0].1.len(), 2, "both layers set in it");

        let tx = ondin_core::families::replace_family(&app.session.doc, "Gone Sans", "Inter");
        app.commit_edit(tx);
        assert!(app.missing_fonts().is_empty(), "replaced everywhere");
        assert!(app.session.undo(), "one step");
        assert_eq!(app.missing_fonts().len(), 1, "and undone in one");
    }
}

#[cfg(test)]
mod close_confirmation_tests {
    //! The close confirmation is built from the app's own modal vocabulary:
    //! `settings::card` paints the focus ring, and Discard is a `FieldButton::Danger`.

    use super::OndinApp;
    use crate::theme::color;
    use eframe::egui;

    /// Every `RectShape` the last frame painted, flattened: an `egui::Frame` paints
    /// its background and shadow as a `Shape::Vec`.
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
        // Read the last frame: a modal's `Area` fades in, scaling every colour.
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

    /// Every stroked rect painted in the accent: the card's focus-visible ring.
    fn accent_rings(app: &mut OndinApp, ctx: &egui::Context, events: Vec<egui::Event>) -> usize {
        rects(app, ctx, events)
            .iter()
            .filter(|r| r.stroke.color == color::ACCENT && r.stroke.width > 0.0)
            .count()
    }

    /// One `Tab` puts the focus ring on screen; the no-key frame is the control,
    /// since `focus_ring` paints only after a `Tab`.
    ///
    /// flip: routing the card through `egui::Modal::new` → fails at the ring assertion with 0
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

    /// The card is on the app's card, not egui's: asserts the backdrop weight and the
    /// hairline, since `color::CARD` is also egui's window fill and discriminates nothing.
    ///
    /// flip: routing back through `egui::Modal::new` → fails at the backdrop with alpha 100
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
    //! a picture it can draw.

    use super::{ImageRefusal, OndinApp, looks_like_svg};

    /// A picture Ondin will not draw is not reported as one it could not read; the
    /// negative assertion matters, since a message saying both would pass the others.
    ///
    /// flip: `.ok().ok_or(ImageRefusal::NotAnImage)` back on `load_image_bytes`' `insert` → fails at the first assertion
    #[test]
    fn a_picture_too_big_to_draw_says_so_rather_than_could_not_read() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);

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

        // No `unwrap_err`: it needs `Debug` on `LoadedImage`, which holds the encoded bytes.
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

        let Err(plain) = app.load_image_bytes(b"not a picture at all".to_vec(), None, None) else {
            panic!("twenty bytes of prose became a picture")
        };
        assert_eq!(plain.message("notes.txt"), "Could not read: notes.txt");
    }

    /// The drop door (bytes) and the paste door (a string) give one answer per file.
    /// The last two rows are this side's own 512-byte cap and lossy decode.
    ///
    /// flip: the old `text.starts_with("<svg") || (text.starts_with("<?xml") && text.contains("<svg"))` body over a giving-up `str::from_utf8` → fails at the first assertion
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

        // The cap truncates rather than refusing: a split multi-byte character does not
        // make the file unreadable.
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
    //! Recovering does not strand the snapshot it is replacing.

    use super::*;

    /// `recover` drops the outgoing session's snapshot, which is safe only because
    /// `may_replace_current_document` runs first. The fixture sets `recovery.key` by
    /// hand, so the second press is covered by `a_second_recover_asks_before_it_spends_the_first`.
    #[test]
    fn recovering_drops_the_outgoing_snapshot_now_that_the_guards_are_there() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);

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

    /// A second Recover press asks before it spends the first: the recovered session
    /// is dirty and unfiled. `confirm_discard` raises a blocking `rfd` dialog, so this
    /// asserts the state the guard reads and that the pending entry remains.
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

        let pending = app.recovery.pending.pop().expect("one waiting");
        app.recover(&pending);

        // Dirty (`recover` marks it) and unfiled (no `target`).
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

    /// *Zoom to fit* on a box too small to fill the canvas stops at `MAX_ZOOM`, and
    /// on one too large at `MIN_ZOOM`, asserted by the constants rather than literals.
    ///
    /// flip: `MAX_ZOOM` at `100.0` and `zoom.clamp(0.02, 256.0)` restored at the call site → fails at the first assertion with 256.0 against 100.0
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
    //! What `Ctrl+A` selects, one test per decision: the candidate set (inside
    //! artboards plus loose non-artboard layers), the empty-set fallback to the
    //! frames, and a locked layer left alone while a hidden one is taken.

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
                corner_smoothing: 0.0,
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

    /// Everything inside artboards plus loose non-artboard layers, and not the
    /// artboards, which would make a following nudge move the whole canvas.
    ///
    /// flip: dropping the `!matches!(… Artboard …)` filter from `marquee_candidates` → fails at the set equality (the `!contains` assertion never runs)
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

    /// A document whose frames are empty selects the frames (the fallback arm).
    ///
    /// flip: replacing `self.root_artboards()` with `Vec::new()` → fails here; the test above never reaches the fallback
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

    /// A locked layer is left alone and a hidden one is taken: a hidden-but-unlocked
    /// layer is fully editable (`context-menus.md` §5.9), so `visible()` must not filter.
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

    /// `Ctrl+A` then `Delete` leaves a locked layer where it was, on a document whose
    /// whole range is locked.
    ///
    /// flip: `select_all` asking `marquee_candidates()` for its emptiness test instead of `marquee_range()` → fails only here
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
    //! Where `chrome_focus` is written, driven through whole frames
    //! (`eframe::App::ui` itself; the rung is tested with the field set by hand).
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

    /// The flag is the focus at the end of the frame; in point editing `Tab` steps an
    /// anchor and the focus egui moved is handed back. The control is an ordinary frame.
    ///
    /// flip: the write moved above the surrender → fails at "after the surrender"
    /// flip: the write deleted → fails at the control
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

    /// The library screen writes the flag as well, since it returns above the
    /// editor's write. Two frames: the second is the steady state.
    ///
    /// flip: the dashboard's write deleted → fails at "not stale"
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

/// The component verbs as the app reaches them: Delete owes the relinking,
/// *Create component* wraps, Detach cuts.
#[cfg(test)]
mod component_verb_tests {
    use super::OndinApp;
    use ondin_core::kurbo::Size;
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    /// A main frame `m` holding a rect, and an instance `i` of it, built through the
    /// app's own *Create component* and *Duplicate*.
    fn main_and_instance(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x7C0);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [m, r] = [(); 2].map(|_| ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: m,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(100.0, 100.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: r,
                parent: m,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
        ]))
        .expect("a frame holding a rect");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(m);
        app.create_component();
        assert!(
            app.session.doc.get(m).unwrap().component(),
            "the fixture's main"
        );
        app.duplicate_selection();
        let i = app
            .session
            .selection
            .single()
            .expect("the copy is selected");
        assert_eq!(
            app.session.doc.get(i).unwrap().link(),
            Some(m),
            "an instance"
        );
        (app, m, i)
    }

    /// Deleting a main with an instance goes through and detaches it, rather than
    /// being refused for the link it would leave, and keeps the instance's layers.
    ///
    /// flip: dropping `relink_for_delete` from `delete_selection` → leaves the main in place
    #[test]
    fn deleting_a_main_detaches_its_instance() {
        let ctx = egui::Context::default();
        let (mut app, m, i) = main_and_instance(&ctx);
        let kids = app.session.doc.get(i).unwrap().children().to_vec();
        assert_eq!(kids.len(), 1, "the fixture: the instance's rect");
        app.session.selection.set_one(m);
        app.delete_selection();
        assert!(app.session.doc.get(m).is_none(), "the main is gone");
        assert_eq!(
            app.session.doc.get(i).unwrap().link(),
            None,
            "the instance detached"
        );
        assert_eq!(
            app.session.doc.get(i).unwrap().children(),
            kids.as_slice(),
            "with its layers"
        );
    }

    /// A panel *Paste* beside a row inside a main, with that main on the clipboard,
    /// falls back to the ordinary paste (an instance of `m` inside `m` is
    /// `ComponentCycle`). A row outside any component keeps its slot.
    ///
    /// flip: `paste_is_held` answering `true` always → fails at "the paste went through"
    #[test]
    fn pasting_a_main_beside_a_row_inside_it_falls_back_to_the_ordinary_rule() {
        let ctx = egui::Context::default();
        let (mut app, m, i) = main_and_instance(&ctx);
        let r = app.session.doc.get(m).unwrap().children()[0];
        let root = app.session.doc.first_page();
        app.session.selection.set_one(m);
        app.copy_selection(&ctx);
        assert!(app.paste_clipboard_beside(r));
        assert!(
            !app.session.status().text.starts_with("Edit failed"),
            "the paste went through: {}",
            app.session.status().text
        );
        let pasted = app
            .session
            .selection
            .single()
            .expect("the paste is selected");
        assert_eq!(
            app.session.doc.get(pasted).unwrap().link(),
            Some(m),
            "an instance of m"
        );
        assert_eq!(
            app.session.doc.get(pasted).unwrap().parent(),
            Some(root),
            "beside the original, not inside it"
        );
        assert_eq!(
            app.session.doc.get(m).unwrap().children(),
            &[r],
            "m is untouched"
        );

        assert!(app.paste_clipboard_beside(i));
        let pasted = app
            .session
            .selection
            .single()
            .expect("the paste is selected");
        let siblings = app.session.doc.get(root).unwrap().children().to_vec();
        let at = |id| siblings.iter().position(|c| *c == id).unwrap();
        assert_eq!(at(pasted), at(i) + 1, "just above the row it was aimed at");
    }

    /// The labels of the context menu a user opens on `ids`; the menu is closed after.
    fn menu_on(app: &mut OndinApp, ctx: &egui::Context, ids: &[NodeId]) -> Vec<String> {
        app.session.selection.set(ids.to_vec());
        app.open_context_menu(
            ctx,
            crate::menu::Target::Layer {
                id: ids[0],
                door: crate::menu::Door::Canvas,
            },
            None,
        );
        let labels = app
            .open_menu_rows()
            .iter()
            .flatten()
            .map(|row| row.label.to_string())
            .collect();
        app.context_menu = None;
        labels
    }

    /// Each component role's menu rows, read off a real document: main `m` holding
    /// rect `r`, its instance `i` (copy `c` of `r`), a local rect `loc` inside `i`, a
    /// second main `m2`, and a Boolean property *Show* bound to `r`, set off on `i`.
    ///
    /// flip: `component_role`'s member arm answering `Role::Local` → fails at "a member goes to its main"
    /// flip: `combinable: false && …` in `menu_context` → fails at "two mains combine"
    /// flip: `holds_main: false && …` → fails at "two mains are not wrapped"
    /// flip: `property_reset`'s `.filter(|_| false)` → fails at "the member resets its property"
    #[test]
    fn the_menu_a_user_opens_reads_each_role_off_the_document() {
        use ondin_core::variant::{PropKind, PropValue};
        let ctx = egui::Context::default();
        let (mut app, m, i) = main_and_instance(&ctx);
        let r = app.session.doc.get(m).unwrap().children()[0];
        let c = app.session.doc.get(i).unwrap().children()[0];
        let root = app.session.doc.first_page();
        let [loc, m2] = [(); 2].map(|_| app.session.ids.mint());
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: loc,
                parent: i,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(4.0, 4.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: m2,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(50.0, 50.0),
                },
                transform: None,
                name: None,
            },
            Operation::SetComponent {
                id: m2,
                component: true,
            },
        ])));
        let (tx, _) = ondin_core::variant::define(
            &app.session.doc,
            &mut app.session.ids,
            m,
            "Show",
            PropKind::Boolean,
            vec![r],
        )
        .expect("a boolean property");
        assert!(app.session.commit(tx));
        let p = ondin_core::variant::instance_properties(&app.session.doc, i)[0]
            .value
            .clone();
        let ops = ondin_core::variant::set_property(
            &app.session.doc,
            &[i],
            &p,
            &PropValue::Boolean(false),
        );
        assert!(app.session.commit(Transaction(ops)));
        assert!(
            !app.session.doc.get(c).unwrap().visible(),
            "the fixture: Show off"
        );
        let has = |labels: &[String], l: &str| labels.iter().any(|x| x == l);

        let member = menu_on(&mut app, &ctx, &[c]);
        assert!(
            has(&member, "Go to main component"),
            "a member goes to its main: {member:?}"
        );
        assert!(!has(&member, "Create component"), "{member:?}");
        assert!(
            has(&member, "Reset Show"),
            "the member resets its property: {member:?}"
        );

        let local = menu_on(&mut app, &ctx, &[loc]);
        assert!(has(&local, "Create component"), "{local:?}");
        assert!(!has(&local, "Go to main component"), "{local:?}");

        let instance = menu_on(&mut app, &ctx, &[i]);
        for l in ["Go to main component", "Reset all", "Detach instance"] {
            assert!(has(&instance, l), "the instance offers {l}: {instance:?}");
        }

        let main = menu_on(&mut app, &ctx, &[m]);
        for l in ["Duplicate as component", "Select all instances"] {
            assert!(has(&main, l), "the main offers {l}: {main:?}");
        }
        assert!(!has(&main, "Create component"), "{main:?}");

        let both = menu_on(&mut app, &ctx, &[m, m2]);
        assert!(
            has(&both, "Combine as variants"),
            "two mains combine: {both:?}"
        );
        assert!(
            !has(&both, "Create component"),
            "two mains are not wrapped: {both:?}"
        );
    }

    /// *Go to main*, *Select all instances* on a main and on a set, and the delete's
    /// detach count: main `m`, instances `i` and `i2`, `c` the copy of `m`'s rect in `i`.
    ///
    /// flip: `select_all_instances`' `is_set` test made `false && …` → fails at "a set selects its variants' instances"
    /// flip: `delete_selection`'s detached filter turned to `!=` → fails at "the delete counts the two instances"
    #[test]
    fn go_to_main_select_all_instances_and_the_detach_count() {
        let ctx = egui::Context::default();
        let (mut app, m, i) = main_and_instance(&ctx);
        let r = app.session.doc.get(m).unwrap().children()[0];
        let c = app.session.doc.get(i).unwrap().children()[0];
        app.session.selection.set_one(m);
        app.duplicate_selection();
        let i2 = app.session.selection.single().expect("a second instance");

        app.session.selection.set_one(i);
        app.go_to_main();
        assert_eq!(
            app.session.selection.ids(),
            &[m],
            "an instance goes to its main"
        );
        app.session.selection.set_one(c);
        app.go_to_main();
        assert_eq!(
            app.session.selection.ids(),
            &[r],
            "a member goes to its source"
        );

        app.session.selection.set_one(m);
        app.select_all_instances();
        let mut got = app.session.selection.ids().to_vec();
        got.sort();
        let mut want = vec![i, i2];
        want.sort();
        assert_eq!(got, want, "a main selects its instances");

        let m2 = app.session.ids.mint();
        let root = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: m2,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(50.0, 50.0),
                },
                transform: None,
                name: None,
            },
            Operation::SetComponent {
                id: m2,
                component: true,
            },
        ])));
        app.session.selection.set(vec![m, m2]);
        app.combine_as_variants();
        let set = app.session.selection.single().expect("the set");
        assert!(ondin_core::variant::is_set(&app.session.doc, set));
        app.select_all_instances();
        let mut got = app.session.selection.ids().to_vec();
        got.sort();
        assert_eq!(got, want, "a set selects its variants' instances");

        // A second child, so a count of members (four) differs from instances (two).
        let r2 = app.session.ids.mint();
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id: r2,
            parent: m,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(4.0, 4.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        }])));
        assert_eq!(
            app.session.doc.get(i2).unwrap().children().len(),
            2,
            "the fixture: the child reached the instances"
        );
        app.session.selection.set_one(m);
        app.delete_selection();
        let text = app.session.status().text.clone();
        assert!(
            text.contains("2 instances detached"),
            "the delete counts the two instances: {text}"
        );
    }

    /// A kit instance's main is never selected: it lives in the undrawn vault.
    /// *Go to main* on the instance and on a member says where the main is and
    /// leaves the selection (a vault id in it would be pruned at the next commit).
    ///
    /// flip: removing `select_main`'s vault guard → fails at "an instance stays selected"
    #[test]
    fn a_kit_instances_main_is_never_selected() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut lids = IdSource::new(0xAC);
        let lroot = lids.mint();
        let mut kit = Document::new(lroot);
        let [m, r] = [(); 2].map(|_| lids.mint());
        kit.apply(&Transaction(vec![
            Operation::CreateNode {
                id: m,
                parent: lroot,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(40.0, 40.0),
                },
                transform: None,
                name: Some("Badge".into()),
            },
            Operation::CreateNode {
                id: r,
                parent: m,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::SetComponent {
                id: m,
                component: true,
            },
        ]))
        .unwrap();
        let published = ondin_core::kit::freeze(&kit).unwrap();
        let root = app.session.doc.first_page();
        let (tx, inst) = ondin_core::kit::insert(
            &app.session.doc,
            &mut app.session.ids,
            "5eed0000000000000000000000000001",
            1,
            &published,
            m,
            root,
        )
        .unwrap();
        assert!(app.session.commit(tx));
        let main = app.session.doc.get(inst).unwrap().link().unwrap();
        assert!(app.session.doc.in_vault(main), "the fixture vendored it");

        app.session.selection.set_one(inst);
        app.go_to_main();
        assert_eq!(
            app.session.selection.ids(),
            &[inst],
            "an instance stays selected"
        );
        assert!(app.session.status().text.contains("in a kit"));
        let member = app.session.doc.get(inst).unwrap().children()[0];
        app.session.selection.set_one(member);
        app.go_to_main();
        assert_eq!(
            app.session.selection.ids(),
            &[member],
            "a member stays selected"
        );

        app.session.selection.set(vec![inst, main]);
        app.session
            .selection
            .retain_existing(&app.session.doc, app.session.page);
        assert_eq!(app.session.selection.ids(), &[inst], "the vault id pruned");

        // Copied into another document, it lands an instance.
        // flip: `insert_all` calling `build::insert_subtrees` instead of `kit::insert_carrying` → fails at "still an instance"
        app.session.selection.set_one(inst);
        app.copy_selection(&ctx);
        let mut eids = IdSource::new(0xEA);
        let eroot = eids.mint();
        app.session.adopt_document(Document::new(eroot), None);
        assert!(app.paste_clipboard());
        let pasted = app.session.selection.single().expect("the paste selected");
        let main = app.session.doc.get(pasted).unwrap().link();
        let main = main.expect("still an instance");
        assert!(app.session.doc.in_vault(main));
    }

    /// A copy whose main does not come along lands detached, and the paste says so.
    /// Driven with a document's own main, the same case as a kit instance's vault main.
    ///
    /// flip: `detached` counted as zero → fails at "the paste says so"
    #[test]
    fn a_paste_that_detaches_an_instance_says_so() {
        let ctx = egui::Context::default();
        let (mut app, _, i) = main_and_instance(&ctx);
        app.session.selection.set_one(i);
        app.copy_selection(&ctx);
        let mut eids = IdSource::new(0xEB);
        app.session.adopt_document(Document::new(eids.mint()), None);
        assert!(app.paste_clipboard());
        let pasted = app.session.selection.single().expect("the paste selected");
        assert_eq!(
            app.session.doc.get(pasted).unwrap().link(),
            None,
            "detached"
        );
        let said = app.session.status().text.clone();
        assert!(
            said.contains("1 instance detached — its main is not in this document"),
            "the paste says so: {said}"
        );

        app.undo();
        let (mut home, _, i) = main_and_instance(&ctx);
        home.session.selection.set_one(i);
        home.copy_selection(&ctx);
        assert!(home.paste_clipboard());
        assert!(!home.session.status().text.contains("detached"));
    }

    /// A cut main moves: the cut detaches the instance and says it will follow; the
    /// first paste puts the main back under its own id and relinks it (an edit made
    /// meanwhile stays an override); a second paste is an instance.
    ///
    /// flip: `paste_cut_move` answering `false` → fails at "the main, under its own id"
    #[test]
    fn a_cut_main_pastes_back_as_a_move() {
        let ctx = egui::Context::default();
        let (mut app, m, i) = main_and_instance(&ctx);
        app.session.selection.set_one(m);
        app.cut_selection(&ctx);
        assert!(app.session.doc.get(m).is_none(), "cut");
        assert_eq!(app.session.doc.get(i).unwrap().link(), None, "detached");
        let said = app.session.status().text.clone();
        assert!(said.contains("will follow it on paste"), "{said}");
        assert!(app.session.commit(Transaction(vec![Operation::SetName {
            id: i,
            name: "mine".into(),
        }])));
        assert!(app.paste_clipboard());
        assert!(
            app.session.doc.get(m).is_some(),
            "the main, under its own id"
        );
        assert_eq!(app.session.doc.get(i).unwrap().link(), Some(m), "relinked");
        assert_eq!(
            app.session.doc.get(i).unwrap().name(),
            "mine",
            "its edit kept"
        );
        assert!(app.cut_move.is_none(), "taken");
        assert!(app.paste_clipboard());
        let two = app
            .session
            .selection
            .single()
            .expect("the paste is selected");
        assert_ne!(two, m);
        assert_eq!(app.session.doc.get(two).unwrap().link(), Some(m));
    }

    /// A cut main's nested copies come back to it too: `Card` holds two instances of
    /// `Star`, the second copy (`c2`) swapped to `Heart`; the paste restores link and swap.
    ///
    /// flip: the give-back filtered on the node being unlinked again → fails at "c1 follows Card's nested Star again"
    /// flip: giving back the link alone, the `SetSwap` dropped → fails at "still showing Heart"
    #[test]
    fn a_cut_main_gives_back_its_nested_copies() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0x7C1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [star, ss, heart, hs, m] = [(); 5].map(|_| ids.mint());
        let frame = |w: f64| NodeKind::Artboard {
            size: Size::new(w, w),
        };
        let rect = || NodeKind::Rect {
            size: Size::new(10.0, 10.0),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        };
        let create = |id, parent, index, kind, name: &str| Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: None,
            name: Some(name.into()),
        };
        doc.apply(&Transaction(vec![
            create(star, root, 0, frame(24.0), "Star"),
            create(ss, star, 0, rect(), "Shape"),
            create(heart, root, 1, frame(24.0), "Heart"),
            create(hs, heart, 0, rect(), "Shape"),
            create(m, root, 2, frame(100.0), "Card"),
            Operation::SetComponent {
                id: star,
                component: true,
            },
            Operation::SetComponent {
                id: heart,
                component: true,
            },
        ]))
        .expect("two icon mains and a card frame");
        let place = |doc: &mut Document, ids: &mut IdSource, main: NodeId, parent: NodeId| {
            let (tx, made) = ondin_core::insert_subtrees(
                doc,
                ids,
                &[ondin_core::Placement {
                    nodes: doc.capture_subtree(main).unwrap(),
                    parent,
                    index: None,
                }],
                Default::default(),
            );
            doc.apply(&tx).expect("an instance");
            made[0]
        };
        let n1 = place(&mut doc, &mut ids, star, m);
        let n2 = place(&mut doc, &mut ids, star, m);
        doc.apply(&Transaction(vec![Operation::SetComponent {
            id: m,
            component: true,
        }]))
        .expect("the card a main");
        let i = place(&mut doc, &mut ids, m, root);
        app.session.adopt_document(doc, None);
        let child = |app: &OndinApp, parent: NodeId, src: NodeId| {
            let doc = &app.session.doc;
            doc.get(parent)
                .unwrap()
                .children()
                .iter()
                .copied()
                .find(|c| doc.get(*c).and_then(|k| k.link()) == Some(src))
                .expect("a copy of it")
        };
        let (c1, c2) = (child(&app, i, n1), child(&app, i, n2));
        let n1s = app.session.doc.get(n1).unwrap().children()[0];
        let c1s = child(&app, c1, n1s);
        let tx = ondin_core::swap::swap(&app.session.doc, c2, heart).unwrap();
        assert!(app.session.commit(tx), "the fixture: c2 shows Heart");
        let c2s = app.session.doc.get(c2).unwrap().children()[0];
        let link = |app: &OndinApp, id: NodeId| app.session.doc.get(id).unwrap().link();

        app.session.selection.set_one(m);
        app.cut_selection(&ctx);
        assert_eq!(link(&app, i), None, "the fixture: i detached");
        assert_eq!(link(&app, c1), Some(star), "the fixture: c1 climbed");
        assert_eq!(link(&app, c2), Some(heart), "the fixture: c2 landed");
        let cut = ondin_core::io::save(&app.session.doc).unwrap();
        assert!(app.paste_clipboard());
        assert!(
            app.session.doc.get(m).is_some(),
            "the main, under its own id"
        );
        assert_eq!(link(&app, i), Some(m), "relinked");
        assert_eq!(
            link(&app, c1),
            Some(n1),
            "c1 follows Card's nested Star again"
        );
        assert_eq!(link(&app, c1s), Some(n1s), "and its layer");
        assert_eq!(link(&app, c2), Some(n2), "c2 follows Card's other Star");
        assert_eq!(
            app.session.doc.get(c2).unwrap().swap(),
            Some(heart),
            "still showing Heart"
        );
        assert_eq!(link(&app, c2s), Some(hs), "Heart's layer");
        let bytes = ondin_core::io::save(&app.session.doc).unwrap();
        let loaded = ondin_core::io::load(&bytes).expect("it reloads");
        assert_eq!(ondin_core::io::save(&loaded).unwrap(), bytes);
        app.undo();
        assert_eq!(
            ondin_core::io::save(&app.session.doc).unwrap(),
            cut,
            "one undo takes the paste back"
        );
    }

    /// An undone cut owes its paste nothing: after cut, undo, *Detach instance* and a
    /// Delete of the main, the paste must not relink the instance (the cut's move is
    /// parked once history is undone past it).
    ///
    /// flip: `document_rewound` leaving the move live on an undo → fails at "the detach stands"
    /// flip: `paste_cut_move` not refusing a parked move → fails at "the detach stands"
    #[test]
    fn an_undone_cut_does_not_move_on_paste() {
        let ctx = egui::Context::default();
        let (mut app, m, i) = main_and_instance(&ctx);
        app.session.selection.set_one(m);
        app.cut_selection(&ctx);
        assert!(app.cut_move.is_some(), "the fixture: a move owed");
        app.undo();
        assert_eq!(
            app.session.doc.get(i).unwrap().link(),
            Some(m),
            "the cut taken back"
        );
        app.session.selection.set_one(i);
        app.detach_instances();
        assert_eq!(app.session.doc.get(i).unwrap().link(), None, "detached");
        app.session.selection.set_one(m);
        app.delete_selection();
        assert!(app.session.doc.get(m).is_none(), "the main deleted");
        assert!(app.paste_clipboard());
        assert_eq!(
            app.session.doc.get(i).unwrap().link(),
            None,
            "the detach stands"
        );
        assert!(
            app.session.doc.get(m).is_none(),
            "an ordinary paste, under new ids"
        );
    }

    /// A redo of the cut is the cut again, and its paste a move: cut, undo, redo,
    /// paste restores the main under its own id and relinks its instance.
    ///
    /// flip: `document_rewound` dropping the move on the undo → fails at "the main, under its own id"
    #[test]
    fn a_redone_cut_still_moves_on_paste() {
        let ctx = egui::Context::default();
        let (mut app, m, i) = main_and_instance(&ctx);
        app.session.selection.set_one(m);
        app.cut_selection(&ctx);
        app.undo();
        app.redo();
        assert!(app.session.doc.get(m).is_none(), "the fixture: cut again");
        assert_eq!(app.session.doc.get(i).unwrap().link(), None, "detached");
        assert!(app.paste_clipboard());
        assert!(
            app.session.doc.get(m).is_some(),
            "the main, under its own id"
        );
        assert_eq!(
            app.session.doc.get(i).unwrap().link(),
            Some(m),
            "the instance follows it again"
        );
    }

    /// After *Go to main* from inside an instance, `Escape` clears the main's layer
    /// rather than yanking the selection back to the instance.
    ///
    /// flip: `leave_a_scope_the_selection_left` answering nothing → fails at "the scope is left"
    #[test]
    fn going_to_the_main_leaves_the_entered_instance() {
        let ctx = egui::Context::default();
        let (mut app, m, i) = main_and_instance(&ctx);
        app.session.selection.set_one(i);
        app.enter_action();
        assert_eq!(app.entered_group, Some(i), "the fixture: entered");
        app.go_to_main();
        let on_main = app.session.selection.single().expect("the main's layer");
        assert!(ondin_core::is_within(&app.session.doc, on_main, m));
        app.leave_a_scope_the_selection_left();
        assert_eq!(app.entered_group, None, "the scope is left");
        app.entered_group = Some(i);
        app.escape(&ctx);
        assert_ne!(
            app.session.selection.single(),
            Some(i),
            "the main's layer stays selected"
        );
    }

    /// `Enter` steps into an instance, as the double-click does, whatever its kind;
    /// its main, an ordinary frame, is not entered.
    ///
    /// flip: `enter_action` without its instance arm → leaves `entered_group` empty
    #[test]
    fn enter_steps_into_an_instance() {
        let ctx = egui::Context::default();
        let (mut app, m, i) = main_and_instance(&ctx);
        app.session.selection.set_one(m);
        app.enter_action();
        assert_eq!(app.entered_group, None, "a main is a frame, not entered");
        app.session.selection.set_one(i);
        app.enter_action();
        assert_eq!(app.entered_group, Some(i));
        let top = *app.session.doc.get(i).unwrap().children().last().unwrap();
        assert_eq!(app.session.selection.single(), Some(top));
    }

    /// Picking and entering an instance through `eframe::App::ui`: a click on its
    /// background or its rect selects it (its main's background selects nothing); a
    /// double-click enters; `Escape` leaves; `Enter` enters again.
    ///
    /// flip: `pick_leaf` without its instance clause → fails at the first assertion
    /// flip: `enter_action` without its instance arm → fails at "Enter steps back in"
    /// flip: the select arm reading egui's `Response::double_clicked` → fails at "a double-click on the instance's background enters it"
    #[test]
    fn a_click_a_double_click_and_enter_reach_an_instance_through_the_app() {
        use super::library_wiring_tests::whole_frame;
        use ondin_core::kurbo::Point;
        let primary = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let key = |key: egui::Key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        let ctx = egui::Context::default();
        let (mut app, m, i) = main_and_instance(&ctx);
        assert!(
            app.session
                .commit(Transaction(vec![Operation::SetTransform {
                    id: i,
                    transform: ondin_core::kurbo::Affine::translate((200.0, 0.0)),
                }]))
        );
        app.session.selection.clear();
        whole_frame(&ctx, &mut app, Vec::new());
        let (canvas, ppp) = (app.canvas_rect, ctx.pixels_per_point());
        let bounds = |app: &OndinApp, id| app.session.resolved.world_bounds(id).expect("bounds");
        let (main_box, inst_box) = (bounds(&app, m), bounds(&app, i));
        assert!(
            main_box.intersect(inst_box).area() == 0.0,
            "the fixture: the main and its instance side by side, {main_box:?} {inst_box:?}"
        );
        let (r, ir) = (
            app.session.doc.get(m).unwrap().children()[0],
            app.session.doc.get(i).unwrap().children()[0],
        );
        // The 10² rect sits in each frame's top-left corner; the middle is background.
        let on = |app: &OndinApp, p: Point| app.to_screen(p, canvas, ppp);
        let inst_ground = on(&app, inst_box.center());
        let main_ground = on(&app, main_box.center());
        let inst_rect = on(&app, bounds(&app, ir).center());
        let main_rect = on(&app, bounds(&app, r).center());
        for p in [inst_ground, main_ground, inst_rect, main_rect] {
            assert!(
                canvas.contains(p),
                "the fixture: {p:?} is on the canvas {canvas:?}"
            );
        }
        let click = |app: &mut OndinApp, at: egui::Pos2| {
            whole_frame(&ctx, app, vec![egui::Event::PointerMoved(at)]);
            whole_frame(&ctx, app, vec![primary(at, true)]);
            whole_frame(&ctx, app, vec![primary(at, false)]);
        };

        click(&mut app, inst_ground);
        assert_eq!(
            app.session.selection.single(),
            Some(i),
            "a click on the instance's background picks it, as a group's would"
        );
        click(&mut app, main_ground);
        assert!(
            app.session.selection.is_empty(),
            "control: the main's background is an occupied frame's, and picks nothing"
        );
        click(&mut app, inst_rect);
        assert_eq!(
            app.session.selection.single(),
            Some(i),
            "a click on a layer inside the instance picks the instance"
        );
        click(&mut app, main_rect);
        assert_eq!(
            app.session.selection.single(),
            Some(r),
            "control: a main's layer is picked directly"
        );
        assert_eq!(app.entered_group, None, "and nothing was entered");

        // No idle frames: egui would count this as a triple click; the app counts its
        // own (`ui::double_clicked`).
        whole_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(inst_ground)]);
        for pressed in [true, false, true, false] {
            whole_frame(&ctx, &mut app, vec![primary(inst_ground, pressed)]);
        }
        assert_eq!(
            app.entered_group,
            Some(i),
            "a double-click on the instance's background enters it"
        );

        whole_frame(&ctx, &mut app, vec![key(egui::Key::Escape)]);
        assert_eq!(app.entered_group, None, "Escape leaves it");
        assert_eq!(
            app.session.selection.single(),
            Some(i),
            "with the instance selected — the rung Enter starts from"
        );

        whole_frame(&ctx, &mut app, vec![key(egui::Key::Enter)]);
        assert_eq!(app.entered_group, Some(i), "Enter steps back in");
        assert_eq!(
            app.session.selection.single(),
            Some(ir),
            "its topmost layer selected"
        );
    }

    /// A `SetSwap` alone comes back from the session's commit with the nested copy's
    /// layer relinked to the swapped-to main's, and one undo takes both back.
    ///
    /// flip: dropping `swap::settle` from `owed` → fails at `relinked`
    #[test]
    fn a_committed_swap_is_rewritten_and_undone_whole() {
        let ctx = egui::Context::default();
        let SwapFixture {
            mut app,
            heart,
            hshape,
            n,
            r,
            shape,
            ..
        } = swap_fixture(&ctx);
        let tx = ondin_core::swap::swap(&app.session.doc, r, heart).expect("a swap");
        app.session.commit(tx);
        let relinked = app.session.doc.get(shape).unwrap().link();
        assert_eq!(relinked, Some(hshape), "the commit rewrote the copy");
        assert!(app.session.undo());
        assert_eq!(app.session.doc.get(r).unwrap().swap(), None);
        assert_eq!(
            app.session.doc.get(shape).unwrap().link(),
            app.session.doc.get(n).unwrap().children().first().copied(),
            "one undo took both back"
        );
    }

    /// The menu's *Reset <property>* reaches a swap property on the instance around
    /// the selected swapped copy, and resetting it clears the swap.
    ///
    /// flip: `overridden_property_and_root` asking only the selected layer's own instance → answers `None`
    #[test]
    fn the_menu_resets_a_swap_property_from_the_swapped_copy() {
        let ctx = egui::Context::default();
        let SwapFixture {
            mut app,
            heart,
            button,
            n,
            b1,
            r,
            ..
        } = swap_fixture(&ctx);
        let (tx, _) = ondin_core::variant::define(
            &app.session.doc,
            &mut app.session.ids,
            button,
            "Icon",
            ondin_core::variant::PropKind::Swap,
            vec![n],
        )
        .expect("a swap property");
        assert!(app.session.commit(tx));
        let p = ondin_core::variant::instance_properties(&app.session.doc, b1)[0]
            .value
            .clone();
        let ops = ondin_core::variant::set_property(
            &app.session.doc,
            &[b1],
            &p,
            &ondin_core::variant::PropValue::Swap(heart),
        );
        assert!(app.session.commit(Transaction(ops)));
        assert_eq!(app.session.doc.get(r).unwrap().swap(), Some(heart));
        app.session.selection.set_one(r);
        assert_eq!(
            app.overridden_property_of_selection().map(|p| p.name),
            Some("Icon".to_string())
        );
        // The copy is an instance root, so `layer_menu` takes its `Role::Instance` arm.
        // flip: the arm's `property_reset` push deleted → fails at "the instance arm offers Reset Icon"
        app.open_context_menu(
            &ctx,
            crate::menu::Target::Layer {
                id: r,
                door: crate::menu::Door::Canvas,
            },
            None,
        );
        let labels: Vec<String> = app
            .open_menu_rows()
            .iter()
            .flatten()
            .map(|row| row.label.to_string())
            .collect();
        assert!(
            labels.iter().any(|l| l == "Reset Icon"),
            "the instance arm offers Reset Icon: {labels:?}"
        );
        app.context_menu = None;
        app.reset_selected_property();
        assert_eq!(app.session.doc.get(r).unwrap().swap(), None);
    }

    /// Two icon mains, `Star` and `Heart`, each holding a `Shape`; a `Button` main
    /// holding a nested instance `n` of Star; and `b1`, an instance of Button, whose
    /// copy of `n` is `r` (the swap target) and of Star's `Shape` is `shape`.
    struct SwapFixture {
        app: OndinApp,
        heart: NodeId,
        hshape: NodeId,
        button: NodeId,
        n: NodeId,
        b1: NodeId,
        r: NodeId,
        shape: NodeId,
    }

    fn swap_fixture(ctx: &egui::Context) -> SwapFixture {
        use ondin_core::Placement;
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x7CA);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [star, sshape, heart, hshape, button] = [(); 5].map(|_| ids.mint());
        let frame = |w: f64| NodeKind::Artboard {
            size: Size::new(w, w),
        };
        let rect = || NodeKind::Rect {
            size: Size::new(4.0, 4.0),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        };
        let create = |id, parent, kind, name: &str| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind,
            transform: None,
            name: Some(name.into()),
        };
        doc.apply(&Transaction(vec![
            create(star, root, frame(24.0), "Star"),
            create(sshape, star, rect(), "Shape"),
            create(heart, root, frame(24.0), "Heart"),
            create(hshape, heart, rect(), "Shape"),
            create(button, root, frame(80.0), "Button"),
            Operation::SetComponent {
                id: star,
                component: true,
            },
            Operation::SetComponent {
                id: heart,
                component: true,
            },
        ]))
        .expect("two icons and a button frame");
        let place = |doc: &Document, ids: &mut IdSource, main, parent| {
            let (tx, made) = ondin_core::insert_subtrees(
                doc,
                ids,
                &[Placement {
                    nodes: doc.capture_subtree(main).unwrap(),
                    parent,
                    index: None,
                }],
                Default::default(),
            );
            (tx, made[0])
        };
        let (tx, n) = place(&doc, &mut ids, star, button);
        doc.apply(&tx).expect("a Star in the button");
        doc.apply(&Transaction(vec![Operation::SetComponent {
            id: button,
            component: true,
        }]))
        .expect("the button a main");
        let (tx, b1) = place(&doc, &mut ids, button, root);
        doc.apply(&tx).expect("an instance of the button");
        let r = doc.get(b1).unwrap().children()[0];
        let shape = doc.get(r).unwrap().children()[0];
        assert_eq!(
            doc.get(r).unwrap().link(),
            Some(n),
            "the fixture: b1's nested copy"
        );
        app.session.adopt_document(doc, None);
        SwapFixture {
            app,
            heart,
            hshape,
            button,
            n,
            b1,
            r,
            shape,
        }
    }

    /// *Detach instance* cuts the instance and every node it links.
    #[test]
    fn detaching_cuts_every_link() {
        let ctx = egui::Context::default();
        let (mut app, _, i) = main_and_instance(&ctx);
        app.session.selection.set_one(i);
        app.detach_instances();
        let doc = &app.session.doc;
        assert_eq!(doc.get(i).unwrap().link(), None);
        assert!(
            doc.get(i)
                .unwrap()
                .children()
                .iter()
                .all(|k| doc.get(*k).unwrap().link().is_none())
        );
    }

    /// Resizing the main's rect reaches the instance's rect through the commit
    /// (`propagate` in `commit_inner`).
    ///
    /// flip: the pass removed from `commit_inner` → leaves the instance's rect at 10
    #[test]
    fn a_main_edit_reaches_its_instance_through_the_commit() {
        let ctx = egui::Context::default();
        let (mut app, m, i) = main_and_instance(&ctx);
        let r = app.session.doc.get(m).unwrap().children()[0];
        let ir = app.session.doc.get(i).unwrap().children()[0];
        assert!(app.session.commit(Transaction(vec![Operation::SetGeometry {
            id: r,
            geometry: ondin_core::GeometryPatch::Size(Size::new(40.0, 40.0)),
        }])));
        match app.session.doc.get(ir).unwrap().kind() {
            NodeKind::Rect { size, .. } => assert_eq!(*size, Size::new(40.0, 40.0)),
            other => panic!("{other:?}"),
        }
    }

    /// A pinned child still follows after the instance is resized: pinned right,
    /// instance widened, then the main's rect moved. It must follow transform and
    /// insets exactly, since a ulp of drift would read as an override.
    #[test]
    fn a_pinned_child_still_follows_after_the_instance_is_resized() {
        let ctx = egui::Context::default();
        let (mut app, m, i) = main_and_instance(&ctx);
        let r = app.session.doc.get(m).unwrap().children()[0];
        let ir = app.session.doc.get(i).unwrap().children()[0];
        let pin = ondin_core::Insets {
            right: Some(ondin_core::LengthPct::Px(12.0)),
            top: Some(ondin_core::LengthPct::Px(8.0)),
            ..Default::default()
        };
        assert!(app.session.commit(Transaction(vec![Operation::SetInsets {
            id: r,
            insets: pin,
        }])));
        assert_eq!(
            app.session.doc.get(ir).unwrap().insets(),
            &pin,
            "the pin followed"
        );
        assert!(app.session.commit(Transaction(vec![Operation::SetGeometry {
            id: i,
            geometry: ondin_core::GeometryPatch::Size(Size::new(260.0, 100.0)),
        }])));
        let before = *app.session.doc.get(ir).unwrap().insets();
        assert_eq!(
            before, pin,
            "resizing the instance wrote nothing to its rect"
        );
        let moved = ondin_core::kurbo::Affine::translate((31.25, 17.5));
        assert!(
            app.session
                .commit(Transaction(vec![Operation::SetTransform {
                    id: r,
                    transform: moved,
                }]))
        );
        let (main_r, inst_r) = (
            app.session.doc.get(r).unwrap(),
            app.session.doc.get(ir).unwrap(),
        );
        assert_ne!(
            main_r.insets(),
            &pin,
            "the fixture: the move re-pinned the main's rect"
        );
        assert_eq!(
            inst_r.insets(),
            main_r.insets(),
            "the insets followed exactly"
        );
        assert_eq!(inst_r.transform(), main_r.transform(), "and the transform");
    }

    /// *Create component* on a layer inside a main is refused — the frame it would
    /// wrap the layer in would be a main inside a main (`LinkRule::NestedMain`).
    #[test]
    fn a_layer_inside_a_main_is_not_made_a_component() {
        let ctx = egui::Context::default();
        let (mut app, m, _) = main_and_instance(&ctx);
        let rect = app.session.doc.get(m).unwrap().children()[0];
        app.session.selection.set_one(rect);
        app.create_component();
        assert!(
            !app.session.doc.get(rect).unwrap().component(),
            "a rect inside a main is refused, not made a main"
        );
    }

    /// Every selection the commit would refuse is refused by *Create component*
    /// itself, saying which rule, and an in-place create is silent. Each case also
    /// asserts that `create_component_refusal` (which dims the menu row) agrees.
    ///
    /// flip: `why_not_wrap`'s parent test removed → fails at "a rect inside a main"
    /// flip: `why_not_main`'s subtree test removed → fails at "a frame holding a main"
    /// flip: the `Created a component` info restored → fails at "in place, silent"
    #[test]
    fn create_component_refuses_with_the_reason_the_commit_would_give() {
        use ondin_core::component::NotMain;
        let ctx = egui::Context::default();
        let (mut app, m, i) = main_and_instance(&ctx);
        let rect = |w| NodeKind::Rect {
            size: Size::new(w, w),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        };
        let create = |id, parent, kind| {
            Transaction(vec![Operation::CreateNode {
                id,
                parent,
                index: 0,
                kind,
                transform: None,
                name: None,
            }])
        };
        let count = |app: &OndinApp| {
            let doc = &app.session.doc;
            ondin_core::subtree_nodes(doc, &[doc.first_page()]).len()
        };
        let refused = |app: &mut OndinApp, sel: Vec<NodeId>, why: NotMain, case: &str| {
            // Each case is its own action, in its own frame.
            app.session.next_status_pass();
            app.session.selection.set(sel);
            assert_eq!(app.create_component_refusal(), Some(why), "{case}");
            let before = count(app);
            app.create_component();
            assert_eq!(
                app.session.status().text,
                crate::menu::create_refusal(why),
                "{case}"
            );
            assert_eq!(count(app), before, "{case}: nothing wrapped");
        };

        let in_main = app.session.doc.get(m).unwrap().children()[0];
        refused(
            &mut app,
            vec![in_main],
            NotMain::Inside,
            "a rect inside a main",
        );

        let second = app.session.ids.mint();
        assert!(app.session.commit(create(second, m, rect(8.0))));
        let members = app.session.doc.get(i).unwrap().children().to_vec();
        assert_eq!(members.len(), 2, "the fixture: two members");
        refused(
            &mut app,
            members,
            NotMain::Inside,
            "two members of one instance",
        );

        let local = app.session.ids.mint();
        assert!(app.session.commit(create(local, i, rect(6.0))));
        assert_eq!(app.session.doc.get(local).unwrap().link(), None);
        refused(&mut app, vec![local], NotMain::Inside, "a local rect");

        refused(&mut app, vec![i], NotMain::Instance, "an instance root");
        assert_eq!(
            app.session.status().text,
            "An instance can't become a component — detach it first (Ctrl+Alt+B).",
            "§15 D1003 (12)'s sentence, word for word"
        );

        let root = app.session.doc.first_page();
        let page = app.session.ids.mint();
        let frame = NodeKind::Artboard {
            size: Size::new(500.0, 500.0),
        };
        assert!(app.session.commit(create(page, root, frame.clone())));
        assert!(app.session.commit(Transaction(vec![Operation::Reparent {
            id: m,
            new_parent: page,
            index: 0,
        }])));
        refused(
            &mut app,
            vec![page],
            NotMain::Holds,
            "a frame holding a main",
        );

        let plain = app.session.ids.mint();
        assert!(app.session.commit(create(plain, root, frame)));
        app.session.selection.set_one(plain);
        app.session.info("");
        app.create_component();
        assert!(
            app.session.doc.get(plain).unwrap().component(),
            "made a main"
        );
        assert_eq!(app.session.status().text, "", "in place, silent");
    }

    /// *Combine as variants* refuses a frame main and a group main, saying why (one
    /// kind per set); `combine_refusal` gives the same sentence. Two frame mains still combine.
    ///
    /// flip: the `one_kind` check deleted from `combine_as_variants` → fails at "said why" (`variant::combine`'s own refusal still holds the tree)
    #[test]
    fn combine_refuses_mains_of_two_kinds() {
        let ctx = egui::Context::default();
        let (mut app, m, _) = main_and_instance(&ctx);
        let root = app.session.doc.first_page();
        let (g, r, f) = (
            app.session.ids.mint(),
            app.session.ids.mint(),
            app.session.ids.mint(),
        );
        let create = |id, parent, kind| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind,
            transform: None,
            name: None,
        };
        assert!(app.session.commit(Transaction(vec![
            create(g, root, NodeKind::Group),
            create(
                r,
                g,
                NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
            ),
            create(
                f,
                root,
                NodeKind::Artboard {
                    size: Size::new(50.0, 50.0),
                },
            ),
            Operation::SetComponent {
                id: g,
                component: true,
            },
            Operation::SetComponent {
                id: f,
                component: true,
            },
        ])));
        app.session.selection.set(vec![m, g]);
        assert!(app.combinable_mains().is_some(), "the fixture: offered");
        assert_eq!(app.combine_refusal(), Some(crate::menu::COMBINE_MIXED));
        app.combine_as_variants();
        assert_eq!(
            app.session.status().text,
            crate::menu::COMBINE_MIXED,
            "said why"
        );
        assert_eq!(
            ondin_core::variant::set_of(&app.session.doc, m),
            None,
            "no set made"
        );

        app.session.selection.set(vec![m, f]);
        assert_eq!(app.combine_refusal(), None);
        app.combine_as_variants();
        assert!(
            ondin_core::variant::set_of(&app.session.doc, m).is_some(),
            "two frame mains combine"
        );
    }

    /// *Reset all* after a resize, rename and pinned-rect move writes size, name,
    /// transform and insets back in one transaction with no drift, and one `Ctrl+Z`
    /// undoes it. That holds because `keep_insets` leaves alone a layer whose insets
    /// the transaction sets itself, not because of op order.
    #[test]
    fn reset_all_through_the_commit_leaves_no_drift_and_undoes_in_one_step() {
        use ondin_core::reset;
        let ctx = egui::Context::default();
        let (mut app, m, i) = main_and_instance(&ctx);
        let r = app.session.doc.get(m).unwrap().children()[0];
        let ir = app.session.doc.get(i).unwrap().children()[0];
        let pin = ondin_core::Insets {
            right: Some(ondin_core::LengthPct::Px(12.0)),
            top: Some(ondin_core::LengthPct::Px(8.0)),
            ..Default::default()
        };
        assert!(app.session.commit(Transaction(vec![Operation::SetInsets {
            id: r,
            insets: pin,
        }])));
        let placed = ondin_core::kurbo::Affine::translate((400.0, 0.0));
        for op in [
            Operation::SetTransform {
                id: i,
                transform: placed,
            },
            Operation::SetGeometry {
                id: i,
                geometry: ondin_core::GeometryPatch::Size(Size::new(260.0, 90.0)),
            },
            Operation::SetName {
                id: i,
                name: "Mine".into(),
            },
            Operation::SetTransform {
                id: ir,
                transform: ondin_core::kurbo::Affine::translate((5.0, 6.0)),
            },
        ] {
            assert!(app.session.commit(Transaction(vec![op])));
        }
        assert_ne!(
            app.session.doc.get(ir).unwrap().insets(),
            &pin,
            "the fixture: the move re-pinned the instance's rect"
        );
        let drifted = reset::drift(&app.session.doc, i);
        assert!(drifted.fields >= 3, "{drifted:?}");

        app.session.selection.set_one(i);
        app.reset_selection(reset::Kind::All);
        let doc = &app.session.doc;
        assert_eq!(reset::drift(doc, i), reset::Drift::default());
        assert_eq!(doc.get(ir).unwrap().insets(), doc.get(r).unwrap().insets());
        assert_eq!(
            doc.get(ir).unwrap().transform(),
            doc.get(r).unwrap().transform()
        );
        assert_eq!(
            doc.get(i).unwrap().transform(),
            placed,
            "its own place stays"
        );

        assert!(app.session.undo());
        assert_eq!(reset::drift(&app.session.doc, i), drifted, "one step back");
    }
}

#[cfg(test)]
mod view_prefs_tests {
    use super::*;

    /// A switch set this session, by its menu or by a side door, is in the prefs the
    /// next app is built from, and that app opens with it; focus mode is not kept.
    /// flip: `remember_view_switches` not called from `ui` → fails at the prefs.
    #[test]
    fn view_and_snap_switches_survive_a_relaunch() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.set_view_switch(ViewSwitch::SnapGrid, false);
        app.set_view_switch(ViewSwitch::Rulers, false);
        app.set_view_switch(ViewSwitch::Focus, true);
        // A side door: pulling a guide out turns guides on without the switch.
        app.show_guides = false;
        let mut frame = eframe::Frame::_new_kittest();
        let _ = ctx.run_ui(Default::default(), |ui| {
            eframe::App::ui(&mut app, ui, &mut frame)
        });
        assert!(!app.prefs.view.snap_grid && !app.prefs.view.rulers && !app.prefs.view.guides);

        let next = OndinApp::with(
            &ctx,
            CanvasRenderer::headless(),
            FontService::inert(),
            app.prefs.clone(),
        );
        assert!(!next.snap_grid && !next.show_rulers && !next.show_guides);
        assert!(next.snap_shapes && next.show_layers, "the rest stay on");
        assert!(!next.focus, "a launch never opens in focus mode");
    }
}

#[cfg(test)]
mod clone_chain_tests {
    use super::OndinApp;
    use ondin_core::kurbo::{Affine, Point, Size};
    use ondin_core::{Document, IdSource, NodeKind, Operation, Transaction};

    /// `Ctrl+D` over two layers, the copies moved, then `Ctrl+D` again: the third pair
    /// repeats the move, as a lone layer's run does.
    /// flip: `clone_step` answering zero for more than one id → fails at the third pair.
    #[test]
    fn a_run_of_duplicates_repeats_its_step_for_several_layers() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0xD0D);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [a, b] = [(); 2].map(|_| ids.mint());
        for (i, id) in [a, b].into_iter().enumerate() {
            doc.apply(&Transaction(vec![Operation::CreateNode {
                id,
                parent: root,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((40.0 * i as f64, 0.0))),
                name: None,
            }]))
            .expect("a rect");
        }
        app.session.adopt_document(doc, None);
        let origins = |app: &OndinApp| {
            let mut at: Vec<Point> = app
                .session
                .selection
                .ids()
                .iter()
                .map(|&id| app.origin_of(id).expect("placed"))
                .collect();
            at.sort_by(|p, q| p.x.total_cmp(&q.x));
            at
        };
        app.session.selection.set(vec![a, b]);
        app.duplicate_selection();
        assert_eq!(
            origins(&app),
            vec![Point::new(0.0, 0.0), Point::new(40.0, 0.0)],
            "the first copies land in place"
        );
        let moved: Vec<Operation> = app
            .session
            .selection
            .ids()
            .iter()
            .map(|&id| Operation::SetTransform {
                id,
                transform: Affine::translate((5.0, 15.0))
                    * app.session.doc.get(id).unwrap().transform(),
            })
            .collect();
        assert!(app.session.commit(Transaction(moved)));
        app.duplicate_selection();
        assert_eq!(
            origins(&app),
            vec![Point::new(10.0, 30.0), Point::new(50.0, 30.0)],
            "the third pair repeats the move"
        );
    }
}

#[cfg(test)]
mod select_same_tests {
    use super::OndinApp;
    use crate::input::{Action, Same};
    use ondin_core::kurbo::Size;
    use ondin_core::peniko::Color;
    use ondin_core::{Fill, IdSource, NodeId, NodeKind, Operation, Transaction};

    struct F {
        app: OndinApp,
        ctx: eframe::egui::Context,
        frame: NodeId,
        /// Red, red, blue inside the frame; red loose at the root; red and locked.
        r: [NodeId; 5],
    }

    fn fixture() -> F {
        let ctx = eframe::egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0x5A3E);
        let root = app.session.doc.first_page();
        let frame = ids.mint();
        let r = [(); 5].map(|_| ids.mint());
        let mut ops = vec![Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
            },
            transform: None,
            name: None,
        }];
        let red = Color::from_rgba8(255, 0, 0, 255);
        let blue = Color::from_rgba8(0, 0, 255, 255);
        for (i, id) in r.iter().enumerate() {
            let parent = if i == 3 { root } else { frame };
            ops.push(Operation::CreateNode {
                id: *id,
                parent,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            });
            let colour = if i == 2 { blue } else { red };
            ops.push(Operation::SetFills {
                id: *id,
                fills: ondin_core::keyed_by_position([Fill {
                    brush: ondin_core::Brush::Solid(colour).into(),
                    visible: true,
                }]),
            });
        }
        ops.push(Operation::SetLocked {
            id: r[4],
            locked: true,
        });
        assert!(app.session.commit(Transaction(ops)));
        F { app, ctx, frame, r }
    }

    fn selected(app: &OndinApp) -> Vec<NodeId> {
        let mut ids = app.session.selection.ids().to_vec();
        ids.sort();
        ids
    }

    /// *Select same fill* finds every layer with the key's fill list, inside frames and
    /// out, and leaves a locked match out.
    /// flip: `select_same` dropping its `may_select_in_bulk` filter → fails at the
    /// selection holding the locked rect.
    #[test]
    fn select_same_fill_reaches_across_the_document_and_skips_the_locked() {
        let F {
            mut app, ctx, r, ..
        } = fixture();
        app.session.selection.set_one(r[0]);
        app.dispatch(&ctx, Action::SelectSame(Same::Fill));
        let mut want = vec![r[0], r[1], r[3]];
        want.sort();
        assert_eq!(selected(&app), want);
    }

    /// The palette dims *Select same stroke* on a layer with no stroke, with the reason.
    #[test]
    fn select_same_names_what_the_source_lacks() {
        let F { mut app, r, .. } = fixture();
        app.session.selection.set_one(r[0]);
        assert_eq!(
            app.select_same_source(Same::Stroke),
            Err("This layer has no stroke")
        );
        assert_eq!(app.select_same_source(Same::Fill), Ok(r[0]));
    }

    /// *Select inverse* takes the selection's unselected siblings, not the whole
    /// document, and skips a locked one.
    /// flip: `select_inverse` dropping its `may_select_in_bulk` filter → fails at the
    /// selection holding the locked rect.
    #[test]
    fn select_inverse_takes_the_siblings_left_out() {
        let F {
            mut app,
            ctx,
            frame,
            r,
        } = fixture();
        app.session.selection.set_one(r[0]);
        app.dispatch(&ctx, Action::SelectInverse);
        let mut want = vec![r[1], r[2]];
        want.sort();
        assert_eq!(selected(&app), want);
        assert!(!selected(&app).contains(&frame));
    }
}

#[cfg(test)]
mod paste_target_tests {
    use super::{OndinApp, PASTE_OFFSET};
    use ondin_core::kurbo::{Affine, Point, Size};
    use ondin_core::{IdSource, NodeId, NodeKind, Operation, Transaction};

    struct F {
        app: OndinApp,
        ctx: eframe::egui::Context,
        /// Frames at (0, 0) and (500, 0), 200 square.
        a: NodeId,
        b: NodeId,
        /// 20 square at (10, 10) in `a`.
        r: NodeId,
        /// 40 square at (1000, 1000) on the canvas.
        q: NodeId,
    }

    fn fixture() -> F {
        let ctx = eframe::egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0x9A57);
        let root = app.session.doc.first_page();
        let [a, b, r, q] = [(); 4].map(|_| ids.mint());
        let at = |id, parent, kind, x: f64, y: f64| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind,
            transform: Some(Affine::translate((x, y))),
            name: None,
        };
        let square = |s: f64| NodeKind::Rect {
            size: Size::new(s, s),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        };
        let frame = || NodeKind::Artboard {
            size: Size::new(200.0, 200.0),
        };
        assert!(app.session.commit(Transaction(vec![
            at(a, root, frame(), 0.0, 0.0),
            at(b, root, frame(), 500.0, 0.0),
            at(r, a, square(20.0), 10.0, 10.0),
            at(q, root, square(40.0), 1000.0, 1000.0),
        ])));
        F {
            app,
            ctx,
            a,
            b,
            r,
            q,
        }
    }

    fn centre(app: &OndinApp, id: NodeId) -> Point {
        app.session.resolved.world_bounds(id).unwrap().center()
    }

    /// A paste with one other frame selected lands inside it, centred on it when its own
    /// place is outside the frame.
    /// flip: `paste_into_selected_frame` keeping `offset` whatever the overlap → fails at
    /// "centred on the frame".
    #[test]
    fn a_paste_onto_a_selected_frame_lands_inside_it() {
        let F {
            mut app, ctx, b, r, ..
        } = fixture();
        app.session.selection.set_one(r);
        app.copy_selection(&ctx);
        app.session.selection.set_one(b);
        assert!(app.paste_into_selected_frame(PASTE_OFFSET, true));
        let pasted = app
            .session
            .selection
            .single()
            .expect("the paste is selected");
        assert_ne!(pasted, r);
        assert_eq!(app.session.doc.get(pasted).unwrap().parent(), Some(b));
        let c = centre(&app, pasted);
        assert!(
            (c - Point::new(600.0, 100.0)).hypot() < 1e-6,
            "centred on the frame: {c:?}"
        );
    }

    /// The frame a layer was copied from, and a copied frame itself, take the ordinary
    /// paste: above the original, and a duplicate beside, not a nesting.
    #[test]
    fn a_frame_holding_the_copy_takes_the_ordinary_paste() {
        let F {
            mut app,
            ctx,
            a,
            b,
            r,
            ..
        } = fixture();
        app.session.selection.set_one(r);
        app.copy_selection(&ctx);
        app.session.selection.set_one(a);
        assert!(
            !app.paste_into_selected_frame(PASTE_OFFSET, true),
            "its own frame"
        );
        app.session.selection.set_one(b);
        app.copy_selection(&ctx);
        assert!(
            !app.paste_into_selected_frame(PASTE_OFFSET, true),
            "the frame itself"
        );
    }

    /// *Paste to replace* puts the copy where the target was — centred on it, in its
    /// slot, even with the original moved since the copy — and takes the target away,
    /// in one undo step.
    /// flip: `clip_box` answering the copy-time box → fails at "centred on the target".
    #[test]
    fn paste_to_replace_takes_the_targets_place() {
        let F {
            mut app, ctx, r, q, ..
        } = fixture();
        let root = app.session.doc.first_page();
        let slot = app
            .session
            .doc
            .get(root)
            .unwrap()
            .children()
            .iter()
            .position(|c| *c == q);
        app.session.selection.set_one(r);
        app.copy_selection(&ctx);
        // Moved after the copy: the aim must follow where the original is now.
        assert!(
            app.session
                .commit(Transaction(vec![Operation::SetTransform {
                    id: r,
                    transform: Affine::translate((60.0, 80.0)),
                }]))
        );
        app.session.selection.set_one(q);
        app.replace_with_clipboard();
        assert!(app.session.doc.get(q).is_none(), "the target is gone");
        let copy = app
            .session
            .selection
            .single()
            .expect("the copy is selected");
        assert_eq!(app.session.doc.get(copy).unwrap().parent(), Some(root));
        assert_eq!(
            app.session
                .doc
                .get(root)
                .unwrap()
                .children()
                .iter()
                .position(|c| *c == copy),
            slot,
            "in its slot"
        );
        let c = centre(&app, copy);
        assert!(
            (c - Point::new(1020.0, 1020.0)).hypot() < 1e-6,
            "centred on the target: {c:?}"
        );
        app.undo();
        assert!(app.session.doc.get(q).is_some() && app.session.doc.get(copy).is_none());
    }
}

#[cfg(test)]
mod zoom_entry_tests {
    use super::{OndinApp, parse_zoom_percent};

    /// The zoom field reads a bare number or a percentage and refuses the rest.
    #[test]
    fn a_typed_zoom_reads_as_a_percentage() {
        assert_eq!(parse_zoom_percent("150"), Some(150.0));
        assert_eq!(parse_zoom_percent(" 37.5 % "), Some(37.5));
        assert_eq!(parse_zoom_percent("0"), None);
        assert_eq!(parse_zoom_percent("-20%"), None);
        assert_eq!(parse_zoom_percent("big"), None);
    }

    /// Applying a typed zoom lands the camera on it, clamped at the ceiling.
    #[test]
    fn a_typed_zoom_lands_the_camera_there() {
        let ctx = eframe::egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.apply_typed_zoom("250%");
        assert!((app.session.camera.zoom - 2.5).abs() < 1e-9);
        app.apply_typed_zoom("1000000");
        assert_eq!(app.session.camera.zoom, crate::view::MAX_ZOOM);
        app.apply_typed_zoom("nonsense");
        assert_eq!(app.session.camera.zoom, crate::view::MAX_ZOOM, "unchanged");
    }
}
