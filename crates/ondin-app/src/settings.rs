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
use crate::prefs::Prefs;
use crate::theme::{self, color, icon};
use crate::ui::{self, FieldButton, Prefix, Scrub, Suffix};

/// Width of the card, border and padding included (the design's `width:376px`).
pub(crate) const CARD_W: f32 = 376.0;
/// The card's inner margin (the design's `padding:18px`).
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
/// Between two rows inside one section (the design's `gap:11px`/`13px`).
pub(crate) const ROW_GAP: f32 = 11.0;
/// A switch row. [`crate::panels`]' feature lists use the same height, and these
/// are the same control in a column.
pub(crate) const SWITCH_H: f32 = 22.0;
/// A value field, and so also the buttons beside one — the app's control height.
pub(crate) const FIELD_H: f32 = 28.0;
/// The footer's buttons, and the *Clear* button in the web-fonts section: the field
/// height they sit among.
const BUTTON_H: f32 = FIELD_H;
/// What a button's label keeps clear of its own corners, per side.
const BUTTON_PAD_X: f32 = 14.0;
/// Between two controls on one row (the design's `gap:10px` / the field gap).
pub(crate) const COL_GAP: f32 = 10.0;
/// The *Default font* dropdown: room for a long family name beside its label.
const DEFAULT_FONT_W: f32 = 200.0;

/// Type size of a row's label — [`ui::SWITCH_ROW_LABEL_PT`], because half these
/// rows *are* switch rows and a caption beside one has to match it.
pub(crate) const LABEL_PT: f32 = ui::SWITCH_ROW_LABEL_PT;
/// Type size of the sentence under a row that needs one.
pub(crate) const CAPTION_PT: f32 = 10.5;
/// Type size of a button's word — [`ui::label_button`]'s.
const BUTTON_PT: f32 = 11.5;

/// What the Settings modal has been edited to, before *Save changes* is pressed.
///
/// A copy of the preferences rather than `Option`s: every row always has a value,
/// and mirroring `Prefs` field for field makes the dirty test one `!=`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    nudge: NudgeStep,
    tree_guides: bool,
    collapse_groups_on_open: bool,
    default_font: String,
    web_fonts: bool,
}

impl Settings {
    /// The form as the saved preferences have it — what opening the modal shows.
    fn from_prefs(prefs: &Prefs) -> Self {
        Self {
            nudge: prefs.nudge,
            tree_guides: prefs.tree_guides,
            collapse_groups_on_open: prefs.collapse_groups_on_open,
            default_font: prefs.default_font.clone(),
            web_fonts: prefs.web_fonts,
        }
    }

    /// Put a nudge pair back — what `OndinApp::cancel_gesture` calls to undo a
    /// right-clicked scrub. A setter because the whole pair is what a cancel restores.
    pub(crate) fn set_nudge(&mut self, nudge: NudgeStep) {
        self.nudge = nudge;
    }

