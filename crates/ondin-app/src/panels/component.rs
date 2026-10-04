//! The **Component** card — what the selection is to the components machinery
//! (`architecture.md` §5.3d), drawn from §15 D981's accepted chrome
//! (`design/Components.dc.html`, sections 3A–3H): a main's instance count and its
//! two verbs, an instance's link to its main, its drift and the reset family, and
//! the summaries several selected instances get. A layer *inside* an instance gets
//! one line instead of a card — D981's *"one line, not a card"*.
//!
//! **Directly under the identity card** (3H), because it says what the layer *is*.
//!
//! **Counts tell you what each reset will do; a reset with nothing to do is
//! disabled, not hidden** (D981). The counts are `reset::Drift`, cached per
//! session revision (`OndinApp::drift_cache`), since the card is read every frame
//! and a drift is a walk of the instance.
//!
//! ⚠️ **The drift summary counts every difference a *Reset all* would undo** —
//! fields, removed children and order — where the mockup's *3 overrides* sat over a
//! *Reset fields 3* and a *Restore removed children 1*. Read literally that hides a
//! removed child from the summary, and an instance whose only drift is a deleted
//! layer then reads as untouched; the session's choice, open to overturning.
//!
//! ⚠️ **The main's glyph is the outline hexagon** — D981 gives a main the *filled*
//! one, which the bundled Phosphor Regular does not have (§15 D10's *"at two, ship
//! the font"*), owed with the layers panel's and the canvas's marks.

use crate::app::OndinApp;
use crate::theme::{self, icon};
use crate::ui::{self, FieldButton};
use eframe::egui;
use ondin_core::component;
use ondin_core::reset::{Drift, Kind};
use ondin_core::{NodeId, Operation};

/// The card's faces, one per kind of selection D981 draws.
enum Face {
    /// A main component, and how many instances it has.
    Main { name: String, instances: usize },
    /// One instance root.
    Instance {
        main: NodeId,
        main_name: String,
        drift: Drift,
    },
    /// A layer inside an instance — linked to a counterpart in the main, or the
    /// instance's own.
    Child { main_name: String, linked: bool },
    /// Several instance roots and nothing else: of one main (`main` is `Some`) or
    /// of several.
    Instances {
        count: usize,
        main: Option<(NodeId, String)>,
        mains: usize,
        drifted: usize,
        drift: Drift,
    },
    /// Instances among ordinary layers — summarised, with a way to narrow to them.
    Mixed { roots: Vec<NodeId>, others: usize },
}

/// What a click on the card asked for, acted on after it is drawn so nothing is
/// borrowed while the document changes.
enum Act {
    Select(Vec<NodeId>),
    GoToMain(NodeId),
    /// The selected child's counterpart in the main — `OndinApp::go_to_main`'s
    /// rule for a child, which selects what it was copied from, not the main.
    GoToSource,
    Reset(Kind),
    Detach,
    SelectAllInstances,
    DuplicateAsComponent,
}

impl OndinApp {
    /// [`reset::drift`](ondin_core::reset::drift) at `scope`, from the cache while
    /// the document is unchanged.
    pub(crate) fn drift_of(&mut self, scope: NodeId) -> Drift {
        let rev = self.session.revision();
        if self.drift_cache.0 != rev {
            self.drift_cache = (rev, Default::default());
        }
        let doc = &self.session.doc;
        *self
            .drift_cache
            .1
            .entry(scope)
            .or_insert_with(|| ondin_core::reset::drift(doc, scope))
    }

