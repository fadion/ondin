//! The Settings modal — the home for preferences that belong to no panel.
//!
//! Rows that are a door onto a panel stay there (*Slashes in names make folders*
//! and *Re-export on save* live in the export popover). What is here has no panel
//! to hang off: the arrow-key step, the layer tree's guides and fold-on-open, the
//! font new text starts in, and whether the web-font library is offered.
//!
//! **A form, not a switchboard.** Every row is staged and written by *Save
//! changes*; Cancel, Escape, the ✕ and the backdrop all discard. The export
//! popover saves each row on the spot because it adjusts work in progress. Staging
//! also keeps the *web-fonts* row safe: turning it off drops ~1,976 families out of
//! the picker.
//!
//! **Escape discards even mid-field, by decision.** `egui::ModalResponse::should_close`
//! gates only on `is_top_modal` and `!any_popup_open`, and `TextEdit` handles no
//! Escape, so the press reaches the modal whatever has focus (`DragValue` reads it
//! one layer down, to abandon a typed number). Kept because only preference values
//! are lost, and gating `should_close()` would also break backdrop-dismiss, while
//! `ctx.egui_wants_keyboard_input()` is true whenever anything has focus. The
//! alternative, confirm-on-dirty, is blocked: `nudge_field`'s clamp moves the draft
//! on the first frame, so it would fire on an unedited card.
//!
//! **Clear is the one exception:** it empties the font cache immediately and Cancel
//! does not restore it, because a deletion cannot be staged honestly. It reports
//! how much went, and the count beside it is the receipt.

use crate::app::OndinApp;
use crate::input::NudgeStep;
use crate::panels::paint::{PaintSlot, PrefColour};
use crate::prefs::{CanvasPrefs, Prefs, ThemePref};
use crate::theme::{self, color, icon};
use crate::ui::{self, FieldButton, Prefix, Scrub, Suffix};

/// Width of the card, border and padding included.
pub(crate) const CARD_W: f32 = 376.0;
/// The card's inner margin.
pub(crate) const PAD: f32 = 18.0;
/// Clear air above and below a section header — the same number both sides.
///
/// Equal air is safe because the header carries a hairline, which reads as
/// everything below it however the space is divided. Spacing must not accumulate:
/// `item_spacing` and the eyebrow row's own height both add to an `add_space`.
pub(crate) const SECTION_AIR: f32 = 18.0;
/// Height the section header's row is allocated: exactly its galley, since a
/// `ui.horizontal` would add points above and below the label.
const EYEBROW_H: f32 = 12.0;
/// Under the modal's title, **in total**. [`section`] adds [`SECTION_AIR`] above its
/// own header, so the caller subtracts that rather than adding 26 on top.
pub(crate) const TITLE_GAP: f32 = 26.0;
/// Above the footer's two buttons — the same standoff the title gets, and the whole
/// of it, since nothing below adds air of its own.
pub(crate) const FOOTER_GAP: f32 = TITLE_GAP;
/// Between two rows inside one section: the inspector's row pitch.
pub(crate) const ROW_GAP: f32 = ui::CARD_ROW_GAP;
/// A switch row. [`crate::panels`]' feature lists use the same height, and these
/// are the same control in a column.
pub(crate) const SWITCH_H: f32 = 22.0;
/// A value field, and so also the buttons beside one — the app's control height.
pub(crate) const FIELD_H: f32 = ui::CONTROL_H;
/// The footer's buttons, and the *Clear* button in the web-fonts section: the field
/// height they sit among.
const BUTTON_H: f32 = FIELD_H;
/// What a button's label keeps clear of its own corners, per side.
const BUTTON_PAD_X: f32 = ui::LABEL_BUTTON_PAD_X;
/// Between two controls on one row: the inspector's.
pub(crate) const COL_GAP: f32 = ui::CARD_COL_GAP;
/// The *Default font* dropdown: room for a long family name beside its label.
const DEFAULT_FONT_W: f32 = 200.0;
/// The *Canvas* section's controls beside their labels: a colour row with its
/// reset, or the thickness field, so their right and left edges line up. Narrower
/// than [`DEFAULT_FONT_W`], whose labels are shorter.
const PREF_CONTROL_W: f32 = 168.0;

/// Type size of a row's label — [`ui::SWITCH_ROW_LABEL_PT`], because half these
/// rows *are* switch rows and a caption beside one has to match it.
pub(crate) const LABEL_PT: f32 = ui::SWITCH_ROW_LABEL_PT;
/// Type size of the sentence under a row that needs one.
pub(crate) const CAPTION_PT: f32 = ui::CAPTION_PT;
/// Type size of a button's word — [`ui::label_button`]'s at [`BUTTON_H`].
const BUTTON_PT: f32 = ui::CONTROL_PT;

/// What the Settings modal has been edited to, before *Save changes* is pressed.
///
/// A copy of the preferences rather than `Option`s: every row always has a value,
/// and mirroring `Prefs` field for field makes the dirty test one `!=`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    theme: ThemePref,
    nudge: NudgeStep,
    canvas: CanvasPrefs,
    tree_guides: bool,
    collapse_groups_on_open: bool,
    resource_meter: bool,
    default_font: String,
    spell_check: bool,
    web_fonts: bool,
}

impl Settings {
    /// The form as the saved preferences have it — what opening the modal shows.
    fn from_prefs(prefs: &Prefs) -> Self {
        Self {
            theme: prefs.theme,
            nudge: prefs.nudge,
            canvas: prefs.canvas,
            tree_guides: prefs.tree_guides,
            collapse_groups_on_open: prefs.collapse_groups_on_open,
            resource_meter: prefs.resource_meter,
            default_font: prefs.default_font.clone(),
            spell_check: prefs.spell_check,
            web_fonts: prefs.web_fonts,
        }
    }

    /// The form's scrubbable numbers, as a scrub's start snapshots them.
    pub(crate) fn scrubbed(&self) -> Scrubbed {
        Scrubbed {
            nudge: self.nudge,
            guide_thickness: self.canvas.guide_thickness,
        }
    }

    /// Put the scrubbable numbers back — what `OndinApp::cancel_gesture` calls to
    /// undo a right-clicked scrub.
    pub(crate) fn restore(&mut self, s: Scrubbed) {
        self.nudge = s.nudge;
        self.canvas.guide_thickness = s.guide_thickness;
    }

    /// The theme picked in the form, which the chrome previews while the modal
    /// is open (`theme::follow` in `OndinApp::frame_body`).
    pub(crate) fn theme(&self) -> ThemePref {
        self.theme
    }

    /// Write the form back. Returns whether the web-fonts switch moved, because
    /// that is the one row with a consequence beyond the file — see
    /// [`crate::fonts::FontService::set_web_fonts`].
    fn apply(&self, prefs: &mut Prefs) -> bool {
        let web_fonts_changed = prefs.web_fonts != self.web_fonts;
        prefs.theme = self.theme;
        prefs.nudge = self.nudge;
        prefs.canvas = self.canvas;
        prefs.tree_guides = self.tree_guides;
        prefs.collapse_groups_on_open = self.collapse_groups_on_open;
        prefs.resource_meter = self.resource_meter;
        prefs.default_font.clone_from(&self.default_font);
        prefs.spell_check = self.spell_check;
        prefs.web_fonts = self.web_fonts;
        web_fonts_changed
    }
}

/// The numbers a Settings field scrubs, as they were when a scrub began: what a
/// right-click cancel puts back (`InFlight::settings_scrub`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Scrubbed {
    nudge: NudgeStep,
    guide_thickness: u8,
}

/// The settings button's box and glyph on both screens — the editor's
/// [`OndinApp::settings_button`] and the library's — so they stay the same button.
pub(crate) const SETTINGS_BOX: f32 = 28.0;
/// The glyph inside [`SETTINGS_BOX`]: the editor bar's icon size, as undo and redo.
pub(crate) const SETTINGS_GLYPH: f32 = 16.0;

