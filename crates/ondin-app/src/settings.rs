//! The Settings modal (`design/Editor.dc.html`, §15 D330) — the home for
//! preferences that belong to no panel.
//!
//! **The rows a panel *is* a door onto do not live here**, and that division is
//! the whole reason this is a short file. *Slashes in names make folders* and
//! *Re-export on save* are export habits and stay in the export panel's own
//! overflow popover, beside the button they change the behaviour of; a setting
//! reached where the work happens is a setting the user finds. What is left over
//! is what this exists for: the arrow-key step, the layer tree's guides and its
//! fold-on-open, and whether the web-font library is offered at all — four things
//! with no panel to hang off, which is why the roadmap could describe a font-cache
//! row for months without anywhere to put it. (`prefs.rs`'s twin sentence has
//! always had all four; this one dropped *Show tree guides* until 2026-09-06, and
//! it is the paragraph a reader consults before adding a row.)
//!
//! **A form, not a switchboard.** Every row here is *staged* and written by *Save
//! changes*; Cancel, Escape, the ✕ and a click on the backdrop all discard. That
//! is the opposite of the export popover, where each row calls `prefs.save()` on
//! the spot, and the difference is which question the surface is answering: a
//! popover beside a button is an adjustment to work in progress, where a modal
//! over the whole editor is a sitting-down-with-the-app. Committing on Save is
//! also what makes the *web-fonts* row safe to offer — turning it off drops ~1,976
//! families out of the picker, and a switch that did that on the way past would be
//! a click nobody meant to make.
//!
//! 🚨 **Escape discards even mid-field, and that is decided rather than
//! overlooked** (§15 D770, `[S18.3-L1-07]`). The finding reports flipping
//! *Load web fonts*, typing into *Step*, pressing Escape, and losing both. It is
//! real: `egui::ModalResponse::should_close` gates only on `is_top_modal` and
//! `!any_popup_open`, and egui's `TextEdit` handles no Escape at all, so the press
//! reaches the modal whatever has focus.
//!
//! ⚠️ **"While a field owns the keyboard" describes the situation and is not a
//! condition on the behaviour** — measured: the card closes identically with and
//! without focus. What the focused case adds is only that `DragValue` *reads*
//! Escape one layer down (to abandon a typed number, `drag_value.rs`) so the user's
//! expectation is narrower than what happens.
//!
//! **Kept, on three grounds.** What is lost is five preference values, not artwork.
//! The sentence above already says Escape discards, so the behaviour is
//! documented rather than accidental. And the obvious guard is a bad trade:
//! `should_close()` is **one expression covering Escape, the ✕ and the backdrop
//! click**, so gating it also breaks backdrop-dismiss while a field has focus, and
//! `ctx.egui_wants_keyboard_input()` is true whenever *anything* has focus —
//! including a button reached by Tab.
//!
//! ⚠️ **The principled alternative is confirm-on-dirty and it is blocked.**
//! `draft != saved` is already computed every frame for the footer, but
//! `[S18.3-L1-02]` records `nudge_field`'s clamp moving the draft on the card's
//! first frame — so the confirmation would fire on a card nobody edited. **That is
//! the thing to fix before this question is worth re-opening.**
//!
//! **One exception, and it is not an inconsistency**: *Clear* empties the font
//! cache immediately, and Cancel does not put the files back. A deletion cannot be
//! staged honestly — a button that promised to delete later, or that showed
//! `0 MB` for a directory still holding 180, would be lying about the one thing the
//! row exists to report. So it acts, says how much went, and the count beside it is
//! the receipt.

use crate::app::OndinApp;
use crate::input::NudgeStep;
use crate::prefs::Prefs;
use crate::theme::{self, color, icon};
use crate::ui::{self, FieldButton, Prefix, Scrub, Suffix};

