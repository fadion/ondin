//! The kit chrome (§5.3e, §15 D1011–D1014) — the document menu that makes a
//! document a kit and the *Kit* tag that says it is one (here), the library's kits
//! as the picker reads them ([`index`]), and the floating insert picker
//! ([`picker`]).
//!
//! The model is `ondin_core::kit`; this module is the editor's face of it. A kit
//! is a document whose `meta.kit` mark is present and sharing
//! (`ondin_core::meta::KitMark`): marking writes that block and saves, because
//! the mark is an outward act — *Save* and *Export* are its company — and is not
//! undoable (§15 D1011 (2)).

pub(crate) mod card;
pub(crate) mod index;
pub(crate) mod picker;
pub(crate) mod publish;
pub(crate) mod updates;
pub(crate) mod used;
pub(crate) mod users;

use crate::app::{OndinApp, TOP_GAP, TopMenu};
use crate::theme::{self, color, icon};
use eframe::egui;
use ondin_core::meta::KitMark;

/// The document menu's width, from the mockup (1A).
const MENU_W: f32 = 232.0;

impl OndinApp {
    /// This document's kit mark, when it is a kit that is sharing — what the top
    /// bar's tag and the menu's kit rows ask. A mark that stopped sharing is kept
    /// for its version (1D) and answers `None` here.
    pub(crate) fn shared_kit(&self) -> Option<&KitMark> {
        self.session
            .doc
            .meta()
            .kit
            .as_ref()
            .filter(|k| k.is_shared())
    }

    /// The document's title as the **document menu's head** (§5.3e, 1A) — the
    /// caret beside the name — then the neutral *Kit* tag when it is one (1C).
    ///
    /// ⚠️ **The menu holds only rows whose verbs exist** (§15 D1013): the mockup's
    /// *Rename*, *Duplicate*, *Version history* and *Export…* have no editor verb
    /// behind them yet, and a row that does nothing is worse than a short menu.
    /// On a kit, *Publish kit…* comes first, disabled with *No changes* when there
    /// is nothing to publish (2D, §15 D1015); *Kits used…* joined with its verb
    /// (§15 D1016), offered while the vault holds anything.
    pub(crate) fn document_menu(&mut self, ui: &mut egui::Ui, title: String) {
        let ctx = ui.ctx().clone();
        let open = self.open_menu == TopMenu::Document;
        // A head pads its content `HEAD_PAD` each side to match an icon button's
        // optical box; here the content is the name, which has to stay exactly
        // where it was before it became a menu — 12 from the mark, as on the
        // library screen (§15 D962). So the head starts that much to the left and
        // its hover ground reaches back into the gap rather than moving the name.
        ui.add_space(-crate::ui::HEAD_PAD);
        let head = crate::ui::menu_head(
            ui,
            egui::RichText::new(title)
                .size(13.0)
                .color(theme::text::STRONG),
            TOP_GAP,
            open,
        )
        .on_hover_text("Document");
        if head.clicked() {
            self.toggle_menu(TopMenu::Document);
        }
        if self.shared_kit().is_some() {
            kit_tag(ui);
        }
        if self.open_menu != TopMenu::Document {
            return;
        }

        // Left-aligned under the name, as the mockup's menu hangs from it.
        let anchor = egui::pos2(head.rect.left(), head.rect.bottom() + crate::ui::MENU_GAP);
        let shared = self.shared_kit().is_some();
        let publishable = self.kit_changes().is_some_and(|c| c.any());
        let uses_kits = self
            .session
            .doc
            .vault()
            .and_then(|v| self.session.doc.get(v))
            .is_some_and(|v| !v.children().is_empty());
        self.dropdown(&ctx, "document-menu", anchor, MENU_W, &head, |app, ui| {
            const H: f32 = crate::ui::MENU_ITEM_H;
            if shared {
                // *Publish kit…*, disabled with its reason when nothing would go
                // out (2D).
                let row = crate::ui::MenuRow {
                    accel: Some(if publishable {
                        "Ctrl+Alt+P"
                    } else {
                        "No changes"
                    }),
                    enabled: publishable,
                    ..crate::ui::MenuRow::new(icon::UPLOAD_SIMPLE, "Publish kit…")
                };
                if crate::ui::menu_row(ui, row, H).clicked() && publishable {
                    app.open_menu = TopMenu::None;
                    app.open_publish();
                }
            }
            // *Kits used…* (6A), between the publish and the mark (7A) — offered
            // when the document vendors from any kit.
            if uses_kits && crate::ui::menu_item(ui, icon::PACKAGE, "Kits used…", H).clicked() {
                app.open_menu = TopMenu::None;
                app.kits_used_open = true;
            }
            if shared {
                // Asks first (1D), as the card's row does — so an ellipsis, as on
                // this menu's other dialog rows (D1011's name for it).
                if crate::ui::menu_item(ui, icon::PACKAGE, "Stop sharing as kit…", H).clicked() {
                    app.unsharing_kit = Some(None);
                    app.open_menu = TopMenu::None;
                }
            } else if crate::ui::menu_item(ui, icon::PACKAGE, "Make this a kit", H)
                .on_hover_text("Other documents can use its components once you publish it")
                .clicked()
            {
                app.make_kit();
                app.open_menu = TopMenu::None;
            }
            crate::ui::menu_sep(ui);
            if crate::ui::menu_item(ui, icon::FOLDER_OPEN, "Show in library", H).clicked() {
                app.open_menu = TopMenu::None;
                app.go_to_dashboard();
            }
        });
    }