impl OndinApp {
    /// The top bar's rightmost control: the button that opens the modal.
    ///
    /// Drawn in the top bar's `right_to_left` cluster, so it is added first to end
    /// up last.
    pub(crate) fn settings_button(&mut self, ui: &mut egui::Ui) {
        if ui::icon_button(
            ui,
            icon::SLIDERS,
            SETTINGS_BOX,
            SETTINGS_GLYPH,
            self.settings.is_some(),
            true,
        )
        .on_hover_text("Settings")
        .clicked()
        {
            self.toggle_settings(ui.ctx());
        }
    }

    /// Open the modal, or close it discarding whatever was typed into it.
    ///
    /// Opening asks for the cache size once, not the row every frame (a `read_dir`
    /// per frame).
    pub(crate) fn toggle_settings(&mut self, ctx: &egui::Context) {
        // No scrub survives either way: there is nothing to wind back into.
        self.in_flight.settings_scrub = None;
        self.settings = match self.settings {
            Some(_) => None,
            None => {
                self.fonts.measure_cache(ctx);
                Some(Settings::from_prefs(&self.prefs))
            }
        };
    }

    /// Draw the modal, if it is open, and act on whichever way it was closed.
    ///
    /// The keymap is silenced while this is open (the guard in
    /// `OndinApp::frame_body`); otherwise an arrow key would nudge the
    /// selection behind the backdrop.
    pub(crate) fn settings_ui(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.settings.clone() else {
            return;
        };
        let saved = Settings::from_prefs(&self.prefs);
        // A picker open on one of the form's colours takes Escape first, so the press
        // closes it rather than discarding the whole form.
        if self
            .picker
            .as_ref()
            .is_some_and(|p| p.slot.pref_scoped().is_some())
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.picker = None;
        }

        let mut decision = None;
        let modal = card("settings", ctx, |ui| {
            ui.set_width(ui::menu_inner_w(CARD_W, PAD));
            // Zero, so every vertical gap is written out: an `add_space` is added to
            // `item_spacing.y` (see [`SECTION_AIR`]).
            ui.spacing_mut().item_spacing.y = 0.0;

            // --- title -------------------------------------------------
            if modal_title(ui, "Settings") {
                decision = Some(Decision::Cancel);
            }

            // --- appearance --------------------------------------------
            ui.add_space(TITLE_GAP - SECTION_AIR);
            theme_section(ui, &mut draft.theme);
            ui.add_space(ROW_GAP);
            if ui::switch_row(ui, "Show resource meter", draft.resource_meter, SWITCH_H)
                .on_hover_text("CPU and memory in the top bar")
                .clicked()
            {
                draft.resource_meter = !draft.resource_meter;
            }

            // --- nudge -------------------------------------------------
            section(ui, "Nudge");
            // Before the fields: `value_field` applies this frame's drag delta inside
            // the call that reports `drag_started()`.
            let before = draft.scrubbed();
            let half = egui::vec2((ui.available_width() - COL_GAP) / 2.0, FIELD_H);
            let mut dragging = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = COL_GAP;
                    let a = nudge_field(ui, half, "Step", &mut draft.nudge.small);
                    let b = nudge_field(ui, half, "Shift", &mut draft.nudge.large);
                    a || b
                })
                .inner;

            // --- canvas ------------------------------------------------
            section(ui, "Canvas");
            dragging |= self.canvas_rows(ui, &mut draft.canvas);
            // The cancel is answered before the snapshot is armed (as in
            // `panels::export`'s quality field): on the frame a gesture is called
            // off, nothing may re-arm it.
            if dragging && !self.gesture_cancelled && self.in_flight.settings_scrub.is_none() {
                self.in_flight.settings_scrub = Some(before);
            }

            // --- layers ------------------------------------------------
            section(ui, "Layers");
            if ui::switch_row(ui, "Show tree guides", draft.tree_guides, SWITCH_H)
                .on_hover_text("Dashed lines from a frame or group down to the layers inside it")
                .clicked()
            {
                draft.tree_guides = !draft.tree_guides;
            }
            ui.add_space(ROW_GAP);
            if ui::switch_row(
                ui,
                "Collapse groups on open",
                draft.collapse_groups_on_open,
                SWITCH_H,
            )
            .on_hover_text("Opening a document folds every group and frame in the tree")
            .clicked()
            {
                draft.collapse_groups_on_open = !draft.collapse_groups_on_open;
            }

            // --- text --------------------------------------------------
            section(ui, "Text");
            if let Some(family) = self.default_font_row(ui, &draft.default_font) {
                draft.default_font = family;
            }
            caption(
                ui,
                "New text layers start in this font, or in Inter where it is neither \
                 installed nor downloadable.",
            );
            ui.add_space(ROW_GAP);
            if ui::switch_row(ui, "Check spelling", draft.spell_check, SWITCH_H)
                .on_hover_text(
                    "Underline misspelled words while editing text, in each run's language",
                )
                .clicked()
            {
                draft.spell_check = !draft.spell_check;
            }

            // --- web fonts ---------------------------------------------
            section(ui, "Web fonts");
            if ui::switch_row(ui, "Load web fonts", draft.web_fonts, SWITCH_H).clicked() {
                draft.web_fonts = !draft.web_fonts;
            }
            // A caption rather than a tooltip: the label names a capability, and
            // "off" is chosen deliberately (metered connection, no CDN), so the
            // sentence must be read before flipping. "No *font* request" because
            // the update check is another network source with its own switch.
            caption(
                ui,
                "Off is the fonts this machine has installed, plus Inter. \
                     Nothing is downloaded and no font request is made.",
            );
            ui.add_space(ROW_GAP);
            self.font_cache_row(ui);

