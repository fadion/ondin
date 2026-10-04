//! Variants and component properties in the inspector (§5.3d build step 7, §15
//! D982, from `design/Variants.dc.html` with the maintainer's three rulings held
//! over it): a set's **Variants** card, a variant's face inside the Component card,
//! an instance's variant dropdowns and property rows between *Instance of* and
//! *Reset all*, the **Properties** card on a main or a set, and the binding line on
//! a layer inside a main.
//!
//! **A variant choice is never an override** (the design's rule): its dropdowns
//! carry no dot, and choosing a value *switches* the instance to another variant
//! (`variant::switch`) rather than writing a field. A component property is a view
//! over fields, so its row's dot is the bound field's override and its ↺ that
//! field's reset.
//!
//! ⚠️ **Where the session departs from the mockup, open to overturning:** a value
//! is renamed, moved and deleted from a popup on its chip rather than edited in
//! place and dragged (3B); a layer is bound from one line on the Component card —
//! *Visibility* and *Content*, each a dropdown of the owner's properties — rather
//! than from a `{}` button in the layers row, the Appearance card and the Type card
//! (3J–3L), so no other card had to learn about properties; and a property's
//! default is the main's own bound field, edited where that field is, so the define
//! popover (3I) has no *Default* row.

use crate::app::OndinApp;
use crate::theme::{self, icon};
use crate::ui::{self, FieldButton};
use eframe::egui;
use ondin_core::variant::{self, PropKind, PropValue, Property};
use ondin_core::{Keyed, NodeId, Transaction, component};

/// A row's label column, the width the Component card's dropdowns sit after.
const LABEL_W: f32 = 76.0;
/// A value chip's height.
const CHIP_H: f32 = 22.0;

/// A text field that edits `current` and answers the new text once, when the
/// field gives its focus up with something new in it — `Escape` abandons, as every
/// committed text field here does (`ui::defocus_commits`). The typed text lives in
/// egui's memory under `id` while the field has focus, and nowhere after.
fn name_field(
    ui: &mut egui::Ui,
    id: egui::Id,
    current: &str,
    size: egui::Vec2,
    pt: f32,
) -> Option<String> {
    let mut buf = ui
        .data(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| current.to_string());
    let resp = ui
        .push_id(id, |ui| ui::text_field(ui, size, &mut buf, "", pt))
        .inner;
    let focused = resp.has_focus();
    ui.data_mut(|d| {
        if focused {
            d.insert_temp(id, buf.clone());
        } else {
            d.remove::<String>(id);
        }
    });
    let done =
        ui::defocus_commits(&resp) || (focused && ui.input(|i| i.key_pressed(egui::Key::Enter)));
    (done && buf.trim() != current && !buf.trim().is_empty()).then(|| buf.trim().to_string())
}

/// One option of a [`dropdown`]: its label, and why it is greyed when it is.
struct Choice {
    label: String,
    refused: Option<String>,
}

/// A dropdown of `choices` showing `shown`, answering the index picked.
fn dropdown(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    width: f32,
    shown: &str,
    choices: &[Choice],
    current: Option<usize>,
    enabled: Option<&str>,
) -> Option<usize> {
    let mut pick = None;
    ui::disable_unless(ui, enabled.is_none(), |ui| {
        let resp = egui::ComboBox::from_id_salt(salt)
            .icon(ui::combo_chevron)
            .width(width)
            .selected_text(egui::RichText::new(shown).size(12.0))
            .show_ui(ui, |ui| {
                ui::menu_rows(ui);
                for (i, c) in choices.iter().enumerate() {
                    let row = ui.add_enabled(
                        c.refused.is_none(),
                        egui::Button::selectable(current == Some(i), c.label.as_str()),
                    );
                    if let Some(why) = &c.refused {
                        row.on_disabled_hover_text(why);
                    } else if row.clicked() {
                        pick = Some(i);
                    }
                }
            })
            .response;
        if let Some(why) = enabled {
            resp.on_disabled_hover_text(why);
        }
    });
    pick
}

/// A row's label in the label column.
fn row_label(ui: &mut egui::Ui, text: &str, mark: Option<&super::component::OverrideMark>) -> bool {
    super::component::label_mark(
        ui,
        Some(egui::vec2(LABEL_W, ui::CONTROL_H)),
        text,
        12.0,
        theme::text::MUTED,
        mark,
    )
}

