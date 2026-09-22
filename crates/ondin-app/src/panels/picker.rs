//! The detached colour picker (`design/Editor.dc.html`).
//!
//! One picker, three faces — solid, linear, radial — that differ only in the
//! geometry controls above the ramp. It floats over the canvas and is dragged
//! rather than docked into the inspector, because a gradient is judged against
//! the artwork it sits on, not against a panel. It has no title bar, so egui
//! makes the whole card the grab surface — but only the parts of it that are
//! not themselves controls, which on this card is not much.
//!
//! **It holds no colour of its own.** Every frame it reads the brush back out of
//! the target slot and writes edits through the inspector's preview→commit valve
//! (§9.3), so a drag across the saturation plane is one undo step and the canvas
//! updates live. The only state carried between frames is intent the document
//! cannot express: which gradient stop is selected, the HSV it last wrote (RGB
//! forgets hue at zero saturation and most of the saturation range near black), and
//! a half-typed hex.
//!
//! **Two of this card's gestures the shared valve cannot see**, both handled in
//! [`OndinApp::pointer_slot`] below and both for the same underlying reason — these
//! controls are *position*-valued, so a colour exists the moment the pointer is over
//! them, where the valve is cut for controls whose value comes from a drag's delta. A
//! **click** sets neither `dragged` nor `changed`, so it commits directly; a **press**
//! sets neither either, so the frames before the gesture travels far enough to count
//! as a drag preview through `preview_slot` (§15 D129).
use super::paint::{self, PaintKind, PaintSlot};
use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use crate::ui::{
    self, Hsv, color_to_egui, field_row, hex_of, icon_button, mask_corners, parse_hex,
    segment_label,
};
use eframe::egui;
use eframe::egui::emath::GuiRounding as _;
use ondin_core::Brush;
use ondin_core::NodeId;
use ondin_core::kurbo::Size;
use ondin_core::peniko::Color;
/// The open picker. Dropped when the target stops existing or the user closes it.
pub struct Picker {
    pub node: NodeId,
    pub slot: PaintSlot,
    /// Which gradient stop the colour controls are editing.
    stop: usize,
    /// The HSV the picker last wrote, and the 8-bit colour it produced.
    ///
    /// **The handle's position is re-derived from the stored colour every frame, and
    /// HSV↔RGB does not round-trip near the edges of the cube.** Hue is the famous
    /// case — a grey has no hue, so dragging value to zero and back comes out red —
    /// and it was the only component carried. Saturation goes the same way near
    /// black, and worse: measured over the 218pt plane, at `v = 0.02` there are
    /// **nine distinct 8-bit colours across the whole saturation range**, and the
    /// recovered handle jumps up to 36pt sideways while the pointer moves smoothly.
    /// Value is fine throughout (a quarter of a point at most). Reported as the
    /// cursor jumping near the bottom of the wheel.
    ///
    /// Keyed by the colour it produced rather than gated on a threshold, so the
    /// remembered intent is trusted exactly while the slot still holds what the
    /// picker put there. An edit from anywhere else — the hex field, another panel,
    /// undo — changes the key and falls straight back to deriving from the colour.
    intent: Option<(Hsv, [u8; 4])>,
    /// A hex field mid-edit. `None` when the field is showing the brush.
    hex: Option<String>,
    /// Where the window goes, and whether it has been put there yet.
    ///
    /// Forced with `current_pos` on the first frame rather than offered as a
    /// `default_pos`, because egui keeps an area's position under its id for
    /// the life of the process and ignores `default_pos` once the window has
    /// been shown once. Without the override a picker the user had dragged
    /// would reopen where they left it — **which is not wanted here**. The
    /// picker is placed relative to the field it edits, and moving it is a way
    /// to clear space for the artwork underneath, not a preference about where
    /// pickers live. Every open starts beside the swatch that opened it.
    spawn: egui::Pos2,
    placed: bool,
}
impl Picker {
    pub fn new(node: NodeId, slot: PaintSlot, spawn: egui::Pos2) -> Self {
        Self {
            node,
            slot,
            stop: 0,
            intent: None,
            hex: None,
            spawn,
            placed: false,
        }
    }
    /// Retarget an already-open picker, keeping its position on screen.
    pub fn retarget(&mut self, node: NodeId, slot: PaintSlot) {
        if self.node != node || self.slot != slot {
            self.node = node;
            self.slot = slot;
            self.stop = 0;
            self.hex = None;
            // A different slot holds a different colour, so the remembered intent
            // describes nothing here. Its key would rule it out anyway; dropping it
            // says so rather than relying on two colours differing.
            self.intent = None;
        }
    }
}
/// Geometry from the design: a card of 11px-padded rows.
///
/// The width is **pinned**, not derived from the contents. The three faces do
/// not need the same room — radial's `CX`/`CY` pair beside the preview chip is
/// the widest thing in the panel — and an auto-sized window would jump every
/// time the user changed tabs. So the width is radial's requirement (a little
/// over the design's 222) and the other two faces stretch to it.
pub(super) const WIDTH: f32 = 240.0;
/// Roughly the tallest face, for placing the window clear of the screen edge.
pub(super) const HEIGHT_HINT: f32 = 300.0;
const PAD: f32 = 11.0;
/// ⚠️ **26 until the popovers were levelled** (§15 D386): the picker is one more
/// card hanging beside the panel, so its rows are [`ui::CONTROL_H`] like the
/// panel's and the two popovers'.
const ROW_H: f32 = ui::CONTROL_H;
/// The **track**'s thickness — the hue ramp and the alpha strip — not a knob
/// diameter (§15 D766).
///
/// 🚨 **Named `SLIDER_H` until D766, in a module that also imports `ui::`, where
/// [`ui::SLIDER_H`] is a different quantity of a different value.** That one is
/// the **knob diameter** (11.0, with a 3pt rail under it); this is the rail
/// itself, and this module's knob is a fixed 6.5/7.0 that the constant does not
/// derive. §15 D673 read the pair as *"two hand-rolled sliders and two rules about
/// whether a row owes its knob any space"* and asked which was right — but
/// **unifying the value would have been a category error**: setting this to 11
/// thickens the two gradient strips, re-corners them (`mask_corners` reads
/// `× 0.5`), and leaves the knob exactly where it was.
///
/// ⚠️ **And the two sliders do not disagree the way D673 says.** Measured: this
/// one bounds its knob **vertically** — that is what the `+ 6.0` at
/// `slider_track` buys, 7.0 of reach against a 7.5 half-row — and lets it hang
/// **7pt off each horizontal end**, unclamped, where `ui::slider` does the reverse
/// (1pt of shadow below the row, and travel clamped so the knob cannot leave the
/// rail). They bound different axes; neither is *the* answer, and there was no
/// "which is right" to rule on as posed.
const TRACK_H: f32 = 9.0;
const STOP_BAR_H: f32 = 11.0;
/// Corner radius of the document-palette chips along the bottom of the card.
const SWATCH_RADIUS: f32 = 5.0;
/// The picker window's name, which is also its `Area` id and so its layer's.
///
/// Named because a *second* popup has to recognise a click that landed in it: the
/// typography popup can open this picker from a decoration's swatch, and would
/// otherwise dismiss itself — and so the picker — the moment it was touched. The
/// rect is not available to that caller (the picker draws after the inspector), so
/// identity is the only question it can ask. See [`layer_id`].
const WINDOW_NAME: &str = "colour-picker";

/// The layer the picker draws into.
pub(super) fn layer_id() -> egui::Id {
    egui::Id::new(WINDOW_NAME)
}

