//! *Kits used…* (§5.3e) and *Make all local* (§15 D1016).
//!
//! From the document menu: one row per kit the document vendors from, with where
//! it lives, how many instances use it, and its status — *v9 waiting*, *Up to
//! date · v16*, *Unavailable* — and per row *Open kit*, *Accept update* where one
//! waits, and *Make all local…*, the way out when a kit is gone or no longer
//! wanted. The footer repeats *Accept all*, so the notice can always be found
//! here. There is no *Locate…* (D1011 (8)): a kit is found by its id through the
//! library scan, so *Unavailable* means it is not in the library folder.

use super::index::kits_in;
use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use eframe::egui;
use ondin_core::NodeId;

/// One kit the document uses, as *Kits used…* lists it.
#[derive(Clone, Debug)]
pub(crate) struct Used {
    pub kit: String,
    pub name: String,
    /// *Library / Atlas Design System*, or *Not in your library folder*.
    pub location: String,
    pub instances: usize,
    /// The vault children of this kit — what *Make all local* reveals.
    pub tops: Vec<NodeId>,
    /// The version the copies are at, and the library's, when it is found.
    pub held: u32,
    pub latest: Option<u32>,
}

impl Used {
    /// The row's status on the right.
    pub(crate) fn status(&self) -> String {
        match self.latest {
            None => "Unavailable".into(),
            Some(v) if v > self.held => format!("v{v} waiting"),
            Some(v) => format!("Up to date · v{v}"),
        }
    }
}

/// What a row of the dialog asked for.
enum Act {
    Open(std::path::PathBuf),
    Accept(String),
    AcceptAll,
    MakeAllLocal(String),
    Close,
}

impl OndinApp {
    /// Every kit this document vendors from, by name.
    pub(crate) fn kits_used(&self) -> Vec<Used> {
        let doc = &self.session.doc;
        let Some(vault) = doc.vault().and_then(|v| doc.get(v)) else {
            return Vec::new();
        };
        let library = kits_in(&self.library, doc.meta().id.as_deref());
        let mut out: Vec<Used> = Vec::new();
        for top in vault.children() {
            let Some(o) = doc.get(*top).and_then(|n| n.origin()) else {
                continue;
            };
            let mains: Vec<NodeId> = match doc.get(*top) {
                Some(n) if n.set().is_some() => ondin_core::variant::variants(doc, *top),
                _ => vec![*top],
            };
            let instances: usize = mains
                .iter()
                .map(|m| ondin_core::component::instances_of(doc, *m).len())
                .sum();
            match out.iter_mut().find(|u| u.kit == o.kit) {
                Some(u) => {
                    u.instances += instances;
                    u.tops.push(*top);
                    u.held = u.held.min(o.version);
                }
                None => {
                    let found = library.iter().find(|k| k.id == o.kit);
                    let location = match found {
                        Some(k) => {
                            let project = self
                                .library
                                .entry_at(&k.path)
                                .and_then(|e| e.meta.project.as_deref())
                                .and_then(|p| self.library.projects.get(p))
                                .map(|p| format!("Library / {}", p.name));
                            project.unwrap_or_else(|| "Library".into())
                        }
                        None => "Not in your library folder".into(),
                    };
                    out.push(Used {
                        kit: o.kit.clone(),
                        name: found
                            .map(|k| k.name.clone())
                            .or_else(|| o.name.clone())
                            .unwrap_or_else(|| "Kit".into()),
                        location,
                        instances,
                        tops: vec![*top],
                        held: o.version,
                        latest: found.map(|k| k.version),
                    });
                }
            }
        }
        out.sort_by_key(|u| u.name.to_lowercase());
        out
    }

