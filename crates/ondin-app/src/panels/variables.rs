//! The styles and variables browser (§5.3f; §15 D1038, D1040).
//!
//! A panel beside the vertical toolbar, bottom-aligned with its swatches button
//! directly above the hexagon — so it and the insert picker **swap in place**, one
//! open at a time. `Shift+S` or the button opens it; `Escape` or the button
//! closes it, and a canvas selection leaves it open. Its search takes the keyboard
//! once, on opening, as the picker's does (§15 D1014).
//!
//! Build step 1 draws the *Variables* tab: a fixed name column and one column per
//! mode — 340 wide with one mode, 96 more per mode to three, the mode columns
//! scrolling sideways past that — variables grouped by the slash path in their
//! names, each with its usage count and its cells, edited in place. Build step 3
//! draws the *Styles* tab (§15 D1063): one group per card, inside it a group per
//! slash path as the variables have, each style's preview, name and count, and
//! its *Rename*, *Edit*, *Select* and *Delete*.

use crate::app::OndinApp;
use crate::panels::PaintSlot;
use crate::panels::picker::Picker;
use crate::theme::{self, color, icon};
use crate::ui::{self, MENU_ROW_H};
use eframe::egui;
use ondin_core::peniko::Color;
use ondin_core::style::{self as sty, Style, StyleId, StyleSlot};
use ondin_core::variable::{
    self as var, Cell, CollectionId, Field, Mode, ModeId, Scope, Value, VarId, VarKind, Variable,
};
use ondin_core::{Described, Document, NodeId, Operation, Transaction};
use std::collections::{BTreeMap, HashSet};

/// The name column, a mode column and the panel's padding: 340 with one
/// mode, 436 with two, 532 with three, the cap.
const NAME_W: f32 = 220.0;
const MODE_W: f32 = 96.0;
const PAD: f32 = ui::RAIL_POPUP_PAD;
const MAX_COLS: usize = 3;
/// The panel's height, the insert picker's and the assets browser's, shorter
/// only when the canvas has less room above the button.
const H: f32 = 460.0;
const SEARCH_H: f32 = 28.0;
const HEAD_H: f32 = 24.0;
const GROUP_H: f32 = 26.0;
const ROW_H: f32 = 28.0;
/// A variable row's name indent past its group's caret.
const INDENT: f32 = 23.0;
/// The *Name* header's inset, and an ungrouped variable's name's, which has no
/// caret to clear.
const LEAD: f32 = 6.0;
const SEARCH_ID: &str = "var-browser-search";

/// The panel's width for `modes` modes.
pub(crate) fn width(modes: usize) -> f32 {
    2.0 * PAD + NAME_W + MODE_W * modes.clamp(1, MAX_COLS) as f32
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Variables,
    Styles,
}

/// A field of the browser being typed into.
#[derive(Clone, Debug, PartialEq)]
enum Edit {
    /// A variable's whole name — a rename that types another group moves it.
    Name {
        var: VarId,
        text: String,
    },
    /// One cell: a value, or `--name` for an alias.
    Cell {
        var: VarId,
        mode: ModeId,
        text: String,
    },
    Mode {
        mode: ModeId,
        text: String,
    },
    /// A group's path, within its own collection.
    Group {
        collection: CollectionId,
        path: String,
        text: String,
    },
    /// A style's whole name, on the *Styles* tab (§15 D1063).
    Style {
        style: StyleId,
        text: String,
    },
    /// A style group's path, within its slot.
    StyleGroup {
        slot: StyleSlot,
        path: String,
        text: String,
    },
    /// A variable's or a style's description, typed across its whole row.
    Describe {
        of: Described,
        text: String,
    },
    /// The shown collection's name, typed over the picker.
    Collection {
        id: CollectionId,
        text: String,
    },
}

impl Edit {
    /// What the field holds.
    fn text(&self) -> &str {
        match self {
            Edit::Name { text, .. }
            | Edit::Cell { text, .. }
            | Edit::Collection { text, .. }
            | Edit::Mode { text, .. }
            | Edit::Group { text, .. }
            | Edit::Style { text, .. }
            | Edit::StyleGroup { text, .. }
            | Edit::Describe { text, .. } => text,
        }
    }
}

/// The menu open over the browser, and where.
#[derive(Clone, Debug, PartialEq)]
enum Menu {
    /// The `+` button's: *Colour variable*, *Number variable*, *Boolean variable*
    /// (§15 D1059), *String variable* (§15 D1069), *New mode*.
    Add,
    Var(VarId),
    Mode(ModeId),
    /// A group of a collection: *Rename group*, *Select*, *Delete group*.
    Group(CollectionId, String),
    /// *Delete mode…*'s confirmation, with the frames it returns to *Inherit*.
    ConfirmMode(ModeId),
    /// A style's row: *Rename*, *Edit*, *Select*, *Delete*.
    Style(StyleId),
    /// A style group's header: *Rename group*, *Select*, *Delete group*.
    StyleGroup(StyleSlot, String),
    /// A kit's mapping for one of the document's modes (D1038 (b), §15 D1065):
    /// the kit's modes, then *Match by name*.
    KitMap(String, ModeId),
    /// The picker's row menu: *Rename collection*, *Duplicate collection*,
    /// *Select n layers*, *Delete collection…*.
    Collection(CollectionId),
    /// *Delete collection…*'s confirmation, saying what goes.
    ConfirmCollection(CollectionId),
    /// A variable's scope: the field families it may bind, ticked.
    Scope(VarId),
    /// *Bind other uses…*: the fields drawing a variable's value, ticked, and
    /// bound together (`bind_others`).
    Bind(VarId),
    /// The words a flex direction's or wrap's string may hold, for one cell.
    Words(VarId, ModeId),
}

/// The browser's own state while it is open.
#[derive(Default)]
pub(crate) struct Browser {
    tab: Tab,
    query: String,
    /// Whether the search should take the keyboard next frame — once, on open.
    focus: bool,
    /// *In selection*, kept pressed across selection changes.
    pub(crate) in_selection: bool,
    /// The Styles tab's *Unused*: only the document's own styles no layer uses.
    pub(crate) unused: bool,
    /// Collapsed group paths — a copy of the document's own set
    /// (`OndinApp::var_collapsed`), loaded for [`Self::root`] (§15 D1054).
    collapsed: HashSet<String>,
    /// The root of the document `collapsed` was loaded for.
    root: Option<NodeId>,
    edit: Option<Edit>,
    /// Whether the field `edit` names still has to take the keyboard.
    edit_focus: bool,
    menu: Option<(Menu, egui::Pos2)>,
    /// The menu drawn last frame, so its measured size places it from then on.
    menu_drawn: Option<Menu>,
    /// A variable menu's *Move to ▸* row while its list of collections is open
    /// beside it.
    move_to: Option<egui::Rect>,
    /// The sentence a refused edit is answered with, until the next edit.
    refusal: Option<String>,
    /// The text that refusal answered, which the field draws dim and dotted
    /// while it still holds it, as the inspector's fields do (§15 D1053).
    refused: Option<String>,
    /// A mode's minimum width while its field in the mode menu is dragged, until
    /// the drag ends and commits it.
    width_draft: Option<(ModeId, f64)>,
    /// How far the mode columns are scrolled sideways, past three modes.
    scroll_x: f32,
    /// A drag on the sideways bar's thumb: the pointer's x and `scroll_x` where
    /// it pressed (§15 D1055).
    bar_drag: Option<(f32, f32)>,
    /// The table's width this frame. **Handed to the rows rather than asked of
    /// them**: inside the scroll area of an `Area` that sizes to its content,
    /// `available_width` has nothing to measure against and answers NaN.
    inner_w: f32,
    /// The panel's rect as last drawn.
    pub(crate) panel: Option<egui::Rect>,
    /// After *Extract into variable…*: the variable made and the other fields
    /// drawing its value — the offer line under its row while it shows.
    pub(crate) offer: Option<super::bind_others::Offer>,
    /// The document revision `offer` was counted at; counted again when it
    /// moves, and gone once nothing else draws the value.
    pub(crate) offer_rev: u64,
    /// *Bind other uses*'s list while its popover is open.
    bind: Option<BindList>,
    /// The layer whose row in that list is under the pointer, outlined on the
    /// canvas as a Layers row's is. Set while drawn, read by the next frame's
    /// canvas, which draws first.
    pub(crate) bind_hover: Option<NodeId>,
    /// The variable menu's count of other uses, by variable and document
    /// revision: the walk is the whole document's, too dear for every frame.
    uses_count: Option<(VarId, u64, usize)>,
}

/// *Bind other uses*'s list: the fields found when it opened, and which are
/// ticked.
struct BindList {
    var: VarId,
    uses: Vec<super::bind_others::Use>,
    on: Vec<bool>,
}

impl Browser {
    /// Open variable `var`'s name for typing — after *Extract into variable…*,
    /// which files the variable under a free name for the user to type over.
    pub(crate) fn start_rename(&mut self, var: VarId, name: String) {
        self.tab = Tab::Variables;
        self.edit = Some(Edit::Name { var, text: name });
        self.edit_focus = true;
        self.refusal = None;
        self.refused = None;
    }

    /// Whether something inside the browser answers `Escape` before the browser
    /// does — a field being typed into, an open menu.
    pub(crate) fn escape(&mut self) -> bool {
        if self.menu.take().is_some() {
            return true;
        }
        if self.edit.take().is_some() {
            self.refusal = None;
            self.refused = None;
            return true;
        }
        false
    }
}

/// What the browser was asked to do this frame, applied once it has drawn.
enum Act {
    NewVar(VarKind),
    NewMode,
    Name(VarId, String),
    /// A typed cell, and the cell to type into next when it lands — `Tab`'s.
    /// One act rather than a commit and an edit, so a refused value keeps its
    /// field instead of the next cell's taking it over in silence.
    Cell(VarId, ModeId, String, Option<Edit>),
    ModeName(ModeId, String),
    /// A collection's group renamed, from one path to another.
    GroupName(CollectionId, String, String),
    Duplicate(VarId),
    DuplicateMode(ModeId),
    /// A mode moved to an index of the list; index 0 makes it the default.
    MoveMode(ModeId, usize),
    Delete(Vec<VarId>, String),
    DeleteMode(ModeId),
    Select(Vec<NodeId>),
    Toggle(String),
    /// `Alt`+click on a caret (D1038 G-series, §15 D1054): every group takes the
    /// state this one's click gives it — all open, or all closed.
    ToggleAll(String),
    StartEdit(Edit),
    OpenMenu(Menu, egui::Pos2),
    /// A colour cell's colour popup, opened beside the panel at this point
    /// (D1038 G-series, §15 D1052).
    OpenPicker(VarId, ModeId, egui::Pos2),
    /// A style renamed, its *Edit* opened, deleted (§15 D1063).
    StyleName(StyleId, String),
    EditStyle(StyleId),
    /// Styles deleted — one, or a group's — and the name the status gives them.
    DeleteStyles(Vec<StyleId>, String),
    /// A style group renamed within its slot.
    StyleGroupName(StyleSlot, String, String),
    /// A variable's or a style's description typed.
    Describe(Described, String),
    /// `Alt`+click on a style group's caret: every style group takes the state
    /// this one's click gives it.
    ToggleAllStyles(String),
    /// A kit's mapping entry for a document mode set to a kit mode, or cleared
    /// to match by name (§15 D1065).
    KitMap(String, ModeId, Option<ModeId>),
    /// A kit header's *v8 → v9* clicked: the update summary opened.
    KitUpdates,
    /// The `+` menu's tokens rows, and the linked file's line (its review).
    ImportTokens,
    ExportTokens,
    UnlinkTokens,
    LinkedTokens,
    /// The picker's pick: the collection the browser shows.
    ShowCollection(CollectionId),
    NewCollection,
    CollectionName(CollectionId, String),
    DuplicateCollection(CollectionId),
    DeleteCollection(CollectionId),
    /// *Pick mode by width* ticked or unticked on a collection.
    ByWidth(CollectionId, bool),
    /// A mode's minimum width, typed in its menu.
    Breakpoint(ModeId, f64),
    /// *Move to*: variables into a collection, or into a new one.
    MoveTo(VarId, Option<CollectionId>),
    /// A scope ticked: the variable's new scope, `None` every field.
    Scope(VarId, Option<Scope>),
    /// *Bind other uses*'s *Bind*: these fields bound to the variable.
    BindUses(VarId, Vec<super::binding::FieldKey>),
    /// The offer line's *Bind N other fields*: what the list would start with
    /// ticked, bound without opening it.
    BindReady(VarId),
    /// A row of that list double-clicked: its layer selected and shown.
    Reveal(NodeId),
}