    /// Gather [`OndinApp::card_overrides`] for this frame: every selected linked
    /// layer's overrides (`reset::overrides`), grouped by the card that shows them.
    /// Over several instances a card counts and resets every one of them — D981's
    /// *"the dot shows, and a reset resets every one of them"*.
    pub(super) fn gather_card_overrides(&mut self) {
        let doc = &self.session.doc;
        let mut cards: Vec<(&'static str, usize, Vec<Operation>)> = Vec::new();
        for id in self.session.selection.ids() {
            for o in ondin_core::reset::overrides(doc, *id) {
                let Some(title) = card_of(&o.reset) else {
                    continue;
                };
                match cards.iter_mut().find(|(t, ..)| *t == title) {
                    Some((_, n, ops)) => {
                        *n += o.units;
                        ops.push(o.reset);
                    }
                    None => cards.push((title, o.units, vec![o.reset])),
                }
            }
        }
        self.card_overrides = cards;
    }

    /// The overrides `title`'s header shows, if it has any.
    pub(super) fn card_override(&self, title: &str) -> Option<(usize, &[Operation])> {
        self.card_overrides
            .iter()
            .find(|(t, ..)| *t == title)
            .map(|(_, n, ops)| (*n, ops.as_slice()))
    }

    /// The card's face for the selection, or `None` where it draws nothing.
    fn component_face(&mut self) -> Option<Face> {
        let ids = ondin_core::build::outermost(&self.session.doc, self.session.selection.ids());
        let doc = &self.session.doc;
        let name = |id: NodeId| {
            doc.get(id)
                .map(|n| n.name().to_string())
                .unwrap_or_default()
        };
        let main_of = |root: NodeId| component::main_of(doc, root);
        if let [one] = ids.as_slice() {
            let node = doc.get(*one)?;
            if node.component() {
                return Some(Face::Main {
                    name: node.name().to_string(),
                    instances: component::instances_of(doc, *one).len(),
                });
            }
            let root = component::instance_root(doc, *one)?;
            let main = main_of(root)?;
            let main_name = name(main);
            if root != *one {
                return Some(Face::Child {
                    main_name,
                    linked: node.link().is_some(),
                });
            }
            let drift = self.drift_of(root);
            return Some(Face::Instance {
                main,
                main_name,
                drift,
            });
        }
        let roots: Vec<NodeId> = ids
            .iter()
            .copied()
            .filter(|id| component::instance_root(doc, *id) == Some(*id))
            .collect();
        if roots.is_empty() {
            return None;
        }
        if roots.len() < ids.len() {
            return Some(Face::Mixed {
                others: ids.len() - roots.len(),
                roots,
            });
        }
        let mut mains: Vec<NodeId> = roots.iter().filter_map(|r| main_of(*r)).collect();
        mains.sort();
        mains.dedup();
        let main = match mains.as_slice() {
            [m] => Some((*m, name(*m))),
            _ => None,
        };
        let count = roots.len();
        let drifts: Vec<Drift> = roots.iter().map(|r| self.drift_of(*r)).collect();
        Some(Face::Instances {
            count,
            main,
            mains: mains.len(),
            drifted: drifts.iter().filter(|d| d.any()).count(),
            drift: drifts.into_iter().fold(Drift::default(), sum),
        })
    }

    /// The Component card (§15 D981) — see the module doc.
    pub(super) fn inspector_component(&mut self, ui: &mut egui::Ui) {
        let Some(face) = self.component_face() else {
            return;
        };
        let mut act = None;
        match &face {
            Face::Child { main_name, linked } => {
                slim_row(ui, |ui| child_line(ui, main_name, *linked, &mut act));
            }
            Face::Mixed { roots, others } => {
                slim_row(ui, |ui| mixed_line(ui, roots, *others, &mut act));
            }
            Face::Main { .. } | Face::Instance { .. } | Face::Instances { .. } => {
                self.panel(ui, "Component", None, |_app, ui| match &face {
                    Face::Main { name, instances } => main_body(ui, name, *instances, &mut act),
                    Face::Instance {
                        main,
                        main_name,
                        drift,
                    } => {
                        let summary = drift_summary(*drift);
                        let link = Some((*main, main_name.as_str()));
                        heading(ui, "Instance of", link, None, &summary, &mut act);
                        reset_row(ui, *drift, true, &mut act);
                    }
                    Face::Instances {
                        count,
                        main,
                        mains,
                        drifted,
                        drift,
                    } => {
                        let summary = match drifted {
                            0 => String::new(),
                            n => format!("{n} with overrides"),
                        };
                        match main {
                            Some((m, n)) => {
                                let caption = format!("{count} instances of");
                                heading(ui, &caption, Some((*m, n)), None, &summary, &mut act);
                                reset_row(ui, *drift, true, &mut act);
                            }
                            // 3F: no single main to go to, so no name link and no
                            // overflow of counted resets.
                            None => {
                                let title = format!("Instances of {mains} components");
                                heading(ui, "", None, Some(&title), &summary, &mut act);
                                reset_row(ui, *drift, false, &mut act);
                            }
                        }
                    }
                    Face::Child { .. } | Face::Mixed { .. } => {}
                });
            }
        }
        match act {
            None => {}
            Some(Act::Select(ids)) => self.session.selection.set(ids),
            Some(Act::GoToMain(main)) => {
                self.session.selection.set_one(main);
                self.reveal_selection();
            }
            Some(Act::GoToSource) => self.go_to_main(),
            // Through `commit_edit`, as every inspector edit that changes ink is
            // (`OndinApp::reset_tx`). *Detach* below changes no pixel and commits
            // as its verb does — *Group*'s case in `commit_edit`'s doc.
            Some(Act::Reset(kind)) => {
                if let Some(tx) = self.reset_tx(kind) {
                    self.commit_edit(tx);
                }
            }
            Some(Act::Detach) => self.detach_instances(),
            Some(Act::SelectAllInstances) => self.select_all_instances(),
            Some(Act::DuplicateAsComponent) => self.duplicate_as_component(),
        }
    }

    /// The list `read` takes from the node `id` is compared with — its source's
    /// (`reset::source_of`) — or `None` outside an instance. What a list card
    /// reads its rows' trailing slots and its ghost rows against (§15 D981, 4D).
    pub(super) fn source_list<T: Clone>(
        &self,
        id: NodeId,
        read: impl Fn(&ondin_core::Node) -> &[ondin_core::Keyed<T>],
    ) -> Option<Vec<ondin_core::Keyed<T>>> {
        let doc = &self.session.doc;
        ondin_core::reset::source_of(doc, id)
            .and_then(|s| doc.get(s))
            .map(|s| read(s).to_vec())
    }
}

/// A list item's reserved 14-pt trailing slot (§15 D981, 4D), laid out by a
/// right-to-left row: empty while the item follows its main, the dot when it
/// differs, `+` when it is the instance's own. On an overridden item whose row is
/// `hot` — the pointer anywhere on it — the dot becomes ↺; answers whether that
/// was clicked.
///
/// ⚠️ **`hot` comes from the caller**, because this sits in a right-to-left run
/// whose own box begins past the row's label: asked of that box, a pointer on the
/// label was not on the row, and the ↺ never showed (measured by
/// `a_fill_list_marks_restores_and_resets_item_by_item`).
pub(super) fn item_slot(ui: &mut egui::Ui, state: ondin_core::reset::ItemState, hot: bool) -> bool {
    use ondin_core::reset::ItemState;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(14.0, ui.max_rect().height().min(ui::CONTROL_H)),
        egui::Sense::click(),
    );
    let p = ui.painter();
    match state {
        ItemState::Follows => false,
        ItemState::Local => {
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                icon::PLUS,
                theme::icon_font(11.0),
                theme::text::MUTED,
            );
            resp.on_hover_text("This instance's own — not in the main");
            false
        }
        ItemState::Overridden if hot => {
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                icon::ARROW_COUNTER_CLOCKWISE,
                theme::icon_font(12.0),
                theme::text::STRONG,
            );
            resp.on_hover_text("Reset to main").clicked()
        }
        ItemState::Overridden => {
            ui::override_dot(p, rect.center());
            false
        }
    }
}

