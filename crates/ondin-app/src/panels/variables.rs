//! The styles and variables browser (§5.3f, D1038 G1–G22; §15 D1040).
//!
//! A panel beside the vertical toolbar, bottom-aligned with its swatches button
//! directly above the hexagon — so it and the insert picker **swap in place**, one
//! open at a time (G11). `Shift+S` or the button opens it; `Escape` or the button
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
use crate::ui;
use eframe::egui;
use ondin_core::peniko::Color;
use ondin_core::style::{self as sty, Style, StyleId, StyleSlot};
use ondin_core::variable::{self as var, Cell, Mode, ModeId, Value, VarId, VarKind, Variable};
use ondin_core::{Described, Document, NodeId, Operation, Transaction};
use std::collections::{BTreeMap, HashSet};

/// The name column, a mode column and the panel's padding (G2): 340 with one
/// mode, 436 with two, 532 with three, the cap.
const NAME_W: f32 = 220.0;
const MODE_W: f32 = 96.0;
const PAD: f32 = 12.0;
const MAX_COLS: usize = 3;
/// The panel's height, as the insert picker's (the two swap in place, so neither
/// jumps), shorter only when the canvas has less room above the button.
const H: f32 = 460.0;
const TAB_H: f32 = 22.0;
const SEARCH_H: f32 = 28.0;
const HEAD_H: f32 = 24.0;
const GROUP_H: f32 = 26.0;
const ROW_H: f32 = 28.0;
/// A variable row's name indent past its group's caret (G1).
const INDENT: f32 = 23.0;
/// An in-place field's text inset from its outline (§15 D1083) — so a field
/// opened over a label puts the text where the label was.
const EDIT_PAD_X: f32 = 6.0;
const MENU_ROW_H: f32 = 26.0;
/// The browser's ground — the text at 17% over black, `#282828` (G1), a step
/// above the inspector's card.
const SURFACE: egui::Color32 = egui::Color32::from_rgb(40, 40, 40);
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
    Group {
        path: String,
        text: String,
    },
    /// A style's whole name, on the *Styles* tab (G8, §15 D1063).
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
}