/// One row of the table as drawn: a group's header or a variable, or a kit's
/// header above its own (§15 D1065).
enum Line<'a> {
    Group {
        path: &'a str,
        shown: usize,
        total: usize,
        of: GroupOf<'a>,
    },
    Var(&'a Variable),
    Kit(&'a str),
    /// A collection's header in a search spanning them, over its own rows,
    /// which read its modes.
    Collection(&'a ondin_core::variable::Collection),
}

/// Whose group a header is: one of the document's collections, or a kit's,
/// which only folds.
#[derive(Clone, Copy, Debug, PartialEq)]
enum GroupOf<'a> {
    Own(CollectionId),
    Kit(&'a str),
}

impl GroupOf<'_> {
    /// The collection a group of the document's own is in; `None` for a kit's.
    fn own(self) -> Option<CollectionId> {
        match self {
            GroupOf::Own(c) => Some(c),
            GroupOf::Kit(_) => None,
        }
    }
}

/// The key a group's collapse is kept under: its path prefixed by its
/// collection's id, or by `\u{2}` and a kit's id, so a *color* group folds
/// apart in each collection and in each kit (§15 D1065, D1054's per-document set).
fn group_key(of: GroupOf<'_>, path: &str) -> String {
    match of {
        GroupOf::Own(c) => format!("\u{3}{}\u{3}{path}", c.0.to_wire()),
        GroupOf::Kit(k) => format!("\u{2}{k}\u{2}{path}"),
    }
}

/// The key a document's collapsed groups are kept under in
/// `OndinApp::var_collapsed`: its lowest page id, which a file keeps across saves
/// and loads and a page reorder does not move, as it moves the first page.
fn collapsed_key(doc: &ondin_core::Document) -> NodeId {
    doc.pages()
        .iter()
        .map(|p| p.id)
        .min()
        .unwrap_or_else(|| doc.first_page())
}

impl OndinApp {
    /// Open the browser with its search taking the keyboard, or close it
    /// (`Shift+S`, the swatches button). Opening it closes the insert picker:
    /// the two share a place.
    pub(crate) fn toggle_var_browser(&mut self) {
        self.var_browser = match self.var_browser {
            Some(_) => None,
            None => {
                self.kit_picker = None;
                self.asset_panel = None;
                // Loaded at the open, not on the first frame, so nothing done
                // before that frame is overwritten by the load (§15 D1054).
                let root = collapsed_key(&self.session.doc);
                Some(Browser {
                    focus: true,
                    collapsed: self.var_collapsed.get(&root).cloned().unwrap_or_default(),
                    root: Some(root),
                    ..Browser::default()
                })
            }
        };
    }

    /// The collection the browser shows — what *New variable* and *Extract
    /// into variable…* file into: the one picked for this document, else its
    /// first.
    pub(crate) fn shown_collection(&self) -> CollectionId {
        let t = self.session.doc.variables();
        self.var_collection
            .get(&collapsed_key(&self.session.doc))
            .copied()
            .filter(|c| t.collection(*c).is_some())
            .unwrap_or(t.first_collection().id)
    }

    /// Open the browser on `var`'s collection with its scope popover open — the
    /// `--` list's *Scope…*.
    pub(crate) fn open_scope(&mut self, var: VarId) {
        self.open_var_popover(var, Menu::Scope(var));
    }

    /// Open the browser on `var` with *Bind other uses…*'s popover open, as its
    /// menu row does — for the tests and the audit, which draw no menu first.
    #[cfg(test)]
    pub(crate) fn open_bind_others(&mut self, var: VarId) {
        self.open_var_popover(var, Menu::Bind(var));
    }

    /// Open the browser on `var`'s collection with `menu` beside the panel.
    fn open_var_popover(&mut self, var: VarId, menu: Menu) {
        let Some(c) = self
            .session
            .doc
            .variables()
            .collection_of(var)
            .map(|c| c.id)
        else {
            return;
        };
        self.var_collection
            .insert(collapsed_key(&self.session.doc), c);
        if self.var_browser.is_none() {
            self.toggle_var_browser();
        }
        if let Some(b) = self.var_browser.as_mut() {
            b.tab = Tab::Variables;
            b.focus = false;
            // Beside the panel, which has not drawn yet when it just opened.
            let at = b
                .panel
                .map_or(self.canvas_rect.left_top() + egui::vec2(60.0, 60.0), |p| {
                    p.right_top() + egui::vec2(ui::MENU_GAP, 0.0)
                });
            b.menu = Some((menu, at));
        }
    }

    /// Draw the browser when it is open, and do what it was asked to.
    pub(crate) fn var_browser_ui(&mut self, ctx: &egui::Context) {
        // Nowhere to stand until the canvas has been laid out once — its rect is
        // `NOTHING` before, and a panel anchored at infinity lays out at NaN.
        if !self.canvas_rect.is_finite() {
            return;
        }
        let Some(mut b) = self.var_browser.take() else {
            return;
        };
        // Set again by the bind list while a row of it is under the pointer;
        // its list is read afresh each time the popover opens.
        b.bind_hover = None;
        if !matches!(b.menu, Some((Menu::Bind(_), _))) {
            b.bind = None;
        }
        let rev = self.session.revision();
        if let Some(o) = b.offer
            && b.offer_rev != rev
        {
            b.offer = self.offer_of(o.var);
            b.offer_rev = rev;
        }
        // **Collapsed groups are the document's, not the panel's** (D1038
        // G-series): loaded from the app's per-document sets ([`collapsed_key`])
        // whenever the document under the browser is not the one it loaded for,
        // and written back after the frame's acts.
        let root = collapsed_key(&self.session.doc);
        if b.root != Some(root) {
            b.collapsed = self.var_collapsed.get(&root).cloned().unwrap_or_default();
            b.root = Some(root);
        }
        let pending = self.var_cell_preview;
        let shown = self.shown_collection();
        // The kits with an update waiting, for their headers' *v8 → v9*.
        let waiting: Vec<(String, u32, u32)> = self
            .kits_waiting(false)
            .into_iter()
            .map(|w| (w.kit.id, w.held, w.kit.version))
            .collect();
        let doc = &self.session.doc;
        let table = doc.variables();
        let modes: Vec<Mode> = table
            .collection(shown)
            .map(|c| c.modes.clone())
            .unwrap_or_default();
        let widths = table.collection(shown).and_then(|c| c.breakpoints.clone());
        let anchor = self.rail_popup_anchor(self.var_button);
        let height = H.min((anchor.y - self.canvas_rect.top() - 8.0).max(160.0));
        let w = width(modes.len());
        b.inner_w = w - 2.0 * PAD;

        // Who reads what, once a frame: the usage counts and *In selection*.
        // Never a vault node, which is never selected (§15 D1012): a kit's
        // frozen mains read its variables and styles by rule.
        let mut users: BTreeMap<VarId, Vec<NodeId>> = BTreeMap::new();
        for v in table.iter() {
            let mut who = var::users(doc, v.id);
            who.retain(|u| !doc.in_vault(*u));
            users.insert(v.id, who);
        }
        let picked: Option<HashSet<VarId>> = b.in_selection.then(|| {
            let ids = ondin_core::subtree_nodes(doc, self.session.selection.ids());
            table
                .iter()
                .filter(|v| users[&v.id].iter().any(|u| ids.contains(u)))
                .map(|v| v.id)
                .collect()
        });
        // And the styles' (§15 D1063), for the *Styles* tab.
        let mut style_users: BTreeMap<StyleId, Vec<NodeId>> = BTreeMap::new();
        for s in doc.styles().iter() {
            let mut who = sty::users(doc, s.id);
            who.retain(|u| !doc.in_vault(*u));
            style_users.insert(s.id, who);
        }
        let unused = unused_styles(doc, &style_users);
        let style_picked: Option<HashSet<StyleId>> = (b.in_selection || b.unused).then(|| {
            let ids = ondin_core::subtree_nodes(doc, self.session.selection.ids());
            doc.styles()
                .iter()
                .filter(|s| !b.in_selection || style_users[&s.id].iter().any(|u| ids.contains(u)))
                .filter(|s| !b.unused || unused.contains(&s.id))
                .map(|s| s.id)
                .collect()
        });
        let query = b.query.trim().to_lowercase();
        // A search, and *In selection*, span every collection.
        let across = table.has_collections() && (!query.is_empty() || b.in_selection);
        let lines = all_lines(doc, shown, across, &query, picked.as_ref(), &b.collapsed);
        let selection_line = b.in_selection.then(|| {
            let (n, one, many) = match b.tab {
                Tab::Variables => (
                    picked.as_ref().map_or(0, HashSet::len),
                    "variable",
                    "variables",
                ),
                Tab::Styles => (
                    style_picked.as_ref().map_or(0, HashSet::len),
                    "style",
                    "styles",
                ),
            };
            let ids = self.session.selection.ids();
            match ids {
                [] => "Nothing selected".to_string(),
                _ => {
                    let who = match ids {
                        [one] => crate::private::layer(doc, *one).into_owned(),
                        many => format!("{} layers", many.len()),
                    };
                    match n {
                        0 => format!("{who} uses no {many}"),
                        1 => format!("Used by {who} · 1 {one}"),
                        n => format!("Used by {who} · {n} {many}"),
                    }
                }
            }
        });

        let mut acts: Vec<Act> = Vec::new();
        let panel = ui::rail_popup(ctx, "var-browser", anchor, height, |ui| {
            ui.set_width(w - 2.0 * PAD);
            ui.set_height(height - 2.0 * PAD);
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 10.0);
            tabs(ui, &mut b.tab);
            search_row(ui, doc, shown, &mut b, &mut acts);
            if let Some(link) = doc.tokens_link()
                && linked_line(ui, &link.path, self.tokens_watch.changes)
            {
                acts.push(Act::LinkedTokens);
            }
            if let Some(line) = &selection_line {
                // Two points further off the search row than it
                // sat, at the maintainer's look (§15 D1086).
                ui.add_space(-6.0);
                ui.label(
                    egui::RichText::new(line)
                        .size(ui::CAPTION_PT)
                        .color(theme::text::dim()),
                );
            }
            if b.unused && matches!(b.tab, Tab::Styles) {
                // What the list shows, so the button takes no style the
                // search or *In selection* hides.
                let shown: Vec<StyleId> = unused
                    .iter()
                    .copied()
                    .filter(|id| style_picked.as_ref().is_none_or(|p| p.contains(id)))
                    .filter(|id| {
                        query.is_empty()
                            || doc
                                .styles()
                                .get(*id)
                                .is_some_and(|s| s.name.to_lowercase().contains(&query))
                    })
                    .collect();
                ui.add_space(-6.0);
                if unused_line(ui, shown.len()) {
                    let name = ui::count(shown.len(), "unused style");
                    acts.push(Act::DeleteStyles(shown, name));
                }
            }
            if let Some(why) = &b.refusal {
                ui.add_space(-6.0);
                ui.label(
                    egui::RichText::new(why)
                        .size(ui::SMALL_PT)
                        .color(theme::text::muted()),
                );
            }
            match b.tab {
                Tab::Styles if doc.styles().is_empty() => empty(
                    ui,
                    "No styles yet",
                    "Use the … menu on any inspector card and choose Create style…",
                ),
                Tab::Styles => styles_tab(
                    ui,
                    doc,
                    &style_users,
                    style_picked.as_ref(),
                    &query,
                    &mut b,
                    &mut acts,
                ),
                Tab::Variables if table.iter().next().is_none() => empty(
                    ui,
                    "No variables yet",
                    "Add a colour or a number with + above, then type -- in a field to bind it.",
                ),
                Tab::Variables if lines.is_empty() && query.is_empty() && !b.in_selection => empty(
                    ui,
                    "No variables in this collection",
                    "Add one with + above, or move one here from its menu.",
                ),
                Tab::Variables if lines.is_empty() => empty(
                    ui,
                    "No variables match",
                    if b.in_selection {
                        "Turn off In selection to see them all."
                    } else {
                        "Try another name."
                    },
                ),
                Tab::Variables => {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    // Spanning, each collection heads its own rows,
                    // so the columns are never shared across them.
                    if !across {
                        header(ui, &modes, widths.as_ref(), &mut b, &mut acts);
                    }
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.add_space(4.0);
                            let mut row_modes: &[Mode] = &modes;
                            for line in &lines {
                                match line {
                                    Line::Collection(c) => {
                                        row_modes = &c.modes;
                                        collection_row(ui, c);
                                        continue;
                                    }
                                    Line::Kit(_) => row_modes = &modes,
                                    _ => {}
                                }
                                let modes = row_modes;
                                match line {
                                    Line::Collection(_) => {}
                                    Line::Group {
                                        path,
                                        shown,
                                        total,
                                        of,
                                    } => group_row(
                                        ui,
                                        path,
                                        *of,
                                        *shown,
                                        *total,
                                        picked.is_some(),
                                        &mut b,
                                        &mut acts,
                                    ),
                                    Line::Var(v) if v.origin.is_some() => {
                                        kit_var_row(ui, doc, v, modes, &users[&v.id], &b, &mut acts)
                                    }
                                    Line::Var(v) => {
                                        var_row(
                                            ui,
                                            doc,
                                            v,
                                            modes,
                                            &users[&v.id],
                                            &mut b,
                                            &mut acts,
                                            pending,
                                        );
                                        if let Some(o) = b.offer
                                            && o.var == v.id
                                        {
                                            offer_line(ui, doc, v, o, b.inner_w, &mut acts);
                                        }
                                    }
                                    Line::Kit(k) => kit_row(
                                        ui,
                                        doc,
                                        k,
                                        modes,
                                        waiting.iter().find(|w| w.0 == *k).map(|w| (w.1, w.2)),
                                        &b,
                                        &mut acts,
                                    ),
                                }
                            }
                            ui::keep_focus_in_view(ui);
                        });
                }
            }
        })
        .rect;
        b.panel = Some(panel);

        // Past three modes the columns scroll sideways under the fixed name
        // column, by a horizontal wheel or `Shift` and the wheel.
        // Only where there are columns to scroll: the *Variables* tab with rows
        // on it — not the *Styles* tab nor an empty state (§15 D1055).
        let columns = matches!(b.tab, Tab::Variables) && !lines.is_empty();
        let over = ctx.pointer_hover_pos().is_some_and(|p| panel.contains(p)) && columns;
        if over && modes.len() > MAX_COLS {
            let (dx, dy, shift) = ctx.input(|i| {
                (
                    i.smooth_scroll_delta.x,
                    i.smooth_scroll_delta.y,
                    i.modifiers.shift,
                )
            });
            let d = if dx != 0.0 {
                dx
            } else if shift {
                dy
            } else {
                0.0
            };
            let max = MODE_W * (modes.len() - MAX_COLS) as f32;
            b.scroll_x = (b.scroll_x - d).clamp(0.0, max);
        } else if modes.len() <= MAX_COLS {
            b.scroll_x = 0.0;
        }
        if columns && modes.len() > MAX_COLS {
            scroll_bar(ctx, panel, modes.len(), &mut b);
        } else {
            b.bar_drag = None;
        }

        match b.menu.clone() {
            Some((menu, at)) => self.var_menu(ctx, &mut b, menu, at, &users, &mut acts),
            None => b.move_to = None,
        }

        self.var_browser = Some(b);
        for act in acts {
            self.var_act(act);
        }
        // The collapsed groups written through to the document's own set, so
        // they outlive the browser and follow the document (§15 D1054).
        let root = collapsed_key(&self.session.doc);
        if let Some(b) = &self.var_browser
            && self.var_collapsed.get(&root) != Some(&b.collapsed)
        {
            self.var_collapsed.insert(root, b.collapsed.clone());
        }
    }

    /// The open menu, drawn over the browser; a press anywhere else closes it.
    fn var_menu(
        &self,
        ctx: &egui::Context,
        b: &mut Browser,
        menu: Menu,
        at: egui::Pos2,
        users: &BTreeMap<VarId, Vec<NodeId>>,
        acts: &mut Vec<Act>,
    ) {
        let doc = &self.session.doc;
        let table = doc.variables();
        let menu_w = match &menu {
            Menu::ConfirmMode(_) | Menu::ConfirmCollection(_) => 240.0,
            Menu::Scope(_) | Menu::Bind(_) => SCOPE_W,
            _ => 200.0,
        };
        let modes_of = |m: ModeId| table.collection_of_mode(m).map_or(0, |c| c.modes.len());
        // A by-width mode past the first carries its width's field, a control's
        // height and a gap.
        let width_field = |m: ModeId| match table.collection_of_mode(m) {
            Some(c) if c.by_width() && c.default_mode() != Some(m) => {
                (ui::CONTROL_H + 1.0) / MENU_ROW_H
            }
            _ => 0.0,
        };
        let rows = match &menu {
            Menu::Add => 7.0,
            Menu::Var(_) => 8.0,
            Menu::Bind(_) => BIND_LIST_ROWS + 5.0,
            Menu::Mode(m) if modes_of(*m) > 1 => 6.5 + width_field(*m),
            Menu::Mode(m) => 3.0 + width_field(*m),
            Menu::Collection(_) => 5.5,
            Menu::ConfirmCollection(_) => 4.0,
            Menu::Scope(v) => scope_rows(table.get(*v)),
            Menu::Group(..) => 3.0,
            Menu::ConfirmMode(_) => 3.0,
            Menu::Style(_) => 5.0,
            Menu::StyleGroup(..) => 3.0,
            Menu::KitMap(kit, _) => table.kits().get(kit).map_or(0, |k| k.modes.len()) as f32 + 1.5,
            Menu::Words(v, _) => table.get(*v).map_or(0, |v| var::flex_words(v).len()) as f32,
        };
        // The estimate places only a menu's first frame; from then on it is placed
        // by the size it painted at, since its rows come and go with what it acts on.
        let estimate = egui::vec2(menu_w, rows * MENU_ROW_H + 24.0);
        let area = egui::Id::new("var-browser-menu");
        let size = match b.menu_drawn.as_ref() == Some(&menu) {
            true => ui::painted_size(ctx, area).unwrap_or(estimate),
            false => estimate,
        };
        b.menu_drawn = Some(menu.clone());
        let pos = ui::menu_place(ctx, at, size);
        let mut close = false;
        let rect = egui::Area::new(area)
            .order(egui::Order::Tooltip)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                ui::menu_frame(ui::MENU_PAD).show(ui, |ui| {
                    ui.set_width(ui::menu_inner_w(menu_w, ui::MENU_PAD));
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let row = |ui: &mut egui::Ui, glyph: &str, label: &str| {
                        ui::menu_row(ui, ui::MenuRow::new(glyph, label), MENU_ROW_H).clicked()
                    };
                    match &menu {
                        Menu::Add => {
                            if row(ui, icon::PALETTE, "Colour variable") {
                                acts.push(Act::NewVar(VarKind::Color));
                                close = true;
                            }
                            if row(ui, icon::HASH, "Number variable") {
                                acts.push(Act::NewVar(VarKind::Number));
                                close = true;
                            }
                            // Step 2's: a boolean binds a layer's visibility
                            // (D1038 (d), §15 D1059).
                            if row(ui, icon::TOGGLE, "Boolean variable") {
                                acts.push(Act::NewVar(VarKind::Boolean));
                                close = true;
                            }
                            // And step 6's: a string binds a text's content and
                            // its family (D1038 (d), §15 D1069).
                            if row(ui, icon::TEXT_T, "String variable") {
                                acts.push(Act::NewVar(VarKind::String));
                                close = true;
                            }
                            ui::menu_sep(ui);
                            if row(ui, icon::COLUMNS_PLUS_RIGHT, "New mode") {
                                acts.push(Act::NewMode);
                                close = true;
                            }
                            if row(ui, icon::DATABASE, "New collection") {
                                acts.push(Act::NewCollection);
                                close = true;
                            }
                            ui::menu_sep(ui);
                            if row(ui, icon::IMPORT, "Import tokens…") {
                                acts.push(Act::ImportTokens);
                                close = true;
                            }
                            if row(ui, icon::EXPORT, "Export tokens…") {
                                acts.push(Act::ExportTokens);
                                close = true;
                            }
                            if let Some(link) = doc.tokens_link() {
                                let name =
                                    std::path::Path::new(&link.path).file_name().map_or_else(
                                        || "tokens file".to_string(),
                                        |n| n.to_string_lossy().into_owned(),
                                    );
                                if row(ui, icon::LINK_BREAK, &format!("Unlink {name}")) {
                                    acts.push(Act::UnlinkTokens);
                                    close = true;
                                }
                            }
                        }
                        Menu::Var(v) => {
                            let Some(v) = table.get(*v) else {
                                close = true;
                                return;
                            };
                            // Any row but *Move to* hovered closes its list.
                            let mut elsewhere = false;
                            let mut item = |ui: &mut egui::Ui, row: ui::MenuRow<'_>| {
                                let r = ui::menu_row(ui, row, MENU_ROW_H);
                                elsewhere |= r.hovered();
                                r
                            };
                            if item(ui, ui::MenuRow::new(icon::CURSOR_TEXT, "Rename")).clicked() {
                                acts.push(Act::StartEdit(Edit::Name {
                                    var: v.id,
                                    text: v.name.clone(),
                                }));
                                close = true;
                            }
                            if item(ui, ui::MenuRow::new(icon::COPY_SIMPLE, "Duplicate")).clicked()
                            {
                                acts.push(Act::Duplicate(v.id));
                                close = true;
                            }
                            if item(ui, ui::MenuRow::new(icon::NOTE, "Description")).clicked() {
                                acts.push(Act::StartEdit(Edit::Describe {
                                    of: Described::Variable(v.id),
                                    text: v.description.clone(),
                                }));
                                close = true;
                            }
                            // A boolean binds visibility and nothing else, so
                            // it has no scope to narrow.
                            let boolean = v.kind == VarKind::Boolean;
                            let scope = item(
                                ui,
                                ui::MenuRow::new(icon::TARGET, "Scope…").enabled(!boolean),
                            );
                            let scope = match boolean {
                                true => scope.on_hover_text("A boolean binds visibility only."),
                                false => scope,
                            };
                            // Not `close`: the popover takes the menu's place.
                            if scope.clicked() && !boolean {
                                b.menu = Some((Menu::Scope(v.id), at));
                            }
                            // Lit while its list is open, as the View menu's
                            // *Panels ▸* is; hover or a click opens it.
                            let held = b.move_to.is_some();
                            let move_to = ui::submenu_row(
                                ui,
                                ui::MenuRow::new(icon::DATABASE, "Move to"),
                                held,
                            );
                            let move_row = (move_to.hovered() || move_to.clicked() || held)
                                .then_some(move_to.rect);
                            let n = users.get(&v.id).map_or(0, Vec::len);
                            let select = item(
                                ui,
                                ui::MenuRow::new(icon::SELECTION_ALL, &layers_label(n))
                                    .enabled(n > 0),
                            );
                            if select.clicked() {
                                acts.push(Act::Select(users[&v.id].clone()));
                                close = true;
                            }
                            // The fields drawing its value, counted once per
                            // revision; a kit's variable is the kit's to scope.
                            if v.origin.is_none() {
                                let rev = self.session.revision();
                                let n = match b.uses_count {
                                    Some((x, r, n)) if x == v.id && r == rev => n,
                                    _ => {
                                        let n = self.other_uses(v.id).len();
                                        b.uses_count = Some((v.id, rev, n));
                                        n
                                    }
                                };
                                let count = n.to_string();
                                let bind = item(
                                    ui,
                                    ui::MenuRow::new(icon::LINK_SIMPLE, "Bind other uses…")
                                        .accel((n > 0).then_some(count.as_str()))
                                        .enabled(n > 0),
                                )
                                .on_disabled_hover_text("No other field draws this value");
                                // Not `close`: the popover takes the menu's place.
                                if bind.clicked() && n > 0 {
                                    b.menu = Some((Menu::Bind(v.id), at));
                                }
                            }
                            ui::menu_sep(ui);
                            let del =
                                item(ui, ui::MenuRow::new(icon::TRASH, "Delete").danger(true));
                            if del.clicked() {
                                acts.push(Act::Delete(vec![v.id], v.name.clone()));
                                close = true;
                            }
                            b.move_to = move_row.filter(|_| !elsewhere);
                        }
                        Menu::Mode(m) => {
                            let Some(mode) = table.mode(*m) else {
                                close = true;
                                return;
                            };
                            if row(ui, icon::CURSOR_TEXT, "Rename mode") {
                                acts.push(Act::StartEdit(Edit::Mode {
                                    mode: mode.id,
                                    text: mode.name.clone(),
                                }));
                                close = true;
                            }
                            let i = table.mode_index(mode.id).unwrap_or(0);
                            // A by-width mode's minimum, past the first's fixed 0:
                            // a drag or a typed number held as a draft, committed
                            // when the drag ends or the number lands.
                            let by_width = table
                                .collection_of_mode(mode.id)
                                .is_some_and(|c| c.by_width());
                            if let Some(min) = table
                                .collection_of_mode(mode.id)
                                .and_then(|c| c.min_width(mode.id))
                                .filter(|_| i > 0)
                            {
                                // A right-click cancel drops the draft, and is
                                // answered first: egui may still report a change or
                                // a drag stop on that frame, which would commit it.
                                let cancelled = self.gesture_cancelled;
                                if cancelled {
                                    b.width_draft = None;
                                }
                                let mut w = match b.width_draft {
                                    Some((m, w)) if m == mode.id => w,
                                    _ => min,
                                };
                                let size = egui::vec2(ui.available_width(), ui::CONTROL_H);
                                let (resp, _) = ui::value_field_suffixed(
                                    ui,
                                    size,
                                    ui::Prefix::Icon(icon::DEVICES),
                                    Some(ui::Suffix {
                                        text: "px",
                                        clickable: false,
                                        tooltip: "",
                                    }),
                                    &mut w,
                                    ui::Scrub::whole(0.5).range(0.0..=f64::from(u16::MAX)),
                                    |d| d.custom_formatter(ui::number(2)),
                                );
                                let resp =
                                    resp.on_hover_text(format!("{} from this width up", mode.name));
                                let done =
                                    (resp.changed() && !resp.dragged()) || resp.drag_stopped();
                                if cancelled {
                                    // Nothing lands; the field shows the minimum again.
                                } else if done {
                                    b.width_draft = None;
                                    acts.push(Act::Breakpoint(mode.id, w));
                                } else if resp.changed() {
                                    b.width_draft = Some((mode.id, w));
                                }
                            }
                            if row(ui, icon::COPY_SIMPLE, "Duplicate mode") {
                                acts.push(Act::DuplicateMode(mode.id));
                                close = true;
                            }
                            // The order: the first mode is the default every
                            // unset frame reads; a by-width collection's is its
                            // widths'.
                            let n = modes_of(mode.id);
                            if n > 1 {
                                ui::menu_sep(ui);
                                let moves = [
                                    (icon::MODE_DEFAULT, "Make default", 0, i > 0),
                                    (icon::ARROW_LEFT, "Move left", i.saturating_sub(1), i > 0),
                                    (icon::ARROW_RIGHT, "Move right", i + 1, i + 1 < n),
                                ];
                                for (glyph, label, to, on) in moves {
                                    let on = on && !by_width;
                                    let r = ui::menu_row(
                                        ui,
                                        ui::MenuRow::new(glyph, label).enabled(on),
                                        MENU_ROW_H,
                                    );
                                    let r = match by_width {
                                        true => r.on_hover_text("Ordered by width"),
                                        false => r,
                                    };
                                    if on && r.clicked() {
                                        acts.push(Act::MoveMode(mode.id, to));
                                        close = true;
                                    }
                                }
                                ui::menu_sep(ui);
                            }
                            // A collection's last mode cannot go while it has
                            // variables: every variable has a value in every mode.
                            let held = table
                                .collection_of_mode(mode.id)
                                .is_some_and(|c| table.of_collection(c.id).next().is_some());
                            let last = n == 1 && held;
                            let del = ui::menu_row(
                                ui,
                                ui::MenuRow::new(icon::TRASH, "Delete mode…")
                                    .danger(true)
                                    .enabled(!last),
                                MENU_ROW_H,
                            );
                            let del = if last {
                                del.on_hover_text(
                                    "Every variable needs a value in at least one mode",
                                )
                            } else {
                                del
                            };
                            // Not `close`: the confirmation takes the menu's place.
                            if del.clicked() {
                                b.menu = Some((Menu::ConfirmMode(mode.id), at));
                            }
                        }
                        Menu::Group(collection, path) => {
                            if row(ui, icon::CURSOR_TEXT, "Rename group") {
                                acts.push(Act::StartEdit(Edit::Group {
                                    collection: *collection,
                                    path: path.clone(),
                                    text: path.clone(),
                                }));
                                close = true;
                            }
                            let members = group_members(table, *collection, path);
                            let mut who: Vec<NodeId> = members
                                .iter()
                                .flat_map(|v| users[&v.id].iter().copied())
                                .collect();
                            who.sort();
                            who.dedup();
                            let select = ui::menu_row(
                                ui,
                                ui::MenuRow::new(icon::SELECTION_ALL, &layers_label(who.len()))
                                    .enabled(!who.is_empty()),
                                MENU_ROW_H,
                            );
                            if select.clicked() {
                                acts.push(Act::Select(who));
                                close = true;
                            }
                            let del = ui::menu_row(
                                ui,
                                ui::MenuRow::new(icon::TRASH, "Delete group").danger(true),
                                MENU_ROW_H,
                            );
                            if del.clicked() {
                                acts.push(Act::Delete(
                                    members.iter().map(|v| v.id).collect(),
                                    path.clone(),
                                ));
                                close = true;
                            }
                        }
                        Menu::Style(s) => {
                            let Some(st) = doc.styles().get(*s) else {
                                close = true;
                                return;
                            };
                            // A kit's style is inert here (§15 D1065): its users
                            // can be selected, and nothing else.
                            if st.origin.is_some() {
                                // Never a vault node, which is never selected.
                                let mut who = sty::users(doc, st.id);
                                who.retain(|u| !doc.in_vault(*u));
                                let select = ui::menu_row(
                                    ui,
                                    ui::MenuRow::new(icon::SELECTION_ALL, &layers_label(who.len()))
                                        .enabled(!who.is_empty()),
                                    MENU_ROW_H,
                                );
                                if select.clicked() {
                                    acts.push(Act::Select(who));
                                    close = true;
                                }
                                return;
                            }
                            if row(ui, icon::CURSOR_TEXT, "Rename") {
                                acts.push(Act::StartEdit(Edit::Style {
                                    style: st.id,
                                    text: st.name.clone(),
                                }));
                                close = true;
                            }
                            if row(ui, icon::NOTE, "Description") {
                                acts.push(Act::StartEdit(Edit::Describe {
                                    of: Described::Style(st.id),
                                    text: st.description.clone(),
                                }));
                                close = true;
                            }
                            let who = sty::users(doc, st.id);
                            // *Edit* opens on the style's users (§15 D1063), so a
                            // style nothing uses has nothing to open it on.
                            let edit = ui::menu_row(
                                ui,
                                ui::MenuRow::new(icon::SLIDERS_HORIZONTAL, "Edit")
                                    .enabled(!who.is_empty()),
                                MENU_ROW_H,
                            );
                            let edit = if who.is_empty() {
                                edit.on_hover_text("Apply it to a layer to edit it")
                            } else {
                                edit
                            };
                            if edit.clicked() && !who.is_empty() {
                                acts.push(Act::EditStyle(st.id));
                                close = true;
                            }
                            let select = ui::menu_row(
                                ui,
                                ui::MenuRow::new(icon::SELECTION_ALL, &layers_label(who.len()))
                                    .enabled(!who.is_empty()),
                                MENU_ROW_H,
                            );
                            if select.clicked() {
                                acts.push(Act::Select(who));
                                close = true;
                            }
                            ui::menu_sep(ui);
                            let del = ui::menu_row(
                                ui,
                                ui::MenuRow::new(icon::TRASH, "Delete").danger(true),
                                MENU_ROW_H,
                            );
                            if del.clicked() {
                                acts.push(Act::DeleteStyles(vec![st.id], st.name.clone()));
                                close = true;
                            }
                        }
                        Menu::StyleGroup(slot, path) => {
                            if row(ui, icon::CURSOR_TEXT, "Rename group") {
                                acts.push(Act::StartEdit(Edit::StyleGroup {
                                    slot: *slot,
                                    path: path.clone(),
                                    text: path.clone(),
                                }));
                                close = true;
                            }
                            let members = style_group_members(doc, *slot, path);
                            let mut who: Vec<NodeId> = members
                                .iter()
                                .flat_map(|s| sty::users(doc, *s))
                                .filter(|u| !doc.in_vault(*u))
                                .collect();
                            who.sort();
                            who.dedup();
                            let select = ui::menu_row(
                                ui,
                                ui::MenuRow::new(icon::SELECTION_ALL, &layers_label(who.len()))
                                    .enabled(!who.is_empty()),
                                MENU_ROW_H,
                            );
                            if select.clicked() {
                                acts.push(Act::Select(who));
                                close = true;
                            }
                            let del = ui::menu_row(
                                ui,
                                ui::MenuRow::new(icon::TRASH, "Delete group").danger(true),
                                MENU_ROW_H,
                            );
                            if del.clicked() {
                                acts.push(Act::DeleteStyles(members, path.clone()));
                                close = true;
                            }
                        }
                        Menu::KitMap(kit, mode) => {
                            let Some(km) = table.kits().get(kit) else {
                                close = true;
                                return;
                            };
                            let read = table.kit_mode(kit, Some(*mode));
                            for m in &km.modes {
                                let r = ui::MenuRow::new(
                                    if read == Some(m.id) { icon::CHECK } else { "" },
                                    &m.name,
                                );
                                if ui::menu_row(ui, r, MENU_ROW_H).clicked() {
                                    acts.push(Act::KitMap(kit.clone(), *mode, Some(m.id)));
                                    close = true;
                                }
                            }
                            ui::menu_sep(ui);
                            let by_name = ui::menu_row(
                                ui,
                                ui::MenuRow::new("", "Match by name")
                                    .enabled(km.map.contains_key(mode)),
                                MENU_ROW_H,
                            );
                            if by_name.clicked() && km.map.contains_key(mode) {
                                acts.push(Act::KitMap(kit.clone(), *mode, None));
                                close = true;
                            }
                        }
                        Menu::Collection(c) => {
                            let Some(col) = table.collection(*c) else {
                                close = true;
                                return;
                            };
                            if row(ui, icon::CURSOR_TEXT, "Rename collection") {
                                acts.push(Act::StartEdit(Edit::Collection {
                                    id: col.id,
                                    text: col.name.clone(),
                                }));
                                close = true;
                            }
                            if row(ui, icon::COPY_SIMPLE, "Duplicate collection") {
                                acts.push(Act::DuplicateCollection(col.id));
                                close = true;
                            }
                            // One collection at most is picked by width.
                            let other = table.by_width().filter(|o| o.id != col.id);
                            let pick = ui::menu_row(
                                ui,
                                ui::MenuRow::new(icon::DEVICES, "Pick mode by width")
                                    .checked(col.by_width())
                                    .enabled(other.is_none()),
                                MENU_ROW_H,
                            );
                            let pick = match other {
                                Some(o) => pick
                                    .on_hover_text(format!("{} picks its mode by width", o.name)),
                                None => pick.on_hover_text(
                                    "A top-level frame picks its mode from its width",
                                ),
                            };
                            if pick.clicked() && other.is_none() {
                                acts.push(Act::ByWidth(col.id, !col.by_width()));
                                close = true;
                            }
                            let mut who: Vec<NodeId> = table
                                .of_collection(col.id)
                                .flat_map(|v| users[&v.id].iter().copied())
                                .collect();
                            who.sort();
                            who.dedup();
                            let select = ui::menu_row(
                                ui,
                                ui::MenuRow::new(icon::SELECTION_ALL, &layers_label(who.len()))
                                    .enabled(!who.is_empty()),
                                MENU_ROW_H,
                            );
                            if select.clicked() {
                                acts.push(Act::Select(who));
                                close = true;
                            }
                            ui::menu_sep(ui);
                            // The last collection holds the document's modes
                            // and cannot go.
                            let last = table.collections().len() == 1;
                            let del = ui::menu_row(
                                ui,
                                ui::MenuRow::new(icon::TRASH, "Delete collection…")
                                    .danger(true)
                                    .enabled(!last),
                                MENU_ROW_H,
                            );
                            let del = match last {
                                true => {
                                    del.on_hover_text("A document keeps at least one collection")
                                }
                                false => del,
                            };
                            if del.clicked() && !last {
                                b.menu = Some((Menu::ConfirmCollection(col.id), at));
                            }
                        }
                        Menu::ConfirmCollection(c) => {
                            let Some(col) = table.collection(*c) else {
                                close = true;
                                return;
                            };
                            let (vars, layers, frames) = var::collection_reach(doc, col.id);
                            let mut what = format!("Delete {}?", col.name);
                            what.push_str(&match vars {
                                0 => " It holds no variable.".to_string(),
                                1 => " 1 variable goes".to_string(),
                                n => format!(" {n} variables go"),
                            });
                            if vars > 0 {
                                what.push_str(&match layers {
                                    0 => ".".to_string(),
                                    1 => ", and 1 layer keeps its values.".to_string(),
                                    n => format!(", and {n} layers keep their values."),
                                });
                            }
                            what.push_str(&match frames {
                                0 => String::new(),
                                1 => " 1 frame returns to Inherit.".to_string(),
                                n => format!(" {n} frames return to Inherit."),
                            });
                            ui::menu_note(ui, &what);
                            let del = ui::menu_row(
                                ui,
                                ui::MenuRow::new(icon::TRASH, "Delete collection").danger(true),
                                MENU_ROW_H,
                            );
                            if del.clicked() {
                                acts.push(Act::DeleteCollection(col.id));
                                close = true;
                            }
                            if row(ui, icon::X, "Cancel") {
                                close = true;
                            }
                        }
                        Menu::Scope(v) => {
                            let Some(v) = table.get(*v) else {
                                close = true;
                                return;
                            };
                            scope_popover(ui, doc, v, users, acts);
                        }
                        Menu::Bind(var) => {
                            if b.bind.as_ref().is_none_or(|l| l.var != *var) {
                                let uses = self.other_uses(*var);
                                let own = table.get(*var).and_then(|v| v.scope);
                                let on = super::bind_others::ticked_by_default(&uses, own);
                                b.bind = Some(BindList {
                                    var: *var,
                                    uses,
                                    on,
                                });
                            }
                            close = self.bind_popover(ui, b, acts);
                        }
                        Menu::Words(v, m) => {
                            let Some(v) = table.get(*v) else {
                                close = true;
                                return;
                            };
                            let held = match v.cells.get(m) {
                                Some(Cell::String(s)) => s.clone(),
                                _ => String::new(),
                            };
                            for word in var::flex_words(v) {
                                let r = ui::MenuRow::new("", word).checked(held == word);
                                if ui::menu_row(ui, r, MENU_ROW_H).clicked() {
                                    acts.push(Act::Cell(v.id, *m, word.to_string(), None));
                                    close = true;
                                }
                            }
                        }
                        Menu::ConfirmMode(m) => {
                            let Some(mode) = table.mode(*m) else {
                                close = true;
                                return;
                            };
                            let n = var::frames_set_to(doc, mode.id);
                            let what = match n {
                                0 => format!("Delete {}? No frame is set to it.", mode.name),
                                1 => format!("Delete {}? 1 frame returns to Inherit.", mode.name),
                                n => format!("Delete {}? {n} frames return to Inherit.", mode.name),
                            };
                            ui::menu_note(ui, &what);
                            let del = ui::menu_row(
                                ui,
                                ui::MenuRow::new(icon::TRASH, "Delete mode").danger(true),
                                MENU_ROW_H,
                            );
                            if del.clicked() {
                                acts.push(Act::DeleteMode(mode.id));
                                close = true;
                            }
                            if row(ui, icon::X, "Cancel") {
                                close = true;
                            }
                        }
                    }
                });
            })
            .response
            .rect;
        // *Move to ▸*'s list beside its row: the collections, the variable's own
        // checked, then *New collection*.
        let mut fly = None;
        if let (Menu::Var(v), Some(row_rect), false) = (&menu, b.move_to, close) {
            let v = *v;
            let id = egui::Id::new("var-browser-menu").with("move-to");
            let rows = (table.collections().len() + 1, 1);
            let card = ui::flyout(id, row_rect, menu_w)
                .pad(4.0)
                .show(ctx, rows, |ui| {
                    let here = table.collection_of(v).map(|c| c.id);
                    for c in table.collections() {
                        // One with no mode has no column to move into.
                        let open = Some(c.id) != here && !c.modes.is_empty();
                        let r = ui::menu_row(
                            ui,
                            ui::MenuRow::new("", &c.name)
                                .checked(Some(c.id) == here)
                                .enabled(open),
                            MENU_ROW_H,
                        );
                        let r = match c.modes.is_empty() {
                            true => r.on_hover_text("It has no mode yet"),
                            false => r,
                        };
                        if r.clicked() && open {
                            acts.push(Act::MoveTo(v, Some(c.id)));
                            close = true;
                        }
                    }
                    ui::menu_sep(ui);
                    let new = ui::menu_row(
                        ui,
                        ui::MenuRow::new(icon::DATABASE, "New collection"),
                        MENU_ROW_H,
                    );
                    if new.clicked() {
                        acts.push(Act::MoveTo(v, None));
                        close = true;
                    }
                });
            fly = Some(card.rect);
        }
        // Not during a gesture: the right-click that cancels the breakpoint scrub
        // lands wherever the drag took the pointer, and is not a press away.
        let in_gesture = ctx.dragged_id().is_some() || self.gesture_cancelled;
        let pressed_elsewhere = !in_gesture
            && ctx.input(|i| {
                i.pointer.any_pressed()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| !rect.contains(p) && !fly.is_some_and(|f| f.contains(p)))
            });
        if close || pressed_elsewhere {
            b.menu = None;
        }
        if b.menu.is_none() || !matches!(menu, Menu::Var(_)) {
            b.move_to = None;
        }
    }

    /// Apply one of the browser's asks.
    fn var_act(&mut self, act: Act) {
        let doc = &self.session.doc;
        let table = doc.variables();
        match act {
            Act::StyleName(id, text) => {
                if doc.styles().get(id).is_none() {
                    self.var_edit_done();
                    return;
                }
                match ondin_core::style::rename_ops(doc.styles(), id, &text) {
                    Err(why) => self.var_refuse(why),
                    Ok(ops) => {
                        self.var_edit_done();
                        if !ops.is_empty() {
                            self.session.name_step("Rename style", icon::CURSOR_TEXT);
                            self.commit_edit(Transaction(ops));
                        }
                    }
                }
            }
            Act::EditStyle(id) => self.edit_style(id),
            Act::StyleGroupName(slot, from, to) => {
                let to = to.trim().trim_matches('/').to_string();
                if to == from {
                    self.var_edit_done();
                    return;
                }
                if to.split('/').any(|s| s.trim().is_empty()) && !to.is_empty() {
                    self.var_refuse("A group's name cannot have an empty part.".into());
                    return;
                }
                match sty::rename_group_ops(doc, slot, &from, &to) {
                    Err(clash) => self.var_refuse(format!("{clash} already exists.")),
                    Ok(ops) => {
                        self.var_edit_done();
                        if !ops.is_empty() {
                            self.session.name_step("Rename group", icon::CURSOR_TEXT);
                            self.commit_edit(Transaction(ops));
                        }
                    }
                }
            }
            Act::Describe(of, text) => {
                let op = Operation::SetDescription {
                    of,
                    text: text.trim().to_string(),
                };
                let same = op.changes_nothing(doc);
                self.var_edit_done();
                if !same {
                    self.session.name_step("Description", icon::NOTE);
                    self.commit_edit(Transaction(vec![op]));
                }
            }
            Act::ToggleAllStyles(key) => {
                let all = style_group_keys(doc);
                if let Some(b) = self.var_browser.as_mut() {
                    match b.collapsed.contains(&key) {
                        true => b.collapsed.retain(|p| !all.contains(p)),
                        false => b.collapsed.extend(all),
                    }
                }
            }
            Act::DeleteStyles(ids, name) => {
                if ids.is_empty() {
                    return;
                }
                // Counted as the browser counts users: a kit's frozen mains are the
                // kit's, never a layer of this document's to report.
                let mut who: Vec<NodeId> = ids
                    .iter()
                    .flat_map(|s| sty::users(doc, *s))
                    .filter(|u| !doc.in_vault(*u))
                    .collect();
                who.sort();
                who.dedup();
                let n = who.len();
                let tx = Transaction(sty::remove_ops(doc, &ids));
                // No confirmation; the status says it, and one undo restores (D1038 (6)).
                self.session.name_step("Delete style", icon::TRASH);
                if self.commit_edit(tx) {
                    self.session.info(match n {
                        0 => format!("Deleted {name}"),
                        1 => format!("Deleted {name}. 1 layer keeps its values."),
                        n => format!("Deleted {name}. {n} layers keep their values."),
                    });
                }
            }
            // The picker rides a node-less slot, so its node is the current
            // page, which the slot never reads (`PaintSlot::VarCell`).
            Act::OpenPicker(var, mode, at) => {
                let root = self.session.page;
                let slot = PaintSlot::VarCell(var, mode);
                match self.picker.as_mut() {
                    Some(p) => p.retarget(root, slot),
                    None => self.picker = Some(Picker::new(root, slot, at)),
                }
            }
            Act::ToggleAll(path) => {
                // Every group path the table has: each name's every prefix
                // before a `/`, so a nested group closes with its parent.
                let mut all: HashSet<String> = HashSet::new();
                for v in table.iter() {
                    let of = match (v.kit(), table.collection_of(v.id)) {
                        (Some(k), _) => GroupOf::Kit(k),
                        (None, Some(c)) => GroupOf::Own(c.id),
                        (None, None) => continue,
                    };
                    let mut at = 0;
                    while let Some(i) = v.name[at..].find('/') {
                        all.insert(group_key(of, &v.name[..at + i]));
                        at += i + 1;
                    }
                }
                if let Some(b) = self.var_browser.as_mut() {
                    match b.collapsed.contains(&path) {
                        true => b.collapsed.retain(|p| !all.contains(p)),
                        false => b.collapsed.extend(all),
                    }
                }
            }
            Act::KitMap(kit, mode, to) => {
                let Some(mut km) = table.kits().get(&kit).cloned() else {
                    return;
                };
                match to {
                    Some(m) => km.map.insert(mode, m),
                    None => km.map.remove(&mode),
                };
                self.session.name_step("Kit mode", icon::PACKAGE);
                self.commit_edit(Transaction(vec![Operation::SetKitModes {
                    kit,
                    modes: Some(km),
                }]));
            }
            Act::Toggle(path) => {
                if let Some(b) = self.var_browser.as_mut()
                    && !b.collapsed.remove(&path)
                {
                    b.collapsed.insert(path);
                }
            }
            Act::StartEdit(edit) => {
                if let Some(b) = self.var_browser.as_mut() {
                    b.edit = Some(edit);
                    b.edit_focus = true;
                    b.refusal = None;
                    b.refused = None;
                }
            }
            Act::OpenMenu(menu, at) => {
                if let Some(b) = self.var_browser.as_mut() {
                    b.menu = Some((menu, at));
                    b.move_to = None;
                }
            }
            Act::KitUpdates => self.open_kit_updates(),
            Act::ImportTokens => self.import_tokens_action(),
            Act::ExportTokens => self.export_tokens_action(),
            Act::UnlinkTokens => self.unlink_tokens(),
            Act::LinkedTokens => self.open_linked_tokens(),
            Act::Select(ids) => {
                if !ids.is_empty() {
                    self.session.selection.set(ids);
                }
            }
            Act::BindUses(var, keys) => {
                if let Some(b) = self.var_browser.as_mut() {
                    b.offer = None;
                    b.bind = None;
                }
                self.bind_other_uses(var, &keys);
            }
            Act::BindReady(var) => {
                if let Some(b) = self.var_browser.as_mut() {
                    b.offer = None;
                    b.bind = None;
                }
                self.bind_ready_uses(var);
            }
            Act::Reveal(id) => self.select_main(id),
            Act::NewVar(kind) => {
                // Into the collection on show.
                let shown = self.shown_collection();
                let table = self.session.doc.variables();
                let Ok((ops, id)) =
                    var::add_variable_ops(table, &mut self.session.ids, shown, kind, None, None)
                else {
                    return;
                };
                self.session.name_step("New variable", icon::BRACKETS_CURLY);
                if self.commit_edit(Transaction(ops)) {
                    let name = self.session.doc.variables().get(id).map(|v| v.name.clone());
                    if let (Some(b), Some(text)) = (self.var_browser.as_mut(), name) {
                        b.edit = Some(Edit::Name { var: id, text });
                        b.edit_focus = true;
                    }
                }
            }
            Act::NewMode => {
                // A mode of the collection on show, copying its first.
                let shown = self.shown_collection();
                let Ok((ops, id)) =
                    var::add_mode_ops(&self.session.doc, &mut self.session.ids, shown, None)
                else {
                    return;
                };
                self.session.name_step("New mode", icon::COLUMNS_PLUS_RIGHT);
                if !ops.is_empty() && self.commit_edit(Transaction(ops)) {
                    let name = self
                        .session
                        .doc
                        .variables()
                        .mode(id)
                        .map(|m| m.name.clone());
                    if let (Some(b), Some(text)) = (self.var_browser.as_mut(), name) {
                        b.edit = Some(Edit::Mode { mode: id, text });
                        b.edit_focus = true;
                    }
                }
            }
            Act::MoveMode(mode, to) => {
                let ops = var::move_mode_ops(doc, mode, to);
                if !ops.is_empty() {
                    self.session.name_step("Move mode", icon::MODE);
                    self.commit_edit(Transaction(ops));
                }
            }
            Act::DuplicateMode(from) => {
                let (Some(index), Some(col)) =
                    (table.mode_index(from), table.collection_of_mode(from))
                else {
                    return;
                };
                let taken: Vec<&str> = col.modes.iter().map(|m| m.name.as_str()).collect();
                let stem = format!("{} copy", col.modes[index].name);
                let name = if taken.contains(&stem.as_str()) {
                    var::numbered_name(&taken, &stem, 2)
                } else {
                    stem
                };
                let id = self.session.ids.mint_mode();
                let ops = var::copy_mode_ops(doc, from, id, &name, index + 1);
                if !ops.is_empty() {
                    self.session.name_step("Duplicate mode", icon::COPY_SIMPLE);
                    self.commit_edit(Transaction(ops));
                }
            }
            Act::Name(id, text) => match var::rename_variable_ops(table, id, &text) {
                Err(why) => self.var_refuse(why),
                Ok(ops) => {
                    self.var_edit_done();
                    if !ops.is_empty() {
                        self.session.name_step("Rename variable", icon::CURSOR_TEXT);
                        self.commit_edit(Transaction(ops));
                    }
                }
            },
            Act::ModeName(id, text) => {
                let text = text.trim().to_string();
                if let Some(why) = var::mode_name_refusal(table, id, &text) {
                    self.var_refuse(why);
                } else {
                    let ops = var::rename_mode_ops(doc, id, &text);
                    self.var_edit_done();
                    if !ops.is_empty() {
                        self.session.name_step("Rename mode", icon::CURSOR_TEXT);
                        self.commit_edit(Transaction(ops));
                    }
                }
            }
            Act::GroupName(collection, from, to) => {
                let to = to.trim().trim_matches('/').to_string();
                if to == from {
                    self.var_edit_done();
                    return;
                }
                if to.split('/').any(|s| s.trim().is_empty()) && !to.is_empty() {
                    self.var_refuse("A group's name cannot have an empty part.".into());
                    return;
                }
                match var::rename_group_ops(doc, collection, &from, &to) {
                    Err(clash) => self.var_refuse(format!("{clash} already exists.")),
                    Ok(ops) => {
                        self.var_edit_done();
                        if !ops.is_empty() {
                            self.session.name_step("Rename group", icon::CURSOR_TEXT);
                            self.commit_edit(Transaction(ops));
                        }
                    }
                }
            }
            Act::Cell(id, mode, text, next) => {
                if self.var_cell(id, mode, text)
                    && let Some(next) = next
                    && let Some(b) = self.var_browser.as_mut()
                {
                    b.edit = Some(next);
                    b.edit_focus = true;
                }
            }
            Act::Duplicate(id) => {
                // The document's own only: a kit's rows are inert (D1038 G), and
                // a copy of one would carry its kit's columns (§15 D1064).
                let Some(v) = table.get(id).filter(|v| v.origin.is_none()) else {
                    return;
                };
                let taken: Vec<&str> = match table.collection_of(v.id) {
                    Some(c) => table.of_collection(c.id).map(|v| v.name.as_str()).collect(),
                    None => Vec::new(),
                };
                let stem = format!("{} copy", v.name);
                let name = if taken.contains(&stem.as_str()) {
                    var::numbered_name(&taken, &stem, 2)
                } else {
                    stem
                };
                let copy = Variable {
                    id: self.session.ids.mint_var(),
                    name,
                    ..v.clone()
                };
                self.session
                    .name_step("Duplicate variable", icon::COPY_SIMPLE);
                self.commit_edit(Transaction(vec![Operation::AddVariable { var: copy }]));
            }
            Act::Delete(ids, label) => {
                let mut layers: Vec<NodeId> =
                    ids.iter().flat_map(|v| var::users(doc, *v)).collect();
                layers.sort();
                layers.dedup();
                // The value the users now hold, named only when every one of them
                // drew the same: each detaches to what it read in its own mode, and
                // two frames in two modes leave two values, so the sentence names
                // a value only in the one-value case.
                let value = match ids.as_slice() {
                    [one] => {
                        let mut drawn = layers.iter().filter_map(|n| doc.resolve_var(*n, *one));
                        drawn
                            .next()
                            .filter(|first| drawn.all(|d| d == *first))
                            .map(value_text)
                    }
                    _ => None,
                };
                let tx = Transaction(var::remove_ops(doc, &ids));
                self.session.name_step("Delete variable", icon::TRASH);
                if self.commit_edit(tx) {
                    let n = layers.len();
                    let said = match (n, value) {
                        (0, _) => format!("Deleted {label}"),
                        (1, Some(v)) => format!("Deleted {label}. 1 layer now uses {v} directly."),
                        (n, Some(v)) => {
                            format!("Deleted {label}. {n} layers now use {v} directly.")
                        }
                        (1, None) => format!("Deleted {label}. 1 layer was detached."),
                        (n, None) => format!("Deleted {label}. {n} layers were detached."),
                    };
                    self.session.info(said);
                }
            }
            Act::ShowCollection(c) => {
                let root = collapsed_key(doc);
                self.var_collection.insert(root, c);
                if let Some(b) = self.var_browser.as_mut() {
                    b.scroll_x = 0.0;
                }
            }
            Act::NewCollection => {
                let taken: Vec<&str> = table
                    .collections()
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect();
                let name = var::numbered_name(&taken, "Collection", table.collections().len() + 1);
                let (id, mode) = (
                    self.session.ids.mint_collection(),
                    self.session.ids.mint_mode(),
                );
                let ops = var::new_collection_ops(id, mode, &name, table.collections().len());
                self.session.name_step("New collection", icon::DATABASE);
                if self.commit_edit(Transaction(ops)) {
                    let root = collapsed_key(&self.session.doc);
                    self.var_collection.insert(root, id);
                    if let Some(b) = self.var_browser.as_mut() {
                        b.tab = Tab::Variables;
                        b.edit = Some(Edit::Collection { id, text: name });
                        b.edit_focus = true;
                    }
                }
            }
            Act::CollectionName(id, text) => {
                let text = text.trim().to_string();
                let taken = table
                    .collections()
                    .iter()
                    .any(|c| c.id != id && c.name == text);
                if text.is_empty() {
                    self.var_refuse("A collection needs a name.".into());
                } else if taken {
                    self.var_refuse(format!("{text} already exists."));
                } else {
                    let changed = table.collection(id).is_some_and(|c| c.name != text);
                    self.var_edit_done();
                    if changed {
                        self.session
                            .name_step("Rename collection", icon::CURSOR_TEXT);
                        self.commit_edit(Transaction(vec![Operation::SetCollectionName {
                            id,
                            name: text,
                        }]));
                    }
                }
            }
            Act::DuplicateCollection(c) => {
                let Some(col) = table.collection(c) else {
                    return;
                };
                let taken: Vec<&str> = table
                    .collections()
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect();
                let stem = format!("{} copy", col.name);
                let name = if taken.contains(&stem.as_str()) {
                    var::numbered_name(&taken, &stem, 2)
                } else {
                    stem
                };
                let ops = var::duplicate_collection_ops(doc, c, &name, &mut self.session.ids);
                if !ops.is_empty() {
                    self.session
                        .name_step("Duplicate collection", icon::COPY_SIMPLE);
                    self.commit_edit(Transaction(ops));
                }
            }
            Act::DeleteCollection(c) => {
                let name = table
                    .collection(c)
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                let (_, layers, _) = var::collection_reach(doc, c);
                let ops = var::remove_collection_ops(doc, c);
                if ops.is_empty() {
                    return;
                }
                self.session.name_step("Delete collection", icon::TRASH);
                if self.commit_edit(Transaction(ops)) {
                    self.session.info(match layers {
                        0 => format!("Deleted {name}"),
                        1 => format!("Deleted {name}. 1 layer keeps its values."),
                        n => format!("Deleted {name}. {n} layers keep their values."),
                    });
                }
            }
            Act::ByWidth(c, on) => match var::by_width_ops(doc, c, on) {
                Err(why) => self.session.info(why),
                Ok(ops) if ops.is_empty() => {}
                Ok(ops) => {
                    // Unticking sets each frame that picked by width to the mode
                    // it drew; say how many, since the frames now carry a pin.
                    let pinned = ops
                        .iter()
                        .filter(|op| matches!(op, Operation::SetModes { .. }))
                        .count();
                    self.session.name_step("Pick mode by width", icon::DEVICES);
                    if self.commit_edit(Transaction(ops)) && pinned > 0 {
                        self.session.info(match pinned {
                            1 => "1 frame keeps the mode it drew".to_string(),
                            n => format!("{n} frames keep the mode they drew"),
                        });
                    }
                }
            },
            Act::Breakpoint(m, width) => match var::breakpoint_ops(doc, m, width) {
                Err(why) => self.var_refuse(why),
                Ok(ops) => {
                    if !ops.is_empty() {
                        self.session.name_step("Breakpoint", icon::DEVICES);
                        self.commit_edit(Transaction(ops));
                    }
                }
            },
            Act::MoveTo(v, to) => {
                let mut ops = Vec::new();
                let target = match to {
                    Some(c) => c,
                    None => {
                        // Into a new collection, named after the one it leaves.
                        let taken: Vec<&str> = table
                            .collections()
                            .iter()
                            .map(|c| c.name.as_str())
                            .collect();
                        let name =
                            var::numbered_name(&taken, "Collection", table.collections().len() + 1);
                        let (id, mode) = (
                            self.session.ids.mint_collection(),
                            self.session.ids.mint_mode(),
                        );
                        let index = table.collections().len();
                        // The new one's first mode takes the source's first's name,
                        // so the move keeps the default's value by name too.
                        let mut add = var::new_collection_ops(id, mode, &name, index);
                        if let (Some(Operation::AddCollection { collection, .. }), Some(first)) = (
                            add.first_mut(),
                            table.collection_of(v).and_then(|c| c.modes.first()),
                        ) {
                            collection.modes[0].name = first.name.clone();
                        }
                        ops.extend(add);
                        id
                    }
                };
                // Planned against the document with the new collection in it.
                let mut scratch = doc.clone();
                if !ops.is_empty() && scratch.apply(&Transaction(ops.clone())).is_err() {
                    return;
                }
                match var::move_ops(&scratch, &[v], target) {
                    Err(clash) => self.session.info(format!(
                        "{clash} is already in {}.",
                        scratch
                            .variables()
                            .collection(target)
                            .map_or("", |c| c.name.as_str())
                    )),
                    Ok(moves) => {
                        ops.extend(moves);
                        self.session.name_step("Move variable", icon::DATABASE);
                        if self.commit_edit(Transaction(ops)) {
                            let root = collapsed_key(&self.session.doc);
                            self.var_collection.insert(root, target);
                        }
                    }
                }
            }
            Act::Scope(v, scope) => {
                // Each layer counted once, however many families it loses.
                let dropped = var::users_by_field(doc, v)
                    .into_iter()
                    .filter(|(f, _)| var::scope_drops(scope, *f))
                    .flat_map(|(_, ids)| ids)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len();
                if let Some(held) = var::scope_held_by(doc, v, scope) {
                    let name = |id| table.get(id).map_or("", |v| v.name.as_str());
                    self.session.info(format!(
                        "{} reads {} from another collection, so its words cannot change.",
                        name(held),
                        name(v)
                    ));
                    return;
                }
                let ops = var::scope_ops(doc, v, scope);
                if ops.is_empty() {
                    return;
                }
                self.session.name_step("Scope", icon::TARGET);
                if self.commit_edit(Transaction(ops)) && dropped > 0 {
                    self.session.info(match dropped {
                        1 => "1 layer was detached.".to_string(),
                        n => format!("{n} layers were detached."),
                    });
                }
            }
            Act::DeleteMode(mode) => {
                let name = table.mode(mode).map(|m| m.name.clone()).unwrap_or_default();
                let tx = Transaction(var::remove_mode_ops(doc, mode));
                self.session.name_step("Delete mode", icon::TRASH);
                if self.commit_edit(tx) {
                    self.session.info(format!("Deleted mode “{name}”"));
                }
            }
        }
    }

    /// Commit a typed cell, or answer why not. Whether it landed.
    fn var_cell(&mut self, id: VarId, mode: ModeId, text: String) -> bool {
        let table = self.session.doc.variables();
        let Some(v) = table.get(id) else { return false };
        // A string of flex words takes only those words, read as the word.
        let words = var::flex_words(v);
        let parsed = match parse_cell(table, v, mode, &text) {
            Ok(Cell::String(s)) if !words.is_empty() => words
                .iter()
                .find(|w| {
                    w.eq_ignore_ascii_case(s.trim())
                        || ondin_core::container::FlexDirection::from_word(&s)
                            .is_some_and(|d| d.word() == **w)
                        || ondin_core::container::FlexWrap::from_word(&s)
                            .is_some_and(|x| x.word() == **w)
                })
                .map(|w| Cell::String(w.to_string()))
                .ok_or_else(|| format!("{} holds {}.", v.name, words.join(", "))),
            other => other,
        };
        let ops = match parsed.and_then(|cell| var::set_cell_ops(table, id, mode, cell)) {
            Ok(ops) => ops,
            Err(why) => {
                self.var_refuse(why);
                return false;
            }
        };
        self.var_edit_done();
        if ops.is_empty() {
            return true;
        }
        self.session
            .name_step("Variable value", icon::BRACKETS_CURLY);
        self.commit_edit(Transaction(ops))
    }

    fn var_refuse(&mut self, why: String) {
        if let Some(b) = self.var_browser.as_mut() {
            b.refusal = Some(why);
            b.refused = b.edit.as_ref().map(|e| e.text().to_owned());
            b.edit_focus = true;
        }
    }

    fn var_edit_done(&mut self) {
        if let Some(b) = self.var_browser.as_mut() {
            b.edit = None;
            b.refusal = None;
            b.refused = None;
        }
    }
}

