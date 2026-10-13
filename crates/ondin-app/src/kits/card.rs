//! A kit instance's chrome (§5.3e, 4A–4E; §15 D1016): the Component card's
//! source line — *Atlas kit · v8 · v9 waiting* — the ⋯ menu's kit rows (drawn by
//! `panels::component`'s reset row from [`KitLine`]), *Make local*'s
//! confirmation, *Go to main in the kit*, and the kit's link back to the
//! document that sent the user there (D1011 (7)).

use super::index::{kit_heading, kits_in};
use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use eframe::egui;
use ondin_core::NodeId;

/// What a kit instance's card says about where its main lives.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct KitLine {
    /// The kit's id.
    pub kit: String,
    /// What the card calls it.
    pub kit_name: String,
    /// The version the vendored copy is at.
    pub version: u32,
    /// A newer version the library has, when one waits.
    pub waiting: Option<u32>,
    /// The vault's child the main sits in — the main, or its set — which *Make
    /// local* reveals.
    pub top: NodeId,
}

impl OndinApp {
    /// The kit line for an instance whose main is `main`, or `None` when the
    /// main is the document's own.
    pub(crate) fn kit_line(&self, main: NodeId) -> Option<KitLine> {
        let doc = &self.session.doc;
        if !doc.in_vault(main) {
            return None;
        }
        let vault = doc.vault()?;
        let mut top = main;
        while doc.get(top)?.parent()? != vault {
            top = doc.get(top)?.parent()?;
        }
        let origin = doc.get(top)?.origin()?;
        let library = kits_in(&self.library, doc.meta().id.as_deref());
        let heading = kit_heading(doc, &library, main)?;
        let waiting = library
            .iter()
            .find(|k| k.id == origin.kit)
            .map(|k| k.version)
            .filter(|v| *v > origin.version);
        Some(KitLine {
            kit: origin.kit.clone(),
            kit_name: heading.name,
            version: origin.version,
            waiting,
            top,
        })
    }

    /// *Go to main in the kit* (4D): open the kit with the selected instance's
    /// main selected there, and leave its top bar a link back here (D1011 (7)).
    pub(crate) fn go_to_kit_main(&mut self) {
        let doc = &self.session.doc;
        let Some(main) = self
            .session
            .selection
            .single()
            .and_then(|i| ondin_core::component::main_of(doc, i))
        else {
            return;
        };
        let Some(origin) = doc.get(main).and_then(|n| n.origin()).cloned() else {
            return;
        };
        let library = kits_in(&self.library, doc.meta().id.as_deref());
        let Some(kit) = library.into_iter().find(|k| k.id == origin.kit) else {
            self.session
                .info("That kit is not in your library folder — Make local is the way out");
            return;
        };
        let back = self
            .session
            .path
            .clone()
            .map(|p| (p, self.session.document_title()));
        self.open_path(&kit.path);
        if self.session.path.as_deref() != Some(kit.path.as_path()) {
            return;
        }
        if self.session.doc.contains(origin.node) {
            self.session.selection.set_one(origin.node);
            self.reveal_selection();
        }
        self.kit_return = back;
    }

    /// The kit's link back to the document that opened it (D1011 (7)): *← name*,
    /// until the user leaves the kit.
    pub(crate) fn kit_back_link(&mut self, ui: &mut egui::Ui) {
        let Some((path, name)) = self.kit_return.clone() else {
            return;
        };
        let resp = ui
            .add(
                egui::Label::new(
                    egui::RichText::new(format!("← {name}"))
                        .size(11.5)
                        .color(theme::text::MUTED),
                )
                .sense(egui::Sense::click()),
            )
            .on_hover_text(format!("Back to “{name}”"));
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            self.open_path(&path);
        }
    }

    /// *Make Button local?* (4E): reveal the vendored main as the document's own,
    /// every instance and override as it was, undoable.
    pub(crate) fn make_local_dialog(&mut self, ctx: &egui::Context) {
        let Some(top) = self.making_local else {
            return;
        };
        let doc = &self.session.doc;
        let Some(name) = doc.get(top).map(|n| n.name().to_string()) else {
            self.making_local = None;
            return;
        };
        let kit = self
            .kit_line(top)
            .map(|k| k.kit_name)
            .unwrap_or_else(|| "its kit".into());
        let mains = match doc.get(top) {
            Some(n) if n.set().is_some() => ondin_core::variant::variants(doc, top),
            _ => vec![top],
        };
        let instances: usize = mains
            .iter()
            .map(|m| ondin_core::component::instances_of(doc, *m).len())
            .sum();
        let mut decision: Option<bool> = None;
        let modal = crate::settings::card("make-local", ctx, |ui| {
            ui.set_width(crate::ui::menu_inner_w(380.0, crate::settings::PAD));
            if crate::settings::modal_title(ui, &format!("Make {name} local?")) {
                decision = Some(false);
            }
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(format!(
                    "{name} becomes a main in this document that you can edit. It stops \
                     receiving updates from {kit}."
                ))
                .size(12.0)
                .color(theme::text::MUTED),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(format!(
                    "Affects {} instance{} of {name} here. Their overrides stay.",
                    if instances == 1 {
                        "the 1".to_string()
                    } else {
                        format!("all {instances}")
                    },
                    if instances == 1 { "" } else { "s" }
                ))
                .size(12.0)
                .color(theme::text::DIM),
            );
            ui.add_space(crate::settings::FOOTER_GAP);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if crate::settings::text_button(ui, "Make local", crate::ui::FieldButton::On)
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
                self.making_local = None;
                self.make_local_now(&[top]);
            }
            Some(false) => self.making_local = None,
            None => {}
        }
    }

    /// Reveal `tops` and place them down the right of the content (D1011 (3)),
    /// one step; the selection does not move.
    pub(crate) fn make_local_now(&mut self, tops: &[NodeId]) {
        // A commit under a live text session (`[X3.2-L5-01]`).
        self.finish_text_first();
        let doc = &self.session.doc;
        let content = self.session.preview_world_bounds(doc.root());
        let mut tx = ondin_core::kit::make_local(doc, tops);
        if tx.0.is_empty() {
            return;
        }
        let placed = super::updates::stack_local(doc, &tx, tops, content);
        tx.0.extend(placed);
        if !self.session.commit(tx) {
            return;
        }
        let doc = &self.session.doc;
        let names: Vec<String> = tops
            .iter()
            .filter_map(|t| doc.get(*t).map(|n| n.name().to_string()))
            .collect();
        self.session.info(match names.as_slice() {
            [one] => format!("{one} is now a local main, at the top right of the canvas"),
            many => format!(
                "{} components are now local mains, at the top right of the canvas",
                many.len()
            ),
        });
    }
}

/// The card's source line under the main's name (4A, 4B): the kit glyph, *Atlas
/// kit · v8*, and *· v9 waiting* when an update waits — no badge, no colour.
pub(crate) fn source_line(ui: &mut egui::Ui, k: &KitLine) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        ui.label(theme::icon_text(icon::PACKAGE, 11.0, theme::text::DIM));
        let mut text = format!("{} kit · v{}", k.kit_name, k.version);
        if let Some(v) = k.waiting {
            text.push_str(&format!(" · v{v} waiting"));
        }
        ui.label(
            egui::RichText::new(text)
                .size(11.0)
                .color(color::text_a(140)),
        );
    });
}
