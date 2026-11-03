//! Colour review (View menu, palette): a card at the command palette's place over
//! the canvas that lists the near-identical colours of the selection or the
//! document (`ondin_core::color_review`) and merges each group into one colour,
//! or into a new variable, as one undo step.
//!
//! Not a modal: the canvas stays live, the groups are found again as the
//! document, the selection or the threshold changes, and a member's reticle
//! selects that colour's users.

use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use crate::ui::{self, FieldButton};
use eframe::egui;
use ondin_core::color_review::{self, Group, NewVar, Swatch, Threshold};
use ondin_core::peniko::Color;
use ondin_core::{Document, NodeId, VarId};
use std::collections::HashMap;

/// The card's width: room for a member row's name, uses, distance and reticle.
const W: f32 = 420.0;
/// The card's inset, and the title row's height: batch rename's.
const PAD: f32 = 14.0;
const TITLE_H: f32 = 40.0;
/// The foot holding the merge: batch rename's.
const FOOT_H: f32 = 48.0;
/// The list's inset inside the card; a row's ground spans it, and its content
/// sits [`ROW_X`] further in, level with the controls above.
const LIST_X: f32 = PAD - ROW_X;
const ROW_X: f32 = 6.0;
/// A group's folded row, and a member's row inside it. Each paints a list row's
/// [`ui::MENU_ROW_H`] ground at its middle; the rest is air between rows.
const HEAD_H: f32 = 30.0;
const MEMBER_H: f32 = 26.0;
/// The split patch that shows a group's members, one slice each.
const PATCH: egui::Vec2 = egui::vec2(42.0, 20.0);
const SWATCH: f32 = 16.0;
/// A member row's columns after the label: uses, distance, reticle.
const USES_W: f32 = 52.0;
const DELTA_W: f32 = 40.0;
const BTN: f32 = 26.0;
const COL_GAP: f32 = 8.0;
/// Where a head's patch, and a member's check, start.
const INDENT: f32 = 21.0;
/// The caret menu's width.
const MENU_W: f32 = 230.0;
const NAME_ID: &str = "colour-review-name";

/// Which colour a member is, across frames: a raw RGBA or a variable at a
/// binding alpha — what keeps a group open and its target picked while the
/// census is taken again.
type Key = (Option<VarId>, [u8; 4]);

fn key(s: &Swatch) -> Key {
    (s.var, s.color.to_rgba8().to_u8_array())
}

/// Which group is unfolded.
#[derive(Clone, Copy, Default, PartialEq)]
enum Unfolded {
    /// The first, as the card opens and after a merge.
    #[default]
    First,
    Group(Key),
    None,
}

/// The card's state; kept closed rather than dropped, so it reopens on the
/// merge last chosen.
#[derive(Default)]
pub(crate) struct ColourReview {
    pub(crate) open: bool,
    /// Review the selection; with nothing selected the document is reviewed.
    selection: bool,
    unfolded: Unfolded,
    /// The target picked in each group, by its anchor.
    targets: HashMap<Key, Key>,
    /// The merge button's last choice: into a new variable rather than the target.
    new_var: bool,
    /// The caret's menu is open.
    menu: bool,
    /// The footer is naming a new variable: the name so far, and whether the
    /// field takes the keyboard next frame.
    naming: Option<(String, bool)>,
    seen: Seen,
}

/// The census and groups for one key, taken again only when it changes.
#[derive(Default)]
struct Seen {
    key: Option<(u64, Vec<NodeId>, Threshold)>,
    roots: Vec<NodeId>,
    census: Vec<Swatch>,
    groups: Vec<Group>,
}

impl ColourReview {
    /// The `Escape` rung: give up naming first, then close. False when it closed.
    pub(crate) fn escape(&mut self) -> bool {
        if self.menu {
            self.menu = false;
            true
        } else if self.naming.take().is_some() {
            true
        } else {
            self.open = false;
            false
        }
    }

    /// How many groups the card lists.
    #[cfg(test)]
    pub(crate) fn groups_shown(&self) -> usize {
        self.seen.groups.len()
    }

    fn target(&self, g: &Group) -> usize {
        let census = &self.seen.census;
        let anchor = key(&census[g.members[0]]);
        self.targets
            .get(&anchor)
            .and_then(|t| g.members.iter().position(|&m| key(&census[m]) == *t))
            .unwrap_or_else(|| g.default_target(census))
    }

    fn unfolded(&self) -> Option<usize> {
        match self.unfolded {
            Unfolded::First => (!self.seen.groups.is_empty()).then_some(0),
            Unfolded::Group(k) => self
                .seen
                .groups
                .iter()
                .position(|g| key(&self.seen.census[g.members[0]]) == k),
            Unfolded::None => None,
        }
    }
}

/// How a member is named: its hex, or its variable, a kit's with the kit's name.
enum Name {
    Hex { hex: String, alpha: Option<u8> },
    Var { name: String, kit: Option<String> },
}

fn name_of(doc: &Document, s: &Swatch) -> Name {
    let t = doc.variables();
    match s.var.and_then(|v| t.get(v)) {
        Some(v) => Name::Var {
            name: v.name.clone(),
            // Masked in private mode, as every kit surface's is.
            kit: v.kit().map(|k| crate::panels::binding::kit_name(doc, k)),
        },
        None => {
            let a = s.color.to_rgba8().to_u8_array()[3];
            Name::Hex {
                hex: ui::hex_of(s.color),
                alpha: (a < 255).then(|| (f32::from(a) / 2.55).round() as u8),
            }
        }
    }
}