    /// Write the form back. Returns whether the web-fonts switch moved, because
    /// that is the one row with a consequence beyond the file — see
    /// [`crate::fonts::FontService::set_web_fonts`].
    fn apply(&self, prefs: &mut Prefs) -> bool {
        let web_fonts_changed = prefs.web_fonts != self.web_fonts;
        prefs.nudge = self.nudge;
        prefs.tree_guides = self.tree_guides;
        prefs.collapse_groups_on_open = self.collapse_groups_on_open;
        prefs.default_font.clone_from(&self.default_font);
        prefs.web_fonts = self.web_fonts;
        web_fonts_changed
    }
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
    /// `<OndinApp as eframe::App>::ui`); otherwise an arrow key would nudge the
    /// selection behind the backdrop.
    pub(crate) fn settings_ui(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.settings.clone() else {
            return;
        };
        let saved = Settings::from_prefs(&self.prefs);

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

            // --- nudge -------------------------------------------------
            ui.add_space(TITLE_GAP - SECTION_AIR);
            section(ui, "Nudge");
            // Before the fields: `value_field` applies this frame's drag delta inside
            // the call that reports `drag_started()`.
            let before = draft.nudge;
            let half = egui::vec2((ui.available_width() - COL_GAP) / 2.0, FIELD_H);
            let dragging = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = COL_GAP;
                    let a = nudge_field(ui, half, "Step", &mut draft.nudge.small);
                    let b = nudge_field(ui, half, "Shift", &mut draft.nudge.large);
                    a || b
                })
                .inner;
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
                            .color(theme::text::MUTED),
                    );
                });
            });
        });
    }

    /// *Default font* and the family dropdown, the typography panel's list in it.
    /// Returns the family picked, for the draft. A family that can never be had
    /// wears its glyph in `WARN`, as in the panel.
    fn default_font_row(&mut self, ui: &mut egui::Ui, current: &str) -> Option<String> {
        let mut chosen = None;
        let missing = self.fonts.family_status(current) == crate::fonts::FamilyStatus::Missing;
        let head = if missing {
            ui::glyph_and_text_tinted(icon::TEXT_AA, current, color::WARN)
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

/// [`card_modal`] shown, with the keyboard's focus ring painted over it.
///
/// The wrapper exists so a card cannot forget the ring: `ui::focus_ring` must be
/// the last thing drawn in the body, and every card goes through here.
pub(crate) fn card<R>(
    id: &str,
    ctx: &egui::Context,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::ModalResponse<R> {
    card_modal(ctx, id).show(ctx, |ui| {
        let out = body(ui);
        ui::focus_ring(ui);
        // The card's rect, frame included, so the window's own controls laid over
        // the backdrop keep clear of it (`chrome::above_modal`).
        let rect = ui.min_rect().expand(PAD);
        crate::chrome::note_card(ui.ctx(), rect);
        out
    })
}

/// The card every settings-shaped modal is drawn on: the ground, the hairline,
/// the corner, the shadow, [`PAD`] of inner margin, and a backdrop at the design's
/// weight.
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
        // The design's `rgba(0,0,0,.5)`: a modal over a canvas must dim more than
        // egui's default 100/255, or a light document still reads as in focus.
        .backdrop_color(egui::Color32::from_black_alpha(128))
        .frame(
            egui::Frame::NONE
                .fill(color::CARD)
                // The token, not its value, so retuning `color::DIVIDER` moves the
                // modal hairline too; no gate could catch a respelled copy.
                .stroke(egui::Stroke::new(1.0, theme::color::DIVIDER))
                .corner_radius(egui::CornerRadius::same(10))
                // Deeper than a dropdown's: this floats over everything.
                .shadow(egui::epaint::Shadow {
                    offset: [0, 24],
                    blur: 60,
                    spread: 0,
                    color: egui::Color32::from_black_alpha(128),
                })
                .inner_margin(egui::Margin::same(PAD as i8)),
        )
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
/// The ✕ is [`ui::icon_button`] at 18/14 in every card.
pub(crate) fn modal_title(ui: &mut egui::Ui, title: &str) -> bool {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(title)
                .size(14.0)
                .color(theme::text::STRONG),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui::icon_button(ui, icon::X, 18.0, 14.0, false, true).clicked()
        })
        .inner
    })
    .inner
}

/// The *Cancel* / commit pair, right-aligned — `commit` is the commit's label,
/// *Save changes* or *Create project* — with the commit lit only when `enabled`:
/// there is something to commit. Returns the [`Decision`] a press made.
///
/// Shared so every card's commit lights on the same rule for *edited*.
pub(crate) fn modal_footer(
    ui: &mut egui::Ui,
    commit: &'static str,
    enabled: bool,
) -> Option<Decision> {
    let state = if enabled {
        FieldButton::On
    } else {
        FieldButton::Disabled
    };
    footer(ui, Some((commit, state)))
}

