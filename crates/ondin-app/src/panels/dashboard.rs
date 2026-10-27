//! The library screen — what the app opens on (§9.5).
//!
//! A whole window, not a panel: `OndinApp::ui` returns after calling
//! [`OndinApp::dashboard_ui`], so there is no canvas, tool or document selection here.
//! The arrows, `Enter`, `Delete` and `Escape` act on a picked card through
//! [`OndinApp::dashboard_keys`], as `input::resolve` never runs here.
//!
//! Every action goes to disk at once and the library is re-scanned, so the screen
//! cannot disagree with a folder a sync client writes into. A refresh reads every
//! document's prefix (`library::state::Library::refresh`): never call it from inside
//! a layout loop.

use crate::app::OndinApp;
use crate::library::scan::Entry;
use crate::library::state::Sort;
use crate::library::{clock, ids, naming, project, store};
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
/// Where the unarchive glyph's centre sits, in from a sidebar row's right edge: the
/// count's own margin plus half a glyph, so both trailing marks share a right edge.
const UNARCHIVE_X: f32 = 12.0 + 13.0 / 2.0;
/// A card in the grid view: the thumbnail's height, plus its caption.
const CARD_THUMB_H: f32 = 116.0;
/// The caption band under the thumbnail: name centred 15 below it, date 31, the
/// rest padding the date to match the name.
const CARD_CAPTION_H: f32 = 46.0;
/// The design's mosaic: 118 tall, two rows, a 7pt gutter, and at most five of the
/// project's files in it.
const MOSAIC_H: f32 = 118.0;
const MOSAIC_GAP: f32 = 7.0;
/// Five: the largest layout the design draws and the largest the three-column rule
/// in [`OndinApp::project_mosaic`] fills without a column growing a third row.
const MOSAIC_MAX: usize = 5;
/// How many stranded filenames the Settings modal lists before "and N more": that
/// card has no scroll area, so an uncapped list would push *Save changes* off screen.
const STRANDED_ROWS: usize = 6;
/// What a project card keeps clear of its own edges (the design's `padding:10px`).
const PROJECT_PAD: f32 = 10.0;
/// Between the mosaic, the name and the two facts under it (the design's `gap:9px`).
const PROJECT_ROW_GAP: f32 = 9.0;
/// A project card on *Recent*: taller than a file card, carrying a picture of up to
/// five documents.
const PROJECT_CARD_H: f32 = PROJECT_PAD * 2.0 + MOSAIC_H + PROJECT_ROW_GAP * 2.0 + 16.0 + 13.0;
/// The widest a card on the grid may be; a row holds as many cards as keep each
/// this wide or narrower.
const CARD_MAX_W: f32 = 300.0;
/// Between two cards on the grid, across and down.
const GRID_GAP: f32 = 14.0;

/// How many columns the grid lays in `width`: the fewest that keep every card at
/// [`CARD_MAX_W`] or narrower, never fewer than one. The cards fill the row exactly
/// ([`card_width`]), so a wider window adds a column rather than a gutter.
fn grid_cols(width: f32) -> usize {
    (((width + GRID_GAP) / (CARD_MAX_W + GRID_GAP)).ceil() as usize).max(1)
}

/// A card's width in a grid `width` wide, at [`grid_cols`] columns.
fn card_width(width: f32) -> f32 {
    let cols = grid_cols(width) as f32;
    ((width - GRID_GAP * (cols - 1.0)) / cols).max(1.0)
}
/// The search field, from the design.
const SEARCH_W: f32 = 340.0;

/// The narrowest the search field is squeezed to before it is allowed to
/// overlap — wide enough for its glyph and a few letters of a query.
const SEARCH_MIN_W: f32 = 120.0;

/// The search field's rectangle in the top bar: [`SEARCH_W`] wide, centred on the
/// window's `centre_x`, clear of the brand (`left`) and the right cluster (`right`) —
/// slid off centre first, then narrowed to [`SEARCH_MIN_W`]. The centre is the
/// window's, not the bar's `max_rect`, which its margins narrow.
fn search_rect(centre_x: f32, centre_y: f32, left: f32, right: f32) -> egui::Rect {
    const GAP: f32 = 12.0;
    let (lo, hi) = (left + GAP, right - GAP);
    let w = SEARCH_W.min(hi - lo).max(SEARCH_MIN_W);
    // Centred if it fits there; else as near the centre as the room allows —
    // and on a window too narrow even for the minimum, against the brand.
    let x = centre_x.min(hi - w / 2.0).max(lo + w / 2.0);
    egui::Rect::from_center_size(egui::pos2(x, centre_y), egui::vec2(w, 28.0))
}
/// A row in the search dropdown.
const SEARCH_ROW_H: f32 = 30.0;
/// Where the search field's text starts from its left edge: the painted placeholder
/// and the live `TextEdit` share it, so the words do not move when the field opens.
const SEARCH_TEXT_X: f32 = 30.0;
/// Type size of the search field's text and its placeholder, in both states.
const SEARCH_PT: f32 = 11.5;
/// The row a [`field_label`] is allocated: its galley and nothing else.
const FIELD_LABEL_H: f32 = 12.0;
/// What the project filter says when nothing is filtered, and the first row of
/// its own menu (which is how it is unset); the two must be the same words.
const ALL_PROJECTS: &str = "All projects";
/// The project filter's closed button; longer project names elide.
const FILTER_W: f32 = 132.0;
/// The filter's dropdown (the design's `width:190px`), wider than the button.
const FILTER_MENU_W: f32 = 190.0;
/// A row of either floating menu on this screen (the design's 26, plus two); one
/// constant so the two menus' rows cannot come apart.
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

/// A floating menu hung under `anchor`, right edges together, or opened at it
/// when `anchor` is a point (a right-click's press, flipped off the screen's
/// edges): the panel, its rows, their hover ground and the click-away test. `paint`
/// draws a row's contents into the rect it is given. Escape is handled in
/// `dashboard_keys` behind `library_menu_open()`, since a keyboard rule in a
/// drawing helper only runs on frames the menu is drawn.
fn anchored_menu(
    ui: &mut egui::Ui,
    id: egui::Id,
    anchor: egui::Rect,
    w: f32,
    rows: usize,
    paint: impl Fn(&egui::Painter, usize, egui::Rect),
) -> MenuOutcome {
    let h = rows as f32 * FILTER_ROW_H + 10.0;
    let pos = if anchor.size() == egui::Vec2::ZERO {
        ui::menu_place(ui.ctx(), anchor.min, egui::vec2(w, h))
    } else {
        egui::pos2(anchor.right() - w, anchor.bottom() + 4.0)
    };
    let area = egui::Area::new(id)
        .order(egui::Order::Foreground)
        .fixed_pos(pos);
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
        // Anchor as well as menu: a click on the opener is its own toggle, and
        // dismissing on it would close and reopen in one frame.
        dismissed: chosen.is_none()
            && ui.ctx().input(|i| i.pointer.any_click())
            && !rect.contains(at(rect))
            && !anchor.contains(at(anchor)),
    }
}
/// What the filter button's content keeps clear of its own corners.
const FILTER_PAD_X: f32 = 10.0;
/// A Settings dropdown's closed width (the design's `min-width:132px`), so the three
/// rows line up as a column.
const PICKER_W: f32 = 132.0;
/// Between a [`field_label`] and the box it names (the design's `gap:4px`).
const FIELD_LABEL_GAP: f32 = 4.0;
/// How many projects and how many documents a search offers, so a query matching
/// thirty files still shows the project it probably meant.
const SEARCH_MAX: usize = 6;
/// How many past queries the empty search offers.
const SEARCH_RECENTS: usize = 5;
/// Every control on the body header's right-hand cluster — the sort button, the
/// view track and *Delete project*; the app's control height, `settings::FIELD_H`.
const HEADER_CONTROL_H: f32 = 28.0;
/// A cell of the grid/list track: [`HEADER_CONTROL_H`] less the two points
/// [`ui::segmented`] pads its track by on each side.
const VIEW_CELL_H: f32 = HEADER_CONTROL_H - 4.0;
/// The grid/list track: two 26pt cells, the 2pt gap and 2pt padding at each end,
/// because [`ui::segmented`] takes the outer width.
const VIEW_TRACK_W: f32 = 26.0 * 2.0 + 2.0 + 4.0;
/// *Delete project*, the design's 32 — wider than [`HEADER_CONTROL_H`] so the one
/// wordless control still reads as a button.
const DELETE_W: f32 = 32.0;

/// How wide the sort button must be: the widest of [`Sort::ALL`], not the one
/// showing, or the button would twitch sideways as its label cycles.
fn sort_button_w(ctx: &egui::Context) -> f32 {
    Sort::ALL
        .into_iter()
        .map(|s| ui::action_button_w(ctx, s.label()))
        .fold(0.0, f32::max)
}

/// Which sidebar entry is showing.
///
/// `Project` carries an id, not an index: the project list is rebuilt from
/// `projects.json` on every refresh and an index can name another project.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) enum Nav {
    #[default]
    Recent,
    All,
    Starred,
    /// The library's kits that are sharing (§5.3e) — a filter, not a section.
    Kits,
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
            Nav::Kits => "kits",
            Nav::Trash => "trash",
            Nav::Project(id) => id,
        }
    }

    /// What the sidebar and the Settings picker call it. A project's label comes
    /// from the project list, which this cannot reach, so it names the kind.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Nav::Recent => "Recent",
            Nav::All => "All files",
            Nav::Starred => "Starred",
            Nav::Kits => "Kits",
            Nav::Trash => "Trash",
            Nav::Project(_) => "Project",
        }
    }

    /// Read a preference back. Only the fixed entries Settings offers: a remembered
    /// project may have been deleted between launches, and *Kits* is never written,
    /// so both land on *Recent*.
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
/// On `OndinApp` rather than in egui's temp storage, because code outside the layout
/// reads it. None of it is a preference or outlives the process: the `dashboard_*`
/// preferences are defaults chosen in Settings, while `nav`, `sort` and `list_view`
/// here are what is showing now.
#[derive(Default)]
pub(crate) struct DashboardState {
    pub(crate) nav: Nav,
    pub(crate) sort: Sort,
    pub(crate) list_view: bool,
    /// How many columns the file grid was last laid in, for the arrow keys; `0`
    /// before the first frame, which `arrow_target` reads as one.
    pub(crate) grid_cols: usize,
    /// Where the keyboard's cursor is, by path; only `OndinApp::dashboard_keys`
    /// writes it. A click opens the document instead, so empty is the ordinary state.
    ///
    /// The path can name a file that is gone, since every action re-scans.
    /// [`OndinApp::dashboard_body`] clears it, and that must run before
    /// [`OndinApp::dashboard_keys`] because `Enter` and `Delete` act on it.
    pub(crate) selected: Option<PathBuf>,
    /// Set when the keyboard moved [`DashboardState::selected`], cleared by the card
    /// or row that scrolled into view; a scroll on every picked card would jerk the
    /// grid under a pointer click.
    pub(crate) scroll_to_selected: bool,
    /// The document whose ⋮ menu is open, by path.
    pub(crate) menu_for: Option<PathBuf>,
    /// Where a right-click opened that menu; `None` hangs it under the ⋮.
    pub(crate) menu_at: Option<egui::Pos2>,
    /// Whether either floating menu was on screen when this frame began.
    ///
    /// Latched at the top of `dashboard_ui`, because the live flags are cleared
    /// mid-frame by the dismissal whose click has to be spent: a menu on a card that
    /// sorts first is already closed when later cards are drawn. Never persisted.
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
    /// Collapsed on every launch, deliberately not a preference.
    pub(crate) archived_open: bool,
    /// The project filter in the header, by id. `None` is *All projects*.
    pub(crate) filter: Option<String>,
    /// Whether the filter's dropdown is showing.
    pub(crate) filter_open: bool,
    /// The project awaiting delete confirmation, and what to do with its files.
    pub(crate) deleting_project: Option<DeleteProject>,
    /// The search query while the overlay is open; `None` when it is closed.
    pub(crate) search: Option<String>,
    /// Which row of the search dropdown the arrows have moved to, if any.
    ///
    /// `None` is "not steered yet", not row zero: the highlight then sits on the
    /// first document match, which the groups above can push down as the query
    /// changes. Cleared when the query changes and when the overlay opens.
    pub(crate) search_row: Option<usize>,
}

/// A migration that did not move everything, kept so the modal can report it and
/// offer the run again (`OndinApp::stranded`): which files are still in the old
/// folder, and where it is.
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
/// it is a draft. `PartialEq` lights *Save changes* when it differs from
/// [`LibrarySettings::from_prefs`]; the derive hides an unread field from
/// `dead_code`, so read the struct against the modal after removing a row.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct LibrarySettings {
    /// As typed. Empty means "the default", not "nothing".
    pub(crate) base_folder: String,
    /// Whether Save should move the existing library to the new folder. Defaults
    /// off: moving a whole library must be something the user turned on.
    pub(crate) migrate: bool,
    pub(crate) autosave_secs: u64,
    pub(crate) trash_keep_days: u64,
    pub(crate) version_history: bool,
    pub(crate) history_size: u64,
    pub(crate) default_page: Nav,
    pub(crate) default_list_view: bool,
    pub(crate) default_sort: Sort,
    pub(crate) reopen_last: bool,
}

impl LibrarySettings {
    /// Seed the draft from what is in force. `base_folder` comes from the resolved
    /// root, not the raw preference, which is `None` until one is set.
    pub(crate) fn from_prefs(prefs: &crate::prefs::Prefs, root: &Path) -> Self {
        Self {
            base_folder: root.display().to_string(),
            migrate: false,
            autosave_secs: prefs.autosave_secs,
            trash_keep_days: prefs.trash_keep_days,
            version_history: prefs.version_history,
            history_size: prefs.history_size,
            default_page: Nav::from_id(&prefs.dashboard_page),
            default_list_view: prefs.dashboard_list_view,
            default_sort: Sort::from_id(&prefs.dashboard_sort),
            reopen_last: prefs.reopen_last,
        }
    }
}

/// The Delete project modal's own state.
///
/// `keep_files` defaults to true and the design's toggle ("Delete files too") is its
/// inverse, so the field reads in the direction the code has to be safe in.
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
    /// Whether to create a matching folder on disk — the user's choice alone
    /// (`library::project`).
    pub(crate) folder: bool,
    /// Whether the name field has been handed the caret: a latch, since asking every
    /// frame would make the field impossible to leave.
    pub(crate) name_focused: bool,
}

impl Default for NewProject {
    fn default() -> Self {
        Self {
            name: String::new(),
            color: 0,
            // On: a project visible in Explorer needs no explanation.
            folder: true,
            name_focused: false,
        }
    }
}

/// The Edit project modal's own state — the New project card again, with
/// *Create folder* replaced by *Archive project* and *Create* by *Save changes*.
///
/// It holds the project's id and a copy of its fields, for [`Nav`]'s reason; Save
/// looks the id up again and does nothing if it has gone.
pub(crate) struct EditProject {
    pub(crate) id: String,
    pub(crate) name: String,
    /// The colour as it will be written: a hex string, unlike [`NewProject::color`],
    /// so a colour this build does not know survives an edit of the name and no
    /// swatch is ringed.
    pub(crate) color: String,
    /// Whether the project is to be archived (`library::project`).
    pub(crate) archived: bool,
    /// [`NewProject::name_focused`]'s latch.
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

    /// Whether this card holds anything [`Self::of`] did not seed it with.
    ///
    /// The mirror of `of`: every field it copies as content is compared here
    /// (`id` and `name_focused` are not content). `name` is compared trimmed
    /// because `save_project` stores it trimmed.
    fn differs_from(&self, p: &project::Project) -> bool {
        self.name.trim() != p.name || self.color != p.color || self.archived != p.archived
    }
}