/// Width of the card, border and padding included (the design's `width:376px`).
pub(crate) const CARD_W: f32 = 376.0;
/// The card's inner margin (the design's `padding:18px`).
pub(crate) const PAD: f32 = 18.0;
/// Clear air **above and below** a section header — the same number both sides.
///
/// ⚠️ **One value, and it is one value on purpose.** The design has `gap:26px`
/// between sections and `gap:11px` inside one, which puts a header more than twice
/// as far from the controls above it as from its own — and the shipped card was
/// worse than the design at it, 32 above against 17 below, because two things
/// accumulated that this file thought were one: the `add_space` compensating for
/// `item_spacing`, and the eyebrow row's own box being ~6pt taller than its 12pt
/// galley. Reported as *"padding top/bottom in sections is unbalanced… if I have to
/// guess, a section has both padding bottom and margin-top, so they accumulate"* —
/// which is exactly what was happening.
///
/// **The header can sit centred in that air because it carries a hairline.** A bare
/// eyebrow floating midway between two groups is ambiguous about which one it names;
/// a rule running out to the card's edge is a fieldset legend and reads as
/// *everything below me* however the space around it is divided. That is what makes
/// equal cheaper than a ratio here — and equal is what was asked for.
///
/// 18 rather than 13 (half the design's 26): the total between one section's last
/// control and the next section's first becomes 18 + [`EYEBROW_H`] + 18 = 48, against
/// the design's ~52 and the shipped card's 61.
pub(crate) const SECTION_AIR: f32 = 18.0;
/// Height the section header's row is allocated, which is exactly its galley — see
/// [`SECTION_AIR`] for why anything more is a bug rather than breathing room.
const EYEBROW_H: f32 = 12.0;
/// Under the modal's title, **in total** — the design's `gap:26px`, kept.
///
/// **Not [`SECTION_AIR`], because the title is not a section header**: it names the
/// card rather than the rows under it, has no rule, and wants to stand apart rather
/// than be centred in anything.
///
/// ⚠️ *In total* is the load-bearing word: [`section`] adds [`SECTION_AIR`] above
/// its own header, so the caller subtracts that here rather than adding 26 on top of
/// it. Measured before the subtraction: 44pt under the title against the design's
/// 26. Accumulating two gaps that each look right on their own is the exact fault
/// [`SECTION_AIR`] was reported for.
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
/// The footer's buttons, and the *Clear* button in the web-fonts section.
///
/// **28, which is neither the design's 30 nor its 26.** The two are the same class
/// of control one card apart, and the number that makes them consistent with
/// everything else in the app is the field height they sit among.
const BUTTON_H: f32 = FIELD_H;
/// What a button's label keeps clear of its own corners, per side.
const BUTTON_PAD_X: f32 = 14.0;
/// Between two controls on one row (the design's `gap:10px` / the field gap).
pub(crate) const COL_GAP: f32 = 10.0;

/// Type size of a row's label — [`ui::SWITCH_ROW_LABEL_PT`], because half these
/// rows *are* switch rows and a caption beside one has to match it.
pub(crate) const LABEL_PT: f32 = ui::SWITCH_ROW_LABEL_PT;
/// Type size of the sentence under a row that needs one.
pub(crate) const CAPTION_PT: f32 = 10.5;
/// Type size of a button's word — [`ui::label_button`]'s.
const BUTTON_PT: f32 = 11.5;

/// What the Settings modal has been edited to, before *Save changes* is pressed.
///
/// **A copy of the preferences rather than a set of `Option`s.** Every row here is
/// always shown and always has a value, so "unset" is not a state the form can be
/// in — and a draft that mirrors `Prefs` field for field means the *dirty* test is
/// one `!=` rather than a per-row comparison somebody has to remember to extend.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    nudge: NudgeStep,
    tree_guides: bool,
    collapse_groups_on_open: bool,
    web_fonts: bool,
}

impl Settings {
    /// The form as the saved preferences have it — what opening the modal shows.
    fn from_prefs(prefs: &Prefs) -> Self {
        Self {
            nudge: prefs.nudge,
            tree_guides: prefs.tree_guides,
            collapse_groups_on_open: prefs.collapse_groups_on_open,
            web_fonts: prefs.web_fonts,
        }
    }

    /// Put a nudge pair back — what `OndinApp::cancel_gesture` calls to undo a
    /// right-clicked scrub, out of `OndinApp::settings_scrub`.
    ///
    /// A setter rather than a public field because the *whole pair* is what a cancel
    /// restores: the two fields are one struct, one snapshot is taken when either
    /// starts a drag, and nothing should be able to wind back half of it.
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
        prefs.web_fonts = self.web_fonts;
        web_fonts_changed
    }
}

impl OndinApp {
    /// The top bar's rightmost control: the button that opens the modal.
    ///
    /// Drawn inside the top bar's `right_to_left` cluster, so it is added first to
    /// end up last.
    pub(crate) fn settings_button(&mut self, ui: &mut egui::Ui) {
        if ui::icon_button(ui, icon::SLIDERS, 28.0, 16.0, self.settings.is_some(), true)
            .on_hover_text("Settings")
            .clicked()
        {
            self.toggle_settings(ui.ctx());
        }
    }