/// A field family as the scope popover lists it.
pub(crate) fn field_label(f: Field) -> &'static str {
    match f {
        Field::Size => "Width and height",
        Field::Limits => "Min and max size",
        Field::Spacing => "Gaps and padding",
        Field::Corner => "Corner radius",
        Field::StrokeWidth => "Stroke width",
        Field::Opacity => "Opacity",
        Field::Effect => "Effect numbers",
        Field::Type => "Size, line height, spacing",
        Field::Weight => "Weight",
        Field::Fill => "Fill",
        Field::Stroke => "Stroke",
        Field::Content => "Text content",
        Field::Family => "Font family",
        Field::FlexDirection => "Flex direction",
        Field::FlexWrap => "Flex wrap",
        Field::Visible => "Visibility",
    }
}

/// The families *Every … field* binds: every one of `kind`'s but those only a
/// scope naming them binds ([`Field::needs_scope`]).
fn every_field(kind: VarKind) -> Scope {
    Scope::of(Field::of_kind(kind).filter(|f| !f.needs_scope()))
}

/// The inspector card a number family sits under in the scope popover; `None`
/// for a colour's or a string's, which list flat.
fn field_group(f: Field) -> Option<&'static str> {
    match f {
        Field::Size | Field::Limits => Some("Size"),
        Field::Spacing => Some("Layout"),
        Field::Corner | Field::StrokeWidth => Some("Shape"),
        Field::Opacity | Field::Effect => Some("Appearance"),
        Field::Type | Field::Weight => Some("Type"),
        _ => None,
    }
}

/// What a variable's scope says in a sentence — *gaps and padding, corner
/// radius* — or `None` when it binds every field of its kind.
pub(crate) fn scope_line(v: &Variable) -> Option<String> {
    let s = v.scope?;
    let words: Vec<String> = s
        .fields()
        .filter(|f| f.kind() == v.kind)
        .map(|f| field_label(f).to_lowercase())
        .collect();
    Some(match words.is_empty() {
        true => "nothing".to_string(),
        false => words.join(", "),
    })
}

/// [`scope_line`] for a refusal: what it binds, every field when unscoped.
pub(crate) fn scope_words(v: &Variable) -> String {
    scope_line(v).unwrap_or_else(|| "every field".to_string())
}

/// A collection's header in a list naming several — the `--` list's and a
/// search's: its glyph and name, small and dim.
pub(crate) fn collection_head(ui: &mut egui::Ui, name: &str) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), GROUP_H),
        egui::Sense::hover(),
    );
    let p = ui.painter();
    let ink = theme::text::dim();
    p.text(
        egui::pos2(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        icon::DATABASE,
        theme::icon_font(13.0),
        ink,
    );
    p.text(
        egui::pos2(rect.left() + 26.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(ui::CAPTION_PT),
        ink,
    );
}

/// A collection's header over its rows in a search spanning the collections:
/// its glyph and name in small caps, its modes at the right.
fn collection_row(ui: &mut egui::Ui, c: &ondin_core::variable::Collection) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), GROUP_H),
        egui::Sense::hover(),
    );
    let p = ui.painter();
    let ink = theme::text::dim();
    p.text(
        egui::pos2(rect.left() + 2.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        icon::DATABASE,
        theme::icon_font(13.0),
        ink,
    );
    ui::paint_eyebrow(
        p,
        egui::pos2(rect.left() + 20.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        &c.name,
    );
    let modes: Vec<&str> = c.modes.iter().map(|m| m.name.as_str()).collect();
    p.text(
        egui::pos2(rect.right() - 2.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        modes.join(" · "),
        egui::FontId::proportional(ui::SMALL_PT),
        theme::text::faint(),
    );
}

/// The scope popover's width.
const SCOPE_W: f32 = 300.0;

/// The scope popover's inset on both sides, a menu row's: its head's glyph, the
/// switch's label, the boxes, the counts and the footer all keep it.
const SCOPE_INSET: f32 = 8.0;

/// Where a ticked row's label starts: a menu row's label column.
const SCOPE_LABEL_X: f32 = ui::MENU_LABEL_X;

/// The scope popover's height in menu rows for `v`'s kind — fixed per kind,
/// so it never resizes while boxes are ticked.
fn scope_rows(v: Option<&Variable>) -> f32 {
    let Some(v) = v else { return 2.0 };
    let fields = Field::of_kind(v.kind).count() as f32;
    let groups = match v.kind {
        VarKind::Number => 5.0,
        _ => 0.0,
    };
    // Head, *Every … field*, the rows and their headings, the two footer lines.
    2.5 + fields + groups * 0.9 + 2.0
}

/// *Bind other uses*'s list at its tallest, in menu rows: past it the fields
/// scroll inside the popover rather than growing it.
const BIND_LIST_ROWS: f32 = 10.0;

impl OndinApp {
    /// *Bind other uses…*'s popover, in the variable menu's place: what was
    /// searched, the fields grouped by family — the scope's own first and
    /// ticked, any other unticked under a heading saying it widens the scope —
    /// and *Cancel* / *Bind N*. A row's hover outlines its layer on the canvas
    /// and a double-click goes to it. Answers whether it closes.
    fn bind_popover(&self, ui: &mut egui::Ui, b: &mut Browser, acts: &mut Vec<Act>) -> bool {
        let doc = &self.session.doc;
        let table = doc.variables();
        let Some(list) = b.bind.as_mut() else {
            return true;
        };
        let Some(v) = table.get(list.var) else {
            return true;
        };
        let inset = |ui: &mut egui::Ui, text: egui::RichText| {
            ui.horizontal(|ui| {
                ui.add_space(SCOPE_INSET);
                ui.add(egui::Label::new(text).wrap());
            });
        };
        inset(
            ui,
            egui::RichText::new(format!("Bind {} to its other users", v.name))
                .size(ui::CONTROL_PT)
                .color(theme::text::strong()),
        );
        let value = table
            .collection_of(v.id)
            .and_then(|c| c.default_mode())
            .and_then(|m| table.resolve(v.id, m));
        let what = value.clone().map(value_text).unwrap_or_default();
        let opacity = match value {
            Some(Value::Color(_)) => " at any opacity",
            _ => "",
        };
        inset(
            ui,
            egui::RichText::new(format!(
                "Fields drawing {what}{opacity}, on every page, in the modes in force there"
            ))
            .size(ui::SMALL_PT)
            .color(theme::text::dim()),
        );
        ui::menu_sep(ui);
        // The scope's own families first, each in the scope popover's order.
        let own = |f: Field| v.scope.is_none_or(|s| s.contains(f));
        let mut families: Vec<Field> = Vec::new();
        for u in &list.uses {
            if !families.contains(&u.field) {
                families.push(u.field);
            }
        }
        let order = |f: &Field| Field::ALL.iter().position(|x| x == f).unwrap_or(0);
        families.sort_by_key(|f| (!own(*f), order(f)));
        let need = list.uses.len() as f32 * (MENU_ROW_H + 1.0)
            + families.len() as f32 * (super::binding::FACE_HEAD_H + 1.0);
        let h = need.min(BIND_LIST_ROWS * MENU_ROW_H);
        let mut hover = None;
        egui::ScrollArea::vertical()
            .id_salt("bind-others")
            .max_height(h)
            .min_scrolled_height(h)
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                let w = ui.available_width();
                for f in &families {
                    let n = list.uses.iter().filter(|u| u.field == *f).count();
                    let (head, _) = ui.allocate_exact_size(
                        egui::vec2(w, super::binding::FACE_HEAD_H),
                        egui::Sense::hover(),
                    );
                    let p = ui.painter();
                    let y = head.bottom() - 7.0;
                    ui::paint_eyebrow(
                        p,
                        egui::pos2(head.left() + SCOPE_INSET, y),
                        egui::Align2::LEFT_CENTER,
                        &format!("{} · {n}", field_label(*f)),
                    );
                    if !own(*f) {
                        p.text(
                            egui::pos2(head.right() - SCOPE_INSET, y),
                            egui::Align2::RIGHT_CENTER,
                            "widens its scope",
                            egui::FontId::proportional(ui::CAPTION_PT),
                            theme::text::muted(),
                        );
                    }
                    for (i, u) in list.uses.iter().enumerate() {
                        if u.field != *f {
                            continue;
                        }
                        let id = u.key.at();
                        let Some(node) = doc.get(id) else { continue };
                        let tick = match u.why_not {
                            Some(_) => ui::Tick::Dead,
                            None => ui::Tick::Open(list.on[i]),
                        };
                        let glyph = super::layers::node_glyph(node).0;
                        let resp = ui::tick_row(
                            ui,
                            w,
                            tick,
                            ui::RowMark::Glyph(glyph),
                            &crate::private::layer(doc, id),
                            ui::RowTail::Text(u.tail.clone()),
                            u.why_not.as_deref(),
                        );
                        // Outlined only where the canvas shows it.
                        if resp.hovered() && doc.page_of(id) == Some(self.session.page) {
                            hover = Some(id);
                        }
                        if u.why_not.is_some() {
                            continue;
                        }
                        if ui::double_clicked(ui, &resp) {
                            // The double-click's first press flipped the
                            // tick; a reveal leaves it as it was.
                            list.on[i] = !list.on[i];
                            acts.push(Act::Reveal(id));
                        } else if resp.clicked() {
                            list.on[i] = !list.on[i];
                        }
                    }
                }
            });
        ui::menu_sep(ui);
        let keys: Vec<super::binding::FieldKey> = list
            .uses
            .iter()
            .zip(&list.on)
            .filter(|(u, on)| **on && u.why_not.is_none())
            .map(|(u, _)| u.key)
            .collect();
        let bind = format!("Bind {}", keys.len());
        let state = match keys.is_empty() {
            true => ui::FieldButton::Disabled,
            false => ui::FieldButton::Primary,
        };
        let why = keys.is_empty().then_some("Tick a field to bind it");
        let var = list.var;
        b.bind_hover = hover;
        let buttons = [("Cancel", ui::FieldButton::Off), (bind.as_str(), state)];
        match crate::settings::footer_row_noted(ui, &buttons, why) {
            Some(0) => true,
            Some(_) => {
                acts.push(Act::BindUses(var, keys));
                true
            }
            None => false,
        }
    }
}

/// *Scope…*: which fields `v` may bind. *Every … field* on is no scope — every
/// field of its kind, any bindable later too; off, a box per family, each
/// counting the layers bound there (the count selects them). Unticking a family
/// with users warns first, in the footer while it is hovered, and detaches on
/// the tick itself (`variable::scope_ops`); a number's footer names the DTCG
/// type the scope makes.
fn scope_popover(
    ui: &mut egui::Ui,
    doc: &Document,
    v: &Variable,
    users: &BTreeMap<VarId, Vec<NodeId>>,
    acts: &mut Vec<Act>,
) {
    let t = doc.variables();
    let by_field = var::users_by_field(doc, v.id);
    let count = |f: Field| -> Vec<NodeId> {
        let mut who = by_field
            .iter()
            .find(|(x, _)| *x == f)
            .map(|(_, ids)| ids.clone())
            .unwrap_or_default();
        who.retain(|u| users.get(&v.id).is_some_and(|all| all.contains(u)));
        who
    };
    // Head: its kind's glyph, its name, its collection.
    ui.horizontal(|ui| {
        ui.add_space(SCOPE_INSET);
        ui.label(theme::icon_text(
            kind_glyph(v.kind),
            15.0,
            theme::text::dim(),
        ));
        ui.label(
            egui::RichText::new(&v.name)
                .size(ui::CONTROL_PT)
                .strong()
                .color(color::text()),
        );
        if let Some(c) = t.collection_of(v.id) {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(SCOPE_INSET);
                ui.label(
                    egui::RichText::new(&c.name)
                        .size(ui::SMALL_PT)
                        .color(theme::text::faint()),
                );
            });
        }
    });
    ui::menu_sep(ui);
    let every = v.scope.is_none();
    let every_label = format!("Every {} field", kind_word(v.kind));
    // At the rows' inset: `switch_row` sets its label flush with its rect.
    let inset = ui
        .available_rect_before_wrap()
        .shrink2(egui::vec2(SCOPE_INSET, 0.0));
    let switch = ui.scope_builder(egui::UiBuilder::new().max_rect(inset), |ui| {
        ui::switch_row(ui, &every_label, every, MENU_ROW_H)
    });
    if switch.inner.clicked() {
        // Off leaves every box ticked to start from.
        let next = match every {
            true => Some(every_field(v.kind)),
            false => None,
        };
        acts.push(Act::Scope(v.id, next));
    }
    ui::menu_sep(ui);
    let scope = v.scope.unwrap_or_else(|| every_field(v.kind));
    let mut warn: Option<String> = None;
    let mut group: Option<&str> = None;
    for f in Field::of_kind(v.kind) {
        if let Some(g) = field_group(f)
            && group != Some(g)
        {
            group = Some(g);
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_space(SCOPE_INSET);
                ui.label(ui::eyebrow(g));
            });
        }
        let on = scope.contains(f);
        let who = count(f);
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), MENU_ROW_H),
            egui::Sense::click(),
        );
        let p = ui.painter();
        ui::list_row_ground(p, rect, false, resp.hovered() && !every);
        // `paint_tick_box` centres the box on the point it is handed.
        ui::paint_tick_box(
            p,
            egui::pos2(
                rect.left() + SCOPE_INSET + ui::TICK_BOX / 2.0,
                rect.center().y,
            ),
            on,
            resp.hovered(),
        );
        let ink = match (every, on) {
            (true, _) => theme::text::disabled(),
            (false, true) => color::text(),
            (false, false) => theme::text::muted(),
        };
        p.text(
            egui::pos2(rect.left() + SCOPE_LABEL_X, rect.center().y),
            egui::Align2::LEFT_CENTER,
            field_label(f),
            egui::FontId::proportional(ui::SMALL_PT),
            ink,
        );
        // The count selects the layers bound there.
        let picked = if who.is_empty() {
            false
        } else {
            let n = who.len().to_string();
            let r = egui::Rect::from_min_max(
                egui::pos2(rect.right() - 36.0, rect.top()),
                rect.right_bottom(),
            );
            let hit = ui.interact(r, resp.id.with("count"), egui::Sense::click());
            ui.painter().text(
                egui::pos2(rect.right() - SCOPE_INSET, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                n,
                egui::FontId::proportional(ui::SMALL_PT),
                if hit.hovered() {
                    color::text()
                } else {
                    theme::text::faint()
                },
            );
            let hit = hit.on_hover_text(layers_label(who.len()));
            if hit.clicked() {
                acts.push(Act::Select(who.clone()));
            }
            hit.clicked()
        };
        if every {
            continue;
        }
        if on && !who.is_empty() && resp.hovered() {
            warn = Some(match who.len() {
                1 => format!("Unticking {} detaches 1 layer.", field_label(f)),
                n => format!("Unticking {} detaches {n} layers.", field_label(f)),
            });
        }
        if resp.clicked() && !picked {
            acts.push(Act::Scope(v.id, Some(scope.with(f, !on))));
        }
    }
    ui::menu_sep(ui);
    let foot = |ui: &mut egui::Ui, text: String, ink: egui::Color32| {
        ui.horizontal(|ui| {
            ui.add_space(SCOPE_INSET);
            ui.label(egui::RichText::new(text).size(ui::SMALL_PT).color(ink));
        });
    };
    // Always the line's room, so the popover keeps its height.
    match warn {
        Some(w) => foot(ui, w, color::warn()),
        None => foot(ui, String::new(), theme::text::faint()),
    }
    if v.kind == VarKind::Number {
        let ty = match v.scope {
            Some(s) => ondin_export::dtcg::scope_type(Some(s)),
            None => "read from its bindings",
        };
        let line = match ty {
            "dimension" => "Exports as dimension · px".to_string(),
            "read from its bindings" => "Exports typed by what it binds".to_string(),
            other => format!("Exports as {other}"),
        };
        foot(ui, line, theme::text::dim());
    }
}

/// The glyph a variable's kind wears in its row.
fn kind_glyph(k: VarKind) -> &'static str {
    match k {
        VarKind::Color => icon::PALETTE,
        VarKind::Number => icon::HASH,
        VarKind::Boolean => icon::TOGGLE,
        VarKind::String => icon::TEXT_T,
    }
}

/// What the picker says of a collection beside its name: how many modes it has,
/// never their names, which would crowd the collection's own out of the row.
fn modes_summary(c: &ondin_core::variable::Collection) -> String {
    match c.modes.len() {
        1 => "1 mode".to_string(),
        n => format!("{n} modes"),
    }
}

/// *Select 3 layers*, *Select 1 layer*, *No layers use it*.
fn layers_label(n: usize) -> String {
    match n {
        0 => "No layers use it".into(),
        1 => "Select 1 layer".into(),
        n => format!("Select {n} layers"),
    }
}

/// The line under a variable *Extract into variable…* just made, at its name's
/// inset: *Bind N other fields*, which binds what the list would start with
/// ticked in one click (it changes nothing on screen, and undoes), then
/// *Choose…*, which opens the list. With none ready (all locked, or of a family
/// the scope leaves out) it says how many draw the value instead. A line rather
/// than a dialog, so a run of extracts goes on uninterrupted.
fn offer_line(
    ui: &mut egui::Ui,
    doc: &Document,
    v: &Variable,
    o: super::bind_others::Offer,
    w: f32,
    acts: &mut Vec<Act>,
) {
    let table = doc.variables();
    let what = table
        .collection_of(v.id)
        .and_then(|c| c.default_mode())
        .and_then(|m| table.resolve(v.id, m))
        .map(value_text)
        .unwrap_or_default();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, OFFER_H), egui::Sense::hover());
    let line =
        egui::Rect::from_x_y_ranges(rect.left() + name_indent(v)..=rect.right(), rect.y_range());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(line)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 10.0;
    if o.ready > 0 {
        let bind = format!("Bind {}", ui::count(o.ready, "other field"));
        let resp = ui::word_link(&mut child, &bind, false).on_hover_text(format!(
            "Bind the fields drawing {what} to this variable — nothing on screen changes"
        ));
        if resp.clicked() {
            acts.push(Act::BindReady(v.id));
        }
    } else {
        let words = match o.total {
            1 => format!("1 other field draws {what}"),
            n => format!("{n} other fields draw {what}"),
        };
        child.label(
            egui::RichText::new(words)
                .size(ui::SMALL_PT)
                .color(theme::text::dim()),
        );
    }
    let choose = ui::word_link(&mut child, "Choose…", true).on_hover_text(format!(
        "Pick which of the {} drawing {what} to bind",
        ui::count(o.total, "field")
    ));
    if choose.clicked() {
        acts.push(Act::OpenMenu(Menu::Bind(v.id), choose.rect.left_bottom()));
    }
}

/// The offer line's height: a line of words under its variable's row.
const OFFER_H: f32 = 22.0;

/// A variable's value as a sentence names it: `16`, `#8BA5E0`, `“Sign in”`.
fn value_text(v: Value) -> String {
    match v {
        Value::Color(c) => format!("#{}", ui::hex_of(c)),
        Value::Number(x) => number_text(x),
        Value::Boolean(b) => super::binding::bool_word(b).into(),
        Value::String(s) => string_text(&s),
    }
}

/// A string as a row shows it: **quoted**, D1038 G's *"a quoted string"* —
/// what tells it from a number or a word like *true* with no type column —
/// on one line, a line break a space.
pub(crate) fn string_text(s: &str) -> String {
    format!("\u{201C}{}\u{201D}", s.replace(['\r', '\n'], " "))
}

pub(crate) fn number_text(x: f64) -> String {
    let r = (x * 100.0).round() / 100.0;
    if r.fract() == 0.0 {
        format!("{r:.0}")
    } else {
        format!("{r}")
    }
}

/// Whether `name` sits in group `path` or below it.
fn in_group(name: &str, path: &str) -> bool {
    path.is_empty() || name.starts_with(&format!("{path}/"))
}

/// The document's own variables under group `path` — what its menu deletes and
/// selects the users of. A kit's group of the same path lists apart, and its
/// rows are inert (§15 D1065).
fn group_members<'a>(
    table: &'a ondin_core::Variables,
    c: CollectionId,
    path: &str,
) -> Vec<&'a Variable> {
    table
        .of_collection(c)
        .filter(|v| in_group(&v.name, path))
        .collect()
}

/// The document's own styles of `slot` under group `path` — what its menu
/// deletes and selects the users of.
fn style_group_members(doc: &Document, slot: StyleSlot, path: &str) -> Vec<StyleId> {
    doc.styles()
        .of_slot(slot)
        .filter(|s| s.origin.is_none() && in_group(&s.name, path))
        .map(|s| s.id)
        .collect()
}

/// The key a slot's collapse is kept under, apart from every variable group's
/// path by a leading `\u{1}` — and a kit's by its id after it.
fn slot_key(kit: Option<&str>, slot: StyleSlot) -> String {
    match kit {
        None => format!("\u{1}{}", slot.title()),
        Some(k) => format!("\u{1}{k}\u{1}{}", slot.title()),
    }
}

/// The key a style group's collapse is kept under, inside its slot's.
fn style_group_key(kit: Option<&str>, slot: StyleSlot, path: &str) -> String {
    format!("{}\u{1}{path}", slot_key(kit, slot))
}

/// Every style group's key — what `Alt`+click folds or opens at once.
fn style_group_keys(doc: &Document) -> HashSet<String> {
    doc.styles()
        .iter()
        .filter_map(|s| {
            let (group, _) = var::split_name(&s.name);
            (!group.is_empty()).then(|| style_group_key(s.kit(), s.slot(), group))
        })
        .collect()
}

/// A typed cell: `--name` aliases another variable of the kind, a colour takes
/// any colour text (`crate::colour_text`, its alpha too), a number takes an expression, and a
/// string takes the text as typed, spaces and all (§15 D1069).
fn parse_cell(
    table: &ondin_core::Variables,
    v: &Variable,
    mode: ModeId,
    text: &str,
) -> Result<Cell, String> {
    let typed = text;
    let text = text.trim();
    if let Some(name) = text.strip_prefix("--") {
        let name = name.trim();
        // The cell's own target first: an alias of a kit's variable reads
        // `--name` unqualified, and the document may have its own of that name.
        let current = match v.cells.get(&mode) {
            Some(Cell::Alias(t)) => table.get(*t).filter(|t| t.name == name),
            _ => None,
        };
        // Then its own collection's, then the first other one's by that name.
        let own = table
            .collection_of(v.id)
            .and_then(|c| table.by_name_in(c.id, name));
        let Some(target) = current.or(own).or_else(|| table.by_name(name)) else {
            return Err(format!("No variable is named {name}."));
        };
        if target.kind != v.kind {
            return Err(format!(
                "{name} is a {}, not a {}.",
                kind_word(target.kind),
                kind_word(v.kind)
            ));
        }
        if table.would_loop(v.id, mode, target.id) {
            return Err(format!(
                "{name} already points to {}. That would loop.",
                v.name
            ));
        }
        return Ok(Cell::Alias(target.id));
    }
    match v.kind {
        VarKind::Color => parse_colour(text)
            .map(Cell::Color)
            .ok_or_else(|| "A colour is a hex or CSS colour, like 8BA5E0, or --name.".to_string()),
        // Held to the fields' limit: a number variable binds those fields.
        VarKind::Number => crate::expr::eval(text)
            .filter(|x| x.is_finite())
            .map(|x| Cell::Number(x.clamp(-crate::ui::FIELD_LIMIT, crate::ui::FIELD_LIMIT)))
            .ok_or_else(|| "A number, or --name.".to_string()),
        VarKind::Boolean => parse_bool(text)
            .map(Cell::Boolean)
            .ok_or_else(|| "True or false, or --name.".to_string()),
        VarKind::String => Ok(Cell::String(typed.to_string())),
    }
}