/// A **ghost row**: an item the instance removed while its main still has it,
/// drawn dashed and dim with *Restore* (§15 D981, 4D) — the list half of the
/// reset family. `label` is the item's own words (a hex, *Linear*, *Drop
/// shadow*). Answers whether *Restore* was clicked.
///
/// ⚠️ **Lists only.** A deleted child *layer* gets no ghost row in the layers
/// panel — D981's asymmetry, ruled: only the card's *Restore removed children*.
pub(super) fn ghost_row(ui: &mut egui::Ui, label: &str, width: f32) -> bool {
    let h = ui::CONTROL_H;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, h), egui::Sense::hover());
    let p = ui.painter();
    let dash = egui::Stroke::new(1.0, theme::text::FAINT);
    let r = rect.shrink(0.5);
    for (a, b) in [
        (r.left_top(), r.right_top()),
        (r.right_top(), r.right_bottom()),
        (r.right_bottom(), r.left_bottom()),
        (r.left_bottom(), r.left_top()),
    ] {
        p.extend(egui::Shape::dashed_line(&[a, b], dash, 3.0, 3.0));
    }
    let chip = egui::Rect::from_center_size(
        egui::pos2(rect.left() + ui::FIELD_PAD_X + 8.0, rect.center().y),
        egui::vec2(12.0, 12.0),
    );
    p.rect_filled(chip, 2.0, theme::text::FAINT.gamma_multiply(0.5));
    p.text(
        egui::pos2(chip.right() + 9.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(12.0),
        theme::text::FAINT,
    );
    let word = "Restore";
    let w = ui::action_button_w(ui.ctx(), word) - 8.0;
    let at = egui::Rect::from_min_size(
        egui::pos2(rect.right() - w - 4.0, rect.top() + 4.0),
        egui::vec2(w, h - 8.0),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(at));
    ui::action_button(
        &mut child,
        icon::ARROW_COUNTER_CLOCKWISE,
        word,
        FieldButton::Off,
        at.size(),
    )
    .on_hover_text("Put the main's item back")
    .clicked()
}

/// The inspector card that shows the field `op` writes — its title, as `panel`
/// takes it — or `None` for a field no card header owns: the name and visibility
/// (the identity card), a path's or a boolean's shape, the text's content and its
/// rail, the mask flag and the fill rule (identity-row toggles).
///
/// Wildcard-free on the operation, so a new field operation is placed here by
/// whoever adds it.
fn card_of(op: &Operation) -> Option<&'static str> {
    use Operation as O;
    use ondin_core::GeometryPatch as G;
    match op {
        O::SetTransform { .. } | O::SetPivot { .. } => Some("Transform"),
        O::SetGeometry { geometry, .. } => match geometry {
            G::Size(_) | G::LineEnd(_) => Some("Transform"),
            G::CornerRadius(_) | G::CornerRadii(_) | G::Sides(_) | G::InnerRatio(_) => {
                Some("Appearance")
            }
            G::TextSizing(_) => Some("Type"),
            G::Path { .. }
            | G::TextPath(_)
            | G::TextPathFlip(_)
            | G::TextPathOffset(_)
            | G::BoolOp(_) => None,
        },
        O::SetOpacity { .. } | O::SetClip { .. } => Some("Appearance"),
        O::SetMaskMode { .. } => Some("Mask"),
        O::SetFills { .. } => Some("Fill"),
        O::SetStrokes { .. } => Some("Stroke"),
        O::SetEffects { .. } => Some("Effects"),
        O::SetExports { .. } => Some("Export"),
        O::SetLayoutGrids { .. } => Some("Layout grid"),
        O::SetInsets { .. } => Some("Position"),
        O::SetLayoutItem { .. } => Some("Item"),
        O::SetDisplay { .. } => Some("Container"),
        O::SetTextStyle { .. } | O::SetParagraphStyle { .. } | O::SetBlockStyle { .. } => {
            Some("Type")
        }
        O::SetName { .. }
        | O::SetVisible { .. }
        | O::SetMask { .. }
        | O::SetFillRule { .. }
        | O::SetText { .. }
        | O::SetTextSpans { .. }
        | O::SetParagraphSpans { .. }
        | O::SetLocked { .. }
        | O::SetProportionsLocked { .. }
        | O::SetComponent { .. }
        | O::SetLink { .. }
        | O::CreateNode { .. }
        | O::DeleteNode { .. }
        | O::InsertSubtree { .. }
        | O::Reparent { .. }
        | O::Reorder { .. }
        | O::SetCanvasBackground { .. }
        | O::AddGuide { .. }
        | O::RemoveGuide { .. }
        | O::SetGuidePosition { .. }
        | O::SetGuideColor { .. }
        | O::SetGuideScope { .. }
        | O::AddImage { .. }
        | O::RemoveImage { .. } => None,
    }
}