    /// Open the modal, or close it discarding whatever was typed into it.
    ///
    /// **Opening is what asks for the cache size**, once, rather than the row asking
    /// every frame it draws: a `read_dir` per frame for as long as a dialog is open
    /// is a directory scan sixty times a second for a number that changes when the
    /// user presses one button.
    pub(crate) fn toggle_settings(&mut self, ctx: &egui::Context) {
        // Whichever way this goes, no scrub survives it: the snapshot exists to wind
        // one of these fields back, and there is nothing to wind back into once the
        // form is gone — or once it has been reopened from the saved preferences.
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
    /// ⚠️ **The keymap is silenced for as long as this is open** — see the guard in
    /// `<OndinApp as eframe::App>::ui` beside the context menu's. Without it an arrow key typed
    /// at a field this modal has *not* focused nudges the selection behind the
    /// backdrop, which is an edit the user cannot see being made.
    pub(crate) fn settings_ui(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.settings.clone() else {
            return;
        };
        let saved = Settings::from_prefs(&self.prefs);

        let mut decision = None;
        let modal = card("settings", ctx, |ui| {
            ui.set_width(ui::menu_inner_w(CARD_W, PAD));
            // ⚠️ **Zero, and every vertical gap in this card is then written
            // out.** With an `item_spacing.y` in play, any `add_space` is *added
            // to* it and the number in the source is not the gap on screen —
            // which is half of what made the section spacing accumulate to 32pt
            // where 26 was intended (see [`SECTION_AIR`]). One rhythm, stated
            // once, arithmetic a probe can check.
            ui.spacing_mut().item_spacing.y = 0.0;

            // --- title -------------------------------------------------
            if modal_title(ui, "Settings") {
                decision = Some(Decision::Cancel);
            }

            // --- nudge -------------------------------------------------
            ui.add_space(TITLE_GAP - SECTION_AIR);
            section(ui, "Nudge");
            // Snapshotted before the fields are drawn, because `value_field`
            // applies this frame's drag delta *inside* the call that reports
            // `drag_started()` — so read after it, this would already be the
            // scrubbed pair rather than the one a cancel has to put back.
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
            // **The cancel is answered before the snapshot is armed**, which is
            // the rule `panels::export`'s quality field learned the hard way: on
            // the frame a gesture is called off, letting anything through re-arms
            // or commits the very movement the cancel exists to discard.
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

            // --- web fonts ---------------------------------------------
            section(ui, "Web fonts");
            if ui::switch_row(ui, "Load web fonts", draft.web_fonts, SWITCH_H).clicked() {
                draft.web_fonts = !draft.web_fonts;
            }
            // **A caption rather than a tooltip, for this row alone.** Its
            // label names a *capability* and says nothing about what the app
            // does without it, and "off" here is the state a user picks
            // deliberately — on a metered connection, or a machine that is not
            // supposed to talk to a CDN. A sentence they have to hover to find
            // is a sentence they will not read before flipping the switch.
            caption(
                ui,
                "Off is the fonts this machine has installed, plus Inter. \
                     Nothing is downloaded and no request is made.",
            );
            ui.add_space(ROW_GAP);
            self.font_cache_row(ui);

            // --- footer ------------------------------------------------
            if let Some(d) = modal_footer(ui, draft != saved) {
                decision = Some(d);
            }
        });

        // The backdrop and Escape, both of which mean Cancel. Read *after* the
        // body, because `should_close` consumes the Escape press — a body that
        // asked first would answer for a frame in which the key had not yet been
        // taken by the topmost modal.
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
                self.prefs.save();
                self.settings = None;
                self.in_flight.settings_scrub = None;
            }
            // Still open: keep whatever was typed into it this frame.
            None => self.settings = Some(draft),
        }
    }

    /// *`180 MB` of web font cache · Clear* — the row this whole modal was asked
    /// for (§15 D330; the `roadmap.md` section that carried the ask is retired).
    ///
    /// **The number is the point, not the button.** A cache bounded at 256 MB is
    /// not a leak, and the sweep at startup already keeps it there; what the user
    /// could not do was *see* it, which is what turns "the app has taken 200 MB"
    /// from a support question into a fact with a button beside it.
    fn font_cache_row(&mut self, ui: &mut egui::Ui) {
        let (text, clearable) = cache_row(self.fonts.cache_bytes());
        // ⚠️ **The button is allocated before the label, and the nesting is what
        // makes that so — it is not decoration.** A `ui.horizontal` centres each
        // item against the row height it knows *at the moment that item is
        // allocated*, so with the label written first it is centred in
        // `interact_size.y` and the 28pt button then grows the row past it: measured
        // at y 581 against the button's centre of 583, a 2pt drop nobody would find
        // by reading. Setting `interact_size.y` ahead of it does **not** fix this,
        // which was the obvious guess and was tried. The button first, then the
        // label in a left-to-right scope inside the same row, puts both on 583.
        // Same 2pt family as `ui::FIELD_BORDER_H`.
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
                    // Said out loud, because the row it changes goes from a number
                    // to a sentence, and a user watching the button cannot tell
                    // how much a click reclaimed.
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
}