/// The word a merge button names its target by.
fn short_name(doc: &Document, s: &Swatch) -> String {
    match name_of(doc, s) {
        Name::Hex { hex, .. } => hex,
        Name::Var { name, .. } => name,
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    match n {
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    }
}

/// What a frame of the card asked for.
enum Act {
    Close,
    Scope(bool),
    Threshold(Threshold),
    Fold(usize),
    Target(usize, usize),
    Select(Vec<NodeId>),
    Merge(usize),
    Menu,
    Choose(bool),
    Name(String),
    CancelName,
    Create(usize),
}

impl OndinApp {
    /// Open the card, on the selection when there is one. Find closes: centred, the
    /// card would run under Find's corner on any canvas short of a wide one.
    pub(crate) fn open_colour_review(&mut self) {
        self.close_find();
        let r = &mut self.colour_review;
        if !r.open {
            r.selection = !self.session.selection.ids().is_empty();
            r.unfolded = Unfolded::First;
            r.naming = None;
            r.menu = false;
        }
        r.open = true;
    }

    /// Take the census again when the document, the scope or the threshold moved.
    fn refresh_colour_review(&mut self) {
        let selection = self.session.selection.ids().to_vec();
        let on_selection = self.colour_review.selection && !selection.is_empty();
        let key = (
            self.session.revision(),
            if on_selection {
                selection.clone()
            } else {
                Vec::new()
            },
            self.prefs.colour_review,
        );
        let seen = &mut self.colour_review.seen;
        if seen.key.as_ref() == Some(&key) {
            return;
        }
        let doc = &self.session.doc;
        seen.roots = color_review::roots(doc, on_selection.then_some(selection.as_slice()));
        seen.census = color_review::census(doc, &seen.roots);
        seen.groups = color_review::groups(&seen.census, key.2);
        seen.key = Some(key);
    }

    /// Draw the card when it is open, and act on this frame's clicks.
    pub(crate) fn colour_review_ui(&mut self, ctx: &egui::Context) {
        if !self.colour_review.open {
            return;
        }
        self.refresh_colour_review();
        let canvas = self.canvas_rect;
        let room = ui::canvas_popup_room(canvas);
        let selected = !self.session.selection.ids().is_empty();
        let threshold = self.prefs.colour_review;
        let doc = &self.session.doc;
        let r = &mut self.colour_review;
        let mut act = None;
        ui::canvas_popup_area(ctx, egui::Id::new("colour-review"), canvas).show(ctx, |ui| {
            crate::settings::card_frame().show(ui, |ui| {
                ui.set_width(W);
                ui.set_max_height(room);
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                if head(ui) {
                    act = Some(Act::Close);
                }
                egui::Frame::NONE
                    .inner_margin(egui::Margin {
                        left: PAD as i8,
                        right: PAD as i8,
                        top: PAD as i8,
                        bottom: 4,
                    })
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 10.0;
                        controls(ui, r, selected, threshold, &mut act);
                        caption(ui, r, selected, threshold);
                    });
                ui.add_space(8.0);
                divider(ui);
                if r.seen.groups.is_empty() {
                    empty(ui, r, threshold);
                    return;
                }
                // The title, the controls and the foot keep their place; the
                // list gives up what the canvas lacks.
                let list_h = ui::CANVAS_POPUP_LIST_H
                    .min(ui.available_height() - FOOT_H - 1.0)
                    .max(HEAD_H * 3.0);
                list(ui, doc, r, list_h, &mut act);
                divider(ui);
                foot(ui, doc, r, &mut act);
            });
        });
        self.colour_review_act(ctx, act);
    }

    fn colour_review_act(&mut self, ctx: &egui::Context, act: Option<Act>) {
        let r = &mut self.colour_review;
        match act {
            None => {}
            Some(Act::Close) => r.open = false,
            Some(Act::Scope(selection)) => r.selection = selection,
            Some(Act::Threshold(t)) => {
                if self.prefs.colour_review != t {
                    self.prefs.colour_review = t;
                    self.prefs.save();
                }
                r.unfolded = Unfolded::First;
            }
            Some(Act::Fold(g)) => {
                let k = key(&r.seen.census[r.seen.groups[g].members[0]]);
                let open = r.unfolded() == Some(g);
                r.unfolded = if open {
                    Unfolded::None
                } else {
                    Unfolded::Group(k)
                };
                r.menu = false;
                r.naming = None;
            }
            Some(Act::Target(g, i)) => {
                let group = &r.seen.groups[g];
                let anchor = key(&r.seen.census[group.members[0]]);
                let target = key(&r.seen.census[group.members[i]]);
                r.targets.insert(anchor, target);
                // A bound target has a variable already; the button says so again.
                if !r.seen.census[group.members[i]].is_raw() {
                    r.naming = None;
                }
            }
            Some(Act::Select(ids)) => self.select_colour_users(ids),
            Some(Act::Merge(g)) => {
                if r.new_var {
                    r.naming = Some(("color/".into(), true));
                } else {
                    self.merge_group(g, None);
                }
            }
            Some(Act::Menu) => r.menu = !r.menu,
            Some(Act::Choose(new_var)) => {
                r.menu = false;
                r.new_var = new_var;
                r.naming = new_var.then(|| ("color/".into(), true));
            }
            Some(Act::Name(n)) => {
                if let Some((name, _)) = &mut r.naming {
                    *name = n;
                }
            }
            Some(Act::CancelName) => r.naming = None,
            Some(Act::Create(g)) => {
                let name = r.naming.as_ref().map(|(n, _)| n.trim().to_string());
                if let Some(name) = name
                    && crate::panels::variables::name_refusal(
                        self.session.doc.variables(),
                        &name,
                        None,
                    )
                    .is_none()
                {
                    self.merge_group(g, Some(name));
                }
            }
        }
        ctx.request_repaint();
    }

    /// Merge group `g` into its target, or into a new variable called `new`.
    fn merge_group(&mut self, g: usize, new: Option<String>) {
        let r = &self.colour_review;
        let Some(group) = r.seen.groups.get(g).cloned() else {
            return;
        };
        let census = r.seen.census.clone();
        let roots = r.seen.roots.clone();
        let target = r.target(&group);
        let doc = &self.session.doc;
        let (merged, colours, into) = match &new {
            Some(name) => {
                let id = self.session.ids.mint_var();
                let first_mode = self.session.ids.mint_mode();
                let colours = group
                    .members
                    .iter()
                    .filter(|&&m| census[m].is_raw())
                    .count();
                let merged = color_review::merge_into_new(
                    doc,
                    &roots,
                    &census,
                    &group,
                    target,
                    NewVar {
                        id,
                        name,
                        first_mode,
                    },
                );
                (merged, colours, name.clone())
            }
            None => {
                let colours = group.merged(&census, target).len();
                let merged = color_review::merge(doc, &roots, &census, &group, target);
                let into = short_name(doc, &census[group.members[target]]);
                (merged, colours, into)
            }
        };
        if merged.tx.0.is_empty() {
            if merged.locked > 0 {
                self.session.info(format!(
                    "Nothing merged · {} on locked layers",
                    plural(merged.locked, "use is", "uses are")
                ));
            }
            return;
        }
        let (label, glyph) = match new {
            Some(_) => ("Merge into a new variable", icon::BRACKETS_CURLY),
            None => ("Merge colours", icon::SWATCHES),
        };
        self.session.name_step(label, glyph);
        if !self.commit_edit(merged.tx) {
            return;
        }
        let mut said = format!(
            "Merged {} into {into} · {} on {}",
            plural(colours, "colour", "colours"),
            plural(merged.uses, "use", "uses"),
            plural(merged.layers, "layer", "layers"),
        );
        if merged.unbound > 0 {
            said.push_str(&format!(
                " · {} took its value, as stops, shadows and text colours do not bind",
                merged.unbound
            ));
        }
        if merged.locked > 0 {
            said.push_str(&format!(" · {} on locked layers left", merged.locked));
        }
        self.session.info(said);
        let r = &mut self.colour_review;
        r.naming = None;
        r.menu = false;
        r.unfolded = Unfolded::First;
    }

    /// A member's reticle: select its users on this page, or on the page of the
    /// first of them when none is here.
    fn select_colour_users(&mut self, ids: Vec<NodeId>) {
        let doc = &self.session.doc;
        let here: Vec<NodeId> = ids
            .iter()
            .copied()
            .filter(|&id| doc.page_of(id) == Some(self.session.page))
            .collect();
        let ids = if here.is_empty() {
            let Some(page) = ids.first().and_then(|&id| doc.page_of(id)) else {
                return;
            };
            let there: Vec<NodeId> = ids
                .into_iter()
                .filter(|&id| doc.page_of(id) == Some(page))
                .collect();
            self.go_to_page(page);
            there
        } else {
            here
        };
        self.select_by_color(ids);
    }
}