/// The picker's window, minus its position and its contents.
///
/// Shared with the test below rather than spelled twice, because the thing that
/// has to be true is a property of *this* builder — a test that rebuilt it
/// would keep passing after someone removed the line it exists to defend.
///
/// ⚠️ **These five lines were sitting on `WINDOW_NAME` above** (§15 D717,
/// `[S23.1-L3-06]`), stacked at the *bottom* of that constant's own doc with no
/// blank `///` between the two — so a `&str` constant claimed to be "a builder"
/// that is "shared with the test below", `WINDOW_NAME`'s real summary line was
/// buried mid-paragraph where rustdoc will not use it, and **this function
/// documented itself nowhere**. `layer_id` was inserted between the two and kept
/// its own doc, which is what makes the boundary hard to see on a read.
///
/// 🚨 **The stolen sentence is an instruction, which is what made this cost
/// something.** *"A test that rebuilt it would keep passing after someone removed
/// the line it exists to defend"* tells a future maintainer **not** to inline this
/// builder into `the_picker_opens_where_it_was_spawned_on_the_very_first_frame` —
/// and it was filed under a string constant, where the person about to do that
/// would never meet it. See `CLAUDE.md`'s *Editing files* list, which keeps the
/// running tally of these and the two checks that find them.
fn picker_window() -> egui::Window<'static> {
    egui::Window::new(WINDOW_NAME)
        .title_bar(false)
        .resizable(false)
        // min == max pins the width; the height still follows the face.
        .min_width(WIDTH)
        .max_width(WIDTH)
        // **Height zero, and do not "correct" it to the real one.**
        //
        // On the frame an `Area` first appears it has no stored size, so it
        // lays out once invisibly to measure itself — and for that pass it
        // assumes `Style::spacing::default_area_size`, 600×400. egui then
        // constrains *that* box to the screen, which shoves a picker opened
        // near the bottom edge upwards (and leftwards, on a screen narrower
        // than `spawn.x + 600`) by the overhang of a window that does not
        // exist. The shoved position is what gets stored, and by the next frame
        // the picker is already `placed`, so `current_pos` never corrects it:
        // measured 60px out at 1600×900 and 160px at 1280×800, on the first
        // opening only, since the measured size outlives the window closing.
        //
        // An estimate of zero cannot overhang anything, so it cannot be clamped
        // too far; the real height is still measured on that same pass. An
        // estimate *larger* than the true height brings the bug back in
        // proportion, which is why the honest 294 would be the wrong answer.
        .default_size(egui::vec2(WIDTH, 0.0))
        // Above the inspector column, which is also a floating area: dragged
        // over it, the picker has to stay on top or it vanishes mid-edit.
        .order(egui::Order::Foreground)
        .frame(
            egui::Frame::NONE
                .fill(color::CARD)
                .stroke(egui::Stroke::new(1.0, theme::color::text_a(15)))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 10],
                    blur: 24,
                    spread: 0,
                    color: egui::Color32::from_black_alpha(72),
                })
                .corner_radius(egui::CornerRadius::same(5))
                .inner_margin(egui::Margin::same(PAD as i8)),
        )
}
impl OndinApp {
    /// Draw the picker if one is open. Closes itself when its target has gone
    /// (the layer was deleted, or the fill row it points at was removed).
    pub(crate) fn picker_ui(&mut self, ctx: &egui::Context) {
        let Some(p) = &self.picker else { return };
        let (node, slot, spawn, placed) = (p.node, p.slot, p.spawn, p.placed);
        // The picker belongs to the layer it was opened from, so it goes away
        // when that layer does — deselected, or replaced by another. Leaving it
        // up means a panel quietly editing something the canvas no longer shows
        // as chosen. The canvas ground is the mirror image: it is what the
        // inspector shows when *nothing* is selected, so selecting anything
        // closes it.
        let orphaned = match slot {
            // The ground's picker is what the inspector offers when *nothing*
            // is selected, so selecting anything closes it — a guide included.
            // `is_empty` would be the wrong question here: it answers about
            // layers, and a guide selection leaves it true while the inspector
            // has already moved on to the Guide panel (`Selection`).
            PaintSlot::Canvas => !self.session.selection.nothing(),
            // A guide's picker belongs to the guides the inspector is showing,
            // so it goes away when they stop being the selection — the same
            // rule as a layer's, with the guides standing in for the node.
            // `guides()`, not `guide()`: the latter answers "and is it the only
            // one", which would close the picker the moment a second guide was
            // Shift-clicked, exactly when a colour over the pair is what is
            // wanted.
            PaintSlot::Guide => self.session.selection.guides().is_empty(),
            // These three belong to the *selection* rather than to any one layer,
            // so what orphans them is the selection emptying — not a particular
            // node leaving it. Changing which layers are selected re-scopes the
            // edit instead of cancelling it, which is what makes it possible to
            // add a layer to the set while the picker is up.
            PaintSlot::Selection(..)
            | PaintSlot::SelectionAll(_)
            | PaintSlot::GroupColor(_)
            | PaintSlot::GroupGradient(_) => self.session.selection.ids().is_empty(),
            // **Both character-scoped slots** (`paint::char_scoped` is the same
            // pair), and the only arm here that orphans on a **row** rather than on
            // a node: the rows that open them live on the Character tab of a popup
            // that can be closed or switched away from, so a picker left up over a
            // hidden row is the "quietly editing something no longer shown" this
            // whole list prevents.
            //
            // A `TextDecoration` switched off needs no term of its own —
            // `slot_brush`'s `char_slot_color` reads `None` and the close below
            // answers it. ⚠️ **Whether an absent run colour behaves the same way is
            // not established**, and that is worth knowing rather than assuming:
            // the sentence was written for the decoration half, before `TextColor`
            // joined the arm (§15 D154).
            PaintSlot::TextDecoration(_) | PaintSlot::TextColor => {
                self.type_menu != Some(crate::app::TypeTab::Character)
                    || !self.session.selection.contains(node)
            }
            // **The third arm that orphans on a *row* rather than on a node**
            // (§15 D476). An effect row is drawn only where the selected layers
            // **agree** — the card says *"these layers have different effects"*
            // and draws no rows at all otherwise — so a picker left up over a
            // disagreement is a control whose row is not on screen, which is the
            // *"quietly editing something no longer shown"* this whole list
            // prevents.
            //
            // ⚠️ **The catch-all below is not enough and is what let this
            // through.** `[S14.5-L1-01]`: `PaintSlot::Effect` fell into
            // `!selection.contains(node)`, and the anchor is still contained after
            // a Shift-click widens the selection — so the picker survived a
            // widening the *card* did not, and one click on its colour plane
            // replaced another layer's whole stack. The four slots two arms above
            // survive a widening deliberately, and can: every one of them is
            // scoped to the selection, so re-scoping is harmless. `Effect(i)` is
            // scoped to **one node's list** and inherited the survival anyway.
            PaintSlot::Effect(_) => {
                !self.session.selection.contains(node)
                    || self.shared_effects(self.session.selection.ids()).is_none()
            }
            // **The fourth arm that orphans on a row, and the fourth slot the
            // catch-all below was governing that it should not have been**
            // (§15 D553, `[S23.1-L1-04]`). The Layout grid card draws its rows
            // only where the selected frames **agree** — `shared_grids` answers
            // `None` otherwise and the card shows a sentence instead — so the
            // `Effect` arm's whole argument applies here verbatim, and this slot
            // was left out of it.
            //
            // ⚠️ **The damage was narrower than `Effect`'s and it was still
            // silent.** `slot_transaction`'s `Grid` arm writes through
            // `frame_subjects()` rather than through `node`, and reads *each
            // subject's own list* — so a hex typed after a Shift-click widened
            // the selection recoloured the other frame's grid while keeping its
            // track count, on a row that had stopped being drawn. The `Effect`
            // arm clones the anchor's whole `Vec`, which is why that one is worse
            // and why fixing it did not reach this.
            //
            // **`frame_subjects()` and not `selection.ids()`, which is the one
            // place this differs from the arm above**: the card is drawn over the
            // artboards in the selection, so a Shift-clicked *rect* does not make
            // the grids disagree and must not close the picker.
            PaintSlot::Grid(_) => {
                !self.session.selection.contains(node)
                    || self.shared_grids(&self.frame_subjects()).is_none()
            }
            _ => !self.session.selection.contains(node),
        };
        let Some(brush) = self.slot_brush(node, slot).filter(|_| !orphaned) else {
            self.picker = None;
            return;
        };
        let box_size = self.local_box(node).unwrap_or(Size::new(100.0, 100.0));
        let mut close = false;
        let mut window = picker_window();
        if !placed {
            window = window.current_pos(spawn);
        }
        window.show(ctx, |ui| {
            ui.set_width(WIDTH - PAD * 2.0);
            // Named rather than repeated — one number across every popover in the
            // app (§15 D275). The value here does not move; what changes is that
            // retuning it now moves all five together.
            ui.spacing_mut().item_spacing.y = crate::ui::POPOVER_SECTION_GAP;
            close = self.picker_body(ui, node, slot, &brush, box_size);
        });
        if let Some(p) = &mut self.picker {
            p.placed = true;
        }
        if close {
            self.picker = None;
        }
    }
    /// Returns true when the user asked to close.
    fn picker_body(
        &mut self,
        ui: &mut egui::Ui,
        node: NodeId,
        slot: PaintSlot,
        brush: &Brush,
        box_size: Size,
    ) -> bool {
        let kind = paint::kind_of(brush);
        // **The colour picker has nothing to say about a picture, so it closes
        // rather than drawing itself over one.** Every control below is written
        // against the stop list, and an image's is a placeholder grey invented
        // for the swatch (`paint::stops_of`) — so the panel would offer a hue
        // wheel, a stop bar and a hex field for a colour the picture does not
        // have. The writes are already refused (`paint::with_stops` returns an
        // image unchanged), which is what makes this a *looks* wrong rather than
        // a *is* wrong, and one guard is the whole fix.
        //
        // An image row does not open the picker in the first place
        // (`inspector::paint_row`); this is the second half, for a picker left
        // open over a slot whose brush has since become an image — a relink, an
        // undo, a paste.
        if kind == PaintKind::Image {
            return true;
        }
        let inner_w = ui.available_width();
        let mut close = false;
        // Kind tabs + close. The track takes everything the close button and the
        // gap before it do not, so the row lands exactly on the right margin.
        //
        // **A slot that cannot hold a gradient shows the tabs anyway, with Linear
        // and Radial disabled.** They used to be dropped entirely, on the grounds
        // that three tabs where two collapse back to the first is worse than none.
        // What that actually left was a header holding nothing but a close button,
        // reported as looking empty — and it also made the guide's picker look like
        // a *different, smaller* panel rather than the same one with two choices
        // unavailable. Disabled says which it is (§9.4, the identity row's mask
        // button makes the same bargain), and the row keeps the height it has
        // everywhere else.
        const CLOSE_W: f32 = 18.0;
        const TAB_GAP: f32 = 6.0;
        let gradients = slot.takes_gradients();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = TAB_GAP;
            let selected = PaintKind::ALL.iter().position(|k| *k == kind).unwrap_or(0);
            let track = inner_w - CLOSE_W - TAB_GAP;
            let live = |i: usize| gradients || PaintKind::ALL[i] == PaintKind::Solid;
            let picked = ui::segmented_enabled(
                ui,
                track,
                ui::SEGMENT_CELL_H,
                3,
                selected,
                live,
                |p, i, rect, on| match live(i) {
                    true => segment_label(p, rect, PaintKind::ALL[i].label(), on),
                    false => ui::segment_label_disabled(p, rect, PaintKind::ALL[i].label()),
                },
            );
            if let Some(i) = picked
                && PaintKind::ALL[i] != kind
            {
                let next = paint::convert(brush, PaintKind::ALL[i], box_size);
                self.write_slot(node, slot, next);
                if let Some(p) = &mut self.picker {
                    p.stop = 0;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(ui, icon::X, CLOSE_W, 14.0, false, true).clicked() {
                    close = true;
                }
            });
        });
        // Geometry controls, then the stop bar — gradients only.
        match kind {
            PaintKind::Solid => {}
            PaintKind::Linear => self.linear_controls(ui, node, slot, brush, box_size),
            PaintKind::Radial => self.radial_controls(ui, node, slot, brush, box_size),
            // Unreachable: the guard at the top of this function returned. Spelled
            // out rather than wildcarded so that removing that guard stops
            // compiling here instead of silently drawing a stop bar over a photo.
            PaintKind::Image => {}
        }
        if kind != PaintKind::Solid {
            self.stop_bar(ui, node, slot, brush, inner_w);
        }
        // The colour controls act on the selected stop.
        let stops = paint::stops_of(brush);
        let index = self
            .picker
            .as_ref()
            .map(|p| p.stop)
            .unwrap_or(0)
            .min(stops.len() - 1);
        let current = stops[index].1;
        let plane_h = if kind == PaintKind::Solid {
            104.0
        } else {
            84.0
        };
        self.sv_plane(ui, node, slot, brush, index, current, inner_w, plane_h);
        self.hue_slider(ui, node, slot, brush, index, current, inner_w);
        if kind == PaintKind::Solid && slot.takes_alpha() {
            self.alpha_slider(ui, node, slot, brush, index, current, inner_w);
        }
        self.hex_row(ui, node, slot, brush, index, current, inner_w);
        if kind == PaintKind::Solid {
            self.document_swatches(ui, node, slot, brush, index, current, inner_w);
        }
        close
    }
    // --- gradient geometry -------------------------------------------------
    /// Preview chip, angle field, and the two stop-order controls.
    fn linear_controls(
        &mut self,
        ui: &mut egui::Ui,
        node: NodeId,
        slot: PaintSlot,
        brush: &Brush,
        box_size: Size,
    ) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 7.0;
            self.preview_chip(ui, brush);
            let size = egui::vec2(ui.available_width(), ROW_H);
            field_row(ui, size, |ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                ui.label(theme::icon_text(
                    icon::ARROW_CLOCKWISE,
                    14.0,
                    theme::text::FAINT,
                ));
                let mut deg = paint::linear_angle(brush).unwrap_or(0.0);
                let resp = ui.add(
                    // The two plain `DragValue`s in this popover are the only
                    // numeric fields the app draws without going through
                    // `ui::value_field` or `ui::bare_drag_value`, so they take
                    // the app's two field-wide decisions from
                    // `ui::plain_drag_value` rather than inherit them (§15 D552).
                    // This one is safe from the clamp for a reason and the CX/CY
                    // pair below was not: `paint::linear_angle` ends
                    // `rem_euclid(360.0)`, so its value cannot leave the range.
                    ui::plain_drag_value(&mut deg)
                        .suffix("°")
                        .speed(ui::scrub(0.5))
                        .max_decimals(0)
                        .range(0.0..=360.0),
                );
                let next = paint::with_linear_angle(brush, deg % 360.0, box_size);
                self.valve_slot(&resp, node, slot, next);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if icon_button(ui, icon::ARROWS_CLOCKWISE, 18.0, 14.0, false, true)
                        .on_hover_text("Reverse the stops")
                        .clicked()
                    {
                        let next = paint::reverse_stops(brush);
                        self.write_slot(node, slot, next);
                    }
                    if icon_button(ui, icon::FLIP_HORIZONTAL, 18.0, 14.0, false, true)
                        .on_hover_text("Flip 180°")
                        .clicked()
                    {
                        let deg = paint::linear_angle(brush).unwrap_or(0.0);
                        let next = paint::with_linear_angle(brush, (deg + 180.0) % 360.0, box_size);
                        self.write_slot(node, slot, next);
                    }
                });
            });
        });
    }
    /// Preview chip with a draggable centre handle, plus CX/CY fields.
    fn radial_controls(
        &mut self,
        ui: &mut egui::Ui,
        node: NodeId,
        slot: PaintSlot,
        brush: &Brush,
        box_size: Size,
    ) {
        let (cx0, cy0) = paint::radial_centre(brush, box_size).unwrap_or((0.5, 0.5));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 7.0;
            // Dragging inside the chip moves the gradient's centre — the same
            // gesture as on canvas, at thumbnail scale.
            let chip = self.preview_chip(ui, brush);
            let p = ui.painter();
            let handle = chip.rect.min
                + egui::vec2(
                    chip.rect.width() * cx0 as f32,
                    chip.rect.height() * cy0 as f32,
                );
            p.circle_stroke(handle, 3.0, egui::Stroke::new(1.2, egui::Color32::WHITE));
            if let Some(pos) = chip.interact_pointer_pos() {
                let cx = ((pos.x - chip.rect.min.x) / chip.rect.width()).clamp(0.0, 1.0);
                let cy = ((pos.y - chip.rect.min.y) / chip.rect.height()).clamp(0.0, 1.0);
                let next = paint::with_radial_centre(brush, cx as f64, cy as f64, box_size);
                self.pointer_slot(&chip, node, slot, next);
            }
            // Split what is left after the chip, allowing for the one gap
            // between the two fields — the row's own `item_spacing`, so the pair
            // finishes flush with the right margin.
            let fw = (ui.available_width() - 7.0) / 2.0;
            for (label, value, is_x) in [("CX", cx0, true), ("CY", cy0, false)] {
                let mut pct = value * 100.0;
                let resp = field_row(ui, egui::vec2(fw, ROW_H), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(
                        egui::RichText::new(label)
                            .size(10.0)
                            .color(theme::text::FAINT),
                    );
                    ui.add(
                        // ⚠️ **`ui::plain_drag_value` for the reason the angle
                        // field above gives, and this is the field that earned
                        // it** (§15 D552, `[S23.1-L1-01]`). A radial centre is
                        // `paint::radial_centre`'s `c.x / w` and is not clamped
                        // — SVG allows a centre outside the shape, and a
                        // highlight off one edge is why anyone writes one. With
                        // egui's default this field painted `100` for a stored
                        // `1.5` and then `mark_changed`, so a bare click through
                        // it moved the gradient 50 units and spent an undo step.
                        // The `range` still bounds what a scrub or a keystroke
                        // may *produce*, which is the `clamp` two lines below.
                        ui::plain_drag_value(&mut pct)
                            .suffix("%")
                            .speed(ui::scrub(0.5))
                            .max_decimals(0)
                            .range(0.0..=100.0),
                    )
                });
                // ⚠️ **Not `.clamp(0.0, 1.0)`, which was the *second* half of
                // `[S23.1-L1-01]` and the half its fix sketch did not name**
                // (§15 D552). With the field no longer rewriting a stored `150`
                // to `100`, this line rewrote `1.5` to `1.0` instead — and
                // `valve_slot` commits whatever `next` holds on the valve's
                // `engaged → !engaged` transition, whether the user moved
                // anything or not. So the fix that stopped the *field* lying
                // still moved the gradient, and the galley was the only thing
                // that got better.
                //
                // **Redundant as well as harmful**: `.range(0.0..=100.0)` above
                // bounds what a drag or a keystroke may reach, which is exactly
                // what `clamp_existing_to_range(false)` leaves in place (D425).
                // The clamp only ever fired on a value the document already held.
                let f = pct / 100.0;
                let next = if is_x {
                    paint::with_radial_centre(brush, f, cy0, box_size)
                } else {
                    paint::with_radial_centre(brush, cx0, f, box_size)
                };
                self.valve_slot(&resp, node, slot, next);
            }
        });
    }
    /// The 34px gradient thumbnail. Returns its response so the radial face can
    /// drag the centre on it.
    fn preview_chip(&mut self, ui: &mut egui::Ui, brush: &Brush) -> egui::Response {
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::click_and_drag());
        let p = ui.painter();
        ui::paint_checkerboard(p, rect, 5.0);
        ui::paint_ramp(p, rect, &ramp(brush));
        mask_corners(p, rect, 5.0, color::CARD);
        p.rect_stroke(
            rect,
            egui::CornerRadius::same(5),
            egui::Stroke::new(1.0, egui::Color32::from_black_alpha(90)),
            egui::StrokeKind::Inside,
        );
        resp
    }
    /// The stop bar: the ramp with a draggable handle per stop. Clicking the bar
    /// adds a stop where it was clicked (sampled, so the ramp does not jump);
    /// right-clicking a handle removes it.
    fn stop_bar(
        &mut self,
        ui: &mut egui::Ui,
        node: NodeId,
        slot: PaintSlot,
        brush: &Brush,
        width: f32,
    ) {
        let stops = paint::stops_of(brush);
        let selected = self.picker.as_ref().map(|p| p.stop).unwrap_or(0);
        // Room for handles that overhang the bar top and bottom.
        let (row, _) =
            ui.allocate_exact_size(egui::vec2(width, STOP_BAR_H + 12.0), egui::Sense::empty());
        let bar = egui::Rect::from_min_size(
            egui::pos2(row.min.x, row.center().y - STOP_BAR_H * 0.5),
            egui::vec2(width, STOP_BAR_H),
        );
        let p = ui.painter();
        ui::paint_checkerboard(p, bar, 5.0);
        // **`stop_colours`, not `ramp`** — see that function. This bar is the
        // editing surface for the stops and sits under the opacity slider; fading
        // it as the slider moves would dim the handles being dragged (§15 D773).
        ui::paint_ramp(p, bar, &stop_colours(brush));
        mask_corners(p, bar, 5.0, color::CARD);
        // The bar is registered *before* the handles that sit on it. egui gives
        // an overlapping click to whichever widget was registered last, so this
        // ordering is what makes grabbing a handle grab the handle rather than
        // dropping a new stop underneath it.
        let bar_resp = ui.interact(bar, ui.id().with("stop-bar"), egui::Sense::click());
        let mut hit_handle = false;
        let mut add_stop: Option<f32> = None;
        let mut remove: Option<usize> = None;
        for (i, (off, col)) in stops.iter().enumerate() {
            let x = bar.min.x + bar.width() * off.clamp(0.0, 1.0);
            let h =
                egui::Rect::from_center_size(egui::pos2(x, bar.center().y), egui::vec2(11.0, 17.0));
            let resp = ui.interact(
                h.expand(2.0),
                ui.id().with(("stop", i)),
                egui::Sense::click_and_drag(),
            );
            if resp.hovered() || resp.dragged() {
                hit_handle = true;
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            let on = i == selected;
            let p = ui.painter();
            p.rect_filled(h, egui::CornerRadius::same(3), color_to_egui(*col));
            p.rect_stroke(
                h,
                egui::CornerRadius::same(3),
                egui::Stroke::new(
                    1.5,
                    if on {
                        egui::Color32::WHITE
                    } else {
                        theme::color::text_a(140)
                    },
                ),
                egui::StrokeKind::Outside,
            );
            if (resp.clicked() || resp.drag_started())
                && let Some(pk) = &mut self.picker
            {
                pk.stop = i;
                pk.hex = None;
            }
            if resp.secondary_clicked() && stops.len() > 2 {
                remove = Some(i);
            }
            // A plain click only selects the stop; only a drag moves it, so that
            // reaching for a stop's colour never nudges its offset.
            if (resp.dragged() || resp.drag_stopped())
                && let Some(pos) = resp.interact_pointer_pos()
            {
                let t = ((pos.x - bar.min.x) / bar.width()).clamp(0.0, 1.0);
                let mut next = stops.clone();
                next[i].0 = t;
                self.valve_slot(&resp, node, slot, paint::with_stops(brush, next));
            }
        }
        // A click on bare ramp adds a stop there.
        if bar_resp.clicked()
            && !hit_handle
            && let Some(pos) = bar_resp.interact_pointer_pos()
        {
            add_stop = Some(((pos.x - bar.min.x) / bar.width()).clamp(0.0, 1.0));
        }
        if let Some(t) = add_stop {
            let mut next = stops.clone();
            next.push((t, paint::sample(&stops, t)));
            next.sort_by(|a, b| a.0.total_cmp(&b.0));
            let index = next.iter().position(|s| s.0 == t).unwrap_or(0);
            self.write_slot(node, slot, paint::with_stops(brush, next));
            if let Some(pk) = &mut self.picker {
                pk.stop = index;
                pk.hex = None;
            }
        } else if let Some(i) = remove {
            let mut next = stops.clone();
            next.remove(i);
            let last = next.len().saturating_sub(1);
            self.write_slot(node, slot, paint::with_stops(brush, next));
            if let Some(pk) = &mut self.picker {
                pk.stop = pk.stop.min(last);
                pk.hex = None;
            }
        }
    }
    // --- colour controls ---------------------------------------------------
    #[allow(clippy::too_many_arguments)]
    fn sv_plane(
        &mut self,
        ui: &mut egui::Ui,
        node: NodeId,
        slot: PaintSlot,
        brush: &Brush,
        index: usize,
        current: Color,
        width: f32,
        height: f32,
    ) {
        let hsv = self.hsv_of(current);
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click_and_drag());
        let p = ui.painter();
        ui::paint_sv_plane(p, rect, hsv.h);
        mask_corners(p, rect, 5.0, color::CARD);
        let handle = egui::pos2(
            rect.min.x + rect.width() * hsv.s,
            rect.min.y + rect.height() * (1.0 - hsv.v),
        );
        p.circle_stroke(handle, 5.5, egui::Stroke::new(1.5, egui::Color32::WHITE));
        p.circle_stroke(
            handle,
            6.5,
            egui::Stroke::new(1.0, egui::Color32::from_black_alpha(100)),
        );
        if let Some(pos) = resp.interact_pointer_pos() {
            let s = ((pos.x - rect.min.x) / rect.width()).clamp(0.0, 1.0);
            let v = 1.0 - ((pos.y - rect.min.y) / rect.height()).clamp(0.0, 1.0);
            let next = Hsv::new(hsv.h, s, v, hsv.a);
            self.write_stop_colour(&resp, node, slot, brush, index, next);
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn hue_slider(
        &mut self,
        ui: &mut egui::Ui,
        node: NodeId,
        slot: PaintSlot,
        brush: &Brush,
        index: usize,
        current: Color,
        width: f32,
    ) {
        let hsv = self.hsv_of(current);
        let (rect, resp) = self.slider_track(ui, width);
        let p = ui.painter();
        // Six segments round the hue wheel.
        let ramp: Vec<(f32, egui::Color32)> = (0..=6)
            .map(|i| {
                let h = i as f32 / 6.0;
                (h, Hsv::new(h.min(0.999), 1.0, 1.0, 1.0).to_egui())
            })
            .collect();
        ui::paint_ramp(p, rect, &ramp);
        mask_corners(p, rect, TRACK_H * 0.5, color::CARD);
        slider_knob(p, rect, hsv.h, Hsv::new(hsv.h, 1.0, 1.0, 1.0).to_egui());
        if let Some(pos) = resp.interact_pointer_pos() {
            let h = ((pos.x - rect.min.x) / rect.width()).clamp(0.0, 1.0);
            let next = Hsv::new(h, hsv.s, hsv.v, hsv.a);
            self.write_stop_colour(&resp, node, slot, brush, index, next);
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn alpha_slider(
        &mut self,
        ui: &mut egui::Ui,
        node: NodeId,
        slot: PaintSlot,
        brush: &Brush,
        index: usize,
        current: Color,
        width: f32,
    ) {
        let hsv = self.hsv_of(current);
        let (rect, resp) = self.slider_track(ui, width);
        let p = ui.painter();
        ui::paint_checkerboard(p, rect, TRACK_H * 0.5);
        let opaque = Hsv::new(hsv.h, hsv.s, hsv.v, 1.0).to_egui();
        let [r, g, b, _] = opaque.to_srgba_unmultiplied();
        let clear = egui::Color32::from_rgba_unmultiplied(r, g, b, 0);
        ui::paint_ramp(p, rect, &[(0.0, clear), (1.0, opaque)]);
        mask_corners(p, rect, TRACK_H * 0.5, color::CARD);
        slider_knob(p, rect, hsv.a, opaque);
        if let Some(pos) = resp.interact_pointer_pos() {
            let a = ((pos.x - rect.min.x) / rect.width()).clamp(0.0, 1.0);
            let next = Hsv::new(hsv.h, hsv.s, hsv.v, a);
            self.write_stop_colour(&resp, node, slot, brush, index, next);
        }
    }
    /// A 9pt slider track in a 15pt row, so the knob clears it vertically.
    ///
    /// **The `+ 6.0` is the knob's room and it is only the vertical half** (§15
    /// D766). [`slider_knob`] draws to radius 7.0 — a 6.5 disc under a 1pt stroke
    /// at 6.5 — against a half-row of 7.5, so 0.5 of slack top and bottom. ⚠️ **On
    /// the horizontal axis it is unclamped**: the centre is
    /// `track.min.x + track.width() * t`, so at `t = 0` and `t = 1` the knob hangs
    /// **7pt past each end** of the allocated row. That lands inside the card's
    /// [`PAD`] of 11 and has always looked right; nothing said so until
    /// `the_alpha_knob_stays_inside_the_cards_padding_at_both_ends`.
    ///
    /// ⚠️ **`ui::slider` bounds the other axis** — its travel is clamped to
    /// `half..width - half` so the knob cannot leave the rail, and its shadow
    /// leaves the row by 1pt below. So the two hand-rolled sliders in this app
    /// bound *different* axes rather than disagreeing about whether a row owes its
    /// knob room, which is what §15 D673 asked and is not a question with a side to
    /// take.
    fn slider_track(&self, ui: &mut egui::Ui, width: f32) -> (egui::Rect, egui::Response) {
        let (row, resp) = ui.allocate_exact_size(
            egui::vec2(width, TRACK_H + 6.0),
            egui::Sense::click_and_drag(),
        );
        let rect = egui::Rect::from_min_size(
            egui::pos2(row.min.x, row.center().y - TRACK_H * 0.5),
            egui::vec2(width, TRACK_H),
        );
        (rect, resp)
    }
    /// Hex + opacity for the selected stop. Hex is buffered so a half-typed
    /// value is not parsed on every keystroke.
    #[allow(clippy::too_many_arguments)]
    fn hex_row(
        &mut self,
        ui: &mut egui::Ui,
        node: NodeId,
        slot: PaintSlot,
        brush: &Brush,
        index: usize,
        current: Color,
        width: f32,
    ) {
        let stops = paint::stops_of(brush);
        let single = stops.len() == 1;
        field_row(ui, egui::vec2(width, ROW_H), |ui| {
            ui.spacing_mut().item_spacing.x = 9.0;
            let mut text = self
                .picker
                .as_ref()
                .and_then(|p| p.hex.clone())
                .unwrap_or_else(|| hex_of(current));
            let resp = ui.add(
                egui::TextEdit::singleline(&mut text)
                    .frame(egui::Frame::NONE)
                    .desired_width(60.0)
                    .char_limit(ui::HEX_CHAR_LIMIT)
                    .font(egui::FontId::proportional(11.5)),
            );
            ui::select_all_on_focus(ui, &resp, &text);
            if (resp.has_focus() || resp.changed())
                && let Some(p) = &mut self.picker
            {
                p.hex = Some(text.clone());
            }
            // Applied on defocus (which Enter also triggers), not per keystroke:
            // typing "6D8CD9" one character at a time would otherwise be six
            // commits, and "6D" would land as a real colour on the way. Clearing
            // the buffer is also what strips a typed or pasted `#` — the field
            // falls back to `hex_of`, which never writes one.
            //
            // 🚨 **[`ui::defocus_commits`] and not a bare `lost_focus()`** (§15
            // D841). `Escape` surrenders focus like every other way out, so a
            // bare check commits the value the user was cancelling — **the same
            // input and the same two numbers §15 D808 records as the bug**, at a
            // site D808 did not enumerate. Its own entry names the four fields
            // it audited and never claims four is the population, so this is a
            // gap rather than a live entry re-argued.
            //
            // ⚠️ **No gate sees this.** `nothing_commits_on_the_expression_d316_removed`
            // fires only where `lost_focus()` sits within 160 characters of
            // `changed()`; here they are ~700 apart, in two separate conditions,
            // and that gate is about D316's compound expression rather than
            // about `Escape`. What finds this shape is
            // `grep -rn "lost_focus()"` and reading each hit.
            //
            // ⚠️ **The condition stays `lost_focus()` and the *write* is what
            // `Escape` suppresses**, which is not interchangeable: the buffer
            // clear at the end of this block has to run on every defocus, or a
            // cancelled edit leaves its typed text sitting in the field for the
            // next person to look at. Gating the whole block was the first shape
            // of this fix and it did exactly that.
            if resp.lost_focus() {
                // 🚨 **Nothing is written unless the typed colour differs from
                // the one that is there** (§15 D730, `[S23.1-L1-05]`) —
                // `inspector::paint_hex_field`'s guard, which has carried it since
                // §15 D517 and which this independently-written copy never got.
                //
                // ⚠️ **Compared as *bytes*, not as components**, for the reason
                // that entry gives: `text` is seeded from `hex_of`, which rounds
                // each channel to 8 bits, and `parse_hex` hands them back for
                // `from_rgba8` to re-expand — so a colour off the 8-bit lattice,
                // **which is anything this picker's own HSV plane produced**, came
                // back *changed* by a click that typed nothing. Measured here:
                // `[0.5, 0.5, 0.5, 1.0]` → `[0.5019608, …]`, and an undo step with
                // it. Comparing the floats would not catch that — they really do
                // differ; what has to match is what the *field* can express.
                //
                // ⚠️ **`SelectionAll` is exempt for D540's reason**, the same
                // exemption `paint_hex_field` makes: over a set that disagrees
                // there is no one colour to compare against, and picking them all
                // to one is exactly the edit being asked for. `current` there is
                // the stand-in brush, so comparing against it would refuse the
                // edit whenever the stand-in happened to match what was typed.
                let mixed = matches!(slot, PaintSlot::SelectionAll(_));
                let unchanged = !mixed && parse_hex(&text) == parse_hex(&hex_of(current));
                let abandoned = !ui::defocus_commits(&resp);
                if let Some([r, g, b]) = parse_hex(&text).filter(|_| !unchanged && !abandoned) {
                    let rgb = Color::from_rgba8(r, g, b, 255);
                    // ⚠️ **Over a selection, each paint keeps its own opacity**
                    // (§15 D540) — `inspector::paint_hex_field`'s branch, which
                    // has carried this since it was written and whose comment is
                    // the whole argument: *"this field owns the colour and the
                    // field at the other end of the row owns the number, and over
                    // a set that disagrees there is no one number to send with the
                    // colour."* Here that other field is literally in the same
                    // `field_row`, eight lines down.
                    //
                    // `[S23.1-L1-02]`: `current` over a *Mixed* row is the
                    // **stand-in** brush — one member of the disagreeing set —
                    // so its alpha went out through `slot_transaction`'s
                    // `SelectionAll` arm, which writes the whole brush onto every
                    // paint in scope. Two layers at 100% and 40%, type `00FF00`,
                    // press Enter: both come back green **at 100%**, and the 40%
                    // is gone. The single-layer control in the same run keeps its
                    // alpha, which is what says the loss is the *scope* and not
                    // this field's arithmetic.
                    //
                    // **Not the picker's general rule, and that is why this is a
                    // branch rather than a change to the slot.** The plane and the
                    // strips send a whole brush to every layer on purpose — *"a
                    // gradient chosen in the picker is a gradient the user asked
                    // every one of them to have"* — and they have no separate
                    // opacity field. The hex field does.
                    if let PaintSlot::SelectionAll(target) = slot {
                        let tx = self.edit_paints_all(target, move |b| {
                            Brush::Solid(rgb.with_alpha(paint::alpha_of(b)))
                        });
                        self.commit_edit(tx);
                        if let Some(p) = &mut self.picker {
                            p.hex = None;
                        }
                        return;
                    }
                    let a = current.components[3];
                    let mut next = stops.clone();
                    next[index].1 = rgb.with_alpha(a);
                    self.write_slot(node, slot, paint::with_stops(brush, next));
                }
                if let Some(p) = &mut self.picker {
                    p.hex = None;
                }
            }
            if !slot.takes_alpha() {
                return;
            }
            // A gradient's stops each have their own alpha, so there is no one
            // alpha slider for them; this field scales the whole ramp, which is
            // the same number the fill row shows.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut pct = if single {
                    current.components[3] * 100.0
                } else {
                    paint::alpha_of(brush) * 100.0
                };
                let r = ui::bare_drag_value(
                    ui,
                    egui::DragValue::new(&mut pct)
                        .suffix("%")
                        .speed(ui::scrub(0.5))
                        .max_decimals(0)
                        .range(0.0..=100.0),
                );
                // ⚠️ **This out-clamp is the shape §15 D552 deleted from the
                // CX/CY field, and it stays** — recorded because a reader taking
                // that entry's *"the clamp only ever fired on a value the document
                // already held"* as general will find one still in this function.
                // The difference is the producer: a radial centre can legitimately
                // be `1.5`, where an alpha is meant to be `0..1`.
                //
                // 🚨 **And the sentence that used to end here is why the clamp
                // stays: *"an alpha **is** 0..1 by construction … this clamp cannot
                // fire"*. It gained a producer on 2026-09-15 and became false**
                // (§15 D767, D713's lesson one field later). `alpha_of` now returns
                // `GradientBrush::opacity` for a gradient — a `#[serde(default)]`
                // field, so any six characters in a `.ondin` become it — where
                // before it folded `Color::components[3]`, which the importer had
                // already bounded. **A safety argument that enumerates its producers
                // is false the day a second one appears**, and this one enumerated
                // two by name.
                //
                // It is guarded in three places now — `build::brush_is_finite`'s
                // `valid_opacity` at the op boundary, `alpha_of`'s own clamp, and
                // here — and this is the one that would catch a fourth producer
                // nobody has thought of. **Not dead weight any more.**
                let pct = pct.clamp(0.0, 100.0);
                let a = (pct / 100.0).clamp(0.0, 1.0);
                let next = if single {
                    let mut s = stops.clone();
                    s[index].1 = current.with_alpha(a);
                    paint::with_stops(brush, s)
                } else {
                    paint::with_alpha(brush, a)
                };
                self.valve_slot(&r, node, slot, next);
            });
        });
    }
    /// The colours already in this document, most-used first. A real palette
    /// beats a decorative one: it is how a second shape gets the exact fill the
    /// first one has.
    #[allow(clippy::too_many_arguments)]
    fn document_swatches(
        &mut self,
        ui: &mut egui::Ui,
        node: NodeId,
        slot: PaintSlot,
        brush: &Brush,
        index: usize,
        current: Color,
        width: f32,
    ) {
        const N: usize = 6;
        let palette = self.document_palette(N);
        if palette.is_empty() {
            return;
        }
        let gap = 5.0;
        let cw = (width - gap * (palette.len() - 1) as f32) / palette.len() as f32;
        let (row, _) = ui.allocate_exact_size(egui::vec2(width, 14.0), egui::Sense::empty());
        let mut chosen = None;
        for (i, col) in palette.iter().enumerate() {
            // **Rounded to whole pixels — but not for the fill's sake.** epaint
            // snaps every `RectShape` to the pixel grid itself
            // (`TessellationOptions::round_rects_to_pixels`), so the chip's own
            // edges come out crisp from a fractional `cw` either way; that was
            // measured, and it is *not* what fixed the dirty right-hand side
            // (§15 D32). What epaint does not snap is a `Mesh` — so this is
            // here to put `mask_corners` and `ui.interact` on the same grid the
            // fill has already been moved to, instead of a fractional one.
            let rect = egui::Rect::from_min_size(
                egui::pos2(row.min.x + i as f32 * (cw + gap), row.min.y),
                egui::vec2(cw, 14.0),
            )
            .round_to_pixels(ui.pixels_per_point());
            let resp = ui.interact(rect, ui.id().with(("swatch", i)), egui::Sense::click());
            let p = ui.painter();
            let radius = egui::CornerRadius::same(SWATCH_RADIUS as u8);
            // **One rounded rect, and nothing underneath it.**
            //
            // The chip used to be a checkerboard, then a square fill, then
            // `mask_corners` carving the corners back out. Two artefacts came
            // out of that. The mask emits a `Mesh`, which epaint rasterizes
            // without feathering — deliberately, see its own note on the spikes
            // an anti-aliased fill throws off a zero-degree vertex — so the
            // corners were hard-stepped, and nothing here drew the smoothing
            // hairline the other call sites use. Worse, `paint_checkerboard`
            // lays down a **white** base before its grey cells, and a rounded
            // fill over it is anti-aliased against that white rather than
            // against the card: a pale halo around every chip, on all four
            // sides, however clean the corners were.
            //
            // So the board is only drawn when there is transparency to show,
            // and then inset so the fill's soft edge never lands on it.
            //
            // **One physical pixel, not one point.** `shrink(1.0)` is 1.5
            // device pixels at 150%, and epaint rounds the near and far edges
            // the same way, so the gap came out 2px on the left and top against
            // 1px on the right and bottom — measured, and visible as a chip
            // that looks off-centre inside its own outline. Dividing by the
            // scale lands the board on the pixel grid the snapped chip is
            // already on, and the ring is 1px on all four sides at 100%, 125%,
            // 150% and 200%.
            if col.components[3] < 1.0 {
                let inset = 1.0 / ui.pixels_per_point();
                ui::paint_checkerboard(p, rect.shrink(inset), 4.0);
                mask_corners(p, rect.shrink(inset), SWATCH_RADIUS - inset, color::CARD);
            }
            p.rect_filled(rect, radius, color_to_egui(*col));
            // The same hairline the inspector's chips wear, for the same reason
            // and against a ground only 10 levels off theirs — the palette is
            // built from the document's own colours, so a dark drawing fills this
            // row with chips that would otherwise be six invisible gaps. It goes
            // on unconditionally, *under* the in-use ring below rather than
            // instead of it: one says "this is a chip", the other "this is the
            // one you have".
            p.rect_stroke(
                rect,
                radius,
                egui::Stroke::new(1.0, ui::SWATCH_HAIRLINE),
                egui::StrokeKind::Inside,
            );
            // Ring the one already in use.
            if col.components[..3] == current.components[..3] {
                p.rect_stroke(
                    rect,
                    radius,
                    egui::Stroke::new(1.5, color::ACCENT_200),
                    egui::StrokeKind::Outside,
                );
            }
            if resp.clicked() {
                chosen = Some(*col);
            }
        }
        if let Some(col) = chosen {
            // The swatch carries the colour, the alpha slider owns the alpha —
            // and over a selection that means **each paint's own** alpha, not the
            // stand-in's (§15 D540). Same sentence as `hex_row`'s above, same
            // missing scope check, and `[S23.1-L1-02]` names both: one fix has to
            // cover the two, because a Mixed row's swatch grid and its hex field
            // are the same edit by two doors.
            if let PaintSlot::SelectionAll(target) = slot {
                let tx = self.edit_paints_all(target, move |b| {
                    Brush::Solid(col.with_alpha(paint::alpha_of(b)))
                });
                self.commit_edit(tx);
                if let Some(p) = &mut self.picker {
                    p.hex = None;
                }
                return;
            }
            let mut stops = paint::stops_of(brush);
            stops[index].1 = col.with_alpha(current.components[3]);
            self.write_slot(node, slot, paint::with_stops(brush, stops));
            if let Some(p) = &mut self.picker {
                p.hex = None;
            }
        }
    }
    /// Distinct solid colours used by the document's fills, strokes and artboard
    /// backgrounds, most-used first.
    ///
    /// The whole-document scope of the same census the Group Colors panel runs
    /// over a selection (`build::colors_in`). One walk serves both: this row and
    /// that panel are the same question asked of different subtrees, and two
    /// copies would answer it differently the first time either learnt about a
    /// new place a colour can hide.
    fn document_palette(&self, want: usize) -> Vec<Color> {
        ondin_core::colors_in(&self.session.doc, &[self.session.doc.root()])
            .into_iter()
            .take(want)
            .map(|u| u.color)
            .collect()
    }
    // --- plumbing ----------------------------------------------------------
    /// HSV for a colour, honouring what the picker last wrote — see
    /// [`Picker::intent`] for the three components and why one of them is not
    /// enough.
    fn hsv_of(&self, c: Color) -> Hsv {
        hsv_for(self.picker.as_ref().and_then(|p| p.intent), c)
    }
    /// Write one stop's colour, remembering the HSV that produced it.
    fn write_stop_colour(
        &mut self,
        resp: &egui::Response,
        node: NodeId,
        slot: PaintSlot,
        brush: &Brush,
        index: usize,
        hsv: Hsv,
    ) {
        if let Some(p) = &mut self.picker {
            // Keyed by the colour, so the intent is trusted only while the slot
            // still holds it.
            p.intent = Some((hsv, hsv.to_color().to_rgba8().to_u8_array()));
            p.hex = None;
        }
        let mut stops = paint::stops_of(brush);
        if index >= stops.len() {
            return;
        }
        stops[index].1 = hsv.to_color();
        let next = paint::with_stops(brush, stops);
        self.pointer_slot(resp, node, slot, next);
    }
    /// Commit a slot edit driven by a pointer gesture: the **press** shows the colour
    /// at once, a drag keeps showing it, and the release commits.
    ///
    /// `edit_valve` alone is not enough for these, twice over. It fires on `dragged` /
    /// `drag_stopped` / `changed`, none of which a plain click sets — so a single
    /// click on the saturation plane or the hue bar would move the handle and
    /// then change nothing.
    ///
    /// **And none of which a *press* sets either**, which is the second gap and was
    /// reported as "you click, you see nothing happening, and only after you move the
    /// mouse it actually starts changing the color". egui does not call a gesture a
    /// drag until it has travelled (`Flags::DRAGGED` waits on
    /// `interact_widgets.dragged`), so the frames between button-down and the first
    /// movement fell through both branches.
    ///
    /// **The press is already a value on these controls, and that is what tells them
    /// apart from a value field.** A plane or a strip reads the pointer's *position*,
    /// so where the button went down names a colour on its own; a `DragValue` reads
    /// its *delta*, so a press there carries nothing and must stay silent. That
    /// difference is why the fix belongs here rather than in the shared valve.
    ///
    /// Shown rather than committed, because the gesture is not over: the release
    /// commits through `clicked()` above if it never travelled, or through
    /// `drag_stopped()` in the valve if it did — so every press this previews is
    /// followed by a commit that clears the preview.
    fn pointer_slot(&mut self, resp: &egui::Response, node: NodeId, slot: PaintSlot, brush: Brush) {
        // A cancelled drag across the plane must not land as a click on
        // release, which is the one way a colour could still get through.
        if self.gesture_cancelled {
            return;
        }
        if resp.clicked() {
            self.write_slot(node, slot, brush);
            return;
        }
        // `is_pointer_button_down_on` is true from the press frame to the release,
        // where `dragged` is true only once egui has decided. The two overlap once
        // travel begins, and the valve owns that half.
        if resp.is_pointer_button_down_on()
            && !resp.dragged()
            && self.preview_slot(node, slot, brush.clone())
        {
            // The colour on screen just changed, so the selection chrome owes the same
            // retreat it owes every other inspector edit (§15 D128). `true` because
            // the button is down: the 1.5s must not start counting until it lifts.
            // Nothing downstream would do it — `edit_note` reads `dragged()` and
            // `changed()`, and a raw sensed region sets neither on the press frame.
            self.note_live_edit(true);
            return;
        }
        self.valve_slot(resp, node, slot, brush);
    }
}
/// A slider knob: a filled circle in the value's own colour, ringed white.
fn slider_knob(p: &egui::Painter, track: egui::Rect, t: f32, fill: egui::Color32) {
    let c = egui::pos2(
        track.min.x + track.width() * t.clamp(0.0, 1.0),
        track.center().y,
    );
    p.circle_filled(c, 6.5, fill);
    p.circle_stroke(c, 5.5, egui::Stroke::new(2.0, egui::Color32::WHITE));
    p.circle_stroke(
        c,
        6.5,
        egui::Stroke::new(1.0, egui::Color32::from_black_alpha(100)),
    );
}
/// A brush's stops as an egui ramp, **with `GradientBrush::opacity` folded in** —
/// what the paint looks like, for every surface that is showing a paint.
///
/// 🚨 **This doc said the opposite of the function under it for four days** (§15
/// D784). It was written as the *statement of the defect* — "it does not fold in
/// `GradientBrush::opacity`, and since §15 D767 that is a gap" — and D773 then
/// fixed the gap by folding the multiplier in **here**, three lines below, without
/// the paragraph following. **A comment that describes the bug you are fixing is
/// the one a fix leaves standing**, because its author reads it as the problem
/// statement rather than as a claim about the code, and the more precisely it
/// describes the defect the more invisible it becomes once the defect is gone.
/// Nothing gates it: it compiles, it lints, its links resolve.
///
/// ⚠️ **It also proposed the design that was not taken.** *"That is what makes this
/// a caller-side fix rather than a change here"* — the fix is not caller-side. The
/// multiplier is folded in here and [`stop_colours`] is the named opt-out, so the
/// common case is the default and the one surface that edits the stops themselves
/// opts out by name. That is the better shape and it is the one shipped; the
/// sentence describing the alternative outlived the decision to reject it.
///
/// ⚠️ **The picture arm is closed too now** (§15 D784), so the asymmetry this
/// paragraph used to record is gone: [`crate::ui::Swatch::Picture`] and the layers
/// row both fold `ImageSampler::alpha` in.
pub(super) fn ramp(brush: &Brush) -> Vec<(f32, egui::Color32)> {
    let opacity = match brush {
        Brush::Gradient(g) => g.opacity.clamp(0.0, 1.0),
        _ => 1.0,
    };
    stop_colours(brush)
        .into_iter()
        .map(|(off, c)| (off, c.gamma_multiply(opacity)))
        .collect()
}