/// What the font-cache row says, and whether *Clear* does anything.
///
/// **Three states, not two, and the middle one is the whole reason this is a
/// function.** `None` is a scan that has not landed and `Some(0)` is an empty
/// cache; a row that rendered both as `0 MB` would report a directory holding two
/// hundred megabytes as clear, for however many frames the scan took. Split out
/// from the row for [`human_bytes`]'s reason — it is the whole of the rule and none
/// of the drawing, and the alternative is a mapping that can only be checked by
/// standing up an app and a `FontService`.
fn cache_row(bytes: Option<u64>) -> (String, bool) {
    match bytes {
        None => ("Counting the web font cache…".to_owned(), false),
        Some(0) => ("Nothing in the web font cache".to_owned(), false),
        Some(n) => (format!("{} of web font cache", human_bytes(n)), true),
    }
}

/// [`card_modal`] shown, with the keyboard's focus ring painted over it.
///
/// **The wrapper exists so a card cannot forget the ring.** `ui::focus_ring` has to
/// be the *last* thing drawn in the body — it marks a widget that has already been
/// laid out, and a ring under the control it rings is no ring — which makes it
/// exactly the kind of call a new modal leaves out. Every card in the app goes
/// through here; `card_modal` stays public for the one caller that needs the
/// builder rather than the call (there is none today, and it is the seam where a
/// modal wanting a different width or backdrop would go).
pub(crate) fn card<R>(
    id: &str,
    ctx: &egui::Context,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::ModalResponse<R> {
    card_modal(id).show(ctx, |ui| {
        let out = body(ui);
        ui::focus_ring(ui);
        out
    })
}

/// The card every settings-shaped modal is drawn on: the ground, the hairline,
/// the corner, the shadow, [`PAD`] of inner margin, and a backdrop at the design's
/// weight.
///
/// **Shared rather than copied, because there were two of these cards and the
/// dashboard's was visibly not the same object** — egui's default modal frame and
/// backdrop against this one. Reported as *"Settings is a mess that looks nothing
/// like the layout in design/"*, of which the chrome was the first half; the second
/// half was that its rows were built out of a different kit (§15 D368). That is
/// history rather than a present count: [`card`] is this function's one caller
/// today, and every card reaches it through the wrapper.
///
/// The caller still sets its own width: [`CARD_W`] is this file's, and a modal that
/// holds a wider row is allowed to be wider than the one that does not.
///
/// ⚠️ **This whole run was sitting on [`card`] until 2026-09-06**, which is
/// `CLAUDE.md`'s *"anchor above"* trap in the tree: `card` was inserted immediately
/// above `card_modal` and slid in under its doc comment, leaving `card_modal` with
/// none and `card`'s own summary line stranded at the *end* of the block. Every
/// gate stayed green — a stolen paragraph is still in the same module, so the
/// links all resolve. The tell is a summary line that is not the run's first line.
pub(crate) fn card_modal(id: &str) -> egui::Modal {
    egui::Modal::new(egui::Id::new(id))
        // The design's `rgba(0,0,0,.5)`, against egui's default 100/255. A
        // modal over a *canvas* has to dim more than one over a form: the
        // artwork behind this is arbitrary, and at 100 a light document still
        // read as the thing in focus.
        .backdrop_color(egui::Color32::from_black_alpha(128))
        .frame(
            egui::Frame::NONE
                .fill(color::CARD)
                // **The constant, not its value** (§15 D701, `[S18.3-L3-05]`).
                // This was `text_a(23)` — the one place in the workspace that
                // respelled `color::DIVIDER`'s definition instead of naming it,
                // and `theme.rs`'s module doc ends *"change a token here and the
                // whole chrome follows"*. It would not have: retuning `DIVIDER`
                // moves the panel dividers, egui's `noninteractive.bg_stroke`
                // and the window stroke, and would have left the hairline round
                // **every modal card in the app** at the old weight. Nothing on
                // screen disagrees today and no gate ever could — the two fold
                // to the same `Color32` — so the divergence was one keystroke
                // away and would have been invisible when it arrived.
                .stroke(egui::Stroke::new(1.0, theme::color::DIVIDER))
                .corner_radius(egui::CornerRadius::same(10))
                // Deeper than a dropdown's, because this floats over
                // everything rather than over one panel.
                .shadow(egui::epaint::Shadow {
                    offset: [0, 24],
                    blur: 60,
                    spread: 0,
                    color: egui::Color32::from_black_alpha(128),
                })
                .inner_margin(egui::Margin::same(PAD as i8)),
        )
}

/// The title row every settings-shaped modal opens with: the name on the left, the
/// ✕ against the right margin. Returns whether the ✕ was pressed.
///
/// **The ✕ is [`ui::icon_button`] at 18/14 in both cards.** The dashboard's was at
/// 22/13 — a bigger box around a smaller glyph, which is the shape of difference
/// nobody can name and everybody sees.
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

/// The *Cancel* / *Save changes* pair, right-aligned, with the commit lit only
/// when there is something to commit. Returns the [`Decision`] a press made.
///
/// **Shared for the reason the footer is the one row a user reads twice**: it is
/// where they look to find out whether the form has taken anything, so two cards
/// whose commits light on different rules are two cards that disagree about what
/// *edited* means.
pub(crate) fn modal_footer(ui: &mut egui::Ui, dirty: bool) -> Option<Decision> {
    let mut decision = None;
    ui.add_space(FOOTER_GAP);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        // Rightmost, because it is added first in a right-to-left
        // row — and rightmost is where the design puts the commit.
        if text_button(
            ui,
            "Save changes",
            if dirty {
                FieldButton::On
            } else {
                FieldButton::Disabled
            },
        )
        .clicked()
            && dirty
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
/// **[`ui::eyebrow`]'s type and grey, not the design's**, which asks for 11px at
/// 20% of the text colour with letter-spacing. Every other section label in the app
/// is an `eyebrow`, and this card is read against the panels rather than on its
/// own; a modal whose section labels were a size and a shade of their own would be
/// the one surface that did not look like the app.
///
/// The rule is what the design adds over a panel's bare eyebrow, and it earns its
/// place here for a reason a panel does not have: a panel's sections are separated
/// by the card edges around them, and this card has none — three eyebrows down one
/// uninterrupted column read as a list of three labels. It is also what lets the air
/// around the header be *equal* rather than lopsided; see [`SECTION_AIR`].
///
/// ⚠️ **The row is allocated at [`EYEBROW_H`] rather than left to grow.** A
/// `ui.horizontal` takes at least `interact_size.y`, which is half a dozen points
/// taller than a 12pt eyebrow galley, and those points land *above and below* the
/// label — invisible in the source and part of what made this card's section
/// spacing come out 32/17 where 26/11 was written.
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
            // Whole device pixels, for the reason `ui::hairline_in_column` states at
            // length: a 1pt hairline on a half pixel reads as a colour change. This
            // used to be that expression written out, and inherited its even-scale
            // bug with it (§15 D479). ⚠️ It said "downwards" as well, which was the
            // old unconditional `floor + 0.5` — the even arm rounds now.
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
/// Closer than [`ROW_GAP`], because a caption belongs *to* the row above it rather
/// than being the next thing down the column.
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
/// **The label goes in the field, where the inspector puts `X`, `W` and `Level`,
/// rather than above it as the design has it.** A caption over a field is a shape
/// this app does not otherwise use, and the two spellings side by side are what
/// makes a modal look assembled from a different kit — the section eyebrow above
/// already says *Nudge*, so `Step` and `Shift` inside the fields say the rest.
///
/// **Whole pixels only**, and the range starts at 1: a nudge of nothing is an arrow
/// key that silently does nothing, which is indistinguishable from a broken keymap,
/// and a nudge of 0.1 is not a distance anyone moves a layer by. It shipped as
/// `Scrub::fine(_, 2)` over `0.01..=1000` and that was simply wrong about what the
/// control is for.
///
/// **The rounding is done here as well as by the scrub**, and both are needed:
/// [`Scrub::whole`] settles a *drag* and leaves typing alone on purpose — its own
/// doc argues a typed 1.5 is a real thing to want, which is true of a stroke width
/// and false of an arrow key. So `0.4` typed in becomes 1 and `1.6` becomes 2.
/// Rounding every frame is safe while the field is being typed into: a focused
/// `DragValue` keeps its own string buffer and only writes back on commit.
///
/// Returns whether the field is being **dragged**, which is how the caller knows to
/// snapshot the pair for a right-click cancel (`OndinApp::settings_scrub`).
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
        // 0.5, the rate every other whole-unit field in the app scrubs at — a
        // position, a size, a corner radius. There is no reason for the one field
        // that means "how far an arrow key moves" to travel at a different speed
        // from the fields it moves things past.
        Scrub::whole(0.5).range(crate::input::NudgeStep::MIN..=crate::input::NudgeStep::MAX),
        |d| d.custom_formatter(ui::number(0)),
    );
    // ⚠️ **Still here, and still only about the draft** — which is the point
    // worth stating rather than deleting. This rounds what the *field* holds so
    // the control cannot be left showing a fraction; what protects the *nudge*
    // is `NudgeStep::step`, at the reader, because Cancel and Escape throw this
    // copy away and a stored value out of range survives them all.
    *value = value
        .round()
        .clamp(crate::input::NudgeStep::MIN, crate::input::NudgeStep::MAX);
    resp.dragged()
}