/// The title row, batch rename's ([`crate::settings::modal_title`] inside
/// [`PAD`]); true when its close button is clicked.
fn head(ui: &mut egui::Ui) -> bool {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(W, TITLE_H), egui::Sense::hover());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(PAD, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    crate::settings::modal_title(&mut child, "Colour review")
}

/// A hairline across the whole card, between its parts.
fn divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(W, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, color::divider());
}

/// The scope and the threshold, side by side.
fn controls(
    ui: &mut egui::Ui,
    r: &ColourReview,
    selected: bool,
    threshold: Threshold,
    act: &mut Option<Act>,
) {
    const SCOPES: [&str; 2] = ["Selection", "Document"];
    let gap = 7.0;
    let total = ui.available_width() - gap;
    // The threshold's three cells against the scope's two, as wide as each other.
    let scope_w = (total / 2.4).floor();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        let lit = usize::from(!(r.selection && selected));
        if let Some(i) = ui::segmented_tipped(
            ui,
            scope_w,
            ui::SEGMENT_CELL_H,
            2,
            lit,
            |i| i != 0 || selected,
            |i| match i {
                0 if !selected => "Nothing is selected",
                _ => "",
            },
            |p, i, rect, on| match (i, selected) {
                (0, false) => ui::segment_label_disabled(p, rect, SCOPES[0]),
                _ => ui::segment_label(p, rect, SCOPES[i], on),
            },
        ) {
            *act = Some(Act::Scope(i == 0));
        }
        let lit = Threshold::ALL
            .iter()
            .position(|t| *t == threshold)
            .unwrap_or(1);
        if let Some(i) = ui::segmented_tipped(
            ui,
            total - scope_w,
            ui::SEGMENT_CELL_H,
            3,
            lit,
            |_| true,
            |i| match Threshold::ALL[i] {
                Threshold::Tight => "Only colours a hair apart: Δ 1 or less",
                Threshold::Close => "Colours that read as one: Δ 2 or less",
                Threshold::Loose => "Colours a little apart too: Δ 4 or less",
            },
            |p, i, rect, on| ui::segment_label(p, rect, Threshold::ALL[i].label(), on),
        ) {
            *act = Some(Act::Threshold(Threshold::ALL[i]));
        }
    });
}