fn sum(a: Drift, b: Drift) -> Drift {
    Drift {
        fields: a.fields + b.fields,
        removed: a.removed + b.removed,
        order: a.order + b.order,
        local: a.local + b.local,
    }
}

/// *3 overrides · 1 local layer* — empty at zero, which is how it disappears.
fn drift_summary(d: Drift) -> String {
    let overrides = d.fields + d.removed + d.order;
    let mut parts = Vec::new();
    if overrides > 0 {
        parts.push(plural(overrides, "override"));
    }
    if d.local > 0 {
        parts.push(plural(d.local, "local layer"));
    }
    parts.join(" · ")
}

fn plural(n: usize, what: &str) -> String {
    match n {
        1 => format!("1 {what}"),
        n => format!("{n} {what}s"),
    }
}

/// The two-line face of an instance card: the outline hexagon, a small caption
/// over the main's name as a link (or over a plain `title`), and the grey drift
/// summary on the right.
fn heading(
    ui: &mut egui::Ui,
    caption: &str,
    link: Option<(NodeId, &str)>,
    title: Option<&str>,
    summary: &str,
    act: &mut Option<Act>,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 9.0;
        let slot = glyph_slot(ui);
        let block = ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            if !caption.is_empty() {
                ui.label(
                    egui::RichText::new(caption)
                        .size(11.0)
                        .color(theme::text::DIM),
                );
            }
            if let Some((main, name)) = link {
                // The name, then the ↗ — one target, so a click on either goes.
                let resp = ui
                    .horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let name = ui.add(
                            egui::Label::new(
                                egui::RichText::new(name)
                                    .size(13.0)
                                    .underline()
                                    .color(theme::text::STRONG),
                            )
                            .sense(egui::Sense::click()),
                        );
                        let arrow = ui.add(
                            egui::Label::new(
                                egui::RichText::new(icon::ARROW_SQUARE_OUT)
                                    .font(theme::icon_font(12.0))
                                    .color(theme::text::DIM),
                            )
                            .sense(egui::Sense::click()),
                        );
                        name | arrow
                    })
                    .inner
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Go to main component");
                if resp.clicked() {
                    *act = Some(Act::GoToMain(main));
                }
            }
            if let Some(title) = title {
                ui.label(
                    egui::RichText::new(title)
                        .size(13.0)
                        .color(theme::text::STRONG),
                );
            }
        });
        paint_glyph(ui, icon::HEXAGON, slot, block.response.rect);
        if !summary.is_empty() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(summary)
                        .size(11.0)
                        .color(theme::text::DIM),
                );
            });
        }
    });
}

/// **Reset all**, **Detach**, and — where there is one main to count against —
/// the overflow of counted resets (3B, 3C).
fn reset_row(ui: &mut egui::Ui, d: Drift, overflow: bool, act: &mut Option<Act>) {
    let gap = ui::CARD_COL_GAP;
    let h = ui::CONTROL_H;
    let w = ui.available_width();
    let dots = if overflow { h + gap } else { 0.0 };
    let half = ((w - dots - gap) / 2.0).max(0.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        let any = d.any();
        let state = if any {
            FieldButton::Off
        } else {
            FieldButton::Disabled
        };
        let reset = ui::action_button(
            ui,
            icon::ARROW_COUNTER_CLOCKWISE,
            "Reset all",
            state,
            egui::vec2(half, h),
        )
        .on_hover_text(if any {
            "Put everything back as the main has it — your own layers stay"
        } else {
            "Nothing here differs from the main component"
        });
        if any && reset.clicked() {
            *act = Some(Act::Reset(Kind::All));
        }
        if ui::action_button(
            ui,
            icon::LINK_BREAK,
            "Detach",
            FieldButton::Off,
            egui::vec2(half, h),
        )
        .on_hover_text("Detach instance · Ctrl+Alt+B")
        .clicked()
        {
            *act = Some(Act::Detach);
        }
        if overflow {
            let more = ui::field_button(ui, icon::DOTS_THREE, h, 15.0, FieldButton::Off)
                .on_hover_text("More resets");
            egui::Popup::menu(&more)
                .align(egui::RectAlign::BOTTOM_END)
                .show(|ui| {
                    ui::menu_rows(ui);
                    for (kind, label, n) in [
                        (Kind::Fields, "Reset fields", d.fields),
                        (Kind::Children, "Restore removed children", d.removed),
                        (Kind::Order, "Reset order", d.order),
                    ] {
                        let count = match n {
                            0 => "—".to_string(),
                            n => n.to_string(),
                        };
                        let row = ui::MenuRow::new("", label)
                            .accel(Some(&count))
                            .enabled(n > 0);
                        if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() && n > 0 {
                            *act = Some(Act::Reset(kind));
                        }
                    }
                });
        }
    });
}