/// The stops as they are **stored**, with no ramp opacity folded in — for the one
/// surface that is editing those stops rather than showing the paint (§15 D773).
///
/// 🚨 **[`ramp`] folds `GradientBrush::opacity` in and this does not, and the
/// defaulting is deliberate.** Three of the four callers are *chips* — the picker's
/// 34×34 preview, the inspector's paint row, the Type panel's text swatch — and
/// they must look like the canvas. One is the **stop bar**, which sits directly
/// under the opacity slider and is the surface for editing the stops themselves; a
/// bar that faded as the slider moved would be dimming the very handles the user is
/// dragging. So the common case is the default and the exception opts out by name:
/// a new chip written next year gets it right by doing nothing.
///
/// ⚠️ **This gap was introduced by §15 D767 and is `[S23.2-L1-01]`'s shape — §15
/// D564's *one document drawn two ways in one frame*.** Until the ramp's opacity
/// moved off the stops, `stops_of` *was* what the paint looked like and every chip
/// was right for free. **A derived value that becomes a stored one turns every
/// reader of the old derivation into a bug**, silently, and no gate can see it.
///
/// ⚠️ **The image arm is closed** (§15 D784). D773 recorded it as a real defect it
/// was deliberately not taking — `ImageSampler::alpha` folded in by neither
/// function, so a half-faded photograph had an opaque thumbnail — precisely so
/// that fixing the gradient alone did not read as a decision somebody took. It was
/// then put to the maintainer and taken. **Writing down the half you are not doing
/// is what got it done**, which is the argument for the paragraph rather than for
/// the silence.
pub(super) fn stop_colours(brush: &Brush) -> Vec<(f32, egui::Color32)> {
    paint::stops_of(brush)
        .into_iter()
        .map(|(off, c)| (off, color_to_egui(c)))
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    /// A translucent chip's checkerboard is inset by exactly one **physical**
    /// pixel, on every side, at every scale the app is likely to meet.
    ///
    /// `shrink(1.0)` is the obvious spelling and it is wrong above 100%: at
    /// 150% it insets by 1.5 device pixels, epaint rounds the near and far
    /// edges the same way, and the gap lands 2px on the left and top against
    /// 1px on the right and bottom. Arithmetic rather than pixels, because
    /// that is all it takes — the chip rect is already snapped, so an inset
    /// that is a whole number of device pixels keeps the board on the grid.
    #[test]
    fn the_translucent_chip_board_is_inset_one_device_pixel_all_round() {
        for ppp in [1.0_f32, 1.25, 1.5, 2.0] {
            // A deliberately fractional chip: the row divided by three.
            let chip = egui::Rect::from_min_size(
                egui::pos2(148.0 + 1.0 / 3.0, 100.25),
                egui::vec2(69.0 + 1.0 / 3.0, 14.0),
            )
            .round_to_pixels(ppp);
            let board = chip.shrink(1.0 / ppp);
            for (name, gap) in [
                ("left", board.min.x - chip.min.x),
                ("right", chip.max.x - board.max.x),
                ("top", board.min.y - chip.min.y),
                ("bottom", chip.max.y - board.max.y),
            ] {
                let device = gap * ppp;
                assert!(
                    (device - 1.0).abs() < 1e-3,
                    "@{ppp}: {name} gap is {device} device px, want 1"
                );
            }
        }
    }
    /// The picker opens where it was asked to **the first time too**.
    ///
    /// It did not, and the asymmetry is the tell: an `Area` measures itself on
    /// the frame it first appears, assuming 600×400 until it knows better, and
    /// egui constrains that imaginary box to the screen. A picker spawned near
    /// the bottom was shoved up by the overhang — 60px at 1600×900, 160px at
    /// 1280×800 — and the shoved position was stored before `placed` turned
    /// off the correction. Every *later* opening was fine, because the measured
    /// size outlives the window closing. See `picker_window`.
    ///
    /// The spawns below are chosen to overhang the 600×400 estimate on both
    /// screens while fitting the real window, so the right answer is `spawn`
    /// untouched and a regression shows up as a plain mismatch.
    #[test]
    fn the_picker_opens_where_it_was_spawned_on_the_very_first_frame() {
        for (w, h) in [(1600.0_f32, 900.0_f32), (1280.0, 800.0)] {
            for ppp in [1.0, 1.25, 1.5, 2.0] {
                let ctx = egui::Context::default();
                crate::theme::install(&ctx);
                ctx.set_pixels_per_point(ppp);
                let input = || egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::pos2(0.0, 0.0),
                        egui::vec2(w, h),
                    )),
                    ..Default::default()
                };
                // One pass to make fonts available before anything is measured.
                let _ = ctx.run_ui(input(), |_| {});
                let spawn = egui::pos2(w - 500.0, h - 340.0);
                let body = |ui: &mut egui::Ui| {
                    ui.set_width(WIDTH - PAD * 2.0);
                    ui.allocate_space(egui::vec2(WIDTH - PAD * 2.0, 270.0));
                };
                // Frame one: the picker has just opened, so it is positioned.
                let _ = ctx.run_ui(input(), |_| {
                    picker_window().current_pos(spawn).show(&ctx, body);
                });
                // Frame two: `placed`, so the app leaves the position alone —
                // this is the frame the user actually sees.
                let mut seen = None;
                let _ = ctx.run_ui(input(), |_| {
                    seen = picker_window().show(&ctx, body).map(|r| r.response.rect);
                });
                let seen = seen.expect("the picker is on screen");
                assert!(
                    seen.min.distance(spawn) < 1.0,
                    "{w}x{h} @{ppp}: opened at {:?}, asked for {spawn:?} \
                     (window is {:?}, which fits without clamping)",
                    seen.min,
                    seen.size(),
                );
            }
        }
    }
}