impl Edit {
    /// What the field holds.
    fn text(&self) -> &str {
        match self {
            Edit::Name { text, .. }
            | Edit::Cell { text, .. }
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
    /// (§15 D1059), *String variable* (§15 D1069), *New mode* (G5).
    Add,
    Var(VarId),
    Mode(ModeId),
    Group(String),
    /// *Delete mode…*'s confirmation, with the frames it returns to *Inherit*.
    ConfirmMode(ModeId),
    /// A style's row: *Rename*, *Edit*, *Select*, *Delete* (G8).
    Style(StyleId),
    /// A style group's header: *Rename group*, *Select*, *Delete group*.
    StyleGroup(StyleSlot, String),
    /// A kit's mapping for one of the document's modes (D1038 (b), §15 D1065):
    /// the kit's modes, then *Match by name*.
    KitMap(String, ModeId),
}

/// The browser's own state while it is open.
#[derive(Default)]
pub(crate) struct Browser {
    tab: Tab,
    query: String,
    /// Whether the search should take the keyboard next frame — once, on open.
    focus: bool,
    /// *In selection*, kept pressed across selection changes (G4).
    in_selection: bool,
    /// Collapsed group paths — a copy of the document's own set
    /// (`OndinApp::var_collapsed`), loaded for [`Self::root`] (§15 D1054).
    collapsed: HashSet<String>,
    /// The root of the document `collapsed` was loaded for.
    root: Option<NodeId>,
    edit: Option<Edit>,
    /// Whether the field `edit` names still has to take the keyboard.
    edit_focus: bool,
    menu: Option<(Menu, egui::Pos2)>,
    /// The sentence a refused edit is answered with, until the next edit.
    refusal: Option<String>,
    /// The text that refusal answered, which the field draws dim and dotted
    /// while it still holds it (B4's manner, §15 D1053).
    refused: Option<String>,
    /// How far the mode columns are scrolled sideways, past three modes (G3).
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
    GroupName(String, String),
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
    /// A style renamed, its *Edit* opened, deleted (G8, G9; §15 D1063).
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
    /// A kit header's *v8 → v9* clicked (G22): the update summary opened.
    KitUpdates,
}

/// One row of the table as drawn: a group's header or a variable, or a kit's
/// header above its own (§15 D1065).
enum Line<'a> {
    Group {
        path: &'a str,
        shown: usize,
        total: usize,
        /// The kit the group is a group of, `None` for the document's own.
        kit: Option<&'a str>,
    },
    Var(&'a Variable),
    Kit(&'a str),
}

/// The key a group's collapse is kept under: its path for the document's own, a
/// kit's prefixed by `\u{2}` and the kit id, so a kit's *brand* folds apart from
/// the document's (§15 D1065, D1054's per-document set).
fn group_key(kit: Option<&str>, path: &str) -> String {
    match kit {
        None => path.to_string(),
        Some(k) => format!("\u{2}{k}\u{2}{path}"),
    }
}

impl OndinApp {
    /// Open the browser with its search taking the keyboard, or close it
    /// (`Shift+S`, the swatches button). Opening it closes the insert picker:
    /// the two share a place (G11).
    pub(crate) fn toggle_var_browser(&mut self) {
        self.var_browser = match self.var_browser {
            Some(_) => None,
            None => {
                self.kit_picker = None;
                // Loaded at the open, not on the first frame, so nothing done
                // before that frame is overwritten by the load (§15 D1054).
                let root = self.session.doc.first_page();
                Some(Browser {
                    focus: true,
                    collapsed: self.var_collapsed.get(&root).cloned().unwrap_or_default(),
                    root: Some(root),
                    ..Browser::default()
                })
            }
        };
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
        // **Collapsed groups are the document's, not the panel's** (D1038
        // G-series, §15 D1054): loaded from the app's per-document sets — keyed
        // by the first page's id, which a file keeps across saves and loads —
        // whenever the document under the browser is not the one it loaded for,
        // and written back after the frame's acts.
        let root = self.session.doc.first_page();
        if b.root != Some(root) {
            b.collapsed = self.var_collapsed.get(&root).cloned().unwrap_or_default();
            b.root = Some(root);
        }
        let pending = self.var_cell_preview;
        // The kits with an update waiting, for their headers' *v8 → v9* (G22).
        let waiting: Vec<(String, u32, u32)> = self
            .kits_waiting(false)
            .into_iter()
            .map(|w| (w.kit.id, w.held, w.kit.version))
            .collect();
        let doc = &self.session.doc;
        let table = doc.variables();
        let modes: Vec<Mode> = table.modes().to_vec();
        let anchor = if self.var_button.is_finite() && self.show_toolbar {
            egui::pos2(
                self.var_button.right() + 8.0,
                self.kit_button.bottom().max(self.var_button.bottom()),
            )
        } else {
            self.canvas_rect.left_bottom() + egui::vec2(8.0, -8.0)
        };
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
        // And the styles' (G8, §15 D1063), for the *Styles* tab.
        let mut style_users: BTreeMap<StyleId, Vec<NodeId>> = BTreeMap::new();
        for s in doc.styles().iter() {
            let mut who = sty::users(doc, s.id);
            who.retain(|u| !doc.in_vault(*u));
            style_users.insert(s.id, who);
        }
        let style_picked: Option<HashSet<StyleId>> = b.in_selection.then(|| {
            let ids = ondin_core::subtree_nodes(doc, self.session.selection.ids());
            doc.styles()
                .iter()
                .filter(|s| style_users[&s.id].iter().any(|u| ids.contains(u)))
                .map(|s| s.id)
                .collect()
        });
        let query = b.query.trim().to_lowercase();
        let lines = all_lines(doc, &query, picked.as_ref(), &b.collapsed);
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
                        [one] => doc
                            .get(*one)
                            .map(|n| n.name().to_string())
                            .unwrap_or_default(),
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
        let panel = egui::Area::new(egui::Id::new("var-browser"))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor - egui::vec2(0.0, height))
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(SURFACE)
                    .corner_radius(8)
                    .stroke(egui::Stroke::new(1.0, color::text_a(20)))
                    .inner_margin(PAD)
                    .shadow(egui::Shadow {
                        offset: [0, 20],
                        blur: 50,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(128),
                    })
                    .show(ui, |ui| {
                        ui.set_width(w - 2.0 * PAD);
                        ui.set_height(height - 2.0 * PAD);
                        ui.spacing_mut().item_spacing = egui::vec2(6.0, 10.0);
                        tabs(ui, &mut b.tab);
                        search_row(ui, &mut b, &mut acts);
                        if let Some(line) = &selection_line {
                            // Two points further off the search row than it
                            // sat, at the maintainer's look (§15 D1086).
                            ui.add_space(-6.0);
                            ui.label(
                                egui::RichText::new(line)
                                    .size(10.5)
                                    .color(theme::text::DIM),
                            );
                        }
                        if let Some(why) = &b.refusal {
                            ui.add_space(-6.0);
                            ui.label(egui::RichText::new(why).size(11.0).color(theme::text::MUTED));
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
                            Tab::Variables if lines.is_empty() => empty(
                                ui,
                                "Nothing matches",
                                if b.in_selection {
                                    "Turn off In selection to see them all."
                                } else {
                                    "Try another name."
                                },
                            ),
                            Tab::Variables => {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                header(ui, &modes, &mut b, &mut acts);
                                egui::ScrollArea::vertical()
                                    .auto_shrink([false, false])
                                    .scroll_bar_visibility(
                                        egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                                    )
                                    .show(ui, |ui| {
                                        ui.spacing_mut().item_spacing.y = 1.0;
                                        ui.add_space(4.0);
                                        for line in &lines {
                                            match line {
                                                Line::Group {
                                                    path,
                                                    shown,
                                                    total,
                                                    kit,
                                                } => group_row(
                                                    ui,
                                                    path,
                                                    *kit,
                                                    *shown,
                                                    *total,
                                                    picked.is_some(),
                                                    &mut b,
                                                    &mut acts,
                                                ),
                                                Line::Var(v) if v.origin.is_some() => {
                                                    kit_var_row(
                                                        ui,
                                                        doc,
                                                        v,
                                                        &modes,
                                                        &users[&v.id],
                                                        &b,
                                                        &mut acts,
                                                    )
                                                }
                                                Line::Var(v) => var_row(
                                                    ui, doc, v, &modes, &users[&v.id], &mut b,
                                                    &mut acts, pending,
                                                ),
                                                Line::Kit(k) => kit_row(
                                                    ui,
                                                    doc,
                                                    k,
                                                    &modes,
                                                    waiting
                                                        .iter()
                                                        .find(|w| w.0 == *k)
                                                        .map(|w| (w.1, w.2)),
                                                    &b,
                                                    &mut acts,
                                                ),
                                            }
                                        }
                                    });
                            }
                        }
                    });
            })
            .response
            .rect;
        b.panel = Some(panel);

        // Past three modes the columns scroll sideways under the fixed name
        // column (G3), by a horizontal wheel or `Shift` and the wheel.
        // Only where there are columns to scroll: the *Variables* tab with rows
        // on it — not the *Styles* tab nor an empty state (`arch-scribe`'s read
        // of §15 D1055's first cut).
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

        if let Some((menu, at)) = b.menu.clone() {
            self.var_menu(ctx, &mut b, menu, at, &users, &mut acts);
        }

        self.var_browser = Some(b);
        for act in acts {
            self.var_act(act);
        }
        // The collapsed groups written through to the document's own set, so
        // they outlive the browser and follow the document (§15 D1054).
        let root = self.session.doc.first_page();
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
            Menu::ConfirmMode(_) => 240.0,
            _ => 200.0,
        };
        let rows = match &menu {
            Menu::Add => 3.0,
            Menu::Var(_) => 5.0,
            Menu::Mode(_) if table.modes().len() > 1 => 6.5,
            Menu::Mode(_) => 3.0,
            Menu::Group(_) => 3.0,
            Menu::ConfirmMode(_) => 3.0,
            Menu::Style(_) => 5.0,
            Menu::StyleGroup(..) => 3.0,
            Menu::KitMap(kit, _) => table.kits().get(kit).map_or(0, |k| k.modes.len()) as f32 + 1.5,
        };
        let size = egui::vec2(menu_w, rows * MENU_ROW_H + 24.0);
        let pos = ui::menu_place(ctx, at, size);
        let mut close = false;
        let rect = egui::Area::new(egui::Id::new("var-browser-menu"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                ui::menu_frame(4.0).show(ui, |ui| {
                    ui.set_width(menu_w - 8.0);
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
                            if row(ui, icon::CHECK, "Boolean variable") {
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
                        }
                        Menu::Var(v) => {
                            let Some(v) = table.get(*v) else {
                                close = true;
                                return;
                            };
                            if row(ui, icon::PENCIL_SIMPLE, "Rename") {
                                acts.push(Act::StartEdit(Edit::Name {
                                    var: v.id,
                                    text: v.name.clone(),
                                }));
                                close = true;
                            }
                            if row(ui, icon::COPY, "Duplicate") {
                                acts.push(Act::Duplicate(v.id));
                                close = true;
                            }
                            if row(ui, icon::TEXT_ALIGN_LEFT, "Description") {
                                acts.push(Act::StartEdit(Edit::Describe {
                                    of: Described::Variable(v.id),
                                    text: v.description.clone(),
                                }));
                                close = true;
                            }
                            let n = users.get(&v.id).map_or(0, Vec::len);
                            let select = ui::menu_row(
                                ui,
                                ui::MenuRow::new(icon::SELECTION, &layers_label(n)).enabled(n > 0),
                                MENU_ROW_H,
                            );
                            if select.clicked() {
                                acts.push(Act::Select(users[&v.id].clone()));
                                close = true;
                            }
                            ui::menu_sep(ui);
                            let del = ui::menu_row(
                                ui,
                                ui::MenuRow::new(icon::TRASH, "Delete").danger(true),
                                MENU_ROW_H,
                            );
                            if del.clicked() {
                                acts.push(Act::Delete(vec![v.id], v.name.clone()));
                                close = true;
                            }
                        }
                        Menu::Mode(m) => {
                            let Some(mode) = table.mode(*m) else {
                                close = true;
                                return;
                            };
                            if row(ui, icon::PENCIL_SIMPLE, "Rename mode") {
                                acts.push(Act::StartEdit(Edit::Mode {
                                    mode: mode.id,
                                    text: mode.name.clone(),
                                }));
                                close = true;
                            }
                            if row(ui, icon::COPY, "Duplicate mode") {
                                acts.push(Act::DuplicateMode(mode.id));
                                close = true;
                            }
                            // The order: the first mode is the default every
                            // unset frame reads.
                            let n = table.modes().len();
                            let i = table.mode_index(mode.id).unwrap_or(0);
                            if n > 1 {
                                ui::menu_sep(ui);
                                let moves = [
                                    (icon::STAR, "Make default", 0, i > 0),
                                    (icon::ARROW_LEFT, "Move left", i.saturating_sub(1), i > 0),
                                    (icon::ARROW_RIGHT, "Move right", i + 1, i + 1 < n),
                                ];
                                for (glyph, label, to, on) in moves {
                                    let r = ui::menu_row(
                                        ui,
                                        ui::MenuRow::new(glyph, label).enabled(on),
                                        MENU_ROW_H,
                                    );
                                    if on && r.clicked() {
                                        acts.push(Act::MoveMode(mode.id, to));
                                        close = true;
                                    }
                                }
                                ui::menu_sep(ui);
                            }
                            // The last mode cannot go while there are variables:
                            // every variable has a value in every mode.
                            let last = table.modes().len() == 1 && table.local().next().is_some();
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
                        Menu::Group(path) => {
                            if row(ui, icon::PENCIL_SIMPLE, "Rename group") {
                                acts.push(Act::StartEdit(Edit::Group {
                                    path: path.clone(),
                                    text: path.clone(),
                                }));
                                close = true;
                            }
                            let members = group_members(table, path);
                            let mut who: Vec<NodeId> = members
                                .iter()
                                .flat_map(|v| users[&v.id].iter().copied())
                                .collect();
                            who.sort();
                            who.dedup();
                            let select = ui::menu_row(
                                ui,
                                ui::MenuRow::new(icon::SELECTION, &layers_label(who.len()))
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
                            if row(ui, icon::PENCIL_SIMPLE, "Rename") {
                                acts.push(Act::StartEdit(Edit::Style {
                                    style: st.id,
                                    text: st.name.clone(),
                                }));
                                close = true;
                            }
                            if row(ui, icon::TEXT_ALIGN_LEFT, "Description") {
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
                            if row(ui, icon::PENCIL_SIMPLE, "Rename group") {
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
                            ui.add_space(4.0);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(what)
                                        .size(11.5)
                                        .color(theme::text::MUTED),
                                )
                                .wrap(),
                            );
                            ui.add_space(4.0);
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
        let pressed_elsewhere = ctx.input(|i| {
            i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p))
        });
        if close || pressed_elsewhere {
            b.menu = None;
        }
    }

    /// Apply one of the browser's asks.
    fn var_act(&mut self, act: Act) {
        let doc = &self.session.doc;
        let table = doc.variables();
        match act {
            Act::StyleName(id, text) => {
                let text = text.trim().to_string();
                let Some(st) = doc.styles().get(id) else {
                    self.var_edit_done();
                    return;
                };
                let taken = doc
                    .styles()
                    .by_name(st.slot(), &text)
                    .is_some_and(|o| o.id != id);
                if text.is_empty() {
                    self.var_refuse("A style needs a name.".into());
                } else if taken {
                    self.var_refuse(format!("{text} already exists."));
                } else {
                    let changed = st.name != text;
                    self.var_edit_done();
                    if changed {
                        self.session.name_step("Rename style", icon::PENCIL_SIMPLE);
                        self.commit_edit(Transaction(vec![Operation::SetStyleName {
                            id,
                            name: text,
                        }]));
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
                            self.session.name_step("Rename group", icon::PENCIL_SIMPLE);
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
                    self.session.name_step("Description", icon::PENCIL_SIMPLE);
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
                let mut who: Vec<NodeId> = ids.iter().flat_map(|s| sty::users(doc, *s)).collect();
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
                    let mut at = 0;
                    while let Some(i) = v.name[at..].find('/') {
                        all.insert(group_key(v.kit(), &v.name[..at + i]));
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
                }
            }
            Act::KitUpdates => self.open_kit_updates(),
            Act::Select(ids) => {
                if !ids.is_empty() {
                    self.session.selection.set(ids);
                }
            }
            Act::NewVar(kind) => {
                let mut ops = Vec::new();
                let first = match table.default_mode() {
                    Some(m) => m,
                    None => {
                        let m = self.session.ids.mint_mode();
                        ops.push(Operation::AddMode {
                            mode: Mode {
                                id: m,
                                name: "Mode 1".into(),
                            },
                            index: 0,
                            cells: vec![],
                        });
                        m
                    }
                };
                let modes: Vec<ModeId> = if table.modes().is_empty() {
                    vec![first]
                } else {
                    table.modes().iter().map(|m| m.id).collect()
                };
                let (stem, cell) = match kind {
                    VarKind::Color => ("colour", Cell::Color(Color::WHITE)),
                    VarKind::Number => ("number", Cell::Number(0.0)),
                    VarKind::Boolean => ("boolean", Cell::Boolean(true)),
                    // A word to start from rather than nothing (§15 D1069): an
                    // empty string bound to a text's content draws no text at
                    // all, which reads as a fault.
                    VarKind::String => ("string", Cell::String("Text".into())),
                };
                let taken: Vec<&str> = table.iter().map(|v| v.name.as_str()).collect();
                let name = free_name(&taken, stem, 1);
                let id = self.session.ids.mint_var();
                ops.push(Operation::AddVariable {
                    var: Variable {
                        id,
                        name: name.clone(),
                        kind,
                        cells: modes.into_iter().map(|m| (m, cell.clone())).collect(),
                        origin: None,
                        forked: None,
                        description: String::new(),
                    },
                });
                self.session.name_step("New variable", icon::BRACKETS_CURLY);
                if self.commit_edit(Transaction(ops))
                    && let Some(b) = self.var_browser.as_mut()
                {
                    b.edit = Some(Edit::Name {
                        var: id,
                        text: name,
                    });
                    b.edit_focus = true;
                }
            }
            Act::NewMode => {
                let taken: Vec<&str> = table.modes().iter().map(|m| m.name.as_str()).collect();
                let name = free_name(&taken, "Mode", table.modes().len() + 1);
                let id = self.session.ids.mint_mode();
                let ops = match table.default_mode() {
                    Some(first) => var::copy_mode_ops(doc, first, id, &name, table.modes().len()),
                    None => vec![Operation::AddMode {
                        mode: Mode {
                            id,
                            name: name.clone(),
                        },
                        index: 0,
                        cells: vec![],
                    }],
                };
                self.session.name_step("New mode", icon::PLUS);
                if !ops.is_empty()
                    && self.commit_edit(Transaction(ops))
                    && let Some(b) = self.var_browser.as_mut()
                {
                    b.edit = Some(Edit::Mode {
                        mode: id,
                        text: name,
                    });
                    b.edit_focus = true;
                }
            }
            Act::MoveMode(mode, to) => {
                let ops = var::move_mode_ops(doc, mode, to);
                if !ops.is_empty() {
                    self.session.name_step("Move mode", icon::ARROWS_LEFT_RIGHT);
                    self.commit_edit(Transaction(ops));
                }
            }
            Act::DuplicateMode(from) => {
                let Some(index) = table.mode_index(from) else {
                    return;
                };
                let taken: Vec<&str> = table.modes().iter().map(|m| m.name.as_str()).collect();
                let stem = format!("{} copy", table.modes()[index].name);
                let name = if taken.contains(&stem.as_str()) {
                    free_name(&taken, &stem, 2)
                } else {
                    stem
                };
                let id = self.session.ids.mint_mode();
                let ops = var::copy_mode_ops(doc, from, id, &name, index + 1);
                if !ops.is_empty() {
                    self.session.name_step("Duplicate mode", icon::COPY);
                    self.commit_edit(Transaction(ops));
                }
            }
            Act::Name(id, text) => {
                let text = text.trim().to_string();
                match name_refusal(table, &text, Some(id)) {
                    Some(why) => self.var_refuse(why),
                    None => {
                        let changed = table.get(id).is_some_and(|v| v.name != text);
                        self.var_edit_done();
                        if changed {
                            self.session
                                .name_step("Rename variable", icon::PENCIL_SIMPLE);
                            self.commit_edit(Transaction(vec![Operation::SetVariableName {
                                id,
                                name: text,
                            }]));
                        }
                    }
                }
            }
            Act::ModeName(id, text) => {
                let text = text.trim().to_string();
                let taken = table.modes().iter().any(|m| m.id != id && m.name == text);
                if text.is_empty() {
                    self.var_refuse("A mode needs a name.".into());
                } else if taken {
                    self.var_refuse(format!("{text} already exists."));
                } else {
                    let ops = var::rename_mode_ops(doc, id, &text);
                    self.var_edit_done();
                    if !ops.is_empty() {
                        self.session.name_step("Rename mode", icon::PENCIL_SIMPLE);
                        self.commit_edit(Transaction(ops));
                    }
                }
            }
            Act::GroupName(from, to) => {
                let to = to.trim().trim_matches('/').to_string();
                if to == from {
                    self.var_edit_done();
                    return;
                }
                if to.split('/').any(|s| s.trim().is_empty()) && !to.is_empty() {
                    self.var_refuse("A group's name cannot have an empty part.".into());
                    return;
                }
                match var::rename_group_ops(doc, &from, &to) {
                    Err(clash) => self.var_refuse(format!("{clash} already exists.")),
                    Ok(ops) => {
                        self.var_edit_done();
                        if !ops.is_empty() {
                            self.session.name_step("Rename group", icon::PENCIL_SIMPLE);
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
                let taken: Vec<&str> = table.local().map(|v| v.name.as_str()).collect();
                let stem = format!("{} copy", v.name);
                let name = if taken.contains(&stem.as_str()) {
                    free_name(&taken, &stem, 2)
                } else {
                    stem
                };
                let copy = Variable {
                    id: self.session.ids.mint_var(),
                    name,
                    ..v.clone()
                };
                self.session.name_step("Duplicate variable", icon::COPY);
                self.commit_edit(Transaction(vec![Operation::AddVariable { var: copy }]));
            }
            Act::Delete(ids, label) => {
                let mut layers: Vec<NodeId> =
                    ids.iter().flat_map(|v| var::users(doc, *v)).collect();
                layers.sort();
                layers.dedup();
                // The value the users now hold, named only when every one of them
                // drew the same: each detaches to what it read in its own mode, and
                // two frames in two modes leave two values (G10's sentence is for
                // the one-value case).
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
            Act::DeleteMode(mode) => {
                let name = table.mode(mode).map(|m| m.name.clone()).unwrap_or_default();
                let tx = Transaction(var::remove_mode_ops(doc, mode));
                self.session.name_step("Delete mode", icon::TRASH);
                if self.commit_edit(tx) {
                    self.session.info(format!("Deleted mode {name}"));
                }
            }
        }
    }

    /// Commit a typed cell, or answer why not. Whether it landed.
    fn var_cell(&mut self, id: VarId, mode: ModeId, text: String) -> bool {
        let table = self.session.doc.variables();
        let Some(v) = table.get(id) else { return false };
        let parsed = parse_cell(table, v, mode, &text);
        let same = parsed.as_ref().is_ok_and(|c| v.cells.get(&mode) == Some(c));
        let cell = match parsed {
            Ok(cell) => cell,
            Err(why) => {
                self.var_refuse(why);
                return false;
            }
        };
        self.var_edit_done();
        if same {
            return true;
        }
        self.session
            .name_step("Variable value", icon::BRACKETS_CURLY);
        self.commit_edit(Transaction(vec![Operation::SetVariableCell {
            id,
            mode,
            cell,
        }]))
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

/// `stem n` for the first `n` from `from` that `taken` does not hold — the
/// browser's own `naming::free_name`, over a slice: a table is a few dozen names.
fn free_name(taken: &[&str], stem: &str, from: usize) -> String {
    (from.max(1)..)
        .map(|n| format!("{stem} {n}"))
        .find(|c| !taken.contains(&c.as_str()))
        .expect("an unbounded range has a free name")
}

/// *Select 3 layers*, *Select 1 layer*, *No layers use it*.
fn layers_label(n: usize) -> String {
    match n {
        0 => "No layers use it".into(),
        1 => "Select 1 layer".into(),
        n => format!("Select {n} layers"),
    }
}

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

/// Why `name` cannot be a variable's (`except` the one being renamed), if it
/// cannot — empty, an empty group part, or taken (G14).
fn name_refusal(
    table: &ondin_core::Variables,
    name: &str,
    except: Option<VarId>,
) -> Option<String> {
    if name.is_empty() {
        return Some("A variable needs a name.".into());
    }
    if name.split('/').any(|s| s.trim().is_empty()) {
        return Some("A name cannot have an empty group.".into());
    }
    let taken = table.iter().any(|v| v.name == name && Some(v.id) != except);
    taken.then(|| format!("{name} already exists."))
}

/// The document's own variables under group `path` — what its menu deletes and
/// selects the users of. A kit's group of the same path lists apart, and its
/// rows are inert (§15 D1065).
fn group_members<'a>(table: &'a ondin_core::Variables, path: &str) -> Vec<&'a Variable> {
    table.local().filter(|v| in_group(&v.name, path)).collect()
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
        let Some(target) = current.or_else(|| table.by_name(name)) else {
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
        VarKind::Number => crate::expr::eval(text)
            .filter(|x| x.is_finite())
            .map(Cell::Number)
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
    kit: Option<&'a str>,
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
                kit,
            });
        }
        // A search opens the groups it matches (D1038 G).
        if path.is_empty() || !query.is_empty() || !collapsed.contains(&group_key(kit, path)) {
            out.extend(shown.into_iter().map(Line::Var));
        }
    }
    out
}

/// The whole table's lines: the document's own, then each kit's under its
/// header, by the kit's name — *kit groups last* (D1038 B, §15 D1065). A kit
/// with nothing shown has no header.
fn all_lines<'a>(
    doc: &'a Document,
    query: &str,
    picked: Option<&HashSet<VarId>>,
    collapsed: &HashSet<String>,
) -> Vec<Line<'a>> {
    let table = doc.variables();
    let mut out = lines_of(table.local(), None, query, picked, collapsed);
    let mut kits: Vec<(String, &'a str)> = table
        .kits()
        .keys()
        .map(|k| (super::binding::kit_name(doc, k), k.as_str()))
        .collect();
    kits.sort();
    for (_, k) in kits {
        let lines = lines_of(table.of_kit(k), Some(k), query, picked, collapsed);
        if !lines.is_empty() {
            out.push(Line::Kit(k));
            out.extend(lines);
        }
    }
    out
}

/// The *Variables* / *Styles* strip (G1).
fn tabs(ui: &mut egui::Ui, tab: &mut Tab) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, TAB_H + 4.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 5, color::text_a(13));
    let cell_w = (rect.width() - 4.0 - 2.0) / 2.0;
    for (i, (label, t)) in [("Variables", Tab::Variables), ("Styles", Tab::Styles)]
        .into_iter()
        .enumerate()
    {
        let cell = egui::Rect::from_min_size(
            rect.min + egui::vec2(2.0 + i as f32 * (cell_w + 2.0), 2.0),
            egui::vec2(cell_w, TAB_H),
        );
        let resp = ui.interact(cell, ui.id().with(("var-tab", i)), egui::Sense::click());
        let on = *tab == t;
        if on {
            ui.painter().rect_filled(cell, 4, color::text_a(33));
        } else if resp.hovered() {
            ui.painter().rect_filled(cell, 4, color::text_a(18));
        }
        ui.painter().text(
            cell.center() + egui::vec2(0.0, 0.5),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(11.0),
            if on { color::TEXT } else { theme::text::MUTED },
        );
        if resp.clicked() {
            *tab = t;
        }
    }
}

/// The search, the *In selection* toggle and the `+` (G1, G4, G5).
fn search_row(ui: &mut egui::Ui, b: &mut Browser, acts: &mut Vec<Act>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let w = ui.available_width() - 2.0 * (SEARCH_H + 6.0);
        let hint = match b.tab {
            Tab::Variables => "Search variables",
            Tab::Styles => "Search styles",
        };
        let resp = ui::field_row(ui, egui::vec2(w, SEARCH_H), |ui| {
            ui.label(theme::icon_text(
                icon::MAGNIFYING_GLASS,
                13.0,
                theme::text::DIM,
            ));
            ui.add(
                egui::TextEdit::singleline(&mut b.query)
                    .id(egui::Id::new(SEARCH_ID))
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY)
                    .font(egui::FontId::proportional(12.0))
                    .hint_text(egui::RichText::new(hint).size(12.0)),
            )
        });
        if b.focus {
            b.focus = false;
            resp.request_focus();
        }
        let toggle =
            square_button(ui, icon::SELECTION, b.in_selection).on_hover_text(match b.tab {
                Tab::Variables => "In selection: only the variables the selection uses",
                Tab::Styles => "In selection: only the styles the selection uses",
            });
        if toggle.clicked() {
            b.in_selection = !b.in_selection;
        }
        let add = square_button(ui, icon::PLUS, matches!(b.menu, Some((Menu::Add, _))))
            .on_hover_text(match b.tab {
                Tab::Variables => "New variable or mode",
                // A style is made from a card's values (F4), and the `+` takes
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

/// A 28×28 button in the search row, pressed while `on`.
fn square_button(ui: &mut egui::Ui, glyph: &str, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(SEARCH_H, SEARCH_H), egui::Sense::click());
    let p = ui.painter();
    let (fill, border, ink) = if on {
        (color::text_a(36), color::text_a(46), color::TEXT)
    } else if resp.hovered() {
        (color::text_a(23), color::text_a(38), theme::text::STRONG)
    } else {
        (color::text_a(13), color::text_a(20), theme::text::MUTED)
    };
    p.rect(
        rect,
        5,
        fill,
        egui::Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::new(14.0, egui::FontFamily::Name("phosphor".into())),
        ink,
    );
    resp
}

/// The *Styles* tab (G8, §15 D1063): one group per card, in the inspector's
/// order, each row a preview, the style's whole name and its usage count; a
/// right-click or a double-click opens *Rename*, *Edit*, *Select*, *Delete*.
/// *In selection* keeps the styles the selection uses, a group's count reading
/// *1 of 4*, and a group with none of them drops out (G20).
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
    // header — *kit groups last* (G8, F5; §15 D1065).
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
            "Nothing matches",
            if b.in_selection {
                "Turn off In selection to see them all."
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
                if resp.hovered() {
                    p.rect_filled(rect, 4, color::text_a(16));
                }
                p.text(
                    egui::pos2(rect.left() + 6.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    if open {
                        icon::CARET_DOWN
                    } else {
                        icon::CARET_RIGHT
                    },
                    egui::FontId::new(11.0, egui::FontFamily::Name("phosphor".into())),
                    theme::text::FAINT,
                );
                let name = p.layout_no_wrap(
                    slot.title().to_string(),
                    egui::FontId::proportional(11.5),
                    color::TEXT,
                );
                let name_w = name.size().x;
                p.galley(
                    egui::pos2(rect.left() + 23.0, rect.center().y - name.size().y / 2.0),
                    name,
                    color::TEXT,
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
                    egui::FontId::proportional(10.5),
                    theme::text::FAINT,
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
        let resp = edit_field(ui, r, text, id.with("edit"), 11.5, None, refused.as_deref());
        ui.painter().rect_stroke(
            r,
            4,
            egui::Stroke::new(1.0, color::ACCENT),
            egui::StrokeKind::Outside,
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
    if menu_open {
        p.rect_filled(rect, 4, color::text_a(23));
    } else if resp.hovered() {
        p.rect_filled(rect, 4, color::text_a(16));
    }
    p.text(
        egui::pos2(left + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        if open {
            icon::CARET_DOWN
        } else {
            icon::CARET_RIGHT
        },
        egui::FontId::new(11.0, egui::FontFamily::Name("phosphor".into())),
        theme::text::FAINT,
    );
    let name = p.layout_no_wrap(
        g.path.to_string(),
        egui::FontId::proportional(11.5),
        color::TEXT,
    );
    let name_w = name.size().x;
    p.galley(
        egui::pos2(left + 23.0, rect.center().y - name.size().y / 2.0),
        name,
        color::TEXT,
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
        egui::FontId::proportional(10.5),
        theme::text::FAINT,
    );
    if g.kit.is_none() && ui::double_clicked(ui, &resp) {
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
    let field = edit_field(ui, r, text, id, 11.5, Some("Description"), None);
    ui.painter().rect_stroke(
        r,
        4,
        egui::Stroke::new(1.0, color::ACCENT),
        egui::StrokeKind::Outside,
    );
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
    if menu_open {
        ui.painter().rect_filled(rect, 4, color::text_a(23));
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 4, color::text_a(18));
    }
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
        let resp = edit_field(ui, r, text, id.with("edit"), 11.5, None, refused.as_deref());
        ui.painter().rect_stroke(
            r,
            4,
            egui::Stroke::new(1.0, color::ACCENT),
            egui::StrokeKind::Outside,
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
        egui::FontId::proportional(11.5),
        theme::text::STRONG,
    );
    p.text(
        egui::pos2(rect.right() - 6.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        count.to_string(),
        egui::FontId::proportional(10.5),
        theme::text::FAINT,
    );
    let menu_at = |r: &egui::Response| {
        r.interact_pointer_pos().unwrap_or(rect.left_bottom()) + egui::vec2(6.0, 8.0)
    };
    if ui::double_clicked(ui, &resp) || resp.secondary_clicked() {
        acts.push(Act::OpenMenu(Menu::Style(s.id), menu_at(&resp)));
    }
}

fn empty(ui: &mut egui::Ui, head: &str, sub: &str) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.label(
            egui::RichText::new(head)
                .size(12.0)
                .color(theme::text::STRONG),
        );
        ui.add_space(-4.0);
        ui.add(
            egui::Label::new(egui::RichText::new(sub).size(11.0).color(theme::text::DIM)).wrap(),
        );
    });
}

/// The columns' x for mode `i`, scrolled.
fn mode_x(left: f32, i: usize, scroll: f32) -> f32 {
    left + NAME_W + MODE_W * i as f32 - scroll
}

/// *Name* and the mode headers — *Value* while there is one mode (G2).
/// Double-click renames a mode; a right-click opens its menu (G6).
fn header(ui: &mut egui::Ui, modes: &[Mode], b: &mut Browser, acts: &mut Vec<Act>) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, HEAD_H), egui::Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        egui::Stroke::new(1.0, color::text_a(20)),
    );
    ui.painter().text(
        egui::pos2(rect.left() + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "NAME",
        egui::FontId::proportional(9.5),
        theme::text::FAINT,
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
            let resp = edit_field(ui, r, text, id.with("edit"), 11.0, None, refused.as_deref());
            ui.painter().rect_stroke(
                r,
                4,
                egui::Stroke::new(1.0, color::ACCENT),
                egui::StrokeKind::Outside,
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
        ui.painter().with_clip_rect(clip).text(
            egui::pos2(cell.left() + 6.0, cell.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(11.0),
            if resp.hovered() {
                theme::text::STRONG
            } else {
                theme::text::MUTED
            },
        );
        if ui::double_clicked(ui, &resp) {
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
    kit: Option<&str>,
    shown: usize,
    total: usize,
    filtered: bool,
    b: &mut Browser,
    acts: &mut Vec<Act>,
) {
    ui.add_space(2.0);
    let w = b.inner_w;
    let key = group_key(kit, path);
    let id = ui.id().with(("var-group", &key));
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, GROUP_H), egui::Sense::click());
    let refused = b.refused.clone();
    if let Some(Edit::Group { path: p, text }) = b.edit.as_mut()
        && p == path
        && kit.is_none()
    {
        let r = egui::Rect::from_min_size(
            rect.min + egui::vec2(20.0, 2.0),
            egui::vec2(NAME_W - 26.0, GROUP_H - 4.0),
        );
        let resp = edit_field(ui, r, text, id.with("edit"), 11.5, None, refused.as_deref());
        ui.painter().rect_stroke(
            r,
            4,
            egui::Stroke::new(1.0, color::ACCENT),
            egui::StrokeKind::Outside,
        );
        if b.edit_focus {
            b.edit_focus = false;
            resp.request_focus();
        }
        ui::select_all_on_focus(ui, &resp, text);
        if ui::defocus_commits(&resp) {
            acts.push(Act::GroupName(path.to_string(), text.clone()));
        }
        return;
    }
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 4, color::text_a(16));
    }
    let open = !b.collapsed.contains(&key) || !b.query.trim().is_empty();
    p.text(
        egui::pos2(rect.left() + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        if open {
            icon::CARET_DOWN
        } else {
            icon::CARET_RIGHT
        },
        egui::FontId::new(11.0, egui::FontFamily::Name("phosphor".into())),
        theme::text::FAINT,
    );
    let name = p.layout_no_wrap(
        path.to_string(),
        egui::FontId::proportional(11.5),
        color::TEXT,
    );
    let name_w = name.size().x;
    p.galley(
        egui::pos2(rect.left() + 23.0, rect.center().y - name.size().y / 2.0),
        name,
        color::TEXT,
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
        egui::FontId::proportional(10.5),
        theme::text::FAINT,
    );
    if kit.is_none() && ui::double_clicked(ui, &resp) {
        acts.push(Act::StartEdit(Edit::Group {
            path: path.to_string(),
            text: path.to_string(),
        }));
    } else if resp.clicked() && ui.input(|i| i.modifiers.alt) {
        acts.push(Act::ToggleAll(key));
    } else if resp.clicked() {
        acts.push(Act::Toggle(key));
    } else if kit.is_none()
        && resp.secondary_clicked()
        && let Some(at) = resp.interact_pointer_pos()
    {
        acts.push(Act::OpenMenu(
            Menu::Group(path.to_string()),
            at + egui::vec2(6.0, 8.0),
        ));
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
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, ROW_H), egui::Sense::click());
    let menu_open = matches!(&b.menu, Some((Menu::Var(m), _)) if *m == v.id);
    if menu_open {
        ui.painter().rect_filled(rect, 4, color::text_a(23));
    } else if ui.rect_contains_pointer(rect) {
        // Not the row's own response: the name's and the cells' hit areas lie
        // over it and take the hover.
        ui.painter().rect_filled(rect, 4, color::text_a(18));
    }
    // From where the rename field starts, across the cells: a variable's name
    // column is too narrow for a sentence.
    let field = egui::Rect::from_min_max(
        egui::pos2(rect.left() + INDENT - 6.0, rect.top() + 3.0),
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
            rect.min + egui::vec2(INDENT - 6.0, 3.0),
            egui::vec2(NAME_W - INDENT, ROW_H - 6.0),
        );
        let field = edit_field(
            ui,
            r,
            text,
            id.with("name"),
            11.5,
            Some("name, / to group"),
            refused.as_deref(),
        );
        ui.painter().rect_stroke(
            r,
            4,
            egui::Stroke::new(1.0, color::ACCENT),
            egui::StrokeKind::Outside,
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
            egui::FontId::proportional(10.5),
            theme::text::FAINT,
        );
        let count_rect = egui::Rect::from_min_size(
            egui::pos2(
                name_rect.right() - 6.0 - count_galley.size().x,
                rect.center().y - count_galley.size().y / 2.0,
            ),
            count_galley.size(),
        );
        let leaf_w = count_rect.left() - (rect.left() + INDENT) - 6.0;
        let galley = p.layout(
            leaf.to_string(),
            egui::FontId::proportional(11.5),
            color::TEXT,
            f32::INFINITY,
        );
        let clip = egui::Rect::from_min_size(
            egui::pos2(rect.left() + INDENT, rect.top()),
            egui::vec2(leaf_w.max(0.0), ROW_H),
        );
        p.with_clip_rect(clip).galley(
            egui::pos2(
                rect.left() + INDENT,
                rect.center().y - galley.size().y / 2.0,
            ),
            galley,
            color::TEXT,
        );
        let count_resp = ui.interact(
            count_rect.expand(3.0),
            id.with("count"),
            egui::Sense::click(),
        );
        let count_col = if count_resp.hovered() && count > 0 {
            color::TEXT
        } else {
            theme::text::FAINT
        };
        ui.painter().text(
            count_rect.left_center(),
            egui::Align2::LEFT_CENTER,
            count.to_string(),
            egui::FontId::proportional(10.5),
            count_col,
        );
        if count > 0 {
            let count_resp = count_resp.on_hover_text(layers_label(count));
            if count_resp.clicked() {
                acts.push(Act::Select(users.to_vec()));
            }
        }
        let mut name_resp = ui.interact(name_rect, id.with("name-hit"), egui::Sense::click());
        if !v.description.is_empty() {
            name_resp = name_resp.on_hover_text(&v.description);
        }
        if ui::double_clicked(ui, &name_resp) {
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
            ui.painter().rect_filled(r, 4, color::text_a(13));
            let field = edit_field(
                ui,
                r,
                text,
                id.with(("cell", m.id)),
                11.0,
                None,
                refused.as_deref(),
            );
            ui.painter().rect_stroke(
                r,
                4,
                egui::Stroke::new(1.0, color::ACCENT),
                egui::StrokeKind::Outside,
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
        if colour && ui::double_clicked(ui, &cell_resp) {
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
/// the name reads on *· v8 → v9* (G22, D1038's *Kit updates*), a click opening
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
        theme::text::DIM,
    );
    let named = p.text(
        egui::pos2(g.right() + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        super::binding::kit_name(doc, kit),
        egui::FontId::proportional(11.5),
        theme::text::STRONG,
    );
    if let Some((held, next)) = waiting {
        let at = egui::pos2(named.right() + 5.0, rect.center().y);
        let galley = ui.ctx().fonts_mut(|f| {
            f.layout_no_wrap(
                format!("· v{held} → v{next}"),
                egui::FontId::proportional(10.5),
                theme::text::DIM,
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
                theme::text::STRONG
            } else {
                theme::text::DIM
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
            egui::FontId::proportional(11.0),
            if hand {
                theme::text::STRONG
            } else {
                theme::text::DIM
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
    if ui.rect_contains_pointer(rect) {
        ui.painter().rect_filled(rect, 4, color::text_a(10));
    }
    let name_rect = egui::Rect::from_min_size(rect.min, egui::vec2(NAME_W, ROW_H));
    let (_, leaf) = var::split_name(&v.name);
    let count = users.len();
    let p = ui.painter();
    let count_galley = p.layout_no_wrap(
        count.to_string(),
        egui::FontId::proportional(10.5),
        theme::text::FAINT,
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
        egui::FontId::proportional(11.5),
        theme::text::MUTED,
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
        egui::FontId::proportional(10.5),
        if count_resp.hovered() && count > 0 {
            color::TEXT
        } else {
            theme::text::FAINT
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
/// naming its target (G1, H4).
fn draw_cell(p: &egui::Painter, table: &ondin_core::Variables, cell: egui::Rect, c: &Cell) {
    let y = cell.center().y;
    let font = egui::FontId::proportional(11.0);
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
                theme::text::STRONG,
            );
        }
        Cell::Number(x) => {
            p.text(
                egui::pos2(cell.left() + 6.0, y + 0.5),
                egui::Align2::LEFT_CENTER,
                number_text(*x),
                font,
                theme::text::STRONG,
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
                theme::text::STRONG,
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
                    color::CARD,
                );
            } else {
                p.rect_stroke(
                    bx,
                    3,
                    egui::Stroke::new(1.0, color::text_a(102)),
                    egui::StrokeKind::Inside,
                );
            }
            p.text(
                egui::pos2(bx.right() + 6.0, y + 0.5),
                egui::Align2::LEFT_CENTER,
                super::binding::bool_word(*b),
                font,
                theme::text::STRONG,
            );
        }
        Cell::Alias(t) => {
            let name = table.get(*t).map(|t| t.name.clone()).unwrap_or_default();
            let galley = p.layout(name, font, color::TEXT, MODE_W - 12.0 - 14.0);
            let w = (galley.size().x + 14.0).min(MODE_W - 12.0);
            let cap = egui::Rect::from_min_size(
                egui::pos2(cell.left() + 6.0, y - 9.0),
                egui::vec2(w, 18.0),
            );
            p.rect_filled(cap, 9, color::text_a(28));
            p.with_clip_rect(cap.shrink2(egui::vec2(7.0, 0.0))).galley(
                egui::pos2(cap.left() + 7.0, y - galley.size().y / 2.0 + 0.5),
                galley,
                color::TEXT,
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
/// widget — the four were `ui.put` each — and **refused in B4's manner** (D1038
/// G14, G15; §15 D1053): while its text is still the text a refusal answered
/// (`refused`), that text is laid out dim over a dotted underline, the
/// inspector's fields' own layouter and dots (`binding::refusal_galley`,
/// `underline_refusal`), and the sentence above the table says why. A keystroke
/// makes it ordinary text again.
///
/// `r` is the field's outline; the text sits [`EDIT_PAD_X`] inside it, centred
/// on its height.
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
    let mut edit = egui::TextEdit::singleline(text)
        .id(id)
        .frame(egui::Frame::NONE)
        .font(egui::FontId::proportional(pt))
        .layouter(&mut layouter);
    if let Some(h) = hint {
        edit = edit.hint_text(h);
    }
    // **Inset and centred in `r`, which is the outline the caller strokes**
    // (§15 D1083): the text sat on the box's top edge flush against its left,
    // because a `Frame::NONE` edit has no margin and `centered_and_justified`
    // stretches it to the whole box with its row at the top. A box exactly one
    // row tall, [`EDIT_PAD_X`] in from either side, puts it where the label it
    // replaces was.
    let row_h = ui.fonts_mut(|f| f.row_height(&egui::FontId::proportional(pt)));
    let inner = egui::Rect::from_min_max(
        egui::pos2(r.left() + EDIT_PAD_X, r.center().y - row_h / 2.0),
        egui::pos2(
            (r.right() - EDIT_PAD_X).max(r.left() + EDIT_PAD_X),
            r.center().y + row_h / 2.0,
        ),
    );
    // **A child that leaves the parent's cursor alone**, not a scope: a scope
    // advances the cursor past what it drew, and a field inside a row the
    // caller had already allocated put the cursor back *above* that row's
    // bottom — the table under a mode being renamed came up five points.
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(
        egui::Layout::centered_and_justified(egui::Direction::TopDown),
    ));
    let out = edit.show(&mut child);
    let at = (refused == Some(text.as_str())).then_some(0);
    super::binding::underline_refusal(ui, &out, at);
    out.response.response
}

/// A 12×12 colour chip, the checkerboard under a translucent one (G1) — the
/// Fill row's look (§15 D1082).
pub(crate) fn swatch(p: &egui::Painter, rect: egui::Rect, c: Color) {
    let painter = p.with_clip_rect(rect.intersect(p.clip_rect()));
    ui::paint_colour_chip(&painter, rect, ui::color_to_egui(c));
}

/// The mode frame `id` inherits when it sets none — the nearest ancestor frame's
/// that sets one, and that frame, else the document's default (D1038 (a)).
pub(crate) fn inherited_mode(doc: &Document, id: NodeId) -> Option<(ModeId, Option<NodeId>)> {
    let mut at = doc.get(id)?.parent();
    while let Some(p) = at {
        let n = doc.get(p)?;
        if let Some(m) = n.mode() {
            return Some((m, Some(p)));
        }
        at = n.parent();
    }
    Some((doc.variables().default_mode()?, None))
}

impl OndinApp {
    /// A frame's or a page's *Mode* row, in its identity card under its name
    /// (D1038 (a); §15 D1042): set, the mode reads bright; inherited, dim and
    /// naming its source — *Light · from Hero / Desktop*, *Dark · from page*,
    /// *Light · default*. A click opens the menu, *Inherit* first. Drawn only in a
    /// document that has a mode to set.
    pub(crate) fn frame_mode_row(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let doc = &self.session.doc;
        let Some(node) = doc.get(id) else { return };
        let page = doc.is_page(id);
        if !page && !matches!(node.kind(), ondin_core::NodeKind::Artboard { .. }) {
            return;
        }
        let table = doc.variables();
        if table.modes().is_empty() {
            return;
        }
        let (value, suffix, set) = match node.mode().and_then(|m| table.mode(m)) {
            Some(m) => (m.name.clone(), None, true),
            None => {
                let Some((m, from)) = inherited_mode(doc, id) else {
                    return;
                };
                let name = table.mode(m).map(|m| m.name.clone()).unwrap_or_default();
                let from = match from.and_then(|f| doc.get(f)) {
                    Some(f) if doc.is_page(f.id()) => "· from page".into(),
                    Some(f) => format!("· from {}", f.name()),
                    None => "· default".into(),
                };
                (name, Some(from), false)
            }
        };
        let open = self.mode_menu.is_some_and(|(n, _)| n == id);
        // **On an instance a mode the copy sets is an override** (D1038 (a), §15
        // D1048, D1042's *Fix*): the label bright with D981's dot, ↺ in its place
        // while the pointer is on the row, a click on it the main's mode back —
        // a field's mark, the label being this row's.
        let mark = self.sub_mark(
            &[id],
            |op| match op {
                ondin_core::Operation::SetMode { mode, .. } => Some(*mode),
                _ => None,
            },
            |n| Some(n.mode()),
            |v| *v,
            |c, v| *c = v,
            |id, mode| ondin_core::Operation::SetMode { id, mode },
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
            let (label, lresp) = ui.allocate_exact_size(egui::vec2(48.0, 28.0), sense);
            match &mark {
                Some(_) if over => {
                    ui.painter().text(
                        label.left_center() + egui::vec2(6.0, 0.0),
                        egui::Align2::CENTER_CENTER,
                        icon::ARROW_COUNTER_CLOCKWISE,
                        theme::icon_font(13.0),
                        color::TEXT,
                    );
                }
                Some(_) => {
                    let g = ui.painter().text(
                        label.left_center(),
                        egui::Align2::LEFT_CENTER,
                        "Mode",
                        egui::FontId::proportional(10.5),
                        theme::text::STRONG,
                    );
                    ui::override_dot(ui.painter(), egui::pos2(g.right() + 5.0, g.center().y));
                }
                None => {
                    ui.painter().text(
                        label.left_center(),
                        egui::Align2::LEFT_CENTER,
                        "Mode",
                        egui::FontId::proportional(10.5),
                        theme::text::DIM,
                    );
                }
            }
            if let Some(m) = &mark {
                reset = lresp.on_hover_text(&m.tip).clicked();
            }
            let w = ui.available_width();
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 28.0), egui::Sense::click());
            let p = ui.painter();
            let border = if open {
                color::ACCENT.gamma_multiply(0.65)
            } else if resp.hovered() {
                color::text_a(38)
            } else {
                color::text_a(20)
            };
            p.rect(
                rect,
                5,
                color::text_a(13),
                egui::Stroke::new(1.0, border),
                egui::StrokeKind::Inside,
            );
            let ink = if set { color::TEXT } else { color::text_a(128) };
            let galley = p.layout_no_wrap(value, egui::FontId::proportional(12.0), ink);
            let x = rect.left() + 9.0;
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
                    egui::FontId::proportional(10.5),
                    color::text_a(89),
                );
            }
            p.text(
                egui::pos2(rect.right() - 9.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                icon::CARET_DOWN,
                egui::FontId::new(12.0, egui::FontFamily::Name("phosphor".into())),
                theme::text::DIM,
            );
            let resp = resp.on_hover_text(match (page, set) {
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
                self.mode_menu = if open { None } else { Some((id, rect)) };
            }
        });
        self.reset_marked(reset, &mark);
    }

    /// The *Mode* menu: *Inherit* with the mode it would inherit, then every
    /// mode, the frame's own checked (H1).
    pub(crate) fn mode_menu_ui(&mut self, ctx: &egui::Context) {
        let Some((id, under)) = self.mode_menu else {
            return;
        };
        let doc = &self.session.doc;
        let Some(node) = doc.get(id) else {
            self.mode_menu = None;
            return;
        };
        let table = doc.variables();
        let modes: Vec<Mode> = table.modes().to_vec();
        let current = node.mode();
        let inherits = inherited_mode(doc, id)
            .and_then(|(m, _)| table.mode(m))
            .map(|m| m.name.clone());
        let w = under.width().max(200.0);
        let size = egui::vec2(w, (modes.len() as f32 + 1.0) * MENU_ROW_H + 18.0);
        let pos = ui::menu_place(ctx, under.left_bottom() + egui::vec2(0.0, 4.0), size);
        let mut pick: Option<Option<ModeId>> = None;
        let rect = egui::Area::new(egui::Id::new("frame-mode-menu"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                ui::menu_frame(4.0).show(ui, |ui| {
                    ui.set_width(w - 8.0);
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let inherit = ui::MenuRow::new(icon::ARROW_ELBOW_LEFT_UP, "Inherit")
                        .accel(inherits.as_deref())
                        .checked(current.is_none());
                    if ui::menu_row(ui, inherit, MENU_ROW_H).clicked() {
                        pick = Some(None);
                    }
                    ui::menu_sep(ui);
                    for m in &modes {
                        let row =
                            ui::MenuRow::new(icon::COLUMNS, &m.name).checked(current == Some(m.id));
                        if ui::menu_row(ui, row, MENU_ROW_H).clicked() {
                            pick = Some(Some(m.id));
                        }
                    }
                });
            })
            .response
            .rect;
        if let Some(mode) = pick {
            self.mode_menu = None;
            if mode != current {
                self.session.name_step("Mode", icon::COLUMNS);
                self.commit_edit(Transaction(vec![Operation::SetMode { id, mode }]));
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
        assert_eq!(table.modes().len(), 1);
        assert_eq!(table.modes()[0].name, "Mode 1");
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
        assert_eq!(app.session.doc.variables().modes().len(), 2);
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

    /// The browser and the insert picker share one place (G11): opening either
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
    /// cell opens only once the value has landed. Before, the commit and the
    /// next cell's edit were two acts, and the second replaced the refused field
    /// and cleared its sentence: the typed value vanished with nothing said
    /// (`arch-scribe`'s reading of the first build).
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
        let modes: Vec<ModeId> = app
            .session
            .doc
            .variables()
            .modes()
            .iter()
            .map(|m| m.id)
            .collect();
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

    /// **Deleting a used variable detaches its users and says so** (G10), and one
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

    /// **The *Styles* tab lists a style under its card and its path's group, and
    /// *Delete* detaches its users, says so, and undoes whole** (G8, D1038 (6);
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
        let mode = table.modes()[0].id;
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
        let modes: Vec<ModeId> = app
            .session
            .doc
            .variables()
            .modes()
            .iter()
            .map(|m| m.id)
            .collect();
        let (outer, inner) = (app.session.ids.mint(), app.session.ids.mint());
        let root = app.session.doc.first_page();
        let frame = |id, parent| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: ondin_core::NodeKind::Artboard {
                size: Size::new(50.0, 50.0),
            },
            transform: None,
            name: None,
        };
        assert!(app.commit_edit(Transaction(vec![frame(outer, root), frame(inner, outer)])));
        let doc = &app.session.doc;
        assert_eq!(
            inherited_mode(doc, inner),
            Some((modes[0], None)),
            "the default"
        );
        assert!(app.commit_edit(Transaction(vec![Operation::SetMode {
            id: outer,
            mode: Some(modes[1]),
        }])));
        let doc = &app.session.doc;
        assert_eq!(inherited_mode(doc, inner), Some((modes[1], Some(outer))));
        assert_eq!(doc.mode_at(inner), Some(modes[1]));
    }

    /// The panel is 340 wide with one mode and 436 with two (G2), and draws
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
    /// (B4's manner, D1038 G14; §15 D1053): *colour 2* renamed *colour 1* is
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
    /// Flip, run: without the write-through to `var_collapsed` fails *"kept for
    /// the document"*. ⚠️ **Its first run failed there with the write-through in
    /// place**: the browser loaded the document's set on its first *frame*, so the
    /// toggles made before it were overwritten — the load moved to the open
    /// (`toggle_var_browser`), the per-frame load kept for a document switch.
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
        app.var_act(Act::ToggleAll("brand".into()));
        assert!(
            collapsed(&app).contains("brand") && collapsed(&app).contains("surface"),
            "both closed: {:?}",
            collapsed(&app)
        );
        app.var_act(Act::ToggleAll("brand".into()));
        assert!(collapsed(&app).is_empty(), "both open again");
        app.var_act(Act::ToggleAll("surface".into()));
        frame(&mut app, &ctx);
        app.toggle_var_browser();
        app.toggle_var_browser();
        frame(&mut app, &ctx);
        assert!(
            collapsed(&app).contains("brand") && collapsed(&app).contains("surface"),
            "kept for the document"
        );
    }

    /// **Past three modes a bar under the columns scrolls them sideways** (G3, §15
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
        assert_eq!(app.session.doc.variables().modes().len(), 4, "the fixture");
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
        let modes: Vec<ModeId> = app
            .session
            .doc
            .variables()
            .modes()
            .iter()
            .map(|m| m.id)
            .collect();
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
        let mode = app.session.doc.variables().modes()[0].clone();
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
                        },
                        ondin_core::variable::KitMode {
                            id: night,
                            name: "Night".into(),
                            from: night,
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

        app.var_act(Act::GroupName("ui".into(), "theme".into()));
        assert_eq!(
            (name(&app, ids[0]), name(&app, ids[1])),
            ("theme/a".into(), "theme/b".into())
        );
        app.var_act(Act::GroupName("theme".into(), "a//b".into()));
        assert!(refusal(&app).is_some(), "an empty part is refused");
        assert_eq!(name(&app, ids[0]), "theme/a");
        app.var_act(Act::GroupName("theme".into(), "other".into()));
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
                .iter()
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
        let last = app.session.doc.variables().modes()[2].id;
        app.var_act(Act::MoveMode(last, 0));
        assert_eq!(
            modes(&app),
            ["Day copy", "Day", "Day copy 2"],
            "made default"
        );
        app.var_act(Act::MoveMode(last, 2));
        assert_eq!(modes(&app), ["Day", "Day copy 2", "Day copy"], "moved back");

        let copy = app.session.doc.variables().modes()[2].id;
        let frame = app.session.ids.mint();
        let root = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(50.0, 50.0),
                },
                transform: None,
                name: None,
            },
            Operation::SetMode {
                id: frame,
                mode: Some(copy),
            },
        ])));
        app.var_act(Act::DeleteMode(copy));
        assert_eq!(modes(&app), ["Day", "Day copy 2"]);
        assert_eq!(
            app.session.doc.get(frame).unwrap().mode(),
            None,
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
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ]);
        assert!(app.session.commit(tx));
        let members: Vec<VarId> = group_members(app.session.doc.variables(), "brand")
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
        let order: Vec<String> = all_lines(doc, "", None, &HashSet::new())
            .iter()
            .map(|l| match l {
                Line::Var(v) => v.name.clone(),
                Line::Kit(k) => format!("kit {k}"),
                Line::Group { path, .. } => format!("group {path}"),
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
}