            // --- footer ------------------------------------------------
            if let Some(d) = modal_footer(ui, "Save changes", draft != saved) {
                decision = Some(d);
            }
        });

        // The two colours are written through their paint slot, by the hex fields
        // above and by the picker, into `self.settings`; the rest of the form into
        // `draft`. Taken back before the draft is saved or written over them.
        if let Some(live) = &self.settings {
            draft.canvas.guide_colour = live.canvas.guide_colour;
            draft.canvas.grid_ink = live.canvas.grid_ink;
        }
        // The backdrop and Escape both mean Cancel. Read after the body, because
        // `should_close` consumes the Escape press.
        if modal.should_close() {
            decision = Some(Decision::Cancel);
        }

        match decision {
            Some(Decision::Cancel) => {
                self.settings = None;
                self.in_flight.settings_scrub = None;
            }
            Some(Decision::Save) => {
                if draft.apply(&mut self.prefs) {
                    self.fonts.set_web_fonts(self.prefs.web_fonts, ctx);
                }
                self.ensure_default_font();
                self.prefs.save();
                self.settings = None;
                self.in_flight.settings_scrub = None;
            }
            // Still open: keep whatever was typed into it this frame.
            None => self.settings = Some(draft),
        }
    }

    /// The *Canvas* section's rows: the wheel-zoom switch, the guide colour and thickness, the adaptive
    /// grid switch, and the grid's own colour, dimmed while adaptive is on. The
    /// colours go through [`PaintSlot::Pref`]; the rest into `canvas`. Returns whether
    /// the thickness is being scrubbed.
    fn canvas_rows(&mut self, ui: &mut egui::Ui, canvas: &mut CanvasPrefs) -> bool {
        let defaults = CanvasPrefs::default();
        if ui::switch_row(
            ui,
            "Zoom with the scroll wheel",
            canvas.wheel_zooms,
            SWITCH_H,
        )
        .on_hover_text("A mouse wheel zooms at the cursor; Ctrl or Alt with it pans")
        .clicked()
        {
            canvas.wheel_zooms = !canvas.wheel_zooms;
        }
        caption(
            ui,
            "Ctrl or Alt with the wheel pans instead. A touchpad's two-finger scroll \
             still pans.",
        );
        ui.add_space(ROW_GAP);
        self.pref_colour_row(
            ui,
            PrefColour::Guide,
            canvas.guide_colour != defaults.guide_colour,
        );
        ui.add_space(ROW_GAP);
        let mut thickness = f64::from(canvas.guide_thickness);
        let (lo, hi) = (CanvasPrefs::GUIDE_MIN, CanvasPrefs::GUIDE_MAX);
        let dragging = labelled_row(ui, "Ruler guides thickness", |ui| {
            let (resp, _) = ui::value_field_suffixed(
                ui,
                egui::vec2(PREF_CONTROL_W, FIELD_H),
                Prefix::Text(""),
                Some(Suffix {
                    text: "pt",
                    clickable: false,
                    tooltip: "",
                }),
                &mut thickness,
                // As slow as the nudge fields: five whole steps.
                Scrub::whole(0.1).range(f64::from(lo)..=f64::from(hi)),
                |d| d.custom_formatter(ui::number(0)),
            );
            resp.on_hover_text("How thick a guide is drawn; a selected one is a point thicker")
                .dragged()
        });
        canvas.guide_thickness = (thickness.round() as u8).clamp(lo, hi);
        ui.add_space(ROW_GAP);
        if ui::switch_row(ui, "Adaptive pixel grid", canvas.grid_adaptive, SWITCH_H)
            .on_hover_text("Shade the grid from the pixels under it")
            .clicked()
        {
            canvas.grid_adaptive = !canvas.grid_adaptive;
        }
        caption(
            ui,
            "Each grid line is a shade lighter or darker than the pixel under it, \
             so the grid shows on any fill.",
        );
        ui.add_space(ROW_GAP);
        ui.add_enabled_ui(!canvas.grid_adaptive, |ui| {
            self.pref_colour_row(ui, PrefColour::Grid, canvas.grid_ink != defaults.grid_ink);
        })
        .response
        .on_disabled_hover_text("Turn off Adaptive pixel grid to pick a colour");
        dragging
    }

    /// One colour row: its label, the inspector's swatch-and-hex row on the slot,
    /// and the reset beside it, live while the colour differs from its default.
    fn pref_colour_row(&mut self, ui: &mut egui::Ui, which: PrefColour, resettable: bool) {
        let slot = PaintSlot::Pref(which);
        let (label, edit, reset) = match which {
            PrefColour::Guide => (
                "Ruler guides colour",
                "Edit the default guide colour",
                ("Reset the guide colour", "Already the default guide colour"),
            ),
            PrefColour::Grid => (
                "Pixel grid colour",
                "Edit the pixel grid colour",
                (
                    "Reset the pixel grid colour",
                    "Already the default pixel grid colour",
                ),
            ),
        };
        let Some(colour) = self.pref_colour(which) else {
            return;
        };
        let row_y = ui.cursor().min.y;
        let opened = labelled_row(ui, label, |ui| {
            let reset_w = FIELD_H;
            let (live, dead) = reset;
            if ui::field_button(
                ui,
                icon::ARROW_COUNTER_CLOCKWISE,
                reset_w,
                FieldButton::enabled_if(resettable),
            )
            .on_hover_text(if resettable { live } else { dead })
            .clicked()
                && resettable
            {
                let to = match which {
                    PrefColour::Guide => CanvasPrefs::default().guide_colour,
                    PrefColour::Grid => CanvasPrefs::default().grid_ink,
                };
                self.write_pref_colour(which, &ondin_core::Brush::Solid(to));
            }
            let size = egui::vec2(PREF_CONTROL_W - reset_w - COL_GAP, FIELD_H);
            self.single_paint_row(ui, size, slot, colour, false, edit)
        });
        if opened && let Some(card) = ui.ctx().memory(|m| m.area_rect(egui::Id::new("settings"))) {
            self.open_picker_beside(ui.ctx(), slot, card, row_y);
        }
    }

    /// The Settings draft's colour for `which`, or `None` while the modal is shut.
    pub(crate) fn pref_colour(&self, which: PrefColour) -> Option<ondin_core::peniko::Color> {
        let canvas = &self.settings.as_ref()?.canvas;
        Some(match which {
            PrefColour::Guide => canvas.guide_colour,
            PrefColour::Grid => canvas.grid_ink,
        })
    }

    /// Put `brush`'s colour in the Settings draft — every slot verb's route for a
    /// [`PaintSlot::Pref`]. A guide's is made opaque. Whether there was a draft to write.
    pub(crate) fn write_pref_colour(
        &mut self,
        which: PrefColour,
        brush: &ondin_core::Brush,
    ) -> bool {
        let Some((_, colour)) = crate::panels::paint::stops_of(brush).first().copied() else {
            return false;
        };
        let Some(settings) = self.settings.as_mut() else {
            return false;
        };
        match which {
            PrefColour::Guide => settings.canvas.guide_colour = colour.with_alpha(1.0),
            PrefColour::Grid => settings.canvas.grid_ink = colour,
        }
        true
    }

    /// *`180 MB` of web font cache · Clear*. The number is the point: the cache is
    /// bounded and swept at startup, but the user could not see its size.
    fn font_cache_row(&mut self, ui: &mut egui::Ui) {
        let (text, clearable) = cache_row(self.fonts.cache_bytes());
        // The button is allocated before the label, and the nesting is what makes
        // that so: a `ui.horizontal` centres each item against the row height known
        // when it is allocated, so a label written first sits 2pt high once the 28pt
        // button grows the row. Setting `interact_size.y` does not fix it.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = COL_GAP;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let clear = text_button(
                    ui,
                    "Clear",
                    if clearable {
                        FieldButton::Off
                    } else {
                        FieldButton::Disabled
                    },
                )
                .on_hover_text(
                    "Delete the downloaded font files. The fonts in this document keep working",
                );
                if clear.clicked() && clearable {
                    let freed = self.fonts.clear_cache();
                    // Said out loud: the row changes and the click's yield is lost.
                    self.session.info(format!(
                        "Cleared {} of cached web fonts",
                        human_bytes(freed)
                    ));
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(text)
                            .size(LABEL_PT)
                            .color(theme::text::muted()),
                    );
                });
            });
        });
    }

    /// *Default font* and the family dropdown, the typography panel's list in it.
    /// Returns the family picked, for the draft. A family that can never be had
    /// wears its glyph in `warn()`, as in the panel.
    fn default_font_row(&mut self, ui: &mut egui::Ui, current: &str) -> Option<String> {
        let mut chosen = None;
        let missing = self.fonts.family_status(current) == crate::fonts::FamilyStatus::Missing;
        let head = if missing {
            ui::glyph_and_text_tinted(icon::TEXT_AA, current, color::warn())
        } else {
            ui::glyph_and_text(icon::TEXT_AA, current)
        };
        // Allocated right to left for `font_cache_row`'s reason.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = COL_GAP;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().interact_size.y = FIELD_H;
                ui.spacing_mut().button_padding.y = 0.0;
                let combo = egui::ComboBox::from_id_salt("settings-default-font")
                    .icon(ui::combo_chevron)
                    .width(DEFAULT_FONT_W)
                    // The popup holds a search field, so a click inside must not dismiss it.
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .height(crate::panels::FAMILY_POPUP_H)
                    .selected_text(head)
                    .show_ui(ui, |ui| {
                        ui::menu_rows(ui);
                        ui.spacing_mut().item_spacing.y = 3.0;
                        chosen = self.family_list(ui, current);
                    })
                    .response;
                if missing {
                    combo.on_hover_text(
                        "This font is not installed and cannot be downloaded — \
                         new text starts in Inter",
                    );
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("Default font")
                            .size(LABEL_PT)
                            .color(ui::switch_row_ink(true)),
                    );
                });
            });
        });
        chosen
    }
}

/// What the font-cache row says, and whether *Clear* does anything.
///
/// Three states: `None` is a scan that has not landed and `Some(0)` an empty cache,
/// and rendering both as `0 MB` would report a full cache as clear. Split out so
/// the rule is testable without an app.
fn cache_row(bytes: Option<u64>) -> (String, bool) {
    match bytes {
        None => ("Counting the web font cache…".to_owned(), false),
        Some(0) => ("Nothing in the web font cache".to_owned(), false),
        Some(n) => (format!("{} of web font cache", human_bytes(n)), true),
    }
}

