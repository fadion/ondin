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
//! names, each with its usage count and its cells, edited in place. The *Styles*
//! tab is the empty state until styles are built (roadmap step 3).

use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use crate::ui;
use eframe::egui;
use ondin_core::peniko::Color;
use ondin_core::variable::{self as var, Cell, Mode, ModeId, Value, VarId, VarKind, Variable};
use ondin_core::{Document, NodeId, Operation, Transaction};
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
}

/// The menu open over the browser, and where.
#[derive(Clone, Debug, PartialEq)]
enum Menu {
    /// The `+` button's: *Colour variable*, *Number variable*, *New mode* (G5).
    Add,
    Var(VarId),
    Mode(ModeId),
    Group(String),
    /// *Delete mode…*'s confirmation, with the frames it returns to *Inherit*.
    ConfirmMode(ModeId),
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
    /// Collapsed group paths. Kept for as long as the browser is open.
    collapsed: HashSet<String>,
    edit: Option<Edit>,
    /// Whether the field `edit` names still has to take the keyboard.
    edit_focus: bool,
    menu: Option<(Menu, egui::Pos2)>,
    /// The sentence a refused edit is answered with, until the next edit.
    refusal: Option<String>,
    /// How far the mode columns are scrolled sideways, past three modes (G3).
    scroll_x: f32,
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
    }

    /// Whether something inside the browser answers `Escape` before the browser
    /// does — a field being typed into, an open menu.
    pub(crate) fn escape(&mut self) -> bool {
        if self.menu.take().is_some() {
            return true;
        }
        if self.edit.take().is_some() {
            self.refusal = None;
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
    Delete(Vec<VarId>, String),
    DeleteMode(ModeId),
    Select(Vec<NodeId>),
    Toggle(String),
    StartEdit(Edit),
    OpenMenu(Menu, egui::Pos2),
}

/// One row of the table as drawn: a group's header or a variable.
enum Line<'a> {
    Group {
        path: &'a str,
        shown: usize,
        total: usize,
    },
    Var(&'a Variable),
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
                Some(Browser {
                    focus: true,
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
        let mut users: BTreeMap<VarId, Vec<NodeId>> = BTreeMap::new();
        for v in table.iter() {
            users.insert(v.id, var::users(doc, v.id));
        }
        let picked: Option<HashSet<VarId>> = b.in_selection.then(|| {
            let ids = ondin_core::subtree_nodes(doc, self.session.selection.ids());
            table
                .iter()
                .filter(|v| users[&v.id].iter().any(|u| ids.contains(u)))
                .map(|v| v.id)
                .collect()
        });
        let query = b.query.trim().to_lowercase();
        let lines = lines_of(table.iter(), &query, picked.as_ref(), &b.collapsed);
        let selection_line = b.in_selection.then(|| {
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
                    match picked.as_ref().map_or(0, HashSet::len) {
                        0 => format!("{who} uses no variables"),
                        1 => format!("Used by {who} · 1 variable"),
                        n => format!("Used by {who} · {n} variables"),
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
                            ui.add_space(-8.0);
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
                            Tab::Styles => empty(ui, "No styles yet", "Styles arrive with fill, stroke and effect styles."),
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
                                                Line::Group { path, shown, total } => group_row(
                                                    ui,
                                                    path,
                                                    *shown,
                                                    *total,
                                                    picked.is_some(),
                                                    &mut b,
                                                    &mut acts,
                                                ),
                                                Line::Var(v) => var_row(
                                                    ui, doc, v, &modes, &users[&v.id], &mut b,
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
        let over = ctx.pointer_hover_pos().is_some_and(|p| panel.contains(p));
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

        if let Some((menu, at)) = b.menu.clone() {
            self.var_menu(ctx, &mut b, menu, at, &users, &mut acts);
        }

        self.var_browser = Some(b);
        for act in acts {
            self.var_act(act);
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
            Menu::Var(_) => 4.0,
            Menu::Mode(_) => 3.0,
            Menu::Group(_) => 3.0,
            Menu::ConfirmMode(_) => 3.0,
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
                            // The last mode cannot go while there are variables:
                            // every variable has a value in every mode.
                            let last = table.modes().len() == 1 && table.iter().next().is_some();
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
                            let members: Vec<&Variable> =
                                table.iter().filter(|v| in_group(&v.name, path)).collect();
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
                }
            }
            Act::OpenMenu(menu, at) => {
                if let Some(b) = self.var_browser.as_mut() {
                    b.menu = Some((menu, at));
                }
            }
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
                };
                let taken: Vec<&str> = table.iter().map(|v| v.name.as_str()).collect();
                let name = free_name(&taken, stem, 1);
                let id = self.session.ids.mint_var();
                ops.push(Operation::AddVariable {
                    var: Variable {
                        id,
                        name: name.clone(),
                        kind,
                        cells: modes.into_iter().map(|m| (m, cell)).collect(),
                    },
                });
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
                let op = match table.default_mode() {
                    Some(first) => var::copy_mode_ops(doc, first, id, &name, table.modes().len()),
                    None => Some(Operation::AddMode {
                        mode: Mode {
                            id,
                            name: name.clone(),
                        },
                        index: 0,
                        cells: vec![],
                    }),
                };
                if let Some(op) = op
                    && self.commit_edit(Transaction(vec![op]))
                    && let Some(b) = self.var_browser.as_mut()
                {
                    b.edit = Some(Edit::Mode {
                        mode: id,
                        text: name,
                    });
                    b.edit_focus = true;
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
                if let Some(op) = var::copy_mode_ops(doc, from, id, &name, index + 1) {
                    self.commit_edit(Transaction(vec![op]));
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
                    let changed = table.mode(id).is_some_and(|m| m.name != text);
                    self.var_edit_done();
                    if changed {
                        self.commit_edit(Transaction(vec![Operation::SetModeName {
                            id,
                            name: text,
                        }]));
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
                let Some(v) = table.get(id) else { return };
                let taken: Vec<&str> = table.iter().map(|v| v.name.as_str()).collect();
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
        same || self.commit_edit(Transaction(vec![Operation::SetVariableCell {
            id,
            mode,
            cell,
        }]))
    }

    fn var_refuse(&mut self, why: String) {
        if let Some(b) = self.var_browser.as_mut() {
            b.refusal = Some(why);
            b.edit_focus = true;
        }
    }

    fn var_edit_done(&mut self) {
        if let Some(b) = self.var_browser.as_mut() {
            b.edit = None;
            b.refusal = None;
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

/// A variable's value as a sentence names it: `16`, `#8BA5E0`.
fn value_text(v: Value) -> String {
    match v {
        Value::Color(c) => format!("#{}", ui::hex_of(c)),
        Value::Number(x) => number_text(x),
    }
}

fn number_text(x: f64) -> String {
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

/// A typed cell: `--name` aliases another variable of the kind, a colour takes
/// a hex (its alpha too, eight digits), a number takes an expression.
fn parse_cell(
    table: &ondin_core::Variables,
    v: &Variable,
    mode: ModeId,
    text: &str,
) -> Result<Cell, String> {
    let text = text.trim();
    if let Some(name) = text.strip_prefix("--") {
        let name = name.trim();
        let Some(target) = table.by_name(name) else {
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
            .ok_or_else(|| "A colour is a hex, like 8BA5E0, or --name.".to_string()),
        VarKind::Number => crate::expr::eval(text)
            .filter(|x| x.is_finite())
            .map(Cell::Number)
            .ok_or_else(|| "A number, or --name.".to_string()),
    }
}

fn kind_word(k: VarKind) -> &'static str {
    match k {
        VarKind::Color => "colour",
        VarKind::Number => "number",
    }
}

/// `RGB`, `RRGGBB` or `RRGGBBAA`, a leading `#` optional — a colour variable
/// carries its own alpha (D1038), so the last pair is kept here where the
/// inspector's hex field drops it.
fn parse_colour(s: &str) -> Option<Color> {
    let [r, g, b] = ui::parse_hex(s)?;
    let digits = s.trim().trim_start_matches('#').trim();
    let a = if digits.len() == 8 {
        u8::from_str_radix(&digits[6..8], 16).ok()?
    } else {
        255
    };
    Some(Color::from_rgba8(r, g, b, a))
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
            });
        }
        // A search opens the groups it matches (D1038 G).
        if path.is_empty() || !query.is_empty() || !collapsed.contains(path) {
            out.extend(shown.into_iter().map(Line::Var));
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
        let toggle = square_button(ui, icon::SELECTION, b.in_selection)
            .on_hover_text("In selection: only the variables the selection uses");
        if toggle.clicked() {
            b.in_selection = !b.in_selection;
        }
        let add = square_button(ui, icon::PLUS, matches!(b.menu, Some((Menu::Add, _))))
            .on_hover_text("New variable or mode");
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
        if let Some(Edit::Mode { mode, text }) = b.edit.as_mut()
            && *mode == m.id
        {
            let r = cell.shrink2(egui::vec2(3.0, 2.0)).intersect(clip);
            let resp = ui.put(
                r,
                egui::TextEdit::singleline(text)
                    .id(id.with("edit"))
                    .frame(egui::Frame::NONE)
                    .font(egui::FontId::proportional(11.0)),
            );
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
#[allow(clippy::too_many_arguments)]
fn group_row(
    ui: &mut egui::Ui,
    path: &str,
    shown: usize,
    total: usize,
    filtered: bool,
    b: &mut Browser,
    acts: &mut Vec<Act>,
) {
    ui.add_space(2.0);
    let w = b.inner_w;
    let id = ui.id().with(("var-group", path));
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, GROUP_H), egui::Sense::click());
    if let Some(Edit::Group { path: p, text }) = b.edit.as_mut()
        && p == path
    {
        let r = egui::Rect::from_min_size(
            rect.min + egui::vec2(20.0, 2.0),
            egui::vec2(NAME_W - 26.0, GROUP_H - 4.0),
        );
        let resp = ui.put(
            r,
            egui::TextEdit::singleline(text)
                .id(id.with("edit"))
                .frame(egui::Frame::NONE)
                .font(egui::FontId::proportional(11.5)),
        );
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
    let open = !b.collapsed.contains(path) || !b.query.trim().is_empty();
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
    if ui::double_clicked(ui, &resp) {
        acts.push(Act::StartEdit(Edit::Group {
            path: path.to_string(),
            text: path.to_string(),
        }));
    } else if resp.clicked() {
        acts.push(Act::Toggle(path.to_string()));
    } else if resp.secondary_clicked()
        && let Some(at) = resp.interact_pointer_pos()
    {
        acts.push(Act::OpenMenu(
            Menu::Group(path.to_string()),
            at + egui::vec2(6.0, 8.0),
        ));
    }
}

/// A variable's row: its leaf name and usage count, then a cell per mode.
fn var_row(
    ui: &mut egui::Ui,
    doc: &Document,
    v: &Variable,
    modes: &[Mode],
    users: &[NodeId],
    b: &mut Browser,
    acts: &mut Vec<Act>,
) {
    let table = doc.variables();
    let w = b.inner_w;
    let id = ui.id().with(("var-row", v.id));
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, ROW_H), egui::Sense::click());
    let menu_open = matches!(&b.menu, Some((Menu::Var(m), _)) if *m == v.id);
    if menu_open {
        ui.painter().rect_filled(rect, 4, color::text_a(23));
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 4, color::text_a(18));
    }
    let name_rect = egui::Rect::from_min_size(rect.min, egui::vec2(NAME_W, ROW_H));

    // The name: the leaf, or the whole name while it is being renamed.
    let editing_name = matches!(&b.edit, Some(Edit::Name { var, .. }) if *var == v.id);
    if let Some(Edit::Name { var: _, text }) = b.edit.as_mut().filter(|_| editing_name) {
        let r = egui::Rect::from_min_size(
            rect.min + egui::vec2(INDENT - 6.0, 3.0),
            egui::vec2(NAME_W - INDENT, ROW_H - 6.0),
        );
        let field = ui.put(
            r,
            egui::TextEdit::singleline(text)
                .id(id.with("name"))
                .frame(egui::Frame::NONE)
                .font(egui::FontId::proportional(11.5))
                .hint_text("name, / to group"),
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
        let name_resp = ui.interact(name_rect, id.with("name-hit"), egui::Sense::click());
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
            let field = ui.put(
                r.shrink2(egui::vec2(4.0, 0.0)),
                egui::TextEdit::singleline(text)
                    .id(id.with(("cell", m.id)))
                    .frame(egui::Frame::NONE)
                    .font(egui::FontId::proportional(11.0)),
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
                acts.push(Act::Cell(v.id, m.id, text.clone(), next));
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
        draw_cell(&p, table, cell, c);
        if cell_resp.clicked() {
            acts.push(Act::StartEdit(Edit::Cell {
                var: v.id,
                mode: m.id,
                text: cell_text(table, c),
            }));
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

/// A 12×12 colour chip, the checkerboard under a translucent one (G1).
fn swatch(p: &egui::Painter, rect: egui::Rect, c: Color) {
    let painter = p.with_clip_rect(rect.intersect(p.clip_rect()));
    ui::paint_checkerboard(&painter, rect, 3.0);
    painter.rect_filled(rect, 0, ui::color_to_egui(c));
    painter.rect_stroke(
        rect,
        3,
        egui::Stroke::new(1.0, egui::Color32::from_black_alpha(90)),
        egui::StrokeKind::Inside,
    );
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
            v.cells.values().copied().collect::<Vec<_>>(),
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

    /// A cell takes a hex — its alpha too, a colour variable carrying its own
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
        let root = app.session.doc.root();
        assert!(app.commit_edit(Transaction(vec![
            Operation::CreateNode {
                id: rect,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::default(),
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

    /// The panel is 340 wide with one mode and 436 with two (G2), and draws
    /// without a panic over a table, a group and an alias.
    #[test]
    fn the_panel_is_as_wide_as_its_modes() {
        let (ctx, mut app) = app();
        app.toggle_var_browser();
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        // The panel's box, without its hairline: the frame strokes a point
        // outside it on each side, where the mockup's `0 0 0 1px` ring sits.
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
}