/// A leading glyph and a line of text, the card's one-line notes (a clash).
fn note(ui: &mut egui::Ui, glyph: &str, text: &str) -> egui::Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(
            egui::RichText::new(glyph)
                .font(theme::icon_font(13.0))
                .color(theme::text::STRONG),
        );
        ui.label(
            egui::RichText::new(text)
                .size(12.0)
                .color(theme::text::STRONG),
        );
    })
    .response
}

/// *2 variants are Large, Hover* — or `None` without a clash.
pub(crate) fn clash_text(doc: &ondin_core::Document, set: NodeId) -> Option<String> {
    let clashing = variant::clashes(doc, set);
    let first = doc.get(*clashing.first()?)?;
    let same = clashing
        .iter()
        .filter(|c| doc.get(**c).is_some_and(|n| n.variant() == first.variant()))
        .count();
    Some(format!(
        "{same} variants are {}",
        variant::derived_name(first.variant())
    ))
}

impl OndinApp {
    /// The **Variants** card's body for the set `set` (3A–3C): its name and
    /// counts, a clash note, each property with its values as chips, and the
    /// verbs that add to it.
    pub(super) fn set_body(&mut self, ui: &mut egui::Ui, set: NodeId) {
        let doc = &self.session.doc;
        let Some(s) = doc.get(set) else { return };
        let Some(vs) = s.set().cloned() else { return };
        let name = s.name().to_string();
        let variants = variant::variants(doc, set);
        let instances: usize = variants
            .iter()
            .map(|v| component::instances_of(doc, *v).len())
            .sum();
        let clash = clash_text(doc, set);
        let mut out: Option<Transaction> = None;
        let mut select_all = false;
        let mut add_variant = false;
        let mut refused: Option<&str> = None;

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 9.0;
            ui.label(
                egui::RichText::new(icon::SQUARES_FOUR)
                    .font(theme::icon_font(16.0))
                    .color(theme::text::STRONG),
            );
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(
                    egui::RichText::new("Component set")
                        .size(11.0)
                        .color(theme::text::DIM),
                );
                ui.label(
                    egui::RichText::new(&name)
                        .size(13.0)
                        .color(theme::text::STRONG),
                );
            });
        });
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{} · {}",
                    plural(variants.len(), "variant"),
                    plural(instances, "instance")
                ))
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
                if ui::action_button(
                    ui,
                    icon::SELECTION_ALL,
                    label,
                    state,
                    egui::vec2(w, ui::CONTROL_H),
                )
                .on_hover_text("Select all instances of every variant")
                .clicked()
                    && instances > 0
                {
                    select_all = true;
                }
            });
        });
        if let Some(text) = &clash {
            note(ui, icon::WARNING, text);
        }
        for (pi, p) in vs.props.iter().enumerate() {
            ui.add_space(2.0);
            // The property's name, renamed in place; its delete in the chip row's
            // last slot.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                let w = ui.available_width() - ui::CONTROL_H - ui::CARD_COL_GAP;
                let id = ui.id().with(("variant-prop", set, pi));
                if let Some(n) = name_field(ui, id, &p.name, egui::vec2(w, ui::CONTROL_H), 12.0) {
                    match variant::rename_property(doc, set, pi, &n) {
                        Some(tx) => out = Some(tx),
                        None => refused = Some("That name is taken"),
                    }
                }
                let last = vs.props.len() < 2;
                let state = if last {
                    FieldButton::Disabled
                } else {
                    FieldButton::Off
                };
                let resp = ui::field_button(ui, icon::X, ui::CONTROL_H, 13.0, state).on_hover_text(
                    if last {
                        "A set needs at least one property"
                    } else {
                        "Delete this property"
                    },
                );
                if resp.clicked() && !last {
                    out = variant::delete_property(doc, set, pi);
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(5.0, 5.0);
                for (vi, v) in p.values.iter().enumerate() {
                    let chip = ui.add(
                        egui::Button::new(egui::RichText::new(v).size(12.0))
                            .min_size(egui::vec2(0.0, CHIP_H)),
                    );
                    egui::Popup::menu(&chip)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .show(|ui| {
                            ui.set_min_width(200.0);
                            ui::menu_rows(ui);
                            let id = ui.id().with(("variant-value", set, pi, vi));
                            if let Some(n) =
                                name_field(ui, id, v, egui::vec2(200.0, ui::CONTROL_H), 12.0)
                            {
                                match variant::rename_value(doc, set, pi, vi, &n) {
                                    Some(tx) => out = Some(tx),
                                    None => refused = Some("That value is taken"),
                                }
                            }
                            let n = p.values.len();
                            let row = |ui: &mut egui::Ui, label: &str, ok: bool| {
                                ui::menu_row(
                                    ui,
                                    ui::MenuRow::new("", label).enabled(ok),
                                    ui::MENU_ROW_H,
                                )
                                .clicked()
                                    && ok
                            };
                            if row(ui, "Move earlier", vi > 0) {
                                out = variant::move_value(doc, set, pi, vi, vi - 1);
                            }
                            if row(ui, "Move later", vi + 1 < n) {
                                out = variant::move_value(doc, set, pi, vi, vi + 1);
                            }
                            ui::menu_sep(ui);
                            // 3C: confirm only when variants use it, with exact
                            // counts — and the instances **detach** (§15 D982,
                            // D979 (c)), where the mockup relinked them.
                            let using = variant::using_value(doc, set, pi, vi);
                            let detach: usize = using
                                .iter()
                                .map(|u| component::instances_of(doc, *u).len())
                                .sum();
                            if n < 2 {
                                row(ui, "Delete value", false);
                            } else if using.is_empty() {
                                if row(ui, "Delete value", true) {
                                    out = variant::delete_value(doc, set, pi, vi);
                                }
                            } else {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} use{} it and {} deleted with it.{}",
                                        plural(using.len(), "variant"),
                                        if using.len() == 1 { "s" } else { "" },
                                        if using.len() == 1 { "is" } else { "are" },
                                        match detach {
                                            0 => String::new(),
                                            d => format!(" {} will detach.", plural(d, "instance")),
                                        }
                                    ))
                                    .size(11.5)
                                    .color(theme::text::MUTED),
                                );
                                let label =
                                    format!("Delete value and {}", plural(using.len(), "variant"));
                                if row(ui, &label, true) {
                                    out = variant::delete_value(doc, set, pi, vi);
                                }
                            }
                        });
                }
                let add = ui.add(
                    egui::Button::new(
                        egui::RichText::new(icon::PLUS)
                            .font(theme::icon_font(12.0))
                            .color(theme::text::MUTED),
                    )
                    .min_size(egui::vec2(CHIP_H, CHIP_H)),
                );
                if add.on_hover_text("Add a value").clicked() {
                    out = variant::add_value(doc, set, pi, &variant::next_value_name(&vs, pi));
                }
            });
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
            let half = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
            if ui::action_button(
                ui,
                icon::PLUS,
                "Property",
                FieldButton::Off,
                egui::vec2(half, ui::CONTROL_H),
            )
            .on_hover_text("Add a variant property")
            .clicked()
            {
                out = variant::add_property(doc, set, &variant::next_property_name(&vs), "Default");
            }
            if ui::action_button(
                ui,
                icon::HEXAGON,
                "Add variant",
                FieldButton::Off,
                egui::vec2(half, ui::CONTROL_H),
            )
            .on_hover_text("A copy of the last variant, at the next free combination")
            .clicked()
            {
                add_variant = true;
            }
        });
        if let Some(why) = refused {
            self.session.info(why);
        }
        if let Some(tx) = out {
            self.commit_edit(tx);
        }
        if select_all {
            self.select_all_instances();
        }
        if add_variant {
            self.add_variant();
        }
    }

    /// A variant's rows inside the Component card (3D, 3E): one dropdown per
    /// property — choosing a value moves this main to that combination, a taken
    /// one showing the clash rather than being refused — and the clash note.
    pub(super) fn variant_rows(&mut self, ui: &mut egui::Ui, main: NodeId) {
        let doc = &self.session.doc;
        let (Some(set), Some(m)) = (variant::set_of(doc, main), doc.get(main)) else {
            return;
        };
        let Some(vs) = doc.get(set).and_then(|s| s.set()).cloned() else {
            return;
        };
        let values = m.variant().to_vec();
        let mut out = None;
        let mut select = None;
        for (pi, p) in vs.props.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                row_label(ui, &p.name, None);
                let choices: Vec<Choice> = p
                    .values
                    .iter()
                    .map(|v| Choice {
                        label: v.clone(),
                        refused: None,
                    })
                    .collect();
                let current = p.values.iter().position(|v| Some(v) == values.get(pi));
                let w = ui.available_width() - 8.0;
                let shown = values.get(pi).map(String::as_str).unwrap_or("");
                if let Some(i) = dropdown(
                    ui,
                    ("variant-of", main, pi),
                    w,
                    shown,
                    &choices,
                    current,
                    None,
                ) {
                    let mut next = values.clone();
                    next[pi] = p.values[i].clone();
                    out = variant::set_values(doc, main, next);
                }
            });
        }
        let other = variant::clashes(doc, set)
            .into_iter()
            .find(|c| *c != main && doc.get(*c).is_some_and(|n| n.variant() == values));
        if let Some(o) = other {
            ui.horizontal(|ui| {
                note(
                    ui,
                    icon::WARNING,
                    &format!("Another variant is also {}", variant::derived_name(&values)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = "Select it";
                    let w = ui::action_button_w(ui.ctx(), label);
                    if ui::action_button(
                        ui,
                        icon::ARROW_SQUARE_OUT,
                        label,
                        FieldButton::Off,
                        egui::vec2(w, ui::CONTROL_H),
                    )
                    .clicked()
                    {
                        select = Some(o);
                    }
                });
            });
        }
        if let Some(tx) = out {
            self.commit_edit(tx);
        }
        if let Some(o) = select {
            self.session.selection.set_one(o);
        }
    }

    /// The rows an instance — or several sharing one owner — gets between
    /// *Instance of* and *Reset all* (4A–4E): a dropdown per variant property,
    /// switching the instances (a missing combination greyed, with the reason),
    /// then each component property, with its override mark.
    pub(super) fn instance_rows(&mut self, ui: &mut egui::Ui, roots: &[NodeId]) {
        let doc = &self.session.doc;
        let Some(&first) = roots.first() else { return };
        let Some(main) = component::main_of(doc, first) else {
            return;
        };
        let Some(owner) = variant::owner_of_main(doc, main) else {
            return;
        };
        let mut out: Option<Transaction> = None;
        let switchable = roots.iter().all(|r| variant::can_switch(doc, *r));
        if let Some(set) = variant::set_of(doc, main)
            && let Some(vs) = doc.get(set).and_then(|s| s.set()).cloned()
        {
            let mains: Vec<NodeId> = roots
                .iter()
                .filter_map(|r| component::main_of(doc, *r))
                .collect();
            for (pi, p) in vs.props.iter().enumerate() {
                let shown_of = |m: &NodeId| doc.get(*m).and_then(|n| n.variant().get(pi).cloned());
                let first_value = shown_of(&mains[0]);
                let mixed = mains.iter().any(|m| shown_of(m) != first_value);
                let shown = if mixed {
                    "Mixed".to_string()
                } else {
                    first_value.clone().unwrap_or_default()
                };
                // A value is offered where every instance has somewhere to go.
                let choices: Vec<Choice> = p
                    .values
                    .iter()
                    .map(|v| {
                        let missing = mains
                            .iter()
                            .find(|m| variant::switch_target(doc, **m, pi, v).is_none());
                        Choice {
                            label: v.clone(),
                            refused: missing.map(|m| {
                                let mut want = doc
                                    .get(*m)
                                    .map(|n| n.variant().to_vec())
                                    .unwrap_or_default();
                                if let Some(slot) = want.get_mut(pi) {
                                    *slot = v.clone();
                                }
                                format!(
                                    "No {} variant in {}",
                                    variant::derived_name(&want),
                                    doc.get(set).map(|s| s.name()).unwrap_or("")
                                )
                            }),
                        }
                    })
                    .collect();
                let current = (!mixed)
                    .then(|| {
                        p.values
                            .iter()
                            .position(|v| Some(v) == first_value.as_ref())
                    })
                    .flatten();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    row_label(ui, &p.name, None);
                    let w = ui.available_width() - 8.0;
                    let refuse = (!switchable).then_some(
                        "A nested instance switches in its main — not yet inside another instance",
                    );
                    if let Some(i) = dropdown(
                        ui,
                        ("variant-pick", first, pi),
                        w,
                        &shown,
                        &choices,
                        current,
                        refuse,
                    ) {
                        let value = p.values[i].clone();
                        let mut ops = Vec::new();
                        for (r, m) in roots.iter().zip(&mains) {
                            let Some(to) = variant::switch_target(doc, *m, pi, &value) else {
                                continue;
                            };
                            if to == *m {
                                continue;
                            }
                            if let Some(tx) = variant::switch(doc, *r, to, &mut self.session.ids) {
                                ops.extend(tx.0);
                            }
                        }
                        out = Some(Transaction(ops));
                    }
                });
            }
        }
        // Component properties, from the owner.
        let props = doc
            .get(owner)
            .map(|o| o.props().to_vec())
            .unwrap_or_default();
        for p in &props {
            let states: Vec<(PropValue, bool)> = roots
                .iter()
                .filter_map(|r| variant::property_state(doc, *r, p))
                .collect();
            let Some((value, _)) = states.first().cloned() else {
                continue;
            };
            let mixed = states.iter().any(|(v, _)| *v != value);
            let overridden = states.iter().any(|(_, o)| *o);
            let reset = variant::reset_property(doc, roots, p);
            let default = main_default(doc, main, p);
            let mark = overridden.then(|| super::component::OverrideMark {
                tip: format!("Reset to main · {}", say(&default)),
                tx: Transaction(reset.clone()),
            });
            match p.kind {
                PropKind::Boolean => {
                    let on = matches!(value, PropValue::Boolean(true));
                    let resp = if mixed {
                        ui::switch_row_mixed(ui, &p.name, ui::CONTROL_H)
                    } else {
                        ui::switch_row_marked(ui, &p.name, on, ui::CONTROL_H, overridden)
                    };
                    let resp = match &mark {
                        Some(m) => resp.on_hover_text(&m.tip),
                        None => resp,
                    };
                    if resp.clicked() {
                        let next = if mixed { true } else { !on };
                        out = Some(Transaction(variant::set_property(
                            doc,
                            roots,
                            p,
                            &PropValue::Boolean(next),
                        )));
                    }
                }
                PropKind::Text => {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        if row_label(ui, &p.name, mark.as_ref()) {
                            out = Some(Transaction(reset.clone()));
                        }
                        let text = match (&value, mixed) {
                            (_, true) => "Mixed".to_string(),
                            (PropValue::Text(t), false) => t.clone(),
                            _ => String::new(),
                        };
                        let w = ui.available_width() - 8.0;
                        let id = ui.id().with(("prop-text", first, p.name.as_str()));
                        if let Some(t) =
                            name_field(ui, id, &text, egui::vec2(w, ui::CONTROL_H), 12.0)
                        {
                            out = Some(Transaction(variant::set_property(
                                doc,
                                roots,
                                p,
                                &PropValue::Text(t),
                            )));
                        }
                    });
                }
            }
        }
        if let Some(tx) = out.filter(|t| !t.0.is_empty()) {
            self.commit_edit(tx);
        }
    }

    /// The **Properties** card on a main or a set (3G, 3H): each property's kind,
    /// name and default, and what it is bound to; a row's name renames it and its ×
    /// deletes it; the header's `+` adds one, and on a lone main also offers
    /// *Variant*, which wraps it in a set.
    pub(super) fn inspector_properties(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.session.selection.single() else {
            return;
        };
        let doc = &self.session.doc;
        let Some(owner) = variant::owner_of_main(doc, id) else {
            return;
        };
        // A variant's card is its set's, shown on the set.
        if owner != id {
            return;
        }
        let props = doc
            .get(owner)
            .map(|o| o.props().to_vec())
            .unwrap_or_default();
        let lone_main = doc.get(owner).is_some_and(|n| n.component());
        let mut out: Option<Transaction> = None;
        let mut combine = false;
        self.panel(ui, "Properties", None, |app, ui| {
            let doc = &app.session.doc;
            if props.is_empty() {
                ui.label(
                    egui::RichText::new(
                        "Bind a layer's visibility or text to a property from that layer's Component line",
                    )
                    .size(11.5)
                    .color(theme::text::DIM),
                );
            }
            for p in &props {
                property_row(ui, doc, owner, p, &mut out);
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                let n = if lone_main { 3.0 } else { 2.0 };
                let w = (ui.available_width() - ui::CARD_COL_GAP * (n - 1.0)) / n;
                for (glyph, label, kind) in [
                    (icon::EYE, "Boolean", PropKind::Boolean),
                    (icon::TEXT_T, "Text", PropKind::Text),
                ] {
                    if ui::action_button(ui, glyph, label, FieldButton::Off, egui::vec2(w, ui::CONTROL_H))
                        .on_hover_text(match kind {
                            PropKind::Boolean => "A property that shows and hides the layers bound to it",
                            PropKind::Text => "A property that sets the text of the layers bound to it",
                        })
                        .clicked()
                    {
                        let name = fresh_name(&props, label);
                        out = variant::define(doc, &mut app.session.ids, owner, &name, kind, Vec::new())
                            .map(|(tx, _)| tx);
                    }
                }
                if lone_main
                    && ui::action_button(ui, icon::SQUARES_FOUR, "Variant", FieldButton::Off, egui::vec2(w, ui::CONTROL_H))
                        .on_hover_text("Make a set — this main becomes its first variant")
                        .clicked()
                {
                    combine = true;
                }
            });
        });
        if let Some(tx) = out {
            self.commit_edit(tx);
        }
        if combine {
            self.combine_as_variants();
        }
    }

    /// The binding line on a layer inside a main (3J–3L, drawn as one line): its
    /// visibility, and for text its content, each a dropdown of the owner's
    /// properties of that kind — *None*, each property, and *New property*.
    pub(super) fn bind_line(&mut self, ui: &mut egui::Ui, node: NodeId) {
        let doc = &self.session.doc;
        let Some(owner) = variant::owner_above(doc, node) else {
            return;
        };
        let Some(n) = doc.get(node) else { return };
        let is_text = matches!(n.kind(), ondin_core::NodeKind::Text { .. });
        let layer = n.name().to_string();
        let props = doc
            .get(owner)
            .map(|o| o.props().to_vec())
            .unwrap_or_default();
        let mut out: Option<Transaction> = None;
        let kinds: &[(PropKind, &str)] = if is_text {
            &[
                (PropKind::Boolean, "Visibility"),
                (PropKind::Text, "Content"),
            ]
        } else {
            &[(PropKind::Boolean, "Visibility")]
        };
        ui::card_at(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(
                    egui::RichText::new(icon::BRACKETS_CURLY)
                        .font(theme::icon_font(14.0))
                        .color(theme::text::STRONG),
                );
                ui.label(
                    egui::RichText::new("Bind to a component property")
                        .size(12.0)
                        .color(theme::text::STRONG),
                );
            });
            for (kind, label) in kinds {
                let of_kind: Vec<&Keyed<Property>> =
                    props.iter().filter(|p| p.kind == *kind).collect();
                let bound = of_kind.iter().position(|p| p.bound.contains(&node));
                let mut choices = vec![Choice {
                    label: "None".into(),
                    refused: None,
                }];
                choices.extend(of_kind.iter().map(|p| Choice {
                    label: p.name.clone(),
                    refused: None,
                }));
                choices.push(Choice {
                    label: "New property…".into(),
                    refused: None,
                });
                let shown = bound
                    .map(|b| of_kind[b].name.clone())
                    .unwrap_or_else(|| "None".into());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    row_label(ui, label, None);
                    let w = ui.available_width() - 8.0;
                    let current = Some(bound.map_or(0, |b| b + 1));
                    let Some(i) = dropdown(
                        ui,
                        ("bind", node, *label),
                        w,
                        &shown,
                        &choices,
                        current,
                        None,
                    ) else {
                        return;
                    };
                    // Off every property of this kind first, then onto the pick.
                    let mut next: Vec<Keyed<Property>> = props.clone();
                    for p in next.iter_mut().filter(|p| p.kind == *kind) {
                        p.value.bound.retain(|b| *b != node);
                    }
                    if (1..=of_kind.len()).contains(&i) {
                        let pick = of_kind[i - 1].id;
                        if let Some(p) = next.iter_mut().find(|p| p.id == pick) {
                            p.value.bound.push(node);
                        }
                    }
                    let mut ops = vec![ondin_core::Operation::SetProperties {
                        id: owner,
                        props: next.clone(),
                    }];
                    if i == choices.len() - 1 {
                        let name = fresh_name(
                            &next,
                            &match kind {
                                PropKind::Boolean => format!("Show {layer}"),
                                PropKind::Text => format!("{layer} text"),
                            },
                        );
                        let item = self.session.ids.mint_item();
                        next.push(Keyed::new(
                            item,
                            Property {
                                name,
                                kind: *kind,
                                bound: vec![node],
                            },
                        ));
                        ops = vec![ondin_core::Operation::SetProperties {
                            id: owner,
                            props: next,
                        }];
                    }
                    out = Some(Transaction(ops));
                });
            }
        });
        if let Some(tx) = out {
            self.commit_edit(tx);
        }
    }
}