/// The HSV to draw the handle at: the picker's remembered intent while the slot
/// still holds the colour it produced, otherwise derived from that colour.
///
/// **A free function because it is the whole decision** — so it can be asserted on
/// its own terms rather than through a panel that would have to be drawn first.
///
/// The key comparison is what makes it safe: an intent is only reused for the exact
/// colour it produced, so this can never report a colour the slot does not hold.
fn hsv_for(intent: Option<(Hsv, [u8; 4])>, c: Color) -> Hsv {
    match intent {
        Some((hsv, key)) if key == c.to_rgba8().to_u8_array() => hsv,
        _ => Hsv::from_color(c),
    }
}

#[cfg(test)]
mod handle_tests {
    use super::*;

    /// **The handle stays where the pointer put it, near black.** The plane is
    /// 218×104 and the handle's position is re-derived from the *stored* colour every
    /// frame, so it is only as stable as HSV↔RGB is invertible. It is not: at
    /// `v = 0.02` the whole saturation range maps to **nine** distinct 8-bit colours,
    /// and deriving `s` back out of one of them moved the handle up to 36pt sideways
    /// while the pointer moved smoothly. Reported as the cursor jumping near the
    /// bottom of the wheel.
    ///
    /// Both halves matter, so both are asserted: the intent is honoured for the
    /// colour it produced, and ignored for any other — otherwise the picker could
    /// show a colour the slot does not hold.
    #[test]
    fn the_handle_honours_what_the_picker_wrote_and_only_that() {
        const W: f32 = 218.0;
        for v in [0.02_f32, 0.01, 0.006] {
            for s in [0.5_f32, 0.85] {
                let wrote = Hsv::new(0.6, s, v, 1.0);
                let stored = wrote.to_color();
                let key = stored.to_rgba8().to_u8_array();

                // Derived: the jump this exists to remove.
                let derived = hsv_for(None, stored);
                let drift = (derived.s - s).abs() * W;

                // Remembered: exactly where the pointer was.
                let kept = hsv_for(Some((wrote, key)), stored);
                assert_eq!(
                    kept, wrote,
                    "the intent must be honoured for the colour it produced \
                     (deriving instead drifts {drift:.1}pt at v={v}, s={s})"
                );
            }
        }

        // A colour the picker did not write falls back to deriving, so the handle can
        // never claim a colour the slot does not hold — a hex typed elsewhere, or undo.
        let wrote = Hsv::new(0.6, 0.85, 0.02, 1.0);
        let elsewhere = Color::from_rgba8(200, 30, 30, 255);
        assert_eq!(
            hsv_for(
                Some((wrote, wrote.to_color().to_rgba8().to_u8_array())),
                elsewhere
            ),
            Hsv::from_color(elsewhere),
            "a stale intent must not describe someone else's colour"
        );
    }
}