/// [`card_modal`] shown, holding `Tab` among its own controls ([`ui::hold_tab`]).
pub(crate) fn card<R>(
    id: &str,
    ctx: &egui::Context,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::ModalResponse<R> {
    card_modal(ctx, id).show(ctx, |ui| {
        ui::hold_tab(ui);
        let out = body(ui);
        // The card's rect, frame included, so the window's own controls laid over
        // the backdrop keep clear of it (`chrome::above_modal`).
        let rect = ui.min_rect().expand(PAD);
        crate::chrome::note_card(ui.ctx(), rect);
        out
    })
}

/// The card every settings-shaped modal is drawn on: the ground, the hairline,
/// the corner, the shadow, [`PAD`] of inner margin, and a half-black backdrop.
///
/// Shared so every settings-shaped modal is the same object. The caller sets its
/// own width: [`CARD_W`] is this file's, and a modal with a wider row may be wider.
///
/// Centred on [`set_card_centre`]'s point when one is set (the canvas, as the
/// command palette is), else on the window. The move off the window's centre is
/// cut short [`CARD_EDGE`] from the edge, measured on last frame's card: not
/// `Area::constrain_to`, which clips the backdrop the card blocks clicks with.
pub(crate) fn card_modal(ctx: &egui::Context, id: &str) -> egui::Modal {
    let id = egui::Id::new(id);
    let screen = ctx.content_rect();
    let size = ctx
        .memory(|m| m.area_rect(id))
        .map_or(egui::Vec2::ZERO, |r| r.size());
    let room = ((screen.size() - size) / 2.0 - egui::Vec2::splat(CARD_EDGE)).max(egui::Vec2::ZERO);
    let offset = ctx
        .data(|d| d.get_temp::<egui::Pos2>(card_centre_id()))
        .map_or(egui::Vec2::ZERO, |c| {
            (c - screen.center()).clamp(-room, room)
        });
    egui::Modal::new(id)
        .area(egui::Modal::default_area(id).anchor(egui::Align2::CENTER_CENTER, offset))
        // Half black: a modal over a canvas must dim more than
        // egui's default 100/255, or a light document still reads as in focus.
        .backdrop_color(egui::Color32::from_black_alpha(128))
        .frame(card_frame().inner_margin(egui::Margin::same(PAD as i8)))
}

/// [`card_modal`]'s ground, hairline, corner and shadow, without its margin: also
/// worn by every popup over the canvas, the command palette and Colour review
/// among them.
pub(crate) fn card_frame() -> egui::Frame {
    ui::card_frame()
}

/// The least room a card moved toward the canvas keeps from the window's edge.
pub(crate) const CARD_EDGE: f32 = 24.0;

/// Where this frame's cards centre: `Some` the canvas's middle in the editor,
/// `None` the window's. Written by `OndinApp::ui` before any card is shown.
pub(crate) fn set_card_centre(ctx: &egui::Context, centre: Option<egui::Pos2>) {
    ctx.data_mut(|d| match centre {
        Some(c) => {
            d.insert_temp(card_centre_id(), c);
        }
        None => d.remove::<egui::Pos2>(card_centre_id()),
    });
}

fn card_centre_id() -> egui::Id {
    egui::Id::new("card-centre")
}

/// The title row every settings-shaped modal opens with: the name on the left, the
/// ✕ against the right margin. Returns whether the ✕ was pressed.
///
/// The ✕ is [`ui::close_button`], as on every popover.
pub(crate) fn modal_title(ui: &mut egui::Ui, title: &str) -> bool {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(title)
                .size(ui::TITLE_PT)
                .color(theme::text::strong()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui::close_button(ui).clicked()
        })
        .inner
    })
    .inner
}

/// A card over the canvas's side inset, tighter than [`PAD`]: its rows run their
/// dividers edge to edge, so the inset is each row's, not the frame's.
pub(crate) const CANVAS_CARD_PAD: f32 = 14.0;
/// A canvas card's title row ([`card_head`]).
pub(crate) const CANVAS_CARD_HEAD_H: f32 = 40.0;
/// A canvas card's footer row ([`card_foot`]).
pub(crate) const CANVAS_CARD_FOOT_H: f32 = 48.0;

/// How a [`canvas_card`] holds the window while it is open.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CardHold {
    /// A modal with a clear backdrop: the selection stays outlined, and a click
    /// outside is answered for the caller to close on.
    Modal,
    /// The canvas stays live under it, and the card takes at most
    /// [`ui::canvas_popup_room`] of height.
    Open,
}

/// The frame every card over the canvas wears: [`card_frame`]'s ground without
/// its margin, `w` wide, hung from [`ui::canvas_popup_area`], with `Tab` held
/// inside and no spacing between rows. `body` lays the rows, [`card_head`] first.
/// Returns whether a [`CardHold::Modal`] card's backdrop was clicked.
pub(crate) fn canvas_card(
    ctx: &egui::Context,
    id: &str,
    canvas: egui::Rect,
    w: f32,
    hold: CardHold,
    body: impl FnOnce(&mut egui::Ui),
) -> bool {
    let area = ui::canvas_popup_area(ctx, egui::Id::new(id), canvas);
    let rows = |ui: &mut egui::Ui| {
        ui.set_width(w);
        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
        ui::hold_tab(ui);
        body(ui)
    };
    match hold {
        CardHold::Modal => {
            let mut modal = card_modal(ctx, id)
                .area(area)
                .backdrop_color(egui::Color32::TRANSPARENT);
            modal.frame = modal.frame.map(|f| f.inner_margin(egui::Margin::ZERO));
            modal.show(ctx, rows).backdrop_response.clicked()
        }
        CardHold::Open => {
            let room = ui::canvas_popup_room(canvas);
            area.show(ctx, |ui| {
                card_frame().show(ui, |ui| {
                    ui.set_max_height(room);
                    rows(ui);
                });
            });
            false
        }
    }
}

/// A canvas card's title row, `w` wide: [`modal_title`] inside
/// [`CANVAS_CARD_PAD`]. Returns whether its ✕ was pressed.
pub(crate) fn card_head(ui: &mut egui::Ui, w: f32, title: &str) -> bool {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, CANVAS_CARD_HEAD_H), egui::Sense::hover());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(CANVAS_CARD_PAD, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    modal_title(&mut child, title)
}

/// A canvas card's footer row, `w` wide: `note` faint on the left, `buttons` on
/// the right as [`footer_row_noted`] lays them, `why_not` the commit's tooltip.
/// Returns the index of the button pressed.
pub(crate) fn card_foot(
    ui: &mut egui::Ui,
    w: f32,
    note: &str,
    buttons: &[(&str, FieldButton)],
    why_not: Option<&str>,
) -> Option<usize> {
    card_foot_with(ui, w, buttons, why_not, |ui, row| {
        ui.painter().text(
            row.left_center(),
            egui::Align2::LEFT_CENTER,
            note,
            egui::FontId::proportional(ui::CAPTION_PT),
            theme::text::faint(),
        );
    })
}

/// [`card_foot`] with its own left side: `lead` draws into the row's content
/// rect, inside [`CANVAS_CARD_PAD`], before the buttons are laid.
pub(crate) fn card_foot_with(
    ui: &mut egui::Ui,
    w: f32,
    buttons: &[(&str, FieldButton)],
    why_not: Option<&str>,
    lead: impl FnOnce(&mut egui::Ui, egui::Rect),
) -> Option<usize> {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, CANVAS_CARD_FOOT_H), egui::Sense::hover());
    let row = rect.shrink2(egui::vec2(CANVAS_CARD_PAD, 10.0));
    lead(ui, row);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(row)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    footer_row_noted(&mut child, buttons, why_not)
}

/// The *Cancel* / commit pair, right-aligned — `commit` is the commit's label,
/// *Save changes* or *Create project* — with the commit
/// [`FieldButton::Primary`] only when `enabled`: there is something to commit.
/// Returns the [`Decision`] a press made.
///
/// Shared so every card's commit lights on the same rule for *edited*.
pub(crate) fn modal_footer(ui: &mut egui::Ui, commit: &str, enabled: bool) -> Option<Decision> {
    let state = if enabled {
        FieldButton::Primary
    } else {
        FieldButton::Disabled
    };
    footer(ui, Some((commit, state)))
}

