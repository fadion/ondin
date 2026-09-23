//! The library screen — what the app opens on (§9.5, §15 D365).
//!
//! **A whole window, not a panel.** `OndinApp::ui` returns after calling
//! [`OndinApp::dashboard_ui`], so while this is up there is no canvas, no tool,
//! no document selection and no keyboard routing into the document. That boundary
//! is why nothing here has to ask whether the editor is listening.
//!
//! ⚠️ **"No selection" and "no keymap" both used to be true of this file and
//! neither is now.** A card can be picked out (§15 D372) and the arrows, `Enter`,
//! `Delete` and `Escape` act on it (§15 D374) — but through
//! [`OndinApp::dashboard_keys`], reading the context directly, *not* through
//! `input::resolve`, which is the function the sentence above is about and which
//! genuinely never runs here. The two keymaps share no code and cannot collide.
//!
//! **Every action goes to disk immediately and the library is re-scanned.**
//! There is no dirty state and nothing to save: renaming writes the file,
//! deleting moves it, creating a project writes `projects.json`. What that buys
//! is that the screen can never disagree with the folder — which matters more
//! here than anywhere else in the app, because the folder is explicitly meant to
//! be one a sync client is also writing into.
//!
//! ⚠️ **So a write is followed by a refresh, and a refresh reads every
//! document's prefix.** That is fine at the rate a person clicks and would not
//! be at the rate a frame draws; see `library::state::Library::refresh`. Nothing
//! in this file may call it from inside a layout loop.

use crate::app::OndinApp;
use crate::library::scan::Entry;
use crate::library::state::Sort;
use crate::library::{clock, ids, naming, project, store};
// **The editor's Settings card is this screen's kit, not a neighbour to copy.**
// Its section headers, its captions, its footer buttons and the card itself are
// the app's answer to "a form in a modal", and there is exactly one of those
// (§15 D368) — a second spelling of the same card is what put the dashboard's
// Settings visibly out of step with the editor's.
use crate::settings;
use crate::theme;
use crate::theme::{color, icon};
use crate::ui::{self, FieldButton, icon_button};
use eframe::egui;
use std::path::{Path, PathBuf};

/// The sidebar's width, from the design.
const SIDEBAR_W: f32 = 266.0;
/// The top bar's height — the same as the editor's, so the two screens do not
/// shift their chrome when you move between them.
const TOP_H: f32 = 46.0;
/// A file row in the list view.
const ROW_H: f32 = 38.0;
/// A sidebar entry.
const NAV_H: f32 = 30.0;
/// Where the unarchive glyph's centre sits, measured in from a sidebar row's
/// right edge.
///
/// **The count's own margin plus half a glyph**, so the two groups' trailing
/// marks share an optical right edge rather than each ending where its own shape
/// happens to: the count is drawn `RIGHT_CENTER` at 12 in, and a 13pt glyph
/// centred here has its own right edge in the same place.
const UNARCHIVE_X: f32 = 12.0 + 13.0 / 2.0;
/// A card in the grid view: the thumbnail's height, plus its caption.
const CARD_THUMB_H: f32 = 116.0;
const CARD_CAPTION_H: f32 = 42.0;
/// The design's mosaic: 118 tall, two rows, a 7pt gutter, and at most five of the
/// project's files in it.
const MOSAIC_H: f32 = 118.0;
const MOSAIC_GAP: f32 = 7.0;
/// **Five, which is the largest layout the design draws** and also the largest that
/// the three-column rule in [`OndinApp::project_mosaic`] can fill without a column
/// growing a third row — at which point the cells are 36pt tall and stop being
/// pictures of anything.
const MOSAIC_MAX: usize = 5;
/// How many stranded filenames the Settings modal lists before saying "and N
/// more" (§15 D810).
///
/// **Six, which is a bound on the modal's height rather than a reading of the
/// list.** That card has no scroll area, and a partly-failed migration can strand
/// every document in the library — so an uncapped list grows the modal past the
/// window and takes *Save changes* off screen with it. Six is enough to recognise
/// what went wrong (all in one project, all version files, one file that is open
/// elsewhere) and the folder is named above them, which is what the user actually
/// opens.
const STRANDED_ROWS: usize = 6;
/// What a project card keeps clear of its own edges (the design's `padding:10px`).
const PROJECT_PAD: f32 = 10.0;
/// Between the mosaic, the name and the two facts under it (the design's `gap:9px`).
const PROJECT_ROW_GAP: f32 = 9.0;
/// A project card on *Recent* — **taller than a file card**, because it carries a
/// picture of up to five documents where a file card carries one.
const PROJECT_CARD_H: f32 = PROJECT_PAD * 2.0 + MOSAIC_H + PROJECT_ROW_GAP * 2.0 + 16.0 + 13.0;
/// How many cards a row of the grid holds. **Fixed rather than fitted**, which
/// is the design's `repeat(4,1fr)`: a grid that reflows to the window makes the
/// same library look different on two monitors, and the cards are thumbnails
/// rather than content that needs a minimum width.
const GRID_COLS: usize = 4;
/// The search field, from the design.
const SEARCH_W: f32 = 340.0;
/// A row in the search dropdown.
const SEARCH_ROW_H: f32 = 30.0;
/// Where the search field's text starts, measured from the field's left edge —
/// past the 10pt inset and the magnifier — and the **same number in both states**.
///
/// ⚠️ **One constant because it is one position.** The field swaps a painted
/// placeholder for a live `TextEdit` when it opens, and the two used to be spelled
/// separately (30 against 28-plus-egui's-own-4), so the words moved sideways at the
/// moment of the click. Anything that reads a hand-painted string against a widget
/// showing the same string owes a shared origin.
const SEARCH_TEXT_X: f32 = 30.0;
/// Type size of the search field's text and its placeholder — again in both
/// states, for [`SEARCH_TEXT_X`]'s reason.
const SEARCH_PT: f32 = 11.5;
/// The row a [`field_label`] is allocated: its galley and nothing else.
const FIELD_LABEL_H: f32 = 12.0;
/// What the project filter says when nothing is filtered — and the first row of
/// its own menu, which is how it is unset.
///
/// One constant because the two must be the same words: a menu whose "off" row is
/// worded differently from the resting button reads as a third state.
const ALL_PROJECTS: &str = "All projects";
/// The project filter's closed button. Wide enough for a project name of ordinary
/// length beside its dot and caret; longer ones elide, as everywhere else on this
/// screen.
const FILTER_W: f32 = 132.0;
/// The filter's dropdown (the design's `width:190px`), wider than the button
/// because a menu is read once and a header control is read every visit.
const FILTER_MENU_W: f32 = 190.0;
/// A row of either floating menu on this screen (the design's 26, plus two so the
/// filter's dropdown is not tighter than the ⋮ menu beside it).
///
/// ⚠️ **`file_menu_popup` wrote this as a bare `28.0` three times** (§15 D729,
/// `[S20.2-L3-05]`) — the same number, uncommonised, which is how the two menus'
/// rows would have come apart on the first change to either. The sentence above
/// is the reason they must not: they are on one screen and are read against each
/// other.
const FILTER_ROW_H: f32 = 28.0;
/// A project's colour chip, in the filter and its menu — the design's 7×7.
const DOT: f32 = 7.0;

/// What one frame of an [`anchored_menu`] came to.
struct MenuOutcome {
    /// The row clicked, if one was.
    chosen: Option<usize>,
    /// Whether a click landed outside both the menu and the control it hangs
    /// from — i.e. whether the caller should close it.
    dismissed: bool,
}

/// A floating menu hung under `anchor`: the panel, its rows, their hover ground
/// and the click-away test (§15 D729, `[S20.2-L3-05]`).
///
/// **The library's two menus were this control written twice**, thirty-five lines
/// apiece, identical down to the `+ 4.0` on the anchor and the `+ 10.0` on the
/// height — `project_filter_menu`'s own doc named the relationship (*"the shape
/// `file_menu_popup` already uses on this screen"*) and copied it rather than
/// sharing it. `panels/mod.rs` states the rule — *"A hand-rolled copy is how the
/// next popover gets three of the six"* — about `ClickAway`'s gating terms over
/// the **inspector's** popovers, and these two hand-rolled a two-term dismissal
/// of their own on a different screen. So this was not a violated invariant; it
/// was **the prediction coming true one screen over**.
///
/// ⚠️ **No ordinal here, deliberately** (§15 D729). A first version of this
/// comment quoted a superseded spelling of that rule and called these menus *"the
/// fifth and sixth copies"* — reintroducing into a second file the count
/// `panels/mod.rs` had just removed from six, in the paragraph that exists to be
/// its only home. **A quotation is a copy.**
///
/// ⚠️ **What is shared is the shell, and only the shell.** `paint` draws a row's
/// contents into the rect it is given, because that is where the two genuinely
/// differ: the filter draws a colour dot and a tick, the ⋮ menu draws a label that
/// turns red on *Delete*. Folding those together would be the flag-and-branch
/// function this file already refuses elsewhere.
///
/// 🚨 **This is deliberately *not* where `[S20.2-L1-01]`'s Escape lives.** The
/// finding proposed extracting first so both that fix and `[S20.2-L1-02]`'s
/// click-swallow would land here; both were closed before this extraction, in
/// `dashboard_keys` behind `library_menu_open()` — one predicate answering for
/// **both** menus and the eight pointer-side doors already asking it (§15 D558).
/// That is the better place, because a keyboard rule that lives inside a
/// *drawing* helper only runs on the frames the menu is drawn. **A shared
/// implementation and a shared predicate are different kinds of sharing, and this
/// screen wanted both.**
fn anchored_menu(
    ui: &mut egui::Ui,
    id: egui::Id,
    anchor: egui::Rect,
    w: f32,
    rows: usize,
    paint: impl Fn(&egui::Painter, usize, egui::Rect),
) -> MenuOutcome {
    let h = rows as f32 * FILTER_ROW_H + 10.0;
    let area = egui::Area::new(id)
        .order(egui::Order::Foreground)
        // Right-aligned to the control, as the design has it: a menu is wider
        // than its trigger and the header's right margin is the fixed edge.
        .fixed_pos(egui::pos2(anchor.right() - w, anchor.bottom() + 4.0));
    let inner = area.show(ui.ctx(), |ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::empty());
        let p = ui.painter();
        p.rect_filled(rect, egui::CornerRadius::same(8), color::FIELD);
        p.rect_stroke(
            rect,
            egui::CornerRadius::same(8),
            egui::Stroke::new(1.0, color::FIELD_BORDER),
            egui::StrokeKind::Inside,
        );
        let mut chosen = None;
        for i in 0..rows {
            let row = egui::Rect::from_min_size(
                egui::pos2(
                    rect.left() + 4.0,
                    rect.top() + 5.0 + i as f32 * FILTER_ROW_H,
                ),
                egui::vec2(w - 8.0, FILTER_ROW_H),
            );
            let r = ui.interact(row, ui.id().with(i), egui::Sense::click());
            if r.hovered() {
                ui.painter().rect_filled(
                    row,
                    egui::CornerRadius::same(6),
                    theme::color::text_a(14),
                );
            }
            paint(ui.painter(), i, row);
            if r.clicked() {
                chosen = Some(i);
            }
        }
        (rect, chosen)
    });
    let (rect, chosen) = inner.inner;
    let at = |r: egui::Rect| ui.ctx().pointer_interact_pos().unwrap_or(r.center());
    MenuOutcome {
        chosen,
        // Anchor as well as menu: a click on the control that opened this is the
        // control's own toggle, and treating it as a dismissal would close and
        // reopen in one frame.
        dismissed: chosen.is_none()
            && ui.ctx().input(|i| i.pointer.any_click())
            && !rect.contains(at(rect))
            && !anchor.contains(at(anchor)),
    }
}
/// What the filter button's content keeps clear of its own corners.
const FILTER_PAD_X: f32 = 10.0;
/// A Settings dropdown — the design's `min-width:132px` on its own popup, given to
/// the closed control so the three rows line up as a column rather than each
/// ending wherever its own word does.
const PICKER_W: f32 = 132.0;
/// Between a [`field_label`] and the box it names (the design's `gap:4px`).
///
/// Closer than `settings::ROW_GAP`, for `settings::caption`'s reason in the other
/// direction: a label belongs *to* the field under it rather than being the
/// previous thing down the column.
const FIELD_LABEL_GAP: f32 = 4.0;
/// How many projects and how many documents a search offers.
///
/// **Six of each rather than one list of twelve**, so a query matching thirty
/// files still shows the project it probably meant. A search that scrolls is a
/// search that has stopped being faster than the sidebar.
const SEARCH_MAX: usize = 6;
/// How many past queries the empty search offers.
const SEARCH_RECENTS: usize = 5;
/// Every control on the body header's right-hand cluster — the sort button, the
/// view track and *Delete project*.
///
/// **28, the app's control height, which is also `settings::FIELD_H`.** The design
/// asks for 28 here and 30 for a modal's buttons; the app answers 28 to both, for
/// the reason `settings::BUTTON_H` states at length — they are the same class of
/// control one card apart.
const HEADER_CONTROL_H: f32 = 28.0;
/// A cell of the grid/list track. [`HEADER_CONTROL_H`] less the two points
/// [`ui::segmented`] pads its track by on each side, so the group measures the same
/// as the buttons beside it rather than four points taller.
const VIEW_CELL_H: f32 = HEADER_CONTROL_H - 4.0;
/// The grid/list track: two 26pt cells, the 2pt gap between them, and the track's
/// own 2pt padding at each end — the design's numbers, added up here because
/// [`ui::segmented`] takes the *outer* width.
const VIEW_TRACK_W: f32 = 26.0 * 2.0 + 2.0 + 4.0;
/// *Delete project*, the design's 32 — wider than [`HEADER_CONTROL_H`] rather
/// than square, so the one control in the cluster carrying no word still reads as
/// a button rather than as a chip.
const DELETE_W: f32 = 32.0;

/// How wide the sort button has to be to hold the longest of the four orders.
///
/// ⚠️ **The widest of [`Sort::ALL`], not the one showing.** The control cycles its
/// own label on every press, so measuring the current word would make the button —
/// and everything left of it — twitch sideways between *Name* and *Date created*
/// each time it is clicked.
fn sort_button_w(ctx: &egui::Context) -> f32 {
    Sort::ALL
        .into_iter()
        .map(|s| ui::action_button_w(ctx, s.label()))
        .fold(0.0, f32::max)
}

/// Which sidebar entry is showing.
///
/// ⚠️ **`Project` carries an id rather than an index**, because the project list
/// is rebuilt from `projects.json` on every refresh — including refreshes caused
/// by something *else* writing that file — so an index selected before a
/// refresh can name a different project after it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) enum Nav {
    #[default]
    Recent,
    All,
    Starred,
    Trash,
    Project(String),
}

impl Nav {
    /// The preference key, with the same degrade-to-default rule as `Sort`.
    pub(crate) fn id(&self) -> &str {
        match self {
            Nav::Recent => "recent",
            Nav::All => "all",
            Nav::Starred => "starred",
            Nav::Trash => "trash",
            Nav::Project(id) => id,
        }
    }

    /// What the sidebar and the Settings picker call it.
    ///
    /// A method rather than a `match` at each site, because there are now three
    /// of them and a project's label has to come from the project list — which
    /// this cannot reach, so it names the *kind* and the sidebar overrides it.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Nav::Recent => "Recent",
            Nav::All => "All files",
            Nav::Starred => "Starred",
            Nav::Trash => "Trash",
            Nav::Project(_) => "Project",
        }
    }

    /// Read a preference back. **Only the four fixed entries**, deliberately: a
    /// remembered project may have been deleted between launches, and landing on
    /// *Recent* is better than landing on a heading with nothing under it.
    pub(crate) fn from_id(id: &str) -> Nav {
        match id {
            "all" => Nav::All,
            "starred" => Nav::Starred,
            "trash" => Nav::Trash,
            _ => Nav::Recent,
        }
    }
}

/// Everything the dashboard needs between frames.
///
/// **On `OndinApp` rather than in `egui`'s temp storage**, because half of it is
/// read by code outside the layout — the file menu's chosen action is acted on
/// after the frame, and `visible_entries` and the modals all read the nav.
///
/// 🚨 **None of it is a preference and none of it outlives the process**
/// (§15 D749). This sentence used to end *"and the nav is written back to
/// preferences on exit"*, which was false in both halves: `on_exit` writes none of
/// the three `dashboard_*` keys, and the writer that did exist ran from the sort
/// and view controls rather than from any navigation — so the page was persisted
/// only if you happened to touch one of the other two on your way out. The three
/// preferences are **defaults chosen in Settings**, answering *what should be open
/// when the app loads*; `nav`, `sort` and `list_view` here answer *what am I
/// looking at now*, and the two are deliberately not the same state.
#[derive(Default)]
pub(crate) struct DashboardState {
    pub(crate) nav: Nav,
    pub(crate) sort: Sort,
    pub(crate) list_view: bool,
    /// Where the **keyboard's cursor** is, by path — and nothing else puts it
    /// there.
    ///
    /// ⚠️ **A single click used to set this and no longer does** (§15 D381). It
    /// opens the document instead, so `OndinApp::dashboard_keys` is the only writer
    /// and the accent outline is a keyboard cursor rather than a selection. What
    /// that changes for a reader: an empty `selected` is the *ordinary* state now
    /// rather than "nothing picked yet", and every consumer already had to handle it
    /// because the clear below runs constantly.
    ///
    /// ⚠️ **A path, and it can name a file that is no longer there** — every
    /// action on this screen writes to disk and re-scans, so the list under a
    /// selection is rebuilt constantly and a delete, a move or a rename all leave
    /// this pointing at nothing. [`OndinApp::dashboard_body`] clears it rather
    /// than leaving a selection nobody can see or dismiss.
    ///
    /// ⚠️ **That clear stopped being cosmetic when the keymap landed** (§15 D374).
    /// While the drawing was the only reader, a stale path merely failed to light
    /// a card; `Enter` and `Delete` *act* on it, so a path naming a file that has
    /// gone is now the difference between opening nothing and opening whatever has
    /// since been written at that name. Which is why the clear runs before
    /// [`OndinApp::dashboard_keys`] and not after it.
    pub(crate) selected: Option<PathBuf>,
    /// Set when the keyboard moved [`DashboardState::selected`], and cleared by
    /// the card or row that scrolled itself into view.
    ///
    /// ⚠️ **A flag rather than a scroll on every picked card**, because the
    /// pointer picks things out too: a card that scrolled itself into view
    /// whenever it was selected would jerk the grid under a click that landed
    /// near the edge of the viewport. Only the keyboard can move the selection
    /// somewhere the user cannot see, so only the keyboard asks.
    pub(crate) scroll_to_selected: bool,
    /// The document whose ⋮ menu is open, by path.
    pub(crate) menu_for: Option<PathBuf>,
    /// Whether either floating menu was on screen when this frame began.
    ///
    /// ⚠️ **Latched at the top of `dashboard_ui`, because the live flags are
    /// cleared *mid-frame* by the very dismissal whose click has to be spent**
    /// (§15 D558, `[S20.2-L1-02]`). `file_menu_button` draws the popup — and runs
    /// its `any_click()` dismissal — from inside the card that owns it, so a menu
    /// on *Alpha* is already closed by the time *Bravo*'s card is drawn if Alpha
    /// sorts first. Reading `menu_for.is_some()` at `pick_or_open` therefore
    /// answered *"no menu"* for exactly half the cards on screen — the half after
    /// the anchor — and the fix worked or did not depending on sort order. The
    /// first version of D558's fix did this and its test caught it.
    ///
    /// **Frame-scoped rather than state**: written once per frame before anything
    /// is drawn, read by [`OndinApp::pick_or_open`], never persisted.
    pub(crate) menu_was_up: bool,
    /// The document whose *Move to project* submenu is open.
    pub(crate) moving: Option<PathBuf>,
    /// The document awaiting delete confirmation.
    pub(crate) deleting: Option<PathBuf>,
    /// The New project modal's fields, while it is open.
    pub(crate) new_project: Option<NewProject>,
    /// The Edit project modal's fields, while it is open.
    pub(crate) edit_project: Option<EditProject>,
    /// Whether the sidebar's *Archived projects* group is showing its rows.
    ///
    /// ⚠️ **Collapsed on every launch, and deliberately not a preference.** The
    /// group exists to get archived projects out of the sidebar; remembering that
    /// it was left open would put them back in it on the next launch, which is
    /// the state archiving was asked for to avoid. It is one click to open, and
    /// unlike the sort order and the view toggle beside it, the answer a user
    /// wants on opening the app is the same every time.
    pub(crate) archived_open: bool,
    /// The project filter in the header, by id. `None` is *All projects*.
    pub(crate) filter: Option<String>,
    /// Whether the filter's dropdown is showing.
    pub(crate) filter_open: bool,
    /// The project awaiting delete confirmation, and what to do with its files.
    pub(crate) deleting_project: Option<DeleteProject>,
    /// The search query while the overlay is open; `None` when it is closed.
    ///
    /// **One field for both**, rather than a `bool` beside a `String`, because
    /// the two can never disagree that way: there is no state where the overlay
    /// is shut and still holding what was typed into it.
    pub(crate) search: Option<String>,
    /// Which row of the search dropdown the arrows have moved to, if any.
    ///
    /// ⚠️ **`None` is not row zero — it is "the user has not steered yet"**, and the
    /// two answer differently: unsteered, the highlight sits on the first *document*
    /// match, which is where `Enter` has always gone and is a row the recents and
    /// project groups above it can push down as the query changes. Storing 0 for
    /// that state would freeze the highlight on whatever happened to be first.
    /// Cleared when the query changes and when the overlay opens.
    pub(crate) search_row: Option<usize>,
}

/// A migration that did not move everything, kept so the modal can report it and
/// offer the run again (§15 D810, `OndinApp::stranded`).
///
/// **Not a `Moved`.** That type is the whole of what a migration did, including
/// the path map a caller re-points an open document through; what survives the
/// operation is the part nobody can reconstruct afterwards — which files are still
/// in the old folder, and where the old folder is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stranded {
    /// The folder the files are still in.
    pub(crate) from: PathBuf,
    /// The folder they were going to, which is where the library now points.
    pub(crate) to: PathBuf,
    /// Every source path that did not move, in `relocate`'s own order.
    pub(crate) failed: Vec<PathBuf>,
}

/// The Settings modal's draft — see [`OndinApp::library_settings_modal`] for why
/// this is a draft rather than live preferences.
///
/// `Clone` and `PartialEq` are what make *Save changes* honest: the card is edited
/// on a copy and the commit lights when that copy differs from
/// [`LibrarySettings::from_prefs`], which is the editor's Settings card's rule
/// (`settings::Settings`) and one `!=` rather than nine per-field comparisons
/// somebody has to remember to extend.
///
/// ⚠️ **A derived `PartialEq` silences `dead_code` on a field nothing reads** —
/// measured, and true anywhere in the workspace, `pub` or not. So this deriving it
/// means the compiler will no longer say if a row is dropped from the card and its
/// field left behind: read the struct against the modal after removing a row.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct LibrarySettings {
    /// As typed. Empty means "the default", not "nothing".
    pub(crate) base_folder: String,
    /// Whether Save should move the existing library to the new folder.
    ///
    /// **Defaults off.** Moving somebody's whole library is the largest thing
    /// this app does to a filesystem, and it must be a thing they turned on.
    pub(crate) migrate: bool,
    pub(crate) autosave_secs: u64,
    pub(crate) trash_keep_days: u64,
    pub(crate) version_history: bool,
    pub(crate) default_page: Nav,
    pub(crate) default_list_view: bool,
    pub(crate) default_sort: Sort,
    pub(crate) reopen_last: bool,
}

impl LibrarySettings {
    /// Seed the draft from what is in force.
    ///
    /// ⚠️ **`base_folder` is seeded from the *resolved* root, not from the raw
    /// preference.** They differ for every user who has never set one: the
    /// preference is `None` and the root is `~/.ondin`. Showing an empty field
    /// there would be honest about the setting and useless about the library —
    /// and worse, editing it would then read as a change when it was a
    /// restatement.
    pub(crate) fn from_prefs(prefs: &crate::prefs::Prefs, root: &Path) -> Self {
        Self {
            base_folder: root.display().to_string(),
            migrate: false,
            autosave_secs: prefs.autosave_secs,
            trash_keep_days: prefs.trash_keep_days,
            version_history: prefs.version_history,
            default_page: Nav::from_id(&prefs.dashboard_page),
            default_list_view: prefs.dashboard_list_view,
            default_sort: Sort::from_id(&prefs.dashboard_sort),
            reopen_last: prefs.reopen_last,
        }
    }
}

/// The Delete project modal's own state.
///
/// ⚠️ **`keep_files` defaults to true and the design's toggle is the inverse of
/// it** ("Delete files too"). Written this way round on purpose: the field is
/// read at the moment the deletion happens, and a field called `delete_files`
/// whose default is `false` reads as "do not delete" — which is right — while a
/// field called `keep_files` whose default is `true` says the same thing in the
/// direction the *code* has to be correct in. The label is the UI's problem.
pub(crate) struct DeleteProject {
    pub(crate) id: String,
    pub(crate) keep_files: bool,
    /// Which project the files move to when they are kept. `None` is "no
    /// project", which is a real destination rather than a missing answer.
    pub(crate) transfer_to: Option<String>,
}

/// The New project modal's own state.
pub(crate) struct NewProject {
    pub(crate) name: String,
    /// Index into `project::PROJECT_COLORS`.
    pub(crate) color: usize,
    /// Whether to create a matching folder on disk — the user's choice, and the
    /// only thing that decides it (`library::project`).
    pub(crate) folder: bool,
    /// Whether the name field has already been handed the caret.
    ///
    /// ⚠️ **A latch, because "focus this on open" is a one-shot and the layout has
    /// no memory of having run before.** Asking every frame is the shape that made
    /// the field impossible to leave; asking on the frame the modal opens is the
    /// thing that was wanted, and this is where that frame is remembered.
    pub(crate) name_focused: bool,
}

impl Default for NewProject {
    fn default() -> Self {
        Self {
            name: String::new(),
            color: 0,
            // **On**, because a project the user can see in Explorer is the
            // version of this feature that needs no explanation, and the whole
            // point of a configurable base folder is that people look in it.
            folder: true,
            name_focused: false,
        }
    }
}

/// The Edit project modal's own state — the New project card again, with
/// *Create folder* replaced by *Archive project* and *Create* by *Save changes*.
///
/// ⚠️ **It holds the project's id and a *copy* of its fields, not a borrow and
/// not an index**, for [`Nav`]'s reason: `projects.json` is re-read on every
/// refresh — including refreshes something else caused — so the row being edited
/// can move, change or vanish while the card is open. The write at Save looks the
/// id up again and does nothing if it has gone.
pub(crate) struct EditProject {
    pub(crate) id: String,
    pub(crate) name: String,
    /// The colour as it will be written — **a hex string rather than an index
    /// into `project::PROJECT_COLORS`**, unlike [`NewProject::color`]. A project
    /// created by an older build or by a hand-edited file may name a colour this
    /// build has never heard of (`project::PROJECT_COLORS` says so in as many
    /// words), and an index has nowhere to put that: it would round the colour to
    /// a swatch the moment the card opened, without a click. Held as text, an
    /// unknown colour survives being edited for its *name*, and no swatch is
    /// ringed — which is the honest drawing of "not one of these nine".
    pub(crate) color: String,
    /// Whether the project is to be archived. See `library::project` for what
    /// that does and does not do.
    pub(crate) archived: bool,
    /// Whether the name field has already been handed the caret —
    /// [`NewProject::name_focused`]'s latch, and for its reason.
    pub(crate) name_focused: bool,
}

impl EditProject {
    /// Seed the card from the project as it stands.
    fn of(p: &project::Project) -> Self {
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            color: p.color.clone(),
            archived: p.archived,
            name_focused: false,
        }
    }

    /// Whether this card holds anything [`Self::of`] did not seed it with
    /// (§15 D719).
    ///
    /// **The mirror of `of`, and written directly under it so it stays one.**
    /// Every field `of` copies as *content* is compared here; a field added above
    /// without a line here is a change the card would offer to save and then not
    /// save, which is the shape this pair exists to make obvious. The two `of`
    /// copies that are deliberately absent are `id`, which is identity rather
    /// than content — a card whose id differs is a card for another project —
    /// and `name_focused`, which is a caret latch.
    ///
    /// ⚠️ **`name` is compared trimmed, because that is what gets written.**
    /// `save_project` stores `ep.name.trim()`, so a name gaining a trailing space
    /// is not an edit — and comparing untrimmed would relight the button for a
    /// keystroke that changes nothing on disk.
    fn differs_from(&self, p: &project::Project) -> bool {
        self.name.trim() != p.name || self.color != p.color || self.archived != p.archived
    }
}

/// Which of the sidebar's two project groups a row belongs to — and, because the
/// answer decides it, what the row carries at its trailing edge.
///
/// One enum rather than a `count: Option<usize>` beside an `archived: bool`,
/// because those two fields have a fourth combination that means nothing: an
/// archived row with a count, or an active one without.
enum ProjectRowKind {
    /// An ordinary project, with its file count.
    Active(usize),
    /// An archived one, with the way back out.
    Archived,
}

/// What a click on a sidebar project row asked for.
#[derive(PartialEq, Eq)]
enum ProjectRowHit {
    None,
    Select,
    Unarchive,
}

/// What a frame of the dashboard decided to do, acted on after the layout.
///
/// ⚠️ **Deferred rather than done in place, because every one of these mutates
/// the entry list the layout is iterating.** Deleting a document inside the loop
/// that is drawing it is the shape of bug egui makes easy and hard to see: the
/// row draws, the file moves, and the next row reads a `Vec` that has been
/// rebuilt underneath it.
enum Act {
    Open(PathBuf),
    NewFile,
    Duplicate(PathBuf),
    MoveTo(PathBuf, Option<String>),
    Trash(PathBuf),
    Restore(PathBuf),
    StartRename(PathBuf, String),
    CommitRename,
    CreateProject,
    /// Write the Edit project card back — the card itself is read from
    /// `DashboardState::edit_project`, as *Create project* reads its own.
    SaveProject,
    /// Archive or unarchive a project outright, with no card in the way: the
    /// sidebar's own unarchive button. Separate from [`Act::SaveProject`] because
    /// it is a different *promise* — this one touches nothing but the flag, and
    /// cannot rename a folder.
    SetArchived(String, bool),
    ToggleStar(String),
    Import,
    /// Delete a project. `Some(id)` transfers its files to another project;
    /// `None` leaves them unfiled; `keep_files: false` trashes them.
    DeleteProject {
        id: String,
        transfer_to: Option<String>,
        keep_files: bool,
    },
}

impl OndinApp {
    /// Draw the library screen.
    pub(crate) fn dashboard_ui(&mut self, ui: &mut egui::Ui) {
        let mut act: Option<Act> = None;

        // **Before anything is drawn** — see [`DashboardState::menu_was_up`]. The
        // dismissals run from inside the cards, so this is the only point in the
        // frame where the question *"was a menu on screen when this click
        // arrived"* still has the right answer.
        self.dash.menu_was_up = self.library_menu_open();

        // ⚠️ **Here, and not in `<OndinApp as eframe::App>::ui`'s
        // `take_dropped_images`.** That
        // call sits several lines *below* the `View::Dashboard` return, so the
        // whole drop path was editor-only and a document dragged onto the library
        // went nowhere at all. The two are deliberately separate rather than one
        // dispatcher: an image dropped on the canvas becomes a layer at a point,
        // and a document dropped on the library becomes a file in a folder —
        // different verbs, different targets, no shared logic beyond the `Vec`
        // egui hands over.
        //
        // 🚨 **A card up means no drop, which is §15 D759 one screen over** (D772).
        // That entry guarded the editor's `take_dropped_images`; this door had the
        // same hole and was found by reading D759's own call graph rather than by
        // the finding, which never named it. `raw.dropped_files` is filled from
        // winit before any widget runs, so an `egui::Modal` backdrop does not stop
        // a drop — a `.ondin` dragged onto the library while *Move to project*, a
        // delete confirmation, *New project*, *Edit project*, the library settings,
        // a rename or the recovery card was up **was filed anyway**, announcing
        // itself into a status line behind the backdrop.
        //
        // **`library_keys_are_free` rather than a new list**, which is the whole
        // point: that predicate is this screen's own enumeration of "a card is up",
        // already written for the keyboard (§15 D383's `dashboard_keys`). The
        // pointer asking a *second* list would be the two-implementations shape
        // D759 removed from the editor in the same breath.
        //
        // ⚠️ **Not `modal_is_up`, which is the editor's**: `settings_ui` is below
        // `dashboard_ui`'s `return` and never draws here, and `confirming_close`
        // belongs to a document this screen is not holding. The one card they share
        // is the recovery prompt — drawn above the view branch (§15 D377) — and
        // `library_keys_are_free` already names it.
        if self.library_keys_are_free() {
            self.take_dropped_documents(ui.ctx());
        }
        self.dashboard_top_bar(ui, &mut act);
        egui::Panel::left("library-sidebar")
            .exact_size(SIDEBAR_W)
            .resizable(false)
            .frame(ui::panel_frame(color::PANEL, 10, 14))
            .show(ui, |ui| self.dashboard_sidebar(ui, &mut act));
        // ⚠️ **The drop hint is drawn from out here, on the panel's *own* rect.**
        // Inside `dashboard_body` the only rect available is `ui.max_rect()`, which
        // is the panel less its 20pt margin — so the outline sat 20pt in from every
        // edge, close enough to the headings and the buttons to read as a border
        // around the content rather than around the drop target. `show` hands back
        // the panel's full rect, which is the container the files actually land in.
        let body = egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(color::BG).inner_margin(20))
            .show(ui, |ui| self.dashboard_body(ui, &mut act));
        self.library_drop_hint(ui, body.response.rect);

        self.new_project_modal(ui.ctx(), &mut act);
        self.edit_project_modal(ui.ctx(), &mut act);
        self.delete_project_modal(ui.ctx(), &mut act);
        self.move_modal(ui.ctx(), &mut act);
        self.delete_modal(ui.ctx(), &mut act);
        self.library_settings_modal(ui.ctx());

        if let Some(act) = act {
            self.dashboard_act(act);
        }
    }

    /// Brand mark, name, and the Settings door.
    fn dashboard_top_bar(&mut self, ui: &mut egui::Ui, act: &mut Option<Act>) {
        egui::Panel::top("library-topbar")
            .exact_size(TOP_H)
            .resizable(false)
            .frame(ui::panel_frame(color::TOPBAR, 14, 0))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    // The same mark as the editor's, at rest rather than as a
                    // control: this *is* the library, so there is nowhere for it
                    // to go. Painting it identically is what makes the editor's
                    // clickable one read as "back here".
                    let (mark, _) =
                        ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::empty());
                    let p = ui.painter();
                    p.rect_filled(mark, egui::CornerRadius::same(7), color::ACCENT_900);
                    p.rect_stroke(
                        mark,
                        egui::CornerRadius::same(7),
                        egui::Stroke::new(1.0, color::ACCENT_700),
                        egui::StrokeKind::Inside,
                    );
                    p.text(
                        mark.center(),
                        egui::Align2::CENTER_CENTER,
                        icon::DIAMOND,
                        theme::icon_font(17.0),
                        color::ACCENT,
                    );
                    ui.label(
                        egui::RichText::new("Ondin")
                            .size(13.0)
                            .color(theme::text::STRONG),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // ⚠️ **Its own modal, not the editor's.** The two settings
                        // screens answer different questions — where the library
                        // lives and how it opens, against how the editor behaves
                        // — so joining them would put six sections behind one
                        // button, of which four are about a document that is not
                        // open.
                        if icon_button(ui, icon::SLIDERS, 28.0, 17.0, false, true)
                            .on_hover_text("Library settings")
                            .clicked()
                            && !self.dash.menu_was_up
                        {
                            self.library_settings =
                                Some(LibrarySettings::from_prefs(&self.prefs, &self.library.root));
                        }
                        // ⚠️ **The library's half of the status line, and it did
                        // not exist.** `EditorSession::status` had one production
                        // reader — the editor's top bar, which is below
                        // `OndinApp::ui`'s `View::Dashboard` return — so all
                        // **22** messages this file writes were painted nowhere.
                        // A *Delete project* that could not read `projects.json`
                        // closed its confirmation, left the project in the
                        // sidebar and said nothing; a *Delete files too* that
                        // failed to trash some members removed the row and left
                        // those documents on disk carrying a project id nothing
                        // lists. The message was not cleared either — it
                        // surfaced later, out of context, in the editor.
                        //
                        // Same function as the editor's, so the two screens
                        // cannot disagree about what a failure looks like
                        // (`crate::app::status_label`).
                        //
                        // 🚨 **A dot with the message on hover, not the message**
                        // (§15 D751). This read
                        // `status_label(ui, self.session.status(), room)` with
                        // `room = (ui.max_rect().width() - SEARCH_W) / 2.0 - 48.0`
                        // — half the leftover space beside the painted search
                        // field, less the gear — under a comment calling it
                        // *"bounded, because the search field is painted across
                        // the centre of this bar rather than laid out in it"*.
                        // The bound was right and it is a **function of the
                        // window**: 382pt at the default 1200, 22pt at 500, zero
                        // at 436, and nothing stops the window being any of them.
                        // So the message faded out exactly where present mode says
                        // the user is working — a laptop screen with the panels in
                        // the way. A dot is the same size at every width and the
                        // tooltip carries the sentence whole rather than an
                        // ellipsis of it.
                        crate::app::status_dot(ui, self.session.status());
                    });

                    // **Painted into the middle of the bar rather than laid out
                    // between the two clusters**, so it is centred on the
                    // *window* and not on whatever room the brand and the gear
                    // left over. The design centres it, and a field that drifts
                    // when a project name gets longer would be the version of
                    // this that layout gives you for free.
                    let bar = ui.max_rect();
                    let field = egui::Rect::from_center_size(
                        egui::pos2(bar.center().x, bar.center().y),
                        egui::vec2(SEARCH_W, 28.0),
                    );
                    self.search_field(ui, field, act);
                });
            });
    }

    /// The search box, and the results under it while it is open.
    fn search_field(&mut self, ui: &mut egui::Ui, rect: egui::Rect, act: &mut Option<Act>) {
        let ctx = ui.ctx().clone();
        // ⚠️ **`Ctrl+K` is read here rather than through `input::resolve`**,
        // which is not running at all while the dashboard is up (`OndinApp::ui`
        // returns before it). The keymap is the editor's; this screen answers its
        // own keys — these two here, and the arrows, `Enter`, `Delete` and
        // `Escape` in [`OndinApp::dashboard_keys`], which is guarded on this
        // overlay being shut precisely because these two are read first.
        //
        // ⚠️ **And guarded by the same list `dashboard_keys` uses** (§15 D542) —
        // [`OndinApp::library_keys_are_free`], which exists because this was the
        // one of the two context-reading sites that had no guard at all.
        // 🚨 **`Ctrl` without Alt and without Shift, which is how the sibling
        // spells it and why** (§15 D712, `[S20.1-L1-06]`). This gate was
        // `i.modifiers.command` alone, so `Ctrl+Shift+K`, `Ctrl+Alt+K`,
        // `Ctrl+Shift+P` and `Ctrl+Alt+P` all opened the search — four
        // spellings `docs/shortcuts.md` §8's dashboard row does not list, on the
        // one chord site on this screen with no exclusion. **The reason is
        // already written in this file**, on `dashboard_keys`' own gate: *"so
        // `Ctrl+Alt+N` and `Ctrl+Shift+D` stay unbound here exactly as they are
        // there, rather than becoming second doors this screen invented."* This
        // was the second door that comment forbids, in the same `impl OndinApp`
        // and some 2,300 lines above it.
        //
        // ⚠️ **`Ctrl+Shift+K` is bound in the *editor*** — *Place image*. The two
        // screens cannot collide, because `input::resolve` does not run while
        // the dashboard is up; what was wrong is that one chord meant two things
        // and only one of them was written down.
        //
        // This is §15 D710's rule on the screen `input::resolve` never reaches,
        // which is why D710 could not have closed it.
        let chord = self.library_keys_are_free()
            && ctx.input(|i| {
                let cmd_only = i.modifiers.command && !i.modifiers.alt && !i.modifiers.shift;
                cmd_only && (i.key_pressed(egui::Key::K) || i.key_pressed(egui::Key::P))
            });
        if chord && self.dash.search.is_none() {
            self.dash.search = Some(String::new());
            self.dash.search_row = None;
        }

        let open = self.dash.search.is_some();
        let resp = ui.interact(rect, egui::Id::new("library-search"), egui::Sense::click());
        let p = ui.painter();
        p.rect_filled(rect, egui::CornerRadius::same(7), theme::color::text_a(12));
        p.rect_stroke(
            rect,
            egui::CornerRadius::same(7),
            egui::Stroke::new(
                1.0,
                if open {
                    color::ACCENT
                } else {
                    theme::color::text_a(20)
                },
            ),
            egui::StrokeKind::Inside,
        );
        p.text(
            egui::pos2(rect.left() + 10.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            icon::MAGNIFYING_GLASS,
            theme::icon_font(14.0),
            theme::text::FAINT,
        );

        if !open {
            p.text(
                egui::pos2(rect.left() + SEARCH_TEXT_X, rect.center().y),
                egui::Align2::LEFT_CENTER,
                "Search files and projects",
                egui::FontId::proportional(SEARCH_PT),
                theme::text::FAINT,
            );
            // The hint is the only thing that teaches the chord, so it is drawn
            // at rest and hidden the moment the field is live — at which point
            // it would be telling the user how to do what they are doing.
            p.text(
                egui::pos2(rect.right() - 10.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                "Ctrl + K",
                egui::FontId::proportional(10.0),
                theme::text::FAINT,
            );
            if resp.clicked() && !self.dash.menu_was_up {
                self.dash.search = Some(String::new());
                self.dash.search_row = None;
            }
            if resp.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::Text);
            }
            return;
        }

        // ⚠️ **Consumed, not read** (§15 D543) — the same spelling and the same
        // reason as the arrows six lines down. `[S20.1-L1-03]`: this branch runs
        // first in the frame (`dashboard_ui` draws the top bar before the body)
        // and returns before the `TextEdit` is drawn, so `dashboard_keys` later
        // in the *same* frame found `search.is_some()` already `false`, found
        // `egui_wants_keyboard_input()` false too — egui surrenders focus on
        // `Escape` during `begin_pass` — and reached its own Escape arm, which
        // clears `dash.selected`. So one press closed the overlay **and** threw
        // away the keyboard cursor, and the following `Enter`, `Delete`, `F2` or
        // `Ctrl+D` did nothing.
        //
        // `docs/shortcuts.md` §8a's Escape row is the spec and the code was the
        // one row disagreeing with it: *"with the search overlay up it belongs to
        // the overlay, which closes **instead**"*. The ⋮ menu is the positive
        // control and always behaved — it closes and returns, and the selection
        // survives.
        //
        // ⚠️ **This is also where §15 D374's measurement stops holding, and the
        // entry says how.** D374 keeps `dashboard_keys`' `search.is_some()` guard
        // on the finding that it is *"measurably redundant"*, because
        // `request_focus` makes `egui_wants_keyboard_input` true inside the same
        // pass — true on a **steady-state** frame, and false on the **closing**
        // one, where the field is never drawn. Its own closing words are the
        // diagnosis: *"a guard resting on the draw order of two functions is not
        // a guard."*
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.dash.search = None;
            return;
        }
        // ⚠️ **Consumed here, above the `TextEdit`, rather than read after it**
        // (§15 D382). The field holds the caret the whole time the overlay is up, so
        // an arrow it can still see is an arrow that moves the caret to the start or
        // the end of the query — the text jumping about under the highlight the same
        // key is supposed to be moving. `consume_key` reads *and* removes, which is
        // the only way to have both, and it must run before the widget is added.
        let step = ctx.input_mut(|i| {
            i32::from(i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown))
                - i32::from(i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp))
        });
        let Some(query) = self.dash.search.as_mut() else {
            return;
        };
        // ⚠️ **The box is exactly one text row tall and centred by hand.**
        // `Ui::put` lays its widget out *justified*, so a `TextEdit` handed the
        // whole 28pt field is stretched to it and then draws its galley at the top
        // of that box — which is why the live field's placeholder sat higher than
        // the resting one it replaces, reported as *"text in search box isn't
        // aligned when it takes focus"*. A rect that is the galley's own height
        // makes top-alignment and centring the same thing.
        //
        // ⚠️ **And the horizontal inset is the rect's, not `TextEdit::margin`'s.**
        // egui ignores that builder entirely once a frame is supplied —
        // `let frame = frame.unwrap_or_else(|| Frame::new().inner_margin(margin))`
        // — so `Frame::NONE` means *no* inset whatever `margin` says, and the left
        // edge of the box is where the text lands. Which is why [`SEARCH_TEXT_X`]
        // is the rect's `left`: it is the only number in play.
        let row_h = ui
            .ctx()
            .fonts_mut(|f| f.row_height(&egui::FontId::proportional(SEARCH_PT)));
        let inner = egui::Rect::from_min_max(
            egui::pos2(rect.left() + SEARCH_TEXT_X, rect.center().y - row_h / 2.0),
            egui::pos2(rect.right() - 10.0, rect.center().y + row_h / 2.0),
        );
        let edit = ui.put(
            inner,
            egui::TextEdit::singleline(query)
                .id(egui::Id::new("library-search-edit"))
                // The box is painted above, so the widget contributes only its
                // text — a second frame inside the first is the tell that a
                // hand-painted field forgot to turn egui's off, and `Frame::NONE`
                // is also what suppresses egui's focus ring (`ui::text_field`).
                .frame(egui::Frame::NONE)
                .font(egui::FontId::proportional(SEARCH_PT))
                .hint_text(
                    egui::RichText::new("Search files and projects")
                        .size(SEARCH_PT)
                        .color(theme::text::FAINT),
                ),
        );
        // Requested every frame it is drawn rather than once, for `rename_field`'s
        // reason: the field appears in a layout with no memory of having asked, and
        // one missed frame leaves a box nobody can type into. What makes that safe
        // to do is the dismissal below — without it the field held focus for the
        // rest of the session, which is what *"once they gain it, clicking anywhere
        // doesn't make them lose focus"* was about.
        if !edit.has_focus() {
            edit.request_focus();
        }
        // A new query is a new list, so the arrows start over from the first
        // document match rather than from wherever they had got to in the last one.
        if edit.changed() {
            self.dash.search_row = None;
        }
        let panel = self.search_results(ui, rect, step, act);
        // **A press outside the field and its dropdown shuts the overlay**, which
        // is what a click anywhere else has to mean: the field is not a mode, and
        // Escape was the only way out of it. Read as a *press* rather than a click
        // so the dismissal lands on the same frame the pointer goes down, before
        // whatever is underneath acts on the release.
        let outside = ui.ctx().input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !rect.contains(p) && !panel.is_some_and(|r| r.contains(p)))
        });
        if outside {
            self.dash.search = None;
        }
    }

    /// The dropdown under the search field: matching projects, then matching
    /// documents.
    ///
    /// **Projects first and files after, both capped**, because the two answer
    /// different questions and a project match is the more useful one when both
    /// exist: "kestrel" almost certainly means the project rather than the six
    /// files inside it.
    fn search_results(
        &mut self,
        ui: &mut egui::Ui,
        anchor: egui::Rect,
        step: i32,
        act: &mut Option<Act>,
    ) -> Option<egui::Rect> {
        let query = self.dash.search.clone().unwrap_or_default();
        let needle = query.trim().to_lowercase();
        // Empty query shows what was searched before, which is the design's
        // *Recent searches* — and is what makes the overlay useful the instant
        // it opens rather than only after typing.
        let recents: Vec<String> = if needle.is_empty() {
            self.library.local.recent_searches.clone()
        } else {
            Vec::new()
        };
        // ⚠️ **The whole list, archived included — the one deliberate exception to
        // `Projects::active`.** Everywhere else a project appears in a list the
        // user is *choosing* from, and archiving is a statement about what should
        // be on offer. A search is the opposite: the name has already been typed,
        // so the only thing hiding the match could achieve is making an archived
        // project unreachable while its group is collapsed.
        let projects: Vec<(String, String, String)> = if needle.is_empty() {
            Vec::new()
        } else {
            self.library
                .projects
                .projects
                .iter()
                .filter(|p| p.name.to_lowercase().contains(&needle))
                .take(SEARCH_MAX)
                .map(|p| (p.id.clone(), p.name.clone(), p.color.clone()))
                .collect()
        };
        let files: Vec<Entry> = if needle.is_empty() {
            Vec::new()
        } else {
            self.library
                .entries
                .iter()
                .filter(|e| e.display_name().to_lowercase().contains(&needle))
                .take(SEARCH_MAX)
                .cloned()
                .collect()
        };
        let rows = recents.len() + projects.len() + files.len();
        if rows == 0 && !needle.is_empty() {
            return Some(Self::search_panel(ui, anchor, 46.0, |ui, rect| {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "No matches",
                    egui::FontId::proportional(11.5),
                    theme::text::DIM,
                );
            }));
        }
        if rows == 0 {
            return None;
        }

        // ⚠️ **Where the highlight sits, and `Enter` takes *that* row now** (§15
        // D382) — every row, not only a document: a recent query refills the field
        // and a project navigates, exactly as clicking them does, because a list you
        // can arrow through and then cannot act on is worse than no list at all.
        //
        // **Unsteered, it still sits on the first document match**, which is the
        // rule `Enter` has always followed — *"opening a file is the thing the field
        // is for, and a project row is one click away with the answer visible"* —
        // and is why `search_row` distinguishes `None` from `Some(0)`. The recents
        // and project groups above it change length as the query does, so the
        // default has to be recomputed rather than remembered.
        let default = if files.is_empty() {
            0
        } else {
            recents.len() + projects.len()
        };
        let mut sel = self.dash.search_row.unwrap_or(default).min(rows - 1);
        if step != 0 {
            // Clamped rather than wrapping, which is `arrow_target`'s answer for the
            // grid two functions away: no press is dead, and neither end runs on.
            sel = (sel as i32 + step).clamp(0, rows as i32 - 1) as usize;
            self.dash.search_row = Some(sel);
        }
        let enter = ui.ctx().input(|i| i.key_pressed(egui::Key::Enter));
        let mut chosen: Option<Act> = None;
        let mut nav_to: Option<Nav> = None;
        let mut refill: Option<String> = None;
        let h = rows as f32 * SEARCH_ROW_H + 10.0;
        let panel = Self::search_panel(ui, anchor, h, |ui, rect| {
            let mut y = rect.top() + 5.0;
            let mut row = |ui: &mut egui::Ui,
                           i: usize,
                           glyph: &str,
                           ink: egui::Color32,
                           label: &str,
                           meta: &str|
             -> bool {
                let r = egui::Rect::from_min_size(
                    egui::pos2(rect.left() + 4.0, y),
                    egui::vec2(rect.width() - 8.0, SEARCH_ROW_H),
                );
                y += SEARCH_ROW_H;
                let resp = ui.interact(r, ui.id().with(("search-row", i)), egui::Sense::click());
                // ⚠️ **Two grounds, because the pointer and the keyboard can be on
                // different rows at once** and the one `Enter` will take has to be
                // the louder of the two. The same split the grid draws for a hovered
                // card against the keyboard's cursor.
                let ground = match (i == sel, resp.hovered()) {
                    (true, _) => Some(theme::color::text_a(20)),
                    (false, true) => Some(theme::color::text_a(14)),
                    (false, false) => None,
                };
                if let Some(ground) = ground {
                    ui.painter()
                        .rect_filled(r, egui::CornerRadius::same(6), ground);
                }
                let p = ui.painter();
                p.text(
                    egui::pos2(r.left() + 10.0, r.center().y),
                    egui::Align2::LEFT_CENTER,
                    glyph,
                    theme::icon_font(13.0),
                    ink,
                );
                p.text(
                    egui::pos2(r.left() + 30.0, r.center().y),
                    egui::Align2::LEFT_CENTER,
                    label,
                    egui::FontId::proportional(12.0),
                    theme::text::STRONG,
                );
                if !meta.is_empty() {
                    p.text(
                        egui::pos2(r.right() - 10.0, r.center().y),
                        egui::Align2::RIGHT_CENTER,
                        meta,
                        egui::FontId::proportional(10.5),
                        theme::text::FAINT,
                    );
                }
                resp.clicked() || (enter && i == sel)
            };
            let mut i = 0;
            for q in &recents {
                if row(
                    ui,
                    i,
                    icon::CLOCK_COUNTER_CLOCKWISE,
                    theme::text::FAINT,
                    q,
                    "",
                ) {
                    refill = Some(q.clone());
                }
                i += 1;
            }
            for (id, name, color) in &projects {
                if row(ui, i, icon::FOLDER_OPEN, swatch(color), name, "Project") {
                    nav_to = Some(Nav::Project(id.clone()));
                }
                i += 1;
            }
            for e in &files {
                if row(
                    ui,
                    i,
                    icon::FILES,
                    theme::text::DIM,
                    &e.display_name(),
                    "File",
                ) {
                    chosen = Some(Act::Open(e.path.clone()));
                }
                i += 1;
            }
        });

        if let Some(q) = refill {
            self.dash.search = Some(q);
            // A refilled query is a new list; the arrows start over.
            self.dash.search_row = None;
            return Some(panel);
        }
        if let Some(nav) = nav_to {
            self.dash.nav = nav;
            self.remember_search(&query);
            self.dash.search = None;
            return Some(panel);
        }
        if let Some(a) = chosen {
            self.remember_search(&query);
            self.dash.search = None;
            *act = Some(a);
        }
        Some(panel)
    }

    /// The panel the results are drawn into — one place that owns its ground,
    /// its border and its position under the field.
    ///
    /// Returns the rect it drew into, which is what lets the field tell a press on
    /// a result row from a press that means *shut this*.
    ///
    /// ⚠️ **No receiver** (§15 D707, `[S20.1-L3-08]`). This took `&self` and its
    /// body reads only `ui.ctx()`, `anchor`, `height` and `body` — the receiver
    /// was the one and only reason it was a method. `clippy::unused_self` is
    /// `pedantic` and this project does not run pedantic as a gate, so nothing
    /// was ever going to say so.
    ///
    /// **Left in the `impl` as an associated function rather than lifted to
    /// module scope**: both call sites already spell it `Self::search_panel`, so
    /// moving the body would be churn with the same outcome.
    ///
    /// 🚨 **This is deliberately *not* §15 D269's move, and the first draft of
    /// this comment said it was.** D269 is about a `&mut self` method's *call
    /// site* being unobservable — *"no test in this crate can say where a `&mut
    /// self` method is called from"* — and its remedy is lifting the **decision**
    /// into a free function so the arithmetic is pinned even though the placement
    /// cannot be. The *reachability* half was answered separately by D303's
    /// `OndinApp::headless`, which that entry says in as many words *"makes the
    /// method callable without making the call site observable"*. This function
    /// decides nothing and has been reachable since D303; dropping the receiver
    /// is the smaller thing, a signature that stops claiming to read state it
    /// does not read.
    fn search_panel(
        ui: &mut egui::Ui,
        anchor: egui::Rect,
        height: f32,
        body: impl FnOnce(&mut egui::Ui, egui::Rect),
    ) -> egui::Rect {
        egui::Area::new(egui::Id::new("library-search-results"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(anchor.left(), anchor.bottom() + 6.0))
            .show(ui.ctx(), |ui| {
                let (rect, _) = ui
                    .allocate_exact_size(egui::vec2(anchor.width(), height), egui::Sense::empty());
                let p = ui.painter();
                p.rect_filled(rect, egui::CornerRadius::same(9), color::FIELD);
                p.rect_stroke(
                    rect,
                    egui::CornerRadius::same(9),
                    egui::Stroke::new(1.0, color::FIELD_BORDER),
                    egui::StrokeKind::Inside,
                );
                body(ui, rect);
                rect
            })
            .inner
    }

    /// Push a query onto the per-machine recent-search list.
    ///
    /// **Only on a query that led somewhere.** Recording every keystroke's worth
    /// of prefix would fill the list with the six substrings of the one word the
    /// user actually typed.
    fn remember_search(&mut self, query: &str) {
        let q = query.trim();
        if q.is_empty() {
            return;
        }
        let recents = &mut self.library.local.recent_searches;
        recents.retain(|r| r != q);
        recents.insert(0, q.to_string());
        recents.truncate(SEARCH_RECENTS);
        self.library.local.save();
    }

    fn dashboard_sidebar(&mut self, ui: &mut egui::Ui, act: &mut Option<Act>) {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.add_space(2.0);

        // *New file* is the first thing in the sidebar because it is the reason
        // most visits happen — and it is the accent button on the screen, the
        // only one.
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::click());
        let lit = resp.hovered();
        let p = ui.painter();
        p.rect_filled(
            rect,
            egui::CornerRadius::same(7),
            if lit {
                color::ACCENT_700
            } else {
                color::ACCENT_800
            },
        );
        p.rect_stroke(
            rect,
            egui::CornerRadius::same(7),
            egui::Stroke::new(1.0, color::ACCENT),
            egui::StrokeKind::Inside,
        );
        // ⚠️ **The glyph is a second `p.text` in the icon font, not a `＋` in the
        // label.** This read `"＋  New file"` in `FontId::proportional`, and U+FF0B
        // is a *fullwidth* plus that bundled Inter has no glyph for — so it drew
        // the empty box egui falls back to and the button looked broken. An icon
        // is `theme::icon_font`'s business; nothing in this app should be spelling
        // one as a character in a prose font.
        //
        // Measured then placed, so the pair is centred as one thing rather than
        // the word being centred with a glyph hanging off it — `ui::action_button`
        // does the same arithmetic, and cannot be borrowed here because it paints
        // `button_face`'s recessed ground and this is the one accent button on the
        // screen.
        const GLYPH_PT: f32 = 15.0;
        const GAP: f32 = 7.0;
        let font = egui::FontId::proportional(12.5);
        let word = ui.ctx().fonts_mut(|f| {
            f.layout_no_wrap("New file".to_owned(), font.clone(), color::ACCENT_100)
        });
        let total = GLYPH_PT + GAP + word.size().x;
        let left = rect.center().x - total / 2.0;
        let p = ui.painter();
        p.text(
            egui::pos2(left + GLYPH_PT / 2.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            icon::PLUS,
            theme::icon_font(GLYPH_PT),
            color::ACCENT_100,
        );
        p.galley(
            egui::pos2(left + GLYPH_PT + GAP, rect.center().y - word.size().y / 2.0),
            word,
            color::ACCENT_100,
        );
        // 🚨 **R4's seventh door and the second that *creates* a document** (§15
        // D558). Not a `nav_row` — this is a hand-rolled accent button — so
        // gating that function did not reach it, and it is the first thing in the
        // sidebar. `arch-scribe` found it by reading every `.clicked()` in the
        // module against the gate, after two rounds of reading the *fix*.
        if resp.clicked() && !self.dash.menu_was_up {
            *act = Some(Act::NewFile);
        }

        ui.add_space(10.0);
        // The labels come from `Nav::label` rather than being written here, so
        // the sidebar and the Settings picker cannot drift apart.
        for (nav, glyph) in [
            (Nav::Recent, icon::CLOCK_COUNTER_CLOCKWISE),
            (Nav::All, icon::FILES),
            (Nav::Starred, icon::STAR),
            (Nav::Trash, icon::TRASH_SIMPLE),
        ] {
            let count = self.nav_count(&nav);
            let label = nav.label();
            if self.nav_row(ui, glyph, label, Some(count), self.dash.nav == nav) {
                self.dash.nav = nav;
            }
        }

        ui.add_space(12.0);
        section_label(ui, "Projects");
        // Cloned because the row draws while `self` is borrowed for the nav
        // comparison below; a project list is a handful of small records.
        let projects: Vec<project::Project> = self.library.projects.active().cloned().collect();
        for p in &projects {
            let selected = self.dash.nav == Nav::Project(p.id.clone());
            let count = self.library.file_count(&p.id);
            if self.project_row(ui, p, ProjectRowKind::Active(count), selected)
                == ProjectRowHit::Select
            {
                self.dash.nav = Nav::Project(p.id.clone());
            }
        }
        if self.nav_row(ui, icon::PLUS, "New project", None, false) {
            self.dash.new_project = Some(NewProject::default());
        }

        // **The archived group, and nothing at all when there is nothing in it.**
        // A permanently visible empty heading would be the sidebar carrying a
        // feature most libraries never use — and the design gates it on
        // `hasArchived` for the same reason.
        let archived: Vec<project::Project> = self.library.projects.archived().cloned().collect();
        if !archived.is_empty() {
            ui.add_space(12.0);
            if archived_header(ui, self.dash.archived_open, archived.len()) {
                self.dash.archived_open = !self.dash.archived_open;
            }
            if self.dash.archived_open {
                for p in &archived {
                    let selected = self.dash.nav == Nav::Project(p.id.clone());
                    match self.project_row(ui, p, ProjectRowKind::Archived, selected) {
                        ProjectRowHit::Select => self.dash.nav = Nav::Project(p.id.clone()),
                        ProjectRowHit::Unarchive => {
                            *act = Some(Act::SetArchived(p.id.clone(), false));
                        }
                        ProjectRowHit::None => {}
                    }
                }
            }
        }

        // Bottom of the sidebar, away from everything above it: importing is the
        // rare action and the only one on this screen that opens an OS dialog.
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            if self.nav_row(ui, icon::FOLDER_OPEN, "Import files", None, false) {
                *act = Some(Act::Import);
            }
        });
    }

    /// How many documents an entry would show, drawn beside its label.
    fn nav_count(&self, nav: &Nav) -> usize {
        match nav {
            Nav::Recent => self.library.recent().len(),
            Nav::All => self.library.entries.len(),
            Nav::Starred => self
                .library
                .entries
                .iter()
                .filter(|e| self.is_starred(e))
                .count(),
            // **The cache, not the scan** (§15 D728, `[S20.1-L3-07]`). This ran
            // `store::trashed` — a scan of every file in `.trash` — and the
            // sidebar paints this count on **every nav, every frame**, whether or
            // not the trash is being looked at. See `Library::trashed`.
            Nav::Trash => self.library.trashed().len(),
            Nav::Project(id) => self.library.file_count(id),
        }
    }

    fn nav_row(
        &mut self,
        ui: &mut egui::Ui,
        glyph: &str,
        label: &str,
        count: Option<usize>,
        selected: bool,
    ) -> bool {
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), NAV_H),
            egui::Sense::click(),
        );
        let lit = resp.hovered();
        let fg = if selected {
            color::TEXT
        } else if lit {
            theme::text::STRONG
        } else {
            theme::text::MUTED
        };
        let p = ui.painter();
        if selected || lit {
            p.rect_filled(
                rect,
                egui::CornerRadius::same(7),
                theme::color::text_a(if selected { 20 } else { 12 }),
            );
        }
        p.text(
            egui::pos2(rect.left() + 12.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            glyph,
            theme::icon_font(15.0),
            fg,
        );
        p.text(
            egui::pos2(rect.left() + 34.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(12.5),
            fg,
        );
        if let Some(n) = count {
            p.text(
                egui::pos2(rect.right() - 12.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                n.to_string(),
                egui::FontId::proportional(11.0),
                theme::text::FAINT,
            );
        }
        // R4, as at every other door — see [`OndinApp::pick_or_open`] and §15 D558.
        resp.clicked() && !self.dash.menu_was_up
    }

    /// One project in the sidebar, in either of the two groups.
    ///
    /// ⚠️ **The archived row is the same row a tier quieter, not a different
    /// control.** It selects the same way and lands on the same page — archiving
    /// hides a project from every *list*, and this group is the one place it is
    /// still listed, so a row that looked or behaved like something else would be
    /// saying the project had changed rather than that it had been put away.
    fn project_row(
        &mut self,
        ui: &mut egui::Ui,
        p: &project::Project,
        kind: ProjectRowKind,
        selected: bool,
    ) -> ProjectRowHit {
        let archived = matches!(kind, ProjectRowKind::Archived);
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), NAV_H),
            egui::Sense::click(),
        );
        // ⚠️ **Registered after the row and therefore on top of it**, which is the
        // whole reason the two can share a rect — `file_menu_button` does the same
        // for the ⋮ inside a file row. The click still has to be *taken* from the
        // row below by returning early, or unarchiving would also navigate.
        let undo = archived.then(|| {
            ui.interact(
                egui::Rect::from_center_size(
                    egui::pos2(rect.right() - UNARCHIVE_X, rect.center().y),
                    egui::Vec2::splat(22.0),
                ),
                egui::Id::new(("unarchive", &p.id)),
                egui::Sense::click(),
            )
        });
        let lit = resp.hovered();
        let fg = if selected {
            color::TEXT
        } else if lit {
            theme::text::STRONG
        } else if archived {
            theme::text::DIM
        } else {
            theme::text::MUTED
        };
        let painter = ui.painter();
        if selected || lit {
            painter.rect_filled(
                rect,
                egui::CornerRadius::same(7),
                theme::color::text_a(if selected { 20 } else { 12 }),
            );
        }
        painter.circle_filled(
            egui::pos2(rect.left() + 18.0, rect.center().y),
            4.0,
            // The design's `opacity:.45` on the archived dot. The colour is the
            // project's identity and stays recognisable; what the fade says is
            // that the row is not in the running.
            if archived {
                swatch(&p.color).gamma_multiply(0.45)
            } else {
                swatch(&p.color)
            },
        );
        painter.text(
            egui::pos2(rect.left() + 34.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            &p.name,
            egui::FontId::proportional(12.5),
            fg,
        );
        match kind {
            ProjectRowKind::Active(n) => painter.text(
                egui::pos2(rect.right() - 12.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                n.to_string(),
                egui::FontId::proportional(11.0),
                theme::text::FAINT,
            ),
            // The count's place, because an archived project's count is the one
            // number on this screen nobody is scanning — and putting the way back
            // where the count was is what keeps the two groups the same shape.
            ProjectRowKind::Archived => painter.text(
                egui::pos2(rect.right() - UNARCHIVE_X, rect.center().y),
                egui::Align2::CENTER_CENTER,
                icon::ARROW_COUNTER_CLOCKWISE,
                theme::icon_font(13.0),
                if undo.as_ref().is_some_and(egui::Response::hovered) {
                    color::TEXT
                } else {
                    theme::text::FAINT
                },
            ),
        };
        if let Some(undo) = undo {
            // ⚠️ **A tooltip on a painted glyph, which is only legible because the
            // glyph has a `Response` of its own.** Hung on the row instead it
            // would explain the wrong thing, and hung on a `Ui::scope` it would
            // show nothing at all.
            let undo = undo.on_hover_text("Unarchive");
            if undo.clicked() && !self.dash.menu_was_up {
                return ProjectRowHit::Unarchive;
            }
        }
        // R4 again (§15 D558): a click spent getting rid of a floating menu must
        // not also re-nav the sidebar. **Both hits, not only `Select`** — the
        // *Unarchive* button shares this row's rect, and "the click did something
        // I did not ask for" is the same complaint whichever of the two it was.
        if resp.clicked() && !self.dash.menu_was_up {
            ProjectRowHit::Select
        } else {
            ProjectRowHit::None
        }
    }

    /// The heading, the controls beside it, and the documents.
    fn dashboard_body(&mut self, ui: &mut egui::Ui, act: &mut Option<Act>) {
        let entries = self.visible_entries();
        // ⚠️ **A selection that names nothing is cleared here rather than left
        // to the drawing.** Every action on this screen writes to disk and
        // re-scans, so a delete, a move or a rename all leave `selected` pointing
        // at a path that is gone — and a stale one is not merely invisible: the
        // *next* thing to read it would act on a file that is not there. Checked
        // against `entries` rather than `library.entries` because that is the list
        // the user is looking at; a selection filtered off screen is one they
        // cannot dismiss either.
        if self
            .dash
            .selected
            .as_ref()
            .is_some_and(|p| !entries.iter().any(|e| &e.path == p))
        {
            self.dash.selected = None;
        }
        // **Before anything draws**, so a card the arrows just moved onto can
        // scroll itself into view on the same frame rather than one late — and
        // after the stale-selection clear above, so the index the arrows step from
        // is an index into the list actually on screen.
        self.dashboard_keys(&ui.ctx().clone(), &entries, act);
        // ⚠️ **Registered before the cards, so the cards win.** egui's hit test
        // takes the *last* widget registered at a point, which is the trap
        // `ui::icon_button_padded` describes from the other side — here it is the
        // mechanism: this claims the whole body, every card claims a piece of it
        // afterwards, and what is left over is the empty space a click has to land
        // on to mean *nothing, thanks*. A background that sensed after the cards
        // would swallow every click on the grid.
        let background = ui.interact(
            ui.max_rect(),
            egui::Id::new("library-body-background"),
            egui::Sense::click(),
        );
        self.dashboard_header(ui, entries.len());
        ui.add_space(14.0);

        // **Projects appear above the files on *Recent* only.** Everywhere else
        // the user has already chosen what they are looking at — a project, or
        // everything — and a row of project cards there would be a second
        // navigation for a decision already made. Recent is the one entry that
        // is a landing page rather than a filter.
        // ⚠️ **The same predicate the cards are drawn from, or the heading appears
        // over nothing.** That is what this gate is for, and there are now three
        // ways to reach the empty case rather than one: no projects, every project
        // archived, and every project empty.
        let show_projects = self.dash.nav == Nav::Recent && !self.recent_projects().is_empty();
        // **A project's grid ends in a *New file* card, so a project is never
        // empty enough for the empty state.** The dashed card says "nothing here,
        // make one" and offers the verb in the same breath, which is strictly more
        // than `empty_state`'s icon and sentence, which answers a question about
        // the *library* rather than the one a project with no files is asking.
        // ⚠️ **This used to add "it names the base folder" and no longer can**:
        // D382 took the path out of every empty state. The argument survives its
        // evidence — the card offers the verb where the sentence only names the
        // absence — but the sentence it replaces is one line shorter than this
        // comment had it. **The card is not drawn at all while the library's
        // folder is missing** (`new_file_into`), which is the one case where the
        // empty state has more to say than the card does.
        let new_file = self.new_file_into();

        if entries.is_empty() && !show_projects && new_file.is_none() {
            self.empty_state(ui);
        } else {
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    if show_projects {
                        self.project_cards(ui);
                        ui.add_space(20.0);
                        section_label(ui, "FILES");
                        ui.add_space(6.0);
                    }
                    if entries.is_empty() && new_file.is_none() {
                        self.empty_state(ui);
                    } else if self.dash.list_view {
                        self.file_list(ui, &entries, new_file.as_deref(), act);
                    } else {
                        self.file_grid(ui, &entries, new_file.as_deref(), act);
                    }
                });
        }
        // Read after the cards have had their turn — the response is last frame's
        // either way, but the *order of registration* above is what makes this
        // mean "on nothing" rather than "anywhere".
        if background.clicked() {
            self.dash.selected = None;
        }
    }

    /// *Drop to import* — the outline and the words, while files are over the
    /// window (§15 D373).
    ///
    /// **The dashboard had no drop handling at all**: `OndinApp::ui` returns to
    /// the caller as soon as the library is up, several lines before
    /// `take_dropped_images`, so a `.ondin` file dragged onto this screen landed
    /// nowhere and said nothing. Importing was the sidebar row and a native dialog.
    ///
    /// **A hint, not just a handler.** A drop target that gives no sign it is one
    /// is a feature nobody finds; the canvas says the same thing the same way
    /// (`CanvasRenderer::draw_drop_indicator`), and this borrows its colour so the
    /// two screens agree about what a drop outline looks like.
    /// `library_` rather than `drop_hint`, because `panels::layers` already has a
    /// `drop_hint` on `OndinApp` and two inherent methods of one name is a compile
    /// error rather than a shadowing — which is the useful kind of collision.
    ///
    /// ⚠️ **The outline marks the body, and the handler takes a drop anywhere on
    /// the window** — `raw.dropped_files` is window-level and
    /// [`Self::take_dropped_documents`] does not ask where. The mismatch is in the
    /// forgiving direction on purpose: the hint under-promises, so a drop aimed at
    /// the outline always works and one that misses it works anyway. Marking the
    /// whole window instead would need a foreground `Area` over the sidebar and
    /// the top bar, which would say the search field and the settings gear were
    /// drop targets.
    fn library_drop_hint(&self, ui: &egui::Ui, rect: egui::Rect) {
        let files = ui.input(|i| i.raw.hovered_files.len());
        if files == 0 {
            return;
        }
        // ⚠️ **Square, and hugging the container rather than floating inside it.**
        // A 10pt corner is what a *card* wears; this is not a card, it is the edge
        // of the region a file will land in, and it now runs along the panel's own
        // boundary where a rounded one would leave four gaps against the sidebar
        // and the window. The caller passes the panel's rect for the same reason —
        // see `dashboard_ui`.
        let p = ui.painter();
        p.rect_stroke(
            rect,
            egui::CornerRadius::ZERO,
            egui::Stroke::new(2.0, theme::color::SELECT),
            egui::StrokeKind::Inside,
        );
        let into = match &self.dash.nav {
            Nav::Project(id) => self
                .library
                .projects
                .get(id)
                .map(|p| format!("Drop to import into {}", p.name)),
            _ => None,
        };
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            into.unwrap_or_else(|| "Drop to import".into()),
            egui::FontId::proportional(14.0),
            theme::color::SELECT,
        );
    }

    /// The documents the current nav and filter select, sorted.
    ///
    /// **Cloned rather than borrowed**, and that is what lets the drawing code
    /// call back into `&mut self` for a menu or a rename. The alternative —
    /// holding a borrow of `library.entries` across the layout — makes every
    /// action in this file need a deferred channel rather than only the ones
    /// that mutate the list (see [`Act`]).
    fn visible_entries(&self) -> Vec<Entry> {
        let mut out = match &self.dash.nav {
            Nav::Recent => self.library.recent(),
            Nav::All => self.library.entries.clone(),
            Nav::Starred => self
                .library
                .entries
                .iter()
                .filter(|e| self.is_starred(e))
                .cloned()
                .collect(),
            // The cache, for `nav_count`'s reason — and this was the *second*
            // scan of the same folder in the same frame (§15 D728).
            Nav::Trash => self.library.trashed().to_vec(),
            Nav::Project(id) => self
                .library
                .entries
                .iter()
                .filter(|e| e.meta.project.as_deref() == Some(id.as_str()))
                .cloned()
                .collect(),
        };
        // ⚠️ **Gated on the same predicate that draws the control**, or the filter
        // goes on selecting on the entries where its dropdown is not shown — a
        // *Recent* list silently missing most of itself with nothing on screen
        // saying why. The design gates both on `showFilter`; this is that, spelled
        // once so the two cannot come apart.
        if self.filter_applies()
            && let Some(filter) = &self.dash.filter
        {
            out.retain(|e| e.meta.project.as_deref() == Some(filter.as_str()));
        }
        // ⚠️ **Gated on the same predicate that draws the control**, which is
        // `filter_applies`' rule applied to the sort — see [`OndinApp::sort_applies`]
        // for the round trip this went through.
        if self.sort_applies() {
            self.library.sort_entries(&mut out, self.dash.sort);
        }
        out
    }

    /// Whether the sort control is offered — and, because it is the same question,
    /// whether [`OndinApp::visible_entries`] applies it.
    ///
    /// **False on *Recent*, where the list is already ordered by when this machine
    /// opened each document.** That order *is* the answer *Recent* exists to give,
    /// and `Sort` has no *Last opened* to cycle back to — so a sort applied here
    /// cannot be undone, and the page loses the only thing that makes it different
    /// from *All files*.
    ///
    /// ⚠️ **This went the other way first and the round trip is the lesson.** The
    /// exemption used to be here with the control still drawn, so the button cycled
    /// its own label over a list that never moved — reported as "sort doesn't seem
    /// to do anything in Recent". Making it *work* was the wrong half to fix: the
    /// control is the thing that does not belong, and hiding it is what the
    /// maintainer asked for once the two options were on the table. **One predicate,
    /// read from both places**, is what stops the pair coming apart again — exactly
    /// as [`OndinApp::filter_applies`] does for the filter, which had this shape from
    /// the start.
    fn sort_applies(&self) -> bool {
        // §15 D381.
        self.dash.nav != Nav::Recent
    }

    /// The projects *Recent* shows cards for — the active ones that have a file
    /// in them.
    ///
    /// ⚠️ **Emptier than [`project::Projects::active`], and only here.** An empty
    /// project is a real thing with a real page, and the sidebar must list it or it
    /// could not be filled; a *card* on the landing page is a different claim — it
    /// carries a file count and a "last touched", and a row of cards reading "0
    /// files · Empty" is a picture of nothing to pick up where you left off. The
    /// sidebar is navigation, this is a summary, and the two are allowed to
    /// disagree about a project with nothing in it.
    fn recent_projects(&self) -> Vec<project::Project> {
        self.library
            .projects
            .active()
            .filter(|p| self.library.file_count(&p.id) > 0)
            .cloned()
            .collect()
    }

    // ⚠️ **No `act` parameter, and its absence is a claim** (§15 D707,
    // `[S20.1-L3-08]`). This took `_act: &mut Option<Act>` and never read it,
    // and `Act` is this file's deferral discipline — *"deferred rather than done
    // in place, because every one of these mutates the entry list the layout is
    // iterating"* — so a header handed `act` reads as a function that may emit
    // one, and the next reader has to walk all 124 lines to find that the two
    // things it *does* commit (`dash.deleting_project`, `dash.edit_project`)
    // bypass the mechanism **correctly**, opening a card mutating no list. The
    // parameter made a real distinction look like an oversight. The `_` prefix
    // silences `unused_variables` by design, so no gate could ever have said so.
    fn dashboard_header(&mut self, ui: &mut egui::Ui, count: usize) {
        let (heading, subheading) = self.heading();
        ui.horizontal(|ui| {
            // ⚠️ **The tall item is allocated first.** A `horizontal` row centres
            // each item against the row height it knows when that item is placed,
            // so a two-line heading added after the 28pt controls would sit them
            // against the wrong height.
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(&heading)
                        .size(17.0)
                        .color(theme::text::STRONG),
                );
                ui.label(
                    egui::RichText::new(subheading.replace("{n}", &count.to_string()))
                        .size(11.5)
                        .color(theme::text::DIM),
                );
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                // ⚠️ **Two plain buttons rather than the ⋮ the files have**, which
                // is the design's answer and is now a choice rather than the
                // absence of one: this was a single button because a project could
                // only be deleted, and the note here said it would become a ⋮ the
                // day a rename arrived. *Edit project* is that day, and it does not
                // — an overflow menu earns its place by hiding a *list*, and two
                // items are not one. The pair separates the two weights the way a
                // menu could not: the ordinary action carries its word, and the
                // destructive one is a glyph that has to be aimed at.
                if let Nav::Project(id) = self.dash.nav.clone() {
                    if delete_project_button(ui).clicked() && !self.dash.menu_was_up {
                        self.dash.deleting_project = Some(DeleteProject {
                            id: id.clone(),
                            keep_files: true,
                            transfer_to: None,
                        });
                    }
                    // ⚠️ **Left of *Delete project* and therefore written after
                    // it**: a right-to-left row places what it is handed first
                    // against the right margin. Sized by `ui::action_button_w`
                    // rather than by a constant, for the sort button's reason one
                    // block down — the arithmetic belongs to `ui.rs`.
                    //
                    // ⚠️ **Drawn only when the project *resolves*, which is
                    // `new_file_card`'s rule (§15 D376) and not *Delete project*'s.**
                    // A card seeded from a lookup that failed would be an empty form
                    // over no project; a button that opens nothing is worse than one
                    // that is not there. The two differ on purpose: deleting a nav
                    // that names a project `projects.json` no longer has is still a
                    // thing a user may want to do.
                    if let Some(p) = self.library.projects.get(&id).cloned()
                        && ui::action_button(
                            ui,
                            icon::PENCIL_SIMPLE,
                            "Edit project",
                            FieldButton::Off,
                            egui::vec2(
                                ui::action_button_w(ui.ctx(), "Edit project"),
                                HEADER_CONTROL_H,
                            ),
                        )
                        .clicked()
                        && !self.dash.menu_was_up
                    {
                        self.dash.edit_project = Some(EditProject::of(&p));
                    }
                }
                // **A segmented track, not two icon buttons** (§15 D367). The
                // design draws one recessed group with the live cell raised, which
                // is the same control the inspector's tabs and the type panel's
                // mode strips are — and two separate buttons say "two independent
                // toggles", which grid-and-list is not: one of them is always on.
                let selected = usize::from(self.dash.list_view);
                let picked = ui::segmented(
                    ui,
                    VIEW_TRACK_W,
                    VIEW_CELL_H,
                    2,
                    selected,
                    |p, i, rect, on| {
                        let glyph = if i == 0 {
                            icon::SQUARES_FOUR
                        } else {
                            icon::ROWS
                        };
                        ui::segment_glyph(p, rect, glyph, on);
                    },
                );
                // **This writes no preference** (§15 D749). It called
                // `remember_dashboard`, which rewrote *all three* of
                // `dashboard_page`, `dashboard_list_view` and `dashboard_sort`
                // from the current state — so pressing the view toggle while
                // standing in the Trash silently set *Default page on open* to
                // **Trash**, a value the Settings picker refuses to offer in a
                // comment saying why. The maintainer's ruling: those three are
                // *"only settable on Settings, not on navigation or any other
                // action in the dashboard. The whole purpose is what to open when
                // the app loads, not to remember your last page visit."*
                if let Some(i) = picked {
                    self.dash.list_view = i == 1;
                }
                // One button that cycles, as the design has it, rather than a
                // dropdown: four options is short enough that stepping through
                // them is faster than opening a menu to pick one.
                //
                // ⚠️ **Sized to the *longest* label, not to the one showing.** The
                // control changes its own word on every press, and a button that
                // measured the current word would shove the view toggle sideways
                // by the difference between *Name* and *Date created* each time.
                if self.sort_applies()
                    && ui::action_button(
                        ui,
                        icon::ARROWS_DOWN_UP,
                        self.dash.sort.label(),
                        FieldButton::Off,
                        egui::vec2(sort_button_w(ui.ctx()), HEADER_CONTROL_H),
                    )
                    .on_hover_text("Sort — click to cycle")
                    .clicked()
                    && !self.dash.menu_was_up
                {
                    // Session state, not a preference — see the view toggle above
                    // and §15 D749.
                    self.dash.sort = self.dash.sort.next();
                }
                // Leftmost of the cluster, which is where the design puts it — and
                // last, because a right-to-left row places what it is given first
                // at the right margin.
                self.project_filter(ui);
            });
        });
    }

    /// Whether the project filter is offered — and, because it is the same
    /// question, whether [`OndinApp::visible_entries`] applies it.
    ///
    /// ⚠️ **One predicate, read from both places.** The design's `showFilter`
    /// gates the control *and* the `base.filter(…)` behind it; splitting them is
    /// how a filter set on *All files* comes to be quietly selecting on *Recent*,
    /// where there is no control to unset it with.
    ///
    /// **Not on *Recent*, and not inside a project.** Recent is a landing page
    /// rather than a list of everything (see [`OndinApp::dashboard_body`]), and a
    /// project's own page has already answered this question — a filter there
    /// would be a second navigation for a decision already made.
    ///
    /// **And not with no projects to filter by**, which is this file's addition
    /// rather than the design's: the dropdown would hold one row saying *All
    /// projects*, which is a control that cannot change anything.
    ///
    /// ⚠️ **`active`, not the whole list** — archiving is a second way to reach
    /// that empty dropdown, and the predicate has to agree with what the menu
    /// actually offers or the button appears over a list of one.
    fn filter_applies(&self) -> bool {
        matches!(self.dash.nav, Nav::All | Nav::Starred | Nav::Trash)
            && self.library.projects.active().next().is_some()
    }

    /// *All projects ⌄* — the body header's project filter
    /// (`design/Dashboard.dc.html`, §15 D369).
    ///
    /// **A button and an `Area`, not a `ComboBox`**, because the closed state
    /// carries a colour dot and the rows carry a dot and a tick — none of which a
    /// `selected_text` can hold, since that is one `WidgetText`. It is the shape
    /// [`OndinApp::file_menu_popup`] already uses on this screen.
    ///
    /// The ground is [`ui::button_face`]'s, shared rather than copied: this sits
    /// against a [`ui::action_button`] and a [`ui::label_button`] in one row, and a
    /// hairline half a shade off reads as two different kinds of control.
    /// [`FieldButton::Set`] is the accent border — the design lights it while the
    /// menu is open, and this also lights it while a filter is *in force*, which is
    /// the state that matters once the menu has closed again.
    fn project_filter(&mut self, ui: &mut egui::Ui) {
        // ⚠️ **A filter naming a project that no longer exists is no filter**, and
        // this runs *above* the early return on purpose. `projects.json` is rebuilt
        // on every refresh, including refreshes caused by something else writing
        // it, so the id in hand can stop resolving between frames — and the case
        // where it stops resolving because the last project was deleted is exactly
        // the case where the control below is not drawn. Put after the return, the
        // degrade would never run on the nav that needs it most. `Nav::Project`
        // carries an id rather than an index for the same reason.
        //
        // The header is drawn on every nav (`dashboard_body`), so this is reached
        // whatever the screen is showing.
        let current = self
            .dash
            .filter
            .as_ref()
            .and_then(|id| self.library.projects.projects.iter().find(|p| &p.id == id))
            .map(|p| (p.name.clone(), swatch(&p.color)));
        if self.dash.filter.is_some() && current.is_none() {
            self.dash.filter = None;
        }
        if !self.filter_applies() {
            // ⚠️ Closed as well as hidden, or the dropdown survives a nav change
            // and reappears floating over the next screen.
            self.dash.filter_open = false;
            return;
        }
        let (label, dot) = current.unwrap_or_else(|| (ALL_PROJECTS.to_owned(), theme::text::DIM));

        let w = FILTER_W;
        let (rect, resp, fg) = ui::button_face(
            ui,
            egui::vec2(w, HEADER_CONTROL_H),
            if self.dash.filter_open || self.dash.filter.is_some() {
                FieldButton::Set
            } else {
                FieldButton::Off
            },
        );
        let p = ui.painter();
        p.rect_filled(
            egui::Rect::from_center_size(
                egui::pos2(rect.left() + FILTER_PAD_X + DOT / 2.0, rect.center().y),
                egui::vec2(DOT, DOT),
            ),
            egui::CornerRadius::same(2),
            dot,
        );
        p.text(
            egui::pos2(rect.left() + FILTER_PAD_X + DOT + 7.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            elide(
                ui,
                &label,
                11.5,
                w - FILTER_PAD_X * 2.0 - DOT - 7.0 - 12.0 - 5.0,
            ),
            egui::FontId::proportional(11.5),
            fg,
        );
        ui.painter().text(
            egui::pos2(rect.right() - FILTER_PAD_X, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            icon::CARET_DOWN,
            theme::icon_font(12.0),
            theme::text::DIM,
        );
        if resp.clicked() {
            self.dash.filter_open = !self.dash.filter_open;
        }
        if self.dash.filter_open {
            self.project_filter_menu(ui, rect);
        }
    }

    /// The filter's dropdown: *All projects* and then every project, each with its
    /// swatch and a tick on the one in force.
    fn project_filter_menu(&mut self, ui: &mut egui::Ui, anchor: egui::Rect) {
        let mut rows: Vec<(Option<String>, String, egui::Color32)> =
            vec![(None, ALL_PROJECTS.to_owned(), theme::text::DIM)];
        rows.extend(
            self.library
                .projects
                .active()
                .map(|p| (Some(p.id.clone()), p.name.clone(), swatch(&p.color))),
        );

        let w = FILTER_MENU_W;
        // The shell is [`anchored_menu`]'s; what is drawn per row is this menu's
        // own — a colour dot and a tick (§15 D729).
        let current = self.dash.filter.clone();
        let out = anchored_menu(
            ui,
            egui::Id::new("library-filter-menu"),
            anchor,
            w,
            rows.len(),
            |p, i, row| {
                let (id, name, dot) = &rows[i];
                let on = &current == id;
                p.rect_filled(
                    egui::Rect::from_center_size(
                        egui::pos2(row.left() + 8.0 + DOT / 2.0, row.center().y),
                        egui::vec2(DOT, DOT),
                    ),
                    egui::CornerRadius::same(2),
                    *dot,
                );
                p.text(
                    egui::pos2(row.left() + 8.0 + DOT + 8.0, row.center().y),
                    egui::Align2::LEFT_CENTER,
                    // ⚠️ **`elide` takes a `&Context` now, not a `&Ui`** — it only
                    // ever wanted `ui.ctx()`, and `paint` is handed a `&Painter`
                    // for the reason `ui::segmented_enabled`'s is: a row painter
                    // must not be able to allocate. Narrowing the parameter was
                    // cheaper than a second spelling of the same function.
                    //
                    // ⚠️ **The other four call sites still pass a `&Ui` and
                    // compile**, because `egui::Ui` is `Deref<Target = Context>`.
                    // So the narrowing bought this caller and *did not* force the
                    // others to say what they mean — worth knowing before reading
                    // `elide(ui, …)` as evidence that a site was reviewed.
                    elide(p.ctx(), name, 11.5, w - 60.0),
                    egui::FontId::proportional(11.5),
                    if on { color::TEXT } else { theme::text::MUTED },
                );
                // The tick's column is reserved either way, for `ui::menu_check`'s
                // reason: a mark that shifts the rows around it is read as the rows
                // moving rather than as the mark arriving.
                if on {
                    p.text(
                        egui::pos2(row.right() - 10.0, row.center().y),
                        egui::Align2::RIGHT_CENTER,
                        icon::CHECK,
                        theme::icon_font(13.0),
                        color::ACCENT,
                    );
                }
            },
        );

        if let Some(i) = out.chosen {
            self.dash.filter = rows.into_iter().nth(i).expect("index from the list").0;
            self.dash.filter_open = false;
        } else if out.dismissed {
            self.dash.filter_open = false;
        }
    }

    /// The title and the line under it, for the current nav.
    ///
    /// `{n}` in the subheading is filled with the visible count by the caller,
    /// which is what keeps this function from needing the entry list.
    fn heading(&self) -> (String, String) {
        let nav = &self.dash.nav;
        match nav {
            Nav::Recent => (nav.label().into(), "Picked up where you left off".into()),
            Nav::All | Nav::Starred => (nav.label().into(), "{n} files".into()),
            Nav::Trash => (
                nav.label().into(),
                match self.prefs.trash_keep_days {
                    // The retention line is the only thing on this screen that
                    // says a delete is not forever, so the zero case has to say
                    // the opposite rather than read as "kept for 0 days".
                    0 => "{n} files · deleted straight away".into(),
                    d => format!("{{n}} files · kept for {d} days"),
                },
            ),
            Nav::Project(id) => match self.library.projects.get(id) {
                Some(p) => {
                    let touched = self
                        .library
                        .project_touched(id)
                        .map(|t| clock::relative_label(clock::since(t)).to_lowercase())
                        .unwrap_or_else(|| "never".into());
                    (p.name.clone(), format!("{{n}} files · updated {touched}"))
                }
                None => ("Project".into(), "{n} files".into()),
            },
        }
    }

    fn empty_state(&mut self, ui: &mut egui::Ui) {
        ui.add_space(60.0);
        ui.vertical_centered(|ui| {
            // ⚠️ **The missing folder answers every nav, and answers first.** The
            // four sentences below are all about a *filter* — nothing starred,
            // nothing opened, an empty trash — and each of them is a lie in this
            // state, where the list is empty because the library cannot be seen
            // rather than because it holds nothing. This is the one case where
            // "no files here" is the app failing to look
            // (`library::state::Library::root_unavailable`).
            let unavailable = self.library.root_unavailable;
            let (glyph, line) = match &self.dash.nav {
                _ if unavailable => (
                    icon::FOLDER_SIMPLE,
                    "This library's folder isn't available. Check the base folder in Settings.",
                ),
                Nav::Recent => (
                    icon::CLOCK_COUNTER_CLOCKWISE,
                    "Nothing opened on this computer yet.",
                ),
                Nav::Starred => (icon::STAR, "Star a file and it will show up here."),
                Nav::Trash => (icon::TRASH_SIMPLE, "The trash is empty."),
                _ => (icon::FILES, "No files here yet."),
            };
            ui.label(
                egui::RichText::new(glyph)
                    .font(theme::icon_font(28.0))
                    // **Warned rather than faint**, which is the one visual
                    // difference between this state and the four below it: those
                    // are ordinary and this is something being wrong.
                    .color(if unavailable {
                        color::WARN
                    } else {
                        theme::text::FAINT
                    }),
            );
            ui.add_space(8.0);
            ui.label(egui::RichText::new(line).size(12.5).color(theme::text::DIM));
            // ⚠️ **No path under the sentence, and there used to be one on *All
            // files* and inside a project** (§15 D382, undoing D365's). The argument for it was
            // that an empty library usually means the base folder points somewhere
            // else, and the path is the only thing that says so — which is true of
            // the *cause* and wrong about where the answer belongs. A raw
            // `C:\Users\…\.ondin` is the least readable thing on the screen, it is
            // in the Settings card where it can also be *changed*, and it is
            // printed under a sentence that is usually right: most empty lists are
            // empty because the library is new.
        });
    }

    /// The row of project cards at the top of *Recent*.
    fn project_cards(&mut self, ui: &mut egui::Ui) {
        section_label(ui, "PROJECTS");
        ui.add_space(6.0);
        let gap = 14.0;
        let card_w =
            ((ui.available_width() - gap * (GRID_COLS as f32 - 1.0)) / GRID_COLS as f32).max(120.0);
        let projects = self.recent_projects();
        let mut go: Option<String> = None;
        for row in projects.chunks(GRID_COLS) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for p in row {
                    let count = self.library.file_count(&p.id);
                    let touched = self
                        .library
                        .project_touched(&p.id)
                        .map(|t| clock::relative_label(clock::since(t)))
                        .unwrap_or_else(|| "Empty".into());
                    // R4's sixth door (§15 D558) — the sidebar row's failure on a
                    // card, on *Recent*.
                    if self.project_card(ui, p, card_w, count, &touched) && !self.dash.menu_was_up {
                        go = Some(p.id.clone());
                    }
                }
            });
            ui.add_space(gap);
        }
        if let Some(id) = go {
            self.dash.nav = Nav::Project(id);
        }
    }

    /// One project card: **a mosaic of its files, then its name, then its two
    /// facts** — the design's shape (§15 D381). Returns whether it was clicked.
    ///
    /// ⚠️ **The coloured band across the top is gone with the mosaic's arrival.**
    /// It was this file's own addition, argued as "the dot alone is 8px of signal
    /// on a 170px card" — which was true of a card that was a name and a count. A
    /// card carrying five of the project's covers has all the identity it needs,
    /// and a wash of the project's hue over the top of the pictures would be
    /// competing with them rather than helping.
    fn project_card(
        &mut self,
        ui: &mut egui::Ui,
        p: &project::Project,
        w: f32,
        count: usize,
        touched: &str,
    ) -> bool {
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(w, PROJECT_CARD_H), egui::Sense::click());
        // No cursor change — the card lights on hover, and that is the affordance
        // every other clickable thing on this screen uses (§9.2).
        let lit = resp.hovered();
        let dot = swatch(&p.color);
        let painter = ui.painter();
        painter.rect_filled(rect, egui::CornerRadius::same(9), color::CARD);
        painter.rect_stroke(
            rect,
            egui::CornerRadius::same(9),
            egui::Stroke::new(
                1.0,
                if lit {
                    color::ACCENT_700
                } else {
                    // The card's ground, not a field's — see `color::CARD_BORDER`.
                    color::CARD_BORDER
                },
            ),
            egui::StrokeKind::Inside,
        );
        let mosaic = egui::Rect::from_min_size(
            rect.min + egui::Vec2::splat(PROJECT_PAD),
            egui::vec2(w - PROJECT_PAD * 2.0, MOSAIC_H),
        );
        self.project_mosaic(ui, &p.id, mosaic, dot);

        let name_y = mosaic.bottom() + PROJECT_ROW_GAP + 8.0;
        let painter = ui.painter();
        painter.rect_filled(
            egui::Rect::from_center_size(
                egui::pos2(rect.left() + PROJECT_PAD + DOT / 2.0, name_y),
                egui::Vec2::splat(DOT),
            ),
            egui::CornerRadius::same(2),
            dot,
        );
        painter.text(
            egui::pos2(rect.left() + PROJECT_PAD + DOT + 7.0, name_y),
            egui::Align2::LEFT_CENTER,
            elide(ui, &p.name, 12.5, w - PROJECT_PAD * 2.0 - DOT - 7.0),
            egui::FontId::proportional(12.5),
            theme::text::STRONG,
        );
        ui.painter().text(
            egui::pos2(rect.left() + PROJECT_PAD, rect.bottom() - PROJECT_PAD - 6.0),
            egui::Align2::LEFT_CENTER,
            format!("{count} file(s) · {touched}"),
            egui::FontId::proportional(11.0),
            theme::text::FAINT,
        );
        resp.clicked()
    }

    /// The project card's picture: up to [`MOSAIC_MAX`] of its files' covers, laid
    /// out the way `design/Dashboard.dc.html`'s `MOSAICS` table lays out its four.
    ///
    /// **The design hand-authors one layout per project; this is the rule behind
    /// them.** The newest file takes a column of its own, full height; the rest fill
    /// the columns after it two at a time, top then bottom; and ⚠️ **a column that
    /// ends up holding only one runs full height as well**, which is what makes the
    /// two-file case two tall cells side by side rather than one tall and one
    /// floating. Those three sentences reproduce all four of the design's layouts
    /// exactly — one, two, three and five cells.
    ///
    /// ⚠️ **Newest first, by `Entry::modified`** — the same order *Recent* itself is
    /// about — so the big cell is the file the user most recently touched rather
    /// than whichever the scan happened to list first. The arithmetic is the free
    /// [`mosaic_cells`], which is where the layout rule is stated and tested.
    fn project_mosaic(
        &mut self,
        ui: &mut egui::Ui,
        id: &str,
        rect: egui::Rect,
        dot: egui::Color32,
    ) {
        let mut files: Vec<Entry> = self
            .library
            .entries
            .iter()
            .filter(|e| e.meta.project.as_deref() == Some(id))
            .cloned()
            .collect();
        files.sort_by_key(|e| std::cmp::Reverse(e.modified));
        files.truncate(MOSAIC_MAX);
        if files.is_empty() {
            return;
        }
        for (entry, cell) in files.iter().zip(mosaic_cells(files.len(), rect)) {
            ui.painter()
                .rect_filled(cell, egui::CornerRadius::same(4), dot.gamma_multiply(0.14));
            // Fitted rather than filled, and inset, for `file_card`'s reasons: a
            // cover is a whole document at its own aspect ratio, and a picture drawn
            // to a rounded plate's corners shows square ones over them.
            if let Some(texture) = self.covers.get(ui.ctx(), entry).cloned() {
                let size = texture.size_vec2();
                let room = cell.shrink(4.0);
                let scale = (room.width() / size.x).min(room.height() / size.y).min(1.0);
                ui.painter().image(
                    texture.id(),
                    egui::Rect::from_center_size(room.center(), size * scale),
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
        }
    }

    /// The project a *New file* card can promise to file into, by name.
    ///
    /// ⚠️ **Gated on the project *resolving*, not merely on the nav being one.**
    /// [`OndinApp::new_library_document`] files into the project it can look up,
    /// so a nav holding an id `projects.json` no longer has would leave the new
    /// document unfiled while the card said which project it was going into — the
    /// one state a shortcut like this must not reach. Nothing else on the screen
    /// notices a dangling id, because everything else only *filters* by it.
    ///
    /// ⚠️ **And gated on the folder being there at all**, for the same reason one
    /// step further out: the card is a promise about where a document will land,
    /// and while the library is missing there is nowhere for it to land — the act
    /// behind it refuses ([`OndinApp::library_writable`]). Returning `None` here
    /// is also what lets the empty state through in a project's nav, which is the
    /// only nav D376 had taken the empty state away from.
    /// ⚠️ **And gated on the project not being *archived*** (§15 D557,
    /// `[S20.3-L1-03]`), which is the third gate and arrived last. `get` resolves
    /// an archived project by design — a document filed in one must still show its
    /// name and colour — so this card stayed on screen after *Edit project ▸
    /// Archive*, and clicking it filed a brand-new document straight into the
    /// archived project. `move_modal`'s own ⚠️ states the rule this breaks: *"a
    /// document cannot be filed into an archived project without unarchiving it
    /// first."* The ⋮ on that document then offered **nothing** under *Move to
    /// project*, because the project it was in is archived.
    ///
    /// The question is *"may a new document land here"*, which is neither a list
    /// nor a lookup, so it has its own name: [`project::Projects::destination`].
    ///
    /// `pub(crate)` so D557's test can assert the card and the act *together* —
    /// they are two functions that have to agree, and the test lives beside the
    /// rest of the library wiring in `app.rs`.
    pub(crate) fn new_file_into(&self) -> Option<String> {
        if self.library.root_unavailable {
            return None;
        }
        match &self.dash.nav {
            Nav::Project(id) => self
                .library
                .projects
                .destination(id)
                .map(|p| p.name.clone()),
            _ => None,
        }
    }

    /// The grid of file cards, with the dashed *New file* card after the last one
    /// when there is a project to put it in.
    ///
    /// ⚠️ **Indices rather than `chunks`, because the card is a cell.** A
    /// `chunks(GRID_COLS)` loop can only put it on a row of its own, which is
    /// wrong exactly when it matters — a project holding one file would show its
    /// card and then a whole empty row with the *New file* card alone at the left
    /// of it. Counting slots is what lets the card finish a short last row.
    fn file_grid(
        &mut self,
        ui: &mut egui::Ui,
        entries: &[Entry],
        new_file: Option<&str>,
        act: &mut Option<Act>,
    ) {
        let gap = 14.0;
        let card_w =
            ((ui.available_width() - gap * (GRID_COLS as f32 - 1.0)) / GRID_COLS as f32).max(120.0);
        let slots = entries.len() + usize::from(new_file.is_some());
        for start in (0..slots).step_by(GRID_COLS) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for i in start..(start + GRID_COLS).min(slots) {
                    match entries.get(i) {
                        Some(entry) => self.file_card(ui, entry, card_w, act),
                        // The one slot past the end, which only exists when
                        // `new_file` is `Some` — so the `unwrap_or_default` is
                        // unreachable rather than a fallback with a meaning.
                        None => {
                            // ⚠️ **R4's third door, and the worst of them** (§15
                            // D558): a click spent dismissing a menu that landed
                            // here did not merely open a document, it **created**
                            // one. `arch-scribe` found this and the sidebar's rows
                            // after the first version of D558's fix gated
                            // `pick_or_open` alone.
                            if self.new_file_card(ui, card_w, new_file.unwrap_or_default())
                                && !self.dash.menu_was_up
                            {
                                *act = Some(Act::NewFile);
                            }
                        }
                    }
                }
            });
            ui.add_space(gap);
        }
    }

    /// The dashed *New file in {project}* card at the end of a project's grid
    /// (`design/Dashboard.dc.html`, §15 D376).
    ///
    /// **A shortcut rather than a capability**: the sidebar's *New file* button
    /// already files into the project being viewed
    /// ([`OndinApp::new_library_document`] reads [`Nav::Project`]), and this is the
    /// same [`Act::NewFile`] offered where the eye already is. Which is also why
    /// there is no card on *Recent*, *All files*, *Starred* or *Trash*: a document
    /// created from one of those has no project to land in, and a card promising a
    /// destination the screen cannot name is worse than no card.
    ///
    /// ⚠️ **The arrows cannot reach it**, and that is deliberate rather than
    /// missed: [`OndinApp::dashboard_keys`] steps through `entries`, which is
    /// documents, and a selection that could land on a *verb* would make `Delete`
    /// and `Enter` mean two different things depending on where it stopped. The
    /// keyboard's route to the same act is `Ctrl+N`, which is bound on this screen
    /// and in the editor (`shortcuts.md` §8, §8a) and reads the same nav this card
    /// is drawn from — so the card is the pointer's spelling of it rather than the
    /// only door.
    ///
    /// Returns whether it was clicked.
    fn new_file_card(&self, ui: &mut egui::Ui, w: f32, into: &str) -> bool {
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(w, CARD_THUMB_H + CARD_CAPTION_H),
            egui::Sense::click(),
        );
        let lit = resp.hovered();
        let ink = if lit {
            color::ACCENT_200
        } else {
            theme::text::FAINT
        };
        let p = ui.painter();
        // A shade under [`color::CARD`] rather than equal to it — the design's
        // `text 12%` against a file card's `15%` — so the slot reads as the place a
        // card would go rather than as a card that failed to draw.
        p.rect_filled(
            rect,
            egui::CornerRadius::same(GHOST_R as u8),
            theme::color::text_a(8),
        );
        // ⚠️ **The dashes follow the same corner the fill is rounded to, and used
        // not to.** `rect_stroke` has no dash pattern, so this is a polyline — and
        // the polyline was the four corner *points*, which draws square corners
        // over a rounded fill: the ground curved away underneath and the dashes cut
        // the corner off. Reported as "the filled shape has a radius but the dashed
        // border doesn't follow it", which is exactly what it was.
        //
        // The closing edge is still owed explicitly (`rounded_rect_path` repeats
        // the first point), the same trap `canvas::image_edit_shapes` and
        // `draw_text_line_boxes` both state: a box missing one edge reads as a
        // rendering fault rather than as a box.
        let closed = rounded_rect_path(rect.shrink(0.5), GHOST_R, 6);
        let stroke = egui::Stroke::new(
            1.0,
            if lit {
                color::ACCENT_700
            } else {
                // A card's edge, dashed — `color::CARD_BORDER`, so the empty slot
                // and the cards beside it draw one line rather than two weights.
                color::CARD_BORDER
            },
        );
        for shape in egui::Shape::dashed_line(&closed, stroke, 5.0, 4.0) {
            p.add(shape);
        }
        p.text(
            rect.center() - egui::vec2(0.0, 13.0),
            egui::Align2::CENTER_CENTER,
            icon::PLUS,
            theme::icon_font(18.0),
            ink,
        );
        p.text(
            rect.center() + egui::vec2(0.0, 12.0),
            egui::Align2::CENTER_CENTER,
            elide(ui, &format!("New file in {into}"), 11.5, w - 20.0),
            egui::FontId::proportional(11.5),
            ink,
        );
        resp.clicked()
    }

    /// The one chip a card or a row wears, or none.
    ///
    /// 🚨 **`Unreadable` was computed on every scan and drawn nowhere** (§15 D613,
    /// `[S1.3-L3-05]`). `scan::Entry::unread` had **no reader outside
    /// `library/`** — `dashboard.rs` did not contain the string once — so a
    /// `.ondin` this build cannot parse got a card carrying a de-slugged filename,
    /// a created date, a hover menu and *Open*, identical in every pixel to a
    /// healthy document, and the only feedback the user ever got was the status
    /// line saying *"Load error: …"* after clicking it. `Entry` derives
    /// `PartialEq`, which is `CLAUDE.md`'s measured mechanism for `dead_code`
    /// staying silent on a written-never-read field — and the field had a test
    /// asserting it, so the suite read as covering a feature that did not exist.
    ///
    /// **Two sources, because neither is sufficient.** `unread` is the scan's
    /// cheap answer and is written only when the first 4 KB will not parse;
    /// `Covers::unreadable` is `io::load`'s answer over the whole file, which the
    /// cover render was already computing and throwing away. A document written by
    /// a newer build is well-formed JSON the loader refuses, so the probe reports
    /// it healthy and only the cover knows — and a document whose cover has not
    /// been attempted yet is not yet known to be broken, so the flag is what
    /// carries the early passes.
    ///
    /// ⚠️ **One mark, most severe first, rather than two chips.** A conflict copy
    /// of a broken document is both, and the corner they would share is one
    /// corner; *this file will not open* is also the thing the user has to act on
    /// first, because until it is fixed there is no choosing between two copies.
    fn mark_of(&self, entry: &Entry) -> Option<Mark> {
        if entry.unread || self.covers.unreadable(entry) {
            return Some(Mark::Unreadable);
        }
        self.library
            .conflict(entry)
            .is_some()
            .then_some(Mark::Conflict)
    }

    fn file_card(&mut self, ui: &mut egui::Ui, entry: &Entry, w: f32, act: &mut Option<Act>) {
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(w, CARD_THUMB_H + CARD_CAPTION_H),
            egui::Sense::click(),
        );
        let lit = resp.hovered();
        let picked = self.dash.selected.as_deref() == Some(entry.path.as_path());
        self.follow_selection(ui, picked, rect);
        let dot = self
            .library
            .project_of(entry)
            .map(|p| swatch(&p.color))
            .unwrap_or(theme::text::FAINT);
        let thumb = egui::Rect::from_min_size(rect.min, egui::vec2(w, CARD_THUMB_H));
        let p = ui.painter();
        p.rect_filled(rect, egui::CornerRadius::same(9), color::CARD);
        // ⚠️ **Three tiers, and the top one is now the *keyboard's* cursor rather
        // than a selection** (§15 D381): a full accent border where hover is the
        // muted `ACCENT_700`. The pointer cannot reach it any more — a click opens
        // — so the two tiers a pointer sees are resting and hover, and the third
        // exists for the arrows. Still three rather than two, for the reason it
        // always was: a hovered card and a cursored one must not be one picture,
        // and now they can genuinely coexist, since the pointer can be anywhere
        // while the cursor sits where the keyboard left it.
        p.rect_stroke(
            rect,
            egui::CornerRadius::same(9),
            egui::Stroke::new(
                1.0,
                match (picked, lit) {
                    (true, _) => color::ACCENT,
                    (false, true) => color::ACCENT_700,
                    // A card's ground — see `color::CARD_BORDER`.
                    (false, false) => color::CARD_BORDER,
                },
            ),
            egui::StrokeKind::Inside,
        );
        // The plate the cover sits on, which is also what a document with no
        // cover shows: an empty file, one that will not load, or one whose cover
        // has not been rendered yet (`library::cover::Covers::get` does not
        // distinguish the three, and neither does this).
        let plate_radius = egui::CornerRadius {
            nw: 8,
            ne: 8,
            sw: 0,
            se: 0,
        };
        p.rect_filled(thumb.shrink(1.0), plate_radius, dot.gamma_multiply(0.14));
        let cover = self.covers.get(ui.ctx(), entry).cloned();
        match cover {
            Some(texture) => {
                // ⚠️ **Fitted inside the plate, never filled to it.** A cover is
                // a whole document at its own aspect ratio; cropping it to a
                // 170×116 card would cut the top off a tall mobile screen, which
                // is precisely the document whose shape identifies it. Insetting
                // rather than bleeding to the edges is the other half — a
                // rectangle drawn to the plate's corners would show square
                // corners over the plate's rounded ones.
                let size = texture.size_vec2();
                let room = thumb.shrink(10.0);
                let scale = (room.width() / size.x).min(room.height() / size.y).min(1.0);
                let at = egui::Rect::from_center_size(room.center(), size * scale);
                ui.painter().image(
                    texture.id(),
                    at,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            None => {
                ui.painter().text(
                    thumb.center(),
                    egui::Align2::CENTER_CENTER,
                    icon::FILES,
                    theme::icon_font(22.0),
                    theme::color::text_a(40),
                );
            }
        }
        // **Bottom-left of the cover, opposite the star** (§15 D375). It shares
        // the star's argument for being on the picture rather than in the caption
        // — a grid is scanned, not read — and takes the other corner because the
        // two are unrelated facts and a document can wear both.
        if let Some(mark) = self.mark_of(entry) {
            mark_chip(
                ui,
                egui::pos2(thumb.left() + 8.0, thumb.bottom() - 8.0),
                egui::Align2::LEFT_BOTTOM,
                mark,
            );
        }
        // **The star, on the cover rather than in the caption** — the design's
        // `top:7px;right:7px` over the thumbnail. It goes here and not beside the
        // name because a starred document has to be findable by *scanning* the
        // grid, and the caption row is already carrying a name, a project dot and
        // a ⋮; a mark in the picture is read at a glance and one in a row of text
        // is read one card at a time.
        //
        // ⚠️ **An outline star, not a filled one, because the bundled Phosphor is
        // Regular weight only** — there is no `ph-fill` face in
        // `assets/fonts/Phosphor.ttf`, so the design's solid glyph is not
        // available without shipping a second font for one mark. What carries the
        // state is the gold; an unstarred card draws nothing at all, so there is
        // no second star for this one to be confused with.
        if self.is_starred(entry) {
            let at = egui::pos2(thumb.right() - 14.0, thumb.top() + 14.0);
            let p = ui.painter();
            // The design's `drop-shadow(0 1px 2px …)`, spelled as a second pass
            // one point down: a cover is arbitrary artwork, and a gold star on a
            // pale drawing has nothing to sit against.
            p.add(star_shape(
                at + egui::vec2(0.0, 1.0),
                STAR_R,
                egui::Color32::from_black_alpha(110),
            ));
            p.add(star_shape(at, STAR_R, theme::color::STAR));
        }
        let name_box = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 8.0, thumb.bottom() + 5.0),
            egui::vec2(w - 42.0, 20.0),
        );
        if !self.rename_field(ui, entry, name_box, 12.5, act) {
            ui.painter().text(
                egui::pos2(rect.left() + 12.0, thumb.bottom() + 15.0),
                egui::Align2::LEFT_CENTER,
                elide(ui, &entry.display_name(), 12.5, w - 46.0),
                egui::FontId::proportional(12.5),
                theme::text::STRONG,
            );
        }
        let p = ui.painter();
        p.text(
            egui::pos2(rect.left() + 12.0, thumb.bottom() + 31.0),
            egui::Align2::LEFT_CENTER,
            clock::relative_label(clock::since(entry.modified)),
            egui::FontId::proportional(11.0),
            theme::text::FAINT,
        );

        self.pick_or_open(&resp, entry, act);
        self.file_menu_button(
            ui,
            entry,
            egui::pos2(rect.right() - 20.0, thumb.bottom() + 22.0),
            act,
        );
    }

    /// What a click on a card or a row means: **it opens the document** (§15 D381).
    ///
    /// ⚠️ **A single click no longer selects, and the double click is gone**, which
    /// reverses most of D372 six days after it landed. The maintainer's words are
    /// the argument: *"the single click selection of files is meaningless. That was
    /// copied from the design, but no need for it."* — and they are right, because
    /// nothing on this screen ever *acted* on a pointer-made selection. Every action
    /// a card offers is on its own ⋮ menu, so the selection was a state the user
    /// could enter, could see, and could do nothing with; opening needed a second
    /// click on top of it.
    ///
    /// **`Response::double_clicked` is not read anywhere now**, which is the part to
    /// be deliberate about: the first click of a double already opened the document
    /// and the editor is up before the second lands, so there is no second gesture
    /// to give a meaning to.
    ///
    /// ⚠️ **The keyboard's cursor survives all of this** (§15 D374) — arrows move it,
    /// `Enter` opens it, `Delete` asks, `Escape` clears it, and the outline draws for
    /// it. It is not the same thing: the pointer now *acts* where it used to point,
    /// and the outline is a keyboard cursor rather than a selection, so
    /// `DashboardState::selected` is only ever written by
    /// [`OndinApp::dashboard_keys`].
    fn pick_or_open(&mut self, resp: &egui::Response, entry: &Entry, act: &mut Option<Act>) {
        // **A click that dismisses a menu is spent doing so** (§15 D558,
        // `context-menus.md` §0 R4, `[S20.2-L1-02]`). `canvas_ui` states this rule
        // and obeys it — *"the menu was aimed at something, and a click to get rid
        // of it that retargets the selection turns dismissal into an edit"* — and
        // this screen did not: the ⋮ menu and the *All projects ⌄* dropdown are
        // `egui::Area`s covering only their own rects, and their dismissals read
        // `any_click()` without consuming it. So clicking another card to get rid
        // of the menu closed it **and opened that document** — the library gone and
        // the editor up on a file the user only clicked to dismiss a popup, which
        // on this screen is a whole navigation rather than a changed selection.
        //
        // ⚠️ **Here rather than at the two dismissals**, which is `canvas_ui`'s
        // shape: a dismissal cannot consume a click the card has already turned
        // into an `Act::Open`. Gating the act is the only order that works. A
        // click on the ⋮ itself is unaffected — that is a different `Response`,
        // and both dismissals already exempt their anchor.
        //
        // ⚠️ **`menu_was_up`, latched at the top of the frame, and *not*
        // `library_menu_open()`.** The dismissal runs from inside the anchor's own
        // card, so the live flag is already `false` for every card drawn after it
        // — which made the first version of this fix work or not depending on
        // where the anchor sorted. Its own test caught that.
        if self.dash.menu_was_up {
            return;
        }
        if resp.clicked() && self.can_open() {
            *act = Some(Act::Open(entry.path.clone()));
        }
    }

    /// Whether one of the library's two floating menus is on screen.
    ///
    /// The ⋮ file menu and the *All projects ⌄* dropdown. **Not the modals** —
    /// those are `egui::Modal`s and stop the pointer themselves, which is exactly
    /// why these two need a predicate and they do not.
    ///
    /// ⚠️ **Now the keyboard's question too, and it used to be deliberately not**
    /// (§15 D578). This paragraph read *"a pointer question, and deliberately not
    /// folded into `library_keys_are_free`"* — which stays true of that function,
    /// since it is about *modals* — but [`OndinApp::dashboard_keys`] asks **this**
    /// predicate for its first early return, so the eight pointer-side doors below
    /// and the keymap cannot disagree about what is on screen.
    ///
    /// ⚠️ **This said the two menus "read no key at all" and that was false for the
    /// ⋮ popup.** [`OndinApp::dashboard_keys`] returns early while `menu_for` is
    /// set and answers `Escape` on the way out — D374 built it,
    /// `the_library_answers_its_own_keys` asserts it, and §9.5 states it.
    /// `arch-scribe` caught the claim.
    ///
    /// 🚨 **It then said the *filter dropdown* "is in no guard at all", and that is
    /// false as of §15 D578 — on the function the fix calls.** For three sessions
    /// this paragraph carried the open finding (*"the fifth class of floating
    /// thing"*), and the fix routed `dashboard_keys` through this very predicate
    /// without the sentence underneath being touched. Every gate stayed green:
    /// **the next reader of the guard would have read, on the guard itself, that the
    /// guard does not exist.** `arch-scribe` caught it reading D578's brief against
    /// the code — the fifteenth doc defect it has found that no gate can see, and
    /// the first where the stale sentence was *about the change being made*.
    /// ⚠️ **A comment that describes a bug is the one most likely to survive its
    /// fix**, because the fix's author reads it as the statement of the problem
    /// rather than as a claim about the code.
    ///
    /// # Where `menu_was_up` is read, and how to re-check it
    ///
    /// 🚨 **This list is the fix, and it took three rounds to get right — twice
    /// by reading the *fix*, once by reading the *module*.** D558 gated
    /// `pick_or_open` alone; `arch-scribe` then found three more doors, then two
    /// more, and the sweep that finally settled it was **every `.clicked()` in
    /// this file checked against what it does**, not a re-reading of the change.
    /// Eight doors, and three of them *create* a document:
    ///
    /// 1. [`OndinApp::pick_or_open`] — the file card and the file row.
    /// 2. `file_grid`'s dashed *New file* card. **Creates.**
    /// 3. `file_list`'s *New file* row, its twin. **Creates.**
    /// 4. `dashboard_sidebar`'s accent *New file* button — hand-rolled, so
    ///    gating [`Self::nav_row`] did not reach it. **Creates.**
    /// 5. [`Self::nav_row`] — *All files*, *Recent*, *Starred*, *Trash*,
    ///    *New project*.
    /// 6. [`Self::project_row`] — **both** hits, `Select` and the *Unarchive*
    ///    early return, which share the row's rect.
    /// 7. `project_cards`' call of [`Self::project_card`], on *Recent*.
    /// 8. The body header's *Library settings*, search, *Delete project*,
    ///    *Edit project* and sort-cycle controls.
    ///
    /// ⚠️ **Three things deliberately do *not* read it, and each would be a bug
    /// if it did.** [`Self::file_menu_button`] is the ⋮ *anchor* — both dismissals
    /// already exempt it, and gating it would make the menu impossible to close by
    /// its own button. The rows **inside** either menu are the menu working. And
    /// the modals do not need it: they are `egui::Modal`s and stop the pointer
    /// themselves.
    ///
    /// ⚠️ **`dashboard_act` is not the choke point it looks like.** Gating there
    /// would cover every `Act` in one line — and would also swallow the ⋮ menu's
    /// own rows, which route through it, so the menu would stop working entirely.
    /// The gate has to distinguish a click *outside* the popup from one *on its
    /// rows*, and only the call sites know which they are.
    ///
    /// **The body background is not in the list on purpose**: a click on empty
    /// body space is the one gesture that dismisses a menu *and is meant to*, and
    /// all it does is clear the keyboard cursor.
    fn library_menu_open(&self) -> bool {
        self.dash.menu_for.is_some() || self.dash.filter_open
    }

    /// Whether a document in the list on screen may be opened.
    ///
    /// ⚠️ **False in the trash**, and [`OndinApp::file_menu_popup`] has said so
    /// since the trash was built — "a trashed document gets two rows and neither
    /// is *Open*", because opening one puts the editor on a file inside `.trash`
    /// that the next autosave keeps writing to and the purge eventually deletes
    /// out from under the person editing it. The ⋮ menu was the only door that
    /// obeyed: a **double click** on a trashed row opened it anyway, which is the
    /// same reasoning ignored by the same screen twenty lines apart. One predicate
    /// now, read by the double click and by `Enter` (§15 D374).
    fn can_open(&self) -> bool {
        !self.in_trash()
    }

    /// Whether the list on screen is the trash.
    ///
    /// **The one fact three rules are made of**, which is why it is a predicate
    /// rather than three comparisons: [`OndinApp::can_open`] above,
    /// [`OndinApp::file_menu_popup`]'s two-row menu, and the keymap's `F2` and
    /// `Ctrl+D` all mean *a trashed document has two verbs and these are not
    /// among them*. Renaming one would rename the file the purge is going to
    /// delete; duplicating one would put a live copy of a deleted document back
    /// in the library, which is *Restore* by another name and without its
    /// bookkeeping.
    fn in_trash(&self) -> bool {
        self.dash.nav == Nav::Trash
    }

    /// Bring the card or row the keyboard just moved onto into view.
    ///
    /// **Called by the card, not by the keymap**, because the keymap knows an
    /// index and the scroll needs a rect — and the rect only exists once the row
    /// the card is on has been laid out. The flag is the message between them, and
    /// clearing it here is what stops the *next* frame scrolling again while the
    /// pointer is trying to drag the scrollbar.
    ///
    /// `None` rather than an alignment: egui then scrolls the least it can to make
    /// the rect visible, so stepping down a long grid moves one row at a time
    /// instead of snapping the selection to the middle of the viewport on every
    /// press.
    ///
    /// ⚠️ **And [`egui::style::ScrollAnimation::none`], where `scroll_to_rect`
    /// would take the style's 0.1–0.3s.** A key step is under one card, so an
    /// animated one leaves the highlight and the viewport disagreeing for a third
    /// of a second on *every* press — and a held arrow re-targets the animation
    /// before it has finished, which is a grid that never quite catches up with
    /// its own selection. The instant jump is also what makes this observable in a
    /// four-frame probe rather than an eighteen-frame one, which is how the
    /// duration was found: the first version of that test failed with the card
    /// still at y=822, mid-animation.
    fn follow_selection(&mut self, ui: &egui::Ui, picked: bool, rect: egui::Rect) {
        if picked && self.dash.scroll_to_selected {
            ui.scroll_to_rect_animation(rect, None, egui::style::ScrollAnimation::none());
            self.dash.scroll_to_selected = false;
        }
    }

    /// Whether the library screen's own keys may be read this frame — nothing on
    /// screen owns the keyboard.
    ///
    /// **One predicate read from two places** (§15 D542), which is the shape this
    /// file already uses for `filter_applies` and `sort_applies`. There are two
    /// sites on this screen that read `egui::Context` directly —
    /// [`OndinApp::dashboard_keys`] and [`OndinApp::search_field`] — and no modal
    /// can intercept either, so each needs the whole list. Only one had it.
    ///
    /// ⚠️ **`[S20.1-L1-02]`: `Ctrl+K` under *New project* stole the caret and the
    /// card could not be typed into.** Measured — after `Ctrl+K` and one
    /// `Event::Text("Z")`, `dash.search == Some("Z")` and `new_project.name == ""`,
    /// with the card still up and nothing on screen to explain it. `search_field`
    /// re-requests focus on every frame it draws, so the theft is permanent rather
    /// than one frame: `NewProject::name_focused` is a one-shot latch and the card
    /// never asks again. With the rename field up the same chord opened the search
    /// *and* closed the rename, by a key `docs/shortcuts.md` does not list as
    /// closing it.
    ///
    /// **The reasoning already existed and had been applied at one of the two
    /// sites.** §15 D377 states it for the neighbouring case in as many words —
    /// *"`dashboard_keys` gained a guard on `recovery.pending`, because that keymap
    /// reads `egui::Context` directly (D374) and no modal can intercept that"* —
    /// and D374's index line enumerates the guard list with `search_field` absent
    /// from it. Under G15 an enumeration is its own scope, so this was a gap.
    ///
    /// **`egui_wants_keyboard_input` is deliberately *not* in here.** It is a
    /// question about a live text cursor rather than about what is on screen, and
    /// the two callers need it at different moments — `search_field` reads its
    /// chord before drawing its own `TextEdit`, so folding it in would make the
    /// overlay unable to open while any field anywhere had focus. Each caller asks
    /// it separately, next to this.
    fn library_keys_are_free(&self) -> bool {
        // ⚠️ **The recovery card is drawn by `OndinApp::ui`, above the view
        // branch, so it is the one modal over this screen that this file knows
        // nothing about** — and the only one a *launch* can put there. It is an
        // `egui::Modal` and stops the pointer by itself; the keyboard is read
        // straight off the context at both call sites, which no modal can
        // intercept (§15 D377).
        self.recovery.pending.is_empty()
            && self.dash.moving.is_none()
            && self.dash.deleting.is_none()
            && self.dash.deleting_project.is_none()
            && self.dash.new_project.is_none()
            && self.dash.edit_project.is_none()
            && self.library_settings.is_none()
            && self.rename_entry.is_none()
    }

    /// The library's own keymap — arrows, `Enter`, `Delete`, `Escape`, `F2`,
    /// `Ctrl+R`, `Ctrl+N` and `Ctrl+D`
    /// (§15 D374 and D383, `shortcuts.md` §8a — §8 is the editor's application
    /// chords).
    ///
    /// ⚠️ **Read straight off the context, like `Ctrl+K` in
    /// [`OndinApp::search_field`] and for the same reason**: `input::resolve` is
    /// not running at all while the library is up — `OndinApp::ui` returns before
    /// it — so the keymap in `input.rs` is the editor's and this screen answers
    /// its own keys.
    ///
    /// **Every guard is something that owns the keyboard while it is up**, and
    /// they are [`OndinApp::library_keys_are_free`] since §15 D542, because
    /// `search_field` needs the same list and had none of it. A modal, the ⋮
    /// menu, the *Move to project* sheet, the search overlay and the rename field
    /// each take the screen, and `Delete` under any of them means something else
    /// or nothing. `egui_wants_keyboard_input` catches only the ones holding a
    /// caret — it is about *focus*, not about text (`input::resolve`) — and the ⋮
    /// menu, the *Move to project* sheet and the two confirmations hold none at
    /// all, so they are named rather than assumed. **The two floating menus are the
    /// ones that do not merely *block***: the ⋮ popup and the *All projects ⌄*
    /// dropdown each take `Escape` as a dismissal, which neither had a way to do
    /// before this function existed. ⚠️ **That read "the ⋮ menu is the one", and it
    /// was one of two** (§15 D578) — they are asked together now, through
    /// [`OndinApp::library_menu_open`], so the keymap and the eight pointer-side
    /// doors cannot disagree about what is on screen.
    ///
    /// ⚠️ **`search.is_some()` was recorded as measurably redundant and it is not**
    /// (§15 D374, corrected by **D543**). The flip that found it redundant was run
    /// on a **steady-state** frame, where the field is drawn in the top bar before
    /// this runs and asks for focus on the same frame. On the frame the overlay
    /// **closes** the field is never drawn at all: `search_field` returns early,
    /// `search` is already `None` here, and egui has surrendered focus during
    /// `begin_pass` — so this guard and `egui_wants_keyboard_input` were both
    /// false and one `Escape` closed the overlay *and* cleared `dash.selected`.
    /// D374's own closing words are the diagnosis: *"a guard resting on the draw
    /// order of two functions is not a guard."* The fix is at the other end —
    /// `search_field` **consumes** the key now rather than reading it.
    ///
    /// `modifiers.is_none()` is the last of them, and it is what keeps `Ctrl+K`
    /// from also stepping the selection. **It was written to leave every chord on
    /// this screen free to be bound later, and three of them now are** — `Ctrl+N`,
    /// `Ctrl+D` and `Ctrl+R` — which is why the modified ones are read *above*
    /// that gate rather than exempted inside it. The shape holds: everything not
    /// named there is still refused by one line, and the third chord did cost
    /// exactly one arm.
    fn dashboard_keys(&mut self, ctx: &egui::Context, entries: &[Entry], act: &mut Option<Act>) {
        // ⚠️ **The ⋮ popup is an `Area`, not a modal, so nothing else gives it an
        // Escape.** Every card on this screen is a `settings::card_modal` and gets
        // Escape and the backdrop for free through `should_close`; the file menu
        // gets only an outside click, which was invisible while nothing here
        // answered the keyboard and is a hole the moment something does. Handled
        // before the guard list rather than inside it, because the menu owning the
        // keyboard and the menu *ignoring* it are two different things.
        // ⚠️ **Both floating menus, not just the ⋮ one** (§15 D578). The *All
        // projects ⌄* dropdown is the same shape — `project_filter_menu`'s own doc
        // says it is *"the shape `file_menu_popup` already uses on this screen"* —
        // and it was in neither half of this function: `Escape` left it open and
        // **cleared the card cursor behind it** instead, `Delete` opened the *Delete
        // file* confirmation underneath it, and an arrow stepped the cursor behind
        // it. One of the two twins was given a keyboard and the other was not.
        //
        // `library_menu_open` is the predicate, so this cannot fall out of step with
        // the eight pointer-side doors that already ask it (§15 D558) — which is the
        // whole reason that predicate exists.
        if self.library_menu_open() {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.dash.menu_for = None;
                self.dash.filter_open = false;
            }
            return;
        }
        if self.dash.search.is_some()
            || !self.library_keys_are_free()
            || ctx.egui_wants_keyboard_input()
        {
            return;
        }
        let (arrow, enter, delete, escape, rename, new_file, duplicate, rename_chord, plain, chord) =
            ctx.input(|i| {
                let arrow = [
                    egui::Key::ArrowLeft,
                    egui::Key::ArrowRight,
                    egui::Key::ArrowUp,
                    egui::Key::ArrowDown,
                ]
                .into_iter()
                .find(|k| i.key_pressed(*k));
                (
                    arrow,
                    i.key_pressed(egui::Key::Enter),
                    // **Backspace as well as Delete**, because this is a file list and
                    // that is the key a Mac keyboard has. Neither can reach here while
                    // anything is taking text — that is what the guard above is.
                    i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                    i.key_pressed(egui::Key::Escape),
                    i.key_pressed(egui::Key::F2),
                    i.key_pressed(egui::Key::N),
                    i.key_pressed(egui::Key::D),
                    // **`Ctrl+R`, the editor's other rename** (`shortcuts.md`
                    // §4 binds both it and `F2` to renaming a layer), which is
                    // read on the chord side while `F2` is read on the plain one
                    // — the same verb entered through the two gates below.
                    i.key_pressed(egui::Key::R),
                    i.modifiers.is_none(),
                    // **Spelled the way `input::normal_mode` spells it** —
                    // `Ctrl` without Alt and without Shift — so `Ctrl+Alt+N` and
                    // `Ctrl+Shift+D` stay unbound here exactly as they are there,
                    // rather than becoming second doors this screen invented.
                    i.modifiers.command && !i.modifiers.alt && !i.modifiers.shift,
                )
            });
        // **The chords, before `plain` closes the door on everything modified.**
        // Each is the editor's own verb read with a different noun: `Ctrl+N` is
        // *New file* — the sidebar button and the dashed card, which the arrows
        // deliberately cannot reach ([`OndinApp::new_file_card`]) — `Ctrl+D` is
        // *Duplicate* on the ⋮ menu, and `Ctrl+R` is *Rename*, which §4 binds
        // alongside `F2` for a layer. One chord, one verb, two nouns.
        //
        // ⚠️ **`Ctrl+R` is here and `F2` is past the gate below**, which is the
        // whole reason [`OndinApp::start_rename`] is a function: the two
        // spellings of one verb cannot both be written at one site, and a rule
        // added to either — the trash refusal, where the name is read from —
        // would otherwise be a rule only one of the keys obeyed.
        if chord {
            if new_file {
                *act = Some(Act::NewFile);
            } else if rename_chord {
                self.start_rename(entries, act);
            } else if duplicate
                && !self.in_trash()
                && let Some(path) = self.dash.selected.clone()
            {
                *act = Some(Act::Duplicate(path));
            }
            return;
        }
        if !plain {
            return;
        }
        if let Some(key) = arrow {
            // ⚠️ **The index is found by path every frame rather than stored.**
            // Every action on this screen re-scans the folder, so the position of
            // a document in `entries` is not stable across a rename, a star or
            // anything a sync client does to the folder — an index kept in
            // `DashboardState` would step from where the selection *used* to be.
            let at = self
                .dash
                .selected
                .as_ref()
                .and_then(|p| entries.iter().position(|e| &e.path == p));
            if let Some(to) = arrow_target(at, key, self.dash.list_view, entries.len()) {
                self.dash.selected = Some(entries[to].path.clone());
                self.dash.scroll_to_selected = true;
            }
            return;
        }
        if escape {
            // The keyboard's version of a click on the empty body, which is the
            // only other way to put a selection back.
            self.dash.selected = None;
            return;
        }
        let Some(path) = self.dash.selected.clone() else {
            return;
        };
        if enter && self.can_open() {
            *act = Some(Act::Open(path));
        } else if rename {
            self.start_rename(entries, act);
        } else if delete {
            // **Through the confirmation rather than straight to the act.** The ⋮'s
            // *Delete* does the same, and a key has more reason to than a menu row
            // does: a menu row was aimed at, where `Delete` can be the tail of a
            // chord the user thought they were typing into something else.
            self.dash.deleting = Some(path);
        }
    }

    /// Open the rename field over the keyboard's selection — `F2` and `Ctrl+R`.
    ///
    /// **One function because the two keys are one verb**, and because they are
    /// read on opposite sides of [`OndinApp::dashboard_keys`]' `plain` gate:
    /// `F2` is bare and `Ctrl+R` is a chord, so a rule written at either site
    /// would be a rule the other spelling did not have. Both are the editor's,
    /// where §4 has bound the same pair to renaming a layer since before there
    /// was a library.
    ///
    /// ⚠️ **The name comes from `entries`, not from the path**, because that is
    /// where the ⋮'s *Rename* reads it and the two must open the same field with
    /// the same text in it: `Entry::display_name` is the stem with the conflict
    /// suffix a sync client may have added taken off, and a field pre-filled with
    /// the raw stem would quietly hand the user that suffix back to save (D375).
    ///
    /// Nothing happens with no selection, in the trash — where renaming a
    /// document renames the file the purge is about to delete
    /// ([`OndinApp::in_trash`]) — or when the selected path is no longer in
    /// `entries`, which is a document deleted by another window between the press
    /// and this frame.
    fn start_rename(&self, entries: &[Entry], act: &mut Option<Act>) {
        if self.in_trash() {
            return;
        }
        let Some(path) = self.dash.selected.clone() else {
            return;
        };
        if let Some(entry) = entries.iter().find(|e| e.path == path) {
            *act = Some(Act::StartRename(path, entry.display_name()));
        }
    }

    fn file_list(
        &mut self,
        ui: &mut egui::Ui,
        entries: &[Entry],
        new_file: Option<&str>,
        act: &mut Option<Act>,
    ) {
        let full = ui.available_width();
        // The design's `1fr 170px 120px 120px 90px`.
        let cols = [full - 500.0, 170.0, 120.0, 120.0, 90.0];
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(full, 26.0), egui::Sense::empty());
            let mut x = rect.left();
            for (label, w) in ["Name", "Project", "Edited", "Created", "Size"]
                .iter()
                .zip(cols)
            {
                ui.painter().text(
                    egui::pos2(x + 10.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    *label,
                    egui::FontId::proportional(11.0),
                    theme::text::FAINT,
                );
                x += w;
            }
        });
        for entry in entries {
            self.file_row(ui, entry, &cols, act);
        }
        // **The row says only *New file*, where the card says *New file in X*.**
        // The design's, and right: a list has one row per file and no picture, so
        // the row reads in the context of the heading above it, while a card is
        // one of four in a grid a person scans rather than reads.
        // R4 (§15 D558) — the list view's twin of the dashed card, and the
        // **third** door on this screen that creates a document.
        if new_file.is_some() && self.new_file_row(ui) && !self.dash.menu_was_up {
            *act = Some(Act::NewFile);
        }
    }

    /// The *New file* row at the end of a project's list. Returns whether it was
    /// clicked. See [`OndinApp::new_file_card`] for why it is gated on a project.
    fn new_file_row(&self, ui: &mut egui::Ui) -> bool {
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), ROW_H),
            egui::Sense::click(),
        );
        let lit = resp.hovered();
        let ink = if lit {
            color::ACCENT_200
        } else {
            theme::text::FAINT
        };
        let p = ui.painter();
        if lit {
            p.rect_filled(rect, egui::CornerRadius::same(6), theme::color::text_a(10));
        }
        // The design's 20pt plate under the glyph, which is what keeps this row's
        // left edge lined up with the names above it: a bare `+` at the same x
        // would sit in the middle of the column the file names start at.
        let badge = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 16.0, rect.center().y),
            egui::Vec2::splat(20.0),
        );
        p.rect_filled(badge, egui::CornerRadius::same(5), theme::color::text_a(7));
        p.text(
            badge.center(),
            egui::Align2::CENTER_CENTER,
            icon::PLUS,
            theme::icon_font(12.0),
            ink,
        );
        p.text(
            egui::pos2(badge.right() + 9.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            "New file",
            egui::FontId::proportional(12.0),
            ink,
        );
        resp.clicked()
    }

    fn file_row(
        &mut self,
        ui: &mut egui::Ui,
        entry: &Entry,
        cols: &[f32; 5],
        act: &mut Option<Act>,
    ) {
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), ROW_H),
            egui::Sense::click(),
        );
        let lit = resp.hovered();
        let picked = self.dash.selected.as_deref() == Some(entry.path.as_path());
        self.follow_selection(ui, picked, rect);
        // **A wash rather than a border, which is the design's answer for a row**
        // (`rowBg: accent 12%`) and the right one: a hairline around a 38pt row
        // that is flush with its neighbours reads as the row having grown, where a
        // ground reads as the row being picked. The card gets the border because a
        // card already has one to change.
        if picked || lit {
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::same(6),
                if picked {
                    color::ACCENT_900
                } else {
                    theme::color::text_a(10)
                },
            );
        }
        let project = self
            .library
            .project_of(entry)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "—".into());
        let cells = [
            entry.display_name(),
            project,
            clock::relative_label(clock::since(entry.modified)),
            entry
                .meta
                .created
                .map(clock::date_label)
                .unwrap_or_else(|| "—".into()),
            crate::settings::human_bytes(entry.size),
        ];
        let name_box = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 6.0, rect.center().y - 11.0),
            egui::vec2(cols[0] - 20.0, 22.0),
        );
        let renaming = self.rename_field(ui, entry, name_box, 12.0, act);
        // ⚠️ **The chip takes its width out of the *Name* column before the name
        // is elided**, rather than being drawn over the end of it. A row has no
        // spare width — the design's grid is `1fr 170px 120px 120px 90px` — so the
        // two genuinely compete, and the name is the one that can shorten.
        let chip = self.mark_of(entry).filter(|_| !renaming);
        let chip_room = chip.map_or(0.0, |m| mark_chip_size(ui, m).x + 8.0);
        let mut x = rect.left();
        for (i, (text, w)) in cells.iter().zip(cols).enumerate() {
            if i == 0 && renaming {
                x += w;
                continue;
            }
            let room = w - 34.0 - if i == 0 { chip_room } else { 0.0 };
            let shown = elide(ui, text, 12.0, room);
            ui.painter().text(
                egui::pos2(x + 10.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                &shown,
                egui::FontId::proportional(12.0),
                if i == 0 {
                    theme::text::STRONG
                } else {
                    theme::text::DIM
                },
            );
            if let (0, Some(mark)) = (i, chip) {
                // Beside the name it belongs to, not at a fixed column: two
                // conflicts with names of different lengths would otherwise have
                // their marks at the same x and their names ending anywhere, which
                // reads as a column of chips rather than as a mark on a document.
                let name_w = ui
                    .ctx()
                    .fonts_mut(|f| {
                        f.layout_no_wrap(
                            shown.clone(),
                            egui::FontId::proportional(12.0),
                            theme::text::STRONG,
                        )
                    })
                    .size()
                    .x;
                mark_chip(
                    ui,
                    egui::pos2(x + 18.0 + name_w, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    mark,
                );
            }
            x += w;
        }
        self.pick_or_open(&resp, entry, act);
        self.file_menu_button(
            ui,
            entry,
            egui::pos2(rect.right() - 16.0, rect.center().y),
            act,
        );
    }

    /// Draw the inline rename field, if this is the document being renamed.
    ///
    /// Returns whether it drew, so the caller knows to skip painting the name.
    ///
    /// ⚠️ **Commits on `lost_focus`, not only on Enter.** A rename field that
    /// only committed on Enter would silently throw the typed name away when the
    /// user clicked elsewhere — which is what "I renamed it and it didn't take"
    /// looks like. Escape is the way to *not* commit, and it clears the buffer
    /// before the focus loss can be read.
    ///
    /// ⚠️ **And it committed on neither for two days** (§15 D380). Everything this
    /// function does about focus, layout and placement was wrong at once, each
    /// fault marked below; the one that made renaming *impossible* is the focus
    /// latch. Read the three together before changing any of them — they are three
    /// answers to "put a live text field where a painted word is", and each has a
    /// plausible-looking wrong version that the others do not catch.
    fn rename_field(
        &mut self,
        ui: &mut egui::Ui,
        entry: &Entry,
        rect: egui::Rect,
        size: f32,
        act: &mut Option<Act>,
    ) -> bool {
        let Some((path, buffer, focused)) = self.rename_entry.as_mut() else {
            return false;
        };
        if path != &entry.path {
            return false;
        }
        if ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
            self.rename_entry = None;
            return false;
        }
        let id = egui::Id::new(("rename", &entry.path));
        // **The app's field ground, painted here rather than left to egui.** A
        // default `TextEdit` grounds itself in `extreme_bg_color` and, once
        // focused, outlines itself in `Visuals::selection.stroke` — the blue ring
        // nothing else in this app draws. `ui::text_field` states the rule; this
        // one cannot use that helper because it is placed into a rect the card has
        // already reserved rather than allocated down a column.
        //
        // ⚠️ **`color::FIELD_BORDER`, which is what `ui::field_frame` strokes with
        // — not the accent.** This drew an `ACCENT_700` ring, which is a blue
        // outline on focus by another route: the app's own, rather than egui's, and
        // just as much the thing no other text box in the app does. A field that is
        // only ever on screen *while* focused has nothing to distinguish with it.
        let p = ui.painter();
        p.rect_filled(rect, egui::CornerRadius::same(ui::BUTTON_R), color::FIELD);
        p.rect_stroke(
            rect,
            egui::CornerRadius::same(ui::BUTTON_R),
            egui::Stroke::new(1.0, color::FIELD_BORDER),
            egui::StrokeKind::Inside,
        );
        // ⚠️ **4, because that is where the name it replaces is painted.** Both
        // callers reserve a box 4pt to the left of their own name text — the card
        // at `left + 8` against a name at `left + 12`, the row at `left + 6`
        // against `left + 10` — so any other inset makes the word jump sideways at
        // the moment the field appears. Same fault as [`SEARCH_TEXT_X`]'s.
        //
        // ⚠️ **It goes on the *frame*, not in `TextEdit::margin`, which does
        // nothing here.** egui reads that builder only when no frame was supplied
        // (`let frame = frame.unwrap_or_else(|| Frame::new().inner_margin(margin))`
        // in its `text_edit::builder`), so `.frame(NONE).margin(m)` silently drops
        // `m` and lays the text against the box's own edge. Found by a flip that
        // did *not* bite: swapping the search field's `Margin::ZERO` for egui's
        // default moved nothing, because neither was ever read.
        // ⚠️ **One row tall, centred in the box, rather than justified into it.**
        // `Ui::put` lays out `centered_and_justified`, so a singleline `TextEdit`
        // handed a 20pt box stretches to it and sets its galley at the top — the
        // word sat two or three points above where the painted name it replaces
        // does, and swapping between them was a visible jump. `SEARCH_TEXT_X`'s
        // paragraph is the same fault on the x axis; this is the y.
        let font = egui::FontId::proportional(size);
        let row = ui.ctx().fonts_mut(|f| f.row_height(&font));
        let line = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), row));
        // ⚠️ **`new_child`, not `Ui::put`, and this is what stopped the card beside
        // it from moving.** `put` ends in `advance_cursor_after_rect`, which in the
        // grid's `horizontal` row sets the cursor to *this* rect's right edge — and
        // this rect is the name box, well to the *left* of where the cursor already
        // was, since the whole card has been allocated. So the cursor rewound and
        // the next card was laid out on top of this one. A child `Ui` is placed at
        // an absolute rect and never touches the parent's placer.
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .id(id.with("ui"))
                .max_rect(line)
                .layout(egui::Layout::centered_and_justified(
                    egui::Direction::TopDown,
                )),
        );
        let resp = child.add(
            egui::TextEdit::singleline(buffer)
                .id(id)
                .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(4, 0)))
                .font(font),
        );
        // ⚠️ **Once, through the latch — never every frame.** `request_focus` here
        // on every pass re-takes the caret in the same frame that surrendered it,
        // and `lost_focus()` is then false for the rest of time: Enter committed
        // nothing, a click elsewhere committed nothing and did not even leave the
        // field, and Escape — which returns above before any of this runs — was the
        // only way out, discarding the name. There was no way to rename a file.
        // `NewProject::name_focused` records this exact fault; this field was left
        // behind when that one was fixed.
        if !*focused {
            resp.request_focus();
            *focused = true;
        }
        if resp.lost_focus() {
            // Enter and a click elsewhere both land here, and both commit. The
            // Escape arm above has already cleared the buffer in the case that
            // must not.
            *act = Some(Act::CommitRename);
        }
        true
    }

    /// The ⋮ and the menu behind it — the one place every per-document action
    /// lives, including *Rename* (which the design had nowhere else to put).
    fn file_menu_button(
        &mut self,
        ui: &mut egui::Ui,
        entry: &Entry,
        centre: egui::Pos2,
        act: &mut Option<Act>,
    ) {
        let hit = egui::Rect::from_center_size(centre, egui::vec2(22.0, 22.0));
        let resp = ui.interact(
            hit,
            egui::Id::new(("file-menu", &entry.path)),
            egui::Sense::click(),
        );
        let open = self.dash.menu_for.as_ref() == Some(&entry.path);
        ui.painter().text(
            centre,
            egui::Align2::CENTER_CENTER,
            icon::DOTS_THREE_VERTICAL,
            theme::icon_font(15.0),
            if open || resp.hovered() {
                color::TEXT
            } else {
                theme::text::DIM
            },
        );
        if resp.clicked() {
            self.dash.menu_for = if open { None } else { Some(entry.path.clone()) };
            self.dash.moving = None;
        }
        if open {
            self.file_menu_popup(ui, entry, hit, act);
        }
    }

    fn file_menu_popup(
        &mut self,
        ui: &mut egui::Ui,
        entry: &Entry,
        anchor: egui::Rect,
        act: &mut Option<Act>,
    ) {
        let trashed = self.in_trash();
        let starred = self.is_starred(entry);
        let rows: Vec<(&str, Act)> = if trashed {
            // ⚠️ **A trashed document gets two rows and neither is *Open*.**
            // Opening one would put the editor on a file inside `.trash`, which
            // the next autosave would then keep writing to — a document the
            // library does not list and the purge will eventually delete out
            // from under the person editing it.
            vec![
                ("Restore", Act::Restore(entry.path.clone())),
                ("Delete permanently", Act::Trash(entry.path.clone())),
            ]
        } else {
            vec![
                ("Open", Act::Open(entry.path.clone())),
                (
                    "Rename",
                    Act::StartRename(entry.path.clone(), entry.display_name()),
                ),
                ("Duplicate", Act::Duplicate(entry.path.clone())),
                (
                    if starred { "Unstar" } else { "Star" },
                    Act::ToggleStar(entry.meta.id.clone().unwrap_or_default()),
                ),
                ("Move to project", Act::MoveTo(entry.path.clone(), None)),
                ("Delete", Act::Trash(entry.path.clone())),
            ]
        };

        // Narrower than the filter's `FILTER_MENU_W`: these are verbs and those
        // are project names, so the two widths are a decision rather than drift.
        // The **row height** was not — it was a bare `28.0` written three times
        // where the constant says the same number (§15 D729).
        let w = 170.0;
        let out = anchored_menu(
            ui,
            egui::Id::new(("file-menu-popup", &entry.path)),
            anchor,
            w,
            rows.len(),
            |p, i, row| {
                let label = rows[i].0;
                // *Delete* is the one row that is not the ordinary ink, because
                // it is the one row that cannot be undone by clicking again.
                let danger = label.starts_with("Delete");
                p.text(
                    egui::pos2(row.left() + 10.0, row.center().y),
                    egui::Align2::LEFT_CENTER,
                    label,
                    egui::FontId::proportional(12.0),
                    if danger {
                        theme::color::DANGER
                    } else {
                        theme::text::STRONG
                    },
                );
            },
        );

        if let Some(i) = out.chosen {
            // ⚠️ **The tuple's label is dropped here rather than bound and
            // discarded.** This read `let (label, action) = …; … let _ = label;`
            // — a destructure left over from a shape this function no longer has
            // (§15 D729).
            let action = rows.into_iter().nth(i).expect("index from the same list").1;
            self.dash.menu_for = None;
            match action {
                // *Move to project* and *Delete* open something rather than
                // doing something, so they set state instead of becoming an act.
                Act::MoveTo(path, None) => self.dash.moving = Some(path),
                Act::Trash(path) if !trashed => self.dash.deleting = Some(path),
                other => *act = Some(other),
            }
        } else if out.dismissed {
            self.dash.menu_for = None;
        }
    }

    /// Apply what the frame decided. See [`Act`] for why this is deferred.
    fn dashboard_act(&mut self, act: Act) {
        let root = self.library.root.clone();
        match act {
            Act::Open(path) => self.open_path(&path),
            Act::NewFile => {
                let project = self.nav_project();
                self.new_library_document(project);
            }
            Act::Duplicate(path) => {
                if let Some(entry) = self.library.entry_at(&path).cloned() {
                    let project = self.library.project_of(&entry).cloned();
                    match store::duplicate(&root, &entry, project.as_ref()) {
                        Ok(copy) => {
                            self.library.refresh();
                            // ⚠️ **Stamped as touched here, or the copy is
                            // invisible on the nav it was made from.** *Recent* is
                            // "documents this machine has opened", and a duplicate
                            // has never been opened — so duplicating from the
                            // landing page appeared to do nothing at all, while the
                            // file sat in *All files*. `new_library_document` has
                            // never had this problem because it opens what it
                            // creates; this is the same rule spelled for the one
                            // action that creates a document without opening it.
                            //
                            // The alternative was dropping *Duplicate* from the ⋮
                            // on *Recent*, which makes one menu differ by nav for
                            // a reason no user could infer.
                            if let Some(id) =
                                self.library.entry_at(&copy).and_then(|e| e.meta.id.clone())
                            {
                                self.library.local.mark_opened(&id);
                                self.library.local.save();
                            }
                        }
                        Err(e) => self.session.fail(format!("Duplicate failed: {e}")),
                    }
                }
            }
            Act::MoveTo(path, project_id) => {
                if let Some(entry) = self.library.entry_at(&path).cloned() {
                    let project = project_id.and_then(|id| self.library.projects.get(&id).cloned());
                    match store::move_to_project(&root, &entry, project.as_ref()) {
                        Ok(moved) => {
                            // **The open document follows its file.** Without
                            // this the editor keeps saving to a path that no
                            // longer exists, and the next autosave recreates the
                            // document where it used to be — and without the
                            // `meta.project` beside it the next save moves the
                            // document back into the project it was just taken
                            // out of.
                            let id = project.as_ref().map(|p| p.id.clone());
                            self.open_document_follows(&path, moved, |m| m.project = id);
                            self.library.refresh();
                        }
                        Err(e) => self.session.fail(format!("Move failed: {e}")),
                    }
                }
                self.dash.moving = None;
            }
            Act::Trash(path) => self.trash_or_purge(&path),
            Act::Restore(path) => {
                if let Some(entry) = store::trashed(&root).into_iter().find(|e| e.path == path) {
                    let project = self.library.project_of(&entry).cloned();
                    match store::move_to_project(&root, &entry, project.as_ref()) {
                        // The third door onto the same file move. ⚠️ **The follow
                        // is unreachable here today and is written anyway**:
                        // `trash_or_purge` sets `session.path = None` when it
                        // trashes the open document, so by the time a restore
                        // runs there is no path left to match. That is a fact
                        // about a *different* function, twenty lines up, and the
                        // day it stops clearing the path this arm would silently
                        // become the rename bug again. Checked, not assumed.
                        Ok(moved) => {
                            let id = project.as_ref().map(|p| p.id.clone());
                            self.open_document_follows(&path, moved, |m| m.project = id);
                            self.library.refresh();
                        }
                        Err(e) => self.session.fail(format!("Restore failed: {e}")),
                    }
                }
            }
            Act::StartRename(path, name) => self.rename_entry = Some((path, name, false)),
            Act::CommitRename => self.commit_rename(),
            Act::CreateProject => self.create_project(),
            Act::SaveProject => self.save_project(),
            Act::SetArchived(id, archived) => self.set_archived(&id, archived),
            Act::ToggleStar(id) => {
                if !id.is_empty() {
                    self.library.local.toggle_star(&id);
                    self.library.local.save();
                }
            }
            Act::Import => self.import_documents(),
            Act::DeleteProject {
                id,
                transfer_to,
                keep_files,
            } => self.delete_project(&id, transfer_to.as_deref(), keep_files),
        }
    }

    /// Remove a project and settle its files.
    ///
    /// ⚠️ **The files are dealt with first, and the project row is removed last.**
    /// If the order were reversed and the run failed halfway, the surviving
    /// documents would point at a project id that no longer exists — which the
    /// library treats as unfiled (`Library::project_of`), so nothing breaks, but
    /// they would have quietly left the project the user asked to *transfer*
    /// them into. Doing the moves first means a failure leaves the project
    /// standing with some of its files already moved, which is a state the user
    /// can see and repeat.
    ///
    /// 🚨 **That last sentence was false until §15 D556, and the ordering it
    /// argues for was doing nothing** (`[S20.2-L1-03]`). The `retain` below ran
    /// unconditionally, so a run that failed halfway removed the project row
    /// anyway — producing exactly the state the ordering was chosen to avoid, and
    /// producing it *silently*, because the `failed` counter's only consumer is a
    /// status line. Measured end to end with `.trash` present as a **file**, so
    /// `store::trash`'s `create_dir_all` fails: the row was gone, **both** members
    /// were still on disk with `meta.project = Some("p-1")`, and `project_of`
    /// answered `None` — so the user watched a project and its files disappear and
    /// the documents were still in the library wearing a project id nothing lists.
    /// (This read *"the member"*, singular, and named **p-2**, which is the empty
    /// transfer target; `arch-scribe` read it against the fixture.)
    ///
    /// **The row now survives a partial failure**, which is what makes the state
    /// *"one the user can see and repeat"*: the project is still there, holding
    /// whatever could not be moved, and pressing *Delete project* again retries
    /// exactly the members that failed.
    pub(crate) fn delete_project(&mut self, id: &str, transfer_to: Option<&str>, keep_files: bool) {
        // ⚠️ **Before a single file moves.** `save_projects` latches on its own,
        // so a check *after* the loop would keep `projects.json` safe and still
        // be a bug: every document would have left the project, and the project
        // would still be there holding none of them. Found by a flip-check that
        // did not bite — the bytes were protected either way, and the files were
        // not.
        if !self.library.may_write_projects() {
            self.session
                .fail("projects.json could not be read — not deleting anything");
            self.dash.deleting_project = None;
            return;
        }
        let root = self.library.root.clone();
        let members: Vec<Entry> = self
            .library
            .entries
            .iter()
            .filter(|e| e.meta.project.as_deref() == Some(id))
            .cloned()
            .collect();
        let target = transfer_to.and_then(|t| self.library.projects.get(t).cloned());
        let mut failed = 0usize;
        for entry in &members {
            // The open document follows its file, exactly as a single move does
            // — and the two halves follow it differently. ⚠️ **The transfer half
            // used to do nothing at all**, because the `!keep_files` on the guard
            // read as "only the trash case needs handling": a transferred
            // document really does move, so the session was left pointing at a
            // path in the deleted project's folder *and* carrying the deleted
            // project's id in its own metadata, which the next save wrote back
            // into the file.
            if keep_files {
                match store::move_to_project(&root, entry, target.as_ref()) {
                    Ok(moved) => {
                        let to = target.as_ref().map(|p| p.id.clone());
                        self.open_document_follows(&entry.path, moved, |m| m.project = to);
                    }
                    Err(_) => failed += 1,
                }
            } else {
                match store::trash(&root, entry) {
                    // Its file is in the trash now, so there is nowhere to save
                    // to — the same answer `trash_or_purge` gives.
                    Ok(_) => {
                        if self.session.path.as_deref() == Some(entry.path.as_path()) {
                            self.session.path = None;
                        }
                    }
                    Err(_) => failed += 1,
                }
            }
        }
        // ⚠️ **Only when every member actually moved** (§15 D556) — see the doc
        // above for what the unconditional version produced. `save_projects` is
        // called either way: on the success path it is the removal being written,
        // and on the failure path it is a no-op write that still reports a
        // `projects.json` that cannot be written, which is worth knowing before
        // the retry.
        if failed == 0 {
            self.library.projects.projects.retain(|p| p.id != id);
        }
        if !self.library.save_projects() {
            self.session.fail("Could not write projects.json");
        }
        self.library.reload_projects();
        self.library.refresh();
        // Standing on a heading that no longer exists is the one state this must
        // not leave behind.
        //
        // **Gated on the removal having happened, for the same reason.** A nav
        // reset on the failure path would take the user off the very heading that
        // now holds the files they have to deal with — the one screen where the
        // retry is one click away.
        if failed == 0 && self.dash.nav == Nav::Project(id.to_string()) {
            self.dash.nav = Nav::All;
        }
        self.dash.deleting_project = None;
        if failed > 0 {
            self.session.fail(format!(
                "{failed} file(s) could not be moved — the project has been kept"
            ));
        }
    }

    /// Copy `.ondin` documents from outside the library into it.
    ///
    /// **The only file dialog left in the app**, and it is an *open* dialog
    /// rather than a save one — the user is naming files that already exist,
    /// which is the one question the library cannot answer for them.
    ///
    /// **A copy, not a move.** The originals are somebody else's files in
    /// somebody else's folder — very possibly a shared drive or a download — and
    /// taking them away is not what "import" means anywhere else. The cost is a
    /// duplicate on disk, which the user can delete knowing the library has its
    /// own.
    ///
    /// ⚠️ **An imported document keeps the name and creation date it arrives
    /// with, and gets a fresh id.** Keeping its id would let two libraries — or
    /// the same library twice — hold two documents claiming one version history,
    /// which is the same reasoning `store::duplicate` records. Importing the
    /// same file twice therefore gives two independent documents rather than one
    /// silently overwritten.
    fn import_documents(&mut self) {
        // Before the dialog rather than after it: a native file picker that
        // collects an answer and then throws it away is worse than one that never
        // opened.
        if !self.library_writable() {
            return;
        }
        let Some(files) = rfd::FileDialog::new()
            .add_filter("Ondin document", &[crate::library::scan::DOCUMENT_EXT])
            .set_title("Import into the library")
            .pick_files()
        else {
            return;
        };
        self.import_paths(&files);
    }

    /// Documents dropped on the library window, filed exactly as *Import files*
    /// files them (§15 D373).
    ///
    /// ⚠️ **Only files with a `path`.** egui's `DroppedFile` carries either a path
    /// (a real file-manager drop) or `bytes` (a drag out of a browser), and
    /// [`Self::import_paths`] is a path-taking function all the way down — it
    /// reads, parses, mints an id and hands the document to `store::file_document`
    /// under a name derived from the file stem. A bytes-only drop has no stem and
    /// no source to skip-if-already-in-the-library against, so it is counted and
    /// reported rather than half-handled. The canvas makes the opposite choice for
    /// images, and can: a pasted picture needs no name.
    ///
    /// **Everything that is not a document is one message, not one per file.**
    /// A folder of forty PNGs dragged here would otherwise be forty failures; what
    /// the user needs to know is that this screen takes `.ondin` files, said once.
    fn take_dropped_documents(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        // **After the emptiness check, so the message only answers a real drop**
        // — this runs on every frame of the library, and a guard above it would
        // fire the refusal continuously while the folder was missing.
        if !self.library_writable() {
            return;
        }
        let mut docs: Vec<PathBuf> = Vec::new();
        let mut others = 0usize;
        for file in dropped {
            match file.path {
                Some(p)
                    if p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("ondin")) =>
                {
                    docs.push(p);
                }
                _ => others += 1,
            }
        }
        // ⚠️ **One message, composed here, and this is the half that was wrong
        // first.** The refusal used to be gated on `docs.is_empty()`, so a drop of
        // one document and forty pictures reported the document and said nothing
        // whatever about the forty — a silent partial failure of exactly the kind
        // this project does not leave lying around. It could not simply say both,
        // either: `EditorSession`'s status is one slot and the second call would
        // have erased the first. So the counting and the announcing are separated
        // ([`Self::import_counting`]) and the sentence is built once.
        if docs.is_empty() {
            self.session
                .fail("The library takes .ondin documents — open one to place images in it");
            return;
        }
        let (ok, failed, already) = self.import_counting(&docs);
        self.session
            .info(import_summary(ok, failed, others, already));
    }

    /// The half of [`Self::import_documents`] that is not the dialog.
    ///
    /// ⚠️ **Split out so it can be tested at all.** `rfd::FileDialog` opens a
    /// native window and blocks; nothing headless can drive it, so a single
    /// function would have left every rule above — the copy, the fresh id, the
    /// skip, the name fallback — verified by reading. This is the seam the probes
    /// enter through.
    ///
    /// The behaviour itself moved down one more level into
    /// [`Self::import_counting`] when the drop path arrived (§15 D373): this is
    /// now that plus the announcement, which is all the dialog needs and one thing
    /// too many for a drop that has its own count to add.
    pub(crate) fn import_paths(&mut self, files: &[PathBuf]) {
        let (ok, failed, already) = self.import_counting(files);
        self.session.info(import_summary(ok, failed, 0, already));
    }

    /// [`Self::import_paths`] without the announcement, for the one caller that
    /// has something of its own to add to it.
    ///
    /// ⚠️ **Split because `EditorSession`'s status is a *single slot*** — `info`
    /// and `fail` assign rather than append, so a caller that spoke after this one
    /// would not be adding a sentence, it would be erasing the count. A drop
    /// carrying documents *and* other files has one thing to say and has to say it
    /// once; see [`import_summary`].
    ///
    /// `pub(crate)` so D557's third door is pinned by a test rather than only
    /// fixed — it was fixed and unasserted until `arch-scribe` said so.
    pub(crate) fn import_counting(&mut self, files: &[PathBuf]) -> (usize, usize, usize) {
        // Into the project being viewed, which is the answer the design's
        // *Target project* field asks for and the one the user has already
        // given by being where they are.
        //
        // ⚠️ **`destination` and not `get`** — the third of D557's doors, and the
        // one that finding did not name (`arch-scribe` found it after the first
        // two were fixed). An archived project's row is selectable from the
        // sidebar's own group, so *"the project being viewed"* can legitimately be
        // an archived one — and an import taken there filed **every** copied
        // document into it, which is `move_modal`'s refused destination reached in
        // bulk. It is the same third question `new_file_into` and `nav_project`
        // ask, so it takes the same predicate; the honest answer here is unfiled.
        let project = match &self.dash.nav {
            Nav::Project(id) => self.library.projects.destination(id).cloned(),
            _ => None,
        };
        let root = self.library.root.clone();
        let (mut ok, mut failed, mut already) = (0usize, 0usize, 0usize);
        for path in files {
            // ⚠️ **Already-in-the-library files are skipped, not re-imported.**
            // The dialog opens on the last-used folder, which may well *be* the
            // base folder; importing a document over itself would give it a
            // second id and a `-1` filename for no reason the user could see.
            //
            // ⚠️ **Counted, because a silent `continue` is a silent partial
            // refusal** (§15 D693, `[S20.2-L1-06]`). This arm used to drop the
            // skip on the floor, so an import of three documents two of which
            // were already here reported *"Imported 1 file(s)"* and said nothing
            // at all about the other two. That is the same defect this module's
            // own comment records having fixed for the *other* skip category —
            // *"a drop of one document and forty pictures reported the document
            // and said nothing whatever about the forty"* — surviving in the one
            // beside it. See [`import_summary`].
            if path.starts_with(&root) {
                already += 1;
                continue;
            }
            let Ok(bytes) = std::fs::read(path) else {
                failed += 1;
                continue;
            };
            let Ok(mut doc) = ondin_core::io::load(&bytes) else {
                failed += 1;
                continue;
            };
            let name = doc
                .meta()
                .name
                .clone()
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| {
                    // No block, so the filename is all there is — de-slugged the
                    // same way the list does it for a pre-library document.
                    crate::library::scan::name_from_stem(
                        path.file_stem().and_then(|s| s.to_str()).unwrap_or(""),
                    )
                });
            // A fresh identity, per the note above.
            let mut meta = doc.meta().clone();
            meta.id = None;
            doc.set_meta(meta);
            match store::file_document(&root, project.as_ref(), &name, &mut doc) {
                Ok(_) => ok += 1,
                Err(_) => failed += 1,
            }
        }
        self.library.refresh();
        (ok, failed, already)
    }

    /// Whether a document is starred here.
    ///
    /// ⚠️ **Per-machine, in the cache**, which is a decision worth knowing about:
    /// a star does not travel with a synced library. It is where it is because
    /// the alternative — a field in the document's own metadata — makes starring
    /// rewrite the whole `.ondin` file, which for a 12 MB document is a
    /// surprising amount of work for one click, and puts a sync upload behind it.
    fn is_starred(&self, entry: &Entry) -> bool {
        entry
            .meta
            .id
            .as_deref()
            .is_some_and(|id| self.library.local.is_starred(id))
    }

    /// Trash a document, or delete it for good if it is already in the trash.
    pub(crate) fn trash_or_purge(&mut self, path: &std::path::Path) {
        let root = self.library.root.clone();
        if self.dash.nav == Nav::Trash {
            if let Err(e) = std::fs::remove_file(path) {
                self.session.fail(format!("Delete failed: {e}"));
            }
            // **The writer §15 D728 warned about, and it was already in the tree when
            // that warning was written** — the permanent delete, the one door into
            // `.trash` that does not go through `store`. (D728 said *"a sixth writer
            // added later has to know"*; the count is gone from `refresh_trash`'s doc
            // now, because it read as a survey of a closed set while this door sat
            // outside it.) D728's enumeration was of `store`'s writers, so a
            // grep for `store::trash` / `store::purge_trash` could not see an
            // `fs::remove_file` sitting in a panel.
            //
            // 🚨 **Before the cache this path was self-correcting and needed
            // nothing.** `visible_entries` rescanned `.trash` every frame, so the row
            // left the list the moment the file left the disk. Holding the listing
            // turned a door that had never had to tell anybody into one that must:
            // the document stayed in the Trash list *and* in the sidebar's count
            // until something unrelated refreshed. **Caching a scan makes every
            // writer that did not have to announce itself into a writer that does**,
            // which is the standing hazard D728 states and this is its first
            // instance.
            //
            // Unconditional rather than in an `Ok` arm: on failure the file is still
            // there and re-reading the folder is still the right answer, and
            // `refresh_trash` re-reads truth either way. Not a full `refresh` —
            // nothing outside `.trash` moved.
            self.library.refresh_trash();
            self.dash.deleting = None;
            return;
        }
        let Some(entry) = self.library.entry_at(path).cloned() else {
            return;
        };
        match store::trash(&root, &entry) {
            Ok(_) => {
                // The open document was just deleted, so the editor must stop
                // treating its path as a place to save — the next autosave would
                // otherwise write it straight back out of the trash.
                if self.session.path.as_deref() == Some(path) {
                    self.session.path = None;
                }
                self.library.refresh();
            }
            Err(e) => self.session.fail(format!("Delete failed: {e}")),
        }
        self.dash.deleting = None;
    }

    /// Create a document and open it.
    ///
    /// **Filed immediately rather than left untitled.** A dashboard whose *New
    /// file* produced something that is not in the library yet would be a screen
    /// that lies about its own contents the moment you go back to it.
    ///
    /// **The destination is an argument rather than something read from here**,
    /// which is what lets one function answer four doors — the sidebar's *New
    /// file* button, the dashed *New file in {project}* card
    /// ([`OndinApp::new_file_card`]), `Ctrl+N` on this screen and `Ctrl+N` in the
    /// editor ([`crate::input::Action::NewDocument`]). The first three know the
    /// nav; the fourth is on a screen that has no nav. See
    /// [`OndinApp::nav_project`] and [`OndinApp::open_document_project`], which
    /// are the two spellings of *the project in front of you*.
    pub(crate) fn new_library_document(&mut self, project: Option<project::Project>) {
        if !self.library_writable() {
            return;
        }
        let root = self.library.root.clone();
        let mut doc = ondin_core::Document::new(self.session.ids.mint());
        match store::file_document(&root, project.as_ref(), "Untitled", &mut doc) {
            Ok(path) => {
                self.library.refresh();
                self.open_path(&path);
            }
            Err(e) => self.session.fail(format!("Could not create the file: {e}")),
        }
    }

    /// Refuse, out loud, to create anything while the base folder is missing.
    ///
    /// **The three doors that make a file are the three that ask** — *New file*,
    /// *New project* and an import — because each of them ends in a
    /// `create_dir_all` that would put an empty folder where the offline library
    /// belongs (`library::state::Library::may_write`). Everything else on this
    /// screen acts on a document in the list, and the list is empty in exactly
    /// the state this guards, so nothing else can be reached to be refused.
    ///
    /// A message rather than a disabled button: the state is rare, temporary and
    /// nothing to do with the control that was pressed, and a greyed *New file*
    /// with no explanation is the worse of the two ways to say "not now".
    fn library_writable(&mut self) -> bool {
        if self.library.may_write() {
            return true;
        }
        self.session
            .fail("The library folder isn't available. Check the base folder in Settings.");
        false
    }

    /// **The project in front of you**, as the dashboard spells it: the nav being
    /// viewed.
    ///
    /// `None` — unfiled, at the root — for *Recent*, *All files*, *Starred* and
    /// the trash, none of which name a destination the screen could promise.
    /// Which is also why the *New file in {project}* card is drawn on a project's
    /// grid and nowhere else.
    /// ⚠️ **`destination`, not `get`** (§15 D557, `[S20.3-L1-03]`) — the same
    /// third gate [`Self::new_file_into`] takes, and it has to be the same or the
    /// card and the act it fires disagree about where a document is going. That
    /// card is the pointer's spelling of this; `Ctrl+N` on this screen is the
    /// keyboard's, and reads this. With the nav on an archived project both now
    /// answer *no project*, which is the honest answer: unfiled, as `Ctrl+N` on
    /// *All files* already is, rather than filed somewhere the app says it cannot
    /// file.
    ///
    /// `pub(crate)` for D557's test — see [`Self::new_file_into`].
    pub(crate) fn nav_project(&self) -> Option<project::Project> {
        match &self.dash.nav {
            Nav::Project(id) => self.library.projects.destination(id).cloned(),
            _ => None,
        }
    }

    /// The same rule as [`OndinApp::nav_project`], as the **editor** spells it:
    /// the open document's own project.
    ///
    /// ⚠️ **Two facts, one rule, and the pair is deliberate rather than a
    /// duplication.** "In front of you" is a different thing on the two screens,
    /// and reading [`Nav`] from the editor would file a new document into
    /// whichever nav the dashboard happened to be left on — a project the user
    /// may not have looked at this session and which nothing on the editor's
    /// screen names. A destination nobody can see is the one kind of wrong answer
    /// a file manager cannot fix by looking, since the file is not where it was
    /// made.
    ///
    /// ⚠️ **And this is why neither of them reads `OndinApp::view`.** That field
    /// *is* exact in production — `OndinApp::ui` returns at the dashboard, so
    /// each screen's doors can only fire on their own screen — but it is not
    /// exact under test: `OndinApp::headless` starts in [`crate::app::View::Editor`]
    /// and every probe in this file draws the dashboard without changing it. A
    /// branch on it would therefore be right in the app and answering for the
    /// wrong screen in every test that exercised it, which is the worst of the
    /// two orders to get this wrong in.
    ///
    /// `None` for a session whose document is not in the library at all — the
    /// never-filed starter the app opens on.
    pub(crate) fn open_document_project(&self) -> Option<project::Project> {
        self.session
            .path
            .as_deref()
            .and_then(|p| self.library.entry_at(p))
            .and_then(|e| self.library.project_of(e))
            .cloned()
    }

    /// The open document follows a library file that has just moved — its
    /// **path** and the **metadata the move wrote to disk**, together
    /// (§15 D429, which is D364's and D379's rule reaching the half that was
    /// missing).
    ///
    /// ⚠️ **Both halves, and the second one is the half that was missing.** Every
    /// one of these operations edits the file: `store::rename` writes a new
    /// `meta.name`, `store::move_to_project` writes a new `meta.project`. The
    /// session's `Document` is never told, and it is the session that the next
    /// save serializes — so re-pointing the path alone means the next save
    /// **undoes the operation**, writing the old value back into the file the
    /// user has just moved. Measured on *Move to project*: the dashboard went on
    /// reporting the old project, because after the save that is what the file
    /// said.
    ///
    /// **One helper rather than the two lines copied a third time**, which is
    /// what the rename arm had and the two `move_to_project` arms did not. The
    /// patch is a closure because the field differs and nothing else does; the
    /// guard, the clone and the write-back are the part that must not diverge.
    ///
    /// A no-op when the moved file is not the one the session has open, which is
    /// the ordinary case — most library operations are performed on documents
    /// nobody is editing.
    fn open_document_follows(
        &mut self,
        was: &Path,
        now: PathBuf,
        patch: impl FnOnce(&mut ondin_core::meta::DocumentMeta),
    ) {
        if self.session.path.as_deref() != Some(was) {
            return;
        }
        self.session.path = Some(now);
        let mut meta = self.session.doc.meta().clone();
        patch(&mut meta);
        self.session.doc.set_meta(meta);
    }

    pub(crate) fn commit_rename(&mut self) {
        let Some((path, name, _)) = self.rename_entry.take() else {
            return;
        };
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let Some(entry) = self.library.entry_at(&path).cloned() else {
            return;
        };
        match store::rename(&entry, name) {
            Ok(moved) => {
                // The open document's own metadata is stale now — the rename was
                // written to the file on disk, not to the copy in memory — and
                // the next save would put the old name straight back.
                self.open_document_follows(&path, moved, |m| m.name = Some(name.to_string()));
                self.library.refresh();
            }
            Err(e) => self.session.fail(format!("Rename failed: {e}")),
        }
    }

    fn create_project(&mut self) {
        let Some(np) = self.dash.new_project.take() else {
            return;
        };
        let name = np.name.trim();
        if name.is_empty() {
            return;
        }
        if !self.library.may_write_projects() {
            self.session
                .fail("projects.json could not be read — not overwriting it");
            return;
        }
        // The other half of the same question, one level up: that one is about a
        // file that could not be parsed, this one about the folder it lives in
        // being gone. ⚠️ **After the `take` above**, so a refusal closes the card
        // rather than leaving it up over a message it cannot act on.
        if !self.library_writable() {
            return;
        }
        let folder = np.folder.then(|| {
            let root = self.library.root.clone();
            naming::unique_stem(name, &|stem| root.join(stem).exists())
        });
        self.library.projects.projects.push(project::Project {
            id: ids::mint(),
            name: name.to_string(),
            color: project::PROJECT_COLORS[np.color.min(project::PROJECT_COLORS.len() - 1)].into(),
            folder,
            created: clock::now(),
            // A project is created into the sidebar, never into the archive —
            // there is no state in which you would make one and hide it.
            archived: false,
        });
        if !self.library.save_projects() {
            self.session.fail("Could not write projects.json");
        }
        self.library.reload_projects();
    }

    /// Write the *Edit project* card back: the name, the colour, the archived
    /// flag, and the folder that follows the name.
    ///
    /// ⚠️ **The folder moves before `projects.json` is written, and the stem that
    /// is recorded is the one the move actually produced.** The two can disagree —
    /// the slug may have stepped around a collision, or the rename may have failed
    /// outright — and recording the *intended* stem would leave the library filing
    /// new documents into a directory that is not the one holding the old ones.
    /// So a failed folder rename is reported and the project keeps the folder it
    /// has, rather than the name change being abandoned: the name is what the user
    /// asked for and the folder is, in this module's own words, only for looking
    /// at.
    fn save_project(&mut self) {
        let Some(ep) = self.dash.edit_project.take() else {
            return;
        };
        let name = ep.name.trim().to_string();
        if name.is_empty() {
            return;
        }
        if !self.library.may_write_projects() {
            self.session
                .fail("projects.json could not be read — not overwriting it");
            return;
        }
        let Some(before) = self.library.projects.get(&ep.id).cloned() else {
            // The project was deleted — by another window, or by a sync client —
            // while the card was open. Nothing to write it back to.
            return;
        };
        // **Nothing edited, nothing written** (§15 D719, `[S20.3-L3-06]`), which
        // is `set_archived`'s `if p.archived == archived { return; }` below,
        // asked of the whole card rather than of one field.
        //
        // The cost of not asking is not one wasted write: `save_projects`
        // rewrites `projects.json`, `reload_projects` re-reads it and `refresh`
        // **rescans the whole library** — a price `refresh`'s own comment accepts
        // on the assumption that something moved. And a failing write here says
        // *"Could not write projects.json"* about a change the user did not make,
        // which `may_write_projects` cannot prevent because it passed a moment
        // earlier and a sync client is enough.
        //
        // ⚠️ **This is the second asker of one predicate and not a second
        // statement of it.** The footer dims the button with the same
        // `differs_from`; this refuses the write however the card was dismissed —
        // Enter reaches here too, and a card that opened before another window
        // changed the project would be dirty at draw time and clean by now.
        if !ep.differs_from(&before) {
            return;
        }
        let root = self.library.root.clone();
        let folder = if before.name == name {
            before.folder.clone()
        } else {
            match project::rename_folder(&root, &before, &name) {
                Ok(f) => f,
                Err(e) => {
                    self.session
                        .fail(format!("The project's folder could not be renamed: {e}"));
                    before.folder.clone()
                }
            }
        };
        // **The open document follows its folder**, exactly as it follows its file
        // through a move: the editor would otherwise keep saving to a path inside a
        // directory that no longer exists, and the next autosave would recreate it.
        if let (Some(old), Some(new)) = (before.folder.as_deref(), folder.as_deref())
            && old != new
            && let Some(open) = self.session.path.clone()
            && let Ok(rest) = open.strip_prefix(root.join(old))
        {
            self.session.path = Some(root.join(new).join(rest));
        }
        if let Some(p) = self
            .library
            .projects
            .projects
            .iter_mut()
            .find(|p| p.id == ep.id)
        {
            p.name = name;
            p.color = ep.color;
            p.archived = ep.archived;
            p.folder = folder;
        }
        if !self.library.save_projects() {
            self.session.fail("Could not write projects.json");
        }
        self.library.reload_projects();
        // The files did not move unless the folder did, and asking twice costs a
        // scan of the library — but a rename that *did* move them leaves every
        // entry holding a stale path, so this is the cheap half of the trade.
        self.library.refresh();
        self.forget_archived_filter(&ep.id, ep.archived);
    }

    /// Archive or unarchive a project without a card in the way.
    fn set_archived(&mut self, id: &str, archived: bool) {
        if !self.library.may_write_projects() {
            self.session
                .fail("projects.json could not be read — not overwriting it");
            return;
        }
        let Some(p) = self
            .library
            .projects
            .projects
            .iter_mut()
            .find(|p| p.id == id)
        else {
            return;
        };
        if p.archived == archived {
            return;
        }
        p.archived = archived;
        if !self.library.save_projects() {
            self.session.fail("Could not write projects.json");
        }
        self.library.reload_projects();
        self.forget_archived_filter(id, archived);
    }

    /// Drop the header's project filter when the project it names has just been
    /// archived.
    ///
    /// ⚠️ **Otherwise the filter is in force and cannot be seen or unset.** Its
    /// dropdown lists only active projects, so the row that would clear it is
    /// gone — and worse, [`OndinApp::filter_applies`] hides the whole control when
    /// no active project is left, at which point *All files* is silently showing a
    /// subset with nothing on screen saying so.
    fn forget_archived_filter(&mut self, id: &str, archived: bool) {
        if archived && self.dash.filter.as_deref() == Some(id) {
            self.dash.filter = None;
        }
    }

    // 🚨 **`remember_dashboard` was deleted here** (§15 D749, `[S20.3-L1-02]`). It
    // read the current `dash.sort`, `dash.list_view` and `dash.nav` into the three
    // `dashboard_*` preferences and saved, and it was called by two controls that are
    // each about *one* of the three: the view toggle and the sort cycle. So pressing
    // either wrote all three, and pressing the sort button while standing in the Trash
    // set *Default page on open* to **Trash** — the one value `library_settings`'
    // picker refuses to offer, under a comment saying *"landing there on launch would
    // be a strange way to start"*.
    //
    // **The maintainer ruled the three are settings and not sticky state**: *"Only
    // settable on Settings, not on navigation or any other action in the dashboard.
    // The whole purpose is what to open when the app loads, not to remember your last
    // page visit."* So `apply_library_settings` is the only writer, and the header's
    // controls change session state that lasts until the app closes.
    //
    // ⚠️ **The finding's other half is closed by the same deletion and is worth
    // knowing about**: read as *sticky* state the old behaviour was not sticky either,
    // because **no nav writer ever called this** — the page was remembered only if you
    // also happened to touch sort or the view. Both readings were broken, which is why
    // this was a defect rather than a naming quibble.
    //
    // ⚠️ **A future *"Remember last"* option is the maintainer's stated idea and is
    // deliberately not this.** It would be a fourth preference choosing between the two
    // behaviours, not a fourth writer of these three.

    fn new_project_modal(&mut self, ctx: &egui::Context, act: &mut Option<Act>) {
        if self.dash.new_project.is_none() {
            return;
        }
        let mut cancel = false;
        let mut create = false;
        let modal = settings::card("new-project", ctx, |ui| {
            ui.set_width(ui::menu_inner_w(settings::CARD_W, settings::PAD));
            // Zero, and every gap in this card is then written out — see
            // `settings::SECTION_AIR` for what accumulating two of them costs.
            ui.spacing_mut().item_spacing.y = 0.0;
            if settings::modal_title(ui, "New project") {
                cancel = true;
            }
            let Some(np) = self.dash.new_project.as_mut() else {
                return;
            };

            ui.add_space(settings::TITLE_GAP);
            field_label(ui, "Project name");
            let field = ui::text_field(
                ui,
                egui::vec2(ui.available_width(), settings::FIELD_H),
                &mut np.name,
                // **Not a plausible project name.** `Kestrel` is the design's
                // fixture, and a placeholder that reads as a real name is one a
                // user has to look twice at to see the field is empty.
                "Your awesome project",
                12.5,
            );
            // ⚠️ **Asked for once, when the modal opens — not every frame.** A
            // `request_focus` on every pass is a field nothing else in the card can
            // take focus from: the switch below it and the two footer buttons could
            // be clicked, but the caret never left, which is what *"once they gain
            // it, clicking anywhere doesn't make them lose focus"* was about.
            //
            // The latch is the whole mechanism, and it is deliberately **not**
            // guarded on `Memory::focused()` being empty: something behind the
            // backdrop may still hold the caret on the frame this opens, and a
            // guard like that would silently never fire in that case. The field is
            // new this frame, so *once* is unambiguous.
            if !np.name_focused {
                field.request_focus();
                np.name_focused = true;
            }

            ui.add_space(settings::ROW_GAP);
            field_label(ui, "Colour");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for (i, hex) in project::PROJECT_COLORS.iter().enumerate() {
                    let (rect, resp) =
                        ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
                    ui.painter().circle_filled(rect.center(), 8.0, swatch(hex));
                    if np.color == i {
                        ui.painter().circle_stroke(
                            rect.center(),
                            10.0,
                            egui::Stroke::new(2.0, theme::color::text_a(140)),
                        );
                    }
                    if resp.clicked() {
                        np.color = i;
                    }
                }
            });

            ui.add_space(settings::ROW_GAP);
            // **A switch, not a checkbox** — the design's toggle, and the control
            // the Typography panel's feature list and the editor's own Settings
            // card already use for exactly this shape of question.
            if ui::switch_row(
                ui,
                "Add a matching folder on disk",
                np.folder,
                settings::SWITCH_H,
            )
            .clicked()
            {
                np.folder = !np.folder;
            }
            settings::caption(
                ui,
                "Files stay in the project either way — the folder is only for looking at.",
            );

            ui.add_space(settings::FOOTER_GAP);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let enabled = !np.name.trim().is_empty();
                if settings::text_button(
                    ui,
                    "Create project",
                    if enabled {
                        FieldButton::On
                    } else {
                        FieldButton::Disabled
                    },
                )
                .clicked()
                    && enabled
                {
                    create = true;
                }
                if settings::text_button(ui, "Cancel", FieldButton::Off).clicked() {
                    cancel = true;
                }
            });
        });
        // Escape *and* a click on the backdrop, both of which mean Cancel — the
        // design hangs `closeNewProject` on the backdrop, and `should_close` is
        // also what makes Escape answer the topmost modal rather than every open
        // one at once. Read after the body, because it consumes the press.
        if modal.should_close() {
            cancel = true;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            create = true;
        }
        if cancel {
            self.dash.new_project = None;
        } else if create {
            *act = Some(Act::CreateProject);
        }
    }

    /// *Edit project* — the New project card with two rows swapped (§15 D379).
    ///
    /// **The same card, deliberately spelled out again rather than shared with
    /// [`OndinApp::new_project_modal`].** The two agree on the name field, the
    /// swatches and the footer, and disagree on the switch, the button's word and
    /// what the state behind them is — so a shared function would be one that took
    /// a flag and branched three times, which is two cards in a trench coat. What
    /// keeps them from drifting is that neither writes a number of its own: the
    /// gaps, the field height and the buttons all come from `settings`.
    ///
    /// ⚠️ **There is no *Create folder* row.** The folder is a decision made once,
    /// at creation, and the honest way to offer it later would be to make it, move
    /// every one of the project's files into it, and say so — which is a different
    /// card. What the rename *does* do is take an existing folder with it
    /// (`library::project::rename_folder`).
    fn edit_project_modal(&mut self, ctx: &egui::Context, act: &mut Option<Act>) {
        if self.dash.edit_project.is_none() {
            return;
        }
        let mut cancel = false;
        let mut save = false;
        // The project as stored, taken **before** the closure because that
        // borrows `self` mutably for the card's own state (§15 D719). Cloned
        // rather than borrowed for the same reason.
        let stored = self
            .dash
            .edit_project
            .as_ref()
            .and_then(|ep| self.library.projects.get(&ep.id).cloned());
        let modal = settings::card("edit-project", ctx, |ui| {
            ui.set_width(ui::menu_inner_w(settings::CARD_W, settings::PAD));
            ui.spacing_mut().item_spacing.y = 0.0;
            if settings::modal_title(ui, "Edit project") {
                cancel = true;
            }
            let Some(ep) = self.dash.edit_project.as_mut() else {
                return;
            };

            ui.add_space(settings::TITLE_GAP);
            field_label(ui, "Project name");
            let field = ui::text_field(
                ui,
                egui::vec2(ui.available_width(), settings::FIELD_H),
                &mut ep.name,
                "Your awesome project",
                12.5,
            );
            // The latch, for `NewProject::name_focused`'s reason — and it matters
            // more here, where the field opens holding text: a `request_focus` on
            // every frame would put the caret back at the same place after every
            // click, which reads as the field refusing to be edited.
            if !ep.name_focused {
                field.request_focus();
                ep.name_focused = true;
            }

            ui.add_space(settings::ROW_GAP);
            field_label(ui, "Colour");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for hex in project::PROJECT_COLORS {
                    let (rect, resp) =
                        ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
                    ui.painter().circle_filled(rect.center(), 8.0, swatch(hex));
                    // ⚠️ **Matched on the value, not on an index** — see
                    // [`EditProject::color`]. A project whose file names a colour
                    // outside these nine rings nothing, which is correct: there is
                    // no swatch to ring.
                    if ep.color.eq_ignore_ascii_case(hex) {
                        ui.painter().circle_stroke(
                            rect.center(),
                            10.0,
                            egui::Stroke::new(2.0, theme::color::text_a(140)),
                        );
                    }
                    if resp.clicked() {
                        ep.color = hex.to_string();
                    }
                }
            });

            ui.add_space(settings::ROW_GAP);
            if ui::switch_row(ui, "Archive project", ep.archived, settings::SWITCH_H).clicked() {
                ep.archived = !ep.archived;
            }
            settings::caption(
                ui,
                "Hidden from the sidebar and from every project list. Its files stay where they are.",
            );

            ui.add_space(settings::FOOTER_GAP);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                // **Lit only when there is something to save** (§15 D719,
                // `[S20.3-L3-06]`). This was `!ep.name.trim().is_empty()` alone,
                // so opening the card and clicking *Save changes* — or pressing
                // Enter — rewrote `projects.json` and rescanned the library for
                // nothing. `settings::modal_footer`, the library Settings card's
                // own footer, has said `&& dirty` all along; this is that rule
                // reaching the card that writes the most.
                //
                // A project deleted from under the card answers `None` and dims
                // the button, which is the honest drawing of it: `save_project`
                // has nothing to write it back to either.
                let enabled = !ep.name.trim().is_empty()
                    && stored
                        .as_ref()
                        .is_some_and(|before| ep.differs_from(before));
                if settings::text_button(
                    ui,
                    "Save changes",
                    if enabled {
                        FieldButton::On
                    } else {
                        FieldButton::Disabled
                    },
                )
                .clicked()
                    && enabled
                {
                    save = true;
                }
                if settings::text_button(ui, "Cancel", FieldButton::Off).clicked() {
                    cancel = true;
                }
            });
        });
        if modal.should_close() {
            cancel = true;
        }
        // ⚠️ **Enter is gated on the same predicate the button is**, which the New
        // project card does not bother with: there, Enter on an empty name closes a
        // card holding nothing. Here it would close a card holding *edits* and
        // silently drop them, because a save with no name is refused.
        if ctx.input(|i| i.key_pressed(egui::Key::Enter))
            && self
                .dash
                .edit_project
                .as_ref()
                .is_some_and(|ep| !ep.name.trim().is_empty())
        {
            save = true;
        }
        if cancel {
            self.dash.edit_project = None;
        } else if save {
            *act = Some(Act::SaveProject);
        }
    }

    /// *Delete project* — and, more importantly, what happens to its files.
    ///
    /// ⚠️ **A project is the one thing here with no trash.** Deleting a document
    /// moves it; deleting a project removes a row from `projects.json` and there
    /// is nothing to restore, because the project was never anything but that
    /// row. So this modal has to be the whole confirmation, and the *files*
    /// question has to be answered before it closes rather than discovered
    /// afterwards.
    fn delete_project_modal(&mut self, ctx: &egui::Context, act: &mut Option<Act>) {
        let Some(dp) = self.dash.deleting_project.as_ref() else {
            return;
        };
        let id = dp.id.clone();
        let Some(project) = self.library.projects.get(&id).cloned() else {
            // The project vanished under us — another window, or a synced
            // `projects.json`. Closing is the whole of the right answer.
            self.dash.deleting_project = None;
            return;
        };
        let count = self.library.file_count(&id);
        // `active`, for `move_modal`'s reason: this list is a set of destinations,
        // and an archived project is not one. No exception is owed here — the
        // project being deleted is the one this list excludes anyway.
        let others: Vec<(String, String, String)> = self
            .library
            .projects
            .active()
            .filter(|p| p.id != id)
            .map(|p| (p.id.clone(), p.name.clone(), p.color.clone()))
            .collect();
        let mut cancel = false;
        let mut confirm = false;

        let modal = settings::card("delete-project", ctx, |ui| {
            ui.set_width(ui::menu_inner_w(settings::CARD_W, settings::PAD));
            if settings::modal_title(ui, "Delete project") {
                cancel = true;
            }
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(format!("“{}” can't be brought back.", project.name))
                    .size(12.0)
                    .color(theme::text::MUTED),
            );
            let Some(dp) = self.dash.deleting_project.as_mut() else {
                return;
            };

            if count > 0 {
                ui.add_space(12.0);
                // The design's toggle, and the same switch the two cards beside
                // this one now use. It reads the *inverse* of the field it writes
                // — see [`DeleteProject`] for why the field is `keep_files`.
                // ⚠️ **The row does not say where they go, deliberately.** It read
                // "… too — they move to Trash", which is true and is the reassuring
                // half of a destructive switch: *Delete* on a document already means
                // the trash everywhere else on this screen, and spelling it out here
                // made the loudest control on the card read as the mildest.
                if ui::switch_row(
                    ui,
                    &format!("Delete its {count} file(s) too"),
                    !dp.keep_files,
                    settings::SWITCH_H,
                )
                .clicked()
                {
                    dp.keep_files = !dp.keep_files;
                }
                if dp.keep_files {
                    ui.add_space(8.0);
                    picker_row(
                        ui,
                        "Otherwise move them to",
                        "transfer-to",
                        &dp.transfer_to
                            .as_ref()
                            .and_then(|t| others.iter().find(|(oid, ..)| oid == t))
                            .map(|(_, name, _)| name.clone())
                            .unwrap_or_else(|| "No project".into()),
                        |ui| {
                            ui.selectable_value(&mut dp.transfer_to, None, "No project");
                            for (oid, name, _) in &others {
                                ui.selectable_value(&mut dp.transfer_to, Some(oid.clone()), name);
                            }
                        },
                    );
                }
            } else {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("It has no files in it.")
                        .size(11.0)
                        .color(theme::text::FAINT),
                );
            }

            // ⚠️ **The folder is kept and the card no longer says so.** The line
            // read "The folder “x” stays on disk — Ondin only removes the project",
            // which is true, is the answer to a question nobody had asked yet, and
            // was the fourth block of prose on a card whose whole job is one
            // question and one switch. Removed as *busy*: the folder surviving is
            // the harmless outcome, and a confirmation should spend its words on
            // what it is about to destroy. It is recorded in `library::project` and
            // in §15 instead of on screen.

            ui.add_space(settings::FOOTER_GAP);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                // **Red where the other cards are blue** — *Create project* and
                // *Save changes* are `FieldButton::On`, so the confirming button of
                // the card that destroys something is `Danger`, its twin on the
                // other ramp. Two grey buttons side by side, one of which removes a
                // project with no trash to come back from, is what this was.
                if settings::text_button(ui, "Delete project", FieldButton::Danger).clicked() {
                    confirm = true;
                }
                if settings::text_button(ui, "Cancel", FieldButton::Off).clicked() {
                    cancel = true;
                }
            });
        });

        // Escape and the backdrop, both Cancel — see `new_project_modal`.
        if modal.should_close() {
            cancel = true;
        }
        if cancel {
            self.dash.deleting_project = None;
        } else if confirm {
            let dp = self
                .dash
                .deleting_project
                .as_ref()
                .expect("checked at the top");
            *act = Some(Act::DeleteProject {
                id: dp.id.clone(),
                transfer_to: dp.transfer_to.clone(),
                keep_files: dp.keep_files,
            });
        }
    }

    /// *Move to project* — a list of the projects plus *No project*.
    ///
    /// **A modal rather than a submenu on the ⋮.** A submenu would have to hold
    /// an arbitrary number of projects inside a popup that is already inside a
    /// popup, and this is the one action in the menu that needs a second choice
    /// rather than a confirmation.
    fn move_modal(&mut self, ctx: &egui::Context, act: &mut Option<Act>) {
        let Some(path) = self.dash.moving.clone() else {
            return;
        };
        let current = self
            .library
            .entry_at(&path)
            .and_then(|e| e.meta.project.clone());
        let name = self
            .library
            .entry_at(&path)
            .map(|e| e.display_name())
            .unwrap_or_default();
        // ⚠️ **Archived projects are not offered as a destination**, which is the
        // one place that rule bites rather than merely tidies: a document cannot
        // be filed into an archived project without unarchiving it first.
        //
        // ⚠️ **Except the one this document is already in.** A list of
        // destinations that omits where the file currently *is* would draw the
        // sheet with nothing selected — i.e. would tell the user the document was
        // unfiled — and the row is the only place the modal says otherwise. So the
        // rule is "no archived project you are not already in", and the exception
        // is a statement of fact rather than an offer.
        let mut projects: Vec<project::Project> = self.library.projects.active().cloned().collect();
        if let Some(home) = current
            .as_deref()
            .and_then(|id| self.library.projects.get(id))
            && home.archived
        {
            projects.push(home.clone());
        }
        let mut chosen: Option<Option<String>> = None;
        let mut cancel = false;
        let modal = settings::card("move-to-project", ctx, |ui| {
            ui.set_width(ui::menu_inner_w(settings::CARD_W, settings::PAD));
            if settings::modal_title(ui, "Move to project") {
                cancel = true;
            }
            ui.label(
                egui::RichText::new(format!("“{name}”"))
                    .size(11.0)
                    .color(theme::text::DIM),
            );
            ui.add_space(10.0);
            egui::ScrollArea::vertical()
                .max_height(240.0)
                .show(ui, |ui| {
                    let row =
                        |ui: &mut egui::Ui, label: &str, dot: Option<&str>, id: Option<String>| {
                            let selected = current == id;
                            let (rect, resp) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), NAV_H),
                                egui::Sense::click(),
                            );
                            if selected || resp.hovered() {
                                ui.painter().rect_filled(
                                    rect,
                                    egui::CornerRadius::same(6),
                                    theme::color::text_a(if selected { 20 } else { 12 }),
                                );
                            }
                            if let Some(hex) = dot {
                                ui.painter().circle_filled(
                                    egui::pos2(rect.left() + 14.0, rect.center().y),
                                    4.0,
                                    swatch(hex),
                                );
                            }
                            ui.painter().text(
                                egui::pos2(rect.left() + 28.0, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                label,
                                egui::FontId::proportional(12.0),
                                theme::text::STRONG,
                            );
                            if selected {
                                ui.painter().text(
                                    egui::pos2(rect.right() - 12.0, rect.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    icon::CHECK,
                                    theme::icon_font(12.0),
                                    color::ACCENT,
                                );
                            }
                            resp.clicked()
                        };
                    if row(ui, "No project", None, None) {
                        chosen = Some(None);
                    }
                    for p in &projects {
                        if row(ui, &p.name, Some(&p.color), Some(p.id.clone())) {
                            chosen = Some(Some(p.id.clone()));
                        }
                    }
                });
            ui.add_space(settings::FOOTER_GAP);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if settings::text_button(ui, "Cancel", FieldButton::Off).clicked() {
                    cancel = true;
                }
            });
        });
        // Escape and the backdrop, both Cancel — see `new_project_modal`.
        if modal.should_close() {
            cancel = true;
        }
        if cancel {
            self.dash.moving = None;
        } else if let Some(target) = chosen {
            // ⚠️ **Clicking the project it is already in still moves it**, which
            // `store::move_to_project` turns into a no-op that returns the same
            // path. Special-casing it here would be one more branch saying what
            // that function already says.
            *act = Some(Act::MoveTo(path, target));
        }
    }

    /// The library's own Settings — storage, and how the dashboard opens.
    ///
    /// ⚠️ **A draft that applies on Save, and here that is load-bearing rather
    /// than a habit.** Two of its fields together are an *operation*: a base
    /// folder plus *Move my files there* moves every document the user owns, and
    /// an apply-as-you-type version would start doing that while they were halfway
    /// through typing the path. The draft is what gives it a moment where the
    /// answer is complete and nothing has happened yet.
    ///
    /// The editor's Settings card stages for a softer version of the same reason
    /// (`crate::settings`), and the two are now the same card: same
    /// [`settings::card_modal`], same [`settings::section`] headers, same footer
    /// with the commit lit only when the draft differs from what is saved. It was
    /// reported as *"Settings is a mess that looks nothing like the layout in
    /// design/"*, and the cause was that it had been built out of egui's default
    /// modal and egui's default checkboxes and combo boxes rather than out of the
    /// app's own kit (§15 D368).
    ///
    /// ⚠️ **"The two" is Settings and Settings, and the sentence above reads as
    /// though it covered this screen** (§15 D719, `[S20.3-L3-06]`).
    /// `new_project_modal` and `edit_project_modal` each hand-roll their footer
    /// rather than calling [`settings::modal_footer`], so *"same footer"* has
    /// never been true of them — the same control written three times, which is
    /// how the rule that matters came to be in only one of them. **The *Edit
    /// project* card's commit is lit on `EditProject::differs_from` now**, so the
    /// two agree about the rule and still not about the footer; folding both
    /// project cards onto `modal_footer` is the remaining half and is a layout
    /// change rather than a behavioural one.
    ///
    /// The Cancel/Save pair is therefore load-bearing rather than convention,
    /// and it is what the design draws.
    fn library_settings_modal(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.library_settings.clone() else {
            return;
        };
        let saved = LibrarySettings::from_prefs(&self.prefs, &self.library.root);
        let resolved_now = self.library.root.display().to_string();
        let mut decision: Option<settings::Decision> = None;
        // **Cloned out and answered after the card**, which is `decision`'s own
        // shape: the closure below cannot borrow `self` while the card holds a
        // `&mut` to build it, and a retry runs a whole migration — the one thing
        // that must not happen inside a paint (§15 D810).
        let stranded = self.stranded.clone();
        let mut retry = false;

        let modal = settings::card("library-settings", ctx, |ui| {
            ui.set_width(ui::menu_inner_w(settings::CARD_W, settings::PAD));
            // Zero, and every vertical gap in this card is then written out — see
            // `settings::SECTION_AIR` for the 32-against-26 this prevents.
            ui.spacing_mut().item_spacing.y = 0.0;

            if settings::modal_title(ui, "Settings") {
                decision = Some(settings::Decision::Cancel);
            }

            // --- storage ---------------------------------------------------
            ui.add_space(settings::TITLE_GAP - settings::SECTION_AIR);
            settings::section(ui, "Storage");
            field_label(ui, "Base folder");
            // ⚠️ **The button is allocated *first*, in a right-to-left row.** A
            // `horizontal` centres each item against the row height it knows when
            // that item is placed, and the field has to be told a width rather than
            // taking `available_width` after something else has eaten into it —
            // right-to-left gives the button the margin and leaves the field the
            // rest, which is `picker_row`'s shape and for its reason.
            //
            // **Disabled when the typed path is not a folder that exists**, which
            // is the honest state for a control that opens one: the field is seeded
            // with the resolved root, so this is only ever off while a new path is
            // being typed, and off says "not yet" where a click that silently did
            // nothing would say the button was broken.
            let open_dir = PathBuf::from(d.base_folder.trim());
            let can_open = !d.base_folder.trim().is_empty() && open_dir.is_dir();
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = settings::COL_GAP;
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui::field_button(
                        ui,
                        icon::FOLDER_OPEN,
                        settings::FIELD_H,
                        15.0,
                        FieldButton::enabled_if(can_open),
                    )
                    .on_hover_text("Open the library folder")
                    .clicked()
                        && can_open
                    {
                        crate::panels::show_in_file_browser(&open_dir);
                    }
                    ui::text_field(
                        ui,
                        egui::vec2(ui.available_width(), settings::FIELD_H),
                        &mut d.base_folder,
                        &resolved_now,
                        12.5,
                    );
                });
            });
            // **No caption.** It read "Point this at a Drive, Dropbox or OneDrive
            // folder and the library syncs" — a true thing, and an advertisement
            // rather than an explanation: the field is labelled, the button beside
            // it opens what it names, and neither needs a third party's product to
            // be understood. The syncable base folder is the reason the whole
            // library is shaped the way it is (`library::project`) and that belongs
            // in the docs, not over a text box.
            // **The migrate switch only exists once the path has changed**, which
            // is the difference between an option and a question nobody asked:
            // offering "move my files" when the folder is not moving is offering
            // to move them to where they already are.
            //
            // ⚠️ Its caption changes with it rather than staying put, because the
            // two answers are both consequences and neither is the default one: a
            // row reading only "Move my existing files there" leaves *off* meaning
            // unstated, and off is the destructive-sounding half.
            if d.base_folder.trim() != resolved_now && !d.base_folder.trim().is_empty() {
                ui.add_space(settings::ROW_GAP);
                if ui::switch_row(
                    ui,
                    "Move my existing files there",
                    d.migrate,
                    settings::SWITCH_H,
                )
                .clicked()
                {
                    d.migrate = !d.migrate;
                }
                settings::caption(
                    ui,
                    if d.migrate {
                        "Documents, version history, the trash and the project list \
                         all move. Nothing is overwritten."
                    } else {
                        "Your files stay where they are; the new folder starts empty."
                    },
                );
            }

            // **What the last migration left behind** (§15 D810). Only drawn when
            // there is something to say, which is why it is below the switch
            // rather than beside it: the switch is a question about the *next*
            // move and this is the report on the last one.
            //
            // ⚠️ **Named files and a count, not a count.** `Moved::summary` put
            // the first one in the status line and the rest were unrecoverable
            // the moment anything else wrote a message; what the user needs in
            // order to act is which files and which folder, which is exactly what
            // a status line cannot hold.
            if let Some(report) = &stranded {
                ui.add_space(settings::ROW_GAP);
                warn_caption(
                    ui,
                    &match report.failed.len() {
                        1 => "1 file could not be moved and is still in:".to_string(),
                        n => format!("{n} files could not be moved and are still in:"),
                    },
                );
                settings::caption(ui, &report.from.display().to_string());
                // **Capped and then said out loud**, rather than scrolled: this
                // card has no scroll area and a modal that grows past the window
                // for a list nobody can act on twenty rows at a time is worse than
                // one that says how many it is not showing. The folder above is
                // what the user opens; these are the names to look for in it.
                for path in report.failed.iter().take(STRANDED_ROWS) {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    settings::caption(ui, &format!("• {name}"));
                }
                if let Some(rest) = report.failed.len().checked_sub(STRANDED_ROWS)
                    && rest > 0
                {
                    settings::caption(ui, &format!("…and {rest} more"));
                }
                ui.add_space(settings::ROW_GAP);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        retry = ui::label_button(
                            ui,
                            "Try again",
                            egui::vec2(88.0, settings::FIELD_H),
                            FieldButton::Off,
                        )
                        .on_hover_text("Move what is left of the old library across")
                        .clicked();
                    });
                });
            }

            ui.add_space(settings::ROW_GAP);
            // **The label inside the field, in a two-column row** — the shape
            // `settings::nudge_field` uses for *Step* and *Shift*, and the reason
            // is the same one it gives: a caption over a value field is a shape
            // this app does not otherwise use, and two spellings side by side are
            // what make a card look assembled from a different kit. *Base folder*
            // above keeps its caption because a path has no width to give a prefix.
            let half = egui::vec2(
                (ui.available_width() - settings::COL_GAP) / 2.0,
                settings::FIELD_H,
            );
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = settings::COL_GAP;
                whole_field(ui, half, "Autosave", "s", &mut d.autosave_secs, 3600);
                whole_field(ui, half, "Trash", "d", &mut d.trash_keep_days, 365);
            });
            // ⚠️ **Zero is a real setting in both fields and means something
            // alarming in both**, so it is said out loud rather than left to be
            // inferred from a `0`. Kept from the card this replaced; it was the one
            // thing that version had which the design does not.
            match (d.autosave_secs, d.trash_keep_days) {
                (0, 0) => warn_caption(
                    ui,
                    "Autosave is off, and deleted files are removed straight away.",
                ),
                (0, _) => warn_caption(ui, "Autosave is off."),
                (_, 0) => warn_caption(ui, "Deleted files are removed straight away."),
                _ => {}
            }

            ui.add_space(settings::ROW_GAP);
            if ui::switch_row(
                ui,
                "Keep version history",
                d.version_history,
                settings::SWITCH_H,
            )
            .on_hover_text("Ctrl+S pins a version you can go back to")
            .clicked()
            {
                d.version_history = !d.version_history;
            }

            // --- library ---------------------------------------------------
            settings::section(ui, "Library");
            picker_row(
                ui,
                "Default page on open",
                "default-page",
                d.default_page.label(),
                |ui| {
                    // ⚠️ Only the three that always exist. *Trash* is not offered
                    // — landing there on launch would be a strange way to start —
                    // and a project is not offered because it can be deleted
                    // between launches (`Nav::from_id`).
                    for nav in [Nav::Recent, Nav::All, Nav::Starred] {
                        let label = nav.label();
                        ui.selectable_value(&mut d.default_page, nav, label);
                    }
                },
            );
            ui.add_space(settings::ROW_GAP);
            picker_row(
                ui,
                "Default view on open",
                "default-view",
                if d.default_list_view { "List" } else { "Grid" },
                |ui| {
                    ui.selectable_value(&mut d.default_list_view, false, "Grid");
                    ui.selectable_value(&mut d.default_list_view, true, "List");
                },
            );
            ui.add_space(settings::ROW_GAP);
            picker_row(
                ui,
                "Default sort",
                "default-sort",
                d.default_sort.label(),
                |ui| {
                    for s in Sort::ALL {
                        ui.selectable_value(&mut d.default_sort, s, s.label());
                    }
                },
            );

            // --- general ---------------------------------------------------
            settings::section(ui, "General");
            if ui::switch_row(
                ui,
                "Reopen the last file on launch",
                d.reopen_last,
                settings::SWITCH_H,
            )
            .clicked()
            {
                d.reopen_last = !d.reopen_last;
            }

            // --- footer ----------------------------------------------------
            if let Some(x) = settings::modal_footer(ui, d != saved) {
                decision = Some(x);
            }
        });

        // The backdrop and Escape, both of which mean Cancel. Read *after* the
        // body, because `should_close` consumes the Escape press.
        if modal.should_close() {
            decision = Some(settings::Decision::Cancel);
        }

        match decision {
            // Cancel throws the draft away — which is the whole point of there
            // being one.
            Some(settings::Decision::Cancel) => self.library_settings = None,
            Some(settings::Decision::Save) => {
                self.library_settings = Some(d);
                self.apply_library_settings();
            }
            // Still open: keep whatever was typed into it this frame.
            None => self.library_settings = Some(d),
        }
        // **After the match, so a retry cannot race a Save** (§15 D810). Both
        // touch the library, and `apply_library_settings` is the one that may
        // *re-point* it — running the retry first would move files into a root the
        // next line is about to change. Cancel takes the draft away and leaves the
        // report standing, which is right: the files are still stranded whatever
        // the user did with the card.
        if retry {
            self.retry_migration();
        }
    }

    /// Re-point the open document, and *reopen last*, at where a migration put
    /// them.
    ///
    /// ⚠️ **Without this the move quietly forks the open document.**
    /// `session.path` is absolute, so it went on naming a file in the folder the
    /// library had just abandoned: the next save recreated the document there, and
    /// the new root already held a copy carrying the same `meta.id`. Two files,
    /// one identity — the library listed only the pre-move one, and every edit
    /// made afterwards went to the one it did not list. Version history and the
    /// cover cache are keyed by that id, so both then answered for whichever copy
    /// was scanned.
    ///
    /// **Looked up in the map rather than re-joined under the new root**, because
    /// `relocate` renames into a collision: a reconstructed path can name a
    /// *different* document that happened to be called the same thing at the
    /// destination.
    ///
    /// `prefs.last_document` travels with it, or *reopen last* reopens the
    /// abandoned copy on the next launch — the same bug, one launch later.
    ///
    /// **A function since §15 D810**, because the retry below is a second
    /// migration and owes everything the first one did. It was inline in
    /// `apply_library_settings` when there was only one caller.
    fn follow_moved_documents(&mut self, moved: &crate::library::relocate::Moved) {
        for path in [self.session.path.clone(), self.prefs.last_document.clone()]
            .into_iter()
            .flatten()
        {
            let Some(now) = moved.destination_of(&path).map(Path::to_path_buf) else {
                continue;
            };
            if self.session.path.as_deref() == Some(path.as_path()) {
                self.session.path = Some(now.clone());
            }
            if self.prefs.last_document.as_deref() == Some(path.as_path()) {
                self.prefs.last_document = Some(now);
            }
        }
    }

    /// Run the stranded half of the last migration again (§15 D810).
    ///
    /// **The recovery `relocate`'s own defence already assumed somebody had.**
    /// That module argues a migration *"run twice leaves duplicates rather than
    /// being idempotent, which is the right way round"* — true, and it assumes a
    /// second run is reachable, where until this the only route was a three-step
    /// dance back through Settings whose middle step looks exactly like the
    /// operation that lost the files.
    ///
    /// **It is not idempotent and does not need to be**, because a successful
    /// move deletes its source: the first run's successes are no longer in the old
    /// folder, so this walk meets only what was left behind. What fails again
    /// fails for the same reason — a name already taken at the destination is
    /// `Collision::Stranded`'s case and stays stranded however many times it is
    /// asked.
    ///
    /// ⚠️ **It owes `follow_moved_documents` and a `refresh`.** A retry moves
    /// documents, so the open one can be among them, and the library's listing is
    /// a scan of a folder that has just changed. The `disk_settle` is the same one
    /// the first run takes and for the same reason.
    ///
    /// 🚨 **Two refusals, and each keeps the report** (§15 D846). A report is
    /// answerable only against the library it was made for: run against any other
    /// folder it moves files somewhere nothing lists them (`[X1.2-L1-01]`). And an
    /// old folder that cannot be reached is not an old folder with nothing left in
    /// it — `relocate` answers *"Nothing to move"* for a source that does not
    /// exist, and this used to take that at its word and **clear the list**, the
    /// only record of which files were left behind and where, while a network
    /// share was merely down (`[R3-L5-02]`). §15 D384's rule, a third time: an
    /// empty listing is not evidence.
    pub(crate) fn retry_migration(&mut self) {
        let Some(stranded) = self.stranded.clone() else {
            return;
        };
        if stranded.to != self.library.root {
            self.session.fail(format!(
                "That list is from a move into {}, which is not the library folder any more — \
                 nothing was moved.",
                stranded.to.display()
            ));
            return;
        }
        if !crate::library::scan::readable(&stranded.from) {
            self.session.fail(format!(
                "Could not reach {} — nothing was moved, and the list is kept.",
                stranded.from.display()
            ));
            return;
        }
        self.disk_settle();
        let moved = crate::library::relocate::relocate(&stranded.from, &stranded.to);
        self.follow_moved_documents(&moved);
        self.session.info(moved.summary());
        self.stranded = (!moved.failed.is_empty()).then(|| Stranded {
            failed: moved.failed.clone(),
            ..stranded
        });
        self.library.refresh();
        self.prefs.save();
    }

    /// Write the settings draft into preferences, moving the library if asked.
    pub(crate) fn apply_library_settings(&mut self) {
        let Some(d) = self.library_settings.take() else {
            return;
        };
        self.prefs.autosave_secs = d.autosave_secs;
        self.prefs.trash_keep_days = d.trash_keep_days;
        self.prefs.version_history = d.version_history;
        self.prefs.dashboard_page = d.default_page.id().to_string();
        self.prefs.dashboard_list_view = d.default_list_view;
        self.prefs.dashboard_sort = d.default_sort.id().to_string();
        self.prefs.reopen_last = d.reopen_last;

        let typed = d.base_folder.trim();
        let requested = if typed.is_empty() {
            None
        } else {
            Some(PathBuf::from(typed))
        };
        let new_root =
            crate::library::root(requested.as_deref()).unwrap_or_else(|| self.library.root.clone());
        // 🚨 **The folder is made *first*, and its failure is what refuses the
        // change** (§15 D700, `[S20.3-L1-05]`). This used to be a
        // `let _ = create_dir_all(…)` at the bottom of the block, after the
        // migration had already run and after `prefs.base_folder` had already
        // been assigned — so a path naming an existing **file** (a mis-copied
        // path, a `.lnk`, a sync placeholder) was adopted anyway. Measured: the
        // root became `…\not-a-folder.txt`, `may_write` went false,
        // `root_unavailable` true, `entries` 0, the preference was **persisted**
        // so the state survived a relaunch, and `apply_library_settings` said
        // nothing at all.
        //
        // ⚠️ **Before the migration and not after it**, which is the half that
        // matters: with *Move my existing files there* on, the old order had
        // `relocate` run against a destination that cannot be a directory.
        //
        // The comment that used to sit on the old line is still the reason this
        // call exists at all, and is kept below.
        let creatable = new_root == self.library.root || std::fs::create_dir_all(&new_root).is_ok();
        if !creatable {
            // Only the base folder is refused. The other settings on this card
            // are unrelated and were applied above; discarding them because a
            // path was mistyped would be a second surprise.
            self.session.fail(format!(
                "Could not use {} as the library folder — it is a file, or could not be created. \
                 The library folder is unchanged.",
                new_root.display()
            ));
        } else if new_root != self.library.root {
            let old_root = self.library.root.clone();
            self.prefs.base_folder = requested;
            // ⚠️ **The in-memory textures go.** They are keyed correctly and
            // would still be *right*, but they are for a set of documents that is
            // no longer on screen and would sit in memory for the session.
            //
            // 🚨 **This used to add "and the disk cache stays … which is what
            // makes a library that has been seen before come back with its
            // covers already drawn", and that is false** (§15 D708,
            // `[S1.3-L8-07]`). `Covers::sweep` runs on the very next
            // `go_to_dashboard` with a keep set built from the **new** library
            // alone, over one un-namespaced directory, so the old library's
            // covers are deleted rather than kept. The claim was written in two
            // places and both said it; `Covers::clear`'s doc carries the whole
            // account and the two options for making it true.
            //
            // 🚨 **And it is here, before the migration, rather than after the
            // re-open where it used to be** (§15 D845, `[X1.1-L1-02]`). `clear`
            // ends the cover renderer, and the renderer is a second worker aimed
            // at the old root — `disk_settle` below exists for the first. A cover
            // job queued before the move read its document at the pre-move path,
            // failed, and cached `Unreadable` under a key that does not contain
            // the path, so the moved and perfectly healthy document wore the red
            // *"will not open"* mark. **Cancelled rather than waited for**: its
            // answers are about paths that are about to stop existing.
            self.covers.clear();
            if d.migrate {
                // ⚠️ **The snapshot writer is drained before the folder moves.**
                // `relocate` carries `.recovery/` across on this thread, and the
                // worker may still be aimed at the old root — so a write in
                // flight either lands in a folder that has just been emptied, or
                // is copied over and then deleted from under the copy, leaving
                // the new library holding a snapshot of work that was saved
                // before the user ever opened Settings. The wait is one
                // document's serialise, at the start of an operation that is
                // about to move every file they own.
                self.disk_settle();
                let moved = crate::library::relocate::relocate(&old_root, &new_root);
                // The open document and *reopen last* follow the move; the whole
                // argument is on [`OndinApp::follow_moved_documents`], which the
                // retry in the modal calls too (§15 D810).
                self.follow_moved_documents(&moved);
                self.session.info(moved.summary());
                // **Kept, because the status line is a sentence and this is a
                // list** (§15 D810). `summary` names the first stranded file and
                // the count; the next status message replaces it, and the other
                // twenty-two are then findable only by hand. Assigned in both
                // directions so a clean migration *clears* a previous one's
                // report — the modal must not go on offering to retry a move that
                // has already succeeded.
                self.stranded = (!moved.failed.is_empty()).then(|| Stranded {
                    from: old_root.clone(),
                    to: new_root.clone(),
                    failed: moved.failed.clone(),
                });
            } else {
                // 🚨 **And a change that does not migrate clears it too**
                // (§15 D846, `[X1.2-L1-01]`). "Both directions" above was both
                // directions *of a migration*, and this branch had neither: a
                // report from moving A to B survived pointing the library at C,
                // so *Try again* ran `relocate(A, B)` into a folder that was no
                // longer the library, re-pointed the open document there, and
                // then cleared the only list of where the files had gone. The
                // files it names are still in A, which is exactly where the user
                // left them by choosing not to move them.
                self.stranded = None;
            }
            // ⚠️ **Why the folder is created at all — the call itself has moved
            // to the top of this block** (§15 D700). It is not tidiness: it is
            // what keeps *pointing somewhere new* from being a dead end.
            // `Library::root_unavailable` is "could not be listed **and** this
            // machine has documents recorded", and the index is keyed by document
            // id rather than by folder, so a user who has ever opened anything
            // carries that second half with them to whatever root they name next.
            // Without that call, choosing a folder that does not exist yet opens
            // a library the app then refuses to write into — permanently, since
            // the only thing that would have created it is the write being
            // refused. Naming a folder as your library is asking for it to be
            // one. `relocate` does this for itself, which is why the migration
            // above never needed it.
            //
            // ⚠️ **Re-opened rather than refreshed.** `Library` caches the root
            // it was built with, so a `refresh` here would re-scan the folder the
            // user just left — and every subsequent write would go there too.
            self.library = crate::library::state::Library::open(new_root);
            // A nav pointing at a project from the old library names nothing in
            // the new one.
            self.dash.nav = Nav::from_id(&self.prefs.dashboard_page);
        }
        self.prefs.save();
        // The retention window may have just shrunk, and this is what makes that
        // take effect now rather than at the next launch.
        //
        // ⚠️ **This was the *only* caller of `purge_trash` in the workspace until
        // §15 D550** (`[S1.2-L5-03]`), and the comment above used to end *"the
        // trash is only ever swept when something asks it to"* — which was true
        // and meant nothing else ever did. So pressing Save on an unrelated field
        // in this card, the autosave interval say, permanently deleted every
        // trashed document past the window, retroactively over however many years
        // had accumulated, with nothing in the modal mentioning it.
        // `OndinApp::open_at_launch` sweeps now, so there is no accumulation for
        // this line to surprise anybody with.
        crate::library::store::purge_trash(&self.library.root, self.prefs.trash_keep_days);
        // The sweep changed the trash, so the count the sidebar draws has to be
        // re-read — `open_at_launch`'s sweep does the same (§15 D728).
        self.library.refresh_trash();
    }

    fn delete_modal(&mut self, ctx: &egui::Context, act: &mut Option<Act>) {
        let Some(path) = self.dash.deleting.clone() else {
            return;
        };
        // ⚠️ **The fallback is the file stem, not "this file", because in the
        // trash the fallback is the *only* branch.** `Library::entry_at` searches
        // `library.entries`, which is the library — a trashed document is not in
        // it, and `store::trashed` re-reads the folder, which nothing may do from
        // inside a layout (see this module's header). So the name comes off the
        // path, through the same `de_slug` the list itself uses.
        let name = self
            .library
            .entry_at(&path)
            .map(|e| e.display_name())
            .or_else(|| {
                // Through `scan::display_name` with an empty `DocumentMeta`
                // rather than through its `de_slug` half directly, so this stays
                // the *one* function that turns a stem into a name — which is the
                // rule that doc comment is written under.
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|stem| crate::library::scan::display_name(&Default::default(), stem))
            })
            .unwrap_or_else(|| "this file".into());
        let mut decision: Option<bool> = None;
        // ⚠️ **The trash's copy is the opposite of the library's, and it became
        // reachable when `Delete` did.** `Act::Trash` means *move to trash*
        // everywhere except in the trash, where `trash_or_purge` deletes the file
        // outright — so a card that says "moves to Trash" over a permanent delete
        // is the worst sentence on the screen. The ⋮ never showed it, because
        // *Delete permanently* skips this modal and purges; the key does not skip
        // it, deliberately (see [`OndinApp::dashboard_keys`]), which is what put
        // this state on screen for the first time.
        let purge = self.dash.nav == Nav::Trash;
        let modal = settings::card("delete-file", ctx, |ui| {
            ui.set_width(ui::menu_inner_w(settings::CARD_W, settings::PAD));
            if settings::modal_title(
                ui,
                if purge {
                    "Delete permanently"
                } else {
                    "Delete file"
                },
            ) {
                decision = Some(false);
            }
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(if purge {
                    format!("“{name}” is deleted for good. This cannot be undone.")
                } else {
                    format!("“{name}” moves to Trash.")
                })
                .size(12.0)
                .color(theme::text::MUTED),
            );
            ui.add_space(settings::FOOTER_GAP);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                // Red, like *Delete project*'s confirm — the two cards on this
                // screen that destroy something now agree, which is the whole
                // argument for `FieldButton::Danger` being a state rather than a
                // colour written at one call site.
                if settings::text_button(ui, "Delete", FieldButton::Danger).clicked() {
                    decision = Some(true);
                }
                if settings::text_button(ui, "Cancel", FieldButton::Off).clicked() {
                    decision = Some(false);
                }
            });
        });
        // Escape and the backdrop, both Cancel — see `new_project_modal`.
        if modal.should_close() {
            decision = Some(false);
        }
        match decision {
            Some(true) => *act = Some(Act::Trash(path)),
            Some(false) => self.dash.deleting = None,
            None => {}
        }
    }
}

/// Where an arrow key moves the selection: the new index, or `None` for a key
/// this view does not move on.
///
/// **A free function rather than a method**, which is this project's standing
/// answer to "a decision inside `&mut self` cannot be tested" (§15 D269): the
/// interesting part of the dashboard's keymap is the arithmetic, and this is all
/// of it. [`OndinApp::dashboard_keys`] is then only guards and a path lookup.
///
/// **Linear order, so the grid wraps at the ends of its rows.** `Right` from the
/// last card of a row is the first of the next, which is what a grid of files
/// laid out in reading order means — the alternative, a `Right` that stops at the
/// right-hand column, makes the fourth card a wall the keyboard cannot get past.
///
/// ⚠️ **Out of range clamps rather than refusing.** `Down` from the third column
/// of the second-to-last row, where the last row holds two cards, lands on the
/// last one — Explorer's behaviour, and the one that never leaves a press doing
/// nothing on a grid that plainly has more below. The cost is that `Down` on the
/// last row and `Up` on the first are no-ops that still return `Some`; the caller
/// sets a selection it already had, which is why it must be a *set* rather than a
/// toggle.
///
/// `Left`/`Right` are dead in the list view, where every row is the full width and
/// there is nothing beside anything.
fn arrow_target(
    current: Option<usize>,
    key: egui::Key,
    list_view: bool,
    len: usize,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let cols = if list_view { 1 } else { GRID_COLS } as isize;
    let step = match key {
        egui::Key::ArrowLeft if !list_view => -1,
        egui::Key::ArrowRight if !list_view => 1,
        egui::Key::ArrowUp => -cols,
        egui::Key::ArrowDown => cols,
        _ => return None,
    };
    let Some(at) = current else {
        // **Nothing selected yet**: a downward or rightward key takes the first
        // item and an upward or leftward one takes the last, so the first press
        // enters the list from the end it came from rather than always at the top.
        return Some(if step < 0 { len - 1 } else { 0 });
    };
    Some((at as isize + step).clamp(0, len as isize - 1) as usize)
}

/// The small grey word above a field — *Project name*, *Base folder*, *Autosave*.
///
/// **A caption over the field, which is the one shape `settings.rs` argues
/// against** for its nudge pair, and the argument does not reach here: `Step` and
/// `Shift` are two characters and fit inside a [`ui::value_field`]'s prefix, where
/// *Keep files in trash* is four words and a path field has no width to spare at
/// all. The design puts every one of these above its box; so does this.
///
/// Allocated at exactly the galley's height for `settings::section`'s reason — a
/// `ui.horizontal` would take `interact_size.y` and put the difference above and
/// below, which is invisible in the source and six points on screen.
fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), FIELD_LABEL_H),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(
                egui::RichText::new(text)
                    .size(settings::CAPTION_PT)
                    .color(theme::text::DIM),
            );
        },
    );
    ui.add_space(FIELD_LABEL_GAP);
}

/// A Settings row that is a *choice*: the question on the left, a dropdown against
/// the right margin.
///
/// ⚠️ **The combo is allocated before the label, and the nesting is what makes
/// that so.** A `ui.horizontal` centres each item against the row height it knows
/// *at the moment that item is allocated*, so with the label written first it is
/// centred in `interact_size.y` and the 28pt combo then grows the row past it — a
/// 2pt drop nobody finds by reading. `settings::font_cache_row` carries the same
/// shape and the same warning.
///
/// The combo's own two corrections are the ones every dropdown in this app owes:
/// [`ui::combo_chevron`] in place of egui's white triangle, and `interact_size.y`
/// forced to the field height, without which it paints 26 beside 28s.
fn picker_row(
    ui: &mut egui::Ui,
    label: &str,
    id: &'static str,
    selected: &str,
    options: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = settings::COL_GAP;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.y = settings::FIELD_H;
                ui.spacing_mut().button_padding.y = 0.0;
                egui::ComboBox::from_id_salt(id)
                    .icon(ui::combo_chevron)
                    .width(PICKER_W)
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        ui::menu_rows(ui);
                        ui.spacing_mut().button_padding.y = 2.0;
                        options(ui);
                    });
            });
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(label)
                        .size(settings::LABEL_PT)
                        .color(theme::text::MUTED),
                );
            });
        });
    });
}

/// A Settings field holding a **whole count of something** — seconds of autosave,
/// days of trash retention.
///
/// **Rounded and clamped here as well as by the scrub**, for
/// `settings::nudge_field`'s reason: [`ui::Scrub::whole`] settles a *drag* and
/// leaves typing alone deliberately, so `29.6` typed into the autosave field has to
/// be dealt with on this side. Zero is allowed in both — it is a real setting
/// meaning *off*, which is why the caller says so under the pair rather than
/// clamping it away.
fn whole_field(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    label: &'static str,
    unit: &'static str,
    value: &mut u64,
    max: u64,
) {
    let mut v = *value as f64;
    ui::value_field_suffixed(
        ui,
        size,
        ui::Prefix::Text(label),
        Some(ui::Suffix {
            text: unit,
            clickable: false,
            tooltip: "",
        }),
        &mut v,
        // 0.5, the rate every other whole-unit field in the app scrubs at.
        ui::Scrub::whole(0.5).range(0.0..=max as f64),
        |d| d.custom_formatter(ui::number(0)),
    );
    *value = v.round().clamp(0.0, max as f64) as u64;
}

/// A [`settings::caption`] in the warning amber — for a setting that is *valid* and
/// has a consequence worth naming.
///
/// Amber rather than red for [`theme::color::WARN`]'s own reason: nothing here is a
/// mistake in the document, and an autosave the user turned off is a thing they
/// meant to do. It is the only ink in this card that is not one of the three greys.
fn warn_caption(ui: &mut egui::Ui, text: &str) {
    ui.add_space(3.0);
    ui.label(
        egui::RichText::new(text)
            .size(settings::CAPTION_PT)
            .color(theme::color::WARN),
    );
}

/// What an import says when it is done: how many landed, how many could not be
/// read, and how many were not documents at all.
///
/// **One function because there is one status slot.** `EditorSession::info`
/// assigns rather than appends, so the dialog's import and the drop's import
/// cannot each say their own half — the sentence has to be built before anything
/// is said. `skipped` is always 0 from the dialog, which filters by extension
/// before it ever gets here, and is the count of non-documents from a drop.
///
/// ⚠️ **`skipped` is reported even when everything else went well**, which is the
/// bug this was extracted to fix: a drop of one document and forty pictures used
/// to report the document and stay silent about the forty. A partial refusal that
/// says nothing is indistinguishable from files that never arrived.
///
/// 🚨 **`already` is the *second* skip category and it had the same bug** (§15
/// D693, `[S20.2-L1-06]`). `import_counting` drops
/// already-in-the-library files with a bare `continue`, so the only arm that
/// could mention them was `(0, 0)` — and that arm did not *report* the reason,
/// it **asserted** it, reading *"Nothing to import — those files are already in
/// the library"* without ever having been told that was why. Two consequences,
/// both measured: importing three documents of which two were already in the
/// base folder said *"Imported 1 file(s)"* and nothing whatever about the other
/// two; and dropping forty pictures and no documents said *"those files are
/// already in the library"* about forty files that were not. The count now
/// arrives rather than being inferred, and the `(0, 0)` arm states only what it
/// knows.
///
/// A free function, and tested as one: four `(ok, failed)` arms times two states
/// each of `skipped` and `already` is **sixteen** sentences, and not one of them
/// needs an app, a library or a filesystem to check.
/// `an_import_says_what_it_took_and_what_it_left` takes seven — every arm at
/// least once, both clauses alone, and both together, which is the ordering the
/// two-clause `rest` can get wrong.
fn import_summary(ok: usize, failed: usize, skipped: usize, already: usize) -> String {
    let mut rest = String::new();
    if already > 0 {
        rest.push_str(&format!(" — {already} file(s) already in the library"));
    }
    if skipped > 0 {
        rest.push_str(&format!(
            " — {skipped} other file(s) were not Ondin documents"
        ));
    }
    match (ok, failed) {
        (0, 0) => format!("Nothing to import{rest}"),
        (n, 0) => format!("Imported {n} file(s){rest}"),
        (0, f) => format!("{f} file(s) could not be imported{rest}"),
        (n, f) => format!("Imported {n} file(s); {f} could not be read{rest}"),
    }
}

/// A section heading in the sidebar.
fn section_label(ui: &mut egui::Ui, text: &str) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 20.0), egui::Sense::empty());
    ui.painter().text(
        egui::pos2(rect.left() + 12.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(10.5),
        theme::text::FAINT,
    );
}

/// The sidebar's *Archived projects* heading: a caret, the words, and how many.
/// Returns whether it was clicked.
///
/// **A heading that is also the control**, which is the design's shape and the
/// only one that works here: the group has no rows of its own to click while it is
/// shut, so a separate caret button would be a 10pt target beside 200pt of dead
/// heading. The whole row takes the click and the caret says which way it goes.
///
/// ⚠️ **Deliberately not a [`section_label`] with things added to it.** The
/// *Projects* heading above is inert and this one is not, and the two are two
/// lines apart — sharing a function would leave one of them silently gaining a
/// hover the day the other wanted it.
fn archived_header(ui: &mut egui::Ui, open: bool, count: usize) -> bool {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 20.0), egui::Sense::click());
    let fg = if resp.hovered() {
        theme::text::MUTED
    } else {
        theme::text::FAINT
    };
    let p = ui.painter();
    p.text(
        egui::pos2(rect.left() + 12.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        if open {
            icon::CARET_DOWN
        } else {
            icon::CARET_RIGHT
        },
        theme::icon_font(10.0),
        fg,
    );
    p.text(
        egui::pos2(rect.left() + 26.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "Archived projects",
        egui::FontId::proportional(10.5),
        fg,
    );
    p.text(
        egui::pos2(rect.right() - 12.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        count.to_string(),
        egui::FontId::proportional(10.5),
        theme::text::FAINT,
    );
    resp.clicked()
}

/// The circumradius of the star a starred card wears — sized so the drawn star
/// covers the box the 15pt `icon::STAR` glyph used to.
const STAR_R: f32 = 7.5;
/// How far in the valleys between a star's arms sit, as a fraction of [`STAR_R`].
///
/// **Higher than the 0.382 of a compass-drawn pentagram**, because that one is a
/// mark on paper and this is an icon at 15 points: thin arms disappear against a
/// thumbnail. The two failure modes bracket it — near 0.35 the arms are needles,
/// past 0.6 the shape is a pentagon with dents.
const STAR_INNER: f32 = 0.5;
/// The fillet at every corner, as a fraction of [`STAR_R`]. See [`star_shape`];
/// this is the constant that separates a drawn star from an icon-font one.
const STAR_ROUND: f32 = 0.16;

/// The *New file in {project}* card's corner — **one number for the fill and for
/// the dashes around it**, which is the whole of what went wrong when there were
/// two: the fill was rounded to 9 and the dashed outline was drawn through the
/// rect's four corner points, so the ground curved away under a square outline.
const GHOST_R: f32 = 9.0;

/// Where each of a project mosaic's `n` cells goes inside `rect`.
///
/// **A free function, not a method, so the layout rule can be asserted without a
/// frame** — D269's shape, and this is the whole of what a mosaic *is*: the
/// drawing around it is a rounded rect and a fitted texture per cell.
///
/// The rule, which reproduces all four of the design's hand-authored `MOSAICS`
/// layouts: the first cell takes a column of its own at full height; the rest fill
/// the columns after it two at a time, top then bottom; and ⚠️ **a column that ends
/// up holding only one cell runs full height too**, which is what makes two files
/// two tall cells side by side rather than one tall and one floating.
///
/// **One departure from the design, stated:** it gives the first column `1.25fr` in
/// the three-file case and `1fr` in the other three. Every column here is equal —
/// that 1.25 appears once in four layouts, and a rule carrying an exception for a
/// single count is a hand-tune rather than a rule.
fn mosaic_cells(n: usize, rect: egui::Rect) -> Vec<egui::Rect> {
    if n == 0 {
        return Vec::new();
    }
    let cols = 1 + (n - 1).div_ceil(2);
    let cell_w = (rect.width() - MOSAIC_GAP * (cols - 1) as f32) / cols as f32;
    let half_h = (rect.height() - MOSAIC_GAP) / 2.0;
    (0..n)
        .map(|i| {
            let col = if i == 0 { 0 } else { 1 + (i - 1) / 2 };
            // The lower cell of column `col` is index `2 * col`; if the list stops
            // short of it, that column's one cell takes the whole height.
            let alone = col == 0 || 2 * col >= n;
            let lower = i > 0 && (i - 1) % 2 == 1;
            let (y, h) = match (alone, lower) {
                (true, _) => (rect.top(), rect.height()),
                (false, true) => (rect.top() + half_h + MOSAIC_GAP, half_h),
                (false, false) => (rect.top(), half_h),
            };
            egui::Rect::from_min_size(
                egui::pos2(rect.left() + col as f32 * (cell_w + MOSAIC_GAP), y),
                egui::vec2(cell_w, h),
            )
        })
        .collect()
}

/// The outline of a rounded rectangle as a closed polyline, for dashing along.
///
/// `per_corner` is how many segments each quarter-turn is sampled at; six puts the
/// chords at about 2.4pt on [`GHOST_R`], which is under the dash length and so
/// invisible as faceting.
///
/// ⚠️ **The straight edges are implicit.** Consecutive points are joined, so the
/// segment between one corner's last arc point and the next corner's first *is* the
/// side — there is nothing to push for it. What must be pushed is the repeat of the
/// first point at the end, or the polyline has no closing edge.
///
/// ⚠️ **`Shape::dashed_line` carries its dash phase across the whole polyline**
/// rather than restarting per segment, which is what makes this work at all: a
/// per-segment restart would put a dash at every one of the twenty-eight points and
/// draw a solid line. Read out of `epaint`'s `dashes_from_line` rather than assumed.
fn rounded_rect_path(rect: egui::Rect, r: f32, per_corner: usize) -> Vec<egui::Pos2> {
    use std::f32::consts::{FRAC_PI_2, PI};
    let r = r.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let mut pts = Vec::with_capacity(per_corner * 4 + 6);
    // Clockwise in screen space (y down), starting on the top edge just past the
    // top-left corner, so each arc begins exactly where the straight run before it
    // ended and the dash phase never doubles back.
    pts.push(rect.left_top() + egui::vec2(r, 0.0));
    for (centre, from) in [
        (rect.right_top() + egui::vec2(-r, r), -FRAC_PI_2),
        (rect.right_bottom() + egui::vec2(-r, -r), 0.0),
        (rect.left_bottom() + egui::vec2(r, -r), FRAC_PI_2),
        (rect.left_top() + egui::vec2(r, r), PI),
    ] {
        for i in 0..=per_corner {
            let a = from + FRAC_PI_2 * i as f32 / per_corner as f32;
            pts.push(centre + egui::vec2(a.cos(), a.sin()) * r);
        }
    }
    // The last arc lands on the first point to within a rounding error rather than
    // exactly, so this is a real closure and not a no-op.
    pts.push(pts[0]);
    pts
}

/// A **filled** five-pointed star, as a convex polygon.
///
/// ⚠️ **Drawn rather than set, because there is no filled star to set.** The design
/// asks for `ph-fill ph-star`; the bundled `Phosphor.ttf` is the Regular face, whose
/// star is an outline, and Phosphor's fill weight is a *separate font file* — a
/// second megabyte of glyphs for one shape. `assets/fonts/Phosphor.codepoints.txt`
/// lists no `-fill` names at all, which is the check to repeat before believing
/// this: if a fill face is ever bundled for another reason, this function should go.
///
/// **The gold is what says *starred* and the fill is what makes it legible at
/// 15pt** — an outline star over a thumbnail is a gold *ring*, and at this size the
/// hole is most of it.
///
/// ⚠️ **The first version of this was a bare ten-vertex polygon and was reported
/// as not looking great**, which is fair: at 15pt a mathematically plain star is
/// thin-armed and needle-pointed, and Phosphor's — the thing it stands in for — is
/// neither. Two constants carry the difference and they are the two to turn.
///
/// [`STAR_INNER`] fattens the arms; the *ratio* is what a star's character is,
/// and 0.42 (near the golden 0.382 a compass-and-straightedge star uses) draws the
/// spiky one. ⚠️ [`STAR_ROUND`] is the one that matters more and is the one a
/// polygon does not give you for free: **every vertex is cut with a fillet**, points
/// and valleys alike, so the silhouette has no needle in it. An icon font's star is
/// rounded everywhere and the eye reads that before it reads the proportions.
fn star_shape(centre: egui::Pos2, r: f32, fill: egui::Color32) -> egui::Shape {
    const POINTS: usize = 5;
    // The ten corners, before any of them is rounded.
    let corner = |i: usize| {
        let radius = if i.is_multiple_of(2) {
            r
        } else {
            r * STAR_INNER
        };
        // Half a step per vertex, starting at −90° so a point is at the top.
        let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / POINTS as f32;
        centre + egui::vec2(a.cos(), a.sin()) * radius
    };
    let n = POINTS * 2;
    let mut pts = Vec::with_capacity(n * 4);
    for i in 0..n {
        let (prev, here, next) = (corner((i + n - 1) % n), corner(i), corner((i + 1) % n));
        // ⚠️ **The fillet is capped at half of the shorter edge**, or a valley
        // between two long arms would round past the corner beside it and the
        // outline would cross itself — which `convex_polygon`'s triangle fan turns
        // into a visible wedge rather than into nothing.
        let (a, b) = (prev - here, next - here);
        let cut = (r * STAR_ROUND).min(a.length() / 2.0).min(b.length() / 2.0);
        let (from, to) = (here + a.normalized() * cut, here + b.normalized() * cut);
        // Three points along a quadratic through the corner: enough at this size,
        // and the mid one is what stops a cut corner reading as a bevel.
        for t in [0.0, 0.5, 1.0] {
            let u = 1.0 - t;
            pts.push(
                ((from.to_vec2() * u + here.to_vec2() * t) * u
                    + (here.to_vec2() * u + to.to_vec2() * t) * t)
                    .to_pos2(),
            );
        }
    }
    // ⚠️ **`convex_polygon`, and a star is not convex** — which is fine and worth
    // stating, because the name reads as a precondition. `epaint` fans a "convex"
    // polygon from its first vertex, and a five-pointed star's alternating radii
    // keep every fan triangle inside the outline; what the fast path actually needs
    // is that the shape is star-shaped about a kernel point, which this is by
    // construction. A concave shape that is *not* would need `Shape::Path` closed
    // with a tessellator that handles self-overlap.
    egui::Shape::convex_polygon(pts, fill, egui::Stroke::NONE)
}

/// *Delete project* — the design's icon button, and the one control on this screen
/// whose click removes something that has no trash to come back from.
///
/// **A glyph rather than a word, which is the design's answer**, and the pairing is
/// the argument for it: two labelled buttons side by side read as two ordinary
/// actions, and one of these is not. A glyph has to be aimed at.
///
/// ⚠️ **Which means a tooltip is owed, and it is now the only place the words
/// survive.** The label was the whole of what said *project* rather than *file* —
/// and this screen has a *Trash* nav two inches to the left. It is hung on the
/// response here rather than at the call site so a caller cannot drop it; note that
/// no probe can check it, since a tooltip's galley never reaches `.shapes`, so what
/// a test can assert is the glyph.
///
/// The face is [`ui::button_face`]'s — shared with the *Edit project* and sort
/// buttons beside it rather than respelled, since a hairline half a shade off reads
/// as two kinds of control in one row — and the **hover is then repainted**: the
/// design's red wash and red border, in [`color::DANGER`], which is the red this app
/// already means *this click removes it* by. ⚠️ **Between the face and the glyph, or
/// the wash goes over the thing it is meant to tint**, and rounded to
/// [`ui::BUTTON_R`] rather than to a 5 written here, or the overlay leaves a
/// hairline of the ground at each corner.
///
/// **The design's own `#e0645c`/`#e0857e` are deliberately not used.** They are two
/// new hexes for a meaning the palette already names, and `color::DANGER`'s own note
/// asks for exactly this: a second destructive control must be able to be the same
/// red without anyone having to go and find the first one.
fn delete_project_button(ui: &mut egui::Ui) -> egui::Response {
    let (rect, resp, fg) =
        ui::button_face(ui, egui::vec2(DELETE_W, HEADER_CONTROL_H), FieldButton::Off);
    let lit = resp.hovered();
    if lit {
        let r = egui::CornerRadius::same(ui::BUTTON_R);
        let p = ui.painter();
        p.rect_filled(rect, r, color::DANGER.gamma_multiply(0.14));
        p.rect_stroke(
            rect,
            r,
            egui::Stroke::new(1.0, color::DANGER.gamma_multiply(0.4)),
            egui::StrokeKind::Inside,
        );
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon::TRASH,
        theme::icon_font(14.0),
        if lit { color::DANGER } else { fg },
    );
    resp.on_hover_text("Delete project")
}

/// The amber *CONFLICT* pill a sync client's copy wears (§15 D375). Returns the
/// rect it took, so a caller can lay the rest of a row out after it.
///
/// **Amber, and [`theme::color::WARN`]'s own words say why**: "the usual cause is
/// a file that moved, which is a thing to fix rather than a mistake in the
/// document". A conflict copy is the same kind of fact — nothing is broken, there
/// are two of something, and the user is the only one who can say which to keep.
/// Red would read as *this click removes it*, which is what the ⋮'s *Delete*
/// means two inches away.
///
/// ⚠️ **A drawn pill rather than a tooltip, deliberately.** The thing it says has
/// to be visible while the eye is *scanning* — the whole failure it marks is two
/// cards that look identical — and a tooltip is by definition invisible until one
/// of the two has already been picked. It is also the one shape this codebase
/// knows a probe can see: a tooltip's galley never reaches `.shapes` however many
/// frames are pumped (see this project's notes on `on_hover_text`).
/// ⚠️ **Two labels now, and the second is why this takes one** (§15 D613,
/// `[S1.3-L3-05]`). A document this build cannot load wants exactly this mark for
/// exactly this reason — the failure is two cards that look identical — and a
/// second hand-rolled pill would be a second set of paddings, a second colour
/// decision and a second thing to keep in step with the list view's elision.
///
/// **The colour comes with the label** ([`Mark`]), because that is the one thing
/// the two disagree about: a conflict is a fact to resolve and a broken file is a
/// failure.
fn mark_chip(ui: &egui::Ui, at: egui::Pos2, align: egui::Align2, mark: Mark) -> egui::Rect {
    let rect = align.anchor_size(at, mark_chip_size(ui, mark));
    let color = mark.color();
    let p = ui.painter();
    // ⚠️ **Two passes, and the first is black.** On a card this sits *on the
    // cover*, which is arbitrary artwork — a 22%-alpha amber over somebody's pale
    // mockup is an amber word on a white ground, i.e. invisible. The dark pass is
    // the same problem the star solves one corner away with a drop shadow, and the
    // same answer: give the mark something of its own to sit against before
    // tinting it. In the list view there is no artwork and the first pass is
    // simply a slightly darker chip.
    p.rect_filled(
        rect,
        egui::CornerRadius::same(4),
        egui::Color32::from_black_alpha(150),
    );
    p.rect_filled(
        rect,
        egui::CornerRadius::same(4),
        color.gamma_multiply(0.26),
    );
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        mark.label(),
        egui::FontId::proportional(MARK_PT),
        color,
    );
    rect
}

/// What a card or a row can be marked with — at most one, most severe first
/// ([`OndinApp::mark_of`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mark {
    /// This build cannot load the file (§15 D613, `[S1.3-L3-05]`).
    ///
    /// 🚨 **The card's *Open* stays live over this mark, by decision** (§15
    /// D818). D613 left the question open — whether *Open* should be disabled or
    /// warning-styled on such a card — and the ruling is **neither**. The mark
    /// here and the status line already say the file will not load, so a refusal
    /// would be the third statement of one fact; and the case that actually
    /// reaches a synced library is a document written by a **newer build**, which
    /// is well-formed and merely ahead of this one. Refusing to open a file the
    /// user may want to look at, or may be about to update the app for, is the
    /// wrong way round: the mark warns, the click is still theirs.
    Unreadable,
    /// A sync client's copy (§15 D375).
    Conflict,
}

impl Mark {
    fn label(self) -> &'static str {
        match self {
            Self::Unreadable => "UNREADABLE",
            Self::Conflict => "CONFLICT",
        }
    }

    /// **Amber for a conflict and red for a broken file, and the difference is
    /// the one the two marks are for.** [`theme::color::WARN`]'s own words are
    /// *"the usual cause is a file that moved, which is a thing to fix rather than
    /// a mistake in the document"* — true of a conflict copy, where nothing is
    /// broken and the user is the only one who can say which of two to keep, and
    /// **not** true of a document this build cannot open. Red is the app's word
    /// for *this does not work*, and the objection that stopped it being used for
    /// a conflict — *"red would read as `this click removes it`, which is what the
    /// ⋮'s Delete means two inches away"* — does not apply here, because the thing
    /// it warns about is exactly the click.
    fn color(self) -> egui::Color32 {
        match self {
            Self::Unreadable => theme::color::DANGER,
            Self::Conflict => theme::color::WARN,
        }
    }
}

const MARK_PT: f32 = 8.5;

/// How much room [`mark_chip`] needs.
///
/// **Measured rather than a constant**, because the list view has to take it out
/// of the *Name* column before the name is elided — a fixed guess that came out
/// short would put the chip on top of the last letters of a long name, which is
/// the one failure this whole mark cannot afford. ⚠️ **And it is measured per
/// mark**: `UNREADABLE` is four characters longer than `CONFLICT`, so a size
/// shared between them is the fixed guess this function exists not to be.
fn mark_chip_size(ui: &egui::Ui, mark: Mark) -> egui::Vec2 {
    ui.ctx()
        .fonts_mut(|f| {
            f.layout_no_wrap(
                mark.label().to_owned(),
                egui::FontId::proportional(MARK_PT),
                mark.color(),
            )
        })
        .size()
        + egui::vec2(10.0, 5.0)
}

/// Parse a `#rrggbb` from `projects.json` into a colour (§15 D417).
///
/// **Falls back to the accent rather than to a panic or to black.** The file is
/// hand-editable and travels between machines, so a malformed value is a thing
/// that will happen; a project drawn in the accent is a project the user can
/// still click.
///
/// ⚠️ **The guard has to ask about characters, not only about length, and for a
/// long time it asked only about length.** `str::len` counts **bytes**, so
/// `"#€abc"` is six bytes and four characters: it passed `h.len() != 6` and then
/// panicked on `&h[0..2]`, which lands inside the `€`. That is a panic on the
/// **layout path of every frame of the library screen** — the sidebar draws one
/// row per active project unconditionally, and `reopen_last` is off by default,
/// so the app opens on the screen that crashes and there is no door back.
/// A *two*-byte character is harmless by luck (`"éa0b1"` slices on the boundary
/// and falls back as intended), which is exactly why the mistake read as
/// correct: **the first non-ASCII value anyone tries is likely to behave.**
///
/// The parse is over the whole string now rather than three slices, so there is
/// nothing left to slice. The `Err` arm is unreachable behind the guard above
/// and is kept as the same fallback, so a future edit that loosens the guard
/// cannot turn this back into a panic.
fn swatch(hex: &str) -> egui::Color32 {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return color::ACCENT;
    }
    match u32::from_str_radix(h, 16) {
        Ok(v) => egui::Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8),
        Err(_) => color::ACCENT,
    }
}

/// Cut a string to fit `max_w`, with an ellipsis.
///
/// ⚠️ **Hand-rolled rather than `ui.label`'s wrapping**, because these are
/// painted strings rather than widgets: the grid card and the list row both draw
/// their text with `Painter::text` into a box they have already allocated, so
/// there is nothing to wrap and an over-long name simply runs into the column
/// beside it.
fn elide(ctx: &egui::Context, text: &str, size: f32, max_w: f32) -> String {
    let width = |s: &str| {
        ctx.fonts_mut(|f| {
            f.layout_no_wrap(
                s.to_owned(),
                egui::FontId::proportional(size),
                theme::text::STRONG,
            )
        })
        .size()
        .x
    };
    if max_w <= 0.0 || width(text) <= max_w {
        return text.to_string();
    }
    let mut cut = text.to_string();
    while !cut.is_empty() && width(&format!("{cut}…")) > max_w {
        cut.pop();
    }
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nav_preference_round_trips_and_a_stale_project_degrades() {
        for nav in [Nav::Recent, Nav::All, Nav::Starred, Nav::Trash] {
            assert_eq!(Nav::from_id(nav.id()), nav);
        }
        // ⚠️ A project id is written but never read back, on purpose: the project
        // may have been deleted between launches. Landing on Recent beats landing
        // on a heading with nothing under it.
        let p = Nav::Project("0f9c2b7a4e".into());
        assert_eq!(p.id(), "0f9c2b7a4e");
        assert_eq!(Nav::from_id(p.id()), Nav::Recent);
    }

    /// A hand-editable colour must never be able to crash the sidebar.
    ///
    /// ⚠️ **This test was green while the sidebar panicked**, which is the thing
    /// worth carrying forward: its five bad values were all ASCII, so every one
    /// of them was answered by the length guard and none of them reached the
    /// byte slicing behind it. *What would also pass this:* the shipped code.
    /// The four multi-byte values below are the cases the name was always
    /// claiming — `"#€abc"` is six **bytes** and four characters, which is
    /// precisely the shape `h.len() != 6` was written to catch and let through.
    ///
    /// **`"éa0b1"` is the one that matters most and it is the one that always
    /// passed**: a two-byte character happens to leave `0..2` on a boundary, so
    /// it fell back correctly by luck. It is kept as the negative control —
    /// under a fix that only special-cased three-byte characters it would still
    /// be green, and it says why nobody noticed.
    ///
    /// Flip-check, run: dropping `is_ascii_hexdigit` from the guard fails here
    /// with *"end byte index 2 is not a char boundary; it is inside '€'"* — the
    /// panic itself, from the layout path, in the failure message.
    #[test]
    fn a_malformed_swatch_falls_back_to_the_accent() {
        assert_eq!(swatch("#6d8cd9"), egui::Color32::from_rgb(0x6d, 0x8c, 0xd9));
        assert_eq!(swatch("6d8cd9"), egui::Color32::from_rgb(0x6d, 0x8c, 0xd9));
        for bad in [
            "",
            "#fff",
            "#gggggg",
            "rebeccapurple",
            "#6d8cd9ff",
            // Six bytes, four characters — the case the length guard passed.
            "#€abc",
            // Six bytes, two characters.
            "#€€",
            // Six bytes, three two-byte characters: every boundary is even, so
            // the old slicing survived this one and answered with the accent for
            // the wrong reason.
            "#ééé",
            // Six bytes, five characters — the harmless-by-luck control.
            "#éa0b1",
        ] {
            assert_eq!(swatch(bad), color::ACCENT, "{bad}");
        }
    }

    /// The window the screen lays itself out in. Wide enough that the four-column
    /// grid and the centred search field are never the thing being clipped.
    const SCREEN: egui::Vec2 = egui::vec2(1320.0, 820.0);

    /// A headless app on an empty temp library.
    fn app(ctx: &egui::Context, tag: &str) -> (OndinApp, PathBuf) {
        let root = std::env::temp_dir().join(format!("ondin-dash-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut app = OndinApp::headless(ctx);
        app.prefs.base_folder = Some(root.clone());
        app.library = crate::library::state::Library::open(root.clone());
        (app, root)
    }

    /// The colour of the library's status dot this frame, or `None` when the top
    /// bar painted no dot at all (§15 D751).
    ///
    /// 🚨 **This replaces reading the message out of `galleys` and the difference
    /// is not cosmetic.** The library's status used to be a sentence, so a test
    /// could assert the words were on screen; it is a dot with the words on hover
    /// now, and **a tooltip's ink never arrives in `.shapes`** — `on_hover_text`
    /// defers an `Area`, and no number of pumped frames puts its galley in the
    /// output. So the surface and the wording have to be asserted separately: this
    /// says *the screen is showing something*, and `session.status().text` says
    /// *what the hover would read*. A test that asserted only the second would be
    /// green for the two months this file wrote 22 messages nobody could see,
    /// which is the exact failure §15 D426 was written for.
    ///
    /// ⚠️ **Found by radius, not by position**, so a re-laid-out header does not
    /// silently stop this working — the failure mode a coordinate lookup has is a
    /// green test measuring nothing. `crate::app::STATUS_DOT_R` is `pub(crate)`
    /// for this, and the colour is returned rather than a `bool` because the two
    /// kinds differ **only** by colour on this screen: a fix wiring up one and not
    /// the other would look right in whichever screenshot was taken.
    fn status_dot_color(app: &mut OndinApp, ctx: &egui::Context) -> Option<egui::Color32> {
        fn walk(s: &egui::Shape, out: &mut Vec<egui::Color32>) {
            match s {
                egui::Shape::Circle(c)
                    if (c.radius - crate::app::STATUS_DOT_R).abs() < 0.01
                        && c.fill != egui::Color32::TRANSPARENT =>
                {
                    out.push(c.fill);
                }
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
            ..Default::default()
        };
        let mut found = Vec::new();
        for _ in 0..4 {
            let full = ctx.run_ui(input.clone(), |ui| app.dashboard_ui(ui));
            found.clear();
            full.shapes
                .iter()
                .for_each(|cs| walk(&cs.shape, &mut found));
        }
        assert!(found.len() < 2, "one status dot at most, got {found:?}");
        found.first().copied()
    }

    /// Every text run the dashboard painted, as `(top-left, size, text)`.
    ///
    /// **Several passes, because a widget's interaction state is last frame's** —
    /// and because a `TextEdit` is handed focus for the *next* frame, so a
    /// single-pass probe of a focused field measures it unfocused.
    fn galleys(app: &mut OndinApp, ctx: &egui::Context) -> Vec<(egui::Pos2, egui::Vec2, String)> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
            ..Default::default()
        };
        let mut out = Vec::new();
        for _ in 0..4 {
            let full = ctx.run_ui(input.clone(), |ui| app.dashboard_ui(ui));
            out = full
                .shapes
                .iter()
                .filter_map(|cs| match &cs.shape {
                    egui::Shape::Text(t) => {
                        Some((t.pos, t.galley.size(), t.galley.text().to_owned()))
                    }
                    _ => None,
                })
                .collect();
        }
        out
    }

    /// Whether `text` was painted in the **body**, right of the sidebar.
    ///
    /// ⚠️ **The sidebar has a *New file* button of its own**, which is the whole
    /// reason this exists: a bare search for that string finds it on every nav and
    /// the first version of these tests asserted the opposite of what it meant.
    /// The card and the row are the ones in the grid.
    fn in_body(runs: &[(egui::Pos2, egui::Vec2, String)], text: &str) -> bool {
        runs.iter()
            .any(|(pos, _, t)| t == text && pos.x > SIDEBAR_W)
    }

    /// Where a run of `text` was painted, as `(left, centre y)`.
    fn run_at(runs: &[(egui::Pos2, egui::Vec2, String)], text: &str) -> (f32, f32) {
        let (pos, size, _) = runs
            .iter()
            .find(|(_, _, t)| t == text)
            .unwrap_or_else(|| panic!("no run for {text:?}"));
        (pos.x, pos.y + size.y / 2.0)
    }

    /// Reported as *"text in search box isn't aligned when it takes focus — it's
    /// aligned when it's not focused"*: the field swaps a painted placeholder for a
    /// live `TextEdit`, and the two were laid out by different arithmetic.
    ///
    /// ⚠️ **The first flip did not bite, and finding out why fixed a live bug.**
    /// The plausible wrong version looked like `TextEdit::margin` — put egui's
    /// default `Margin::symmetric(4, 2)` back and the run should move 4pt right.
    /// It moved nothing: egui reads that builder *only* when no frame was supplied
    /// (`let frame = frame.unwrap_or_else(|| Frame::new().inner_margin(margin))`),
    /// so with `.frame(Frame::NONE)` in play the margin is dead in both versions.
    /// `OndinApp::rename_field` had been written the same way the same hour and
    /// its 4pt inset was silently doing nothing; it now puts the inset in the
    /// frame.
    ///
    /// **The flip that does bite is the rect**: give the `TextEdit` the whole 28pt
    /// field instead of one galley's height, which is what it had before and the
    /// version anybody would write. The y assertion fails at 16.0 against 23.0 —
    /// and the x assertion stays green, because the horizontal position never came
    /// from the margin at all. So of the two axes only one is load-bearing, which
    /// is the opposite of what the first version of this comment predicted.
    #[test]
    fn the_search_placeholder_does_not_move_when_the_field_takes_focus() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "search-align");

        let resting = run_at(&galleys(&mut app, &ctx), "Search files and projects");
        // The fixture has to be in the state the assertion is about: a *closed*
        // field paints its own string, an open one hands the same string to a
        // `TextEdit` as hint text. Without this the test compares a run to itself.
        app.dash.search = Some(String::new());
        let live = run_at(&galleys(&mut app, &ctx), "Search files and projects");

        assert!(
            (resting.0 - live.0).abs() < 0.5,
            "the placeholder starts at {:.1} at rest and {:.1} once focused",
            resting.0,
            live.0
        );
        assert!(
            (resting.1 - live.1).abs() < 0.5,
            "the placeholder's centre is {:.1} at rest and {:.1} once focused",
            resting.1,
            live.1
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every row of the rebuilt Settings card fits the card's own column, and the
    /// commit is dim until something has been edited.
    ///
    /// ⚠️ **The card got *narrower* — 376 against the 420 it shipped at — so this
    /// is the assertion the rebuild most needed.** It moved to `settings::CARD_W`
    /// so the two Settings modals are one card, and every row in it had been laid
    /// out against 44 more points than it now has. The longest is the *Default page
    /// on open* picker: its label plus `PICKER_W` plus a gap.
    ///
    /// ⚠️ **Flip-checked twice, and the first flip is the more useful half.**
    /// Raising `PICKER_W` to 240 — a dropdown widened to hold its longest option
    /// without checking what is beside it — leaves this **green**, because a
    /// `picker_row`'s label is a `ui.label` and a `ui.label` wraps: the row absorbs
    /// the squeeze instead of overflowing. So this probe does not see a crowded
    /// row, only an escaping one.
    ///
    /// What it *does* see is painted text, which cannot wrap and is not clipped —
    /// `ui::switch_row` draws its label with `painter.text`. Lengthening *Reopen
    /// the last file on launch* to a sentence fails it at 23.8pt over. That is the
    /// real hazard the card shrinking from 420 to 376 introduced.
    #[test]
    fn every_settings_row_fits_the_card_and_the_commit_starts_dim() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "settings-fit");
        app.library_settings = Some(LibrarySettings::from_prefs(&app.prefs, &app.library.root));

        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
            ..Default::default()
        };
        let mut full = ctx.run_ui(input.clone(), |ui| app.dashboard_ui(ui));
        for _ in 1..4 {
            full = ctx.run_ui(input.clone(), |ui| app.dashboard_ui(ui));
        }
        // ⚠️ **The card has to be located before anything is measured, because the
        // dashboard is still painting behind the backdrop.** A first version took
        // an x-band around the first eyebrow and let the *empty state* — centred in
        // the central panel — into the loop, reporting "Nothing opened on this
        // computer yet." 186pt over a card it is not in.
        //
        // ⚠️ **And it is not found by width.** `settings::CARD_W` is the design's
        // *outer* number and nothing in the frame paints a rect that wide: the
        // widths on screen are 340, 338 and 318. So the card is whatever rect
        // contains its first eyebrow — which is what the loop below actually needs.
        let eyebrow = full
            .shapes
            .iter()
            .find_map(|cs| match &cs.shape {
                egui::Shape::Text(t) if t.galley.text() == "STORAGE" => Some(t.pos),
                _ => None,
            })
            .expect("the modal is open and painting its first section header");
        // ⚠️ **And the walk has to descend into `Shape::Vec`.** A `Frame` carrying a
        // shadow paints `Shape::Vec([shadow, rect])`, so the card's own fill never
        // appears in a flat scan of `FullOutput::shapes` — the only rects that
        // *look* like they contain the eyebrow are the backdrop (1320×820) and the
        // central panel (1054×774) behind it. A first version of this probe found
        // neither card nor bug and simply matched the panel.
        fn rects(shape: &egui::Shape, out: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Rect(r) => out.push(r.rect),
                egui::Shape::Vec(v) => v.iter().for_each(|s| rects(s, out)),
                _ => {}
            }
        }
        let mut card = None;
        let mut card_at = 0;
        for (i, cs) in full.shapes.iter().enumerate() {
            let mut here = Vec::new();
            rects(&cs.shape, &mut here);
            // Bounded above as well as below, or this finds the backdrop and the
            // central panel, both of which contain the eyebrow.
            if let Some(r) = here
                .into_iter()
                .filter(|r| r.contains(eyebrow) && (300.0..400.0).contains(&r.width()))
                .max_by(|a, b| a.height().total_cmp(&b.height()))
            {
                card = Some(r);
                card_at = i;
                break;
            }
        }
        let card = card.expect("the settings card");
        // The eyebrow starts at the content's left edge, and the column is what
        // `set_width` was handed — see `settings::every_row_fits_the_cards_own_column`
        // for the same pair of numbers on the editor's copy of this card.
        let column = ui::menu_inner_w(settings::CARD_W, settings::PAD);
        assert_eq!(
            column, 338.0,
            "376 across, less 18 of padding and a hairline"
        );
        // ⚠️ **Painted *after* the card as well as inside it.** The dashboard goes
        // on drawing behind the backdrop, and its empty state is centred in the
        // central panel — which puts "Nothing opened on this computer yet." inside
        // the card's rect and 186pt past its column. Paint order is what separates
        // them: the modal is an `Area` on top, so everything of its own comes later
        // in `FullOutput::shapes` than the frame it sits in.
        for cs in full.shapes.iter().skip(card_at + 1) {
            let egui::Shape::Text(t) = &cs.shape else {
                continue;
            };
            if !card.contains(t.pos) {
                continue;
            }
            // ⚠️ **Against the *clipped* right edge, not the galley's.** A
            // `singleline` field scrolls rather than wraps, so the base folder's
            // galley is as long as the path is and egui clips it to the field —
            // measured raw, a temp directory reported 64pt of overflow that is not
            // on screen. `cs.clip_rect` is where the ink actually stops.
            let ink = (t.pos.x + t.galley.size().x).min(cs.clip_rect.right());
            let over = ink - (eyebrow.x + column);
            assert!(
                over <= 0.5,
                "{:?} runs {over:.1}pt past the card's own column",
                t.galley.text()
            );
        }

        // Nothing has been touched, so there is nothing to save. Asserted on the
        // *enum* rather than on the ground colour, because unlike the editor's card
        // this one is drawn over a live dashboard and a rect search would have the
        // whole screen to pick a false positive out of.
        let saved = LibrarySettings::from_prefs(&app.prefs, &app.library.root);
        assert!(
            app.library_settings.as_ref() == Some(&saved),
            "an untouched form equals what is saved, which is what dims the commit"
        );
        let mut edited = saved.clone();
        edited.reopen_last = !edited.reopen_last;
        assert!(edited != saved, "and one moved row is what lights it");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A starred document's card carries the star, and an unstarred one carries
    /// nothing.
    ///
    /// ⚠️ **Both halves, because only the pair says the mark means *starred*.** A
    /// test that a starred card draws a gold glyph passes just as well against a
    /// card that always does. The flip that bites is inverting the condition to
    /// `!self.is_starred(entry)`: the unstarred assertion fails, reporting one star
    /// on a library where nothing is starred.
    ///
    /// ⚠️ **It is a drawn polygon, not a glyph, so this counts *shapes*.** The
    /// bundled Phosphor is Regular weight only and its star is an outline — a gold
    /// ring at 15pt, which is mostly hole — and the fill face is a separate font
    /// file. `star_shape` draws it instead, so the assertion moved off
    /// `Shape::Text` and onto the two `Shape::Path`s: the gold one and the dark
    /// pass standing in for the design's drop shadow.
    #[test]
    fn only_a_starred_card_wears_the_star() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "card-star");
        let mut doc = ondin_core::Document::new(app.session.ids.mint());
        store::file_document(&root, None, "Landing v4", &mut doc).unwrap();
        app.library.refresh();
        app.dash.nav = Nav::All;
        app.dash.list_view = false;
        let entry = app.library.entries.first().cloned().expect("one document");
        let id = entry.meta.id.clone().expect("a filed document has an id");

        // ⚠️ **Counted by *fill*, which is what the sidebar cannot satisfy.** The
        // *Starred* nav row still wears the outline glyph and is always painted —
        // an unfiltered count of stars answers 1 for a library where nothing is
        // starred, which is what the first run of this probe reported. Now the
        // glyph and the drawn star are different kinds of shape as well as
        // different places, so both guards hold.
        let stars = |app: &mut OndinApp| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
                ..Default::default()
            };
            let mut out = (0usize, 0usize);
            for _ in 0..4 {
                let full = ctx.run_ui(input.clone(), |ui| app.dashboard_ui(ui));
                out = (0, 0);
                for cs in &full.shapes {
                    // Thirty vertices: ten corners — five points and five valleys —
                    // each cut into three by its fillet. Checked, so a future dashed
                    // outline or drop indicator cannot be counted as a star.
                    if let egui::Shape::Path(path) = &cs.shape
                        && path.points.len() == 30
                    {
                        if path.fill == theme::color::STAR {
                            out.0 += 1;
                        } else if path.fill.a() > 0 {
                            out.1 += 1;
                        }
                    }
                }
            }
            out
        };
        assert_eq!(stars(&mut app), (0, 0), "nothing is starred yet");

        app.library.local.toggle_star(&id);
        assert!(app.library.local.is_starred(&id), "the fixture is starred");
        // One gold, and one dark pass under it: the design's `drop-shadow` spelled
        // as a second draw one point down, because a cover is arbitrary artwork.
        assert_eq!(
            stars(&mut app),
            (1, 1),
            "the gold star and the dark pass under it"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The New project name field takes the caret when the modal opens and does
    /// **not** take it back once something else has it.
    ///
    /// Reported as *"text boxes don't seem to lose focus — once they gain it,
    /// clicking anywhere doesn't make them lose focus"*, which was a
    /// `field.request_focus()` on every frame: nothing else in the card could ever
    /// hold the caret, so the switch and the two footer buttons were unreachable by
    /// keyboard and the field could not be left.
    ///
    /// **The second half is the assertion, and the first is only the fixture.** A
    /// probe that checked the field gets focus would pass against exactly the
    /// version that was reported. Taking the caret away by hand and running four
    /// more frames is what the wrong version cannot survive — flip-checked by
    /// dropping the `name_focused` latch, which fails at the second assertion with
    /// the field focused again.
    #[test]
    fn the_new_project_name_field_asks_for_focus_once() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "np-focus");
        app.dash.new_project = Some(NewProject::default());

        let _ = galleys(&mut app, &ctx);
        let focused = ctx.memory(|m| m.focused());
        assert!(
            focused.is_some(),
            "opening the modal puts the caret in the name field"
        );

        // What a click on anything else in the card amounts to, without having to
        // find that thing's rect: the caret goes, and nothing may pull it back.
        ctx.memory_mut(|m| m.surrender_focus(focused.expect("checked above")));
        let _ = galleys(&mut app, &ctx);
        assert_eq!(
            ctx.memory(|m| m.focused()),
            None,
            "the field asks once, so once it has been left it stays left"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The project filter is offered exactly where it acts, and a filter naming a
    /// project that has gone stops filtering.
    ///
    /// ⚠️ **The two halves of the first sentence are one predicate, and that is
    /// the bug this pins.** A filter set on *All files* and left set is invisible
    /// on *Recent* — the control is not drawn there — so if the retain in
    /// `OndinApp::visible_entries` were gated on anything else, *Recent* would
    /// come back short with nothing on screen saying why. Flip-checked by dropping
    /// `self.filter_applies() &&` from that retain: the *Recent* assertion fails,
    /// reporting 1 entry where the library has 2.
    ///
    /// The stale-id half is the same shape as `Nav::from_id`'s: `projects.json`
    /// is rebuilt on every refresh, including one caused by something *else*
    /// writing it, so an id in hand can stop resolving between frames.
    #[test]
    fn the_filter_selects_only_where_it_is_offered() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "filter");
        app.library.projects.projects.push(project::Project {
            id: "p-1".into(),
            name: "Kestrel".into(),
            color: project::PROJECT_COLORS[0].into(),
            folder: None,
            created: 0,
            archived: false,
        });
        let kestrel = app.library.projects.projects[0].clone();
        let mut a = ondin_core::Document::new(app.session.ids.mint());
        store::file_document(&root, Some(&kestrel), "Landing v4", &mut a).unwrap();
        let mut b = ondin_core::Document::new(app.session.ids.mint());
        store::file_document(&root, None, "Loose sketch", &mut b).unwrap();
        app.library.refresh();
        assert_eq!(
            app.library.entries.len(),
            2,
            "the fixture has two documents"
        );
        // ⚠️ **Recent is what this machine has *opened*, not what exists**, so
        // both have to be marked or the Recent arm below reads 0 either way and
        // discriminates nothing. The first run of this probe failed here, at 0
        // against 2, which is the fixture being wrong rather than the code.
        for id in app
            .library
            .entries
            .iter()
            .filter_map(|e| e.meta.id.clone())
            .collect::<Vec<_>>()
        {
            app.library.local.mark_opened(&id);
        }

        app.dash.nav = Nav::All;
        assert!(app.filter_applies(), "All files offers the filter");
        // Drawn, not just permitted: `filter_applies` is a predicate and the
        // control reading it is the thing the user sees.
        let painted = |app: &mut OndinApp| {
            galleys(app, &ctx)
                .into_iter()
                .any(|(_, _, t)| t == ALL_PROJECTS)
        };
        assert!(painted(&mut app), "the button is on screen on All files");
        assert_eq!(app.visible_entries().len(), 2, "unfiltered, both show");
        app.dash.filter = Some("p-1".into());
        assert_eq!(
            app.visible_entries().len(),
            1,
            "filtered to Kestrel, only the filed one"
        );

        // Recent does not offer the control, so it must not obey the setting.
        app.dash.nav = Nav::Recent;
        assert!(
            !app.filter_applies(),
            "Recent is a landing page, not a list"
        );
        assert!(
            !painted(&mut app),
            "and the control is not drawn there, which is what makes the next \
             assertion the whole point"
        );
        assert_eq!(
            app.visible_entries().len(),
            2,
            "a filter with no control on screen selects nothing"
        );

        // And a project id that stops resolving is not a filter any more. Drawn
        // rather than asserted directly, because the degrade happens in the
        // control: nothing else is in a position to notice.
        app.dash.nav = Nav::All;
        app.library.projects.projects.clear();
        app.library.refresh();
        let _ = galleys(&mut app, &ctx);
        assert_eq!(
            app.dash.filter, None,
            "a filter naming a project that has gone is cleared rather than left \
             selecting nothing"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Drive one full click at `at` and return where the card of `name` was drawn.
    ///
    /// ⚠️ **Press and release are separate frames.** egui reports `clicked()` on
    /// the release, against a widget whose rect it knows from the frame before, so
    /// a probe that puts both events in one `RawInput` measures a click on a
    /// layout that has not happened yet.
    fn click_at(app: &mut OndinApp, ctx: &egui::Context, at: egui::Pos2) {
        let frame = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| app.dashboard_ui(ui));
        };
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(app, vec![egui::Event::PointerMoved(at)]);
        frame(app, vec![button(true)]);
        frame(app, vec![button(false)]);
        frame(app, vec![]);
    }

    /// The centre of the card or row whose name is `name`, from the last frame.
    fn card_centre(app: &mut OndinApp, ctx: &egui::Context, name: &str) -> egui::Pos2 {
        let (pos, size, _) = galleys(app, ctx)
            .into_iter()
            .find(|(_, _, t)| t == name)
            .unwrap_or_else(|| panic!("no card named {name:?}"));
        pos + size / 2.0
    }

    /// **A click that dismisses one of the library's floating menus is spent doing
    /// so** — §15 D558, `context-menus.md` §0 R4, `[S20.2-L1-02]`.
    ///
    /// Open the ⋮ menu on *Alpha*, decide against it, and click *Bravo* to get rid
    /// of the menu: the menu closed **and Bravo opened**, so the library was gone
    /// and the editor up on a document the user only clicked to dismiss a popup.
    /// `canvas_ui` quotes R4 and obeys it; this screen did not, at either of its
    /// two menus. On the canvas the cost is a retargeted selection; here it is a
    /// whole navigation.
    ///
    /// ⚠️ **Both menus, because they are two independent six-line dismissals** —
    /// the ⋮ popup's and the *All projects ⌄* dropdown's — and neither consumes
    /// the click. One predicate reads both.
    ///
    /// ⚠️ **The control is the assertion that makes this mean anything.** The same
    /// click with no menu open must still open the document — that is the whole
    /// point of a card — so a fix that simply stopped cards opening would pass
    /// every other assertion here. It is asserted *first*, so a broken fixture
    /// reports as a broken fixture.
    ///
    /// ⚠️ **And the menu still has to close**, which is the other half: a gate that
    /// swallowed the click *and* left the popup up would be a menu with no pointer
    /// route out of it at all.
    ///
    /// ⚠️ **Flip-check, run: the `menu_was_up` early return removed from
    /// `pick_or_open`.** Fails on the first *"must not have opened"* assertion with
    /// Bravo's path in `session.path`. The control runs before it and is green, so
    /// the gate is narrow.
    ///
    /// ⚠️ **And a second flip that is the reason this test exists rather than the
    /// obvious one-liner: `self.dash.menu_was_up` replaced by
    /// `self.library_menu_open()`.** It fails at the same assertion, because
    /// `file_menu_button` draws the popup — and runs its dismissal — from inside
    /// the card that owns it, so a menu anchored on *Alpha* is already closed by
    /// the time *Bravo*'s card is drawn. **The live flag answers "no menu" for
    /// every card after the anchor**, which means the naive fix works or does not
    /// depending on sort order. That was the first version of it, and this test
    /// caught it.
    ///
    /// ⚠️ **The dropdown row needed the fixture assertion above it for a third
    /// reason, and it is worth reading before adding a row here**: with no project
    /// in the library `project_filter` returns early and *clears* `filter_open`
    /// on the way out, so the flag was gone before the click and the row measured
    /// nothing.
    #[test]
    fn a_click_that_dismisses_a_library_menu_does_not_also_open_a_document() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "dismiss");
        let mut alpha_doc = ondin_core::Document::new(app.session.ids.mint());
        let alpha = store::file_document(&root, None, "Alpha", &mut alpha_doc).unwrap();
        let mut bravo_doc = ondin_core::Document::new(app.session.ids.mint());
        let bravo = store::file_document(&root, None, "Bravo", &mut bravo_doc).unwrap();
        app.library.refresh();
        app.dash.nav = Nav::All;

        // **The control, first**: with nothing floating, a click on Bravo opens it.
        let on_bravo = card_centre(&mut app, &ctx, "Bravo");
        click_at(&mut app, &ctx, on_bravo);
        assert_eq!(
            app.session.path.as_deref(),
            Some(bravo.as_path()),
            "control: a card with no menu open still opens on one click, which is \
             what this gate must not take away"
        );

        // The ⋮ menu on Alpha, dismissed by a click on Bravo.
        app.go_to_dashboard();
        app.dash.nav = Nav::All;
        app.session.path = None;
        let on_bravo = card_centre(&mut app, &ctx, "Bravo");
        app.dash.menu_for = Some(alpha.clone());
        click_at(&mut app, &ctx, on_bravo);
        assert_eq!(
            app.session.path, None,
            "a click aimed at getting rid of the ⋮ menu must not have opened the \
             document it landed on — the library is gone and the editor is up on a \
             file nobody asked for"
        );
        assert!(
            app.dash.menu_for.is_none(),
            "and it still has to have closed the menu, or there is no pointer \
             route out of it"
        );

        // The *All projects ⌄* dropdown, the same way.
        //
        // ⚠️ **A project has to exist first, and the fixture assertion below is
        // why this is not obvious.** `project_filter` returns early where
        // `filter_applies()` is false — *"closed as well as hidden, or the
        // dropdown survives a nav change"* — and **clears `filter_open` on the way
        // out**. So on a library with no projects the flag is gone by the end of
        // the first layout frame, and this row measured a click with nothing
        // floating: it read as the fix failing when the fixture had never reached
        // the state. Which is the finding's own scenario anyway — *"on All files
        // with at least one active project"*.
        app.library
            .projects
            .projects
            .push(crate::library::project::Project {
                id: "p-1".into(),
                name: "Kestrel".into(),
                color: crate::library::project::PROJECT_COLORS[0].into(),
                folder: None,
                created: 0,
                archived: false,
            });
        app.library.save_projects();
        app.library.refresh();
        app.dash.filter_open = true;
        let on_bravo = card_centre(&mut app, &ctx, "Bravo");
        assert!(
            app.dash.filter_open,
            "the fixture has to keep the dropdown open through the layout frame, \
             or this row is a click with nothing floating"
        );
        click_at(&mut app, &ctx, on_bravo);
        assert_eq!(
            app.session.path, None,
            "the same for the project-filter dropdown, which is a second \
             independent dismissal"
        );
        assert!(!app.dash.filter_open, "and it closed too");

        // **The other two doors, which the first version of this fix missed.**
        // `pick_or_open` is the file card and row; a dismissing click also
        // reached the sidebar's nav rows and the dashed *New file* card — and
        // that one **creates** a document rather than opening one, which is
        // strictly worse than the failure the finding named. `arch-scribe` found
        // both by reading the gate against the screen's other click sites.
        // ⚠️ **The anchor has to be a document *on this nav*, and two fixture
        // traps sit in the way.** `file_menu_popup` is drawn from inside its own
        // card, so a ⋮ menu anchored on a document the current nav does not show
        // is never drawn and never dismissed — `menu_for` just stays `Some`. And
        // the dropdown is no substitute: `filter_applies()` is false on a project
        // nav, so `project_filter` returns early and clears `filter_open`, which
        // is the same trap the row above needed a project to escape. So: a
        // document filed *in* Kestrel, as Kestrel's own grid.
        let mut charlie_doc = ondin_core::Document::new(app.session.ids.mint());
        let kestrel = app.library.projects.get("p-1").cloned().unwrap();
        let charlie =
            store::file_document(&root, Some(&kestrel), "Charlie", &mut charlie_doc).unwrap();
        app.library.refresh();
        app.dash.nav = Nav::Project("p-1".into());
        let before = crate::library::scan::scan(&root).len();
        let card = card_centre(&mut app, &ctx, "New file in Kestrel");
        app.dash.menu_for = Some(charlie.clone());
        click_at(&mut app, &ctx, card);
        assert_eq!(
            crate::library::scan::scan(&root).len(),
            before,
            "a click spent getting rid of a menu created a document — the worst of \
             R4's three doors on this screen, because the other two merely navigate"
        );
        assert!(app.dash.menu_for.is_none(), "and the menu still closed");

        // And the sidebar: a dismissing click on a project row must not re-nav.
        app.dash.nav = Nav::All;
        let row = card_centre(&mut app, &ctx, "Kestrel");
        app.dash.menu_for = Some(alpha.clone());
        click_at(&mut app, &ctx, row);
        assert_eq!(
            app.dash.nav,
            Nav::All,
            "nor may it move the sidebar off the screen the user was reading"
        );

        // **`nav_row`, the sidebar's other five rows.** Asserted because the
        // predicate is copied and a copied predicate is a second thing to fall
        // out of step — `arch-scribe` noted two of the gates had no assertion
        // behind them at all.
        app.dash.nav = Nav::Project("p-1".into());
        let all_files = card_centre(&mut app, &ctx, "All files");
        app.dash.menu_for = Some(charlie.clone());
        click_at(&mut app, &ctx, all_files);
        assert_eq!(
            app.dash.nav,
            Nav::Project("p-1".into()),
            "a dismissing click on a sidebar *nav* row must not re-nav either — \
             `nav_row` carries the same predicate as `project_row` and nothing \
             was watching it"
        );

        // **The body header's controls**, of which the search is the one with a
        // visible consequence.
        app.dash.nav = Nav::All;
        let search = card_centre(&mut app, &ctx, "Search files and projects");
        app.dash.menu_for = Some(alpha.clone());
        click_at(&mut app, &ctx, search);
        assert!(
            app.dash.search.is_none(),
            "nor may it open the search overlay — the header's controls are the \
             eighth door and were the last to be found"
        );

        // **The control for all of them**, without which a gate that broke them
        // outright would pass: the same clicks with nothing floating.
        let row = card_centre(&mut app, &ctx, "Kestrel");
        click_at(&mut app, &ctx, row);
        assert_eq!(
            app.dash.nav,
            Nav::Project("p-1".into()),
            "control: a sidebar row with no menu open still navigates"
        );
        let card = card_centre(&mut app, &ctx, "New file in Kestrel");
        click_at(&mut app, &ctx, card);
        assert_eq!(
            crate::library::scan::scan(&root).len(),
            before + 1,
            "control: and the New file card still creates one"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A single click **opens** a document, and never selects one; the keyboard's
    /// cursor is a separate thing that still works, and a cursor whose file has
    /// gone does not survive the next frame.
    ///
    /// ⚠️ **The interesting assertion is still the hit test**, and it is about egui
    /// rather than about this screen: the background is `ui.interact`ed over the
    /// *whole* body before any card is drawn, and it must lose to every card on top
    /// of it. egui takes the **last** widget registered at a point, so the order in
    /// `dashboard_body` is the mechanism — reversed, the background would swallow
    /// every click on the grid and nothing would open at all. Flip-checked by moving
    /// the `ui.interact` below the `ScrollArea`: the first assertion fails with the
    /// session still on no document, which is the symptom exactly.
    ///
    /// ⚠️ **And that the click leaves `selected` alone**, which is the half a test
    /// of "does it open" would miss: the old behaviour set both, so an assertion
    /// about opening alone would pass against a card that still lights an outline
    /// nothing acts on (§15 D381).
    #[test]
    fn a_click_opens_a_document_and_leaves_the_keyboard_cursor_alone() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "pick");
        let mut doc = ondin_core::Document::new(app.session.ids.mint());
        let path = store::file_document(&root, None, "Landing v4", &mut doc).unwrap();
        app.library.refresh();
        app.dash.nav = Nav::All;
        assert!(app.session.path.is_none(), "nothing is open to begin with");

        let on_card = card_centre(&mut app, &ctx, "Landing v4");
        click_at(&mut app, &ctx, on_card);
        assert_eq!(
            app.session.path.as_deref(),
            Some(path.as_path()),
            "one click on a card opens it — it took two until D381"
        );
        assert_eq!(
            app.dash.selected, None,
            "and picks nothing out on the way: the outline is the keyboard's now"
        );

        // Back to the library, and the keyboard cursor from there.
        app.go_to_dashboard();
        press(&mut app, &ctx, egui::Key::ArrowRight);
        assert_eq!(
            app.dash.selected.as_deref(),
            Some(path.as_path()),
            "the arrows still put a cursor on a card (D374)"
        );

        // The bottom-left of the body, which is below one row of one card and so
        // is the background by construction.
        let empty = egui::pos2(SIDEBAR_W + 40.0, SCREEN.y - 40.0);
        click_at(&mut app, &ctx, empty);
        assert_eq!(
            app.dash.selected, None,
            "and a click on nothing puts it back"
        );

        // A cursor whose file has gone is cleared by the next frame that draws.
        app.dash.selected = Some(path.clone());
        std::fs::remove_file(&path).unwrap();
        app.library.refresh();
        let _ = galleys(&mut app, &ctx);
        assert_eq!(
            app.dash.selected, None,
            "a path that names nothing is not a cursor"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A dropped `.ondin` is imported; a dropped anything-else is not, and says so
    /// once rather than once per file.
    ///
    /// ⚠️ **The fixture is a document *outside* the library**, because
    /// `import_paths` deliberately skips files already under the base folder — a
    /// probe that dropped one of the library's own would assert that nothing
    /// happened and pass for the wrong reason.
    ///
    /// **Flip-checked** by removing the `take_dropped_documents` call from
    /// `dashboard_ui` — which is the state this shipped in — and the first
    /// assertion fails at 0 imported against 1.
    #[test]
    fn a_dropped_document_is_imported_and_a_dropped_image_is_not() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "drop");
        // Process-unique: `app`'s root is salted by pid but `root.parent()` is
        // `temp_dir()` itself, so this deliberately-outside fixture was shared —
        // and it opens by deleting the directory, which is a concurrent run's
        // fixture. Same defect as `recovery::a_key_that_is_a_path_is_refused`'s
        // victim file, and the same cause: a pid on the directory is not a pid on
        // a fixture that is meant to sit beside it.
        let outside = root
            .parent()
            .expect("temp dir")
            .join(format!("ondin-drop-source-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();
        let mut doc = ondin_core::Document::new(app.session.ids.mint());
        let source = store::file_document(&outside, None, "Dropped in", &mut doc).unwrap();
        app.library.refresh();
        assert!(app.library.entries.is_empty(), "the library starts empty");

        let drop = |app: &mut OndinApp, path: &std::path::Path| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
                dropped_files: vec![egui::DroppedFile {
                    path: Some(path.to_path_buf()),
                    ..Default::default()
                }],
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| app.dashboard_ui(ui));
        };

        drop(&mut app, &source);
        assert_eq!(
            app.library.entries.len(),
            1,
            "a document dropped on the library is filed in it"
        );

        // Anything that is not a document is refused, and the library is untouched.
        let png = outside.join("picture.png");
        std::fs::write(&png, [0u8; 8]).unwrap();
        drop(&mut app, &png);
        assert_eq!(
            app.library.entries.len(),
            1,
            "an image is not a document — the library takes .ondin files"
        );
        // On the screen, not on the field — the same conversion as the mixed drop
        // below and for the same reason (§15 D605). `arch-scribe` found this one
        // surviving after the other had moved, which is the ordinary way a
        // two-site fix comes out one short.
        // Two assertions since §15 D751 — the dot is the surface and the slot is
        // the wording; the tooltip that joins them is unassertable.
        assert!(
            status_dot_color(&mut app, &ctx).is_some(),
            "the bar is marked"
        );
        assert!(
            app.session.status().text.contains(".ondin"),
            "and it says what this screen takes: {}",
            app.session.status().text
        );

        // ⚠️ **The mixed drop, which the first version of this was silent about.**
        // It reported the document it took and said nothing at all about the file
        // it ignored, because the refusal was gated on nothing having been
        // imported. One slot, one sentence, both facts.
        let mut second = ondin_core::Document::new(app.session.ids.mint());
        let source2 = store::file_document(&outside, None, "Also dropped", &mut second).unwrap();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
            dropped_files: vec![
                egui::DroppedFile {
                    path: Some(source2),
                    ..Default::default()
                },
                egui::DroppedFile {
                    path: Some(png),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| app.dashboard_ui(ui));
        assert_eq!(
            app.library.entries.len(),
            2,
            "the document in the pile lands"
        );
        // ⚠️ **Asserted on the *screen*, not on the field** (§15 D605), which is
        // `a_missing_library_folder_says_so_instead_of_reading_as_empty`'s note
        // arriving at the second of the two sites `[S20.1-L6-04]` names. This read
        // `app.session.status().text` — a `String` with exactly one production
        // reader, the editor's top bar, which sits below `OndinApp::ui`'s
        // `View::Dashboard` return — so the test was named for a sentence the user
        // is told and asserted a value nobody could see. **No flip demonstrated
        // that at the time: the plausible wrong version was what shipped**, which
        // is G19's shape. `[S20.1-L1-01]` painted the line; this is what pins it
        // here.
        //
        // Flip-check, run now that there is something to flip: deleting the
        // status call from the top bar fails the *first* assertion below.
        // **The field assertion this replaces stays green under that flip**,
        // which is the finding in one line — and it is why the surface is still
        // asserted separately now that §15 D751 has made the surface a dot. The
        // second assertion is the one that would have been green all along.
        assert!(
            status_dot_color(&mut app, &ctx).is_some(),
            "the bar is marked"
        );
        assert!(
            app.session.status().text.contains("Imported 1")
                && app.session.status().text.contains("1 other file(s)"),
            "and the one it ignored is named in the same sentence: {}",
            app.session.status().text
        );

        let _ = std::fs::remove_dir_all(&outside);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Seven of the sixteen things an import can have to say, in one sentence
    /// each.
    ///
    /// ⚠️ **Both refusal clauses have to survive the *happy* arms**, which is the
    /// whole reason this is a function with a test rather than a `match` inline:
    /// the bug it was extracted from reported a clean import and dropped the
    /// refusal on the floor. Flip-checked by moving `rest` into the `(0, 0)` arm
    /// only — the second, fourth and sixth assertions fail.
    ///
    /// ⚠️ **The `(0, 0)` arm no longer names a reason it was not given** (§15
    /// D693). It used to read *"Nothing to import — those files are already in
    /// the library"* unconditionally, which is a claim about *why*, and the
    /// fifth assertion below is the case that made it false: forty pictures and
    /// no documents is `(0, 0)` with `already == 0`. Asserting the old sentence
    /// was what made the bug look covered.
    #[test]
    fn an_import_says_what_it_took_and_what_it_left() {
        assert_eq!(
            import_summary(3, 0, 0, 0),
            "Imported 3 file(s)",
            "the ordinary case says nothing it does not have to"
        );
        assert_eq!(
            import_summary(3, 0, 40, 0),
            "Imported 3 file(s) — 40 other file(s) were not Ondin documents",
            "a mixed drop names both halves"
        );
        assert_eq!(
            import_summary(0, 0, 0, 3),
            "Nothing to import — 3 file(s) already in the library"
        );
        assert_eq!(
            import_summary(0, 2, 1, 0),
            "2 file(s) could not be imported — 1 other file(s) were not Ondin documents"
        );
        assert_eq!(
            import_summary(0, 0, 40, 0),
            "Nothing to import — 40 other file(s) were not Ondin documents",
            "forty pictures and no documents is not the library already holding them"
        );
        assert_eq!(
            import_summary(1, 0, 2, 3),
            "Imported 1 file(s) — 3 file(s) already in the library \
             — 2 other file(s) were not Ondin documents",
            "both clauses, in the order the two skips happen"
        );
        assert_eq!(
            import_summary(1, 2, 0, 0),
            "Imported 1 file(s); 2 could not be read"
        );
    }

    /// **A partial import names the files it left behind** (§15 D693,
    /// `[S20.2-L1-06]`) — driven through `import_counting` rather than through
    /// the formatter, because the formatter was never the broken half.
    ///
    /// 🚨 **The whole finding is that the *producer* had no test.**
    /// `an_import_says_what_it_took_and_what_it_left` enumerates
    /// `import_summary`'s arms by hand and never calls `import_counting`, and
    /// `a_dropped_document_is_imported_and_a_dropped_image_is_not` drops only
    /// files from a sibling folder — its own doc says so, and says why. So the
    /// rule read as covered because the formatter was covered, while the count
    /// that feeds it dropped a whole category on the floor. This is the probe
    /// that closes the seam.
    ///
    /// ⚠️ **The mix is the case, not the all-skipped one.** All-skipped was
    /// already right by accident: it is `(0, 0)`, and that arm asserted the
    /// in-library reason without being told it. Two in and one out is what
    /// reported *"Imported 1 file(s)"* and said nothing about the other two.
    ///
    /// ⚠️ **Flip:** restoring the bare `continue` — dropping `already += 1` —
    /// fails the second assertion with `left: (1, 0, 0)`, and then the third
    /// with the sentence back to a bare `"Imported 1 file(s)"`.
    ///
    /// (Plain backticks per §15 D319 — `cargo doc` builds without the `test` cfg.)
    #[test]
    fn an_import_of_files_already_in_the_library_says_so() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "already");
        let outside = root
            .parent()
            .expect("temp dir")
            .join(format!("ondin-already-source-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();

        // Two documents filed *into* the library, and one left outside it.
        let mut a = ondin_core::Document::new(app.session.ids.mint());
        let inside_a = store::file_document(&root, None, "Already A", &mut a).unwrap();
        let mut b = ondin_core::Document::new(app.session.ids.mint());
        let inside_b = store::file_document(&root, None, "Already B", &mut b).unwrap();
        let mut c = ondin_core::Document::new(app.session.ids.mint());
        let source = store::file_document(&outside, None, "Brought in", &mut c).unwrap();
        app.library.refresh();
        assert!(
            inside_a.starts_with(&root) && inside_b.starts_with(&root),
            "the fixture must reach the state: two of the three really are under \
             the base folder, or the skip arm is never taken"
        );

        let counts = app.import_counting(&[inside_a, inside_b, source]);
        assert_eq!(
            counts,
            (1, 0, 2),
            "one taken, none failed, and the two already here are counted rather \
             than dropped"
        );

        let (ok, failed, already) = counts;
        assert_eq!(
            import_summary(ok, failed, 0, already),
            "Imported 1 file(s) — 2 file(s) already in the library",
            "the reported symptom was this sentence ending after `1 file(s)`"
        );

        let _ = std::fs::remove_dir_all(&outside);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A base folder naming an existing *file* is refused, and nothing about
    /// it is persisted** (§15 D700, `[S20.3-L1-05]`).
    ///
    /// The old order created the folder at the *bottom* of the block with
    /// `let _ = create_dir_all(…)`, after the migration and after
    /// `prefs.base_folder` had been assigned. So a mis-copied path — a file, a
    /// `.lnk`, a sync placeholder — was adopted: the root became the file, every
    /// write door refused, `root_unavailable` went true, the preference was
    /// written to disk so the state survived a relaunch, and the card said
    /// nothing whatever.
    ///
    /// ⚠️ **The sentence is asserted on the *screen*, not on
    /// `session.status().text`** — §15 D605's rule, learnt on this very panel:
    /// that field's only production reader used to be the editor's top bar, so a
    /// test that read it was named for something the user is told and asserted a
    /// value nobody could see. `[S20.1-L1-01]` painted the line at `:653` and
    /// this asserts what it paints.
    ///
    /// ⚠️ **The unrelated settings must survive**, which is the third assertion.
    /// Discarding the autosave interval because a path was mistyped is a second
    /// surprise, and a refusal written as an early `return` would do exactly
    /// that.
    ///
    /// ⚠️ **Flip:** restoring `let _ = std::fs::create_dir_all(&new_root)` at the
    /// foot of the block fails the first assertion — the root becomes the file
    /// — and then the second, the preference having been persisted.
    ///
    /// (Plain backticks per §15 D319 — `cargo doc` builds without the `test` cfg.)
    #[test]
    fn a_base_folder_that_names_a_file_is_refused_and_not_persisted() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "base-is-file");

        let file = root.join("not-a-folder.txt");
        std::fs::write(&file, b"not a folder").unwrap();
        assert!(
            file.is_file(),
            "the fixture must reach the state: the path really is a file"
        );

        let root_before = app.library.root.clone();
        let pref_before = app.prefs.base_folder.clone();

        let mut d = LibrarySettings::from_prefs(&app.prefs, &app.library.root);
        d.base_folder = file.display().to_string();
        d.migrate = false;
        d.autosave_secs = 97;
        app.library_settings = Some(d);
        app.apply_library_settings();

        assert_eq!(
            app.library.root, root_before,
            "the library folder is unchanged"
        );
        assert_eq!(
            app.prefs.base_folder, pref_before,
            "and nothing was written to preferences, so a relaunch is not stuck there"
        );
        assert_eq!(
            app.prefs.autosave_secs, 97,
            "the settings that were not about the folder still applied"
        );

        // **On the screen rather than only in a field** — two assertions since
        // §15 D751, because this screen's status is a dot with the sentence on
        // hover and a tooltip's ink never reaches `.shapes`. The dot is the
        // *surface* (without it the message is written where nobody can read it,
        // which is the failure D426 exists for) and the slot is the *wording*.
        assert!(
            status_dot_color(&mut app, &ctx).is_some(),
            "the bar is marked"
        );
        assert!(
            app.session.status().text.contains("library folder"),
            "and the refusal names what was wrong: {}",
            app.session.status().text
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A partly-failed migration names every file it left behind, on the screen,
    /// and the retry finishes the job** (§15 D810).
    ///
    /// 🚨 **The list existed and reached nothing.** `Moved::failed` has held every
    /// stranded path since §15 D620 and `Moved::summary` put the *first* of them
    /// in the status line, which the next message replaces — so a migration that
    /// stranded a document was, a minute later, recoverable only by hunting the
    /// old folder by hand, with `apply_library_settings` having re-pointed the app
    /// at the new root either way.
    ///
    /// **The failure is arranged rather than mocked**, by `relocate`'s own trick:
    /// a *file* where a project's *folder* has to go, so `create_dir_all` refuses
    /// for exactly one document and the rest of the migration runs. Removing it is
    /// then a real repair rather than a flag being flipped, which is what makes
    /// the retry half of this test mean anything.
    ///
    /// ⚠️ **Three claims and they fail in different places on purpose.** The
    /// record is a field, the report is *galleys* — a list nobody can see is the
    /// defect this closes, and §15 D426 is the entry for asserting a message on a
    /// `String` nobody draws — and the retry is the filesystem.
    ///
    /// ⚠️ **The loose document is the control.** A version that gave up at the
    /// first failure would satisfy every assertion about the stranded one.
    ///
    /// **Flip-checks, both run, and the second is why the fourth assertion
    /// exists.** Dropping the `self.stranded = …` assignment in
    /// `apply_library_settings` fails at *"the migration left a report"*, the
    /// predicted site. Dropping the `self.library.refresh()` from
    /// `retry_migration` left every assertion here **green** on the first
    /// reading — the file arrives and the record clears whatever the listing says
    /// — which is a coverage finding rather than a failed experiment: the retry's
    /// whole effect on the *dashboard behind the modal* was untested. The
    /// `library.entries` assertion is what that flip bought, and it fails at
    /// `["loose-sketch"]` now.
    #[test]
    fn a_partly_failed_migration_is_listed_and_can_be_run_again() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, from) = app(&ctx, "stranded");
        let to =
            std::env::temp_dir().join(format!("ondin-dash-stranded-to-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&to);
        std::fs::create_dir_all(&to).unwrap();

        let kestrel = crate::library::project::Project {
            id: "p-1".into(),
            name: "Kestrel".into(),
            color: crate::library::project::PROJECT_COLORS[0].into(),
            folder: Some("kestrel".into()),
            created: 0,
            archived: false,
        };
        crate::library::project::Projects {
            version: 1,
            projects: vec![kestrel.clone()],
        }
        .write(&from)
        .unwrap();
        let mut doc = ondin_core::Document::new(ondin_core::IdSource::new(0xD0C).mint());
        let stuck =
            crate::library::store::file_document(&from, Some(&kestrel), "Landing v4", &mut doc)
                .unwrap();
        let mut loose = ondin_core::Document::new(ondin_core::IdSource::new(0xD0D).mint());
        crate::library::store::file_document(&from, None, "Loose sketch", &mut loose).unwrap();

        // The obstruction: a file where the project folder has to be.
        let blocker = to.join("kestrel");
        std::fs::write(&blocker, b"not a directory").unwrap();

        let mut d = LibrarySettings::from_prefs(&app.prefs, &app.library.root);
        d.base_folder = to.display().to_string();
        d.migrate = true;
        app.library_settings = Some(d);
        app.apply_library_settings();

        let report = app
            .stranded
            .clone()
            .expect("the migration left a report of what it could not move");
        assert_eq!(report.failed, vec![stuck.clone()], "and it names the file");
        assert_eq!(report.from, from, "and the folder the file is still in");
        assert!(stuck.exists(), "which it is — a failed move loses nothing");
        assert!(
            to.join("loose-sketch.ondin").exists(),
            "control: the rest of the migration ran"
        );

        // The modal is where it is readable, and that is the half that was missing.
        app.library_settings = Some(LibrarySettings::from_prefs(&app.prefs, &app.library.root));
        let shown = galleys(&mut app, &ctx);
        let text = |needle: &str| shown.iter().any(|(_, _, t)| t.contains(needle));
        assert!(
            text("could not be moved"),
            "the card says a migration left something behind: {:?}",
            shown.iter().map(|(_, _, t)| t).collect::<Vec<_>>()
        );
        assert!(text("landing-v4.ondin"), "and names the file");
        assert!(
            text(&from.display().to_string()),
            "and the folder to go and look in"
        );
        assert!(text("Try again"), "and offers to run it again");

        // Clear the obstruction and take the offer.
        std::fs::remove_file(&blocker).unwrap();
        app.retry_migration();

        assert!(
            to.join("kestrel").join("landing-v4.ondin").is_file(),
            "the retry moved what was left"
        );
        assert!(!stuck.exists(), "and it is not in two places");
        assert!(
            app.library.entries.iter().any(|e| e.stem == "landing-v4"),
            "and the library behind the modal lists it: {:?}",
            app.library
                .entries
                .iter()
                .map(|e| &e.stem)
                .collect::<Vec<_>>()
        );
        assert!(
            app.stranded.is_none(),
            "and the report is gone, so the card stops offering: {:?}",
            app.stranded
        );

        let _ = std::fs::remove_dir_all(&from);
        let _ = std::fs::remove_dir_all(&to);
    }

    /// A report the tests below plant by hand: one file left in `from` on the
    /// way to `to`. **Planted rather than produced**, because what is under test
    /// is what a *later* settings change and a *later* retry do with a report,
    /// and `a_partly_failed_migration_is_listed_and_can_be_run_again` already
    /// covers how one is made.
    fn planted_report(from: &Path, to: &Path) -> (Stranded, PathBuf) {
        std::fs::create_dir_all(from).unwrap();
        let left = from.join("left-behind.ondin");
        let doc = ondin_core::Document::new(ondin_core::IdSource::new(0xDA).mint());
        std::fs::write(&left, ondin_core::io::save(&doc).unwrap()).unwrap();
        let report = Stranded {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            failed: vec![left.clone()],
        };
        (report, left)
    }

    /// **Every change of base folder settles the last report, migrating or not**
    /// (§15 D846, `[X1.2-L1-01]`, `[X1.2-L6-03]`).
    ///
    /// §15 D810 says the report is *"assigned in both directions so a clean
    /// migration clears a previous report"*, and nothing tested that half: a
    /// version with no `None` arm passed the suite. And the both-directions
    /// assignment sat inside `if d.migrate`, so a change that did **not** migrate
    /// assigned nothing — and that is where the retry into a folder that is no
    /// longer the library came from.
    ///
    /// ⚠️ **Flip-checks, run**: `if !failed.is_empty() { stranded = Some(..) }`
    /// in place of the `then` fails at *"a clean migration clears"*; deleting
    /// the `else { self.stranded = None }` fails at *"and so does a change that
    /// moves nothing"*.
    #[test]
    fn every_change_of_base_folder_settles_the_last_report() {
        let ctx = egui::Context::default();
        let (mut app, root) = app(&ctx, "report-settles");
        let elsewhere = root.with_file_name(format!("{}-b", root.file_name().unwrap().display()));
        let third = root.with_file_name(format!("{}-c", root.file_name().unwrap().display()));
        for dir in [&elsewhere, &third] {
            let _ = std::fs::remove_dir_all(dir);
        }
        let (report, _) = planted_report(&root.join("old"), &root);

        // A clean migration.
        app.stranded = Some(report.clone());
        let mut d = LibrarySettings::from_prefs(&app.prefs, &app.library.root);
        d.base_folder = elsewhere.display().to_string();
        d.migrate = true;
        app.library_settings = Some(d);
        app.apply_library_settings();
        assert_eq!(app.library.root, elsewhere, "the fixture moved the library");
        assert!(
            app.stranded.is_none(),
            "a clean migration clears the last report: {:?}",
            app.stranded
        );

        // A change that moves nothing.
        app.stranded = Some(report);
        let mut d = LibrarySettings::from_prefs(&app.prefs, &app.library.root);
        d.base_folder = third.display().to_string();
        d.migrate = false;
        app.library_settings = Some(d);
        app.apply_library_settings();
        assert_eq!(app.library.root, third, "the fixture moved the library");
        assert!(
            app.stranded.is_none(),
            "and so does a change that moves nothing: {:?}",
            app.stranded
        );

        for dir in [&root, &elsewhere, &third] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// **A retry refuses a report made for another library, and keeps it**
    /// (§15 D846, `[X1.2-L1-01]`).
    ///
    /// The planted report says the files were going to a folder that is not the
    /// library. Run anyway, that moved them somewhere nothing lists and then
    /// cleared the one record of where. **The fixture file is the loss**, so it
    /// is asserted first.
    ///
    /// ⚠️ **Flip-check, run**: deleting the `stranded.to != self.library.root`
    /// refusal fails at *"the file is where it was"* — the move happened.
    #[test]
    fn a_retry_refuses_a_report_made_for_another_library() {
        let ctx = egui::Context::default();
        let (mut app, root) = app(&ctx, "retry-other");
        let not_the_library = root.join("not-the-library");
        let (report, left) = planted_report(&root.join("old"), &not_the_library);
        app.stranded = Some(report.clone());

        app.retry_migration();
        assert!(left.exists(), "the file is where it was");
        assert!(
            !not_the_library.join("left-behind.ondin").exists(),
            "and nothing went to the folder that is not the library"
        );
        assert_eq!(app.stranded, Some(report), "and the list is kept");
        assert!(
            app.session.status().text.contains("not the library folder"),
            "and the refusal says why: {}",
            app.session.status().text
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A retry against an old folder that cannot be reached keeps the list**
    /// (§15 D846, `[R3-L5-02]`).
    ///
    /// `relocate` answers *"Nothing to move"* for a source that does not exist,
    /// which is true of a folder that is gone and false of a share that is down
    /// — and the retry took it at its word, clearing the only record of which
    /// files were left behind.
    ///
    /// ⚠️ **Flip-check, run**: deleting the `scan::readable` refusal fails at
    /// *"the list is kept"*, `left: None` — the list cleared, the symptom as the
    /// finding reported it.
    #[test]
    fn a_retry_against_an_unreachable_old_folder_keeps_the_list() {
        let ctx = egui::Context::default();
        let (mut app, root) = app(&ctx, "retry-unreachable");
        let (mut report, _) = planted_report(&root.join("old"), &root);
        // Unplugged: the folder the report names is not there to list.
        report.from = root.join("an-unplugged-drive");
        app.stranded = Some(report.clone());

        app.retry_migration();
        assert_eq!(app.stranded, Some(report), "the list is kept");
        assert!(
            app.session.status().text.contains("Could not reach"),
            "and the status says the folder was not there rather than empty: {}",
            app.session.status().text
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The stranded list shows `STRANDED_ROWS` names and says how many it did
    /// not** (§15 D846, `[X1.2-L6-04]`).
    ///
    /// The only test that draws the list strands one file, so deleting
    /// `.take(STRANDED_ROWS)` — letting the card grow past the window, which is
    /// the reason the cap exists — passed, and so did deleting the *"…and N
    /// more"* line.
    ///
    /// ⚠️ **Flip-checks, run**: removing the `.take` fails at *"the seventh is
    /// not drawn"*; removing the *"…and N more"* block fails at *"and the rest
    /// are counted"*.
    #[test]
    fn the_stranded_list_is_capped_and_counts_the_rest() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "stranded-cap");
        let failed: Vec<PathBuf> = (1..=STRANDED_ROWS + 2)
            .map(|n| root.join("old").join(format!("stranded-{n}.ondin")))
            .collect();
        app.stranded = Some(Stranded {
            from: root.join("old"),
            to: root.clone(),
            failed,
        });
        app.library_settings = Some(LibrarySettings::from_prefs(&app.prefs, &app.library.root));

        let shown = galleys(&mut app, &ctx);
        let text = |needle: &str| shown.iter().any(|(_, _, t)| t.contains(needle));
        assert!(
            text(&format!("stranded-{STRANDED_ROWS}.ondin")),
            "the last row under the cap is drawn — the fixture reached the list"
        );
        assert!(
            !text(&format!("stranded-{}.ondin", STRANDED_ROWS + 1)),
            "the seventh is not drawn"
        );
        assert!(text("…and 2 more"), "and the rest are counted");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The header's three controls are one row of one height, sharing one centre.
    ///
    /// ⚠️ **The centre is what this is really about, not the height.** A
    /// `ui.horizontal` centres each item against the row height it knows *when that
    /// item is allocated*, so a cluster whose members disagree by two points lands
    /// them on different baselines — the fault `settings::font_cache_row` carries a
    /// warning about. Flipping `VIEW_CELL_H` from `HEADER_CONTROL_H - 4.0` to
    /// `HEADER_CONTROL_H` (the obvious wrong version: forgetting that
    /// `ui::segmented` pads its track) fails the height assertion at 32 against
    /// 28 — and leaves the *centre* one green, because the row simply grows around
    /// both. So the height assertion is the load-bearing one here and the centre is
    /// the guard against the next change.
    #[test]
    fn the_header_controls_share_a_height_and_a_centre() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "header-row");
        app.dash.sort = Sort::Edited;
        // ⚠️ **Not *Recent*, where the sort button is not drawn at all** (§15 D381).
        // The default nav is Recent, so this measured a cluster the screen no longer
        // has there and failed looking for the label.
        app.dash.nav = Nav::All;

        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
            ..Default::default()
        };
        let mut full = ctx.run_ui(input.clone(), |ui| app.dashboard_ui(ui));
        for _ in 1..4 {
            full = ctx.run_ui(input.clone(), |ui| app.dashboard_ui(ui));
        }
        // The segmented track is the one rect in the frame exactly [`VIEW_TRACK_W`]
        // wide — the sort button is measured from its own longest label and the
        // cells inside the track are narrower.
        let track = full
            .shapes
            .iter()
            .find_map(|cs| match &cs.shape {
                egui::Shape::Rect(r) if (r.rect.width() - VIEW_TRACK_W).abs() < 0.5 => Some(r.rect),
                _ => None,
            })
            .expect("the grid/list track");
        // The sort button's ground is the rect holding its label.
        let label = full
            .shapes
            .iter()
            .find_map(|cs| match &cs.shape {
                egui::Shape::Text(t) if t.galley.text() == Sort::Edited.label() => {
                    Some(t.pos + t.galley.size() / 2.0)
                }
                _ => None,
            })
            .expect("the sort button's label");
        // ⚠️ **Bounded by height, or this finds the panel.** Every rect the frame
        // painted is searched in paint order, and the central panel's own ground
        // contains the label too — the first version of this line matched it and
        // reported the sort button as 774pt tall.
        let button = full
            .shapes
            .iter()
            .find_map(|cs| match &cs.shape {
                egui::Shape::Rect(r)
                    if r.rect.contains(label) && r.rect.height() <= HEADER_CONTROL_H =>
                {
                    Some(r.rect)
                }
                _ => None,
            })
            .expect("the sort button's ground");

        assert_eq!(
            (track.height(), button.height()),
            (HEADER_CONTROL_H, HEADER_CONTROL_H),
            "the view track and the sort button are the app's control height"
        );
        assert!(
            (track.center().y - button.center().y).abs() < 0.5,
            "the track sits at {:.1} and the sort button at {:.1}",
            track.center().y,
            button.center().y
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Drive one bare key press through a frame of the dashboard.
    ///
    /// **One frame, unlike `click_at`'s four.** A key is read out of
    /// `i.key_pressed` on the frame its event arrives, and `dashboard_keys` runs
    /// at the top of `dashboard_body` — so the selection this moves is already
    /// moved by the time the frame it was pressed on has finished drawing. A
    /// pointer click needs three frames because `clicked()` is reported on the
    /// release, against a rect laid out the frame before.
    fn press(app: &mut OndinApp, ctx: &egui::Context, key: egui::Key) {
        press_with(app, ctx, key, Default::default());
    }

    /// The same, for a chord.
    ///
    /// ⚠️ **The modifiers go on the frame as well as on the event, and the frame
    /// is the half that matters.** `dashboard_keys` reads `i.modifiers`, and
    /// `InputState` takes that field straight from `RawInput::modifiers`
    /// (`input_state/mod.rs`, `modifiers: new.modifiers`) rather than from the
    /// `Event::Key` it is attached to — so a `Ctrl+N` spelled only on the event
    /// arrives as a bare `N` and is refused by the `plain` gate it was written to
    /// pass. Both are set here because the real app sets both.
    fn press_with(
        app: &mut OndinApp,
        ctx: &egui::Context,
        key: egui::Key,
        modifiers: egui::Modifiers,
    ) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
            modifiers,
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| app.dashboard_ui(ui));
    }

    /// The arithmetic behind the arrow keys, on its own.
    ///
    /// ⚠️ **Flip-checked against the plausible wrong version rather than against
    /// nothing.** Replacing the `clamp` with a range test that returns `None` off
    /// the end — which is the first implementation anyone writes — was predicted
    /// to bite on the `Up` from index 0. It did **not**: `0 + -4 = -4` is out of
    /// range for both, and the wrong version returns `None` while this one returns
    /// `Some(0)`, so that assertion *does* fail — but the one that fails **first**
    /// is the short-last-row `Down`, four lines above it, because assertions run
    /// in order and that is the one the feature is actually about. Both are worth
    /// keeping: the first says the grid has no dead press at the bottom, the
    /// second that it has none at the top either.
    #[test]
    fn the_arrows_step_in_reading_order_and_clamp_at_the_ends() {
        use egui::Key::{ArrowDown, ArrowLeft, ArrowRight, ArrowUp};
        // Six items in a four-wide grid: one full row and a short one.
        let g = |cur, key| arrow_target(cur, key, false, 6);
        assert_eq!(
            g(None, ArrowDown),
            Some(0),
            "the first press enters the grid at the top"
        );
        assert_eq!(g(None, ArrowUp), Some(5), "and from below, at the bottom");
        assert_eq!(
            g(Some(3), ArrowRight),
            Some(4),
            "right off the end of a row is the start of the next"
        );
        assert_eq!(g(Some(4), ArrowLeft), Some(3), "and left wraps back");
        assert_eq!(g(Some(1), ArrowDown), Some(5), "down is a whole row");
        assert_eq!(g(Some(5), ArrowUp), Some(1), "and up is that row back");
        assert_eq!(
            g(Some(2), ArrowDown),
            Some(5),
            "down from a column the short last row does not reach lands on the \
             last card rather than doing nothing"
        );
        assert_eq!(g(Some(0), ArrowUp), Some(0), "up on the top row stays put");
        assert_eq!(
            g(Some(5), ArrowDown),
            Some(5),
            "and down on the last card stays put"
        );
        // The list is one column wide, so its verticals are single steps and it
        // has no horizontals at all.
        let l = |cur, key| arrow_target(cur, key, true, 6);
        assert_eq!(l(Some(2), ArrowDown), Some(3));
        assert_eq!(l(Some(2), ArrowUp), Some(1));
        assert_eq!(
            l(Some(2), ArrowLeft),
            None,
            "a full-width row has nothing beside it"
        );
        assert_eq!(l(Some(2), ArrowRight), None);
        // ⚠️ The caller indexes `entries` with what this returns, so `None` on an
        // empty list is the thing standing between an arrow key and a panic.
        assert_eq!(arrow_target(None, ArrowDown, false, 0), None);
        assert_eq!(arrow_target(Some(0), ArrowUp, true, 0), None);
    }

    /// Arrows move the selection, `Escape` puts it back, `Enter` opens it and
    /// `Delete` asks first — and none of the four reaches the library while the
    /// search overlay has the keyboard (§15 D374).
    ///
    /// ⚠️ **The last assertion has teeth, and the flip aimed at it does not
    /// bite — which is a finding about the guard rather than about the test.**
    /// Removing `self.dash.search.is_some()` from `dashboard_keys`' guard leaves
    /// every assertion here green: the search field is drawn in the *top bar*,
    /// which runs before `dashboard_body`, so by the time the keymap is read
    /// `edit.request_focus()` has already made `egui_wants_keyboard_input` true on
    /// the same frame. The prediction was the opposite — that focus lands a frame
    /// late and only the explicit flag holds — and it is wrong here for the same
    /// reason §15 D317 gives: egui settles focus inside the pass, not after it.
    /// Removing **both** guards fails only this assertion, at the picked path
    /// against `None`, so what the pair really is is one guard and one belt: the
    /// flag does not depend on a draw order two functions apart staying what it
    /// is.
    ///
    /// ⚠️ **The dropdown rows added by §15 D578 have their own flip, and it bites at
    /// the *first* of the three** — the guard put back to `self.dash.menu_for
    /// .is_some()` fails on *"the filter dropdown is open, so the arrows are not the
    /// grid's either"*, Beta against Alpha. Predicted at the `Escape` assertion,
    /// which is the one the finding's title is about; the arrow simply comes first
    /// and the guard is one predicate for all three. **Three assertions, one flip
    /// site** — worth knowing before adding a fourth and expecting it to be
    /// independently covered.
    ///
    /// 🚨 **That paragraph was written into the *next* test's doc comment, and
    /// `arch-scribe` moved it back here.** It described these assertions from under
    /// a heading about `Ctrl+K`, two functions away. Not a theft — both docs kept
    /// their own items, so neither detector could see it — but it is the eighth
    /// insertion-trap instance's lesson by another road: **a flip note filed under
    /// the wrong name is invisible to the person changing the thing it is about.**
    /// The only check is somebody reading the note against the test it names.
    #[test]
    fn the_keyboard_moves_the_selection_opens_it_and_asks_before_deleting() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "keys");
        let mut first = ondin_core::Document::new(app.session.ids.mint());
        let a = store::file_document(&root, None, "Alpha", &mut first).unwrap();
        let mut second = ondin_core::Document::new(app.session.ids.mint());
        let b = store::file_document(&root, None, "Beta", &mut second).unwrap();
        app.library.refresh();
        app.dash.nav = Nav::All;
        // **Named rather than left on the default**, which is *Edited* — two files
        // written in the same second have no order under it, and a test whose
        // fixture is in an order it did not set is about nothing.
        app.dash.sort = Sort::Name;
        assert_eq!(
            app.visible_entries()
                .iter()
                .map(|e| e.display_name())
                .collect::<Vec<_>>(),
            vec!["Alpha".to_string(), "Beta".to_string()],
            "the fixture is in the order the arrows are asserted against"
        );

        press(&mut app, &ctx, egui::Key::ArrowDown);
        assert_eq!(
            app.dash.selected.as_deref(),
            Some(a.as_path()),
            "the first arrow enters the list"
        );
        press(&mut app, &ctx, egui::Key::ArrowDown);
        assert_eq!(app.dash.selected.as_deref(), Some(b.as_path()));
        press(&mut app, &ctx, egui::Key::ArrowUp);
        assert_eq!(app.dash.selected.as_deref(), Some(a.as_path()));
        press(&mut app, &ctx, egui::Key::Escape);
        assert_eq!(
            app.dash.selected, None,
            "Escape is the keyboard's click on the empty body"
        );

        press(&mut app, &ctx, egui::Key::ArrowDown);
        press(&mut app, &ctx, egui::Key::Enter);
        assert_eq!(
            app.session.path.as_deref(),
            Some(a.as_path()),
            "Enter opens what is picked"
        );

        app.dash.selected = Some(b.clone());
        press(&mut app, &ctx, egui::Key::Delete);
        assert_eq!(
            app.dash.deleting.as_deref(),
            Some(b.as_path()),
            "Delete opens the confirmation rather than deleting"
        );
        assert!(b.exists(), "and nothing has been moved yet");

        // The guard: with the overlay up, the arrows are the search field's.
        app.dash.deleting = None;
        app.dash.selected = None;
        app.dash.search = Some(String::new());
        press(&mut app, &ctx, egui::Key::ArrowDown);
        assert_eq!(
            app.dash.selected, None,
            "an arrow key belongs to whatever owns the keyboard"
        );
        app.dash.search = None;

        // The ⋮ popup blocks the keymap and takes Escape, which is the one thing
        // on this screen a `settings::card_modal` does not do for itself.
        app.dash.menu_for = Some(a.clone());
        press(&mut app, &ctx, egui::Key::ArrowDown);
        assert_eq!(
            app.dash.selected, None,
            "the file menu is open, so the arrows are not the grid's"
        );
        press(&mut app, &ctx, egui::Key::Escape);
        assert_eq!(app.dash.menu_for, None, "and Escape shuts it");

        // **The *All projects ⌄* dropdown, which is the same shape and had none of
        // this** (`[S20.2-L1-01]`, §15 D578). `Escape` left it open and cleared the
        // card cursor instead; `Delete` opened the confirmation underneath it.
        //
        // ⚠️ **`filter_open` is set directly, and *before every press*.** Two
        // reasons, and both were found by the assertions below failing. It cannot be
        // raised by clicking the control, because `project_filter` returns early and
        // **clears** the flag when the library holds no project — the trap
        // `a_click_that_dismisses_a_library_menu_does_not_also_open_a_document`
        // records — and this fixture has none. And it does not survive a frame for
        // the same reason: `press` runs a whole `dashboard_ui`, whose header runs
        // *after* `dashboard_keys` and clears the flag on the way past. So one
        // assignment covers exactly one press, and a test that made three presses on
        // one assignment measured the guard once and nothing twice.
        let open_dropdown = |app: &mut OndinApp| app.dash.filter_open = true;

        app.dash.selected = Some(a.clone());
        open_dropdown(&mut app);
        press(&mut app, &ctx, egui::Key::ArrowDown);
        assert_eq!(
            app.dash.selected.as_deref(),
            Some(a.as_path()),
            "the filter dropdown is open, so the arrows are not the grid's either"
        );
        open_dropdown(&mut app);
        press(&mut app, &ctx, egui::Key::Delete);
        assert_eq!(
            app.dash.deleting, None,
            "nor may Delete open the confirmation behind it"
        );
        open_dropdown(&mut app);
        press(&mut app, &ctx, egui::Key::Escape);
        assert!(!app.dash.filter_open, "Escape shuts the dropdown");
        assert_eq!(
            app.dash.selected.as_deref(),
            Some(a.as_path()),
            "and spends itself doing so — the card cursor behind it survives, which \
             is the half that was measurably wrong rather than merely missing"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **`Ctrl+K` does not open the search from under a card that owns the
    /// keyboard** — `[S20.1-L1-02]`, §15 D542.
    ///
    /// `search_field` read the chord straight off the context with no reference
    /// to any of the eight modals `dashboard_keys` guards on, and `dashboard_ui`
    /// draws the top bar **before** the modal calls — so `Ctrl+K` under *New
    /// project* opened the overlay behind the backdrop and took the caret, and
    /// every character after it went into the query. The card stayed on screen
    /// with an empty *Name* field and nothing to explain why. Permanent rather
    /// than one frame: `search_field` re-requests focus every frame it draws,
    /// while `NewProject::name_focused` is a one-shot latch that never asks
    /// again.
    ///
    /// ⚠️ **Two of the eight are asserted, not one, and they fail differently.**
    /// Under *New project* the theft is silent; under the rename field the same
    /// chord also **closes the rename** — a key `docs/shortcuts.md` does not list
    /// as closing it — so the second arm would stay green against a fix that
    /// guarded only on `new_project`.
    ///
    /// **The unguarded control is what says the chord still works**, and without
    /// it a fix that simply deleted the chord passes everything above.
    ///
    /// ⚠️ **Flipped** by dropping `library_keys_are_free()` from `search_field`'s
    /// `chord`: fails on the first assertion, `Some("")` against `None`. The
    /// control stays green.
    ///
    /// (Plain backticks rather than `[links]`, §15 D319's convention.)
    #[test]
    fn ctrl_k_does_not_steal_the_caret_from_a_card_that_owns_the_keyboard() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "ctrlk");
        app.dash.nav = Nav::All;
        let cmd = egui::Modifiers::COMMAND;

        app.dash.new_project = Some(Default::default());
        press_with(&mut app, &ctx, egui::Key::K, cmd);
        assert_eq!(
            app.dash.search, None,
            "the New project card owns the keyboard, so Ctrl+K is not the top \
             bar's — it used to open behind the backdrop and take the caret"
        );
        assert!(
            app.dash.new_project.is_some(),
            "and the card is still up, which is what makes the theft invisible"
        );
        app.dash.new_project = None;

        // The rename field is the arm that fails *loudly* rather than silently.
        app.rename_entry = Some(Default::default());
        press_with(&mut app, &ctx, egui::Key::K, cmd);
        assert_eq!(app.dash.search, None, "nor the rename field's");
        assert!(
            app.rename_entry.is_some(),
            "and Ctrl+K does not close the rename — shortcuts.md does not list it \
             as a key that does"
        );
        app.rename_entry = None;

        // Control: with nothing on screen owning the keyboard, the chord is the
        // feature and must still open the overlay.
        press_with(&mut app, &ctx, egui::Key::K, cmd);
        assert_eq!(
            app.dash.search.as_deref(),
            Some(""),
            "control: Ctrl+K still opens the search when nothing else wants the \
             keyboard"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The search chord is `Ctrl` without Alt and without Shift** (§15 D712,
    /// `[S20.1-L1-06]`).
    ///
    /// `search_field`'s gate was `i.modifiers.command` alone, so four spellings
    /// `docs/shortcuts.md` §8's dashboard row does not list — `Ctrl+Shift+K`,
    /// `Ctrl+Alt+K`, `Ctrl+Shift+P`, `Ctrl+Alt+P` — opened the overlay. **The
    /// reason not to is already written in this file**, on `dashboard_keys`' own
    /// gate — the same `impl OndinApp`, some 2,300 lines below `search_field`:
    /// *"so `Ctrl+Alt+N` and `Ctrl+Shift+D` stay unbound
    /// here exactly as they are there, rather than becoming second doors this
    /// screen invented."* This was that second door.
    ///
    /// 🚨 **`input::resolve` does not run while the dashboard is up**, which is
    /// why §15 D710 — the same rule, applied to eight arms of `normal_mode` in
    /// the same session — could not have closed this. **A keymap rule stated for
    /// one screen holds nothing on the other**, and this file is where that has
    /// to be said twice on purpose.
    ///
    /// ⚠️ **The bare-modifier controls come first**, because a fix that simply
    /// deleted the chord passes every negative assertion below.
    ///
    /// ⚠️ **Flip:** restoring `i.modifiers.command` alone fails on the first
    /// `Ctrl+Shift+K` assertion, `Some("")` against `None`.
    ///
    /// (Plain backticks per §15 D319 — `cargo doc` builds without the `test` cfg.)
    #[test]
    fn the_search_chord_does_not_answer_to_alt_or_shift() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "searchmods");
        app.dash.nav = Nav::All;
        let cmd = egui::Modifiers::COMMAND;
        let shift = egui::Modifiers { shift: true, ..cmd };
        let alt = egui::Modifiers { alt: true, ..cmd };

        // The fixture must reach the state: both plain chords really do open it.
        for k in [egui::Key::K, egui::Key::P] {
            app.dash.search = None;
            press_with(&mut app, &ctx, k, cmd);
            assert_eq!(
                app.dash.search.as_deref(),
                Some(""),
                "control: Ctrl+{k:?} is the chord and must still work"
            );
        }

        for (mods, name) in [(shift, "Shift"), (alt, "Alt")] {
            for k in [egui::Key::K, egui::Key::P] {
                app.dash.search = None;
                press_with(&mut app, &ctx, k, mods);
                assert_eq!(
                    app.dash.search, None,
                    "Ctrl+{name}+{k:?} is not listed and must not open the search"
                );
            }
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **`Escape` under the search overlay closes the overlay and nothing else**
    /// — `[S20.1-L1-03]`, §15 D543.
    ///
    /// `search_field` runs first in the frame and its Escape branch returned
    /// before the `TextEdit` was drawn, so `dashboard_keys` — later in the *same*
    /// frame — found `search.is_some()` already `false`, found
    /// `egui_wants_keyboard_input()` false too (egui surrenders focus on `Escape`
    /// during `begin_pass`), and reached its own Escape arm, which clears
    /// `dash.selected`. One press closed the overlay **and** threw away the
    /// keyboard cursor, so the next `Enter`, `Delete`, `F2` or `Ctrl+D` did
    /// nothing and the cursor had to be re-established from the top of the grid.
    ///
    /// `docs/shortcuts.md` §8a is the spec and this was the one row the code
    /// disagreed with: *"with the search overlay up it belongs to the overlay,
    /// which closes **instead**"*.
    ///
    /// ⚠️ **This is where §15 D374's measurement stops holding**, and the entry
    /// supplies its own diagnosis. D374 keeps `dashboard_keys`' `search.is_some()`
    /// guard as *"measurably redundant"* because `request_focus` makes
    /// `egui_wants_keyboard_input` true inside the same pass — true on a
    /// **steady-state** frame, false on the **closing** one, where the field is
    /// never drawn at all. Its closing words: *"a guard resting on the draw order
    /// of two functions is not a guard."*
    ///
    /// **The bare-Escape control is the other half**: with no overlay up, Escape
    /// is still the keyboard's click on the empty body and still clears the
    /// selection. A fix that stopped `dashboard_keys` seeing Escape at all would
    /// pass the first assertion and fail this one.
    ///
    /// ⚠️ **Flipped** by putting `key_pressed` back in place of `consume_key`:
    /// fails on the surviving-selection assertion, `None` against the path. The
    /// control stays green.
    #[test]
    fn escape_closes_the_search_overlay_without_clearing_the_keyboard_cursor() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "esc");
        let mut first = ondin_core::Document::new(app.session.ids.mint());
        let a = store::file_document(&root, None, "Alpha", &mut first).unwrap();
        app.library.refresh();
        app.dash.nav = Nav::All;
        app.dash.sort = Sort::Name;

        app.dash.selected = Some(a.clone());
        app.dash.search = Some("alp".to_string());
        press(&mut app, &ctx, egui::Key::Escape);
        assert_eq!(app.dash.search, None, "Escape closes the overlay");
        assert_eq!(
            app.dash.selected.as_deref(),
            Some(a.as_path()),
            "and the keyboard cursor survives it — shortcuts.md §8a says the key \
             belongs to the overlay, which closes *instead*"
        );

        // Control: with no overlay, Escape is still the keyboard's click on the
        // empty body.
        press(&mut app, &ctx, egui::Key::Escape);
        assert_eq!(
            app.dash.selected, None,
            "control: with nothing to close, Escape still drops the selection"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `F2` and `Ctrl+R` rename and `Ctrl+D` duplicates whatever the keyboard's
    /// cursor is on — the two verbs the ⋮ menu had and the keymap did not — and
    /// **none of them is offered in the trash**.
    ///
    /// ⚠️ **The two rename keys are asserted separately on purpose.** They enter
    /// through opposite sides of the `plain` gate — `F2` bare, `Ctrl+R` as a
    /// chord — so "one of them works" says nothing about the other, and the trash
    /// refusal is asked of both because it lives in the function they share
    /// rather than at either site.
    ///
    /// ⚠️ **The trash half is the part worth the fixture.** `Enter` has been
    /// refused there since D374, through `can_open`; these two are refused
    /// through `in_trash` on the *other* side of the same predicate, so a test
    /// that only proved the happy path would say nothing about the case where
    /// renaming means renaming a file the purge is about to delete. Asserted as
    /// state rather than as an absence of ink: `rename_entry` stays `None` and the
    /// file count does not move.
    ///
    /// ⚠️ **And `Ctrl+D` is asserted by *count*, not by name.** The copy is
    /// "Alpha copy" (`store::duplicate`), but a name assertion would also pass
    /// against a duplicate that wrote itself over the original's row — counting
    /// the entries is what says a second file exists.
    ///
    /// **Four flips run, and the last is the interesting one.** Dropping the
    /// `Ctrl+R` arm fails at *"Ctrl+R opens the same field with the same text in
    /// it"*, `None` against the path — which is only worth stating because it is
    /// the arm a reader would assume `F2`'s presence already covers. Dropping
    /// `!in_trash()` from `start_rename` fails at *"renaming a trashed document
    /// renames what the purge is going to delete"*, with the field opened on
    /// `.trash\alpha.ondin` — ⚠️ **re-run against that function after the guard
    /// moved into it**, since the flip recorded here first was against the arm it
    /// used to live on, and a flip is a claim about the code as it stands rather
    /// than about the code it was written for. It fails at the `F2` assertion,
    /// the first of the pair, which is the one that reaches the shared guard
    /// first rather than the one the guard "belongs" to. Dropping it from the `Ctrl+D` arm changes **nothing**
    /// — every assertion stays green — and that is a finding about the code
    /// rather than a failed experiment: `Act::Duplicate` looks its subject up in
    /// `library.entries`, which does not contain the trash, so a trashed
    /// duplicate is already silent by way of a *different* module's scan. The
    /// guard stays because that silence is a coincidence of where the trash is
    /// kept rather than a decision about what a trashed document may do — and
    /// note the asymmetry it exposes: `F2` reaches its subject through the
    /// `entries` this function is *handed*, which in the trash **is** the trashed
    /// list. Two verbs, two different lookups, only one of which happened to be
    /// safe.
    #[test]
    fn f2_renames_and_ctrl_d_duplicates_the_keyboard_cursor_but_not_in_the_trash() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "verbs");
        let mut first = ondin_core::Document::new(app.session.ids.mint());
        let a = store::file_document(&root, None, "Alpha", &mut first).unwrap();
        app.library.refresh();
        app.dash.nav = Nav::All;
        app.dash.sort = Sort::Name;
        let cmd = egui::Modifiers::COMMAND;

        app.dash.selected = Some(a.clone());
        press(&mut app, &ctx, egui::Key::F2);
        assert_eq!(
            app.rename_entry
                .as_ref()
                .map(|(p, n, _)| (p.clone(), n.clone())),
            Some((a.clone(), "Alpha".to_string())),
            "F2 opens the field the ⋮'s Rename opens, pre-filled the same way"
        );
        assert_eq!(
            app.rename_entry.as_ref().map(|(_, _, focused)| *focused),
            Some(false),
            "and the caret latch starts down, which is the whole of D380's fix"
        );
        app.rename_entry = None;

        // **The editor's other rename**, read on the *chord* side of the keymap's
        // `plain` gate — a different arm of a different branch, which is why it
        // is asserted rather than assumed to follow from `F2` above.
        press_with(&mut app, &ctx, egui::Key::R, cmd);
        assert_eq!(
            app.rename_entry
                .as_ref()
                .map(|(p, n, _)| (p.clone(), n.clone())),
            Some((a.clone(), "Alpha".to_string())),
            "Ctrl+R opens the same field with the same text in it"
        );
        app.rename_entry = None;

        press_with(&mut app, &ctx, egui::Key::D, cmd);
        app.library.refresh();
        assert_eq!(
            app.library.entries.len(),
            2,
            "Ctrl+D left a second file behind"
        );

        // The trash: the same keys, the same selection, nothing offered.
        let trashed = store::trash(&root, &app.library.entry_at(&a).cloned().unwrap()).unwrap();
        app.library.refresh();
        app.dash.nav = Nav::Trash;
        app.dash.selected = Some(trashed.clone());
        press(&mut app, &ctx, egui::Key::F2);
        assert_eq!(
            app.rename_entry, None,
            "renaming a trashed document renames what the purge is going to delete"
        );
        press_with(&mut app, &ctx, egui::Key::R, cmd);
        assert_eq!(
            app.rename_entry, None,
            "and the other spelling of the same verb obeys the same refusal — \
             which is what `start_rename` being one function is for"
        );
        press_with(&mut app, &ctx, egui::Key::D, cmd);
        assert_eq!(
            store::trashed(&root).len(),
            1,
            "and duplicating one would be Restore without its bookkeeping"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A library whose folder has gone says so on every nav, and refuses to make
    /// a document into the hole where it used to be.
    ///
    /// ⚠️ **The nav asserted here is a *project*, which is the one that had no
    /// empty state at all.** D376 replaced it with the dashed *New file in
    /// {project}* card — right for a project the user has simply not filled, and
    /// exactly wrong here, where the card would promise a destination that is not
    /// there and the act behind it refuses. `new_file_into` returning `None` is
    /// what lets the sentence through, so this asserts the harder of the five
    /// navs and the other four follow from the same branch.
    ///
    /// ⚠️ **The `mark_opened` is the fixture and not a formality**: the flag being
    /// asserted is *"the folder could not be read **and** this machine has seen
    /// documents in it"*, so the index has to say the second half. Until §15 D807
    /// this line also had to *overwrite* the index, because `Library::open` read
    /// the developer's real one off the machine and the fixture was therefore
    /// whatever was in the cache directory — non-empty here, empty on a clean CI
    /// box, with the flag reading the difference. It is empty by construction now,
    /// which is why one `mark_opened` is the whole of it.
    ///
    /// **Two flips, both run.** Ungating `new_file_into` fails at *"the empty
    /// state names the fault"* rather than at the card assertion below it — the
    /// card takes the whole branch, so the sentence is never drawn at all and the
    /// *first* assertion is the one that catches it. Ungating
    /// `new_library_document` fails at *"Ctrl+N created nothing"*, which is the
    /// assertion this guard exists for: `store::file_document` starts with a
    /// `create_dir_all`, so without it the press **succeeds** and leaves a
    /// document in a brand-new folder wearing the missing library's name.
    #[test]
    fn a_missing_library_folder_says_so_instead_of_reading_as_empty() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "offline");
        seed(&mut app, "p-1", "Alps", Some("Alps"));
        app.dash.nav = Nav::Project("p-1".into());
        app.library.local.mark_opened("doc-1");
        // The unplugged drive. Everything above this line is a library that was
        // working a moment ago.
        std::fs::remove_dir_all(&root).unwrap();
        app.library.refresh();

        let text = galleys(&mut app, &ctx)
            .into_iter()
            .map(|(_, _, t)| t)
            .collect::<Vec<_>>();
        assert!(
            text.iter().any(|t| t.contains("isn't available")),
            "the empty state names the fault: {text:?}"
        );
        assert!(
            !text.iter().any(|t| t.contains("New file in")),
            "and the card that promises a destination is not drawn"
        );

        press_with(&mut app, &ctx, egui::Key::N, egui::Modifiers::COMMAND);
        assert_eq!(
            app.session.path, None,
            "Ctrl+N created nothing — which is the point, since `create_dir_all` \
             would have succeeded and made a second library at the same path"
        );
        assert!(!root.exists(), "and the folder was not resurrected");
        // ⚠️ **Asserted on the *screen*, not on the field.** This read
        // `app.session.status().text` and passed for two months while the
        // library screen painted no status line at all: the session's status had
        // exactly one production reader, the editor's top bar, which sits below
        // `OndinApp::ui`'s `View::Dashboard` return. So *"out loud"* was a claim
        // about a `String` nobody could see, and the twenty-two messages this
        // file writes were all in the same position. `galleys` is what makes the
        // sentence a fact about the frame.
        let text = galleys(&mut app, &ctx)
            .into_iter()
            .map(|(_, _, t)| t)
            .collect::<Vec<_>>();
        assert!(
            text.iter().any(|t| t.contains("isn't available")),
            "out loud, rather than by doing nothing: {text:?}"
        );
    }

    /// Every failure the library screen writes is **painted on the library
    /// screen**.
    ///
    /// ⚠️ **This file writes 22 status messages and none of them reached a
    /// pixel.** `EditorSession::status` is a single slot with one production
    /// reader — `OndinApp::status_text`, called from the editor's top bar, which
    /// `OndinApp::ui` returns before ever reaching while the dashboard is up. So
    /// a *Delete project* that could not read `projects.json` closed its
    /// confirmation, left the project in the sidebar and said nothing at all;
    /// a *Delete files too* that failed to trash some members removed the row
    /// and left those documents on disk carrying a project id nothing lists.
    /// The status was not even cleared, so the message surfaced later, out of
    /// context, in the editor's top bar the next time a document was opened —
    /// which is the second-worst possible time for it.
    ///
    /// **Both kinds, because they take different colours** and a fix that only
    /// wired up one would look right in whichever screenshot was taken. 🚨 **That
    /// mattered more after §15 D751 than before it**: the sentence is gone from
    /// this screen and the colour is now the *only* thing on it that separates a
    /// failure from a report.
    ///
    /// ⚠️ **The empty control is what stops the assertion being about nothing.**
    /// A bar that painted a dot every frame would satisfy a
    /// something-is-there check; asserting that a fresh session paints **no** dot
    /// is what says the mark appears because there is something to say.
    ///
    /// 🚨 **This test was rewritten by D751 and the rewrite is the interesting
    /// part.** It read the sentinel out of `galleys` — the library painted the
    /// message as text — and the message is a dot with the words on hover now,
    /// because the text was bounded by the room beside the search field and that
    /// bound is a function of the window: zero at 436pt, with nothing stopping the
    /// window being 436pt. **`galleys` can never see the new surface**, because a
    /// tooltip's ink never reaches `.shapes` at all. So the assertions split:
    /// `status_dot_color` says the screen is showing something and in which
    /// colour, and `session.status().text` says what the hover would read.
    ///
    /// ⚠️ **Asserting the text alone would be the original bug.** For two months
    /// this file wrote 22 messages into a slot the library never drew, and a test
    /// on `session.status().text` passed the whole time — which is what D426 was
    /// written to stop and what the dot assertion carries forward.
    ///
    /// ⚠️ **Flip, run — both bit, and one predicted site was wrong.** Removing the
    /// `status_dot` call from `dashboard_top_bar` fails here at the first
    /// `Some(...)` assertion (`left: None`) **and takes two other tests with it**,
    /// which is what says three separate places are watching this surface rather
    /// than one. Swapping `status_color`'s two arms was predicted to fail at the
    /// *info* assertion and fails at the **failure** one instead
    /// (`left: Some(#58_58_5A_61)`), for the dull reason that it runs first — the
    /// info assertion is `assert_ne!` and would have caught it too, one line
    /// later. **The colour pair is pinned in both directions either way**, which
    /// is the property worth having now that colour is the only thing separating
    /// the two kinds on this screen.
    #[test]
    fn a_library_failure_is_painted_on_the_library_screen() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "status");

        let runs = |app: &mut OndinApp| {
            galleys(app, &ctx)
                .into_iter()
                .map(|(_, _, t)| t)
                .collect::<Vec<_>>()
        };

        let quiet = runs(&mut app);
        assert!(
            !quiet.iter().any(|t| t.contains("SENTINEL")),
            "the control: nothing has happened yet"
        );
        assert_eq!(
            status_dot_color(&mut app, &ctx),
            None,
            "and nothing is marked on the bar either"
        );

        let fail_text = "SENTINEL-FAIL Rename failed: Access is denied.";
        app.session.fail(fail_text);
        assert_eq!(
            status_dot_color(&mut app, &ctx),
            Some(egui::Color32::from_rgb(226, 138, 138)),
            "a failure on this screen has to be visible on this screen, in the \
             colour a failure takes"
        );
        assert_eq!(
            app.session.status().text,
            fail_text,
            "and the hover reads the whole sentence rather than an ellipsis of it \
             — which is the half no probe can see, `on_hover_text` deferring an \
             Area whose galley never reaches `.shapes`"
        );

        let info_text = "SENTINEL-INFO Imported 3 documents";
        app.session.info(info_text);
        let lit = status_dot_color(&mut app, &ctx).expect("a report is marked too");
        assert_ne!(
            lit,
            egui::Color32::from_rgb(226, 138, 138),
            "and a report is not painted as a failure — on this screen the colour \
             is the only thing that separates them"
        );
        assert_eq!(app.session.status().text, info_text);

        // And the mark really is absent rather than a colourless dot when there
        // is nothing to say.
        app.session.info("");
        assert_eq!(status_dot_color(&mut app, &ctx), None);
        assert_eq!(runs(&mut app).len(), quiet.len());

        // ⚠️ **The three latched records say so through this same line**, which
        // is why the status region had to exist before the latches were worth
        // having: a `prefs.json` that could not be read leaves the app running
        // on defaults with `base_folder` unset — an empty library — and without
        // a sentence the user's obvious repair is to make a *second* library
        // beside the real one.
        app.prefs.unreadable = true;
        app.report_unreadable_records();
        assert!(
            status_dot_color(&mut app, &ctx).is_some(),
            "the bar is marked, which is what makes the sentence reachable at all"
        );
        assert!(
            app.session.status().text.contains("base folder"),
            "an unreadable record names itself, and names the field that matters: {}",
            app.session.status().text
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `Ctrl+N` files a new document in **the project in front of you**, which is
    /// two different facts on the two screens (`OndinApp::nav_project` and
    /// `OndinApp::open_document_project`).
    ///
    /// ⚠️ **The second half is the one reading would not settle.** On the
    /// dashboard the destination is the nav, which is what the sidebar button has
    /// always read; in the editor there is no nav on screen, and the door under
    /// test is the one that goes to the *open document's* project instead — with
    /// the nav deliberately left pointing somewhere else, so a version that read
    /// it would fail rather than agree by accident. Both are asserted by where the
    /// file landed on disk — `Project::dir` is a real folder — rather than by the
    /// metadata, which `file_document` writes from the same argument and would
    /// agree with itself.
    #[test]
    fn ctrl_n_files_into_the_project_in_front_of_you() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "new-doc");
        // ⚠️ **No `reload_projects` after the seed.** `seed` pushes the project
        // into memory and that call re-reads the file on disk, which has none —
        // so the fixture would be a nav pointing at a project the library cannot
        // look up, and every assertion below would be about the unfiled arm.
        seed(&mut app, "p-1", "Alps", Some("Alps"));
        app.library.refresh();
        app.dash.nav = Nav::Project("p-1".into());
        // ⚠️ **Set, because `OndinApp::headless` starts in `View::Editor`** and
        // every probe in this file draws the dashboard without changing it. No
        // production code reads it on this path — that is the point of
        // `open_document_project`'s note — but the assertion below that `Ctrl+N`
        // *opened* what it made is vacuous unless the fixture starts somewhere
        // else.
        app.view = crate::app::View::Dashboard;
        let cmd = egui::Modifiers::COMMAND;

        press_with(&mut app, &ctx, egui::Key::N, cmd);
        let made = app
            .session
            .path
            .clone()
            .expect("Ctrl+N opened what it made");
        assert_eq!(
            made.parent(),
            Some(root.join("Alps").as_path()),
            "the nav being viewed is the project it lands in"
        );
        assert_eq!(app.view, crate::app::View::Editor, "and it opens it");

        // Now from the editor, with that document open and no nav on screen. The
        // nav is deliberately left pointing somewhere else, which is the case the
        // editor's arm exists for: reading it would file into a project nothing
        // in front of the user names.
        app.dash.nav = Nav::All;
        app.dispatch(&ctx, crate::input::Action::NewDocument);
        let second = app.session.path.clone().expect("a second document");
        assert_ne!(second, made, "a new file, not the one that was open");
        assert_eq!(
            second.parent(),
            Some(root.join("Alps").as_path()),
            "the open document's project, not the nav left behind on the dashboard"
        );

        // And a session whose document is not in the library at all — the starter
        // the app opens on — files at the root rather than guessing.
        app.session.path = None;
        app.dispatch(&ctx, crate::input::Action::NewDocument);
        assert_eq!(
            app.session.path.as_deref().and_then(|p| p.parent()),
            Some(root.as_path()),
            "unfiled, which is what None means everywhere else on this screen"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A sync client's copy wears the chip, in both views, and the document it
    /// copied does not (§15 D375).
    ///
    /// ⚠️ **Two cards, one name — which is why the assertion counts chips rather
    /// than looking one up.** The conflict file is a byte copy, so it carries the
    /// original's metadata and both cards paint the galley "Landing v4"; a probe
    /// that searched for a run near a *name* could not tell them apart, and that
    /// is the exact confusion the chip exists to end. What is assertable is that
    /// exactly one of the two is marked.
    ///
    /// **Flip-checked** by dropping the `conflict` branch from `file_card`: the
    /// grid assertion fails at 0 against 1. The list assertion is not redundant
    /// with it — the two draw the chip from different call sites and only the list
    /// takes its width out of a column first.
    #[test]
    fn a_conflict_copy_is_the_one_of_the_two_that_is_marked() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "conflict");
        let mut doc = ondin_core::Document::new(app.session.ids.mint());
        let original = store::file_document(&root, None, "Landing v4", &mut doc).unwrap();
        std::fs::copy(&original, root.join("landing-v4 (1).ondin")).unwrap();
        app.library.refresh();
        app.dash.nav = Nav::All;

        let chips = |app: &mut OndinApp, ctx: &egui::Context| {
            galleys(app, ctx)
                .into_iter()
                .filter(|(_, _, t)| t == Mark::Conflict.label())
                .count()
        };
        let names = |app: &mut OndinApp, ctx: &egui::Context| {
            galleys(app, ctx)
                .into_iter()
                .filter(|(_, _, t)| t == "Landing v4")
                .count()
        };
        assert_eq!(
            names(&mut app, &ctx),
            2,
            "the fixture is two cards the list cannot otherwise tell apart"
        );
        assert_eq!(chips(&mut app, &ctx), 1, "exactly one of them is the copy");
        app.dash.list_view = true;
        assert_eq!(chips(&mut app, &ctx), 1, "and the list marks it too");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A document this build cannot open does not look like one it can**
    /// (§15 D613, `[S1.3-L3-05]`).
    ///
    /// 🚨 **The mark was computed on every scan and drawn nowhere.**
    /// `scan::Entry::unread` had no reader outside `library/` — this file, 7,600
    /// lines and the only consumer of `Entry`, did not contain the string once —
    /// so a broken `.ondin` got a card identical in every pixel to a healthy one,
    /// and the only feedback was the status line after the user had already
    /// clicked *Open*. `scan.rs`'s test asserted `found[0].unread` and nothing
    /// downstream, which is this project's "asserts a consequence no user can
    /// observe" shape; `Entry` derives `PartialEq`, so `dead_code` was silent.
    ///
    /// **The fixture is a document written by a newer build**, which is the case
    /// the flag itself cannot reach: it is well-formed JSON, so `MetaProbe`
    /// answers `Found` and `read_meta` never opens the rest of the file, and only
    /// `io::load` — which the cover render runs anyway — refuses it. A synced
    /// library is exactly where one turns up. So this asserts the *drawing* and
    /// `cover.rs`'s
    /// `a_document_the_loader_refuses_is_unreadable_where_an_empty_one_is_merely_blank`
    /// asserts the *fact*, and neither is the other.
    ///
    /// ⚠️ **The healthy document in the same library is the control**, and it is
    /// not decoration: `mark_of` reads two sources, and a version that marked
    /// every card would satisfy a one-card assertion perfectly.
    ///
    /// 🚨 **A pass and then a `settle` before the chip is asserted, and the
    /// difference between those two is what this test learned the hard way**
    /// (§15 D820). `Covers::get` only learns a document is unloadable when it
    /// **renders** it, so the first pass queues the work and the mark comes after.
    /// While the render was inline, *"the next pass"* was a fact; with it on a
    /// thread, *"the next pass"* is a **race** — this failed 2 runs in 20 of its
    /// own filter, after six clean ones, which is `CLAUDE.md`'s §14 point and
    /// §15 D796's lesson arriving from a second direction. `get` requests a
    /// repaint, so in the app late still arrives on its own; a probe waits.
    ///
    /// ⚠️ **This paragraph said the two passes were *"the cover cache's laziness
    /// rather than a flake"*.** That was true when it was written and is the
    /// sentence most worth not leaving standing: the laziness is still there and
    /// the *timing* it promised is gone.
    ///
    /// **Flip-check, run**: dropping `covers.unreadable` from `mark_of` — leaving
    /// the `unread` flag alone, which is what shipped — fails at 0 against 1. The
    /// predicted site, for once, and the `names` control stays at 2 under it,
    /// which is what says the card is still being drawn and only its mark is gone.
    #[test]
    fn a_document_this_build_cannot_open_wears_the_mark_and_a_healthy_one_does_not() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "unreadable");
        let mut healthy = ondin_core::Document::new(app.session.ids.mint());
        store::file_document(&root, None, "Landing v4", &mut healthy).unwrap();
        let mut broken = ondin_core::Document::new(app.session.ids.mint());
        let path = store::file_document(&root, None, "From tomorrow", &mut broken).unwrap();

        // Well-formed JSON this loader refuses — one number, and the shape a
        // future build's file has.
        let text = std::fs::read_to_string(&path).unwrap();
        let at = text.find("\"schema_version\":").unwrap();
        let end = at + text[at..].find(',').unwrap();
        std::fs::write(
            &path,
            format!("{}\"schema_version\":9999{}", &text[..at], &text[end..]),
        )
        .unwrap();

        app.library.refresh();
        app.dash.nav = Nav::All;
        let chips = |app: &mut OndinApp, ctx: &egui::Context| {
            galleys(app, ctx)
                .into_iter()
                .filter(|(_, _, t)| t == Mark::Unreadable.label())
                .count()
        };
        assert_eq!(
            galleys(&mut app, &ctx)
                .into_iter()
                .filter(|(_, _, t)| t == "Landing v4" || t == "From tomorrow")
                .count(),
            2,
            "both documents are listed — hiding the broken one is the other bug"
        );
        // 🚨 **One pass queues the render; `settle` is what waits for it**
        // (§15 D820). This was `let _ = chips(…)` — one extra pass — which was
        // exactly right while `Covers::get` rendered inline, and became a **race**
        // the day the render moved to a thread: whether the answer had arrived by
        // the next pass depended on how loaded the machine was. Measured at 2
        // failures in 20 paired runs of this filter, and **green in six** before
        // that, which is D796's lesson arriving a second time — a filtered run
        // packs the related tests onto every core at once, and a handful of clean
        // ones is not evidence.
        let _ = chips(&mut app, &ctx);
        app.covers.settle(&ctx);
        assert_eq!(chips(&mut app, &ctx), 1, "exactly the broken one is marked");
        app.dash.list_view = true;
        assert_eq!(chips(&mut app, &ctx), 1, "and the list marks it too");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **All four of `remember_search`'s decisions, and the rows they feed**
    /// (§15 D619, `[S20.1-L6-05]`).
    ///
    /// 🚨 **A persisted, disk-writing feature with zero assertions anywhere in the
    /// workspace.** `grep -rn "recent_searches\|remember_search" crates/` returned
    /// six hits, none in a test. All four decisions live in five lines: the
    /// refusal of a blank query, the `retain` that dedupes and promotes to the
    /// front, the cap of `SEARCH_RECENTS` (plain backticks — a `#[cfg(test)]`
    /// module, §15 D319), and the save. Delete the `retain` and
    /// the list fills with repeats of one query; change the cap to 50 and nothing
    /// goes red.
    ///
    /// ⚠️ **It was *executed* and unasserted, which is the distinction that makes
    /// this invisible to a coverage number** — the search-arrows test runs it
    /// through the `nav_to` arm and looks at none of it. `[A5-L6-02]` draws the
    /// same line for `grid.rs`.
    ///
    /// **And the display half had never been laid out**, because `recents` is
    /// non-empty only when the needle is: the one test that opens the search with
    /// an empty query has an empty `recent_searches`, so `search_results` returns
    /// before drawing anything. The rows here are the first time that branch has
    /// been on screen.
    ///
    /// ⚠️ **`save()` used to write to the user's real cache directory** —
    /// `dirs::cache_dir()`, no injection point — which is why this asserts
    /// persistence through `save_to`/`load_from` against a temp path. **It was
    /// this test's own first run that found the defect**, on a fixture already
    /// holding the strings below from a previous run, and the arrows test had been
    /// writing there for longer. Closed by §15 D807: `library::cache`'s `load` and
    /// `save` are both unreachable under `cfg(test)`, so the fixture starts empty
    /// by construction. The temp-path round trip stays, because it is the only
    /// thing that asserts what survives a restart.
    ///
    /// **Flip-check, run, four of them, all bite — and two land earlier than
    /// predicted:**
    /// - dropping the `retain` fails the *promote* assertion, the predicted site,
    ///   with `["alpha", "gamma", "beta", "alpha"]`;
    /// - `truncate(50)` fails the *cap* assertion with a sixth entry, `"beta"`,
    ///   still on the end — the predicted site;
    /// - dropping the empty-query guard fails the *blank* assertion, predicted;
    /// - `insert(0, ..)` → `push` was predicted to fail the promote assertion and
    ///   fails **"newest first"** two assertions earlier, with the list simply
    ///   reversed. ⚠️ *An order flip bites at the first assertion that reads an
    ///   order, which is rarely the one the claim is filed under.*
    #[test]
    fn a_remembered_search_is_deduped_promoted_capped_and_shown() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "recents");

        let list = |app: &OndinApp| app.library.local.recent_searches.clone();

        // ⚠️ **Asserted rather than cleared, which is what §15 D807 bought.**
        // `app()` redirects the *base folder* and could not redirect the local
        // index, which `Library::open` loaded from `dirs::cache_dir()` — so this
        // fixture used to start with whatever the person running the suite last
        // searched for, and the first run of this test found exactly that. Now the
        // index is unreachable from a test, so the empty list is a fact about the
        // fixture and worth demanding instead of arranging.
        assert!(
            list(&app).is_empty(),
            "a headless app's index is empty by construction, not by being cleared"
        );

        // ⚠️ **Blank first, so the guard is asserted against an empty list rather
        // than against a list it could not have changed anyway.**
        app.remember_search("   ");
        assert!(list(&app).is_empty(), "a blank query is not a search");

        for q in ["alpha", "beta", "gamma"] {
            app.remember_search(q);
        }
        assert_eq!(list(&app), vec!["gamma", "beta", "alpha"], "newest first");

        // Searching something already there promotes it rather than repeating it.
        app.remember_search("alpha");
        assert_eq!(
            list(&app),
            vec!["alpha", "gamma", "beta"],
            "an old query moves to the front and is not duplicated"
        );

        // The cap, asserted at its own boundary: five in, a sixth pushes one off.
        for q in ["d", "e", "f"] {
            app.remember_search(q);
        }
        assert_eq!(
            list(&app),
            vec!["f", "e", "d", "alpha", "gamma"],
            "five, and the sixth-oldest is gone"
        );

        // The persistence, through the explicit path rather than the global one.
        let at = root.join("library.json");
        app.library.local.save_to(&at);
        let back = crate::library::cache::LocalIndex::load_from(&at);
        assert_eq!(
            back.recent_searches,
            list(&app),
            "the list is what survives a restart, which is the whole point of it"
        );

        // And the rows reach the screen — the branch nothing had drawn.
        app.dash.search = Some(String::new());
        let shown = galleys(&mut app, &ctx);
        for q in ["f", "e", "d", "alpha", "gamma"] {
            assert!(
                shown.iter().any(|(_, _, t)| t == q),
                "the recents row {q:?} was not drawn: {:?}",
                shown.iter().map(|(_, _, t)| t).collect::<Vec<_>>()
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The delete card tells the truth about which delete it is** (§15 D618,
    /// `[S20.3-L6-04]`).
    ///
    /// 🚨 **`dash.deleting` was never assigned in any test in the workspace**, so
    /// this card had never been on screen. `delete_modal`'s own ⚠️ calls the
    /// sentence it chooses between *"the worst sentence on the screen"*, and the
    /// review flipped `let purge = self.dash.nav == Nav::Trash` to `!=` — making
    /// every *Delete file* card in the library read *"is deleted for good. This
    /// cannot be undone"* and every card in the Trash read *"moves to Trash"* —
    /// with the whole `ondin-app` suite green.
    ///
    /// **Both navs, because the claim is which sentence goes with which**, and one
    /// of the two is satisfied by a build that shows that sentence everywhere.
    /// Asserted on the painted galleys — this is a question about what the user
    /// reads, and the title is checked with it because the title and the body have
    /// to agree or the card contradicts itself.
    ///
    /// ⚠️ **`nav` is set directly rather than reached by clicking.** The route in
    /// is `dashboard_keys`' Delete, which is `[S20.3-L1-02]`'s territory and is
    /// still an open ruling; what this pins is the *card*, which is where the
    /// sentence lives and where the flip was.
    ///
    /// **Flip-check, run**: `==` → `!=` fails on the first assertion, at the
    /// library nav, with the trash's own sentence painted — the predicted site.
    #[test]
    fn the_delete_card_says_for_good_only_where_it_means_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "delete-wording");
        let mut doc = ondin_core::Document::new(app.session.ids.mint());
        let path = store::file_document(&root, None, "Landing v4", &mut doc).unwrap();
        app.library.refresh();

        let said = |app: &mut OndinApp, ctx: &egui::Context, want: &str| {
            galleys(app, ctx)
                .into_iter()
                .any(|(_, _, t)| t.contains(want))
        };

        app.dash.nav = Nav::All;
        app.dash.deleting = Some(path.clone());
        assert!(
            said(&mut app, &ctx, "moves to Trash"),
            "a delete from the library is a move to the Trash and must say so"
        );
        assert!(
            !said(&mut app, &ctx, "cannot be undone"),
            "and must not borrow the permanent card's sentence"
        );
        assert!(said(&mut app, &ctx, "Delete file"), "nor its title");

        app.dash.nav = Nav::Trash;
        app.dash.deleting = Some(path);
        assert!(
            said(&mut app, &ctx, "cannot be undone"),
            "a delete from the Trash is permanent and must say so"
        );
        assert!(
            said(&mut app, &ctx, "Delete permanently"),
            "and the title has to agree with the body"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The *Delete its N file(s) too* switch reads the way it acts** (§15 D618,
    /// `[S20.3-L6-04]`).
    ///
    /// 🚨 `dash.deleting_project` was never assigned in any test either, and this
    /// is the one destructive card on the screen **with no trash behind it** — the
    /// project is gone for good. The switch reads the *inverse* of the field it
    /// writes (`!dp.keep_files`), which is exactly the shape a refactor gets
    /// backwards, and the review flipped it to `dp.keep_files` — drawing it *off*
    /// when it will delete and *on* when it will not — with the suite green.
    ///
    /// **Asserted through the ink rather than the field**, because the field was
    /// never in doubt: `ui::switch_row_ink` is the label's colour and it is what a
    /// user reads the state off, so this is the same question the eye asks. The
    /// default is `keep_files: true`, so the switch starts **off** and the files
    /// are kept unless the user says otherwise — which is the safe default and is
    /// the half a flipped read makes look like the dangerous one.
    ///
    /// ⚠️ **The count in the label is asserted too.** *"Delete its 0 file(s) too"*
    /// is a row the card does not draw at all (`if count > 0`), so a fixture with
    /// no documents in the project would assert nothing and read as passing.
    ///
    /// **Flip-check, run**: `!dp.keep_files` → `dp.keep_files` fails on the muted
    /// assertion at `#CCCCD0E0` against `#909092 9E` — the label paints in
    /// `STRONG` for a switch that is about to *keep* the files. The predicted
    /// site. ⚠️ Note the alphas differ too (`E0` against `9E`): both are the
    /// settled fade, and the alpha is part of the theme's two inks rather than an
    /// artefact — which is what says thirty passes was enough.
    #[test]
    fn the_delete_project_switch_is_off_while_the_files_are_being_kept() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "delete-project-switch");
        app.dash.new_project = Some(NewProject {
            name: "Marketing".into(),
            folder: false,
            ..Default::default()
        });
        app.create_project();
        let project = app.library.projects.projects[0].clone();
        let id = project.id.clone();
        let mut doc = ondin_core::Document::new(app.session.ids.mint());
        store::file_document(&root, Some(&project), "Landing v4", &mut doc).unwrap();
        app.library.refresh();

        app.dash.deleting_project = Some(DeleteProject {
            id: id.clone(),
            keep_files: true,
            transfer_to: None,
        });
        let row = |app: &mut OndinApp, ctx: &egui::Context| {
            galleys(app, ctx)
                .into_iter()
                .find(|(_, _, t)| t.starts_with("Delete its"))
        };
        let (_, _, label) = row(&mut app, &ctx).expect("the switch row is on the card");
        assert_eq!(
            label, "Delete its 1 file(s) too",
            "the row counts the project's own documents"
        );

        // The label's colour is the switch's state, which is what the eye reads.
        let ink = |app: &mut OndinApp, ctx: &egui::Context| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
                ..Default::default()
            };
            let mut found = None;
            // ⚠️ **Thirty passes, not four, and it is not superstition.** A modal
            // is an `Area` and an `Area` **fades in**, so every colour it paints
            // is gamma-multiplied by however far through the fade the pass is —
            // read at four passes this label comes back `#7C7C7D88` against a
            // `MUTED` of `#909092 9E`, a uniform 0.86 of it. `galleys` above can
            // afford four because a string does not fade; a *colour* has to be
            // read after the animation has settled.
            for _ in 0..30 {
                let full = ctx.run_ui(input.clone(), |ui| app.dashboard_ui(ui));
                found = full.shapes.iter().find_map(|cs| match &cs.shape {
                    egui::Shape::Text(t) if t.galley.text().starts_with("Delete its") => {
                        Some(t.fallback_color)
                    }
                    _ => None,
                });
            }
            found.expect("the switch row is painted")
        };
        assert_eq!(
            ink(&mut app, &ctx),
            crate::ui::switch_row_ink(false),
            "keep_files is the default, so the switch is off and the label is muted"
        );

        if let Some(dp) = app.dash.deleting_project.as_mut() {
            dp.keep_files = false;
        }
        assert_eq!(
            ink(&mut app, &ctx),
            crate::ui::switch_row_ink(true),
            "and turning it on is what says the files go with the project"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Stepping down past the bottom of the viewport brings the card with it.
    ///
    /// ⚠️ **This is the assertion that makes the arrow keys a feature rather than
    /// a state change.** Without `OndinApp::follow_selection` the selection
    /// moves perfectly and goes off screen on the fifth press, which reads as the
    /// key having stopped working. Flip-checked by deleting the call from
    /// `file_card`: the label comes back at **y=970** against a window 820 tall,
    /// a row and a half below the fold rather than marginally under it. **Twenty
    /// documents is the fixture and not a round number**: the rows are 172 apart
    /// (a 158 card and a 14 gap), so a sixteen-document version would have put the
    /// fourth row's caption at y≈798 against a fold at 800 — two points inside,
    /// which is a test that passes with no scrolling at all.
    #[test]
    fn arrowing_off_the_bottom_of_the_grid_scrolls_the_card_into_view() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "scroll");
        for i in 1..=20 {
            let mut doc = ondin_core::Document::new(app.session.ids.mint());
            store::file_document(&root, None, &format!("Doc {i:02}"), &mut doc).unwrap();
        }
        app.library.refresh();
        app.dash.nav = Nav::All;
        app.dash.sort = Sort::Name;
        // Five rows of four, which is taller than the body — the fixture only
        // means anything if the last row starts below the fold.
        assert_eq!(app.visible_entries().len(), 20);

        for _ in 0..5 {
            press(&mut app, &ctx, egui::Key::ArrowDown);
        }
        let runs = galleys(&mut app, &ctx);
        assert_eq!(
            app.dash
                .selected
                .as_ref()
                .and_then(|p| p.file_stem())
                .and_then(|s| s.to_str()),
            Some("doc-17"),
            "five presses is four rows down from the first card"
        );
        // The body's own bottom, not the window's: a card that ends up in the
        // twenty points of margin under the scroll area is still not in view.
        let (_, y) = run_at(&runs, "Doc 17");
        assert!(
            y < SCREEN.y - 20.0,
            "the fifth row scrolled into view rather than staying below the fold \
             (the card's name is at y={y:.0})"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `Enter` will not open a document in the trash, which is the rule the ⋮ menu
    /// has stated since the trash was built and the double click quietly broke.
    ///
    /// ⚠️ **The double click is asserted by reading, not by this test.** Driving
    /// one through `RawInput` means two press/release pairs inside egui's
    /// double-click window, which is wall-clock rather than frame-counted — so what
    /// pins the click is that it calls the same `OndinApp::can_open` one line
    /// away in `OndinApp::pick_or_open`. Flip-checked on the key: making
    /// `can_open` return `true` unconditionally fails the second assertion with the
    /// trashed path.
    #[test]
    fn nothing_opens_a_document_out_of_the_trash() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "trashopen");
        let mut doc = ondin_core::Document::new(app.session.ids.mint());
        let path = store::file_document(&root, None, "Alpha", &mut doc).unwrap();
        app.library.refresh();
        app.trash_or_purge(&path);
        app.dash.nav = Nav::Trash;
        let trashed = app.visible_entries();
        assert_eq!(trashed.len(), 1, "the fixture is a document in the trash");

        press(&mut app, &ctx, egui::Key::ArrowDown);
        assert_eq!(
            app.dash.selected.as_deref(),
            Some(trashed[0].path.as_path()),
            "a trashed document can still be picked out — Delete and Restore need it"
        );
        press(&mut app, &ctx, egui::Key::Enter);
        assert_eq!(
            app.session.path, None,
            "but Enter does not put the editor on a file inside .trash"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The centre of the ⋮ menu row whose painted label is exactly `label`.
    ///
    /// Exact rather than `contains`, because *Delete* and *Delete permanently* are
    /// two rows of two different menus and one is a prefix of the other.
    fn menu_row_centre(app: &mut OndinApp, ctx: &egui::Context, label: &str) -> egui::Pos2 {
        let painted = galleys(app, ctx);
        let Some((pos, size, _)) = painted.iter().find(|(_, _, t)| t == label) else {
            let all: Vec<&str> = painted.iter().map(|(_, _, t)| t.as_str()).collect();
            panic!("no menu row {label:?} on screen; painted: {all:?}");
        };
        egui::pos2(pos.x + size.x / 2.0, pos.y + size.y / 2.0)
    }

    /// 🚨 ***Delete* from the ⋮ menu raises the confirmation card; it does not
    /// trash the document** (§15 D606, `[S20.2-L6-04]`).
    ///
    /// **This is the regression test for the worst of three simultaneous flips
    /// that left 975/975 green.** `Act::Trash(path) if !trashed` inverted to
    /// `if trashed` makes *Delete* — outside the trash — fall through to
    /// `other => *act = Some(other)`, and `trash_or_purge` then trashes the
    /// document **with no confirmation card at all**. Nothing in the workspace went
    /// red: `file_menu_popup` and `file_menu_button` had **zero test callers**, and
    /// the string `file_menu_popup` did not occur once in this file's 2,300-line
    /// test module.
    ///
    /// ⚠️ **The suite reached this file the whole time and simply never entered
    /// this function.** The finding's control — `can_open` reduced to `true` —
    /// failed `nothing_opens_a_document_out_of_the_trash` above at the assertion
    /// that test's own doc predicts. So the harness had teeth; the door was the
    /// gap.
    ///
    /// **Driven through the painted row**, which is what makes the four rules here
    /// testable at all: `galleys` finds the label, `click_at` takes its centre. Six
    /// lines on helpers that were already in this module.
    ///
    /// Flip-check, run: `if !trashed` → `if trashed` fails at the `deleting`
    /// assertion — `None` where the card was expected — and the entry-count
    /// assertion below it goes with it, the document having been trashed outright.
    /// **Both halves are asserted because the card and the act are different
    /// failures**: a version that raised the card *and* trashed would pass the
    /// first alone.
    #[test]
    fn delete_from_the_file_menu_raises_the_card_rather_than_trashing() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "filemenu-del");
        let mut doc = ondin_core::Document::new(app.session.ids.mint());
        let path = store::file_document(&root, None, "Alpha", &mut doc).unwrap();
        app.library.refresh();
        app.dash.nav = Nav::All;
        assert_eq!(
            app.visible_entries().len(),
            1,
            "the fixture is one document"
        );

        app.dash.menu_for = Some(path.clone());
        let row = menu_row_centre(&mut app, &ctx, "Delete");
        click_at(&mut app, &ctx, row);

        assert_eq!(
            app.dash.deleting.as_deref(),
            Some(path.as_path()),
            "the row opens the confirmation rather than becoming an act"
        );
        app.library.refresh();
        app.dash.nav = Nav::All;
        assert_eq!(
            app.visible_entries().len(),
            1,
            "and nothing was trashed on the way"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The other three rules the ⋮ menu encodes, none of which was asserted
    /// anywhere (§15 D606, `[S20.2-L6-04]`).
    ///
    /// - **A trashed document gets two rows and neither is *Open*.** Opening one
    ///   would put the editor on a file inside `.trash`, which the next autosave
    ///   keeps writing to and the purge eventually deletes out from under the
    ///   person editing it — the function's own doc says so.
    /// - **The *Star*/*Unstar* label follows `is_starred`.** Inverting it was the
    ///   second of the three green flips.
    /// - ***Move to project* opens the sheet** rather than becoming an act.
    ///
    /// 🚨 **The first draft of this test could not see the *Open* flip, and the
    /// flip is what said so.** Deleting the `("Open", …)` row from the non-trash
    /// arm left both of this session's new tests **green**: the trash assertion is
    /// *"the trash menu must not offer Open"*, and a build that offers it nowhere
    /// satisfies that for free. **An absence is only evidence beside the presence it
    /// is an absence of** — so the ordinary menu's six rows are asserted present
    /// first, and that block is the whole reason the eight below mean anything.
    ///
    /// Flip-check, run, and **both predicted sites were wrong**:
    ///
    /// - Deleting the `("Open", …)` row now fails at *"the ordinary menu offers
    ///   Open"* — the control, not the trash list, which was the prediction.
    /// - Inverting `if starred` fails at the **`menu_row_centre("Star")` lookup**,
    ///   not at the *Unstar* assertion twenty lines below it: an unstarred document
    ///   now paints *Unstar*, so the row the test reaches for is not on screen and
    ///   it dies at the fixture step. The panic prints the whole frame's text, which
    ///   is how it reads as a finding rather than a helper failing.
    #[test]
    fn the_file_menu_reads_the_trash_the_star_and_the_move_sheet() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "filemenu-rows");
        let mut doc = ondin_core::Document::new(app.session.ids.mint());
        let path = store::file_document(&root, None, "Alpha", &mut doc).unwrap();
        app.library.refresh();
        app.dash.nav = Nav::All;

        // The star label, both ways round.
        app.dash.menu_for = Some(path.clone());
        let star = menu_row_centre(&mut app, &ctx, "Star");
        click_at(&mut app, &ctx, star);
        let entry = app.visible_entries()[0].clone();
        assert!(app.is_starred(&entry), "the row starred it");
        app.dash.menu_for = Some(path.clone());
        let _ = menu_row_centre(&mut app, &ctx, "Unstar");

        // *Move to project* opens the sheet instead of acting.
        app.dash.menu_for = Some(path.clone());
        let move_to = menu_row_centre(&mut app, &ctx, "Move to project");
        click_at(&mut app, &ctx, move_to);
        assert_eq!(
            app.dash.moving.as_deref(),
            Some(path.as_path()),
            "the row opens the sheet rather than moving anything"
        );
        app.dash.moving = None;

        // **All six of the ordinary menu's rows, before the trash's two** — and this is
        // the control the absence assertions below are worthless without, which a
        // flip found rather than reading did. Deleting the `("Open", …)` row from
        // the non-trash arm left both of these tests **green**: "the trash menu
        // must not offer Open" is satisfied for free by a build that offers it
        // nowhere. *An absence is only evidence beside the presence it is an
        // absence of.*
        app.dash.menu_for = Some(path.clone());
        let ordinary: Vec<String> = galleys(&mut app, &ctx)
            .into_iter()
            .map(|(_, _, t)| t)
            .collect();
        // **`Unstar` rather than `Star`, because the click above starred it** —
        // and listing the star row here rather than leaving it to that click is
        // `arch-scribe`'s correction: the block said "six rows" and named five,
        // with the sixth pinned only by a `menu_row_centre` *lookup* whose job was
        // the toggle. A row that is only reached as a fixture step is not a row
        // this list has asserted.
        for row in [
            "Open",
            "Rename",
            "Duplicate",
            "Unstar",
            "Move to project",
            "Delete",
        ] {
            assert!(
                ordinary.iter().any(|t| t == row),
                "the ordinary menu offers {row}: {ordinary:?}"
            );
        }

        // The trash's two rows, and the four the ordinary menu has that it does not.
        app.trash_or_purge(&path);
        app.library.refresh();
        app.dash.nav = Nav::Trash;
        let trashed = app.visible_entries();
        assert_eq!(trashed.len(), 1, "the fixture is a document in the trash");
        app.dash.menu_for = Some(trashed[0].path.clone());
        let painted: Vec<String> = galleys(&mut app, &ctx)
            .into_iter()
            .map(|(_, _, t)| t)
            .collect();
        for present in ["Restore", "Delete permanently"] {
            assert!(
                painted.iter().any(|t| t == present),
                "the trash menu offers {present}: {painted:?}"
            );
        }
        for absent in ["Open", "Rename", "Duplicate", "Star", "Unstar", "Delete"] {
            assert!(
                !painted.iter().any(|t| t == absent),
                "the trash menu must not offer {absent}: {painted:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A project's grid ends in the dashed *New file* card, it sits in the next
    /// cell of the last row rather than on a row of its own, and it creates a
    /// document filed in that project. No other nav has one.
    ///
    /// ⚠️ **Both assertions bite, and the prediction that only one would was
    /// wrong** — the flip is `file_grid` back to `entries.chunks(GRID_COLS)` plus
    /// a trailing card, which is the shape this was written against. The x fails
    /// unambiguously (the label lands at 359, centred in the *first* column,
    /// against 298 for the file's name beside it). The y was expected to pass
    /// vacuously and does not: the labels come out 138pt apart, over a threshold
    /// of `CARD_THUMB_H` at 116. **The margin is 22pt, and that is the finding**
    /// — had the threshold been the card's full height, 158, which is the number a
    /// reader reaches for first when writing "the same row", the assertion would
    /// have been green for exactly the layout it rules out. It is the thumbnail's
    /// height that gives it teeth, and only by accident, so the x is the one to
    /// keep if these ever have to be cut to one.
    #[test]
    fn a_project_grid_ends_in_a_new_file_card_and_nothing_else_does() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "newfile");
        app.dash.new_project = Some(NewProject {
            name: "Kestrel".into(),
            folder: false,
            ..Default::default()
        });
        app.create_project();
        let id = app.library.projects.projects[0].id.clone();
        let project = app.library.projects.projects[0].clone();
        let mut doc = ondin_core::Document::new(app.session.ids.mint());
        store::file_document(&root, Some(&project), "Alpha", &mut doc).unwrap();
        app.library.refresh();

        app.dash.nav = Nav::All;
        let runs = galleys(&mut app, &ctx);
        assert!(
            !in_body(&runs, "New file") && !in_body(&runs, "New file in Kestrel"),
            "*All files* has no project to put a new document in, so it offers none"
        );

        app.dash.nav = Nav::Project(id.clone());
        let runs = galleys(&mut app, &ctx);
        let (name_x, name_y) = run_at(&runs, "Alpha");
        let (card_x, card_y) = run_at(&runs, "New file in Kestrel");
        assert!(
            (card_y - name_y).abs() < CARD_THUMB_H,
            "and it is on that row rather than under it ({name_y:.0} vs {card_y:.0})"
        );
        assert!(
            card_x > name_x + 200.0,
            "the card finishes the row the one file started: the file's name is at \
             {name_x:.0} and the card's label at {card_x:.0}"
        );

        // The list says only *New file* — the heading above it has already named
        // the project.
        app.dash.list_view = true;
        let runs = galleys(&mut app, &ctx);
        assert!(in_body(&runs, "New file"), "the list has the row");
        app.dash.list_view = false;

        let at = card_centre(&mut app, &ctx, "New file in Kestrel");
        click_at(&mut app, &ctx, at);
        app.library.refresh();
        let filed: Vec<String> = app
            .library
            .entries
            .iter()
            .filter(|e| e.meta.project.as_deref() == Some(id.as_str()))
            .map(|e| e.display_name())
            .collect();
        assert_eq!(
            filed.len(),
            2,
            "the card filed a second document into the project: {filed:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An empty project shows the card instead of the empty state, and a project
    /// whose id no longer resolves shows the empty state instead of a card
    /// promising a destination that is not there.
    #[test]
    fn an_empty_project_offers_the_card_and_a_dangling_one_does_not() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "emptyproject");
        app.dash.new_project = Some(NewProject {
            name: "Kestrel".into(),
            folder: false,
            ..Default::default()
        });
        app.create_project();
        let id = app.library.projects.projects[0].id.clone();

        app.dash.nav = Nav::Project(id);
        let runs = galleys(&mut app, &ctx);
        assert!(
            runs.iter().any(|(_, _, t)| t == "New file in Kestrel"),
            "an empty project is a place to make the first file, not a dead end"
        );
        assert!(
            !runs.iter().any(|(_, _, t)| t == "No files here yet."),
            "so the library's empty state does not also appear"
        );

        // ⚠️ A nav holding an id `projects.json` no longer has. Everything else on
        // the screen merely filters by it and shows nothing; the card would have
        // named a project and then filed the document nowhere.
        app.dash.nav = Nav::Project("0f9c2b7a4e".into());
        let runs = galleys(&mut app, &ctx);
        assert!(
            !in_body(&runs, "New file"),
            "a project that cannot be looked up is not a destination"
        );
        assert!(
            runs.iter().any(|(_, _, t)| t == "No files here yet."),
            "and the empty state is what is left"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The mosaic reproduces the four layouts `design/Dashboard.dc.html` draws by
    /// hand, from one rule rather than a table.
    ///
    /// ⚠️ **The full-height assertions are the ones with teeth.** Every cell's
    /// *column* falls out of the arithmetic and would be right under several wrong
    /// rules; what distinguishes this layout from a plain grid is which cells run
    /// the whole height — the first always, and any column that ends up holding one.
    ///
    /// Flip-check, run: dropping the `2 * col >= n` arm (so only cell 0 is ever
    /// full height) leaves the one- and three-file cases green and fails at two
    /// files, where the second cell comes back half height — which is the design's
    /// `harbor`, the layout that arm exists for.
    #[test]
    fn a_project_mosaic_lays_out_the_designs_four_shapes_from_one_rule() {
        // 200 × 118 with a 7pt gutter, so a two-column split is 96.5 and a
        // three-column one is 62.
        let box_ = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 118.0));
        let half = (118.0 - MOSAIC_GAP) / 2.0;
        let shape = |n: usize| {
            mosaic_cells(n, box_)
                .into_iter()
                .map(|c| (c.left(), c.top(), c.width(), c.height()))
                .collect::<Vec<_>>()
        };

        assert_eq!(shape(0), [], "an empty project draws no mosaic at all");
        assert_eq!(
            shape(1),
            [(0.0, 0.0, 200.0, 118.0)],
            "one file fills the box — the design's `kestrel`"
        );
        assert_eq!(
            shape(2),
            [(0.0, 0.0, 96.5, 118.0), (103.5, 0.0, 96.5, 118.0)],
            "two are two tall columns, not one tall and one half — `harbor`"
        );
        assert_eq!(
            shape(3),
            [
                (0.0, 0.0, 96.5, 118.0),
                (103.5, 0.0, 96.5, half),
                (103.5, half + MOSAIC_GAP, 96.5, half),
            ],
            "three is one tall and two stacked — `atlas`"
        );
        let five = shape(5);
        assert_eq!(five.len(), 5);
        assert_eq!(five[0], (0.0, 0.0, 62.0, 118.0), "`field`'s big cell");
        assert_eq!(
            five.iter().filter(|c| c.3 == 118.0).count(),
            1,
            "and nothing else is full height once both columns are paired"
        );
        assert_eq!(
            five.iter().map(|c| c.0).collect::<Vec<_>>(),
            [0.0, 69.0, 69.0, 138.0, 138.0],
            "two per column, left to right"
        );

        // Four is the count the design has no layout for, and the rule answers it:
        // three columns, the last holding one and therefore running full height.
        let four = shape(4);
        assert_eq!(four[3], (138.0, 0.0, 62.0, 118.0));
    }

    /// The search dropdown answers the arrows and `Enter` takes the row they are
    /// on — reported as *"search doesn't have keyboard navigation. I can't scroll up
    /// and down through results and press enter."*
    ///
    /// ⚠️ **The unsteered default is the assertion that carries the old rule.**
    /// `Enter` has always taken the first *document* match rather than the first
    /// row, and a project match sits above the files — so a test that only checked
    /// "the arrows move something" would pass against a highlight starting at row
    /// zero, which would silently make `Enter` navigate to a project instead of
    /// opening a file.
    ///
    /// Flip-check, run: dropping the `consume_key` to a plain `key_pressed` leaves
    /// every assertion here green — the arrow still steps — and the damage is
    /// invisible to a probe, because what it costs is the *caret* jumping to the end
    /// of the query inside a widget whose text this test never reads. Recorded
    /// rather than asserted: see §15 D382.
    #[test]
    fn the_search_arrows_move_the_highlight_and_enter_takes_the_row_it_is_on() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "search-keys");
        seed(&mut app, "p-1", "Alps", None);
        let mut paths = Vec::new();
        for name in ["Alpha", "Alpine"] {
            let mut doc = ondin_core::Document::new(app.session.ids.mint());
            paths.push(store::file_document(&root, None, name, &mut doc).unwrap());
        }
        app.library.refresh();
        // One project row and two file rows, in that order — the fixture the
        // default is about.
        app.dash.search = Some("alp".into());
        let _ = galleys(&mut app, &ctx);
        assert_eq!(
            app.dash.search_row, None,
            "nothing has been steered yet, which is not the same as row zero"
        );

        press(&mut app, &ctx, egui::Key::ArrowDown);
        assert_eq!(
            app.dash.search_row,
            Some(2),
            "one step down from the first *file* row, not from the project above it"
        );
        press(&mut app, &ctx, egui::Key::ArrowDown);
        assert_eq!(
            app.dash.search_row,
            Some(2),
            "and the last row is the last row — clamped, like the grid's arrows"
        );
        for _ in 0..4 {
            press(&mut app, &ctx, egui::Key::ArrowUp);
        }
        assert_eq!(app.dash.search_row, Some(0), "clamped at the top too");

        // Row 0 is the project, so Enter navigates rather than opening.
        press(&mut app, &ctx, egui::Key::Enter);
        assert_eq!(app.dash.nav, Nav::Project("p-1".into()));
        assert!(
            app.dash.search.is_none(),
            "and the overlay closes behind it"
        );
        assert!(
            app.session.path.is_none(),
            "a project row opens no document"
        );

        // And with nothing steered, Enter still takes the first document.
        app.dash.search = Some("alp".into());
        app.dash.search_row = None;
        let _ = galleys(&mut app, &ctx);
        press(&mut app, &ctx, egui::Key::Enter);
        assert_eq!(
            app.session.path.as_deref(),
            Some(paths[0].as_path()),
            "which is the rule Enter had before the arrows existed"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A modal marks the focused control **only once a key has moved the focus** —
    /// CSS's `:focus-visible`, reported as *"I just can't see where I'm tabbing
    /// at."*
    ///
    /// ⚠️ **The first and third assertions are the feature.** A ring that appears
    /// whenever something is focused is the thing browsers stopped doing, and this
    /// card focuses its name field on the frame it opens — so a test that only
    /// checked "Tab draws a ring" would pass against a ring that was there all
    /// along.
    ///
    /// Flip-checks, both run. Dropping `&& !pointed` fails the third assertion at 1
    /// against 0. Removing the visibility gate outright fails the *first* — ⚠️ **but
    /// only at sixteen frames per call, and at eight it did not**: the ring was
    /// drawn and merely still tinted by the modal's fade-in, so the assertion passed
    /// against a card that was ringing the whole time. Found by the flip and not by
    /// the run, which is the argument for flipping the case you believe is already
    /// covered.
    #[test]
    fn a_modal_rings_the_focused_control_for_the_keyboard_and_not_for_the_mouse() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "focus-ring");
        app.dash.new_project = Some(NewProject::default());

        // ⚠️ **Sixteen frames per call, and the events only on the first.** A `Modal`
        // is an `Area` and an `Area` *fades in*, which multiplies every colour it
        // paints — the ring came out `#293451` at alpha 95 against the `#6D8CD9` it
        // was asked for, and an exact colour comparison on a single frame therefore
        // matched nothing at all. Eight frames is past the default `animation_time`
        // (0.083s at egui's own 60 Hz clock, which advances even with no real time
        // in the `RawInput`), so the colour is the constant again. ⚠️ **Eight was not
        // enough and looked like it was**: the ring was drawn and simply still
        // tinted, so the "no ring yet" assertion passed against a card that was
        // ringing the whole time — found by a flip, not by the run.
        //
        // ⚠️ **And the colour alone is not enough either**: the sidebar's *New file*
        // button is an accent hairline on a transparent rect at the same corner
        // radius, and the first spelling of this counted it as a ring on a card
        // nobody had tabbed into. `StrokeKind::Outside` is what separates them — a
        // ring *surrounds* its widget where every border in the app bounds one —
        // and neither half is sufficient by itself.
        let rings = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let mut events = Some(events);
            let mut out = 0;
            for _ in 0..16 {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
                    events: events.take().unwrap_or_default(),
                    ..Default::default()
                };
                let full = ctx.run_ui(input, |ui| app.dashboard_ui(ui));
                out = full
                    .shapes
                    .iter()
                    .filter(|cs| match &cs.shape {
                        egui::Shape::Rect(r) => {
                            r.stroke.color == color::ACCENT
                                && r.stroke_kind == egui::StrokeKind::Outside
                        }
                        _ => false,
                    })
                    .count();
            }
            out
        };
        let tab = || {
            vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }]
        };
        let press_at = |at: egui::Pos2| {
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }]
        };

        assert_eq!(
            rings(&mut app, Vec::new()),
            0,
            "the card opens with its name field focused and no ring on it"
        );
        let _ = rings(&mut app, tab());
        assert_eq!(
            rings(&mut app, Vec::new()),
            1,
            "Tab makes the focus visible, and it stays visible"
        );
        // A press anywhere hands it back to the pointer. Aimed at the backdrop,
        // which is a click this card treats as Cancel — the flag is read before the
        // card decides that, so the ring is gone on the same frame.
        assert_eq!(
            rings(&mut app, press_at(egui::pos2(20.0, 20.0))),
            0,
            "and a mouse press takes it away again"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The empty states name what is missing and no longer print the base folder
    /// under it.
    #[test]
    fn an_empty_list_says_so_without_naming_the_folder() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "empty");
        let path = app.library.root.display().to_string();
        for nav in [Nav::All, Nav::Project("p-1".into())] {
            app.dash.nav = nav.clone();
            let runs = galleys(&mut app, &ctx);
            assert!(
                runs.iter().any(|(_, _, t)| t == "No files here yet."),
                "{nav:?} still says the list is empty"
            );
            assert!(
                !runs.iter().any(|(_, _, t)| t == &path),
                "{nav:?} does not print {path:?} under it"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Push a project straight into the library, bypassing the New project card.
    fn seed(app: &mut OndinApp, id: &str, name: &str, folder: Option<&str>) {
        app.library.projects.projects.push(project::Project {
            id: id.into(),
            name: name.into(),
            color: project::PROJECT_COLORS[0].into(),
            folder: folder.map(str::to_string),
            created: 0,
            archived: false,
        });
    }

    /// File one document into a project, so it has a card on *Recent*.
    ///
    /// ⚠️ **Needed by any fixture that counts a project's drawings**, since
    /// `OndinApp::recent_projects` hides a project with no files: a `seed` alone
    /// puts a row in the sidebar and nothing on the page.
    fn seed_file(app: &mut OndinApp, root: &Path, id: &str, name: &str) {
        let project = app.library.projects.get(id).cloned().expect("seeded");
        let mut doc = ondin_core::Document::new(app.session.ids.mint());
        store::file_document(root, Some(&project), name, &mut doc).unwrap();
        app.library.refresh();
    }

    /// **`[S1.3-L1-02]`'s loss, and it is the *second* save that shows it.**
    ///
    /// *Move to project* on the open document rewrote the file's `meta.project`
    /// and re-pointed `session.path`, and told the session's own `Document`
    /// nothing. The session is what the next save serializes, so the save after
    /// the move wrote the **old** project back into the file the move had just
    /// put in the new one — the dashboard then filed the card under the project
    /// the user had moved it out of, correctly, because that is what the file
    /// said.
    ///
    /// ⚠️ **A test that stopped at the move would have been green the whole
    /// time.** The move itself is right: the file is written, the rename
    /// happens, the library lists it under Atlas. Everything about this bug is
    /// downstream of the next `write_document`, so the save is not a flourish at
    /// the end of the fixture — it is the assertion.
    ///
    /// Flip-check, run: dropping the `patch` call from `open_document_follows`
    /// (path re-pointed, metadata left alone) fails here at *"the file the move
    /// produced still belongs to Atlas"*, `left: Some("p-1") right: Some("p-2")`.
    /// That is the on-disk assertion and it is deliberately the **first** of the
    /// two: the in-memory one below it is the mechanism, and a run that showed
    /// the mechanism first would tell the next reader what is wrong before
    /// telling them what is lost.
    #[test]
    fn moving_the_open_document_to_another_project_survives_the_next_save() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "move-open-doc");
        seed(&mut app, "p-1", "Kestrel", Some("kestrel"));
        seed(&mut app, "p-2", "Atlas", Some("atlas"));

        let kestrel = app.library.projects.get("p-1").cloned().expect("seeded");
        let mut doc = ondin_core::Document::new(app.session.ids.mint());
        let path = store::file_document(&root, Some(&kestrel), "Landing v4", &mut doc).unwrap();
        app.library.refresh();
        // Opened, which is what makes this the *open* document rather than one of
        // the other cards on the screen — every one of which was always fine.
        app.session.adopt_document(doc, Some(path.clone()));

        app.dashboard_act(Act::MoveTo(path.clone(), Some("p-2".into())));
        let moved = app
            .session
            .path
            .clone()
            .expect("the session follows its file");
        assert_ne!(moved, path, "the move really moved it");

        // The save the user makes next, from the editor they went back to.
        store::write_document(&moved, &app.session.doc).unwrap();
        app.library.refresh();
        assert_eq!(
            app.library
                .entry_at(&moved)
                .and_then(|e| e.meta.project.clone()),
            Some("p-2".into()),
            "the file the move produced still belongs to Atlas"
        );
        // The mechanism, second: what the save wrote came from here.
        assert_eq!(
            app.session.doc.meta().project.as_deref(),
            Some("p-2"),
            "and it does because the open document was told about the move"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An archived project leaves the sidebar's *Projects* group for a collapsed
    /// group of its own, and leaves the body's project cards and the header's
    /// filter entirely.
    ///
    /// ⚠️ **Collapsed is asserted by the name being *absent*, which is the sort of
    /// assertion that passes for free when a fixture is wrong** — so the same run
    /// asserts the heading *is* there and that opening the group brings the name
    /// back. All three or none of them mean anything.
    ///
    /// Flip-checks, both run, and the finding is that they are **indistinguishable
    /// here**: forcing the group open (`if true` for `if self.dash.archived_open`)
    /// and listing every project in the sidebar (`projects.projects.clone()` for
    /// `active()`) each fail at the same assertion — the archived name being absent
    /// while the group is shut — with the same `left: 1, right: 0`. Two unrelated
    /// bugs, one failure message. ⚠️ **The position assertion further down does not
    /// rescue that**, which was the guess: the run aborts before reaching it. It is
    /// a second statement about where the row lives, not a discriminator.
    #[test]
    fn an_archived_project_leaves_the_lists_for_a_collapsed_group_of_its_own() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "archived-sidebar");
        seed(&mut app, "p-1", "Kestrel", None);
        seed(&mut app, "p-2", "Field Notes", None);
        // A file each, or neither gets a card on *Recent* and the counts below
        // measure the sidebar alone — see `seed_file`.
        seed_file(&mut app, &root, "p-1", "Landing v4");
        seed_file(&mut app, &root, "p-2", "Moodboard");
        app.dash.nav = Nav::Recent;

        let named = |app: &mut OndinApp, name: &str| {
            galleys(app, &ctx)
                .into_iter()
                .filter(|(_, _, t)| t == name)
                .count()
        };
        // ⚠️ **Two runs, not one**: on *Recent* every project is drawn twice, once
        // as a sidebar row and once as a body card. Asserting 1 here passes for the
        // wrong reason later — the counts below are what say *which* of the two
        // drawings archiving removed, and they only mean that if this one is right.
        assert_eq!(
            named(&mut app, "Field Notes"),
            2,
            "the fixture starts with both projects in the sidebar and on the page"
        );
        assert_eq!(
            named(&mut app, "Archived projects"),
            0,
            "and no group heading, because nothing is archived"
        );

        app.set_archived("p-2", true);
        assert!(!app.dash.archived_open, "the group starts collapsed");
        assert_eq!(
            named(&mut app, "Archived projects"),
            1,
            "the heading arrives with the first archived project"
        );
        assert_eq!(
            named(&mut app, "Field Notes"),
            0,
            "and the project itself is nowhere on screen while the group is shut — \
             not in the sidebar, not in the body's project cards"
        );
        assert_eq!(
            named(&mut app, "Kestrel"),
            2,
            "while the one that is not archived is still in both, which is what \
             makes the assertion above about archiving rather than about drawing"
        );

        app.dash.archived_open = true;
        assert_eq!(
            named(&mut app, "Field Notes"),
            1,
            "opening the group brings back the sidebar row and *only* that one — \
             the body's project card stays gone, which is the half of archiving \
             the caret cannot undo"
        );
        // **And under its own heading** — a count says the row exists, not where it
        // is, and "in its own group" is the whole of what archiving does to the
        // sidebar.
        let runs = galleys(&mut app, &ctx);
        assert!(
            run_at(&runs, "Field Notes").1 > run_at(&runs, "Archived projects").1,
            "the row belongs to the archived group, not to the Projects list above it"
        );

        // The filter's own list, asserted through the predicate that gates it:
        // with one active project left it is still offered, and with none it is
        // not — which is the second way to reach an empty dropdown.
        app.dash.nav = Nav::All;
        assert!(
            app.filter_applies(),
            "one active project is still filterable"
        );
        app.set_archived("p-1", true);
        assert!(
            !app.filter_applies(),
            "a library whose every project is archived offers no filter"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// *Save changes* writes all four fields, takes the project's folder — and the
    /// files inside it — with the name, and drops a header filter the archive would
    /// otherwise have made unreachable.
    ///
    /// Flip-checks, both run. Dropping the `forget_archived_filter` call fails at
    /// the filter assertion, as predicted. Discarding `rename_folder`'s answer
    /// (`Ok(_) => before.folder.clone()`, i.e. move the directory and record the old
    /// stem) fails at the **`folder` assertion**, not at the `is_file` one below it
    /// — the prediction here was the wrong way round. Worth keeping both: the files
    /// are already at `harbor-app/` when that first assertion fires, so the failure
    /// message names the *record* while the damage is that the record and the disk
    /// have come apart. A test asserting only the file's new path would have been
    /// green for a library filing every future document into a directory that no
    /// longer exists.
    #[test]
    fn saving_the_card_renames_the_folder_under_the_files_and_forgets_the_filter() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "edit-project");
        seed(&mut app, "p-1", "Kestrel", Some("kestrel"));
        let kestrel = app.library.projects.projects[0].clone();
        let mut doc = ondin_core::Document::new(app.session.ids.mint());
        let filed = store::file_document(&root, Some(&kestrel), "Landing v4", &mut doc).unwrap();
        app.library.refresh();
        assert_eq!(
            filed.parent(),
            Some(root.join("kestrel").as_path()),
            "the fixture put the document inside the project's folder"
        );
        // The open document is this one, so the path fix-up has something to fix.
        app.session.path = Some(filed.clone());
        app.dash.filter = Some("p-1".into());

        app.dash.edit_project = Some(EditProject {
            id: "p-1".into(),
            name: "  Harbor App  ".into(),
            color: project::PROJECT_COLORS[3].into(),
            archived: true,
            name_focused: true,
        });
        app.save_project();

        let saved = app
            .library
            .projects
            .get("p-1")
            .expect("still there")
            .clone();
        assert_eq!(saved.name, "Harbor App", "the name is trimmed and written");
        assert_eq!(saved.color, project::PROJECT_COLORS[3]);
        assert!(saved.archived);
        assert_eq!(saved.folder.as_deref(), Some("harbor-app"));
        assert!(
            root.join("harbor-app").join("landing-v4.ondin").is_file(),
            "and the document went with the folder rather than being left behind"
        );
        assert_eq!(
            app.session.path.as_deref(),
            Some(root.join("harbor-app").join("landing-v4.ondin").as_path()),
            "the open document follows its folder, or the next autosave writes to \
             a path that no longer exists"
        );
        assert_eq!(
            app.dash.filter, None,
            "the header filter named the project that has just been archived, and \
             its own dropdown no longer offers a way to unset it"
        );
        // Written, not just held: this is the file another machine reads.
        assert_eq!(
            project::Projects::read(&root).or_empty().projects[0],
            saved,
            "and all of it reached projects.json"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **Both floating menus still draw their rows** (§15 D729,
    /// `[S20.2-L3-05]`).
    ///
    /// 🚨 **This exists because nothing drove either of them.** The library's two
    /// menus were the same thirty-five lines written twice, and folding them onto
    /// one `anchored_menu` was a refactor the whole 1,247-test suite was silent
    /// about — no test opened either. **A green suite is not evidence about code
    /// no test reaches**, and an extraction is exactly the change that looks safe
    /// because everything stays green.
    ///
    /// What it pins is the shared shell through **both** callers: the panel is
    /// drawn, the rows are laid out at the pitch the constant gives, and each
    /// caller's own `paint` closure put its own contents in them. The filter's
    /// rows are project names; the ⋮ menu's are verbs, one of which is red.
    ///
    /// ⚠️ **Row *contents* rather than row *count*.** A count would pass against a
    /// shell that drew the right number of empty rectangles — which is precisely
    /// what a mis-wired `paint` closure produces.
    ///
    /// 🚨 **The filter half is asserted as a *difference*, and the first version
    /// of this test was vacuous for not being.** `"All projects"` is the filter
    /// **button's** own label and `"Kestrel"` is a **sidebar** row, so both strings
    /// are on screen with the menu shut — `contains` found them and proved
    /// nothing. The flip is what said so: with `paint` never called, the file-menu
    /// assertions went red and the filter ones stayed green. **A string that the
    /// closed state also paints is not evidence about the open one**, so what is
    /// asserted is that opening the menu paints each of them one *more* time.
    #[test]
    fn both_library_menus_draw_their_rows_through_the_shared_shell() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "anchored-menu");
        seed(&mut app, "p-1", "Kestrel", None);
        seed_file(&mut app, &root, "p-1", "Landing v4");
        app.dash.nav = Nav::All;

        let times = |app: &mut OndinApp, ctx: &egui::Context, of: &str| {
            galleys(app, ctx).iter().filter(|(_, _, t)| t == of).count()
        };

        // The filter's dropdown: *All projects* plus every active project — each
        // counted against the closed state, because the button and the sidebar
        // paint the same two strings.
        let shut = (
            times(&mut app, &ctx, ALL_PROJECTS),
            times(&mut app, &ctx, "Kestrel"),
        );
        app.dash.filter_open = true;
        assert_eq!(
            times(&mut app, &ctx, ALL_PROJECTS),
            shut.0 + 1,
            "opening the filter draws its first row on top of whatever already \
             said {ALL_PROJECTS:?}"
        );
        assert_eq!(
            times(&mut app, &ctx, "Kestrel"),
            shut.1 + 1,
            "and a row per active project"
        );

        // The ⋮ menu: verbs, hung off a card. These strings appear nowhere else
        // on the screen, so they are asserted directly — and the contrast with
        // the pair above is the point.
        app.dash.filter_open = false;
        let entry = app.library.entries.first().cloned().expect("one document");
        app.dash.menu_for = Some(entry.path.clone());
        let painted = galleys(&mut app, &ctx);
        let text: Vec<&str> = painted.iter().map(|(_, _, t)| t.as_str()).collect();
        for row in ["Open", "Rename", "Duplicate", "Move to project", "Delete"] {
            assert!(
                text.contains(&row),
                "the file menu draws {row:?}, in {text:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The sidebar's Trash count is read from the cache, not from the folder**
    /// (§15 D728, `[S20.1-L3-07]`).
    ///
    /// `nav_count`'s `Nav::Trash` arm ran `store::trashed`, which is
    /// `scan::scan_dir` — **one file open and one JSON prefix parse per document
    /// in `.trash`** — and `dashboard_sidebar` paints that count on every nav,
    /// every frame, whether or not the trash is being looked at.
    /// `visible_entries` scanned the same folder a second time in the same frame
    /// when it was. With 200 documents in the trash that is 200 file opens a
    /// frame and 400 on *Trash*, for a number that changes only when the user
    /// acts.
    ///
    /// ⚠️ **The module head already forbade this and named the wrong road.** It
    /// says *"a refresh reads every document's prefix… Nothing in this file may
    /// call it from inside a layout loop"* — of `Library::refresh`, which nothing
    /// does call per frame. This was the same hazard by a route that sentence
    /// does not enumerate, **which is exactly why it read as guarded**.
    ///
    /// 🚨 **Asserted by deleting the folder behind the app's back**, which is a
    /// direct test of *"this did not go to disk"* and needs no instrumentation: a
    /// count that survives its own directory being removed was not read from it.
    /// The alternative — counting `read_meta` calls — measures the same thing
    /// through a seam that would have to be built and then maintained.
    ///
    /// **The third step is what stops this being a test for a count that is
    /// simply frozen.** A cache nothing invalidates would pass the first two
    /// assertions and be a worse bug than the scan, because the sidebar would
    /// then lie about the trash for the rest of the session.
    #[test]
    fn the_trash_count_is_cached_rather_than_rescanned_every_frame() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "trash-cache");
        let mut doc = ondin_core::Document::new(app.session.ids.mint());
        let filed = store::file_document(&root, None, "Landing v4", &mut doc).unwrap();
        app.library.refresh();
        store::trash(&root, &app.library.entry_at(&filed).cloned().unwrap()).unwrap();
        app.library.refresh();

        assert_eq!(
            app.nav_count(&Nav::Trash),
            1,
            "the fixture is one trashed document, or this test is about nothing"
        );

        // Behind the app's back: nothing told it, so a cached answer must not move
        // and a scanning one must.
        std::fs::remove_dir_all(root.join(crate::library::scan::TRASH_DIR)).unwrap();
        assert_eq!(
            app.nav_count(&Nav::Trash),
            1,
            "the count still reads 1 with the folder gone — which is the only way \
             to say it never opened it. A scan would answer 0 here, once per nav, \
             once per frame"
        );
        app.dash.nav = Nav::Trash;
        assert_eq!(
            app.visible_entries().len(),
            1,
            "and the list beside it reads the same cache — this was the second \
             scan of the same folder in the same frame"
        );

        // And it is invalidatable, or the sidebar would lie for the session.
        app.library.refresh_trash();
        assert_eq!(
            app.nav_count(&Nav::Trash),
            0,
            "a refresh is what moves it, and every writer of .trash owes one"
        );
    }

    /// **A card with nothing edited writes nothing** (§15 D719, `[S20.3-L3-06]`).
    ///
    /// `EditProject::of` seeds the card from the project, so opening it and
    /// clicking *Save changes* — or pressing Enter, which is gated on the same
    /// predicate the button was — reached `save_project` with every field equal to
    /// what was already stored. It ran anyway: `save_projects` rewrote
    /// `projects.json`, `reload_projects` re-read it, and `refresh` **rescanned
    /// the whole library**, a price `refresh`'s own comment accepts on the
    /// assumption that something moved.
    ///
    /// ⚠️ **The failure is not the wasted work, it is the message.** If that write
    /// fails — and `may_write_projects` cannot prevent it, having passed a moment
    /// earlier, so a sync client holding the file is enough — the user is told
    /// *"Could not write projects.json"* about a change they did not make.
    ///
    /// 🚨 **And that alarm lands *because* `[S20.1-L1-01]` was fixed.** That
    /// finding is the one about this screen writing 22 status messages and
    /// painting none of them; §15 D426 gave it a status line, so a spurious
    /// failure here now reaches the user where it would once have been swallowed.
    /// **A citation to a fixed finding still resolves and reads as though the old
    /// state held**, which is worth saying out loud rather than leaving the
    /// sentence to imply the opposite.
    ///
    /// **`projects.json`'s mtime is the witness**, which is what the finding asked
    /// for: the *record* is identical either way, so an assertion on the projects
    /// list is green through the bug. The stamp is set a second back rather than
    /// read and compared, because whole-second mtimes make "did this get rewritten
    /// in the same second" a coin toss — the trap
    /// `recovery::a_snapshot_older_than_its_document_…` was intermittently red for.
    ///
    /// The two ends are asserted together: a *changed* card still writes. Without
    /// that, a `save_project` that refused everything would pass this test.
    ///
    /// ⚠️ **Flip-check, run: `save_project`'s `differs_from` guard disabled.**
    /// Red at the first mtime assertion, as predicted. **The button's own
    /// `enabled` is deliberately not the thing under test** — it is the second
    /// asker of the predicate, not the rule, and Enter reaches `save_project`
    /// through a different door.
    #[test]
    fn a_card_with_nothing_edited_neither_writes_nor_rescans() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "edit-project-noop");
        seed(&mut app, "p-1", "Kestrel", Some("kestrel"));
        // ⚠️ **`seed` pushes into memory and writes nothing** — it says so, and
        // the fixture assertion below is what said so first. The file has to
        // exist before "did this rewrite it" can mean anything.
        assert!(
            app.library.save_projects(),
            "the fixture needs projects.json on disk to watch"
        );
        let file = root.join("projects.json");
        assert!(file.is_file(), "and there it is");

        let backdate = || {
            std::fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(60))
                .unwrap();
            std::fs::metadata(&file).unwrap().modified().unwrap()
        };

        // The card as `EditProject::of` seeds it — nothing typed.
        let stored = app.library.projects.get("p-1").expect("seeded").clone();
        let before = backdate();
        app.dash.edit_project = Some(EditProject::of(&stored));
        assert!(
            !app.dash
                .edit_project
                .as_ref()
                .unwrap()
                .differs_from(&stored),
            "the fixture is an untouched card, or this test is about nothing"
        );
        app.save_project();
        assert_eq!(
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            before,
            "an untouched card must not rewrite projects.json — and the record \
             being identical either way is exactly why the stamp is the witness"
        );

        // And the control: one character typed and the write happens.
        let before = backdate();
        let mut edited = EditProject::of(&stored);
        edited.name = "Kestrels".into();
        app.dash.edit_project = Some(edited);
        app.save_project();
        assert_ne!(
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            before,
            "a card that was edited still writes — or the guard refuses everything"
        );
        assert_eq!(
            app.library.projects.get("p-1").expect("still there").name,
            "Kestrels"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// *Edit project* is offered on a project's page and nowhere else — and, unlike
    /// *Delete project* beside it, only when the project actually resolves.
    ///
    /// Flip-checks, both run. Falling back to a placeholder `Project` when the
    /// lookup fails (the shape somebody reaches for to "make the button always
    /// work") fails at the dangling-id assertion, with the trash glyph still drawn
    /// one line below — which is what says the two gates are deliberately different
    /// rather than one of them forgotten. And hoisting *Delete project* out of the
    /// `Nav::Project` arm — "you can always delete" — fails in the loop, at the
    /// glyph rather than at the words, since the words are in a tooltip now.
    #[test]
    fn edit_project_is_offered_inside_a_project_and_nowhere_else() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "edit-button");
        seed(&mut app, "p-1", "Kestrel", None);

        for nav in [Nav::Recent, Nav::All, Nav::Starred, Nav::Trash] {
            app.dash.nav = nav.clone();
            let runs = galleys(&mut app, &ctx);
            assert!(
                !runs.iter().any(|(_, _, t)| t == "Edit project"),
                "{nav:?} is not a project"
            );
            // The trash glyph too, which is what gives the positive assertion
            // below any teeth: a glyph that were always on screen would satisfy it
            // from the sidebar, and *Trash* is one of the navs in this loop.
            assert!(
                !runs.iter().any(|(_, _, t)| t == icon::TRASH),
                "{nav:?} has no project to delete either"
            );
        }
        app.dash.nav = Nav::Project("p-1".into());
        let runs = galleys(&mut app, &ctx);
        assert!(runs.iter().any(|(_, _, t)| t == "Edit project"));
        // ⚠️ **The trash *glyph*, because *Delete project* no longer paints its
        // words anywhere on this screen** — they moved into a tooltip, and a
        // tooltip's galley never reaches `.shapes` however many frames are pumped.
        // `icon::TRASH` is the lidded bin and the sidebar's *Trash* row draws
        // `TRASH_SIMPLE`, so the two cannot be confused for one another here.
        assert!(
            runs.iter().any(|(_, _, t)| t == icon::TRASH),
            "beside the button it was split out of"
        );

        // ⚠️ **And not on a nav holding an id `projects.json` no longer has** —
        // the card is seeded from that lookup, so the button would open an empty
        // form over no project. *Delete project* deliberately stays: removing a
        // nav that names nothing is still something a user may want.
        app.dash.nav = Nav::Project("0f9c2b7a4e".into());
        let runs = galleys(&mut app, &ctx);
        assert!(
            !runs.iter().any(|(_, _, t)| t == "Edit project"),
            "a project that cannot be looked up is not editable"
        );
        assert!(runs.iter().any(|(_, _, t)| t == icon::TRASH));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Renaming a document commits — which it could not do **at all**.
    ///
    /// Reported as *"it doesn't save if I press enter. Doesn't lose focus if I press
    /// anywhere on the page. The only option is pressing escape, which dismissed the
    /// name. There's no way I can rename the file."* The field asked for focus on
    /// every frame it drew, so the pass that surrendered the caret took it straight
    /// back and `lost_focus()` — the only thing that commits — was never true.
    ///
    /// **Both doors, because they are one line apart and were both dead.** Enter and
    /// a click elsewhere are the two ways a text field is finished with, and a test
    /// of either alone would leave the other looking covered.
    ///
    /// Flip-check, run: restoring `if !resp.has_focus() { resp.request_focus(); }`
    /// in place of the latch fails at the *first* assertion — the field is still
    /// open — rather than at the name, which is the honest failure: nothing was
    /// renamed because nothing was ever committed.
    #[test]
    fn a_rename_commits_on_enter_and_on_a_click_away() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "rename-commit");
        for name in ["Landing v4", "Sketch"] {
            let mut doc = ondin_core::Document::new(app.session.ids.mint());
            store::file_document(&root, None, name, &mut doc).unwrap();
        }
        app.library.refresh();
        app.dash.nav = Nav::All;
        app.dash.list_view = false;
        let first = app
            .library
            .entries
            .iter()
            .find(|e| e.display_name() == "Landing v4")
            .cloned()
            .expect("the fixture");

        app.rename_entry = Some((first.path.clone(), "Homepage hero".into(), false));
        let _ = galleys(&mut app, &ctx);
        press(&mut app, &ctx, egui::Key::Enter);
        let _ = galleys(&mut app, &ctx);
        assert!(app.rename_entry.is_none(), "Enter closed the field");
        assert!(
            app.library
                .entries
                .iter()
                .any(|e| e.display_name() == "Homepage hero"),
            "and the name was written"
        );
        assert!(
            root.join("homepage-hero.ondin").is_file(),
            "and the file followed it, which is what `store::rename` is for"
        );

        // The other door: a click on the page. Aimed at the sidebar's own margin,
        // which is background on every nav and cannot be a card that steals it.
        let renamed = app
            .library
            .entries
            .iter()
            .find(|e| e.display_name() == "Homepage hero")
            .cloned()
            .expect("just renamed");
        app.rename_entry = Some((renamed.path.clone(), "Hero content".into(), false));
        let _ = galleys(&mut app, &ctx);
        click_at(&mut app, &ctx, egui::pos2(SIDEBAR_W + 400.0, 700.0));
        let _ = galleys(&mut app, &ctx);
        assert!(
            app.rename_entry.is_none(),
            "a click elsewhere closed it too"
        );
        assert!(
            app.library
                .entries
                .iter()
                .any(|e| e.display_name() == "Hero content"),
            "and committed rather than discarding — Escape is the way to not commit"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The rename field draws where the name it replaces did, and leaves the card
    /// beside it alone.
    ///
    /// Two reported faults with one test, because they are two axes of the same
    /// misplacement: *"the textfield moves the text a few px up, so there's a
    /// visible jump"* and *"the card next to it moves and overlaps"*.
    ///
    /// ⚠️ **The x assertion is the one that was already passing** — both spellings
    /// put the word at `rect.left() + 12`, and it is here so a future change to
    /// either inset fails rather than silently splitting them, which is
    /// `SEARCH_TEXT_X`'s whole story.
    ///
    /// Flip-checks, both run and both at the predicted site. Justifying into the
    /// whole 20pt box instead of a one-row rect fails the y assertion at 255.5
    /// against 258 — *"a few px up"*, measured at two and a half. Going back to
    /// `Ui::put` fails the neighbour at 521 against 555: the card beside it is
    /// dragged 34pt left, which is the name box's right edge, which is where `put`
    /// leaves the row's cursor.
    #[test]
    fn the_rename_field_lands_on_the_name_and_moves_no_card() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "rename-geometry");
        for name in ["Landing v4", "Sketch"] {
            let mut doc = ondin_core::Document::new(app.session.ids.mint());
            store::file_document(&root, None, name, &mut doc).unwrap();
        }
        app.library.refresh();
        app.dash.nav = Nav::All;
        app.dash.list_view = false;
        app.dash.sort = Sort::Name;
        let first = app
            .library
            .entries
            .iter()
            .find(|e| e.display_name() == "Landing v4")
            .cloned()
            .expect("the fixture");

        let runs = galleys(&mut app, &ctx);
        let painted = run_at(&runs, "Landing v4");
        let neighbour = run_at(&runs, "Sketch");

        app.rename_entry = Some((first.path.clone(), "Landing v4".into(), false));
        let runs = galleys(&mut app, &ctx);
        let field = run_at(&runs, "Landing v4");
        assert!(
            (field.1 - painted.1).abs() < 0.5,
            "the field's word sits where the painted one did — {field:?} against {painted:?}"
        );
        assert!(
            (field.0 - painted.0).abs() < 0.5,
            "on both axes — {field:?} against {painted:?}"
        );
        assert_eq!(
            run_at(&runs, "Sketch"),
            neighbour,
            "and the card beside it has not moved, which `Ui::put` did by rewinding \
             the row's cursor to the name box"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Three things *Recent* was getting wrong, all reported together: the sort
    /// control did nothing there, projects with no files still drew a card, and a
    /// duplicate made from that page appeared nowhere.
    ///
    /// ⚠️ **One test because they share a fixture and each needs the others to be
    /// meaningful** — the duplicate assertion is about a document showing up in a
    /// list, and a list that is not sorted the way the header says makes "showing
    /// up" a question about ordering.
    ///
    /// Flip-checks, all three run, and one of them corrected the test. Showing every
    /// active project again fails the *Field Notes* count at 2 against 1; dropping
    /// the `mark_opened` on a duplicate fails the last assertion, which prints the
    /// list the copy is missing from. ⚠️ **Exempting *Recent* from the sort failed
    /// at the wrong assertion** — the `assert_ne!` rather than the `Sort::Name` one
    /// above it — because three documents stamped by `mark_opened` land in the same
    /// second and `recent()`'s tie-break is the stem, so its order was *already*
    /// alphabetical and the interesting assertion was vacuous. The fixture now sets
    /// the opened times backwards by hand and asserts that it did.
    #[test]
    fn recent_obeys_the_sort_hides_empty_projects_and_shows_a_duplicate() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "recent");
        seed(&mut app, "p-1", "Kestrel", None);
        seed(&mut app, "p-2", "Field Notes", None);
        seed_file(&mut app, &root, "p-1", "Landing v4");
        for name in ["Alpha", "Zulu"] {
            let mut doc = ondin_core::Document::new(app.session.ids.mint());
            store::file_document(&root, None, name, &mut doc).unwrap();
        }
        app.library.refresh();
        // ⚠️ **Opened in reverse alphabetical order, and the times are set rather
        // than stamped.** `mark_opened` reads the clock, so three calls in a row
        // land in the same second and `recent()`'s tie-break — the stem — puts the
        // list in *alphabetical* order anyway. The `Sort::Name` assertion below
        // would then pass against a Recent that ignores the sort completely, which
        // is exactly the version it exists to rule out. Found by a flip that
        // reported the wrong site.
        for (i, name) in ["Zulu", "Landing v4", "Alpha"].into_iter().enumerate() {
            let id = app
                .library
                .entries
                .iter()
                .find(|e| e.display_name() == name)
                .and_then(|e| e.meta.id.clone())
                .expect("the fixture");
            app.library.local.opened.insert(id, 2_000 - i as u64);
        }
        app.dash.nav = Nav::Recent;
        assert_eq!(
            app.library
                .recent()
                .into_iter()
                .map(|e| e.display_name())
                .collect::<Vec<_>>(),
            ["Zulu", "Landing v4", "Alpha"],
            "the fixture opens them backwards, or the sort below proves nothing"
        );

        // ⚠️ **A project with a file draws twice and one without draws once**, which
        // is the assertion — the sidebar lists both either way, so counting only
        // *Field Notes* would not say whether the card or the row had gone.
        let named = |app: &mut OndinApp, name: &str| {
            galleys(app, &ctx)
                .into_iter()
                .filter(|(_, _, t)| t == name)
                .count()
        };
        assert_eq!(named(&mut app, "Kestrel"), 2, "sidebar row and body card");
        assert_eq!(
            named(&mut app, "Field Notes"),
            1,
            "the empty project keeps its sidebar row and loses its card — a card \
             reading “0 files · Empty” is a picture of nothing to pick up"
        );

        // ⚠️ **The sort is neither applied nor offered here** — one predicate read
        // from both places, so the pair cannot come apart into a control that
        // cycles its own label over a list that never moves, which is what it was.
        let names = |app: &OndinApp| {
            app.visible_entries()
                .into_iter()
                .map(|e| e.display_name())
                .collect::<Vec<_>>()
        };
        for sort in Sort::ALL {
            app.dash.sort = sort;
            assert_eq!(
                names(&app),
                ["Zulu", "Landing v4", "Alpha"],
                "Recent keeps the order it opened them in, whatever {sort:?} says"
            );
            assert!(
                !galleys(&mut app, &ctx)
                    .iter()
                    .any(|(_, _, t)| t == sort.label()),
                "and does not draw a control that would say otherwise"
            );
        }

        // The duplicate, which landed in the library and nowhere the user was
        // looking.
        app.dash.sort = Sort::Name;
        let alpha = app
            .library
            .entries
            .iter()
            .find(|e| e.display_name() == "Alpha")
            .cloned()
            .expect("the fixture");
        app.dashboard_act(Act::Duplicate(alpha.path.clone()));
        assert!(
            names(&app).iter().any(|n| n == "Alpha copy"),
            "a duplicate made on Recent is on Recent — it is in {:?}",
            names(&app)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A save that fails on the way to the library is marked on the library**
    /// — §15 D751, `[S16.3-L1-03]`.
    ///
    /// 🚨 **The finding's premise was already stale and its verdict was not.** It
    /// says the failure *"reports itself into a widget the library never draws"*,
    /// and §15 D426 had given the library a status line before the finding was
    /// ranked — the sentence *was* drawn. What was true is that it was drawn in a
    /// width that shrinks with the window, so the report faded out on the small
    /// screen present mode exists for. **A finding is a claim to check, including
    /// the parts that read as already checked.**
    ///
    /// ⚠️ **`go_to_dashboard` leaves the editor either way, which is the
    /// maintainer's ruling on half (a).** The finding offered making `save_file`
    /// return a `bool` and refusing the walk on `false`; the answer was the status
    /// surface instead, so the assertion here is that the view **did** change and
    /// the failure came with it. A version that blocked the walk would fail the
    /// first assertion, which is what makes it a decision this test records rather
    /// than an accident it tolerates.
    ///
    /// ⚠️ **The unwritable path is a *directory*.** `write_document` on a path
    /// that names a folder fails on every platform without needing a permission
    /// bit set, which is what the finding's own confidence line asked for — it had
    /// reasoned the read-only case from `io::Result` rather than driving it.
    ///
    /// ⚠️ **The recovery snapshot is deliberately *not* asserted here, and the
    /// reason is worth more than the assertion would be.** `go_to_dashboard` drops
    /// it only when the session is clean, so a failed save keeps it and the work
    /// survives to the next launch — the finding says so and it is true. But this
    /// fixture never writes a snapshot, so an assertion that one survives would
    /// pass against a `drop_recovery` that ran unconditionally, which is the
    /// *"its fixture never reached the state"* shape. Pinning it wants a fixture
    /// that has actually taken a snapshot, and that belongs with
    /// `recovery_snapshot_tests` rather than bolted onto a test about a status
    /// surface.
    #[test]
    fn a_failed_save_walks_to_the_library_and_is_marked_there() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "save-fail");

        // A document whose path names a directory: writable target, unwritable
        // file.
        let doomed = root.join("not-a-file.ondin");
        std::fs::create_dir_all(&doomed).unwrap();
        app.session.path = Some(doomed.clone());
        app.view = crate::app::View::Editor;
        // Dirty, or `go_to_dashboard` has nothing to save and the test is about
        // an early return. `mark_unsaved` rather than a real edit, for the reason
        // its own doc gives: this is about a transition, not about a document.
        app.session.mark_unsaved();
        assert!(
            app.session.is_dirty(),
            "fixture: there is something to save, or this asserts nothing"
        );

        app.go_to_dashboard();

        assert_eq!(
            app.view,
            crate::app::View::Dashboard,
            "the walk is not blocked — that is the ruling on the finding's half (a)"
        );
        assert!(
            app.session.status().text.contains("Save failed"),
            "and the failure is in the slot the library reads: {}",
            app.session.status().text
        );
        assert!(
            status_dot_color(&mut app, &ctx).is_some(),
            "and it is marked on the screen the user is now looking at, which is \
             the whole finding — a message written where nobody can read it is \
             the state §15 D426 was written to end"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The header's controls change what you are looking at and no preference**
    /// — §15 D749, `[S20.3-L1-02]`, the maintainer's ruling.
    ///
    /// 🚨 **The failure this pins destroyed a setting the user had chosen, from a
    /// control that is not about it.** `remember_dashboard` wrote all three of
    /// `dashboard_page`, `dashboard_list_view` and `dashboard_sort` from the
    /// current state, and both header controls called it. So: set *Default page on
    /// open* to **Starred**, walk to the **Trash**, press the sort button once, and
    /// the setting is now **Trash** — a value `library_settings`' own picker
    /// refuses to offer, under a comment saying *"landing there on launch would be
    /// a strange way to start"*. The app then opened on the Trash at every launch.
    ///
    /// ⚠️ **Driven through the painted button rather than by calling anything.**
    /// The bug was entirely in *which* control wrote *which* preference, so a test
    /// that called a function would be choosing the very thing under test. The
    /// sort button is found by its label, which is the sort's own word — so this
    /// keeps working when the header is re-laid out, where a hard-coded position
    /// turns into a silently green test.
    ///
    /// ⚠️ **The two assertions are different claims.** That `dash.sort` moved is
    /// the control that the click landed and did its job; that `dashboard_page` did
    /// **not** move is the finding. Without the first this passes against a click
    /// that missed the button entirely — which is the failure mode a coordinate-
    /// driven probe has, and the reason the label lookup is worth the lines.
    ///
    /// ⚠️ **Flip, run:** putting `self.remember_dashboard()` back after
    /// `self.dash.sort = self.dash.sort.next()` fails at the *page* assertion with
    /// `left: "trash"`, and leaves every other test in the workspace green — which
    /// is what says nothing else was watching this.
    ///
    /// (Plain backticks per §15 D319 — `cargo doc` builds without the `test` cfg.)
    #[test]
    fn the_header_controls_do_not_write_the_default_page() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "header-prefs");

        // The state the ruling is about: a chosen default, and the user standing
        // somewhere else entirely.
        app.prefs.dashboard_page = "starred".into();
        app.prefs.dashboard_sort = "edited".into();
        app.dash.nav = Nav::Trash;
        let was = app.dash.sort;

        // The sort button wears the current sort's word, which is what makes it
        // findable without knowing the header's arithmetic.
        let at = {
            let runs = galleys(&mut app, &ctx);
            let label = was.label();
            let (pos, size, _) = runs
                .into_iter()
                .find(|(_, _, t)| t == label)
                .unwrap_or_else(|| panic!("the sort button paints its label {label:?}"));
            pos + size / 2.0
        };
        click_at(&mut app, &ctx, at);

        assert_ne!(
            app.dash.sort, was,
            "control: the click landed on the sort button and cycled it — without \
             this the assertion below is about a click that hit nothing"
        );
        assert_eq!(
            app.prefs.dashboard_page, "starred",
            "the sort button does not touch Default page on open (§15 D749)"
        );
        assert_eq!(
            app.prefs.dashboard_sort, "edited",
            "nor Default sort, which is the preference it is nearest to being \
             about — the header's sort is session state and the setting is a \
             setting"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