/// What the list holds, in a line; and the fallback to the document said.
fn caption(ui: &mut egui::Ui, r: &ColourReview, selected: bool, threshold: Threshold) {
    let line = |ui: &mut egui::Ui, text: String| {
        ui.label(
            egui::RichText::new(text)
                .size(10.5)
                .color(theme::text::faint()),
        );
    };
    if r.selection && !selected {
        line(ui, "Nothing selected · reviewing the document".into());
    }
    let groups = &r.seen.groups;
    if groups.is_empty() {
        return;
    }
    let colours: usize = groups.iter().map(|g| g.members.len()).sum();
    line(
        ui,
        format!(
            "{} · {colours} colours could be {} · Δ ≤ {}",
            plural(groups.len(), "group", "groups"),
            groups.len(),
            threshold.limit()
        ),
    );
}

/// No group: the threshold named and what was counted, so a looser one is the
/// obvious next try.
fn empty(ui: &mut egui::Ui, r: &ColourReview, threshold: Threshold) {
    let n = r.seen.census.len();
    let place = if r.seen.key.as_ref().is_some_and(|k| !k.1.is_empty()) {
        "the selection"
    } else {
        "the document"
    };
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: PAD as i8,
            right: PAD as i8,
            top: 14,
            bottom: 18,
        })
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.label(
                    egui::RichText::new(format!(
                        "No near-identical colours at {}",
                        threshold.label()
                    ))
                    .size(12.0)
                    .color(theme::text::muted()),
                );
                let counted = match n {
                    0 => format!("No colours in {place}"),
                    1 => format!("1 colour in {place}"),
                    n => format!("{n} colours in {place}, each one apart"),
                };
                ui.label(
                    egui::RichText::new(counted)
                        .size(11.0)
                        .color(theme::text::faint()),
                );
            });
        });
}

/// The groups, the unfolded one with its members, scrolling past `max_h`.
fn list(ui: &mut egui::Ui, doc: &Document, r: &ColourReview, max_h: f32, act: &mut Option<Act>) {
    let unfolded = r.unfolded();
    egui::ScrollArea::vertical()
        .id_salt("colour-review-list")
        .auto_shrink([false, true])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .max_height(max_h)
        .show(ui, |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(LIST_X as i8, 6))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                    for g in 0..r.seen.groups.len() {
                        let open = unfolded == Some(g);
                        if group_head(ui, doc, r, g, open).clicked() {
                            *act = Some(Act::Fold(g));
                        }
                        if open {
                            members(ui, doc, r, g, act);
                            ui.add_space(4.0);
                        }
                    }
                });
        });
}

/// A row's ground: a list row's height at the row's middle.
fn ground(row: egui::Rect) -> egui::Rect {
    egui::Rect::from_center_size(row.center(), egui::vec2(row.width(), ui::MENU_ROW_H))
}

/// One slice per member, in member order, in a rounded patch.
fn paint_patch(p: &egui::Painter, rect: egui::Rect, colours: &[Color]) {
    ui::paint_checkerboard(p, rect, 4.0);
    let n = colours.len().max(1) as f32;
    let w = (rect.width() - (n - 1.0)) / n;
    for (i, c) in colours.iter().enumerate() {
        let x = rect.left() + i as f32 * (w + 1.0);
        let slice =
            egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(w, rect.height()));
        p.rect_filled(slice, 0.0, ui::color_to_egui(*c));
    }
    ui::mask_corners(p, rect, 4.0, color::card());
    p.rect_stroke(
        rect,
        egui::CornerRadius::same(4),
        egui::Stroke::new(1.0, ui::swatch_hairline()),
        egui::StrokeKind::Inside,
    );
}

/// A name drawn at `x` on `y`, ending before `right`: a hex with its alpha, or a
/// variable's capsule, a kit's after the package glyph. `kit_name` says the kit's
/// name too, as a member row does. Returns where it ends.
fn paint_name(ui: &egui::Ui, name: &Name, x: f32, y: f32, right: f32, kit_name: bool) -> f32 {
    let p = ui.painter();
    match name {
        Name::Hex { hex, alpha } => {
            let g = p.layout_no_wrap(hex.clone(), egui::FontId::proportional(12.0), color::text());
            let w = g.size().x;
            p.galley(egui::pos2(x, y - g.size().y / 2.0), g, color::text());
            let mut end = x + w;
            if let Some(a) = alpha {
                let g = p.layout_no_wrap(
                    format!("{a}%"),
                    egui::FontId::proportional(11.0),
                    theme::text::dim(),
                );
                let ax = end + 9.0;
                end = ax + g.size().x;
                p.galley(egui::pos2(ax, y - g.size().y / 2.0), g, theme::text::dim());
            }
            end
        }
        Name::Var { name, kit } => {
            let mut x = x;
            if let Some(k) = kit {
                p.text(
                    egui::pos2(x + 7.0, y),
                    egui::Align2::CENTER_CENTER,
                    icon::PACKAGE,
                    theme::icon_font(14.0),
                    theme::text::dim(),
                );
                x += 14.0 + 6.0;
                if kit_name {
                    let shown = format!("{k} · {name}");
                    return crate::panels::binding::capsule(
                        ui,
                        egui::pos2(x, y),
                        &shown,
                        right - x,
                    )
                    .right();
                }
            }
            crate::panels::binding::capsule(ui, egui::pos2(x, y), name, right - x).right()
        }
    }
}