/// A boolean cell's typed value (§15 D1059): `true`/`false`, `yes`/`no`,
/// `on`/`off` or `1`/`0`, any case.
fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn kind_word(k: VarKind) -> &'static str {
    match k {
        VarKind::Color => "colour",
        VarKind::Number => "number",
        VarKind::Boolean => "boolean",
        VarKind::String => "string",
    }
}

/// Any colour a field reads (`crate::colour_text`); a colour variable carries its
/// own alpha, opaque when none is typed.
fn parse_colour(s: &str) -> Option<Color> {
    Some(crate::colour_text::parse(s)?.color(1.0))
}

/// What a cell shows when it is not being typed into, and what typing starts
/// from.
fn cell_text(table: &ondin_core::Variables, cell: &Cell) -> String {
    match cell {
        Cell::Color(c) => {
            let a = (c.components[3] * 255.0).round() as u8;
            if a == 255 {
                ui::hex_of(*c)
            } else {
                format!("{}{a:02X}", ui::hex_of(*c))
            }
        }
        Cell::Number(x) => number_text(*x),
        Cell::Boolean(b) => super::binding::bool_word(*b).into(),
        // Typing starts from the string bare; at rest it is drawn quoted.
        Cell::String(s) => s.clone(),
        Cell::Alias(t) => format!(
            "--{}",
            table.get(*t).map(|v| v.name.as_str()).unwrap_or_default()
        ),
    }
}

/// The table's lines: each group's header (the unnamed top group has none) and
/// its variables, by name, filtered by the search and by *In selection*.
fn lines_of<'a>(
    vars: impl Iterator<Item = &'a Variable>,
    of: GroupOf<'a>,
    query: &str,
    picked: Option<&HashSet<VarId>>,
    collapsed: &HashSet<String>,
) -> Vec<Line<'a>> {
    let mut groups: BTreeMap<&'a str, (Vec<&'a Variable>, usize)> = BTreeMap::new();
    for v in vars {
        let (group, _) = var::split_name(&v.name);
        let entry = groups.entry(group).or_default();
        entry.1 += 1;
        let hit = query.is_empty() || v.name.to_lowercase().contains(query);
        let chosen = picked.is_none_or(|p| p.contains(&v.id));
        if hit && chosen {
            entry.0.push(v);
        }
    }
    let mut out = Vec::new();
    for (path, (mut shown, total)) in groups {
        if shown.is_empty() {
            continue;
        }
        shown.sort_by(|a, b| a.name.cmp(&b.name));
        if !path.is_empty() {
            out.push(Line::Group {
                path,
                shown: shown.len(),
                total,
                of,
            });
        }
        // A search opens the groups it matches (D1038 G).
        if path.is_empty() || !query.is_empty() || !collapsed.contains(&group_key(of, path)) {
            out.extend(shown.into_iter().map(Line::Var));
        }
    }
    out
}

/// The whole table's lines: collection `shown`'s own, then each kit's that maps
/// from it under its header, by the kit's name — *kit groups last* (D1038 B,
/// §15 D1065). A kit with nothing shown has no header.
///
/// **`across`, a search or *In selection* spans every collection**, each under
/// its header with its own rows, and every kit after them.
fn all_lines<'a>(
    doc: &'a Document,
    shown: CollectionId,
    across: bool,
    query: &str,
    picked: Option<&HashSet<VarId>>,
    collapsed: &HashSet<String>,
) -> Vec<Line<'a>> {
    let table = doc.variables();
    let mut out = Vec::new();
    if across {
        for c in table.collections() {
            let of = GroupOf::Own(c.id);
            let lines = lines_of(table.of_collection(c.id), of, query, picked, collapsed);
            if !lines.is_empty() {
                out.push(Line::Collection(c));
                out.extend(lines);
            }
        }
    } else {
        let of = GroupOf::Own(shown);
        out = lines_of(table.of_collection(shown), of, query, picked, collapsed);
    }
    let mut kits: Vec<(String, &'a str)> = table
        .kits()
        .keys()
        .map(|k| (super::binding::kit_name(doc, k), k.as_str()))
        .collect();
    kits.sort();
    for (_, k) in kits {
        // Shown under the collection its variables map from, unless spanning.
        let maps_here = |v: &&Variable| {
            across
                || table
                    .kit_onto(table.kit_collection_of(v).unwrap_or_default())
                    .is_some_and(|c| c.id == shown)
        };
        let vars = table.of_kit(k).filter(maps_here);
        let lines = lines_of(vars, GroupOf::Kit(k), query, picked, collapsed);
        if !lines.is_empty() {
            out.push(Line::Kit(k));
            out.extend(lines);
        }
    }
    out
}

/// The *Variables* / *Styles* strip.
fn tabs(ui: &mut egui::Ui, tab: &mut Tab) {
    const TABS: [(&str, Tab); 2] = [("Variables", Tab::Variables), ("Styles", Tab::Styles)];
    let w = ui.available_width();
    let selected = TABS.iter().position(|(_, t)| t == tab).unwrap_or(0);
    let picked = ui::segmented(
        ui,
        w,
        ui::SEGMENT_CELL_H,
        TABS.len(),
        selected,
        |p, i, r, on| {
            ui::segment_label(p, r, TABS[i].0, on);
        },
    );
    if let Some(i) = picked {
        *tab = TABS[i].1;
    }
}

/// The collection picker's width, when it shows.
const PICKER_W: f32 = 150.0;

/// The collection picker: a dropdown of the document's collections, the shown
/// one checked with its modes beside it, then *New collection*; a right-click
/// opens the shown one's menu, and its rename types over it. Only with two
/// collections, or one renamed or being renamed.
fn collection_picker(
    ui: &mut egui::Ui,
    doc: &Document,
    shown: CollectionId,
    b: &mut Browser,
    acts: &mut Vec<Act>,
) {
    let t = doc.variables();
    let Some(current) = t.collection(shown) else {
        return;
    };
    let refused = b.refused.clone();
    if let Some(Edit::Collection { id, text }) = &mut b.edit
        && *id == shown
    {
        let (r, _) = ui.allocate_exact_size(egui::vec2(PICKER_W, SEARCH_H), egui::Sense::hover());
        let resp = edit_field(
            ui,
            r,
            text,
            egui::Id::new("var-collection-name"),
            ui::CONTROL_PT,
            Some("Collection name"),
            refused.as_deref(),
        );
        if b.edit_focus {
            b.edit_focus = false;
            resp.request_focus();
        }
        ui::select_all_on_focus(ui, &resp, text);
        if ui::defocus_commits(&resp) {
            acts.push(Act::CollectionName(shown, text.clone()));
        }
        return;
    }
    ui.spacing_mut().interact_size.y = SEARCH_H;
    ui.spacing_mut().button_padding.y = 0.0;
    // A truncating face lays its text out against the room it is given, so the
    // room is the picker's width: a long name ends in `…` instead of widening it.
    let combo = ui
        .scope(|ui| {
            ui.set_max_width(PICKER_W);
            egui::ComboBox::from_id_salt("var-collection")
                .icon(ui::combo_chevron)
                .width(PICKER_W)
                .truncate()
                .selected_text(ui::glyph_and_text(icon::DATABASE, &current.name))
                .show_ui(ui, |ui| {
                    ui::menu_rows(ui);
                    for c in t.collections() {
                        let summary = modes_summary(c);
                        let row = ui::MenuRow::new("", &c.name)
                            .checked(c.id == shown)
                            .accel(Some(&summary));
                        if ui::menu_row(ui, row, MENU_ROW_H).clicked() {
                            acts.push(Act::ShowCollection(c.id));
                        }
                    }
                    ui::menu_sep(ui);
                    if ui::menu_row(
                        ui,
                        ui::MenuRow::new(icon::DATABASE, "New collection"),
                        MENU_ROW_H,
                    )
                    .clicked()
                    {
                        acts.push(Act::NewCollection);
                    }
                })
                .response
        })
        .inner;
    // The whole name, which a long one's face cuts.
    let combo = combo.on_hover_text(format!("{} · right-click for its menu", current.name));
    if combo.secondary_clicked() {
        acts.push(Act::OpenMenu(
            Menu::Collection(shown),
            combo.rect.left_bottom() + egui::vec2(0.0, ui::MENU_GAP),
        ));
    }
}

/// The search, the *In selection* toggle and the `+` — and, with two
/// collections, the collection picker before them.
fn search_row(
    ui: &mut egui::Ui,
    doc: &Document,
    shown: CollectionId,
    b: &mut Browser,
    acts: &mut Vec<Act>,
) {
    let t = doc.variables();
    let renaming = matches!(&b.edit, Some(Edit::Collection { .. }));
    let picker = matches!(b.tab, Tab::Variables) && (t.has_collections() || renaming);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if picker {
            collection_picker(ui, doc, shown, b, acts);
        }
        let styles = matches!(b.tab, Tab::Styles);
        let buttons = if styles { 3.0 } else { 2.0 };
        let w = ui.available_width() - buttons * (SEARCH_H + 6.0);
        let hint = match b.tab {
            Tab::Variables if t.has_collections() => "Search all collections",
            Tab::Variables => "Search variables",
            Tab::Styles => "Search styles",
        };
        let resp = ui::search_box(
            ui,
            egui::vec2(w, SEARCH_H),
            &mut b.query,
            hint,
            egui::Id::new(SEARCH_ID),
        );
        if b.focus {
            b.focus = false;
            resp.request_focus();
        }
        let toggle = ui::field_button(
            ui,
            icon::SELECTION,
            SEARCH_H,
            ui::FieldButton::on_if(b.in_selection),
        )
        .on_hover_text(match b.tab {
            Tab::Variables => "In selection: only the variables the selection uses",
            Tab::Styles => "In selection: only the styles the selection uses",
        });
        if toggle.clicked() {
            b.in_selection = !b.in_selection;
        }
        if styles {
            let toggle =
                ui::field_button(ui, icon::FUNNEL, SEARCH_H, ui::FieldButton::on_if(b.unused))
                    .on_hover_text("Unused: only this document's styles no layer uses");
            if toggle.clicked() {
                b.unused = !b.unused;
            }
        }
        // Open, the `+` wears an open dropdown's border.
        let adding = matches!(b.menu, Some((Menu::Add, _)));
        let add_state = match adding {
            true => ui::FieldButton::Set,
            false => ui::FieldButton::Off,
        };
        let add =
            ui::field_button(ui, icon::PLUS, SEARCH_H, add_state).on_hover_text(match b.tab {
                Tab::Variables => "New variable or mode",
                // A style is made from a card's values, and the `+` takes
                // the browser to the variables it can make (§15 D1063). The
                // menu named in words: the chrome's text face has no `⋯`, and
                // the tooltip drew a missing-glyph box in its place.
                Tab::Styles => "New variable or mode — a style is made from a card's … menu",
            });
        if add.clicked() {
            acts.push(Act::OpenMenu(
                Menu::Add,
                add.rect.left_bottom() + egui::vec2(0.0, 4.0),
            ));
        }
    });
}

/// The document's own styles no layer uses, by the browser's usage count
/// (`users`, vault nodes left out); a kit's styles are the kit's to keep.
fn unused_styles(doc: &Document, users: &BTreeMap<StyleId, Vec<NodeId>>) -> Vec<StyleId> {
    doc.styles()
        .iter()
        .filter(|s| s.kit().is_none() && users.get(&s.id).is_none_or(Vec::is_empty))
        .map(|s| s.id)
        .collect()
}

/// *Unused*'s line under the search row: how many unused styles the list shows,
/// and *Delete unused*, dim with its reason when there are none. True when *Delete
/// unused* is clicked.
fn unused_line(ui: &mut egui::Ui, n: usize) -> bool {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(match n {
                0 => "No unused styles here".to_string(),
                n => ui::count(n, "unused style"),
            })
            .size(ui::CAPTION_PT)
            .color(theme::text::dim()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let state = match n {
                0 => ui::FieldButton::Disabled,
                _ => ui::FieldButton::Off,
            };
            let resp = crate::settings::text_button(ui, "Delete unused", state);
            let resp = match n {
                0 => resp.on_disabled_hover_text("No style here is unused"),
                _ => resp.on_hover_text("Delete them in one step; Ctrl+Z brings them back"),
            };
            n > 0 && resp.clicked()
        })
        .inner
    })
    .inner
}

/// The linked tokens file under the search row: its path, and how many changes
/// it holds since it was last applied. True when clicked, which opens their
/// review.
fn linked_line(ui: &mut egui::Ui, path: &str, changes: usize) -> bool {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, SEARCH_H), egui::Sense::click());
    let p = ui.painter();
    let ground = if resp.hovered() {
        color::text_a(20)
    } else {
        color::text_a(13)
    };
    p.rect(
        rect,
        5,
        ground,
        egui::Stroke::new(1.0, color::text_a(20)),
        egui::StrokeKind::Inside,
    );
    let y = rect.center().y;
    p.text(
        egui::pos2(rect.left() + 9.0, y),
        egui::Align2::LEFT_CENTER,
        icon::LINK_SIMPLE,
        theme::icon_font(13.0),
        theme::text::dim(),
    );
    let tail = match changes {
        0 => String::new(),
        1 => "1 change".to_string(),
        n => format!("{n} changes"),
    };
    let tail_g = ui.fonts_mut(|f| {
        f.layout_no_wrap(
            tail,
            egui::FontId::proportional(ui::CAPTION_PT),
            color::accent_300(),
        )
    });
    let tail_w = tail_g.size().x;
    ui.painter().galley(
        egui::pos2(rect.right() - 9.0 - tail_w, y - tail_g.size().y / 2.0),
        tail_g,
        color::accent_300(),
    );
    // The path elided from the left, so the file's own name stays.
    let font = egui::FontId::proportional(ui::SMALL_PT);
    let room = rect.width() - 9.0 - 20.0 - 9.0 - tail_w - 8.0;
    let mut shown = path.replace('\\', "/");
    let width = |s: &str, ui: &egui::Ui| {
        ui.fonts_mut(|f| f.layout_no_wrap(s.to_string(), font.clone(), color::text()))
            .size()
            .x
    };
    while width(&shown, ui) > room && shown.chars().count() > 2 {
        let cut: String = shown.chars().skip(2).collect();
        shown = format!("…{}", cut.trim_start_matches('…'));
    }
    ui.painter().text(
        egui::pos2(rect.left() + 9.0 + 20.0, y),
        egui::Align2::LEFT_CENTER,
        shown,
        font,
        theme::text::strong(),
    );
    resp.on_hover_text(match changes {
        0 => format!("{path} · linked, nothing new since it was applied"),
        _ => format!("{path} · click to review what it changed"),
    })
    .clicked()
}

/// The *Styles* tab (§15 D1063): one group per card, in the inspector's
/// order, each row a preview, the style's whole name and its usage count; a
/// right-click or a double-click opens *Rename*, *Edit*, *Select*, *Delete*.
/// *In selection* keeps the styles the selection uses, a group's count reading
/// *1 of 4*, and a group with none of them drops out.
#[allow(clippy::too_many_arguments)]
fn styles_tab(
    ui: &mut egui::Ui,
    doc: &Document,
    users: &BTreeMap<StyleId, Vec<NodeId>>,
    picked: Option<&HashSet<StyleId>>,
    query: &str,
    b: &mut Browser,
    acts: &mut Vec<Act>,
) {
    // The document's own by card, then each kit's by card under the kit's
    // header — *kit groups last* (§15 D1065).
    let mut kits: Vec<(String, &str)> = doc
        .styles()
        .iter()
        .filter_map(|s| s.kit())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|k| (super::binding::kit_name(doc, k), k))
        .collect();
    kits.sort();
    let owners: Vec<Option<&str>> = std::iter::once(None)
        .chain(kits.iter().map(|(_, k)| Some(*k)))
        .collect();
    let groups: Vec<(Option<&str>, StyleSlot, Vec<&Style>, usize)> = owners
        .into_iter()
        .flat_map(|kit| StyleSlot::ALL.into_iter().map(move |slot| (kit, slot)))
        .map(|(kit, slot)| {
            let all: Vec<&Style> = doc
                .styles()
                .of_slot(slot)
                .filter(|s| s.kit() == kit)
                .collect();
            let total = all.len();
            let mut shown: Vec<&Style> = all
                .into_iter()
                .filter(|s| query.is_empty() || s.name.to_lowercase().contains(query))
                .filter(|s| picked.is_none_or(|p| p.contains(&s.id)))
                .collect();
            shown.sort_by(|a, b| a.name.cmp(&b.name));
            (kit, slot, shown, total)
        })
        .filter(|(_, _, shown, _)| !shown.is_empty())
        .collect();
    if groups.is_empty() {
        empty(
            ui,
            "No styles match",
            if b.in_selection {
                "Turn off In selection to see them all."
            } else if b.unused {
                "Turn off Unused to see them all."
            } else {
                "Try another name."
            },
        );
        return;
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            let mut header: Option<&str> = None;
            for (kit, slot, shown, total) in groups {
                if let Some(k) = kit
                    && header != Some(k)
                {
                    header = Some(k);
                    ui.add_space(6.0);
                    super::binding::kit_header(ui, &super::binding::kit_name(doc, k), b.inner_w);
                }
                let key = slot_key(kit, slot);
                let open = !b.collapsed.contains(&key) || !query.is_empty();
                ui.add_space(2.0);
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(b.inner_w, GROUP_H), egui::Sense::click());
                let p = ui.painter();
                ui::list_row_ground(p, rect, false, resp.hovered());
                ui::fold_caret(
                    p,
                    egui::pos2(rect.left() + 6.0, rect.center().y),
                    open,
                    theme::text::faint(),
                );
                let name = p.layout_no_wrap(
                    slot.title().to_string(),
                    egui::FontId::proportional(ui::SMALL_PT),
                    color::text(),
                );
                let name_w = name.size().x;
                p.galley(
                    egui::pos2(rect.left() + 23.0, rect.center().y - name.size().y / 2.0),
                    name,
                    color::text(),
                );
                let count = if picked.is_some() {
                    format!("{} of {total}", shown.len())
                } else {
                    total.to_string()
                };
                p.text(
                    egui::pos2(rect.left() + 23.0 + name_w + 6.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    count,
                    egui::FontId::proportional(ui::CAPTION_PT),
                    theme::text::faint(),
                );
                if resp.clicked() {
                    acts.push(Act::Toggle(key));
                }
                if !open {
                    continue;
                }
                // Then the slot's own groups, by the slash path in the names:
                // the top level first, with no header, then each group's.
                let mut groups: BTreeMap<&str, (Vec<&Style>, usize)> = BTreeMap::new();
                for s in doc.styles().of_slot(slot).filter(|s| s.kit() == kit) {
                    groups.entry(var::split_name(&s.name).0).or_default().1 += 1;
                }
                for s in shown {
                    groups
                        .entry(var::split_name(&s.name).0)
                        .or_default()
                        .0
                        .push(s);
                }
                for (path, (rows, total)) in groups {
                    if rows.is_empty() {
                        continue;
                    }
                    let depth = if path.is_empty() {
                        0.0
                    } else {
                        let g = StyleGroupRow {
                            kit,
                            slot,
                            path,
                            shown: rows.len(),
                            total,
                            filtered: picked.is_some(),
                        };
                        if !style_group_row(ui, &g, b, acts) {
                            continue;
                        }
                        GROUP_STEP
                    };
                    for s in rows {
                        let count = users.get(&s.id).map_or(0, Vec::len);
                        style_row(ui, doc, s, count, depth, b, acts);
                    }
                }
            }
            ui::keep_focus_in_view(ui);
        });
}

/// How far a style group sits in from its slot, and its rows with it.
const GROUP_STEP: f32 = 14.0;

/// A style group's header, as drawn.
struct StyleGroupRow<'a> {
    kit: Option<&'a str>,
    slot: StyleSlot,
    path: &'a str,
    shown: usize,
    total: usize,
    filtered: bool,
}

/// A style group's header, `GROUP_STEP` in from its slot's: its caret, its path,
/// its count. Double-click renames it, a right-click opens its menu; a kit's
/// folds and nothing else. Answers whether the group is open.
fn style_group_row(
    ui: &mut egui::Ui,
    g: &StyleGroupRow,
    b: &mut Browser,
    acts: &mut Vec<Act>,
) -> bool {
    ui.add_space(2.0);
    let key = style_group_key(g.kit, g.slot, g.path);
    let open = !b.collapsed.contains(&key) || !b.query.trim().is_empty();
    let id = ui.id().with(("style-group", &key));
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(b.inner_w, GROUP_H), egui::Sense::click());
    let left = rect.left() + GROUP_STEP;
    let refused = b.refused.clone();
    if let Some(Edit::StyleGroup { slot, path, text }) = b.edit.as_mut()
        && *slot == g.slot
        && path == g.path
        && g.kit.is_none()
    {
        let r = egui::Rect::from_min_max(
            egui::pos2(left + 20.0, rect.top() + 2.0),
            egui::pos2(rect.right() - 40.0, rect.bottom() - 2.0),
        );
        let resp = edit_field(
            ui,
            r,
            text,
            id.with("edit"),
            ui::SMALL_PT,
            None,
            refused.as_deref(),
        );
        if b.edit_focus {
            b.edit_focus = false;
            resp.request_focus();
        }
        ui::select_all_on_focus(ui, &resp, text);
        if ui::defocus_commits(&resp) {
            acts.push(Act::StyleGroupName(
                g.slot,
                g.path.to_string(),
                text.clone(),
            ));
        }
        return open;
    }
    let menu_open =
        matches!(&b.menu, Some((Menu::StyleGroup(s, p), _)) if *s == g.slot && p == g.path);
    let p = ui.painter();
    ui::list_row_ground(p, rect, menu_open, resp.hovered());
    ui::fold_caret(
        p,
        egui::pos2(left + 6.0, rect.center().y),
        open,
        theme::text::faint(),
    );
    let name = p.layout_no_wrap(
        g.path.to_string(),
        egui::FontId::proportional(ui::SMALL_PT),
        color::text(),
    );
    let name_w = name.size().x;
    p.galley(
        egui::pos2(left + 23.0, rect.center().y - name.size().y / 2.0),
        name,
        color::text(),
    );
    let count = if g.filtered {
        format!("{} of {}", g.shown, g.total)
    } else {
        g.total.to_string()
    };
    p.text(
        egui::pos2(left + 23.0 + name_w + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        count,
        egui::FontId::proportional(ui::CAPTION_PT),
        theme::text::faint(),
    );
    if g.kit.is_none() && (ui::double_clicked(ui, &resp) || ui::rename_key(ui, &resp)) {
        acts.push(Act::StartEdit(Edit::StyleGroup {
            slot: g.slot,
            path: g.path.to_string(),
            text: g.path.to_string(),
        }));
    } else if resp.clicked() && ui.input(|i| i.modifiers.alt) {
        acts.push(Act::ToggleAllStyles(key));
    } else if resp.clicked() {
        acts.push(Act::Toggle(key));
    } else if g.kit.is_none()
        && resp.secondary_clicked()
        && let Some(at) = resp.interact_pointer_pos()
    {
        acts.push(Act::OpenMenu(
            Menu::StyleGroup(g.slot, g.path.to_string()),
            at + egui::vec2(6.0, 8.0),
        ));
    }
    open
}

/// The description field in box `r` — where the row's rename field sits — when
/// the browser is typing `of`'s. Answers whether it drew, so the row draws
/// nothing else in its place.
fn describe_row(
    ui: &mut egui::Ui,
    r: egui::Rect,
    of: Described,
    b: &mut Browser,
    acts: &mut Vec<Act>,
) -> bool {
    let Some(Edit::Describe { of: o, text }) = b.edit.as_mut() else {
        return false;
    };
    if *o != of {
        return false;
    }
    let id = ui.id().with(("describe", of));
    let field = edit_field(ui, r, text, id, ui::SMALL_PT, Some("Description"), None);
    if b.edit_focus {
        b.edit_focus = false;
        field.request_focus();
    }
    if ui::defocus_commits(&field) {
        acts.push(Act::Describe(of, text.clone()));
    }
    true
}

/// One style's row on the *Styles* tab, `depth` in past its slot's: its preview,
/// its name past its group's path, its count. A rename types the whole name.
fn style_row(
    ui: &mut egui::Ui,
    doc: &Document,
    s: &Style,
    count: usize,
    depth: f32,
    b: &mut Browser,
    acts: &mut Vec<Act>,
) {
    let id = ui.id().with(("style-row", s.id));
    let (rect, mut resp) =
        ui.allocate_exact_size(egui::vec2(b.inner_w, ROW_H), egui::Sense::click());
    if !s.description.is_empty() {
        resp = resp.on_hover_text(&s.description);
    }
    let menu_open = matches!(&b.menu, Some((Menu::Style(m), _)) if *m == s.id);
    ui::list_row_ground(ui.painter(), rect, menu_open, resp.hovered());
    let sw = egui::Rect::from_min_size(
        egui::pos2(rect.left() + INDENT + depth, rect.center().y - 7.0),
        egui::vec2(14.0, 14.0),
    );
    crate::panels::styles::paint_preview(ui.painter(), doc, s, sw);
    // A rename's and a description's field share the box past the preview.
    let r = egui::Rect::from_min_max(
        egui::pos2(sw.right() + 6.0, rect.top() + 3.0),
        egui::pos2(rect.right() - 40.0, rect.bottom() - 3.0),
    );
    if describe_row(ui, r, Described::Style(s.id), b, acts) {
        return;
    }
    let refused = b.refused.clone();
    if let Some(Edit::Style { style, text }) = b.edit.as_mut()
        && *style == s.id
    {
        let resp = edit_field(
            ui,
            r,
            text,
            id.with("edit"),
            ui::SMALL_PT,
            None,
            refused.as_deref(),
        );
        if b.edit_focus {
            b.edit_focus = false;
            resp.request_focus();
        }
        ui::select_all_on_focus(ui, &resp, text);
        if ui::defocus_commits(&resp) {
            acts.push(Act::StyleName(s.id, text.clone()));
        }
        return;
    }
    let p = ui.painter();
    p.text(
        egui::pos2(sw.right() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        var::split_name(&s.name).1,
        egui::FontId::proportional(ui::SMALL_PT),
        theme::text::strong(),
    );
    p.text(
        egui::pos2(rect.right() - 6.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        count.to_string(),
        egui::FontId::proportional(ui::CAPTION_PT),
        theme::text::faint(),
    );
    let menu_at = |r: &egui::Response| {
        r.interact_pointer_pos().unwrap_or(rect.left_bottom()) + egui::vec2(6.0, 8.0)
    };
    if ui::double_clicked(ui, &resp) || resp.secondary_clicked() {
        acts.push(Act::OpenMenu(Menu::Style(s.id), menu_at(&resp)));
    }
}

/// The browser's empty state, under the tabs.
fn empty(ui: &mut egui::Ui, head: &str, sub: &str) {
    ui.add_space(40.0);
    ui::empty_state(ui, None, head, sub, |_| ());
}

/// The columns' x for mode `i`, scrolled.
fn mode_x(left: f32, i: usize, scroll: f32) -> f32 {
    left + NAME_W + MODE_W * i as f32 - scroll
}

/// *Name* and the mode headers — *Value* while there is one mode — each mode of
/// a collection picked by width followed by its minimum, the first's 0 left
/// unwritten. Double-click renames a mode; a right-click opens its menu.
fn header(
    ui: &mut egui::Ui,
    modes: &[Mode],
    widths: Option<&var::Breakpoints>,
    b: &mut Browser,
    acts: &mut Vec<Act>,
) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, HEAD_H), egui::Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        egui::Stroke::new(1.0, color::text_a(20)),
    );
    ui::paint_eyebrow(
        ui.painter(),
        egui::pos2(rect.left() + LEAD, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "Name",
    );
    let clip = egui::Rect::from_x_y_ranges(rect.left() + NAME_W..=rect.right(), rect.y_range());
    for (i, m) in modes.iter().enumerate() {
        let x = mode_x(rect.left(), i, b.scroll_x);
        let cell = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(MODE_W, HEAD_H));
        if !cell.intersects(clip) {
            continue;
        }
        let id = ui.id().with(("var-mode-head", m.id));
        let refused = b.refused.clone();
        if let Some(Edit::Mode { mode, text }) = b.edit.as_mut()
            && *mode == m.id
        {
            let r = cell.shrink2(egui::vec2(3.0, 2.0)).intersect(clip);
            let resp = edit_field(
                ui,
                r,
                text,
                id.with("edit"),
                ui::SMALL_PT,
                None,
                refused.as_deref(),
            );
            if b.edit_focus {
                b.edit_focus = false;
                resp.request_focus();
            }
            ui::select_all_on_focus(ui, &resp, text);
            if ui::defocus_commits(&resp) {
                acts.push(Act::ModeName(m.id, text.clone()));
            }
            continue;
        }
        let resp = ui.interact(cell.intersect(clip), id, egui::Sense::click());
        let label = if modes.len() == 1 {
            "Value"
        } else {
            m.name.as_str()
        };
        let painter = ui.painter().with_clip_rect(cell.intersect(clip));
        let named = painter.text(
            egui::pos2(cell.left() + 6.0, cell.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(ui::SMALL_PT),
            if resp.hovered() {
                theme::text::strong()
            } else {
                theme::text::muted()
            },
        );
        if let Some(w) = widths.and_then(|b| b.get(&m.id)).filter(|w| **w > 0.0) {
            painter.text(
                egui::pos2(named.right() + 4.0, cell.center().y),
                egui::Align2::LEFT_CENTER,
                format!("≥{}", var::px(*w)),
                egui::FontId::proportional(ui::CAPTION_PT),
                theme::text::faint(),
            );
        }
        if ui::double_clicked(ui, &resp) || ui::rename_key(ui, &resp) {
            acts.push(Act::StartEdit(Edit::Mode {
                mode: m.id,
                text: m.name.clone(),
            }));
        } else if resp.secondary_clicked()
            && let Some(at) = resp.interact_pointer_pos()
        {
            acts.push(Act::OpenMenu(Menu::Mode(m.id), at + egui::vec2(6.0, 8.0)));
        }
    }
}

/// A group's header: its caret, its path, its count (*1 of 3* while filtered).
/// A kit's folds and nothing else — no rename, no menu (§15 D1065).
#[allow(clippy::too_many_arguments)]
fn group_row(
    ui: &mut egui::Ui,
    path: &str,
    of: GroupOf<'_>,
    shown: usize,
    total: usize,
    filtered: bool,
    b: &mut Browser,
    acts: &mut Vec<Act>,
) {
    ui.add_space(2.0);
    let w = b.inner_w;
    let key = group_key(of, path);
    let id = ui.id().with(("var-group", &key));
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, GROUP_H), egui::Sense::click());
    let refused = b.refused.clone();
    if let Some(Edit::Group {
        collection,
        path: p,
        text,
    }) = b.edit.as_mut()
        && p == path
        && of.own() == Some(*collection)
    {
        let r = egui::Rect::from_min_size(
            rect.min + egui::vec2(20.0, 2.0),
            egui::vec2(NAME_W - 26.0, GROUP_H - 4.0),
        );
        let resp = edit_field(
            ui,
            r,
            text,
            id.with("edit"),
            ui::SMALL_PT,
            None,
            refused.as_deref(),
        );
        if b.edit_focus {
            b.edit_focus = false;
            resp.request_focus();
        }
        ui::select_all_on_focus(ui, &resp, text);
        if ui::defocus_commits(&resp) {
            acts.push(Act::GroupName(*collection, path.to_string(), text.clone()));
        }
        return;
    }
    let p = ui.painter();
    ui::list_row_ground(p, rect, false, resp.hovered());
    let open = !b.collapsed.contains(&key) || !b.query.trim().is_empty();
    ui::fold_caret(
        p,
        egui::pos2(rect.left() + 6.0, rect.center().y),
        open,
        theme::text::faint(),
    );
    let name = p.layout_no_wrap(
        path.to_string(),
        egui::FontId::proportional(ui::SMALL_PT),
        color::text(),
    );
    let name_w = name.size().x;
    p.galley(
        egui::pos2(rect.left() + 23.0, rect.center().y - name.size().y / 2.0),
        name,
        color::text(),
    );
    let count = if filtered {
        format!("{shown} of {total}")
    } else {
        total.to_string()
    };
    p.text(
        egui::pos2(rect.left() + 23.0 + name_w + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        count,
        egui::FontId::proportional(ui::CAPTION_PT),
        theme::text::faint(),
    );
    if let Some(collection) = of.own()
        && (ui::double_clicked(ui, &resp) || ui::rename_key(ui, &resp))
    {
        acts.push(Act::StartEdit(Edit::Group {
            collection,
            path: path.to_string(),
            text: path.to_string(),
        }));
    } else if resp.clicked() && ui.input(|i| i.modifiers.alt) {
        acts.push(Act::ToggleAll(key));
    } else if resp.clicked() {
        acts.push(Act::Toggle(key));
    } else if let Some(collection) = of.own()
        && resp.secondary_clicked()
        && let Some(at) = resp.interact_pointer_pos()
    {
        acts.push(Act::OpenMenu(
            Menu::Group(collection, path.to_string()),
            at + egui::vec2(6.0, 8.0),
        ));
    }
}