    /// The *Kits used* dialog, while it is open.
    pub(crate) fn kits_used_dialog(&mut self, ctx: &egui::Context) {
        if !self.kits_used_open {
            return;
        }
        let used = self.kits_used();
        let title = format!("Kits used in {}", self.session.document_title());
        let library = kits_in(&self.library, self.session.doc.meta().id.as_deref());
        let mut act: Option<Act> = None;
        let modal = crate::settings::card("kits-used", ctx, |ui| {
            ui.set_width(crate::ui::menu_inner_w(420.0, crate::settings::PAD));
            if crate::settings::modal_title(ui, &title) {
                act = Some(Act::Close);
            }
            ui.add_space(8.0);
            if used.is_empty() {
                ui.label(
                    egui::RichText::new("This document uses no kit.")
                        .size(12.0)
                        .color(theme::text::DIM),
                );
            }
            for u in &used {
                ui.horizontal(|ui| {
                    ui.label(theme::icon_text(icon::PACKAGE, 14.0, theme::text::MUTED));
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.label(egui::RichText::new(&u.name).size(12.5).color(color::TEXT));
                        ui.label(
                            egui::RichText::new(format!(
                                "{} · {} instance{}",
                                u.location,
                                u.instances,
                                if u.instances == 1 { "" } else { "s" }
                            ))
                            .size(11.0)
                            .color(theme::text::DIM),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let more = crate::ui::field_button(
                            ui,
                            icon::DOTS_THREE,
                            24.0,
                            14.0,
                            crate::ui::FieldButton::Off,
                        )
                        .on_hover_text("More");
                        egui::Popup::menu(&more)
                            .align(egui::RectAlign::BOTTOM_END)
                            .gap(crate::ui::MENU_GAP)
                            .show(|ui| {
                                crate::ui::menu_rows(ui);
                                const H: f32 = crate::ui::MENU_ROW_H;
                                if let Some(k) = library.iter().find(|k| k.id == u.kit)
                                    && crate::ui::menu_item(
                                        ui,
                                        icon::ARROW_SQUARE_OUT,
                                        "Open kit",
                                        H,
                                    )
                                    .clicked()
                                {
                                    act = Some(Act::Open(k.path.clone()));
                                }
                                if u.latest.is_some_and(|v| v > u.held)
                                    && crate::ui::menu_item(
                                        ui,
                                        icon::ARROW_CIRCLE_DOWN,
                                        "Accept update",
                                        H,
                                    )
                                    .clicked()
                                {
                                    act = Some(Act::Accept(u.kit.clone()));
                                }
                                if crate::ui::menu_item(ui, icon::PACKAGE, "Make all local…", H)
                                    .clicked()
                                {
                                    act = Some(Act::MakeAllLocal(u.kit.clone()));
                                }
                            });
                        let waiting = u.latest.is_some_and(|v| v > u.held);
                        ui.label(
                            egui::RichText::new(u.status())
                                .size(11.0)
                                .color(if waiting {
                                    color::TEXT
                                } else {
                                    theme::text::FAINT
                                }),
                        );
                    });
                });
                ui.add_space(6.0);
            }
            ui.add_space(crate::settings::FOOTER_GAP);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if used.iter().any(|u| u.latest.is_some_and(|v| v > u.held))
                    && crate::settings::text_button(ui, "Accept all", crate::ui::FieldButton::On)
                        .clicked()
                {
                    act = Some(Act::AcceptAll);
                }
                if crate::settings::text_button(ui, "Close", crate::ui::FieldButton::Off).clicked()
                {
                    act = Some(Act::Close);
                }
            });
        });
        if modal.should_close() && act.is_none() {
            act = Some(Act::Close);
        }
        match act {
            None => {}
            Some(Act::Close) => self.kits_used_open = false,
            Some(Act::Open(path)) => {
                self.kits_used_open = false;
                self.open_path(&path);
            }
            Some(Act::Accept(kit)) => self.accept_kit_updates(Some(&kit)),
            Some(Act::AcceptAll) => self.accept_kit_updates(None),
            Some(Act::MakeAllLocal(kit)) => self.making_all_local = Some(kit),
        }
        self.make_all_local_dialog(ctx);
    }

    /// *Make all Marketing components local?*: the whole kit at once, every
    /// instance and override kept, undoable. The kit then leaves the list and the
    /// picker's used group.
    pub(crate) fn make_all_local_dialog(&mut self, ctx: &egui::Context) {
        let Some(kit) = self.making_all_local.clone() else {
            return;
        };
        let Some(u) = self.kits_used().into_iter().find(|u| u.kit == kit) else {
            self.making_all_local = None;
            return;
        };
        let mut decision: Option<bool> = None;
        let n = u.tops.len();
        let modal = crate::settings::card("make-all-local", ctx, |ui| {
            ui.set_width(crate::ui::menu_inner_w(380.0, crate::settings::PAD));
            if crate::settings::modal_title(ui, &format!("Make all {} components local?", u.name)) {
                decision = Some(false);
            }
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(format!(
                    "{n} component{} become{} mains in this document that you can edit. They \
                     stop receiving updates from {}.",
                    if n == 1 { "" } else { "s" },
                    if n == 1 { "s" } else { "" },
                    u.name
                ))
                .size(12.0)
                .color(theme::text::MUTED),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(format!(
                    "{} instance{}. Overrides stay. You can undo this.",
                    u.instances,
                    if u.instances == 1 { "" } else { "s" }
                ))
                .size(12.0)
                .color(theme::text::DIM),
            );
            ui.add_space(crate::settings::FOOTER_GAP);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if crate::settings::text_button(ui, "Make all local", crate::ui::FieldButton::On)
                    .clicked()
                {
                    decision = Some(true);
                }
                if crate::settings::text_button(ui, "Cancel", crate::ui::FieldButton::Off).clicked()
                {
                    decision = Some(false);
                }
            });
        });
        if modal.should_close() {
            decision = Some(false);
        }
        match decision {
            Some(true) => {
                self.making_all_local = None;
                self.make_local_now(&u.tops);
            }
            Some(false) => self.making_all_local = None,
            None => {}
        }
    }
}