/// [`modal_footer`] for a card that destroys something: the commit red
/// (`FieldButton::Danger`), the blue commits' twin on the other ramp, and always
/// live — the card opening is the question.
pub(crate) fn danger_footer(ui: &mut egui::Ui, commit: &'static str) -> Option<Decision> {
    footer(ui, Some((commit, FieldButton::Danger)))
}

/// [`modal_footer`] for a card whose choice is made in its body: *Cancel*
/// alone. Answers whether it was pressed.
pub(crate) fn cancel_footer(ui: &mut egui::Ui) -> bool {
    matches!(footer(ui, None), Some(Decision::Cancel))
}

/// The one footer row: *Cancel*, with `commit`'s label and state to its right.
fn footer(ui: &mut egui::Ui, commit: Option<(&'static str, FieldButton)>) -> Option<Decision> {
    let mut decision = None;
    ui.add_space(FOOTER_GAP);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        // Rightmost: added first in a right-to-left row.
        if let Some((label, state)) = commit
            && text_button(ui, label, state).clicked()
            && state != FieldButton::Disabled
        {
            decision = Some(Decision::Save);
        }
        if text_button(ui, "Cancel", FieldButton::Off).clicked() {
            decision = Some(Decision::Cancel);
        }
    });
    decision
}

/// Which way the modal was closed. `None` for "still open" is the third state and
/// lives in the caller's `Option`.
pub(crate) enum Decision {
    Save,
    Cancel,
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
                egui::Stroke::new(1.0, theme::color::DIVIDER),
            );
        },
    );
    ui.add_space(SECTION_AIR);
}

/// A sentence under the row above it, wrapped to the card.
///
/// Closer than [`ROW_GAP`], because a caption belongs to the row above it.
pub(crate) fn caption(ui: &mut egui::Ui, text: &str) {
    ui.add_space(3.0);
    ui.label(
        egui::RichText::new(text)
            .size(CAPTION_PT)
            .color(theme::text::FAINT),
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
        // 0.5, the rate every other whole-unit field scrubs at.
        Scrub::whole(0.5).range(crate::input::NudgeStep::MIN..=crate::input::NudgeStep::MAX),
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
    let w = ui
        .ctx()
        .fonts_mut(|f| {
            f.layout_no_wrap(
                label.to_owned(),
                egui::FontId::proportional(BUTTON_PT),
                theme::text::MUTED,
            )
        })
        .size()
        .x;
    ui::label_button(
        ui,
        label,
        egui::vec2(w + BUTTON_PAD_X * 2.0, BUTTON_H),
        state,
    )
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
             the design gives it"
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
    /// flip: hand `text_button` `FieldButton::On` unconditionally → the clean case
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
    /// left to take `interact_size.y` → both headers fail (*Layers* 32 above
    /// against 20 below). Dropping the `TITLE_GAP - SECTION_AIR` line fails the
    /// title assertion at 21.5 against 18.
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

        for header in ["LAYERS", "WEB FONTS"] {
            let (above, below) = air(header);
            assert!(
                (above - below).abs() <= 4.0,
                "{header}: {above:.1}pt of air above the header and {below:.1}pt below it"
            );
        }

        // The title's standoff is the larger one.
        let (title_gap, first_row) = air("NUDGE");
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
        app.in_flight.settings_scrub = Some(before);
        app.settings = Some(Settings {
            nudge: NudgeStep {
                small: 37.0,
                large: before.large,
            },
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
                nudge: NudgeStep::default(),
                tree_guides: true,
                collapse_groups_on_open: false,
                default_font: "Inter".into(),
                web_fonts: true,
            },
            "the form opens showing what is saved"
        );

        let edited = Settings {
            nudge: NudgeStep {
                small: 0.5,
                large: 25.0,
            },
            tree_guides: false,
            collapse_groups_on_open: true,
            default_font: "Roboto".into(),
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
}