/// Where a variable row's name starts: past its group's caret, or at the
/// header's inset when it is in no group.
fn name_indent(v: &Variable) -> f32 {
    match var::split_name(&v.name).0.is_empty() {
        true => LEAD,
        false => INDENT,
    }
}

/// A variable's row: its leaf name and usage count, then a cell per mode.
#[allow(clippy::too_many_arguments)]
fn var_row(
    ui: &mut egui::Ui,
    doc: &Document,
    v: &Variable,
    modes: &[Mode],
    users: &[NodeId],
    b: &mut Browser,
    acts: &mut Vec<Act>,
    pending: Option<(VarId, ModeId, Color)>,
) {
    let table = doc.variables();
    let w = b.inner_w;
    let id = ui.id().with(("var-row", v.id));
    let indent = name_indent(v);
    // The bare flag: the name and the cells are the row's tab stops.
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, ROW_H), egui::Sense::CLICK);
    let menu_open = matches!(&b.menu, Some((Menu::Var(m), _)) if *m == v.id);
    // Not the row's own response for the hover: the name's and the cells' hit
    // areas lie over it and take it.
    ui::list_row_ground(
        ui.painter(),
        rect,
        menu_open,
        ui.rect_contains_pointer(rect),
    );
    // From where the rename field starts, across the cells: a variable's name
    // column is too narrow for a sentence.
    let field = egui::Rect::from_min_max(
        egui::pos2(rect.left() + indent - 6.0, rect.top() + 3.0),
        egui::pos2(rect.right() - 6.0, rect.bottom() - 3.0),
    );
    if describe_row(ui, field, Described::Variable(v.id), b, acts) {
        return;
    }
    let name_rect = egui::Rect::from_min_size(rect.min, egui::vec2(NAME_W, ROW_H));

    // The name: the leaf, or the whole name while it is being renamed.
    let editing_name = matches!(&b.edit, Some(Edit::Name { var, .. }) if *var == v.id);
    let refused = b.refused.clone();
    if let Some(Edit::Name { var: _, text }) = b.edit.as_mut().filter(|_| editing_name) {
        let r = egui::Rect::from_min_size(
            rect.min + egui::vec2(indent - 6.0, 3.0),
            egui::vec2(NAME_W - indent, ROW_H - 6.0),
        );
        let field = edit_field(
            ui,
            r,
            text,
            id.with("name"),
            ui::SMALL_PT,
            Some("name, / to group"),
            refused.as_deref(),
        );
        if b.edit_focus {
            b.edit_focus = false;
            field.request_focus();
        }
        ui::select_all_on_focus(ui, &field, text);
        if ui::defocus_commits(&field) {
            acts.push(Act::Name(v.id, text.clone()));
        }
    } else {
        let (_, leaf) = var::split_name(&v.name);
        let count = users.len();
        let count_text = count.to_string();
        let p = ui.painter();
        let count_galley = p.layout_no_wrap(
            count_text,
            egui::FontId::proportional(ui::CAPTION_PT),
            theme::text::faint(),
        );
        let count_rect = egui::Rect::from_min_size(
            egui::pos2(
                name_rect.right() - 6.0 - count_galley.size().x,
                rect.center().y - count_galley.size().y / 2.0,
            ),
            count_galley.size(),
        );
        let leaf_w = count_rect.left() - (rect.left() + indent) - 6.0;
        let galley = p.layout(
            leaf.to_string(),
            egui::FontId::proportional(ui::SMALL_PT),
            color::text(),
            f32::INFINITY,
        );
        let clip = egui::Rect::from_min_size(
            egui::pos2(rect.left() + indent, rect.top()),
            egui::vec2(leaf_w.max(0.0), ROW_H),
        );
        p.with_clip_rect(clip).galley(
            egui::pos2(
                rect.left() + indent,
                rect.center().y - galley.size().y / 2.0,
            ),
            galley,
            color::text(),
        );
        let count_resp = ui.interact(
            count_rect.expand(3.0),
            id.with("count"),
            egui::Sense::click(),
        );
        let count_col = if count_resp.hovered() && count > 0 {
            color::text()
        } else {
            theme::text::faint()
        };
        ui.painter().text(
            count_rect.left_center(),
            egui::Align2::LEFT_CENTER,
            count.to_string(),
            egui::FontId::proportional(ui::CAPTION_PT),
            count_col,
        );
        if count > 0 {
            let count_resp = count_resp.on_hover_text(layers_label(count));
            if count_resp.clicked() {
                acts.push(Act::Select(users.to_vec()));
            }
        }
        let mut name_resp = ui.interact(name_rect, id.with("name-hit"), egui::Sense::click());
        // The description, and under it the scope when it is not every field.
        let tip = match (v.description.is_empty(), scope_line(v)) {
            (true, None) => None,
            (false, None) => Some(v.description.clone()),
            (true, Some(s)) => Some(format!("Binds {s}")),
            (false, Some(s)) => Some(format!("{}\nBinds {s}", v.description)),
        };
        if let Some(tip) = tip {
            name_resp = name_resp.on_hover_text(tip);
        }
        if ui::double_clicked(ui, &name_resp) || ui::rename_key(ui, &name_resp) {
            acts.push(Act::StartEdit(Edit::Name {
                var: v.id,
                text: v.name.clone(),
            }));
        } else if name_resp.secondary_clicked()
            && let Some(at) = name_resp.interact_pointer_pos()
        {
            acts.push(Act::OpenMenu(Menu::Var(v.id), at + egui::vec2(6.0, 8.0)));
        }
    }

    // The cells.
    let clip = egui::Rect::from_x_y_ranges(rect.left() + NAME_W..=rect.right(), rect.y_range());
    for (i, m) in modes.iter().enumerate() {
        let x = mode_x(rect.left(), i, b.scroll_x);
        let cell = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(MODE_W, ROW_H));
        if !cell.intersects(clip) {
            continue;
        }
        let Some(c) = v.cells.get(&m.id) else {
            continue;
        };
        let editing =
            matches!(&b.edit, Some(Edit::Cell { var, mode, .. }) if *var == v.id && *mode == m.id);
        if editing {
            let Some(Edit::Cell { text, .. }) = b.edit.as_mut() else {
                unreachable!()
            };
            let r = cell.shrink2(egui::vec2(3.0, 3.0)).intersect(clip);
            let field = edit_field(
                ui,
                r,
                text,
                id.with(("cell", m.id)),
                ui::SMALL_PT,
                None,
                refused.as_deref(),
            );
            if b.edit_focus {
                b.edit_focus = false;
                field.request_focus();
            }
            ui::select_all_on_focus(ui, &field, text);
            let tab = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Tab));
            if tab {
                let next = modes.get(i + 1).map(|next| Edit::Cell {
                    var: v.id,
                    mode: next.id,
                    text: v
                        .cells
                        .get(&next.id)
                        .map(|c| cell_text(table, c))
                        .unwrap_or_default(),
                });
                let has_next = next.is_some();
                acts.push(Act::Cell(v.id, m.id, text.clone(), next));
                // Scrolled so the next column is wholly on show: a field off
                // the clip is never drawn nor focused, and the keys typed for
                // it would reach the canvas's shortcuts.
                if has_next {
                    let j = i + 1;
                    let lo = MODE_W * j.saturating_sub(MAX_COLS - 1) as f32;
                    b.scroll_x = b.scroll_x.clamp(lo, MODE_W * j as f32);
                }
            } else if ui::defocus_commits(&field) {
                acts.push(Act::Cell(v.id, m.id, text.clone(), None));
            }
            continue;
        }
        let cell_resp = ui.interact(
            cell.intersect(clip),
            id.with(("cell-hit", m.id)),
            egui::Sense::click(),
        );
        let p = ui.painter().with_clip_rect(clip);
        // The colour a drag on the popup is giving this cell, which the table
        // does not hold yet (§15 D1052's preview fork).
        let shown = match pending {
            Some((pv, pm, colour)) if pv == v.id && pm == m.id => Cell::Color(colour),
            _ => c.clone(),
        };
        draw_cell(&p, table, cell, &shown);
        // **A colour cell opens the colour popup; a double-click types into it**
        // (D1038 G-series, §15 D1052 — the gesture split the session's call): the
        // popup beside the panel, its *Variables* tab making an alias, and the
        // text for a hex or `--name` one click further. A number keeps the
        // single click, having no popup.
        let colour = v.kind == VarKind::Color;
        let start = Edit::Cell {
            var: v.id,
            mode: m.id,
            text: cell_text(table, c),
        };
        // `F2` types into the focused cell, as a spreadsheet's does.
        if colour && (ui::double_clicked(ui, &cell_resp) || ui::rename_key(ui, &cell_resp)) {
            acts.push(Act::StartEdit(start));
        } else if colour && cell_resp.clicked() {
            acts.push(Act::OpenPicker(
                v.id,
                m.id,
                egui::pos2(rect.right() + 20.0, cell.top() - 12.0),
            ));
        } else if let Cell::Boolean(b) = c
            && cell_resp.clicked()
            && cell_resp
                .interact_pointer_pos()
                .is_some_and(|at| at.x < cell.left() + 6.0 + 12.0 + 4.0)
        {
            // **A boolean's box toggles it** (§15 D1059); the word beside it
            // types, as a number's cell does — `--name` for an alias.
            acts.push(Act::Cell(
                v.id,
                m.id,
                super::binding::bool_word(!b).into(),
                None,
            ));
        } else if !var::flex_words(v).is_empty()
            && matches!(c, Cell::String(_))
            && !ui::double_clicked(ui, &cell_resp)
            && cell_resp.clicked()
        {
            // **A string of flex words picks one from a menu**; a double-click
            // still types, for an alias.
            acts.push(Act::OpenMenu(
                Menu::Words(v.id, m.id),
                cell.left_bottom() + egui::vec2(0.0, 2.0),
            ));
        } else if !colour && cell_resp.clicked() {
            acts.push(Act::StartEdit(start));
        } else if cell_resp.secondary_clicked()
            && let Some(at) = cell_resp.interact_pointer_pos()
        {
            acts.push(Act::OpenMenu(Menu::Var(v.id), at + egui::vec2(6.0, 8.0)));
        }
        if let Cell::Alias(t) = c
            && let Some(t) = table.get(*t)
        {
            cell_resp.on_hover_text(format!("uses {}", t.name));
        }
    }
}

/// The columns a kit's rows fill: each of the document's modes, or — in a
/// document with none of its own — one, the kit's default (§15 D1065).
fn kit_columns(modes: &[Mode]) -> Vec<Option<ModeId>> {
    if modes.is_empty() {
        vec![None]
    } else {
        modes.iter().map(|m| Some(m.id)).collect()
    }
}

/// A kit's header (D1038 (b), §15 D1065): the kit mark and its name, and in each
/// mode column **the per-kit mapping** — the kit mode that document mode reads,
/// *Day →* under *Light*. A click on one opens its menu: the kit's modes, and
/// *Match by name*.
///
/// While an update of the kit waits (`waiting`, held and published versions)
/// the name reads on *· v8 → v9* (§15 D1038's *Kit updates*), a click opening
/// the update summary; its rows keep showing what the document holds — the
/// summary is the one place that says what v9 changes.
fn kit_row(
    ui: &mut egui::Ui,
    doc: &Document,
    kit: &str,
    modes: &[Mode],
    waiting: Option<(u32, u32)>,
    b: &Browser,
    acts: &mut Vec<Act>,
) {
    let table = doc.variables();
    let Some(km) = table.kits().get(kit) else {
        return;
    };
    ui.add_space(6.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(b.inner_w, GROUP_H), egui::Sense::hover());
    let p = ui.painter();
    p.hline(
        rect.x_range(),
        rect.top(),
        egui::Stroke::new(1.0, color::text_a(20)),
    );
    let g = p.text(
        egui::pos2(rect.left() + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        icon::PACKAGE,
        theme::icon_font(12.0),
        theme::text::dim(),
    );
    let named = p.text(
        egui::pos2(g.right() + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        super::binding::kit_name(doc, kit),
        egui::FontId::proportional(ui::SMALL_PT),
        theme::text::strong(),
    );
    if let Some((held, next)) = waiting {
        let at = egui::pos2(named.right() + 5.0, rect.center().y);
        let galley = ui.ctx().fonts_mut(|f| {
            f.layout_no_wrap(
                format!("· v{held} → v{next}"),
                egui::FontId::proportional(ui::CAPTION_PT),
                theme::text::dim(),
            )
        });
        let hit = egui::Rect::from_min_size(
            at - egui::vec2(2.0, galley.size().y / 2.0 + 2.0),
            galley.size() + egui::vec2(4.0, 4.0),
        )
        .intersect(egui::Rect::from_x_y_ranges(
            rect.left()..=rect.left() + NAME_W,
            rect.y_range(),
        ));
        let resp = ui
            .interact(hit, ui.id().with(("kit-update", kit)), egui::Sense::click())
            .on_hover_text(format!("v{next} is waiting — click for what it changes"));
        let p = ui.painter().with_clip_rect(hit);
        p.galley(
            egui::pos2(at.x, at.y - galley.size().y / 2.0),
            galley,
            if resp.hovered() {
                theme::text::strong()
            } else {
                theme::text::dim()
            },
        );
        if resp.clicked() {
            acts.push(Act::KitUpdates);
        }
    }
    let clip = egui::Rect::from_x_y_ranges(rect.left() + NAME_W..=rect.right(), rect.y_range());
    for (i, col) in kit_columns(modes).into_iter().enumerate() {
        let x = mode_x(rect.left(), i, b.scroll_x);
        let cell =
            egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(MODE_W, GROUP_H));
        if !cell.intersects(clip) {
            continue;
        }
        let read = table.kit_mode(kit, col);
        let name = read
            .and_then(|m| km.mode(m))
            .map_or(String::new(), |m| format!("{} →", m.name));
        let hand = col.is_some_and(|c| km.map.contains_key(&c));
        let resp = ui.interact(
            cell.intersect(clip),
            ui.id().with(("kit-map", kit, i)),
            egui::Sense::click(),
        );
        let p = ui.painter().with_clip_rect(clip);
        if resp.hovered() && col.is_some() {
            p.rect_filled(cell.shrink2(egui::vec2(2.0, 3.0)), 4, color::text_a(16));
        }
        p.text(
            egui::pos2(cell.left() + 6.0, cell.center().y),
            egui::Align2::LEFT_CENTER,
            name,
            egui::FontId::proportional(ui::SMALL_PT),
            if hand {
                theme::text::strong()
            } else {
                theme::text::dim()
            },
        );
        if let Some(c) = col {
            let resp = resp.on_hover_text(if hand {
                "Set by hand — click to change"
            } else {
                "Matched by name, else the kit's first mode — click to change"
            });
            if resp.clicked() {
                acts.push(Act::OpenMenu(
                    Menu::KitMap(kit.to_string(), c),
                    cell.left_bottom() + egui::vec2(0.0, 2.0),
                ));
            }
        }
    }
}

/// A kit's variable's row — **inert** (D1038 G's *"a kit's rows are inert"*,
/// §15 D1065): its leaf name dimmed and its usage count, which selects its
/// users, then in each mode column the value it reads there through the mapping.
fn kit_var_row(
    ui: &mut egui::Ui,
    doc: &Document,
    v: &Variable,
    modes: &[Mode],
    users: &[NodeId],
    b: &Browser,
    acts: &mut Vec<Act>,
) {
    let table = doc.variables();
    let id = ui.id().with(("kit-var-row", v.id));
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(b.inner_w, ROW_H), egui::Sense::hover());
    // The pointer, not the row's response, which the count's and the cells' hit
    // areas lying over it take.
    ui::list_row_ground(ui.painter(), rect, false, ui.rect_contains_pointer(rect));
    let name_rect = egui::Rect::from_min_size(rect.min, egui::vec2(NAME_W, ROW_H));
    let (_, leaf) = var::split_name(&v.name);
    let count = users.len();
    let p = ui.painter();
    let count_galley = p.layout_no_wrap(
        count.to_string(),
        egui::FontId::proportional(ui::CAPTION_PT),
        theme::text::faint(),
    );
    let count_rect = egui::Rect::from_min_size(
        egui::pos2(
            name_rect.right() - 6.0 - count_galley.size().x,
            rect.center().y - count_galley.size().y / 2.0,
        ),
        count_galley.size(),
    );
    let clip = egui::Rect::from_min_max(
        egui::pos2(rect.left() + INDENT, rect.top()),
        egui::pos2(count_rect.left() - 6.0, rect.bottom()),
    );
    p.with_clip_rect(clip).text(
        egui::pos2(rect.left() + INDENT, rect.center().y),
        egui::Align2::LEFT_CENTER,
        leaf,
        egui::FontId::proportional(ui::SMALL_PT),
        theme::text::muted(),
    );
    let count_resp = ui.interact(
        count_rect.expand(3.0),
        id.with("count"),
        egui::Sense::click(),
    );
    ui.painter().text(
        count_rect.left_center(),
        egui::Align2::LEFT_CENTER,
        count.to_string(),
        egui::FontId::proportional(ui::CAPTION_PT),
        if count_resp.hovered() && count > 0 {
            color::text()
        } else {
            theme::text::faint()
        },
    );
    if count > 0 && count_resp.on_hover_text(layers_label(count)).clicked() {
        acts.push(Act::Select(users.to_vec()));
    }
    let clip = egui::Rect::from_x_y_ranges(rect.left() + NAME_W..=rect.right(), rect.y_range());
    for (i, col) in kit_columns(modes).into_iter().enumerate() {
        let x = mode_x(rect.left(), i, b.scroll_x);
        let cell = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(MODE_W, ROW_H));
        if !cell.intersects(clip) {
            continue;
        }
        let Some(value) = table.resolve_opt(v.id, col) else {
            continue;
        };
        draw_cell(
            &ui.painter().with_clip_rect(clip),
            table,
            cell,
            &Cell::from(value),
        );
    }
    let _ = resp.on_hover_text(format!(
        "From {} — edited in the kit, read-only here",
        super::binding::kit_name(doc, v.kit().unwrap_or(""))
    ));
}

/// One cell at rest: a colour's swatch and hex, a bare number, an alias's capsule
/// naming its target.
fn draw_cell(p: &egui::Painter, table: &ondin_core::Variables, cell: egui::Rect, c: &Cell) {
    let y = cell.center().y;
    let font = egui::FontId::proportional(ui::SMALL_PT);
    match c {
        Cell::Color(col) => {
            let sw = egui::Rect::from_center_size(
                egui::pos2(cell.left() + 6.0 + 6.0, y),
                egui::vec2(12.0, 12.0),
            );
            swatch(p, sw, *col);
            p.text(
                egui::pos2(sw.right() + 6.0, y + 0.5),
                egui::Align2::LEFT_CENTER,
                cell_text(table, c),
                font,
                theme::text::strong(),
            );
        }
        Cell::Number(x) => {
            p.text(
                egui::pos2(cell.left() + 6.0, y + 0.5),
                egui::Align2::LEFT_CENTER,
                number_text(*x),
                font,
                theme::text::strong(),
            );
        }
        // A string (§15 D1069): quoted, cut at the cell's edge by the painter's
        // clip — the whole string is one click away, in the field.
        Cell::String(s) => {
            p.text(
                egui::pos2(cell.left() + 6.0, y + 0.5),
                egui::Align2::LEFT_CENTER,
                string_text(s),
                font,
                theme::text::strong(),
            );
        }
        // A boolean (§15 D1059): a 12-point box where the colour cell has its
        // swatch, checked when true, then the word.
        Cell::Boolean(b) => {
            let bx = egui::Rect::from_center_size(
                egui::pos2(cell.left() + 6.0 + 6.0, y),
                egui::vec2(12.0, 12.0),
            );
            if *b {
                p.rect_filled(bx, 3, color::text_a(200));
                p.text(
                    bx.center(),
                    egui::Align2::CENTER_CENTER,
                    icon::CHECK,
                    theme::icon_font(10.0),
                    color::card(),
                );
            } else {
                p.rect_stroke(
                    bx,
                    3,
                    egui::Stroke::new(1.0, theme::color::text_a(102)),
                    egui::StrokeKind::Inside,
                );
            }
            p.text(
                egui::pos2(bx.right() + 6.0, y + 0.5),
                egui::Align2::LEFT_CENTER,
                super::binding::bool_word(*b),
                font,
                theme::text::strong(),
            );
        }
        Cell::Alias(t) => {
            let name = table.get(*t).map(|t| t.name.clone()).unwrap_or_default();
            let mut job = egui::text::LayoutJob::simple_singleline(name, font, color::text());
            job.wrap = egui::text::TextWrapping::truncate_at_width(MODE_W - 12.0 - 14.0);
            let galley = p.layout_job(job);
            let w = (galley.size().x + 14.0).min(MODE_W - 12.0);
            let cap = egui::Rect::from_min_size(
                egui::pos2(cell.left() + 6.0, y - 10.0),
                egui::vec2(w, 20.0),
            );
            p.rect_filled(cap, 10, color::text_a(28));
            p.with_clip_rect(cap.shrink2(egui::vec2(7.0, 0.0))).galley(
                egui::pos2(cap.left() + 7.0, y - galley.size().y / 2.0 + 0.5),
                galley,
                color::text(),
            );
        }
    }
}

/// The sideways bar under the mode columns, past three modes (§15 D1055): 4
/// points tall from the name column's edge to 12 short of the panel's right, 6
/// above its bottom — a 6% track and a 22% thumb as wide as the
/// share of the columns on show, where `scroll_x` puts it. Its thumb drags; the
/// wheel scrolls as before.
fn scroll_bar(ctx: &egui::Context, panel: egui::Rect, modes: usize, b: &mut Browser) {
    let track = egui::Rect::from_min_max(
        egui::pos2(panel.left() + PAD + NAME_W, panel.bottom() - 10.0),
        egui::pos2(panel.right() - 12.0, panel.bottom() - 6.0),
    );
    let total = MODE_W * modes as f32;
    let shown = MODE_W * MAX_COLS as f32;
    let max = total - shown;
    let thumb_w = (track.width() * shown / total).max(16.0);
    let room = (track.width() - thumb_w).max(1.0);
    let thumb = egui::Rect::from_min_size(
        egui::pos2(track.left() + room * (b.scroll_x / max), track.top()),
        egui::vec2(thumb_w, track.height()),
    );
    let (pos, pressed, down) = ctx.input(|i| {
        (
            i.pointer.interact_pos(),
            i.pointer.primary_pressed(),
            i.pointer.primary_down(),
        )
    });
    match (b.bar_drag, pos) {
        (None, Some(p)) if pressed && thumb.expand2(egui::vec2(0.0, 5.0)).contains(p) => {
            b.bar_drag = Some((p.x, b.scroll_x));
        }
        (Some((x0, s0)), Some(p)) if down => {
            b.scroll_x = (s0 + (p.x - x0) * max / room).clamp(0.0, max);
        }
        _ if !down => b.bar_drag = None,
        _ => {}
    }
    let p = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("var-browser"),
    ));
    p.rect_filled(track, 2, color::text_a(15));
    let thumb = egui::Rect::from_min_size(
        egui::pos2(track.left() + room * (b.scroll_x / max), track.top()),
        thumb.size(),
    );
    p.rect_filled(thumb, 2, color::text_a(56));
}

/// One of the browser's in-place fields, placed in `r` as `ui.put` places a
/// widget, and **refused as the inspector's fields are** (§15 D1038, D1053):
/// while its text is still the text a refusal answered
/// (`refused`), that text is laid out dim over a dotted underline, the
/// inspector's fields' own layouter and dots (`binding::refusal_galley`,
/// `underline_refusal`), and the sentence above the table says why. A keystroke
/// makes it ordinary text again.
///
/// `r` is the field: [`ui::inline_edit`]'s ground, its text
/// [`ui::INLINE_EDIT_PAD_X`] inside and centred on its height, and
/// [`ui::typing_edge`] while it holds focus. Not `inline_edit` itself, which
/// takes no layouter for the refusal or a hint.
fn edit_field(
    ui: &mut egui::Ui,
    r: egui::Rect,
    text: &mut String,
    id: egui::Id,
    pt: f32,
    hint: Option<&str>,
    refused: Option<&str>,
) -> egui::Response {
    let font = egui::FontId::proportional(pt);
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
        let at = (refused == Some(buf.as_str()) && !buf.as_str().is_empty()).then_some(0);
        super::binding::refusal_galley(ui, buf.as_str(), at, &font)
    };
    #[expect(
        clippy::disallowed_methods,
        reason = "the entry inside a composed field row, which paints the ground and ui::typing_edge itself"
    )]
    let mut edit = ui::edit_frame(
        ui,
        &egui::FontId::proportional(pt),
        r.height(),
        ui::INLINE_EDIT_PAD_X,
    )
    .on(egui::TextEdit::singleline(text))
    .id(id)
    .font(egui::FontId::proportional(pt))
    .layouter(&mut layouter);
    if let Some(h) = hint {
        edit = edit.hint_text(h);
    }
    // **Over the whole of `r`, its text inset and centred** (`ui::edit_frame`):
    // `centered_and_justified` stretches the edit to the box, and the padding,
    // being the edit's, takes a press anywhere in it.
    // **A child that leaves the parent's cursor alone**, not a scope: a scope
    // advances the cursor past what it drew, and a field inside a row the
    // caller had already allocated put the cursor back *above* that row's
    // bottom — the table under a mode being renamed came up five points.
    ui::paint_inline_ground(ui, r);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(
        egui::Layout::centered_and_justified(egui::Direction::TopDown),
    ));
    let out = edit.show(&mut child);
    let at = (refused == Some(text.as_str())).then_some(0);
    super::binding::underline_refusal(ui, &out, at);
    if out.response.response.has_focus() {
        ui::typing_edge(ui, r);
    }
    out.response.response
}

/// A 12×12 colour chip, the checkerboard under a translucent one — the
/// Fill row's look (§15 D1082).
pub(crate) fn swatch(p: &egui::Painter, rect: egui::Rect, c: Color) {
    let painter = p.with_clip_rect(rect.intersect(p.clip_rect()));
    ui::paint_colour_chip(&painter, rect, ui::color_to_egui(c));
}

/// The mode of collection `c` frame `id` inherits when it sets none of it — the
/// nearest ancestor frame's that sets one, picks one by its width or has its
/// contents pick one by a width query, and that frame, else the collection's
/// default. What `id` would pick by its own width is [`width_inherits`]'s.
pub(crate) fn inherited_mode(
    doc: &Document,
    id: NodeId,
    c: CollectionId,
) -> Option<(ModeId, Option<NodeId>)> {
    let col = doc.variables().collection(c)?;
    let mut at = doc.get(id)?.parent();
    while let Some(p) = at {
        let n = doc.get(p)?;
        let queried = n.queries().iter().filter_map(|q| doc.query_pick(p, q));
        if let Some(m) = n
            .modes()
            .iter()
            .copied()
            .chain(queried)
            .chain(doc.width_pick(p))
            .find(|m| col.has(*m))
        {
            return Some((m, Some(p)));
        }
        at = n.parent();
    }
    Some((col.default_mode()?, None))
}

/// Whether *Inherit* on `id`'s row for collection `c` means *By width*: `c` is
/// picked by width and `id` is a page, or a top-level frame whose page sets no
/// mode of `c`. A page has no width, so on it the choice leaves each top-level
/// frame to pick its own.
pub(crate) fn width_inherits(doc: &Document, id: NodeId, c: CollectionId) -> bool {
    let Some(col) = doc.variables().collection(c).filter(|c| c.by_width()) else {
        return false;
    };
    if doc.is_page(id) {
        return true;
    }
    doc.picks_by_width(id)
        && doc
            .get(id)
            .and_then(|n| n.parent())
            .and_then(|p| doc.get(p))
            .is_some_and(|page| !page.modes().iter().any(|m| col.has(*m)))
}

/// Whether `id`'s width queries are its own to edit, so its mode rows offer
/// *By width* beside *Inherit* and the *Widths* fold: a main, or a frame that
/// holds a query and follows no main — a detached instance keeps its main's,
/// and must be able to let them go.
pub(crate) fn node_is_main(doc: &Document, id: NodeId) -> bool {
    doc.get(id)
        .is_some_and(|n| n.component() || (n.link().is_none() && !n.queries().is_empty()))
}

/// The collections a frame's identity card gives a mode row: the one a
/// document with one collection has, once it has a mode; else each with two or
/// more modes — one mode has nothing to choose.
pub(crate) fn mode_rows(t: &ondin_core::Variables) -> Vec<CollectionId> {
    if !t.has_collections() {
        return match t.first_collection().modes.is_empty() {
            true => Vec::new(),
            false => vec![t.first_collection().id],
        };
    }
    t.collections()
        .iter()
        .filter(|c| c.modes.len() > 1)
        .map(|c| c.id)
        .collect()
}

impl OndinApp {
    /// A frame's or a page's mode rows, in its identity card under its name
    /// (D1038 (a); §15 D1042): one *Mode* row, or one per collection
    /// ([`mode_rows`]) under a *Modes* eyebrow, each labelled by its collection.
    pub(crate) fn frame_mode_row(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let doc = &self.session.doc;
        let Some(node) = doc.get(id) else { return };
        if !doc.is_page(id) && !matches!(node.kind(), ondin_core::NodeKind::Artboard { .. }) {
            return;
        }
        let table = doc.variables();
        let rows = mode_rows(table);
        match rows.as_slice() {
            [] => {}
            [c] => self.collection_mode_row(ui, id, *c, "Mode", 48.0),
            many => {
                let named: Vec<(CollectionId, String)> = many
                    .iter()
                    .filter_map(|c| Some((*c, table.collection(*c)?.name.clone())))
                    .collect();
                ui.label(ui::eyebrow("Modes"));
                ui.add_space(ui::SECTION_LABEL_GAP - ui.spacing().item_spacing.y);
                for (c, name) in named {
                    self.collection_mode_row(ui, id, c, &name, 74.0);
                }
            }
        }
    }