#[cfg(test)]
mod pointer_slot_tests {
    //! **Which of the three slot verbs a pointer state selects** — the picker's
    //! frame-by-frame call pattern, which the roadmap carried as read-only until
    //! §15 D303 and which my own notes record as having burned three
    //! code-reasoned fixes for one frozen picker before an `eprintln!` named the
    //! cause in a single run. *"Who calls this and how often" is never in the text
    //! of the function*, so it is asserted here by driving real frames rather than
    //! by reading `OndinApp::pointer_slot`.
    //!
    //! The three states are told apart by egui, not by us: a **click** sets neither
    //! `dragged` nor `changed` and commits directly; the frames of a **press** that
    //! has not yet travelled far enough to be a drag preview; and everything else
    //! goes to the valve. Getting the middle one wrong is what makes a picker look
    //! frozen — the press paints nothing until the pointer moves.
    use super::*;
    use crate::app::OndinApp;
    use ondin_core::{Operation, Transaction};

    const AREA: egui::Rect = egui::Rect {
        min: egui::pos2(0.0, 0.0),
        max: egui::pos2(400.0, 400.0),
    };

    /// A headless app with one rect carrying one white fill, and that fill's slot.
    fn app_with_a_fill() -> (egui::Context, OndinApp, NodeId, PaintSlot) {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let id = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: ondin_core::NodeKind::Rect {
                size: Size::new(100.0, 100.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        }]))
        .expect("a rect");
        app.session.adopt_document(doc, None);
        app.session
            .try_commit(Transaction(vec![Operation::SetFills {
                id,
                fills: vec![ondin_core::Fill {
                    brush: Brush::Solid(Color::WHITE),
                    visible: true,
                }],
            }]))
            .expect("a fill to edit");
        (ctx, app, id, PaintSlot::Fill(0))
    }

    /// Run one frame, handing `pointer_slot` a response over the whole area.
    fn frame(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>, brush: &Brush) {
        let node = app.session.selection.ids().first().copied();
        let input = egui::RawInput {
            screen_rect: Some(AREA),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            let resp = ui.interact(
                AREA,
                egui::Id::new("picker-plane"),
                egui::Sense::click_and_drag(),
            );
            if let Some(node) = node {
                app.pointer_slot(&resp, node, PaintSlot::Fill(0), brush.clone());
            }
        });
    }

    fn press(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// **A press previews and the release that completes a click commits** — the two
    /// halves of the pattern, and the ones that are wrong together when the picker
    /// looks dead.
    ///
    /// A press sets neither `dragged` nor `changed`, so the shared valve is cut for
    /// it; without the explicit `is_pointer_button_down_on() && !dragged()` arm
    /// nothing at all happens until the pointer travels, which is exactly "I press
    /// on the plane and the colour does not change". The assertion is therefore that
    /// the **preview** is installed on the press frame, before any movement.
    #[test]
    fn a_press_previews_before_it_travels_and_the_click_commits() {
        let (ctx, mut app, id, _) = app_with_a_fill();
        app.session.selection.set_one(id);
        let red = Brush::Solid(Color::from_rgb8(255, 0, 0));
        let shown = |app: &OndinApp| {
            app.session
                .display_node(id)
                .map(|n| n.paint().fills[0].brush.clone())
        };
        let at = egui::pos2(200.0, 200.0);

        // Warm-up: egui resolves a press against last frame's widget rects.
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)], &red);
        let before = app.session.revision();

        frame(
            &ctx,
            &mut app,
            vec![egui::Event::PointerMoved(at), press(at, true)],
            &red,
        );
        assert_eq!(
            shown(&app),
            Some(red.clone()),
            "the press frame has to preview, or the plane does nothing until the \
             pointer moves — which is what a frozen picker looks like"
        );
        assert_eq!(
            app.session.revision(),
            before,
            "and it is a preview, so the whole gesture stays one undo step"
        );

        frame(&ctx, &mut app, vec![press(at, false)], &red);
        assert_ne!(
            app.session.revision(),
            before,
            "the release completes a click, and a click commits directly — neither \
             `dragged` nor `changed` is ever set for one, so the valve cannot"
        );
        assert_eq!(shown(&app), Some(red), "on the colour the press showed");
    }

    /// **A cancelled gesture must not land as a click on release**, which is the one
    /// way a colour could still get through after Escape.
    ///
    /// Asserted at the *release*, because that is where it would get through: the
    /// press is already gone by then and `clicked()` is the state that fires
    /// afterwards. A guard checked only on the press frame would pass a test that
    /// looked at the press.
    #[test]
    fn a_cancelled_gesture_does_not_land_as_a_click() {
        let (ctx, mut app, id, _) = app_with_a_fill();
        app.session.selection.set_one(id);
        let red = Brush::Solid(Color::from_rgb8(255, 0, 0));
        let at = egui::pos2(200.0, 200.0);

        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)], &red);
        frame(
            &ctx,
            &mut app,
            vec![egui::Event::PointerMoved(at), press(at, true)],
            &red,
        );
        let before = app.session.revision();

        // Escape's mark, set by `cancel_gesture` and read for the rest of the press.
        app.gesture_cancelled = true;
        frame(&ctx, &mut app, vec![press(at, false)], &red);
        assert_eq!(
            app.session.revision(),
            before,
            "a cancelled gesture releasing over the plane must commit nothing"
        );
    }
}

#[cfg(test)]
mod mixed_hex_tests {
    //! **The picker's hex field carries a colour, not an opacity** — §15 D540,
    //! `[S23.1-L1-02]`.
    //!
    //! Over a *Mixed* row `current` is the **stand-in** brush — one member of the
    //! disagreeing set — and its alpha went out with the colour through
    //! `slot_transaction`'s `SelectionAll` arm, which writes the whole brush onto
    //! every paint in scope. Two layers at 100% and 40%, `00FF00` typed and
    //! committed: both came back green **at 100%**, and the 40% was gone.
    //!
    //! The guard has existed twelve hundred lines away since it was written —
    //! `inspector::paint_hex_field` — under the comment that is the whole
    //! argument: *"this field owns the colour and the field at the other end of
    //! the row owns the number, and over a set that disagrees there is no one
    //! number to send with the colour."* In `hex_row` that other field is
    //! literally in the same `field_row`.
    //!
    //! (Plain backticks, not `[links]` — §15 D319's convention.)

    use super::*;
    use crate::app::OndinApp;
    use crate::theme;
    use ondin_core::peniko::Color;
    use ondin_core::{
        Document, Fill, IdSource, NodeId, NodeKind, Operation, PaintTarget, Transaction,
        kurbo::Size,
    };

    const SCREEN: egui::Vec2 = egui::vec2(1200.0, 900.0);
    /// Measured by sweep: with the picker spawned at `(400, 300)` the hex field's
    /// hit band is **x 420…485, y 512…537**, and identical at 1.0, 1.25, 1.5 and
    /// 2.0 `pixels_per_point` (event positions are in points, so the scaling
    /// sweep is a null result rather than a confirmation — worth having run only
    /// because it says this is not a physical-pixel number that could drift).
    const HEX_FIELD: egui::Pos2 = egui::pos2(450.0, 524.0);

