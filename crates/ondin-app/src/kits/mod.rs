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
pub(crate) mod summary;
pub(crate) mod updates;
pub(crate) mod used;
pub(crate) mod users;

use crate::app::{OndinApp, TOP_GAP, TopMenu};
use crate::menu::Item;
use crate::theme::{self, color, icon};
use eframe::egui;
use ondin_core::meta::KitMark;

/// The document menu's width, from the mockup (1A).
const MENU_W: f32 = 232.0;

/// *Make this a kit*'s tooltip, in the menu and the palette alike.
pub(crate) const MAKE_KIT_TIP: &str = "Other documents can use its components once you publish it";

impl OndinApp {
    /// Whether the document vendors anything from a kit — when *Kits used…* is offered.
    pub(crate) fn uses_kits(&self) -> bool {
        self.session
            .doc
            .vault()
            .and_then(|v| self.session.doc.get(v))
            .is_some_and(|v| !v.children().is_empty())
    }

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
    /// caret beside the name.
    ///
    /// ⚠️ **The menu holds only rows whose verbs exist**: the mockup's *Rename*,
    /// *Duplicate* and *Export…* have no editor verb behind them yet, and a row that
    /// does nothing is worse than a short menu. *Version history…* leads, as the
    /// document's own verb above a separator; on a kit, *Publish kit…* heads the kit
    /// rows, disabled with *No changes* when there is nothing to publish; *Kits
    /// used…* is offered while the vault holds anything.
    pub(crate) fn document_menu(&mut self, ui: &mut egui::Ui, title: String) {
        let ctx = ui.ctx().clone();
        let open = self.open_menu == TopMenu::Document;
        // A head insets its content `HEAD_LEAD`; here the content is the name,
        // which has to stay 12 from the mark, as on the library screen. So the
        // head starts that much to the left and its hover ground reaches back into
        // the gap rather than moving the name.
        ui.add_space(-crate::ui::HEAD_LEAD);
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
        // The *Kit* tag is the top bar's, after the save dot (§15 D1025).
        if self.open_menu != TopMenu::Document {
            return;
        }

        // Left-aligned under the name, as the mockup's menu hangs from it.
        let anchor = egui::pos2(head.rect.left(), head.rect.bottom() + crate::ui::MENU_GAP);
        let shared = self.shared_kit().is_some();
        let publishable = self.kit_changes().is_some_and(|c| c.any());
        let uses_kits = self.uses_kits();
        // Labels and glyphs from the registry the palette reads too.
        let item = |ui: &mut egui::Ui, item: Item| {
            let spec = item.spec();
            crate::ui::menu_item(ui, spec.glyph, spec.label, crate::ui::MENU_ITEM_H)
        };
        self.dropdown(&ctx, "document-menu", anchor, MENU_W, &head, |app, ui| {
            const H: f32 = crate::ui::MENU_ITEM_H;
            // The document's own verbs, above the kit's (1A).
            if item(ui, Item::VersionHistory).clicked() {
                app.open_menu = TopMenu::None;
                app.versions_open = true;
            }
            crate::ui::menu_sep(ui);
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
            if uses_kits && item(ui, Item::KitsUsed).clicked() {
                app.open_menu = TopMenu::None;
                app.kits_used_open = true;
            }
            if shared {
                // Asks first (1D), as the card's row does — so an ellipsis, as on
                // this menu's other dialog rows.
                if item(ui, Item::StopSharingKit).clicked() {
                    app.unsharing_kit = Some(None);
                    app.open_menu = TopMenu::None;
                }
            } else if item(ui, Item::MakeKit)
                .on_hover_text(MAKE_KIT_TIP)
                .clicked()
            {
                app.make_kit();
                app.open_menu = TopMenu::None;
            }
            crate::ui::menu_sep(ui);
            if item(ui, Item::ShowInLibrary).clicked() {
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
        // The save below is of `session.doc`, which holds no typed text until
        // the session ends (`[X3.2-L5-01]`).
        self.finish_text_first();
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

    /// What a paste carrying kit mains says when the kit refuses it: an update
    /// owed for a main this document holds at an older version (§15 D1036 (7)),
    /// a kit built on this one (D1036 (2)), or a kit held here without a mode
    /// the copy read (§15 D1064).
    pub(crate) fn carry_refused(&self, e: &ondin_core::kit::KitError) -> String {
        use ondin_core::kit::KitError;
        let doc = &self.session.doc;
        match e {
            KitError::NeedsUpdate(held) => match self.kit_line(*held) {
                Some(line) => format!("Accept {}'s update first, then paste again", line.kit_name),
                None => "Accept the kit's update first, then paste again".to_string(),
            },
            KitError::Circular(own) => format!(
                "The pasted component uses this kit's {}, so it cannot be pasted here",
                doc.get(*own).map_or("component", |n| n.name())
            ),
            // The kit's modes held here at another version than the copy's
            // (§15 D1064, `tokens::plan`).
            KitError::Modes(kit) => {
                match doc.variables().kits().get(kit).and_then(|k| k.name.clone()) {
                    Some(name) => format!("Accept {name}'s update first, then paste again"),
                    None => "Accept the kit's update first, then paste again".to_string(),
                }
            }
            // The carry answers only the three above; this is for a future one,
            // worded rather than printed (D1024: no error prints its Debug).
            _ => "That paste could not be placed — the kit it carries is incomplete".to_string(),
        }
    }

    /// *Stop sharing as kit* (1D): the mark stays, no longer sharing, so its
    /// version carries on if the document is made a kit again. Documents that use
    /// it keep their copies; they read it as no longer a kit. Reached through
    /// [`OndinApp::stop_sharing_dialog`], which asks first.
    pub(crate) fn stop_sharing_kit(&mut self) {
        // As `make_kit`: the save holds no typed text until the session ends.
        self.finish_text_first();
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
    ///
    /// 🚨 **On the dashboard every file is closed** (§15 D1036 (6), the v0.5.0
    /// release review's `[X3.1-L5-01]`), the one the session still holds
    /// included: that copy is the editor's, behind the library, and saving it
    /// here wrote it over whatever had reached the disk since — another
    /// window's or a synced machine's publish — so a re-made kit could publish a
    /// version its consumers already held. The file is changed on disk, and the
    /// held copy's mark patched to match, as the dashboard's rename and move
    /// patch it.
    pub(crate) fn set_kit_at(&mut self, path: &std::path::Path, sharing: bool) {
        if self.view != crate::app::View::Dashboard && self.session.path.as_deref() == Some(path) {
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
        if self.session.path.as_deref() == Some(path)
            && let Some(on_disk) = self.library.entry_at(path).map(|e| e.meta.clone())
        {
            let mut meta = self.session.doc.meta().clone();
            meta.id = on_disk.id;
            meta.kit = on_disk.kit;
            self.session.doc.set_meta(meta);
        }
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
        // The document menu's, or a card's for the file the editor holds — but
        // on the dashboard every file is read from the library, as
        // `set_kit_at` writes it there (§15 D1036 (6)).
        let here = target.as_deref().is_none_or(|p| {
            self.view != crate::app::View::Dashboard && self.session.path.as_deref() == Some(p)
        });
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
            // From the editor the library is as last scanned, and nothing
            // rescans a kit's own window — a consumer filed since went
            // uncounted and the card said no document used the kit (the v0.5.0
            // release review's `[X3.1-L1-02]`). The dashboard refreshed on the
            // way in.
            if self.view != crate::app::View::Dashboard {
                self.library.refresh();
            }
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

/// The neutral *Kit* tag after the save dot (1C): a package glyph and the word
/// on an 8% chip — so the author always knows this file reaches others. Neutral
/// rather than the component violet, which belongs to selection (§15 D1001,
/// D1011 (1)).
///
/// **Publish's box exactly** (§15 D1025): 26 tall, 10 in from each end, 7
/// between glyph and word, the same corner — the two sit side by side and a
/// 20-tall chip against a 26-tall button read as two different sizes of one
/// thing.
pub(crate) fn kit_tag(ui: &mut egui::Ui) {
    const H: f32 = 26.0;
    const PAD: f32 = 10.0;
    const GAP: f32 = 7.0;
    let ink = color::text_a(191);
    let glyph = egui::WidgetText::from(theme::icon_text(icon::PACKAGE, 13.0, ink)).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Body,
    );
    let word = ui
        .ctx()
        .fonts_mut(|f| f.layout_no_wrap("Kit".into(), egui::FontId::proportional(11.5), ink));
    let w = PAD + glyph.size().x + GAP + word.size().x + PAD;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, H), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, egui::CornerRadius::same(6), color::text_a(20));
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