    /// *Make this a kit* (§5.3e, 1A): write the mark and save. One click and safe —
    /// nothing is shared until the first publish.
    ///
    /// **An unfiled document is given its identity here** (§15 D1013, D1010's
    /// premise (a)): a kit is found by `meta.id` through the library scan, so a
    /// mark on a document with none would be a kit nobody could ever find. The id
    /// is minted the way filing mints one (`library::store::file_document`), and
    /// the save that follows files a document that has no path yet. A mark that
    /// stopped sharing is turned back on with its version kept, so a re-made kit
    /// continues from its last publish (1D).
    pub(crate) fn make_kit(&mut self) {
        let mut meta = self.session.doc.meta().clone();
        if meta.id.is_none() {
            meta.id = Some(crate::library::ids::mint());
        }
        meta.kit.get_or_insert_with(KitMark::default).sharing = true;
        self.session.doc.set_meta(meta);
        self.session.mark_unsaved();
        // A failed save's own message stands — the status line has one slot
        // (`D1013`'s fix); the mark is in the document and goes out with the
        // next save that works.
        if !self.save_file(false) {
            return;
        }
        let name = self.session.document_title();
        self.session.info(format!(
            "{name} is a kit — nothing is shared until you publish it"
        ));
    }

    /// *Stop sharing as kit* (1D): the mark stays, no longer sharing, so its
    /// version carries on if the document is made a kit again. Documents that use
    /// it keep their copies; they read it as no longer a kit. Reached through
    /// [`OndinApp::stop_sharing_dialog`], which asks first.
    pub(crate) fn stop_sharing_kit(&mut self) {
        let mut meta = self.session.doc.meta().clone();
        let Some(kit) = meta.kit.as_mut() else {
            return;
        };
        kit.sharing = false;
        self.session.doc.set_meta(meta);
        self.session.mark_unsaved();
        // A failed save's own message stands — the status line has one slot
        // (`D1013`'s fix); the mark is in the document and goes out with the
        // next save that works.
        if !self.save_file(false) {
            return;
        }
        let name = self.session.document_title();
        self.session.info(format!(
            "{name} is no longer shared — documents using it keep their copies"
        ));
    }

    /// The card menu's *Make a kit* / *Stop sharing as kit* on a library file
    /// (1B). The open document goes through the editor's own verbs, which save
    /// it; a closed one has its mark written into the file
    /// (`library::store::set_kit`). Stopping is confirmed first (1D) — this is
    /// what the confirmation runs.
    pub(crate) fn set_kit_at(&mut self, path: &std::path::Path, sharing: bool) {
        if self.session.path.as_deref() == Some(path) {
            if sharing {
                self.make_kit();
            } else {
                self.stop_sharing_kit();
            }
            self.library.refresh();
            return;
        }
        let Some(entry) = self.library.entry_at(path).cloned() else {
            return;
        };
        if let Err(e) = crate::library::store::set_kit(&entry, sharing) {
            self.session
                .fail(format!("Could not change {}: {e}", entry.display_name()));
            return;
        }
        self.library.refresh();
        let name = entry.display_name();
        self.session.info(if sharing {
            format!("{name} is a kit — nothing is shared until you publish it")
        } else {
            format!("{name} is no longer shared — documents using it keep their copies")
        });
    }