    /// One mode row of frame `id` for collection `c`, labelled `label` in a
    /// `label_w` column: set, the mode reads bright; inherited, dim and naming
    /// its source — *Light · from Hero / Desktop*, *Dark · from page*, *Light ·
    /// default*. A click opens the menu, *Inherit* first.
    fn collection_mode_row(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        c: CollectionId,
        row_label: &str,
        label_w: f32,
    ) {
        let doc = &self.session.doc;
        let Some(node) = doc.get(id) else { return };
        let page = doc.is_page(id);
        let table = doc.variables();
        let owner = var::ModeOwner::Collection(c);
        let by_width = width_inherits(doc, id, c);
        // A width query over `c` on this frame: its contents pick by its width.
        let query = node.query_of(table, owner).cloned();
        let (value, suffix, set) = match node.mode_of(table, owner).and_then(|m| table.mode(m)) {
            Some(m) => (m.name.clone(), None, true),
            None if query.is_some() => {
                let picked = query.as_ref().and_then(|q| doc.query_pick(id, q));
                let Some(m) = picked.and_then(|m| table.mode(m)) else {
                    return;
                };
                (m.name.clone(), Some("· by width".to_string()), false)
            }
            // A page has no width: each of its top-level frames picks its own.
            None if by_width && page => ("By width".to_string(), None, false),
            None if by_width => {
                let Some(m) = doc.width_mode(id).and_then(|m| table.mode(m)) else {
                    return;
                };
                (m.name.clone(), Some("· by width".to_string()), false)
            }
            None => {
                let Some((m, from)) = inherited_mode(doc, id, c) else {
                    return;
                };
                let name = table.mode(m).map(|m| m.name.clone()).unwrap_or_default();
                let from = match from.and_then(|f| doc.get(f)) {
                    Some(f) if doc.is_page(f.id()) => "· from page".into(),
                    Some(f) => format!("· from {}", crate::private::layer(doc, f.id())),
                    None => "· default".into(),
                };
                (name, Some(from), false)
            }
        };
        let open = self.mode_menu.is_some_and(|(n, x, _)| n == id && x == c);
        // **On an instance a mode the copy sets is an override** (D1038 (a), §15
        // D1048, D1042's *Fix*): the label bright with D981's dot, ↺ in its place
        // while the pointer is on the row, a click on it the main's mode back —
        // a field's mark, the label being this row's. The field written is the
        // frame's whole set; the row compares its own collection's mode in it.
        let of = |set: &Vec<ModeId>| {
            set.iter()
                .copied()
                .find(|m| table.owner_of(*m) == Some(owner))
        };
        let mark = self.sub_mark(
            &[id],
            |op| match op {
                ondin_core::Operation::SetModes { modes, .. } => Some(modes.clone()),
                _ => None,
            },
            |n| Some(n.modes().to_vec()),
            of,
            |set, v| *set = var::with_mode(table, set, owner, v),
            |id, modes| ondin_core::Operation::SetModes { id, modes },
            |v| match v.and_then(|m| table.mode(m)) {
                Some(m) => m.name.clone(),
                None => "Inherit".into(),
            },
        );
        let mut reset = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let over = ui.rect_contains_pointer(egui::Rect::from_min_size(
                ui.cursor().min,
                egui::vec2(ui.available_width(), 28.0),
            ));
            let sense = match mark {
                Some(_) => egui::Sense::click(),
                None => egui::Sense::hover(),
            };
            let (label, lresp) = ui.allocate_exact_size(egui::vec2(label_w, 28.0), sense);
            match &mark {
                Some(_) if over => {
                    ui.painter().text(
                        label.left_center() + egui::vec2(6.0, 0.0),
                        egui::Align2::CENTER_CENTER,
                        icon::ARROW_COUNTER_CLOCKWISE,
                        theme::icon_font(13.0),
                        color::text(),
                    );
                }
                Some(_) => {
                    let g = ui.painter().text(
                        label.left_center(),
                        egui::Align2::LEFT_CENTER,
                        row_label,
                        egui::FontId::proportional(ui::CAPTION_PT),
                        theme::text::strong(),
                    );
                    ui::override_dot(ui.painter(), egui::pos2(g.right() + 5.0, g.center().y));
                }
                None => {
                    ui.painter().text(
                        label.left_center(),
                        egui::Align2::LEFT_CENTER,
                        row_label,
                        egui::FontId::proportional(ui::CAPTION_PT),
                        theme::text::dim(),
                    );
                }
            }
            if let Some(m) = &mark {
                reset = lresp.on_hover_text(&m.tip).clicked();
            }
            let w = ui.available_width();
            // A dropdown's face: open, it wears the `accent_700` border.
            let state = match open {
                true => ui::FieldButton::Set,
                false => ui::FieldButton::Off,
            };
            let (rect, resp, _) = ui::button_face(ui, egui::vec2(w, ui::CONTROL_H), state);
            let p = ui.painter();
            let ink = if set {
                color::text()
            } else {
                theme::text::muted()
            };
            let galley = p.layout_no_wrap(value, egui::FontId::proportional(ui::CONTROL_PT), ink);
            let x = rect.left() + ui::FIELD_PAD_X;
            let gw = galley.size().x;
            p.galley(
                egui::pos2(x, rect.center().y - galley.size().y / 2.0),
                galley,
                ink,
            );
            if let Some(s) = suffix {
                p.text(
                    egui::pos2(x + gw + 5.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    s,
                    egui::FontId::proportional(ui::CAPTION_PT),
                    theme::text::faint(),
                );
            }
            p.text(
                egui::pos2(rect.right() - ui::FIELD_PAD_X, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                icon::CARET_DOWN,
                crate::theme::icon_font(ui::COMBO_CHEVRON_PT),
                ui::combo_chevron_ink(),
            );
            let resp = resp.on_hover_text(match (page, set) {
                (false, false) if query.is_some() => {
                    "Picked by this component's width — each instance picks by its own"
                }
                (false, false) if by_width => {
                    "Picked by this frame's width — pick a mode to set one whatever its width"
                }
                (true, false) if by_width => {
                    "Each top-level frame on this page picks its mode from its width"
                }
                (false, true) => "The mode this frame sets for itself and everything in it",
                (false, false) => {
                    "Inherited — pick a mode to set one for this frame and everything in it"
                }
                (true, true) => {
                    "The mode every frame on this page reads in, unless it sets its own"
                }
                (true, false) => {
                    "The document's default — pick a mode for every frame on this page"
                }
            });
            if resp.clicked() {
                self.mode_menu = if open { None } else { Some((id, c, rect)) };
            }
        });
        self.reset_marked(reset, &mark);
        if let Some(q) = query {
            // Measured empty, as CSS measures a container: a width from its
            // content is only its padding, rarely what was meant.
            let hugs =
                self.session.doc.get(id).is_some_and(|n| {
                    n.item().width == ondin_core::container::Dimension::FitContent
                });
            if hugs {
                ui.label(
                    egui::RichText::new("Measured empty, so its width is only its padding")
                        .size(ui::SMALL_PT)
                        .color(color::warn()),
                );
            }
            if node_is_main(&self.session.doc, id) {
                self.widths_fold(ui, id, c, &q);
            }
        }
    }

    /// The *Widths* fold under a main's mode row while it picks by width: a
    /// field per mode after the first, its minimum width, labelled with the
    /// mode's name. A drag holds a draft committed when it ends; a typed width
    /// lands on `Enter` or defocus, one step *Breakpoint*; a refusal is said on
    /// the status line.
    fn widths_fold(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        c: CollectionId,
        q: &var::Breakpoints,
    ) {
        let key = (id, c);
        let open = !self.widths_folded.contains(&key);
        let (head, _) = ui::fold_head(ui, "Widths", open);
        if head.clicked() {
            match open {
                true => self.widths_folded.insert(key),
                false => self.widths_folded.remove(&key),
            };
        }
        if !open {
            return;
        }
        // Narrowest first, by the query's own widths: a mode moved in its
        // collection keeps its place here.
        let modes: Vec<Mode> = var::query_order(
            self.session.doc.variables(),
            var::ModeOwner::Collection(c),
            q,
        )
        .into_iter()
        .map(|(id, name)| Mode { id, name })
        .collect();
        let Some(first) = modes.first() else { return };
        ui.label(
            egui::RichText::new(format!("{} below {}", first.name, {
                modes
                    .get(1)
                    .and_then(|m| q.get(&m.id))
                    .map_or_else(String::new, |w| var::px(*w))
            }))
            .size(ui::SMALL_PT)
            .color(theme::text::dim()),
        );
        let cancelled = self.gesture_cancelled;
        if cancelled {
            self.query_draft = None;
        }
        let mut commit: Option<(ModeId, f64)> = None;
        for m in &modes[1..] {
            let Some(min) = q.get(&m.id).copied() else {
                continue;
            };
            let mut w = match self.query_draft {
                Some((n, mode, w)) if n == id && mode == m.id => w,
                _ => min,
            };
            let done = ui::labelled(ui, &m.name, |ui| {
                let size = egui::vec2(ui.available_width(), ui::CONTROL_H);
                let (resp, _) = ui::value_field_suffixed(
                    ui,
                    size,
                    ui::Prefix::Icon(icon::DEVICES),
                    Some(ui::Suffix {
                        text: "px",
                        clickable: false,
                        tooltip: "",
                    }),
                    &mut w,
                    ui::Scrub::whole(0.5).range(0.0..=f64::from(u16::MAX)),
                    |d| d.custom_formatter(ui::number(2)),
                );
                let resp = resp.on_hover_text(format!("{} from this width up", m.name));
                let done = (resp.changed() && !resp.dragged()) || resp.drag_stopped();
                if !done && resp.changed() {
                    self.query_draft = Some((id, m.id, w));
                }
                done
            });
            if done && !cancelled {
                self.query_draft = None;
                commit = Some((m.id, w));
            }
        }
        if let Some((mode, w)) = commit {
            match var::query_width_ops(&self.session.doc, id, mode, w) {
                Ok(ops) if ops.is_empty() => {}
                Ok(ops) => {
                    self.session.name_step("Breakpoint", icon::DEVICES);
                    self.commit_edit(Transaction(ops));
                }
                Err(why) => self.session.info(why),
            }
        }
    }