/// [`modal_footer`] for a card that destroys something: the commit red
/// (`FieldButton::Danger`), the blue commits' twin on the other ramp, and always
/// live — the card opening is the question.
pub(crate) fn danger_footer(ui: &mut egui::Ui, commit: &str) -> Option<Decision> {
    footer(ui, Some((commit, FieldButton::Danger)))
}

/// [`modal_footer`] for a card whose choice is made in its body: *Cancel*
/// alone. Answers whether it was pressed.
pub(crate) fn cancel_footer(ui: &mut egui::Ui) -> bool {
    matches!(footer(ui, None), Some(Decision::Cancel))
}

/// The one footer row: *Cancel*, with `commit`'s label and state to its right.
fn footer(ui: &mut egui::Ui, commit: Option<(&str, FieldButton)>) -> Option<Decision> {
    let mut buttons = vec![("Cancel", FieldButton::Off)];
    buttons.extend(commit);
    footer_buttons(ui, &buttons).map(|i| match i {
        0 => Decision::Cancel,
        _ => Decision::Save,
    })
}

/// Every dialog's footer: `buttons` left to right, right-aligned after
/// [`FOOTER_GAP`], the way out first and the commit last, so the commit is
/// always rightmost. Answers the index pressed; a `Disabled` button answers
/// nothing.
pub(crate) fn footer_buttons(ui: &mut egui::Ui, buttons: &[(&str, FieldButton)]) -> Option<usize> {
    ui.add_space(FOOTER_GAP);
    footer_row(ui, buttons)
}

/// [`footer_buttons`]' row without the gap above, for a popover, whose fields sit
/// closer than a dialog's.
pub(crate) fn footer_row(ui: &mut egui::Ui, buttons: &[(&str, FieldButton)]) -> Option<usize> {
    footer_row_noted(ui, buttons, None)
}

/// [`footer_row`], the last button (the commit) wearing `why_not` as its tooltip:
/// the reason it is `Disabled`.
pub(crate) fn footer_row_noted(
    ui: &mut egui::Ui,
    buttons: &[(&str, FieldButton)],
    why_not: Option<&str>,
) -> Option<usize> {
    let mut pressed = None;
    let row = footer_row_w(ui, buttons);
    // Laid left to right after the slack, not right to left: `Tab` follows the
    // order controls are added in, and it must read the way out first.
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = FOOTER_ROW_GAP;
        ui.add_space((ui.available_width() - row).max(0.0));
        for (i, (label, state)) in buttons.iter().enumerate() {
            let mut resp = text_button(ui, label, *state);
            if i + 1 == buttons.len()
                && let Some(why) = why_not
            {
                resp = resp.on_hover_text(why);
            }
            if resp.clicked() && *state != FieldButton::Disabled {
                pressed = Some(i);
            }
        }
    });
    pressed
}

/// The gap between a footer row's buttons.
const FOOTER_ROW_GAP: f32 = 8.0;

/// The width [`footer_row_noted`] lays `buttons` in, gaps included: what a caller
/// painting a note to the row's left keeps clear.
pub(crate) fn footer_row_w(ui: &egui::Ui, buttons: &[(&str, FieldButton)]) -> f32 {
    let widths: f32 = buttons
        .iter()
        .map(|(label, _)| text_button_w(ui, label))
        .sum();
    widths + FOOTER_ROW_GAP * buttons.len().saturating_sub(1) as f32
}

/// Which way the modal was closed. `None` for "still open" is the third state and
/// lives in the caller's `Option`.
pub(crate) enum Decision {
    Save,
    Cancel,
}

/// The *Appearance* section, first in both screens' Settings: a System / Dark /
/// Light segmented control writing `theme` in the modal's draft.
pub(crate) fn theme_section(ui: &mut egui::Ui, theme: &mut ThemePref) {
    const THEMES: [(ThemePref, &str); 3] = [
        (ThemePref::System, "System"),
        (ThemePref::Dark, "Dark"),
        (ThemePref::Light, "Light"),
    ];
    section(ui, "Appearance");
    let current = THEMES.iter().position(|(t, _)| t == theme).unwrap_or(0);
    if let Some(i) = ui::segmented(
        ui,
        ui.available_width(),
        ui::SEGMENT_CELL_H,
        THEMES.len(),
        current,
        |p, i, rect, on| ui::segment_label(p, rect, THEMES[i].1, on),
    ) {
        *theme = THEMES[i].0;
    }
}

/// A section eyebrow with a hairline running out to the card's edge.
///
/// Uses [`ui::eyebrow`]'s type and grey, as every other section label in the app.
/// The rule separates sections, which this card has no edges to do, and is what
/// lets the air around the header be equal ([`SECTION_AIR`]). The row is allocated
/// at [`EYEBROW_H`], not left to grow to `interact_size.y`.
pub(crate) fn section(ui: &mut egui::Ui, label: &str) {
    ui.add_space(SECTION_AIR);
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), EYEBROW_H),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = COL_GAP;
            ui.label(ui::eyebrow(label));
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::empty());
            // Whole device pixels, as `ui::hairline_in_column` explains.
            let ppp = ui.ctx().pixels_per_point();
            let y = ui::hairline_in_column(rect.center().y, ppp, 1.0);
            ui.painter().line_segment(
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                egui::Stroke::new(1.0, theme::color::divider()),
            );
        },
    );
    ui.add_space(SECTION_AIR);
}

/// A row's label on the left and its controls on the right, laid as
/// *Default font*'s row is: `controls` adds them right to left, first at the edge.
fn labelled_row<R>(ui: &mut egui::Ui, label: &str, controls: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = COL_GAP;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().interact_size.y = FIELD_H;
            let out = controls(ui);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(label)
                        .size(LABEL_PT)
                        .color(ui::switch_row_ink(ui.is_enabled())),
                );
            });
            out
        })
        .inner
    })
    .inner
}

/// A sentence under the row above it, wrapped to the card.
///
/// Closer than [`ROW_GAP`], because a caption belongs to the row above it.
pub(crate) fn caption(ui: &mut egui::Ui, text: &str) {
    ui.add_space(3.0);
    ui.label(
        egui::RichText::new(text)
            .size(CAPTION_PT)
            .color(theme::text::faint()),
    );
}

/// One of the two nudge distances: a [`ui::value_field`] with the word inside it
/// and `px` at the far end.
///
/// The label goes in the field, as the inspector does for `X`, `W` and `Level`.
///
/// Whole pixels only, from 1: a nudge of nothing silently does nothing. Rounding
/// is done here as well as by the scrub, since [`Scrub::whole`] settles a drag but
/// leaves typing alone; it is safe every frame because a focused `DragValue` keeps
/// its own buffer until commit.
///
/// Returns whether the field is being dragged, so the caller can snapshot the pair
/// for a right-click cancel (`OndinApp::settings_scrub`).
fn nudge_field(ui: &mut egui::Ui, size: egui::Vec2, label: &'static str, value: &mut f64) -> bool {
    let (resp, _) = ui::value_field_suffixed(
        ui,
        size,
        Prefix::Text(label),
        Some(Suffix {
            text: "px",
            clickable: false,
            tooltip: "",
        }),
        value,
        // Slow, as a grid's column count is: a nudge is set to the pixel, so each
        // wants 20pt of travel rather than the usual 4.
        Scrub::whole(0.1).range(crate::input::NudgeStep::MIN..=crate::input::NudgeStep::MAX),
        |d| d.custom_formatter(ui::number(0)),
    );
    // Only the draft: this keeps the control from showing a fraction. What protects
    // the nudge itself is `NudgeStep::step`, at the reader.
    *value = value
        .round()
        .clamp(crate::input::NudgeStep::MIN, crate::input::NudgeStep::MAX);
    resp.dragged()
}

/// A word on a [`ui::label_button`], sized to the word.
///
/// Measured rather than given a constant width: `label_button` centres its text in
/// the box it is handed, and the card's three buttons differ in length.
pub(crate) fn text_button(ui: &mut egui::Ui, label: &str, state: FieldButton) -> egui::Response {
    let w = text_button_w(ui, label);
    ui::label_button(ui, label, egui::vec2(w, BUTTON_H), state)
}