/// Which of the sidebar's two project groups a row belongs to, which decides what
/// the row carries at its trailing edge.
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
/// Deferred rather than done in place, because each of these mutates the entry list
/// the layout is iterating.
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
    /// Write the Edit project card back, read from `DashboardState::edit_project`.
    SaveProject,
    /// Archive or unarchive a project outright (the sidebar's unarchive button);
    /// unlike [`Act::SaveProject`] it touches only the flag and cannot rename a folder.
    SetArchived(String, bool),
    ToggleStar(String),
    /// *Make a kit* (`true`) or *Stop sharing as kit* (`false`) on a file; the
    /// second asks first, so the menu sets `OndinApp::unsharing_kit` instead.
    SetKit(PathBuf, bool),
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

        // Before anything is drawn: the dismissals run from inside the cards. The tab
        // menu counts, as its click-away is spent here too.
        let library_menu = self.library_menu_open();
        self.dash.menu_was_up = library_menu || self.tabs.menu.is_some();

        // Before any card asks: take the worker's finished answers and drop unwanted covers.
        self.covers.pass(ui.ctx());

        // Taken here, not in the editor's `take_dropped_images`, which sits below the
        // `View::Dashboard` return. A modal backdrop does not stop a drop, so
        // `library_keys_are_free` gates it, not the editor's `modal_is_up`.
        if self.library_keys_are_free() {
            self.take_dropped_documents(ui.ctx());
            if let Some(key) = crate::tabs::read_keys(ui.ctx()) {
                self.tab_key(key);
            }
        }
        // The tab menu holds the keyboard (`library_keys_are_free`); `Escape` closes
        // only it, consumed so `dashboard_keys` does not also clear the cursor.
        if self.tabs.menu.is_some()
            && ui
                .ctx()
                .input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.tabs.menu = None;
        }
        // A click that dismisses a library menu is spent doing so; the strip latches
        // its own menu.
        self.tab_strip(ui, true, library_menu);
        self.dashboard_top_bar(ui, &mut act);
        egui::Panel::left("library-sidebar")
            .exact_size(SIDEBAR_W)
            .resizable(false)
            .frame(ui::panel_frame(color::PANEL, 10, 14))
            .show(ui, |ui| self.dashboard_sidebar(ui, &mut act));
        // The drop hint uses the panel's own rect; `dashboard_body`'s is less its 20pt margin.
        let body = egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(color::BG).inner_margin(20))
            .show(ui, |ui| self.dashboard_body(ui, &mut act));
        self.library_drop_hint(ui, body.response.rect);

        self.new_project_modal(ui.ctx(), &mut act);
        self.edit_project_modal(ui.ctx(), &mut act);
        self.delete_project_modal(ui.ctx(), &mut act);
        self.move_modal(ui.ctx(), &mut act);
        self.delete_modal(ui.ctx(), &mut act);
        self.stop_sharing_dialog(ui.ctx());
        self.library_settings_modal(ui.ctx());

        if let Some(act) = act {
            self.dashboard_act(act);
        }
    }

    /// Brand mark, name, search, the update chip and the Settings door, under the tab
    /// strip, which is the window's title bar.
    fn dashboard_top_bar(&mut self, ui: &mut egui::Ui, act: &mut Option<Act>) {
        egui::Panel::top("library-topbar")
            .exact_size(TOP_H)
            .resizable(false)
            .frame(crate::app::top_bar_frame())
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    // The editor's mark, at rest rather than as a control.
                    let (mark, _) =
                        ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::empty());
                    crate::ui::logo(ui, mark, false);
                    let brand = ui
                        .label(
                            egui::RichText::new("Ondin")
                                .size(13.0)
                                .color(theme::text::STRONG),
                        )
                        .rect;

                    let cluster = ui
                        .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // The gap to the update pill is `TOP_GAP`, set here because
                            // egui adds an item's gap when the item is placed; with no
                            // pill the status dot follows and keeps the row's 12.
                            let pill = self.updater.state().label().is_some();
                            ui.spacing_mut().item_spacing.x =
                                if pill { crate::app::TOP_GAP } else { 12.0 };
                            if icon_button(
                                ui,
                                icon::SLIDERS,
                                crate::settings::SETTINGS_BOX,
                                crate::settings::SETTINGS_GLYPH,
                                false,
                                true,
                            )
                            .on_hover_text("Library settings")
                            .clicked()
                                && !self.dash.menu_was_up
                            {
                                self.library_settings = Some(LibrarySettings::from_prefs(
                                    &self.prefs,
                                    &self.library.root,
                                ));
                            }
                            ui.spacing_mut().item_spacing.x = 12.0;
                            if crate::update::chip(ui, self.updater.state())
                                && !self.dash.menu_was_up
                            {
                                self.restart_to_update();
                            }
                            // A dot with the message on hover: a width-bound message
                            // would fade out on narrow windows.
                            crate::app::status_dot(ui, self.session.status());
                        })
                        .response
                        .rect;

                    // Painted, not laid out between the clusters, so it is window-centred.
                    let field = search_rect(
                        ui.ctx().content_rect().center().x,
                        ui.max_rect().center().y,
                        brand.right(),
                        cluster.left(),
                    );
                    self.search_field(ui, field, act);
                });
            });
    }

    /// The search box, and the results under it while it is open.
    fn search_field(&mut self, ui: &mut egui::Ui, rect: egui::Rect, act: &mut Option<Act>) {
        let ctx = ui.ctx().clone();
        // `Ctrl+K`/`Ctrl+P` are read here, not through `input::resolve`. Ctrl without
        // Alt or Shift, so `Ctrl+Shift+K` (*Place image* in the editor) opens nothing.
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

        // Escape is consumed, not read: this branch shuts the overlay before the body, so
        // `dashboard_keys` would otherwise clear `dash.selected` with the same press.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.dash.search = None;
            return;
        }
        // Arrows are consumed before the `TextEdit` is added, or it would also move its caret.
        let step = ctx.input_mut(|i| {
            i32::from(i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown))
                - i32::from(i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp))
        });
        let Some(query) = self.dash.search.as_mut() else {
            return;
        };
        // One text row tall and centred by hand: `Ui::put` justifies its widget, so a
        // taller rect would draw the galley at the top. egui ignores `TextEdit::margin`
        // once a frame is supplied, so the inset is the rect's.
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
                // The box is painted above; `Frame::NONE` also suppresses egui's
                // focus ring (`ui::text_field`).
                .frame(egui::Frame::NONE)
                .font(egui::FontId::proportional(SEARCH_PT))
                .hint_text(
                    egui::RichText::new("Search files and projects")
                        .size(SEARCH_PT)
                        .color(theme::text::FAINT),
                ),
        );
        // Requested every frame, for `rename_field`'s reason; the dismissal below makes that safe.
        if !edit.has_focus() {
            edit.request_focus();
        }
        if edit.changed() {
            self.dash.search_row = None;
        }
        let panel = self.search_results(ui, rect, step, act);
        // Read as a press so it lands before whatever is underneath acts on the release.
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

    /// The dropdown under the search field: matching projects, then documents, both
    /// capped; a project match is the likelier intent.
    fn search_results(
        &mut self,
        ui: &mut egui::Ui,
        anchor: egui::Rect,
        step: i32,
        act: &mut Option<Act>,
    ) -> Option<egui::Rect> {
        let query = self.dash.search.clone().unwrap_or_default();
        let needle = query.trim().to_lowercase();
        let recents: Vec<String> = if needle.is_empty() {
            self.library.local.recent_searches.clone()
        } else {
            Vec::new()
        };
        // Archived projects are included, unlike `Projects::active`: the name is typed already.
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
        // Most recently edited first, ties by name, before the cap: `Library::entries`
        // is in directory order, which differs by platform.
        let files: Vec<Entry> = if needle.is_empty() {
            Vec::new()
        } else {
            let mut found: Vec<Entry> = self
                .library
                .entries
                .iter()
                .filter(|e| e.display_name().to_lowercase().contains(&needle))
                .cloned()
                .collect();
            self.library.sort_entries(&mut found, Sort::Edited);
            found.truncate(SEARCH_MAX);
            found
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

        // `Enter` takes the highlighted row, whatever its kind. Unsteered, it sits on
        // the first document match, recomputed as the groups above change length.
        let default = if files.is_empty() {
            0
        } else {
            recents.len() + projects.len()
        };
        let mut sel = self.dash.search_row.unwrap_or(default).min(rows - 1);
        if step != 0 {
            // Clamped rather than wrapping, as `arrow_target` does for the grid.
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
                // Pointer and keyboard can be on different rows; the `Enter` row is the louder.
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

    /// The panel the results are drawn into: its ground, border and position under
    /// the field. Returns its rect, so the field can tell a press on a result row
    /// from one that means *shut this*.
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

    /// Push a query onto the per-machine recent-search list, only when it led
    /// somewhere, or every prefix of a typed word would fill the list.
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
        // The glyph is a separate icon-font `p.text`, as the bundled Inter has no
        // fullwidth plus; `ui::action_button` paints a recessed ground, so is not reused.
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
        // Hand-rolled, so `nav_row`'s menu gate does not reach it.
        if resp.clicked() && !self.dash.menu_was_up {
            *act = Some(Act::NewFile);
        }

        ui.add_space(10.0);
        for (nav, glyph) in [
            (Nav::Recent, icon::CLOCK_COUNTER_CLOCKWISE),
            (Nav::All, icon::FILES),
            (Nav::Starred, icon::STAR),
            (Nav::Kits, icon::PACKAGE),
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
        // Cloned because the row draws while `self` is borrowed.
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
            Nav::Kits => self
                .library
                .entries
                .iter()
                .filter(|e| is_kit(&self.library, e))
                .count(),
            // The cache, not the scan: the sidebar paints this count every frame
            // (`Library::trashed`).
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
        resp.clicked() && !self.dash.menu_was_up
    }

    /// One project in the sidebar, in either of the two groups. The archived row is
    /// the same row a tier quieter: it selects the same way.
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
        // Registered after the row, so on top of it (as `file_menu_button` does).
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
            // The design's `opacity:.45` on the archived dot.
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
            // The tooltip needs the glyph's own `Response`; a `Ui::scope` shows none.
            let undo = undo.on_hover_text("Unarchive");
            if undo.clicked() && !self.dash.menu_was_up {
                return ProjectRowHit::Unarchive;
            }
        }
        // A click spent dismissing a menu must not also re-nav; *Unarchive* shares this rect.
        if resp.clicked() && !self.dash.menu_was_up {
            ProjectRowHit::Select
        } else {
            ProjectRowHit::None
        }
    }

    /// The heading, the controls beside it, and the documents.
    fn dashboard_body(&mut self, ui: &mut egui::Ui, act: &mut Option<Act>) {
        let entries = self.visible_entries();
        // A selection that names nothing is cleared here, against the list on screen: the
        // keys act on it and a filtered-off one cannot be dismissed.
        if self
            .dash
            .selected
            .as_ref()
            .is_some_and(|p| !entries.iter().any(|e| &e.path == p))
        {
            self.dash.selected = None;
        }
        // Before anything draws, so a card the arrows moved onto scrolls into view the
        // same frame; after the clear, so the index is into the list on screen.
        self.dashboard_keys(&ui.ctx().clone(), &entries, act);
        // Registered before the cards so the cards win: egui's hit test takes the last
        // widget at a point, and a later background would swallow every card click.
        let background = ui.interact(
            ui.max_rect(),
            egui::Id::new("library-body-background"),
            egui::Sense::click(),
        );
        self.dashboard_header(ui, entries.len());
        ui.add_space(14.0);

        // The same predicate the cards are drawn from, or the heading appears over nothing.
        let show_projects = self.dash.nav == Nav::Recent && !self.recent_projects().is_empty();
        // A project's grid ends in a *New file* card, so it needs no empty state, except
        // while the library's folder is missing (`new_file_into`).
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
        if background.clicked() {
            self.dash.selected = None;
        }
    }

    /// *Drop to import* — the outline and the words, while files are over the window.
    /// It borrows the canvas drop indicator's colour. The outline marks only the body
    /// while [`Self::take_dropped_documents`] takes a drop anywhere: under-promising.
    fn library_drop_hint(&self, ui: &egui::Ui, rect: egui::Rect) {
        let files = ui.input(|i| i.raw.hovered_files.len());
        if files == 0 {
            return;
        }
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

    /// The documents the current nav and filter select, sorted. Cloned rather than
    /// borrowed so the drawing code can call back into `&mut self` (see [`Act`]).
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
            Nav::Kits => self
                .library
                .entries
                .iter()
                .filter(|e| is_kit(&self.library, e))
                .cloned()
                .collect(),
            // The cache, for `nav_count`'s reason.
            Nav::Trash => self.library.trashed().to_vec(),
            Nav::Project(id) => self
                .library
                .entries
                .iter()
                .filter(|e| e.meta.project.as_deref() == Some(id.as_str()))
                .cloned()
                .collect(),
        };
        if self.filter_applies()
            && let Some(filter) = &self.dash.filter
        {
            out.retain(|e| e.meta.project.as_deref() == Some(filter.as_str()));
        }
        if self.sort_applies() {
            self.library.sort_entries(&mut out, self.dash.sort);
        }
        out
    }

    /// Whether the sort control is offered, and so whether
    /// [`OndinApp::visible_entries`] applies it.
    ///
    /// False on *Recent*, whose order (when this machine opened each document) is
    /// the page's point and which `Sort` cannot cycle back to.
    fn sort_applies(&self) -> bool {
        self.dash.nav != Nav::Recent
    }

    /// The projects *Recent* shows cards for — the active ones that have a file
    /// in them. Narrower than [`project::Projects::active`], which the sidebar uses:
    /// an empty project needs a page but not a "0 files" card.
    fn recent_projects(&self) -> Vec<project::Project> {
        self.library
            .projects
            .active()
            .filter(|p| self.library.file_count(&p.id) > 0)
            .cloned()
            .collect()
    }

    // No `act` parameter: the two things this commits (`dash.deleting_project`,
    // `dash.edit_project`) open a card and mutate no list, so they bypass `Act`.
    fn dashboard_header(&mut self, ui: &mut egui::Ui, count: usize) {
        let (heading, subheading) = self.heading();
        ui.horizontal(|ui| {
            // The tall item first: a `horizontal` row centres each item against the
            // height known when it is placed.
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
                // Two plain buttons rather than a ⋮ menu: the destructive one is a glyph to aim at.
                if let Nav::Project(id) = self.dash.nav.clone() {
                    if delete_project_button(ui).clicked() && !self.dash.menu_was_up {
                        self.dash.deleting_project = Some(DeleteProject {
                            id: id.clone(),
                            keep_files: true,
                            transfer_to: None,
                        });
                    }
                    // After *Delete project*: a right-to-left row places the first item at
                    // the right margin. Drawn only when the project resolves, unlike
                    // *Delete project*, which stays useful for one `projects.json` lost.
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
                // A segmented track, not two icon buttons: one of grid and list is always on.
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
                // Writes no preference: the `dashboard_*` defaults are set only in Settings.
                if let Some(i) = picked {
                    self.dash.list_view = i == 1;
                }
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
                    self.dash.sort = self.dash.sort.next();
                }
                self.project_filter(ui);
            });
        });
    }

    /// Whether the project filter is offered, and so whether
    /// [`OndinApp::visible_entries`] applies it: one predicate for the control and
    /// the selection, or a filter set on *All files* would keep selecting on
    /// *Recent* with no control to unset it. Not on *Recent*, not inside a project,
    /// and not without an active project to filter by.
    fn filter_applies(&self) -> bool {
        matches!(
            self.dash.nav,
            Nav::All | Nav::Starred | Nav::Kits | Nav::Trash
        ) && self.library.projects.active().next().is_some()
    }

    /// *All projects ⌄* — the body header's project filter: a button and an `Area`, not
    /// a `ComboBox`, because the closed state carries a colour dot. The ground is
    /// [`ui::button_face`]'s; [`FieldButton::Set`] lights while open or filtering.
    fn project_filter(&mut self, ui: &mut egui::Ui) {
        // A filter naming a vanished project is no filter. Above the early return on
        // purpose: deleting the last project is exactly when the control is not drawn.
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
            // Closed as well as hidden, or the dropdown reappears on the next screen.
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
                    elide(p.ctx(), name, 11.5, w - 60.0),
                    egui::FontId::proportional(11.5),
                    if on { color::TEXT } else { theme::text::MUTED },
                );
                // The tick's column is reserved either way, so rows do not shift.
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

    /// The title and the line under it, for the current nav. `{n}` in the
    /// subheading is filled with the visible count by the caller.
    fn heading(&self) -> (String, String) {
        let nav = &self.dash.nav;
        match nav {
            Nav::Recent => (nav.label().into(), "Picked up where you left off".into()),
            Nav::All | Nav::Starred => (nav.label().into(), "{n} files".into()),
            Nav::Kits => (nav.label().into(), "{n} kits".into()),
            Nav::Trash => (
                nav.label().into(),
                match self.prefs.trash_keep_days {
                    // The zero case says the opposite of "kept for 0 days".
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
            // A missing folder (`library::state::Library::root_unavailable`) answers
            // every nav, first: the other sentences would lie.
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
                Nav::Kits => (
                    icon::PACKAGE,
                    "No kits yet. Make a file a kit from its menu to share its components.",
                ),
                Nav::Trash => (icon::TRASH_SIMPLE, "The trash is empty."),
                _ => (icon::FILES, "No files here yet."),
            };
            ui.label(
                egui::RichText::new(glyph)
                    .font(theme::icon_font(28.0))
                    .color(if unavailable {
                        color::WARN
                    } else {
                        theme::text::FAINT
                    }),
            );
            ui.add_space(8.0);
            ui.label(egui::RichText::new(line).size(12.5).color(theme::text::DIM));
            // No path under the sentence: the base folder lives in Settings.
        });
    }

    /// The row of project cards at the top of *Recent*.
    fn project_cards(&mut self, ui: &mut egui::Ui) {
        section_label(ui, "PROJECTS");
        ui.add_space(6.0);
        let gap = GRID_GAP;
        let cols = grid_cols(ui.available_width());
        let card_w = card_width(ui.available_width());
        let projects = self.recent_projects();
        let mut go: Option<String> = None;
        for row in projects.chunks(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for p in row {
                    let count = self.library.file_count(&p.id);
                    let touched = self
                        .library
                        .project_touched(&p.id)
                        .map(|t| clock::relative_label(clock::since(t)))
                        .unwrap_or_else(|| "Empty".into());
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

    /// One project card: a mosaic of its files, then its name, then its two facts
    /// — the design's shape. Returns whether it was clicked.
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

    /// The project card's picture: up to [`MOSAIC_MAX`] of its files' covers, newest
    /// first by `Entry::modified`, so the big cell is the file most recently touched.
    /// The layout rule is stated and tested in the free [`mosaic_cells`].
    fn project_mosaic(
        &mut self,
        ui: &mut egui::Ui,
        id: &str,
        rect: egui::Rect,
        dot: egui::Color32,
    ) {
        if !ui.is_rect_visible(rect) {
            return;
        }
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
            // Fitted and inset, for `file_card`'s reasons.
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
    /// `None` unless the nav's project resolves and may receive a document
    /// ([`project::Projects::destination`] refuses an archived one) and the library
    /// folder is there ([`OndinApp::library_writable`]); `None` also lets the empty
    /// state through. `pub(crate)` so a test in `app.rs` can check card and act agree.
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
    /// Slots are counted by index, not `chunks`, so the *New file* card can finish a
    /// short last row.
    fn file_grid(
        &mut self,
        ui: &mut egui::Ui,
        entries: &[Entry],
        new_file: Option<&str>,
        act: &mut Option<Act>,
    ) {
        let gap = GRID_GAP;
        let cols = grid_cols(ui.available_width());
        let card_w = card_width(ui.available_width());
        self.dash.grid_cols = cols;
        let slots = entries.len() + usize::from(new_file.is_some());
        for start in (0..slots).step_by(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for i in start..(start + cols).min(slots) {
                    match entries.get(i) {
                        Some(entry) => self.file_card(ui, entry, card_w, act),
                        None => {
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
    /// — the sidebar button's [`Act::NewFile`] offered where the eye is, only where
    /// the nav names a project to land in.
    ///
    /// The arrows cannot reach it, deliberately: a selection on a verb would make
    /// `Delete` and `Enter` ambiguous; the keyboard's route is `Ctrl+N`. Returns
    /// whether it was clicked.
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
        // A shade under [`color::CARD`] (`text 12%` against `15%`): reads as an empty slot.
        p.rect_filled(
            rect,
            egui::CornerRadius::same(GHOST_R as u8),
            theme::color::text_a(8),
        );
        // `rect_stroke` has no dash pattern, so the dashes follow a polyline of the
        // fill's corners; `rounded_rect_path` repeats the first point to close the last edge.
        let closed = rounded_rect_path(rect.shrink(0.5), GHOST_R, 6);
        let stroke = egui::Stroke::new(
            1.0,
            if lit {
                color::ACCENT_700
            } else {
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

    /// The one chip a card or a row wears, or none, most severe first.
    ///
    /// `Unreadable` has two sources: `Entry::unread`, the scan's cheap answer for a
    /// file whose first 4 KB will not parse, and `Covers::unreadable`, the loader's
    /// verdict on the whole file, which alone catches a document from a newer build.
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
        let opacity = ui.opacity();
        ui.multiply_opacity(self.opening_alpha(entry));
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
        // Three tiers: the top is the keyboard's cursor, a full accent border against
        // hover's muted `ACCENT_700`, since the two can coexist.
        p.rect_stroke(
            rect,
            egui::CornerRadius::same(9),
            egui::Stroke::new(
                1.0,
                match (picked, lit) {
                    (true, _) => color::ACCENT,
                    (false, true) => color::ACCENT_700,
                    (false, false) => color::CARD_BORDER,
                },
            ),
            egui::StrokeKind::Inside,
        );
        // The plate the cover sits on, also shown for a document with no cover yet
        // (`library::cover::Covers::get` does not distinguish the cases).
        let plate_radius = egui::CornerRadius {
            nw: 8,
            ne: 8,
            sw: 0,
            se: 0,
        };
        p.rect_filled(thumb.shrink(1.0), plate_radius, dot.gamma_multiply(0.14));
        // Only while the plate is on screen: the grid is not virtualised, so an
        // unconditional `get` would render the whole library on the first frame. So
        // `Covers::unreadable` is known only for documents the grid has shown.
        let cover = if ui.is_rect_visible(thumb) {
            self.covers.get(ui.ctx(), entry).cloned()
        } else {
            None
        };
        match cover {
            Some(texture) => {
                // Fitted, never filled: bleeding to the edges would show square
                // corners over the rounded plate.
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
        let mut corner = egui::pos2(thumb.left() + 8.0, thumb.bottom() - 8.0);
        if let Some(mark) = self.mark_of(entry) {
            let took = mark_chip(ui, corner, egui::Align2::LEFT_BOTTOM, mark);
            corner.x = took.right() + 5.0;
        }
        // The *Kit* chip shares the corner (§5.3e), after a mark when there is one.
        if marks_kit(entry) {
            kit_chip(ui, corner);
        }
        // A document already in a tab: a click brings that tab forward.
        if self.tab_of_path(&entry.path).is_some() {
            open_chip(
                ui,
                thumb.left_top() + egui::vec2(7.0, 7.0),
                egui::Align2::LEFT_TOP,
            );
        }
        // The star sits on the cover (`top:7px;right:7px`) so a grid can be scanned for
        // it. Outline, as the bundled Phosphor has only Regular; the gold carries the state.
        if self.is_starred(entry) {
            let at = egui::pos2(thumb.right() - 14.0, thumb.top() + 14.0);
            let p = ui.painter();
            // The design's drop shadow, as a second pass one point down.
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
        ui.set_opacity(opacity);
    }

    /// How opaque `entry`'s card or row draws: the open pulse's value while
    /// [`OndinApp::begin_open`] is reading it, and whole otherwise.
    fn opening_alpha(&self, entry: &Entry) -> f32 {
        self.opening
            .as_ref()
            .filter(|o| o.path == entry.path)
            .map_or(1.0, |o| {
                crate::ui::pulse_alpha(o.at.elapsed().as_secs_f32())
            })
    }

    /// What a click on a card or a row means: it opens the document, and a
    /// right-click opens the ⋮ menu at the press. A click does not select and no
    /// card reads a double-click. The keyboard's cursor is separate: arrows move it,
    /// `Enter` opens it, `Delete` asks, `Escape` clears it.
    fn pick_or_open(&mut self, resp: &egui::Response, entry: &Entry, act: &mut Option<Act>) {
        // Above the gate: a second right-click replaces the menu (§0 R4).
        if resp.secondary_clicked() {
            self.dash.menu_for = Some(entry.path.clone());
            self.dash.menu_at = resp.interact_pointer_pos();
            self.dash.moving = None;
            self.dash.filter_open = false;
            self.tabs.menu = None;
            return;
        }
        // A click that dismisses a menu is spent doing so (`context-menus.md` §0 R4), as
        // in `canvas_ui`. Reads the latched `menu_was_up`: `library_menu_open()` is
        // already false for cards drawn after the anchor's.
        if self.dash.menu_was_up {
            return;
        }
        if resp.clicked() && self.can_open() {
            *act = Some(Act::Open(entry.path.clone()));
        }
    }

    /// Whether one of the library's two floating menus is on screen: the ⋮ file
    /// menu and the *All projects ⌄* dropdown (the modals stop the pointer
    /// themselves). [`OndinApp::dashboard_keys`] asks it too, so the keymap and the
    /// pointer-side doors agree about what is on screen.
    ///
    /// # Where `menu_was_up` is read
    ///
    /// Every door a click can reach gates on it, so the click that dismisses a menu
    /// does nothing else: [`OndinApp::pick_or_open`] (file card and row), the dashed
    /// *New file* card and list row, the sidebar's hand-rolled *New file* button,
    /// [`Self::nav_row`], [`Self::project_row`] (both hits), `project_cards`' call of
    /// [`Self::project_card`], the body header's controls and the top bar's search,
    /// settings, update chip, caption buttons and drag strip.
    ///
    /// Not gated: [`Self::file_menu_button`] (the ⋮ anchor both dismissals exempt),
    /// rows inside either menu, and `dashboard_act`, where gating would swallow the ⋮
    /// menu's own rows. The body background dismisses a menu on purpose.
    pub(crate) fn library_menu_open(&self) -> bool {
        self.dash.menu_for.is_some() || self.dash.filter_open
    }

    /// Whether a document in the list on screen may be opened: false in the trash,
    /// where opening would put the editor on a file the purge will delete. Read by
    /// the card click and by `Enter`.
    fn can_open(&self) -> bool {
        !self.in_trash()
    }

    /// Whether the list on screen is the trash — the one fact behind
    /// [`OndinApp::can_open`], [`OndinApp::file_menu_popup`]'s two-row menu and the
    /// keymap's `F2` and `Ctrl+D`.
    fn in_trash(&self) -> bool {
        self.dash.nav == Nav::Trash
    }

    /// Bring the card or row the keyboard just moved onto into view.
    ///
    /// Called by the card, not the keymap: the scroll needs a rect that exists only
    /// once the card is laid out, and clearing the flag here stops later frames
    /// fighting a scrollbar drag. No alignment and
    /// [`egui::style::ScrollAnimation::none`], so a held arrow does not chase an animation.
    fn follow_selection(&mut self, ui: &egui::Ui, picked: bool, rect: egui::Rect) {
        if picked && self.dash.scroll_to_selected {
            ui.scroll_to_rect_animation(rect, None, egui::style::ScrollAnimation::none());
            self.dash.scroll_to_selected = false;
        }
    }

    /// Whether the library screen's own keys may be read this frame — nothing on
    /// screen owns the keyboard.
    ///
    /// One predicate for the two sites that read `egui::Context` directly,
    /// [`OndinApp::dashboard_keys`] and [`OndinApp::search_field`], which no modal
    /// can intercept. `egui_wants_keyboard_input` is deliberately absent: it asks
    /// about a live text cursor, and `search_field` reads its chord before drawing
    /// its own `TextEdit`.
    pub(crate) fn library_keys_are_free(&self) -> bool {
        // The recovery card is drawn by `OndinApp::ui` above the view branch, so
        // this file lists it by name.
        self.recovery.pending.is_empty()
            && self.dash.moving.is_none()
            && self.dash.deleting.is_none()
            && self.dash.deleting_project.is_none()
            && self.dash.new_project.is_none()
            && self.dash.edit_project.is_none()
            && self.library_settings.is_none()
            && self.rename_entry.is_none()
            && self.unsharing_kit.is_none()
            && self.cheatsheet.is_none()
            && self.tabs.menu.is_none()
    }

    /// The library's own keymap — arrows, `Enter`, `Delete`, `Escape`, `F2`,
    /// `Ctrl+R`, `Ctrl+N` and `Ctrl+D` (`shortcuts.md` §8a).
    ///
    /// Read straight off the context, as `input::resolve` does not run while the
    /// library is up. The guards are the things that own the keyboard: the floating
    /// menus ([`OndinApp::library_menu_open`], which also handle `Escape`), then
    /// [`OndinApp::library_keys_are_free`] and `egui_wants_keyboard_input`.
    /// `search.is_some()` is not redundant: on the frame the overlay closes, the
    /// field is never drawn and egui has already dropped focus. `modifiers.is_none()`
    /// is the last gate, so the modified chords are read above it.
    fn dashboard_keys(&mut self, ctx: &egui::Context, entries: &[Entry], act: &mut Option<Act>) {
        // The ⋮ popup and the filter dropdown are `Area`s, not modals, so nothing else
        // gives them an `Escape`. Handled before the guard list.
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
        // The editor's chord, read here because `input::resolve` is not.
        if crate::input::cheatsheet_chord(ctx) {
            self.open_cheatsheet(crate::cheatsheet::Opened::Library);
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
                    // Backspace as well as Delete, the key a Mac keyboard has.
                    i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                    i.key_pressed(egui::Key::Escape),
                    i.key_pressed(egui::Key::F2),
                    i.key_pressed(egui::Key::N),
                    i.key_pressed(egui::Key::D),
                    // `Ctrl+R`, the editor's other rename; `F2` is read on the plain side.
                    i.key_pressed(egui::Key::R),
                    i.modifiers.is_none(),
                    // Spelled as `input::normal_mode` spells it, so `Ctrl+Alt+N` and
                    // `Ctrl+Shift+D` stay unbound here as there.
                    i.modifiers.command && !i.modifiers.alt && !i.modifiers.shift,
                )
            });
        // The chords come before `plain` closes the door on everything modified.
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
            // The index is found by path every frame: re-scans make a stored one stale.
            let at = self
                .dash
                .selected
                .as_ref()
                .and_then(|p| entries.iter().position(|e| &e.path == p));
            if let Some(to) = arrow_target(
                at,
                key,
                self.dash.list_view,
                self.dash.grid_cols,
                entries.len(),
            ) {
                self.dash.selected = Some(entries[to].path.clone());
                self.dash.scroll_to_selected = true;
            }
            return;
        }
        if escape {
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
            // Through the confirmation: a key can be the tail of a mistyped chord.
            self.dash.deleting = Some(path);
        }
    }

    /// Open the rename field over the keyboard's selection (`F2`, `Ctrl+R`).
    ///
    /// The name comes from `entries` like the ⋮'s *Rename*: `Entry::display_name`
    /// drops the conflict suffix a sync client may have added. Nothing happens with
    /// no selection, in the trash ([`OndinApp::in_trash`]), or when the selected
    /// path is no longer in `entries`.
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
        // The row says only *New file*, where the card says *New file in X*, as the design has it.
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
        // The design's 20pt plate under the glyph keeps the left edge in line with the names.
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
        let opacity = ui.opacity();
        ui.multiply_opacity(self.opening_alpha(entry));
        let lit = resp.hovered();
        let picked = self.dash.selected.as_deref() == Some(entry.path.as_path());
        self.follow_selection(ui, picked, rect);
        // A wash rather than a border, the design's answer for a row (`rowBg: accent 12%`).
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
        // The chips take their width out of the *Name* column before the name is elided.
        let chip = self.mark_of(entry).filter(|_| !renaming);
        let open = !renaming && self.tab_of_path(&entry.path).is_some();
        let chip_room = chip.map_or(0.0, |m| mark_chip_size(ui, m).x + 8.0)
            + if open {
                open_chip_size(ui).x + 8.0
            } else {
                0.0
            };
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
            if i == 0 && (chip.is_some() || open) {
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
                let mut at = egui::pos2(x + 18.0 + name_w, rect.center().y);
                if let Some(mark) = chip {
                    at.x = mark_chip(ui, at, egui::Align2::LEFT_CENTER, mark).right() + 8.0;
                }
                if open {
                    open_chip(ui, at, egui::Align2::LEFT_CENTER);
                }
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
        ui.set_opacity(opacity);
    }

    /// Draw the inline rename field, if this is the document being renamed.
    ///
    /// Returns whether it drew, so the caller knows to skip painting the name.
    ///
    /// Commits on `lost_focus`, not only Enter, so clicking elsewhere keeps the typed
    /// name; Escape clears the buffer before the focus loss can be read.
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
        // The app's field ground, painted here rather than left to egui's focus ring
        // (`ui::text_field` does not fit a reserved rect). `color::FIELD_BORDER`, not
        // the accent: the field is only on screen while focused.
        let p = ui.painter();
        p.rect_filled(rect, egui::CornerRadius::same(ui::BUTTON_R), color::FIELD);
        p.rect_stroke(
            rect,
            egui::CornerRadius::same(ui::BUTTON_R),
            egui::Stroke::new(1.0, color::FIELD_BORDER),
            egui::StrokeKind::Inside,
        );
        // The inset is 4 on the frame, where the painted name sits 4pt right of the box
        // in both callers. One row tall and centred, for [`search_field`]'s reason.
        let font = egui::FontId::proportional(size);
        let row = ui.ctx().fonts_mut(|f| f.row_height(&font));
        let line = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), row));
        // `new_child`, not `Ui::put`, which advances the parent's cursor to this rect's
        // right edge and lays the next card on top of this one.
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
        // Once, through the latch: requesting focus every pass re-takes the caret in the
        // frame that surrendered it, so `lost_focus()` would never be true.
        if !*focused {
            resp.request_focus();
            *focused = true;
        }
        if resp.lost_focus() {
            *act = Some(Act::CommitRename);
        }
        true
    }

    /// The ⋮ and the menu behind it — the one place every per-document action lives.
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
            // Open from a right-click, the dots move the menu under them.
            let shut = open && self.dash.menu_at.is_none();
            self.dash.menu_for = (!shut).then(|| entry.path.clone());
            self.dash.menu_at = None;
            self.dash.moving = None;
        }
        if self.dash.menu_for.as_ref() == Some(&entry.path) {
            let anchor = match self.dash.menu_at {
                Some(at) => egui::Rect::from_min_size(at, egui::Vec2::ZERO),
                None => hit,
            };
            self.file_menu_popup(ui, entry, anchor, act);
        }
    }

    /// The file menu's kit row: *Make a kit* or *Stop sharing as kit*, the same
    /// toggle as the document menu's. None on a conflict copy, which carries the
    /// original's id; one left as its kit's only file is the kit and gets the row.
    fn kit_row(&self, entry: &Entry) -> Option<(&'static str, Act)> {
        if !stands_for_its_kit(&self.library, entry) {
            return None;
        }
        Some(if is_kit(&self.library, entry) {
            (
                "Stop sharing as kit",
                Act::SetKit(entry.path.clone(), false),
            )
        } else {
            ("Make a kit", Act::SetKit(entry.path.clone(), true))
        })
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
            // A trashed document gets two rows and neither is *Open* (`can_open`).
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
        let mut rows = rows;
        if !trashed && let Some(row) = self.kit_row(entry) {
            rows.insert(rows.len() - 1, row);
        }

        // Narrower than the filter's `FILTER_MENU_W`: these are verbs, those project names.
        let w = 170.0;
        let out = anchored_menu(
            ui,
            egui::Id::new(("file-menu-popup", &entry.path)),
            anchor,
            w,
            rows.len(),
            |p, i, row| {
                let label = rows[i].0;
                // *Delete* alone is not the ordinary ink: it cannot be undone.
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
            let action = rows.into_iter().nth(i).expect("index from the same list").1;
            self.dash.menu_for = None;
            if let Some(a) = self.file_menu_chose(action, trashed) {
                *act = Some(a);
            }
        } else if out.dismissed {
            self.dash.menu_for = None;
        }
    }

    /// What a chosen ⋯ row does: a row that opens a card sets its state and
    /// answers `None`; any other becomes the frame's act.
    ///
    /// A row that opens a card gives up a file still loading too, as
    /// [`Self::dashboard_act`] does for the rest, or the load would land under the card.
    fn file_menu_chose(&mut self, action: Act, trashed: bool) -> Option<Act> {
        if matches!(action, Act::MoveTo(_, None) | Act::SetKit(_, false))
            || matches!(action, Act::Trash(_) if !trashed)
        {
            self.opening = None;
        }
        match action {
            // These open a card, so they set state instead of becoming an act.
            Act::MoveTo(path, None) => self.dash.moving = Some(path),
            Act::Trash(path) if !trashed => self.dash.deleting = Some(path),
            Act::SetKit(path, false) => self.unsharing_kit = Some(Some(path)),
            other => return Some(other),
        }
        None
    }

    /// Apply what the frame decided. See [`Act`] for why this is deferred.
    fn dashboard_act(&mut self, act: Act) {
        let root = self.library.root.clone();
        // Any other act gives up a file still loading, or a trash, rename or move
        // would see it adopted at its old path.
        if !matches!(act, Act::Open(_)) {
            self.opening = None;
        }
        match act {
            Act::Open(path) => self.begin_open(&path),
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
                            // Stamped as touched: *Recent* lists documents this machine
                            // has opened, so the copy would be invisible there.
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
                            // The open document follows its file and its
                            // `meta.project`, or the next autosave recreates it.
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
                        // Unreachable today, since `trash_or_purge` clears `session.path`;
                        // written so this arm cannot become the rename bug if that stops.
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
            Act::SetKit(path, sharing) => self.set_kit_at(&path, sharing),
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
    /// The files go first and the project row last, and only when every member moved:
    /// a partial failure leaves the project standing with what could not be moved,
    /// and pressing *Delete project* again retries exactly those.
    pub(crate) fn delete_project(&mut self, id: &str, transfer_to: Option<&str>, keep_files: bool) {
        // Before a single file moves: a later check would protect the bytes, not the files.
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
            // The open document follows its file, as a single move does; a transfer
            // must also re-point the session's path and project id.
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
                    Ok(_) => self.open_file_trashed(&entry.path),
                    Err(_) => failed += 1,
                }
            }
        }
        // Only when every member moved. `save_projects` runs either way, so an
        // unwritable `projects.json` is reported before the retry.
        if failed == 0 {
            self.library.projects.projects.retain(|p| p.id != id);
        }
        if !self.library.save_projects() {
            self.session.fail("Could not write projects.json");
        }
        self.library.reload_projects();
        self.library.refresh();
        // Gated on the removal: a reset on failure would take the user off the heading
        // that holds the files to retry.
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

    /// Copy `.ondin` documents from outside the library into it, through an open
    /// dialog. A copy, not a move: the originals are somebody else's files. It keeps
    /// its name and creation date and gets a fresh id, as in `store::duplicate`.
    fn import_documents(&mut self) {
        // Before the dialog, so a picked answer is never thrown away.
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
    /// files them. Only files with a `path` count: [`Self::import_paths`] takes paths
    /// all the way down, so a bytes-only drop (from a browser) is "other". Everything
    /// that is not a document is reported in one message.
    fn take_dropped_documents(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        // After the emptiness check, or the refusal fires every frame while the folder is missing.
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
        // One message, composed here: the status is a single slot.
        if docs.is_empty() {
            self.session
                .fail("The library takes .ondin documents — open one to place images in it");
            return;
        }
        let (ok, failed, already) = self.import_counting(&docs);
        self.session
            .info(import_summary(ok, failed, others, already));
    }

    /// The half of [`Self::import_documents`] that is not the dialog, split out so
    /// tests can drive it: [`Self::import_counting`] plus the announcement.
    pub(crate) fn import_paths(&mut self, files: &[PathBuf]) {
        let (ok, failed, already) = self.import_counting(files);
        self.session.info(import_summary(ok, failed, 0, already));
    }

    /// [`Self::import_paths`] without the announcement, for the caller that adds its
    /// own count to it (the status is a single slot; see [`import_summary`]).
    pub(crate) fn import_counting(&mut self, files: &[PathBuf]) -> (usize, usize, usize) {
        // Into the project being viewed. `destination`, not `get`: an archived project
        // is selectable, and a refused destination files as unfiled.
        let project = match &self.dash.nav {
            Nav::Project(id) => self.library.projects.destination(id).cloned(),
            _ => None,
        };
        let root = self.library.root.clone();
        let (mut ok, mut failed, mut already) = (0usize, 0usize, 0usize);
        for path in files {
            // Files already in the library (the dialog may open on the base folder) are
            // skipped and counted for [`import_summary`].
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
                    // No name block: de-slug the filename as the list does.
                    crate::library::scan::name_from_stem(
                        path.file_stem().and_then(|s| s.to_str()).unwrap_or(""),
                    )
                });
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

    /// Whether a document is starred here. Per-machine, in the cache: a star in the
    /// document's metadata would rewrite the whole `.ondin` file and trigger a sync
    /// upload.
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
            // Read before the file goes: its id names the kept undo history.
            let id = crate::library::scan::read_meta(path).and_then(|(meta, _)| meta.id);
            match std::fs::remove_file(path) {
                Ok(()) => {
                    if let Some(id) = id {
                        crate::library::history::remove(&root, &id);
                    }
                }
                Err(e) => self.session.fail(format!("Delete failed: {e}")),
            }
            // The permanent delete bypasses `store` and the trash listing is cached, so it
            // must announce itself. Unconditional, and not a full `refresh`: nothing
            // outside `.trash` moved.
            self.library.refresh_trash();
            self.dash.deleting = None;
            return;
        }
        let Some(entry) = self.library.entry_at(path).cloned() else {
            return;
        };
        match store::trash(&root, &entry) {
            Ok(_) => {
                // The next autosave would otherwise write the open document back out
                // of the trash.
                self.open_file_trashed(path);
                self.library.refresh();
            }
            Err(e) => self.session.fail(format!("Delete failed: {e}")),
        }
        self.dash.deleting = None;
    }

    /// Create a document, filed immediately, and open it.
    ///
    /// The destination is an argument so one function answers four doors: the sidebar
    /// button, the dashed card, `Ctrl+N` here and in the editor
    /// ([`crate::input::Action::NewDocument`]); see [`OndinApp::nav_project`] and
    /// [`OndinApp::open_document_project`].
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
    /// *New file*, *New project* and an import ask, as each ends in a `create_dir_all`
    /// that would put an empty folder where the offline library belongs
    /// (`library::state::Library::may_write`). A message, not a disabled button.
    fn library_writable(&mut self) -> bool {
        if self.library.may_write() {
            return true;
        }
        self.session
            .fail("The library folder isn't available. Check the base folder in Settings.");
        false
    }

    /// The project in front of you, as the dashboard spells it: the nav being
    /// viewed. `None` (unfiled) for *Recent*, *All files*, *Starred* and the trash.
    ///
    /// `destination`, not `get`: the same gate as [`Self::new_file_into`], or the card
    /// and the act it fires disagree; an archived project answers *no project*.
    pub(crate) fn nav_project(&self) -> Option<project::Project> {
        match &self.dash.nav {
            Nav::Project(id) => self.library.projects.destination(id).cloned(),
            _ => None,
        }
    }

    /// The same rule as [`OndinApp::nav_project`], as the editor spells it: the open
    /// document's own project. Reading [`Nav`] from the editor would file into
    /// whichever nav the dashboard was left on. Neither reads `OndinApp::view`, which
    /// starts as [`crate::app::View::Editor`] under `OndinApp::headless`. `None` for
    /// a document not in the library — the never-filed starter the app opens on.
    pub(crate) fn open_document_project(&self) -> Option<project::Project> {
        self.session
            .path
            .as_deref()
            .and_then(|p| self.library.entry_at(p))
            .and_then(|e| self.library.project_of(e))
            .cloned()
    }

    /// An open document follows a library file that has just moved — its path and
    /// the metadata the move wrote to disk, together: re-pointing the path alone
    /// would let the next save write the old `meta.name` or `meta.project` back. A
    /// tab that only names the file follows by its path. A no-op when no tab holds it.
    fn open_document_follows(
        &mut self,
        was: &Path,
        now: PathBuf,
        patch: impl FnOnce(&mut ondin_core::meta::DocumentMeta),
    ) {
        for path in self.unloaded_paths() {
            if path == was {
                *path = now.clone();
            }
        }
        let Some(session) = self
            .open_sessions()
            .into_iter()
            .find(|s| s.path.as_deref() == Some(was))
        else {
            return;
        };
        session.path = Some(now);
        let mut meta = session.doc.meta().clone();
        patch(&mut meta);
        session.doc.set_meta(meta);
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
        match store::rename(&self.library.root, &entry, name) {
            Ok(moved) => {
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
        // After the `take` above, so a refusal closes the card.
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
    /// The folder moves before `projects.json` is written, and the stem recorded is
    /// the one the move produced (the slug may step around a collision). A failed
    /// rename is reported and the project keeps its folder; the name change stands.
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
            return;
        };
        // Nothing edited, nothing written: a write rewrites `projects.json` and rescans.
        // The footer dims the button with the same `differs_from`; this also refuses
        // when Enter dismissed the card.
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
        if let (Some(old), Some(new)) = (before.folder.as_deref(), folder.as_deref())
            && old != new
        {
            let (from, to) = (root.join(old), root.join(new));
            let rebase = |path: &mut PathBuf| {
                if let Ok(rest) = path.strip_prefix(&from) {
                    *path = to.join(rest);
                }
            };
            for session in self.open_sessions() {
                if let Some(path) = session.path.as_mut() {
                    rebase(path);
                }
            }
            for path in self.unloaded_paths() {
                rebase(path);
            }
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
        // A folder rename leaves every entry holding a stale path.
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
    /// archived: the dropdown lists only active projects and
    /// [`OndinApp::filter_applies`] may hide the control, so it could not be unset.
    fn forget_archived_filter(&mut self, id: &str, archived: bool) {
        if archived && self.dash.filter.as_deref() == Some(id) {
            self.dash.filter = None;
        }
    }

    // The `dashboard_*` preferences are written only by `apply_library_settings`.

    fn new_project_modal(&mut self, ctx: &egui::Context, act: &mut Option<Act>) {
        if self.dash.new_project.is_none() {
            return;
        }
        let mut cancel = false;
        let mut create = false;
        let modal = settings::card("new-project", ctx, |ui| {
            ui.set_width(ui::menu_inner_w(settings::CARD_W, settings::PAD));
            // Zero, so every gap is written out (`settings::SECTION_AIR`).
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
                "Your awesome project",
                12.5,
            );
            // Once, when the modal opens: a per-frame `request_focus` would keep the caret
            // from ever leaving. Not guarded on `Memory::focused()` being empty, as
            // something behind the backdrop may still hold the caret on the opening frame.
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

            match settings::modal_footer(ui, "Create project", !np.name.trim().is_empty()) {
                Some(settings::Decision::Save) => create = true,
                Some(settings::Decision::Cancel) => cancel = true,
                None => {}
            }
        });
        // Escape and a backdrop click both cancel; `should_close` makes Escape answer
        // only the topmost modal. Read after the body, because it consumes the press.
        if modal.should_close() {
            cancel = true;
        }
        // Enter needs a name, as the disabled button does, or it would close the
        // card and silently drop the colour and folder chosen.
        if ctx.input(|i| i.key_pressed(egui::Key::Enter))
            && self
                .dash
                .new_project
                .as_ref()
                .is_some_and(|np| !np.name.trim().is_empty())
        {
            create = true;
        }
        if cancel {
            self.dash.new_project = None;
        } else if create {
            *act = Some(Act::CreateProject);
        }
    }

    /// *Edit project* — the New project card with two rows swapped.
    ///
    /// Spelled out again rather than sharing [`OndinApp::new_project_modal`], which
    /// would branch on a flag three times. There is no *Create folder* row: the
    /// folder is decided at creation, and a rename takes it along
    /// (`library::project::rename_folder`).
    fn edit_project_modal(&mut self, ctx: &egui::Context, act: &mut Option<Act>) {
        if self.dash.edit_project.is_none() {
            return;
        }
        let mut cancel = false;
        let mut save = false;
        // The project as stored, cloned before the closure, which borrows `self` mutably.
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
            // The latch, for `NewProject::name_focused`'s reason.
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
                    // Matched on the value, not an index (see [`EditProject::color`]).
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

            // Lit only when there is something to save; a vanished project dims it.
            let enabled = !ep.name.trim().is_empty()
                && stored
                    .as_ref()
                    .is_some_and(|before| ep.differs_from(before));
            match settings::modal_footer(ui, "Save changes", enabled) {
                Some(settings::Decision::Save) => save = true,
                Some(settings::Decision::Cancel) => cancel = true,
                None => {}
            }
        });
        if modal.should_close() {
            cancel = true;
        }
        // Enter needs a name, or it would close the card and silently drop the edits.
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
    /// A project has no trash: deleting one removes a row from `projects.json`, so
    /// this modal is the whole confirmation.
    fn delete_project_modal(&mut self, ctx: &egui::Context, act: &mut Option<Act>) {
        let Some(dp) = self.dash.deleting_project.as_ref() else {
            return;
        };
        let id = dp.id.clone();
        let Some(project) = self.library.projects.get(&id).cloned() else {
            // Vanished under us (another window or a synced `projects.json`).
            self.dash.deleting_project = None;
            return;
        };
        let count = self.library.file_count(&id);
        // `active`, for `move_modal`'s reason: an archived project is not a destination.
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
                // The inverse of `keep_files` (see [`DeleteProject`]). The row does not say
                // the files go to the trash, which would soften the loudest control.
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

            // The folder is kept and the card does not say so: a confirmation spends
            // its words on what it destroys (see `library::project`).

            match settings::danger_footer(ui, "Delete project") {
                Some(settings::Decision::Save) => confirm = true,
                Some(settings::Decision::Cancel) => cancel = true,
                None => {}
            }
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

    /// *Move to project* — the projects plus *No project*, a modal rather than a submenu.
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
        // Archived projects are not destinations, except the one this document is
        // already in, or the sheet would show it as unfiled.
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
            if settings::cancel_footer(ui) {
                cancel = true;
            }
        });
        // Escape and the backdrop, both Cancel — see `new_project_modal`.
        if modal.should_close() {
            cancel = true;
        }
        if cancel {
            self.dash.moving = None;
        } else if let Some(target) = chosen {
            // Clicking the project it is already in still moves it, a no-op in
            // `store::move_to_project`.
            *act = Some(Act::MoveTo(path, target));
        }
    }

    /// The library's own Settings — storage, and how the dashboard opens.
    ///
    /// A draft that applies on Save: a base folder plus *Move my files there* is an
    /// operation on every document, which must not start while the path is half
    /// typed. Built from the editor's Settings kit ([`settings::card_modal`],
    /// [`settings::section`], [`settings::modal_footer`]).
    fn library_settings_modal(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.library_settings.clone() else {
            return;
        };
        let saved = LibrarySettings::from_prefs(&self.prefs, &self.library.root);
        let resolved_now = self.library.root.display().to_string();
        let mut decision: Option<settings::Decision> = None;
        // Cloned out and answered after the card: a retry runs a whole migration, which
        // must not happen inside a paint.
        let stranded = self.stranded.clone();
        let mut retry = false;

        let modal = settings::card("library-settings", ctx, |ui| {
            ui.set_width(ui::menu_inner_w(settings::CARD_W, settings::PAD));
            // Zero, so every vertical gap is written out (`settings::SECTION_AIR`).
            ui.spacing_mut().item_spacing.y = 0.0;

            if settings::modal_title(ui, "Settings") {
                decision = Some(settings::Decision::Cancel);
            }

            ui.add_space(settings::TITLE_GAP - settings::SECTION_AIR);
            settings::section(ui, "Storage");
            field_label(ui, "Base folder");
            // The button is allocated first, in a right-to-left row, so the field takes
            // the rest (`picker_row`'s shape). Disabled while the typed path is no folder.
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
            // The migrate switch exists only once the path has changed; its caption
            // changes with it so *off* is stated.
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

            // What the last migration left behind: named files and the folder, which a
            // status line cannot hold. Below the switch, which is about the next move.
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
                // Capped ([`STRANDED_ROWS`]) and the remainder counted, not scrolled.
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
            // The label sits inside the field in a two-column row, as `settings::nudge_field`
            // does; *Base folder* keeps a caption because a path has no width for a prefix.
            let half = egui::vec2(
                (ui.available_width() - settings::COL_GAP) / 2.0,
                settings::FIELD_H,
            );
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = settings::COL_GAP;
                whole_field(ui, half, "Autosave", "s", &mut d.autosave_secs, 3600);
                whole_field(ui, half, "Trash", "d", &mut d.trash_keep_days, 365);
            });
            // Zero is a real setting in both fields and means something alarming, so
            // it is said out loud.
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
            ui.add_space(settings::ROW_GAP);
            whole_field(ui, half, "Undo history", "steps", &mut d.history_size, 1000);
            if d.history_size == 0 {
                warn_caption(ui, "Undo history is cleared when a document is closed.");
            }

            settings::section(ui, "Library");
            picker_row(
                ui,
                "Default page on open",
                "default-page",
                d.default_page.label(),
                |ui| {
                    // Only the three that always exist: not *Trash*, a project (it can be
                    // deleted between launches, `Nav::from_id`) or *Kits*.
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

            if let Some(x) = settings::modal_footer(ui, "Save changes", d != saved) {
                decision = Some(x);
            }
        });

        // The backdrop and Escape both cancel; read after the body, which they consume.
        if modal.should_close() {
            decision = Some(settings::Decision::Cancel);
        }

        match decision {
            Some(settings::Decision::Cancel) => self.library_settings = None,
            Some(settings::Decision::Save) => {
                self.library_settings = Some(d);
                self.apply_library_settings();
            }
            // Still open: keep whatever was typed into it this frame.
            None => self.library_settings = Some(d),
        }
        // After the match, so a retry cannot race a Save that re-points the library.
        // Cancel leaves the report standing.
        if retry {
            self.retry_migration();
        }
    }

    /// Re-point the open document, and *reopen last*, at where a migration put them.
    ///
    /// Without this `session.path` still names the abandoned folder and the next
    /// save forks the document under one `meta.id`. The path is looked up in the
    /// map, not re-joined under the new root, as `relocate` renames into a
    /// collision. `prefs.last_document` travels with it. Shared by the retry.
    fn follow_moved_documents(&mut self, moved: &crate::library::relocate::Moved) {
        let follow = |path: &mut PathBuf| {
            if let Some(now) = moved.destination_of(path) {
                *path = now.to_path_buf();
            }
        };
        for session in self.open_sessions() {
            if let Some(path) = session.path.as_mut() {
                follow(path);
            }
        }
        for path in self.unloaded_paths() {
            follow(path);
        }
        if let Some(path) = self.prefs.last_document.as_mut() {
            follow(path);
        }
    }

    /// Run the stranded half of the last migration again.
    ///
    /// Not idempotent and need not be: a successful move deletes its source, so the
    /// walk meets only what was left behind. It owes `follow_moved_documents`, a
    /// `refresh` and the same `disk_settle` as the first run.
    ///
    /// Three refusals, each keeping the report: the report is answerable only against
    /// the library it was made for; an unreachable old folder is not an empty one
    /// (`relocate` says "Nothing to move" for a missing source); and an unreachable
    /// library folder would make `relocate` record the root itself as the one
    /// failure and resurrect an offline library (`Library::may_write`).
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
        if !crate::library::scan::readable(&self.library.root) {
            self.session.fail(format!(
                "Could not reach the library folder {} — nothing was moved, and the list is kept.",
                self.library.root.display()
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
        self.prefs.history_size = d.history_size;
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
        // The folder is made first and its failure refuses the change: a path naming an
        // existing file must be adopted neither by the migration nor by `prefs.base_folder`.
        let creatable = new_root == self.library.root || std::fs::create_dir_all(&new_root).is_ok();
        if !creatable {
            // Only the base folder is refused; the other settings were applied above.
            self.session.fail(format!(
                "Could not use {} as the library folder — it is a file, or could not be created. \
                 The library folder is unchanged.",
                new_root.display()
            ));
        } else if new_root != self.library.root {
            let old_root = self.library.root.clone();
            self.prefs.base_folder = requested;
            // `clear` ends the cover renderer, a second worker aimed at the old root, so
            // it runs before the migration and cancels rather than waits: a job queued
            // before the move would cache `Unreadable` for a healthy moved document.
            self.covers.clear();
            if d.migrate {
                // Drain the snapshot writer first: `relocate` carries `.recovery/` across,
                // and a write in flight to the old root would be copied then deleted.
                self.disk_settle();
                let moved = crate::library::relocate::relocate(&old_root, &new_root);
                self.follow_moved_documents(&moved);
                self.session.info(moved.summary());
                // Kept as a list, which the status line cannot hold; assigned both ways
                // so a clean migration clears a previous report.
                self.stranded = (!moved.failed.is_empty()).then(|| Stranded {
                    from: old_root.clone(),
                    to: new_root.clone(),
                    failed: moved.failed.clone(),
                });
            } else {
                // A change that does not migrate clears the report too, or *Try again*
                // would relocate into a folder that is no longer the library.
                self.stranded = None;
            }
            // The folder is created because `Library::root_unavailable` is "could not be
            // listed and this machine has documents recorded", so a new root that does
            // not exist yet would be a library the app refuses to write into.
            //
            // Re-opened rather than refreshed: `Library` caches its root. The per-machine
            // index is handed across, bar the exceptions `Library::open_with_index` names.
            let local = std::mem::take(&mut self.library.local);
            self.library = crate::library::state::Library::open_with_index(new_root, local);
            // A nav naming a project of the old library names nothing in the new one.
            self.dash.nav = Nav::from_id(&self.prefs.dashboard_page);
        }
        self.prefs.save();
        // The retention window may have just shrunk; sweeping applies it now.
        crate::library::store::purge_trash(&self.library.root, self.prefs.trash_keep_days);
        self.library.refresh_trash();
    }

    fn delete_modal(&mut self, ctx: &egui::Context, act: &mut Option<Act>) {
        let Some(path) = self.dash.deleting.clone() else {
            return;
        };
        // In the trash the fallback is the only branch: `entry_at` searches the library,
        // and re-reading `.trash` is not allowed inside a layout.
        let name = self
            .library
            .entry_at(&path)
            .map(|e| e.display_name())
            .or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|stem| crate::library::scan::display_name(&Default::default(), stem))
            })
            .unwrap_or_else(|| "this file".into());
        let mut decision: Option<bool> = None;
        // In the trash `Act::Trash` deletes outright, so the copy must not say "moves to Trash".
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
            match settings::danger_footer(ui, "Delete") {
                Some(settings::Decision::Save) => decision = Some(true),
                Some(settings::Decision::Cancel) => decision = Some(false),
                None => {}
            }
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
/// Linear order: the grid wraps at the ends of its rows. Out of range clamps, as
/// Explorer does, so `Down` toward a short last row lands on its last card; `Down`
/// on the last row and `Up` on the first still return `Some`, which is why the
/// caller sets rather than toggles. `Left`/`Right` are dead in the list view.
fn arrow_target(
    current: Option<usize>,
    key: egui::Key,
    list_view: bool,
    grid_cols: usize,
    len: usize,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    // The caller passes the grid's column count; `0` is one.
    let cols = if list_view { 1 } else { grid_cols.max(1) } as isize;
    let step = match key {
        egui::Key::ArrowLeft if !list_view => -1,
        egui::Key::ArrowRight if !list_view => 1,
        egui::Key::ArrowUp => -cols,
        egui::Key::ArrowDown => cols,
        _ => return None,
    };
    let Some(at) = current else {
        // Nothing selected: a downward or rightward key takes the first item, an
        // upward or leftward one the last.
        return Some(if step < 0 { len - 1 } else { 0 });
    };
    Some((at as isize + step).clamp(0, len as isize - 1) as usize)
}

/// The small grey word above a field — *Project name*, *Base folder*, *Autosave*.
///
/// A caption over the field, unlike the nudge pair, whose two-character labels fit a
/// [`ui::value_field`] prefix. Allocated at exactly the galley's height for
/// `settings::section`'s reason: a `ui.horizontal` would add `interact_size.y` slack.
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
/// The combo is allocated before the label (a `ui.horizontal` centres each item
/// against the height known when it is placed) and owes the usual two corrections:
/// [`ui::combo_chevron`] for egui's triangle and `interact_size.y` forced to the
/// field height.
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

/// A Settings field holding a whole count of something — seconds of autosave, days
/// of trash retention. Rounded and clamped here as well as by the scrub, which
/// settles a drag and leaves typing alone (`settings::nudge_field`'s reason). Zero
/// is allowed: it means *off*.
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

/// A [`settings::caption`] in the warning amber ([`theme::color::WARN`]) — for a
/// setting that is valid and has a consequence worth naming.
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
/// One function because the status is a single slot, so the dialog's import and the
/// drop's cannot each say half. `skipped` (non-documents, always 0 from the dialog)
/// and `already` (files already in the library) are reported even when everything
/// else went well, and the `(0, 0)` arm states only what it knows.
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
/// The whole row is the control; it is not a [`section_label`] with additions, since
/// that one is inert.
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

/// The circumradius of the star a starred card wears.
const STAR_R: f32 = 7.5;
/// How far in the valleys between a star's arms sit, as a fraction of [`STAR_R`];
/// near 0.35 the arms are needles, past 0.6 the shape is a pentagon with dents.
const STAR_INNER: f32 = 0.5;
/// The fillet at every corner, as a fraction of [`STAR_R`]; see [`star_shape`].
const STAR_ROUND: f32 = 0.16;

/// The *New file in {project}* card's corner radius, shared by the fill and the
/// dashed outline so the ground does not curve away under a square outline.
const GHOST_R: f32 = 9.0;

/// Where each of a project mosaic's `n` cells goes inside `rect`.
///
/// The first cell takes a column at full height; the rest fill the following
/// columns two at a time, top then bottom; a column holding only one cell runs full
/// height. All columns are equal width.
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
/// `per_corner` is how many segments each quarter-turn is sampled at; the first point
/// is repeated at the end to close it. `Shape::dashed_line` carries its dash phase
/// across the whole polyline, which this relies on.
fn rounded_rect_path(rect: egui::Rect, r: f32, per_corner: usize) -> Vec<egui::Pos2> {
    use std::f32::consts::{FRAC_PI_2, PI};
    let r = r.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let mut pts = Vec::with_capacity(per_corner * 4 + 6);
    // Clockwise from just past the top-left corner so the dash phase never doubles back.
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
    pts.push(pts[0]);
    pts
}

/// A filled five-pointed star, as a polygon.
///
/// Drawn because the bundled Phosphor face is Regular only (no filled star); if a
/// fill face is ever bundled, this should go. [`STAR_INNER`] sets the arm fatness and
/// [`STAR_ROUND`] fillets every vertex so no needle remains at 15pt.
fn star_shape(centre: egui::Pos2, r: f32, fill: egui::Color32) -> egui::Shape {
    const POINTS: usize = 5;
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
        // The fillet is capped at half the shorter edge, or the outline crosses itself.
        let (a, b) = (prev - here, next - here);
        let cut = (r * STAR_ROUND).min(a.length() / 2.0).min(b.length() / 2.0);
        let (from, to) = (here + a.normalized() * cut, here + b.normalized() * cut);
        // Three points along a quadratic through the corner; the mid one avoids a bevel.
        for t in [0.0, 0.5, 1.0] {
            let u = 1.0 - t;
            pts.push(
                ((from.to_vec2() * u + here.to_vec2() * t) * u
                    + (here.to_vec2() * u + to.to_vec2() * t) * t)
                    .to_pos2(),
            );
        }
    }
    // Not convex, but star-shaped about its centre, which is all the triangle fan needs.
    egui::Shape::convex_polygon(pts, fill, egui::Stroke::NONE)
}

/// *Delete project* — an icon button; its click removes something with no trash.
///
/// The tooltip is attached here so a caller cannot drop it: it is the only thing
/// saying *project* rather than *file*. The face is [`ui::button_face`]'s, with a
/// [`color::DANGER`] hover wash between face and glyph, rounded to [`ui::BUTTON_R`].
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

/// The pill a [`Mark`] wears on a card or row, drawn rather than a tooltip so it is
/// visible while scanning. Returns the rect it took, for laying the row out after it.
fn mark_chip(ui: &egui::Ui, at: egui::Pos2, align: egui::Align2, mark: Mark) -> egui::Rect {
    let rect = align.anchor_size(at, mark_chip_size(ui, mark));
    let color = mark.color();
    let p = ui.painter();
    // The black pass first: over arbitrary artwork a translucent tint alone could vanish.
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

/// Whether a library file is a kit that is sharing — the *Kits* filter's and the
/// file menu's question. A mark that stopped sharing is not one, nor is
/// a conflict copy unless it is the kit's only file left.
fn is_kit(lib: &crate::library::state::Library, entry: &Entry) -> bool {
    marks_kit(entry) && stands_for_its_kit(lib, entry)
}

/// Whether `entry` is the file that stands for the kit id it carries —
/// `kits::index::kit_file`'s answer, the picker's. A file with no kit mark stands
/// for none and may become one, unless it is a conflict copy.
fn stands_for_its_kit(lib: &crate::library::state::Library, entry: &Entry) -> bool {
    let (Some(id), Some(_)) = (entry.meta.id.as_deref(), entry.meta.kit.as_ref()) else {
        return lib.conflict(entry).is_none();
    };
    crate::kits::index::kit_file(lib, id).is_some_and(|k| k.path == entry.path)
}

/// Whether a file carries a sharing kit mark — the card's *Kit* chip, which a
/// conflict copy of a kit wears beside its conflict mark.
fn marks_kit(entry: &Entry) -> bool {
    entry
        .meta
        .kit
        .as_ref()
        .is_some_and(ondin_core::meta::KitMark::is_shared)
}

/// The *Kit* chip on a card's cover, its bottom-left at `at`: the package glyph and
/// the word. Returns its rect. A dark pass sits under the ground, for [`mark_chip`]'s
/// reason.
fn kit_chip(ui: &egui::Ui, at: egui::Pos2) -> egui::Rect {
    const H: f32 = 20.0;
    const PAD: f32 = 7.0;
    const GAP: f32 = 5.0;
    const GLYPH: f32 = 11.0;
    let ink = theme::color::text_a(179);
    let word = ui
        .ctx()
        .fonts_mut(|f| f.layout_no_wrap("Kit".into(), egui::FontId::proportional(10.5), ink));
    let w = PAD + GLYPH + GAP + word.size().x + PAD;
    let rect = egui::Align2::LEFT_BOTTOM.anchor_size(at, egui::vec2(w, H));
    let p = ui.painter();
    let r = egui::CornerRadius::same(5);
    p.rect_filled(rect, r, egui::Color32::from_black_alpha(150));
    p.rect_filled(rect, r, theme::color::text_a(20));
    p.text(
        egui::pos2(rect.left() + PAD + GLYPH / 2.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        icon::PACKAGE,
        theme::icon_font(GLYPH),
        ink,
    );
    p.galley(
        egui::pos2(
            rect.left() + PAD + GLYPH + GAP,
            rect.center().y - word.size().y / 2.0,
        ),
        word,
        ink,
    );
    rect
}

/// The *Open* chip, anchored at `at` by `align`, on a card or row whose document is
/// open in a tab: an accent chip, since a click brings that tab forward rather than
/// opening the document again.
fn open_chip(ui: &egui::Ui, at: egui::Pos2, align: egui::Align2) -> egui::Rect {
    let ink = color::ACCENT_100;
    let word = open_word(ui);
    let rect = align.anchor_size(at, open_chip_size(ui));
    let p = ui.painter();
    let r = egui::CornerRadius::same(4);
    p.rect_filled(rect, r, color::ACCENT_900);
    p.rect_stroke(
        rect,
        r,
        egui::Stroke::new(1.0, color::ACCENT.gamma_multiply(0.45)),
        egui::StrokeKind::Inside,
    );
    p.text(
        egui::pos2(rect.left() + OPEN_PAD + OPEN_GLYPH / 2.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        icon::BROWSERS,
        theme::icon_font(OPEN_GLYPH),
        ink,
    );
    p.galley(
        egui::pos2(
            rect.left() + OPEN_PAD + OPEN_GLYPH + OPEN_GAP,
            rect.center().y - word.size().y / 2.0,
        ),
        word,
        ink,
    );
    rect
}

const OPEN_PAD: f32 = 7.0;
const OPEN_GAP: f32 = 4.0;
const OPEN_GLYPH: f32 = 11.0;

/// The *Open* chip's word, laid out once for both its size and its drawing.
fn open_word(ui: &egui::Ui) -> std::sync::Arc<egui::Galley> {
    ui.ctx().fonts_mut(|f| {
        f.layout_no_wrap(
            "Open".into(),
            egui::FontId::proportional(10.5),
            color::ACCENT_100,
        )
    })
}

/// How much room [`open_chip`] needs; the list view takes it out of the *Name*
/// column before eliding.
fn open_chip_size(ui: &egui::Ui) -> egui::Vec2 {
    egui::vec2(
        OPEN_PAD + OPEN_GLYPH + OPEN_GAP + open_word(ui).size().x + OPEN_PAD,
        18.0,
    )
}

/// What a card or a row can be marked with — at most one, most severe first
/// ([`OndinApp::mark_of`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mark {
    /// This build cannot load the file.
    ///
    /// The card's *Open* stays live over this mark: the file is often merely written
    /// by a newer build, so the mark warns and the click is still the user's.
    Unreadable,
    /// A sync client's copy.
    Conflict,
}

impl Mark {
    fn label(self) -> &'static str {
        match self {
            Self::Unreadable => "UNREADABLE",
            Self::Conflict => "CONFLICT",
        }
    }

    /// Amber for a conflict (nothing is broken, the user picks which to keep), red
    /// for a file this build cannot open.
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
/// Measured per mark: the list view takes it out of the *Name* column before eliding.
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

/// Parse a `#rrggbb` from `projects.json` into a colour, falling back to the
/// accent on a malformed value (the file is hand-editable).
///
/// The guard checks bytes are hex digits, not only length, so a non-ASCII value
/// cannot reach the parse.
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

/// Cut a string to fit `max_w`, with an ellipsis. Hand-rolled because cards and
/// rows paint text with `Painter::text` into an allocated box.
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
        // A project id is never read back: the project may be gone by next launch.
        let p = Nav::Project("0f9c2b7a4e".into());
        assert_eq!(p.id(), "0f9c2b7a4e");
        assert_eq!(Nav::from_id(p.id()), Nav::Recent);
    }

    /// A hand-editable colour must never crash the sidebar; multi-byte values reach
    /// past the length guard. flip: drop `is_ascii_hexdigit` from `swatch` → panics
    /// at the `"#€abc"` slice.
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
            // Six bytes, four characters.
            "#€abc",
            // Six bytes, two characters.
            "#€€",
            // Six bytes, three two-byte characters.
            "#ééé",
            // Six bytes, five characters.
            "#éa0b1",
        ] {
            assert_eq!(swatch(bad), color::ACCENT, "{bad}");
        }
    }

    /// The window the screen lays itself out in.
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

    /// The colour of the library's status dot this frame, or `None` when the top bar
    /// painted none. Found by radius; a tooltip never reaches `.shapes`, so wording
    /// is asserted via `session.status().text`.
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
    /// Several passes, because interaction state and `TextEdit` focus lag a frame.
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

    /// Whether `text` was painted in the body, right of the sidebar (which has its
    /// own *New file* button).
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

    /// The search field's placeholder sits at the same spot painted and as a live
    /// `TextEdit`. flip: give the `TextEdit` the whole field rect → fails at the y
    /// assertion (`TextEdit::margin` is dead under `Frame::NONE`, so it is no flip).
    #[test]
    fn the_search_placeholder_does_not_move_when_the_field_takes_focus() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "search-align");

        let resting = run_at(&galleys(&mut app, &ctx), "Search files and projects");
        // Open the field, or the test compares a run to itself.
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

    /// Every row of the Settings card fits the card's own column, and the commit is
    /// dim until something has been edited. Sees only painted text escaping (wrapping
    /// labels absorb a squeeze). flip: lengthen the *Reopen the last file* label →
    /// fails at the overflow assertion.
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
        // The card is the rect containing its first eyebrow; the dashboard still paints behind.
        let eyebrow = full
            .shapes
            .iter()
            .find_map(|cs| match &cs.shape {
                egui::Shape::Text(t) if t.galley.text() == "STORAGE" => Some(t.pos),
                _ => None,
            })
            .expect("the modal is open and painting its first section header");
        // Descends into `Shape::Vec`: a shadowed `Frame` paints `Vec([shadow, rect])`.
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
            // Bounded above too, or the backdrop and central panel match.
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
        // The eyebrow starts at the content's left edge; the column is `set_width`'s.
        let column = ui::menu_inner_w(settings::CARD_W, settings::PAD);
        assert_eq!(
            column, 338.0,
            "376 across, less 18 of padding and a hairline"
        );
        // Paint order separates the modal's own text from the empty state behind it.
        for cs in full.shapes.iter().skip(card_at + 1) {
            let egui::Shape::Text(t) = &cs.shape else {
                continue;
            };
            if !card.contains(t.pos) {
                continue;
            }
            // Against the clipped right edge: a singleline field scrolls, so its galley overruns.
            let ink = (t.pos.x + t.galley.size().x).min(cs.clip_rect.right());
            let over = ink - (eyebrow.x + column);
            assert!(
                over <= 0.5,
                "{:?} runs {over:.1}pt past the card's own column",
                t.galley.text()
            );
        }

        // Asserted on the form state, not a ground colour: the screen is live behind.
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

    /// A starred document's card carries the star (a gold and a dark `Shape::Path`),
    /// and an unstarred one carries nothing. flip: invert the condition to
    /// `!self.is_starred(entry)` → fails at the unstarred assertion.
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

        // Counted by fill: the sidebar's *Starred* row wears an outline glyph.
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
                    // Thirty vertices: ten corners, each cut into three by its fillet.
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
        // One gold, and one dark pass under it as the drop shadow.
        assert_eq!(
            stars(&mut app),
            (1, 1),
            "the gold star and the dark pass under it"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The New project name field takes the caret when the modal opens and does not
    /// take it back once something else has it. flip: drop the `name_focused` latch
    /// → fails at the second assertion.
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

        // Stands in for a click elsewhere in the card: the caret goes.
        ctx.memory_mut(|m| m.surrender_focus(focused.expect("checked above")));
        let _ = galleys(&mut app, &ctx);
        assert_eq!(
            ctx.memory(|m| m.focused()),
            None,
            "the field asks once, so once it has been left it stays left"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Enter in *New project* with no name keeps the card and what was chosen on
    /// it, as the disabled *Create project* button does.
    /// flip: `new_project_modal`'s Enter not checking the name → fails here.
    #[test]
    fn enter_with_no_name_keeps_the_new_project_card() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "np-enter");
        app.dash.new_project = Some(NewProject {
            color: 3,
            ..NewProject::default()
        });
        let _ = galleys(&mut app, &ctx);
        press(&mut app, &ctx, egui::Key::Enter);
        assert_eq!(
            app.dash.new_project.as_ref().map(|np| np.color),
            Some(3),
            "the card and its colour stay"
        );
        assert!(app.library.projects.projects.is_empty(), "nothing created");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The project filter is offered exactly where it acts, and a filter naming a
    /// project that has gone stops filtering. flip: drop `self.filter_applies() &&`
    /// from the retain in `visible_entries` → fails at the *Recent* assertion.
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
        // Recent is what was opened, so mark both or that arm reads 0 either way.
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

    /// Drive one full click at `at`, press and release on separate frames.
    fn click_at(app: &mut OndinApp, ctx: &egui::Context, at: egui::Pos2) {
        click_with(app, ctx, at, egui::PointerButton::Primary);
    }

    /// [`click_at`] with `button`.
    fn click_with(
        app: &mut OndinApp,
        ctx: &egui::Context,
        at: egui::Pos2,
        button: egui::PointerButton,
    ) {
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
            button,
            pressed,
            modifiers: Default::default(),
        };
        frame(app, vec![egui::Event::PointerMoved(at)]);
        frame(app, vec![button(true)]);
        frame(app, vec![button(false)]);
        frame(app, vec![]);
        app.settle_open();
    }

    /// The centre of the card or row whose name is `name`, from the last frame.
    fn card_centre(app: &mut OndinApp, ctx: &egui::Context, name: &str) -> egui::Pos2 {
        let (pos, size, _) = galleys(app, ctx)
            .into_iter()
            .find(|(_, _, t)| t == name)
            .unwrap_or_else(|| panic!("no card named {name:?}"));
        pos + size / 2.0
    }

    /// A right-click on a card or a row opens the ⋮ menu's rows at the press, opens
    /// no document, and a row chosen from it runs; a second right-click moves the
    /// menu to its own card.
    /// flip: `pick_or_open` not reading `secondary_clicked` → fails at "the menu opens".
    #[test]
    fn a_right_click_opens_the_file_menu_at_the_press() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "right-click");
        let mut alpha_doc = ondin_core::Document::new(app.session.ids.mint());
        let alpha = store::file_document(&root, None, "Alpha", &mut alpha_doc).unwrap();
        let mut bravo_doc = ondin_core::Document::new(app.session.ids.mint());
        let bravo = store::file_document(&root, None, "Bravo", &mut bravo_doc).unwrap();
        app.library.refresh();
        app.dash.nav = Nav::All;

        for list in [false, true] {
            app.dash.list_view = list;
            app.dash.menu_for = None;
            let on_alpha = card_centre(&mut app, &ctx, "Alpha");
            click_with(&mut app, &ctx, on_alpha, egui::PointerButton::Secondary);
            assert_eq!(app.dash.menu_for.as_ref(), Some(&alpha), "the menu opens");
            assert_eq!(app.session.path, None, "and opens no document");
            let (rename, _, _) = galleys(&mut app, &ctx)
                .into_iter()
                .find(|(_, _, t)| t == "Rename")
                .expect("the ⋮ menu's rows");
            assert!(
                rename.x > on_alpha.x && rename.y > on_alpha.y,
                "at the press, not under the dots (list: {list})"
            );

            // Clear of the menu, which hangs over Bravo's name: its cover, or its row's
            // far side.
            let on_bravo = card_centre(&mut app, &ctx, "Bravo")
                + if list {
                    egui::vec2(250.0, 0.0)
                } else {
                    egui::vec2(0.0, -40.0)
                };
            click_with(&mut app, &ctx, on_bravo, egui::PointerButton::Secondary);
            assert_eq!(
                app.dash.menu_for.as_ref(),
                Some(&bravo),
                "a second one moves it"
            );
        }

        let rename = card_centre(&mut app, &ctx, "Rename");
        click_at(&mut app, &ctx, rename);
        assert_eq!(
            app.rename_entry.as_ref().map(|r| &r.0),
            Some(&bravo),
            "a row chosen from it runs"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A click that dismisses a floating library menu (⋮ popup, project dropdown) at
    /// any click site is spent doing so; the menu closes, and the same clicks work
    /// with no menu up. flip: remove the `menu_was_up` early return from
    /// `pick_or_open` → fails at the first "must not have opened" assertion;
    /// `library_menu_open()` in its place fails there too.
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

        // The control, first: with nothing floating, a click on Bravo opens it.
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

        // The *All projects ⌄* dropdown; `project_filter` clears `filter_open` without a project.
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

        // The menu anchor must be shown on this nav, or `menu_for` is never drawn nor
        // dismissed, so file Charlie in Kestrel.
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

        app.dash.nav = Nav::All;
        let row = card_centre(&mut app, &ctx, "Kestrel");
        app.dash.menu_for = Some(alpha.clone());
        click_at(&mut app, &ctx, row);
        assert_eq!(
            app.dash.nav,
            Nav::All,
            "nor may it move the sidebar off the screen the user was reading"
        );

        // `nav_row`, the sidebar's other rows, carries a copy of the predicate.
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

        // The body header's controls, the search being the visible one.
        app.dash.nav = Nav::All;
        let search = card_centre(&mut app, &ctx, "Search files and projects");
        app.dash.menu_for = Some(alpha.clone());
        click_at(&mut app, &ctx, search);
        assert!(
            app.dash.search.is_none(),
            "nor may it open the search overlay — the header's controls are the \
             eighth door and were the last to be found"
        );

        // The control for all of them: the same clicks with nothing floating.
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

    /// A clicked card pulses until its document lands, and only that card. flip:
    /// `opening_alpha` answering `1.0` → fails at "the clicked card dims".
    #[test]
    fn a_card_pulses_while_its_document_opens() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "pulse");
        let mut a = ondin_core::Document::new(app.session.ids.mint());
        let path = store::file_document(&root, None, "Pulsing", &mut a).unwrap();
        let mut b = ondin_core::Document::new(app.session.ids.mint());
        store::file_document(&root, None, "Resting", &mut b).unwrap();
        app.library.refresh();
        app.dash.nav = Nav::All;

        let grounds = |app: &mut OndinApp| -> Vec<u8> {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
                    ..Default::default()
                },
                |ui| app.dashboard_ui(ui),
            );
            let mut alphas: Vec<u8> = out
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    // Premultiplied: the card's red is `CARD`'s scaled by its alpha.
                    egui::Shape::Rect(r)
                        if r.corner_radius == egui::CornerRadius::same(9)
                            && r.fill.a() > 0
                            && (r.fill.r() as i32
                                - (color::CARD.r() as i32 * r.fill.a() as i32 / 255))
                                .abs()
                                <= 1 =>
                    {
                        Some(r.fill.a())
                    }
                    _ => None,
                })
                .collect();
            alphas.sort();
            alphas
        };
        assert_eq!(grounds(&mut app), vec![255, 255], "the control: both whole");

        app.begin_open(&path);
        assert!(app.opening.is_some(), "the read is off the frame");
        // Into the held half of the pulse.
        app.opening.as_mut().unwrap().at =
            std::time::Instant::now() - std::time::Duration::from_secs_f32(0.4);
        let dimmed = grounds(&mut app);
        assert_eq!(dimmed.len(), 2, "{dimmed:?}");
        assert!(dimmed[0] < 160, "the clicked card dims: {dimmed:?}");
        assert_eq!(dimmed[1], 255, "and only it: {dimmed:?}");

        app.settle_open();
        assert_eq!(app.session.path.as_deref(), Some(path.as_path()));
        assert!(app.opening.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A library act made while a file loads gives the load up. flip: drop
    /// `dashboard_act`'s clear → fails at "the move gave the open up".
    #[test]
    fn a_library_act_mid_load_gives_the_open_up() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "stale-open");
        let mut a = ondin_core::Document::new(app.session.ids.mint());
        let path = store::file_document(&root, None, "Moving", &mut a).unwrap();
        app.library.refresh();
        app.begin_open(&path);
        assert!(app.opening.is_some(), "the fixture: a load in flight");
        app.dashboard_act(Act::MoveTo(path.clone(), None));
        assert!(app.opening.is_none(), "the move gave the open up");
        app.settle_open();
        assert_ne!(app.session.path.as_deref(), Some(path.as_path()));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A single click opens a document and never selects one; the keyboard cursor
    /// is separate, and a cursor whose file has gone does not survive the next frame.
    /// flip: move the background `ui.interact` below the `ScrollArea` in
    /// `dashboard_body` → fails at the first assertion (the last widget wins the hit).
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

        app.go_to_dashboard();
        press(&mut app, &ctx, egui::Key::ArrowRight);
        assert_eq!(
            app.dash.selected.as_deref(),
            Some(path.as_path()),
            "the arrows still put a cursor on a card (D374)"
        );

        // The bottom-left of the body is the background by construction.
        let empty = egui::pos2(SIDEBAR_W + 40.0, SCREEN.y - 40.0);
        click_at(&mut app, &ctx, empty);
        assert_eq!(
            app.dash.selected, None,
            "and a click on nothing puts it back"
        );

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

    /// A dropped `.ondin` outside the library is imported; anything else is not, and
    /// says so once. flip: remove `take_dropped_documents` from `dashboard_ui` →
    /// fails at the first assertion.
    #[test]
    fn a_dropped_document_is_imported_and_a_dropped_image_is_not() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "drop");
        // Process-unique, so a concurrent run's fixture is not deleted.
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

        let png = outside.join("picture.png");
        std::fs::write(&png, [0u8; 8]).unwrap();
        drop(&mut app, &png);
        assert_eq!(
            app.library.entries.len(),
            1,
            "an image is not a document — the library takes .ondin files"
        );
        // The dot is the surface and the slot is the wording.
        assert!(
            status_dot_color(&mut app, &ctx).is_some(),
            "the bar is marked"
        );
        assert!(
            app.session.status().text.contains(".ondin"),
            "and it says what this screen takes: {}",
            app.session.status().text
        );

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
        // Surface asserted apart from the field: no status call in the top bar fails the first.
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

    /// The import summary's arms, one sentence each; the refusal clauses survive the
    /// happy arms. flip: move
    /// `rest` into the `(0, 0)` arm only → the second, fourth and sixth fail.
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

    /// A partial import, driven through `import_counting`, counts the files already
    /// in the library rather than dropping them. flip: drop `already += 1` → fails
    /// the second assertion with `(1, 0, 0)`.
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

    /// A base folder naming an existing file is refused and not persisted, while the
    /// unrelated settings still apply. flip: restore `create_dir_all(&new_root)` at
    /// the foot of the block → fails the first and second assertions.
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

        // The dot is the surface and the slot is the wording.
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

    /// A partly-failed migration names every file it left behind on screen, and the
    /// retry finishes the job (a file where a project folder must go; the loose
    /// document is the control). flip: drop the
    /// `self.stranded = …` assignment in `apply_library_settings` → fails at "the
    /// migration left a report"; drop `self.library.refresh()` from
    /// `retry_migration` → fails at the `library.entries` assertion.
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

    /// A report planted by hand: one file left in `from` on the way to `to`.
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

    /// Every change of base folder settles the last report, migrating or not. flip:
    /// `if !failed.is_empty() { stranded = Some(..) }` in place of the `then` → fails
    /// at "a clean migration clears"; delete `else { self.stranded = None }` → fails
    /// at "and so does a change that moves nothing".
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

    /// A retry refuses a report made for another library, and keeps it. flip: delete
    /// the `stranded.to != self.library.root` refusal → fails at "the file is where
    /// it was".
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

    /// A retry against an old folder that cannot be reached keeps the list (a down
    /// share is not an empty folder). flip: delete the `scan::readable` refusal →
    /// fails at "the list is kept".
    #[test]
    fn a_retry_against_an_unreachable_old_folder_keeps_the_list() {
        let ctx = egui::Context::default();
        let (mut app, root) = app(&ctx, "retry-unreachable");
        let (mut report, _) = planted_report(&root.join("old"), &root);
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

    /// A retry with the library's own folder unreachable keeps the list and does not
    /// recreate the root. flip: delete the library-root refusal → fails at "the list
    /// is kept".
    #[test]
    fn a_retry_with_the_library_folder_unreachable_keeps_the_list() {
        let ctx = egui::Context::default();
        let (mut app, root) = app(&ctx, "retry-root-gone");
        let old = root.with_file_name(format!("{}-old", root.file_name().unwrap().display()));
        let _ = std::fs::remove_dir_all(&old);
        let (report, left) = planted_report(&old, &root);
        app.stranded = Some(report.clone());
        std::fs::remove_dir_all(&root).unwrap();

        app.retry_migration();
        assert_eq!(app.stranded, Some(report), "the list is kept");
        assert!(left.exists(), "and the file is where it was");
        assert!(!root.exists(), "and nothing recreated the library's folder");
        let _ = std::fs::remove_dir_all(&old);
    }

    /// The stranded list shows `STRANDED_ROWS` names and says how many it did not.
    /// flip: remove the `.take` → fails at "the seventh is not drawn"; remove the
    /// "…and N more" block → fails at "and the rest are counted".
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

    /// The header's controls are one row of one height, sharing one centre. flip:
    /// `VIEW_CELL_H` from `HEADER_CONTROL_H - 4.0` to `HEADER_CONTROL_H` → fails at
    /// the height assertion (the centre one stays green).
    #[test]
    fn the_header_controls_share_a_height_and_a_centre() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "header-row");
        app.dash.sort = Sort::Edited;
        // Not *Recent*, where the sort button is not drawn.
        app.dash.nav = Nav::All;

        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
            ..Default::default()
        };
        let mut full = ctx.run_ui(input.clone(), |ui| app.dashboard_ui(ui));
        for _ in 1..4 {
            full = ctx.run_ui(input.clone(), |ui| app.dashboard_ui(ui));
        }
        // The segmented track is the one rect exactly `VIEW_TRACK_W` wide.
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
        // Bounded by height, or the central panel's ground matches.
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

    /// Drive one bare key press through one frame of the dashboard.
    fn press(app: &mut OndinApp, ctx: &egui::Context, key: egui::Key) {
        press_with(app, ctx, key, Default::default());
    }

    /// The same, for a chord. The modifiers go on the frame as well as the event:
    /// `InputState` takes `modifiers` from `RawInput`, not from the `Event::Key`.
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
        app.settle_open();
    }

    /// The arithmetic behind the arrow keys. flip: replace the `clamp` with a range
    /// test returning `None` off the end → fails at the short-last-row `Down`.
    #[test]
    fn the_arrows_step_in_reading_order_and_clamp_at_the_ends() {
        use egui::Key::{ArrowDown, ArrowLeft, ArrowRight, ArrowUp};
        // Six items in a four-wide grid: one full row and a short one.
        let g = |cur, key| arrow_target(cur, key, false, 4, 6);
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
        // The list is one column wide: single vertical steps, no horizontals.
        let l = |cur, key| arrow_target(cur, key, true, 4, 6);
        assert_eq!(l(Some(2), ArrowDown), Some(3));
        assert_eq!(l(Some(2), ArrowUp), Some(1));
        assert_eq!(
            l(Some(2), ArrowLeft),
            None,
            "a full-width row has nothing beside it"
        );
        assert_eq!(l(Some(2), ArrowRight), None);
        // The caller indexes `entries`: `None` on an empty list prevents a panic.
        assert_eq!(arrow_target(None, ArrowDown, false, 4, 0), None);
        assert_eq!(arrow_target(Some(0), ArrowUp, true, 4, 0), None);
    }

    /// `Down` is a row of whatever width the window made; a count of `0` steps one.
    /// flip: put a fixed 4 back in place of `grid_cols` → fails at "down is a row of
    /// six".
    #[test]
    fn the_arrows_step_a_row_of_the_grid_as_laid() {
        use egui::Key::ArrowDown;
        assert_eq!(
            arrow_target(Some(1), ArrowDown, false, 6, 20),
            Some(7),
            "down is a row of six"
        );
        assert_eq!(
            arrow_target(Some(1), ArrowDown, false, 0, 20),
            Some(2),
            "before any grid is drawn, a row of one"
        );
    }

    /// The grid holds as many columns as keep every card at `CARD_MAX_W` or narrower,
    /// and the cards fill the row; a narrow body is one column. flip: `floor` for
    /// `ceil` in `grid_cols` → fails at "no card is wider than the ceiling".
    #[test]
    fn the_grid_adds_columns_rather_than_stretching_cards() {
        for width in [
            120.0, 299.0, 300.0, 614.0, 900.0, 1242.0, 1243.0, 1600.0, 2400.0,
        ] {
            let w = card_width(width);
            let cols = grid_cols(width) as f32;
            assert!(
                w <= CARD_MAX_W + 1e-3,
                "no card is wider than the ceiling: {w} at {width}"
            );
            assert!(
                (cols * w + (cols - 1.0) * GRID_GAP - width).abs() < 1e-3,
                "and the row is filled exactly at {width}"
            );
        }
        let four = 4.0 * CARD_MAX_W + 3.0 * GRID_GAP;
        assert_eq!(grid_cols(four), 4, "exactly four at 300");
        assert_eq!(grid_cols(four + 1.0), 5, "one point more is a fifth column");
        assert_eq!(grid_cols(1600.0), 6, "the screenshot's width holds six");
        assert_eq!(grid_cols(100.0), 1, "and a narrow body is one column");
    }

    /// On the frame the search overlay closes its field is not drawn, so egui holds
    /// no focus; `search.is_some()` alone keeps that frame's arrow off the grid.
    /// Calls `dashboard_keys` with nothing drawn to reach that frame.
    /// flip: deleting `self.dash.search.is_some() ||` from `dashboard_keys` → fails
    /// at "an open search owns the arrows even with no field drawn".
    #[test]
    fn an_open_search_owns_the_keys_on_a_frame_its_field_is_not_drawn() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "keys-unfocused");
        let mut doc = ondin_core::Document::new(app.session.ids.mint());
        store::file_document(&root, None, "Alpha", &mut doc).unwrap();
        app.library.refresh();
        app.dash.nav = Nav::All;
        let entries = app.visible_entries();
        assert!(!entries.is_empty(), "fixture: the library lists the file");

        let arrow = |app: &mut OndinApp| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
                events: vec![egui::Event::Key {
                    key: egui::Key::ArrowDown,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
                ..Default::default()
            };
            let mut act = None;
            let _ = ctx.run_ui(input, |ui| {
                assert!(
                    !ui.ctx().egui_wants_keyboard_input(),
                    "fixture: nothing has focus"
                );
                app.dashboard_keys(ui.ctx(), &entries, &mut act);
            });
        };

        app.dash.search = Some(String::new());
        arrow(&mut app);
        assert_eq!(
            app.dash.selected, None,
            "an open search owns the arrows even with no field drawn"
        );

        // Control: the same frame with the search shut reaches the keymap.
        app.dash.search = None;
        arrow(&mut app);
        assert!(
            app.dash.selected.is_some(),
            "with no search the arrow selects"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Arrows move the selection, `Escape` puts it back, `Enter` opens it and `Delete`
    /// asks first — and none reaches the library while the search overlay, the file
    /// menu or the filter dropdown has the keyboard. The `search.is_some()` guard on
    /// its own is pinned by `an_open_search_owns_the_keys_on_a_frame_its_field_is_not_drawn`.
    /// flip: the guard back to `menu_for.is_some()` → fails at "the filter dropdown is open".
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
        // Two files written in one second have no order under the default sort.
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

        // The ⋮ popup blocks the keymap and takes Escape.
        app.dash.menu_for = Some(a.clone());
        press(&mut app, &ctx, egui::Key::ArrowDown);
        assert_eq!(
            app.dash.selected, None,
            "the file menu is open, so the arrows are not the grid's"
        );
        press(&mut app, &ctx, egui::Key::Escape);
        assert_eq!(app.dash.menu_for, None, "and Escape shuts it");

        // Set before every press: with no project `project_filter` clears `filter_open`,
        // and the header clears it after `dashboard_keys` each frame.
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

    /// `Ctrl+K` does not open the search from under a card that owns the keyboard
    /// (*New project*, the rename field). flip: drop `library_keys_are_free()` from `search_field`'s `chord`
    /// → fails at the first assertion.
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

        app.rename_entry = Some(Default::default());
        press_with(&mut app, &ctx, egui::Key::K, cmd);
        assert_eq!(app.dash.search, None, "nor the rename field's");
        assert!(
            app.rename_entry.is_some(),
            "and Ctrl+K does not close the rename — shortcuts.md does not list it \
             as a key that does"
        );
        app.rename_entry = None;

        // Control: with nothing owning the keyboard the chord still opens the overlay.
        press_with(&mut app, &ctx, egui::Key::K, cmd);
        assert_eq!(
            app.dash.search.as_deref(),
            Some(""),
            "control: Ctrl+K still opens the search when nothing else wants the \
             keyboard"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The search chord is `Ctrl` without Alt and without Shift (`input::resolve`
    /// does not run on this screen). flip: restore `i.modifiers.command` alone →
    /// fails at the first `Ctrl+Shift+K` assertion.
    #[test]
    fn the_search_chord_does_not_answer_to_alt_or_shift() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "searchmods");
        app.dash.nav = Nav::All;
        let cmd = egui::Modifiers::COMMAND;
        let shift = egui::Modifiers { shift: true, ..cmd };
        let alt = egui::Modifiers { alt: true, ..cmd };

        // Controls first: both plain chords really do open it.
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

    /// `Escape` under the search overlay closes the overlay and nothing else; bare
    /// `Escape` still clears the selection. flip: `key_pressed` in place of
    /// `consume_key` → fails at the surviving-selection assertion.
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

        press(&mut app, &ctx, egui::Key::Escape);
        assert_eq!(
            app.dash.selected, None,
            "control: with nothing to close, Escape still drops the selection"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `F2` and `Ctrl+R` rename and `Ctrl+D` duplicates the keyboard cursor's
    /// document, and none is offered in the trash. Duplicate is asserted by count,
    /// not name. flip: drop `Ctrl+R`'s arm → fails at "Ctrl+R opens the same field";
    /// drop `!in_trash()` from `start_rename` → fails at the `F2` trash assertion.
    /// Dropping the guard from the `Ctrl+D` arm stays green (the trash is not in
    /// `library.entries`).
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

        // `Ctrl+R` enters through the chord side of the `plain` gate.
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

    /// A library whose folder has gone says so, on the project nav (the harder one),
    /// and refuses to create a document in the hole. flip: ungate `new_file_into` →
    /// fails at "the empty state names the fault"; ungate `new_library_document` →
    /// fails at "Ctrl+N created nothing" (`create_dir_all` would succeed).
    #[test]
    fn a_missing_library_folder_says_so_instead_of_reading_as_empty() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "offline");
        seed(&mut app, "p-1", "Alps", Some("Alps"));
        app.dash.nav = Nav::Project("p-1".into());
        app.library.local.mark_opened("doc-1");
        // The unplugged drive.
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
        let text = galleys(&mut app, &ctx)
            .into_iter()
            .map(|(_, _, t)| t)
            .collect::<Vec<_>>();
        assert!(
            text.iter().any(|t| t.contains("isn't available")),
            "out loud, rather than by doing nothing: {text:?}"
        );
    }

    /// Every failure and report the library screen writes is painted on it as a
    /// status dot (failure and info in different colours; none when quiet), with the
    /// wording in `session.status().text`. flip: remove the `status_dot` call from
    /// `dashboard_top_bar` → fails at the first `Some(...)` assertion; swap
    /// `status_color`'s arms → fails at the failure-colour assertion.
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
        // A later action's report, so a later frame's: in one frame it would join.
        app.session.next_status_pass();
        app.session.info(info_text);
        let lit = status_dot_color(&mut app, &ctx).expect("a report is marked too");
        assert_ne!(
            lit,
            egui::Color32::from_rgb(226, 138, 138),
            "and a report is not painted as a failure — on this screen the colour \
             is the only thing that separates them"
        );
        assert_eq!(app.session.status().text, info_text);

        // Absent, not a colourless dot, when there is nothing to say.
        app.session.info("");
        assert_eq!(status_dot_color(&mut app, &ctx), None);
        assert_eq!(runs(&mut app).len(), quiet.len());

        // The latched unreadable records report through the same line.
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

    /// `Ctrl+N` files a new document in the project in front of you: the nav on the
    /// dashboard, the open document's project in the editor. Asserted by where the
    /// file landed on disk.
    #[test]
    fn ctrl_n_files_into_the_project_in_front_of_you() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "new-doc");
        // No `reload_projects` after the seed: it re-reads the file on disk, which has none.
        seed(&mut app, "p-1", "Alps", Some("Alps"));
        app.library.refresh();
        app.dash.nav = Nav::Project("p-1".into());
        // `headless` starts in `View::Editor`; start elsewhere so "opened" means something.
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

        app.dash.nav = Nav::All;
        app.dispatch(&ctx, crate::input::Action::NewDocument);
        let second = app.session.path.clone().expect("a second document");
        assert_ne!(second, made, "a new file, not the one that was open");
        assert_eq!(
            second.parent(),
            Some(root.join("Alps").as_path()),
            "the open document's project, not the nav left behind on the dashboard"
        );

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
    /// copied does not. flip: drop the `conflict` branch from `file_card` → the grid
    /// assertion fails (the list draws from a different call site).
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

    /// A document written by a newer build (well-formed JSON the loader refuses)
    /// wears the *UNREADABLE* mark in both views, and the healthy one beside it does
    /// not. The mark is learned when the cover renders. flip: drop
    /// `covers.unreadable` from `mark_of` → fails at 0 against 1.
    #[test]
    fn a_document_this_build_cannot_open_wears_the_mark_and_a_healthy_one_does_not() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "unreadable");
        let mut healthy = ondin_core::Document::new(app.session.ids.mint());
        store::file_document(&root, None, "Landing v4", &mut healthy).unwrap();
        let mut broken = ondin_core::Document::new(app.session.ids.mint());
        let path = store::file_document(&root, None, "From tomorrow", &mut broken).unwrap();

        // Well-formed JSON this loader refuses: a future build's schema version.
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
        // One pass queues the render; `settle` waits for it (the render is on a thread).
        let _ = chips(&mut app, &ctx);
        app.covers.settle(&ctx);
        assert_eq!(chips(&mut app, &ctx), 1, "exactly the broken one is marked");
        app.dash.list_view = true;
        assert_eq!(chips(&mut app, &ctx), 1, "and the list marks it too");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A document open in a tab wears the *Open* chip in both views, after its name
    /// in a list row, and the document beside it does not.
    /// flip: drop the `open_chip` call from the list row → fails at "the list".
    #[test]
    fn a_document_in_a_tab_wears_the_open_chip_in_both_views() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "open-chip");
        let mut open = ondin_core::Document::new(app.session.ids.mint());
        let path = store::file_document(&root, None, "Landing v4", &mut open).unwrap();
        let mut shut = ondin_core::Document::new(app.session.ids.mint());
        store::file_document(&root, None, "Pricing", &mut shut).unwrap();
        app.library.refresh();
        app.begin_open(&path);
        app.settle_open();
        app.show_library();
        assert!(
            app.tab_of_path(&path).is_some(),
            "the fixture: one in a tab"
        );
        app.dash.nav = Nav::All;
        let chips = |app: &mut OndinApp, ctx: &egui::Context| {
            galleys(app, ctx)
                .into_iter()
                .filter(|(_, _, t)| t == "Open")
                .collect::<Vec<_>>()
        };
        app.dash.list_view = false;
        assert_eq!(chips(&mut app, &ctx).len(), 1, "the grid marks exactly one");
        app.dash.list_view = true;
        let runs = galleys(&mut app, &ctx);
        let chip = chips(&mut app, &ctx);
        assert_eq!(chip.len(), 1, "the list marks exactly one");
        // The name is painted twice on the screen; one of them must be the chip's row.
        assert!(
            runs.iter()
                .filter(|(_, _, t)| t == "Landing v4")
                .any(|(at, size, _)| {
                    (chip[0].0.y + chip[0].1.y / 2.0 - (at.y + size.y / 2.0)).abs() < 2.0
                        && chip[0].0.x > at.x + size.x
                }),
            "on the open document's row, after its name"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A card off screen asks for no cover, and a card scrolled to (by
    /// `follow_selection`) asks on the frame it arrives. flip: drop the
    /// `is_rect_visible` gate → fails at "the last card is off screen".
    #[test]
    fn only_a_card_on_screen_asks_for_its_cover() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "cover-viewport");
        for i in 0..40 {
            let mut doc = ondin_core::Document::new(app.session.ids.mint());
            store::file_document(&root, None, &format!("Screen {i:02}"), &mut doc).unwrap();
        }
        app.library.refresh();
        app.dash.nav = Nav::All;
        let order = app.visible_entries();
        assert_eq!(order.len(), 40, "the fixture: forty documents listed");
        let (first, last) = (&order[0], &order[39]);

        let _ = galleys(&mut app, &ctx);
        assert!(
            app.covers.asked_for(first),
            "the fixture: the first card is on screen and asks — or every card \
             answers `false` for a reason that is not the gate"
        );
        assert!(
            !app.covers.asked_for(last),
            "the last card is off screen and must not have asked"
        );
        let asked = order.iter().filter(|e| app.covers.asked_for(e)).count();
        assert!(
            asked < 40 / 2,
            "{asked} of 40 covers asked for with at most four rows on screen"
        );

        app.dash.selected = Some(last.path.clone());
        app.dash.scroll_to_selected = true;
        let _ = galleys(&mut app, &ctx);
        assert!(
            app.covers.asked_for(last),
            "a card scrolled to asks for its cover once it is on screen"
        );

        app.covers.settle(&ctx);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A project card off screen asks for none of its mosaic (the other caller of
    /// `Covers::get`). flip: delete `project_mosaic`'s `is_rect_visible` return →
    /// fails at "the last project's mosaic is off screen".
    #[test]
    fn a_project_card_off_screen_asks_for_no_covers() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "mosaic-viewport");
        let mut docs = Vec::new();
        for i in 0..24 {
            let p = project::Project {
                id: format!("p-{i:02}"),
                name: format!("Project {i:02}"),
                color: project::PROJECT_COLORS[0].into(),
                folder: None,
                created: 0,
                archived: false,
            };
            app.library.projects.projects.push(p.clone());
            let mut doc = ondin_core::Document::new(app.session.ids.mint());
            let path =
                store::file_document(&root, Some(&p), &format!("Screen {i:02}"), &mut doc).unwrap();
            docs.push(path);
        }
        app.library.refresh();
        app.dash.nav = Nav::Recent;
        let entry = |app: &OndinApp, path: &PathBuf| {
            app.library
                .entries
                .iter()
                .find(|e| &e.path == path)
                .cloned()
                .expect("the fixture: every document is listed")
        };
        let (first, last) = (entry(&app, &docs[0]), entry(&app, &docs[23]));

        let _ = galleys(&mut app, &ctx);
        assert!(
            app.covers.asked_for(&first),
            "the fixture: the first project's mosaic is on screen and asks"
        );
        assert!(
            !app.covers.asked_for(&last),
            "the last project's mosaic is off screen and must not have asked"
        );
        app.covers.settle(&ctx);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The list view takes a cover's answer though no row asks for one: the
    /// `Covers::pass` call at the top of `dashboard_ui` is the only taker. The wait
    /// is a deadline, not `settle` (which takes answers itself). flip: delete the
    /// `self.covers.pass(ui.ctx())` line → fails at "a list frame took the answer no
    /// row asked for".
    #[test]
    fn the_list_view_takes_an_unreadable_answer_no_row_asks_for() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "pass-list");
        let mut broken = ondin_core::Document::new(app.session.ids.mint());
        let path = store::file_document(&root, None, "From tomorrow", &mut broken).unwrap();
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
        app.dash.list_view = true;
        let entry = app
            .library
            .entries
            .iter()
            .find(|e| e.path == path)
            .cloned()
            .expect("the fixture: the document is listed");
        let chips = |app: &mut OndinApp, ctx: &egui::Context| {
            galleys(app, ctx)
                .into_iter()
                .filter(|(_, _, t)| t == Mark::Unreadable.label())
                .count()
        };
        assert_eq!(
            chips(&mut app, &ctx),
            0,
            "the fixture: the scan calls it healthy and nothing has asked"
        );
        assert!(
            !app.covers.asked_for(&entry),
            "the fixture: the list asks for no covers"
        );

        let _ = app.covers.get(&ctx, &entry);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
            ..Default::default()
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.covers.unreadable(&entry) && std::time::Instant::now() < deadline {
            let _ = ctx.run_ui(input.clone(), |ui| app.dashboard_ui(ui));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            app.covers.unreadable(&entry),
            "a list frame took the answer no row asked for"
        );
        assert_eq!(chips(&mut app, &ctx), 1, "and the row wears the chip");
        app.covers.settle(&ctx);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The cover pass runs once a frame, before any card asks, counted through
    /// `Covers::take_calls`. flip: move `self.covers.pass` below
    /// `library_settings_modal` → fails at "the pass runs before any card asks"; call
    /// it twice → fails at "one cover pass a frame".
    #[test]
    fn the_cover_pass_runs_once_a_frame_before_any_card_asks() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "pass-order");
        for name in ["Pass A", "Pass B", "Pass C"] {
            let mut doc = ondin_core::Document::new(app.session.ids.mint());
            store::file_document(&root, None, name, &mut doc).unwrap();
        }
        app.library.refresh();
        app.dash.nav = Nav::All;
        let _ = galleys(&mut app, &ctx);

        let _ = app.covers.take_calls();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| app.dashboard_ui(ui));
        let calls = app.covers.take_calls();

        assert!(
            calls.iter().filter(|c| **c == "get").count() >= 3,
            "the fixture: the three cards on screen each ask for a cover — or the \
             order below is about nothing; got {calls:?}"
        );
        assert_eq!(
            calls.iter().filter(|c| **c == "pass").count(),
            1,
            "one cover pass a frame; got {calls:?}"
        );
        assert_eq!(
            calls.first(),
            Some(&"pass"),
            "the pass runs before any card asks; got {calls:?}"
        );
        app.covers.settle(&ctx);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `remember_search` refuses a blank query, dedupes and promotes to the front,
    /// caps at `SEARCH_RECENTS`, persists (through `save_to`/`load_from`), and the
    /// rows reach the screen. flip: drop the `retain` → fails the promote assertion;
    /// `truncate(50)` → the cap; drop the empty-query guard → the blank one;
    /// `insert(0, ..)` → `push` fails at "newest first".
    #[test]
    fn a_remembered_search_is_deduped_promoted_capped_and_shown() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "recents");

        let list = |app: &OndinApp| app.library.local.recent_searches.clone();

        // The local index is unreachable under `cfg(test)`, so this holds by construction.
        assert!(
            list(&app).is_empty(),
            "a headless app's index is empty by construction, not by being cleared"
        );

        app.remember_search("   ");
        assert!(list(&app).is_empty(), "a blank query is not a search");

        for q in ["alpha", "beta", "gamma"] {
            app.remember_search(q);
        }
        assert_eq!(list(&app), vec!["gamma", "beta", "alpha"], "newest first");

        app.remember_search("alpha");
        assert_eq!(
            list(&app),
            vec!["alpha", "gamma", "beta"],
            "an old query moves to the front and is not duplicated"
        );

        for q in ["d", "e", "f"] {
            app.remember_search(q);
        }
        assert_eq!(
            list(&app),
            vec!["f", "e", "d", "alpha", "gamma"],
            "five, and the sixth-oldest is gone"
        );

        // Persistence, through an explicit path rather than the global one.
        let at = root.join("library.json");
        app.library.local.save_to(&at);
        let back = crate::library::cache::LocalIndex::load_from(&at);
        assert_eq!(
            back.recent_searches,
            list(&app),
            "the list is what survives a restart, which is the whole point of it"
        );

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

    /// The delete card says "for good" only in the Trash, in both body and title,
    /// asserted on the painted galleys in both navs. flip: `let purge = nav ==
    /// Nav::Trash` to `!=` → fails at the first assertion.
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

    /// The *Delete its N file(s) too* switch reads the way it acts: asserted through
    /// `ui::switch_row_ink`, starting off (`keep_files: true`). flip: `!dp.keep_files`
    /// → `dp.keep_files` → fails at the muted-ink assertion.
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

        let ink = |app: &mut OndinApp, ctx: &egui::Context| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
                ..Default::default()
            };
            let mut found = None;
            // Thirty passes: a modal `Area` fades in, scaling every colour until settled.
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

    /// Stepping down past the bottom of the viewport brings the card with it. Twenty
    /// documents keep the fifth row well below the fold. flip: delete the
    /// `follow_selection` call from `file_card` → the label stays far below the fold.
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
        // The body's own bottom, not the window's.
        let (_, y) = run_at(&runs, "Doc 17");
        assert!(
            y < SCREEN.y - 20.0,
            "the fifth row scrolled into view rather than staying below the fold \
             (the card's name is at y={y:.0})"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `Enter` will not open a document in the trash (the click shares `can_open`, but
    /// egui's double-click window is wall-clock). flip: `can_open` returning `true`
    /// → fails at the second assertion.
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

    /// The centre of the ⋮ menu row whose painted label is exactly `label` (*Delete*
    /// is a prefix of *Delete permanently*).
    fn menu_row_centre(app: &mut OndinApp, ctx: &egui::Context, label: &str) -> egui::Pos2 {
        let painted = galleys(app, ctx);
        let Some((pos, size, _)) = painted.iter().find(|(_, _, t)| t == label) else {
            let all: Vec<&str> = painted.iter().map(|(_, _, t)| t.as_str()).collect();
            panic!("no menu row {label:?} on screen; painted: {all:?}");
        };
        egui::pos2(pos.x + size.x / 2.0, pos.y + size.y / 2.0)
    }

    /// *Delete* from the ⋮ menu raises the confirmation card and does not trash the
    /// document. flip: `Act::Trash(path) if !trashed` → `if trashed` → fails at the
    /// `deleting` assertion.
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

    /// The ⋮ menu's rules: a trashed document gets two rows and neither is *Open*;
    /// *Star*/*Unstar* follows `is_starred`; *Move to project* opens the sheet.
    /// flip: delete the `("Open", …)` row → fails at "the
    /// ordinary menu offers Open"; invert `if starred` → fails at the
    /// `menu_row_centre("Star")` lookup.
    #[test]
    fn the_file_menu_reads_the_trash_the_star_and_the_move_sheet() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "filemenu-rows");
        let mut doc = ondin_core::Document::new(app.session.ids.mint());
        let path = store::file_document(&root, None, "Alpha", &mut doc).unwrap();
        app.library.refresh();
        app.dash.nav = Nav::All;

        app.dash.menu_for = Some(path.clone());
        let star = menu_row_centre(&mut app, &ctx, "Star");
        click_at(&mut app, &ctx, star);
        let entry = app.visible_entries()[0].clone();
        assert!(app.is_starred(&entry), "the row starred it");
        app.dash.menu_for = Some(path.clone());
        let _ = menu_row_centre(&mut app, &ctx, "Unstar");

        app.dash.menu_for = Some(path.clone());
        let move_to = menu_row_centre(&mut app, &ctx, "Move to project");
        click_at(&mut app, &ctx, move_to);
        assert_eq!(
            app.dash.moving.as_deref(),
            Some(path.as_path()),
            "the row opens the sheet rather than moving anything"
        );
        app.dash.moving = None;

        // All six ordinary rows first: the absence assertions below need the presence.
        // `Unstar`, because the click above starred it.
        app.dash.menu_for = Some(path.clone());
        let ordinary: Vec<String> = galleys(&mut app, &ctx)
            .into_iter()
            .map(|(_, _, t)| t)
            .collect();
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

    /// A project's grid ends in the dashed *New file* card, in the next cell of the
    /// last row, creating a document filed in that project; no other nav has one.
    /// flip: `file_grid` back to `entries.chunks(cols)` plus a trailing card → both
    /// position assertions fail; the x is the stronger.
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

        // The list says only *New file*; the heading names the project.
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

    /// The mosaic reproduces the design's four layouts from one rule; the
    /// full-height assertions carry the weight. flip: drop the `2 * col >= n` arm →
    /// fails at two files (the second cell comes back half height).
    #[test]
    fn a_project_mosaic_lays_out_the_designs_four_shapes_from_one_rule() {
        // 200 × 118 with a 7pt gutter: a two-column split is 96.5, three is 62.
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

        // Four has no design layout: three columns, the last full height.
        let four = shape(4);
        assert_eq!(four[3], (138.0, 0.0, 62.0, 118.0));
    }

    /// The search dropdown answers the arrows and `Enter` takes the row they are on;
    /// unsteered, `Enter` takes the first *document* match, not row zero. flip: remove
    /// the file matches' `sort_entries` → fails at "the most recently edited document
    /// is the one Enter opens". A plain `key_pressed` for `consume_key` stays green
    /// (it only moves the caret).
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

        // Unsteered, Enter takes the most recently edited document; stamps are set, as
        // two writes can land either side of a second.
        let stamp = |path: &std::path::Path, secs: u64| {
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
                .unwrap();
        };
        let enter_opens = |app: &mut OndinApp| {
            app.library.refresh();
            app.dash.search = Some("alp".into());
            app.dash.search_row = None;
            let _ = galleys(app, &ctx);
            press(app, &ctx, egui::Key::Enter);
            app.session.path.clone()
        };
        // *Alpine* edited later: it is first, though *Alpha* sorts first by name.
        stamp(&paths[0], 1_700_000_000);
        stamp(&paths[1], 1_700_086_400);
        assert_eq!(
            enter_opens(&mut app).as_deref(),
            Some(paths[1].as_path()),
            "the most recently edited document is the one Enter opens"
        );
        // The same second: the name decides, so no folder's order can.
        stamp(&paths[1], 1_700_000_000);
        assert_eq!(
            enter_opens(&mut app).as_deref(),
            Some(paths[0].as_path()),
            "and a tie goes to the name, whatever order the folder lists them in"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A modal marks the focused control only once a key has moved the focus (CSS's
    /// `:focus-visible`); the first and third assertions are the feature. flip: drop
    /// `&& !pointed` → fails the third; remove the visibility gate → fails the first
    /// (only at sixteen frames per call).
    #[test]
    fn a_modal_rings_the_focused_control_for_the_keyboard_and_not_for_the_mouse() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "focus-ring");
        app.dash.new_project = Some(NewProject::default());

        // Sixteen frames per call, events only on the first: the modal fades in, tinting the
        // ring until settled. Colour alone also matches the sidebar's *New file*
        // hairline; `StrokeKind::Outside` separates them.
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
        // A press on the backdrop (a Cancel click) hands it back to the pointer on
        // the same frame.
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

    /// File one document into a project, so it has a card on *Recent*
    /// (`OndinApp::recent_projects` hides a project with no files).
    fn seed_file(app: &mut OndinApp, root: &Path, id: &str, name: &str) {
        let project = app.library.projects.get(id).cloned().expect("seeded");
        let mut doc = ondin_core::Document::new(app.session.ids.mint());
        store::file_document(root, Some(&project), name, &mut doc).unwrap();
        app.library.refresh();
    }

    /// Moving the open document to another project survives the next save, which is
    /// the assertion. flip: drop the `patch` call from `open_document_follows` →
    /// fails at "the file the move produced still belongs to Atlas".
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
        app.session.adopt_document(doc, Some(path.clone()));

        app.dashboard_act(Act::MoveTo(path.clone(), Some("p-2".into())));
        let moved = app
            .session
            .path
            .clone()
            .expect("the session follows its file");
        assert_ne!(moved, path, "the move really moved it");

        store::write_document(&moved, &app.session.doc).unwrap();
        app.library.refresh();
        assert_eq!(
            app.library
                .entry_at(&moved)
                .and_then(|e| e.meta.project.clone()),
            Some("p-2".into()),
            "the file the move produced still belongs to Atlas"
        );
        // The mechanism: what the save wrote came from here.
        assert_eq!(
            app.session.doc.meta().project.as_deref(),
            Some("p-2"),
            "and it does because the open document was told about the move"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An archived project leaves the sidebar's *Projects* group for a collapsed
    /// group of its own, and leaves the body's project cards and the header's filter.
    /// flip: force the group open, or list every project in the sidebar → both fail
    /// at the same absence assertion (indistinguishable).
    #[test]
    fn an_archived_project_leaves_the_lists_for_a_collapsed_group_of_its_own() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "archived-sidebar");
        seed(&mut app, "p-1", "Kestrel", None);
        seed(&mut app, "p-2", "Field Notes", None);
        // A file each, or neither gets a card on *Recent*.
        seed_file(&mut app, &root, "p-1", "Landing v4");
        seed_file(&mut app, &root, "p-2", "Moodboard");
        app.dash.nav = Nav::Recent;

        let named = |app: &mut OndinApp, name: &str| {
            galleys(app, &ctx)
                .into_iter()
                .filter(|(_, _, t)| t == name)
                .count()
        };
        // Two runs: on *Recent* every project is a sidebar row and a body card.
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
        // Under its own heading: a count says the row exists, not where.
        let runs = galleys(&mut app, &ctx);
        assert!(
            run_at(&runs, "Field Notes").1 > run_at(&runs, "Archived projects").1,
            "the row belongs to the archived group, not to the Projects list above it"
        );

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

    /// *Save changes* writes all four fields, takes the project's folder and its files
    /// with the name, and drops a header filter the archive would make unreachable.
    /// flip: drop `forget_archived_filter` → fails at the filter assertion; discard
    /// `rename_folder`'s answer (`Ok(_) => before.folder.clone()`) → fails at the
    /// `folder` assertion.
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
        assert_eq!(
            project::Projects::read(&root).or_empty().projects[0],
            saved,
            "and all of it reached projects.json"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Both floating menus draw their rows through the shared `anchored_menu` shell;
    /// the filter half is a difference against the closed state. flip: never call
    /// `paint` → the file-menu assertions fail.
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

        // The ⋮ menu's verbs appear nowhere else, so they are asserted directly.
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

    /// The sidebar's Trash count is read from the cache, not scanned from the folder
    /// every frame: it survives the folder being deleted behind the app's back, and
    /// `refresh_trash` is what moves it.
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

        // Behind the app's back: a cached answer must not move.
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

        app.library.refresh_trash();
        assert_eq!(
            app.nav_count(&Nav::Trash),
            0,
            "a refresh is what moves it, and every writer of .trash owes one"
        );
    }

    /// A card with nothing edited writes nothing and rescans nothing; a changed card
    /// still writes. `projects.json`'s mtime is the witness, backdated. flip: disable
    /// `save_project`'s `differs_from` guard → fails at the first mtime assertion.
    #[test]
    fn a_card_with_nothing_edited_neither_writes_nor_rescans() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "edit-project-noop");
        seed(&mut app, "p-1", "Kestrel", Some("kestrel"));
        // `seed` writes nothing; the file must exist to be watched.
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

    /// *Edit project* is offered on a project's page and nowhere else, and only when
    /// the project resolves (unlike *Delete project* beside it). flip: fall back to a
    /// placeholder `Project` on a failed lookup → fails at the dangling-id assertion;
    /// hoist *Delete project* out of the `Nav::Project` arm → fails in the loop at
    /// the glyph.
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
            // The trash glyph too, or the sidebar satisfies the positive assertion below.
            assert!(
                !runs.iter().any(|(_, _, t)| t == icon::TRASH),
                "{nav:?} has no project to delete either"
            );
        }
        app.dash.nav = Nav::Project("p-1".into());
        let runs = galleys(&mut app, &ctx);
        assert!(runs.iter().any(|(_, _, t)| t == "Edit project"));
        // The trash glyph, since *Delete project*'s words are in a tooltip;
        // `icon::TRASH` differs from the sidebar row's `TRASH_SIMPLE`.
        assert!(
            runs.iter().any(|(_, _, t)| t == icon::TRASH),
            "beside the button it was split out of"
        );

        // Not on a nav holding an id `projects.json` no longer has; *Delete project*
        // deliberately stays.
        app.dash.nav = Nav::Project("0f9c2b7a4e".into());
        let runs = galleys(&mut app, &ctx);
        assert!(
            !runs.iter().any(|(_, _, t)| t == "Edit project"),
            "a project that cannot be looked up is not editable"
        );
        assert!(runs.iter().any(|(_, _, t)| t == icon::TRASH));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A rename commits on Enter and on a click away (`lost_focus()` is what commits).
    /// flip: restore `if !resp.has_focus() { resp.request_focus(); }` in place of the
    /// latch → fails at the first assertion (the field is still open).
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
    /// beside it alone. flip: justify into the whole 20pt box instead of a one-row
    /// rect → fails the y
    /// assertion; go back to `Ui::put` → fails the neighbour assertion.
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

    /// *Recent* ignores the sort (and does not offer the control), hides projects
    /// with no files, and shows a duplicate made from that page. flip: show every
    /// active project → fails the *Field Notes* count; drop `mark_opened` on a
    /// duplicate → fails the last assertion.
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
        // Opened in reverse alphabetical order, times set rather than stamped:
        // same-second stamps tie-break by stem, i.e. alphabetically anyway.
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

        // A project with a file draws twice (row and card), one without once.
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

        // The sort is neither applied nor offered here (one predicate for both).
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

    /// A save that fails on the way to the library does not block the walk and is
    /// marked on the library's status surface. The unwritable path is a directory,
    /// which fails on every platform.
    #[test]
    fn a_failed_save_walks_to_the_library_and_is_marked_there() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "save-fail");

        let doomed = root.join("not-a-file.ondin");
        std::fs::create_dir_all(&doomed).unwrap();
        app.session.path = Some(doomed.clone());
        app.view = crate::app::View::Editor;
        // Dirty, or `go_to_dashboard` has nothing to save.
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

    /// The header's controls change what you are looking at and no preference
    /// (`dashboard_page` and `dashboard_sort` stay put). flip: put `self.remember_dashboard()` back after the sort cycles → fails at
    /// the page assertion.
    #[test]
    fn the_header_controls_do_not_write_the_default_page() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "header-prefs");

        app.prefs.dashboard_page = "starred".into();
        app.prefs.dashboard_sort = "edited".into();
        app.dash.nav = Nav::Trash;
        let was = app.dash.sort;

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

    /// At half a screen the search field leaves the gear and the update chip their
    /// clicks, and at the default width it is centred on the window. flip:
    /// `search_rect` returning `SEARCH_W` wide on the bar's own centre → fails at
    /// "the gear opens Library settings" at 640.
    #[test]
    fn at_half_a_screen_the_search_field_leaves_the_gear_and_the_chip_their_clicks() {
        let find = |out: &egui::FullOutput, want: &dyn Fn(&str) -> bool| {
            out.shapes.iter().find_map(|cs| match &cs.shape {
                egui::Shape::Text(t) if want(&t.galley.job.text) => {
                    Some(t.pos + t.galley.size() / 2.0)
                }
                _ => None,
            })
        };
        for (width, staged) in [(640.0, false), (640.0, true), (760.0, true), (1320.0, true)] {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            let (mut app, root) = app(&ctx, &format!("narrow-{width}-{staged}"));
            if staged {
                app.updater = crate::update::Updater::ready("9.9.9");
            }
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 820.0));
            let run = |app: &mut OndinApp, events: Vec<egui::Event>| {
                let input = egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                };
                ctx.run_ui(input, |ui| app.dashboard_ui(ui))
            };
            let mut out = run(&mut app, vec![]);
            for _ in 0..3 {
                out = run(&mut app, vec![]);
            }
            let click = |app: &mut OndinApp, at: egui::Pos2| {
                let button = |pressed| egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                };
                run(app, vec![egui::Event::PointerMoved(at)]);
                run(app, vec![button(true)]);
                run(app, vec![button(false)]);
                run(app, vec![]);
            };

            // The field is the one stroked 28-point-high rect in the bar, under the
            // tab strip.
            let bar = crate::tabs::STRIP_H..crate::tabs::STRIP_H + TOP_H;
            let field = out.shapes.iter().find_map(|cs| match &cs.shape {
                egui::Shape::Rect(r)
                    if (r.rect.height() - 28.0).abs() < 0.01
                        && bar.contains(&r.rect.center().y) =>
                {
                    Some(r.rect)
                }
                _ => None,
            });
            let field = field.expect("the search field is drawn");
            if width >= 1320.0 {
                assert!(
                    (field.center().x - width / 2.0).abs() < 0.5,
                    "centred on the window: {field:?} in {width}"
                );
            }

            if staged {
                let chip = find(&out, &|t| t.contains("RESTART")).expect("the chip is drawn");
                assert!(
                    !field.contains(chip),
                    "{width}: the field lies over the chip"
                );
                click(&mut app, chip);
                assert_eq!(
                    app.dash.search, None,
                    "{width}: the chip does not open the search"
                );
            }
            let gear = find(&out, &|t| t == crate::theme::icon::SLIDERS).expect("the gear");
            click(&mut app, gear);
            assert!(
                app.library_settings.is_some(),
                "{width}: the gear opens Library settings (search: {:?})",
                app.dash.search
            );
            assert_eq!(app.dash.search, None);
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    /// *Make a kit* writes the mark; the *Kits* filter then lists it and the card
    /// wears the *Kit* chip; *Stop sharing as kit* asks first, and the confirmation
    /// (`set_kit_at` is called directly) keeps the mark and version. flip: `is_kit`
    /// answering for any mark → fails at "a mark that stopped sharing is not a kit";
    /// delete `file_menu_chose`'s `SetKit(_, false)` arm → fails at "the row opens the
    /// confirmation".
    #[test]
    fn a_card_makes_a_kit_the_filter_lists_it_and_stopping_asks_first() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "kits");
        let mark_in = |path: &Path| {
            ondin_core::io::load(&std::fs::read(path).unwrap())
                .unwrap()
                .meta()
                .kit
                .clone()
        };
        let mut a = ondin_core::Document::new(app.session.ids.mint());
        let atlas = store::file_document(&root, None, "Atlas", &mut a).unwrap();
        let mut b = ondin_core::Document::new(app.session.ids.mint());
        store::file_document(&root, None, "Landing", &mut b).unwrap();
        app.library.refresh();
        app.dash.nav = Nav::Kits;
        assert_eq!(app.visible_entries().len(), 0, "no kit yet");
        assert!(
            !galleys(&mut app, &ctx).iter().any(|(_, _, t)| t == "Kit"),
            "no chip yet"
        );

        app.dashboard_act(Act::SetKit(atlas.clone(), true));
        assert_eq!(
            mark_in(&atlas).map(|k| k.sharing),
            Some(true),
            "in the file"
        );
        assert_eq!(app.nav_count(&Nav::Kits), 1);
        let shown = app.visible_entries();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].path, atlas);
        let text = galleys(&mut app, &ctx);
        assert!(
            text.iter().any(|(_, _, t)| t == "Kit"),
            "the chip: {text:?}"
        );
        assert!(text.iter().any(|(_, _, t)| t == "Kits"), "the sidebar row");

        assert!(
            app.file_menu_chose(Act::SetKit(atlas.clone(), false), false)
                .is_none(),
            "the row opens the confirmation rather than acting"
        );
        assert_eq!(app.unsharing_kit, Some(Some(atlas.clone())));
        let text = galleys(&mut app, &ctx);
        assert!(
            text.iter()
                .any(|(_, _, t)| t == "Stop sharing Atlas as a kit?"),
            "{text:?}"
        );
        assert_eq!(
            mark_in(&atlas).map(|k| k.sharing),
            Some(true),
            "asking changed nothing"
        );
        app.unsharing_kit = None;
        app.set_kit_at(&atlas, false);
        assert_eq!(mark_in(&atlas).map(|k| k.sharing), Some(false), "stopped");
        assert_eq!(
            app.nav_count(&Nav::Kits),
            0,
            "a mark that stopped sharing is not a kit"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A kit's conflict copy is not a second kit: the *Kits* filter counts it once
    /// and the copy's menu offers no kit row. flip: `is_kit` without its conflict
    /// test → fails at "one kit"; `kit_row` without it → "no kit row on the copy".
    #[test]
    fn a_kits_conflict_copy_is_not_a_second_kit() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "kit-conflict");
        let mut a = ondin_core::Document::new(app.session.ids.mint());
        let atlas = store::file_document(&root, None, "Atlas", &mut a).unwrap();
        app.library.refresh();
        app.dashboard_act(Act::SetKit(atlas.clone(), true));
        let copy = root.join("atlas (Jon's conflicted copy 2026-10-08).ondin");
        std::fs::copy(&atlas, &copy).unwrap();
        app.library.refresh();
        assert_eq!(app.library.entries.len(), 2, "the fixture: both files");

        assert_eq!(app.nav_count(&Nav::Kits), 1, "one kit");
        let entry = |p: &Path| app.library.entry_at(p).unwrap().clone();
        assert!(
            app.kit_row(&entry(&copy)).is_none(),
            "no kit row on the copy"
        );
        assert_eq!(
            app.kit_row(&entry(&atlas)).map(|r| r.0),
            Some("Stop sharing as kit"),
            "the original's"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The kit mark in the file at `path`, read from disk.
    fn mark_on_disk(path: &Path) -> Option<ondin_core::meta::KitMark> {
        ondin_core::io::load(&std::fs::read(path).unwrap())
            .unwrap()
            .meta()
            .kit
            .clone()
    }

    /// A library holding a sharing kit *Atlas*; its path.
    fn shared_atlas(app: &mut OndinApp, root: &Path) -> PathBuf {
        let mut a = ondin_core::Document::new(app.session.ids.mint());
        let atlas = store::file_document(root, None, "Atlas", &mut a).unwrap();
        app.library.refresh();
        app.dashboard_act(Act::SetKit(atlas.clone(), true));
        atlas
    }

    /// A kit's conflict copy left as its only file is the kit on the dashboard too,
    /// as in the picker. flip: `stands_for_its_kit` false for every conflict copy →
    /// fails at "listed under Kits".
    #[test]
    fn a_kits_last_conflict_copy_is_the_kit() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "kit-last-copy");
        let atlas = shared_atlas(&mut app, &root);
        let copy = root.join("atlas (Jon's conflicted copy 2026-10-08).ondin");
        std::fs::copy(&atlas, &copy).unwrap();
        std::fs::remove_file(&atlas).unwrap();
        app.library.refresh();
        assert_eq!(app.library.entries.len(), 1, "the fixture: the copy alone");
        let picker = crate::kits::index::kits_in(&app.library, None);
        assert_eq!(picker.len(), 1, "fixture: the picker lists it");

        assert_eq!(app.nav_count(&Nav::Kits), 1, "listed under Kits");
        let entry = app.library.entry_at(&copy).unwrap().clone();
        assert_eq!(
            app.kit_row(&entry).map(|r| r.0),
            Some("Stop sharing as kit")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Two ordinary files of one kit (a copy made outside *Duplicate*) are one kit,
    /// the picker's file being the dashboard's. flip: `stands_for_its_kit` answering
    /// yes to every non-conflict file → fails at "one kit".
    #[test]
    fn two_files_of_one_kit_are_one_kit() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "kit-twice");
        let atlas = shared_atlas(&mut app, &root);
        let copy = root.join("atlas copy.ondin");
        std::fs::copy(&atlas, &copy).unwrap();
        app.library.refresh();
        assert_eq!(app.library.entries.len(), 2, "fixture: both files");
        assert_eq!(app.nav_count(&Nav::Kits), 1, "one kit");
        let picked = crate::kits::index::kits_in(&app.library, None);
        let stands = |p: &Path| app.kit_row(app.library.entry_at(p).unwrap()).is_some();
        assert_eq!(picked.len(), 1);
        assert!(
            stands(&picked[0].path),
            "the picker's file is the dashboard's"
        );
        let other = if picked[0].path == atlas {
            &copy
        } else {
            &atlas
        };
        assert!(!stands(other), "and the other gets no kit row");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A card's *Stop sharing* gives up the file still loading, or the load lands
    /// after the confirm and the next save shares the kit again. flip:
    /// `file_menu_chose` leaving `opening` → fails at "the load is given up".
    #[test]
    fn a_card_rows_card_gives_up_a_pending_open() {
        let ctx = egui::Context::default();
        let (mut app, root) = app(&ctx, "kit-pending");
        let atlas = shared_atlas(&mut app, &root);
        app.begin_open(&atlas);
        assert!(app.opening.is_some(), "fixture: the load is pending");
        assert!(
            app.file_menu_chose(Act::SetKit(atlas.clone(), false), false)
                .is_none()
        );
        assert!(app.opening.is_none(), "the load is given up");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// On the dashboard the file the editor still holds is closed: *Stop sharing*
    /// changes the file on disk (keeping another writer's newer publish) and the held
    /// copy's mark follows. flip: `set_kit_at` taking the editor's verbs for the held
    /// file → fails at "the newer publish is kept".
    #[test]
    fn stopping_a_kit_from_the_dashboard_keeps_a_newer_publish_on_disk() {
        let ctx = egui::Context::default();
        let (mut app, root) = app(&ctx, "kit-held");
        let atlas = shared_atlas(&mut app, &root);
        app.open_path(&atlas);
        app.go_to_dashboard();
        assert_eq!(app.view, crate::app::View::Dashboard);
        assert_eq!(
            app.session.path.as_deref(),
            Some(atlas.as_path()),
            "fixture: held"
        );

        let mut newer = ondin_core::io::load(&std::fs::read(&atlas).unwrap()).unwrap();
        let mut meta = newer.meta().clone();
        meta.kit.as_mut().unwrap().version = 7;
        newer.set_meta(meta);
        store::write_document(&atlas, &newer).unwrap();
        app.library.refresh();

        app.set_kit_at(&atlas, false);
        let mark = mark_on_disk(&atlas).unwrap();
        assert_eq!(mark.version, 7, "the newer publish is kept");
        assert!(!mark.sharing, "and only sharing turned off");
        let held = app.session.doc.meta().kit.clone().unwrap();
        assert_eq!(
            (held.version, held.sharing),
            (7, false),
            "the held copy follows"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// From the editor, *Stop sharing* counts over a fresh scan (a kit's own window
    /// never rescans). flip: remove the dialog's refresh → fails at "the new file is
    /// in the count's list".
    #[test]
    fn stop_sharing_from_the_editor_counts_over_a_fresh_scan() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, root) = app(&ctx, "kit-fresh");
        let atlas = shared_atlas(&mut app, &root);
        app.open_path(&atlas);
        assert_eq!(app.view, crate::app::View::Editor, "fixture: in the editor");
        let mut other = ondin_core::Document::new(app.session.ids.mint());
        store::file_document(&root, None, "Landing", &mut other).unwrap();
        assert_eq!(app.library.entries.len(), 1, "fixture: not scanned yet");

        app.unsharing_kit = Some(None);
        let _ = ctx.run_ui(Default::default(), |_| app.stop_sharing_dialog(&ctx));
        assert_eq!(
            app.library.entries.len(),
            2,
            "the new file is in the count's list"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