    /// Two rects, one solid fill each, at the two given alphas.
    fn two_rects(ctx: &egui::Context, alphas: (f32, f32)) -> (OndinApp, NodeId, NodeId) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x5A1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (a, b) = (ids.mint(), ids.mint());
        let rect = |id| Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        };
        let paint = |id, c: Color| Operation::SetFills {
            id,
            fills: vec![Fill {
                brush: Brush::Solid(c),
                visible: true,
            }],
        };
        doc.apply(&Transaction(vec![
            rect(a),
            rect(b),
            paint(a, Color::new([1.0, 0.0, 0.0, alphas.0])),
            paint(b, Color::new([0.0, 0.0, 1.0, alphas.1])),
        ]))
        .expect("build");
        app.session.adopt_document(doc, None);
        (app, a, b)
    }

    fn frame(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) {
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
                events,
                ..Default::default()
            },
            |ui| app.picker_ui(ui.ctx()),
        );
    }

    fn rgba8(app: &OndinApp, id: NodeId) -> [u8; 4] {
        match &app.session.doc.get(id).expect("node").paint().fills[0].brush {
            Brush::Solid(c) => c.to_rgba8().to_u8_array(),
            other => panic!("the fixture's fill is a solid colour, got {other:?}"),
        }
    }

    /// Click into the hex field, replace its text with `00FF00`, press `Enter`.
    fn type_a_green(ctx: &egui::Context, app: &mut OndinApp) {
        for _ in 0..2 {
            frame(ctx, app, Vec::new());
        }
        let button = |pressed| egui::Event::PointerButton {
            pos: HEX_FIELD,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(
            ctx,
            app,
            vec![egui::Event::PointerMoved(HEX_FIELD), button(true)],
        );
        frame(ctx, app, vec![button(false)]);
        // **One `Event::Text` replaces the whole buffer**, because
        // `ui::select_all_on_focus` selects it on `gained_focus` — no Backspace.
        frame(ctx, app, vec![egui::Event::Text("00FF00".into())]);
        frame(
            ctx,
            app,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        for _ in 0..2 {
            frame(ctx, app, Vec::new());
        }
    }

    /// **A hex typed over a Mixed row leaves each layer its own opacity.**
    ///
    /// ⚠️ **Typed rather than preset, and that is the fixture assertion's whole
    /// job.** `hex_row` seeds its buffer from `picker.hex` when that is `Some`,
    /// so a test that set `hex = Some("00FF00")` directly would commit from
    /// wherever focus happened to be — green, correct, and equally green with the
    /// click missing the field entirely. `hex` going `FF0000` → `00FF00` is what
    /// says the events reached *this* widget.
    ///
    /// ⚠️ **Flipped** by deleting `hex_row`'s `SelectionAll` branch so it falls
    /// through to `write_slot`: B comes back `[0, 255, 0, 255]` and the assertion
    /// on its alpha bites. **Two of the four assertions do not**, and both are
    /// still worth writing:
    ///
    /// - **`undo_depth` is 1 either way.** It is here to say the fix did not turn
    ///   one commit into two — which `edit_paints_all` plus `commit_edit` could
    ///   easily have done — not to catch the regression.
    /// - **The single-layer control is green under the flip**, unchanged. That is
    ///   the point of it: it turns *"the loss is the scope, not this field's
    ///   arithmetic"* from a sentence into a measurement. **A test with only the
    ///   control in it would have passed all along.**
    #[test]
    fn a_mixed_rows_hex_field_leaves_each_layer_its_own_opacity() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = two_rects(&ctx, (1.0, 0.4));
        app.session.selection.set(vec![a, b]);
        app.picker = Some(Picker::new(
            a,
            PaintSlot::SelectionAll(PaintTarget::Fill),
            egui::pos2(400.0, 300.0),
        ));
        type_a_green(&ctx, &mut app);

        assert_eq!(
            rgba8(&app, a),
            [0, 255, 0, 255],
            "A was opaque and stays so"
        );
        assert_eq!(
            rgba8(&app, b),
            [0, 255, 0, 102],
            "B keeps its 40% — the colour came from the field, the number did not"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            1,
            "one edit, one undo step (this does not catch the regression — the \
             wrong version is one commit too)"
        );

        // **The control**, and it is green either way: one layer, its own slot,
        // and its alpha survives because there is no scope to lose it to.
        let ctx = egui::Context::default();
        let (mut app, a, b) = two_rects(&ctx, (1.0, 0.4));
        app.session.selection.set(vec![b]);
        app.picker = Some(Picker::new(b, PaintSlot::Fill(0), egui::pos2(400.0, 300.0)));
        type_a_green(&ctx, &mut app);
        assert_eq!(
            rgba8(&app, b),
            [0, 255, 0, 102],
            "control: B goes green at 40%"
        );
        assert_eq!(rgba8(&app, a), [255, 0, 0, 255], "control: A is untouched");
    }

    /// **A bare click through the picker's hex field writes nothing** (§15 D730,
    /// `[S23.1-L1-05]`).
    ///
    /// Open the picker on a colour no hex string names, click once in the field,
    /// type nothing, leave. `hex_row` parsed its own unchanged text back and
    /// committed it: `parse_hex` gives `u8`s and `from_rgba8` re-expands them, so
    /// **`0.5` came back `0.5019608`** on every channel — and it cost an undo
    /// step. Anything off the 8-bit lattice is affected, which is anything the
    /// picker's own HSV plane produced.
    ///
    /// 🚨 **`inspector::paint_hex_field` has had this guard since §15 D517 and
    /// this copy never got it**, which is the finding's real content — but its
    /// *fix sketch* is no longer available. It says to delete this field and call
    /// `paint_hex_field`; since it was written, `hex_row` gained the
    /// `SelectionAll` branch (D540) and, more decisively, it edits **one stop of a
    /// gradient by `index`**, which `paint_hex_field` has no concept of. The two
    /// are not one control written twice any more; they are two controls that
    /// share a guard. **A fix sketch is a hypothesis about code the reviewer did
    /// not change**, and this one aged out between the review and the fix.
    ///
    /// ⚠️ **Compared as bytes, not as floats**, exactly as `paint_hex_field` does
    /// and for the reason its comment gives: the floats really do differ, and what
    /// has to match is what the *field* can express.
    ///
    /// **The typed control is the other half.** A guard that refused everything
    /// would pass the first three assertions.
    ///
    /// ⚠️ **Flip-check, run: `!mixed` inverted to `mixed`**, which makes the guard
    /// apply to the case it exempts and skip the case it is for. Red here at
    /// `[0.5019608, …]` against `[0.5, …]`, and
    /// `a_mixed_rows_hex_field_leaves_each_layer_its_own_opacity` above stays
    /// **green** — the exemption is unobservable from that test, so the two are
    /// asking different questions and both are needed.
    #[test]
    fn a_bare_click_through_the_hex_field_writes_nothing() {
        let ctx = egui::Context::default();
        // 0.5 is not on the 8-bit lattice: `hex_of` rounds it to `80` and
        // `from_rgba8` gives back 0.5019608.
        let (mut app, a, _b) = two_rects(&ctx, (1.0, 0.4));
        let grey = Color::new([0.5, 0.5, 0.5, 1.0]);
        app.session.commit(Transaction(vec![Operation::SetFills {
            id: a,
            fills: vec![Fill {
                brush: Brush::Solid(grey),
                visible: true,
            }],
        }]));
        app.session.selection.set(vec![a]);
        app.picker = Some(Picker::new(a, PaintSlot::Fill(0), egui::pos2(400.0, 300.0)));
        let before = match &app.session.doc.get(a).expect("node").paint().fills[0].brush {
            Brush::Solid(c) => c.components,
            other => panic!("solid, got {other:?}"),
        };
        assert_eq!(
            before[0], 0.5,
            "the fixture is off the 8-bit lattice, or this test is about nothing"
        );
        let depth = app.session.history.undo_depth();

        // Click in, click out. No text, no Enter.
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        for _ in 0..2 {
            frame(&ctx, &mut app, Vec::new());
        }
        frame(
            &ctx,
            &mut app,
            vec![
                egui::Event::PointerMoved(HEX_FIELD),
                button(HEX_FIELD, true),
            ],
        );
        frame(&ctx, &mut app, vec![button(HEX_FIELD, false)]);
        // ⚠️ **The click has to have landed in the field.** `hex_row` arms
        // `picker.hex` on focus, so this is the one observable that says the
        // events reached *this* widget — without it a click that missed would
        // make every assertion below pass for nothing.
        assert!(
            app.picker.as_ref().and_then(|p| p.hex.as_ref()).is_some(),
            "the click has to focus the hex field, or this test is about nothing"
        );
        // ⚠️ **`Enter`, not a click away, and the difference was measured.**
        // `hex_row` commits on `lost_focus()`, and its comment says Enter
        // triggers that too — so this is the same door. A click elsewhere *in the
        // picker* does **not** open it: driven at `(430, 380)`, on the SV plane,
        // `picker.hex` was still `Some` three frames later, so the field kept the
        // caret and the commit path was never reached. The first version of this
        // test did exactly that and was green **for that reason** rather than for
        // the fix. The `is_none()` assertion below is what caught it and is kept.
        frame(
            &ctx,
            &mut app,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        for _ in 0..3 {
            frame(&ctx, &mut app, Vec::new());
        }

        let after = match &app.session.doc.get(a).expect("node").paint().fills[0].brush {
            Brush::Solid(c) => c.components,
            other => panic!("solid, got {other:?}"),
        };
        assert!(
            app.picker.as_ref().and_then(|p| p.hex.as_ref()).is_none(),
            "PROBE: the field must actually have lost focus, or the commit path \
             is never reached and this test proves nothing"
        );
        assert_eq!(
            after, before,
            "a click that typed nothing must leave the colour alone — parsing the \
             field's own text back quantises it to the 8-bit lattice"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            depth,
            "and must not cost an undo step"
        );

        // The control: a colour that *is* typed still lands.
        let ctx = egui::Context::default();
        let (mut app, a, _b) = two_rects(&ctx, (1.0, 0.4));
        app.session.selection.set(vec![a]);
        app.picker = Some(Picker::new(a, PaintSlot::Fill(0), egui::pos2(400.0, 300.0)));
        type_a_green(&ctx, &mut app);
        assert_eq!(
            rgba8(&app, a),
            [0, 255, 0, 255],
            "control: a typed hex still writes, or the guard refuses everything"
        );
    }
}

#[cfg(test)]
mod radial_centre_tests {
    //! **A radial centre the document holds outside the shape survives being
    //! drawn** — §15 D552, `[S23.1-L1-01]`.
    //!
    //! SVG allows a radial gradient centred outside the shape it paints, and a
    //! highlight sitting off one edge is the reason anyone writes one. The picker's
    //! CX/CY fields are `pct = centre * 100.0` handed to a `DragValue` with
    //! `.range(0.0..=100.0)`, and egui's `clamp_existing_to_range` defaults to
    //! `true` and is applied every frame — so the field painted `100` for a centre
    //! stored at `1.5`, marked its response changed, and `valve_slot` committed the
    //! rewrite on the next `engaged` transition. One bare click through the field
    //! moved the gradient 50 units and spent an undo step.
    //!
    //! This is the same door as `ui::value_field_f64`'s (§15 D425) at the two
    //! numeric fields in the app that go through neither `value_field` nor
    //! `bare_drag_value`. It is fixed by `ui::plain_drag_value`, which carries the
    //! decision so the next field added here inherits it.
    //!
    //! (Plain backticks, not `[links]` — §15 D319's convention.)

    use super::*;
    use crate::app::OndinApp;
    use crate::theme;
    use ondin_core::kurbo::{Affine, Point, Size};
    use ondin_core::peniko::Color;
    use ondin_core::{
        Document, Fill, GradientBrush, IdSource, NodeId, NodeKind, Operation, Transaction, peniko,
    };

    const SCREEN: egui::Vec2 = egui::vec2(1200.0, 900.0);
    const BOX: Size = Size::new(100.0, 100.0);
    /// Measured with a throwaway probe that dumped every text galley in the frame:
    /// with the picker spawned at `(400, 300)` the CX row's label sits at
    /// `(463, 360)` and its number at `(491, 358.5)`, so this lands inside the
    /// field rather than on the label.
    const CX_FIELD: egui::Pos2 = egui::pos2(495.0, 360.0);

    /// A 100×100 rect filled with a radial gradient centred at `cx` of its width.
    fn radial_rect(ctx: &egui::Context, cx: f64) -> (OndinApp, NodeId) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x5A1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let a = ids.mint();
        let centred = Brush::Gradient(GradientBrush {
            gradient: peniko::Gradient {
                kind: peniko::GradientKind::Radial(peniko::RadialGradientPosition {
                    start_center: Point::new(50.0, 50.0),
                    start_radius: 0.0,
                    end_center: Point::new(50.0, 50.0),
                    end_radius: 50.0,
                }),
                stops: [
                    peniko::ColorStop {
                        offset: 0.0,
                        color: Color::new([1.0, 0.0, 0.0, 1.0]).into(),
                    },
                    peniko::ColorStop {
                        offset: 1.0,
                        color: Color::new([0.0, 0.0, 1.0, 1.0]).into(),
                    },
                ][..]
                    .into(),
                ..Default::default()
            },
            transform: Affine::IDENTITY,
            opacity: 1.0,
        });
        // Through the panel's own writer, so the fixture is a brush the app can
        // actually produce rather than one hand-assembled to suit the assertion.
        let brush = crate::panels::paint::with_radial_centre(&centred, cx, 0.5, BOX);
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: a,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: BOX,
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id: a,
                fills: vec![Fill {
                    brush,
                    visible: true,
                }],
            },
        ]))
        .expect("build");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![a]);
        app.picker = Some(Picker::new(a, PaintSlot::Fill(0), egui::pos2(400.0, 300.0)));
        (app, a)
    }

    fn centre(app: &OndinApp, id: NodeId) -> (f64, f64) {
        crate::panels::paint::radial_centre(
            &app.session.doc.get(id).expect("node").paint().fills[0].brush,
            BOX,
        )
        .expect("the fixture's fill is a radial gradient")
    }

    /// Every text galley the frame drew, in paint order.
    fn galleys(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) -> Vec<String> {
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
                events,
                ..Default::default()
            },
            |ui| app.picker_ui(ui.ctx()),
        );
        out.shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    /// Click into the CX field, type nothing, and then leave it.
    ///
    /// ⚠️ **The `Tab` is not decoration and the test is vacuous without it.** The
    /// commit is `app::edit_valve`'s `engaged → !engaged` transition, and a click
    /// leaves the field *focused* — so `engaged` stays true and nothing is written
    /// while the caret is still in there. Measured, not reasoned: with the fix
    /// reverted and no `Tab`, the centre came back `(1.5, 0.5)` at `undo_depth` 0,
    /// which is the *correct* answer arrived at by never reaching the code under
    /// test. Adding the `Tab` makes the same flip report `(1.5 → 1.0, 0.5)` at
    /// depth 1, which is `[S23.1-L1-01]`'s own measured table.
    ///
    /// `Tab` rather than a click elsewhere because a click outside the popover
    /// closes it, and a closed picker draws no rows to assert on.
    fn bare_click_and_leave(ctx: &egui::Context, app: &mut OndinApp) {
        let button = |pressed| egui::Event::PointerButton {
            pos: CX_FIELD,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let _ = galleys(
            ctx,
            app,
            vec![egui::Event::PointerMoved(CX_FIELD), button(true)],
        );
        let _ = galleys(ctx, app, vec![button(false)]);
        let _ = galleys(
            ctx,
            app,
            vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        for _ in 0..3 {
            let _ = galleys(ctx, app, Vec::new());
        }
    }

    /// The number drawn in the row labelled `label`, which is the galley after it.
    fn field_reads(drawn: &[String], label: &str) -> String {
        let at = drawn
            .iter()
            .position(|t| t == label)
            .unwrap_or_else(|| panic!("no `{label}` row in the frame: {drawn:?}"));
        drawn
            .get(at + 1)
            .unwrap_or_else(|| panic!("`{label}` drew no number: {drawn:?}"))
            .clone()
    }

    /// **The field shows what the document holds, and a bare click leaves it
    /// there.**
    ///
    /// ⚠️ **The galley assertion is the one that names the bug.** The centre
    /// surviving the click is the *consequence*; what is wrong before any click is
    /// that the card and the document disagree about the number, and the card is
    /// the one that is wrong. A test asserting only the document would pass against
    /// a version that painted `100` and merely declined to commit it — which is a
    /// control lying about the value under it, still.
    ///
    /// ⚠️ **The in-range control is what says this is about the clamp and not about
    /// the click.** Without it, a fix that stopped `valve_slot` committing
    /// altogether would pass — and the picker's own `pointer_slot` tests are the
    /// evidence that committing is wanted.
    ///
    /// ⚠️ **Flip-check, run: `ui::plain_drag_value` reverted to
    /// `egui::DragValue::new` at the CX/CY site.** Fails on the **galley**
    /// assertion, `"100"` where `"150"` was, which is the reported symptom — and
    /// not on the centre assertion, which was the predicted site. The reason is
    /// worth keeping: the galley is wrong on the *first* frame the panel draws,
    /// before any event, so it bites two assertions earlier than the commit does.
    /// Re-run with the galley assertion relaxed, **the centre assertion bites too**,
    /// at `(1.0, 0.5)` — `[S23.1-L1-01]`'s own measured number.
    ///
    /// 🚨 **And the first version of that flip is what found the second half of the
    /// bug, which the finding's fix sketch does not name.** With
    /// `plain_drag_value` in place the galley read `150` and the centre *still*
    /// came back `(1.0, 0.5)`: `radial_controls` had a **second** clamp,
    /// `let f = (pct / 100.0).clamp(0.0, 1.0)`, and `valve_slot` commits whatever
    /// `next` holds on the valve transition whether the user moved anything or not.
    /// So the one-line fix `[S14.4-L1-02]` established — the fix sketch's whole
    /// content — makes the *field* honest and leaves the *document* being rewritten,
    /// with the galley the only thing that got better. **Both clamps had to go**,
    /// and only running the flip said so.
    ///
    /// ⚠️ **A flip that does *not* bite here, which says where the teeth are:**
    /// `.range(0.0..=100.0)` deleted from the field. This test stays green, because
    /// with the clamp gone the range bounds only what a drag or a keystroke may
    /// reach and neither happens here. That is not a gap — it is
    /// `a_number_typed_past_the_fields_range_is_still_bounded_by_it` below, which
    /// the same flip fails at `5.0`.
    #[test]
    fn a_radial_centre_outside_the_shape_is_shown_and_not_rewritten_by_a_bare_click() {
        let ctx = egui::Context::default();
        let (mut app, a) = radial_rect(&ctx, 1.5);
        assert_eq!(
            centre(&app, a).0,
            1.5,
            "the fixture has to reach the state: an imported centre outside its own \
             shape, which `svg_in` produces with `skipped` and `approximated` both \
             empty"
        );

        // **Three frames before reading any ink**, because the first frame of a
        // headless `Context` has no fonts and the picker's placement settles over
        // two (`the_picker_opens_where_it_was_spawned_on_the_very_first_frame` is
        // about the *rect*, not the galleys). A read on frame one comes back with
        // no shapes at all, which reads exactly like a missing row.
        for _ in 0..3 {
            let _ = galleys(&ctx, &mut app, Vec::new());
        }
        let drawn = galleys(&ctx, &mut app, Vec::new());
        assert_eq!(
            field_reads(&drawn, "CX"),
            "150",
            "the field paints the document's number. `100` here is egui's \
             `clamp_existing_to_range` rewriting a stored value it has no business \
             touching, and the card is then lying about the gradient under it"
        );

        bare_click_and_leave(&ctx, &mut app);
        assert_eq!(
            centre(&app, a),
            (1.5, 0.5),
            "a click that typed nothing moved the gradient's centre 50 units, \
             permanently"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            0,
            "and it must not have spent an undo step doing it — the widget's own \
             rewrite reported as a user edit is what `edit_valve` then commits"
        );

        // **The control**: a centre inside the shape, the identical click.
        let ctx = egui::Context::default();
        let (mut app, a) = radial_rect(&ctx, 0.5);
        for _ in 0..3 {
            let _ = galleys(&ctx, &mut app, Vec::new());
        }
        let drawn = galleys(&ctx, &mut app, Vec::new());
        assert_eq!(field_reads(&drawn, "CX"), "50", "control: shown as stored");
        bare_click_and_leave(&ctx, &mut app);
        assert_eq!(
            centre(&app, a),
            (0.5, 0.5),
            "control: an in-range centre is untouched by the same click"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            0,
            "control: and spends no undo step either"
        );
    }

    /// **The range still bounds what a keystroke may reach**, which is the half of
    /// §15 D552 that the removed `.clamp(0.0, 1.0)` looked like it was doing.
    ///
    /// ⚠️ **This exists because the fix deleted a clamp, and a deleted bound has
    /// to be shown to have been redundant rather than argued to be.** D425's
    /// paragraph says `.range(…)` *"bounds what a gesture or a keystroke can
    /// reach; it does not rewrite what the document already holds"* — so typing
    /// `500` into a field ranged `0..=100` must still land at `100`. Measured here
    /// rather than taken from that paragraph, because the paragraph is about
    /// `value_field_f64` and this field goes through `ui::plain_drag_value`.
    ///
    /// ⚠️ **Flip-check, run: `.range(0.0..=100.0)` deleted from the CX/CY field.**
    /// Fails here with `5.0` — a radial centre five times the shape's width, typed
    /// straight through into the document — and **leaves the test above green**,
    /// which is what says the two assertions are about different halves and
    /// neither is a restatement of the other.
    ///
    /// **What is *not* asserted, deliberately:** the drag path. A `DragValue`'s
    /// drag is `Scrub::settle`'s business at every other field in the app and
    /// egui's own at this one, and driving a synthetic drag through a popover that
    /// also drags its preview chip is a fixture with two answers. The keystroke is
    /// the road a user takes to a number like 500.
    #[test]
    fn a_number_typed_past_the_fields_range_is_still_bounded_by_it() {
        let ctx = egui::Context::default();
        let (mut app, a) = radial_rect(&ctx, 0.5);
        for _ in 0..3 {
            let _ = galleys(&ctx, &mut app, Vec::new());
        }

        let button = |pressed| egui::Event::PointerButton {
            pos: CX_FIELD,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let _ = galleys(
            &ctx,
            &mut app,
            vec![egui::Event::PointerMoved(CX_FIELD), button(true)],
        );
        let _ = galleys(&ctx, &mut app, vec![button(false)]);
        // One `Event::Text` replaces the whole buffer — `ui::select_all_on_focus`
        // selects it on `gained_focus`, so no Backspace.
        let _ = galleys(&ctx, &mut app, vec![egui::Event::Text("500".into())]);
        let _ = galleys(
            &ctx,
            &mut app,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        for _ in 0..3 {
            let _ = galleys(&ctx, &mut app, Vec::new());
        }

        assert_eq!(
            centre(&app, a).0,
            1.0,
            "500% typed into a field ranged 0..=100 has to land at the range's end. \
             `5.0` here means the range bounds nothing and deleting the clamp gave \
             the keystroke a free run at the document"
        );
    }
}

#[cfg(test)]
mod grid_orphan_tests {
    //! **A grid picker goes away when the card's row does** — §15 D553,
    //! `[S23.1-L1-04]`.
    //!
    //! `picker_ui`'s orphan match governs every slot in the app, and `Grid(i)` fell
    //! into its catch-all `!selection.contains(node)`. The anchor is still
    //! contained after a Shift-click widens the selection, so the picker survived a
    //! widening the *card* did not: `shared_grids` answers `None` the moment two
    //! frames' grid lists differ and the Layout grid card draws a sentence instead
    //! of rows, while the picker went on floating over A's red. A hex typed into it
    //! then recoloured **B's** grid, on a row that had stopped being drawn.
    //!
    //! This is the `Effect(i)` arm's argument verbatim (`[S14.5-L1-01]`, D476) at
    //! the fourth slot, and fixing that one did not reach this one — its arm names
    //! `PaintSlot::Effect(_)` and nothing else.
    //!
    //! (Plain backticks, not `[links]` — §15 D319's convention.)

    use super::*;
    use crate::app::OndinApp;
    use crate::theme;
    use ondin_core::kurbo::Size;
    use ondin_core::layout::{GridAxis, LayoutGrid};
    use ondin_core::peniko::Color;
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    const SCREEN: egui::Vec2 = egui::vec2(1200.0, 900.0);
    /// `mixed_hex_tests`' measured hit band for the hex field, at the same spawn.
    /// Repeated rather than shared because the two modules are independent
    /// fixtures and a constant reaching across them would tie one's spawn to the
    /// other's.
    const HEX_FIELD: egui::Pos2 = egui::pos2(450.0, 524.0);

    fn grid(count: u32, color: Color) -> LayoutGrid {
        LayoutGrid {
            count,
            color,
            ..LayoutGrid::new(GridAxis::Columns)
        }
    }

    /// Two artboards with **disagreeing** grid lists, plus a rect that has none.
    fn two_frames(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId, NodeId) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x6121);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (a, b, r) = (ids.mint(), ids.mint(), ids.mint());
        let node = |id, kind| Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind,
            transform: None,
            name: None,
        };
        doc.apply(&Transaction(vec![
            node(
                a,
                NodeKind::Artboard {
                    size: Size::new(400.0, 300.0),
                },
            ),
            node(
                b,
                NodeKind::Artboard {
                    size: Size::new(400.0, 300.0),
                },
            ),
            node(
                r,
                NodeKind::Rect {
                    size: Size::new(20.0, 20.0),
                    corner_radii: Default::default(),
                },
            ),
            Operation::SetLayoutGrids {
                id: a,
                grids: vec![grid(5, Color::new([1.0, 0.0, 0.0, 1.0]))],
            },
            Operation::SetLayoutGrids {
                id: b,
                grids: vec![grid(10, Color::new([0.0, 0.0, 1.0, 1.0]))],
            },
        ]))
        .expect("build");
        app.session.adopt_document(doc, None);
        (app, a, b, r)
    }

    fn frame(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) {
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
                events,
                ..Default::default()
            },
            |ui| app.picker_ui(ui.ctx()),
        );
    }

    fn grid_rgba8(app: &OndinApp, id: NodeId) -> [u8; 4] {
        app.session.doc.get(id).expect("node").grids()[0]
            .color
            .to_rgba8()
            .to_u8_array()
    }

    /// Click into the hex field, replace its text with `00FF00`, press `Enter`.
    /// `mixed_hex_tests::type_a_green`, repeated for that module's reason.
    fn type_a_green(ctx: &egui::Context, app: &mut OndinApp) {
        for _ in 0..2 {
            frame(ctx, app, Vec::new());
        }
        let button = |pressed| egui::Event::PointerButton {
            pos: HEX_FIELD,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(
            ctx,
            app,
            vec![egui::Event::PointerMoved(HEX_FIELD), button(true)],
        );
        frame(ctx, app, vec![button(false)]);
        frame(ctx, app, vec![egui::Event::Text("00FF00".into())]);
        frame(
            ctx,
            app,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        for _ in 0..2 {
            frame(ctx, app, Vec::new());
        }
    }

    /// **Widening the selection past agreement closes the grid picker**, so the
    /// hex that follows reaches nothing.
    ///
    /// ⚠️ **The damage assertion is `b`'s colour and not the picker being `None`.**
    /// The picker closing is the *mechanism*; what the user loses is a grid colour
    /// on a frame whose row is not on screen. A test asserting only
    /// `app.picker.is_none()` would pass against a fix that closed the picker for
    /// the wrong reason — the anchor being dropped, say — so the hex is typed
    /// anyway and B is read back off the document.
    ///
    /// ⚠️ **Two controls, and each rules out a different wrong fix.** *A alone*
    /// says the fixture and the write path work at all — without it, a version that
    /// closed the picker unconditionally would pass. *A plus the rect* says the
    /// question asked is `frame_subjects`' and not `selection.ids()`': a Shift-clicked
    /// rectangle cannot make two frames' grids disagree, and closing the picker
    /// over it would be a regression this arm could easily have introduced.
    ///
    /// ⚠️ **Flip-check, run: the `PaintSlot::Grid(_)` arm deleted so the slot falls
    /// back into the catch-all.** Fails on B's colour with `[0, 255, 0, 255]` where
    /// `[0, 0, 255, 255]` was — the finding's own measurement — and the *track
    /// count* assertion stays green, which is the finding's point that `Grid`'s
    /// damage is narrower than `Effect`'s because its arm reads each subject's own
    /// list. Both controls stay green under the flip, as they must.
    #[test]
    fn widening_past_agreement_closes_the_grid_picker_before_it_can_write() {
        let ctx = egui::Context::default();
        let (mut app, a, b, _r) = two_frames(&ctx);
        app.session.selection.set(vec![a]);
        app.picker = Some(Picker::new(a, PaintSlot::Grid(0), egui::pos2(400.0, 300.0)));
        for _ in 0..2 {
            frame(&ctx, &mut app, Vec::new());
        }
        assert!(
            app.picker.is_some(),
            "the fixture has to reach the state: over one frame the card draws its \
             row and the picker stays up"
        );

        // The Shift-click. `shared_grids` now answers `None` — 5 tracks against
        // 10 — so the card stops drawing rows.
        app.session.selection.set(vec![a, b]);
        assert!(
            app.shared_grids(&app.frame_subjects()).is_none(),
            "and the fixture's two frames have to actually disagree, or the card \
             would still be drawing the row this is about"
        );
        type_a_green(&ctx, &mut app);

        assert_eq!(
            grid_rgba8(&app, b),
            [0, 0, 255, 255],
            "B's grid was never on screen as a row and its colour is now A's"
        );
        assert_eq!(
            app.session.doc.get(b).unwrap().grids()[0].count,
            10,
            "and its track count survives either way — `Grid`'s arm reads each \
             subject's own list, which is what makes this narrower than `Effect`'s"
        );
        assert!(
            app.picker.is_none(),
            "the picker has to have gone, not merely failed to write"
        );

        // **Control 1**: one frame, the identical hex. The write must land.
        let ctx = egui::Context::default();
        let (mut app, a, _b, _r) = two_frames(&ctx);
        app.session.selection.set(vec![a]);
        app.picker = Some(Picker::new(a, PaintSlot::Grid(0), egui::pos2(400.0, 300.0)));
        type_a_green(&ctx, &mut app);
        assert_eq!(
            grid_rgba8(&app, a),
            [0, 255, 0, 255],
            "control: over one frame the hex recolours that frame's grid"
        );

        // **Control 2**: a rect joins the selection. It has no grids, so
        // `frame_subjects` filters it out and the frames still agree.
        let ctx = egui::Context::default();
        let (mut app, a, _b, r) = two_frames(&ctx);
        app.session.selection.set(vec![a, r]);
        app.picker = Some(Picker::new(a, PaintSlot::Grid(0), egui::pos2(400.0, 300.0)));
        for _ in 0..2 {
            frame(&ctx, &mut app, Vec::new());
        }
        assert!(
            app.picker.is_some(),
            "control: a Shift-clicked rectangle cannot make two frames' grids \
             disagree, so the card is still drawing the row and the picker must \
             still be up"
        );
    }

    /// **A palette chip whose colour is already in the slot costs no undo step** —
    /// `[S23.1-L1-03]`, and it was closed by §15 D428 before anybody wrote this.
    ///
    /// ⚠️ **Pinned rather than fixed, which is why it is worth having.** The
    /// finding measured `undo_depth 0 → 1` for a click on the chip the picker
    /// itself rings — the ring means *"this is the one you have"* — with the brush
    /// read back byte-identical. Its own fix sketch named G16's single guard,
    /// `Transaction::changes_nothing` at `session::commit_inner`, and that landed
    /// as D428. Nothing since then has said this door is covered, and the door is
    /// three functions away from the guard.
    ///
    /// ⚠️ **Driven at `write_slot` rather than through a click on the chip, and
    /// the reason is a fixture that would rot.** The palette row's chips are laid
    /// out from the *document's own* colours, so a hit position depends on how
    /// many distinct colours the fixture has and on the row's wrapping — a
    /// coordinate that changes when the palette does, which is
    /// `atomic::leftover_temps`' warning in another dress. `write_slot` is the
    /// single line the click handler runs (`document_swatches`:
    /// `self.write_slot(node, slot, paint::with_stops(brush, stops))`), so this
    /// asserts the whole of what the click does and none of where it is.
    ///
    /// ⚠️ **Flip-check, run: the `tx.changes_nothing(&self.doc)` guard removed
    /// from `commit_inner`.** Fails on the first assertion at depth 1, and — the
    /// half that says the test is not vacuous — the *second* assertion stays
    /// green, so a version that refused every write would not pass.
    #[test]
    fn a_palette_chip_already_in_the_slot_costs_no_undo_step() {
        use ondin_core::Fill;
        let ctx = egui::Context::default();
        let (mut app, a, _b, _r) = two_frames(&ctx);
        let red = Color::new([1.0, 0.0, 0.0, 1.0]);
        app.session
            .doc
            .apply(&Transaction(vec![Operation::SetFills {
                id: a,
                fills: vec![Fill {
                    brush: Brush::Solid(red),
                    visible: true,
                }],
            }]))
            .expect("fill the frame");
        app.session.selection.set(vec![a]);
        assert_eq!(
            app.session.history.undo_depth(),
            0,
            "the fixture has to start with an empty history, or the assertion \
             below is about whatever built it"
        );

        // Exactly what the ringed chip's click handler does: the colour it
        // already has, back into the slot it came from.
        app.write_slot(a, PaintSlot::Fill(0), Brush::Solid(red));
        assert_eq!(
            app.session.history.undo_depth(),
            0,
            "clicking the chip the picker rings dirties the document — arming \
             autosave and the crash snapshot — and puts a step in the history \
             that undoes to the picture it undid from"
        );

        // **The control**: a different colour through the same door has to land,
        // or the assertion above is satisfied by a write that never works.
        let green = Color::new([0.0, 1.0, 0.0, 1.0]);
        app.write_slot(a, PaintSlot::Fill(0), Brush::Solid(green));
        assert_eq!(
            app.session.history.undo_depth(),
            1,
            "control: a chip the picker did *not* ring still commits"
        );
        match &app.session.doc.get(a).unwrap().paint().fills[0].brush {
            Brush::Solid(c) => assert_eq!(*c, green, "control: and lands"),
            other => panic!("control: expected a solid fill, got {other:?}"),
        }
    }

    /// **A chip shows the ramp's opacity; the stop bar shows the stops** (§15 D773).
    ///
    /// 🚨 **§15 D767 introduced this gap and the entire suite stayed green**, which
    /// is why the assertion exists rather than the fix alone. Until the ramp's
    /// opacity moved off the stops, `stops_of` *was* what the paint looked like, so
    /// every chip in the app was correct for free; the moment it became a stored
    /// field, every reader of the old derivation became a bug. **A derived value
    /// that becomes a stored one is invisible to every gate in this project** — the
    /// types are unchanged, the callers still compile, and the picture is wrong.
    ///
    /// ⚠️ **Both halves, because the defaulting is the decision.** An assertion that
    /// only checked the chip would be satisfied by folding the opacity into
    /// `stop_colours` as well, which dims the handles the user is dragging.
    ///
    /// **Flip, run:** making `ramp` return `stop_colours(brush)` unchanged — the
    /// pre-D773 body — fails at the chip assertion, whose message reports the faded
    /// chip reading the control's own `255`. Predicted correctly. ⚠️ **No pair of
    /// numbers here**, because this is an `assert!` rather than an `assert_eq!`:
    /// the failure names what the chip *did* read and never the value it should
    /// have, so a "`255` against `76`" gloss describes a message the run does not
    /// print. The assertion is deliberately a `<` rather than an equality — the
    /// rounding of `gamma_multiply` is egui's business and not this test's.
    #[test]
    fn a_faded_gradient_dims_its_chip_and_not_its_stop_bar() {
        let brush = crate::panels::paint::default_brush(
            crate::panels::paint::PaintKind::Linear,
            vec![
                (0.0, Color::from_rgba8(255, 0, 0, 255)),
                (1.0, Color::from_rgba8(0, 0, 255, 255)),
            ],
            ondin_core::kurbo::Size::new(100.0, 100.0),
        );
        let faded = crate::panels::paint::with_alpha(&brush, 0.3);

        let opaque_chip = ramp(&brush)[0].1.a();
        let faded_chip = ramp(&faded)[0].1.a();
        assert_eq!(opaque_chip, 255, "the control: an untouched ramp is opaque");
        assert!(
            faded_chip < opaque_chip && faded_chip > 0,
            "a 30% gradient's chip has to look 30% — it draws beside a canvas \
             that does, and one document drawn two ways in one frame is §15 \
             D564's own defect (got {faded_chip})"
        );

        assert_eq!(
            stop_colours(&faded)[0].1.a(),
            255,
            "and the stop bar stays raw — it is the editing surface for these \
             stops and sits under the slider that would be fading it"
        );
    }

    /// **The knob hangs 7pt past each end of its row, and the card's padding is
    /// what catches it** (§15 D766).
    ///
    /// 🚨 **Nothing in this module asserted a single dimension of the sliders**
    /// before this — eleven tests covering chips, placement, hex fields, radial
    /// centres and palettes, and none touching `slider_track` or `slider_knob`. So
    /// §15 D673 could describe the geometry wrongly for six days (it says this
    /// slider *"is given room where `ui::slider`'s overhangs"*, true only of the
    /// vertical axis) and nothing disagreed.
    ///
    /// The vertical half is genuinely bounded — 7.0 of reach in a 7.5 half-row. The
    /// horizontal half is not bounded at all: `slider_knob` puts the centre at
    /// `track.min.x + width * t` with no inset, so `t = 0` and `t = 1` reach 7pt
    /// outside. It fits because `PAD` is 11, which is a fact about the *card*
    /// rather than about the slider, and is exactly the kind of accidental clearance
    /// that a later layout change spends without noticing.
    ///
    /// **Flip, run:** dropping `PAD` to 6 fails at the horizontal assertion — the
    /// knob would be drawn outside the card. Predicted correctly. Growing the knob's
    /// outer stroke radius from 6.5 to 8.0 fails the vertical one, which is the
    /// change somebody adjusting the knob's weight would actually make.
    ///
    /// ⚠️ **A first draft of this asserted constants against constants** — it
    /// recomputed the knob's reach as a literal and compared it to `TRACK_H + 6.0`
    /// and `PAD`, which clippy's `assertions_on_constants` reported and which was
    /// right to report: it restated arithmetic rather than measuring the drawing,
    /// so it would have passed against any `slider_knob` at all. The ink is read
    /// out of the painter now, which is `ui.rs`'s own slider test's method and the
    /// reason that one has teeth.
    #[test]
    fn the_alpha_knob_stays_inside_the_cards_padding_at_both_ends() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let track = egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(120.0, TRACK_H));

        // Both ends, because the overhang is symmetric and a knob clamped at one
        // end only is a plausible wrong version.
        for (label, t) in [("t = 0", 0.0_f32), ("t = 1", 1.0)] {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 200.0),
                    )),
                    ..Default::default()
                },
                |ui| slider_knob(ui.painter(), track, t, egui::Color32::WHITE),
            );
            let ink = out
                .shapes
                .iter()
                .map(|c| c.shape.visual_bounding_rect())
                .filter(|r| r.is_finite() && r.is_positive())
                .reduce(|a, b| a.union(b))
                .expect("the knob painted something");

            let row_half = (TRACK_H + 6.0) * 0.5;
            let centre_y = track.center().y;
            let up = centre_y - ink.min.y;
            let down = ink.max.y - centre_y;
            assert!(
                up <= row_half && down <= row_half,
                "{label}: the knob left its row vertically — {up} up and {down} \
                 down against a {row_half} half-row. The `+ 6.0` in `slider_track` \
                 is what buys this clearance (§15 D766)"
            );

            let over = (track.min.x - ink.min.x).max(ink.max.x - track.max.x);
            assert!(
                over > 0.0,
                "{label}: the knob no longer overhangs at all — if it has been \
                 clamped, this test and `slider_track`'s doc both need rewriting \
                 rather than deleting"
            );
            assert!(
                over <= PAD,
                "{label}: the knob hangs {over}pt past the track against {PAD}pt \
                 of card padding, so it is drawn outside the card. `slider_knob` \
                 insets nothing — `PAD` is the only thing holding it in (§15 D766)"
            );
        }
    }
}