    /// *Stop sharing as kit*'s confirmation (§5.3e, 1D), for the document menu
    /// and the card menu alike: how many documents use the kit and what happens
    /// to them, and that making it a kit again continues from its last version.
    ///
    /// **The count is read by loading every library file on a worker** (§15
    /// D1021, [`users`]), started when the dialog opens and dropped when it
    /// closes, so each opening counts afresh; until it arrives the first sentence
    /// stands without a number, as it did before there was one (§15 D1017).
    pub(crate) fn stop_sharing_dialog(&mut self, ctx: &egui::Context) {
        let Some(target) = self.unsharing_kit.clone() else {
            self.kit_users = None;
            return;
        };
        let here = target
            .as_deref()
            .is_none_or(|p| self.session.path.as_deref() == Some(p));
        let found = if here {
            self.session
                .doc
                .meta()
                .kit
                .as_ref()
                .map(|k| (self.session.document_title(), k.version))
        } else {
            target
                .as_deref()
                .and_then(|p| self.library.entry_at(p))
                .and_then(|e| e.meta.kit.as_ref().map(|k| (e.display_name(), k.version)))
        };
        let Some((name, version)) = found else {
            self.unsharing_kit = None;
            return;
        };
        let kit_id = if here {
            self.session.doc.meta().id.clone()
        } else {
            target
                .as_deref()
                .and_then(|p| self.library.entry_at(p))
                .and_then(|e| e.meta.id.clone())
        };
        if let Some(id) = kit_id
            && self.kit_users.as_ref().is_none_or(|u| u.kit != id)
        {
            // Every library file but the kit's own and its conflict copies,
            // which carry its id and never vendor it.
            let paths = self
                .library
                .entries
                .iter()
                .filter(|e| e.meta.id.as_deref() != Some(id.as_str()))
                .map(|e| e.path.clone())
                .collect();
            self.kit_users = Some(users::KitUsers::start(id, paths));
        }
        if self.kit_users.as_mut().is_some_and(|u| u.pass()) {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        let count = self.kit_users.as_ref().and_then(|u| u.count);
        let mut decision: Option<bool> = None;
        let modal = crate::settings::card("stop-sharing-kit", ctx, |ui| {
            ui.set_width(crate::ui::menu_inner_w(380.0, crate::settings::PAD));
            if crate::settings::modal_title(ui, &format!("Stop sharing {name} as a kit?")) {
                decision = Some(false);
            }
            ui.add_space(8.0);
            let again = if version == 0 {
                "You can make it a kit again later.".to_string()
            } else {
                format!(
                    "You can make it a kit again later. Publishing then continues from v{version}."
                )
            };
            for line in [users::sentence(&name, count), again] {
                ui.label(
                    egui::RichText::new(line)
                        .size(12.0)
                        .color(theme::text::MUTED),
                );
                ui.add_space(4.0);
            }
            ui.add_space(crate::settings::FOOTER_GAP);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if crate::settings::text_button(ui, "Stop sharing", crate::ui::FieldButton::On)
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
        // `Escape` and the backdrop are *Cancel*, as on every card (§15 D525).
        if modal.should_close() {
            decision = Some(false);
        }
        match decision {
            Some(true) => {
                self.unsharing_kit = None;
                match target {
                    Some(path) => self.set_kit_at(&path, false),
                    None => self.stop_sharing_kit(),
                }
            }
            Some(false) => self.unsharing_kit = None,
            None => {}
        }
        if self.unsharing_kit.is_none() {
            self.kit_users = None;
        }
    }
}

/// The neutral *Kit* tag after the document's name (1C): a package glyph and the
/// word on an 8% chip, 20 tall — so the author always knows this file reaches
/// others. Neutral rather than the component violet, which belongs to selection
/// (§15 D1001, D1011 (1)).
fn kit_tag(ui: &mut egui::Ui) {
    const H: f32 = 20.0;
    const PAD: f32 = 7.0;
    const GAP: f32 = 5.0;
    let ink = color::text_a(191);
    let glyph = egui::WidgetText::from(theme::icon_text(icon::PACKAGE, 12.0, ink)).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Body,
    );
    let word = ui
        .ctx()
        .fonts_mut(|f| f.layout_no_wrap("Kit".into(), egui::FontId::proportional(10.5), ink));
    let w = PAD + glyph.size().x + GAP + word.size().x + PAD;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, H), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, egui::CornerRadius::same(5), color::text_a(20));
    let mid = rect.center().y;
    p.galley(
        egui::pos2(rect.left() + PAD, mid - glyph.size().y / 2.0),
        glyph.clone(),
        ink,
    );
    p.galley(
        egui::pos2(
            rect.left() + PAD + glyph.size().x + GAP,
            mid - word.size().y / 2.0,
        ),
        word,
        ink,
    );
    resp.on_hover_text("This document is a kit: other documents can use what it publishes");
}