/// The width [`text_button`] gives `label`: the text and its padding.
pub(crate) fn text_button_w(ui: &egui::Ui, label: &str) -> f32 {
    let text = ui.ctx().fonts_mut(|f| {
        f.layout_no_wrap(
            label.to_owned(),
            egui::FontId::proportional(BUTTON_PT),
            theme::text::muted(),
        )
    });
    text.size().x + BUTTON_PAD_X * 2.0
}

/// A byte count as a person reads one: `0 B`, `840 KB`, `1.4 MB`, `180 MB`.
///
/// One decimal below ten of a unit and none above, which keeps the row a steady
/// length as it fills. Binary units, matching the budget it reports against
/// (`fonts::FONT_CACHE_BYTES`): decimal MB would read a full cache as 268.
///
/// `pub(crate)` because the dashboard's *Size* column reads it. `panels::export`
/// has a different `human_bytes` that rounds up, since it estimates a file not yet
/// written; this one reports what is on disk, rounded to nearest.
pub(crate) fn human_bytes(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    let (unit, label) = match n {
        n if n >= GB => (GB, "GB"),
        n if n >= MB => (MB, "MB"),
        n if n >= KB => (KB, "KB"),
        _ => return format!("{n} B"),
    };
    let v = n as f64 / unit as f64;
    if v < 10.0 {
        format!("{v:.1} {label}")
    } else {
        format!("{v:.0} {label}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::OndinApp;

    /// The window the modal centres itself in, big enough that the card is never clipped.
    const SCREEN: egui::Vec2 = egui::vec2(1200.0, 900.0);

    /// Every text run the open modal painted, as `(top-left, size, text)`.
    ///
    /// Three passes, because a widget's interaction state is last frame's and a
    /// single pass measures a card still converging.
    fn galleys(app: &mut OndinApp, ctx: &egui::Context) -> Vec<(egui::Pos2, egui::Vec2, String)> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
            ..Default::default()
        };
        let mut out = Vec::new();
        for _ in 0..3 {
            let full = ctx.run_ui(input.clone(), |ui| app.settings_ui(ui.ctx()));
            out = full
                .shapes
                .iter()
                .filter_map(|cs| match &cs.shape {
                    egui::Shape::Text(t) => {
                        Some((t.pos, t.galley.size(), t.galley.text().to_owned()))
                    }
                    _ => None,
                })
                .collect();
        }
        out
    }

    /// The ground colour under each label's button, all read out of one frame:
    /// an `egui::Area` fades in, so grounds from two calls are tinted differently.
    /// Ten passes let the fade finish. The rect containing the label's centre is
    /// found by `find_map`, which takes the fill and not the hairline stroke
    /// `label_button` paints at the same place.
    fn button_grounds(
        app: &mut OndinApp,
        ctx: &egui::Context,
        labels: &[&str],
    ) -> Vec<egui::Color32> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
            ..Default::default()
        };
        let mut full = ctx.run_ui(input.clone(), |ui| app.settings_ui(ui.ctx()));
        for _ in 1..10 {
            full = ctx.run_ui(input.clone(), |ui| app.settings_ui(ui.ctx()));
        }
        labels
            .iter()
            .map(|label| {
                let at = full
                    .shapes
                    .iter()
                    .find_map(|cs| match &cs.shape {
                        egui::Shape::Text(t) if t.galley.text() == *label => {
                            Some(t.pos + t.galley.size() / 2.0)
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("no run for {label:?}"));
                full.shapes
                    .iter()
                    .find_map(|cs| match &cs.shape {
                        egui::Shape::Rect(r)
                            if r.rect.contains(at) && r.rect.height() <= BUTTON_H =>
                        {
                            Some(r.fill)
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("no button ground under {label:?}"))
            })
            .collect()
    }

    /// The column is what `CARD_W` leaves, no row runs past it, and the caption
    /// stays within two lines.
    ///
    /// The overflow loop covers only the `painter.text` runs (switch labels, button
    /// words), since `ui.label` wraps; the caption is asserted on its height.
    ///
    /// flip: `CARD_W` at 300 → fails the column assertion; one more clause on the
    /// caption → fails the caption height at 39pt.
    #[test]
    fn every_row_fits_the_cards_own_column() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.toggle_settings(&ctx);
        let runs = galleys(&mut app, &ctx);

        assert!(
            runs.iter().any(|(_, _, t)| t == "Settings"),
            "the fixture: the modal is open and painting (got {} runs)",
            runs.len()
        );
        let left = runs
            .iter()
            .map(|(p, _, _)| p.x)
            .fold(f32::INFINITY, f32::min);
        let column = ui::menu_inner_w(CARD_W, PAD);
        assert_eq!(
            column, 338.0,
            "376 across, less 18 of padding and a hairline"
        );

        for (pos, size, text) in &runs {
            let over = pos.x + size.x - (left + column);
            assert!(
                over <= 0.5,
                "{text:?} runs {over:.1}pt past the card's own column"
            );
        }
        // The column is used, or a card twice as wide would pass the loop.
        let widest = runs
            .iter()
            .map(|(p, s, _)| p.x + s.x - left)
            .fold(0.0_f32, f32::max);
        assert!(
            widest > column - 40.0,
            "the widest row is {widest:.1} in a {column} column — the card is not the \
             thing setting the width"
        );

        // The caption as a height: one line at `CAPTION_PT` is 13pt, so 26 is two.
        let caption = runs
            .iter()
            .find(|(_, _, t)| t.starts_with("Off is the fonts"))
            .map(|(_, s, _)| s.y)
            .expect("the web-fonts caption");
        assert!(
            caption <= 27.0,
            "the caption wrapped to {caption:.0}pt, which is more than the two lines \
             it is given"
        );
    }

    /// Every row sits on one centre with the control beside it, asserted on galley
    /// centres since the runs differ in size. The cache row is the case it exists
    /// for (see `OndinApp::font_cache_row`).
    ///
    /// flip: write the label before the button in one `horizontal` → the sentence's
    /// centre lands at 581 against the button's 583.
    #[test]
    fn a_row_and_the_button_beside_it_share_one_centre() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.toggle_settings(&ctx);
        let runs = galleys(&mut app, &ctx);
        let mid = |label: &str| {
            runs.iter()
                .find(|(_, _, t)| t == label)
                .map(|(p, s, _)| p.y + s.y / 2.0)
                .unwrap_or_else(|| panic!("no run for {label:?}"))
        };
        for (a, b) in [
            // Cache row, footer, a nudge field.
            ("Counting the web font cache…", "Clear"),
            ("Cancel", "Save changes"),
            ("Step", "px"),
        ] {
            assert!(
                (mid(a) - mid(b)).abs() <= 0.5,
                "{a:?} sits at {:.1} and {b:?} at {:.1}",
                mid(a),
                mid(b)
            );
        }
    }

    /// The commit wears the accent only once there is something to commit.
    /// `FieldButton::Off` and `Disabled` share a ground, so this measures the accent
    /// wash against *Cancel* in the same frame rather than the enum.
    ///
    /// flip: hand `text_button` `FieldButton::Primary` unconditionally → the clean case
    /// fails with the two grounds unequal.
    #[test]
    fn save_changes_is_dim_until_the_form_is_edited() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.toggle_settings(&ctx);
        let clean = button_grounds(&mut app, &ctx, &["Save changes", "Cancel"]);
        assert_eq!(
            clean[0], clean[1],
            "an untouched form has nothing to save, so its commit is not a commit yet"
        );

        // One row moved.
        let mut edited = app.settings.clone().expect("the modal is open");
        edited.web_fonts = !edited.web_fonts;
        app.settings = Some(edited);

        let dirty = button_grounds(&mut app, &ctx, &["Save changes", "Cancel"]);
        let (save, cancel) = (dirty[0], dirty[1]);
        assert_ne!(save, cancel, "an edited form's commit is lit");
        assert!(
            save.b() > save.r(),
            "and it is lit in the accent rather than merely brighter: {save:?}"
        );
    }

    /// Every painted text run and rect in the open card as a vertical interval, in
    /// order down the page; the window-spanning backdrop is excluded.
    fn ink(app: &mut OndinApp, ctx: &egui::Context) -> Vec<(f32, f32)> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
            ..Default::default()
        };
        let mut full = ctx.run_ui(input.clone(), |ui| app.settings_ui(ui.ctx()));
        for _ in 1..10 {
            full = ctx.run_ui(input.clone(), |ui| app.settings_ui(ui.ctx()));
        }
        let mut out: Vec<(f32, f32)> = full
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Rect(r) if r.rect.height() < SCREEN.y => {
                    Some((r.rect.top(), r.rect.bottom()))
                }
                egui::Shape::Text(t) => Some((t.pos.y, t.pos.y + t.galley.size().y)),
                _ => None,
            })
            .collect();
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out
    }

    /// A section header has the same air above it as below it.
    ///
    /// Measured on ink, not layout boxes: a 22pt `ui::switch_row` centres a 16pt
    /// track, so 3pt of row padding sits between header and ink; 4pt of slack
    /// covers that. The first header is exempt because the card's title above it
    /// stands off by `TITLE_GAP` on purpose, and gets its own assertion.
    ///
    /// flip: a 15pt `add_space` over an 11pt `item_spacing`, with the eyebrow row
    /// left to take `interact_size.y` → the headers fail (*Layers* 32 above
    /// against 20 below). Dropping the `TITLE_GAP - SECTION_AIR` line fails the
    /// title assertion.
    #[test]
    fn a_section_header_has_equal_air_above_and_below_it() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.toggle_settings(&ctx);
        let runs = galleys(&mut app, &ctx);
        let bands = ink(&mut app, &ctx);
        // Nearest ink ending before the header and starting after it; the header's
        // own galley is in `bands`, hence the strict comparisons.
        let air = |header: &str| -> (f32, f32) {
            let (top, bottom) = runs
                .iter()
                .find(|(_, _, t)| t == header)
                .map(|(p, s, _)| (p.y, p.y + s.y))
                .unwrap_or_else(|| panic!("no {header:?} eyebrow"));
            (
                bands
                    .iter()
                    .filter(|(_, b)| *b < top - 0.5)
                    .map(|(_, b)| top - b)
                    .fold(f32::INFINITY, f32::min),
                bands
                    .iter()
                    .filter(|(t, _)| *t > bottom + 0.5)
                    .map(|(t, _)| t - bottom)
                    .fold(f32::INFINITY, f32::min),
            )
        };

        for header in ["NUDGE", "LAYERS", "WEB FONTS"] {
            let (above, below) = air(header);
            assert!(
                (above - below).abs() <= 4.0,
                "{header}: {above:.1}pt of air above the header and {below:.1}pt below it"
            );
        }

        // The title's standoff is the larger one.
        let (title_gap, first_row) = air("APPEARANCE");
        assert!(
            title_gap > first_row + 6.0,
            "the title stands off by {title_gap:.1}pt against the header's own \
             {first_row:.1}pt — too close to tell apart"
        );
    }

    /// Both nudge fields hold whole pixels however the number got there. The
    /// fixture puts fractions straight into the draft, as typing does, so the
    /// field's own rounding is what is tested: `0.4` is clamped up to 1, `1.6`
    /// rounds to 2.
    ///
    /// flip: drop the `round()` in `nudge_field` → the pair comes back `1.0`/`1.6`;
    /// the clamp alone passes the first half and fails the second.
    #[test]
    fn the_nudge_fields_hold_whole_pixels() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.toggle_settings(&ctx);
        app.settings = Some(Settings {
            nudge: NudgeStep {
                small: 0.4,
                large: 1.6,
            },
            ..Settings::from_prefs(&app.prefs)
        });
        let _ = ink(&mut app, &ctx);
        let got = app.settings.expect("the modal is still open").nudge;
        assert_eq!(
            got,
            NudgeStep {
                small: 1.0,
                large: 2.0
            },
            "a fraction in either field is rounded, and 0 is clamped up to 1"
        );
    }

    /// A right-click puts a scrubbed nudge back, and `cancel_gesture` fires at all
    /// for one: its field test leads on `ctx.dragged_id()`, which egui clears on
    /// any button release, so `settings_scrub` (armed for the whole scrub) must be
    /// a term in it.
    ///
    /// flip: drop `settings_scrub` from `cancel_gesture`'s `field` test → returns
    /// `false` with the scrubbed value standing; drop the wind-back → value
    /// standing with `true` returned.
    #[test]
    fn a_right_click_winds_a_scrubbed_nudge_back() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.toggle_settings(&ctx);
        let before = app.settings.as_ref().expect("open").nudge;

        // The state a live scrub leaves, set directly: the cancel is under test,
        // not egui's drag detection.
        app.in_flight.settings_scrub = Some(before_form(&app).scrubbed());
        let mut canvas = before_form(&app).canvas;
        canvas.guide_thickness = 4;
        app.settings = Some(Settings {
            nudge: NudgeStep {
                small: 37.0,
                large: before.large,
            },
            canvas,
            ..before_form(&app)
        });

        assert!(
            app.cancel_gesture(&ctx),
            "a settings scrub is a gesture in flight, so the cancel has something to do"
        );
        assert_eq!(
            app.settings.as_ref().expect("still open").nudge,
            before,
            "the pair the scrub started from is what comes back"
        );
        assert_eq!(
            app.settings
                .as_ref()
                .expect("still open")
                .canvas
                .guide_thickness,
            CanvasPrefs::default().guide_thickness,
            "and so does the guide thickness"
        );
        assert!(
            app.in_flight.settings_scrub.is_none(),
            "and the snapshot is spent, so a second right-click winds nothing back"
        );
    }

    /// The draft's non-nudge rows as they stand, for building a modified one.
    fn before_form(app: &OndinApp) -> Settings {
        app.settings.clone().expect("the modal is open")
    }

    /// A cache that has not been counted must not read as an empty one.
    ///
    /// flip: fold `None` into the `Some(0)` arm of `cache_row` → fails at the first
    /// assertion.
    #[test]
    fn the_cache_row_tells_unknown_from_empty() {
        assert_eq!(
            cache_row(None),
            ("Counting the web font cache…".to_owned(), false),
            "not yet counted"
        );
        assert_eq!(
            cache_row(Some(0)),
            ("Nothing in the web font cache".to_owned(), false),
            "counted, and empty — so Clear has nothing to do"
        );
        assert_eq!(
            cache_row(Some(180 * 1024 * 1024)),
            ("180 MB of web font cache".to_owned(), true),
            "the row the roadmap asked for, and a live Clear beside it"
        );
    }

    /// The four bands, and the boundary between each pair.
    ///
    /// flip: drop the `v < 10.0` arm → `1024` comes back `1 KB`, not `1.0 KB`. The
    /// bands above ten pass either way, which is why the cases below ten exist.
    #[test]
    fn a_byte_count_reads_as_a_person_would_say_it() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(840 * 1024), "840 KB");
        assert_eq!(human_bytes(1024 * 1024 + 419_430), "1.4 MB");
        assert_eq!(human_bytes(180 * 1024 * 1024), "180 MB");
        // The cache's budget: binary units, or it would read 268.
        assert_eq!(human_bytes(256 * 1024 * 1024), "256 MB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
    }

    /// A draft equal to the preferences dims *Save changes*, so `from_prefs` and
    /// `apply` must be a round trip: a field forgotten in either is quietly unsaved.
    ///
    /// flip: drop the `nudge` line from `Settings::apply` → the round-trip
    /// assertion fails, reporting `1/10` where `0.5/25` is due.
    #[test]
    fn the_form_is_a_round_trip_through_the_preferences() {
        let mut prefs = Prefs::default();
        let opened = Settings::from_prefs(&prefs);
        assert_eq!(
            opened,
            Settings {
                theme: ThemePref::Dark,
                nudge: NudgeStep::default(),
                canvas: CanvasPrefs::default(),
                tree_guides: true,
                collapse_groups_on_open: false,
                resource_meter: false,
                default_font: "Inter".into(),
                spell_check: true,
                web_fonts: true,
            },
            "the form opens showing what is saved"
        );

        let edited = Settings {
            theme: ThemePref::Light,
            nudge: NudgeStep {
                small: 0.5,
                large: 25.0,
            },
            canvas: CanvasPrefs {
                wheel_zooms: true,
                guide_colour: ondin_core::peniko::Color::from_rgb8(217, 70, 143),
                guide_thickness: 3,
                grid_adaptive: false,
                grid_ink: ondin_core::peniko::Color::from_rgba8(40, 120, 220, 90),
            },
            tree_guides: false,
            collapse_groups_on_open: true,
            resource_meter: true,
            default_font: "Roboto".into(),
            spell_check: false,
            web_fonts: false,
        };
        assert!(edited != opened, "the fixture has to be a real edit");
        assert!(
            edited.apply(&mut prefs),
            "moving the web-fonts switch is what the return value reports"
        );
        assert_eq!(
            Settings::from_prefs(&prefs),
            edited,
            "everything the form holds survives being written and read back"
        );
        assert!(
            !edited.apply(&mut prefs),
            "applying the same form twice moves no switch"
        );
    }

    /// A card centres on the point `set_card_centre` names, and on the window with none.
    /// flip: `card_modal` ignoring the centre → fails at the canvas assert.
    #[test]
    fn a_card_centres_on_the_canvas_when_there_is_one() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let centre_of = |centre: Option<egui::Pos2>| {
            let ctx = egui::Context::default();
            let mut rect = egui::Rect::NOTHING;
            for _ in 0..3 {
                let input = egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                };
                let _ = ctx.run_ui(input, |ui| {
                    set_card_centre(ui.ctx(), centre);
                    rect = card("probe-card", ui.ctx(), |ui| {
                        ui.allocate_exact_size(egui::vec2(200.0, 100.0), egui::Sense::hover());
                    })
                    .response
                    .rect;
                });
            }
            rect.center()
        };
        let canvas = egui::pos2(500.0, 330.0);
        assert!(
            (centre_of(Some(canvas)) - canvas).length() < 1.0,
            "on the canvas: {:?}",
            centre_of(Some(canvas))
        );
        assert!((centre_of(None) - screen.center()).length() < 1.0);
    }

    /// `Tab` walks a footer in the order it reads, the way out first, and the row
    /// still ends at the card's right edge.
    /// flip: `footer_row_noted` laid right to left again → fails at "left to right".
    #[test]
    fn tab_walks_a_footer_left_to_right() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let buttons = [
            ("Cancel", FieldButton::Off),
            ("Discard", FieldButton::Danger),
            ("Save and close", FieldButton::Primary),
        ];
        let frame = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
                events,
                ..Default::default()
            };
            let mut right = 0.0;
            let _ = ctx.run_ui(input, |ui| {
                ui.set_width(400.0);
                let _ = footer_row(ui, &buttons);
                right = ui.max_rect().right();
            });
            let at = ctx
                .memory(|m| m.focused())
                .and_then(|id| ctx.read_response(id))
                .map(|r| r.rect);
            (at, right)
        };
        let tab = || {
            vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]
        };
        frame(Vec::new());
        let mut stops = Vec::new();
        let mut edge = 0.0;
        for _ in 0..3 {
            frame(tab());
            let (at, right) = frame(Vec::new());
            stops.push(at.expect("a footer button holds the focus"));
            edge = right;
        }
        assert!(
            stops.windows(2).all(|w| w[0].center().x < w[1].center().x),
            "left to right: {stops:?}"
        );
        assert!(
            (stops[2].right() - edge).abs() < 0.5,
            "the commit is rightmost"
        );
    }

    /// The Settings card with `which` colour's picker open beside it, the two
    /// drawn each frame as the app draws them. Returns the app and the two rects.
    fn settings_with_picker(
        ctx: &egui::Context,
        which: PrefColour,
    ) -> (OndinApp, egui::Rect, egui::Rect) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        app.toggle_settings(ctx);
        for _ in 0..5 {
            frame(ctx, &mut app, Vec::new());
        }
        let card = ctx
            .memory(|m| m.area_rect(egui::Id::new("settings")))
            .expect("the card is up");
        app.open_picker_beside(ctx, PaintSlot::Pref(which), card, card.center().y);
        for _ in 0..5 {
            frame(ctx, &mut app, Vec::new());
        }
        let picker = ctx
            .memory(|m| m.area_rect(egui::Id::new("colour-picker")))
            .expect("the picker is up");
        (app, card, picker)
    }

    /// One frame of the modal and the picker, in the app's order.
    fn frame(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            app.settings_ui(ui.ctx());
            app.picker_ui(ui.ctx());
        });
    }

    /// A click at `at`, pressed and released over two frames.
    fn click(ctx: &egui::Context, app: &mut OndinApp, at: egui::Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(ctx, app, vec![egui::Event::PointerMoved(at), button(true)]);
        frame(ctx, app, vec![button(false)]);
        frame(ctx, app, Vec::new());
    }

    /// The picker opens beside the card and stays live over it after a press on the
    /// card raises the modal: a press in the picker picks into the form's draft,
    /// and neither cancels the form nor reaches the saved preferences.
    ///
    /// flip: `picker_ui` without its `move_to_top` for a Settings slot → fails at
    /// "the press is not a backdrop click": the buried picker's press lands on the
    /// modal's backdrop and discards the form.
    #[test]
    fn a_colour_picked_over_the_card_is_staged_in_the_form() {
        let ctx = egui::Context::default();
        let (mut app, card, picker) = settings_with_picker(&ctx, PrefColour::Guide);
        assert!(
            !card.intersects(picker),
            "beside the card: {card:?} {picker:?}"
        );
        // On the title, which does nothing but raise the modal.
        click(
            &ctx,
            &mut app,
            card.left_top() + egui::vec2(PAD + 4.0, PAD + 4.0),
        );
        let before = app.pref_colour(PrefColour::Guide);
        click(&ctx, &mut app, picker.center());
        assert!(app.settings.is_some(), "the press is not a backdrop click");
        assert!(app.picker.is_some(), "and the picker is still open");
        assert_ne!(
            app.pref_colour(PrefColour::Guide),
            before,
            "the press reached the picker"
        );
        assert_eq!(
            app.prefs.canvas.guide_colour,
            CanvasPrefs::default().guide_colour,
            "nothing is saved before Save changes"
        );
    }

    /// A press in the pixel grid colour's picker picks, as the guide's does: the grid's
    /// picker also draws the alpha field, whose idle frames must not write the colour
    /// back over the pick.
    ///
    /// flip: `valve_slot` writing a Settings colour on every call, not only while the
    /// control is engaged → fails at "the press reached the picker".
    #[test]
    fn a_pick_in_the_grid_colour_s_picker_holds() {
        let ctx = egui::Context::default();
        let (mut app, card, picker) = settings_with_picker(&ctx, PrefColour::Grid);
        assert!(
            PaintSlot::Pref(PrefColour::Grid).takes_alpha(),
            "fixture: the picker draws the alpha field"
        );
        click(
            &ctx,
            &mut app,
            card.left_top() + egui::vec2(PAD + 4.0, PAD + 4.0),
        );
        let before = app.pref_colour(PrefColour::Grid);
        click(&ctx, &mut app, picker.center());
        assert_ne!(
            app.pref_colour(PrefColour::Grid),
            before,
            "the press reached the picker"
        );
        let picked = app.pref_colour(PrefColour::Grid);
        for _ in 0..5 {
            frame(&ctx, &mut app, Vec::new());
        }
        assert_eq!(
            app.pref_colour(PrefColour::Grid),
            picked,
            "idle frames keep it"
        );
    }

    /// Escape with a Settings colour's picker open closes the picker and keeps the
    /// form; a second Escape discards the form.
    ///
    /// flip: `settings_ui` without its Escape guard → fails at "the form stays".
    #[test]
    fn escape_closes_the_picker_before_the_form() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = settings_with_picker(&ctx, PrefColour::Guide);
        let escape = || egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&ctx, &mut app, vec![escape()]);
        assert!(app.picker.is_none(), "the picker closes");
        assert!(app.settings.is_some(), "the form stays");
        frame(&ctx, &mut app, Vec::new());
        frame(&ctx, &mut app, vec![escape()]);
        assert!(app.settings.is_none(), "a second Escape discards the form");
    }
}