    /// The *Mode* menu of one collection: *Inherit* with the mode it would
    /// inherit, then each of its modes, the frame's own checked. One pick is one
    /// `SetModes`, every other collection's mode kept.
    pub(crate) fn mode_menu_ui(&mut self, ctx: &egui::Context) {
        let Some((id, c, under)) = self.mode_menu else {
            return;
        };
        let doc = &self.session.doc;
        let Some(node) = doc.get(id) else {
            self.mode_menu = None;
            return;
        };
        let table = doc.variables();
        let Some(col) = table.collection(c) else {
            self.mode_menu = None;
            return;
        };
        let modes: Vec<Mode> = col.modes.clone();
        let owner = var::ModeOwner::Collection(c);
        let current = node.mode_of(table, owner);
        let set = node.modes().to_vec();
        // On a top-level frame or a page of a collection picked by width,
        // *Inherit* is *By width*, naming the mode the frame picks now.
        let by_width = width_inherits(doc, id, c);
        let inherits = match by_width {
            true => doc.width_mode(id),
            false => inherited_mode(doc, id, c).map(|(m, _)| m),
        }
        .and_then(|m| table.mode(m))
        .map(|m| m.name.clone());
        // A width query over `c` on this frame — a main's own, or its instance's
        // following it — and whether this frame is the main that sets one.
        let query = node.query_of(table, owner).cloned();
        let main = node_is_main(doc, id);
        let refusal = var::why_no_query(doc, id);
        let picked = query
            .as_ref()
            .and_then(|q| doc.query_pick(id, q))
            .and_then(|m| table.mode(m))
            .map(|m| m.name.clone());
        // The accelerators: this frame's own widths while it queries, else the
        // collection's; the first mode has none.
        let widths: Vec<Option<String>> = modes
            .iter()
            .map(|m| {
                match &query {
                    Some(q) => q.get(&m.id).copied(),
                    None => col.min_width(m.id),
                }
                .filter(|w| *w > 0.0)
                .map(|w| format!("≥{}", var::px(w)))
            })
            .collect();
        let w = under.width().max(200.0);
        // *Inherit*, *By width* or both, a rule, then one row per mode.
        let heads = if main { 2 } else { 1 };
        let size = egui::vec2(w, ui::menu_height(modes.len() + heads, 1));
        let pos = ui::dropdown_place(ctx, under, size, 4.0);
        /// What a row of this menu picks.
        enum Pick {
            Inherit,
            ByWidth,
            Mode(ModeId),
        }
        let mut pick: Option<Pick> = None;
        let rect = egui::Area::new(egui::Id::new("frame-mode-menu"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                ui::menu_frame(ui::MENU_PAD).show(ui, |ui| {
                    ui.set_width(ui::menu_inner_w(w, ui::MENU_PAD));
                    ui.spacing_mut().item_spacing.y = 1.0;
                    if main {
                        // A main keeps both: inheriting from around it is still a
                        // real choice there.
                        let inherit = ui::MenuRow::new(icon::ARROW_ELBOW_LEFT_UP, "Inherit")
                            .accel(inherits.as_deref())
                            .checked(current.is_none() && query.is_none());
                        if ui::menu_row(ui, inherit, MENU_ROW_H).clicked() {
                            pick = Some(Pick::Inherit);
                        }
                        let row = ui::MenuRow::new(icon::DEVICES, "By width")
                            .accel(picked.as_deref())
                            .checked(current.is_none() && query.is_some())
                            .enabled(refusal.is_none());
                        let resp = ui::menu_row(ui, row, MENU_ROW_H);
                        let resp = match refusal {
                            Some(why) => resp.on_disabled_hover_text(why),
                            None => resp.on_hover_text(
                                "Picked by this component's width — each instance picks by its own",
                            ),
                        };
                        if refusal.is_none() && resp.clicked() {
                            pick = Some(Pick::ByWidth);
                        }
                    } else {
                        let row = match (query.is_some(), by_width) {
                            (true, _) => {
                                ui::MenuRow::new(icon::DEVICES, "By width").accel(picked.as_deref())
                            }
                            (false, true) => ui::MenuRow::new(icon::DEVICES, "By width")
                                .accel(inherits.as_deref()),
                            (false, false) => {
                                ui::MenuRow::new(icon::ARROW_ELBOW_LEFT_UP, "Inherit")
                                    .accel(inherits.as_deref())
                            }
                        }
                        .checked(current.is_none());
                        if ui::menu_row(ui, row, MENU_ROW_H).clicked() {
                            pick = Some(Pick::Inherit);
                        }
                    }
                    ui::menu_sep(ui);
                    for (m, width) in modes.iter().zip(&widths) {
                        let row = ui::MenuRow::new(icon::MODE, &m.name)
                            .accel(width.as_deref())
                            .checked(current == Some(m.id));
                        if ui::menu_row(ui, row, MENU_ROW_H).clicked() {
                            pick = Some(Pick::Mode(m.id));
                        }
                    }
                });
            })
            .response
            .rect;
        if let Some(pick) = pick {
            self.mode_menu = None;
            let doc = &self.session.doc;
            let t = doc.variables();
            let pin = |mode: Option<ModeId>| -> Vec<Operation> {
                match mode == current {
                    true => Vec::new(),
                    false => vec![Operation::SetModes {
                        id,
                        modes: var::with_mode(t, &set, owner, mode),
                    }],
                }
            };
            // On a main, *Inherit* and a mode each end the query; on any other
            // frame the query, an instance's following its main, is left alone.
            let unquery = || match main {
                true => var::query_ops(doc, id, owner, false).unwrap_or_default(),
                false => Vec::new(),
            };
            let (ops, step) = match pick {
                Pick::Inherit => ([pin(None), unquery()].concat(), ("Mode", icon::MODE)),
                Pick::Mode(m) => ([pin(Some(m)), unquery()].concat(), ("Mode", icon::MODE)),
                Pick::ByWidth => match var::query_ops(doc, id, owner, true) {
                    Ok(ops) => (ops, ("By width", icon::DEVICES)),
                    Err(why) => {
                        self.session.info(why);
                        return;
                    }
                },
            };
            if !ops.is_empty() {
                self.session.name_step(step.0, step.1);
                self.commit_edit(Transaction(ops));
            }
            return;
        }
        let away = ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !rect.contains(p) && !under.contains(p))
        });
        if away {
            self.mode_menu = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::{RoundedRectRadii, Size};
    use ondin_core::{BrushSource, Fill, NodeKind, keyed_by_position};

    fn app() -> (egui::Context, OndinApp) {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.canvas_rect =
            egui::Rect::from_min_max(egui::pos2(300.0, 40.0), egui::pos2(900.0, 780.0));
        let _ = ctx.run_ui(Default::default(), |_| {});
        (ctx, app)
    }

    /// One frame of the browser on a 1200×800 screen.
    fn frame(app: &mut OndinApp, ctx: &egui::Context) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1200.0, 800.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |_| app.var_browser_ui(ctx));
    }

    /// Every piece of text `draw` lays out, on its second frame: an `Area` draws
    /// nothing on its first, which only sizes it.
    fn texts(ctx: &egui::Context, draw: impl FnMut(&egui::Ui)) -> Vec<String> {
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1200.0, 800.0),
            )),
            ..Default::default()
        };
        let mut draw = draw;
        let _ = ctx.run_ui(input(), |ui| draw(ui));
        let out = ctx.run_ui(input(), |ui| draw(ui));
        out.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    fn the_only_var(app: &OndinApp) -> Variable {
        let vars: Vec<&Variable> = app.session.doc.variables().iter().collect();
        assert_eq!(vars.len(), 1, "the fixture: one variable");
        vars[0].clone()
    }

    /// **A first variable brings the document's first mode with it**, one cell
    /// in it, and is named to be typed over. A variable has a value in every
    /// mode, so with no mode there is nowhere for the value to go.
    #[test]
    fn a_first_variable_brings_a_mode_and_opens_its_name() {
        let (_ctx, mut app) = app();
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Color));
        let table = app.session.doc.variables();
        assert_eq!(table.modes().count(), 1);
        assert_eq!(table.modes().next().unwrap().name, "Mode 1");
        let v = the_only_var(&app);
        assert_eq!(v.name, "colour 1");
        assert_eq!(
            v.cells.values().cloned().collect::<Vec<_>>(),
            vec![Cell::Color(Color::WHITE)]
        );
        let b = app.var_browser.as_ref().unwrap();
        assert!(
            matches!(&b.edit, Some(Edit::Name { var, .. }) if *var == v.id),
            "renaming it"
        );
        // And *New mode* copies the first mode's value into a second column.
        app.var_act(Act::NewMode);
        let v = the_only_var(&app);
        assert_eq!(app.session.doc.variables().modes().count(), 2);
        assert_eq!(v.cells.len(), 2);
        assert!(v.cells.values().all(|c| *c == Cell::Color(Color::WHITE)));
    }

    /// **A variable row lights under the pointer**, over its name as a group's
    /// and a style's rows do, though the name's own hit area lies on top.
    ///
    /// flip: `var_row` asking its own response `hovered()` → fails at "lit".
    #[test]
    fn a_variable_row_lights_under_the_pointer() {
        let (ctx, mut app) = app();
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Color));
        app.var_browser.as_mut().unwrap().edit = None;
        let mut shapes = Vec::new();
        for _ in 0..2 {
            shapes = pass(&ctx, &mut app, vec![], |app, ctx| app.var_browser_ui(ctx));
        }
        let at = last_text(&shapes, "colour 1").expect("the row's name");
        let hover = vec![egui::Event::PointerMoved(at)];
        let mut shapes = Vec::new();
        for _ in 0..3 {
            shapes = pass(&ctx, &mut app, hover.clone(), |app, ctx| {
                app.var_browser_ui(ctx)
            });
        }
        // A row-tall fill under the pointer, any alpha: the browser's `Area`
        // fades in, so the colour is not yet `text_a(18)`.
        fn lit(shape: &egui::Shape, at: egui::Pos2) -> bool {
            match shape {
                egui::Shape::Rect(r) => {
                    r.rect.contains(at) && r.rect.height() == ROW_H && r.fill.a() > 0
                }
                egui::Shape::Vec(v) => v.iter().any(|x| lit(x, at)),
                _ => false,
            }
        }
        assert!(shapes.iter().any(|c| lit(&c.shape, at)), "lit");
    }

    /// The browser and the insert picker share one place: opening either
    /// closes the other.
    #[test]
    fn the_browser_and_the_insert_picker_take_turns() {
        let (_ctx, mut app) = app();
        app.toggle_var_browser();
        assert!(app.var_browser.is_some());
        app.toggle_kit_picker();
        assert!(app.var_browser.is_none() && app.kit_picker.is_some());
        app.toggle_var_browser();
        assert!(app.var_browser.is_some() && app.kit_picker.is_none());
    }

    /// A cell takes a colour — its alpha too, a colour variable carrying its own
    /// (D1038) — an alias as `--name`, and refuses a loop with a sentence,
    /// keeping the field open.
    #[test]
    fn a_cell_takes_a_hex_an_alias_and_refuses_a_loop() {
        let (_ctx, mut app) = app();
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Color));
        app.var_act(Act::NewVar(VarKind::Color));
        let table = app.session.doc.variables();
        let mode = table.default_mode().unwrap();
        let a = table.by_name("colour 1").unwrap().id;
        let b = table.by_name("colour 2").unwrap().id;
        app.var_act(Act::Cell(a, mode, "#8BA5E080".into(), None));
        assert_eq!(
            app.session.doc.variables().get(a).unwrap().cells[&mode],
            Cell::Color(Color::from_rgba8(0x8b, 0xa5, 0xe0, 0x80))
        );
        app.var_act(Act::Cell(b, mode, "--colour 1".into(), None));
        assert_eq!(
            app.session.doc.variables().get(b).unwrap().cells[&mode],
            Cell::Alias(a)
        );
        let before = app.session.doc.clone();
        app.var_act(Act::StartEdit(Edit::Cell {
            var: a,
            mode,
            text: String::new(),
        }));
        app.var_act(Act::Cell(a, mode, "--colour 2".into(), None));
        assert_eq!(app.session.doc, before, "a loop is not written");
        let br = app.var_browser.as_ref().unwrap();
        assert!(
            br.refusal.as_deref().is_some_and(|w| w.contains("loop")),
            "{:?}",
            br.refusal
        );
        assert!(br.edit.is_some(), "the field stays open to be fixed");
    }

    /// **A refused value keeps its field through a `Tab`** — the next mode's
    /// cell opens only once the value has landed. Were the commit and the next
    /// cell's edit two acts, the second would replace the refused field and clear
    /// its sentence: the typed value would vanish with nothing said.
    ///
    /// Flip, run: opening `next` whatever `var_cell` answered fails
    /// *"the refused field is kept"*.
    #[test]
    fn a_refused_tab_keeps_the_typed_cell() {
        let (_ctx, mut app) = app();
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Color));
        app.var_act(Act::NewMode);
        let v = the_only_var(&app);
        let modes: Vec<ModeId> = app.session.doc.variables().modes().map(|m| m.id).collect();
        let next = Some(Edit::Cell {
            var: v.id,
            mode: modes[1],
            text: "FFFFFF".into(),
        });
        app.var_act(Act::StartEdit(Edit::Cell {
            var: v.id,
            mode: modes[0],
            text: "nope".into(),
        }));
        app.var_act(Act::Cell(v.id, modes[0], "nope".into(), next.clone()));
        let b = app.var_browser.as_ref().unwrap();
        assert!(
            matches!(&b.edit, Some(Edit::Cell { mode, .. }) if *mode == modes[0]),
            "the refused field is kept: {:?}",
            b.edit
        );
        assert!(b.refusal.is_some(), "and says why");
        app.var_act(Act::Cell(v.id, modes[0], "336699".into(), next));
        let b = app.var_browser.as_ref().unwrap();
        assert!(
            matches!(&b.edit, Some(Edit::Cell { mode, .. }) if *mode == modes[1]),
            "a value that lands moves on"
        );
    }

    /// **Deleting a used variable detaches its users and says so**, and one
    /// undo brings the binding back.
    #[test]
    fn deleting_a_used_variable_detaches_and_says_so() {
        let (_ctx, mut app) = app();
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Color));
        let v = the_only_var(&app);
        let rect = app.session.ids.mint();
        let root = app.session.doc.first_page();
        assert!(app.commit_edit(Transaction(vec![
            Operation::CreateNode {
                id: rect,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id: rect,
                fills: keyed_by_position([Fill {
                    brush: BrushSource::Bound {
                        var: v.id,
                        alpha: 1.0
                    },
                    visible: true,
                }]),
            },
        ])));
        app.var_act(Act::Delete(vec![v.id], v.name.clone()));
        assert!(app.session.doc.variables().get(v.id).is_none());
        let fill = &app.session.doc.get(rect).unwrap().paint().fills[0];
        assert_eq!(fill.brush, BrushSource::from(Color::WHITE));
        assert_eq!(
            app.session.status().text,
            "Deleted colour 1. 1 layer now uses #FFFFFF directly."
        );
        app.session.undo();
        let fill = &app.session.doc.get(rect).unwrap().paint().fills[0];
        assert_eq!(
            fill.brush.var(),
            Some(v.id),
            "one undo restores the binding"
        );
    }

    /// *Unused* finds the document's own styles no layer uses, and *Delete unused*
    /// takes exactly those in one undo step.
    /// flip: `unused_styles` ignoring usage → fails at "only the idle style is unused".
    #[test]
    fn delete_unused_takes_only_the_styles_no_layer_uses() {
        let (ctx, mut app) = app();
        let rect = app.session.ids.mint();
        let root = app.session.doc.first_page();
        let item = app.session.ids.mint_item();
        let (used, idle) = (app.session.ids.mint_style(), app.session.ids.mint_style());
        let style = |id, name: &str| Operation::AddStyle {
            style: Style {
                id,
                name: name.into(),
                value: sty::StyleValue::Fills(vec![ondin_core::item::Keyed::new(
                    item,
                    Fill {
                        brush: Color::WHITE.into(),
                        visible: true,
                    },
                )]),
                origin: None,
                forked: None,
                description: String::new(),
            },
        };
        assert!(app.commit_edit(Transaction(vec![
            Operation::CreateNode {
                id: rect,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            style(used, "surface/raised"),
            style(idle, "surface/old"),
            Operation::SetFillStyle {
                id: rect,
                style: Some(used),
            },
        ])));
        let doc = &app.session.doc;
        let users: BTreeMap<StyleId, Vec<NodeId>> = doc
            .styles()
            .iter()
            .map(|s| (s.id, sty::users(doc, s.id)))
            .collect();
        let unused = unused_styles(doc, &users);
        assert_eq!(unused, [idle], "only the idle style is unused");

        app.toggle_var_browser();
        let b = app.var_browser.as_mut().unwrap();
        b.tab = Tab::Styles;
        b.unused = true;
        pass(&ctx, &mut app, vec![], |app, ctx| app.var_browser_ui(ctx));
        let depth = app.session.history.undo_depth();
        app.var_act(Act::DeleteStyles(unused, "1 unused style".into()));
        let styles = app.session.doc.styles();
        assert!(styles.get(idle).is_none(), "the unused style is gone");
        assert!(styles.get(used).is_some(), "the used style stays");
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one undo step");
    }

    /// **The *Styles* tab lists a style under its card and its path's group, and
    /// *Delete* detaches its users, says so, and undoes whole** (D1038 (6);
    /// §15 D1063). A folded group hides its rows, *Rename group* moves its
    /// styles, and a rename to a name its slot already holds is refused.
    #[test]
    fn the_styles_tab_lists_renames_and_deletes() {
        let (ctx, mut app) = app();
        let rect = app.session.ids.mint();
        let root = app.session.doc.first_page();
        let item = app.session.ids.mint_item();
        let (s, t) = (app.session.ids.mint_style(), app.session.ids.mint_style());
        let style = |id, name: &str| Operation::AddStyle {
            style: Style {
                id,
                name: name.into(),
                value: sty::StyleValue::Fills(vec![ondin_core::item::Keyed::new(
                    item,
                    Fill {
                        brush: Color::WHITE.into(),
                        visible: true,
                    },
                )]),
                origin: None,
                forked: None,
                description: String::new(),
            },
        };
        assert!(app.commit_edit(Transaction(vec![
            Operation::CreateNode {
                id: rect,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            style(s, "surface/raised"),
            style(t, "surface/base"),
            Operation::SetFillStyle {
                id: rect,
                style: Some(s),
            },
        ])));
        assert_eq!(
            app.session.doc.get(rect).unwrap().paint().fills.len(),
            1,
            "the commit wrote the style's fill"
        );
        app.toggle_var_browser();
        app.var_browser.as_mut().unwrap().tab = Tab::Styles;
        let mut shapes = vec![];
        for _ in 0..2 {
            shapes = pass(&ctx, &mut app, vec![], |app, ctx| app.var_browser_ui(ctx));
        }
        assert!(last_text(&shapes, "raised").is_some(), "the row is drawn");
        assert!(
            last_text(&shapes, "surface").is_some(),
            "under its path's group"
        );
        assert!(
            last_text(&shapes, "Fill").is_some(),
            "under its card's group"
        );
        let key = style_group_key(None, StyleSlot::Fills, "surface");
        app.var_act(Act::Toggle(key.clone()));
        let shapes = pass(&ctx, &mut app, vec![], |app, ctx| app.var_browser_ui(ctx));
        assert!(
            last_text(&shapes, "raised").is_none(),
            "a folded group hides its rows"
        );
        assert!(
            last_text(&shapes, "surface").is_some(),
            "and keeps its header"
        );
        app.var_act(Act::Toggle(key));

        // *Rename group* moves both, and back.
        app.var_act(Act::StyleGroupName(
            StyleSlot::Fills,
            "surface".into(),
            "card".into(),
        ));
        let name = |app: &OndinApp, id| app.session.doc.styles().get(id).unwrap().name.clone();
        assert_eq!(
            (name(&app, s), name(&app, t)),
            ("card/raised".into(), "card/base".into())
        );
        app.var_act(Act::StyleGroupName(
            StyleSlot::Fills,
            "card".into(),
            "surface".into(),
        ));
        assert_eq!(name(&app, s), "surface/raised");
        assert_eq!(
            style_group_members(&app.session.doc, StyleSlot::Fills, "surface"),
            [s, t]
        );

        // *Description* types where the name was, the preview kept, and
        // commits trimmed.
        let shapes = pass(&ctx, &mut app, vec![], |app, ctx| app.var_browser_ui(ctx));
        let name_at = text_left(&shapes, "base").expect("the leaf is drawn");
        app.var_act(Act::StartEdit(Edit::Describe {
            of: Described::Style(t),
            text: String::new(),
        }));
        let shapes = pass(&ctx, &mut app, vec![], |app, ctx| app.var_browser_ui(ctx));
        assert!(
            last_text(&shapes, "base").is_none(),
            "the field covers the name"
        );
        let hint_at = text_left(&shapes, "Description").expect("the field's hint");
        assert!(
            (hint_at - name_at).abs() <= 6.0,
            "the field sits where the name was: {hint_at} vs {name_at}"
        );
        app.var_act(Act::Describe(Described::Style(t), " The page ".into()));
        let desc = |app: &OndinApp| app.session.doc.styles().get(t).unwrap().description.clone();
        assert_eq!(desc(&app), "The page");
        assert!(app.var_browser.as_ref().unwrap().edit.is_none());

        app.var_act(Act::StartEdit(Edit::Style {
            style: s,
            text: "surface/base".into(),
        }));
        app.var_act(Act::StyleName(s, "surface/base".into()));
        assert_eq!(
            app.var_browser.as_ref().unwrap().refusal.as_deref(),
            Some("surface/base already exists.")
        );

        app.var_act(Act::DeleteStyles(vec![s], "surface/raised".into()));
        assert!(app.session.doc.styles().get(s).is_none());
        let n = app.session.doc.get(rect).unwrap();
        assert_eq!(n.styles().fills, None);
        assert_eq!(n.paint().fills.len(), 1, "the values stay");
        assert_eq!(
            app.session.status().text,
            "Deleted surface/raised. 1 layer keeps its values."
        );
        app.session.undo();
        assert_eq!(app.session.doc.get(rect).unwrap().styles().fills, Some(s));
    }

    /// **A boolean variable** (§15 D1059): the `+` menu's *Boolean variable* files
    /// one true in every mode; its box's click writes the word for the other
    /// value, which lands as `false`; a typed `no` is false too, and a typed `7`
    /// is refused with the kind's own sentence.
    #[test]
    fn a_boolean_variable_is_made_toggled_and_typed() {
        let (_ctx, mut app) = app();
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Boolean));
        let table = app.session.doc.variables();
        let v = table
            .iter()
            .find(|v| v.kind == VarKind::Boolean)
            .expect("a boolean variable")
            .clone();
        assert_eq!(v.name, "boolean 1");
        let mode = table.modes().next().unwrap().id;
        assert_eq!(v.cells[&mode], Cell::Boolean(true), "true to start");
        app.var_act(Act::Cell(
            v.id,
            mode,
            super::super::binding::bool_word(false).into(),
            None,
        ));
        let cell =
            |app: &OndinApp| app.session.doc.variables().get(v.id).unwrap().cells[&mode].clone();
        assert_eq!(cell(&app), Cell::Boolean(false), "the box's word lands");
        app.var_act(Act::Cell(v.id, mode, "yes".into(), None));
        assert_eq!(cell(&app), Cell::Boolean(true));
        assert_eq!(
            parse_cell(app.session.doc.variables(), &v, mode, "7"),
            Err("True or false, or --name.".to_string())
        );
    }

    /// A number cell stops at the fields' limit, as the fields it binds do.
    /// flip: `parse_cell`'s number arm without its clamp → fails at "capped".
    #[test]
    fn a_number_cell_stops_at_the_field_limit() {
        let (_ctx, mut app) = app();
        app.var_act(Act::NewVar(VarKind::Number));
        let table = app.session.doc.variables();
        let v = table
            .iter()
            .find(|v| v.kind == VarKind::Number)
            .expect("a number variable")
            .clone();
        let mode = table.modes().next().unwrap().id;
        assert_eq!(
            parse_cell(table, &v, mode, "1e50"),
            Ok(Cell::Number(crate::ui::FIELD_LIMIT)),
            "capped"
        );
        assert_eq!(
            parse_cell(table, &v, mode, "-12.5"),
            Ok(Cell::Number(-12.5))
        );
    }

    /// `Escape` closes one thing at a time: a field being typed into, then the
    /// browser.
    #[test]
    fn escape_closes_the_field_then_the_browser() {
        let (ctx, mut app) = app();
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Number));
        assert!(app.var_browser.as_ref().unwrap().edit.is_some());
        app.escape(&ctx);
        assert!(
            app.var_browser.as_ref().is_some_and(|b| b.edit.is_none()),
            "the field first"
        );
        app.escape(&ctx);
        assert!(app.var_browser.is_none(), "then the browser");
    }

    /// **A frame inherits from the nearest ancestor frame that sets a mode, else
    /// the default** (D1038 (a)) — what the *Mode* row names, *from Outer* or
    /// *default*. The row and its menu are not drawn here; this pins what they
    /// read.
    #[test]
    fn a_frame_inherits_from_the_nearest_frame_that_sets_a_mode() {
        let (_ctx, mut app) = app();
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Color));
        app.var_act(Act::NewMode);
        let modes: Vec<ModeId> = app.session.doc.variables().modes().map(|m| m.id).collect();
        let (outer, inner) = (app.session.ids.mint(), app.session.ids.mint());
        let root = app.session.doc.first_page();
        let frame = |id, parent| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: ondin_core::NodeKind::Artboard {
                size: Size::new(50.0, 50.0),
                corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        };
        assert!(app.commit_edit(Transaction(vec![frame(outer, root), frame(inner, outer)])));
        let doc = &app.session.doc;
        let first = ondin_core::variable::CollectionId::FIRST;
        assert_eq!(
            inherited_mode(doc, inner, first),
            Some((modes[0], None)),
            "the default"
        );
        assert!(app.commit_edit(Transaction(vec![Operation::SetModes {
            id: outer,
            modes: vec![modes[1]],
        }])));
        let doc = &app.session.doc;
        assert_eq!(
            inherited_mode(doc, inner, first),
            Some((modes[1], Some(outer)))
        );
        assert_eq!(doc.mode_at(inner, first), Some(modes[1]));
    }

    /// The panel is 340 wide with one mode and 436 with two, and draws
    /// without a panic over a table, a group and an alias.
    #[test]
    fn the_panel_is_as_wide_as_its_modes() {
        let (ctx, mut app) = app();
        app.toggle_var_browser();
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        // The panel's box, without its hairline: the frame strokes a point
        // outside it on each side.
        let w = |app: &OndinApp| app.var_browser.as_ref().unwrap().panel.unwrap().width() - 2.0;
        assert_eq!(w(&app), 340.0, "empty");
        app.var_act(Act::NewVar(VarKind::Color));
        app.var_act(Act::Name(the_only_var(&app).id, "brand/500".into()));
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        assert_eq!(w(&app), 340.0, "one mode");
        app.var_act(Act::NewMode);
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        assert_eq!(w(&app), 436.0, "two modes");
    }

    /// **A refused rename keeps its text, dim and dotted, until it is changed**
    /// (§15 D1038, D1053): *colour 2* renamed *colour 1* is
    /// refused as taken, the sentence says so, and the field still holding
    /// *colour 1* lays it out at 40% over dots; typed into, it is plain again.
    ///
    /// Flip, run: `var_refuse` without `refused` set fails *"the refused text is
    /// dim"*.
    #[test]
    fn a_refused_rename_keeps_its_text_dim_until_it_changes() {
        let (ctx, mut app) = app();
        crate::theme::install(&ctx);
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Color));
        app.var_act(Act::NewVar(VarKind::Color));
        let b = app.session.doc.variables().by_name("colour 2").unwrap().id;
        app.var_act(Act::StartEdit(Edit::Name {
            var: b,
            text: "colour 1".into(),
        }));
        app.var_act(Act::Name(b, "colour 1".into()));
        assert!(
            app.var_browser.as_ref().unwrap().refusal.is_some(),
            "refused"
        );
        let ink = |app: &mut OndinApp| {
            let mut shapes = vec![];
            for _ in 0..2 {
                shapes = pass(&ctx, app, vec![], |app, ctx| app.var_browser_ui(ctx));
            }
            fn find(shape: &egui::Shape, out: &mut Vec<egui::Color32>) {
                match shape {
                    egui::Shape::Text(t) if t.galley.text() == "colour 1" => {
                        out.extend(t.galley.job.sections.iter().map(|s| s.format.color))
                    }
                    egui::Shape::Vec(v) => v.iter().for_each(|x| find(x, out)),
                    _ => {}
                }
            }
            let mut out = vec![];
            shapes.iter().for_each(|c| find(&c.shape, &mut out));
            out
        };
        assert!(
            ink(&mut app).contains(&color::text_a(102)),
            "the refused text is dim"
        );
        if let Some(Edit::Name { text, .. }) =
            app.var_browser.as_mut().and_then(|b| b.edit.as_mut())
        {
            text.push('0');
        }
        let shapes = pass(&ctx, &mut app, vec![], |app, ctx| app.var_browser_ui(ctx));
        assert!(
            last_text(&shapes, "colour 10").is_some(),
            "the changed text is drawn"
        );
        let dim = shapes.iter().any(|c| {
            matches!(&c.shape, egui::Shape::Text(t) if t.galley.text() == "colour 10"
                && t.galley.job.sections.iter().any(|s| s.format.color == color::text_a(102)))
        });
        assert!(!dim, "changed, it is plain again");
    }

    /// **`Alt`-click on a caret closes or opens every group, and the collapsed
    /// groups outlive the browser** (D1038 G-series, §15 D1054): two groups both
    /// close on one `Alt`-click and both open on the next; closed, they stay closed
    /// through closing and reopening the browser.
    ///
    /// flip: without the write-through to `var_collapsed` → fails at "kept for
    /// the document".
    #[test]
    fn alt_toggles_every_group_and_the_document_keeps_them() {
        let (ctx, mut app) = app();
        crate::theme::install(&ctx);
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Color));
        app.var_act(Act::NewVar(VarKind::Color));
        let (a, b) = {
            let t = app.session.doc.variables();
            (
                t.by_name("colour 1").unwrap().id,
                t.by_name("colour 2").unwrap().id,
            )
        };
        app.var_act(Act::Name(a, "brand/500".into()));
        app.var_act(Act::Name(b, "surface/base".into()));
        let collapsed = |app: &OndinApp| app.var_browser.as_ref().unwrap().collapsed.clone();
        let key = |path: &str| group_key(GroupOf::Own(CollectionId::FIRST), path);
        let both = |app: &OndinApp| {
            collapsed(app).contains(&key("brand")) && collapsed(app).contains(&key("surface"))
        };
        app.var_act(Act::ToggleAll(key("brand")));
        assert!(both(&app), "both closed: {:?}", collapsed(&app));
        app.var_act(Act::ToggleAll(key("brand")));
        assert!(collapsed(&app).is_empty(), "both open again");
        app.var_act(Act::ToggleAll(key("surface")));
        frame(&mut app, &ctx);
        app.toggle_var_browser();
        app.toggle_var_browser();
        frame(&mut app, &ctx);
        assert!(both(&app), "kept for the document");
    }

    /// **A group folds in its own collection only**, and its rename acts on its
    /// own collection, not the one on show, as a search spanning them needs.
    ///
    /// flip: `group_key` keying `GroupOf::Own` by the path alone → fails at
    /// "apart"; `Act::GroupName` renaming in `shown_collection` → fails at "its own".
    #[test]
    fn a_group_belongs_to_its_collection() {
        let (_ctx, mut app) = app();
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Color));
        let a = the_only_var(&app).id;
        app.var_act(Act::Name(a, "color/a".into()));
        app.var_act(Act::NewCollection);
        app.var_act(Act::NewVar(VarKind::Color));
        let second = app.shown_collection();
        assert_ne!(
            second,
            CollectionId::FIRST,
            "the fixture: a second collection on show"
        );
        let b = app
            .session
            .doc
            .variables()
            .of_collection(second)
            .next()
            .unwrap()
            .id;
        app.var_act(Act::Name(b, "color/b".into()));
        assert_eq!(
            app.session.doc.variables().collection_of(b).map(|c| c.id),
            Some(second),
            "the fixture: one `color` group in each"
        );
        app.var_act(Act::Toggle(group_key(
            GroupOf::Own(CollectionId::FIRST),
            "color",
        )));
        let doc = &app.session.doc;
        let br = app.var_browser.as_ref().unwrap();
        let lines = all_lines(doc, second, true, "", None, &br.collapsed);
        let rows: Vec<String> = lines
            .iter()
            .filter_map(|l| match l {
                Line::Var(v) => Some(v.name.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            rows,
            ["color/b"],
            "apart: only the first collection's folds"
        );
        app.var_act(Act::GroupName(
            CollectionId::FIRST,
            "color".into(),
            "tint".into(),
        ));
        let t = app.session.doc.variables();
        assert_eq!(
            (
                t.get(a).unwrap().name.as_str(),
                t.get(b).unwrap().name.as_str()
            ),
            ("tint/a", "color/b"),
            "its own: the rename stays in the first collection"
        );
    }

    /// **Past three modes a bar under the columns scrolls them sideways** (§15
    /// D1055): with four modes, a drag on its thumb moves `scroll_x` by the share
    /// of the hidden column the drag covers, and a long drag stops at the last
    /// column.
    ///
    /// Flip, run: the `Some((x0, s0))` arm's write removed fails *"the drag
    /// scrolls"*.
    #[test]
    fn past_three_modes_the_bar_scrolls_the_columns() {
        let (ctx, mut app) = app();
        crate::theme::install(&ctx);
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Color));
        for _ in 0..3 {
            app.var_act(Act::NewMode);
        }
        if let Some(b) = app.var_browser.as_mut() {
            b.edit = None;
        }
        assert_eq!(
            app.session.doc.variables().modes().count(),
            4,
            "the fixture"
        );
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        let panel = app.var_browser.as_ref().unwrap().panel.expect("drawn");
        // The thumb's middle at rest: the track's left plus half its width.
        let track_l = panel.left() + PAD + NAME_W;
        let track_w = panel.right() - 12.0 - track_l;
        let thumb_w = track_w * 3.0 / 4.0;
        let at = egui::pos2(track_l + thumb_w / 2.0, panel.bottom() - 8.0);
        let ev = |pos, pressed: Option<bool>| match pressed {
            Some(pressed) => egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            },
            None => egui::Event::PointerMoved(pos),
        };
        let run = |app: &mut OndinApp, events: Vec<egui::Event>| {
            pass(&ctx, app, events, |app, ctx| app.var_browser_ui(ctx));
        };
        run(&mut app, vec![ev(at, None)]);
        run(&mut app, vec![ev(at, Some(true))]);
        let to = at + egui::vec2(10.0, 0.0);
        run(&mut app, vec![ev(to, None)]);
        let s = app.var_browser.as_ref().unwrap().scroll_x;
        let room = track_w - thumb_w;
        assert!(
            (s - 10.0 * MODE_W / room).abs() < 0.5,
            "the drag scrolls: {s}"
        );
        run(&mut app, vec![ev(to + egui::vec2(500.0, 0.0), None)]);
        assert_eq!(
            app.var_browser.as_ref().unwrap().scroll_x,
            MODE_W,
            "stops at the last column"
        );
        run(&mut app, vec![ev(to, Some(false))]);
        assert!(app.var_browser.as_ref().unwrap().bar_drag.is_none());
    }

    /// Where text `s` was last drawn this frame, by its galley's centre.
    fn last_text(shapes: &[egui::epaint::ClippedShape], s: &str) -> Option<egui::Pos2> {
        fn find(shape: &egui::Shape, s: &str, out: &mut Option<egui::Pos2>) {
            match shape {
                egui::Shape::Text(t) if t.galley.text() == s => {
                    *out = Some(t.pos + t.galley.rect.center().to_vec2());
                }
                egui::Shape::Vec(v) => v.iter().for_each(|x| find(x, s, out)),
                _ => {}
            }
        }
        let mut out = None;
        shapes.iter().for_each(|c| find(&c.shape, s, &mut out));
        out
    }

    /// The left edge of the last text `s` the pass painted.
    fn text_left(shapes: &[egui::epaint::ClippedShape], s: &str) -> Option<f32> {
        fn find(shape: &egui::Shape, s: &str, out: &mut Option<f32>) {
            match shape {
                egui::Shape::Text(t) if t.galley.text() == s => {
                    *out = Some(t.pos.x + t.galley.rect.left());
                }
                egui::Shape::Vec(v) => v.iter().for_each(|x| find(x, s, out)),
                _ => {}
            }
        }
        let mut out = None;
        shapes.iter().for_each(|c| find(&c.shape, s, &mut out));
        out
    }

    /// One pass of `draw` on the 1200×800 screen, its shapes returned.
    fn pass(
        ctx: &egui::Context,
        app: &mut OndinApp,
        events: Vec<egui::Event>,
        draw: fn(&mut OndinApp, &egui::Context),
    ) -> Vec<egui::epaint::ClippedShape> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 800.0),
            )),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |_| draw(app, ctx)).shapes
    }

    /// **`Tab` into a column scrolled out of view scrolls it on show**, so its
    /// field is drawn and focused rather than leaving the keys to the canvas.
    ///
    /// flip: the `Tab` branch's scroll removed → fails *"the fourth column on
    /// show"*, `scroll_x` still 0.
    #[test]
    fn tab_into_a_hidden_column_scrolls_it_on_show() {
        let (ctx, mut app) = app();
        crate::theme::install(&ctx);
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Number));
        for _ in 0..4 {
            app.var_act(Act::NewMode);
        }
        let v = the_only_var(&app);
        let modes: Vec<ModeId> = app.session.doc.variables().modes().map(|m| m.id).collect();
        assert_eq!(modes.len(), 5, "fixture: five modes");
        let b = app.var_browser.as_mut().unwrap();
        b.edit = Some(Edit::Cell {
            var: v.id,
            mode: modes[2],
            text: "7".into(),
        });
        b.edit_focus = true;
        let draw: fn(&mut OndinApp, &egui::Context) = |a, c| a.var_browser_ui(c);
        for _ in 0..3 {
            pass(&ctx, &mut app, vec![], draw);
        }
        let tab = |pressed| egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Default::default(),
        };
        pass(&ctx, &mut app, vec![tab(true), tab(false)], draw);
        for _ in 0..3 {
            pass(&ctx, &mut app, vec![], draw);
        }
        let b = app.var_browser.as_ref().unwrap();
        assert!(
            matches!(&b.edit, Some(Edit::Cell { mode, .. }) if *mode == modes[3]),
            "fixture: the edit moved on, {:?}",
            b.edit.is_some()
        );
        assert_eq!(b.scroll_x, MODE_W, "the fourth column on show");
    }

    /// **Renaming a mode moves nothing below the header** (§15 D1083): the
    /// variable rows sit where they sat. Reported as the table shifting up a
    /// point or two while the mode's field was open.
    ///
    /// Flip, run: `edit_field` drawing in a `scope_builder` again fails, the
    /// name 5 points higher (436 against 441).
    #[test]
    fn renaming_a_mode_leaves_the_rows_where_they_are() {
        let (ctx, mut app) = app();
        crate::theme::install(&ctx);
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Number));
        let v = the_only_var(&app);
        let mode = app.session.doc.variables().modes().next().unwrap().clone();
        app.var_browser.as_mut().unwrap().edit = None;
        let draw: fn(&mut OndinApp, &egui::Context) = |a, c| a.var_browser_ui(c);
        let mut shapes = vec![];
        for _ in 0..3 {
            shapes = pass(&ctx, &mut app, vec![], draw);
        }
        let name = v.name.rsplit('/').next().unwrap().to_string();
        let resting = last_text(&shapes, &name).expect("the variable's name");
        let b = app.var_browser.as_mut().unwrap();
        b.edit = Some(Edit::Mode {
            mode: mode.id,
            text: mode.name.clone(),
        });
        b.edit_focus = true;
        for _ in 0..3 {
            shapes = pass(&ctx, &mut app, vec![], draw);
        }
        let editing = last_text(&shapes, &name).expect("the variable's name, the mode open");
        assert_eq!(editing, resting, "the row moved while the mode was renamed");
    }

    /// Click the text `s` that `draw` lays out — moved to, pressed, released.
    fn click_text(
        ctx: &egui::Context,
        app: &mut OndinApp,
        s: &str,
        draw: fn(&mut OndinApp, &egui::Context),
    ) {
        let mut shapes = vec![];
        for _ in 0..2 {
            shapes = pass(ctx, app, vec![], draw);
        }
        let at = last_text(&shapes, s).unwrap_or_else(|| panic!("{s:?} is drawn"));
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        pass(ctx, app, vec![egui::Event::PointerMoved(at)], draw);
        pass(ctx, app, vec![button(true)], draw);
        pass(ctx, app, vec![button(false)], draw);
    }

    /// Focus the topmost clickable widget under the text `s` the browser draws,
    /// then press `F2` on it; what the browser is editing afterwards.
    fn f2_on(ctx: &egui::Context, app: &mut OndinApp, s: &str) -> Option<Edit> {
        let draw: fn(&mut OndinApp, &egui::Context) = |a, c| a.var_browser_ui(c);
        app.var_browser.as_mut().unwrap().edit = None;
        let mut shapes = vec![];
        for _ in 0..3 {
            shapes = pass(ctx, app, vec![], draw);
        }
        let at = last_text(&shapes, s).unwrap_or_else(|| panic!("{s:?} is drawn"));
        let hit = ctx.viewport(|v| {
            v.this_pass
                .widgets
                .layers()
                .flat_map(|(_, ws)| ws.iter())
                .filter(|w| w.rect.contains(at) && w.sense.senses_click())
                .last()
                .map(|w| w.id)
        });
        ctx.memory_mut(|m| m.request_focus(hit.expect("a clickable widget")));
        pass(ctx, app, vec![], draw);
        let f2 = egui::Event::Key {
            key: egui::Key::F2,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        pass(ctx, app, vec![f2], draw);
        app.var_browser.as_ref().unwrap().edit.clone()
    }

    /// `F2` on a focused variable name, group row, mode header (*Value* while it
    /// is the only one) or colour cell opens it for typing, as a double-click does.
    /// flip: `var_row` dropping the name's `rename_key` → fails at "the name".
    #[test]
    fn f2_on_a_focused_browser_row_opens_it_for_typing() {
        let (ctx, mut app) = app();
        crate::theme::install(&ctx);
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Color));
        let v = the_only_var(&app);
        assert!(
            app.commit_edit(Transaction(vec![Operation::SetVariableName {
                id: v.id,
                name: "brand/ink".into(),
            }]))
        );
        let hex = ui::hex_of(Color::WHITE);
        assert!(
            matches!(f2_on(&ctx, &mut app, "ink"), Some(Edit::Name { var, .. }) if var == v.id),
            "the name"
        );
        assert!(
            matches!(f2_on(&ctx, &mut app, "brand"), Some(Edit::Group { path, .. }) if path == "brand"),
            "the group row"
        );
        assert!(
            matches!(f2_on(&ctx, &mut app, "Value"), Some(Edit::Mode { .. })),
            "the mode header"
        );
        assert!(
            matches!(f2_on(&ctx, &mut app, &hex), Some(Edit::Cell { var, .. }) if var == v.id),
            "the colour cell"
        );
    }

    /// **A colour cell opens the colour popup, whose *Variables* tab makes it an
    /// alias and whose *Solid* writes the colour back** (D1038 G-series, §15
    /// D1052), each through the real clicks: the browser's cell, then the
    /// popup's rows and tabs. *colour 1* itself is refused (*this variable*), and
    /// from *colour 2*'s cell once *colour 1* uses it, *colour 1* is refused too
    /// (*uses colour 2*, the loop it would close).
    ///
    /// ⚠️ **Two defences against the loop**: the row's refusal, and the commit's
    /// own table check. Flip, run: dropping the face's `refusal.is_none()` gate
    /// left *"a loop is not written"* green — the commit refused it — and fails
    /// *"an inert row, not a refused commit"*, the status reading *Edit failed: …
    /// aliases make a loop*, which is what the refusal spares the user.
    #[test]
    fn a_colour_cell_opens_the_popup_and_its_variables_tab_makes_an_alias() {
        let (ctx, mut app) = app();
        crate::theme::install(&ctx);
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Color));
        app.var_act(Act::NewVar(VarKind::Color));
        let table = app.session.doc.variables();
        let mode = table.default_mode().unwrap();
        let a = table.by_name("colour 1").unwrap().id;
        let b = table.by_name("colour 2").unwrap().id;
        app.var_act(Act::Cell(a, mode, "#8BA5E0".into(), None));
        app.var_act(Act::Cell(b, mode, "#102030".into(), None));
        if let Some(br) = app.var_browser.as_mut() {
            br.edit = None;
        }
        let browser: fn(&mut OndinApp, &egui::Context) = |app, ctx| {
            app.var_browser_ui(ctx);
            app.picker_ui(ctx);
        };
        let popup: fn(&mut OndinApp, &egui::Context) = |app, ctx| app.picker_ui(ctx);
        click_text(&ctx, &mut app, "8BA5E0", browser);
        assert_eq!(
            app.picker.as_ref().map(|p| p.slot),
            Some(PaintSlot::VarCell(a, mode)),
            "the cell opened the popup on itself"
        );
        assert!(
            app.var_browser.as_ref().unwrap().edit.is_none(),
            "not the text"
        );

        click_text(&ctx, &mut app, "Variables", popup);
        let shapes = pass(&ctx, &mut app, vec![], popup);
        assert!(
            last_text(&shapes, "this variable").is_some(),
            "itself is refused"
        );
        click_text(&ctx, &mut app, "colour 2", popup);
        let cell =
            |app: &OndinApp, v| app.session.doc.variables().get(v).unwrap().cells[&mode].clone();
        assert_eq!(cell(&app, a), Cell::Alias(b), "a row makes the alias");

        app.var_act(Act::OpenPicker(b, mode, egui::pos2(500.0, 200.0)));
        click_text(&ctx, &mut app, "Variables", popup);
        let shapes = pass(&ctx, &mut app, vec![], popup);
        assert!(
            last_text(&shapes, "uses colour 2").is_some(),
            "colour 1 would close a loop"
        );
        let before = app.session.doc.clone();
        click_text(&ctx, &mut app, "colour 1", popup);
        assert_eq!(app.session.doc, before, "a loop is not written");
        // The commit would refuse it anyway — what the row's refusal adds is that
        // the click never reaches it, so the user is not told an edit *failed*.
        assert!(
            !app.session.status().text.starts_with("Edit failed"),
            "an inert row, not a refused commit: {:?}",
            app.session.status().text
        );

        app.var_act(Act::OpenPicker(a, mode, egui::pos2(500.0, 200.0)));
        click_text(&ctx, &mut app, "Solid", popup);
        assert_eq!(
            cell(&app, a),
            Cell::Color(Color::from_rgba8(0x10, 0x20, 0x30, 0xff)),
            "Solid writes the colour it drew, ending the alias"
        );
    }

    const KIT: &str = "5eed0000000000000000000000000021";

    /// A document with a mode *Light* and a variable of its own, *surface*, and
    /// a kit *Atlas*'s variable *brand* — white by *Day*, navy by *Night* — and a
    /// kit fill style *Accent* on a rectangle (§15 D1064). Answers the ids.
    fn with_a_kit(app: &mut OndinApp) -> (ModeId, ModeId, VarId, VarId, StyleId, NodeId) {
        let ids = &mut app.session.ids;
        let (light, day, night) = (ids.mint_mode(), ids.mint_mode(), ids.mint_mode());
        let (surface, brand) = (ids.mint_var(), ids.mint_var());
        let (accent, rect) = (ids.mint_style(), ids.mint());
        let navy = Color::from_rgba8(10, 20, 80, 255);
        let gold = keyed_by_position([Fill {
            brush: Color::from_rgba8(220, 180, 40, 255).into(),
            visible: true,
        }]);
        let root = app.session.doc.first_page();
        let tx = Transaction(vec![
            Operation::AddMode {
                collection: ondin_core::variable::CollectionId::FIRST,
                mode: Mode {
                    id: light,
                    name: "Light".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: surface,
                    name: "surface".into(),
                    kind: VarKind::Color,
                    cells: [(light, Cell::Color(Color::WHITE))].into(),
                    scope: None,
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::SetKitModes {
                kit: KIT.into(),
                modes: Some(ondin_core::variable::KitModes {
                    name: Some("Atlas".into()),
                    modes: vec![
                        ondin_core::variable::KitMode {
                            id: day,
                            name: "Day".into(),
                            from: day,
                            collection: String::new(),
                        },
                        ondin_core::variable::KitMode {
                            id: night,
                            name: "Night".into(),
                            from: night,
                            collection: String::new(),
                        },
                    ],
                    map: Default::default(),
                }),
            },
            Operation::AddVariable {
                var: Variable {
                    id: brand,
                    name: "brand".into(),
                    kind: VarKind::Color,
                    cells: [(day, Cell::Color(Color::WHITE)), (night, Cell::Color(navy))].into(),
                    scope: None,
                    origin: Some(ondin_core::variable::VarOrigin {
                        kit: KIT.into(),
                        var: brand,
                        version: 1,
                    }),
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::AddStyle {
                style: Style {
                    id: accent,
                    name: "Accent".into(),
                    value: sty::StyleValue::Fills(gold.clone()),
                    origin: Some(sty::StyleOrigin {
                        kit: KIT.into(),
                        style: accent,
                        version: 1,
                    }),
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::CreateNode {
                id: rect,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id: rect,
                fills: gold,
            },
            Operation::SetFillStyle {
                id: rect,
                style: Some(accent),
            },
        ]);
        assert!(app.session.commit(tx), "the fixture commits");
        (light, night, surface, brand, accent, rect)
    }

    /// **The browser's group, mode and selection doors** (D1038 G): a group
    /// renamed moves its members, refusing an empty part and a name taken; a
    /// mode renamed, refusing an empty name and a taken one; *Duplicate mode*
    /// names its copy after its source, then *copy 2*, each placed after it;
    /// *Delete mode* returns a frame set to it to *Inherit*; the count's
    /// *Select* selects its layers.
    ///
    /// flip: `GroupName`'s empty-part refusal removed → fails *"an empty part
    /// is refused"*, the rename committed to `a//b`'s parts.
    #[test]
    fn the_browsers_group_mode_and_select_doors() {
        let (_ctx, mut app) = app();
        let (light, _, _, _, _, rect) = with_a_kit(&mut app);
        app.toggle_var_browser();
        let number = |id, name: &str| Operation::AddVariable {
            var: Variable {
                id,
                name: name.into(),
                kind: VarKind::Number,
                cells: [(light, Cell::Number(1.0))].into(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        };
        let ids: Vec<VarId> = (0..3).map(|_| app.session.ids.mint_var()).collect();
        assert!(app.session.commit(Transaction(vec![
            number(ids[0], "ui/a"),
            number(ids[1], "ui/b"),
            number(ids[2], "other/a"),
        ])));
        let name =
            |app: &OndinApp, v: VarId| app.session.doc.variables().get(v).unwrap().name.clone();
        let refusal = |app: &OndinApp| app.var_browser.as_ref().unwrap().refusal.clone();

        let first = CollectionId::FIRST;
        app.var_act(Act::GroupName(first, "ui".into(), "theme".into()));
        assert_eq!(
            (name(&app, ids[0]), name(&app, ids[1])),
            ("theme/a".into(), "theme/b".into())
        );
        app.var_act(Act::GroupName(first, "theme".into(), "a//b".into()));
        assert!(refusal(&app).is_some(), "an empty part is refused");
        assert_eq!(name(&app, ids[0]), "theme/a");
        app.var_act(Act::GroupName(first, "theme".into(), "other".into()));
        assert_eq!(
            refusal(&app).as_deref(),
            Some("other/a already exists."),
            "a name taken is refused"
        );

        app.var_act(Act::ModeName(light, "  ".into()));
        assert_eq!(refusal(&app).as_deref(), Some("A mode needs a name."));
        app.var_act(Act::ModeName(light, "Day".into()));
        let modes = |app: &OndinApp| -> Vec<String> {
            app.session
                .doc
                .variables()
                .modes()
                .map(|m| m.name.clone())
                .collect()
        };
        assert_eq!(modes(&app), ["Day"]);
        app.var_act(Act::DuplicateMode(light));
        app.var_act(Act::DuplicateMode(light));
        assert_eq!(
            modes(&app),
            ["Day", "Day copy 2", "Day copy"],
            "each after its source"
        );
        app.var_act(Act::ModeName(light, "Day copy".into()));
        assert_eq!(refusal(&app).as_deref(), Some("Day copy already exists."));
        let last = app.session.doc.variables().modes().nth(2).unwrap().id;
        app.var_act(Act::MoveMode(last, 0));
        assert_eq!(
            modes(&app),
            ["Day copy", "Day", "Day copy 2"],
            "made default"
        );
        app.var_act(Act::MoveMode(last, 2));
        assert_eq!(modes(&app), ["Day", "Day copy 2", "Day copy"], "moved back");

        let copy = app.session.doc.variables().modes().nth(2).unwrap().id;
        let frame = app.session.ids.mint();
        let root = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(50.0, 50.0),
                    corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::SetModes {
                id: frame,
                modes: vec![copy],
            },
        ])));
        app.var_act(Act::DeleteMode(copy));
        assert_eq!(modes(&app), ["Day", "Day copy 2"]);
        assert!(
            app.session.doc.get(frame).unwrap().modes().is_empty(),
            "back to Inherit"
        );

        app.session.selection.set(Vec::new());
        app.var_act(Act::Select(vec![rect]));
        assert_eq!(app.session.selection.ids(), &[rect]);
    }

    /// **A document group's menu reaches the document's own variables only**:
    /// a kit's group of the same path is inert, so *Delete group* and *Select*
    /// leave it and its readers alone (§15 D1065).
    ///
    /// flip: `group_members` walking `table.iter()` → fails *"the kit's
    /// brand/primary is not a member"*.
    #[test]
    fn a_document_groups_members_are_the_documents_own() {
        let (_ctx, mut app) = app();
        let (light, _, _, brand, _, _) = with_a_kit(&mut app);
        let own = app.session.ids.mint_var();
        let kit_brand = app.session.doc.variables().get(brand).unwrap().clone();
        let tx = Transaction(vec![
            Operation::SetVariable {
                var: Variable {
                    name: "brand/primary".into(),
                    ..kit_brand
                },
            },
            Operation::AddVariable {
                var: Variable {
                    id: own,
                    name: "brand/accent".into(),
                    kind: VarKind::Color,
                    cells: [(light, Cell::Color(Color::WHITE))].into(),
                    scope: None,
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ]);
        assert!(app.session.commit(tx));
        let table = app.session.doc.variables();
        let members: Vec<VarId> = group_members(table, table.first_collection().id, "brand")
            .iter()
            .map(|v| v.id)
            .collect();
        assert_eq!(
            members,
            vec![own],
            "the kit's brand/primary is not a member"
        );
    }

    /// **Re-typing an alias of a kit's variable keeps it**, though the document
    /// has its own variable of that name: `--brand` reads unqualified, and the
    /// cell's own target comes first.
    ///
    /// flip: `parse_cell` resolving `--name` by `by_name` alone → fails *"the
    /// alias still names the kit's brand"*.
    #[test]
    fn re_typing_a_kit_alias_keeps_its_target() {
        let (_ctx, mut app) = app();
        let (light, _, surface, brand, _, _) = with_a_kit(&mut app);
        let own = app.session.ids.mint_var();
        let tx = Transaction(vec![
            Operation::AddVariable {
                var: Variable {
                    id: own,
                    name: "brand".into(),
                    kind: VarKind::Color,
                    cells: [(light, Cell::Color(Color::BLACK))].into(),
                    scope: None,
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::SetVariableCell {
                id: surface,
                mode: light,
                cell: Cell::Alias(brand),
            },
        ]);
        assert!(app.session.commit(tx), "the fixture commits");
        let table = app.session.doc.variables();
        let v = table.get(surface).unwrap();
        let text = cell_text(table, &v.cells[&light]);
        assert_eq!(text, "--brand");
        assert_eq!(
            parse_cell(table, v, light, &text),
            Ok(Cell::Alias(brand)),
            "the alias still names the kit's brand"
        );
    }

    /// **A kit's variables list last, under the kit, inert — and the kit's header
    /// carries the mapping** (D1038 (b), G; §15 D1065): *brand* sits after the
    /// document's own *surface*, below an *Atlas* header whose *Light* column
    /// reads *Day →* — no kit mode is named *Light*, so the kit's first — and a
    /// pick from that column's menu sets the entry by hand, the column then
    /// reading *Night →*. *Duplicate* of a kit's variable makes nothing, and the
    /// `--` list puts the kit's after the document's own.
    ///
    /// Flip, run: `all_lines` listing `table.iter()` whole, with no kit header,
    /// fails at *"the kit's header, after the document's own"* — the kit's
    /// *brand* first by name, and editable as the document's own.
    #[test]
    fn a_kits_variables_list_last_under_the_kit_with_its_mapping() {
        let (ctx, mut app) = app();
        crate::theme::install(&ctx);
        let (light, night, surface, brand, accent, rect) = with_a_kit(&mut app);
        app.toggle_var_browser();

        let doc = &app.session.doc;
        let first = doc.variables().first_collection().id;
        let order: Vec<String> = all_lines(doc, first, false, "", None, &HashSet::new())
            .iter()
            .map(|l| match l {
                Line::Var(v) => v.name.clone(),
                Line::Kit(k) => format!("kit {k}"),
                Line::Group { path, .. } => format!("group {path}"),
                Line::Collection(c) => format!("collection {}", c.name),
            })
            .collect();
        assert_eq!(
            order,
            ["surface".to_string(), format!("kit {KIT}"), "brand".into()],
            "the kit's header, after the document's own"
        );
        let listed: Vec<VarId> =
            super::super::binding::matches(doc.variables(), VarKind::Color, "")
                .iter()
                .map(|v| v.id)
                .collect();
        assert_eq!(listed, [surface, brand], "the `--` list's kit groups last");

        // Two passes: an `Area` draws nothing on the frame it first sizes itself.
        pass(&ctx, &mut app, vec![], |a, c| a.var_browser_ui(c));
        let shapes = pass(&ctx, &mut app, vec![], |a, c| a.var_browser_ui(c));
        assert!(
            last_text(&shapes, "Atlas").is_some(),
            "the kit's name heads it"
        );
        click_text(&ctx, &mut app, "Day →", |a, c| a.var_browser_ui(c));
        let b = app.var_browser.as_ref().unwrap();
        assert!(
            matches!(&b.menu, Some((Menu::KitMap(k, m), _)) if k == KIT && *m == light),
            "the column's menu"
        );
        app.var_act(Act::KitMap(KIT.into(), light, Some(night)));
        assert_eq!(
            app.session.doc.variables().kits()[KIT].map.get(&light),
            Some(&night)
        );
        let shapes = pass(&ctx, &mut app, vec![], |a, c| a.var_browser_ui(c));
        assert!(last_text(&shapes, "Night →").is_some(), "set by hand");

        let before = app.session.doc.variables().iter().count();
        app.var_act(Act::Duplicate(brand));
        assert_eq!(app.session.doc.variables().iter().count(), before, "inert");

        // And a kit's style is not edited here: *Edit* opens on nothing.
        app.edit_style(accent);
        assert_eq!(app.session.style_edit, None);
        assert_eq!(
            sty::users(&app.session.doc, accent),
            [rect],
            "though it has a user"
        );
    }

    /// The key a document's collapsed groups are kept under survives a page
    /// reorder, which changes the first page.
    ///
    /// flip: `collapsed_key` answering `first_page` → fails at the reorder.
    #[test]
    fn the_collapse_key_survives_a_page_reorder() {
        let mut ids = ondin_core::IdSource::new(0xC011);
        let first = ids.mint();
        let mut doc = ondin_core::Document::new(first);
        let second = ids.mint();
        doc.apply(&Transaction(vec![Operation::AddPage {
            id: second,
            index: 1,
            name: "Two".into(),
            ground: None,
            draft: false,
            modes: Vec::new(),
        }]))
        .unwrap();
        let before = collapsed_key(&doc);
        doc.apply(&Transaction(vec![Operation::MovePage {
            id: second,
            index: 0,
        }]))
        .unwrap();
        assert_eq!(doc.first_page(), second, "fixture: the first page changed");
        assert_eq!(collapsed_key(&doc), before);
    }

    /// **New collection shows it with its name to type, and new variables and
    /// modes land in the collection on show**; a rename to a taken name is
    /// refused; picking another shows it; a search spans both, each under its
    /// header.
    ///
    /// flip: `Act::NewVar` filing into the first collection → fails at "into
    /// the one on show".
    #[test]
    fn a_new_collection_is_shown_and_takes_what_is_made() {
        let (ctx, mut app) = app();
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Color));
        app.var_act(Act::NewCollection);
        let t = app.session.doc.variables();
        assert_eq!(t.collections().len(), 2);
        let second = t.collections()[1].id;
        assert_eq!(app.shown_collection(), second);
        assert!(matches!(
            &app.var_browser.as_ref().unwrap().edit,
            Some(Edit::Collection { id, .. }) if *id == second
        ));
        app.var_act(Act::CollectionName(second, "Variables".into()));
        assert!(
            app.var_browser.as_ref().unwrap().refusal.is_some(),
            "a taken name is refused"
        );
        app.var_act(Act::CollectionName(second, "Spacing".into()));
        app.var_act(Act::NewVar(VarKind::Number));
        app.var_act(Act::NewMode);
        let t = app.session.doc.variables();
        let num = t.iter().find(|v| v.kind == VarKind::Number).unwrap();
        assert_eq!(
            t.collection_of(num.id).map(|c| c.id),
            Some(second),
            "into the one on show"
        );
        assert_eq!(t.collection(second).unwrap().modes.len(), 2);
        assert_eq!(t.first_collection().modes.len(), 1, "the other untouched");
        assert_eq!(t.collection(second).unwrap().name, "Spacing");
        frame(&mut app, &ctx);
        app.var_act(Act::ShowCollection(CollectionId::FIRST));
        assert_eq!(app.shown_collection(), CollectionId::FIRST);
        let doc = &app.session.doc;
        let lines = all_lines(doc, CollectionId::FIRST, true, "", None, &HashSet::new());
        let heads = lines
            .iter()
            .filter(|l| matches!(l, Line::Collection(_)))
            .count();
        assert_eq!(heads, 2, "spanning, each collection heads its rows");
        frame(&mut app, &ctx);
    }

    /// **Move to keeps the binding; narrowing a scope detaches what it drops**,
    /// one undo bringing it back; deleting a collection takes its variables.
    #[test]
    fn move_to_scope_and_delete_collection() {
        let (_ctx, mut app) = app();
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Number));
        let v = the_only_var(&app).id;
        let rect = app.session.ids.mint();
        let root = app.session.doc.first_page();
        assert!(app.commit_edit(Transaction(vec![
            Operation::CreateNode {
                id: rect,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::SetCornerVars {
                id: rect,
                vars: [Some(v); 4],
            },
        ])));
        app.var_act(Act::MoveTo(v, None));
        let t = app.session.doc.variables();
        let to = t.collection_of(v).map(|c| c.id).unwrap();
        assert_ne!(to, CollectionId::FIRST, "moved into a new collection");
        assert_eq!(
            app.session.doc.get(rect).unwrap().corner_vars(),
            [Some(v); 4]
        );
        app.var_act(Act::Scope(v, Some(Scope::of([Field::Spacing]))));
        assert_eq!(app.session.doc.get(rect).unwrap().corner_vars(), [None; 4]);
        app.undo();
        assert_eq!(
            app.session.doc.get(rect).unwrap().corner_vars(),
            [Some(v); 4]
        );
        app.var_act(Act::DeleteCollection(to));
        let t = app.session.doc.variables();
        assert!(t.collection(to).is_none());
        assert!(t.get(v).is_none());
        assert_eq!(app.session.doc.get(rect).unwrap().corner_vars(), [None; 4]);
    }

    /// **A variable's *Move to ▸* opens its list beside the row on hover**, with
    /// the menu still up, and another row hovered closes the list.
    ///
    /// flip: `move_row` without `move_to.hovered()` → fails at "opens on hover".
    #[test]
    fn move_to_is_a_submenu_beside_its_row() {
        let (ctx, mut app) = app();
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Color));
        let v = the_only_var(&app).id;
        app.var_browser.as_mut().unwrap().edit = None;
        app.var_act(Act::OpenMenu(Menu::Var(v), egui::pos2(400.0, 200.0)));
        let mut shapes = Vec::new();
        for _ in 0..2 {
            shapes = pass(&ctx, &mut app, vec![], |app, ctx| app.var_browser_ui(ctx));
        }
        let row = last_text(&shapes, "Move to").expect("the row");
        assert!(
            last_text(&shapes, "New collection").is_none(),
            "shut at first"
        );
        let hover = |at: egui::Pos2| vec![egui::Event::PointerMoved(at)];
        for _ in 0..3 {
            shapes = pass(&ctx, &mut app, hover(row), |app, ctx| {
                app.var_browser_ui(ctx)
            });
        }
        let new = last_text(&shapes, "New collection").expect("opens on hover");
        assert!(
            new.x > row.x + 150.0,
            "beside the row: {new:?} against {row:?}"
        );
        assert!(
            matches!(
                app.var_browser.as_ref().unwrap().menu,
                Some((Menu::Var(_), _))
            ),
            "the menu stays up under it"
        );
        let rename = last_text(&shapes, "Rename").expect("the menu's first row");
        for _ in 0..3 {
            shapes = pass(&ctx, &mut app, hover(rename), |app, ctx| {
                app.var_browser_ui(ctx)
            });
        }
        assert!(
            last_text(&shapes, "New collection").is_none(),
            "closed by another row"
        );
    }

    /// **The scope popover's boxes sit level with their labels**, inset from the
    /// card's edge as a menu row's glyph is.
    ///
    /// flip: `paint_tick_box` handed the box's top-left again → fails at "level".
    #[test]
    fn the_scope_boxes_line_up_with_their_labels() {
        let (ctx, mut app) = app();
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Color));
        let v = the_only_var(&app).id;
        app.var_browser.as_mut().unwrap().edit = None;
        app.var_act(Act::Scope(
            v,
            Some(Scope::of(Field::of_kind(VarKind::Color))),
        ));
        app.var_act(Act::OpenMenu(Menu::Scope(v), egui::pos2(400.0, 200.0)));
        let mut shapes = Vec::new();
        for _ in 0..2 {
            shapes = pass(&ctx, &mut app, vec![], |app, ctx| app.var_browser_ui(ctx));
        }
        let label = last_text(&shapes, "Fill").expect("the fill row");
        fn tick(shape: &egui::Shape, out: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Rect(r) if r.rect.width() == ui::TICK_BOX => out.push(r.rect),
                egui::Shape::Vec(v) => v.iter().for_each(|x| tick(x, out)),
                _ => {}
            }
        }
        let mut boxes = Vec::new();
        shapes.iter().for_each(|s| tick(&s.shape, &mut boxes));
        let row = boxes
            .iter()
            .find(|b| (b.center().y - label.y).abs() < MENU_ROW_H / 2.0)
            .expect("a box in the fill row");
        assert!(
            (row.center().y - label.y).abs() < 1.0,
            "level: {row:?} against {label:?}"
        );
        let card = 400.0 + 4.0 + ui::MENU_BORDER;
        assert!(row.left() >= card + SCOPE_INSET - 0.5, "inset: {row:?}");
    }

    /// **The picker counts a collection's modes and never names them.**
    #[test]
    fn the_picker_counts_modes() {
        let (_ctx, mut app) = app();
        app.var_act(Act::NewVar(VarKind::Color));
        app.var_act(Act::NewMode);
        let t = app.session.doc.variables();
        assert_eq!(t.collections()[0].modes.len(), 2, "the fixture: two modes");
        assert_eq!(modes_summary(&t.collections()[0]), "2 modes");
    }

    /// **A long collection name ends in `…` on the picker's face** instead of
    /// widening it past [`PICKER_W`].
    ///
    /// flip: the picker's `ComboBox` without `.truncate()` → the face takes the
    /// row's room and the search box after it panics on a negative size, before
    /// "as wide" is reached.
    #[test]
    fn a_long_collection_name_is_cut_on_the_picker() {
        let (ctx, mut app) = app();
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Color));
        app.var_act(Act::NewCollection);
        let c = app.session.doc.variables().collections()[1].id;
        let long = "A very long collection name that has to be cut short";
        app.var_act(Act::CollectionName(c, long.into()));
        app.var_browser.as_mut().unwrap().edit = None;
        let mut shapes = Vec::new();
        for _ in 0..3 {
            shapes = pass(&ctx, &mut app, vec![], |app, ctx| app.var_browser_ui(ctx));
        }
        assert_eq!(
            app.session.doc.variables().collection(c).unwrap().name,
            long
        );
        fn face(shape: &egui::Shape, out: &mut Option<f32>) {
            match shape {
                egui::Shape::Text(t) if t.galley.job.text.contains("A very long") => {
                    *out = Some(t.galley.size().x);
                }
                egui::Shape::Vec(v) => v.iter().for_each(|x| face(x, out)),
                _ => {}
            }
        }
        let mut w = None;
        shapes.iter().for_each(|s| face(&s.shape, &mut w));
        let w = w.expect("the face shows the collection");
        assert!(w < PICKER_W, "as wide as the picker at most: {w}");
    }

    /// **A frame's card has a mode row per collection with two modes or more**,
    /// one for a document with one collection, none for a one-mode collection.
    #[test]
    fn mode_rows_are_one_per_choosing_collection() {
        let (_ctx, mut app) = app();
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Color));
        assert_eq!(
            mode_rows(app.session.doc.variables()).len(),
            1,
            "one mode, one collection"
        );
        app.var_act(Act::NewCollection);
        assert!(
            mode_rows(app.session.doc.variables()).is_empty(),
            "two collections of one mode each"
        );
        app.var_act(Act::NewMode);
        let t = app.session.doc.variables();
        assert_eq!(mode_rows(t), vec![t.collections()[1].id]);
    }

    /// **A string scoped to flex direction holds its words and binds a
    /// container's direction**: scoping it turns *Text* into *Row*, a typed
    /// `column` lands as *Column* and *Sideways* is refused naming the words; bound
    /// through the field, the layout draws Column, and a detach keeps it.
    ///
    /// flip: `var_cell` without its `flex_words` arm → fails at "a word typed
    /// lowercase lands as the word".
    #[test]
    fn a_flex_direction_string_through_the_browser_and_the_field() {
        use ondin_core::container::{Display, Flex, FlexDirection};
        let (_ctx, mut app) = app();
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::String));
        let v = the_only_var(&app);
        let mode = *v.cells.keys().next().unwrap();
        app.var_act(Act::Scope(v.id, Some(Scope::of([Field::FlexDirection]))));
        assert_eq!(
            the_only_var(&app).cells[&mode],
            Cell::String("Row".into()),
            "scoping it makes its cells words"
        );
        app.var_act(Act::Cell(v.id, mode, "column".into(), None));
        assert_eq!(
            the_only_var(&app).cells[&mode],
            Cell::String("Column".into()),
            "a word typed lowercase lands as the word"
        );
        app.var_act(Act::StartEdit(Edit::Cell {
            var: v.id,
            mode,
            text: "Sideways".into(),
        }));
        app.var_act(Act::Cell(v.id, mode, "Sideways".into(), None));
        let refusal = app.var_browser.as_ref().unwrap().refusal.clone();
        assert!(
            refusal
                .as_deref()
                .is_some_and(|r| r.contains("Row, Column")),
            "{refusal:?}"
        );

        let id = app.session.ids.mint();
        let root = app.session.doc.first_page();
        assert!(app.commit_edit(Transaction(vec![
            Operation::CreateNode {
                id,
                parent: root,
                index: 0,
                kind: ondin_core::NodeKind::Artboard {
                    size: Size::new(200.0, 100.0),
                    corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::SetDisplay {
                id,
                display: Some(Display::Flex(Flex::default())),
            },
        ])));
        let key = super::super::binding::FieldKey::Flow { id, slot: 0 };
        app.bind_field(key, v.id);
        assert_eq!(app.field_var(key), Some(v.id));
        let drawn = |app: &OndinApp| match app.session.doc.resolved_display(id).as_deref() {
            Some(Display::Flex(f)) => f.direction,
            _ => unreachable!(),
        };
        assert_eq!(drawn(&app), FlexDirection::Column);
        app.detach_field(key);
        assert_eq!(app.field_var(key), None);
        assert_eq!(
            drawn(&app),
            FlexDirection::Column,
            "detached to what it drew"
        );
    }

    /// **A collection picked by width, through the browser and the mode row**:
    /// ticking gives the modes 0, 768 and 1200, the headers draw them, a width
    /// at a neighbour's is refused naming it, a top-level frame's *Inherit* is
    /// *By width* naming its pick, a nested frame inherits it from the top-level
    /// one, and unticking leaves the frame set to the mode it drew.
    ///
    /// flip: `inherited_mode` without `width_pick` in its walk → fails at "from
    /// the top-level frame".
    #[test]
    fn a_collection_picked_by_width_through_the_browser_and_the_mode_row() {
        let (ctx, mut app) = app();
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Number));
        app.var_act(Act::NewMode);
        app.var_act(Act::NewMode);
        let c = CollectionId::FIRST;
        app.var_act(Act::ByWidth(c, true));
        let modes: Vec<ModeId> = app.session.doc.variables().first_collection().modes[..]
            .iter()
            .map(|m| m.id)
            .collect();
        let col = app.session.doc.variables().first_collection().clone();
        assert_eq!(
            modes.iter().map(|m| col.min_width(*m)).collect::<Vec<_>>(),
            [Some(0.0), Some(768.0), Some(1200.0)]
        );
        let shown = texts(&ctx, |_| app.var_browser_ui(&ctx));
        assert!(shown.iter().any(|t| t == "≥768"), "{shown:?}");
        assert!(
            !shown.iter().any(|t| t == "≥0"),
            "the first's 0 is unwritten"
        );

        app.var_act(Act::Breakpoint(modes[1], 1200.0));
        let b = app.var_browser.as_ref().unwrap();
        assert_eq!(
            b.refusal.as_deref(),
            Some(format!("{} starts at 1200.", col.modes[2].name).as_str())
        );
        app.var_act(Act::Breakpoint(modes[1], 900.0));
        let col = app.session.doc.variables().first_collection().clone();
        assert_eq!(col.min_width(modes[1]), Some(900.0));

        let (home, card) = (app.session.ids.mint(), app.session.ids.mint());
        let root = app.session.doc.first_page();
        let frame = |id, parent, w| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: ondin_core::NodeKind::Artboard {
                size: Size::new(w, 400.0),
                corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        };
        assert!(app.commit_edit(Transaction(vec![
            frame(home, root, 1024.0),
            frame(card, home, 100.0),
        ])));
        let doc = &app.session.doc;
        assert!(width_inherits(doc, home, c));
        assert!(
            width_inherits(doc, root, c),
            "a page's Inherit is By width too"
        );
        assert!(!width_inherits(doc, card, c));
        assert_eq!(
            inherited_mode(doc, card, c),
            Some((modes[1], Some(home))),
            "from the top-level frame"
        );
        app.mode_menu = Some((
            home,
            c,
            egui::Rect::from_min_size(egui::pos2(400.0, 100.0), egui::vec2(200.0, 28.0)),
        ));
        let menu = texts(&ctx, |_| app.mode_menu_ui(&ctx));
        assert!(menu.iter().any(|t| t == "By width"), "{menu:?}");
        assert!(menu.iter().any(|t| t == "≥900"), "{menu:?}");
        // Pins `ui::menu_height` to what the menu paints, not that `mode_menu_ui`
        // places by it: an estimate put back there leaves this green.
        let painted = ui::painted_size(&ctx, egui::Id::new("frame-mode-menu"))
            .expect("the menu painted")
            .y;
        let computed = ui::menu_height(modes.len() + 1, 1);
        assert!(
            (painted - computed).abs() < 0.01,
            "the mode menu is placed by its painted height: {computed} vs {painted}"
        );
        // Short of room below, it opens above its row rather than over it.
        // flip: `mode_menu_ui` placing with `menu_place` → fails here.
        let low = egui::Rect::from_min_size(egui::pos2(400.0, 760.0), egui::vec2(200.0, 28.0));
        app.mode_menu = Some((home, c, low));
        let _ = texts(&ctx, |_| app.mode_menu_ui(&ctx));
        let card = ctx
            .memory(|m| m.area_rect(egui::Id::new("frame-mode-menu")))
            .expect("drawn");
        assert!(card.bottom() <= low.top(), "above the row: {card:?}");
        app.mode_menu = None;

        app.var_act(Act::ByWidth(c, false));
        let doc = &app.session.doc;
        assert!(!doc.variables().first_collection().by_width());
        assert_eq!(doc.get(home).unwrap().modes(), [modes[1]]);
        assert!(!width_inherits(doc, home, c));
    }

    /// **Modes by a main's width, through the mode row**: a main's menu
    /// offers *Inherit* and *By width* both, sized for both; picking by width,
    /// the main's row and its instance's read the mode the width picks *· by
    /// width*; the instance's menu has *By width* in *Inherit*'s place with the
    /// main's own widths; and a main hugging its content offers no *By width*.
    ///
    /// flip: `collection_mode_row` without its query arm → fails at the main's
    /// row, which reads *· default*.
    #[test]
    fn a_main_picks_by_its_width_through_the_mode_row() {
        let (ctx, mut app) = app();
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Number));
        app.var_act(Act::NewMode);
        app.var_act(Act::NewMode);
        let c = CollectionId::FIRST;
        let modes = app.session.doc.variables().first_collection().modes.clone();
        let (card, inner) = (app.session.ids.mint(), app.session.ids.mint());
        let root = app.session.doc.first_page();
        let frame = |id, parent, w| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: ondin_core::NodeKind::Artboard {
                size: Size::new(w, 200.0),
                corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some("Card".into()),
        };
        assert!(app.commit_edit(Transaction(vec![
            frame(card, root, 300.0),
            frame(inner, card, 100.0),
            Operation::SetComponent {
                id: card,
                component: true,
            },
        ])));
        let place = egui::Rect::from_min_size(egui::pos2(400.0, 100.0), egui::vec2(200.0, 28.0));
        app.mode_menu = Some((card, c, place));
        let menu = texts(&ctx, |_| app.mode_menu_ui(&ctx));
        assert!(menu.iter().any(|t| t == "Inherit"), "{menu:?}");
        assert!(menu.iter().any(|t| t == "By width"), "{menu:?}");
        let painted = ui::painted_size(&ctx, egui::Id::new("frame-mode-menu"))
            .expect("the menu painted")
            .y;
        let computed = ui::menu_height(modes.len() + 2, 1);
        assert!((painted - computed).abs() < 0.01, "{computed} vs {painted}");
        app.mode_menu = None;

        let ops = var::query_ops(&app.session.doc, card, var::ModeOwner::Collection(c), true)
            .expect("a main 300 wide picks by width");
        assert!(app.commit_edit(Transaction(ops)));
        let instance = {
            let doc = &app.session.doc;
            let (tx, made) = ondin_core::insert_subtrees(
                doc,
                &mut app.session.ids,
                &[ondin_core::Placement {
                    nodes: doc.capture_subtree(card).unwrap(),
                    parent: root,
                    index: None,
                }],
                Default::default(),
            );
            assert!(app.commit_edit(tx));
            made[0]
        };
        assert!(app.commit_edit(Transaction(vec![Operation::SetGeometry {
            id: instance,
            geometry: ondin_core::GeometryPatch::Size(Size::new(700.0, 200.0)),
        }])));
        let row = |app: &mut OndinApp, id| {
            let mut shown = Vec::new();
            for _ in 0..2 {
                let out = ctx.run_ui(Default::default(), |ui| app.frame_mode_row(ui, id));
                shown = out
                    .shapes
                    .iter()
                    .filter_map(|s| match &s.shape {
                        egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
            }
            shown
        };
        let main_row = row(&mut app, card);
        assert!(main_row.iter().any(|t| t == "· by width"), "{main_row:?}");
        assert!(
            main_row.iter().any(|t| t == "WIDTHS"),
            "the fold: {main_row:?}"
        );
        let i_row = row(&mut app, instance);
        assert!(
            i_row.iter().any(|t| t == &modes[2].name) && i_row.iter().any(|t| t == "· by width"),
            "the instance reads by width: {i_row:?}"
        );
        app.mode_menu = Some((instance, c, place));
        let menu = texts(&ctx, |_| app.mode_menu_ui(&ctx));
        assert!(menu.iter().any(|t| t == "By width"), "{menu:?}");
        assert!(!menu.iter().any(|t| t == "Inherit"), "{menu:?}");
        assert!(menu.iter().any(|t| t == "≥320"), "{menu:?}");
        app.mode_menu = None;

        // Detached, the instance keeps its query and owns it: its menu can end it.
        assert!(!node_is_main(&app.session.doc, instance));
        let detach = ondin_core::component::detach(&app.session.doc, instance).unwrap();
        assert!(app.commit_edit(detach));
        assert!(node_is_main(&app.session.doc, instance), "a detached copy");

        let mut item = *app.session.doc.get(card).unwrap().item();
        item.width = ondin_core::container::Dimension::FitContent;
        assert!(app.commit_edit(Transaction(vec![Operation::SetLayoutItem {
            id: card,
            item
        }])));
        assert!(var::why_no_query(&app.session.doc, card).is_some());
    }

    /// A right-click cancelling the mode menu's breakpoint scrub drops the draft,
    /// commits nothing, and leaves the menu open though it landed outside it.
    ///
    /// flip: `pressed_elsewhere` without its `in_gesture` test → fails at "the
    /// menu stays open". The commit guard is not reached by this fixture: it has no
    /// egui drag to report a stop.
    #[test]
    fn a_right_click_cancels_a_breakpoint_scrub_and_keeps_the_menu() {
        let (ctx, mut app) = app();
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Number));
        app.var_act(Act::NewMode);
        let c = CollectionId::FIRST;
        app.var_act(Act::ByWidth(c, true));
        let m = app.session.doc.variables().first_collection().modes[1].id;
        let at = egui::pos2(400.0, 100.0);
        app.var_browser.as_mut().unwrap().menu = Some((Menu::Mode(m), at));
        frame(&mut app, &ctx);
        // The state a scrub that a right-click just cancelled leaves, set directly.
        app.var_browser.as_mut().unwrap().width_draft = Some((m, 1000.0));
        app.gesture_cancelled = true;
        let away = egui::pos2(1100.0, 700.0);
        let _ = pass(
            &ctx,
            &mut app,
            vec![
                egui::Event::PointerMoved(away),
                egui::Event::PointerButton {
                    pos: away,
                    button: egui::PointerButton::Secondary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
            |app, ctx| app.var_browser_ui(ctx),
        );
        let b = app.var_browser.as_ref().unwrap();
        assert!(
            matches!(b.menu, Some((Menu::Mode(x), _)) if x == m),
            "the menu stays open"
        );
        assert_eq!(b.width_draft, None, "the draft is dropped");
        assert_eq!(
            app.session.doc.variables().first_collection().min_width(m),
            Some(768.0),
            "nothing is committed"
        );

        // The control: with no gesture, the same press away closes it.
        app.gesture_cancelled = false;
        let release = egui::Event::PointerButton {
            pos: away,
            button: egui::PointerButton::Secondary,
            pressed: false,
            modifiers: Default::default(),
        };
        let mut press = release.clone();
        if let egui::Event::PointerButton { pressed, .. } = &mut press {
            *pressed = true;
        }
        let _ = pass(&ctx, &mut app, vec![release], |app, ctx| {
            app.var_browser_ui(ctx)
        });
        let _ = pass(&ctx, &mut app, vec![press], |app, ctx| {
            app.var_browser_ui(ctx)
        });
        assert!(
            app.var_browser.as_ref().unwrap().menu.is_none(),
            "a press away closes it"
        );
    }

    /// **The `--` list offers only what is scoped to the field**, counts the
    /// rest, and a whole out-of-scope name is the one refused.
    #[test]
    fn the_list_filters_by_scope() {
        let (_ctx, mut app) = app();
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Number));
        app.var_act(Act::NewVar(VarKind::Number));
        let ids: Vec<VarId> = app.session.doc.variables().iter().map(|v| v.id).collect();
        app.var_act(Act::Scope(ids[0], Some(Scope::of([Field::Corner]))));
        let t = app.session.doc.variables();
        let name = t.get(ids[0]).unwrap().name.clone();
        let listed: Vec<VarId> = super::super::binding::matches_for(t, Field::Spacing, "")
            .iter()
            .map(|v| v.id)
            .collect();
        assert_eq!(listed, vec![ids[1]]);
        assert_eq!(super::super::binding::scoped_out(t, Field::Spacing, ""), 1);
        assert_eq!(
            super::super::binding::out_of_scope(t, Field::Spacing, &format!("--{name}"))
                .map(|v| v.id),
            Some(ids[0])
        );
        assert_eq!(scope_words(t.get(ids[0]).unwrap()), "corner radius");
    }

    /// *New mode* on a modeless collection picked by width commits its first
    /// mode with its width.
    /// flip: `Act::NewMode`'s modeless arm a bare `AddMode` → fails at "added".
    #[test]
    fn new_mode_on_a_modeless_collection_picked_by_width_commits() {
        let (_ctx, mut app) = app();
        app.toggle_var_browser();
        let c = CollectionId::FIRST;
        assert!(
            app.session
                .doc
                .variables()
                .first_collection()
                .modes
                .is_empty()
        );
        app.var_act(Act::ByWidth(c, true));
        assert!(app.session.doc.variables().first_collection().by_width());
        app.var_act(Act::NewMode);
        let col = app.session.doc.variables().first_collection();
        assert_eq!(col.modes.len(), 1, "added: {}", app.session.status().text);
        assert_eq!(col.min_width(col.modes[0].id), Some(0.0));
    }

    /// *Scope…* counts each detached layer once, however many of its fields
    /// lose the variable.
    /// flip: `Act::Scope` summing `users_by_field`'s lists → fails at the
    /// status.
    #[test]
    fn scoping_counts_each_detached_layer_once() {
        let (_ctx, mut app) = app();
        app.toggle_var_browser();
        app.var_act(Act::NewVar(VarKind::Number));
        let v = the_only_var(&app).id;
        let rect = app.session.ids.mint();
        let root = app.session.doc.first_page();
        assert!(app.commit_edit(Transaction(vec![
            Operation::CreateNode {
                id: rect,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(40.0, 20.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::SetCornerVars {
                id: rect,
                vars: [Some(v); 4],
            },
            Operation::SetSizeVars {
                id: rect,
                vars: [Some(v), None],
            },
        ])));
        app.var_act(Act::Scope(v, Some(Scope::of([Field::Spacing]))));
        assert_eq!(app.session.status().text, "1 layer was detached.");
        assert!(var::users(&app.session.doc, v).is_empty());
    }

    /// Two boxes of one blue, the first's fill extracted into a variable:
    /// the twin's id and the variable.
    fn extract_one_of_two_blues(app: &mut OndinApp) -> (NodeId, VarId) {
        let page = app.session.doc.first_page();
        let [one, twin] = [(); 2].map(|_| app.session.ids.mint());
        let blue = keyed_by_position([ondin_core::Fill {
            brush: Color::from_rgba8(47, 111, 216, 255).into(),
            visible: true,
        }]);
        let rect = |id, name: &str| Operation::CreateNode {
            id,
            parent: page,
            index: 0,
            kind: ondin_core::NodeKind::Rect {
                size: ondin_core::kurbo::Size::new(40.0, 40.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some(name.into()),
        };
        app.session
            .try_commit(Transaction(vec![
                rect(one, "One"),
                Operation::SetFills {
                    id: one,
                    fills: blue.clone(),
                },
                rect(twin, "Twin"),
                Operation::SetFills {
                    id: twin,
                    fills: blue,
                },
            ]))
            .unwrap();
        let key = super::super::binding::FieldKey::Paint {
            id: one,
            slot: super::super::paint::PaintSlot::Fill(0),
        };
        let value = app.field_value(key).unwrap();
        app.extract_field(key, value);
        (twin, app.field_var(key).unwrap())
    }

    /// **An extract offers the value's other users, and their list binds
    /// them**: extracting one of two blues draws the offer line under the new
    /// row, its *Choose…* opens the list with the twin ticked, and *Bind 1*
    /// binds it.
    ///
    /// flip: `extract_field` leaving `offer` unset → fails at "the offer line".
    #[test]
    fn an_extract_offers_the_other_users_and_the_list_binds_them() {
        let (ctx, mut app) = app();
        crate::theme::install(&ctx);
        let (twin, var) = extract_one_of_two_blues(&mut app);
        let draw: fn(&mut OndinApp, &egui::Context) = |a, c| a.var_browser_ui(c);
        let mut shapes = vec![];
        for _ in 0..3 {
            shapes = pass(&ctx, &mut app, vec![], draw);
        }
        assert!(
            last_text(&shapes, "Bind 1 other field").is_some(),
            "the offer line"
        );
        app.var_browser.as_mut().unwrap().edit = None;
        click_text(&ctx, &mut app, "Choose…", draw);
        assert!(matches!(
            app.var_browser.as_ref().unwrap().menu,
            Some((Menu::Bind(v), _)) if v == var
        ));
        for _ in 0..2 {
            shapes = pass(&ctx, &mut app, vec![], draw);
        }
        let at = last_text(&shapes, "Twin").expect("the twin's row");
        // A double-click goes to the layer and leaves its tick as it was.
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        pass(&ctx, &mut app, vec![egui::Event::PointerMoved(at)], draw);
        for _ in 0..2 {
            pass(&ctx, &mut app, vec![press(true)], draw);
            pass(&ctx, &mut app, vec![press(false)], draw);
        }
        assert_eq!(app.session.selection.single(), Some(twin), "revealed");
        for _ in 0..2 {
            shapes = pass(&ctx, &mut app, vec![], draw);
        }
        assert!(last_text(&shapes, "Bind 1").is_some(), "still ticked");
        click_text(&ctx, &mut app, "Bind 1", draw);
        let fills = &app.session.doc.get(twin).unwrap().paint().fills;
        assert_eq!(fills[0].brush.var(), Some(var), "the twin is bound");
        assert!(
            app.var_browser.as_ref().unwrap().menu.is_none(),
            "and it closes"
        );
    }

    /// The offer line's *Bind 1 other field* binds the twin in one click,
    /// without the list, and the line goes.
    /// flip: `bind_ready_uses` binding no keys → fails at "the twin is bound".
    #[test]
    fn the_offer_line_binds_the_ready_fields_in_one_click() {
        let (ctx, mut app) = app();
        crate::theme::install(&ctx);
        let (twin, var) = extract_one_of_two_blues(&mut app);
        let draw: fn(&mut OndinApp, &egui::Context) = |a, c| a.var_browser_ui(c);
        for _ in 0..3 {
            pass(&ctx, &mut app, vec![], draw);
        }
        app.var_browser.as_mut().unwrap().edit = None;
        click_text(&ctx, &mut app, "Bind 1 other field", draw);
        let fills = &app.session.doc.get(twin).unwrap().paint().fills;
        assert_eq!(fills[0].brush.var(), Some(var), "the twin is bound");
        let b = app.var_browser.as_ref().unwrap();
        assert!(b.menu.is_none(), "without opening the list");
        assert!(b.offer.is_none(), "and the offer line goes");
    }
}