/// One row of the Properties card: the kind's glyph, the name (renamed in
/// place), the default, the ×, and what it is bound to under it.
fn property_row(
    ui: &mut egui::Ui,
    doc: &ondin_core::Document,
    owner: NodeId,
    p: &Keyed<Property>,
    out: &mut Option<Transaction>,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
        let glyph = match p.kind {
            PropKind::Boolean => icon::EYE,
            PropKind::Text => icon::TEXT_T,
        };
        ui.label(
            egui::RichText::new(glyph)
                .font(theme::icon_font(14.0))
                .color(theme::text::MUTED),
        );
        let w = ui.available_width() - ui::CONTROL_H - ui::CARD_COL_GAP;
        let id = ui.id().with(("prop-name", owner, p.id));
        if let Some(n) = name_field(ui, id, &p.name, egui::vec2(w, ui::CONTROL_H), 12.0) {
            let taken = doc
                .get(owner)
                .is_some_and(|o| o.props().iter().any(|q| q.id != p.id && q.name == n));
            if !taken {
                *out = variant::edit_property(doc, owner, p.id, |q| {
                    Some(Property {
                        name: n.clone(),
                        ..q.clone()
                    })
                });
            }
        }
        if ui::field_button(ui, icon::X, ui::CONTROL_H, 13.0, FieldButton::Off)
            .on_hover_text("Delete this property — its layers keep their values")
            .clicked()
        {
            *out = variant::edit_property(doc, owner, p.id, |_| None);
        }
    });
    let bound: Vec<String> = p
        .bound
        .iter()
        .filter_map(|b| doc.get(*b))
        .map(|n| {
            format!(
                "{} · {}",
                n.name(),
                match p.kind {
                    PropKind::Boolean => "visibility",
                    PropKind::Text => "content",
                }
            )
        })
        .collect();
    let default = p
        .bound
        .first()
        .and_then(|b| variant::field_value(doc, *b, p.kind));
    let default = say(&default);
    let line = match bound.as_slice() {
        [] => "Not bound to any layer".to_string(),
        [one] => format!("{default} · {one}"),
        [one, rest @ ..] => format!("{default} · {one} and {} more", rest.len()),
    };
    ui.label(egui::RichText::new(line).size(11.0).color(theme::text::DIM));
}

/// A property's value as a row or a tooltip says it.
fn say(v: &Option<PropValue>) -> String {
    match v {
        Some(PropValue::Boolean(true)) => "On".into(),
        Some(PropValue::Boolean(false)) => "Off".into(),
        Some(PropValue::Text(t)) => t.clone(),
        None => String::new(),
    }
}

/// The main's own value of `p` — its default — read off the bound layer inside
/// `main`.
fn main_default(doc: &ondin_core::Document, main: NodeId, p: &Property) -> Option<PropValue> {
    let inside = ondin_core::build::subtree_nodes(doc, &[main]);
    p.bound
        .iter()
        .find(|b| inside.contains(b))
        .and_then(|b| variant::field_value(doc, *b, p.kind))
}

/// `base`, or `base 2`, `base 3`… — the first no property has.
fn fresh_name(props: &[Keyed<Property>], base: &str) -> String {
    let taken = |n: &str| props.iter().any(|p| p.name == n);
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|k| format!("{base} {k}"))
        .find(|n| !taken(n))
        .expect("an unbounded range")
}

fn plural(n: usize, what: &str) -> String {
    match n {
        1 => format!("1 {what}"),
        n => format!("{n} {what}s"),
    }
}