/// A group's folded row: the caret, the split patch, the target's name and the
/// count. Clicked, it folds or unfolds.
fn group_head(
    ui: &mut egui::Ui,
    doc: &Document,
    r: &ColourReview,
    g: usize,
    open: bool,
) -> egui::Response {
    let group = &r.seen.groups[g];
    let census = &r.seen.census;
    let (row, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HEAD_H),
        egui::Sense::click(),
    );
    let p = ui.painter();
    ui::list_row_ground(p, ground(row), false, resp.hovered());
    let rect = row.shrink2(egui::vec2(ROW_X, 0.0));
    let y = rect.center().y;
    p.text(
        egui::pos2(rect.left() + 6.0, y),
        egui::Align2::CENTER_CENTER,
        if open {
            icon::CARET_DOWN
        } else {
            icon::CARET_RIGHT
        },
        theme::icon_font(12.0),
        theme::text::faint(),
    );
    let patch =
        egui::Rect::from_min_size(egui::pos2(rect.left() + INDENT, y - PATCH.y / 2.0), PATCH);
    let colours: Vec<Color> = group.members.iter().map(|&m| census[m].color).collect();
    paint_patch(p, patch, &colours);
    let count = format!(
        "{} · {}",
        plural(group.members.len(), "colour", "colours"),
        plural(group.uses(census), "use", "uses")
    );
    let cg = p.layout_no_wrap(
        count,
        egui::FontId::proportional(11.0),
        theme::text::faint(),
    );
    let cx = rect.right() - cg.size().x;
    p.galley(
        egui::pos2(cx, y - cg.size().y / 2.0),
        cg,
        theme::text::faint(),
    );
    let target = &census[group.members[r.target(group)]];
    paint_name(
        ui,
        &name_of(doc, target),
        patch.right() + 9.0,
        y,
        cx - 9.0,
        false,
    );
    resp
}

/// The unfolded group's members, one a click picks as the target, lit and
/// checked as a picker's current row is: the swatch, the name, uses, the
/// distance from the anchor and the reticle.
fn members(ui: &mut egui::Ui, doc: &Document, r: &ColourReview, g: usize, act: &mut Option<Act>) {
    let group = &r.seen.groups[g];
    let census = &r.seen.census;
    let target = r.target(group);
    let anchor = census[group.members[0]].color;
    for (i, &m) in group.members.iter().enumerate() {
        let s = &census[m];
        let (row, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), MEMBER_H),
            egui::Sense::click(),
        );
        // `contains_pointer`: the reticle on top takes the hover from the row.
        let hovered = resp.contains_pointer();
        ui::list_row_ground(ui.painter(), ground(row), i == target, hovered);
        if i != target && resp.on_hover_text("Merge into this colour").clicked() {
            *act = Some(Act::Target(g, i));
        }
        let rect = row.shrink2(egui::vec2(ROW_X, 0.0));
        let y = rect.center().y;
        let mut x = rect.left() + INDENT;
        let cell = |x: f32, w: f32| {
            egui::Rect::from_min_size(egui::pos2(x, y - w / 2.0), egui::vec2(w, w))
        };
        if i == target {
            ui.painter().text(
                cell(x, SWATCH).center(),
                egui::Align2::CENTER_CENTER,
                icon::CHECK,
                theme::icon_font(13.0),
                color::accent(),
            );
        }
        x += SWATCH + COL_GAP;
        paint_patch(ui.painter(), cell(x, SWATCH), &[s.color]);
        x += SWATCH + COL_GAP;
        let reticle_x = rect.right() - BTN;
        let delta_x = reticle_x - COL_GAP - DELTA_W;
        let uses_x = delta_x - COL_GAP - USES_W;
        let end = paint_name(ui, &name_of(doc, s), x, y, uses_x - COL_GAP, true);
        let p = ui.painter();
        // A bound colour the merge leaves as it is.
        if i != target && !s.is_raw() {
            p.text(
                egui::pos2(end + 7.0, y),
                egui::Align2::LEFT_CENTER,
                "kept",
                egui::FontId::proportional(11.0),
                theme::text::faint(),
            );
        }
        p.text(
            egui::pos2(uses_x + USES_W, y),
            egui::Align2::RIGHT_CENTER,
            plural(s.uses, "use", "uses"),
            egui::FontId::proportional(11.0),
            theme::text::faint(),
        );
        let delta = match i {
            0 => "anchor".to_string(),
            _ => format!("Δ {:.1}", color_review::delta(anchor, s.color)),
        };
        p.text(
            egui::pos2(delta_x + DELTA_W, y),
            egui::Align2::RIGHT_CENTER,
            delta,
            egui::FontId::proportional(11.0),
            theme::text::faint(),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(cell(reticle_x, BTN)));
        let tip = match s.nodes.len() {
            1 => "Select the layer using this colour".to_string(),
            n => format!("Select the {n} layers using this colour"),
        };
        if ui::icon_button(&mut child, icon::SCAN, BTN, 15.0, false, true)
            .on_hover_text(tip)
            .clicked()
        {
            *act = Some(Act::Select(s.nodes.clone()));
        }
    }
}