#[cfg(test)]
mod text_colour_route_tests {
    //! **How the picker reaches a text layer's colour, and which valve arm it
    //! does not use** (§15 D802).
    //!
    //! Written while trying to cover `char_valve`'s third arm, which the roadmap
    //! held as *"writable now and unwritten"*. It is not writable from here: the
    //! controls §15 D523 kept that arm for do not reach it. What is here instead
    //! is the route they *do* take, which had no test either.
    use super::*;
    use crate::app::OndinApp;
    use crate::theme;
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    const SCREEN: egui::Vec2 = egui::vec2(1200.0, 900.0);

    fn frame(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) {
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SCREEN)),
                events,
                ..Default::default()
            },
            |ui| app.picker_ui(ui.ctx()),
        );
    }

    fn text_app(ctx: &egui::Context) -> (OndinApp, NodeId) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x7E5);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Text {
                content: "hello".into(),
                style: Box::default(),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing: ondin_core::TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("a text node");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        // Without this the picker orphans itself on its first frame: `TextColor`
        // is only *shown* while the Type panel's Character tab is up, so a picker
        // left over it would be editing something not on screen.
        app.type_menu = Some(crate::app::TypeTab::Character);
        app.picker = Some(Picker::new(
            id,
            PaintSlot::TextColor,
            egui::pos2(400.0, 300.0),
        ));
        (app, id)
    }

    fn text_rgba(app: &OndinApp, id: NodeId) -> Option<[u8; 4]> {
        match app.session.doc.get(id)?.kind() {
            NodeKind::Text { style, .. } => style.color.map(|c| c.to_rgba8().to_u8_array()),
            _ => None,
        }
    }

    /// **A click on the picker's plane sets a text layer's colour, and it does
    /// not go through `char_valve` at all** (§15 D802).
    ///
    /// The route is `hue_slider`/`sv_plane` → `write_stop_colour` →
    /// `pointer_slot`, and the first thing `pointer_slot` does after the
    /// cancelled-gesture bail is `if resp.clicked() { self.write_slot(…);
    /// return; }`. **A click never reaches `valve_slot`**, so it never reaches
    /// `char_valve`; a *drag* does, as the engaged arm, and its release as the
    /// falling edge.
    ///
    /// 🚨 **Which makes `char_valve`'s third arm unreachable from the controls
    /// §15 D523 kept it for.** That entry's reason reads *"without it a click on
    /// the hue strip would commit nothing, ever"*. Measured by instrumenting all
    /// three arms and driving this very click: `char_valve` is entered with
    /// `changed=false dragged=false focus=false lost_focus=false` on every frame,
    /// so no arm fires — and the colour commits anyway, through `write_slot`, one
    /// call earlier. Flipping the third arm out entirely leaves this test green.
    /// **The arm's users are not the ones named**, which is why removing it left
    /// 1,042 tests green and why the roadmap's "write a test for it" is the wrong
    /// next move: what is owed first is whether anything reaches it.
    ///
    /// ⚠️ **The fixture is two things, and the picker closes itself without the
    /// second.** A text node selected is not enough: `TextColor` orphans unless
    /// `type_menu` is on the Character tab, because the slot is only *shown*
    /// there and a picker over an unshown control is what that guard prevents.
    /// Without it `app.picker` is `None` by the second frame and every click in a
    /// 5 × 66 sweep landed on nothing — which reads exactly like "the click does
    /// not work" and is the fixture not being in the state the test is about.
    ///
    /// ⚠️ **The coordinate is measured, not derived.** With the picker spawned at
    /// (400, 300), (500, 344) is on the saturation/value plane. The sweep that
    /// found it also says what is *not* there: no column in x ∈ {420…600} over
    /// y ∈ [280, 800] ever moved the hue off red, so the plane is what this
    /// reaches and the hue strip's own band is still unlocated.
    #[test]
    fn a_click_on_the_picker_writes_a_text_layers_colour() {
        let ctx = egui::Context::default();
        let (mut app, id) = text_app(&ctx);
        frame(&ctx, &mut app, Vec::new());
        frame(&ctx, &mut app, Vec::new());

        assert!(
            app.picker.is_some(),
            "the fixture is not in the state this test is about — `TextColor` \
             orphans the picker unless the Character tab is up, and a closed \
             picker answers every click the same way"
        );
        assert_eq!(
            text_rgba(&app, id),
            None,
            "the layer starts with no colour of its own, so anything below is \
             this click's doing"
        );
        let depth = app.session.history.undo_depth();

        let at = egui::pos2(500.0, 344.0);
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        frame(&ctx, &mut app, vec![button(true)]);
        frame(&ctx, &mut app, vec![button(false)]);
        frame(&ctx, &mut app, Vec::new());

        assert_eq!(
            text_rgba(&app, id),
            Some([255, 152, 152, 255]),
            "the click writes the colour under it"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            depth + 1,
            "and it is one undo step, not a preview left on the floor"
        );
    }
}