/// 3A: the filled hexagon (owed — see the module doc), *Main component* over the
/// name, the instance count with *Select all*, and *Duplicate as component*.
fn main_body(ui: &mut egui::Ui, name: &str, instances: usize, act: &mut Option<Act>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 9.0;
        let slot = glyph_slot(ui);
        let block = ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(
                egui::RichText::new("Main component")
                    .size(11.0)
                    .color(theme::text::DIM),
            );
            ui.label(
                egui::RichText::new(name)
                    .size(13.0)
                    .color(theme::text::STRONG),
            );
        });
        paint_glyph(ui, icon::HEXAGON, slot, block.response.rect);
    });
    let h = ui::CONTROL_H;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(plural(instances, "instance"))
                .size(12.0)
                .color(theme::text::MUTED),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = "Select all";
            let w = ui::action_button_w(ui.ctx(), label);
            let state = match instances {
                0 => FieldButton::Disabled,
                _ => FieldButton::Off,
            };
            let resp = ui::action_button(ui, icon::SELECTION_ALL, label, state, egui::vec2(w, h))
                .on_hover_text("Select all instances");
            if instances > 0 && resp.clicked() {
                *act = Some(Act::SelectAllInstances);
            }
        });
    });
    let w = ui.available_width();
    if ui::action_button(
        ui,
        icon::COPY,
        "Duplicate as component",
        FieldButton::Off,
        egui::vec2(w, h),
    )
    .on_hover_text("A copy that is a new main, not an instance of this one")
    .clicked()
    {
        *act = Some(Act::DuplicateAsComponent);
    }
}

/// 3D: *In Button instance · Go to main*, or *Local to this instance · no
/// counterpart*.
fn child_line(ui: &mut egui::Ui, main_name: &str, linked: bool, act: &mut Option<Act>) {
    let (glyph, text) = if linked {
        (icon::HEXAGON, format!("In {main_name} instance"))
    } else {
        (icon::PLUS, "Local to this instance".to_string())
    };
    let mut slot = egui::Rect::NOTHING;
    let row = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 9.0;
        slot = glyph_slot(ui);
        ui.label(
            egui::RichText::new(text)
                .size(12.0)
                .color(theme::text::STRONG),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if linked {
                let label = "Go to main";
                let w = ui::action_button_w(ui.ctx(), label);
                if ui::action_button(
                    ui,
                    icon::ARROW_SQUARE_OUT,
                    label,
                    FieldButton::Off,
                    egui::vec2(w, ui::CONTROL_H),
                )
                .on_hover_text("Go to main component")
                .clicked()
                {
                    *act = Some(Act::GoToSource);
                }
            } else {
                ui.label(
                    egui::RichText::new("no counterpart")
                        .size(11.0)
                        .color(theme::text::DIM),
                );
            }
        });
    });
    paint_glyph(ui, glyph, slot, row.response.rect);
}

/// 3G: *2 instances · 3 other layers*, and *Select instances* to narrow to them.
fn mixed_line(ui: &mut egui::Ui, roots: &[NodeId], others: usize, act: &mut Option<Act>) {
    let mut slot = egui::Rect::NOTHING;
    let row = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 9.0;
        slot = glyph_slot(ui);
        ui.label(
            egui::RichText::new(format!(
                "{} · {}",
                plural(roots.len(), "instance"),
                plural(others, "other layer")
            ))
            .size(12.0)
            .color(theme::text::STRONG),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = "Select instances";
            let w = ui::action_button_w(ui.ctx(), label);
            if ui::action_button(
                ui,
                icon::SELECTION_ALL,
                label,
                FieldButton::Off,
                egui::vec2(w, ui::CONTROL_H),
            )
            .clicked()
            {
                *act = Some(Act::Select(roots.to_vec()));
            }
        });
    });
    paint_glyph(ui, icon::HEXAGON, slot, row.response.rect);
}

/// The leading glyph's column, reserved before the row's other content exists.
///
/// ⚠️ **Painted afterwards ([`paint_glyph`]) rather than laid out as a label**:
/// a horizontal row centres each item against the height the row has *so far*,
/// so a glyph placed first sat centred on its own line and read as top-aligned
/// beside a two-line block — measured at y≈151 against the block's ≈158.
fn glyph_slot(ui: &mut egui::Ui) -> egui::Rect {
    ui.allocate_exact_size(egui::vec2(16.0, 0.0), egui::Sense::hover())
        .0
}

/// The leading glyph, centred on `beside` — the block or the row it leads.
fn paint_glyph(ui: &egui::Ui, glyph: &str, slot: egui::Rect, beside: egui::Rect) {
    ui.painter().text(
        egui::pos2(slot.center().x, beside.center().y),
        egui::Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(16.0),
        theme::text::STRONG,
    );
}