/// The card's foot, batch rename's: what the merge writes on the left and a
/// split button merging the unfolded group on the right, its caret holding the
/// other merge; or, while a new variable is being named, the name field.
fn foot(ui: &mut egui::Ui, doc: &Document, r: &mut ColourReview, act: &mut Option<Act>) {
    let Some(g) = r.unfolded() else {
        let mut buttons = foot_row(ui, "Unfold a group to merge it");
        crate::settings::text_button(&mut buttons, "Merge", FieldButton::Disabled);
        return;
    };
    let group = &r.seen.groups[g];
    let census = &r.seen.census;
    let target = r.target(group);
    let t = &census[group.members[target]];
    let bound = !t.is_raw();
    let merged = group.merged(census, target).len();
    let raw: Vec<&Swatch> = group
        .members
        .iter()
        .map(|&m| &census[m])
        .filter(|s| s.is_raw())
        .collect();
    let uses: usize = raw.iter().map(|s| s.uses).sum();
    if let Some((name, focus)) = r.naming.as_mut().filter(|_| !bound) {
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(PAD as i8, 12))
            .show(ui, |ui| {
                naming(ui, doc, t, name, focus, (uses, raw.len()), g, act)
            });
        return;
    }
    let target_name = short_name(doc, t);
    let new_var = r.new_var && !bound;
    let (label, hint) = if new_var {
        (
            format!("Merge {} into a new variable…", raw.len()),
            format!("Binds {}", plural(uses, "use", "uses")),
        )
    } else {
        let rewrites: usize = group
            .merged(census, target)
            .iter()
            .map(|&m| census[m].uses)
            .sum();
        (
            format!("Merge {merged} into {target_name}"),
            format!("Rewrites {}", plural(rewrites, "use", "uses")),
        )
    };
    let live = if new_var { !raw.is_empty() } else { merged > 0 };
    let mut buttons = foot_row(ui, if live { &hint } else { "" });
    let button = ui::split_button(&mut buttons, &label, ui::CONTROL_H, live);
    if button.action && live {
        *act = Some(Act::Merge(g));
    }
    if button.caret {
        *act = Some(Act::Menu);
    }
    let on_caret = button
        .resp
        .hover_pos()
        .is_some_and(|p| button.caret_rect.contains(p));
    let caret_rect = button.caret_rect;
    let resp = button.resp;
    let resp = match (on_caret, live) {
        (true, _) => resp.on_hover_text("More merge options"),
        (false, false) => {
            resp.on_hover_text("The other colours are variables, which the merge keeps")
        }
        (false, true) => resp,
    };
    if !r.menu {
        return;
    }
    // Under the caret, right edges together; above it when the canvas is short.
    let id = egui::Id::new("colour-review-menu");
    let size = ui
        .ctx()
        .memory(|m| m.area_rect(id))
        .map_or(egui::vec2(MENU_W, 64.0), |r| r.size());
    let under = egui::Rect::from_min_max(
        egui::pos2(caret_rect.right() - size.x, caret_rect.top()),
        caret_rect.max,
    );
    let menu = egui::Area::new(id)
        .order(egui::Order::Foreground)
        .fixed_pos(ui::dropdown_place(ui.ctx(), under, size, ui::MENU_GAP))
        .show(ui.ctx(), |ui| {
            ui::menu_frame(5.0).show(ui, |ui| {
                ui.set_width(ui::menu_inner_w(MENU_W, 5.0));
                ui.spacing_mut().item_spacing.y = 1.0;
                let into = format!("Merge into {target_name}");
                let row = ui::MenuRow::new("", &into).checked(!new_var);
                if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() {
                    *act = Some(Act::Choose(false));
                }
                let row = ui::MenuRow::new("", "Merge into a new variable…")
                    .checked(new_var)
                    .enabled(!bound)
                    .accel(bound.then_some("The group has a variable"));
                if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() && !bound {
                    *act = Some(Act::Choose(true));
                }
            });
        })
        .response;
    let outside = ui.ctx().input(|i| i.pointer.any_click())
        && !menu.contains_pointer()
        && !resp.contains_pointer();
    if outside && act.is_none() {
        r.menu = false;
    }
}

/// The foot's row: `hint` on the left, and the buttons' `Ui`, laid right to left.
fn foot_row(ui: &mut egui::Ui, hint: &str) -> egui::Ui {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(W, FOOT_H), egui::Sense::hover());
    ui.painter().text(
        egui::pos2(rect.left() + PAD, rect.center().y),
        egui::Align2::LEFT_CENTER,
        hint,
        egui::FontId::proportional(10.5),
        theme::text::faint(),
    );
    ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(PAD, 10.0)))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    )
}

/// Naming the new variable `t`'s colour goes into: the field, *Cancel* and
/// *Create and merge*, between what it will be and what it will bind — `(uses,
/// colours)` — or why the name is refused.
#[allow(clippy::too_many_arguments)]
fn naming(
    ui: &mut egui::Ui,
    doc: &Document,
    t: &Swatch,
    name: &mut String,
    focus: &mut bool,
    (uses, colours): (usize, usize),
    g: usize,
    act: &mut Option<Act>,
) {
    ui.spacing_mut().item_spacing.y = 7.0;
    ui.label(
        egui::RichText::new(format!(
            "New colour variable · value {} in every mode",
            ui::hex_of(t.color)
        ))
        .size(10.5)
        .color(theme::text::faint()),
    );
    let refusal = crate::panels::variables::name_refusal(doc.variables(), name.trim(), None);
    let mut typed = name.clone();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 7.0;
        let cancel_w = crate::find::button_w(ui, "Cancel");
        let create_w = crate::find::button_w(ui, "Create and merge");
        let w = ui.available_width() - cancel_w - create_w - 14.0;
        let resp = ui
            .push_id(NAME_ID, |ui| {
                ui::text_field(ui, egui::vec2(w, ui::CONTROL_H), &mut typed, "color/name")
            })
            .inner;
        if *focus {
            *focus = false;
            resp.request_focus();
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), resp.id) {
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::one(
                        egui::text::CCursor::new(typed.chars().count()),
                    )));
                state.store(ui.ctx(), resp.id);
            }
        }
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if crate::settings::text_button(ui, "Cancel", FieldButton::Off).clicked() {
            *act = Some(Act::CancelName);
        }
        let ok = refusal.is_none();
        let state = if ok {
            FieldButton::Primary
        } else {
            FieldButton::Disabled
        };
        let resp = crate::settings::text_button(ui, "Create and merge", state);
        if (resp.clicked() || enter) && ok {
            *act = Some(Act::Create(g));
        }
    });
    if typed != *name {
        *act = Some(Act::Name(typed));
    }
    let note = match (&refusal, name.trim() == "color/" || name.trim().is_empty()) {
        (Some(why), false) => (why.clone(), color::warn()),
        _ => (
            format!(
                "Binds all {} of the {} to it",
                plural(uses, "use", "uses"),
                plural(colours, "colour", "colours")
            ),
            theme::text::faint(),
        ),
    };
    ui.label(egui::RichText::new(note.0).size(10.5).color(note.1));
}