/// A word on a [`ui::label_button`], sized to the word.
///
/// **Measured rather than given a constant width**, because the three worded
/// buttons in this card are *Cancel*, *Save changes* and *Clear* — three different
/// lengths,
/// and `label_button` centres its text in whatever box it is handed rather than
/// growing to fit. A fixed width would be one number chosen for the longest of them
/// and too wide for the other two.
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
/// **One decimal below ten of a unit and none above**, which is the rule that keeps
/// the row the same length as it fills up: `1.4 MB` says something `1 MB` does not,
/// where `183.7 MB` says nothing `184 MB` does not and is two characters wider for
/// it. Binary units, because that is what the budget this reports against is
/// counted in (`fonts::FONT_CACHE_BYTES` is `256 * 1024 * 1024`) — a `MB` that meant
/// 10⁶ here would make a full cache read as 268.
/// ⚠️ **`pub(crate)` because the dashboard's *Size* column reads it too, and
/// there is already a second, different `human_bytes` in `panels::export`.**
/// That one rounds *up* — `1025` is `2 KB` — because it estimates the size of a
/// file that does not exist yet and must not promise less than it delivers. This
/// one reports a size that is already on disk, where rounding to nearest is
/// simply what the number is. Two rules, two functions, on purpose; a third
/// would not be.
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

    /// The window the modal centres itself in. Big enough that the card is never
    /// the thing being clipped, which would make every measurement below a
    /// measurement of the window instead.
    const SCREEN: egui::Vec2 = egui::vec2(1200.0, 900.0);

    /// Every text run the open modal painted, as `(top-left, size, text)`.
    ///
    /// ⚠️ **Three passes, because a widget's interaction state is last frame's.**
    /// The buttons here read their own `Response` to pick a hover treatment, so a
    /// single pass measures a card that is still converging — `CLAUDE.md`'s note on
    /// `read_response`, and the reason two probes of one dropdown once disagreed.
    /// Nothing in this card animates its *geometry*, so three is settling rather
    /// than sampling.
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

    /// The ground colour under each label's button, **all read out of one frame**.
    ///
    /// ⚠️ **Two grounds compared across two calls is a comparison of two different
    /// frames, and it fails.** An `egui::Area` fades in, so every colour the card
    /// paints is tinted by however far through that fade the pass is — six passes
    /// deep the same `color::FIELD` reads `#21_21_22_B4` where three passes deep it
    /// reads `#12_12_12_5F`. That is what a first version of this helper did, once
    /// per label, and the untouched form failed against itself. So the labels come
    /// in together and leave together.
    ///
    /// Ten passes rather than three: enough for the fade to finish as well as the
    /// responses to settle, which makes the values the real ones rather than merely
    /// mutually consistent.
    ///
    /// The rect is found by containing the label's own centre. `label_button` paints
    /// two at the same place — a fill and a hairline stroke — and `find_map` takes
    /// the first, which is the fill.
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

    /// **The card is the design's 376 across, the column is what that leaves, and
    /// the caption still fits the two lines the design gives it.**
    ///
    /// The width is checked through the *content*, which is the only thing a frame
    /// with an inner margin lets a probe see, and `menu_inner_w` is arithmetic this
    /// app has got wrong before (§15 D307).
    ///
    /// ⚠️ **The overflow loop is a guard, not the biting half, and the reason is
    /// worth knowing.** `ui.label` *wraps* to the available width, so a sentence in
    /// this card physically cannot run out of it — a first version of this test
    /// claimed the caption would, and squeezing `CARD_W` to 300 proved the opposite:
    /// the caption re-wrapped and the loop passed clean. What the loop does cover is
    /// the runs `painter.text` puts down — the switch-row labels and the button
    /// words, none of which wrap and none of which are clipped by anything.
    ///
    /// So the caption is asserted on its *height* instead, which is the question it
    /// actually raises: a third line pushes the card taller than the design draws
    /// it, and that is what a longer sentence or a narrower column produces.
    ///
    /// Flip-checked twice, and each half bites separately: `CARD_W` at 300 fails the
    /// column, and one more clause on the caption takes it to 39pt and fails that.
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
        // And the column is *used*, or a card twice as wide would pass the loop
        // above without anything looking right.
        let widest = runs
            .iter()
            .map(|(p, s, _)| p.x + s.x - left)
            .fold(0.0_f32, f32::max);
        assert!(
            widest > column - 40.0,
            "the widest row is {widest:.1} in a {column} column — the card is not the \
             thing setting the width"
        );

        // The caption, on two lines. Measured as a height rather than a line count
        // because a galley's rows are not in the shape — one line at `CAPTION_PT`
        // lays out 13pt, so 26 is two and 39 would be three.
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

    /// **Every row in a section sits on one baseline as its own control.**
    ///
    /// The cache row is the one this exists for: the sentence and the *Clear*
    /// button are allocated in two different scopes, and getting them into the same
    /// one puts the sentence 2pt high — see the ⚠️ in `OndinApp::font_cache_row`.
    /// Asserted on the galley *centres*, because the two runs are different sizes
    /// and a shared `pos.y` would mean they were not aligned.
    ///
    /// Flip-checked by writing the label before the button in one `horizontal`:
    /// the sentence's centre lands at 581 against the button's 583.
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
            // The cache row: a sentence and a button.
            ("Counting the web font cache…", "Clear"),
            // The footer: two buttons, one of them wider.
            ("Cancel", "Save changes"),
            // A nudge field: a word prefix, the digits and a unit, three sizes.
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

    /// **The commit wears the accent only once there is something to commit.**
    ///
    /// A modal whose primary button is always live is a modal that cannot say
    /// whether the click did anything — and *Save changes* on an untouched form
    /// writes the file it just read. `FieldButton::Off` and `Disabled` share a
    /// ground, so the visible difference is the accent wash arriving, which is what
    /// this measures rather than the enum.
    ///
    /// Against *Cancel* in the same frame rather than against a constant, because
    /// an `Area` fades in and every colour in the card is tinted by however far
    /// through that fade the frame is.
    ///
    /// Flip-checked by handing `text_button` `FieldButton::On` unconditionally: the
    /// clean case fails, reporting the two grounds unequal.
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

        // One row moved, by the same field the modal's own switch writes.
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

    /// Every painted thing in the open card as a vertical interval, in order down
    /// the page — the backdrop excluded, since it spans the whole window and would
    /// be every row's nearest neighbour in both directions.
    ///
    /// Text *and* rects, because a section's neighbour above may be a field's ground
    /// (a rect) and below may be a switch's track (also a rect) or a label (text).
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

    /// **A section header has the same air above it as below it.**
    ///
    /// Reported after living with the card: *"padding top/bottom in sections is
    /// unbalanced… both Nudge and Layers have more spacing from the controls to the
    /// bottom than from the controls to the top. If I have to guess, a section has
    /// both padding bottom and margin-top, so they accumulate."* That guess was
    /// right — an `add_space` compensating for `item_spacing`, plus ~6pt of the
    /// eyebrow row's own box over its galley — and the card shipped at 32 above each
    /// header against 17 below.
    ///
    /// ⚠️ **Measured on ink, not on the layout boxes, and the tolerance is what that
    /// costs.** The boxes are exactly `SECTION_AIR` on both sides; the *ink* is
    /// not, because a `ui::switch_row` is 22pt tall around a 16pt track and centres
    /// it, so 3pt of that row's own padding lands between the header and the first
    /// thing the eye sees. Ink is what was reported, so ink is what is asserted, and
    /// 4pt is the slack a row's internal centring can account for — an order of
    /// magnitude under the 15pt that was reported.
    ///
    /// ⚠️ **The first header is excluded, and that is a real exemption rather than a
    /// convenience.** Nothing above *Nudge* is a section: it is the card's title,
    /// which stands off by `TITLE_GAP` on purpose, so the balance this test is
    /// about does not apply to it. It gets its own assertion below — one that would
    /// fail if the two gaps were ever collapsed into one number.
    ///
    /// Flip-checked by putting back the version this replaced — a 15pt `add_space`
    /// against an 11pt `item_spacing`, above an eyebrow row left to take
    /// `interact_size.y`: both headers fail, *Layers* reporting 32 above against 20
    /// below. The title assertion flip-checks separately, by dropping the
    /// `TITLE_GAP - SECTION_AIR` line: 21.5 against 18, too close to tell apart.
    #[test]
    fn a_section_header_has_equal_air_above_and_below_it() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.toggle_settings(&ctx);
        let runs = galleys(&mut app, &ctx);
        let bands = ink(&mut app, &ctx);
        // The nearest ink that ends before the header starts, and the nearest that
        // starts after it ends. The header's own galley is in `bands` too, hence the
        // strict comparisons.
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

        // And the title's standoff is *not* that number — it is the larger one, so a
        // reader can tell the card's own title from the headings under it.
        let (title_gap, first_row) = air("NUDGE");
        assert!(
            title_gap > first_row + 6.0,
            "the title stands off by {title_gap:.1}pt against the header's own \
             {first_row:.1}pt — too close to tell apart"
        );
    }

    /// **Both nudge fields hold whole pixels, however the number got there.**
    ///
    /// A drag settles through `Scrub::whole`; typing does not, deliberately — that
    /// constructor's own doc argues a typed `1.5` is a real thing to want, which is
    /// true of a stroke width and false of an arrow key. So `nudge_field` rounds
    /// as well, and this asserts the *field* does it rather than the scrub: the
    /// fixture puts fractions straight into the draft, which is the state a typed
    /// entry leaves it in, and one frame of drawing is what has to clean them up.
    ///
    /// Both directions and both fields: `0.4` rounds *down* past the floor and is
    /// clamped up to 1, `1.6` rounds up to 2. Reported as *"nudging by 0.1px is
    /// crazy"*, against a field that shipped as `Scrub::fine(_, 2)` over
    /// `0.01..=1000`.
    ///
    /// Flip-checked by dropping the `round()` — the pair comes back `1.0`/`1.6`, so
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

    /// **A right-click puts a scrubbed nudge back, and `cancel_gesture` fires at all
    /// for one.**
    ///
    /// Two claims, and the second is the one that is easy to lose. That function
    /// early-returns unless something is in flight, and its test for a field scrub
    /// leads on `ctx.dragged_id()` — which egui clears on the release of *any*
    /// button, so a right-click whose press and release fall inside one rendered
    /// frame reads an empty snapshot and cancels nothing (the fault behind
    /// `cancel_gate`'s own test). `settings_scrub` is armed for the whole scrub and
    /// does not blink, which is why it is a term in that test rather than only the
    /// thing the cancel winds back.
    ///
    /// Flip-checked twice: dropping `settings_scrub` from `cancel_gesture`'s `field`
    /// test makes it return `false` and leave the scrubbed value standing, and
    /// dropping the wind-back leaves the value standing with `true` returned.
    #[test]
    fn a_right_click_winds_a_scrubbed_nudge_back() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.toggle_settings(&ctx);
        let before = app.settings.as_ref().expect("open").nudge;

        // The state one frame of a live scrub leaves: the snapshot armed, the draft
        // moved on. Set directly rather than driven through the pointer, because
        // what is under test is the cancel and not egui's drag detection.
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

    /// **A cache that has not been counted must not read as an empty one.**
    ///
    /// Flip-checked by folding `None` into the `Some(0)` arm: the first assertion
    /// fails, reporting "Nothing in the web font cache" for a directory nobody has
    /// looked at.
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
    /// Flip-checked by dropping the `v < 10.0` arm: `1024` comes back `1 KB` rather
    /// than `1.0 KB`. The bands *above* ten pass either way — `840 KB` and `180 MB`
    /// are what a no-decimals version also prints — which is why the cases below ten
    /// are here at all.
    #[test]
    fn a_byte_count_reads_as_a_person_would_say_it() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(840 * 1024), "840 KB");
        assert_eq!(human_bytes(1024 * 1024 + 419_430), "1.4 MB");
        assert_eq!(human_bytes(180 * 1024 * 1024), "180 MB");
        // The cache's own budget, which is the largest number this row can show
        // for a cache the app itself is keeping — and the reason the units are
        // binary: 256 MiB in decimal MB is 268, which would read as the sweep
        // having failed.
        assert_eq!(human_bytes(256 * 1024 * 1024), "256 MB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
    }

    /// ⚠️ **A draft equal to the preferences is what dims *Save changes***, so the
    /// two directions of that comparison have to be a round trip. A field added to
    /// `Prefs` and forgotten in `Settings` makes the modal quietly unable to save
    /// it; a field added to `Settings` and forgotten in `apply` makes it quietly
    /// unable to *keep* it.
    ///
    /// Flip-checked by dropping the `nudge` line from `Settings::apply`: the
    /// round-trip assertion fails, reporting the default `1/10` where the edited
    /// `0.5/25` should be. Note it is the *third* assertion that bites and not the
    /// first — a form that reads correctly and writes nothing looks right until it
    /// is closed, which is exactly why the round trip is the shape of this test.
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
}