/// A card with no header — the one-line faces (3D, 3G).
fn slim_row(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui::card_at(ui, |ui| add(ui));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::Size;
    use ondin_core::{Document, IdSource, NodeKind, Operation, Placement, Transaction};

    /// A main frame *Button* holding a rect, an instance of it, and a plain rect
    /// beside them — adopted into a headless app.
    struct F {
        app: OndinApp,
        m: NodeId,
        i: NodeId,
        ir: NodeId,
        plain: NodeId,
    }

    fn fixture(ctx: &egui::Context) -> F {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0xC4);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [m, r, plain] = [(); 3].map(|_| ids.mint());
        let rect = || NodeKind::Rect {
            size: Size::new(10.0, 10.0),
            corner_radii: Default::default(),
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
            create(
                m,
                root,
                NodeKind::Artboard {
                    size: Size::new(100.0, 100.0),
                },
                "Button",
            ),
            create(r, m, rect(), "Label"),
            create(plain, root, rect(), "Plain"),
            Operation::SetComponent {
                id: m,
                component: true,
            },
        ]))
        .expect("a main");
        let (tx, made) = ondin_core::insert_subtrees(
            &doc,
            &mut ids,
            &[Placement {
                nodes: doc.capture_subtree(m).unwrap(),
                parent: root,
                index: None,
            }],
            Default::default(),
        );
        doc.apply(&tx).expect("an instance");
        let i = made[0];
        let ir = doc.get(i).unwrap().children()[0];
        app.session.adopt_document(doc, None);
        F {
            app,
            m,
            i,
            ir,
            plain,
        }
    }

    fn face_of(app: &mut OndinApp, ids: &[NodeId]) -> Option<Face> {
        app.session.selection.set(ids.to_vec());
        app.component_face()
    }

    /// Each selection gets D981's face for it: a main, an instance, a child, two
    /// instances of one main, instances among other layers, and nothing for a
    /// plain layer.
    #[test]
    fn each_selection_gets_its_face() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        assert!(matches!(
            face_of(&mut f.app, &[f.m]),
            Some(Face::Main { instances: 1, .. })
        ));
        assert!(matches!(
            face_of(&mut f.app, &[f.i]),
            Some(Face::Instance { main, .. }) if main == f.m
        ));
        assert!(matches!(
            face_of(&mut f.app, &[f.ir]),
            Some(Face::Child { linked: true, .. })
        ));
        assert!(matches!(
            face_of(&mut f.app, &[f.i, f.plain]),
            Some(Face::Mixed { others: 1, .. })
        ));
        assert!(face_of(&mut f.app, &[f.plain]).is_none());
        let second = {
            let doc = &f.app.session.doc;
            let (tx, made) = ondin_core::insert_subtrees(
                doc,
                &mut f.app.session.ids,
                &[Placement {
                    nodes: doc.capture_subtree(f.m).unwrap(),
                    parent: doc.root(),
                    index: None,
                }],
                Default::default(),
            );
            assert!(f.app.session.commit(tx));
            made[0]
        };
        assert!(matches!(
            face_of(&mut f.app, &[f.i, second]),
            Some(Face::Instances {
                count: 2,
                main: Some(_),
                ..
            })
        ));
    }

    /// The drift cache follows the document: an edit moves the revision and the
    /// count with it. Flip: never clearing the cache leaves the stale zero — and
    /// fails `reset_all_on_the_card_puts_the_instance_back` too, whose *1 override*
    /// is read through it.
    #[test]
    fn the_drift_cache_follows_an_edit() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        assert_eq!(f.app.drift_of(f.i), Drift::default());
        assert!(f.app.session.commit(Transaction(vec![Operation::SetName {
            id: f.ir,
            name: "Mine".into(),
        }])));
        assert_eq!(f.app.drift_of(f.i).fields, 1);
    }

    /// Every text the frame painted, with where it was painted.
    fn texts(out: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(t) => {
                    out.push((t.galley.text().to_string(), t.visual_bounding_rect()))
                }
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut v = Vec::new();
        for s in &out.shapes {
            walk(&s.shape, &mut v);
        }
        v
    }

    fn frame(
        app: &mut OndinApp,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1400.0, 2400.0),
            )),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| {
            ui.set_max_width(280.0);
            app.inspector_ui(ui)
        })
    }

    /// **The card, drawn and clicked.** An instance with a renamed child shows
    /// *Instance of* over its main's name and *1 override*; a press and release on
    /// *Reset all* puts the name back and the summary goes. Driven through
    /// `RawInput`, so the button's `Response` is the one the card reads.
    #[test]
    fn reset_all_on_the_card_puts_the_instance_back() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        assert!(f.app.session.commit(Transaction(vec![Operation::SetName {
            id: f.ir,
            name: "Mine".into(),
        }])));
        f.app.session.selection.set_one(f.i);
        let mut out = frame(&mut f.app, &ctx, Vec::new());
        for _ in 0..3 {
            out = frame(&mut f.app, &ctx, Vec::new());
        }
        let painted = texts(&out);
        // The hexagon is centred on the caption-and-name block, not on its own
        // first line (`glyph_slot`): it measured y≈151 against the block's ≈158
        // laid out as a leading label.
        let at = painted
            .iter()
            .position(|(t, _)| t == "Instance of")
            .unwrap();
        let after = &painted[at..];
        let rect_of = |s: &str| after.iter().find(|(t, _)| t == s).unwrap().1;
        let block = rect_of("Instance of").union(rect_of("Button"));
        let hex = rect_of(icon::HEXAGON);
        assert!(
            (hex.center().y - block.center().y).abs() <= 1.0,
            "hexagon {hex:?} beside block {block:?}"
        );
        let has = |s: &str| painted.iter().any(|(t, _)| t == s);
        assert!(has("Instance of") && has("Button"), "{painted:?}");
        assert!(has("1 override"), "{painted:?}");
        let at = painted
            .iter()
            .find(|(t, _)| t == "Reset all")
            .map(|(_, r)| r.center())
            .expect("the button's word is painted");
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut f.app, &ctx, vec![egui::Event::PointerMoved(at)]);
        frame(&mut f.app, &ctx, vec![press(true)]);
        frame(&mut f.app, &ctx, vec![press(false)]);
        assert_eq!(f.app.session.doc.get(f.ir).unwrap().name(), "Label");
        let out = frame(&mut f.app, &ctx, Vec::new());
        assert!(
            !texts(&out).iter().any(|(t, _)| t.contains("override")),
            "the summary is gone at zero"
        );
    }

    /// Every text the frame painted with its fallback colour — what a galley
    /// painted in one colour carries.
    fn inks(out: &egui::FullOutput) -> Vec<(String, egui::Rect, egui::Color32)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect, egui::Color32)>) {
            match shape {
                egui::Shape::Text(t) => out.push((
                    t.galley.text().to_string(),
                    t.visual_bounding_rect(),
                    t.fallback_color,
                )),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut v = Vec::new();
        for s in &out.shapes {
            walk(&s.shape, &mut v);
        }
        v
    }

    /// **A field's mark** (§15 D981, 4A–4B). The instance's rect is moved along
    /// x only: its X label is painted bright, its Y dim — compared on the stored
    /// local translation, since the card's numbers are world ones and the copy sits
    /// elsewhere than its main with nothing overridden. A click on X's label slot
    /// writes the main's x back and leaves y alone. Flip: comparing the whole
    /// transform rather than one coefficient in `transform_marks` fails *"Y
    /// follows"* — Y painted bright.
    #[test]
    fn an_overridden_field_is_bright_and_its_label_resets_it() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let main_rect = f.app.session.doc.get(f.m).unwrap().children()[0];
        let moved = ondin_core::kurbo::Affine::translate((7.0, 0.0));
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetTransform {
                    id: f.ir,
                    transform: moved,
                }]))
        );
        f.app.session.selection.set_one(f.ir);
        let mut out = frame(&mut f.app, &ctx, Vec::new());
        for _ in 0..3 {
            out = frame(&mut f.app, &ctx, Vec::new());
        }
        let painted = inks(&out);
        let ink_of = |s: &str| {
            painted
                .iter()
                .find(|(t, ..)| t == s)
                .map(|(_, r, c)| (*r, *c))
                .unwrap_or_else(|| panic!("no {s} in {painted:?}"))
        };
        let (x_at, x_ink) = ink_of("X");
        let (_, y_ink) = ink_of("Y");
        assert_eq!(x_ink, theme::text::STRONG, "X is overridden");
        assert_eq!(y_ink, theme::text::FAINT, "Y follows");
        let at = x_at.center();
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut f.app, &ctx, vec![egui::Event::PointerMoved(at)]);
        frame(&mut f.app, &ctx, vec![press(true)]);
        frame(&mut f.app, &ctx, vec![press(false)]);
        let doc = &f.app.session.doc;
        assert_eq!(
            doc.get(f.ir).unwrap().transform(),
            doc.get(main_rect).unwrap().transform(),
            "the main's x is back"
        );
    }

    /// **A list reads item by item against the main's** (§15 D981, 4D). The main's
    /// rect has two fills; the instance recolours the first, removes the second and
    /// adds one of its own. The removed one draws as a ghost row whose *Restore*
    /// puts it back by its id; the own one shows `+`; and the recoloured one's
    /// slot, under the pointer, is ↺, whose click takes the main's colour back.
    #[test]
    fn a_fill_list_marks_restores_and_resets_item_by_item() {
        use ondin_core::{Fill, Keyed, keyed_by_position};
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let r = f.app.session.doc.get(f.m).unwrap().children()[0];
        let solid = |g: u8| Fill {
            brush: ondin_core::Brush::Solid(ondin_core::peniko::Color::from_rgb8(10, g, 30)),
            visible: true,
        };
        let main = keyed_by_position([solid(1), solid(2)]);
        assert!(f.app.session.commit(Transaction(vec![Operation::SetFills {
            id: r,
            fills: main.clone(),
        }])));
        assert_eq!(fills_of(&f.app, f.ir), main, "the fixture: it propagated");
        let own = Keyed::new(f.app.session.ids.mint_item(), solid(3));
        assert!(f.app.session.commit(Transaction(vec![Operation::SetFills {
            id: f.ir,
            fills: vec![main[0].map(|_| solid(9)), own.clone()],
        }])));
        f.app.session.selection.set_one(f.ir);
        let mut out = frame(&mut f.app, &ctx, Vec::new());
        for _ in 0..3 {
            out = frame(&mut f.app, &ctx, Vec::new());
        }
        let painted = texts(&out);
        let gone_hex = crate::ui::hex_of(ondin_core::peniko::Color::from_rgb8(10, 2, 30));
        assert!(painted.iter().any(|(t, _)| *t == gone_hex), "a ghost row");
        let pluses = painted.iter().filter(|(t, _)| t == icon::PLUS).count();
        assert!(
            pluses >= 2,
            "the header's + and the own fill's: {painted:?}"
        );
        // *Restore* on the ghost row.
        let click = |app: &mut OndinApp, at: egui::Pos2| {
            let press = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            frame(app, &ctx, vec![egui::Event::PointerMoved(at)]);
            frame(app, &ctx, vec![press(true)]);
            frame(app, &ctx, vec![press(false)]);
        };
        let restore = painted
            .iter()
            .find(|(t, _)| t == "Restore")
            .map(|(_, r)| r.center())
            .expect("the ghost row's Restore");
        click(&mut f.app, restore);
        let now = fills_of(&f.app, f.ir);
        assert!(now.iter().any(|k| k.id == main[1].id), "restored by its id");
        assert!(now.contains(&own), "the own fill stays");
        // ↺ on the recoloured row.
        let changed_hex = crate::ui::hex_of(ondin_core::peniko::Color::from_rgb8(10, 9, 30));
        let out = frame(&mut f.app, &ctx, Vec::new());
        let row = texts(&out)
            .into_iter()
            .find(|(t, _)| *t == changed_hex)
            .map(|(_, r)| r)
            .expect("the recoloured row");
        frame(
            &mut f.app,
            &ctx,
            vec![egui::Event::PointerMoved(row.center())],
        );
        let out = frame(&mut f.app, &ctx, Vec::new());
        let undo = texts(&out)
            .into_iter()
            .find(|(t, r)| {
                t == icon::ARROW_COUNTER_CLOCKWISE && (r.center().y - row.center().y).abs() < 4.0
            })
            .map(|(_, r)| r.center())
            .expect("↺ in the hovered row's slot");
        click(&mut f.app, undo);
        let now = fills_of(&f.app, f.ir);
        assert_eq!(now.iter().find(|k| k.id == main[0].id).unwrap(), &main[0]);
    }

    /// The Effects card reads its stack the same way (§15 D981, 4D–4E's *Inner
    /// shadow ↺*): the instance hides the main's first effect and removes the
    /// second; the second is a ghost row whose *Restore* brings it back, and the
    /// first's ↺ under the pointer takes the main's back.
    #[test]
    fn an_effect_stack_restores_and_resets_item_by_item() {
        use ondin_core::{Effect, EffectKind, keyed_by_position};
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let r = f.app.session.doc.get(f.m).unwrap().children()[0];
        let [a, b, ..] = EffectKind::all_defaults();
        let main = keyed_by_position([Effect::new(a.clone()), Effect::new(b.clone())]);
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetEffects {
                    id: r,
                    effects: main.clone(),
                }]))
        );
        let mut hidden = main[0].clone();
        hidden.visible = false;
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetEffects {
                    id: f.ir,
                    effects: vec![hidden],
                }]))
        );
        f.app.collapsed_panels.remove("Effects");
        f.app.session.selection.set_one(f.ir);
        let effects_of = |app: &OndinApp| app.session.doc.get(f.ir).unwrap().effects().to_vec();
        let click = |app: &mut OndinApp, at: egui::Pos2| {
            let press = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            frame(app, &ctx, vec![egui::Event::PointerMoved(at)]);
            frame(app, &ctx, vec![press(true)]);
            frame(app, &ctx, vec![press(false)]);
        };
        let mut out = frame(&mut f.app, &ctx, Vec::new());
        for _ in 0..3 {
            out = frame(&mut f.app, &ctx, Vec::new());
        }
        let restore = texts(&out)
            .into_iter()
            .find(|(t, _)| t == "Restore")
            .map(|(_, r)| r.center())
            .unwrap_or_else(|| panic!("no ghost row in {:?}", texts(&out)));
        click(&mut f.app, restore);
        assert!(
            effects_of(&f.app).iter().any(|k| k.id == main[1].id),
            "restored"
        );
        let out = frame(&mut f.app, &ctx, Vec::new());
        let row = texts(&out)
            .into_iter()
            .find(|(t, _)| t == a.label())
            .map(|(_, r)| r)
            .expect("the hidden effect's row");
        frame(
            &mut f.app,
            &ctx,
            vec![egui::Event::PointerMoved(row.center())],
        );
        let out = frame(&mut f.app, &ctx, Vec::new());
        let undo = texts(&out)
            .into_iter()
            .find(|(t, r)| {
                t == icon::ARROW_COUNTER_CLOCKWISE && (r.center().y - row.center().y).abs() < 4.0
            })
            .map(|(_, r)| r.center())
            .expect("↺ in the hovered effect row");
        click(&mut f.app, undo);
        assert_eq!(
            effects_of(&f.app)
                .iter()
                .find(|k| k.id == main[0].id)
                .unwrap(),
            &main[0]
        );
    }

    fn fills_of(app: &OndinApp, id: NodeId) -> Vec<ondin_core::Keyed<ondin_core::Fill>> {
        app.session.doc.get(id).unwrap().paint().fills.clone()
    }

    /// **A card header counts its overrides and resets them** (§15 D981, 4E). The
    /// instance's rect has its opacity overridden: the Appearance header shows `1`
    /// at rest; with the pointer on the header the count becomes *Reset
    /// appearance*, and a click there puts the main's opacity back — and leaves the
    /// card open, the header's own toggle lying under the chip. Flip: `card_of`
    /// mapping opacity nowhere fails *"the count after the label"* (the header has
    /// none, and the next text is the field's `50`). ⚠️ Dropping
    /// `section_head_full`'s `clicks.toggled = false` stays **green**: egui gives
    /// the press to the topmost target only, the chip registered after the header,
    /// so that line is a belt and the registration order is what holds it.
    #[test]
    fn a_card_header_counts_and_resets_its_overrides() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetOpacity {
                    id: f.ir,
                    opacity: 0.5,
                }]))
        );
        f.app.session.selection.set_one(f.ir);
        let mut out = frame(&mut f.app, &ctx, Vec::new());
        for _ in 0..3 {
            out = frame(&mut f.app, &ctx, Vec::new());
        }
        let painted = texts(&out);
        let head = painted
            .iter()
            .position(|(t, _)| t == "APPEARANCE")
            .unwrap_or_else(|| panic!("no Appearance header in {painted:?}"));
        assert_eq!(painted[head + 1].0, "1", "the count after the label");
        let on_head = painted[head].1.center();
        frame(&mut f.app, &ctx, vec![egui::Event::PointerMoved(on_head)]);
        let out = frame(&mut f.app, &ctx, Vec::new());
        let at = texts(&out)
            .into_iter()
            .find(|(t, _)| t == "Reset appearance")
            .map(|(_, r)| r.center())
            .expect("the hovered header offers the reset");
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut f.app, &ctx, vec![egui::Event::PointerMoved(at)]);
        frame(&mut f.app, &ctx, vec![press(true)]);
        frame(&mut f.app, &ctx, vec![press(false)]);
        assert_eq!(f.app.session.doc.get(f.ir).unwrap().opacity(), 1.0);
        assert!(
            !f.app.collapsed_panels.contains("Appearance"),
            "still open: the reset is the press, not the toggle"
        );
    }
}
