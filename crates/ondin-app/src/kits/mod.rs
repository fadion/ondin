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

pub(crate) mod index;
pub(crate) mod picker;

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
    /// *Rename*, *Duplicate*, *Version history*, *Kits used…* and *Export…* have
    /// no editor verb behind them yet, and a row that does nothing is worse than
    /// a short menu. *Publish kit…* joins with publishing (step 4).
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
        self.dropdown(&ctx, "document-menu", anchor, MENU_W, &head, |app, ui| {
            const H: f32 = crate::ui::MENU_ITEM_H;
            if shared {
                if crate::ui::menu_item(ui, icon::PACKAGE, "Stop sharing as kit", H)
                    .on_hover_text(
                        "Documents using it keep their copies and stop receiving \
                         updates. Making it a kit again continues from its last version.",
                    )
                    .clicked()
                {
                    app.stop_sharing_kit();
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
        self.save_file(false);
        let name = self.session.document_title();
        self.session.info(format!(
            "{name} is a kit — nothing is shared until you publish it"
        ));
    }

    /// *Stop sharing as kit* (1D): the mark stays, no longer sharing, so its
    /// version carries on if the document is made a kit again. Documents that use
    /// it keep their copies; they read it as no longer a kit.
    ///
    /// ⚠️ **No confirmation yet** (§15 D1013): 1D's dialog counts the documents
    /// using the kit, and nothing can count them — the library scan reads each
    /// file's first 4 KB, and which kits a document uses is not in it. The step
    /// that decides where that count comes from (step 4) brings the dialog. The
    /// verb loses nothing meanwhile: *Make this a kit* undoes it exactly.
    pub(crate) fn stop_sharing_kit(&mut self) {
        let mut meta = self.session.doc.meta().clone();
        let Some(kit) = meta.kit.as_mut() else {
            return;
        };
        kit.sharing = false;
        self.session.doc.set_meta(meta);
        self.session.mark_unsaved();
        self.save_file(false);
        let name = self.session.document_title();
        self.session.info(format!(
            "{name} is no longer shared — documents using it keep their copies"
        ));
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