#[cfg(test)]
mod tests {
    //! The card on a headless app: what it lists, and what its acts write.
    //!
    //! Plain backticks: a `#[cfg(test)]` module is invisible to `cargo doc`.
    use super::*;
    use ondin_core::kurbo::{RoundedRectRadii, Size};
    use ondin_core::{Brush, BrushSource, Fill, NodeKind, Operation, Transaction};

    const A: Color = Color::from_rgb8(0x6D, 0x8C, 0xD9);
    const B: Color = Color::from_rgb8(0x6E, 0x8D, 0xDA);

    /// Three rects in A, one in B, and a locked one in B, on this page.
    struct F {
        app: OndinApp,
        a: NodeId,
        b: NodeId,
        locked: NodeId,
    }

    fn fixture(ctx: &egui::Context) -> F {
        crate::theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let page = app.session.page;
        let mut ops = Vec::new();
        let mut rect = |app: &mut OndinApp, c: Color| {
            let id = app.session.ids.mint();
            ops.push(Operation::CreateNode {
                id,
                parent: page,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            });
            ops.push(Operation::SetFills {
                id,
                fills: ondin_core::keyed_by_position([Fill {
                    brush: Brush::Solid(c).into(),
                    visible: true,
                }]),
            });
            id
        };
        let a = rect(&mut app, A);
        rect(&mut app, A);
        rect(&mut app, A);
        let b = rect(&mut app, B);
        let locked = rect(&mut app, B);
        ops.push(Operation::SetLocked {
            id: locked,
            locked: true,
        });
        assert!(app.session.commit(Transaction(ops)));
        app.canvas_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 700.0));
        F { app, a, b, locked }
    }

    fn run(app: &mut OndinApp, ctx: &egui::Context) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1400.0, 900.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |_| app.colour_review_ui(ctx));
    }

    fn fill(app: &OndinApp, id: NodeId) -> BrushSource {
        app.session.doc.get(id).unwrap().paint().fills[0]
            .brush
            .clone()
    }

    /// **A merge is one undo step and the status line says what it did**: the
    /// raw member takes the anchor, and the locked layer is left, counted, and
    /// still listed.
    ///
    /// flip: `merge_group` dropping the locked note → fails at the status line.
    #[test]
    fn a_merge_is_one_step_and_says_what_it_did() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.open_colour_review();
        run(&mut f.app, &ctx);
        assert_eq!(f.app.colour_review.seen.groups.len(), 1, "A and B");
        assert_eq!(f.app.colour_review.unfolded(), Some(0), "the first is open");

        f.app.colour_review_act(&ctx, Some(Act::Merge(0)));
        assert_eq!(fill(&f.app, f.b), BrushSource::from(A));
        assert_eq!(fill(&f.app, f.locked), BrushSource::from(B), "locked");
        assert_eq!(
            f.app.session.status().text,
            "Merged 1 colour into 6D8CD9 · 1 use on 1 layer · 1 on locked layers left"
        );
        run(&mut f.app, &ctx);
        let r = &f.app.colour_review;
        assert_eq!(r.seen.groups.len(), 1, "the locked B is shown still");
        assert_eq!(r.seen.census[r.seen.groups[0].members[1]].uses, 1);
        assert!(f.app.session.undo());
        assert_eq!(fill(&f.app, f.b), BrushSource::from(B), "one step");
    }

    /// The caret's other merge names a new variable first; a name with an empty
    /// group is refused, a free one creates it and binds every member, the
    /// anchor's too.
    #[test]
    fn a_new_variable_is_named_then_binds_every_member() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.open_colour_review();
        run(&mut f.app, &ctx);
        f.app.colour_review_act(&ctx, Some(Act::Choose(true)));
        assert_eq!(
            f.app.colour_review.naming.as_ref().map(|(n, _)| n.as_str()),
            Some("color/")
        );
        run(&mut f.app, &ctx);
        f.app.colour_review_act(&ctx, Some(Act::Create(0)));
        assert!(
            f.app.session.doc.variables().iter().next().is_none(),
            "an empty group part is refused"
        );
        f.app
            .colour_review_act(&ctx, Some(Act::Name("color/accent".into())));
        f.app.colour_review_act(&ctx, Some(Act::Create(0)));
        let var = f
            .app
            .session
            .doc
            .variables()
            .iter()
            .find(|v| v.name == "color/accent")
            .map(|v| v.id)
            .expect("created");
        for id in [f.a, f.b] {
            assert_eq!(fill(&f.app, id), BrushSource::Bound { var, alpha: 1.0 });
        }
        assert!(f.app.colour_review.naming.is_none(), "the footer is back");
        assert!(f.app.colour_review.new_var, "the button keeps the choice");
    }

    /// The card opens on the selection when there is one, and it and Find take
    /// turns at the canvas's corner.
    #[test]
    fn it_opens_on_the_selection_and_takes_turns_with_find() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.open_find(false);
        f.app.open_colour_review();
        assert!(!f.app.find.open, "Find closed");
        assert!(
            !f.app.colour_review.selection,
            "nothing selected: the document"
        );
        f.app.colour_review.open = false;
        f.app.session.selection.set(vec![f.a]);
        f.app.open_colour_review();
        assert!(f.app.colour_review.selection);
        f.app.open_find(false);
        assert!(!f.app.colour_review.open, "Find closed the card");
    }

    /// A kit colour's row names its kit masked in private mode, as every kit
    /// surface does; the variable's own name stays.
    ///
    /// flip: `name_of` reading the kit's name from the table → fails at "masked".
    #[test]
    fn a_kit_member_names_its_kit_masked_in_private_mode() {
        use ondin_core::variable::{Cell, KitMode, KitModes, VarKind, VarOrigin, Variable};
        const KIT: &str = "0123456789abcdef0123456789abcdef";
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let (mode, var) = (f.app.session.ids.mint_mode(), f.app.session.ids.mint_var());
        let from = f.app.session.ids.mint_mode();
        let foreign = f.app.session.ids.mint_var();
        // Applied to a copy: a commit's passes drop a kit variable no main reads.
        let mut doc = f.app.session.doc.clone();
        doc.apply(&Transaction(vec![
            Operation::SetKitModes {
                kit: KIT.into(),
                modes: Some(KitModes {
                    name: Some("Atlas".into()),
                    modes: vec![KitMode {
                        id: mode,
                        name: "Light".into(),
                        from,
                    }],
                    map: Default::default(),
                }),
            },
            Operation::AddVariable {
                var: Variable {
                    id: var,
                    name: "warning".into(),
                    kind: VarKind::Color,
                    cells: [(mode, Cell::Color(A))].into_iter().collect(),
                    origin: Some(VarOrigin {
                        kit: KIT.into(),
                        var: foreign,
                        version: 1,
                    }),
                    forked: None,
                    description: String::new(),
                },
            },
        ]))
        .expect("a kit's colour");
        let s = Swatch {
            color: A,
            var: Some(var),
            uses: 1,
            nodes: Vec::new(),
        };
        let kit = |doc: &Document| match name_of(doc, &s) {
            Name::Var { name, kit } => (name, kit.unwrap_or_default()),
            Name::Hex { .. } => panic!("a variable"),
        };
        assert_eq!(kit(&doc).1, "Atlas", "the fixture's kit");
        crate::private::set(true);
        let (name, masked) = kit(&doc);
        crate::private::set(false);
        assert_eq!(name, "warning", "the variable's name stays");
        assert!(!masked.contains("Atlas"), "masked: {masked}");
    }

    /// `Escape` closes the menu, then gives up the name, then closes the card.
    #[test]
    fn escape_steps_back_one_thing_at_a_time() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.open_colour_review();
        let r = &mut f.app.colour_review;
        r.menu = true;
        r.naming = Some(("color/".into(), false));
        assert!(r.escape() && !r.menu && r.naming.is_some());
        assert!(r.escape() && r.naming.is_none() && r.open);
        assert!(!r.escape() && !r.open);
    }

    /// The card draws its states without a panic — unfolded with the menu open,
    /// naming, and at Tight — and the threshold reflows the groups.
    #[test]
    fn every_state_draws() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.open_colour_review();
        f.app.colour_review.menu = true;
        run(&mut f.app, &ctx);
        f.app.colour_review_act(&ctx, Some(Act::Choose(true)));
        run(&mut f.app, &ctx);
        run(&mut f.app, &ctx);
        f.app.prefs.colour_review = Threshold::Tight;
        run(&mut f.app, &ctx);
        assert_eq!(f.app.colour_review.seen.groups.len(), 1, "a hair apart");
    }

    /// The card hangs at the command palette's place — centred on the canvas,
    /// `ui::CANVAS_POPUP_TOP` under its top — as tall as its rows with a group
    /// unfolded, and no taller than its list's cap with many groups.
    ///
    /// flip: `list` scrolling past the room left rather than `CANVAS_POPUP_LIST_H`
    /// → fails at "capped".
    #[test]
    fn the_card_sits_at_the_palettes_place_and_keeps_its_height() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.open_colour_review();
        let card = |app: &mut OndinApp| {
            for _ in 0..3 {
                run(app, &ctx);
            }
            ctx.memory(|m| m.area_rect(egui::Id::new("colour-review")))
                .expect("shown")
        };
        let rect = card(&mut f.app);
        assert_eq!(f.app.colour_review.unfolded(), Some(0), "unfolded");
        let canvas = f.app.canvas_rect;
        assert!(
            (rect.center().x - canvas.center().x).abs() < 1.0,
            "{rect:?}"
        );
        assert!((rect.top() - canvas.top() - ui::CANVAS_POPUP_TOP).abs() < 1.0);
        assert!(rect.height() < 320.0, "as tall as its rows: {rect:?}");

        // Twenty pairs of greys a level apart, each pair ten levels from the next.
        let page = f.app.session.page;
        let mut ops = Vec::new();
        for i in 0..40u8 {
            let id = f.app.session.ids.mint();
            let v = 20 + (i / 2) * 10 + i % 2;
            ops.push(Operation::CreateNode {
                id,
                parent: page,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            });
            ops.push(Operation::SetFills {
                id,
                fills: ondin_core::keyed_by_position([Fill {
                    brush: Brush::Solid(Color::from_rgb8(v, v, v)).into(),
                    visible: true,
                }]),
            });
        }
        assert!(f.app.session.commit(Transaction(ops)));
        let rect = card(&mut f.app);
        assert!(
            f.app.colour_review.groups_shown() > 15,
            "the fixture's pairs"
        );
        let most = 40.0 + 90.0 + ui::CANVAS_POPUP_LIST_H + FOOT_H + 12.0;
        assert!(rect.height() <= most, "capped: {} > {most}", rect.height());
        assert!(rect.height() > most - 60.0, "the list filled its cap");
    }
}
